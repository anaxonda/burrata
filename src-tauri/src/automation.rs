//! Authenticated, loopback-only automation API. The MCP wire protocol lives in the
//! companion bridge; this module exposes a small JSON tool-call transport.
use crate::{
    acquisition::{AcquisitionManager, AcquisitionProvider},
    analysis::AnalysisManager,
    artists::{ArtistCredentials, ArtistRef, ArtistReleaseRef, ArtistService},
    connections::ConnectionManager,
    discovery::{DiscoveryRequest, DiscoveryService},
    models::*,
    playback_history::{PlaybackHistory, PlaybackHistorySearchOptions},
    Store,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::Emitter;
use tokio::sync::oneshot;
use uuid::Uuid;

const PERMISSIONS_KEY: &str = "automation.permissions.v1";
const IDEMPOTENCY_KEY: &str = "automation.idempotency.v1";
const MAX_PAGE: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPermissions {
    pub read: bool,
    pub control: bool,
    pub manage: bool,
}
impl Default for AutomationPermissions {
    fn default() -> Self {
        Self {
            read: true,
            control: true,
            manage: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AutomationFile {
    url: String,
    token: String,
}
#[derive(Debug, Deserialize)]
struct ToolCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}
#[derive(Debug)]
struct ToolError {
    code: &'static str,
    message: String,
}
impl ToolError {
    fn bad(s: impl Into<String>) -> Self {
        Self {
            code: "INVALID_ARGUMENT",
            message: s.into(),
        }
    }
    fn failed(s: impl Into<String>) -> Self {
        Self {
            code: "OPERATION_FAILED",
            message: s.into(),
        }
    }
}

pub struct AutomationService {
    app: tauri::AppHandle,
    store: Arc<Store>,
    acquisition: Arc<AcquisitionManager>,
    artists: Arc<ArtistService>,
    connections: Arc<ConnectionManager>,
    analysis: Arc<AnalysisManager>,
    history: Arc<PlaybackHistory>,
    data_dir: PathBuf,
    token: Mutex<String>,
    playback_state: Mutex<Value>,
    playback_updates: tokio::sync::watch::Sender<Value>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    autotag: Mutex<Option<Arc<crate::autotag::AutoTagger>>>,
    audit: Mutex<VecDeque<Value>>,
    mutations: tokio::sync::Mutex<()>,
}

impl AutomationService {
    pub fn new(
        app: tauri::AppHandle,
        store: Arc<Store>,
        acquisition: Arc<AcquisitionManager>,
        artists: Arc<ArtistService>,
        connections: Arc<ConnectionManager>,
        analysis: Arc<AnalysisManager>,
        history: Arc<PlaybackHistory>,
        data_dir: PathBuf,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            app,
            store,
            acquisition,
            artists,
            connections,
            analysis,
            history,
            data_dir,
            token: Mutex::new(Uuid::new_v4().to_string()),
            playback_state: Mutex::new(json!({"status":"idle"})),
            playback_updates: tokio::sync::watch::channel(json!({"status":"idle"})).0,
            pending: Mutex::new(HashMap::new()),
            autotag: Mutex::new(None),
            audit: Mutex::new(VecDeque::new()),
            mutations: tokio::sync::Mutex::new(()),
        });
        let _ = service.load_or_create_token();
        service
    }
    pub fn set_autotag_manager(&self, manager: Arc<crate::autotag::AutoTagger>) {
        if let Ok(mut slot) = self.autotag.lock() {
            *slot = Some(manager);
        }
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/automation/tools", get(list_tools))
            .route("/automation/call", post(call_tool))
            .with_state(self.clone())
    }
    /// Call after the shared loopback listener is bound. The token is independent
    /// from the browser bridge token and the file is owner-readable only.
    pub fn set_base_url(&self, base_url: &str) -> Result<(), String> {
        let mut url = url::Url::parse(base_url).map_err(|_| "Invalid automation listener URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url
                .host_str()
                .is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]" | "::1"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Automation URL must use loopback".into());
        }
        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{path}/automation"));
        let token = self
            .token
            .lock()
            .map_err(|_| "Automation token lock unavailable")?
            .clone();
        let payload = serde_json::to_vec(&AutomationFile {
            url: url.to_string(),
            token,
        })
        .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        let target = self.data_dir.join("automation.json");
        let temp = self
            .data_dir
            .join(format!("automation.{}.tmp", Uuid::new_v4()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            use std::io::Write;
            f.write_all(&payload).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        #[cfg(not(unix))]
        std::fs::write(&temp, &payload).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        if target.exists() {
            std::fs::remove_file(&target).map_err(|error| error.to_string())?;
        }
        std::fs::rename(temp, target).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn load_or_create_token(&self) -> Result<(), String> {
        let path = self.data_dir.join("automation.json");
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(file) = serde_json::from_slice::<AutomationFile>(&bytes) {
                if !file.token.is_empty() {
                    *self.token.lock().map_err(|_| "token lock unavailable")? = file.token;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
    pub fn permissions(&self) -> Result<AutomationPermissions, String> {
        let raw = self
            .store
            .get_json_preference(PERMISSIONS_KEY)
            .map_err(|e| e.to_string())?;
        raw.map(serde_json::from_value)
            .transpose()
            .map(|v| v.unwrap_or_default())
            .map_err(|e| e.to_string())
    }
    pub fn set_permissions(&self, p: AutomationPermissions) -> Result<(), String> {
        self.store
            .set_json_preference(
                PERMISSIONS_KEY,
                &serde_json::to_value(p).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
    }
    pub fn set_permission_values(&self, control: bool, manage: bool) -> Result<Value, String> {
        self.set_permissions(AutomationPermissions {
            read: true,
            control,
            manage,
        })?;
        Ok(self.status())
    }
    pub fn status(&self) -> Value {
        let p = self.permissions().unwrap_or_default();
        let state = self
            .playback_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| json!({"status":"unavailable"}));
        json!({"running":true,"permissions":{"read":p.read,"control":p.control,"manage":p.manage},"playback":state})
    }
    pub fn update_playback_state(&self, state: Value) {
        if let Ok(mut s) = self.playback_state.lock() {
            *s = state.clone();
            self.playback_updates.send_replace(state);
        }
    }
    pub fn playback_snapshot(&self) -> Value {
        self.playback_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| json!({"status":"idle"}))
    }
    pub fn subscribe_playback_state(&self) -> tokio::sync::watch::Receiver<Value> {
        self.playback_updates.subscribe()
    }
    /// Same-session desktop media controls use the shared player, independently of AI permissions.
    pub async fn system_playback(
        &self,
        action: &str,
        position: Option<f64>,
        volume: Option<f64>,
        mode: Option<String>,
    ) -> Result<Value, String> {
        if !matches!(
            action,
            "resume"
                | "pause"
                | "next"
                | "previous"
                | "stop"
                | "seek"
                | "volume"
                | "shuffle"
                | "repeat"
                | "openUri"
        ) {
            return Err("Unsupported system playback action".into());
        }
        self.playback(action, vec![], mode, position, volume)
            .await
            .map_err(|e| e.message)
    }
    pub fn complete_playback_request(&self, id: &str, state: Value, error: Option<String>) {
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(tx) = pending.remove(id) {
                let _ = tx.send(error.map_or(Ok(state), Err));
            }
        }
    }
    fn remove_pending_playback(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }
    fn log(&self, op: &str, status: &str, duration_ms: u128) {
        if let Ok(mut a) = self.audit.lock() {
            a.push_back(
                json!({"operation":op,"status":status,"durationMs":duration_ms,"at":chrono::Utc::now().to_rfc3339()}),
            );
            while a.len() > 100 {
                a.pop_front();
            }
        }
    }
    async fn playback(
        &self,
        action: &str,
        track_ids: Vec<String>,
        mode: Option<String>,
        position: Option<f64>,
        volume: Option<f64>,
    ) -> Result<Value, ToolError> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| ToolError::failed("Playback queue unavailable"))?
            .insert(id.clone(), tx);
        if let Err(error) = self.app.emit(
            "automation-playback",
            json!({"requestId":id,"action":action,"trackIds":track_ids,"mode":mode,"position":position,"volume":volume}),
        ) {
            self.remove_pending_playback(&id);
            let _ = self.app.emit("automation-playback-cancelled", json!({"requestId":id}));
            return Err(ToolError::failed(error.to_string()));
        }
        match tokio::time::timeout(Duration::from_secs(45), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(ToolError::failed(e)),
            Ok(Err(_)) => {
                self.remove_pending_playback(&id);
                Err(ToolError::failed("Playback response channel closed"))
            }
            Err(_) => {
                self.remove_pending_playback(&id);
                let _ = self
                    .app
                    .emit("automation-playback-cancelled", json!({"requestId":id}));
                Err(ToolError {
                    code: "TIMEOUT",
                    message: "Player did not respond in time".into(),
                })
            }
        }
    }
    async fn execute(&self, name: &str, a: Value) -> Result<Value, ToolError> {
        let permissions = self.permissions().map_err(ToolError::failed)?;
        let scope = tool_scope(name).ok_or_else(|| ToolError {
            code: "UNKNOWN_TOOL",
            message: format!("Unknown tool: {name}"),
        })?;
        if !match scope {
            "read" => permissions.read,
            "control" => permissions.control,
            _ => permissions.manage,
        } {
            return Err(ToolError {
                code: "PERMISSION_DENIED",
                message: format!("{scope} permission is disabled"),
            });
        }
        let result = match name {
            "discover_music" => {
                let query = str_arg(&a, "query", 500)?;
                let source_ids = if a.get("sourceIds").is_some() {
                    string_array(&a, "sourceIds", 20)?
                } else {
                    vec![]
                };
                let kinds = if a.get("kinds").is_some() {
                    string_array(&a, "kinds", 10)?
                } else {
                    vec![]
                };
                let allowed_sources = [
                    "library",
                    "musicbrainz",
                    "discogs",
                    "youtube",
                    "soundcloud",
                    "soulseek",
                ];
                if source_ids
                    .iter()
                    .any(|source| !allowed_sources.contains(&source.as_str()))
                {
                    return Err(ToolError::bad("sourceIds contains an unsupported source"));
                }
                if kinds.iter().any(|kind| {
                    !["artist", "album", "track", "video", "set", "file"].contains(&kind.as_str())
                }) {
                    return Err(ToolError::bad("kinds contains an unsupported result kind"));
                }
                let request = DiscoveryRequest {
                    query,
                    source_ids,
                    kinds,
                    cursor: a.get("cursor").and_then(Value::as_str).map(str::to_owned),
                    limit: usize_arg(&a, "limit", 100)?.or(Some(25)),
                    isrc: a.get("isrc").and_then(Value::as_str).map(str::to_owned),
                };
                let discovery = DiscoveryService::new(
                    self.artists.clone(),
                    self.acquisition.clone(),
                    self.store.clone(),
                    self.connections.clone(),
                );
                let pages = discovery.async_shared_search(request).await;
                for page in &pages {
                    for result in &page.results {
                        if let Some(track) = &result.track {
                            self.store.save_track(track).map_err(ToolError::failed)?;
                        }
                    }
                }
                json!({"pages":pages,"partial":pages.iter().any(|page|!page.complete),"hasWarnings":pages.iter().any(|page|!page.warnings.is_empty())})
            }
            "poll_music_search" => {
                let discovery = DiscoveryService::new(
                    self.artists.clone(),
                    self.acquisition.clone(),
                    self.store.clone(),
                    self.connections.clone(),
                );
                let page = discovery
                    .poll_slskd(&str_arg(&a, "searchId", 300)?)
                    .await
                    .map_err(ToolError::failed)?;
                json!({"page":page,"partial":!page.complete})
            }
            "inspect_source_chapters" => {
                let parent = required_track(&self.store, &str_arg(&a, "trackId", 300)?)?;
                let candidate: Candidate =
                    serde_json::from_value(a.get("candidate").cloned().ok_or_else(|| {
                        ToolError::bad("An explicit source candidate is required")
                    })?)
                    .map_err(|e| ToolError::bad(e.to_string()))?;
                validate_public_candidate(&candidate)?;
                let chapters = crate::acquisition::inspect_chapters(
                    &parent,
                    &candidate,
                    &self.store.settings().map_err(ToolError::failed)?,
                )
                .await
                .map_err(ToolError::failed)?;
                let mut registered = Vec::new();
                let mut tracks = Vec::new();
                for chapter in chapters {
                    if !valid_chapter(&chapter, candidate.duration_seconds) {
                        continue;
                    }
                    let inferred = chapter.suggested_track.as_deref();
                    let (label_artist, label_title) = chapter
                        .title
                        .split_once(" - ")
                        .map(|(artist, title)| (artist.trim(), title.trim()))
                        .filter(|(artist, title)| !artist.is_empty() && !title.is_empty())
                        .unwrap_or(("", chapter.title.as_str()));
                    let inferred_artist = inferred
                        .map(|track| track.artist.as_str())
                        .filter(|artist| !artist.trim().is_empty() && *artist != "Unknown artist")
                        .unwrap_or(label_artist);
                    let chapter_track_id = registered_chapter_id(&parent.id, &chapter.id);
                    let track = Track {
                        id: chapter_track_id.clone(),
                        provider: "chapter".into(),
                        provider_id: format!("{}:{}", parent.id, chapter.id),
                        source_url: candidate.url.clone(),
                        artist: inferred_artist.to_owned(),
                        title: inferred
                            .map(|track| track.title.clone())
                            .filter(|title| !title.trim().is_empty())
                            .unwrap_or_else(|| label_title.to_owned()),
                        duration_seconds: Some(chapter.end_seconds - chapter.start_seconds),
                        artwork_url: candidate.artwork_url.clone().or(parent.artwork_url.clone()),
                        isrc: None,
                        catalog_number: None,
                        playback_range: Some(PlaybackRange {
                            parent_track_id: parent.id.clone(),
                            chapter_id: chapter.id.clone(),
                            source_url: candidate.url.clone(),
                            start_time: chapter.start_seconds,
                            end_time: chapter.end_seconds,
                            parent_title: candidate.title.clone(),
                            parent_duration_seconds: candidate.duration_seconds,
                        }),
                        provenance: json!({"sourceKind":"chapter","parentTrackId":parent.id,"parentTrack":parent,"chapterId":chapter.id,"audioCandidate":candidate,"inferredArtist":(!inferred_artist.is_empty()).then_some(inferred_artist)}),
                        ..Track::default()
                    };
                    let saved = self.store.save_track(&track).map_err(ToolError::failed)?;
                    registered.push(json!({"chapter":chapter,"trackId":chapter_track_id}));
                    tracks.push(safe_track(saved));
                }
                json!({"parentTrackId":parent.id,"candidate":candidate,"chapters":registered,"tracks":tracks})
            }
            "get_autotag_status" => {
                let manager = self.autotag_manager()?;
                let all = manager.records().map_err(ToolError::failed)?;
                let offset = usize_arg(&a, "offset", 100000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", MAX_PAGE)?.unwrap_or(25).max(1);
                let total = all.len();
                json!({"items":all.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"offset":offset,"limit":limit,"total":total,"hasMore":offset.saturating_add(limit)<total})
            }
            "retry_autotag" => {
                json!({"queued":self.autotag_manager()?.retry(&str_arg(&a,"assetId",300)?).map_err(ToolError::failed)?})
            }
            "review_autotag_candidate" => {
                json!({"job":self.autotag_manager()?.review(&str_arg(&a,"assetId",300)?,&str_arg(&a,"providerId",300)?).map_err(ToolError::failed)?})
            }
            "apply_autotag_review" => {
                let fields = string_array(&a, "fieldKeys", 3)?;
                let asset = self
                    .autotag_manager()?
                    .apply_review(&str_arg(&a, "assetId", 300)?, &fields)
                    .await
                    .map_err(ToolError::failed)?;
                let mut value = serde_json::to_value(asset).unwrap_or_else(|_| json!({}));
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("path");
                    if let Some(track) = obj.get_mut("track").and_then(Value::as_object_mut) {
                        track.remove("localPath");
                        strip_file_fields(&mut track["provenance"]);
                    }
                }
                value
            }
            "play_chapter" => {
                let id = str_arg(&a, "trackId", 300)?;
                let track = required_track(&self.store, &id)?;
                if track.playback_range.is_none() {
                    return Err(ToolError::bad("trackId must identify a registered chapter"));
                }
                self.playback("play", vec![id], None, None, None).await?
            }
            "list_radio_stations" => {
                let stations = crate::radio::get_radio_stations_saved(&self.store)
                    .map_err(ToolError::failed)?;
                json!({"stations":stations})
            }
            "search_radio_stations" => {
                let query = optional_str_arg(&a, "query", 120)?;
                let country = optional_str_arg(&a, "country", 80)?;
                let tag = optional_str_arg(&a, "tag", 80)?;
                if [query.as_deref(), country.as_deref(), tag.as_deref()]
                    .into_iter()
                    .all(|value| value.map_or(true, |value| value.trim().is_empty()))
                {
                    return Err(ToolError::bad("Provide a query, country, or tag"));
                }
                let limit = usize_arg(&a, "limit", 50)?.unwrap_or(20).clamp(1, 50);
                let stations = crate::radio::search_radio_stations(
                    query.unwrap_or_default(),
                    country,
                    tag,
                    Some(limit),
                )
                .await
                .map_err(ToolError::failed)?;
                json!({"stations":stations,"count":stations.len()})
            }
            "get_station_details" => {
                let station_id = str_arg(&a, "stationId", 120)?;
                let details = crate::radio::get_station_details(station_id, &self.store)
                    .await
                    .map_err(ToolError::failed)?;
                serde_json::to_value(details).map_err(|e| ToolError::failed(e.to_string()))?
            }
            "add_radio_station" => {
                let station: crate::radio::RadioStation = serde_json::from_value(
                    a.get("station")
                        .cloned()
                        .ok_or_else(|| ToolError::bad("station is required"))?,
                )
                .map_err(|e| ToolError::bad(format!("Invalid station: {e}")))?;
                let _guard = self.mutations.lock().await;
                let stations = crate::radio::add_radio_station(station, &self.store)
                    .map_err(ToolError::failed)?;
                let _ = self
                    .app
                    .emit("radio-stations-changed", json!({"stations":stations}));
                json!({"stations":stations})
            }
            "add_custom_radio_station" => {
                let name = str_arg(&a, "name", 160)?;
                let url = str_arg(&a, "url", 2048)?;
                let _guard = self.mutations.lock().await;
                let stations = crate::radio::add_custom_radio_station(name, url, &self.store)
                    .await
                    .map_err(ToolError::failed)?;
                let _ = self
                    .app
                    .emit("radio-stations-changed", json!({"stations":stations}));
                json!({"stations":stations})
            }
            "remove_radio_station" => {
                let station_id = str_arg(&a, "stationId", 120)?;
                let _guard = self.mutations.lock().await;
                let stations = crate::radio::remove_radio_station(station_id, &self.store)
                    .map_err(ToolError::failed)?;
                let _ = self
                    .app
                    .emit("radio-stations-changed", json!({"stations":stations}));
                json!({"stations":stations})
            }
            "rename_radio_station" => {
                let station_id = str_arg(&a, "stationId", 120)?;
                let name = str_arg(&a, "name", 160)?;
                let _guard = self.mutations.lock().await;
                let stations = crate::radio::rename_radio_station(station_id, name, &self.store)
                    .map_err(ToolError::failed)?;
                let _ = self
                    .app
                    .emit("radio-stations-changed", json!({"stations":stations}));
                json!({"stations":stations})
            }
            "play_radio_station" => {
                let station_id = str_arg(&a, "stationId", 120)?;
                let station = crate::radio::get_radio_stations_saved(&self.store)
                    .map_err(ToolError::failed)?
                    .into_iter()
                    .find(|station| station.id == station_id)
                    .ok_or_else(|| ToolError {
                        code: "NOT_FOUND",
                        message: "Radio station was not found".into(),
                    })?;
                let saved = self
                    .store
                    .save_track(&radio_station_track(&station))
                    .map_err(ToolError::failed)?;
                self.notify_library();
                self.playback("play", vec![saved.id], None, None, None)
                    .await?
            }
            "search_library" => {
                let q = str_arg(&a, "query", 500)?;
                let offset = usize_arg(&a, "offset", 100000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", MAX_PAGE)?.unwrap_or(25).max(1);
                let (rows, total) = self
                    .store
                    .search_library_tracks(&[q], limit, offset)
                    .map_err(ToolError::failed)?;
                json!({"items":rows.into_iter().map(safe_track).collect::<Vec<_>>(),"total":total,"offset":offset,"limit":limit,"hasMore":offset.saturating_add(limit)<total})
            }
            name if playback_history_tool(name) == Some(PlaybackHistoryTool::Search) => {
                let options = playback_history_search_options(&a)?;
                let page = self
                    .history
                    .search(&self.store, options)
                    .map_err(ToolError::failed)?;
                safe_playback_history_page(page)
            }
            name if playback_history_tool(name) == Some(PlaybackHistoryTool::Entry) => {
                let session_id = playback_history_session_id(&a)?;
                let entry = self
                    .history
                    .get_enriched(&self.store, &session_id)
                    .map_err(ToolError::failed)?
                    .ok_or_else(|| ToolError {
                        code: "NOT_FOUND",
                        message: "Playback history entry was not found".into(),
                    })?;
                safe_playback_history_entry(entry)
            }
            "get_track" => {
                let id = str_arg(&a, "trackId", 300)?;
                let track = required_track(&self.store, &id)?;
                json!({"track":safe_track(track)})
            }
            "get_song_radio" => {
                let track = required_track(&self.store, &str_arg(&a, "trackId", 300)?)?;
                let candidate: Candidate = serde_json::from_value(
                    a.get("candidate")
                        .cloned()
                        .ok_or_else(|| ToolError::bad("A YouTube seed candidate is required"))?,
                )
                .map_err(|e| ToolError::bad(e.to_string()))?;
                let mut result = crate::radio::start_song_radio(
                    &track,
                    &candidate,
                    &self.store.settings().map_err(ToolError::failed)?,
                    usize_arg(&a, "limit", 50)?.unwrap_or(25),
                )
                .await
                .map_err(ToolError::failed)?;
                result.tracks = result
                    .tracks
                    .iter()
                    .map(|item| self.store.save_track(item).map_err(ToolError::failed))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                json!(result)
            }
            "get_app_status" => self.status(),
            "library_stats" => {
                let tracks = self
                    .store
                    .list_tracks("library", None)
                    .map_err(ToolError::failed)?;
                let assets = self.store.list_assets().map_err(ToolError::failed)?;
                json!({"tracks":tracks.len(),"assets":assets.len(),"playlists":self.store.list_collections().map_err(ToolError::failed)?.len()})
            }
            "list_playlists" => {
                let offset = usize_arg(&a, "offset", 100000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", MAX_PAGE)?.unwrap_or(25).max(1);
                let collections = self.store.list_collections().map_err(ToolError::failed)?;
                let imports = self
                    .store
                    .list_browser_imports()
                    .map_err(ToolError::failed)?;
                let total = collections.len();
                let page = collections
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .collect::<Vec<_>>();
                let mut v = Vec::new();
                for c in page {
                    let snaps = imports
                        .iter()
                        .filter(|s| s.collection_id == c.id)
                        .collect::<Vec<_>>();
                    v.push(json!({"playlist":c,"browserSource":self.store.is_browser_source_collection(&c.id).unwrap_or(false),"latestFetchedAt":snaps.first().map(|s|s.fetched_at.clone()),"sourceProvider":snaps.first().map(|s|s.provider.clone()),"sourceUrl":snaps.first().map(|s|s.source_url.clone()),"latestSnapshotId":snaps.first().map(|s|s.id.clone()),"entryCount":snaps.first().map(|s|s.captured_count),"versions":snaps.len()}));
                }
                json!({"items":v,"offset":offset,"limit":limit,"total":total,"hasMore":offset.saturating_add(limit)<total})
            }
            "get_playlist" => {
                let id = str_arg(&a, "playlistId", 300)?;
                let offset = usize_arg(&a, "offset", 100000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", MAX_PAGE)?.unwrap_or(25).max(1);
                let collection = self
                    .store
                    .list_collections()
                    .map_err(ToolError::failed)?
                    .into_iter()
                    .find(|p| p.id == id)
                    .ok_or_else(|| ToolError {
                        code: "NOT_FOUND",
                        message: "Playlist was not found".into(),
                    })?;
                if self
                    .store
                    .is_browser_source_collection(&id)
                    .map_err(ToolError::failed)?
                {
                    let imports = self
                        .store
                        .list_browser_imports()
                        .map_err(ToolError::failed)?
                        .into_iter()
                        .filter(|s| s.collection_id == id)
                        .collect::<Vec<_>>();
                    let snapshot_id = a
                        .get("snapshotId")
                        .map(|v| {
                            v.as_str()
                                .ok_or_else(|| ToolError::bad("snapshotId must be a string"))
                        })
                        .transpose()?;
                    let info = if let Some(snapshot_id) = snapshot_id {
                        imports.iter().find(|s| s.id == snapshot_id)
                    } else {
                        imports.first()
                    }
                    .ok_or_else(|| ToolError {
                        code: "NOT_FOUND",
                        message: "Browser playlist snapshot was not found".into(),
                    })?;
                    let chart = self
                        .store
                        .load_browser_import(&info.id)
                        .map_err(ToolError::failed)?;
                    let total = chart.tracks.len();
                    let tracks = chart
                        .tracks
                        .into_iter()
                        .skip(offset)
                        .take(limit)
                        .collect::<Vec<_>>();
                    let versions=imports.iter().map(|v|json!({"snapshotId":v.id,"fetchedAt":v.fetched_at,"title":v.title,"complete":v.complete})).collect::<Vec<_>>();
                    json!({"playlist":collection,"snapshot":{"snapshotId":info.id,"fetchedAt":info.fetched_at,"title":info.title,"sourceUrl":info.source_url,"complete":info.complete},"versions":versions,"tracks":tracks.into_iter().map(safe_track).collect::<Vec<_>>(),"offset":offset,"limit":limit,"total":total,"hasMore":offset.saturating_add(limit)<total})
                } else {
                    let all = self
                        .store
                        .list_tracks("collection", Some(&id))
                        .map_err(ToolError::failed)?;
                    let total = all.len();
                    let tracks = all.into_iter().skip(offset).take(limit).collect::<Vec<_>>();
                    json!({"playlist":collection,"tracks":tracks.into_iter().map(safe_track).collect::<Vec<_>>(),"offset":offset,"limit":limit,"total":total,"hasMore":offset.saturating_add(limit)<total})
                }
            }
            "search_tracks" => {
                let query = str_arg(&a, "query", 300)?;
                let ids = if a.get("providers").is_some() {
                    string_array(&a, "providers", 20)?
                } else {
                    vec![]
                };
                if ids.iter().any(|id| {
                    ![
                        "library",
                        "musicbrainz",
                        "youtube",
                        "soundcloud",
                        "soulseek",
                        "monochrome",
                    ]
                    .contains(&id.as_str())
                }) {
                    return Err(ToolError::bad(
                        "Choose a searchable track source or library",
                    ));
                }
                let discovery = DiscoveryService::new(
                    self.artists.clone(),
                    self.acquisition.clone(),
                    self.store.clone(),
                    self.connections.clone(),
                );
                let pages = discovery
                    .async_shared_search(DiscoveryRequest {
                        query,
                        source_ids: ids,
                        kinds: vec!["track".into()],
                        cursor: None,
                        limit: Some(50),
                        isrc: None,
                    })
                    .await;
                let mut found = Vec::new();
                let mut seen = std::collections::HashSet::new();
                let mut warnings = Vec::new();
                for page in &pages {
                    warnings.extend(page.warnings.clone());
                    for result in &page.results {
                        if let Some(track) = &result.track {
                            if seen.insert(track.id.clone()) && found.len() < 100 {
                                found
                                    .push(self.store.save_track(track).map_err(ToolError::failed)?);
                            }
                        }
                    }
                }
                json!({"items":found.into_iter().map(safe_track).collect::<Vec<_>>(),"warnings":warnings,"pages":pages,"partial":pages.iter().any(|page|!page.complete)})
            }
            "search_artists" => {
                let q = str_arg(&a, "query", 120)?;
                let creds = self.artist_credentials()?;
                let page = usize_arg(&a, "page", 200)?.unwrap_or(1) as u32;
                let found = self
                    .artists
                    .search_artists(&q, page, 25, &creds)
                    .await
                    .map_err(ToolError::failed)?;
                json!(found)
            }
            "get_artist_discography" => {
                let artist = artist_arg(&a)?;
                let cursor = cursor_arg(&a)?;
                let found = self
                    .artists
                    .get_artist_releases(&artist, cursor, &self.artist_credentials()?)
                    .await
                    .map_err(ToolError::failed)?;
                json!(found)
            }
            "get_release_tracks" => {
                let artist = artist_arg(&a)?;
                let release = release_arg(&a)?;
                let tracks = self
                    .artists
                    .get_release_tracks(&release, &artist, &self.artist_credentials()?)
                    .await
                    .map_err(ToolError::failed)?;
                let mut saved = Vec::with_capacity(tracks.len());
                for track in tracks {
                    let track = self.store.save_track(&track).map_err(ToolError::failed)?;
                    saved.push(safe_track(track));
                }
                let total = saved.len();
                json!({"tracks":saved,"total":total})
            }
            "get_artist_tracks" => {
                let artist = artist_arg(&a)?;
                let mut page = self
                    .artists
                    .get_artist_tracks(&artist, cursor_arg(&a)?, &self.artist_credentials()?)
                    .await
                    .map_err(ToolError::failed)?;
                let mut saved = Vec::new();
                for mut track in page.items.drain(..) {
                    match self.store.save_track(&track) {
                        Ok(value) => {
                            track = value;
                            saved.push(safe_track(track));
                        }
                        Err(e) => page
                            .warnings
                            .push(format!("A recording could not be saved: {e}")),
                    }
                }
                json!({"artist":page.artist,"items":saved,"nextCursor":page.next_cursor,"source":page.source,"warnings":page.warnings})
            }
            "get_artist_collaborations" => {
                let artist = artist_arg(&a)?;
                let page = self
                    .artists
                    .get_artist_collaborations(
                        &artist,
                        cursor_arg(&a)?,
                        &self.artist_credentials()?,
                    )
                    .await
                    .map_err(ToolError::failed)?;
                json!(page)
            }
            "get_artist_profile" => {
                let artist = artist_arg(&a)?;
                let mut profile = self
                    .artists
                    .get_artist_profile(&artist, &self.artist_credentials()?)
                    .await
                    .map_err(ToolError::failed)?;
                let mut saved = Vec::new();
                for mut track in profile.top_tracks.drain(..) {
                    match self.store.save_track(&track) {
                        Ok(value) => {
                            track = value;
                            saved.push(safe_track(track));
                        }
                        Err(e) => profile
                            .warnings
                            .push(format!("A top track could not be saved: {e}")),
                    }
                }
                profile.top_tracks = Vec::new();
                let mut value =
                    serde_json::to_value(profile).map_err(|e| ToolError::failed(e.to_string()))?;
                value["topTracks"] = json!(saved);
                value
            }
            "create_playlist" => {
                let _guard = self.mutations.lock().await;
                let name = str_arg(&a, "name", 120)?;
                let key = a
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !key.is_empty() {
                    if key.len() > 200 {
                        return Err(ToolError::bad("idempotencyKey too long"));
                    }
                    let mut db = self
                        .store
                        .get_json_preference(IDEMPOTENCY_KEY)
                        .map_err(ToolError::failed)?
                        .unwrap_or_else(|| json!({}));
                    if !db.is_object() {
                        db = json!({});
                    }
                    if let Some(id) =
                        idempotent_id(&db, key, "create_playlist", &json!({"name":name}))?
                    {
                        return Ok(json!({"playlistId":id,"reused":true}));
                    }
                    let c = self
                        .store
                        .create_collection(&name)
                        .map_err(ToolError::failed)?;
                    db[key] =
                        json!({"operation":"create_playlist","signature":{"name":name},"id":c.id});
                    self.store
                        .set_json_preference(IDEMPOTENCY_KEY, &db)
                        .map_err(ToolError::failed)?;
                    self.notify_library();
                    json!({"playlistId":c.id,"reused":false})
                } else {
                    let c = self
                        .store
                        .create_collection(&name)
                        .map_err(ToolError::failed)?;
                    self.notify_library();
                    json!({"playlistId":c.id,"reused":false})
                }
            }
            "add_tracks_to_playlist" => {
                let _guard = self.mutations.lock().await;
                let id = str_arg(&a, "playlistId", 300)?;
                let mut ids = string_array(&a, "trackIds", 500)?;
                let mut seen = std::collections::HashSet::new();
                ids.retain(|v| seen.insert(v.clone()));
                let key = a
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let signature = json!({"playlistId":id,"trackIds":ids});
                let mut db = self
                    .store
                    .get_json_preference(IDEMPOTENCY_KEY)
                    .map_err(ToolError::failed)?
                    .unwrap_or_else(|| json!({}));
                if !db.is_object() {
                    db = json!({});
                }
                if !key.is_empty() {
                    if key.len() > 200 {
                        return Err(ToolError::bad("idempotencyKey too long"));
                    }
                    if let Some(playlist_id) =
                        idempotent_id(&db, key, "add_tracks_to_playlist", &signature)?
                    {
                        return Ok(json!({"playlistId":playlist_id,"reused":true}));
                    }
                }
                let original = self
                    .store
                    .list_collections()
                    .map_err(ToolError::failed)?
                    .into_iter()
                    .find(|p| p.id == id)
                    .ok_or_else(|| ToolError {
                        code: "NOT_FOUND",
                        message: "Playlist was not found".into(),
                    })?;
                let mut ts = Vec::new();
                for tid in ids {
                    ts.push(
                        self.store
                            .get_track(&tid)
                            .map_err(ToolError::failed)?
                            .ok_or_else(|| ToolError {
                                code: "NOT_FOUND",
                                message: format!("Track was not found: {tid}"),
                            })?,
                    );
                }
                let target_id = if self
                    .store
                    .is_browser_source_collection(&id)
                    .map_err(ToolError::failed)?
                {
                    let copied = self
                        .store
                        .create_collection(&format!("{} (Copy)", original.name))
                        .map_err(ToolError::failed)?;
                    let snapshot = self
                        .store
                        .list_browser_imports()
                        .map_err(ToolError::failed)?
                        .into_iter()
                        .find(|i| i.collection_id == id)
                        .ok_or_else(|| ToolError {
                            code: "NOT_FOUND",
                            message: "No saved browser playlist snapshot was found".into(),
                        })?;
                    let existing = self
                        .store
                        .load_browser_import(&snapshot.id)
                        .map_err(ToolError::failed)?
                        .tracks;
                    self.store
                        .replace_playlist_entries(&copied.id, &existing)
                        .map_err(ToolError::failed)?;
                    copied.id
                } else {
                    id.clone()
                };
                let before = self
                    .store
                    .list_tracks("collection", Some(&target_id))
                    .map_err(ToolError::failed)?
                    .len();
                self.store
                    .add_to_collection(&target_id, &ts)
                    .map_err(ToolError::failed)?;
                crate::playlist_preferences::resync_favorite_flags_after_collection_change(
                    &self.store,
                    &target_id,
                )
                .map_err(ToolError::failed)?;
                let added = self
                    .store
                    .list_tracks("collection", Some(&target_id))
                    .map_err(ToolError::failed)?
                    .len()
                    .saturating_sub(before);
                if !key.is_empty() {
                    db[key] = json!({"operation":"add_tracks_to_playlist","signature":signature,"id":target_id});
                    self.store
                        .set_json_preference(IDEMPOTENCY_KEY, &db)
                        .map_err(ToolError::failed)?;
                }
                self.notify_library();
                json!({"added":added,"playlistId":target_id,"copied":target_id!=id,"reused":false})
            }
            "playback_state" => self
                .playback_state
                .lock()
                .map_err(|_| ToolError::failed("Player state unavailable"))?
                .clone(),
            "play_tracks" => {
                let ids = string_array(&a, "trackIds", 500)?;
                self.playback(
                    "play",
                    ids,
                    a.get("mode").and_then(Value::as_str).map(str::to_owned),
                    None,
                    None,
                )
                .await?
            }
            "pause_playback" => self.playback("pause", vec![], None, None, None).await?,
            "resume_playback" => self.playback("resume", vec![], None, None, None).await?,
            "queue_tracks" => {
                self.playback(
                    "queue",
                    string_array(&a, "trackIds", 500)?,
                    queue_mode(&a)?,
                    None,
                    None,
                )
                .await?
            }
            "seek_playback" => {
                let n = a
                    .get("position")
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && *v >= 0.)
                    .ok_or_else(|| ToolError::bad("position must be nonnegative seconds"))?;
                self.playback("seek", vec![], None, Some(n), None).await?
            }
            "set_volume" => {
                let n = a
                    .get("volume")
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && (0. ..=1.).contains(v))
                    .ok_or_else(|| ToolError::bad("volume must be from 0 to 1"))?;
                self.playback("volume", vec![], None, None, Some(n)).await?
            }
            "search_audio_sources" => {
                let providers = string_array(&a, "providers", 10)?;
                if let Some(track_id) = a.get("trackId").and_then(Value::as_str) {
                    let track = required_track(&self.store, track_id)?;
                    let candidates = crate::acquisition::find_candidates(
                        &track,
                        &providers,
                        &self.store.settings().map_err(ToolError::failed)?,
                    )
                    .await
                    .map_err(ToolError::failed)?;
                    json!({"items":candidates})
                } else {
                    let mut tracks = self
                        .acquisition
                        .search_audio_tracks(
                            &str_arg(&a, "query", 500)?,
                            &providers,
                            &self.store.settings().map_err(ToolError::failed)?,
                        )
                        .await
                        .map_err(ToolError::failed)?;
                    let mut warnings = Vec::new();
                    for track in &mut tracks {
                        match self.store.save_track(track) {
                            Ok(saved) => *track = saved,
                            Err(e) => warnings.push(e),
                        }
                    }
                    tracks.truncate(100);
                    json!({"items":tracks.into_iter().map(safe_track).collect::<Vec<_>>(),"warnings":warnings})
                }
            }
            "inspect_audio_source" => {
                let track = required_track(&self.store, &str_arg(&a, "trackId", 300)?)?;
                let candidate: Candidate = serde_json::from_value(
                    a.get("candidate")
                        .cloned()
                        .ok_or_else(|| ToolError::bad("An explicit candidate is required"))?,
                )
                .map_err(|e| ToolError::bad(e.to_string()))?;
                let inspected = self
                    .acquisition
                    .inspect_formats(
                        &track,
                        &candidate,
                        &self.store.settings().map_err(ToolError::failed)?,
                    )
                    .await
                    .map_err(ToolError::failed)?;
                json!({"candidate":inspected})
            }
            "start_slskd_search" => {
                json!({"searchId":self.acquisition.start_slskd_search(&str_arg(&a,"query",500)?).await.map_err(ToolError::failed)?})
            }
            "get_slskd_search" => json!(self
                .acquisition
                .poll_slskd_search(&str_arg(&a, "searchId", 300)?)
                .await
                .map_err(ToolError::failed)?),
            "cancel_slskd_search" => {
                self.acquisition
                    .cancel_slskd_search(&str_arg(&a, "searchId", 300)?)
                    .await
                    .map_err(ToolError::failed)?;
                json!({"cancelled":true})
            }
            "download_candidate" => {
                let track_id = str_arg(&a, "trackId", 300)?;
                let track = self
                    .store
                    .get_track(&track_id)
                    .map_err(ToolError::failed)?
                    .ok_or_else(|| ToolError {
                        code: "NOT_FOUND",
                        message: "Track was not found; add it to the library first".into(),
                    })?;
                let candidate: Candidate = serde_json::from_value(
                    a.get("candidate")
                        .cloned()
                        .ok_or_else(|| ToolError::bad("An explicit candidate is required"))?,
                )
                .map_err(|e| ToolError::bad(e.to_string()))?;
                let _guard = self.mutations.lock().await;
                let key = a
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let mut db = self
                    .store
                    .get_json_preference(IDEMPOTENCY_KEY)
                    .map_err(ToolError::failed)?
                    .unwrap_or_else(|| json!({}));
                if !db.is_object() {
                    db = json!({});
                }
                if !key.is_empty() {
                    if key.len() > 200 {
                        return Err(ToolError::bad("idempotencyKey too long"));
                    }
                    let signature = json!({"trackId":track.id,"candidateId":candidate.id,"formatId":a.get("formatId"),"outputProfile":a.get("outputProfile"),"chapterIds":a.get("chapterIds")});
                    if let Some(job_id) = idempotent_id(&db, key, "download_candidate", &signature)?
                    {
                        let job = self.store.get_job(&job_id).map_err(ToolError::failed)?;
                        return Ok(json!({"job":safe_job(job),"reused":true}));
                    }
                }
                let format_id = a
                    .get("formatId")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| ToolError::bad("formatId must be a string"))
                    })
                    .transpose()?;
                let chapter_ids = a
                    .get("chapterIds")
                    .map(|_| string_array(&a, "chapterIds", 500))
                    .transpose()?;
                let req = DownloadRequest {
                    track,
                    candidate,
                    format_id,
                    output_profile: a
                        .get("outputProfile")
                        .and_then(Value::as_str)
                        .unwrap_or("source")
                        .into(),
                    chapter_ids,
                };
                let job = self
                    .acquisition
                    .enqueue(req, self.store.settings().map_err(ToolError::failed)?)
                    .await
                    .map_err(ToolError::failed)?;
                if !key.is_empty() {
                    db[key] = json!({"operation":"download_candidate","signature":{"trackId":job.track.id,"candidateId":job.candidate.id,"formatId":job.format_id,"outputProfile":job.output_profile,"chapterIds":job.chapter_ids},"id":job.id});
                    self.store
                        .set_json_preference(IDEMPOTENCY_KEY, &db)
                        .map_err(ToolError::failed)?;
                }
                json!({"job":safe_job(job),"reused":false})
            }
            "download_jobs" => {
                let offset = usize_arg(&a, "offset", 100000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", MAX_PAGE)?.unwrap_or(25).max(1);
                let all = self.store.list_jobs().map_err(ToolError::failed)?;
                let total = all.len();
                json!({"items":all.into_iter().skip(offset).take(limit).map(safe_job).collect::<Vec<_>>(),"offset":offset,"limit":limit,"total":total,"hasMore":offset.saturating_add(limit)<total})
            }
            "retry_download" => {
                let job = self
                    .acquisition
                    .retry(
                        &str_arg(&a, "jobId", 300)?,
                        self.store.settings().map_err(ToolError::failed)?,
                    )
                    .await
                    .map_err(ToolError::failed)?;
                json!({"job":safe_job(job)})
            }
            "cancel_download" => {
                self.acquisition
                    .cancel(&str_arg(&a, "jobId", 300)?)
                    .await
                    .map_err(ToolError::failed)?;
                json!({"cancelled":true})
            }
            "analyze_library" => {
                let ids = a
                    .get("assetIds")
                    .map(|_| string_array(&a, "assetIds", 1000))
                    .transpose()?;
                let force = a.get("force").and_then(Value::as_bool).unwrap_or(false);
                json!({"queued":self.analysis.queue(ids,force).map_err(ToolError::failed)?})
            }
            "analysis_status" => {
                let offset = usize_arg(&a, "offset", 100000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", MAX_PAGE)?.unwrap_or(25).max(1);
                let all = self.analysis.status().map_err(ToolError::failed)?;
                let total = all.len();
                json!({"items":all.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"offset":offset,"limit":limit,"total":total,"hasMore":offset.saturating_add(limit)<total})
            }
            "export_mixxx_playlist" => {
                let collection_id = str_arg(&a, "playlistId", 300)?;
                let result = crate::playlist_sync::export_mixxx_playlist(
                    &self.store,
                    &self.store.settings().map_err(ToolError::failed)?,
                    &collection_id,
                )
                .map_err(ToolError::failed)?;
                json!({"path":result.path,"exported":result.exported,"missing":result.missing,"total":result.total})
            }
            _ => {
                return Err(ToolError {
                    code: "UNKNOWN_TOOL",
                    message: format!("Unknown tool: {name}"),
                });
            }
        };
        Ok(result)
    }
    fn artist_credentials(&self) -> Result<ArtistCredentials, ToolError> {
        let settings = self.connections.settings().map_err(ToolError::failed)?;
        let mut warnings = Vec::new();
        let discogs_token = if settings.discogs_enabled {
            self.connections
                .credential("discogs")
                .map_err(ToolError::failed)?
        } else {
            None
        };
        let lastfm_api_key = if settings.lastfm_enabled {
            self.connections
                .credential("lastfm")
                .map_err(ToolError::failed)?
        } else {
            None
        };
        if settings.discogs_enabled && discogs_token.is_none() {
            warnings.push("Discogs credential unavailable".into());
        }
        if settings.lastfm_enabled && lastfm_api_key.is_none() {
            warnings.push("Last.fm credential unavailable".into());
        }
        Ok(ArtistCredentials {
            discogs_token,
            lastfm_api_key,
            warnings,
        })
    }
    fn notify_library(&self) {
        let _ = self.app.emit(
            "automation-library-changed",
            json!({"at":chrono::Utc::now().to_rfc3339()}),
        );
    }
    fn autotag_manager(&self) -> Result<Arc<crate::autotag::AutoTagger>, ToolError> {
        self.autotag
            .lock()
            .map_err(|_| ToolError::failed("Autotag manager unavailable"))?
            .clone()
            .ok_or_else(|| ToolError::failed("Autotag manager is not configured"))
    }
}

pub fn registered_chapter_id(parent_track_id: &str, chapter_id: &str) -> String {
    format!("chapter:{parent_track_id}:{chapter_id}")
}

fn valid_chapter(chapter: &Chapter, duration: Option<f64>) -> bool {
    chapter.start_seconds.is_finite()
        && chapter.end_seconds.is_finite()
        && chapter.start_seconds >= 0.0
        && chapter.end_seconds > chapter.start_seconds
        && duration.map_or(true, |length| {
            !length.is_finite() || chapter.end_seconds <= length
        })
}

fn validate_public_candidate(candidate: &Candidate) -> Result<(), ToolError> {
    if !matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
        return Err(ToolError::bad(
            "Chapter inspection supports YouTube and SoundCloud candidates",
        ));
    }
    let url =
        url::Url::parse(&candidate.url).map_err(|_| ToolError::bad("Candidate URL is invalid"))?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = if candidate.provider == "youtube" {
        [
            "youtube.com",
            "www.youtube.com",
            "m.youtube.com",
            "youtu.be",
            "music.youtube.com",
        ]
        .contains(&host.as_str())
    } else {
        [
            "soundcloud.com",
            "www.soundcloud.com",
            "m.soundcloud.com",
            "on.soundcloud.com",
            "api.soundcloud.com",
        ]
        .contains(&host.as_str())
    };
    if url.scheme() != "https" || !allowed || !url.username().is_empty() || url.password().is_some()
    {
        return Err(ToolError::bad(
            "Candidate URL must be a public HTTPS URL for its selected provider",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackHistoryTool {
    Search,
    Entry,
}
fn playback_history_tool(name: &str) -> Option<PlaybackHistoryTool> {
    match name {
        "search_playback_history" | "list_playback_history" => Some(PlaybackHistoryTool::Search),
        "get_playback_history_entry" => Some(PlaybackHistoryTool::Entry),
        _ => None,
    }
}

fn tool_scope(name: &str) -> Option<&'static str> {
    Some(match name {
        "search_library"
        | "get_track"
        | "library_stats"
        | "list_playlists"
        | "get_playlist"
        | "search_tracks"
        | "search_artists"
        | "get_artist_discography"
        | "get_release_tracks"
        | "get_artist_tracks"
        | "get_artist_collaborations"
        | "get_artist_profile"
        | "get_song_radio"
        | "list_radio_stations"
        | "search_radio_stations"
        | "get_station_details"
        | "get_app_status"
        | "discover_music"
        | "poll_music_search"
        | "inspect_source_chapters"
        | "get_autotag_status"
        | "search_audio_sources"
        | "inspect_audio_source"
        | "download_jobs"
        | "analysis_status"
        | "search_playback_history"
        | "list_playback_history"
        | "get_playback_history_entry"
        | "playback_state"
        | "get_slskd_search"
        | "start_slskd_search"
        | "cancel_slskd_search" => "read",
        "play_tracks"
        | "pause_playback"
        | "resume_playback"
        | "queue_tracks"
        | "seek_playback"
        | "set_volume"
        | "create_playlist"
        | "add_tracks_to_playlist" => "control",
        "play_chapter" => "control",
        "play_radio_station" => "control",
        "add_radio_station"
        | "add_custom_radio_station"
        | "remove_radio_station"
        | "rename_radio_station" => "control",
        "download_candidate"
        | "retry_download"
        | "cancel_download"
        | "analyze_library"
        | "retry_autotag"
        | "review_autotag_candidate"
        | "apply_autotag_review"
        | "export_mixxx_playlist" => "manage",
        _ => return None,
    })
}
fn queue_mode(arguments: &Value) -> Result<Option<String>, ToolError> {
    arguments
        .get("mode")
        .map(|value| {
            value
                .as_str()
                .filter(|mode| matches!(*mode, "append" | "replace" | "next"))
                .map(str::to_owned)
                .ok_or_else(|| ToolError::bad("mode must be append, replace, or next"))
        })
        .transpose()
}
fn str_arg(a: &Value, k: &str, max: usize) -> Result<String, ToolError> {
    let s = a
        .get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::bad(format!("{k} is required")))?
        .trim();
    if s.is_empty() || s.chars().count() > max {
        return Err(ToolError::bad(format!(
            "{k} must contain 1–{max} characters"
        )));
    }
    Ok(s.to_owned())
}
fn optional_str_arg(a: &Value, k: &str, max: usize) -> Result<Option<String>, ToolError> {
    let Some(value) = a.get(k) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| ToolError::bad(format!("{k} must be text")))?
        .trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max {
        return Err(ToolError::bad(format!(
            "{k} must be at most {max} characters"
        )));
    }
    Ok(Some(value.to_owned()))
}
fn radio_station_track(station: &crate::radio::RadioStation) -> Track {
    let is_radio_paradise = station.provider.as_deref() == Some("radio_paradise");
    Track {
        id: format!("radio:{}", station.id),
        provider: "radio".into(),
        provider_id: station.id.clone(),
        source_url: station
            .homepage_url
            .clone()
            .unwrap_or_else(|| station.source_url.clone()),
        artist: if is_radio_paradise {
            "Radio Paradise".into()
        } else {
            "Internet Radio".into()
        },
        title: station.name.clone(),
        genre: station.tags.clone(),
        artwork_url: station.favicon_url.clone(),
        provenance: json!({
            "sourceKind":"radio",
            "stationId":station.id,
            "stationName":station.name,
            "stationSourceUrl":station.source_url,
            "radioStreamUrl":station.stream_url,
            "radioProvider":station.provider,
            "country":station.country,
            "tags":station.tags,
            "homepageUrl":station.homepage_url,
            "codec":station.codec,
            "bitrateKbps":station.bitrate_kbps
        }),
        ..Track::default()
    }
}
fn artist_arg(a: &Value) -> Result<ArtistRef, ToolError> {
    let value = a
        .get("artist")
        .cloned()
        .ok_or_else(|| ToolError::bad("artist is required"))?;
    let artist: ArtistRef =
        serde_json::from_value(value).map_err(|e| ToolError::bad(e.to_string()))?;
    if !matches!(artist.provider.as_str(), "musicbrainz" | "discogs")
        || artist.id.is_empty()
        || artist.id.len() > 128
    {
        return Err(ToolError::bad(
            "artist must contain a supported provider and ID",
        ));
    }
    Ok(artist)
}
fn release_arg(a: &Value) -> Result<ArtistReleaseRef, ToolError> {
    let value = a
        .get("release")
        .cloned()
        .ok_or_else(|| ToolError::bad("release is required"))?;
    let release: ArtistReleaseRef =
        serde_json::from_value(value).map_err(|e| ToolError::bad(e.to_string()))?;
    if !matches!(release.release.provider.as_str(), "musicbrainz" | "discogs")
        || release.release.id.is_empty()
        || release.release.id.len() > 128
        || release.title.trim().is_empty()
        || release.title.chars().count() > 500
    {
        return Err(ToolError::bad(
            "release must contain a supported provider, ID, and title",
        ));
    }
    Ok(release)
}
fn artist_ref_schema() -> Value {
    json!({
        "type":"object",
        "properties":{
            "provider":{"type":"string","enum":["musicbrainz","discogs"]},
            "id":{"type":"string"}
        },
        "required":["provider","id"],
        "additionalProperties":false
    })
}
fn release_ref_schema() -> Value {
    json!({
        "type":"object",
        "properties":{
            "release":artist_ref_schema(),
            "title":{"type":"string"},
            "year":{"type":["integer","null"]},
            "releaseDate":{"type":["string","null"]},
            "country":{"type":["string","null"]},
            "label":{"type":["string","null"]},
            "catalogNumber":{"type":["string","null"]},
            "format":{"type":["string","null"]},
            "primaryType":{"type":["string","null"]},
            "trackCount":{"type":["integer","null"]},
            "tracks":{"type":"array","items":{"type":"object","properties":{
                "position":{"type":["string","null"]},"title":{"type":"string"},"sourceUrl":{"type":"string"}
            },"required":["position","title","sourceUrl"],"additionalProperties":false}},
            "sourceUrl":{"type":"string"},
            "credits":{"type":"array","items":{"type":"object","properties":{
                "name":{"type":"string"},"artist":{"anyOf":[artist_ref_schema(),{"type":"null"}]},
                "role":{"type":"string"},"creditedAs":{"type":["string","null"]},
                "joinPhrase":{"type":["string","null"]},"sourceUrl":{"type":"string"}
            },"required":["name","artist","role","creditedAs","joinPhrase","sourceUrl"],"additionalProperties":false}}
        },
        "required":["release","title","year","releaseDate","country","label","catalogNumber","format","primaryType","trackCount","tracks","sourceUrl","credits"],
        "additionalProperties":false
    })
}
fn cursor_arg(a: &Value) -> Result<Option<&str>, ToolError> {
    a.get("cursor")
        .map(|value| {
            value
                .as_str()
                .filter(|s| s.len() <= 100)
                .ok_or_else(|| ToolError::bad("cursor must be a string of at most 100 characters"))
        })
        .transpose()
}
fn usize_arg(a: &Value, k: &str, max: usize) -> Result<Option<usize>, ToolError> {
    a.get(k)
        .map(|v| {
            v.as_u64()
                .filter(|n| *n <= max as u64)
                .map(|n| n as usize)
                .ok_or_else(|| ToolError::bad(format!("{k} is out of range")))
        })
        .transpose()
}
fn string_array(a: &Value, k: &str, max: usize) -> Result<Vec<String>, ToolError> {
    let v = a
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::bad(format!("{k} must be an array")))?;
    if v.len() > max {
        return Err(ToolError::bad(format!("{k} has too many items")));
    }
    v.iter()
        .map(|x| {
            x.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 500)
                .map(str::to_owned)
                .ok_or_else(|| ToolError::bad(format!("Invalid value in {k}")))
        })
        .collect()
}
fn idempotent_id(
    db: &Value,
    key: &str,
    operation: &str,
    signature: &Value,
) -> Result<Option<String>, ToolError> {
    let Some(value) = db.get(key) else {
        return Ok(None);
    };
    if let Some(id) = value.as_str() {
        return Ok(Some(id.to_owned()));
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::failed("Saved idempotency record is invalid"))?;
    if value.get("operation").and_then(Value::as_str) != Some(operation)
        || value.get("signature") != Some(signature)
    {
        return Err(ToolError {
            code: "IDEMPOTENCY_CONFLICT",
            message: "Idempotency key was already used for a different request".into(),
        });
    }
    Ok(Some(id.to_owned()))
}
fn required_track(store: &Store, id: &str) -> Result<Track, ToolError> {
    store
        .get_track(id)
        .map_err(ToolError::failed)?
        .ok_or_else(|| ToolError {
            code: "NOT_FOUND",
            message: "Track was not found".into(),
        })
}
fn safe_track(track: Track) -> Value {
    let mut value = serde_json::to_value(track).unwrap_or_else(|_| json!({}));
    strip_file_fields(&mut value);
    value
}
fn safe_playback_history_entry(entry: crate::playback_history::PlaybackHistoryEntry) -> Value {
    let mut value = serde_json::to_value(entry).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        if let Some(track) = object.remove("track") {
            let track: Track = serde_json::from_value(track).unwrap_or_default();
            object.insert("track".into(), safe_track(track));
        }
    }
    strip_file_fields(&mut value);
    value
}
fn safe_playback_history_page(page: impl Serialize) -> Value {
    let mut value = serde_json::to_value(page).unwrap_or_else(|_| json!({}));
    if let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) {
        for item in items {
            strip_file_fields(item);
            if let Some(object) = item.as_object_mut() {
                if let Some(track) = object.remove("track") {
                    let track: Track = serde_json::from_value(track).unwrap_or_default();
                    object.insert("track".into(), safe_track(track));
                }
            }
        }
    }
    value
}
fn playback_history_search_options(
    arguments: &Value,
) -> Result<PlaybackHistorySearchOptions, ToolError> {
    let empty = Value::Object(serde_json::Map::new());
    let arguments = if arguments.is_null() {
        &empty
    } else {
        arguments
    };
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("arguments must be an object"))?;
    const ALLOWED: &[&str] = &[
        "query",
        "provider",
        "stationId",
        "from",
        "to",
        "sort",
        "limit",
        "offset",
    ];
    if object.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(ToolError::bad("Unsupported playback history argument"));
    }
    let query = optional_str_arg(arguments, "query", 300)?;
    let provider = optional_str_arg(arguments, "provider", 120)?;
    let station_id = optional_str_arg(arguments, "stationId", 120)?;
    if provider
        .as_deref()
        .is_some_and(|provider| provider.chars().any(char::is_control))
    {
        return Err(ToolError::bad("provider contains invalid characters"));
    }
    let from = optional_str_arg(arguments, "from", 64)?;
    let to = optional_str_arg(arguments, "to", 64)?;
    for (name, value) in [("from", &from), ("to", &to)] {
        if value
            .as_ref()
            .is_some_and(|value| !valid_history_date(value))
        {
            return Err(ToolError::bad(format!(
                "{name} must be YYYY-MM-DD or an RFC3339 timestamp"
            )));
        }
    }
    if let (Some(from), Some(to)) = (&from, &to) {
        if history_bound_millis(from, false) > history_bound_millis(to, true) {
            return Err(ToolError::bad("from must not be later than to"));
        }
    }
    let sort = optional_str_arg(arguments, "sort", 16)?;
    if sort
        .as_deref()
        .is_some_and(|sort| !matches!(sort, "newest" | "oldest"))
    {
        return Err(ToolError::bad("sort must be newest or oldest"));
    }
    let limit = usize_arg(arguments, "limit", 100)?.unwrap_or(25);
    if limit == 0 {
        return Err(ToolError::bad("limit must be at least 1"));
    }
    let limit = limit as u32;
    let offset = usize_arg(arguments, "offset", 1_000_000)?.unwrap_or(0) as u32;
    Ok(PlaybackHistorySearchOptions {
        query,
        provider,
        station_id,
        from,
        to,
        sort,
        limit,
        offset,
    })
}
fn valid_history_date(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
        || chrono::DateTime::parse_from_rfc3339(value).is_ok()
}
fn history_bound_millis(value: &str, upper: bool) -> i64 {
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let time = if upper {
            chrono::NaiveTime::from_hms_milli_opt(23, 59, 59, 999).unwrap()
        } else {
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
        };
        date.and_time(time).and_utc().timestamp_millis()
    } else {
        chrono::DateTime::parse_from_rfc3339(value)
            .expect("bound validated before comparison")
            .timestamp_millis()
    }
}
fn playback_history_session_id(arguments: &Value) -> Result<String, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("arguments must be an object"))?;
    if object.keys().any(|key| key != "sessionId") {
        return Err(ToolError::bad("Unsupported playback history argument"));
    }
    str_arg(arguments, "sessionId", 300)
}
fn safe_job(job: DownloadJob) -> Value {
    let mut value = serde_json::to_value(job).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.remove("outputPath");
        if let Some(track) = object.get_mut("track") {
            if let Some(track) = track.as_object_mut() {
                track.remove("localPath");
                if let Some(p) = track.get_mut("provenance") {
                    strip_file_fields(p);
                }
            }
        }
    }
    value
}
fn strip_file_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, value| {
                let k = key.to_ascii_lowercase();
                !(k.contains("filename")
                    || k.ends_with("path")
                    || k.ends_with("directory")
                    || k.ends_with("folder"))
                    && !value.as_str().is_some_and(is_local_resource_url)
            });
            for child in object.values_mut() {
                strip_file_fields(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                strip_file_fields(child)
            }
        }
        _ => {}
    }
}
fn is_local_resource_url(value: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "file" | "asset"))
}
fn origin_allowed(headers: &HeaderMap) -> bool {
    headers.get(axum::http::header::ORIGIN).is_none_or(|o| {
        o.to_str().ok().is_some_and(|s| {
            if s == "tauri://localhost" {
                return true;
            }
            let Ok(url) = url::Url::parse(s) else {
                return false;
            };
            matches!(url.scheme(), "http" | "https")
                && matches!(url.path(), "" | "/")
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.host_str().is_some_and(|h| {
                    matches!(h, "localhost" | "tauri.localhost" | "127.0.0.1" | "::1")
                })
        })
    })
}
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    if !origin_allowed(headers) {
        return false;
    }
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
}
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}
async fn list_tools(
    State(s): State<Arc<AutomationService>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = s.token.lock().map(|t| t.clone()).unwrap_or_default();
    if !authorized(&headers, &token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({"error":{"code":"UNAUTHORIZED","message":"Invalid local automation credentials"}}),
            ),
        );
    }
    (StatusCode::OK, Json(json!({"tools":tool_definitions()})))
}
async fn call_tool(
    State(s): State<Arc<AutomationService>>,
    headers: HeaderMap,
    Json(call): Json<ToolCall>,
) -> impl IntoResponse {
    let token = s.token.lock().map(|t| t.clone()).unwrap_or_default();
    if !authorized(&headers, &token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({"content":[{"type":"text","text":"Unauthorized"}],"structuredContent":{"error":{"code":"UNAUTHORIZED"}},"isError":true}),
            ),
        );
    }
    let name = call.name.clone();
    let started = std::time::Instant::now();
    let response = match s.execute(&name, call.arguments).await {
        Ok(result) => {
            s.log(&name, "ok", started.elapsed().as_millis());
            json!({"content":[{"type":"text","text":result.to_string()}],"structuredContent":{"result":result},"isError":false})
        }
        Err(e) => {
            s.log(&name, e.code, started.elapsed().as_millis());
            json!({"content":[{"type":"text","text":e.message}],"structuredContent":{"error":{"code":e.code,"message":e.message}},"isError":true})
        }
    };
    (StatusCode::OK, Json(response))
}
fn tool_definitions() -> Vec<Value> {
    [
 ("search_library","Search local library", "read"),("get_track","Get a library track", "read"),("library_stats","Get library counts", "read"),("list_playlists","List playlists and browser snapshot versions", "read"),("get_playlist","Get playlist tracks and browser versions", "read"),("search_tracks","Search catalog providers", "read"),("search_artists","Search artists", "read"),("get_artist_discography","Get paginated artist discography", "read"),("get_release_tracks","Get ordered tracks for an artist release", "read"),("get_artist_tracks","Get paginated recordings for an artist", "read"),("get_artist_collaborations","Get paginated artist relationships", "read"),("get_artist_profile","Get public artist profile and saved top tracks", "read"),("get_app_status","Get application and playback status", "read"),("get_song_radio","Get bounded YouTube autoplay Mix recommendations for a track; does not alter playback", "read"),("search_playback_history","Search recent playback history", "read"),("list_playback_history","List recent playback history", "read"),("get_playback_history_entry","Get a playback history entry by session ID", "read"),
 ("discover_music","Search selected music sources with independent result pages", "read"),("poll_music_search","Poll a Soulseek discovery search", "read"),("inspect_source_chapters","Inspect source chapters and register stable chapter tracks", "read"),("get_autotag_status","Get paginated autotag review jobs", "read"),("retry_autotag","Retry an autotag job", "manage"),("review_autotag_candidate","Select a catalog candidate for autotag review", "manage"),("apply_autotag_review","Apply approved autotag fields", "manage"),("play_chapter","Play a registered chapter track", "control"),
 ("list_radio_stations","List built-in and saved radio stations", "read"),("search_radio_stations","Search radio stations by name, country, or tag", "read"),("get_station_details","Get details and available current metadata for a station", "read"),("add_radio_station","Save a selected Radio Browser search result", "control"),("add_custom_radio_station","Save a station from a stream URL", "control"),("remove_radio_station","Remove a saved station or hide a built-in station", "control"),("rename_radio_station","Rename a saved or built-in station", "control"),("play_radio_station","Play a saved or built-in radio station", "control"),
 ("playback_state","Get player state", "control"),("play_tracks","Play specific track IDs", "control"),("pause_playback","Pause playback", "control"),("resume_playback","Resume playback", "control"),("queue_tracks","Queue specific track IDs", "control"),("seek_playback","Seek in seconds", "control"),("set_volume","Set volume from 0 to 1", "control"),("create_playlist","Create a playlist", "control"),("add_tracks_to_playlist","Append track IDs to a playlist", "control"),
 ("search_audio_sources","Search audio sources for a track or query", "read"),("inspect_audio_source","Inspect formats and chapters for an explicit source candidate", "read"),("start_slskd_search","Start a Soulseek search", "read"),("get_slskd_search","Poll a Soulseek search", "read"),("cancel_slskd_search","Cancel a Soulseek search", "read"),("download_candidate","Download one explicitly selected candidate", "manage"),("download_jobs","List download jobs", "read"),("retry_download","Retry a failed download", "manage"),("cancel_download","Cancel a download", "manage"),("analyze_library","Queue library analysis", "manage"),("analysis_status","Get analysis status", "read"),("export_mixxx_playlist","Export playlist for Mixxx", "manage")].into_iter().map(|(name,description,_scope)|{
 let scope=tool_scope(name).unwrap_or("read");
 let (properties,required)=match name{
 "list_radio_stations"=>(json!({}),json!([])),
 "search_radio_stations"=>(json!({"query":{"type":"string","maxLength":120},"country":{"type":"string","maxLength":80},"tag":{"type":"string","maxLength":80},"limit":{"type":"integer","minimum":1,"maximum":50}}),json!([])),
 "get_station_details"|"play_radio_station"=>(json!({"stationId":{"type":"string","maxLength":120}}),json!(["stationId"])),
 "add_radio_station"=>(json!({"station":{"type":"object","description":"A structured Radio Browser station result returned by search_radio_stations"}}),json!(["station"])),
 "add_custom_radio_station"=>(json!({"name":{"type":"string","minLength":1,"maxLength":160},"url":{"type":"string","format":"uri","maxLength":2048}}),json!(["name","url"])),
 "remove_radio_station"=>(json!({"stationId":{"type":"string","maxLength":120}}),json!(["stationId"])),
 "rename_radio_station"=>(json!({"stationId":{"type":"string","maxLength":120},"name":{"type":"string","minLength":1,"maxLength":160}}),json!(["stationId","name"])),
 "get_song_radio"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"},"limit":{"type":"integer","minimum":1,"maximum":50}}),json!(["trackId","candidate"])),
 "discover_music"=>(json!({"query":{"type":"string","maxLength":500},"sourceIds":{"type":"array","maxItems":20,"items":{"type":"string","enum":["library","musicbrainz","discogs","youtube","soundcloud","soulseek","monochrome"]}},"kinds":{"type":"array","maxItems":10,"items":{"type":"string","enum":["artist","album","track","video","set","file"]}},"cursor":{"type":"string","maxLength":100},"limit":{"type":"integer","minimum":1,"maximum":100},"isrc":{"type":"string","maxLength":32}}),json!(["query"])),
 "poll_music_search"=>(json!({"searchId":{"type":"string","maxLength":300}}),json!(["searchId"])),"inspect_source_chapters"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"}}),json!(["trackId","candidate"])),"get_autotag_status"=>(json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),"retry_autotag"=>(json!({"assetId":{"type":"string"}}),json!(["assetId"])),"review_autotag_candidate"=>(json!({"assetId":{"type":"string"},"providerId":{"type":"string"}}),json!(["assetId","providerId"])),"apply_autotag_review"=>(json!({"assetId":{"type":"string"},"fieldKeys":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"string","enum":["artist","title","recordingDate"]}}}),json!(["assetId","fieldKeys"])),"play_chapter"=>(json!({"trackId":{"type":"string"}}),json!(["trackId"])),
 "search_library"=>(json!({"query":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!(["query"])),
 "search_playback_history"|"list_playback_history"=>(json!({"query":{"type":"string","maxLength":300},"provider":{"type":"string","maxLength":120,"description":"Exact playback provider filter. Use radio for radio songs or non-radio to exclude them."},"stationId":{"type":"string","maxLength":120,"description":"Filter radio playback history to one station"},"from":{"type":"string","maxLength":64,"description":"Inclusive lower bound as YYYY-MM-DD or RFC3339"},"to":{"type":"string","maxLength":64,"description":"Inclusive upper bound as YYYY-MM-DD or RFC3339"},"sort":{"type":"string","enum":["newest","oldest"]},"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),
 "get_playback_history_entry"=>(json!({"sessionId":{"type":"string","minLength":1,"maxLength":300}}),json!(["sessionId"])),
 "get_track"=>(json!({"trackId":{"type":"string"}}),json!(["trackId"])),"list_playlists"=>(json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),"get_playlist"=>(json!({"playlistId":{"type":"string"},"snapshotId":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!(["playlistId"])),
 "search_tracks"=>(json!({"query":{"type":"string"},"providers":{"type":"array","items":{"type":"string","enum":["library","musicbrainz","youtube","soundcloud","soulseek","monochrome"]}}}),json!(["query"])),
 "search_artists"=>(json!({"query":{"type":"string"},"page":{"type":"integer","minimum":1,"maximum":200}}),json!(["query"])),"get_artist_discography"|"get_artist_tracks"|"get_artist_collaborations"=>(json!({"artist":{"type":"object","properties":{"provider":{"type":"string","enum":["musicbrainz","discogs"]},"id":{"type":"string"}},"required":["provider","id"],"additionalProperties":false},"cursor":{"type":"string"}}),json!(["artist"])),"get_artist_profile"=>(json!({"artist":{"type":"object","properties":{"provider":{"type":"string","enum":["musicbrainz","discogs"]},"id":{"type":"string"}},"required":["provider","id"],"additionalProperties":false}}),json!(["artist"])),"get_release_tracks"=>(json!({"release":release_ref_schema(),"artist":artist_ref_schema()}),json!(["release","artist"])),
 "play_tracks"=>(json!({"trackIds":{"type":"array","minItems":1,"maxItems":500,"items":{"type":"string"}}}),json!(["trackIds"])),"queue_tracks"=>(json!({"trackIds":{"type":"array","minItems":1,"maxItems":500,"items":{"type":"string"}},"mode":{"type":"string","enum":["append","replace","next"]}}),json!(["trackIds"])),"seek_playback"=>(json!({"position":{"type":"number","minimum":0}}),json!(["position"])),"set_volume"=>(json!({"volume":{"type":"number","minimum":0,"maximum":1}}),json!(["volume"])),
 "create_playlist"=>(json!({"name":{"type":"string"},"idempotencyKey":{"type":"string"}}),json!(["name"])),"add_tracks_to_playlist"=>(json!({"playlistId":{"type":"string"},"trackIds":{"type":"array","items":{"type":"string"}},"idempotencyKey":{"type":"string"}}),json!(["playlistId","trackIds"])),
 "search_audio_sources"=>(json!({"trackId":{"type":"string"},"query":{"type":"string"},"providers":{"type":"array","items":{"type":"string","enum":["youtube","soundcloud","monochrome"]}}}),json!(["providers"])),"inspect_audio_source"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"}}),json!(["trackId","candidate"])),"start_slskd_search"=>(json!({"query":{"type":"string"}}),json!(["query"])),"get_slskd_search"|"cancel_slskd_search"=>(json!({"searchId":{"type":"string"}}),json!(["searchId"])),"download_candidate"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"},"formatId":{"type":"string"},"outputProfile":{"type":"string","enum":["source","mp3"]},"chapterIds":{"type":"array","items":{"type":"string"}},"idempotencyKey":{"type":"string"}}),json!(["trackId","candidate"])),
 "retry_download"|"cancel_download"=>(json!({"jobId":{"type":"string"}}),json!(["jobId"])),"download_jobs"|"analysis_status"=>(json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),"export_mixxx_playlist"=>(json!({"playlistId":{"type":"string"}}),json!(["playlistId"])),"analyze_library"=>(json!({"assetIds":{"type":"array","items":{"type":"string"}},"force":{"type":"boolean"}}),json!([])),_ => (json!({}),json!([]))};
 let mut input_schema=json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
 if name=="search_radio_stations" { input_schema["properties"]["query"]["maxLength"]=json!(100); input_schema["anyOf"]=json!([{"required":["query"]},{"required":["country"]},{"required":["tag"]}]); }
 json!({"name":name,"description":description,"scope":scope,"inputSchema":input_schema})
 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chapter_ids_and_ranges_are_stable_and_bounded() {
        let first = registered_chapter_id("recording-1", "chapter-2");
        assert_eq!(first, "chapter:recording-1:chapter-2");
        assert_eq!(first, registered_chapter_id("recording-1", "chapter-2"));
        let chapter = Chapter {
            id: "chapter-2".into(),
            title: "Part".into(),
            start_seconds: 5.0,
            end_seconds: 10.0,
            confidence: "high".into(),
            suggested_track: None,
            suggestion_confidence: None,
        };
        assert!(valid_chapter(&chapter, Some(10.0)));
        assert!(!valid_chapter(
            &Chapter {
                end_seconds: 11.0,
                ..chapter.clone()
            },
            Some(10.0)
        ));
        assert!(!valid_chapter(
            &Chapter {
                start_seconds: f64::NAN,
                ..chapter
            },
            None
        ));
    }
    #[test]
    fn new_automation_tools_have_explicit_scopes_and_schemas() {
        for (name, scope) in [
            ("discover_music", "read"),
            ("poll_music_search", "read"),
            ("inspect_source_chapters", "read"),
            ("get_autotag_status", "read"),
            ("retry_autotag", "manage"),
            ("review_autotag_candidate", "manage"),
            ("apply_autotag_review", "manage"),
            ("play_chapter", "control"),
        ] {
            assert_eq!(tool_scope(name), Some(scope));
            let definition = tool_definitions()
                .into_iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            assert_eq!(definition["scope"], scope);
            assert_eq!(definition["inputSchema"]["additionalProperties"], false);
        }
    }
    #[test]
    fn radio_mcp_tools_have_narrow_scopes_bounded_inputs_and_stable_track_identity() {
        for (name, scope) in [
            ("list_radio_stations", "read"),
            ("search_radio_stations", "read"),
            ("get_station_details", "read"),
            ("play_radio_station", "control"),
            ("add_radio_station", "control"),
            ("add_custom_radio_station", "control"),
            ("remove_radio_station", "control"),
            ("rename_radio_station", "control"),
        ] {
            assert_eq!(tool_scope(name), Some(scope));
            let definition = tool_definitions()
                .into_iter()
                .find(|tool| tool["name"] == name)
                .expect("radio tool definition");
            assert_eq!(definition["scope"], scope);
            assert_eq!(definition["inputSchema"]["additionalProperties"], false);
        }
        let search = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "search_radio_stations")
            .unwrap();
        assert_eq!(search["inputSchema"]["properties"]["limit"]["maximum"], 50);
        assert_eq!(
            search["inputSchema"]["properties"]["query"]["maxLength"],
            100
        );
        assert_eq!(search["inputSchema"]["anyOf"].as_array().unwrap().len(), 3);
        let definitions = tool_definitions();
        let custom = definitions
            .iter()
            .find(|tool| tool["name"] == "add_custom_radio_station")
            .unwrap();
        assert_eq!(custom["inputSchema"]["required"], json!(["name", "url"]));
        assert_eq!(custom["inputSchema"]["additionalProperties"], false);
        let add = definitions
            .iter()
            .find(|tool| tool["name"] == "add_radio_station")
            .unwrap();
        assert_eq!(add["inputSchema"]["required"], json!(["station"]));
        assert_eq!(add["inputSchema"]["additionalProperties"], false);

        let station = crate::radio::get_radio_stations()
            .into_iter()
            .find(|station| station.id == "main")
            .unwrap();
        let track = radio_station_track(&station);
        assert_eq!(track.id, "radio:main");
        assert_eq!(track.provider, "radio");
        assert_eq!(track.provider_id, "main");
        assert_eq!(track.provenance["sourceKind"], "radio");
        assert_eq!(track.provenance["radioStreamUrl"], station.stream_url);

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let saved = store.save_track(&track).unwrap();
        assert_eq!(saved.id, "radio:main");
        assert_eq!(saved.provider, "radio");
        assert_eq!(saved.provider_id, "main");
        assert_eq!(saved.provenance["sourceKind"], "radio");
        assert_eq!(saved.provenance["radioStreamUrl"], station.stream_url);

        let station = crate::radio::get_radio_stations()
            .into_iter()
            .find(|station| station.id == "the-north")
            .unwrap();
        let track = radio_station_track(&station);
        assert_eq!(track.id, "radio:the-north");
        assert_eq!(track.provider, "radio");
        assert_eq!(track.artist, "Internet Radio");
    }
    #[test]
    fn chapter_candidate_urls_are_provider_scoped() {
        let candidate = Candidate {
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abc".into(),
            ..Candidate::default()
        };
        assert!(validate_public_candidate(&candidate).is_ok());
        assert!(validate_public_candidate(&Candidate {
            url: "http://www.youtube.com/watch?v=abc".into(),
            ..candidate.clone()
        })
        .is_err());
        assert!(validate_public_candidate(&Candidate {
            url: "https://127.0.0.1/".into(),
            ..candidate
        })
        .is_err());
    }
    #[test]
    fn queue_modes_are_explicit_and_validated() {
        assert_eq!(
            queue_mode(&json!({"mode":"replace"})).unwrap(),
            Some("replace".into())
        );
        assert!(queue_mode(&json!({})).unwrap().is_none());
        assert_eq!(
            queue_mode(&json!({"mode":"unsupported"})).unwrap_err().code,
            "INVALID_ARGUMENT"
        );
        let tools = tool_definitions();
        let queue = tools
            .iter()
            .find(|tool| tool["name"] == "queue_tracks")
            .unwrap();
        assert_eq!(
            queue["inputSchema"]["properties"]["mode"]["enum"],
            json!(["append", "replace", "next"])
        );
    }
    #[test]
    fn origins_are_restricted() {
        let mut h = HeaderMap::new();
        assert!(origin_allowed(&h));
        h.insert("origin", "https://example.com".parse().unwrap());
        assert!(!origin_allowed(&h));
        h.insert("origin", "tauri://localhost".parse().unwrap());
        assert!(origin_allowed(&h));
        h.insert("origin", "http://localhost:43321".parse().unwrap());
        assert!(origin_allowed(&h));
        h.insert("origin", "http://localhost.attacker.test".parse().unwrap());
        assert!(!origin_allowed(&h));
    }
    #[test]
    fn bearer_token_is_required() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer abc".parse().unwrap());
        assert!(authorized(&h, "abc"));
        assert!(!authorized(&h, "abd"));
    }
    #[test]
    fn permission_defaults_are_scoped() {
        assert!(AutomationPermissions::default().read);
        assert!(AutomationPermissions::default().control);
        assert!(!AutomationPermissions::default().manage);
    }
    #[test]
    fn tool_names_have_explicit_scopes() {
        for v in tool_definitions() {
            let n = v["name"].as_str().unwrap();
            assert!(tool_scope(n).is_some());
        }
        for name in [
            "search_audio_sources",
            "download_jobs",
            "analysis_status",
            "playback_state",
            "get_app_status",
        ] {
            assert_eq!(tool_scope(name), Some("read"));
        }
        for name in [
            "search_playback_history",
            "list_playback_history",
            "get_playback_history_entry",
        ] {
            assert_eq!(tool_scope(name), Some("read"));
            assert!(playback_history_tool(name).is_some());
            let tool = tool_definitions()
                .into_iter()
                .find(|tool| tool["name"] == name)
                .expect("history tool is registered");
            assert_eq!(tool["scope"], "read");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
        let search = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "search_playback_history")
            .unwrap();
        for key in [
            "query",
            "provider",
            "stationId",
            "from",
            "to",
            "sort",
            "offset",
            "limit",
        ] {
            assert!(search["inputSchema"]["properties"].get(key).is_some());
        }
        assert_eq!(
            search["inputSchema"]["properties"]["sort"]["enum"],
            json!(["newest", "oldest"])
        );
        let get_entry = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "get_playback_history_entry")
            .unwrap();
        assert_eq!(get_entry["inputSchema"]["required"], json!(["sessionId"]));
        let playlist = tool_definitions()
            .into_iter()
            .find(|v| v["name"] == "get_playlist")
            .unwrap();
        for key in ["playlistId", "snapshotId", "offset", "limit"] {
            assert!(playlist["inputSchema"]["properties"].get(key).is_some());
        }
        for name in ["download_jobs", "analysis_status"] {
            let tool = tool_definitions()
                .into_iter()
                .find(|v| v["name"] == name)
                .unwrap();
            assert!(tool["inputSchema"]["properties"].get("offset").is_some());
            assert!(tool["inputSchema"]["properties"].get("limit").is_some());
        }
    }
    #[test]
    fn playback_history_search_options_are_bounded_and_validated() {
        let options = playback_history_search_options(&json!({
            "query":"  Night Drive ", "provider":"youtube", "stationId":"station-1", "from":"2026-01-02",
            "to":"2026-01-05T12:30:00Z", "sort":"oldest", "offset":4, "limit":9
        }))
        .unwrap();
        assert_eq!(options.query.as_deref(), Some("Night Drive"));
        assert_eq!(options.provider.as_deref(), Some("youtube"));
        assert_eq!(options.station_id.as_deref(), Some("station-1"));
        assert_eq!(options.sort.as_deref(), Some("oldest"));
        assert_eq!((options.limit, options.offset), (9, 4));
        assert!(playback_history_search_options(&json!({
            "from":"2026-01-02T12:30:00Z", "to":"2026-01-02"
        }))
        .is_ok());
        assert!(playback_history_search_options(&json!({"from":"yesterday"})).is_err());
        assert!(playback_history_search_options(&json!({"sort":"random"})).is_err());
        assert!(
            playback_history_search_options(&json!({"from":"2026-03-03","to":"2026-03-02"}))
                .is_err()
        );
        assert!(playback_history_search_options(&json!({"limit":101})).is_err());
        assert!(playback_history_search_options(&json!({"limit":0})).is_err());
        assert!(playback_history_search_options(&json!({"offset":1000001})).is_err());
        assert!(playback_history_search_options(&json!({"sql":"select *"})).is_err());
        assert!(playback_history_search_options(&Value::Null).is_ok());
        assert!(playback_history_search_options(&json!({"provider":"you\ntube"})).is_err());
        assert_eq!(
            playback_history_session_id(&json!({"sessionId":"session-123"})).unwrap(),
            "session-123"
        );
        assert_eq!(
            playback_history_session_id(&json!({"sessionId":"session-123","clear":true}))
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
    }
    #[test]
    fn playback_history_output_hides_local_paths_and_keeps_stable_ids() {
        let entry = crate::playback_history::PlaybackHistoryEntry {
            session_id: "stable-session-id".into(),
            played_at: "2026-10-14T12:00:00Z".into(),
            provider: "local".into(),
            track: Track {
                id: "recording:stable-id".into(),
                artwork_url: Some("asset://localhost/%2Fhome%2Flistener%2FMusic%2Fcover.jpg".into()),
                local_path: Some("/secret/music/file.flac".into()),
                provenance: json!({"sourcePath":"/secret/music/file.flac","safe":"kept"}),
                ..Track::default()
            },
            chapter: Some(json!({"chapterId":"c1","path":"/secret/music/file.flac"})),
        };
        let value = safe_playback_history_entry(entry.clone());
        assert_eq!(value["sessionId"], "stable-session-id");
        assert_eq!(value["track"]["id"], "recording:stable-id");
        assert_eq!(value["track"]["provenance"]["safe"], "kept");
        assert!(value["track"].get("localPath").is_none());
        assert!(value["track"].get("artworkUrl").is_none());
        assert!(value["track"]["provenance"].get("sourcePath").is_none());
        assert!(value["chapter"].get("path").is_none());
        let page = crate::playback_history::PlaybackHistoryPage {
            items: vec![entry],
            total: 1,
            has_more: false,
        };
        let page = safe_playback_history_page(page);
        assert!(page["items"][0]["track"]["provenance"]
            .get("sourcePath")
            .is_none());
        assert!(page["items"][0]["chapter"].get("path").is_none());

        let remote = safe_track(Track {
            artwork_url: Some("https://cdn.example.test/cover.jpg".into()),
            ..Track::default()
        });
        assert_eq!(remote["artworkUrl"], "https://cdn.example.test/cover.jpg");
        let file_url = safe_track(Track {
            artwork_url: Some("file:///secret/music/cover.jpg".into()),
            ..Track::default()
        });
        assert!(file_url.get("artworkUrl").is_none());
    }
    #[test]
    fn release_tracks_tool_requires_release_and_artist_references() {
        let tool = tool_definitions()
            .into_iter()
            .find(|v| v["name"] == "get_release_tracks")
            .unwrap();
        assert_eq!(tool["scope"], "read");
        assert_eq!(
            tool["inputSchema"]["required"],
            json!(["release", "artist"])
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["release"]["properties"]["release"]["required"],
            json!(["provider", "id"])
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);

        let args = json!({
            "artist":{"provider":"musicbrainz","id":"artist-id"},
            "release":{
                "release":{"provider":"musicbrainz","id":"release-id"},
                "title":"Example release","sourceUrl":"https://musicbrainz.org/release/release-id",
                "credits":[]
            }
        });
        assert_eq!(artist_arg(&args).unwrap().id, "artist-id");
        assert_eq!(release_arg(&args).unwrap().release.id, "release-id");
        assert_eq!(
            release_arg(&json!({"release":{}})).unwrap_err().code,
            "INVALID_ARGUMENT"
        );
    }
    #[test]
    fn malformed_arguments_are_rejected_and_missing_tracks_are_typed() {
        assert_eq!(
            str_arg(&json!({}), "query", 50).unwrap_err().code,
            "INVALID_ARGUMENT"
        );
        assert_eq!(
            string_array(&json!({"ids":[4]}), "ids", 5)
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("automation-test.sqlite")).unwrap();
        assert_eq!(
            required_track(&store, "missing:id").unwrap_err().code,
            "NOT_FOUND"
        );
    }
    #[test]
    fn idempotency_keys_reject_conflicting_signatures() {
        let db =
            json!({"k":{"operation":"create_playlist","signature":{"name":"One"},"id":"id-1"}});
        assert_eq!(
            idempotent_id(&db, "k", "create_playlist", &json!({"name":"One"}))
                .unwrap()
                .as_deref(),
            Some("id-1")
        );
        assert_eq!(
            idempotent_id(&db, "k", "create_playlist", &json!({"name":"Two"}))
                .unwrap_err()
                .code,
            "IDEMPOTENCY_CONFLICT"
        );
    }
    #[test]
    fn public_track_payload_removes_local_paths_but_keeps_availability() {
        let track = Track {
            local_path: Some("/music/private.mp3".into()),
            local_availability: "present".into(),
            provenance: json!({"filePath":"/private/file","metadataSource":"MusicBrainz"}),
            ..Track::default()
        };
        let safe = safe_track(track);
        assert!(safe.get("localPath").is_none());
        assert_eq!(safe["localAvailability"], "present");
        assert!(safe["provenance"].get("filePath").is_none());
        assert_eq!(safe["provenance"]["metadataSource"], "MusicBrainz");
    }
}
