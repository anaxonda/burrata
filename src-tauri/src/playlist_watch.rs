//! Background filesystem watcher for local M3U/M3U8 playlists.
//!
//! Filesystem events are only a prompt to rescan. A periodic sync remains enabled for
//! network filesystems and platforms that do not report all changes reliably.

use crate::{models::Settings, playlist_sync, storage::Store};
use notify::{event::ModifyKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

const SETTINGS_REFRESH: Duration = Duration::from_secs(5);
const FALLBACK_SYNC: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_millis(450);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_DRAINED_EVENTS: usize = 512;
const MAX_WATCHER_WARNINGS: usize = 32;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncEvent {
    ok: bool,
    status: Option<playlist_sync::PlaylistSyncStatus>,
    error: Option<String>,
    watcher_warnings: Vec<String>,
}

/// Owns the synchronization thread. Dropping it requests shutdown and waits for its short
/// receive timeout to finish, so the file watcher is always released promptly.
pub struct PlaylistWatcher {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl PlaylistWatcher {
    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Acquire)
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
    }

    pub fn start(app: AppHandle, store: Arc<Store>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .map_err(|error| format!("Could not create playlist filesystem watcher: {error}"))?;

        let settings = store.settings()?;
        let initial_roots = effective_watch_roots(&settings);
        let mirror_directories = mirror_directories_for_settings(&settings);
        let mut watched = HashSet::new();
        let mut warnings = Vec::new();
        watch_roots(&mut watcher, &initial_roots, &mut watched, &mut warnings);

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("playlist-watcher".into())
            .spawn(move || {
                worker_loop(
                    app,
                    store,
                    watcher,
                    rx,
                    watched,
                    warnings,
                    mirror_directories,
                    thread_stop,
                );
            })
            .map_err(|error| format!("Could not start playlist watcher thread: {error}"))?;

        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for PlaylistWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    app: AppHandle,
    store: Arc<Store>,
    mut watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    mut watched: HashSet<PathBuf>,
    mut watcher_warnings: Vec<String>,
    mut mirror_directories: Vec<PathBuf>,
    stop: Arc<AtomicBool>,
) {
    let mut last_settings_refresh = Instant::now();

    let mut pending_event: Option<Instant> = None;

    // Always reconcile once at startup.
    let mut last_sync_payload = None;
    emit_sync(&app, &store, &watcher_warnings, &mut last_sync_payload);
    let mut last_fallback_sync = Instant::now();

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now.duration_since(last_settings_refresh) >= SETTINGS_REFRESH {
            refresh_roots(
                &store,
                &mut watcher,
                &mut watched,
                &mut mirror_directories,
                &mut watcher_warnings,
            );
            last_settings_refresh = now;
        }

        let mut timeout = POLL_INTERVAL;
        if let Some(last) = pending_event {
            timeout = timeout.min(DEBOUNCE.saturating_sub(now.saturating_duration_since(last)));
        }
        timeout =
            timeout.min(SETTINGS_REFRESH.saturating_sub(now.duration_since(last_settings_refresh)));
        timeout = timeout.min(FALLBACK_SYNC.saturating_sub(now.duration_since(last_fallback_sync)));

        match rx.recv_timeout(timeout.max(Duration::from_millis(1))) {
            Ok(Ok(event)) => {
                if is_relevant_event(&event, &mirror_directories) {
                    pending_event = Some(Instant::now());
                }
            }
            Ok(Err(error)) => {
                push_warning(&mut watcher_warnings, format!("Playlist watcher: {error}"))
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        // Drain already queued events too, resetting the quiet interval for relevant changes.
        for _ in 0..MAX_DRAINED_EVENTS {
            match rx.try_recv() {
                Ok(Ok(event)) => {
                    if is_relevant_event(&event, &mirror_directories) {
                        pending_event = Some(Instant::now());
                    }
                }
                Ok(Err(error)) => {
                    push_warning(&mut watcher_warnings, format!("Playlist watcher: {error}"))
                }
                Err(_) => break,
            }
        }

        let now = Instant::now();
        let should_sync_for_event =
            pending_event.is_some_and(|last| now.duration_since(last) >= DEBOUNCE);
        let should_fallback_sync = now.duration_since(last_fallback_sync) >= FALLBACK_SYNC;
        if should_sync_for_event || should_fallback_sync {
            emit_sync(&app, &store, &watcher_warnings, &mut last_sync_payload);
            pending_event = None;
            last_fallback_sync = Instant::now();
        }
    }
    drop(watcher);
}

fn refresh_roots(
    store: &Store,
    watcher: &mut RecommendedWatcher,
    watched: &mut HashSet<PathBuf>,
    mirror_directories: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    let settings = match store.settings() {
        Ok(settings) => settings,
        Err(error) => {
            push_warning(
                warnings,
                format!("Could not refresh playlist watch settings: {error}"),
            );
            return;
        }
    };
    let roots = effective_watch_roots(&settings);
    *mirror_directories = mirror_directories_for_settings(&settings);
    let next = roots.iter().cloned().collect::<HashSet<_>>();
    for root in watched.difference(&next).cloned().collect::<Vec<_>>() {
        let _ = watcher.unwatch(&root);
        watched.remove(&root);
    }
    watch_roots(watcher, &roots, watched, warnings);
}

fn watch_roots(
    watcher: &mut RecommendedWatcher,
    roots: &[PathBuf],
    watched: &mut HashSet<PathBuf>,
    warnings: &mut Vec<String>,
) {
    for root in roots {
        if watched.contains(root) {
            continue;
        }
        match watcher.watch(root, RecursiveMode::Recursive) {
            Ok(()) => {
                watched.insert(root.clone());
            }
            Err(error) => push_warning(
                warnings,
                format!(
                    "Could not watch playlist directory {}: {error}",
                    root.display()
                ),
            ),
        }
    }
}

fn emit_sync(
    app: &AppHandle,
    store: &Store,
    watcher_warnings: &[String],
    previous_payload: &mut Option<serde_json::Value>,
) {
    let payload = match store
        .settings()
        .and_then(|settings| playlist_sync::sync_playlists(store, &settings))
    {
        Ok(status) => SyncEvent {
            ok: true,
            status: Some(status),
            error: None,
            watcher_warnings: watcher_warnings.to_vec(),
        },
        Err(error) => SyncEvent {
            ok: false,
            status: None,
            error: Some(error),
            watcher_warnings: watcher_warnings.to_vec(),
        },
    };
    let Ok(value) = serde_json::to_value(payload) else {
        return;
    };
    if !should_emit_sync_payload(previous_payload.as_ref(), &value) {
        return;
    }
    *previous_payload = Some(value.clone());
    let _ = app.emit("playlist-sync", value);
}

fn mirror_directories_for_settings(settings: &Settings) -> Vec<PathBuf> {
    let music = settings.music_directory.trim();
    (!music.is_empty())
        .then(|| PathBuf::from(music).join("Playlists"))
        .into_iter()
        .collect()
}

fn should_emit_sync_payload(
    previous: Option<&serde_json::Value>,
    next: &serde_json::Value,
) -> bool {
    previous != Some(next)
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < MAX_WATCHER_WARNINGS && !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

/// Watch exactly the same validated roots that the playlist scanner reads.
/// Canonical aliases and nested roots collapse to one recursive watcher.
fn effective_watch_roots(settings: &Settings) -> Vec<PathBuf> {
    deduplicate_roots(crate::playlist_files::effective_directories(settings))
}

/// Canonicalize existing roots, normalize missing absolute paths, and remove children of
/// already watched roots. Missing roots are left for periodic settings refresh and fallback scan.
fn deduplicate_roots(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut roots = candidates
        .into_iter()
        .filter_map(|path| {
            let canonical = std::fs::canonicalize(&path).ok().filter(|p| p.is_dir())?;
            Some(canonical)
        })
        .collect::<Vec<_>>();
    roots.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    let mut result = Vec::new();
    for root in roots {
        if !result
            .iter()
            .any(|parent: &PathBuf| root.starts_with(parent))
        {
            result.push(root);
        }
    }
    result
}

/// Ignore immutable archives and unrelated files; directory create/remove/rename events are relevant
/// because they can add or remove playlist files below a recursively watched root.
fn is_relevant_event(event: &Event, mirror_directories: &[PathBuf]) -> bool {
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
    ) {
        return false;
    }
    let is_rename = matches!(event.kind, EventKind::Modify(ModifyKind::Name(_)));
    event.paths.iter().any(|path| {
        if mirror_directories
            .iter()
            .any(|mirror| path.starts_with(mirror.join("Archives")))
        {
            return false;
        }
        if matches!(
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("m3u" | "m3u8")
        ) {
            return true;
        }
        let no_extension = path.extension().is_none();
        no_extension
            && (path.is_dir()
                || matches!(event.kind, EventKind::Create(_) | EventKind::Remove(_))
                || is_rename)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};

    #[test]
    fn deduplicates_aliases_and_nested_roots() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("playlists");
        std::fs::create_dir(&nested).unwrap();
        let roots = deduplicate_roots(vec![
            directory.path().to_path_buf(),
            nested,
            directory.path().join("."),
        ]);
        assert_eq!(
            roots,
            vec![std::fs::canonicalize(directory.path()).unwrap()]
        );
    }

    #[test]
    fn event_filter_accepts_editable_mirrors_but_skips_archives_and_audio() {
        let mirror = PathBuf::from("/music/Playlists");
        let mut playlist = Event::new(EventKind::Create(CreateKind::File));
        playlist.paths.push(PathBuf::from("/music/set.m3u8"));
        assert!(is_relevant_event(&playlist, &[]));

        let mut mirror_event = Event::new(EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Content,
        )));
        mirror_event
            .paths
            .push(PathBuf::from("/music/Playlists/Liked Songs.m3u8"));
        assert!(is_relevant_event(
            &mirror_event,
            std::slice::from_ref(&mirror)
        ));

        let mut archive = Event::new(EventKind::Create(CreateKind::File));
        archive
            .paths
            .push(PathBuf::from("/music/Playlists/Archives/Spotify/old.m3u8"));
        assert!(!is_relevant_event(&archive, std::slice::from_ref(&mirror)));
        let mut temp = Event::new(EventKind::Create(CreateKind::File));
        temp.paths
            .push(PathBuf::from("/music/Playlists/set.123.tmp"));
        assert!(
            !is_relevant_event(&temp, std::slice::from_ref(&mirror)),
            "atomic mirror temp files must not trigger a rescan"
        );
        let mut audio = Event::new(EventKind::Create(CreateKind::File));
        audio.paths.push(PathBuf::from("/music/track.flac"));
        assert!(!is_relevant_event(&audio, &[]));

        let mut folder = Event::new(EventKind::Remove(RemoveKind::Folder));
        folder.paths.push(PathBuf::from("/music/old-set"));
        assert!(is_relevant_event(&folder, &[]));
    }

    #[test]
    fn mirror_roots_are_cached_from_settings_and_sync_events_skip_unchanged_payloads() {
        let settings = Settings {
            music_directory: "/music".into(),
            ..Settings::default()
        };
        assert_eq!(
            mirror_directories_for_settings(&settings),
            vec![PathBuf::from("/music/Playlists")]
        );
        let first = serde_json::json!({"ok":true,"status":{"scanned":2}});
        assert!(should_emit_sync_payload(None, &first));
        assert!(!should_emit_sync_payload(Some(&first), &first));
        assert!(should_emit_sync_payload(
            Some(&first),
            &serde_json::json!({"ok":true,"status":{"scanned":3}})
        ));
        assert_eq!(FALLBACK_SYNC, Duration::from_secs(60));
        assert_eq!(MAX_DRAINED_EVENTS, 512);
    }

    #[test]
    fn watcher_uses_the_shared_configured_roots_without_a_home_music_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("configured");
        let music = temp.path().join("music");
        std::fs::create_dir_all(&configured).unwrap();
        std::fs::create_dir_all(&music).unwrap();
        let settings = Settings {
            playlist_directories: vec![configured.to_string_lossy().into_owned()],
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };

        assert_eq!(
            effective_watch_roots(&settings),
            vec![
                std::fs::canonicalize(&configured).unwrap(),
                std::fs::canonicalize(&music).unwrap(),
            ]
        );
    }
}
