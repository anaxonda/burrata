//! Linux MPRIS session-bus integration.
//!
//! The frontend remains the owner of the audio element. This service translates
//! desktop media-key requests into the same acknowledged automation requests
//! used by the app, and mirrors the latest frontend playback snapshot.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, RwLock},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::Value as JsonValue;
use sha2::Digest;
use tauri::{AppHandle, Manager};
use tokio::sync::{oneshot, watch};
use zbus::{
    fdo,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

use crate::automation::AutomationService;

const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.burrata";

/// Owns the MPRIS worker thread. Dropping it shuts down the session-bus name.
pub struct MprisService {
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl MprisService {
    /// Start the MPRIS service on the current user's session bus.
    pub fn start(
        app: AppHandle,
        automation: Arc<AutomationService>,
        mut playback: watch::Receiver<JsonValue>,
    ) -> Result<Self, String> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("burrata-mpris".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("create MPRIS runtime: {error}")));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let initial = playback.borrow().clone();
                    let state = Arc::new(RwLock::new(PlaybackCache::new(initial)));
                    let root = RootInterface { app: app.clone() };
                    let artwork_dir = app.path().app_data_dir().ok().map(|path| path.join("artwork"));
                    let player = PlayerInterface { state: state.clone(), automation, artwork_dir };
                    let connection = match zbus::connection::Builder::session()
                        .and_then(|builder| builder.name(BUS_NAME))
                    {
                        Ok(builder) => match builder
                            .serve_at(OBJECT_PATH, root)
                            .and_then(|builder| builder.serve_at(OBJECT_PATH, player))
                        {
                            Ok(builder) => builder.build().await,
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    };
                    let connection = match connection {
                        Ok(connection) => connection,
                        Err(error) => {
                            let _ = ready_tx.send(Err(format!("connect MPRIS session bus: {error}")));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    let mut shutdown_rx = shutdown_rx;
                    loop {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            changed = playback.changed() => {
                                if changed.is_err() { break; }
                                let next = playback.borrow().clone();
                                let previous = {
                                    let mut current = state.write().unwrap_or_else(|poisoned| poisoned.into_inner());
                                    let old = current.snapshot.clone();
                                    current.update(next.clone());
                                    old
                                };
                                let server = connection.object_server();
                                if let Ok(iface_ref) = server.interface::<_, PlayerInterface>(OBJECT_PATH).await {
                                    // Metadata and status are the notifications most desktop shells
                                    // depend on; the remaining mutable player properties are included
                                    // too so remote controls stay in sync.
                                    let signal = iface_ref.signal_context();
                                    let iface = iface_ref.get().await;
                                    let old_position = position_seconds(&previous);
                                    let new_position = position_seconds(&next);
                                    let same_track = previous["trackId"] == next["trackId"];
                                    let seeked = same_track && (previous["seekSerial"] != next["seekSerial"]
                                        || (new_position - old_position).abs() > 2.0);
                                    if previous["trackId"] != next["trackId"]
                                        || previous["track"] != next["track"]
                                        || previous["artworkUrl"] != next["artworkUrl"]
                                        || previous["track"]["artworkFilePath"] != next["track"]["artworkFilePath"]
                                        || previous["sourceUrl"] != next["sourceUrl"]
                                        || duration_seconds(&previous) != duration_seconds(&next)
                                    { let _ = iface.metadata_changed(signal).await; }
                                    if playback_status(&previous) != playback_status(&next) { let _ = iface.playback_status_changed(signal).await; }
                                    if volume_value(&previous) != volume_value(&next) { let _ = iface.volume_changed(signal).await; }
                                    if previous["shuffle"] != next["shuffle"] { let _ = iface.shuffle_changed(signal).await; }
                                    if loop_status(&previous) != loop_status(&next) { let _ = iface.loop_status_changed(signal).await; }
                                    let old_can_next = queue_index(&previous) + 1 < queue_len(&previous)
                                        || (queue_len(&previous) > 0 && loop_status(&previous) == "Playlist");
                                    let new_can_next = queue_index(&next) + 1 < queue_len(&next)
                                        || (queue_len(&next) > 0 && loop_status(&next) == "Playlist");
                                    let old_can_previous = queue_index(&previous) > 0
                                        || (queue_len(&previous) > 0 && loop_status(&previous) == "Playlist");
                                    let new_can_previous = queue_index(&next) > 0
                                        || (queue_len(&next) > 0 && loop_status(&next) == "Playlist");
                                    if old_can_next != new_can_next { let _ = iface.can_go_next_changed(signal).await; }
                                    if old_can_previous != new_can_previous { let _ = iface.can_go_previous_changed(signal).await; }
                                    let old_can_play = queue_len(&previous) > 0 || previous["trackId"].as_str().is_some();
                                    let new_can_play = queue_len(&next) > 0 || next["trackId"].as_str().is_some();
                                    if old_can_play != new_can_play { let _ = iface.can_play_changed(signal).await; }
                                    let old_can_pause = matches!(playback_status(&previous), "Playing" | "Paused");
                                    let new_can_pause = matches!(playback_status(&next), "Playing" | "Paused");
                                    if old_can_pause != new_can_pause { let _ = iface.can_pause_changed(signal).await; }
                                    if previous["canSeek"] != next["canSeek"] { let _ = iface.can_seek_changed(signal).await; }
                                    if seeked {
                                        let pos = (new_position.max(0.0) * 1_000_000.0).round() as i64;
                                        let _ = PlayerInterface::seeked(signal, pos).await;
                                    }
                                }
                            }
                        }
                    }
                });
            })
            .map_err(|error| format!("start MPRIS worker: {error}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                shutdown: Some(shutdown_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = shutdown_tx.send(());
                // The builder may still be waiting on a slow session bus. Drop
                // the handle instead of blocking startup past the timeout; the
                // cancelled shutdown channel makes a late connection exit.
                drop(thread);
                Err("MPRIS worker did not connect to the session bus within 5 seconds".into())
            }
        }
    }
}

impl Drop for MprisService {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct RootInterface {
    app: AppHandle,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl RootInterface {
    fn raise(&self) -> fdo::Result<()> {
        let app = self.app.clone();
        self.app
            .run_on_main_thread(move || {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            })
            .map_err(failed)
    }

    fn quit(&self) {
        self.app.exit(0);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Burrata"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "burrata"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into(), "http".into(), "https".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/wav",
            "audio/mp4",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }
}

struct PlayerInterface {
    state: Arc<RwLock<PlaybackCache>>,
    automation: Arc<AutomationService>,
    artwork_dir: Option<PathBuf>,
}

impl PlayerInterface {
    async fn command(
        &self,
        action: &str,
        position: Option<f64>,
        volume: Option<f64>,
        mode: Option<String>,
    ) -> fdo::Result<()> {
        self.automation
            .system_playback(action, position, volume, mode)
            .await
            .map(|_| ())
            .map_err(failed)
    }

    fn snapshot(&self) -> JsonValue {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current_snapshot()
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerInterface {
    async fn next(&self) -> fdo::Result<()> {
        if !self.can_go_next() {
            return Ok(());
        }
        self.command("next", None, None, None).await
    }
    async fn previous(&self) -> fdo::Result<()> {
        if !self.can_go_previous() {
            return Ok(());
        }
        self.command("previous", None, None, None).await
    }
    async fn pause(&self) -> fdo::Result<()> {
        self.command("pause", None, None, None).await
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        let action = if playback_status(&self.snapshot()) == "Playing" {
            "pause"
        } else {
            "resume"
        };
        self.command(action, None, None, None).await
    }
    async fn play(&self) -> fdo::Result<()> {
        if !self.can_play() || self.playback_status() == "Playing" {
            return Ok(());
        }
        self.command("resume", None, None, None).await
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.command("stop", None, None, None).await
    }
    async fn seek(&self, offset: i64) -> fdo::Result<()> {
        let snapshot = self.snapshot();
        if !snapshot["canSeek"].as_bool().unwrap_or(false) {
            return Ok(());
        }
        let current = position_seconds(&snapshot);
        let target = current + offset as f64 / 1_000_000.0;
        if duration_seconds(&snapshot) > 0.0 && target > duration_seconds(&snapshot) {
            return self.next().await;
        }
        self.command(
            "seek",
            Some((current + offset as f64 / 1_000_000.0).max(0.0)),
            None,
            None,
        )
        .await
    }
    async fn set_position(&self, track_id: OwnedObjectPath, position: i64) -> fdo::Result<()> {
        let snapshot = self.snapshot();
        if snapshot["trackId"].as_str().is_none()
            || track_id.as_str() != track_object_path(&snapshot)
            || !snapshot["canSeek"].as_bool().unwrap_or(false)
            || position < 0
            || (position as f64 / 1_000_000.0) > duration_seconds(&snapshot)
        {
            return Ok(());
        }
        self.command(
            "seek",
            Some((position.max(0) as f64 / 1_000_000.0).min(duration_seconds(&snapshot))),
            None,
            None,
        )
        .await
    }
    async fn open_uri(&self, uri: String) -> fdo::Result<()> {
        if !has_supported_uri_scheme(&uri) {
            return Err(fdo::Error::InvalidArgs("unsupported URI scheme".into()));
        }
        self.command("openUri", None, None, Some(uri)).await
    }

    #[zbus(property)]
    async fn set_volume(&self, volume: f64) -> zbus::Result<()> {
        if !volume.is_finite() {
            return Err(fdo::Error::InvalidArgs("volume must be finite".into()).into());
        }
        self.command("volume", None, Some(volume.clamp(0.0, 1.0)), None)
            .await
            .map_err(Into::into)
    }
    #[zbus(property)]
    async fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()> {
        self.command("shuffle", None, None, Some(shuffle.to_string()))
            .await
            .map_err(Into::into)
    }
    #[zbus(property)]
    async fn set_loop_status(&self, status: String) -> zbus::Result<()> {
        if !["None", "Track", "Playlist"].contains(&status.as_str()) {
            return Err(fdo::Error::InvalidArgs(
                "LoopStatus must be None, Track, or Playlist".into(),
            )
            .into());
        }
        let mode = match status.as_str() {
            "None" => "off",
            "Track" => "one",
            "Playlist" => "all",
            _ => unreachable!(),
        };
        self.command("repeat", None, None, Some(mode.into()))
            .await
            .map_err(Into::into)
    }
    #[zbus(property)]
    async fn set_rate(&self, rate: f64) -> zbus::Result<()> {
        if rate == 0.0 {
            return self
                .command("pause", None, None, None)
                .await
                .map_err(Into::into);
        }
        if !rate.is_finite() || (rate - 1.0).abs() > f64::EPSILON {
            return Err(
                fdo::Error::NotSupported("variable playback rate is not supported".into()).into(),
            );
        }
        Ok(())
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        playback_status(&self.snapshot()).to_string()
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        loop_status(&self.snapshot()).to_string()
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.snapshot()["shuffle"].as_bool().unwrap_or(false)
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(&self.snapshot(), self.artwork_dir.as_deref())
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.snapshot()["volume"]
            .as_f64()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        (position_seconds(&self.snapshot()) * 1_000_000.0).round() as i64
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        let snapshot = self.snapshot();
        queue_index(&snapshot) + 1 < queue_len(&snapshot)
            || (queue_len(&snapshot) > 0 && loop_status(&snapshot) == "Playlist")
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        let snapshot = self.snapshot();
        queue_index(&snapshot) > 0
            || (queue_len(&snapshot) > 0 && loop_status(&snapshot) == "Playlist")
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        let snapshot = self.snapshot();
        queue_len(&snapshot) > 0 || snapshot["trackId"].as_str().is_some()
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        matches!(playback_status(&self.snapshot()), "Playing" | "Paused")
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.snapshot()["canSeek"].as_bool().unwrap_or(false)
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(signal)]
    async fn seeked(signal: &zbus::SignalContext<'_>, position: i64) -> zbus::Result<()>;
}

fn failed(error: impl std::fmt::Display) -> fdo::Error {
    fdo::Error::Failed(error.to_string())
}

fn has_supported_uri_scheme(uri: &str) -> bool {
    url::Url::parse(uri)
        .map(|parsed| matches!(parsed.scheme(), "file" | "http" | "https"))
        .unwrap_or(false)
}

fn playback_status(snapshot: &JsonValue) -> &'static str {
    match snapshot["status"]
        .as_str()
        .or_else(|| snapshot["playbackStatus"].as_str())
    {
        Some("playing" | "Playing") => "Playing",
        Some("paused" | "Paused") => "Paused",
        Some("idle" | "stopped" | "Stopped") => "Stopped",
        _ => {
            if snapshot["paused"].as_bool() == Some(false) && snapshot["trackId"].as_str().is_some()
            {
                "Playing"
            } else if snapshot["trackId"].as_str().is_some() {
                "Paused"
            } else {
                "Stopped"
            }
        }
    }
}

fn loop_status(snapshot: &JsonValue) -> &'static str {
    match snapshot["repeat"].as_str().unwrap_or("off") {
        "one" | "track" | "Track" => "Track",
        "all" | "playlist" | "Playlist" => "Playlist",
        _ => "None",
    }
}

fn position_seconds(snapshot: &JsonValue) -> f64 {
    snapshot["position"].as_f64().unwrap_or(0.0).max(0.0)
}
fn volume_value(snapshot: &JsonValue) -> f64 {
    snapshot["volume"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0)
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
    .unwrap_or(0.0)
}
fn queue_len(snapshot: &JsonValue) -> usize {
    snapshot["queue"].as_array().map_or(0, Vec::len)
}
fn queue_index(snapshot: &JsonValue) -> usize {
    snapshot["currentIndex"].as_u64().unwrap_or(0) as usize
}

struct PlaybackCache {
    snapshot: JsonValue,
    updated_at: Instant,
}
impl PlaybackCache {
    fn new(snapshot: JsonValue) -> Self {
        Self {
            snapshot,
            updated_at: Instant::now(),
        }
    }
    fn update(&mut self, snapshot: JsonValue) {
        self.snapshot = snapshot;
        self.updated_at = Instant::now();
    }
    fn current_snapshot(&self) -> JsonValue {
        let mut current = self.snapshot.clone();
        if playback_status(&current) == "Playing" {
            let position = position_seconds(&current) + self.updated_at.elapsed().as_secs_f64();
            let duration = duration_seconds(&current);
            current["position"] = serde_json::json!(if duration > 0.0 {
                position.min(duration)
            } else {
                position
            });
        }
        current
    }
}

fn track_object_path(snapshot: &JsonValue) -> String {
    let Some(id) = snapshot["trackId"].as_str() else {
        return "/org/mpris/MediaPlayer2/TrackList/NoTrack".into();
    };
    let digest = sha2::Sha256::digest(id.as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    format!("/org/mpris/MediaPlayer2/track/{}", &encoded[..24])
}

fn into_owned<T: Into<Value<'static>>>(value: T) -> OwnedValue {
    OwnedValue::try_from(value.into()).expect("MPRIS metadata value is owned")
}

fn metadata(snapshot: &JsonValue, artwork_dir: Option<&Path>) -> HashMap<String, OwnedValue> {
    let mut result = HashMap::new();
    let track = &snapshot["track"];
    let title = track["title"]
        .as_str()
        .or_else(|| snapshot["title"].as_str())
        .unwrap_or("");
    let artist = track["artist"]
        .as_str()
        .or_else(|| snapshot["artist"].as_str())
        .unwrap_or("");
    let album = track["album"]
        .as_str()
        .or_else(|| snapshot["album"].as_str())
        .unwrap_or("");
    let track_path = track_object_path(snapshot);
    result.insert(
        "mpris:trackid".into(),
        OwnedValue::try_from(
            zbus::zvariant::ObjectPath::try_from(track_path.as_str())
                .expect("valid generated track path"),
        )
        .expect("owned MPRIS track path"),
    );
    result.insert("xesam:title".into(), into_owned(title.to_string()));
    result.insert("xesam:artist".into(), into_owned(vec![artist.to_string()]));
    if !album.is_empty() {
        result.insert("xesam:album".into(), into_owned(album.to_string()));
    }
    if let Some(art) =
        cached_artwork_url(track["artworkFilePath"].as_str(), artwork_dir).or_else(|| {
            track["artworkUrl"]
                .as_str()
                .or_else(|| track["artUrl"].as_str())
                .or_else(|| snapshot["artworkUrl"].as_str())
                .filter(|url| has_supported_uri_scheme(url) && !url.starts_with("file:"))
                .map(str::to_string)
        })
    {
        result.insert("mpris:artUrl".into(), into_owned(art));
    }
    let url = track["sourceUrl"]
        .as_str()
        .or_else(|| track["url"].as_str())
        .or_else(|| snapshot["sourceUrl"].as_str());
    if let Some(url) = url.filter(|value| !value.is_empty()) {
        result.insert("xesam:url".into(), into_owned(url.to_string()));
    } else if let Some(path) = track["localPath"]
        .as_str()
        .or_else(|| snapshot["localPath"].as_str())
    {
        if let Ok(url) = url::Url::from_file_path(path) {
            result.insert("xesam:url".into(), into_owned(url.to_string()));
        }
    }
    let duration = duration_seconds(snapshot);
    if duration > 0.0 {
        result.insert(
            "mpris:length".into(),
            into_owned((duration * 1_000_000.0).round() as i64),
        );
    }
    result
}

fn cached_artwork_url(path: Option<&str>, artwork_dir: Option<&Path>) -> Option<String> {
    let path = Path::new(path?);
    let root = artwork_dir?.canonicalize().ok()?;
    let canonical = path.canonicalize().ok()?;
    if !canonical.is_file() || !canonical.starts_with(root) {
        return None;
    }
    let metadata = canonical.metadata().ok()?;
    if metadata.len() == 0 || metadata.len() > 8 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(&canonical).ok()?;
    if crate::media::artwork_extension(&bytes).is_err() {
        return None;
    }
    url::Url::from_file_path(canonical)
        .ok()
        .map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_playback_state_and_repeat_modes_to_mpris_values() {
        assert_eq!(playback_status(&json!({"status":"playing"})), "Playing");
        assert_eq!(playback_status(&json!({"status":"paused"})), "Paused");
        assert_eq!(playback_status(&json!({})), "Stopped");
        assert_eq!(
            playback_status(&json!({"status":"idle","trackId":"old"})),
            "Stopped"
        );
        assert_eq!(loop_status(&json!({"repeat":"all"})), "Playlist");
        assert_eq!(loop_status(&json!({"repeat":"one"})), "Track");
        assert_eq!(loop_status(&json!({"repeat":"off"})), "None");
    }

    #[test]
    fn stopped_track_retains_known_duration_and_live_duration_takes_priority() {
        assert_eq!(
            duration_seconds(&json!({"duration":0,"track":{"durationSeconds":211}})),
            211.0
        );
        assert_eq!(
            duration_seconds(&json!({"duration":205,"track":{"durationSeconds":211}})),
            205.0
        );
    }

    #[test]
    fn metadata_has_mpris_types_and_microsecond_duration() {
        let metadata = metadata(
            &json!({
                "trackId":"track 1", "position":2.5, "durationSeconds":187.25,
                "track":{"title":"Song","artist":"Artist","album":"Record","artworkUrl":"https://example.test/a.jpg","sourceUrl":"file:///music/a.flac"}
            }),
            None,
        );
        assert_eq!(
            String::try_from(metadata["xesam:title"].try_clone().unwrap()).unwrap(),
            "Song"
        );
        assert_eq!(
            i64::try_from(&metadata["mpris:length"]).unwrap(),
            187_250_000
        );
        let path =
            OwnedObjectPath::try_from(metadata["mpris:trackid"].try_clone().unwrap()).unwrap();
        assert!(path.as_str().starts_with("/org/mpris/MediaPlayer2/track/"));
    }

    #[test]
    fn position_and_volume_values_are_bounded() {
        let snapshot = json!({"position":-2.0,"durationSeconds":42.0,"volume":4.0});
        assert_eq!(position_seconds(&snapshot), 0.0);
        assert_eq!(duration_seconds(&snapshot), 42.0);
        assert_eq!(snapshot["volume"].as_f64().unwrap().clamp(0.0, 1.0), 1.0);
    }

    #[test]
    fn open_uri_only_accepts_parseable_file_and_web_uris() {
        assert!(has_supported_uri_scheme("file:///music/song.flac"));
        assert!(has_supported_uri_scheme(
            "https://soundcloud.com/artist/track"
        ));
        assert!(!has_supported_uri_scheme("javascript:alert(1)"));
        assert!(!has_supported_uri_scheme("https://"));
    }

    #[test]
    fn cached_artwork_is_exposed_as_a_confined_file_uri() {
        let temp = tempfile::tempdir().unwrap();
        let art_dir = temp.path().join("artwork");
        std::fs::create_dir_all(&art_dir).unwrap();
        let image = art_dir.join("album cover.jpg");
        std::fs::write(&image, [0xff, 0xd8, 0xff, 0]).unwrap();
        let outside = temp.path().join("outside.jpg");
        std::fs::write(&outside, [0xff, 0xd8, 0xff, 0]).unwrap();
        let uri = cached_artwork_url(image.to_str(), Some(&art_dir)).unwrap();
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("album%20cover.jpg"));
        assert!(cached_artwork_url(outside.to_str(), Some(&art_dir)).is_none());
        assert!(!has_supported_uri_scheme(
            "asset://localhost/album%20cover.jpg"
        ));
    }

    #[test]
    fn uncached_public_artwork_remains_a_remote_mpris_url() {
        let metadata = metadata(
            &json!({"track":{"artworkUrl":"https://cdn.example.test/cover.jpg"}}),
            None,
        );
        assert_eq!(
            String::try_from(metadata["mpris:artUrl"].try_clone().unwrap()).unwrap(),
            "https://cdn.example.test/cover.jpg"
        );
        let no_art = super::metadata(
            &json!({"track":{"artworkUrl":"asset://localhost/cover.jpg"}}),
            None,
        );
        assert!(!no_art.contains_key("mpris:artUrl"));
    }

    #[test]
    fn playing_position_advances_but_never_exceeds_duration() {
        let mut cache =
            PlaybackCache::new(json!({"status":"playing","position":40.0,"durationSeconds":42.0}));
        cache.updated_at = Instant::now() - Duration::from_secs(5);
        assert_eq!(position_seconds(&cache.current_snapshot()), 42.0);
        cache.update(json!({"status":"paused","position":40.0,"durationSeconds":42.0}));
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(position_seconds(&cache.current_snapshot()), 40.0);
    }
}
