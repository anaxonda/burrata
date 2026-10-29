//! Per-source unified discovery API. The UI can call `search_source` once per
//! selected source and render each bounded response as it arrives.
use crate::{
    acquisition::{AcquisitionManager, SlskdSearchPoll},
    artists::{ArtistCredentials, ArtistService},
    connections::ConnectionManager,
    models::Track,
    storage::Store,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const MAX_PAGE: usize = 50;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRequest {
    pub query: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub isrc: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    pub id: String,
    pub kind: String,
    pub source_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    pub track: Option<Track>,
    pub data: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPage {
    pub source_id: String,
    pub results: Vec<DiscoveryResult>,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub progress: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapability {
    pub id: String,
    pub name: String,
    pub kinds: Vec<String>,
    pub enabled: bool,
    pub detail: Option<String>,
}

/// Maximum serialized discovery buffer cursor length accepted by both the
/// discovery decoder and the authenticated automation request boundary.
pub(crate) const MAX_DISCOVERY_CURSOR_LENGTH: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryBufferCursor {
    source_id: String,
    query: String,
    kinds: Vec<String>,
    #[serde(default)]
    isrc: Option<String>,
    provider_cursor: Option<String>,
    buffered: Vec<DiscoveryResult>,
    /// Old cursors predate partial-result metadata; those pages were complete.
    #[serde(default = "default_page_complete")]
    complete: bool,
    #[serde(default)]
    progress: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

fn default_page_complete() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioPageCursor {
    source_id: String,
    query: String,
    kinds: Vec<String>,
    #[serde(default)]
    isrc: Option<String>,
    offset: usize,
    remote_cursor: Option<String>,
    /// Provider-issued YouTube Music continuation, bound to the fields above.
    #[serde(default)]
    music_continuation: Option<crate::youtube_music::YouTubeMusicContinuation>,
    #[serde(default)]
    music_searched: bool,
    #[serde(default)]
    audio_search_done: bool,
    #[serde(default)]
    seen_music_tokens: Vec<u64>,
    #[serde(default)]
    seen_music_ids: Vec<String>,
    #[serde(default = "default_page_complete")]
    complete: bool,
    #[serde(default)]
    progress: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

fn should_search_youtube_music(
    source_id: &str,
    wants_track: bool,
    remote_page_continuation: bool,
    audio_offset: Option<usize>,
    music_continuation: Option<&crate::youtube_music::YouTubeMusicContinuation>,
    music_searched: bool,
) -> bool {
    source_id == "youtube"
        && wants_track
        && !remote_page_continuation
        && (audio_offset.is_none() || music_continuation.is_some())
        && (!music_searched || music_continuation.is_some())
}

fn take_unseen_music_items(
    items: Vec<crate::youtube_music::YouTubeMusicItem>,
    seen_ids: &mut Vec<String>,
) -> Vec<crate::youtube_music::YouTubeMusicItem> {
    let mut seen = seen_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut fresh = Vec::new();
    for item in items {
        if seen.insert(item.id.clone()) {
            seen_ids.push(item.id.clone());
            fresh.push(item);
        }
    }
    let excess = seen_ids.len().saturating_sub(500);
    if excess > 0 {
        seen_ids.drain(..excess);
    }
    fresh
}

fn valid_youtube_music_continuation(
    continuation: &crate::youtube_music::YouTubeMusicContinuation,
) -> bool {
    !continuation.token.is_empty()
        && continuation.token.len() <= 8 * 1024
        && continuation
            .click_tracking_params
            .as_ref()
            .map_or(true, |value| value.len() <= 2 * 1024)
}

fn music_token_fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

fn encode_audio_page_cursor(cursor: &AudioPageCursor) -> Result<String, String> {
    let bytes = serde_json::to_vec(cursor).map_err(|error| error.to_string())?;
    Ok(format!(
        "ap1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn decode_audio_page_cursor(
    value: &str,
    source_id: &str,
    query: &str,
    kinds: &[String],
    isrc: Option<&str>,
) -> Result<AudioPageCursor, String> {
    if value.len() > 32 * 1024 {
        return Err("Audio search cursor is too large".into());
    }
    let encoded = value
        .strip_prefix("ap1:")
        .ok_or("Invalid audio search cursor")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Invalid audio search cursor")?;
    let cursor: AudioPageCursor =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid audio search cursor")?;
    if cursor.source_id != source_id
        || cursor.query != query
        || cursor.kinds != kinds
        || cursor.isrc.as_deref() != isrc
        || cursor.offset >= 10_000
        || cursor
            .music_continuation
            .as_ref()
            .is_some_and(|continuation| !valid_youtube_music_continuation(continuation))
        || cursor.seen_music_tokens.len() > 32
        || cursor.seen_music_ids.len() > 500
        || cursor.seen_music_ids.iter().any(|id| id.len() != 11)
        || (cursor.music_continuation.is_some() && source_id != "youtube")
        || !matches!(source_id, "youtube" | "soundcloud")
    {
        return Err("Audio search cursor does not match this search".into());
    }
    Ok(cursor)
}

fn encode_discovery_cursor(cursor: &DiscoveryBufferCursor) -> Result<String, String> {
    let bytes = serde_json::to_vec(cursor).map_err(|error| error.to_string())?;
    Ok(format!(
        "dsb1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn decode_discovery_cursor(
    value: &str,
    source_id: &str,
    query: &str,
    kinds: &[String],
    isrc: Option<&str>,
) -> Result<DiscoveryBufferCursor, String> {
    if value.len() > MAX_DISCOVERY_CURSOR_LENGTH {
        return Err("Discovery cursor is too large".into());
    }
    let encoded = value
        .strip_prefix("dsb1:")
        .ok_or("Invalid discovery cursor")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Invalid discovery cursor")?;
    let cursor: DiscoveryBufferCursor =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid discovery cursor")?;
    if cursor.source_id != source_id
        || cursor.query != query
        || cursor.kinds != kinds
        || cursor.isrc.as_deref() != isrc
        || cursor.buffered.len() > 500
    {
        return Err("Discovery cursor does not match this search".into());
    }
    Ok(cursor)
}

fn cap_discovery_page(
    mut page: DiscoveryPage,
    mut buffered: Vec<DiscoveryResult>,
    source_id: &str,
    query: &str,
    kinds: &[String],
    isrc: Option<&str>,
    limit: usize,
) -> Result<DiscoveryPage, String> {
    let mut seen_warnings = std::collections::HashSet::new();
    page.warnings
        .retain(|warning| seen_warnings.insert(warning.clone()));
    buffered.append(&mut page.results);
    page.results = buffered;
    if page.results.len() > limit {
        let remainder = page.results.split_off(limit);
        page.next_cursor = Some(encode_discovery_cursor(&DiscoveryBufferCursor {
            source_id: source_id.into(),
            query: query.into(),
            kinds: kinds.to_vec(),
            isrc: isrc.map(str::to_owned),
            provider_cursor: page.next_cursor,
            buffered: remainder,
            complete: page.complete,
            progress: page.progress.clone(),
            warnings: page.warnings.clone(),
        })?);
    }
    Ok(page)
}
pub fn source_registry(connections: &ConnectionManager) -> Result<Vec<SourceCapability>, String> {
    let c = connections.settings()?;
    Ok(vec![
        cap("library", &["track"], true, None),
        cap("musicbrainz", &["artist", "album", "track"], true, None),
        cap(
            "discogs",
            &["artist", "album"],
            c.discogs_enabled && connections.credential("discogs")?.is_some(),
            Some("Enable Discogs in Connections"),
        ),
        cap("youtube", &["track", "video"], true, None),
        cap("soundcloud", &["track", "video"], true, None),
        cap(
            "bandcamp",
            &["track"],
            true,
            Some("Public track search; streaming availability is checked on the track page"),
        ),
        cap(
            "monochrome",
            &["artist", "album", "track"],
            true,
            Some("Artist and release metadata from the public Tracks API; audio availability and quality are not asserted"),
        ),
        cap(
            "soulseek",
            &["artist", "album", "track", "file"],
            c.slskd.enabled,
            Some(
                "Artists and albums are inferred from peer filenames and folders; directory completeness is unknown until browsed",
            ),
        ),
    ])
}

/// Add providers whose availability depends on a live browser account. The
/// ordinary registry remains synchronous for older callers and tests.
pub async fn source_registry_with_remote(
    connections: &ConnectionManager,
    remote: Option<&crate::remote_playlists::RemotePlaylistService>,
) -> Result<Vec<SourceCapability>, String> {
    let mut sources = source_registry(connections)?;
    if let Some(remote) = remote {
        if let Some(youtube) = sources
            .iter_mut()
            .find(|source| source.id == "youtube" && remote.browser_connected())
        {
            youtube.kinds.push("playlist".into());
            youtube.kinds.push("channel".into());
        }
        let spotify = remote.connection_status("spotify").await;
        if let Ok(status) = spotify {
            if status.connected {
                sources.push(cap(
                    "spotify",
                    &["track", "artist", "album", "playlist", "profile"],
                    true,
                    None,
                ));
            }
        }
    }
    Ok(sources)
}
fn cap(id: &str, kinds: &[&str], enabled: bool, detail: Option<&str>) -> SourceCapability {
    SourceCapability {
        id: id.into(),
        name: match id {
            "library" => "Library",
            "musicbrainz" => "MusicBrainz",
            "discogs" => "Discogs",
            "beatport" => "Beatport",
            "traxsource" => "Traxsource",
            "spotify" => "Spotify",
            "youtube" => "YouTube",
            "soundcloud" => "SoundCloud",
            "bandcamp" => "Bandcamp",
            "soulseek" => "Soulseek",
            "monochrome" => "Monochrome",
            _ => id,
        }
        .into(),
        kinds: kinds.iter().map(|x| (*x).into()).collect(),
        enabled,
        detail: detail.map(str::to_owned),
    }
}
fn youtube_tokens(value: &str) -> std::collections::HashSet<String> {
    value
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}
fn youtube_relevance(row: &DiscoveryResult, query: &str) -> (usize, usize, usize) {
    let title = row
        .track
        .as_ref()
        .map(|t| t.title.as_str())
        .unwrap_or(row.title.as_str());
    let artist = row
        .track
        .as_ref()
        .map(|t| t.artist.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            row.data
                .pointer("/youtubeMusic/artist")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            row.data
                .pointer("/sourceMetadata/artist")
                .and_then(Value::as_str)
        })
        .or(row.subtitle.as_deref())
        .unwrap_or("");
    let title_tokens = youtube_tokens(title);
    let artist_tokens = youtube_tokens(artist);
    let query_tokens = youtube_tokens(query);
    let title_hits = query_tokens
        .iter()
        .filter(|token| title_tokens.contains(*token))
        .count();
    let total = query_tokens
        .iter()
        .filter(|token| title_tokens.contains(*token) || artist_tokens.contains(*token))
        .count();
    (
        (total == query_tokens.len() && !query_tokens.is_empty()) as usize,
        total,
        title_hits,
    )
}
fn rank_youtube_results(
    rows: &mut [DiscoveryResult],
    query: &str,
    prefer_video: bool,
    prefer_track: bool,
) {
    rows.sort_by(|left, right| {
        let l = youtube_relevance(left, query);
        let r = youtube_relevance(right, query);
        let l_kind = if prefer_video {
            matches!(left.kind.as_str(), "video" | "set")
        } else if prefer_track {
            left.kind == "track"
        } else {
            false
        };
        let r_kind = if prefer_video {
            matches!(right.kind.as_str(), "video" | "set")
        } else if prefer_track {
            right.kind == "track"
        } else {
            false
        };
        (r.0, r.1, r.2, r_kind).cmp(&(l.0, l.1, l.2, l_kind))
    });
}

fn append_remote_catalog_results(out: &mut DiscoveryPage, source_id: &str, page: &Value) {
    out.next_cursor = page
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(items) = page.get("items").and_then(Value::as_array) else {
        out.warnings
            .push("Remote catalog returned an invalid result page".into());
        return;
    };
    for item in items {
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(kind) = item
            .get("kind")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_owned();
        let subtitle = item
            .get("subtitle")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let url = item.get("url").and_then(Value::as_str).map(str::to_owned);
        let artwork_url = item
            .get("artworkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let native = item.get("data").cloned().unwrap_or(Value::Null);
        let track = (kind == "track").then(|| {
            let artist = native.get("artist").and_then(Value::as_str).map(str::to_owned).or_else(|| subtitle.clone()).unwrap_or_default();
            let album = native.get("album").and_then(Value::as_str).map(str::to_owned);
            let mut provenance = json!({"catalog":{"provider":source_id,"providerId":id,"verified":source_id == "spotify"},"remoteCatalog":native.clone()});
            if source_id == "spotify" {
                // Preserve valid duplicate credits and the provider's named-credit
                // count. Missing/invalid/truncated identities remain visibly
                // incomplete to the UI, rather than becoming a false exact match.
                let artist_refs = native.get("artistRefs").and_then(Value::as_array)
                    .into_iter().flatten().take(100).filter_map(|artist| {
                        let provider = artist.get("provider").and_then(Value::as_str)?;
                        let artist_id = artist.get("id").and_then(Value::as_str)?;
                        let name = artist.get("name").and_then(Value::as_str)?.trim();
                        (provider == "spotify" && artist_id.len() == 22
                            && artist_id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                            && !name.is_empty())
                            .then(|| json!({"provider":"spotify","id":artist_id,"name":name}))
                    }).collect::<Vec<_>>();
                provenance["catalog"]["artists"] = json!(artist_refs);
                if let Some(count) = native.get("artistCount").and_then(Value::as_u64) {
                    provenance["catalog"]["artistCount"] = json!(count);
                }
            }
            if source_id == "youtube" {
                provenance["sourceMetadata"] = native.clone();
                provenance["catalogVerified"] = json!(false);
            }
            Track {
                id: format!("{source_id}:{id}"),
                provider: source_id.into(),
                provider_id: id.into(),
                source_url: url.clone().unwrap_or_default(),
                artist,
                title: title.clone(),
                album,
                isrc: native.get("isrc").and_then(Value::as_str).map(str::to_owned),
                duration_seconds: native.get("durationSeconds").and_then(Value::as_f64),
                artwork_url: item.get("artworkUrl").and_then(Value::as_str).map(str::to_owned),
                provenance,
                ..Track::default()
            }
        });
        out.results.push(DiscoveryResult {
            id: format!("{source_id}:{id}"),
            kind: kind.into(),
            source_id: source_id.into(),
            title,
            subtitle,
            url,
            artwork_url,
            track,
            data: item.clone(),
        });
    }
}
fn library_search_page(
    store: &Store,
    query: &str,
    page: u32,
    limit: usize,
) -> Result<(Vec<Track>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_PAGE);
    let page = page.clamp(1, 200);
    let offset = (page as usize - 1) * limit;
    let terms = query
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (rows, total) = store.search_library_tracks(&terms, limit, offset)?;
    let next = (offset.saturating_add(limit) < total).then(|| (page + 1).to_string());
    Ok((rows, next))
}
async fn search_musicbrainz_recordings(
    query: &str,
    isrc: Option<&str>,
    limit: usize,
    page: u32,
) -> Result<(Vec<Value>, bool), String> {
    let mut url =
        url::Url::parse("https://musicbrainz.org/ws/2/recording").map_err(|e| e.to_string())?;
    let isrc = isrc.filter(|s| {
        !s.trim().is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    let lucene = isrc
        .map(|s| format!("isrc:{}", s.trim()))
        .unwrap_or_else(|| query.split_whitespace().collect::<Vec<_>>().join(" "));
    let limit = limit.clamp(1, 50);
    url.query_pairs_mut()
        .append_pair("query", &lucene)
        .append_pair("fmt", "json")
        .append_pair("limit", &limit.to_string())
        .append_pair(
            "offset",
            &page
                .saturating_sub(1)
                .saturating_mul(limit as u32)
                .to_string(),
        )
        .append_pair("inc", "artist-credits+isrcs+genres+releases");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Burrata/0.1.0 (music discovery)")
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("MusicBrainz recording search failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("MusicBrainz returned HTTP {}", response.status()));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Invalid MusicBrainz recording response: {e}"))?;
    let more = body["count"]
        .as_u64()
        .is_some_and(|count| count > page.saturating_mul(limit as u32) as u64);
    Ok((
        body["recordings"].as_array().cloned().unwrap_or_default(),
        more,
    ))
}

pub struct DiscoveryService {
    pub artists: Arc<ArtistService>,
    pub acquisition: Arc<AcquisitionManager>,
    pub store: Arc<Store>,
    pub connections: Arc<ConnectionManager>,
    remote: std::sync::RwLock<Option<Arc<crate::remote_playlists::RemotePlaylistService>>>,
    #[cfg(test)]
    monochrome_test_base: std::sync::RwLock<Option<String>>,
}
impl DiscoveryService {
    pub fn new(
        artists: Arc<ArtistService>,
        acquisition: Arc<AcquisitionManager>,
        store: Arc<Store>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        Self {
            artists,
            acquisition,
            store,
            connections,
            remote: std::sync::RwLock::new(None),
            #[cfg(test)]
            monochrome_test_base: std::sync::RwLock::new(None),
        }
    }
    pub fn set_remote_service(&self, service: Arc<crate::remote_playlists::RemotePlaylistService>) {
        if let Ok(mut slot) = self.remote.write() {
            *slot = Some(service);
        }
    }
    fn remote_service(&self) -> Option<Arc<crate::remote_playlists::RemotePlaylistService>> {
        self.remote.read().ok().and_then(|slot| slot.clone())
    }
    async fn search_monochrome(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<(Vec<(Track, crate::models::Candidate)>, usize), String> {
        #[cfg(test)]
        if let Some(base) = self
            .monochrome_test_base
            .read()
            .ok()
            .and_then(|base| base.clone())
        {
            return crate::monochrome::Client::at(&base)?
                .search_with_limit(query, limit)
                .await;
        }
        crate::monochrome::Client::new()?
            .search_with_limit(query, limit)
            .await
    }
    pub async fn search_source(
        &self,
        mut request: DiscoveryRequest,
        source_id: &str,
    ) -> Result<DiscoveryPage, String> {
        let registered_id = if source_id == "slskd" {
            "soulseek"
        } else {
            source_id
        };
        let remote = self.remote_service();
        // A search for another provider must not wait on Spotify's browser session.
        let mut sources = if registered_id == "spotify" {
            source_registry_with_remote(&self.connections, remote.as_deref()).await?
        } else {
            source_registry(&self.connections)?
        };
        if registered_id == "youtube"
            && remote
                .as_ref()
                .is_some_and(|service| service.browser_connected())
        {
            if let Some(youtube) = sources.iter_mut().find(|source| source.id == "youtube") {
                youtube.kinds.extend(["playlist".into(), "channel".into()]);
            }
        }
        let capability = sources.into_iter().find(|source| source.id == registered_id)
            .ok_or_else(|| format!("{source_id} is not a searchable catalog. Use My library to search saved/imported tracks."))?;
        if !capability.enabled {
            return Err(format!("{} is not configured for search", capability.name));
        }
        let q = request.query.trim();
        if q.is_empty() || q.chars().count() > 500 {
            return Err("Search query must contain 1 to 500 characters".into());
        }
        if !request.source_ids.is_empty() && !request.source_ids.iter().any(|s| s == source_id) {
            return Err("Source is not selected for this search".into());
        }
        // YouTube combines Innertube music pages and raw video pages. Keep each
        // public discovery page bounded at 30; cap_discovery_page retains any
        // additional renderer rows in its opaque buffer cursor.
        let limit = request
            .limit
            .unwrap_or(25)
            .clamp(1, MAX_PAGE)
            .min(if source_id == "youtube" { 30 } else { MAX_PAGE });
        if matches!(source_id, "slskd" | "soulseek") {
            if let Some(cursor) = request.cursor.as_deref() {
                return self.poll_slskd_with_kinds(cursor, &request.kinds).await;
            }
        }
        let mut buffered = Vec::new();
        let mut prior_complete = true;
        let mut prior_progress = None;
        let mut prior_warnings = Vec::new();
        if !matches!(registered_id, "slskd" | "soulseek")
            && request
                .cursor
                .as_deref()
                .is_some_and(|cursor| cursor.starts_with("dsb1:"))
        {
            let state = decode_discovery_cursor(
                request.cursor.as_deref().unwrap_or_default(),
                registered_id,
                q,
                &request.kinds,
                request.isrc.as_deref(),
            )?;
            request.cursor = state.provider_cursor;
            buffered = state.buffered;
            prior_complete = state.complete;
            prior_progress = state.progress;
            prior_warnings = state.warnings;
            if buffered.len() >= limit || request.cursor.is_none() {
                return cap_discovery_page(
                    DiscoveryPage {
                        source_id: registered_id.into(),
                        results: Vec::new(),
                        next_cursor: request.cursor,
                        complete: prior_complete,
                        progress: prior_progress,
                        warnings: prior_warnings,
                    },
                    buffered,
                    registered_id,
                    q,
                    &request.kinds,
                    request.isrc.as_deref(),
                    limit,
                );
            }
        }
        let mut audio_offset = None;
        let mut music_continuation = None;
        let mut music_searched = false;
        let mut audio_search_done = false;
        let mut seen_music_tokens = Vec::new();
        let mut seen_music_ids = Vec::new();
        if request
            .cursor
            .as_deref()
            .is_some_and(|cursor| cursor.starts_with("ap1:"))
        {
            let state = decode_audio_page_cursor(
                request.cursor.as_deref().unwrap_or_default(),
                registered_id,
                q,
                &request.kinds,
                request.isrc.as_deref(),
            )?;
            audio_offset = Some(state.offset);
            music_continuation = state.music_continuation;
            music_searched = state.music_searched;
            audio_search_done = state.audio_search_done;
            prior_complete = state.complete;
            prior_progress = state.progress;
            prior_warnings.extend(state.warnings);
            seen_music_tokens = state.seen_music_tokens;
            seen_music_ids = state.seen_music_ids;
            if let Some(continuation) = music_continuation.as_ref() {
                let fingerprint = music_token_fingerprint(&continuation.token);
                if !seen_music_tokens.contains(&fingerprint) {
                    seen_music_tokens.push(fingerprint);
                }
            }
            request.cursor = state.remote_cursor;
        }
        let page = request
            .cursor
            .as_deref()
            .unwrap_or("1")
            .parse::<u32>()
            .unwrap_or(1)
            .clamp(1, 200);
        let mut out = DiscoveryPage {
            source_id: source_id.into(),
            results: vec![],
            next_cursor: None,
            complete: prior_complete,
            progress: prior_progress,
            warnings: prior_warnings,
        };
        match source_id {
            "spotify" => {
                let service = remote.ok_or_else(|| {
                    "Connect Spotify in Browser Connections before searching it".to_owned()
                })?;
                let kinds = if request.kinds.is_empty() {
                    vec!["track", "artist", "album", "playlist", "profile"]
                } else {
                    request
                        .kinds
                        .iter()
                        .map(String::as_str)
                        .filter(|kind| {
                            matches!(*kind, "track" | "artist" | "album" | "playlist" | "profile")
                        })
                        .collect()
                };
                if !kinds.is_empty() {
                    let page = service
                        .search_catalog(
                            "spotify",
                            q,
                            kinds.into_iter().map(str::to_owned).collect(),
                            request.cursor.as_deref(),
                        )
                        .await?;
                    append_remote_catalog_results(&mut out, "spotify", &page);
                }
            }
            "monochrome" => {
                if kind_ok(&request, "artist") {
                    let artists = self
                        .artists
                        .search_artists_for(
                            "monochrome",
                            q,
                            page,
                            limit.min(25) as u32,
                            &ArtistCredentials::default(),
                        )
                        .await?;
                    for artist in artists.results.into_iter().take(limit) {
                        out.results.push(DiscoveryResult {
                            id: format!("monochrome:{}", artist.artist.id),
                            kind: "artist".into(),
                            source_id: source_id.into(),
                            title: artist.name.clone(),
                            subtitle: None,
                            url: Some(artist.source_url.clone()),
                            artwork_url: artist.image_url.clone(),
                            track: None,
                            data: serde_json::to_value(artist).unwrap_or(Value::Null),
                        });
                    }
                    out.warnings.extend(artists.warnings);
                }
                if kind_ok(&request, "album") {
                    let albums = self
                        .artists
                        .search_albums_for("monochrome", q, page, &ArtistCredentials::default())
                        .await?;
                    for album in albums.items.into_iter().take(limit) {
                        out.results.push(DiscoveryResult {
                            id: format!("monochrome:{}", album.release.id),
                            kind: "album".into(),
                            source_id: source_id.into(),
                            title: album.title.clone(),
                            subtitle: album.credits.first().map(|credit| credit.name.clone()),
                            url: Some(album.source_url.clone()),
                            artwork_url: album.artwork_urls.first().cloned(),
                            track: None,
                            data: serde_json::to_value(album).unwrap_or(Value::Null),
                        });
                    }
                    out.warnings.extend(albums.warnings);
                }
                if kind_ok(&request, "track") {
                    let search_limit = limit.min(crate::monochrome::INTERACTIVE_SEARCH_LIMIT);
                    let (tracks, raw_count) = self.search_monochrome(q, search_limit).await?;
                    // The public API provides no total count or continuation
                    // cursor. Mark a full result batch incomplete without
                    // inventing an offset that the endpoint ignores.
                    note_monochrome_search_coverage(&mut out, raw_count, search_limit);
                    for (track, _) in tracks.into_iter().take(limit) {
                        let track = self.store.cache_track(&track)?;
                        out.results.push(track_result(source_id, track));
                    }
                }
            }
            "bandcamp" => {
                if kind_ok(&request, "track") {
                    let tracks = crate::providers::bandcamp::Bandcamp::new()
                        .search_tracks(q)
                        .await?;
                    for track in tracks.into_iter().take(limit) {
                        let track = self.store.cache_track(&track)?;
                        out.results.push(track_result(source_id, track));
                    }
                }
            }
            "library" => {
                if kind_ok(&request, "track") {
                    let (tracks, next) = library_search_page(&self.store, q, page, limit)?;
                    out.next_cursor = next;
                    out.results = tracks
                        .into_iter()
                        .map(|track| track_result(source_id, track))
                        .collect();
                }
            }
            "musicbrainz" => {
                if kind_ok(&request, "artist") {
                    let p = self
                        .artists
                        .search_artists_for(
                            "musicbrainz",
                            q,
                            page,
                            limit.min(25) as u32,
                            &ArtistCredentials::default(),
                        )
                        .await?;
                    for a in p
                        .results
                        .into_iter()
                        .filter(|a| a.artist.provider == "musicbrainz")
                    {
                        let id = format!("musicbrainz:{}", a.artist.id);
                        out.results.push(DiscoveryResult {
                            id,
                            kind: "artist".into(),
                            source_id: source_id.into(),
                            title: a.name.clone(),
                            subtitle: a.disambiguation.clone(),
                            url: Some(a.source_url.clone()),
                            artwork_url: a.image_url.clone(),
                            track: None,
                            data: serde_json::to_value(a).unwrap_or(Value::Null),
                        });
                    }
                    out.next_cursor = p.has_more.then(|| (page + 1).to_string());
                    out.warnings = p.warnings;
                }
                if kind_ok(&request, "track") {
                    let (recordings, more) =
                        search_musicbrainz_recordings(q, request.isrc.as_deref(), limit, page)
                            .await?;
                    if more && out.next_cursor.is_none() {
                        out.next_cursor = Some((page + 1).to_string());
                    }
                    for r in recordings {
                        let rid = r["id"].as_str().unwrap_or_default();
                        let title = r["title"].as_str().unwrap_or("Untitled recording");
                        let credits = r["artist-credit"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|x| {
                                x["name"]
                                    .as_str()
                                    .or(x["artist"]["name"].as_str())
                                    .map(str::to_owned)
                            })
                            .collect::<Vec<_>>();
                        let artist = credits.join(", ");
                        let artist_ids=r["artist-credit"].as_array().into_iter().flatten().filter_map(|x|x["artist"]["id"].as_str().map(|id|json!({"provider":"musicbrainz","id":id,"name":x["artist"]["name"]}))).collect::<Vec<_>>();
                        let genre = r["genres"]
                            .as_array()
                            .and_then(|v| v.first())
                            .and_then(|v| v["name"].as_str())
                            .map(str::to_owned);
                        let recording_track = Track {
                            id: format!("musicbrainz:{rid}"),
                            provider: "musicbrainz".into(),
                            provider_id: rid.into(),
                            source_url: format!("https://musicbrainz.org/recording/{rid}"),
                            artist: artist.clone(),
                            title: title.into(),
                            version: r["disambiguation"]
                                .as_str()
                                .filter(|v| !v.trim().is_empty())
                                .map(str::to_owned),
                            isrc: r["isrcs"]
                                .as_array()
                                .and_then(|v| v.first())
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            duration_seconds: r["length"].as_u64().map(|ms| ms as f64 / 1000.0),
                            release_date: r["first-release-date"].as_str().map(str::to_owned),
                            album: r["releases"]
                                .as_array()
                                .and_then(|v| v.first())
                                .and_then(|v| v["title"].as_str())
                                .map(str::to_owned),
                            genre: genre.clone(),
                            normalized_genre: genre,
                            provenance: json!({"catalog":{"provider":"musicbrainz","recordingId":rid,"artistCredit":credits,"artists":artist_ids,"isrcs":r["isrcs"],"verified":true}}),
                            ..Track::default()
                        };
                        out.results.push(DiscoveryResult {
                            id: format!("musicbrainz:{rid}"),
                            kind: "track".into(),
                            source_id: source_id.into(),
                            title: title.into(),
                            subtitle: (!artist.is_empty()).then_some(artist),
                            url: Some(format!("https://musicbrainz.org/recording/{rid}")),
                            artwork_url: None,
                            track: Some(recording_track),
                            data: r.clone(),
                        });
                    }
                }
                if kind_ok(&request, "album") {
                    let a = self
                        .artists
                        .search_albums_for("musicbrainz", q, page, &ArtistCredentials::default())
                        .await?;
                    for r in a
                        .items
                        .into_iter()
                        .filter(|r| r.release.provider == "musicbrainz")
                        .take(limit)
                    {
                        out.results.push(DiscoveryResult {
                            id: format!("musicbrainz:{}", r.release.id),
                            kind: "album".into(),
                            source_id: source_id.into(),
                            title: r.title.clone(),
                            subtitle: r.credits.first().map(|c| c.name.clone()),
                            url: Some(r.source_url.clone()),
                            artwork_url: r.artwork_urls.first().cloned(),
                            track: None,
                            data: serde_json::to_value(r).unwrap_or(Value::Null),
                        });
                    }
                    if out.next_cursor.is_none() {
                        out.next_cursor = a.next_page.map(|n| n.to_string());
                    }
                    out.warnings.extend(a.warnings);
                }
            }
            "discogs" => {
                let settings = self.connections.settings()?;
                if !settings.discogs_enabled {
                    return Err("Enable Discogs in Connections before searching it".into());
                }
                let credentials = ArtistCredentials {
                    discogs_token: self.connections.credential("discogs")?,
                    ..Default::default()
                };
                let artists = kind_ok(&request, "artist");
                let albums = kind_ok(&request, "album");
                if artists {
                    let p = self
                        .artists
                        .search_artists_for("discogs", q, page, limit.min(25) as u32, &credentials)
                        .await?;
                    out.next_cursor = p.has_more.then(|| (page + 1).to_string());
                    for a in p
                        .results
                        .into_iter()
                        .filter(|a| a.artist.provider == "discogs")
                    {
                        out.results.push(DiscoveryResult {
                            id: format!("discogs:{}", a.artist.id),
                            kind: "artist".into(),
                            source_id: source_id.into(),
                            title: a.name.clone(),
                            subtitle: a.disambiguation.clone(),
                            url: Some(a.source_url.clone()),
                            artwork_url: None,
                            track: None,
                            data: serde_json::to_value(a).unwrap_or(Value::Null),
                        });
                    }
                    out.warnings.extend(p.warnings);
                }
                if albums {
                    let p = self
                        .artists
                        .search_albums_for("discogs", q, page, &credentials)
                        .await?;
                    for a in p
                        .items
                        .into_iter()
                        .filter(|a| a.release.provider == "discogs")
                        .take(limit)
                    {
                        out.results.push(DiscoveryResult {
                            id: format!("discogs:{}", a.release.id),
                            kind: "album".into(),
                            source_id: source_id.into(),
                            title: a.title.clone(),
                            subtitle: a.credits.first().map(|c| c.name.clone()),
                            url: Some(a.source_url.clone()),
                            artwork_url: a.artwork_urls.first().cloned(),
                            track: None,
                            data: serde_json::to_value(a).unwrap_or(Value::Null),
                        });
                    }
                    if let Some(n) = p.next_page {
                        out.next_cursor = Some(n.to_string());
                    }
                    out.warnings.extend(p.warnings);
                }
            }
            "youtube" | "soundcloud" => {
                if source_id == "youtube"
                    && request
                        .kinds
                        .iter()
                        .any(|kind| matches!(kind.as_str(), "playlist" | "channel"))
                {
                    let kinds = request
                        .kinds
                        .iter()
                        .filter(|kind| matches!(kind.as_str(), "playlist" | "channel"))
                        .cloned()
                        .collect();
                    let catalog_result = match remote.as_ref() {
                        Some(service) => {
                            service
                                .search_catalog("youtube", q, kinds, request.cursor.as_deref())
                                .await
                        }
                        None => Err("YouTube playlist and channel discovery is unavailable".into()),
                    };
                    match catalog_result {
                        Ok(page) => append_remote_catalog_results(&mut out, "youtube", &page),
                        Err(_error)
                            if kind_ok(&request, "track")
                                || kind_ok(&request, "video")
                                || kind_ok(&request, "set") =>
                        {
                            // A catalog failure must not discard independent native
                            // YouTube video/music results requested in the same search.
                            out.warnings.push(
                                "YouTube playlist/channel search was unavailable; continuing with available results".into(),
                            );
                            out.complete = false;
                            out.progress = Some(
                                "Partial results: YouTube playlist/channel search unavailable"
                                    .into(),
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }
                let settings = self.store.settings().map_err(|e| e.to_string())?;
                let wants_track = kind_ok(&request, "track");
                let wants_video = kind_ok(&request, "video") || kind_ok(&request, "set");
                let remote_page_continuation = request
                    .cursor
                    .as_deref()
                    .is_some_and(|cursor| cursor.starts_with("rp1:"));
                let should_search_raw =
                    (source_id == "youtube" && wants_video) || source_id == "soundcloud";
                let audio_page_offset = audio_offset.unwrap_or(0);
                let audio_page_limit = limit
                    .min(30)
                    .min(10_000usize.saturating_sub(audio_page_offset));
                let raw_search = async {
                    if !should_search_raw
                        || audio_search_done
                        || audio_page_limit == 0
                        || (remote_page_continuation && audio_offset.is_none())
                    {
                        None
                    } else {
                        Some(
                            self.acquisition
                                .search_audio_tracks_page(
                                    q,
                                    source_id,
                                    audio_page_offset,
                                    audio_page_limit,
                                    &settings,
                                )
                                .await,
                        )
                    }
                };
                let should_search_music = should_search_youtube_music(
                    source_id,
                    wants_track,
                    remote_page_continuation,
                    audio_offset,
                    music_continuation.as_ref(),
                    music_searched,
                );
                let music_search = async {
                    if should_search_music {
                        Some(
                            crate::youtube_music::search_page(
                                q,
                                request.isrc.as_deref(),
                                music_continuation.as_ref(),
                                &settings.yt_dlp_path,
                            )
                            .await,
                        )
                    } else {
                        None
                    }
                };
                let (raw_page, music) = tokio::join!(raw_search, music_search);
                let mut next_audio_offset = None;
                let rows = match raw_page {
                    None => vec![],
                    Some(Ok(page)) => {
                        if page.entry_count == audio_page_limit
                            && audio_page_offset + page.entry_count < 10_000
                        {
                            next_audio_offset = Some(audio_page_offset + page.entry_count);
                        }
                        page.tracks
                    }
                    Some(Err(error)) => {
                        out.warnings.push(error);
                        vec![]
                    }
                };
                let raw_was_requested = should_search_raw
                    && !audio_search_done
                    && audio_page_limit > 0
                    && !(remote_page_continuation && audio_offset.is_none());
                if raw_was_requested {
                    audio_search_done = next_audio_offset.is_none();
                }
                for t in rows {
                    let mut row = track_result(source_id, t);
                    if source_id == "youtube" {
                        row.kind = "video".into();
                    } else if kind_ok(&request, "video") && !kind_ok(&request, "track") {
                        row.kind = "video".into();
                    }
                    if kind_ok(&request, &row.kind) {
                        out.results.push(row);
                    }
                }
                let mut next_music_continuation = None;
                if let Some(music) = music {
                    music_searched = true;
                    let music = match music {
                        Ok(page) => page,
                        Err(error) => {
                            out.warnings.push(error);
                            crate::youtube_music::YouTubeMusicPage {
                                items: vec![],
                                continuation: None,
                                warnings: vec![],
                                incomplete: true,
                            }
                        }
                    };
                    out.warnings.extend(music.warnings);
                    if music.incomplete {
                        out.complete = false;
                        out.progress = Some(
                            "Partial results: YouTube Music page could not be completed".into(),
                        );
                    }
                    next_music_continuation = music.continuation;
                    for r in take_unseen_music_items(music.items, &mut seen_music_ids) {
                        let source_metadata = json!({"provider":"youtube","catalogVerified":false,"artist":r.artist,"song":r.title,"album":r.album,"sourceGenres":[]});
                        let candidate = json!({"id":r.id,"provider":"youtube","url":r.url,"title":r.title,"uploader":r.artist,"durationSeconds":r.duration_seconds,"formats":[],"permission":"unknown","confidence":"review","score":0.0,"reasons":["YouTube Music result; source identity is unverified"],"official":false,"sourceMetadata":source_metadata});
                        let track = Track {
                            id: format!("youtube:{}", r.id),
                            provider: "youtube".into(),
                            provider_id: r.id.clone(),
                            source_url: r.url.clone(),
                            title: r.title.clone(),
                            artist: r.artist.clone().unwrap_or_default(),
                            album: r.album.clone(),
                            isrc: r.isrc.clone(),
                            artwork_url: r.artwork_url.clone(),
                            duration_seconds: r.duration_seconds,
                            provenance: json!({"source":{"provider":"youtube","id":r.id,"url":r.url,"catalogVerified":false},"sourceMetadata":source_metadata,"audioCandidate":candidate}),
                            ..Track::default()
                        };
                        out.results.push(DiscoveryResult {
                            id: format!("youtube:{}", r.id),
                            kind: "track".into(),
                            source_id: source_id.into(),
                            title: r.title.clone(),
                            subtitle: r.artist.clone(),
                            url: Some(r.url.clone()),
                            artwork_url: r.artwork_url.clone(),
                            track: Some(track),
                            data: json!({"youtubeMusic":r,"candidate":candidate}),
                        });
                    }
                }
                if next_music_continuation
                    .as_ref()
                    .is_some_and(|next| !valid_youtube_music_continuation(next))
                {
                    next_music_continuation = None;
                    out.complete = false;
                    out.progress = Some(
                        "Partial results: YouTube Music returned an unusable continuation token"
                            .into(),
                    );
                    out.warnings.push(
                        "YouTube Music continuation token exceeded the safe cursor size; further pages were not requested".into(),
                    );
                }
                if next_music_continuation.as_ref().is_some_and(|next| {
                    seen_music_tokens.contains(&music_token_fingerprint(&next.token))
                }) {
                    next_music_continuation = None;
                    out.complete = false;
                    out.progress =
                        Some("Partial results: YouTube Music repeated a continuation token".into());
                    out.warnings.push(
                        "YouTube Music repeated a continuation token; stopped paging safely".into(),
                    );
                }
                if let Some(next) = next_music_continuation.as_ref() {
                    // Keep a bounded recent-token window to detect loops without
                    // imposing a low total-page ceiling on legitimate searches.
                    if seen_music_tokens.len() >= 32 {
                        seen_music_tokens.remove(0);
                    }
                    seen_music_tokens.push(music_token_fingerprint(&next.token));
                }
                if next_audio_offset.is_some()
                    || next_music_continuation.is_some()
                    || out.next_cursor.is_some()
                {
                    out.next_cursor = Some(encode_audio_page_cursor(&AudioPageCursor {
                        source_id: registered_id.into(),
                        query: q.into(),
                        kinds: request.kinds.clone(),
                        isrc: request.isrc.clone(),
                        offset: next_audio_offset.unwrap_or(audio_page_offset),
                        remote_cursor: out.next_cursor.clone(),
                        music_continuation: next_music_continuation,
                        music_searched,
                        audio_search_done,
                        seen_music_tokens,
                        seen_music_ids,
                        complete: out.complete,
                        progress: out.progress.clone(),
                        warnings: out.warnings.clone(),
                    })?);
                }
            }
            "slskd" | "soulseek" => {
                let id = self.acquisition.start_slskd_search(q).await?;
                out.source_id = "soulseek".into();
                out.complete = false;
                out.progress = Some("searching".into());
                out.next_cursor = Some(encode_slskd_discovery_cursor(&SlskdDiscoveryCursor {
                    search_id: id,
                    offset: 0,
                    kinds: request.kinds.clone(),
                })?);
            }
            _ => return Err(format!("Unknown discovery source: {source_id}")),
        }
        let mut positions = std::collections::HashMap::<String, usize>::new();
        let mut unique: Vec<DiscoveryResult> = Vec::with_capacity(out.results.len());
        for item in out.results.drain(..) {
            if let Some(position) = positions.get(&item.id).copied() {
                if unique[position].data.get("candidate").is_none()
                    && item.data.get("candidate").is_some()
                {
                    unique[position] = item;
                }
            } else {
                positions.insert(item.id.clone(), unique.len());
                unique.push(item);
            }
        }
        out.results = unique;
        if source_id == "youtube" {
            let wants_video = kind_ok(&request, "video") || kind_ok(&request, "set");
            let wants_track = kind_ok(&request, "track");
            let prefer_video = wants_video && !wants_track;
            let prefer_track = wants_track && !wants_video;
            rank_youtube_results(&mut out.results, q, prefer_video, prefer_track);
        }
        if matches!(registered_id, "slskd" | "soulseek") {
            return Ok(out);
        }
        cap_discovery_page(
            out,
            buffered,
            registered_id,
            q,
            &request.kinds,
            request.isrc.as_deref(),
            limit,
        )
    }
    pub async fn poll_slskd(&self, search_id: &str) -> Result<DiscoveryPage, String> {
        // Legacy poll callers only requested file/track rows. New typed
        // discovery cursors preserve their own requested kinds.
        self.poll_slskd_with_kinds(search_id, &["track".into()])
            .await
    }
    async fn poll_slskd_with_kinds(
        &self,
        cursor: &str,
        requested_kinds: &[String],
    ) -> Result<DiscoveryPage, String> {
        let decoded = decode_slskd_discovery_cursor(cursor);
        if cursor.starts_with("ssdk1:") && decoded.is_none() {
            return Err("Invalid Soulseek discovery cursor".into());
        }
        let (id, offset, kinds) = if let Some(state) = decoded {
            (state.search_id, state.offset, state.kinds)
        } else if let Some((id, offset)) = cursor.split_once('@') {
            (
                id.to_owned(),
                offset
                    .parse::<usize>()
                    .map_err(|_| "Invalid Soulseek result cursor")?,
                requested_kinds.to_vec(),
            )
        } else {
            (cursor.to_owned(), 0, requested_kinds.to_vec())
        };
        if offset > 10_000 {
            return Err("Soulseek result cursor is out of range".into());
        }
        let poll: SlskdSearchPoll = self.acquisition.poll_slskd_search(&id).await?;
        let page = slskd_page(poll, &id, offset, &kinds)?;
        for row in &page.results {
            if let Some(track) = &row.track {
                self.store.cache_track(track)?;
            }
        }
        Ok(page)
    }
    pub async fn enrich_source_track(&self, mut track: Track) -> Result<Track, String> {
        if !matches!(track.provider.as_str(), "youtube" | "soundcloud") {
            return Err("Source metadata enrichment supports YouTube and SoundCloud URLs".into());
        }
        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let metadata = crate::youtube_music::inspect_url_with_runtime(
            &track.source_url,
            &settings.yt_dlp_path,
            &settings.js_runtime_path,
        )
        .await?;
        let source_metadata = serde_json::to_value(&metadata).map_err(|e| e.to_string())?;
        track.provenance["sourceMetadata"] = source_metadata.clone();
        if track.artist.is_empty() {
            track.artist = metadata.artist.clone().unwrap_or_default();
        }
        if track.title.is_empty() {
            track.title = metadata
                .song
                .clone()
                .unwrap_or_else(|| metadata.title.clone());
        }
        if track.album.is_none() {
            track.album = metadata.album.clone();
        }
        if track.duration_seconds.is_none() {
            track.duration_seconds = metadata.duration_seconds;
        }
        track.provenance["audioCandidate"] = json!({"id":metadata.id,"provider":track.provider,"url":metadata.url,"title":metadata.title,"uploader":metadata.channel,"durationSeconds":metadata.duration_seconds,"chapters":metadata.chapters,"formats":[],"permission":"unknown","confidence":"review","score":0.0,"reasons":["Source-page metadata is not a catalog identity"],"official":false,"sourceMetadata":source_metadata});
        Ok(track)
    }
    /// MCP-facing aggregate API; each source is isolated so a failing provider
    /// produces its own warning page while other configured sources complete.
    pub async fn async_shared_search(&self, request: DiscoveryRequest) -> Vec<DiscoveryPage> {
        let remote = self.remote_service();
        let sources = if request.source_ids.is_empty() {
            source_registry_with_remote(&self.connections, remote.as_deref())
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|source| {
                    source.enabled
                        && source.id != "library"
                        && (request.kinds.is_empty()
                            || source.kinds.iter().any(|kind| request.kinds.contains(kind)))
                })
                .map(|source| source.id)
                .collect::<Vec<_>>()
        } else {
            request.source_ids.clone()
        };
        let pages = futures_util::future::join_all(
            sources
                .iter()
                .map(|source| self.search_source(request.clone(), source)),
        )
        .await;
        pages
            .into_iter()
            .zip(sources)
            .map(|(page, source)| {
                page.unwrap_or_else(|error| DiscoveryPage {
                    source_id: source,
                    results: vec![],
                    next_cursor: None,
                    complete: true,
                    progress: None,
                    warnings: vec![error],
                })
            })
            .collect()
    }
}
fn kind_ok(req: &DiscoveryRequest, k: &str) -> bool {
    req.kinds.is_empty() || req.kinds.iter().any(|x| x == k)
}
fn track_result(source: &str, t: Track) -> DiscoveryResult {
    let id = t.id.clone();
    DiscoveryResult {
        id,
        kind: "track".into(),
        source_id: source.into(),
        title: t.title.clone(),
        subtitle: Some(t.artist.clone()),
        url: Some(t.source_url.clone()),
        artwork_url: t.artwork_url.clone(),
        data: json!({"provenance":t.provenance,"providerId":t.provider_id}),
        track: Some(t),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlskdDiscoveryCursor {
    search_id: String,
    offset: usize,
    #[serde(default)]
    kinds: Vec<String>,
}

fn encode_slskd_discovery_cursor(cursor: &SlskdDiscoveryCursor) -> Result<String, String> {
    let bytes = serde_json::to_vec(cursor).map_err(|error| error.to_string())?;
    Ok(format!(
        "ssdk1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn decode_slskd_discovery_cursor(value: &str) -> Option<SlskdDiscoveryCursor> {
    let encoded = value.strip_prefix("ssdk1:")?;
    if encoded.len() > 16 * 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    let cursor: SlskdDiscoveryCursor = serde_json::from_slice(&bytes).ok()?;
    if cursor.search_id.is_empty() || cursor.offset > 10_000 || cursor.kinds.len() > 10 {
        return None;
    }
    Some(cursor)
}

#[derive(Default)]
struct SoulseekFolderGroup {
    peer: String,
    directory: String,
    files: usize,
    artist_names: std::collections::BTreeSet<String>,
}

fn soulseek_group_identity(kind: &str, peer: &str, value: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{peer}\0{value}").as_bytes());
    let suffix = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("soulseek:{kind}:{suffix}")
}

fn soulseek_folder_groups(tracks: &[Track]) -> Vec<SoulseekFolderGroup> {
    let mut groups = std::collections::BTreeMap::<(String, String), SoulseekFolderGroup>::new();
    for track in tracks {
        let candidate = &track.provenance["audioCandidate"];
        let source = &candidate["source"];
        if source["kind"] != "soulseek" {
            continue;
        }
        let Some(peer) = source["username"]
            .as_str()
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(filename) = source["filename"]
            .as_str()
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some((directory, _)) = filename.rsplit_once(['/', '\\']) else {
            continue;
        };
        if directory.is_empty() {
            continue;
        }
        let key = (peer.to_owned(), directory.to_owned());
        let group = groups.entry(key).or_insert_with(|| SoulseekFolderGroup {
            peer: peer.to_owned(),
            directory: directory.to_owned(),
            ..Default::default()
        });
        group.files += 1;
        if let Some(name) = candidate["sourceMetadata"]["filenameObservations"]["folderArtist"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            group.artist_names.insert(name.to_owned());
        }
    }
    groups.into_values().collect()
}

fn slskd_group_results(groups: &[SoulseekFolderGroup], kinds: &[String]) -> Vec<DiscoveryResult> {
    let wants = |kind: &str| kinds.is_empty() || kinds.iter().any(|requested| requested == kind);
    let mut results = Vec::new();
    if wants("album") {
        for group in groups {
            let folder_name = group
                .directory
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&group.directory);
            let inferred_artist = (group.artist_names.len() == 1)
                .then(|| group.artist_names.iter().next().cloned())
                .flatten();
            let identity = soulseek_group_identity("folder", &group.peer, &group.directory);
            results.push(DiscoveryResult {
                id: identity.clone(), kind: "album".into(), source_id: "soulseek".into(),
                title: folder_name.into(), subtitle: inferred_artist.clone(), url: None, artwork_url: None, track: None,
                data: json!({"soulseekGroup": {"kind":"peerFolder","identity":identity,
                    "peer":group.peer,"directory":group.directory,"folderName":folder_name,
                    "inferredArtist":inferred_artist,"artistEvidence":inferred_artist.as_ref().map(|_|"filenameObservations.folderArtist"),
                    "observedFileCount":group.files,"completeness":"unknown"}}),
            });
        }
    }
    if wants("artist") {
        let mut artists =
            std::collections::BTreeMap::<(String, String), Vec<&SoulseekFolderGroup>>::new();
        for group in groups {
            if group.artist_names.len() != 1 {
                continue;
            }
            let name = group.artist_names.iter().next().unwrap();
            artists
                .entry((group.peer.clone(), name.clone()))
                .or_default()
                .push(group);
        }
        for ((peer, name), mut folders) in artists {
            folders.sort_by(|left, right| left.directory.cmp(&right.directory));
            let folder_rows = folders.iter().map(|group| {
                json!({"identity":soulseek_group_identity("folder", &group.peer, &group.directory),
                    "peer":group.peer,"directory":group.directory,
                    "folderName":group.directory.rsplit(['/', '\\']).next().unwrap_or(&group.directory),
                    "inferredArtist":name,"observedFileCount":group.files,"completeness":"unknown"})
            }).collect::<Vec<_>>();
            let identity = soulseek_group_identity("artist", &peer, &name);
            results.push(DiscoveryResult {
                id: identity.clone(),
                kind: "artist".into(),
                source_id: "soulseek".into(),
                title: name.clone(),
                subtitle: Some("Inferred from peer filename metadata".into()),
                url: None,
                artwork_url: None,
                track: None,
                data: json!({"soulseekArtistGroup":{"kind":"peerArtist","identity":identity,
                    "peer":peer,"inferredName":name,"evidence":"filenameObservations.folderArtist",
                    "folderCount":folder_rows.len(),"folders":folder_rows,
                    "completeness":"unknown"}}),
            });
        }
    }
    results
}

fn slskd_page(
    p: SlskdSearchPoll,
    search_id: &str,
    offset: usize,
    kinds: &[String],
) -> Result<DiscoveryPage, String> {
    let groups = soulseek_folder_groups(&p.tracks);
    let wants_files = kinds.is_empty()
        || kinds
            .iter()
            .any(|kind| matches!(kind.as_str(), "track" | "file"));
    let mut all_results = slskd_group_results(&groups, kinds);
    if wants_files {
        all_results.extend(
            p.tracks
                .into_iter()
                .map(|track| track_result("soulseek", track)),
        );
    }
    let total = all_results.len();
    let results = all_results
        .into_iter()
        .skip(offset)
        .take(MAX_PAGE)
        .collect();
    let next_cursor = if !p.complete {
        Some(encode_slskd_discovery_cursor(&SlskdDiscoveryCursor {
            search_id: search_id.into(),
            offset: 0,
            kinds: kinds.to_vec(),
        })?)
    } else if offset.saturating_add(MAX_PAGE) < total {
        Some(encode_slskd_discovery_cursor(&SlskdDiscoveryCursor {
            search_id: search_id.into(),
            offset: offset + MAX_PAGE,
            kinds: kinds.to_vec(),
        })?)
    } else {
        None
    };
    Ok(DiscoveryPage {
        source_id: "soulseek".into(),
        results,
        next_cursor,
        complete: p.complete,
        progress: Some(format!("{} · {} results", p.state, total)),
        warnings: p.warnings,
    })
}

fn note_monochrome_search_coverage(page: &mut DiscoveryPage, raw_count: usize, limit: usize) {
    page.warnings.push(
        "Showing a limited set of Monochrome matches. Refine your search to narrow the results."
            .into(),
    );
    if raw_count >= limit {
        page.complete = false;
        page.progress = Some(
            "Showing a limited set of matches. Refine your search to narrow the results.".into(),
        );
    }
}

/// Convenient entry points for Tauri command registration.
pub async fn search_source(
    service: &DiscoveryService,
    query: String,
    source_ids: Vec<String>,
    kinds: Vec<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    isrc: Option<String>,
    search_id: Option<String>,
) -> Result<DiscoveryPage, String> {
    if let Some(id) = search_id {
        return service.poll_slskd(&id).await;
    }
    let source = source_ids.first().cloned().ok_or("Select a source")?;
    service
        .search_source(
            DiscoveryRequest {
                query,
                source_ids,
                kinds,
                cursor,
                limit,
                isrc,
            },
            &source,
        )
        .await
}
pub async fn poll_search_source(
    service: &DiscoveryService,
    search_id: &str,
    _source_id: &str,
) -> Result<DiscoveryPage, String> {
    service.poll_slskd(search_id).await
}
pub async fn inspect_youtube_source(
    url: &str,
    yt_dlp_path: &str,
) -> Result<crate::youtube_music::YouTubeSourceMetadata, String> {
    crate::youtube_music::inspect_url(url, yt_dlp_path).await
}
pub async fn enrich_search_result(
    service: &DiscoveryService,
    track: Track,
) -> Result<Track, String> {
    service.enrich_source_track(track).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_search_reports_unknown_coverage_without_fabricating_a_cursor() {
        let mut short = DiscoveryPage {
            source_id: "monochrome".into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        note_monochrome_search_coverage(&mut short, 8, 30);
        assert!(short.complete);
        assert!(short.next_cursor.is_none());
        assert!(short.progress.is_none());
        assert!(short
            .warnings
            .iter()
            .any(|warning| warning.contains("limited set of Monochrome matches")));

        let mut full = DiscoveryPage {
            source_id: "monochrome".into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        note_monochrome_search_coverage(&mut full, 30, 30);
        assert!(!full.complete);
        assert!(full.next_cursor.is_none());
        assert!(full
            .progress
            .as_deref()
            .is_some_and(|progress| progress.contains("limited set of matches")));
    }

    #[tokio::test]
    async fn monochrome_search_caches_result_until_favorite_promotes_it() {
        use axum::{extract::Query, routing::get, Json, Router};
        use std::collections::HashMap;

        async fn search(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
            assert_eq!(query.get("q").map(String::as_str), Some("Fixture Song"));
            Json(json!({"tracks":[{
                "trackId":"12345678901234567",
                "title":"Fixture Song",
                "artistNames":["Fixture Artist"],
                "duration":180000,
                "playable":true
            }]}))
        }

        let app = Router::new().route("/search/tracks", get(search));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let store = Arc::new(Store::open(std::path::Path::new(":memory:")).unwrap());
        let connections = Arc::new(ConnectionManager::new(store.clone()));
        let acquisition = Arc::new(AcquisitionManager::new_for_playback_test(
            store.clone(),
            connections.clone(),
        ));
        let service = DiscoveryService::new(
            Arc::new(ArtistService::new(store.clone()).unwrap()),
            acquisition,
            store.clone(),
            connections,
        );
        *service.monochrome_test_base.write().unwrap() = Some(base);

        let page = service
            .search_source(
                DiscoveryRequest {
                    query: "Fixture Song".into(),
                    source_ids: vec!["monochrome".into()],
                    kinds: vec!["track".into()],
                    cursor: None,
                    limit: Some(10),
                    isrc: None,
                },
                "monochrome",
            )
            .await
            .unwrap();
        server.abort();

        let result = page
            .results
            .iter()
            .find(|result| result.id == "monochrome:12345678901234567")
            .and_then(|result| result.track.clone())
            .expect("Monochrome result should include a track");
        assert!(store.get_track(&result.id).unwrap().is_some());
        assert!(store.list_tracks("library", None).unwrap().is_empty());
        assert_eq!(
            store
                .list_tracks_page("library", None, 10, 0, None, None, None, None, None, None, None,)
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            store
                .search_library_tracks(&["Fixture".into()], 10, 0)
                .unwrap()
                .1,
            0
        );

        store.set_favorite(&result, true).unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap()[0].id, result.id);
        assert_eq!(
            store
                .search_library_tracks(&["Fixture".into()], 10, 0)
                .unwrap()
                .1,
            1
        );
    }

    use crate::browser_bridge::ProviderTransport;
    use serde_json::json;
    use std::sync::Arc;

    struct FailingYoutubeCatalogTransport;

    #[async_trait::async_trait]
    impl ProviderTransport for FailingYoutubeCatalogTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }

        async fn provider_operation(
            &self,
            _provider: &str,
            _operation: &str,
            _args: Value,
        ) -> Result<Value, String> {
            Err("browser transport unavailable".into())
        }

        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        ) -> Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn youtube_catalog_failure_preserves_native_results_for_mixed_kind_search() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let yt_dlp = temp.path().join("yt-dlp-fixture");
        std::fs::write(
            &yt_dlp,
            "#!/bin/sh\nprintf '%s\\n' '{\"entries\":[{\"id\":\"fixture1234\",\"title\":\"Artist - Fixture Song\",\"artist\":\"Artist\",\"track\":\"Fixture Song\",\"duration\":180,\"webpage_url\":\"https://www.youtube.com/watch?v=fixture1234\"}]}'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&yt_dlp).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&yt_dlp, permissions).unwrap();

        let store = Arc::new(Store::open(std::path::Path::new(":memory:")).unwrap());
        let mut settings = store.settings().unwrap();
        settings.yt_dlp_path = yt_dlp.to_string_lossy().into_owned();
        store.save_settings(&settings).unwrap();
        let connections = Arc::new(ConnectionManager::new(store.clone()));
        let acquisition = Arc::new(AcquisitionManager::new_for_playback_test(
            store.clone(),
            connections.clone(),
        ));
        let service = DiscoveryService::new(
            Arc::new(ArtistService::new(store.clone()).unwrap()),
            acquisition,
            store.clone(),
            connections,
        );
        service.set_remote_service(Arc::new(
            crate::remote_playlists::RemotePlaylistService::new(
                store,
                Arc::new(FailingYoutubeCatalogTransport),
            ),
        ));

        let page = service
            .search_source(
                DiscoveryRequest {
                    query: "Fixture Song".into(),
                    source_ids: vec!["youtube".into()],
                    kinds: vec![
                        "track".into(),
                        "video".into(),
                        "set".into(),
                        "playlist".into(),
                        "channel".into(),
                    ],
                    cursor: None,
                    limit: Some(25),
                    isrc: None,
                },
                "youtube",
            )
            .await
            .unwrap();

        assert!(page
            .results
            .iter()
            .any(|row| row.id == "youtube:fixture1234"));
        assert!(page
            .warnings
            .iter()
            .any(|warning| warning.contains("continuing with available results")));
        assert!(!page.complete);
        assert!(page
            .progress
            .as_deref()
            .is_some_and(|progress| progress.contains("Partial results")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn youtube_catalog_only_failure_remains_an_error() {
        let store = Arc::new(Store::open(std::path::Path::new(":memory:")).unwrap());
        let connections = Arc::new(ConnectionManager::new(store.clone()));
        let acquisition = Arc::new(AcquisitionManager::new_for_playback_test(
            store.clone(),
            connections.clone(),
        ));
        let service = DiscoveryService::new(
            Arc::new(ArtistService::new(store.clone()).unwrap()),
            acquisition,
            store.clone(),
            connections,
        );
        service.set_remote_service(Arc::new(
            crate::remote_playlists::RemotePlaylistService::new(
                store,
                Arc::new(FailingYoutubeCatalogTransport),
            ),
        ));

        let error = service
            .search_source(
                DiscoveryRequest {
                    query: "playlist title".into(),
                    source_ids: vec!["youtube".into()],
                    kinds: vec!["playlist".into(), "channel".into()],
                    cursor: None,
                    limit: Some(25),
                    isrc: None,
                },
                "youtube",
            )
            .await
            .unwrap_err();
        assert!(error.contains("browser transport unavailable"));
    }

    #[test]
    fn partial_catalog_status_survives_buffered_page_drain_and_legacy_cursors() {
        let rows = ["one", "two", "three"]
            .into_iter()
            .map(|id| DiscoveryResult {
                id: id.into(),
                kind: "video".into(),
                source_id: "youtube".into(),
                title: id.into(),
                subtitle: None,
                url: None,
                artwork_url: None,
                track: None,
                data: Value::Null,
            })
            .collect();
        let first = cap_discovery_page(
            DiscoveryPage {
                source_id: "youtube".into(),
                results: rows,
                next_cursor: None,
                complete: false,
                progress: Some(
                    "Partial results: YouTube playlist/channel search unavailable".into(),
                ),
                warnings: vec![
                    "YouTube playlist/channel search was unavailable; continuing with available results".into(),
                ],
            },
            Vec::new(),
            "youtube",
            "query",
            &["video".into(), "playlist".into()],
            None,
            1,
        )
        .unwrap();
        assert_eq!(first.results.len(), 1);
        assert!(!first.complete);
        let cursor = decode_discovery_cursor(
            first.next_cursor.as_deref().unwrap(),
            "youtube",
            "query",
            &["video".into(), "playlist".into()],
            None,
        )
        .unwrap();

        // This is the buffered-only fast path in search_source: no provider
        // request is made while the remaining page is drained.
        let next = cap_discovery_page(
            DiscoveryPage {
                source_id: "youtube".into(),
                results: Vec::new(),
                next_cursor: cursor.provider_cursor,
                complete: cursor.complete,
                progress: cursor.progress,
                warnings: cursor.warnings,
            },
            cursor.buffered,
            "youtube",
            "query",
            &["video".into(), "playlist".into()],
            None,
            1,
        )
        .unwrap();
        assert_eq!(next.results.len(), 1);
        assert!(!next.complete);
        assert!(next.progress.unwrap().contains("Partial results"));
        assert!(next
            .warnings
            .iter()
            .any(|warning| warning.contains("continuing with available results")));

        // Cursors emitted before partial metadata existed remain complete.
        let legacy = json!({
            "sourceId":"youtube", "query":"query", "kinds":["video"],
            "providerCursor":null, "buffered":[]
        });
        let encoded = format!(
            "dsb1:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&legacy).unwrap())
        );
        let decoded =
            decode_discovery_cursor(&encoded, "youtube", "query", &["video".into()], None).unwrap();
        assert!(decoded.complete);
        assert!(decoded.progress.is_none());
        assert!(decoded.warnings.is_empty());

        let buffered_cursor = DiscoveryBufferCursor {
            source_id: "youtube".into(),
            query: "query".into(),
            kinds: vec!["video".into()],
            isrc: None,
            provider_cursor: Some("provider-page-2".into()),
            buffered: (0..20)
                .map(|index| DiscoveryResult {
                    id: format!("video-{index}"),
                    kind: "video".into(),
                    source_id: "youtube".into(),
                    title: format!("Buffered result {index}"),
                    subtitle: None,
                    url: None,
                    artwork_url: None,
                    track: None,
                    data: Value::Null,
                })
                .collect(),
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let encoded_buffer = encode_discovery_cursor(&buffered_cursor).unwrap();
        assert!(encoded_buffer.len() > 100);
        let decoded_buffer =
            decode_discovery_cursor(&encoded_buffer, "youtube", "query", &["video".into()], None)
                .unwrap();
        assert_eq!(decoded_buffer.buffered.len(), 20);
        assert!(encoded_buffer.len() <= MAX_DISCOVERY_CURSOR_LENGTH);
    }

    #[test]
    fn buffered_discovery_cursor_binds_isrc_and_keeps_legacy_none_compatible() {
        let page = DiscoveryPage {
            source_id: "youtube".into(),
            results: vec![
                DiscoveryResult {
                    id: "one".into(),
                    kind: "track".into(),
                    source_id: "youtube".into(),
                    title: "One".into(),
                    subtitle: None,
                    url: None,
                    artwork_url: None,
                    track: None,
                    data: Value::Null,
                },
                DiscoveryResult {
                    id: "two".into(),
                    kind: "track".into(),
                    source_id: "youtube".into(),
                    title: "Two".into(),
                    subtitle: None,
                    url: None,
                    artwork_url: None,
                    track: None,
                    data: Value::Null,
                },
            ],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let kinds = vec!["track".into()];
        let buffered = cap_discovery_page(
            page,
            vec![],
            "youtube",
            "same query",
            &kinds,
            Some("GRABC1234567"),
            1,
        )
        .unwrap();
        let encoded = buffered.next_cursor.unwrap();
        assert!(decode_discovery_cursor(
            &encoded,
            "youtube",
            "same query",
            &kinds,
            Some("GRABC1234567"),
        )
        .is_ok());
        assert!(decode_discovery_cursor(&encoded, "youtube", "same query", &kinds, None,).is_err());
        assert!(decode_discovery_cursor(
            &encoded,
            "youtube",
            "same query",
            &kinds,
            Some("DIFFERENT"),
        )
        .is_err());

        let legacy_none = json!({
            "sourceId":"youtube", "query":"same query", "kinds":["track"],
            "providerCursor":null, "buffered":[]
        });
        let legacy_encoded = format!(
            "dsb1:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&legacy_none).unwrap())
        );
        assert!(
            decode_discovery_cursor(&legacy_encoded, "youtube", "same query", &kinds, None,)
                .is_ok()
        );
    }

    fn yt_row(kind: &str, title: &str, artist: &str, id: &str) -> DiscoveryResult {
        DiscoveryResult {
            id: id.into(),
            kind: kind.into(),
            source_id: "youtube".into(),
            title: title.into(),
            subtitle: Some(artist.into()),
            url: None,
            artwork_url: None,
            track: Some(Track {
                title: title.into(),
                artist: artist.into(),
                ..Track::default()
            }),
            data: Value::Null,
        }
    }
    #[test]
    fn youtube_video_intent_ranks_matching_video_ahead_of_music_suggestions() {
        let mut rows = vec![
            yt_row("track", "Salsa Na Ma", "Camilo Azuquita", "music"),
            yt_row("video", "Gia Fu Salsa Dance", "Gia Fu", "video"),
        ];
        rank_youtube_results(&mut rows, "Gia Fu salsa", true, false);
        assert_eq!(rows[0].id, "video");
    }
    #[test]
    fn youtube_track_intent_ranks_native_song_search_by_artist_and_title() {
        let mut rows = vec![
            yt_row("track", "Salsa Na Ma", "Camilo Azuquita", "unrelated"),
            yt_row("track", "Mary", "Big Thief", "mary"),
        ];
        rank_youtube_results(&mut rows, "Mary Big Thief", false, true);
        assert_eq!(rows[0].id, "mary");
    }
    #[test]
    fn registry_contains_searchable_catalogs_and_library_only() {
        let store = Arc::new(Store::open(std::path::Path::new(":memory:")).unwrap());
        let connections = ConnectionManager::new(store);
        let registry = source_registry(&connections).unwrap();
        assert_eq!(
            registry
                .iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "library",
                "musicbrainz",
                "discogs",
                "youtube",
                "soundcloud",
                "bandcamp",
                "monochrome",
                "soulseek"
            ]
        );
        assert!(
            !registry
                .iter()
                .find(|source| source.id == "discogs")
                .unwrap()
                .enabled
        );
        let bandcamp = registry
            .iter()
            .find(|source| source.id == "bandcamp")
            .unwrap();
        assert!(bandcamp.enabled);
        assert_eq!(bandcamp.kinds, vec!["track"]);
        assert!(
            !registry
                .iter()
                .find(|source| source.id == "soulseek")
                .unwrap()
                .enabled
        );
        assert_eq!(
            registry
                .iter()
                .find(|source| source.id == "soulseek")
                .unwrap()
                .kinds,
            vec!["artist", "album", "track", "file"]
        );
    }
    #[test]
    fn remote_catalog_tracks_keep_provider_identity_and_opaque_cursor() {
        let mut page = DiscoveryPage {
            source_id: "spotify".into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        append_remote_catalog_results(
            &mut page,
            "spotify",
            &json!({
                "nextCursor":"rp1:opaque",
                "items":[{"id":"0123456789012345678901","kind":"track","title":"Song","subtitle":"Artist","url":"https://open.spotify.com/track/0123456789012345678901","artworkUrl":null,"data":{"artist":"Artist","album":"Album","durationSeconds":180.0,"isrc":"USABC1234567","artistCount":3,"artistRefs":[{"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"Artist"},{"provider":"bad/id","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Invalid"},{"provider":"musicbrainz","id":"cccccccccccccccccccccc","name":"Wrong provider"}]}}]
            }),
        );
        assert_eq!(page.next_cursor.as_deref(), Some("rp1:opaque"));
        let result = &page.results[0];
        assert_eq!(result.id, "spotify:0123456789012345678901");
        assert_eq!(result.kind, "track");
        let track = result.track.as_ref().unwrap();
        assert_eq!(track.provider, "spotify");
        assert_eq!(track.provider_id, "0123456789012345678901");
        assert_eq!(track.isrc.as_deref(), Some("USABC1234567"));
        assert_eq!(track.provenance["catalog"]["artistCount"], json!(3));
        assert_eq!(
            track.provenance["catalog"]["artists"],
            json!([{"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"Artist"}])
        );
    }
    #[test]
    fn normalized_remote_catalog_artist_credits_reach_discovery_provenance() {
        let credit = json!({"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"  Featured Artist  ","authorization":"not-forwarded"});
        let mut raw = json!({"id":"0123456789012345678901","title":"Catalog recording","url":"https://open.spotify.com/track/0123456789012345678901","data":{
            "artist":"Featured Artist, Featured Artist, Unidentified","artistCount":4,
            "artistRefs":[credit.clone(),credit.clone(),{"provider":"musicbrainz","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Wrong provider"},{"provider":"spotify","id":"bad/id","name":"Invalid identity"}]
        }});
        let mut out = DiscoveryPage {
            source_id: "spotify".into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let normalized =
            crate::remote_playlists::normalize_catalog_item(&raw, "spotify", "track").unwrap();
        assert!(!normalized.to_string().contains("not-forwarded"));
        append_remote_catalog_results(&mut out, "spotify", &json!({"items":[normalized]}));
        let catalog = &out.results[0].track.as_ref().unwrap().provenance["catalog"];
        let expected =
            json!({"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"Featured Artist"});
        assert_eq!(catalog["artists"], json!([expected.clone(), expected]));
        assert_eq!(
            catalog["artistCount"],
            json!(4),
            "invalid identities must not shrink named-credit count"
        );
        raw["data"]["artistRefs"] = json!(vec![credit; 101]);
        raw["data"]["artistCount"] = json!(101);
        let normalized =
            crate::remote_playlists::normalize_catalog_item(&raw, "spotify", "track").unwrap();
        append_remote_catalog_results(&mut out, "spotify", &json!({"items":[normalized]}));
        let catalog = &out.results[1].track.as_ref().unwrap().provenance["catalog"];
        assert_eq!(catalog["artists"].as_array().unwrap().len(), 100);
        assert_eq!(catalog["artistCount"], json!(101));
    }
    #[test]
    fn remote_catalog_artist_credit_validation_preserves_duplicates_and_incomplete_counts() {
        let mut page = DiscoveryPage {
            source_id: "spotify".into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let valid = json!({"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"  Credited Artist  "});
        let response = json!({"items":[{"id":"0123456789012345678901","kind":"track","title":"Song","data":{
            "artistCount":5,"artistRefs":[valid.clone(),valid.clone(),
                {"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"   "},
                {"provider":"spotify","id":"invalid/id","name":"Invalid identity"},
                {"provider":"musicbrainz","id":"cccccccccccccccccccccc","name":"Wrong provider"}]
        }}]});
        append_remote_catalog_results(&mut page, "spotify", &response);
        let catalog = &page.results[0].track.as_ref().unwrap().provenance["catalog"];
        assert_eq!(catalog["artistCount"], json!(5));
        let normalized =
            json!({"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"Credited Artist"});
        assert_eq!(catalog["artists"], json!([normalized.clone(), normalized]));
        let bounded = json!({"items":[{"id":"0123456789012345678901","kind":"track","title":"Song","data":{"artistCount":101,"artistRefs":vec![valid;101]}}]});
        append_remote_catalog_results(&mut page, "spotify", &bounded);
        let catalog = &page.results[1].track.as_ref().unwrap().provenance["catalog"];
        assert_eq!(catalog["artists"].as_array().unwrap().len(), 100);
        assert_eq!(
            catalog["artistCount"],
            json!(101),
            "bounded identities must not imply complete credits"
        );
        append_remote_catalog_results(&mut page, "youtube", &response);
        assert!(
            page.results[2].track.as_ref().unwrap().provenance["catalog"]
                .get("artists")
                .is_none()
        );
    }

    #[test]
    fn spotify_artist_search_keeps_provider_artwork_on_discovery_rows() {
        let raw = json!({"id":"aaaaaaaaaaaaaaaaaaaaaa","kind":"artist","title":"Fixture Artist",
            "url":"https://open.spotify.com/artist/aaaaaaaaaaaaaaaaaaaaaa",
            "artworkUrl":"https://i.scdn.co/image/fixture","data":{"genres":["jazz"]}});
        let normalized =
            crate::remote_playlists::normalize_catalog_item(&raw, "spotify", "artist").unwrap();
        let mut page = DiscoveryPage {
            source_id: "spotify".into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        append_remote_catalog_results(&mut page, "spotify", &json!({"items":[normalized]}));
        assert_eq!(page.results.len(), 1);
        assert_eq!(
            page.results[0].artwork_url.as_deref(),
            Some("https://i.scdn.co/image/fixture")
        );
    }

    #[test]
    fn capped_multi_kind_pages_buffer_every_result_before_advancing_provider_cursor() {
        let source = "spotify";
        let query = "sample";
        let kinds = vec!["track".to_owned(), "artist".to_owned(), "album".to_owned()];
        let mut first = DiscoveryPage {
            source_id: source.into(),
            results: (0..5)
                .map(|index| DiscoveryResult {
                    id: format!("row-{index}"),
                    kind: "track".into(),
                    source_id: source.into(),
                    title: format!("Row {index}"),
                    subtitle: None,
                    url: None,
                    artwork_url: None,
                    track: None,
                    data: Value::Null,
                })
                .collect(),
            next_cursor: Some("rp1:next-page".into()),
            complete: true,
            progress: None,
            warnings: vec![],
        };
        first = cap_discovery_page(first, vec![], source, query, &kinds, None, 2).unwrap();
        assert_eq!(
            first
                .results
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["row-0", "row-1"]
        );
        let state = decode_discovery_cursor(
            first.next_cursor.as_deref().unwrap(),
            source,
            query,
            &kinds,
            None,
        )
        .unwrap();
        assert_eq!(
            state
                .buffered
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["row-2", "row-3", "row-4"]
        );
        assert_eq!(state.provider_cursor.as_deref(), Some("rp1:next-page"));

        let second = cap_discovery_page(
            DiscoveryPage {
                source_id: source.into(),
                results: vec![],
                next_cursor: state.provider_cursor.clone(),
                complete: true,
                progress: None,
                warnings: vec![],
            },
            state.buffered,
            source,
            query,
            &kinds,
            None,
            2,
        )
        .unwrap();
        assert_eq!(
            second
                .results
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["row-2", "row-3"]
        );
        let state = decode_discovery_cursor(
            second.next_cursor.as_deref().unwrap(),
            source,
            query,
            &kinds,
            None,
        )
        .unwrap();
        let last = cap_discovery_page(
            DiscoveryPage {
                source_id: source.into(),
                results: vec![],
                next_cursor: state.provider_cursor.clone(),
                complete: true,
                progress: None,
                warnings: vec![],
            },
            state.buffered,
            source,
            query,
            &kinds,
            None,
            2,
        )
        .unwrap();
        assert_eq!(
            last.results
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["row-4"]
        );
        assert_eq!(last.next_cursor.as_deref(), Some("rp1:next-page"));
    }

    #[test]
    fn audio_search_cursor_binds_provider_query_kinds_and_offset() {
        let kinds = vec!["video".to_owned()];
        let cursor = AudioPageCursor {
            source_id: "youtube".into(),
            query: "night drive".into(),
            kinds: kinds.clone(),
            isrc: None,
            offset: 30,
            remote_cursor: Some("rp1:catalog-next".into()),
            music_continuation: None,
            music_searched: true,
            audio_search_done: false,
            seen_music_tokens: vec![],
            seen_music_ids: vec![],
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let encoded = encode_audio_page_cursor(&cursor).unwrap();
        assert!(encoded.starts_with("ap1:"));
        assert_eq!(
            decode_audio_page_cursor(&encoded, "youtube", "night drive", &kinds, None)
                .unwrap()
                .offset,
            30
        );
        assert!(
            decode_audio_page_cursor(&encoded, "soundcloud", "night drive", &kinds, None).is_err()
        );
        assert!(
            decode_audio_page_cursor(&encoded, "youtube", "other query", &kinds, None).is_err()
        );
        assert!(decode_audio_page_cursor(
            &encoded,
            "youtube",
            "night drive",
            &["track".into()],
            None
        )
        .is_err());
    }
    #[test]
    fn youtube_music_continuation_bounds_match_cursor_limits() {
        use crate::youtube_music::YouTubeMusicContinuation;
        assert!(valid_youtube_music_continuation(
            &YouTubeMusicContinuation {
                token: "provider-token".into(),
                click_tracking_params: Some("click-tracking".into()),
            }
        ));
        assert!(!valid_youtube_music_continuation(
            &YouTubeMusicContinuation {
                token: String::new(),
                click_tracking_params: None,
            }
        ));
        assert!(!valid_youtube_music_continuation(
            &YouTubeMusicContinuation {
                token: "x".repeat(8 * 1024 + 1),
                click_tracking_params: None,
            }
        ));
        assert!(!valid_youtube_music_continuation(
            &YouTubeMusicContinuation {
                token: "provider-token".into(),
                click_tracking_params: Some("x".repeat(2 * 1024 + 1)),
            }
        ));
    }

    #[test]
    fn youtube_music_cursor_binds_token_to_isrc_and_rejects_oversized_token() {
        let kinds = vec!["track".to_owned()];
        let cursor = AudioPageCursor {
            source_id: "youtube".into(),
            query: "poso s'agapo".into(),
            kinds: kinds.clone(),
            isrc: Some("GRABC1234567".into()),
            offset: 0,
            remote_cursor: None,
            music_continuation: Some(crate::youtube_music::YouTubeMusicContinuation {
                token: "opaque-provider-token".into(),
                click_tracking_params: None,
            }),
            music_searched: true,
            audio_search_done: true,
            seen_music_tokens: vec![music_token_fingerprint("older-token")],
            seen_music_ids: vec!["validvideo1".into()],
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let encoded = encode_audio_page_cursor(&cursor).unwrap();
        let decoded = decode_audio_page_cursor(
            &encoded,
            "youtube",
            "poso s'agapo",
            &kinds,
            Some("GRABC1234567"),
        )
        .unwrap();
        assert_eq!(
            decoded.music_continuation.as_ref().unwrap().token,
            "opaque-provider-token"
        );
        assert_eq!(decoded.seen_music_ids, ["validvideo1"]);
        assert!(decode_audio_page_cursor(
            &encoded,
            "youtube",
            "poso s'agapo",
            &kinds,
            Some("OTHER")
        )
        .is_err());
        let oversized = AudioPageCursor {
            music_continuation: Some(crate::youtube_music::YouTubeMusicContinuation {
                token: "x".repeat(8 * 1024 + 1),
                click_tracking_params: None,
            }),
            ..cursor
        };
        let encoded = encode_audio_page_cursor(&oversized).unwrap();
        assert!(decode_audio_page_cursor(
            &encoded,
            "youtube",
            "poso s'agapo",
            &kinds,
            Some("GRABC1234567")
        )
        .is_err());
    }

    #[test]
    fn continuation_page_deduplicates_seen_ids_and_advances_provider_token() {
        let kinds = vec!["track".to_owned()];
        let first_cursor = AudioPageCursor {
            source_id: "youtube".into(),
            query: "Daft Punk".into(),
            kinds: kinds.clone(),
            isrc: None,
            offset: 0,
            remote_cursor: None,
            music_continuation: Some(crate::youtube_music::YouTubeMusicContinuation {
                token: "synthetic-page-2".into(),
                click_tracking_params: Some("synthetic-ctp".into()),
            }),
            music_searched: true,
            audio_search_done: true,
            seen_music_tokens: vec![music_token_fingerprint("synthetic-page-2")],
            seen_music_ids: vec!["duplicate01".into()],
            complete: true,
            progress: None,
            warnings: vec![],
        };
        let encoded = encode_audio_page_cursor(&first_cursor).unwrap();
        let page_two_request =
            decode_audio_page_cursor(&encoded, "youtube", "Daft Punk", &kinds, None).unwrap();
        assert_eq!(
            page_two_request.music_continuation.as_ref().unwrap().token,
            "synthetic-page-2"
        );
        let mut seen_ids = page_two_request.seen_music_ids.clone();
        let fresh = take_unseen_music_items(
            vec![
                crate::youtube_music::YouTubeMusicItem {
                    id: "duplicate01".into(),
                    title: "duplicate".into(),
                    artist: None,
                    album: None,
                    duration_seconds: None,
                    url: "https://music.youtube.com/watch?v=duplicate01".into(),
                    artwork_url: None,
                    isrc: None,
                },
                crate::youtube_music::YouTubeMusicItem {
                    id: "unique00001".into(),
                    title: "unique".into(),
                    artist: None,
                    album: None,
                    duration_seconds: None,
                    url: "https://music.youtube.com/watch?v=unique00001".into(),
                    artwork_url: None,
                    isrc: None,
                },
            ],
            &mut seen_ids,
        );
        assert_eq!(
            fresh
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["unique00001"]
        );
        assert_eq!(seen_ids, ["duplicate01", "unique00001"]);
    }

    #[test]
    fn failed_music_continuation_keeps_mixed_search_on_video_offset() {
        let kinds = vec!["track".to_owned(), "video".to_owned()];
        let cursor = AudioPageCursor {
            source_id: "youtube".into(),
            query: "Daft Punk".into(),
            kinds: kinds.clone(),
            isrc: None,
            offset: 30,
            remote_cursor: None,
            music_continuation: None,
            music_searched: true,
            audio_search_done: false,
            seen_music_tokens: vec![],
            seen_music_ids: vec![],
            complete: false,
            progress: Some("music continuation failed".into()),
            warnings: vec!["YouTube Music continuation request failed".into()],
        };
        let encoded = encode_audio_page_cursor(&cursor).unwrap();
        let state =
            decode_audio_page_cursor(&encoded, "youtube", "Daft Punk", &kinds, None).unwrap();
        assert!(!should_search_youtube_music(
            "youtube",
            true,
            false,
            Some(state.offset),
            state.music_continuation.as_ref(),
            state.music_searched
        ));
        assert_eq!(state.query, "Daft Punk");
        assert_eq!(state.offset, 30);
        assert!(!state.complete);
    }

    #[test]
    fn library_search_finds_imported_remixer_metadata_and_pages_saved_tracks() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        for index in 0..4 {
            store
                .save_track(&Track {
                    id: format!("beatport:{index}"),
                    provider: "beatport".into(),
                    provider_id: index.to_string(),
                    artist: "Original Artist".into(),
                    title: format!("Song {index}"),
                    version: Some("Extended Remix".into()),
                    remixers: Some(vec!["Kerri Chandler".into()]),
                    ..Track::default()
                })
                .unwrap();
        }
        let (first, next) = library_search_page(&store, "kerri remix", 1, 3).unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(next.as_deref(), Some("2"));
        let (last, next) = library_search_page(&store, "kerri remix", 2, 3).unwrap();
        assert_eq!(last.len(), 1);
        assert!(next.is_none());
        assert!(!first.iter().any(|track| track.id == last[0].id));
        assert_eq!(last[0].provider, "beatport");
        assert!(library_search_page(&store, "missing artist", 1, 3)
            .unwrap()
            .0
            .is_empty());
    }
    #[test]
    fn library_search_preserves_unicode_case_folding_and_page_order() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        for (id, title) in [("first", "Björk – Jóga"), ("second", "Other")] {
            store
                .save_track(&Track {
                    id: format!("fixture:{id}"),
                    provider: "fixture".into(),
                    provider_id: id.into(),
                    artist: "ÁRNI".into(),
                    title: title.into(),
                    ..Track::default()
                })
                .unwrap();
        }
        let (page, next) = library_search_page(&store, "árni björk", 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "fixture:first");
        assert!(next.is_none());
    }
    fn poll(complete: bool, count: usize) -> SlskdSearchPoll {
        SlskdSearchPoll {
            id: "search".into(),
            state: if complete { "complete" } else { "searching" }.into(),
            complete,
            warnings: vec![],
            candidates: vec![],
            tracks: (0..count)
                .map(|index| Track {
                    id: format!("track-{index}"),
                    provider_id: format!("track-{index}"),
                    ..Track::default()
                })
                .collect(),
        }
    }
    fn slskd_fixture_track(
        id: &str,
        peer: &str,
        filename: &str,
        folder_artist: Option<&str>,
    ) -> Track {
        Track {
            id: id.into(),
            provenance: json!({"identity":"unverified peer filename","audioCandidate":{
                "id":id,"provider":"slskd","source":{"kind":"soulseek","username":peer,"filename":filename},
                "sourceMetadata":{"filenameObservations":{"folderArtist":folder_artist}}
            }}),
            ..Track::default()
        }
    }
    #[test]
    fn empty_partial_search_keeps_poll_cursor_and_complete_results_page_without_gaps() {
        let kinds = vec!["track".to_owned()];
        let partial = slskd_page(poll(false, 0), "search", 0, &kinds).unwrap();
        assert!(!partial.complete);
        let cursor =
            decode_slskd_discovery_cursor(partial.next_cursor.as_deref().unwrap()).unwrap();
        assert_eq!(cursor.search_id, "search");
        assert_eq!(cursor.kinds, kinds);
        let first = slskd_page(poll(true, 111), "search", 0, &kinds).unwrap();
        let second = slskd_page(poll(true, 111), "search", 50, &kinds).unwrap();
        let last = slskd_page(poll(true, 111), "search", 100, &kinds).unwrap();
        assert_eq!(
            (
                first.results.len(),
                second.results.len(),
                last.results.len()
            ),
            (50, 50, 11)
        );
        assert_eq!(
            decode_slskd_discovery_cursor(first.next_cursor.as_deref().unwrap())
                .unwrap()
                .offset,
            50
        );
        assert_eq!(
            decode_slskd_discovery_cursor(second.next_cursor.as_deref().unwrap())
                .unwrap()
                .offset,
            100
        );
        assert!(last.next_cursor.is_none());
        let ids = first
            .results
            .into_iter()
            .chain(second.results)
            .chain(last.results)
            .map(|row| row.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 111);
    }

    #[test]
    fn soulseek_search_exposes_inferred_peer_folder_album_and_artist_groups() {
        let tracks = vec![
            slskd_fixture_track(
                "one",
                "peer-a",
                "Music/Artist/Album/01.flac",
                Some("Artist"),
            ),
            slskd_fixture_track(
                "two",
                "peer-a",
                "Music/Artist/Album/02.flac",
                Some("Artist"),
            ),
            slskd_fixture_track(
                "three",
                "peer-b",
                "Music/Artist/Album/01.flac",
                Some("Artist"),
            ),
            slskd_fixture_track("unknown", "peer-a", "Loose/unknown.mp3", None),
        ];
        let groups = soulseek_folder_groups(&tracks);
        let rows = slskd_group_results(&groups, &["album".into(), "artist".into()]);
        let albums = rows
            .iter()
            .filter(|row| row.kind == "album")
            .collect::<Vec<_>>();
        let artists = rows
            .iter()
            .filter(|row| row.kind == "artist")
            .collect::<Vec<_>>();
        assert_eq!(albums.len(), 3);
        assert_eq!(artists.len(), 2);
        assert!(albums.iter().all(|row| row.track.is_none()));
        let album = albums
            .iter()
            .find(|row| {
                row.data["soulseekGroup"]["peer"] == "peer-a"
                    && row.data["soulseekGroup"]["directory"] == "Music/Artist/Album"
            })
            .unwrap();
        assert_eq!(album.data["soulseekGroup"]["observedFileCount"], 2);
        assert_eq!(album.data["soulseekGroup"]["completeness"], "unknown");
        assert_eq!(album.data["soulseekGroup"]["inferredArtist"], "Artist");
        assert!(!artists.iter().any(|row| row.title == "unknown"));
    }

    #[test]
    fn soulseek_discovery_cursor_preserves_requested_kinds_and_accepts_legacy_ids() {
        let kinds = vec!["album".into(), "artist".into()];
        let encoded = encode_slskd_discovery_cursor(&SlskdDiscoveryCursor {
            search_id: "search-1".into(),
            offset: 0,
            kinds: kinds.clone(),
        })
        .unwrap();
        let decoded = decode_slskd_discovery_cursor(&encoded).unwrap();
        assert_eq!(decoded.kinds, kinds);
        assert_eq!(decoded.search_id, "search-1");
        assert!(decode_slskd_discovery_cursor("legacy-search").is_none());
        assert_eq!(
            "legacy-search@50"
                .split_once('@')
                .unwrap()
                .1
                .parse::<usize>()
                .unwrap(),
            50
        );
        let groups = soulseek_folder_groups(&[slskd_fixture_track(
            "one",
            "peer-a",
            "Music/Artist/Album/01.flac",
            Some("Artist"),
        )]);
        let rows = slskd_group_results(&groups, &kinds);
        assert!(rows.iter().any(|row| row.kind == "album"));
        assert!(rows.iter().any(|row| row.kind == "artist"));
        assert!(!rows.iter().any(|row| row.kind == "track"));
    }
}
