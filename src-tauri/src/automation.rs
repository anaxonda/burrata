//! Authenticated, loopback-only automation API. The MCP wire protocol lives in the
//! companion bridge; this module exposes a small JSON tool-call transport.
use crate::{
    acquisition::{AcquisitionManager, AcquisitionProvider},
    analysis::AnalysisManager,
    artists::{ArtistCredentials, ArtistRef, ArtistReleaseRef, ArtistService},
    connections::ConnectionManager,
    discovery::{DiscoveryRequest, DiscoveryService},
    models::*,
    playback::PlaybackService,
    playback_history::{PlaybackHistory, PlaybackHistorySearchOptions},
    Store,
};
use axum::{
    extract::{DefaultBodyLimit, State},
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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;
use uuid::Uuid;

const PERMISSIONS_KEY: &str = "automation.permissions.v1";
const IDEMPOTENCY_KEY: &str = "automation.idempotency.v1";
const MAX_PAGE: usize = 100;
const AUTOMATION_REQUEST_BODY_LIMIT: usize =
    crate::discovery::MAX_DISCOVERY_CURSOR_LENGTH + 64 * 1024;
const DISCOVERY_SOURCE_IDS: &[&str] = &[
    "library",
    "musicbrainz",
    "discogs",
    "youtube",
    "soundcloud",
    "soulseek",
    "monochrome",
    "spotify",
];
const SEARCH_UI_SOURCE_IDS: &[&str] = &[
    "all",
    "library",
    "musicbrainz",
    "discogs",
    "youtube",
    "soundcloud",
    "soulseek",
    "monochrome",
    "spotify",
];
const SEARCH_UI_KINDS: &[&str] = &[
    "all",
    "tracks",
    "artists",
    "albums",
    "videos",
    "playlists",
    "profiles",
    "channels",
];
const SEARCH_UI_ERROR_CODES: &[&str] = &[
    "no_sources",
    "source_unavailable",
    "request_failed",
    "cancelled",
    "invalid_request",
];
const MAX_SEARCH_UI_RESULT_COUNT: u64 = 10_000;
const UI_NAVIGATION_VIEWS: &[&str] = &[
    "library",
    "favorites",
    "history",
    "downloads",
    "settings",
    "radio",
];

const UI_SETTINGS_CATEGORIES: &[&str] = &[
    "general",
    "playback",
    "library",
    "downloads",
    "connections",
    "integrations",
    "advanced",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayerPanel {
    Queue,
    Info,
    Closed,
}

impl PlayerPanel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Info => "info",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiNavigationTarget {
    View(String),
    SettingsCategory(String),
    SettingsControl(String),
    SpotifyArtist(String),
    CatalogArtist(ArtistRef, String),
    SpotifyProfile(String),
    SpotifyPlaylist(String),
    HistoryDirection(String),
}
impl UiNavigationTarget {
    fn kind(&self) -> &'static str {
        match self {
            Self::View(_) | Self::SettingsCategory(_) | Self::SettingsControl(_) => "view",
            Self::SpotifyArtist(_) => "spotify_artist",
            Self::CatalogArtist(_, _) => "catalog_artist",
            Self::SpotifyProfile(_) => "spotify_profile",
            Self::SpotifyPlaylist(_) => "spotify_playlist",
            Self::HistoryDirection(_) => "history",
        }
    }
    fn ack_key(&self) -> &str {
        match self {
            Self::View(view) => view,
            Self::CatalogArtist(artist, _) => &artist.id,
            Self::SettingsCategory(_) | Self::SettingsControl(_) => "settings",
            Self::SpotifyArtist(id)
            | Self::SpotifyProfile(id)
            | Self::SpotifyPlaylist(id)
            | Self::HistoryDirection(id) => id,
        }
    }
    fn payload(&self) -> Value {
        match self {
            Self::View(view) => json!({"kind":"view","view":view}),
            Self::SettingsCategory(category) => {
                json!({"kind":"view","view":"settings","settingsCategory":category})
            }
            Self::SettingsControl(control) => json!({
                "kind":"view","view":"settings","settingsCategory":"playback","settingsControl":control
            }),
            Self::SpotifyArtist(id) => json!({"kind":"spotify_artist","spotifyArtistId":id}),
            Self::CatalogArtist(artist, section) => json!({"kind":"catalog_artist","catalogArtist":artist,"catalogArtistSection":section}),
            Self::SpotifyProfile(id) => json!({"kind":"spotify_profile","spotifyProfileId":id}),
            Self::SpotifyPlaylist(id) => json!({"kind":"spotify_playlist","spotifyPlaylistId":id}),
            Self::HistoryDirection(direction) => {
                json!({"kind":"history","historyDirection":direction})
            }
        }
    }
}
fn parse_ui_navigation_request(arguments: &Value) -> Result<UiNavigationTarget, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("UI navigation arguments must be an object"))?;
    if object.contains_key("catalogArtist") || object.contains_key("catalogArtistSection") {
        if !object.keys().all(|key| matches!(key.as_str(), "catalogArtist" | "catalogArtistSection")) {
            return Err(ToolError::bad("Catalog artist navigation accepts only an artist and optional section"));
        }
        let artist = object.get("catalogArtist").and_then(Value::as_object)
            .filter(|artist| artist.len() == 2)
            .ok_or_else(|| ToolError::bad("Catalog artist requires only provider and id"))?;
        let provider = artist.get("provider").and_then(Value::as_str).unwrap_or("");
        let id = artist.get("id").and_then(Value::as_str).unwrap_or("");
        let valid = match provider {
            "musicbrainz" => id.len() == 36 && Uuid::parse_str(id)
                .is_ok_and(|value| value.to_string() == id.to_ascii_lowercase()),
            "discogs" => id.parse::<u64>().is_ok_and(|value| value > 0 && value.to_string() == id),
            _ => false,
        };
        if !valid { return Err(ToolError::bad("Catalog artist identity is invalid")); }
        let section = match object.get("catalogArtistSection") {
            None => "overview",
            Some(Value::String(section)) if matches!(section.as_str(), "overview" | "releases" | "tracks") => section,
            _ => return Err(ToolError::bad("Catalog artist section is unsupported")),
        };
        return Ok(UiNavigationTarget::CatalogArtist(ArtistRef { provider: provider.into(), id: id.into() }, section.into()));
    }
    if let Some(category) = object.get("settingsCategory") {
        if object.len() == 3
            && object.get("view").and_then(Value::as_str) == Some("settings")
            && category.as_str() == Some("playback")
            && object.get("settingsControl").and_then(Value::as_str)
                == Some("playbackStartupTimeout")
        {
            return Ok(UiNavigationTarget::SettingsControl(
                "playbackStartupTimeout".into(),
            ));
        }
        if object.len() == 2 && object.get("view").and_then(Value::as_str) == Some("settings") {
            if let Some(category) = category
                .as_str()
                .filter(|category| UI_SETTINGS_CATEGORIES.contains(category))
            {
                return Ok(UiNavigationTarget::SettingsCategory(category.to_owned()));
            }
        }
        return Err(ToolError::bad(
            "A supported settingsCategory requires only view=settings",
        ));
    }
    if object.contains_key("settingsControl") {
        return Err(ToolError::bad(
            "A supported settingsControl requires view=settings and settingsCategory=playback",
        ));
    }
    if object.len() != 1 {
        return Err(ToolError::bad("UI navigation accepts exactly one target"));
    }
    if let Some(view) = object.get("view").and_then(Value::as_str) {
        if UI_NAVIGATION_VIEWS.contains(&view) {
            return Ok(UiNavigationTarget::View(view.to_owned()));
        }
        return Err(ToolError::bad("UI view is unsupported"));
    }
    if let Some(id) = object.get("spotifyArtistId").and_then(Value::as_str) {
        if id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Ok(UiNavigationTarget::SpotifyArtist(id.to_owned()));
        }
    }
    if let Some(id) = object.get("spotifyPlaylistId").and_then(Value::as_str) {
        if id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Ok(UiNavigationTarget::SpotifyPlaylist(id.to_owned()));
        }
    }
    if let Some(direction) = object.get("historyDirection").and_then(Value::as_str) {
        if matches!(direction, "back" | "forward") {
            return Ok(UiNavigationTarget::HistoryDirection(direction.to_owned()));
        }
    }
    if let Some(id) = object.get("spotifyProfileId").and_then(Value::as_str) {
        let request = crate::remote_playlists::SpotifyProfileInspectionRequest {
            owner_id: id.to_owned(),
            offset: None,
            limit: None,
            mode: None,
            playlist_limit: None,
        };
        // Only a canonical ID is a UI target; URLs and encoded alternatives are not targets.
        if !id.contains("://")
            && request
                .normalized_owner_id()
                .is_ok_and(|normalized| normalized == id)
        {
            return Ok(UiNavigationTarget::SpotifyProfile(id.to_owned()));
        }
    }
    Err(ToolError::bad("UI navigation target is invalid"))
}
fn sanitize_ui_navigation_completion(
    expected: &UiNavigationTarget,
    destination: &str,
    status: &str,
) -> Result<Value, String> {
    if expected.ack_key() != destination || !matches!(status, "complete" | "failed" | "cancelled") {
        return Err("UI navigation response did not match the request".into());
    }
    let mut result =
        json!({"status":status,"targetKind":expected.kind(),"destination":expected.ack_key()});
    match expected {
        UiNavigationTarget::CatalogArtist(artist, section) => {
            result["catalogArtist"] = json!(artist);
            result["catalogArtistSection"] = json!(section);
        }
        UiNavigationTarget::SettingsCategory(category) => {
            result["settingsCategory"] = json!(category)
        }
        UiNavigationTarget::SettingsControl(control) => {
            result["settingsCategory"] = json!("playback");
            result["settingsControl"] = json!(control);
        }
        _ => {}
    }
    if matches!(
        expected,
        UiNavigationTarget::SpotifyPlaylist(_) | UiNavigationTarget::HistoryDirection(_) | UiNavigationTarget::CatalogArtist(_, _)
    ) {
        result["completionScope"] = json!("route_transition");
    }
    Ok(result)
}

fn parse_player_panel_request(arguments: &Value) -> Result<PlayerPanel, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("Player panel arguments must be an object"))?;
    if object.len() != 1 || !object.contains_key("panel") {
        return Err(ToolError::bad("Player panel accepts only a panel value"));
    }
    match object.get("panel").and_then(Value::as_str) {
        Some("queue") => Ok(PlayerPanel::Queue),
        Some("info") => Ok(PlayerPanel::Info),
        Some("closed") => Ok(PlayerPanel::Closed),
        _ => Err(ToolError::bad(
            "Player panel must be queue, info, or closed",
        )),
    }
}

fn complete_identify_pending(
    pending: &Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    id: &str,
    result: Value,
    error: Option<String>,
) -> bool {
    let sender = pending.lock().ok().and_then(|mut map| map.remove(id));
    let Some(sender) = sender else {
        return false;
    };
    let response = if let Some(error) = error {
        if error.chars().count() <= 500 {
            Err(error)
        } else {
            Err("Audio identification failed".into())
        }
    } else {
        sanitize_identification_result(result)
    };
    let _ = sender.send(response);
    true
}

fn validate_playing_identification_snapshot(
    snapshot: &Value,
    native_enabled: bool,
) -> Result<(), String> {
    if native_enabled {
        return Err("Playback engine changed during audio identification".into());
    }
    if snapshot.get("status").and_then(Value::as_str) != Some("playing") {
        return Err("Playback is not currently playing".into());
    }
    if snapshot
        .get("trackId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .is_none()
    {
        return Err("There is no current playback track".into());
    }
    if snapshot
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .is_none()
    {
        return Err("Current playback session is unavailable".into());
    }
    if snapshot
        .get("sourceGeneration")
        .and_then(Value::as_u64)
        .is_none()
    {
        return Err("Current WebView playback source is unavailable".into());
    }
    if snapshot.get("seekSerial").and_then(Value::as_u64).is_none() {
        return Err("Current playback seek state is unavailable".into());
    }
    if snapshot.get("chapterId").is_some_and(|value| {
        !value.is_null()
            && value
                .as_str()
                .is_none_or(|chapter| chapter.is_empty() || chapter.len() > 256)
    }) {
        return Err("Current playback chapter identity is invalid".into());
    }
    Ok(())
}

fn validate_playing_identification_result(
    expected: &Value,
    current: &Value,
    native_enabled: bool,
    result: Value,
) -> Result<Value, String> {
    validate_playing_identification_snapshot(expected, false)?;
    validate_playing_identification_snapshot(current, native_enabled)?;
    for key in [
        "trackId",
        "sessionId",
        "sourceGeneration",
        "seekSerial",
        "chapterId",
    ] {
        if expected.get(key) != current.get(key) {
            return Err("Playback changed during audio identification".into());
        }
    }
    let result = sanitize_identification_result(result)?;
    if result.get("trackId") != expected.get("trackId") {
        return Err("Identification result did not match the current playback track".into());
    }
    Ok(result)
}

fn sanitize_identification_result(result: Value) -> Result<Value, String> {
    let object = result
        .as_object()
        .ok_or("Identification response must be an object")?;
    let text = |key: &str, limit: usize| -> Result<Option<String>, String> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) if value.chars().count() <= limit => Ok(Some(value.clone())),
            _ => Err(format!("Identification field {key} is invalid")),
        }
    };
    let track_id = text("trackId", 512)?
        .filter(|value| !value.is_empty())
        .ok_or("Identification response has no track identity")?;
    let title = text("title", 500)?;
    let artist = text("artist", 500)?;
    let note = text("note", 2000)?;
    let artwork_url =
        crate::radio::safe_recognition_artwork_url(text("artworkUrl", 4096)?.as_deref());
    Ok(
        json!({"trackId":track_id,"title":title,"artist":artist,"artworkUrl":artwork_url,"note":note}),
    )
}

fn sanitize_player_panel_completion(
    expected: PlayerPanel,
    panel: &str,
    status: &str,
) -> Result<Value, String> {
    if panel != expected.as_str() || status != "complete" {
        return Err("Player panel response did not match the request".into());
    }
    Ok(json!({"panel": expected.as_str(), "status": "complete"}))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchUiRequest {
    query: String,
    source_id: String,
    kind: String,
}

fn parse_search_ui_request(arguments: &Value) -> Result<SearchUiRequest, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("Search UI arguments must be an object"))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "query" | "sourceId" | "kind"))
    {
        return Err(ToolError::bad("Unsupported search UI argument"));
    }
    let query = object
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty() && query.chars().take(501).count() <= 500)
        .ok_or_else(|| ToolError::bad("Search query must contain 1 to 500 characters"))?;
    let source_id = object
        .get("sourceId")
        .and_then(Value::as_str)
        .filter(|source| SEARCH_UI_SOURCE_IDS.contains(source))
        .ok_or_else(|| ToolError::bad("Search source is unsupported"))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| SEARCH_UI_KINDS.contains(kind))
        .ok_or_else(|| ToolError::bad("Search result type is unsupported"))?;
    Ok(SearchUiRequest {
        query: query.to_owned(),
        source_id: source_id.to_owned(),
        kind: kind.to_owned(),
    })
}

fn sanitize_search_ui_completion(
    expected_source: &str,
    expected_kind: &str,
    status: &str,
    source_id: &str,
    kind: &str,
    result_counts: &Value,
    error_code: Option<&str>,
) -> Result<Value, String> {
    sanitize_search_ui_completion_with_coverage(
        expected_source,
        expected_kind,
        status,
        source_id,
        kind,
        result_counts,
        error_code,
        None,
    )
}

fn sanitize_search_ui_completion_with_coverage(
    expected_source: &str,
    expected_kind: &str,
    status: &str,
    source_id: &str,
    kind: &str,
    result_counts: &Value,
    error_code: Option<&str>,
    source_coverage: Option<&Value>,
) -> Result<Value, String> {
    if !matches!(status, "complete" | "failed" | "cancelled")
        || source_id != expected_source
        || kind != expected_kind
    {
        return Err("Search UI response did not match the request".into());
    }
    let mut counts = serde_json::Map::new();
    let values = result_counts
        .as_object()
        .ok_or_else(|| "Search UI result counts were invalid".to_string())?;
    if values.len() > SEARCH_UI_SOURCE_IDS.len() {
        return Err("Search UI result counts were invalid".into());
    }
    for (source, count) in values {
        if source == "all"
            || !SEARCH_UI_SOURCE_IDS.contains(&source.as_str())
            || (expected_source != "all" && source != expected_source)
        {
            return Err("Search UI result counts were invalid".into());
        }
        let count = count
            .as_u64()
            .filter(|count| *count <= MAX_SEARCH_UI_RESULT_COUNT)
            .ok_or_else(|| "Search UI result counts were invalid".to_string())?;
        counts.insert(source.clone(), json!(count));
    }
    if let Some(code) = error_code {
        if !SEARCH_UI_ERROR_CODES.contains(&code) {
            return Err("Search UI response code was invalid".into());
        }
    }
    if status == "failed" && (error_code.is_none() || error_code == Some("cancelled")) {
        return Err("Search UI failure code was missing".into());
    }
    if status == "cancelled" && error_code != Some("cancelled") {
        return Err("Search UI cancellation code was missing".into());
    }
    let mut sanitized = json!({
        "status": status,
        "sourceId": expected_source,
        "kind": expected_kind,
        "resultCounts": counts,
        "errorCode": error_code,
    });
    if let Some(coverage) = source_coverage {
        let values = coverage
            .as_object()
            .ok_or_else(|| "Search UI source coverage was invalid".to_string())?;
        if values.len() > SEARCH_UI_SOURCE_IDS.len().saturating_sub(1) {
            return Err("Search UI source coverage was invalid".into());
        }
        let mut safe_coverage = serde_json::Map::new();
        for (source, value) in values {
            if source == "all"
                || !SEARCH_UI_SOURCE_IDS.contains(&source.as_str())
                || (expected_source != "all" && source != expected_source)
            {
                return Err("Search UI source coverage was invalid".into());
            }
            let entry = value
                .as_object()
                .filter(|entry| entry.len() == 2)
                .ok_or_else(|| "Search UI source coverage was invalid".to_string())?;
            let has_more = entry
                .get("hasMore")
                .and_then(Value::as_bool)
                .ok_or_else(|| "Search UI source coverage was invalid".to_string())?;
            let incomplete = entry
                .get("incomplete")
                .and_then(Value::as_bool)
                .ok_or_else(|| "Search UI source coverage was invalid".to_string())?;
            safe_coverage.insert(
                source.clone(),
                json!({"hasMore":has_more,"incomplete":incomplete}),
            );
        }
        sanitized["sourceCoverage"] = Value::Object(safe_coverage);
    }
    Ok(sanitized)
}

fn validate_discovery_sources(sources: &[String]) -> Result<(), ToolError> {
    if sources
        .iter()
        .any(|source| !DISCOVERY_SOURCE_IDS.contains(&source.as_str()))
    {
        return Err(ToolError::bad("sourceIds contains an unsupported source"));
    }
    Ok(())
}

fn managed_provider_qualification_status(
    status: &crate::managed_provider_session::ManagedProviderSessionStatus,
) -> Value {
    json!({
        "enabled": status.enabled,
        "running": status.running,
        "mode": status.mode.clone(),
        "connectionSeen": status.connection_seen,
        "confirmedAccounts": {
            "spotify": status.spotify_account_id.is_some(),
            "youtube": status.youtube_account_id.is_some(),
        },
        "accountRevisions": status.account_revisions.clone(),
        "transport": status.transport.clone(),
        "credentialStorage": status.credential_storage.clone(),
        "youtubeLoginPage": status.youtube_login_page.clone(),
        "youtubeReconnectRequired": status.youtube_reconnect_required,
    })
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResizeWindowRequest {
    width: u32,
    height: u32,
    restore_maximized: Option<bool>,
}

fn parse_resize_window_request(arguments: &Value) -> Result<ResizeWindowRequest, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("arguments must be an object"))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "width" | "height" | "restoreMaximized"))
    {
        return Err(ToolError::bad("Unsupported resize_app_window argument"));
    }
    let dimension = |key: &str, minimum: u64, maximum: u64| -> Result<u32, ToolError> {
        let value = object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| (minimum..=maximum).contains(value))
            .ok_or_else(|| {
                ToolError::bad(format!(
                    "{key} must be an integer from {minimum} through {maximum}"
                ))
            })?;
        Ok(value as u32)
    };
    let restore_maximized = object
        .get("restoreMaximized")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| ToolError::bad("restoreMaximized must be a boolean"))
        })
        .transpose()?;
    Ok(ResizeWindowRequest {
        width: dimension("width", 320, 7680)?,
        height: dimension("height", 240, 4320)?,
        restore_maximized,
    })
}

fn logical_window_size(window: &tauri::WebviewWindow) -> Result<Value, ToolError> {
    let scale_factor = window
        .scale_factor()
        .map_err(|error| ToolError::failed(error.to_string()))?;
    let size = window
        .inner_size()
        .map_err(|error| ToolError::failed(error.to_string()))?;
    Ok(logical_size_value(size, scale_factor))
}

fn logical_size_value(size: tauri::PhysicalSize<u32>, scale_factor: f64) -> Value {
    let size = size.to_logical::<f64>(scale_factor);
    json!({"width":size.width,"height":size.height})
}

const PLAYBACK_LAYOUT_ROLES: &[&str] = &[
    "footer",
    "now_playing",
    "transport",
    "scrubber",
    "right",
    "quality",
    "error",
    "queue_panel",
    "queue_header",
    "queue_title",
    "identify",
    "queue_close",
    "panel_close",
];
const REQUIRED_PLAYBACK_LAYOUT_ROLES: &[&str] =
    &["footer", "now_playing", "transport", "scrubber", "right"];
const PLAYBACK_TRACK_LIST_ROLES: &[&str] = &["search", "library", "history", "artist", "remote"];
const MAX_PLAYBACK_TRACK_LIST_ROWS: u64 = 10_000;
fn sanitize_track_list_counts(report: &Value) -> Result<Vec<Value>, ToolError> {
    let empty = || {
        PLAYBACK_TRACK_LIST_ROLES
            .iter()
            .map(|role| json!({"role":role,"visibleRows":0,"playing":0,"paused":0,"buffering":0}))
            .collect::<Vec<_>>()
    };
    let Some(raw) = report.get("trackListsCounts") else {
        return Ok(empty());
    };
    let raw = raw
        .as_array()
        .filter(|items| items.len() == PLAYBACK_TRACK_LIST_ROLES.len())
        .ok_or_else(|| ToolError::bad("Invalid playback track list counts"))?;
    let mut counts = std::collections::HashMap::new();
    for item in raw {
        let role = item
            .get("role")
            .and_then(Value::as_str)
            .filter(|role| PLAYBACK_TRACK_LIST_ROLES.contains(role))
            .ok_or_else(|| ToolError::bad("Invalid playback track list role"))?;
        let count = |key: &str| {
            item.get(key)
                .and_then(Value::as_u64)
                .filter(|value| *value <= MAX_PLAYBACK_TRACK_LIST_ROWS)
                .ok_or_else(|| ToolError::bad(format!("Invalid playback track list {key}")))
        };
        let visible_rows = count("visibleRows")?;
        let playing = count("playing")?;
        let paused = count("paused")?;
        let buffering = count("buffering")?;
        if playing > visible_rows || paused > visible_rows || buffering > visible_rows {
            return Err(ToolError::bad(
                "Playback track state count exceeds visible rows",
            ));
        }
        if counts
            .insert(role, json!({"role":role,"visibleRows":visible_rows,"playing":playing,"paused":paused,"buffering":buffering}))
            .is_some()
        {
            return Err(ToolError::bad("Duplicate playback track list role"));
        }
    }
    if counts.len() != PLAYBACK_TRACK_LIST_ROLES.len() {
        return Err(ToolError::bad("Incomplete playback track list counts"));
    }
    Ok(PLAYBACK_TRACK_LIST_ROLES
        .iter()
        .map(|role| counts.remove(role).expect("validated track list role"))
        .collect())
}
fn bounded_metric(value: &Value, key: &str, max: f64) -> Result<f64, ToolError> {
    let number = value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0. && *n <= max)
        .ok_or_else(|| ToolError::bad(format!("Invalid playback layout {key}")))?;
    Ok(number)
}
fn sanitize_artwork_diagnostics(report: &Value) -> Result<Value, ToolError> {
    let Some(raw) = report.get("artworkDiagnostics") else {
        return Ok(Value::Null);
    };
    let boolean = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| ToolError::bad("Invalid artwork diagnostic boolean"))
    };
    let scheme_key = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    "none" | "asset" | "http" | "https" | "data" | "blob" | "other"
                )
            })
            .map(str::to_owned)
            .ok_or_else(|| ToolError::bad("Invalid artwork diagnostic scheme"))
    };
    let scheme = |value: &Value| scheme_key(value, "urlScheme");
    let optional_scheme = |value: &Value, key: &str| -> Result<Option<String>, ToolError> {
        value.get(key).map(|_| scheme_key(value, key)).transpose()
    };
    let optional_boolean = |value: &Value, key: &str| -> Result<Option<bool>, ToolError> {
        value
            .get(key)
            .map(|field| {
                field
                    .as_bool()
                    .ok_or_else(|| ToolError::bad("Invalid artwork diagnostic boolean"))
            })
            .transpose()
    };
    let optional_enum =
        |value: &Value, key: &str, allowed: &[&str]| -> Result<Option<String>, ToolError> {
            value
                .get(key)
                .map(|field| {
                    field
                        .as_str()
                        .filter(|value| allowed.contains(value))
                        .map(str::to_owned)
                        .ok_or_else(|| ToolError::bad("Invalid artwork diagnostic state"))
                })
                .transpose()
        };
    let raw_images = raw
        .get("images")
        .and_then(Value::as_array)
        .filter(|images| images.len() == 2)
        .ok_or_else(|| ToolError::bad("Invalid artwork diagnostic images"))?;
    let mut images = Vec::new();
    for (image, role) in raw_images.iter().zip(["footer", "details"]) {
        if image.get("role").and_then(Value::as_str) != Some(role) {
            return Err(ToolError::bad("Invalid artwork diagnostic role"));
        }
        let mut sanitized = json!({"role":role,"present":boolean(image,"present")?,"complete":boolean(image,"complete")?,"naturalWidth":bounded_metric(image,"naturalWidth",32768.)?,"naturalHeight":bounded_metric(image,"naturalHeight",32768.)?,"urlScheme":scheme(image)?});
        for key in ["srcScheme", "currentSrcScheme"] {
            if let Some(value) = optional_scheme(image, key)? {
                sanitized[key] = json!(value);
            }
        }
        for key in ["srcMatchesExpected", "currentSrcMatchesExpected"] {
            if let Some(value) = optional_boolean(image, key)? {
                sanitized[key] = json!(value);
            }
        }
        if let Some(value) = optional_enum(
            image,
            "loadState",
            &["absent", "loading", "loaded", "failed"],
        )? {
            sanitized["loadState"] = json!(value);
        }
        images.push(sanitized);
    }
    let status = raw
        .get("probeStatus")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "not_local" | "loaded" | "failed" | "timeout"))
        .ok_or_else(|| ToolError::bad("Invalid artwork probe status"))?;
    let mut sanitized = json!({"urlScheme":scheme(raw)?,"blacklisted":boolean(raw,"blacklisted")?,"images":images,"probeStatus":status,"probeWidth":bounded_metric(raw,"probeWidth",32768.)?,"probeHeight":bounded_metric(raw,"probeHeight",32768.)?,"cspBlocked":boolean(raw,"cspBlocked")?});
    for key in ["probeSrcScheme", "probeCurrentSrcScheme"] {
        if let Some(value) = optional_scheme(raw, key)? {
            sanitized[key] = json!(value);
        }
    }
    for key in [
        "probeSrcMatchesExpected",
        "probeCurrentSrcMatchesExpected",
        "probeOnError",
    ] {
        if let Some(value) = optional_boolean(raw, key)? {
            sanitized[key] = json!(value);
        }
    }
    Ok(sanitized)
}
fn sanitize_playback_layout(report: Value) -> Result<Value, ToolError> {
    let viewport = report
        .get("viewport")
        .ok_or_else(|| ToolError::bad("Missing playback layout viewport"))?;
    let viewport_width = bounded_metric(viewport, "width", 100_000.)?;
    let viewport_height = bounded_metric(viewport, "height", 100_000.)?;
    if viewport_width <= 0. || viewport_height <= 0. {
        return Err(ToolError::bad("Invalid playback layout viewport"));
    }
    let footer_height = bounded_metric(&report, "footerHeight", 2_000.)?;
    let raw = report
        .get("elements")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::bad("Missing playback layout elements"))?;
    if raw.len() != PLAYBACK_LAYOUT_ROLES.len() {
        return Err(ToolError::bad("Invalid playback layout element set"));
    }
    let mut elements = Vec::with_capacity(raw.len());
    let mut seen = std::collections::HashSet::new();
    for item in raw {
        let role = item
            .get("role")
            .and_then(Value::as_str)
            .filter(|role| PLAYBACK_LAYOUT_ROLES.contains(role))
            .ok_or_else(|| ToolError::bad("Invalid playback layout role"))?;
        if !seen.insert(role) {
            return Err(ToolError::bad("Duplicate playback layout role"));
        }
        let present = item
            .get("present")
            .and_then(Value::as_bool)
            .ok_or_else(|| ToolError::bad("Invalid playback layout presence"))?;
        let visible = item
            .get("visible")
            .and_then(Value::as_bool)
            .ok_or_else(|| ToolError::bad("Invalid playback layout visibility"))?;
        let x = item
            .get("x")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && n.abs() <= 100_000.)
            .ok_or_else(|| ToolError::bad("Invalid playback layout x"))?;
        let y = item
            .get("y")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && n.abs() <= 100_000.)
            .ok_or_else(|| ToolError::bad("Invalid playback layout y"))?;
        let width = bounded_metric(item, "width", 100_000.)?;
        let height = bounded_metric(item, "height", 100_000.)?;
        let client_width = bounded_metric(item, "clientWidth", 100_000.)?;
        let scroll_width = bounded_metric(item, "scrollWidth", 100_000.)?;
        if !present
            && (visible
                || x != 0.
                || y != 0.
                || width != 0.
                || height != 0.
                || client_width != 0.
                || scroll_width != 0.)
        {
            return Err(ToolError::bad("Absent playback layout role has geometry"));
        }
        let in_viewport = present
            && visible
            && x < viewport_width
            && y < viewport_height
            && x + width > 0.
            && y + height > 0.;
        elements.push(json!({"role":role,"present":present,"visible":visible,"inViewport":in_viewport,"x":x,"y":y,"width":width,"height":height,"clientWidth":client_width,"scrollWidth":scroll_width}));
    }
    if seen.len() != PLAYBACK_LAYOUT_ROLES.len() {
        return Err(ToolError::bad("Incomplete playback layout element set"));
    }
    let geometry = |role: &str| -> Option<(f64, f64, f64, f64, bool, bool, f64, f64)> {
        elements
            .iter()
            .find(|element| element["role"] == role)
            .map(|element| {
                (
                    element["x"].as_f64().unwrap_or(0.),
                    element["y"].as_f64().unwrap_or(0.),
                    element["width"].as_f64().unwrap_or(0.),
                    element["height"].as_f64().unwrap_or(0.),
                    element["present"].as_bool().unwrap_or(false),
                    element["visible"].as_bool().unwrap_or(false),
                    element["clientWidth"].as_f64().unwrap_or(0.),
                    element["scrollWidth"].as_f64().unwrap_or(0.),
                )
            })
    };
    let mut findings = Vec::new();
    for role in REQUIRED_PLAYBACK_LAYOUT_ROLES {
        match geometry(role) {
            Some((_, _, _, _, false, _, _, _)) | None => {
                findings.push(json!({"code":"missing_required_role","roles":[role]}))
            }
            Some((_, _, _, _, true, false, _, _)) => {
                findings.push(json!({"code":"hidden_required_role","roles":[role]}))
            }
            _ => {}
        }
    }
    let queue_panel_visible = geometry("queue_panel").is_some_and(|g| g.4 && g.5);
    let queue_header_visible = geometry("queue_header").is_some_and(|g| g.4 && g.5);
    if queue_panel_visible {
        if !queue_header_visible {
            findings
                .push(json!({"code":"hidden_queue_header","roles":["queue_panel","queue_header"]}));
        }
        if !geometry("panel_close").is_some_and(|g| g.4 && g.5) {
            findings
                .push(json!({"code":"hidden_panel_close","roles":["queue_panel","panel_close"]}));
        }
    }
    if queue_header_visible && !geometry("queue_title").is_some_and(|g| g.4 && g.5) {
        findings.push(json!({"code":"hidden_queue_title","roles":["queue_header","queue_title"]}));
    }
    if queue_header_visible {
        for child in ["queue_title", "identify", "queue_close"] {
            let Some((cx, cy, cw, ch, child_present, child_visible, _, _)) = geometry(child) else {
                continue;
            };
            if !child_present || !child_visible {
                continue;
            }
            let (hx, hy, hw, hh, _, _, _, _) = geometry("queue_header").unwrap();
            if cx < hx || cy < hy || cx + cw > hx + hw || cy + ch > hy + hh {
                findings
                    .push(json!({"code":"queue_header_clipping","roles":["queue_header",child]}));
            }
        }
    }
    for role in PLAYBACK_LAYOUT_ROLES {
        let Some((x, y, w, h, present, visible, client, scroll)) = geometry(role) else {
            continue;
        };
        if !present || !visible {
            continue;
        }
        if x < 0. || y < 0. || x + w > viewport_width || y + h > viewport_height {
            findings.push(json!({"code":"out_of_viewport","roles":[role],"edges":{"left":x<0.,"top":y<0.,"right":x+w>viewport_width,"bottom":y+h>viewport_height}}));
        }
        if scroll > client + 1. {
            findings.push(json!({"code":"horizontal_overflow","roles":[role],"overflowPx":(scroll-client).round()}));
        }
    }
    for (i, a) in PLAYBACK_LAYOUT_ROLES.iter().enumerate() {
        if *a == "footer" {
            continue;
        }
        let Some((ax, ay, aw, ah, ap, av, _, _)) = geometry(a) else {
            continue;
        };
        if !ap || !av {
            continue;
        }
        for b in PLAYBACK_LAYOUT_ROLES.iter().skip(i + 1) {
            if *b == "footer" {
                continue;
            }
            let Some((bx, by, bw, bh, bp, bv, _, _)) = geometry(b) else {
                continue;
            };
            if !bp || !bv {
                continue;
            }
            let ix = (ax + aw).min(bx + bw) - ax.max(bx);
            let iy = (ay + ah).min(by + bh) - ay.max(by);
            let area = ix.max(0.) * iy.max(0.);
            // Suppress only known semantic containment: the footer wraps every zone,
            // and the quality indicator is part of the now-playing control.
            let queue_panel_child = |role: &str| {
                matches!(
                    role,
                    "queue_header" | "queue_title" | "identify" | "queue_close"
                )
            };
            let queue_header_child =
                |role: &str| matches!(role, "queue_title" | "identify" | "queue_close");
            let semantic_container_pair = *a == "footer"
                || *b == "footer"
                || (*a == "now_playing" && *b == "quality")
                || (*a == "quality" && *b == "now_playing")
                || (*a == "queue_panel" && queue_panel_child(*b))
                || (*b == "queue_panel" && queue_panel_child(*a))
                || (*a == "queue_header" && queue_header_child(*b))
                || (*b == "queue_header" && queue_header_child(*a));
            if area > 1. && !semantic_container_pair {
                findings.push(json!({"code":"overlap","roles":[a,b],"areaPx":area.round()}));
            }
        }
    }
    let artwork_diagnostics = sanitize_artwork_diagnostics(&report)?;
    let track_lists = sanitize_track_list_counts(&report)?;
    Ok(
        json!({"schemaVersion":1,"viewport":{"width":viewport_width,"height":viewport_height},"footerHeight":footer_height,"elements":elements,"findings":findings,"trackListsCounts":track_lists,"artworkDiagnostics":artwork_diagnostics}),
    )
}

fn sanitize_webview_media_inspection(raw: Value) -> Result<Value, ToolError> {
    let media = raw
        .get("media")
        .and_then(Value::as_object)
        .ok_or_else(|| ToolError::bad("Missing WebView media sample"))?;
    if raw.get("engine").and_then(Value::as_str) != Some("webview")
        || raw.get("applicable").and_then(Value::as_bool) != Some(true)
    {
        return Err(ToolError::bad("Invalid WebView media sample engine"));
    }
    let nullable_string = |key: &str| -> Result<Value, ToolError> {
        match raw.get(key) {
            Some(Value::String(value)) if value.len() <= 512 => Ok(json!(value)),
            Some(Value::Null) => Ok(Value::Null),
            _ => Err(ToolError::bad(format!("Invalid media sample {key}"))),
        }
    };
    let nullable_u64 = |key: &str| -> Result<Value, ToolError> {
        match raw.get(key) {
            Some(Value::Number(value)) if value.as_u64().is_some() => {
                Ok(json!(value.as_u64().unwrap()))
            }
            Some(Value::Null) => Ok(Value::Null),
            _ => Err(ToolError::bad(format!("Invalid media sample {key}"))),
        }
    };
    let current_index = raw
        .get("currentIndex")
        .and_then(Value::as_i64)
        .filter(|value| *value >= -1 && *value <= 1_000_000)
        .ok_or_else(|| ToolError::bad("Invalid media sample currentIndex"))?;
    let queue_total = raw
        .get("queueTotal")
        .and_then(Value::as_u64)
        .filter(|value| *value <= 1_000_000)
        .ok_or_else(|| ToolError::bad("Invalid media sample queueTotal"))?;
    let entry_id = match raw.get("entryId") {
        Some(Value::String(value)) if value.len() <= 256 => json!(value),
        Some(Value::Null) => Value::Null,
        _ => return Err(ToolError::bad("Invalid media sample entryId")),
    };
    let queue_track_id = match raw.get("queueTrackId") {
        Some(Value::String(value)) if value.len() <= 512 => json!(value),
        Some(Value::Null) => Value::Null,
        _ => return Err(ToolError::bad("Invalid media sample queueTrackId")),
    };
    let media_present = media
        .get("present")
        .and_then(Value::as_bool)
        .ok_or_else(|| ToolError::bad("Invalid media element presence"))?;
    let finite_or_null = |key: &str, max: f64| -> Result<Value, ToolError> {
        match media.get(key) {
            Some(Value::Number(value))
                if value
                    .as_f64()
                    .is_some_and(|n| n.is_finite() && n >= 0. && n <= max) =>
            {
                Ok(json!(value.as_f64().unwrap()))
            }
            Some(Value::Null) => Ok(Value::Null),
            _ => Err(ToolError::bad(format!("Invalid media element {key}"))),
        }
    };
    let boolean = |key: &str| {
        media
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| ToolError::bad(format!("Invalid media element {key}")))
    };
    let nullable_boolean = |key: &str| -> Result<Value, ToolError> {
        match media.get(key) {
            Some(Value::Bool(value)) => Ok(json!(value)),
            Some(Value::Null) => Ok(Value::Null),
            _ => Err(ToolError::bad(format!("Invalid media element {key}"))),
        }
    };
    let ready_state = finite_or_null("readyState", 4.)?;
    let network_state = finite_or_null("networkState", 3.)?;
    let error_code = match media.get("errorCode") {
        Some(Value::Number(value)) if value.as_u64().is_some_and(|n| n <= 4) => {
            json!(value.as_u64().unwrap())
        }
        Some(Value::Null) => Value::Null,
        _ => return Err(ToolError::bad("Invalid media element errorCode")),
    };
    let current_time = finite_or_null("currentTime", 1.0e9)?;
    let duration = finite_or_null("duration", 1.0e9)?;
    let source_stable = media
        .get("sourceStable")
        .and_then(Value::as_bool)
        .ok_or_else(|| ToolError::bad("Invalid media element source stability"))?;
    if media_present && (current_time.is_null() || ready_state.is_null() || network_state.is_null())
    {
        return Err(ToolError::bad(
            "Present media element is missing a finite sample",
        ));
    }
    if media_present && (media.get("paused").and_then(Value::as_bool).is_none() || !source_stable) {
        return Err(ToolError::bad(
            "Present media element identity changed during inspection",
        ));
    }
    let diagnostics = match raw.get("diagnostics") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let object = value
                .as_object()
                .ok_or_else(|| ToolError::bad("Invalid media diagnostics"))?;
            let owner = object
                .get("owner")
                .and_then(Value::as_object)
                .ok_or_else(|| ToolError::bad("Invalid media diagnostic owner"))?;
            let events = object
                .get("events")
                .and_then(Value::as_array)
                .filter(|items| items.len() <= 64)
                .ok_or_else(|| ToolError::bad("Invalid media diagnostic events"))?;
            let owner_matches = owner.get("sessionId") == raw.get("sessionId")
                && owner.get("sourceGeneration") == raw.get("sourceGeneration")
                && owner.get("entryId") == raw.get("entryId")
                && owner.get("seekSerial") == raw.get("seekSerial");
            if !owner_matches {
                return Err(ToolError::bad("Stale media diagnostic owner"));
            }
            let allowed = [
                "play",
                "playing",
                "waiting",
                "stalled",
                "canplay",
                "canplaythrough",
                "seeking",
                "seeked",
                "ended",
                "error",
            ];
            let nullable_small = |item: &Value, key: &str, max: u64| -> Result<Value, ToolError> {
                match item.get(key) {
                    Some(Value::Number(number)) if number.as_u64().is_some_and(|v| v <= max) => {
                        Ok(json!(number.as_u64().unwrap()))
                    }
                    Some(Value::Null) => Ok(Value::Null),
                    _ => Err(ToolError::bad("Invalid media diagnostic state")),
                }
            };
            let nullable_finite = |item: &Value, key: &str, max: f64| -> Result<Value, ToolError> {
                match item.get(key) {
                    Some(Value::Number(number))
                        if number
                            .as_f64()
                            .is_some_and(|v| v.is_finite() && v >= 0. && v <= max) =>
                    {
                        Ok(json!(number.as_f64().unwrap()))
                    }
                    Some(Value::Null) => Ok(Value::Null),
                    _ => Err(ToolError::bad("Invalid media diagnostic number")),
                }
            };
            let mut safe_events = Vec::with_capacity(events.len());
            for event in events {
                let kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|value| allowed.contains(value))
                    .ok_or_else(|| ToolError::bad("Invalid media diagnostic event type"))?;
                let paused = match event.get("paused") {
                    Some(Value::Bool(value)) => json!(value),
                    Some(Value::Null) => Value::Null,
                    _ => return Err(ToolError::bad("Invalid media diagnostic paused state")),
                };
                let ranges = event
                    .get("buffered")
                    .and_then(Value::as_array)
                    .filter(|items| items.len() <= 8)
                    .ok_or_else(|| ToolError::bad("Invalid media diagnostic buffered ranges"))?;
                let mut safe_ranges = Vec::with_capacity(ranges.len());
                for range in ranges {
                    let start = range
                        .get("start")
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite() && *v >= 0. && *v <= 1.0e9)
                        .ok_or_else(|| ToolError::bad("Invalid media diagnostic range"))?;
                    let end = range
                        .get("end")
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite() && *v >= start && *v <= 1.0e9)
                        .ok_or_else(|| ToolError::bad("Invalid media diagnostic range"))?;
                    safe_ranges.push(json!({"start":start,"end":end}));
                }
                safe_events.push(json!({"atMs":nullable_finite(event,"atMs",1.0e12)?,"type":kind,"readyState":nullable_small(event,"readyState",4)?,"networkState":nullable_small(event,"networkState",3)?,"paused":paused,"currentTime":nullable_finite(event,"currentTime",1.0e9)?,"buffered":safe_ranges}));
            }
            let source_format = match object.get("sourceFormat") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let format = value
                        .as_object()
                        .ok_or_else(|| ToolError::bad("Invalid media source format"))?;
                    let safe_token = |key: &str,
                                      max_len: usize,
                                      allow: &dyn Fn(u8) -> bool|
                     -> Result<Value, ToolError> {
                        match format.get(key) {
                            Some(Value::String(text))
                                if !text.is_empty()
                                    && text.len() <= max_len
                                    && text.bytes().all(allow) =>
                            {
                                Ok(json!(text))
                            }
                            Some(Value::Null) => Ok(Value::Null),
                            _ => Err(ToolError::bad("Invalid media source format")),
                        }
                    };
                    let codec = safe_token("codec", 64, &|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')
                    })?;
                    let mime_type = safe_token("mimeType", 96, &|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'+' | b'-')
                    })?;
                    if mime_type.as_str().is_some_and(|mime| {
                        mime.matches('/').count() != 1
                            || mime.starts_with('/')
                            || mime.ends_with('/')
                    }) {
                        return Err(ToolError::bad("Invalid media source format"));
                    }
                    let format_id = safe_token("formatId", 40, &|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')
                    })?;
                    let has_video = match format.get("hasVideo") {
                        Some(Value::Bool(v)) => json!(v),
                        Some(Value::Null) => Value::Null,
                        _ => return Err(ToolError::bad("Invalid media source format")),
                    };
                    let expected_length = match format.get("expectedContentLength") {
                        Some(Value::Number(number))
                            if number.as_u64().is_some_and(|v| v <= 17_179_869_184) =>
                        {
                            json!(number.as_u64().unwrap())
                        }
                        Some(Value::Null) => Value::Null,
                        _ => return Err(ToolError::bad("Invalid media source format")),
                    };
                    let stream_kind = match format.get("streamKind") {
                        Some(Value::String(v))
                            if v == "direct" || v == "hls" || v == "unavailable" =>
                        {
                            json!(v)
                        }
                        Some(Value::Null) => Value::Null,
                        _ => return Err(ToolError::bad("Invalid media source format")),
                    };
                    Some(
                        json!({"codec":codec,"mimeType":mime_type,"formatId":format_id,"hasVideo":has_video,"expectedContentLength":expected_length,"streamKind":stream_kind}),
                    )
                }
            };
            let mut safe = json!({"owner":{"sessionId":owner.get("sessionId").cloned().unwrap_or(Value::Null),"sourceGeneration":owner.get("sourceGeneration").cloned().unwrap_or(Value::Null),"entryId":owner.get("entryId").cloned().unwrap_or(Value::Null),"seekSerial":owner.get("seekSerial").cloned().unwrap_or(Value::Null)},"events":safe_events});
            if let Some(source_format) = source_format {
                safe["sourceFormat"] = source_format;
            }
            Some(safe)
        }
    };
    let mut output = json!({"engine":"webview","applicable":true,"trackId":nullable_string("trackId")?,"sessionId":nullable_string("sessionId")?,"sourceGeneration":nullable_u64("sourceGeneration")?,"seekSerial":nullable_u64("seekSerial")?,"currentIndex":current_index,"queueTotal":queue_total,"entryId":entry_id,"queueTrackId":queue_track_id,"previewKind":match raw.get("previewKind"){Some(Value::String(v)) if v.len()<=32=>json!(v),Some(Value::Null)=>Value::Null,_=>return Err(ToolError::bad("Invalid media sample previewKind"))},"media":{"present":media_present,"currentTime":current_time,"duration":duration,"paused":nullable_boolean("paused")?,"readyState":ready_state,"networkState":network_state,"errorCode":error_code,"sourcePresent":boolean("sourcePresent")?,"sourceStable":source_stable}});
    // Optional fields preserve compatibility with older frontends while exposing
    // only bounded numeric playback ranges, never source URLs or headers.
    if let Some(value) = media.get("seeking") {
        if !value.is_boolean() && !value.is_null() {
            return Err(ToolError::bad("Invalid media element seeking"));
        }
        output["media"]["seeking"] = value.clone();
    }
    for key in ["buffered", "seekable"] {
        if let Some(value) = media.get(key) {
            let rows = value
                .as_array()
                .filter(|rows| rows.len() <= 8)
                .ok_or_else(|| ToolError::bad("Invalid media element ranges"))?;
            let mut safe = Vec::new();
            for row in rows {
                let pair = row
                    .as_array()
                    .filter(|pair| pair.len() == 2)
                    .ok_or_else(|| ToolError::bad("Invalid media element range"))?;
                let start = pair[0]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.)
                    .ok_or_else(|| ToolError::bad("Invalid media range start"))?;
                let end = pair[1]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= start && *n <= 1.0e9)
                    .ok_or_else(|| ToolError::bad("Invalid media range end"))?;
                safe.push(json!([start, end]));
            }
            output["media"][key] = json!(safe);
        }
    }
    if let Some(diagnostics) = diagnostics {
        output["diagnostics"] = diagnostics;
    }
    Ok(output)
}

fn webview_media_identity_matches(
    before: &Value,
    after: &Value,
    sample: &Value,
    before_entry: &Value,
    after_entry: &Value,
) -> bool {
    [
        "trackId",
        "sessionId",
        "sourceGeneration",
        "seekSerial",
        "currentIndex",
        "queueTotal",
    ]
    .iter()
    .all(|key| before[*key] == after[*key] && before[*key] == sample[*key])
        && before_entry == after_entry
        && sample["entryId"] == *before_entry
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum QueueUiAction {
    Select {
        index: u16,
        modifier: QueueUiModifier,
    },
    Arrow {
        index: u16,
        direction: QueueUiDirection,
    },
    Drag {
        from: u16,
        to: u16,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum QueueUiModifier {
    None,
    Ctrl,
    Shift,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum QueueUiDirection {
    Up,
    Down,
}
fn parse_queue_ui_action(value: &Value) -> Result<QueueUiAction, ToolError> {
    let request: QueueUiAction = serde_json::from_value(value.clone())
        .map_err(|_| ToolError::bad("Invalid finite queue UI action"))?;
    let valid = match request {
        QueueUiAction::Select { index, .. } | QueueUiAction::Arrow { index, .. } => index < 200,
        QueueUiAction::Drag { from, to } => from < 200 && to < 200,
    };
    if !valid {
        return Err(ToolError::bad("Queue UI index exceeds bound"));
    }
    Ok(request)
}
fn queue_ui_profile_allowed(data: &std::path::Path) -> bool {
    let Ok(data) = data.canonicalize() else {
        return false;
    };
    let Some(profile) = data.parent().and_then(std::path::Path::parent) else {
        return false;
    };
    let name = profile
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !cfg!(target_os = "linux")
        || profile.parent() != Some(std::path::Path::new("/tmp"))
        || !name.starts_with("burrata-launch.")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || data != profile.join("data/org.burrata.discovery")
    {
        return false;
    }
    let marker = profile.join(".burrata-isolated-profile");
    std::fs::symlink_metadata(&marker).is_ok_and(|m| {
        m.file_type().is_file() && m.len() == b"burrata isolated memory run\n".len() as u64
    }) && std::fs::read(marker).is_ok_and(|b| b == b"burrata isolated memory run\n")
}
fn queue_ui_snapshot_valid(snapshot: &Value) -> bool {
    let ids = snapshot["queueEntryIds"].as_array();
    snapshot["engine"] == "native"
        && snapshot["status"] == "paused"
        && snapshot["paused"] == true
        && snapshot["generation"].as_u64().is_some()
        && snapshot["queueRevision"].as_u64().is_some()
        && snapshot["entryId"].as_str().is_some()
        && snapshot["currentIndex"]
            .as_u64()
            .zip(ids)
            .is_some_and(|(index, ids)| ids.get(index as usize) == Some(&snapshot["entryId"]))
        && ids.is_some_and(|ids| {
            !ids.is_empty()
                && ids.len() <= 200
                && snapshot["queueTotal"].as_u64() == Some(ids.len() as u64)
                && ids
                    .iter()
                    .all(|id| id.as_str().is_some_and(|id| !id.is_empty()))
                && ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id))
        })
}
fn sanitize_queue_ui_result(value: Value) -> Result<Value, ToolError> {
    let object = value
        .as_object()
        .filter(|o| o.len() == 12)
        .ok_or_else(|| ToolError::bad("Invalid queue UI result"))?;
    let expected = [
        "protocolVersion",
        "action",
        "dispatched",
        "syntheticEvents",
        "trustedGesture",
        "movePending",
        "selectedIndices",
        "focusedIndex",
        "rowCount",
        "errorCode",
        "acknowledgedMoves",
        "mutationState",
    ];
    if object.keys().any(|key| !expected.contains(&key.as_str()))
        || value["protocolVersion"] != 1
        || value["syntheticEvents"] != true
        || value["trustedGesture"] != false
        || !matches!(value["action"].as_str(), Some("select" | "arrow" | "drag"))
        || value["dispatched"].as_bool().is_none()
        || value["movePending"].as_bool().is_none()
    {
        return Err(ToolError::bad("Invalid queue UI flags"));
    }
    let rows = value["rowCount"]
        .as_u64()
        .filter(|n| *n <= 200)
        .ok_or_else(|| ToolError::bad("Invalid queue UI count"))?;
    let indices = value["selectedIndices"]
        .as_array()
        .filter(|v| v.len() <= 200)
        .ok_or_else(|| ToolError::bad("Invalid queue selection"))?;
    if indices.iter().any(|i| i.as_u64().is_none_or(|i| i >= rows))
        || indices
            .windows(2)
            .any(|pair| pair[0].as_u64() >= pair[1].as_u64())
        || (!value["focusedIndex"].is_null()
            && value["focusedIndex"].as_u64().is_none_or(|i| i >= rows))
        || (!value["errorCode"].is_null()
            && !matches!(
                value["errorCode"].as_str(),
                Some(
                    "invalid_request"
                        | "unavailable"
                        | "stale"
                        | "busy"
                        | "unsupported_drag"
                        | "cancelled"
                        | "timeout"
                        | "move_failed"
                )
            ))
    {
        return Err(ToolError::bad("Invalid queue UI bounds"));
    }
    let moves = value["acknowledgedMoves"]
        .as_array()
        .filter(|m| m.len() <= 200)
        .ok_or_else(|| ToolError::bad("Invalid queue move acknowledgements"))?;
    if moves.iter().any(|m| {
        m.as_object()
            .is_none_or(|o| o.len() != 2 || !o.contains_key("from") || !o.contains_key("to"))
            || m["from"].as_u64().is_none_or(|n| n >= rows)
            || m["to"].as_u64().is_none_or(|n| n >= rows)
    }) || !matches!(
        value["mutationState"].as_str(),
        Some("unchanged" | "completed" | "partial_or_unknown")
    ) || (value["mutationState"] == "unchanged" && !moves.is_empty())
        || (value["mutationState"] == "completed" && moves.is_empty())
        || (value["action"] == "select" && !moves.is_empty())
        || (value["mutationState"] == "partial_or_unknown" && value["errorCode"].is_null())
    {
        return Err(ToolError::bad("Invalid queue mutation state"));
    }
    if value["errorCode"].is_null()
        && (value["dispatched"] != true
            || value["movePending"] != false
            || value["mutationState"] == "partial_or_unknown")
    {
        return Err(ToolError::bad("Incomplete queue UI acknowledgement"));
    }
    Ok(value)
}
struct PendingQueueUiGuard<'a> {
    service: &'a AutomationService,
    id: String,
}
impl Drop for PendingQueueUiGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut map) = self.service.pending.lock() {
            map.remove(&self.id);
        }
        if let Ok(mut active) = self.service.queue_ui_active.lock() {
            if active.as_deref() == Some(self.id.as_str()) {
                *active = None;
            }
        }
        let _ = self.service.app.emit(
            "automation-queue-ui-cancelled",
            json!({"requestId":self.id}),
        );
    }
}

fn queued_acceptance_snapshot_matches(receipt: &Value, playback: &Value) -> bool {
    if playback["protocolVersion"] != 1
        || playback["queueUpdate"]["kind"] != "full"
        || playback["failureHistoryUpdate"]["kind"] != "full"
        || playback["queueRevision"] != receipt["queueRevision"]
        || playback["queueEntryIds"] != receipt["queueEntryIds"]
    {
        return false;
    }
    let Some(requested) = receipt["trackIds"].as_array() else {
        return false;
    };
    let Some(queue) = playback["queue"].as_array() else {
        return false;
    };
    if queue.len() != requested.len()
        || !queue
            .iter()
            .zip(requested)
            .all(|(row, id)| row["id"] == *id)
    {
        return false;
    }
    let Some(index) = playback["currentIndex"]
        .as_u64()
        .map(|i| i as usize)
        .filter(|i| *i < requested.len())
    else {
        return false;
    };
    playback["trackId"] == requested[index]
        && playback["entryId"] == receipt["queueEntryIds"][index]
}

fn accept_queued_projection(
    registry: &mut crate::queued_playback::Registry<Value>,
    id: &str,
    transport: &str,
    receipt: Value,
    playback: Value,
    publish: impl FnOnce(Value) -> Value,
) -> Value {
    let rejected = || json!({"accepted":false,"retryable":false});
    let Some(row) = registry.get(id) else {
        return rejected();
    };
    if row.transport != transport || row.status.terminal() {
        return rejected();
    }
    let Some(ids) = row.tracks else {
        return rejected();
    };
    let Some(received) = receipt["trackIds"]
        .as_array()
        .filter(|a| a.len() == ids.len())
    else {
        return rejected();
    };
    let Some(entries) = receipt["queueEntryIds"]
        .as_array()
        .filter(|a| a.len() == ids.len())
    else {
        return rejected();
    };
    let Some(received) = received
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
    else {
        return rejected();
    };
    let Some(entries) = entries
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
    else {
        return rejected();
    };
    if receipt["engine"] != "webview"
        || !crate::queued_playback::valid_webview_receipt(
            &ids,
            crate::queued_playback::ReceiptIdentity {
                tracks: &received,
                entries: &entries,
                revision: receipt["queueRevision"].as_str().unwrap_or(""),
                generation: receipt["acceptedGeneration"].as_u64(),
            },
        )
    {
        return rejected();
    }
    let safe = json!({"operationId":id,"status":"accepted","engine":"webview","trackIds":ids,"queueRevision":receipt["queueRevision"],"queueEntryIds":receipt["queueEntryIds"],"acceptedGeneration":receipt["acceptedGeneration"],"requestedCount":ids.len()});
    if let Some(previous) = row.receipt {
        // An identical retry is idempotent and cannot rewrite a newer projection.
        return json!({"accepted":previous==safe,"retryable":false});
    }
    if !queued_acceptance_snapshot_matches(&safe, &playback) {
        return rejected();
    }
    let ack = publish(playback);
    let published = ack["protocolVersion"] == 1 && ack["accepted"] == true;
    let accepted = published && registry.accept(id, transport, safe);
    json!({"accepted":accepted,"playbackAck":ack,"retryable":!published})
}

struct QueuedPlaybackPrepareGuard<'a> {
    service: &'a AutomationService,
    operation_id: String,
    transport: String,
}
impl Drop for QueuedPlaybackPrepareGuard<'_> {
    fn drop(&mut self) {
        let expired = self
            .service
            .queued_playback
            .lock()
            .map(|mut r| r.expire(&self.operation_id, &self.transport))
            .unwrap_or(false);
        if expired {
            let _ = self.service.app.emit(
                "automation-queued-playback-cancelled",
                json!({"operationId":self.operation_id,"requestId":self.transport}),
            );
        }
    }
}

struct PendingPlaybackGuard<'a> {
    pending: &'a Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    id: String,
}
impl<'a> PendingPlaybackGuard<'a> {
    fn new(service: &'a AutomationService, id: String) -> Self {
        Self {
            pending: &service.pending,
            id,
        }
    }
}
impl Drop for PendingPlaybackGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

struct PendingIdentifyGuard<'a> {
    app: &'a tauri::AppHandle,
    pending: &'a Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    active: &'a Mutex<Option<String>>,
    id: String,
    resolved: bool,
}
impl PendingIdentifyGuard<'_> {
    fn disarm(&mut self) {
        self.resolved = true;
    }
}
impl Drop for PendingIdentifyGuard<'_> {
    fn drop(&mut self) {
        let was_pending = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&self.id))
            .is_some();
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(self.id.as_str()) {
                *active = None;
            }
        }
        if !self.resolved && was_pending {
            let _ = self.app.emit(
                "automation-identify-cancelled",
                json!({"captureId":self.id}),
            );
        }
    }
}

struct PendingSearchUi {
    tx: oneshot::Sender<Result<Value, String>>,
    source_id: String,
    kind: String,
}

struct PendingSearchControl {
    tx: oneshot::Sender<Result<Value, String>>,
    action: String,
    expires_at_ms: u64,
}

fn parse_search_control_arguments(action: &str, arguments: &Value) -> Result<Value, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("Arguments must be an object"))?;
    let allowed: &[&str] = match action {
        "inspect" => &["offset", "limit"],
        "set_continue" => &["revision", "enabled"],
        "play" => &["revision", "occurrenceToken"],
        _ => return Err(ToolError::bad("Unknown search control")),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ToolError::bad("Unexpected search control argument"));
    }
    let mut payload = json!({"action":action});
    match action {
        "inspect" => {
            let offset = match object.get("offset") {
                None => 0,
                Some(v) => v
                    .as_u64()
                    .filter(|v| *v <= 1_000_000)
                    .ok_or_else(|| ToolError::bad("offset must be an integer from 0 to 1000000"))?,
            };
            let limit = match object.get("limit") {
                None => 100,
                Some(v) => v
                    .as_u64()
                    .filter(|v| (1..=200).contains(v))
                    .ok_or_else(|| ToolError::bad("limit must be an integer from 1 to 200"))?,
            };
            payload["offset"] = json!(offset);
            payload["limit"] = json!(limit);
        }
        "set_continue" => {
            let revision = object
                .get("revision")
                .and_then(Value::as_str)
                .filter(|v| is_control_uuid(v))
                .ok_or_else(|| ToolError::bad("valid revision UUID is required"))?;
            let enabled = object
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| ToolError::bad("enabled is required"))?;
            payload["revision"] = json!(revision);
            payload["enabled"] = json!(enabled);
        }
        "play" => {
            let revision = object
                .get("revision")
                .and_then(Value::as_str)
                .filter(|v| is_control_uuid(v))
                .ok_or_else(|| ToolError::bad("valid revision UUID is required"))?;
            let token = object
                .get("occurrenceToken")
                .and_then(Value::as_str)
                .filter(|v| is_control_uuid(v))
                .ok_or_else(|| ToolError::bad("valid occurrenceToken is required"))?;
            payload["revision"] = json!(revision);
            payload["occurrenceToken"] = json!(token);
        }
        _ => return Err(ToolError::bad("Unknown search control")),
    }
    Ok(payload)
}

fn search_control_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn search_control_expired(expires_at_ms: u64, now_ms: u64) -> bool {
    now_ms >= expires_at_ms
}
fn search_control_completion_expired(
    action: &str,
    result: &Value,
    expires_at_ms: u64,
    now_ms: u64,
) -> bool {
    let status = result.get("status").and_then(Value::as_str).unwrap_or("");
    let acted_at_ms = match (action, status) {
        ("inspect", "ready") => result.get("observedAtMs").and_then(Value::as_u64),
        ("set_continue", "complete") => result.get("appliedAtMs").and_then(Value::as_u64),
        ("play", "dispatched") => result.get("dispatchedAtMs").and_then(Value::as_u64),
        _ => None,
    };
    if matches!(
        (action, status),
        ("inspect", "ready") | ("set_continue", "complete") | ("play", "dispatched")
    ) {
        acted_at_ms
            .map(|time| time >= expires_at_ms)
            .unwrap_or(true)
    } else {
        search_control_expired(expires_at_ms, now_ms)
    }
}

fn is_control_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|i| bytes[*i] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| [8, 13, 18, 23].contains(&i) || b.is_ascii_hexdigit())
}

fn sanitize_search_control_result(action: &str, result: &Value) -> Result<Value, String> {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing status".to_string())?;
    if matches!(status, "expired" | "unavailable") {
        return Ok(json!({"status":status}));
    }
    if matches!(status, "failed" | "invalid_request") {
        return Err("search UI action failed".into());
    }
    let revision = result
        .get("revision")
        .and_then(Value::as_str)
        .filter(|v| is_control_uuid(v))
        .ok_or_else(|| "invalid revision".to_string())?;
    match (action, status) {
        ("inspect", "ready") => {
            let query = result["query"]
                .as_str()
                .filter(|v| v.len() <= 500)
                .ok_or_else(|| "invalid query".to_string())?;
            let mode = result["mode"]
                .as_str()
                .filter(|v| matches!(*v, "online" | "library"))
                .ok_or_else(|| "invalid mode".to_string())?;
            let source = result["sourceId"]
                .as_str()
                .filter(|v| SEARCH_UI_SOURCE_IDS.contains(v))
                .ok_or_else(|| "invalid source".to_string())?;
            let kind = result["kind"]
                .as_str()
                .filter(|v| SEARCH_UI_KINDS.contains(v))
                .ok_or_else(|| "invalid kind".to_string())?;
            if (mode == "library" && source != "library")
                || (mode == "online" && source == "library")
            {
                return Err("source/mode mismatch".into());
            }
            let total = result["total"]
                .as_u64()
                .filter(|v| *v <= 1_000_000)
                .ok_or_else(|| "invalid total".to_string())?;
            let offset = result["offset"]
                .as_u64()
                .filter(|v| *v <= 1_000_000)
                .ok_or_else(|| "invalid offset".to_string())?;
            let rows = result["rows"]
                .as_array()
                .filter(|v| v.len() <= 200)
                .ok_or_else(|| "invalid rows".to_string())?;
            let mut safe_rows = Vec::with_capacity(rows.len());
            for row in rows {
                let token = row["occurrenceToken"]
                    .as_str()
                    .filter(|v| is_control_uuid(v))
                    .ok_or_else(|| "invalid token".to_string())?;
                let row_source = row["sourceId"]
                    .as_str()
                    .filter(|v| SEARCH_UI_SOURCE_IDS.contains(v) && *v != "all")
                    .ok_or_else(|| "invalid row source".to_string())?;
                if source != "all" && row_source != source {
                    return Err("result source mismatch".into());
                }
                let title = row["title"]
                    .as_str()
                    .filter(|v| v.len() <= 300)
                    .ok_or_else(|| "invalid title".to_string())?;
                let artist = row["artist"]
                    .as_str()
                    .filter(|v| v.len() <= 300)
                    .ok_or_else(|| "invalid artist".to_string())?;
                let duration = match &row["durationSeconds"] {
                    Value::Null => Value::Null,
                    Value::Number(n) if n.as_f64().is_some_and(|v| v.is_finite() && v > 0.0) => {
                        json!(n.as_f64().unwrap())
                    }
                    _ => return Err("invalid duration".into()),
                };
                safe_rows.push(json!({"occurrenceToken":token,"sourceId":row_source,"title":title,"artist":artist,"durationSeconds":duration}));
            }
            let has_more = result["hasMore"]
                .as_bool()
                .ok_or_else(|| "invalid hasMore".to_string())?;
            let collecting = result["collecting"]
                .as_bool()
                .ok_or_else(|| "invalid collecting".to_string())?;
            let continuation = result["continueSearch"]
                .as_bool()
                .ok_or_else(|| "invalid continuation".to_string())?;
            let query_pending = result["queryInputPending"]
                .as_bool()
                .ok_or_else(|| "invalid query state".to_string())?;
            Ok(
                json!({"status":"ready","revision":revision,"query":query,"queryInputPending":query_pending,"mode":mode,"sourceId":source,"kind":kind,"continueSearch":continuation,"total":total,"offset":offset,"hasMore":has_more,"collecting":collecting,"rows":safe_rows}),
            )
        }
        ("set_continue", "complete") => {
            let enabled = result["enabled"]
                .as_bool()
                .ok_or_else(|| "invalid preference".to_string())?;
            Ok(json!({"status":"complete","revision":revision,"enabled":enabled}))
        }
        ("set_continue", "stale") => Ok(json!({"status":"stale","revision":revision})),
        ("play", "dispatched") => {
            let source = result["sourceId"]
                .as_str()
                .filter(|v| SEARCH_UI_SOURCE_IDS.contains(v) && *v != "all")
                .ok_or_else(|| "invalid source".to_string())?;
            Ok(json!({"status":"dispatched","revision":revision,"sourceId":source}))
        }
        ("play", "stale") => Ok(json!({"status":"stale","revision":revision})),
        _ => Err("unexpected search control completion".into()),
    }
}

struct PendingSearchControlGuard<'a> {
    app: &'a tauri::AppHandle,
    pending: &'a Mutex<HashMap<String, PendingSearchControl>>,
    active: &'a Mutex<Option<String>>,
    id: String,
    resolved: bool,
}

impl Drop for PendingSearchControlGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(self.id.as_str()) {
                *active = None;
            }
        }
        if !self.resolved {
            let _ = self.app.emit(
                "automation-search-controls-cancelled",
                json!({"requestId":self.id}),
            );
        }
    }
}

struct PendingSearchUiGuard<'a> {
    app: Option<&'a tauri::AppHandle>,
    pending: &'a Mutex<HashMap<String, PendingSearchUi>>,
    active: &'a Mutex<Option<String>>,
    id: String,
    resolved: bool,
}

impl PendingSearchUiGuard<'_> {
    fn disarm(&mut self) {
        self.resolved = true;
    }
}

impl Drop for PendingSearchUiGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(self.id.as_str()) {
                *active = None;
            }
        }
        if !self.resolved {
            if let Some(app) = self.app {
                let _ = app.emit(
                    "automation-search-ui-cancelled",
                    json!({"requestId":self.id}),
                );
            }
        }
    }
}

struct PendingPlayerPanel {
    tx: oneshot::Sender<Result<Value, String>>,
    panel: PlayerPanel,
}
struct PendingUiNavigation {
    tx: oneshot::Sender<Result<Value, String>>,
    target: UiNavigationTarget,
}

struct PendingUiNavigationGuard<'a> {
    app: &'a tauri::AppHandle,
    pending: &'a Mutex<HashMap<String, PendingUiNavigation>>,
    active: &'a Mutex<Option<String>>,
    id: String,
    resolved: bool,
}
impl PendingUiNavigationGuard<'_> {
    fn disarm(&mut self) {
        self.resolved = true;
    }
}
impl Drop for PendingUiNavigationGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(self.id.as_str()) {
                *active = None;
            }
        }
        if !self.resolved {
            let _ = self.app.emit(
                "automation-ui-navigation-cancelled",
                json!({"requestId":self.id}),
            );
        }
    }
}

struct PendingPlayerPanelGuard<'a> {
    pending: &'a Mutex<HashMap<String, PendingPlayerPanel>>,
    active: &'a Mutex<Option<String>>,
    id: String,
}

impl Drop for PendingPlayerPanelGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
        if let Ok(mut active) = self.active.lock() {
            if active.as_deref() == Some(self.id.as_str()) {
                *active = None;
            }
        }
    }
}

const PLAYBACK_PUBLISHER_PROTOCOL: u64 = 1;
const MAX_RETIRED_PLAYBACK_CLIENTS: usize = 16;

#[derive(Default)]
struct PlaybackPublisherProtocol {
    active_client: Option<String>,
    last_sequence: u64,
    queue_revision: Option<String>,
    failure_history_revision: Option<String>,
    retired_clients: VecDeque<String>,
}

struct WebViewPlaybackState {
    fields: Value,
    queue: Arc<Vec<Value>>,
    queue_entry_ids: Arc<Vec<Value>>,
}

impl Default for WebViewPlaybackState {
    fn default() -> Self {
        Self {
            fields: json!({"status":"idle"}),
            queue: Arc::new(Vec::new()),
            queue_entry_ids: Arc::new(Vec::new()),
        }
    }
}

impl WebViewPlaybackState {
    fn watch_snapshot(&self) -> Value {
        // Watch consumers need control state, not the explicit status/query ledger.
        // Skip its Value before cloning any diagnostics, rather than clone then remove.
        let mut snapshot = if let Some(fields) = self.fields.as_object() {
            let mut projection = serde_json::Map::new();
            for (key, value) in fields {
                let value = if key == "diagnostics" {
                    if let Some(fields) = value.as_object() {
                        let mut diagnostics = serde_json::Map::new();
                        for (key, value) in fields {
                            if key != "recentTrackFailures" {
                                diagnostics.insert(key.clone(), value.clone());
                            }
                        }
                        Value::Object(diagnostics)
                    } else {
                        value.clone()
                    }
                } else {
                    value.clone()
                };
                projection.insert(key.clone(), value);
            }
            Value::Object(projection)
        } else {
            self.fields.clone()
        };
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("queueTotal".into(), json!(self.queue.len()));
        }
        snapshot
    }

    fn page(&self, offset: usize, limit: usize) -> Value {
        let limit = limit.clamp(1, 200);
        let total = self.queue.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit).min(total);
        let mut page = self.fields.clone();
        if !page.is_object() {
            page = json!({});
        }
        if let Some(object) = page.as_object_mut() {
            object.insert("queue".into(), json!(self.queue[start..end]));
            object.insert(
                "queueEntryIds".into(),
                json!(self.queue_entry_ids.get(start..end).unwrap_or(&[])),
            );
            object.insert("queueTotal".into(), json!(total));
            object.insert("queueOffset".into(), json!(start));
            object.insert("queueLimit".into(), json!(limit));
            object.insert("queueHasMore".into(), json!(end < total));
        }
        page
    }
}

fn playback_update_ack(
    accepted: bool,
    need_full: bool,
    failure_history_revision: Option<&str>,
) -> Value {
    let mut ack = json!({"protocolVersion": PLAYBACK_PUBLISHER_PROTOCOL, "accepted": accepted, "needFull": need_full});
    if accepted {
        if let Some(revision) = failure_history_revision {
            ack["failureHistoryRevision"] = json!(revision);
        }
    }
    ack
}

/// Accept only the bounded failure DTO; never forward arbitrary frontend metadata.
fn sanitize_recent_track_failures(raw: &Value, revision: Option<&Value>) -> Option<Value> {
    use crate::playback::{
        safe_failure_identity, safe_failure_text, track_failure_category, MAX_RECENT_TRACK_FAILURES,
    };
    let failures = raw.as_array()?;
    if failures.len() > MAX_RECENT_TRACK_FAILURES {
        return None;
    }
    let mut safe = Vec::with_capacity(failures.len());
    for failure in failures {
        let object = failure.as_object()?;
        let queue_revision = object.get("queueRevision")?;
        if Some(queue_revision) != revision
            || !(queue_revision.as_u64().is_some()
                || queue_revision
                    .as_str()
                    .is_some_and(|value| value.len() <= 128))
        {
            return None;
        }
        let generation = object.get("generation")?.as_u64()?;
        let entry_id = safe_failure_identity(object.get("entryId")?.as_str()?)?;
        let outcome = object.get("outcome")?.as_str()?;
        if !["skipped", "queue_exhausted"].contains(&outcome) {
            return None;
        }
        let reason = object.get("reason")?.as_str()?;
        if reason.len() > 2048 {
            return None;
        }
        let mut record = json!({"queueRevision":queue_revision,"generation":generation,"entryId":entry_id,"outcome":outcome,"reason":safe_failure_text(reason,250),"category":track_failure_category(reason)});
        for key in ["trackId", "provider", "providerId", "operationId"] {
            record[key] = json!(object
                .get(key)
                .and_then(Value::as_str)
                .and_then(safe_failure_identity));
        }
        for (key, maximum) in [("title", 160), ("artist", 160), ("version", 120)] {
            record[key] = match object.get(key) {
                None | Some(Value::Null) => Value::Null,
                Some(Value::String(value)) if value.len() <= 2048 => {
                    json!(safe_failure_text(value, maximum))
                }
                _ => return None,
            };
        }
        safe.push(record);
    }
    Some(json!(safe))
}

fn replace_playback_fields(state: &mut WebViewPlaybackState, incoming: &mut Value) {
    // Capture the protocol's queue owner before removing transport fields.
    let failures = incoming
        .get("diagnostics")
        .and_then(|value| value.get("recentTrackFailures"))
        .and_then(|raw| sanitize_recent_track_failures(raw, incoming.get("queueRevision")));
    replace_playback_fields_with_history(state, incoming, failures);
}

fn replace_playback_fields_with_history(
    state: &mut WebViewPlaybackState,
    incoming: &mut Value,
    failures: Option<Value>,
) {
    if let Some(object) = incoming.as_object_mut() {
        if !object.get("diagnostics").is_some_and(Value::is_object) && failures.is_some() {
            object.insert("diagnostics".into(), json!({}));
        }
        if let Some(diagnostics) = object.get_mut("diagnostics").and_then(Value::as_object_mut) {
            diagnostics.remove("recentTrackFailures");
            if let Some(failures) = failures {
                diagnostics.insert("recentTrackFailures".into(), failures);
            }
        }
        object.remove("queue");
        object.remove("queueEntryIds");
        for key in [
            "protocolVersion",
            "clientId",
            "stateSequence",
            "queueRevision",
            "queueUpdate",
            "failureHistoryRevision",
            "failureHistoryUpdate",
        ] {
            object.remove(key);
        }
        object.insert("queueTotal".into(), json!(state.queue.len()));
        state.fields = Value::Object(std::mem::take(object));
    } else {
        state.fields = json!({"status":"idle","queueTotal":state.queue.len()});
    }
}

fn replace_playback_queue(
    state: &mut WebViewPlaybackState,
    incoming: &mut Value,
    same_revision: bool,
) -> bool {
    let Some(object) = incoming.as_object_mut() else {
        return false;
    };
    let Some(Value::Array(queue)) = object.remove("queue") else {
        return false;
    };
    let entry_ids = match object.remove("queueEntryIds") {
        Some(Value::Array(values)) => values,
        _ => Vec::new(),
    };
    if !same_revision {
        state.queue = Arc::new(queue);
        state.queue_entry_ids = Arc::new(entry_ids);
    }
    true
}

/// Mutates only compact control fields on unchanged queue pulses; queue pages remain Arc-backed.
fn accept_playback_update(
    protocol: &mut PlaybackPublisherProtocol,
    current: &mut WebViewPlaybackState,
    mut incoming: Value,
) -> (bool, bool) {
    let Some(object) = incoming.as_object() else {
        return (false, true);
    };
    if object.get("protocolVersion").and_then(Value::as_u64) != Some(PLAYBACK_PUBLISHER_PROTOCOL) {
        if protocol.active_client.is_some() {
            return (false, false);
        }
        if let Some(Value::Array(queue)) = incoming.get_mut("queue").and_then(|value| {
            if value.is_array() {
                Some(std::mem::replace(value, Value::Null))
            } else {
                None
            }
        }) {
            current.queue = Arc::new(queue);
            if let Some(Value::Array(ids)) = incoming.get_mut("queueEntryIds").and_then(|value| {
                if value.is_array() {
                    Some(std::mem::replace(value, Value::Null))
                } else {
                    None
                }
            }) {
                current.queue_entry_ids = Arc::new(ids);
            }
        }
        replace_playback_fields(current, &mut incoming);
        return (true, false);
    }
    let Some(client) = object
        .get("clientId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_owned)
    else {
        return (false, true);
    };
    let Some(sequence) = object.get("stateSequence").and_then(Value::as_u64) else {
        return (false, true);
    };
    let Some(revision) = object
        .get("queueRevision")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 128)
        .map(str::to_owned)
    else {
        return (false, true);
    };
    let Some(queue_update) = object.get("queueUpdate") else {
        return (false, true);
    };
    let kind = queue_update
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let update_revision = queue_update
        .get("queueRevision")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let new_client = protocol.active_client.as_deref() != Some(client.as_str());
    if new_client {
        if protocol
            .retired_clients
            .iter()
            .any(|retired| retired == &client)
            || kind.as_deref() != Some("full")
        {
            return (false, true);
        }
    } else if sequence <= protocol.last_sequence {
        return (false, false);
    }
    match kind.as_deref() {
        Some("full") => {
            if object.get("queue").and_then(Value::as_array).is_none() {
                return (false, true);
            }
        }
        Some("unchanged") => {
            if new_client
                || protocol.queue_revision.as_deref() != Some(revision.as_str())
                || update_revision.as_deref() != Some(revision.as_str())
            {
                return (false, true);
            }
        }
        _ => return (false, true),
    }
    // Validate the history owner and revision before mutating queue, fields, or protocol state.
    // Old publishers may omit the optional extension; they retain full-snapshot behavior.
    let failure_history = match (
        object.get("failureHistoryRevision"),
        object.get("failureHistoryUpdate"),
    ) {
        (None, None) => None,
        (Some(history_revision), Some(update)) => {
            let Some(history_revision) = history_revision.as_str().filter(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_:".contains(&byte))
            }) else {
                return (false, true);
            };
            let raw = object
                .get("diagnostics")
                .and_then(|value| value.get("recentTrackFailures"));
            match update.get("kind").and_then(Value::as_str) {
                Some("full") => {
                    let Some(failures) = raw.and_then(|raw| {
                        sanitize_recent_track_failures(raw, object.get("queueRevision"))
                    }) else {
                        return (false, true);
                    };
                    Some((history_revision.to_owned(), Some(failures)))
                }
                Some("unchanged") => {
                    if new_client
                        || kind.as_deref() == Some("full")
                        || protocol.queue_revision.as_deref() != Some(revision.as_str())
                        || protocol.failure_history_revision.as_deref() != Some(history_revision)
                        || update.get("failureHistoryRevision").and_then(Value::as_str)
                            != Some(history_revision)
                        || raw.is_some()
                        || !current.fields["diagnostics"]["recentTrackFailures"].is_array()
                    {
                        return (false, true);
                    }
                    Some((history_revision.to_owned(), None))
                }
                _ => return (false, true),
            }
        }
        _ => return (false, true),
    };
    if kind.as_deref() == Some("full") {
        let same_revision =
            !new_client && protocol.queue_revision.as_deref() == Some(revision.as_str());
        if !replace_playback_queue(current, &mut incoming, same_revision) {
            return (false, true);
        }
    }
    if new_client {
        if let Some(previous) = protocol.active_client.replace(client) {
            protocol.retired_clients.push_back(previous);
            while protocol.retired_clients.len() > MAX_RETIRED_PLAYBACK_CLIENTS {
                protocol.retired_clients.pop_front();
            }
        }
    }
    protocol.last_sequence = sequence;
    protocol.queue_revision = Some(revision);
    if let Some((history_revision, failures)) = failure_history {
        // Move the retained Value on unchanged pulses; no history allocation, clone, or scan.
        let failures = failures.or_else(|| {
            current
                .fields
                .get_mut("diagnostics")
                .and_then(|value| value.get_mut("recentTrackFailures"))
                .map(Value::take)
        });
        protocol.failure_history_revision = Some(history_revision);
        replace_playback_fields_with_history(current, &mut incoming, failures);
    } else {
        protocol.failure_history_revision = None;
        replace_playback_fields(current, &mut incoming);
    }
    (true, false)
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
    playback_state: Mutex<WebViewPlaybackState>,
    playback_publisher: Mutex<PlaybackPublisherProtocol>,
    playback_updates: tokio::sync::watch::Sender<Value>,
    playback: OnceLock<Arc<PlaybackService>>,
    queued_playback: Arc<Mutex<crate::queued_playback::Registry<Value>>>,
    queued_notify: Arc<tokio::sync::Notify>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    identify_pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    identify_active: Mutex<Option<String>>,
    search_ui_pending: Mutex<HashMap<String, PendingSearchUi>>,
    search_ui_active: Mutex<Option<String>>,
    search_control_pending: Mutex<HashMap<String, PendingSearchControl>>,
    search_control_active: Mutex<Option<String>>,
    player_panel_pending: Mutex<HashMap<String, PendingPlayerPanel>>,
    player_panel_active: Mutex<Option<String>>,
    queue_ui_active: Mutex<Option<String>>,
    ui_navigation_pending: Mutex<HashMap<String, PendingUiNavigation>>,
    ui_navigation_active: Mutex<Option<String>>,
    frontend_ready: AtomicBool,
    autotag: Mutex<Option<Arc<crate::autotag::AutoTagger>>>,
    remote_playlists:
        std::sync::RwLock<Option<Arc<crate::remote_playlists::RemotePlaylistService>>>,
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
            playback_state: Mutex::new(WebViewPlaybackState::default()),
            playback_publisher: Mutex::new(PlaybackPublisherProtocol::default()),
            playback_updates: tokio::sync::watch::channel(json!({"status":"idle","queueTotal":0}))
                .0,
            playback: OnceLock::new(),
            queued_playback: Arc::new(Mutex::new(crate::queued_playback::Registry::new(128))),
            queued_notify: Arc::new(tokio::sync::Notify::new()),
            pending: Mutex::new(HashMap::new()),
            identify_pending: Mutex::new(HashMap::new()),
            identify_active: Mutex::new(None),
            search_ui_pending: Mutex::new(HashMap::new()),
            search_ui_active: Mutex::new(None),
            search_control_pending: Mutex::new(HashMap::new()),
            search_control_active: Mutex::new(None),
            player_panel_pending: Mutex::new(HashMap::new()),
            player_panel_active: Mutex::new(None),
            queue_ui_active: Mutex::new(None),
            ui_navigation_pending: Mutex::new(HashMap::new()),
            ui_navigation_active: Mutex::new(None),
            frontend_ready: AtomicBool::new(false),
            autotag: Mutex::new(None),
            remote_playlists: std::sync::RwLock::new(None),
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
    pub fn set_remote_service(&self, service: Arc<crate::remote_playlists::RemotePlaylistService>) {
        if let Ok(mut slot) = self.remote_playlists.write() {
            *slot = Some(service);
        }
    }
    fn remote_service(
        &self,
    ) -> Result<Arc<crate::remote_playlists::RemotePlaylistService>, ToolError> {
        self.remote_playlists
            .read()
            .map_err(|_| ToolError::failed("Remote playlist service is unavailable"))?
            .clone()
            .ok_or_else(|| ToolError::failed("Remote playlist service is unavailable"))
    }
    pub fn set_playback_service(&self, playback: Arc<PlaybackService>) {
        let _ = self.playback.set(playback);
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/automation/tools", get(list_tools))
            .route("/automation/call", post(call_tool))
            .with_state(self.clone())
            .layer(DefaultBodyLimit::max(AUTOMATION_REQUEST_BODY_LIMIT))
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
        let state = self.playback_snapshot();
        let mut status = json!({"running":true,"permissions":{"read":p.read,"control":p.control,"manage":p.manage},"playback":state.clone()});
        if self.native_playback_enabled() {
            if let Some(object) = status.as_object_mut() {
                if let Some(engine) = state.get("engine") {
                    object.insert("playbackEngine".into(), engine.clone());
                }
                if let Some(quality) = state.get("quality") {
                    object.insert("playbackQuality".into(), quality.clone());
                }
            }
        }
        if let Some(app_state) = self.app.try_state::<crate::AppState>() {
            let managed = app_state.managed_provider_session.status();
            if let Some(object) = status.as_object_mut() {
                object.insert(
                    "managedProviderSession".into(),
                    managed_provider_qualification_status(&managed),
                );
                if let Some(metrics) = app_state.store.connection_wait_metrics() {
                    object.insert("diagnostics".into(), json!({"storeConnectionWait":metrics}));
                }
            }
        }
        status
    }
    pub fn update_playback_state(&self, state: Value) -> Value {
        if self.native_playback_enabled() {
            return playback_update_ack(false, false, None);
        }
        let Ok(mut protocol) = self.playback_publisher.lock() else {
            return playback_update_ack(false, true, None);
        };
        let Ok(mut current) = self.playback_state.lock() else {
            return playback_update_ack(false, true, None);
        };
        let (accepted, need_full) = accept_playback_update(&mut protocol, &mut current, state);
        if accepted {
            self.playback_updates.send_replace(current.watch_snapshot());
        }
        playback_update_ack(
            accepted,
            need_full,
            protocol.failure_history_revision.as_deref(),
        )
    }
    pub fn playback_snapshot(&self) -> Value {
        self.playback_snapshot_page(0, 50)
    }
    /// Minimal metadata used to compare WebView source identity without paging queue entries.
    pub fn playback_tracking_snapshot(&self) -> Value {
        if let Some(playback) = self
            .playback
            .get()
            .filter(|playback| playback.native_enabled())
        {
            let mut snapshot = playback.snapshot_page(0, 1);
            strip_native_playback_fields(&mut snapshot);
            return snapshot;
        }
        self.playback_state
            .lock()
            .map(|state| state.watch_snapshot())
            .unwrap_or_else(|_| json!({"status":"idle"}))
    }
    pub fn playback_snapshot_page(&self, offset: usize, limit: usize) -> Value {
        if let Some(playback) = self.playback.get().filter(|p| p.native_enabled()) {
            let mut snapshot = playback.snapshot_page(offset, limit);
            strip_native_playback_fields(&mut snapshot);
            return snapshot;
        }
        self.playback_state
            .lock()
            .map(|state| state.page(offset,limit))
            .unwrap_or_else(|_| json!({"status":"idle","queue":[],"queueEntryIds":[],"queueTotal":0,"queueOffset":0,"queueLimit":limit.clamp(1,200),"queueHasMore":false}))
    }
    pub fn subscribe_playback_state(&self) -> tokio::sync::watch::Receiver<Value> {
        if let Some(playback) = self.playback.get().filter(|p| p.native_enabled()) {
            return playback.subscribe();
        }
        self.playback_updates.subscribe()
    }
    fn native_playback_enabled(&self) -> bool {
        self.playback
            .get()
            .is_some_and(|playback| playback.native_enabled())
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
    pub fn set_frontend_ready(&self, ready: bool) {
        self.frontend_ready.store(ready, Ordering::Release);
    }
    async fn resize_app_window(&self, arguments: &Value) -> Result<Value, ToolError> {
        let request = parse_resize_window_request(arguments)?;
        let window = self
            .app
            .get_webview_window("main")
            .ok_or_else(|| ToolError::failed("Main application window is unavailable"))?;
        let before_maximized = window
            .is_maximized()
            .map_err(|error| ToolError::failed(error.to_string()))?;
        let before_size = logical_window_size(&window)?;
        let restore_maximized = request.restore_maximized.unwrap_or(before_maximized);

        if before_maximized {
            window
                .unmaximize()
                .map_err(|error| ToolError::failed(error.to_string()))?;
        }
        if let Err(error) = window.set_size(tauri::LogicalSize::new(
            request.width as f64,
            request.height as f64,
        )) {
            if restore_maximized {
                let _ = window.maximize();
            }
            return Err(ToolError::failed(error.to_string()));
        }
        if restore_maximized {
            window
                .maximize()
                .map_err(|error| ToolError::failed(error.to_string()))?;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let after_size = logical_window_size(&window)?;
        let after_maximized = window
            .is_maximized()
            .map_err(|error| ToolError::failed(error.to_string()))?;
        Ok(json!({
            "before": {"innerSize": before_size, "maximized": before_maximized},
            "after": {"innerSize": after_size, "maximized": after_maximized}
        }))
    }
    pub(crate) fn complete_layout_request(&self, id: &str, report: Value) {
        let result = if id.starts_with("queue-ui:") {
            sanitize_queue_ui_result(report)
        } else if id.starts_with("webview-media:") {
            sanitize_webview_media_inspection(report)
        } else {
            sanitize_playback_layout(report).map(|mut report| {
            let snapshot = self.playback_snapshot_page(0, 1);
            let own_directory = self.app.path().app_data_dir().ok().map(|path| path.join("artwork"));
            let cached_path = snapshot["track"]["artworkFilePath"].as_str().map(std::path::Path::new);
            let in_own_directory = cached_path.zip(own_directory.as_ref()).is_some_and(|(path, directory)| path.parent() == Some(directory.as_path()));
            let scope = self.app.asset_protocol_scope();
            report["nativeArtwork"] = json!({
                "cachedPathPresent":cached_path.is_some(),
                "cachedPathInOwnDirectory":in_own_directory,
                "cachedFileExists":in_own_directory && cached_path.is_some_and(std::path::Path::is_file),
                "cachedFileGranted":in_own_directory && cached_path.is_some_and(|path|scope.is_allowed(path)),
                "cacheDirectoryGranted":own_directory.is_some_and(|path|scope.is_allowed(path.join("scope-check.jpg")))
            });
            report
            })
        }.map_err(|error| error.message);
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(tx) = pending.remove(id) {
                let _ = tx.send(result);
            }
        }
    }
    async fn inspect_playback_layout(&self) -> Result<Value, ToolError> {
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let snapshot = self.playback_snapshot_page(0, 1);
        let artwork_path = snapshot["track"]["artworkFilePath"]
            .as_str()
            .map(std::path::PathBuf::from);
        let artwork_observation =
            crate::artwork_resource_probe::start(&self.app, artwork_path).await;
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| ToolError::failed("Player layout inspection is unavailable"))?
            .insert(id.clone(), tx);
        let _pending_guard = PendingPlaybackGuard::new(self, id.clone());
        if let Err(error) = self.app.emit(
            "automation-inspect-playback-layout",
            json!({"requestId":id}),
        ) {
            self.remove_pending_playback(&id);
            return Err(ToolError::failed(error.to_string()));
        }
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(mut layout))) => {
                artwork_observation.wait_until_finished().await;
                layout["artworkResource"] = artwork_observation.snapshot();
                Ok(layout)
            }
            Ok(Ok(Err(error))) => Err(ToolError::failed(error)),
            Ok(Err(_)) => Err(ToolError::failed("Player layout response channel closed")),
            Err(_) => {
                self.remove_pending_playback(&id);
                Err(ToolError {
                    code: "TIMEOUT",
                    message: "Player layout inspection timed out".into(),
                })
            }
        }
    }
    async fn inspect_webview_media(&self) -> Result<Value, ToolError> {
        if self
            .playback
            .get()
            .is_some_and(|playback| playback.native_enabled())
        {
            return Ok(
                json!({"engine":"native","applicable":false,"reason":"webview_media_element_not_applicable"}),
            );
        }
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let snapshot = self.playback_snapshot_page(0, 1);
        let track_id = snapshot["trackId"].as_str().map(str::to_owned);
        let session_id = snapshot["sessionId"].as_str().map(str::to_owned);
        let generation = snapshot["sourceGeneration"].as_u64();
        let seek_serial = snapshot["seekSerial"].as_u64().unwrap_or(0);
        let current_index = snapshot["currentIndex"].as_i64().unwrap_or(-1);
        let queue_total = snapshot["queueTotal"].as_u64().unwrap_or(0);
        let occurrence_page = if current_index >= 0 {
            self.playback_snapshot_page(current_index as usize, 1)
        } else {
            Value::Null
        };
        let queue_track_id = occurrence_page["queue"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item["id"].as_str())
            .map(str::to_owned);
        let entry_id = occurrence_page["queueEntryIds"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = format!("webview-media:{}", Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| ToolError::failed("Media inspection is unavailable"))?
            .insert(id.clone(), tx);
        let _pending_guard = PendingPlaybackGuard::new(self, id.clone());
        let request = json!({"requestId":id,"trackId":track_id,"sessionId":session_id,"sourceGeneration":generation,"seekSerial":seek_serial,"currentIndex":current_index,"queueTotal":queue_total,"entryId":entry_id,"queueTrackId":queue_track_id});
        if let Err(error) = self.app.emit("automation-inspect-webview-media", request) {
            self.remove_pending_playback(&id);
            return Err(ToolError::failed(error.to_string()));
        }
        let raw = match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(value))) => value,
            Ok(Ok(Err(error))) => return Err(ToolError::failed(error)),
            Ok(Err(_)) => {
                return Err(ToolError::failed(
                    "Media inspection response channel closed",
                ))
            }
            Err(_) => {
                self.remove_pending_playback(&id);
                return Err(ToolError {
                    code: "TIMEOUT",
                    message: "WebView media inspection timed out".into(),
                });
            }
        };
        let after = self.playback_snapshot_page(0, 1);
        let after_entry = if current_index >= 0 {
            self.playback_snapshot_page(current_index as usize, 1)["queueEntryIds"]
                .as_array()
                .and_then(|items| items.first())
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let after_queue_track_id = if current_index >= 0 {
            self.playback_snapshot_page(current_index as usize, 1)["queue"]
                .as_array()
                .and_then(|items| items.first())
                .and_then(|item| item["id"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        if !webview_media_identity_matches(
            &snapshot,
            &after,
            &raw,
            &json!(entry_id),
            &json!(after_entry),
        ) || raw["queueTrackId"] != json!(queue_track_id)
            || after_queue_track_id != queue_track_id
        {
            return Err(ToolError {
                code: "STALE_MEDIA_SAMPLE",
                message: "Playback changed during WebView media inspection".into(),
            });
        }
        Ok(raw)
    }
    pub(crate) fn complete_player_panel_request(&self, id: &str, panel: &str, status: &str) {
        let pending = self
            .player_panel_pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(id));
        if let Some(pending) = pending {
            let result = sanitize_player_panel_completion(pending.panel, panel, status);
            let _ = pending.tx.send(result);
        }
        if let Ok(mut active) = self.player_panel_active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
    }

    async fn interact_queue_ui(&self, arguments: &Value) -> Result<Value, ToolError> {
        let action = parse_queue_ui_action(arguments)?;
        let data = self
            .app
            .path()
            .app_data_dir()
            .map_err(|_| ToolError::failed("App data is unavailable"))?;
        if !queue_ui_profile_allowed(&data) {
            return Err(ToolError {
                code: "ISOLATED_PROFILE_REQUIRED",
                message: "Queue UI verification requires a marked isolated profile".into(),
            });
        }
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "Player UI is not ready".into(),
            });
        }
        let playback = self
            .playback
            .get()
            .filter(|p| p.native_enabled())
            .ok_or_else(|| ToolError::bad("Native paused queue required"))?;
        let before = playback.snapshot_page(0, 200);
        if !queue_ui_snapshot_valid(&before) {
            return Err(ToolError::bad(
                "Distinct paused native queue occurrences required",
            ));
        }
        let id = format!("queue-ui:{}", Uuid::new_v4());
        {
            let mut active = self
                .queue_ui_active
                .lock()
                .map_err(|_| ToolError::failed("Queue UI control unavailable"))?;
            if active.is_some() {
                return Err(ToolError {
                    code: "BUSY",
                    message: "Queue UI interaction already pending".into(),
                });
            }
            *active = Some(id.clone());
        }
        let _guard = PendingQueueUiGuard {
            service: self,
            id: id.clone(),
        };
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| ToolError::failed("Queue UI response unavailable"))?
            .insert(id.clone(), tx);
        self.app.emit("automation-queue-ui",json!({"requestId":id,"action":action,"context":{"generation":before["generation"],"queueRevision":before["queueRevision"],"entryId":before["entryId"],"trackId":before["trackId"],"rowCount":before["queueTotal"]}})).map_err(|_|ToolError::failed("Queue UI dispatch unavailable"))?;
        let result = match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(result))) => result,
            Ok(Ok(Err(_))) => return Err(ToolError::failed("Queue UI completion invalid")),
            Ok(Err(_)) => return Err(ToolError::failed("Queue UI response closed")),
            Err(_) => {
                return Err(ToolError {
                    code: "TIMEOUT",
                    message: "Queue UI interaction timed out; state must be inspected before retry"
                        .into(),
                })
            }
        };
        if result["action"] != arguments["action"] || result["rowCount"] != before["queueTotal"] {
            return Err(ToolError::bad("Queue UI completion mismatched"));
        }
        let after = playback.snapshot_page(0, 200);
        let mut expected = before["queueEntryIds"].as_array().unwrap().clone();
        let moves = result["acknowledgedMoves"].as_array().unwrap();
        for movement in moves {
            let entry = expected.remove(movement["from"].as_u64().unwrap() as usize);
            expected.insert(movement["to"].as_u64().unwrap() as usize, entry);
        }
        if !queue_ui_snapshot_valid(&after)
            || after["generation"] != before["generation"]
            || after["entryId"] != before["entryId"]
            || after["position"] != before["position"]
            || after["volume"] != before["volume"]
            || after["queueEntryIds"].as_array() != Some(&expected)
            || after["queueRevision"].as_u64()
                != before["queueRevision"]
                    .as_u64()
                    .and_then(|r| r.checked_add(moves.len() as u64))
        {
            return Err(ToolError {
                code: "STALE_QUEUE_UI",
                message: "Queue changed during interaction; partial mutation is possible, inspect before retry".into(),
            });
        }
        Ok(result)
    }

    async fn set_player_panel(&self, panel: PlayerPanel) -> Result<Value, ToolError> {
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut active = self
                .player_panel_active
                .lock()
                .map_err(|_| ToolError::failed("Player panel control is unavailable"))?;
            if active.is_some() {
                return Err(ToolError {
                    code: "BUSY",
                    message: "A player panel request is already running".into(),
                });
            }
            self.player_panel_pending
                .lock()
                .map_err(|_| ToolError::failed("Player panel control is unavailable"))?
                .insert(id.clone(), PendingPlayerPanel { tx, panel });
            *active = Some(id.clone());
        }
        let _guard = PendingPlayerPanelGuard {
            pending: &self.player_panel_pending,
            active: &self.player_panel_active,
            id: id.clone(),
        };
        self.app
            .emit(
                "automation-player-panel",
                json!({"requestId":id,"panel":panel.as_str()}),
            )
            .map_err(|error| ToolError::failed(error.to_string()))?;
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(_))) => Err(ToolError::failed("Player panel response was invalid")),
            Ok(Err(_)) => Err(ToolError::failed("Player panel response channel closed")),
            Err(_) => Err(ToolError {
                code: "TIMEOUT",
                message: "Player panel request timed out".into(),
            }),
        }
    }

    pub(crate) fn complete_ui_navigation_request(&self, id: &str, destination: &str, status: &str) {
        let pending = self
            .ui_navigation_pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(id));
        if let Some(pending) = pending {
            let _ = pending.tx.send(sanitize_ui_navigation_completion(
                &pending.target,
                destination,
                status,
            ));
        }
        if let Ok(mut active) = self.ui_navigation_active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
    }
    async fn navigate_ui(&self, target: UiNavigationTarget) -> Result<Value, ToolError> {
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut active = self
                .ui_navigation_active
                .lock()
                .map_err(|_| ToolError::failed("UI navigation is unavailable"))?;
            if active.is_some() {
                return Err(ToolError {
                    code: "BUSY",
                    message: "A UI navigation request is already running".into(),
                });
            }
            self.ui_navigation_pending
                .lock()
                .map_err(|_| ToolError::failed("UI navigation is unavailable"))?
                .insert(
                    id.clone(),
                    PendingUiNavigation {
                        tx,
                        target: target.clone(),
                    },
                );
            *active = Some(id.clone());
        }
        let mut guard = PendingUiNavigationGuard {
            app: &self.app,
            pending: &self.ui_navigation_pending,
            active: &self.ui_navigation_active,
            id: id.clone(),
            resolved: false,
        };
        self.app
            .emit(
                "automation-ui-navigation",
                json!({"requestId":id,"target":target.payload()}),
            )
            .map_err(|_| ToolError::failed("Could not deliver UI navigation request"))?;
        match tokio::time::timeout(Duration::from_secs(15), rx).await {
            Ok(Ok(Ok(result))) => {
                guard.disarm();
                Ok(result)
            }
            Ok(Ok(Err(_))) => {
                guard.disarm();
                Err(ToolError::failed("UI navigation response was invalid"))
            }
            Ok(Err(_)) => {
                guard.disarm();
                Err(ToolError::failed("UI navigation response channel closed"))
            }
            Err(_) => Err(ToolError {
                code: "TIMEOUT",
                message: "UI navigation timed out".into(),
            }),
        }
    }
    pub(crate) fn complete_search_ui_request(
        &self,
        id: &str,
        status: &str,
        source_id: &str,
        kind: &str,
        result_counts: &Value,
        source_coverage: Option<&Value>,
        error_code: Option<&str>,
    ) {
        let pending = self
            .search_ui_pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(id));
        if let Some(pending) = pending {
            let result = sanitize_search_ui_completion_with_coverage(
                &pending.source_id,
                &pending.kind,
                status,
                source_id,
                kind,
                result_counts,
                error_code,
                source_coverage,
            );
            let _ = pending.tx.send(result);
        }
    }
    pub(crate) fn complete_search_control_request(&self, id: &str, result: &Value) {
        let pending = self
            .search_control_pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(id));
        if let Some(pending) = pending {
            let response = if search_control_completion_expired(
                &pending.action,
                result,
                pending.expires_at_ms,
                search_control_now_ms(),
            ) {
                Ok(json!({"status":"expired"}))
            } else {
                sanitize_search_control_result(&pending.action, result)
            };
            let _ = pending.tx.send(response);
        }
        if let Ok(mut active) = self.search_control_active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
    }

    async fn search_control(&self, action: &str, arguments: &Value) -> Result<Value, ToolError> {
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let mut payload = parse_search_control_arguments(action, arguments)?;
        let id = Uuid::new_v4().to_string();
        let expires_at_ms = search_control_now_ms().saturating_add(4_500);
        let (tx, rx) = oneshot::channel();
        {
            let mut active = self
                .search_control_active
                .lock()
                .map_err(|_| ToolError::failed("Search UI controls unavailable"))?;
            if active.is_some() {
                return Err(ToolError {
                    code: "BUSY",
                    message: "A search UI control is already pending".into(),
                });
            }
            self.search_control_pending
                .lock()
                .map_err(|_| ToolError::failed("Search UI controls unavailable"))?
                .insert(
                    id.clone(),
                    PendingSearchControl {
                        tx,
                        action: action.into(),
                        expires_at_ms,
                    },
                );
            *active = Some(id.clone());
        }
        let mut guard = PendingSearchControlGuard {
            app: &self.app,
            pending: &self.search_control_pending,
            active: &self.search_control_active,
            id: id.clone(),
            resolved: false,
        };
        payload["requestId"] = json!(id);
        payload["expiresAtMs"] = json!(expires_at_ms);
        self.app
            .emit("automation-search-controls", payload)
            .map_err(|_| ToolError::failed("Could not deliver search UI control"))?;
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(result))) => {
                guard.resolved = true;
                Ok(result)
            }
            Ok(Ok(Err(_))) => Err(ToolError::failed("Search UI response was invalid")),
            Ok(Err(_)) => Err(ToolError::failed("Search UI response channel closed")),
            Err(_) => Err(ToolError {
                code: "TIMEOUT",
                message: "Search UI control timed out; inspect state before retry".into(),
            }),
        }
    }
    async fn search_ui(&self, request: SearchUiRequest) -> Result<Value, ToolError> {
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut active = self
                .search_ui_active
                .lock()
                .map_err(|_| ToolError::failed("Search UI control is unavailable"))?;
            if active.is_some() {
                return Err(ToolError {
                    code: "BUSY",
                    message: "A UI search request is already running".into(),
                });
            }
            self.search_ui_pending
                .lock()
                .map_err(|_| ToolError::failed("Search UI control is unavailable"))?
                .insert(
                    id.clone(),
                    PendingSearchUi {
                        tx,
                        source_id: request.source_id.clone(),
                        kind: request.kind.clone(),
                    },
                );
            *active = Some(id.clone());
        }
        let mut guard = PendingSearchUiGuard {
            app: Some(&self.app),
            pending: &self.search_ui_pending,
            active: &self.search_ui_active,
            id: id.clone(),
            resolved: false,
        };
        if self
            .app
            .emit(
            "automation-search-ui",
            json!({"requestId":id,"query":request.query,"sourceId":request.source_id,"kind":request.kind}),
            )
            .is_err()
        {
            return Err(ToolError::failed("Could not deliver the search request to the UI"));
        }
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(mut result))) => {
                result["requestId"] = json!(id);
                guard.disarm();
                Ok(result)
            }
            Ok(Ok(Err(_))) => {
                guard.disarm();
                Err(ToolError {
                    code: "INVALID_RESPONSE",
                    message: "Search UI returned an invalid response".into(),
                })
            }
            Ok(Err(_)) => Err(ToolError::failed("Search UI response channel closed")),
            Err(_) => Err(ToolError {
                code: "TIMEOUT",
                message: "Search UI request timed out".into(),
            }),
        }
    }
    pub(crate) fn complete_identify_request(&self, id: &str, result: Value, error: Option<String>) {
        if id.is_empty() || id.len() > 64 {
            return;
        }
        complete_identify_pending(&self.identify_pending, id, result, error);
        if let Ok(mut active) = self.identify_active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
    }

    fn cancel_identify_request(&self, id: &str) -> bool {
        let active = self
            .identify_active
            .lock()
            .ok()
            .is_some_and(|active| active.as_deref() == Some(id));
        if !active {
            return false;
        }
        let _ = self
            .app
            .emit("automation-identify-cancelled", json!({"captureId":id}));
        let completed = complete_identify_pending(
            &self.identify_pending,
            id,
            json!({}),
            Some("Audio identification was cancelled".into()),
        );
        if let Ok(mut active) = self.identify_active.lock() {
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
        completed
    }

    async fn identify_playing_track_via_ui(
        &self,
        capture_id: Option<String>,
    ) -> Result<Value, ToolError> {
        if !self.frontend_ready.load(Ordering::Acquire) {
            return Err(ToolError {
                code: "FRONTEND_NOT_READY",
                message: "The player UI is not ready".into(),
            });
        }
        let id = capture_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let (tx, rx) = oneshot::channel();
        {
            let mut active = self
                .identify_active
                .lock()
                .map_err(|_| ToolError::failed("Audio identification is unavailable"))?;
            if active.is_some() {
                return Err(ToolError {
                    code: "BUSY",
                    message: "An audio identification request is already running".into(),
                });
            }
            self.identify_pending
                .lock()
                .map_err(|_| ToolError::failed("Audio identification is unavailable"))?
                .insert(id.clone(), tx);
            *active = Some(id.clone());
        }
        let mut guard = PendingIdentifyGuard {
            app: &self.app,
            pending: &self.identify_pending,
            active: &self.identify_active,
            id: id.clone(),
            resolved: false,
        };
        let snapshot = self.playback_tracking_snapshot();
        validate_playing_identification_snapshot(
            &snapshot,
            self.playback
                .get()
                .is_some_and(|playback| playback.native_enabled()),
        )
        .map_err(ToolError::failed)?;
        let track_id = snapshot
            .get("trackId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or_else(|| ToolError::failed("There is no current playback track"))?;
        let session_id = snapshot
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or_else(|| ToolError::failed("Current playback session is unavailable"))?;
        let generation = snapshot
            .get("sourceGeneration")
            .and_then(Value::as_u64)
            .ok_or_else(|| ToolError::failed("Current WebView playback source is unavailable"))?;
        let seek_serial = snapshot
            .get("seekSerial")
            .and_then(Value::as_u64)
            .ok_or_else(|| ToolError::failed("Current playback seek state is unavailable"))?;
        let chapter_id = snapshot.get("chapterId").and_then(Value::as_str);
        self.app.emit("automation-identify-playing-audio", json!({"captureId":id,"trackId":track_id,"sessionId":session_id,"generation":generation,"seekSerial":seek_serial,"chapterId":chapter_id})).map_err(|_| ToolError::failed("Could not deliver audio identification request to the player UI"))?;
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(Ok(result))) => {
                let current = self.playback_tracking_snapshot();
                let checked = validate_playing_identification_result(
                    &snapshot,
                    &current,
                    self.playback
                        .get()
                        .is_some_and(|playback| playback.native_enabled()),
                    result,
                )
                .map_err(ToolError::failed)?;
                guard.disarm();
                Ok(checked)
            }
            Ok(Ok(Err(error))) => {
                guard.disarm();
                Err(ToolError::failed(error))
            }
            Ok(Err(_)) => Err(ToolError::failed(
                "Audio identification response channel closed",
            )),
            Err(_) => Err(ToolError {
                code: "TIMEOUT",
                message: "Audio identification timed out".into(),
            }),
        }
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
    fn queued_operation_id(a: &Value) -> Result<String, ToolError> {
        let raw = a
            .get("operationId")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::bad("operationId UUID is required"))?;
        if raw.len() != 36 {
            return Err(ToolError::bad("operationId must be a UUID"));
        }
        Uuid::parse_str(raw)
            .map(|id| id.to_string())
            .map_err(|_| ToolError::bad("operationId must be a UUID"))
    }
    pub fn queued_playback_status(&self, id: &str) -> Value {
        self.queued_playback.lock().ok().and_then(|r|r.get(id)).map(|r|json!({"operationId":r.id,"status":r.status.as_str(),"receipt":r.receipt,"detail":r.detail})).unwrap_or_else(||json!({"operationId":id,"status":"unknown_or_expired"}))
    }
    pub fn accept_queued_playback(
        &self,
        id: &str,
        transport: &str,
        receipt: Value,
        playback: Value,
    ) -> Value {
        let result = self
            .queued_playback
            .lock()
            .map(|mut registry| {
                // Operation ownership and publication acceptance share this lock.
                // The publisher itself only acquires its protocol/state locks.
                accept_queued_projection(
                    &mut registry,
                    id,
                    transport,
                    receipt,
                    playback,
                    |packet| self.update_playback_state(packet),
                )
            })
            .unwrap_or_else(|_| json!({"accepted":false}));
        self.queued_notify.notify_waiters();
        result
    }
    pub fn update_queued_playback(
        &self,
        id: &str,
        transport: &str,
        status: &str,
        detail: Value,
    ) -> Value {
        use crate::queued_playback::Status;
        let status = match status {
            "playing" => Status::Playing,
            "paused" => Status::Paused,
            "failed" => Status::Failed,
            "exhausted" => Status::Exhausted,
            "cancelled" => Status::Cancelled,
            "superseded" => Status::Superseded,
            _ => return json!({"accepted":false}),
        };
        let safe = json!({"currentGeneration":detail["currentGeneration"].as_u64(),"entryId":detail["entryId"].as_str().and_then(crate::playback::safe_failure_identity),"reason":detail["reason"].as_str().map(|s|crate::playback::safe_failure_text(s,250))});
        let accepted = self
            .queued_playback
            .lock()
            .map(|mut r| {
                let Some(row) = r.get(id) else { return false };
                if !crate::queued_playback::generation_is_current(
                    safe["currentGeneration"].as_u64(),
                    row.receipt
                        .as_ref()
                        .and_then(|v| v["acceptedGeneration"].as_u64()),
                    row.detail
                        .as_ref()
                        .and_then(|v| v["currentGeneration"].as_u64()),
                ) {
                    return false;
                }
                r.update(id, transport, status, safe)
            })
            .unwrap_or(false);
        self.queued_notify.notify_waiters();
        json!({"accepted":accepted})
    }
    async fn cancel_queued_playback(&self, id: &str) -> Result<Value, ToolError> {
        let changed = self
            .queued_playback
            .lock()
            .map_err(|_| ToolError::failed("Playback operations unavailable"))?
            .cancel(id)
            .map_err(ToolError::failed)?;
        if changed {
            let _ = self.app.emit(
                "automation-queued-playback-cancelled",
                json!({"operationId":id}),
            );
            if let Some(p) = self.playback.get().filter(|p| p.native_enabled()).cloned() {
                let operation = id.to_string();
                // Cancellation delivery belongs to the operation, not this RPC.
                tokio::spawn(async move {
                    let _ = p
                        .execute(json!({"action":"stop","expectedQueuedOperationId":operation}))
                        .await;
                });
            }
        }
        self.queued_notify.notify_waiters();
        Ok(self.queued_playback_status(id))
    }
    async fn queued_play_tracks(&self, id: String, ids: Vec<String>) -> Result<Value, ToolError> {
        if ids.is_empty() {
            return Err(ToolError::bad("trackIds must not be empty"));
        }
        let native_epoch = self
            .playback
            .get()
            .filter(|p| p.native_enabled())
            .map(|p| p.queued_playback_epoch());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let transport = Uuid::new_v4().to_string();
        let (fresh, superseded) = {
            let mut registry = self
                .queued_playback
                .lock()
                .map_err(|_| ToolError::failed("Playback operations unavailable"))?;
            let superseded = registry.preparing();
            let fresh = registry
                .begin(id.clone(), transport.clone(), ids.clone())
                .map_err(ToolError::bad)?;
            (fresh, superseded)
        };
        if !fresh {
            return Ok(self.queued_playback_status(&id));
        }
        // A new preparation retires older preparing rows; wake their bounded waiters.
        self.queued_notify.notify_waiters();
        for (operation, request) in superseded {
            let _ = self.app.emit(
                "automation-queued-playback-cancelled",
                json!({"operationId":operation,"requestId":request}),
            );
        }
        let _guard = QueuedPlaybackPrepareGuard {
            service: self,
            operation_id: id.clone(),
            transport: transport.clone(),
        };
        if let Some(playback) = self.playback.get().filter(|p| p.native_enabled()).cloned() {
            use crate::queued_playback::Status;
            let tracks = ids
                .iter()
                .map(|id| {
                    self.store
                        .get_track(id)?
                        .ok_or_else(|| "Unknown track ID".to_string())
                })
                .collect::<Result<Vec<Track>, String>>();
            let tracks = match tracks {
                Ok(tracks) => tracks,
                Err(e) => {
                    self.update_queued_playback(&id, &transport, "failed", json!({"reason":e}));
                    return Err(ToolError::failed(e));
                }
            };
            let gate = self.queued_playback.clone();
            let operation = id.clone();
            let request = transport.clone();
            // Check preparation under the native command lock, after any wait for
            // older commands. Cancelling an operation cannot leave a queued start
            // that later replaces a newer playback command.
            let commit_registry = self.queued_playback.clone();
            let commit_operation = id.clone();
            let commit_transport = transport.clone();
            let expected_tracks = ids.clone();
            let result = tokio::time::timeout_at(
                deadline,
                playback.execute_guarded_committed(
                    json!({"action":"play","tracks":tracks,"index":0,"operationId":id,"queuedOperationId":id}),
                    native_epoch.unwrap_or(0),
                    move || {
                        gate.lock().ok().and_then(|r| r.get(&operation)).is_some_and(|r| {
                            r.transport == request && r.status == Status::Preparing
                        })
                    },
                    move |full| {
                        let owned = full["queuedOperationId"].as_str() == Some(commit_operation.as_str())
                            && full["trackIds"] == json!(expected_tracks);
                        let receipt = json!({
                            "operationId":commit_operation,"status":"accepted","engine":"native",
                            "trackIds":expected_tracks,"queueRevision":full["queueRevision"],
                            "queueEntryIds":full["queueEntryIds"],
                            "acceptedGeneration":full["generation"],"requestedCount":expected_tracks.len()
                        });
                        if owned && commit_registry.lock().map(|mut registry| {
                            registry.accept(&commit_operation, &commit_transport, receipt)
                        }).unwrap_or(false) {
                            Ok(())
                        } else {
                            Err("Queued playback operation was cancelled or superseded".into())
                        }
                    },
                ),
            ).await.map_err(|_| ToolError {
                code: "TIMEOUT", message: "Player did not accept the queue in time".into()
            })?;
            if let Err(error) = result {
                self.update_queued_playback(&id, &transport, "failed", json!({"reason":error}));
                // A failed commit may follow dispatch. Stop only this root; a
                // competing replacement remains protected by the command lock.
                let cleanup = playback.clone();
                let operation = id.clone();
                tokio::spawn(async move {
                    let _ = cleanup
                        .execute(json!({
                            "action":"stop","expectedQueuedOperationId":operation
                        }))
                        .await;
                });
                return Err(ToolError::failed(error));
            }
            // The only detached native task observes an already accepted owner.
            // Command dispatch itself is cancellable while awaiting its lock.
            let registry = self.queued_playback.clone();
            let notify = self.queued_notify.clone();
            let operation = id.clone();
            let request = transport.clone();
            tokio::spawn(async move {
                let mut updates = playback.subscribe();
                loop {
                    let notified = notify.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    let snapshot = updates.borrow().clone();
                    let status =
                        if snapshot["queuedOperationId"].as_str() != Some(operation.as_str()) {
                            Some(
                                if matches!(snapshot["status"].as_str(), Some("idle" | "paused")) {
                                    Status::Cancelled
                                } else {
                                    Status::Superseded
                                },
                            )
                        } else {
                            match snapshot["status"].as_str() {
                                Some("playing") => Some(Status::Playing),
                                Some("failed") => {
                                    let full = playback.snapshot_page(0, 1);
                                    let exhausted = full["diagnostics"]["recentTrackFailures"]
                                        .as_array()
                                        .and_then(|a| a.last())
                                        .is_some_and(|f| {
                                            f["operationId"].as_str() == Some(operation.as_str())
                                                && f["outcome"] == "queue_exhausted"
                                        });
                                    Some(crate::queued_playback::native_failure_status(exhausted))
                                }
                                Some("paused") => Some(Status::Paused),
                                Some("idle") => Some(Status::Cancelled),
                                Some("ended") => Some(Status::Exhausted),
                                _ => None,
                            }
                        };
                    if let Some(status) = status {
                        let terminal = status.terminal();
                        if let Ok(mut r) = registry.lock() {
                            r.update(&operation,&request,status,json!({"currentGeneration":snapshot["generation"],"entryId":snapshot["entryId"],"reason":snapshot["playerMessage"].as_str().map(|s|crate::playback::safe_failure_text(s,250))}));
                        }
                        if terminal {
                            return;
                        }
                    }
                    if registry
                        .lock()
                        .ok()
                        .and_then(|r| r.get(&operation))
                        .is_none_or(|r| r.status.terminal())
                    {
                        return;
                    }
                    tokio::select! { result=updates.changed()=>{if result.is_err(){return}}, _=notified=>{} }
                }
            });
        } else if let Err(e) = self.app.emit(
            "automation-queued-playback",
            json!({"operationId":id,"requestId":transport,"trackIds":ids}),
        ) {
            return Err(ToolError::failed(e.to_string()));
        }
        loop {
            let notified = self.queued_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let row = self
                .queued_playback
                .lock()
                .map_err(|_| ToolError::failed("Playback operations unavailable"))?
                .get(&id)
                .ok_or_else(|| ToolError::failed("Playback operation expired"))?;
            if row.receipt.is_some() || row.status.terminal() {
                return Ok(self.queued_playback_status(&id));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                // Guard and accept share registry lock: never cancel an already accepted owner.
                let expired = self
                    .queued_playback
                    .lock()
                    .map(|mut r| r.expire(&id, &transport))
                    .unwrap_or(false);
                if expired {
                    let _ = self.app.emit(
                        "automation-queued-playback-cancelled",
                        json!({"operationId":id,"requestId":transport}),
                    );
                }
                return Ok(self.queued_playback_status(&id));
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
        if let Some(playback) = self.playback.get().filter(|p| p.native_enabled()) {
            let mut result = playback
                .execute_ids(action, track_ids, mode, position, volume)
                .await
                .map_err(ToolError::failed);
            if let Ok(value) = &mut result {
                strip_native_playback_fields(value);
            }
            return result;
        }
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| ToolError::failed("Playback queue unavailable"))?
            .insert(id.clone(), tx);
        let _pending_guard = PendingPlaybackGuard::new(self, id.clone());
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
            "preview_verification_playlist_intent" => {
                let request = parse_verification_intent_approval(&a, false)?;
                self.app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Provider session unavailable"))?
                    .managed_provider_session
                    .preview_verification_playlist_intent(&request.provider)
                    .map_err(ToolError::failed)?
            }
            "apply_verification_playlist_intent" => {
                let request = parse_verification_intent_approval(&a, true)?;
                self.app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Provider session unavailable"))?
                    .managed_provider_session
                    .apply_verification_playlist_intent(
                        &request.provider,
                        request.intent_id.as_deref().unwrap(),
                        request.review_token.as_deref().unwrap(),
                        request.contract_hash.as_deref().unwrap(),
                    )
                    .await
                    .map_err(ToolError::failed)?
            }
            "create_verification_playlist" => {
                let provider = a
                    .get("provider")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::failed("Choose Spotify or YouTube"))?;
                let app_state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Provider session is unavailable"))?;
                app_state
                    .managed_provider_session
                    .create_verification_playlist(provider)
                    .await
                    .map_err(ToolError::failed)?
            }

            "get_managed_provider_session_status"
            | "start_managed_provider_login"
            | "confirm_managed_provider_account"
            | "return_youtube_login_to_music"
            | "start_managed_provider_background"
            | "stop_managed_provider_session" => {
                let app_state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Provider session is unavailable"))?;
                let session = &app_state.managed_provider_session;
                let status = match name {
                    "get_managed_provider_session_status" => session.status(),
                    "start_managed_provider_background" => {
                        session.start_background().map_err(ToolError::failed)?
                    }
                    "stop_managed_provider_session" => session.stop().map_err(ToolError::failed)?,
                    "return_youtube_login_to_music" => session
                        .return_youtube_login_to_music()
                        .await
                        .map_err(ToolError::failed)?,
                    _ => {
                        let provider = str_arg(&a, "provider", 32)?;
                        if !matches!(provider.as_str(), "spotify" | "youtube") {
                            return Err(ToolError::bad("provider must be spotify or youtube"));
                        }
                        if name == "start_managed_provider_login" {
                            session.start_login(&provider).map_err(ToolError::failed)?
                        } else {
                            session
                                .confirm_account(&provider)
                                .await
                                .map_err(ToolError::failed)?
                        }
                    }
                };
                managed_provider_qualification_status(&status)
            }
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
                validate_discovery_sources(&source_ids)?;
                if kinds.iter().any(|kind| {
                    ![
                        "artist", "album", "track", "video", "set", "file", "playlist", "profile",
                        "channel",
                    ]
                    .contains(&kind.as_str())
                }) {
                    return Err(ToolError::bad("kinds contains an unsupported result kind"));
                }
                let request = DiscoveryRequest {
                    query,
                    source_ids,
                    kinds,
                    cursor: discovery_cursor_arg(&a)?,
                    limit: usize_arg(&a, "limit", 100)?.or(Some(25)),
                    isrc: a.get("isrc").and_then(Value::as_str).map(str::to_owned),
                };
                let discovery = DiscoveryService::new(
                    self.artists.clone(),
                    self.acquisition.clone(),
                    self.store.clone(),
                    self.connections.clone(),
                );
                if let Ok(remote) = self.remote_playlists.read() {
                    if let Some(remote) = remote.clone() {
                        discovery.set_remote_service(remote);
                    }
                }
                let pages = discovery.async_shared_search(request).await;
                for page in &pages {
                    for result in &page.results {
                        if let Some(track) = &result.track {
                            self.store.cache_track(track).map_err(ToolError::failed)?;
                        }
                    }
                }
                json!({"pages":pages,"partial":pages.iter().any(|page|!page.complete),"hasWarnings":pages.iter().any(|page|!page.warnings.is_empty())})
            }
            "list_remote_playlists" => {
                let service = self.remote_service()?;
                let provider = str_arg(&a, "provider", 32)?;
                let owner = optional_str_arg(&a, "ownerId", 256)?;
                let cursor = optional_str_arg(&a, "cursor", 512)?;
                serde_json::to_value(
                    service
                        .list_playlists(&provider, owner.as_deref(), cursor.as_deref())
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "repair_spotify_durations_public" => {
                require_no_arguments(&a, name)?;
                let result = self
                    .remote_service()?
                    .repair_spotify_durations_public()
                    .await
                    .map_err(|error| public_duration_repair_error(&error))?;
                json!({"changed":result.changed,"processed":result.processed,"complete":result.complete})
            }
            "cancel_public_spotify_duration_repair" => {
                require_no_arguments(&a, name)?;
                json!({"cancelled":self.remote_service()?.cancel_public_spotify_duration_repair().map_err(ToolError::failed)?})
            }
            "public_spotify_duration_repair_running" => {
                require_no_arguments(&a, name)?;
                json!({"running":self.remote_service()?.public_spotify_duration_repair_running()})
            }
            "get_remote_track" => {
                let service = self.remote_service()?;
                let provider = str_arg(&a, "provider", 16)?;
                let track_id = str_arg(&a, "trackId", 22)?;
                let track = service
                    .get_remote_track(&provider, &track_id)
                    .await
                    .map_err(ToolError::failed)?;
                json!({"track":safe_track(track)})
            }
            "prepare_playback_catalog_identity" => {
                let track_id = parse_catalog_identity_request(&a)?;
                // Read an existing saved track only. The acquisition method may
                // cache proof, but this diagnostic never writes tags or playback state.
                let track = required_track(&self.store, &track_id)?;
                let app_state = self.app.try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                let guard = app_state.provider_reads.begin(Uuid::new_v4().to_string())
                    .map_err(|_| ToolError::failed("Catalog identity read is unavailable"))?;
                let started = std::time::Instant::now();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
                let result = tokio::time::timeout_at(deadline, self.acquisition.prepare_catalog_duration_track(
                    &track, guard.cancellation(), deadline,
                )).await;
                // Dropping the bounded registry guard closes its cancellation
                // sender on every return/drop path; no detached diagnostic job.
                drop(guard);
                match result {
                    Ok(Ok(prepared)) => catalog_identity_summary(
                        prepared.as_ref().map(|value| &value.track), started.elapsed().as_millis(),
                    ),
                    _ => return Err(ToolError {
                        code: if tokio::time::Instant::now() >= deadline { "TIMEOUT" } else { "OPERATION_FAILED" },
                        message: "Catalog identity preparation did not complete".into(),
                    }),
                }
            }
            "prepare_spotify_recording_crosswalk" => {
                let track_id = spotify_id_arg(&a, "trackId")?;
                let album_id_hint = optional_str_arg(&a, "albumIdHint", 22)?;
                let musicbrainz_recording_id_hint =
                    optional_str_arg(&a, "musicBrainzRecordingIdHint", 36)?;
                if album_id_hint.as_deref().is_some_and(|id| {
                    !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) || id.len() != 22
                }) {
                    return Err(ToolError::bad("albumIdHint must be a Spotify album ID"));
                }
                if album_id_hint.is_some() && musicbrainz_recording_id_hint.is_some() {
                    return Err(ToolError::bad(
                        "Choose either albumIdHint or musicBrainzRecordingIdHint",
                    ));
                }
                if musicbrainz_recording_id_hint.as_deref().is_some_and(|id| {
                    !crate::artist_credit_aliases::valid_musicbrainz_recording_id(id)
                }) {
                    return Err(ToolError::bad(
                        "musicBrainzRecordingIdHint must be a MusicBrainz recording ID",
                    ));
                }
                let request_id = str_arg(&a, "requestId", 36)?;
                if Uuid::parse_str(&request_id).is_err() {
                    return Err(ToolError::bad("requestId must be a UUID"));
                }
                let app_state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                let remote = self.remote_service()?;
                let artists = self.artists.clone();
                let guard = app_state
                    .provider_reads
                    .begin(request_id)
                    .map_err(ToolError::failed)?;
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                if let Some(recording_id) = musicbrainz_recording_id_hint.as_deref() {
                    let status = remote
                        .prepare_spotify_artist_credit_crosswalk(
                            &artists,
                            &track_id,
                            recording_id,
                            deadline,
                            guard.cancellation(),
                        )
                        .await
                        .map_err(ToolError::failed)?;
                    drop(guard);
                    return serde_json::to_value(status).map_err(|_| {
                        ToolError::failed("Recording credit crosswalk serialization failed")
                    });
                }
                let result = remote
                    .prepare_spotify_recording_crosswalk(
                        &artists,
                        &track_id,
                        album_id_hint.as_deref(),
                        deadline,
                        guard.cancellation(),
                    )
                    .await
                    .map_err(ToolError::failed)?;
                drop(guard);
                serde_json::to_value(result)
                    .map_err(|_| ToolError::failed("Recording crosswalk serialization failed"))?
            }
            "get_remote_artist" => {
                let service = self.remote_service()?;
                let artist_id = spotify_id_arg(&a, "artistId")?;
                let cursor = numeric_spotify_cursor_arg(&a)?;
                serde_json::to_value(
                    service
                        .browse_artist(&artist_id, cursor.as_deref())
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "browse_remote_artist_tracks" => {
                let service = self.remote_service()?;
                let artist_id = spotify_id_arg(&a, "artistId")?;
                let cursor = numeric_spotify_cursor_arg(&a)?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                let page = state
                    .provider_reads
                    .run(Some(Uuid::new_v4().to_string()), || async {
                        service
                            .browse_artist_tracks(&artist_id, cursor.as_deref())
                            .await
                    })
                    .await
                    .map_err(ToolError::failed)?;
                serde_json::to_value(page)
                    .map_err(|_| ToolError::failed("Artist tracks serialization failed"))?
            }
            "get_remote_album_tracks" => {
                let service = self.remote_service()?;
                let album_id = spotify_id_arg(&a, "albumId")?;
                let cursor = numeric_spotify_cursor_arg(&a)?;
                serde_json::to_value(
                    service
                        .browse_album_tracks(&album_id, cursor.as_deref())
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "inspect_spotify_profile_page" => {
                let request: crate::remote_playlists::SpotifyProfileInspectionRequest =
                    serde_json::from_value(a.clone()).map_err(|_| {
                        ToolError::bad("Spotify profile inspection request is invalid")
                    })?;
                request.normalized_owner_id().map_err(ToolError::bad)?;
                if request.summary_mode() {
                    request.summary_limit().map_err(ToolError::bad)?;
                } else {
                    request.page_bounds().map_err(ToolError::bad)?;
                }
                let service = self.remote_service()?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                let result = state
                    .provider_reads
                    .run(Some(Uuid::new_v4().to_string()), || async {
                        service.inspect_spotify_profile_page(&request).await
                    })
                    .await
                    .map_err(ToolError::failed)?;
                serde_json::to_value(result).map_err(|_| {
                    ToolError::failed("Spotify profile inspection serialization failed")
                })?
            }
            "inspect_spotify_artist_page" => {
                let request: crate::remote_playlists::SpotifyArtistInspectionRequest =
                    serde_json::from_value(a.clone()).map_err(|_| {
                        ToolError::bad("Spotify artist inspection request is invalid")
                    })?;
                request.validate().map_err(ToolError::bad)?;
                let service = self.remote_service()?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                let result = state
                    .provider_reads
                    .run(Some(Uuid::new_v4().to_string()), || async {
                        service.inspect_spotify_artist_page(&request).await
                    })
                    .await
                    .map_err(ToolError::failed)?;
                serde_json::to_value(result).map_err(|_| {
                    ToolError::failed("Spotify artist inspection serialization failed")
                })?
            }
            "inspect_youtube_library_page" => {
                if !a.as_object().is_some_and(|object| object.is_empty()) {
                    return Err(ToolError::bad(
                        "YouTube library inspection accepts no arguments",
                    ));
                }
                let service = self.remote_service()?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                let result = state
                    .provider_reads
                    .run(Some(Uuid::new_v4().to_string()), || async {
                        service.inspect_youtube_library_page().await
                    })
                    .await
                    .map_err(ToolError::failed)?;
                serde_json::to_value(result).map_err(|_| {
                    ToolError::failed("YouTube library inspection serialization failed")
                })?
            }
            "inspect_youtube_playlist_page" => {
                let request: crate::remote_playlists::YouTubePlaylistInspectionRequest =
                    serde_json::from_value(a.clone())
                        .map_err(|_| ToolError::bad("YouTube inspection request is invalid"))?;
                request.validate().map_err(ToolError::bad)?;
                let service = self.remote_service()?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Remote read registry is unavailable"))?;
                // A distinct guard prevents this diagnostic read from sharing cancellation
                // state with UI navigation or any other provider read.
                let result = state
                    .provider_reads
                    .run(Some(Uuid::new_v4().to_string()), || async {
                        service
                            .inspect_youtube_playlist_page(
                                &request.playlist_id,
                                request.cursor.as_deref(),
                            )
                            .await
                    })
                    .await
                    .map_err(ToolError::failed)?;
                serde_json::to_value(result)
                    .map_err(|_| ToolError::failed("YouTube inspection serialization failed"))?
            }
            "get_remote_playlist" => {
                let service = self.remote_service()?;
                let provider = str_arg(&a, "provider", 32)?;
                let playlist_id = str_arg(&a, "playlistId", 2048)?;
                let cursor = optional_str_arg(&a, "cursor", 2048)?;
                serde_json::to_value(
                    service
                        .browse_playlist(&provider, &playlist_id, cursor.as_deref())
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "list_track_files" => {
                let track_id = str_arg(&a, "trackId", 300)?;
                let files = self
                    .store
                    .list_track_files(&track_id)
                    .map_err(ToolError::failed)?;
                serde_json::to_value(files).map_err(|e| ToolError::failed(e.to_string()))?
            }
            "set_preferred_track_file" => {
                let track_id = str_arg(&a, "trackId", 300)?;
                let asset_id = str_arg(&a, "assetId", 300)?;
                let track = self
                    .store
                    .set_preferred_track_file(&track_id, &asset_id)
                    .map_err(ToolError::failed)?;
                self.notify_library();
                serde_json::to_value(track).map_err(|e| ToolError::failed(e.to_string()))?
            }
            "import_remote_playlists" => {
                let service = self.remote_service()?;
                let provider = str_arg(&a, "provider", 32)?;
                let playlist_ids = string_array(&a, "playlistIds", 500)?;
                if playlist_ids.is_empty() {
                    return Err(ToolError::bad("playlistIds must contain at least one ID"));
                }
                serde_json::to_value(
                    service
                        .import_playlists(&provider, playlist_ids)
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "refresh_remote_playlist" => {
                let service = self.remote_service()?;
                let binding_id = str_arg(&a, "bindingId", 256)?;
                serde_json::to_value(
                    service
                        .refresh(&binding_id)
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "search_remote_playlist_writeback_candidates" => {
                let request = parse_writeback_candidates(&a)?;
                let service = self.remote_service()?;
                serde_json::to_value(
                    service
                        .search_remote_playlist_writeback_candidates(
                            &request.binding_id,
                            &request.local_track_id,
                        )
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|_| ToolError::failed("Playlist candidates serialization failed"))?
            }
            "get_playlist_sync_preview" => {
                let request = parse_writeback_preview(&a)?;
                let service = self.remote_service()?;
                let reviewed = request.reviewed_matches.map(|matches| {
                    matches
                        .into_iter()
                        .map(|entry| crate::remote_playlists::ReviewedRemoteTrackMatch {
                            local_track_id: entry.local_track_id,
                            candidate_token: entry.candidate_token,
                        })
                        .collect()
                });
                serde_json::to_value(
                    service
                        .get_sync_preview(&request.binding_id, reviewed)
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "apply_playlist_sync_preview" => {
                let service = self.remote_service()?;
                let preview_id = str_arg(&a, "previewId", 256)?;
                let changes =
                    a.get("changes")
                        .cloned()
                        .map(
                            serde_json::from_value::<
                                crate::remote_playlists::RemotePlaylistSyncChanges,
                            >,
                        )
                        .transpose()
                        .map_err(|e| ToolError::bad(e.to_string()))?;
                serde_json::to_value(
                    service
                        .apply_sync_preview(&preview_id, changes)
                        .await
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "get_remote_playlist_import_job" => {
                let service = self.remote_service()?;
                let job_id = str_arg(&a, "jobId", 256)?;
                serde_json::to_value(
                    service
                        .import_job_status(&job_id)
                        .map_err(ToolError::failed)?,
                )
                .map_err(|e| ToolError::failed(e.to_string()))?
            }
            "cancel_remote_playlist_import" => {
                let service = self.remote_service()?;
                let job_id = str_arg(&a, "jobId", 256)?;
                serde_json::to_value(service.cancel_import(&job_id).map_err(ToolError::failed)?)
                    .map_err(|e| ToolError::failed(e.to_string()))?
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
                    let saved = self.store.cache_track(&track).map_err(ToolError::failed)?;
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
                    .cache_track(&radio_station_track(&station))
                    .map_err(ToolError::failed)?;
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
            "list_tracks_page" => {
                let view = str_arg(&a, "view", 20)?;
                let store_view = match view.as_str() {
                    "library" | "favorites" | "shortlist" => view.as_str(),
                    "collection" | "collections" => "collection",
                    _ => {
                        return Err(ToolError::bad(
                            "view must be library, favorites, shortlist, or collections",
                        ))
                    }
                };
                let collection_id = optional_str_arg(&a, "collectionId", 256)?;
                if store_view == "collection" && collection_id.is_none() {
                    return Err(ToolError::bad(
                        "collectionId is required for the collections view",
                    ));
                }
                if store_view != "collection" && collection_id.is_some() {
                    return Err(ToolError::bad(
                        "collectionId is only valid for the collections view",
                    ));
                }
                let query = optional_str_arg(&a, "query", 500)?;
                let sort_field = optional_str_arg(&a, "sortField", 32)?;
                let sort_field = sort_field
                    .map(|field| match field.as_str() {
                        "rank" => Ok("sourceOrder".to_owned()),
                        "durationSeconds" => Ok("duration".to_owned()),
                        _ if matches!(
                            field.as_str(),
                            "sourceOrder"
                                | "title"
                                | "artist"
                                | "album"
                                | "duration"
                                | "releaseDate"
                                | "dateAdded"
                                | "bpm"
                                | "key"
                                | "plays"
                                | "views"
                        ) =>
                        {
                            Ok(field)
                        }
                        _ => Err(ToolError::bad("Unsupported sortField")),
                    })
                    .transpose()?;
                let direction = optional_str_arg(&a, "direction", 4)?;
                if direction
                    .as_deref()
                    .is_some_and(|value| !matches!(value, "asc" | "desc"))
                {
                    return Err(ToolError::bad("direction must be asc or desc"));
                }
                let min_duration = optional_u32_arg(&a, "minDurationSeconds")?;
                let max_duration = optional_u32_arg(&a, "maxDurationSeconds")?;
                if min_duration
                    .zip(max_duration)
                    .is_some_and(|(min, max)| min > max)
                {
                    return Err(ToolError::bad(
                        "minDurationSeconds cannot exceed maxDurationSeconds",
                    ));
                }
                let availability = optional_str_arg(&a, "availability", 16)?;
                if availability
                    .as_deref()
                    .is_some_and(|value| !matches!(value, "all" | "local" | "not-local"))
                {
                    return Err(ToolError::bad(
                        "availability must be all, local, or not-local",
                    ));
                }
                let provider = optional_str_arg(&a, "provider", 80)?;
                let offset = usize_arg(&a, "offset", 1_000_000)?.unwrap_or(0);
                let limit = usize_arg(&a, "limit", 200)?.unwrap_or(50).max(1);
                let (rows, total) = self
                    .store
                    .list_tracks_page_exact_cached(
                        store_view,
                        collection_id.as_deref(),
                        limit,
                        offset,
                        query.as_deref(),
                        sort_field.as_deref(),
                        direction.as_deref(),
                        min_duration,
                        max_duration,
                        availability.as_deref(),
                        provider.as_deref(),
                    )
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
                    .map(|item| self.store.cache_track(item).map_err(ToolError::failed))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                json!(result)
            }
            "get_app_status" => self.status(),
            "resize_app_window" => self.resize_app_window(&a).await?,
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
                                found.push(
                                    self.store.cache_track(track).map_err(ToolError::failed)?,
                                );
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
                    let track = self.store.cache_track(&track).map_err(ToolError::failed)?;
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
                    match self.store.cache_track(&track) {
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
                    match self.store.cache_track(&track) {
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
            "identify_playing_track" => {
                let capture_id = parse_identification_capture_id(&a, false)?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Playback identification is unavailable"))?;
                if state.playback.native_enabled() {
                    serde_json::to_value(
                        crate::identify_playing_track(state, capture_id)
                            .await
                            .map_err(ToolError::failed)?,
                    )
                    .map_err(|error| ToolError::failed(error.to_string()))?
                } else {
                    self.identify_playing_track_via_ui(capture_id).await?
                }
            }
            "cancel_playing_track_identification" => {
                let capture_id = parse_identification_capture_id(&a, true)?
                    .ok_or_else(|| ToolError::bad("captureId is required"))?;
                let state = self
                    .app
                    .try_state::<crate::AppState>()
                    .ok_or_else(|| ToolError::failed("Playback identification is unavailable"))?;
                // These are explicit ID-scoped calls to the native and
                // WebView registries plus the owned WebView UI request. Calling
                // both registries records a bounded pre-cancel tombstone if
                // identify has not reached its backend yet. No arbitrary
                // command name or invoke payload is accepted.
                let backend_cancelled = cancel_identification_registries(
                    &capture_id,
                    |id| state.playback.cancel_playing_track_identification(id),
                    |id| state.webview_identification.cancel_pcm_capture(id),
                );
                let ui_cancelled = self.cancel_identify_request(&capture_id);
                json!({"cancelled":backend_cancelled || ui_cancelled})
            }
            "capture_main_ui" => {
                require_no_arguments(&a, name)?;
                crate::main_ui_capture::capture(&self.app)
                    .await
                    .map_err(ToolError::failed)?
            }
            "inspect_playback_layout" => self.inspect_playback_layout().await?,
            "inspect_webview_media" => {
                require_no_arguments(&a, name)?;
                self.inspect_webview_media().await?
            }
            "interact_queue_ui" => self.interact_queue_ui(&a).await?,
            "set_player_panel" => {
                self.set_player_panel(parse_player_panel_request(&a)?)
                    .await?
            }
            "search_ui" => self.search_ui(parse_search_ui_request(&a)?).await?,
            "inspect_search_ui" => self.search_control("inspect", &a).await?,
            "set_search_continuation" => self.search_control("set_continue", &a).await?,
            "play_search_result" => self.search_control("play", &a).await?,
            "navigate_ui" => self.navigate_ui(parse_ui_navigation_request(&a)?).await?,
            "playback_state" => {
                let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                self.playback_snapshot_page(offset, limit)
            }
            "play_tracks_queued" => {
                self.queued_play_tracks(
                    Self::queued_operation_id(&a)?,
                    string_array(&a, "trackIds", 500)?,
                )
                .await?
            }
            "get_playback_operation" => {
                self.queued_playback_status(&Self::queued_operation_id(&a)?)
            }
            "cancel_playback_operation" => {
                self.cancel_queued_playback(&Self::queued_operation_id(&a)?)
                    .await?
            }
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
                        match self.store.cache_track(track) {
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
                    let signature = json!({"trackId":track.id,"candidateId":candidate.id,"formatId":a.get("formatId"),"outputProfile":a.get("outputProfile"),"chapterIds":a.get("chapterIds"),"disposition":a.get("disposition")});
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
                    disposition: serde_json::from_value(
                        a.get("disposition")
                            .cloned()
                            .unwrap_or_else(|| json!({"mode":"keep"})),
                    )
                    .map_err(|e| ToolError::bad(format!("Invalid file disposition: {e}")))?,
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
                    db[key] = json!({"operation":"download_candidate","signature":{"trackId":job.track.id,"candidateId":job.candidate.id,"formatId":job.format_id,"outputProfile":job.output_profile,"chapterIds":job.chapter_ids,"disposition":a.get("disposition")},"id":job.id});
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
fn require_no_arguments(arguments: &Value, tool_name: &str) -> Result<(), ToolError> {
    if arguments
        .as_object()
        .is_some_and(|object| object.is_empty())
    {
        Ok(())
    } else {
        Err(ToolError::bad(format!("{tool_name} takes no arguments")))
    }
}

fn parse_identification_capture_id(
    arguments: &Value,
    required: bool,
) -> Result<Option<String>, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::bad("Audio identification arguments must be an object"))?;
    if object.keys().any(|key| key != "captureId") {
        return Err(ToolError::bad(
            "Audio identification accepts only a captureId UUID",
        ));
    }
    let Some(value) = object.get("captureId") else {
        return if required {
            Err(ToolError::bad(
                "captureId is required to cancel audio identification",
            ))
        } else {
            Ok(None)
        };
    };
    let value = value
        .as_str()
        .filter(|value| value.len() == 36)
        .ok_or_else(|| ToolError::bad("captureId must be a canonical UUID"))?;
    let parsed =
        Uuid::parse_str(value).map_err(|_| ToolError::bad("captureId must be a canonical UUID"))?;
    if parsed.to_string() != value.to_ascii_lowercase() {
        return Err(ToolError::bad("captureId must be a canonical UUID"));
    }
    Ok(Some(parsed.to_string()))
}

fn cancel_identification_registries(
    capture_id: &str,
    cancel_native: impl FnOnce(&str) -> bool,
    cancel_webview: impl FnOnce(&str) -> bool,
) -> bool {
    // Evaluate both even when the native registry accepts a pre-cancel.
    let native_cancelled = cancel_native(capture_id);
    let webview_cancelled = cancel_webview(capture_id);
    native_cancelled || webview_cancelled
}

fn public_duration_repair_error(error: &str) -> ToolError {
    if error.starts_with("RATE_LIMITED:") {
        ToolError {
            code: "RATE_LIMITED",
            message: "Spotify public duration repair is cooling down; retry later".into(),
        }
    } else if error.to_ascii_lowercase().contains("already running") {
        ToolError {
            code: "BUSY",
            message: "A Spotify duration repair is already running".into(),
        }
    } else if error.to_ascii_lowercase().contains("cancelled") {
        ToolError {
            code: "CANCELLED",
            message: "Spotify public duration repair was cancelled".into(),
        }
    } else {
        ToolError {
            code: "OPERATION_FAILED",
            message: "Public Spotify duration repair failed; check status before retrying".into(),
        }
    }
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
        | "list_tracks_page"
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
        | "get_managed_provider_session_status"
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
        | "list_remote_playlists"
        | "get_remote_playlist"
        | "inspect_spotify_artist_page"
        | "inspect_spotify_profile_page"
        | "inspect_youtube_library_page"
        | "inspect_youtube_playlist_page"
        | "get_remote_track"
        | "prepare_spotify_recording_crosswalk"
        | "prepare_playback_catalog_identity"
        | "browse_remote_artist_tracks"
        | "get_remote_artist"
        | "get_remote_album_tracks"
        | "public_spotify_duration_repair_running"
        | "search_remote_playlist_writeback_candidates"
        | "get_playlist_sync_preview"
        | "list_track_files"
        | "get_remote_playlist_import_job"
        | "playback_state"
        | "capture_main_ui"
        | "inspect_playback_layout"
        | "inspect_webview_media"
        | "get_slskd_search"
        | "start_slskd_search"
        | "cancel_slskd_search"
        | "inspect_search_ui" => "read",
        "play_tracks"
        | "play_tracks_queued"
        | "get_playback_operation"
        | "cancel_playback_operation"
        | "identify_playing_track"
        | "cancel_playing_track_identification"
        | "search_ui"
        | "set_search_continuation"
        | "play_search_result"
        | "interact_queue_ui"
        | "set_player_panel"
        | "navigate_ui"
        | "resize_app_window"
        | "repair_spotify_durations_public"
        | "cancel_public_spotify_duration_repair"
        | "start_managed_provider_login"
        | "confirm_managed_provider_account"
        | "return_youtube_login_to_music"
        | "start_managed_provider_background"
        | "stop_managed_provider_session"
        | "pause_playback"
        | "resume_playback"
        | "queue_tracks"
        | "seek_playback"
        | "set_volume"
        | "create_playlist"
        | "add_tracks_to_playlist" => "control",
        "create_verification_playlist" | "preview_verification_playlist_intent" => "control",
        "apply_verification_playlist_intent" => "manage",
        "set_preferred_track_file" => "control",
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
        "import_remote_playlists" | "refresh_remote_playlist" | "cancel_remote_playlist_import" => {
            "control"
        }
        "apply_playlist_sync_preview" => "manage",
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
// These read-only tools reuse service-owned, account/track-bound review tokens.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WritebackCandidateRequest {
    binding_id: String,
    local_track_id: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WritebackReviewedMatch {
    local_track_id: String,
    candidate_token: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WritebackPreviewRequest {
    binding_id: String,
    #[serde(default)]
    reviewed_matches: Option<Vec<WritebackReviewedMatch>>,
}
fn valid_writeback_id(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max
}
fn parse_writeback_candidates(a: &Value) -> Result<WritebackCandidateRequest, ToolError> {
    let request: WritebackCandidateRequest = serde_json::from_value(a.clone())
        .map_err(|_| ToolError::bad("Invalid playlist candidate-search arguments"))?;
    if !valid_writeback_id(&request.binding_id, 256)
        || !valid_writeback_id(&request.local_track_id, 300)
    {
        return Err(ToolError::bad("Invalid playlist candidate-search identity"));
    }
    Ok(request)
}
fn parse_writeback_preview(a: &Value) -> Result<WritebackPreviewRequest, ToolError> {
    if a.get("reviewedMatches").is_some_and(Value::is_null) {
        return Err(ToolError::bad(
            "reviewedMatches must be an array when provided",
        ));
    }
    let request: WritebackPreviewRequest = serde_json::from_value(a.clone())
        .map_err(|_| ToolError::bad("Invalid playlist preview arguments"))?;
    if !valid_writeback_id(&request.binding_id, 256) {
        return Err(ToolError::bad("Invalid playlist binding identity"));
    }
    if let Some(matches) = &request.reviewed_matches {
        // Service retains at most 1000 review tokens; no URI/raw candidate can be supplied.
        if matches.len() > 1000 {
            return Err(ToolError::bad("Too many reviewed matches"));
        }
        let mut tracks = std::collections::HashSet::new();
        let mut tokens = std::collections::HashSet::new();
        for entry in matches {
            if !valid_writeback_id(&entry.local_track_id, 300)
                || entry.candidate_token.len() != 36
                || Uuid::parse_str(&entry.candidate_token).is_err()
                || !tracks.insert(&entry.local_track_id)
                || !tokens.insert(&entry.candidate_token)
            {
                return Err(ToolError::bad("Invalid or duplicate reviewed match"));
            }
        }
    }
    Ok(request)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationIntentApproval {
    provider: String,
    #[serde(default)]
    intent_id: Option<String>,
    #[serde(default)]
    review_token: Option<String>,
    #[serde(default)]
    contract_hash: Option<String>,
}
fn parse_verification_intent_approval(
    a: &Value,
    apply: bool,
) -> Result<VerificationIntentApproval, ToolError> {
    let r: VerificationIntentApproval = serde_json::from_value(a.clone())
        .map_err(|_| ToolError::bad("Invalid verification intent arguments"))?;
    if !matches!(r.provider.as_str(), "spotify" | "youtube") {
        return Err(ToolError::bad("Choose Spotify or YouTube"));
    }
    let hash = |v: Option<&str>| {
        v.is_some_and(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    };
    if apply {
        if !r.intent_id.as_deref().is_some_and(|id| {
            Uuid::parse_str(id)
                .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == id)
        }) || !hash(r.review_token.as_deref())
            || !hash(r.contract_hash.as_deref())
        {
            return Err(ToolError::bad(
                "Approval requires exact intent ID, token and contract hash",
            ));
        }
    } else if a.as_object().is_none_or(|o| o.len() != 1) {
        return Err(ToolError::bad("Preview accepts only provider"));
    }
    Ok(r)
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
fn optional_u32_arg(a: &Value, k: &str) -> Result<Option<u32>, ToolError> {
    a.get(k)
        .map(|value| {
            value
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .ok_or_else(|| ToolError::bad(format!("{k} must be a non-negative 32-bit integer")))
        })
        .transpose()
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
fn spotify_id_arg(a: &Value, key: &str) -> Result<String, ToolError> {
    let id = a
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .ok_or_else(|| {
            ToolError::bad(format!(
                "{key} must be a 22-character alphanumeric Spotify ID"
            ))
        })?;
    Ok(id.to_owned())
}
fn numeric_spotify_cursor_arg(a: &Value) -> Result<Option<String>, ToolError> {
    a.get("cursor")
        .filter(|value| !value.is_null())
        .map(|value| {
            let cursor = value
                .as_str()
                .filter(|s| {
                    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                        return false;
                    }
                    match s.parse::<u64>() {
                        Ok(n) => n <= 100_000 && n.to_string() == *s,
                        Err(_) => false,
                    }
                })
                .ok_or_else(|| {
                    ToolError::bad("cursor must be a canonical decimal string from 0 to 100000")
                })?;
            Ok(cursor.to_owned())
        })
        .transpose()
}
fn discovery_cursor_arg(a: &Value) -> Result<Option<String>, ToolError> {
    a.get("cursor")
        .map(|value| {
            value
                .as_str()
                .filter(|cursor| cursor.len() <= crate::discovery::MAX_DISCOVERY_CURSOR_LENGTH)
                .map(str::to_owned)
                .ok_or_else(|| {
                    ToolError::bad(format!(
                        "discovery cursor must be a string of at most {} bytes",
                        crate::discovery::MAX_DISCOVERY_CURSOR_LENGTH
                    ))
                })
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
fn parse_catalog_identity_request(arguments: &Value) -> Result<String, ToolError> {
    arguments.as_object()
        .filter(|object| object.len() == 1 && object.contains_key("trackId"))
        .ok_or_else(|| ToolError::bad("Catalog identity preparation accepts only trackId"))?;
    str_arg(arguments, "trackId", 256)
}

fn catalog_identity_summary(prepared: Option<&Track>, elapsed_ms: u128) -> Value {
    let duration = prepared.and_then(|track| track.duration_seconds)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    json!({
        "enriched": duration.is_some(),
        "durationSeconds": duration,
        "isrcKnown": duration.is_some() && prepared.and_then(|track| track.isrc.as_deref()).is_some_and(|code| !code.trim().is_empty()),
        "provider": "spotify",
        "elapsedMs": elapsed_ms.min(u64::MAX as u128) as u64,
    })
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
        "sortField",
        "direction",
        "minDurationSeconds",
        "maxDurationSeconds",
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
    let sort_field = optional_str_arg(arguments, "sortField", 32)?;
    if sort_field.as_deref().is_some_and(|field| {
        !matches!(
            field,
            "title"
                | "artist"
                | "album"
                | "duration"
                | "releaseDate"
                | "dateAdded"
                | "sourceOrder"
                | "bpm"
                | "key"
                | "plays"
                | "views"
        )
    }) {
        return Err(ToolError::bad("sortField is not supported"));
    }
    let direction = optional_str_arg(arguments, "direction", 8)?;
    if direction
        .as_deref()
        .is_some_and(|direction| !matches!(direction, "asc" | "desc"))
    {
        return Err(ToolError::bad("direction must be asc or desc"));
    }
    let duration_bound = |key: &str| -> Result<Option<f64>, ToolError> {
        let Some(value) = arguments.get(key) else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        let number = value
            .as_f64()
            .filter(|number| number.is_finite() && *number >= 0.0)
            .ok_or_else(|| ToolError::bad(format!("{key} must be a finite nonnegative number")))?;
        Ok(Some(number))
    };
    let min_duration_seconds = duration_bound("minDurationSeconds")?;
    let max_duration_seconds = duration_bound("maxDurationSeconds")?;
    if min_duration_seconds
        .zip(max_duration_seconds)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(ToolError::bad(
            "minDurationSeconds must not exceed maxDurationSeconds",
        ));
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
        sort_field,
        direction,
        min_duration_seconds,
        max_duration_seconds,
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
fn strip_native_playback_fields(value: &mut Value) {
    strip_file_fields(value);
    strip_private_playback_urls(value);
}

fn page_playback_snapshot(snapshot: &Value, offset: usize, limit: usize) -> Value {
    let limit = limit.clamp(1, 200);
    let queue = snapshot["queue"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = snapshot["queueTotal"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(queue.len());
    let start = offset.min(total).min(queue.len());
    let end = start.saturating_add(limit).min(queue.len());
    let mut page = snapshot
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "queue"
                            | "queueEntryIds"
                            | "queueTotal"
                            | "queueOffset"
                            | "queueLimit"
                            | "queueHasMore"
                    )
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));
    let entry_ids = snapshot["queueEntryIds"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    page["queue"] = json!(queue[start..end]);
    page["queueEntryIds"] = json!(entry_ids.get(start..end).unwrap_or(&[]));
    page["queueTotal"] = json!(total);
    page["queueOffset"] = json!(offset.min(total));
    page["queueLimit"] = json!(limit);
    page["queueHasMore"] = json!(offset.saturating_add(limit) < total);
    page
}

fn strip_private_playback_urls(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|_, child| !child.as_str().is_some_and(is_private_or_signed_url));
            for child in object.values_mut() {
                strip_private_playback_urls(child);
            }
        }
        Value::Array(items) => {
            items.retain(|child| !child.as_str().is_some_and(is_private_or_signed_url));
            for child in items {
                strip_private_playback_urls(child);
            }
        }
        _ => {}
    }
}
fn is_private_or_signed_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    if let Some(host) = url.host_str() {
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        if matches!(host.as_str(), "localhost" | "localhost.localdomain")
            || host.ends_with(".localhost")
            || host.ends_with(".local")
        {
            return true;
        }
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            let private = match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_loopback()
                        || ip.is_unique_local()
                        || ip.is_unicast_link_local()
                        || ip.is_unspecified()
                }
            };
            if private {
                return true;
            }
        }
    }
    url.query_pairs().any(|(key, _)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "sig"
                | "signature"
                | "lsig"
                | "token"
                | "access_token"
                | "auth"
                | "authorization"
                | "credential"
                | "x-amz-signature"
                | "x-amz-credential"
                | "x-goog-signature"
                | "x-goog-credential"
                | "expires"
                | "expire"
                | "policy"
                | "key-pair-id"
                | "hdnea"
                | "pot"
        )
    })
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
        Ok(mut result) => {
            s.log(&name, "ok", started.elapsed().as_millis());
            if name == "capture_main_ui" {
                let data = result
                    .as_object_mut()
                    .and_then(|object| object.remove("data"))
                    .unwrap_or(Value::Null);
                json!({"content":[{"type":"image","mimeType":"image/png","data":data}],"structuredContent":{"result":result},"isError":false})
            } else {
                json!({"content":[{"type":"text","text":result.to_string()}],"structuredContent":{"result":result},"isError":false})
            }
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
 ("get_managed_provider_session_status","Get redacted provider session and credential-storage status", "read"),
 ("search_ui","Run a search in the visible app and return sanitized source counts", "control"),
 ("inspect_search_ui","Read bounded currently loaded playable rows from the visible search UI", "read"),
 ("set_search_continuation","Set the visible search continuation preference for loaded results", "control"),
 ("play_search_result","Play one exact currently loaded search occurrence by its expiring UI token", "control"),
 ("interact_queue_ui","Dispatch finite synthetic queue DOM events in a marked isolated paused native profile; not a trusted OS gesture", "control"),
 ("set_player_panel","Open the playback queue, track information panel, or close the player panel", "control"),
 ("navigate_ui","Navigate to a finite view, one settings category, the Playback startup timeout control, exact Spotify artist/profile/playlist, a validated MusicBrainz/Discogs catalog artist and section, or Back/Forward. settingsCategory is allowed only with view=settings; settingsControl is allowed only for playbackStartupTimeout under Playback settings", "control"),
 ("start_managed_provider_login","Open the isolated provider sign-in window; login remains interactive", "control"),
 ("preview_verification_playlist_intent","Prepare an exact new private empty UUID-title intent locally; requires explicit review and never creates remotely", "control"),("apply_verification_playlist_intent","Create a separately explicitly approved private empty intent exactly once; never retry unknown outcomes", "manage"),("create_verification_playlist","Create exactly one private empty Burrata verification playlist for the confirmed account; never automatically retry an unknown result", "control"),
 ("confirm_managed_provider_account","Verify the account in the owned login window and enable provider access", "control"),
 ("return_youtube_login_to_music","After completing Google sign-in, return the owned YouTube window to Music; requires the Google sign-in origin and a valid YouTube authentication cookie", "control"),
 ("start_managed_provider_background","Enable the confirmed provider session without reopening login", "control"),
 ("stop_managed_provider_session","Stop provider jobs and close the owned login window", "control"),
 ("search_library","Search local library", "read"),("list_tracks_page","Read one filtered and sorted page from a saved library view", "read"),("get_track","Get a library track", "read"),("library_stats","Get library counts", "read"),("list_playlists","List playlists and browser snapshot versions", "read"),("get_playlist","Get playlist tracks and browser versions", "read"),("search_tracks","Search catalog providers", "read"),("search_artists","Search artists", "read"),("get_artist_discography","Get paginated artist discography", "read"),("get_release_tracks","Get ordered tracks for an artist release", "read"),("get_artist_tracks","Get paginated recordings for an artist", "read"),("get_artist_collaborations","Get paginated artist relationships", "read"),("get_artist_profile","Get public artist profile and saved top tracks", "read"),("get_app_status","Get application and playback status", "read"),("capture_main_ui","Capture only the main Burrata UI as a bounded PNG, without desktop capture or focus changes", "read"),("inspect_playback_layout","Inspect visible player UI geometry and report layout overlaps; read-only, returns no playback metadata", "read"),("inspect_webview_media","Read current WebView audio-element state; read-only and not a physical-audibility measurement", "read"),("resize_app_window","Resize the main application in logical pixels", "control"),("get_song_radio","Get bounded YouTube autoplay Mix recommendations for a track; does not alter playback", "read"),("search_playback_history","Search recent playback history", "read"),("list_playback_history","List recent playback history", "read"),("get_playback_history_entry","Get a playback history entry by session ID", "read"),
 ("discover_music","Search selected music sources with independent result pages", "read"),("poll_music_search","Poll a Soulseek discovery search", "read"),("inspect_source_chapters","Inspect source chapters and register stable chapter tracks", "read"),("get_autotag_status","Get paginated autotag review jobs", "read"),("retry_autotag","Retry an autotag job", "manage"),("review_autotag_candidate","Select a catalog candidate for autotag review", "manage"),("apply_autotag_review","Apply approved autotag fields", "manage"),("play_chapter","Play a registered chapter track", "control"),
 ("inspect_spotify_artist_page","Inspect fixed Spotify artist-list guard counts and enums without content or identifiers", "read"),
 ("inspect_spotify_profile_page","Inspect one bounded Spotify profile page or full-profile summary; returns ordered playlist-ID hashes and counts only", "read"),
 ("inspect_youtube_library_page","Inspect fixed own YouTube library response counts and flags without exposing content or identifiers", "read"),
 ("inspect_youtube_playlist_page","Inspect fixed YouTube playlist response counts and flags without exposing content or identifiers", "read"),
 ("browse_remote_artist_tracks","Read one bounded Spotify artist track page without importing", "read"),
 ("list_remote_playlists","List available Spotify or YouTube playlists", "read"),("get_remote_playlist","Browse remote playlist tracks without importing into the library", "read"),("get_remote_track","Read one exact Spotify track by catalog ID without importing it", "read"),("prepare_playback_catalog_identity","Prepare independent Spotify catalog duration for one saved track within four seconds; returns only enrichment status, duration, ISRC-known and elapsed time. Cache-only, no saved tags or queue changes", "read"),("prepare_spotify_recording_crosswalk","Verify an exact Spotify track against a linked MusicBrainz release, or verify complete artist-ID credits against an explicitly selected MusicBrainz recording; read-only, cached for playback identity, no tag changes", "read"),("get_remote_artist","Read a Spotify artist and one page of releases", "read"),("get_remote_album_tracks","Read a Spotify album and one page of tracks", "read"),("repair_spotify_durations_public","Fill missing Spotify durations from public metadata for up to 300 local tracks in 60 seconds; no account or playlist writeback", "control"),("cancel_public_spotify_duration_repair","Cancel the active public Spotify duration repair", "control"),("public_spotify_duration_repair_running","Check whether public Spotify duration repair is running", "read"),("import_remote_playlists","Import selected remote playlists into the library", "control"),("get_remote_playlist_import_job","Get status for a remote playlist import", "read"),("cancel_remote_playlist_import","Cancel an active remote playlist import", "control"),("refresh_remote_playlist","Refresh an imported remote playlist", "control"),("search_remote_playlist_writeback_candidates","Search provider candidates for one local playlist track; returns expiring review tokens without writing provider data", "read"),("get_playlist_sync_preview","Preview pending remote playlist changes with optional explicitly reviewed candidate tokens", "read"),("apply_playlist_sync_preview","Apply a reviewed remote playlist sync preview", "manage"),("list_track_files","List local files linked to a track", "read"),("set_preferred_track_file","Choose the preferred local file for a track", "control"),
 ("list_radio_stations","List built-in and saved radio stations", "read"),("search_radio_stations","Search radio stations by name, country, or tag", "read"),("get_station_details","Get details and available current metadata for a station", "read"),("add_radio_station","Save a selected Radio Browser search result", "control"),("add_custom_radio_station","Save a station from a stream URL", "control"),("remove_radio_station","Remove a saved station or hide a built-in station", "control"),("rename_radio_station","Rename a saved or built-in station", "control"),("play_radio_station","Play a saved or built-in radio station", "control"),
 ("identify_playing_track","Identify current audio on explicit request; returns a separate result without editing tags", "control"),
 ("cancel_playing_track_identification","Cancel one native or WebView audio identification request by its captureId UUID", "control"),
 ("play_tracks_queued","Accept an exact playback queue without waiting for sound; caller UUID permits receipt recovery", "control"),("get_playback_operation","Get queued playback operation status by caller UUID", "control"),("cancel_playback_operation","Cancel only the named playback operation; supports pre-cancel", "control"),
 ("playback_state","Get player state", "control"),("play_tracks","Play specific track IDs", "control"),("pause_playback","Pause playback", "control"),("resume_playback","Resume playback", "control"),("queue_tracks","Queue specific track IDs", "control"),("seek_playback","Seek in seconds", "control"),("set_volume","Set volume from 0 to 1", "control"),("create_playlist","Create a playlist", "control"),("add_tracks_to_playlist","Append track IDs to a playlist", "control"),
 ("search_audio_sources","Search audio sources for a track or query", "read"),("inspect_audio_source","Inspect formats and chapters for an explicit source candidate", "read"),("start_slskd_search","Start a Soulseek search", "read"),("get_slskd_search","Poll a Soulseek search", "read"),("cancel_slskd_search","Cancel a Soulseek search", "read"),("download_candidate","Download one explicitly selected candidate", "manage"),("download_jobs","List download jobs", "read"),("retry_download","Retry a failed download", "manage"),("cancel_download","Cancel a download", "manage"),("analyze_library","Queue library analysis", "manage"),("analysis_status","Get analysis status", "read"),("export_mixxx_playlist","Export playlist for Mixxx", "manage")].into_iter().map(|(name,description,_scope)|{
 let scope=tool_scope(name).unwrap_or("read");
 let (properties,required)=match name{
 "preview_verification_playlist_intent"=>(json!({"provider":{"type":"string","enum":["spotify","youtube"]}}),json!(["provider"])),
 "apply_verification_playlist_intent"=>(json!({"provider":{"type":"string","enum":["spotify","youtube"]},"intentId":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$","minLength":36,"maxLength":36},"reviewToken":{"type":"string","pattern":"^[0-9a-f]{64}$","minLength":64,"maxLength":64},"contractHash":{"type":"string","pattern":"^[0-9a-f]{64}$","minLength":64,"maxLength":64}}),json!(["provider","intentId","reviewToken","contractHash"])),
 "create_verification_playlist"|"start_managed_provider_login"|"confirm_managed_provider_account"=>(json!({"provider":{"type":"string","enum":["spotify","youtube"]}}),json!(["provider"])),
 "list_radio_stations"=>(json!({}),json!([])),
 "search_radio_stations"=>(json!({"query":{"type":"string","maxLength":120},"country":{"type":"string","maxLength":80},"tag":{"type":"string","maxLength":80},"limit":{"type":"integer","minimum":1,"maximum":50}}),json!([])),
 "get_station_details"|"play_radio_station"=>(json!({"stationId":{"type":"string","maxLength":120}}),json!(["stationId"])),
 "add_radio_station"=>(json!({"station":{"type":"object","description":"A structured Radio Browser station result returned by search_radio_stations"}}),json!(["station"])),
 "add_custom_radio_station"=>(json!({"name":{"type":"string","minLength":1,"maxLength":160},"url":{"type":"string","format":"uri","maxLength":2048}}),json!(["name","url"])),
 "remove_radio_station"=>(json!({"stationId":{"type":"string","maxLength":120}}),json!(["stationId"])),
 "rename_radio_station"=>(json!({"stationId":{"type":"string","maxLength":120},"name":{"type":"string","minLength":1,"maxLength":160}}),json!(["stationId","name"])),
 "get_song_radio"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"},"limit":{"type":"integer","minimum":1,"maximum":50}}),json!(["trackId","candidate"])),
 "discover_music"=>(json!({"query":{"type":"string","maxLength":500},"sourceIds":{"type":"array","maxItems":20,"items":{"type":"string","enum":DISCOVERY_SOURCE_IDS}},"kinds":{"type":"array","maxItems":10,"items":{"type":"string","enum":["artist","album","track","video","set","file","playlist","profile","channel"]}},"cursor":{"type":"string","maxLength":crate::discovery::MAX_DISCOVERY_CURSOR_LENGTH},"limit":{"type":"integer","minimum":1,"maximum":100},"isrc":{"type":"string","maxLength":32}}),json!(["query"])),
 "inspect_spotify_artist_page"=>(json!({"artistId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$"},"cursor":{"type":"string","pattern":"^(?:0|[1-9][0-9]{0,5})$","maxLength":6}}),json!(["artistId"])),
 "inspect_spotify_profile_page"=>(json!({"ownerId":{"type":"string","minLength":1,"maxLength":2048,"description":"An official Spotify user-profile URL or legacy profile ID"},"mode":{"type":"string","enum":["paged","summary"],"default":"paged"},"offset":{"type":"integer","minimum":0,"maximum":10000000,"default":0},"limit":{"type":"integer","minimum":1,"maximum":30,"default":30},"playlistLimit":{"type":"integer","minimum":1,"maximum":200,"default":200,"description":"Summary mode only"}}),json!(["ownerId"])),
 "inspect_youtube_library_page"=>(json!({}),json!([])),
 "inspect_youtube_playlist_page"=>(json!({"playlistId":{"type":"string","pattern":"^(?:LM|SE|LL|WL|[A-Za-z0-9_-]{10,80})$","maxLength":80},"cursor":{"type":"string","minLength":1,"maxLength":2048}}),json!(["playlistId"])),
 "get_remote_playlist"=>(json!({"provider":{"type":"string","enum":["spotify","youtube"]},"playlistId":{"type":"string","maxLength":2048},"cursor":{"type":"string","maxLength":2048}}),json!(["provider","playlistId"])),
 "get_remote_track"=>(json!({"provider":{"type":"string","enum":["spotify"]},"trackId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$"}}),json!(["provider","trackId"])),
 "prepare_playback_catalog_identity"=>(json!({"trackId":{"type":"string","minLength":1,"maxLength":256,"description":"Exact existing saved-track ID; no supplied metadata or URL"}}),json!(["trackId"])),
 "prepare_spotify_recording_crosswalk"=>(json!({"trackId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$"},"albumIdHint":{"type":"string","pattern":"^[A-Za-z0-9]{22}$","description":"Optional candidate album locator. The complete album read must independently contain the requested track exactly once."},"musicBrainzRecordingIdHint":{"type":"string","pattern":"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$","description":"Optional locator for a user-selected MusicBrainz recording. Exact stored Spotify metadata, ordered artist IDs, duration, ISRC and artist URL relations must independently match."},"requestId":{"type":"string","format":"uuid","pattern":"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"}}),json!(["trackId","requestId"])),
 "browse_remote_artist_tracks"|"get_remote_artist"=>(json!({"artistId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$"},"cursor":{"type":["string","null"],"pattern":"^[0-9]{1,6}$","maxLength":6}}),json!(["artistId"])),
 "get_remote_album_tracks"=>(json!({"albumId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$"},"cursor":{"type":["string","null"],"pattern":"^[0-9]{1,6}$","maxLength":6}}),json!(["albumId"])),
 "list_remote_playlists"=>(json!({"provider":{"type":"string","enum":["spotify","youtube"]},"ownerId":{"type":"string","maxLength":256},"cursor":{"type":"string","maxLength":512}}),json!(["provider"])),
 "import_remote_playlists"=>(json!({"provider":{"type":"string","enum":["spotify","youtube"]},"playlistIds":{"type":"array","minItems":1,"maxItems":20,"items":{"type":"string","maxLength":256}}}),json!(["provider","playlistIds"])),
 "get_remote_playlist_import_job"|"cancel_remote_playlist_import"=>(json!({"jobId":{"type":"string","minLength":1,"maxLength":256}}),json!(["jobId"])),
 "refresh_remote_playlist"=>(json!({"bindingId":{"type":"string","minLength":1,"maxLength":256}}),json!(["bindingId"])),
 "search_remote_playlist_writeback_candidates"=>(json!({"bindingId":{"type":"string","minLength":1,"maxLength":256},"localTrackId":{"type":"string","minLength":1,"maxLength":300}}),json!(["bindingId","localTrackId"])),
 "get_playlist_sync_preview"=>(json!({"bindingId":{"type":"string","minLength":1,"maxLength":256},"reviewedMatches":{"type":"array","maxItems":1000,"items":{"type":"object","properties":{"localTrackId":{"type":"string","minLength":1,"maxLength":300},"candidateToken":{"type":"string","minLength":36,"maxLength":36,"pattern":"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"}},"required":["localTrackId","candidateToken"],"additionalProperties":false}}}),json!(["bindingId"])),
 "apply_playlist_sync_preview"=>(json!({"previewId":{"type":"string","minLength":1,"maxLength":256},"changes":{"type":"object","description":"Optional reviewed change set returned by get_playlist_sync_preview; edits are checked against the saved preview."}}),json!(["previewId"])),
 "list_track_files"=>(json!({"trackId":{"type":"string","minLength":1,"maxLength":300}}),json!(["trackId"])),
 "set_preferred_track_file"=>(json!({"trackId":{"type":"string","minLength":1,"maxLength":300},"assetId":{"type":"string","minLength":1,"maxLength":300}}),json!(["trackId","assetId"])),
 "poll_music_search"=>(json!({"searchId":{"type":"string","maxLength":300}}),json!(["searchId"])),"inspect_source_chapters"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"}}),json!(["trackId","candidate"])),"get_autotag_status"=>(json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),"retry_autotag"=>(json!({"assetId":{"type":"string"}}),json!(["assetId"])),"review_autotag_candidate"=>(json!({"assetId":{"type":"string"},"providerId":{"type":"string"}}),json!(["assetId","providerId"])),"apply_autotag_review"=>(json!({"assetId":{"type":"string"},"fieldKeys":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"string","enum":["artist","title","recordingDate"]}}}),json!(["assetId","fieldKeys"])),"play_chapter"=>(json!({"trackId":{"type":"string"}}),json!(["trackId"])),
 "search_ui"=>(json!({"query":{"type":"string","minLength":1,"maxLength":500},"sourceId":{"type":"string","enum":SEARCH_UI_SOURCE_IDS},"kind":{"type":"string","enum":SEARCH_UI_KINDS}}),json!(["query","sourceId","kind"])),
 "inspect_search_ui"=>(json!({"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":{"type":"integer","minimum":1,"maximum":200}}),json!([])),
 "set_search_continuation"=>(json!({"revision":{"type":"string","format":"uuid","minLength":36,"maxLength":36},"enabled":{"type":"boolean"}}),json!(["revision","enabled"])),
 "play_search_result"=>(json!({"revision":{"type":"string","format":"uuid","minLength":36,"maxLength":36},"occurrenceToken":{"type":"string","format":"uuid","minLength":36,"maxLength":36}}),json!(["revision","occurrenceToken"])),
 "set_player_panel"=>(json!({"panel":{"type":"string","enum":["queue","info","closed"]}}),json!(["panel"])),
 "identify_playing_track"=>(json!({"captureId":{"type":"string","minLength":36,"maxLength":36,"pattern":"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$","description":"Optional caller-generated UUID; pass the same value to cancel_playing_track_identification. Omit it for legacy uncancellable calls."}}),json!([])),
 "cancel_playing_track_identification"=>(json!({"captureId":{"type":"string","minLength":36,"maxLength":36,"pattern":"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"}}),json!(["captureId"])),
 "navigate_ui"=>(json!({"catalogArtist":{"type":"object","properties":{"provider":{"type":"string","enum":["musicbrainz","discogs"]},"id":{"type":"string","minLength":1,"maxLength":36}},"required":["provider","id"],"additionalProperties":false,"oneOf":[{"properties":{"provider":{"const":"musicbrainz"},"id":{"pattern":"^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$"}}},{"properties":{"provider":{"const":"discogs"},"id":{"pattern":"^[1-9][0-9]{0,19}$","maxLength":20}}}]},"catalogArtistSection":{"type":"string","enum":["overview","releases","tracks"],"description":"Optional catalog artist section; requires catalogArtist, defaults to overview"},"view":{"type":"string","enum":["library","favorites","history","downloads","settings","radio"]},"settingsCategory":{"type":"string","enum":UI_SETTINGS_CATEGORIES,"description":"Optional existing category; accepted only with view=settings"},"settingsControl":{"type":"string","enum":["playbackStartupTimeout"],"description":"Optional fixed control; requires view=settings and settingsCategory=playback"},"spotifyArtistId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$"},"spotifyProfileId":{"type":"string","minLength":1,"maxLength":256,"description":"Canonical Spotify user ID only; no URL, whitespace, separators or query"},"spotifyPlaylistId":{"type":"string","pattern":"^[A-Za-z0-9]{22}$","minLength":22,"maxLength":22},"historyDirection":{"type":"string","enum":["back","forward"]}}),json!([])),
 "repair_spotify_durations_public"|"cancel_public_spotify_duration_repair"|"public_spotify_duration_repair_running"=>(json!({}),json!([])),
 "search_library"=>(json!({"query":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!(["query"])),
 "list_tracks_page"=>(json!({"view":{"type":"string","enum":["library","favorites","shortlist","collections"]},"collectionId":{"type":"string","maxLength":256},"query":{"type":"string","maxLength":500},"sortField":{"type":"string","enum":["sourceOrder","rank","title","artist","album","duration","durationSeconds","releaseDate","dateAdded","bpm","key","plays","views"]},"direction":{"type":"string","enum":["asc","desc"]},"minDurationSeconds":{"type":"integer","minimum":0,"maximum":4294967295u64},"maxDurationSeconds":{"type":"integer","minimum":0,"maximum":4294967295u64},"availability":{"type":"string","enum":["all","local","not-local"]},"provider":{"type":"string","maxLength":80},"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":{"type":"integer","minimum":1,"maximum":200}}),json!(["view"])),
 "search_playback_history"|"list_playback_history"=>(json!({"query":{"type":"string","maxLength":300},"provider":{"type":"string","maxLength":120,"description":"Exact playback provider filter. Use radio for radio songs or non-radio to exclude them."},"stationId":{"type":"string","maxLength":120,"description":"Filter radio playback history to one station"},"from":{"type":"string","maxLength":64,"description":"Inclusive lower bound as YYYY-MM-DD or RFC3339"},"to":{"type":"string","maxLength":64,"description":"Inclusive upper bound as YYYY-MM-DD or RFC3339"},"sort":{"type":"string","enum":["newest","oldest"]},"sortField":{"type":"string","enum":["title","artist","album","duration","releaseDate","dateAdded","sourceOrder","bpm","key","plays","views"]},"direction":{"type":"string","enum":["asc","desc"]},"minDurationSeconds":{"type":"number","minimum":0},"maxDurationSeconds":{"type":"number","minimum":0},"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),
 "get_playback_history_entry"=>(json!({"sessionId":{"type":"string","minLength":1,"maxLength":300}}),json!(["sessionId"])),
 "get_track"=>(json!({"trackId":{"type":"string"}}),json!(["trackId"])),"list_playlists"=>(json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),"get_playlist"=>(json!({"playlistId":{"type":"string"},"snapshotId":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!(["playlistId"])),
 "search_tracks"=>(json!({"query":{"type":"string"},"providers":{"type":"array","items":{"type":"string","enum":["library","musicbrainz","youtube","soundcloud","soulseek","monochrome"]}}}),json!(["query"])),
 "search_artists"=>(json!({"query":{"type":"string"},"page":{"type":"integer","minimum":1,"maximum":200}}),json!(["query"])),"get_artist_discography"|"get_artist_tracks"|"get_artist_collaborations"=>(json!({"artist":{"type":"object","properties":{"provider":{"type":"string","enum":["musicbrainz","discogs"]},"id":{"type":"string"}},"required":["provider","id"],"additionalProperties":false},"cursor":{"type":"string"}}),json!(["artist"])),"get_artist_profile"=>(json!({"artist":{"type":"object","properties":{"provider":{"type":"string","enum":["musicbrainz","discogs"]},"id":{"type":"string"}},"required":["provider","id"],"additionalProperties":false}}),json!(["artist"])),"get_release_tracks"=>(json!({"release":release_ref_schema(),"artist":artist_ref_schema()}),json!(["release","artist"])),
 "playback_state"=>(json!({"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":{"type":"integer","minimum":1,"maximum":200}}),json!([])),
 "inspect_webview_media"=>(json!({}),json!([])),
 "resize_app_window"=>(json!({"width":{"type":"integer","minimum":320,"maximum":7680},"height":{"type":"integer","minimum":240,"maximum":4320},"restoreMaximized":{"type":"boolean","description":"Whether to maximize the window after resizing; omitted preserves its prior maximized state."}}),json!(["width","height"])),
 "play_tracks_queued"=>(json!({"trackIds":{"type":"array","minItems":1,"maxItems":500,"items":{"type":"string"}},"operationId":{"type":"string","format":"uuid","minLength":36,"maxLength":36}}),json!(["trackIds","operationId"])),
 "get_playback_operation"|"cancel_playback_operation"=>(json!({"operationId":{"type":"string","format":"uuid","minLength":36,"maxLength":36}}),json!(["operationId"])),
 "play_tracks"=>(json!({"trackIds":{"type":"array","minItems":1,"maxItems":500,"items":{"type":"string"}}}),json!(["trackIds"])),"queue_tracks"=>(json!({"trackIds":{"type":"array","minItems":1,"maxItems":500,"items":{"type":"string"}},"mode":{"type":"string","enum":["append","replace","next"]}}),json!(["trackIds"])),"seek_playback"=>(json!({"position":{"type":"number","minimum":0}}),json!(["position"])),"set_volume"=>(json!({"volume":{"type":"number","minimum":0,"maximum":1}}),json!(["volume"])),
 "create_playlist"=>(json!({"name":{"type":"string"},"idempotencyKey":{"type":"string"}}),json!(["name"])),"add_tracks_to_playlist"=>(json!({"playlistId":{"type":"string"},"trackIds":{"type":"array","items":{"type":"string"}},"idempotencyKey":{"type":"string"}}),json!(["playlistId","trackIds"])),
 "search_audio_sources"=>(json!({"trackId":{"type":"string"},"query":{"type":"string"},"providers":{"type":"array","items":{"type":"string","enum":["youtube","soundcloud","monochrome"]}}}),json!(["providers"])),"inspect_audio_source"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"}}),json!(["trackId","candidate"])),"start_slskd_search"=>(json!({"query":{"type":"string"}}),json!(["query"])),"get_slskd_search"|"cancel_slskd_search"=>(json!({"searchId":{"type":"string"}}),json!(["searchId"])),"download_candidate"=>(json!({"trackId":{"type":"string"},"candidate":{"type":"object"},"formatId":{"type":"string"},"outputProfile":{"type":"string","enum":["source","mp3"]},"chapterIds":{"type":"array","items":{"type":"string"}},"disposition":{"type":"object","properties":{"mode":{"type":"string","enum":["keep","replace"]},"targetAssetId":{"type":"string"},"expectedHash":{"type":"string"}},"required":["mode"],"additionalProperties":false},"idempotencyKey":{"type":"string"}}),json!(["trackId","candidate"])),
 "retry_download"|"cancel_download"=>(json!({"jobId":{"type":"string"}}),json!(["jobId"])),"download_jobs"|"analysis_status"=>(json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),json!([])),"export_mixxx_playlist"=>(json!({"playlistId":{"type":"string"}}),json!(["playlistId"])),"analyze_library"=>(json!({"assetIds":{"type":"array","items":{"type":"string"}},"force":{"type":"boolean"}}),json!([])),_ => (json!({}),json!([]))};
 let mut input_schema=json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
 if name=="search_radio_stations" { input_schema["properties"]["query"]["maxLength"]=json!(100); input_schema["anyOf"]=json!([{"required":["query"]},{"required":["country"]},{"required":["tag"]}]); }
 if name=="interact_queue_ui" { let index=json!({"type":"integer","minimum":0,"maximum":199});input_schema=json!({"type":"object","oneOf":[
 {"type":"object","properties":{"action":{"const":"select"},"index":index,"modifier":{"enum":["none","ctrl","shift"]}},"required":["action","index","modifier"],"additionalProperties":false},
 {"type":"object","properties":{"action":{"const":"arrow"},"index":index,"direction":{"enum":["up","down"]}},"required":["action","index","direction"],"additionalProperties":false},
 {"type":"object","properties":{"action":{"const":"drag"},"from":index,"to":index},"required":["action","from","to"],"additionalProperties":false}]}); }
 if name=="navigate_ui" { input_schema["oneOf"]=json!([{"required":["view"]},{"required":["spotifyArtistId"]},{"required":["spotifyProfileId"]},{"required":["spotifyPlaylistId"]},{"required":["historyDirection"]},{"required":["catalogArtist"]}]); input_schema["if"]=json!({"required":["settingsCategory"]}); input_schema["then"]=json!({"required":["view"],"properties":{"view":{"const":"settings"}}}); input_schema["allOf"]=json!([{"if":{"required":["settingsControl"]},"then":{"required":["view","settingsCategory"],"properties":{"view":{"const":"settings"},"settingsCategory":{"const":"playback"}}}},{"if":{"required":["catalogArtistSection"]},"then":{"required":["catalogArtist"]}}]); }
 json!({"name":name,"description":description,"scope":scope,"inputSchema":input_schema})
 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_control_completions_are_bounded_and_action_typed() {
        let revision = "123e4567-e89b-12d3-a456-426614174000";
        let row = json!({"occurrenceToken":"123e4567-e89b-12d3-a456-426614174001","sourceId":"youtube","title":"Song","artist":"Artist","durationSeconds":120,"secret":"test-fixture-credential-7f3a9c1d02"});
        let inspect = json!({"status":"ready","revision":revision,"query":"song","queryInputPending":false,"mode":"online","sourceId":"youtube","kind":"tracks","continueSearch":false,"total":1,"offset":0,"hasMore":true,"collecting":false,"rows":[row],"secret":"test-fixture-credential-7f3a9c1d02"});
        let safe = sanitize_search_control_result("inspect", &inspect).unwrap();
        assert_eq!(safe["rows"][0].as_object().unwrap().len(), 5);
        assert!(safe.get("secret").is_none());
        assert!(sanitize_search_control_result("play", &inspect).is_err());
        let mut oversized = inspect.clone();
        oversized["rows"] = json!(vec![
            json!({"occurrenceToken":"123e4567-e89b-12d3-a456-426614174001","sourceId":"youtube","title":"Song","artist":"Artist","durationSeconds":120});
            201
        ]);
        assert!(sanitize_search_control_result("inspect", &oversized).is_err());
        let mut bad_title = inspect.clone();
        bad_title["rows"][0]["title"] = json!("t".repeat(301));
        assert!(sanitize_search_control_result("inspect", &bad_title).is_err());
        let mut bad_duration = inspect.clone();
        bad_duration["rows"][0]["durationSeconds"] = json!(0);
        assert!(sanitize_search_control_result("inspect", &bad_duration).is_err());
        let mut unicode = inspect.clone();
        unicode["query"] = json!("界".repeat(166));
        unicode["rows"][0]["title"] = json!("🎵".repeat(75));
        assert!(sanitize_search_control_result("inspect", &unicode).is_ok());
        unicode["query"] = json!("界".repeat(167));
        assert!(sanitize_search_control_result("inspect", &unicode).is_err());
        assert_eq!(
            sanitize_search_control_result(
                "set_continue",
                &json!({"status":"complete","revision":revision,"enabled":true,"extra":1})
            )
            .unwrap(),
            json!({"status":"complete","revision":revision,"enabled":true})
        );
        assert_eq!(
            sanitize_search_control_result(
                "play",
                &json!({"status":"dispatched","revision":revision,"sourceId":"youtube","secret":1})
            )
            .unwrap(),
            json!({"status":"dispatched","revision":revision,"sourceId":"youtube"})
        );
        assert!(
            sanitize_search_control_result("play", &json!({"status":"failed","secret":1})).is_err()
        );
        assert_eq!(
            sanitize_search_control_result("play", &json!({"status":"expired","secret":1}))
                .unwrap(),
            json!({"status":"expired"})
        );
        assert_eq!(tool_scope("inspect_search_ui"), Some("read"));
        assert_eq!(tool_scope("set_search_continuation"), Some("control"));
        assert_eq!(tool_scope("play_search_result"), Some("control"));
    }
    #[test]
    fn search_control_inputs_reject_unknown_wrong_typed_and_out_of_range_values() {
        assert_eq!(
            parse_search_control_arguments("inspect", &json!({"offset":1,"limit":200})).unwrap(),
            json!({"action":"inspect","offset":1,"limit":200})
        );
        for invalid in [
            json!({"offset":"1"}),
            json!({"offset":1_000_001}),
            json!({"limit":0}),
            json!({"unknown":true}),
            json!(null),
        ] {
            assert!(
                parse_search_control_arguments("inspect", &invalid).is_err(),
                "accepted {invalid}"
            );
        }
        assert!(parse_search_control_arguments(
            "set_continue",
            &json!({"revision":"123e4567-e89b-12d3-a456-426614174000","enabled":true,"extra":1})
        )
        .is_err());
        assert!(parse_search_control_arguments(
            "set_continue",
            &json!({"revision":"123e4567-e89b-12d3-a456-426614174000","enabled":"true"})
        )
        .is_err());
        assert!(parse_search_control_arguments(
            "play",
            &json!({"revision":"bad","occurrenceToken":"123e4567-e89b-12d3-a456-426614174000"})
        )
        .is_err());
        assert!(parse_search_control_arguments("play", &json!({"revision":"123456789012345678901234567890123456","occurrenceToken":"123e4567-e89b-12d3-a456-426614174000"})).is_err());
        assert!(!search_control_expired(5000, 4999));
        assert!(search_control_expired(5000, 5000));
        let dispatched_before_deadline = json!({"status":"dispatched","dispatchedAtMs":4999});
        assert!(!search_control_completion_expired(
            "play",
            &dispatched_before_deadline,
            5000,
            5001
        ));
        assert!(search_control_completion_expired(
            "play",
            &json!({"status":"dispatched","dispatchedAtMs":5000}),
            5000,
            5000
        ));
        assert!(search_control_completion_expired(
            "play",
            &json!({"status":"dispatched"}),
            5000,
            4999
        ));
    }

    fn queued_publication_fixture() -> (crate::queued_playback::Registry<Value>, Value, Value) {
        let mut registry = crate::queued_playback::Registry::new(4);
        registry
            .begin(
                "owner".into(),
                "rpc".into(),
                vec!["same".into(), "same".into(), "other".into()],
            )
            .unwrap();
        let receipt = json!({"engine":"webview","trackIds":["same","same","other"],"queueEntryIds":["first","second","third"],"queueRevision":"3:1:2:3:4","acceptedGeneration":5});
        let packet = json!({"protocolVersion":1,"queueUpdate":{"kind":"full"},"failureHistoryUpdate":{"kind":"full"},"failureHistoryRevision":"history-7","queueRevision":"3:1:2:3:4","queueEntryIds":["first","second","third"],"queue":[{"id":"same"},{"id":"same"},{"id":"other"}],"currentIndex":1,"trackId":"same","entryId":"second"});
        (registry, receipt, packet)
    }
    #[test]
    fn queued_acceptance_publishes_before_receipt_and_preserves_history_ack() {
        let (mut registry, receipt, packet) = queued_publication_fixture();
        let mut cached = None;
        let result = accept_queued_projection(
            &mut registry,
            "owner",
            "rpc",
            receipt,
            packet.clone(),
            |full| {
                cached = Some(full);
                json!({"protocolVersion":1,"accepted":true,"failureHistoryRevision":"history-7"})
            },
        );
        assert_eq!(cached, Some(packet));
        assert_eq!(result["accepted"], true);
        assert_eq!(result["playbackAck"]["failureHistoryRevision"], "history-7");
        assert!(registry.get("owner").unwrap().receipt.is_some());
    }
    #[test]
    fn queued_invalid_receipt_or_projection_never_publishes() {
        for (field, value) in [
            ("engine", json!("native")),
            ("trackIds", json!(["same", "other", "same"])),
            ("queueEntryIds", json!(["same", "same", "other"])),
            ("acceptedGeneration", json!(-1)),
        ] {
            let (mut registry, mut receipt, packet) = queued_publication_fixture();
            receipt[field] = value;
            let result =
                accept_queued_projection(&mut registry, "owner", "rpc", receipt, packet, |_| {
                    panic!("invalid receipt must not mutate projection")
                });
            assert_eq!(result["accepted"], false);
            assert!(registry.get("owner").unwrap().receipt.is_none());
        }
        let (mut registry, receipt, mut packet) = queued_publication_fixture();
        packet["queueRevision"] = json!("old-queue");
        assert_eq!(
            accept_queued_projection(&mut registry, "owner", "rpc", receipt, packet, |_| panic!(
                "wrong projection must not publish"
            ))["accepted"],
            false
        );
    }
    #[test]
    fn queued_cancelled_superseded_and_wrong_transport_never_publish() {
        for mode in ["cancelled", "superseded", "wrong_transport"] {
            let (mut registry, receipt, packet) = queued_publication_fixture();
            if mode == "cancelled" {
                registry.cancel("owner").unwrap();
            }
            if mode == "superseded" {
                registry
                    .begin("new".into(), "new-rpc".into(), vec!["other".into()])
                    .unwrap();
            }
            let transport = if mode == "wrong_transport" {
                "stale-rpc"
            } else {
                "rpc"
            };
            assert_eq!(
                accept_queued_projection(
                    &mut registry,
                    "owner",
                    transport,
                    receipt,
                    packet,
                    |_| panic!("obsolete owner must not publish")
                )["accepted"],
                false
            );
        }
    }
    #[test]
    fn queued_stale_publication_can_retry_but_duplicate_receipt_cannot_republish() {
        let (mut registry, receipt, packet) = queued_publication_fixture();
        let stale = accept_queued_projection(
            &mut registry,
            "owner",
            "rpc",
            receipt.clone(),
            packet.clone(),
            |_| json!({"protocolVersion":1,"accepted":false,"needFull":false}),
        );
        assert_eq!(stale["accepted"], false);
        assert_eq!(stale["retryable"], true);
        assert!(registry.get("owner").unwrap().receipt.is_none());
        assert_eq!(
            accept_queued_projection(
                &mut registry,
                "owner",
                "rpc",
                receipt.clone(),
                packet.clone(),
                |_| json!({"protocolVersion":1,"accepted":true,"failureHistoryRevision":"history-7"})
            )["accepted"],
            true
        );
        assert_eq!(
            accept_queued_projection(
                &mut registry,
                "owner",
                "rpc",
                receipt.clone(),
                packet.clone(),
                |_| panic!("idempotent retry must not rewrite projection")
            )["accepted"],
            true
        );
        let mut changed = receipt;
        changed["acceptedGeneration"] = json!(6);
        assert_eq!(
            accept_queued_projection(&mut registry, "owner", "rpc", changed, packet, |_| panic!(
                "mutated receipt must not publish"
            ))["accepted"],
            false
        );
    }

    #[test]
    fn queued_acceptance_requires_full_exact_projection_and_occurrence() {
        let receipt = json!({"trackIds":["same","same","other"],"queueEntryIds":["first","second","third"],"queueRevision":"3:1:2:3:4"});
        let full = json!({"protocolVersion":1,"queueUpdate":{"kind":"full"},"failureHistoryUpdate":{"kind":"full"},"queueRevision":"3:1:2:3:4","queueEntryIds":["first","second","third"],"queue":[{"id":"same"},{"id":"same"},{"id":"other"}],"currentIndex":1,"trackId":"same","entryId":"second"});
        assert!(queued_acceptance_snapshot_matches(&receipt, &full));
        for (field, value) in [
            ("queueRevision", json!("stale")),
            ("queueEntryIds", json!(["second", "first", "third"])),
            ("currentIndex", json!(-1)),
            ("trackId", json!("old-source")),
            ("entryId", json!("first")),
            ("queueUpdate", json!({"kind":"unchanged"})),
            ("failureHistoryUpdate", json!({"kind":"unchanged"})),
        ] {
            let mut wrong = full.clone();
            wrong[field] = value;
            assert!(
                !queued_acceptance_snapshot_matches(&receipt, &wrong),
                "{field}"
            );
        }
    }

    #[test]
    fn queued_playback_tools_keep_legacy_play_schema_and_require_control_uuid() {
        let tools = tool_definitions();
        for name in [
            "play_tracks_queued",
            "get_playback_operation",
            "cancel_playback_operation",
        ] {
            assert_eq!(tool_scope(name), Some("control"));
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert_eq!(
                tool["inputSchema"]["properties"]["operationId"]["minLength"],
                36
            );
            assert_eq!(
                tool["inputSchema"]["properties"]["operationId"]["maxLength"],
                36
            );
        }
        let queued = tools
            .iter()
            .find(|tool| tool["name"] == "play_tracks_queued")
            .unwrap();
        assert_eq!(
            queued["inputSchema"]["required"],
            json!(["trackIds", "operationId"])
        );
        assert_eq!(
            queued["inputSchema"]["properties"]["trackIds"]["maxItems"],
            500
        );
        let legacy = tools
            .iter()
            .find(|tool| tool["name"] == "play_tracks")
            .unwrap();
        assert_eq!(legacy["inputSchema"]["required"], json!(["trackIds"]));
        assert!(legacy["inputSchema"]["properties"]
            .get("operationId")
            .is_none());
    }

    #[test]
    fn queue_ui_strict_schema_and_scope_reject_arbitrary_input() {
        assert_eq!(tool_scope("interact_queue_ui"), Some("control"));
        let t = tool_definitions()
            .into_iter()
            .find(|t| t["name"] == "interact_queue_ui")
            .unwrap();
        assert_eq!(t["inputSchema"]["oneOf"].as_array().unwrap().len(), 3);
        for v in [
            json!({"action":"select","index":199,"modifier":"ctrl"}),
            json!({"action":"arrow","index":0,"direction":"down"}),
            json!({"action":"drag","from":1,"to":0}),
        ] {
            assert!(parse_queue_ui_action(&v).is_ok());
        }
        for v in [
            json!({"action":"select","index":200,"modifier":"ctrl"}),
            json!({"action":"select","index":-1,"modifier":"ctrl"}),
            json!({"action":"arrow","index":0,"direction":"Enter"}),
            json!({"action":"drag","from":0,"to":2,"selector":"body"}),
        ] {
            assert!(parse_queue_ui_action(&v).is_err());
        }
    }
    #[test]
    fn queue_ui_requires_exact_isolated_marker_and_paused_distinct_occurrences() {
        assert!(!queue_ui_profile_allowed(std::path::Path::new(
            "/home/user/.local/share/org.burrata.discovery"
        )));
        let mut builder = tempfile::Builder::new();
        builder.prefix("burrata-launch.");
        let root = if cfg!(target_os = "linux") {
            builder.tempdir_in("/tmp")
        } else {
            builder.tempdir()
        }
        .unwrap();
        let data = root.path().join("data/org.burrata.discovery");
        std::fs::create_dir_all(&data).unwrap();
        assert!(!queue_ui_profile_allowed(&data));
        std::fs::write(
            root.path().join(".burrata-isolated-profile"),
            b"burrata isolated memory run\n",
        )
        .unwrap();
        assert_eq!(queue_ui_profile_allowed(&data), cfg!(target_os = "linux"));
        let good = json!({"engine":"native","status":"paused","paused":true,"generation":2,"queueRevision":5,"entryId":"second","currentIndex":1,"queueEntryIds":["first","second"],"queueTotal":2});
        assert!(queue_ui_snapshot_valid(&good));
        for (key, value) in [
            ("engine", json!("webview")),
            ("status", json!("playing")),
            ("entryId", json!("wrong")),
            ("queueEntryIds", json!(["same", "same"])),
            ("queueTotal", json!(201)),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(!queue_ui_snapshot_valid(&bad));
        }
    }
    #[test]
    fn queue_ui_completion_rejects_unbounded_untrusted_or_secret_fields() {
        let good = json!({"protocolVersion":1,"action":"select","dispatched":true,"syntheticEvents":true,"trustedGesture":false,"movePending":false,"selectedIndices":[0,2],"focusedIndex":0,"rowCount":3,"errorCode":null,"acknowledgedMoves":[],"mutationState":"unchanged"});
        assert_eq!(sanitize_queue_ui_result(good.clone()).unwrap(), good);
        for (key, value) in [
            ("selectedIndices", json!([2, 0])),
            ("selectedIndices", json!([0, 3])),
            ("trustedGesture", json!(true)),
            ("errorCode", json!("https://secret")),
            ("rowCount", json!(201)),
            ("dispatched", json!(false)),
            ("movePending", json!(true)),
            ("mutationState", json!("completed")),
            ("acknowledgedMoves", json!([{ "from":0,"to":1 }])),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(sanitize_queue_ui_result(bad).is_err());
        }
        let mut bad = good;
        bad["url"] = json!("https://secret");
        assert!(sanitize_queue_ui_result(bad).is_err());
    }

    #[test]
    fn spotify_profile_inspection_tool_is_read_only_and_accepts_only_bounded_page_args() {
        assert_eq!(tool_scope("inspect_spotify_profile_page"), Some("read"));
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "inspect_spotify_profile_page")
            .unwrap();
        assert_eq!(tool["scope"], "read");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["inputSchema"]["required"], json!(["ownerId"]));
        let properties = tool["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 5);
        assert_eq!(properties["mode"]["enum"], json!(["paged", "summary"]));
        assert_eq!(properties["playlistLimit"]["minimum"], 1);
        assert_eq!(properties["playlistLimit"]["maximum"], 200);
        assert_eq!(properties["offset"]["minimum"], 0);
        assert_eq!(properties["offset"]["maximum"], 10_000_000);
        assert_eq!(properties["offset"]["default"], 0);
        assert_eq!(properties["limit"]["minimum"], 1);
        assert_eq!(properties["limit"]["maximum"], 30);
        assert_eq!(properties["limit"]["default"], 30);
        assert_eq!(properties["ownerId"]["maxLength"], 2048);
    }

    #[test]
    fn youtube_inspection_tool_is_read_only_and_accepts_only_playlist_and_cursor() {
        assert_eq!(tool_scope("inspect_youtube_playlist_page"), Some("read"));
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "inspect_youtube_playlist_page")
            .unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["inputSchema"]["required"], json!(["playlistId"]));
        let properties = tool["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 2);
        assert_eq!(properties["playlistId"]["maxLength"], 80);
        assert_eq!(properties["cursor"]["maxLength"], 2048);
    }

    fn playing_snapshot() -> Value {
        json!({"status":"playing","trackId":"track-a","sessionId":"session-a","sourceGeneration":9,"seekSerial":2,"chapterId":"chapter-a"})
    }

    #[test]
    fn verification_playlist_tool_is_control_scoped_and_provider_only() {
        assert_eq!(tool_scope("create_verification_playlist"), Some("control"));
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "create_verification_playlist")
            .unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["inputSchema"]["required"], json!(["provider"]));
        assert_eq!(
            tool["inputSchema"]["properties"].as_object().unwrap().len(),
            1
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["provider"]["enum"],
            json!(["spotify", "youtube"])
        );
    }

    #[test]
    fn identify_result_requires_playing_unchanged_webview_identity_and_matching_result_track() {
        let expected = playing_snapshot();
        let result = json!({"trackId":"track-a","title":"Song","artist":"Artist","note":"Matched"});
        assert!(validate_playing_identification_result(
            &expected,
            &expected,
            false,
            result.clone()
        )
        .is_ok());

        let mut paused = expected.clone();
        paused["status"] = json!("paused");
        assert!(validate_playing_identification_snapshot(&paused, false).is_err());
        for (key, value) in [
            ("seekSerial", json!(3)),
            ("sessionId", json!("session-b")),
            ("sourceGeneration", json!(10)),
            ("trackId", json!("track-b")),
            ("chapterId", json!("chapter-b")),
        ] {
            let mut changed = expected.clone();
            changed[key] = value;
            assert!(
                validate_playing_identification_result(&expected, &changed, false, result.clone())
                    .is_err(),
                "accepted changed {key}"
            );
        }
        assert!(
            validate_playing_identification_result(&expected, &expected, true, result.clone())
                .is_err()
        );
        let wrong_track = json!({"trackId":"track-b","title":"Song","artist":"Artist"});
        assert!(
            validate_playing_identification_result(&expected, &expected, false, wrong_track)
                .is_err()
        );
    }

    #[test]
    fn identify_completion_preserves_only_safe_recognized_artwork() {
        let result = |url: &str| {
            sanitize_identification_result(json!({
                "trackId":"radio:nova", "title":"Song", "artist":"Artist", "artworkUrl":url
            }))
            .unwrap()
        };
        let cover = "https://is1-ssl.mzstatic.com/cover.jpg";
        assert_eq!(result(cover)["artworkUrl"], cover);
        for invalid in [
            "file:///tmp/private.jpg",
            "data:image/png;base64,AAA",
            "javascript:alert(1)",
        ] {
            assert!(result(invalid)["artworkUrl"].is_null());
        }
        assert!(sanitize_identification_result(
            json!({"trackId":"x","artworkUrl":"x".repeat(4097)})
        )
        .is_err());
    }

    #[test]
    fn identify_completion_sanitizes_known_fields_and_rejects_oversized_or_unknown_shapes() {
        let clean = sanitize_identification_result(json!({
            "trackId":"track-1", "title":"Song", "artist":"Artist", "note":"matched",
            "sourceUrl":"https://should-not-escape.example"
        }))
        .unwrap();
        assert_eq!(clean["trackId"], "track-1");
        assert!(clean.get("sourceUrl").is_none());
        assert!(
            sanitize_identification_result(json!({"trackId":"x","title":"t".repeat(501)})).is_err()
        );
        assert!(sanitize_identification_result(
            json!({"trackId":"x","nowPlaying":{"title":"T","artist":"A"}})
        )
        .unwrap()
        .get("nowPlaying")
        .is_none());
        assert!(sanitize_identification_result(json!({"title":"orphan"})).is_err());
    }

    #[test]
    fn identify_completion_drops_unregistered_request_ids() {
        // A timed-out/cancelled nonce has already been removed; a late response cannot
        // reach a newer request registered under another UUID.
        let pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>> =
            Mutex::new(HashMap::new());
        let (tx, mut newer) = oneshot::channel();
        pending.lock().unwrap().insert("newer".into(), tx);
        assert!(!complete_identify_pending(
            &pending,
            "expired",
            json!({"trackId":"stale"}),
            None
        ));
        assert!(pending.lock().unwrap().contains_key("newer"));
        assert!(newer.try_recv().is_err());
        assert!(complete_identify_pending(
            &pending,
            "newer",
            json!({"trackId":"current"}),
            None
        ));
        assert_eq!(newer.try_recv().unwrap().unwrap()["trackId"], "current");
    }

    #[test]
    fn pending_playback_guard_cleans_registry_when_request_is_dropped() {
        let pending = Mutex::new(HashMap::new());
        let (tx, _rx) = oneshot::channel();
        pending.lock().unwrap().insert("request".to_string(), tx);
        {
            let _guard = PendingPlaybackGuard {
                pending: &pending,
                id: "request".to_string(),
            };
            assert_eq!(pending.lock().unwrap().len(), 1);
        }
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn discovery_cursor_argument_accepts_buffered_cursors_and_rejects_over_limit() {
        use base64::Engine;

        let buffered = (0..20)
            .map(|index| {
                json!({
                    "id":format!("video-{index}"),
                    "kind":"video",
                    "sourceId":"youtube",
                    "title":format!("Buffered result {index}"),
                    "subtitle":null,
                    "url":null,
                    "track":null,
                    "data":{}
                })
            })
            .collect::<Vec<_>>();
        let cursor_json = json!({
            "sourceId":"youtube",
            "query":"query",
            "kinds":["video"],
            "providerCursor":"provider-page-2",
            "buffered":buffered,
            "complete":true,
            "progress":null,
            "warnings":[]
        });
        let cursor = format!(
            "dsb1:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&cursor_json).unwrap())
        );
        assert!(cursor.len() > 100);
        let accepted = discovery_cursor_arg(&json!({"cursor":cursor}))
            .unwrap()
            .unwrap();
        assert!(accepted.len() > 100);
        let too_large = "x".repeat(crate::discovery::MAX_DISCOVERY_CURSOR_LENGTH + 1);
        assert!(discovery_cursor_arg(&json!({"cursor":too_large})).is_err());
        assert!(discovery_cursor_arg(&json!({"cursor":42})).is_err());
    }

    #[test]
    fn pending_search_ui_guard_cleans_registry_and_active_slot_on_drop() {
        let pending = Mutex::new(HashMap::new());
        let active = Mutex::new(Some("request".to_owned()));
        let (tx, _rx) = oneshot::channel();
        pending.lock().unwrap().insert(
            "request".to_owned(),
            PendingSearchUi {
                tx,
                source_id: "all".to_owned(),
                kind: "tracks".to_owned(),
            },
        );
        {
            let _guard = PendingSearchUiGuard {
                app: None,
                pending: &pending,
                active: &active,
                id: "request".to_owned(),
                resolved: false,
            };
            assert_eq!(pending.lock().unwrap().len(), 1);
        }
        assert!(pending.lock().unwrap().is_empty());
        assert!(active.lock().unwrap().is_none());
    }

    #[test]
    fn search_ui_request_accepts_only_bounded_query_source_and_kind() {
        let request = parse_search_ui_request(&json!({
            "query":"  Radiohead  ",
            "sourceId":"all",
            "kind":"tracks"
        }))
        .unwrap();
        assert_eq!(request.query, "Radiohead");
        assert_eq!(request.source_id, "all");
        assert_eq!(request.kind, "tracks");
        assert!(
            parse_search_ui_request(&json!({"query":" ","sourceId":"all","kind":"all"})).is_err()
        );
        assert!(
            parse_search_ui_request(&json!({"query":"x","sourceId":"all","kind":"future"}))
                .is_err()
        );
        assert!(parse_search_ui_request(
            &json!({"query":"x","sourceId":"all","kind":"all","debug":true})
        )
        .is_err());
        assert!(parse_search_ui_request(
            &json!({"query":"x","sourceId":"https://example.test","kind":"all"})
        )
        .is_err());
        assert!(parse_search_ui_request(
            &json!({"query":"x".repeat(501),"sourceId":"all","kind":"all"})
        )
        .is_err());
    }

    #[test]
    fn search_ui_completion_sanitizes_optional_source_coverage() {
        let coverage = json!({
            "youtube": {"hasMore": true, "incomplete": true},
            "musicbrainz": {"hasMore": false, "incomplete": true}
        });
        let result = sanitize_search_ui_completion_with_coverage(
            "all",
            "tracks",
            "complete",
            "all",
            "tracks",
            &json!({"youtube":3,"musicbrainz":2}),
            None,
            Some(&coverage),
        )
        .unwrap();
        assert_eq!(result["status"], "complete");
        assert_eq!(result["sourceCoverage"]["youtube"]["hasMore"], true);
        assert_eq!(result["sourceCoverage"]["musicbrainz"]["incomplete"], true);
        assert!(result.get("query").is_none());
        assert!(result.get("results").is_none());

        for invalid in [
            json!({"unknown": {"hasMore": false, "incomplete": false}}),
            json!({"spotify": {"hasMore": "yes", "incomplete": false}}),
            json!({"spotify": {"hasMore": false, "incomplete": false, "extra": 1}}),
        ] {
            assert!(sanitize_search_ui_completion_with_coverage(
                "spotify",
                "tracks",
                "complete",
                "spotify",
                "tracks",
                &json!({"spotify":1}),
                None,
                Some(&invalid),
            )
            .is_err());
        }
        assert!(sanitize_search_ui_completion_with_coverage(
            "spotify",
            "tracks",
            "complete",
            "spotify",
            "tracks",
            &json!({"spotify":1}),
            None,
            Some(&json!({"youtube": {"hasMore": false, "incomplete": false}})),
        )
        .is_err());
    }

    #[test]
    fn search_ui_completion_returns_only_allowlisted_counts_and_status() {
        assert_eq!(
            sanitize_search_ui_completion(
                "spotify",
                "tracks",
                "complete",
                "spotify",
                "tracks",
                &json!({"spotify":7}),
                None,
            )
            .unwrap(),
            json!({"status":"complete","sourceId":"spotify","kind":"tracks","resultCounts":{"spotify":7},"errorCode":null})
        );
        for counts in [
            json!({"spotify":-1}),
            json!({"spotify":1.5}),
            json!({"unknown":2}),
            json!({"spotify":10001}),
        ] {
            assert!(sanitize_search_ui_completion(
                "spotify", "tracks", "complete", "spotify", "tracks", &counts, None
            )
            .is_err());
        }
        assert!(sanitize_search_ui_completion(
            "spotify",
            "tracks",
            "complete",
            "spotify",
            "tracks",
            &json!({"spotify":7,"youtube":0}),
            None
        )
        .is_err());
        assert!(sanitize_search_ui_completion(
            "spotify",
            "tracks",
            "complete",
            "youtube",
            "tracks",
            &json!({}),
            None
        )
        .is_err());
        assert!(sanitize_search_ui_completion(
            "spotify",
            "tracks",
            "failed",
            "spotify",
            "tracks",
            &json!({}),
            None
        )
        .is_err());
        assert!(sanitize_search_ui_completion(
            "spotify",
            "tracks",
            "failed",
            "spotify",
            "tracks",
            &json!({}),
            Some("raw exception URL")
        )
        .is_err());
    }

    #[test]
    fn search_ui_tool_is_control_scoped_and_strictly_typed() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "search_ui")
            .expect("search UI tool");
        assert_eq!(tool_scope("search_ui"), Some("control"));
        assert_eq!(
            tool["inputSchema"]["required"],
            json!(["query", "sourceId", "kind"])
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            tool["inputSchema"]["properties"]["sourceId"]["enum"][0],
            "all"
        );
    }

    #[test]
    fn logical_window_readback_converts_client_area_pixels_at_display_scale() {
        let actual = logical_size_value(tauri::PhysicalSize::new(2871, 720), 1.5);
        assert_eq!(actual, json!({"width":1914.0,"height":480.0}));
    }

    #[test]
    fn resize_window_arguments_accept_bounded_integer_sizes_and_restore_state() {
        assert_eq!(
            parse_resize_window_request(
                &json!({"width":320,"height":240,"restoreMaximized":false})
            )
            .unwrap(),
            ResizeWindowRequest {
                width: 320,
                height: 240,
                restore_maximized: Some(false)
            }
        );
        assert_eq!(
            parse_resize_window_request(&json!({"width":7680,"height":4320})).unwrap(),
            ResizeWindowRequest {
                width: 7680,
                height: 4320,
                restore_maximized: None
            }
        );
    }

    #[test]
    fn resize_window_arguments_reject_fractional_or_out_of_range_sizes() {
        for arguments in [
            json!({"width":319,"height":800}),
            json!({"width":7681,"height":800}),
            json!({"width":500,"height":239}),
            json!({"width":500,"height":4321}),
            json!({"width":500.5,"height":800}),
        ] {
            assert!(
                parse_resize_window_request(&arguments).is_err(),
                "accepted {arguments}"
            );
        }
    }

    #[test]
    fn optional_u32_arguments_accept_the_u32_maximum_and_absence() {
        assert_eq!(
            optional_u32_arg(&json!({"duration":u32::MAX}), "duration").unwrap(),
            Some(u32::MAX)
        );
        assert_eq!(optional_u32_arg(&json!({}), "duration").unwrap(), None);
    }

    #[test]
    fn optional_u32_arguments_reject_negative_fractional_and_overflow_values() {
        for arguments in [
            json!({"duration":-1}),
            json!({"duration":1.5}),
            json!({"duration":u64::from(u32::MAX)+1}),
        ] {
            assert!(
                optional_u32_arg(&arguments, "duration").is_err(),
                "accepted {arguments}"
            );
        }
    }

    #[test]
    fn list_tracks_page_tool_is_read_scoped_with_bounded_paging_and_filters() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "list_tracks_page")
            .unwrap();
        assert_eq!(tool_scope("list_tracks_page"), Some("read"));
        assert_eq!(tool["inputSchema"]["required"], json!(["view"]));
        assert_eq!(tool["inputSchema"]["properties"]["limit"]["maximum"], 200);
        assert_eq!(
            tool["inputSchema"]["properties"]["offset"]["maximum"],
            1_000_000
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }

    #[test]
    fn resize_window_tool_is_control_scoped_with_a_bounded_schema() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "resize_app_window")
            .expect("window resize tool");
        assert_eq!(tool_scope("resize_app_window"), Some("control"));
        assert_eq!(tool["scope"], "control");
        assert_eq!(tool["inputSchema"]["required"], json!(["width", "height"]));
        assert_eq!(tool["inputSchema"]["properties"]["width"]["minimum"], 320);
        assert_eq!(tool["inputSchema"]["properties"]["width"]["maximum"], 7680);
        assert!(
            parse_resize_window_request(&json!({"width":500,"height":800,"script":"x"})).is_err()
        );
        assert!(parse_resize_window_request(
            &json!({"width":500,"height":800,"restoreMaximized":"yes"})
        )
        .is_err());
    }

    #[test]
    fn ui_navigation_targets_are_finite_and_exact() {
        for view in [
            "library",
            "favorites",
            "history",
            "downloads",
            "settings",
            "radio",
        ] {
            assert_eq!(
                parse_ui_navigation_request(&json!({"view":view})).unwrap(),
                UiNavigationTarget::View(view.into())
            );
        }
        assert_eq!(
            parse_ui_navigation_request(&json!({"spotifyArtistId":"0123456789ABCDEFGHIJKL"}))
                .unwrap(),
            UiNavigationTarget::SpotifyArtist("0123456789ABCDEFGHIJKL".into())
        );
        for value in [
            json!({"view":"search"}),
            json!({"view":"library","extra":true}),
            json!({"url":"https://open.spotify.com/artist/0123456789ABCDEFGHIJKL"}),
            json!({"spotifyArtistId":"short"}),
            json!({"spotifyArtistId":"0123456789ABCDEFGHIJK "}),
        ] {
            assert!(
                parse_ui_navigation_request(&value).is_err(),
                "accepted {value}"
            );
        }
        assert!(sanitize_ui_navigation_completion(
            &UiNavigationTarget::View("library".into()),
            "library",
            "complete"
        )
        .is_ok());
        assert!(sanitize_ui_navigation_completion(
            &UiNavigationTarget::SpotifyArtist("0123456789ABCDEFGHIJKL".into()),
            "view",
            "complete"
        )
        .is_err());
        assert_eq!(
            sanitize_ui_navigation_completion(
                &UiNavigationTarget::View("library".into()),
                "library",
                "failed"
            )
            .unwrap()["status"],
            "failed"
        );
    }

    #[test]
    fn ui_navigation_settings_categories_are_finite_and_settings_only() {
        for category in UI_SETTINGS_CATEGORIES {
            let target = parse_ui_navigation_request(
                &json!({"view":"settings","settingsCategory":category}),
            )
            .unwrap();
            assert_eq!(
                target,
                UiNavigationTarget::SettingsCategory((*category).into())
            );
            assert_eq!(
                target.payload(),
                json!({"kind":"view","view":"settings","settingsCategory":category})
            );
            assert_eq!(
                sanitize_ui_navigation_completion(&target, "settings", "complete").unwrap(),
                json!({"status":"complete","targetKind":"view","destination":"settings","settingsCategory":category})
            );
            assert!(sanitize_ui_navigation_completion(&target, category, "complete").is_err());
        }
        for arguments in [
            json!({"view":"settings","settingsCategory":"arbitrary"}),
            json!({"view":"settings","settingsCategory":null}),
            json!({"view":"settings","settingsCategory":7}),
            json!({"view":"library","settingsCategory":"playback"}),
            json!({"settingsCategory":"playback"}),
            json!({"spotifyArtistId":"0123456789ABCDEFGHIJKL","settingsCategory":"playback"}),
            json!({"view":"settings","settingsCategory":"playback","historyDirection":"back"}),
            json!({"view":"settings","settingsCategory":"playback","script":"x"}),
        ] {
            assert!(
                parse_ui_navigation_request(&arguments).is_err(),
                "accepted {arguments}"
            );
        }
        let control = parse_ui_navigation_request(&json!({"view":"settings","settingsCategory":"playback","settingsControl":"playbackStartupTimeout"})).unwrap();
        assert_eq!(
            control,
            UiNavigationTarget::SettingsControl("playbackStartupTimeout".into())
        );
        assert_eq!(
            control.payload(),
            json!({"kind":"view","view":"settings","settingsCategory":"playback","settingsControl":"playbackStartupTimeout"})
        );
        assert_eq!(
            sanitize_ui_navigation_completion(&control, "settings", "complete").unwrap(),
            json!({"status":"complete","targetKind":"view","destination":"settings","settingsCategory":"playback","settingsControl":"playbackStartupTimeout"})
        );
        for arguments in [
            json!({"view":"settings","settingsControl":"playbackStartupTimeout"}),
            json!({"view":"settings","settingsCategory":"general","settingsControl":"playbackStartupTimeout"}),
            json!({"view":"library","settingsCategory":"playback","settingsControl":"playbackStartupTimeout"}),
            json!({"view":"settings","settingsCategory":"playback","settingsControl":"arbitrary"}),
            json!({"view":"settings","settingsCategory":"playback","settingsControl":"playbackStartupTimeout","script":"x"}),
        ] {
            assert!(
                parse_ui_navigation_request(&arguments).is_err(),
                "accepted {arguments}"
            );
        }
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "navigate_ui")
            .unwrap();
        assert_eq!(
            tool["inputSchema"]["properties"]["settingsCategory"]["enum"],
            json!(UI_SETTINGS_CATEGORIES)
        );
        assert_eq!(
            tool["inputSchema"]["then"],
            json!({"required":["view"],"properties":{"view":{"const":"settings"}}})
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["settingsControl"]["enum"],
            json!(["playbackStartupTimeout"])
        );
        assert_eq!(
            tool["inputSchema"]["allOf"][0]["then"],
            json!({"required":["view","settingsCategory"],"properties":{"view":{"const":"settings"},"settingsCategory":{"const":"playback"}}})
        );
    }

    #[test]
    fn spotify_profile_ui_navigation_is_canonical_and_completion_is_exact() {
        for id in ["dkkelsh1", "Andrew.Kalš"] {
            let target = parse_ui_navigation_request(&json!({"spotifyProfileId":id})).unwrap();
            assert_eq!(target, UiNavigationTarget::SpotifyProfile(id.into()));
            assert_eq!(
                target.payload(),
                json!({"kind":"spotify_profile","spotifyProfileId":id})
            );
            assert_eq!(
                sanitize_ui_navigation_completion(&target, id, "complete").unwrap(),
                json!({"status":"complete","targetKind":"spotify_profile","destination":id})
            );
            assert!(sanitize_ui_navigation_completion(&target, "different", "complete").is_err());
            assert!(sanitize_ui_navigation_completion(&target, id, "arbitrary").is_err());
        }
        for args in [
            json!({"spotifyProfileId":""}),
            json!({"spotifyProfileId":"x".repeat(257)}),
            json!({"spotifyProfileId":"https://open.spotify.com/user/dkkelsh1"}),
            json!({"spotifyProfileId":"a/b"}),
            json!({"spotifyProfileId":"a\\b"}),
            json!({"spotifyProfileId":"a?token=x"}),
            json!({"spotifyProfileId":"a#x"}),
            json!({"spotifyProfileId":"a b"}),
            json!({"spotifyProfileId":"a\n"}),
            json!({"spotifyProfileId":"dkkelsh1","view":"library"}),
            json!({"spotifyProfileId":"dkkelsh1","spotifyArtistId":"0123456789ABCDEFGHIJKL"}),
            json!({"spotifyProfileId":"dkkelsh1","script":"x"}),
        ] {
            assert!(parse_ui_navigation_request(&args).is_err());
        }
    }

    #[test]
    fn playback_catalog_identity_tool_is_saved_track_only_and_read_scoped() {
        assert_eq!(tool_scope("prepare_playback_catalog_identity"), Some("read"));
        let tool = tool_definitions().into_iter()
            .find(|tool| tool["name"] == "prepare_playback_catalog_identity").unwrap();
        assert_eq!(tool["inputSchema"]["required"], json!(["trackId"]));
        assert_eq!(tool["inputSchema"]["properties"].as_object().unwrap().len(), 1);
        assert_eq!(tool["inputSchema"]["properties"]["trackId"]["maxLength"], 256);
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(parse_catalog_identity_request(&json!({"trackId":"radio_song:saved"})).unwrap(), "radio_song:saved");
        for input in [json!({}), json!({"trackId":""}), json!({"trackId":"x".repeat(257)}),
            json!({"trackId":42}), json!({"trackId":"saved","track":{"durationSeconds":999}}),
            json!({"trackId":"saved","url":"https://example.org"})] {
            assert!(parse_catalog_identity_request(&input).is_err());
        }
    }

    #[test]
    fn playback_catalog_identity_summary_is_safe_and_does_not_mutate_track() {
        let track = Track { duration_seconds:Some(228.0), isrc:Some("PRIVATE_CODE".into()),
            source_url:"https://example.org/signed?token=PRIVATE_TOKEN".into(),
            provenance:json!({"proof":"PRIVATE_PROOF","account":"PRIVATE_ACCOUNT"}), ..Track::default() };
        let before = serde_json::to_value(&track).unwrap();
        assert_eq!(catalog_identity_summary(Some(&track), 17), json!({"enriched":true,
            "durationSeconds":228.0,"isrcKnown":true,"provider":"spotify","elapsedMs":17}));
        assert_eq!(serde_json::to_value(&track).unwrap(), before);
        assert_eq!(catalog_identity_summary(None, 20), json!({"enriched":false,
            "durationSeconds":null,"isrcKnown":false,"provider":"spotify","elapsedMs":20}));
        let invalid = Track { duration_seconds:Some(f64::NAN), ..track };
        assert_eq!(catalog_identity_summary(Some(&invalid), 0)["enriched"], false);
    }

    #[test]
    fn catalog_artist_navigation_validates_identity_section_and_completion() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        for (provider, id) in [("musicbrainz", id), ("discogs", "12345"), ("discogs", "18446744073709551615")] {
            let artist = ArtistRef { provider: provider.into(), id: id.into() };
            for section in ["overview", "releases", "tracks"] {
                let target = parse_ui_navigation_request(&json!({"catalogArtist":artist,"catalogArtistSection":section})).unwrap();
                assert_eq!(target, UiNavigationTarget::CatalogArtist(artist.clone(), section.into()));
                assert_eq!(target.payload(), json!({"kind":"catalog_artist","catalogArtist":artist,"catalogArtistSection":section}));
                let result = sanitize_ui_navigation_completion(&target, id, "complete").unwrap();
                assert_eq!(result["completionScope"], "route_transition");
                assert_eq!(result["catalogArtist"], json!(artist));
                assert_eq!(result["catalogArtistSection"], section);
                assert!(sanitize_ui_navigation_completion(&target, "other", "complete").is_err());
            }
        }
        assert_eq!(parse_ui_navigation_request(&json!({"catalogArtist":{"provider":"musicbrainz","id":id}})).unwrap(),
            UiNavigationTarget::CatalogArtist(ArtistRef {provider:"musicbrainz".into(),id:id.into()}, "overview".into()));
        for arguments in [
            json!({"catalogArtistSection":"tracks"}),
            json!({"catalogArtist":null}),
            json!({"catalogArtist":{"provider":"spotify","id":id}}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":"not-a-uuid"}}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":format!("https://musicbrainz.org/artist/{id}")}}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":id,"url":"https://example.org"}}),
            json!({"catalogArtist":{"provider":"discogs","id":"0"}}),
            json!({"catalogArtist":{"provider":"discogs","id":"01"}}),
            json!({"catalogArtist":{"provider":"discogs","id":"-1"}}),
            json!({"catalogArtist":{"provider":"discogs","id":"18446744073709551616"}}),
            json!({"catalogArtist":{"provider":"discogs","id":123}}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":id},"catalogArtistSection":"local"}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":id},"catalogArtistSection":null}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":id},"view":"library"}),
            json!({"catalogArtist":{"provider":"musicbrainz","id":id},"script":"arbitrary"}),
        ] {
            assert!(parse_ui_navigation_request(&arguments).is_err(), "accepted {arguments}");
        }
    }

    #[test]
    fn navigate_ui_tool_is_control_scoped_and_bounded() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "navigate_ui")
            .unwrap();
        assert_eq!(tool_scope("navigate_ui"), Some("control"));
        assert!(tool["inputSchema"]["properties"]["view"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("radio")));
        assert_eq!(
            tool["inputSchema"]["properties"].as_object().unwrap().len(),
            9
        );
        assert_eq!(
            tool["inputSchema"]["oneOf"],
            json!([{"required":["view"]},{"required":["spotifyArtistId"]},{"required":["spotifyProfileId"]},{"required":["spotifyPlaylistId"]},{"required":["historyDirection"]},{"required":["catalogArtist"]}])
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["spotifyProfileId"]["maxLength"],
            256
        );
        assert_eq!(tool["scope"], "control");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            tool["inputSchema"]["properties"]["spotifyArtistId"]["pattern"],
            "^[A-Za-z0-9]{22}$"
        );
    }

    #[test]
    fn player_panel_request_is_bounded_and_completion_is_correlated() {
        assert_eq!(
            parse_player_panel_request(&json!({"panel":"queue"})).unwrap(),
            PlayerPanel::Queue
        );
        assert_eq!(
            parse_player_panel_request(&json!({"panel":"info"})).unwrap(),
            PlayerPanel::Info
        );
        assert_eq!(
            parse_player_panel_request(&json!({"panel":"closed"})).unwrap(),
            PlayerPanel::Closed
        );
        for invalid in [
            json!({"panel":"downloads"}),
            json!({"panel":"queue","selector":"*"}),
            json!({"panel":2}),
        ] {
            assert!(parse_player_panel_request(&invalid).is_err());
        }
        assert_eq!(
            sanitize_player_panel_completion(PlayerPanel::Queue, "queue", "complete").unwrap(),
            json!({"panel":"queue","status":"complete"})
        );
        assert!(sanitize_player_panel_completion(PlayerPanel::Queue, "info", "complete").is_err());
        assert!(sanitize_player_panel_completion(PlayerPanel::Queue, "queue", "failed").is_err());
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "set_player_panel")
            .unwrap();
        assert_eq!(tool_scope("set_player_panel"), Some("control"));
        assert_eq!(
            tool["inputSchema"]["properties"]["panel"]["enum"],
            json!(["queue", "info", "closed"])
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }

    #[test]
    fn public_spotify_duration_tools_have_strict_scopes_and_no_argument_schemas() {
        for (name, scope) in [
            ("repair_spotify_durations_public", "control"),
            ("cancel_public_spotify_duration_repair", "control"),
            ("public_spotify_duration_repair_running", "read"),
        ] {
            let tool = tool_definitions()
                .into_iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            assert_eq!(tool_scope(name), Some(scope));
            assert_eq!(tool["scope"], scope);
            assert_eq!(tool["inputSchema"]["required"], json!([]));
            assert_eq!(tool["inputSchema"]["properties"], json!({}));
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert!(require_no_arguments(&json!({}), name).is_ok());
            assert!(require_no_arguments(&json!({"unexpected":true}), name).is_err());
            assert!(require_no_arguments(&json!(null), name).is_err());
        }
    }

    #[test]
    fn webview_media_inspection_is_read_only_no_argument_and_sanitizes_element_fields() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "inspect_webview_media")
            .unwrap();
        assert_eq!(tool_scope("inspect_webview_media"), Some("read"));
        assert_eq!(tool["scope"], "read");
        assert_eq!(tool["inputSchema"]["properties"], json!({}));
        assert_eq!(tool["inputSchema"]["required"], json!([]));
        assert!(require_no_arguments(&json!({}), "inspect_webview_media").is_ok());
        assert!(require_no_arguments(&json!({"command":"play"}), "inspect_webview_media").is_err());
        let sample = sanitize_webview_media_inspection(json!({
            "engine":"webview","applicable":true,"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,
            "currentIndex":0,"queueTotal":1,"entryId":null,"queueTrackId":"t","previewKind":"audio","currentSrc":"https://private.invalid/?token=secret",
                "media":{"present":true,"currentTime":10.25,"duration":131.0,"paused":false,"readyState":4,"networkState":2,"errorCode":null,"sourcePresent":true,"sourceStable":true}
        })).unwrap();
        assert_eq!(sample["media"]["currentTime"], 10.25);
        let mut with_ranges = sample.clone();
        with_ranges["media"]["seeking"] = json!(true);
        with_ranges["media"]["buffered"] = json!([[0., 42.112]]);
        with_ranges["media"]["seekable"] = json!([[0., 257.]]);
        let safe = sanitize_webview_media_inspection(with_ranges.clone()).unwrap();
        assert_eq!(safe["media"]["seeking"], true);
        assert_eq!(safe["media"]["buffered"], json!([[0., 42.112]]));
        with_ranges["media"]["buffered"] = json!([[2., 1.]]);
        assert!(sanitize_webview_media_inspection(with_ranges.clone()).is_err());
        with_ranges["media"]["buffered"] = json!(vec![vec![0., 1.]; 9]);
        assert!(sanitize_webview_media_inspection(with_ranges).is_err());
        assert!(sample.get("currentSrc").is_none());
        assert!(!sample.to_string().contains("private.invalid"));
        assert!(!sample.to_string().contains("secret"));
        let before_metadata = sanitize_webview_media_inspection(json!({
            "engine":"webview","applicable":true,"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,
            "currentIndex":0,"queueTotal":1,"entryId":null,"queueTrackId":"t","previewKind":"audio",
            "media":{"present":true,"currentTime":0.0,"duration":null,"paused":false,"readyState":0,"networkState":2,"errorCode":null,"sourcePresent":true,"sourceStable":true}
        })).unwrap();
        assert_eq!(before_metadata["media"]["duration"], Value::Null);
        let live_audio = sanitize_webview_media_inspection(json!({
            "engine":"webview","applicable":true,"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,
            "currentIndex":0,"queueTotal":1,"entryId":null,"queueTrackId":"t","previewKind":"audio",
            "media":{"present":true,"currentTime":4.0,"duration":null,"paused":false,"readyState":4,"networkState":2,"errorCode":null,"sourcePresent":true,"sourceStable":true}
        })).unwrap();
        assert_eq!(live_audio["media"]["duration"], Value::Null);
        let absent = sanitize_webview_media_inspection(json!({
            "engine":"webview","applicable":true,"trackId":null,"sessionId":"s","sourceGeneration":null,"seekSerial":0,
            "currentIndex":-1,"queueTotal":0,"entryId":null,"queueTrackId":null,"previewKind":null,
                "media":{"present":false,"currentTime":null,"duration":null,"paused":null,"readyState":null,"networkState":null,"errorCode":null,"sourcePresent":false,"sourceStable":false}
        })).unwrap();
        assert_eq!(absent["media"]["present"], false);
        assert!(sanitize_webview_media_inspection(json!({
            "engine":"webview","applicable":true,"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,
            "currentIndex":0,"queueTotal":1,"entryId":null,"queueTrackId":"t","previewKind":"audio",
                "media":{"present":true,"currentTime":1e99,"duration":10,"paused":false,"readyState":4,"networkState":2,"errorCode":null,"sourcePresent":true,"sourceStable":true}
        })).is_err());
        assert!(sanitize_webview_media_inspection(json!({
            "engine":"webview","applicable":true,"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,
            "currentIndex":0,"queueTotal":1,"entryId":null,"queueTrackId":"t","previewKind":"audio",
            "media":{"present":true,"currentTime":4.0,"duration":1e99,"paused":false,"readyState":4,"networkState":2,"errorCode":null,"sourcePresent":true,"sourceStable":true}
        })).is_err());
        let before = json!({"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,"currentIndex":0,"queueTotal":1});
        let sample = before.clone();
        assert!(webview_media_identity_matches(
            &before,
            &before,
            &sample,
            &Value::Null,
            &Value::Null
        ));
        let changed = json!({"trackId":"t","sessionId":"s","sourceGeneration":8,"seekSerial":2,"currentIndex":0,"queueTotal":1});
        assert!(!webview_media_identity_matches(
            &before,
            &changed,
            &sample,
            &Value::Null,
            &Value::Null
        ));
        assert!(!webview_media_identity_matches(
            &before,
            &before,
            &sample,
            &json!("old-entry"),
            &Value::Null
        ));
    }

    #[test]
    fn webview_media_event_diagnostics_are_bounded_owned_and_redacted() {
        let base = json!({
            "engine":"webview","applicable":true,"trackId":"t","sessionId":"s","sourceGeneration":7,"seekSerial":2,
            "currentIndex":0,"queueTotal":1,"entryId":"e","queueTrackId":"t","previewKind":"audio",
            "media":{"present":true,"currentTime":10.0,"duration":131.0,"paused":false,"readyState":2,"networkState":2,"errorCode":null,"sourcePresent":true,"sourceStable":true},
            "diagnostics":{"owner":{"sessionId":"s","sourceGeneration":7,"entryId":"e","seekSerial":2},"sourceFormat":{"codec":"mp4a.40.2","mimeType":"audio/mp4","formatId":"140","hasVideo":false,"expectedContentLength":123456,"streamKind":"direct","url":"https://private.invalid/?token=secret"},"events":[{"atMs":20.5,"type":"waiting","readyState":2,"networkState":2,"paused":false,"currentTime":10.0,"buffered":[{"start":9.0,"end":12.0}],"url":"https://private.invalid/?token=secret","title":"private title","path":"/private/file"}]}
        });
        let safe = sanitize_webview_media_inspection(base.clone()).unwrap();
        assert_eq!(safe["diagnostics"]["events"][0]["type"], "waiting");
        assert_eq!(safe["diagnostics"]["events"][0]["buffered"][0]["end"], 12.0);
        assert_eq!(
            safe["diagnostics"]["sourceFormat"],
            json!({"codec":"mp4a.40.2","mimeType":"audio/mp4","formatId":"140","hasVideo":false,"expectedContentLength":123456,"streamKind":"direct"})
        );
        let mut null_format = base.clone();
        null_format["diagnostics"]["sourceFormat"] = json!({"codec":null,"mimeType":null,"formatId":null,"hasVideo":null,"expectedContentLength":null,"streamKind":"unavailable"});
        let null_safe = sanitize_webview_media_inspection(null_format).unwrap();
        assert_eq!(
            null_safe["diagnostics"]["sourceFormat"]["codec"],
            Value::Null
        );
        assert!(!safe.to_string().contains("private.invalid"));
        assert!(!safe.to_string().contains("secret"));
        assert!(!safe.to_string().contains("private title"));
        assert!(!safe.to_string().contains("/private/file"));
        let mut stale = base.clone();
        stale["diagnostics"]["owner"]["seekSerial"] = json!(1);
        assert!(sanitize_webview_media_inspection(stale).is_err());
        let mut oversized = base.clone();
        oversized["diagnostics"]["events"] = json!(vec![json!({"type":"playing"}); 65]);
        assert!(sanitize_webview_media_inspection(oversized).is_err());
        let mut malformed_format = base.clone();
        malformed_format["diagnostics"]["sourceFormat"]["codec"] =
            json!("codec\nhttps://private.invalid/token");
        assert!(sanitize_webview_media_inspection(malformed_format).is_err());
        let mut oversized_length = base;
        oversized_length["diagnostics"]["sourceFormat"]["expectedContentLength"] = json!(u64::MAX);
        assert!(sanitize_webview_media_inspection(oversized_length).is_err());
    }

    #[test]
    fn public_duration_repair_errors_are_sanitized() {
        assert_eq!(
            public_duration_repair_error("RATE_LIMITED: private retry metadata").code,
            "RATE_LIMITED"
        );
        assert_eq!(
            public_duration_repair_error("PUBLIC_FETCH_FAILED: https://private/path").message,
            "Public Spotify duration repair failed; check status before retrying"
        );
    }

    #[test]
    fn main_ui_capture_is_finite_and_read_only() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "capture_main_ui")
            .unwrap();
        assert_eq!(tool_scope("capture_main_ui"), Some("read"));
        assert_eq!(tool["scope"], "read");
        assert_eq!(tool["inputSchema"]["properties"], json!({}));
        assert_eq!(tool["inputSchema"]["required"], json!([]));
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert!(require_no_arguments(&json!({}), "capture_main_ui").is_ok());
        assert!(require_no_arguments(&json!({"window":"login"}), "capture_main_ui").is_err());
        assert!(require_no_arguments(&json!(null), "capture_main_ui").is_err());
    }

    #[test]
    fn artwork_diagnostics_redacts_private_fields_and_rejects_unbounded_data() {
        let fixture = || {
            json!({"artworkDiagnostics":{
                "urlScheme":"asset","blacklisted":true,"probeStatus":"failed",
                "probeWidth":0,"probeHeight":0,"cspBlocked":false,
                "privateUrl":"asset://localhost/private-artwork.jpg",
                "images":[
                    {"role":"footer","present":false,"complete":false,"naturalWidth":0,"naturalHeight":0,"urlScheme":"none","src":"private"},
                    {"role":"details","present":true,"complete":true,"naturalWidth":300,"naturalHeight":300,"urlScheme":"asset"}
                ]
            }})
        };
        let report = sanitize_artwork_diagnostics(&fixture()).unwrap();
        assert_eq!(report["blacklisted"], true);
        assert_eq!(report["images"][1]["naturalWidth"], 300.);
        assert!(!report.to_string().contains("private"));
        assert!(!report.to_string().contains("asset://"));
        assert!(report["probeSrcScheme"].is_null());
        assert!(report["images"][0]["srcScheme"].is_null());
        let mut extended = fixture();
        extended["artworkDiagnostics"]["probeSrcScheme"] = json!("asset");
        extended["artworkDiagnostics"]["probeCurrentSrcScheme"] = json!("http");
        extended["artworkDiagnostics"]["probeSrcMatchesExpected"] = json!(true);
        extended["artworkDiagnostics"]["probeCurrentSrcMatchesExpected"] = json!(false);
        extended["artworkDiagnostics"]["probeOnError"] = json!(true);
        extended["artworkDiagnostics"]["privatePath"] = json!("/private/artwork.jpg");
        extended["artworkDiagnostics"]["images"][0]["srcScheme"] = json!("asset");
        extended["artworkDiagnostics"]["images"][0]["currentSrcScheme"] = json!("http");
        extended["artworkDiagnostics"]["images"][0]["srcMatchesExpected"] = json!(true);
        extended["artworkDiagnostics"]["images"][0]["currentSrcMatchesExpected"] = json!(false);
        extended["artworkDiagnostics"]["images"][0]["loadState"] = json!("failed");
        let extended_report = sanitize_artwork_diagnostics(&extended).unwrap();
        assert_eq!(extended_report["probeSrcScheme"], "asset");
        assert_eq!(extended_report["probeCurrentSrcScheme"], "http");
        assert_eq!(extended_report["probeSrcMatchesExpected"], true);
        assert_eq!(extended_report["probeCurrentSrcMatchesExpected"], false);
        assert_eq!(extended_report["probeOnError"], true);
        assert_eq!(extended_report["images"][0]["srcScheme"], "asset");
        assert_eq!(extended_report["images"][0]["currentSrcScheme"], "http");
        assert_eq!(extended_report["images"][0]["srcMatchesExpected"], true);
        assert_eq!(
            extended_report["images"][0]["currentSrcMatchesExpected"],
            false
        );
        assert_eq!(extended_report["images"][0]["loadState"], "failed");
        assert!(!extended_report.to_string().contains("private"));
        let mut invalid_extended = extended.clone();
        invalid_extended["artworkDiagnostics"]["probeCurrentSrcScheme"] = json!("file:///private");
        assert!(sanitize_artwork_diagnostics(&invalid_extended).is_err());
        let mut invalid_extended = extended.clone();
        invalid_extended["artworkDiagnostics"]["probeOnError"] = json!("true");
        assert!(sanitize_artwork_diagnostics(&invalid_extended).is_err());
        let mut invalid_extended = extended.clone();
        invalid_extended["artworkDiagnostics"]["images"][0]["srcMatchesExpected"] = json!("yes");
        assert!(sanitize_artwork_diagnostics(&invalid_extended).is_err());
        let mut invalid_extended = extended.clone();
        invalid_extended["artworkDiagnostics"]["images"][0]["loadState"] = json!("/private/path");
        assert!(sanitize_artwork_diagnostics(&invalid_extended).is_err());
        let mut invalid = fixture();
        invalid["artworkDiagnostics"]["probeWidth"] = json!(32769);
        assert!(sanitize_artwork_diagnostics(&invalid).is_err());
        let mut invalid = fixture();
        invalid["artworkDiagnostics"]["images"][0]["naturalHeight"] = json!(-1);
        assert!(sanitize_artwork_diagnostics(&invalid).is_err());
        let mut invalid = fixture();
        invalid["artworkDiagnostics"]["images"][0]["urlScheme"] = json!("file:///private");
        assert!(sanitize_artwork_diagnostics(&invalid).is_err());
        let mut invalid = fixture();
        invalid["artworkDiagnostics"]["probeStatus"] = json!("private-error-text");
        assert!(sanitize_artwork_diagnostics(&invalid).is_err());
        let mut invalid = fixture();
        invalid["artworkDiagnostics"]["blacklisted"] = json!("true");
        assert!(sanitize_artwork_diagnostics(&invalid).is_err());
        let mut invalid = fixture();
        invalid["artworkDiagnostics"]["images"][0]["role"] = json!("login");
        assert!(sanitize_artwork_diagnostics(&invalid).is_err());
    }

    #[test]
    fn playback_layout_tool_is_read_only_and_requires_no_arguments() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "inspect_playback_layout")
            .unwrap();
        assert_eq!(tool["scope"], "read");
        assert_eq!(tool["inputSchema"]["required"], json!([]));
        assert_eq!(tool_scope("inspect_playback_layout"), Some("read"));
    }
    #[test]
    fn playback_layout_reports_real_overlap_and_overflow_from_allowed_roles() {
        let elements = PLAYBACK_LAYOUT_ROLES.iter().map(|role| {
            let (x,y,width,height,client,scroll,present,visible) = match *role {
                "footer" => (0.,150.,320.,50.,320.,320.,true,true),
                "now_playing" => (0.,150.,180.,40.,180.,205.,true,true),
                "transport" => (164.,151.,140.,35.,140.,140.,true,true),
                "scrubber" => (0.,190.,180.,10.,180.,180.,true,true),
                "right" => (20.,175.,30.,10.,30.,30.,true,true),
                "quality" => (60.,170.,90.,10.,90.,108.,true,true),
                _ => (220.,24.,80.,24.,80.,80.,true,false),
            };
            json!({"role":role,"present":present,"visible":visible,"x":x,"y":y,"width":width,"height":height,"clientWidth":client,"scrollWidth":scroll})
        }).collect::<Vec<_>>();
        // The 16px overlap between the now-playing and transport rectangles is intentional test input.
        assert_eq!(elements.len(), PLAYBACK_LAYOUT_ROLES.len());
        let report = sanitize_playback_layout(
            json!({"viewport":{"width":320,"height":200},"footerHeight":50,"elements":elements}),
        )
        .unwrap();
        let findings = report["findings"].as_array().unwrap();
        assert!(findings
            .iter()
            .any(|f| f["code"] == "overlap" && f["roles"] == json!(["now_playing", "transport"])));
        assert!(findings
            .iter()
            .any(|f| f["code"] == "overlap" && f["roles"] == json!(["now_playing", "right"])));
        assert!(!findings
            .iter()
            .any(|f| f["code"] == "overlap" && f["roles"] == json!(["now_playing", "quality"])));
        assert!(findings
            .iter()
            .any(|f| f["code"] == "horizontal_overflow" && f["roles"] == json!(["now_playing"])));
    }
    #[test]
    fn playback_layout_reports_queue_header_clipping_and_control_overlaps() {
        let elements = PLAYBACK_LAYOUT_ROLES.iter().map(|role| {
            let (x,y,width,height,present,visible) = match *role {
                "footer" => (0.,550.,320.,50.,true,true),
                "now_playing" => (0.,550.,80.,30.,true,true),
                "transport" => (100.,550.,80.,30.,true,true),
                "scrubber" => (0.,590.,180.,8.,true,true),
                "right" => (280.,590.,40.,8.,true,true),
                "quality" | "error" => (0.,0.,0.,0.,false,false),
                "queue_panel" => (40.,0.,280.,500.,true,true),
                "queue_header" => (41.,1.,278.,64.,true,true),
                "queue_title" => (58.,20.,64.,22.,true,true),
                "identify" => (136.,18.,82.,34.,true,true),
                "queue_close" => (232.,18.,24.,34.,true,true),
                "panel_close" => (270.,82.,28.,34.,true,true),
                _ => unreachable!(),
            };
            json!({"role":role,"present":present,"visible":visible,"x":x,"y":y,"width":width,"height":height,"clientWidth":width,"scrollWidth":width})
        }).collect::<Vec<_>>();
        let report = sanitize_playback_layout(
            json!({"viewport":{"width":320,"height":600},"footerHeight":50,"elements":elements}),
        )
        .unwrap();
        let findings = report["findings"].as_array().unwrap();
        assert!(!findings
            .iter()
            .any(|finding| finding["code"] == "queue_header_clipping"));
        assert!(!findings.iter().any(|finding| finding["code"] == "overlap"
            && finding["roles"] == json!(["queue_title", "identify"])));
        let clipped = PLAYBACK_LAYOUT_ROLES.iter().map(|role| {
            let mut item = json!({"role":role,"present":false,"visible":false,"x":0,"y":0,"width":0,"height":0,"clientWidth":0,"scrollWidth":0});
            match *role {
                "queue_panel" => item = json!({"role":role,"present":true,"visible":true,"x":40,"y":0,"width":280,"height":500,"clientWidth":280,"scrollWidth":280}),
                "queue_header" => item = json!({"role":role,"present":true,"visible":true,"x":41,"y":1,"width":278,"height":64,"clientWidth":278,"scrollWidth":278}),
                "queue_title" => item = json!({"role":role,"present":true,"visible":true,"x":58,"y":20,"width":64,"height":22,"clientWidth":64,"scrollWidth":64}),
                "identify" => item = json!({"role":role,"present":true,"visible":true,"x":270,"y":18,"width":82,"height":34,"clientWidth":82,"scrollWidth":82}),
                "panel_close" => item = json!({"role":role,"present":true,"visible":true,"x":270,"y":82,"width":28,"height":34,"clientWidth":28,"scrollWidth":28}),
                _ => {}
            }
            item
        }).collect::<Vec<_>>();
        let clipped = sanitize_playback_layout(
            json!({"viewport":{"width":400,"height":600},"footerHeight":50,"elements":clipped}),
        )
        .unwrap();
        assert!(clipped["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "queue_header_clipping"
                && finding["roles"] == json!(["queue_header", "identify"])));
    }

    #[test]
    fn playback_layout_accepts_only_sanitized_track_state_counts() {
        let elements=PLAYBACK_LAYOUT_ROLES.iter().map(|role|json!({"role":role,"present":true,"visible":true,"x":0,"y":0,"width":10,"height":10,"clientWidth":10,"scrollWidth":10})).collect::<Vec<_>>();
        let track_lists = PLAYBACK_TRACK_LIST_ROLES.iter().map(|role| {
            json!({"role":role,"visibleRows":4,"playing":1,"paused":1,"buffering":1,"trackId":"PRIVATE-ID","title":"PRIVATE TITLE","url":"https://private.invalid"})
        }).collect::<Vec<_>>();
        let result = sanitize_playback_layout(
            json!({"viewport":{"width":100,"height":100},"footerHeight":10,"elements":elements,"trackListsCounts":track_lists}),
        ).unwrap();
        assert_eq!(
            result["trackListsCounts"].as_array().unwrap().len(),
            PLAYBACK_TRACK_LIST_ROLES.len()
        );
        assert_eq!(
            result["trackListsCounts"][0],
            json!({"role":"search","visibleRows":4,"playing":1,"paused":1,"buffering":1})
        );
        let serialized = result.to_string();
        assert!(!serialized.contains("PRIVATE-ID"));
        assert!(!serialized.contains("PRIVATE TITLE"));
        assert!(!serialized.contains("private.invalid"));
    }

    #[test]
    fn playback_layout_rejects_unknown_or_inconsistent_track_counts() {
        assert!(sanitize_track_list_counts(&json!({"trackListsCounts":[{"role":"custom","visibleRows":0,"playing":0,"paused":0,"buffering":0}]})).is_err());
        let inconsistent = json!({"trackListsCounts":[
            {"role":"search","visibleRows":0,"playing":1,"paused":0,"buffering":0},
            {"role":"library","visibleRows":0,"playing":0,"paused":0,"buffering":0},
            {"role":"history","visibleRows":0,"playing":0,"paused":0,"buffering":0},
            {"role":"artist","visibleRows":0,"playing":0,"paused":0,"buffering":0},
            {"role":"remote","visibleRows":0,"playing":0,"paused":0,"buffering":0}
        ]});
        assert!(sanitize_track_list_counts(&inconsistent).is_err());
    }

    #[test]
    fn playback_layout_drops_nonallowlisted_payload_fields() {
        let elements=PLAYBACK_LAYOUT_ROLES.iter().map(|role|json!({"role":role,"present":true,"visible":true,"x":0,"y":0,"width":10,"height":10,"clientWidth":10,"scrollWidth":10,"title":"private"})).collect::<Vec<_>>();
        let result = sanitize_playback_layout(
            json!({"viewport":{"width":100,"height":100},"footerHeight":10,"elements":elements}),
        )
        .unwrap();
        assert!(result["elements"][0].get("title").is_none());
    }
    #[test]
    fn advertised_discovery_sources_are_accepted_by_request_validation() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "discover_music")
            .unwrap();
        let advertised = tool["inputSchema"]["properties"]["sourceIds"]["items"]["enum"]
            .as_array()
            .unwrap();
        assert!(advertised
            .iter()
            .any(|source| source.as_str() == Some("monochrome")));
        for source in advertised {
            assert!(validate_discovery_sources(&[source.as_str().unwrap().to_owned()]).is_ok());
        }
        assert!(validate_discovery_sources(&["unrecognized-provider".into()]).is_err());
    }
    #[test]
    fn discovery_cursor_schema_matches_the_decoder_protocol_limit() {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "discover_music")
            .unwrap();
        assert_eq!(
            tool["inputSchema"]["properties"]["cursor"]["maxLength"],
            crate::discovery::MAX_DISCOVERY_CURSOR_LENGTH
        );
    }

    #[test]
    fn playing_audio_identification_tools_require_control_and_share_a_bounded_uuid() {
        let tools = tool_definitions();
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == "identify_playing_track")
            .expect("playing audio identification tool");
        let cancel = tools
            .iter()
            .find(|tool| tool["name"] == "cancel_playing_track_identification")
            .expect("identification cancellation tool");
        assert_eq!(tool_scope("identify_playing_track"), Some("control"));
        assert_eq!(
            tool_scope("cancel_playing_track_identification"),
            Some("control")
        );
        assert_eq!(tool["inputSchema"]["required"], json!([]));
        assert_eq!(
            tool["inputSchema"]["properties"]["captureId"]["maxLength"],
            36
        );
        assert_eq!(cancel["inputSchema"]["required"], json!(["captureId"]));
        assert_eq!(
            cancel["inputSchema"]["properties"]["captureId"]["maxLength"],
            36
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(cancel["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            parse_identification_capture_id(&json!({}), false).unwrap(),
            None
        );
        assert_eq!(
            parse_identification_capture_id(
                &json!({"captureId":"123e4567-e89b-12d3-a456-426614174000"}),
                false
            )
            .unwrap(),
            Some("123e4567-e89b-12d3-a456-426614174000".into())
        );
        for arguments in [
            json!({"captureId":"not-a-uuid"}),
            json!({"captureId":"123e4567e89b12d3a456426614174000"}),
            json!({"captureId":"123e4567-e89b-12d3-a456-426614174000","command":"other"}),
            json!({}),
        ] {
            assert!(parse_identification_capture_id(&arguments, true).is_err());
        }
    }

    #[test]
    fn mcp_identification_cancel_tombstones_both_backends_before_identify_arrives() {
        let capture_id = Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000")
            .unwrap()
            .to_string();
        let native = crate::playing_track_identification::PlayingTrackIdentifications::default();
        let webview = crate::webview_identification::WebViewIdentification::default();
        assert!(cancel_identification_registries(
            &capture_id,
            |id| native.cancel(id),
            |id| webview.cancel_pcm_capture(id),
        ));
        assert!(native.begin(Some(capture_id.clone())).is_err());
        assert!(webview.begin_pcm_capture(capture_id).is_err());
    }

    #[test]
    fn app_status_exposes_only_sanitized_managed_provider_qualification() {
        let status = crate::managed_provider_session::ManagedProviderSessionStatus {
            enabled: true,
            running: true,
            mode: "background".into(),
            spotify_account_id: Some("private-account-id".into()),
            youtube_account_id: None,
            pending_youtube_account_id: None,
            pending_youtube_display_name: None,
            account_revisions: std::collections::BTreeMap::from([("spotify".into(), 2)]),
            connection_seen: true,
            message: "contains a friendly message".into(),
            transport: "native-webview-http".into(),
            credential_storage: "os-keyring".into(),
            youtube_login_page: Some("google-accounts".into()),
            youtube_reconnect_required: true,
        };
        let safe = managed_provider_qualification_status(&status);
        assert_eq!(safe["confirmedAccounts"]["spotify"], true);
        assert_eq!(safe["confirmedAccounts"]["youtube"], false);
        assert_eq!(safe["accountRevisions"]["spotify"], 2);
        assert_eq!(safe["mode"], "background");
        assert_eq!(safe["transport"], "native-webview-http");
        assert_eq!(safe["credentialStorage"], "os-keyring");
        assert_eq!(safe["youtubeLoginPage"], "google-accounts");
        assert_eq!(safe["youtubeReconnectRequired"], true);
        assert!(safe.get("spotifyAccountId").is_none());
        assert!(safe.get("youtubeAccountId").is_none());
        assert!(safe.get("message").is_none());
        assert!(!safe.to_string().contains("private-account-id"));
    }

    #[test]
    fn provider_session_tools_require_control_and_accept_no_credentials() {
        assert_eq!(
            tool_scope("get_managed_provider_session_status"),
            Some("read")
        );
        for name in [
            "start_managed_provider_login",
            "confirm_managed_provider_account",
            "start_managed_provider_background",
            "stop_managed_provider_session",
            "return_youtube_login_to_music",
        ] {
            assert_eq!(tool_scope(name), Some("control"));
            let definition = tool_definitions()
                .into_iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            let properties = definition["inputSchema"]["properties"].as_object().unwrap();
            if name == "return_youtube_login_to_music"
                || name == "start_managed_provider_background"
                || name == "stop_managed_provider_session"
            {
                assert!(properties.is_empty());
            } else {
                assert!(properties.keys().all(|key| key == "provider"));
            }
            assert_eq!(definition["inputSchema"]["additionalProperties"], false);
        }
    }

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
        assert_eq!(tool_scope("list_remote_playlists"), Some("read"));
        let exact_track = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "get_remote_track")
            .expect("exact remote track tool");
        assert_eq!(tool_scope("get_remote_track"), Some("read"));
        assert_eq!(
            exact_track["inputSchema"]["required"],
            json!(["provider", "trackId"])
        );
        assert_eq!(
            exact_track["inputSchema"]["properties"]["trackId"]["pattern"],
            "^[A-Za-z0-9]{22}$"
        );
        assert_eq!(exact_track["inputSchema"]["additionalProperties"], false);
        let crosswalk = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == "prepare_spotify_recording_crosswalk")
            .expect("Spotify recording crosswalk tool");
        assert_eq!(
            tool_scope("prepare_spotify_recording_crosswalk"),
            Some("read")
        );
        assert_eq!(
            crosswalk["inputSchema"]["required"],
            json!(["trackId", "requestId"])
        );
        assert_eq!(
            crosswalk["inputSchema"]["properties"]["trackId"]["pattern"],
            "^[A-Za-z0-9]{22}$"
        );
        assert_eq!(
            crosswalk["inputSchema"]["properties"]["musicBrainzRecordingIdHint"]["pattern"],
            "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
        );
        assert_eq!(crosswalk["inputSchema"]["additionalProperties"], false);
        for (name, id_key) in [
            ("get_remote_artist", "artistId"),
            ("browse_remote_artist_tracks", "artistId"),
            ("get_remote_album_tracks", "albumId"),
        ] {
            let tool = tool_definitions()
                .into_iter()
                .find(|tool| tool["name"] == name)
                .expect("Spotify browse tool");
            assert_eq!(tool_scope(name), Some("read"));
            assert_eq!(tool["inputSchema"]["required"], json!([id_key]));
            assert_eq!(
                tool["inputSchema"]["properties"][id_key]["pattern"],
                "^[A-Za-z0-9]{22}$"
            );
            assert_eq!(
                tool["inputSchema"]["properties"]["cursor"]["type"],
                json!(["string", "null"])
            );
            assert_eq!(
                tool["inputSchema"]["properties"]["cursor"]["pattern"],
                "^[0-9]{1,6}$"
            );
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
        assert_eq!(tool_scope("get_playlist_sync_preview"), Some("read"));
        assert_eq!(tool_scope("import_remote_playlists"), Some("control"));
        assert_eq!(tool_scope("apply_playlist_sync_preview"), Some("manage"));
        assert_eq!(tool_scope("list_track_files"), Some("read"));
        assert_eq!(tool_scope("set_preferred_track_file"), Some("control"));
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
    fn spotify_browse_arguments_validate_ids_and_numeric_cursors() {
        assert!(spotify_id_arg(&json!({"id":"0123456789ABCDEFGHIJKL"}), "id").is_ok());
        assert!(spotify_id_arg(&json!({"id":"too-short"}), "id").is_err());
        assert!(spotify_id_arg(&json!({"id":"0123456789ABCDEFGHIJK!"}), "id").is_err());
        assert_eq!(
            numeric_spotify_cursor_arg(&json!({"cursor":"30"}))
                .unwrap()
                .as_deref(),
            Some("30")
        );
        assert_eq!(numeric_spotify_cursor_arg(&json!({})).unwrap(), None);
        assert_eq!(
            numeric_spotify_cursor_arg(&json!({"cursor":null})).unwrap(),
            None
        );
        assert!(numeric_spotify_cursor_arg(&json!({"cursor":"100001"})).is_err());
        assert!(numeric_spotify_cursor_arg(&json!({"cursor":"3.5"})).is_err());
        assert!(numeric_spotify_cursor_arg(&json!({"cursor":"030"})).is_err());
    }
    #[test]
    fn playback_history_search_options_are_bounded_and_validated() {
        let options = playback_history_search_options(&json!({
            "query":"  Night Drive ", "provider":"youtube", "stationId":"station-1", "from":"2026-01-02",
            "to":"2026-01-05T12:30:00Z", "sort":"oldest", "sortField":"duration", "direction":"desc",
            "minDurationSeconds":60, "maxDurationSeconds":240, "offset":4, "limit":9
        }))
        .unwrap();
        assert_eq!(options.query.as_deref(), Some("Night Drive"));
        assert_eq!(options.provider.as_deref(), Some("youtube"));
        assert_eq!(options.station_id.as_deref(), Some("station-1"));
        assert_eq!(options.sort.as_deref(), Some("oldest"));
        assert_eq!(options.sort_field.as_deref(), Some("duration"));
        assert_eq!(options.direction.as_deref(), Some("desc"));
        assert_eq!(options.min_duration_seconds, Some(60.0));
        assert_eq!(options.max_duration_seconds, Some(240.0));
        assert_eq!((options.limit, options.offset), (9, 4));
        assert!(playback_history_search_options(&json!({
            "from":"2026-01-02T12:30:00Z", "to":"2026-01-02"
        }))
        .is_ok());
        assert!(playback_history_search_options(&json!({"from":"yesterday"})).is_err());
        assert!(playback_history_search_options(&json!({"sort":"random"})).is_err());
        assert!(playback_history_search_options(&json!({"sortField":"title desc"})).is_err());
        assert!(playback_history_search_options(&json!({"direction":"sideways"})).is_err());
        assert!(playback_history_search_options(&json!({"minDurationSeconds":-1})).is_err());
        assert!(playback_history_search_options(
            &json!({"minDurationSeconds":120,"maxDurationSeconds":60})
        )
        .is_err());
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
                artwork_url: Some(
                    "asset://localhost/%2Fhome%2Flistener%2FMusic%2Fcover.jpg".into(),
                ),
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
    fn native_playback_automation_payload_redacts_paths_and_private_or_signed_urls() {
        let mut payload = json!({
            "track": {
                "id": "track-1",
                "localPath": "/private/music/song.flac",
                "sourceUrl": "https://media.example.test/audio?signature=secret",
                "provenance": {
                    "registeredPath": "/private/music/song.flac",
                    "stream": "http://192.168.1.10:9000/audio",
                    "page": "https://www.youtube.com/watch?v=public-id"
                }
            },
            "queue": [{
                "id": "track-2",
                "localPath": "/private/music/next.flac",
                "sourceUrl": "https://media.example.test/audio?token=private"
            }],
            "candidate": {"url": "https://cdn.example.test/audio?lsig=private"},
            "quality": {"codec": "flac"}
        });

        strip_native_playback_fields(&mut payload);

        assert!(payload["track"].get("localPath").is_none());
        assert!(payload["track"]["provenance"]
            .get("registeredPath")
            .is_none());
        assert!(payload["track"].get("sourceUrl").is_none());
        assert!(payload["track"]["provenance"].get("stream").is_none());
        assert_eq!(
            payload["track"]["provenance"]["page"],
            "https://www.youtube.com/watch?v=public-id"
        );
        assert!(payload["queue"][0].get("localPath").is_none());
        assert!(payload["queue"][0].get("sourceUrl").is_none());
        assert!(payload["candidate"].get("url").is_none());
        assert_eq!(payload["quality"]["codec"], "flac");
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

    fn history_protocol_packet(
        client: &str,
        sequence: u64,
        queue: &str,
        history: &str,
        full_queue: bool,
        full_history: bool,
    ) -> Value {
        let mut packet = json!({"protocolVersion":1,"clientId":client,"stateSequence":sequence,"queueRevision":queue,"queueUpdate":if full_queue {json!({"kind":"full"})} else {json!({"kind":"unchanged","queueRevision":queue})},"position":sequence,"failureHistoryRevision":history,"failureHistoryUpdate":if full_history {json!({"kind":"full"})} else {json!({"kind":"unchanged","failureHistoryRevision":history})}});
        if full_queue {
            packet["queue"] = json!([{"id":"track-a"}]);
            packet["queueEntryIds"] = json!(["entry-a"]);
        }
        if full_history {
            packet["diagnostics"] = json!({"recentTrackFailures":[{"queueRevision":queue,"generation":1,"entryId":"entry-a","trackId":"track-a","reason":"No playable audio match was found","outcome":"skipped"}]});
        }
        packet
    }

    #[test]
    fn acknowledged_history_pulses_move_the_same_array_and_keep_status_dto() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        let initial = history_protocol_packet("page-a", 1, "queue-a", "h1", true, true);
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, initial),
            (true, false)
        );
        assert_eq!(
            playback_update_ack(true, false, protocol.failure_history_revision.as_deref())
                ["failureHistoryRevision"],
            "h1"
        );
        assert!(
            playback_update_ack(false, true, protocol.failure_history_revision.as_deref())
                .get("failureHistoryRevision")
                .is_none()
        );
        let history = current.fields["diagnostics"]["recentTrackFailures"]
            .as_array()
            .unwrap()
            .as_ptr();
        let queue = Arc::clone(&current.queue);
        for sequence in 2..=101 {
            let pulse = history_protocol_packet("page-a", sequence, "queue-a", "h1", false, false);
            assert!(pulse.to_string().len() < 400);
            assert_eq!(
                accept_playback_update(&mut protocol, &mut current, pulse),
                (true, false)
            );
            assert_eq!(
                current.fields["diagnostics"]["recentTrackFailures"]
                    .as_array()
                    .unwrap()
                    .as_ptr(),
                history
            );
            assert!(Arc::ptr_eq(&queue, &current.queue));
        }
        assert_eq!(
            current.page(0, 10)["diagnostics"]["recentTrackFailures"][0]["entryId"],
            "entry-a"
        );
        assert_eq!(
            current.page(0, 10)["diagnostics"]["recentTrackFailures"][0]["category"],
            "no_playable_match"
        );
        assert!(current.watch_snapshot()["diagnostics"]
            .get("recentTrackFailures")
            .is_none());
        assert!(current.fields.get("failureHistoryRevision").is_none());
        assert!(current.fields.get("failureHistoryUpdate").is_none());
    }

    #[test]
    fn history_revision_mismatch_rejects_without_mutation_and_full_resync_restores_it() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        assert!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-a", 1, "queue-a", "h1", true, true)
            )
            .0
        );
        let before = current.page(0, 10);
        let queue = Arc::clone(&current.queue);
        for packet in [
            history_protocol_packet("page-a", 2, "queue-a", "wrong", false, false),
            history_protocol_packet("page-a", 2, "queue-b", "h1", true, false),
            history_protocol_packet("page-b", 1, "queue-a", "h1", true, false),
            history_protocol_packet("page-a", 2, "queue-a", "h1", true, false),
        ] {
            assert_eq!(
                accept_playback_update(&mut protocol, &mut current, packet),
                (false, true)
            );
            assert_eq!(current.page(0, 10), before);
            assert!(Arc::ptr_eq(&queue, &current.queue));
            assert_eq!(protocol.last_sequence, 1);
            assert_eq!(protocol.queue_revision.as_deref(), Some("queue-a"));
            assert_eq!(protocol.failure_history_revision.as_deref(), Some("h1"));
        }
        let resync = history_protocol_packet("page-a", 2, "queue-a", "h2", true, true);
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, resync),
            (true, false)
        );
        assert_eq!(protocol.failure_history_revision.as_deref(), Some("h2"));
        assert_eq!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-a", 3, "queue-a", "h2", false, false)
            ),
            (true, false)
        );
        let stale = history_protocol_packet("page-a", 1, "queue-a", "h1", true, true);
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, stale),
            (false, false)
        );
        assert_eq!(protocol.failure_history_revision.as_deref(), Some("h2"));
    }

    #[test]
    fn history_source_and_client_resets_require_full_owned_history_and_clear_old_records() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        assert!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-a", 1, "queue-a", "h1", true, true)
            )
            .0
        );
        let mut source_reset = history_protocol_packet("page-a", 2, "queue-a", "h2", false, true);
        source_reset["diagnostics"]["recentTrackFailures"] = json!([]);
        assert!(accept_playback_update(&mut protocol, &mut current, source_reset).0);
        assert_eq!(
            current.page(0, 10)["diagnostics"]["recentTrackFailures"],
            json!([])
        );
        assert_eq!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-a", 3, "queue-a", "h1", false, false)
            ),
            (false, true)
        );
        assert!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-b", 1, "queue-b", "h1", true, true)
            )
            .0
        );
        assert_eq!(
            current.page(0, 10)["diagnostics"]["recentTrackFailures"][0]["queueRevision"],
            "queue-b"
        );
        assert_eq!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-a", 4, "queue-a", "h2", true, true)
            ),
            (false, true)
        );
        assert_eq!(protocol.active_client.as_deref(), Some("page-b"));
        assert_eq!(protocol.queue_revision.as_deref(), Some("queue-b"));
    }

    #[test]
    fn malformed_history_full_packets_do_not_commit_queue_or_sequence() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        assert!(
            accept_playback_update(
                &mut protocol,
                &mut current,
                history_protocol_packet("page-a", 1, "queue-a", "h1", true, true)
            )
            .0
        );
        let before = current.page(0, 10);
        let mut malformed = history_protocol_packet("page-a", 2, "queue-b", "h2", true, true);
        malformed["diagnostics"]["recentTrackFailures"][0]["queueRevision"] = json!("stale-owner");
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, malformed),
            (false, true)
        );
        assert_eq!(current.page(0, 10), before);
        assert_eq!(protocol.last_sequence, 1);
        let mut missing = history_protocol_packet("page-a", 2, "queue-b", "h2", true, true);
        missing.as_object_mut().unwrap().remove("diagnostics");
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, missing),
            (false, true)
        );
        assert_eq!(current.page(0, 10), before);
        let mut oversized = history_protocol_packet("page-a", 2, "queue-b", "h2", true, true);
        let record = oversized["diagnostics"]["recentTrackFailures"][0].clone();
        oversized["diagnostics"]["recentTrackFailures"] = json!(vec![record; 65]);
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, oversized),
            (false, true)
        );
        assert_eq!(current.page(0, 10), before);
        let mut unexpected = history_protocol_packet("page-a", 2, "queue-a", "h1", false, false);
        unexpected["diagnostics"] = json!({"recentTrackFailures":[]});
        assert_eq!(
            accept_playback_update(&mut protocol, &mut current, unexpected),
            (false, true)
        );
        assert_eq!(current.page(0, 10), before);
    }

    #[test]
    fn playback_failure_history_is_allowlisted_redacted_and_preserved_in_status_pages() {
        let revision = json!("queue-a");
        let raw = json!([{"queueRevision":"queue-a","generation":12,"operationId":"retry-12","entryId":"duplicate-second","trackId":"same-track","title":"Title https://private.invalid/?sig=secret","artist":"Artist","reason":"HTTP 404 unavailable /home/private/music.flac C:\\private\\audio.flac token=token-secret cookie=cookie-secret Authorization: Bearer bearer-secret","outcome":"skipped","category":"no_playable_match","sourceUrl":"https://private.invalid/?token=secret","private":{"token":"secret"}}]);
        let safe = sanitize_recent_track_failures(&raw, Some(&revision)).unwrap();
        assert_eq!(safe[0]["category"], "failed_with_reason");
        assert_eq!(safe[0]["entryId"], "duplicate-second");
        assert_eq!(safe[0]["operationId"], "retry-12");
        let encoded = safe.to_string();
        for secret in [
            "private.invalid",
            "music.flac",
            "audio.flac",
            "token-secret",
            "cookie-secret",
            "bearer-secret",
            "sourceUrl",
        ] {
            assert!(!encoded.contains(secret));
        }
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut state = WebViewPlaybackState::default();
        let packet = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":1,"queueRevision":"queue-a","queueUpdate":{"kind":"full"},"queue":[{"id":"same-track"}],"queueEntryIds":["duplicate-second"],"diagnostics":{"recentTrackFailures":raw}});
        assert_eq!(
            accept_playback_update(&mut protocol, &mut state, packet),
            (true, false)
        );
        assert_eq!(
            state.page(0, 10)["diagnostics"]["recentTrackFailures"],
            safe
        );
        assert!(state.watch_snapshot()["diagnostics"]
            .get("recentTrackFailures")
            .is_none());
    }

    #[test]
    fn webview_watch_projection_skips_history_before_cloning_and_explicit_pages_keep_it() {
        let mut state = WebViewPlaybackState::default();
        state.fields = json!({"position":7,"status":"playing","diagnostics":{"metric":12,"lastSkippedTrackFailure":{"reason":"earlier failure"},"recentTrackFailures":[]}});
        let empty_watch = state.watch_snapshot();
        let record = json!({"queueRevision":"queue-a","generation":12,"entryId":"entry-a","trackId":"track-a","title":"T".repeat(160),"artist":"A".repeat(160),"reason":"r".repeat(250),"outcome":"skipped","category":"failed_with_reason"});
        state.fields["diagnostics"]["recentTrackFailures"] = json!(vec![record; 64]);
        for _ in 0..100 {
            let watch = state.watch_snapshot();
            assert_eq!(watch, empty_watch);
            assert!(watch["diagnostics"].get("recentTrackFailures").is_none());
            assert_eq!(watch["diagnostics"]["metric"], 12);
            assert_eq!(
                watch["diagnostics"]["lastSkippedTrackFailure"]["reason"],
                "earlier failure"
            );
            assert!(watch.to_string().len() < 1000);
        }
        let page = state.page(0, 50);
        assert_eq!(
            page["diagnostics"]["recentTrackFailures"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert!(page.to_string().len() > 30_000);
        assert_eq!(
            state.fields["diagnostics"]["recentTrackFailures"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
    }

    #[test]
    fn playback_failure_history_rejects_unbounded_stale_and_malformed_records() {
        let revision = json!("queue-a");
        let record = json!({"queueRevision":"queue-a","generation":12,"entryId":"first","reason":"No playable audio match was found","outcome":"queue_exhausted"});
        assert!(
            sanitize_recent_track_failures(&json!(vec![record.clone(); 65]), Some(&revision))
                .is_none()
        );
        assert!(
            sanitize_recent_track_failures(&json!([record.clone()]), Some(&json!("queue-b")))
                .is_none()
        );
        for (key, value) in [
            ("generation", json!(-1)),
            ("entryId", json!("https://private.invalid/entry")),
            ("outcome", json!("cancelled")),
            ("reason", json!("x".repeat(2049))),
        ] {
            let mut malformed = record.clone();
            malformed[key] = value;
            assert!(sanitize_recent_track_failures(&json!([malformed]), Some(&revision)).is_none());
        }
        let mut state = WebViewPlaybackState::default();
        let mut incoming =
            json!({"queueRevision":"queue-b","diagnostics":{"recentTrackFailures":[record]}});
        replace_playback_fields(&mut state, &mut incoming);
        assert!(state.page(0, 10)["diagnostics"]
            .get("recentTrackFailures")
            .is_none());
    }

    #[test]
    fn playback_queue_pulses_reuse_arc_and_reject_mismatched_revisions_wholesale() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        let initial = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":1,"queueRevision":"rev-a","queueUpdate":{"kind":"full"},"queue":[{"id":"a"}],"position":0,"currentIndex":0});
        let (ok, need_full) = accept_playback_update(&mut protocol, &mut current, initial);
        assert!(ok && !need_full);
        assert_eq!(current.page(0, 10)["queue"][0]["id"], "a");
        let queue = Arc::clone(&current.queue);
        for sequence in 2..102 {
            let pulse = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":sequence,"queueRevision":"rev-a","queueUpdate":{"kind":"unchanged","queueRevision":"rev-a"},"position":sequence,"currentIndex":0});
            let (ok, need_full) = accept_playback_update(&mut protocol, &mut current, pulse);
            assert!(ok && !need_full);
            assert!(Arc::ptr_eq(&queue, &current.queue));
        }
        assert_eq!(current.fields["position"], 101);
        let same_revision_full = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":102,"queueRevision":"rev-a","queueUpdate":{"kind":"full"},"queue":[{"id":"a"}],"position":102,"currentIndex":0});
        assert!(accept_playback_update(&mut protocol, &mut current, same_revision_full).0);
        assert!(
            Arc::ptr_eq(&queue, &current.queue),
            "same-revision completion full snapshots keep the existing queue Arc"
        );
        let mismatch = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":103,"queueRevision":"rev-b","queueUpdate":{"kind":"unchanged","queueRevision":"rev-b"},"position":77,"currentIndex":99});
        let (ok, need_full) = accept_playback_update(&mut protocol, &mut current, mismatch);
        assert!(!ok && need_full);
        assert_eq!(
            current.fields["position"], 102,
            "mismatch must reject the entire control snapshot"
        );
        assert!(Arc::ptr_eq(&queue, &current.queue));
    }

    #[test]
    fn playback_pages_remain_bounded_and_preserve_duplicate_occurrences_without_full_queue_watch_updates(
    ) {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        let queue = (0..20_000)
            .map(|slot| json!({"id":"duplicate-track","slot":slot}))
            .collect::<Vec<_>>();
        let entry_ids = (0..20_000)
            .map(|slot| json!(format!("entry-{slot}")))
            .collect::<Vec<_>>();
        let full = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":1,"queueRevision":"large","queueUpdate":{"kind":"full"},"queue":queue,"queueEntryIds":entry_ids,"status":"playing"});
        let (ok, need_full) = accept_playback_update(&mut protocol, &mut current, full);
        assert!(ok && !need_full);
        let saved_queue = Arc::clone(&current.queue);
        let watch = current.watch_snapshot();
        assert!(watch.get("queue").is_none());
        assert_eq!(watch["queueTotal"], 20_000);
        let page = current.page(19_995, 200);
        assert_eq!(page["queue"].as_array().unwrap().len(), 5);
        assert_eq!(page["queueEntryIds"][0], "entry-19995");
        assert_eq!(page["queue"][4]["slot"], 19_999);
        for sequence in 2..102 {
            let pulse = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":sequence,"queueRevision":"large","queueUpdate":{"kind":"unchanged","queueRevision":"large"},"position":sequence});
            assert!(accept_playback_update(&mut protocol, &mut current, pulse).0);
            assert!(Arc::ptr_eq(&saved_queue, &current.queue));
        }
    }

    #[test]
    fn playback_publisher_rejects_reordered_sequences_and_retired_page_ids() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        let full = |client: &str, seq: u64, rev: &str| json!({"protocolVersion":1,"clientId":client,"stateSequence":seq,"queueRevision":rev,"queueUpdate":{"kind":"full"},"queue":[{"id":client}],"position":seq});
        assert!(accept_playback_update(&mut protocol, &mut current, full("page-a", 5, "a")).0);
        let old_fields = current.fields.clone();
        let (ok, need_full) =
            accept_playback_update(&mut protocol, &mut current, full("page-a", 4, "old"));
        assert!(!ok && !need_full);
        assert_eq!(current.fields, old_fields);
        assert!(accept_playback_update(&mut protocol, &mut current, full("page-b", 1, "a")).0);
        let new_fields = current.fields.clone();
        let (ok, need_full) =
            accept_playback_update(&mut protocol, &mut current, full("page-a", 6, "late"));
        assert!(!ok && need_full);
        assert_eq!(current.fields, new_fields);
    }

    #[test]
    fn legacy_playback_snapshots_remain_accepted_before_protocol_negotiation() {
        let mut protocol = PlaybackPublisherProtocol::default();
        let mut current = WebViewPlaybackState::default();
        let legacy = json!({"status":"playing","queue":[{"id":"legacy"}]});
        assert!(accept_playback_update(&mut protocol, &mut current, legacy.clone()).0);
        assert_eq!(current.page(0, 10)["queue"][0]["id"], "legacy");
        assert!(accept_playback_update(&mut protocol, &mut current, legacy).0);
        let full = json!({"protocolVersion":1,"clientId":"page-a","stateSequence":1,"queueRevision":"r","queueUpdate":{"kind":"full"},"queue":[{"id":"a"}]});
        assert!(accept_playback_update(&mut protocol, &mut current, full).0);
        let (ok, need_full) = accept_playback_update(
            &mut protocol,
            &mut current,
            json!({"status":"idle","queue":[]}),
        );
        assert!(!ok && !need_full);
        assert_eq!(current.page(0, 10)["queue"][0]["id"], "a");
    }

    #[test]
    fn playback_state_pages_bound_queue_and_preserve_control_metadata() {
        let queue = (0..4500)
            .map(|index| json!({"id":format!("track-{index}")}))
            .collect::<Vec<_>>();
        let entries = (0..4500)
            .map(|index| json!(format!("entry-{index}")))
            .collect::<Vec<_>>();
        let snapshot = json!({
            "status":"playing", "queueRevision":7, "currentIndex":2140,
            "queue":queue, "queueEntryIds":entries
        });
        let page = page_playback_snapshot(&snapshot, 2100, 50);
        assert_eq!(page["status"], "playing");
        assert_eq!(page["queueRevision"], 7);
        assert_eq!(page["currentIndex"], 2140);
        assert_eq!(page["queueTotal"], 4500);
        assert_eq!(page["queueOffset"], 2100);
        assert_eq!(page["queue"].as_array().unwrap().len(), 50);
        assert_eq!(page["queue"][0]["id"], "track-2100");
        assert_eq!(page["queueEntryIds"][0], "entry-2100");
        assert_eq!(page["queueHasMore"], true);

        let oversized = page_playback_snapshot(&snapshot, 4499, 500);
        assert_eq!(oversized["queue"].as_array().unwrap().len(), 1);
        assert_eq!(oversized["queueHasMore"], false);
    }
    #[test]
    fn spotify_artist_inspector_is_read_scoped_and_rejects_extra_arguments() {
        assert_eq!(tool_scope("inspect_spotify_artist_page"), Some("read"));
        assert!(
            serde_json::from_value::<crate::remote_playlists::SpotifyArtistInspectionRequest>(
                json!({"artistId":"0123456789ABCDEFGHIJKL","raw":true})
            )
            .is_err()
        );
    }
}

#[cfg(test)]
#[test]
fn youtube_library_inspector_is_read_only_and_has_no_identifier_arguments() {
    assert_eq!(tool_scope("inspect_youtube_library_page"), Some("read"));
    let tools = tool_definitions();
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "inspect_youtube_library_page")
        .unwrap();
    assert_eq!(tool["inputSchema"]["properties"], json!({}));
}

#[cfg(test)]
#[test]
fn exact_playlist_and_history_navigation_are_finite_route_only_targets() {
    let id = "0123456789ABCDEFGHIJKL";
    assert_eq!(
        parse_ui_navigation_request(&json!({"spotifyPlaylistId":id})).unwrap(),
        UiNavigationTarget::SpotifyPlaylist(id.into())
    );
    for direction in ["back", "forward"] {
        let target = parse_ui_navigation_request(&json!({"historyDirection":direction})).unwrap();
        assert_eq!(
            target.payload(),
            json!({"kind":"history","historyDirection":direction})
        );
        assert_eq!(
            sanitize_ui_navigation_completion(&target, direction, "complete").unwrap()
                ["completionScope"],
            "route_transition"
        );
        assert!(sanitize_ui_navigation_completion(&target, "library", "complete").is_err());
    }
    for args in [
        json!({"spotifyPlaylistId":"https://open.spotify.com/playlist/0123456789ABCDEFGHIJKL"}),
        json!({"spotifyPlaylistId":"0123456789ABCDEFGHIJK!"}),
        json!({"spotifyPlaylistId":id,"view":"library"}),
        json!({"historyDirection":"back","spotifyPlaylistId":id}),
        json!({"historyDirection":"javascript:history.back()"}),
        json!({"historyDirection":-1}),
    ] {
        assert!(parse_ui_navigation_request(&args).is_err());
    }
}

#[cfg(test)]
mod writeback_mcp_parity_tests {
    use super::*;
    #[test]
    fn reviewed_preview_parser_preserves_tokens_and_rejects_unreviewed_payloads() {
        let token = "123e4567-e89b-42d3-a456-426614174000";
        assert!(parse_writeback_preview(&json!({"bindingId":"binding"}))
            .unwrap()
            .reviewed_matches
            .is_none());
        let item = json!({"localTrackId":"local","candidateToken":token});
        let parsed = parse_writeback_preview(
            &json!({"bindingId":"binding","reviewedMatches":[item.clone()]}),
        )
        .unwrap();
        assert_eq!(parsed.reviewed_matches.unwrap()[0].candidate_token, token);
        for value in [
            json!({"bindingId":"binding","reviewedMatches":[{"localTrackId":"local","candidateToken":token,"uri":"spotify:track:injected"}]}),
            json!({"bindingId":"binding","reviewedMatches":[item.clone(),item]}),
            json!({"bindingId":"binding","reviewedMatches":[{"localTrackId":"local","candidateToken":"not-a-token"}]}),
            json!({"bindingId":"binding","enableWrites":true}),
            json!({"bindingId":"binding","reviewedMatches":null}),
            json!({"bindingId":" "}),
        ] {
            assert!(parse_writeback_preview(&value).is_err());
        }
    }
    #[test]
    fn candidate_search_parser_is_bounded_and_read_only() {
        assert!(
            parse_writeback_candidates(&json!({"bindingId":"binding","localTrackId":"local"}))
                .is_ok()
        );
        for value in [
            json!({"bindingId":"binding"}),
            json!({"bindingId":"binding","localTrackId":""}),
            json!({"bindingId":"binding","localTrackId":"x".repeat(301)}),
            json!({"bindingId":"binding","localTrackId":"local","provider":"spotify"}),
        ] {
            assert!(parse_writeback_candidates(&value).is_err());
        }
        assert_eq!(
            tool_scope("search_remote_playlist_writeback_candidates"),
            Some("read")
        );
        assert_eq!(tool_scope("get_playlist_sync_preview"), Some("read"));
    }
}

#[cfg(test)]
mod verification_intent_mcp_tests {
    use super::*;
    #[test]
    fn creation_requires_explicit_complete_approval() {
        assert!(parse_verification_intent_approval(&json!({"provider":"spotify"}), false).is_ok());
        assert!(parse_verification_intent_approval(&json!({"provider":"spotify"}), true).is_err());
        let good = json!({"provider":"youtube","intentId":"123e4567-e89b-42d3-a456-426614174000","reviewToken":"a".repeat(64),"contractHash":"b".repeat(64)});
        assert!(parse_verification_intent_approval(&good, true).is_ok());
        let mut bad = good;
        bad["title"] = json!("arbitrary");
        assert!(parse_verification_intent_approval(&bad, true).is_err());
        assert_eq!(
            tool_scope("apply_verification_playlist_intent"),
            Some("manage")
        );
        assert_eq!(
            tool_scope("preview_verification_playlist_intent"),
            Some("control")
        );
    }
}
