//! Browser mediated remote playlist access and explicit, conflict checked Spotify sync.
//!
//! Provider credentials remain in the browser tab. This module only sends operations from the
//! fixed `BrowserBridge` allowlist and stores playlist snapshots plus app owned working copies.
use crate::{
    browser_bridge::{
        expand_playlist_occurrences, validate_snapshot, BrowserEntry, BrowserSnapshot,
        ProviderTransport,
    },
    models::{Chart, Track},
    storage::Store,
};
use base64::Engine;
use futures_util::StreamExt;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

type Result<T> = std::result::Result<T, String>;

const ALIAS_HYDRATION_CACHE: &str = "artists.spotify-alias-hydration.v1";
fn alias_hydration_identity(track: &Track) -> Value {
    json!({"id":track.id,"provider":track.provider,"providerId":track.provider_id,"sourceUrl":track.source_url,
        "title":track.title,"artist":track.artist,"album":track.album,"albumArtist":track.album_artist,
        "duration":track.duration_seconds,"isrc":track.isrc,"version":track.version,"range":track.playback_range})
}
pub(crate) fn alias_hydration_key(track: &Track) -> Result<String> {
    let bytes = serde_json::to_vec(&alias_hydration_identity(track)).map_err(|e| e.to_string())?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn merge_alias_hydration(original: &Track, provider: &Track) -> Option<Track> {
    if original.id != provider.id
        || original.provider != provider.provider
        || original.provider_id != provider.provider_id
        || original.source_url != provider.source_url
        || original.title != provider.title
        || original.artist != provider.artist
        || original.duration_seconds != provider.duration_seconds
        || original.isrc.as_ref().is_some_and(|isrc| {
            provider.provenance["remoteCatalog"]["isrc"]
                .as_str()
                .or_else(|| provider.provenance["recordingContext"]["isrc"].as_str())
                .is_none_or(|remote| !crate::recording_identity::same_isrc(isrc, remote))
        })
    {
        return None;
    }
    let mut track = original.clone();
    if !track.provenance.is_object() {
        return None;
    }
    for key in ["catalog", "remoteCatalog", "recordingContext"] {
        if let Some(value) = provider.provenance.get(key) {
            track.provenance[key] = value.clone();
        }
    }
    crate::artist_credit_aliases::spotify_artist_credit_fingerprint(&track)?;
    Some(track)
}

fn alias_hydration_search_query(track: &Track) -> Result<String> {
    let clean = |value: &str| {
        value
            .chars()
            .map(|ch| {
                if ch.is_control() || matches!(ch, '"' | '\\' | '`') {
                    ' '
                } else {
                    ch
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let title = clean(&track.title);
    let artist = clean(&track.artist);
    let query = format!("{title} {artist}");
    if title.is_empty() || artist.is_empty() || query.len() > 256 {
        return Err("Saved Spotify identity cannot form a bounded search query".into());
    }
    Ok(query)
}

fn exact_alias_search_track(
    original: &Track,
    response: &Value,
    expected_account: &str,
) -> Result<Track> {
    if response.get("accountId").and_then(Value::as_str) != Some(expected_account) {
        return Err("Provider account changed during alias hydration search".into());
    }
    let rows = response
        .get("items")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= 50)
        .ok_or("Spotify returned invalid alias search results")?;
    let exact = rows
        .iter()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(original.provider_id.as_str()))
        .collect::<Vec<_>>();
    if exact.len() != 1 {
        return Err("Spotify search did not return exactly one exact track ID".into());
    }
    let row = exact[0];
    if row.get("kind").and_then(Value::as_str) != Some("track") {
        return Err("Spotify search returned the exact ID with the wrong type".into());
    }
    let normalized = normalize_catalog_item(row, "spotify", "track")?;
    let data = normalized
        .get("data")
        .filter(|value| value.is_object())
        .ok_or("Spotify search omitted canonical track metadata")?;
    let title = required_text(data, "title", 512)?;
    let artist = required_text(data, "artist", 1024)?;
    let album = required_text(data, "album", 512)?;
    let duration = data
        .get("durationSeconds")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 86_400.0)
        .ok_or("Spotify search omitted a valid track duration")?;
    if data
        .get("id")
        .or_else(|| data.get("trackId"))
        .and_then(Value::as_str)
        .is_some_and(|id| id != original.provider_id)
    {
        return Err("Spotify search returned inconsistent track IDs".into());
    }
    if normalized["title"].as_str() != Some(original.title.as_str())
        || title != original.title
        || artist != original.artist
        || normalized["subtitle"]
            .as_str()
            .is_some_and(|subtitle| subtitle != original.artist)
        || original
            .album
            .as_deref()
            .is_some_and(|saved| saved != album)
        || original
            .duration_seconds
            .is_none_or(|saved| !saved.is_finite() || saved <= 0.0 || saved != duration)
    {
        return Err("Spotify search metadata disagrees with the saved track".into());
    }
    if let Some(data_url) = data.get("url").and_then(Value::as_str) {
        if data_url != normalized["url"].as_str().unwrap_or_default() {
            return Err("Spotify search returned inconsistent canonical track URLs".into());
        }
    }
    let url = normalized["url"]
        .as_str()
        .ok_or("Spotify search omitted a canonical track URL")?;
    let mut remote = data.clone();
    remote["trackId"] = json!(original.provider_id);
    let refs = remote.get("artistRefs").cloned().unwrap_or(Value::Null);
    let artist_count = remote.get("artistCount").cloned().unwrap_or(Value::Null);
    let mut provider = Track {
        id: format!("spotify:{}", original.provider_id),
        provider: "spotify".into(),
        provider_id: original.provider_id.clone(),
        source_url: url.into(),
        title: title.into(),
        artist: artist.into(),
        album: Some(album.into()),
        duration_seconds: original.duration_seconds,
        artwork_url: data
            .get("artworkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        provenance: json!({
            "providerLookup":"spotifyCatalogSearchExactId",
            "remoteCatalog":remote,
            "catalog":{"verified":true,"provider":"spotify","providerId":original.provider_id,
                "artists":refs,"artistCount":artist_count}
        }),
        ..Track::default()
    };
    crate::recording_identity::hydrate_identifiers(&mut provider);
    merge_alias_hydration(original, &provider).ok_or_else(|| {
        "Exact Spotify search result failed the saved identity and credit checks".to_owned()
    })
}
/// Trusted exact-lookup evidence bound to the original recording identity. Age affects
/// refresh eligibility, while retained evidence remains available to cache reads.
pub(crate) fn read_alias_hydrated_track(store: &Store, original: &Track) -> Option<Track> {
    let payload = store
        .get_provider_cache(ALIAS_HYDRATION_CACHE, &alias_hydration_key(original).ok()?)
        .ok()??;
    let saved_original: Track = serde_json::from_value(payload["original"].clone()).ok()?;
    if payload["version"] != 1
        || alias_hydration_identity(&saved_original) != alias_hydration_identity(original)
    {
        return None;
    }
    let hydrated: Track = serde_json::from_value(payload["hydrated"].clone()).ok()?;
    merge_alias_hydration(original, &hydrated)
}

const VERIFIED_SPOTIFY_TRACK_CONTEXT_CACHE: &str = "artists.spotify-track-context.verified-v1";
const VERIFIED_SPOTIFY_TRACK_CONTEXT_TTL_SECS: u64 = 6 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VerifiedSpotifyTrackContext {
    version: u8,
    status: String,
    pub(crate) track_id: String,
    pub(crate) album_id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) duration_ms: Option<u64>,
    account_id_hash: String,
}

fn valid_cached_spotify_context(value: &VerifiedSpotifyTrackContext, track_id: &str) -> bool {
    value.version == 1
        && value.status == "verified"
        && value.track_id == track_id
        && valid_spotify_id(&value.track_id)
        && valid_spotify_id(&value.album_id)
        && !value.title.trim().is_empty()
        && value.title.len() <= 512
        && value.artist.len() <= 1024
        && value
            .duration_ms
            .is_none_or(|ms| ms > 0 && ms <= 86_400_000)
        && value.account_id_hash.len() == 64
        && value
            .account_id_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

/// Read an exact, recently prepared provider track→album context. This cache is
/// written only after exact getTrack, complete album, and MusicBrainz verification.
pub(crate) fn read_verified_spotify_track_context(
    store: &Store,
    track_id: &str,
) -> Option<VerifiedSpotifyTrackContext> {
    if !valid_spotify_id(track_id) {
        return None;
    }
    let payload = store
        .get_provider_cache(VERIFIED_SPOTIFY_TRACK_CONTEXT_CACHE, track_id)
        .ok()??;
    let value = serde_json::from_value::<VerifiedSpotifyTrackContext>(payload).ok()?;
    valid_cached_spotify_context(&value, track_id).then_some(value)
}

fn write_verified_spotify_track_context(
    store: &Store,
    account_id: &str,
    track_id: &str,
    occurrence: &crate::artists::ReleaseOccurrence,
) -> Result<()> {
    if !valid_spotify_id(track_id)
        || occurrence.spotify_track_id != track_id
        || !valid_spotify_id(&occurrence.spotify_album_id)
        || occurrence.spotify_title.trim().is_empty()
        || occurrence.spotify_title.len() > 512
        || occurrence.spotify_artist.len() > 1024
        || occurrence
            .spotify_duration_ms
            .is_some_and(|ms| ms == 0 || ms > 86_400_000)
        || account_id.is_empty()
    {
        return Err("Verified Spotify track context is invalid".into());
    }
    let digest = Sha256::digest(account_id.as_bytes());
    let account_id_hash = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let value = VerifiedSpotifyTrackContext {
        version: 1,
        status: "verified".into(),
        track_id: track_id.into(),
        album_id: occurrence.spotify_album_id.clone(),
        title: occurrence.spotify_title.clone(),
        artist: occurrence.spotify_artist.clone(),
        duration_ms: occurrence.spotify_duration_ms,
        account_id_hash,
    };
    let payload =
        serde_json::to_value(&value).map_err(|_| "Could not encode verified Spotify context")?;
    let existing = store.get_provider_cache(VERIFIED_SPOTIFY_TRACK_CONTEXT_CACHE, track_id)?;
    let payload = match existing {
        Some(previous) if previous["status"].as_str() == Some("ambiguous") => previous,
        Some(previous) => {
            match serde_json::from_value::<VerifiedSpotifyTrackContext>(previous.clone()) {
                Ok(previous)
                    if valid_cached_spotify_context(&previous, track_id) && previous == value =>
                {
                    payload
                }
                _ => json!({"version":1,"status":"ambiguous"}),
            }
        }
        None => payload,
    };
    store.set_provider_cache(
        VERIFIED_SPOTIFY_TRACK_CONTEXT_CACHE,
        track_id,
        &payload,
        VERIFIED_SPOTIFY_TRACK_CONTEXT_TTL_SECS,
    )
}

async fn await_crosswalk_step<T, F>(
    deadline: tokio::time::Instant,
    cancel: tokio::sync::watch::Receiver<bool>,
    future: F,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    if *cancel.borrow() {
        return Err("Spotify recording verification was canceled".into());
    }
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => {
            Err("Spotify recording verification was canceled".into())
        }
        result = tokio::time::timeout_at(deadline, future) => {
            result.map_err(|_| "Spotify recording verification exceeded its deadline".to_owned())?
        }
    }
}

const PUBLIC_SPOTIFY_EMBED_BODY_LIMIT: usize = 2 * 1024 * 1024;
const PUBLIC_SPOTIFY_EMBED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PUBLIC_SPOTIFY_RATE_LIMIT_DEFAULT_COOLDOWN: std::time::Duration =
    std::time::Duration::from_secs(30);
const PUBLIC_SPOTIFY_RATE_LIMIT_MAX_COOLDOWN_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyDurationRepairResult {
    pub changed: usize,
    pub processed: usize,
    pub complete: bool,
}

#[async_trait::async_trait]
trait PublicSpotifyDurationTransport: Send + Sync {
    async fn duration_seconds(&self, track_id: &str) -> Result<f64>;
}

struct AnonymousSpotifyEmbedTransport;

#[async_trait::async_trait]
impl PublicSpotifyDurationTransport for AnonymousSpotifyEmbedTransport {
    async fn duration_seconds(&self, track_id: &str) -> Result<f64> {
        fetch_anonymous_spotify_embed_duration(track_id).await
    }
}

fn parse_anonymous_spotify_embed_duration(track_id: &str, html: &str) -> Result<f64> {
    if !valid_spotify_id(track_id) || html.len() > PUBLIC_SPOTIFY_EMBED_BODY_LIMIT {
        return Err("PUBLIC_TRACK_INVALID: invalid track ID or response size".into());
    }
    let document = Html::parse_document(html);
    let selector = Selector::parse("script#__NEXT_DATA__")
        .map_err(|_| "PUBLIC_TRACK_INVALID: embed state selector is invalid")?;
    let script = document
        .select(&selector)
        .next()
        .ok_or("PUBLIC_TRACK_INVALID: embed state is missing")?;
    let state = script.text().collect::<String>();
    let data: Value = serde_json::from_str(&state)
        .map_err(|_| "PUBLIC_TRACK_INVALID: embed state is truncated")?;
    let entity = data
        .pointer("/props/pageProps/state/data/entity")
        .ok_or("PUBLIC_TRACK_INVALID: embed entity is missing or schema changed")?;
    let expected_uri = format!("spotify:track:{track_id}");
    if entity.get("type").and_then(Value::as_str) != Some("track")
        || entity.get("id").and_then(Value::as_str) != Some(track_id)
        || entity.get("uri").and_then(Value::as_str) != Some(expected_uri.as_str())
    {
        return Err("PUBLIC_TRACK_MISMATCH: embed entity did not match requested track".into());
    }
    if let Some(duration_ms) = entity
        .get("duration")
        .and_then(Value::as_u64)
        .filter(|duration| (1..=86_400_000).contains(duration))
    {
        // Duration belongs to the exact immutable track ID even if Spotify marks playback
        // unavailable in the current market.
        return Ok(duration_ms as f64 / 1000.0);
    }
    let unavailable_reason = entity
        .get("playabilityReason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.trim().is_empty());
    if entity.get("isPlayable").and_then(Value::as_bool) == Some(false) {
        if unavailable_reason.is_some() {
            return Err("TRACK_NOT_FOUND: Spotify track is unavailable".into());
        }
    }
    Err("PUBLIC_TRACK_INVALID: track duration is missing or out of range".into())
}

async fn collect_bounded_spotify_embed_body<S, B, E>(mut stream: S) -> Result<Vec<u8>>
where
    S: futures_util::Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("PUBLIC_FETCH_FAILED: {error}"))?;
        if bytes.len().saturating_add(chunk.as_ref().len()) > PUBLIC_SPOTIFY_EMBED_BODY_LIMIT {
            return Err("PUBLIC_TRACK_TOO_LARGE: Spotify embed response exceeded 2 MiB".into());
        }
        bytes.extend_from_slice(chunk.as_ref());
    }
    Ok(bytes)
}

fn anonymous_spotify_embed_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(PUBLIC_SPOTIFY_EMBED_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("PUBLIC_FETCH_FAILED: {error}"))
}

async fn fetch_anonymous_spotify_embed_duration(track_id: &str) -> Result<f64> {
    if !valid_spotify_id(track_id) {
        return Err("PUBLIC_TRACK_INVALID: invalid Spotify track ID".into());
    }
    static CLIENT: OnceLock<std::result::Result<reqwest::Client, String>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(anonymous_spotify_embed_client)
        .as_ref()
        .map_err(|error| error.clone())?;
    let url = format!("https://open.spotify.com/embed/track/{track_id}");
    let mut response = tokio::time::timeout(
        PUBLIC_SPOTIFY_EMBED_TIMEOUT,
        client
            .get(url)
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .header(reqwest::header::USER_AGENT, "Burrata/SpotifyPublicMetadata")
            .send(),
    )
    .await
    .map_err(|_| "PUBLIC_FETCH_TIMEOUT: Spotify embed request timed out".to_owned())?
    .map_err(|error| format!("PUBLIC_FETCH_FAILED: {error}"))?;
    let status = response.status();
    if status.as_u16() == 404 {
        return Err("TRACK_NOT_FOUND: Spotify public embed returned 404".into());
    }
    if status.as_u16() == 429 {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|seconds| seconds.clamp(1, PUBLIC_SPOTIFY_RATE_LIMIT_MAX_COOLDOWN_SECS));
        return Err(match retry_after {
            Some(seconds) => format!("RATE_LIMITED: retry after {seconds} seconds"),
            None => "RATE_LIMITED: Spotify public embed returned 429".into(),
        });
    }
    if status.as_u16() == 403 {
        return Err("API_DENIED: Spotify public embed denied this request".into());
    }
    if !status.is_success() {
        return Err(format!(
            "PUBLIC_FETCH_FAILED: Spotify public embed HTTP {}",
            status.as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > PUBLIC_SPOTIFY_EMBED_BODY_LIMIT as u64)
    {
        return Err("PUBLIC_TRACK_TOO_LARGE: Spotify embed response exceeded 2 MiB".into());
    }
    let body_bytes = tokio::time::timeout(
        PUBLIC_SPOTIFY_EMBED_TIMEOUT,
        collect_bounded_spotify_embed_body(response.bytes_stream()),
    )
    .await
    .map_err(|_| "PUBLIC_FETCH_TIMEOUT: Spotify embed body timed out".to_owned())??;
    let body = String::from_utf8(body_bytes)
        .map_err(|_| "PUBLIC_TRACK_INVALID: embed response was not UTF-8".to_owned())?;
    parse_anonymous_spotify_embed_duration(track_id, &body)
}
const STATE_KEY: &str = "remotePlaylists.v1";
const MAX_IMPORT_IDS: usize = 500;
const MAX_PLAYLIST_ENTRIES: usize = 10_000;
const MAX_SPOTIFY_DURATION_BACKFILL: usize = 30;
const SPOTIFY_DURATION_BACKFILL_CONCURRENCY: usize = 5;
const MAX_EXISTING_SPOTIFY_DURATION_BACKFILL: usize = 300;
const EXISTING_SPOTIFY_DURATION_BACKFILL_BATCH: usize = 30;
const EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY: usize = 5;
const EXISTING_SPOTIFY_DURATION_BACKFILL_IDLE_MS: u64 = 1_000;
const EXISTING_SPOTIFY_DURATION_BACKFILL_DEADLINE_SECS: u64 = 60;
const ACCOUNT_FRESH_SECS: i64 = 300;
const REMOTE_BROWSE_PAGE_SIZE: usize = 30;
const MAX_REPORTED_REMOTE_COUNT: u64 = 1_000_000_000_000_000;

fn permanent_duration_lookup_error(error: &str) -> bool {
    matches!(
        error.split_once(':').map(|(code, _)| code.trim()),
        Some(
            "SIGN_IN_REQUIRED"
                | "API_DENIED"
                | "PROVIDER_CHANGED"
                | "PUBLIC_TRACK_INVALID"
                | "PUBLIC_TRACK_MISMATCH"
                | "PUBLIC_TRACK_TOO_LARGE",
        )
    )
}

fn duration_rate_limit_cooldown_seconds(error: &str) -> u64 {
    error
        .split_once(':')
        .map(|(_, detail)| detail.trim())
        .and_then(|detail| detail.strip_prefix("retry after "))
        .and_then(|detail| detail.strip_suffix(" seconds"))
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .map(|seconds| seconds.clamp(1, PUBLIC_SPOTIFY_RATE_LIMIT_MAX_COOLDOWN_SECS))
        .unwrap_or(PUBLIC_SPOTIFY_RATE_LIMIT_DEFAULT_COOLDOWN.as_secs())
}

async fn wait_for_duration_backfill_cancel(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn rate_limited_duration_lookup_error(error: &str) -> bool {
    matches!(
        error.split_once(':').map(|(code, _)| code.trim()),
        Some("RATE_LIMITED")
    )
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
enum DurationBackfillScope {
    Account(String),
    AnonymousPublic,
}

fn existing_duration_cursor_key(account_id: &str) -> String {
    DurationBackfillScope::Account(account_id.to_owned()).cursor_key()
}

impl DurationBackfillScope {
    fn account_id(&self) -> Option<&str> {
        match self {
            Self::Account(account_id) => Some(account_id),
            Self::AnonymousPublic => None,
        }
    }

    fn cursor_key(&self) -> String {
        match self {
            Self::Account(account_id) => {
                let digest = Sha256::digest(account_id.as_bytes());
                format!(
                    "spotifyDurationBackfill.{}",
                    digest
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                )
            }
            Self::AnonymousPublic => "spotifyDurationBackfill.public.v1".into(),
        }
    }
}

#[derive(Clone)]
pub struct RemotePlaylistService {
    store: Arc<Store>,
    bridge: Arc<dyn ProviderTransport>,
    public_spotify_duration: Arc<dyn PublicSpotifyDurationTransport>,
    jobs: Arc<Mutex<HashMap<String, ImportJob>>>,
    cancel: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    account_cache: Arc<Mutex<HashMap<String, CachedAccount>>>,
    duration_backfill_cancel: Arc<Mutex<HashMap<DurationBackfillScope, Arc<AtomicBool>>>>,
    duration_backfill_scheduled: Arc<Mutex<HashSet<DurationBackfillScope>>>,
    duration_backfill_deferred: Arc<Mutex<HashSet<String>>>,
    duration_backfill_commit_gate: Arc<Mutex<()>>,
    public_duration_backoff_until: Arc<Mutex<Option<tokio::time::Instant>>>,
    previews: Arc<Mutex<HashMap<String, StoredPreview>>>,
    track_match_candidates: Arc<Mutex<HashMap<String, CachedTrackMatch>>>,
    preview_apply_claims: Arc<Mutex<HashSet<String>>>,
    change_notifier: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePlaylistWriteCapabilities {
    pub add: bool,
    pub remove: bool,
    pub reorder: bool,
}
fn parse_write_capabilities(metadata: &Value) -> Result<Option<RemotePlaylistWriteCapabilities>> {
    metadata
        .get("writeCapabilities")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| "Provider returned invalid playlist write capabilities".to_owned())
        })
        .transpose()
}
fn deserialize_write_capabilities<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<RemotePlaylistWriteCapabilities>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    RemotePlaylistWriteCapabilities::deserialize(deserializer).map(Some)
}
fn effective_write_capabilities(
    can_edit: bool,
    explicit: Option<RemotePlaylistWriteCapabilities>,
) -> RemotePlaylistWriteCapabilities {
    explicit.unwrap_or(RemotePlaylistWriteCapabilities {
        add: can_edit,
        remove: can_edit,
        reorder: can_edit,
    })
}
fn write_changes_allowed(
    can_edit: bool,
    explicit: Option<RemotePlaylistWriteCapabilities>,
    changes: &RemotePlaylistSyncChanges,
) -> bool {
    let caps = effective_write_capabilities(can_edit, explicit);
    (caps.add || caps.remove || caps.reorder)
        && (changes.add.is_empty() || caps.add)
        && (changes.remove.is_empty() || caps.remove)
        && (changes.move_entries.is_empty() || caps.reorder)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RemoteState {
    #[serde(default)]
    bindings: Vec<RemotePlaylistBindingStatus>,
    #[serde(default)]
    baselines: HashMap<String, RemoteSnapshot>,
    #[serde(default)]
    observed: HashMap<String, RemoteSnapshot>,
    #[serde(default)]
    forgotten_providers: HashSet<String>,
    #[serde(default)]
    allow_write_sync: HashSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistBindingStatus {
    #[serde(
        default,
        deserialize_with = "deserialize_write_capabilities",
        skip_serializing_if = "Option::is_none"
    )]
    pub write_capabilities: Option<RemotePlaylistWriteCapabilities>,
    pub binding_id: String,
    pub provider: String,
    pub playlist_id: String,
    pub title: String,
    pub url: String,
    pub account_id: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub owner_name: Option<String>,
    #[serde(default)]
    pub owner_url: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    pub collection_id: String,
    #[serde(default)]
    pub source_collection_id: Option<String>,
    pub collection_name: String,
    pub track_count: usize,
    pub revision: String,
    pub complete: bool,
    #[serde(default)]
    pub can_edit: bool,
    pub refreshed_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteSnapshot {
    entries: Vec<RemoteEntry>,
    revision: String,
    content_hash: String,
    complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteEntry {
    entry_id: String,
    track_id: String,
    title: String,
    artist: String,
    album: Option<String>,
    duration_seconds: Option<f64>,
    url: String,
    artwork_url: Option<String>,
    available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plays: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    views: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProviderStatus {
    pub provider: String,
    pub connected: bool,
    pub account_id: Option<String>,
    pub display_name: Option<String>,
    pub write_sync_enabled: bool,
    pub checked_at: String,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistItem {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub url: String,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub owner_url: Option<String>,
    pub artwork_url: Option<String>,
    pub track_count: Option<usize>,
    pub visibility: Option<String>,
    pub can_edit: bool,
    pub imported: bool,
    pub binding_id: Option<String>,
    pub collection_id: Option<String>,
}
/// Finite provider consistency modes; this is display metadata, not an import cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemotePlaylistSnapshotConsistency {
    Progressive,
}
// Provider totals fit signed 32-bit scalar fields. They describe the remote result and
// must not be confused with our bounded number of entries fetched/imported.
const MAX_PROVIDER_DISPLAY_COUNT: u64 = i32::MAX as u64;
fn provider_display_count(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_PROVIDER_DISPLAY_COUNT)
        .and_then(|value| usize::try_from(value).ok())
}
fn playlist_snapshot_consistency(
    value: Option<&Value>,
) -> Result<Option<RemotePlaylistSnapshotConsistency>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value == "progressive" => {
            Ok(Some(RemotePlaylistSnapshotConsistency::Progressive))
        }
        _ => Err("Provider returned an invalid playlist snapshot consistency".into()),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistPage {
    pub provider: String,
    pub account_id: Option<String>,
    pub items: Vec<RemotePlaylistItem>,
    pub next_cursor: Option<String>,
    /// Provider-reported total; may refer to the complete profile result, not this page.
    pub total_count: Option<usize>,
    /// True when the provider bounded the result before exhausting the profile.
    pub truncated: bool,
    /// Provider-reported completeness, retained separately from cursor presence.
    pub complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_consistency: Option<RemotePlaylistSnapshotConsistency>,
}
// Finite profile diagnostics: bounded domain-separated ID hashes permit overlap comparison.
// No raw IDs, URLs, provider bodies, account IDs, tokens, or error text are returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpotifyProfileResponseShape {
    version: u8,
    response_mode: SpotifyProfileResponseMode,
    http_status: u16,
    content_type_class: SpotifyProfileContentTypeClass,
    body_bytes: u32,
    parse_class: SpotifyProfileParseClass,
    failure_class: SpotifyProfileFailureClass,
    decoded_rows: u16,
    decoded_total: Option<u32>,
    #[serde(default)]
    playlist_id_hashes: Vec<String>,
    top_level_fields: Vec<SpotifyProfileWireField>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpotifyProfileResponseMode {
    Paged,
    Summary,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SpotifyProfileContentTypeClass {
    Protobuf,
    Json,
    Text,
    Other,
    Missing,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SpotifyProfileParseClass {
    Valid,
    Malformed,
    Oversize,
    NotBinary,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SpotifyProfileFailureClass {
    None,
    ContentType,
    ResponseShape,
    PageCount,
    DuplicateId,
    InvalidId,
    CursorBoundary,
    Identity,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotifyProfileWireField {
    field: u32,
    wire: u8,
    count: u16,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpotifyProfileInspectionRequest {
    pub owner_id: String,
    pub offset: Option<u32>,
    pub limit: Option<u8>,
    pub mode: Option<SpotifyProfileResponseMode>,
    pub playlist_limit: Option<u16>,
}
impl SpotifyProfileInspectionRequest {
    pub(crate) fn summary_mode(&self) -> bool {
        self.mode == Some(SpotifyProfileResponseMode::Summary)
    }
    pub(crate) fn summary_limit(&self) -> Result<u16> {
        if !self.summary_mode() || self.offset.is_some() || self.limit.is_some() {
            return Err("Spotify profile summary bounds are invalid".into());
        }
        let limit = self.playlist_limit.unwrap_or(200);
        if !(1..=200).contains(&limit) {
            return Err("Spotify profile summary bounds are invalid".into());
        }
        Ok(limit)
    }
    pub(crate) fn page_bounds(&self) -> Result<(u32, u8)> {
        if self.summary_mode() || self.playlist_limit.is_some() {
            return Err("Spotify profile inspection page bounds are invalid".into());
        }
        let offset = self.offset.unwrap_or(0);
        let limit = self.limit.unwrap_or(30);
        if offset > 10_000_000 || !(1..=30).contains(&limit) {
            return Err("Spotify profile inspection page bounds are invalid".into());
        }
        Ok((offset, limit))
    }
    pub(crate) fn normalized_owner_id(&self) -> Result<String> {
        let invalid = || "Spotify profile inspection owner is invalid".to_owned();
        if self.owner_id.is_empty()
            || self.owner_id.len() > 2048
            || self.owner_id.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let id = if self.owner_id.contains("://") {
            let parsed = url::Url::parse(&self.owner_id).map_err(|_| invalid())?;
            if parsed.scheme() != "https"
                || parsed.host_str() != Some("open.spotify.com")
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.port().is_some_and(|port| port != 443)
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err(invalid());
            }
            let mut segments = parsed
                .path_segments()
                .ok_or_else(invalid)?
                .collect::<Vec<_>>();
            if segments.last() == Some(&"") {
                segments.pop();
            }
            if segments.len() != 2 || segments[0] != "user" {
                return Err(invalid());
            }
            decode_url_path_segment_once(segments[1]).ok_or_else(invalid)?
        } else {
            self.owner_id.clone()
        };
        if !valid_spotify_profile_id(&id)
            || id.chars().any(char::is_whitespace)
            || id.contains(['?', '#'])
        {
            return Err(invalid());
        }
        Ok(id)
    }
}
fn parse_spotify_profile_response_shape(page: &Value) -> Result<SpotifyProfileResponseShape> {
    let invalid = || "Spotify profile response structure is unavailable or invalid".to_owned();
    let raw = page.get("profileResponseShape").ok_or_else(invalid)?;
    if raw
        .get("topLevelFields")
        .and_then(Value::as_array)
        .is_none_or(|fields| fields.len() > 24)
    {
        return Err(invalid());
    }
    if raw
        .get("playlistIdHashes")
        .is_some_and(|hashes| hashes.as_array().is_none_or(|hashes| hashes.len() > 200))
    {
        return Err(invalid());
    }
    let shape: SpotifyProfileResponseShape =
        serde_json::from_value(raw.clone()).map_err(|_| invalid())?;
    if shape.version != 1
        || !(100..=599).contains(&shape.http_status)
        || shape.body_bytes > 2_097_152
        || shape.decoded_rows > 200
        || shape
            .decoded_total
            .is_some_and(|total| total > i32::MAX as u32)
        || shape.playlist_id_hashes.len()
            > match shape.response_mode {
                SpotifyProfileResponseMode::Paged => 30,
                SpotifyProfileResponseMode::Summary => 200,
            }
        || (!shape.playlist_id_hashes.is_empty()
            && (shape.playlist_id_hashes.len() != usize::from(shape.decoded_rows)
                || !matches!(shape.parse_class, SpotifyProfileParseClass::Valid)))
        || shape.playlist_id_hashes.iter().any(|hash| {
            hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || shape.top_level_fields.iter().any(|field| {
            field.field == 0
                || field.field > 536_870_911
                || field.wire > 5
                || !(1..=10_000).contains(&field.count)
        })
    {
        return Err(invalid());
    }
    Ok(shape)
}

// Inspection DTOs contain fixed field names and bounded numbers/flags only.
// No provider IDs, arbitrary keys, strings, raw data, or cursor values cross this boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpotifyArtistResponseShape {
    version: u8,
    has_data: bool,
    has_graphql_errors: bool,
    lists_array: bool,
    list_count: u16,
    list_kind: SpotifyArtistListKind,
    list_identity_matches: bool,
    page_items_array: bool,
    row_count: u16,
    total_kind: SpotifyArtistTotalKind,
    rows: SpotifyArtistRowChecks,
    first_failed_guard: SpotifyArtistGuard,
    truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SpotifyArtistListKind {
    Absent,
    List,
    Other,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SpotifyArtistTotalKind {
    Missing,
    Null,
    Valid,
    Invalid,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SpotifyArtistGuard {
    None,
    GraphqlErrors,
    ListIdentity,
    PageItems,
    Total,
    PageOverflow,
    RowEntity,
    RowIdentity,
    RowContributors,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpotifyArtistRowChecks {
    entity_wrapper: u16,
    entity_data: u16,
    track_type: u16,
    valid_track_uri: u16,
    matching_data_uri: u16,
    identity: u16,
    valid_name: u16,
    contributors_array: u16,
    valid_contributors: u16,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpotifyArtistInspectionRequest {
    pub artist_id: String,
    pub cursor: Option<String>,
}
impl SpotifyArtistInspectionRequest {
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_spotify_id(&self.artist_id) {
            return Err("Spotify artist inspection request is invalid".into());
        }
        validate_numeric_cursor(self.cursor.as_deref(), 100_000)
    }
}
fn parse_spotify_artist_response_shape(page: &Value) -> Result<SpotifyArtistResponseShape> {
    let invalid = || "Spotify artist response structure is unavailable or invalid".to_owned();
    let shape: SpotifyArtistResponseShape =
        serde_json::from_value(page.get("responseShape").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    fn bounded(value: &Value) -> bool {
        match value {
            Value::Number(number) => number.as_u64().is_some_and(|count| count <= 10_000),
            Value::Bool(_) | Value::String(_) => true,
            Value::Object(object) => object.values().all(bounded),
            _ => false,
        }
    }
    if shape.version != 1 || !bounded(&serde_json::to_value(&shape).map_err(|_| invalid())?) {
        return Err(invalid());
    }
    Ok(shape)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct YouTubePlaylistResponseShape {
    version: u8,
    continuation: bool,
    counts: YouTubeShapeBuckets,
    append_targets: YouTubeAppendTargets,
    recognized_sections: u16,
    recognized_cursor: bool,
    ambiguous: bool,
    truncated: bool,
    membership: YouTubeMembershipBuckets,
    cursor_locations: YouTubeCursorLocations,
    retained_rule_candidate_present: bool,
    retained_rule_equals_selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    playability: Option<YouTubePlayabilityBuckets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_evidence: Option<YouTubePlaylistIdentityEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total_evidence: Option<YouTubePlaylistTotalEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubePlaylistIdentityEvidence {
    target_headers: u16,
    primary_playlist_ids: u16,
    primary_navigation_playlist_ids: u16,
    canonical_playlist_ids: u16,
    edit_endpoint_playlist_ids: u16,
    privacy_field_present: u16,
    private_headers: u16,
    public_headers: u16,
    unlisted_headers: u16,
    unknown_privacy_headers: u16,
    owner_endpoints: u16,
    owner_matches_confirmed_account: u16,
    owner_mismatches: u16,
    sidebar_owner_endpoints: u16,
    sidebar_owner_matches_confirmed_account: u16,
    sidebar_owner_mismatches: u16,
    sidebar_privacy_fields: u16,
    sidebar_private_values: u16,
    sidebar_public_values: u16,
    sidebar_unlisted_values: u16,
    sidebar_unknown_privacy_values: u16,
    privacy_dropdown_entries: u16,
    privacy_dropdown_selected_private: u16,
    privacy_dropdown_selected_public: u16,
    privacy_dropdown_selected_unlisted: u16,
    privacy_dropdown_selected_unknown: u16,
    sidebar_video_owner_nodes: u16,
    sidebar_video_owner_browse_endpoints: u16,
    sidebar_video_owner_matches_confirmed_account: u16,
    sidebar_video_owner_mismatches: u16,
    modern_primary_count: u16,
    modern_secondary_count: u16,
    modern_matching_ids: u16,
    modern_conflicting_ids: u16,
    modern_malformed_edit_ids: u16,
    modern_malformed_title_runs: u16,
    modern_matching_canonicals: u16,
    modern_rejected_canonicals: u16,
    modern_traversal_truncated: u16,
    modern_owner_runs: u16,
    modern_matching_owners: u16,
    modern_conflicting_owners: u16,
    modern_malformed_owner_runs: u16,
    modern_selected_privacy_count: u16,
    modern_target_bound: u16,
    modern_identity_recognized: u16,
    modern_selected_tabs: u16,
    modern_member_sections: u16,
    modern_message_only_section: u16,
    modern_empty_recognized: u16,
    canonical_reject_reason: u16,
    canonical_protocol_class: u16,
    canonical_host_class: u16,
    canonical_has_credentials: u16,
    canonical_has_port: u16,
    canonical_path_class: u16,
    canonical_list_count: u16,
    canonical_list_matches: u16,
    edit_matched_commands: u16,
    edit_foreign_commands: u16,
    edit_malformed_commands: u16,
    edit_api_path_matched: u16,
    edit_params_present: u16,
    edit_actions_missing: u16,
    edit_actions_empty: u16,
    edit_actions_malformed: u16,
    edit_action_add: u16,
    edit_action_remove: u16,
    edit_action_move: u16,
    edit_action_privacy: u16,
    edit_action_metadata: u16,
    edit_action_other: u16,
    edit_action_collection: u16,
    edit_action_collaboration: u16,
    edit_action_voting: u16,
    edit_flag_true: u16,
    edit_flag_false: u16,
    edit_flag_malformed: u16,
    edit_flag_unbound: u16,
    edit_reorder_true: u16,
    edit_traversal_truncated: u16,
    empty_reject_reason: u16,
    empty_continuation_header: u16,
    empty_continuation_sidebar: u16,
    empty_continuation_contents: u16,
    empty_continuation_other: u16,
    empty_membership_nodes: u16,
    empty_traversal_truncated: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubePlaylistTotalEvidence {
    sidebar_primary_info_renderers: u16,
    sidebar_stats_arrays: u16,
    sidebar_first_stat_present: u16,
    sidebar_first_stat_parseable: u16,
    sidebar_first_stat_numeric_zero: u16,
    sidebar_first_stat_english_no_videos: u16,
    sidebar_first_stat_english_locale: bool,
    sidebar_first_stat_text_shape: YouTubeStatTextShape,
    sidebar_first_stat_zero: u16,
    exact_total_recognized: bool,
    exact_total_is_zero: bool,
    truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeStatTextShape {
    string: u16,
    simple_text: u16,
    runs: u16,
    content: u16,
    dynamic_text_view_model: u16,
    text_content: u16,
    accessibility_label: u16,
    other: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubePlayabilityBuckets {
    all_response: YouTubePlayabilityStates,
    recognized_sections: YouTubePlayabilityStates,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubePlayabilityStates {
    explicit_true: u16,
    explicit_false: u16,
    missing: u16,
    invalid: u16,
    with_watch_endpoint: u16,
    known_placeholder: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeMembershipBuckets {
    all_response: YouTubeMembershipCounts,
    recognized_sections: YouTubeMembershipCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeMembershipCounts {
    ordinary_rows: u16,
    recommended_rows: u16,
    unavailable_rows: u16,
    rows_with_occurrence_id: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeCursorLocations {
    named_list_direct: u16,
    named_list_item_section: u16,
    section_list_direct: u16,
    section_list_other_item_section: u16,
    other: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeShapeBuckets {
    contents: YouTubeRendererCounts,
    continuation_contents: YouTubeRendererCounts,
    on_response_received_actions: YouTubeRendererCounts,
    on_response_received_endpoints: YouTubeRendererCounts,
    on_response_received_commands: YouTubeRendererCounts,
    sidebar: YouTubeRendererCounts,
    header: YouTubeRendererCounts,
    other: YouTubeRendererCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeRendererCounts {
    playlist_video_list_renderer: u16,
    playlist_video_list_continuation: u16,
    music_playlist_shelf_renderer: u16,
    music_playlist_shelf_continuation: u16,
    section_list_renderer: u16,
    section_list_continuation: u16,
    item_section_renderer: u16,
    item_section_continuation: u16,
    rich_grid_renderer: u16,
    rich_grid_continuation: u16,
    playlist_video_renderer: u16,
    music_responsive_list_item_renderer: u16,
    lockup_view_model: u16,
    continuation_item_renderer: u16,
    continuation_item_view_model: u16,
    append_continuation_items_action: u16,
    reload_continuation_items_command: u16,
    command_executor_command: u16,
    open_popup_action: u16,
    playlist_header_renderer: u16,
    music_editable_playlist_detail_header_renderer: u16,
    music_playlist_edit_header_renderer: u16,
    music_detail_header_renderer: u16,
    music_responsive_header_renderer: u16,
    playlist_sidebar_renderer: u16,
    playlist_sidebar_primary_info_renderer: u16,
    playlist_sidebar_secondary_info_renderer: u16,
    video_owner: u16,
    video_owner_renderer: u16,
    playlist_metadata_renderer: u16,
    page_header_renderer: u16,
    page_header_view_model: u16,
    playlist_header_view_model: u16,
    owner_endpoint: u16,
    owner_text: u16,
    navigation_endpoint: u16,
    browse_endpoint: u16,
    watch_endpoint: u16,
    canonical_base_url: u16,
    canonical_url: u16,
    url_canonical: u16,
    microformat_data_renderer: u16,
    privacy: u16,
    privacy_status: u16,
    privacy_form: u16,
    dropdown_form_field_renderer: u16,
    dropdown_renderer: u16,
    privacy_dropdown_item_renderer: u16,
    icon: u16,
    edit_endpoint: u16,
    playlist_edit_endpoint: u16,
    playlist_editor_endpoint: u16,
    change_playlist_privacy_endpoint: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YouTubeAppendTargets {
    absent: u16,
    playlist_list: u16,
    requested_playlist: u16,
    other: u16,
    malformed: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct YouTubePlaylistInspectionRequest {
    pub playlist_id: String,
    pub cursor: Option<String>,
}

impl YouTubePlaylistInspectionRequest {
    pub(crate) fn validate(&self) -> Result<()> {
        let normalized = normalize_playlist_reference("youtube", &self.playlist_id)
            .map_err(|_| "YouTube inspection playlist ID is invalid".to_owned())?;
        if normalized != self.playlist_id
            || self.playlist_id.len() > 80
            || self.cursor.as_deref().is_some_and(|cursor| {
                cursor.is_empty() || cursor.len() > 2048 || cursor.chars().any(char::is_control)
            })
        {
            return Err("YouTube inspection request is invalid".into());
        }
        Ok(())
    }
}

/// Fixed own-library structure only; never serialize provider content or identifiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct YouTubeLibraryResponseShape {
    version: u8,
    recognized_containers: u32,
    direct_cursor_items: u32,
    wrapped_cursor_items: u32,
    invalid_cursor_items: u32,
    repeated_cursor_items: u32,
    distinct_tokens: u32,
    unrecognized_continuation_nodes: u32,
    visited_nodes: u32,
    truncated: bool,
    ambiguous: bool,
    terminal_observed: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeLibraryInspection {
    response_shape: YouTubeLibraryResponseShape,
    row_count: usize,
    has_cursor: bool,
    complete: Option<bool>,
    truncated: bool,
}
fn parse_youtube_library_inspection(page: &Value) -> Result<YouTubeLibraryInspection> {
    let invalid = || "YouTube library inspection structure is invalid".to_owned();
    let shape: YouTubeLibraryResponseShape = serde_json::from_value(
        page.get("libraryResponseShape")
            .cloned()
            .ok_or_else(invalid)?,
    )
    .map_err(|_| invalid())?;
    let counts = [
        shape.recognized_containers,
        shape.direct_cursor_items,
        shape.wrapped_cursor_items,
        shape.invalid_cursor_items,
        shape.repeated_cursor_items,
        shape.distinct_tokens,
        shape.unrecognized_continuation_nodes,
    ];
    if shape.version != 1
        || counts.iter().any(|count| *count > 10000)
        || shape.visited_nodes > 30000
    {
        return Err(invalid());
    }
    let rows = page
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if rows.len() > 10000 {
        return Err(invalid());
    }
    let has_cursor = match page.get("nextCursor") {
        Some(Value::Null) => false,
        Some(Value::String(value)) if !value.is_empty() && value.len() <= 100000 => true,
        _ => return Err(invalid()),
    };
    let complete = match page.get("complete") {
        Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        _ => return Err(invalid()),
    };
    let truncated = match page.get("truncated") {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => return Err(invalid()),
    };
    // This helper cannot certify full-library coverage, even at a structural terminal.
    if complete == Some(true)
        || shape.ambiguous && (has_cursor || complete != Some(false) || !truncated)
        || shape.terminal_observed
            && (has_cursor
                || shape.ambiguous
                || shape.truncated
                || shape.unrecognized_continuation_nodes > 0)
    {
        return Err(invalid());
    }
    Ok(YouTubeLibraryInspection {
        response_shape: shape,
        row_count: rows.len(),
        has_cursor,
        complete,
        truncated,
    })
}

fn parse_youtube_playlist_response_shape(
    page: &Value,
    continuation: bool,
) -> Result<YouTubePlaylistResponseShape> {
    let invalid = || "YouTube playlist response structure is unavailable or invalid".to_owned();
    let shape: YouTubePlaylistResponseShape =
        serde_json::from_value(page.get("responseShape").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    fn bounded(value: &Value) -> bool {
        match value {
            Value::Number(number) => number.as_u64().is_some_and(|count| count <= 10_000),
            Value::Bool(_) => true,
            Value::Object(object) => object.values().all(bounded),
            _ => false,
        }
    }
    if shape.identity_evidence.as_ref().is_some_and(|e| {
        e.empty_reject_reason > 10
            || e.canonical_reject_reason > 9
            || e.canonical_protocol_class > 3
            || e.canonical_host_class > 4
            || e.canonical_has_credentials > 1
            || e.canonical_has_port > 1
            || e.canonical_path_class > 3
            || e.canonical_list_matches > 1
    }) {
        return Err(invalid());
    }
    if shape.version != 1
        || shape.continuation != continuation
        || !bounded(&serde_json::to_value(&shape).map_err(|_| invalid())?)
    {
        return Err(invalid());
    }
    Ok(shape)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistBrowsePage {
    #[serde(
        default,
        deserialize_with = "deserialize_write_capabilities",
        skip_serializing_if = "Option::is_none"
    )]
    pub write_capabilities: Option<RemotePlaylistWriteCapabilities>,
    pub provider: String,
    pub playlist_id: String,
    pub url: String,
    pub title: Option<String>,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub owner_url: Option<String>,
    pub visibility: Option<String>,
    pub account_id: Option<String>,
    pub can_edit: bool,
    pub tracks: Vec<Track>,
    pub next_cursor: Option<String>,
    pub total: Option<usize>,
    /// This page reached the provider's terminal result; clients that append
    /// pages must validate the cumulative row count against `total`.
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSpotifyArtist {
    pub id: String,
    pub name: String,
    pub url: String,
    pub artwork_url: Option<String>,
    pub genres: Vec<String>,
    pub popularity: Option<u8>,
    pub followers: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSpotifyAlbum {
    pub id: String,
    pub title: String,
    pub url: String,
    pub artwork_url: Option<String>,
    pub release_date: Option<String>,
    pub album_type: Option<String>,
    pub total_tracks: Option<usize>,
    pub artists: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteArtistBrowsePage {
    pub provider: String,
    pub account_id: String,
    pub artist: RemoteSpotifyArtist,
    pub albums: Vec<RemoteSpotifyAlbum>,
    pub next_cursor: Option<String>,
    pub total_albums: Option<usize>,
    pub top_tracks_unavailable: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteArtistTracksPage {
    pub provider: String,
    pub account_id: String,
    pub artist_id: String,
    pub tracks: Vec<Track>,
    pub next_cursor: Option<String>,
    pub total: Option<usize>,
}

fn parse_spotify_recording_context(value: &Value) -> Result<Option<Value>> {
    let Some(context) = value.get("recordingContext") else {
        return Ok(None);
    };
    if context.is_null() {
        return Ok(None);
    }
    let object = context
        .as_object()
        .ok_or("Spotify returned invalid recording context")?;
    if object.is_empty() || object.len() > 6 {
        return Err("Spotify returned invalid recording context".into());
    }
    if object.keys().any(|key| {
        ![
            "albumId",
            "albumUri",
            "trackNumber",
            "discNumber",
            "isrc",
            "releaseDate",
        ]
        .contains(&key.as_str())
    }) {
        return Err("Spotify returned invalid recording context".into());
    }
    match (context.get("albumId"), context.get("albumUri")) {
        (None, None) => {}
        (Some(id), Some(uri)) => {
            let id = id
                .as_str()
                .filter(|id| valid_spotify_id(id))
                .ok_or("Spotify returned invalid recording album identity")?;
            let expected_uri = format!("spotify:album:{id}");
            if uri.as_str() != Some(expected_uri.as_str()) {
                return Err("Spotify returned conflicting recording album identity".into());
            }
        }
        _ => return Err("Spotify returned incomplete recording album identity".into()),
    }
    for field in ["trackNumber", "discNumber"] {
        if let Some(value) = context.get(field) {
            if !value
                .as_u64()
                .is_some_and(|number| (1..=10_000).contains(&number))
            {
                return Err("Spotify returned invalid recording position metadata".into());
            }
        }
    }
    if let Some(isrc) = context.get("isrc") {
        if !isrc.as_str().is_some_and(|value| {
            value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }) {
            return Err("Spotify returned invalid recording ISRC metadata".into());
        }
    }
    if let Some(date) = context.get("releaseDate") {
        if !date.as_str().is_some_and(|value| {
            value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit())
                || value.len() == 10
                    && value.as_bytes().get(4) == Some(&b'-')
                    && value.as_bytes().get(7) == Some(&b'-')
                    && value
                        .bytes()
                        .enumerate()
                        .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
        }) {
            return Err("Spotify returned invalid recording release metadata".into());
        }
    }
    Ok(Some(context.clone()))
}

fn parse_artist_tracks_page(
    page: &Value,
    artist_id: &str,
    cursor: Option<&str>,
    account_id: &str,
) -> Result<RemoteArtistTracksPage> {
    validate_numeric_cursor(cursor, 100_000)?;
    if page.get("artistId").and_then(Value::as_str) != Some(artist_id) {
        return Err("Spotify returned a different artist".into());
    }
    let offset = cursor
        .unwrap_or("0")
        .parse::<usize>()
        .map_err(|_| "Invalid artist tracks cursor")?;
    let rows = page
        .get("items")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= 30)
        .ok_or("Spotify returned invalid artist tracks")?;
    let next_cursor = match page.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(next)) => {
            validate_numeric_cursor(Some(next), 100_000)?;
            let next_offset = next
                .parse::<usize>()
                .map_err(|_| "Invalid artist tracks cursor")?;
            if rows.is_empty() || next_offset != offset + rows.len() {
                return Err("Spotify returned a nonadvancing artist tracks cursor".into());
            }
            Some(next.clone())
        }
        _ => return Err("Spotify returned invalid artist tracks cursor".into()),
    };
    let tracks = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let position = offset + index;
            let track_id = row
                .get("trackId")
                .and_then(Value::as_str)
                .filter(|id| valid_spotify_id(id))
                .ok_or("Spotify returned an invalid artist track ID")?;
            if let Some(url) = row
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
            {
                if url != format!("https://open.spotify.com/track/{track_id}") {
                    return Err("Spotify returned a URL for a different artist track".into());
                }
            }
            let mut row = row.clone();
            row["entryId"] = json!(format!("artist-{artist_id}-{position}"));
            let entry = parse_remote_entry(&row, "spotify", "artist")?;
            let mut track = entry_to_track("spotify", &entry, position);
            track.provenance["remotePlaylist"] = json!(false);
            track.provenance["artistId"] = json!(artist_id);
            track.provenance["positionScope"] = json!("artist");
            if let Some(context) = parse_spotify_recording_context(&row)? {
                track.provenance["recordingContext"] = context;
            }
            Ok(track)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RemoteArtistTracksPage {
        provider: "spotify".into(),
        account_id: account_id.into(),
        artist_id: artist_id.into(),
        tracks,
        next_cursor,
        total: page
            .get("total")
            .and_then(Value::as_u64)
            .filter(|v| *v <= i32::MAX as u64)
            .map(|v| v as usize),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAlbumTracksPage {
    pub provider: String,
    pub account_id: String,
    pub album: RemoteSpotifyAlbum,
    pub tracks: Vec<Track>,
    pub next_cursor: Option<String>,
    pub total: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistImportItem {
    pub binding_id: String,
    pub collection_id: String,
    pub provider: String,
    pub playlist_id: String,
    pub title: String,
    pub track_count: usize,
    pub revision: String,
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistJobStatus {
    pub job_id: String,
    pub state: String,
    pub completed: usize,
    pub total: usize,
    pub results: Vec<RemotePlaylistImportItem>,
    pub errors: Vec<RemotePlaylistJobError>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistJobError {
    pub playlist_id: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistSyncChanges {
    pub add: Vec<RemoteAddChange>,
    pub remove: Vec<RemoteRemoveChange>,
    #[serde(rename = "move")]
    pub move_entries: Vec<RemoteMoveChange>,
    pub unresolved: Vec<RemoteUnresolvedChange>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAddChange {
    pub track_uri: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRemoveChange {
    pub entry_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMoveChange {
    pub entry_ids: Vec<String>,
    pub before_entry_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUnresolvedChange {
    pub entry_id: Option<String>,
    #[serde(default)]
    pub local_track_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistTrackMatchCandidate {
    pub candidate_token: String,
    pub title: String,
    pub artist: String,
    pub version: Option<String>,
    pub duration_seconds: Option<f64>,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistTrackMatches {
    pub binding_id: String,
    pub local_track_id: String,
    pub provider: String,
    pub candidates: Vec<RemotePlaylistTrackMatchCandidate>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedRemoteTrackMatch {
    pub local_track_id: String,
    pub candidate_token: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylistSyncPreview {
    #[serde(
        default,
        deserialize_with = "deserialize_write_capabilities",
        skip_serializing_if = "Option::is_none"
    )]
    pub write_capabilities: Option<RemotePlaylistWriteCapabilities>,
    pub preview_id: String,
    pub binding_id: String,
    pub provider: String,
    pub playlist_id: String,
    pub local_collection_id: String,
    pub baseline_revision: String,
    pub remote_revision: String,
    pub conflict: bool,
    pub applied: bool,
    pub can_edit: bool,
    pub write_sync_enabled: bool,
    pub changes: RemotePlaylistSyncChanges,
    pub summary: String,
}
#[derive(Clone)]
struct ImportJob {
    status: RemotePlaylistJobStatus,
}
#[derive(Clone)]
struct CachedAccount {
    id: String,
    name: Option<String>,
    checked_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Clone)]
struct StoredPreview {
    preview: RemotePlaylistSyncPreview,
    remote_hash: String,
    account_id: String,
    desired_uris: Vec<String>,
    local_identity_hashes: HashMap<String, String>,
    local_order_hash: String,
}

#[derive(Clone)]
struct CachedTrackMatch {
    binding_id: String,
    provider: String,
    account_id: String,
    local_track_id: String,
    local_identity_hash: String,
    uri: String,
    created_at: std::time::Instant,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogCursor {
    provider: String,
    kinds: Vec<String>,
    #[serde(default)]
    query_hash: String,
    cursors: HashMap<String, String>,
}

struct DurationBackfillLease {
    service: RemotePlaylistService,
    scope: DurationBackfillScope,
    cancel: Arc<AtomicBool>,
}

impl Drop for DurationBackfillLease {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        {
            let Ok(_gate) = self.service.duration_backfill_commit_gate.lock() else {
                return;
            };
            let Ok(mut active) = self.service.duration_backfill_cancel.lock() else {
                return;
            };
            if active
                .get(&self.scope)
                .is_some_and(|current| Arc::ptr_eq(current, &self.cancel))
            {
                active.remove(&self.scope);
            }
        }
        if self.scope == DurationBackfillScope::AnonymousPublic {
            let current_account = self
                .service
                .account_cache
                .lock()
                .ok()
                .and_then(|accounts| accounts.get("spotify").map(|account| account.id.clone()));
            if tokio::runtime::Handle::try_current().is_ok() {
                let should_resume =
                    self.service
                        .duration_backfill_deferred
                        .lock()
                        .is_ok_and(|mut deferred| {
                            let should_resume = current_account
                                .as_ref()
                                .is_some_and(|account_id| deferred.remove(account_id));
                            deferred.clear();
                            should_resume
                        });
                if should_resume {
                    if let Some(account_id) = current_account {
                        self.service
                            .schedule_existing_spotify_duration_backfill(account_id);
                    }
                }
            }
        }
    }
}

struct PreviewApplyClaim {
    binding_id: String,
    claims: Arc<Mutex<HashSet<String>>>,
}

impl Drop for PreviewApplyClaim {
    fn drop(&mut self) {
        if let Ok(mut claims) = self.claims.lock() {
            claims.remove(&self.binding_id);
        }
    }
}

impl RemotePlaylistService {
    pub fn new(store: Arc<Store>, bridge: Arc<dyn ProviderTransport>) -> Self {
        Self::new_with_public_spotify_duration(
            store,
            bridge,
            Arc::new(AnonymousSpotifyEmbedTransport),
        )
    }
    fn new_with_public_spotify_duration(
        store: Arc<Store>,
        bridge: Arc<dyn ProviderTransport>,
        public_spotify_duration: Arc<dyn PublicSpotifyDurationTransport>,
    ) -> Self {
        Self {
            store,
            bridge,
            public_spotify_duration,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            cancel: Arc::new(Mutex::new(HashMap::new())),
            account_cache: Arc::new(Mutex::new(HashMap::new())),
            duration_backfill_cancel: Arc::new(Mutex::new(HashMap::new())),
            duration_backfill_scheduled: Arc::new(Mutex::new(HashSet::new())),
            duration_backfill_deferred: Arc::new(Mutex::new(HashSet::new())),
            duration_backfill_commit_gate: Arc::new(Mutex::new(())),
            public_duration_backoff_until: Arc::new(Mutex::new(None)),
            previews: Arc::new(Mutex::new(HashMap::new())),
            track_match_candidates: Arc::new(Mutex::new(HashMap::new())),
            preview_apply_claims: Arc::new(Mutex::new(HashSet::new())),
            change_notifier: Arc::new(Mutex::new(None)),
        }
    }
    fn claim_preview_apply(&self, binding_id: &str) -> Result<PreviewApplyClaim> {
        if binding_id.is_empty() || binding_id.len() > 256 {
            return Err("Sync binding ID is invalid".into());
        }
        let mut claims = self
            .preview_apply_claims
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?;
        if !claims.insert(binding_id.to_owned()) {
            return Err("A sync preview for this playlist is already being applied".into());
        }
        Ok(PreviewApplyClaim {
            binding_id: binding_id.to_owned(),
            claims: self.preview_apply_claims.clone(),
        })
    }
    pub fn set_change_notifier(&self, notifier: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut current) = self.change_notifier.lock() {
            *current = Some(notifier);
        }
    }
    pub fn browser_connected(&self) -> bool {
        self.bridge.provider_operations_supported()
    }
    pub fn write_sync_enabled(&self, provider: &str) -> Result<bool> {
        validate_provider(provider)?;
        Ok(self.load_state()?.allow_write_sync.contains(provider))
    }
    pub fn set_write_sync_enabled(&self, provider: &str, enabled: bool) -> Result<bool> {
        validate_provider(provider)?;
        if provider != "spotify" && provider != "youtube" && enabled {
            return Err("Write sync is not supported for this provider".into());
        }
        let mut state = self.load_state()?;
        if enabled {
            state.allow_write_sync.insert(provider.into());
        } else {
            state.allow_write_sync.remove(provider);
        }
        self.save_state(&state)?;
        Ok(enabled && provider_supports_write_sync(provider))
    }

    pub async fn connection_status(&self, provider: &str) -> Result<RemoteProviderStatus> {
        validate_provider(provider)?;
        let write_sync_enabled = self.write_sync_enabled(provider)?;
        if self.load_state()?.forgotten_providers.contains(provider) {
            return Ok(RemoteProviderStatus {
                provider: provider.into(),
                connected: false,
                account_id: None,
                display_name: None,
                write_sync_enabled,
                checked_at: chrono::Utc::now().to_rfc3339(),
                error: Some("Connection was forgotten in Burrata".into()),
            });
        }
        let now = chrono::Utc::now();
        let cached_account = {
            let account_cache = self
                .account_cache
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            account_cache.get(provider).cloned()
        };
        if let Some(cached) = cached_account {
            if now.signed_duration_since(cached.checked_at).num_seconds() < ACCOUNT_FRESH_SECS
                && self.bridge.provider_operations_supported()
            {
                if provider == "spotify" {
                    self.schedule_existing_spotify_duration_backfill(cached.id.clone());
                }
                return Ok(RemoteProviderStatus {
                    provider: provider.into(),
                    connected: true,
                    account_id: Some(cached.id),
                    display_name: cached.name,
                    write_sync_enabled,
                    checked_at: cached.checked_at.to_rfc3339(),
                    error: None,
                });
            }
        }
        let checked_at = now.to_rfc3339();
        if !self.bridge.provider_operations_supported() {
            return Ok(RemoteProviderStatus {
                provider: provider.into(),
                connected: false,
                account_id: None,
                display_name: None,
                write_sync_enabled,
                checked_at,
                error: Some("Browser companion is not connected".into()),
            });
        }
        match self
            .bridge
            .provider_operation(provider, "status", json!({}))
            .await
            .and_then(|value| self.optional_result_account(provider, &value))
        {
            Ok(Some(account)) => {
                if provider == "spotify" {
                    self.schedule_existing_spotify_duration_backfill(account.id.clone());
                }
                Ok(RemoteProviderStatus {
                    provider: provider.into(),
                    connected: true,
                    account_id: Some(account.id),
                    display_name: account.name,
                    write_sync_enabled,
                    checked_at: account.checked_at.to_rfc3339(),
                    error: None,
                })
            }
            Ok(None) => Ok(RemoteProviderStatus {
                provider: provider.into(),
                connected: false,
                account_id: None,
                display_name: None,
                write_sync_enabled,
                checked_at,
                error: Some(
                    "No signed-in provider account. Public YouTube playlists remain available."
                        .into(),
                ),
            }),
            Err(error) => Ok(RemoteProviderStatus {
                provider: provider.into(),
                connected: false,
                account_id: None,
                display_name: None,
                write_sync_enabled,
                checked_at,
                error: Some(error),
            }),
        }
    }
    pub async fn connect(&self, provider: &str) -> Result<RemoteProviderStatus> {
        validate_provider(provider)?;
        let value = self
            .bridge
            .provider_operation(provider, "connect", json!({}))
            .await?;
        let account = self.optional_result_account(provider, &value)?;
        let mut state = self.load_state()?;
        state.forgotten_providers.remove(provider);
        self.save_state(&state)?;
        if provider == "spotify" {
            if let Some(account) = account.as_ref() {
                {
                    if let Ok(_commit_gate) = self.duration_backfill_commit_gate.lock() {
                        let running =
                            self.duration_backfill_cancel
                                .lock()
                                .ok()
                                .is_some_and(|active| {
                                    active.contains_key(&DurationBackfillScope::Account(
                                        account.id.clone(),
                                    ))
                                });
                        if !running {
                            if let Ok(mut scheduled) = self.duration_backfill_scheduled.lock() {
                                scheduled
                                    .remove(&DurationBackfillScope::Account(account.id.clone()));
                            }
                        }
                    }
                }
                self.schedule_existing_spotify_duration_backfill(account.id.clone());
            }
        }
        let write_sync_enabled = self.write_sync_enabled(provider)?;
        Ok(RemoteProviderStatus {
            provider: provider.into(),
            connected: account.is_some(),
            account_id: account.as_ref().map(|account| account.id.clone()),
            display_name: account.as_ref().and_then(|account| account.name.clone()),
            write_sync_enabled,
            checked_at: chrono::Utc::now().to_rfc3339(),
            error: if provider == "youtube" && account.is_none() {
                Some(
                    "No signed-in account. Public YouTube playlist browsing remains available."
                        .into(),
                )
            } else {
                None
            },
        })
    }
    /// Forget cached identity for this provider. Provider logout remains an explicit browser action.
    pub fn disconnect(&self, provider: &str) -> Result<RemoteProviderStatus> {
        validate_provider(provider)?;
        self.account_cache
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .remove(provider);
        if provider == "spotify" {
            let _commit_gate = self
                .duration_backfill_commit_gate
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            let mut active = self
                .duration_backfill_cancel
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            for cancel in active.values() {
                cancel.store(true, Ordering::SeqCst);
            }
            active.clear();
            self.duration_backfill_scheduled
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?
                .clear();
        }
        let mut state = self.load_state()?;
        state.forgotten_providers.insert(provider.into());
        self.save_state(&state)?;
        let write_sync_enabled = self.write_sync_enabled(provider)?;
        Ok(RemoteProviderStatus {
            provider: provider.into(),
            connected: false,
            account_id: None,
            display_name: None,
            write_sync_enabled,
            checked_at: chrono::Utc::now().to_rfc3339(),
            error: None,
        })
    }
    pub fn list_bindings(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<RemotePlaylistBindingStatus>> {
        let state = self.load_state()?;
        let collections = self
            .store
            .list_collections()?
            .into_iter()
            .map(|collection| (collection.id, (collection.name, collection.track_count)))
            .collect::<HashMap<_, _>>();
        Ok(state
            .bindings
            .into_iter()
            .filter(|binding| provider.is_none_or(|provider| provider == binding.provider))
            .map(|mut binding| {
                if let Some((name, count)) = collections.get(&binding.collection_id) {
                    binding.collection_name = name.clone();
                    binding.track_count = *count as usize;
                }
                binding
            })
            .collect())
    }
    /// Inspect one bounded Spotify profile page, without returning contents.
    pub async fn inspect_spotify_profile_page(
        &self,
        request: &SpotifyProfileInspectionRequest,
    ) -> Result<SpotifyProfileResponseShape> {
        let owner_id = request.normalized_owner_id()?;
        let args = if request.summary_mode() {
            json!({"ownerId":owner_id,"inspectProfileResponse":true,"profileSummary":true,"playlistLimit":request.summary_limit()?})
        } else {
            let (offset, limit) = request.page_bounds()?;
            json!({"ownerId":owner_id,"offset":offset,"limit":limit,"inspectProfileResponse":true})
        };
        let account = self
            .require_account("spotify")
            .await
            .map_err(|_| "Spotify account is unavailable for profile inspection".to_owned())?;
        let page = self
            .bridge
            .provider_operation("spotify", "listPlaylists", args)
            .await
            .map_err(|_| "Spotify profile inspection failed".to_owned())?;
        if page.get("accountId").and_then(Value::as_str) != Some(account.id.as_str()) {
            return Err("Spotify account changed during profile inspection".into());
        }
        let shape = parse_spotify_profile_response_shape(&page)?;
        let expected_mode = if request.summary_mode() {
            SpotifyProfileResponseMode::Summary
        } else {
            SpotifyProfileResponseMode::Paged
        };
        let limit = if request.summary_mode() {
            request.summary_limit()? as usize
        } else {
            request.page_bounds()?.1 as usize
        };
        if shape.response_mode != expected_mode || usize::from(shape.decoded_rows) > limit {
            return Err("Spotify profile inspection response did not match the request".into());
        }
        Ok(shape)
    }

    /// Inspect only the first confirmed-account own-library page, without returning content.
    pub async fn inspect_youtube_library_page(&self) -> Result<YouTubeLibraryInspection> {
        let account = self
            .require_account("youtube")
            .await
            .map_err(|_| "YouTube account is unavailable for library inspection".to_owned())?;
        let page = self
            .bridge
            .provider_operation(
                "youtube",
                "listPlaylists",
                json!({"limit":50,"inspectLibraryResponseShape":true}),
            )
            .await
            .map_err(|_| "YouTube library inspection failed".to_owned())?;
        if page.get("accountId").and_then(Value::as_str) != Some(account.id.as_str()) {
            return Err("YouTube account changed during library inspection".into());
        }
        parse_youtube_library_inspection(&page)
    }

    /// Finite read-only diagnostics, isolated from ordinary playlist DTOs and UI flows.
    pub async fn inspect_youtube_playlist_page(
        &self,
        playlist_id: &str,
        cursor: Option<&str>,
    ) -> Result<YouTubePlaylistResponseShape> {
        YouTubePlaylistInspectionRequest {
            playlist_id: playlist_id.to_owned(),
            cursor: cursor.map(str::to_owned),
        }
        .validate()?;
        let account = self
            .require_account("youtube")
            .await
            .map_err(|_| "YouTube account is unavailable for inspection".to_owned())?;
        let mut args = json!({"playlistId":playlist_id,"limit":REMOTE_BROWSE_PAGE_SIZE,"inspectResponseShape":true});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let page = self
            .bridge
            .provider_operation("youtube", "getPlaylist", args)
            .await
            .map_err(|_| "YouTube playlist inspection failed".to_owned())?;
        if page.get("accountId").and_then(Value::as_str) != Some(account.id.as_str()) {
            return Err("YouTube account changed during playlist inspection".into());
        }
        if page.pointer("/playlist/id").and_then(Value::as_str) != Some(playlist_id) {
            return Err("YouTube playlist changed during inspection".into());
        }
        parse_youtube_playlist_response_shape(&page, cursor.is_some())
    }

    /// Read one provider playlist page without creating an archive or local collection.
    pub async fn browse_playlist(
        &self,
        provider: &str,
        playlist_reference: &str,
        cursor: Option<&str>,
    ) -> Result<RemotePlaylistBrowsePage> {
        validate_provider(provider)?;
        let playlist_id = normalize_playlist_reference(provider, playlist_reference)?;
        let expected_account = if provider == "spotify" {
            Some(self.require_account(provider).await?.id)
        } else {
            None
        };
        let mut args = json!({"playlistId":playlist_id,"limit":REMOTE_BROWSE_PAGE_SIZE});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let page = self
            .bridge
            .provider_operation(provider, "getPlaylist", args)
            .await?;
        let response_account = self
            .optional_result_account(provider, &page)?
            .map(|account| account.id);
        if let Some(expected) = expected_account.as_deref() {
            if response_account.as_deref() != Some(expected) {
                return Err("Provider account changed or became unavailable while browsing".into());
            }
        }
        let metadata = page
            .get("playlist")
            .and_then(Value::as_object)
            .ok_or("Provider returned invalid playlist details")?;
        if metadata.get("id").and_then(Value::as_str) != Some(playlist_id.as_str()) {
            return Err("Provider returned a different playlist".into());
        }
        let metadata_value = Value::Object(metadata.clone());
        let url = validate_remote_url(required_text(&metadata_value, "url", 2048)?, provider)?;
        if normalize_playlist_reference(provider, &url)? != playlist_id {
            return Err("Provider returned a playlist URL for a different ID".into());
        }
        let owner_id = bounded_optional_text(&metadata_value, "ownerId", 256);
        let owner_name = bounded_optional_text(&metadata_value, "ownerName", 512);
        let owner_url = normalize_owner_url(
            provider,
            owner_id.as_deref(),
            metadata.get("ownerUrl").and_then(Value::as_str),
        );
        let title = bounded_optional_text(&metadata_value, "title", 256);
        let entries = page
            .get("entries")
            .and_then(Value::as_array)
            .ok_or("Provider returned invalid playlist entries")?;
        let tracks = parse_browse_tracks(entries, provider, cursor.unwrap_or("first"))?;
        let next_cursor = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let provider_complete = page
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(next_cursor.is_none());
        let total = page
            .get("total")
            .and_then(Value::as_u64)
            .filter(|total| *total <= MAX_PLAYLIST_ENTRIES as u64)
            .map(|total| total as usize)
            .or_else(|| {
                metadata
                    .get("total")
                    .and_then(Value::as_u64)
                    .filter(|total| *total <= MAX_PLAYLIST_ENTRIES as u64)
                    .map(|total| total as usize)
            });
        let complete = Self::remote_playlist_page_is_complete(
            cursor.is_some(),
            provider_complete,
            next_cursor.is_some(),
            total,
            tracks.len(),
        );
        Ok(RemotePlaylistBrowsePage {
            write_capabilities: parse_write_capabilities(&metadata_value)?,
            provider: provider.into(),
            playlist_id,
            url,
            title,
            owner_id,
            owner_name,
            owner_url,
            visibility: normalize_visibility(metadata.get("visibility").and_then(Value::as_str)),
            account_id: response_account.or(expected_account),
            can_edit: metadata
                .get("canEdit")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            tracks,
            next_cursor,
            total,
            complete,
        })
    }

    /// A provider page's `total` is global, while `tracks` contains only this page.
    /// First-page terminal responses must cover the reported total. For continuation
    /// pages, the caller validates the cumulative loaded count against that same total.
    fn remote_playlist_page_is_complete(
        is_continuation: bool,
        provider_complete: bool,
        has_next_cursor: bool,
        total: Option<usize>,
        page_track_count: usize,
    ) -> bool {
        provider_complete
            && !has_next_cursor
            && total.is_none_or(|total| {
                if is_continuation {
                    total >= page_track_count
                } else {
                    total == page_track_count
                }
            })
    }

    /// Read a Spotify artist profile and one release page. Spotify's current public Web API
    /// removed the artist top-tracks endpoint, so the response carries an explicit explanation.
    pub async fn browse_artist(
        &self,
        artist_id: &str,
        cursor: Option<&str>,
    ) -> Result<RemoteArtistBrowsePage> {
        if !valid_spotify_id(artist_id) {
            return Err("Spotify artist ID is invalid".into());
        }
        validate_numeric_cursor(cursor, 100_000)?;
        let account = self.require_account("spotify").await?;
        let artist_page = self
            .bridge
            .provider_operation("spotify", "getArtist", json!({"artistId":artist_id}))
            .await?;
        self.ensure_response_account("spotify", &artist_page, &account.id)?;
        let artist_row = artist_page
            .get("artist")
            .ok_or("Spotify returned invalid artist details")?;
        let artist = parse_spotify_artist(artist_row)?;
        if artist.id != artist_id {
            return Err("Spotify returned a different artist".into());
        }
        let mut args = json!({"artistId":artist_id,"limit":REMOTE_BROWSE_PAGE_SIZE});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let album_page = self
            .bridge
            .provider_operation("spotify", "getArtistAlbums", args)
            .await?;
        self.ensure_response_account("spotify", &album_page, &account.id)?;
        let albums = album_page
            .get("items")
            .and_then(Value::as_array)
            .ok_or("Spotify returned invalid artist releases")?
            .iter()
            .map(parse_spotify_album)
            .collect::<Result<Vec<_>>>()?;
        Ok(RemoteArtistBrowsePage {
            provider: "spotify".into(),
            account_id: account.id,
            artist,
            albums,
            next_cursor: album_page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned),
            total_albums: album_page
                .get("total")
                .and_then(Value::as_u64)
                .filter(|total| *total <= 100_000)
                .map(|total| total as usize),
            top_tracks_unavailable: "Spotify's documented Web API no longer provides artist top tracks. Browse the artist's releases instead.".into(),
        })
    }
    /// Read one bounded provider-ordered artist track page without persisting it.
    /// Inspect one authenticated artist-list response; return fixed counts/enums only.
    pub async fn inspect_spotify_artist_page(
        &self,
        request: &SpotifyArtistInspectionRequest,
    ) -> Result<SpotifyArtistResponseShape> {
        request.validate()?;
        let account = self
            .require_account("spotify")
            .await
            .map_err(|_| "Spotify account is unavailable for artist inspection".to_owned())?;
        let mut args = json!({"artistId":request.artist_id,"limit":30,"inspectResponseShape":true});
        if let Some(cursor) = &request.cursor {
            args["cursor"] = json!(cursor);
        }
        let page = self
            .bridge
            .provider_operation("spotify", "getArtistTracks", args)
            .await
            .map_err(|_| "Spotify artist inspection failed".to_owned())?;
        self.ensure_response_account("spotify", &page, &account.id)
            .map_err(|_| "Spotify account changed during artist inspection".to_owned())?;
        if page.get("artistId").and_then(Value::as_str) != Some(request.artist_id.as_str()) {
            return Err("Spotify artist changed during inspection".into());
        }
        parse_spotify_artist_response_shape(&page)
    }
    pub async fn browse_artist_tracks(
        &self,
        artist_id: &str,
        cursor: Option<&str>,
    ) -> Result<RemoteArtistTracksPage> {
        if !valid_spotify_id(artist_id) {
            return Err("Spotify artist ID is invalid".into());
        }
        validate_numeric_cursor(cursor, 100_000)?;
        let account = self.require_account("spotify").await?;
        let mut args = json!({"artistId":artist_id,"limit":30});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let page = self
            .bridge
            .provider_operation("spotify", "getArtistTracks", args)
            .await?;
        self.ensure_response_account("spotify", &page, &account.id)?;
        parse_artist_tracks_page(&page, artist_id, cursor, &account.id)
    }
    /// Read album metadata and one page of tracks for a Spotify release.
    pub async fn browse_album_tracks(
        &self,
        album_id: &str,
        cursor: Option<&str>,
    ) -> Result<RemoteAlbumTracksPage> {
        if !valid_spotify_id(album_id) {
            return Err("Spotify album ID is invalid".into());
        }
        validate_numeric_cursor(cursor, 100_000)?;
        let account = self.require_account("spotify").await?;
        let album_result = self
            .bridge
            .provider_operation("spotify", "getAlbum", json!({"albumId":album_id}))
            .await?;
        self.ensure_response_account("spotify", &album_result, &account.id)?;
        let album_row = album_result
            .get("album")
            .ok_or("Spotify returned invalid album details")?;
        let album = parse_spotify_album(album_row)?;
        if album.id != album_id {
            return Err("Spotify returned a different album".into());
        }
        let mut args = json!({"albumId":album_id,"limit":REMOTE_BROWSE_PAGE_SIZE});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let track_page = self
            .bridge
            .provider_operation("spotify", "getAlbumTracks", args)
            .await?;
        self.ensure_response_account("spotify", &track_page, &account.id)?;
        let track_items = track_page
            .get("items")
            .and_then(Value::as_array)
            .ok_or("Spotify returned invalid album tracks")?;
        let offset = cursor
            .unwrap_or("0")
            .parse::<usize>()
            .expect("validated numeric Spotify album cursor");
        let raw_page_count = track_items
            .len()
            .checked_add(
                track_page
                    .get("unrecognizedCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
            )
            .ok_or("Spotify album track page is out of range")?;
        let page_end = offset
            .checked_add(raw_page_count)
            .ok_or("Spotify album track page is out of range")?;
        let mut seen_positions = HashSet::new();
        let tracks = track_items.iter()
            .enumerate()
            .map(|(index, item)| {
                let track_id = required_text(item, "trackId", 64)?;
                // The adapter filters unrecognized source rows, so its explicit one-based
                // position is authoritative. Preserve that gap in occurrence identity/order.
                let position = if let Some(value) = item.get("position") {
                    let ordinal = value.as_u64()
                        .filter(|ordinal| *ordinal > 0)
                        .ok_or("Spotify returned an invalid album track position")?;
                    let position = usize::try_from(ordinal - 1)
                        .map_err(|_| "Spotify album track position is out of range")?;
                    if position < offset || position >= page_end {
                        return Err("Spotify returned an album track position outside its page".into());
                    }
                    position
                } else {
                    offset.checked_add(index)
                        .ok_or("Spotify album track position is out of range")?
                };
                if !seen_positions.insert(position) {
                    return Err("Spotify returned duplicate album track positions".into());
                }
                let ordinal = position
                    .checked_add(1)
                    .ok_or("Spotify album track position is out of range")?;
                let occurrence_id = spotify_album_occurrence_id(album_id, position);
                let row = json!({
                    "entryId": occurrence_id,
                    "trackId": track_id,
                    "title": item.get("title").and_then(Value::as_str).unwrap_or(""),
                    "artist": item.get("artist").and_then(Value::as_str).unwrap_or(""),
                    "album": album.title,
                    "durationSeconds": item.get("durationSeconds").cloned().unwrap_or(Value::Null),
                    "url": item.get("url").and_then(Value::as_str).unwrap_or(""),
                    "artworkUrl": album.artwork_url,
                    "available": item.get("available").and_then(Value::as_bool).unwrap_or(true),
                    "plays": reported_remote_count(item, &["plays", "playCount", "play_count"]).map(Value::from).unwrap_or(Value::Null),
                    "views": reported_remote_count(item, &["views", "viewCount", "view_count"]).map(Value::from).unwrap_or(Value::Null),
                });
                let remote =
                    parse_remote_entry(&row, "spotify", &occurrence_id)?;
                let mut track = entry_to_track("spotify", &remote, position);
                track.album = Some(album.title.clone());
                track.provenance["albumId"] = json!(album_id);
                track.provenance["position"] = json!(ordinal);
                track.provenance["positionScope"] = json!("album");
                Ok(track)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(RemoteAlbumTracksPage {
            provider: "spotify".into(),
            account_id: account.id,
            album,
            tracks,
            next_cursor: track_page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned),
            total: track_page
                .get("total")
                .and_then(Value::as_u64)
                .filter(|total| *total <= MAX_PLAYLIST_ENTRIES as u64)
                .map(|total| total as usize),
        })
    }

    /// Prepare a verified Spotify-to-MusicBrainz occurrence sidecar for an exact
    /// Spotify track. This is an explicit read operation; playback consumes only
    /// the resulting Store cache and never performs provider network requests.
    pub async fn prepare_spotify_recording_crosswalk(
        &self,
        artists: &crate::artists::ArtistService,
        track_id: &str,
        album_id_hint: Option<&str>,
        deadline: tokio::time::Instant,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<crate::artists::ReleaseOccurrence> {
        if !valid_spotify_id(track_id) {
            return Err("A Spotify track ID must contain exactly 22 letters or digits".into());
        }
        let (mut target, expected_account, album_id) = if let Some(hint) = album_id_hint {
            if !valid_spotify_id(hint) {
                return Err(
                    "A Spotify album hint must contain exactly 22 letters or digits".into(),
                );
            }
            // The caller-supplied album ID is only a locator. It never establishes
            // identity; the complete, repeated album read below must contain the
            // exact requested track exactly once before verification can proceed.
            let account =
                await_crosswalk_step(deadline, cancel.clone(), self.require_account("spotify"))
                    .await?;
            (None, account.id, hint.to_owned())
        } else {
            let (target, expected_account) = await_crosswalk_step(
                deadline,
                cancel.clone(),
                self.get_remote_track_with_account("spotify", track_id),
            )
            .await?;
            let context = target
                .provenance
                .get("recordingContext")
                .and_then(Value::as_object)
                .ok_or("Spotify track has no verified album occurrence context")?;
            let album_id = context
                .get("albumId")
                .and_then(Value::as_str)
                .filter(|id| valid_spotify_id(id))
                .ok_or("Spotify track has no verified album identity")?;
            if context.get("albumUri").and_then(Value::as_str)
                != Some(format!("spotify:album:{album_id}").as_str())
            {
                return Err("Spotify track album identity is inconsistent".into());
            }
            let album_id = album_id.to_owned();
            (Some(target), expected_account, album_id)
        };

        let mut cursor: Option<String> = None;
        let mut total: Option<usize> = None;
        let mut account_id: Option<String> = None;
        let mut album_title: Option<String> = None;
        let mut album_url: Option<String> = None;
        let mut album_artwork: Option<Option<String>> = None;
        let mut album_release_date: Option<Option<String>> = None;
        let mut album_total_tracks: Option<Option<usize>> = None;
        let mut tracks = Vec::<Value>::new();
        let mut target_positions = Vec::<u32>::new();
        let mut album_target: Option<Track> = None;
        let mut first_snapshot: Option<Value> = None;
        let mut verified_snapshot: Option<Value> = None;
        loop {
            if tracks.len() > MAX_PLAYLIST_ENTRIES {
                return Err("Spotify album is too large for bounded recording verification".into());
            }
            let current_offset = cursor
                .as_deref()
                .unwrap_or("0")
                .parse::<usize>()
                .map_err(|_| "Spotify album cursor is invalid")?;
            let page = await_crosswalk_step(
                deadline,
                cancel.clone(),
                self.browse_album_tracks(&album_id, cursor.as_deref()),
            )
            .await?;
            if page.provider != "spotify"
                || page.account_id != expected_account
                || page.album.id != album_id
                || page.album.url != format!("https://open.spotify.com/album/{album_id}")
            {
                return Err(
                    "Spotify account or album changed during recording verification".into(),
                );
            }
            if account_id
                .as_ref()
                .is_some_and(|value| value != &page.account_id)
                || album_title
                    .as_ref()
                    .is_some_and(|value| value != &page.album.title)
                || album_url
                    .as_ref()
                    .is_some_and(|value| value != &page.album.url)
                || album_artwork
                    .as_ref()
                    .is_some_and(|value| value != &page.album.artwork_url)
                || album_release_date
                    .as_ref()
                    .is_some_and(|value| value != &page.album.release_date)
                || album_total_tracks
                    .as_ref()
                    .is_some_and(|value| value != &page.album.total_tracks)
            {
                return Err("Spotify album metadata changed during recording verification".into());
            }
            account_id.get_or_insert_with(|| page.account_id.clone());
            album_title.get_or_insert_with(|| page.album.title.clone());
            album_url.get_or_insert_with(|| page.album.url.clone());
            album_artwork.get_or_insert_with(|| page.album.artwork_url.clone());
            album_release_date.get_or_insert_with(|| page.album.release_date.clone());
            album_total_tracks.get_or_insert_with(|| page.album.total_tracks);

            let page_total = page
                .total
                .ok_or("Spotify album track total is unavailable")?;
            if page_total > MAX_PLAYLIST_ENTRIES
                || total.is_some_and(|expected| expected != page_total)
                || page
                    .album
                    .total_tracks
                    .is_some_and(|album_total| album_total != page_total)
            {
                return Err("Spotify album track total changed or is inconsistent".into());
            }
            total.get_or_insert(page_total);
            if page.tracks.is_empty() && page.next_cursor.is_some() {
                return Err("Spotify album returned an empty nonterminal page".into());
            }
            for (index, track) in page.tracks.iter().enumerate() {
                let expected_position = current_offset
                    .checked_add(index)
                    .and_then(|v| v.checked_add(1))
                    .ok_or("Spotify album position is out of range")?;
                let position = track
                    .provenance
                    .get("position")
                    .and_then(Value::as_u64)
                    .filter(|position| *position <= u32::MAX as u64)
                    .ok_or("Spotify album track position is missing")?
                    as u32;
                if track.provider != "spotify"
                    || !valid_spotify_id(&track.provider_id)
                    || track.provenance.get("albumId").and_then(Value::as_str)
                        != Some(album_id.as_str())
                    || track
                        .provenance
                        .get("positionScope")
                        .and_then(Value::as_str)
                        != Some("album")
                    || position as usize != expected_position
                {
                    return Err("Spotify album track order is incomplete or inconsistent".into());
                }
                if track.provider_id == track_id {
                    target_positions.push(position);
                    album_target = Some(track.clone());
                }
                let duration_ms = track
                    .duration_seconds
                    .filter(|seconds| seconds.is_finite() && *seconds > 0.0 && *seconds <= 86_400.0)
                    .map(|seconds| (seconds * 1000.0).round() as u64);
                let mut row = json!({
                    "position": position,
                    "trackId": track.provider_id,
                    "title": track.title,
                    "artist": track.artist,
                    "durationMs": duration_ms,
                });
                if let Some(version) = track.version.as_deref() {
                    row["version"] = json!(version);
                }
                tracks.push(row);
            }
            if tracks.len() > page_total {
                return Err("Spotify album returned more rows than its reported total".into());
            }
            match page.next_cursor {
                Some(next) => {
                    let next_offset = next
                        .parse::<usize>()
                        .map_err(|_| "Spotify album returned an invalid continuation cursor")?;
                    if next_offset != tracks.len()
                        || next_offset <= current_offset
                        || next_offset >= page_total
                    {
                        return Err("Spotify album continuation cursor is inconsistent".into());
                    }
                    cursor = Some(next);
                }
                None => {
                    if tracks.len() != page_total {
                        return Err("Spotify album ended before its reported total".into());
                    }
                    let snapshot = json!({
                        "albumId": album_id,
                        "albumUrl": format!("https://open.spotify.com/album/{album_id}"),
                        "albumTitle": album_title.clone(),
                        "artworkUrl": album_artwork.clone(),
                        "releaseDate": album_release_date.clone(),
                        "spotifyReportedAlbumTotal": album_total_tracks.flatten(),
                        "complete": true,
                        "total": page_total,
                        "tracks": tracks.clone(),
                    });
                    if let Some(first) = first_snapshot.as_ref() {
                        if first != &snapshot {
                            return Err(
                                "Spotify album changed during repeated occurrence verification"
                                    .into(),
                            );
                        }
                        verified_snapshot = Some(snapshot);
                        break;
                    }
                    // Offset pagination is not a provider snapshot. Require two
                    // complete identical reads before using ordinals as evidence.
                    first_snapshot = Some(snapshot);
                    cursor = None;
                    total = None;
                    account_id = None;
                    album_title = None;
                    album_url = None;
                    album_artwork = None;
                    album_release_date = None;
                    album_total_tracks = None;
                    tracks = Vec::new();
                    target_positions = Vec::new();
                    album_target = None;
                }
            }
        }
        let snapshot = verified_snapshot.ok_or("Spotify album verification did not complete")?;
        if target_positions.len() != 1 {
            artists.invalidate_verified_spotify_recording(
                &album_id,
                track_id,
                target_positions.len(),
            )?;
            return Err("Spotify exact track does not occur once in its complete album".into());
        }
        if target.is_none() {
            target = album_target;
        }
        let target = target.ok_or("Spotify exact track is missing from its complete album")?;
        let occurrence = await_crosswalk_step(
            deadline,
            cancel.clone(),
            artists.verify_spotify_album_recording(&snapshot, track_id, target_positions[0]),
        )
        .await?;
        // Alias preparation is optional; playback itself remains network-free.
        // Await it under the same absolute deadline and cancellation receiver.
        let profile_deadline =
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
        if let Err(error) = await_crosswalk_step(profile_deadline, cancel.clone(), async {
            artists
                .prepare_verified_spotify_artist_profiles(&occurrence)
                .await;
            Ok(())
        })
        .await
        {
            if *cancel.borrow() {
                return Err(error);
            }
            if tokio::time::Instant::now() < profile_deadline {
                return Err(error);
            }
            // Optional profile warming may time out; verified mapping remains cached.
        }
        // Older library rows may lack recordingContext. Cache the exact provider
        // lookup only when its source fields agree with the verified album row.
        let target_duration_ms = target
            .duration_seconds
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0 && *seconds <= 86_400.0)
            .map(|seconds| (seconds * 1000.0).round() as u64);
        let duration_agrees = match (target_duration_ms, occurrence.spotify_duration_ms) {
            (Some(left), Some(right)) => left.abs_diff(right) <= 5_000,
            _ => true,
        };
        if target.title == occurrence.spotify_title
            && target.artist == occurrence.spotify_artist
            && duration_agrees
        {
            write_verified_spotify_track_context(
                &self.store,
                &expected_account,
                track_id,
                &occurrence,
            )?;
        }
        Ok(occurrence)
    }

    /// Alternative cache prewarm for a user-selected MusicBrainz recording when
    /// Spotify's album relation is absent. The MBID is only a locator; source
    /// track data comes from the exact stored Spotify listing and the artist
    /// service validates complete ordered IDs, duration, title, ISRC and URLs.
    pub async fn prepare_spotify_artist_credit_crosswalk(
        &self,
        artists: &crate::artists::ArtistService,
        track_id: &str,
        recording_id_hint: &str,
        deadline: tokio::time::Instant,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<crate::artist_credit_aliases::SpotifyArtistCreditCrosswalkStatus> {
        if !valid_spotify_id(track_id) {
            return Err("A Spotify track ID must contain exactly 22 letters or digits".into());
        }
        let listing_id = format!("spotify:{track_id}");
        let track = self
            .store
            .get_library_track(&listing_id)?
            .ok_or("The exact Spotify track is not saved in the library")?;
        if track.id != listing_id || track.provider_id != track_id {
            return Err("The saved Spotify track identity changed".into());
        }
        let fingerprint =
            crate::artist_credit_aliases::spotify_artist_credit_fingerprint(&track)
                .ok_or("The exact saved Spotify track has no complete verified artist identity")?;
        let status = await_crosswalk_step(
            deadline,
            cancel,
            artists.prepare_spotify_recording_credit_crosswalk(&track, recording_id_hint),
        )
        .await?;
        if status.spotify_track_id != fingerprint.track_id {
            return Err("The saved Spotify track changed during recording verification".into());
        }
        Ok(status)
    }

    /// Hydrate provider identity into a retained sidecar, preserving every saved tag.
    pub(crate) async fn prepare_alias_track(
        &self,
        original: &Track,
        cancel: tokio::sync::watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> Result<Track> {
        let fresh = alias_hydration_key(original)
            .ok()
            .and_then(|key| {
                self.store
                    .get_provider_cache(ALIAS_HYDRATION_CACHE, &key)
                    .ok()
                    .flatten()
            })
            .and_then(|value| value["checkedAt"].as_i64())
            .is_some_and(|at| {
                (0..=30 * 24 * 60 * 60).contains(&(chrono::Utc::now().timestamp() - at))
            });
        if fresh {
            if let Some(track) = read_alias_hydrated_track(&self.store, original) {
                return Ok(track);
            }
        }
        if original.provider != "spotify"
            || original.id != format!("spotify:{}", original.provider_id)
            || original.source_url
                != format!("https://open.spotify.com/track/{}", original.provider_id)
            || original.version.is_some()
            || original.playback_range.is_some()
        {
            return Err("Saved track has no exact Spotify identity".into());
        }
        let account =
            await_crosswalk_step(deadline, cancel.clone(), self.require_account("spotify")).await?;
        let provider = match await_crosswalk_step(
            deadline,
            cancel.clone(),
            self.get_remote_track_for_account("spotify", &original.provider_id, account.clone()),
        )
        .await
        {
            Ok((provider, response_account)) if response_account == account.id => provider,
            Ok(_) => return Err("Provider account changed during alias hydration".into()),
            Err(_error) if !*cancel.borrow() && tokio::time::Instant::now() < deadline => {
                let query = alias_hydration_search_query(original)?;
                let response = await_crosswalk_step(
                    deadline,
                    cancel.clone(),
                    self.bridge.provider_operation(
                        "spotify",
                        "search",
                        json!({"query":query,"kind":"track","limit":50}),
                    ),
                )
                .await
                .map_err(|_| {
                    "Spotify alias hydration could not verify the exact track".to_owned()
                })?;
                self.ensure_response_account("spotify", &response, &account.id)?;
                exact_alias_search_track(original, &response, &account.id)?
            }
            Err(error) => return Err(error),
        };
        let current =
            await_crosswalk_step(deadline, cancel, self.require_account("spotify")).await?;
        if current.id != account.id {
            return Err("Provider account changed during alias hydration".into());
        }
        let hydrated = merge_alias_hydration(original, &provider)
            .ok_or("Exact provider identity disagrees with saved track")?;
        self.store.set_retained_provider_cache(ALIAS_HYDRATION_CACHE, &alias_hydration_key(original)?,
            &json!({"version":1,"original":original,"hydrated":hydrated,"checkedAt":chrono::Utc::now().timestamp()}))?;
        Ok(hydrated)
    }

    /// Read one exact Spotify catalog track without persisting it to the local library.
    pub async fn get_remote_track(&self, provider: &str, track_id: &str) -> Result<Track> {
        self.get_remote_track_with_account(provider, track_id)
            .await
            .map(|(track, _account_id)| track)
    }

    /// Same exact lookup with the account identity that guarded the provider response.
    /// Crosswalk preparation uses it to bind the subsequent album pages to this read.
    async fn get_remote_track_with_account(
        &self,
        provider: &str,
        track_id: &str,
    ) -> Result<(Track, String)> {
        if provider != "spotify" || !valid_spotify_id(track_id) {
            return Err("A Spotify track ID must contain exactly 22 letters or digits".into());
        }
        let account = self.require_account(provider).await?;
        self.get_remote_track_for_account(provider, track_id, account)
            .await
    }

    async fn get_remote_track_for_account(
        &self,
        provider: &str,
        track_id: &str,
        account: CachedAccount,
    ) -> Result<(Track, String)> {
        if provider != "spotify" || !valid_spotify_id(track_id) {
            return Err("Invalid Spotify track identity".into());
        }
        let result = self
            .bridge
            .provider_operation(provider, "getTrack", json!({"trackId":track_id}))
            .await?;
        self.ensure_response_account(provider, &result, &account.id)?;
        let items = result
            .get("items")
            .and_then(Value::as_array)
            .filter(|items| items.len() == 1)
            .ok_or("Spotify returned invalid exact-track details")?;
        let item = &items[0];
        if item.get("kind").and_then(Value::as_str) != Some("track")
            || item.get("id").and_then(Value::as_str) != Some(track_id)
        {
            return Err("Spotify returned a different track than requested".into());
        }
        let data = item
            .get("data")
            .ok_or("Spotify returned invalid track metadata")?;
        if required_text(data, "id", 64)? != track_id {
            return Err("Spotify returned a different track than requested".into());
        }
        let url = validate_remote_url(required_text(data, "url", 2048)?, provider)?;
        if url != format!("https://open.spotify.com/track/{track_id}") {
            return Err("Spotify returned a track URL for a different ID".into());
        }
        let mut track = Track {
            id: format!("spotify:{track_id}"),
            provider: "spotify".into(),
            provider_id: track_id.into(),
            source_url: url,
            title: required_text(data, "title", 512)?.into(),
            artist: bounded_optional_text(data, "artist", 512)
                .unwrap_or_else(|| "Unknown Artist".into()),
            album: bounded_optional_text(data, "album", 512),
            duration_seconds: verified_spotify_duration(item, track_id),
            artwork_url: bounded_optional_text(data, "artworkUrl", 2048),
            provenance: {
                let mut remote = normalize_catalog_data(Some(data), "track", "spotify")?;
                remote["trackId"] = json!(track_id);
                let mut provenance = json!({"providerLookup":"spotifyCatalog", "remoteCatalog":remote,
                    "catalog":{"verified":true,"provider":"spotify","providerId":track_id,
                        "artists":remote["artistRefs"],"artistCount":remote["artistCount"]}});
                if let Some(context) = parse_spotify_recording_context(data)? {
                    provenance["recordingContext"] = context;
                }
                provenance
            },
            ..Track::default()
        };
        crate::recording_identity::hydrate_identifiers(&mut track);
        Ok((track, account.id))
    }
    pub async fn list_playlists(
        &self,
        provider: &str,
        owner_id: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<RemotePlaylistPage> {
        validate_provider(provider)?;
        let account = if provider == "youtube" && owner_id.is_some() {
            None
        } else {
            Some(self.require_account(provider).await?)
        };
        let mut args =
            json!({"limit":if provider == "spotify" && owner_id.is_some() { 30 } else { 50 }});
        if let Some(owner) = owner_id {
            args["ownerId"] = json!(owner);
        }
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let value = self
            .bridge
            .provider_operation(provider, "listPlaylists", args)
            .await?;
        let result_account = self.optional_result_account(provider, &value)?;
        if account
            .as_ref()
            .zip(result_account.as_ref())
            .is_some_and(|(expected, actual)| actual.id != expected.id)
        {
            return Err("Provider account changed while browsing playlists".into());
        }
        let rows = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or("Provider returned an invalid playlist page")?;
        let bindings = self.load_state()?.bindings;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let item = parse_playlist_item(row, provider)?;
            let binding = result_account
                .as_ref()
                .or(account.as_ref())
                .and_then(|account| {
                    bindings.iter().find(|b| {
                        b.provider == provider
                            && b.playlist_id == item.id
                            && b.account_id == account.id
                    })
                });
            items.push(RemotePlaylistItem {
                imported: binding.is_some(),
                binding_id: binding.map(|b| b.binding_id.clone()),
                collection_id: binding.map(|b| b.collection_id.clone()),
                ..item
            });
        }
        Ok(RemotePlaylistPage {
            provider: provider.into(),
            account_id: result_account.or(account).map(|account| account.id),
            items,
            next_cursor: value
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned),
            total_count: provider_display_count(
                value.get("totalCount").or_else(|| value.get("total")),
            ),
            truncated: value
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            complete: value.get("complete").and_then(Value::as_bool),
            snapshot_consistency: playlist_snapshot_consistency(value.get("snapshotConsistency"))?,
        })
    }
    pub async fn search(
        &self,
        provider: &str,
        query: &str,
        kind: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<RemotePlaylistPage> {
        validate_provider(provider)?;
        if query.trim().is_empty() || query.len() > 256 {
            return Err("Search text must contain 1–256 characters".into());
        }
        let account = if provider == "youtube" {
            None
        } else {
            Some(self.require_account(provider).await?)
        };
        let mut args = json!({"query":query,"limit":50});
        if let Some(kind) = kind {
            args["kind"] = json!(kind);
        }
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let value = self
            .bridge
            .provider_operation(provider, "search", args)
            .await?;
        let result_account = self.optional_result_account(provider, &value)?;
        if account
            .as_ref()
            .zip(result_account.as_ref())
            .is_some_and(|(expected, actual)| actual.id != expected.id)
        {
            return Err("Provider account changed while searching".into());
        }
        let rows = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or("Provider returned an invalid search page")?;
        let bindings = self.load_state()?.bindings;
        let mut items = Vec::new();
        for row in rows {
            let mapped = if row.get("kind").and_then(Value::as_str) == Some("playlist")
                || row.get("total").is_some()
            {
                parse_playlist_item(row, provider)?
            } else {
                continue;
            };
            let binding = result_account
                .as_ref()
                .or(account.as_ref())
                .and_then(|account| {
                    bindings.iter().find(|b| {
                        b.provider == provider
                            && b.playlist_id == mapped.id
                            && b.account_id == account.id
                    })
                });
            items.push(RemotePlaylistItem {
                imported: binding.is_some(),
                binding_id: binding.map(|b| b.binding_id.clone()),
                collection_id: binding.map(|b| b.collection_id.clone()),
                ..mapped
            });
        }
        Ok(RemotePlaylistPage {
            provider: provider.into(),
            account_id: result_account.or(account).map(|account| account.id),
            items,
            next_cursor: value
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned),
            total_count: provider_display_count(
                value.get("totalCount").or_else(|| value.get("total")),
            ),
            truncated: value
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            complete: value.get("complete").and_then(Value::as_bool),
            snapshot_consistency: None,
        })
    }
    /// Search fixed, provider-specific catalog types and return normalized typed rows.
    pub async fn search_catalog(
        &self,
        provider: &str,
        query: &str,
        kinds: Vec<String>,
        cursor: Option<&str>,
    ) -> Result<Value> {
        validate_provider(provider)?;
        if query.trim().is_empty() || query.len() > 256 {
            return Err("Search text must contain 1–256 characters".into());
        }
        if kinds.is_empty() || kinds.len() > 5 {
            return Err("Choose one to five search types".into());
        }
        let mut seen = HashSet::new();
        for kind in &kinds {
            if !supported_search_kind(provider, kind) || !seen.insert(kind.clone()) {
                return Err("Search contains an unsupported or repeated type".into());
            }
        }
        let account = if provider == "spotify" {
            Some(self.require_account(provider).await?)
        } else {
            None
        };
        let mut cursors = HashMap::<String, String>::new();
        if let Some(cursor) = cursor {
            if let Some(encoded) = cursor.strip_prefix("rp1:") {
                let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| "Invalid catalog cursor")?;
                let state: CatalogCursor =
                    serde_json::from_slice(&bytes).map_err(|_| "Invalid catalog cursor")?;
                if state.provider != provider
                    || state.kinds != kinds
                    || state.query_hash != catalog_query_hash(query)
                {
                    return Err("Catalog cursor does not match this search".into());
                }
                cursors = state.cursors;
            } else if kinds.len() == 1 {
                cursors.insert(kinds[0].clone(), cursor.to_owned());
            } else {
                return Err("Invalid catalog cursor".into());
            }
        }
        let mut items = Vec::new();
        let mut next = HashMap::<String, String>::new();
        let mut result_account = None;
        for kind in &kinds {
            let mut args = json!({"query":query,"kind":kind,"limit":30});
            if let Some(cursor) = cursors.get(kind) {
                args["cursor"] = json!(cursor);
            }
            let page = self
                .bridge
                .provider_operation(provider, "search", args)
                .await?;
            if let Some(actual) = self.optional_result_account(provider, &page)? {
                if account
                    .as_ref()
                    .is_some_and(|expected| expected.id != actual.id)
                {
                    return Err("Provider account changed during catalog search".into());
                }
                result_account = Some(actual.id);
            }
            let rows = page
                .get("items")
                .and_then(Value::as_array)
                .ok_or("Provider returned an invalid search page")?;
            for row in rows {
                if row.get("kind").and_then(Value::as_str) != Some(kind.as_str()) {
                    return Err("Provider returned a result of the wrong type".into());
                }
                items.push(normalize_catalog_item(row, provider, kind)?);
            }
            if let Some(next_cursor) = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                next.insert(kind.clone(), next_cursor.to_owned());
            }
        }
        let account_id =
            result_account.or_else(|| account.as_ref().map(|account| account.id.clone()));
        let next_cursor = if next.is_empty() {
            None
        } else {
            let state = CatalogCursor {
                provider: provider.into(),
                kinds,
                query_hash: catalog_query_hash(query),
                cursors: next,
            };
            let bytes = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
            Some(format!(
                "rp1:{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
            ))
        };
        let result = json!({"provider":provider,"accountId":account_id,"items":items,"nextCursor":next_cursor});
        if serde_json::to_vec(&result)
            .map_err(|e| e.to_string())?
            .len()
            > 512 * 1024
        {
            return Err("Catalog page exceeds the response size limit".into());
        }
        Ok(result)
    }
    /// Start a bounded import job. Each selected playlist is persisted independently so a later
    /// failure or cancellation keeps completed snapshots available.
    pub fn import_playlists(
        &self,
        provider: &str,
        playlist_ids: Vec<String>,
    ) -> Result<RemotePlaylistJobStatus> {
        validate_provider(provider)?;
        if playlist_ids.is_empty() || playlist_ids.len() > MAX_IMPORT_IDS {
            return Err(format!("Select 1–{MAX_IMPORT_IDS} playlists"));
        }
        let playlist_ids = playlist_ids
            .iter()
            .map(|value| normalize_playlist_reference(provider, value))
            .collect::<Result<Vec<_>>>()?;
        let job_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .insert(job_id.clone(), cancel.clone());
        let status = RemotePlaylistJobStatus {
            job_id: job_id.clone(),
            state: "queued".into(),
            completed: 0,
            total: playlist_ids.len(),
            results: vec![],
            errors: vec![],
        };
        self.jobs
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .insert(
                job_id.clone(),
                ImportJob {
                    status: status.clone(),
                },
            );
        let service = self.clone();
        let id = job_id.clone();
        let provider = provider.to_owned();
        tauri::async_runtime::spawn(async move {
            service.set_job_state(&id, "running");
            for playlist_id in playlist_ids {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let result = service
                    .capture_and_bind(&provider, &playlist_id, None, false, &cancel)
                    .await;
                if let Ok(mut jobs) = service.jobs.lock() {
                    if let Some(job) = jobs.get_mut(&id) {
                        match result {
                            Ok(item) => job.status.results.push(item),
                            Err(message) => job.status.errors.push(RemotePlaylistJobError {
                                playlist_id: playlist_id.clone(),
                                message,
                            }),
                        }
                    }
                }
                if let Ok(mut jobs) = service.jobs.lock() {
                    if let Some(job) = jobs.get_mut(&id) {
                        job.status.completed += 1;
                    }
                }
            }
            let final_state = if cancel.load(Ordering::SeqCst) {
                "cancelled"
            } else if service
                .import_job_status(&id)
                .is_ok_and(|job| !job.errors.is_empty())
            {
                "partial"
            } else {
                "completed"
            };
            service.set_job_state(&id, final_state);
            if let Ok(mut cancel_map) = service.cancel.lock() {
                cancel_map.remove(&id);
            }
        });
        Ok(status)
    }
    pub fn import_job_status(&self, job_id: &str) -> Result<RemotePlaylistJobStatus> {
        self.jobs
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .get(job_id)
            .map(|job| job.status.clone())
            .ok_or("Import job not found".into())
    }
    pub fn cancel_import(&self, job_id: &str) -> Result<RemotePlaylistJobStatus> {
        let cancel = self
            .cancel
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .get(job_id)
            .cloned()
            .ok_or("Import job is no longer active".to_owned())?;
        cancel.store(true, Ordering::SeqCst);
        self.import_job_status(job_id)
    }
    pub async fn refresh(&self, binding_id: &str) -> Result<RemotePlaylistImportItem> {
        let binding = self.find_binding(binding_id)?;
        self.capture_and_bind(
            &binding.provider,
            &binding.playlist_id,
            Some(binding_id),
            false,
            &Arc::new(AtomicBool::new(false)),
        )
        .await
    }
    pub async fn search_remote_playlist_writeback_candidates(
        &self,
        binding_id: &str,
        local_track_id: &str,
    ) -> Result<RemotePlaylistTrackMatches> {
        let binding = self.find_binding(binding_id)?;
        if !matches!(binding.provider.as_str(), "spotify" | "youtube") {
            return Err("Write sync is not available for this provider".into());
        }
        let track = self
            .store
            .list_tracks("collection", Some(&binding.collection_id))?
            .into_iter()
            .find(|track| track.id == local_track_id)
            .ok_or("Track is not in this playlist's local collection")?;
        if track.playback_range.is_some() {
            return Err("Chapter excerpts cannot be matched to a provider track".into());
        }
        if (binding.provider == "spotify" && has_confirmed_spotify_identity(&track))
            || (binding.provider == "youtube" && has_confirmed_youtube_identity(&track))
        {
            return Err("This track already has a verified provider identity".into());
        }
        let query = format!("{} {}", track.artist, track.title);
        let before = self.require_account(&binding.provider).await?;
        let page = self
            .search_catalog(&binding.provider, &query, vec!["track".into()], None)
            .await?;
        let after = self.require_account(&binding.provider).await?;
        let returned_account = page.get("accountId").and_then(Value::as_str);
        if before.id != after.id
            || returned_account.is_some_and(|account_id| account_id != before.id)
        {
            return Err("Provider account changed while searching".into());
        }
        let local_identity_hash = track_identity_hash(&track)?;
        let mut candidates = Vec::new();
        let mut tokens = self
            .track_match_candidates
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?;
        let ttl = std::time::Duration::from_secs(15 * 60);
        tokens.retain(|_, candidate| candidate.created_at.elapsed() < ttl);
        for row in page
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(30)
        {
            let Some(candidate_id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            if (binding.provider == "spotify" && !valid_spotify_id(candidate_id))
                || (binding.provider == "youtube" && !valid_youtube_video_id(candidate_id))
            {
                continue;
            }
            let Some(url) = row.get("url").and_then(Value::as_str) else {
                continue;
            };
            let data = row.get("data").unwrap_or(&Value::Null);
            if data.get("available").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let artist = data
                .get("artist")
                .and_then(Value::as_str)
                .or_else(|| row.get("subtitle").and_then(Value::as_str))
                .unwrap_or("Unknown Artist")
                .to_owned();
            let duration_seconds = data.get("durationSeconds").and_then(Value::as_f64);
            let version = data
                .get("version")
                .and_then(Value::as_str)
                .filter(|version| !version.trim().is_empty())
                .map(str::to_owned);
            let local_version =
                format!("{} {}", track.title, track.version.as_deref().unwrap_or(""));
            let candidate_version = format!("{} {}", title, version.as_deref().unwrap_or(""));
            if version_markers(&local_version) != version_markers(&candidate_version) {
                continue;
            }
            if track
                .duration_seconds
                .zip(duration_seconds)
                .is_some_and(|(local, candidate)| {
                    (local - candidate).abs() > 3.0_f64.max(local.abs() * 0.03)
                })
            {
                continue;
            }
            let token = uuid::Uuid::new_v4().to_string();
            let uri = if binding.provider == "spotify" {
                format!("spotify:track:{candidate_id}")
            } else {
                format!("youtube:video:{candidate_id}")
            };
            tokens.insert(
                token.clone(),
                CachedTrackMatch {
                    binding_id: binding_id.to_owned(),
                    provider: binding.provider.clone(),
                    account_id: before.id.clone(),
                    local_track_id: track.id.clone(),
                    local_identity_hash: local_identity_hash.clone(),
                    uri,
                    created_at: std::time::Instant::now(),
                },
            );
            candidates.push(RemotePlaylistTrackMatchCandidate {
                candidate_token: token,
                title,
                artist,
                version,
                duration_seconds,
                url: url.to_owned(),
            });
        }
        if tokens.len() > 1000 {
            let mut oldest = tokens
                .iter()
                .map(|(token, value)| (token.clone(), value.created_at))
                .collect::<Vec<_>>();
            oldest.sort_by_key(|(_, created)| *created);
            for (token, _) in oldest.into_iter().take(tokens.len() - 1000) {
                tokens.remove(&token);
            }
        }
        Ok(RemotePlaylistTrackMatches {
            binding_id: binding_id.to_owned(),
            local_track_id: track.id,
            provider: binding.provider,
            candidates,
        })
    }

    pub async fn get_sync_preview(
        &self,
        binding_id: &str,
        reviewed_matches: Option<Vec<ReviewedRemoteTrackMatch>>,
    ) -> Result<RemotePlaylistSyncPreview> {
        self.make_preview(binding_id, reviewed_matches.unwrap_or_default())
            .await
    }

    fn resolve_reviewed_matches(
        &self,
        binding_id: &str,
        provider: &str,
        account_id: &str,
        tracks: &[Track],
        reviewed_matches: &[ReviewedRemoteTrackMatch],
    ) -> Result<HashMap<String, String>> {
        if reviewed_matches.len() > tracks.len() {
            return Err("Reviewed playlist matches are invalid".into());
        }
        let by_id = tracks
            .iter()
            .map(|track| (track.id.as_str(), track))
            .collect::<HashMap<_, _>>();
        let mut seen_tracks = HashSet::new();
        let mut seen_tokens = HashSet::new();
        let mut resolved = HashMap::new();
        let mut candidates = self
            .track_match_candidates
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?;
        let ttl = std::time::Duration::from_secs(15 * 60);
        candidates.retain(|_, candidate| candidate.created_at.elapsed() < ttl);
        for reviewed in reviewed_matches {
            if !seen_tracks.insert(reviewed.local_track_id.as_str())
                || !seen_tokens.insert(reviewed.candidate_token.as_str())
            {
                return Err(
                    "A local track or provider candidate was reviewed more than once".into(),
                );
            }
            let track = by_id
                .get(reviewed.local_track_id.as_str())
                .ok_or("Reviewed match no longer belongs to this playlist")?;
            if (provider == "spotify" && has_confirmed_spotify_identity(track))
                || (provider == "youtube" && has_confirmed_youtube_identity(track))
            {
                return Err(
                    "A reviewed mapping targets a track that already has a provider ID".into(),
                );
            }
            let hash = track_identity_hash(track)?;
            let candidate = candidates
                .get(&reviewed.candidate_token)
                .ok_or("Provider match expired; search for this track again")?;
            if candidate.binding_id != binding_id
                || candidate.provider != provider
                || candidate.account_id != account_id
                || candidate.local_track_id != track.id
                || candidate.local_identity_hash != hash
            {
                return Err("Provider match no longer belongs to this playlist track".into());
            }
            resolved.insert(track.id.clone(), candidate.uri.clone());
        }
        Ok(resolved)
    }
    fn validate_local_preview(
        &self,
        stored: &StoredPreview,
        binding: &RemotePlaylistBindingStatus,
    ) -> Result<()> {
        let current_binding = self.find_binding(&stored.preview.binding_id)?;
        if current_binding.collection_id != binding.collection_id
            || current_binding.provider != binding.provider
            || current_binding.playlist_id != binding.playlist_id
            || current_binding.account_id != binding.account_id
        {
            return Err("Playlist binding changed after preview; build a new preview".into());
        }
        let current_tracks = self
            .store
            .list_tracks("collection", Some(&binding.collection_id))?;
        let current_identity_hashes = current_tracks
            .iter()
            .map(|track| Ok((track.id.clone(), track_identity_hash(track)?)))
            .collect::<Result<HashMap<_, _>>>()?;
        let current_order_hash = hash_json(
            &current_tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
        )?;
        if current_identity_hashes != stored.local_identity_hashes
            || current_order_hash != stored.local_order_hash
        {
            return Err("Local playlist changed after preview; build a new preview".into());
        }
        Ok(())
    }
    pub async fn apply_sync_preview(
        &self,
        preview_id: &str,
        reviewed_changes: Option<RemotePlaylistSyncChanges>,
    ) -> Result<RemotePlaylistSyncPreview> {
        let stored = self
            .previews
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .get(preview_id)
            .cloned()
            .ok_or("Sync preview expired; build a new preview".to_owned())?;
        // Serialize applies for this playlist before any await. This also covers two distinct
        // previews of one binding, which could otherwise pass the same remote revision check.
        // The claim releases on validation failures and remains held through dispatch/verification.
        let _apply_claim = self.claim_preview_apply(&stored.preview.binding_id)?;
        if reviewed_changes
            .as_ref()
            .is_some_and(|changes| changes != &stored.preview.changes)
        {
            return Err(
                "Sync must apply the complete reviewed preview; rebuild it to change the plan"
                    .into(),
            );
        }
        let changes = stored.preview.changes.clone();
        if stored.preview.conflict
            || !changes.unresolved.is_empty()
            || !write_changes_allowed(
                stored.preview.can_edit,
                stored.preview.write_capabilities,
                &changes,
            )
            || !stored.preview.write_sync_enabled
        {
            return Err(
                "Resolve the preview conflict, unavailable tracks, read-only permission, or write-sync opt-in before applying".into(),
            );
        }
        let binding = self.find_binding(&stored.preview.binding_id)?;
        self.validate_local_preview(&stored, &binding)?;
        let fresh = self
            .fetch_remote(
                &binding.provider,
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        if fresh.snapshot.content_hash != stored.remote_hash {
            return Err("Remote playlist changed after preview; build a new preview".into());
        }
        ensure_playlist_capture(
            &binding.provider,
            fresh.snapshot.complete,
            &fresh.snapshot.entries,
        )?;
        if !write_changes_allowed(fresh.can_edit, fresh.write_capabilities, &changes) {
            return Err("Remote playlist permissions no longer allow every reviewed change".into());
        }
        let account = self.require_account(&binding.provider).await?;
        if account.id != stored.account_id || account.id != binding.account_id {
            return Err("Provider account changed after preview".into());
        }
        self.verify_mutation_account(&binding.provider, &account.id)
            .await?;
        if !self.write_sync_enabled(&binding.provider)? {
            return Err("Provider write sync was disabled after this preview was created".into());
        }
        // Remote checks yield; reject local edits made while those checks were pending.
        self.validate_local_preview(&stored, &binding)?;
        if binding.provider == "spotify" && self.bridge.native_write_sync_enabled() {
            return self
                .apply_native_spotify_sync_preview(stored, binding, fresh, account)
                .await;
        }
        if binding.provider == "youtube" {
            return self
                .apply_youtube_sync_preview(stored, binding, fresh, account)
                .await;
        }
        // Consume before the first mutation: a lost result may mean Spotify committed the write.
        self.previews
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .remove(preview_id);
        // Each mutation is sent once. If the result is lost, the caller must refresh and preview again.
        for removal in &changes.remove {
            for batch in removal.entry_ids.chunks(100) {
                if !self.write_sync_enabled("spotify")? {
                    return Err("Spotify write sync was disabled during apply".into());
                }
                self.bridge
                    .provider_operation(
                        "spotify",
                        "removeTracks",
                        json!({"playlistId":binding.playlist_id,"entryIds":batch}),
                    )
                    .await?;
                self.verify_mutation_account("spotify", &account.id).await?;
            }
        }
        let additions = changes
            .add
            .iter()
            .map(|addition| addition.track_uri.clone())
            .collect::<Vec<_>>();
        for batch in additions.chunks(100) {
            if batch.is_empty() {
                continue;
            }
            if !self.write_sync_enabled("spotify")? {
                return Err("Spotify write sync was disabled during apply".into());
            }
            self.bridge
                .provider_operation(
                    "spotify",
                    "addTracks",
                    json!({"playlistId":binding.playlist_id,"trackUris":batch}),
                )
                .await?;
            self.verify_mutation_account("spotify", &account.id).await?;
        }
        let after_add = self
            .fetch_remote(
                "spotify",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        if after_add.account_id.as_deref() != Some(account.id.as_str()) {
            return Err(
                "Playlist changed, became read-only, or contains unavailable entries during sync"
                    .into(),
            );
        }
        ensure_writable(
            "spotify",
            after_add.can_edit,
            after_add.snapshot.complete,
            &after_add.snapshot.entries,
        )?;
        let desired_entry_ids = match_entry_ids(&after_add.snapshot.entries, &stored.desired_uris)?;
        let move_plan = reorder_plan(
            &after_add
                .snapshot
                .entries
                .iter()
                .map(|entry| entry.entry_id.clone())
                .collect::<Vec<_>>(),
            &desired_entry_ids,
        )?;
        if move_plan.len() > 500 {
            return Err("This ordering change exceeds the 500-operation safety limit".into());
        }
        for movement in &move_plan {
            if !self.write_sync_enabled("spotify")? {
                return Err("Spotify write sync was disabled during apply".into());
            }
            self.bridge.provider_operation("spotify", "moveTracks", json!({"playlistId":binding.playlist_id,"entryIds":movement.entry_ids,"beforeEntryId":movement.before_entry_id})).await?;
            self.verify_mutation_account("spotify", &account.id).await?;
        }
        let verified = self
            .fetch_remote(
                "spotify",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        if verified.account_id.as_deref() != Some(account.id.as_str())
            || verified
                .snapshot
                .entries
                .iter()
                .map(|entry| format!("spotify:track:{}", entry.track_id))
                .collect::<Vec<_>>()
                != stored.desired_uris
        {
            return Err("Spotify did not confirm the exact requested playlist order; refresh and review a new preview".into());
        }
        ensure_writable(
            "spotify",
            verified.can_edit,
            verified.snapshot.complete,
            &verified.snapshot.entries,
        )?;
        self.verify_mutation_account("spotify", &account.id).await?;
        let binding_id = stored.preview.binding_id.clone();
        self.persist_fetched(
            "spotify",
            &binding.playlist_id,
            Some(&binding_id),
            true,
            verified,
        )?;
        let result = self.make_preview(&binding_id, vec![]).await?;
        Ok(RemotePlaylistSyncPreview { applied: true, summary: "Spotify accepted the planned changes. Review the refreshed diff before any further write.".into(), ..result })
    }

    async fn apply_youtube_sync_preview(
        &self,
        stored: StoredPreview,
        binding: RemotePlaylistBindingStatus,
        fresh: FetchedPlaylist,
        account: CachedAccount,
    ) -> Result<RemotePlaylistSyncPreview> {
        let change_count = stored
            .preview
            .changes
            .remove
            .iter()
            .map(|change| change.entry_ids.len())
            .sum::<usize>()
            + stored.preview.changes.add.len()
            + stored.preview.changes.move_entries.len();
        if change_count > 500 {
            return Err("This YouTube preview exceeds the 500-operation safety limit; split it into smaller reviewed changes".into());
        }
        let entry_by_id = fresh
            .snapshot
            .entries
            .iter()
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect::<HashMap<_, _>>();
        let mut initial_actions = Vec::new();
        for removal in &stored.preview.changes.remove {
            for entry_id in &removal.entry_ids {
                let entry = entry_by_id
                    .get(entry_id.as_str())
                    .ok_or("A reviewed YouTube occurrence disappeared")?;
                if !entry.available || !valid_youtube_video_id(&entry.track_id) {
                    return Err(
                        "The reviewed YouTube playlist contains an unavailable occurrence".into(),
                    );
                }
                initial_actions.push(
                    json!({"type":"remove","entryId":entry.entry_id,"trackId":entry.track_id}),
                );
            }
        }
        for addition in &stored.preview.changes.add {
            let track_id = addition
                .track_uri
                .strip_prefix("youtube:video:")
                .filter(|id| valid_youtube_video_id(id))
                .ok_or("The reviewed preview contains an invalid YouTube video ID")?;
            initial_actions.push(json!({"type":"add","trackId":track_id}));
        }
        let expected_entries = youtube_worker_snapshot(&fresh.snapshot.entries);
        let sync_preview = json!({
            "previewId": stored.preview.preview_id,
            "accountId": account.id,
            "baselineRevision": stored.preview.baseline_revision,
            "remoteRevision": stored.preview.remote_revision,
            "remoteHash": stored.remote_hash,
        });
        if sync_preview.get("accountId").and_then(Value::as_str) != Some(account.id.as_str())
            || sync_preview
                .get("remoteHash")
                .and_then(Value::as_str)
                .is_none_or(|hash| hash.len() != 64)
        {
            return Err("The reviewed YouTube sync preview is invalid; build a new preview".into());
        }
        self.previews
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .remove(&stored.preview.preview_id);
        if !initial_actions.is_empty() {
            let initial_action_count = initial_actions.len();
            if !self.write_sync_enabled("youtube")? {
                return Err("YouTube write sync was disabled during apply".into());
            }
            let receipt = self
                .bridge
                .provider_operation(
                    "youtube",
                    "applyPlaylistEdits",
                    json!({
                        "previewApproved": true,
                        "playlistId": binding.playlist_id,
                        "expectedEntries": expected_entries.clone(),
                        "actions": initial_actions,
                        "syncPreview": sync_preview,
                    }),
                )
                .await?;
            self.ensure_response_account("youtube", &receipt, &account.id)?;
            if receipt.get("applied").and_then(Value::as_u64) != Some(initial_action_count as u64) {
                return Err("YouTube returned an incomplete mutation receipt. Refresh the playlist before any retry".into());
            }
            self.verify_mutation_account("youtube", &account.id).await?;
        }

        let after_initial = self
            .fetch_remote(
                "youtube",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        if after_initial.account_id.as_deref() != Some(account.id.as_str()) {
            return Err(
                "YouTube account changed after playlist update; refresh before retrying".into(),
            );
        }
        ensure_playlist_capture(
            "youtube",
            after_initial.snapshot.complete,
            &after_initial.snapshot.entries,
        )?;
        let desired_entry_ids =
            match_youtube_entry_ids(&after_initial.snapshot.entries, &stored.desired_uris)?;
        let current_entry_ids = after_initial
            .snapshot
            .entries
            .iter()
            .map(|entry| entry.entry_id.clone())
            .collect::<Vec<_>>();
        let move_plan = reorder_plan(&current_entry_ids, &desired_entry_ids)?;
        if move_plan.len() > 500 {
            return Err(
                "This YouTube ordering change exceeds the 500-operation safety limit".into(),
            );
        }
        if !move_plan.is_empty() {
            if !effective_write_capabilities(
                stored.preview.can_edit,
                stored.preview.write_capabilities,
            )
            .reorder
                || !effective_write_capabilities(
                    after_initial.can_edit,
                    after_initial.write_capabilities,
                )
                .reorder
            {
                return Err("YouTube accepted initial changes but reorder permission is unavailable. Refresh and review a new preview before any further write".into());
            }
            let after_entries_by_id = after_initial
                .snapshot
                .entries
                .iter()
                .map(|entry| (entry.entry_id.as_str(), entry))
                .collect::<HashMap<_, _>>();
            let moves = move_plan.iter().map(|movement| {
                let entry_id = movement.entry_ids.first().ok_or("A YouTube move preview is empty")?;
                let entry = after_entries_by_id.get(entry_id.as_str()).ok_or("A YouTube move occurrence disappeared")?;
                if !entry.available || !valid_youtube_video_id(&entry.track_id) {
                    return Err("The reviewed YouTube playlist contains an unavailable occurrence".into());
                }
                Ok(json!({"type":"moveBefore","entryId":entry.entry_id,"trackId":entry.track_id,"successorEntryId":movement.before_entry_id}))
            }).collect::<Result<Vec<_>>>()?;
            if !self.write_sync_enabled("youtube")? {
                return Err("YouTube write sync was disabled during apply".into());
            }
            let receipt = self
                .bridge
                .provider_operation(
                    "youtube",
                    "applyPlaylistEdits",
                    json!({
                        "previewApproved": true,
                        "playlistId": binding.playlist_id,
                        "expectedEntries": youtube_worker_snapshot(&after_initial.snapshot.entries),
                        "actions": moves,
                        "syncPreview": sync_preview.clone(),
                    }),
                )
                .await?;
            self.ensure_response_account("youtube", &receipt, &account.id)?;
            if receipt.get("applied").and_then(Value::as_u64) != Some(move_plan.len() as u64) {
                return Err("YouTube returned an incomplete ordering receipt. Refresh the playlist before any retry".into());
            }
            self.verify_mutation_account("youtube", &account.id).await?;
        }
        let verified = self
            .fetch_remote(
                "youtube",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        if verified.account_id.as_deref() != Some(account.id.as_str())
            || verified
                .snapshot
                .entries
                .iter()
                .map(|entry| format!("youtube:video:{}", entry.track_id))
                .collect::<Vec<_>>()
                != stored.desired_uris
        {
            return Err("YouTube did not confirm the requested playlist order. Refresh and review a new sync preview".into());
        }
        ensure_playlist_capture(
            "youtube",
            verified.snapshot.complete,
            &verified.snapshot.entries,
        )?;
        self.verify_mutation_account("youtube", &account.id).await?;
        let binding_id = stored.preview.binding_id.clone();
        self.persist_fetched(
            "youtube",
            &binding.playlist_id,
            Some(&binding_id),
            true,
            verified,
        )?;
        let result = self.make_preview(&binding_id, vec![]).await?;
        Ok(RemotePlaylistSyncPreview { applied: true, summary: "YouTube accepted the planned changes. Review the refreshed diff before any further write.".into(), ..result })
    }

    async fn apply_native_spotify_sync_preview(
        &self,
        stored: StoredPreview,
        binding: RemotePlaylistBindingStatus,
        fresh: FetchedPlaylist,
        account: CachedAccount,
    ) -> Result<RemotePlaylistSyncPreview> {
        let removed = stored
            .preview
            .changes
            .remove
            .iter()
            .flat_map(|change| change.entry_ids.iter().cloned())
            .collect::<Vec<_>>();
        let added = stored
            .preview
            .changes
            .add
            .iter()
            .map(|change| change.track_uri.clone())
            .collect::<Vec<_>>();
        let action_count = removed.len() + added.len() + stored.preview.changes.move_entries.len();
        if action_count > 500 {
            return Err("This Spotify preview exceeds the 500-operation safety limit; split it into smaller reviewed changes".into());
        }
        let expected_entries = fresh.snapshot.entries.iter().map(|entry| {
            json!({"entryId":entry.entry_id,"trackId":if entry.available { Some(entry.track_id.as_str()) } else { None }})
        }).collect::<Vec<_>>();
        let args = json!({
            "previewApproved": true,
            "playlistId": binding.playlist_id,
            "expectedEntries": expected_entries,
            "removeEntryIds": removed,
            "addTrackUris": added,
            "desiredTrackUris": stored.desired_uris,
            "syncPreview": {
                "previewId": stored.preview.preview_id,
                "accountId": account.id,
                "baselineRevision": stored.preview.baseline_revision,
                "remoteRevision": stored.preview.remote_revision,
                "remoteHash": stored.remote_hash,
            },
        });
        self.previews
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .remove(&stored.preview.preview_id);
        let receipt = self
            .bridge
            .provider_operation("spotify", "applySyncPreview", args)
            .await?;
        self.ensure_response_account("spotify", &receipt, &account.id)?;
        if receipt
            .get("applied")
            .and_then(Value::as_u64)
            .is_none_or(|count| count > 500)
            || receipt.get("status").and_then(Value::as_str) != Some("VERIFIED")
            || receipt.get("playlistId").and_then(Value::as_str)
                != Some(binding.playlist_id.as_str())
        {
            return Err("Spotify may have received playlist changes without a complete receipt. Refresh the playlist before any retry".into());
        }
        self.verify_mutation_account("spotify", &account.id).await?;
        let verified = self
            .fetch_remote(
                "spotify",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        if verified.account_id.as_deref() != Some(account.id.as_str())
            || verified
                .snapshot
                .entries
                .iter()
                .map(|entry| format!("spotify:track:{}", entry.track_id))
                .collect::<Vec<_>>()
                != stored.desired_uris
        {
            return Err("Spotify did not confirm the requested playlist order. Refresh the playlist before any retry".into());
        }
        ensure_writable(
            "spotify",
            verified.can_edit,
            verified.snapshot.complete,
            &verified.snapshot.entries,
        )?;
        self.verify_mutation_account("spotify", &account.id).await?;
        let binding_id = stored.preview.binding_id.clone();
        self.persist_fetched(
            "spotify",
            &binding.playlist_id,
            Some(&binding_id),
            true,
            verified,
        )?;
        let result = self.make_preview(&binding_id, vec![]).await?;
        Ok(RemotePlaylistSyncPreview { applied: true, summary: "Spotify accepted the reviewed changes. Review the refreshed diff before any further write.".into(), ..result })
    }

    async fn capture_and_bind(
        &self,
        provider: &str,
        playlist_id: &str,
        existing_binding: Option<&str>,
        force_baseline: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<RemotePlaylistImportItem> {
        let fetched = self.fetch_remote(provider, playlist_id, cancel).await?;
        self.persist_fetched(
            provider,
            playlist_id,
            existing_binding,
            force_baseline,
            fetched,
        )
    }

    fn persist_fetched(
        &self,
        provider: &str,
        playlist_id: &str,
        existing_binding: Option<&str>,
        force_baseline: bool,
        mut fetched: FetchedPlaylist,
    ) -> Result<RemotePlaylistImportItem> {
        let account_id = fetched
            .account_id
            .clone()
            .or_else(|| (provider == "youtube").then(|| "public".to_owned()))
            .ok_or("Provider account identity is unavailable")?;
        // Compare the working copy before importing the immutable snapshot: archive import can
        // enrich shared catalog metadata (for example, adding a newly known duration) and make
        // an untouched playlist look locally edited against its previous baseline.
        let mut state = self.load_state()?;
        let existing = if let Some(binding_id) = existing_binding {
            Some(
                state
                    .bindings
                    .iter()
                    .find(|binding| binding.binding_id == binding_id)
                    .cloned()
                    .ok_or("Remote playlist binding not found")?,
            )
        } else {
            find_bound_playlist(&state.bindings, provider, playlist_id, &account_id).cloned()
        };
        let existing_index = match existing.as_ref() {
            Some(binding) => Some(
                state
                    .bindings
                    .iter()
                    .position(|candidate| candidate.binding_id == binding.binding_id)
                    .ok_or("Remote playlist binding not found")?,
            ),
            None => None,
        };
        let replace_working_copy = if let Some(binding) = existing.as_ref() {
            if binding.account_id != account_id {
                return Err("Provider account differs from the original playlist owner".into());
            }
            let current = self
                .store
                .list_tracks("collection", Some(&binding.collection_id))?;
            preserve_known_durations(&mut fetched.chart.tracks, &current);
            !force_baseline
                && state
                    .baselines
                    .get(&binding.binding_id)
                    .is_some_and(|baseline| {
                        should_replace_working_copy(
                            &current,
                            baseline,
                            &binding.provider,
                            force_baseline,
                        )
                    })
        } else {
            false
        };
        let mut new_missing_spotify_ids = Vec::new();
        if provider == "spotify" {
            let mut seen = HashSet::new();
            for track in &fetched.chart.tracks {
                if !is_duration_backfill_eligible(track) || !seen.insert(track.provider_id.clone())
                {
                    continue;
                }
                let needs_duration = self
                    .store
                    .get_track(&track.id)?
                    .as_ref()
                    .is_none_or(is_duration_backfill_eligible);
                if needs_duration {
                    new_missing_spotify_ids.push(track.provider_id.clone());
                }
            }
        }
        let source_collection_id = self.archive_snapshot(provider, &fetched)?;
        if provider == "spotify" {
            self.rearm_spotify_duration_backfill_for_new_rows(
                &account_id,
                &new_missing_spotify_ids,
            )?;
        }
        if let (Some(index), Some(mut binding)) = (existing_index, existing) {
            if (replace_working_copy || force_baseline) && fetched.snapshot.complete {
                // An applied preview already represents the user's exact working copy.
                // Advance its remote baseline without replacing cross-catalog identities
                // or local tags with the provider's freshly fetched catalog records.
                if replace_working_copy {
                    self.store.replace_playlist_entries(
                        &binding.collection_id,
                        &expand_playlist_occurrences(fetched.chart.tracks.clone()),
                    )?;
                }
                state
                    .baselines
                    .insert(binding.binding_id.clone(), fetched.snapshot.clone());
            }
            state
                .observed
                .insert(binding.binding_id.clone(), fetched.snapshot.clone());
            binding.title = fetched.title.clone();
            binding.url = fetched.url.clone();
            binding.owner_id = fetched.owner_id.clone().or(binding.owner_id);
            binding.owner_name = fetched.owner_name.clone().or(binding.owner_name);
            binding.owner_url = fetched.owner_url.clone().or(binding.owner_url);
            binding.visibility = fetched.visibility.clone().or(binding.visibility);
            binding.source_collection_id = Some(source_collection_id);
            binding.track_count = fetched.snapshot.entries.len();
            binding.revision = fetched.snapshot.revision.clone();
            binding.complete = fetched.snapshot.complete;
            binding.can_edit = fetched.can_edit;
            binding.write_capabilities = fetched.write_capabilities;
            binding.refreshed_at = chrono::Utc::now().to_rfc3339();
            state.bindings[index] = binding.clone();
            self.save_state(&state)?;
            self.notify_changed();
            return Ok(import_item(&binding, &fetched));
        }
        let collection = self.store.create_collection(&fetched.title)?;
        let ordered = expand_playlist_occurrences(fetched.chart.tracks.clone());
        self.store
            .replace_playlist_entries(&collection.id, &ordered)?;
        let binding = RemotePlaylistBindingStatus {
            write_capabilities: fetched.write_capabilities,
            binding_id: uuid::Uuid::new_v4().to_string(),
            provider: provider.into(),
            playlist_id: playlist_id.into(),
            title: fetched.title.clone(),
            url: fetched.url.clone(),
            account_id,
            owner_id: fetched.owner_id.clone(),
            owner_name: fetched.owner_name.clone(),
            owner_url: fetched.owner_url.clone(),
            visibility: fetched.visibility.clone(),
            collection_id: collection.id.clone(),
            source_collection_id: Some(source_collection_id),
            collection_name: collection.name,
            track_count: ordered.len(),
            revision: fetched.snapshot.revision.clone(),
            complete: fetched.snapshot.complete,
            can_edit: fetched.can_edit,
            refreshed_at: chrono::Utc::now().to_rfc3339(),
        };
        let mut state = self.load_state()?;
        state.bindings.push(binding.clone());
        state
            .baselines
            .insert(binding.binding_id.clone(), fetched.snapshot.clone());
        state
            .observed
            .insert(binding.binding_id.clone(), fetched.snapshot.clone());
        self.save_state(&state)?;
        self.notify_changed();
        Ok(import_item(&binding, &fetched))
    }

    /// A successful Spotify import may add missing catalog rows after the prior bounded
    /// scan exhausted (or below its saved cursor). Restart only when that import actually
    /// introduced a still-missing eligible Spotify listing.
    fn rearm_spotify_duration_backfill_for_new_rows(
        &self,
        account_id: &str,
        candidate_ids: &[String],
    ) -> Result<()> {
        if candidate_ids.is_empty() {
            return Ok(());
        }
        let mut still_missing = false;
        for provider_id in candidate_ids {
            let id = format!("spotify:{provider_id}");
            if self
                .store
                .get_track(&id)?
                .as_ref()
                .is_some_and(is_duration_backfill_eligible)
            {
                still_missing = true;
                break;
            }
        }
        if !still_missing {
            return Ok(());
        }
        {
            // Serialize cursor reset with the worker's guarded commit. Removing the exact
            // active token prevents an old generation from committing after the reset.
            let _commit_gate = self
                .duration_backfill_commit_gate
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            let mut active = self
                .duration_backfill_cancel
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            if let Some(cancel) = active.remove(&DurationBackfillScope::AnonymousPublic) {
                cancel.store(true, Ordering::SeqCst);
            }
            self.store.set_json_preference(
                &DurationBackfillScope::AnonymousPublic.cursor_key(),
                &Value::Null,
            )?;
            let current_account = self
                .account_cache
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?
                .get("spotify")
                .is_some_and(|account| account.id == account_id);
            if !current_account {
                return Ok(());
            }
            if let Some(cancel) =
                active.remove(&DurationBackfillScope::Account(account_id.to_owned()))
            {
                cancel.store(true, Ordering::SeqCst);
            }
            self.duration_backfill_scheduled
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?
                .remove(&DurationBackfillScope::Account(account_id.to_owned()));
            self.store.set_json_preference(
                &DurationBackfillScope::Account(account_id.to_owned()).cursor_key(),
                &Value::Null,
            )?;
        }
        self.schedule_existing_spotify_duration_backfill(account_id.to_owned());
        Ok(())
    }

    fn schedule_existing_spotify_duration_backfill(&self, account_id: String) {
        let scope = DurationBackfillScope::Account(account_id.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let Ok(_commit_gate) = self.duration_backfill_commit_gate.lock() else {
                return;
            };
            let Ok(accounts) = self.account_cache.lock() else {
                return;
            };
            if !accounts
                .get("spotify")
                .is_some_and(|confirmed| confirmed.id == account_id)
            {
                return;
            }
            drop(accounts);
            let Ok(mut active) = self.duration_backfill_cancel.lock() else {
                return;
            };
            if active.contains_key(&DurationBackfillScope::AnonymousPublic) {
                if let Ok(mut deferred) = self.duration_backfill_deferred.lock() {
                    deferred.insert(account_id.clone());
                }
                return;
            }
            if active.contains_key(&scope) {
                return;
            }
            let Ok(mut scheduled) = self.duration_backfill_scheduled.lock() else {
                return;
            };
            // A confirmed account change is an explicit retry trigger for any prior account.
            scheduled.retain(|scheduled_scope| scheduled_scope == &scope);
            let already_scheduled = scheduled.contains(&scope);
            for (account, flag) in active.iter() {
                if account != &scope {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            active.retain(|active_scope, _| active_scope == &scope);
            if already_scheduled {
                active.clear();
                return;
            }
            active.insert(scope.clone(), cancel.clone());
            scheduled.insert(scope.clone());
        }
        let service = self.clone();
        tokio::spawn(async move {
            let mut retried_transient_failure = false;
            loop {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                match service
                    .backfill_existing_spotify_durations(&account_id, &cancel)
                    .await
                {
                    Ok((_, true, _)) => break,
                    Ok((_, false, processed)) if processed > 0 => {
                        retried_transient_failure = false;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(EXISTING_SPOTIFY_DURATION_BACKFILL_IDLE_MS)) => {},
                            _ = async { while !cancel.load(Ordering::SeqCst) { tokio::time::sleep(std::time::Duration::from_millis(50)).await; } } => break,
                        }
                    }
                    // RATE_LIMITED has no trustworthy retry-after hint in the current
                    // bridge error contract. Keep the scheduled marker and stop until an
                    // explicit reconnect/retry trigger rather than hammering after 1s.
                    Err(ref error)
                        if !permanent_duration_lookup_error(error)
                            && !rate_limited_duration_lookup_error(error)
                            && !retried_transient_failure
                            && !cancel.load(Ordering::SeqCst) =>
                    {
                        retried_transient_failure = true;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(EXISTING_SPOTIFY_DURATION_BACKFILL_IDLE_MS)) => {},
                            _ = async { while !cancel.load(Ordering::SeqCst) { tokio::time::sleep(std::time::Duration::from_millis(50)).await; } } => break,
                        }
                    }
                    Ok((_, false, _)) | Err(_) => break,
                }
            }
            if let Ok(mut active) = service.duration_backfill_cancel.lock() {
                if active
                    .get(&scope)
                    .is_some_and(|current| Arc::ptr_eq(current, &cancel))
                {
                    active.remove(&scope);
                }
            }
        });
    }

    /// Explicit public metadata repair. This has no account identity and makes no provider
    /// status calls. It processes one bounded slice and resumes from its own cursor on a later
    /// invocation. The same generation gate prevents overlap with authenticated repair.
    pub async fn repair_spotify_durations_public(&self) -> Result<SpotifyDurationRepairResult> {
        let scope = DurationBackfillScope::AnonymousPublic;
        let now = tokio::time::Instant::now();
        {
            let mut backoff = self
                .public_duration_backoff_until
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            if backoff.is_some_and(|until| until > now) {
                return Err("RATE_LIMITED: public Spotify repair is cooling down".into());
            }
            *backoff = None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let _gate = self
                .duration_backfill_commit_gate
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            let mut active = self
                .duration_backfill_cancel
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?;
            if !active.is_empty() {
                return Err("Spotify duration repair is already running".into());
            }
            self.duration_backfill_scheduled
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?
                .clear();
            active.insert(scope.clone(), cancel.clone());
        }
        let _lease = DurationBackfillLease {
            service: self.clone(),
            scope: scope.clone(),
            cancel: cancel.clone(),
        };
        let result = tokio::select! {
            biased;
            _ = wait_for_duration_backfill_cancel(&cancel) => Err("Public Spotify duration repair cancelled".to_owned()),
            result = self.backfill_duration_scope(
                &scope,
                &cancel,
                std::time::Duration::from_secs(EXISTING_SPOTIFY_DURATION_BACKFILL_DEADLINE_SECS),
            ) => result,
        };
        if let Err(error) = result.as_ref() {
            if rate_limited_duration_lookup_error(error) {
                let seconds = duration_rate_limit_cooldown_seconds(error);
                if let Ok(mut backoff) = self.public_duration_backoff_until.lock() {
                    *backoff =
                        Some(tokio::time::Instant::now() + std::time::Duration::from_secs(seconds));
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Err("Public Spotify duration repair cancelled".into());
        }
        let (changed, complete, processed) = result?;
        Ok(SpotifyDurationRepairResult {
            changed,
            processed,
            complete,
        })
    }

    pub fn cancel_public_spotify_duration_repair(&self) -> Result<bool> {
        let _gate = self
            .duration_backfill_commit_gate
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?;
        let active = self
            .duration_backfill_cancel
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?;
        let Some(cancel) = active.get(&DurationBackfillScope::AnonymousPublic) else {
            return Ok(false);
        };
        cancel.store(true, Ordering::SeqCst);
        Ok(true)
    }

    pub fn public_spotify_duration_repair_running(&self) -> bool {
        self.duration_backfill_cancel
            .lock()
            .is_ok_and(|active| active.contains_key(&DurationBackfillScope::AnonymousPublic))
    }

    /// Backfill already-imported Spotify listings after an account is confirmed. This path is
    /// independent of remote-playlist bindings and only writes guarded duration metadata.
    /// Each invocation processes at most 300 rows and 60 seconds, then yields before continuing.
    async fn backfill_existing_spotify_durations(
        &self,
        expected_account: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(usize, bool, usize)> {
        self.backfill_duration_scope(
            &DurationBackfillScope::Account(expected_account.to_owned()),
            cancel,
            std::time::Duration::from_secs(EXISTING_SPOTIFY_DURATION_BACKFILL_DEADLINE_SECS),
        )
        .await
    }

    async fn backfill_existing_spotify_durations_with_budget(
        &self,
        expected_account: &str,
        cancel: &Arc<AtomicBool>,
        budget: std::time::Duration,
    ) -> Result<(usize, bool, usize)> {
        self.backfill_duration_scope(
            &DurationBackfillScope::Account(expected_account.to_owned()),
            cancel,
            budget,
        )
        .await
    }

    async fn backfill_duration_scope(
        &self,
        scope: &DurationBackfillScope,
        cancel: &Arc<AtomicBool>,
        budget: std::time::Duration,
    ) -> Result<(usize, bool, usize)> {
        let mut total_changed = 0usize;
        let run = async {
            let deadline = tokio::time::Instant::now() + budget;
            let cursor_key = scope.cursor_key();
            let mut after = self
                .store
                .get_json_preference(&cursor_key)?
                .and_then(|value| value.as_str().map(str::to_owned));
            let mut processed = 0usize;
            let mut exhausted = false;
            while processed < MAX_EXISTING_SPOTIFY_DURATION_BACKFILL
                && !cancel.load(Ordering::SeqCst)
                && tokio::time::Instant::now() < deadline
            {
                let ids = self.store.missing_spotify_duration_ids(
                    after.as_deref(),
                    EXISTING_SPOTIFY_DURATION_BACKFILL_BATCH,
                )?;
                let Some(last_id) = ids.last().cloned() else {
                    // Keep the public cursor at the last inspected ID so a repeated explicit
                    // repair does not refetch the entire exhausted library. Imports reset this
                    // independent cursor when they introduce eligible missing rows.
                    if matches!(scope, DurationBackfillScope::Account(_)) {
                        self.store.set_json_preference(&cursor_key, &Value::Null)?;
                    }
                    exhausted = true;
                    break;
                };
                let mut matches = HashMap::<String, f64>::new();
                let mut completed_ids = Vec::new();
                for request_batch in ids.chunks(EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY) {
                    if cancel.load(Ordering::SeqCst) || tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    let batch_deadline = (tokio::time::Instant::now()
                        + std::time::Duration::from_secs(20))
                    .min(deadline);
                    let rate_limited = Arc::new(AtomicBool::new(false));
                    let requests = futures_util::future::join_all(request_batch.iter().map(|id| {
                        let rate_limited = rate_limited.clone();
                        let scope = scope.clone();
                        async move {
                            let mut found = None;
                            let mut terminal_error = None;
                            let mut lookup_completed = false;
                            for _attempt in 0..2 {
                                if rate_limited.load(Ordering::SeqCst) {
                                    break;
                                }
                                let lookup = async {
                                    match &scope {
                                        DurationBackfillScope::Account(_) => self
                                            .bridge
                                            .provider_operation_cancellable(
                                                "spotify",
                                                "getTrack",
                                                json!({"trackId": id}),
                                                Some(cancel.clone()),
                                            )
                                            .await,
                                        DurationBackfillScope::AnonymousPublic => self
                                            .public_spotify_duration
                                            .duration_seconds(&id)
                                            .await
                                            .map(|duration_seconds| {
                                                json!({"id":id,"durationSeconds":duration_seconds})
                                            }),
                                    }
                                };
                                match tokio::time::timeout_at(batch_deadline, lookup).await {
                                    Ok(Ok(page)) => {
                                        found = Some(page);
                                        lookup_completed = true;
                                        break;
                                    }
                                    Ok(Err(error))
                                        if permanent_duration_lookup_error(&error)
                                            || rate_limited_duration_lookup_error(&error) =>
                                    {
                                        if rate_limited_duration_lookup_error(&error) {
                                            rate_limited.store(true, Ordering::SeqCst);
                                        }
                                        terminal_error = Some(error);
                                        break;
                                    }
                                    Ok(Err(error)) if error.starts_with("TRACK_NOT_FOUND:") => {
                                        lookup_completed = true;
                                        break;
                                    }
                                    _ if tokio::time::Instant::now() < batch_deadline => continue,
                                    _ => break,
                                }
                            }
                            (id.clone(), found, terminal_error, lookup_completed)
                        }
                    }));
                    let mut requests = Box::pin(tokio::time::timeout_at(batch_deadline, requests));
                    let results = loop {
                        tokio::select! {
                            result = &mut requests => break result.ok(),
                            _ = tokio::time::sleep(std::time::Duration::from_millis(40)) => {
                                if cancel.load(Ordering::SeqCst) { break None; }
                            }
                        }
                    };
                    let Some(results) = results else {
                        if rate_limited.load(Ordering::SeqCst) {
                            return Err(
                                "RATE_LIMITED: Spotify catalog request was rate limited".into()
                            );
                        }
                        break;
                    };
                    if let Some(error) = results.iter().find_map(|(_, _, error, _)| error.as_ref())
                    {
                        return Err(error.clone());
                    }
                    if results.iter().any(|(_, _, _, completed)| !completed) {
                        break;
                    }
                    completed_ids.extend(request_batch.iter().cloned());
                    for (id, result, _, _) in results {
                        let Some(page) = result else {
                            continue;
                        };
                        match scope {
                            DurationBackfillScope::Account(expected_account) => {
                                if page.get("accountId").and_then(Value::as_str)
                                    != Some(expected_account.as_str())
                                {
                                    return Err("Provider account changed during existing duration backfill".into());
                                }
                                if let Some(duration) = page
                                    .get("items")
                                    .and_then(Value::as_array)
                                    .and_then(|items| {
                                        items
                                            .iter()
                                            .find_map(|item| verified_spotify_duration(item, &id))
                                    })
                                {
                                    matches.insert(id, duration);
                                }
                            }
                            DurationBackfillScope::AnonymousPublic => {
                                if page.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                                    return Err("PUBLIC_TRACK_MISMATCH: response track ID differed from request".into());
                                }
                                if let Some(duration) = page
                                    .get("durationSeconds")
                                    .and_then(Value::as_f64)
                                    .filter(|duration| {
                                        duration.is_finite()
                                            && *duration > 0.0
                                            && *duration <= 86_400.0
                                    })
                                {
                                    matches.insert(id, duration);
                                } else {
                                    return Err("PUBLIC_TRACK_INVALID: verified duration was missing or out of range".into());
                                }
                            }
                        }
                    }
                }
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                if completed_ids.is_empty() {
                    return Err("Provider search batch made no progress before its deadline".into());
                }
                if let Some(expected_account) = scope.account_id() {
                    let status = tokio::time::timeout_at(deadline, async {
                        tokio::select! {
                            biased;
                            _ = async { while !cancel.load(Ordering::SeqCst) { tokio::time::sleep(std::time::Duration::from_millis(40)).await; } } => Err("Duration backfill cancelled".to_owned()),
                            result = self.bridge.provider_operation("spotify", "status", json!({})) => result,
                        }
                    }).await.map_err(|_| "Provider status timed out during duration backfill".to_owned())??;
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let status_account = self.optional_result_account("spotify", &status)?;
                    if status_account.as_ref().map(|account| account.id.as_str())
                        != Some(expected_account)
                    {
                        return Err(
                            "Provider account changed during existing duration backfill".into()
                        );
                    }
                }
                let candidates = matches.into_iter().collect::<Vec<_>>();
                {
                    let _commit_gate = self
                        .duration_backfill_commit_gate
                        .lock()
                        .map_err(|_| "Remote playlist state unavailable")?;
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let active = self
                        .duration_backfill_cancel
                        .lock()
                        .map_err(|_| "Remote playlist state unavailable")?;
                    if !active
                        .get(scope)
                        .is_some_and(|current| Arc::ptr_eq(current, cancel))
                    {
                        return Err("Duration backfill generation is no longer active".into());
                    }
                    if !candidates.is_empty() {
                        total_changed += self.store.fill_missing_spotify_durations(&candidates)?;
                    }
                    processed += completed_ids.len();
                    let completed_through = completed_ids.last().cloned().unwrap_or(last_id);
                    after = Some(completed_through.clone());
                    self.store
                        .set_json_preference(&cursor_key, &json!(completed_through))?;
                }
                if completed_ids.len() < ids.len() || tokio::time::Instant::now() >= deadline {
                    break;
                }
            }
            Ok((total_changed, exhausted, processed))
        };
        let run = run.await;
        if total_changed > 0 {
            self.notify_changed();
        }
        run
    }

    /// Fill gaps from verified Spotify track search results during playlist import/refresh.
    /// This is deliberately capped and time bounded so imports remain usable when the provider
    /// search is slow; track IDs and returned IDs must match exactly before any write occurs.
    async fn backfill_spotify_durations(
        &self,
        tracks: &mut [Track],
        expected_account: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<()> {
        let mut seen_ids = HashSet::new();
        let ids = tracks
            .iter()
            .filter(|track| is_duration_backfill_eligible(track))
            .filter_map(|track| {
                seen_ids
                    .insert(track.provider_id.clone())
                    .then(|| track.provider_id.clone())
            })
            .take(MAX_SPOTIFY_DURATION_BACKFILL)
            .collect::<Vec<_>>();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
        let mut matches = HashMap::<String, f64>::new();
        let mut rate_limited = false;
        for batch in ids.chunks(SPOTIFY_DURATION_BACKFILL_CONCURRENCY) {
            if cancel.load(Ordering::SeqCst) || tokio::time::Instant::now() >= deadline {
                break;
            }
            let requests = futures_util::future::join_all(batch.iter().map(|id| async move {
                let result = self
                    .search_catalog("spotify", id, vec!["track".into()], None)
                    .await;
                (id.clone(), result)
            }));
            let Ok(results) = tokio::time::timeout_at(deadline, requests).await else {
                break;
            };
            for (id, result) in results {
                let page = match result {
                    Ok(page) => page,
                    Err(error) if rate_limited_duration_lookup_error(&error) => {
                        rate_limited = true;
                        break;
                    }
                    Err(_) => continue,
                };
                if page
                    .get("accountId")
                    .and_then(Value::as_str)
                    .is_some_and(|account_id| account_id != expected_account)
                {
                    return Err("Provider account changed during duration backfill".into());
                }
                let Some(items) = page.get("items").and_then(Value::as_array) else {
                    continue;
                };
                let duration = items
                    .iter()
                    .find_map(|item| verified_spotify_duration(item, &id));
                if let Some(duration) = duration {
                    matches.insert(id, duration);
                }
            }
            if rate_limited {
                break;
            }
        }
        let candidates = matches
            .iter()
            .map(|(id, duration)| (id.clone(), *duration))
            .collect::<Vec<_>>();
        if !candidates.is_empty() {
            self.store.fill_missing_spotify_durations(&candidates)?;
        }
        apply_verified_spotify_durations(tracks, &matches);
        Ok(())
    }

    async fn fetch_remote(
        &self,
        provider: &str,
        playlist_id: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<FetchedPlaylist> {
        let mut cursor: Option<String> = None;
        let mut rows = Vec::new();
        let mut metadata: Option<Value> = None;
        let mut revision = String::new();
        let mut expected_account = if provider == "spotify" {
            Some(self.require_account(provider).await?.id)
        } else {
            None
        };
        let mut remote_total = None;
        let mut terminal_page_complete = false;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("Import cancelled".into());
            }
            let mut args = json!({"playlistId":playlist_id,"limit":100});
            if let Some(next) = cursor.as_deref() {
                args["cursor"] = json!(next);
            }
            let page = self
                .bridge
                .provider_operation_cancellable(provider, "getPlaylist", args, Some(cancel.clone()))
                .await?;
            let page_account = self
                .optional_result_account(provider, &page)?
                .map(|account| account.id);
            if expected_account.is_none() && page_account.is_some() {
                expected_account.clone_from(&page_account);
            }
            if expected_account
                .as_ref()
                .zip(page_account.as_ref())
                .is_some_and(|(expected, actual)| expected != actual)
            {
                return Err("Provider account changed during playlist capture".into());
            }
            if expected_account.is_some() && page_account.is_none() {
                return Err(
                    "Provider account identity became unavailable during playlist capture".into(),
                );
            }
            let playlist = page
                .get("playlist")
                .and_then(Value::as_object)
                .ok_or("Provider returned invalid playlist details")?;
            if playlist.get("id").and_then(Value::as_str) != Some(playlist_id) {
                return Err("Provider returned a different playlist".into());
            }
            if metadata.is_none() {
                metadata = Some(Value::Object(playlist.clone()));
            }
            remote_total = page
                .get("total")
                .and_then(Value::as_u64)
                .map(|total| total as usize)
                .or_else(|| {
                    playlist
                        .get("total")
                        .and_then(Value::as_u64)
                        .map(|total| total as usize)
                })
                .or(remote_total);
            let page_revision = page
                .get("revision")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if revision.is_empty() {
                revision = page_revision.clone();
            }
            if page_revision != revision {
                return Err("Playlist changed while its pages were being captured".into());
            }
            let entries = page
                .get("entries")
                .and_then(Value::as_array)
                .ok_or("Provider returned invalid playlist entries")?;
            for entry in entries {
                let occurrence_index = rows.len();
                rows.push(parse_remote_entry(
                    entry,
                    provider,
                    &occurrence_index.to_string(),
                )?);
            }
            if rows.len() > MAX_PLAYLIST_ENTRIES {
                return Err("Remote playlist exceeds the 10,000 entry limit".into());
            }
            let page_complete = page
                .get("complete")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            observe_page_completion(&mut terminal_page_complete, page_complete, cursor.is_some());
            if cursor.is_none() {
                break;
            }
        }
        let meta = metadata.ok_or("Provider returned no playlist pages")?;
        let title = required_text(&meta, "title", 256)?.to_owned();
        let url = validate_remote_url(required_text(&meta, "url", 2048)?, provider)?;
        let owner_id = bounded_optional_text(&meta, "ownerId", 256);
        let owner_name = bounded_optional_text(&meta, "ownerName", 512);
        let owner_url = normalize_owner_url(
            provider,
            owner_id.as_deref(),
            meta.get("ownerUrl").and_then(Value::as_str),
        );
        let visibility = normalize_visibility(meta.get("visibility").and_then(Value::as_str));
        let write_capabilities = parse_write_capabilities(&meta)?;
        let can_edit = meta
            .get("canEdit")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let tracks = rows
            .iter()
            .enumerate()
            .map(|(index, entry)| entry_to_track(provider, entry, index))
            .collect::<Vec<_>>();
        let total = remote_total.or_else(|| {
            meta.get("total")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
        });
        let complete = capture_is_complete(terminal_page_complete, total, rows.len());
        let snapshot_time = chrono::Utc::now().to_rfc3339();
        let content_hash = remote_entries_content_hash(&rows)?;
        let snapshot = RemoteSnapshot {
            content_hash: content_hash.clone(),
            entries: rows,
            revision: if revision.is_empty() {
                remote_tracks_revision_hash(&tracks)?
            } else {
                revision
            },
            complete,
        };
        let browser_snapshot = BrowserSnapshot {
            schema_version: 1,
            provider: provider.to_owned(),
            kind: "playlist".into(),
            title: title.clone(),
            raw_page_title: None,
            source_url: url.clone(),
            captured_at: snapshot_time.clone(),
            complete: snapshot.complete,
            expected_total: total,
            warnings: if snapshot.complete {
                vec![]
            } else {
                vec!["Provider returned a partial playlist page; imported tracks retain their captured order.".into()]
            },
            entries: snapshot
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| BrowserEntry {
                    provider_id: entry.available.then(|| entry.track_id.clone()),
                    source_url: (!entry.url.is_empty()).then(|| entry.url.clone()),
                    artist: entry.artist.clone(),
                    title: entry.title.clone(),
                    version: None,
                    position: (index + 1) as u32,
                    rank: None,
                    duration_seconds: entry.duration_seconds,
                    bpm: None,
                    key: None,
                    album: entry.album.clone(),
                    label: None,
                    genre: None,
                    release_date: None,
                    artwork_url: entry.artwork_url.clone(),
                    isrc: None,
                    catalog_number: None,
                })
                .collect(),
        };
        let mut chart = validate_snapshot(&browser_snapshot)?;
        preserve_remote_counts(&mut chart.tracks, &snapshot.entries);
        if provider == "spotify" {
            if let Some(account_id) = expected_account.as_deref() {
                self.backfill_spotify_durations(&mut chart.tracks, account_id, cancel)
                    .await?;
            }
        }
        Ok(FetchedPlaylist {
            write_capabilities,
            title,
            url,
            owner_id,
            owner_name,
            owner_url,
            visibility,
            can_edit,
            tracks,
            snapshot,
            account_id: expected_account,
            browser_snapshot,
            chart,
        })
    }

    async fn make_preview(
        &self,
        binding_id: &str,
        reviewed_matches: Vec<ReviewedRemoteTrackMatch>,
    ) -> Result<RemotePlaylistSyncPreview> {
        let binding = self.find_binding(binding_id)?;
        if binding.provider == "youtube" {
            return self
                .make_youtube_preview(binding, binding_id, reviewed_matches)
                .await;
        }
        if binding.provider != "spotify" {
            return Err("Write sync is not available for this provider".into());
        }
        let state = self.load_state()?;
        let baseline = state
            .baselines
            .get(binding_id)
            .cloned()
            .ok_or("Refresh this binding before previewing changes")?;
        let observed = state
            .observed
            .get(binding_id)
            .or_else(|| state.baselines.get(binding_id))
            .cloned()
            .ok_or("Refresh this binding before previewing changes")?;
        let fresh = self
            .fetch_remote(
                "spotify",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        let account = self.require_account("spotify").await?;
        let conflict = account.id != binding.account_id
            || fresh.snapshot.content_hash != observed.content_hash
            || !fresh.snapshot.complete
            || !fresh.can_edit;
        let write_sync_enabled = self.write_sync_enabled("spotify")?;
        let local_tracks = self
            .store
            .list_tracks("collection", Some(&binding.collection_id))?;
        let reviewed_uris = self.resolve_reviewed_matches(
            binding_id,
            "spotify",
            &account.id,
            &local_tracks,
            &reviewed_matches,
        )?;
        let local_identity_hashes = local_tracks
            .iter()
            .map(|track| Ok((track.id.clone(), track_identity_hash(track)?)))
            .collect::<Result<HashMap<_, _>>>()?;
        let local_order_hash = hash_json(
            &local_tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
        )?;
        let mut unresolved = Vec::new();
        let mut local_uris = Vec::new();
        for track in local_tracks {
            if has_confirmed_spotify_identity(&track) {
                local_uris.push((format!("spotify:track:{}", track.provider_id), track));
            } else if let Some(uri) = reviewed_uris.get(&track.id) {
                local_uris.push((uri.clone(), track));
            } else {
                unresolved.push(RemoteUnresolvedChange {
                    entry_id: None,
                    local_track_id: Some(track.id.clone()),
                    title: Some(track.title.clone()),
                    artist: Some(track.artist.clone()),
                    duration_seconds: track.duration_seconds,
                    reason: format!(
                        "{} — {} has no verified Spotify track ID",
                        track.artist, track.title
                    ),
                });
            }
        }
        let desired_uris = local_uris
            .iter()
            .map(|(uri, _)| uri.clone())
            .collect::<Vec<_>>();
        let changes = plan_changes(&fresh.snapshot.entries, &local_uris, unresolved);
        let preview = RemotePlaylistSyncPreview {
            write_capabilities: fresh.write_capabilities,
            preview_id: uuid::Uuid::new_v4().to_string(),
            binding_id: binding_id.into(),
            provider: binding.provider.clone(),
            playlist_id: binding.playlist_id.clone(),
            local_collection_id: binding.collection_id.clone(),
            baseline_revision: baseline.revision,
            remote_revision: fresh.snapshot.revision.clone(),
            conflict,
            applied: false,
            can_edit: fresh.can_edit,
            write_sync_enabled,
            summary: diff_summary(&changes, conflict),
            changes,
        };
        self.previews
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .insert(
                preview.preview_id.clone(),
                StoredPreview {
                    preview: preview.clone(),
                    remote_hash: fresh.snapshot.content_hash,
                    account_id: account.id,
                    desired_uris,
                    local_identity_hashes,
                    local_order_hash,
                },
            );
        Ok(preview)
    }

    async fn make_youtube_preview(
        &self,
        binding: RemotePlaylistBindingStatus,
        binding_id: &str,
        reviewed_matches: Vec<ReviewedRemoteTrackMatch>,
    ) -> Result<RemotePlaylistSyncPreview> {
        let state = self.load_state()?;
        let baseline = state
            .baselines
            .get(binding_id)
            .cloned()
            .ok_or("Refresh this binding before previewing changes")?;
        let observed = state
            .observed
            .get(binding_id)
            .or_else(|| state.baselines.get(binding_id))
            .cloned()
            .ok_or("Refresh this binding before previewing changes")?;
        let fresh = self
            .fetch_remote(
                "youtube",
                &binding.playlist_id,
                &Arc::new(AtomicBool::new(false)),
            )
            .await?;
        let account = self.require_account("youtube").await?;
        let mut conflict = account.id != binding.account_id
            || fresh.snapshot.content_hash != observed.content_hash
            || !fresh.snapshot.complete;
        let write_sync_enabled = self.write_sync_enabled("youtube")?;
        let local_tracks = self
            .store
            .list_tracks("collection", Some(&binding.collection_id))?;
        let reviewed_uris = self.resolve_reviewed_matches(
            binding_id,
            "youtube",
            &account.id,
            &local_tracks,
            &reviewed_matches,
        )?;
        let local_identity_hashes = local_tracks
            .iter()
            .map(|track| Ok((track.id.clone(), track_identity_hash(track)?)))
            .collect::<Result<HashMap<_, _>>>()?;
        let local_order_hash = hash_json(
            &local_tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
        )?;
        let mut local = Vec::new();
        let mut unresolved = Vec::new();
        for track in local_tracks {
            if has_confirmed_youtube_identity(&track) {
                local.push((format!("youtube:video:{}", track.provider_id), track));
            } else if let Some(uri) = reviewed_uris.get(&track.id) {
                local.push((uri.clone(), track));
            } else {
                unresolved.push(RemoteUnresolvedChange {
                    entry_id: None,
                    local_track_id: Some(track.id.clone()),
                    title: Some(track.title.clone()),
                    artist: Some(track.artist.clone()),
                    duration_seconds: track.duration_seconds,
                    reason: format!(
                        "{} — {} has no verified YouTube video ID",
                        track.artist, track.title
                    ),
                });
            }
        }
        let desired_uris = local
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        let changes = plan_youtube_changes(&fresh.snapshot.entries, &local, unresolved);
        conflict |= !write_changes_allowed(fresh.can_edit, fresh.write_capabilities, &changes);
        let preview = RemotePlaylistSyncPreview {
            write_capabilities: fresh.write_capabilities,
            preview_id: uuid::Uuid::new_v4().to_string(),
            binding_id: binding_id.into(),
            provider: "youtube".into(),
            playlist_id: binding.playlist_id,
            local_collection_id: binding.collection_id,
            baseline_revision: baseline.revision,
            remote_revision: fresh.snapshot.revision.clone(),
            conflict,
            applied: false,
            can_edit: fresh.can_edit,
            write_sync_enabled,
            summary: diff_summary(&changes, conflict),
            changes,
        };
        self.previews
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .insert(
                preview.preview_id.clone(),
                StoredPreview {
                    preview: preview.clone(),
                    remote_hash: fresh.snapshot.content_hash,
                    account_id: account.id,
                    desired_uris,
                    local_identity_hashes,
                    local_order_hash,
                },
            );
        Ok(preview)
    }

    async fn verify_mutation_account(&self, provider: &str, expected_id: &str) -> Result<()> {
        let value = self
            .bridge
            .provider_operation(provider, "status", json!({}))
            .await?;
        let account = self
            .optional_result_account(provider, &value)?
            .ok_or("Provider did not confirm an account identity")?;
        if account.id != expected_id {
            return Err("Provider account changed during playlist sync".into());
        }
        Ok(())
    }
    async fn require_account(&self, provider: &str) -> Result<CachedAccount> {
        match self.connection_status(provider).await? {
            status if status.connected => self
                .account_cache
                .lock()
                .map_err(|_| "Remote playlist state unavailable")?
                .get(provider)
                .cloned()
                .ok_or("Provider identity is not fresh".into()),
            status => Err(status
                .error
                .unwrap_or_else(|| "Provider is not connected".into())),
        }
    }
    fn optional_result_account(
        &self,
        provider: &str,
        value: &Value,
    ) -> Result<Option<CachedAccount>> {
        let Some(id) = value
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            return Ok(None);
        };
        let id = id.to_owned();
        let name = value
            .get("displayName")
            .and_then(Value::as_str)
            .map(|name| name.chars().take(160).collect());
        let account = CachedAccount {
            id,
            name,
            checked_at: chrono::Utc::now(),
        };
        self.account_cache
            .lock()
            .map_err(|_| "Remote playlist state unavailable")?
            .insert(provider.into(), account.clone());
        Ok(Some(account))
    }
    fn ensure_response_account(&self, provider: &str, value: &Value, expected: &str) -> Result<()> {
        let actual = value
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 256)
            .ok_or("Provider response did not identify the signed-in account")?;
        if actual != expected {
            return Err("Provider account changed during catalog browse".into());
        }
        self.optional_result_account(provider, value)?;
        Ok(())
    }
    fn find_binding(&self, binding_id: &str) -> Result<RemotePlaylistBindingStatus> {
        self.load_state()?
            .bindings
            .into_iter()
            .find(|binding| binding.binding_id == binding_id)
            .ok_or("Remote playlist binding not found".into())
    }
    fn load_state(&self) -> Result<RemoteState> {
        self.store
            .get_json_preference(STATE_KEY)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())
            .map(|state| state.unwrap_or_default())
    }
    fn save_state(&self, state: &RemoteState) -> Result<()> {
        self.store.set_json_preference(
            STATE_KEY,
            &serde_json::to_value(state).map_err(|e| e.to_string())?,
        )
    }
    fn archive_snapshot(&self, _provider: &str, fetched: &FetchedPlaylist) -> Result<String> {
        let result = self
            .store
            .import_browser_chart(&fetched.chart, &fetched.browser_snapshot)?;
        Ok(result.collection_id)
    }
    fn set_job_state(&self, id: &str, state: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.get_mut(id) {
                job.status.state = state.into();
            }
        }
    }
    fn notify_changed(&self) {
        let callback = self
            .change_notifier
            .lock()
            .ok()
            .and_then(|notifier| notifier.clone());
        if let Some(callback) = callback {
            callback();
        }
    }
}

struct FetchedPlaylist {
    write_capabilities: Option<RemotePlaylistWriteCapabilities>,
    title: String,
    url: String,
    owner_id: Option<String>,
    owner_name: Option<String>,
    owner_url: Option<String>,
    visibility: Option<String>,
    can_edit: bool,
    tracks: Vec<Track>,
    snapshot: RemoteSnapshot,
    account_id: Option<String>,
    browser_snapshot: BrowserSnapshot,
    chart: Chart,
}
fn ensure_writable(
    provider: &str,
    can_edit: bool,
    complete: bool,
    entries: &[RemoteEntry],
) -> Result<()> {
    if !can_edit {
        return Err("Remote playlist is read-only".into());
    }
    ensure_playlist_capture(provider, complete, entries)
}
fn ensure_playlist_capture(provider: &str, complete: bool, entries: &[RemoteEntry]) -> Result<()> {
    if !complete {
        return Err("Remote playlist capture is incomplete".into());
    }
    if entries.iter().any(|entry| !entry.available) {
        return Err("Unavailable remote entries prevent a safe write".into());
    }
    if provider == "youtube"
        && entries.iter().any(|entry| {
            entry.entry_id.starts_with("ytocc")
                || !valid_youtube_video_id(&entry.track_id)
                || !valid_provider_occurrence_id(&entry.entry_id)
        })
    {
        return Err("YouTube playlist items are missing verified occurrence IDs".into());
    }
    Ok(())
}
fn capture_is_complete(
    terminal_page_complete: bool,
    expected_total: Option<usize>,
    captured: usize,
) -> bool {
    terminal_page_complete && expected_total.is_none_or(|total| total == captured)
}
fn observe_page_completion(terminal_page_complete: &mut bool, page_complete: bool, has_next: bool) {
    if !has_next {
        *terminal_page_complete = page_complete;
    }
}
fn validate_provider(provider: &str) -> Result<()> {
    if matches!(provider, "spotify" | "youtube") {
        Ok(())
    } else {
        Err("Unsupported remote playlist provider".into())
    }
}
fn provider_supports_write_sync(provider: &str) -> bool {
    matches!(provider, "spotify" | "youtube")
}
fn valid_remote_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn spotify_album_occurrence_id(album_id: &str, position: usize) -> String {
    // Remote entry IDs are restricted to ASCII alphanumerics, '_' and '-'.
    format!("album_{album_id}_{position}")
}
fn valid_youtube_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
fn valid_provider_occurrence_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
fn has_confirmed_youtube_identity(track: &Track) -> bool {
    if track.provider != "youtube" || !valid_youtube_video_id(&track.provider_id) {
        return false;
    }
    let Ok(url) = url::Url::parse(&track.source_url) else {
        return false;
    };
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.youtube.com" | "youtube.com" | "music.youtube.com")
        )
        || url.path() != "/watch"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return false;
    }
    let mut ids = url
        .query_pairs()
        .filter(|(key, _)| key == "v")
        .map(|(_, value)| value.into_owned());
    ids.next().as_deref() == Some(track.provider_id.as_str()) && ids.next().is_none()
}
fn validate_numeric_cursor(cursor: Option<&str>, maximum: usize) -> Result<()> {
    if let Some(cursor) = cursor {
        if cursor.is_empty() || !cursor.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Provider page cursor is invalid".into());
        }
        let value = cursor
            .parse::<usize>()
            .map_err(|_| "Provider page cursor is invalid")?;
        if value > maximum {
            return Err("Provider page cursor is out of range".into());
        }
    }
    Ok(())
}
fn catalog_query_hash(query: &str) -> String {
    let normalized = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}
fn parse_spotify_artist(row: &Value) -> Result<RemoteSpotifyArtist> {
    let item = normalize_catalog_item(row, "spotify", "artist")?;
    let id = required_text(&item, "id", 22)?;
    if !valid_spotify_id(&id) {
        return Err("Spotify returned an invalid artist ID".into());
    }
    let data = item.get("data").unwrap_or(&Value::Null);
    Ok(RemoteSpotifyArtist {
        id: id.to_owned(),
        name: required_text(&item, "title", 512)?.to_owned(),
        url: required_text(&item, "url", 2048)?.to_owned(),
        artwork_url: item
            .get("artworkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        genres: data
            .get("genres")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .take(50)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        popularity: data
            .get("popularity")
            .and_then(Value::as_u64)
            .filter(|value| *value <= 100)
            .map(|value| value as u8),
        followers: data.get("followers").and_then(Value::as_u64),
    })
}
fn parse_spotify_album(row: &Value) -> Result<RemoteSpotifyAlbum> {
    let item = normalize_catalog_item(row, "spotify", "album")?;
    let id = required_text(&item, "id", 22)?;
    if !valid_spotify_id(&id) {
        return Err("Spotify returned an invalid album ID".into());
    }
    let data = item.get("data").unwrap_or(&Value::Null);
    Ok(RemoteSpotifyAlbum {
        id: id.to_owned(),
        title: required_text(&item, "title", 512)?.to_owned(),
        url: required_text(&item, "url", 2048)?.to_owned(),
        artwork_url: item
            .get("artworkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        release_date: data
            .get("releaseDate")
            .and_then(Value::as_str)
            .map(str::to_owned),
        album_type: data
            .get("albumType")
            .and_then(Value::as_str)
            .map(str::to_owned),
        total_tracks: data
            .get("totalTracks")
            .and_then(Value::as_u64)
            .filter(|value| *value <= 10_000)
            .map(|value| value as usize),
        artists: data
            .get("artists")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .take(50)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}
// Lookup deduplication is by provider ID; application eligibility must remain per row.
fn apply_verified_spotify_durations(tracks: &mut [Track], matches: &HashMap<String, f64>) {
    for track in tracks
        .iter_mut()
        .filter(|track| is_duration_backfill_eligible(track))
    {
        if let Some(duration) = matches.get(&track.provider_id) {
            track.duration_seconds = Some(*duration);
            if !track.provenance.is_object() {
                track.provenance = json!({});
            }
            track.provenance["durationBackfill"] =
                json!({"provider":"spotify","verifiedProviderId":track.provider_id});
        }
    }
}
fn is_duration_backfill_eligible(track: &Track) -> bool {
    track.provider == "spotify"
        && valid_spotify_id(&track.provider_id)
        && duration_is_missing(track.duration_seconds)
        && track.playback_range.is_none()
        && !track.provenance["chapters"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        && track.provenance["manualFields"]["durationSeconds"] != true
}
fn duration_is_missing(duration: Option<f64>) -> bool {
    !duration.is_some_and(|seconds| seconds.is_finite() && seconds > 0.0)
}
fn verified_spotify_duration(item: &Value, requested_id: &str) -> Option<f64> {
    if item.get("kind").and_then(Value::as_str) != Some("track")
        || item.get("id").and_then(Value::as_str) != Some(requested_id)
        || !valid_spotify_id(requested_id)
    {
        return None;
    }
    item.get("data")?
        .get("durationSeconds")?
        .as_f64()
        .filter(|duration| duration.is_finite() && *duration > 0.0 && *duration <= 86_400.0)
}
fn valid_provider_track_id(provider: &str, value: &str) -> bool {
    if provider == "spotify" {
        valid_spotify_id(value)
    } else {
        valid_remote_id(value)
    }
}
fn has_confirmed_spotify_identity(track: &Track) -> bool {
    if track.provider != "spotify" || !valid_spotify_id(&track.provider_id) {
        return false;
    }
    let Ok(url) = url::Url::parse(&track.source_url) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("open.spotify.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return false;
    }
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    segments.as_slice() == ["track", track.provider_id.as_str()]
}
fn track_identity_hash(track: &Track) -> Result<String> {
    let mut provenance = track.provenance.clone();
    if let Some(object) = provenance.as_object_mut() {
        object.remove("playlistPosition");
    }
    let identity = json!({
        "id": &track.id,
        "provider": &track.provider,
        "providerId": &track.provider_id,
        "sourceUrl": &track.source_url,
        "artist": &track.artist,
        "title": &track.title,
        "version": &track.version,
        "isrc": &track.isrc,
        "playbackRange": &track.playback_range,
        "provenance": provenance,
        "durationSeconds": track.duration_seconds,
    });
    let bytes = serde_json::to_vec(&identity).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn version_markers(value: &str) -> HashSet<&'static str> {
    let value = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let value = format!(" {} ", value.join(" "));
    let mut markers = HashSet::new();
    if ["radio edit", "radio version", "single edit"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("radio");
    }
    if ["remix", "rework", "refix", "bootleg"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("remix");
    }
    if ["live", "concert", "session"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("live");
    }
    if ["acoustic", "unplugged"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("acoustic");
    }
    if ["instrumental", "karaoke"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("instrumental");
    }
    if ["extended", "club mix", "12 inch"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("extended");
    }
    if ["remaster", "remastered"]
        .iter()
        .any(|term| value.contains(&format!(" {term} ")))
    {
        markers.insert("remaster");
    }
    markers
}
fn valid_spotify_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && !value.contains('/')
        && !value.contains('\\')
}
fn normalize_playlist_reference(provider: &str, value: &str) -> Result<String> {
    if provider == "spotify"
        && (value == "tracks"
            || value.starts_with("spotify:collection:")
            || value.starts_with("https://open.spotify.com/collection/")
            || value == "https://open.spotify.com/playlist/tracks")
    {
        return Err(
            "Spotify Liked Songs and other collections are not available as playlists".into(),
        );
    }
    if valid_remote_id(value) {
        return Ok(value.to_owned());
    }
    let url = url::Url::parse(value).map_err(|_| "Enter a playlist ID or official playlist URL")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return Err("Playlist URL must use its official HTTPS domain".into());
    }
    let id = match provider {
        "spotify" if url.host_str() == Some("open.spotify.com") => {
            let segments = url
                .path_segments()
                .map(|segments| segments.collect::<Vec<_>>())
                .unwrap_or_default();
            if segments.len() == 2 && segments[0] == "playlist" {
                segments[1].to_owned()
            } else {
                return Err("Use a Spotify playlist URL".into());
            }
        }
        "youtube"
            if matches!(
                url.host_str(),
                Some("music.youtube.com" | "www.youtube.com" | "youtube.com")
            ) =>
        {
            url.query_pairs()
                .find(|(key, _)| key == "list")
                .map(|(_, value)| value.into_owned())
                .ok_or("Use a YouTube playlist URL containing a playlist ID")?
        }
        _ => return Err("Playlist URL is outside the selected provider".into()),
    };
    if !valid_remote_id(&id) {
        return Err("Playlist URL contains an invalid ID".into());
    }
    if provider == "spotify" && !valid_spotify_id(&id) {
        return Err("Invalid Spotify playlist ID".into());
    }
    Ok(id)
}
fn supported_search_kind(provider: &str, kind: &str) -> bool {
    match provider {
        "spotify" => matches!(kind, "track" | "artist" | "album" | "playlist" | "profile"),
        "youtube" => matches!(kind, "track" | "playlist" | "channel"),
        _ => false,
    }
}
pub(crate) fn normalize_catalog_item(row: &Value, provider: &str, kind: &str) -> Result<Value> {
    let id_max = if provider == "spotify" && kind == "profile" {
        256
    } else {
        128
    };
    let id = required_text(row, "id", id_max)?;
    let valid_id = if provider == "spotify" && kind == "profile" {
        valid_spotify_profile_id(id)
    } else if provider == "spotify" {
        valid_spotify_id(id)
    } else if kind == "channel" && id.starts_with('@') {
        id.len() <= 100
            && id[1..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    } else {
        valid_remote_id(id)
    };
    if !valid_id {
        return Err("Provider returned an invalid catalog ID".into());
    }
    let url = validate_catalog_url(required_text(row, "url", 2048)?, provider, kind, id)?;
    let title = required_text(row, "title", 512)?;
    let subtitle = row
        .get("subtitle")
        .and_then(Value::as_str)
        .filter(|text| text.len() <= 512);
    let artwork_url = row
        .get("artworkUrl")
        .and_then(Value::as_str)
        .filter(|value| safe_https_asset(value));
    let data = normalize_catalog_data(row.get("data"), kind, provider)?;
    let mut result = json!({"id":id,"kind":kind,"title":title,"url":url,"data":data});
    result["subtitle"] = subtitle.map_or(Value::Null, |text| json!(text));
    result["artworkUrl"] = artwork_url.map_or(Value::Null, |url| json!(url));
    Ok(result)
}
fn validate_catalog_url(input: &str, provider: &str, kind: &str, id: &str) -> Result<String> {
    let canonical = validate_remote_url(input, provider)?;
    let parsed =
        url::Url::parse(&canonical).map_err(|_| "Provider returned an invalid catalog URL")?;
    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    let accepted = if provider == "spotify" {
        if kind == "profile" {
            segments.windows(2).any(|pair| {
                matches!(pair[0], "user" | "profile")
                    && decode_url_path_segment_once(pair[1]).as_deref() == Some(id)
            })
        } else {
            segments
                .windows(2)
                .any(|pair| pair[0] == kind && pair[1] == id)
        }
    } else {
        match kind {
            "playlist" => parsed
                .query_pairs()
                .any(|(key, value)| key == "list" && value == id),
            "track" => {
                (segments.contains(&"watch") || segments.is_empty())
                    && parsed
                        .query_pairs()
                        .any(|(key, value)| key == "v" && value == id)
            }
            "channel" => segments
                .iter()
                .any(|segment| *segment == id || format!("@{segment}") == id),
            _ => false,
        }
    };
    if !accepted {
        return Err("Catalog URL does not match its provider ID and type".into());
    }
    Ok(canonical)
}
fn decode_url_path_segment_once(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push((hex_nibble(high)? << 4) | hex_nibble(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
fn safe_https_asset(input: &str) -> bool {
    url::Url::parse(input).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none_or(|port| port == 443)
    })
}
fn normalize_catalog_data(data: Option<&Value>, kind: &str, provider: &str) -> Result<Value> {
    let Some(Value::Object(object)) = data else {
        return Ok(Value::Null);
    };
    let keys: &[&str] = match kind {
        "track" => &[
            "trackId",
            "title",
            "artist",
            "version",
            "artists",
            "album",
            "durationSeconds",
            "url",
            "artworkUrl",
            "isrc",
            "available",
            "explicit",
            "views",
            "plays",
            "viewCount",
            "playCount",
        ],
        "artist" | "channel" | "profile" => &[
            "id",
            "name",
            "title",
            "url",
            "artworkUrl",
            "description",
            "ownerId",
            "genres",
            "popularity",
            "followers",
        ],
        "album" => &[
            "id",
            "title",
            "artist",
            "artists",
            "trackCount",
            "totalTracks",
            "albumType",
            "releaseDate",
            "url",
            "artworkUrl",
        ],
        "playlist" => &[
            "id",
            "title",
            "ownerId",
            "ownerName",
            "total",
            "canEdit",
            "writeCapabilities",
            "url",
            "artworkUrl",
        ],
        _ => &[],
    };
    let mut result = serde_json::Map::new();
    for key in keys {
        if let Some(value) = object.get(*key) {
            if !bounded_plain_json(value, 0) {
                return Err("Provider returned invalid catalog metadata".into());
            }
            result.insert((*key).to_owned(), value.clone());
        }
    }
    if kind == "track" && provider == "spotify" {
        // Keep raw credit count separate from validated identities: filtering or
        // truncation must not turn incomplete credits into a false exact match.
        if let Some(count) = object.get("artistCount").and_then(Value::as_u64) {
            result.insert("artistCount".into(), json!(count));
        }
        if let Some(rows) = object.get("artistRefs").and_then(Value::as_array) {
            let credits = rows
                .iter()
                .take(100)
                .filter_map(|row| {
                    let id = row.get("id").and_then(Value::as_str)?;
                    let name = row.get("name").and_then(Value::as_str)?.trim();
                    (row.get("provider").and_then(Value::as_str) == Some("spotify")
                        && valid_spotify_id(id)
                        && !name.is_empty()
                        && name.len() <= 512)
                        .then(|| json!({"provider":"spotify","id":id,"name":name}))
                })
                .collect::<Vec<_>>();
            result.insert("artistRefs".into(), json!(credits));
        }
    }
    Ok(Value::Object(result))
}
fn bounded_plain_json(value: &Value, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    match value {
        Value::String(text) => text.len() <= 2048,
        Value::Array(values) => {
            values.len() <= 100
                && values
                    .iter()
                    .all(|value| bounded_plain_json(value, depth + 1))
        }
        Value::Object(values) => {
            values.len() <= 100
                && values
                    .iter()
                    .all(|(key, value)| key.len() <= 64 && bounded_plain_json(value, depth + 1))
        }
        _ => true,
    }
}
fn required_text<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty() && text.len() <= max)
        .ok_or_else(|| format!("Provider returned invalid {key}"))
}
fn bounded_optional_text(value: &Value, key: &str, max: usize) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= max)
        .map(str::to_owned)
}
fn normalize_visibility(value: Option<&str>) -> Option<String> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "public" => Some("public".into()),
        "private" => Some("private".into()),
        "unlisted" => Some("unlisted".into()),
        "collaborative" => Some("collaborative".into()),
        _ => None,
    }
}
fn normalize_owner_url(
    provider: &str,
    owner_id: Option<&str>,
    supplied: Option<&str>,
) -> Option<String> {
    if provider == "spotify" {
        if let Some(input) = supplied {
            let url = url::Url::parse(input).ok()?;
            if url.scheme() != "https"
                || url.host_str() != Some("open.spotify.com")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some_and(|port| port != 443)
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return None;
            }
            let segments = url.path_segments()?.collect::<Vec<_>>();
            if segments.len() != 2
                || segments[0] != "user"
                || !valid_remote_id(segments[1])
                || owner_id.is_some_and(|id| id != segments[1])
            {
                return None;
            }
            return Some(format!("https://open.spotify.com/user/{}", segments[1]));
        }
        let id = owner_id.filter(|id| valid_remote_id(id))?;
        return Some(format!("https://open.spotify.com/user/{id}"));
    }
    if provider != "youtube" {
        return None;
    }
    if let Some(input) = supplied {
        let url = url::Url::parse(input).ok()?;
        if url.scheme() != "https"
            || !matches!(url.host_str(), Some("youtube.com" | "www.youtube.com"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some_and(|port| port != 443)
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let segments = url
            .path_segments()?
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if segments.len() == 2 && segments[0] == "channel" && segments[1].starts_with("UC") {
            let id = segments[1];
            if !valid_remote_id(id) || owner_id.is_some_and(|owner| owner != id) {
                return None;
            }
            return Some(format!("https://www.youtube.com/channel/{id}"));
        }
        if segments.len() == 1 && valid_youtube_handle(segments[0]) {
            let handle = segments[0];
            if owner_id.is_some_and(|owner| {
                owner.trim_start_matches('@') != handle.trim_start_matches('@')
            }) {
                return None;
            }
            return Some(format!(
                "https://www.youtube.com/@{}",
                handle.trim_start_matches('@')
            ));
        }
        return None;
    }
    let id = owner_id?;
    if id.starts_with("UC") && valid_remote_id(id) {
        Some(format!("https://www.youtube.com/channel/{id}"))
    } else if valid_youtube_handle(id) {
        Some(format!(
            "https://www.youtube.com/@{}",
            id.trim_start_matches('@')
        ))
    } else {
        None
    }
}
fn valid_youtube_handle(value: &str) -> bool {
    let handle = value.trim_start_matches('@');
    (1..=100).contains(&handle.len())
        && handle
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}
fn parse_playlist_item(row: &Value, provider: &str) -> Result<RemotePlaylistItem> {
    if row
        .get("provider")
        .and_then(Value::as_str)
        .is_some_and(|value| value != provider)
    {
        return Err("Provider returned a playlist from another provider".into());
    }
    let id = required_text(row, "id", 128)?.to_owned();
    if !valid_remote_id(&id) {
        return Err("Provider returned an invalid playlist ID".into());
    }
    let url = validate_remote_url(required_text(row, "url", 2048)?, provider)?;
    let owner_id = bounded_optional_text(row, "ownerId", 256);
    let owner_name = bounded_optional_text(row, "ownerName", 512);
    let owner_url = normalize_owner_url(
        provider,
        owner_id.as_deref(),
        row.get("ownerUrl").and_then(Value::as_str),
    );
    Ok(RemotePlaylistItem {
        id,
        provider: provider.into(),
        title: required_text(row, "title", 256)?.to_owned(),
        url,
        owner_id,
        owner_name,
        owner_url,
        artwork_url: row
            .get("artworkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        track_count: provider_display_count(row.get("total")),
        visibility: normalize_visibility(row.get("visibility").and_then(Value::as_str)),
        can_edit: row.get("canEdit").and_then(Value::as_bool).unwrap_or(false),
        imported: false,
        binding_id: None,
        collection_id: None,
    })
}
fn validate_remote_url(input: &str, provider: &str) -> Result<String> {
    let url = url::Url::parse(input).map_err(|_| "Provider returned an invalid playlist URL")?;
    let host = url.host_str().unwrap_or("");
    let approved_host = if provider == "spotify" {
        host == "open.spotify.com"
    } else {
        matches!(
            host,
            "music.youtube.com" | "www.youtube.com" | "youtube.com"
        )
    };
    if url.scheme() != "https"
        || !approved_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return Err("Provider returned a URL outside its official HTTPS domain".into());
    }
    Ok(url.to_string())
}
fn parse_remote_entry(value: &Value, provider: &str, occurrence_key: &str) -> Result<RemoteEntry> {
    let available = value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let entry_id = match value.get("entryId") {
        Some(Value::String(entry_id)) if !entry_id.is_empty() => {
            if !valid_remote_id(entry_id) {
                return Err("Provider returned an invalid playlist entry ID".into());
            }
            entry_id.clone()
        }
        Some(Value::Null) | None | Some(Value::String(_)) if provider == "youtube" => {
            let track_id = value
                .get("trackId")
                .and_then(Value::as_str)
                .unwrap_or("unavailable");
            synthetic_youtube_entry_id(occurrence_key, track_id)
        }
        _ => return Err("Provider returned an invalid playlist entry ID".into()),
    };
    let track_id = value
        .get("trackId")
        .and_then(Value::as_str)
        .filter(|id| valid_provider_track_id(provider, id))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("unknown{:x}", Sha256::digest(entry_id.as_bytes())));
    if available
        && !value
            .get("trackId")
            .and_then(Value::as_str)
            .is_some_and(|id| valid_provider_track_id(provider, id))
    {
        return Err("Provider returned an invalid track ID".into());
    }
    let url = match value.get("url").and_then(Value::as_str) {
        Some(url) if !url.is_empty() => validate_remote_url(url, provider)?,
        _ if !available => String::new(),
        _ => return Err("Provider returned a track without a URL".into()),
    };
    let duration = value
        .get("durationSeconds")
        .and_then(Value::as_f64)
        .filter(|duration| duration.is_finite() && *duration > 0.0 && *duration <= 86_400.0);
    let title = match value
        .get("title")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty() && text.len() <= 512)
    {
        Some(title) => title,
        None if available => return Err("Provider returned an invalid title".into()),
        None => "Unavailable track",
    };
    Ok(RemoteEntry {
        entry_id,
        track_id,
        title: title.to_owned(),
        artist: value
            .get("artist")
            .and_then(Value::as_str)
            .unwrap_or("Unknown Artist")
            .chars()
            .take(512)
            .collect(),
        album: value
            .get("album")
            .and_then(Value::as_str)
            .map(|v| v.chars().take(512).collect()),
        duration_seconds: duration,
        url,
        artwork_url: value
            .get("artworkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        available,
        plays: reported_remote_count(value, &["plays", "playCount", "play_count"]),
        views: reported_remote_count(value, &["views", "viewCount", "view_count"]),
    })
}
fn reported_remote_count(value: &Value, aliases: &[&str]) -> Option<u64> {
    aliases.iter().find_map(|alias| {
        let count = value.get(*alias)?;
        let parsed = count
            .as_u64()
            .or_else(|| {
                count.as_f64().and_then(|number| {
                    (number.is_finite()
                        && number >= 0.0
                        && number.fract() == 0.0
                        && number <= MAX_REPORTED_REMOTE_COUNT as f64)
                        .then_some(number as u64)
                })
            })
            .or_else(|| {
                let text = count.as_str()?.trim();
                let groups = text.split(',').collect::<Vec<_>>();
                let grouped_digits = groups.len() > 1
                    && (1..=3).contains(&groups[0].len())
                    && groups[0].bytes().all(|byte| byte.is_ascii_digit())
                    && groups[1..].iter().all(|group| {
                        group.len() == 3 && group.bytes().all(|byte| byte.is_ascii_digit())
                    });
                let plain_digits = groups.len() == 1
                    && !text.is_empty()
                    && text.bytes().all(|byte| byte.is_ascii_digit());
                if !grouped_digits && !plain_digits {
                    return None;
                }
                text.replace(',', "").parse::<u64>().ok()
            })?;
        (parsed <= MAX_REPORTED_REMOTE_COUNT).then_some(parsed)
    })
}
fn synthetic_youtube_entry_id(occurrence_key: &str, track_id: &str) -> String {
    let bounded_track_id = track_id.chars().take(256).collect::<String>();
    format!(
        "ytocc{:x}",
        Sha256::digest(format!("{occurrence_key}:{bounded_track_id}").as_bytes())
    )
}
fn entry_to_track(provider: &str, entry: &RemoteEntry, position: usize) -> Track {
    let id = format!("{provider}:{}", entry.track_id);
    let mut provenance = json!({"remotePlaylist":true,"remoteEntryId":entry.entry_id,"position":position+1,"available":entry.available});
    if let Some(plays) = entry.plays {
        provenance["plays"] = json!(plays);
    }
    if let Some(views) = entry.views {
        provenance["views"] = json!(views);
    }
    Track {
        id: id.clone(),
        provider: provider.into(),
        provider_id: entry.track_id.clone(),
        source_url: entry.url.clone(),
        artist: entry.artist.clone(),
        title: entry.title.clone(),
        album: entry.album.clone(),
        duration_seconds: entry.duration_seconds,
        artwork_url: entry.artwork_url.clone(),
        provenance,
        ..Track::default()
    }
}
fn parse_browse_tracks(entries: &[Value], provider: &str, page_key: &str) -> Result<Vec<Track>> {
    // General YouTube browse responses may contain more than Spotify's 100
    // rows. Keep the entire bounded provider page and its original cursor;
    // truncating here would silently lose playlist occurrences.
    let max_page_entries = if provider == "youtube" { 1_000 } else { 100 };
    if entries.len() > max_page_entries {
        return Err(
            format!("Provider returned more than {max_page_entries} tracks for one page").into(),
        );
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let occurrence_key = format!("{page_key}:{index}");
            let remote = parse_remote_entry(entry, provider, &occurrence_key)?;
            let mut track = entry_to_track(provider, &remote, index);
            if let Some(position) = entry
                .get("position")
                .and_then(Value::as_u64)
                .filter(|position| *position > 0)
            {
                track.provenance["position"] = json!(position);
                track.provenance["positionScope"] = json!("playlist");
            } else {
                track.provenance["positionScope"] = json!("page");
            }
            Ok(track)
        })
        .collect()
}
/// A provider may omit duration on a later page capture even when the app already
/// knows it. Keep that catalog metadata when refreshing an otherwise untouched copy.
fn preserve_known_durations(incoming: &mut [Track], existing: &[Track]) {
    let known: HashMap<(&str, &str), f64> = existing
        .iter()
        .filter_map(|track| {
            track
                .duration_seconds
                .filter(|duration| duration.is_finite() && *duration > 0.0)
                .map(|duration| {
                    (
                        (track.provider.as_str(), track.provider_id.as_str()),
                        duration,
                    )
                })
        })
        .collect();
    for track in incoming {
        if duration_is_missing(track.duration_seconds) {
            if let Some(duration) =
                known.get(&(track.provider.as_str(), track.provider_id.as_str()))
            {
                track.duration_seconds = Some(*duration);
            }
        }
    }
}
fn hash_json(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn remote_entries_content_hash(entries: &[RemoteEntry]) -> Result<String> {
    let mut stable_entries = entries.to_vec();
    for entry in &mut stable_entries {
        entry.plays = None;
        entry.views = None;
    }
    hash_json(&stable_entries)
}
fn remote_tracks_revision_hash(tracks: &[Track]) -> Result<String> {
    let mut stable_tracks = tracks.to_vec();
    for track in &mut stable_tracks {
        if let Some(provenance) = track.provenance.as_object_mut() {
            provenance.remove("plays");
            provenance.remove("views");
        }
    }
    hash_json(&stable_tracks)
}
fn preserve_remote_counts(tracks: &mut [Track], entries: &[RemoteEntry]) {
    for (track, entry) in tracks.iter_mut().zip(entries) {
        if let Some(plays) = entry.plays {
            track.provenance["plays"] = json!(plays);
        }
        if let Some(views) = entry.views {
            track.provenance["views"] = json!(views);
        }
    }
}
fn import_item(
    binding: &RemotePlaylistBindingStatus,
    fetched: &FetchedPlaylist,
) -> RemotePlaylistImportItem {
    RemotePlaylistImportItem {
        binding_id: binding.binding_id.clone(),
        collection_id: binding.collection_id.clone(),
        provider: binding.provider.clone(),
        playlist_id: binding.playlist_id.clone(),
        title: binding.title.clone(),
        track_count: fetched.tracks.len(),
        revision: fetched.snapshot.revision.clone(),
        complete: fetched.snapshot.complete,
    }
}
fn find_bound_playlist<'a>(
    bindings: &'a [RemotePlaylistBindingStatus],
    provider: &str,
    playlist_id: &str,
    account_id: &str,
) -> Option<&'a RemotePlaylistBindingStatus> {
    bindings.iter().find(|binding| {
        binding.provider == provider
            && binding.playlist_id == playlist_id
            && binding.account_id == account_id
    })
}
fn collection_matches(tracks: &[Track], snapshot: &RemoteSnapshot, provider: &str) -> bool {
    snapshot.complete
        && tracks.len() == snapshot.entries.len()
        && tracks.iter().zip(&snapshot.entries).all(|(track, entry)| {
            let identity_matches = if entry.available {
                track.provider == provider && track.provider_id == entry.track_id
            } else {
                track.provider == "import" && track.provider_id == unavailable_fallback_id(entry)
            };
            identity_matches
                && track.title.trim() == entry.title.trim()
                && track.artist.trim() == entry.artist.trim()
                && track.album == entry.album
                && track.duration_seconds == entry.duration_seconds
                && track.artwork_url == entry.artwork_url
        })
}
fn should_replace_working_copy(
    tracks: &[Track],
    baseline: &RemoteSnapshot,
    provider: &str,
    force_baseline: bool,
) -> bool {
    force_baseline || collection_matches(tracks, baseline, provider)
}
fn unavailable_fallback_id(entry: &RemoteEntry) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}\n{}\n",
                entry.artist.trim().to_lowercase(),
                entry.title.trim().to_lowercase()
            )
            .as_bytes()
        )
    )
}
fn plan_changes(
    remote: &[RemoteEntry],
    local: &[(String, Track)],
    mut unresolved: Vec<RemoteUnresolvedChange>,
) -> RemotePlaylistSyncChanges {
    let mut remaining_local = local.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
    let mut keep = Vec::<RemoteEntry>::new();
    let mut remove = Vec::new();
    for entry in remote {
        if !entry.available {
            unresolved.push(RemoteUnresolvedChange { entry_id: Some(entry.entry_id.clone()), local_track_id: None, title: None, artist: None, duration_seconds: None, reason: "Remote playlist contains an unavailable entry whose track identity cannot be safely changed".into() });
            keep.push(entry.clone());
            continue;
        }
        let uri = format!("spotify:track:{}", entry.track_id);
        if let Some(index) = remaining_local.iter().position(|local| local == &uri) {
            remaining_local.remove(index);
            keep.push(entry.clone());
        } else {
            remove.push(entry.entry_id.clone());
        }
    }
    let add = remaining_local
        .into_iter()
        .map(|track_uri| RemoteAddChange { track_uri })
        .collect::<Vec<_>>();
    let mut unmatched = keep.clone();
    let mut desired_common = Vec::new();
    for (uri, _) in local {
        if let Some(index) = unmatched
            .iter()
            .position(|entry| format!("spotify:track:{}", entry.track_id) == *uri)
        {
            desired_common.push(unmatched.remove(index).entry_id);
        }
    }
    let current_common = keep
        .iter()
        .map(|entry| entry.entry_id.clone())
        .collect::<Vec<_>>();
    let mut moves = Vec::new();
    if current_common != desired_common {
        for (index, entry_id) in desired_common.iter().enumerate().rev() {
            let before = desired_common.get(index + 1).cloned();
            let current_pos = keep.iter().position(|entry| &entry.entry_id == entry_id);
            if current_pos != Some(index) {
                moves.push(RemoteMoveChange {
                    entry_ids: vec![entry_id.clone()],
                    before_entry_id: before,
                });
            }
        }
    }
    RemotePlaylistSyncChanges {
        add,
        remove: if remove.is_empty() {
            vec![]
        } else {
            vec![RemoteRemoveChange { entry_ids: remove }]
        },
        move_entries: moves,
        unresolved,
    }
}
fn plan_youtube_changes(
    remote: &[RemoteEntry],
    local: &[(String, Track)],
    mut unresolved: Vec<RemoteUnresolvedChange>,
) -> RemotePlaylistSyncChanges {
    let mut remaining = local
        .iter()
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    let mut keep = Vec::<RemoteEntry>::new();
    let mut remove = Vec::new();
    for entry in remote {
        if !entry.available || !valid_youtube_video_id(&entry.track_id) {
            unresolved.push(RemoteUnresolvedChange {
                entry_id: Some(entry.entry_id.clone()),
                local_track_id: None,
                title: None,
                artist: None,
                duration_seconds: None,
                reason: "Remote YouTube playlist contains an unavailable or unverified video"
                    .into(),
            });
            keep.push(entry.clone());
            continue;
        }
        let identity = format!("youtube:video:{}", entry.track_id);
        if let Some(index) = remaining.iter().position(|local| local == &identity) {
            remaining.remove(index);
            keep.push(entry.clone());
        } else {
            remove.push(entry.entry_id.clone());
        }
    }
    let add = remaining
        .into_iter()
        .map(|track_uri| RemoteAddChange { track_uri })
        .collect();
    let mut unmatched = keep.clone();
    let mut desired_common = Vec::new();
    for (identity, _) in local {
        if let Some(index) = unmatched
            .iter()
            .position(|entry| format!("youtube:video:{}", entry.track_id) == *identity)
        {
            desired_common.push(unmatched.remove(index).entry_id);
        }
    }
    let current_common = keep
        .iter()
        .map(|entry| entry.entry_id.clone())
        .collect::<Vec<_>>();
    let mut moves = Vec::new();
    if current_common != desired_common {
        for (index, entry_id) in desired_common.iter().enumerate().rev() {
            let before = desired_common.get(index + 1).cloned();
            let current_pos = keep.iter().position(|entry| &entry.entry_id == entry_id);
            if current_pos != Some(index) {
                moves.push(RemoteMoveChange {
                    entry_ids: vec![entry_id.clone()],
                    before_entry_id: before,
                });
            }
        }
    }
    RemotePlaylistSyncChanges {
        add,
        remove: if remove.is_empty() {
            vec![]
        } else {
            vec![RemoteRemoveChange { entry_ids: remove }]
        },
        move_entries: moves,
        unresolved,
    }
}
fn diff_summary(changes: &RemotePlaylistSyncChanges, conflict: bool) -> String {
    if conflict {
        return "The remote playlist changed since its last refresh, or the capture is incomplete. Refresh the binding before reviewing a write.".into();
    }
    format!(
        "{} additions, {} removals, {} moves, {} unresolved tracks",
        changes.add.len(),
        changes
            .remove
            .iter()
            .map(|r| r.entry_ids.len())
            .sum::<usize>(),
        changes.move_entries.len(),
        changes.unresolved.len()
    )
}
fn match_entry_ids(entries: &[RemoteEntry], desired_uris: &[String]) -> Result<Vec<String>> {
    let mut ids_by_track = HashMap::<String, VecDeque<String>>::new();
    for entry in entries {
        if !entry.available || !valid_spotify_id(&entry.track_id) {
            return Err("Playlist contains an unavailable or unverified track".into());
        }
        ids_by_track
            .entry(entry.track_id.clone())
            .or_default()
            .push_back(entry.entry_id.clone());
    }
    let mut result = Vec::with_capacity(desired_uris.len());
    for uri in desired_uris {
        let id = uri
            .strip_prefix("spotify:track:")
            .filter(|id| valid_spotify_id(id))
            .ok_or("Preview contains an invalid Spotify track URI")?;
        let entry_id = ids_by_track
            .get_mut(id)
            .and_then(VecDeque::pop_front)
            .ok_or("Spotify did not return every requested track occurrence")?;
        result.push(entry_id);
    }
    if result.len() != entries.len() || ids_by_track.values().any(|ids| !ids.is_empty()) {
        return Err(
            "Spotify playlist occurrence count differs from the requested local order".into(),
        );
    }
    Ok(result)
}

fn youtube_worker_snapshot(entries: &[RemoteEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| {
            json!({
                "entryId": entry.entry_id,
                "trackId": if entry.available { Some(entry.track_id.as_str()) } else { None },
            })
        })
        .collect()
}

fn match_youtube_entry_ids(
    entries: &[RemoteEntry],
    desired_uris: &[String],
) -> Result<Vec<String>> {
    let mut ids_by_track = HashMap::<String, VecDeque<String>>::new();
    let mut seen_occurrences = HashSet::new();
    for entry in entries {
        if !entry.available
            || !valid_youtube_video_id(&entry.track_id)
            || !valid_provider_occurrence_id(&entry.entry_id)
            || !seen_occurrences.insert(entry.entry_id.as_str())
        {
            return Err("YouTube playlist contains an unavailable or unverified occurrence".into());
        }
        ids_by_track
            .entry(entry.track_id.clone())
            .or_default()
            .push_back(entry.entry_id.clone());
    }
    let mut result = Vec::with_capacity(desired_uris.len());
    for uri in desired_uris {
        let video_id = uri
            .strip_prefix("youtube:video:")
            .filter(|id| valid_youtube_video_id(id))
            .ok_or("Preview contains an invalid YouTube video URI")?;
        let entry_id = ids_by_track
            .get_mut(video_id)
            .and_then(VecDeque::pop_front)
            .ok_or("YouTube did not return every requested video occurrence")?;
        result.push(entry_id);
    }
    if result.len() != entries.len() || ids_by_track.values().any(|ids| !ids.is_empty()) {
        return Err(
            "YouTube playlist occurrence count differs from the requested local order".into(),
        );
    }
    Ok(result)
}

fn reorder_plan(current: &[String], desired: &[String]) -> Result<Vec<RemoteMoveChange>> {
    if current.len() != desired.len()
        || current.iter().collect::<HashSet<_>>().len() != current.len()
        || desired.iter().collect::<HashSet<_>>().len() != desired.len()
        || current.iter().collect::<HashSet<_>>() != desired.iter().collect::<HashSet<_>>()
    {
        return Err("Remote entry IDs do not match the requested playlist order".into());
    }
    let mut order = current.to_vec();
    let mut moves = Vec::new();
    for index in (0..desired.len()).rev() {
        let entry_id = &desired[index];
        let position = order
            .iter()
            .position(|current_id| current_id == entry_id)
            .ok_or("Remote entry disappeared during reorder planning")?;
        if index + 1 == desired.len() {
            if position + 1 != order.len() {
                order.remove(position);
                order.push(entry_id.clone());
                moves.push(RemoteMoveChange {
                    entry_ids: vec![entry_id.clone()],
                    before_entry_id: None,
                });
            }
        } else {
            let anchor = &desired[index + 1];
            let anchor_position = order
                .iter()
                .position(|current_id| current_id == anchor)
                .ok_or("Remote entry disappeared during reorder planning")?;
            if position + 1 != anchor_position {
                order.remove(position);
                let insert_at = order
                    .iter()
                    .position(|current_id| current_id == anchor)
                    .ok_or("Remote entry disappeared during reorder planning")?;
                order.insert(insert_at, entry_id.clone());
                moves.push(RemoteMoveChange {
                    entry_ids: vec![entry_id.clone()],
                    before_entry_id: Some(anchor.clone()),
                });
            }
        }
    }
    if order != desired {
        return Err("Could not derive an exact remote playlist reorder".into());
    }
    Ok(moves)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn youtube_inspection_shape_fixture() -> Value {
        let renderers = [
            "playlistVideoListRenderer",
            "playlistVideoListContinuation",
            "musicPlaylistShelfRenderer",
            "musicPlaylistShelfContinuation",
            "sectionListRenderer",
            "sectionListContinuation",
            "itemSectionRenderer",
            "itemSectionContinuation",
            "richGridRenderer",
            "richGridContinuation",
            "playlistVideoRenderer",
            "musicResponsiveListItemRenderer",
            "lockupViewModel",
            "continuationItemRenderer",
            "continuationItemViewModel",
            "appendContinuationItemsAction",
            "reloadContinuationItemsCommand",
            "commandExecutorCommand",
            "openPopupAction",
            "playlistHeaderRenderer",
            "musicEditablePlaylistDetailHeaderRenderer",
            "musicPlaylistEditHeaderRenderer",
            "musicDetailHeaderRenderer",
            "musicResponsiveHeaderRenderer",
            "playlistSidebarRenderer",
            "playlistSidebarPrimaryInfoRenderer",
            "playlistSidebarSecondaryInfoRenderer",
            "videoOwner",
            "videoOwnerRenderer",
            "playlistMetadataRenderer",
            "pageHeaderRenderer",
            "pageHeaderViewModel",
            "playlistHeaderViewModel",
            "ownerEndpoint",
            "ownerText",
            "navigationEndpoint",
            "browseEndpoint",
            "watchEndpoint",
            "canonicalBaseUrl",
            "canonicalUrl",
            "urlCanonical",
            "microformatDataRenderer",
            "privacy",
            "privacyStatus",
            "privacyForm",
            "dropdownFormFieldRenderer",
            "dropdownRenderer",
            "privacyDropdownItemRenderer",
            "icon",
            "editEndpoint",
            "playlistEditEndpoint",
            "playlistEditorEndpoint",
            "changePlaylistPrivacyEndpoint",
        ];
        let counts: serde_json::Map<String, Value> = [
            "contents",
            "continuationContents",
            "onResponseReceivedActions",
            "onResponseReceivedEndpoints",
            "onResponseReceivedCommands",
            "sidebar",
            "header",
            "other",
        ]
        .into_iter()
        .map(|bucket| {
            let values: serde_json::Map<String, Value> = renderers
                .into_iter()
                .map(|key| (key.to_owned(), json!(0)))
                .collect();
            (bucket.to_owned(), Value::Object(values))
        })
        .collect();
        json!({"version":1,"continuation":false,"counts":counts,"appendTargets":{"absent":0,"playlistList":1,"requestedPlaylist":0,"other":0,"malformed":0},"recognizedSections":1,"recognizedCursor":true,"ambiguous":false,"truncated":false,"membership":{"allResponse":{"ordinaryRows":1,"recommendedRows":2,"unavailableRows":0,"rowsWithOccurrenceId":1},"recognizedSections":{"ordinaryRows":1,"recommendedRows":0,"unavailableRows":0,"rowsWithOccurrenceId":1}},"cursorLocations":{"namedListDirect":1,"namedListItemSection":0,"sectionListDirect":1,"sectionListOtherItemSection":0,"other":0},"retainedRuleCandidatePresent":true,"retainedRuleEqualsSelected":false})
    }

    #[test]
    fn youtube_modern_inspection_evidence_is_bounded_and_redacted() {
        let mut shape = youtube_inspection_shape_fixture();
        let mut identity = serde_json::Map::new();
        for key in [
            "targetHeaders",
            "primaryPlaylistIds",
            "primaryNavigationPlaylistIds",
            "canonicalPlaylistIds",
            "editEndpointPlaylistIds",
            "privacyFieldPresent",
            "privateHeaders",
            "publicHeaders",
            "unlistedHeaders",
            "unknownPrivacyHeaders",
            "ownerEndpoints",
            "ownerMatchesConfirmedAccount",
            "ownerMismatches",
            "sidebarOwnerEndpoints",
            "sidebarOwnerMatchesConfirmedAccount",
            "sidebarOwnerMismatches",
            "sidebarPrivacyFields",
            "sidebarPrivateValues",
            "sidebarPublicValues",
            "sidebarUnlistedValues",
            "sidebarUnknownPrivacyValues",
            "privacyDropdownEntries",
            "privacyDropdownSelectedPrivate",
            "privacyDropdownSelectedPublic",
            "privacyDropdownSelectedUnlisted",
            "privacyDropdownSelectedUnknown",
            "sidebarVideoOwnerNodes",
            "sidebarVideoOwnerBrowseEndpoints",
            "sidebarVideoOwnerMatchesConfirmedAccount",
            "sidebarVideoOwnerMismatches",
            "modernPrimaryCount",
            "modernSecondaryCount",
            "modernMatchingIds",
            "modernConflictingIds",
            "modernMalformedEditIds",
            "modernMalformedTitleRuns",
            "modernMatchingCanonicals",
            "modernRejectedCanonicals",
            "modernTraversalTruncated",
            "modernOwnerRuns",
            "modernMatchingOwners",
            "modernConflictingOwners",
            "modernMalformedOwnerRuns",
            "modernSelectedPrivacyCount",
            "modernTargetBound",
            "modernIdentityRecognized",
            "modernSelectedTabs",
            "modernMemberSections",
            "modernMessageOnlySection",
            "modernEmptyRecognized",
            "canonicalRejectReason",
            "canonicalProtocolClass",
            "canonicalHostClass",
            "canonicalHasCredentials",
            "canonicalHasPort",
            "canonicalPathClass",
            "canonicalListCount",
            "canonicalListMatches",
            "editMatchedCommands",
            "editForeignCommands",
            "editMalformedCommands",
            "editApiPathMatched",
            "editParamsPresent",
            "editActionsMissing",
            "editActionsEmpty",
            "editActionsMalformed",
            "editActionAdd",
            "editActionRemove",
            "editActionMove",
            "editActionPrivacy",
            "editActionMetadata",
            "editActionOther",
            "editActionCollection",
            "editActionCollaboration",
            "editActionVoting",
            "editFlagTrue",
            "editFlagFalse",
            "editFlagMalformed",
            "editFlagUnbound",
            "editReorderTrue",
            "editTraversalTruncated",
            "emptyRejectReason",
            "emptyContinuationHeader",
            "emptyContinuationSidebar",
            "emptyContinuationContents",
            "emptyContinuationOther",
            "emptyMembershipNodes",
            "emptyTraversalTruncated",
        ] {
            identity.insert(key.to_owned(), json!(0));
        }
        shape["identityEvidence"] = Value::Object(identity);
        shape["identityEvidence"]["privacyDropdownSelectedPrivate"] = json!(1);
        let page =
            json!({"responseShape":shape,"rawBody":"private-marker","accountId":"private-account"});
        let result =
            serde_json::to_value(parse_youtube_playlist_response_shape(&page, false).unwrap())
                .unwrap();
        assert_eq!(
            result["identityEvidence"]["privacyDropdownSelectedPrivate"],
            1
        );
        assert!(!result.to_string().contains("private-marker"));
        for invalid in [
            json!(10001),
            json!(-1),
            json!(1.5),
            json!("private-token"),
            Value::Null,
        ] {
            let mut bad = shape.clone();
            bad["identityEvidence"]["privacyDropdownSelectedPrivate"] = invalid;
            assert!(
                parse_youtube_playlist_response_shape(&json!({"responseShape":bad}), false)
                    .is_err()
            );
        }
        let mut excessive_renderer = shape.clone();
        excessive_renderer["counts"]["header"]["playlistEditEndpoint"] = json!(10001);
        assert!(parse_youtube_playlist_response_shape(
            &json!({"responseShape":excessive_renderer}),
            false
        )
        .is_err());
        for (key, invalid_enum) in [
            ("canonicalRejectReason", 10),
            ("emptyRejectReason", 11),
            ("canonicalProtocolClass", 4),
            ("canonicalHostClass", 5),
            ("canonicalHasCredentials", 2),
            ("canonicalHasPort", 2),
            ("canonicalPathClass", 4),
            ("canonicalListMatches", 2),
        ] {
            let mut bad = shape.clone();
            bad["identityEvidence"][key] = json!(invalid_enum);
            assert!(
                parse_youtube_playlist_response_shape(&json!({"responseShape":bad}), false)
                    .is_err()
            );
        }
        let mut bad = shape;
        bad["identityEvidence"]["rawOwner"] = json!("private-marker");
        assert!(
            parse_youtube_playlist_response_shape(&json!({"responseShape":bad}), false).is_err()
        );
    }

    #[test]
    fn youtube_inspection_playability_is_optional_bounded_and_redacted() {
        let base = youtube_inspection_shape_fixture();
        let old = serde_json::to_value(
            parse_youtube_playlist_response_shape(&json!({"responseShape":base}), false).unwrap(),
        )
        .unwrap();
        assert!(old.get("playability").is_none());
        let states = json!({"explicitTrue":240,"explicitFalse":0,"missing":0,"invalid":0,"withWatchEndpoint":240,"knownPlaceholder":19});
        let mut shape = youtube_inspection_shape_fixture();
        shape["playability"] = json!({"allResponse":states,"recognizedSections":states});
        let page = json!({"responseShape":shape,"rawBody":"private-title-token","accountId":"private-account"});
        let result =
            serde_json::to_value(parse_youtube_playlist_response_shape(&page, false).unwrap())
                .unwrap();
        assert_eq!(
            result["playability"]["recognizedSections"]["explicitTrue"],
            240
        );
        assert_eq!(
            result["playability"]["recognizedSections"]["knownPlaceholder"],
            19
        );
        assert!(!result.to_string().contains("private"));
        for key in [
            "explicitTrue",
            "explicitFalse",
            "missing",
            "invalid",
            "withWatchEndpoint",
            "knownPlaceholder",
        ] {
            let mut boundary = shape.clone();
            boundary["playability"]["allResponse"][key] = json!(10000);
            assert!(parse_youtube_playlist_response_shape(
                &json!({"responseShape":boundary}),
                false
            )
            .is_ok());
            for invalid in [
                json!(10001),
                json!(-1),
                json!(1.5),
                json!("private-token"),
                Value::Null,
            ] {
                let mut bad = shape.clone();
                bad["playability"]["allResponse"][key] = invalid;
                assert_eq!(
                    parse_youtube_playlist_response_shape(&json!({"responseShape":bad}), false)
                        .unwrap_err(),
                    "YouTube playlist response structure is unavailable or invalid"
                );
            }
        }
        let mut bad = shape.clone();
        bad["playability"]["rawRows"] = json!("private-token");
        assert!(
            parse_youtube_playlist_response_shape(&json!({"responseShape":bad}), false).is_err()
        );
        let mut bad = shape.clone();
        bad["playability"]["recognizedSections"]["title"] = json!("private-title");
        assert!(
            parse_youtube_playlist_response_shape(&json!({"responseShape":bad}), false).is_err()
        );
        let mut optional = shape;
        optional["playability"] = Value::Null;
        let result = serde_json::to_value(
            parse_youtube_playlist_response_shape(&json!({"responseShape":optional}), false)
                .unwrap(),
        )
        .unwrap();
        assert!(result.get("playability").is_none());
    }

    #[test]
    fn youtube_inspection_returns_only_fixed_structure() {
        let shape = youtube_inspection_shape_fixture();
        let page = json!({"responseShape":shape,"accountId":"private-account-marker","playlist":{"id":"private-id-marker","title":"private-title-marker"},"cursor":"private-cursor-marker","raw":{"secret-key":"secret-value"}});
        let result =
            serde_json::to_value(parse_youtube_playlist_response_shape(&page, false).unwrap())
                .unwrap();
        assert_eq!(result, shape);
        let text = result.to_string();
        for forbidden in ["private-", "secret-", "accountId", "\"cursor\"", "title"] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn youtube_inspection_rejects_unknown_missing_and_unbounded_shapes_without_echo() {
        let base = youtube_inspection_shape_fixture();
        let mut cases = Vec::new();
        for value in [
            json!(10001),
            json!(-1),
            json!(1.5),
            json!("secret-value"),
            json!(null),
        ] {
            let mut shape = base.clone();
            shape["counts"]["contents"]["playlistVideoRenderer"] = value;
            cases.push(shape);
        }
        let mut shape = base.clone();
        shape["secret-key"] = json!("secret-value");
        cases.push(shape);
        let mut shape = base.clone();
        shape["counts"]["contents"]["secret-key"] = json!(0);
        cases.push(shape);
        let mut shape = base.clone();
        shape["appendTargets"]["other"] = json!(10001);
        cases.push(shape);
        let mut shape = base.clone();
        shape["recognizedSections"] = json!(10001);
        cases.push(shape);
        let mut shape = base.clone();
        shape["version"] = json!(2);
        cases.push(shape);
        let mut shape = base.clone();
        shape["counts"].as_object_mut().unwrap().remove("header");
        cases.push(shape);
        let mut shape = base.clone();
        shape["continuation"] = json!(true);
        cases.push(shape);
        let mut shape = base.clone();
        shape["membership"]["allResponse"]["ordinaryRows"] = json!(10001);
        cases.push(shape);
        let mut shape = base.clone();
        shape["membership"]["recognizedSections"]["secret-key"] = json!(0);
        cases.push(shape);
        let mut shape = base.clone();
        shape["cursorLocations"]["other"] = json!(-1);
        cases.push(shape);
        let mut shape = base.clone();
        shape["cursorLocations"]["private-token"] = json!(1);
        cases.push(shape);
        let mut shape = base.clone();
        shape["retainedRuleEqualsSelected"] = json!("private-token");
        cases.push(shape);
        let mut shape = base.clone();
        shape.as_object_mut().unwrap().remove("membership");
        cases.push(shape);
        for shape in cases {
            let error =
                parse_youtube_playlist_response_shape(&json!({"responseShape":shape}), false)
                    .unwrap_err();
            assert_eq!(
                error,
                "YouTube playlist response structure is unavailable or invalid"
            );
        }
        let mut limit = base;
        limit["counts"]["contents"]["playlistVideoRenderer"] = json!(10000);
        assert!(
            parse_youtube_playlist_response_shape(&json!({"responseShape":limit}), false).is_ok()
        );
    }

    #[test]
    fn youtube_inspection_request_is_playlist_only_and_bounded() {
        assert!(serde_json::from_value::<YouTubePlaylistInspectionRequest>(
            json!({"playlistId":"PLfixture12345","provider":"spotify"})
        )
        .is_err());
        for playlist_id in [
            "https://www.youtube.com/playlist?list=PLfixture12345",
            "",
            "bad id",
        ] {
            assert!(YouTubePlaylistInspectionRequest {
                playlist_id: playlist_id.into(),
                cursor: None
            }
            .validate()
            .is_err());
        }
        for cursor in [String::new(), "x".repeat(2049), "line\nbreak".into()] {
            assert!(YouTubePlaylistInspectionRequest {
                playlist_id: "PLfixture12345".into(),
                cursor: Some(cursor)
            }
            .validate()
            .is_err());
        }
        assert!(YouTubePlaylistInspectionRequest {
            playlist_id: "PLfixture12345".into(),
            cursor: Some("opaque-token".into())
        }
        .validate()
        .is_ok());
    }

    struct BrowseFixtureTransport {
        calls: Mutex<Vec<(String, Value)>>,
        gapped_album_page: bool,
        invalid_album_position: bool,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for BrowseFixtureTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }

        async fn provider_operation(
            &self,
            _provider: &str,
            operation: &str,
            args: Value,
        ) -> std::result::Result<Value, String> {
            self.calls
                .lock()
                .unwrap()
                .push((operation.to_owned(), args.clone()));
            match operation {
                "status" => Ok(json!({"accountId":"browse-account","displayName":"Fixture"})),
                "getPlaylist" => Ok(json!({
                    "accountId":"browse-account",
                    "playlist":{"id":"1234567890123456789012","url":"https://open.spotify.com/playlist/1234567890123456789012","title":"Fixture","total":2},
                    "entries":[
                        {"entryId":"entry-one","trackId":"abcdefghijklmnopqrstuv","title":"Same track","artist":"Fixture","url":"https://open.spotify.com/track/abcdefghijklmnopqrstuv","plays":0},
                        {"entryId":"entry-two","trackId":"abcdefghijklmnopqrstuv","title":"Same track","artist":"Fixture","url":"https://open.spotify.com/track/abcdefghijklmnopqrstuv"}
                    ],
                    "nextCursor":"next-page","total":2,"complete":false
                })),
                "getAlbum" => Ok(json!({
                    "accountId":"browse-account",
                    "album":{"id":"aaaaaaaaaaaaaaaaaaaaaa","title":"Fixture Album","url":"https://open.spotify.com/album/aaaaaaaaaaaaaaaaaaaaaa"}
                })),
                "getAlbumTracks" => {
                    let offset = args
                        .get("cursor")
                        .and_then(Value::as_str)
                        .unwrap_or("0")
                        .parse::<usize>()
                        .unwrap();
                    if self.gapped_album_page || self.invalid_album_position {
                        let (items, next_cursor, unrecognized_count) = if offset == 0 {
                            (
                                vec![json!({
                                    "trackId":"abcdefghijklmnopqrstuv",
                                    "title":"Duplicate recording",
                                    "artist":"Fixture Artist",
                                    "durationSeconds":180,
                                    "url":"https://open.spotify.com/track/abcdefghijklmnopqrstuv",
                                    "position":if self.invalid_album_position { json!(0) } else { json!(2) }
                                })],
                                json!("2"),
                                1,
                            )
                        } else {
                            (
                                vec![json!({
                                    "trackId":"abcdefghijklmnopqrstuv",
                                    "title":"Duplicate recording",
                                    "artist":"Fixture Artist",
                                    "durationSeconds":180,
                                    "url":"https://open.spotify.com/track/abcdefghijklmnopqrstuv",
                                    "position":3
                                })],
                                Value::Null,
                                0,
                            )
                        };
                        return Ok(json!({
                            "accountId":"browse-account", "items":items,
                            "nextCursor":next_cursor, "total":3,
                            "unrecognizedCount":unrecognized_count
                        }));
                    }
                    let items = (0..REMOTE_BROWSE_PAGE_SIZE).map(|index| json!({
                        "trackId":"abcdefghijklmnopqrstuv",
                        "title":format!("Occurrence {}", offset + index + 1),
                        "artist":"Fixture Artist",
                        "durationSeconds":180,
                        "url":"https://open.spotify.com/track/abcdefghijklmnopqrstuv",
                        "playCount":if offset == 0 && index == 0 { json!(0) } else { Value::Null },
                        "view_count":if offset == 0 && index == 0 { json!(7) } else { Value::Null }
                    })).collect::<Vec<_>>();
                    Ok(json!({
                        "accountId":"browse-account",
                        "items":items,
                        "nextCursor":if offset == 0 { json!("30") } else { Value::Null },
                        "total":60
                    }))
                }
                _ => Err(format!("unexpected operation {operation}")),
            }
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    #[tokio::test]
    async fn playlist_browse_uses_thirty_and_preserves_cursor_and_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let transport = Arc::new(BrowseFixtureTransport {
            calls: Mutex::new(Vec::new()),
            gapped_album_page: false,
            invalid_album_position: false,
        });
        let service = RemotePlaylistService::new(store, transport.clone());
        set_confirmed_duration_account(&service, "browse-account");

        let page = service
            .browse_playlist(
                "spotify",
                "1234567890123456789012",
                Some("opaque-source-cursor"),
            )
            .await
            .unwrap();

        let calls = transport.calls.lock().unwrap();
        let (_, args) = calls
            .iter()
            .find(|(operation, _)| operation == "getPlaylist")
            .unwrap();
        assert_eq!(args["limit"], REMOTE_BROWSE_PAGE_SIZE);
        assert_eq!(args["cursor"], "opaque-source-cursor");
        assert_eq!(page.tracks.len(), 2);
        assert_eq!(page.tracks[0].provider_id, page.tracks[1].provider_id);
        assert_eq!(page.tracks[0].provenance["remoteEntryId"], "entry-one");
        assert_eq!(page.tracks[1].provenance["remoteEntryId"], "entry-two");
        assert_eq!(page.tracks[0].provenance["plays"], 0);
        assert!(page.tracks[1].provenance.get("plays").is_none());
    }

    #[tokio::test]
    async fn album_browse_keeps_duplicate_recordings_as_distinct_global_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let transport = Arc::new(BrowseFixtureTransport {
            calls: Mutex::new(Vec::new()),
            gapped_album_page: false,
            invalid_album_position: false,
        });
        let service = RemotePlaylistService::new(store, transport.clone());
        set_confirmed_duration_account(&service, "browse-account");

        let first = service
            .browse_album_tracks("aaaaaaaaaaaaaaaaaaaaaa", None)
            .await
            .unwrap();
        let second = service
            .browse_album_tracks("aaaaaaaaaaaaaaaaaaaaaa", first.next_cursor.as_deref())
            .await
            .unwrap();

        assert_eq!(first.tracks.len(), REMOTE_BROWSE_PAGE_SIZE);
        assert_eq!(second.tracks.len(), REMOTE_BROWSE_PAGE_SIZE);
        let first_track = &first.tracks[0];
        let second_track = &first.tracks[1];
        let last_first = first.tracks.last().unwrap();
        let first_second = &second.tracks[0];
        let last_second = second.tracks.last().unwrap();
        assert_eq!(first_track.id, "spotify:abcdefghijklmnopqrstuv");
        assert_eq!(first_track.id, second_track.id);
        assert_eq!(first_track.provider_id, "abcdefghijklmnopqrstuv");
        assert_eq!(
            first_track.source_url,
            "https://open.spotify.com/track/abcdefghijklmnopqrstuv"
        );
        assert_eq!(first_track.provenance["plays"], 0);
        assert_eq!(first_track.provenance["views"], 7);
        assert_eq!(
            first_track.provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_0"
        );
        assert_eq!(
            second_track.provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_1"
        );
        assert_eq!(
            last_first.provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_29"
        );
        assert_eq!(
            first_second.provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_30"
        );
        assert_eq!(
            last_second.provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_59"
        );
        for track in first.tracks.iter().chain(&second.tracks) {
            let entry_id = track.provenance["remoteEntryId"].as_str().unwrap();
            assert!(valid_remote_id(entry_id));
            assert!(entry_id.len() <= 128);
        }
        let max_cursor_key = spotify_album_occurrence_id("aaaaaaaaaaaaaaaaaaaaaa", 100_000);
        assert!(valid_remote_id(&max_cursor_key));
        assert!(max_cursor_key.len() <= 128);
        assert_eq!(last_first.provenance["position"], 30);
        assert_eq!(first_second.provenance["position"], 31);
        assert_eq!(last_second.provenance["position"], 60);
        assert_eq!(first_track.provenance["positionScope"], "album");
    }

    #[tokio::test]
    async fn album_browse_preserves_raw_positions_across_filtered_duplicate_pages() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let transport = Arc::new(BrowseFixtureTransport {
            calls: Mutex::new(Vec::new()),
            gapped_album_page: true,
            invalid_album_position: false,
        });
        let service = RemotePlaylistService::new(store, transport);
        set_confirmed_duration_account(&service, "browse-account");

        let first = service
            .browse_album_tracks("aaaaaaaaaaaaaaaaaaaaaa", None)
            .await
            .unwrap();
        let second = service
            .browse_album_tracks("aaaaaaaaaaaaaaaaaaaaaa", first.next_cursor.as_deref())
            .await
            .unwrap();

        assert_eq!(first.tracks.len(), 1);
        assert_eq!(second.tracks.len(), 1);
        assert_eq!(first.tracks[0].provider_id, second.tracks[0].provider_id);
        assert_eq!(first.tracks[0].provenance["position"], 2);
        assert_eq!(second.tracks[0].provenance["position"], 3);
        assert_eq!(
            first.tracks[0].provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_1"
        );
        assert_eq!(
            second.tracks[0].provenance["remoteEntryId"],
            "album_aaaaaaaaaaaaaaaaaaaaaa_2"
        );
    }

    #[tokio::test]
    async fn album_browse_rejects_invalid_explicit_source_positions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let transport = Arc::new(BrowseFixtureTransport {
            calls: Mutex::new(Vec::new()),
            gapped_album_page: false,
            invalid_album_position: true,
        });
        let service = RemotePlaylistService::new(store, transport);
        set_confirmed_duration_account(&service, "browse-account");

        let error = service
            .browse_album_tracks("aaaaaaaaaaaaaaaaaaaaaa", None)
            .await
            .unwrap_err();
        assert!(error.contains("invalid album track position"));
    }

    struct DurationFixtureTransport {
        status_account: Mutex<String>,
        search_account: Mutex<String>,
        searches: Mutex<Vec<String>>,
        search_delay: Mutex<std::time::Duration>,
        delayed_search_ids: Mutex<HashSet<String>>,
        delayed_search_delay: Mutex<std::time::Duration>,
        get_track_error: Mutex<Option<String>>,
        search_error: Mutex<Option<String>>,
        get_track_blocker: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    }
    #[async_trait::async_trait]
    impl ProviderTransport for DurationFixtureTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }
        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
        ) -> std::result::Result<Value, String> {
            match operation {
                "status" => Ok(
                    json!({"accountId":self.status_account.lock().unwrap().clone(),"displayName":"Fixture"}),
                ),
                "getTrack" => {
                    let id = args
                        .get("trackId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    self.searches.lock().unwrap().push(id.clone());
                    if let Some(error) = self.get_track_error.lock().unwrap().clone() {
                        return Err(error);
                    }
                    let blocker = self.get_track_blocker.lock().unwrap().clone();
                    if let Some((entered, release)) = blocker {
                        entered.notify_one();
                        release.notified().await;
                    }
                    let delay = if self.delayed_search_ids.lock().unwrap().contains(&id) {
                        *self.delayed_search_delay.lock().unwrap()
                    } else {
                        *self.search_delay.lock().unwrap()
                    };
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    Ok(
                        json!({"accountId":self.search_account.lock().unwrap().clone(),"items":[{
                            "id":id,"kind":"track","title":"Fixture track","subtitle":"Fixture artist",
                            "url":format!("https://open.spotify.com/track/{id}"),"artworkUrl":null,
                            "data":{"id":id,"title":"Fixture track","artist":"Fixture artist",
                                "album":"Fixture album","durationSeconds":245.48,
                                "url":format!("https://open.spotify.com/track/{id}")}
                        }]}),
                    )
                }
                "search" => {
                    let id = args
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    self.searches.lock().unwrap().push(id.clone());
                    if let Some(error) = self.search_error.lock().unwrap().clone() {
                        return Err(error);
                    }
                    let delay = if self.delayed_search_ids.lock().unwrap().contains(&id) {
                        *self.delayed_search_delay.lock().unwrap()
                    } else {
                        *self.search_delay.lock().unwrap()
                    };
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    Ok(
                        json!({"accountId":self.search_account.lock().unwrap().clone(),"items":[{
                            "kind":"track","id":id,"title":"Fixture track",
                            "url":format!("https://open.spotify.com/track/{id}"),
                            "data":{"durationSeconds":245.48}
                        }]}),
                    )
                }
                _ => Err(format!("unexpected operation {provider}/{operation}")),
            }
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    fn duration_fixture_service(
        account: &str,
    ) -> (
        tempfile::TempDir,
        Arc<Store>,
        RemotePlaylistService,
        Arc<DurationFixtureTransport>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let transport = Arc::new(DurationFixtureTransport {
            status_account: Mutex::new(account.into()),
            search_account: Mutex::new(account.into()),
            searches: Mutex::new(Vec::new()),
            search_delay: Mutex::new(std::time::Duration::ZERO),
            delayed_search_ids: Mutex::new(HashSet::new()),
            delayed_search_delay: Mutex::new(std::time::Duration::from_millis(80)),
            get_track_error: Mutex::new(None),
            search_error: Mutex::new(None),
            get_track_blocker: Mutex::new(None),
        });
        let service = RemotePlaylistService::new(store.clone(), transport.clone());
        set_confirmed_duration_account(&service, account);
        (dir, store, service, transport)
    }

    fn set_confirmed_duration_account(service: &RemotePlaylistService, account_id: &str) {
        service.account_cache.lock().unwrap().insert(
            "spotify".into(),
            CachedAccount {
                id: account_id.into(),
                name: Some("Fixture".into()),
                checked_at: chrono::Utc::now(),
            },
        );
    }

    struct BlockingYouTubeApplyTransport {
        get_playlist_calls: std::sync::atomic::AtomicUsize,
        fetch_entered: tokio::sync::Notify,
        release_fetch: tokio::sync::Notify,
        apply_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for BlockingYouTubeApplyTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }
        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            _args: Value,
        ) -> std::result::Result<Value, String> {
            match (provider, operation) {
                ("youtube", "status") => {
                    Ok(json!({"accountId":"fixture-account","displayName":"Fixture"}))
                }
                ("youtube", "getPlaylist") => {
                    if self.get_playlist_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        self.fetch_entered.notify_one();
                        self.release_fetch.notified().await;
                    }
                    Ok(json!({
                        "accountId":"fixture-account",
                        "playlist":{"id":"PLfixture123","title":"Fixture playlist","url":"https://www.youtube.com/playlist?list=PLfixture123","canEdit":true},
                        "entries":[{
                            "entryId":"occ-1","trackId":"existing123","title":"Existing track",
                            "artist":"Fixture artist","url":"https://www.youtube.com/watch?v=existing123",
                            "available":true
                        }],"revision":"rev-1","complete":true,"total":1
                    }))
                }
                ("youtube", "applyPlaylistEdits") => {
                    self.apply_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"accountId":"fixture-account","applied":1}))
                }
                _ => Err(format!("unexpected operation {provider}/{operation}")),
            }
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    struct MutableYouTubeSyncTransport {
        entries: Mutex<Vec<RemoteEntry>>,
        revision: Mutex<u64>,
        apply_actions: Mutex<Vec<Vec<Value>>>,
        switched_account: AtomicBool,
        switch_on_first_apply: AtomicBool,
        playlist_blocker: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for MutableYouTubeSyncTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }

        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
        ) -> std::result::Result<Value, String> {
            if provider != "youtube" {
                return Err(format!("unexpected provider {provider}"));
            }
            match operation {
                "status" => Ok(json!({
                    "accountId": if self.switched_account.load(Ordering::SeqCst) { "other-account" } else { "fixture-account" },
                    "displayName":"Fixture"
                })),
                "getPlaylist" => {
                    let blocker = self.playlist_blocker.lock().unwrap().clone();
                    if let Some((entered, release)) = blocker {
                        entered.notify_one();
                        release.notified().await;
                    }
                    let entries = self.entries.lock().unwrap().clone();
                    let revision = *self.revision.lock().unwrap();
                    let total = entries.len();
                    Ok(json!({
                        "accountId":"fixture-account",
                        "playlist":{"id":"PLfixture123","title":"Fixture playlist","url":"https://www.youtube.com/playlist?list=PLfixture123","canEdit":true},
                        "entries":entries,
                        "revision":format!("rev-{revision}"),
                        "complete":true,
                        "total":total
                    }))
                }
                "applyPlaylistEdits" => {
                    if args.get("previewApproved").and_then(Value::as_bool) != Some(true) {
                        return Err("mutation did not carry the preview approval".into());
                    }
                    let actions = args
                        .get("actions")
                        .and_then(Value::as_array)
                        .ok_or_else(|| "missing mutation actions".to_owned())?
                        .clone();
                    let mut entries = self.entries.lock().unwrap();
                    for action in &actions {
                        match action.get("type").and_then(Value::as_str) {
                            Some("remove") => {
                                let id = action
                                    .get("entryId")
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| "remove missing occurrence".to_owned())?;
                                let index =
                                    entries
                                        .iter()
                                        .position(|entry| entry.entry_id == id)
                                        .ok_or_else(|| format!("missing remove occurrence {id}"))?;
                                entries.remove(index);
                            }
                            Some("add") => {
                                let track_id = action
                                    .get("trackId")
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| "add missing video".to_owned())?;
                                let id = format!(
                                    "added-{}-{}",
                                    *self.revision.lock().unwrap(),
                                    entries.len()
                                );
                                entries.push(youtube_remote_entry(&id, track_id));
                            }
                            Some("moveBefore") => {
                                let id = action
                                    .get("entryId")
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| "move missing occurrence".to_owned())?;
                                let successor_value = action
                                    .get("successorEntryId")
                                    .ok_or_else(|| "move missing successor field".to_owned())?;
                                let successor = match successor_value {
                                    Value::Null => None,
                                    Value::String(successor) => Some(successor.as_str()),
                                    _ => {
                                        return Err("move successor must be a string or null".into())
                                    }
                                };
                                let index =
                                    entries
                                        .iter()
                                        .position(|entry| entry.entry_id == id)
                                        .ok_or_else(|| format!("missing move occurrence {id}"))?;
                                let entry = entries.remove(index);
                                if let Some(successor) = successor {
                                    let target = entries
                                        .iter()
                                        .position(|candidate| candidate.entry_id == successor)
                                        .ok_or_else(|| {
                                            format!("missing move successor {successor}")
                                        })?;
                                    entries.insert(target, entry);
                                } else {
                                    entries.push(entry);
                                }
                            }
                            other => return Err(format!("unexpected YouTube action {other:?}")),
                        }
                    }
                    self.apply_actions.lock().unwrap().push(actions.clone());
                    *self.revision.lock().unwrap() += 1;
                    if self.switch_on_first_apply.load(Ordering::SeqCst) {
                        self.switched_account.store(true, Ordering::SeqCst);
                    }
                    Ok(json!({"accountId":"fixture-account","applied":actions.len()}))
                }
                _ => Err(format!("unexpected operation {provider}/{operation}")),
            }
        }

        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    fn youtube_remote_entry(entry_id: &str, track_id: &str) -> RemoteEntry {
        RemoteEntry {
            entry_id: entry_id.into(),
            track_id: track_id.into(),
            title: format!("Track {track_id}"),
            artist: "Fixture artist".into(),
            album: None,
            duration_seconds: None,
            url: format!("https://www.youtube.com/watch?v={track_id}"),
            artwork_url: None,
            available: true,
            plays: None,
            views: None,
        }
    }

    fn youtube_working_track(track_id: &str) -> Track {
        Track {
            id: format!("youtube:{track_id}"),
            provider: "youtube".into(),
            provider_id: track_id.into(),
            source_url: format!("https://www.youtube.com/watch?v={track_id}"),
            title: format!("Track {track_id}"),
            artist: "Fixture artist".into(),
            ..Track::default()
        }
    }

    fn youtube_sync_fixture(
        initial: Vec<RemoteEntry>,
        desired: Vec<Track>,
    ) -> (
        tempfile::TempDir,
        Arc<Store>,
        RemotePlaylistService,
        Arc<MutableYouTubeSyncTransport>,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let collection = store
            .create_collection("YouTube fixture working copy")
            .unwrap();
        store
            .replace_playlist_entries(&collection.id, &desired)
            .unwrap();
        let content_hash = hash_json(&initial).unwrap();
        let snapshot = RemoteSnapshot {
            entries: initial.clone(),
            revision: "rev-0".into(),
            content_hash,
            complete: true,
        };
        let binding = RemotePlaylistBindingStatus {
            write_capabilities: None,
            binding_id: "youtube-binding-fixture".into(),
            provider: "youtube".into(),
            playlist_id: "PLfixture123".into(),
            title: "Fixture playlist".into(),
            url: "https://www.youtube.com/playlist?list=PLfixture123".into(),
            account_id: "fixture-account".into(),
            owner_id: None,
            owner_name: None,
            owner_url: None,
            visibility: None,
            collection_id: collection.id.clone(),
            source_collection_id: None,
            collection_name: collection.name,
            track_count: initial.len(),
            revision: "rev-0".into(),
            complete: true,
            can_edit: true,
            refreshed_at: "2026-10-15T00:00:00Z".into(),
        };
        let mut state = RemoteState::default();
        state.bindings.push(binding);
        state
            .baselines
            .insert("youtube-binding-fixture".into(), snapshot.clone());
        state
            .observed
            .insert("youtube-binding-fixture".into(), snapshot);
        state.allow_write_sync.insert("youtube".into());
        let transport = Arc::new(MutableYouTubeSyncTransport {
            entries: Mutex::new(initial),
            revision: Mutex::new(0),
            apply_actions: Mutex::new(Vec::new()),
            switched_account: AtomicBool::new(false),
            switch_on_first_apply: AtomicBool::new(false),
            playlist_blocker: Mutex::new(None),
        });
        let service = RemotePlaylistService::new(store.clone(), transport.clone());
        service.save_state(&state).unwrap();
        service.account_cache.lock().unwrap().insert(
            "youtube".into(),
            CachedAccount {
                id: "fixture-account".into(),
                name: Some("Fixture".into()),
                checked_at: chrono::Utc::now(),
            },
        );
        (dir, store, service, transport, collection.id)
    }

    struct WritebackSearchFixtureTransport {
        rows: Vec<Value>,
        delegate: Arc<dyn ProviderTransport>,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for WritebackSearchFixtureTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }
        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
        ) -> std::result::Result<Value, String> {
            match operation {
                "status" => Ok(json!({"accountId":"fixture-account","displayName":"Fixture"})),
                "search" => Ok(json!({"accountId":"fixture-account","items":self.rows})),
                _ => {
                    self.delegate
                        .provider_operation(provider, operation, args)
                        .await
                }
            }
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    fn writeback_search_row(
        id: &str,
        title: &str,
        duration: Option<f64>,
        available: bool,
    ) -> Value {
        let mut data = json!({"artist":"Fixture artist", "available":available});
        if let Some(duration) = duration {
            data["durationSeconds"] = json!(duration);
        }
        json!({"id":id,"kind":"track","title":title,"subtitle":"Fixture artist",
            "url":format!("https://www.youtube.com/watch?v={id}"),"data":data})
    }

    #[tokio::test]
    async fn writeback_search_returns_reviewable_candidates_and_tokens_for_repeated_local_occurrences(
    ) {
        let local = Track {
            id: "fixture:local-song".into(),
            provider: "fixture".into(),
            provider_id: "local-song".into(),
            source_url: "https://example.test/local-song".into(),
            title: "Song".into(),
            artist: "Fixture artist".into(),
            duration_seconds: Some(245.0),
            ..Track::default()
        };
        let (_dir, store, mut service, transport, collection_id) =
            youtube_sync_fixture(vec![], vec![local.clone(), local]);
        let track = store
            .list_tracks("collection", Some(&collection_id))
            .unwrap()[0]
            .clone();
        service.bridge = Arc::new(WritebackSearchFixtureTransport {
            delegate: transport,
            rows: vec![
                writeback_search_row("abcdefghijk", "Song", Some(246.0), true),
                writeback_search_row("short-id", "Song", Some(245.0), true),
                writeback_search_row("unavail0001", "Song", Some(245.0), false),
                writeback_search_row("livevid0001", "Song - Live", Some(245.0), true),
                writeback_search_row("longvid0001", "Song", Some(300.0), true),
            ],
        });

        let result = service
            .search_remote_playlist_writeback_candidates("youtube-binding-fixture", &track.id)
            .await
            .unwrap();
        assert_eq!(result.binding_id, "youtube-binding-fixture");
        assert_eq!(result.provider, "youtube");
        assert_eq!(result.local_track_id, track.id);
        assert_eq!(result.candidates.len(), 1);
        let candidate = &result.candidates[0];
        let rendered = serde_json::to_value(&result).unwrap();
        assert_eq!(rendered["bindingId"], "youtube-binding-fixture");
        assert_eq!(rendered["localTrackId"], track.id);
        assert!(rendered["candidates"][0]["candidateToken"]
            .as_str()
            .is_some_and(|token| !token.is_empty()));
        assert_eq!(rendered["candidates"][0]["durationSeconds"], 246.0);
        assert_eq!(candidate.title, "Song");
        assert_eq!(candidate.artist, "Fixture artist");
        assert_eq!(candidate.duration_seconds, Some(246.0));
        assert_eq!(candidate.url, "https://www.youtube.com/watch?v=abcdefghijk");

        let preview = service
            .get_sync_preview(
                "youtube-binding-fixture",
                Some(vec![ReviewedRemoteTrackMatch {
                    local_track_id: track.id.clone(),
                    candidate_token: candidate.candidate_token.clone(),
                }]),
            )
            .await
            .unwrap();
        assert!(preview.changes.unresolved.is_empty());
        assert_eq!(preview.changes.add.len(), 2);
        assert!(preview
            .changes
            .add
            .iter()
            .all(|change| change.track_uri == "youtube:video:abcdefghijk"));
    }

    #[tokio::test]
    async fn youtube_sync_entrypoints_apply_duplicate_occurrences_add_remove_and_reorder_then_persist(
    ) {
        let initial = vec![
            youtube_remote_entry("duplicate-occurrence-a", "duplicate01"),
            youtube_remote_entry("duplicate-occurrence-b", "duplicate01"),
            youtube_remote_entry("removed-occurrence", "removev0001"),
        ];
        let desired = vec![
            youtube_working_track("newvid00001"),
            youtube_working_track("duplicate01"),
            youtube_working_track("duplicate01"),
        ];
        let (_dir, store, service, transport, collection_id) =
            youtube_sync_fixture(initial, desired);

        let preview = service
            .get_sync_preview("youtube-binding-fixture", None)
            .await
            .unwrap();
        assert!(!preview.conflict);
        assert!(preview
            .changes
            .add
            .iter()
            .any(|change| change.track_uri == "youtube:video:newvid00001"));
        assert!(preview
            .changes
            .remove
            .iter()
            .flat_map(|change| &change.entry_ids)
            .any(|id| id == "removed-occurrence"));

        let applied = service
            .apply_sync_preview(&preview.preview_id, None)
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(
            transport.apply_actions.lock().unwrap().len(),
            2,
            "expected one add/remove phase followed by a reorder phase"
        );
        let actions = transport.apply_actions.lock().unwrap().clone();
        assert!(actions[0]
            .iter()
            .any(|action| action["type"] == "remove" && action["entryId"] == "removed-occurrence"));
        assert!(actions[0]
            .iter()
            .any(|action| action["type"] == "add" && action["trackId"] == "newvid00001"));
        assert!(actions[1]
            .iter()
            .any(|action| action["type"] == "moveBefore"));

        let final_ids = transport
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|entry| entry.track_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            final_ids,
            vec![
                "newvid00001".to_owned(),
                "duplicate01".to_owned(),
                "duplicate01".to_owned()
            ]
        );
        let local_ids = store
            .list_tracks("collection", Some(&collection_id))
            .unwrap()
            .iter()
            .map(|track| track.provider_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            local_ids, final_ids,
            "persisted working copy must retain both identical video occurrences in reviewed order"
        );
        let persisted = service.load_state().unwrap();
        let baseline = persisted.baselines.get("youtube-binding-fixture").unwrap();
        let observed = persisted.observed.get("youtube-binding-fixture").unwrap();
        assert_eq!(baseline.content_hash, observed.content_hash);
        assert_eq!(
            baseline.content_hash,
            hash_json(&transport.entries.lock().unwrap().clone()).unwrap()
        );
        assert!(applied.changes.add.is_empty());
        assert!(applied.changes.remove.is_empty());
        assert!(applied.changes.move_entries.is_empty());
    }

    #[tokio::test]
    async fn youtube_sync_account_switch_after_first_phase_consumes_preview_before_any_second_write(
    ) {
        let initial = vec![
            youtube_remote_entry("keep-occurrence", "keepvid0001"),
            youtube_remote_entry("remove-occurrence", "removev0001"),
        ];
        // Adding appends remotely, so this order requires a second reorder mutation.
        let desired = vec![
            youtube_working_track("newvid00001"),
            youtube_working_track("keepvid0001"),
        ];
        let (_dir, _store, service, transport, _collection_id) =
            youtube_sync_fixture(initial, desired);
        transport
            .switch_on_first_apply
            .store(true, Ordering::SeqCst);
        let preview = service
            .get_sync_preview("youtube-binding-fixture", None)
            .await
            .unwrap();
        // The transport changes identity immediately after the first successful phase.
        // It deliberately reports a different account from the next status check.
        let result = service.apply_sync_preview(&preview.preview_id, None).await;
        assert!(result.unwrap_err().contains("account changed"));
        let actions = transport.apply_actions.lock().unwrap();
        assert_eq!(
            actions.len(),
            1,
            "account verification must stop before the reorder phase"
        );
        assert!(actions[0].iter().any(|action| action["type"] == "remove"));
        assert!(actions[0].iter().any(|action| action["type"] == "add"));
        drop(actions);
        assert!(service
            .apply_sync_preview(&preview.preview_id, None)
            .await
            .unwrap_err()
            .contains("preview expired"));
    }

    #[tokio::test]
    async fn local_edits_during_remote_preview_validation_prevent_writeback() {
        let initial = vec![youtube_remote_entry("keep-occurrence", "keepvid0001")];
        let desired = vec![
            youtube_working_track("keepvid0001"),
            youtube_working_track("newvid00001"),
        ];
        let (_dir, store, service, transport, collection_id) =
            youtube_sync_fixture(initial, desired);
        let preview = service
            .get_sync_preview("youtube-binding-fixture", None)
            .await
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *transport.playlist_blocker.lock().unwrap() = Some((entered.clone(), release.clone()));
        let task_service = service.clone();
        let apply = tokio::spawn(async move {
            task_service
                .apply_sync_preview(&preview.preview_id, None)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("apply reached remote validation");
        store
            .replace_playlist_entries(&collection_id, &[youtube_working_track("othervid001")])
            .unwrap();
        release.notify_one();
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), apply)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("Local playlist changed"), "{error}");
        assert!(transport.apply_actions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_youtube_apply_entrypoint_dispatches_exactly_one_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let collection = store.create_collection("Concurrent fixture").unwrap();
        let transport = Arc::new(BlockingYouTubeApplyTransport {
            get_playlist_calls: std::sync::atomic::AtomicUsize::new(0),
            fetch_entered: tokio::sync::Notify::new(),
            release_fetch: tokio::sync::Notify::new(),
            apply_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let service = RemotePlaylistService::new(store, transport.clone());
        let current_entries = vec![RemoteEntry {
            entry_id: "occ-1".into(),
            track_id: "existing123".into(),
            title: "Existing track".into(),
            artist: "Fixture artist".into(),
            album: None,
            duration_seconds: None,
            url: "https://www.youtube.com/watch?v=existing123".into(),
            artwork_url: None,
            available: true,
            plays: None,
            views: None,
        }];
        let remote_hash = hash_json(&current_entries).unwrap();
        let binding = RemotePlaylistBindingStatus {
            write_capabilities: None,
            binding_id: "binding-fixture".into(),
            provider: "youtube".into(),
            playlist_id: "PLfixture123".into(),
            title: "Fixture playlist".into(),
            url: "https://www.youtube.com/playlist?list=PLfixture123".into(),
            account_id: "fixture-account".into(),
            owner_id: None,
            owner_name: None,
            owner_url: None,
            visibility: None,
            collection_id: collection.id.clone(),
            source_collection_id: None,
            collection_name: "Fixture playlist".into(),
            track_count: 0,
            revision: "rev-1".into(),
            complete: true,
            can_edit: true,
            refreshed_at: "2026-10-15T00:00:00Z".into(),
        };
        let mut state = RemoteState::default();
        state.bindings.push(binding);
        state.allow_write_sync.insert("youtube".into());
        service.save_state(&state).unwrap();
        let preview = RemotePlaylistSyncPreview {
            write_capabilities: None,
            preview_id: "preview-fixture".into(),
            binding_id: "binding-fixture".into(),
            provider: "youtube".into(),
            playlist_id: "PLfixture123".into(),
            local_collection_id: collection.id.clone(),
            baseline_revision: "rev-1".into(),
            remote_revision: "rev-1".into(),
            conflict: false,
            applied: false,
            can_edit: true,
            write_sync_enabled: true,
            changes: RemotePlaylistSyncChanges {
                add: vec![RemoteAddChange {
                    track_uri: "youtube:video:newtrack123".into(),
                }],
                remove: vec![],
                move_entries: vec![],
                unresolved: vec![],
            },
            summary: "fixture".into(),
        };
        let stored = StoredPreview {
            preview,
            remote_hash,
            account_id: "fixture-account".into(),
            desired_uris: vec![
                "youtube:video:existing123".into(),
                "youtube:video:newtrack123".into(),
            ],
            local_identity_hashes: HashMap::new(),
            local_order_hash: hash_json(&Vec::<String>::new()).unwrap(),
        };
        let mut second_stored_preview = stored.clone();
        second_stored_preview.preview.preview_id = "preview-fixture-2".into();
        {
            let mut previews = service.previews.lock().unwrap();
            previews.insert("preview-fixture".into(), stored);
            previews.insert("preview-fixture-2".into(), second_stored_preview);
        }

        // A pre-mutation review mismatch must release the binding claim immediately.
        let mismatched = RemotePlaylistSyncChanges {
            add: vec![],
            remove: vec![],
            move_entries: vec![],
            unresolved: vec![],
        };
        assert!(service
            .apply_sync_preview("preview-fixture", Some(mismatched))
            .await
            .err()
            .unwrap()
            .contains("complete reviewed preview"));
        assert_eq!(transport.get_playlist_calls.load(Ordering::SeqCst), 0);

        let fetch_entered = transport.fetch_entered.notified();
        let first_service = service.clone();
        let first = tokio::spawn(async move {
            first_service
                .apply_sync_preview("preview-fixture", None)
                .await
        });
        if tokio::time::timeout(std::time::Duration::from_secs(2), fetch_entered)
            .await
            .is_err()
        {
            first.abort();
            panic!("first YouTube apply did not reach the mocked playlist fetch");
        }
        // The first caller is blocked in remote validation after loading the stored preview.
        // A second call must be rejected by the real apply entrypoint before provider mutation.
        let second = service.apply_sync_preview("preview-fixture", None).await;
        assert!(second.err().unwrap().contains("already being applied"));
        let distinct_preview = service.apply_sync_preview("preview-fixture-2", None).await;
        assert!(distinct_preview
            .err()
            .unwrap()
            .contains("already being applied"));
        assert_eq!(transport.apply_calls.load(Ordering::SeqCst), 0);

        transport.release_fetch.notify_one();
        // First may report post-write verification failure because the fixture intentionally
        // keeps its remote snapshot empty; the dispatch should still happen exactly once.
        let first_result = first.await.unwrap();
        assert_eq!(
            transport.apply_calls.load(Ordering::SeqCst),
            1,
            "first apply returned before mock mutation: {:?}",
            first_result.err()
        );
        assert!(service.claim_preview_apply("binding-fixture").is_ok());
    }

    #[tokio::test]
    async fn preview_apply_claim_releases_when_owner_future_is_cancelled() {
        let (_dir, _store, service, _transport) = duration_fixture_service("fixture-account");
        let entered = Arc::new(tokio::sync::Notify::new());
        let task_service = service.clone();
        let task_entered = entered.clone();
        let task = tokio::spawn(async move {
            let _claim = task_service
                .claim_preview_apply("binding-cancel-fixture")
                .unwrap();
            task_entered.notify_one();
            std::future::pending::<()>().await;
        });
        entered.notified().await;
        task.abort();
        let _ = task.await;
        assert!(service
            .claim_preview_apply("binding-cancel-fixture")
            .is_ok());
    }

    struct NoProviderOperationTransport {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for NoProviderOperationTransport {
        fn provider_operations_supported(&self) -> bool {
            false
        }
        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            _args: Value,
        ) -> std::result::Result<Value, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(format!(
                "unexpected provider operation {provider}/{operation}"
            ))
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    struct FixturePublicDurationTransport {
        durations: HashMap<String, f64>,
        requests: Mutex<Vec<String>>,
        blocker: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        fail_with: Option<String>,
    }

    #[async_trait::async_trait]
    impl PublicSpotifyDurationTransport for FixturePublicDurationTransport {
        async fn duration_seconds(&self, track_id: &str) -> Result<f64> {
            self.requests.lock().unwrap().push(track_id.to_owned());
            if let Some((entered, release)) = self.blocker.as_ref() {
                entered.notify_one();
                release.notified().await;
            }
            if let Some(error) = self.fail_with.as_ref() {
                return Err(error.clone());
            }
            self.durations
                .get(track_id)
                .copied()
                .ok_or_else(|| "TRACK_NOT_FOUND: fixture has no public track".into())
        }
    }

    fn spotify_entity_html(entity: Value) -> String {
        let data = json!({"props":{"pageProps":{"state":{"data":{"entity":entity}}}}});
        format!("<script id=\"__NEXT_DATA__\" type=\"application/json\">{data}</script>")
    }

    #[test]
    fn public_duration_rate_limit_delay_is_bounded() {
        assert_eq!(
            duration_rate_limit_cooldown_seconds("RATE_LIMITED: retry after 36 seconds"),
            36
        );
        assert_eq!(
            duration_rate_limit_cooldown_seconds("RATE_LIMITED: retry after 99999 seconds"),
            PUBLIC_SPOTIFY_RATE_LIMIT_MAX_COOLDOWN_SECS
        );
        assert_eq!(
            duration_rate_limit_cooldown_seconds("RATE_LIMITED: unparseable"),
            PUBLIC_SPOTIFY_RATE_LIMIT_DEFAULT_COOLDOWN.as_secs()
        );
    }

    #[tokio::test]
    async fn anonymous_spotify_embed_stream_enforces_body_limit() {
        let exact = "x".repeat(PUBLIC_SPOTIFY_EMBED_BODY_LIMIT);
        let body = collect_bounded_spotify_embed_body(futures_util::stream::iter(vec![Ok::<
            _,
            std::convert::Infallible,
        >(
            exact
        )]))
        .await
        .unwrap();
        assert_eq!(body.len(), PUBLIC_SPOTIFY_EMBED_BODY_LIMIT);
        let oversized = "x".repeat(PUBLIC_SPOTIFY_EMBED_BODY_LIMIT + 1);
        assert!(
            collect_bounded_spotify_embed_body(futures_util::stream::iter(vec![Ok::<
                _,
                std::convert::Infallible,
            >(
                oversized
            ),]))
            .await
            .unwrap_err()
            .starts_with("PUBLIC_TRACK_TOO_LARGE:")
        );
    }

    #[tokio::test]
    async fn anonymous_spotify_embed_client_does_not_follow_redirects() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut request)
                .await
                .unwrap();
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let client = anonymous_spotify_embed_client().unwrap();
        let response = client
            .get(format!("http://{address}/embed"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        server.await.unwrap();
    }

    #[test]
    fn anonymous_spotify_embed_parser_verifies_exact_track_and_millisecond_duration() {
        let id = "0Kvqc2zK0UKPTK6pAgyv1v";
        let entity = json!({
            "type":"track","id":id,"uri":format!("spotify:track:{id}"),
            "name":"Poso S' Agapo","duration":245480
        });
        assert_eq!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(entity)).unwrap(),
            245.48
        );

        let wrong_id = json!({"type":"track","id":"1Kvqc2zK0UKPTK6pAgyv1v","uri":"spotify:track:1Kvqc2zK0UKPTK6pAgyv1v","duration":245480});
        assert!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(wrong_id))
                .unwrap_err()
                .starts_with("PUBLIC_TRACK_MISMATCH:")
        );
        let wrong_uri =
            json!({"type":"track","id":id,"uri":"spotify:track:wrong","duration":245480});
        assert!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(wrong_uri))
                .unwrap_err()
                .starts_with("PUBLIC_TRACK_MISMATCH:")
        );
        let wrong_type =
            json!({"type":"album","id":id,"uri":format!("spotify:track:{id}"),"duration":245480});
        assert!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(wrong_type))
                .unwrap_err()
                .starts_with("PUBLIC_TRACK_MISMATCH:")
        );
        let unavailable = json!({"type":"track","id":id,"uri":format!("spotify:track:{id}"),"duration":0,"isPlayable":false,"playabilityReason":"NOT_AVAILABLE_IN_MARKET"});
        assert!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(unavailable))
                .unwrap_err()
                .starts_with("TRACK_NOT_FOUND:")
        );
        let unavailable_with_valid_duration = json!({"type":"track","id":id,"uri":format!("spotify:track:{id}"),"duration":245480,"isPlayable":false,"playabilityReason":"NOT_AVAILABLE_IN_MARKET"});
        assert_eq!(
            parse_anonymous_spotify_embed_duration(
                id,
                &spotify_entity_html(unavailable_with_valid_duration)
            )
            .unwrap(),
            245.48
        );
        let missing_reason = json!({"type":"track","id":id,"uri":format!("spotify:track:{id}"),"duration":null,"isPlayable":false,"playabilityReason":" "});
        assert!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(missing_reason))
                .unwrap_err()
                .starts_with("PUBLIC_TRACK_INVALID:")
        );
        let too_long = json!({"type":"track","id":id,"uri":format!("spotify:track:{id}"),"duration":86_400_001});
        assert!(
            parse_anonymous_spotify_embed_duration(id, &spotify_entity_html(too_long))
                .unwrap_err()
                .starts_with("PUBLIC_TRACK_INVALID:")
        );
        assert!(parse_anonymous_spotify_embed_duration(
            id,
            &" ".repeat(PUBLIC_SPOTIFY_EMBED_BODY_LIMIT + 1)
        )
        .unwrap_err()
        .starts_with("PUBLIC_TRACK_INVALID:"));
    }

    #[tokio::test]
    async fn explicit_public_duration_repair_runs_without_account_and_uses_guarded_store_fill() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let found_id = "0Kvqc2zK0UKPTK6pAgyv1v";
        let absent_id = "1234567890123456789012";
        for id in [found_id, absent_id] {
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id.into(),
                    source_url: format!("https://open.spotify.com/track/{id}"),
                    duration_seconds: None,
                    ..Track::default()
                })
                .unwrap();
        }
        let provider = Arc::new(NoProviderOperationTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let public = Arc::new(FixturePublicDurationTransport {
            durations: HashMap::from([(found_id.to_owned(), 245.48)]),
            requests: Mutex::new(Vec::new()),
            blocker: None,
            fail_with: None,
        });
        let service = RemotePlaylistService::new_with_public_spotify_duration(
            store.clone(),
            provider.clone(),
            public.clone(),
        );
        assert!(service
            .account_cache
            .lock()
            .unwrap()
            .get("spotify")
            .is_none());

        let result = service.repair_spotify_durations_public().await.unwrap();
        assert!(result.complete);
        assert_eq!(result.changed, 1);
        assert_eq!(result.processed, 2);
        assert_eq!(
            store
                .get_track(&format!("spotify:{found_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(245.48)
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{absent_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            0,
            "public mode must not call account/status APIs"
        );
        assert_eq!(public.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn verified_unavailable_track_is_terminal_and_advances_public_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let id = "0Kvqc2zK0UKPTK6pAgyv1v";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Track::default()
            })
            .unwrap();
        let provider = Arc::new(NoProviderOperationTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let public = Arc::new(FixturePublicDurationTransport {
            durations: HashMap::new(),
            requests: Mutex::new(Vec::new()),
            blocker: None,
            fail_with: Some(
                "TRACK_NOT_FOUND: Spotify track is unavailable (NOT_AVAILABLE_IN_MARKET)".into(),
            ),
        });
        let service = RemotePlaylistService::new_with_public_spotify_duration(
            store.clone(),
            provider,
            public.clone(),
        );
        let result = service.repair_spotify_durations_public().await.unwrap();
        assert!(result.complete);
        assert_eq!(result.processed, 1);
        assert_eq!(result.changed, 0);
        let cursor = store
            .get_json_preference(&DurationBackfillScope::AnonymousPublic.cursor_key())
            .unwrap()
            .and_then(|value| value.as_str().map(str::to_owned));
        assert_eq!(cursor.as_deref(), Some(id));
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
        let second = service.repair_spotify_durations_public().await.unwrap();
        assert!(second.complete);
        assert_eq!(second.processed, 0);
        assert_eq!(
            public.requests.lock().unwrap().len(),
            1,
            "completed cursor should prevent refetching a known unavailable ID"
        );
    }

    #[tokio::test]
    async fn changed_embed_schema_stops_without_advancing_public_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let id = "0Kvqc2zK0UKPTK6pAgyv1v";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Track::default()
            })
            .unwrap();
        let provider = Arc::new(NoProviderOperationTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let public = Arc::new(FixturePublicDurationTransport {
            durations: HashMap::new(),
            requests: Mutex::new(Vec::new()),
            blocker: None,
            fail_with: Some(
                "PUBLIC_TRACK_INVALID: embed entity is missing or schema changed".into(),
            ),
        });
        let service = RemotePlaylistService::new_with_public_spotify_duration(
            store.clone(),
            provider,
            public,
        );
        assert!(service
            .repair_spotify_durations_public()
            .await
            .unwrap_err()
            .starts_with("PUBLIC_TRACK_INVALID:"));
        assert!(store
            .get_json_preference(&DurationBackfillScope::AnonymousPublic.cursor_key())
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
    }

    #[tokio::test]
    async fn explicit_public_duration_repair_stops_a_rate_limited_chunk_without_cursor_advance() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let ids = (0..10)
            .map(|index| format!("{index:022}"))
            .collect::<Vec<_>>();
        for id in &ids {
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id.clone(),
                    duration_seconds: None,
                    ..Track::default()
                })
                .unwrap();
        }
        let provider = Arc::new(NoProviderOperationTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let public = Arc::new(FixturePublicDurationTransport {
            durations: HashMap::new(),
            requests: Mutex::new(Vec::new()),
            blocker: None,
            fail_with: Some("RATE_LIMITED: public fixture cooldown".into()),
        });
        let service = RemotePlaylistService::new_with_public_spotify_duration(
            store.clone(),
            provider.clone(),
            public.clone(),
        );
        assert!(service
            .repair_spotify_durations_public()
            .await
            .unwrap_err()
            .starts_with("RATE_LIMITED:"));
        assert!((1..=EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY)
            .contains(&public.requests.lock().unwrap().len()));
        assert!(store
            .get_json_preference(&DurationBackfillScope::AnonymousPublic.cursor_key())
            .unwrap()
            .is_none());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let request_count = public.requests.lock().unwrap().len();
        let retry = service.repair_spotify_durations_public().await.unwrap_err();
        assert!(
            retry.starts_with("RATE_LIMITED:"),
            "retry should be suppressed during cooldown"
        );
        assert_eq!(
            public.requests.lock().unwrap().len(),
            request_count,
            "cooldown must not dispatch more HTTP reads"
        );
    }

    #[tokio::test]
    async fn dropping_public_repair_future_releases_its_generation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let id = "0Kvqc2zK0UKPTK6pAgyv1v";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Track::default()
            })
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let provider = Arc::new(NoProviderOperationTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let public = Arc::new(FixturePublicDurationTransport {
            durations: HashMap::from([(id.to_owned(), 245.48)]),
            requests: Mutex::new(Vec::new()),
            blocker: Some((entered.clone(), Arc::new(tokio::sync::Notify::new()))),
            fail_with: None,
        });
        let service =
            RemotePlaylistService::new_with_public_spotify_duration(store, provider, public);
        let task_service = service.clone();
        let task =
            tokio::spawn(async move { task_service.repair_spotify_durations_public().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        assert!(service.public_spotify_duration_repair_running());
        task.abort();
        let _ = task.await;
        assert!(
            !service.public_spotify_duration_repair_running(),
            "dropping the command must release its active generation"
        );
    }

    #[test]
    fn dropping_public_lease_without_runtime_preserves_deferred_account() {
        let (_dir, _store, service, _transport) = duration_fixture_service("account-deferred");
        let cancel = Arc::new(AtomicBool::new(false));
        let scope = DurationBackfillScope::AnonymousPublic;
        service
            .duration_backfill_cancel
            .lock()
            .unwrap()
            .insert(scope.clone(), cancel.clone());
        service
            .duration_backfill_deferred
            .lock()
            .unwrap()
            .insert("account-deferred".into());
        drop(DurationBackfillLease {
            service: service.clone(),
            scope,
            cancel,
        });
        assert!(!service.public_spotify_duration_repair_running());
        assert!(service
            .duration_backfill_deferred
            .lock()
            .unwrap()
            .contains("account-deferred"));
    }

    #[tokio::test]
    async fn account_backfill_requested_during_public_repair_resumes_after_cleanup() {
        let (_dir, _store, service, _transport) = duration_fixture_service("account-deferred");
        let cancel = Arc::new(AtomicBool::new(false));
        let scope = DurationBackfillScope::AnonymousPublic;
        {
            let _gate = service.duration_backfill_commit_gate.lock().unwrap();
            service
                .duration_backfill_cancel
                .lock()
                .unwrap()
                .insert(scope.clone(), cancel.clone());
        }
        let lease = DurationBackfillLease {
            service: service.clone(),
            scope,
            cancel,
        };
        service.schedule_existing_spotify_duration_backfill("account-deferred".into());
        assert!(service
            .duration_backfill_deferred
            .lock()
            .unwrap()
            .contains("account-deferred"));
        drop(lease);
        assert!(service
            .duration_backfill_cancel
            .lock()
            .unwrap()
            .contains_key(&DurationBackfillScope::Account("account-deferred".into())));
        wait_for_duration_backfill_idle(&service, "account-deferred").await;
        assert!(service
            .duration_backfill_scheduled
            .lock()
            .unwrap()
            .contains(&DurationBackfillScope::Account("account-deferred".into())));
    }

    #[tokio::test]
    async fn explicit_public_duration_repair_cancels_inflight_read_and_does_not_commit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let id = "0Kvqc2zK0UKPTK6pAgyv1v";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Track::default()
            })
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let provider = Arc::new(NoProviderOperationTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let public = Arc::new(FixturePublicDurationTransport {
            durations: HashMap::from([(id.to_owned(), 245.48)]),
            requests: Mutex::new(Vec::new()),
            blocker: Some((entered.clone(), release)),
            fail_with: None,
        });
        let service = RemotePlaylistService::new_with_public_spotify_duration(
            store.clone(),
            provider.clone(),
            public,
        );
        let task_service = service.clone();
        let task =
            tokio::spawn(async move { task_service.repair_spotify_durations_public().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        assert!(service.public_spotify_duration_repair_running());
        assert!(service.cancel_public_spotify_duration_repair().unwrap());
        assert!(task.await.unwrap().unwrap_err().contains("cancelled"));
        assert!(!service.public_spotify_duration_repair_running());
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    fn register_duration_backfill_job(
        service: &RemotePlaylistService,
        account: &str,
        cancel: Arc<AtomicBool>,
    ) {
        service
            .duration_backfill_scheduled
            .lock()
            .unwrap()
            .insert(DurationBackfillScope::Account(account.to_owned()));
        service
            .duration_backfill_cancel
            .lock()
            .unwrap()
            .insert(DurationBackfillScope::Account(account.to_owned()), cancel);
    }

    async fn wait_for_duration_backfill_idle(service: &RemotePlaylistService, account: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !service
                    .duration_backfill_cancel
                    .lock()
                    .unwrap()
                    .contains_key(&DurationBackfillScope::Account(account.to_owned()))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn account_switch_cancels_old_generation_and_allows_a_to_b_to_a_rescan() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let id = "1234567890123456789012";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Default::default()
            })
            .unwrap();
        *transport.search_delay.lock().unwrap() = std::time::Duration::from_millis(100);

        service.schedule_existing_spotify_duration_backfill("account-a".into());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !transport.searches.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        *transport.status_account.lock().unwrap() = "account-b".into();
        *transport.search_account.lock().unwrap() = "account-b".into();
        set_confirmed_duration_account(&service, "account-b");
        service.schedule_existing_spotify_duration_backfill("account-b".into());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if transport.searches.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        wait_for_duration_backfill_idle(&service, "account-b").await;
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(245.48)
        );
        {
            let scheduled = service.duration_backfill_scheduled.lock().unwrap();
            assert!(!scheduled.contains(&DurationBackfillScope::Account("account-a".into())));
            assert!(scheduled.contains(&DurationBackfillScope::Account("account-b".into())));
        }

        *transport.status_account.lock().unwrap() = "account-a".into();
        *transport.search_account.lock().unwrap() = "account-a".into();
        set_confirmed_duration_account(&service, "account-a");
        service.schedule_existing_spotify_duration_backfill("account-a".into());
        wait_for_duration_backfill_idle(&service, "account-a").await;
        let scheduled = service.duration_backfill_scheduled.lock().unwrap();
        assert!(scheduled.contains(&DurationBackfillScope::Account("account-a".into())));
        assert!(!scheduled.contains(&DurationBackfillScope::Account("account-b".into())));
    }

    #[tokio::test]
    async fn stale_account_trigger_cannot_cancel_confirmed_account_backfill() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let id = "1234567890123456789012";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Default::default()
            })
            .unwrap();
        *transport.status_account.lock().unwrap() = "account-b".into();
        *transport.search_account.lock().unwrap() = "account-b".into();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *transport.get_track_blocker.lock().unwrap() = Some((entered.clone(), release.clone()));
        set_confirmed_duration_account(&service, "account-b");
        service.schedule_existing_spotify_duration_backfill("account-b".into());
        tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        let active_b = service
            .duration_backfill_cancel
            .lock()
            .unwrap()
            .get(&DurationBackfillScope::Account("account-b".into()))
            .cloned()
            .expect("confirmed account B job should be active");
        assert!(!active_b.load(Ordering::SeqCst));

        // A delayed stale A caller must be rejected before it can cancel B or rewrite markers.
        service.schedule_existing_spotify_duration_backfill("account-a".into());
        assert!(!active_b.load(Ordering::SeqCst));
        assert!(Arc::ptr_eq(
            service
                .duration_backfill_cancel
                .lock()
                .unwrap()
                .get(&DurationBackfillScope::Account("account-b".into()))
                .unwrap(),
            &active_b,
        ));
        assert!(service
            .duration_backfill_scheduled
            .lock()
            .unwrap()
            .contains(&DurationBackfillScope::Account("account-b".into())));
        release.notify_one();
        wait_for_duration_backfill_idle(&service, "account-b").await;
        assert_eq!(transport.searches.lock().unwrap().len(), 1);
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(245.48)
        );
    }

    #[tokio::test]
    async fn background_duration_backfill_finds_existing_import_without_playlist_binding() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let id = "1234567890123456789012";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Default::default()
            })
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", cancel.clone());
        assert_eq!(
            service
                .backfill_existing_spotify_durations("account-a", &cancel)
                .await
                .unwrap()
                .0,
            1
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(245.48)
        );
        assert_eq!(*transport.searches.lock().unwrap(), vec![id.to_owned()]);
        assert!(service.list_bindings(None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_remote_track_read_is_account_guarded_and_never_persists() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let id = "1234567890123456789012";
        let track = service.get_remote_track("spotify", id).await.unwrap();
        assert_eq!(track.id, format!("spotify:{id}"));
        assert_eq!(
            track.source_url,
            format!("https://open.spotify.com/track/{id}")
        );
        assert_eq!(track.artist, "Fixture artist");
        assert_eq!(track.duration_seconds, Some(245.48));
        assert!(store.get_track(&track.id).unwrap().is_none());
        assert!(service.list_bindings(None).unwrap().is_empty());
        assert_eq!(*transport.searches.lock().unwrap(), vec![id]);

        *transport.search_account.lock().unwrap() = "account-b".into();
        assert!(service
            .get_remote_track("spotify", id)
            .await
            .unwrap_err()
            .contains("account"));
        assert!(service.get_remote_track("youtube", id).await.is_err());
        assert!(service.get_remote_track("spotify", "short").await.is_err());
        assert_eq!(transport.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn existing_duration_backfill_fails_fast_on_permanent_catalog_denial() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        for index in 0..300 {
            let id = format!("{index:022}");
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                })
                .unwrap();
        }
        *transport.get_track_error.lock().unwrap() =
            Some("API_DENIED: Spotify denied this catalog request".into());
        let cancel = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", cancel.clone());
        let result = service
            .backfill_existing_spotify_durations("account-a", &cancel)
            .await;
        assert!(result.unwrap_err().contains("API_DENIED:"));
        let read_count = transport.searches.lock().unwrap().len();
        assert!((1..=EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY).contains(&read_count));
        assert!(store
            .get_json_preference(&existing_duration_cursor_key("account-a"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn scheduler_does_not_retry_permanent_catalog_denial() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        for index in 0..300 {
            let id = format!("{index:022}");
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                })
                .unwrap();
        }
        *transport.get_track_error.lock().unwrap() =
            Some("API_DENIED: Spotify denied this catalog request".into());
        service.schedule_existing_spotify_duration_backfill("account-a".into());
        wait_for_duration_backfill_idle(&service, "account-a").await;
        tokio::time::sleep(std::time::Duration::from_millis(
            EXISTING_SPOTIFY_DURATION_BACKFILL_IDLE_MS + 100,
        ))
        .await;
        let read_count = transport.searches.lock().unwrap().len();
        assert!((1..=EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY).contains(&read_count));
        assert!(store
            .get_json_preference(&existing_duration_cursor_key("account-a"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn existing_duration_backfill_stops_rate_limited_chunk_without_cursor_advance() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        for index in 0..300 {
            let id = format!("{index:022}");
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                })
                .unwrap();
        }
        *transport.get_track_error.lock().unwrap() =
            Some("RATE_LIMITED: retry after 36 seconds".into());
        let cancel = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", cancel.clone());
        let result = service
            .backfill_existing_spotify_durations("account-a", &cancel)
            .await;
        assert!(result.unwrap_err().starts_with("RATE_LIMITED:"));
        let dispatched = transport.searches.lock().unwrap().len();
        assert!((1..=EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY).contains(&dispatched));
        assert!(store
            .get_json_preference(&existing_duration_cursor_key("account-a"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn scheduler_does_not_retry_rate_limited_lookup_before_hint_cooldown() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        for index in 0..300 {
            let id = format!("{index:022}");
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                })
                .unwrap();
        }
        *transport.get_track_error.lock().unwrap() =
            Some("RATE_LIMITED: retry after 36 seconds".into());
        service.schedule_existing_spotify_duration_backfill("account-a".into());
        wait_for_duration_backfill_idle(&service, "account-a").await;
        tokio::time::sleep(std::time::Duration::from_millis(
            EXISTING_SPOTIFY_DURATION_BACKFILL_IDLE_MS + 100,
        ))
        .await;
        let dispatched = transport.searches.lock().unwrap().len();
        assert!((1..=EXISTING_SPOTIFY_DURATION_BACKFILL_CONCURRENCY).contains(&dispatched));
        assert!(store
            .get_json_preference(&existing_duration_cursor_key("account-a"))
            .unwrap()
            .is_none());
        assert!(service
            .duration_backfill_scheduled
            .lock()
            .unwrap()
            .contains(&DurationBackfillScope::Account("account-a".into())));
    }

    #[tokio::test]
    async fn new_spotify_import_rearms_exhausted_scan_from_before_old_cursor() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let status = service.connection_status("spotify").await.unwrap();
        assert!(status.connected);
        wait_for_duration_backfill_idle(&service, "account-a").await;
        let cursor_key = existing_duration_cursor_key("account-a");
        store
            .set_json_preference(&cursor_key, &json!("ZZZZZZZZZZZZZZZZZZZZZZ"))
            .unwrap();
        let public_cursor_key = DurationBackfillScope::AnonymousPublic.cursor_key();
        store
            .set_json_preference(&public_cursor_key, &json!("ZZZZZZZZZZZZZZZZZZZZZZ"))
            .unwrap();
        assert!(service
            .duration_backfill_scheduled
            .lock()
            .unwrap()
            .contains(&DurationBackfillScope::Account("account-a".into())));

        let track_id = "1234567890123456789012";
        let url = format!("https://open.spotify.com/track/{track_id}");
        let track = Track {
            id: format!("spotify:{track_id}"),
            provider: "spotify".into(),
            provider_id: track_id.into(),
            source_url: url.clone(),
            title: "New import".into(),
            artist: "Fixture artist".into(),
            duration_seconds: None,
            ..Default::default()
        };
        // This is an existing missing-duration listing below the prior cursor, not a new row.
        store.save_track(&track).unwrap();
        let entry = RemoteEntry {
            entry_id: "entry-new".into(),
            track_id: track_id.into(),
            title: "New import".into(),
            artist: "Fixture artist".into(),
            album: None,
            duration_seconds: None,
            url: url.clone(),
            artwork_url: None,
            available: true,
            plays: None,
            views: None,
        };
        let playlist_id = "2234567890123456789012";
        let playlist_url = format!("https://open.spotify.com/playlist/{playlist_id}");
        let chart = Chart {
            id: String::new(),
            provider: "spotify".into(),
            title: "New import".into(),
            source_url: playlist_url.clone(),
            fetched_at: "2026-10-15T00:00:00Z".into(),
            tracks: vec![track.clone()],
            stale: false,
            warning: None,
        };
        let browser_snapshot = BrowserSnapshot {
            schema_version: 1,
            provider: "spotify".into(),
            kind: "playlist".into(),
            title: "New import".into(),
            raw_page_title: None,
            source_url: playlist_url.clone(),
            captured_at: "2026-10-15T00:00:00Z".into(),
            complete: true,
            expected_total: Some(1),
            warnings: vec![],
            entries: vec![BrowserEntry {
                provider_id: Some(track_id.into()),
                source_url: Some(url.clone()),
                artist: "Fixture artist".into(),
                title: "New import".into(),
                version: None,
                position: 1,
                rank: None,
                duration_seconds: None,
                bpm: None,
                key: None,
                album: None,
                label: None,
                genre: None,
                release_date: None,
                artwork_url: None,
                isrc: None,
                catalog_number: None,
            }],
        };
        let fetched = FetchedPlaylist {
            write_capabilities: None,
            title: "New import".into(),
            url: playlist_url,
            owner_id: None,
            owner_name: None,
            owner_url: None,
            visibility: None,
            can_edit: false,
            tracks: vec![track],
            snapshot: RemoteSnapshot {
                entries: vec![entry.clone()],
                content_hash: hash_json(&[entry.clone()]).unwrap(),
                revision: "rev-new".into(),
                complete: true,
            },
            account_id: Some("account-a".into()),
            browser_snapshot,
            chart,
        };
        service
            .persist_fetched("spotify", &playlist_id, None, false, fetched)
            .unwrap();
        assert_eq!(
            store.get_json_preference(&public_cursor_key).unwrap(),
            Some(Value::Null)
        );
        assert!(
            !service.public_spotify_duration_repair_running(),
            "import must not auto-start anonymous repair"
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let track = store.get_track(&format!("spotify:{track_id}")).unwrap();
                if track
                    .as_ref()
                    .and_then(|value| value.duration_seconds)
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(transport
            .searches
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == track_id));
        assert_eq!(
            store
                .get_track(&format!("spotify:{track_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(245.48)
        );
    }

    #[tokio::test]
    async fn import_duration_search_stops_after_rate_limited_current_chunk() {
        let (_dir, _store, service, transport) = duration_fixture_service("account-a");
        *transport.search_error.lock().unwrap() =
            Some("RATE_LIMITED: retry after 36 seconds".into());
        let mut tracks = (0..30)
            .map(|index| {
                let id = format!("{index:022}");
                Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        service
            .backfill_spotify_durations(&mut tracks, "account-a", &Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let count = transport.searches.lock().unwrap().len();
        assert!((1..=SPOTIFY_DURATION_BACKFILL_CONCURRENCY).contains(&count));
    }

    #[tokio::test]
    async fn existing_duration_backfill_resumes_after_bounded_batch_without_losing_rows() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        for index in 0..301 {
            let id = format!("{index:022}");
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                })
                .unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", cancel.clone());
        let first = service
            .backfill_existing_spotify_durations("account-a", &cancel)
            .await
            .unwrap();
        assert_eq!(first, (300, false, 300));
        let second = service
            .backfill_existing_spotify_durations("account-a", &cancel)
            .await
            .unwrap();
        assert_eq!(second, (1, true, 1));
        assert_eq!(transport.searches.lock().unwrap().len(), 301);
    }

    #[tokio::test]
    async fn exhausted_backfill_is_not_restarted_by_connection_status_polling() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let id = "1234567890123456789012";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Default::default()
            })
            .unwrap();
        service.schedule_existing_spotify_duration_backfill("account-a".into());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !service
                    .duration_backfill_cancel
                    .lock()
                    .unwrap()
                    .contains_key(&DurationBackfillScope::Account("account-a".into()))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        service.schedule_existing_spotify_duration_backfill("account-a".into());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(transport.searches.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deadline_does_not_advance_cursor_past_unfinished_search_chunk() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        for index in 0..10 {
            let id = format!("{index:022}");
            store
                .save_track(&Track {
                    id: format!("spotify:{id}"),
                    provider: "spotify".into(),
                    provider_id: id,
                    duration_seconds: None,
                    ..Default::default()
                })
                .unwrap();
        }
        *transport.delayed_search_ids.lock().unwrap() =
            (5..10).map(|index| format!("{index:022}")).collect();
        *transport.delayed_search_delay.lock().unwrap() = std::time::Duration::from_secs(2);
        let cancel = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", cancel.clone());
        // The first five finish; the second five exceed the overall deadline.
        let result = service
            .backfill_existing_spotify_durations_with_budget(
                "account-a",
                &cancel,
                std::time::Duration::from_millis(500),
            )
            .await;
        // The first complete five-request batch may commit; the timed-out second batch
        // must remain after the cursor so a later invocation can resume it.
        assert_eq!(result.unwrap(), (5, false, 5));
        let cursor = store
            .get_json_preference(&existing_duration_cursor_key("account-a"))
            .unwrap();
        assert_eq!(cursor, Some(json!(format!("{:022}", 4))));
        let remaining = store
            .missing_spotify_duration_ids(cursor.as_ref().and_then(Value::as_str), 10)
            .unwrap();
        assert_eq!(
            remaining,
            (5..10)
                .map(|index| format!("{index:022}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(transport.searches.lock().unwrap().len(), 10);
    }

    #[tokio::test]
    async fn existing_duration_backfill_stops_on_account_change_or_cancellation() {
        let (_dir, store, service, transport) = duration_fixture_service("account-a");
        let id = "1234567890123456789012";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Default::default()
            })
            .unwrap();
        // The service status remains account A, but a provider search returning account B is rejected.
        *transport.search_account.lock().unwrap() = "account-b".into();
        let not_canceled = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", not_canceled.clone());
        assert!(service
            .backfill_existing_spotify_durations("account-a", &not_canceled)
            .await
            .is_err());
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
        let canceled = Arc::new(AtomicBool::new(true));
        register_duration_backfill_job(&service, "account-a", canceled.clone());
        assert_eq!(
            service
                .backfill_existing_spotify_durations("account-a", &canceled)
                .await
                .unwrap()
                .0,
            0
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
    }

    #[tokio::test]
    async fn existing_duration_backfill_cancellation_drops_an_in_flight_provider_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let id = "1234567890123456789012";
        store
            .save_track(&Track {
                id: format!("spotify:{id}"),
                provider: "spotify".into(),
                provider_id: id.into(),
                duration_seconds: None,
                ..Default::default()
            })
            .unwrap();
        let transport = Arc::new(DurationFixtureTransport {
            status_account: Mutex::new("account-a".into()),
            search_account: Mutex::new("account-a".into()),
            searches: Mutex::new(Vec::new()),
            search_delay: Mutex::new(std::time::Duration::from_millis(200)),
            delayed_search_ids: Mutex::new(HashSet::new()),
            delayed_search_delay: Mutex::new(std::time::Duration::from_millis(80)),
            get_track_error: Mutex::new(None),
            search_error: Mutex::new(None),
            get_track_blocker: Mutex::new(None),
        });
        let service = RemotePlaylistService::new(store.clone(), transport);
        let cancel = Arc::new(AtomicBool::new(false));
        register_duration_backfill_job(&service, "account-a", cancel.clone());
        let task_service = service.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            task_service
                .backfill_existing_spotify_durations("account-a", &task_cancel)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cancel.store(true, Ordering::SeqCst);
        assert_eq!(task.await.unwrap().unwrap().0, 0);
        assert_eq!(
            store
                .get_track(&format!("spotify:{id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
    }

    #[test]
    fn youtube_write_plan_requires_unique_verified_occurrences_and_preserves_duplicates() {
        let entries = vec![
            RemoteEntry {
                entry_id: "occ-a".into(),
                track_id: "abcdefghijk".into(),
                title: "A".into(),
                artist: "X".into(),
                album: None,
                duration_seconds: None,
                url: String::new(),
                artwork_url: None,
                available: true,
                plays: None,
                views: None,
            },
            RemoteEntry {
                entry_id: "occ-b".into(),
                track_id: "abcdefghijk".into(),
                title: "A".into(),
                artist: "X".into(),
                album: None,
                duration_seconds: None,
                url: String::new(),
                artwork_url: None,
                available: true,
                plays: None,
                views: None,
            },
        ];
        let desired = vec![
            "youtube:video:abcdefghijk".into(),
            "youtube:video:abcdefghijk".into(),
        ];
        let ids = match_youtube_entry_ids(&entries, &desired).unwrap();
        assert_eq!(ids, vec!["occ-a", "occ-b"]);
        assert_eq!(
            youtube_worker_snapshot(&entries),
            vec![
                json!({"entryId":"occ-a","trackId":"abcdefghijk"}),
                json!({"entryId":"occ-b","trackId":"abcdefghijk"}),
            ]
        );
        assert!(match_youtube_entry_ids(&entries, &desired[..1]).is_err());
        let mut duplicate_occurrence = entries.clone();
        duplicate_occurrence[1].entry_id = "occ-a".into();
        assert!(match_youtube_entry_ids(&duplicate_occurrence, &desired).is_err());
        let mut unavailable = entries;
        unavailable[0].available = false;
        assert!(match_youtube_entry_ids(&unavailable, &desired).is_err());
    }

    #[test]
    fn catalog_query_hash_normalizes_case_and_whitespace() {
        assert_eq!(
            catalog_query_hash("  Daft   Punk "),
            catalog_query_hash("daft punk")
        );
        assert_ne!(catalog_query_hash("daft punk"), catalog_query_hash("daft"));
    }

    #[test]
    fn provider_cursor_requires_bounded_numeric_offset() {
        assert!(validate_numeric_cursor(Some("0"), 10).is_ok());
        assert!(validate_numeric_cursor(Some("10"), 10).is_ok());
        assert!(validate_numeric_cursor(Some("../1"), 10).is_err());
        assert!(validate_numeric_cursor(Some("11"), 10).is_err());
    }

    #[test]
    fn spotify_profile_and_album_fixtures_retain_artwork_and_metadata() {
        let artist_id = "AAAAAAAAAAAAAAAAAAAAAA";
        let artist = parse_spotify_artist(&json!({"id":artist_id,"kind":"artist","title":"Fixture Artist","url":format!("https://open.spotify.com/artist/{artist_id}"),"artworkUrl":"https://i.scdn.co/image/artist","data":{"genres":["house"],"popularity":82,"followers":500}})).unwrap();
        assert_eq!(
            artist.artwork_url.as_deref(),
            Some("https://i.scdn.co/image/artist")
        );
        assert_eq!(artist.genres, vec!["house"]);
        let album_id = "BBBBBBBBBBBBBBBBBBBBBB";
        let album = parse_spotify_album(&json!({"id":album_id,"kind":"album","title":"Fixture Album","url":format!("https://open.spotify.com/album/{album_id}"),"artworkUrl":"https://i.scdn.co/image/cover","data":{"artists":["Fixture Artist"],"releaseDate":"2024-01-01","albumType":"album","totalTracks":12}})).unwrap();
        assert_eq!(
            album.artwork_url.as_deref(),
            Some("https://i.scdn.co/image/cover")
        );
        assert_eq!(album.total_tracks, Some(12));
    }

    #[test]
    fn refresh_keeps_existing_duration_when_provider_omits_it() {
        let existing = Track {
            provider: "spotify".into(),
            provider_id: "known-track".into(),
            duration_seconds: Some(193.375),
            ..Track::default()
        };
        let mut incoming = vec![Track {
            provider: "spotify".into(),
            provider_id: "known-track".into(),
            duration_seconds: None,
            ..Track::default()
        }];

        preserve_known_durations(&mut incoming, &[existing]);
        assert_eq!(incoming[0].duration_seconds, Some(193.375));
    }

    #[test]
    fn spotify_zero_duration_is_unknown_and_replaced_by_known_catalog_metadata() {
        let row = json!({"entryId":"uid-1","trackId":"AAAAAAAAAAAAAAAAAAAAAA","title":"Track","artist":"Artist","durationSeconds":0,"url":"https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA","available":true});
        let parsed = parse_remote_entry(&row, "spotify", "0").unwrap();
        assert_eq!(parsed.duration_seconds, None);
        let mut incoming = [Track {
            provider: "spotify".into(),
            provider_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            duration_seconds: Some(0.0),
            ..Default::default()
        }];
        let existing = Track {
            provider: "spotify".into(),
            provider_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            duration_seconds: Some(193.375),
            ..Default::default()
        };
        preserve_known_durations(&mut incoming, &[existing]);
        assert_eq!(incoming[0].duration_seconds, Some(193.375));
        let mut missing = incoming[0].clone();
        missing.duration_seconds = Some(0.0);
        assert!(is_duration_backfill_eligible(&missing));
        let long_known = Track {
            duration_seconds: Some(90_000.0),
            ..missing.clone()
        };
        assert!(!is_duration_backfill_eligible(&long_known));
        let mut refresh = [Track {
            duration_seconds: None,
            ..incoming[0].clone()
        }];
        let previous = Track {
            duration_seconds: Some(90_000.0),
            ..incoming[0].clone()
        };
        preserve_known_durations(&mut refresh, &[previous]);
        assert_eq!(refresh[0].duration_seconds, Some(90_000.0));
    }

    #[test]
    fn duration_backfill_application_preserves_protected_rows_sharing_an_eligible_id() {
        let id = "AAAAAAAAAAAAAAAAAAAAAA";
        let base = Track {
            provider: "spotify".into(),
            provider_id: id.into(),
            title: "Keep title".into(),
            ..Default::default()
        };
        let mut manual = base.clone();
        manual.provenance = json!({"manualFields":{"durationSeconds":true},"keep":"manual"});
        let mut chapter = base.clone();
        chapter.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "spotify:parent".into(),
            chapter_id: "chapter-1".into(),
            source_url: "https://example.com/source".into(),
            start_time: 10.0,
            end_time: 25.0,
            parent_title: "Parent".into(),
            parent_duration_seconds: Some(100.0),
        });
        let mut provenance_chapters = base.clone();
        provenance_chapters.provenance = json!({"chapters":[{"startTime":10.0,"endTime":25.0}]});
        let mut known = base.clone();
        known.duration_seconds = Some(99.5);
        let protected = [manual, chapter, provenance_chapters, known];
        let mut tracks = vec![base.clone(), base];
        tracks.extend(protected.iter().cloned());
        let before = protected
            .iter()
            .map(|track| serde_json::to_value(track).unwrap())
            .collect::<Vec<_>>();
        apply_verified_spotify_durations(&mut tracks, &HashMap::from([(id.to_owned(), 245.48)]));
        assert_eq!(tracks[0].duration_seconds, Some(245.48));
        assert_eq!(tracks[1].duration_seconds, Some(245.48));
        assert_eq!(tracks[0].title, "Keep title");
        assert_eq!(
            tracks[0].provenance["durationBackfill"]["verifiedProviderId"],
            id
        );
        for (track, before) in tracks[2..].iter().zip(before) {
            assert_eq!(serde_json::to_value(track).unwrap(), before);
        }
    }
    #[test]
    fn spotify_duration_backfill_requires_exact_identity_and_missing_unedited_duration() {
        let requested = "AAAAAAAAAAAAAAAAAAAAAA";
        let row = json!({"id":requested,"kind":"track","data":{"durationSeconds":201.25}});
        let other =
            json!({"id":"BBBBBBBBBBBBBBBBBBBBBB","kind":"track","data":{"durationSeconds":201.25}});
        assert_eq!(verified_spotify_duration(&row, requested), Some(201.25));
        assert_eq!(verified_spotify_duration(&other, requested), None);

        let candidate = Track {
            provider: "spotify".into(),
            provider_id: requested.into(),
            ..Track::default()
        };
        assert!(is_duration_backfill_eligible(&candidate));
        let mut manual = candidate.clone();
        manual.provenance = json!({"manualFields":{"durationSeconds":true}});
        assert!(!is_duration_backfill_eligible(&manual));
        let mut known = candidate.clone();
        known.duration_seconds = Some(210.0);
        assert!(!is_duration_backfill_eligible(&known));
        let mut chapter = candidate;
        chapter.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "spotify:parent".into(),
            chapter_id: "chapter-1".into(),
            source_url: "https://example.com/source".into(),
            start_time: 10.0,
            end_time: 20.0,
            parent_title: "Parent".into(),
            parent_duration_seconds: Some(100.0),
        });
        assert!(!is_duration_backfill_eligible(&chapter));
    }

    fn remote_entry(entry_id: &str, track_id: &str) -> RemoteEntry {
        RemoteEntry {
            entry_id: entry_id.into(),
            track_id: track_id.into(),
            title: track_id.into(),
            artist: "Artist".into(),
            album: None,
            duration_seconds: None,
            url: format!("https://open.spotify.com/track/{track_id}"),
            artwork_url: None,
            available: true,
            plays: None,
            views: None,
        }
    }

    #[test]
    fn inserted_middle_track_reorders_existing_entries_exactly() {
        let current = vec!["a".into(), "b".into(), "new".into()];
        let desired = vec!["a".into(), "new".into(), "b".into()];
        let moves = reorder_plan(&current, &desired).unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].entry_ids, vec!["b".to_owned()]);
        assert_eq!(moves[0].before_entry_id, None);
    }

    #[test]
    fn duplicate_track_occurrences_keep_distinct_entry_uids_and_order() {
        let entries = vec![
            remote_entry("uid-a", "AAAAAAAAAAAAAAAAAAAAAA"),
            remote_entry("uid-b", "AAAAAAAAAAAAAAAAAAAAAA"),
            remote_entry("uid-c", "BBBBBBBBBBBBBBBBBBBBBB"),
        ];
        let desired = vec![
            "spotify:track:AAAAAAAAAAAAAAAAAAAAAA".into(),
            "spotify:track:BBBBBBBBBBBBBBBBBBBBBB".into(),
            "spotify:track:AAAAAAAAAAAAAAAAAAAAAA".into(),
        ];
        let ids = match_entry_ids(&entries, &desired).unwrap();
        assert_eq!(
            ids,
            vec!["uid-a".to_owned(), "uid-c".to_owned(), "uid-b".to_owned()]
        );
        let moves = reorder_plan(
            &entries
                .iter()
                .map(|entry| entry.entry_id.clone())
                .collect::<Vec<_>>(),
            &ids,
        )
        .unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].entry_ids, vec!["uid-b".to_owned()]);
        assert_eq!(moves[0].before_entry_id, None);
    }

    #[test]
    fn typed_write_capabilities_are_strict_and_action_specific() {
        assert_eq!(parse_write_capabilities(&json!({})).unwrap(), None);
        let add_only = RemotePlaylistWriteCapabilities {
            add: true,
            remove: false,
            reorder: false,
        };
        assert_eq!(
            parse_write_capabilities(
                &json!({"writeCapabilities":{"add":true,"remove":false,"reorder":false}})
            )
            .unwrap(),
            Some(add_only)
        );
        for bad in [
            Value::Null,
            json!(true),
            json!({}),
            json!({"add":true,"remove":"false","reorder":false}),
            json!({"add":true,"remove":false,"reorder":false,"extra":true}),
        ] {
            assert!(parse_write_capabilities(&json!({"writeCapabilities":bad})).is_err());
        }
        let additions: RemotePlaylistSyncChanges = serde_json::from_value(json!({"add":[{"trackUri":"youtube:video:abcdefghijk"}],"remove":[],"move":[],"unresolved":[]})).unwrap();
        assert!(write_changes_allowed(false, Some(add_only), &additions));
        assert!(write_changes_allowed(true, None, &additions));
        assert!(!write_changes_allowed(false, None, &additions));
        assert!(!write_changes_allowed(
            true,
            Some(RemotePlaylistWriteCapabilities::default()),
            &additions
        ));
        let mut remove = additions.clone();
        remove.remove.push(RemoteRemoveChange {
            entry_ids: vec!["verified-occurrence".into()],
        });
        assert!(!write_changes_allowed(true, Some(add_only), &remove));
        let mut reorder = additions;
        reorder.move_entries.push(RemoteMoveChange {
            entry_ids: vec!["verified-occurrence".into()],
            before_entry_id: None,
        });
        assert!(!write_changes_allowed(true, Some(add_only), &reorder));
        assert!(write_changes_allowed(true, None, &reorder));
    }

    #[test]
    fn youtube_readback_integrity_does_not_grant_or_require_future_write_permission() {
        let valid = remote_entry("verified-occurrence", "abcdefghijk");
        assert!(ensure_playlist_capture("youtube", true, &[valid.clone()]).is_ok());
        assert!(ensure_playlist_capture("youtube", false, &[valid.clone()]).is_err());
        let mut unavailable = valid.clone();
        unavailable.available = false;
        assert!(ensure_playlist_capture("youtube", true, &[unavailable]).is_err());
        let mut synthetic = valid;
        synthetic.entry_id = "ytocc-synthetic".into();
        assert!(ensure_playlist_capture("youtube", true, &[synthetic]).is_err());
        assert!(
            ensure_writable("youtube", false, true, &[]).is_err(),
            "the legacy broad guard remains strict"
        );
    }

    #[test]
    fn read_only_playlists_are_rejected_before_mutation() {
        assert_eq!(
            ensure_writable("spotify", false, true, &[]).unwrap_err(),
            "Remote playlist is read-only"
        );
        assert!(ensure_writable("spotify", true, false, &[]).is_err());
        let mut unavailable = remote_entry("uid", "AAAAAAAAAAAAAAAAAAAAAA");
        unavailable.available = false;
        assert!(ensure_writable("spotify", true, true, &[unavailable]).is_err());
    }

    #[test]
    fn paginated_capture_uses_terminal_page_completeness() {
        let mut terminal_page_complete = false;
        observe_page_completion(&mut terminal_page_complete, false, true);
        assert!(!terminal_page_complete);
        observe_page_completion(&mut terminal_page_complete, true, false);
        assert!(capture_is_complete(terminal_page_complete, Some(150), 150));
    }

    #[test]
    fn playlist_browse_checks_global_total_against_page_context() {
        let terminal_page = |continuation, provider_complete, has_next, total, page_rows| {
            RemotePlaylistService::remote_playlist_page_is_complete(
                continuation,
                provider_complete,
                has_next,
                total,
                page_rows,
            )
        };
        // A terminal first page still has to contain the entire known playlist.
        assert!(!terminal_page(false, true, false, Some(240), 10));
        assert!(terminal_page(false, true, false, Some(10), 10));
        // A terminal continuation may contain only the tail; the frontend checks
        // the cumulative loaded rows against the preserved global total.
        assert!(terminal_page(true, true, false, Some(240), 10));
        assert!(terminal_page(true, true, false, Some(10), 10));
        assert!(!terminal_page(true, true, false, Some(9), 10));
        // Explicit incompleteness and a remaining cursor always win.
        assert!(!terminal_page(true, false, false, Some(240), 10));
        assert!(!terminal_page(true, true, true, Some(240), 10));
    }

    #[test]
    fn incomplete_terminal_page_or_count_mismatch_stays_incomplete() {
        let mut terminal_page_complete = false;
        observe_page_completion(&mut terminal_page_complete, false, true);
        observe_page_completion(&mut terminal_page_complete, false, false);
        assert!(!capture_is_complete(terminal_page_complete, Some(150), 150));
        assert!(!capture_is_complete(true, Some(150), 149));
    }

    #[test]
    fn valid_spotify_track_title_parses_without_fallback_error() {
        let row = json!({
            "entryId": "uid-1",
            "trackId": "AAAAAAAAAAAAAAAAAAAAAA",
            "title": "Valid title",
            "artist": "Artist",
            "url": "https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA",
            "available": true
        });
        let parsed = parse_remote_entry(&row, "spotify", "0").unwrap();
        assert_eq!(parsed.title, "Valid title");
        assert_eq!(parsed.entry_id, "uid-1");
    }

    #[test]
    fn spotify_catalog_metrics_survive_normalization_for_track_provenance() {
        let data = json!({
            "trackId": "AAAAAAAAAAAAAAAAAAAAAA",
            "title": "Catalog track",
            "views": 1200,
            "plays": 3400,
            "viewCount": 1200,
            "playCount": 3400
        });
        let normalized = normalize_catalog_data(Some(&data), "track", "spotify").unwrap();
        assert_eq!(normalized["views"], 1200);
        assert_eq!(normalized["plays"], 3400);
        assert_eq!(normalized["viewCount"], 1200);
        assert_eq!(normalized["playCount"], 3400);
    }

    #[test]
    fn remote_playlist_reported_counts_round_trip_zero_and_omit_missing_or_invalid_values() {
        let row = json!({
            "entryId":"count-entry",
            "trackId":"AAAAAAAAAAAAAAAAAAAAAA",
            "title":"Counted track",
            "artist":"Artist",
            "url":"https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA",
            "playCount":0,
            "view_count":"1,234"
        });
        let entry = parse_remote_entry(&row, "spotify", "0").unwrap();
        assert_eq!(entry.plays, Some(0));
        assert_eq!(entry.views, Some(1234));
        let track = entry_to_track("spotify", &entry, 0);
        assert_eq!(track.provenance["plays"], 0);
        assert_eq!(track.provenance["views"], 1234);

        let mut invalid = row.clone();
        invalid["plays"] = json!("1,,2");
        invalid["playCount"] = json!(MAX_REPORTED_REMOTE_COUNT + 1);
        invalid["play_count"] = json!(-1);
        invalid["views"] = json!("1,23");
        invalid["viewCount"] = json!("-3");
        invalid["view_count"] = json!(1.5);
        let invalid_entry = parse_remote_entry(&invalid, "spotify", "0").unwrap();
        assert_eq!(invalid_entry.plays, None);
        assert_eq!(invalid_entry.views, None);
        let invalid_track = entry_to_track("spotify", &invalid_entry, 0);
        assert!(invalid_track.provenance.get("plays").is_none());
        assert!(invalid_track.provenance.get("views").is_none());

        let mut fallback_alias = row.clone();
        fallback_alias["plays"] = json!("1,,2");
        fallback_alias["playCount"] = json!(12);
        assert_eq!(
            parse_remote_entry(&fallback_alias, "spotify", "0")
                .unwrap()
                .plays,
            Some(12)
        );

        let missing = json!({
            "entryId":"missing-count-entry",
            "trackId":"AAAAAAAAAAAAAAAAAAAAAA",
            "title":"Uncounted track",
            "artist":"Artist",
            "url":"https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA"
        });
        let missing_entry = parse_remote_entry(&missing, "spotify", "1").unwrap();
        assert_eq!(missing_entry.plays, None);
        assert_eq!(missing_entry.views, None);
    }

    #[test]
    fn remote_playlist_stats_only_refresh_keeps_revision_and_sync_hash_stable() {
        let original = remote_entry("count-entry", "AAAAAAAAAAAAAAAAAAAAAA");
        let original_content_hash =
            remote_entries_content_hash(std::slice::from_ref(&original)).unwrap();
        let original_track_hash =
            remote_tracks_revision_hash(&[entry_to_track("spotify", &original, 0)]).unwrap();

        let mut refreshed = original.clone();
        refreshed.plays = Some(0);
        refreshed.views = Some(45);
        assert_eq!(
            remote_entries_content_hash(std::slice::from_ref(&refreshed)).unwrap(),
            original_content_hash
        );
        assert_eq!(
            remote_tracks_revision_hash(&[entry_to_track("spotify", &refreshed, 0)]).unwrap(),
            original_track_hash
        );

        // Old persisted snapshots omit the optional fields and continue to deserialize.
        let legacy = serde_json::to_value(&original).unwrap();
        assert!(legacy.get("plays").is_none());
        assert!(legacy.get("views").is_none());
        let decoded: RemoteEntry = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.plays, None);
        assert_eq!(decoded.views, None);
    }

    #[test]
    fn youtube_null_entry_ids_get_stable_distinct_duplicate_occurrences() {
        let row = json!({
            "entryId": null,
            "trackId": "youtubeTrack_1",
            "title": "Repeated track",
            "artist": "Artist",
            "url": "https://music.youtube.com/watch?v=youtubeTrack_1",
            "available": true
        });
        let first = parse_remote_entry(&row, "youtube", "7").unwrap();
        let second = parse_remote_entry(&row, "youtube", "8").unwrap();
        let same_position = parse_remote_entry(&row, "youtube", "7").unwrap();
        assert_ne!(first.entry_id, second.entry_id);
        assert_eq!(first.entry_id, same_position.entry_id);
        assert_eq!(first.track_id, second.track_id);
        assert_eq!(first.title, "Repeated track");
    }

    #[test]
    fn playlist_count_distinguishes_known_zero_from_unknown() {
        let base = json!({
            "id": "AAAAAAAAAAAAAAAAAAAAAA",
            "provider": "spotify",
            "title": "Empty",
            "url": "https://open.spotify.com/playlist/AAAAAAAAAAAAAAAAAAAAAA"
        });
        let mut zero = base.clone();
        zero["total"] = json!(0);
        assert_eq!(
            parse_playlist_item(&zero, "spotify").unwrap().track_count,
            Some(0)
        );
        assert_eq!(
            parse_playlist_item(&base, "spotify").unwrap().track_count,
            None
        );
    }

    #[test]
    fn spotify_collection_references_are_not_playlist_ids() {
        for reference in [
            "tracks",
            "spotify:collection:tracks",
            "https://open.spotify.com/collection/tracks",
            "https://open.spotify.com/playlist/tracks",
        ] {
            assert!(normalize_playlist_reference("spotify", reference)
                .unwrap_err()
                .contains("not available as playlists"));
        }
        assert_eq!(
            normalize_playlist_reference("spotify", "37i9dQZF1DXcBWIGoYBM5M").unwrap(),
            "37i9dQZF1DXcBWIGoYBM5M"
        );
    }

    #[test]
    fn owner_urls_are_canonicalized_only_for_matching_official_profiles() {
        assert_eq!(
            normalize_owner_url(
                "spotify",
                Some("owner_123"),
                Some("https://open.spotify.com/user/owner_123")
            ),
            Some("https://open.spotify.com/user/owner_123".into())
        );
        assert_eq!(
            normalize_owner_url(
                "spotify",
                Some("owner_123"),
                Some("https://attacker.example/user/owner_123")
            ),
            None
        );
        assert_eq!(
            normalize_owner_url(
                "youtube",
                Some("UCchannel_123"),
                Some("https://www.youtube.com/channel/UCchannel_123")
            ),
            Some("https://www.youtube.com/channel/UCchannel_123".into())
        );
        assert_eq!(
            normalize_owner_url("youtube", Some("@creator_name"), None),
            Some("https://www.youtube.com/@creator_name".into())
        );
        assert_eq!(
            normalize_owner_url(
                "youtube",
                Some("UCchannel_123"),
                Some("https://youtube.com/@other")
            ),
            None
        );
    }

    #[test]
    fn old_remote_binding_preferences_deserialize_without_creator_metadata() {
        let old_binding = json!({
            "bindingId": "binding",
            "provider": "spotify",
            "playlistId": "AAAAAAAAAAAAAAAAAAAAAA",
            "title": "Playlist",
            "url": "https://open.spotify.com/playlist/AAAAAAAAAAAAAAAAAAAAAA",
            "accountId": "account",
            "collectionId": "collection",
            "collectionName": "Playlist copy",
            "trackCount": 1,
            "revision": "revision",
            "complete": true,
            "refreshedAt": "2026-10-14T00:00:00Z"
        });
        let parsed: RemotePlaylistBindingStatus = serde_json::from_value(old_binding).unwrap();
        assert_eq!(parsed.owner_id, None);
        assert_eq!(parsed.owner_name, None);
        assert_eq!(parsed.owner_url, None);
        assert_eq!(parsed.visibility, None);
    }

    #[test]
    fn repeated_import_reuses_binding_for_same_provider_account_and_playlist() {
        let binding = RemotePlaylistBindingStatus {
            write_capabilities: None,
            binding_id: "binding-1".into(),
            provider: "spotify".into(),
            playlist_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            title: "Playlist".into(),
            url: "https://open.spotify.com/playlist/AAAAAAAAAAAAAAAAAAAAAA".into(),
            account_id: "account-1".into(),
            owner_id: None,
            owner_name: None,
            owner_url: None,
            visibility: None,
            collection_id: "working-collection-1".into(),
            source_collection_id: Some("snapshot-collection-1".into()),
            collection_name: "Playlist".into(),
            track_count: 0,
            revision: "revision-1".into(),
            complete: true,
            can_edit: true,
            refreshed_at: "2026-10-14T00:00:00Z".into(),
        };
        let bindings = vec![binding];

        let repeated_import =
            find_bound_playlist(&bindings, "spotify", "AAAAAAAAAAAAAAAAAAAAAA", "account-1")
                .unwrap();
        assert_eq!(repeated_import.binding_id, "binding-1");
        assert_eq!(repeated_import.collection_id, "working-collection-1");

        assert!(
            find_bound_playlist(&bindings, "spotify", "AAAAAAAAAAAAAAAAAAAAAA", "account-2")
                .is_none()
        );
        assert!(
            find_bound_playlist(&bindings, "youtube", "AAAAAAAAAAAAAAAAAAAAAA", "account-1")
                .is_none()
        );
    }

    #[test]
    fn refresh_updates_unavailable_baseline_without_overwriting_user_edits() {
        let unavailable = RemoteEntry {
            entry_id: "unavailable-1".into(),
            track_id: "unknown-track".into(),
            title: "Old unavailable title".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            duration_seconds: Some(180.0),
            url: String::new(),
            artwork_url: None,
            available: false,
            plays: None,
            views: None,
        };
        let baseline = RemoteSnapshot {
            entries: vec![unavailable.clone()],
            revision: "old-revision".into(),
            content_hash: "old-hash".into(),
            complete: true,
        };
        let fallback_id = unavailable_fallback_id(&unavailable);
        let untouched_copy = Track {
            id: format!("import:{fallback_id}"),
            provider: "import".into(),
            provider_id: fallback_id,
            title: unavailable.title.clone(),
            artist: unavailable.artist.clone(),
            album: unavailable.album.clone(),
            duration_seconds: unavailable.duration_seconds,
            artwork_url: unavailable.artwork_url.clone(),
            ..Track::default()
        };
        assert!(should_replace_working_copy(
            std::slice::from_ref(&untouched_copy),
            &baseline,
            "spotify",
            false
        ));

        let refreshed = Track {
            id: "spotify:AAAAAAAAAAAAAAAAAAAAAA".into(),
            provider: "spotify".into(),
            provider_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            title: "Now available title".into(),
            artist: "Artist".into(),
            source_url: "https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA".into(),
            ..Track::default()
        };
        let mut working_copy = vec![untouched_copy.clone()];
        if should_replace_working_copy(&working_copy, &baseline, "spotify", false) {
            working_copy = vec![refreshed.clone()];
        }
        assert_eq!(working_copy[0].title, "Now available title");

        let mut user_edit = untouched_copy;
        user_edit.title = "My local title edit".into();
        let mut edited_working_copy = vec![user_edit.clone()];
        if should_replace_working_copy(&edited_working_copy, &baseline, "spotify", false) {
            edited_working_copy = vec![refreshed];
        }
        assert_eq!(edited_working_copy[0].title, "My local title edit");
    }

    #[test]
    fn archive_metadata_enrichment_does_not_mask_remote_add_remove_on_refresh() {
        let mut old_a = remote_entry("uid-a", "AAAAAAAAAAAAAAAAAAAAAA");
        let mut old_b = remote_entry("uid-b", "BBBBBBBBBBBBBBBBBBBBBB");
        old_a.duration_seconds = None;
        old_b.duration_seconds = Some(200.0);
        let baseline = RemoteSnapshot {
            entries: vec![old_a.clone(), old_b.clone()],
            revision: "old-revision".into(),
            content_hash: "old-hash".into(),
            complete: true,
        };
        let original_working_copy = vec![
            entry_to_track("spotify", &old_a, 0),
            entry_to_track("spotify", &old_b, 1),
        ];
        assert!(should_replace_working_copy(
            &original_working_copy,
            &baseline,
            "spotify",
            false
        ));

        // Importing the next immutable snapshot enriches the shared catalog row for A before
        // refresh replaces the working collection. Against the old baseline this resembles a
        // local edit, even though the working collection was untouched.
        let mut enriched_a = old_a.clone();
        enriched_a.duration_seconds = Some(195.0);
        let after_archive_enrichment = vec![
            entry_to_track("spotify", &enriched_a, 0),
            entry_to_track("spotify", &old_b, 1),
        ];
        assert!(!collection_matches(
            &after_archive_enrichment,
            &baseline,
            "spotify"
        ));

        let mut new_a = enriched_a;
        new_a.title = "Updated A".into();
        let new_c = remote_entry("uid-c", "CCCCCCCCCCCCCCCCCCCCCC");
        let refreshed_working_copy = vec![
            entry_to_track("spotify", &new_a, 0),
            entry_to_track("spotify", &new_c, 1),
        ];
        let mut working_copy = original_working_copy.clone();
        if should_replace_working_copy(&working_copy, &baseline, "spotify", false) {
            working_copy = refreshed_working_copy.clone();
        }
        assert_eq!(
            working_copy
                .iter()
                .map(|track| track.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["AAAAAAAAAAAAAAAAAAAAAA", "CCCCCCCCCCCCCCCCCCCCCC"]
        );

        let mut user_edit = original_working_copy;
        user_edit[0].title = "My local title edit".into();
        let mut edited_working_copy = user_edit.clone();
        if should_replace_working_copy(&edited_working_copy, &baseline, "spotify", false) {
            edited_working_copy = refreshed_working_copy;
        }
        assert_eq!(edited_working_copy[0].title, "My local title edit");
        assert_eq!(edited_working_copy[1].provider_id, "BBBBBBBBBBBBBBBBBBBBBB");
    }

    #[test]
    fn spotify_profile_inspection_request_is_bounded_and_normalized() {
        for owner in [
            "legacy.name",
            "Andrew.Kalš",
            "https://open.spotify.com/user/Andrew.Kal%C5%A1/",
        ] {
            let request: SpotifyProfileInspectionRequest =
                serde_json::from_value(json!({"ownerId":owner})).unwrap();
            assert_eq!(
                request.normalized_owner_id().unwrap(),
                if owner.starts_with("https:") {
                    "Andrew.Kalš"
                } else {
                    owner
                }
            );
        }
        assert!(serde_json::from_value::<SpotifyProfileInspectionRequest>(
            json!({"ownerId":"legacy","cursor":"private"})
        )
        .is_err());
        for owner in [
            "",
            "https://private.example/user/legacy",
            "https://open.spotify.com/user/legacy?token=secret",
            "https://open.spotify.com/user/one%2Ftwo",
            "https://open.spotify.com/user/legacy/extra",
            "legacy\nsecret",
            "legacy name",
        ] {
            let request = SpotifyProfileInspectionRequest {
                owner_id: owner.into(),
                offset: None,
                limit: None,
                mode: None,
                playlist_limit: None,
            };
            assert_eq!(
                request.normalized_owner_id().unwrap_err(),
                "Spotify profile inspection owner is invalid"
            );
        }
    }
    #[test]
    fn spotify_recording_context_preserves_only_consistent_optional_album_metadata() {
        let artist = "aaaaaaaaaaaaaaaaaaaaaa";
        let album = "cccccccccccccccccccccc";
        let base = json!({"trackId":"bbbbbbbbbbbbbbbbbbbbbb","title":"Fixture","artist":"Artist","album":"Release","url":"https://open.spotify.com/track/bbbbbbbbbbbbbbbbbbbbbb","recordingContext":{"albumId":album,"albumUri":format!("spotify:album:{album}"),"trackNumber":1,"discNumber":2,"isrc":"USABC2600001","releaseDate":"1988-05-03"}});
        let page = json!({"artistId":artist,"items":[base.clone()],"nextCursor":null,"total":1});
        let parsed = parse_artist_tracks_page(&page, artist, None, "account").unwrap();
        assert_eq!(
            parsed.tracks[0].provenance["recordingContext"]["albumId"],
            album
        );
        assert_eq!(
            parsed.tracks[0].provenance["recordingContext"]["trackNumber"],
            1
        );
        assert_eq!(
            parsed.tracks[0].provenance["recordingContext"]["discNumber"],
            2
        );
        assert_eq!(
            parsed.tracks[0].provenance["recordingContext"]["isrc"],
            "USABC2600001"
        );
        assert_eq!(
            parsed.tracks[0].provenance["recordingContext"]["releaseDate"],
            "1988-05-03"
        );
        let legacy = json!({"artistId":artist,"items":[{"trackId":"bbbbbbbbbbbbbbbbbbbbbb","title":"Fixture","artist":"Artist","album":"Release","url":"https://open.spotify.com/track/bbbbbbbbbbbbbbbbbbbbbb"}],"nextCursor":null});
        let old = parse_artist_tracks_page(&legacy, artist, None, "account").unwrap();
        assert!(old.tracks[0].provenance.get("recordingContext").is_none());
        let position_only = json!({"artistId":artist,"items":[{"trackId":"bbbbbbbbbbbbbbbbbbbbbb","title":"Fixture","artist":"Artist","url":"https://open.spotify.com/track/bbbbbbbbbbbbbbbbbbbbbb","recordingContext":{"trackNumber":1}}],"nextCursor":null});
        let position = parse_artist_tracks_page(&position_only, artist, None, "account").unwrap();
        assert_eq!(
            position.tracks[0].provenance["recordingContext"]["trackNumber"],
            1
        );
        for invalid_context in [
            json!({"albumId":album,"albumUri":"spotify:album:dddddddddddddddddddddd"}),
            json!({"albumId":album,"albumUri":format!("spotify:album:{album}"),"trackNumber":0}),
            json!({"albumId":album,"albumUri":format!("spotify:album:{album}"),"isrc":"bad"}),
            json!({"unreviewedIdentity":"x"}),
        ] {
            let mut invalid = base.clone();
            invalid["recordingContext"] = invalid_context;
            let page = json!({"artistId":artist,"items":[invalid],"nextCursor":null});
            assert!(parse_artist_tracks_page(&page, artist, None, "account").is_err());
        }
    }

    #[test]
    fn artist_track_pages_preserve_metadata_duplicates_and_reject_bad_identity_or_paging() {
        let artist = "aaaaaaaaaaaaaaaaaaaaaa";
        let row = json!({"trackId":"bbbbbbbbbbbbbbbbbbbbbb","title":"Fixture","artist":"Artist","album":"Release","url":"https://open.spotify.com/track/bbbbbbbbbbbbbbbbbbbbbb","artworkUrl":"https://i.scdn.co/image/fixture","durationSeconds":123.456});
        let mut page =
            json!({"artistId":artist,"items":[row.clone(),row],"nextCursor":"32","total":null});
        let parsed = parse_artist_tracks_page(&page, artist, Some("30"), "account").unwrap();
        assert_eq!(parsed.tracks.len(), 2);
        assert_eq!(parsed.tracks[0].id, parsed.tracks[1].id);
        assert_ne!(
            parsed.tracks[0].provenance["remoteEntryId"],
            parsed.tracks[1].provenance["remoteEntryId"]
        );
        assert_eq!(parsed.tracks[0].provenance["position"], 31);
        assert_eq!(parsed.tracks[0].duration_seconds, Some(123.456));
        assert_eq!(parsed.tracks[0].album.as_deref(), Some("Release"));
        assert!(parsed.tracks[0].artwork_url.is_some());
        assert!(parsed.tracks[0].provenance.get("plays").is_none());
        assert!(parsed.total.is_none());
        assert!(
            parse_artist_tracks_page(&page, "cccccccccccccccccccccc", Some("30"), "account")
                .is_err()
        );
        for next in [
            json!("30"),
            json!("33"),
            json!("61"),
            json!("opaque"),
            json!(12),
        ] {
            page["nextCursor"] = next;
            assert!(parse_artist_tracks_page(&page, artist, Some("30"), "account").is_err());
        }
        page["nextCursor"] = Value::Null;
        let valid = page["items"][0].clone();
        for row in [
            json!({"trackId":"invalid","available":false}),
            json!({"trackId":"bbbbbbbbbbbbbbbbbbbbbb","title":"Fixture","url":"https://open.spotify.com/track/cccccccccccccccccccccc"}),
            json!({"trackId":"bbbbbbbbbbbbbbbbbbbbbb","available":false,"url":"https://open.spotify.com/album/bbbbbbbbbbbbbbbbbbbbbb"}),
        ] {
            page["items"] = json!([row]);
            assert!(parse_artist_tracks_page(&page, artist, None, "account").is_err());
        }
        page["items"] = json!([{"trackId":"bbbbbbbbbbbbbbbbbbbbbb","available":false}]);
        let unavailable = parse_artist_tracks_page(&page, artist, None, "account").unwrap();
        assert_eq!(unavailable.tracks[0].provider_id, "bbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(unavailable.tracks[0].provenance["available"], false);
        page["items"] = json!([valid]);
        page["items"] = json!(vec![page["items"][0].clone(); 31]);
        assert!(parse_artist_tracks_page(&page, artist, None, "account").is_err());
    }

    #[test]
    fn spotify_profile_inspection_page_bounds_are_finite() {
        for (args, expected) in [
            (json!({"ownerId":"legacy"}), (0, 30)),
            (json!({"ownerId":"legacy","offset":28,"limit":2}), (28, 2)),
            (json!({"ownerId":"legacy","offset":30,"limit":30}), (30, 30)),
            (
                json!({"ownerId":"legacy","offset":10000000,"limit":1}),
                (10000000, 1),
            ),
        ] {
            let request: SpotifyProfileInspectionRequest = serde_json::from_value(args).unwrap();
            assert_eq!(request.page_bounds().unwrap(), expected);
        }
        for args in [
            json!({"ownerId":"legacy","offset":10000001}),
            json!({"ownerId":"legacy","limit":0}),
            json!({"ownerId":"legacy","limit":31}),
            json!({"ownerId":"legacy","offset":-1}),
            json!({"ownerId":"legacy","offset":4294967296_u64}),
            json!({"ownerId":"legacy","limit":256}),
            json!({"ownerId":"legacy","cursor":"private"}),
            json!({"ownerId":"legacy","url":"https://private.example"}),
        ] {
            let accepted = serde_json::from_value::<SpotifyProfileInspectionRequest>(args)
                .is_ok_and(|request| request.page_bounds().is_ok());
            assert!(!accepted);
        }
    }
    #[test]
    fn spotify_profile_summary_inspection_is_a_separate_bounded_mode() {
        let request: SpotifyProfileInspectionRequest = serde_json::from_value(json!({
            "ownerId":"legacy", "mode":"summary", "playlistLimit":200
        }))
        .unwrap();
        assert!(request.summary_mode());
        assert_eq!(request.summary_limit().unwrap(), 200);
        assert!(request.page_bounds().is_err());
        for args in [
            json!({"ownerId":"legacy","mode":"summary","playlistLimit":201}),
            json!({"ownerId":"legacy","mode":"summary","playlistLimit":0}),
            json!({"ownerId":"legacy","mode":"summary","playlistLimit":200,"offset":1}),
            json!({"ownerId":"legacy","mode":"summary","playlistLimit":200,"limit":30}),
            json!({"ownerId":"legacy","mode":"unknown"}),
        ] {
            let accepted = serde_json::from_value::<SpotifyProfileInspectionRequest>(args)
                .is_ok_and(|request| request.summary_limit().is_ok());
            assert!(!accepted);
        }
        let mut shape = json!({"profileResponseShape":{"version":1,"responseMode":"summary","httpStatus":200,"contentTypeClass":"protobuf","bodyBytes":100,"parseClass":"valid","failureClass":"page_count","decodedRows":99,"decodedTotal":110,"playlistIdHashes":vec!["a".repeat(64);99],"topLevelFields":[{"field":8,"wire":2,"count":99},{"field":9,"wire":0,"count":1}]}});
        assert!(parse_spotify_profile_response_shape(&shape).is_ok());
        shape["profileResponseShape"]["playlistIdHashes"] = json!(vec!["a".repeat(64); 201]);
        assert!(parse_spotify_profile_response_shape(&shape).is_err());
    }
    fn spotify_profile_shape_fixture() -> Value {
        json!({"profileResponseShape":{"version":1,"responseMode":"paged","httpStatus":200,"contentTypeClass":"protobuf","bodyBytes":2097152,"parseClass":"valid","failureClass":"none","decodedRows":200,"decodedTotal":i32::MAX,"topLevelFields":[{"field":536870911,"wire":5,"count":10000}]}})
    }
    #[test]
    fn spotify_profile_hash_diagnostics_are_bounded_and_do_not_accept_raw_ids() {
        let mut fixture = spotify_profile_shape_fixture();
        fixture["profileResponseShape"]["decodedRows"] = json!(2);
        fixture["profileResponseShape"]["playlistIdHashes"] =
            json!(["a".repeat(64), "b".repeat(64)]);
        assert!(parse_spotify_profile_response_shape(&fixture).is_ok());
        for hashes in [
            json!(["raw-private-playlist-id"]),
            json!(["A".repeat(64), "b".repeat(64)]),
            json!(["a".repeat(64)]),
            json!(vec!["a".repeat(64); 31]),
            Value::Null,
        ] {
            let mut invalid = fixture.clone();
            invalid["profileResponseShape"]["playlistIdHashes"] = hashes;
            assert!(parse_spotify_profile_response_shape(&invalid).is_err());
        }
        fixture["profileResponseShape"]["parseClass"] = json!("malformed");
        assert!(parse_spotify_profile_response_shape(&fixture).is_err());
        fixture["profileResponseShape"]
            .as_object_mut()
            .unwrap()
            .remove("playlistIdHashes");
        assert!(parse_spotify_profile_response_shape(&fixture).is_ok());
    }
    #[test]
    fn spotify_profile_inspection_shape_is_finite_and_drops_outer_private_data() {
        let mut fixture = spotify_profile_shape_fixture();
        fixture["accountId"] = json!("private-account");
        fixture["items"] = json!([{"url":"https://private.example/token"}]);
        let shape = parse_spotify_profile_response_shape(&fixture).unwrap();
        let encoded = serde_json::to_value(shape).unwrap();
        assert_eq!(encoded["decodedTotal"], i32::MAX);
        assert_eq!(encoded["topLevelFields"][0]["field"], 536870911);
        assert!(!encoded.to_string().contains("private"));
        assert!(!encoded.to_string().contains("token"));
        for (key, value) in [
            ("version", json!(2)),
            ("httpStatus", json!(99)),
            ("httpStatus", json!(600)),
            ("bodyBytes", json!(2097153)),
            ("decodedRows", json!(201)),
            ("decodedTotal", json!((i32::MAX as u64) + 1)),
            ("contentTypeClass", json!("private-content-type")),
            ("parseClass", json!("private-parser-error")),
            ("failureClass", json!("private-failure")),
            ("rawBody", json!("private")),
        ] {
            let mut invalid = spotify_profile_shape_fixture();
            invalid["profileResponseShape"][key] = value;
            assert_eq!(
                parse_spotify_profile_response_shape(&invalid).unwrap_err(),
                "Spotify profile response structure is unavailable or invalid",
                "accepted {key}"
            );
        }
        for (key, value) in [
            ("field", json!(0)),
            ("field", json!(536870912)),
            ("wire", json!(6)),
            ("count", json!(0)),
            ("count", json!(10001)),
            ("raw", json!("secret")),
        ] {
            let mut invalid = spotify_profile_shape_fixture();
            invalid["profileResponseShape"]["topLevelFields"][0][key] = value;
            assert!(parse_spotify_profile_response_shape(&invalid).is_err());
        }
        let mut invalid = spotify_profile_shape_fixture();
        invalid["profileResponseShape"]["topLevelFields"] =
            json!(vec![json!({"field":1,"wire":0,"count":1}); 25]);
        assert!(parse_spotify_profile_response_shape(&invalid).is_err());
        let mut optional = spotify_profile_shape_fixture();
        optional["profileResponseShape"]["decodedTotal"] = Value::Null;
        assert!(parse_spotify_profile_response_shape(&optional).is_ok());
    }

    #[test]
    fn spotify_legacy_profile_ids_allow_unicode_and_punctuation_with_matching_url() {
        let legacy_id = "Andrew.Kalš";
        assert!(valid_spotify_profile_id(legacy_id));
        let item = json!({
            "id": legacy_id,
            "kind": "profile",
            "title": "Andrew Kalš",
            "url": "https://open.spotify.com/user/Andrew.Kal%C5%A1"
        });
        let normalized = normalize_catalog_item(&item, "spotify", "profile").unwrap();
        assert_eq!(normalized["id"], legacy_id);
        assert_eq!(normalized["url"], item["url"]);
        assert!(!valid_spotify_profile_id("Andrew/Kalš"));
        assert!(!valid_spotify_profile_id("Andrew\\Kalš"));
        assert!(!valid_spotify_profile_id("Andrew\nKalš"));
    }

    #[test]
    fn spotify_legacy_profile_id_must_match_decoded_official_profile_path() {
        let item = json!({
            "id": "Andrew.Kalš",
            "title": "Andrew Kalš",
            "url": "https://open.spotify.com/user/other.user"
        });
        assert!(normalize_catalog_item(&item, "spotify", "profile").is_err());
        assert_eq!(
            decode_url_path_segment_once("Andrew.Kal%C5%A1").as_deref(),
            Some("Andrew.Kalš")
        );
        assert_eq!(
            decode_url_path_segment_once("%252F").as_deref(),
            Some("%2F")
        );
    }

    #[test]
    fn youtube_browse_keeps_large_provider_pages_without_losing_occurrences() {
        let entries: Vec<Value> = (0..250)
            .map(|index| {
                json!({
                    "entryId": format!("occurrence-{index}"),
                    "trackId": "abcdefghijk",
                    "title": "Repeated video",
                    "artist": "Channel",
                    "url": "https://www.youtube.com/watch?v=abcdefghijk",
                    "available": true
                })
            })
            .collect();
        let tracks = parse_browse_tracks(&entries, "youtube", "source-page").unwrap();
        assert_eq!(tracks.len(), 250);
        assert_eq!(tracks[249].provenance["remoteEntryId"], "occurrence-249");
        assert_eq!(tracks[249].provenance["position"], 250);
        assert!(parse_browse_tracks(&entries, "spotify", "source-page").is_err());
        assert!(
            parse_browse_tracks(&vec![entries[0].clone(); 1_001], "youtube", "source-page")
                .is_err()
        );
    }

    #[test]
    fn browse_page_parser_keeps_duplicate_and_unavailable_rows() {
        let repeated = json!({
            "entryId": null,
            "trackId": "youtubeTrack_1",
            "title": "Repeated",
            "artist": "Artist",
            "url": "https://music.youtube.com/watch?v=youtubeTrack_1",
            "available": true
        });
        let unavailable = json!({
            "entryId": null,
            "title": "Unavailable track",
            "artist": "Unknown Artist",
            "available": false
        });
        let tracks = parse_browse_tracks(
            &[repeated.clone(), repeated, unavailable],
            "youtube",
            "page-token-1",
        )
        .unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].provider_id, tracks[1].provider_id);
        assert_ne!(
            tracks[0].provenance["remoteEntryId"],
            tracks[1].provenance["remoteEntryId"]
        );
        assert_eq!(tracks[2].provenance["available"], false);
        assert_eq!(tracks[0].provenance["position"], 1);
        assert_eq!(tracks[0].provenance["positionScope"], "page");

        let positioned = json!({
            "entryId": "entry-global-17",
            "trackId": "youtubeTrack_1",
            "title": "Repeated",
            "artist": "Artist",
            "url": "https://music.youtube.com/watch?v=youtubeTrack_1",
            "available": true,
            "position": 17
        });
        let positioned = parse_browse_tracks(&[positioned], "youtube", "page-token-2").unwrap();
        assert_eq!(positioned[0].provenance["position"], 17);
        assert_eq!(positioned[0].provenance["positionScope"], "playlist");
        assert!(tracks[2].source_url.is_empty());
    }

    #[test]
    fn write_sync_is_opt_in_per_provider() {
        let mut state = RemoteState::default();
        assert!(state.allow_write_sync.is_empty());
        assert!(provider_supports_write_sync("spotify"));
        assert!(provider_supports_write_sync("youtube"));
        state.allow_write_sync.insert("spotify".into());
        state.allow_write_sync.insert("youtube".into());
        let encoded = serde_json::to_value(&state).unwrap();
        let decoded: RemoteState = serde_json::from_value(encoded).unwrap();
        assert!(decoded.allow_write_sync.contains("spotify"));
        assert!(decoded.allow_write_sync.contains("youtube"));
        let old_state: RemoteState = serde_json::from_value(json!({})).unwrap();
        assert!(old_state.allow_write_sync.is_empty());
    }

    #[test]
    fn playlist_display_totals_are_independent_of_entry_fetch_limits() {
        assert_eq!(provider_display_count(Some(&json!(10_001))), Some(10_001));
        assert_eq!(provider_display_count(Some(&json!(150_000))), Some(150_000));
        assert_eq!(
            provider_display_count(Some(&json!(i32::MAX))),
            Some(i32::MAX as usize)
        );
        assert_eq!(provider_display_count(Some(&json!(0))), Some(0));
        for invalid in [
            Value::Null,
            json!(-1),
            json!(1.5),
            json!("15000"),
            json!(true),
            json!((i32::MAX as u64) + 1),
            json!(u64::MAX),
        ] {
            assert_eq!(provider_display_count(Some(&invalid)), None);
        }
        assert_eq!(provider_display_count(None), None);
        assert_eq!(MAX_PLAYLIST_ENTRIES, 10_000);
    }
    #[test]
    fn playlist_snapshot_consistency_accepts_only_the_finite_mode() {
        assert_eq!(
            playlist_snapshot_consistency(Some(&json!("progressive"))).unwrap(),
            Some(RemotePlaylistSnapshotConsistency::Progressive)
        );
        assert_eq!(playlist_snapshot_consistency(None).unwrap(), None);
        assert_eq!(
            playlist_snapshot_consistency(Some(&Value::Null)).unwrap(),
            None
        );
        for invalid in [
            json!("unknown-private-mode"),
            json!(true),
            json!(1),
            json!({"mode":"progressive"}),
        ] {
            let error = playlist_snapshot_consistency(Some(&invalid)).unwrap_err();
            assert_eq!(
                error,
                "Provider returned an invalid playlist snapshot consistency"
            );
        }
        assert!(
            serde_json::from_value::<RemotePlaylistSnapshotConsistency>(json!(
                "unknown-private-mode"
            ))
            .is_err()
        );
    }

    #[test]
    fn remote_playlist_page_serializes_provider_completeness_without_inference() {
        let page = RemotePlaylistPage {
            provider: "spotify".into(),
            account_id: Some("account".into()),
            items: vec![],
            next_cursor: None,
            total_count: Some(137),
            truncated: true,
            complete: Some(false),
            snapshot_consistency: Some(RemotePlaylistSnapshotConsistency::Progressive),
        };
        let value = serde_json::to_value(page).unwrap();
        assert_eq!(value["totalCount"], 137);
        assert_eq!(value["snapshotConsistency"], "progressive");
        assert_eq!(value["truncated"], true);
        assert_eq!(value["complete"], false);
        assert!(value["nextCursor"].is_null());
    }

    #[test]
    fn write_planner_requires_an_official_url_matching_spotify_track_id() {
        let base = Track {
            id: "spotify:AAAAAAAAAAAAAAAAAAAAAA".into(),
            provider: "spotify".into(),
            provider_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            source_url: "https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA".into(),
            ..Track::default()
        };
        assert!(has_confirmed_spotify_identity(&base));
        let mut soundcloud = base.clone();
        soundcloud.provider = "soundcloud".into();
        assert!(!has_confirmed_spotify_identity(&soundcloud));
        let mut mismatched = base.clone();
        mismatched.source_url = "https://open.spotify.com/track/BBBBBBBBBBBBBBBBBBBBBB".into();
        assert!(!has_confirmed_spotify_identity(&mismatched));
        let mut non_official = base;
        non_official.source_url =
            "https://open.spotify.com.evil.test/track/AAAAAAAAAAAAAAAAAAAAAA".into();
        assert!(!has_confirmed_spotify_identity(&non_official));
    }
    #[test]
    fn reviewed_writeback_matches_are_scoped_to_binding_account_and_recording_identity() {
        let (_dir, _store, service, _transport) = duration_fixture_service("account-a");
        let track = Track {
            id: "local-recording-1".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(120.0),
            ..Track::default()
        };
        let token = "opaque-candidate-token".to_owned();
        service.track_match_candidates.lock().unwrap().insert(
            token.clone(),
            CachedTrackMatch {
                binding_id: "binding-a".into(),
                provider: "spotify".into(),
                account_id: "account-a".into(),
                local_track_id: track.id.clone(),
                local_identity_hash: track_identity_hash(&track).unwrap(),
                uri: "spotify:track:AAAAAAAAAAAAAAAAAAAAAA".into(),
                created_at: std::time::Instant::now(),
            },
        );
        let selection = ReviewedRemoteTrackMatch {
            local_track_id: track.id.clone(),
            candidate_token: token,
        };
        assert!(service
            .resolve_reviewed_matches(
                "binding-a",
                "spotify",
                "account-a",
                &[track.clone()],
                &[selection.clone()]
            )
            .is_ok());
        assert!(service
            .resolve_reviewed_matches(
                "binding-b",
                "spotify",
                "account-a",
                &[track.clone()],
                &[selection.clone()]
            )
            .is_err());
        assert!(service
            .resolve_reviewed_matches(
                "binding-a",
                "spotify",
                "account-b",
                &[track.clone()],
                &[selection.clone()]
            )
            .is_err());
        let mut changed = track.clone();
        changed.title = "Different song".into();
        assert!(service
            .resolve_reviewed_matches(
                "binding-a",
                "spotify",
                "account-a",
                &[changed],
                &[selection]
            )
            .is_err());
    }

    #[tokio::test]
    async fn reviewed_cross_catalog_preview_preserves_occurrences_and_rejects_local_edits() {
        let track = Track {
            id: "fixture:local".into(),
            provider: "fixture".into(),
            provider_id: "local".into(),
            source_url: "https://example.test/local".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            ..Track::default()
        };
        let (_dir, store, service, transport, collection_id) =
            youtube_sync_fixture(vec![], vec![track.clone(), track.clone()]);
        let track = store
            .list_tracks("collection", Some(&collection_id))
            .unwrap()[0]
            .clone();
        let unresolved = service
            .get_sync_preview("youtube-binding-fixture", None)
            .await
            .unwrap();
        assert_eq!(unresolved.changes.unresolved.len(), 2);
        service.track_match_candidates.lock().unwrap().insert(
            "reviewed-token".into(),
            CachedTrackMatch {
                binding_id: "youtube-binding-fixture".into(),
                provider: "youtube".into(),
                account_id: "fixture-account".into(),
                local_track_id: track.id.clone(),
                local_identity_hash: track_identity_hash(&track).unwrap(),
                uri: "youtube:video:newvid00001".into(),
                created_at: std::time::Instant::now(),
            },
        );
        let reviewed = service
            .get_sync_preview(
                "youtube-binding-fixture",
                Some(vec![ReviewedRemoteTrackMatch {
                    local_track_id: track.id.clone(),
                    candidate_token: "reviewed-token".into(),
                }]),
            )
            .await
            .unwrap();
        assert!(reviewed.changes.unresolved.is_empty());
        assert_eq!(reviewed.changes.add.len(), 2);
        assert_eq!(
            reviewed.changes.add[0].track_uri,
            reviewed.changes.add[1].track_uri
        );
        let mut edited = track.clone();
        edited.title = "Different recording".into();
        store
            .replace_playlist_entries(&collection_id, &[edited])
            .unwrap();
        assert!(service
            .apply_sync_preview(&reviewed.preview_id, None)
            .await
            .unwrap_err()
            .contains("Local playlist changed"));
        assert!(transport.apply_actions.lock().unwrap().is_empty());
        store
            .replace_playlist_entries(&collection_id, &[track.clone(), track.clone()])
            .unwrap();
        let fresh = service
            .get_sync_preview(
                "youtube-binding-fixture",
                Some(vec![ReviewedRemoteTrackMatch {
                    local_track_id: track.id.clone(),
                    candidate_token: "reviewed-token".into(),
                }]),
            )
            .await
            .unwrap();
        assert!(
            service
                .apply_sync_preview(&fresh.preview_id, None)
                .await
                .unwrap()
                .applied
        );
        let kept = store
            .list_tracks("collection", Some(&collection_id))
            .unwrap();
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|row| row.id == track.id
            && row.title == track.title
            && row.provider == "fixture"));
    }

    #[test]
    fn writeback_matching_rejects_edition_mismatches() {
        assert!(version_markers("Song (Main Version)").is_empty());
        assert_eq!(
            version_markers("Song - Radio Edit"),
            HashSet::from(["radio"])
        );
        assert_ne!(
            version_markers("Song (Main Version)"),
            version_markers("Song - Radio Edit")
        );
        assert_eq!(version_markers("Song - Live"), HashSet::from(["live"]));
        assert_eq!(
            version_markers("Song - Live Remix"),
            HashSet::from(["live", "remix"])
        );
        assert_eq!(
            version_markers("Song - Radio Edit Live"),
            HashSet::from(["radio", "live"])
        );
    }

    #[tokio::test]
    async fn candidate_search_rejects_partial_matches_of_combined_edition_markers() {
        for (local_title, matching_title, mismatched_title, matching_id, mismatched_id) in [
            (
                "Song (Live Remix)",
                "Song - Live Remix",
                "Song - Remix",
                "liveRemix01",
                "liveRemix02",
            ),
            (
                "Song (Radio Edit Live)",
                "Song - Radio Edit Live",
                "Song - Radio Edit",
                "radioLive01",
                "radioLive02",
            ),
        ] {
            let local = Track {
                id: format!("fixture:{matching_id}"),
                provider: "fixture".into(),
                provider_id: matching_id.into(),
                source_url: format!("https://example.test/{matching_id}"),
                title: local_title.into(),
                artist: "Fixture artist".into(),
                duration_seconds: Some(200.0),
                ..Track::default()
            };
            let (_dir, store, mut service, delegate, collection_id) =
                youtube_sync_fixture(vec![], vec![local]);
            let local = store
                .list_tracks("collection", Some(&collection_id))
                .unwrap()
                .remove(0);
            service.bridge = Arc::new(WritebackSearchFixtureTransport {
                delegate,
                rows: vec![
                    writeback_search_row(matching_id, matching_title, Some(200.0), true),
                    writeback_search_row(mismatched_id, mismatched_title, Some(200.0), true),
                ],
            });

            let result = service
                .search_remote_playlist_writeback_candidates("youtube-binding-fixture", &local.id)
                .await
                .unwrap();
            assert_eq!(result.candidates.len(), 1, "{local_title}");
            assert_eq!(result.candidates[0].title, matching_title);
        }
    }

    #[test]
    fn writeback_planning_preserves_duplicate_local_occurrences() {
        let track = Track {
            id: "local-recording-1".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            ..Track::default()
        };
        let local = vec![
            ("youtube:video:youtubeTrack_1".into(), track.clone()),
            ("youtube:video:youtubeTrack_1".into(), track),
        ];
        let changes = plan_youtube_changes(&[], &local, vec![]);
        assert_eq!(changes.add.len(), 2);
        assert_eq!(changes.add[0].track_uri, changes.add[1].track_uri);
    }

    #[test]
    fn writeback_identity_fingerprint_includes_chapter_range_and_isrc() {
        let base = Track {
            id: "local-recording-1".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            ..Track::default()
        };
        let original = track_identity_hash(&base).unwrap();
        let mut chapter = base.clone();
        chapter.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "parent".into(),
            chapter_id: "chapter-a".into(),
            source_url: "https://example.test/audio".into(),
            start_time: 0.0,
            end_time: 30.0,
            parent_title: "Song".into(),
            parent_duration_seconds: Some(120.0),
        });
        assert_ne!(original, track_identity_hash(&chapter).unwrap());
        let mut with_isrc = base;
        with_isrc.isrc = Some("USABC2600001".into());
        assert_ne!(original, track_identity_hash(&with_isrc).unwrap());
    }
    #[test]
    fn spotify_artist_inspection_accepts_only_finite_guard_shape() {
        let shape = json!({"version":1,"hasData":true,"hasGraphqlErrors":false,"listsArray":true,"listCount":1,"listKind":"list","listIdentityMatches":false,"pageItemsArray":true,"rowCount":30,"totalKind":"valid","rows":{"entityWrapper":30,"entityData":30,"trackType":30,"validTrackUri":30,"matchingDataUri":30,"identity":30,"validName":30,"contributorsArray":30,"validContributors":30},"firstFailedGuard":"list_identity","truncated":false});
        assert!(parse_spotify_artist_response_shape(&json!({"responseShape":shape})).is_ok());
        for modified in [
            {
                let mut value = shape.clone();
                value["raw"] = json!("private");
                value
            },
            {
                let mut value = shape.clone();
                value["rows"]["raw"] = json!(1);
                value
            },
            {
                let mut value = shape.clone();
                value["rows"]["identity"] = json!(10001);
                value
            },
            {
                let mut value = shape.clone();
                value["firstFailedGuard"] = json!("private");
                value
            },
            {
                let mut value = shape.clone();
                value["listKind"] = json!("private");
                value
            },
        ] {
            assert!(
                parse_spotify_artist_response_shape(&json!({"responseShape":modified})).is_err()
            );
        }
    }
    #[test]
    fn spotify_artist_inspection_rejects_unbound_ids_and_invalid_offsets() {
        for (artist, cursor) in [
            ("short", None),
            ("0123456789ABCDEFGHIJKL", Some("-1")),
            ("0123456789ABCDEFGHIJKL", Some("100001")),
        ] {
            assert!(SpotifyArtistInspectionRequest {
                artist_id: artist.into(),
                cursor: cursor.map(str::to_owned)
            }
            .validate()
            .is_err());
        }
        assert!(SpotifyArtistInspectionRequest {
            artist_id: "0123456789ABCDEFGHIJKL".into(),
            cursor: Some("30".into())
        }
        .validate()
        .is_ok());
    }
}

#[cfg(test)]
mod youtube_library_inspection_tests {
    use super::*;
    fn page() -> Value {
        json!({"accountId":"private-account", "items":[{"title":"private-title","id":"private-id"}],"nextCursor":null,"complete":null,
          "libraryResponseShape":{"version":1,"recognizedContainers":1,"directCursorItems":0,"wrappedCursorItems":0,"invalidCursorItems":0,"repeatedCursorItems":0,"distinctTokens":0,"unrecognizedContinuationNodes":0,"visitedNodes":3,"truncated":false,"ambiguous":false,"terminalObserved":true}})
    }
    #[test]
    fn library_inspection_emits_only_bounded_structure_and_never_claims_complete() {
        let result =
            serde_json::to_value(parse_youtube_library_inspection(&page()).unwrap()).unwrap();
        assert_eq!(result["rowCount"], 1);
        assert_eq!(result["complete"], Value::Null);
        assert_eq!(result["hasCursor"], false);
        assert!(!result.to_string().contains("private"));
        for (key, value) in [
            ("recognizedContainers", json!(10001)),
            ("visitedNodes", json!(30001)),
            ("version", json!(2)),
            ("untrusted", json!(true)),
            ("ambiguous", json!("false")),
        ] {
            let mut bad = page();
            bad["libraryResponseShape"][key] = value;
            assert!(parse_youtube_library_inspection(&bad).is_err(), "{key}");
        }
        for (key, value) in [
            ("complete", json!(true)),
            ("nextCursor", json!(42)),
            ("truncated", json!("false")),
        ] {
            let mut bad = page();
            bad[key] = value;
            assert!(parse_youtube_library_inspection(&bad).is_err(), "{key}");
        }
        let mut ambiguous = page();
        ambiguous["libraryResponseShape"]["ambiguous"] = json!(true);
        ambiguous["libraryResponseShape"]["terminalObserved"] = json!(false);
        assert!(parse_youtube_library_inspection(&ambiguous).is_err());
        ambiguous["complete"] = json!(false);
        ambiguous["truncated"] = json!(true);
        assert!(parse_youtube_library_inspection(&ambiguous).is_ok());
    }
}

#[cfg(test)]
mod spotify_album_hint_crosswalk_tests {
    use super::*;

    const ACCOUNT: &str = "hint-fixture-account";
    const ALBUM_A: &str = "6E4eD35ZrPXH2bF6LnPphg";
    const ALBUM_B: &str = "aaaaaaaaaaaaaaaaaaaaaa";
    const TRACK: &str = "0j8ulfrtVtBnyBr6p0CfFu";

    struct HintCrosswalkTransport {
        calls: Mutex<Vec<(String, Value)>>,
        include_target: Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for HintCrosswalkTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }

        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
        ) -> std::result::Result<Value, String> {
            if provider != "spotify" {
                return Err("unexpected provider".into());
            }
            self.calls
                .lock()
                .unwrap()
                .push((operation.to_owned(), args.clone()));
            let album_id = args
                .get("albumId")
                .and_then(Value::as_str)
                .unwrap_or(ALBUM_A);
            match operation {
                "getAlbum" => Ok(json!({
                    "accountId":ACCOUNT,
                    "album":{
                        "id":album_id,
                        "title":"Fixture album",
                        "url":format!("https://open.spotify.com/album/{album_id}"),
                        "data":{"totalTracks":16}
                    }
                })),
                "getAlbumTracks" => {
                    let source: Value =
                        serde_json::from_str(include_str!("../fixtures/spotify-album.json"))
                            .map_err(|_| "fixture invalid".to_owned())?;
                    let include_target = *self.include_target.lock().unwrap();
                    let mut items = source["tracks"]
                        .as_array()
                        .ok_or_else(|| "fixture tracks missing".to_owned())?
                        .iter()
                        .map(|row| {
                            let track_id = row["trackId"].as_str().unwrap_or_default();
                            let track_id = if !include_target && track_id == TRACK {
                                "1111111111111111111111"
                            } else {
                                track_id
                            };
                            json!({
                                "trackId":track_id,
                                "title":row["title"],
                                "artist":row["artist"],
                                "durationSeconds":row["durationMs"].as_u64().unwrap_or(0) as f64 / 1000.0,
                                "url":format!("https://open.spotify.com/track/{track_id}"),
                                "position":row["position"],
                                "available":true,
                                "albumId":album_id
                            })
                        })
                        .collect::<Vec<_>>();
                    // The missing-target case remains a complete 16-row album; only the
                    // exact immutable track identity differs at its prior ordinal.
                    if !include_target {
                        items[0]["title"] = json!("Different exact track");
                    }
                    Ok(json!({
                        "accountId":ACCOUNT,
                        "items":items,
                        "nextCursor":null,
                        "total":16
                    }))
                }
                _ => Err(format!("unexpected provider operation {operation}")),
            }
        }

        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> std::result::Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    fn preseed_musicbrainz(store: &Store) {
        let relations: Value =
            serde_json::from_str(include_str!("../fixtures/mb-url-relations.json")).unwrap();
        let release: Value =
            serde_json::from_str(include_str!("../fixtures/mb-release-public.json")).unwrap();
        store
            .set_provider_cache(
                "artists.spotify-album-crosswalk.musicbrainz-url",
                "https://open.spotify.com/album/6E4eD35ZrPXH2bF6LnPphg",
                &relations,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                "artists.spotify-album-crosswalk.musicbrainz-release",
                "b8254485-3b63-4e55-8776-7bd713fe3dd7",
                &release,
                3600,
            )
            .unwrap();
        for credit in release["media"][0]["tracks"][0]["recording"]["artist-credit"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = credit.pointer("/artist/id").and_then(Value::as_str) {
                store
                    .set_provider_cache("artists.profile.musicbrainz", id, &json!({}), 3600)
                    .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn album_hint_is_only_a_locator_and_missing_target_invalidation_is_album_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&temp.path().join("hint-crosswalk.sqlite")).unwrap());
        preseed_musicbrainz(&store);
        let transport = Arc::new(HintCrosswalkTransport {
            calls: Mutex::new(Vec::new()),
            include_target: Mutex::new(true),
        });
        let service = RemotePlaylistService::new(store.clone(), transport.clone());
        service.account_cache.lock().unwrap().insert(
            "spotify".into(),
            CachedAccount {
                id: ACCOUNT.into(),
                name: Some("Fixture".into()),
                checked_at: chrono::Utc::now(),
            },
        );
        let artists = crate::artists::ArtistService::new(store.clone()).unwrap();

        let mapped = service
            .prepare_spotify_recording_crosswalk(
                &artists,
                TRACK,
                Some(ALBUM_A),
                tokio::time::Instant::now() + std::time::Duration::from_secs(10),
                tokio::sync::watch::channel(false).1,
            )
            .await
            .unwrap();
        assert_eq!(mapped.spotify_album_id, ALBUM_A);
        assert_eq!(mapped.spotify_track_id, TRACK);
        assert_eq!(mapped.spotify_position, 1);
        assert_eq!(mapped.spotify_title, "Kaval Sviri (The Flute Plays)");
        assert_eq!(
            mapped.spotify_artist,
            "Bulgarian State Television Female Choir"
        );
        assert_eq!(mapped.spotify_duration_ms, Some(131_706));

        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|(op, _)| op == "getAlbum").count(), 2);
        assert_eq!(
            calls
                .iter()
                .filter(|(op, _)| op == "getAlbumTracks")
                .count(),
            2
        );
        assert_eq!(calls.iter().filter(|(op, _)| op == "getTrack").count(), 0);
        drop(calls);

        // Establish a second album-scoped mapping for the same ID, then prove a
        // complete but wrong hinted album cannot erase it or rewrite track context.
        let mut other_album_mapping = mapped.clone();
        other_album_mapping.spotify_album_id = ALBUM_B.into();
        crate::artists::write_verified_spotify_recording_sidecar(&store, &other_album_mapping, 1)
            .unwrap();
        let context_before = read_verified_spotify_track_context(&store, TRACK).unwrap();
        *transport.include_target.lock().unwrap() = false;
        let error = service
            .prepare_spotify_recording_crosswalk(
                &artists,
                TRACK,
                Some(ALBUM_A),
                tokio::time::Instant::now() + std::time::Duration::from_secs(10),
                tokio::sync::watch::channel(false).1,
            )
            .await
            .unwrap_err();
        assert!(error.contains("does not occur once"));
        assert!(artists
            .get_verified_spotify_recording_by_album_track(ALBUM_A, TRACK)
            .unwrap()
            .is_none());
        assert_eq!(
            artists
                .get_verified_spotify_recording_by_album_track(ALBUM_B, TRACK)
                .unwrap()
                .unwrap(),
            other_album_mapping
        );
        assert_eq!(
            read_verified_spotify_track_context(&store, TRACK).unwrap(),
            context_before
        );
    }
}

#[cfg(test)]
mod alias_hydration_tests {
    use super::*;
    fn provider_track() -> Track {
        let id = "1234567890123456789012";
        let refs = json!([{"provider":"spotify","id":"abcdefghijklmnopqrstuv","name":"Artist"}]);
        Track {
            id: format!("spotify:{id}"),
            provider: "spotify".into(),
            provider_id: id.into(),
            source_url: format!("https://open.spotify.com/track/{id}"),
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            duration_seconds: Some(120.0),
            provenance: json!({"catalog":{"verified":true,"provider":"spotify",
                "providerId":id,"artists":refs,"artistCount":1},"remoteCatalog":{"trackId":id,
                "title":"Song","artist":"Artist","durationSeconds":120.0,"artistRefs":refs,"artistCount":1}}),
            ..Track::default()
        }
    }

    fn search_row(id: &str) -> Value {
        let url = format!("https://open.spotify.com/track/{id}");
        json!({"kind":"track","id":id,"title":"Song","subtitle":"Artist","url":url,
            "data":{"id":id,"title":"Song","artist":"Artist","album":"Album",
                "durationSeconds":120.0,"url":url,
                "artistRefs":[{"provider":"spotify","id":"abcdefghijklmnopqrstuv","name":"Artist"}],
                "artistCount":1}})
    }

    #[test]
    fn exact_id_search_hydrates_one_complete_identity_and_ignores_other_rows() {
        let mut original = provider_track();
        original.genre = Some("Saved genre".into());
        original.album_artist = Some("Saved album artist".into());
        let response = json!({"accountId":"account-a","items":[
            search_row("abcdefghijklmnopqrstuv"), search_row(&original.provider_id)
        ]});
        let hydrated = exact_alias_search_track(&original, &response, "account-a").unwrap();
        assert_eq!(hydrated.id, original.id);
        assert_eq!(hydrated.album, original.album);
        assert_eq!(hydrated.duration_seconds, original.duration_seconds);
        assert_eq!(hydrated.genre, original.genre);
        assert_eq!(hydrated.album_artist, original.album_artist);
        assert_eq!(
            hydrated.provenance["providerLookup"],
            original.provenance["providerLookup"]
        );
        assert!(
            crate::artist_credit_aliases::spotify_artist_credit_fingerprint(&hydrated).is_some()
        );
    }

    #[test]
    fn exact_id_search_rejects_missing_or_duplicate_exact_ids() {
        let original = provider_track();
        let different =
            json!({"accountId":"account-a","items":[search_row("abcdefghijklmnopqrstuv")]});
        assert!(exact_alias_search_track(&original, &different, "account-a").is_err());
        let duplicate = json!({"accountId":"account-a","items":[
            search_row(&original.provider_id), search_row(&original.provider_id)
        ]});
        assert!(exact_alias_search_track(&original, &duplicate, "account-a").is_err());
    }

    #[test]
    fn exact_id_search_rejects_incomplete_credits_changed_metadata_and_account() {
        let original = provider_track();
        let mut incomplete = search_row(&original.provider_id);
        incomplete["data"]["artistCount"] = json!(2);
        let response = json!({"accountId":"account-a","items":[incomplete]});
        assert!(exact_alias_search_track(&original, &response, "account-a").is_err());

        let mut altered = search_row(&original.provider_id);
        altered["data"]["title"] = json!("Different song");
        let response = json!({"accountId":"account-a","items":[altered]});
        assert!(exact_alias_search_track(&original, &response, "account-a").is_err());

        let response = json!({"accountId":"account-b","items":[search_row(&original.provider_id)]});
        assert!(exact_alias_search_track(&original, &response, "account-a").is_err());
    }

    #[test]
    fn exact_id_search_rejects_conflicting_saved_isrc() {
        let mut original = provider_track();
        original.isrc = Some("USABC1234567".into());
        let mut row = search_row(&original.provider_id);
        row["data"]["isrc"] = json!("GBXYZ1234567");
        let response = json!({"accountId":"account-a","items":[row]});
        assert!(exact_alias_search_track(&original, &response, "account-a").is_err());
    }
    #[test]
    fn hydration_preserves_saved_tags_and_rejects_manual_identity_edits() {
        let provider = provider_track();
        let mut original = provider.clone();
        original.provenance = json!({"saved":true});
        original.album = Some("User album".into());
        original.genre = Some("User genre".into());
        let hydrated = merge_alias_hydration(&original, &provider).unwrap();
        assert_eq!(hydrated.album, original.album);
        assert_eq!(hydrated.genre, original.genre);
        assert_eq!(original.provenance, json!({"saved":true}));
        let mut changed = original.clone();
        changed.title = "Edited title".into();
        assert!(merge_alias_hydration(&changed, &provider).is_none());
        changed = original.clone();
        changed.artist = "Edited artist".into();
        assert!(merge_alias_hydration(&changed, &provider).is_none());
        changed = original;
        changed.isrc = Some("USABC1234567".into());
        assert!(merge_alias_hydration(&changed, &provider).is_none());
    }
}

#[path = "catalog_duration_proof.rs"]
mod catalog_duration_proof;
pub(crate) use catalog_duration_proof::PreparedCatalogDuration;
