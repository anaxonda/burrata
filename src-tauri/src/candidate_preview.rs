//! Official provider players in a window without application command permissions.
use crate::models::Candidate;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    session_id: String,
    source_id: String,
    provider: String,
    title: String,
    url: String,
    media_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlayerStatus {
    session_id: String,
    paused: bool,
    position: f64,
    duration: f64,
    can_seek: bool,
    can_volume: bool,
    #[serde(default)]
    error: Option<String>,
}

pub struct CandidatePreviewManager {
    app: AppHandle,
    token: String,
    operations: Mutex<()>,
    accepted_generation: Mutex<u64>,
    base_url: Mutex<Option<String>>,
    session: Mutex<Option<Session>>,
}

impl CandidatePreviewManager {
    pub fn new(app: AppHandle, token: String) -> Arc<Self> {
        Arc::new(Self {
            app,
            token,
            operations: Mutex::new(()),
            accepted_generation: Mutex::new(0),
            base_url: Mutex::new(None),
            session: Mutex::new(None),
        })
    }
    pub fn set_base_url(&self, url: String) {
        *self.base_url.lock().unwrap() = Some(url.trim_end_matches('/').into());
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/player", get(player_html))
            .route("/player.js", get(player_js))
            .route("/player.css", get(player_css))
            .route("/player/session", get(player_session))
            .route("/player/status", post(player_status))
            .with_state(self.clone())
    }
    pub async fn start(
        self: &Arc<Self>,
        candidate: &Candidate,
        request_generation: Option<u64>,
    ) -> Result<Value, String> {
        // Serialize window operations and use the frontend generation to reject an
        // older IPC that arrives after a newer preview request. No awaits occur here.
        let _operation = self
            .operations
            .lock()
            .map_err(|_| "Player state unavailable")?;
        if let Some(generation) = request_generation {
            let mut accepted = self
                .accepted_generation
                .lock()
                .map_err(|_| "Player state unavailable")?;
            if !accept_generation(generation, &mut accepted) {
                return Err("Candidate preview request was superseded".into());
            }
        }
        let (provider, url, media_id) = canonical_source(&candidate.url)?;
        let session = Session {
            session_id: uuid::Uuid::new_v4().to_string(),
            source_id: candidate.id.clone(),
            provider,
            title: candidate.title.clone(),
            url,
            media_id,
        };
        let base = self
            .base_url
            .lock()
            .map_err(|_| "Player state unavailable")?
            .clone()
            .ok_or("Local preview server is not running")?;
        *self
            .session
            .lock()
            .map_err(|_| "Player state unavailable")? = Some(session.clone());
        if let Some(window) = self.app.get_webview_window("candidate-preview") {
            window.eval(&command_script(json!({"action":"start","sessionId":session.session_id,"sourceId":session.source_id,"provider":session.provider,"title":session.title,"url":session.url,"mediaId":session.media_id}))) .map_err(|e|e.to_string())?;
            window.show().map_err(|e| e.to_string())?;
        } else {
            let mut target =
                url::Url::parse(&format!("{base}/player")).map_err(|e| e.to_string())?;
            target.query_pairs_mut().append_pair("token", &self.token);
            let window = WebviewWindowBuilder::new(
                &self.app,
                "candidate-preview",
                WebviewUrl::External(target),
            )
            .title("Burrata Player")
            .inner_size(480.0, 390.0)
            .min_inner_size(400.0, 360.0)
            .build()
            .map_err(|e| e.to_string())?;
            #[cfg(target_os = "linux")]
            enable_candidate_autoplay(&window)?;
            let manager = Arc::downgrade(self);
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    if let Some(manager) = manager.upgrade() {
                        manager.closed();
                    }
                }
            });
        }
        self.emit(&session,json!({"paused":true,"position":0,"duration":0,"canSeek":false,"canVolume":false,"closed":false,"error":null}));
        Ok(
            json!({"kind":"candidate","url":candidate.url,"canSeek":true,"canVolume":true,"canAutoplay":true,"isExcerpt":false,"sessionId":session.session_id,"provider":session.provider,"title":session.title}),
        )
    }
    pub fn control(&self, action: &str, value: Option<f64>) -> Result<(), String> {
        validate_action(action, value)?;
        if action == "stop" {
            return self.stop();
        }
        let _operation = self
            .operations
            .lock()
            .map_err(|_| "Player state unavailable")?;
        let session = self
            .session
            .lock()
            .map_err(|_| "Player state unavailable")?
            .clone()
            .ok_or("No candidate is playing")?;
        let window = self
            .app
            .get_webview_window("candidate-preview")
            .ok_or("Candidate player window is closed")?;
        window
            .eval(&command_script(
                json!({"action":action,"value":value,"sessionId":session.session_id}),
            ))
            .map_err(|e| e.to_string())
    }
    pub fn stop(&self) -> Result<(), String> {
        self.stop_session(None)
    }
    pub fn stop_session(&self, expected_session: Option<&str>) -> Result<(), String> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| "Player state unavailable")?;
        if let Some(expected) = expected_session {
            let current = self
                .session
                .lock()
                .map_err(|_| "Player state unavailable")?;
            if !session_matches(
                Some(expected),
                current.as_ref().map(|s| s.session_id.as_str()),
            ) {
                return Ok(());
            }
        }
        if let Some(window) = self.app.get_webview_window("candidate-preview") {
            window
                .eval(&command_script(json!({"action":"stop"})))
                .map_err(|e| e.to_string())?;
        }
        if let Some(session) = self
            .session
            .lock()
            .map_err(|_| "Player state unavailable")?
            .take()
        {
            self.emit(&session,json!({"paused":true,"position":0,"duration":0,"canSeek":false,"canVolume":false,"closed":false,"error":null}));
        }
        Ok(())
    }
    fn closed(&self) {
        if let Ok(mut guard) = self.session.lock() {
            if let Some(session) = guard.take() {
                self.emit(&session,json!({"paused":true,"position":0,"duration":0,"canSeek":false,"canVolume":false,"closed":true,"error":null}));
            }
        }
    }
    fn emit(&self, session: &Session, mut payload: Value) {
        payload["sessionId"] = json!(session.session_id);
        payload["sourceId"] = json!(session.source_id);
        payload["provider"] = json!(session.provider);
        let _ = self
            .app
            .emit_to("main", "candidate-preview-status", payload);
    }
    fn authorized(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == format!("Bearer {}", self.token))
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn enable_candidate_autoplay(window: &tauri::WebviewWindow) -> Result<(), String> {
    window
        .with_webview(|webview| {
            use webkit2gtk::{SettingsExt, WebViewExt};
            if let Some(settings) = webview.inner().settings() {
                settings.set_media_playback_requires_user_gesture(false);
            }
        })
        .map_err(|e| format!("Cannot enable preview autoplay: {e}"))
}

fn accept_generation(generation: u64, accepted: &mut u64) -> bool {
    if generation < *accepted {
        false
    } else {
        *accepted = generation;
        true
    }
}

fn session_matches(expected: Option<&str>, current: Option<&str>) -> bool {
    expected.is_none_or(|id| current == Some(id))
}

fn command_script(value: Value) -> String {
    format!("window.burrataCandidateCommand && window.burrataCandidateCommand({value});")
}

fn canonical_source(raw: &str) -> Result<(String, String, String), String> {
    let parsed = url::Url::parse(raw).map_err(|_| "Candidate URL is invalid")?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Unsupported candidate URL".into());
    }
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let parts: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
    let id = if matches!(
        host.as_str(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
    ) {
        if parsed.path() == "/watch" {
            parsed
                .query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.into_owned())
        } else if parts.len() == 2 && matches!(parts[0], "shorts" | "embed" | "live") {
            Some(parts[1].into())
        } else {
            None
        }
    } else if host == "youtu.be" && parts.len() == 1 {
        Some(parts[0].into())
    } else {
        None
    };
    if let Some(id) = id {
        if id.len() != 11
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err("Invalid YouTube video ID".into());
        }
        return Ok((
            "youtube".into(),
            format!("https://www.youtube.com/watch?v={id}"),
            id,
        ));
    }
    if matches!(
        host.as_str(),
        "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com"
    ) && parts.len() == 2
        && parts.iter().all(|p| !p.is_empty())
        && !matches!(
            parts[0],
            "discover" | "search" | "charts" | "you" | "stream"
        )
        && parts[1] != "sets"
    {
        return Ok((
            "soundcloud".into(),
            format!("https://soundcloud.com/{}/{}", parts[0], parts[1]),
            String::new(),
        ));
    }
    Err("Preview supports individual YouTube videos and SoundCloud tracks, not playlists or other sites".into())
}

fn validate_action(action: &str, value: Option<f64>) -> Result<(), String> {
    match action {
        "play" | "pause" | "stop" if value.is_none() => Ok(()),
        "seek" if value.is_some_and(|v| v.is_finite() && (0.0..=604800.0).contains(&v)) => Ok(()),
        "volume" if value.is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v)) => Ok(()),
        _ => Err("Invalid player action or value (seek seconds; volume 0–1)".into()),
    }
}
fn valid_status(status: &PlayerStatus, current: &str) -> bool {
    status.session_id == current
        && status.position.is_finite()
        && status.duration.is_finite()
        && (0.0..=604800.0).contains(&status.position)
        && (0.0..=604800.0).contains(&status.duration)
        && status.error.as_ref().is_none_or(|e| e.len() <= 2000)
}

#[derive(Deserialize)]
struct PlayerQuery {
    token: Option<String>,
}
async fn player_html(
    State(manager): State<Arc<CandidatePreviewManager>>,
    Query(query): Query<PlayerQuery>,
) -> Response {
    if query.token.as_deref() != Some(manager.token.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ([ (header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CACHE_CONTROL,"no-store"),
        (header::CONTENT_SECURITY_POLICY,"default-src 'none'; script-src 'self' https://www.youtube.com https://s.ytimg.com https://w.soundcloud.com; style-src 'self'; frame-src https://www.youtube.com https://w.soundcloud.com; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"),
        (header::REFERRER_POLICY,"strict-origin-when-cross-origin") ],include_str!("player/index.html")).into_response()
}
async fn player_js() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("player/player.js"),
    )
}
async fn player_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("player/player.css"),
    )
}
async fn player_session(
    State(manager): State<Arc<CandidatePreviewManager>>,
    headers: HeaderMap,
) -> Response {
    if !manager.authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match manager.session.lock().unwrap().clone() {
        Some(session) => Json(session).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn player_status(
    State(manager): State<Arc<CandidatePreviewManager>>,
    headers: HeaderMap,
    Json(status): Json<PlayerStatus>,
) -> StatusCode {
    if !manager.authorized(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    let guard = manager.session.lock().unwrap();
    if let Some(session) = guard.as_ref() {
        if status.session_id != session.session_id {
            return StatusCode::NO_CONTENT;
        }
        if !valid_status(&status, &session.session_id) {
            return StatusCode::BAD_REQUEST;
        }
        manager.emit(session,json!({"paused":status.paused,"position":status.position,"duration":status.duration,"canSeek":status.can_seek,"canVolume":status.can_volume,"error":status.error,"closed":false}));
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_older_preview_request_generations() {
        let mut accepted = 0;
        assert!(accept_generation(20, &mut accepted));
        assert_eq!(accepted, 20);
        assert!(!accept_generation(19, &mut accepted));
        assert_eq!(accepted, 20);
        assert!(accept_generation(21, &mut accepted));
        assert_eq!(accepted, 21);
    }
    #[test]
    fn scoped_stop_matches_only_its_current_session() {
        assert!(session_matches(Some("current"), Some("current")));
        assert!(!session_matches(Some("old"), Some("current")));
        assert!(!session_matches(Some("old"), None));
        assert!(session_matches(None, Some("current")));
    }
    #[test]
    fn youtube_canonical_urls() {
        for url in [
            "https://youtu.be/M7lc1UVf-VE?t=3",
            "https://www.youtube.com/watch?v=M7lc1UVf-VE&list=x",
            "https://youtube.com/shorts/M7lc1UVf-VE",
        ] {
            let (provider, url, id) = canonical_source(url).unwrap();
            assert_eq!(provider, "youtube");
            assert_eq!(url, "https://www.youtube.com/watch?v=M7lc1UVf-VE");
            assert_eq!(id, "M7lc1UVf-VE");
        }
    }
    #[test]
    fn reject_playlists_and_wrong_hosts() {
        for url in [
            "https://youtube.com/playlist?list=x",
            "https://youtube.com.evil/watch?v=M7lc1UVf-VE",
            "https://youtu.be/invalid",
            "https://soundcloud.com/artist/sets/list",
            "https://soundcloud.com/search",
            "https://soundcloud.com:444/a/b",
            "https://user@soundcloud.com/a/b",
        ] {
            assert!(canonical_source(url).is_err(), "{url}");
        }
    }
    #[test]
    fn soundcloud_track_url() {
        let (provider, url, _) =
            canonical_source("https://www.soundcloud.com/artist/song?utm_source=x").unwrap();
        assert_eq!(provider, "soundcloud");
        assert_eq!(url, "https://soundcloud.com/artist/song");
    }
    #[test]
    fn controls_validate_numeric_ranges() {
        assert!(validate_action("play", None).is_ok());
        assert!(validate_action("seek", Some(10.0)).is_ok());
        assert!(validate_action("volume", Some(0.5)).is_ok());
        for (action, value) in [
            ("seek", Some(f64::NAN)),
            ("seek", Some(-1.0)),
            ("volume", Some(100.0)),
            ("unknown", None),
            ("pause", Some(1.0)),
        ] {
            assert!(validate_action(action, value).is_err());
        }
    }
    #[test]
    fn rejects_stale_and_nonfinite_status() {
        let mut status = PlayerStatus {
            session_id: "old".into(),
            paused: true,
            position: 0.0,
            duration: 0.0,
            can_seek: true,
            can_volume: true,
            error: None,
        };
        assert!(!valid_status(&status, "new"));
        status.session_id = "new".into();
        assert!(valid_status(&status, "new"));
        status.position = f64::INFINITY;
        assert!(!valid_status(&status, "new"));
    }
    #[test]
    fn metadata_serialized_as_data() {
        let script = command_script(json!({"title":"\";alert('bad'); //"}));
        assert!(script.contains("\\\";alert"));
    }
}
