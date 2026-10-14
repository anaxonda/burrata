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
const FALLBACK_SYNC: Duration = Duration::from_secs(15);
const DEBOUNCE: Duration = Duration::from_millis(450);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

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
    pub fn start(app: AppHandle, store: Arc<Store>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .map_err(|error| format!("Could not create playlist filesystem watcher: {error}"))?;

        let settings = store.settings()?;
        let initial_roots = effective_watch_roots(&settings);
        let mut watched = HashSet::new();
        let mut warnings = Vec::new();
        watch_roots(&mut watcher, &initial_roots, &mut watched, &mut warnings);

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("playlist-watcher".into())
            .spawn(move || {
                worker_loop(app, store, watcher, rx, watched, warnings, thread_stop);
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
    stop: Arc<AtomicBool>,
) {
    let mut last_settings_refresh = Instant::now();

    let mut pending_event: Option<Instant> = None;

    // Always reconcile once at startup.
    emit_sync(&app, &store, &watcher_warnings);
    let mut last_fallback_sync = Instant::now();

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now.duration_since(last_settings_refresh) >= SETTINGS_REFRESH {
            refresh_roots(&store, &mut watcher, &mut watched, &mut watcher_warnings);
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
                if is_relevant_event(&event, &mirror_directories_from_store(&store)) {
                    pending_event = Some(Instant::now());
                }
            }
            Ok(Err(error)) => watcher_warnings.push(format!("Playlist watcher: {error}")),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        // Drain already queued events too, resetting the quiet interval for relevant changes.
        loop {
            match rx.try_recv() {
                Ok(Ok(event)) => {
                    if is_relevant_event(&event, &mirror_directories_from_store(&store)) {
                        pending_event = Some(Instant::now());
                    }
                }
                Ok(Err(error)) => watcher_warnings.push(format!("Playlist watcher: {error}")),
                Err(_) => break,
            }
        }

        let now = Instant::now();
        let should_sync_for_event =
            pending_event.is_some_and(|last| now.duration_since(last) >= DEBOUNCE);
        let should_fallback_sync = now.duration_since(last_fallback_sync) >= FALLBACK_SYNC;
        if should_sync_for_event || should_fallback_sync {
            emit_sync(&app, &store, &watcher_warnings);
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
    warnings: &mut Vec<String>,
) {
    let roots = match store.settings() {
        Ok(settings) => effective_watch_roots(&settings),
        Err(error) => {
            warnings.push(format!(
                "Could not refresh playlist watch settings: {error}"
            ));
            return;
        }
    };
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
            Err(error) => warnings.push(format!(
                "Could not watch playlist directory {}: {error}",
                root.display()
            )),
        }
    }
}

fn emit_sync(app: &AppHandle, store: &Store, watcher_warnings: &[String]) {
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
    let _ = app.emit("playlist-sync", payload);
}

fn mirror_directories_from_store(store: &Store) -> Vec<PathBuf> {
    store
        .settings()
        .ok()
        .and_then(|settings| mirror_directory(&settings))
        .into_iter()
        .collect()
}

fn mirror_directory(settings: &Settings) -> Option<PathBuf> {
    let music = settings.music_directory.trim();
    (!music.is_empty()).then(|| PathBuf::from(music).join("Playlists"))
}

/// Roots are explicit playlist folders, the configured music folder, and ~/Music.
/// Canonical aliases and nested roots collapse to one recursive watcher.
fn effective_watch_roots(settings: &Settings) -> Vec<PathBuf> {
    let mut candidates = settings
        .playlist_directories
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if !settings.music_directory.trim().is_empty() {
        candidates.push(PathBuf::from(&settings.music_directory));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Music"));
    }
    deduplicate_roots(candidates)
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
        let mut audio = Event::new(EventKind::Create(CreateKind::File));
        audio.paths.push(PathBuf::from("/music/track.flac"));
        assert!(!is_relevant_event(&audio, &[]));

        let mut folder = Event::new(EventKind::Remove(RemoveKind::Folder));
        folder.paths.push(PathBuf::from("/music/old-set"));
        assert!(is_relevant_event(&folder, &[]));
    }
}
