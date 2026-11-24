//! Read-only Subsonic protocol client for one explicitly configured server and account.

use reqwest::{
    header::{HeaderValue, CONTENT_TYPE, RANGE},
    redirect::Policy,
    Response, StatusCode, Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::HashSet, net::IpAddr, sync::Arc, time::Duration};
use url::Host;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const MAX_COVER_BYTES: usize = 384 * 1024;
const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
const BAD_RESPONSE: &str = "The Subsonic server returned invalid data.";
const UNREACHABLE: &str = "Could not reach the Subsonic server.";
const NOT_AVAILABLE: &str = "The Subsonic item is unavailable.";

#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    endpoint: String,
    username: String,
    password: Arc<str>,
    open_subsonic: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Album {
    pub id: String,
    pub cover_art: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Song {
    pub id: String,
    pub album_id: Option<String>,
    pub cover_art: Option<String>,
    pub album: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub album_artist: Option<String>,
    pub duration: Option<f64>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    pub version: Option<String>,
    pub movement: Option<String>,
    pub performers: Vec<String>,
    pub work: Option<String>,
    pub work_mbid: Option<String>,
    pub movement_number: Option<u32>,
}

pub(crate) fn canonical_endpoint(input: &str) -> Result<String, String> {
    let input = input.trim();
    let invalid = "Enter a Subsonic server URL with no credentials, query or fragment.";
    if input.len() > 2048 || input.chars().any(char::is_control) {
        return Err(invalid.into());
    }
    let url = Url::parse(input).map_err(|_| invalid)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(invalid.into());
    }
    if url.scheme() == "http" {
        match url.host() {
            Some(Host::Ipv4(ip)) if !private_network(ip.into()) => return Err(https_required()),
            Some(Host::Ipv6(ip)) if !private_network(ip.into()) => return Err(https_required()),
            _ => {}
        }
    }
    Ok(url.as_str().trim_end_matches('/').into())
}

pub(crate) fn canonical_username(input: &str) -> Result<String, String> {
    let value = input.trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err("Enter a Subsonic username.".into());
    }
    Ok(value.into())
}

fn https_required() -> String {
    "Use HTTPS for a Subsonic server outside your private network.".into()
}

fn private_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            ip.is_private() || ip.is_loopback() || (first == 100 && (64..128).contains(&second))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return private_network(mapped.into());
            }
            ip.is_loopback() || ip.octets()[0] & 0xfe == 0xfc
        }
    }
}

pub(crate) async fn server_http(endpoint: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .read_timeout(REQUEST_TIMEOUT)
        .user_agent("Burrata");
    let url = Url::parse(endpoint).map_err(|_| "Invalid Subsonic server URL.")?;
    if url.scheme() == "http" {
        if let Some(Host::Domain(host)) = url.host() {
            let port = url.port_or_known_default().unwrap_or(80);
            let addresses =
                tokio::time::timeout(REQUEST_TIMEOUT, tokio::net::lookup_host((host, port)))
                    .await
                    .map_err(|_| UNREACHABLE)?
                    .map_err(|_| UNREACHABLE)?
                    .collect::<Vec<_>>();
            if addresses.is_empty() {
                return Err(UNREACHABLE.into());
            }
            if addresses
                .iter()
                .any(|address| !private_network(address.ip()))
            {
                return Err(https_required());
            }
            // Validate every DNS result, then connect only to those checked addresses.
            builder = builder.resolve_to_addrs(host, &addresses);
        }
    }
    builder.build().map_err(|_| UNREACHABLE.into())
}

impl Client {
    pub(crate) async fn connect(
        endpoint: &str,
        username: &str,
        password: String,
    ) -> Result<Self, String> {
        let endpoint = canonical_endpoint(endpoint)?;
        let username = canonical_username(username)?;
        if password.is_empty() || password.len() > 4096 {
            return Err("Enter a Subsonic password.".into());
        }
        let mut client = Self {
            http: server_http(&endpoint).await?,
            endpoint,
            username,
            password: password.into(),
            open_subsonic: false,
        };
        client.open_subsonic = client.metadata::<Ping>("ping", &[]).await?.open_subsonic;
        Ok(client)
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    pub(crate) fn supports_song_catalog(&self) -> bool {
        self.open_subsonic
    }

    fn request(&self, method: &str, options: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let salt = uuid::Uuid::new_v4().simple().to_string();
        let token = auth_token(self.password.as_bytes(), salt.as_bytes());
        self.http
            .get(format!("{}/rest/{method}.view", self.endpoint))
            .query(&[
                ("u", self.username.as_str()),
                ("t", token.as_str()),
                ("s", salt.as_str()),
                ("v", "1.16.1"),
                ("c", "Burrata"),
                ("f", "json"),
            ])
            .query(options)
    }

    async fn metadata<T: DeserializeOwned>(
        &self,
        method: &str,
        options: &[(&str, &str)],
    ) -> Result<T, String> {
        let mut response = self
            .request(method, options)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| UNREACHABLE)?;
        if response.status() != StatusCode::OK {
            return Err(http_error(response.status()));
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes > MAX_METADATA_BYTES as u64)
        {
            return Err(BAD_RESPONSE.into());
        }
        let mut body = Vec::new();
        while let Some(bytes) = response.chunk().await.map_err(|_| UNREACHABLE)? {
            if body.len() + bytes.len() > MAX_METADATA_BYTES {
                return Err(BAD_RESPONSE.into());
            }
            body.extend_from_slice(&bytes);
        }
        decode_metadata(&body)
    }

    pub(crate) async fn album_page(
        &self,
        offset: usize,
        size: usize,
    ) -> Result<Vec<Album>, String> {
        if !(1..=500).contains(&size) || offset > i32::MAX as usize {
            return Err("Invalid Subsonic album page.".into());
        }
        let envelope: AlbumPage = self
            .metadata(
                "getAlbumList2",
                &[
                    ("type", "alphabeticalByName"),
                    ("size", &size.to_string()),
                    ("offset", &offset.to_string()),
                ],
            )
            .await?;
        if envelope.album_list2.album.len() > size {
            return Err(BAD_RESPONSE.into());
        }
        let mut seen = HashSet::new();
        envelope
            .album_list2
            .album
            .into_iter()
            .map(|raw| {
                let album = raw.into_album()?;
                if !seen.insert(album.id.clone()) {
                    return Err(BAD_RESPONSE.into());
                }
                Ok(album)
            })
            .collect()
    }

    // OpenSubsonic defines an empty search query as a full catalog browse.
    pub(crate) async fn song_catalog_page(
        &self,
        offset: usize,
        size: usize,
    ) -> Result<(Vec<Song>, usize), String> {
        let envelope: SongSearch = self
            .metadata(
                "search3",
                &[
                    ("query", ""),
                    ("artistCount", "0"),
                    ("albumCount", "0"),
                    ("songCount", &size.to_string()),
                    ("songOffset", &offset.to_string()),
                ],
            )
            .await?;
        let rows = envelope.search_result3.song;
        let count = rows.len();
        if count > size {
            return Err(BAD_RESPONSE.into());
        }
        let songs = rows
            .into_iter()
            .filter(RawSong::is_music)
            .map(RawSong::into_song)
            .collect::<Result<_, _>>()?;
        Ok((songs, count))
    }

    pub(crate) async fn album(&self, id: &str) -> Result<Vec<Song>, String> {
        validate_id(id)?;
        let result: AlbumResult = self.metadata("getAlbum", &[("id", id)]).await?;
        let album = result.album.info.into_album()?;
        if album.id != id {
            return Err(BAD_RESPONSE.into());
        }
        let mut seen = HashSet::new();
        let mut songs = Vec::new();
        for raw in result.album.song {
            if !raw.is_music() {
                continue;
            }
            let mut song = raw.into_song()?;
            if song.album_id.as_deref().is_some_and(|actual| actual != id)
                || !seen.insert(song.id.clone())
            {
                return Err(BAD_RESPONSE.into());
            }
            song.album_id = Some(album.id.clone());
            song.album.get_or_insert_with(|| album.title.clone());
            if song.cover_art.is_none() {
                song.cover_art.clone_from(&album.cover_art);
            }
            if song.album_artist.is_none() {
                song.album_artist.clone_from(&album.artist);
            }
            songs.push(song);
        }
        Ok(songs)
    }

    pub(crate) async fn song(&self, id: &str) -> Result<Song, String> {
        validate_id(id)?;
        let result: SongResult = self.metadata("getSong", &[("id", id)]).await?;
        if result.song.id != id {
            return Err(BAD_RESPONSE.into());
        }
        if !result.song.is_music() {
            return Err(NOT_AVAILABLE.into());
        }
        result.song.into_song()
    }

    pub(crate) async fn album_cover_id(&self, id: &str) -> Result<Option<String>, String> {
        validate_id(id)?;
        let result: AlbumResult = self.metadata("getAlbum", &[("id", id)]).await?;
        let album = result.album.info.into_album()?;
        if album.id != id {
            return Err(BAD_RESPONSE.into());
        }
        Ok(album.cover_art)
    }

    pub(crate) async fn cover_art(&self, id: &str) -> Result<(&'static str, Vec<u8>), String> {
        validate_id(id)?;
        let mut response = self
            .request("getCoverArt", &[("id", id), ("size", "256")])
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| UNREACHABLE)?;
        if response.status() != StatusCode::OK
            || response
                .content_length()
                .is_some_and(|size| size > MAX_COVER_BYTES as u64)
        {
            return Err(NOT_AVAILABLE.into());
        }
        let mut image = Vec::new();
        while let Some(bytes) = response.chunk().await.map_err(|_| UNREACHABLE)? {
            if image.len() + bytes.len() > MAX_COVER_BYTES {
                return Err(BAD_RESPONSE.into());
            }
            image.extend_from_slice(&bytes);
        }
        let mime = if image.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else if image.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if image.starts_with(b"RIFF") && image.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            return Err(NOT_AVAILABLE.into());
        };
        Ok((mime, image))
    }

    // For the native loopback handler only. Never serialize or debug-log this
    // response: reqwest retains the authenticated URL internally.
    pub(crate) async fn stream(
        &self,
        id: &str,
        range: Option<HeaderValue>,
    ) -> Result<Response, String> {
        validate_id(id)?;
        let ranged = range.is_some();
        let mut request = self.request("stream", &[("id", id), ("format", "raw")]);
        if let Some(range) = range {
            if !is_single_byte_range(range.to_str().unwrap_or_default()) {
                return Err("Invalid Subsonic byte range.".into());
            }
            request = request.header(RANGE, range);
        }
        let response = tokio::time::timeout(REQUEST_TIMEOUT, request.send())
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
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !(content_type.starts_with("audio/")
            || matches!(
                content_type.as_str(),
                "application/octet-stream"
                    | "binary/octet-stream"
                    | "application/ogg"
                    | "video/mp4"
            ))
        {
            return Err("The Subsonic server did not return audio.".into());
        }
        Ok(response)
    }
}

fn http_error(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED => "The Subsonic server did not accept these credentials.",
        StatusCode::FORBIDDEN => "The Subsonic server denied access to this item.",
        StatusCode::NOT_FOUND | StatusCode::GONE => NOT_AVAILABLE,
        _ if status.is_redirection() => "Use the final Subsonic server URL without a redirect.",
        _ => "The Subsonic server could not complete the request.",
    }
    .into()
}

fn decode_metadata<T: DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    let envelope: MetadataEnvelope = serde_json::from_slice(body).map_err(|_| BAD_RESPONSE)?;
    let data = envelope.response;
    match data.status.as_str() {
        "ok" if data.error.is_none() => {
            serde_json::from_value(serde_json::Value::Object(data.payload))
                .map_err(|_| BAD_RESPONSE.into())
        }
        "failed" => Err(match data.error.map(|error| error.code) {
            Some(40) => "The Subsonic server did not accept these credentials.",
            Some(41 | 42) => "The Subsonic server does not support the requested authentication.",
            Some(50) => "The Subsonic server denied access to this item.",
            Some(70) => NOT_AVAILABLE,
            _ => "The Subsonic server could not complete the request.",
        }
        .into()),
        _ => Err(BAD_RESPONSE.into()),
    }
}

#[derive(Deserialize)]
struct MetadataEnvelope {
    #[serde(rename = "subsonic-response")]
    response: MetadataResponse,
}

#[derive(Deserialize)]
struct MetadataResponse {
    status: String,
    error: Option<MetadataError>,
    #[serde(flatten)]
    payload: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct MetadataError {
    code: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ping {
    #[serde(default)]
    open_subsonic: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SongSearch {
    search_result3: SongSearchRows,
}

#[derive(Deserialize)]
struct SongSearchRows {
    #[serde(default)]
    song: Vec<RawSong>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlbumPage {
    album_list2: AlbumRows,
}

#[derive(Deserialize)]
struct AlbumRows {
    #[serde(default)]
    album: Vec<RawAlbum>,
}

#[derive(Deserialize)]
struct AlbumResult {
    album: AlbumWithSongs,
}

#[derive(Deserialize)]
struct AlbumWithSongs {
    #[serde(flatten)]
    info: RawAlbum,
    #[serde(default)]
    song: Vec<RawSong>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAlbum {
    id: String,
    cover_art: Option<String>,
    name: Option<String>,
    title: Option<String>,
    album: Option<String>,
    artist: Option<String>,
    display_artist: Option<String>,
    year: Option<u32>,
}

impl RawAlbum {
    fn into_album(self) -> Result<Album, String> {
        validate_id(&self.id)?;
        let title = optional_text(self.name)
            .or_else(|| optional_text(self.title))
            .or_else(|| optional_text(self.album))
            .ok_or(BAD_RESPONSE)?;
        Ok(Album {
            id: self.id,
            cover_art: self.cover_art.filter(|id| validate_id(id).is_ok()),
            title,
            artist: optional_text(self.display_artist).or_else(|| optional_text(self.artist)),
            year: self.year.filter(|year| (1..=9999).contains(year)),
        })
    }
}

#[derive(Deserialize)]
struct SongResult {
    song: RawSong,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSong {
    id: String,
    cover_art: Option<String>,
    title: String,
    album: Option<String>,
    album_id: Option<String>,
    artist: Option<String>,
    artist_id: Option<String>,
    display_artist: Option<String>,
    album_artist: Option<String>,
    display_album_artist: Option<String>,
    duration: Option<f64>,
    track: Option<u32>,
    disc_number: Option<u32>,
    genre: Option<String>,
    year: Option<u32>,
    isrc: Option<RawIsrc>,
    music_brainz_id: Option<String>,
    media_type: Option<String>,
    #[serde(rename = "type")]
    media_kind: Option<String>,
    is_video: Option<bool>,
    is_dir: Option<bool>,
    version: Option<String>,
    #[serde(default)]
    artists: Vec<RawArtist>,
    #[serde(default)]
    works: Vec<RawWork>,
    #[serde(default)]
    movements: Vec<RawMovement>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIsrc {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct RawArtist {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWork {
    name: Option<String>,
    music_brainz_id: Option<String>,
}

#[derive(Deserialize)]
struct RawMovement {
    name: Option<String>,
    number: Option<u32>,
}

impl RawSong {
    fn is_music(&self) -> bool {
        self.is_video != Some(true)
            && self.is_dir != Some(true)
            && self
                .media_type
                .as_deref()
                .is_none_or(|kind| kind.is_empty() || kind == "song")
            && self
                .media_kind
                .as_deref()
                .is_none_or(|kind| kind.is_empty() || kind == "music")
    }

    fn into_song(self) -> Result<Song, String> {
        validate_id(&self.id)?;
        let album_id = self.album_id.filter(|id| !id.is_empty());
        if let Some(album_id) = &album_id {
            validate_id(album_id)?;
        }
        // Titles come from file tags. Replace embedded controls with spaces so
        // one damaged tag cannot prevent browsing the album or join words.
        let title =
            optional_text(Some(self.title.replace(char::is_control, " "))).ok_or(BAD_RESPONSE)?;
        let work = match self.works.as_slice() {
            [work] => optional_text(work.name.clone()),
            _ => None,
        };
        let work_mbid = match self.works.as_slice() {
            [work] => checked_mbid(work.music_brainz_id.as_deref()),
            _ => None,
        };
        let (movement, movement_number) = match self.movements.as_slice() {
            [movement] => (
                optional_text(movement.name.clone()),
                movement.number.filter(|number| *number > 0),
            ),
            _ => (None, None),
        };
        // The legacy artistId identifies one artist. A display credit with
        // several structured performers needs its own navigation identities.
        let artist_id = self.artist_id.filter(|id| self.artists.len() <= 1 && validate_id(id).is_ok());
        let performers = if self.artists.len() <= 32 {
            self.artists
                .into_iter()
                .map(|artist| optional_text(Some(artist.name)))
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Song {
            id: self.id,
            album_id,
            cover_art: self.cover_art.filter(|id| validate_id(id).is_ok()),
            album: optional_text(self.album),
            title,
            artist: optional_text(self.display_artist).or_else(|| optional_text(self.artist)),
            artist_id,
            album_artist: optional_text(self.display_album_artist)
                .or_else(|| optional_text(self.album_artist)),
            duration: self
                .duration
                .filter(|duration| duration.is_finite() && *duration > 0.0),
            track: self.track.filter(|number| *number > 0),
            disc: self.disc_number.filter(|number| *number > 0),
            genre: optional_text(self.genre),
            year: self.year.filter(|year| (1..=9999).contains(year)),
            isrc: single_isrc(self.isrc),
            recording_mbid: if self.media_type.as_deref() == Some("song") {
                checked_mbid(self.music_brainz_id.as_deref())
            } else {
                None
            },
            version: optional_text(self.version),
            movement,
            performers,
            work,
            work_mbid,
            movement_number,
        })
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        return Err("Invalid Subsonic item ID.".into());
    }
    Ok(())
}

fn optional_text(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed.len() <= 4096 && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.to_owned())
}

fn checked_mbid(value: Option<&str>) -> Option<String> {
    let raw = value?;
    let parsed = uuid::Uuid::parse_str(raw).ok()?;
    (!parsed.is_nil() && raw.len() == 36).then(|| parsed.to_string())
}

fn single_isrc(value: Option<RawIsrc>) -> Option<String> {
    let values = match value? {
        RawIsrc::One(value) => vec![value],
        RawIsrc::Many(values) => values,
    };
    let codes = values
        .iter()
        .map(|code| {
            let value = code.trim().replace('-', "").to_ascii_uppercase();
            let bytes = value.as_bytes();
            (bytes.len() == 12
                && bytes[..2].iter().all(u8::is_ascii_alphabetic)
                && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
                && bytes[5..].iter().all(u8::is_ascii_digit))
            .then_some(value)
        })
        .collect::<Option<HashSet<_>>>()?;
    if codes.len() == 1 {
        codes.into_iter().next()
    } else {
        None
    }
}

pub(crate) fn is_single_byte_range(value: &str) -> bool {
    let Some((start, end)) = value
        .strip_prefix("bytes=")
        .and_then(|range| range.split_once('-'))
    else {
        return false;
    };
    fn number(value: &str) -> Option<u64> {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    }
    if start.is_empty() {
        number(end).is_some_and(|suffix| suffix > 0)
    } else if end.is_empty() {
        number(start).is_some()
    } else {
        number(start)
            .zip(number(end))
            .is_some_and(|(start, end)| start <= end)
    }
}

// Subsonic 1.13 token authentication requires the RFC 1321 MD5 transform.
// This never hashes persisted credentials or serves as an identity check.
fn auth_token(password: &[u8], salt: &[u8]) -> String {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const CONSTANTS: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let input_len = password.len() + salt.len();
    let padded_len = (input_len + 9).div_ceil(64) * 64;
    let mut input = Vec::with_capacity(padded_len);
    input.extend_from_slice(password);
    input.extend_from_slice(salt);
    input.push(0x80);
    input.resize(padded_len - 8, 0);
    input.extend_from_slice(&((input_len as u64) * 8).to_le_bytes());
    let mut state = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];

    for chunk in input.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (slot, bytes) in words.iter_mut().zip(chunk.chunks_exact(4)) {
            *slot = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for step in 0..64 {
            let (combined, index) = match step {
                0..=15 => ((b & c) | (!b & d), step),
                16..=31 => ((d & b) | (!d & c), (5 * step + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * step + 5) % 16),
                _ => (c ^ (b | !d), (7 * step) % 16),
            };
            let next = a
                .wrapping_add(combined)
                .wrapping_add(CONSTANTS[step])
                .wrapping_add(words[index])
                .rotate_left(SHIFTS[step])
                .wrapping_add(b);
            (a, b, c, d) = (d, next, b, c);
        }
        for (sum, value) in state.iter_mut().zip([a, b, c, d]) {
            *sum = sum.wrapping_add(value);
        }
    }
    input.fill(0);
    state
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_token_matches_documented_and_multiblock_vectors() {
        assert_eq!(
            auth_token(b"sesame", b"c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
        assert_eq!(auth_token(b"", b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            auth_token(
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                b"0123456789"
            ),
            "d174ab98d277d9f5a5611c2c9f419d9f"
        );
        assert_eq!(
            auth_token(&[b'a'; 56], b""),
            "3b0c8ac703f828b04c6c197006d17218"
        );
    }

    #[test]
    fn endpoint_rejects_credentials_and_public_cleartext() {
        assert_eq!(
            canonical_endpoint(" https://Example.ORG:443/music/// ").unwrap(),
            "https://example.org/music"
        );
        assert!(canonical_endpoint("http://192.168.1.8:4533/music").is_ok());
        assert!(canonical_endpoint("http://100.64.1.8:4533").is_ok());
        assert!(canonical_endpoint("http://[::ffff:127.0.0.1]:4533").is_ok());
        assert!(canonical_endpoint("https://8.8.8.8").is_ok());
        for input in [
            "file:///tmp/music",
            "https://user@example.org",
            "https://:secret@example.org",
            "https://example.org?key=secret",
            "https://example.org/#key",
            "http://8.8.8.8",
            "http://100.128.1.8",
            "http://169.254.169.254",
            "http://[2001:4860::8888]",
        ] {
            assert!(canonical_endpoint(input).is_err(), "{input}");
        }
    }

    #[test]
    fn wire_song_retains_supported_identity_fields_and_ignores_paths() {
        let song: SongResult = decode_metadata(
            br#"{"subsonic-response":{"status":"ok","song":{
                "id":"m1","albumId":"a1","title":"Adagio (Live)","album":"Live",
                "artist":"Historical display","displayArtist":"Artist A & Artist B","artistId":"artist-a",
                "displayAlbumArtist":"Artist A","duration":153,"track":3,"discNumber":2,
                "isrc":["USSM18300073","US-SM1-83-00073"],
                "mediaType":"song","musicBrainzId":"189002e7-3285-4e2e-92a3-7f6c30d407a2",
                "artists":[{"name":"Artist A"},{"name":"Artist B"}],"version":"Live",
                "works":[{"name":"Concerto","musicBrainzId":"d03bff61-26fc-301b-98ac-4d8e85771cbc"}],
                "movements":[{"name":"Adagio","number":2}],
                "path":"/private/Music/example.flac","streamUrl":"https://private/?p=secret"
            }}}"#,
        )
        .unwrap();
        let song = song.song.into_song().unwrap();
        assert_eq!(song.artist.as_deref(), Some("Artist A & Artist B"));
        assert_eq!(song.artist_id, None);
        assert_eq!(song.disc, Some(2));
        assert_eq!(song.isrc.as_deref(), Some("USSM18300073"));
        assert_eq!(song.performers, vec!["Artist A", "Artist B"]);
        assert_eq!(song.movement.as_deref(), Some("Adagio"));
        assert_eq!(song.movement_number, Some(2));
        assert!(song.recording_mbid.is_some() && song.work_mbid.is_some());
        let encoded = serde_json::to_string(&song).unwrap();
        assert!(!encoded.contains("/private/"));
        assert!(!encoded.contains("secret"));
    }

    #[test]
    fn song_retains_a_single_server_artist_id() {
        let raw: RawSong = serde_json::from_value(serde_json::json!({
            "id":"song", "title":"Track", "artist":"Artist", "artistId":"artist-id"
        })).unwrap();
        assert_eq!(raw.into_song().unwrap().artist_id.as_deref(), Some("artist-id"));
    }

    #[test]
    fn track_title_controls_are_removed_without_changing_source_identity() {
        let album: AlbumResult = decode_metadata(
            br#"{"subsonic-response":{"status":"ok","album":{
                "id":"album","name":"Album","song":[
                    {"id":"first","albumId":"album","title":"First"},
                    {"id":"track","albumId":"album","title":"  Theme\u0000II\nAdagio\u001b  "},
                    {"id":"last","albumId":"album","title":"Last"}
                ]
            }}}"#,
        )
        .unwrap();
        let tracks = album
            .album
            .song
            .into_iter()
            .map(RawSong::into_song)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            tracks
                .iter()
                .map(|song| song.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "track", "last"]
        );
        assert_eq!(tracks[1].album_id.as_deref(), Some("album"));
        assert_eq!(tracks[1].title, "Theme II Adagio");

        for invalid in [
            r#"{"id":"track\u0000","title":"Theme"}"#,
            r#"{"id":"track","title":"\u0000\u001b"}"#,
        ] {
            let raw: RawSong = serde_json::from_str(invalid).unwrap();
            assert!(raw.into_song().is_err());
        }
    }

    #[test]
    fn rejects_ambiguous_identity_and_sanitizes_server_errors() {
        assert!(single_isrc(Some(RawIsrc::Many(vec![
            "USSM18300073".into(),
            "DELV42300297".into(),
        ])))
        .is_none());
        let raw: RawSong = serde_json::from_str(
            r#"{"id":"track","title":"Track","musicBrainzId":"189002e7-3285-4e2e-92a3-7f6c30d407a2","movements":[{"name":"First"},{"name":"Second"}]}"#,
        )
        .unwrap();
        let song = raw.into_song().unwrap();
        assert!(song.recording_mbid.is_none() && song.movement.is_none());
        let error = decode_metadata::<serde_json::Value>(
            br#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"private password secret"}}}"#,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "The Subsonic server did not accept these credentials."
        );
    }

    #[test]
    fn supports_only_a_valid_single_byte_range() {
        for valid in ["bytes=4-20", "bytes=0-", "bytes=-80"] {
            assert!(is_single_byte_range(valid));
        }
        for invalid in [
            "bytes=-0",
            "bytes=20-5",
            "bytes=1-2,4-5",
            "other=0-",
            "bytes=+-",
        ] {
            assert!(!is_single_byte_range(invalid));
        }
    }
}
