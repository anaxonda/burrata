//! A narrowly scoped, authenticated bridge to the user's browser extension.
use crate::{
    models::{Chart, Track},
    storage::Store,
};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

pub const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_PROVIDER_RESULT: usize = 512 * 1024;
const PROVIDER_OPERATION_TIMEOUT: Duration = Duration::from_secs(45);
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserSnapshot {
    pub schema_version: u32,
    pub provider: String,
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_page_title: Option<String>,
    pub source_url: String,
    pub captured_at: String,
    pub complete: bool,
    pub expected_total: Option<usize>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub entries: Vec<BrowserEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserEntry {
    pub provider_id: Option<String>,
    pub source_url: Option<String>,
    pub artist: String,
    pub title: String,
    pub version: Option<String>,
    pub position: u32,
    pub rank: Option<u32>,
    pub duration_seconds: Option<f64>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub album: Option<String>,
    pub label: Option<String>,
    pub genre: Option<String>,
    pub release_date: Option<String>,
    pub artwork_url: Option<String>,
    pub isrc: Option<String>,
    pub catalog_number: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserImportResult {
    pub id: String,
    pub provider: String,
    pub kind: String,
    pub title: String,
    pub source_url: String,
    pub fetched_at: String,
    pub collection_id: String,
    pub captured_count: usize,
    pub imported_count: usize,
    pub expected_total: Option<usize>,
    pub complete: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    pub connected: bool,
    pub pending_count: usize,
    pub last_seen: Option<String>,
    #[serde(default)]
    pub provider_protocol_version: Option<u32>,
    pub host_name: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ImportRequest {
    pub id: String,
    pub url: String,
    pub provider: String,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum NativeRequest {
    Import {
        #[serde(rename = "requestId")]
        request_id: Option<String>,
        snapshot: BrowserSnapshot,
    },
    #[serde(rename = "importAlbumCollection")]
    ImportAlbumCollection {
        #[serde(rename = "requestId")]
        request_id: Option<String>,
        #[serde(rename = "sourceUrl")]
        source_url: String,
        title: String,
        #[serde(rename = "albumUrls")]
        album_urls: Vec<String>,
    },
    Heartbeat {
        #[serde(default, rename = "providerProtocolVersion")]
        provider_protocol_version: Option<u32>,
        #[serde(default, rename = "managedSessionId")]
        managed_session_id: Option<String>,
    },
    Poll {
        #[serde(default, rename = "providerProtocolVersion")]
        provider_protocol_version: Option<u32>,
        #[serde(default, rename = "managedSessionId")]
        managed_session_id: Option<String>,
    },
    Status {
        #[serde(rename = "requestId")]
        request_id: String,
        state: String,
        message: String,
    },
    #[serde(rename = "providerResult")]
    ProviderResult {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "operationId")]
        operation_id: String,
        ok: bool,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<ProviderOperationError>,
        #[serde(default, rename = "managedSessionId")]
        managed_session_id: Option<String>,
    },
    #[serde(rename = "youtubeSessionHandoff")]
    YoutubeSessionHandoff {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "pairingCode")]
        pairing_code: String,
        session: Value,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderOperationError {
    code: String,
    message: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderOperationRequest {
    id: String,
    kind: &'static str,
    operation_id: String,
    provider: String,
    operation: String,
    args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed_session_id: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
    url: String,
    token: String,
}

pub fn provider_from_url(value: &str) -> Result<&'static str, String> {
    let parsed = url::Url::parse(value).map_err(|_| "Invalid source URL")?;
    let provider = match parsed.host_str() {
        Some("beatport.com" | "www.beatport.com") => "beatport",
        Some("traxsource.com" | "www.traxsource.com") => "traxsource",
        Some("open.spotify.com") => "spotify",
        Some("music.youtube.com" | "www.youtube.com" | "youtube.com") => "youtube",
        Some("shazam.com" | "www.shazam.com") => "shazam",
        Some("last.fm" | "www.last.fm") => "lastfm",
        Some(host) if host == "bandcamp.com" || host.ends_with(".bandcamp.com") => "bandcamp",
        _ => return Err("Unsupported collection source URL".into()),
    };
    provider_url(value, provider)?;
    Ok(provider)
}
fn provider_url(value: &str, provider: &str) -> Result<url::Url, String> {
    let mut u = url::Url::parse(value).map_err(|_| "Invalid provider URL")?;
    let hosts: &[&str] = match provider {
        "beatport" => &["beatport.com", "www.beatport.com"],
        "traxsource" => &["traxsource.com", "www.traxsource.com"],
        "spotify" => &["open.spotify.com"],
        "youtube" => &["music.youtube.com", "www.youtube.com", "youtube.com"],
        "shazam" => &["shazam.com", "www.shazam.com"],
        "lastfm" => &["last.fm", "www.last.fm"],
        "bandcamp" => &["bandcamp.com"],
        _ => return Err("Unsupported browser provider".into()),
    };
    if u.scheme() != "https"
        || !(hosts.contains(&u.host_str().unwrap_or(""))
            || (provider == "bandcamp"
                && u.host_str()
                    .is_some_and(|host| host.ends_with(".bandcamp.com"))))
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some_and(|p| p != 443)
    {
        return Err("Provider URL must use its exact HTTPS host".into());
    }
    if provider == "spotify" {
        if let Some(path) = spotify_unlocalized_path(u.path()) {
            u.set_path(&path);
        }
    }
    u.set_fragment(None);
    let preserved = if provider == "lastfm" {
        let mut pairs = u
            .query_pairs()
            .filter(|(key, _)| {
                [
                    "period",
                    "date_preset",
                    "from",
                    "to",
                    "date_from",
                    "date_to",
                ]
                .contains(&key.as_ref())
            })
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        pairs.sort();
        pairs
    } else if provider == "youtube" {
        u.query_pairs()
            .filter(|(key, _)| matches!(key.as_ref(), "list" | "v"))
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    u.set_query(None);
    if !preserved.is_empty() {
        u.query_pairs_mut().extend_pairs(preserved);
    }
    Ok(u)
}
fn spotify_unlocalized_path(path: &str) -> Option<String> {
    let (locale, rest) = path.strip_prefix("/intl-")?.split_once('/')?;
    let valid_locale = if let Some((language, region)) = locale.split_once('-') {
        language.len() == 2
            && region.len() == 2
            && language.bytes().all(|b| b.is_ascii_alphabetic())
            && region.bytes().all(|b| b.is_ascii_alphabetic())
    } else {
        locale.len() == 2 && locale.bytes().all(|b| b.is_ascii_alphabetic())
    };
    valid_locale.then(|| format!("/{rest}"))
}
fn canonical_id(provider: &str, u: Option<&url::Url>, id: Option<&str>) -> Option<String> {
    let parts: Vec<_> = u
        .map(|u| {
            u.path_segments()
                .map(|p| p.filter(|s| !s.is_empty()).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    if matches!(provider, "lastfm" | "bandcamp") {
        let valid = u.filter(|url| {
            if provider == "bandcamp" {
                url.path().starts_with("/track/")
            } else {
                url.path().starts_with("/music/") && url.path().contains("/_/")
            }
        })?;
        return Some(format!("{:x}", Sha256::digest(valid.as_str().as_bytes())));
    }
    let youtube_url_id = if provider == "youtube" {
        u.and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())
        })
    } else {
        None
    };
    let from_url = match provider {
        "beatport" if parts.first() == Some(&"track") => parts.last().copied(),
        "traxsource" if parts.first() == Some(&"track") => parts.get(1).copied(),
        "shazam" if parts.first() == Some(&"song") => parts.get(1).copied(),
        "spotify" => parts
            .iter()
            .position(|p| *p == "track")
            .and_then(|i| parts.get(i + 1).copied()),
        "youtube" => youtube_url_id.as_deref(),
        _ => None,
    };
    let valid = |s: &str| match provider {
        "spotify" => s.len() == 22 && s.bytes().all(|b| b.is_ascii_alphanumeric()),
        "youtube" => {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        }
        _ => !s.is_empty() && s.len() < 32 && s.bytes().all(|b| b.is_ascii_digit()),
    };
    // URL and explicit ID must agree. A DOM-only ID still needs its provider's syntax.
    match (from_url.filter(|s| valid(s)), id.filter(|s| valid(s))) {
        (Some(a), Some(b)) if a != b => None,
        (Some(a), _) => Some(a.into()),
        (None, Some(b)) if u.is_none() => Some(b.into()),
        _ => None,
    }
}
pub fn snapshot_warnings(s: &BrowserSnapshot, chart: &Chart) -> Vec<String> {
    let mut warnings = s.warnings.clone();
    if s.entries.len() != chart.tracks.len() {
        warnings.push(
            "Duplicate entries retain their first position; all occurrence positions are preserved"
                .into(),
        );
    }
    if s.expected_total.is_some_and(|n| n != s.entries.len()) {
        warnings
            .push("Captured count differs from the expected total; capture is incomplete".into());
    }
    if s.complete && s.expected_total.is_none() {
        warnings.push("Completion cannot be verified without a known total".into());
    } else if s.complete && !snapshot_complete(s) && s.expected_total == Some(s.entries.len()) {
        warnings.push("Captured positions have gaps; capture is incomplete".into());
    }
    warnings
}
pub fn snapshot_complete(s: &BrowserSnapshot) -> bool {
    let positions: HashSet<_> = s.entries.iter().map(|e| e.position).collect();
    s.complete
        && s.expected_total == Some(s.entries.len())
        && positions.len() == s.entries.len()
        && (1..=s.entries.len() as u32).all(|p| positions.contains(&p))
}
pub fn validate_snapshot(s: &BrowserSnapshot) -> Result<Chart, String> {
    if s.schema_version != 1 {
        return Err("Unsupported browser snapshot version".into());
    }
    if !matches!(
        s.kind.as_str(),
        "chart" | "playlist" | "album" | "collection"
    ) || (s.provider == "spotify" && !matches!(s.kind.as_str(), "playlist" | "album"))
        || (s.kind == "collection" && s.provider != "bandcamp")
    {
        return Err("Invalid snapshot kind".into());
    }
    let source = provider_url(&s.source_url, &s.provider)?;
    if s.provider == "spotify" {
        let valid_path = ["playlist", "album"].iter().any(|kind| {
            source
                .path()
                .strip_prefix(&format!("/{kind}/"))
                .is_some_and(|id| !id.is_empty() && !id.contains('/'))
        });
        if !valid_path
            || (s.kind == "playlist" && !source.path().starts_with("/playlist/"))
            || (s.kind == "album" && !source.path().starts_with("/album/"))
        {
            return Err("Spotify snapshot kind must match a playlist or album URL".into());
        }
    }
    if s.title.trim().is_empty() || s.title.len() > 1000 {
        return Err("Snapshot title is required and limited to 1,000 characters".into());
    }
    if s.raw_page_title
        .as_ref()
        .is_some_and(|title| title.len() > 1000)
    {
        return Err("Raw page title is limited to 1,000 characters".into());
    }
    chrono::DateTime::parse_from_rfc3339(&s.captured_at)
        .map_err(|_| "Invalid capture timestamp")?;
    let verified_empty_playlist =
        s.kind == "playlist" && s.complete && s.expected_total == Some(0) && s.entries.is_empty();
    if (!verified_empty_playlist && s.entries.is_empty()) || s.entries.len() > 10000 {
        return Err("Capture must contain 1–10,000 entries or be a complete playlist reporting zero entries".into());
    }
    let mut tracks: Vec<Track> = Vec::new();
    let mut indices = HashMap::new();
    let mut entries: Vec<_> = s.entries.iter().collect();
    entries.sort_by_key(|entry| entry.position);
    let mut positions = HashSet::new();
    for e in entries {
        if !positions.insert(e.position) {
            return Err("Capture contains duplicate occurrence positions".into());
        }
        if e.position == 0
            || e.rank == Some(0)
            || e.artist.trim().is_empty()
            || e.title.trim().is_empty()
        {
            return Err("Entries require artist, title and positive positions/ranks".into());
        }
        if e.artist.len() > 2000 || e.title.len() > 2000 {
            return Err("Entry text is too long".into());
        }
        for value in [e.bpm, e.duration_seconds].into_iter().flatten() {
            if !value.is_finite() || value <= 0.0 {
                return Err("BPM and duration must be finite and positive".into());
            }
        }
        let entry_url = e
            .source_url
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| provider_url(v, &s.provider))
            .transpose()?;
        if let Some(image) = &e.artwork_url {
            let u = url::Url::parse(image).map_err(|_| "Invalid artwork URL")?;
            if u.scheme() != "https"
                || !u.username().is_empty()
                || u.password().is_some()
                || u.host_str().is_none()
            {
                return Err("Artwork must use HTTPS".into());
            }
        }
        let verified_id = canonical_id(&s.provider, entry_url.as_ref(), e.provider_id.as_deref());
        let fallback = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}\n{}\n{}",
                    e.artist.trim().to_lowercase(),
                    e.title.trim().to_lowercase(),
                    e.version.as_deref().unwrap_or("").trim().to_lowercase()
                )
                .as_bytes()
            )
        );
        let (provider, provider_id, id) = if let Some(id) = verified_id {
            (
                s.provider.clone(),
                id.clone(),
                format!("{}:{id}", s.provider),
            )
        } else {
            (
                "import".into(),
                fallback.clone(),
                format!("import:{fallback}"),
            )
        };
        if let Some(&index) = indices.get(&id) {
            let t: &mut Track = &mut tracks[index];
            t.provenance["occurrencePositions"]
                .as_array_mut()
                .unwrap()
                .push(json!(e.position));
            t.provenance["rawCaptures"]
                .as_array_mut()
                .unwrap()
                .push(json!(e));
            continue;
        }
        indices.insert(id.clone(), tracks.len());
        let canonical_source = if provider == "spotify" {
            format!("https://open.spotify.com/track/{provider_id}")
        } else if provider == "youtube" {
            format!("https://music.youtube.com/watch?v={provider_id}")
        } else {
            entry_url.map(|u| u.to_string()).unwrap_or_default()
        };
        tracks.push(Track { id, provider, provider_id, source_url: canonical_source, artist:e.artist.trim().into(), title:e.title.trim().into(), version:e.version.clone(), album:e.album.clone(), label:e.label.clone(), genre:e.genre.clone(), duration_seconds:e.duration_seconds, bpm:e.bpm, key:e.key.clone(), release_date:e.release_date.clone(), artwork_url:e.artwork_url.clone(), isrc:e.isrc.clone(), catalog_number:e.catalog_number.clone(), rank:if s.kind == "chart" { e.rank } else { None }, provenance:json!({"sourceKind":s.kind,"browserImport":true,"captureProvider":s.provider,"sourceUrl":source.as_str(),"rawPageTitle":s.raw_page_title,"position":e.position,"playlistPosition":if matches!(s.kind.as_str(),"playlist"|"collection") {Some(e.position)} else {None},"albumPosition":if s.kind=="album" {Some(e.position)} else {None},"occurrencePositions":[e.position],"rawCaptures":[e],"capturedAt":s.captured_at,"identity":"unverified browser capture"}), ..Track::default() });
    }
    let mut chart = Chart {
        id: uuid::Uuid::new_v4().to_string(),
        provider: s.provider.clone(),
        title: s.title.trim().into(),
        source_url: source.to_string(),
        fetched_at: s.captured_at.clone(),
        tracks,
        stale: false,
        warning: None,
    };
    let warnings = snapshot_warnings(s, &chart);
    if !warnings.is_empty() {
        chart.warning = Some(warnings.join("; "));
    }
    Ok(chart)
}

/// Expand deduplicated browser playlist/album tracks into their captured ordered occurrences.
/// Chart snapshots intentionally remain unique by recording identity so rank and chart storage
/// semantics are unchanged.
pub fn expand_playlist_occurrences(tracks: Vec<Track>) -> Vec<Track> {
    let mut expanded = Vec::new();
    let mut ordinal = 0usize;
    for track in tracks {
        let kind = track.provenance.get("sourceKind").and_then(Value::as_str);
        let position_field = match kind {
            Some("playlist" | "collection") => Some("playlistPosition"),
            Some("album") => Some("albumPosition"),
            _ => None,
        };
        let Some(position_field) = position_field else {
            expanded.push((track, ordinal, None));
            ordinal += 1;
            continue;
        };
        let positions = track
            .provenance
            .get("occurrencePositions")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_u64).collect::<Vec<_>>())
            .filter(|items| !items.is_empty())
            .unwrap_or_else(|| {
                track
                    .provenance
                    .get(position_field)
                    .and_then(Value::as_u64)
                    .into_iter()
                    .collect()
            });
        for position in positions {
            let mut occurrence = track.clone();
            occurrence.provenance["position"] = json!(position);
            occurrence.provenance[position_field] = json!(position);
            expanded.push((occurrence, ordinal, Some(position)));
            ordinal += 1;
        }
    }
    expanded.sort_by_key(|(_, ordinal, position)| (position.unwrap_or(u64::MAX), *ordinal));
    expanded.into_iter().map(|(track, _, _)| track).collect()
}

struct BridgeState {
    queue: VecDeque<Value>,
    active: HashMap<String, ImportRequest>,
    provider_operations: HashMap<
        String,
        (
            String,
            Option<String>,
            oneshot::Sender<Result<Value, String>>,
        ),
    >,
    provider_protocol_version: Option<u32>,
    expected_managed_session: Option<String>,
    managed_session_seen: Option<(String, Instant)>,
    seen: Option<(Instant, String)>,
}
pub struct BrowserBridge {
    app: tauri::AppHandle,
    store: Arc<Store>,
    token: String,
    url: String,
    discovery: PathBuf,
    inner: Mutex<BridgeState>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

struct PendingProviderOperationGuard<'a> {
    bridge: &'a BrowserBridge,
    id: String,
}
impl Drop for PendingProviderOperationGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.bridge.inner.lock() {
            state.provider_operations.remove(&self.id);
            state
                .queue
                .retain(|request| request.get("id").and_then(Value::as_str) != Some(&self.id));
        }
    }
}

#[async_trait::async_trait]
pub trait ProviderTransport: Send + Sync {
    fn provider_operations_supported(&self) -> bool;
    fn native_write_sync_enabled(&self) -> bool {
        false
    }
    async fn provider_operation(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
    ) -> Result<Value, String>;
    async fn provider_operation_cancellable(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Value, String>;
}
impl BrowserBridge {
    pub fn start(
        app: tauri::AppHandle,
        store: Arc<Store>,
        data_dir: PathBuf,
        token: String,
        extra_router: Router,
    ) -> Result<Arc<Self>, String> {
        if token.len() < 32 {
            return Err("Bridge token is too short".into());
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let url = format!(
            "http://{}",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let discovery = data_dir.join("browser-bridge.json");
        let bytes = serde_json::to_vec(&Discovery {
            url: url.clone(),
            token: token.clone(),
        })
        .map_err(|e| e.to_string())?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let pending = data_dir.join(format!("browser-bridge-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = options.open(&pending).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        // Same-directory rename publishes a complete discovery file with restrictive permissions.
        #[cfg(windows)]
        if discovery.exists() {
            std::fs::remove_file(&discovery).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&pending, &discovery).map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();
        let bridge = Arc::new(Self {
            app,
            store,
            token,
            url,
            discovery,
            inner: Mutex::new(BridgeState {
                queue: VecDeque::new(),
                active: HashMap::new(),
                provider_operations: HashMap::new(),
                provider_protocol_version: None,
                expected_managed_session: None,
                managed_session_seen: None,
                seen: None,
            }),
            stop: Mutex::new(Some(tx)),
        });
        let router = Router::new()
            .route("/browser/import", post(import_route))
            .route("/browser/heartbeat", post(heartbeat_route))
            .route("/browser/poll", post(poll_route))
            .route("/browser/status", post(status_route))
            .route("/browser/provider-result", post(provider_result_route))
            .route(
                "/browser/youtube-session-handoff",
                post(youtube_session_handoff_route),
            )
            .with_state(bridge.clone())
            .merge(extra_router)
            .layer(DefaultBodyLimit::max(MAX_MESSAGE));
        tauri::async_runtime::spawn(async move {
            if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await;
            }
        });
        Ok(bridge)
    }
    pub fn base_url(&self) -> String {
        self.url.clone()
    }
    pub fn status(&self) -> BrowserStatus {
        let s = self.inner.lock().unwrap();
        BrowserStatus {
            connected: companion_seen_is_fresh(s.seen.as_ref().map(|(time, _)| time)),
            pending_count: s.active.len() + s.provider_operations.len(),
            last_seen: s.seen.as_ref().map(|(_, date)| date.clone()),
            provider_protocol_version: s.provider_protocol_version,
            host_name: "org.burrata.browser".into(),
        }
    }
    pub fn provider_operations_supported(&self) -> bool {
        let status = self.status();
        let session_matches = self.inner.lock().ok().is_some_and(|state| {
            state
                .expected_managed_session
                .as_ref()
                .is_none_or(|expected| {
                    state
                        .managed_session_seen
                        .as_ref()
                        .is_some_and(|(actual, at)| {
                            actual == expected && at.elapsed() <= Duration::from_secs(15)
                        })
                })
        });
        session_matches
            && status.connected
            && status
                .provider_protocol_version
                .is_some_and(|version| version >= 2)
    }
    pub fn set_managed_session(&self, session_id: Option<String>) -> Result<(), String> {
        if session_id.as_ref().is_some_and(|id| {
            id.len() < 16
                || id.len() > 128
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }) {
            return Err("Managed session ID is invalid".into());
        }
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "Browser bridge is unavailable")?;
        if state.expected_managed_session != session_id {
            let previous = state.expected_managed_session.clone();
            let stale: Vec<String> = state
                .provider_operations
                .iter()
                .filter_map(|(id, (_, expected, _))| (*expected == previous).then_some(id.clone()))
                .collect();
            for id in stale {
                if let Some((_, _, sender)) = state.provider_operations.remove(&id) {
                    let _ = sender.send(Err("Browser provider session changed".into()));
                }
                state.queue.retain(|request| {
                    request.get("id").and_then(Value::as_str) != Some(id.as_str())
                });
            }
        }
        state.expected_managed_session = session_id;
        state.managed_session_seen = None;
        Ok(())
    }
    pub fn managed_session_seen(&self, session_id: &str) -> bool {
        self.inner.lock().ok().is_some_and(|state| {
            state.expected_managed_session.as_deref() == Some(session_id)
                && state
                    .managed_session_seen
                    .as_ref()
                    .is_some_and(|(actual, at)| {
                        actual == session_id && at.elapsed() <= Duration::from_secs(15)
                    })
        })
    }
    pub fn request_import(&self, value: &str) -> Result<String, String> {
        let provider = provider_from_url(value)?;
        let u = provider_url(value, provider)?;
        if provider == "spotify"
            && !["/playlist/", "/album/"]
                .iter()
                .any(|prefix| u.path().starts_with(prefix))
        {
            return Err("Expected a Spotify playlist or album".into());
        }
        let request = ImportRequest {
            id: uuid::Uuid::new_v4().to_string(),
            url: u.into(),
            provider: provider.into(),
        };
        let id = request.id.clone();
        let mut s = self.inner.lock().unwrap();
        if s.active.len() >= 100 {
            return Err("Too many pending browser requests".into());
        }
        s.active.insert(id.clone(), request.clone());
        s.queue
            .push_back(serde_json::to_value(request).map_err(|e| e.to_string())?);
        Ok(id)
    }
    /// Run one fixed, validated provider operation through the authenticated browser companion.
    /// The extension owns provider session state; the native app never receives cookies or tokens.
    pub async fn provider_operation(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
    ) -> Result<Value, String> {
        self.provider_operation_cancellable(provider, operation, args, None)
            .await
    }
    pub async fn provider_operation_cancellable(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Value, String> {
        validate_provider_operation(provider, operation, &args)?;
        if !self.status().connected {
            return Err("Browser companion is not connected".into());
        }
        if self
            .status()
            .provider_protocol_version
            .is_none_or(|version| version < 2)
        {
            return Err("Reload the Burrata browser companion and enable app-driven imports to use remote playlists".into());
        }
        if !self.provider_operations_supported() {
            return Err("The expected browser provider session is not connected".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let request = ProviderOperationRequest {
            id: id.clone(),
            kind: "providerOperation",
            operation_id: operation_id.clone(),
            provider: provider.to_owned(),
            operation: operation.to_owned(),
            args,
            managed_session_id: self
                .inner
                .lock()
                .ok()
                .and_then(|state| state.expected_managed_session.clone()),
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self
                .inner
                .lock()
                .map_err(|_| "Browser bridge is unavailable")?;
            if state.active.len() + state.provider_operations.len() >= 100 {
                return Err("Too many pending browser requests".into());
            }
            state.provider_operations.insert(
                id.clone(),
                (operation_id, request.managed_session_id.clone(), tx),
            );
            state
                .queue
                .push_back(serde_json::to_value(request).map_err(|e| e.to_string())?);
        }
        // A canceled native read drops its waiter and frees the bridge slot. A read may still
        // finish in the extension if it already received the request; it has no library write.
        let _pending_guard = PendingProviderOperationGuard {
            bridge: self,
            id: id.clone(),
        };
        let wait = async move {
            if let Some(cancel) = cancel {
                tokio::select! {
                    result = rx => result.map_err(|_| "Browser provider operation was cancelled".to_owned())?,
                    _ = wait_for_cancel(cancel) => Err("Browser provider operation was cancelled".into()),
                }
            } else {
                rx.await
                    .map_err(|_| "Browser provider operation was cancelled".to_owned())?
            }
        };
        match tokio::time::timeout(PROVIDER_OPERATION_TIMEOUT, wait).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => {
                let mut state = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser bridge is unavailable")?;
                state.provider_operations.remove(&id);
                state
                    .queue
                    .retain(|request| request.get("id").and_then(Value::as_str) != Some(&id));
                Err(message)
            }
            Err(_) => {
                let mut state = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser bridge is unavailable")?;
                state.provider_operations.remove(&id);
                state
                    .queue
                    .retain(|request| request.get("id").and_then(Value::as_str) != Some(&id));
                Err(
                    "Browser provider operation timed out; retry after checking the provider state"
                        .into(),
                )
            }
        }
    }
    fn authorize(&self, h: &HeaderMap) -> bool {
        h.get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == format!("Bearer {}", self.token))
    }
    fn seen(&self) {
        self.inner.lock().unwrap().seen = Some((Instant::now(), chrono::Utc::now().to_rfc3339()));
        let _ = self.app.emit("browser-status", self.status());
    }
    fn seen_provider_protocol(
        &self,
        version: Option<u32>,
        managed_session_id: Option<&str>,
    ) -> bool {
        let mut state = self.inner.lock().unwrap();
        if state.expected_managed_session.as_deref() != managed_session_id {
            return false;
        }
        state.seen = Some((Instant::now(), chrono::Utc::now().to_rfc3339()));
        state.provider_protocol_version = version;
        if let Some(id) = managed_session_id {
            state.managed_session_seen = Some((id.to_owned(), Instant::now()));
        }
        drop(state);
        let _ = self.app.emit("browser-status", self.status());
        true
    }
    pub fn shutdown(&self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        if let Ok(data) = std::fs::read(&self.discovery) {
            if serde_json::from_slice::<Discovery>(&data).is_ok_and(|d| d.token == self.token) {
                let _ = std::fs::remove_file(&self.discovery);
            }
        }
    }
}
#[async_trait::async_trait]
impl ProviderTransport for BrowserBridge {
    fn provider_operations_supported(&self) -> bool {
        BrowserBridge::provider_operations_supported(self)
    }
    async fn provider_operation(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
    ) -> Result<Value, String> {
        BrowserBridge::provider_operation(self, provider, operation, args).await
    }
    async fn provider_operation_cancellable(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Value, String> {
        BrowserBridge::provider_operation_cancellable(self, provider, operation, args, cancel).await
    }
}
async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
fn companion_seen_is_fresh(seen: Option<&Instant>) -> bool {
    // Chromium's background alarm is a one-minute fallback if its service
    // worker is suspended; leave a small delivery margin.
    seen.is_some_and(|time| time.elapsed() < Duration::from_secs(75))
}
fn error(message: impl ToString) -> Json<Value> {
    Json(json!({"ok":false,"error":message.to_string()}))
}
async fn import_route(
    State(b): State<Arc<BrowserBridge>>,
    headers: HeaderMap,
    Json(r): Json<NativeRequest>,
) -> (StatusCode, Json<Value>) {
    if !b.authorize(&headers) {
        return (StatusCode::UNAUTHORIZED, error("Unauthorized"));
    }
    b.seen();
    let (request_id, snapshot) = match r {
        NativeRequest::Import {
            request_id,
            snapshot,
        } => (request_id, snapshot),
        NativeRequest::ImportAlbumCollection {
            request_id,
            source_url,
            title,
            album_urls,
        } => {
            if let Some(id) = &request_id {
                let pending = b.inner.lock().unwrap().active.get(id).cloned();
                match pending {
                    None => return (StatusCode::BAD_REQUEST, error("Unknown browser request")),
                    Some(request) if request.provider != "bandcamp" => {
                        return (
                            StatusCode::BAD_REQUEST,
                            error("Requested provider is not Bandcamp"),
                        );
                    }
                    _ => {}
                }
            }
            match crate::collection_sources::import_bandcamp_album_collection(
                &source_url,
                &title,
                album_urls,
            )
            .await
            {
                Ok(snapshot) => (request_id, snapshot),
                Err(message) => return (StatusCode::BAD_REQUEST, error(message)),
            }
        }
        _ => return (StatusCode::BAD_REQUEST, error("Expected import request")),
    };
    if let Some(id) = &request_id {
        let pending = b.inner.lock().unwrap().active.get(id).cloned();
        match pending {
            None => return (StatusCode::BAD_REQUEST, error("Unknown import request")),
            Some(request) if request.provider != snapshot.provider => {
                return (
                    StatusCode::BAD_REQUEST,
                    error("Imported provider differs from requested provider"),
                );
            }
            _ => {}
        }
    }
    let chart = match validate_snapshot(&snapshot) {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, error(e)),
    };
    match b.store.import_browser_chart(&chart, &snapshot) {
        Ok(result) => {
            if let Some(id) = request_id {
                b.inner.lock().unwrap().active.remove(&id);
            }
            let _ = b.app.emit("browser-imported", &result);
            (StatusCode::OK, Json(json!({"ok":true,"result":result})))
        }
        Err(e) => (StatusCode::BAD_REQUEST, error(e)),
    }
}

async fn youtube_session_handoff_route(
    State(b): State<Arc<BrowserBridge>>,
    Json(r): Json<NativeRequest>,
) -> (StatusCode, Json<Value>) {
    let NativeRequest::YoutubeSessionHandoff {
        request_id,
        pairing_code,
        session,
    } = r
    else {
        return (
            StatusCode::BAD_REQUEST,
            error("Expected YouTube session handoff"),
        );
    };
    if request_id.len() > 200 || !request_id.starts_with("youtube-session-") {
        return (
            StatusCode::BAD_REQUEST,
            error("Invalid YouTube handoff request"),
        );
    }
    let Some(state) = b.app.try_state::<crate::AppState>() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            error("Provider session is unavailable"),
        );
    };
    match state
        .managed_provider_session
        .accept_youtube_handoff(&pairing_code, session)
        .await
    {
        Ok(account) => (StatusCode::OK, Json(json!({"ok":true,"account":account}))),
        Err(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok":false,"error":{"code":"SIGN_IN_REQUIRED","message":message}})),
        ),
    }
}
async fn heartbeat_route(
    State(b): State<Arc<BrowserBridge>>,
    headers: HeaderMap,
    Json(r): Json<NativeRequest>,
) -> (StatusCode, Json<Value>) {
    if !b.authorize(&headers) {
        return (StatusCode::UNAUTHORIZED, error("Unauthorized"));
    }
    let NativeRequest::Heartbeat {
        provider_protocol_version,
        managed_session_id,
    } = r
    else {
        return (StatusCode::BAD_REQUEST, error("Expected heartbeat request"));
    };
    if !b.seen_provider_protocol(provider_protocol_version, managed_session_id.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            error("Browser session is not authorized for this bridge"),
        );
    }
    (StatusCode::OK, Json(json!({"ok":true})))
}
async fn poll_route(
    State(b): State<Arc<BrowserBridge>>,
    headers: HeaderMap,
    Json(r): Json<NativeRequest>,
) -> (StatusCode, Json<Value>) {
    if !b.authorize(&headers) {
        return (StatusCode::UNAUTHORIZED, error("Unauthorized"));
    }
    let NativeRequest::Poll {
        provider_protocol_version,
        managed_session_id,
    } = r
    else {
        return (StatusCode::BAD_REQUEST, error("Expected poll request"));
    };
    if !b.seen_provider_protocol(provider_protocol_version, managed_session_id.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            error("Browser session is not authorized for this bridge"),
        );
    }
    let requests: Vec<_> = {
        let mut state = b.inner.lock().unwrap();
        let expected = state.expected_managed_session.clone();
        let mut ready = Vec::new();
        let mut pending = VecDeque::new();
        while let Some(request) = state.queue.pop_front() {
            let is_provider =
                request.get("kind").and_then(Value::as_str) == Some("providerOperation");
            let request_session = request.get("managedSessionId").and_then(Value::as_str);
            let eligible = provider_request_eligible(
                is_provider,
                request_session,
                expected.as_deref(),
                managed_session_id.as_deref(),
            );
            if eligible {
                ready.push(request);
            } else {
                pending.push_back(request);
            }
        }
        state.queue = pending;
        ready
    };
    (StatusCode::OK, Json(json!({"ok":true,"requests":requests})))
}
async fn status_route(
    State(b): State<Arc<BrowserBridge>>,
    headers: HeaderMap,
    Json(r): Json<NativeRequest>,
) -> (StatusCode, Json<Value>) {
    if !b.authorize(&headers) {
        return (StatusCode::UNAUTHORIZED, error("Unauthorized"));
    }
    let NativeRequest::Status {
        request_id,
        state,
        message,
    } = r
    else {
        return (StatusCode::BAD_REQUEST, error("Expected status request"));
    };
    if !matches!(state.as_str(), "loading" | "verification" | "failed") || message.len() > 2000 {
        return (
            StatusCode::BAD_REQUEST,
            error("Invalid browser request status"),
        );
    }
    let mut s = b.inner.lock().unwrap();
    if !s.active.contains_key(&request_id) {
        return (StatusCode::BAD_REQUEST, error("Unknown browser request"));
    }
    if state == "failed" {
        s.active.remove(&request_id);
    }
    drop(s);
    b.seen();
    let _ = b.app.emit(
        "browser-import-status",
        json!({"requestId":request_id,"state":state,"message":message}),
    );
    (StatusCode::OK, Json(json!({"ok":true})))
}

async fn provider_result_route(
    State(b): State<Arc<BrowserBridge>>,
    headers: HeaderMap,
    Json(r): Json<NativeRequest>,
) -> (StatusCode, Json<Value>) {
    if !b.authorize(&headers) {
        return (StatusCode::UNAUTHORIZED, error("Unauthorized"));
    }
    let NativeRequest::ProviderResult {
        request_id,
        operation_id,
        ok,
        result,
        error: remote_error,
        managed_session_id,
    } = r
    else {
        return (StatusCode::BAD_REQUEST, error("Expected provider result"));
    };
    let pending = {
        let mut state = b.inner.lock().unwrap();
        match state.provider_operations.remove(&request_id) {
            Some((expected_operation, expected_session, sender))
                if provider_result_matches(
                    &expected_operation,
                    expected_session.as_deref(),
                    &operation_id,
                    managed_session_id.as_deref(),
                ) =>
            {
                Some(sender)
            }
            Some(pending) => {
                state.provider_operations.insert(request_id, pending);
                None
            }
            None => None,
        }
    };
    let Some(sender) = pending else {
        return (
            StatusCode::BAD_REQUEST,
            error("Unknown or mismatched provider operation"),
        );
    };
    b.seen();
    let answer = if ok {
        match result {
            Some(value)
                if serde_json::to_vec(&value)
                    .is_ok_and(|bytes| bytes.len() <= MAX_PROVIDER_RESULT) =>
            {
                Ok(value)
            }
            Some(_) => Err("Provider result exceeds size limit".into()),
            None => Err("Provider result was empty".into()),
        }
    } else {
        let remote_error = remote_error
            .filter(|e| !e.code.is_empty() && e.code.len() <= 64 && e.message.len() <= 1000);
        Err(remote_error
            .map(|e| format!("{}: {}", e.code, e.message))
            .unwrap_or_else(|| "Provider operation failed".into()))
    };
    let _ = sender.send(answer);
    (StatusCode::OK, Json(json!({"ok":true})))
}

fn provider_request_eligible(
    is_provider: bool,
    request_session: Option<&str>,
    expected_session: Option<&str>,
    caller_session: Option<&str>,
) -> bool {
    if is_provider {
        request_session == expected_session && request_session == caller_session
    } else {
        caller_session.is_none()
    }
}

fn provider_result_matches(
    expected_operation: &str,
    expected_session: Option<&str>,
    actual_operation: &str,
    actual_session: Option<&str>,
) -> bool {
    expected_operation == actual_operation && expected_session == actual_session
}

fn validate_provider_operation(
    provider: &str,
    operation: &str,
    args: &Value,
) -> Result<(), String> {
    if !matches!(provider, "spotify" | "youtube") {
        return Err("Unsupported remote playlist provider".into());
    }
    let object = args
        .as_object()
        .ok_or("Provider operation args must be an object")?;
    let allowed: &[&str] = match operation {
        "connect" | "status" => &[],
        "createVerificationPlaylist" => &["accountId"],
        "verifyVerificationPlaylist" => &["accountId", "playlistId"],
        "search" => &["query", "kind", "limit", "cursor"],
        "listPlaylists" => &["ownerId", "limit", "cursor"],
        "getPlaylist" => &["playlistId", "limit", "cursor"],
        "getTrack" => &["trackId"],
        "getArtist" => &["artistId"],
        "getArtistAlbums" | "getArtistTracks" => &["artistId", "limit", "cursor"],
        "getAlbum" => &["albumId"],
        "getAlbumTracks" => &["albumId", "limit", "cursor"],
        "addTracks" => &["playlistId", "trackUris"],
        "removeTracks" => &["playlistId", "entryIds"],
        "moveTracks" => &["playlistId", "entryIds", "beforeEntryId"],
        "applyPlaylistEdits" => &[
            "previewApproved",
            "playlistId",
            "expectedEntries",
            "actions",
            "syncPreview",
        ],
        "applySyncPreview" => &[
            "previewApproved",
            "playlistId",
            "expectedEntries",
            "removeEntryIds",
            "addTrackUris",
            "desiredTrackUris",
            "syncPreview",
        ],
        _ => return Err("Unsupported provider operation".into()),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("Provider operation contains an unsupported argument".into());
    }
    if operation == "verifyVerificationPlaylist"
        && (provider != "spotify"
            || object
                .get("accountId")
                .and_then(Value::as_str)
                .is_none_or(|id| {
                    id.is_empty() || id.len() > 256 || id.chars().any(char::is_control)
                })
            || object
                .get("playlistId")
                .and_then(Value::as_str)
                .is_none_or(|id| id.len() != 22 || !id.bytes().all(|b| b.is_ascii_alphanumeric())))
    {
        return Err(
            "Verification readback requires a bounded Spotify account and playlist ID".into(),
        );
    }
    if operation == "createVerificationPlaylist"
        && object
            .get("accountId")
            .and_then(Value::as_str)
            .is_none_or(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
    {
        return Err("Verification playlist creation requires a bounded confirmed account".into());
    }
    if operation == "applyPlaylistEdits" {
        if provider != "youtube"
            || object.get("previewApproved").and_then(Value::as_bool) != Some(true)
            || object
                .get("playlistId")
                .and_then(Value::as_str)
                .is_none_or(|id| !valid_remote_id(id))
        {
            return Err("YouTube playlist edits require an approved, valid sync preview".into());
        }
        let entries = object
            .get("expectedEntries")
            .and_then(Value::as_array)
            .ok_or("YouTube sync preview has no verified playlist snapshot")?;
        let actions = object
            .get("actions")
            .and_then(Value::as_array)
            .ok_or("YouTube sync preview has no edit actions")?;
        let preview = object
            .get("syncPreview")
            .and_then(Value::as_object)
            .ok_or("YouTube edits require a stored sync preview")?;
        if preview.len() != 5
            || preview
                .get("previewId")
                .and_then(Value::as_str)
                .is_none_or(|id| id.len() > 128 || id.is_empty())
            || preview
                .get("accountId")
                .and_then(Value::as_str)
                .is_none_or(|id| id.is_empty() || id.len() > 256)
            || preview
                .get("baselineRevision")
                .and_then(Value::as_str)
                .is_none_or(|value| value.len() > 256)
            || preview
                .get("remoteRevision")
                .and_then(Value::as_str)
                .is_none_or(|value| value.len() > 256)
            || preview
                .get("remoteHash")
                .and_then(Value::as_str)
                .is_none_or(|value| {
                    value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit())
                })
        {
            return Err("YouTube edits require a valid stored sync preview".into());
        }
        if entries.len() > 10_000 || actions.is_empty() || actions.len() > 500 {
            return Err("YouTube sync preview exceeds the safe edit limits".into());
        }
        for entry in entries {
            let row = entry
                .as_object()
                .ok_or("YouTube playlist snapshot is invalid")?;
            if row.len() != 2
                || row
                    .get("entryId")
                    .and_then(Value::as_str)
                    .is_none_or(|id| !valid_provider_occurrence_id(id))
                || !row.get("trackId").is_some_and(|id| {
                    id.is_null() || id.as_str().is_some_and(valid_youtube_video_id)
                })
            {
                return Err("YouTube playlist snapshot contains an invalid occurrence".into());
            }
        }
        for action in actions {
            let row = action
                .as_object()
                .ok_or("YouTube playlist action is invalid")?;
            match row.get("type").and_then(Value::as_str) {
                Some("add")
                    if row.len() == 2
                        && row
                            .get("trackId")
                            .and_then(Value::as_str)
                            .is_some_and(valid_youtube_video_id) => {}
                Some("remove")
                    if row.len() == 3
                        && row
                            .get("entryId")
                            .and_then(Value::as_str)
                            .is_some_and(valid_provider_occurrence_id)
                        && row
                            .get("trackId")
                            .and_then(Value::as_str)
                            .is_some_and(valid_youtube_video_id) => {}
                Some("moveBefore")
                    if row.len() == 4
                        && row
                            .get("entryId")
                            .and_then(Value::as_str)
                            .is_some_and(valid_provider_occurrence_id)
                        && row
                            .get("trackId")
                            .and_then(Value::as_str)
                            .is_some_and(valid_youtube_video_id)
                        && row.get("successorEntryId").is_some_and(|id| {
                            id.is_null() || id.as_str().is_some_and(valid_provider_occurrence_id)
                        }) => {}
                _ => return Err("YouTube playlist action is not supported".into()),
            }
        }
    }
    if operation == "applySyncPreview" {
        if provider != "spotify"
            || object.get("previewApproved").and_then(Value::as_bool) != Some(true)
            || object
                .get("playlistId")
                .and_then(Value::as_str)
                .is_none_or(|id| !valid_remote_id(id))
        {
            return Err("Spotify playlist edits require an approved, valid sync preview".into());
        }
        let entries = object
            .get("expectedEntries")
            .and_then(Value::as_array)
            .ok_or("Spotify sync preview has no verified playlist snapshot")?;
        let removals = object
            .get("removeEntryIds")
            .and_then(Value::as_array)
            .ok_or("Spotify sync preview has no removal list")?;
        let additions = object
            .get("addTrackUris")
            .and_then(Value::as_array)
            .ok_or("Spotify sync preview has no addition list")?;
        let desired = object
            .get("desiredTrackUris")
            .and_then(Value::as_array)
            .ok_or("Spotify sync preview has no desired order")?;
        let preview = object
            .get("syncPreview")
            .and_then(Value::as_object)
            .ok_or("Spotify edits require a stored sync preview")?;
        if entries.len() > 10_000
            || desired.len() > 10_000
            || removals.len() + additions.len() > 500
            || preview.len() != 5
            || preview
                .get("previewId")
                .and_then(Value::as_str)
                .is_none_or(|id| id.is_empty() || id.len() > 128)
            || preview
                .get("accountId")
                .and_then(Value::as_str)
                .is_none_or(|id| id.is_empty() || id.len() > 256)
            || preview
                .get("baselineRevision")
                .and_then(Value::as_str)
                .is_none_or(|v| v.len() > 256)
            || preview
                .get("remoteRevision")
                .and_then(Value::as_str)
                .is_none_or(|v| v.len() > 256)
            || preview
                .get("remoteHash")
                .and_then(Value::as_str)
                .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
            || entries.iter().any(|row| {
                row.as_object().is_none_or(|row| {
                    row.len() != 2
                        || row
                            .get("entryId")
                            .and_then(Value::as_str)
                            .is_none_or(|id| !valid_provider_occurrence_id(id))
                        || row
                            .get("trackId")
                            .and_then(Value::as_str)
                            .is_none_or(|id| !valid_spotify_id(id))
                })
            })
            || removals.iter().any(|id| {
                id.as_str()
                    .is_none_or(|id| !valid_provider_occurrence_id(id))
            })
            || additions
                .iter()
                .any(|uri| uri.as_str().is_none_or(|uri| !valid_spotify_track_uri(uri)))
            || desired
                .iter()
                .any(|uri| uri.as_str().is_none_or(|uri| !valid_spotify_track_uri(uri)))
        {
            return Err("Spotify edits require a valid complete reviewed sync preview".into());
        }
    }
    let required_id = match operation {
        "getTrack" => Some("trackId"),
        "getArtist" | "getArtistAlbums" | "getArtistTracks" => Some("artistId"),
        "getAlbum" | "getAlbumTracks" => Some("albumId"),
        _ => None,
    };
    if required_id.is_some_and(|key| !object.contains_key(key)) {
        return Err("Spotify catalog ID is required".into());
    }
    if matches!(
        operation,
        "getTrack"
            | "getArtist"
            | "getArtistAlbums"
            | "getArtistTracks"
            | "getAlbum"
            | "getAlbumTracks"
    ) && provider != "spotify"
    {
        return Err("Spotify catalog operation is not supported for this provider".into());
    }
    if matches!(operation, "addTracks" | "removeTracks" | "moveTracks") && provider != "spotify" {
        return Err("Provider does not support playlist mutations".into());
    }
    if let Some(query) = object.get("query") {
        let query = query.as_str().ok_or("Search query must be text")?;
        if query.trim().is_empty() || query.len() > 256 {
            return Err("Search query is empty or too long".into());
        }
    }
    if let Some(kind) = object.get("kind") {
        let Some(kind) = kind.as_str() else {
            return Err("Unsupported search kind".into());
        };
        let supported = match provider {
            "spotify" => matches!(kind, "track" | "artist" | "album" | "playlist" | "profile"),
            "youtube" => matches!(kind, "track" | "playlist" | "channel"),
            _ => false,
        };
        if !supported {
            return Err("Unsupported search kind".into());
        }
    }
    if let Some(limit) = object.get("limit") {
        if !(1..=100).contains(&limit.as_u64().unwrap_or(0)) {
            return Err("Page limit must be from 1 to 100".into());
        }
    }
    for key in ["cursor", "ownerId"] {
        if let Some(value) = object.get(key) {
            if value
                .as_str()
                .is_none_or(|value| value.is_empty() || value.len() > 512)
            {
                return Err(format!("Invalid {key}"));
            }
        }
    }
    for key in ["playlistId"] {
        if let Some(value) = object.get(key) {
            let value = value.as_str().ok_or("Playlist ID must be text")?;
            if (provider == "spotify" && !valid_spotify_id(value))
                || (provider == "youtube" && !valid_remote_id(value))
            {
                return Err("Invalid remote playlist ID".into());
            }
        }
    }
    for key in ["trackId", "artistId", "albumId"] {
        if let Some(value) = object.get(key) {
            let value = value.as_str().ok_or("Catalog ID must be text")?;
            if provider != "spotify" || !valid_spotify_id(value) {
                return Err("Invalid Spotify catalog ID".into());
            }
        }
    }
    if let Some(value) = object.get("trackUris") {
        let values = value.as_array().ok_or("Track URIs must be a list")?;
        if values.is_empty() || values.len() > 100 {
            return Err("Track batch must contain 1 to 100 tracks".into());
        }
        for item in values {
            let uri = item.as_str().ok_or("Invalid Spotify track URI")?;
            let id = uri
                .strip_prefix("spotify:track:")
                .ok_or("Only Spotify track URIs can be added")?;
            if !valid_spotify_id(id) {
                return Err("Invalid Spotify track URI".into());
            }
        }
    }
    if let Some(value) = object.get("entryIds") {
        let values = value
            .as_array()
            .ok_or("Playlist entry IDs must be a list")?;
        if values.is_empty() || values.len() > 100 {
            return Err("Entry batch must contain 1 to 100 entries".into());
        }
        for item in values {
            let id = item.as_str().ok_or("Invalid playlist entry ID")?;
            if id.is_empty()
                || id.len() > 256
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err("Invalid playlist entry ID".into());
            }
        }
    }
    if let Some(value) = object.get("beforeEntryId") {
        if !value.is_null()
            && value.as_str().is_none_or(|id| {
                id.is_empty()
                    || id.len() > 256
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
        {
            return Err("Invalid destination entry ID".into());
        }
    }
    match operation {
        "search" if !object.contains_key("query") => return Err("Search query is required".into()),
        "getPlaylist" if !object.contains_key("playlistId") => {
            return Err("Playlist ID is required".into());
        }
        operation
            if matches!(operation, "addTracks" | "removeTracks" | "moveTracks")
                && !object.contains_key("playlistId") =>
        {
            return Err("Playlist ID is required".into());
        }
        "addTracks" if !object.contains_key("trackUris") => {
            return Err("Track URIs are required".into());
        }
        operation
            if matches!(operation, "removeTracks" | "moveTracks")
                && !object.contains_key("entryIds") =>
        {
            return Err("Entry IDs are required".into());
        }
        _ => {}
    }
    Ok(())
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
fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
fn valid_spotify_track_uri(value: &str) -> bool {
    value
        .strip_prefix("spotify:track:")
        .is_some_and(valid_spotify_id)
}
fn valid_remote_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn read_frame(reader: &mut impl Read) -> Result<Option<Vec<u8>>, String> {
    let mut prefix = [0u8; 4];
    let mut read = 0;
    while read < 4 {
        match reader.read(&mut prefix[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err("Truncated native message header".into()),
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err("Invalid native message length".into());
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| "Truncated native message body")?;
    Ok(Some(body))
}
fn write_frame(writer: &mut impl Write, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_MESSAGE {
        return Err("Native reply exceeds size limit".into());
    }
    writer
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|_| writer.write_all(&bytes))
        .and_then(|_| writer.flush())
        .map_err(|e| e.to_string())
}
fn discovery_url(d: &Discovery) -> Result<url::Url, String> {
    let u = url::Url::parse(&d.url).map_err(|_| "Invalid bridge discovery URL")?;
    if u.scheme() != "http"
        || u.host_str() != Some("127.0.0.1")
        || u.port().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
        || d.token.len() < 32
    {
        return Err("Bridge discovery must point to authenticated loopback".into());
    }
    Ok(u)
}
fn youtube_pairing_url(code: &str) -> Result<url::Url, String> {
    let (port, nonce) = code.split_once('-').ok_or("Invalid YouTube pairing code")?;
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Invalid YouTube pairing code".into());
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or("Invalid YouTube pairing code")?;
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid YouTube pairing code".into());
    }
    url::Url::parse(&format!(
        "http://127.0.0.1:{port}/browser/youtube-session-handoff"
    ))
    .map_err(|_| "Invalid YouTube pairing target".into())
}

async fn forward_youtube_handoff(client: &reqwest::Client, request: &NativeRequest) -> Value {
    let NativeRequest::YoutubeSessionHandoff { pairing_code, .. } = request else {
        return json!({"ok":false,"error":"Invalid YouTube handoff request"});
    };
    let url = match youtube_pairing_url(pairing_code) {
        Ok(url) => url,
        Err(_) => return json!({"ok":false,"error":"Invalid YouTube pairing code"}),
    };
    let response = match client.post(url).json(request).send().await {
        Ok(response) => response,
        Err(_) => return json!({"ok":false,"error":"Paired Burrata app is unavailable"}),
    };
    if response
        .content_length()
        .is_some_and(|size| size > MAX_MESSAGE as u64)
    {
        return json!({"ok":false,"error":"Bridge reply exceeds size limit"});
    }
    let mut response = response;
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_MESSAGE => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) => return json!({"ok":false,"error":"Bridge reply exceeds size limit"}),
            Ok(None) => break,
            Err(_) => return json!({"ok":false,"error":"Invalid bridge response"}),
        }
    }
    serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| json!({"ok":false,"error":"Invalid bridge response"}))
}
async fn forward_native(client: &reqwest::Client, d: &Discovery, r: &NativeRequest) -> Value {
    if matches!(r, NativeRequest::YoutubeSessionHandoff { .. }) {
        return forward_youtube_handoff(client, r).await;
    }
    let route = match r {
        NativeRequest::Import { .. } | NativeRequest::ImportAlbumCollection { .. } => {
            "browser/import"
        }
        NativeRequest::Heartbeat { .. } => "browser/heartbeat",
        NativeRequest::Poll { .. } => "browser/poll",
        NativeRequest::Status { .. } => "browser/status",
        NativeRequest::ProviderResult { .. } => "browser/provider-result",
        NativeRequest::YoutubeSessionHandoff { .. } => "browser/youtube-session-handoff",
    };
    let u = match discovery_url(d).and_then(|u| u.join(route).map_err(|e| e.to_string())) {
        Ok(u) => u,
        Err(e) => return json!({"ok":false,"error":e}),
    };
    match client.post(u).bearer_auth(&d.token).json(r).send().await {
        Ok(mut response) => {
            if response
                .content_length()
                .is_some_and(|n| n > MAX_MESSAGE as u64)
            {
                return json!({"ok":false,"error":"Bridge reply exceeds size limit"});
            }
            let mut bytes = Vec::new();
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_MESSAGE => {
                        bytes.extend_from_slice(&chunk)
                    }
                    Ok(Some(_)) => {
                        return json!({"ok":false,"error":"Bridge reply exceeds size limit"});
                    }
                    Ok(None) => break,
                    Err(_) => return json!({"ok":false,"error":"Invalid bridge response"}),
                }
            }
            serde_json::from_slice::<Value>(&bytes)
                .unwrap_or_else(|_| json!({"ok":false,"error":"Invalid bridge response"}))
        }
        Err(_) => json!({"ok":false,"error":"Burrata browser bridge unavailable"}),
    }
}
pub fn native_host_main() -> Result<(), String> {
    let data_dir = std::env::var_os("BURRATA_APP_DATA")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|p| p.join("org.burrata.discovery")))
        .ok_or("Application data directory unavailable")?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    while let Some(bytes) = read_frame(&mut input)? {
        let reply = runtime.block_on(async {
            let r: NativeRequest = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(_) => return json!({"ok":false,"error":"Invalid native request"}),
            };
            if matches!(&r, NativeRequest::YoutubeSessionHandoff { .. }) {
                return forward_youtube_handoff(&client, &r).await;
            }
            let discovery = std::fs::read(data_dir.join("browser-bridge.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Discovery>(&b).ok());
            let Some(d) = discovery else {
                return json!({"ok":false,"error":"Start Burrata to connect the browser"});
            };
            forward_native(&client, &d, &r).await
        });
        write_frame(&mut output, &reply)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> BrowserSnapshot {
        serde_json::from_value(json!({"schemaVersion":1,"provider":"spotify","kind":"playlist","title":"Test","sourceUrl":"https://open.spotify.com/playlist/0123456789ABCDEFGHIJKL?si=test","capturedAt":"2026-10-11T23:00:00Z","complete":true,"expectedTotal":1,"warnings":[],"entries":[{"providerId":"0123456789ABCDEFGHIJKL","sourceUrl":"https://open.spotify.com/track/0123456789ABCDEFGHIJKL","artist":"Artist","title":"Title","position":1,"rank":1}]})).unwrap()
    }
    #[test]
    fn verification_playlist_transport_accepts_only_account_scope() {
        assert!(validate_provider_operation(
            "spotify",
            "createVerificationPlaylist",
            &json!({"accountId":"confirmed"})
        )
        .is_ok());
        for args in [
            json!({}),
            json!({"accountId":""}),
            json!({"accountId":"confirmed","name":"arbitrary"}),
            json!({"accountId":"confirmed","url":"https://example.com"}),
        ] {
            assert!(
                validate_provider_operation("youtube", "createVerificationPlaylist", &args)
                    .is_err()
            );
        }
    }

    #[test]
    fn manual_companion_heartbeat_marks_fresh_connection_without_app_polling() {
        let request: NativeRequest = serde_json::from_value(json!({"type":"heartbeat"})).unwrap();
        assert!(matches!(
            request,
            NativeRequest::Heartbeat {
                provider_protocol_version: None,
                ..
            }
        ));
        let seen = Instant::now();
        assert!(companion_seen_is_fresh(Some(&seen)));
        assert!(!companion_seen_is_fresh(None));
        assert!(!companion_seen_is_fresh(Some(
            &(Instant::now() - Duration::from_secs(76))
        )));
    }
    #[test]
    fn raw_page_title_is_retained_in_track_provenance() {
        let mut imported = snapshot();
        imported.raw_page_title = Some("Jazz music and merch on Bandcamp".into());
        let chart = validate_snapshot(&imported).unwrap();
        assert_eq!(
            chart.tracks[0].provenance["rawPageTitle"],
            "Jazz music and merch on Bandcamp"
        );
    }
    #[test]
    fn additional_sources_keep_safe_identity_and_reject_lookalikes() {
        for (provider, source, track_url) in [
            (
                "shazam",
                "https://www.shazam.com/charts/top-200/world",
                "https://www.shazam.com/song/12345/song",
            ),
            (
                "lastfm",
                "https://www.last.fm/charts",
                "https://www.last.fm/music/Artist/_/Song",
            ),
            (
                "bandcamp",
                "https://artist.bandcamp.com/album/album",
                "https://artist.bandcamp.com/track/song",
            ),
        ] {
            assert_eq!(provider_from_url(source).unwrap(), provider);
            let mut s = snapshot();
            s.provider = provider.into();
            s.kind = if provider == "bandcamp" {
                "album"
            } else {
                "chart"
            }
            .into();
            s.source_url = source.into();
            s.entries[0].source_url = Some(track_url.into());
            s.entries[0].provider_id = None;
            let chart = validate_snapshot(&s).unwrap();
            assert_eq!(chart.tracks[0].provider, provider);
            assert!(!chart.tracks[0].id.starts_with("import:"));
            s.source_url = format!("https://{provider}.evil.test/charts");
            assert!(validate_snapshot(&s).is_err());
        }
        for value in [
            "https://artist.bandcamp.com.evil.test/album/x",
            "https://user:secret@www.last.fm/charts",
            "http://www.shazam.com/charts/top-200/world",
            "https://www.last.fm:8000/charts",
        ] {
            assert!(provider_from_url(value).is_err());
        }
    }
    #[test]
    fn lastfm_chart_periods_remain_distinct_without_tracking_parameters() {
        let weekly = provider_url(
            "https://www.last.fm/user/example/library?utm_source=x&period=7day#top",
            "lastfm",
        )
        .unwrap();
        let overall = provider_url(
            "https://www.last.fm/user/example/library?period=overall",
            "lastfm",
        )
        .unwrap();
        assert_eq!(
            weekly.as_str(),
            "https://www.last.fm/user/example/library?period=7day"
        );
        assert_ne!(weekly, overall);
    }
    #[test]
    fn playlists_never_get_chart_rank() {
        let s = snapshot();
        let c = validate_snapshot(&s).unwrap();
        assert_eq!(c.tracks[0].rank, None);
        assert_eq!(c.tracks[0].provider, "spotify");
        assert!(!c.source_url.contains('?'));
        assert!(snapshot_complete(&s));
    }
    #[test]
    fn spotify_album_uses_album_position_and_requires_matching_url_kind() {
        let mut s = snapshot();
        s.kind = "album".into();
        s.source_url = "https://open.spotify.com/album/0123456789ABCDEFGHIJKL".into();
        let chart = validate_snapshot(&s).unwrap();
        assert_eq!(chart.tracks[0].rank, None);
        assert_eq!(chart.tracks[0].provenance["albumPosition"], json!(1));
        assert_eq!(chart.tracks[0].album, None);
        assert!(snapshot_complete(&s));
        s.source_url = "https://open.spotify.com/playlist/0123456789ABCDEFGHIJKL".into();
        assert!(validate_snapshot(&s).is_err());
    }
    #[test]
    fn spotify_album_display_name_is_used_without_rewriting_recording_artist() {
        let mut s = snapshot();
        s.kind = "album".into();
        s.source_url = "https://open.spotify.com/album/0123456789ABCDEFGHIJKL".into();
        s.title = "Album Artist — Album Title".into();
        let chart = validate_snapshot(&s).unwrap();
        assert_eq!(chart.title, "Album Artist — Album Title");
        assert_eq!(chart.tracks[0].artist, "Artist");
        assert_eq!(chart.tracks[0].album, None);
    }
    #[test]
    fn localized_spotify_paths_are_validated_and_canonicalized() {
        for (kind, locale, id) in [
            ("playlist", "en", "PLAYLIST0123456789ABCD"),
            ("album", "es-MX", "ALBUM0123456789ABCDEFG"),
        ] {
            let mut s = snapshot();
            s.kind = kind.into();
            s.source_url = format!("https://open.spotify.com/intl-{locale}/{kind}/{id}?si=share");
            let chart = validate_snapshot(&s).unwrap();
            assert_eq!(
                chart.source_url,
                format!("https://open.spotify.com/{kind}/{id}")
            );
            assert_eq!(
                chart.tracks[0].provenance["sourceUrl"],
                format!("https://open.spotify.com/{kind}/{id}")
            );
        }
        assert_eq!(
            spotify_unlocalized_path("/intl-en-US/album/ID"),
            Some("/album/ID".into())
        );
        assert_eq!(spotify_unlocalized_path("/intl-english/album/ID"), None);
    }
    #[test]
    fn only_complete_known_empty_playlists_are_valid_empty_snapshots() {
        let mut s = snapshot();
        s.entries.clear();
        s.expected_total = Some(0);
        assert!(validate_snapshot(&s).is_ok());
        assert!(snapshot_complete(&s));

        s.expected_total = None;
        assert!(validate_snapshot(&s).is_err());
        s.expected_total = Some(0);
        s.complete = false;
        assert!(validate_snapshot(&s).is_err());
        s.complete = true;
        s.kind = "album".into();
        s.source_url = "https://open.spotify.com/album/0123456789ABCDEFGHIJKL".into();
        assert!(validate_snapshot(&s).is_err());
    }

    #[test]
    fn capture_entry_limit_remains_enforced() {
        let mut s = snapshot();
        s.entries.clear();
        s.expected_total = Some(0);
        s.entries.resize(10001, snapshot().entries[0].clone());
        for (index, entry) in s.entries.iter_mut().enumerate() {
            entry.position = index as u32 + 1;
        }
        s.expected_total = Some(10001);
        assert!(validate_snapshot(&s).is_err());
    }

    #[test]
    fn duplicates_retain_positions() {
        let mut s = snapshot();
        let mut e = s.entries[0].clone();
        e.position = 3;
        s.entries.push(e);
        s.expected_total = Some(2);
        let c = validate_snapshot(&s).unwrap();
        assert_eq!(c.tracks.len(), 1);
        assert_eq!(c.tracks[0].provenance["occurrencePositions"], json!([1, 3]));
        assert!(c.warning.is_some());
        let occurrences = expand_playlist_occurrences(c.tracks);
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].provenance["playlistPosition"], json!(1));
        assert_eq!(occurrences[1].provenance["playlistPosition"], json!(3));
        assert_eq!(occurrences[0].rank, None);
        assert_eq!(occurrences[1].rank, None);
    }
    #[test]
    fn first_occurrence_is_smallest_position_and_gaps_are_incomplete() {
        let mut s = snapshot();
        let mut later = s.entries[0].clone();
        later.position = 3;
        later.title = "Later title".into();
        s.entries.insert(0, later);
        s.expected_total = Some(2);
        let chart = validate_snapshot(&s).unwrap();
        assert_eq!(chart.tracks[0].title, "Title");
        assert_eq!(
            chart.tracks[0].provenance["occurrencePositions"],
            json!([1, 3])
        );
        assert!(!snapshot_complete(&s));
        s.entries[0].position = 1;
        assert!(validate_snapshot(&s).is_err());
    }
    #[test]
    fn unsafe_urls_and_fields_rejected() {
        let mut s = snapshot();
        s.source_url = "https://open.spotify.com.attacker.test/playlist/id".into();
        assert!(validate_snapshot(&s).is_err());
        let mut v = serde_json::to_value(snapshot()).unwrap();
        v["entries"][0]["localPath"] = json!("/tmp/audio");
        assert!(serde_json::from_value::<BrowserSnapshot>(v).is_err());
        let mut s = snapshot();
        s.entries[0].source_url = Some("http://127.0.0.1/admin".into());
        assert!(validate_snapshot(&s).is_err());
    }
    #[test]
    fn completeness_requires_total_and_count() {
        let mut s = snapshot();
        s.expected_total = Some(10);
        assert!(!snapshot_complete(&s));
        assert!(validate_snapshot(&s).unwrap().warning.is_some());
        s.expected_total = None;
        assert!(!snapshot_complete(&s));
    }
    #[test]
    fn malformed_ids_remain_unverified() {
        let mut s = snapshot();
        s.entries[0].provider_id = Some("dom-row-123".into());
        s.entries[0].source_url = None;
        let c = validate_snapshot(&s).unwrap();
        assert_eq!(c.tracks[0].provider, "import");
        assert!(c.tracks[0].id.starts_with("import:"));
    }
    #[test]
    fn positive_finite_metadata_required() {
        let mut s = snapshot();
        s.entries[0].bpm = Some(f64::NAN);
        assert!(validate_snapshot(&s).is_err());
        s.entries[0].bpm = Some(124.);
        s.entries[0].position = 0;
        assert!(validate_snapshot(&s).is_err());
    }
    #[test]
    fn framing_roundtrip_and_malformed() {
        let v = json!({"ok":true});
        let mut output = Vec::new();
        write_frame(&mut output, &v).unwrap();
        let mut input = std::io::Cursor::new(output);
        assert_eq!(
            serde_json::from_slice::<Value>(&read_frame(&mut input).unwrap().unwrap()).unwrap(),
            v
        );
        assert!(read_frame(&mut input).unwrap().is_none());
        assert!(read_frame(&mut std::io::Cursor::new(vec![1, 2])).is_err());
        assert!(read_frame(&mut std::io::Cursor::new(
            ((MAX_MESSAGE + 1) as u32).to_le_bytes()
        ))
        .is_err());
        assert!(read_frame(&mut std::io::Cursor::new(vec![3, 0, 0, 0, 1])).is_err());
        assert!(serde_json::from_slice::<NativeRequest>(b"{malformed}").is_err());
    }
    #[test]
    fn discovery_only_authenticated_loopback() {
        assert!(discovery_url(&Discovery {
            url: "http://127.0.0.1:1234".into(),
            token: "a".repeat(32)
        })
        .is_ok());
        for url in [
            "http://localhost:1234",
            "http://127.0.0.1:1234/private",
            "https://example.com",
            "http://127.0.0.1:1234?token=secret",
        ] {
            assert!(discovery_url(&Discovery {
                url: url.into(),
                token: "a".repeat(32)
            })
            .is_err());
        }
    }
    #[test]
    fn youtube_pairing_code_selects_only_a_fixed_loopback_target() {
        let url = youtube_pairing_url("49152-0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:49152/browser/youtube-session-handoff"
        );
        for invalid in [
            "0-0123456789abcdef0123456789abcdef",
            "65536-0123456789abcdef0123456789abcdef",
            "49152-0123456789abcdef",
            "49152-0123456789abcdef0123456789abcdef/path",
            "example.com-0123456789abcdef0123456789abcdef",
        ] {
            assert!(youtube_pairing_url(invalid).is_err(), "accepted {invalid}");
        }
        let request: NativeRequest = serde_json::from_value(json!({
            "type":"youtubeSessionHandoff",
            "requestId":"youtube-session-test",
            "pairingCode":"49152-0123456789abcdef0123456789abcdef",
            "session":{}
        }))
        .unwrap();
        assert!(matches!(
            request,
            NativeRequest::YoutubeSessionHandoff { .. }
        ));
    }
    #[tokio::test]
    #[ignore = "Requires permission to bind a local TCP listener; run explicitly outside restricted sandbox"]
    async fn native_forwarding_authenticates_exact_request() {
        async fn poll(headers: HeaderMap, Json(value): Json<Value>) -> (StatusCode, Json<Value>) {
            if headers.get("authorization").and_then(|h| h.to_str().ok())
                != Some("Bearer 0123456789abcdefghijklmnopqrstuv")
            {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"ok":false,"error":"Unauthorized"})),
                );
            }
            assert_eq!(value, json!({"type":"poll","providerProtocolVersion":2}));
            (StatusCode::OK, Json(json!({"ok":true,"requests":[]})))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/browser/poll", post(poll)))
                .await
                .unwrap();
        });
        let client = reqwest::Client::new();
        let mut discovery = Discovery {
            url,
            token: "0123456789abcdefghijklmnopqrstuv".into(),
        };
        let value = forward_native(
            &client,
            &discovery,
            &NativeRequest::Poll {
                provider_protocol_version: Some(2),
                managed_session_id: None,
            },
        )
        .await;
        assert_eq!(value, json!({"ok":true,"requests":[]}));
        discovery.token = "z".repeat(32);
        assert_eq!(
            forward_native(
                &client,
                &discovery,
                &NativeRequest::Poll {
                    provider_protocol_version: Some(2),
                    managed_session_id: None,
                }
            )
            .await["ok"],
            false
        );
        task.abort();
    }

    #[test]
    fn spotify_get_track_operation_requires_one_exact_track_id() {
        let id = "1234567890123456789012";
        assert!(validate_provider_operation("spotify", "getTrack", &json!({"trackId":id})).is_ok());
        assert!(
            validate_provider_operation("spotify", "getTrack", &json!({"trackId":"short"}))
                .is_err()
        );
        assert!(validate_provider_operation(
            "spotify",
            "getTrack",
            &json!({"trackId":id,"query":"title"})
        )
        .is_err());
        assert!(
            validate_provider_operation("youtube", "getTrack", &json!({"trackId":id})).is_err()
        );
    }

    #[test]
    fn provider_requests_and_results_are_bound_to_the_managed_session() {
        let managed = Some("managed-profile-session-1234");
        assert!(provider_request_eligible(true, managed, managed, managed));
        assert!(!provider_request_eligible(true, None, managed, None));
        assert!(!provider_request_eligible(true, managed, managed, None));
        assert!(provider_request_eligible(false, None, None, None));
        assert!(!provider_request_eligible(false, None, managed, managed));
        assert!(provider_result_matches(
            "operation-1",
            managed,
            "operation-1",
            managed
        ));
        assert!(!provider_result_matches(
            "operation-1",
            managed,
            "operation-1",
            None
        ));
        assert!(!provider_result_matches(
            "operation-1",
            managed,
            "operation-2",
            managed
        ));
    }
}
