//! Native macOS and Windows system media controls for the shared audio player.
//!
//! `start` is intended to run from Tauri's `setup` callback. That callback runs
//! on the app's main thread, which is required by the macOS Now Playing bridge;
//! Windows also needs the live HWND of the main webview when controls are made.

use std::sync::Arc;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::time::Duration;

use serde_json::Value as JsonValue;
use tauri::AppHandle;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::Manager;

use crate::playback::PlaybackService;

/// Keeps the OS media session alive while the Tauri application is running.
pub struct SystemMediaService {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    worker: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl SystemMediaService {
    /// Connect system media keys and Now Playing metadata to native playback.
    ///
    /// Call during Tauri setup, while the main window and its native handle are
    /// available. On Linux this is deliberately a no-op; Linux MPRIS remains
    /// owned by the existing `MprisService`.
    pub fn start(app: AppHandle, playback: Arc<PlaybackService>) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            start_native(app, playback)
        }
        #[cfg(target_os = "windows")]
        {
            start_native(app, playback)
        }
        #[cfg(target_os = "linux")]
        {
            let _ = (app, playback);
            Ok(Self {})
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = (app, playback);
            Ok(Self {})
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for SystemMediaService {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_native(
    app: AppHandle,
    playback: Arc<PlaybackService>,
) -> Result<SystemMediaService, String> {
    use souvlaki::{MediaControlEvent, MediaControls, PlatformConfig};
    // Tauri's setup callback is entered after its main native event loop and
    // AppDelegate have been installed. Keep this explicit check so this
    // service cannot silently start before the Tauri window lifecycle.
    let _main_window = app.get_webview_window("main").ok_or_else(|| {
        "cannot start system media controls before the main window exists".to_owned()
    })?;

    #[cfg(target_os = "windows")]
    let hwnd = {
        let window = app.get_webview_window("main").ok_or_else(|| {
            "cannot start system media controls: main window is missing".to_owned()
        })?;
        let hwnd = window.hwnd().map_err(|error| {
            format!("cannot get main window handle for media controls: {error}")
        })?;
        Some(hwnd.0)
    };
    #[cfg(target_os = "macos")]
    let hwnd = None;

    let mut controls = MediaControls::new(PlatformConfig {
        display_name: "Burrata",
        dbus_name: "org.burrata.player",
        hwnd,
    })
    .map_err(|error| format!("cannot initialize system media controls: {error}"))?;

    let callback_playback = playback.clone();
    controls
        .attach(move |event| {
            let playback = callback_playback.clone();
            tauri::async_runtime::spawn(async move {
                let _ = dispatch_event(playback, event).await;
            });
        })
        .map_err(|error| format!("cannot attach system media controls: {error}"))?;

    // Construction and attachment happen synchronously on Tauri's setup
    // thread. Updates then run on Tauri's async runtime while its native event
    // loop remains active.
    let mut state = playback.subscribe();
    let worker = tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        update_controls(&mut controls, state.borrow().clone());
        loop {
            tokio::select! {
                changed = state.changed() => {
                    if changed.is_err() { break; }
                    update_controls(&mut controls, state.borrow_and_update().clone());
                }
                _ = interval.tick() => {
                    // Refresh progress even when a backend does not publish
                    // a new snapshot every second.
                    update_controls(&mut controls, state.borrow().clone());
                }
            }
        }
    });

    Ok(SystemMediaService {
        worker: Some(worker),
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn dispatch_event(
    playback: Arc<PlaybackService>,
    event: souvlaki::MediaControlEvent,
) -> Result<(), String> {
    use souvlaki::{MediaControlEvent as Event, SeekDirection};
    if !playback.native_enabled() {
        return Ok(());
    }
    let snapshot = playback.snapshot();
    match event {
        Event::Play => execute(playback, "resume", None, None).await,
        Event::Pause => execute(playback, "pause", None, None).await,
        Event::Stop => execute(playback, "stop", None, None).await,
        Event::Toggle => {
            let action = if is_playing(&snapshot) {
                "pause"
            } else {
                "resume"
            };
            execute(playback, action, None, None).await
        }
        Event::Next => execute(playback, "next", None, None).await,
        Event::Previous => execute(playback, "previous", None, None).await,
        Event::Seek(direction) => {
            let offset = match direction {
                SeekDirection::Forward => 10.0,
                SeekDirection::Backward => -10.0,
            };
            execute(
                playback,
                "seek",
                Some((position_seconds(&snapshot) + offset).max(0.0)),
                None,
            )
            .await
        }
        Event::SeekBy(direction, amount) => {
            let offset = amount.as_secs_f64()
                * if direction == SeekDirection::Forward {
                    1.0
                } else {
                    -1.0
                };
            execute(
                playback,
                "seek",
                Some((position_seconds(&snapshot) + offset).max(0.0)),
                None,
            )
            .await
        }
        Event::SetPosition(position) => {
            execute(playback, "seek", Some(position.0.as_secs_f64()), None).await
        }
        Event::SetVolume(volume) => {
            execute(playback, "volume", None, Some(volume.clamp(0.0, 1.0))).await
        }
        // Opening arbitrary URIs and app lifecycle actions are not part of the
        // audio player's system-media command surface.
        Event::OpenUri(_) | Event::Raise | Event::Quit => Ok(()),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn execute(
    playback: Arc<PlaybackService>,
    action: &str,
    position: Option<f64>,
    volume: Option<f64>,
) -> Result<(), String> {
    playback
        .execute_ids(action, Vec::new(), None, position, volume)
        .await
        .map(|_| ())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn update_controls(controls: &mut souvlaki::MediaControls, snapshot: JsonValue) {
    use souvlaki::{MediaMetadata, MediaPlayback, MediaPosition};

    if snapshot["engine"].as_str() != Some("native") {
        return;
    }

    let metadata = track_metadata(&snapshot);
    let _ = controls.set_metadata(MediaMetadata {
        title: metadata.title.as_deref(),
        artist: metadata.artist.as_deref(),
        album: metadata.album.as_deref(),
        cover_url: metadata.cover_url.as_deref(),
        duration: metadata.duration_seconds.map(Duration::from_secs_f64),
    });

    let position = Duration::from_secs_f64(position_seconds(&snapshot));
    let progress = Some(MediaPosition(position));
    let status = match playback_status(&snapshot) {
        PlaybackStatus::Playing => MediaPlayback::Playing { progress },
        PlaybackStatus::Paused => MediaPlayback::Paused { progress },
        PlaybackStatus::Stopped => MediaPlayback::Stopped,
    };
    let _ = controls.set_playback(status);
}

#[derive(Debug, PartialEq)]
struct TrackMetadata {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    cover_url: Option<String>,
    duration_seconds: Option<f64>,
}

fn track_metadata(snapshot: &JsonValue) -> TrackMetadata {
    let track = &snapshot["track"];
    TrackMetadata {
        title: track["title"]
            .as_str()
            .or_else(|| snapshot["title"].as_str())
            .map(str::to_owned),
        artist: track["artist"]
            .as_str()
            .or_else(|| snapshot["artist"].as_str())
            .map(str::to_owned),
        album: track["album"]
            .as_str()
            .or_else(|| snapshot["album"].as_str())
            .map(str::to_owned),
        cover_url: artwork_uri(snapshot),
        duration_seconds: (duration_seconds(snapshot) > 0.0).then(|| duration_seconds(snapshot)),
    }
}

fn artwork_uri(snapshot: &JsonValue) -> Option<String> {
    let track = &snapshot["track"];
    if let Some(path) = track["artworkFilePath"]
        .as_str()
        .filter(|path| !path.is_empty() && std::path::Path::new(path).is_file())
    {
        let path = std::path::Path::new(path).canonicalize().ok()?;
        #[cfg(target_os = "windows")]
        {
            // souvlaki's Windows backend removes the `file://` prefix and
            // passes the remaining string directly to StorageFile. Preserve
            // Windows separators and avoid Url's `file:///C:/` form here.
            return Some(format!("file://{}", path.display()));
        }
        #[cfg(not(target_os = "windows"))]
        {
            return url::Url::from_file_path(path)
                .ok()
                .map(|url| url.to_string());
        }
    }
    track["artworkUrl"]
        .as_str()
        .or_else(|| track["artUrl"].as_str())
        .or_else(|| snapshot["artworkUrl"].as_str())
        .filter(|url| {
            url.starts_with("file://") || url.starts_with("https://") || url.starts_with("http://")
        })
        .map(str::to_owned)
}

fn is_playing(snapshot: &JsonValue) -> bool {
    match snapshot["status"]
        .as_str()
        .or_else(|| snapshot["playbackStatus"].as_str())
    {
        Some("playing" | "Playing") => true,
        Some("paused" | "Paused" | "idle" | "stopped" | "Stopped") => false,
        _ => snapshot["paused"].as_bool() == Some(false) && snapshot["trackId"].as_str().is_some(),
    }
}

fn position_seconds(snapshot: &JsonValue) -> f64 {
    finite_nonnegative(snapshot["position"].as_f64().unwrap_or(0.0))
}

fn duration_seconds(snapshot: &JsonValue) -> f64 {
    [
        snapshot["duration"].as_f64(),
        snapshot["durationSeconds"].as_f64(),
        snapshot["track"]["durationSeconds"].as_f64(),
    ]
    .into_iter()
    .flatten()
    .find(|value| value.is_finite() && *value > 0.0)
    .map(|value| value.min(315_576_000.0))
    .unwrap_or(0.0)
}

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0).min(315_576_000.0)
    } else {
        0.0
    }
}

#[derive(Debug, PartialEq)]
enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

fn playback_status(snapshot: &JsonValue) -> PlaybackStatus {
    match snapshot["status"]
        .as_str()
        .or_else(|| snapshot["playbackStatus"].as_str())
    {
        Some("playing" | "Playing") => PlaybackStatus::Playing,
        Some("paused" | "Paused") => PlaybackStatus::Paused,
        Some("idle" | "stopped" | "Stopped") => PlaybackStatus::Stopped,
        _ if is_playing(snapshot) => PlaybackStatus::Playing,
        _ if snapshot["trackId"].as_str().is_some() => PlaybackStatus::Paused,
        _ => PlaybackStatus::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_track_fields_duration_and_supported_artwork() {
        let mapped = track_metadata(&json!({
            "trackId": "track-1",
            "position": 18.5,
            "durationSeconds": 240.0,
            "track": {
                "title": "Night Drive",
                "artist": "DJ Example",
                "album": "After Hours",
                "artworkUrl": "asset://localhost/art.jpg"
            }
        }));
        assert_eq!(mapped.title.as_deref(), Some("Night Drive"));
        assert_eq!(mapped.artist.as_deref(), Some("DJ Example"));
        assert_eq!(mapped.album.as_deref(), Some("After Hours"));
        assert_eq!(mapped.duration_seconds, Some(240.0));
        assert_eq!(
            mapped.cover_url, None,
            "Tauri asset URLs are not OS-readable artwork URLs"
        );
    }

    #[test]
    fn maps_status_fallback_and_bounds_position() {
        assert_eq!(
            playback_status(&json!({"status":"playing"})),
            PlaybackStatus::Playing
        );
        assert_eq!(
            playback_status(&json!({"trackId":"t"})),
            PlaybackStatus::Paused
        );
        assert_eq!(playback_status(&json!({})), PlaybackStatus::Stopped);
        assert_eq!(position_seconds(&json!({"position":-3.0})), 0.0);
        assert_eq!(position_seconds(&json!({"position":1e300})), 315_576_000.0);
        assert_eq!(position_seconds(&json!({"position":"bad"})), 0.0);
    }
}
