//! Identity-aware artist discovery and cached provider pages.
//!
//! MusicBrainz is the primary identity source. Discogs results remain separate
//! provider identities until a provider URL explicitly cross-links them.
pub(crate) use crate::spotify_mb_crosswalk::ReleaseOccurrence;
use crate::spotify_mb_crosswalk::{
    exact_release_id, normalize_musicbrainz_release, normalize_url_relations, resolve_occurrence,
    spotify_track_occurrence_count, valid_mbid as valid_crosswalk_mbid, valid_spotify_id,
    validate_spotify_target,
};
use crate::{
    matching,
    models::{Candidate, Chapter, Track},
    Store,
};
use reqwest::{header, Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

const MUSICBRAINZ: &str = "https://musicbrainz.org/ws/2";
const DISCOGS: &str = "https://api.discogs.com";
const LASTFM: &str = "https://ws.audioscrobbler.com/2.0/";
const COVER_ART: &str = "https://coverartarchive.org";
const PAGE_SIZE: u32 = 25;
const ARTIST_LIST_PAGE_SIZE: u32 = 100;
const SEARCH_TTL: u64 = 60 * 60;
const PROFILE_TTL: u64 = 24 * 60 * 60;
const LIST_TTL: u64 = 6 * 60 * 60;
const MUSICBRAINZ_RELEASE_GROUP_SEARCH_CACHE: &str = "albums.search.musicbrainz.v2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRef {
    pub provider: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSearchResult {
    pub artist: ArtistRef,
    pub name: String,
    pub image_url: Option<String>,
    pub sort_name: Option<String>,
    pub disambiguation: Option<String>,
    pub artist_type: Option<String>,
    pub country: Option<String>,
    pub begin_date: Option<String>,
    pub end_date: Option<String>,
    pub score: Option<f64>,
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSearchPage {
    pub query: String,
    pub page: u32,
    pub page_size: u32,
    pub total: Option<u32>,
    pub has_more: bool,
    pub results: Vec<ArtistSearchResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistExternalLink {
    pub label: String,
    pub url: String,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCredit {
    pub name: String,
    pub artist: Option<ArtistRef>,
    /// Provider role text is preserved verbatim (for example, `Remixer` or `Producer`).
    pub role: String,
    pub credited_as: Option<String>,
    pub join_phrase: Option<String>,
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistReleaseTrack {
    pub position: Option<String>,
    pub title: String,
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistReleaseRef {
    pub release: ArtistRef,
    pub title: String,
    pub year: Option<u32>,
    pub release_date: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub format: Option<String>,
    pub primary_type: Option<String>,
    pub track_count: Option<u32>,
    #[serde(default)]
    pub tracks: Vec<ArtistReleaseTrack>,
    #[serde(default)]
    pub artwork_urls: Vec<String>,
    pub source_url: String,
    pub credits: Vec<ArtistCredit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistReleasePage {
    pub artist: ArtistRef,
    pub items: Vec<ArtistReleaseRef>,
    pub next_cursor: Option<String>,
    pub source_coverage: Vec<ArtistSourceCoverage>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSearchPage {
    pub items: Vec<ArtistReleaseRef>,
    pub next_page: Option<u32>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistTrackPage {
    pub artist: ArtistRef,
    pub items: Vec<Track>,
    pub next_cursor: Option<String>,
    pub source: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCollaboration {
    pub credits: Vec<ArtistCredit>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCollaborationPage {
    pub artist: ArtistRef,
    pub items: Vec<ArtistCollaboration>,
    pub next_cursor: Option<String>,
    pub source_coverage: Vec<ArtistSourceCoverage>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistProfile {
    pub artist: ArtistSearchResult,
    pub biography: Option<String>,
    pub biography_source: Option<String>,
    pub image_url: Option<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub source_tags: Vec<ArtistTag>,
    pub aliases: Vec<String>,
    pub links: Vec<ArtistExternalLink>,
    pub top_tracks: Vec<Track>,
    pub releases: ArtistReleasePage,
    pub discogs_available: bool,
    pub lastfm_available: bool,
    pub source_coverage: Vec<ArtistSourceCoverage>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistTag {
    pub name: String,
    pub provider: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSourceCoverage {
    pub provider: String,
    pub status: String,
    pub detail: String,
    pub source_url: Option<String>,
}

/// Credentials are read by the application connection manager and passed in only
/// for the current request. They are never persisted in artist cache keys or payloads.
#[derive(Debug, Clone, Default)]
pub struct ArtistCredentials {
    pub discogs_token: Option<String>,
    pub lastfm_api_key: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
struct Endpoints {
    musicbrainz: String,
    discogs: String,
    lastfm: String,
    cover_art: String,
}

#[derive(Clone)]
pub struct ArtistService {
    store: Arc<Store>,
    client: Client,
    endpoints: Endpoints,
}

impl ArtistService {
    pub fn new(store: Arc<Store>) -> Result<Self, String> {
        let client = Client::builder()
            .user_agent(concat!(
                "Burrata/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/anaxonda/burrata)"
            ))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("Could not create artist metadata client: {error}"))?;
        Ok(Self {
            store,
            client,
            endpoints: Endpoints {
                musicbrainz: MUSICBRAINZ.into(),
                discogs: DISCOGS.into(),
                lastfm: LASTFM.into(),
                cover_art: COVER_ART.into(),
            },
        })
    }

    async fn get_json(
        &self,
        namespace: &str,
        cache_key: &str,
        url: Url,
        ttl: u64,
        discogs_token: Option<&str>,
    ) -> Result<Value, String> {
        if let Some(value) = self.store.get_provider_cache(namespace, cache_key)? {
            return Ok(value);
        }
        let mut last_error = String::new();
        for attempt in 0..3 {
            if namespace.contains("musicbrainz") {
                musicbrainz_rate_limit().await;
            }
            if namespace.contains("discogs") {
                discogs_rate_limit().await;
            }
            let mut request = self.client.get(url.clone());
            if let Some(token) = discogs_token.filter(|token| !token.trim().is_empty()) {
                request = request.header(header::AUTHORIZATION, format!("Discogs token={token}"));
            }
            // reqwest errors can include the full request URL. Last.fm credentials
            // are query parameters, so keep transport errors free of URL details.
            crate::diagnostic_metrics::record_http_send_attempt();
            let response = request
                .send()
                .await
                .map_err(|_| "Provider request failed; check your network connection".to_owned())?;
            let status = response.status();
            if matches!(status.as_u16(), 429 | 502 | 503 | 504) && attempt < 2 {
                let retry_after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(1u64 << attempt)
                    .clamp(1, 8);
                last_error = format!("Provider returned HTTP {status}");
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }
            let body = response
                .text()
                .await
                .map_err(|_| "Provider response could not be read".to_owned())?;
            if !status.is_success() {
                return Err(format!("Provider returned HTTP {status}"));
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|error| format!("Provider returned invalid JSON: {error}"))?;
            if value.get("error").is_some() {
                return Err("Provider rejected the request or credential".into());
            }
            // A cache write must never discard a valid provider response.
            let _ = self
                .store
                .set_provider_cache(namespace, cache_key, &value, ttl);
            return Ok(value);
        }
        Err(format!("{last_error} after retries"))
    }

    async fn musicbrainz_json(
        &self,
        path: &str,
        query: &[(&str, String)],
        namespace: &str,
        cache_key: &str,
        ttl: u64,
    ) -> Result<Value, String> {
        let mut url = Url::parse(&format!("{}/{path}", self.endpoints.musicbrainz))
            .map_err(|error| error.to_string())?;
        url.query_pairs_mut().append_pair("fmt", "json");
        for (key, value) in query {
            url.query_pairs_mut().append_pair(key, value);
        }
        self.get_json(namespace, cache_key, url, ttl, None).await
    }

    async fn discogs_json(
        &self,
        path: &str,
        query: &[(&str, String)],
        namespace: &str,
        cache_key: &str,
        ttl: u64,
        token: &str,
    ) -> Result<Value, String> {
        let mut url = Url::parse(&format!("{}/{path}", self.endpoints.discogs))
            .map_err(|error| error.to_string())?;
        for (key, value) in query {
            url.query_pairs_mut().append_pair(key, value);
        }
        self.get_json(namespace, cache_key, url, ttl, Some(token))
            .await
    }

    pub async fn search_artists(
        &self,
        query: &str,
        page: u32,
        page_size: u32,
        credentials: &ArtistCredentials,
    ) -> Result<ArtistSearchPage, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 120 {
            return Err("Artist search must contain 1–120 characters".into());
        }
        if !(1..=200).contains(&page) || !(1..=PAGE_SIZE).contains(&page_size) {
            return Err("Artist search page or page size is out of range".into());
        }
        let offset = (page - 1) * page_size;
        let cache_id = format!("{}:{page}:{page_size}", query.to_lowercase());
        let mut warnings = credentials.warnings.clone();
        let mut results = Vec::new();
        let mut total = None;
        let mut more = false;
        let mut provider_succeeded = false;
        let mb_query = vec![
            ("query", query.to_owned()),
            ("limit", page_size.to_string()),
            ("offset", offset.to_string()),
        ];
        match self
            .musicbrainz_json(
                "artist",
                &mb_query,
                "artists.search.musicbrainz",
                &cache_id,
                SEARCH_TTL,
            )
            .await
        {
            Ok(data) => {
                provider_succeeded = true;
                total = data["count"]
                    .as_u64()
                    .map(|n| n.min(u32::MAX as u64) as u32);
                more = total.is_some_and(|count| offset + page_size < count);
                if let Some(items) = data["artists"].as_array() {
                    results.extend(items.iter().filter_map(parse_musicbrainz_artist));
                }
            }
            Err(error) => warnings.push(format!("MusicBrainz artist search failed: {error}")),
        }
        if let Some(token) = credentials
            .discogs_token
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            let discogs_query = vec![
                ("q", query.to_owned()),
                ("type", "artist".into()),
                ("per_page", page_size.to_string()),
                ("page", page.to_string()),
            ];
            match self
                .discogs_json(
                    "database/search",
                    &discogs_query,
                    "artists.search.discogs",
                    &cache_id,
                    SEARCH_TTL,
                    token,
                )
                .await
            {
                Ok(data) => {
                    provider_succeeded = true;
                    if let Some(items) = data["results"].as_array() {
                        results.extend(items.iter().filter_map(parse_discogs_artist));
                    }
                    more |= data["pagination"]["pages"]
                        .as_u64()
                        .is_some_and(|n| n > page as u64);
                    total =
                        total.or_else(|| data["pagination"]["items"].as_u64().map(|n| n as u32));
                }
                Err(error) => warnings.push(format!("Discogs artist search failed: {error}")),
            }
        }
        if !provider_succeeded && warnings.iter().any(|warning| warning.contains(" failed:")) {
            return Err(warnings.join("; "));
        }
        self.attach_cached_musicbrainz_artist_images(&mut results);
        Ok(ArtistSearchPage {
            query: query.into(),
            page,
            page_size,
            total,
            has_more: more,
            results,
            warnings,
        })
    }

    /// Search exactly one provider. Discovery source tabs use this API so a
    /// selected source never waits on, mixes with, or inherits errors from its
    /// sibling catalog. `search_artists` remains aggregated for Artist Pages.
    pub async fn search_artists_for(
        &self,
        provider: &str,
        query: &str,
        page: u32,
        page_size: u32,
        credentials: &ArtistCredentials,
    ) -> Result<ArtistSearchPage, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 120 {
            return Err("Artist search must contain 1–120 characters".into());
        }
        if !(1..=200).contains(&page) || !(1..=PAGE_SIZE).contains(&page_size) {
            return Err("Artist search page or page size is out of range".into());
        }
        let offset = (page - 1) * page_size;
        let cache_id = format!("{}:{page}:{page_size}", query.to_lowercase());
        let (mut results, total, has_more) = match provider {
            "musicbrainz" => {
                let params = vec![
                    ("query", query.to_owned()),
                    ("limit", page_size.to_string()),
                    ("offset", offset.to_string()),
                ];
                let data = self
                    .musicbrainz_json(
                        "artist",
                        &params,
                        "artists.search.musicbrainz",
                        &cache_id,
                        SEARCH_TTL,
                    )
                    .await?;
                let count = data["count"]
                    .as_u64()
                    .map(|n| n.min(u32::MAX as u64) as u32);
                let rows = data["artists"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_musicbrainz_artist)
                    .collect::<Vec<ArtistSearchResult>>();
                let more = count.is_some_and(|n| offset + page_size < n);
                (rows, count, more)
            }
            "discogs" => {
                let token = credentials
                    .discogs_token
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("Discogs is not configured")?;
                let params = vec![
                    ("q", query.to_owned()),
                    ("type", "artist".into()),
                    ("per_page", page_size.to_string()),
                    ("page", page.to_string()),
                ];
                let data = self
                    .discogs_json(
                        "database/search",
                        &params,
                        "artists.search.discogs",
                        &cache_id,
                        SEARCH_TTL,
                        token,
                    )
                    .await?;
                let rows = data["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_discogs_artist)
                    .collect::<Vec<ArtistSearchResult>>();
                let count = data["pagination"]["items"]
                    .as_u64()
                    .map(|n| n.min(u32::MAX as u64) as u32);
                let more = data["pagination"]["pages"]
                    .as_u64()
                    .is_some_and(|n| n > page as u64);
                (rows, count, more)
            }
            _ => return Err("Unsupported artist search provider".into()),
        };
        if provider == "musicbrainz" {
            self.attach_cached_musicbrainz_artist_images(&mut results);
        }
        Ok(ArtistSearchPage {
            query: query.into(),
            page,
            page_size,
            total,
            has_more,
            results,
            warnings: credentials.warnings.clone(),
        })
    }

    pub async fn search_albums(
        &self,
        query: &str,
        page: u32,
        credentials: &ArtistCredentials,
    ) -> Result<AlbumSearchPage, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 120 {
            return Err("Album search must contain 1–120 characters".into());
        }
        if !(1..=200).contains(&page) {
            return Err("Album search page is out of range".into());
        }
        let offset = (page - 1) * PAGE_SIZE;
        let cache_key = format!("v2:{}:{page}", query.to_lowercase());
        let mut items = Vec::new();
        let mut has_more = false;
        let mut warnings = credentials.warnings.clone();
        let mb_query = vec![
            ("query", musicbrainz_release_group_search_query(query)),
            ("limit", PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
        ];
        match self
            .musicbrainz_json(
                "release-group",
                &mb_query,
                MUSICBRAINZ_RELEASE_GROUP_SEARCH_CACHE,
                &cache_key,
                SEARCH_TTL,
            )
            .await
        {
            Ok(data) => {
                let count = data["count"].as_u64().unwrap_or(0) as u32;
                if let Some(groups) = data["release-groups"].as_array() {
                    items.extend(groups.iter().filter_map(parse_musicbrainz_release_group));
                }
                has_more |= offset.saturating_add(PAGE_SIZE) < count;
            }
            Err(error) => warnings.push(format!("MusicBrainz album search failed: {error}")),
        }
        if let Some(token) = credentials
            .discogs_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let discogs_query = vec![
                ("q", query.to_owned()),
                ("type", "release".into()),
                ("per_page", PAGE_SIZE.to_string()),
                ("page", page.to_string()),
            ];
            match self
                .discogs_json(
                    "database/search",
                    &discogs_query,
                    "albums.search.discogs",
                    &cache_key,
                    SEARCH_TTL,
                    token,
                )
                .await
            {
                Ok(data) => {
                    if let Some(results) = data["results"].as_array() {
                        items.extend(results.iter().filter_map(parse_discogs_release));
                    }
                    has_more |= data["pagination"]["pages"]
                        .as_u64()
                        .is_some_and(|pages| pages > page as u64);
                }
                Err(error) => warnings.push(format!("Discogs album search failed: {error}")),
            }
        } else {
            warnings.push(
                "Discogs album search is not configured; connect a token to include its catalog."
                    .into(),
            );
        }
        if items.is_empty()
            && !warnings.is_empty()
            && warnings.iter().all(|warning| warning.contains("failed:"))
        {
            return Err(warnings.join("; "));
        }
        let mut unique = Vec::new();
        for item in items {
            if !unique.iter().any(|existing: &ArtistReleaseRef| {
                existing.release.provider == item.release.provider
                    && existing.source_url == item.source_url
            }) {
                unique.push(item);
            }
        }
        Ok(AlbumSearchPage {
            items: unique,
            next_page: has_more.then_some(page + 1),
            warnings,
        })
    }

    /// Provider-scoped release search used by unified discovery sources.
    /// The combined `search_albums` API remains unchanged for Artist Pages.
    pub async fn search_albums_for(
        &self,
        provider: &str,
        query: &str,
        page: u32,
        credentials: &ArtistCredentials,
    ) -> Result<AlbumSearchPage, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 120 {
            return Err("Album search must contain 1–120 characters".into());
        }
        if !(1..=200).contains(&page) {
            return Err("Album search page is out of range".into());
        }
        let offset = (page - 1) * PAGE_SIZE;
        let cache_key = format!("v2:{}:{page}", query.to_lowercase());
        let (items, has_more) = match provider {
            "musicbrainz" => {
                let params = vec![
                    ("query", musicbrainz_release_group_search_query(query)),
                    ("limit", PAGE_SIZE.to_string()),
                    ("offset", offset.to_string()),
                ];
                let data = self
                    .musicbrainz_json(
                        "release-group",
                        &params,
                        MUSICBRAINZ_RELEASE_GROUP_SEARCH_CACHE,
                        &cache_key,
                        SEARCH_TTL,
                    )
                    .await?;
                let count = data["count"].as_u64().unwrap_or(0) as u32;
                let items = data["release-groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_musicbrainz_release_group)
                    .collect();
                (items, offset.saturating_add(PAGE_SIZE) < count)
            }
            "discogs" => {
                let token = credentials
                    .discogs_token
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("Discogs is not configured")?;
                let params = vec![
                    ("q", query.to_owned()),
                    ("type", "release".into()),
                    ("per_page", PAGE_SIZE.to_string()),
                    ("page", page.to_string()),
                ];
                let data = self
                    .discogs_json(
                        "database/search",
                        &params,
                        "albums.search.discogs",
                        &cache_key,
                        SEARCH_TTL,
                        token,
                    )
                    .await?;
                let items = data["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_discogs_release)
                    .collect();
                let more = data["pagination"]["pages"]
                    .as_u64()
                    .is_some_and(|n| n > page as u64);
                (items, more)
            }
            _ => return Err("Unsupported album search provider".into()),
        };
        Ok(AlbumSearchPage {
            items,
            next_page: has_more.then_some(page + 1),
            warnings: credentials.warnings.clone(),
        })
    }

    async fn wikidata_portrait(&self, qid: &str) -> Option<(String, String)> {
        let namespace = "artists.portrait.wikidata.v1";
        if let Ok(Some(cached)) = self.store.get_provider_cache(namespace, qid) {
            return Some((
                cached["imageUrl"].as_str()?.to_owned(),
                cached["sourceUrl"].as_str()?.to_owned(),
            ));
        }
        // This ID comes only from the exact MusicBrainz identity's explicit URL
        // relation. Never find a portrait by guessing an artist's name.
        let mut response = self
            .client
            .get(format!(
                "https://www.wikidata.org/wiki/Special:EntityData/{qid}.json?flavor=simple"
            ))
            .send()
            .await
            .ok()?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|n| n > 1024 * 1024)
        {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        let raw: Value = serde_json::from_slice(&bytes).ok()?;
        let image = wikidata_portrait_urls(qid, &raw);
        let payload = image
            .as_ref()
            .map(|(image, source)| json!({"imageUrl":image,"sourceUrl":source}))
            .unwrap_or(json!({"imageUrl":null,"sourceUrl":null}));
        let _ = self
            .store
            .set_provider_cache(namespace, qid, &payload, PROFILE_TTL);
        image
    }

    fn attach_cached_musicbrainz_artist_images(&self, results: &mut [ArtistSearchResult]) {
        for result in results
            .iter_mut()
            .filter(|result| result.artist.provider == "musicbrainz")
        {
            result.image_url = self.cached_musicbrainz_artist_image(&result.artist.id);
        }
    }

    /// Reuse portrait enrichment already loaded by an artist profile without
    /// making one or more extra HTTP requests per search result.
    pub fn cached_musicbrainz_artist_image(&self, artist_id: &str) -> Option<String> {
        if !valid_mbid(artist_id) {
            return None;
        }
        if let Ok(Some(cached)) = self
            .store
            .get_provider_cache("artists.portrait.artist.v1", artist_id)
        {
            if let Some(image) = cached["imageUrl"].as_str().and_then(secure_url) {
                return Some(image);
            }
        }
        let profile = self
            .store
            .get_provider_cache("artists.profile.musicbrainz", artist_id)
            .ok()??;
        let qid = profile["relations"]
            .as_array()?
            .iter()
            .find_map(|relation| wikidata_item_id(relation["url"]["resource"].as_str()?))?;
        let portrait = self
            .store
            .get_provider_cache("artists.portrait.wikidata.v1", &qid)
            .ok()??;
        portrait["imageUrl"].as_str().and_then(secure_url)
    }

    /// Search thumbnails must not wait for biography, top tracks or discography.
    /// Resolve only artwork attached to the exact provider identity.
    pub async fn get_artist_artwork(
        &self,
        artist: &ArtistRef,
        credentials: &ArtistCredentials,
    ) -> Result<Option<String>, String> {
        validate_artist_ref(artist)?;
        if artist.provider == "musicbrainz" {
            if let Some(image) = self.cached_musicbrainz_artist_image(&artist.id) {
                return Ok(Some(image));
            }
        }
        let mut wikidata = None;
        let discogs = if artist.provider == "musicbrainz" {
            let data = self
                .musicbrainz_json(
                    &format!("artist/{}", artist.id),
                    &[("inc", "aliases+genres+tags+artist-rels+url-rels".into())],
                    "artists.profile.musicbrainz",
                    &artist.id,
                    PROFILE_TTL,
                )
                .await?;
            let links = data["relations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["url"]["resource"].as_str())
                .collect::<Vec<_>>();
            wikidata = links.iter().find_map(|url| wikidata_item_id(url));
            links.iter().find_map(|url| discogs_artist_id(url))
        } else {
            Some(artist.id.clone())
        };
        let mut image = None;
        if let Some(id) = discogs {
            let cached = self
                .store
                .get_provider_cache("artists.profile.discogs", &id)
                .ok()
                .flatten();
            let data = if cached.is_some() {
                cached
            } else if let Some(token) = credentials
                .discogs_token
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                tokio::time::timeout(
                    Duration::from_secs(3),
                    self.discogs_json(
                        &format!("artists/{id}"),
                        &[],
                        "artists.profile.discogs",
                        &id,
                        PROFILE_TTL,
                        token,
                    ),
                )
                .await
                .ok()
                .and_then(Result::ok)
            } else {
                None
            };
            image = data.and_then(|data| {
                data["images"].as_array().and_then(|items| {
                    items
                        .iter()
                        .find_map(|item| item["uri"].as_str().and_then(secure_url))
                })
            });
        }
        if image.is_none() {
            if let Some(qid) = wikidata {
                image = tokio::time::timeout(Duration::from_secs(3), self.wikidata_portrait(&qid))
                    .await
                    .ok()
                    .flatten()
                    .map(|(image, _)| image);
            }
        }
        if image.is_none() && artist.provider == "musicbrainz" {
            let cached = self
                .store
                .get_provider_cache("artists.profile.lastfm", &artist.id)
                .ok()
                .flatten();
            let data = if cached.is_some() {
                cached
            } else if let Some(key) = credentials
                .lastfm_api_key
                .as_deref()
                .filter(|key| !key.trim().is_empty())
            {
                tokio::time::timeout(
                    Duration::from_secs(3),
                    self.lastfm_artist_info(artist, "", key),
                )
                .await
                .ok()
                .and_then(Result::ok)
            } else {
                None
            };
            image = data.and_then(|data| {
                data["artist"]["image"].as_array().and_then(|items| {
                    items
                        .iter()
                        .rev()
                        .find_map(|item| item["#text"].as_str().and_then(secure_url))
                })
            });
        }
        if artist.provider == "musicbrainz" {
            if let Some(image) = image.as_ref() {
                let _ = self.store.set_provider_cache(
                    "artists.portrait.artist.v1",
                    &artist.id,
                    &json!({"imageUrl":image}),
                    PROFILE_TTL,
                );
            }
        }
        Ok(image)
    }

    pub async fn get_artist_profile(
        &self,
        artist: &ArtistRef,
        credentials: &ArtistCredentials,
    ) -> Result<ArtistProfile, String> {
        validate_artist_ref(artist)?;
        let mut warnings = credentials.warnings.clone();
        let mut genres;
        let mut tags;
        let mut aliases;
        let mut links = Vec::new();
        let mut top_tracks = Vec::new();
        let mut biography = None;
        let mut biography_source = None;
        let mut image_url = None;
        let mut lastfm_available = false;
        let mut discogs_crosslink = (artist.provider == "discogs").then(|| artist.id.clone());
        let mut base_artist = match artist.provider.as_str() {
            "musicbrainz" => {
                let data = self
                    .musicbrainz_json(
                        &format!("artist/{}", artist.id),
                        &[("inc", "aliases+genres+tags+artist-rels+url-rels".into())],
                        "artists.profile.musicbrainz",
                        &artist.id,
                        PROFILE_TTL,
                    )
                    .await?;
                let profile = parse_musicbrainz_artist(&data)
                    .ok_or("MusicBrainz artist response omitted its identity")?;
                for relation in data["relations"].as_array().into_iter().flatten() {
                    if let Some(resource) = relation["url"]["resource"].as_str() {
                        if let Some(url) = secure_url(resource) {
                            let label = relation["type"].as_str().unwrap_or("External link");
                            let provider = link_provider(&url);
                            links.push(ArtistExternalLink {
                                label: label.into(),
                                url: url.clone(),
                                provider,
                            });
                            if let Some(discogs_id) = discogs_artist_id(&url) {
                                discogs_crosslink = Some(discogs_id.clone());
                                // A MusicBrainz URL relation is an explicit cross-link, never a name match.
                                let _ = self.store.save_artist_mapping(
                                    "discogs",
                                    &discogs_id,
                                    &format!("musicbrainz:{}", artist.id),
                                );
                                let _ = self.store.save_artist_mapping(
                                    "musicbrainz",
                                    &artist.id,
                                    &format!("discogs:{discogs_id}"),
                                );
                            }
                        }
                    }
                }
                aliases = data["aliases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|alias| alias["name"].as_str().map(str::to_owned))
                    .collect();
                genres = named_items(&data["genres"]);
                tags = named_items(&data["tags"]);
                profile
            }
            "discogs" => {
                let token = credentials
                    .discogs_token
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("Connect a Discogs token to view this artist")?;
                let data = self
                    .discogs_json(
                        &format!("artists/{}", artist.id),
                        &[],
                        "artists.profile.discogs",
                        &artist.id,
                        PROFILE_TTL,
                        token,
                    )
                    .await?;
                let profile = parse_discogs_artist(&data)
                    .ok_or("Discogs artist response omitted its identity")?;
                aliases = string_array(&data["aliases"]);
                genres = string_array(&data["genres"]);
                tags = string_array(&data["groups"]);
                biography = data["profile"]
                    .as_str()
                    .map(str::to_owned)
                    .filter(|s| !s.trim().is_empty());
                biography_source = biography.as_ref().map(|_| "Discogs".to_owned());
                image_url = data["images"].as_array().and_then(|items| {
                    items
                        .iter()
                        .find_map(|item| item["uri"].as_str().and_then(secure_url))
                });
                for url in string_array(&data["urls"]) {
                    if let Some(url) = secure_url(&url) {
                        let provider = link_provider(&url);
                        if let Some(mbid) = musicbrainz_artist_id(&url) {
                            let _ = self.store.save_artist_mapping(
                                "musicbrainz",
                                &mbid,
                                &format!("discogs:{}", artist.id),
                            );
                        }
                        links.push(ArtistExternalLink {
                            label: provider.clone().unwrap_or_else(|| "Artist link".into()),
                            url,
                            provider,
                        });
                    }
                }
                profile
            }
            _ => return Err("Unsupported artist identity provider".into()),
        };
        let mut source_tags = Vec::new();
        source_tags.extend(genres.iter().cloned().map(|name| ArtistTag {
            name,
            provider: artist.provider.clone(),
            kind: "genre".into(),
        }));
        source_tags.extend(tags.iter().cloned().map(|name| ArtistTag {
            name,
            provider: artist.provider.clone(),
            kind: "tag".into(),
        }));
        let discogs_configured = credentials
            .discogs_token
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        let mut discogs_available = artist.provider == "discogs";
        if artist.provider == "musicbrainz" {
            if let Some(discogs_id) = discogs_crosslink.as_deref() {
                if let Some(token) = credentials
                    .discogs_token
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    match self
                        .discogs_json(
                            &format!("artists/{discogs_id}"),
                            &[],
                            "artists.profile.discogs",
                            discogs_id,
                            PROFILE_TTL,
                            token,
                        )
                        .await
                    {
                        Ok(data) => {
                            discogs_available = true;
                            if biography.is_none() {
                                biography = data["profile"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .filter(|value| !value.trim().is_empty());
                                biography_source = biography.as_ref().map(|_| "Discogs".to_owned());
                            }
                            if image_url.is_none() {
                                image_url = data["images"].as_array().and_then(|items| {
                                    items
                                        .iter()
                                        .find_map(|item| item["uri"].as_str().and_then(secure_url))
                                });
                            }
                            extend_unique(&mut aliases, string_array(&data["aliases"]));
                            let discogs_genres = string_array(&data["genres"]);
                            let discogs_tags = string_array(&data["groups"]);
                            source_tags.extend(discogs_genres.iter().cloned().map(|name| {
                                ArtistTag {
                                    name,
                                    provider: "discogs".into(),
                                    kind: "genre".into(),
                                }
                            }));
                            source_tags.extend(discogs_tags.iter().cloned().map(|name| {
                                ArtistTag {
                                    name,
                                    provider: "discogs".into(),
                                    kind: "tag".into(),
                                }
                            }));
                            extend_unique(&mut genres, discogs_genres);
                            extend_unique(&mut tags, discogs_tags);
                            for raw_url in string_array(&data["urls"]) {
                                if let Some(url) = secure_url(&raw_url) {
                                    if !links.iter().any(|link| link.url == url) {
                                        links.push(ArtistExternalLink {
                                            label: link_provider(&url)
                                                .unwrap_or_else(|| "Discogs link".into()),
                                            provider: link_provider(&url),
                                            url,
                                        });
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            warnings.push(format!("Discogs details unavailable: {error}"))
                        }
                    }
                }
            }
        }
        if let Some(token) = credentials
            .lastfm_api_key
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            let lastfm_result = self
                .lastfm_artist_info(artist, &base_artist.name, token)
                .await;
            match lastfm_result {
                Ok(data) => {
                    lastfm_available = true;
                    if biography.is_none() {
                        biography = data["bio"]["content"]
                            .as_str()
                            .map(strip_html)
                            .filter(|value| !value.is_empty());
                        biography_source = biography.as_ref().map(|_| "Last.fm".to_owned());
                    }
                    if image_url.is_none() {
                        image_url = data["image"].as_array().and_then(|items| {
                            items.iter().rev().find_map(|item| {
                                item["#text"]
                                    .as_str()
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned)
                            })
                        });
                    }
                    let lastfm_tags = named_items(&data["tags"]["tag"]);
                    source_tags.extend(lastfm_tags.iter().cloned().map(|name| ArtistTag {
                        name,
                        provider: "lastfm".into(),
                        kind: "tag".into(),
                    }));
                    for tag in lastfm_tags {
                        if !tags.contains(&tag) {
                            tags.push(tag);
                        }
                    }
                    match self
                        .lastfm_top_tracks(artist, &base_artist.name, token)
                        .await
                    {
                        Ok(tracks) => {
                            top_tracks = parse_lastfm_tracks(&tracks, &base_artist.name, artist)
                        }
                        Err(error) => {
                            warnings.push(format!("Last.fm top tracks unavailable: {error}"))
                        }
                    }
                }
                Err(error) => warnings.push(format!("Last.fm artist details unavailable: {error}")),
            }
        }
        if image_url.is_none() && artist.provider == "musicbrainz" {
            if let Some(qid) = links.iter().find_map(|link| wikidata_item_id(&link.url)) {
                if let Ok(Some((image, source))) =
                    tokio::time::timeout(Duration::from_secs(3), self.wikidata_portrait(&qid)).await
                {
                    image_url = Some(image);
                    links.push(ArtistExternalLink {
                        label: "Artist image (Wikimedia Commons)".into(),
                        url: source,
                        provider: Some("wikimedia".into()),
                    });
                }
            }
        }
        if artist.provider == "musicbrainz" {
            if let Some(image) = image_url.as_deref() {
                let _ = self.store.set_provider_cache(
                    "artists.portrait.artist.v1",
                    &artist.id,
                    &json!({"imageUrl":image}),
                    PROFILE_TTL,
                );
            }
        }
        let releases = self
            .get_artist_releases(artist, None, credentials)
            .await
            .unwrap_or_else(|error| {
                warnings.push(format!("Artist releases unavailable: {error}"));
                ArtistReleasePage {
                    artist: artist.clone(),
                    items: Vec::new(),
                    next_cursor: None,
                    source_coverage: Vec::new(),
                    warnings: vec![],
                }
            });
        warnings.extend(releases.warnings.iter().cloned());
        let discogs_status = if discogs_available {
            (
                "available",
                "Explicitly linked Discogs artist data was loaded.",
            )
        } else if discogs_crosslink.is_none() {
            (
                "not linked",
                "No explicit Discogs cross-link is recorded for this identity.",
            )
        } else if discogs_configured {
            (
                "unavailable",
                "A Discogs identity is linked, but its details could not be loaded.",
            )
        } else {
            (
                "not configured",
                "Connect Discogs to load this linked artist profile.",
            )
        };
        let lastfm_configured = credentials
            .lastfm_api_key
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        let lastfm_status = if lastfm_available {
            ("available", "Last.fm profile details loaded.")
        } else if lastfm_configured {
            (
                "unavailable",
                "Last.fm did not return profile details for this identity.",
            )
        } else {
            (
                "not configured",
                "Connect Last.fm to load tags and top tracks.",
            )
        };
        let mut source_coverage = vec![ArtistSourceCoverage {
            provider: artist.provider.clone(),
            status: "available".into(),
            detail: "Selected identity and source profile loaded.".into(),
            source_url: Some(base_artist.source_url.clone()),
        }];
        if artist.provider != "discogs" {
            source_coverage.push(ArtistSourceCoverage {
                provider: "discogs".into(),
                status: discogs_status.0.into(),
                detail: discogs_status.1.into(),
                source_url: discogs_crosslink
                    .as_ref()
                    .map(|id| format!("https://www.discogs.com/artist/{id}")),
            });
        }
        source_coverage.push(ArtistSourceCoverage {
            provider: "lastfm".into(),
            status: lastfm_status.0.into(),
            detail: lastfm_status.1.into(),
            source_url: Some("https://www.last.fm/".into()),
        });
        base_artist.source_url = if artist.provider == "musicbrainz" {
            format!("https://musicbrainz.org/artist/{}", artist.id)
        } else {
            format!("https://www.discogs.com/artist/{}", artist.id)
        };
        Ok(ArtistProfile {
            artist: base_artist,
            biography,
            biography_source,
            image_url,
            genres: dedupe(genres),
            tags: dedupe(tags),
            source_tags: dedupe_source_tags(source_tags),
            aliases: dedupe(aliases),
            links,
            top_tracks,
            releases,
            discogs_available,
            lastfm_available,
            source_coverage,
            warnings,
        })
    }

    async fn lastfm_artist_info(
        &self,
        artist: &ArtistRef,
        name: &str,
        key: &str,
    ) -> Result<Value, String> {
        let mut url = Url::parse(&self.endpoints.lastfm).map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("method", "artist.getInfo");
        url.query_pairs_mut().append_pair("api_key", key);
        url.query_pairs_mut().append_pair("format", "json");
        if artist.provider == "musicbrainz" {
            url.query_pairs_mut().append_pair("mbid", &artist.id);
        } else {
            url.query_pairs_mut().append_pair("artist", name);
        }
        self.get_json("artists.profile.lastfm", &artist.id, url, PROFILE_TTL, None)
            .await
    }

    async fn lastfm_top_tracks(
        &self,
        artist: &ArtistRef,
        name: &str,
        key: &str,
    ) -> Result<Value, String> {
        let mut url = Url::parse(&self.endpoints.lastfm).map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("method", "artist.getTopTracks");
        url.query_pairs_mut().append_pair("api_key", key);
        url.query_pairs_mut().append_pair("format", "json");
        if artist.provider == "musicbrainz" {
            url.query_pairs_mut().append_pair("mbid", &artist.id);
        } else {
            url.query_pairs_mut().append_pair("artist", name);
        }
        self.get_json("artists.tracks.lastfm", &artist.id, url, LIST_TTL, None)
            .await
    }

    pub async fn get_artist_releases(
        &self,
        artist: &ArtistRef,
        cursor: Option<&str>,
        credentials: &ArtistCredentials,
    ) -> Result<ArtistReleasePage, String> {
        validate_artist_ref(artist)?;
        let (mut mb_offset, mut discogs_page) = parse_release_cursor(cursor)?;
        let mut warnings = credentials.warnings.clone();
        let mut source_coverage = Vec::new();
        let mut items = Vec::new();
        let (mbid, discogs_id) = match artist.provider.as_str() {
            "musicbrainz" => (
                Some(artist.id.clone()),
                self.store
                    .get_artist_mapping("musicbrainz", &artist.id)?
                    .and_then(|value| value.strip_prefix("discogs:").map(str::to_owned)),
            ),
            "discogs" => (
                self.store
                    .get_artist_mapping("discogs", &artist.id)?
                    .and_then(|value| value.strip_prefix("musicbrainz:").map(str::to_owned)),
                Some(artist.id.clone()),
            ),
            _ => return Err("Unsupported artist identity provider".into()),
        };
        if let Some(mbid) = mbid.filter(|id| valid_mbid(id)) {
            if let Some(offset) = mb_offset {
                match self.fetch_musicbrainz_releases(&mbid, offset, artist).await {
                    Ok((page_items, more, raw_len)) => {
                        items.extend(page_items);
                        mb_offset = more.then(|| offset.saturating_add(raw_len));
                        source_coverage.push(ArtistSourceCoverage {
                            provider: "musicbrainz".into(),
                            status: "available".into(),
                            detail: "Paginated release and track data loaded.".into(),
                            source_url: Some(format!("https://musicbrainz.org/artist/{mbid}")),
                        });
                    }
                    Err(error) => {
                        warnings.push(format!("MusicBrainz releases unavailable: {error}"));
                        source_coverage.push(ArtistSourceCoverage {
                            provider: "musicbrainz".into(),
                            status: "unavailable".into(),
                            detail: "Release request failed.".into(),
                            source_url: Some(format!("https://musicbrainz.org/artist/{mbid}")),
                        });
                    }
                }
            }
        } else {
            mb_offset = None;
            source_coverage.push(ArtistSourceCoverage {
                provider: "musicbrainz".into(),
                status: "not linked".into(),
                detail: "No explicit MusicBrainz identity link is recorded.".into(),
                source_url: None,
            });
        }
        if let Some(discogs_id) =
            discogs_id.filter(|id| id.chars().all(|character| character.is_ascii_digit()))
        {
            let url = format!("https://www.discogs.com/artist/{discogs_id}");
            if let Some(token) = credentials
                .discogs_token
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                if let Some(page) = discogs_page {
                    match self.fetch_discogs_releases(&discogs_id, page, token).await {
                        Ok((page_items, more)) => {
                            items.extend(page_items);
                            discogs_page = more.then(|| page.saturating_add(1));
                            source_coverage.push(ArtistSourceCoverage {
                                provider: "discogs".into(),
                                status: "available".into(),
                                detail: "Paginated release data loaded.".into(),
                                source_url: Some(url),
                            });
                        }
                        Err(error) => {
                            warnings.push(format!("Discogs releases unavailable: {error}"));
                            source_coverage.push(ArtistSourceCoverage {
                                provider: "discogs".into(),
                                status: "unavailable".into(),
                                detail: "Release request failed.".into(),
                                source_url: Some(url),
                            });
                        }
                    }
                } else {
                    source_coverage.push(ArtistSourceCoverage {
                        provider: "discogs".into(),
                        status: "available".into(),
                        detail: "All Discogs release pages are loaded.".into(),
                        source_url: Some(url),
                    });
                }
            } else {
                discogs_page = None;
                source_coverage.push(ArtistSourceCoverage {
                    provider: "discogs".into(),
                    status: "not configured".into(),
                    detail:
                        "A linked Discogs identity exists; connect a token to load its releases."
                            .into(),
                    source_url: Some(url),
                });
            }
        } else {
            discogs_page = None;
            source_coverage.push(ArtistSourceCoverage {
                provider: "discogs".into(),
                status: "not linked".into(),
                detail: "No explicit Discogs identity link is recorded.".into(),
                source_url: None,
            });
        }
        let mut deduped: Vec<ArtistReleaseRef> = Vec::new();
        for item in items {
            if let Some(existing) = deduped
                .iter_mut()
                .find(|current| current.source_url == item.source_url)
            {
                for credit in item.credits {
                    if !existing.credits.iter().any(|known| {
                        known.name == credit.name
                            && known.role == credit.role
                            && known.credited_as == credit.credited_as
                    }) {
                        existing.credits.push(credit);
                    }
                }
            } else {
                deduped.push(item);
            }
        }
        let next_cursor = match (mb_offset, discogs_page) {
            (Some(mb), Some(discogs)) => Some(format!("r3:{mb}:{discogs}")),
            (Some(mb), None) => Some(format!("r3:{mb}:x")),
            (None, Some(discogs)) => Some(format!("r3:x:{discogs}")),
            (None, None) => None,
        };
        Ok(ArtistReleasePage {
            artist: artist.clone(),
            items: deduped,
            next_cursor,
            source_coverage,
            warnings,
        })
    }

    /// Resolve a release identity into its ordered catalog tracklist for playback and queueing.
    /// No audio is fetched here; tracks carry `sourceKind=catalog` so playback may search
    /// public audio sources while acquisition still requires an explicit user action.
    pub async fn get_release_tracks(
        &self,
        release: &ArtistReleaseRef,
        artist: &ArtistRef,
        credentials: &ArtistCredentials,
    ) -> Result<Vec<Track>, String> {
        validate_artist_ref(artist)?;
        let edition = match release.release.provider.as_str() {
            "musicbrainz" => {
                if !valid_crosswalk_mbid(&release.release.id) {
                    return Err("Invalid MusicBrainz release identity".into());
                }
                let entity =
                    trusted_musicbrainz_release_entity(&release.source_url, &release.release.id)
                        .ok_or("MusicBrainz source URL does not match its release identity")?;
                let edition_id = match entity.as_str() {
                    "release" => release.release.id.clone(),
                    "release-group" => {
                        let data = self
                            .musicbrainz_json(
                                &format!("release-group/{}", release.release.id),
                                &[("inc", "releases+artist-credits".into())],
                                "artists.release-group.musicbrainz",
                                &release.release.id,
                                LIST_TTL,
                            )
                            .await?;
                        choose_representative_release(&data["releases"])
                            .ok_or("MusicBrainz release group has no representative edition")?
                    }
                    _ => return Err("Unsupported MusicBrainz release entity".into()),
                };
                if !valid_crosswalk_mbid(&edition_id) {
                    return Err("MusicBrainz returned an invalid edition identity".into());
                }
                let data = self
                    .musicbrainz_json(
                        &format!("release/{edition_id}"),
                        &[(
                            "inc",
                            "artist-credits+labels+recordings+media+isrcs+release-groups".into(),
                        )],
                        "artists.release-tracks.musicbrainz",
                        &edition_id,
                        LIST_TTL,
                    )
                    .await?;
                let selected_name = selected_artist_name_for_release(release, artist);
                parse_musicbrainz_release_tracks(
                    &data,
                    &edition_id,
                    &release.source_url,
                    entity == "release-group",
                    selected_name.as_deref(),
                )
            }
            "discogs" => {
                if release.release.id.is_empty()
                    || release.release.id.len() > 20
                    || !release.release.id.chars().all(|c| c.is_ascii_digit())
                {
                    return Err("Invalid Discogs release identity".into());
                }
                let entity =
                    trusted_discogs_release_entity(&release.source_url, &release.release.id)
                        .ok_or("Discogs source URL does not match its release identity")?;
                let token = credentials
                    .discogs_token
                    .as_deref()
                    .filter(|token| !token.trim().is_empty())
                    .ok_or("Connect a Discogs token to view this release tracklist")?;
                let (edition_id, data) = if entity == "master" {
                    let master = self
                        .discogs_json(
                            &format!("masters/{}", release.release.id),
                            &[],
                            "artists.release-tracks.discogs-master",
                            &release.release.id,
                            LIST_TTL,
                            token,
                        )
                        .await?;
                    let main_release = master["main_release"]
                        .as_u64()
                        .map(|id| id.to_string())
                        .or_else(|| master["main_release"].as_str().map(str::to_owned))
                        .filter(|id| {
                            !id.is_empty()
                                && id.len() <= 20
                                && id.chars().all(|c| c.is_ascii_digit())
                        })
                        .ok_or("Discogs master has no valid main release edition")?;
                    let data = self
                        .discogs_json(
                            &format!("releases/{main_release}"),
                            &[],
                            "artists.release-tracks.discogs-release",
                            &main_release,
                            LIST_TTL,
                            token,
                        )
                        .await?;
                    (main_release, data)
                } else {
                    let data = self
                        .discogs_json(
                            &format!("releases/{}", release.release.id),
                            &[],
                            "artists.release-tracks.discogs-release",
                            &release.release.id,
                            LIST_TTL,
                            token,
                        )
                        .await?;
                    (release.release.id.clone(), data)
                };
                let selected_name = selected_artist_name_for_release(release, artist);
                parse_discogs_release_tracks(
                    &data,
                    &edition_id,
                    &release.source_url,
                    entity == "master",
                    selected_name.as_deref(),
                )
            }
            _ => return Err("Unsupported release provider".into()),
        };
        Ok(edition)
    }

    async fn fetch_musicbrainz_releases(
        &self,
        mbid: &str,
        offset: u32,
        selected_artist: &ArtistRef,
    ) -> Result<(Vec<ArtistReleaseRef>, bool, u32), String> {
        let query = vec![
            ("artist", mbid.to_owned()),
            ("limit", ARTIST_LIST_PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
            ("inc", "artist-credits+labels+media+release-groups".into()),
        ];
        let data = self
            .musicbrainz_json(
                "release",
                &query,
                "artists.releases.musicbrainz.v2",
                &format!("{mbid}:{offset}"),
                LIST_TTL,
            )
            .await?;
        let count = data["release-count"]
            .as_u64()
            .or_else(|| data["count"].as_u64())
            .unwrap_or(0) as u32;
        let raw_len = data["releases"].as_array().map_or(0, Vec::len) as u32;
        let releases = data["releases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|release| parse_musicbrainz_release(release, selected_artist))
            .collect();
        Ok((releases, offset.saturating_add(raw_len) < count, raw_len))
    }

    async fn fetch_discogs_releases(
        &self,
        discogs_id: &str,
        page: u32,
        token: &str,
    ) -> Result<(Vec<ArtistReleaseRef>, bool), String> {
        let query = vec![
            ("page", page.to_string()),
            ("per_page", ARTIST_LIST_PAGE_SIZE.to_string()),
            ("sort", "year".into()),
            ("sort_order", "asc".into()),
        ];
        let data = self
            .discogs_json(
                &format!("artists/{discogs_id}/releases"),
                &query,
                "artists.releases.discogs",
                &format!("{discogs_id}:{page}"),
                LIST_TTL,
                token,
            )
            .await?;
        let raw_len = data["releases"].as_array().map_or(0, Vec::len);
        let pages = data["pagination"]["pages"].as_u64().unwrap_or(page as u64) as u32;
        let releases = data["releases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_discogs_release)
            .collect();
        Ok((releases, raw_len > 0 && page < pages))
    }

    pub async fn get_artist_tracks(
        &self,
        artist: &ArtistRef,
        cursor: Option<&str>,
        _credentials: &ArtistCredentials,
    ) -> Result<ArtistTrackPage, String> {
        validate_artist_ref(artist)?;
        let offset = parse_cursor(cursor)?;
        let mbid = musicbrainz_id_for(artist, &self.store)?;
        let Some(mbid) = mbid else {
            return Ok(ArtistTrackPage {
                artist: artist.clone(), items: Vec::new(), next_cursor: None,
                source: "MusicBrainz recordings".into(),
                warnings: vec!["No explicit MusicBrainz cross-link is available for this Discogs artist; no same-name match was assumed.".into()],
            });
        };
        let query = vec![
            ("artist", mbid.clone()),
            ("limit", ARTIST_LIST_PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
            ("inc", "artist-credits+isrcs+genres+tags".into()),
        ];
        let data = self
            .musicbrainz_json(
                "recording",
                &query,
                "artists.tracks.musicbrainz",
                &format!("{mbid}:{offset}"),
                LIST_TTL,
            )
            .await?;
        let total = data["count"].as_u64().unwrap_or(0) as u32;
        let items = data["recordings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|recording| parse_musicbrainz_recording(recording, artist))
            .collect::<Vec<_>>();
        let next = if offset + (items.len() as u32) < total {
            Some((offset + items.len() as u32).to_string())
        } else {
            None
        };
        Ok(ArtistTrackPage {
            artist: artist.clone(),
            items,
            next_cursor: next,
            source: "MusicBrainz recordings".into(),
            warnings: vec![],
        })
    }

    pub async fn get_artist_collaborations(
        &self,
        artist: &ArtistRef,
        cursor: Option<&str>,
        credentials: &ArtistCredentials,
    ) -> Result<ArtistCollaborationPage, String> {
        validate_artist_ref(artist)?;
        let offset = parse_cursor(cursor)? as usize;
        let mbid = musicbrainz_id_for(artist, &self.store)?;
        let mut warnings = credentials.warnings.clone();
        let mut source_coverage = Vec::new();
        let mut items = Vec::new();
        if let Some(mbid) = mbid.as_deref() {
            let source = format!("https://musicbrainz.org/artist/{mbid}");
            match self
                .musicbrainz_json(
                    &format!("artist/{mbid}"),
                    &[("inc", "artist-rels".into())],
                    "artists.collaborations.musicbrainz",
                    mbid,
                    PROFILE_TTL,
                )
                .await
            {
                Ok(data) => {
                    let name = data["name"].as_str().unwrap_or("Unknown artist");
                    for relation in data["relations"].as_array().into_iter().flatten() {
                        let Some(related) = relation["artist"].as_object() else {
                            continue;
                        };
                        let Some(id) = related.get("id").and_then(Value::as_str) else {
                            continue;
                        };
                        let related_name = related
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("Unknown artist");
                        let role = relation["type"].as_str().unwrap_or("related artist");
                        let related_url = format!("https://musicbrainz.org/artist/{id}");
                        items.push(artist_relationship(
                            name,
                            &artist.provider,
                            &artist.id,
                            related_name,
                            "musicbrainz",
                            id,
                            role,
                            &source,
                            &related_url,
                            relation["direction"]
                                .as_str()
                                .map(|direction| format!("Relationship direction: {direction}")),
                        ));
                    }
                    source_coverage.push(ArtistSourceCoverage {
                        provider: "musicbrainz".into(),
                        status: "available".into(),
                        detail: "Artist relationships loaded.".into(),
                        source_url: Some(source),
                    });
                }
                Err(error) => {
                    warnings.push(format!("MusicBrainz relationships unavailable: {error}"));
                    source_coverage.push(ArtistSourceCoverage {
                        provider: "musicbrainz".into(),
                        status: "unavailable".into(),
                        detail: "Relationship request failed.".into(),
                        source_url: Some(source),
                    });
                }
            }
        } else {
            source_coverage.push(ArtistSourceCoverage {
                provider: "musicbrainz".into(),
                status: "not linked".into(),
                detail: "No explicit MusicBrainz identity link is recorded.".into(),
                source_url: None,
            });
        }
        let discogs_id = if artist.provider == "discogs" {
            Some(artist.id.clone())
        } else {
            self.store
                .get_artist_mapping("musicbrainz", &artist.id)?
                .and_then(|mapping| mapping.strip_prefix("discogs:").map(str::to_owned))
        };
        if let Some(discogs_id) = discogs_id {
            let url = format!("https://www.discogs.com/artist/{discogs_id}");
            if let Some(token) = credentials
                .discogs_token
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                match self
                    .discogs_json(
                        &format!("artists/{discogs_id}"),
                        &[],
                        "artists.profile.discogs",
                        &discogs_id,
                        PROFILE_TTL,
                        token,
                    )
                    .await
                {
                    Ok(data) => {
                        items.extend(parse_discogs_artist_relations(&data, artist, &url));
                        source_coverage.push(ArtistSourceCoverage {
                            provider: "discogs".into(),
                            status: "available".into(),
                            detail: "Artist member and group relationships loaded.".into(),
                            source_url: Some(url),
                        });
                    }
                    Err(error) => {
                        warnings.push(format!("Discogs relationships unavailable: {error}"));
                        source_coverage.push(ArtistSourceCoverage {
                            provider: "discogs".into(),
                            status: "unavailable".into(),
                            detail: "Relationship request failed.".into(),
                            source_url: Some(url),
                        });
                    }
                }
            } else {
                warnings
                    .push("Connect Discogs to view linked member and group relationships.".into());
                source_coverage.push(ArtistSourceCoverage {
                    provider: "discogs".into(),
                    status: "not configured".into(),
                    detail: "A Discogs identity is linked, but no credential is available.".into(),
                    source_url: Some(url),
                });
            }
        } else {
            source_coverage.push(ArtistSourceCoverage {
                provider: "discogs".into(),
                status: "not linked".into(),
                detail: "No explicit Discogs identity link is recorded.".into(),
                source_url: None,
            });
        }
        let total = items.len();
        let page_items = items
            .into_iter()
            .skip(offset)
            .take(ARTIST_LIST_PAGE_SIZE as usize)
            .collect::<Vec<_>>();
        if total == 0
            && source_coverage
                .iter()
                .any(|coverage| coverage.status == "available")
        {
            warnings.push(
                "No artist-to-artist relationships are recorded by the available sources.".into(),
            );
        }
        Ok(ArtistCollaborationPage {
            artist: artist.clone(),
            next_cursor: (offset + page_items.len() < total)
                .then(|| (offset + page_items.len()).to_string()),
            items: page_items,
            source_coverage,
            warnings,
        })
    }

    pub async fn resolve_recording_metadata(
        &self,
        track: &Track,
        release: Option<&ArtistReleaseRef>,
        recording: Option<&ArtistRef>,
        credentials: &ArtistCredentials,
    ) -> Result<crate::metadata::MetadataReview, String> {
        crate::metadata::resolve_recording_metadata(self, track, release, recording, credentials)
            .await
    }

    pub(crate) async fn recording_search(&self, track: &Track) -> Result<Value, String> {
        let query = format!(
            "recording:\"{}\" AND artist:\"{}\"",
            lucene_escape(&track.title),
            lucene_escape(&track.artist)
        );
        self.musicbrainz_json(
            "recording",
            &[
                ("query", query),
                ("limit", "10".into()),
                ("inc", "artist-credits+isrcs+genres".into()),
            ],
            "metadata.recording-search.musicbrainz",
            &format!(
                "{}\u{1f}{}",
                track.artist.to_lowercase(),
                track.title.to_lowercase()
            ),
            SEARCH_TTL,
        )
        .await
    }

    /// Adds review-only MusicBrainz suggestions to plainly labeled source chapters.
    /// A source chapter remains unverified unless one exact artist/title result is unique.
    pub async fn enrich_candidate_chapters(
        &self,
        candidate: &Candidate,
        _credentials: &ArtistCredentials,
    ) -> Candidate {
        let mut result = candidate.clone();
        let Some(chapters) = result.chapters.as_mut() else {
            return result;
        };
        for chapter in chapters.iter_mut().take(20) {
            let Some((artist, title)) = chapter.title.split_once(" - ") else {
                chapter.suggestion_confidence = Some("unverified_label".into());
                continue;
            };
            let (artist, title) = (artist.trim(), title.trim());
            if artist.is_empty() || title.is_empty() {
                chapter.suggestion_confidence = Some("unverified_label".into());
                continue;
            }
            let query_track = Track {
                artist: artist.to_owned(),
                title: title.to_owned(),
                duration_seconds: Some(chapter.end_seconds - chapter.start_seconds),
                ..Track::default()
            };
            let Ok(data) = self.recording_search(&query_track).await else {
                chapter.suggestion_confidence = Some("catalog_lookup_failed".into());
                continue;
            };
            let (suggestion, confidence) = chapter_suggestion(
                chapter,
                data["recordings"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            chapter.suggested_track = suggestion;
            chapter.suggestion_confidence = Some(confidence);
        }
        result
    }

    /// Resolve a verified Spotify album occurrence to one exact MusicBrainz release
    /// occurrence. The caller must supply the complete album snapshot produced by
    /// RemotePlaylistService after account, paging, album-echo, and position checks.
    /// This reads catalog sidecar data only; it does not rewrite the Spotify Track.
    pub(crate) async fn verify_spotify_album_recording(
        &self,
        spotify_album: &Value,
        target_track_id: &str,
        target_album_position: u32,
    ) -> Result<ReleaseOccurrence, String> {
        validate_spotify_target(spotify_album, target_track_id, target_album_position)
            .map_err(|_| "Spotify album occurrence is incomplete or inconsistent".to_owned())?;
        let album_id = spotify_album["albumId"]
            .as_str()
            .ok_or_else(|| "Spotify album identity is missing".to_owned())?;
        let canonical_url = format!("https://open.spotify.com/album/{album_id}");
        let relation_data = self
            .musicbrainz_json(
                "url",
                &[
                    ("resource", canonical_url.clone()),
                    ("inc", "release-rels".into()),
                ],
                "artists.spotify-album-crosswalk.musicbrainz-url",
                &canonical_url,
                LIST_TTL,
            )
            .await?;
        let normalized_relations = normalize_url_relations(&relation_data)
            .map_err(|_| "MusicBrainz URL relation response is incomplete".to_owned())?;
        let release_id = exact_release_id(&normalized_relations, album_id).map_err(|_| {
            "Spotify album URL does not identify exactly one MusicBrainz release".to_owned()
        })?;
        let release_data = self
            .musicbrainz_json(
                &format!("release/{release_id}"),
                &[("inc", "recordings+artist-credits".into())],
                "artists.spotify-album-crosswalk.musicbrainz-release",
                &release_id,
                LIST_TTL,
            )
            .await?;
        let normalized_release = normalize_musicbrainz_release(&release_data)
            .map_err(|_| "MusicBrainz release occurrence list is incomplete".to_owned())?;
        let occurrence = resolve_occurrence(
            spotify_album,
            target_track_id,
            target_album_position,
            &normalized_relations,
            &normalized_release,
        )
        .map_err(|_| {
            "Spotify and MusicBrainz release occurrences do not safely align".to_owned()
        })?;
        let occurrence_count = spotify_track_occurrence_count(spotify_album, target_track_id);
        self.store.set_provider_cache(
            "artists.release-tracks.musicbrainz",
            &release_id,
            &release_data,
            LIST_TTL,
        )?;
        write_verified_spotify_recording_sidecar(&self.store, &occurrence, occurrence_count)?;
        Ok(occurrence)
    }

    /// Optional alias warming: await inside the existing outer deadline/cancel future.
    /// Dropping this future cancels pending HTTP; playback reads cached profiles only.
    pub(crate) async fn prepare_verified_spotify_artist_profiles(
        &self,
        occurrence: &ReleaseOccurrence,
    ) {
        let mut seen = std::collections::HashSet::new();
        for credit in &occurrence.musicbrainz_artist_credits {
            if !valid_crosswalk_mbid(&credit.artist_id) || !seen.insert(&credit.artist_id) {
                continue;
            }
            let _ = self
                .musicbrainz_json(
                    &format!("artist/{}", credit.artist_id),
                    &[("inc", "aliases+genres+tags+artist-rels+url-rels".into())],
                    "artists.profile.musicbrainz",
                    &credit.artist_id,
                    PROFILE_TTL,
                )
                .await;
        }
    }

    /// Warm an explicit, exact Spotify-track to MusicBrainz-recording artist-ID
    /// crosswalk. The Spotify row is already stored and provider-normalized; the
    /// supplied recording MBID is only a locator. Playback consumes the cache
    /// written here and never performs these requests.
    pub(crate) async fn prepare_spotify_recording_credit_crosswalk(
        &self,
        track: &Track,
        recording_id: &str,
    ) -> Result<crate::artist_credit_aliases::SpotifyArtistCreditCrosswalkStatus, String> {
        if !valid_crosswalk_mbid(recording_id) {
            return Err("A MusicBrainz recording hint must be a valid recording ID".into());
        }
        let fingerprint = crate::artist_credit_aliases::spotify_artist_credit_fingerprint(track)
            .ok_or("The exact saved Spotify track does not have complete verified artist identities and duration")?;
        let recording = self
            .musicbrainz_json(
                &format!("recording/{recording_id}"),
                &[("inc", "artist-credits+isrcs".into())],
                crate::artist_credit_aliases::spotify_recording_credit_raw_cache_namespace(),
                recording_id,
                LIST_TTL,
            )
            .await?;
        if recording.get("id").and_then(Value::as_str) != Some(recording_id) {
            return Err("MusicBrainz returned a different recording than requested".into());
        }
        let credits = recording
            .get("artist-credit")
            .and_then(Value::as_array)
            .filter(|credits| !credits.is_empty() && credits.len() <= 16)
            .ok_or("MusicBrainz returned an incomplete recording artist credit")?;
        let mut seen = std::collections::HashSet::new();
        for credit in credits {
            let mbid = credit
                .get("artist")
                .and_then(|artist| artist.get("id"))
                .and_then(Value::as_str)
                .filter(|id| valid_crosswalk_mbid(id))
                .ok_or("MusicBrainz returned an artist credit without a valid artist ID")?;
            if !seen.insert(mbid.to_owned()) {
                continue;
            }
            self.musicbrainz_json(
                &format!("artist/{mbid}"),
                &[("inc", "aliases+genres+tags+artist-rels+url-rels".into())],
                crate::artist_credit_aliases::spotify_recording_credit_profile_cache_namespace(),
                mbid,
                PROFILE_TTL,
            )
            .await?;
        }
        let latest_track = self
            .store
            .get_library_track(&track.id)?
            .ok_or("The Spotify track was removed from the library during verification")?;
        if crate::artist_credit_aliases::spotify_artist_credit_fingerprint(&latest_track).as_ref()
            != Some(&fingerprint)
        {
            return Err("The saved Spotify track changed during recording verification".into());
        }
        // Verify only after every exact raw response is in the provider cache.
        // The helper rechecks all IDs, order, title, duration, ISRC and canonical
        // Spotify URLs before writing the playback sidecar.
        crate::artist_credit_aliases::write_verified_spotify_artist_credit_recording(
            &self.store,
            track,
            recording_id,
        )
    }

    /// Invalidate only after the caller has validated repeated complete album reads.
    /// A missing or repeated target must supersede an older unique index entry.
    pub(crate) fn invalidate_verified_spotify_recording(
        &self,
        album_id: &str,
        track_id: &str,
        occurrence_count: usize,
    ) -> Result<(), String> {
        invalidate_verified_spotify_recording_sidecar(
            &self.store,
            album_id,
            track_id,
            occurrence_count,
        )
    }

    /// Read only the versioned, source-anchored sidecar written by the exact album
    /// verifier. Album ID is required to prevent same-track-ID cross-album reuse.
    pub(crate) fn get_verified_spotify_recording(
        &self,
        spotify_album_id: &str,
        spotify_track_id: &str,
        target_album_position: u32,
    ) -> Result<Option<ReleaseOccurrence>, String> {
        if !valid_spotify_id(spotify_album_id)
            || !valid_spotify_id(spotify_track_id)
            || target_album_position == 0
        {
            return Ok(None);
        }
        let cache_key = verified_spotify_occurrence_key(
            spotify_album_id,
            spotify_track_id,
            target_album_position,
        );
        let Some(payload) = self
            .store
            .get_provider_cache("artists.spotify-album-crosswalk.verified-v2", &cache_key)?
        else {
            return Ok(None);
        };
        let occurrence: ReleaseOccurrence = serde_json::from_value(payload)
            .map_err(|_| "Verified Spotify recording sidecar is invalid".to_owned())?;
        if occurrence.spotify_album_id != spotify_album_id
            || occurrence.spotify_track_id != spotify_track_id
            || occurrence.spotify_position != target_album_position
        {
            return Ok(None);
        }
        Ok(Some(occurrence))
    }

    /// Resolve by provider album + track ID only when the complete source album
    /// proved this ID occurs exactly once. Ambiguous marker entries never return a mapping.
    pub(crate) fn get_verified_spotify_recording_by_album_track(
        &self,
        spotify_album_id: &str,
        spotify_track_id: &str,
    ) -> Result<Option<ReleaseOccurrence>, String> {
        read_verified_spotify_recording_sidecar(&self.store, spotify_album_id, spotify_track_id)
    }

    pub(crate) async fn json_for_metadata(
        &self,
        provider: &str,
        path: &str,
        namespace: &str,
        cache_key: &str,
        token: Option<&str>,
    ) -> Result<Value, String> {
        match provider {
            "musicbrainz" => {
                self.musicbrainz_json(path, &[], namespace, cache_key, PROFILE_TTL)
                    .await
            }
            "discogs" => {
                let token = token
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("Connect a Discogs token to resolve Discogs release metadata")?;
                self.discogs_json(path, &[], namespace, cache_key, PROFILE_TTL, token)
                    .await
            }
            "coverart" => {
                let url = Url::parse(&format!("{}/{path}", self.endpoints.cover_art))
                    .map_err(|error| error.to_string())?;
                self.get_json(namespace, cache_key, url, PROFILE_TTL, None)
                    .await
            }
            _ => Err("Unsupported metadata provider".into()),
        }
    }
}

fn invalidate_verified_spotify_recording_sidecar(
    store: &Store,
    album_id: &str,
    track_id: &str,
    occurrence_count: usize,
) -> Result<(), String> {
    if !valid_spotify_id(album_id) || !valid_spotify_id(track_id) || occurrence_count == 1 {
        return Err("Invalid Spotify crosswalk invalidation evidence".into());
    }
    store.set_provider_cache(
        "artists.spotify-album-crosswalk.verified-v2",
        &verified_spotify_album_track_key(album_id, track_id),
        &json!({"status":"ambiguous", "spotifyAlbumId":album_id,
            "spotifyTrackId":track_id, "occurrenceCount":occurrence_count}),
        LIST_TTL,
    )?;
    Ok(())
}

/// Persist exact-position provenance and an album+track index only when the
/// complete album proves that provider ID is unique. Duplicates get a tombstone
/// so an older unique entry cannot be reused after the album snapshot changes.
pub(crate) fn write_verified_spotify_recording_sidecar(
    store: &Store,
    occurrence: &ReleaseOccurrence,
    occurrence_count: usize,
) -> Result<(), String> {
    if occurrence_count == 0 {
        return Err("Verified Spotify occurrence is absent from its album".into());
    }
    let payload = serde_json::to_value(occurrence)
        .map_err(|_| "Verified Spotify recording sidecar could not be encoded".to_owned())?;
    let index_key = verified_spotify_album_track_key(
        &occurrence.spotify_album_id,
        &occurrence.spotify_track_id,
    );
    let album_track_payload = if occurrence_count == 1 {
        payload.clone()
    } else {
        serde_json::json!({
            "status":"ambiguous",
            "spotifyAlbumId":occurrence.spotify_album_id,
            "spotifyTrackId":occurrence.spotify_track_id,
            "occurrenceCount":occurrence_count
        })
    };
    store.set_provider_cache(
        "artists.spotify-album-crosswalk.verified-v2",
        &index_key,
        &album_track_payload,
        LIST_TTL,
    )?;
    let position_key = verified_spotify_occurrence_key(
        &occurrence.spotify_album_id,
        &occurrence.spotify_track_id,
        occurrence.spotify_position,
    );
    store.set_provider_cache(
        "artists.spotify-album-crosswalk.verified-v2",
        &position_key,
        &payload,
        LIST_TTL,
    )?;
    Ok(())
}

/// Read the unique verified sidecar without network access, suitable for the
/// metadata alias consumer. Malformed, ambiguous, stale, or identity-mismatched
/// cache payloads return no mapping.
pub(crate) fn read_verified_spotify_recording_sidecar(
    store: &Store,
    spotify_album_id: &str,
    spotify_track_id: &str,
) -> Result<Option<ReleaseOccurrence>, String> {
    if !valid_spotify_id(spotify_album_id) || !valid_spotify_id(spotify_track_id) {
        return Ok(None);
    }
    let key = verified_spotify_album_track_key(spotify_album_id, spotify_track_id);
    let Some(payload) =
        store.get_provider_cache("artists.spotify-album-crosswalk.verified-v2", &key)?
    else {
        return Ok(None);
    };
    if payload["status"].as_str() == Some("ambiguous") {
        return Ok(None);
    }
    let Ok(occurrence) = serde_json::from_value::<ReleaseOccurrence>(payload) else {
        return Ok(None);
    };
    if occurrence.spotify_album_id != spotify_album_id
        || occurrence.spotify_track_id != spotify_track_id
        || occurrence.spotify_position == 0
        || !valid_crosswalk_mbid(&occurrence.musicbrainz_release_id)
        || !valid_crosswalk_mbid(&occurrence.musicbrainz_recording_id)
        || !valid_crosswalk_mbid(&occurrence.musicbrainz_track_id)
        || occurrence.spotify_title.trim().is_empty()
        || occurrence.spotify_title.len() > 512
        || occurrence.spotify_artist.len() > 1024
        || occurrence
            .spotify_duration_ms
            .is_some_and(|duration| duration == 0 || duration > 86_400_000)
        || occurrence.musicbrainz_title.trim().is_empty()
        || occurrence.musicbrainz_title.len() > 512
        || occurrence.musicbrainz_artist.len() > 1024
        || occurrence
            .musicbrainz_duration_ms
            .is_some_and(|duration| duration == 0 || duration > 86_400_000)
        || occurrence.medium_position == 0
        || occurrence.medium_track_position == 0
    {
        return Ok(None);
    }
    if occurrence.musicbrainz_artist_credits.is_empty()
        || occurrence.musicbrainz_artist_credits.len() > 32
        || occurrence.musicbrainz_artist_credits.iter().any(|credit| {
            !valid_crosswalk_mbid(&credit.artist_id)
                || credit.name.trim().is_empty()
                || credit.name.len() > 512
                || credit.joinphrase.len() > 128
        })
    {
        return Ok(None);
    }
    let primary_key = verified_spotify_occurrence_key(
        spotify_album_id,
        spotify_track_id,
        occurrence.spotify_position,
    );
    let Some(primary) =
        store.get_provider_cache("artists.spotify-album-crosswalk.verified-v2", &primary_key)?
    else {
        return Ok(None);
    };
    let Ok(primary) = serde_json::from_value::<ReleaseOccurrence>(primary) else {
        return Ok(None);
    };
    if primary != occurrence {
        return Ok(None);
    }
    Ok(Some(occurrence))
}

fn verified_spotify_occurrence_key(album_id: &str, track_id: &str, position: u32) -> String {
    format!("{album_id}:{track_id}:{position}")
}
fn verified_spotify_album_track_key(album_id: &str, track_id: &str) -> String {
    format!("{album_id}:{track_id}")
}

fn parse_musicbrainz_artist(value: &Value) -> Option<ArtistSearchResult> {
    let id = value["id"].as_str()?.to_owned();
    let name = value["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())?
        .to_owned();
    Some(ArtistSearchResult {
        artist: ArtistRef {
            provider: "musicbrainz".into(),
            id: id.clone(),
        },
        name,
        image_url: None,
        sort_name: value["sort-name"].as_str().map(str::to_owned),
        disambiguation: nonempty(value["disambiguation"].as_str()),
        artist_type: value["type"].as_str().map(str::to_owned),
        country: value["country"]
            .as_str()
            .or(value["area"]["name"].as_str())
            .map(str::to_owned),
        begin_date: value["life-span"]["begin"].as_str().map(str::to_owned),
        end_date: value["life-span"]["end"].as_str().map(str::to_owned),
        score: value["score"]
            .as_f64()
            .or_else(|| value["score"].as_str().and_then(|s| s.parse().ok())),
        source_url: format!("https://musicbrainz.org/artist/{id}"),
    })
}

fn valid_mbid(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            })
}

fn parse_discogs_artist(value: &Value) -> Option<ArtistSearchResult> {
    let id = value["id"].as_u64()?.to_string();
    let name = value["title"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            value["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
        })?
        .to_owned();
    Some(ArtistSearchResult {
        artist: ArtistRef {
            provider: "discogs".into(),
            id: id.clone(),
        },
        name,
        image_url: None,
        sort_name: None,
        disambiguation: nonempty(value["profile"].as_str()).map(|s| s.chars().take(140).collect()),
        artist_type: None,
        country: None,
        begin_date: None,
        end_date: None,
        score: value["community"]["rating"]["average"].as_f64(),
        source_url: format!("https://www.discogs.com/artist/{id}"),
    })
}

fn trusted_entity_path(
    source_url: &str,
    host: &str,
    allowed_entities: &[&str],
    id: &str,
) -> Option<String> {
    let url = Url::parse(source_url).ok()?;
    if url.scheme() != "https"
        || url.host_str()? != host
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.len() != 2 || !allowed_entities.contains(&segments[0]) || segments[1] != id {
        return None;
    }
    Some(segments[0].to_owned())
}

fn trusted_musicbrainz_release_entity(source_url: &str, id: &str) -> Option<String> {
    trusted_entity_path(
        source_url,
        "musicbrainz.org",
        &["release", "release-group"],
        id,
    )
}

fn trusted_discogs_release_entity(source_url: &str, id: &str) -> Option<String> {
    trusted_entity_path(source_url, "www.discogs.com", &["release", "master"], id)
}

fn choose_representative_release(releases: &Value) -> Option<String> {
    let mut candidates = releases
        .as_array()?
        .iter()
        .filter_map(|release| {
            let id = release["id"].as_str()?.to_owned();
            valid_mbid(&id).then(|| {
                let official = release["status"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("official"));
                let date = release["date"].as_str().unwrap_or("9999").to_owned();
                (official, date, id)
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    candidates.into_iter().next().map(|(_, _, id)| id)
}

fn selected_artist_name_for_release(
    release: &ArtistReleaseRef,
    artist: &ArtistRef,
) -> Option<String> {
    if release
        .credits
        .iter()
        .any(|credit| is_various_artist(&credit.name))
    {
        return None;
    }
    release
        .credits
        .iter()
        .find(|credit| credit.artist.as_ref() == Some(artist))
        .map(|credit| credit.name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            (artist.provider == "discogs")
                .then(|| release.credits.first().map(|credit| credit.name.clone()))
                .flatten()
        })
}

fn is_various_artist(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    normalized == "various" || normalized == "various artists"
}

fn catalog_track_id(
    provider: &str,
    edition_id: &str,
    source_track_id: Option<&str>,
    medium: &str,
    position: &str,
    occurrence: u32,
) -> (String, String) {
    let stable_id = if let Some(source_track_id) = source_track_id.filter(|id| !id.is_empty()) {
        format!(
            "catalog:{provider}:{edition_id}:{medium}:{position}:{occurrence}:{source_track_id}"
        )
    } else {
        let material = format!("{provider}\0{edition_id}\0{medium}\0{position}\0{occurrence}");
        let digest = Sha256::digest(material.as_bytes());
        let hash = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("catalog:{provider}:{hash}")
    };
    // Storage requires every track ID to be exactly provider:provider_id.
    let provider_id = stable_id
        .strip_prefix("catalog:")
        .unwrap_or(&stable_id)
        .to_owned();
    (stable_id, provider_id)
}

fn mb_credit_name(credits: &Value) -> String {
    credits
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    item["artist"]["name"]
                        .as_str()
                        .filter(|name| !name.trim().is_empty())
                })?;
            Some(format!(
                "{}{}",
                name,
                item["joinphrase"].as_str().unwrap_or("")
            ))
        })
        .collect::<String>()
}

fn discogs_credit_values(
    values: &Value,
    source_url: &str,
    default_role: &str,
) -> Vec<ArtistCredit> {
    values
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())?
                .to_owned();
            let id = item["id"]
                .as_u64()
                .map(|id| id.to_string())
                .or_else(|| item["id"].as_str().map(str::to_owned));
            Some(ArtistCredit {
                name: name.clone(),
                artist: id
                    .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
                    .map(|id| ArtistRef {
                        provider: "discogs".into(),
                        id,
                    }),
                role: item["role"]
                    .as_str()
                    .filter(|role| !role.trim().is_empty())
                    .unwrap_or(default_role)
                    .to_owned(),
                credited_as: Some(name),
                join_phrase: item["join"].as_str().map(str::to_owned),
                source_url: source_url.to_owned(),
            })
        })
        .collect()
}

fn format_discogs_credits(credits: &[ArtistCredit]) -> String {
    credits
        .iter()
        .filter(|credit| {
            credit.role.eq_ignore_ascii_case("artist")
                || credit.role.eq_ignore_ascii_case("main artist")
                || credit.role.eq_ignore_ascii_case("main")
        })
        .map(|credit| {
            format!(
                "{}{}",
                credit.name,
                credit.join_phrase.as_deref().unwrap_or("")
            )
        })
        .collect::<String>()
}

fn discogs_duration(value: Option<&str>) -> Option<f64> {
    let parts = value?
        .split(':')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    if parts[1] >= 60 || (parts.len() == 3 && parts[2] >= 60) {
        return None;
    }
    Some(match parts.as_slice() {
        [minutes, seconds] => (*minutes * 60 + *seconds) as f64,
        [hours, minutes, seconds] => (*hours * 3600 + *minutes * 60 + *seconds) as f64,
        _ => return None,
    })
}

pub(crate) fn parse_musicbrainz_release_tracks(
    data: &Value,
    edition_id: &str,
    requested_source_url: &str,
    representative: bool,
    selected_artist_name: Option<&str>,
) -> Vec<Track> {
    let release_url = format!("https://musicbrainz.org/release/{edition_id}");
    let release_artist_credits = &data["artist-credit"];
    let release_artist = mb_credit_name(release_artist_credits);
    let release_date = data["date"].as_str().map(str::to_owned);
    let label_info = data["label-info"]
        .as_array()
        .and_then(|values| values.first());
    let label = label_info
        .and_then(|value| value["label"]["name"].as_str())
        .map(str::to_owned);
    let catalog_number = label_info
        .and_then(|value| value["catalog-number"].as_str())
        .map(str::to_owned);
    let album = data["title"].as_str().unwrap_or_default().to_owned();
    let artwork_url = musicbrainz_artwork_urls(data, "release", edition_id)
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("https://coverartarchive.org/release/{edition_id}/front-250"));
    let mut ordered = Vec::new();
    for (medium_index, medium) in data["media"].as_array().into_iter().flatten().enumerate() {
        let medium_position = medium["position"]
            .as_u64()
            .and_then(|position| u32::try_from(position).ok())
            .unwrap_or((medium_index + 1) as u32);
        for (track_index, track) in medium["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let recording = &track["recording"];
            let title = track["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .or_else(|| {
                    recording["title"]
                        .as_str()
                        .filter(|title| !title.trim().is_empty())
                });
            let Some(title) = title.filter(|title| !title.trim().is_empty()) else {
                continue;
            };
            let position = track["number"]
                .as_str()
                .filter(|position| !position.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    track["position"]
                        .as_u64()
                        .map(|position| position.to_string())
                })
                .unwrap_or_else(|| (track_index + 1).to_string());
            let numeric_position = track["position"]
                .as_u64()
                .unwrap_or((track_index + 1) as u64);
            let source_track_number = track["position"]
                .as_u64()
                .or_else(|| {
                    track["number"]
                        .as_str()
                        .and_then(|number| number.parse::<u64>().ok())
                })
                .and_then(|number| u32::try_from(number).ok())
                .filter(|number| *number > 0);
            let source_disc_number = medium["position"]
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .filter(|number| *number > 0);
            let recording_credits =
                parse_musicbrainz_credits(&recording["artist-credit"], &release_url);
            let track_credits = parse_musicbrainz_credits(&track["artist-credit"], &release_url);
            let credits = if !recording_credits.is_empty() {
                recording_credits.clone()
            } else if !track_credits.is_empty() {
                track_credits.clone()
            } else {
                parse_musicbrainz_credits(release_artist_credits, &release_url)
            };
            let track_artist_credits = if !recording_credits.is_empty() {
                &recording["artist-credit"]
            } else if !track_credits.is_empty() {
                &track["artist-credit"]
            } else {
                release_artist_credits
            };
            let track_artist_name = mb_credit_name(track_artist_credits);
            let album_artist_is_various = is_various_artist(&release_artist);
            let artist_name = if !track_artist_name.trim().is_empty() {
                track_artist_name
            } else if !release_artist.trim().is_empty() && !album_artist_is_various {
                release_artist.clone()
            } else if album_artist_is_various {
                release_artist.clone()
            } else {
                selected_artist_name.unwrap_or("Unknown artist").to_owned()
            };
            let recording_id = recording["id"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .map(str::to_owned);
            let track_id = track["id"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .map(str::to_owned);
            let reference_id = recording_id.clone().or(track_id);
            let source_track_guid = track["id"].as_str().map(str::to_owned);
            let duration_ms = recording["length"]
                .as_u64()
                .or_else(|| track["length"].as_u64());
            let isrc = recording["isrcs"].as_array().and_then(|values| {
                let codes = values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(crate::recording_identity::normalize_isrc)
                    .collect::<std::collections::BTreeSet<_>>();
                (codes.len() == 1).then(|| codes.into_iter().next().unwrap())
            });
            ordered.push((
                medium_position,
                numeric_position,
                medium_index * 100_000 + track_index,
                title.to_owned(),
                position,
                credits,
                artist_name,
                reference_id,
                source_track_guid,
                duration_ms,
                isrc,
                source_track_number,
                source_disc_number,
                track["position"].as_u64(),
                medium["position"].as_u64(),
            ));
        }
    }
    ordered.sort_by_key(|row| (row.0, row.1, row.2));
    let mut occurrences = std::collections::HashMap::<(u32, String), u32>::new();
    ordered.into_iter().map(|(medium, _, _, title, position, credits, artist, reference_id, source_track_guid, duration_ms, isrc, track_number, disc_number, source_track_position, source_medium_position)| {
        let key = (medium, position.clone());
        let occurrence = occurrences.entry(key).and_modify(|value| *value += 1).or_insert(1);
        let (id, provider_id) = catalog_track_id("musicbrainz", edition_id, reference_id.as_deref(), &medium.to_string(), &position, *occurrence);
        Track {
            id, provider: "catalog".into(), provider_id, source_url: release_url.clone(),
            artist, title, album: Some(album.clone()), label: label.clone(), catalog_number: catalog_number.clone(),
            duration_seconds: duration_ms.map(|duration| duration as f64 / 1000.0),
            track_number,
            disc_number,
            release_date: release_date.clone(), isrc,
            artwork_url: Some(artwork_url.clone()),
            provenance: json!({"sourceKind":"catalog", "catalogProvider":"musicbrainz", "sourceUrl":release_url,
                "requestedReleaseSourceUrl":requested_source_url, "releaseId":edition_id,
                "releaseGroupId":data["release-group"]["id"],
                "artworkAlternates":musicbrainz_artwork_urls(data, "release", edition_id),
                "mediumPosition":medium, "trackPosition":position, "occurrence":occurrence,
                "trackPositionOrdinal":source_track_position, "sourceMediumPosition":source_medium_position,
                "recordingId":reference_id, "trackGuid":source_track_guid, "artistCredits":credits, "representativeEdition":representative}),
            ..Track::default()
        }
    }).collect()
}

pub(crate) fn parse_discogs_release_tracks(
    data: &Value,
    edition_id: &str,
    requested_source_url: &str,
    representative: bool,
    selected_artist_name: Option<&str>,
) -> Vec<Track> {
    let release_url = format!("https://www.discogs.com/release/{edition_id}");
    let album = data["title"].as_str().unwrap_or_default().to_owned();
    let artwork_url = data["images"]
        .as_array()
        .and_then(|images| images.first())
        .and_then(|image| image["uri"].as_str().or(image["uri150"].as_str()))
        .and_then(secure_url);
    let release_credits = discogs_credit_values(&data["artists"], &release_url, "Artist");
    let release_artist = format_discogs_credits(&release_credits);
    let release_date = data["released"]
        .as_str()
        .filter(|date| !date.is_empty())
        .map(str::to_owned)
        .or_else(|| data["released_formatted"].as_str().map(str::to_owned))
        .or_else(|| data["year"].as_u64().map(|year| year.to_string()));
    let label_info = data["labels"].as_array().and_then(|values| values.first());
    let label = label_info
        .and_then(|value| value["name"].as_str())
        .map(str::to_owned);
    let catalog_number = label_info
        .and_then(|value| value["catno"].as_str())
        .map(str::to_owned);
    let mut medium = 1u32;
    let mut occurrences = std::collections::HashMap::<(u32, String), u32>::new();
    let mut tracks = Vec::new();
    let mut playable_rows = Vec::<(Value, Option<String>)>::new();
    for value in data["tracklist"].as_array().into_iter().flatten() {
        if value["type_"].as_str() == Some("heading") {
            playable_rows.push((value.clone(), None));
            continue;
        }
        let children = value["sub_tracks"]
            .as_array()
            .filter(|children| !children.is_empty());
        if let Some(children) = children {
            let parent_title = value["title"].as_str().map(str::to_owned);
            playable_rows.extend(
                children
                    .iter()
                    .cloned()
                    .map(|child| (child, parent_title.clone())),
            );
        } else {
            playable_rows.push((value.clone(), None));
        }
    }
    for (index, (value, parent_title)) in playable_rows.into_iter().enumerate() {
        if value["type_"].as_str() == Some("heading") {
            medium = medium.saturating_add(1);
            continue;
        }
        let Some(title) = value["title"]
            .as_str()
            .filter(|title| !title.trim().is_empty())
        else {
            continue;
        };
        let position = value["position"]
            .as_str()
            .filter(|position| !position.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| (index + 1).to_string());
        let key = (medium, position.clone());
        let occurrence = occurrences
            .entry(key)
            .and_modify(|value| *value += 1)
            .or_insert(1);
        let track_credits = discogs_credit_values(&value["artists"], &release_url, "Artist");
        let track_artist = format_discogs_credits(&track_credits);
        let use_release_artist = track_artist.trim().is_empty();
        let artist = if !track_artist.trim().is_empty() {
            track_artist
        } else if !release_artist.trim().is_empty() && !is_various_artist(&release_artist) {
            release_artist.clone()
        } else if is_various_artist(&release_artist) {
            release_artist.clone()
        } else {
            selected_artist_name.unwrap_or("Unknown artist").to_owned()
        };
        let mut credits = if track_credits.is_empty() || use_release_artist {
            release_credits.clone()
        } else {
            track_credits.clone()
        };
        if use_release_artist {
            // Keep release-level context alongside the track's own artist credit.
            // Contributors in `extraartists` remain separately role-labelled below.
            credits.extend(track_credits);
        }
        credits.extend(discogs_credit_values(
            &value["extraartists"],
            &release_url,
            "Contributor",
        ));
        let reference_id = value["id"]
            .as_u64()
            .map(|id| id.to_string())
            .or_else(|| value["id"].as_str().map(str::to_owned));
        let (id, provider_id) = catalog_track_id(
            "discogs",
            edition_id,
            reference_id.as_deref(),
            &medium.to_string(),
            &position,
            *occurrence,
        );
        tracks.push(Track {
            id, provider: "catalog".into(), provider_id, source_url: release_url.clone(),
            artist, title: title.to_owned(), album: Some(album.clone()), label: label.clone(), catalog_number: catalog_number.clone(),
            duration_seconds: discogs_duration(value["duration"].as_str()), release_date: release_date.clone(),
            artwork_url: artwork_url.clone(),
            provenance: json!({"sourceKind":"catalog", "catalogProvider":"discogs", "sourceUrl":release_url,
                "requestedReleaseSourceUrl":requested_source_url, "releaseId":edition_id,
                "releaseGroupId":data["release-group"]["id"],
                "mediumPosition":medium, "trackPosition":position, "occurrence":occurrence,
                "trackId":reference_id, "parentTitle":parent_title, "artistCredits":credits, "representativeEdition":representative}),
            ..Track::default()
        });
    }
    tracks
}

fn musicbrainz_artwork_urls(value: &Value, entity: &str, id: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if !valid_crosswalk_mbid(id) {
        return urls;
    }
    let group = if entity == "release-group" {
        Some(id)
    } else {
        value["release-group"]["id"]
            .as_str()
            .filter(|id| valid_crosswalk_mbid(id))
    };
    if entity == "release" && value["cover-art-archive"]["front"] != false {
        urls.push(format!(
            "https://coverartarchive.org/release/{id}/front-250"
        ));
    }
    if let Some(group) = group {
        urls.push(format!(
            "https://coverartarchive.org/release-group/{group}/front-250"
        ));
    }
    if urls.is_empty() && entity == "release" {
        urls.push(format!(
            "https://coverartarchive.org/release/{id}/front-250"
        ));
    }
    urls
}

fn parse_musicbrainz_release_group(value: &Value) -> Option<ArtistReleaseRef> {
    let id = value["id"].as_str()?.to_owned();
    let title = value["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())?
        .to_owned();
    let source_url = format!("https://musicbrainz.org/release-group/{id}");
    let release_date = value["first-release-date"].as_str().map(str::to_owned);
    let credits = parse_musicbrainz_credits(&value["artist-credit"], &source_url);
    Some(ArtistReleaseRef {
        artwork_urls: musicbrainz_artwork_urls(value, "release-group", &id),
        release: ArtistRef {
            provider: "musicbrainz".into(),
            id,
        },
        title,
        year: release_date.as_deref().and_then(year_from_date),
        release_date,
        country: None,
        label: None,
        catalog_number: None,
        format: None,
        primary_type: value["primary-type"].as_str().map(str::to_owned),
        track_count: None,
        tracks: Vec::new(),
        source_url,
        credits,
    })
}

fn parse_musicbrainz_release(value: &Value, artist: &ArtistRef) -> Option<ArtistReleaseRef> {
    let id = value["id"].as_str()?.to_owned();
    let title = value["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())?
        .to_owned();
    let release_date = value["date"].as_str().map(str::to_owned);
    let credits = parse_musicbrainz_credits(
        &value["artist-credit"],
        &format!("https://musicbrainz.org/release/{id}"),
    );
    let labels = value["label-info"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let label = labels
        .iter()
        .find_map(|item| item["label"]["name"].as_str())
        .map(str::to_owned);
    let catalog_number = labels
        .iter()
        .find_map(|item| item["catalog-number"].as_str())
        .map(str::to_owned);
    let media = value["media"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let track_count = media
        .iter()
        .filter_map(|item| item["track-count"].as_u64())
        .sum::<u64>()
        .try_into()
        .ok();
    let format = media
        .iter()
        .filter_map(|item| item["format"].as_str())
        .next()
        .map(str::to_owned);
    let tracks = media
        .iter()
        .flat_map(|medium| medium["tracks"].as_array().into_iter().flatten())
        .filter_map(|track| {
            Some(ArtistReleaseTrack {
                position: track["position"].as_str().map(str::to_owned),
                title: track["title"].as_str()?.to_owned(),
                source_url: format!("https://musicbrainz.org/release/{id}"),
            })
        })
        .collect();
    let _ = artist;
    Some(ArtistReleaseRef {
        artwork_urls: musicbrainz_artwork_urls(value, "release", &id),
        release: ArtistRef {
            provider: "musicbrainz".into(),
            id: id.clone(),
        },
        title,
        year: release_date.as_deref().and_then(year_from_date),
        release_date,
        country: value["country"].as_str().map(str::to_owned),
        label,
        catalog_number,
        format,
        primary_type: value["release-group"]["primary-type"]
            .as_str()
            .map(str::to_owned),
        track_count,
        tracks,
        source_url: format!("https://musicbrainz.org/release/{id}"),
        credits,
    })
}

fn parse_discogs_release(value: &Value) -> Option<ArtistReleaseRef> {
    let raw_id = value["id"].as_u64()?.to_string();
    let entity = value["type"].as_str().unwrap_or("release");
    let entity = if entity == "master" {
        "master"
    } else {
        "release"
    };
    let id = raw_id.clone();
    let title = value["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())?
        .to_owned();
    let role = value["role"].as_str().unwrap_or("artist credit").to_owned();
    let credited_as = value["artist"].as_str().map(str::to_owned);
    let format = value["format"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| string_array(&value["format"]).into_iter().next());
    let primary_type = format.as_deref().and_then(|format| {
        let words: Vec<_> = format.split(',').map(str::trim).collect();
        if words.iter().any(|word| word.eq_ignore_ascii_case("Album")) {
            Some("Album".into())
        } else if words.iter().any(|word| word.eq_ignore_ascii_case("EP")) {
            Some("EP".into())
        } else if words.iter().any(|word| word.eq_ignore_ascii_case("Single")) {
            Some("Single".into())
        } else {
            None
        }
    });
    Some(ArtistReleaseRef {
        artwork_urls: value["cover_image"]
            .as_str()
            .and_then(secure_url)
            .into_iter()
            .collect(),
        release: ArtistRef {
            provider: "discogs".into(),
            id: id.clone(),
        },
        title,
        year: value["year"].as_u64().map(|n| n as u32),
        release_date: None,
        country: None,
        label: value["label"].as_str().map(str::to_owned),
        catalog_number: value["catno"].as_str().map(str::to_owned),
        format,
        primary_type,
        track_count: None,
        tracks: Vec::new(),
        source_url: format!("https://www.discogs.com/{entity}/{raw_id}"),
        credits: vec![ArtistCredit {
            name: credited_as.clone().unwrap_or_default(),
            artist: None,
            role,
            credited_as,
            join_phrase: None,
            source_url: format!("https://www.discogs.com/{entity}/{raw_id}"),
        }],
    })
}

fn parse_musicbrainz_recording(value: &Value, _artist: &ArtistRef) -> Option<Track> {
    let id = value["id"].as_str()?.to_owned();
    let title = value["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())?
        .to_owned();
    let source_url = format!("https://musicbrainz.org/recording/{id}");
    let credits = parse_musicbrainz_credits(&value["artist-credit"], &source_url);
    let artist = credits
        .iter()
        .map(|credit| {
            format!(
                "{}{}",
                credit.name,
                credit.join_phrase.as_deref().unwrap_or("")
            )
        })
        .collect::<String>();
    let isrc = value["isrcs"].as_array().and_then(|values| {
        let codes = values
            .iter()
            .filter_map(Value::as_str)
            .filter_map(crate::recording_identity::normalize_isrc)
            .collect::<std::collections::BTreeSet<_>>();
        (codes.len() == 1).then(|| codes.into_iter().next().unwrap())
    });
    let release_date = value["first-release-date"].as_str().map(str::to_owned);
    let duration_seconds = value["length"]
        .as_f64()
        .map(|length_ms| length_ms / 1000.0)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    let tags = named_items(&value["genres"]);
    let tag_list = if tags.is_empty() {
        named_items(&value["tags"])
    } else {
        tags
    };
    Some(Track {
        id: format!("musicbrainz:{id}"),
        provider: "musicbrainz".into(),
        provider_id: id.clone(),
        source_url: source_url.clone(),
        artist,
        title,
        duration_seconds,
        release_date: release_date.clone(),
        genre: tag_list.first().cloned(),
        isrc,
        provenance: json!({
            "source": "MusicBrainz recording",
            "provider": "musicbrainz",
            "providerId": id,
            "sourceUrl": source_url,
            "artistCredits": credits,
            "recordingId": id,
            "recordingRelations": value["relations"],
            "recordingIsrcs": value["isrcs"],
            "recordingFirstReleaseDate": release_date,
            "dateMeaning": "Recording first release date; not the date this result was uploaded or searched",
            "mixVersion": "unknown; no version inferred from artist or release text"
        }),
        ..Track::default()
    })
}

fn chapter_suggestion(chapter: &Chapter, recordings: &[Value]) -> (Option<Box<Track>>, String) {
    let Some((artist, title)) = chapter.title.split_once(" - ") else {
        return (None, "unverified_label".into());
    };
    let (artist, title) = (artist.trim(), title.trim());
    if artist.is_empty() || title.is_empty() {
        return (None, "unverified_label".into());
    }
    let expected_duration = chapter.end_seconds - chapter.start_seconds;
    let matches = recordings
        .iter()
        .filter_map(|recording| {
            let mut track = parse_musicbrainz_recording(
                recording,
                &ArtistRef {
                    provider: "musicbrainz".into(),
                    id: String::new(),
                },
            )?;
            if matching::normalize(&track.artist) != matching::normalize(artist)
                || matching::normalize(&track.title) != matching::normalize(title)
            {
                return None;
            }
            let duration = recording["length"].as_f64().map(|ms| ms / 1000.0);
            if duration.is_some_and(|duration| {
                (duration - expected_duration).abs() > (expected_duration * 0.03).max(10.0)
            }) {
                return None;
            }
            track.duration_seconds = duration;
            // Recording ISRC identifies the full recording, never this excerpt.
            track.isrc = None;
            Some(track)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [track] => (
            Some(Box::new(track.clone())),
            "exact_unique_catalog_match".into(),
        ),
        [] => (None, "no_exact_catalog_match".into()),
        _ => (None, "ambiguous_catalog_matches".into()),
    }
}

fn parse_musicbrainz_credits(value: &Value, source_url: &str) -> Vec<ArtistCredit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let artist = item["artist"].as_object();
            let name = item["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    artist?
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.trim().is_empty())
                })?
                .to_owned();
            let artist_ref = artist
                .and_then(|artist| artist.get("id"))
                .and_then(Value::as_str)
                .filter(|id| valid_mbid(id))
                .map(|id| ArtistRef {
                    provider: "musicbrainz".into(),
                    id: id.to_owned(),
                });
            Some(ArtistCredit {
                name,
                artist: artist_ref,
                role: "artist credit".into(),
                credited_as: nonempty(item["name"].as_str()),
                join_phrase: item["joinphrase"].as_str().map(str::to_owned),
                source_url: source_url.to_owned(),
            })
        })
        .collect()
}

fn parse_lastfm_tracks(value: &Value, artist_name: &str, artist: &ArtistRef) -> Vec<Track> {
    value["toptracks"]["track"].as_array().into_iter().flatten().filter_map(|item| {
        let title = item["name"].as_str().filter(|title| !title.trim().is_empty())?.to_owned();
        let url = item["url"].as_str().and_then(secure_url).unwrap_or_else(|| format!("https://www.last.fm/music/{}/_/{}", url_component(artist_name), url_component(&title)));
        Some(Track {
            id: format!("lastfm:{}:{}", artist.id, title.to_lowercase()),
            provider: "lastfm".into(),
            provider_id: format!("{}:{}", artist.id, title.to_lowercase()),
            source_url: url.clone(),
            artist: artist_name.to_owned(),
            title,
            provenance: json!({"source":"Last.fm top tracks", "provider":"lastfm", "artistRef":artist, "sourceUrl":url}),
            ..Track::default()
        })
    }).collect()
}

fn artist_relationship(
    artist_name: &str,
    artist_provider: &str,
    artist_id: &str,
    related_name: &str,
    related_provider: &str,
    related_id: &str,
    role: &str,
    source_url: &str,
    related_url: &str,
    description: Option<String>,
) -> ArtistCollaboration {
    ArtistCollaboration {
        credits: vec![
            ArtistCredit {
                name: artist_name.into(),
                artist: Some(ArtistRef {
                    provider: artist_provider.to_lowercase(),
                    id: artist_id.into(),
                }),
                role: "artist".into(),
                credited_as: None,
                join_phrase: None,
                source_url: source_url.into(),
            },
            ArtistCredit {
                name: related_name.into(),
                artist: Some(ArtistRef {
                    provider: related_provider.into(),
                    id: related_id.into(),
                }),
                role: role.into(),
                credited_as: None,
                join_phrase: None,
                source_url: related_url.into(),
            },
        ],
        description,
        title: None,
        date: None,
        source_url: source_url.into(),
    }
}

fn parse_discogs_artist_relations(
    data: &Value,
    selected_artist: &ArtistRef,
    source_url: &str,
) -> Vec<ArtistCollaboration> {
    let name = data["name"]
        .as_str()
        .or(data["title"].as_str())
        .unwrap_or("Unknown artist");
    let mut output = Vec::new();
    for (field, role) in [("members", "Member"), ("groups", "Group member of")] {
        for related in data[field].as_array().into_iter().flatten() {
            let Some(id) = related["id"].as_u64().map(|id| id.to_string()) else {
                continue;
            };
            let Some(related_name) = related["name"].as_str() else {
                continue;
            };
            let related_url = format!("https://www.discogs.com/artist/{id}");
            output.push(artist_relationship(
                name,
                &selected_artist.provider,
                &selected_artist.id,
                related_name,
                "discogs",
                &id,
                role,
                source_url,
                &related_url,
                related["active"].as_bool().map(|active| {
                    if active {
                        "Active relationship".into()
                    } else {
                        "Former relationship".into()
                    }
                }),
            ));
        }
    }
    output
}

fn named_items(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["name"].as_str().or(item.as_str()).map(str::to_owned))
        .filter(|name| !name.trim().is_empty())
        .collect()
}

fn musicbrainz_release_group_search_query(query: &str) -> String {
    // Keep the full literal query for title-only and artist-only searches,
    // then add bounded artist/title splits for combined natural-language
    // queries such as "Kendrick Lamar good kid".
    let escaped_phrase = |phrase: &str| {
        phrase
            .chars()
            .flat_map(|character| {
                if matches!(
                    character,
                    '+' | '-'
                        | '&'
                        | '|'
                        | '!'
                        | '('
                        | ')'
                        | '{'
                        | '}'
                        | '['
                        | ']'
                        | '^'
                        | '"'
                        | '~'
                        | '*'
                        | '?'
                        | ':'
                        | '\\'
                        | '/'
                ) {
                    vec!['\\', character]
                } else {
                    vec![character]
                }
            })
            .collect::<String>()
    };
    let phrase = escaped_phrase(query);
    let mut clauses = vec![
        format!("releasegroup:\"{phrase}\""),
        format!("artist:\"{phrase}\""),
        format!("artistname:\"{phrase}\""),
    ];
    let words = query.split_whitespace().collect::<Vec<_>>();
    if (3..=8).contains(&words.len()) {
        for split in 1..words.len() {
            let left = escaped_phrase(&words[..split].join(" "));
            let right = escaped_phrase(&words[split..].join(" "));
            clauses.push(format!(
                "(artistname:\"{left}\" AND releasegroup:\"{right}\")"
            ));
            clauses.push(format!(
                "(releasegroup:\"{left}\" AND artistname:\"{right}\")"
            ));
        }
    }
    clauses.join(" OR ")
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn nonempty(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty()).map(str::to_owned)
}
fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if !result
            .iter()
            .any(|item: &String| item.eq_ignore_ascii_case(&value))
        {
            result.push(value);
        }
    }
    result
}
fn extend_unique(target: &mut Vec<String>, source: Vec<String>) {
    for value in source {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}
fn dedupe_source_tags(values: Vec<ArtistTag>) -> Vec<ArtistTag> {
    let mut result = Vec::new();
    for value in values {
        if !result.iter().any(|existing: &ArtistTag| {
            existing.provider == value.provider
                && existing.kind == value.kind
                && existing.name.eq_ignore_ascii_case(&value.name)
        }) {
            result.push(value);
        }
    }
    result
}
fn year_from_date(date: &str) -> Option<u32> {
    date.get(0..4)?.parse().ok()
}
fn parse_cursor(cursor: Option<&str>) -> Result<u32, String> {
    let value = cursor
        .unwrap_or("0")
        .parse::<u32>()
        .map_err(|_| "Invalid artist page cursor")?;
    if value > 100_000 {
        return Err("Artist page cursor is out of range".into());
    }
    Ok(value)
}
fn parse_release_cursor(cursor: Option<&str>) -> Result<(Option<u32>, Option<u32>), String> {
    let Some(value) = cursor else {
        return Ok((Some(0), Some(1)));
    };
    if let Some(rest) = value.strip_prefix("r3:") {
        let (mb, discogs) = rest.split_once(':').ok_or("Invalid artist page cursor")?;
        let mb = if mb == "x" {
            None
        } else {
            Some(
                mb.parse::<u32>()
                    .map_err(|_| "Invalid artist page cursor")?,
            )
        };
        let discogs = if discogs == "x" {
            None
        } else {
            Some(
                discogs
                    .parse::<u32>()
                    .map_err(|_| "Invalid artist page cursor")?,
            )
        };
        if mb.is_some_and(|offset| offset > 100_000)
            || discogs.is_some_and(|page| !(1..=100_001).contains(&page))
        {
            return Err("Artist page cursor is out of range".into());
        }
        return Ok((mb, discogs));
    }
    if let Some(rest) = value.strip_prefix("r2:") {
        let (mb, discogs) = rest.split_once(':').ok_or("Invalid artist page cursor")?;
        let mb = mb
            .parse::<u32>()
            .map_err(|_| "Invalid artist page cursor")?;
        let discogs = discogs
            .parse::<u32>()
            .map_err(|_| "Invalid artist page cursor")?;
        if mb > 100_000 || !(1..=100_001).contains(&discogs) {
            return Err("Artist page cursor is out of range".into());
        }
        return Ok((Some(mb), Some(discogs)));
    }
    let legacy = parse_cursor(Some(value))?;
    Ok((Some(legacy), Some(legacy / ARTIST_LIST_PAGE_SIZE + 1)))
}
fn validate_artist_ref(artist: &ArtistRef) -> Result<(), String> {
    match artist.provider.as_str() {
        "musicbrainz" if uuid_like(&artist.id) => Ok(()),
        "discogs"
            if !artist.id.is_empty()
                && artist.id.len() <= 20
                && artist.id.chars().all(|c| c.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err("Invalid artist provider identity".into()),
    }
}
fn uuid_like(id: &str) -> bool {
    id.len() == 36
        && id.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}
fn wikidata_item_id(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("www.wikidata.org" | "wikidata.org"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let id = url.path().strip_prefix("/wiki/")?;
    if !(2..=24).contains(&id.len())
        || !id.starts_with('Q')
        || !id[1..].bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(id.to_owned())
}
fn wikidata_portrait_urls(qid: &str, raw: &Value) -> Option<(String, String)> {
    let entity = &raw["entities"][qid];
    if entity["id"] != qid {
        return None;
    }
    let claims = entity["claims"]["P18"].as_array()?;
    let claim = claims
        .iter()
        .find(|c| c["rank"] == "preferred")
        .or_else(|| claims.iter().find(|c| c["rank"] == "normal"))?;
    if claim["mainsnak"]["snaktype"] != "value" || claim["mainsnak"]["datatype"] != "commonsMedia" {
        return None;
    }
    let name = claim["mainsnak"]["datavalue"]["value"].as_str()?;
    let lower = name.to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 512
        || name.chars().any(char::is_control)
        || ![".jpg", ".jpeg", ".png", ".webp"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    {
        return None;
    }
    let mut image = Url::parse("https://commons.wikimedia.org/wiki/Special:FilePath/").ok()?;
    image.path_segments_mut().ok()?.pop_if_empty().push(name);
    image.query_pairs_mut().append_pair("width", "300");
    let mut source = Url::parse("https://commons.wikimedia.org/wiki/").ok()?;
    source
        .path_segments_mut()
        .ok()?
        .pop_if_empty()
        .push(&format!("File:{name}"));
    Some((image.to_string(), source.to_string()))
}

fn secure_url(raw: &str) -> Option<String> {
    let mut url = Url::parse(raw).ok()?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.to_string())
}
fn link_provider(url: &str) -> Option<String> {
    let host = Url::parse(url).ok()?.host_str()?.to_owned();
    if host.ends_with("discogs.com") {
        Some("discogs".into())
    } else if host.ends_with("musicbrainz.org") {
        Some("musicbrainz".into())
    } else if host.ends_with("last.fm") {
        Some("lastfm".into())
    } else {
        Some(host)
    }
}
fn discogs_artist_id(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if host != "discogs.com" && !host.ends_with(".discogs.com") {
        return None;
    }
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let index = segments.iter().position(|part| *part == "artist")?;
    let raw_id = *segments.get(index + 1)?;
    let id = raw_id.split('-').next()?;
    id.parse::<u64>().ok().map(|_| id.to_owned())
}
fn musicbrainz_artist_id(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if host != "musicbrainz.org" && !host.ends_with(".musicbrainz.org") {
        return None;
    }
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let index = segments.iter().position(|part| *part == "artist")?;
    let id = *segments.get(index + 1)?;
    uuid_like(id).then(|| id.to_owned())
}
fn musicbrainz_id_for(artist: &ArtistRef, store: &Store) -> Result<Option<String>, String> {
    if artist.provider == "musicbrainz" {
        return Ok(Some(artist.id.clone()));
    }
    if artist.provider != "discogs" {
        return Ok(None);
    }
    let mapping = store.get_artist_mapping("discogs", &artist.id)?;
    Ok(mapping.and_then(|value| value.strip_prefix("musicbrainz:").map(str::to_owned)))
}
fn strip_html(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    for c in value.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(c),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn url_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
fn lucene_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if matches!(
                ch,
                '+' | '-'
                    | '&'
                    | '|'
                    | '!'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '^'
                    | '"'
                    | '~'
                    | '*'
                    | '?'
                    | ':'
                    | '\\'
                    | '/'
            ) {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect()
}

async fn musicbrainz_rate_limit() {
    use std::sync::OnceLock;
    use tokio::sync::Mutex;
    static LAST: OnceLock<Mutex<std::time::Instant>> = OnceLock::new();
    let lock = LAST.get_or_init(|| Mutex::new(std::time::Instant::now() - Duration::from_secs(2)));
    let mut last = lock.lock().await;
    let elapsed = last.elapsed();
    if elapsed < Duration::from_secs(1) {
        tokio::time::sleep(Duration::from_secs(1) - elapsed).await;
    }
    *last = std::time::Instant::now();
}

async fn discogs_rate_limit() {
    use std::sync::OnceLock;
    use tokio::sync::Mutex;
    static LAST: OnceLock<Mutex<std::time::Instant>> = OnceLock::new();
    let lock = LAST.get_or_init(|| Mutex::new(std::time::Instant::now() - Duration::from_secs(2)));
    let mut last = lock.lock().await;
    let elapsed = last.elapsed();
    if elapsed < Duration::from_secs(1) {
        tokio::time::sleep(Duration::from_secs(1) - elapsed).await;
    }
    *last = std::time::Instant::now();
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn oversized_provider_response_remains_usable_without_cache() {
        use axum::{routing::get, Json, Router};
        let app = Router::new().route("/large", get(|| async {
            Json(json!({"releases":[{"title":"Still available"}], "padding":"x".repeat(1024*1024)}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("cache.sqlite")).unwrap());
        let service = ArtistService::new(store.clone()).unwrap();
        let result = service
            .get_json(
                "test.large",
                "page",
                Url::parse(&format!("http://{address}/large")).unwrap(),
                60,
                None,
            )
            .await
            .unwrap();
        assert_eq!(result["releases"][0]["title"], "Still available");
        assert!(store
            .get_provider_cache("test.large", "page")
            .unwrap()
            .is_none());
        server.abort();
    }

    #[test]
    fn edition_without_cover_uses_its_exact_release_group() {
        let edition = "4de779b6-75f4-4041-9cde-70a60ea21a89";
        let group = "2c50bbe3-b000-4b28-a80f-e55b022757b9";
        let raw = json!({"cover-art-archive":{"front":false},"release-group":{"id":group}});
        assert_eq!(
            musicbrainz_artwork_urls(&raw, "release", edition),
            vec![format!(
                "https://coverartarchive.org/release-group/{group}/front-250"
            )]
        );
        let raw = json!({"cover-art-archive":{"front":true},"release-group":{"id":group}});
        assert_eq!(musicbrainz_artwork_urls(&raw, "release", edition).len(), 2);
        assert!(musicbrainz_artwork_urls(&raw, "release", edition)[0].contains(edition));
    }

    #[test]
    fn portraits_require_exact_wikidata_link_and_supported_non_deprecated_media() {
        assert_eq!(
            wikidata_item_id("https://www.wikidata.org/wiki/Q123"),
            Some("Q123".into())
        );
        for url in [
            "https://wikidata.org.evil/wiki/Q123",
            "https://www.wikidata.org/wiki/Q123?x=1",
            "https://www.wikidata.org/wiki/artist",
            "https://user@www.wikidata.org/wiki/Q123",
        ] {
            assert!(wikidata_item_id(url).is_none());
        }
        let mut raw = json!({"entities":{"Q123":{"id":"Q123","claims":{"P18":[{"rank":"normal","mainsnak":{"snaktype":"value","datatype":"commonsMedia","datavalue":{"value":"Artist portrait.jpg"}}}]}}}});
        let (image, source) = wikidata_portrait_urls("Q123", &raw).unwrap();
        assert!(image.starts_with("https://commons.wikimedia.org/wiki/Special:FilePath/"));
        assert!(image.ends_with("?width=300"));
        assert!(source.contains("Artist%20portrait.jpg"));
        assert!(wikidata_portrait_urls("Q999", &raw).is_none());
        raw["entities"]["Q123"]["claims"]["P18"][0]["rank"] = json!("deprecated");
        assert!(wikidata_portrait_urls("Q123", &raw).is_none());
    }
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn assert_tracks_round_trip(tracks: &[Track]) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("catalog-track-roundtrip.sqlite")).unwrap();
        for track in tracks {
            assert_eq!(
                track.id,
                format!("{}:{}", track.provider, track.provider_id)
            );
            let saved = store.save_track(track).unwrap();
            assert_eq!(saved.id, track.id);
            let loaded = store.get_track(&track.id).unwrap().unwrap();
            assert_eq!(loaded.provider_id, track.provider_id);
            assert_eq!(loaded.title, track.title);
        }
    }

    #[test]
    fn search_parsers_keep_provider_ids_and_disambiguation_separate() {
        let mb = parse_musicbrainz_artist(&json!({
            "id":"00000000-0000-4000-8000-000000000001", "name":"Pulse",
            "disambiguation":"Swedish electronic group", "score":93,
            "life-span":{"begin":"1994"}, "country":"SE"
        }))
        .unwrap();
        let discogs = parse_discogs_artist(&json!({"id":123,"title":"Pulse"})).unwrap();
        assert_ne!(mb.artist, discogs.artist);
        assert_eq!(
            mb.disambiguation.as_deref(),
            Some("Swedish electronic group")
        );
        assert_eq!(mb.begin_date.as_deref(), Some("1994"));
        assert_eq!(discogs.artist.provider, "discogs");
    }

    #[test]
    fn musicbrainz_album_query_searches_titles_and_artists_as_literal_text() {
        assert_eq!(
            musicbrainz_release_group_search_query("Nipsey Hussle"),
            "releasegroup:\"Nipsey Hussle\" OR artist:\"Nipsey Hussle\" OR artistname:\"Nipsey Hussle\""
        );
        let combined = musicbrainz_release_group_search_query("Kendrick Lamar good kid");
        assert!(combined.contains("(artistname:\"Kendrick Lamar\" AND releasegroup:\"good kid\")"));
        assert!(combined.contains("(releasegroup:\"Kendrick Lamar\" AND artistname:\"good kid\")"));
        let injected = musicbrainz_release_group_search_query("Nipsey\" OR artist:*");
        assert!(injected.contains("Nipsey\\\" OR artist\\:\\*"));
        assert!(!injected.contains("OR artist:*\""));
    }

    #[tokio::test]
    async fn musicbrainz_artist_search_reuses_cached_portrait_without_extra_fetches() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 2048];
            loop {
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"count":1,"artists":[{"id":"00000000-0000-4000-8000-000000000111","name":"Fixture Artist"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("cached-artist-image.sqlite")).unwrap());
        store
            .set_provider_cache(
                "artists.portrait.artist.v1",
                "00000000-0000-4000-8000-000000000111",
                &json!({"imageUrl":"https://commons.wikimedia.org/fixture.jpg"}),
                3600,
            )
            .unwrap();
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.musicbrainz = format!("http://{address}/ws/2");
        let page = service
            .search_artists_for(
                "musicbrainz",
                "Fixture Artist",
                1,
                25,
                &ArtistCredentials::default(),
            )
            .await
            .unwrap();
        assert_eq!(page.results.len(), 1);
        assert_eq!(
            page.results[0].image_url.as_deref(),
            Some("https://commons.wikimedia.org/fixture.jpg")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn artist_artwork_only_uses_exact_linked_cached_portrait_without_discography() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("artwork-only.sqlite")).unwrap());
        let id = "00000000-0000-4000-8000-000000000111";
        store.set_provider_cache("artists.profile.musicbrainz",id,&json!({"id":id,"relations":[{"url":{"resource":"https://www.discogs.com/artist/1234"}}]}),3600).unwrap();
        store
            .set_provider_cache(
                "artists.profile.discogs",
                "1234",
                &json!({"images":[{"uri":"https://images.test/exact-artist.jpg"}]}),
                3600,
            )
            .unwrap();
        let mut service = ArtistService::new(store.clone()).unwrap();
        service.endpoints.musicbrainz = "http://127.0.0.1:1/ws/2".into();
        service.endpoints.discogs = "http://127.0.0.1:1".into();
        let artist = ArtistRef {
            provider: "musicbrainz".into(),
            id: id.into(),
        };
        let image = tokio::time::timeout(
            Duration::from_secs(1),
            service.get_artist_artwork(&artist, &ArtistCredentials::default()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            image.as_deref(),
            Some("https://images.test/exact-artist.jpg")
        );
        assert_eq!(service.cached_musicbrainz_artist_image(id), image);
        assert!(store
            .get_provider_cache("artists.release-groups.musicbrainz", id)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn musicbrainz_album_search_includes_artist_matches() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 2048];
            loop {
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request_line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let body = r#"{"count":1,"release-groups":[{"id":"00000000-0000-4000-8000-000000000011","title":"good kid, m.A.A.d city","artist-credit":[{"name":"Kendrick Lamar","artist":{"id":"00000000-0000-4000-8000-000000000012","name":"Kendrick Lamar"}}]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request_line
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("album-artist-search.sqlite")).unwrap());
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.musicbrainz = format!("http://{address}/ws/2");
        let page = service
            .search_albums_for(
                "musicbrainz",
                "Kendrick Lamar good kid",
                1,
                &ArtistCredentials::default(),
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "good kid, m.A.A.d city");
        assert_eq!(page.items[0].credits[0].name, "Kendrick Lamar");
        let request_line = server.await.unwrap();
        let request_path = request_line.split_whitespace().nth(1).unwrap();
        let request_url = Url::parse(&format!("http://localhost{request_path}")).unwrap();
        let params = request_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            params.get("query").map(|value| value.as_ref()),
            Some(musicbrainz_release_group_search_query("Kendrick Lamar good kid").as_str())
        );
        assert_eq!(params.get("limit").map(|value| value.as_ref()), Some("25"));
        assert_eq!(params.get("offset").map(|value| value.as_ref()), Some("0"));
    }

    #[tokio::test]
    async fn scoped_discogs_artist_search_only_calls_discogs_and_returns_its_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let first = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let body = r#"{"message":"temporary failure"}"#;
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            first
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("scoped-provider.sqlite")).unwrap());
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.discogs = format!("http://{address}");
        service.endpoints.musicbrainz = format!("http://{address}/ws/2");
        let error = service
            .search_artists_for(
                "discogs",
                "Big Thief",
                1,
                25,
                &ArtistCredentials {
                    discogs_token: Some("test-token".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.contains("HTTP 404"));
        let request = server.await.unwrap();
        assert!(
            request.starts_with("GET /database/search?"),
            "unexpected scoped request: {request}"
        );
        assert!(request.contains("type=artist"));
    }

    #[test]
    fn discogs_master_and_release_are_distinct_source_entities() {
        let master =
            parse_discogs_release(&json!({"id":123,"type":"master","title":"Pulse"})).unwrap();
        let release =
            parse_discogs_release(&json!({"id":123,"type":"release","title":"Pulse"})).unwrap();
        assert_eq!(master.release.id, release.release.id);
        assert_ne!(master.source_url, release.source_url);
        assert_eq!(master.source_url, "https://www.discogs.com/master/123");
        assert_eq!(release.source_url, "https://www.discogs.com/release/123");
    }

    #[test]
    fn release_cursor_keeps_musicbrainz_offset_and_discogs_page_independent() {
        assert_eq!(parse_release_cursor(None).unwrap(), (Some(0), Some(1)));
        assert_eq!(
            parse_release_cursor(Some("r2:137:4")).unwrap(),
            (Some(137), Some(4))
        );
        assert_eq!(
            parse_release_cursor(Some("r3:137:x")).unwrap(),
            (Some(137), None)
        );
        assert_eq!(
            parse_release_cursor(Some("r3:x:4")).unwrap(),
            (None, Some(4))
        );
        assert_eq!(
            parse_release_cursor(Some("200")).unwrap(),
            (Some(200), Some(3))
        );
    }

    #[test]
    fn album_search_parser_uses_release_group_identity_and_url() {
        let item = parse_musicbrainz_release_group(&json!({
            "id":"00000000-0000-4000-8000-000000000003", "title":"Dragon New Warm Mountain I Believe in You",
            "first-release-date":"2022-02-11", "primary-type":"Album",
            "artist-credit":[{"name":"Big Thief","artist":{"id":"00000000-0000-4000-8000-000000000004","name":"Big Thief"}}]
        })).unwrap();
        assert_eq!(item.primary_type.as_deref(), Some("Album"));
        assert_eq!(
            item.source_url,
            "https://musicbrainz.org/release-group/00000000-0000-4000-8000-000000000003"
        );
        assert_eq!(item.release.provider, "musicbrainz");
    }

    #[test]
    fn explicit_crosslink_parsing_rejects_lookalike_links() {
        assert_eq!(
            discogs_artist_id("https://www.discogs.com/artist/123-Pulse"),
            Some("123".into())
        );
        assert_eq!(
            musicbrainz_artist_id(
                "https://musicbrainz.org/artist/00000000-0000-4000-8000-000000000001"
            ),
            Some("00000000-0000-4000-8000-000000000001".into())
        );
        assert_eq!(
            musicbrainz_artist_id("https://musicbrainz.org/artist/not-an-id"),
            None
        );
        assert_eq!(
            discogs_artist_id("https://discogs.com.evil.test/artist/123"),
            None
        );
    }

    #[test]
    fn recording_tracks_keep_first_release_date_and_unknown_mix_provenance() {
        let track = parse_musicbrainz_recording(&json!({
            "id":"00000000-0000-4000-8000-000000000002", "title":"Night Drive",
            "length":123456.5, "first-release-date":"2004-03-01", "isrcs":["USAAA0400001"],
            "artist-credit":[{"name":"Pulse","joinphrase":"","artist":{"id":"00000000-0000-4000-8000-000000000001","name":"Pulse"}}]
        }), &ArtistRef { provider:"musicbrainz".into(), id:"00000000-0000-4000-8000-000000000001".into() }).unwrap();
        assert_eq!(track.duration_seconds, Some(123.4565));
        assert_eq!(track.release_date.as_deref(), Some("2004-03-01"));
        assert_eq!(track.isrc.as_deref(), Some("USAAA0400001"));
        assert!(track.version.is_none());
        assert_eq!(track.provenance["recordingFirstReleaseDate"], "2004-03-01");
        assert_eq!(
            track.provenance["mixVersion"],
            "unknown; no version inferred from artist or release text"
        );
    }

    #[test]
    fn recording_tracks_keep_missing_or_invalid_musicbrainz_lengths_unknown() {
        let artist = ArtistRef {
            provider: "musicbrainz".into(),
            id: "00000000-0000-4000-8000-000000000001".into(),
        };
        for (label, length) in [
            ("missing", None),
            ("null", Some(json!(null))),
            ("zero", Some(json!(0))),
            ("negative", Some(json!(-1))),
            ("string", Some(json!("180000"))),
            ("object", Some(json!({"milliseconds":180000}))),
        ] {
            let mut recording = json!({
                "id":"00000000-0000-4000-8000-000000000003",
                "title":"Unknown Length",
                "artist-credit":[{"name":"Pulse","joinphrase":"","artist":{"name":"Pulse"}}]
            });
            if let Some(length) = length {
                recording["length"] = length;
            }
            let parsed = parse_musicbrainz_recording(&recording, &artist)
                .unwrap_or_else(|| panic!("fixture should parse for {label}"));
            assert_eq!(parsed.duration_seconds, None, "{label} length");
        }
    }

    #[test]
    fn recording_duration_refresh_preserves_existing_known_length() {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Store::open(&directory.path().join("artist-recording-duration.sqlite")).unwrap();
        let artist = ArtistRef {
            provider: "musicbrainz".into(),
            id: "00000000-0000-4000-8000-000000000001".into(),
        };
        let known = parse_musicbrainz_recording(
            &json!({
                "id":"00000000-0000-4000-8000-000000000004",
                "title":"Cached Duration",
                "length":180000,
                "artist-credit":[{"name":"Pulse","joinphrase":"","artist":{"name":"Pulse"}}]
            }),
            &artist,
        )
        .unwrap();
        store.save_track(&known).unwrap();
        let refreshed_row = parse_musicbrainz_recording(
            &json!({
                "id":"00000000-0000-4000-8000-000000000004",
                "title":"Cached Duration",
                "artist-credit":[{"name":"Pulse","joinphrase":"","artist":{"name":"Pulse"}}]
            }),
            &artist,
        )
        .unwrap();
        let refreshed = store.save_track(&refreshed_row).unwrap();
        assert_eq!(refreshed.duration_seconds, Some(180.0));
    }

    #[test]
    fn chapter_catalog_suggestion_requires_one_exact_match_and_drops_full_recording_isrc() {
        let chapter = Chapter {
            id: "c1".into(),
            title: "Example Artist - Night Drive".into(),
            start_seconds: 0.0,
            end_seconds: 180.0,
            confidence: "source_metadata_unverified".into(),
            suggested_track: None,
            suggestion_confidence: None,
        };
        let recording = json!({
            "id":"00000000-0000-4000-8000-000000000001", "title":"Night Drive", "length":180000,
            "isrcs":["US-ABC-24-00001"],
            "artist-credit":[{"name":"Example Artist","artist":{"id":"00000000-0000-4000-8000-000000000002","name":"Example Artist"}}]
        });
        let (suggested, confidence) =
            chapter_suggestion(&chapter, std::slice::from_ref(&recording));
        assert_eq!(confidence, "exact_unique_catalog_match");
        assert_eq!(suggested.unwrap().isrc, None);
        let (suggested, confidence) = chapter_suggestion(&chapter, &[recording.clone(), recording]);
        assert_eq!(confidence, "ambiguous_catalog_matches");
        assert!(suggested.is_none());
    }

    #[test]
    fn unsafe_links_and_untrusted_refs_are_rejected() {
        assert!(secure_url("javascript:alert(1)").is_none());
        assert!(secure_url("https://user@example.com/artist").is_none());
        assert!(validate_artist_ref(&ArtistRef {
            provider: "musicbrainz".into(),
            id: "bad".into()
        })
        .is_err());
    }

    #[test]
    fn html_markup_is_removed_from_lastfm_bio() {
        assert_eq!(
            strip_html("<p>One &amp; Two</p>\n<b>three</b>"),
            "One &amp; Two three"
        );
    }

    #[test]
    fn discogs_member_and_group_relationships_keep_roles_and_provider_ids() {
        let selected = ArtistRef {
            provider: "musicbrainz".into(),
            id: "00000000-0000-4000-8000-000000000001".into(),
        };
        let items = parse_discogs_artist_relations(
            &json!({
                "name":"Pulse",
                "members":[{"id":12,"name":"Nova","active":true}],
                "groups":[{"id":34,"name":"The Collective","active":false}]
            }),
            &selected,
            "https://www.discogs.com/artist/123",
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].credits[1].role, "Member");
        assert_eq!(
            items[0].credits[1].artist.as_ref().unwrap().provider,
            "discogs"
        );
        assert_eq!(items[1].credits[1].role, "Group member of");
        assert_eq!(items[1].description.as_deref(), Some("Former relationship"));
        assert_eq!(items[0].credits[0].artist.as_ref().unwrap(), &selected);
    }

    #[tokio::test]
    async fn provider_cache_avoids_a_second_http_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                let mut request = Vec::new();
                let mut chunk = [0u8; 512];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let body = if request.contains("artist.getTopTracks") {
                    r#"{"toptracks":{"track":[{"name":"Signal Bloom","url":"https://www.last.fm/music/Mock/_/Signal+Bloom"}]}}"#
                } else {
                    r#"{"id":"00000000-0000-4000-8000-000000000001","name":"Mock Artist","type":null,"artist-credit":[]}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("artists.sqlite")).unwrap());
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.musicbrainz = format!("http://{address}");
        let url = Url::parse(&format!("http://{address}/artist?fmt=json")).unwrap();
        let first = service
            .get_json(
                "artists.search.musicbrainz",
                "query-1",
                url.clone(),
                60,
                None,
            )
            .await
            .unwrap();
        let second = service
            .get_json("artists.search.musicbrainz", "query-1", url, 60, None)
            .await
            .unwrap();
        assert_eq!(first, second);
        let parsed_artist = parse_musicbrainz_artist(&first).unwrap();
        assert_eq!(parsed_artist.artist_type, None);
        assert!(parse_musicbrainz_credits(
            &first["artist-credit"],
            "https://musicbrainz.org/artist/mock"
        )
        .is_empty());
        let lastfm_url = Url::parse(&format!(
            "http://{address}/2.0/?method=artist.getTopTracks&api_key=test-secret&format=json"
        ))
        .unwrap();
        let lastfm = service
            .get_json(
                "artists.tracks.lastfm",
                "artist-toptracks",
                lastfm_url,
                60,
                None,
            )
            .await
            .unwrap();
        let parsed_tracks = parse_lastfm_tracks(&lastfm, "Mock Artist", &parsed_artist.artist);
        assert_eq!(parsed_tracks.len(), 1);
        assert_eq!(parsed_tracks[0].title, "Signal Bloom");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn discogs_search_404_keeps_musicbrainz_artist_result_usable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen_discogs = Arc::new(std::sync::Mutex::new(String::new()));
        let observed_path = seen_discogs.clone();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let route = request.lines().next().unwrap_or_default().to_owned();
                let (status, body) = if index == 0 {
                    assert!(
                        route.starts_with("GET /ws/2/artist?"),
                        "unexpected MusicBrainz URL: {route}"
                    );
                    (
                        "200 OK",
                        json!({
                            "count":1,
                            "artists":[{"id":"00000000-0000-4000-8000-000000000001","name":"Big Thief","score":"100"}]
                        }),
                    )
                } else {
                    *observed_path.lock().unwrap() = route.clone();
                    ("404 Not Found", json!({"message":"Not Found"}))
                };
                let body = serde_json::to_string(&body).unwrap();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("artist-search-404.sqlite")).unwrap());
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.musicbrainz = format!("http://{address}/ws/2");
        service.endpoints.discogs = format!("http://{address}");
        let page = service
            .search_artists(
                "Big Thief",
                1,
                25,
                &ArtistCredentials {
                    discogs_token: Some("test-token".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.results.len(), 1);
        assert_eq!(page.results[0].name, "Big Thief");
        assert_eq!(page.results[0].artist.provider, "musicbrainz");
        assert!(page.warnings.iter().any(|warning| {
            warning.contains("Discogs artist search failed: Provider returned HTTP 404")
        }));
        let route = seen_discogs.lock().unwrap();
        assert!(
            route.starts_with("GET /database/search?"),
            "Discogs API path is wrong: {route}"
        );
        assert!(
            route.contains("type=artist"),
            "Discogs search is not scoped to artists: {route}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn release_pages_stop_requesting_each_provider_after_its_last_page() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let route = request.lines().next().unwrap_or_default();
                let body = if route.starts_with("GET /ws/2/release?") {
                    observed.fetch_add(1, Ordering::SeqCst);
                    let offset_62 = route.contains("offset=62");
                    let start = if offset_62 { 62 } else { 0 };
                    let count = if offset_62 { 23 } else { 62 };
                    json!({
                        "release-count":85,
                        "releases":(start..start + count).map(|index| json!({"id":format!("mb-{index}"),"title":format!("MB {index}")})).collect::<Vec<_>>()
                    })
                } else if route.starts_with("GET /artists/123/releases?") {
                    observed.fetch_add(1, Ordering::SeqCst);
                    json!({
                        "pagination":{"pages":1},
                        "releases":(1..=65).map(|index| json!({"id":index,"type":"release","title":format!("DG {index}")})).collect::<Vec<_>>()
                    })
                } else {
                    panic!("Unexpected provider request: {route}");
                };
                let body = serde_json::to_string(&body).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("release-pages.sqlite")).unwrap());
        let mbid = "00000000-0000-4000-8000-000000000001";
        store
            .save_artist_mapping("musicbrainz", mbid, "discogs:123")
            .unwrap();
        let mut service = ArtistService::new(store.clone()).unwrap();
        service.endpoints.musicbrainz = format!("http://{address}/ws/2");
        service.endpoints.discogs = format!("http://{address}");
        let selected = ArtistRef {
            provider: "musicbrainz".into(),
            id: mbid.into(),
        };
        let credentials = ArtistCredentials {
            discogs_token: Some("test-token".into()),
            ..Default::default()
        };

        let first = service
            .get_artist_releases(&selected, None, &credentials)
            .await
            .unwrap();
        assert_eq!(first.items.len(), 127);
        assert_eq!(first.next_cursor.as_deref(), Some("r3:62:x"));
        // Force any repeated Discogs page request to reach the fixture server.
        store
            .set_provider_cache(
                "artists.releases.discogs",
                "123:1",
                &json!({"expired":true}),
                0,
            )
            .unwrap();
        let second = service
            .get_artist_releases(&selected, first.next_cursor.as_deref(), &credentials)
            .await
            .unwrap();
        assert_eq!(second.items.len(), 23);
        assert_eq!(second.next_cursor, None);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            3,
            "Discogs is exhausted after its first page and must not be fetched again"
        );
        server.abort();
    }

    #[test]
    fn musicbrainz_track_parser_preserves_compilation_credits_and_occurrences() {
        let edition = "00000000-0000-4000-8000-000000000002";
        let recording = "00000000-0000-4000-8000-000000000003";
        let rows = parse_musicbrainz_release_tracks(
            &json!({
                "title":"Compilation", "artist-credit":[{"name":"Various Artists"}],
                "cover-art-archive":{"front":true},
                "release-group":{"id":"00000000-0000-4000-8000-000000000005"},
                "media":[{"position":1,"tracks":[
                    {"id":"not-a-recording-mbid","number":"1","position":1,"title":"One","recording":{"id":recording,"length":95000}},
                    {"id":"00000000-0000-4000-8000-000000000004","number":"1","position":1,"title":"One (reprise)","recording":{"length":97000,"artist-credit":[{"name":"Guest"}]}}
                ]}]
            }),
            edition,
            "https://musicbrainz.org/release-group/00000000-0000-4000-8000-000000000005",
            true,
            Some("Selected Artist"),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].artist, "Various Artists");
        assert_eq!(rows[1].artist, "Guest");
        assert_ne!(rows[0].id, rows[1].id);
        assert_eq!(rows[0].provenance["recordingId"], recording);
        assert_eq!(rows[0].provenance["artworkAlternates"], json!([
            "https://coverartarchive.org/release/00000000-0000-4000-8000-000000000002/front-250",
            "https://coverartarchive.org/release-group/00000000-0000-4000-8000-000000000005/front-250"
        ]));
        assert_eq!(rows[0].provenance["trackGuid"], "not-a-recording-mbid");
        assert_eq!(rows[0].track_number, Some(1));
        assert_eq!(rows[0].disc_number, Some(1));
        assert!(rows[0].provider_id.contains(recording));
        assert_tracks_round_trip(&rows);
    }

    #[test]
    fn musicbrainz_track_parser_separates_numeric_ordinals_from_printed_numbering() {
        let release_id = "00000000-0000-4000-8000-000000000002";
        let rows = parse_musicbrainz_release_tracks(
            &json!({
                "title":"Album",
                "artist-credit":[{"name":"Artist"}],
                "media":[{"position":2,"tracks":[{
                    "id":"00000000-0000-4000-8000-000000000003",
                    "number":"A1","position":3,"title":"Song",
                    "recording":{"id":"00000000-0000-4000-8000-000000000004","length":120000}
                }]}]
            }),
            release_id,
            &format!("https://musicbrainz.org/release/{release_id}"),
            false,
            None,
        );
        let track = &rows[0];
        assert_eq!(track.track_number, Some(3));
        assert_eq!(track.disc_number, Some(2));
        assert_eq!(track.provenance["trackPosition"], "A1");
        assert_eq!(track.provenance["trackPositionOrdinal"], 3);
        assert_eq!(track.provenance["sourceMediumPosition"], 2);
        assert_eq!(
            crate::recording_identity::trusted_musicbrainz_ordinals(track),
            (Some(3), Some(2))
        );

        let mut cached_legacy = track.clone();
        cached_legacy.track_number = None;
        cached_legacy.disc_number = None;
        cached_legacy
            .provenance
            .as_object_mut()
            .unwrap()
            .remove("trackPositionOrdinal");
        cached_legacy
            .provenance
            .as_object_mut()
            .unwrap()
            .remove("sourceMediumPosition");
        cached_legacy.provenance["trackPosition"] = json!("4");
        cached_legacy.provenance["mediumPosition"] = json!(2);
        assert_eq!(
            crate::recording_identity::trusted_musicbrainz_ordinals(&cached_legacy),
            (Some(4), Some(2))
        );

        cached_legacy.provenance["trackPosition"] = json!("A1");
        assert_eq!(
            crate::recording_identity::trusted_musicbrainz_ordinals(&cached_legacy),
            (None, Some(2)),
            "nonnumeric printed numbers remain provenance, not ordinals"
        );

        cached_legacy.provenance["manualFields"] = json!({"trackNumber":true});
        assert_eq!(
            crate::recording_identity::trusted_musicbrainz_ordinals(&cached_legacy),
            (None, Some(2))
        );
    }

    #[test]
    fn musicbrainz_track_parser_uses_track_credit_when_recording_credit_is_empty() {
        let rows = parse_musicbrainz_release_tracks(
            &json!({
                "title":"Compilation",
                "artist-credit":[{"name":"Various Artists"}],
                "media":[{"position":1,"tracks":[{
                    "id":"00000000-0000-4000-8000-000000000008",
                    "number":"1","position":1,"title":"Guest Song",
                    "artist-credit":[{"name":"","artist":{"id":"00000000-0000-4000-8000-000000000009","name":"Guest"}}],
                    "recording":{"id":"00000000-0000-4000-8000-000000000010","length":120000,"artist-credit":[]}
                }]}]
            }),
            "00000000-0000-4000-8000-000000000002",
            "https://musicbrainz.org/release/00000000-0000-4000-8000-000000000002",
            false,
            Some("Selected Artist"),
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].artist, "Guest");
        assert_eq!(rows[0].provenance["artistCredits"][0]["name"], "Guest");
        assert_tracks_round_trip(&rows);
    }

    #[test]
    fn discogs_parser_flattens_subtracks_and_keeps_various_artist_credit() {
        let rows = parse_discogs_release_tracks(
            &json!({
                "title":"Compilation", "artists":[{"id":1,"name":"Various"}],
                "tracklist":[
                    {"type_":"heading","title":"Side A"},
                    {"type_":"index","title":"Suite","position":"A1","sub_tracks":[
                        {"type_":"track","title":"Part I","position":"A1-1","duration":"1:30"},
                        {"type_":"track","title":"Part II","position":"A1-2","duration":"2:00"}
                    ]},
                    {"type_":"track","title":"End","position":"A2","duration":"3:00"}
                ]
            }),
            "123",
            "https://www.discogs.com/master/45",
            true,
            Some("Selected Artist"),
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].title, "Part I");
        assert_eq!(rows[0].provenance["parentTitle"], "Suite");
        assert_eq!(rows[0].artist, "Various");
        assert_eq!(rows[0].duration_seconds, Some(90.0));
        assert_eq!(rows[2].title, "End");
        assert_tracks_round_trip(&rows);
    }

    #[test]
    fn discogs_parser_uses_main_credits_and_defaults_empty_track_credit_roles() {
        let rows = parse_discogs_release_tracks(
            &json!({
                "title":"Secret Life at the Coliseum",
                "artists":[
                    {"id":8664813,"name":"Fred again..","role":"Main","join":" & "},
                    {"id":634,"name":"Brian Eno","role":"Main"}
                ],
                "tracklist":[
                    {"type_":"track","title":"Enough...","position":"A1","artists":[
                        {"id":8664813,"name":"Fred again..","role":"","join":" & "},
                        {"id":634,"name":"Brian Eno","role":""}
                    ]},
                    {"type_":"track","title":"Secret","position":"A2"}
                ]
            }),
            "36487582",
            "https://www.discogs.com/release/36487582",
            false,
            None,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Enough...");
        assert_eq!(rows[0].artist, "Fred again.. & Brian Eno");
        assert_eq!(rows[1].artist, "Fred again.. & Brian Eno");
        assert_tracks_round_trip(&rows);
    }

    #[test]
    fn discogs_non_artist_track_credit_does_not_blank_the_release_artist() {
        let rows = parse_discogs_release_tracks(
            &json!({
                "title":"Release",
                "artists":[{"id":8664813,"name":"Fred again..","role":"Main"}],
                "tracklist":[{"type_":"track","title":"Enough...","position":"A1","artists":[
                    {"id":634,"name":"Brian Eno","role":"Remix"}
                ]}]
            }),
            "12345",
            "https://www.discogs.com/release/12345",
            false,
            None,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].artist, "Fred again..");
        let credits = rows[0].provenance["artistCredits"].as_array().unwrap();
        assert_eq!(credits.len(), 2);
        assert_eq!(credits[0]["role"], "Main");
        assert_eq!(credits[1]["role"], "Remix");
    }
    #[tokio::test]
    async fn spotify_album_crosswalk_fetches_exact_url_and_release_then_returns_occurrence() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let responses = [
                include_str!("../fixtures/mb-url-relations.json"),
                include_str!("../fixtures/mb-release-public.json"),
            ];
            let mut paths = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                paths.push(request.lines().next().unwrap_or_default().to_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            paths
        });
        let temp = tempfile::tempdir().unwrap();
        let store =
            Arc::new(Store::open(&temp.path().join("spotify-mb-crosswalk.sqlite")).unwrap());
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.musicbrainz = format!("http://{address}/ws/2");
        let album: Value =
            serde_json::from_str(include_str!("../fixtures/spotify-album.json")).unwrap();
        let mapped = service
            .verify_spotify_album_recording(&album, "0j8ulfrtVtBnyBr6p0CfFu", 1)
            .await
            .unwrap();
        assert_eq!(
            mapped.musicbrainz_release_id,
            "b8254485-3b63-4e55-8776-7bd713fe3dd7"
        );
        assert_eq!(
            mapped.musicbrainz_recording_id,
            "5a1d6c4a-bd16-4d09-87da-6af8d4daf36c"
        );
        assert_eq!(mapped.spotify_position, 1);
        assert_eq!(mapped.musicbrainz_title, "Kaval Sviri (The Flute Plays)");
        assert_eq!(mapped.musicbrainz_duration_ms, Some(131000));
        assert!(!mapped.provider_artist_credit_matches);
        let cached = service
            .get_verified_spotify_recording("6E4eD35ZrPXH2bF6LnPphg", "0j8ulfrtVtBnyBr6p0CfFu", 1)
            .unwrap()
            .unwrap();
        assert_eq!(cached, mapped);
        assert_eq!(
            service
                .get_verified_spotify_recording_by_album_track(
                    "6E4eD35ZrPXH2bF6LnPphg",
                    "0j8ulfrtVtBnyBr6p0CfFu",
                )
                .unwrap()
                .unwrap(),
            mapped,
        );
        write_verified_spotify_recording_sidecar(&service.store, &mapped, 2).unwrap();
        assert!(service
            .get_verified_spotify_recording_by_album_track(
                "6E4eD35ZrPXH2bF6LnPphg",
                "0j8ulfrtVtBnyBr6p0CfFu",
            )
            .unwrap()
            .is_none());
        assert_eq!(
            service
                .get_verified_spotify_recording(
                    "6E4eD35ZrPXH2bF6LnPphg",
                    "0j8ulfrtVtBnyBr6p0CfFu",
                    1,
                )
                .unwrap()
                .unwrap(),
            mapped
        );
        assert!(service
            .get_verified_spotify_recording("6E4eD35ZrPXH2bF6LnPphX", "0j8ulfrtVtBnyBr6p0CfFu", 1,)
            .unwrap()
            .is_none());
        assert!(service
            .get_verified_spotify_recording_by_album_track(
                "6E4eD35ZrPXH2bF6LnPphX",
                "0j8ulfrtVtBnyBr6p0CfFu",
            )
            .unwrap()
            .is_none());
        let paths = server.await.unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].starts_with("GET /ws/2/url?"));
        assert!(paths[0]
            .contains("resource=https%3A%2F%2Fopen.spotify.com%2Falbum%2F6E4eD35ZrPXH2bF6LnPphg"));
        let request_target = paths[0].split_whitespace().nth(1).unwrap();
        let request_url = Url::parse(&format!("http://localhost{request_target}")).unwrap();
        let includes = request_url
            .query_pairs()
            .filter(|(key, _)| key == "inc")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>();
        assert_eq!(includes, vec!["release-rels"]);
        let release_target = paths[1].split_whitespace().nth(1).unwrap();
        let release_url = Url::parse(&format!("http://localhost{release_target}")).unwrap();
        let release_includes = release_url
            .query_pairs()
            .filter(|(key, _)| key == "inc")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>();
        assert_eq!(release_includes, vec!["recordings+artist-credits"]);
        assert!(paths[1].starts_with("GET /ws/2/release/b8254485-3b63-4e55-8776-7bd713fe3dd7?"));
    }

    #[test]
    fn verified_spotify_unique_index_is_invalidated_by_duplicate_or_missing_target() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("spotify-mb-invalidation.sqlite")).unwrap();
        let album: Value =
            serde_json::from_str(include_str!("../fixtures/spotify-album.json")).unwrap();
        let relations = normalize_url_relations(
            &serde_json::from_str::<Value>(include_str!("../fixtures/mb-url-relations.json"))
                .unwrap(),
        )
        .unwrap();
        let release = normalize_musicbrainz_release(
            &serde_json::from_str::<Value>(include_str!("../fixtures/mb-release-public.json"))
                .unwrap(),
        )
        .unwrap();
        let occurrence =
            resolve_occurrence(&album, "0j8ulfrtVtBnyBr6p0CfFu", 1, &relations, &release).unwrap();
        for occurrence_count in [2, 0] {
            write_verified_spotify_recording_sidecar(&store, &occurrence, 1).unwrap();
            assert!(read_verified_spotify_recording_sidecar(
                &store,
                &occurrence.spotify_album_id,
                &occurrence.spotify_track_id
            )
            .unwrap()
            .is_some());
            invalidate_verified_spotify_recording_sidecar(
                &store,
                &occurrence.spotify_album_id,
                &occurrence.spotify_track_id,
                occurrence_count,
            )
            .unwrap();
            assert!(read_verified_spotify_recording_sidecar(
                &store,
                &occurrence.spotify_album_id,
                &occurrence.spotify_track_id
            )
            .unwrap()
            .is_none());
        }
    }

    #[tokio::test]
    async fn spotify_same_title_track_outside_album_is_rejected_before_catalog_http() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("spotify-mb-negative.sqlite")).unwrap());
        let mut service = ArtistService::new(store).unwrap();
        service.endpoints.musicbrainz = "http://127.0.0.1:9/ws/2".into();
        let album: Value =
            serde_json::from_str(include_str!("../fixtures/spotify-album.json")).unwrap();
        let error = service
            .verify_spotify_album_recording(&album, "64rvuYgKLuIHqacdZZiwPr", 1)
            .await
            .unwrap_err();
        assert!(error.contains("Spotify album occurrence"));
    }
}
