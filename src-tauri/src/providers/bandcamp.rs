//! Public Bandcamp track search adapter. Search results locate artist hosted
//! pages; they do not assert that a recording is streamable.
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
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 300 {
            return Err("Bandcamp search requires 1–300 characters".into());
        }
        let gate = REQUEST_GATE.get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(1)));
        let mut previous = gate.lock().await;
        tokio::time::sleep(Duration::from_secs(1).saturating_sub(previous.elapsed())).await;
        *previous = Instant::now();
        let response = self
            .client
            .post(ENDPOINT)
            .json(&json!({"search_text": query, "search_filter": "t", "full_page": true}))
            .send()
            .await
            .map_err(|_| "Bandcamp track search is unavailable")?;
        if !response.status().is_success() {
            return Err(format!(
                "Bandcamp track search returned HTTP {}",
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
        Ok(parse_search(&raw))
    }
}

pub fn parse_search(raw: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in raw["auto"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .take(100)
    {
        if row["type"] != "t" {
            continue;
        }
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
        let Some(artist) = row["band_name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(url) = row["item_url_path"]
            .as_str()
            .and_then(|url| crate::bandcamp::validate_track_page_url(url).ok())
        else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        tracks.push(Track {
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
            artwork_url: row["img"].as_str().and_then(artwork_url),
            provenance: json!({"catalog":"bandcamp","searchResult":row}),
            ..Default::default()
        });
    }
    tracks
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
}
