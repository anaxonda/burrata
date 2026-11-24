//! Read-only Jellyfin API for an authenticated account's music library.

use crate::subsonic_client::{
    canonical_endpoint, canonical_username, is_single_byte_range, server_http, Song,
    MAX_COVER_BYTES,
};
use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, RANGE},
    RequestBuilder, Response, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize};
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_PAGE: usize = 1000;
const UNREACHABLE: &str = "Could not reach the Jellyfin server.";
const BAD_RESPONSE: &str = "The Jellyfin server returned invalid data.";
const UNAVAILABLE: &str = "The Jellyfin item is unavailable.";

#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    endpoint: String,
    user_id: String,
    account_identity: String,
    authorization: HeaderValue,
}

impl Client {
    pub(crate) async fn connect_with_device_id(
        endpoint: &str,
        username: &str,
        password: String,
        device_id: &str,
    ) -> Result<Self, String> {
        let endpoint = canonical_endpoint(endpoint).map_err(|_| "Enter a Jellyfin HTTPS URL or a private HTTP URL without credentials, query or fragment.")?;
        let username = canonical_username(username).map_err(|_| "Enter a Jellyfin username.")?;
        if password.len() > 4096 {
            return Err("Enter a valid Jellyfin password.".into());
        }
        let http = server_http(&endpoint).await.map_err(|_| UNREACHABLE)?;
        let auth = authorization(device_id, None)?;
        let login: LoginResponse = metadata(
            http.post(format!("{endpoint}/Users/AuthenticateByName"))
                .header(AUTHORIZATION, auth)
                .json(&serde_json::json!({"Username": username, "Pw": password})),
        )
        .await?;
        let user_id = jellyfin_id(&login.user.id).ok_or(BAD_RESPONSE)?;
        let account_identity = format!("{}:{user_id}", login.server_id.as_deref().and_then(jellyfin_id).unwrap_or_default());
        let authorization = authorization(device_id, Some(&login.access_token))?;
        Ok(Self {
            http,
            endpoint,
            user_id,
            account_identity,
            authorization,
        })
    }

    pub(crate) fn account_identity(&self) -> &str { &self.account_identity }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http
            .get(format!("{}{path}", self.endpoint))
            .header(AUTHORIZATION, self.authorization.clone())
    }

    pub(crate) async fn song_catalog_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Song>, usize), String> {
        let limit = limit.clamp(1, MAX_PAGE);
        let page: ItemPage = metadata(
            self.get("/Items")
                .query(&[
                    ("userId", self.user_id.as_str()),
                    ("includeItemTypes", "Audio"),
                    ("recursive", "true"),
                    ("fields", "Genres,ProviderIds"),
                    ("enableUserData", "false"),
                    ("enableImages", "true"),
                    ("enableImageTypes", "Primary"),
                    ("sortBy", "SortName,DateCreated"),
                    ("sortOrder", "Ascending"),
                    ("enableTotalRecordCount", "true"),
                ])
                .query(&[("startIndex", offset), ("limit", limit)]),
        )
        .await?;
        // Offsets count raw provider records, including filtered placeholders or
        // audio items the provider returned without usable metadata.
        let consumed = page.items.len();
        if consumed > limit
            || (consumed == 0 && page.total_record_count.is_some_and(|total| total > offset))
        {
            return Err(BAD_RESPONSE.into());
        }
        Ok((
            page.items.into_iter().filter_map(Item::into_song).collect(),
            consumed,
        ))
    }

    async fn item(&self, id: &str) -> Result<Item, String> {
        let id = jellyfin_id(id).ok_or("Invalid Jellyfin item ID.")?;
        let item: Item = metadata(
            self.get(&format!("/Items/{id}"))
                .query(&[("userId", &self.user_id)]),
        )
        .await?;
        if item.id.as_deref().and_then(jellyfin_id).as_deref() != Some(id.as_str()) {
            return Err(BAD_RESPONSE.into());
        }
        Ok(item)
    }

    pub(crate) async fn song(&self, id: &str) -> Result<Song, String> {
        self.item(id).await?.into_song().ok_or(UNAVAILABLE.into())
    }

    pub(crate) async fn album_cover_id(&self, id: &str) -> Result<Option<String>, String> {
        let album = self.item(id).await?;
        if album.kind.as_deref() != Some("MusicAlbum") {
            return Err(UNAVAILABLE.into());
        }
        Ok(album
            .primary_image()
            .then(|| album.id.as_deref().and_then(jellyfin_id))
            .flatten())
    }

    pub(crate) async fn cover_art(&self, id: &str) -> Result<(&'static str, Vec<u8>), String> {
        let id = jellyfin_id(id).ok_or("Invalid Jellyfin image ID.")?;
        let response = self
            .get(&format!("/Items/{id}/Images/Primary"))
            .query(&[("maxWidth", 256), ("maxHeight", 256)])
            .header(ACCEPT, "image/jpeg, image/png, image/webp")
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|_| UNREACHABLE)?;
        let image = bounded_response(response, MAX_COVER_BYTES).await?;
        let mime = if image.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else if image.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if image.starts_with(b"RIFF") && image.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            return Err(UNAVAILABLE.into());
        };
        Ok((mime, image))
    }

    /// Used only by Burrata's authenticated native relay. Never expose the
    /// request or its headers as player URLs or provider data.
    pub(crate) async fn stream(
        &self,
        id: &str,
        range: Option<HeaderValue>,
    ) -> Result<Response, String> {
        let id = jellyfin_id(id).ok_or("Invalid Jellyfin item ID.")?;
        let mut request = self
            .get(&format!("/Audio/{id}/stream"))
            .query(&[("static", true)]);
        let ranged = range.is_some();
        if let Some(range) = range {
            if !is_single_byte_range(range.to_str().unwrap_or_default()) {
                return Err("Invalid Jellyfin byte range.".into());
            }
            request = request.header(RANGE, range);
        }
        // Bound time until headers without imposing a total deadline on a song.
        let response = tokio::time::timeout(TIMEOUT, request.send())
            .await
            .map_err(|_| UNREACHABLE)?
            .map_err(|_| UNREACHABLE)?;
        let status = response.status();
        if ranged && status == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(response);
        }
        if status != StatusCode::OK && (status != StatusCode::PARTIAL_CONTENT || !ranged) {
            return Err(http_error(status));
        }
        let mime = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !(mime.starts_with("audio/")
            || matches!(
                mime.as_str(),
                "application/octet-stream"
                    | "binary/octet-stream"
                    | "application/ogg"
                    | "video/mp4"
            ))
        {
            return Err("The Jellyfin server did not return audio.".into());
        }
        Ok(response)
    }
}

fn authorization(device_id: &str, token: Option<&str>) -> Result<HeaderValue, String> {
    let safe = |text: &str, bound: usize| {
        !text.is_empty()
            && text.len() <= bound
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    };
    if !safe(device_id, 128) || token.is_some_and(|token| !safe(token, 512)) {
        return Err(BAD_RESPONSE.into());
    }
    let mut value = format!("MediaBrowser Client=\"Burrata\", Device=\"Desktop\", DeviceId=\"{device_id}\", Version=\"{}\"", env!("CARGO_PKG_VERSION"));
    if let Some(token) = token {
        value.push_str(&format!(", Token=\"{token}\""));
    }
    let mut header = HeaderValue::from_str(&value).map_err(|_| BAD_RESPONSE)?;
    header.set_sensitive(true);
    Ok(header)
}

fn http_error(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED => "The Jellyfin server did not accept these credentials.",
        StatusCode::FORBIDDEN => "The Jellyfin server denied access to this item.",
        StatusCode::NOT_FOUND | StatusCode::GONE => UNAVAILABLE,
        _ if status.is_redirection() => "Use the final Jellyfin server URL without a redirect.",
        _ => "The Jellyfin server could not complete the request.",
    }
    .into()
}

async fn metadata<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, String> {
    let response = request
        .header(ACCEPT, "application/json")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|_| UNREACHABLE)?;
    let bytes = bounded_response(response, MAX_METADATA_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|_| BAD_RESPONSE.into())
}

async fn bounded_response(mut response: Response, limit: usize) -> Result<Vec<u8>, String> {
    if response.status() != StatusCode::OK {
        return Err(http_error(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(BAD_RESPONSE.into());
    }
    let mut output = Vec::new();
    while let Some(bytes) = response.chunk().await.map_err(|_| UNREACHABLE)? {
        if bytes.len() > limit - output.len() {
            return Err(BAD_RESPONSE.into());
        }
        output.extend_from_slice(&bytes);
    }
    Ok(output)
}

fn jellyfin_id(value: &str) -> Option<String> {
    if !matches!(value.len(), 32 | 36) {
        return None;
    }
    Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil())
        .map(|id| id.simple().to_string())
}

fn text(value: Option<String>) -> Option<String> {
    let value = value?;
    let value = value.trim();
    (!value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LoginResponse {
    user: LoginUser,
    access_token: String,
    server_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LoginUser {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ItemPage {
    items: Vec<Item>,
    total_record_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NamedArtist {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Item {
    id: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    media_type: Option<String>,
    #[serde(rename = "IsPlaceHolder")]
    is_placeholder: Option<bool>,
    name: Option<String>,
    album_id: Option<String>,
    album_primary_image_tag: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    album_artists: Option<Vec<NamedArtist>>,
    artist_items: Option<Vec<NamedArtist>>,
    artists: Option<Vec<String>>,
    run_time_ticks: Option<i64>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
    production_year: Option<i32>,
    genres: Option<Vec<String>>,
    provider_ids: Option<HashMap<String, Option<String>>>,
    image_tags: Option<HashMap<String, Option<String>>>,
}

impl Item {
    fn primary_image(&self) -> bool {
        self.image_tags
            .as_ref()
            .and_then(|tags| tags.get("Primary"))
            .and_then(Option::as_deref)
            .is_some_and(|tag| !tag.is_empty())
    }

    fn into_song(self) -> Option<Song> {
        if self.kind.as_deref() != Some("Audio")
            || self.is_placeholder == Some(true)
            || self
                .media_type
                .as_deref()
                .is_some_and(|media| media != "Audio")
        {
            return None;
        }
        let id = jellyfin_id(self.id.as_deref()?)?;
        let album_id = self.album_id.as_deref().and_then(jellyfin_id);
        let cover_art =
            if text(self.album_primary_image_tag.clone()).is_some() && album_id.is_some() {
                album_id.clone()
            } else {
                self.primary_image().then(|| id.clone())
            };
        let title = text(self.name.map(|name| name.replace(char::is_control, " ")))?;
        let artist_items = self.artist_items.unwrap_or_default();
        let mut performers: Vec<_> = self
            .artists
            .unwrap_or_default()
            .into_iter()
            .take(32)
            .filter_map(|name| text(Some(name)))
            .collect();
        if performers.is_empty() {
            performers.extend(
                artist_items
                    .iter()
                    .take(32)
                    .filter_map(|row| text(row.name.clone())),
            );
        }
        let artist_id = match (performers.as_slice(), artist_items.as_slice()) {
            ([performer], [row])
                if row
                    .name
                    .as_deref()
                    .is_some_and(|name| name.trim() == performer) =>
            {
                row.id.as_deref().and_then(jellyfin_id)
            }
            _ => None,
        };
        // Jellyfin's MusicBrainzTrack key names a release-track ID on current
        // servers. Only the explicit recording key can establish a recording.
        let recording_mbid = self
            .provider_ids
            .as_ref()
            .and_then(|ids| ids.get("MusicBrainzRecording"))
            .and_then(Option::as_deref)
            .and_then(|id| Uuid::parse_str(id).ok())
            .filter(|id| !id.is_nil())
            .map(|id| id.to_string());
        Some(Song {
            id,
            album_id,
            cover_art,
            title,
            album: text(self.album),
            artist: (!performers.is_empty()).then(|| performers.join(", ")),
            artist_id,
            album_artist: text(self.album_artist).or_else(|| {
                let names: Vec<_> = self
                    .album_artists
                    .unwrap_or_default()
                    .into_iter()
                    .take(32)
                    .filter_map(|row| text(row.name))
                    .collect();
                (!names.is_empty()).then(|| names.join(", "))
            }),
            duration: self
                .run_time_ticks
                .map(|ticks| ticks as f64 / 10_000_000.0)
                .filter(|duration| *duration > 0.0 && *duration <= 86_400.0),
            track: self
                .index_number
                .filter(|number| *number > 0)
                .map(|number| number as u32),
            disc: self
                .parent_index_number
                .filter(|number| *number > 0)
                .map(|number| number as u32),
            year: self
                .production_year
                .filter(|year| (1..=9999).contains(year))
                .map(|year| year as u32),
            genre: self
                .genres
                .into_iter()
                .flatten()
                .find_map(|genre| text(Some(genre))),
            recording_mbid,
            performers,
            ..Song::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header::CONTENT_RANGE, Request};
    use axum::response::{IntoResponse, Response as AxumResponse};
    use serde_json::{json, Value};

    const USER: &str = "11111111111111111111111111111111";
    const ITEM: &str = "22222222222222222222222222222222";
    const ALBUM: &str = "33333333333333333333333333333333";
    const ARTIST: &str = "44444444444444444444444444444444";
    const WRONG: &str = "55555555555555555555555555555555";
    const RECORDING: &str = "66666666-6666-4666-8666-666666666666";
    const RELEASE_TRACK: &str = "77777777-7777-4777-8777-777777777777";
    const TOKEN: &str = "fixture-token";

    fn valid_item() -> Value {
        json!({"Id": ITEM, "Type": "Audio", "MediaType": "Audio", "Name": " Sample\u{0} Song ",
            "AlbumId": ALBUM, "Album": "An edition", "AlbumArtist": "Main Artist", "AlbumPrimaryImageTag": "present",
            "Artists": ["Main Artist"], "ArtistItems": [{"Id": ARTIST, "Name": "Main Artist"}],
            "RunTimeTicks": 1_215_000_000_u64, "IndexNumber": 3, "ParentIndexNumber": 2,
            "ProductionYear": 2017, "Genres": ["Jazz"],
            "ProviderIds": {"MusicBrainzRecording": RECORDING, "MusicBrainzTrack": RELEASE_TRACK},
            "Path": "/private/library/filename.m4a" })
    }

    async fn fixture(req: Request<Body>) -> AxumResponse {
        let (parts, body) = req.into_parts();
        let path = parts.uri.path().strip_prefix("/jellyfin").unwrap();
        let auth = parts.headers[AUTHORIZATION].to_str().unwrap();
        assert!(auth.starts_with("MediaBrowser "));
        let query: HashMap<_, _> =
            url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes()).collect();
        assert!(!parts.uri.to_string().contains(TOKEN));
        if path == "/Users/AuthenticateByName" {
            assert_eq!(parts.method, "POST");
            assert!(!auth.contains("Token="));
            let body: Value = serde_json::from_slice(&to_bytes(body, 4096).await.unwrap()).unwrap();
            assert_eq!(body["Username"], "reader");
            if body["Pw"] != "fixture-password" {
                return (StatusCode::UNAUTHORIZED, "private upstream error").into_response();
            }
            return axum::Json(json!({"User": {"Id": USER}, "AccessToken": TOKEN})).into_response();
        }
        assert_eq!(parts.method, "GET");
        assert!(auth.contains(&format!("Token=\"{TOKEN}\"")));
        if path == "/Items" {
            assert_eq!(query["userId"], USER);
            assert_eq!(query["includeItemTypes"], "Audio");
            assert_eq!(query["recursive"], "true");
            assert_eq!(query["limit"], "3");
            let items = match query["startIndex"].as_ref() {
                "0" => {
                    json!([valid_item(), {"Id": WRONG, "Type": "Audio", "Name": "Unavailable", "IsPlaceHolder": true}, {"Type": "MusicVideo"}])
                }
                "3" => json!([]),
                _ => panic!("unexpected page"),
            };
            return axum::Json(json!({"Items": items, "TotalRecordCount": 3})).into_response();
        }
        if path == format!("/Items/{ITEM}") || path == format!("/Items/{WRONG}") {
            assert_eq!(query["userId"], USER);
            return axum::Json(valid_item()).into_response();
        }
        if path == format!("/Items/{ALBUM}") {
            assert_eq!(query["userId"], USER);
            return axum::Json(
                json!({"Id": ALBUM, "Type": "MusicAlbum", "ImageTags": {"Primary": "present"}}),
            )
            .into_response();
        }
        if path == format!("/Items/{ALBUM}/Images/Primary") {
            assert_eq!(query["maxWidth"], "256");
            assert_eq!(query["maxHeight"], "256");
            return (
                [(CONTENT_TYPE, "image/png")],
                b"\x89PNG\r\n\x1a\nfixture".as_slice(),
            )
                .into_response();
        }
        if path == format!("/Items/{WRONG}/Images/Primary") {
            return (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", "/jellyfin/unexpected")],
            )
                .into_response();
        }
        if path == format!("/Audio/{ITEM}/stream") {
            assert_eq!(query["static"], "true");
            return match parts.headers[RANGE].to_str().unwrap() {
                "bytes=0-1" => (
                    StatusCode::PARTIAL_CONTENT,
                    [
                        (CONTENT_TYPE, "audio/mp4"),
                        (CONTENT_RANGE, "bytes 0-1/100"),
                    ],
                    b"aa".as_slice(),
                )
                    .into_response(),
                "bytes=100-" => (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(CONTENT_RANGE, "bytes */100")],
                )
                    .into_response(),
                _ => panic!("unexpected range"),
            };
        }
        panic!("unexpected fixture path {path}");
    }

    #[tokio::test]
    async fn jellyfin_authenticates_pages_and_fetches_exact_audio_and_artwork() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let endpoint = format!("http://{}/jellyfin", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().fallback(fixture))
                .await
                .unwrap();
        });
        let failed = Client::connect_with_device_id(
            &endpoint,
            "reader",
            "incorrect".into(),
            "profile-fixture",
        )
        .await;
        assert_eq!(
            failed.err().as_deref(),
            Some("The Jellyfin server did not accept these credentials.")
        );

        let client = Client::connect_with_device_id(
            &endpoint,
            "reader",
            "fixture-password".into(),
            "profile-fixture",
        )
        .await
        .unwrap();
        let (songs, consumed) = client.song_catalog_page(0, 3).await.unwrap();
        assert_eq!(consumed, 3);
        assert_eq!(songs.len(), 1);
        assert_eq!(client.song_catalog_page(consumed, 3).await.unwrap().1, 0);
        let song = client.song(ITEM).await.unwrap();
        assert_eq!(song.id, ITEM);
        assert_eq!(song.title, "Sample  Song");
        assert_eq!(song.artist_id.as_deref(), Some(ARTIST));
        assert_eq!(song.album_id.as_deref(), Some(ALBUM));
        assert_eq!(song.duration, Some(121.5));
        assert_eq!(
            (song.disc, song.track, song.year),
            (Some(2), Some(3), Some(2017))
        );
        assert_eq!(song.recording_mbid.as_deref(), Some(RECORDING));
        assert!(!serde_json::to_string(&song).unwrap().contains("/private"));
        assert!(client.song(WRONG).await.is_err());
        assert_eq!(
            client.album_cover_id(ALBUM).await.unwrap().as_deref(),
            Some(ALBUM)
        );
        assert_eq!(
            client.cover_art(&song.cover_art.unwrap()).await.unwrap().0,
            "image/png"
        );
        assert_eq!(
            client.cover_art(WRONG).await.unwrap_err(),
            "Use the final Jellyfin server URL without a redirect."
        );
        let response = client
            .stream(ITEM, Some(HeaderValue::from_static("bytes=0-1")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"aa");
        assert_eq!(
            client
                .stream(ITEM, Some(HeaderValue::from_static("bytes=100-")))
                .await
                .unwrap()
                .status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
        assert!(client
            .stream(ITEM, Some(HeaderValue::from_static("bytes=0-1,3-4")))
            .await
            .is_err());
        server.abort();
    }

    #[test]
    fn refuses_invalid_identifiers_and_does_not_treat_release_tracks_as_recordings() {
        let mut raw = valid_item();
        raw["ProviderIds"] = json!({"MusicBrainzTrack": RELEASE_TRACK});
        raw["Artists"] = json!(["Main Artist", "Guest"]);
        let song = serde_json::from_value::<Item>(raw)
            .unwrap()
            .into_song()
            .unwrap();
        assert_eq!(song.recording_mbid, None);
        assert_eq!(song.artist_id, None);
        assert_eq!(song.performers, ["Main Artist", "Guest"]);
        assert!(jellyfin_id("../private").is_none());
        assert!(authorization("device", Some("bad\"token")).is_err());
        assert!(authorization("device", Some(TOKEN)).unwrap().is_sensitive());
    }
}
