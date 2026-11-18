//! Public Bandcamp artist, album and track search adapter. Search results do
//! not assert that a recording is streamable.
use crate::models::Track;
use reqwest::{redirect::Policy, Client};
use serde_json::{json, Value};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const ENDPOINT: &str = "https://bandcamp.com/api/bcsearch_public_api/1/autocomplete_elastic";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
static REQUEST_GATE: OnceLock<Mutex<Instant>> = OnceLock::new();

pub struct Bandcamp {
    client: Client,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub url: String,
    pub artwork_url: Option<String>,
    pub track: Option<Track>,
    pub data: Value,
}

impl Bandcamp {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .redirect(Policy::none())
                .user_agent("Burrata/0.1 (public music search)")
                .build()
                .expect("HTTP client configuration"),
        }
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String> {
        Ok(self
            .search(query, &["track".to_owned()])
            .await?
            .into_iter()
            .filter_map(|item| item.track)
            .collect())
    }

    /// Search one or more public Bandcamp result kinds. The empty filter is
    /// Bandcamp's all-kinds mode, so mixed requests need only one HTTP call.
    pub async fn search(&self, query: &str, kinds: &[String]) -> Result<Vec<SearchResult>, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 300 {
            return Err("Bandcamp search requires 1–300 characters".into());
        }
        let kinds = selected_kinds(kinds);
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let search_filter = search_filter(&kinds);
        let gate = REQUEST_GATE.get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(1)));
        let mut previous = gate.lock().await;
        tokio::time::sleep(Duration::from_secs(1).saturating_sub(previous.elapsed())).await;
        *previous = Instant::now();
        let response = self
            .client
            .post(ENDPOINT)
            .json(&json!({"search_text": query, "search_filter": search_filter, "full_page": true}))
            .send()
            .await
            .map_err(|_| "Bandcamp search is unavailable")?;
        if !response.status().is_success() {
            return Err(format!(
                "Bandcamp search returned HTTP {}",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err("Bandcamp search response exceeds the size limit".into());
        }
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Bandcamp search response could not be read")?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err("Bandcamp search response exceeds the size limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let raw: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Bandcamp search returned invalid data")?;
        Ok(parse_results(&raw, &kinds))
    }
}

pub fn parse_search(raw: &Value) -> Vec<Track> {
    parse_results(raw, &["track".to_owned()])
        .into_iter()
        .filter_map(|item| item.track)
        .collect()
}

fn selected_kinds(kinds: &[String]) -> Vec<String> {
    if kinds.is_empty() {
        return vec!["artist".into(), "album".into(), "track".into()];
    }
    let mut selected = Vec::new();
    for kind in kinds {
        if matches!(kind.as_str(), "artist" | "album" | "track") && !selected.contains(kind) {
            selected.push(kind.clone());
        }
    }
    selected
}

fn result_type(kind: &str) -> Option<&'static str> {
    match kind {
        "artist" => Some("b"),
        "album" => Some("a"),
        "track" => Some("t"),
        _ => None,
    }
}

fn search_filter(kinds: &[String]) -> &'static str {
    if kinds.len() != 1 {
        // The live endpoint returns mixed b/a/t rows for an empty filter.
        ""
    } else {
        match kinds[0].as_str() {
            "artist" => "b",
            "album" => "a",
            "track" => "t",
            _ => "",
        }
    }
}

pub fn parse_results(raw: &Value, kinds: &[String]) -> Vec<SearchResult> {
    let kinds = selected_kinds(kinds);
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in raw["auto"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .take(100)
    {
        let Some(kind) = kinds
            .iter()
            .find(|kind| result_type(kind) == row["type"].as_str())
        else {
            continue;
        };
        let Some(id) = row["id"].as_u64().filter(|id| *id > 0) else {
            continue;
        };
        let Some(title) = row["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if seen.contains(&(kind.as_str(), id)) {
            continue;
        }
        let artwork_url = row["img"].as_str().and_then(artwork_url);
        let parsed = match kind.as_str() {
            "artist" => {
                let Some(url) = row["item_url_root"]
                    .as_str()
                    .and_then(validate_artist_root_url)
                else {
                    continue;
                };
                let is_label = row["is_label"].as_bool().unwrap_or(false);
                SearchResult {
                    id: format!("bandcamp:artist:{id}"),
                    kind: "artist".into(),
                    title: title.into(),
                    subtitle: if is_label {
                        Some("Label".into())
                    } else {
                        row["location"]
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned)
                    },
                    url,
                    artwork_url,
                    track: None,
                    data: json!({"provider":"bandcamp","providerId":id,"isLabel":is_label,"genre":row["genre_name"]}),
                }
            }
            "album" => {
                let Some(artist) = row["band_name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let Some(url) = row["item_url_path"]
                    .as_str()
                    .and_then(|value| crate::bandcamp::validate_album_page_url(value).ok())
                else {
                    continue;
                };
                SearchResult {
                    id: format!("bandcamp:album:{id}"),
                    kind: "album".into(),
                    title: title.into(),
                    subtitle: Some(artist.to_owned()),
                    url: url.to_string(),
                    track: None,
                    data: json!({
                        "release": {"provider": "bandcamp", "id": id.to_string()},
                        "title": title,
                        "sourceUrl": url.as_str(),
                        "primaryType": "Album",
                        "artworkUrls": artwork_url.as_deref().into_iter().collect::<Vec<_>>(),
                        "tracks": [],
                        "credits": [{"name": artist, "role": "artist", "sourceUrl": url.origin().ascii_serialization()}]
                    }),
                    artwork_url,
                }
            }
            "track" => {
                let Some(artist) = row["band_name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let Some(url) = row["item_url_path"]
                    .as_str()
                    .and_then(|value| crate::bandcamp::validate_track_page_url(value).ok())
                else {
                    continue;
                };
                let track = Track {
                    id: format!("bandcamp:{id}"),
                    provider: "bandcamp".into(),
                    provider_id: id.to_string(),
                    source_url: url.to_string(),
                    artist: artist.into(),
                    title: title.into(),
                    album: row["album_name"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    artwork_url: artwork_url.clone(),
                    provenance: json!({"catalog":"bandcamp","searchResult":row}),
                    ..Default::default()
                };
                SearchResult {
                    id: track.id.clone(),
                    kind: "track".into(),
                    title: title.into(),
                    subtitle: Some(artist.to_owned()),
                    url: url.to_string(),
                    artwork_url,
                    track: Some(track),
                    data: json!({"provenance":json!({"catalog":"bandcamp","searchResult":row}),"providerId":id}),
                }
            }
            _ => continue,
        };
        seen.insert((kind.as_str(), id));
        tracks.push(parsed);
    }
    tracks
}

fn validate_artist_root_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    let host = url.host_str()?;
    let subdomain = host.strip_suffix(".bandcamp.com")?;
    if url.scheme() != "https"
        || subdomain.is_empty()
        || subdomain.contains('.')
        || has_explicit_port(raw)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || !host.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !part.starts_with('-')
                && !part.ends_with('-')
        })
    {
        return None;
    }
    Some(url.to_string())
}

fn has_explicit_port(raw: &str) -> bool {
    raw.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .is_some_and(|authority| authority.rsplit_once(':').is_some())
}

fn artwork_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https"
        && (host == "bcbits.com" || host.ends_with(".bcbits.com"))
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_valid_track_rows_and_artist_pages() {
        let raw = json!({"auto":{"results":[
                {"type":"t","id":9,"name":"Song","band_name":"Artist","item_url_path":"https://artist.bandcamp.com/track/song","album_name":"Record","img":"https://f4.bcbits.com/img/123_3.jpg"},
            {"type":"a","id":10,"name":"Album","band_name":"Artist","item_url_path":"https://artist.bandcamp.com/album/record"},
            {"type":"t","id":11,"name":"Bad","band_name":"Artist","item_url_path":"https://evil.example/track/song"},
            {"type":"t","id":12,"name":"Bad","band_name":"Artist","item_url_path":"http://artist.bandcamp.com/track/song"}
        ]}});
        let tracks = parse_search(&raw);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].provider, "bandcamp");
        assert_eq!(
            tracks[0].source_url,
            "https://artist.bandcamp.com/track/song"
        );
        assert_eq!(tracks[0].album.as_deref(), Some("Record"));
        assert_eq!(
            tracks[0].artwork_url.as_deref(),
            Some("https://f4.bcbits.com/img/123_3.jpg")
        );
    }

    #[test]
    fn parses_mixed_artist_album_track_rows_as_typed_results() {
        let raw = json!({"auto":{"results":[
            {"type":"b","id":21,"name":"Artist Page","item_url_root":"https://artistpage.bandcamp.com","location":"Portland, OR","is_label":false},
            {"type":"a","id":22,"name":"First Record","band_name":"Artist Page","item_url_path":"https://artistpage.bandcamp.com/album/first-record","img":"https://f4.bcbits.com/img/a.jpg"},
            {"type":"t","id":23,"name":"First Song","band_name":"Artist Page","album_name":"First Record","item_url_path":"https://artistpage.bandcamp.com/track/first-song"},
            {"type":"b","id":21,"name":"Duplicate Artist","item_url_root":"https://artistpage.bandcamp.com/"},
            {"type":"a","id":22,"name":"Duplicate Album","band_name":"Artist Page","item_url_path":"https://artistpage.bandcamp.com/album/first-record"}
        ]}});
        let results = parse_results(&raw, &[]);
        assert_eq!(
            results
                .iter()
                .map(|item| item.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["artist", "album", "track"]
        );
        assert_eq!(results[0].id, "bandcamp:artist:21");
        assert_eq!(results[0].url, "https://artistpage.bandcamp.com/");
        assert_eq!(results[0].subtitle.as_deref(), Some("Portland, OR"));
        assert!(
            results[0].track.is_none(),
            "artist discovery must not fabricate a Track"
        );
        assert_eq!(results[0].data["isLabel"], false);
        assert_eq!(results[1].id, "bandcamp:album:22");
        assert_eq!(results[1].subtitle.as_deref(), Some("Artist Page"));
        let release: crate::artists::ArtistReleaseRef =
            serde_json::from_value(results[1].data.clone()).unwrap();
        assert_eq!(release.release.provider, "bandcamp");
        assert_eq!(release.release.id, "22");
        assert_eq!(release.source_url, results[1].url);
        assert_eq!(release.credits[0].name, "Artist Page");
        assert!(
            results[1].track.is_none(),
            "album discovery must not fabricate a Track"
        );
        assert_eq!(
            results[1].artwork_url.as_deref(),
            Some("https://f4.bcbits.com/img/a.jpg")
        );
        assert_eq!(results[2].id, "bandcamp:23");
        assert_eq!(
            results[2].track.as_ref().map(|track| track.title.as_str()),
            Some("First Song")
        );
    }

    #[test]
    fn selected_bandcamp_kinds_use_one_combined_filter_when_possible() {
        assert_eq!(search_filter(&selected_kinds(&[])), "");
        assert_eq!(search_filter(&selected_kinds(&["artist".into()])), "b");
        assert_eq!(search_filter(&selected_kinds(&["album".into()])), "a");
        assert_eq!(search_filter(&selected_kinds(&["track".into()])), "t");
        assert_eq!(
            search_filter(&selected_kinds(&["artist".into(), "track".into()])),
            ""
        );
        assert!(selected_kinds(&["video".into()]).is_empty());
    }

    #[test]
    fn filters_mismatched_types_and_rejects_bad_ids_and_urls() {
        let raw = json!({"auto":{"results":[
            {"type":"b","id":31,"name":"Wrongly selected","item_url_root":"https://good.bandcamp.com"},
            {"type":"b","id":32,"name":"Bad path","item_url_root":"https://good.bandcamp.com/music"},
            {"type":"b","id":33,"name":"Fake host","item_url_root":"https://good.bandcamp.com.evil.test"},
            {"type":"b","id":34,"name":"Credential host","item_url_root":"https://user@good.bandcamp.com"},
            {"type":"b","id":35,"name":"Query host","item_url_root":"https://good.bandcamp.com/?x=1"},
            {"type":"b","id":36,"name":"Port host","item_url_root":"https://good.bandcamp.com:443/"},
            {"type":"b","id":"37","name":"String id","item_url_root":"https://good.bandcamp.com"},
            {"type":"b","id":0,"name":"Zero id","item_url_root":"https://good.bandcamp.com"},
            {"type":"a","id":37,"name":"Bad album path","band_name":"Artist","item_url_path":"https://good.bandcamp.com/track/wrong-kind"},
            {"type":"a","id":38,"name":"Bad album host","band_name":"Artist","item_url_path":"https://evil.test/album/record"},
            {"type":"a","id":40,"name":"Album query","band_name":"Artist","item_url_path":"https://good.bandcamp.com/album/record?from=search"},
            {"type":"a","id":39,"name":"No artist","item_url_path":"https://good.bandcamp.com/album/record"},
            {"type":"t","id":40,"name":"Track excluded","band_name":"Artist","item_url_path":"https://good.bandcamp.com/track/song"}
        ]}});
        let artists = parse_results(&raw, &["artist".into()]);
        assert_eq!(
            artists.len(),
            1,
            "only the valid b-kind artist row should remain"
        );
        assert_eq!(artists[0].id, "bandcamp:artist:31");
        let albums = parse_results(&raw, &["album".into()]);
        assert!(
            albums.is_empty(),
            "bad album URLs and missing artist names must be excluded"
        );
        let selected = parse_results(&raw, &["artist".into(), "album".into()]);
        assert_eq!(
            selected.len(),
            1,
            "mixed search should keep only the valid artist row"
        );
        assert_eq!(selected[0].kind, "artist");
    }

    #[test]
    fn invalid_duplicate_does_not_hide_a_later_valid_result() {
        let raw = json!({"auto":{"results":[
            {"type":"a","id":51,"name":"First Record","band_name":"Artist","item_url_path":"https://evil.test/album/first-record"},
            {"type":"a","id":51,"name":"First Record","band_name":"Artist","item_url_path":"https://artist.bandcamp.com/album/first-record"}
        ]}});
        let results = parse_results(&raw, &["album".into()]);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].url,
            "https://artist.bandcamp.com/album/first-record"
        );
    }
}
