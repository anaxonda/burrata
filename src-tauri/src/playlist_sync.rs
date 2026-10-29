//! Local playlist synchronization and stable M3U mirrors.
//!
//! The source files remain user-owned. This module records canonical source bindings,
//! keeps immutable content snapshots in the application database, and writes app-owned
//! mirrors atomically under `Music/Playlists`.
use crate::{
    models::{Candidate, Settings, Track},
    playlist_files,
    storage::Store,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
// The portable serializer caps its body at MAX_BYTES. History mirror rendering
// adds this fixed marker and default playlist-name suffix afterward.
const HISTORY_MIRROR_HEADER_OVERHEAD: u64 =
    ("#BURRATA-MIRROR\n#PLAYLIST:Listening History\n").len() as u64;
const MAX_HISTORY_MIRROR_BYTES: u64 = MAX_BYTES + HISTORY_MIRROR_HEADER_OVERHEAD;
const MAX_ENTRIES: usize = 10_000;
const HISTORY_MIRROR_OPTIONS_KEY: &str = "playlistHistoryMirror.v1";
static PLAYLIST_SYNC_LOCK: Mutex<()> = Mutex::new(());
static PLAYLIST_EXPORT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMirrorOptions {
    pub schema_version: u32,
    #[serde(default)]
    pub recent_days: Option<u32>,
}

pub fn get_history_mirror_options(store: &Store) -> Result<HistoryMirrorOptions> {
    let Some(value) = store.get_json_preference(HISTORY_MIRROR_OPTIONS_KEY)? else {
        return Ok(HistoryMirrorOptions {
            schema_version: 1,
            recent_days: None,
        });
    };
    let options: HistoryMirrorOptions =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    validate_history_mirror_options(&options)?;
    Ok(options)
}

pub fn set_history_mirror_options(
    store: &Store,
    options: HistoryMirrorOptions,
) -> Result<HistoryMirrorOptions> {
    let _library_write = store.try_library_write()?;
    validate_history_mirror_options(&options)?;
    let mut options = options;
    options.schema_version = 1;
    store.set_json_preference(
        HISTORY_MIRROR_OPTIONS_KEY,
        &serde_json::to_value(&options).map_err(|error| error.to_string())?,
    )?;
    Ok(options)
}

fn validate_history_mirror_options(options: &HistoryMirrorOptions) -> Result<()> {
    if options.schema_version != 0 && options.schema_version != 1 {
        return Err("Unsupported history mirror options version".into());
    }
    if options
        .recent_days
        .is_some_and(|days| !(1..=3650).contains(&days))
    {
        return Err("Recent history scope must be between 1 and 3,650 days".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NamedExportIndexEntry {
    filename: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSyncStatus {
    pub scanned: usize,
    pub synchronized: usize,
    #[serde(default)]
    pub migrated_generated_imports: usize,
    #[serde(default)]
    pub repaired_track_metadata: usize,
    pub warnings: Vec<String>,
    pub mirrors_directory: Option<String>,
    pub sources: Vec<PlaylistSourceStatus>,
    #[serde(default)]
    pub paused_mirrors: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSourceStatus {
    pub path: String,
    pub name: String,
    pub entry_count: usize,
    pub state: String,
    pub warning: Option<String>,
    pub collection_id: Option<String>,
    pub revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixxxMissingTrack {
    pub id: String,
    pub artist: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixxxPlaylistExport {
    pub path: String,
    pub exported: usize,
    pub missing: Vec<MixxxMissingTrack>,
    pub total: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SyncState {
    sources: HashMap<String, SourceState>,
    #[serde(default)]
    mirror_hashes: HashMap<String, String>,
    #[serde(default)]
    mirror_paths: HashMap<String, String>,
    #[serde(default, skip_serializing)]
    mirror_snapshots: HashMap<String, Vec<SourceSnapshot>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    mirror_snapshot_refs: HashMap<String, Vec<SnapshotReference>>,
    #[serde(default)]
    protected_copies: HashMap<String, String>,
    /// Full-backup source/mirror history is deliberately inert after restore.
    /// It contains only portable snapshots and basename relink hints; keeping it
    /// here prevents ordinary runtime state writes from discarding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup_projection: Option<serde_json::Value>,
    /// Restored state must not start publishing mirrors until the user explicitly
    /// resumes synchronization on this device.
    #[serde(default)]
    writes_paused: bool,
    #[serde(skip)]
    preserve_mirrors: HashSet<String>,
    #[serde(skip)]
    changed_sources: HashSet<String>,
    #[serde(skip)]
    paused_mirrors: HashSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotReference {
    key: String,
    revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SourceState {
    collection_id: Option<String>,
    personal_collection_id: Option<String>,
    original_name: Option<String>,
    last_source_hash: Option<String>,
    last_mirror_hash: Option<String>,
    snapshots: Vec<SourceSnapshot>,
}

fn source_reconciliation_target_excluded(
    policy: &crate::playlist_mirror_options::PlaylistMirrorPolicy,
    source: &SourceState,
) -> bool {
    [
        source.collection_id.as_deref(),
        source.personal_collection_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|id| {
        policy
            .playlists
            .get(id)
            .is_some_and(|options| !options.included)
    })
}

fn mapped_personal_collection_id<'a>(state: &'a SyncState, collection_id: &str) -> Option<&'a str> {
    state
        .sources
        .values()
        .find(|source| source.collection_id.as_deref() == Some(collection_id))
        .and_then(|source| source.personal_collection_id.as_deref())
        .or_else(|| {
            state
                .protected_copies
                .get(collection_id)
                .map(String::as_str)
        })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSnapshot {
    pub revision: String,
    pub captured_at: String,
    pub contents: String,
}

/// Scan configured playlist roots and synchronize readable playlist files to one-time imports.
/// Errors are reported per source so one unavailable directory does not hide the others.
pub fn sync_playlists(store: &Store, settings: &Settings) -> Result<PlaylistSyncStatus> {
    sync_incremental(store, settings, None, false)
}

pub fn sync_playlists_with_history(
    store: &Store,
    settings: &Settings,
    history: &crate::playback_history::PlaybackHistory,
) -> Result<PlaylistSyncStatus> {
    sync_incremental(store, settings, Some(history), false)
}

/// Explicit user reconciliation also verifies bodies on filesystems with weak timestamps.
pub fn force_sync_playlists_with_history(
    store: &Store,
    settings: &Settings,
    history: &crate::playback_history::PlaybackHistory,
) -> Result<PlaylistSyncStatus> {
    sync_incremental(store, settings, Some(history), true)
}

fn sync_incremental(
    store: &Store,
    settings: &Settings,
    history: Option<&crate::playback_history::PlaybackHistory>,
    force: bool,
) -> Result<PlaylistSyncStatus> {
    if !settings.playlist_mirroring_enabled {
        return Ok(PlaylistSyncStatus::default());
    }
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    if force {
        *store
            .playlist_incremental
            .lock()
            .map_err(|e| e.to_string())? = Default::default();
    }
    let started = std::time::Instant::now();
    let revisions = store.playlist_sync_revisions()?;
    let history_revision = history.map(|h| h.mirror_revision()).transpose()?;
    let mut roots = playlist_files::effective_directories(settings);
    let mirror_root = mirrors_dir(settings);
    if let Some(root) = &mirror_root {
        roots.push(root.clone());
    }
    roots.sort();
    roots.dedup();
    let archive_root = mirror_root.as_ref().map(|root| root.join("Archives"));
    let files = crate::playlist_incremental::fingerprint(&roots, archive_root.as_deref())?;
    let token = format!(
        "{}:{}:{:?}:{}:{:?}:{:?}",
        revisions.generation,
        revisions.external,
        history_revision,
        files,
        roots,
        chrono::Utc::now().date_naive()
    );
    {
        let mut cache = store
            .playlist_incremental
            .lock()
            .map_err(|e| e.to_string())?;
        // Periodic body verification remains a safety net for lost events and network filesystems.
        if cache
            .verified_at
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(900))
        {
            cache.pass = None;
            cache.mirrors.clear();
            cache.verified_at = Some(started);
        }
        if let Some((previous, status)) = &cache.pass {
            if previous == &token {
                let mut status = status.clone();
                cache.noops += 1;
                status.synchronized = 0;
                status.repaired_track_metadata = 0;
                status.migrated_generated_imports = 0;
                if std::env::var_os("BURRATA_PERF_TRACE").is_some() {
                    eprintln!(
                        "[perf] playlist_sync.noop elapsed_ms={}",
                        started.elapsed().as_millis()
                    );
                }
                return Ok(status);
            }
        }
    }
    let scan = playlist_files::scan_local_playlists(settings)?;
    let status = match reconcile_scan(store, settings, scan, history) {
        Ok(status) => status,
        Err(error) => {
            // A failed pass may have created archives not yet committed to state.
            // Do not let partial cache entries skip their reconciliation on retry.
            *store
                .playlist_incremental
                .lock()
                .map_err(|e| e.to_string())? = Default::default();
            return Err(error);
        }
    };
    if store.playlist_sync_revisions()?.generation == revisions.generation
        && history.map(|h| h.mirror_revision()).transpose()? == history_revision
    {
        // Keep the *input* file token: concurrent edits can never be mistaken for our own writes.
        store
            .playlist_incremental
            .lock()
            .map_err(|e| e.to_string())?
            .pass = Some((token, status.clone()));
    }
    if std::env::var_os("BURRATA_PERF_TRACE").is_some() {
        eprintln!(
            "[perf] playlist_sync.total elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }
    Ok(status)
}

fn sync_playlists_with_scan(
    store: &Store,
    settings: &Settings,
    scan: playlist_files::LocalPlaylistScan,
) -> Result<PlaylistSyncStatus> {
    sync_playlists_with_scan_and_history(store, settings, scan, None)
}

fn sync_playlists_with_scan_and_history(
    store: &Store,
    settings: &Settings,
    scan: playlist_files::LocalPlaylistScan,
    history: Option<&crate::playback_history::PlaybackHistory>,
) -> Result<PlaylistSyncStatus> {
    if !settings.playlist_mirroring_enabled {
        return Ok(PlaylistSyncStatus::default());
    }
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    reconcile_scan(store, settings, scan, history)
}

fn reconcile_scan(
    store: &Store,
    settings: &Settings,
    scan: playlist_files::LocalPlaylistScan,
    history: Option<&crate::playback_history::PlaybackHistory>,
) -> Result<PlaylistSyncStatus> {
    let perf_trace = std::env::var_os("BURRATA_PERF_TRACE").is_some();
    let reconcile_started = std::time::Instant::now();
    let mut status = PlaylistSyncStatus::default();
    status.warnings.extend(scan.warnings);
    let state_started = std::time::Instant::now();
    let mut state = read_runtime_state(store)?;
    let mirror_policy = crate::playlist_mirror_options::get_playlist_mirror_policy(store)?;
    if state.writes_paused {
        status.warnings.push(restored_pause_message());
        return Ok(status);
    }
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.read_state sources={} elapsed_ms={}",
            state.sources.len(),
            state_started.elapsed().as_millis()
        );
    }
    let migration_started = std::time::Instant::now();
    status.migrated_generated_imports = migrate_generated_imports(store, settings, &mut state)?;
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.migration migrated={} elapsed_ms={}",
            status.migrated_generated_imports,
            migration_started.elapsed().as_millis()
        );
    }
    let mut legacy_mirror_paths = state
        .mirror_paths
        .values()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<HashSet<_>>();
    if state.mirror_hashes.contains_key("favorites") {
        if let Some(path) = mirrors_dir(settings).map(|dir| dir.join("Liked Songs.m3u8")) {
            if let Ok(path) = fs::canonicalize(path) {
                legacy_mirror_paths.insert(path);
            }
        }
    }
    let files = scan
        .files
        .into_iter()
        .filter(|file| {
            fs::canonicalize(&file.path)
                .map(|path| !legacy_mirror_paths.contains(&path))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    status.scanned = files.len();
    state.preserve_mirrors.clear();
    state.changed_sources.clear();
    state.paused_mirrors.clear();
    let mut present_sources = HashSet::new();
    let source_count = files.len();
    let source_started = std::time::Instant::now();
    for file in files {
        let canonical = match fs::canonicalize(&file.path) {
            Ok(p) => p,
            Err(e) => {
                status.warnings.push(format!("{}: {e}", file.path));
                continue;
            }
        };
        let key = canonical.to_string_lossy().into_owned();
        present_sources.insert(key.clone());
        let read = read_playlist(&canonical);
        let mut entry_count = 0;
        let (state_name, warning, revision) = match read {
            Err(message) => ("unavailable".to_owned(), Some(message), None),
            Ok(contents) => {
                let hash = digest(contents.as_bytes());
                entry_count = count_entries(&contents);
                let mut source_changed = false;
                if let Some(source) = state.sources.get_mut(&key) {
                    if source.last_source_hash.as_deref() != Some(&hash) {
                        source_changed = source.collection_id.is_some();
                        source.snapshots.push(SourceSnapshot {
                            revision: hash.clone(),
                            captured_at: chrono::Utc::now().to_rfc3339(),
                            contents: contents.clone(),
                        });
                        source.last_source_hash = Some(hash.clone());
                    }
                } else {
                    state.sources.insert(
                        key.clone(),
                        SourceState {
                            collection_id: None,
                            personal_collection_id: None,
                            original_name: Some(file.name.clone()),
                            last_source_hash: Some(hash.clone()),
                            last_mirror_hash: None,
                            snapshots: vec![SourceSnapshot {
                                revision: hash.clone(),
                                captured_at: chrono::Utc::now().to_rfc3339(),
                                contents: contents.clone(),
                            }],
                        },
                    );
                }
                if source_changed {
                    state.changed_sources.insert(key.clone());
                }
                let mut import_warning = None;
                if entry_count > 0
                    && entry_count <= MAX_ENTRIES
                    && !source_changed
                    && state.sources.get(&key).is_some_and(|source| {
                        source.collection_id.is_none()
                            && !source_reconciliation_target_excluded(&mirror_policy, source)
                    })
                {
                    match playlist_files::import_local_playlist(settings, store, &key) {
                        Ok(imported) => {
                            if let Some(source) = state.sources.get_mut(&key) {
                                source.collection_id = Some(imported.collection.id.clone());
                                source.original_name = Some(imported.collection.name.clone());
                            }
                            import_warning = imported.warning;
                        }
                        Err(error) => {
                            import_warning = Some(format!("Automatic import failed: {error}"));
                        }
                    }
                }
                // A same-revision M3U may already be bound to placeholder URL
                // tracks imported before plain artist/title comments were parsed.
                // Repair those records in place; changed source revisions remain
                // archived for review and never mutate the bound collection here.
                if !source_changed {
                    if let Some(source) = state.sources.get(&key) {
                        let collection_id = source.collection_id.as_deref();
                        if let Some(collection_id) = collection_id.filter(|_| {
                            !source_reconciliation_target_excluded(&mirror_policy, source)
                        }) {
                            status.repaired_track_metadata +=
                                playlist_files::repair_placeholder_url_metadata(
                                    store,
                                    collection_id,
                                    &contents,
                                )?;
                        }
                    }
                }
                let bound_id = state
                    .sources
                    .get(&key)
                    .and_then(|source| source.collection_id.as_ref());
                let bound_collection_deleted = if let Some(id) = bound_id {
                    !store
                        .list_collections()?
                        .iter()
                        .any(|collection| &collection.id == id)
                } else {
                    false
                };
                let source = state.sources.get(&key).unwrap();
                let warning = if entry_count > MAX_ENTRIES {
                    Some(format!("Playlist exceeds the {MAX_ENTRIES} entry limit"))
                } else if bound_collection_deleted {
                    Some("Collection was deleted; the source binding is retained and will not recreate it".into())
                } else if entry_count == 0 {
                    None
                } else if source_changed {
                    Some("Source contents changed; the new revision was archived for review".into())
                } else if source.collection_id.is_none() {
                    import_warning.or_else(|| {
                        Some("Playlist has not been imported into a collection yet".into())
                    })
                } else {
                    import_warning
                };
                (
                    if bound_collection_deleted {
                        "deleted"
                    } else if entry_count == 0 {
                        "empty"
                    } else if warning.is_some() {
                        "needsReview"
                    } else {
                        "bound"
                    }
                    .to_owned(),
                    warning,
                    Some(hash),
                )
            }
        };
        if let Some(w) = &warning {
            status.warnings.push(format!("{}: {w}", file.path));
        }
        let source = state.sources.get(&key);
        status.sources.push(PlaylistSourceStatus {
            path: key,
            name: file.name,
            entry_count,
            state: state_name,
            warning,
            collection_id: source.and_then(|s| s.collection_id.clone()),
            revision,
        });
    }
    for (path, source) in &state.sources {
        if !present_sources.contains(path) && source.collection_id.is_some() {
            let warning = "Source file is missing; collection binding is paused".to_owned();
            status.warnings.push(format!("{path}: {warning}"));
            status.sources.push(PlaylistSourceStatus {
                path: path.clone(),
                name: source
                    .original_name
                    .clone()
                    .unwrap_or_else(|| "Missing playlist".into()),
                entry_count: source
                    .snapshots
                    .last()
                    .map(|snapshot| count_entries(&snapshot.contents))
                    .unwrap_or(0),
                state: "paused".into(),
                warning: Some(warning),
                collection_id: source.collection_id.clone(),
                revision: source.last_source_hash.clone(),
            });
        }
    }
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.source_reconcile sources={} elapsed_ms={}",
            source_count,
            source_started.elapsed().as_millis()
        );
    }
    // Missing paths are retained as paused bindings: a temporary disconnect never deletes
    // the corresponding collection or its snapshots.
    let persist_started = std::time::Instant::now();
    persist_state(store, &state)?;
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.persist_state pass=before_mirrors elapsed_ms={}",
            persist_started.elapsed().as_millis()
        );
    }
    let mirror_started = std::time::Instant::now();
    if state.writes_paused {
        status.warnings.push(
            "Playlist mirror writes are paused after restore; review and explicitly resume them on this device".into(),
        );
    } else {
        status.mirrors_directory =
            write_mirrors(store, settings, &mut state, &mut status.warnings, history)?;
    }
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.mirror_write elapsed_ms={}",
            mirror_started.elapsed().as_millis()
        );
    }
    status.paused_mirrors = state.paused_mirrors.iter().cloned().collect();
    status.paused_mirrors.sort_unstable();
    let persist_started = std::time::Instant::now();
    persist_state(store, &state)?;
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.persist_state pass=after_mirrors elapsed_ms={}",
            persist_started.elapsed().as_millis()
        );
    }
    status.synchronized = status.sources.iter().filter(|s| s.state == "bound").count();
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.reconcile_counts sources={} collections_bound={} migrated={} repaired={} elapsed_ms={}",
            status.sources.len(),
            status.synchronized,
            status.migrated_generated_imports,
            status.repaired_track_metadata,
            reconcile_started.elapsed().as_millis()
        );
    }
    Ok(status)
}

/// Detach old imports of Burrata-owned mirror/archive files so they are not treated as
/// user sources. Soft-delete only a provably untouched auto-imported collection.
fn migrate_generated_imports(
    store: &Store,
    settings: &Settings,
    state: &mut SyncState,
) -> Result<usize> {
    if settings.music_directory.trim().is_empty() {
        return Ok(0);
    }
    let mirror_root = Path::new(&settings.music_directory).join("Playlists");
    let mirror_root = match fs::canonicalize(mirror_root) {
        Ok(path) => path,
        Err(_) => return Ok(0),
    };
    let mirror_policy = crate::playlist_mirror_options::get_playlist_mirror_policy(store)?;
    let mut recorded_mirror_paths = state
        .mirror_paths
        .values()
        .map(PathBuf::from)
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect::<HashSet<_>>();
    if state.mirror_hashes.contains_key("favorites") {
        let favorite_path = mirror_root.join("Liked Songs.m3u8");
        recorded_mirror_paths.insert(fs::canonicalize(&favorite_path).unwrap_or(favorite_path));
    }
    let generated_paths = state
        .sources
        .iter()
        .filter(|(_, source)| source.collection_id.is_some())
        .map(|(path, _)| path)
        .filter(|source| {
            let path = Path::new(source);
            let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            recorded_mirror_paths.contains(&canonical)
                || (path.starts_with(&mirror_root)
                    && playlist_files::is_burrata_generated_playlist(path, &mirror_root))
        })
        .filter(|source| {
            state
                .sources
                .get(*source)
                .and_then(|source| source.collection_id.as_deref())
                .and_then(|id| mirror_policy.playlists.get(id))
                .is_none_or(|options| options.included)
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut migrated = 0;
    for path in generated_paths {
        let Some(source) = state.sources.get(&path).cloned() else {
            continue;
        };
        if let Some(collection_id) = source.collection_id.as_deref() {
            if generated_import_is_unchanged(store, settings, state, &path, collection_id, &source)
            {
                // Soft deletion preserves collection membership and every referenced audio asset.
                if store
                    .list_collections()?
                    .iter()
                    .any(|collection| collection.id == collection_id)
                {
                    store.delete_collection(collection_id)?;
                }
            }
        }
        if let Some(source) = state.sources.get_mut(&path) {
            source.collection_id = None;
            source.personal_collection_id = None;
        }
        migrated += 1;
    }
    Ok(migrated)
}

fn generated_import_is_unchanged(
    store: &Store,
    settings: &Settings,
    state: &SyncState,
    source_path: &str,
    collection_id: &str,
    source: &SourceState,
) -> bool {
    if source.personal_collection_id.is_some()
        || state.protected_copies.contains_key(collection_id)
        || state
            .protected_copies
            .values()
            .any(|id| id == collection_id)
        || state.sources.iter().any(|(path, other)| {
            path != source_path && other.collection_id.as_deref() == Some(collection_id)
        })
        || store
            .is_browser_source_collection(collection_id)
            .unwrap_or(true)
    {
        return false;
    }
    let Some(original_name) = source.original_name.as_deref() else {
        return false;
    };
    if Path::new(source_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        != Some(original_name)
    {
        return false;
    }
    let Some(snapshot) = source.snapshots.last() else {
        return false;
    };
    if source.last_source_hash.as_deref() != Some(&snapshot.revision)
        || digest(snapshot.contents.as_bytes()) != snapshot.revision
    {
        return false;
    }
    let Ok(collections) = store.list_collections() else {
        return false;
    };
    let Some(collection) = collections.iter().find(|item| item.id == collection_id) else {
        return false;
    };
    if collection.name != original_name {
        return false;
    }
    let Ok((expected_tracks, _, _)) = playlist_files::preview_playlist_tracks(
        settings,
        store,
        Path::new(source_path),
        &snapshot.contents,
    ) else {
        return false;
    };
    let Ok(current_tracks) = store.list_tracks("collection", Some(collection_id)) else {
        return false;
    };
    expected_tracks
        .iter()
        .map(|track| &track.id)
        .eq(current_tracks.iter().map(|track| &track.id))
}

/// Recreate a deliberately deleted mirror from its current database contents and resume sync.
pub fn resume_playlist_mirror(store: &Store, settings: &Settings, id: &str) -> Result<String> {
    require_playlist_mirroring_enabled(settings)?;
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let dir = mirrors_dir(settings).ok_or("Music directory is not configured")?;
    let mut state = read_state(store)?;
    if state.writes_paused {
        return Err(restored_pause_message().into());
    }
    let (path, name, tracks) = if id == "favorites" {
        let tracks = store.list_tracks("favorites", None)?;
        let relative_dir = mirror_relative_directory(store, &state, id, &tracks)?;
        (
            dir.join(relative_dir).join("Liked Songs.m3u8"),
            "Liked Songs".to_owned(),
            tracks,
        )
    } else {
        let collection = store
            .list_collections()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or("Collection not found")?;
        let tracks = store.list_tracks("collection", Some(id))?;
        let relative_dir = mirror_relative_directory(store, &state, id, &tracks)?;
        let path = dir.join(relative_dir).join(format!("{id}.m3u8"));
        (path, collection.name, tracks)
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body =
        render_managed_playlist_m3u(store, id, &name, &tracks, path.parent().unwrap_or(&dir))?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(format!(
                    "{}: mirror destination is not a regular file; review or move it before restoring",
                    path.display()
                ));
            }
            let current = fs::read_to_string(&path).map_err(|error| {
                format!(
                    "{}: cannot read the existing mirror safely ({error}); review or move it before restoring",
                    path.display()
                )
            })?;
            let baseline_matches = state
                .mirror_hashes
                .get(id)
                .is_some_and(|baseline| digest(current.as_bytes()) == *baseline);
            if !baseline_matches && current != body {
                return Err(format!(
                    "{}: contains external edits; review or move it before restoring the mirror",
                    path.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "{}: cannot inspect the existing mirror safely ({error}); review or move it before restoring",
                path.display()
            ));
        }
    }
    atomic_write(&path, body.as_bytes())?;
    let section_keys = state
        .mirror_paths
        .keys()
        .filter(|key| key.starts_with(&format!("section:{id}:")))
        .cloned()
        .collect::<Vec<_>>();
    for key in section_keys {
        if let Some(section_path) = state.mirror_paths.get(&key).map(PathBuf::from) {
            if let Ok(contents) = fs::read_to_string(&section_path) {
                archive_mirror(
                    &mut state,
                    &dir,
                    "Sections",
                    &key.replace(':', "__"),
                    &contents,
                )?;
                fs::remove_file(section_path).map_err(|error| error.to_string())?;
            }
        }
        state.mirror_paths.remove(&key);
        state.mirror_hashes.remove(&key);
        state.paused_mirrors.remove(&key);
        state.preserve_mirrors.remove(&key);
    }
    let mut warnings = Vec::new();
    write_section_mirrors(
        store,
        &mut state,
        id,
        &name,
        &tracks,
        path.parent().unwrap_or(&dir),
        &dir,
        &mut warnings,
    )?;
    state.paused_mirrors.remove(id);
    state.preserve_mirrors.remove(id);
    state
        .mirror_paths
        .insert(id.to_owned(), path.to_string_lossy().into_owned());
    state
        .mirror_hashes
        .insert(id.to_owned(), digest(body.as_bytes()));
    persist_state(store, &state)?;
    Ok(path.to_string_lossy().into_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistMirrorMigrationPreview {
    pub id: String,
    pub path: String,
    pub source_hash: String,
    pub already_portable: bool,
    pub search_link_count: usize,
    pub entry_count: usize,
    pub warnings: Vec<String>,
}

/// Preview conversion of a legacy mirror. Applying it requires the exact reviewed
/// file hash and a match with the saved mirror baseline, so editor changes survive.
pub fn preview_playlist_mirror_migration(
    store: &Store,
    settings: &Settings,
    id: &str,
) -> Result<PlaylistMirrorMigrationPreview> {
    let state = read_state(store)?;
    let path = state
        .mirror_paths
        .get(id)
        .map(PathBuf::from)
        .or_else(|| mirrors_dir(settings).map(|dir| dir.join(format!("{id}.m3u8"))))
        .ok_or("Music directory is not configured")?;
    let text = fs::read_to_string(&path).map_err(|error| format!("Cannot read mirror: {error}"))?;
    let parsed = crate::portable_playlist::parse(&text).ok();
    let entry_count = parsed.as_ref().map_or_else(
        || {
            text.lines()
                .filter(|line| line.trim().starts_with("#EXTINF:"))
                .count()
        },
        |playlist| playlist.entries.len(),
    );
    let search_link_count = text
        .lines()
        .filter(|line| line.trim().starts_with("ytdl://ytsearch"))
        .count();
    Ok(PlaylistMirrorMigrationPreview {
        id: id.to_owned(),
        path: path.to_string_lossy().into_owned(),
        source_hash: digest(text.as_bytes()),
        already_portable: crate::portable_playlist::is_portable_playlist(&text),
        search_link_count,
        entry_count,
        warnings: if search_link_count > 0 {
            vec!["Legacy search links are inert metadata and will become identity placeholders unless explicitly opted in".into()]
        } else {
            Vec::new()
        },
    })
}

pub fn migrate_playlist_mirror_v1(
    store: &Store,
    id: &str,
    expected_source_hash: &str,
) -> Result<String> {
    let settings = store.settings()?;
    require_playlist_mirroring_enabled(&settings)?;
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let mut state = read_state(store)?;
    if state.writes_paused {
        return Err(restored_pause_message().into());
    }
    if state.paused_mirrors.contains(id) {
        return Err("Mirror is paused; explicitly resume it before migration".into());
    }
    let path = state
        .mirror_paths
        .get(id)
        .map(PathBuf::from)
        .ok_or("Mirror has no saved path binding")?;
    let old = fs::read_to_string(&path).map_err(|error| format!("Cannot read mirror: {error}"))?;
    let current_hash = digest(old.as_bytes());
    if current_hash != expected_source_hash
        || state
            .mirror_hashes
            .get(id)
            .is_some_and(|baseline| baseline != &current_hash)
    {
        return Err(
            "Mirror changed since preview or has unsaved external edits; it was preserved".into(),
        );
    }
    if crate::portable_playlist::is_portable_playlist(&old) {
        return Ok(path.to_string_lossy().into_owned());
    }
    let (name, tracks) = if id == "favorites" {
        (
            "Liked Songs".to_owned(),
            store.list_tracks("favorites", None)?,
        )
    } else {
        let collection = store
            .list_collections()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or("Collection not found")?;
        (collection.name, store.list_tracks("collection", Some(id))?)
    };
    let body = render_managed_playlist_m3u(
        store,
        id,
        &name,
        &tracks,
        path.parent().unwrap_or(Path::new(".")),
    )?;
    archive_mirror(
        &mut state,
        &path.parent().unwrap_or(Path::new(".")),
        "Migration",
        id,
        &old,
    )?;
    atomic_write(&path, body.as_bytes())?;
    state
        .mirror_hashes
        .insert(id.to_owned(), digest(body.as_bytes()));
    persist_state(store, &state)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Explicitly recreate the one-way history mirror after a deletion or edit pause.
pub fn resume_history_mirror(
    store: &Store,
    history: &crate::playback_history::PlaybackHistory,
    settings: &Settings,
) -> Result<String> {
    require_playlist_mirroring_enabled(settings)?;
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let dir = mirrors_dir(settings).ok_or("Music directory is not configured")?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let mut state = read_state(store)?;
    if state.writes_paused {
        return Err(restored_pause_message().into());
    }
    let old_keys = state
        .mirror_paths
        .keys()
        .filter(|key| *key == "history" || key.starts_with("history:"))
        .cloned()
        .collect::<Vec<_>>();
    for key in old_keys {
        if let Some(path) = state.mirror_paths.get(&key).map(PathBuf::from) {
            if let Ok(contents) = fs::read_to_string(&path) {
                archive_mirror(
                    &mut state,
                    &dir,
                    "History",
                    &key.replace(':', "__"),
                    &contents,
                )?;
                fs::remove_file(path).map_err(|error| error.to_string())?;
            }
        }
        state.mirror_paths.remove(&key);
        state.mirror_hashes.remove(&key);
        state.paused_mirrors.remove(&key);
        state.preserve_mirrors.remove(&key);
    }
    state.paused_mirrors.remove("history");
    state.preserve_mirrors.remove("history");
    let mut warnings = Vec::new();
    write_history_mirror(store, history, &dir, &mut state, &mut warnings)?;
    let path = state
        .mirror_paths
        .get("history")
        .or_else(|| {
            state
                .mirror_paths
                .iter()
                .find(|(key, _)| key.starts_with("history:"))
                .map(|(_, path)| path)
        })
        .cloned()
        .ok_or_else(|| warnings.join("; ").trim().to_owned())?;
    persist_state(store, &state)?;
    Ok(path)
}

/// Import a discovered source only once. Reopening its canonical path returns the original
/// collection binding, including after the source file has temporarily disappeared.
pub fn import_discovered_playlist(
    store: &Store,
    settings: &Settings,
    path: &str,
) -> Result<playlist_files::LocalPlaylistImport> {
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let canonical = fs::canonicalize(path).map_err(|_| "Playlist file is unavailable")?;
    let key = canonical.to_string_lossy().into_owned();
    let mut state = read_state(store)?;
    if let Some(id) = state
        .sources
        .get(&key)
        .and_then(|s| s.collection_id.clone())
    {
        let collection = store
            .list_collections()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or("Playlist collection binding is missing")?;
        return Ok(playlist_files::LocalPlaylistImport {
            imported_count: collection.track_count as usize,
            unmatched_count: 0,
            collection,
            warning: None,
        });
    }
    let imported = playlist_files::import_local_playlist(settings, store, &key)?;
    let contents = read_playlist(&canonical)?;
    let hash = digest(contents.as_bytes());
    let source = state.sources.entry(key).or_default();
    source.collection_id = Some(imported.collection.id.clone());
    source.original_name = Some(imported.collection.name.clone());
    source.last_source_hash = Some(hash.clone());
    if !source.snapshots.iter().any(|s| s.revision == hash) {
        source.snapshots.push(SourceSnapshot {
            revision: hash,
            captured_at: chrono::Utc::now().to_rfc3339(),
            contents,
        });
    }
    persist_state(store, &state)?;
    Ok(imported)
}

/// Explicitly refresh a changed source into a new collection version, keeping the old
/// collection and every prior snapshot intact. The source binding advances only after import succeeds.
pub fn refresh_discovered_playlist(
    store: &Store,
    settings: &Settings,
    path: &str,
) -> Result<playlist_files::LocalPlaylistImport> {
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let canonical = fs::canonicalize(path).map_err(|_| "Playlist file is unavailable")?;
    let key = canonical.to_string_lossy().into_owned();
    let contents = read_playlist(&canonical)?;
    let hash = digest(contents.as_bytes());
    let mut state = read_state(store)?;
    let existing = state
        .sources
        .get(&key)
        .and_then(|source| source.collection_id.clone());
    if existing.is_none() {
        return Err("Import this playlist once before refreshing it".into());
    }
    let imported = playlist_files::import_local_playlist(settings, store, &key)?;
    let source = state.sources.entry(key).or_default();
    source.collection_id = Some(imported.collection.id.clone());
    source.personal_collection_id = None;
    source.original_name = Some(imported.collection.name.clone());
    source.last_source_hash = Some(hash.clone());
    if !source
        .snapshots
        .iter()
        .any(|snapshot| snapshot.revision == hash)
    {
        source.snapshots.push(SourceSnapshot {
            revision: hash,
            captured_at: chrono::Utc::now().to_rfc3339(),
            contents,
        });
    }
    persist_state(store, &state)?;
    Ok(imported)
}

/// Stable path used by the user-facing playlist export command.
pub fn export_playlist_path(
    store: &Store,
    settings: &Settings,
    collection_id: &str,
) -> Result<String> {
    let _library_write = store.try_library_write()?;
    let collection = store
        .list_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or("Collection not found")?;
    let tracks = current_collection_tracks(store, collection_id)?;
    let mirrors = mirrors_dir(settings).ok_or("Choose a music directory first")?;
    let export_root = mirrors.join("Exports");
    let body = if settings.playlist_mirroring_enabled {
        sync_playlists(store, settings)?;
        let state = read_state(store)?;
        let relative_dir = mirror_relative_directory(store, &state, collection_id, &tracks)?;
        let mirror_path = mirrors
            .join(relative_dir)
            .join(format!("{}.m3u8", collection.id));
        fs::read(&mirror_path).map_err(|e| format!("Cannot read playlist mirror: {e}"))?
    } else {
        render_managed_playlist_m3u(
            store,
            collection_id,
            &collection.name,
            &tracks,
            &export_root,
        )?
        .into_bytes()
    };
    let path = write_named_export(&export_root, &collection.id, &collection.name, &body)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Build an export DTO from current ordered playlist data and explicit hierarchy
/// metadata. Collection/provider IDs and raw provenance never become export IDs.
pub fn portable_playlist_input(
    store: &Store,
    collection_id: &str,
) -> Result<crate::portable_playlist::PlaylistInput> {
    portable_playlist_input_for_purpose(
        store,
        collection_id,
        crate::portable_playlist::PlaylistPurpose::LocalMirror,
    )
}

fn portable_playlist_input_for_purpose(
    store: &Store,
    collection_id: &str,
    purpose: crate::portable_playlist::PlaylistPurpose,
) -> Result<crate::portable_playlist::PlaylistInput> {
    if collection_id == "favorites" {
        let name = crate::sidebar_layout::load(store)
            .ok()
            .and_then(|layout| layout.names.get("favorites").cloned())
            .unwrap_or_else(|| "Liked Songs".into());
        let tracks = store.list_tracks("favorites", None)?;
        let mut input = crate::portable_playlist::PlaylistInput {
            id: "favorites".into(),
            name,
            folder_ancestry: crate::playlist_hierarchy::folder_ancestry(store, "favorites")?,
            sections: crate::playlist_hierarchy::sections(store, "favorites")?,
            entries: tracks
                .iter()
                .map(|track| crate::portable_playlist::PortableEntry {
                    track: crate::portable_playlist::PortableTrack::from_track(track),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        if purpose == crate::portable_playlist::PlaylistPurpose::LocalMirror {
            attach_local_mirror_resources(store, &mut input, &tracks)?;
        }
        return Ok(input);
    }
    let collection = store
        .list_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or("Playlist not found")?;
    let tracks = store.list_tracks("collection", Some(collection_id))?;
    let mut input =
        portable_playlist_input_for_tracks_base(store, collection_id, collection.name, &tracks)?;
    if purpose == crate::portable_playlist::PlaylistPurpose::LocalMirror {
        attach_local_mirror_resources(store, &mut input, &tracks)?;
    }
    Ok(input)
}

fn portable_playlist_input_for_tracks(
    store: &Store,
    collection_id: &str,
    name: String,
    tracks: &[Track],
) -> Result<crate::portable_playlist::PlaylistInput> {
    let mut input = portable_playlist_input_for_tracks_base(store, collection_id, name, tracks)?;
    attach_local_mirror_resources(store, &mut input, tracks)?;
    Ok(input)
}

fn portable_playlist_input_for_tracks_base(
    store: &Store,
    collection_id: &str,
    name: String,
    tracks: &[Track],
) -> Result<crate::portable_playlist::PlaylistInput> {
    let input = crate::portable_playlist::PlaylistInput {
        id: collection_id.to_owned(),
        name,
        folder_ancestry: crate::playlist_hierarchy::folder_ancestry(store, collection_id)?,
        sections: crate::playlist_hierarchy::sections(store, collection_id)?,
        entries: crate::playlist_hierarchy::portable_entries(store, collection_id, tracks)?,
        ..crate::portable_playlist::PlaylistInput::default()
    };
    Ok(input)
}

fn attach_local_mirror_resources(
    store: &Store,
    input: &mut crate::portable_playlist::PlaylistInput,
    tracks: &[Track],
) -> Result<()> {
    let music_directory = store.settings()?.music_directory;
    let directory = (!music_directory.trim().is_empty())
        .then(|| PathBuf::from(music_directory).join("Playlists"));
    attach_local_resources_at(store, input, tracks, directory.as_deref())
}

fn attach_local_resources_at(
    store: &Store,
    input: &mut crate::portable_playlist::PlaylistInput,
    tracks: &[Track],
    directory: Option<&Path>,
) -> Result<()> {
    let paths = tracks
        .iter()
        .filter_map(|track| track.local_path.clone())
        .collect::<Vec<_>>();
    let assets = store.registered_asset_paths(&paths)?;
    for (entry, track) in input.entries.iter_mut().zip(tracks) {
        if let (Some(directory), Some(path)) = (directory, track.local_path.as_deref()) {
            if assets.contains(path) {
                if let Ok(resource) = relative_path_resource(directory, Path::new(path)) {
                    entry.resolved_resource = Some(resource);
                }
            }
        }
        // A remembered audio candidate is an explicit user-selected playback
        // source. Keep it in local mirrors when there is no available local file.
        if entry.resolved_resource.is_none() {
            entry.resolved_resource = track
                .provenance
                .get("candidateUrl")
                .and_then(serde_json::Value::as_str)
                .filter(|url| url.starts_with("https://") && !url.chars().any(char::is_control))
                .map(str::to_owned);
        }
    }
    Ok(())
}

fn export_tracks(store: &Store, collection_id: &str) -> Result<Vec<Track>> {
    if collection_id == "favorites" {
        store.list_tracks("favorites", None)
    } else {
        store.list_tracks("collection", Some(collection_id))
    }
}

fn attach_local_export_resources(
    store: &Store,
    input: &mut crate::portable_playlist::PlaylistInput,
    tracks: &[Track],
    export_dir: &Path,
) {
    let Ok(assets) = store.list_assets() else {
        return;
    };
    for (entry, track) in input.entries.iter_mut().zip(tracks) {
        entry.resolved_resource = track
            .local_path
            .as_deref()
            .and_then(|path| assets.iter().find(|asset| asset.path == path))
            .and_then(|asset| relative_path_resource(export_dir, Path::new(&asset.path)).ok());
    }
}

pub fn preview_portable_playlist_export(
    store: &Store,
    collection_id: &str,
    options: &crate::portable_playlist::PlaylistExportOptions,
) -> Result<crate::portable_playlist::PlaylistExportPreview> {
    let input = portable_playlist_input_for_purpose(store, collection_id, options.purpose)?;
    crate::portable_playlist::preview_export(&input, options)
}

pub fn render_portable_playlist_export(
    store: &Store,
    collection_id: &str,
    options: &crate::portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    let input = portable_playlist_input_for_purpose(store, collection_id, options.purpose)?;
    render_portable_m3u(&input, options)
}

pub fn export_portable_playlist_path(
    store: &Store,
    collection_id: &str,
    options: &crate::portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    export_portable_playlist_path_with_fallback(store, collection_id, options, None)
}

pub fn export_portable_playlist_path_with_fallback(
    store: &Store,
    collection_id: &str,
    options: &crate::portable_playlist::PlaylistExportOptions,
    fallback_download_dir: Option<&Path>,
) -> Result<String> {
    let _library_write = store.try_library_write()?;
    let settings = store.settings()?;
    let export_dir = if let Some(mirrors) = mirrors_dir(&settings) {
        mirrors.join("Exports")
    } else if options.purpose == crate::portable_playlist::PlaylistPurpose::Share {
        fallback_download_dir
            .map(Path::to_path_buf)
            .ok_or("Choose a music directory or Downloads folder first")?
    } else {
        return Err("Choose a music directory first".into());
    };
    fs::create_dir_all(&export_dir).map_err(|error| error.to_string())?;
    let mut input = portable_playlist_input_for_purpose(store, collection_id, options.purpose)?;
    if options.purpose == crate::portable_playlist::PlaylistPurpose::LocalMirror {
        let tracks = export_tracks(store, collection_id)?;
        attach_local_export_resources(store, &mut input, &tracks, &export_dir);
    }
    let mut body = render_portable_m3u(&input, options)?;
    body = body.replacen("#EXTM3U\n", "#EXTM3U\n#BURRATA-EXPORT\n", 1);
    let path = write_named_export(&export_dir, collection_id, &input.name, body.as_bytes())?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn preview_history_export(
    history: &crate::playback_history::PlaybackHistory,
    options: &crate::portable_playlist::PlaylistExportOptions,
) -> Result<crate::portable_playlist::PlaylistExportPreview> {
    let input = history_playlist_input(history, None)?;
    crate::portable_playlist::preview_export(&input, options)
}

pub fn render_history_export(
    history: &crate::playback_history::PlaybackHistory,
    options: &crate::portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    let input = history_playlist_input(history, None)?;
    render_portable_m3u(&input, options)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePlaylistImportPreview {
    pub preview_id: String,
    pub source_hash: String,
    pub name: String,
    pub entry_count: usize,
    pub duplicate_occurrences: usize,
    pub exact_local_matches: usize,
    pub ambiguous_local_matches: usize,
    pub unresolved_entries: usize,
    pub provider_match_suggestions: usize,
    pub provider_ambiguous_entries: usize,
    pub provider_conflict_entries: usize,
    pub matching_level: crate::recording_match::MatchLevel,
    pub mode: PortablePlaylistImportMode,
    pub target_collection_id: Option<String>,
    pub target_name: Option<String>,
    pub target_entry_count: usize,
    pub target_fingerprint: Option<String>,
    pub projected_added_entries: usize,
    pub projected_skipped_duplicates: usize,
    pub folder_ancestry: Vec<String>,
    pub sections: Vec<PortablePlaylistSectionPreview>,
    pub entries: Vec<PortablePlaylistEntryPreview>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PortablePlaylistImportMode {
    #[default]
    Create,
    Update,
    Merge,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PortablePlaylistDuplicatePolicy {
    #[default]
    Keep,
    Skip,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePlaylistImportOptions {
    #[serde(default)]
    pub matching_level: crate::recording_match::MatchLevel,
    #[serde(default)]
    pub mode: PortablePlaylistImportMode,
    #[serde(default)]
    pub target_collection_id: Option<String>,
    #[serde(default)]
    pub duplicate_policy: PortablePlaylistDuplicatePolicy,
    #[serde(default)]
    pub destination_name: Option<String>,
    #[serde(default)]
    pub destination_folder_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePlaylistEntryPreview {
    pub entry_order: usize,
    pub order: usize,
    pub title: String,
    pub artists: Vec<String>,
    pub version: Option<String>,
    pub duration_seconds: Option<f64>,
    pub section_name: Option<String>,
    pub status: String,
    pub provider: Option<String>,
    pub provider_title: Option<String>,
    pub provider_artist: Option<String>,
    pub provider_version: Option<String>,
    pub provider_duration_seconds: Option<f64>,
    pub relationship: Option<String>,
    pub reason: Option<String>,
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub provider_options: Vec<PortablePlaylistProviderOption>,
    pub suggested_selection: Option<PortablePlaylistProviderSelection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePlaylistProviderOption {
    pub provider: String,
    pub source_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub version: Option<String>,
    pub duration_seconds: Option<f64>,
    pub relationship: String,
    pub action: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortablePlaylistProviderSelection {
    pub entry_order: usize,
    pub provider: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePlaylistSectionPreview {
    pub name: String,
    pub entry_count: usize,
}

/// Preview a share file without writing to the library, history, or favorites.
pub fn preview_portable_playlist_import(
    store: &Store,
    text: &str,
) -> Result<PortablePlaylistImportPreview> {
    let input = crate::portable_playlist::parse(text).map_err(|error| error.to_string())?;
    let (tracks, exact, ambiguous) = resolve_portable_import_tracks(store, &input)?;
    let unique_ids = tracks.iter().map(|track| &track.id).collect::<HashSet<_>>();
    let duplicate_occurrences = tracks.len().saturating_sub(unique_ids.len());
    let unresolved_entries = tracks
        .iter()
        .filter(|track| track.provider == "import")
        .count();
    let mut warnings = Vec::new();
    if unresolved_entries > 0 {
        warnings.push(format!(
            "{unresolved_entries} entries will retain unverified metadata and will not be downloaded"
        ));
    }
    if ambiguous > 0 {
        warnings.push(format!(
            "{ambiguous} entries match multiple local recordings and will remain unresolved"
        ));
    }
    Ok(PortablePlaylistImportPreview {
        preview_id: String::new(),
        source_hash: digest(text.as_bytes()),
        name: input.name,
        entry_count: tracks.len(),
        duplicate_occurrences,
        exact_local_matches: exact,
        ambiguous_local_matches: ambiguous,
        unresolved_entries,
        provider_match_suggestions: 0,
        provider_ambiguous_entries: 0,
        provider_conflict_entries: 0,
        matching_level: crate::recording_match::MatchLevel::Balanced,
        mode: PortablePlaylistImportMode::Create,
        target_collection_id: None,
        target_name: None,
        target_entry_count: 0,
        target_fingerprint: None,
        projected_added_entries: tracks.len(),
        projected_skipped_duplicates: 0,
        folder_ancestry: input
            .folder_ancestry
            .iter()
            .map(|folder| folder.name.clone())
            .collect(),
        sections: input
            .sections
            .iter()
            .map(|section| PortablePlaylistSectionPreview {
                name: section.name.clone(),
                entry_count: input
                    .entries
                    .iter()
                    .filter(|entry| entry.section_id.as_deref() == Some(section.id.as_str()))
                    .count(),
            })
            .collect(),
        entries: input
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| PortablePlaylistEntryPreview {
                entry_order: index + 1,
                order: index + 1,
                title: entry.track.title.clone(),
                artists: entry
                    .track
                    .artists
                    .iter()
                    .map(|artist| artist.name.clone())
                    .collect(),
                version: entry.track.version.clone(),
                duration_seconds: entry.track.duration_seconds,
                section_name: entry.section_id.as_deref().and_then(|id| {
                    input
                        .sections
                        .iter()
                        .find(|section| section.id == id)
                        .map(|section| section.name.clone())
                }),
                status: if tracks
                    .get(index)
                    .is_some_and(|track| track.provider != "import")
                {
                    "exactLocal"
                } else {
                    "unresolved"
                }
                .into(),
                provider: None,
                provider_title: None,
                provider_artist: None,
                provider_version: None,
                provider_duration_seconds: None,
                relationship: None,
                reason: None,
                conflicts: Vec::new(),
                provider_options: Vec::new(),
                suggested_selection: None,
            })
            .collect(),
        warnings,
    })
}

/// Review local assets first, then inspect configured streaming providers with
/// the shared recording resolver policy. Provider choices are bound by a fresh
/// lookup when applied; ephemeral URLs are never stored in preview state.
pub async fn preview_portable_playlist_import_with_options(
    store: &Store,
    acquisition: &crate::acquisition::AcquisitionManager,
    settings: &Settings,
    text: &str,
    options: PortablePlaylistImportOptions,
) -> Result<PortablePlaylistImportPreview> {
    if text.len() > 2 * 1024 * 1024 {
        return Err("Portable playlist imports are limited to 2 MB".into());
    }
    validate_portable_import_override(options.destination_name.as_deref(), 120)?;
    validate_portable_import_override(options.destination_folder_path.as_deref(), 2048)?;
    let input = crate::portable_playlist::parse(text).map_err(|error| error.to_string())?;
    let (tracks, exact, ambiguous) = resolve_portable_import_tracks(store, &input)?;
    let mut entries = input
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| PortablePlaylistEntryPreview {
            entry_order: index + 1,
            order: index + 1,
            title: entry.track.title.clone(),
            artists: entry
                .track
                .artists
                .iter()
                .map(|artist| artist.name.clone())
                .collect(),
            version: entry.track.version.clone(),
            duration_seconds: entry.track.duration_seconds,
            section_name: entry.section_id.as_deref().and_then(|id| {
                input
                    .sections
                    .iter()
                    .find(|section| section.id == id)
                    .map(|section| section.name.clone())
            }),
            status: if tracks
                .get(index)
                .is_some_and(|track| track.provider != "import")
            {
                "exactLocal".into()
            } else {
                "unresolved".into()
            },
            provider: None,
            provider_title: None,
            provider_artist: None,
            provider_version: None,
            provider_duration_seconds: None,
            relationship: None,
            reason: None,
            conflicts: Vec::new(),
            provider_options: Vec::new(),
            suggested_selection: None,
        })
        .collect::<Vec<_>>();
    let provider_ids = portable_import_providers(settings);
    let mut provider_matches = 0;
    let mut provider_ambiguous = 0;
    let mut provider_conflicts = 0;
    let mut searched = HashSet::new();
    let mut provider_failures = 0usize;
    if !provider_ids.is_empty() {
        for (index, entry) in input.entries.iter().enumerate() {
            if entries[index].status == "exactLocal" || !searched.insert(tracks[index].id.clone()) {
                continue;
            }
            // Bound latency and provider traffic for large files. Each repeated
            // recording shares the first occurrence's safe result summary.
            if searched.len() > 32 {
                break;
            }
            let requested = entry.track.to_track();
            let candidates = match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                acquisition.prefetch_preview_candidates(&requested, &provider_ids, settings),
            )
            .await
            {
                Ok(Ok(candidates)) => candidates,
                Ok(Err(_)) | Err(_) => {
                    provider_failures += 1;
                    Vec::new()
                }
            };
            let assessed = assess_portable_provider_candidates(
                store,
                &requested,
                candidates,
                options.matching_level,
            );
            let suggestions = assessed
                .iter()
                .filter(|(_, assessment)| {
                    assessment.action != crate::recording_match::MatchAction::Reject
                        && assessment.relationship
                            != crate::recording_match::MatchRelationship::Unresolved
                })
                .collect::<Vec<_>>();
            let row = &mut entries[index];
            row.provider_options = suggestions
                .iter()
                .filter_map(|(candidate, assessment)| {
                    let source_id = portable_provider_source_id(candidate)?;
                    Some(PortablePlaylistProviderOption {
                        provider: candidate.provider.clone(),
                        source_id,
                        title: safe_import_display(&candidate.title, 512),
                        artist: candidate
                            .uploader
                            .as_deref()
                            .map(|value| safe_import_display(value, 512)),
                        version: candidate
                            .source_metadata
                            .as_ref()
                            .and_then(|metadata| {
                                metadata
                                    .get("version")
                                    .or_else(|| metadata.get("versionTitle"))
                            })
                            .and_then(serde_json::Value::as_str)
                            .map(|value| safe_import_display(value, 256))
                            .filter(|value| !value.is_empty()),
                        duration_seconds: candidate.duration_seconds.filter(|value| {
                            value.is_finite() && *value > 0.0 && *value <= 604_800.0
                        }),
                        relationship: format!("{:?}", assessment.relationship),
                        action: format!("{:?}", assessment.action),
                        evidence: assessment
                            .reasons
                            .iter()
                            .map(|reason| format!("{:?}", reason))
                            .take(8)
                            .collect(),
                    })
                })
                .collect();
            if options.matching_level == crate::recording_match::MatchLevel::Exact
                && suggestions.len() == 1
                && suggestions[0].1.action == crate::recording_match::MatchAction::Accept
            {
                if let Some(source_id) = portable_provider_source_id(&suggestions[0].0) {
                    row.suggested_selection = Some(PortablePlaylistProviderSelection {
                        entry_order: row.entry_order,
                        provider: suggestions[0].0.provider.clone(),
                        source_id,
                    });
                }
            }
            if suggestions.len() == 1 {
                let (candidate, assessment) = suggestions[0];
                row.status = "providerSuggestion".into();
                row.provider = Some(candidate.provider.clone());
                row.provider_title = Some(safe_import_display(&candidate.title, 512));
                row.provider_artist = candidate
                    .uploader
                    .as_deref()
                    .map(|value| safe_import_display(value, 512));
                row.provider_duration_seconds = candidate
                    .duration_seconds
                    .filter(|value| value.is_finite() && *value > 0.0 && *value <= 604_800.0);
                row.relationship = Some(format!("{:?}", assessment.relationship));
                row.conflicts = Vec::new();
                row.reason = assessment
                    .reasons
                    .first()
                    .map(|reason| format!("{:?}", reason));
                provider_matches += 1;
            } else if suggestions.len() > 1 {
                row.status = "providerAmbiguous".into();
                row.reason =
                    Some("Multiple provider results satisfy the selected match policy".into());
                provider_ambiguous += 1;
            } else if !assessed.is_empty() {
                let conflicts = assessed
                    .iter()
                    .filter(|(_, assessment)| {
                        assessment.action == crate::recording_match::MatchAction::Reject
                    })
                    .collect::<Vec<_>>();
                if !conflicts.is_empty() {
                    row.status = "providerConflict".into();
                    row.conflicts = conflicts
                        .first()
                        .map(|(_, assessment)| {
                            assessment
                                .reasons
                                .iter()
                                .map(|reason| format!("{:?}", reason))
                                .take(8)
                                .collect()
                        })
                        .unwrap_or_default();
                    provider_conflicts += 1;
                }
            }
        }
    }
    // Replicated source occurrences carry the same evidence without another
    // provider lookup, while keeping one safe per-occurrence review row.
    let mut first_by_identity = HashMap::<String, usize>::new();
    for index in 0..input.entries.len() {
        let identity = tracks[index].id.clone();
        if let Some(first) = first_by_identity.get(&identity).copied() {
            if entries[index].status == "unresolved" {
                entries[index].status = entries[first].status.clone();
                entries[index].provider = entries[first].provider.clone();
                entries[index].provider_title = entries[first].provider_title.clone();
                entries[index].provider_artist = entries[first].provider_artist.clone();
                entries[index].provider_version = entries[first].provider_version.clone();
                entries[index].provider_duration_seconds = entries[first].provider_duration_seconds;
                entries[index].relationship = entries[first].relationship.clone();
                entries[index].reason = entries[first].reason.clone();
                entries[index].conflicts = entries[first].conflicts.clone();
                entries[index].provider_options = entries[first].provider_options.clone();
                entries[index].suggested_selection = entries[first]
                    .suggested_selection
                    .clone()
                    .map(|mut selection| {
                        selection.entry_order = entries[index].entry_order;
                        selection
                    });
            }
        } else {
            first_by_identity.insert(identity, index);
        }
    }
    provider_matches = entries
        .iter()
        .filter(|row| row.status == "providerSuggestion")
        .count();
    provider_ambiguous = entries
        .iter()
        .filter(|row| row.status == "providerAmbiguous")
        .count();
    provider_conflicts = entries
        .iter()
        .filter(|row| row.status == "providerConflict")
        .count();
    let target = portable_import_target(store, &options)?;
    let folder_ancestry = if let Some(path) = options.destination_folder_path.as_deref() {
        split_folder_names(path)
    } else if let Some(target) = target.as_ref() {
        collection_folder_ancestry(store, &target.0.id)?
    } else {
        input
            .folder_ancestry
            .iter()
            .map(|folder| folder.name.clone())
            .collect()
    };
    let source_hash = digest(text.as_bytes());
    let preview_id = uuid::Uuid::new_v4().to_string();
    let local_ambiguous = ambiguous;
    let unresolved_entries = entries
        .iter()
        .filter(|row| {
            matches!(
                row.status.as_str(),
                "unresolved" | "providerConflict" | "providerAmbiguous"
            )
        })
        .count();
    let mut projected_added = tracks.len();
    let mut skipped = 0;
    if options.mode == PortablePlaylistImportMode::Merge
        && options.duplicate_policy == PortablePlaylistDuplicatePolicy::Skip
    {
        let existing = store
            .list_tracks("collection", options.target_collection_id.as_deref())?
            .iter()
            .map(|track| track.id.clone())
            .collect::<HashSet<_>>();
        let destination_ids = existing;
        projected_added = 0;
        for track in &tracks {
            if destination_ids.contains(&track.id) {
                skipped += 1;
            } else {
                projected_added += 1;
            }
        }
    }
    let mut warnings = Vec::new();
    if unresolved_entries > 0 {
        warnings.push(format!("{unresolved_entries} entries remain unavailable or uncertain; review any provider choices before applying. Import does not download audio."));
    }
    if local_ambiguous > 0 {
        warnings.push(format!(
            "{local_ambiguous} entries match multiple local recordings"
        ));
    }
    if provider_failures > 0 {
        warnings.push(format!(
            "Provider lookup did not complete for {provider_failures} entries"
        ));
    }
    if searched.len() > 32 {
        warnings.push("Provider lookup was limited to 32 distinct unmatched recordings".into());
    }
    let preview = PortablePlaylistImportPreview {
        preview_id: preview_id.clone(),
        source_hash: source_hash.clone(),
        name: input.name.clone(),
        entry_count: tracks.len(),
        duplicate_occurrences: tracks.len().saturating_sub(
            tracks
                .iter()
                .map(|track| &track.id)
                .collect::<HashSet<_>>()
                .len(),
        ),
        exact_local_matches: exact,
        ambiguous_local_matches: ambiguous,
        unresolved_entries,
        provider_match_suggestions: provider_matches,
        provider_ambiguous_entries: provider_ambiguous,
        provider_conflict_entries: provider_conflicts,
        matching_level: options.matching_level,
        mode: options.mode,
        target_collection_id: target.as_ref().map(|target| target.0.id.clone()),
        target_name: target.as_ref().map(|target| target.0.name.clone()),
        target_entry_count: target.as_ref().map_or(0, |target| target.1),
        target_fingerprint: target.as_ref().map(|target| target.2.clone()),
        projected_added_entries: projected_added,
        projected_skipped_duplicates: skipped,
        folder_ancestry,
        sections: input
            .sections
            .iter()
            .map(|section| PortablePlaylistSectionPreview {
                name: section.name.clone(),
                entry_count: input
                    .entries
                    .iter()
                    .filter(|entry| entry.section_id.as_deref() == Some(section.id.as_str()))
                    .count(),
            })
            .collect(),
        entries,
        warnings,
    };
    let binding = serde_json::json!({
        "sourceHash": source_hash,
        "matchingLevel": options.matching_level,
        "mode": options.mode,
        "targetCollectionId": target.as_ref().map(|target| &target.0.id),
        "targetFingerprint": target.as_ref().map(|target| &target.2),
        "duplicatePolicy": options.duplicate_policy,
        "destinationName": options.destination_name,
        "destinationFolderPath": options.destination_folder_path,
        "providerOptions": preview.entries.iter().map(|entry| serde_json::json!({
            "entryOrder": entry.entry_order,
            "providerOptions": entry.provider_options.iter().map(|option| serde_json::json!({
                "provider": option.provider,
                "sourceId": option.source_id,
                "action": option.action,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    store.set_provider_cache(
        "portable-playlist-import-preview.v1",
        &preview_id,
        &binding,
        1800,
    )?;
    Ok(preview)
}

fn portable_import_providers(settings: &Settings) -> Vec<String> {
    let mut providers = Vec::new();
    for provider in std::iter::once(&settings.preferred_streaming_source).chain(
        settings
            .streaming_fallback_enabled
            .then_some(settings.streaming_fallback_sources.as_slice())
            .into_iter()
            .flatten(),
    ) {
        if matches!(provider.as_str(), "youtube" | "soundcloud" | "monochrome")
            && !providers.contains(provider)
        {
            providers.push(provider.clone());
        }
    }
    providers
}

fn assess_portable_provider_candidates(
    store: &Store,
    requested: &Track,
    candidates: Vec<Candidate>,
    matching_level: crate::recording_match::MatchLevel,
) -> Vec<(Candidate, crate::recording_match::MatchAssessment)> {
    candidates
        .into_iter()
        .map(|candidate| {
            let assessment = crate::recording_match_context::assess(
                store,
                requested,
                &candidate,
                matching_level,
                crate::recording_match::MatchStage::Search,
            );
            (candidate, assessment)
        })
        .collect()
}

fn safe_import_display(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn portable_provider_source_id(candidate: &Candidate) -> Option<String> {
    if !matches!(
        candidate.provider.as_str(),
        "youtube" | "soundcloud" | "monochrome"
    ) {
        return None;
    }
    let prefix = format!("{}:", candidate.provider);
    let source_id = candidate.id.strip_prefix(&prefix).unwrap_or(&candidate.id);
    if source_id.is_empty()
        || source_id.len() > 256
        || !source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some(source_id.to_owned())
}

fn portable_provider_selection_id(provider: &str, source_id: &str) -> Option<String> {
    if !matches!(provider, "youtube" | "soundcloud" | "monochrome")
        || source_id.is_empty()
        || source_id.len() > 256
        || !source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some(format!("{provider}:{source_id}"))
}

fn portable_candidate_matches_selection(
    candidate: &Candidate,
    provider: &str,
    source_id: &str,
) -> bool {
    candidate.provider == provider
        && portable_provider_source_id(candidate).as_deref() == Some(source_id)
}

/// Resolve only a previously reviewed portable playback identity. The durable
/// track stores a provider and opaque source ID, never an ephemeral URL.
pub async fn resolve_portable_playback_candidate(
    store: &Store,
    acquisition: &crate::acquisition::AcquisitionManager,
    track: &Track,
    settings: &Settings,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<Option<Candidate>> {
    let Some(source) = track.provenance.get("portablePlaybackSource") else {
        return Ok(None);
    };
    let provider = source["provider"]
        .as_str()
        .filter(|provider| matches!(*provider, "youtube" | "soundcloud" | "monochrome"))
        .ok_or("Saved portable playback provider is invalid")?;
    let source_id = source["sourceId"]
        .as_str()
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 256
                && id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
                })
        })
        .ok_or("Saved portable playback source ID is invalid")?;
    let matching_level: crate::recording_match::MatchLevel =
        serde_json::from_value(source["matchingLevel"].clone())
            .map_err(|_| "Saved portable playback matching policy is invalid")?;
    let source_hash = source["sourceHash"]
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("Saved portable playback source hash is invalid")?;
    let _ = source_hash;
    if !portable_import_providers(settings)
        .iter()
        .any(|configured| configured == provider)
    {
        return Err("The reviewed portable playback provider is no longer configured".into());
    }
    let candidates = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        acquisition.find_fresh_preview_candidates_cancelled(
            track,
            &[provider.to_owned()],
            settings,
            cancel.clone(),
        ),
    )
    .await
    .map_err(|_| "Portable playback source lookup timed out")??;
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let Some(candidate) = candidates
        .into_iter()
        .find(|candidate| portable_candidate_matches_selection(candidate, provider, source_id))
    else {
        return Err("The reviewed portable playback source is no longer available".into());
    };
    let assessment = crate::recording_match_context::assess(
        store,
        track,
        &candidate,
        matching_level,
        crate::recording_match::MatchStage::Search,
    );
    if assessment.action == crate::recording_match::MatchAction::Reject
        || assessment.relationship == crate::recording_match::MatchRelationship::Unresolved
    {
        return Err("Portable playback candidate no longer matches the reviewed track".into());
    }
    Ok(Some(candidate))
}

fn portable_import_target(
    store: &Store,
    options: &PortablePlaylistImportOptions,
) -> Result<Option<(crate::models::Collection, usize, String)>> {
    match options.mode {
        PortablePlaylistImportMode::Create => {
            if options.target_collection_id.is_some() {
                return Err("Create mode cannot target an existing playlist".into());
            }
            Ok(None)
        }
        PortablePlaylistImportMode::Update | PortablePlaylistImportMode::Merge => {
            let id = options
                .target_collection_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or("Choose an existing playlist for update or merge")?;
            if id.len() > 256 || ["favorites", "history", "downloads"].contains(&id) {
                return Err("Choose a regular named playlist".into());
            }
            let collection = store
                .list_collections()?
                .into_iter()
                .find(|collection| collection.id == id)
                .ok_or("Destination playlist was deleted or is unavailable")?;
            let sync_state = read_state(store)?;
            if sync_state
                .sources
                .values()
                .any(|source| source.collection_id.as_deref() == Some(id))
                || store.is_browser_source_collection(id)?
            {
                return Err("This playlist is a protected source; choose its personal copy or another playlist".into());
            }
            let favorites_id = store
                .get_json_preference("playlistPreferences.v1")?
                .and_then(|value| value["favoritesPlaylistId"].as_str().map(str::to_owned));
            if favorites_id.as_deref() == Some(id) {
                return Err("Choose a regular named playlist instead of Favorites".into());
            }
            let tracks = store.list_tracks("collection", Some(id))?;
            let hierarchy = crate::playlist_hierarchy::load(store)?;
            let placement = hierarchy
                .playlists
                .iter()
                .find(|playlist| playlist.collection_id == id);
            let fingerprint = digest(
                &serde_json::to_vec(&serde_json::json!({
                    "collectionId":&collection.id,
                    "name":&collection.name,
                    "tracks":tracks.iter().map(|track| &track.id).collect::<Vec<_>>(),
                    "placement":placement,
                }))
                .map_err(|error| error.to_string())?,
            );
            Ok(Some((collection, tracks.len(), fingerprint)))
        }
    }
}

fn collection_folder_ancestry(store: &Store, collection_id: &str) -> Result<Vec<String>> {
    let hierarchy = crate::playlist_hierarchy::load(store)?;
    let Some(mut folder_id) = hierarchy
        .playlists
        .iter()
        .find(|placement| placement.collection_id == collection_id)
        .and_then(|placement| placement.folder_id.clone())
    else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for _ in 0..32 {
        let Some(folder) = hierarchy
            .folders
            .iter()
            .find(|folder| folder.id == folder_id)
        else {
            return Err("Destination playlist has invalid folder placement".into());
        };
        result.push(folder.name.clone());
        let Some(parent) = folder.parent_id.clone() else {
            break;
        };
        folder_id = parent;
    }
    result.reverse();
    Ok(result)
}

fn validate_portable_import_override(value: Option<&str>, max_chars: usize) -> Result<()> {
    if value.is_some_and(|value| {
        value.trim().is_empty()
            || value.chars().count() > max_chars
            || value.chars().any(char::is_control)
    }) {
        return Err("Playlist destination override is invalid".into());
    }
    Ok(())
}

/// Apply reviewed playlist metadata, exact local matches, and provider identity
/// selections. No candidate URLs or audio downloads are persisted.
pub fn apply_portable_playlist_import(
    store: &Store,
    text: &str,
    preview_id: &str,
    expected_source_hash: &str,
    destination_name: Option<&str>,
    destination_folder_path: Option<&str>,
) -> Result<playlist_files::LocalPlaylistImport> {
    apply_portable_playlist_import_core(
        store,
        text,
        preview_id,
        expected_source_hash,
        destination_name,
        destination_folder_path,
        &HashMap::new(),
    )
}

/// Apply a preview after freshly resolving every explicitly reviewed provider
/// selection. Selected IDs are rechecked against the preview and matching
/// policy; candidates and URLs themselves are never trusted from the caller.
pub async fn apply_portable_playlist_import_with_providers(
    store: &Store,
    acquisition: &crate::acquisition::AcquisitionManager,
    settings: &Settings,
    text: &str,
    preview_id: &str,
    expected_source_hash: &str,
    destination_name: Option<&str>,
    destination_folder_path: Option<&str>,
    provider_selections: &[PortablePlaylistProviderSelection],
) -> Result<playlist_files::LocalPlaylistImport> {
    let binding = store
        .get_provider_cache("portable-playlist-import-preview.v1", preview_id)?
        .ok_or("Playlist preview expired; review the import again")?;
    if binding["sourceHash"].as_str() != Some(expected_source_hash)
        || digest(text.as_bytes()) != expected_source_hash
    {
        return Err("Playlist changed after preview; review the import again".into());
    }
    let input = crate::portable_playlist::parse(text).map_err(|error| error.to_string())?;
    let matching_level: crate::recording_match::MatchLevel =
        serde_json::from_value(binding["matchingLevel"].clone())
            .map_err(|_| "Playlist preview has an invalid matching policy")?;
    let (local_tracks, _, _) = resolve_portable_import_tracks(store, &input)?;
    let reviewed_options = binding["providerOptions"]
        .as_array()
        .ok_or("Playlist preview has no reviewed provider options")?;
    let configured_providers = portable_import_providers(settings);
    let mut selections_by_order = HashMap::new();
    let mut provider_sources = HashMap::new();
    let mut lookup_cache = HashMap::<(String, String, String), bool>::new();
    for selection in provider_selections {
        if selection.entry_order == 0
            || selection.entry_order > input.entries.len()
            || selections_by_order
                .insert(selection.entry_order, selection)
                .is_some()
        {
            return Err("Provider selections contain an invalid or repeated entry order".into());
        }
        if local_tracks[selection.entry_order - 1].provider != "import" {
            return Err("A provider selection cannot replace an exact local match".into());
        }
        if !configured_providers
            .iter()
            .any(|provider| provider == &selection.provider)
            || portable_provider_selection_id(&selection.provider, &selection.source_id).is_none()
        {
            return Err("Provider selection is invalid or no longer configured".into());
        }
        let row = reviewed_options
            .iter()
            .find(|row| row["entryOrder"].as_u64() == Some(selection.entry_order as u64))
            .ok_or("Selected provider option was not part of the reviewed preview")?;
        let option_was_reviewed = row["providerOptions"].as_array().is_some_and(|options| {
            options.iter().any(|option| {
                option["provider"].as_str() == Some(selection.provider.as_str())
                    && option["sourceId"].as_str() == Some(selection.source_id.as_str())
                    && option["action"].as_str() != Some("Reject")
            })
        });
        if !option_was_reviewed {
            return Err("Selected provider option was not part of the reviewed preview".into());
        }
        let track = &input.entries[selection.entry_order - 1].track;
        let lookup_key = (
            track.to_track().id,
            selection.provider.clone(),
            selection.source_id.clone(),
        );
        if !lookup_cache.contains_key(&lookup_key) {
            if lookup_cache.len() >= 32 {
                return Err(
                    "Provider selection validation is limited to 32 distinct recordings".into(),
                );
            }
            let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
            let candidates = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                acquisition.find_fresh_preview_candidates_cancelled(
                    &track.to_track(),
                    std::slice::from_ref(&selection.provider),
                    settings,
                    cancel,
                ),
            )
            .await
            .map_err(|_| "Provider selection validation timed out")??;
            let matched = candidates.into_iter().find(|candidate| {
                portable_candidate_matches_selection(
                    candidate,
                    &selection.provider,
                    &selection.source_id,
                )
            });
            let valid = if let Some(candidate) = matched {
                let assessment = crate::recording_match_context::assess(
                    store,
                    &track.to_track(),
                    &candidate,
                    matching_level,
                    crate::recording_match::MatchStage::Search,
                );
                assessment.action != crate::recording_match::MatchAction::Reject
                    && assessment.relationship
                        != crate::recording_match::MatchRelationship::Unresolved
            } else {
                false
            };
            lookup_cache.insert(lookup_key.clone(), valid);
        }
        if !lookup_cache.get(&lookup_key).copied().unwrap_or(false) {
            return Err("Selected provider candidate no longer matches the reviewed track".into());
        }
        provider_sources.insert(
            selection.entry_order - 1,
            serde_json::json!({
                "provider": selection.provider,
                "sourceId": selection.source_id,
                "matchingLevel": matching_level,
                "sourceHash": expected_source_hash,
            }),
        );
    }
    apply_portable_playlist_import_core(
        store,
        text,
        preview_id,
        expected_source_hash,
        destination_name,
        destination_folder_path,
        &provider_sources,
    )
}

fn apply_portable_playlist_import_core(
    store: &Store,
    text: &str,
    preview_id: &str,
    expected_source_hash: &str,
    destination_name: Option<&str>,
    destination_folder_path: Option<&str>,
    provider_sources: &HashMap<usize, serde_json::Value>,
) -> Result<playlist_files::LocalPlaylistImport> {
    let _library_write = store.try_library_write()?;
    let binding = store
        .get_provider_cache("portable-playlist-import-preview.v1", preview_id)?
        .ok_or("Playlist preview expired; review the import again")?;
    if digest(text.as_bytes()) != expected_source_hash
        || binding["sourceHash"].as_str() != Some(expected_source_hash)
        || binding["destinationName"] != serde_json::json!(destination_name)
        || binding["destinationFolderPath"] != serde_json::json!(destination_folder_path)
    {
        return Err("Playlist changed after preview; review the import again".into());
    }
    let input = crate::portable_playlist::parse(text).map_err(|error| error.to_string())?;
    let mode: PortablePlaylistImportMode = serde_json::from_value(binding["mode"].clone())
        .map_err(|_| "Playlist preview has an invalid import mode")?;
    let matching_level: crate::recording_match::MatchLevel =
        serde_json::from_value(binding["matchingLevel"].clone())
            .map_err(|_| "Playlist preview has an invalid matching policy")?;
    let duplicate_policy: PortablePlaylistDuplicatePolicy =
        serde_json::from_value(binding["duplicatePolicy"].clone())
            .map_err(|_| "Playlist preview has an invalid duplicate policy")?;
    let target_collection_id = binding["targetCollectionId"].as_str().map(str::to_owned);
    let options = PortablePlaylistImportOptions {
        matching_level,
        mode,
        target_collection_id,
        duplicate_policy,
        destination_name: destination_name.map(str::to_owned),
        destination_folder_path: destination_folder_path.map(str::to_owned),
    };
    validate_portable_import_override(destination_name, 120)?;
    validate_portable_import_override(destination_folder_path, 2048)?;
    let reviewed_target = portable_import_target(store, &options)?;
    if binding["targetFingerprint"].as_str()
        != reviewed_target.as_ref().map(|target| target.2.as_str())
    {
        return Err("Destination playlist changed after preview; review the import again".into());
    }
    let (mut resolved_tracks, exact, ambiguous) = resolve_portable_import_tracks(store, &input)?;
    // Check any pre-existing placeholder rows before mutating clones or collection state.
    store.validate_portable_sources_for_import(&resolved_tracks, provider_sources)?;
    for (index, source) in provider_sources {
        let Some(track) = resolved_tracks.get_mut(*index) else {
            return Err("Provider selection entry is outside the imported playlist".into());
        };
        track.provenance["portablePlaybackSource"] = source.clone();
    }
    let (collection_id, tracks, metadata_input) = match mode {
        PortablePlaylistImportMode::Create => {
            let name = destination_name
                .map(str::trim)
                .unwrap_or(input.name.as_str());
            let collection = store.create_collection(name)?;
            store.replace_playlist_entries_with_portable_sources(
                &collection.id,
                &resolved_tracks,
                &provider_sources,
            )?;
            (collection.id, resolved_tracks, Some(input.clone()))
        }
        PortablePlaylistImportMode::Update => {
            let target_id = options
                .target_collection_id
                .as_deref()
                .ok_or("Choose a playlist to update")?;
            let collection_id = ensure_editable_collection(store, target_id)?;
            if let Some(name) = destination_name.map(str::trim) {
                let current = store
                    .list_collections()?
                    .into_iter()
                    .find(|item| item.id == collection_id)
                    .ok_or("Destination playlist was deleted")?;
                if current.name != name {
                    store.rename_collection(&collection_id, name)?;
                }
            }
            store.replace_playlist_entries_with_portable_sources(
                &collection_id,
                &resolved_tracks,
                &provider_sources,
            )?;
            (collection_id, resolved_tracks, Some(input.clone()))
        }
        PortablePlaylistImportMode::Merge => {
            let target_id = options
                .target_collection_id
                .as_deref()
                .ok_or("Choose a playlist to merge into")?;
            let collection_id = ensure_editable_collection(store, target_id)?;
            let existing = store.list_tracks("collection", Some(&collection_id))?;
            let destination_ids = existing
                .iter()
                .map(|track| track.id.clone())
                .collect::<HashSet<_>>();
            if duplicate_policy == PortablePlaylistDuplicatePolicy::Skip
                && resolved_tracks.iter().enumerate().any(|(index, track)| {
                    provider_sources.contains_key(&index) && destination_ids.contains(&track.id)
                })
            {
                return Err(
                    "The reviewed provider source would be skipped by the merge duplicate policy; choose Append or Update to apply it"
                        .into(),
                );
            }
            if let Some(name) = destination_name.map(str::trim) {
                let current = store
                    .list_collections()?
                    .into_iter()
                    .find(|item| item.id == collection_id)
                    .ok_or("Destination playlist was deleted")?;
                if current.name != name {
                    store.rename_collection(&collection_id, name)?;
                }
            }
            let mut appended = Vec::new();
            let mut incoming_indices = Vec::new();
            for (index, track) in resolved_tracks.into_iter().enumerate() {
                if duplicate_policy == PortablePlaylistDuplicatePolicy::Skip
                    && destination_ids.contains(&track.id)
                {
                    continue;
                }
                appended.push(track);
                incoming_indices.push(index);
            }
            let appended_sources = incoming_indices
                .iter()
                .enumerate()
                .filter_map(|(appended_index, incoming_index)| {
                    provider_sources
                        .get(incoming_index)
                        .map(|source| (appended_index, source.clone()))
                })
                .collect::<HashMap<_, _>>();
            store.append_playlist_entries_with_portable_sources(
                &collection_id,
                &appended,
                &appended_sources,
            )?;
            let mut selected_input = input.clone();
            selected_input.entries = incoming_indices
                .iter()
                .map(|index| input.entries[*index].clone())
                .collect();
            (collection_id, appended, Some(selected_input))
        }
    };
    if mode == PortablePlaylistImportMode::Create {
        let folder_names = destination_folder_path
            .map(split_folder_names)
            .unwrap_or_else(|| {
                input
                    .folder_ancestry
                    .iter()
                    .map(|item| item.name.clone())
                    .collect()
            });
        crate::playlist_hierarchy::set_folder_ancestry(store, &collection_id, &folder_names)?;
    } else if let Some(path) = destination_folder_path {
        crate::playlist_hierarchy::set_folder_ancestry(
            store,
            &collection_id,
            &split_folder_names(path),
        )?;
    }
    if mode == PortablePlaylistImportMode::Merge {
        if !tracks.is_empty() {
            if let Some(metadata) = metadata_input.as_ref() {
                merge_portable_playlist_metadata(store, &collection_id, metadata)?;
            }
        }
    } else if let Some(metadata) = metadata_input.as_ref() {
        crate::playlist_hierarchy::import_portable_metadata(store, &collection_id, metadata)?;
    }
    let unmatched = tracks.len().saturating_sub(exact);
    let warning = (unmatched > 0 || ambiguous > 0).then(|| {
        format!("{unmatched} entries remain unverified; {ambiguous} local matches were ambiguous")
    });
    Ok(playlist_files::LocalPlaylistImport {
        collection: store
            .list_collections()?
            .into_iter()
            .find(|collection| collection.id == collection_id)
            .ok_or("Imported playlist could not be loaded")?,
        imported_count: tracks.len(),
        unmatched_count: unmatched,
        warning,
    })
}

fn split_folder_names(path: &str) -> Vec<String> {
    path.split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Merge imported annotations onto the appended occurrence tail while keeping
/// all existing section definitions and occurrence annotations intact.
fn merge_portable_playlist_metadata(
    store: &Store,
    collection_id: &str,
    input: &crate::portable_playlist::PlaylistInput,
) -> Result<()> {
    if input.entries.is_empty() {
        return Ok(());
    }
    let mut hierarchy = crate::playlist_hierarchy::load(store)?;
    let current_sections = hierarchy
        .playlists
        .iter()
        .find(|placement| placement.collection_id == collection_id)
        .map(|placement| {
            placement
                .sections
                .iter()
                .map(|section| crate::portable_playlist::NamedItem {
                    id: section.id.clone(),
                    name: section.name.clone(),
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut combined = current_sections.clone();
    for section in &input.sections {
        if !combined
            .iter()
            .any(|known| known.name.eq_ignore_ascii_case(&section.name))
        {
            combined.push(section.clone());
        }
    }
    crate::playlist_hierarchy::set_sections(store, collection_id, &combined)?;
    hierarchy = crate::playlist_hierarchy::load(store)?;
    let placement = hierarchy
        .playlists
        .iter_mut()
        .find(|placement| placement.collection_id == collection_id)
        .ok_or("Destination playlist placement not found")?;
    let section_map = input
        .sections
        .iter()
        .filter_map(|source| {
            placement
                .sections
                .iter()
                .find(|target| target.name.eq_ignore_ascii_case(&source.name))
                .map(|target| (source.id.as_str(), target.id.as_str()))
        })
        .collect::<HashMap<_, _>>();
    let ordered = store.list_playlist_occurrences(collection_id)?;
    if ordered.len() < input.entries.len() {
        return Err("Merged playlist occurrence count changed during metadata application".into());
    }
    let start = ordered.len() - input.entries.len();
    for (row, incoming) in ordered[start..].iter().zip(&input.entries) {
        let section_id = incoming
            .section_id
            .as_deref()
            .and_then(|source| section_map.get(source).copied())
            .map(str::to_owned);
        if let Some(existing) = placement
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.id == row.occurrence_id)
        {
            // append_playlist_entries synchronizes an empty overlay row inside
            // its transaction. Replace only that new row's importable fields;
            // existing target occurrences and their annotations stay intact.
            existing.section_id = section_id;
            existing.note = incoming.note.clone();
            existing.cue_points = incoming.cue_points.clone();
            existing.playback_range = incoming.playback_range.clone();
        } else {
            placement
                .occurrences
                .push(crate::playlist_hierarchy::OccurrenceMetadata {
                    id: row.occurrence_id.clone(),
                    section_id,
                    note: incoming.note.clone(),
                    cue_points: incoming.cue_points.clone(),
                    playback_range: incoming.playback_range.clone(),
                });
        }
    }
    crate::playlist_hierarchy::save(store, &hierarchy)?;
    Ok(())
}

fn resolve_portable_import_tracks(
    store: &Store,
    input: &crate::portable_playlist::PlaylistInput,
) -> Result<(Vec<Track>, usize, usize)> {
    let assets = store.list_assets()?;
    let mut tracks = Vec::with_capacity(input.entries.len());
    let mut exact = 0;
    let mut ambiguous = 0;
    for entry in &input.entries {
        let candidates = assets
            .iter()
            .filter(|asset| portable_identity_matches(&entry.track, &asset.track))
            .map(|asset| &asset.track)
            .fold(HashMap::<&str, &Track>::new(), |mut unique, track| {
                unique.entry(&track.id).or_insert(track);
                unique
            });
        if candidates.len() == 1 {
            let track = *candidates.values().next().expect("one exact match");
            exact += 1;
            tracks.push(track.clone());
        } else {
            if candidates.len() > 1 {
                ambiguous += 1;
            }
            tracks.push(entry.track.to_track());
        }
    }
    Ok((tracks, exact, ambiguous))
}

fn portable_identity_matches(
    portable: &crate::portable_playlist::PortableTrack,
    local: &Track,
) -> bool {
    let normalized = |value: &str| value.trim().to_lowercase();
    if normalized(&portable.title) != normalized(&local.title)
        || normalized(portable.version.as_deref().unwrap_or_default())
            != normalized(local.version.as_deref().unwrap_or_default())
    {
        return false;
    }
    let artist =
        portable
            .artists
            .iter()
            .enumerate()
            .fold(String::new(), |mut value, (index, credit)| {
                value.push_str(&credit.name);
                if let Some(join) = credit.join_phrase.as_deref() {
                    value.push_str(join);
                } else if index + 1 < portable.artists.len() {
                    value.push_str(", ");
                }
                value
            });
    let primary = portable
        .artists
        .first()
        .map(|artist| normalized(&artist.name))
        .unwrap_or_default();
    if normalized(&artist) != normalized(&local.artist) && primary != normalized(&local.artist) {
        return false;
    }
    if let (Some(remote), Some(saved)) = (portable.isrc.as_deref(), local.isrc.as_deref()) {
        if crate::recording_identity::normalize_isrc(remote)
            != crate::recording_identity::normalize_isrc(saved)
        {
            return false;
        }
    }
    if let (Some(remote), Some(saved)) = (portable.duration_seconds, local.duration_seconds) {
        let tolerance = 5.0_f64.max(remote.max(saved) * 0.02);
        if (remote - saved).abs() > tolerance {
            return false;
        }
    }
    true
}

fn history_playlist_input(
    history: &crate::playback_history::PlaybackHistory,
    recent_days: Option<u32>,
) -> Result<crate::portable_playlist::PlaylistInput> {
    let mut entries = Vec::new();
    let mut offset = 0u32;
    loop {
        let page = history.list(500, offset)?;
        if page.is_empty() {
            break;
        }
        for row in &page {
            if !history_entry_in_scope(&row.played_at, recent_days, chrono::Utc::now()) {
                continue;
            }
            entries.push(crate::portable_playlist::PortableEntry {
                // Stable IDs keep older monthly mirrors unchanged when a new session is inserted.
                id: format!("h-{}", digest(row.session_id.as_bytes())),
                history: Some(crate::portable_playlist::HistoryOccurrence {
                    occurrence_id: Some(format!("h-{}", digest(row.session_id.as_bytes()))),
                    played_at: Some(row.played_at.clone()),
                    ..Default::default()
                }),
                resolved_resource: row
                    .track
                    .local_path
                    .as_deref()
                    .filter(|path| Path::new(path).is_file())
                    .map(str::to_owned)
                    .or_else(|| {
                        row.track
                            .provenance
                            .get("candidateUrl")
                            .and_then(serde_json::Value::as_str)
                            .filter(|url| {
                                url.starts_with("https://") && !url.chars().any(char::is_control)
                            })
                            .map(str::to_owned)
                    }),
                track: crate::portable_playlist::PortableTrack::from_track(&row.track),
                ..Default::default()
            });
        }
        offset = offset.saturating_add(page.len() as u32);
        if page.len() < 500 {
            break;
        }
    }
    let scope_description =
        recent_days.map(|days| format!("Configured recent-history scope: last {days} days."));
    Ok(crate::portable_playlist::PlaylistInput {
        id: "history".into(),
        name: recent_days.map_or_else(
            || "Listening History".into(),
            |days| format!("Listening History (Recent {days} days)"),
        ),
        description: scope_description,
        entries,
        ..Default::default()
    })
}

fn history_entry_in_scope(
    played_at: &str,
    recent_days: Option<u32>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(days) = recent_days else {
        return true;
    };
    chrono::DateTime::parse_from_rfc3339(played_at)
        .ok()
        .is_none_or(|played_at| {
            played_at.with_timezone(&chrono::Utc) >= now - chrono::Duration::days(days as i64)
        })
}

#[derive(Debug, Clone)]
struct HistoryMirrorDocument {
    key: String,
    relative_path: PathBuf,
    body: String,
}

fn history_mirror_documents(
    input: &crate::portable_playlist::PlaylistInput,
    recent_days: Option<u32>,
) -> Result<Vec<HistoryMirrorDocument>> {
    history_mirror_documents_with_options(
        input,
        recent_days,
        &crate::playlist_mirror_options::PlaylistMirrorOptions::default(),
    )
}

fn history_mirror_documents_with_options(
    input: &crate::portable_playlist::PlaylistInput,
    recent_days: Option<u32>,
    mirror_options: &crate::playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<Vec<HistoryMirrorDocument>> {
    let options = crate::portable_playlist::PlaylistExportOptions {
        purpose: crate::portable_playlist::PlaylistPurpose::History,
        include_history_dates: true,
        include_media_links: Some(mirror_options.include_media_links.unwrap_or(false)),
        include_ytdlp_search: mirror_options.include_ytdlp_search.unwrap_or(false),
        ..Default::default()
    };
    match render_portable_m3u(input, &options) {
        Ok(body) => {
            let filename = recent_days.map_or_else(
                || "Listening History.m3u8".to_owned(),
                |days| format!("Listening History - Recent {days} days.m3u8"),
            );
            return Ok(vec![HistoryMirrorDocument {
                key: "history".into(),
                relative_path: PathBuf::from(filename),
                body,
            }]);
        }
        Err(error) if input.entries.len() > MAX_ENTRIES || error.contains("2 MB") => {}
        Err(error) => return Err(error),
    }

    let mut months: std::collections::BTreeMap<
        String,
        Vec<crate::portable_playlist::PortableEntry>,
    > = std::collections::BTreeMap::new();
    for entry in &input.entries {
        let month = entry
            .history
            .as_ref()
            .and_then(|history| history.played_at.as_deref())
            .and_then(|played_at| chrono::DateTime::parse_from_rfc3339(played_at).ok())
            .map(|played_at| played_at.format("%Y-%m").to_string())
            .unwrap_or_else(|| "Undated".into());
        months.entry(month).or_default().push(entry.clone());
    }
    let mut documents = Vec::with_capacity(months.len());
    for (month, entries) in months {
        let name = match recent_days {
            Some(days) => format!("Listening History (Recent {days} days) — {month}"),
            None => format!("Listening History — {month}"),
        };
        let partition = crate::portable_playlist::PlaylistInput {
            id: format!("history-{month}"),
            name,
            description: input.description.clone(),
            entries,
            ..Default::default()
        };
        let body = render_portable_m3u(&partition, &options).map_err(|_| {
            format!("History partition {month} exceeds portable playlist limits; choose a shorter recent-history scope")
        })?;
        let scope = recent_days.map_or_else(String::new, |days| format!("recent{days}:"));
        let partition_directory = recent_days.map_or_else(
            || PathBuf::from("Listening History"),
            |days| PathBuf::from(format!("Listening History - Recent {days} days")),
        );
        documents.push(HistoryMirrorDocument {
            key: format!("history:{scope}{month}"),
            relative_path: partition_directory.join(format!("{month}.m3u8")),
            body,
        });
    }
    Ok(documents)
}

fn render_portable_m3u(
    input: &crate::portable_playlist::PlaylistInput,
    options: &crate::portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    let body = crate::portable_playlist::serialize(input, options)?;
    let marker = if matches!(
        options.purpose,
        crate::portable_playlist::PlaylistPurpose::LocalMirror
            | crate::portable_playlist::PlaylistPurpose::History
    ) {
        "#BURRATA-MIRROR\n"
    } else {
        ""
    };
    let name = input.name.replace(['\r', '\n'], " ");
    Ok(body.replacen(
        "#EXTM3U\n",
        &format!("#EXTM3U\n{marker}#PLAYLIST:{name}\n"),
        1,
    ))
}

fn relative_mirror_resource(music_root: &Path, asset: &Path) -> Result<String> {
    let root = fs::canonicalize(music_root).map_err(|_| "Music directory is unavailable")?;
    relative_path_resource(&root.join("Playlists"), asset)
}

fn relative_path_resource(mirror_directory: &Path, asset: &Path) -> Result<String> {
    let mirror_directory = fs::canonicalize(mirror_directory)
        .map_err(|_| "Playlist mirror directory is unavailable")?;
    let asset = fs::canonicalize(asset).map_err(|_| "Playlist asset is unavailable")?;
    if !asset.is_file() {
        return Err("Playlist asset is unavailable".into());
    }
    let from = mirror_directory.components().collect::<Vec<_>>();
    let to = asset.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in to.iter().skip(common) {
        relative.push(component.as_os_str());
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// Export available local files to a deterministic Mixxx-compatible M3U8 location.
/// Remote and unavailable entries are reported and never emitted as stream URLs.
pub fn export_mixxx_playlist(
    store: &Store,
    settings: &Settings,
    collection_id: &str,
) -> Result<MixxxPlaylistExport> {
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    if settings.music_directory.trim().is_empty() {
        return Err("Choose a music directory first".into());
    }
    let collection = store
        .list_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or("Collection not found")?;
    let tracks = current_collection_tracks(store, collection_id)?;
    let root = Path::new(&settings.music_directory)
        .join("Playlists")
        .join("Mixxx");
    fs::create_dir_all(&root).map_err(|e| format!("Cannot create Mixxx export directory: {e}"))?;
    let title = collection.name.replace(['\r', '\n'], " ");
    let mut body = format!("#EXTM3U\n#BURRATA-MIRROR\n#PLAYLIST:{title}\n");
    let mut exported = 0;
    let mut missing = Vec::new();
    for track in &tracks {
        let local_path = track
            .local_path
            .as_deref()
            .and_then(|path| fs::canonicalize(path).ok())
            .filter(|path| path.is_file());
        let Some(local_path) = local_path else {
            missing.push(MixxxMissingTrack {
                id: track.id.clone(),
                artist: track.artist.clone(),
                title: track.title.clone(),
            });
            continue;
        };
        body.push_str(&format!(
            "#EXTINF:{},{} - {}{}\n{}\n",
            track
                .duration_seconds
                .map(|seconds| seconds.round() as i64)
                .unwrap_or(-1),
            track.artist,
            track.title,
            track
                .version
                .as_deref()
                .map(|version| format!(" ({version})"))
                .unwrap_or_default(),
            local_path.to_string_lossy()
        ));
        exported += 1;
    }
    let path = write_named_export(&root, &collection.id, &collection.name, body.as_bytes())?;
    Ok(MixxxPlaylistExport {
        path: path.to_string_lossy().into_owned(),
        exported,
        missing,
        total: tracks.len(),
    })
}

/// Write a user-facing export using a portable title while retaining distinct files
/// for collections whose names collide on case-insensitive filesystems.
fn write_named_export(root: &Path, owner_id: &str, title: &str, body: &[u8]) -> Result<PathBuf> {
    let _guard = PLAYLIST_EXPORT_LOCK
        .lock()
        .map_err(|_| "Playlist export lock is unavailable")?;
    fs::create_dir_all(root)
        .map_err(|e| format!("Cannot create playlist export directory: {e}"))?;
    let base = safe_playlist_filename(title);
    let index_path = root.join(".burrata-exports.json");
    let mut index: HashMap<String, NamedExportIndexEntry> = fs::read(&index_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if let Some(entry) = index.get(owner_id) {
        let entry = entry.clone();
        let path = root.join(&entry.filename);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let safe_component = Path::new(&entry.filename).components().count() == 1
            && Path::new(&entry.filename).file_name().is_some();
        let valid_extension = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("m3u8"));
        if safe_component
            && valid_extension
            && (stem == base
                || stem
                    .strip_prefix(&base)
                    .is_some_and(|tail| tail.starts_with(" (")))
        {
            match fs::symlink_metadata(&path) {
                Ok(metadata)
                    if metadata.file_type().is_file()
                        && digest(&fs::read(&path).unwrap_or_default()) == entry.sha256 =>
                {
                    atomic_write(&path, body)?;
                    index.insert(
                        owner_id.to_owned(),
                        NamedExportIndexEntry {
                            filename: entry.filename.clone(),
                            sha256: digest(body),
                        },
                    );
                    write_export_index(&index_path, &index)?;
                    return Ok(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    atomic_write(&path, body)?;
                    index.insert(
                        owner_id.to_owned(),
                        NamedExportIndexEntry {
                            filename: entry.filename.clone(),
                            sha256: digest(body),
                        },
                    );
                    write_export_index(&index_path, &index)?;
                    return Ok(path);
                }
                _ => {}
            }
        }
    }
    let mut suffix = 1usize;
    loop {
        if suffix > 10_000 {
            return Err("Too many playlists share this title".into());
        }
        let filename = if suffix == 1 {
            base.clone()
        } else {
            format!("{base} ({suffix})")
        };
        let candidate = root.join(format!("{filename}.m3u8"));
        let folded = format!("{filename}.m3u8").to_lowercase();
        let existing = fs::read_dir(root)
            .map_err(|e| format!("Cannot inspect playlist export directory: {e}"))?
            .filter_map(std::result::Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().to_lowercase() == folded);
        if existing.is_some() {
            suffix += 1;
            continue;
        }
        atomic_write(&candidate, body)?;
        index.insert(
            owner_id.to_owned(),
            NamedExportIndexEntry {
                filename: candidate
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                sha256: digest(body),
            },
        );
        write_export_index(&index_path, &index)?;
        return Ok(candidate);
    }
}

fn write_export_index(path: &Path, index: &HashMap<String, NamedExportIndexEntry>) -> Result<()> {
    let bytes = serde_json::to_vec(index).map_err(|error| error.to_string())?;
    atomic_write(path, &bytes)
}

fn safe_playlist_filename(title: &str) -> String {
    let mut safe = String::new();
    let mut pending_space = false;
    for ch in title.trim().chars() {
        if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            pending_space = !safe.is_empty();
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !safe.is_empty();
            continue;
        }
        if pending_space && !safe.ends_with(' ') {
            safe.push(' ');
        }
        pending_space = false;
        safe.push(ch);
        if safe.len() >= 120 {
            break;
        }
    }
    let safe = safe.trim().trim_matches('.').trim().to_owned();
    let safe = if safe.is_empty() {
        "Playlist".to_owned()
    } else {
        safe
    };
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if reserved {
        format!("_{safe}")
    } else {
        safe
    }
}

fn current_collection_tracks(store: &Store, collection_id: &str) -> Result<Vec<Track>> {
    let snapshots = store.browser_snapshots_for_collection(collection_id)?;
    current_collection_tracks_from_snapshots(store, collection_id, &snapshots)
}

fn current_collection_tracks_from_snapshots(
    store: &Store,
    collection_id: &str,
    snapshots: &[crate::models::Chart],
) -> Result<Vec<Track>> {
    if let Some(snapshot) = snapshots.first() {
        let ids = snapshot
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let saved_tracks = store.get_tracks(&ids)?;
        let tracks = merge_snapshot_track_state(snapshot.tracks.clone(), saved_tracks);
        Ok(crate::browser_bridge::expand_playlist_occurrences(tracks))
    } else {
        store.list_tracks("collection", Some(collection_id))
    }
}

fn merge_snapshot_track_state(snapshot_tracks: Vec<Track>, saved_tracks: Vec<Track>) -> Vec<Track> {
    let saved_by_id = saved_tracks
        .into_iter()
        .map(|track| (track.id.clone(), track))
        .collect::<HashMap<_, _>>();
    snapshot_tracks
        .into_iter()
        .map(|mut current| {
            if let Some(saved) = saved_by_id.get(&current.id) {
                // Match the prior per-track overlay, including None overwrites, while retaining
                // snapshot metadata and occurrence information.
                current.local_path = saved.local_path.clone();
                current.local_availability = saved.local_availability.clone();
                current.asset_id = saved.asset_id.clone();
                if let Some(candidate) = saved.provenance.get("audioCandidate") {
                    current.provenance["audioCandidate"] = candidate.clone();
                }
                if let Some(url) = saved.provenance.get("candidateUrl") {
                    current.provenance["candidateUrl"] = url.clone();
                }
            }
            current
        })
        .collect()
}

pub fn list_source_snapshots(store: &Store, canonical_path: &str) -> Result<Vec<SourceSnapshot>> {
    let key = fs::canonicalize(canonical_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| canonical_path.to_owned());
    Ok(read_state(store)?
        .sources
        .get(&key)
        .map(|s| s.snapshots.clone())
        .unwrap_or_default())
}

/// Load the exact archived playlist contents selected by revision hash.
pub fn load_source_snapshot(
    store: &Store,
    canonical_path: &str,
    revision: &str,
) -> Result<SourceSnapshot> {
    list_source_snapshots(store, canonical_path)?
        .into_iter()
        .find(|snapshot| snapshot.revision == revision)
        .ok_or_else(|| "Playlist snapshot not found".into())
}

pub fn list_mirror_snapshots(store: &Store, collection_id: &str) -> Result<Vec<SourceSnapshot>> {
    Ok(read_state(store)?
        .mirror_snapshots
        .get(collection_id)
        .cloned()
        .unwrap_or_default())
}

pub fn load_mirror_snapshot(
    store: &Store,
    collection_id: &str,
    revision: &str,
) -> Result<SourceSnapshot> {
    list_mirror_snapshots(store, collection_id)?
        .into_iter()
        .find(|snapshot| snapshot.revision == revision)
        .ok_or_else(|| "Playlist mirror snapshot not found".into())
}

/// Source imports are protected originals. Before a collection mutation, callers use this
/// helper to get a personal collection to edit. Repeated edits keep returning the same copy.
pub fn ensure_editable_collection(store: &Store, collection_id: &str) -> Result<String> {
    let mut state = read_state(store)?;
    let binding_path = state
        .sources
        .iter()
        .find(|(_, source)| source.collection_id.as_deref() == Some(collection_id))
        .map(|(path, _)| path.clone());
    let is_browser = store.is_browser_source_collection(collection_id)?;
    if binding_path.is_none() && !is_browser {
        return Ok(collection_id.to_owned());
    }
    let existing_copy = binding_path
        .as_ref()
        .and_then(|path| {
            state
                .sources
                .get(path)
                .and_then(|source| source.personal_collection_id.clone())
        })
        .or_else(|| state.protected_copies.get(collection_id).cloned());
    if let Some(personal_id) = existing_copy {
        if store
            .list_collections()?
            .iter()
            .any(|collection| collection.id == personal_id)
        {
            return Ok(personal_id);
        }
    }
    let original = store
        .list_collections()?
        .into_iter()
        .find(|collection| collection.id == collection_id)
        .ok_or("Collection not found")?;
    let tracks = current_collection_tracks(store, collection_id)?;
    let copy = store.create_collection(&format!("{} (Personal copy)", original.name))?;
    store.replace_playlist_entries(&copy.id, &tracks)?;
    crate::playlist_hierarchy::copy_layout(store, collection_id, &copy.id)?;
    if let Some(path) = binding_path {
        if let Some(binding) = state.sources.get_mut(&path) {
            binding.personal_collection_id = Some(copy.id.clone());
        }
    } else {
        state
            .protected_copies
            .insert(collection_id.to_owned(), copy.id.clone());
    }
    persist_state(store, &state)?;
    Ok(copy.id)
}

fn read_playlist(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).map_err(|e| format!("Cannot read playlist: {e}"))?;
    if !metadata.is_file() {
        return Err("Playlist path is not a file".into());
    }
    if metadata.len() > MAX_BYTES {
        return Err("Playlist exceeds 2 MB".into());
    }
    let bytes = fs::read(path).map_err(|e| format!("Cannot read playlist: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "Playlist is not valid UTF-8".into())
}
fn count_entries(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .count()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_state(store: &Store) -> Result<SyncState> {
    let Some(value) = store.playlist_sync_state()? else {
        return Ok(SyncState::default());
    };
    decode_sync_state(value)
}

/// Ordinary reconciliation only needs reference headers, never archived bodies.
fn read_runtime_state(store: &Store) -> Result<SyncState> {
    let Some(mut value) = store.get_json_preference("playlistSync.v1")? else {
        return Ok(SyncState::default());
    };
    if let Some(refs) = value
        .get_mut("mirrorSnapshotRefs")
        .and_then(serde_json::Value::as_object_mut)
    {
        for records in refs.values_mut() {
            for reference in records
                .as_array_mut()
                .ok_or("Invalid playlist snapshot references")?
            {
                if let Some(key) = reference.as_str() {
                    if !key.starts_with("playlistSnapshot.v1.") || key.len() != 84 {
                        return Err("Invalid playlist snapshot reference".into());
                    }
                    let revision = store.playlist_snapshot_revision(key)?;
                    *reference = serde_json::json!({"key":key,"revision":revision});
                }
            }
        }
    }
    decode_sync_state(value)
}

fn decode_sync_state(value: serde_json::Value) -> Result<SyncState> {
    // Older state has no restore gate. A backup projection, however, must carry
    // the explicit paused bit; fail closed if that invariant is malformed.
    let projection = value
        .get("backupProjection")
        .cloned()
        .map(normalize_section_mirror_projection);
    if let Some(projection) = projection.as_ref() {
        if !valid_backup_projection(projection) {
            return Err(
                "Restored playlist mirror projection is invalid; writes remain paused".into(),
            );
        }
    }
    let writes_paused = value
        .get("writesPaused")
        .and_then(serde_json::Value::as_bool);
    let mut state: SyncState = serde_json::from_value(value).map_err(|error| {
        if projection.is_some() {
            format!("Restored playlist mirror state is invalid ({error}); writes remain paused")
        } else {
            error.to_string()
        }
    })?;
    if let Some(projection) = projection {
        state.backup_projection = Some(projection);
        // The first read of an imported archive has writesPaused=true. Once an
        // explicit resume is persisted, preserve the inert projection without
        // re-arming the gate on every subsequent read.
        state.writes_paused = writes_paused.unwrap_or(true);
    }
    if writes_paused == Some(true) {
        state.writes_paused = true;
    }
    Ok(state)
}

fn restored_pause_message() -> String {
    "Playlist synchronization is paused after restore; review and explicitly resume it on this device".into()
}

fn persist_state(store: &Store, state: &SyncState) -> Result<()> {
    let mut value = serde_json::to_value(state).map_err(|e| e.to_string())?;
    if let Some(projection) = &state.backup_projection {
        value["backupProjection"] = projection.clone();
    }
    if state.writes_paused {
        value["writesPaused"] = serde_json::Value::Bool(true);
    }
    // Immutable snapshots are written once; mutable state contains only references.
    // Readers expand these references so backups retain the existing portable format.
    let mut refs = state.mirror_snapshot_refs.clone();
    for (owner, snapshots) in &state.mirror_snapshots {
        let references = refs.entry(owner.clone()).or_default();
        for snapshot in snapshots {
            if references
                .iter()
                .any(|reference| reference.revision == snapshot.revision)
            {
                continue;
            }
            let key = format!(
                "playlistSnapshot.v1.{}",
                digest(
                    format!(
                        "{}\0{}\0{}",
                        snapshot.revision, snapshot.captured_at, snapshot.contents
                    )
                    .as_bytes()
                )
            );
            if !store.json_preference_exists(&key)? {
                store.set_json_preference(
                    &key,
                    &serde_json::to_value(snapshot).map_err(|e| e.to_string())?,
                )?;
            }
            references.push(SnapshotReference {
                key,
                revision: snapshot.revision.clone(),
            });
        }
    }
    value["mirrorSnapshotRefs"] = serde_json::to_value(refs).map_err(|e| e.to_string())?;
    store.save_playlist_sync_state(&value)
}

fn valid_backup_projection(value: &serde_json::Value) -> bool {
    let base_is_valid = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(1)
        && value
            .get("writesPaused")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && ["sources", "mirrors", "protectedCopies"].iter().all(|key| {
            value
                .get(*key)
                .and_then(serde_json::Value::as_array)
                .is_some()
        });
    if !base_is_valid {
        return false;
    }
    let Some(section_mirrors) = value.get("sectionMirrors") else {
        // Older schema-version-one projections did not include section files.
        return true;
    };
    let Some(section_mirrors) = section_mirrors.as_array() else {
        return false;
    };
    let mut identities = HashSet::new();
    section_mirrors.iter().all(|entry| {
        let Some(collection_id) = entry
            .get("collectionId")
            .and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        let Some(section_id) = entry.get("sectionId").and_then(serde_json::Value::as_str) else {
            return false;
        };
        let Some(hash) = entry.get("hash").and_then(serde_json::Value::as_str) else {
            return false;
        };
        if !valid_section_mirror_identity(collection_id, section_id)
            || !valid_sha256(hash)
            || !identities.insert((collection_id, section_id))
        {
            return false;
        }
        if entry.get("relinkHint").is_some_and(|hint| {
            hint.get("basename")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|name| {
                    name.is_empty()
                        || name.len() > 4096
                        || name.contains(['/', '\\', ':'])
                        || !name.to_ascii_lowercase().ends_with(".m3u8")
                })
        }) {
            return false;
        }
        entry
            .get("snapshots")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|snapshots| {
                snapshots.iter().all(|snapshot| {
                    let Some(revision) = snapshot
                        .get("sourceOriginalSha256")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return false;
                    };
                    let Some(captured_at) = snapshot
                        .get("capturedAt")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return false;
                    };
                    let Some(contents) =
                        snapshot.get("contents").and_then(serde_json::Value::as_str)
                    else {
                        return false;
                    };
                    valid_sha256(revision)
                        && captured_at.len() <= 64
                        && contents.len() as u64 <= MAX_BYTES
                        && snapshot
                            .get("contentsSha256")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|hash| {
                                valid_sha256(hash) && digest(contents.as_bytes()) == hash
                            })
                        && crate::portable_playlist::parse(contents).is_ok()
                })
            })
    })
}

fn normalize_section_mirror_projection(mut projection: serde_json::Value) -> serde_json::Value {
    let Some(object) = projection.as_object_mut() else {
        return projection;
    };
    let mut mirrors = Vec::new();
    let mut section_entries = HashMap::<(String, String), Vec<serde_json::Value>>::new();
    for entry in object
        .get("mirrors")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(collection_key) = entry
            .get("collectionId")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if collection_key.starts_with("section:") {
            if let Some((collection_id, section_id)) = parse_section_mirror_key(collection_key) {
                if let Some(hash) = entry
                    .get("hash")
                    .and_then(serde_json::Value::as_str)
                    .filter(|hash| valid_sha256(hash))
                {
                    let mut section = serde_json::json!({
                        "collectionId": collection_id,
                        "sectionId": section_id,
                        "hash": hash.to_ascii_lowercase(),
                    });
                    if let Some(snapshots) =
                        entry.get("snapshots").and_then(serde_json::Value::as_array)
                    {
                        section["snapshots"] = serde_json::Value::Array(snapshots.clone());
                    }
                    if let Some(name) = entry
                        .get("relinkHint")
                        .and_then(|value| value.get("basename"))
                        .and_then(serde_json::Value::as_str)
                        .filter(|name| safe_section_relink_basename(name))
                    {
                        section["relinkHint"] = serde_json::json!({"basename":name});
                    }
                    section_entries
                        .entry((collection_id.to_owned(), section_id.to_owned()))
                        .or_default()
                        .push(section);
                }
            }
            continue;
        }
        mirrors.push(entry.clone());
    }
    if let Some(items) = object
        .get("sectionMirrors")
        .and_then(serde_json::Value::as_array)
    {
        for entry in items {
            let Some(collection_id) = entry
                .get("collectionId")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let Some(section_id) = entry.get("sectionId").and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let Some(hash) = entry
                .get("hash")
                .and_then(serde_json::Value::as_str)
                .filter(|hash| valid_sha256(hash))
            else {
                continue;
            };
            if !valid_section_mirror_identity(collection_id, section_id) {
                continue;
            }
            let mut section = serde_json::json!({
                "collectionId": collection_id,
                "sectionId": section_id,
                "hash": hash.to_ascii_lowercase(),
            });
            if let Some(snapshots) = entry.get("snapshots").and_then(serde_json::Value::as_array) {
                section["snapshots"] = serde_json::Value::Array(snapshots.clone());
            }
            if let Some(name) = entry
                .get("relinkHint")
                .and_then(|value| value.get("basename"))
                .and_then(serde_json::Value::as_str)
                .filter(|name| safe_section_relink_basename(name))
            {
                section["relinkHint"] = serde_json::json!({"basename":name});
            }
            section_entries
                .entry((collection_id.to_owned(), section_id.to_owned()))
                .or_default()
                .push(section);
        }
    }
    let mut section_mirrors = section_entries
        .into_values()
        .filter_map(|mut entries| (entries.len() == 1).then(|| entries.remove(0)))
        .collect::<Vec<_>>();
    section_mirrors.sort_by(|left, right| {
        left["collectionId"]
            .as_str()
            .cmp(&right["collectionId"].as_str())
            .then_with(|| left["sectionId"].as_str().cmp(&right["sectionId"].as_str()))
    });
    object.insert("mirrors".into(), serde_json::Value::Array(mirrors));
    object.insert(
        "sectionMirrors".into(),
        serde_json::Value::Array(section_mirrors),
    );
    projection
}

fn parse_section_mirror_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("section:")?;
    let (collection_id, section_id) = rest.split_once(':')?;
    if section_id.contains(':') || !valid_section_mirror_identity(collection_id, section_id) {
        return None;
    }
    Some((collection_id, section_id))
}

fn safe_section_relink_basename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 4096
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && name.to_ascii_lowercase().ends_with(".m3u8")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredSyncResumePreview {
    pub review_token: String,
    pub current_mirror_directory: Option<String>,
    pub source_count: usize,
    pub mirror_count: usize,
    pub protected_copy_count: usize,
    #[serde(default)]
    pub section_mirror_relinks: Vec<SectionMirrorRelink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionMirrorRelink {
    pub playlist_name: String,
    pub section_name: String,
    pub relative_path: String,
}

#[derive(Debug, Clone)]
struct VerifiedSectionMirrorRelink {
    collection_id: String,
    section_id: String,
    hash: String,
    path: PathBuf,
    relative_path: String,
    playlist_name: String,
    section_name: String,
    snapshots: Vec<SourceSnapshot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SectionMirrorReviewIdentity<'a> {
    collection_id: &'a str,
    section_id: &'a str,
    hash: &'a str,
    relative_path: &'a str,
    playlist_name: &'a str,
    section_name: &'a str,
}

/// Build a review token for the exact inert projection and local destination
/// the user is about to activate. Restored absolute paths are never consulted.
pub fn preview_restored_playlist_sync(
    store: &Store,
    settings: &Settings,
) -> Result<RestoredSyncResumePreview> {
    let state = read_state(store)?;
    let projection = state
        .backup_projection
        .as_ref()
        .ok_or_else(|| "There is no restored playlist projection to review".to_string())?;
    if !state.writes_paused {
        return Err("Restored playlist synchronization is already active".into());
    }
    let (directory, root_identity, hierarchy, candidates) =
        restored_section_mirror_review(store, settings, &state, projection)?;
    let review_token = restored_sync_review_token(
        projection,
        directory.as_deref(),
        &root_identity,
        &hierarchy,
        &candidates,
    )?;
    Ok(RestoredSyncResumePreview {
        review_token,
        current_mirror_directory: directory,
        source_count: projection["sources"].as_array().map_or(0, Vec::len),
        mirror_count: projection["mirrors"]
            .as_array()
            .map_or(0, Vec::len)
            .saturating_add(projection["sectionMirrors"].as_array().map_or(0, Vec::len)),
        protected_copy_count: projection["protectedCopies"].as_array().map_or(0, Vec::len),
        section_mirror_relinks: candidates
            .iter()
            .map(|candidate| SectionMirrorRelink {
                playlist_name: candidate.playlist_name.clone(),
                section_name: candidate.section_name.clone(),
                relative_path: candidate.relative_path.clone(),
            })
            .collect(),
    })
}

/// Bind a restored history mirror only after the user has reviewed this
/// device's destination. The archive stores a path-free hash and basename;
/// requiring both the canonical filename and exact bytes keeps unrelated
/// destination files unbound and fail-closed.
fn bind_verified_restored_history_mirror(
    store: &Store,
    settings: &Settings,
    state: &mut SyncState,
) -> Result<()> {
    let Some(directory) = mirrors_dir(settings) else {
        return Ok(());
    };
    if get_history_mirror_options(store)?.recent_days.is_some() {
        // A scoped history mirror uses a different output filename and cannot
        // be matched safely from this basename-only projection.
        return Ok(());
    }
    let Some(mirrors) = state
        .backup_projection
        .as_ref()
        .and_then(|projection| projection.get("mirrors"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    let mut history = mirrors.iter().filter(|mirror| {
        mirror
            .get("collectionId")
            .and_then(serde_json::Value::as_str)
            == Some("history")
    });
    let Some(mirror) = history.next() else {
        return Ok(());
    };
    if history.next().is_some()
        || mirror
            .get("relinkHint")
            .and_then(|hint| hint.get("basename"))
            .and_then(serde_json::Value::as_str)
            != Some("Listening History.m3u8")
    {
        return Ok(());
    }
    let Some(expected_hash) = mirror
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
    else {
        return Ok(());
    };
    let path = directory.join("Listening History.m3u8");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        _ => return Ok(()),
    };
    if metadata.len() > MAX_HISTORY_MIRROR_BYTES {
        return Ok(());
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => return Ok(()),
    };
    if digest(contents.as_bytes()) != expected_hash {
        return Ok(());
    }
    state.mirror_hashes.insert("history".into(), expected_hash);
    state
        .mirror_paths
        .insert("history".into(), path.to_string_lossy().into_owned());
    Ok(())
}

/// Resume only when the user's reviewed projection and destination still match.
pub fn resume_reviewed_restored_playlist_sync(
    store: &Store,
    settings: &Settings,
    expected_review_token: &str,
) -> Result<()> {
    require_playlist_mirroring_enabled(settings)?;
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let mut state = read_state(store)?;
    let projection = state
        .backup_projection
        .as_ref()
        .ok_or_else(|| "There is no restored playlist projection to review".to_string())?;
    if !state.writes_paused {
        return Err("Restored playlist synchronization is already active".into());
    }
    let (directory, root_identity, hierarchy, candidates) =
        restored_section_mirror_review(store, settings, &state, projection)?;
    let current_review_token = restored_sync_review_token(
        projection,
        directory.as_deref(),
        &root_identity,
        &hierarchy,
        &candidates,
    )?;
    if current_review_token != expected_review_token {
        return Err(
            "Restored playlist state or music folder changed; review it again before resuming"
                .into(),
        );
    }
    for candidate in candidates {
        let key = section_mirror_key(&candidate.collection_id, &candidate.section_id);
        state.mirror_hashes.insert(key.clone(), candidate.hash);
        state
            .mirror_paths
            .insert(key, candidate.path.to_string_lossy().into_owned());
        if !candidate.snapshots.is_empty() {
            state.mirror_snapshots.insert(
                section_mirror_snapshot_key(&candidate.collection_id, &candidate.section_id),
                candidate.snapshots,
            );
        }
    }
    bind_verified_restored_history_mirror(store, settings, &mut state)?;
    state.writes_paused = false;
    persist_state(store, &state)
}

fn restored_section_mirror_review(
    store: &Store,
    settings: &Settings,
    state: &SyncState,
    projection: &serde_json::Value,
) -> Result<(
    Option<String>,
    serde_json::Value,
    serde_json::Value,
    Vec<VerifiedSectionMirrorRelink>,
)> {
    let root = mirrors_dir(settings);
    let directory = root
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    let root_identity = section_mirror_root_identity(root.as_deref());
    let hierarchy = crate::playlist_hierarchy::load(store)?;
    let hierarchy_value = serde_json::to_value(&hierarchy).map_err(|error| error.to_string())?;
    let Some(root) = root else {
        return Ok((directory, root_identity, hierarchy_value, Vec::new()));
    };
    if !root.is_absolute() {
        return Ok((directory, root_identity, hierarchy_value, Vec::new()));
    }
    let Some(items) = projection
        .get("sectionMirrors")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok((directory, root_identity, hierarchy_value, Vec::new()));
    };
    let mut identity_counts = HashMap::<(String, String), usize>::new();
    for item in items {
        if let (Some(collection_id), Some(section_id)) = (
            item.get("collectionId").and_then(serde_json::Value::as_str),
            item.get("sectionId").and_then(serde_json::Value::as_str),
        ) {
            *identity_counts
                .entry((collection_id.to_owned(), section_id.to_owned()))
                .or_default() += 1;
        }
    }
    let collections = store.list_collections()?;
    let mut candidates = Vec::new();
    for item in items {
        let Some(collection_id) = item.get("collectionId").and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(section_id) = item.get("sectionId").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !valid_section_mirror_identity(collection_id, section_id)
            || identity_counts.get(&(collection_id.to_owned(), section_id.to_owned())) != Some(&1)
        {
            continue;
        }
        let Some(hash) = item
            .get("hash")
            .and_then(serde_json::Value::as_str)
            .filter(|hash| valid_sha256(hash))
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        let Some(collection) = collections
            .iter()
            .find(|collection| collection.id == collection_id)
        else {
            continue;
        };
        let placement = hierarchy
            .playlists
            .iter()
            .find(|playlist| playlist.collection_id == collection_id);
        let Some(section) = placement.and_then(|playlist| {
            playlist
                .sections
                .iter()
                .find(|section| section.id == section_id)
        }) else {
            continue;
        };
        let tracks = store.list_tracks("collection", Some(collection_id))?;
        let relative_directory = mirror_relative_directory(store, state, collection_id, &tracks)?;
        let path = root
            .join(relative_directory)
            .join(collection_id)
            .join("Sections")
            .join(format!("{section_id}.m3u8"));
        let relative_path = match path.strip_prefix(&root) {
            // This value is a portable relative path shown during cross-device
            // relinking, so serialize separators independently of the host OS.
            Ok(relative) => relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
            Err(_) => continue,
        };
        let basename = format!("{section_id}.m3u8");
        let hint = item
            .get("relinkHint")
            .and_then(|value| value.get("basename"))
            .and_then(serde_json::Value::as_str);
        if hint != Some(basename.as_str()) {
            continue;
        }
        let Some(contents) = read_regular_section_file_under_root(&root, &path) else {
            continue;
        };
        if digest(&contents) != hash
            || !is_matching_restored_section_mirror(&contents, &collection.name, &section.name)
        {
            continue;
        }
        candidates.push(VerifiedSectionMirrorRelink {
            collection_id: collection_id.to_owned(),
            section_id: section_id.to_owned(),
            hash,
            path,
            relative_path,
            playlist_name: collection.name.clone(),
            section_name: section.name.clone(),
            snapshots: projected_section_snapshots(item),
        });
    }
    candidates.sort_by(|left, right| {
        left.collection_id
            .cmp(&right.collection_id)
            .then_with(|| left.section_id.cmp(&right.section_id))
    });
    Ok((directory, root_identity, hierarchy_value, candidates))
}

fn section_mirror_root_identity(root: Option<&Path>) -> serde_json::Value {
    let Some(root) = root else {
        return serde_json::Value::Null;
    };
    let metadata = fs::symlink_metadata(root).ok();
    let canonical = fs::canonicalize(root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    #[cfg(unix)]
    let filesystem_identity = metadata.as_ref().map(|metadata| {
        use std::os::unix::fs::MetadataExt;
        serde_json::json!({
            "kind": "device-inode",
            "device": metadata.dev(),
            "inode": metadata.ino(),
        })
    });
    #[cfg(not(unix))]
    let filesystem_identity = metadata.as_ref().map(|metadata| {
        let time_identity = |time: std::io::Result<std::time::SystemTime>| {
            time.ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| format!("{}.{:09}", duration.as_secs(), duration.subsec_nanos()))
        };
        serde_json::json!({
            "kind": "bounded-metadata-signature",
            "created": time_identity(metadata.created()),
            "modified": time_identity(metadata.modified()),
            "length": metadata.len(),
        })
    });
    serde_json::json!({
        "configuredPath": root.to_string_lossy(),
        "canonicalPath": canonical,
        "isDirectory": metadata.as_ref().is_some_and(|metadata| metadata.is_dir()),
        "isSymlink": metadata.as_ref().is_some_and(|metadata| metadata.file_type().is_symlink()),
        "filesystemIdentity": filesystem_identity,
    })
}

fn read_regular_section_file_under_root(root: &Path, path: &Path) -> Option<Vec<u8>> {
    let root_metadata = fs::symlink_metadata(root).ok()?;
    if !root.is_absolute() || !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return None;
    }
    let canonical_root = fs::canonicalize(root).ok()?;
    let relative = path.strip_prefix(root).ok()?;
    let mut cursor = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return None;
    }
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return None;
        };
        cursor.push(name);
        let metadata = fs::symlink_metadata(&cursor).ok()?;
        if metadata.file_type().is_symlink() {
            return None;
        }
        if index + 1 == components.len() {
            if !metadata.is_file() || metadata.len() > MAX_BYTES {
                return None;
            }
        } else if !metadata.is_dir() {
            return None;
        }
    }
    let canonical_path = fs::canonicalize(path).ok()?;
    if !canonical_path.starts_with(&canonical_root) {
        return None;
    }
    fs::read(path).ok()
}

fn is_matching_restored_section_mirror(
    contents: &[u8],
    playlist_name: &str,
    section_name: &str,
) -> bool {
    let Ok(contents) = std::str::from_utf8(contents) else {
        return false;
    };
    if !contents.lines().any(|line| line.trim() == "#EXTM3U")
        || !contents
            .lines()
            .any(|line| line.trim() == "#BURRATA-MIRROR")
    {
        return false;
    }
    let Ok(parsed) = crate::portable_playlist::parse(contents) else {
        return false;
    };
    let expected_name = format!("{playlist_name} — {section_name}")
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    parsed.sections.len() == 1
        && parsed.sections[0].name == section_name
        && parsed.name == expected_name
        && contents
            .lines()
            .any(|line| line.trim() == format!("#PLAYLIST:{}", parsed.name))
        && parsed
            .entries
            .iter()
            .all(|entry| entry.section_id.as_deref() == Some("s1"))
}

fn projected_section_snapshots(entry: &serde_json::Value) -> Vec<SourceSnapshot> {
    entry
        .get("snapshots")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|snapshot| {
            let source_original_hash = snapshot
                .get("sourceOriginalSha256")
                .and_then(serde_json::Value::as_str)?;
            let contents_hash = snapshot
                .get("contentsSha256")
                .and_then(serde_json::Value::as_str)?;
            let captured_at = snapshot
                .get("capturedAt")
                .and_then(serde_json::Value::as_str)?;
            let contents = snapshot
                .get("contents")
                .and_then(serde_json::Value::as_str)?;
            if !valid_sha256(source_original_hash)
                || !valid_sha256(contents_hash)
                || captured_at.len() > 64
                || contents.len() as u64 > MAX_BYTES
                || digest(contents.as_bytes()) != contents_hash
            {
                return None;
            }
            Some(SourceSnapshot {
                // The active snapshot baseline must describe these sanitized
                // bytes. Keep the archive's original hash only in the inert
                // backup projection as provenance.
                revision: contents_hash.to_ascii_lowercase(),
                captured_at: captured_at.to_owned(),
                contents: contents.to_owned(),
            })
        })
        .collect()
}

fn restored_sync_review_token(
    projection: &serde_json::Value,
    directory: Option<&str>,
    root_identity: &serde_json::Value,
    hierarchy: &serde_json::Value,
    candidates: &[VerifiedSectionMirrorRelink],
) -> Result<String> {
    let candidate_identities = candidates
        .iter()
        .map(|candidate| SectionMirrorReviewIdentity {
            collection_id: &candidate.collection_id,
            section_id: &candidate.section_id,
            hash: &candidate.hash,
            relative_path: &candidate.relative_path,
            playlist_name: &candidate.playlist_name,
            section_name: &candidate.section_name,
        })
        .collect::<Vec<_>>();
    let token_input = serde_json::json!({
        "projection": projection,
        "currentMirrorDirectory": directory,
        "mirrorRootIdentity": root_identity,
        "playlistHierarchy": hierarchy,
        "sectionMirrorRelinks": candidate_identities,
    });
    let token_bytes = serde_json::to_vec(&token_input).map_err(|error| error.to_string())?;
    Ok(digest(&token_bytes))
}

fn section_mirror_key(collection_id: &str, section_id: &str) -> String {
    format!("section:{collection_id}:{section_id}")
}

fn section_mirror_snapshot_key(collection_id: &str, section_id: &str) -> String {
    format!("section__{collection_id}__{section_id}")
}

fn valid_section_mirror_identity(collection_id: &str, section_id: &str) -> bool {
    [collection_id, section_id].into_iter().all(|id| {
        !id.is_empty()
            && id.len() <= 256
            && id != "."
            && id != ".."
            && !id.chars().any(char::is_control)
            && !id.contains([':', '/', '\\'])
            && !id.contains("://")
    })
}

fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Explicit user action to begin publishing local mirrors after a restore.
/// The imported backup projection remains inert and available for future relink
/// UI; this only lifts the global write gate for the current device.
pub fn resume_restored_playlist_sync(store: &Store) -> Result<()> {
    let settings = store.settings()?;
    require_playlist_mirroring_enabled(&settings)?;
    let _library_write = store.try_library_write()?;
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let mut state = read_state(store)?;
    if state.backup_projection.is_none() {
        return Err("There is no restored playlist projection to resume".into());
    }
    state.writes_paused = false;
    persist_state(store, &state)
}
fn require_playlist_mirroring_enabled(settings: &Settings) -> Result<()> {
    if settings.playlist_mirroring_enabled {
        Ok(())
    } else {
        Err("Playlist mirroring is turned off in Settings".into())
    }
}

fn mirrors_dir(settings: &Settings) -> Option<PathBuf> {
    if settings.music_directory.trim().is_empty() {
        return None;
    }
    Some(Path::new(&settings.music_directory).join("Playlists"))
}

/// Detect the known legacy renderer without scanning or changing user files. Only a
/// path already bound in sync state (or the bound Favorites path) under this
/// device's managed mirror root is eligible. Reads are capped at the playlist
/// format's 2 MB limit and the current bytes must match the stored baseline hash.
pub(crate) fn legacy_mirror_has_search_fallback(
    store: &Store,
    collection_id: &str,
) -> std::result::Result<bool, String> {
    let settings = store.settings()?;
    let state = read_state(store)?;
    legacy_mirror_search_fallback_in_state(&settings, &state, collection_id)
}

fn legacy_mirror_search_fallback_in_state(
    settings: &Settings,
    state: &SyncState,
    collection_id: &str,
) -> std::result::Result<bool, String> {
    let Some(mirror_root) = mirrors_dir(&settings) else {
        return Ok(false);
    };
    let Ok(canonical_root) = fs::canonicalize(&mirror_root) else {
        return Ok(false);
    };
    let (key, path) = if let Some(path) = state.mirror_paths.get(collection_id) {
        (collection_id.to_owned(), PathBuf::from(path))
    } else if collection_id == "favorites" && state.mirror_hashes.contains_key("favorites") {
        ("favorites".to_owned(), mirror_root.join("Liked Songs.m3u8"))
    } else if collection_id == "history" {
        let Some((key, path)) = state
            .mirror_paths
            .iter()
            .find(|(key, _)| *key == "history" || key.starts_with("history:"))
        else {
            return Ok(false);
        };
        (key.clone(), PathBuf::from(path))
    } else {
        return Ok(false);
    };
    if !state.mirror_hashes.contains_key(&key) {
        return Ok(false);
    }
    let Ok(canonical_path) = fs::canonicalize(path) else {
        return Ok(false);
    };
    if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
        return Ok(false);
    }
    let Some(baseline) = state.mirror_hashes.get(&key) else {
        return Ok(false);
    };
    let file = fs::File::open(canonical_path).map_err(|error| error.to_string())?;
    let mut contents = Vec::with_capacity(4096);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| error.to_string())?;
    if contents.len() as u64 > MAX_BYTES || digest(&contents) != *baseline {
        return Ok(false);
    }
    let contents = String::from_utf8_lossy(&contents);
    Ok(contents
        .lines()
        .any(|line| line.trim() == "#BURRATA-MIRROR")
        && !crate::portable_playlist::is_portable_playlist(&contents))
}

fn effective_render_options(
    settings: &Settings,
    state: &SyncState,
    collection_id: &str,
    options: &crate::playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<crate::playlist_mirror_options::PlaylistMirrorOptions> {
    let default_media_links = collection_id != "history";
    let default_ytdlp_search = collection_id != "history"
        && legacy_mirror_search_fallback_in_state(settings, state, collection_id)?;
    Ok(crate::playlist_mirror_options::PlaylistMirrorOptions {
        included: options.included,
        include_media_links: Some(options.include_media_links.unwrap_or(default_media_links)),
        include_ytdlp_search: Some(options.include_ytdlp_search.unwrap_or(default_ytdlp_search)),
    })
}

fn mirror_stamp(store: &Store, state: &SyncState, id: &str, settings: &Settings) -> Result<String> {
    let (global, external, revision) = store.playlist_mirror_revision(id)?;
    let mut hash = Sha256::new();
    hash.update(format!(
        "{global}:{external}:{revision}:{}:{id}",
        settings.music_directory
    ));
    let mut paths = state
        .mirror_paths
        .iter()
        .filter(|(key, _)| {
            key.as_str() == id
                || parse_section_mirror_key(key).is_some_and(|(owner, _)| owner == id)
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|(key, _)| *key);
    for (key, path) in paths {
        hash.update(key.as_bytes());
        crate::playlist_incremental::stamp_path(&mut hash, Path::new(path));
    }
    for path in store.playlist_asset_paths(id)? {
        crate::playlist_incremental::stamp_path(&mut hash, Path::new(&path));
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn mirror_is_clean(store: &Store, id: &str, stamp: &str) -> Result<bool> {
    let mut cache = store
        .playlist_incremental
        .lock()
        .map_err(|e| e.to_string())?;
    let clean = cache.mirrors.get(id).is_some_and(|old| old == stamp);
    if !clean {
        cache.mirror_work += 1;
    }
    Ok(clean)
}
fn remember_mirror(store: &Store, id: &str, stamp: String) -> Result<()> {
    let mut cache = store
        .playlist_incremental
        .lock()
        .map_err(|e| e.to_string())?;
    if cache.mirrors.len() >= 10_000 {
        cache.mirrors.clear();
    }
    cache.mirrors.insert(id.to_owned(), stamp);
    Ok(())
}

fn mirror_browser_snapshots(
    store: &Store,
    collection_id: &str,
    root: &Path,
) -> Result<Vec<crate::models::Chart>> {
    let headers = store.browser_snapshot_headers(collection_id)?;
    let mut snapshots = Vec::new();
    for (index, (id, fetched_at)) in headers.into_iter().enumerate() {
        let stamp = fetched_at
            .chars()
            .filter(|c| c.is_ascii_digit())
            .take(14)
            .collect::<String>();
        let filename = format!("{stamp}-{id}.m3u8");
        let archived = ["Personal", "Spotify", "Beatport", "Traxsource", "Local"]
            .iter()
            .any(|group| {
                root.join("Archives")
                    .join(group)
                    .join(collection_id)
                    .join(&filename)
                    .is_file()
            });
        if index == 0 || !archived {
            let snapshot = store.raw_chart_snapshot(&id)?;
            if !archived {
                archive_chart_snapshot(
                    root,
                    &collection_group(&snapshot.tracks),
                    collection_id,
                    &snapshot,
                )?;
            }
            if index == 0 {
                snapshots.push(snapshot);
            }
        }
    }
    Ok(snapshots)
}

fn write_mirrors(
    store: &Store,
    settings: &Settings,
    state: &mut SyncState,
    warnings: &mut Vec<String>,
    history: Option<&crate::playback_history::PlaybackHistory>,
) -> Result<Option<String>> {
    if !settings.playlist_mirroring_enabled {
        return Ok(None);
    }
    let Some(dir) = mirrors_dir(settings) else {
        return Ok(None);
    };
    let mirror_policy = crate::playlist_mirror_options::get_playlist_mirror_policy(store)?;
    let collections = store.list_collections()?;
    let has_included_collection = collections.iter().any(|collection| {
        mirror_policy
            .playlists
            .get(&collection.id)
            .is_none_or(|options| options.included)
    });
    let history_included = mirror_policy
        .playlists
        .get("history")
        .is_none_or(|options| options.included);
    let favorites_included = mirror_policy
        .playlists
        .get("favorites")
        .is_none_or(|options| options.included);
    let history_options = mirror_policy
        .playlists
        .get("history")
        .cloned()
        .unwrap_or_default();
    let history_render_options =
        effective_render_options(settings, state, "history", &history_options)?;
    if !has_included_collection && !favorites_included && !(history_included && history.is_some()) {
        return Ok(None);
    }
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create playlist mirror directory: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create collection mirrors: {e}"))?;
    let mirror_started = std::time::Instant::now();
    if std::env::var_os("BURRATA_PERF_TRACE").is_some() {
        eprintln!(
            "[perf] playlist_sync.mirror_collections count={}",
            collections.len()
        );
    }
    let mut collection_names = collections
        .iter()
        .map(|collection| (collection.id.clone(), collection.name.clone()))
        .collect::<HashMap<_, _>>();
    for collection in collections {
        std::thread::yield_now();
        let mirror_options = mirror_policy
            .playlists
            .get(&collection.id)
            .cloned()
            .unwrap_or_default();
        if !mirror_options.included {
            continue;
        }
        if mapped_personal_collection_id(state, &collection.id).is_some_and(|personal_id| {
            mirror_policy
                .playlists
                .get(personal_id)
                .is_some_and(|personal_options| !personal_options.included)
        }) {
            // The origin's mirror is the route used to refresh this personal copy.
            // Honor the destination's exclusion before reading or applying the origin file.
            continue;
        }
        let stamp = mirror_stamp(store, state, &collection.id, settings)?;
        if mirror_is_clean(store, &collection.id, &stamp)? {
            continue;
        }
        let mirror_render_options =
            effective_render_options(settings, state, &collection.id, &mirror_options)?;
        let browser_snapshots = mirror_browser_snapshots(store, &collection.id, &dir)?;
        let tracks =
            current_collection_tracks_from_snapshots(store, &collection.id, &browser_snapshots)?;
        let original_group = mirror_group(state, &collection.id, &tracks);
        let relative_dir = mirror_relative_directory(store, state, &collection.id, &tracks)?;
        let group_dir = dir.join(&relative_dir);
        fs::create_dir_all(&group_dir)
            .map_err(|e| format!("Cannot create playlist mirrors: {e}"))?;
        let expected_path = group_dir.join(format!("{}.m3u8", collection.id));
        let prior_path = state.mirror_paths.get(&collection.id).map(PathBuf::from);
        let original_path = prior_path.as_deref().unwrap_or(&expected_path);
        if prior_path.as_deref().is_some_and(|prior| !prior.exists()) {
            state.paused_mirrors.insert(collection.id.clone());
            state.preserve_mirrors.insert(collection.id.clone());
            warnings.push(format!(
                "{}: mirror was deleted; synchronization is paused until explicitly resumed",
                original_path.display()
            ));
            continue;
        }
        let edit = apply_mirror_edit_with_options(
            store,
            state,
            &collection.id,
            original_path,
            &dir,
            &tracks,
            warnings,
            &mirror_render_options,
        )?;
        if state.preserve_mirrors.contains(&collection.id) {
            if has_bound_section_mirror(state, &collection.id) {
                write_section_mirrors_with_options(
                    store,
                    state,
                    &collection.id,
                    &collection.name,
                    &tracks,
                    &group_dir,
                    &dir,
                    warnings,
                    &mirror_render_options,
                )?;
            }
            continue;
        }
        let target_id = edit
            .personal_collection_id
            .clone()
            .unwrap_or_else(|| collection.id.clone());
        let target_mirror_options = if target_id == collection.id {
            mirror_options.clone()
        } else {
            mirror_policy
                .playlists
                .get(&target_id)
                .cloned()
                .unwrap_or_default()
        };
        if !target_mirror_options.included {
            // This is a second guard for targets resolved during edit application;
            // known mappings are filtered before apply_mirror_edit above.
            continue;
        }
        let target_render_options =
            effective_render_options(settings, state, &target_id, &target_mirror_options)?;
        let target_tracks = if target_id == collection.id {
            if edit.original_collection_changed {
                current_collection_tracks(store, &collection.id)?
            } else {
                tracks
            }
        } else {
            store.list_tracks("collection", Some(&target_id))?
        };
        let cached_name = collection_names
            .get(&target_id)
            .filter(|_| !edit.original_collection_changed && edit.personal_collection_id.is_none());
        let target_name = if let Some(name) = cached_name {
            name.clone()
        } else {
            // An edit may have created a new personal collection during this pass.
            store
                .list_collections()?
                .into_iter()
                .find(|item| item.id == target_id)
                .map(|item| item.name)
                .unwrap_or_else(|| collection.name.clone())
        };
        collection_names.insert(target_id.clone(), target_name.clone());
        let target_group = mirror_group(state, &target_id, &target_tracks);
        let target_relative_dir =
            mirror_relative_directory(store, state, &target_id, &target_tracks)?;
        let target_dir = dir.join(&target_relative_dir);
        fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Cannot create playlist mirrors: {e}"))?;
        let target_path = target_dir.join(format!("{}.m3u8", target_id));
        let previous_target = fs::read_to_string(&target_path).ok();
        let previous_source_is_portable = fs::read_to_string(original_path)
            .ok()
            .as_deref()
            .is_some_and(crate::portable_playlist::is_portable_playlist);
        // Keep legacy mirrors lazy. New mirrors use the portable format, and once
        // a mirror is v1 it stays v1 through normal syncs.
        let target_body = if previous_target
            .as_deref()
            .is_some_and(crate::portable_playlist::is_portable_playlist)
            || previous_source_is_portable
            || target_mirror_options.include_media_links.is_some()
            || target_mirror_options.include_ytdlp_search.is_some()
            || (previous_target.is_none() && !state.mirror_hashes.contains_key(&target_id))
        {
            render_managed_playlist_m3u_with_options(
                store,
                &target_id,
                &target_name,
                &target_tracks,
                &target_dir,
                &target_render_options,
            )?
        } else {
            render_named_m3u(&target_name, &target_tracks, &settings.music_directory)
        };
        if let Some(previous) = previous_target.as_deref() {
            if previous != target_body {
                archive_mirror(state, &dir, &target_group, &target_id, previous)?;
            }
        }
        if previous_target.as_deref() != Some(&target_body) {
            atomic_write(&target_path, target_body.as_bytes())?;
        }
        if target_id == collection.id && original_path != target_path && original_path.exists() {
            archive_mirror(
                state,
                &dir,
                &original_group,
                &collection.id,
                &fs::read_to_string(original_path).unwrap_or_default(),
            )?;
            fs::remove_file(&original_path)
                .map_err(|e| format!("Cannot retire prior playlist mirror: {e}"))?;
        }
        state
            .mirror_hashes
            .insert(target_id.clone(), digest(target_body.as_bytes()));
        state.mirror_paths.insert(
            target_id.clone(),
            target_path.to_string_lossy().into_owned(),
        );
        write_section_mirrors_with_options(
            store,
            state,
            &target_id,
            &target_name,
            &target_tracks,
            &target_dir,
            &dir,
            warnings,
            &target_render_options,
        )?;
        let after = mirror_stamp(store, state, &collection.id, settings)?;
        // A changed output path causes one verification pass; concurrent edits stay dirty.
        if stamp == after {
            remember_mirror(store, &collection.id, stamp)?;
        }
    }
    let favorites_stamp = mirror_stamp(store, state, "favorites", settings)?;
    if favorites_included && !mirror_is_clean(store, "favorites", &favorites_stamp)? {
        let favorite_options = mirror_policy
            .playlists
            .get("favorites")
            .cloned()
            .unwrap_or_default();
        let favorite_render_options =
            effective_render_options(settings, state, "favorites", &favorite_options)?;
        let favorite_tracks = store.list_tracks("favorites", None)?;
        let favorite_relative_dir =
            mirror_relative_directory(store, state, "favorites", &favorite_tracks)?;
        let likes_path = dir.join(favorite_relative_dir).join("Liked Songs.m3u8");
        let prior_favorites_path = state.mirror_paths.get("favorites").map(PathBuf::from);
        let original_favorites_path = prior_favorites_path.as_deref().unwrap_or(&likes_path);
        if prior_favorites_path
            .as_deref()
            .is_some_and(|path| !path.exists())
        {
            state.paused_mirrors.insert("favorites".into());
            state.preserve_mirrors.insert("favorites".into());
            warnings.push(
            "Liked Songs mirror was deleted; synchronization is paused until explicitly resumed"
                .into(),
        );
        }
        let original_favorites_dir = original_favorites_path.parent().unwrap_or(&dir);
        let original_favorites_body = if fs::read_to_string(original_favorites_path)
            .ok()
            .as_deref()
            .is_some_and(crate::portable_playlist::is_portable_playlist)
            || !state.mirror_hashes.contains_key("favorites")
        {
            render_managed_playlist_m3u_with_options(
                store,
                "favorites",
                "Liked Songs",
                &favorite_tracks,
                original_favorites_dir,
                &favorite_render_options,
            )?
        } else {
            render_named_m3u("Liked Songs", &favorite_tracks, &settings.music_directory)
        };
        let original_favorites_is_portable = fs::read_to_string(original_favorites_path)
            .ok()
            .as_deref()
            .is_some_and(crate::portable_playlist::is_portable_playlist);
        if let Some(contents) = read_external_edit(
            &state.mirror_hashes,
            "favorites",
            original_favorites_path,
            &original_favorites_body,
        ) {
            match resolve_mirror_edit(store, &contents, original_favorites_dir) {
                Ok((_, _, Some(sections))) if !sections.sections.is_empty() => {
                    warnings.push(format!(
                    "{}: this Liked Songs file contains playlist sections that Favorites cannot store; import it as a playlist to preserve them. The file and favorites were kept for review",
                    original_favorites_path.display()
                ));
                    state.paused_mirrors.insert("favorites".into());
                    state.preserve_mirrors.insert("favorites".into());
                }
                Ok((tracks, 0, _)) => {
                    for track in store
                        .list_tracks("library", None)?
                        .iter()
                        .filter(|t| t.favorite)
                    {
                        store.set_favorite(track, false)?;
                    }
                    for track in &tracks {
                        store.set_favorite(track, true)?;
                    }
                }
                Ok((_, unresolved, _)) => {
                    warnings.push(format!(
                    "{}: Liked Songs mirror has {unresolved} unresolved entries; the file and favorites were kept for review",
                    original_favorites_path.display()
                ));
                    state.paused_mirrors.insert("favorites".into());
                    state.preserve_mirrors.insert("favorites".into());
                }
                Err(error) => {
                    warnings.push(format!(
                    "{}: Liked Songs edit could not be applied ({error}); the file and favorites were kept for review",
                    original_favorites_path.display()
                ));
                    state.paused_mirrors.insert("favorites".into());
                    state.preserve_mirrors.insert("favorites".into());
                }
            }
        }
        if state.preserve_mirrors.contains("favorites") {
            if history_included {
                if let Some(history) = history {
                    write_history_mirror_with_options(
                        store,
                        history,
                        &dir,
                        state,
                        warnings,
                        &history_render_options,
                    )?;
                }
            }
            return Ok(Some(dir.to_string_lossy().into_owned()));
        }
        let favorites = store.list_tracks("favorites", None)?;
        let previous_favorites = fs::read_to_string(&likes_path).ok();
        let target_is_known = previous_favorites
            .as_ref()
            .zip(state.mirror_hashes.get("favorites"))
            .is_some_and(|(contents, hash)| digest(contents.as_bytes()) == *hash);
        if original_favorites_path != likes_path && likes_path.exists() && !target_is_known {
            state.paused_mirrors.insert("favorites".into());
            state.preserve_mirrors.insert("favorites".into());
            warnings.push(
            "Liked Songs hierarchy destination already contains an unbound file; it was preserved"
                .into(),
        );
        }
        if state.preserve_mirrors.contains("favorites") {
            if history_included {
                if let Some(history) = history {
                    write_history_mirror_with_options(
                        store,
                        history,
                        &dir,
                        state,
                        warnings,
                        &history_render_options,
                    )?;
                }
            }
            return Ok(Some(dir.to_string_lossy().into_owned()));
        }
        let body = if previous_favorites
            .as_deref()
            .is_some_and(crate::portable_playlist::is_portable_playlist)
            || original_favorites_is_portable
            || favorite_options.include_media_links.is_some()
            || favorite_options.include_ytdlp_search.is_some()
            || (previous_favorites.is_none() && !state.mirror_hashes.contains_key("favorites"))
        {
            render_managed_playlist_m3u_with_options(
                store,
                "favorites",
                "Liked Songs",
                &favorites,
                likes_path.parent().unwrap_or(&dir),
                &favorite_render_options,
            )?
        } else {
            render_named_m3u("Liked Songs", &favorites, &settings.music_directory)
        };
        if fs::read_to_string(&likes_path).ok().as_deref() != Some(&body) {
            if let Ok(previous) = fs::read_to_string(&likes_path) {
                archive_mirror(state, &dir, "Favorites", "favorites", &previous)?;
            }
            atomic_write(&likes_path, body.as_bytes())?;
        }
        if original_favorites_path != likes_path && original_favorites_path.exists() {
            archive_mirror(
                state,
                &dir,
                "Favorites",
                "favorites",
                &fs::read_to_string(original_favorites_path).unwrap_or_default(),
            )?;
            fs::remove_file(original_favorites_path)
                .map_err(|error| format!("Cannot retire prior Liked Songs mirror: {error}"))?;
        }
        state
            .mirror_hashes
            .insert("favorites".into(), digest(body.as_bytes()));
        state.mirror_paths.insert(
            "favorites".into(),
            likes_path.to_string_lossy().into_owned(),
        );
        write_section_mirrors_with_options(
            store,
            state,
            "favorites",
            "Liked Songs",
            &favorites,
            likes_path.parent().unwrap_or(&dir),
            &dir,
            warnings,
            &favorite_render_options,
        )?;
        if favorites_stamp == mirror_stamp(store, state, "favorites", settings)? {
            remember_mirror(store, "favorites", favorites_stamp)?;
        }
    }
    if history_included {
        if let Some(history) = history {
            let stamp = format!(
                "{}:{:?}:{}",
                mirror_stamp(store, state, "history", settings)?,
                history.mirror_revision()?,
                chrono::Utc::now().date_naive()
            );
            if mirror_is_clean(store, "history", &stamp)? {
                return Ok(Some(dir.to_string_lossy().into_owned()));
            }
            write_history_mirror_with_options(
                store,
                history,
                &dir,
                state,
                warnings,
                &history_render_options,
            )?;
            let after = format!(
                "{}:{:?}:{}",
                mirror_stamp(store, state, "history", settings)?,
                history.mirror_revision()?,
                chrono::Utc::now().date_naive()
            );
            if stamp == after {
                remember_mirror(store, "history", stamp)?;
            }
        }
    }
    if std::env::var_os("BURRATA_PERF_TRACE").is_some() {
        eprintln!(
            "[perf] playlist_sync.mirror_collections_elapsed elapsed_ms={}",
            mirror_started.elapsed().as_millis()
        );
    }
    Ok(Some(dir.to_string_lossy().into_owned()))
}

fn read_external_edit(
    hashes: &HashMap<String, String>,
    key: &str,
    path: &Path,
    generated: &str,
) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let current = digest(text.as_bytes());
    let baseline_changed = hashes
        .get(key)
        .map_or(true, |baseline| current != *baseline);
    (baseline_changed && current != digest(generated.as_bytes())).then_some(text)
}

fn collection_group(tracks: &[Track]) -> String {
    let providers: HashSet<&str> = tracks.iter().map(|track| track.provider.as_str()).collect();
    if providers.is_empty() {
        return "Personal".into();
    }
    if providers.len() != 1 {
        return "Personal".into();
    }
    let provider = providers
        .iter()
        .next()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match provider.as_str() {
        "spotify" => "Spotify".into(),
        "beatport" => "Beatport".into(),
        "traxsource" => "Traxsource".into(),
        "local" => "Local".into(),
        _ => "Personal".into(),
    }
}

fn mirror_group(state: &SyncState, collection_id: &str, tracks: &[Track]) -> String {
    let projected_personal = state.backup_projection.as_ref().is_some_and(|projection| {
        projection
            .get("protectedCopies")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .any(|entry| {
                entry
                    .get("personalCollectionId")
                    .and_then(serde_json::Value::as_str)
                    == Some(collection_id)
            })
            || projection
                .get("sources")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|source| {
                    source
                        .get("personalCollectionId")
                        .and_then(serde_json::Value::as_str)
                        == Some(collection_id)
                })
    });
    if projected_personal
        || state
            .protected_copies
            .values()
            .any(|id| id == collection_id)
        || state
            .sources
            .values()
            .any(|source| source.personal_collection_id.as_deref() == Some(collection_id))
    {
        "Personal".into()
    } else {
        collection_group(tracks)
    }
}

fn has_bound_section_mirror(state: &SyncState, collection_id: &str) -> bool {
    let prefix = format!("section:{collection_id}:");
    state
        .mirror_paths
        .keys()
        .any(|key| key.starts_with(&prefix) && state.mirror_hashes.contains_key(key))
}

/// Use explicit persisted folder ancestry for mirrors. Provider group folders
/// remain the fallback for playlists without user/source hierarchy metadata.
fn mirror_relative_directory(
    store: &Store,
    state: &SyncState,
    collection_id: &str,
    tracks: &[Track],
) -> Result<PathBuf> {
    let ancestry = crate::playlist_hierarchy::folder_ancestry(store, collection_id)?;
    if ancestry.is_empty() {
        if collection_id == "favorites" || collection_id == "history" {
            return Ok(PathBuf::new());
        }
        return Ok(PathBuf::from(mirror_group(state, collection_id, tracks)));
    }
    let hierarchy = crate::playlist_hierarchy::load(store)?;
    let mut relative = PathBuf::new();
    for folder in ancestry {
        let parent = hierarchy
            .folders
            .iter()
            .find(|candidate| candidate.id == folder.id)
            .and_then(|candidate| candidate.parent_id.as_deref());
        let component = mirror_component(&folder.name);
        let collides = hierarchy
            .folders
            .iter()
            .filter(|candidate| {
                candidate.parent_id.as_deref() == parent
                    && mirror_component(&candidate.name).eq_ignore_ascii_case(&component)
            })
            .count()
            > 1;
        let component = if collides {
            let suffix = folder
                .id
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .rev()
                .take(8)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            format!("{component}~{suffix}")
        } else {
            component
        };
        relative.push(component);
    }
    Ok(relative)
}

fn mirror_component(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    result = result.trim().trim_matches('.').to_owned();
    if result.is_empty() {
        result = "Untitled".into();
    }
    if [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .iter()
    .any(|reserved| result.eq_ignore_ascii_case(reserved))
    {
        result.insert(0, '_');
    }
    result.chars().take(100).collect()
}

fn archive_mirror(
    state: &mut SyncState,
    root: &Path,
    group: &str,
    id: &str,
    contents: &str,
) -> Result<()> {
    let revision = digest(contents.as_bytes());
    if state
        .mirror_snapshot_refs
        .get(id)
        .is_some_and(|refs| refs.iter().any(|reference| reference.revision == revision))
    {
        return Ok(());
    }
    let snapshots = state.mirror_snapshots.entry(id.to_owned()).or_default();
    if snapshots
        .iter()
        .any(|snapshot| snapshot.revision == revision)
    {
        return Ok(());
    }
    let captured_at = chrono::Utc::now().to_rfc3339();
    snapshots.push(SourceSnapshot {
        revision: revision.clone(),
        captured_at: captured_at.clone(),
        contents: contents.to_owned(),
    });
    let dir = root.join("Archives").join(group).join(id);
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create playlist archive: {e}"))?;
    let stamp = captured_at
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(14)
        .collect::<String>();
    let path = dir.join(format!("{stamp}-{revision}.m3u8"));
    if !path.exists() {
        atomic_write(&path, &mark_burrata_mirror(contents))?;
    }
    Ok(())
}

fn mark_burrata_mirror(contents: &str) -> Vec<u8> {
    if contents
        .lines()
        .any(|line| line.trim() == "#BURRATA-MIRROR")
    {
        return contents.as_bytes().to_vec();
    }
    if let Some(body) = contents.strip_prefix("#EXTM3U\n") {
        format!("#EXTM3U\n#BURRATA-MIRROR\n{body}").into_bytes()
    } else {
        format!("#BURRATA-MIRROR\n{contents}").into_bytes()
    }
}

fn archive_chart_snapshot(
    root: &Path,
    group: &str,
    id: &str,
    snapshot: &crate::models::Chart,
) -> Result<()> {
    let dir = root.join("Archives").join(group).join(id);
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create chart playlist archive: {e}"))?;
    let stamp = snapshot
        .fetched_at
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(14)
        .collect::<String>();
    let path = dir.join(format!("{}-{}.m3u8", stamp, snapshot.id));
    if !path.exists() {
        atomic_write(
            &path,
            render_named_m3u(
                &snapshot.title,
                &crate::browser_bridge::expand_playlist_occurrences(snapshot.tracks.clone()),
                "",
            )
            .as_bytes(),
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct MirrorEditOutcome {
    personal_collection_id: Option<String>,
    original_collection_changed: bool,
}

fn apply_mirror_edit(
    store: &Store,
    state: &mut SyncState,
    collection_id: &str,
    path: &Path,
    mirror_root: &Path,
    current_tracks: &[Track],
    warnings: &mut Vec<String>,
) -> Result<MirrorEditOutcome> {
    apply_mirror_edit_with_options(
        store,
        state,
        collection_id,
        path,
        mirror_root,
        current_tracks,
        warnings,
        &crate::playlist_mirror_options::PlaylistMirrorOptions::default(),
    )
}

fn apply_mirror_edit_with_options(
    store: &Store,
    state: &mut SyncState,
    collection_id: &str,
    path: &Path,
    mirror_root: &Path,
    current_tracks: &[Track],
    warnings: &mut Vec<String>,
    mirror_options: &crate::playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<MirrorEditOutcome> {
    // Most sync passes see untouched mirrors. Check the persisted baseline before doing
    // any additional work; the caller's hydrated tracks are reused for rendering.
    if let Some(baseline) = state.mirror_hashes.get(collection_id) {
        if fs::read_to_string(path)
            .ok()
            .is_some_and(|text| digest(text.as_bytes()) == *baseline)
        {
            return Ok(MirrorEditOutcome::default());
        }
    }
    let collection = store
        .list_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or("Collection not found")?;
    let managed = render_managed_playlist_m3u_with_options(
        store,
        collection_id,
        &collection.name,
        current_tracks,
        path.parent().unwrap_or(mirror_root),
        mirror_options,
    )?;
    let legacy = render_named_m3u(&collection.name, current_tracks, "");
    let baseline = state.mirror_hashes.get(collection_id);
    // Pick the format that matches the known app-written baseline. That keeps
    // edits recognizable when a user removes the portable headers in an editor.
    let generated = if baseline.is_some_and(|hash| *hash == digest(managed.as_bytes()))
        || fs::read_to_string(path)
            .ok()
            .as_deref()
            .is_some_and(crate::portable_playlist::is_portable_playlist)
    {
        managed
    } else if baseline.is_some_and(|hash| *hash == digest(legacy.as_bytes())) || baseline.is_some()
    {
        legacy
    } else {
        managed
    };
    let Some(contents) = read_external_edit(&state.mirror_hashes, collection_id, path, &generated)
    else {
        return Ok(MirrorEditOutcome::default());
    };
    let group = collection_group(current_tracks);
    archive_mirror(state, mirror_root, &group, collection_id, &generated)?;
    archive_mirror(state, mirror_root, &group, collection_id, &contents)?;
    let (tracks, unresolved, portable_sections) = match resolve_mirror_edit(
        store,
        &contents,
        path.parent().unwrap_or(mirror_root),
    ) {
        Ok(result) => result,
        Err(error) => {
            warnings.push(format!(
                "{}: mirror edit could not be applied ({error}); the file and playlist were kept for review",
                path.display()
            ));
            state.paused_mirrors.insert(collection_id.to_owned());
            state.preserve_mirrors.insert(collection_id.to_owned());
            return Ok(MirrorEditOutcome::default());
        }
    };
    if unresolved > 0 {
        warnings.push(format!(
            "{}: mirror edit has {unresolved} unresolved entries; the file and playlist were kept for review",
            path.display()
        ));
        state.paused_mirrors.insert(collection_id.to_owned());
        state.preserve_mirrors.insert(collection_id.to_owned());
        return Ok(MirrorEditOutcome::default());
    }
    let source_changed = state.sources.iter().any(|(source_path, source)| {
        source.collection_id.as_deref() == Some(collection_id)
            && state.changed_sources.contains(source_path)
    });
    if source_changed {
        warnings.push(format!(
            "{}: simultaneous source and mirror edits were preserved as a personal conflict copy",
            path.display()
        ));
    }
    let baseline = state.mirror_hashes.get(collection_id);
    let app_changed = baseline.is_some_and(|hash| digest(generated.as_bytes()) != *hash);
    let mirror_changed = baseline.is_some_and(|hash| digest(contents.as_bytes()) != *hash);
    if app_changed && mirror_changed {
        if source_changed {
            let personal_id = ensure_editable_collection(store, collection_id)?;
            replace_mirror_entries(store, &personal_id, &tracks, portable_sections.as_ref())?;
            state
                .mirror_hashes
                .insert(collection_id.to_owned(), digest(contents.as_bytes()));
            warnings.push(format!(
                "{}: source and mirror both changed; preserved the mirror edit in a personal copy",
                path.display()
            ));
            return Ok(MirrorEditOutcome {
                personal_collection_id: Some(personal_id),
                original_collection_changed: false,
            });
        }
        let conflict_name = format!(
            "{} (conflict {})",
            collection.name,
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        );
        let conflict = store.create_collection(&conflict_name)?;
        replace_mirror_entries(store, &conflict.id, &tracks, portable_sections.as_ref())?;
        state
            .mirror_hashes
            .insert(collection_id.to_owned(), digest(contents.as_bytes()));
        state.preserve_mirrors.insert(collection_id.to_owned());
        warnings.push(format!(
            "{}: app and mirror both changed; preserved the mirror edit in {}",
            path.display(),
            conflict_name
        ));
        return Ok(MirrorEditOutcome::default());
    }
    let target_id = ensure_editable_collection(store, collection_id)?;
    if let Some(name) = playlist_title(&contents) {
        if name != collection.name {
            store.rename_collection(&target_id, &name)?;
        }
    }
    if target_id != collection_id {
        if let Some(source) = state
            .sources
            .values_mut()
            .find(|source| source.collection_id.as_deref() == Some(collection_id))
        {
            source.personal_collection_id = Some(target_id.clone());
        } else if store.is_browser_source_collection(collection_id)? {
            state
                .protected_copies
                .insert(collection_id.to_owned(), target_id.clone());
        }
    }
    replace_mirror_entries(store, &target_id, &tracks, portable_sections.as_ref())?;
    if target_id != collection_id {
        atomic_write(path, generated.as_bytes())?;
        state
            .mirror_hashes
            .insert(collection_id.to_owned(), digest(generated.as_bytes()));
        Ok(MirrorEditOutcome {
            personal_collection_id: Some(target_id),
            original_collection_changed: false,
        })
    } else {
        Ok(MirrorEditOutcome {
            personal_collection_id: None,
            original_collection_changed: true,
        })
    }
}

fn validate_mirror_section_tokens(input: &crate::portable_playlist::PlaylistInput) -> Result<()> {
    if input.sections.len() > 256 {
        return Err("Portable playlist section metadata exceeds the supported limit".into());
    }
    let mut tokens = HashSet::new();
    for section in &input.sections {
        let Some(number) = section
            .id
            .strip_prefix('s')
            .and_then(|value| value.parse::<usize>().ok())
        else {
            return Err("Portable playlist section metadata contains a foreign token".into());
        };
        if number == 0
            || number > 256
            || section.id != format!("s{number}")
            || !tokens.insert(section.id.as_str())
        {
            return Err("Portable playlist section metadata is duplicated or out of bounds".into());
        }
    }
    Ok(())
}

struct ResolvedMirrorSections {
    sections: Vec<crate::portable_playlist::NamedItem>,
    entry_sections: Vec<Option<String>>,
}

fn replace_mirror_entries(
    store: &Store,
    collection_id: &str,
    tracks: &[Track],
    sections: Option<&ResolvedMirrorSections>,
) -> Result<()> {
    if let Some(sections) = sections {
        store.replace_playlist_entries_with_portable_sections(
            collection_id,
            tracks,
            &sections.sections,
            &sections.entry_sections,
        )
    } else {
        store.replace_playlist_entries(collection_id, tracks)
    }
}

fn resolve_mirror_edit(
    store: &Store,
    text: &str,
    mirror_root: &Path,
) -> Result<(Vec<Track>, usize, Option<ResolvedMirrorSections>)> {
    if crate::portable_playlist::is_portable_playlist(text) {
        let parsed = crate::portable_playlist::parse(text)
            .map_err(|error| format!("Invalid portable playlist: {error}"))?;
        validate_mirror_section_tokens(&parsed)?;
        let assets = store.list_assets()?;
        let mut tracks = Vec::with_capacity(parsed.entries.len());
        let sections = ResolvedMirrorSections {
            sections: parsed.sections.clone(),
            entry_sections: parsed
                .entries
                .iter()
                .map(|entry| entry.section_id.clone())
                .collect(),
        };
        for entry in parsed.entries {
            let matched = entry.resource.as_deref().and_then(|resource| {
                if resource.contains("://") {
                    return None;
                }
                let candidate = Path::new(resource);
                let path = if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    mirror_root.join(candidate)
                };
                let canonical = fs::canonicalize(path).ok()?;
                assets.iter().find_map(|asset| {
                    (fs::canonicalize(&asset.path).ok().as_ref() == Some(&canonical))
                        .then(|| asset.track.clone())
                })
            });
            // Placeholder links, URLs, and unmatched paths are inert. Retain their
            // structured metadata as an unverified entry instead of opening them.
            tracks.push(matched.unwrap_or_else(|| entry.track.to_track()));
        }
        return Ok((tracks, 0, Some(sections)));
    }
    let assets = store.list_assets()?;
    let library = store.list_tracks("library", None)?;
    let mut tracks = Vec::new();
    let mut unresolved = 0;
    let mut extinf: Option<String> = None;
    for raw in text.lines().map(str::trim) {
        if raw.starts_with("#EXTINF:") {
            extinf = Some(raw.to_owned());
            continue;
        }
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let candidate = Path::new(raw);
        let path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            mirror_root.join(candidate)
        };
        let matched = if raw.contains("://") {
            library.iter().find(|track| {
                track.source_url == raw
                    || track
                        .provenance
                        .get("candidateUrl")
                        .and_then(serde_json::Value::as_str)
                        == Some(raw)
            })
        } else {
            fs::canonicalize(path).ok().and_then(|real| {
                assets
                    .iter()
                    .find(|asset| fs::canonicalize(&asset.path).ok().as_ref() == Some(&real))
                    .map(|asset| &asset.track)
            })
        };
        if let Some(track) = matched {
            tracks.push(track.clone());
            extinf = None;
            continue;
        }
        let metadata = extinf.take().and_then(|line| {
            crate::imports::parse(&format!("#EXTM3U\n{line}\n{raw}\n"), "m3u")
                .ok()?
                .into_iter()
                .next()
        });
        if let Some(mut track) = metadata {
            if raw.contains("://") {
                track.source_url = raw.to_owned();
                playlist_files::attach_playlist_audio_candidate(&mut track, raw);
            } else {
                track.provenance["unverifiedPlaylistPath"] = serde_json::json!(raw);
            }
            track.provenance["playlistEntry"] = serde_json::json!(raw);
            tracks.push(track);
        } else {
            unresolved += 1;
        }
        extinf = None;
    }
    Ok((tracks, unresolved, None))
}

fn render_managed_playlist_m3u(
    store: &Store,
    collection_id: &str,
    name: &str,
    tracks: &[Track],
    mirror_directory: &Path,
) -> Result<String> {
    render_managed_playlist_m3u_with_options(
        store,
        collection_id,
        name,
        tracks,
        mirror_directory,
        &crate::playlist_mirror_options::PlaylistMirrorOptions::default(),
    )
}

fn render_managed_playlist_m3u_with_options(
    store: &Store,
    collection_id: &str,
    name: &str,
    tracks: &[Track],
    mirror_directory: &Path,
    mirror_options: &crate::playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<String> {
    let mut input = if collection_id == "favorites" {
        crate::portable_playlist::PlaylistInput {
            id: "favorites".into(),
            name: name.to_owned(),
            folder_ancestry: crate::playlist_hierarchy::folder_ancestry(store, "favorites")?,
            sections: crate::playlist_hierarchy::sections(store, "favorites")?,
            entries: tracks
                .iter()
                .enumerate()
                .map(|(index, track)| crate::portable_playlist::PortableEntry {
                    id: format!("f{}", index + 1),
                    track: crate::portable_playlist::PortableTrack::from_track(track),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    } else {
        portable_playlist_input_for_tracks(store, collection_id, name.to_owned(), tracks)?
    };
    attach_local_resources_at(store, &mut input, tracks, Some(mirror_directory))?;
    render_portable_m3u(
        &input,
        &crate::portable_playlist::PlaylistExportOptions {
            purpose: crate::portable_playlist::PlaylistPurpose::LocalMirror,
            include_media_links: Some(
                mirror_options
                    .include_media_links
                    .unwrap_or(collection_id != "history"),
            ),
            include_ytdlp_search: mirror_options.include_ytdlp_search.unwrap_or(false),
            ..Default::default()
        },
    )
}

fn write_section_mirrors(
    store: &Store,
    state: &mut SyncState,
    collection_id: &str,
    playlist_name: &str,
    tracks: &[Track],
    playlist_directory: &Path,
    mirror_root: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    write_section_mirrors_with_options(
        store,
        state,
        collection_id,
        playlist_name,
        tracks,
        playlist_directory,
        mirror_root,
        warnings,
        &crate::playlist_mirror_options::PlaylistMirrorOptions::default(),
    )
}

fn write_section_mirrors_with_options(
    store: &Store,
    state: &mut SyncState,
    collection_id: &str,
    playlist_name: &str,
    tracks: &[Track],
    playlist_directory: &Path,
    mirror_root: &Path,
    warnings: &mut Vec<String>,
    mirror_options: &crate::playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<()> {
    let input = if collection_id == "favorites" {
        crate::portable_playlist::PlaylistInput {
            id: "favorites".into(),
            name: playlist_name.to_owned(),
            folder_ancestry: crate::playlist_hierarchy::folder_ancestry(store, "favorites")?,
            sections: crate::playlist_hierarchy::sections(store, "favorites")?,
            entries: tracks
                .iter()
                .enumerate()
                .map(|(index, track)| crate::portable_playlist::PortableEntry {
                    id: format!("f{}", index + 1),
                    track: crate::portable_playlist::PortableTrack::from_track(track),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    } else {
        portable_playlist_input_for_tracks(store, collection_id, playlist_name.to_owned(), tracks)?
    };
    let sections = input.sections.clone();
    let asset_paths = tracks
        .iter()
        .filter_map(|track| track.local_path.clone())
        .collect::<Vec<_>>();
    let assets = store.registered_asset_paths(&asset_paths)?;
    let active_keys = sections
        .iter()
        .map(|section| format!("section:{collection_id}:{}", section.id))
        .collect::<HashSet<_>>();
    for section in sections {
        let key = format!("section:{collection_id}:{}", section.id);
        let expected_path = playlist_directory
            .join(collection_id)
            .join("Sections")
            .join(format!("{}.m3u8", section.id));
        let prior_path = state.mirror_paths.get(&key).map(PathBuf::from);
        let source_path = prior_path.as_deref().unwrap_or(&expected_path);
        if prior_path.as_deref().is_some_and(|path| !path.exists()) {
            state.paused_mirrors.insert(key.clone());
            state.preserve_mirrors.insert(key.clone());
            warnings.push(format!(
                "Section mirror {} was deleted; it was preserved",
                source_path.display()
            ));
            continue;
        }
        let mut section_input = input.clone();
        section_input.name = format!("{playlist_name} — {}", section.name);
        section_input.sections.retain(|item| item.id == section.id);
        let mut section_tracks = Vec::new();
        let mut section_entries = Vec::new();
        for (entry, track) in input.entries.iter().zip(tracks) {
            if entry.section_id.as_deref() == Some(section.id.as_str()) {
                section_entries.push(entry.clone());
                section_tracks.push(track.clone());
            }
        }
        section_input.entries = section_entries;
        let section_dir = expected_path
            .parent()
            .ok_or("Section mirror path has no directory")?;
        for (entry, track) in section_input.entries.iter_mut().zip(&section_tracks) {
            let local = track
                .local_path
                .as_deref()
                .filter(|path| assets.contains(*path))
                .and_then(|path| fs::canonicalize(path).ok())
                .filter(|path| path.is_file())
                .and_then(|path| relative_path_resource(section_dir, &path).ok());
            entry.resolved_resource = local.or_else(|| {
                track
                    .provenance
                    .get("candidateUrl")
                    .and_then(serde_json::Value::as_str)
                    .filter(|url| url.starts_with("https://") && !url.chars().any(char::is_control))
                    .map(str::to_owned)
            });
        }
        let body = render_portable_m3u(
            &section_input,
            &crate::portable_playlist::PlaylistExportOptions {
                purpose: crate::portable_playlist::PlaylistPurpose::LocalMirror,
                include_media_links: Some(
                    mirror_options
                        .include_media_links
                        .unwrap_or(collection_id != "history"),
                ),
                include_ytdlp_search: mirror_options.include_ytdlp_search.unwrap_or(false),
                ..Default::default()
            },
        )?;
        match fs::read_to_string(source_path) {
            Ok(previous) => {
                let current_hash = digest(previous.as_bytes());
                let Some(baseline) = state.mirror_hashes.get(&key) else {
                    state.paused_mirrors.insert(key.clone());
                    state.preserve_mirrors.insert(key.clone());
                    warnings.push(format!(
                        "Unbound section file {} was preserved",
                        source_path.display()
                    ));
                    continue;
                };
                if current_hash != *baseline {
                    state.paused_mirrors.insert(key.clone());
                    state.preserve_mirrors.insert(key.clone());
                    warnings.push(format!(
                        "Section file {} has external edits and was preserved",
                        source_path.display()
                    ));
                    continue;
                }
                if previous != body {
                    archive_mirror(
                        state,
                        mirror_root,
                        "Sections",
                        &section_mirror_snapshot_key(collection_id, &section.id),
                        &previous,
                    )?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if state.mirror_hashes.contains_key(&key) {
                    state.paused_mirrors.insert(key.clone());
                    state.preserve_mirrors.insert(key.clone());
                    warnings.push(format!(
                        "Section file {} was deleted and not recreated",
                        source_path.display()
                    ));
                    continue;
                }
            }
            Err(error) => return Err(format!("Cannot read section mirror: {error}")),
        }
        if let Some(parent) = expected_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if fs::read_to_string(&expected_path).ok().as_deref() != Some(&body) {
            atomic_write(&expected_path, body.as_bytes())?;
        }
        if source_path != expected_path && source_path.exists() {
            let previous = fs::read_to_string(source_path).unwrap_or_default();
            archive_mirror(
                state,
                mirror_root,
                "Sections",
                &section_mirror_snapshot_key(collection_id, &section.id),
                &previous,
            )?;
            fs::remove_file(source_path).map_err(|error| error.to_string())?;
        }
        state
            .mirror_hashes
            .insert(key.clone(), digest(body.as_bytes()));
        state
            .mirror_paths
            .insert(key, expected_path.to_string_lossy().into_owned());
    }
    let stale_keys = state
        .mirror_paths
        .keys()
        .filter(|key| key.starts_with(&format!("section:{collection_id}:")))
        .filter(|key| !active_keys.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for key in stale_keys {
        let Some(path) = state.mirror_paths.get(&key).map(PathBuf::from) else {
            continue;
        };
        if let Ok(contents) = fs::read_to_string(&path) {
            let hash = digest(contents.as_bytes());
            if state
                .mirror_hashes
                .get(&key)
                .is_some_and(|baseline| baseline == &hash)
            {
                archive_mirror(
                    state,
                    mirror_root,
                    "Sections",
                    &key.replace(':', "__"),
                    &contents,
                )?;
                fs::remove_file(&path).map_err(|error| error.to_string())?;
                state.mirror_paths.remove(&key);
                state.mirror_hashes.remove(&key);
            } else {
                warnings.push(format!(
                    "Removed section mirror {} has external edits and was preserved",
                    path.display()
                ));
            }
        } else {
            state.mirror_paths.remove(&key);
            state.mirror_hashes.remove(&key);
        }
    }
    Ok(())
}

fn write_history_mirror(
    store: &Store,
    history: &crate::playback_history::PlaybackHistory,
    dir: &Path,
    state: &mut SyncState,
    warnings: &mut Vec<String>,
) -> Result<()> {
    write_history_mirror_with_options(
        store,
        history,
        dir,
        state,
        warnings,
        &crate::playlist_mirror_options::PlaylistMirrorOptions::default(),
    )
}

fn write_history_mirror_with_options(
    store: &Store,
    history: &crate::playback_history::PlaybackHistory,
    dir: &Path,
    state: &mut SyncState,
    warnings: &mut Vec<String>,
    mirror_options: &crate::playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<()> {
    let key = "history";
    let relative_dir = mirror_relative_directory(store, state, "history", &[])?;
    let options = get_history_mirror_options(store)?;
    let input = history_playlist_input(history, options.recent_days)?;
    let documents =
        history_mirror_documents_with_options(&input, options.recent_days, mirror_options)?;
    if documents.len() > 1 {
        let active = documents
            .iter()
            .map(|document| document.key.clone())
            .collect::<HashSet<_>>();
        for document in &documents {
            if !write_history_document(state, dir, &relative_dir, document, warnings)? {
                state.paused_mirrors.insert(key.to_owned());
            }
        }
        let stale = state
            .mirror_paths
            .keys()
            .filter(|candidate| *candidate == key || candidate.starts_with("history:"))
            .filter(|candidate| !active.contains(*candidate))
            .cloned()
            .collect::<Vec<_>>();
        for stale_key in stale {
            let Some(stale_path) = state.mirror_paths.get(&stale_key).map(PathBuf::from) else {
                continue;
            };
            if let Ok(contents) = fs::read_to_string(&stale_path) {
                let hash = digest(contents.as_bytes());
                if state
                    .mirror_hashes
                    .get(&stale_key)
                    .is_some_and(|baseline| baseline == &hash)
                {
                    archive_mirror(
                        state,
                        dir,
                        "History",
                        &stale_key.replace(':', "__"),
                        &contents,
                    )?;
                    fs::remove_file(&stale_path).map_err(|error| error.to_string())?;
                    state.mirror_paths.remove(&stale_key);
                    state.mirror_hashes.remove(&stale_key);
                } else {
                    warnings.push(format!(
                        "Stale history file {} has external edits and was preserved",
                        stale_path.display()
                    ));
                    state.paused_mirrors.insert(key.to_owned());
                }
            }
        }
        return Ok(());
    }
    let document = documents.first().ok_or("History mirror has no entries")?;
    let key = document.key.as_str();
    let path = dir.join(relative_dir).join(&document.relative_path);
    let prior_path = state.mirror_paths.get(key).map(PathBuf::from);
    let source_path = prior_path.as_deref().unwrap_or(&path);
    if prior_path.as_deref().is_some_and(|prior| !prior.exists()) {
        state.paused_mirrors.insert(key.to_owned());
        state.preserve_mirrors.insert(key.to_owned());
        warnings.push("Listening History mirror was deleted; synchronization is paused until explicitly resumed".into());
        return Ok(());
    }
    let body = document.body.as_str();
    match fs::read_to_string(source_path) {
        Ok(previous) => {
            let current_hash = digest(previous.as_bytes());
            let baseline = state.mirror_hashes.get(key);
            if baseline.map_or(true, |baseline| baseline != &current_hash)
                && current_hash != digest(body.as_bytes())
            {
                state.paused_mirrors.insert(key.to_owned());
                state.preserve_mirrors.insert(key.to_owned());
                warnings.push("Listening History mirror has external edits; it was preserved and writes are paused".into());
                return Ok(());
            }
            if source_path != path && path.exists() {
                let target = fs::read_to_string(&path).unwrap_or_default();
                if state
                    .mirror_hashes
                    .get(key)
                    .map_or(true, |hash| digest(target.as_bytes()) != *hash)
                {
                    state.paused_mirrors.insert(key.to_owned());
                    state.preserve_mirrors.insert(key.to_owned());
                    warnings.push("Listening History hierarchy destination already contains an unbound file; it was preserved".into());
                    return Ok(());
                }
            }
            if previous != body {
                archive_mirror(state, dir, "History", key, &previous)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if state.mirror_hashes.contains_key(key) {
                state.paused_mirrors.insert(key.to_owned());
                state.preserve_mirrors.insert(key.to_owned());
                warnings.push("Listening History mirror was deleted; synchronization is paused until explicitly resumed".into());
                return Ok(());
            }
        }
        Err(error) => return Err(format!("Cannot read Listening History mirror: {error}")),
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if fs::read_to_string(&path).ok().as_deref() != Some(&body) {
        atomic_write(&path, body.as_bytes())?;
    }
    if source_path != path && source_path.exists() {
        archive_mirror(
            state,
            dir,
            "History",
            key,
            &fs::read_to_string(source_path).unwrap_or_default(),
        )?;
        fs::remove_file(source_path)
            .map_err(|error| format!("Cannot retire prior history mirror: {error}"))?;
    }
    state
        .mirror_hashes
        .insert(key.to_owned(), digest(body.as_bytes()));
    state
        .mirror_paths
        .insert(key.to_owned(), path.to_string_lossy().into_owned());
    Ok(())
}

fn write_history_document(
    state: &mut SyncState,
    dir: &Path,
    relative_dir: &Path,
    document: &HistoryMirrorDocument,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    let path = dir.join(relative_dir).join(&document.relative_path);
    let prior_path = state.mirror_paths.get(&document.key).map(PathBuf::from);
    let source_path = prior_path.as_deref().unwrap_or(&path);
    if prior_path.as_deref().is_some_and(|prior| !prior.exists()) {
        warnings.push(format!(
            "{} was deleted; its history mirror was preserved",
            source_path.display()
        ));
        return Ok(false);
    }
    match fs::read_to_string(source_path) {
        Ok(previous) => {
            let hash = digest(previous.as_bytes());
            if !state
                .mirror_hashes
                .get(&document.key)
                .is_some_and(|baseline| baseline == &hash)
            {
                warnings.push(format!(
                    "History file {} has external edits and was preserved",
                    source_path.display()
                ));
                return Ok(false);
            }
            if source_path != path
                && path.exists()
                && fs::read_to_string(&path).ok().as_deref() != Some(&document.body)
            {
                warnings.push(format!(
                    "History destination {} already contains an unbound file and was preserved",
                    path.display()
                ));
                return Ok(false);
            }
            if previous != document.body {
                archive_mirror(
                    state,
                    dir,
                    "History",
                    &document.key.replace(':', "__"),
                    &previous,
                )?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if state.mirror_hashes.contains_key(&document.key) {
                warnings.push(format!(
                    "{} was deleted; synchronization is paused until explicitly resumed",
                    path.display()
                ));
                return Ok(false);
            }
        }
        Err(error) => return Err(format!("Cannot read history mirror: {error}")),
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if fs::read_to_string(&path).ok().as_deref() != Some(&document.body) {
        atomic_write(&path, document.body.as_bytes())?;
    }
    if source_path != path && source_path.exists() {
        let previous = fs::read_to_string(source_path).unwrap_or_default();
        archive_mirror(
            state,
            dir,
            "History",
            &document.key.replace(':', "__"),
            &previous,
        )?;
        fs::remove_file(source_path).map_err(|error| error.to_string())?;
    }
    state
        .mirror_hashes
        .insert(document.key.clone(), digest(document.body.as_bytes()));
    state
        .mirror_paths
        .insert(document.key.clone(), path.to_string_lossy().into_owned());
    Ok(true)
}

fn render_m3u(tracks: &[Track], _music_root: &str) -> String {
    let mut out = String::from("#EXTM3U\n");
    for track in tracks {
        out.push_str(&format!(
            "#EXTINF:{},{} - {}{}\n",
            track
                .duration_seconds
                .map(|n| n.round() as i64)
                .unwrap_or(-1),
            track.artist,
            track.title,
            track
                .version
                .as_deref()
                .map(|v| format!(" ({v})"))
                .unwrap_or_default()
        ));
        let local = track.local_path.as_deref().or_else(|| {
            track
                .provenance
                .get("localPath")
                .and_then(serde_json::Value::as_str)
        });
        let entry = local.map(str::to_owned).unwrap_or_else(|| {
            let saved_entry = track
                .provenance
                .get("playlistEntry")
                .and_then(serde_json::Value::as_str);
            if let Some(raw) = saved_entry.filter(|entry| !entry.contains("://")) {
                return raw.to_owned();
            }
            let candidate_url = track
                .provenance
                .get("candidateUrl")
                .and_then(serde_json::Value::as_str)
                .or(saved_entry)
                .unwrap_or(&track.source_url);
            let known_stream = url::Url::parse(candidate_url).ok().is_some_and(|url| {
                matches!(
                    url.host_str(),
                    Some(
                        "youtube.com"
                            | "www.youtube.com"
                            | "youtu.be"
                            | "soundcloud.com"
                            | "www.soundcloud.com"
                    )
                )
            });
            if known_stream {
                candidate_url.to_owned()
            } else if let Some(raw) = saved_entry.filter(|entry| entry.contains("://")) {
                raw.to_owned()
            } else {
                let version = track
                    .version
                    .as_deref()
                    .map(|value| format!(" {value}"))
                    .unwrap_or_default();
                format!(
                    "ytdl://ytsearch1:{} {}{} mix",
                    track.artist, track.title, version
                )
            }
        });
        out.push_str(&entry);
        out.push('\n');
    }
    out
}

fn render_named_m3u(name: &str, tracks: &[Track], music_root: &str) -> String {
    render_m3u(tracks, music_root).replacen(
        "#EXTM3U\n",
        &format!("#EXTM3U\n#BURRATA-MIRROR\n#PLAYLIST:{name}\n"),
        1,
    )
}

fn playlist_title(text: &str) -> Option<String> {
    text.lines().map(str::trim).find_map(|line| {
        line.strip_prefix("#PLAYLIST:")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    })
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e| format!("Cannot write playlist {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Candidate, LocalAsset, Track};

    #[test]
    fn immutable_mirror_snapshots_are_stored_once_and_expand_for_readers() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let mut state = SyncState::default();
        state.mirror_snapshots.insert(
            "playlist-a".into(),
            vec![SourceSnapshot {
                revision: "revision-a".into(),
                captured_at: "2026-10-28T00:00:00Z".into(),
                contents: "#EXTM3U\n".repeat(1000),
            }],
        );
        persist_state(&store, &state).unwrap();
        let raw = store
            .get_json_preference("playlistSync.v1")
            .unwrap()
            .unwrap();
        assert!(raw.get("mirrorSnapshots").is_none());
        assert!(raw["mirrorSnapshotRefs"]["playlist-a"][0]["key"].is_string());
        let portable = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(
            portable["mirrorSnapshots"]["playlist-a"][0]["contents"],
            state.mirror_snapshots["playlist-a"][0].contents
        );
        assert!(portable.get("mirrorSnapshotRefs").is_none());
        persist_state(&store, &state).unwrap();
        assert_eq!(
            read_state(&store).unwrap().mirror_snapshots["playlist-a"].len(),
            1
        );
    }

    fn incremental_fixture() -> (tempfile::TempDir, Store, Settings, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let a = store.create_collection("First").unwrap().id;
        let b = store.create_collection("Second").unwrap().id;
        for id in [&a, &b] {
            let track = Track {
                id: format!("local:track-{id}"),
                provider_id: format!("track-{id}"),
                title: "Track".into(),
                artist: "Artist".into(),
                provider: "local".into(),
                ..Default::default()
            };
            store.save_track(&track).unwrap();
            store.add_to_collection(id, &[track]).unwrap();
        }
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_mirroring_enabled: true,
            ..Default::default()
        };
        for _ in 0..4 {
            sync_playlists(&store, &settings).unwrap();
        }
        (dir, store, settings, a, b)
    }

    #[test]
    fn incremental_journal_ignores_bookmarks_and_unrelated_catalog_cache() {
        let (_dir, store, _settings, a, b) = incremental_fixture();
        let generation = store.playlist_sync_generation().unwrap();
        store
            .set_json_preference(
                "nativePlaybackSession.v1",
                &serde_json::json!({"position":42}),
            )
            .unwrap();
        store
            .cache_track(&Track {
                id: "fixture:unrelated".into(),
                provider_id: "unrelated".into(),
                provider: "fixture".into(),
                title: "Unrelated".into(),
                artist: "Artist".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.playlist_sync_generation().unwrap(), generation);
        let a_before = store.playlist_mirror_revision(&a).unwrap();
        let b_before = store.playlist_mirror_revision(&b).unwrap();
        store.rename_collection(&a, "Changed").unwrap();
        assert_ne!(store.playlist_mirror_revision(&a).unwrap(), a_before);
        assert_eq!(store.playlist_mirror_revision(&b).unwrap(), b_before);
    }

    #[test]
    fn runtime_sync_does_not_expand_archived_snapshot_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let mut state = SyncState::default();
        state.mirror_snapshots.insert(
            "playlist".into(),
            vec![SourceSnapshot {
                revision: "first".into(),
                captured_at: "2026-10-28T00:00:00Z".into(),
                contents: "#EXTM3U\n".repeat(1000),
            }],
        );
        persist_state(&store, &state).unwrap();
        let mut light = read_runtime_state(&store).unwrap();
        assert!(light.mirror_snapshots.is_empty());
        assert_eq!(light.mirror_snapshot_refs["playlist"].len(), 1);
        // Saving a runtime mutation retains the original immutable body.
        light
            .mirror_hashes
            .insert("playlist".into(), "changed".into());
        persist_state(&store, &light).unwrap();
        assert_eq!(
            read_state(&store).unwrap().mirror_snapshots["playlist"][0].contents,
            "#EXTM3U\n".repeat(1000)
        );
        // References written by the earlier build migrate without loading bodies.
        let mut raw = store
            .get_json_preference("playlistSync.v1")
            .unwrap()
            .unwrap();
        let key = raw["mirrorSnapshotRefs"]["playlist"][0]["key"].clone();
        raw["mirrorSnapshotRefs"]["playlist"][0] = key;
        store.set_json_preference("playlistSync.v1", &raw).unwrap();
        let migrated = read_runtime_state(&store).unwrap();
        assert_eq!(
            migrated.mirror_snapshot_refs["playlist"][0].revision,
            "first"
        );
        assert!(migrated.mirror_snapshots.is_empty());
    }

    #[test]
    fn incremental_failed_pass_discards_partial_cache_before_retry() {
        let (_dir, store, settings, _, _) = incremental_fixture();
        assert!(!store
            .playlist_incremental
            .lock()
            .unwrap()
            .mirrors
            .is_empty());
        let root = Path::new(&settings.music_directory).join("Playlists");
        fs::rename(&root, root.with_extension("saved")).unwrap();
        fs::write(&root, b"not a directory").unwrap();
        assert!(sync_playlists(&store, &settings).is_err());
        let cache = store.playlist_incremental.lock().unwrap();
        assert!(cache.pass.is_none());
        assert!(cache.mirrors.is_empty());
    }

    #[test]
    fn history_export_ids_do_not_shift_when_newer_sessions_arrive() {
        let directory = tempfile::tempdir().unwrap();
        let history = crate::playback_history::PlaybackHistory::open(
            &directory.path().join("history.sqlite"),
        )
        .unwrap();
        let track = Track {
            id: "local:history".into(),
            provider: "local".into(),
            provider_id: "history".into(),
            title: "History".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        history.record("first", "local", &track, None).unwrap();
        let before = history_playlist_input(&history, None).unwrap();
        let occurrence = before.entries[0]
            .history
            .as_ref()
            .unwrap()
            .occurrence_id
            .clone();
        history.record("second", "local", &track, None).unwrap();
        let after = history_playlist_input(&history, None).unwrap();
        let retained = after
            .entries
            .iter()
            .find(|entry| entry.history.as_ref().unwrap().occurrence_id == occurrence)
            .unwrap();
        assert_eq!(retained.id, before.entries[0].id);
    }

    #[test]
    fn incremental_idle_sync_has_no_writes_or_mirror_work() {
        let (_dir, store, settings, _, _) = incremental_fixture();
        let before = store
            .with_backup_connection(|conn| Ok(conn.total_changes()))
            .unwrap();
        let work = store.playlist_incremental.lock().unwrap().mirror_work;
        for _ in 0..3 {
            sync_playlists(&store, &settings).unwrap();
        }
        assert_eq!(
            store
                .with_backup_connection(|conn| Ok(conn.total_changes()))
                .unwrap(),
            before
        );
        let cache = store.playlist_incremental.lock().unwrap();
        assert_eq!(cache.mirror_work, work);
        assert!(cache.noops >= 3);
    }

    #[test]
    fn incremental_rename_only_renders_affected_playlist_and_detects_external_edits() {
        let (_dir, store, settings, a, b) = incremental_fixture();
        let before = store.playlist_incremental.lock().unwrap().mirror_work;
        let b_path = read_state(&store).unwrap().mirror_paths[&b].clone();
        let b_before = fs::metadata(&b_path).unwrap().modified().unwrap();
        store.rename_collection(&a, "Renamed").unwrap();
        sync_playlists(&store, &settings).unwrap();
        assert_eq!(
            store.playlist_incremental.lock().unwrap().mirror_work - before,
            1
        );
        assert_eq!(fs::metadata(&b_path).unwrap().modified().unwrap(), b_before);
        let body = fs::read_to_string(&b_path).unwrap();
        fs::write(&b_path, body.replace("Second", "External edit")).unwrap();
        sync_playlists(&store, &settings).unwrap();
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .into_iter()
                .find(|c| c.id == b)
                .unwrap()
                .name,
            "External edit"
        );
    }

    fn restored_sync_state() -> serde_json::Value {
        serde_json::json!({
            "sources": {},
            "mirrorHashes": {},
            "mirrorPaths": {},
            "mirrorSnapshots": {},
            "protectedCopies": {},
            "writesPaused": true,
            "backupProjection": {
                "schemaVersion": 1,
                "writesPaused": true,
                "sources": [{"bindingId":"b-123456789012345678901234","relinkHint":{"basename":"mix.m3u8"},"snapshots":[]}],
                "mirrors": [],
                "sectionMirrors": [],
                "protectedCopies": []
            }
        })
    }

    fn restored_history_mirror_state(hash: &str, basename: &str) -> serde_json::Value {
        let mut state = restored_sync_state();
        state["backupProjection"]["mirrors"] = serde_json::json!([{
            "collectionId": "history",
            "hash": hash,
            "relinkHint": {"basename": basename},
            "snapshots": []
        }]);
        state
    }

    #[test]
    fn excluded_mirrors_freeze_collection_favorites_and_history_files_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let history =
            crate::playback_history::PlaybackHistory::open(&dir.path().join("history.sqlite"))
                .unwrap();
        let music = dir.path().join("Music");
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let collection = store.create_collection("Policy fixture").unwrap();
        let track = Track {
            id: "fixture:policy-fixture-track".into(),
            provider: "fixture".into(),
            provider_id: "policy-fixture-track".into(),
            title: "Policy fixture track".into(),
            artist: "Qualification artist".into(),
            ..Track::default()
        };
        store
            .replace_playlist_entries(&collection.id, &[track.clone()])
            .unwrap();
        history
            .record("policy-fixture-session", "fixture", &track, None)
            .unwrap();

        let mut state = SyncState::default();
        let mut warnings = Vec::new();
        write_mirrors(&store, &settings, &mut state, &mut warnings, Some(&history)).unwrap();
        let collection_path = PathBuf::from(&state.mirror_paths[&collection.id]);
        let favorites_path = PathBuf::from(&state.mirror_paths["favorites"]);
        let history_path = PathBuf::from(&state.mirror_paths["history"]);
        let files = [collection_path, favorites_path, history_path];
        let edited = files
            .iter()
            .map(|path| {
                let contents =
                    format!("{}\n#BURRATA-TRACK:{{\n", fs::read_to_string(path).unwrap());
                fs::write(path, &contents).unwrap();
                contents
            })
            .collect::<Vec<_>>();
        let before_state = serde_json::to_value(&state).unwrap();
        for id in [collection.id.as_str(), "favorites", "history"] {
            crate::playlist_mirror_options::set_playlist_mirror_options(
                &store,
                id,
                crate::playlist_mirror_options::PlaylistMirrorOptions {
                    included: false,
                    ..Default::default()
                },
            )
            .unwrap();
        }

        write_mirrors(&store, &settings, &mut state, &mut warnings, Some(&history)).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), before_state);
        for (path, contents) in files.iter().zip(&edited) {
            assert_eq!(fs::read_to_string(path).unwrap(), *contents);
        }

        crate::playlist_mirror_options::set_playlist_mirror_options(
            &store,
            &collection.id,
            crate::playlist_mirror_options::PlaylistMirrorOptions::default(),
        )
        .unwrap();
        write_mirrors(&store, &settings, &mut state, &mut warnings, Some(&history)).unwrap();
        assert_eq!(fs::read_to_string(&files[0]).unwrap(), edited[0]);
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("kept for review")));
    }

    #[test]
    fn excluded_personal_target_blocks_origin_mirror_before_file_or_state_access() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let music = dir.path().join("Music");
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let original = store.create_collection("Protected source").unwrap();
        let original_track = Track {
            id: "fixture:protected-source-track".into(),
            provider: "fixture".into(),
            provider_id: "protected-source-track".into(),
            title: "Protected source track".into(),
            artist: "Qualification artist".into(),
            ..Track::default()
        };
        store
            .replace_playlist_entries(&original.id, &[original_track])
            .unwrap();
        let personal = store
            .create_collection("Protected source personal copy")
            .unwrap();
        let personal_track = Track {
            id: "fixture:protected-personal-track".into(),
            provider: "fixture".into(),
            provider_id: "protected-personal-track".into(),
            title: "Personal copy track".into(),
            artist: "Qualification artist".into(),
            ..Track::default()
        };
        store
            .replace_playlist_entries(&personal.id, &[personal_track])
            .unwrap();

        let mut state = SyncState::default();
        let mut warnings = Vec::new();
        write_mirrors(&store, &settings, &mut state, &mut warnings, None).unwrap();
        let origin_path = PathBuf::from(&state.mirror_paths[&original.id]);
        let personal_path = PathBuf::from(&state.mirror_paths[&personal.id]);
        let personal_body = fs::read_to_string(&personal_path).unwrap();
        let external_edit = format!(
            "{}\n# external edit that must not be read while its mapped destination is excluded\n",
            fs::read_to_string(&origin_path).unwrap()
        );
        fs::write(&origin_path, &external_edit).unwrap();
        state
            .protected_copies
            .insert(original.id.clone(), personal.id.clone());
        crate::playlist_mirror_options::set_playlist_mirror_options(
            &store,
            "favorites",
            crate::playlist_mirror_options::PlaylistMirrorOptions {
                included: false,
                ..Default::default()
            },
        )
        .unwrap();
        crate::playlist_mirror_options::set_playlist_mirror_options(
            &store,
            &personal.id,
            crate::playlist_mirror_options::PlaylistMirrorOptions {
                included: false,
                ..Default::default()
            },
        )
        .unwrap();
        let before_state = serde_json::to_value(&state).unwrap();

        write_mirrors(&store, &settings, &mut state, &mut warnings, None).unwrap();

        assert_eq!(fs::read_to_string(&origin_path).unwrap(), external_edit);
        assert_eq!(serde_json::to_value(&state).unwrap(), before_state);
        assert_eq!(
            fs::read_to_string(&personal_path).unwrap(),
            personal_body,
            "exclusion must leave the existing derived mirror in place"
        );
        assert!(!music.join("Playlists/Archives").exists());
    }

    #[test]
    fn mirror_options_view_detects_legacy_search_fallback_only_on_bound_managed_m3u() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let music = dir.path().join("Music");
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        store.save_settings(&settings).unwrap();
        let collection = store.create_collection("Legacy format fixture").unwrap();
        let track = Track {
            id: "fixture:legacy-format-fixture-track".into(),
            provider: "fixture".into(),
            provider_id: "legacy-format-fixture-track".into(),
            title: "Legacy fallback fixture".into(),
            artist: "Qualification artist".into(),
            ..Track::default()
        };
        store
            .replace_playlist_entries(&collection.id, &[track.clone()])
            .unwrap();
        let mut state = SyncState::default();
        let mut warnings = Vec::new();
        write_mirrors(&store, &settings, &mut state, &mut warnings, None).unwrap();
        let path = PathBuf::from(&state.mirror_paths[&collection.id]);

        let legacy = render_named_m3u(
            &collection.name,
            &[track.clone()],
            &settings.music_directory,
        );
        fs::write(&path, &legacy).unwrap();
        state
            .mirror_hashes
            .insert(collection.id.clone(), digest(legacy.as_bytes()));
        persist_state(&store, &state).unwrap();
        let view =
            crate::playlist_mirror_options::get_playlist_mirror_options(&store, &collection.id)
                .unwrap();
        assert!(view.defaults.include_ytdlp_search);
        assert!(view.effective.include_ytdlp_search);

        let portable = render_managed_playlist_m3u(
            &store,
            &collection.id,
            &collection.name,
            &[track],
            path.parent().unwrap(),
        )
        .unwrap();
        fs::write(&path, &portable).unwrap();
        state
            .mirror_hashes
            .insert(collection.id.clone(), digest(portable.as_bytes()));
        persist_state(&store, &state).unwrap();
        let view =
            crate::playlist_mirror_options::get_playlist_mirror_options(&store, &collection.id)
                .unwrap();
        assert!(!view.defaults.include_ytdlp_search);
        assert!(!view.effective.include_ytdlp_search);
    }

    fn history_mirror_fixture(
        root: &Path,
    ) -> (
        Store,
        crate::playback_history::PlaybackHistory,
        Settings,
        PathBuf,
        String,
    ) {
        let store = Store::open(&root.join("db.sqlite")).unwrap();
        let history =
            crate::playback_history::PlaybackHistory::open(&root.join("history.sqlite")).unwrap();
        let track = Track {
            id: "fixture-history-track".into(),
            provider: "fixture".into(),
            title: "Synthetic History Entry".into(),
            artist: "Qualification Artist".into(),
            duration_seconds: Some(20.0),
            ..Track::default()
        };
        history
            .record("fixture-history-session", "fixture", &track, None)
            .unwrap();
        let input = history_playlist_input(&history, None).unwrap();
        let document = history_mirror_documents(&input, None).unwrap();
        assert_eq!(document.len(), 1);
        let music = root.join("CurrentMusic");
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let directory = mirrors_dir(&settings).unwrap();
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Listening History.m3u8");
        let body = document[0].body.clone();
        (store, history, settings, path, body)
    }

    fn section_mirror_relink_fixture(
        root: &Path,
    ) -> (Store, Settings, String, String, PathBuf, String) {
        let store = Store::open(&root.join("db.sqlite")).unwrap();
        let track = Track {
            id: "fixture:section-relink-track".into(),
            provider: "fixture".into(),
            provider_id: "section-relink-track".into(),
            title: "Synthetic Section Track".into(),
            artist: "Qualification Artist".into(),
            duration_seconds: Some(30.0),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let collection = store.create_collection("Section Relink Playlist").unwrap();
        store
            .replace_playlist_entries(&collection.id, std::slice::from_ref(&track))
            .unwrap();
        crate::playlist_hierarchy::set_folder_ancestry(
            &store,
            &collection.id,
            &["Relink Sets".into(), "Nested".into()],
        )
        .unwrap();
        crate::playlist_hierarchy::set_sections(
            &store,
            &collection.id,
            &[crate::portable_playlist::NamedItem {
                name: "Side A".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let mut hierarchy = crate::playlist_hierarchy::load(&store).unwrap();
        let placement = hierarchy
            .playlists
            .iter_mut()
            .find(|placement| placement.collection_id == collection.id)
            .unwrap();
        let section_id = placement.sections[0].id.clone();
        let occurrence_id = store.list_playlist_occurrences(&collection.id).unwrap()[0]
            .occurrence_id
            .clone();
        if let Some(metadata) = placement
            .occurrences
            .iter_mut()
            .find(|metadata| metadata.id == occurrence_id)
        {
            metadata.section_id = Some(section_id.clone());
        } else {
            placement
                .occurrences
                .push(crate::playlist_hierarchy::OccurrenceMetadata {
                    id: occurrence_id,
                    section_id: Some(section_id.clone()),
                    ..Default::default()
                });
        }
        crate::playlist_hierarchy::save(&store, &hierarchy).unwrap();

        let music = root.join("CurrentMusic");
        fs::create_dir_all(&music).unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_mirroring_enabled: true,
            ..Settings::default()
        };
        let mut state = SyncState::default();
        let mut warnings = Vec::new();
        write_mirrors(&store, &settings, &mut state, &mut warnings, None).unwrap();
        assert!(warnings.is_empty());
        let key = section_mirror_key(&collection.id, &section_id);
        let path = PathBuf::from(&state.mirror_paths[&key]);
        let body = fs::read_to_string(&path).unwrap();
        let hash = digest(body.as_bytes());
        let projected_contents = body.clone();
        // A backup's original file may contain private data that was removed
        // from the share-safe projected snapshot. Preserve its hash as
        // provenance, but never use it as the active snapshot revision.
        let original_contents = format!("{body}\n# private source-only metadata\n");
        let original_hash = digest(original_contents.as_bytes());
        assert_ne!(original_hash, digest(projected_contents.as_bytes()));
        let projection = serde_json::json!({
            "schemaVersion": 1,
            "writesPaused": true,
            "sources": [],
            "mirrors": [],
            "sectionMirrors": [{
                "collectionId": collection.id,
                "sectionId": section_id,
                "hash": hash,
                "relinkHint": {"basename": path.file_name().unwrap().to_string_lossy()},
                "snapshots": [{
                    "sourceOriginalSha256": original_hash,
                    "contentsSha256": digest(projected_contents.as_bytes()),
                    "capturedAt": "2026-10-27T00:00:00Z",
                    "contents": projected_contents,
                }]
            }],
            "protectedCopies": []
        });
        store
            .save_playlist_sync_state(&serde_json::json!({
                "sources": {},
                "mirrorHashes": {},
                "mirrorPaths": {},
                "mirrorSnapshots": {},
                "protectedCopies": {},
                "writesPaused": true,
                "backupProjection": projection,
            }))
            .unwrap();
        (store, settings, collection.id, section_id, path, body)
    }

    #[test]
    fn reviewed_restore_relinks_exact_section_mirror_and_repeat_sync_is_stable() {
        let temp = tempfile::tempdir().unwrap();
        let (store, settings, collection_id, section_id, path, body) =
            section_mirror_relink_fixture(temp.path());
        let preview = preview_restored_playlist_sync(&store, &settings).unwrap();
        assert_eq!(preview.section_mirror_relinks.len(), 1);
        assert_eq!(
            preview.section_mirror_relinks[0].playlist_name,
            "Section Relink Playlist"
        );
        assert_eq!(preview.section_mirror_relinks[0].section_name, "Side A");
        assert_eq!(
            preview.section_mirror_relinks[0].relative_path,
            format!("Relink Sets/Nested/{collection_id}/Sections/{section_id}.m3u8")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), body);

        resume_reviewed_restored_playlist_sync(&store, &settings, &preview.review_token).unwrap();
        let key = section_mirror_key(&collection_id, &section_id);
        let state = read_state(&store).unwrap();
        assert_eq!(state.mirror_paths[&key], path.to_string_lossy().as_ref());
        assert_eq!(state.mirror_hashes[&key], digest(body.as_bytes()));
        let snapshots =
            &state.mirror_snapshots[&section_mirror_snapshot_key(&collection_id, &section_id)];
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].revision, digest(body.as_bytes()));
        let source_original_hash = state.backup_projection.as_ref().unwrap()["sectionMirrors"][0]
            ["snapshots"][0]["sourceOriginalSha256"]
            .as_str()
            .unwrap();
        assert_ne!(snapshots[0].revision, source_original_hash);
        assert_eq!(
            source_original_hash,
            digest(format!("{body}\n# private source-only metadata\n").as_bytes())
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            body,
            "review and resume must not write files"
        );

        let repeated = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        assert!(!repeated
            .warnings
            .iter()
            .any(|warning| warning.contains("Unbound section file")));
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
        let repeated_state = read_state(&store).unwrap();
        assert_eq!(repeated_state.mirror_hashes[&key], digest(body.as_bytes()));
    }

    #[test]
    fn reviewed_restore_migrates_legacy_section_mirror_projection_to_safe_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let (store, settings, collection_id, section_id, path, body) =
            section_mirror_relink_fixture(temp.path());
        let mut restored_state = store.playlist_sync_state().unwrap().unwrap();
        let projection = restored_state
            .get_mut("backupProjection")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap();
        projection.insert(
            "mirrors".into(),
            serde_json::json!([{
                "collectionId": section_mirror_key(&collection_id, &section_id),
                "hash": digest(body.as_bytes()),
                "relinkHint": {"basename": path.file_name().unwrap().to_string_lossy()},
                "snapshots": []
            }]),
        );
        projection.remove("sectionMirrors");
        store.save_playlist_sync_state(&restored_state).unwrap();

        let preview = preview_restored_playlist_sync(&store, &settings).unwrap();
        assert_eq!(preview.section_mirror_relinks.len(), 1);
        assert_eq!(preview.section_mirror_relinks[0].section_name, "Side A");
        resume_reviewed_restored_playlist_sync(&store, &settings, &preview.review_token).unwrap();
        let key = section_mirror_key(&collection_id, &section_id);
        let state = read_state(&store).unwrap();
        assert_eq!(state.mirror_hashes[&key], digest(body.as_bytes()));
        assert_eq!(state.mirror_paths[&key], path.to_string_lossy().as_ref());
        let migrated = state.backup_projection.unwrap();
        assert!(migrated["mirrors"].as_array().unwrap().is_empty());
        assert_eq!(migrated["sectionMirrors"][0]["collectionId"], collection_id);
        assert_eq!(migrated["sectionMirrors"][0]["sectionId"], section_id);
        assert_eq!(
            migrated["sectionMirrors"][0]["hash"],
            digest(body.as_bytes())
        );
        assert_eq!(
            migrated["sectionMirrors"][0]["relinkHint"]["basename"],
            path.file_name().unwrap().to_string_lossy().as_ref()
        );
    }

    #[test]
    fn reviewed_section_relink_rejects_stale_file_root_and_hierarchy() {
        let temp = tempfile::tempdir().unwrap();
        let (store, settings, _, _, path, _) = section_mirror_relink_fixture(temp.path());
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        fs::write(&path, "#EXTM3U\n# external edit\n").unwrap();
        assert!(
            resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token)
                .unwrap_err()
                .contains("review it again")
        );
        assert!(read_state(&store).unwrap().writes_paused);

        let temp = tempfile::tempdir().unwrap();
        let (store, settings, _, _, _, _) = section_mirror_relink_fixture(temp.path());
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        let changed_settings = Settings {
            music_directory: temp
                .path()
                .join("OtherMusic")
                .to_string_lossy()
                .into_owned(),
            ..settings.clone()
        };
        assert!(resume_reviewed_restored_playlist_sync(
            &store,
            &changed_settings,
            &review.review_token
        )
        .unwrap_err()
        .contains("review it again"));
        assert!(read_state(&store).unwrap().writes_paused);

        let temp = tempfile::tempdir().unwrap();
        let (store, settings, _, _, path, body) = section_mirror_relink_fixture(temp.path());
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        let root = mirrors_dir(&settings).unwrap();
        let detached_root = temp.path().join("DetachedPlaylists");
        let relative_path = path.strip_prefix(&root).unwrap().to_path_buf();
        fs::rename(&root, &detached_root).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(detached_root.join(&relative_path), &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
        assert!(
            resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token)
                .unwrap_err()
                .contains("review it again")
        );
        assert!(read_state(&store).unwrap().writes_paused);

        let temp = tempfile::tempdir().unwrap();
        let (store, settings, collection_id, _, _, _) = section_mirror_relink_fixture(temp.path());
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        crate::playlist_hierarchy::set_sections(
            &store,
            &collection_id,
            &[crate::portable_playlist::NamedItem {
                name: "Renamed Side".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        assert!(
            resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token)
                .unwrap_err()
                .contains("review it again")
        );
        assert!(read_state(&store).unwrap().writes_paused);
    }

    #[cfg(unix)]
    #[test]
    fn restored_section_relink_leaves_missing_edited_and_symlink_files_unbound() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let (store, settings, collection_id, section_id, path, _) =
            section_mirror_relink_fixture(temp.path());
        let edited = "#EXTM3U\n# unrelated section file\n";
        fs::write(&path, edited).unwrap();
        let preview = preview_restored_playlist_sync(&store, &settings).unwrap();
        assert!(preview.section_mirror_relinks.is_empty());
        resume_reviewed_restored_playlist_sync(&store, &settings, &preview.review_token).unwrap();
        let edited_state = read_state(&store).unwrap();
        let key = section_mirror_key(&collection_id, &section_id);
        assert!(!edited_state.mirror_paths.contains_key(&key));
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);

        let temp = tempfile::tempdir().unwrap();
        let (store, settings, collection_id, section_id, path, body) =
            section_mirror_relink_fixture(temp.path());
        fs::remove_file(&path).unwrap();
        let preview = preview_restored_playlist_sync(&store, &settings).unwrap();
        assert!(preview.section_mirror_relinks.is_empty());
        resume_reviewed_restored_playlist_sync(&store, &settings, &preview.review_token).unwrap();
        let missing_state = read_state(&store).unwrap();
        let key = section_mirror_key(&collection_id, &section_id);
        assert!(!missing_state.mirror_paths.contains_key(&key));
        assert!(!path.exists());

        let temp = tempfile::tempdir().unwrap();
        let (store, settings, collection_id, section_id, path, body) =
            section_mirror_relink_fixture(temp.path());
        let outside = temp.path().join("outside-section.m3u8");
        fs::write(&outside, &body).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        let preview = preview_restored_playlist_sync(&store, &settings).unwrap();
        assert!(preview.section_mirror_relinks.is_empty());
        resume_reviewed_restored_playlist_sync(&store, &settings, &preview.review_token).unwrap();
        let symlink_state = read_state(&store).unwrap();
        let key = section_mirror_key(&collection_id, &section_id);
        assert!(!symlink_state.mirror_paths.contains_key(&key));
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(outside).unwrap(), body);
    }

    #[test]
    fn restored_projection_survives_runtime_state_rewrites_and_mirror_sync_stays_paused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let original = restored_sync_state();
        store.save_playlist_sync_state(&original).unwrap();
        let projection = original["backupProjection"].clone();

        // Ordinary source-state persistence must keep the archive-only projection.
        let state = read_state(&store).unwrap();
        persist_state(&store, &state).unwrap();
        let persisted = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(persisted["backupProjection"], projection);
        assert_eq!(persisted["writesPaused"], true);

        // The normal synchronization entry point must not create/write mirrors
        // or import newly discovered sources merely because the restored profile
        // starts its playlist watcher.
        let music = dir.path().join("Music");
        let incoming = music.join("Incoming");
        fs::create_dir_all(&incoming).unwrap();
        fs::write(
            incoming.join("new.m3u8"),
            "#EXTM3U\n#EXTINF:180,Artist - New\nfile.flac\n",
        )
        .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![incoming.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let status = sync_playlists(&store, &settings).unwrap();
        assert!(status.mirrors_directory.is_none());
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("paused after restore")));
        assert!(!music.join("Playlists").exists());
        assert!(store.list_collections().unwrap().is_empty());
        assert!(resume_playlist_mirror(&store, &settings, "favorites").is_err());
        assert!(!music.join("Playlists").exists());
        let persisted = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(persisted["backupProjection"], projection);
        assert_eq!(persisted["writesPaused"], true);
    }

    #[test]
    fn restore_exclusive_permit_rejects_playlist_sync_and_mirror_resume_before_effects() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let mut settings = store.settings().unwrap();
        settings.music_directory = dir.path().join("Music").to_string_lossy().into_owned();
        let scan = playlist_files::LocalPlaylistScan {
            files: Vec::new(),
            roots: Vec::new(),
            warnings: Vec::new(),
        };
        let restore = store.try_restore_permit().unwrap();

        assert!(sync_playlists_with_scan(&store, &settings, scan).is_err());
        assert!(resume_playlist_mirror(&store, &settings, "favorites").is_err());
        assert!(store.playlist_sync_state().unwrap().is_none());
        assert!(!Path::new(&settings.music_directory)
            .join("Playlists")
            .exists());

        drop(restore);
    }

    #[test]
    fn restored_projection_requires_explicit_resume_and_remains_inert_afterward() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let original = restored_sync_state();
        store.save_playlist_sync_state(&original).unwrap();
        let projection = original["backupProjection"].clone();

        resume_restored_playlist_sync(&store).unwrap();

        let persisted = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(persisted["writesPaused"], false);
        assert_eq!(persisted["backupProjection"], projection);
        let state = read_state(&store).unwrap();
        assert!(!state.writes_paused);
        assert!(state.backup_projection.is_some());
    }

    #[test]
    fn reviewed_resume_is_bound_to_current_music_folder_and_keeps_projection_inert() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let original = restored_sync_state();
        store.save_playlist_sync_state(&original).unwrap();
        let projection = original["backupProjection"].clone();
        let settings = Settings {
            music_directory: dir
                .path()
                .join("CurrentMusic")
                .to_string_lossy()
                .into_owned(),
            ..Settings::default()
        };

        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        let expected_directory = dir
            .path()
            .join("CurrentMusic")
            .join("Playlists")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            review.current_mirror_directory.as_deref(),
            Some(expected_directory.as_str())
        );
        assert_eq!(review.source_count, 1);
        assert_eq!(review.mirror_count, 0);
        assert_eq!(review.protected_copy_count, 0);

        let changed_settings = Settings {
            music_directory: dir
                .path()
                .join("ChangedMusic")
                .to_string_lossy()
                .into_owned(),
            ..Settings::default()
        };
        assert!(resume_reviewed_restored_playlist_sync(
            &store,
            &changed_settings,
            &review.review_token
        )
        .unwrap_err()
        .contains("review it again"));
        assert_eq!(
            store.playlist_sync_state().unwrap().unwrap()["writesPaused"],
            true
        );

        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let persisted = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(persisted["writesPaused"], false);
        assert_eq!(persisted["backupProjection"], projection);
    }

    #[test]
    fn reviewed_resume_binds_exact_history_mirror_but_startup_sync_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let (store, history, settings, path, body) = history_mirror_fixture(dir.path());
        fs::write(&path, &body).unwrap();
        let expected_hash = digest(body.as_bytes());
        store
            .save_playlist_sync_state(&restored_history_mirror_state(
                &expected_hash,
                "Listening History.m3u8",
            ))
            .unwrap();

        let startup = sync_playlists_with_history(&store, &settings, &history).unwrap();
        assert!(startup
            .warnings
            .iter()
            .any(|warning| warning.contains("paused after restore")));
        let paused = store.playlist_sync_state().unwrap().unwrap();
        assert!(paused["mirrorHashes"].get("history").is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), body);

        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let bound = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(bound["mirrorHashes"]["history"], expected_hash);
        assert_eq!(
            bound["mirrorPaths"]["history"],
            path.to_string_lossy().as_ref()
        );
        assert_eq!(
            bound["backupProjection"]["mirrors"][0]["hash"],
            expected_hash
        );

        let resumed = sync_playlists_with_history(&store, &settings, &history).unwrap();
        assert!(!resumed
            .warnings
            .iter()
            .any(|warning| { warning.contains("Listening History mirror has external edits") }));
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
    }

    #[test]
    fn reviewed_resume_leaves_modified_projected_history_file_unbound_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let (store, history, settings, path, body) = history_mirror_fixture(dir.path());
        let changed = format!("{body}# external fixture edit\n");
        fs::write(&path, &changed).unwrap();
        let expected_hash = digest(body.as_bytes());
        store
            .save_playlist_sync_state(&restored_history_mirror_state(
                &expected_hash,
                "Listening History.m3u8",
            ))
            .unwrap();
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let unbound = store.playlist_sync_state().unwrap().unwrap();
        assert!(unbound["mirrorHashes"].get("history").is_none());
        assert!(unbound["mirrorPaths"].get("history").is_none());

        let status = sync_playlists_with_history(&store, &settings, &history).unwrap();
        assert!(status
            .warnings
            .iter()
            .any(|warning| { warning.contains("Listening History mirror has external edits") }));
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
    }

    #[test]
    fn reviewed_resume_leaves_history_mirror_with_wrong_relink_hint_unbound() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _history, settings, path, body) = history_mirror_fixture(dir.path());
        fs::write(&path, &body).unwrap();
        let expected_hash = digest(body.as_bytes());
        store
            .save_playlist_sync_state(&restored_history_mirror_state(
                &expected_hash,
                "Listening History — Recent 30 days.m3u8",
            ))
            .unwrap();
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let unbound = store.playlist_sync_state().unwrap().unwrap();
        assert!(unbound["mirrorHashes"].get("history").is_none());
        assert!(unbound["mirrorPaths"].get("history").is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
    }

    #[cfg(unix)]
    #[test]
    fn reviewed_resume_does_not_bind_symlink_history_destination() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let (store, _history, settings, path, body) = history_mirror_fixture(dir.path());
        let target = dir.path().join("outside-history-fixture.m3u8");
        fs::write(&target, &body).unwrap();
        symlink(&target, &path).unwrap();
        let expected_hash = digest(body.as_bytes());
        store
            .save_playlist_sync_state(&restored_history_mirror_state(
                &expected_hash,
                "Listening History.m3u8",
            ))
            .unwrap();
        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let unbound = store.playlist_sync_state().unwrap().unwrap();
        assert!(unbound["mirrorHashes"].get("history").is_none());
        assert!(unbound["mirrorPaths"].get("history").is_none());
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), body);
    }

    #[test]
    fn reviewed_resume_accepts_history_file_at_serialized_limit_plus_header() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _history, settings, path, _) = history_mirror_fixture(dir.path());
        let size = MAX_HISTORY_MIRROR_BYTES as usize;
        let body = format!("#EXTM3U\n#{}", "x".repeat(size - "#EXTM3U\n#".len()));
        assert_eq!(body.len() as u64, MAX_HISTORY_MIRROR_BYTES);
        fs::write(&path, &body).unwrap();
        let expected_hash = digest(body.as_bytes());
        store
            .save_playlist_sync_state(&restored_history_mirror_state(
                &expected_hash,
                "Listening History.m3u8",
            ))
            .unwrap();

        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let bound = store.playlist_sync_state().unwrap().unwrap();
        assert_eq!(bound["mirrorHashes"]["history"], expected_hash);
        assert_eq!(fs::metadata(path).unwrap().len(), MAX_HISTORY_MIRROR_BYTES);
    }

    #[test]
    fn reviewed_resume_rejects_history_file_one_byte_above_supported_output_limit() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _history, settings, path, _) = history_mirror_fixture(dir.path());
        let size = MAX_HISTORY_MIRROR_BYTES as usize + 1;
        let body = format!("#EXTM3U\n#{}", "x".repeat(size - "#EXTM3U\n#".len()));
        assert_eq!(body.len() as u64, MAX_HISTORY_MIRROR_BYTES + 1);
        fs::write(&path, &body).unwrap();
        let expected_hash = digest(body.as_bytes());
        store
            .save_playlist_sync_state(&restored_history_mirror_state(
                &expected_hash,
                "Listening History.m3u8",
            ))
            .unwrap();

        let review = preview_restored_playlist_sync(&store, &settings).unwrap();
        resume_reviewed_restored_playlist_sync(&store, &settings, &review.review_token).unwrap();
        let unbound = store.playlist_sync_state().unwrap().unwrap();
        assert!(unbound["mirrorHashes"].get("history").is_none());
        assert!(unbound["mirrorPaths"].get("history").is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
    }

    #[test]
    fn malformed_restored_projection_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        store
            .save_playlist_sync_state(&serde_json::json!({
                "writesPaused": true,
                "backupProjection": {"schemaVersion": 9}
            }))
            .unwrap();
        assert!(read_state(&store)
            .unwrap_err()
            .contains("writes remain paused"));
    }

    #[test]
    fn virtual_favorites_can_be_shared_as_a_privacy_safe_portable_playlist() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let track = Track {
            id: "spotify:private-provider-id".into(),
            provider: "spotify".into(),
            provider_id: "private-provider-id".into(),
            source_url: "https://open.spotify.com/track/private-provider-id".into(),
            artist: "Artist".into(),
            title: "Title".into(),
            local_path: Some("/private/library/song.flac".into()),
            favorite: true,
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        store.set_favorite(&track, true).unwrap();

        let input = portable_playlist_input(&store, "favorites").unwrap();
        assert_eq!(input.name, "Liked Songs");
        assert_eq!(input.entries.len(), 1);
        let options = crate::portable_playlist::PlaylistExportOptions::default();
        let preview = crate::portable_playlist::preview_export(&input, &options).unwrap();
        let rendered = render_portable_m3u(&input, &options).unwrap();
        assert_eq!(preview.identity_placeholders, 1);
        assert_eq!(preview.search_links, 0);
        assert!(rendered.contains("burrata://track/"));
        assert!(!rendered.contains("private-listing-id"));
        assert!(!rendered.contains("private-provider-id"));
        assert!(!rendered.contains("/private/library"));
    }

    #[test]
    fn disabled_mirroring_leaves_managed_files_state_and_source_files_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let source_dir = dir.path().join("SourcePlaylists");
        fs::create_dir_all(&music).unwrap();
        fs::create_dir_all(&source_dir).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let history =
            crate::playback_history::PlaybackHistory::open(&dir.path().join("history.sqlite"))
                .unwrap();
        let track = save_local(
            &store,
            &music.join("kept.mp3"),
            "mirror-off-kept",
            "Artist",
            "Kept",
        );
        store.set_favorite(&track, true).unwrap();
        history
            .record("mirror-off-session", "fixture", &track, None)
            .unwrap();
        let collection = store.create_collection("Preserved playlist").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[track])
            .unwrap();
        let enabled = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![source_dir.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        sync_playlists_with_history(&store, &enabled, &history).unwrap();
        let state_before = store.playlist_sync_state().unwrap();
        let mirror_paths = read_state(&store).unwrap().mirror_paths;
        let mirror_bytes = mirror_paths
            .iter()
            .map(|(key, path)| (key.clone(), fs::read(path).unwrap()))
            .collect::<HashMap<_, _>>();
        assert!(mirror_bytes.contains_key("favorites"));
        assert!(mirror_bytes.contains_key("history"));
        assert!(mirror_bytes.contains_key(&collection.id));

        let imported_source = source_dir.join("must-not-auto-import.m3u8");
        let source_bytes = b"#EXTM3U\n#PLAYLIST:New source\n#EXTINF:180,Artist - Kept\nhttps://example.invalid/new-source\n".to_vec();
        fs::write(&imported_source, &source_bytes).unwrap();
        let collection_count = store.list_collections().unwrap().len();
        let disabled = Settings {
            playlist_mirroring_enabled: false,
            ..enabled.clone()
        };
        let status = sync_playlists(&store, &disabled).unwrap();
        assert_eq!(status.scanned, 0);
        assert_eq!(status.synchronized, 0);
        assert!(status.warnings.is_empty());
        let with_history = sync_playlists_with_history(&store, &disabled, &history).unwrap();
        assert_eq!(with_history.scanned, 0);
        assert_eq!(with_history.mirrors_directory, None);
        assert_eq!(store.list_collections().unwrap().len(), collection_count);
        assert_eq!(store.playlist_sync_state().unwrap(), state_before);
        assert_eq!(fs::read(&imported_source).unwrap(), source_bytes);
        for (key, path) in &mirror_paths {
            assert_eq!(fs::read(path).unwrap(), mirror_bytes[key]);
        }
        store.save_settings(&disabled).unwrap();
        assert!(resume_playlist_mirror(&store, &disabled, &collection.id)
            .unwrap_err()
            .contains("turned off"));
        assert!(resume_history_mirror(&store, &history, &disabled)
            .unwrap_err()
            .contains("turned off"));
        assert!(migrate_playlist_mirror_v1(
            &store,
            &collection.id,
            read_state(&store).unwrap().mirror_hashes[&collection.id].as_str(),
        )
        .unwrap_err()
        .contains("turned off"));

        let export =
            PathBuf::from(export_playlist_path(&store, &disabled, &collection.id).unwrap());
        assert!(
            export.is_file(),
            "manual playlist export remains available when mirrors are off"
        );
        assert!(crate::portable_playlist::is_portable_playlist(
            &fs::read_to_string(export).unwrap()
        ));
        assert_eq!(store.playlist_sync_state().unwrap(), state_before);
        for (key, path) in &mirror_paths {
            assert_eq!(fs::read(path).unwrap(), mirror_bytes[key]);
        }
    }

    #[test]
    fn reenabled_mirroring_uses_saved_edit_and_deletion_protections() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let track = save_local(
            &store,
            &music.join("song.mp3"),
            "mirror-reenable",
            "Artist",
            "Song",
        );
        let edited_collection = store.create_collection("Editable set").unwrap();
        let deleted_collection = store.create_collection("Deleted set").unwrap();
        store
            .replace_playlist_entries(&edited_collection.id, &[track.clone()])
            .unwrap();
        store
            .replace_playlist_entries(&deleted_collection.id, &[track])
            .unwrap();
        let enabled = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &enabled).unwrap();
        let paths = read_state(&store).unwrap().mirror_paths;
        let edited_path = PathBuf::from(&paths[&edited_collection.id]);
        let deleted_path = PathBuf::from(&paths[&deleted_collection.id]);
        let baseline = fs::read_to_string(&edited_path).unwrap();
        let external_edit = baseline.replace(
            "#PLAYLIST:Editable set",
            "#PLAYLIST:Edited while mirroring was off",
        );
        assert_ne!(external_edit, baseline);
        let disabled = Settings {
            playlist_mirroring_enabled: false,
            ..enabled.clone()
        };
        fs::write(&edited_path, &external_edit).unwrap();
        fs::remove_file(&deleted_path).unwrap();
        let state_before_disabled_sync = store.playlist_sync_state().unwrap();
        assert_eq!(sync_playlists(&store, &disabled).unwrap().scanned, 0);
        assert_eq!(fs::read_to_string(&edited_path).unwrap(), external_edit);
        assert!(!deleted_path.exists());
        assert_eq!(
            store.playlist_sync_state().unwrap(),
            state_before_disabled_sync
        );

        let status = sync_playlists(&store, &enabled).unwrap();
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .into_iter()
                .find(|item| item.id == edited_collection.id)
                .unwrap()
                .name,
            "Edited while mirroring was off"
        );
        assert!(fs::read_to_string(&edited_path)
            .unwrap()
            .contains("#PLAYLIST:Edited while mirroring was off"));
        assert!(
            list_mirror_snapshots(&store, &edited_collection.id)
                .unwrap()
                .len()
                >= 2
        );
        assert!(
            !deleted_path.exists(),
            "reenabling must retain the existing deletion pause"
        );
        assert!(status.paused_mirrors.contains(&deleted_collection.id));
    }

    #[test]
    fn mirrors_follow_typed_folder_hierarchy_and_keep_relocation_archive_and_deletion_pause() {
        let temp = tempfile::tempdir().unwrap();
        let music = temp.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&temp.path().join("db.sqlite")).unwrap();
        let track = Track {
            id: "import:folder-track".into(),
            provider: "import".into(),
            provider_id: "folder-track".into(),
            artist: "Artist".into(),
            title: "Title".into(),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let collection = store.create_collection("Folder playlist").unwrap();
        store
            .replace_playlist_entries(&collection.id, std::slice::from_ref(&track))
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        let old = music
            .join("Playlists/Personal")
            .join(format!("{}.m3u8", collection.id));
        assert!(old.exists());

        let folder_id = uuid::Uuid::new_v4().to_string();
        let hierarchy = crate::playlist_hierarchy::PlaylistHierarchy {
            schema_version: 1,
            folders: vec![crate::playlist_hierarchy::PlaylistFolder {
                id: folder_id.clone(),
                name: "Reviewed Sets".into(),
                parent_id: None,
                position: 0,
            }],
            playlists: vec![crate::playlist_hierarchy::PlaylistPlacement {
                collection_id: collection.id.clone(),
                folder_id: Some(folder_id),
                position: 0,
                ..Default::default()
            }],
        };
        store
            .set_json_preference(
                "playlistHierarchy.v1",
                &serde_json::to_value(hierarchy).unwrap(),
            )
            .unwrap();
        sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        let moved = music
            .join("Playlists/Reviewed Sets")
            .join(format!("{}.m3u8", collection.id));
        assert!(moved.exists());
        assert!(crate::portable_playlist::is_portable_playlist(
            &fs::read_to_string(&moved).unwrap()
        ));
        assert!(!old.exists());
        assert!(music
            .join("Playlists/Archives/Personal")
            .join(&collection.id)
            .exists());

        fs::remove_file(&moved).unwrap();
        let status = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        assert!(
            !moved.exists(),
            "a missing moved mirror remains protected from automatic recreation"
        );
        assert!(status
            .paused_mirrors
            .iter()
            .any(|name| name.contains("Folder playlist") || name.contains(&collection.id)));
    }

    #[test]
    fn section_mirrors_live_below_their_playlist_and_preserve_external_edits() {
        let temp = tempfile::tempdir().unwrap();
        let music = temp.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&temp.path().join("db.sqlite")).unwrap();
        let track = Track {
            id: "import:section-track".into(),
            provider: "import".into(),
            provider_id: "section-track".into(),
            artist: "Artist".into(),
            title: "Section song".into(),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let collection = store.create_collection("Road set").unwrap();
        store
            .append_playlist_entries(&collection.id, &[track.clone(), track])
            .unwrap();
        let occurrences = store.list_playlist_occurrences(&collection.id).unwrap();
        let section_id = uuid::Uuid::new_v4().to_string();
        let hierarchy = crate::playlist_hierarchy::PlaylistHierarchy {
            schema_version: 1,
            folders: Vec::new(),
            playlists: vec![crate::playlist_hierarchy::PlaylistPlacement {
                collection_id: collection.id.clone(),
                sections: vec![crate::playlist_hierarchy::PlaylistSection {
                    id: section_id.clone(),
                    name: "Warm up".into(),
                    position: 0,
                }],
                occurrences: occurrences
                    .iter()
                    .map(|entry| crate::playlist_hierarchy::OccurrenceMetadata {
                        id: entry.occurrence_id.clone(),
                        section_id: Some(section_id.clone()),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
        };
        store
            .set_json_preference(
                "playlistHierarchy.v1",
                &serde_json::to_value(hierarchy).unwrap(),
            )
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        let path = music
            .join("Playlists/Personal")
            .join(&collection.id)
            .join("Sections")
            .join(format!("{section_id}.m3u8"));
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents.matches("#BURRATA-ENTRY:").count(), 2);
        assert!(contents.contains("Road set — Warm up"));

        let edited = format!("{contents}# user section note\n");
        fs::write(&path, &edited).unwrap();
        let status = sync_playlists(&store, &settings).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("Section file") && warning.contains("external edits")));
    }

    #[test]
    fn portable_mirror_section_edit_roundtrips_without_losing_occurrence_annotations() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("db.sqlite")).unwrap();
        let audio = temp.path().join("mirror-section-roundtrip.mp3");
        let track = save_local(
            &store,
            &audio,
            "mirror-section-roundtrip",
            "Artist",
            "Repeated track",
        );
        let collection = store.create_collection("Section edit").unwrap();
        store
            .append_playlist_entries(&collection.id, &[track.clone(), track.clone()])
            .unwrap();
        let occurrences = store.list_playlist_occurrences(&collection.id).unwrap();
        let old_section_id = uuid::Uuid::new_v4().to_string();
        let hierarchy = crate::playlist_hierarchy::PlaylistHierarchy {
            schema_version: 1,
            playlists: vec![crate::playlist_hierarchy::PlaylistPlacement {
                collection_id: collection.id.clone(),
                sections: vec![crate::playlist_hierarchy::PlaylistSection {
                    id: old_section_id.clone(),
                    name: "Warm up".into(),
                    position: 0,
                }],
                occurrences: occurrences
                    .iter()
                    .enumerate()
                    .map(
                        |(index, row)| crate::playlist_hierarchy::OccurrenceMetadata {
                            id: row.occurrence_id.clone(),
                            section_id: Some(old_section_id.clone()),
                            note: Some(format!("private note {index}")),
                            cue_points: vec![index as f64 + 0.5],
                            playback_range: Some(crate::portable_playlist::PlaybackRange {
                                start_seconds: index as f64 + 1.0,
                                end_seconds: index as f64 + 4.0,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store
            .set_json_preference(
                "playlistHierarchy.v1",
                &serde_json::to_value(hierarchy).unwrap(),
            )
            .unwrap();

        let tracks = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        let baseline = render_managed_playlist_m3u(
            &store,
            &collection.id,
            &collection.name,
            &tracks,
            temp.path(),
        )
        .unwrap();
        let mut edited = crate::portable_playlist::parse(&baseline).unwrap();
        // Simulate an editor changing section metadata while keeping the local file links.
        // Parsing retains raw resources separately from the caller-approved serializer field.
        for entry in &mut edited.entries {
            entry.resolved_resource = entry.resource.clone();
        }
        assert_eq!(edited.sections[0].id, "s1");
        assert!(edited
            .entries
            .iter()
            .all(|entry| entry.section_id.as_deref() == Some("s1")));
        edited.sections[0].name = "Renamed warm up".into();
        edited.sections.push(crate::portable_playlist::NamedItem {
            id: "s2".into(),
            name: "Break".into(),
            ..Default::default()
        });
        edited.entries[0].section_id = Some("s2".into());
        edited.entries[1].section_id = None;
        let mirror_text = crate::portable_playlist::serialize(
            &edited,
            &crate::portable_playlist::PlaylistExportOptions {
                purpose: crate::portable_playlist::PlaylistPurpose::LocalMirror,
                ..Default::default()
            },
        )
        .unwrap();
        let parsed_edit = crate::portable_playlist::parse(&mirror_text).unwrap();
        assert_eq!(parsed_edit.entries[0].section_id.as_deref(), Some("s2"));
        assert_eq!(parsed_edit.entries[1].section_id, None);
        let (resolved_tracks, unresolved, sections) =
            resolve_mirror_edit(&store, &mirror_text, temp.path()).unwrap();
        assert_eq!(unresolved, 0);
        assert_eq!(
            resolved_tracks
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec![track.id.as_str(), track.id.as_str()]
        );
        replace_mirror_entries(&store, &collection.id, &resolved_tracks, sections.as_ref())
            .unwrap();

        let after = crate::playlist_hierarchy::load(&store).unwrap();
        let placement = after
            .playlists
            .iter()
            .find(|item| item.collection_id == collection.id)
            .unwrap();
        assert_eq!(
            placement
                .sections
                .iter()
                .map(|section| section.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Renamed warm up", "Break"]
        );
        assert_eq!(placement.sections[0].id, old_section_id);
        assert_ne!(placement.sections[1].id, "s2");
        assert_eq!(
            placement
                .occurrences
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            occurrences
                .iter()
                .map(|entry| entry.occurrence_id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            placement.occurrences[0].section_id.as_deref(),
            Some(placement.sections[1].id.as_str())
        );
        assert_eq!(placement.occurrences[1].section_id, None);
        assert_eq!(
            placement.occurrences[0].note.as_deref(),
            Some("private note 0")
        );
        assert_eq!(
            placement.occurrences[1].note.as_deref(),
            Some("private note 1")
        );
        assert_eq!(placement.occurrences[0].cue_points, vec![0.5]);
        assert_eq!(placement.occurrences[1].cue_points, vec![1.5]);
        assert_eq!(
            placement.occurrences[0]
                .playback_range
                .as_ref()
                .map(|range| (range.start_seconds, range.end_seconds)),
            Some((1.0, 4.0))
        );
        assert_eq!(
            placement.occurrences[1]
                .playback_range
                .as_ref()
                .map(|range| (range.start_seconds, range.end_seconds)),
            Some((2.0, 5.0))
        );

        let before_invalid = serde_json::to_value(&after).unwrap();
        let mut foreign_ids = parsed_edit.clone();
        foreign_ids.sections[0].id = old_section_id.clone();
        assert!(validate_mirror_section_tokens(&foreign_ids).is_err());
        assert!(store
            .replace_playlist_entries_with_portable_sections(
                &collection.id,
                &tracks,
                &parsed_edit.sections,
                &[Some("foreign-uuid".into()), None],
            )
            .unwrap_err()
            .contains("unknown section"));
        let invalid_sections = vec![crate::portable_playlist::NamedItem {
            id: "s257".into(),
            name: "Out of bounds".into(),
            ..Default::default()
        }];
        assert!(store
            .replace_playlist_entries_with_portable_sections(
                &collection.id,
                &[],
                &invalid_sections,
                &[]
            )
            .unwrap_err()
            .contains("section metadata is invalid"));
        assert_eq!(
            serde_json::to_value(crate::playlist_hierarchy::load(&store).unwrap()).unwrap(),
            before_invalid
        );
    }

    #[test]
    fn oversized_history_is_partitioned_by_month_without_losing_occurrences() {
        let track = Track {
            id: "import:history-partition-song".into(),
            provider: "import".into(),
            provider_id: "history-partition-song".into(),
            artist: "A".into(),
            title: "B".into(),
            ..Track::default()
        };
        let portable = crate::portable_playlist::PortableTrack::from_track(&track);
        let entries = (0..10_001)
            .map(|index| {
                let month = ["2026-01", "2026-02", "2026-03", "2026-04"][index % 4];
                crate::portable_playlist::PortableEntry {
                    id: format!("h{}", index + 1),
                    history: Some(crate::portable_playlist::HistoryOccurrence {
                        occurrence_id: Some(format!("h{}", index + 1)),
                        played_at: Some(format!("{month}-15T12:00:00Z")),
                        ..Default::default()
                    }),
                    track: portable.clone(),
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        let input = crate::portable_playlist::PlaylistInput {
            id: "history".into(),
            name: "Listening History".into(),
            entries,
            ..Default::default()
        };
        let documents = history_mirror_documents(&input, None).unwrap();
        assert_eq!(documents.len(), 4);
        assert_eq!(
            documents
                .iter()
                .map(|document| crate::portable_playlist::parse(&document.body)
                    .unwrap()
                    .entries
                    .len())
                .sum::<usize>(),
            10_001
        );
        assert!(documents
            .iter()
            .all(|document| document.relative_path.starts_with("Listening History")));
    }

    #[test]
    fn recent_history_scope_is_validated_and_disclosed() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(history_entry_in_scope(
            "2026-10-20T12:00:00Z",
            Some(14),
            now
        ));
        assert!(!history_entry_in_scope(
            "2026-09-01T12:00:00Z",
            Some(14),
            now
        ));
        assert!(history_entry_in_scope("not-a-date", Some(14), now));
        assert!(history_entry_in_scope("2020-01-01T00:00:00Z", None, now));
        let input = crate::portable_playlist::PlaylistInput {
            name: "Listening History (Recent 14 days)".into(),
            description: Some("Configured recent-history scope: last 14 days.".into()),
            ..Default::default()
        };
        let documents = history_mirror_documents(&input, Some(14)).unwrap();
        assert!(documents[0]
            .relative_path
            .to_string_lossy()
            .contains("Recent 14 days"));
        let rendered = crate::portable_playlist::parse(&documents[0].body).unwrap();
        assert!(rendered
            .description
            .as_deref()
            .unwrap()
            .contains("last 14 days"));
        assert!(validate_history_mirror_options(&HistoryMirrorOptions {
            schema_version: 1,
            recent_days: Some(0)
        })
        .is_err());
    }

    #[test]
    fn history_mirror_scope_persists_with_all_history_as_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        assert_eq!(
            get_history_mirror_options(&store).unwrap().recent_days,
            None
        );
        let saved = set_history_mirror_options(
            &store,
            HistoryMirrorOptions {
                schema_version: 1,
                recent_days: Some(45),
            },
        )
        .unwrap();
        assert_eq!(saved.recent_days, Some(45));
        assert_eq!(get_history_mirror_options(&store).unwrap(), saved);
    }
    #[test]
    fn m3u_mirror_keeps_order_and_emits_local_then_known_url_then_search() {
        let tracks = vec![
            Track {
                id: "1".into(),
                artist: "A".into(),
                title: "One".into(),
                local_path: Some("/music/A/One.flac".into()),
                ..Track::default()
            },
            Track {
                id: "2".into(),
                artist: "B".into(),
                title: "Two".into(),
                source_url: "https://youtube.com/watch?v=two".into(),
                ..Track::default()
            },
            Track {
                id: "3".into(),
                artist: "C".into(),
                title: "Three".into(),
                ..Track::default()
            },
        ];
        let text = render_m3u(&tracks, "/music");
        assert!(
            text.find("A/One.flac").unwrap()
                < text.find("https://youtube.com/watch?v=two").unwrap()
        );
        assert!(text.contains("ytdl://ytsearch1:C Three mix"));
        let managed_text = render_named_m3u("Managed", &tracks, "/music");
        assert!(managed_text.starts_with("#EXTM3U\n#BURRATA-MIRROR\n#PLAYLIST:Managed\n"));
        assert_eq!(
            count_entries("#EXTM3U\n#EXTINF:1,A - B\nhttps://x\n../c.mp3\n"),
            2
        );
    }

    #[test]
    fn batched_snapshot_hydration_preserves_duplicate_occurrences_and_none_overwrites() {
        let snapshot = Track {
            id: "spotify:duplicate".into(),
            provider: "spotify".into(),
            duration_seconds: Some(245.48),
            local_path: Some("snapshot-path.flac".into()),
            local_availability: "available".into(),
            asset_id: Some("snapshot-asset".into()),
            provenance: serde_json::json!({"sourceKind":"playlist","playlistPosition":1,"occurrencePositions":[1,3],"captureMarker":"retained"}),
            ..Track::default()
        };
        let saved = Track {
            id: snapshot.id.clone(),
            local_path: None,
            local_availability: "none".into(),
            asset_id: None,
            provenance: serde_json::json!({"audioCandidate":{"provider":"youtube","providerId":"abc"},"candidateUrl":"https://example.invalid/stable"}),
            ..Track::default()
        };
        let merged = merge_snapshot_track_state(vec![snapshot], vec![saved]);
        let occurrences = crate::browser_bridge::expand_playlist_occurrences(merged);
        assert_eq!(occurrences.len(), 2);
        assert_eq!(
            occurrences
                .iter()
                .map(|track| track.duration_seconds)
                .collect::<Vec<_>>(),
            vec![Some(245.48); 2]
        );
        assert_eq!(
            occurrences
                .iter()
                .map(|track| track.provenance["position"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        for track in occurrences {
            assert_eq!(track.local_path, None);
            assert_eq!(track.local_availability, "none");
            assert_eq!(track.asset_id, None);
            assert_eq!(track.provenance["captureMarker"], "retained");
            assert_eq!(
                track.provenance["candidateUrl"],
                "https://example.invalid/stable"
            );
            assert_eq!(track.provenance["audioCandidate"]["provider"], "youtube");
        }
    }

    fn save_local(store: &Store, path: &Path, id: &str, artist: &str, title: &str) -> Track {
        fs::write(path, b"audio").unwrap();
        let track = Track {
            id: format!("local:{id}"),
            provider: "local".into(),
            provider_id: id.into(),
            source_url: format!("local:{id}"),
            artist: artist.into(),
            title: title.into(),
            local_path: Some(path.to_string_lossy().into_owned()),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        store
            .save_asset(&LocalAsset {
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_seconds: None,
                size_bytes: None,
                bitrate_origin: None,
                id: format!("asset:{id}"),
                recording_id: track.id.clone(),
                path: path.to_string_lossy().into_owned(),
                hash: id.into(),
                source_codec: "mp3".into(),
                final_codec: "mp3".into(),
                source_bitrate: None,
                final_bitrate: None,
                processing: "external-readonly".into(),
                track: track.clone(),
            })
            .unwrap();
        track
    }

    #[test]
    fn simultaneous_source_and_mirror_changes_preserve_source_and_personal_conflict_order() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let playlists = dir.path().join("source-playlists");
        fs::create_dir_all(&music).unwrap();
        fs::create_dir_all(&playlists).unwrap();
        let audio = music.join("song.mp3");
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let track = save_local(&store, &audio, "song", "Artist", "Song");
        let source = playlists.join("Set.m3u8");
        fs::write(
            &source,
            format!("#EXTM3U\n#EXTINF:180,Artist - Song\n{}\n", audio.display()),
        )
        .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![playlists.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let imported =
            import_discovered_playlist(&store, &settings, &source.to_string_lossy()).unwrap();
        sync_playlists(&store, &settings).unwrap();
        let mirror = PathBuf::from(
            read_state(&store)
                .unwrap()
                .mirror_paths
                .get(&imported.collection.id)
                .expect("sync records its generated mirror path"),
        );
        fs::write(
            &source,
            format!(
                "#EXTM3U\n# changed source\n#EXTINF:180,Artist - Song\n{}\n",
                audio.display()
            ),
        )
        .unwrap();
        fs::write(
            &mirror,
            format!(
                "#EXTM3U\n#EXTINF:180,Artist - Song\n{}\n#EXTINF:180,Artist - Song\n{}\n",
                audio.display(),
                audio.display()
            ),
        )
        .unwrap();
        let status = sync_playlists(&store, &settings).unwrap();
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("simultaneous source and mirror edits")));
        assert!(source.exists());
        assert_eq!(
            store
                .list_tracks("collection", Some(&imported.collection.id))
                .unwrap()
                .len(),
            1
        );
        let collections = store.list_collections().unwrap();
        let personal = collections
            .into_iter()
            .find(|c| c.name.ends_with("Personal copy)"))
            .unwrap_or_else(|| {
                panic!(
                    "personal copy missing; names={:?}",
                    store
                        .list_collections()
                        .unwrap()
                        .iter()
                        .map(|c| &c.name)
                        .collect::<Vec<_>>()
                )
            });
        let tracks = store.list_tracks("collection", Some(&personal.id)).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].id, track.id);
        assert_eq!(tracks[1].id, track.id);
        assert!(
            list_mirror_snapshots(&store, &imported.collection.id)
                .unwrap()
                .len()
                >= 2
        );
    }

    #[test]
    fn mirror_addition_with_extinf_keeps_unregistered_streaming_entry() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let audio = music.join("known.mp3");
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let mut known = save_local(&store, &audio, "known", "Known", "Track");
        known.duration_seconds = Some(200.0);
        known.rank = Some(7);
        known.provenance = serde_json::json!({"testMarker":"catalog-hydrated"});
        let known = store.save_track(&known).unwrap();
        let collection = store.create_collection("Personal set").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[known.clone()])
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        let mirror = music
            .join("Playlists/Local")
            .join(format!("{}.m3u8", collection.id));
        fs::write(&mirror, format!("#EXTM3U\n#PLAYLIST:Renamed in editor\n#EXTINF:200,Known - Track\n{}\n#EXTINF:200,Known - Track\n{}\n#EXTINF:190,New Artist - New Song (Club Mix)\nytdl://ytsearch1:New Artist New Song Club Mix mix\n#EXTINF:180,Stream Artist - Stream Song\nhttps://www.youtube.com/watch?v=ABCDEF12345\n", audio.display(), audio.display())).unwrap();
        sync_playlists(&store, &settings).unwrap();
        let tracks = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(tracks.len(), 4);
        assert_eq!(
            tracks[0].id, tracks[1].id,
            "duplicate playlist entry must remain duplicated"
        );
        for known_occurrence in &tracks[..2] {
            assert_eq!(known_occurrence.duration_seconds, Some(200.0));
            assert_eq!(known_occurrence.rank, Some(7));
            assert_eq!(
                known_occurrence.provenance["testMarker"],
                "catalog-hydrated"
            );
        }
        assert_eq!(tracks[2].artist, "New Artist");
        assert_eq!(tracks[2].provider, "import", "track={:?}", tracks[2]);
        assert_eq!(
            tracks[3].source_url,
            "https://www.youtube.com/watch?v=ABCDEF12345"
        );
        assert_eq!(tracks[3].provenance["sourceKind"], "audioSearch");
        assert_eq!(
            tracks[3].provenance["audioCandidate"]["provider"],
            "youtube"
        );
        assert_eq!(
            tracks[3].provenance["candidateUrl"],
            "https://www.youtube.com/watch?v=ABCDEF12345"
        );
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .iter()
                .find(|c| c.id == collection.id)
                .unwrap()
                .name,
            "Renamed in editor"
        );
        let saved_mirror = music
            .join("Playlists/Personal")
            .join(format!("{}.m3u8", collection.id));
        assert!(
            saved_mirror.exists(),
            "expected mirror, found {:?}",
            fs::read_dir(music.join("Playlists"))
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
                .collect::<Vec<_>>()
        );
        let expected_body =
            render_named_m3u("Renamed in editor", &tracks, &settings.music_directory);
        assert_eq!(fs::read_to_string(&saved_mirror).unwrap(), expected_body);
        sync_playlists(&store, &settings).unwrap();
        let reloaded = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(reloaded.len(), 4);
        assert_eq!(reloaded[0].id, reloaded[1].id);
        assert_eq!(
            fs::read_to_string(&saved_mirror).unwrap(),
            render_named_m3u("Renamed in editor", &reloaded, &settings.music_directory),
            "next sync must not revert the persisted mirror edit"
        );
        store
            .rename_collection(&collection.id, "Renamed in app")
            .unwrap();
        sync_playlists(&store, &settings).unwrap();
        let saved_mirror = music
            .join("Playlists/Personal")
            .join(format!("{}.m3u8", collection.id));
        assert!(fs::read_to_string(saved_mirror)
            .unwrap()
            .contains("#PLAYLIST:Renamed in app"));
    }

    fn malformed_portable_edit(path: &Path) -> Vec<u8> {
        let mut bytes = fs::read(path).unwrap();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(
            b"#EXTINF:180,Unreviewed - Entry\nburrata://track/unreviewed-entry\n",
        );
        fs::write(path, &bytes).unwrap();
        bytes
    }

    #[test]
    fn duplicate_portable_section_ids_are_rejected_before_conflict_copy_creation() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let first = save_local(
            &store,
            &music.join("first.mp3"),
            "duplicate-section-first",
            "First",
            "Song",
        );
        let second = save_local(
            &store,
            &music.join("second.mp3"),
            "duplicate-section-second",
            "Second",
            "Song",
        );
        let collection = store.create_collection("Duplicate section mirror").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[first.clone()])
            .unwrap();
        let occurrences = store.list_playlist_occurrences(&collection.id).unwrap();
        let section_id = uuid::Uuid::new_v4().to_string();
        let hierarchy = crate::playlist_hierarchy::PlaylistHierarchy {
            schema_version: 1,
            playlists: vec![crate::playlist_hierarchy::PlaylistPlacement {
                collection_id: collection.id.clone(),
                sections: vec![crate::playlist_hierarchy::PlaylistSection {
                    id: section_id.clone(),
                    name: "Warm up".into(),
                    position: 0,
                }],
                occurrences: occurrences
                    .iter()
                    .map(|row| crate::playlist_hierarchy::OccurrenceMetadata {
                        id: row.occurrence_id.clone(),
                        section_id: Some(section_id.clone()),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store
            .set_json_preference(
                "playlistHierarchy.v1",
                &serde_json::to_value(hierarchy).unwrap(),
            )
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        let mirror = music
            .join("Playlists/Local")
            .join(format!("{}.m3u8", collection.id));
        let baseline = fs::read_to_string(&mirror).unwrap();
        let edited = baseline.replace(
            "\"sections\":[{\"id\":\"s1\",\"name\":\"Warm up\"}]",
            "\"sections\":[{\"id\":\"s1\",\"name\":\"Warm up\"},{\"id\":\"s1\",\"name\":\"Duplicate\"}]",
        );
        assert_ne!(
            edited, baseline,
            "fixture must add a duplicate canonical section token"
        );
        fs::write(&mirror, &edited).unwrap();
        store
            .append_playlist_entries(&collection.id, &[second])
            .unwrap();
        let collection_count = store.list_collections().unwrap().len();

        let status = sync_playlists(&store, &settings).unwrap();
        assert!(status.paused_mirrors.contains(&collection.id));
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("duplicated or out of bounds")));
        assert_eq!(fs::read_to_string(&mirror).unwrap(), edited);
        assert_eq!(store.list_collections().unwrap().len(), collection_count);
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn malformed_portable_collection_edit_is_paused_preserved_and_does_not_block_other_mirrors() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let first = save_local(
            &store,
            &music.join("first.mp3"),
            "malformed-first",
            "First",
            "Song",
        );
        let second = save_local(
            &store,
            &music.join("second.mp3"),
            "malformed-second",
            "Second",
            "Song",
        );
        let malformed = store.create_collection("Malformed mirror").unwrap();
        let unrelated = store.create_collection("Unrelated mirror").unwrap();
        store
            .replace_playlist_entries(&malformed.id, &[first.clone()])
            .unwrap();
        store
            .replace_playlist_entries(&unrelated.id, &[first.clone()])
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        let mirror_dir = music.join("Playlists").join("Local");
        let malformed_path = mirror_dir.join(format!("{}.m3u8", malformed.id));
        let unrelated_path = mirror_dir.join(format!("{}.m3u8", unrelated.id));
        let malformed_bytes = malformed_portable_edit(&malformed_path);
        let baseline_hash = read_state(&store).unwrap().mirror_hashes[&malformed.id].clone();
        fs::write(
            &unrelated_path,
            format!(
                "#EXTM3U\n#PLAYLIST:Unrelated mirror\n#EXTINF:180,Second - Song\n{}\n",
                second.local_path.as_deref().unwrap()
            ),
        )
        .unwrap();

        let status = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        assert!(status.paused_mirrors.contains(&malformed.id));
        assert!(status.warnings.iter().any(|warning| warning
            .contains(&malformed_path.to_string_lossy().to_string())
            && warning.contains("kept for review")));
        assert_eq!(fs::read(&malformed_path).unwrap(), malformed_bytes);
        assert_eq!(
            read_state(&store).unwrap().mirror_hashes[&malformed.id],
            baseline_hash
        );
        let malformed_tracks = store
            .list_tracks("collection", Some(&malformed.id))
            .unwrap();
        assert_eq!(malformed_tracks.len(), 1);
        assert_eq!(malformed_tracks[0].id, first.id);
        let unrelated_tracks = store
            .list_tracks("collection", Some(&unrelated.id))
            .unwrap();
        assert_eq!(unrelated_tracks.len(), 1);
        assert_eq!(unrelated_tracks[0].id, second.id);

        let repeat = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        assert!(repeat.paused_mirrors.contains(&malformed.id));
        assert_eq!(fs::read(&malformed_path).unwrap(), malformed_bytes);
        assert_eq!(
            store
                .list_tracks("collection", Some(&malformed.id))
                .unwrap()[0]
                .id,
            first.id
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&unrelated.id))
                .unwrap()[0]
                .id,
            second.id
        );
        let resume_error = resume_playlist_mirror(&store, &settings, &malformed.id).unwrap_err();
        assert!(resume_error.contains("contains external edits"));
        assert_eq!(fs::read(&malformed_path).unwrap(), malformed_bytes);
        assert_eq!(
            read_state(&store).unwrap().mirror_hashes[&malformed.id],
            baseline_hash
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&malformed.id))
                .unwrap()[0]
                .id,
            first.id
        );
    }

    #[test]
    fn sectioned_favorites_mirror_is_preserved_before_favorites_change() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let first = save_local(
            &store,
            &music.join("favorite.mp3"),
            "sectioned-favorite",
            "Favorite",
            "Song",
        );
        let second = save_local(
            &store,
            &music.join("other.mp3"),
            "sectioned-other",
            "Other",
            "Song",
        );
        store.set_favorite(&first, true).unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        let favorites_path = music.join("Playlists/Liked Songs.m3u8");
        let baseline_hash = read_state(&store).unwrap().mirror_hashes["favorites"].clone();
        let mut edited =
            crate::portable_playlist::parse(&fs::read_to_string(&favorites_path).unwrap()).unwrap();
        edited.sections.push(crate::portable_playlist::NamedItem {
            id: "s1".into(),
            name: "Verse".into(),
            ..Default::default()
        });
        edited.entries[0].section_id = Some("s1".into());
        let sectioned = crate::portable_playlist::serialize(
            &edited,
            &crate::portable_playlist::PlaylistExportOptions {
                purpose: crate::portable_playlist::PlaylistPurpose::LocalMirror,
                ..Default::default()
            },
        )
        .unwrap();
        fs::write(&favorites_path, &sectioned).unwrap();

        let status = sync_playlists(&store, &settings).unwrap();
        assert!(status.paused_mirrors.iter().any(|id| id == "favorites"));
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("import it as a playlist")
                && warning.contains("favorites were kept")));
        assert_eq!(fs::read_to_string(&favorites_path).unwrap(), sectioned);
        assert_eq!(
            read_state(&store).unwrap().mirror_hashes["favorites"],
            baseline_hash
        );
        assert!(store.get_track(&first.id).unwrap().unwrap().favorite);
        assert!(!store.get_track(&second.id).unwrap().unwrap().favorite);
    }

    #[test]
    fn malformed_portable_favorites_edit_is_paused_preserved_and_does_not_block_other_mirrors() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let first = save_local(
            &store,
            &music.join("favorite.mp3"),
            "malformed-favorite",
            "Favorite",
            "Song",
        );
        let second = save_local(
            &store,
            &music.join("other.mp3"),
            "malformed-other",
            "Other",
            "Song",
        );
        store.set_favorite(&first, true).unwrap();
        let unrelated = store.create_collection("Unrelated mirror").unwrap();
        store
            .replace_playlist_entries(&unrelated.id, &[first.clone()])
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        let favorites_path = music.join("Playlists/Liked Songs.m3u8");
        let unrelated_path = music
            .join("Playlists/Local")
            .join(format!("{}.m3u8", unrelated.id));
        let favorites_bytes = malformed_portable_edit(&favorites_path);
        let baseline_hash = read_state(&store).unwrap().mirror_hashes["favorites"].clone();
        fs::write(
            &unrelated_path,
            format!(
                "#EXTM3U\n#PLAYLIST:Unrelated mirror\n#EXTINF:180,Other - Song\n{}\n",
                second.local_path.as_deref().unwrap()
            ),
        )
        .unwrap();

        let status = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        assert!(status.paused_mirrors.iter().any(|id| id == "favorites"));
        assert!(status.warnings.iter().any(|warning| warning
            .contains("Liked Songs edit could not be applied")
            && warning.contains("kept for review")));
        assert_eq!(fs::read(&favorites_path).unwrap(), favorites_bytes);
        assert_eq!(
            read_state(&store).unwrap().mirror_hashes["favorites"],
            baseline_hash
        );
        assert!(store.get_track(&first.id).unwrap().unwrap().favorite);
        assert!(!store.get_track(&second.id).unwrap().unwrap().favorite);
        assert_eq!(
            store
                .list_tracks("collection", Some(&unrelated.id))
                .unwrap()[0]
                .id,
            second.id
        );
        let resume_error = resume_playlist_mirror(&store, &settings, "favorites").unwrap_err();
        assert!(resume_error.contains("contains external edits"));
        assert_eq!(fs::read(&favorites_path).unwrap(), favorites_bytes);
        assert_eq!(
            read_state(&store).unwrap().mirror_hashes["favorites"],
            baseline_hash
        );
        assert!(store.get_track(&first.id).unwrap().unwrap().favorite);
        assert!(!store.get_track(&second.id).unwrap().unwrap().favorite);

        let repeat = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();
        assert!(repeat.paused_mirrors.iter().any(|id| id == "favorites"));
        assert_eq!(fs::read(&favorites_path).unwrap(), favorites_bytes);
        assert!(store.get_track(&first.id).unwrap().unwrap().favorite);
        assert!(!store.get_track(&second.id).unwrap().unwrap().favorite);
        assert_eq!(
            store
                .list_tracks("collection", Some(&unrelated.id))
                .unwrap()[0]
                .id,
            second.id
        );
    }

    #[test]
    fn simultaneous_app_and_mirror_edits_fork_conflict_and_keep_app_membership() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let app_track = save_local(&store, &music.join("app.mp3"), "app", "App", "Track");
        let mirror_track = save_local(
            &store,
            &music.join("mirror.mp3"),
            "mirror",
            "Mirror",
            "Track",
        );
        let collection = store.create_collection("Personal set").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[app_track.clone()])
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        // App changes after the baseline, then the mirror independently changes from that baseline.
        store
            .replace_playlist_entries(&collection.id, &[app_track.clone(), app_track.clone()])
            .unwrap();
        let mirror = music
            .join("Playlists/Local")
            .join(format!("{}.m3u8", collection.id));
        fs::write(
            &mirror,
            format!(
                "#EXTM3U\n#EXTINF:180,Mirror - Track\n{}\n",
                mirror_track.local_path.as_deref().unwrap()
            ),
        )
        .unwrap();
        sync_playlists(&store, &settings).unwrap();
        let original = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(original.len(), 2);
        assert!(original.iter().all(|track| track.id == app_track.id));
        let conflict = store
            .list_collections()
            .unwrap()
            .into_iter()
            .find(|c| c.name.contains("(conflict "))
            .expect("conflict copy");
        let conflict_tracks = store.list_tracks("collection", Some(&conflict.id)).unwrap();
        assert_eq!(conflict_tracks.len(), 1);
        assert_eq!(conflict_tracks[0].id, mirror_track.id);
    }

    #[test]
    fn deleted_mirror_pauses_until_explicit_resume() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let track = save_local(&store, &music.join("track.mp3"), "track", "Artist", "Track");
        let collection = store.create_collection("Personal set").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[track])
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        let mirror = music
            .join("Playlists/Local")
            .join(format!("{}.m3u8", collection.id));
        fs::remove_file(&mirror).unwrap();
        let status = sync_playlists(&store, &settings).unwrap();
        assert!(status.paused_mirrors.contains(&collection.id));
        assert!(
            !mirror.exists(),
            "routine sync must not recreate a deleted mirror"
        );
        resume_playlist_mirror(&store, &settings, &collection.id).unwrap();
        assert!(mirror.exists());
        assert!(!sync_playlists(&store, &settings)
            .unwrap()
            .paused_mirrors
            .contains(&collection.id));
    }

    #[test]
    fn repeated_browser_playlist_positions_export_and_copy_as_duplicate_entries() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let entries = (1..=3)
            .map(|position| serde_json::json!({
                "providerId":"1234567890123456789012","artist":"Artist","title":"Track","position":position
            }))
            .collect::<Vec<_>>();
        let snapshot: crate::browser_bridge::BrowserSnapshot =
            serde_json::from_value(serde_json::json!({
                "schemaVersion":1,"provider":"spotify","kind":"playlist","title":"Repeated",
                "sourceUrl":"https://open.spotify.com/playlist/1234567890123456789012",
                "capturedAt":"2026-10-12T12:00:00Z","complete":true,"expectedTotal":3,"warnings":[],
                "entries":entries
            }))
            .unwrap();
        let chart = crate::browser_bridge::validate_snapshot(&snapshot).unwrap();
        let imported = store.import_browser_chart(&chart, &snapshot).unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        assert_eq!(
            store
                .load_browser_import(&imported.id)
                .unwrap()
                .tracks
                .len(),
            3
        );
        let mirror = music
            .join("Playlists/Spotify")
            .join(format!("{}.m3u8", imported.collection_id));
        sync_playlists(&store, &settings).unwrap();
        let contents = fs::read_to_string(&mirror).unwrap();
        assert_eq!(
            contents
                .lines()
                .filter(|line| line.starts_with("#EXTINF:"))
                .count(),
            3
        );
        let personal_id = ensure_editable_collection(&store, &imported.collection_id).unwrap();
        assert_eq!(
            store
                .list_tracks("collection", Some(&personal_id))
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn mirror_uses_remembered_audio_candidate_url_without_changing_track_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let track = Track {
            id: "spotify:catalog-id".into(),
            provider: "spotify".into(),
            provider_id: "catalog-id".into(),
            source_url: "https://open.spotify.com/track/catalog-id".into(),
            artist: "Catalog Artist".into(),
            title: "Catalog Title".into(),
            isrc: Some("CATALOG12345".into()),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let candidate = Candidate {
            id: "youtube:Known12345".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=Known12345".into(),
            title: "Unverified source title".into(),
            permission: "unknown".into(),
            confidence: "low".into(),
            ..Candidate::default()
        };
        let saved = store.save_audio_match(&track, &candidate).unwrap();
        assert_eq!(saved.artist, "Catalog Artist");
        assert_eq!(saved.title, "Catalog Title");
        assert_eq!(saved.isrc.as_deref(), Some("CATALOG12345"));
        let collection = store.create_collection("Candidate URL").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[saved])
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        sync_playlists(&store, &settings).unwrap();
        let mirror = music
            .join("Playlists/Spotify")
            .join(format!("{}.m3u8", collection.id));
        let contents = fs::read_to_string(mirror).unwrap();
        assert!(contents.contains("https://www.youtube.com/watch?v=Known12345"));
        assert!(contents.contains("#EXTINF:-1,Catalog Artist - Catalog Title"));
    }

    #[test]
    fn startup_sync_imports_discovered_playlist_once_with_order_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let sources = dir.path().join("source-playlists");
        fs::create_dir_all(&music).unwrap();
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("favorites.m3u");
        let repeated = "https://www.youtube.com/watch?v=Repeat123";
        fs::write(
            &source,
            format!("#EXTM3U\n#EXTINF:180,Artist - Song\n{repeated}\n#EXTINF:180,Artist - Song\n{repeated}\n"),
        )
        .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![sources.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let source_path = source.to_string_lossy().into_owned();
        let source_root = sources.to_string_lossy().into_owned();
        let scan = || playlist_files::LocalPlaylistScan {
            files: vec![playlist_files::LocalPlaylistFile {
                path: source_path.clone(),
                name: "favorites".into(),
                modified: None,
                track_count: 2,
                warning: None,
            }],
            roots: vec![playlist_files::PlaylistRootStatus {
                path: source_root.clone(),
                available: true,
                readable: true,
            }],
            warnings: Vec::new(),
        };

        let first = sync_playlists_with_scan(&store, &settings, scan()).unwrap();
        assert_eq!(first.scanned, 1);
        assert!(!first
            .warnings
            .iter()
            .any(|warning| warning.contains("has not been imported")));
        let collection_id = first.sources[0]
            .collection_id
            .clone()
            .expect("auto-imported collection");
        let entries = store
            .list_tracks("collection", Some(&collection_id))
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, entries[1].id);
        assert_eq!(
            entries
                .iter()
                .map(|track| track.provenance["playlistPosition"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let second = sync_playlists_with_scan(&store, &settings, scan()).unwrap();
        assert_eq!(
            second.sources[0].collection_id.as_deref(),
            Some(collection_id.as_str())
        );
        assert!(!second
            .warnings
            .iter()
            .any(|warning| warning.contains("has not been imported")));
        assert_eq!(
            store.list_collections().unwrap().len(),
            1,
            "repeated startup scans must not make a second collection"
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection_id))
                .unwrap()
                .len(),
            2
        );
        let original_contents = fs::read_to_string(&source).unwrap();
        store.delete_collection(&collection_id).unwrap();
        let deleted_status = sync_playlists_with_scan(&store, &settings, scan()).unwrap();
        assert_eq!(deleted_status.sources[0].state, "deleted");
        assert_eq!(
            deleted_status.sources[0].collection_id.as_deref(),
            Some(collection_id.as_str())
        );
        assert!(store.list_collections().unwrap().is_empty());
        assert!(store
            .list_tracks("collection", Some(&collection_id))
            .is_err());
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            original_contents,
            "deleting a collection must not alter its external source playlist"
        );

        let restored = store.restore_collection(&collection_id).unwrap();
        assert_eq!(restored.id, collection_id);
        let restored_status = sync_playlists_with_scan(&store, &settings, scan()).unwrap();
        assert_eq!(
            restored_status.sources[0].collection_id.as_deref(),
            Some(collection_id.as_str())
        );
        assert_eq!(store.list_collections().unwrap().len(), 1);
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection_id))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn same_revision_sync_repairs_legacy_url_placeholder_from_source_comment() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let sources = dir.path().join("source-playlists");
        fs::create_dir_all(&music).unwrap();
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("Omará.m3u");
        let url = "https://www.youtube.com/watch?v=ABCDEF12345";
        let contents = format!("#EXTM3U\n# Bebo Valdés - Lágrimas Negras\n{url}\n");
        fs::write(&source, &contents).unwrap();
        let canonical = fs::canonicalize(&source).unwrap();
        let key = canonical.to_string_lossy().into_owned();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![sources.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let hash = format!("{:x}", Sha256::digest(contents.as_bytes()));
        let placeholder = playlist_files::unverified_url_track(url).unwrap();
        store.save_track(&placeholder).unwrap();
        let collection = store.create_collection("Omará").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[placeholder.clone()])
            .unwrap();
        let mut state = SyncState::default();
        state.sources.insert(
            key.clone(),
            SourceState {
                collection_id: Some(collection.id.clone()),
                original_name: Some("Omará".into()),
                last_source_hash: Some(hash.clone()),
                snapshots: vec![SourceSnapshot {
                    revision: hash,
                    captured_at: "2026-10-14T00:00:00Z".into(),
                    contents: contents.clone(),
                }],
                ..SourceState::default()
            },
        );
        persist_state(&store, &state).unwrap();
        let scan = playlist_files::LocalPlaylistScan {
            files: vec![playlist_files::LocalPlaylistFile {
                path: key.clone(),
                name: "Omará".into(),
                modified: None,
                track_count: 1,
                warning: None,
            }],
            roots: Vec::new(),
            warnings: Vec::new(),
        };

        let status = sync_playlists_with_scan(&store, &settings, scan).unwrap();
        let entries = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();

        assert_eq!(status.sources[0].state, "bound");
        assert_eq!(status.repaired_track_metadata, 1);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, placeholder.id);
        assert_eq!(entries[0].source_url, url);
        assert_eq!(entries[0].artist, "Bebo Valdés");
        assert_eq!(entries[0].title, "Lágrimas Negras");
        assert_eq!(
            entries[0].provenance["playlistComment"]["catalogVerified"],
            false
        );
    }

    #[test]
    fn excluded_source_bound_collection_skips_automatic_placeholder_repair() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let sources = dir.path().join("source-playlists");
        fs::create_dir_all(&music).unwrap();
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("Excluded source.m3u");
        let url = "https://www.youtube.com/watch?v=EXCLUDED12345";
        let contents = format!("#EXTM3U\n# Artist - Track\n{url}\n");
        fs::write(&source, &contents).unwrap();
        let canonical = fs::canonicalize(&source).unwrap();
        let key = canonical.to_string_lossy().into_owned();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![sources.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let placeholder = playlist_files::unverified_url_track(url).unwrap();
        store.save_track(&placeholder).unwrap();
        let collection = store.create_collection("Excluded source").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[placeholder.clone()])
            .unwrap();
        let hash = digest(contents.as_bytes());
        let mut state = SyncState::default();
        state.sources.insert(
            key.clone(),
            SourceState {
                collection_id: Some(collection.id.clone()),
                original_name: Some("Excluded source".into()),
                last_source_hash: Some(hash.clone()),
                snapshots: vec![SourceSnapshot {
                    revision: hash,
                    captured_at: "2026-10-27T00:00:00Z".into(),
                    contents: contents.clone(),
                }],
                ..SourceState::default()
            },
        );
        persist_state(&store, &state).unwrap();
        crate::playlist_mirror_options::set_playlist_mirror_options(
            &store,
            &collection.id,
            crate::playlist_mirror_options::PlaylistMirrorOptions {
                included: false,
                ..Default::default()
            },
        )
        .unwrap();
        let scan = || playlist_files::LocalPlaylistScan {
            files: vec![playlist_files::LocalPlaylistFile {
                path: key.clone(),
                name: "Excluded source".into(),
                modified: None,
                track_count: 1,
                warning: None,
            }],
            roots: Vec::new(),
            warnings: Vec::new(),
        };

        let status = sync_playlists_with_scan(&store, &settings, scan()).unwrap();
        let entries = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(status.sources[0].state, "bound");
        assert_eq!(status.repaired_track_metadata, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, placeholder.id);
        assert_eq!(entries[0].artist, "Unknown artist");
        assert_eq!(entries[0].title, url);

        let changed_url = "https://www.youtube.com/watch?v=CHANGED12345";
        fs::write(
            &source,
            format!("#EXTM3U\n# New artist - New track\n{changed_url}\n"),
        )
        .unwrap();
        let changed_status = sync_playlists_with_scan(&store, &settings, scan()).unwrap();
        let entries = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(changed_status.sources[0].state, "needsReview");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, placeholder.id);
        assert_eq!(entries[0].source_url, url);
    }

    #[test]
    fn startup_sync_ignores_legacy_unmarked_persisted_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let mirror_dir = music.join("Playlists/Personal");
        fs::create_dir_all(&mirror_dir).unwrap();
        let mirror = mirror_dir.join("legacy-collection.m3u8");
        fs::write(
            &mirror,
            "#EXTM3U\n#PLAYLIST:Old generated mirror\nhttps://example.com/track\n",
        )
        .unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        store
            .save_playlist_sync_state(&serde_json::json!({
                "sources": {},
                "mirrorPaths": {"legacy-collection": mirror.to_string_lossy()},
                "mirrorHashes": {"legacy-collection": "previous-hash"}
            }))
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![music.join("Playlists").to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let scan = playlist_files::LocalPlaylistScan {
            files: vec![playlist_files::LocalPlaylistFile {
                path: mirror.to_string_lossy().into_owned(),
                name: "legacy-collection".into(),
                modified: None,
                track_count: 1,
                warning: None,
            }],
            roots: Vec::new(),
            warnings: Vec::new(),
        };

        let status = sync_playlists_with_scan(&store, &settings, scan).unwrap();

        assert_eq!(status.scanned, 0);
        assert!(status.sources.is_empty());
        assert!(store.list_collections().unwrap().is_empty());
    }

    #[test]
    fn empty_user_playlist_does_not_create_sync_warning() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let playlist = dir.path().join("Empty.m3u");
        fs::create_dir_all(&music).unwrap();
        fs::write(&playlist, "#EXTM3U\n").unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let scan = playlist_files::LocalPlaylistScan {
            files: vec![playlist_files::LocalPlaylistFile {
                path: playlist.to_string_lossy().into_owned(),
                name: "Empty".into(),
                modified: None,
                track_count: 0,
                warning: None,
            }],
            roots: Vec::new(),
            warnings: Vec::new(),
        };

        let status = sync_playlists_with_scan(&store, &settings, scan).unwrap();

        assert!(status.warnings.is_empty());
        assert_eq!(status.sources.len(), 1);
        assert_eq!(status.sources[0].state, "empty");
        assert!(status.sources[0].warning.is_none());
        assert!(store.list_collections().unwrap().is_empty());
    }

    fn generated_archive_fixture(
        dir: &Path,
    ) -> (
        Store,
        Settings,
        PathBuf,
        PathBuf,
        playlist_files::LocalPlaylistScan,
    ) {
        let music = dir.join("Music");
        fs::create_dir_all(&music).unwrap();
        let audio = music.join("song.mp3");
        fs::write(&audio, b"audio fixture").unwrap();
        let collection_dir = music
            .join("Playlists/Archives/Personal")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&collection_dir).unwrap();
        let path = collection_dir.join(format!("20261013010101-{}.m3u8", "b".repeat(64)));
        fs::write(
            &path,
            format!("#EXTM3U\n#EXTINF:180,Artist - Song\n{}\n", audio.display()),
        )
        .unwrap();
        let store = Store::open(&dir.join("db.sqlite")).unwrap();
        let track = Track {
            id: "local:generated-archive-audio".into(),
            provider: "local".into(),
            provider_id: "generated-archive-audio".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            source_url: "local:generated-archive-audio".into(),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        store
            .save_asset(&crate::models::LocalAsset {
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_seconds: None,
                size_bytes: None,
                bitrate_origin: None,
                id: "asset-generated-archive-audio".into(),
                recording_id: track.id.clone(),
                path: audio.to_string_lossy().into_owned(),
                hash: "archive-audio-hash".into(),
                source_codec: "mp3".into(),
                final_codec: "mp3".into(),
                source_bitrate: None,
                final_bitrate: None,
                processing: "imported".into(),
                track,
            })
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![music.join("Playlists").to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let scan = playlist_files::LocalPlaylistScan {
            files: vec![playlist_files::LocalPlaylistFile {
                path: path.to_string_lossy().into_owned(),
                name: path.file_stem().unwrap().to_string_lossy().into_owned(),
                modified: None,
                track_count: 1,
                warning: None,
            }],
            roots: Vec::new(),
            warnings: Vec::new(),
        };
        (store, settings, path, audio, scan)
    }

    fn empty_scan() -> playlist_files::LocalPlaylistScan {
        playlist_files::LocalPlaylistScan {
            files: Vec::new(),
            roots: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn startup_migration_soft_deletes_unchanged_archive_self_import_and_preserves_membership() {
        let dir = tempfile::tempdir().unwrap();
        let (store, settings, archive, audio, scan) = generated_archive_fixture(dir.path());
        let imported = sync_playlists_with_scan(&store, &settings, scan).unwrap();
        let collection_id = imported.sources[0]
            .collection_id
            .clone()
            .expect("archive was imported before migration");
        assert_eq!(store.list_collections().unwrap().len(), 1);
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection_id))
                .unwrap()
                .len(),
            1
        );
        let source_bytes = fs::read(&archive).unwrap();

        let migrated = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();

        assert_eq!(migrated.migrated_generated_imports, 1);
        assert!(store.list_collections().unwrap().is_empty());
        let deleted = store.list_deleted_collections().unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].id, collection_id);
        assert_eq!(deleted[0].track_count, 1);
        assert_eq!(fs::read(&archive).unwrap(), source_bytes);
        assert!(audio.is_file());
        assert_eq!(store.list_assets().unwrap().len(), 1);
        let source_key = fs::canonicalize(archive)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(read_state(&store).unwrap().sources[&source_key]
            .collection_id
            .is_none());
    }

    #[test]
    fn startup_migration_keeps_user_edited_archive_collection() {
        let dir = tempfile::tempdir().unwrap();
        let (store, settings, _archive, _audio, scan) = generated_archive_fixture(dir.path());
        let imported = sync_playlists_with_scan(&store, &settings, scan).unwrap();
        let collection_id = imported.sources[0]
            .collection_id
            .clone()
            .expect("archive was imported before migration");
        let added_track = store
            .save_track(&Track {
                id: "fixture:user-added-track".into(),
                provider: "fixture".into(),
                provider_id: "user-added-track".into(),
                artist: "User".into(),
                title: "Added track".into(),
                ..Track::default()
            })
            .unwrap();
        store
            .replace_playlist_entries(&collection_id, &[added_track])
            .unwrap();

        let migrated = sync_playlists_with_scan(&store, &settings, empty_scan()).unwrap();

        assert_eq!(migrated.migrated_generated_imports, 1);
        let active = store.list_collections().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, collection_id);
        assert_eq!(active[0].track_count, 1);
        assert!(store.list_deleted_collections().unwrap().is_empty());
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection_id))
                .unwrap()[0]
                .id,
            "fixture:user-added-track"
        );
    }

    #[test]
    fn mixxx_export_keeps_local_duplicates_and_reports_skipped_remote_entries() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let local = save_local(&store, &music.join("known.mp3"), "known", "Known", "Track");
        let remote = Track {
            id: "spotify:remote-id".into(),
            provider: "spotify".into(),
            provider_id: "remote-id".into(),
            source_url: "https://open.spotify.com/track/remote-id".into(),
            artist: "Remote".into(),
            title: "Unavailable".into(),
            ..Track::default()
        };
        store.save_track(&remote).unwrap();
        let collection = store.create_collection("Mixxx set").unwrap();
        store
            .replace_playlist_entries(
                &collection.id,
                &[local.clone(), remote.clone(), local.clone()],
            )
            .unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let result = export_mixxx_playlist(&store, &settings, &collection.id).unwrap();
        assert_eq!(
            PathBuf::from(&result.path),
            music.join("Playlists").join("Mixxx").join("Mixxx set.m3u8")
        );
        assert_eq!((result.exported, result.total), (2, 3));
        assert_eq!(result.missing.len(), 1);
        assert_eq!(result.missing[0].id, remote.id);
        let contents = fs::read_to_string(&result.path).unwrap();
        assert!(contents.starts_with("#EXTM3U\n#BURRATA-MIRROR\n#PLAYLIST:Mixxx set\n"));
        assert!(!Path::new(&result.path)
            .to_string_lossy()
            .contains(&collection.id));
        assert_eq!(
            contents
                .matches(
                    &fs::canonicalize(local.local_path.as_deref().unwrap())
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                )
                .count(),
            2
        );
        assert!(!contents.contains("https://open.spotify.com/track/remote-id"));
        assert_eq!(
            contents
                .lines()
                .filter(|line| line.starts_with("#EXTINF:"))
                .count(),
            2
        );
    }

    #[test]
    fn named_exports_are_portable_readable_and_do_not_overwrite_case_collisions() {
        assert_eq!(safe_playlist_filename("  Café / CON.  "), "Café CON");
        assert_eq!(safe_playlist_filename("../NUL"), "_NUL");
        assert_eq!(safe_playlist_filename("..."), "Playlist");

        let dir = tempfile::tempdir().unwrap();
        let first = write_named_export(dir.path(), "one", "Summer / Set", b"first").unwrap();
        let second = write_named_export(dir.path(), "two", "summer : set", b"second").unwrap();
        assert_eq!(first.file_name().unwrap(), "Summer Set.m3u8");
        assert_eq!(second.file_name().unwrap(), "summer set (2).m3u8");
        assert_eq!(fs::read(&first).unwrap(), b"first");
        assert_eq!(fs::read(&second).unwrap(), b"second");
        write_named_export(dir.path(), "one", "Summer / Set", b"updated").unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"updated");

        let same_title_one =
            write_named_export(dir.path(), "same-one", "Identical", b"same").unwrap();
        let same_title_two =
            write_named_export(dir.path(), "same-two", "Identical", b"same").unwrap();
        assert_ne!(same_title_one, same_title_two);

        fs::write(&first, b"edited outside Burrata").unwrap();
        let preserved_replacement =
            write_named_export(dir.path(), "one", "Summer / Set", b"new export").unwrap();
        assert_ne!(preserved_replacement, first);
        assert_eq!(fs::read(&first).unwrap(), b"edited outside Burrata");
    }

    #[test]
    fn named_export_index_rejects_paths_outside_export_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("outside.m3u8"), b"must stay untouched").unwrap();
        fs::write(
            dir.path().join(".burrata-exports.json"),
            r#"{"owner":{"filename":"../outside.m3u8","sha256":"bad"}}"#,
        )
        .unwrap();
        let result = write_named_export(dir.path(), "owner", "Safe", b"export").unwrap();
        assert_eq!(result.file_name().unwrap(), "Safe.m3u8");
        assert_eq!(
            fs::read(dir.path().join("outside.m3u8")).unwrap(),
            b"must stay untouched"
        );
    }

    #[test]
    fn playlist_export_uses_title_in_a_separate_user_facing_file() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let collection = store.create_collection("Crème / Brûlée").unwrap();
        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..Settings::default()
        };

        let exported =
            PathBuf::from(export_playlist_path(&store, &settings, &collection.id).unwrap());

        assert_eq!(exported.file_name().unwrap(), "Crème Brûlée.m3u8");
        assert!(exported.starts_with(music.join("Playlists").join("Exports")));
        assert!(!exported.to_string_lossy().contains(&collection.id));
        assert!(fs::read_to_string(exported)
            .unwrap()
            .contains("#PLAYLIST:Crème / Brûlée"));
    }

    fn portable_review_text() -> String {
        let track = crate::portable_playlist::PortableTrack {
            title: "New Song".into(),
            artists: vec![crate::portable_playlist::ArtistCredit {
                name: "New Artist".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let input = crate::portable_playlist::PlaylistInput {
            name: "Imported Mix".into(),
            folder_ancestry: vec![crate::portable_playlist::NamedItem {
                id: "source-folder".into(),
                name: "Source Folder".into(),
                ..Default::default()
            }],
            sections: vec![crate::portable_playlist::NamedItem {
                id: "source-section".into(),
                name: "Imported Section".into(),
                ..Default::default()
            }],
            entries: (0..2)
                .map(|index| crate::portable_playlist::PortableEntry {
                    id: format!("e{index}"),
                    section_id: Some("source-section".into()),
                    note: Some(format!("incoming note {index}")),
                    track: track.clone(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        crate::portable_playlist::serialize(
            &input,
            &crate::portable_playlist::PlaylistExportOptions {
                purpose: crate::portable_playlist::PlaylistPurpose::Share,
                include_notes: true,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn share_preview_and_render_ignore_unavailable_local_mirror_roots() {
        for music_root_available in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let music_root = directory.path().join("Music");
            if music_root_available {
                fs::create_dir_all(&music_root).unwrap();
            }
            let audio_dir = directory.path().join("LibraryAudio");
            fs::create_dir_all(&audio_dir).unwrap();
            let audio_path = audio_dir.join("private-song.mp3");
            let store = Store::open(&directory.path().join("source.sqlite")).unwrap();
            let mut track = save_local(&store, &audio_path, "share-local", "Artist", "Song");
            track.provenance =
                serde_json::json!({"candidateUrl":"https://private.example/audio?token=secret"});
            store.save_track(&track).unwrap();
            let collection = store.create_collection("Shared Local Mix").unwrap();
            store
                .replace_playlist_entries(&collection.id, &[track.clone()])
                .unwrap();
            store
                .save_settings(&Settings {
                    music_directory: music_root.to_string_lossy().into_owned(),
                    ..Settings::default()
                })
                .unwrap();

            let options = crate::portable_playlist::PlaylistExportOptions {
                purpose: crate::portable_playlist::PlaylistPurpose::Share,
                ..Default::default()
            };
            let preview =
                preview_portable_playlist_export(&store, &collection.id, &options).unwrap();
            let rendered =
                render_portable_playlist_export(&store, &collection.id, &options).unwrap();
            assert_eq!(preview.entry_count, 1);
            assert!(preview.identity_placeholders > 0);
            assert!(!rendered.contains(&audio_path.to_string_lossy().to_string()));
            assert!(!rendered.contains("private.example"));
            assert!(!rendered.contains("token=secret"));
            assert!(!music_root.join("Playlists").exists());
        }
    }

    #[test]
    fn share_export_uses_downloads_fallback_when_music_directory_is_empty() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = directory.path().join("Downloads");
        fs::create_dir_all(&downloads).unwrap();
        let audio_path = directory.path().join("LibraryAudio/private-song.mp3");
        fs::create_dir_all(audio_path.parent().unwrap()).unwrap();
        let store = Store::open(&directory.path().join("source.sqlite")).unwrap();
        let mut track = save_local(&store, &audio_path, "fallback-share", "Artist", "Song");
        track.provenance =
            serde_json::json!({"candidateUrl":"https://private.example/audio?token=secret"});
        store.save_track(&track).unwrap();
        let collection = store.create_collection("Shared Local Mix").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[track])
            .unwrap();
        store
            .save_settings(&Settings {
                music_directory: String::new(),
                ..Settings::default()
            })
            .unwrap();

        let share_options = crate::portable_playlist::PlaylistExportOptions {
            purpose: crate::portable_playlist::PlaylistPurpose::Share,
            ..Default::default()
        };
        let exported = export_portable_playlist_path_with_fallback(
            &store,
            &collection.id,
            &share_options,
            Some(&downloads),
        )
        .unwrap();
        let output_path = PathBuf::from(exported);
        assert!(output_path.starts_with(&downloads));
        let body = fs::read_to_string(output_path).unwrap();
        assert!(!body.contains(&audio_path.to_string_lossy().to_string()));
        assert!(!body.contains("private.example"));
        assert!(!body.contains("token=secret"));
        assert!(!directory.path().join("Music").exists());

        let local_mirror_options = crate::portable_playlist::PlaylistExportOptions {
            purpose: crate::portable_playlist::PlaylistPurpose::LocalMirror,
            ..Default::default()
        };
        assert_eq!(
            export_portable_playlist_path_with_fallback(
                &store,
                &collection.id,
                &local_mirror_options,
                Some(&downloads),
            )
            .unwrap_err(),
            "Choose a music directory first"
        );
    }

    #[test]
    fn portable_share_file_and_copy_import_as_equivalent_private_playlists() {
        let directory = tempfile::tempdir().unwrap();
        let music = directory.path().join("Music");
        fs::create_dir_all(&music).unwrap();
        let source = Store::open(&directory.path().join("source.sqlite")).unwrap();
        source
            .save_settings(&Settings {
                music_directory: music.to_string_lossy().into_owned(),
                ..Settings::default()
            })
            .unwrap();
        let collection = source.create_collection("Shared Mix").unwrap();
        let track = Track {
            id: "fixture:private-provider-id".into(),
            provider: "fixture".into(),
            provider_id: "private-provider-id".into(),
            source_url: "https://cdn.example/private.flac?signature=private-token".into(),
            artist: "Fixture Artist".into(),
            title: "Fixture Song".into(),
            local_path: Some("/private/library/fixture.flac".into()),
            provenance: serde_json::json!({"rating":5,"tags":["private-tag"]}),
            ..Default::default()
        };
        source.save_track(&track).unwrap();
        source
            .replace_playlist_entries(&collection.id, &[track.clone(), track])
            .unwrap();
        let occurrences = source.list_playlist_occurrences(&collection.id).unwrap();
        let hierarchy = crate::playlist_hierarchy::PlaylistHierarchy {
            schema_version: 1,
            playlists: vec![crate::playlist_hierarchy::PlaylistPlacement {
                collection_id: collection.id.clone(),
                sections: vec![crate::playlist_hierarchy::PlaylistSection {
                    id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                    name: "First set".into(),
                    position: 0,
                }],
                occurrences: occurrences
                    .iter()
                    .map(|occurrence| crate::playlist_hierarchy::OccurrenceMetadata {
                        id: occurrence.occurrence_id.clone(),
                        section_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()),
                        note: Some("private note".into()),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        crate::playlist_hierarchy::save(&source, &hierarchy).unwrap();

        let options = crate::portable_playlist::PlaylistExportOptions {
            purpose: crate::portable_playlist::PlaylistPurpose::Share,
            ..Default::default()
        };
        let copied_text =
            render_portable_playlist_export(&source, &collection.id, &options).unwrap();
        let exported_path =
            export_portable_playlist_path(&source, &collection.id, &options).unwrap();
        let file_text = fs::read_to_string(exported_path).unwrap();
        let normalized_file_text = file_text.replacen("#EXTM3U\n#BURRATA-EXPORT\n", "#EXTM3U\n", 1);
        assert_eq!(normalized_file_text, copied_text);
        for private_value in [
            "/private/library",
            "private-provider-id",
            "signature=private-token",
            "private note",
            "private-tag",
            "\"rating\":5",
        ] {
            assert!(
                !copied_text.contains(private_value),
                "share output leaked {private_value}"
            );
        }

        let mut imported = Vec::new();
        for (payload, matching_level) in [
            crate::recording_match::MatchLevel::Exact,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchLevel::Flexible,
        ]
        .into_iter()
        .flat_map(|level| [(&copied_text, level), (&file_text, level)])
        {
            let recipient = Store::open(
                &directory
                    .path()
                    .join(format!("recipient-{}.sqlite", imported.len())),
            )
            .unwrap();
            let import_options = PortablePlaylistImportOptions {
                matching_level,
                ..PortablePlaylistImportOptions::default()
            };
            let preview_id = install_portable_review(&recipient, payload, &import_options);
            let result = apply_portable_playlist_import(
                &recipient,
                payload,
                &preview_id,
                &digest(payload.as_bytes()),
                None,
                None,
            )
            .unwrap();
            let tracks = recipient
                .list_tracks("collection", Some(&result.collection.id))
                .unwrap();
            assert!(tracks.iter().all(|track| track.provider == "import"));
            let hierarchy = crate::playlist_hierarchy::load(&recipient).unwrap();
            let placement = hierarchy
                .playlists
                .iter()
                .find(|placement| placement.collection_id == result.collection.id)
                .unwrap();
            imported.push((
                result.collection.name,
                tracks
                    .iter()
                    .map(|track| track.title.clone())
                    .collect::<Vec<_>>(),
                placement
                    .sections
                    .iter()
                    .map(|section| section.name.clone())
                    .collect::<Vec<_>>(),
                placement
                    .occurrences
                    .iter()
                    .map(|occurrence| {
                        occurrence.section_id.as_deref().and_then(|id| {
                            placement
                                .sections
                                .iter()
                                .find(|section| section.id == id)
                                .map(|section| section.name.clone())
                        })
                    })
                    .collect::<Vec<_>>(),
            ));
        }
        assert!(imported.iter().all(|result| result == &imported[0]));
        assert_eq!(imported[0].0, "Shared Mix");
        assert_eq!(imported[0].1, ["Fixture Song", "Fixture Song"]);
        assert_eq!(imported[0].2, ["First set"]);
        assert_eq!(
            imported[0].3,
            [Some("First set".into()), Some("First set".into())]
        );
    }

    fn install_portable_review(
        store: &Store,
        text: &str,
        options: &PortablePlaylistImportOptions,
    ) -> String {
        let target = portable_import_target(store, options).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        store
            .set_provider_cache(
                "portable-playlist-import-preview.v1",
                &id,
                &serde_json::json!({
                    "sourceHash":digest(text.as_bytes()),
                    "matchingLevel":options.matching_level,
                    "mode":options.mode,
                    "targetCollectionId":target.as_ref().map(|target| &target.0.id),
                    "targetFingerprint":target.as_ref().map(|target| &target.2),
                    "duplicatePolicy":options.duplicate_policy,
                    "destinationName":options.destination_name,
                    "destinationFolderPath":options.destination_folder_path,
                }),
                1800,
            )
            .unwrap();
        id
    }

    #[test]
    fn portable_import_persists_only_reviewed_bindings_for_create_update_and_merge() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("db.sqlite")).unwrap();
        let mut input = crate::portable_playlist::parse(&portable_review_text()).unwrap();
        input.entries.insert(
            1,
            crate::portable_playlist::PortableEntry {
                id: "middle".into(),
                track: crate::portable_playlist::PortableTrack {
                    title: "A Minor Song".into(),
                    artists: vec![crate::portable_playlist::ArtistCredit {
                        name: "A Minor Artist".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let text = crate::portable_playlist::serialize(
            &input,
            &crate::portable_playlist::PlaylistExportOptions {
                purpose: crate::portable_playlist::PlaylistPurpose::Share,
                ..Default::default()
            },
        )
        .unwrap();
        let make_binding = |source_id: &str| {
            serde_json::json!({
                "provider":"youtube",
                "sourceId":source_id,
                "matchingLevel":crate::recording_match::MatchLevel::Balanced,
                "sourceHash":digest(text.as_bytes()),
            })
        };
        let mut selected = HashMap::new();
        selected.insert(0, make_binding("reviewed-c-major"));
        selected.insert(2, make_binding("reviewed-c-major"));
        let mut unresolved = input.entries[0].track.to_track();
        unresolved.provenance["userOwnedNote"] = serde_json::json!("preserve me");
        store.save_track(&unresolved).unwrap();

        let create_options = PortablePlaylistImportOptions::default();
        let preview_id = install_portable_review(&store, &text, &create_options);
        let created = apply_portable_playlist_import_core(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
            &selected,
        )
        .unwrap();
        let tracks = store
            .list_tracks("collection", Some(&created.collection.id))
            .unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].id, tracks[2].id);
        assert_eq!(
            tracks[0].provenance["portablePlaybackSource"]["sourceId"],
            "reviewed-c-major"
        );
        assert_eq!(
            tracks[2].provenance["portablePlaybackSource"]["sourceId"],
            "reviewed-c-major"
        );
        assert!(tracks[1].provenance.get("portablePlaybackSource").is_none());
        assert_eq!(tracks[0].provenance["userOwnedNote"], "preserve me");

        let mut invalid_selection = HashMap::new();
        invalid_selection.insert(99, make_binding("out-of-range"));
        let before_invalid = store
            .list_tracks("collection", Some(&created.collection.id))
            .unwrap();
        assert!(apply_portable_playlist_import_core(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
            &invalid_selection,
        )
        .is_err());
        assert_eq!(
            store
                .list_tracks("collection", Some(&created.collection.id))
                .unwrap(),
            before_invalid
        );

        let mut manual_edit = store.get_track(&tracks[0].id).unwrap().unwrap();
        manual_edit.title = "Manual title preserved".into();
        store.update_track(&manual_edit).unwrap();
        let alternate = make_binding("reviewed-alternate-c-major");
        selected.insert(0, alternate.clone());
        selected.insert(2, alternate);

        let update_options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Update,
            target_collection_id: Some(created.collection.id.clone()),
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &update_options);
        apply_portable_playlist_import_core(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
            &selected,
        )
        .unwrap();
        let updated = store.get_track(&tracks[0].id).unwrap().unwrap();
        assert_eq!(updated.title, "Manual title preserved");
        assert_eq!(updated.provenance["userOwnedNote"], "preserve me");
        assert_eq!(
            updated.provenance["portablePlaybackSource"]["sourceId"],
            "reviewed-alternate-c-major"
        );
        assert!(store
            .get_track(&tracks[1].id)
            .unwrap()
            .unwrap()
            .provenance
            .get("portablePlaybackSource")
            .is_none());
        let repeated_preview = install_portable_review(&store, &text, &update_options);
        apply_portable_playlist_import_core(
            &store,
            &text,
            &repeated_preview,
            &digest(text.as_bytes()),
            None,
            None,
            &selected,
        )
        .unwrap();
        let repeated = store.get_track(&tracks[0].id).unwrap().unwrap();
        assert_eq!(repeated.provenance["userOwnedNote"], "preserve me");
        assert_eq!(
            repeated.provenance["portablePlaybackSource"]["sourceId"],
            "reviewed-alternate-c-major"
        );

        let skip_options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Merge,
            target_collection_id: Some(created.collection.id.clone()),
            duplicate_policy: PortablePlaylistDuplicatePolicy::Skip,
            destination_name: Some("Would have been renamed".into()),
            ..Default::default()
        };
        let skip_preview = install_portable_review(&store, &text, &skip_options);
        let before_skipped_review = store
            .list_tracks("collection", Some(&created.collection.id))
            .unwrap();
        assert!(apply_portable_playlist_import_core(
            &store,
            &text,
            &skip_preview,
            &digest(text.as_bytes()),
            skip_options.destination_name.as_deref(),
            None,
            &selected,
        )
        .unwrap_err()
        .contains("choose Append or Update"));
        assert_eq!(
            store
                .list_tracks("collection", Some(&created.collection.id))
                .unwrap(),
            before_skipped_review
        );
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .into_iter()
                .find(|item| item.id == created.collection.id)
                .unwrap()
                .name,
            "Imported Mix"
        );

        let merge_options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Merge,
            target_collection_id: Some(created.collection.id.clone()),
            duplicate_policy: PortablePlaylistDuplicatePolicy::Keep,
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &merge_options);
        apply_portable_playlist_import_core(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
            &selected,
        )
        .unwrap();
        let merged = store
            .list_tracks("collection", Some(&created.collection.id))
            .unwrap();
        assert_eq!(merged.len(), 6);
        assert_eq!(
            merged[3].provenance["portablePlaybackSource"]["sourceId"],
            "reviewed-alternate-c-major"
        );
        assert_eq!(
            merged[5].provenance["portablePlaybackSource"]["sourceId"],
            "reviewed-alternate-c-major"
        );
        assert!(merged[4].provenance.get("portablePlaybackSource").is_none());
    }

    #[test]
    fn malformed_import_provenance_is_rejected_before_create_or_update_and_rolls_back_storage() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("db.sqlite")).unwrap();
        let text = portable_review_text();
        let input = crate::portable_playlist::parse(&text).unwrap();
        let mut malformed = input.entries[0].track.to_track();
        malformed.provenance = serde_json::json!("opaque legacy value");
        store.save_track(&malformed).unwrap();
        let stored_metadata = || -> String {
            let conn = rusqlite::Connection::open(directory.path().join("db.sqlite")).unwrap();
            conn.query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&malformed.id],
                |row| row.get(0),
            )
            .unwrap()
        };
        let original_metadata = stored_metadata();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&original_metadata).unwrap()["provenance"],
            serde_json::json!("opaque legacy value")
        );
        let binding = serde_json::json!({
            "provider":"youtube",
            "sourceId":"reviewed-source",
            "matchingLevel":crate::recording_match::MatchLevel::Balanced,
            "sourceHash":digest(text.as_bytes()),
        });
        let mut reviewed = HashMap::new();
        reviewed.insert(0, binding.clone());
        reviewed.insert(1, binding);

        let create_options = PortablePlaylistImportOptions::default();
        let preview_id = install_portable_review(&store, &text, &create_options);
        let before_create = store.list_collections().unwrap();
        assert!(apply_portable_playlist_import_core(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
            &reviewed,
        )
        .unwrap_err()
        .contains("malformed or non-import metadata"));
        assert_eq!(store.list_collections().unwrap().len(), before_create.len());
        assert_eq!(stored_metadata(), original_metadata);

        let collection = store.create_collection("Keep this name").unwrap();
        let old_track = Track {
            id: "fixture:keep-entry".into(),
            provider: "fixture".into(),
            provider_id: "keep-entry".into(),
            title: "Keep entry".into(),
            ..Default::default()
        };
        store.save_track(&old_track).unwrap();
        store
            .replace_playlist_entries(&collection.id, std::slice::from_ref(&old_track))
            .unwrap();
        let before_malformed = store.get_track(&malformed.id).unwrap().unwrap();
        let update_options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Update,
            target_collection_id: Some(collection.id.clone()),
            destination_name: Some("Must not rename".into()),
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &update_options);
        let before_entries = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert!(apply_portable_playlist_import_core(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            update_options.destination_name.as_deref(),
            None,
            &reviewed,
        )
        .unwrap_err()
        .contains("malformed or non-import metadata"));
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .into_iter()
                .find(|item| item.id == collection.id)
                .unwrap()
                .name,
            "Keep this name"
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap(),
            before_entries
        );

        let mut single_review = HashMap::new();
        single_review.insert(0, reviewed[&0].clone());
        assert!(store
            .replace_playlist_entries_with_portable_sources(
                &collection.id,
                &[malformed.clone()],
                &single_review,
            )
            .unwrap_err()
            .contains("valid import placeholder"));
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap(),
            before_entries
        );
        assert_eq!(stored_metadata(), original_metadata);
        assert_eq!(
            store.get_track(&malformed.id).unwrap().unwrap(),
            before_malformed
        );
    }

    #[test]
    fn portable_import_update_and_merge_preserve_reviewed_order_hierarchy_and_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("db.sqlite")).unwrap();
        let existing = Track {
            id: "fixture:existing".into(),
            provider: "fixture".into(),
            provider_id: "existing".into(),
            artist: "Old Artist".into(),
            title: "Old Song".into(),
            ..Default::default()
        };
        store.save_track(&existing).unwrap();
        let collection = store.create_collection("Target Playlist").unwrap();
        store
            .replace_playlist_entries(&collection.id, std::slice::from_ref(&existing))
            .unwrap();
        crate::playlist_hierarchy::set_folder_ancestry(
            &store,
            &collection.id,
            &["Keep".into(), "Folder".into()],
        )
        .unwrap();
        crate::playlist_hierarchy::set_sections(
            &store,
            &collection.id,
            &[crate::portable_playlist::NamedItem {
                id: "old-section".into(),
                name: "Existing Section".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let mut hierarchy = crate::playlist_hierarchy::load(&store).unwrap();
        let placement = hierarchy
            .playlists
            .iter_mut()
            .find(|placement| placement.collection_id == collection.id)
            .unwrap();
        let old_occurrence = store.list_playlist_occurrences(&collection.id).unwrap()[0]
            .occurrence_id
            .clone();
        if let Some(occurrence) = placement
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.id == old_occurrence)
        {
            occurrence.note = Some("keep this annotation".into());
        } else {
            placement
                .occurrences
                .push(crate::playlist_hierarchy::OccurrenceMetadata {
                    id: old_occurrence.clone(),
                    note: Some("keep this annotation".into()),
                    ..Default::default()
                });
        }
        crate::playlist_hierarchy::save(&store, &hierarchy).unwrap();

        let text = portable_review_text();
        let merge_options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Merge,
            target_collection_id: Some(collection.id.clone()),
            duplicate_policy: PortablePlaylistDuplicatePolicy::Skip,
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &merge_options);
        let result = apply_portable_playlist_import(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.collection.id, collection.id);
        assert_eq!(result.imported_count, 2); // Skip applies only to destination duplicates; source repeats remain occurrences.
        let merged = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(merged.len(), 3);
        let after_merge = crate::playlist_hierarchy::load(&store).unwrap();
        let placement = after_merge
            .playlists
            .iter()
            .find(|placement| placement.collection_id == collection.id)
            .unwrap();
        let folder_names = after_merge
            .folders
            .iter()
            .filter(|folder| placement.folder_id.as_deref() == Some(folder.id.as_str()))
            .map(|folder| folder.name.as_str())
            .collect::<Vec<_>>();
        assert!(folder_names.contains(&"Folder"));
        assert!(placement
            .sections
            .iter()
            .any(|section| section.name == "Existing Section"));
        assert!(placement
            .sections
            .iter()
            .any(|section| section.name == "Imported Section"));
        assert!(placement
            .occurrences
            .iter()
            .any(|occurrence| occurrence.id == old_occurrence
                && occurrence.note.as_deref() == Some("keep this annotation")));
        assert!(placement
            .occurrences
            .iter()
            .any(|occurrence| occurrence.note.as_deref() == Some("incoming note 0")));
        assert!(placement
            .occurrences
            .iter()
            .any(|occurrence| occurrence.note.as_deref() == Some("incoming note 1")));

        let update_options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Update,
            target_collection_id: Some(collection.id.clone()),
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &update_options);
        let updated = apply_portable_playlist_import(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(updated.collection.name, "Target Playlist");
        let replaced = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(replaced.len(), 2);
        assert_eq!(replaced[0].id, replaced[1].id); // Update retains source duplicate occurrences.
        let after_update = crate::playlist_hierarchy::load(&store).unwrap();
        let placement = after_update
            .playlists
            .iter()
            .find(|placement| placement.collection_id == collection.id)
            .unwrap();
        assert_eq!(placement.sections.len(), 1);
        assert_eq!(placement.sections[0].name, "Imported Section");
        assert!(after_update
            .folders
            .iter()
            .any(
                |folder| placement.folder_id.as_deref() == Some(folder.id.as_str())
                    && folder.name == "Folder"
            ));

        let repeat_merge = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Merge,
            target_collection_id: Some(collection.id.clone()),
            duplicate_policy: PortablePlaylistDuplicatePolicy::Skip,
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &repeat_merge);
        let skipped = apply_portable_playlist_import(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(skipped.imported_count, 0);
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn portable_import_review_rejects_stale_target_and_provider_policy_is_shared() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("db.sqlite")).unwrap();
        let collection = store.create_collection("Target").unwrap();
        let text = portable_review_text();
        let options = PortablePlaylistImportOptions {
            mode: PortablePlaylistImportMode::Update,
            target_collection_id: Some(collection.id.clone()),
            matching_level: crate::recording_match::MatchLevel::Flexible,
            ..Default::default()
        };
        let preview_id = install_portable_review(&store, &text, &options);
        store
            .rename_collection(&collection.id, "Changed after review")
            .unwrap();
        assert!(apply_portable_playlist_import(
            &store,
            &text,
            &preview_id,
            &digest(text.as_bytes()),
            None,
            None,
        )
        .unwrap_err()
        .contains("changed after preview"));

        let requested = Track {
            id: "import:policy-fixture".into(),
            provider: "import".into(),
            provider_id: "policy-fixture".into(),
            artist: "Policy Artist".into(),
            title: "Policy Song".into(),
            duration_seconds: Some(180.0),
            ..Default::default()
        };
        let candidate = Candidate {
            id: "youtube:policy-video".into(),
            provider: "youtube".into(),
            title: "Policy Song".into(),
            uploader: Some("Policy Artist".into()),
            duration_seconds: Some(180.0),
            source_metadata: Some(
                serde_json::json!({"provider":"youtube","trackId":"policy-video","catalogArtist":"Policy Artist","catalogTitle":"Policy Song","duration":180.0,"sourceMetadataOrigin":"ytDlpCandidateMetadata"}),
            ),
            ..Default::default()
        };
        let balanced = assess_portable_provider_candidates(
            &store,
            &requested,
            vec![candidate.clone()],
            crate::recording_match::MatchLevel::Balanced,
        );
        let exact = assess_portable_provider_candidates(
            &store,
            &requested,
            vec![candidate],
            crate::recording_match::MatchLevel::Exact,
        );
        assert_eq!(
            balanced[0].1.action,
            crate::recording_match::MatchAction::Inspect
        );
        assert_ne!(
            exact[0].1.action,
            crate::recording_match::MatchAction::Accept
        );

        // A provider candidate with moderately different runtime can be shown
        // as a probable review suggestion under Flexible. Exact and Balanced
        // do not auto-accept it, and search-stage Flexible still requires review.
        let relaxed_request = Track {
            id: "import:relaxed-policy-fixture".into(),
            provider: "import".into(),
            provider_id: "relaxed-policy-fixture".into(),
            artist: "Main Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Default::default()
        };
        let relaxed_candidate = Candidate {
            id: "youtube:relaxed-policy-fixture".into(),
            provider: "youtube".into(),
            title: "Song".into(),
            duration_seconds: Some(220.0),
            source_metadata: Some(serde_json::json!({
                "catalogTitle":"Song",
                "artists":["Main Artist"],
                "artistCount":1,
                "duration":220.0,
                "isrc":null
            })),
            ..Default::default()
        };
        let balanced = assess_portable_provider_candidates(
            &store,
            &relaxed_request,
            vec![relaxed_candidate.clone()],
            crate::recording_match::MatchLevel::Balanced,
        );
        let exact = assess_portable_provider_candidates(
            &store,
            &relaxed_request,
            vec![relaxed_candidate.clone()],
            crate::recording_match::MatchLevel::Exact,
        );
        let flexible = assess_portable_provider_candidates(
            &store,
            &relaxed_request,
            vec![relaxed_candidate.clone()],
            crate::recording_match::MatchLevel::Flexible,
        );
        assert_ne!(
            balanced[0].1.action,
            crate::recording_match::MatchAction::Accept
        );
        assert_ne!(
            exact[0].1.action,
            crate::recording_match::MatchAction::Accept
        );
        assert_eq!(
            flexible[0].1.relationship,
            crate::recording_match::MatchRelationship::Probable
        );
        assert_eq!(
            flexible[0].1.action,
            crate::recording_match::MatchAction::Inspect
        );
        assert_eq!(
            portable_import_providers(&Settings {
                preferred_streaming_source: "soundcloud".into(),
                streaming_fallback_enabled: true,
                streaming_fallback_sources: vec![
                    "youtube".into(),
                    "soundcloud".into(),
                    "slskd".into()
                ],
                ..Settings::default()
            }),
            vec!["soundcloud", "youtube"]
        );
    }

    #[test]
    fn portable_provider_identity_is_opaque_and_rejects_url_material() {
        let candidate = Candidate {
            id: "youtube:abc_123".into(),
            provider: "youtube".into(),
            url: "https://cdn.example/video?signature=secret".into(),
            title: "A title".into(),
            ..Default::default()
        };
        let source_id = portable_provider_source_id(&candidate).unwrap();
        assert_eq!(source_id, "abc_123");
        assert_eq!(
            portable_provider_selection_id("youtube", &source_id).as_deref(),
            Some("youtube:abc_123")
        );
        assert!(portable_candidate_matches_selection(
            &candidate, "youtube", "abc_123"
        ));
        assert!(!portable_candidate_matches_selection(
            &candidate,
            "youtube",
            "abc_123?signature=secret"
        ));
        assert!(portable_provider_source_id(&Candidate {
            id: "youtube:https://example.invalid/video?id=secret".into(),
            ..candidate
        })
        .is_none());
    }
}
