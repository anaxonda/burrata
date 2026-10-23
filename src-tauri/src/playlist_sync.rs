//! Local playlist synchronization and stable M3U mirrors.
//!
//! The source files remain user-owned. This module records canonical source bindings,
//! keeps immutable content snapshots in the application database, and writes app-owned
//! mirrors atomically under `Music/Playlists`.
use crate::{
    models::{Settings, Track},
    playlist_files,
    storage::Store,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
static PLAYLIST_SYNC_LOCK: Mutex<()> = Mutex::new(());
static PLAYLIST_EXPORT_LOCK: Mutex<()> = Mutex::new(());

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
    #[serde(default)]
    mirror_snapshots: HashMap<String, Vec<SourceSnapshot>>,
    #[serde(default)]
    protected_copies: HashMap<String, String>,
    #[serde(skip)]
    preserve_mirrors: HashSet<String>,
    #[serde(skip)]
    changed_sources: HashSet<String>,
    #[serde(skip)]
    paused_mirrors: HashSet<String>,
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
    let perf_trace = std::env::var_os("BURRATA_PERF_TRACE").is_some();
    let total_started = std::time::Instant::now();
    let scan_started = std::time::Instant::now();
    let scan = playlist_files::scan_local_playlists(settings)?;
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.scan files={} warnings={} elapsed_ms={}",
            scan.files.len(),
            scan.warnings.len(),
            scan_started.elapsed().as_millis()
        );
    }
    let sync_started = std::time::Instant::now();
    let result = sync_playlists_with_scan(store, settings, scan);
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.total outcome={} elapsed_ms={}",
            if result.is_ok() { "ok" } else { "error" },
            total_started.elapsed().as_millis()
        );
        eprintln!(
            "[perf] playlist_sync.reconcile_lock_inclusive elapsed_ms={}",
            sync_started.elapsed().as_millis()
        );
    }
    result
}

fn sync_playlists_with_scan(
    store: &Store,
    settings: &Settings,
    scan: playlist_files::LocalPlaylistScan,
) -> Result<PlaylistSyncStatus> {
    let perf_trace = std::env::var_os("BURRATA_PERF_TRACE").is_some();
    let lock_started = std::time::Instant::now();
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    if perf_trace {
        eprintln!(
            "[perf] playlist_sync.lock_wait elapsed_ms={}",
            lock_started.elapsed().as_millis()
        );
    }
    let reconcile_started = std::time::Instant::now();
    let mut status = PlaylistSyncStatus::default();
    status.warnings.extend(scan.warnings);
    let state_started = std::time::Instant::now();
    let mut state = read_state(store)?;
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
                    && state
                        .sources
                        .get(&key)
                        .is_some_and(|source| source.collection_id.is_none())
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
                    if let Some(collection_id) = state
                        .sources
                        .get(&key)
                        .and_then(|source| source.collection_id.as_deref())
                    {
                        status.repaired_track_metadata +=
                            playlist_files::repair_placeholder_url_metadata(
                                store,
                                collection_id,
                                &contents,
                            )?;
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
    status.mirrors_directory = write_mirrors(store, settings, &mut state, &mut status.warnings)?;
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
    let _guard = PLAYLIST_SYNC_LOCK
        .lock()
        .map_err(|_| "Playlist sync lock is unavailable")?;
    let dir = mirrors_dir(settings).ok_or("Music directory is not configured")?;
    let mut state = read_state(store)?;
    let (path, body) = if id == "favorites" {
        let path = dir.join("Liked Songs.m3u8");
        (
            path,
            render_named_m3u(
                "Liked Songs",
                &store.list_tracks("favorites", None)?,
                &settings.music_directory,
            ),
        )
    } else {
        let collection = store
            .list_collections()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or("Collection not found")?;
        let tracks = store.list_tracks("collection", Some(id))?;
        let path = dir
            .join(mirror_group(&state, id, &tracks))
            .join(format!("{id}.m3u8"));
        (
            path,
            render_named_m3u(&collection.name, &tracks, &settings.music_directory),
        )
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    atomic_write(&path, body.as_bytes())?;
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

/// Import a discovered source only once. Reopening its canonical path returns the original
/// collection binding, including after the source file has temporarily disappeared.
pub fn import_discovered_playlist(
    store: &Store,
    settings: &Settings,
    path: &str,
) -> Result<playlist_files::LocalPlaylistImport> {
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
    if !store
        .list_collections()?
        .iter()
        .any(|item| item.id == collection_id)
    {
        return Err("Collection not found".into());
    }
    sync_playlists(store, settings)?;
    let collection = store
        .list_collections()?
        .into_iter()
        .find(|item| item.id == collection_id)
        .ok_or("Collection not found")?;
    let tracks = current_collection_tracks(store, collection_id)?;
    let group = mirror_group(&read_state(store)?, collection_id, &tracks);
    let mirrors = mirrors_dir(settings).ok_or("Choose a music directory first")?;
    let mirror_path = mirrors.join(group).join(format!("{}.m3u8", collection.id));
    let body = fs::read(&mirror_path).map_err(|e| format!("Cannot read playlist mirror: {e}"))?;
    let path = write_named_export(
        &mirrors.join("Exports"),
        &collection.id,
        &collection.name,
        &body,
    )?;
    Ok(path.to_string_lossy().into_owned())
}

/// Export available local files to a deterministic Mixxx-compatible M3U8 location.
/// Remote and unavailable entries are reported and never emitted as stream URLs.
pub fn export_mixxx_playlist(
    store: &Store,
    settings: &Settings,
    collection_id: &str,
) -> Result<MixxxPlaylistExport> {
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
    store
        .playlist_sync_state()?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())
        .map(|s| s.unwrap_or_default())
}
fn persist_state(store: &Store, state: &SyncState) -> Result<()> {
    store.save_playlist_sync_state(&serde_json::to_value(state).map_err(|e| e.to_string())?)
}
fn mirrors_dir(settings: &Settings) -> Option<PathBuf> {
    if settings.music_directory.trim().is_empty() {
        return None;
    }
    Some(Path::new(&settings.music_directory).join("Playlists"))
}
fn write_mirrors(
    store: &Store,
    settings: &Settings,
    state: &mut SyncState,
    warnings: &mut Vec<String>,
) -> Result<Option<String>> {
    let Some(dir) = mirrors_dir(settings) else {
        return Ok(None);
    };
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create playlist mirror directory: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create collection mirrors: {e}"))?;
    let mirror_started = std::time::Instant::now();
    let collections = store.list_collections()?;
    if std::env::var_os("BURRATA_PERF_TRACE").is_some() {
        eprintln!(
            "[perf] playlist_sync.mirror_collections count={}",
            collections.len()
        );
    }
    for collection in collections {
        let browser_snapshots = store.browser_snapshots_for_collection(&collection.id)?;
        if !browser_snapshots.is_empty() {
            for snapshot in browser_snapshots.iter().rev() {
                let group = collection_group(&snapshot.tracks);
                archive_chart_snapshot(&dir, &group, &collection.id, snapshot)?;
            }
        }
        let tracks =
            current_collection_tracks_from_snapshots(store, &collection.id, &browser_snapshots)?;
        let original_group = mirror_group(state, &collection.id, &tracks);
        let group_dir = dir.join(&original_group);
        fs::create_dir_all(&group_dir)
            .map_err(|e| format!("Cannot create {original_group} mirrors: {e}"))?;
        let expected_path = group_dir.join(format!("{}.m3u8", collection.id));
        let prior_path = state.mirror_paths.get(&collection.id).map(PathBuf::from);
        let original_path = prior_path.as_deref().unwrap_or(&expected_path);
        if prior_path.as_deref().is_some_and(|prior| !prior.exists())
            && prior_path
                .as_deref()
                .is_some_and(|prior| prior.parent() == expected_path.parent())
        {
            state.paused_mirrors.insert(collection.id.clone());
            state.preserve_mirrors.insert(collection.id.clone());
            warnings.push(format!(
                "{}: mirror was deleted; synchronization is paused until explicitly resumed",
                original_path.display()
            ));
            continue;
        }
        let edit = apply_mirror_edit(
            store,
            state,
            &collection.id,
            original_path,
            &dir,
            &tracks,
            warnings,
        )?;
        if state.preserve_mirrors.contains(&collection.id) {
            continue;
        }
        let target_id = edit
            .personal_collection_id
            .clone()
            .unwrap_or_else(|| collection.id.clone());
        let target_tracks = if target_id == collection.id {
            if edit.original_collection_changed {
                current_collection_tracks(store, &collection.id)?
            } else {
                tracks
            }
        } else {
            store.list_tracks("collection", Some(&target_id))?
        };
        let target_name = store
            .list_collections()?
            .into_iter()
            .find(|item| item.id == target_id)
            .map(|item| item.name)
            .unwrap_or_else(|| collection.name.clone());
        let target_body = render_named_m3u(&target_name, &target_tracks, &settings.music_directory);
        let target_group = mirror_group(state, &target_id, &target_tracks);
        let target_dir = dir.join(&target_group);
        fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Cannot create {target_group} mirrors: {e}"))?;
        let target_path = target_dir.join(format!("{}.m3u8", target_id));
        if let Ok(previous) = fs::read_to_string(&target_path) {
            if previous != target_body {
                archive_mirror(state, &dir, &target_group, &target_id, &previous)?;
            }
        }
        if fs::read_to_string(&target_path).ok().as_deref() != Some(&target_body) {
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
        state
            .mirror_paths
            .insert(target_id, target_path.to_string_lossy().into_owned());
    }
    let likes_path = dir.join("Liked Songs.m3u8");
    let favorite_body = render_named_m3u(
        "Liked Songs",
        &store.list_tracks("favorites", None)?,
        &settings.music_directory,
    );
    if state.mirror_hashes.contains_key("favorites") && !likes_path.exists() {
        state.paused_mirrors.insert("favorites".into());
        state.preserve_mirrors.insert("favorites".into());
        warnings.push(
            "Liked Songs mirror was deleted; synchronization is paused until explicitly resumed"
                .into(),
        );
    }
    if let Some(contents) = read_external_edit(
        &state.mirror_hashes,
        "favorites",
        &likes_path,
        &favorite_body,
    ) {
        let (tracks, unresolved) =
            resolve_mirror_tracks(store, &contents, likes_path.parent().unwrap_or(&dir))?;
        if unresolved == 0 {
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
        } else {
            warnings.push(format!(
                "Liked Songs mirror has {unresolved} entries that could not be resolved"
            ));
            state.preserve_mirrors.insert("favorites".into());
        }
    }
    if state.preserve_mirrors.contains("favorites") {
        return Ok(Some(dir.to_string_lossy().into_owned()));
    }
    let favorites = store.list_tracks("favorites", None)?;
    let body = render_named_m3u("Liked Songs", &favorites, &settings.music_directory);
    if fs::read_to_string(&likes_path).ok().as_deref() != Some(&body) {
        if let Ok(previous) = fs::read_to_string(&likes_path) {
            archive_mirror(state, &dir, "Favorites", "favorites", &previous)?;
        }
        atomic_write(&likes_path, body.as_bytes())?;
    }
    state
        .mirror_hashes
        .insert("favorites".into(), digest(body.as_bytes()));
    state.mirror_paths.insert(
        "favorites".into(),
        likes_path.to_string_lossy().into_owned(),
    );
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
    if state
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

fn archive_mirror(
    state: &mut SyncState,
    root: &Path,
    group: &str,
    id: &str,
    contents: &str,
) -> Result<()> {
    let revision = digest(contents.as_bytes());
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
    let generated = render_named_m3u(&collection.name, current_tracks, "");
    let Some(contents) = read_external_edit(&state.mirror_hashes, collection_id, path, &generated)
    else {
        return Ok(MirrorEditOutcome::default());
    };
    let group = collection_group(current_tracks);
    archive_mirror(state, mirror_root, &group, collection_id, &generated)?;
    archive_mirror(state, mirror_root, &group, collection_id, &contents)?;
    let (tracks, unresolved) =
        resolve_mirror_tracks(store, &contents, path.parent().unwrap_or(mirror_root))?;
    if unresolved > 0 {
        warnings.push(format!(
            "{}: mirror edit has {unresolved} unresolved entries; changes were kept for review",
            path.display()
        ));
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
        let conflict_name = format!(
            "{} (conflict {})",
            collection.name,
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        );
        let conflict = store.create_collection(&conflict_name)?;
        store.replace_playlist_entries(&conflict.id, &tracks)?;
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
    store.replace_playlist_entries(&target_id, &tracks)?;
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

fn resolve_mirror_tracks(
    store: &Store,
    text: &str,
    mirror_root: &Path,
) -> Result<(Vec<Track>, usize)> {
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
    Ok((tracks, unresolved))
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
}
