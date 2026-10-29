//! Typed projection of durable user state for full backups.
//!
//! An archive database is data, never executable schema. We read only fixed
//! allowlisted tables/columns with `trusted_schema=OFF`, and copy rows into a
//! fresh database created by Burrata's own `Store::open` migrations.

use crate::{models::Settings, storage::Store};
use rusqlite::{
    backup::Backup, params, params_from_iter, types::Value as SqlValue, Connection, OpenFlags,
    OptionalExtension,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use uuid::Uuid;

const SETTINGS_SCHEMA_VERSION: u32 = 1;
const PLAYBACK_SCHEMA_VERSION: u32 = 1;
pub const BACKUP_PLAYBACK_HANDOFF_KEY: &str = "backupPlaybackHydration.v1";
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_HISTORY_ROWS: usize = 2_000_000;
const MAX_LYRIC_CHOICE_BODY_BYTES: usize = 64 * 1024;
const MAX_LYRIC_CHOICE_LINES: usize = 800;
// `programme:` plus 200 supported layout labels (the longest is
// `5.1(side)`) and 199 commas, matching LoudnessResourceIdentity's member cap.
const MAX_LOUDNESS_PROGRAM_LAYOUT_BYTES: usize = 10 + 200 * 9 + 199;
const MAX_ROWS_PER_TABLE: usize = 2_000_000;
const ROOT_TOKEN_PREFIX: &str = "@burrata-root/";
const MISSING_ASSET_PREFIX: &str = "burrata-missing://";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStateOptions {
    /// Optional queue/session captured from the WebView before entering native code.
    /// Native checkpoints are used when this is absent.
    #[serde(default)]
    pub webview_playback: Option<Value>,
    /// Parsed values for the exact allowlisted WebView localStorage keys below.
    /// The backup command must pass these; native code cannot read WebView storage.
    #[serde(default)]
    pub frontend_preferences: Option<Value>,
}

#[derive(Debug)]
pub struct PreparedBackup {
    pub directory: TempDir,
    pub library_sqlite: PathBuf,
    pub history_sqlite: PathBuf,
    pub settings_json: PathBuf,
    pub playback_json: PathBuf,
    pub snapshot_id: Uuid,
    pub summary: BackupStateSummary,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStateSummary {
    pub recordings: usize,
    pub catalog_listings: usize,
    pub collections: usize,
    pub playlist_occurrences: usize,
    pub assets: usize,
    pub history_events: usize,
    pub omitted_cache_rows: usize,
    pub omitted_jobs: usize,
    pub omitted_private_settings: usize,
    pub path_hints: usize,
    pub omitted_stale_analyses: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsArchive {
    pub format: String,
    pub schema_version: u32,
    pub snapshot_id: String,
    pub created_at: String,
    pub application: Value,
    pub preferences: BTreeMap<String, Value>,
    #[serde(default)]
    pub frontend_preferences: BTreeMap<String, Value>,
    #[serde(default)]
    pub asset_path_hints: Vec<AssetPathHint>,
    #[serde(default)]
    pub projection_summary: SettingsProjectionSummary,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsProjectionSummary {
    pub omitted_cache_rows: usize,
    pub omitted_jobs: usize,
    pub omitted_private_settings: usize,
    pub excluded_application_fields: usize,
    pub asset_path_hints: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetPathHint {
    pub asset_id: String,
    pub root_key: String,
    pub relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackArchive {
    pub format: String,
    pub schema_version: u32,
    pub snapshot_id: String,
    pub queue: Vec<Value>,
    pub entry_ids: Vec<String>,
    /// Album programme identity bound to specific queue occurrences. Values
    /// are revalidated as typed LoudnessResourceIdentity records on restore.
    #[serde(default)]
    pub queue_loudness_contexts: BTreeMap<String, Value>,
    pub current_index: i64,
    pub current_entry_id: Option<String>,
    pub queue_revision: u64,
    pub position_seconds: f64,
    pub repeat: String,
    pub shuffle: bool,
    pub volume: f64,
    pub saved_at: Option<String>,
    pub restore_paused: bool,
}

/// Convert the selected playback component already materialized in the staged
/// database into a private, one-time frontend handoff. This deliberately reads
/// staged queue/session rows rather than the original archive paths.
pub fn make_backup_playback_handoff(queue: &Value, session: &Value, nonce: &str) -> Result<Value> {
    Uuid::parse_str(nonce).map_err(|_| "Invalid playback handoff nonce".to_owned())?;
    let tracks = queue["tracks"]
        .as_array()
        .ok_or_else(|| "Restored playback queue is missing".to_owned())?;
    let _: Vec<crate::models::Track> =
        serde_json::from_value(Value::Array(tracks.clone())).map_err(err)?;
    let entry_ids = queue["entryIds"]
        .as_array()
        .ok_or_else(|| "Restored playback occurrence IDs are missing".to_owned())?;
    if entry_ids.len() != tracks.len() {
        return Err("Restored playback occurrence IDs are invalid".to_owned());
    }
    let mut seen_entry_ids = HashSet::with_capacity(entry_ids.len());
    for entry_id in entry_ids {
        let Some(entry_id) = entry_id.as_str() else {
            return Err("Restored playback occurrence IDs are invalid".to_owned());
        };
        if entry_id.is_empty()
            || entry_id.len() > 128
            || entry_id.chars().any(char::is_control)
            || !seen_entry_ids.insert(entry_id)
        {
            return Err("Restored playback occurrence IDs are invalid".to_owned());
        }
    }
    let index = session["index"]
        .as_i64()
        .ok_or_else(|| "Restored playback index is invalid".to_owned())?;
    let queue_index = if tracks.is_empty() {
        if index != 0 && index != -1 {
            return Err("Restored empty playback index is invalid".to_owned());
        }
        0
    } else if index >= 0 && (index as usize) < tracks.len() {
        index as usize
    } else {
        return Err("Restored playback index is outside the queue".to_owned());
    };
    let position = session["position"]
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.)
        .ok_or_else(|| "Restored playback position is invalid".to_owned())?;
    let volume = session["volume"]
        .as_f64()
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .ok_or_else(|| "Restored playback volume is invalid".to_owned())?;
    let repeat = session["repeat"]
        .as_str()
        .filter(|value| matches!(*value, "off" | "all" | "one"))
        .ok_or_else(|| "Restored playback repeat mode is invalid".to_owned())?;
    let shuffle = session["shuffle"]
        .as_bool()
        .ok_or_else(|| "Restored playback shuffle state is invalid".to_owned())?;
    let queue_snapshot = serde_json::json!({
        "tracks": tracks,
        "entryIds": entry_ids,
        "currentIndex": queue_index,
        "repeat": repeat,
        "queueLoudnessContexts": queue.get("queueLoudnessContexts").cloned().unwrap_or_else(|| serde_json::json!({})),
    });
    Ok(serde_json::json!({
        "version": 1,
        "nonce": nonce,
        "queue": queue_snapshot,
        "position": position,
        "volume": volume,
        "repeat": repeat,
        "shuffle": shuffle,
        "paused": true,
    }))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathMapping {
    pub root_key: String,
    pub destination: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreStateOptions {
    pub mode: RestoreMode,
    pub snapshot_id: String,
    pub path_mappings: Vec<PathMapping>,
    /// Ephemeral bindings from the live, lock-consistent current profile. These
    /// device-local paths are never serialized into an archive.
    #[serde(skip)]
    pub current_asset_paths: Vec<CurrentAssetPath>,
    /// The current device's configured music root, also ephemeral. It may
    /// resolve a previously tokenized local binding only after hash checking.
    #[serde(skip)]
    pub current_music_root: Option<PathBuf>,
    /// Ephemeral current-device playback snapshot used by Merge when the
    /// archive's playback component is not selected. This preserves the
    /// destination queue without importing the archive queue.
    #[serde(skip)]
    pub current_playback_snapshot: Option<PathBuf>,
    pub include_library: bool,
    pub include_history: bool,
    pub include_settings: bool,
    pub include_playback: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentAssetPath {
    pub asset_id: String,
    pub path: String,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RestoreMode {
    #[default]
    Merge,
    Replace,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreStateSummary {
    pub already_applied: bool,
    pub imported_recordings: usize,
    pub imported_catalog_listings: usize,
    pub imported_collections: usize,
    pub imported_playlist_occurrences: usize,
    pub imported_assets: usize,
    pub imported_history_events: usize,
    pub skipped_conflicts: usize,
    pub unavailable_assets: usize,
    pub omitted_stale_analyses: usize,
    pub device_settings_skipped: usize,
    pub private_settings_omitted: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct TableSpec {
    name: &'static str,
    columns: &'static [&'static str],
}

// Never take identifiers or executable SQL from the archive. Column lists are
// explicit and are intersected with the source and current target schemas.
const LIBRARY_TABLES: &[TableSpec] = &[
    TableSpec {
        name: "works",
        columns: &["id", "title", "artist"],
    },
    TableSpec {
        name: "recordings",
        columns: &[
            "id",
            "work_id",
            "version",
            "favorite",
            "disposition",
            "manual_metadata",
        ],
    },
    TableSpec {
        name: "catalog_listings",
        columns: &[
            "id",
            "provider",
            "provider_id",
            "recording_id",
            "source_url",
            "metadata",
            "updated_at",
            "in_library",
        ],
    },
    TableSpec {
        name: "chart_snapshots",
        columns: &["id", "provider", "source_url", "fetched_at", "payload"],
    },
    TableSpec {
        name: "chart_entries",
        columns: &["snapshot_id", "listing_id", "rank"],
    },
    TableSpec {
        name: "browser_sources",
        columns: &[
            "source_url",
            "provider",
            "kind",
            "collection_id",
            "latest_snapshot_id",
        ],
    },
    TableSpec {
        name: "browser_imports",
        columns: &["snapshot_id", "payload"],
    },
    TableSpec {
        name: "collections",
        columns: &["id", "name", "deleted_at"],
    },
    TableSpec {
        name: "collection_members",
        columns: &["collection_id", "recording_id", "position"],
    },
    TableSpec {
        name: "playlist_ordered_collections",
        columns: &["collection_id"],
    },
    TableSpec {
        name: "playlist_ordered_entries",
        // occurrence_id is part of Store schema v11. Keeping it here preserves
        // stable occurrence annotations in playlistHierarchy.v1 during restore.
        columns: &["collection_id", "position", "listing_id", "occurrence_id"],
    },
    TableSpec {
        name: "local_assets",
        columns: &["id", "recording_id", "path", "hash", "payload"],
    },
    TableSpec {
        name: "asset_analyses",
        columns: &[
            "asset_id",
            "file_hash",
            "version",
            "status",
            "payload",
            "updated_at",
        ],
    },
    TableSpec {
        name: "asset_autotags",
        columns: &[
            "asset_id",
            "file_hash",
            "version",
            "status",
            "payload",
            "updated_at",
        ],
    },
    TableSpec {
        name: "analysis_cache_records",
        columns: &[
            "namespace",
            "cache_key",
            "payload",
            "payload_bytes",
            "updated_at",
            "last_used_at",
            "protected",
        ],
    },
    TableSpec {
        name: "analysis_cache_record_refs",
        columns: &["namespace", "cache_key", "recording_id"],
    },
];

// Durable, user-authored settings. Unknown settings are excluded by design.
// `connections`, session-only markers, OAuth/account state, remote caches,
// automation permissions and task state must never be copied.
const PREFERENCE_KEYS: &[&str] = &[
    "soulseekDownloadWatches.v1",
    "playlistPreferences.v1",
    "playlistMirrorOptions.v1",
    "playlistHierarchy.v1",
    "playlistSync.v1",
    "sidebarLayout.v1",
    "radioStations.v1",
    "hiddenRadioStations.v1",
    "radioStationNames.v1",
    "radioHistory.v1",
    "includeRadioSongsInPlaybackHistory.v1",
    "autotag.settings",
    "radioRecognitionEnabled.v1",
    "loudness.v1",
    "streamAnalysisCache.v1",
    "playingTrackDetectionCache.v1",
    "playingTrackDetectionPolicy.v1",
    "playlistHistoryMirror.v1",
    "backupSchedule.v1",
];

const PORTABLE_APPLICATION_FIELDS: &[&str] = &[
    "downloadLayout",
    "playlistMirroringEnabled",
    "compactPlaybackBar",
    "autoPreview",
    "liveBpmEnabled",
    "matchingLevel",
    "pauseOnSourceOpen",
    "sourceOpenAtPosition",
    "previewDelayMs",
    "playbackStartupTimeoutSeconds",
    "theme",
    "preferredDownloadSource",
    "preferredStreamingSource",
    "overridePlaylistTrackUrls",
    "streamingFallbackEnabled",
    "streamingFallbackDelayMs",
    "streamingFallbackSources",
    "streamingFallbackPolicyVersion",
    "bandcampFallbackEnabled",
    "backgroundSourcePreparation",
    "downloadFallbackEnabled",
    "downloadFallbackSources",
    "downloadFallbackAllowLossy",
    "soulseekWaitForSource",
    "soulseekLosslessUpgrades",
    "embedLyricsOnDownload",
];

const FRONTEND_PREFERENCE_KEYS: &[&str] = &[
    "burrata.discovery.search-scope.v2",
    "burrata.discovery.sources.v1",
    "burrata.discovery.sources.v1",
    "burrata.discovery.continue-search.v1",
    "burrata.discovery.search-history.v1",
    "burrata.soulseek.wishlist.v1",
    "burrata.playlist.include-ytdlp-search.v1",
    "burrata.queue-end-radio.v1",
    "burrata.waveform-enabled.v1",
    "burrata.stream-media-analysis-enabled.v1",
    "burrata.layout.sidebar.v1",
    "burrata.layout.info.v1",
    "burrata.lyrics-timing.v1",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportPreview {
    pub preview_id: String,
    pub schema_version: u32,
    pub archive_bytes: u64,
    pub application_fields: Vec<String>,
    pub preference_keys: Vec<String>,
    pub frontend_preference_keys: Vec<String>,
    pub excluded_application_fields: usize,
    pub excluded_preference_keys: usize,
    pub excluded_frontend_preference_keys: usize,
    pub excluded_asset_path_hints: usize,
    pub omitted_cache_rows: usize,
    pub omitted_jobs: usize,
    pub omitted_private_settings: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportSelection {
    pub application: bool,
    pub preference_keys: Vec<String>,
    pub frontend_preference_keys: Vec<String>,
}

/// A settings import is projected into typed allowlisted values. Callers own
/// the transaction and apply these values through Burrata's Store APIs.
#[derive(Debug, Clone, Default)]
pub struct SettingsImportProjection {
    pub application_fields: BTreeMap<String, Value>,
    pub preferences: BTreeMap<String, Value>,
    pub frontend_preferences: BTreeMap<String, Value>,
    pub application_settings: Option<Settings>,
    pub summary: SettingsImportProjectionSummary,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportProjectionSummary {
    pub application_fields: usize,
    pub preferences: usize,
    pub frontend_preferences: usize,
}

type Result<T> = std::result::Result<T, String>;

/// Capture consistent SQLite online-backup snapshots, then sanitize only the
/// private staged copies. `snapshot_id` must match the archive manifest ID.
pub fn prepare_backup(
    data_dir: &Path,
    snapshot_id: Uuid,
    options: BackupStateOptions,
) -> Result<PreparedBackup> {
    let directory = tempfile::tempdir().map_err(err)?;
    let library_sqlite = directory.path().join("library.sqlite");
    let history_sqlite = directory.path().join("playback-history.sqlite");
    online_backup(&data_dir.join("burrata.sqlite"), &library_sqlite)?;
    online_backup(&data_dir.join("playback-history.sqlite"), &history_sqlite)?;
    finish_prepared_backup(
        directory,
        library_sqlite,
        history_sqlite,
        snapshot_id,
        options,
    )
}

/// Project online snapshots produced by a caller that already owns the store
/// checkpoint/coordinator. The input files are never modified; they are copied
/// only after they are complete staged SQLite snapshots.
pub fn prepare_from_snapshots(
    library_snapshot: &Path,
    history_snapshot: &Path,
    snapshot_id: Uuid,
    options: BackupStateOptions,
) -> Result<PreparedBackup> {
    let directory = tempfile::tempdir().map_err(err)?;
    prepare_from_snapshots_in_directory(
        library_snapshot,
        history_snapshot,
        snapshot_id,
        options,
        directory,
    )
}

/// Profile-staged variant used by the application coordinator. `scratch_parent`
/// must be an app-owned staging directory whose guard outlives the returned
/// prepared backup.
pub fn prepare_from_snapshots_in(
    library_snapshot: &Path,
    history_snapshot: &Path,
    snapshot_id: Uuid,
    options: BackupStateOptions,
    scratch_parent: &Path,
) -> Result<PreparedBackup> {
    let directory = tempfile::tempdir_in(scratch_parent).map_err(err)?;
    prepare_from_snapshots_in_directory(
        library_snapshot,
        history_snapshot,
        snapshot_id,
        options,
        directory,
    )
}

fn prepare_from_snapshots_in_directory(
    library_snapshot: &Path,
    history_snapshot: &Path,
    snapshot_id: Uuid,
    options: BackupStateOptions,
    directory: TempDir,
) -> Result<PreparedBackup> {
    let library_sqlite = directory.path().join("library.sqlite");
    let history_sqlite = directory.path().join("playback-history.sqlite");
    std::fs::copy(library_snapshot, &library_sqlite).map_err(err)?;
    std::fs::copy(history_snapshot, &history_sqlite).map_err(err)?;
    finish_prepared_backup(
        directory,
        library_sqlite,
        history_sqlite,
        snapshot_id,
        options,
    )
}

fn finish_prepared_backup(
    directory: TempDir,
    library_sqlite: PathBuf,
    history_sqlite: PathBuf,
    snapshot_id: Uuid,
    options: BackupStateOptions,
) -> Result<PreparedBackup> {
    let settings_json = directory.path().join("settings.json");
    let playback_json = directory.path().join("playback.json");

    let playback = build_playback_archive(&library_sqlite, snapshot_id, options.webview_playback)?;
    write_json(&playback_json, &playback)?;
    let mut summary = sanitize_library_snapshot(&library_sqlite, snapshot_id)?;
    sanitize_history_snapshot(&history_sqlite, &mut summary)?;
    let settings = build_settings_archive(
        &library_sqlite,
        snapshot_id,
        options.frontend_preferences,
        &mut summary,
    )?;
    write_json(&settings_json, &settings)?;
    Ok(PreparedBackup {
        directory,
        library_sqlite,
        history_sqlite,
        settings_json,
        playback_json,
        snapshot_id,
        summary,
    })
}

/// Reconstruct selected components into an isolated staged profile. The target
/// DBs are opened/migrated by Burrata's own schema; no archive DDL is executed.
/// Callers must point `target_data_dir` at a staging profile, never the live one.
pub fn reconstruct_staged(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    target_data_dir: &Path,
    options: &RestoreStateOptions,
) -> Result<RestoreStateSummary> {
    Uuid::parse_str(&options.snapshot_id).map_err(|_| "Backup snapshot identity is invalid")?;
    std::fs::create_dir_all(target_data_dir).map_err(err)?;
    let library_target = target_data_dir.join("burrata.sqlite");
    let history_target = target_data_dir.join("playback-history.sqlite");
    if options.mode == RestoreMode::Replace {
        if library_target.exists() || history_target.exists() {
            return Err("Replace restore requires a fresh isolated staging directory".into());
        }
        let fresh_library = library_target.clone();
        let fresh_history = history_target.clone();
        initialize_trusted_library(&fresh_library)?;
        initialize_trusted_history(&fresh_history)?;
        let mut summary = RestoreStateSummary::default();
        apply_components(
            archive_library,
            archive_history,
            archive_settings,
            archive_playback,
            &fresh_library,
            &fresh_history,
            options,
            &mut summary,
        )?;
        Ok(summary)
    } else {
        initialize_trusted_library(&library_target)?;
        initialize_trusted_history(&history_target)?;
        let mut summary = RestoreStateSummary::default();
        apply_components(
            archive_library,
            archive_history,
            archive_settings,
            archive_playback,
            &library_target,
            &history_target,
            options,
            &mut summary,
        )?;
        Ok(summary)
    }
}

/// Build a disposable merge/replace preview and return exact component counts
/// without writing to the caller's profile.
pub fn preview_restore(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    current_data_dir: Option<&Path>,
    options: &RestoreStateOptions,
) -> Result<RestoreStateSummary> {
    let staging = tempfile::tempdir().map_err(err)?;
    preview_restore_in_directory(
        archive_library,
        archive_history,
        archive_settings,
        archive_playback,
        current_data_dir,
        options,
        staging,
    )
}

/// Profile-staged variant used by the application coordinator. The caller
/// holds the app-owned stage guard and profile lock for this operation.
pub fn preview_restore_in(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    current_data_dir: Option<&Path>,
    options: &RestoreStateOptions,
    scratch_parent: &Path,
) -> Result<RestoreStateSummary> {
    let staging = tempfile::tempdir_in(scratch_parent).map_err(err)?;
    preview_restore_in_directory(
        archive_library,
        archive_history,
        archive_settings,
        archive_playback,
        current_data_dir,
        options,
        staging,
    )
}

fn preview_restore_in_directory(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    current_data_dir: Option<&Path>,
    options: &RestoreStateOptions,
    staging: TempDir,
) -> Result<RestoreStateSummary> {
    if options.mode == RestoreMode::Merge {
        if let Some(current) = current_data_dir {
            for filename in ["burrata.sqlite", "playback-history.sqlite"] {
                let source_path = current.join(filename);
                if source_path.is_file() {
                    online_backup(&source_path, &staging.path().join(filename))?;
                }
            }
        }
    }
    reconstruct_staged(
        archive_library,
        archive_history,
        archive_settings,
        archive_playback,
        staging.path(),
        options,
    )
}

/// Compute the restore summary for the component/mode selection currently
/// shown to the user. The supplied paths must be validated staged components.
pub fn preview_restore_selection(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    current_data_dir: Option<&Path>,
    snapshot_id: &str,
    mode: RestoreMode,
    component_ids: &[String],
    path_mappings: Vec<PathMapping>,
) -> Result<RestoreStateSummary> {
    preview_restore_selection_inner(
        archive_library,
        archive_history,
        archive_settings,
        archive_playback,
        current_data_dir,
        snapshot_id,
        mode,
        component_ids,
        path_mappings,
        None,
    )
}

/// Profile-staged counterpart to [`preview_restore_selection`].
pub fn preview_restore_selection_in(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    current_data_dir: Option<&Path>,
    snapshot_id: &str,
    mode: RestoreMode,
    component_ids: &[String],
    path_mappings: Vec<PathMapping>,
    scratch_parent: &Path,
) -> Result<RestoreStateSummary> {
    preview_restore_selection_inner(
        archive_library,
        archive_history,
        archive_settings,
        archive_playback,
        current_data_dir,
        snapshot_id,
        mode,
        component_ids,
        path_mappings,
        Some(scratch_parent),
    )
}

#[allow(clippy::too_many_arguments)]
fn preview_restore_selection_inner(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    current_data_dir: Option<&Path>,
    snapshot_id: &str,
    mode: RestoreMode,
    component_ids: &[String],
    path_mappings: Vec<PathMapping>,
    scratch_parent: Option<&Path>,
) -> Result<RestoreStateSummary> {
    let selected = |id: &str| component_ids.iter().any(|value| value == id);
    let options = RestoreStateOptions {
        mode,
        snapshot_id: snapshot_id.into(),
        path_mappings,
        current_asset_paths: Vec::new(),
        current_music_root: None,
        current_playback_snapshot: None,
        include_library: selected("library"),
        include_history: selected("history"),
        include_settings: selected("settings"),
        include_playback: selected("playback"),
    };
    let archive_library = archive_library.filter(|_| options.include_library);
    let archive_history = archive_history.filter(|_| options.include_history);
    let archive_settings = archive_settings.filter(|_| options.include_settings);
    let archive_playback = archive_playback.filter(|_| options.include_playback);
    match scratch_parent {
        Some(scratch_parent) => preview_restore_in(
            archive_library,
            archive_history,
            archive_settings,
            archive_playback,
            current_data_dir,
            &options,
            scratch_parent,
        ),
        None => preview_restore(
            archive_library,
            archive_history,
            archive_settings,
            archive_playback,
            current_data_dir,
            &options,
        ),
    }
}

/// Parse the separately selected WebView settings component after archive
/// inspection. The frontend bridge may apply these allowlisted values to localStorage.
pub fn read_frontend_preferences(path: &Path) -> Result<BTreeMap<String, Value>> {
    let archive: SettingsArchive = read_json(path)?;
    if archive.format != "org.burrata.settings" || archive.schema_version != SETTINGS_SCHEMA_VERSION
    {
        return Err("Unsupported settings archive schema".into());
    }
    Ok(sanitize_frontend_preferences(&Value::Object(
        archive.frontend_preferences.into_iter().collect(),
    )))
}

/// Inspect an independent settings archive without applying any of its data.
/// Unsupported fields, device paths, and path hints are counted for preview.
pub fn inspect_settings_archive(path: &Path) -> Result<SettingsImportPreview> {
    let archive: SettingsArchive = read_json(path)?;
    validate_settings_archive(&archive)?;
    let safe_application = portable_application_fields(&archive.application);
    let safe_preference_keys = archive
        .preferences
        .iter()
        .filter_map(|(key, value)| {
            (is_preference_allowlisted(key) && sanitize_preference(key, value.clone()).is_some())
                .then(|| key.clone())
        })
        .collect::<Vec<_>>();
    let safe_frontend_preferences = sanitize_frontend_preferences(&Value::Object(
        archive.frontend_preferences.clone().into_iter().collect(),
    ));
    let mut warnings = Vec::new();
    let excluded_asset_path_hints = archive
        .asset_path_hints
        .len()
        .max(archive.projection_summary.asset_path_hints);
    if excluded_asset_path_hints > 0 {
        warnings.push(format!(
            "{} local asset path hints are excluded from settings import.",
            excluded_asset_path_hints
        ));
    }
    if archive.projection_summary.omitted_private_settings > 0 {
        warnings.push(format!(
            "{} private settings were excluded when this archive was created.",
            archive.projection_summary.omitted_private_settings
        ));
    }
    let excluded_application_fields = archive
        .application
        .as_object()
        .map(|fields| fields.len().saturating_sub(safe_application.len()))
        .unwrap_or(0)
        .max(archive.projection_summary.excluded_application_fields);
    let excluded_preference_keys = archive
        .preferences
        .len()
        .saturating_sub(safe_preference_keys.len());
    let excluded_frontend_preference_keys = archive
        .frontend_preferences
        .len()
        .saturating_sub(safe_frontend_preferences.len());
    if excluded_application_fields + excluded_preference_keys + excluded_frontend_preference_keys
        > 0
    {
        warnings.push("Unsupported, invalid, or device-specific settings will be skipped.".into());
    }
    Ok(SettingsImportPreview {
        preview_id: Uuid::new_v4().to_string(),
        schema_version: archive.schema_version,
        archive_bytes: std::fs::metadata(path).map_err(err)?.len(),
        application_fields: safe_application.keys().cloned().collect(),
        preference_keys: safe_preference_keys,
        frontend_preference_keys: safe_frontend_preferences.keys().cloned().collect(),
        excluded_application_fields,
        excluded_preference_keys,
        excluded_frontend_preference_keys: archive
            .frontend_preferences
            .len()
            .saturating_sub(safe_frontend_preferences.len()),
        excluded_asset_path_hints,
        omitted_cache_rows: archive.projection_summary.omitted_cache_rows,
        omitted_jobs: archive.projection_summary.omitted_jobs,
        omitted_private_settings: archive.projection_summary.omitted_private_settings,
        warnings,
    })
}

/// Return selected settings as already-sanitized values. Device-bound fields
/// and path hints never enter the projection, even if named in the input JSON.
pub fn project_settings_archive(
    path: &Path,
    selection: &SettingsImportSelection,
) -> Result<SettingsImportProjection> {
    let archive: SettingsArchive = read_json(path)?;
    validate_settings_archive(&archive)?;
    let mut projection = SettingsImportProjection::default();
    if selection.application {
        projection.application_fields = portable_application_fields(&archive.application);
        if !projection.application_fields.is_empty() {
            projection.application_settings = Some(application_from_archive(&archive.application)?);
        }
    }
    for key in &selection.preference_keys {
        if !is_preference_allowlisted(key) {
            continue;
        }
        if let Some(value) = archive
            .preferences
            .get(key)
            .cloned()
            .and_then(|value| sanitize_preference(key, value))
        {
            projection.preferences.insert(key.clone(), value);
        }
    }
    for key in &selection.frontend_preference_keys {
        if !FRONTEND_PREFERENCE_KEYS.contains(&key.as_str()) {
            continue;
        }
        if let Some(value) = archive
            .frontend_preferences
            .get(key)
            .cloned()
            .and_then(|value| sanitize_frontend_preference(key, value))
        {
            projection.frontend_preferences.insert(key.clone(), value);
        }
    }
    projection.summary = SettingsImportProjectionSummary {
        application_fields: projection.application_fields.len(),
        preferences: projection.preferences.len(),
        frontend_preferences: projection.frontend_preferences.len(),
    };
    Ok(projection)
}

fn validate_settings_archive(archive: &SettingsArchive) -> Result<()> {
    if archive.format != "org.burrata.settings" || archive.schema_version != SETTINGS_SCHEMA_VERSION
    {
        return Err("Unsupported settings archive schema".into());
    }
    if archive.preferences.len() > PREFERENCE_KEYS.len() + 10_000
        || archive.frontend_preferences.len() > FRONTEND_PREFERENCE_KEYS.len()
        || archive.asset_path_hints.len() > MAX_ROWS_PER_TABLE
    {
        return Err("Settings archive exceeds entry limits".into());
    }
    Ok(())
}

fn portable_application_fields(value: &Value) -> BTreeMap<String, Value> {
    let Some(source) = value.as_object() else {
        return BTreeMap::new();
    };
    let mut fields: BTreeMap<String, Value> = PORTABLE_APPLICATION_FIELDS
        .iter()
        .filter_map(|key| {
            let value = source.get(*key)?;
            safe_setting_value(key, value).then(|| ((*key).to_string(), sanitize_json(value)))
        })
        .collect();
    fields
}

/// Revalidate the frontend-only settings envelope before sending it to the
/// WebView. This is also used for post-restart hydration from staged restore.
pub fn sanitize_frontend_preferences(value: &Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            sanitize_frontend_preference(key, value.clone()).map(|safe| (key.clone(), safe))
        })
        .collect()
}

fn apply_components(
    archive_library: Option<&Path>,
    archive_history: Option<&Path>,
    archive_settings: Option<&Path>,
    archive_playback: Option<&Path>,
    library_target: &Path,
    history_target: &Path,
    options: &RestoreStateOptions,
    summary: &mut RestoreStateSummary,
) -> Result<()> {
    if options.mode == RestoreMode::Merge {
        restore_current_target_asset_paths(library_target, options)?;
    }
    if snapshot_already_applied(library_target, &options.snapshot_id)? {
        summary.already_applied = true;
        return Ok(());
    }
    let mut transaction = Connection::open(library_target).map_err(err)?;
    transaction
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")
        .map_err(err)?;
    let tx = transaction.transaction().map_err(err)?;
    let mut occurrence_id_mapping = HashMap::new();
    let mut asset_id_mapping = HashMap::new();
    let mut listing_id_mapping = HashMap::new();
    if options.include_library {
        if let Some(source_path) = archive_library {
            let source = open_archive_db(source_path)?;
            merge_library(
                &source,
                &tx,
                options,
                summary,
                &mut occurrence_id_mapping,
                &mut asset_id_mapping,
                &mut listing_id_mapping,
            )?;
        }
    }
    if options.include_settings {
        if let Some(path) = archive_settings {
            merge_settings_component(&tx, path, options.mode, &occurrence_id_mapping, summary)?;
        }
    }
    if options.include_playback {
        if let Some(path) = archive_playback {
            merge_playback_component(&tx, path, summary, &asset_id_mapping, &listing_id_mapping)?;
        }
    } else if options.mode == RestoreMode::Merge {
        if let Some(path) = options.current_playback_snapshot.as_deref() {
            preserve_current_playback_component(&tx, path)?;
            summary.warnings.push(
                "The current playback queue was preserved and will return paused after Merge."
                    .into(),
            );
        }
    }
    tx.commit().map_err(err)?;

    if options.include_history {
        if let Some(source_path) = archive_history {
            merge_history(source_path, history_target, summary)?;
        }
    }
    let mut checkpoint = Connection::open(library_target).map_err(err)?;
    checkpoint
        .execute_batch("PRAGMA trusted_schema=OFF;")
        .map_err(err)?;
    let tx = checkpoint.transaction().map_err(err)?;
    mark_snapshot_applied(&tx, &options.snapshot_id)?;
    tx.commit().map_err(err)?;
    Ok(())
}

/// Read asset bindings from the private, lock-consistent SQLite snapshot used
/// to build the current-profile backup projection. These paths stay in memory
/// for the restore operation and are never written to the archive.
pub fn current_asset_paths_from_snapshot(path: &Path) -> Result<Vec<CurrentAssetPath>> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let mut statement = connection
        .prepare("SELECT id,path,hash FROM local_assets ORDER BY id")
        .map_err(err)?;
    let rows = statement
        .query_map([], |row| {
            Ok(CurrentAssetPath {
                asset_id: row.get(0)?,
                path: row.get(1)?,
                hash: row.get(2)?,
            })
        })
        .map_err(err)?;
    let collected = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
    Ok(collected)
}

pub fn current_music_root_from_snapshot(path: &Path) -> Result<Option<PathBuf>> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let application: Option<String> = connection
        .query_row(
            "SELECT value FROM settings WHERE key='application'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(err)?;
    let Some(application) = application else {
        return Ok(None);
    };
    let Ok(settings) = serde_json::from_str::<Settings>(&application) else {
        return Ok(None);
    };
    let root = PathBuf::from(settings.music_directory.trim());
    Ok((!settings.music_directory.trim().is_empty() && root.is_absolute()).then_some(root))
}

/// Rebuild local paths in the sanitized current-profile merge base before
/// incoming archive rows are considered. A same-ID asset conflict must not
/// strand an existing current asset at its privacy token. Every restored path
/// is verified against the stored SHA-256 before it becomes available.
fn restore_current_target_asset_paths(
    library_target: &Path,
    options: &RestoreStateOptions,
) -> Result<()> {
    let mut connection = Connection::open(library_target).map_err(err)?;
    connection
        .execute_batch("PRAGMA trusted_schema=OFF;")
        .map_err(err)?;
    let tx = connection.transaction().map_err(err)?;
    // These bindings come only from the lock-consistent snapshot of the live
    // target profile. Incoming archive path mappings belong to different
    // identities and are applied later by `merge_assets`.
    let live_bindings = options
        .current_asset_paths
        .iter()
        .map(|binding| (binding.asset_id.as_str(), binding))
        .collect::<HashMap<_, _>>();
    let local_music_mapping = options
        .current_music_root
        .as_deref()
        .map(|root| HashMap::from([("music", root)]));
    let rows = {
        let mut statement = tx
            .prepare("SELECT id,recording_id,path,hash,payload FROM local_assets ORDER BY id")
            .map_err(err)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(err)?;
        let collected = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
        collected
    };
    for (id, recording_id, stored_path, hash, raw_payload) in rows {
        if !valid_sha256(&hash) {
            continue;
        }
        let binding_path = live_bindings.get(id.as_str()).and_then(|binding| {
            (binding.hash.eq_ignore_ascii_case(&hash) && Path::new(&binding.path).is_absolute())
                .then(|| PathBuf::from(&binding.path))
                .filter(|path| file_matches_sha256(path, &hash))
        });
        let existing_path = Path::new(&stored_path);
        let verified_existing = (existing_path.is_absolute()
            && file_matches_sha256(existing_path, &hash))
        .then(|| existing_path.to_path_buf());
        let verified_local_music_path = local_music_mapping
            .as_ref()
            .and_then(|mapping| {
                let path = resolve_asset_path(&id, &stored_path, &hash, mapping);
                (!path.starts_with(MISSING_ASSET_PREFIX)).then_some(path)
            })
            .map(PathBuf::from);
        let resolved = binding_path
            .or(verified_existing)
            .or(verified_local_music_path)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{MISSING_ASSET_PREFIX}{id}"));
        let payload =
            materialize_restored_asset_payload(&raw_payload, &id, &recording_id, &hash, &resolved)?;
        tx.execute(
            "UPDATE local_assets SET path=?1,payload=?2 WHERE id=?3 AND hash=?4",
            params![
                resolved,
                serde_json::to_string(&payload).map_err(err)?,
                id,
                hash
            ],
        )
        .map_err(err)?;
    }
    tx.commit().map_err(err)
}

fn online_backup(source_path: &Path, destination_path: &Path) -> Result<()> {
    let source =
        Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    source
        .execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
        .map_err(err)?;
    let mut target = Connection::open(destination_path).map_err(err)?;
    target
        .execute_batch("PRAGMA trusted_schema=OFF;")
        .map_err(err)?;
    let backup = Backup::new(&source, &mut target).map_err(err)?;
    backup
        .run_to_completion(128, Duration::from_millis(20), None)
        .map_err(err)?;
    drop(backup);
    verify_database(&target)?;
    Ok(())
}

fn initialize_trusted_library(path: &Path) -> Result<()> {
    drop(Store::open(path).map_err(err)?);
    Ok(())
}

fn initialize_trusted_history(path: &Path) -> Result<()> {
    drop(crate::playback_history::PlaybackHistory::open(path).map_err(err)?);
    Ok(())
}

fn open_archive_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")
        .map_err(err)?;
    verify_database(&conn)?;
    Ok(conn)
}

fn verify_database(conn: &Connection) -> Result<()> {
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(err)?;
    if integrity != "ok" {
        return Err("Backup SQLite integrity check failed".into());
    }
    let mut stmt = conn.prepare("PRAGMA foreign_key_check").map_err(err)?;
    if stmt.exists([]).map_err(err)? {
        return Err("Backup SQLite foreign-key check failed".into());
    }
    Ok(())
}

fn sanitize_library_snapshot(path: &Path, snapshot_id: Uuid) -> Result<BackupStateSummary> {
    let conn = Connection::open(path).map_err(err)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON;")
        .map_err(err)?;
    let music_root = read_preference(&conn, "application")?.and_then(|v| {
        v.get("musicDirectory")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let tx = conn.unchecked_transaction().map_err(err)?;
    let mut summary = BackupStateSummary::default();
    for table in [
        "provider_cache",
        "preview_sources",
        "acquisition_candidates",
        "artist_mappings",
        "verified_playback_sources",
    ] {
        summary.omitted_cache_rows += delete_rows_if_table_exists(&tx, table)?;
    }
    summary.omitted_jobs = delete_rows_if_table_exists(&tx, "download_jobs")?;
    for spec in LIBRARY_TABLES {
        sanitize_table(&tx, spec, music_root.as_deref())?;
    }
    sanitize_settings_table(&tx, &mut summary)?;
    count_library(&tx, &mut summary)?;
    tx.execute("INSERT INTO settings(key,value) VALUES('backupRestore.snapshotId.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::json!(snapshot_id.to_string()).to_string()]).map_err(err)?;
    tx.commit().map_err(err)?;
    verify_database(&conn)?;
    Ok(summary)
}

fn sanitize_history_snapshot(path: &Path, summary: &mut BackupStateSummary) -> Result<()> {
    let conn = Connection::open(path).map_err(err)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON;")
        .map_err(err)?;
    let cols = table_columns(&conn, "playback_history")?;
    if ![
        "session_id",
        "played_at",
        "provider",
        "track_json",
        "chapter_json",
    ]
    .iter()
    .all(|c| cols.contains(*c))
    {
        return Err("Playback history schema is unsupported".into());
    }
    let tx = conn.unchecked_transaction().map_err(err)?;
    let mut cursor = String::new();
    let mut started = false;
    let mut total = 0usize;
    loop {
        let mut stmt = tx.prepare("SELECT session_id,track_json,chapter_json FROM playback_history WHERE (?1=0 OR session_id>?2) ORDER BY session_id LIMIT 256").map_err(err)?;
        let mut rows = stmt
            .query(params![i64::from(started), cursor])
            .map_err(err)?;
        let mut page = Vec::new();
        while let Some(row) = rows.next().map_err(err)? {
            let session_id: String = row.get(0).map_err(err)?;
            let raw_track: String = row.get(1).map_err(err)?;
            let raw_chapter: Option<String> = row.get(2).map_err(err)?;
            let track = sanitize_track_value(&parse_bounded_json(&raw_track)?);
            let chapter = raw_chapter
                .map(|s| parse_bounded_json(&s).map(|v| sanitize_json(&v)))
                .transpose()?;
            page.push((
                session_id,
                serde_json::to_string(&track).map_err(err)?,
                chapter.map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "null".into())),
            ));
        }
        drop(rows);
        drop(stmt);
        if page.is_empty() {
            break;
        }
        for (id, track, chapter) in &page {
            total += 1;
            if total > MAX_HISTORY_ROWS {
                return Err("Playback history exceeds backup row limit".into());
            }
            tx.execute(
                "UPDATE playback_history SET track_json=?1, chapter_json=?2 WHERE session_id=?3",
                params![track, chapter, id],
            )
            .map_err(err)?;
        }
        cursor = page.last().map(|row| row.0.clone()).unwrap_or(cursor);
        started = true;
    }
    tx.commit().map_err(err)?;
    summary.history_events = total;
    verify_database(&conn)
}

fn sanitize_table(
    tx: &rusqlite::Transaction<'_>,
    spec: &TableSpec,
    music_root: Option<&str>,
) -> Result<()> {
    if !table_exists(tx, spec.name)? {
        return Ok(());
    }
    let columns = table_columns(tx, spec.name)?;
    if !columns.contains("id")
        && !matches!(
            spec.name,
            "collection_members"
                | "playlist_ordered_collections"
                | "playlist_ordered_entries"
                | "asset_analyses"
                | "asset_autotags"
                | "analysis_cache_records"
                | "analysis_cache_record_refs"
                | "chart_entries"
                | "browser_sources"
                | "browser_imports"
        )
    {
        return Err(format!(
            "Backup table {} is missing its identity column",
            spec.name
        ));
    }
    let present = spec
        .columns
        .iter()
        .filter(|c| columns.contains(**c))
        .copied()
        .collect::<Vec<_>>();
    if present.is_empty() {
        return Ok(());
    }
    let mut cursor = 0i64;
    let mut scanned = 0usize;
    loop {
        let select = format!(
            "SELECT rowid,{} FROM {} WHERE rowid>?1 ORDER BY rowid LIMIT 256",
            quoted_columns(&present),
            spec.name
        );
        let mut stmt = tx.prepare(&select).map_err(err)?;
        let mut rows = stmt.query([cursor]).map_err(err)?;
        let mut page = Vec::new();
        while let Some(row) = rows.next().map_err(err)? {
            let rowid: i64 = row.get(0).map_err(err)?;
            let values = present
                .iter()
                .enumerate()
                .map(|(i, name)| Ok((*name, row.get::<_, SqlValue>(i + 1).map_err(err)?)))
                .collect::<Result<Vec<_>>>()?;
            page.push((rowid, values));
        }
        drop(rows);
        drop(stmt);
        if page.is_empty() {
            break;
        }
        for (rowid, values) in &page {
            scanned += 1;
            if scanned > MAX_ROWS_PER_TABLE {
                return Err(format!("Backup table {} exceeds row limit", spec.name));
            }
            let asset_id = values
                .iter()
                .find(|(name, _)| *name == "id")
                .and_then(|(_, value)| sql_text(value))
                .unwrap_or("");
            let local_asset_path = (spec.name == "local_assets")
                .then(|| {
                    values
                        .iter()
                        .find(|(name, _)| *name == "path")
                        .and_then(|(_, value)| sql_text(value))
                })
                .flatten();
            let local_asset_recording_id = (spec.name == "local_assets")
                .then(|| {
                    values
                        .iter()
                        .find(|(name, _)| *name == "recording_id")
                        .and_then(|(_, value)| sql_text(value))
                })
                .flatten();
            let local_asset_hash = (spec.name == "local_assets")
                .then(|| {
                    values
                        .iter()
                        .find(|(name, _)| *name == "hash")
                        .and_then(|(_, value)| sql_text(value))
                })
                .flatten();
            let autotag_status = (spec.name == "asset_autotags")
                .then(|| {
                    values
                        .iter()
                        .find(|(name, _)| *name == "status")
                        .and_then(|(_, value)| sql_text(value))
                })
                .flatten();
            let analysis_version = (spec.name == "asset_analyses")
                .then(|| {
                    values
                        .iter()
                        .find(|(name, _)| *name == "version")
                        .and_then(|(_, value)| sql_text(value))
                })
                .flatten();
            let mut sets = Vec::new();
            let mut set_values: Vec<SqlValue> = Vec::new();
            for (name, value) in values {
                let clean = sanitize_column(
                    spec.name,
                    name,
                    value.clone(),
                    music_root,
                    asset_id,
                    local_asset_path,
                    local_asset_recording_id,
                    local_asset_hash,
                    autotag_status,
                    analysis_version,
                )?;
                if let Some(clean) = clean {
                    sets.push(format!("{}=?", name));
                    set_values.push(clean);
                }
            }
            if sets.is_empty() {
                continue;
            }
            let sql = format!("UPDATE {} SET {} WHERE rowid=?", spec.name, sets.join(","));
            set_values.push(SqlValue::Integer(*rowid));
            tx.execute(&sql, params_from_iter(set_values))
                .map_err(err)?;
        }
        cursor = page.last().map(|(rowid, _)| *rowid).unwrap_or(cursor);
    }
    if spec.name == "catalog_listings" {
        let derived = [
            "sort_availability_mask",
            "sort_availability_revision",
            "sort_metadata_projection",
            "sort_projection_revision",
        ];
        let present = derived
            .iter()
            .filter(|name| columns.contains(**name))
            .copied()
            .collect::<Vec<_>>();
        if !present.is_empty() {
            tx.execute(
                &format!(
                    "UPDATE catalog_listings SET {}",
                    present
                        .iter()
                        .map(|n| format!("{n}=NULL"))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                [],
            )
            .map_err(err)?;
        }
    }
    Ok(())
}

fn sanitize_column(
    table: &str,
    column: &str,
    value: SqlValue,
    music_root: Option<&str>,
    asset_id: &str,
    local_asset_path: Option<&str>,
    local_asset_recording_id: Option<&str>,
    local_asset_hash: Option<&str>,
    autotag_status: Option<&str>,
    analysis_version: Option<&str>,
) -> Result<Option<SqlValue>> {
    let clean = match (table, column, value) {
        ("asset_autotags", "status", SqlValue::Text(status)) => {
            // An imported matcher job is evidence from a different profile and
            // can trigger filesystem writes or recognition requests on startup.
            // Keep it available for explicit review/retry, but never executable.
            SqlValue::Text(restored_autotag_status(&status).to_owned())
        }
        ("asset_autotags", "version", SqlValue::Text(_)) => {
            SqlValue::Text(crate::autotag::VERSION.to_owned())
        }
        ("asset_autotags", "payload", SqlValue::Text(raw)) => SqlValue::Text(
            serde_json::to_string(&sanitize_autotag_backup_payload(
                &parse_bounded_json(&raw)?,
                autotag_status.unwrap_or("review"),
            ))
            .map_err(err)?,
        ),
        ("catalog_listings", "source_url", SqlValue::Text(s)) => {
            SqlValue::Text(safe_public_url(&s))
        }
        ("chart_snapshots" | "browser_sources", "source_url", SqlValue::Text(s)) => {
            SqlValue::Text(safe_public_url(&s))
        }
        ("local_assets", "payload", SqlValue::Text(raw)) => {
            let source_path = local_asset_path.ok_or("Local asset path is missing")?;
            let recording_id =
                local_asset_recording_id.ok_or("Local asset recording ID is missing")?;
            let hash = local_asset_hash.ok_or("Local asset hash is missing")?;
            let archive_path = archive_asset_path(source_path, music_root, asset_id);
            SqlValue::Text(
                serde_json::to_string(&sanitize_local_asset_payload(
                    &raw,
                    asset_id,
                    recording_id,
                    hash,
                    &archive_path,
                )?)
                .map_err(err)?,
            )
        }
        ("asset_analyses", "payload", SqlValue::Text(raw)) => {
            let safe = if let Some(version) =
                analysis_version.filter(|version| is_loudness_analysis_cache_version(version))
            {
                sanitize_loudness_analysis_payload(&raw, version).unwrap_or(Value::Null)
            } else {
                sanitize_json(&parse_bounded_json(&raw)?)
            };
            SqlValue::Text(serde_json::to_string(&safe).map_err(err)?)
        }
        (_, "manual_metadata", SqlValue::Text(s)) => SqlValue::Text(
            serde_json::to_string(&sanitize_manual_metadata(&parse_bounded_json(&s)?))
                .map_err(err)?,
        ),
        (_, "metadata" | "payload", SqlValue::Text(s)) => SqlValue::Text(
            serde_json::to_string(&sanitize_json(&parse_bounded_json(&s)?)).map_err(err)?,
        ),
        ("local_assets", "path", SqlValue::Text(path)) => {
            SqlValue::Text(archive_asset_path(&path, music_root, asset_id))
        }
        ("recordings", "favorite", SqlValue::Integer(n)) => SqlValue::Integer(i64::from(n != 0)),
        (_, _, value) => value,
    };
    Ok(Some(clean))
}

fn restored_autotag_status(status: &str) -> &str {
    if matches!(
        status,
        "queued" | "retry" | "matching" | "paused" | "approved" | "review"
    ) {
        "restore_review"
    } else {
        status
    }
}

/// AutoTag rows are personal review history, but their queue state, matching
/// proof, recognition output and provider URLs are not portable execution
/// authority. Preserve only bounded candidate display metadata and require a
/// fresh explicit retry before those candidates can be used again.
fn sanitize_autotag_backup_payload(value: &Value, source_status: &str) -> Value {
    let Some(object) = value.as_object() else {
        let status = restored_autotag_status(source_status);
        return serde_json::json!({"assetId":"invalid","inputHash":"","version":"autotag-v1","updatedAt":"1970-01-01T00:00:00Z","status":status,"attempts":0,"candidates":[],"message":if status == "restore_review" { Some("Imported matcher state requires a fresh explicit retry.") } else { None }});
    };
    let asset_id = object
        .get("assetId")
        .and_then(Value::as_str)
        .filter(|id| safe_opaque_identity(id))
        .unwrap_or("invalid");
    let input_hash = object
        .get("inputHash")
        .and_then(Value::as_str)
        .filter(|hash| valid_sha256(hash))
        .unwrap_or("");
    let version = crate::autotag::VERSION;
    let updated_at = object
        .get("updatedAt")
        .and_then(Value::as_str)
        .filter(|date| safe_text(date, 128))
        .unwrap_or("1970-01-01T00:00:00Z");
    let status = restored_autotag_status(source_status);
    let candidates = object
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            let candidate = candidate.as_object()?;
            let provider = candidate.get("provider")?.as_str()?;
            if !["musicbrainz", "discogs"].contains(&provider) {
                return None;
            }
            let provider_id = candidate.get("providerId")?.as_str()?;
            if !safe_text(provider_id, 256) || looks_like_url(provider_id) {
                return None;
            }
            let title = candidate.get("title")?.as_str()?;
            let artist = candidate.get("artist")?.as_str()?;
            if !safe_text(title, 1000) || !safe_text(artist, 1000) {
                return None;
            }
            let mut fields = Map::from_iter([
                ("id".into(), Value::String(String::new())),
                ("provider".into(), Value::String(String::new())),
                ("providerId".into(), Value::String(String::new())),
                ("sourceUrl".into(), Value::String(String::new())),
                ("artist".into(), Value::String(artist.to_owned())),
                ("title".into(), Value::String(title.to_owned())),
                ("favorite".into(), Value::Bool(false)),
            ]);
            if let Some(source_fields) = candidate.get("fields").and_then(Value::as_object) {
                for key in [
                    "artist",
                    "title",
                    "album",
                    "albumArtist",
                    "releaseDate",
                    "isrc",
                    "genre",
                    "label",
                    "catalogNumber",
                    "trackNumber",
                    "trackTotal",
                    "discNumber",
                    "discTotal",
                ] {
                    if let Some(value) = source_fields.get(key) {
                        let numeric =
                            ["trackNumber", "trackTotal", "discNumber", "discTotal"].contains(&key);
                        let valid = if numeric {
                            value.is_null()
                                || value
                                    .as_u64()
                                    .is_some_and(|number| number <= u32::MAX as u64)
                        } else {
                            value.as_str().is_some_and(|text| safe_text(text, 1000))
                                || value.is_null()
                        };
                        if valid {
                            fields.insert(key.into(), value.clone());
                        }
                    }
                }
            }
            let duration = candidate
                .get("durationSeconds")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n > 0.0 && *n <= 86_400.0);
            let score = candidate
                .get("score")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))?;
            Some(serde_json::json!({
                "provider": provider,
                "providerId": provider_id,
                "sourceUrl": "",
                "title": title,
                "artist": artist,
                "durationSeconds": duration,
                "score": score,
                "fields": fields,
                "evidence": [],
            }))
        })
        .take(64)
        .collect::<Vec<_>>();
    serde_json::json!({
        "assetId":asset_id,
        "inputHash":input_hash,
        "version":version,
        "updatedAt":updated_at,
        "status":status,
        "attempts":0,
        "nextAttemptAt":null,
        "message":if status == "restore_review" { Some("Imported matcher state is paused. Retry to revalidate this audio before applying metadata.") } else { None },
        "candidates":candidates,
        "chosen":null,
        "recognition":null,
        "approvedFields":[],
        "excerptOnly":false,
    })
}

fn sanitize_local_asset_payload(
    raw: &str,
    asset_id: &str,
    recording_id: &str,
    hash: &str,
    archived_path: &str,
) -> Result<Value> {
    if !safe_opaque_identity(asset_id) || !safe_opaque_identity(recording_id) || !valid_sha256(hash)
    {
        return Err("Local asset identity or hash is invalid".into());
    }
    let source = parse_bounded_json(raw)?;
    let object = source.as_object().ok_or("Local asset payload is invalid")?;
    let source_codec = object
        .get("sourceCodec")
        .and_then(Value::as_str)
        .filter(|value| safe_text(value, 64))
        .ok_or("Local asset source codec is invalid")?;
    let final_codec = object
        .get("finalCodec")
        .and_then(Value::as_str)
        .filter(|value| safe_text(value, 64))
        .ok_or("Local asset final codec is invalid")?;
    let processing = object
        .get("processing")
        .and_then(Value::as_str)
        .filter(|value| safe_text(value, 64))
        .ok_or("Local asset processing state is invalid")?;
    let track = object
        .get("track")
        .filter(|track| track.is_object())
        .ok_or("Local asset track is missing")?;
    let mut clean = serde_json::json!({
        "id": asset_id,
        "recordingId": recording_id,
        "path": archived_path,
        "hash": hash,
        "sourceCodec": source_codec,
        "finalCodec": final_codec,
        "sourceBitrate": object.get("sourceBitrate").cloned().unwrap_or(Value::Null),
        "finalBitrate": object.get("finalBitrate").cloned().unwrap_or(Value::Null),
        "bitrateOrigin": object.get("bitrateOrigin").cloned().unwrap_or(Value::Null),
        "sampleRate": object.get("sampleRate").cloned().unwrap_or(Value::Null),
        "channels": object.get("channels").cloned().unwrap_or(Value::Null),
        "bitDepth": object.get("bitDepth").cloned().unwrap_or(Value::Null),
        "durationSeconds": object.get("durationSeconds").cloned().unwrap_or(Value::Null),
        "sizeBytes": object.get("sizeBytes").cloned().unwrap_or(Value::Null),
        "processing": processing,
        "track": sanitize_track_value(track),
    });
    for (key, minimum, maximum) in [
        ("sourceBitrate", 0.0, 100_000.0),
        ("finalBitrate", 0.0, 100_000.0),
        ("durationSeconds", 0.0, 604_800.0),
    ] {
        if clean[key].is_null()
            || clean[key]
                .as_f64()
                .is_some_and(|value| value.is_finite() && value > minimum && value <= maximum)
        {
            continue;
        }
        clean[key] = Value::Null;
    }
    for (key, maximum) in [("sampleRate", 768_000), ("channels", 64), ("bitDepth", 64)] {
        if clean[key].is_null() || clean[key].as_u64().is_some_and(|value| value <= maximum) {
            continue;
        }
        clean[key] = Value::Null;
    }
    if !clean["bitrateOrigin"]
        .as_str()
        .is_some_and(|value| matches!(value, "stream" | "format" | "file-size-estimate"))
    {
        clean["bitrateOrigin"] = Value::Null;
    }
    let _: crate::models::LocalAsset = serde_json::from_value(clean.clone()).map_err(err)?;
    Ok(clean)
}

fn sanitize_settings_table(
    tx: &rusqlite::Transaction<'_>,
    summary: &mut BackupStateSummary,
) -> Result<()> {
    if !table_exists(tx, "settings")? {
        return Err("Backup library database has no settings table".into());
    }
    let mut stmt = tx.prepare("SELECT key,value FROM settings").map_err(err)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(err)?;
    let mut saved = Vec::new();
    for item in rows {
        let (key, raw) = item.map_err(err)?;
        if key == "application" {
            let value = parse_bounded_json(&raw)?;
            saved.push((
                key,
                serde_json::to_string(&sanitize_application(&value)).map_err(err)?,
            ));
        } else if is_preference_allowlisted(&key) {
            let mut value = parse_bounded_json(&raw)?;
            if key == "playlistSync.v1" {
                crate::storage::expand_playlist_snapshot_refs(&mut value, |key| {
                    let raw: Option<String> = tx
                        .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                            row.get(0)
                        })
                        .optional()
                        .map_err(err)?;
                    raw.map(|raw| parse_bounded_json(&raw)).transpose()
                })?;
                validate_playlist_sync_snapshot_limits(&value)?;
            }
            if let Some(value) = sanitize_preference(&key, value) {
                saved.push((key, serde_json::to_string(&value).map_err(err)?));
            }
        } else {
            summary.omitted_private_settings += 1;
        }
    }
    drop(stmt);
    tx.execute("DELETE FROM settings", []).map_err(err)?;
    for (key, value) in saved {
        tx.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)",
            params![key, value],
        )
        .map_err(err)?;
    }
    Ok(())
}

fn build_settings_archive(
    path: &Path,
    snapshot_id: Uuid,
    frontend_preferences: Option<Value>,
    summary: &mut BackupStateSummary,
) -> Result<SettingsArchive> {
    let conn = open_archive_db(path)?;
    let mut stmt = conn
        .prepare("SELECT key,value FROM settings")
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(err)?;
    let mut application = serde_json::to_value(Settings::default()).map_err(err)?;
    let mut preferences = BTreeMap::new();
    let mut safe_frontend_preferences = BTreeMap::new();
    for row in rows {
        let (key, value) = row.map_err(err)?;
        if key == "application" {
            let json = parse_bounded_json(&value)?;
            application = sanitize_application(&json);
        } else if is_preference_allowlisted(&key) {
            let mut json = parse_bounded_json(&value)?;
            if key == "playlistSync.v1" {
                crate::storage::expand_playlist_snapshot_refs(&mut json, |key| {
                    let raw: Option<String> = conn
                        .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                            row.get(0)
                        })
                        .optional()
                        .map_err(err)?;
                    raw.map(|raw| parse_bounded_json(&raw)).transpose()
                })?;
            }
            if let Some(clean) = sanitize_preference(&key, json) {
                preferences.insert(key, clean);
            }
        }
    }
    if let Some(frontend) = frontend_preferences.as_ref().and_then(Value::as_object) {
        for (key, value) in frontend {
            if let Some(clean) = sanitize_frontend_preference(key, value.clone()) {
                safe_frontend_preferences.insert(key.clone(), clean);
            }
        }
    }
    let mut hints = Vec::new();
    if table_exists(&conn, "local_assets")? {
        let mut assets = conn
            .prepare("SELECT id,path FROM local_assets ORDER BY id")
            .map_err(err)?;
        let rows = assets
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?;
        for row in rows {
            let (asset_id, path) = row.map_err(err)?;
            if let Some((root_key, relative_path)) = split_asset_hint(&path) {
                hints.push(AssetPathHint {
                    asset_id,
                    root_key,
                    relative_path,
                });
            }
        }
    }
    summary.path_hints = hints.len();
    let excluded_application_fields = application
        .as_object()
        .map(|fields| {
            fields
                .len()
                .saturating_sub(portable_application_fields(&application).len())
        })
        .unwrap_or(0);
    Ok(SettingsArchive {
        format: "org.burrata.settings".into(),
        schema_version: SETTINGS_SCHEMA_VERSION,
        snapshot_id: snapshot_id.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        application,
        preferences,
        frontend_preferences: safe_frontend_preferences,
        asset_path_hints: hints,
        projection_summary: SettingsProjectionSummary {
            omitted_cache_rows: summary.omitted_cache_rows,
            omitted_jobs: summary.omitted_jobs,
            omitted_private_settings: summary.omitted_private_settings,
            excluded_application_fields,
            asset_path_hints: summary.path_hints,
        },
    })
}

fn build_playback_archive(
    path: &Path,
    snapshot_id: Uuid,
    webview: Option<Value>,
) -> Result<PlaybackArchive> {
    let conn = open_archive_db(path)?;
    let mut queue: Value = Value::Null;
    let mut session: Value = Value::Null;
    if let Some(value) = webview {
        if value.get("queue").is_some() {
            queue = value.get("queue").cloned().unwrap_or(Value::Null);
            session = value.get("session").cloned().unwrap_or(Value::Null);
        } else {
            // Frontend localStorage snapshot: PlaybackQueueSnapshot plus the
            // separately persisted position cursor.
            session = serde_json::json!({
                "index": value.get("currentIndex").cloned().unwrap_or(Value::from(0)),
                "position": value.get("positionSeconds").cloned().unwrap_or(Value::from(0.0)),
                "repeat": value.get("repeat").cloned().unwrap_or(Value::String("off".into())),
                "shuffle": value.get("shuffle").cloned().unwrap_or(Value::Bool(false)),
                "volume": value.get("volume").cloned().unwrap_or(Value::from(0.8))
            });
            queue = value;
        }
    }
    if !queue.is_object() || !session.is_object() {
        queue = read_preference(&conn, "nativePlaybackQueue.v1")?.unwrap_or(Value::Null);
        session = read_preference(&conn, "nativePlaybackSession.v1")?.unwrap_or(Value::Null);
    }
    let raw_tracks = queue
        .get("tracks")
        .and_then(Value::as_array)
        .or_else(|| session.get("tracks").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    if raw_tracks.len() > 10_000 {
        return Err("Saved playback queue exceeds backup limit".into());
    }
    let tracks = raw_tracks
        .iter()
        .map(sanitize_track_value)
        .collect::<Vec<_>>();
    let mut entry_ids = queue
        .get("entryIds")
        .and_then(Value::as_array)
        .or_else(|| session.get("entryIds").and_then(Value::as_array))
        .and_then(|a| {
            a.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_default();
    if entry_ids.len() != tracks.len()
        || entry_ids.iter().any(|id| id.is_empty() || id.len() > 128)
        || entry_ids.iter().collect::<HashSet<_>>().len() != entry_ids.len()
    {
        entry_ids = (0..tracks.len())
            .map(|_| Uuid::new_v4().to_string())
            .collect();
    }
    let index = if tracks.is_empty() {
        -1
    } else {
        session
            .get("index")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, tracks.len().saturating_sub(1) as i64)
    };
    let current_entry_id = if index >= 0 {
        entry_ids.get(index as usize).cloned()
    } else {
        None
    };
    let position_seconds = finite_nonnegative(
        session
            .get("position")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    );
    let volume = session
        .get("volume")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(0.8)
        .clamp(0.0, 1.0);
    let repeat = session
        .get("repeat")
        .and_then(Value::as_str)
        .filter(|v| matches!(*v, "off" | "all" | "one"))
        .unwrap_or("off")
        .to_owned();
    let shuffle = session
        .get("shuffle")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let queue_revision = queue
        .get("queueRevision")
        .and_then(Value::as_u64)
        .or_else(|| session.get("queueRevision").and_then(Value::as_u64))
        .unwrap_or(1)
        .max(1);
    let queue_loudness_contexts = sanitize_queue_loudness_contexts(
        queue
            .get("queueLoudnessContexts")
            .or_else(|| session.get("queueLoudnessContexts")),
        &entry_ids,
    );
    Ok(PlaybackArchive {
        format: "org.burrata.playback".into(),
        schema_version: PLAYBACK_SCHEMA_VERSION,
        snapshot_id: snapshot_id.to_string(),
        queue: tracks,
        entry_ids,
        queue_loudness_contexts,
        current_index: index,
        current_entry_id,
        queue_revision,
        position_seconds,
        repeat,
        shuffle,
        volume,
        saved_at: chrono::Utc::now().to_rfc3339().into(),
        restore_paused: true,
    })
}

fn merge_library(
    source: &Connection,
    target: &rusqlite::Transaction<'_>,
    options: &RestoreStateOptions,
    summary: &mut RestoreStateSummary,
    occurrence_id_mapping: &mut HashMap<String, String>,
    asset_id_mapping: &mut HashMap<String, String>,
    listing_id_mapping: &mut HashMap<String, String>,
) -> Result<()> {
    let user_version: i64 = source
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(err)?;
    if !(1..=14).contains(&user_version) {
        return Err("Unsupported library backup schema version".into());
    }
    // Reject archive-defined views, virtual tables, and shadow tables, even if
    // they use the name of an otherwise optional versioned table.
    for spec in LIBRARY_TABLES {
        if object_exists(source, spec.name)? && !is_plain_table(source, spec.name)? {
            return Err(format!(
                "Backup table {} is not an ordinary table",
                spec.name
            ));
        }
    }
    for required in [
        "works",
        "recordings",
        "catalog_listings",
        "collections",
        "local_assets",
        "settings",
    ] {
        if !table_exists(source, required)? {
            return Err(format!(
                "Backup library is missing required table {required}"
            ));
        }
    }
    if user_version >= 11
        && !table_columns(source, "playlist_ordered_entries")?.contains("occurrence_id")
    {
        return Err("Library schema v11 is missing playlist occurrence IDs".into());
    }
    let incoming_collections = read_rows(source, "collections", &["id", "name", "deleted_at"])?;
    let mut collection_stmt = target.prepare("SELECT id FROM collections").map_err(err)?;
    let existing_collections = collection_stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(err)?;
    let new_collection_ids = incoming_collections
        .iter()
        .filter_map(|r| text(r, "id"))
        .filter(|id| !existing_collections.contains(*id))
        .cloned()
        .collect::<HashSet<_>>();
    let mut recording_mapping = HashMap::<String, String>::new();

    merge_simple_table(source, target, "works", &LIBRARY_TABLES[0], &[], summary)?;
    merge_recordings(source, target, summary)?;
    let listing_rows = read_rows(source, "catalog_listings", LIBRARY_TABLES[2].columns)?;
    for row in listing_rows {
        let source_id = text(&row, "id")
            .ok_or("Catalog listing ID is missing")?
            .clone();
        let provider = text(&row, "provider")
            .map(String::as_str)
            .unwrap_or("")
            .to_owned();
        let provider_id = text(&row, "provider_id")
            .map(String::as_str)
            .unwrap_or("")
            .to_owned();
        let source_recording = text(&row, "recording_id")
            .ok_or("Catalog listing recording ID is missing")?
            .clone();
        let existing = target.query_row("SELECT id,recording_id FROM catalog_listings WHERE id=?1 OR (provider=?2 AND provider_id=?3) ORDER BY CASE WHEN id=?1 THEN 0 ELSE 1 END LIMIT 1", params![source_id,provider,provider_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional().map_err(err)?;
        let (dest_id, dest_recording) = if let Some(pair) = existing {
            summary.skipped_conflicts += 1;
            pair
        } else {
            let cols = LIBRARY_TABLES[2]
                .columns
                .iter()
                .copied()
                .filter(|c| row.contains_key(*c))
                .collect::<Vec<_>>();
            insert_values(target, "catalog_listings", &cols, &row, false)?;
            summary.imported_catalog_listings += 1;
            (source_id.clone(), source_recording.clone())
        };
        listing_id_mapping.insert(source_id, dest_id);
        recording_mapping
            .entry(source_recording)
            .or_insert(dest_recording);
    }
    // Keep durable browser-source definitions with the sanitized snapshot that
    // they point at; unrelated chart/provider cache tables remain excluded.
    merge_simple_table(
        source,
        target,
        "chart_snapshots",
        &LIBRARY_TABLES[3],
        &[],
        summary,
    )?;
    for mut row in read_rows(source, "chart_entries", LIBRARY_TABLES[4].columns)? {
        if let Some(mapped) = text(&row, "listing_id")
            .and_then(|id| listing_id_mapping.get(id))
            .cloned()
        {
            row.insert("listing_id".into(), SqlValue::Text(mapped));
        }
        let _ = insert_values(
            target,
            "chart_entries",
            LIBRARY_TABLES[4].columns,
            &row,
            true,
        )?;
    }
    for row in incoming_collections {
        let id = text(&row, "id").ok_or("Collection ID is missing")?.clone();
        if new_collection_ids.contains(&id) {
            insert_values(
                target,
                "collections",
                &["id", "name", "deleted_at"],
                &row,
                false,
            )?;
            summary.imported_collections += 1;
        } else {
            summary.skipped_conflicts += 1;
        }
    }
    merge_simple_table(
        source,
        target,
        "browser_sources",
        &LIBRARY_TABLES[5],
        &new_collection_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        summary,
    )?;
    merge_simple_table(
        source,
        target,
        "browser_imports",
        &LIBRARY_TABLES[6],
        &[],
        summary,
    )?;
    for mut row in read_rows(source, "collection_members", LIBRARY_TABLES[8].columns)? {
        let collection = text(&row, "collection_id").cloned().unwrap_or_default();
        if !new_collection_ids.contains(&collection) {
            continue;
        }
        if let Some(mapped) = text(&row, "recording_id")
            .and_then(|id| recording_mapping.get(id))
            .cloned()
        {
            row.insert("recording_id".into(), SqlValue::Text(mapped));
        }
        let _ = insert_values(
            target,
            "collection_members",
            LIBRARY_TABLES[8].columns,
            &row,
            true,
        )?;
    }
    merge_simple_table(
        source,
        target,
        "playlist_ordered_collections",
        &LIBRARY_TABLES[9],
        &new_collection_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        summary,
    )?;
    let mut ordered = read_rows(
        source,
        "playlist_ordered_entries",
        LIBRARY_TABLES[10].columns,
    )?;
    // read_rows is schema agnostic; impose the source ordering before inserting
    // so duplicate listing occurrences retain their stable IDs in playlist order.
    ordered.sort_by(|left, right| {
        text(left, "collection_id")
            .cmp(&text(right, "collection_id"))
            .then_with(|| {
                left.get("position")
                    .and_then(sql_integer)
                    .cmp(&right.get("position").and_then(sql_integer))
            })
    });
    if ordered
        .iter()
        .any(|row| text(row, "occurrence_id").is_some_and(|id| Uuid::parse_str(id).is_err()))
    {
        return Err("Library backup has an invalid playlist occurrence ID".into());
    }
    if ordered
        .iter()
        .filter_map(|row| text(row, "occurrence_id"))
        .collect::<HashSet<_>>()
        .len()
        != ordered
            .iter()
            .filter(|row| text(row, "occurrence_id").is_some())
            .count()
    {
        return Err("Library backup has duplicate playlist occurrence IDs".into());
    }
    for mut row in ordered {
        let collection = text(&row, "collection_id").cloned().unwrap_or_default();
        if !new_collection_ids.contains(&collection) {
            continue;
        }
        if let Some(listing) = text(&row, "listing_id")
            .and_then(|id| listing_id_mapping.get(id))
            .cloned()
        {
            row.insert("listing_id".into(), SqlValue::Text(listing));
        }
        if let Some(imported_id) = text(&row, "occurrence_id").cloned() {
            let collision: bool = target
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM playlist_ordered_entries WHERE occurrence_id=?1)",
                    [&imported_id],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if collision {
                let remapped_id = loop {
                    let candidate = Uuid::new_v4().to_string();
                    let exists: bool = target
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM playlist_ordered_entries WHERE occurrence_id=?1)",
                            [&candidate],
                            |r| r.get(0),
                        )
                        .map_err(err)?;
                    if !exists {
                        break candidate;
                    }
                };
                row.insert("occurrence_id".into(), SqlValue::Text(remapped_id.clone()));
                occurrence_id_mapping.insert(imported_id, remapped_id);
            }
        }
        let columns = LIBRARY_TABLES[10]
            .columns
            .iter()
            .copied()
            .filter(|column| row.contains_key(*column))
            .collect::<Vec<_>>();
        insert_values(target, "playlist_ordered_entries", &columns, &row, false)?;
        summary.imported_playlist_occurrences += 1;
    }
    merge_assets(
        source,
        target,
        options,
        &recording_mapping,
        asset_id_mapping,
        summary,
    )?;
    Ok(())
}

fn merge_simple_table(
    source: &Connection,
    target: &rusqlite::Transaction<'_>,
    name: &str,
    spec: &TableSpec,
    only_collections: &[&str],
    summary: &mut RestoreStateSummary,
) -> Result<()> {
    for row in read_rows(source, name, spec.columns)? {
        if !only_collections.is_empty()
            && row
                .get("collection_id")
                .and_then(sql_text)
                .is_some_and(|id| !only_collections.contains(&id))
        {
            continue;
        }
        let columns = spec
            .columns
            .iter()
            .copied()
            .filter(|c| row.contains_key(*c))
            .collect::<Vec<_>>();
        match insert_values(target, name, &columns, &row, true) {
            Ok(true) => {}
            Ok(false) => summary.skipped_conflicts += 1,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn merge_recordings(
    source: &Connection,
    target: &rusqlite::Transaction<'_>,
    summary: &mut RestoreStateSummary,
) -> Result<()> {
    for row in read_rows(source, "recordings", LIBRARY_TABLES[1].columns)? {
        let id = text(&row, "id").ok_or("Recording ID is missing")?.clone();
        let exists: bool = target
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM recordings WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            insert_values(target, "recordings", LIBRARY_TABLES[1].columns, &row, false)?;
            summary.imported_recordings += 1;
        } else {
            let favorite = row.get("favorite").and_then(sql_integer).unwrap_or(0) != 0;
            let disposition = row.get("disposition").and_then(sql_text).map(str::to_owned);
            let imported_manual = row
                .get("manual_metadata")
                .and_then(sql_text)
                .map(|s| parse_bounded_json(s))
                .transpose()?
                .unwrap_or(Value::Object(Map::new()));
            let (old_favorite, old_disposition, old_manual): (i64, Option<String>, String) = target
                .query_row(
                    "SELECT favorite,disposition,manual_metadata FROM recordings WHERE id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(err)?;
            let merged = merge_manual_metadata(&parse_bounded_json(&old_manual)?, &imported_manual);
            target.execute("UPDATE recordings SET favorite=?1, disposition=COALESCE(disposition,?2), manual_metadata=?3 WHERE id=?4",params![i64::from(favorite||old_favorite!=0),disposition,serde_json::to_string(&merged).map_err(err)?,id]).map_err(err)?;
            summary.skipped_conflicts += 1;
        }
    }
    Ok(())
}

fn merge_assets(
    source: &Connection,
    target: &rusqlite::Transaction<'_>,
    options: &RestoreStateOptions,
    recording_mapping: &HashMap<String, String>,
    asset_map: &mut HashMap<String, String>,
    summary: &mut RestoreStateSummary,
) -> Result<()> {
    let mappings = options
        .path_mappings
        .iter()
        .map(|m| (m.root_key.as_str(), m.destination.as_path()))
        .collect::<HashMap<_, _>>();
    let assets = read_rows(source, "local_assets", LIBRARY_TABLES[11].columns)?;
    let mut source_asset_hashes = HashMap::new();
    for mut row in assets {
        let id = text(&row, "id").ok_or("Asset ID is missing")?.clone();
        let source_hash = text(&row, "hash").cloned().unwrap_or_default();
        source_asset_hashes.insert(id.clone(), source_hash.clone());
        if let Some(rec) = text(&row, "recording_id")
            .and_then(|v| recording_mapping.get(v))
            .cloned()
        {
            row.insert("recording_id".into(), SqlValue::Text(rec));
        }
        let stored_path = text(&row, "path").cloned().unwrap_or_default();
        let expected_hash = text(&row, "hash").cloned().unwrap_or_default();
        let mut resolved = resolve_asset_path(&id, &stored_path, &expected_hash, &mappings);
        let id_conflict: Option<(String, String)> = target
            .query_row("SELECT id,hash FROM local_assets WHERE id=?1", [&id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(err)?;
        if let Some((existing_id, existing_hash)) = id_conflict {
            summary.skipped_conflicts += 1;
            if existing_hash.eq_ignore_ascii_case(&expected_hash) {
                // A retained same-identity asset is available even when the
                // incoming portable snapshot has no usable path mapping.
                // A verified incoming copy can repair it when its current
                // local file is missing.
                let existing: Option<(String, String, String)> = target
                    .query_row(
                        "SELECT recording_id,path,payload FROM local_assets WHERE id=?1",
                        [&existing_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(err)?;
                if let Some((recording_id, current_path, payload)) = existing {
                    let current_is_verified = Path::new(&current_path).is_absolute()
                        && file_matches_sha256(Path::new(&current_path), &expected_hash);
                    let incoming_is_verified = !resolved.starts_with(MISSING_ASSET_PREFIX)
                        && file_matches_sha256(Path::new(&resolved), &expected_hash);
                    if !current_is_verified && incoming_is_verified {
                        let payload = materialize_restored_asset_payload(
                            &payload,
                            &existing_id,
                            &recording_id,
                            &expected_hash,
                            &resolved,
                        )?;
                        target
                                .execute(
                                    "UPDATE local_assets SET path=?1,payload=?2 WHERE id=?3 AND hash=?4",
                                    params![
                                        resolved,
                                        serde_json::to_string(&payload).map_err(err)?,
                                        existing_id,
                                        expected_hash
                                    ],
                                )
                                .map_err(err)?;
                    } else if !current_is_verified {
                        summary.unavailable_assets += 1;
                    }
                }
                asset_map.insert(id, existing_id);
            } else {
                // The incoming identity conflicts with a different current
                // asset hash and cannot be retained as the same available
                // asset, regardless of whether its path hint resolved.
                summary.unavailable_assets += 1;
            }
            continue;
        }
        if !resolved.starts_with(MISSING_ASSET_PREFIX) {
            let path_conflict: Option<(String, String)> = target
                .query_row(
                    "SELECT id,hash FROM local_assets WHERE path=?1",
                    [&resolved],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            if let Some((existing_id, existing_hash)) = path_conflict {
                if existing_hash.eq_ignore_ascii_case(&expected_hash) {
                    summary.skipped_conflicts += 1;
                    asset_map.insert(id, existing_id);
                    continue;
                }
                // Keep the imported asset as a missing identity instead of
                // attaching it to an unrelated file already at that path.
                resolved = format!("{MISSING_ASSET_PREFIX}{id}");
            }
        }
        if resolved.starts_with(MISSING_ASSET_PREFIX) {
            summary.unavailable_assets += 1;
        }
        row.insert("path".into(), SqlValue::Text(resolved));
        let recording_id = text(&row, "recording_id")
            .cloned()
            .ok_or("Restored asset recording ID is missing")?;
        let payload = row
            .get("payload")
            .and_then(sql_text)
            .ok_or("Restored asset payload is missing")?;
        let resolved_path = text(&row, "path")
            .cloned()
            .ok_or("Restored asset path is missing")?;
        let payload = materialize_restored_asset_payload(
            payload,
            &id,
            &recording_id,
            &expected_hash,
            &resolved_path,
        )?;
        row.insert(
            "payload".into(),
            SqlValue::Text(serde_json::to_string(&payload).map_err(err)?),
        );
        let present = LIBRARY_TABLES[11]
            .columns
            .iter()
            .copied()
            .filter(|c| row.contains_key(*c))
            .collect::<Vec<_>>();
        let inserted = insert_values(target, "local_assets", &present, &row, true)?;
        if inserted {
            summary.imported_assets += 1;
            asset_map.insert(id.clone(), id);
        } else {
            summary.skipped_conflicts += 1;
        }
    }
    for spec in [&LIBRARY_TABLES[12], &LIBRARY_TABLES[13]] {
        for mut row in read_rows(source, spec.name, spec.columns)? {
            let Some(source_asset_id) = text(&row, "asset_id").cloned() else {
                continue;
            };
            let Some(dest) = asset_map.get(&source_asset_id).cloned() else {
                summary.omitted_stale_analyses += 1;
                continue;
            };
            let analysis_hash = text(&row, "file_hash").cloned().unwrap_or_default();
            let source_hash = source_asset_hashes
                .get(&source_asset_id)
                .cloned()
                .unwrap_or_default();
            let target_hash: String = target
                .query_row("SELECT hash FROM local_assets WHERE id=?1", [&dest], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(err)?
                .unwrap_or_default();
            if analysis_hash.is_empty()
                || !analysis_hash.eq_ignore_ascii_case(&source_hash)
                || !analysis_hash.eq_ignore_ascii_case(&target_hash)
            {
                summary.omitted_stale_analyses += 1;
                continue;
            }
            // Archive databases are inert input and may not have been produced
            // by our writer. Reapply the typed AutoTag projection while
            // reconstructing trusted Store rows; never import executable job
            // states or stale provider/recognition evidence.
            if spec.name == "asset_autotags" {
                let status = text(&row, "status")
                    .cloned()
                    .unwrap_or_else(|| "review".to_owned());
                row.insert(
                    "status".into(),
                    SqlValue::Text(restored_autotag_status(&status).to_owned()),
                );
                row.insert(
                    "version".into(),
                    SqlValue::Text(crate::autotag::VERSION.to_owned()),
                );
                if let Some(raw) = row.get("payload").and_then(sql_text) {
                    let mut safe =
                        sanitize_autotag_backup_payload(&parse_bounded_json(raw)?, &status);
                    if let Some(payload) = safe.as_object_mut() {
                        payload.insert("assetId".into(), Value::String(dest.clone()));
                        payload.insert("inputHash".into(), Value::String(analysis_hash.clone()));
                        payload.insert(
                            "status".into(),
                            Value::String(restored_autotag_status(&status).to_owned()),
                        );
                    }
                    row.insert(
                        "payload".into(),
                        SqlValue::Text(serde_json::to_string(&safe).map_err(err)?),
                    );
                }
            } else if spec.name == "asset_analyses" {
                let version = text(&row, "version").cloned().unwrap_or_default();
                if is_loudness_analysis_cache_version(&version) {
                    if text(&row, "status").map(String::as_str) != Some("completed") {
                        summary.omitted_stale_analyses += 1;
                        continue;
                    }
                    let Some(raw) = row.get("payload").and_then(sql_text) else {
                        summary.omitted_stale_analyses += 1;
                        continue;
                    };
                    let Some(safe) = remap_loudness_analysis_payload(
                        raw,
                        &version,
                        &source_asset_id,
                        &analysis_hash,
                        &dest,
                        asset_map,
                        &source_asset_hashes,
                        target,
                    )?
                    else {
                        summary.omitted_stale_analyses += 1;
                        continue;
                    };
                    let Some(mapped_anchor_hash) = safe
                        .pointer("/resource/members/0/fileHash")
                        .and_then(Value::as_str)
                    else {
                        summary.omitted_stale_analyses += 1;
                        continue;
                    };
                    row.insert(
                        "file_hash".into(),
                        SqlValue::Text(mapped_anchor_hash.to_owned()),
                    );
                    row.insert(
                        "payload".into(),
                        SqlValue::Text(serde_json::to_string(&safe).map_err(err)?),
                    );
                } else if let Some(raw) = row.get("payload").and_then(sql_text) {
                    // Preserve future non-loudness analysis payloads as inert
                    // data, applying the same export privacy filters.
                    let safe = sanitize_json(&parse_bounded_json(raw)?);
                    row.insert(
                        "payload".into(),
                        SqlValue::Text(serde_json::to_string(&safe).map_err(err)?),
                    );
                }
            }
            {
                row.insert("asset_id".into(), SqlValue::Text(dest));
            }
            let cols = spec
                .columns
                .iter()
                .copied()
                .filter(|c| row.contains_key(*c))
                .collect::<Vec<_>>();
            let _ = insert_values(target, spec.name, &cols, &row, true)?;
        }
    }
    let cache_key_mapping =
        merge_analysis_cache_records(source, target, recording_mapping, asset_map)?;
    merge_analysis_cache_refs(source, target, recording_mapping, &cache_key_mapping)?;
    Ok(())
}

fn merge_analysis_cache_records(
    source: &Connection,
    target: &rusqlite::Transaction<'_>,
    recording_id_mapping: &HashMap<String, String>,
    asset_id_mapping: &HashMap<String, String>,
) -> Result<HashMap<(String, String), String>> {
    const MAX_RECORD_BYTES: usize = 256 * 1024;
    if !table_exists(source, "analysis_cache_records")? {
        return Ok(HashMap::new());
    }
    let mut cache_key_mapping = HashMap::new();
    for row in read_rows(
        source,
        "analysis_cache_records",
        &[
            "namespace",
            "cache_key",
            "payload",
            "payload_bytes",
            "updated_at",
            "last_used_at",
            "protected",
        ],
    )? {
        let Some(namespace) = text(&row, "namespace") else {
            continue;
        };
        let Some(source_key) = text(&row, "cache_key") else {
            continue;
        };
        let Some(raw) = row.get("payload").and_then(sql_text) else {
            continue;
        };
        if namespace.is_empty()
            || namespace.len() > 64
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            || source_key.len() != 64
            || !source_key
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || raw.len() > MAX_RECORD_BYTES
        {
            continue;
        }
        let mut payload = sanitize_json(&parse_bounded_json(raw)?);
        let mut key = source_key.to_owned();
        if namespace == "stream-analysis-v1" {
            let Ok(mut analysis) = serde_json::from_value::<
                crate::stream_analysis::WholeTrackAnalysis,
            >(payload.clone()) else {
                continue;
            };
            let resource = &mut analysis.resource;
            match resource {
                crate::stream_analysis::AnalysisResource::Stream { recording_id, .. }
                | crate::stream_analysis::AnalysisResource::RegisteredAsset {
                    recording_id, ..
                } => {
                    if let Some(mapped) = recording_id_mapping.get(recording_id) {
                        *recording_id = mapped.clone();
                    }
                }
            }
            key = analysis
                .resource
                .cache_identity()
                .map_err(|error| error.to_string())?;
            payload = serde_json::to_value(analysis).map_err(err)?;
        } else if namespace == "local-waveform-v1" {
            let Some(source_asset_id) = payload.get("assetId").and_then(Value::as_str) else {
                continue;
            };
            let Some(asset_id) = asset_id_mapping.get(source_asset_id) else {
                continue;
            };
            let Some(file_hash) = payload
                .get("fileHash")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            let source_hash: Option<String> = source
                .query_row(
                    "SELECT hash FROM local_assets WHERE id=?1",
                    [source_asset_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(err)?;
            let target_hash: Option<String> = target
                .query_row(
                    "SELECT hash FROM local_assets WHERE id=?1",
                    [asset_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(err)?;
            if !source_hash
                .as_deref()
                .is_some_and(|hash| hash.eq_ignore_ascii_case(&file_hash))
                || !target_hash
                    .as_deref()
                    .is_some_and(|hash| hash.eq_ignore_ascii_case(&file_hash))
            {
                continue;
            }
            if let Some(object) = payload.as_object_mut() {
                object.insert("assetId".into(), Value::String(asset_id.clone()));
            }
            let Some(bin_count) = payload.get("binCount").and_then(Value::as_u64) else {
                continue;
            };
            let Some(analyzer) = payload.get("analyzerVersion").and_then(Value::as_str) else {
                continue;
            };
            let identity = format!(
                "burrata-local-waveform-content-v1\n{}\n{bin_count}\n{analyzer}",
                file_hash.to_ascii_lowercase()
            );
            key = format!("{:x}", sha2::Sha256::digest(identity.as_bytes()));
        }
        let encoded = serde_json::to_string(&payload).map_err(err)?;
        if encoded.len() > MAX_RECORD_BYTES {
            continue;
        }
        let Some(source_updated_at) = text(&row, "updated_at")
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        else {
            continue;
        };
        let source_updated_at = source_updated_at.with_timezone(&chrono::Utc).to_rfc3339();
        let current_updated_at: Option<String> = target
            .query_row(
                "SELECT updated_at FROM analysis_cache_records WHERE namespace=?1 AND cache_key=?2",
                rusqlite::params![namespace, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        if current_updated_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|current| {
                current.with_timezone(&chrono::Utc).to_rfc3339() >= source_updated_at
            })
        {
            continue;
        }
        let now = chrono::Utc::now().to_rfc3339();
        target.execute(
            "INSERT INTO analysis_cache_records(namespace,cache_key,payload,payload_bytes,updated_at,last_used_at,protected)
             VALUES(?1,?2,?3,?4,?5,?6,0)
             ON CONFLICT(namespace,cache_key) DO UPDATE SET payload=excluded.payload,
               payload_bytes=excluded.payload_bytes,updated_at=excluded.updated_at,
               last_used_at=excluded.last_used_at",
            rusqlite::params![namespace, key, encoded, encoded.len() as i64, source_updated_at, now],
        ).map_err(err)?;
        cache_key_mapping.insert((namespace.to_owned(), source_key.to_owned()), key);
    }
    Ok(cache_key_mapping)
}

fn merge_analysis_cache_refs(
    source: &Connection,
    target: &rusqlite::Transaction<'_>,
    recording_id_mapping: &HashMap<String, String>,
    cache_key_mapping: &HashMap<(String, String), String>,
) -> Result<()> {
    if !table_exists(source, "analysis_cache_record_refs")? {
        return Ok(());
    }
    for row in read_rows(
        source,
        "analysis_cache_record_refs",
        &["namespace", "cache_key", "recording_id"],
    )? {
        let Some(namespace) = text(&row, "namespace") else {
            continue;
        };
        let Some(source_key) = text(&row, "cache_key") else {
            continue;
        };
        let Some(source_id) = text(&row, "recording_id") else {
            continue;
        };
        let key = cache_key_mapping
            .get(&(namespace.to_owned(), source_key.to_owned()))
            .map(String::as_str)
            .unwrap_or(source_key);
        if namespace.is_empty()
            || namespace.len() > 64
            || key.len() != 64
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            || !key
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            continue;
        }
        let cache_exists = target.query_row(
            "SELECT EXISTS(SELECT 1 FROM analysis_cache_records WHERE namespace=?1 AND cache_key=?2)",
            rusqlite::params![namespace, key], |row| row.get::<_, bool>(0),
        ).map_err(err)?;
        if !cache_exists {
            continue;
        }
        let recording_id = recording_id_mapping
            .get(source_id)
            .map(String::as_str)
            .unwrap_or(source_id);
        let is_library = target.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_listings WHERE id=?1 AND recording_id=?1 AND in_library=1)",
            [recording_id], |row| row.get::<_, bool>(0),
        ).map_err(err)?;
        if !is_library {
            continue;
        }
        target.execute(
            "INSERT OR IGNORE INTO analysis_cache_record_refs(namespace,cache_key,recording_id) VALUES(?1,?2,?3)",
            rusqlite::params![namespace, key, recording_id],
        ).map_err(err)?;
        target
            .execute(
                "UPDATE analysis_cache_records SET protected=1 WHERE namespace=?1 AND cache_key=?2",
                rusqlite::params![namespace, key],
            )
            .map_err(err)?;
    }
    target.execute(
        "UPDATE analysis_cache_records SET protected=EXISTS(
           SELECT 1 FROM analysis_cache_record_refs r JOIN catalog_listings l
             ON l.id=r.recording_id AND l.recording_id=r.recording_id AND l.in_library=1
           WHERE r.namespace=analysis_cache_records.namespace AND r.cache_key=analysis_cache_records.cache_key)",
        [],
    ).map_err(err)?;
    Ok(())
}

fn loudness_cache_schema(version: &str) -> Option<(crate::loudness::ResourceScope, u32)> {
    use crate::loudness::{ResourceScope, LEGACY_LOUDNESS_SCHEMA_VERSION, LOUDNESS_SCHEMA_VERSION};
    match version {
        "loudness-ebur128-v1" => Some((ResourceScope::Track, LEGACY_LOUDNESS_SCHEMA_VERSION)),
        "loudness-ebur128-album-programme-v1" => {
            Some((ResourceScope::Album, LEGACY_LOUDNESS_SCHEMA_VERSION))
        }
        crate::loudness_manager::ANALYSIS_CACHE_VERSION => {
            Some((ResourceScope::Track, LOUDNESS_SCHEMA_VERSION))
        }
        crate::loudness_manager::ALBUM_ANALYSIS_CACHE_VERSION => {
            Some((ResourceScope::Album, LOUDNESS_SCHEMA_VERSION))
        }
        _ => None,
    }
}

fn is_loudness_analysis_cache_version(version: &str) -> bool {
    loudness_cache_schema(version).is_some()
}

fn sanitize_loudness_analysis_payload(raw: &str, version: &str) -> Option<Value> {
    use crate::loudness::LoudnessRecord;
    let (expected_scope, expected_schema) = loudness_cache_schema(version)?;
    let value = parse_bounded_json(raw).ok()?;
    let mut record = serde_json::from_value::<LoudnessRecord>(value).ok()?;
    if record.schema_version != expected_schema
        || record.resource.scope != expected_scope
        || record.validate().is_err()
        || !safe_loudness_text(&record.analyzer_version, 128)
        || chrono::DateTime::parse_from_rfc3339(&record.measured_at).is_err()
        || record.resource.members.iter().any(|member| {
            !safe_opaque_identity(&member.asset_id) || !valid_sha256(&member.file_hash)
        })
        || record.opus_decoder_proofs.iter().any(|proof| {
            !safe_opaque_identity(&proof.asset_id)
                || !valid_sha256(&proof.file_hash)
                || !valid_sha256(&proof.decoder_sha256)
        })
        || record
            .stereo_mix_matrix
            .as_deref()
            .is_some_and(|value| !safe_loudness_text(value, 128))
        || record
            .stereo_mix_layout
            .as_deref()
            .is_some_and(|value| !safe_loudness_text(value, MAX_LOUDNESS_PROGRAM_LAYOUT_BYTES))
    {
        return None;
    }
    record.local_file_seals.clear();
    let value = serde_json::to_value(record).ok()?;
    // The generic JSON policy interprets `provenance` as manual metadata and
    // turns enum scalars into objects. Preserve this typed enum while applying
    // the generic path, URL and credential filtering to every other field.
    let provenance = value.get("provenance")?.clone();
    let mut safe = sanitize_json(&value);
    safe.as_object_mut()?
        .insert("provenance".into(), provenance);
    let record = serde_json::from_value::<LoudnessRecord>(safe.clone()).ok()?;
    record.validate().ok()?;
    Some(safe)
}

fn safe_loudness_text(value: &str, maximum_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
        && !looks_like_url(value)
        && !looks_like_absolute_path(value)
        && !looks_sensitive_inline(value)
}

fn remap_loudness_analysis_payload(
    raw: &str,
    version: &str,
    source_anchor_id: &str,
    analysis_hash: &str,
    destination_anchor_id: &str,
    asset_map: &HashMap<String, String>,
    source_asset_hashes: &HashMap<String, String>,
    target: &Connection,
) -> Result<Option<Value>> {
    use crate::loudness::{LoudnessRecord, LoudnessResourceIdentity, ResourceScope};
    let Some((expected_scope, expected_schema)) = loudness_cache_schema(version) else {
        return Ok(None);
    };
    let Ok(payload) = parse_bounded_json(raw) else {
        return Ok(None);
    };
    let Ok(mut record) = serde_json::from_value::<LoudnessRecord>(payload) else {
        return Ok(None);
    };
    if record.schema_version != expected_schema
        || record.resource.scope != expected_scope
        || record.validate().is_err()
        || record.resource.members.first().is_none_or(|anchor| {
            anchor.asset_id != source_anchor_id
                || !anchor.file_hash.eq_ignore_ascii_case(analysis_hash)
        })
        || asset_map.get(source_anchor_id).map(String::as_str) != Some(destination_anchor_id)
    {
        return Ok(None);
    }

    let mut mapped_members = Vec::with_capacity(record.resource.members.len());
    for member in &record.resource.members {
        if !valid_sha256(&member.file_hash)
            || source_asset_hashes
                .get(&member.asset_id)
                .is_none_or(|source_hash| !source_hash.eq_ignore_ascii_case(&member.file_hash))
        {
            return Ok(None);
        }
        let Some(destination_id) = asset_map.get(&member.asset_id) else {
            return Ok(None);
        };
        let destination_hash: Option<String> = target
            .query_row(
                "SELECT hash FROM local_assets WHERE id=?1",
                [destination_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        if !destination_hash
            .as_deref()
            .is_some_and(|hash| hash.eq_ignore_ascii_case(&member.file_hash))
        {
            return Ok(None);
        }
        let Some(destination_hash) = destination_hash else {
            return Ok(None);
        };
        mapped_members.push(crate::loudness::LoudnessAssetIdentity {
            asset_id: destination_id.clone(),
            file_hash: destination_hash,
            range: member.range,
        });
    }
    for proof in &mut record.opus_decoder_proofs {
        let Some(destination_id) = asset_map.get(&proof.asset_id) else {
            return Ok(None);
        };
        let Some(member) = mapped_members
            .iter()
            .find(|member| member.asset_id == *destination_id)
        else {
            return Ok(None);
        };
        if !member.file_hash.eq_ignore_ascii_case(&proof.file_hash) {
            return Ok(None);
        }
        proof.asset_id = destination_id.clone();
        proof.file_hash = member.file_hash.clone();
    }
    record.resource = match expected_scope {
        ResourceScope::Track if mapped_members.len() == 1 => LoudnessResourceIdentity::track(
            mapped_members[0].asset_id.clone(),
            mapped_members[0].file_hash.clone(),
            mapped_members[0].range,
        ),
        ResourceScope::Album => LoudnessResourceIdentity::album(
            mapped_members,
            record.resource.inter_track_gaps_seconds.clone(),
        ),
        _ => return Ok(None),
    };
    record.local_file_seals.clear();
    let raw = serde_json::to_string(&record).map_err(err)?;
    Ok(sanitize_loudness_analysis_payload(&raw, version))
}

fn materialize_restored_asset_payload(
    raw: &str,
    asset_id: &str,
    recording_id: &str,
    hash: &str,
    path: &str,
) -> Result<Value> {
    let mut payload = parse_bounded_json(raw)?;
    let object = payload
        .as_object_mut()
        .ok_or("Restored asset payload is invalid")?;
    object.insert("id".into(), Value::String(asset_id.to_owned()));
    object.insert("recordingId".into(), Value::String(recording_id.to_owned()));
    object.insert("hash".into(), Value::String(hash.to_owned()));
    object.insert("path".into(), Value::String(path.to_owned()));
    let track = object
        .get_mut("track")
        .filter(|track| track.is_object())
        .ok_or("Restored asset track is missing")?;
    let mut track = sanitize_track_value(track);
    if path.starts_with(MISSING_ASSET_PREFIX) {
        track["assetId"] = Value::Null;
        track["localPath"] = Value::Null;
        track["localAvailability"] = Value::String("none".into());
    } else {
        track["assetId"] = Value::String(asset_id.to_owned());
        track["localPath"] = Value::String(path.to_owned());
        track["localAvailability"] = Value::String("available".into());
    }
    object.insert("track".into(), track);
    let _: crate::models::LocalAsset = serde_json::from_value(payload.clone()).map_err(err)?;
    Ok(payload)
}

fn queue_track_recording_id(
    tx: &rusqlite::Transaction<'_>,
    track: &Value,
) -> Result<Option<String>> {
    let Some(track_id) = track.get("id").and_then(Value::as_str) else {
        return Ok(None);
    };
    tx.query_row(
        "SELECT COALESCE((SELECT recording_id FROM catalog_listings WHERE id=?1), (SELECT id FROM recordings WHERE id=?1))",
        [track_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .map_err(err)
}

fn materialize_restored_queue_track_asset(
    tx: &rusqlite::Transaction<'_>,
    track: &mut Value,
    asset_mapping: &HashMap<String, String>,
) -> Result<()> {
    let source_id = track
        .get("assetId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let registered = if let Some(asset_id) = source_id
        .as_deref()
        .and_then(|source_id| asset_mapping.get(source_id))
    {
        tx.query_row(
            "SELECT recording_id,path,hash FROM local_assets WHERE id=?1",
            [asset_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(err)?
    } else {
        None
    };
    let track_recording_id = queue_track_recording_id(tx, track)?;
    let Some((_, path, hash)) =
        registered.filter(|(recording_id, _, _)| Some(recording_id) == track_recording_id.as_ref())
    else {
        track["assetId"] = Value::Null;
        track["localPath"] = Value::Null;
        track["localAvailability"] = Value::String("none".into());
        return Ok(());
    };
    let canonical = if path.starts_with(MISSING_ASSET_PREFIX) || !valid_sha256(&hash) {
        None
    } else {
        std::fs::canonicalize(&path)
            .ok()
            .filter(|canonical| canonical.is_file() && file_matches_sha256(canonical, &hash))
    };
    if let Some(path) = canonical {
        let asset_id = source_id
            .as_deref()
            .and_then(|source_id| asset_mapping.get(source_id))
            .ok_or("Restored queue asset mapping disappeared")?;
        track["assetId"] = Value::String(asset_id.clone());
        track["localPath"] = Value::String(path.to_string_lossy().into_owned());
        track["localAvailability"] = Value::String("available".into());
    } else {
        track["assetId"] = Value::Null;
        track["localPath"] = Value::Null;
        track["localAvailability"] = Value::String("none".into());
    }
    Ok(())
}

fn merge_history(
    source_path: &Path,
    target_path: &Path,
    summary: &mut RestoreStateSummary,
) -> Result<()> {
    let source = open_archive_db(source_path)?;
    let target = Connection::open(target_path).map_err(err)?;
    target
        .execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON;")
        .map_err(err)?;
    if !is_plain_table(&source, "playback_history")? {
        return Err("Backup history table is invalid".into());
    }
    let tx = target.unchecked_transaction().map_err(err)?;
    let rows = read_rows(
        &source,
        "playback_history",
        &[
            "session_id",
            "played_at",
            "provider",
            "track_json",
            "chapter_json",
        ],
    )?;
    for row in rows {
        let track = parse_bounded_json(
            row.get("track_json")
                .and_then(sql_text)
                .ok_or("History track is missing")?,
        )?;
        let track = sanitize_track_value(&track);
        let track = serde_json::to_string(&track).map_err(err)?;
        let chapter = row
            .get("chapter_json")
            .and_then(sql_text)
            .map(|s| {
                parse_bounded_json(s).map(|v| {
                    serde_json::to_string(&sanitize_json(&v)).unwrap_or_else(|_| "null".into())
                })
            })
            .transpose()?;
        let inserted=tx.execute("INSERT OR IGNORE INTO playback_history(session_id,played_at,provider,track_json,chapter_json) VALUES(?1,?2,?3,?4,?5)",params![text(&row,"session_id"),text(&row,"played_at"),text(&row,"provider"),track,chapter]).map_err(err)?;
        if inserted == 1 {
            summary.imported_history_events += 1
        } else {
            summary.skipped_conflicts += 1
        }
    }
    tx.commit().map_err(err)?;
    verify_database(&target)
}

fn merge_settings_component(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    mode: RestoreMode,
    occurrence_id_mapping: &HashMap<String, String>,
    summary: &mut RestoreStateSummary,
) -> Result<()> {
    let archive: SettingsArchive = read_json(path)?;
    if archive.format != "org.burrata.settings" || archive.schema_version != SETTINGS_SCHEMA_VERSION
    {
        return Err("Unsupported settings archive schema".into());
    }
    summary.device_settings_skipped = summary
        .device_settings_skipped
        .saturating_add(archive.projection_summary.excluded_application_fields)
        .saturating_add(
            archive
                .projection_summary
                .asset_path_hints
                .max(archive.asset_path_hints.len()),
        );
    summary.private_settings_omitted = summary
        .private_settings_omitted
        .saturating_add(archive.projection_summary.omitted_private_settings);
    if archive.projection_summary.omitted_private_settings > 0 {
        summary.warnings.push(format!(
            "{} private settings were excluded when this backup was created.",
            archive.projection_summary.omitted_private_settings
        ));
    }
    if mode == RestoreMode::Replace {
        let settings = application_from_archive(&archive.application)?;
        tx.execute("INSERT INTO settings(key,value) VALUES('application',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(&settings).map_err(err)?]).map_err(err)?;
    } else {
        // Merge starts from the destination profile's sanitized projection.
        // Overlay only fields explicitly allowed for portable backup so the
        // imported appearance/playback choices apply while fields omitted from
        // the archive (and machine-local fields) remain destination-owned.
        let portable = portable_application_fields(&archive.application);
        if !portable.is_empty() {
            let current_json: Option<String> = tx
                .query_row(
                    "SELECT value FROM settings WHERE key='application'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(err)?;
            let mut current: Value = match current_json {
                Some(current_json) => serde_json::from_str(&current_json).map_err(err)?,
                // A valid first-run profile may rely on Settings::default()
                // without having persisted an `application` row yet.
                None => serde_json::to_value(Settings::default()).map_err(err)?,
            };
            let Some(current_fields) = current.as_object_mut() else {
                return Err("Current application settings are invalid".into());
            };
            for (key, value) in portable {
                current_fields.insert(key, value);
            }
            let settings = application_from_archive(&current)?;
            tx.execute(
                "INSERT INTO settings(key,value) VALUES('application',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [serde_json::to_string(&settings).map_err(err)?],
            )
            .map_err(err)?;
        }
    }
    for (key, value) in archive.preferences {
        if !is_preference_allowlisted(&key) {
            summary
                .warnings
                .push(format!("Ignored unsupported setting {key}"));
            continue;
        }
        let Some(value) = sanitize_preference(&key, value) else {
            continue;
        };
        let value = reconcile_preference_references(tx, &key, value, occurrence_id_mapping)?;
        let serialized = serde_json::to_string(&value).map_err(err)?;
        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO settings(key,value) VALUES(?1,?2)",
                params![key, serialized],
            )
            .map_err(err)?;
        if changed == 0 {
            summary.skipped_conflicts += 1;
        }
    }
    if !archive.frontend_preferences.is_empty() {
        let frontend = archive
            .frontend_preferences
            .into_iter()
            .filter_map(|(key, value)| sanitize_frontend_preference(&key, value).map(|v| (key, v)))
            .collect::<BTreeMap<_, _>>();
        if !frontend.is_empty() {
            let envelope = crate::backup_frontend_preferences::PendingEnvelope::new(frontend);
            let json = serde_json::to_string(&envelope).map_err(err)?;
            let sql = if mode == RestoreMode::Replace {
                "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value"
            } else {
                "INSERT OR IGNORE INTO settings(key,value) VALUES(?1,?2)"
            };
            tx.execute(
                sql,
                params![crate::backup_frontend_preferences::PENDING_KEY, json],
            )
            .map_err(err)?;
            summary
                .warnings
                .push("Portable WebView preferences are staged for frontend hydration.".into());
        }
    }
    Ok(())
}

fn reconcile_preference_references(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    mut value: Value,
    occurrence_id_mapping: &HashMap<String, String>,
) -> Result<Value> {
    let exists = |id: &str| -> Result<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(err)
    };
    match key {
        "playlistHierarchy.v1" => {
            if let Some(playlists) = value.get_mut("playlists").and_then(Value::as_array_mut) {
                for playlist in playlists {
                    if let Some(occurrences) = playlist
                        .get_mut("occurrences")
                        .and_then(Value::as_array_mut)
                    {
                        for occurrence in occurrences {
                            if let Some(id) = occurrence.get("id").and_then(Value::as_str) {
                                if let Some(mapped) = occurrence_id_mapping.get(id) {
                                    occurrence["id"] = Value::String(mapped.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        "playlistPreferences.v1" => {
            if let Some(object) = value.as_object_mut() {
                if let Some(id) = object.get("favoritesPlaylistId").and_then(Value::as_str) {
                    if !id.is_empty() && !exists(id)? {
                        object.insert("favoritesPlaylistId".into(), Value::String(String::new()));
                    }
                }
                if let Some(shortcuts) = object.get_mut("shortcuts").and_then(Value::as_array_mut) {
                    for slot in shortcuts {
                        if let Some(id) = slot.as_str().map(str::to_owned) {
                            if !exists(&id)? {
                                *slot = Value::Null;
                            }
                        }
                    }
                }
            }
        }
        "playlistMirrorOptions.v1" => {
            let Some(playlists) = value.get_mut("playlists").and_then(Value::as_object_mut) else {
                return Ok(value);
            };
            let mut remove = Vec::new();
            for id in playlists.keys() {
                if !matches!(id.as_str(), "favorites" | "history") && !exists(id)? {
                    remove.push(id.clone());
                }
            }
            for id in remove {
                playlists.remove(&id);
            }
        }
        "sidebarLayout.v1" => {
            if let Some(assignments) = value.get_mut("assignments").and_then(Value::as_object_mut) {
                let mut remove = Vec::new();
                for (playlist, _) in assignments.iter() {
                    if !exists(playlist)? {
                        remove.push(playlist.clone());
                    }
                }
                for playlist in remove {
                    assignments.remove(&playlist);
                }
            }
        }
        key if key.starts_with("radioPlaylistCapture.v1:") => {
            if let Some(id) = value
                .get("collectionId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                if !exists(&id)? {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("enabled".into(), Value::Bool(false));
                        object.insert("collectionId".into(), Value::Null);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(value)
}

fn merge_playback_component(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    summary: &mut RestoreStateSummary,
    asset_mapping: &HashMap<String, String>,
    listing_id_mapping: &HashMap<String, String>,
) -> Result<()> {
    let archive: PlaybackArchive = read_json(path)?;
    if archive.format != "org.burrata.playback" || archive.schema_version != PLAYBACK_SCHEMA_VERSION
    {
        return Err("Unsupported playback archive schema".into());
    }
    validate_playback(&archive)?;
    let tracks = archive
        .queue
        .iter()
        .map(|value| {
            let mut track = sanitize_track_value(value);
            if let Some(mapped_id) = track
                .get("id")
                .and_then(Value::as_str)
                .and_then(|source_id| listing_id_mapping.get(source_id))
                .cloned()
            {
                track["id"] = Value::String(mapped_id);
            }
            materialize_restored_queue_track_asset(tx, &mut track, asset_mapping)?;
            Ok(track)
        })
        .collect::<Result<Vec<_>>>()?;
    let queue_loudness_contexts = sanitize_queue_loudness_contexts(
        Some(&Value::Object(
            archive
                .queue_loudness_contexts
                .clone()
                .into_iter()
                .collect(),
        )),
        &archive.entry_ids,
    );
    let mut queue = serde_json::json!({"tracks":tracks,"entryIds":archive.entry_ids,"queueRevision":archive.queue_revision});
    if !queue_loudness_contexts.is_empty() {
        queue["queueLoudnessContexts"] =
            serde_json::to_value(&queue_loudness_contexts).map_err(err)?;
    }
    let session = serde_json::json!({"tracks":tracks,"entryIds":archive.entry_ids,"index":if archive.current_index<0{0}else{archive.current_index},"currentEntryId":archive.current_entry_id,"queueRevision":archive.queue_revision,"position":archive.position_seconds,"volume":archive.volume,"repeat":archive.repeat,"shuffle":archive.shuffle,"finished":false});
    tx.execute(
        "INSERT OR IGNORE INTO settings(key,value) VALUES('nativePlaybackQueue.v1',?1)",
        [serde_json::to_string(&queue).map_err(err)?],
    )
    .map_err(err)?;
    tx.execute(
        "INSERT OR IGNORE INTO settings(key,value) VALUES('nativePlaybackSession.v1',?1)",
        [serde_json::to_string(&session).map_err(err)?],
    )
    .map_err(err)?;
    // Restored playback is always paused; engine selection is not imported.
    summary.warnings.push(
        "The saved queue was restored paused; review the output device before resuming.".into(),
    );
    Ok(())
}

/// Retain the destination queue for Merge when the user did not select the
/// archive's playback component. The recovery checkpoint is already a
/// sanitized PlaybackArchive; rebind only asset IDs that still exist in the
/// staged current library, and verify their files before exposing a local path.
fn preserve_current_playback_component(tx: &rusqlite::Transaction<'_>, path: &Path) -> Result<()> {
    let archive: PlaybackArchive = read_json(path)?;
    if archive.format != "org.burrata.playback" || archive.schema_version != PLAYBACK_SCHEMA_VERSION
    {
        return Err("Unsupported current playback snapshot schema".into());
    }
    validate_playback(&archive)?;

    let tracks = archive
        .queue
        .iter()
        .map(|value| {
            let mut track = sanitize_track_value(value);
            let Some(asset_id) = track
                .get("assetId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                track["assetId"] = Value::Null;
                track["localPath"] = Value::Null;
                track["localAvailability"] = Value::String("none".into());
                return Ok(track);
            };
            let track_recording_id = queue_track_recording_id(tx, &track)?;
            let registered: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT recording_id,path,hash FROM local_assets WHERE id=?1",
                    [&asset_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(err)?;
            let Some((_, path, hash)) = registered
                .filter(|(recording_id, _, _)| Some(recording_id) == track_recording_id.as_ref())
            else {
                track["assetId"] = Value::Null;
                track["localPath"] = Value::Null;
                track["localAvailability"] = Value::String("none".into());
                return Ok(track);
            };

            let verified_path = if !path.starts_with(MISSING_ASSET_PREFIX) && valid_sha256(&hash) {
                std::fs::canonicalize(&path).ok().filter(|canonical| {
                    canonical.is_file() && file_matches_sha256(canonical, &hash)
                })
            } else {
                None
            };
            track["assetId"] = Value::String(asset_id);
            if let Some(path) = verified_path {
                track["localPath"] = Value::String(path.to_string_lossy().into_owned());
                track["localAvailability"] = Value::String("available".into());
            } else {
                // Keep the correctly registered association so later relinking
                // can identify this queue item, but never expose an old,
                // missing, or hash-mismatched path as playable.
                track["localPath"] = Value::Null;
                track["localAvailability"] = Value::String("none".into());
            }
            Ok(track)
        })
        .collect::<Result<Vec<_>>>()?;
    let queue_loudness_contexts = sanitize_queue_loudness_contexts(
        Some(&Value::Object(
            archive
                .queue_loudness_contexts
                .clone()
                .into_iter()
                .collect(),
        )),
        &archive.entry_ids,
    );
    let mut queue = serde_json::json!({
        "tracks": tracks,
        "entryIds": archive.entry_ids,
        "queueRevision": archive.queue_revision
    });
    if !queue_loudness_contexts.is_empty() {
        queue["queueLoudnessContexts"] =
            serde_json::to_value(&queue_loudness_contexts).map_err(err)?;
    }
    let session = serde_json::json!({
        "tracks": tracks,
        "entryIds": archive.entry_ids,
        "index": if archive.current_index < 0 { 0 } else { archive.current_index },
        "currentEntryId": archive.current_entry_id,
        "queueRevision": archive.queue_revision,
        "position": archive.position_seconds,
        "volume": archive.volume,
        "repeat": archive.repeat,
        "shuffle": archive.shuffle,
        "finished": false
    });
    for (key, value) in [
        ("nativePlaybackQueue.v1", queue),
        ("nativePlaybackSession.v1", session),
    ] {
        tx.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(&value).map_err(err)?],
        )
        .map_err(err)?;
    }
    Ok(())
}

fn read_rows(
    conn: &Connection,
    table: &str,
    allowlist: &[&str],
) -> Result<Vec<HashMap<String, SqlValue>>> {
    if !table_exists(conn, table)? {
        return Ok(Vec::new());
    }
    if !is_plain_table(conn, table)? {
        return Err(format!("Backup object {table} is not an ordinary table"));
    }
    let available = table_columns(conn, table)?;
    let columns = allowlist
        .iter()
        .filter(|c| available.contains(**c))
        .copied()
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!("SELECT {} FROM {}", quoted_columns(&columns), table);
    let mut stmt = conn.prepare(&sql).map_err(err)?;
    let mut rows = stmt.query([]).map_err(err)?;
    let mut output = Vec::new();
    while let Some(row) = rows.next().map_err(err)? {
        if output.len() >= MAX_ROWS_PER_TABLE {
            return Err(format!("Backup table {table} exceeds row limit"));
        }
        let mut item = HashMap::new();
        for (i, col) in columns.iter().enumerate() {
            item.insert((*col).to_owned(), row.get(i).map_err(err)?);
        }
        output.push(item)
    }
    Ok(output)
}

fn insert_values(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    columns: &[&str],
    row: &HashMap<String, SqlValue>,
    ignore: bool,
) -> Result<bool> {
    if columns.is_empty() {
        return Ok(false);
    }
    let cols = quoted_columns(columns);
    let binds = (1..=columns.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT {} INTO {} ({cols}) VALUES ({binds})",
        if ignore { "OR IGNORE" } else { "" },
        table
    );
    let values = columns
        .iter()
        .map(|c| {
            row.get(*c)
                .cloned()
                .ok_or_else(|| "Backup row omitted an allowlisted column".to_string())
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(tx.execute(&sql, params_from_iter(values)).map_err(err)? == 1)
}

fn count_library(conn: &Connection, summary: &mut BackupStateSummary) -> Result<()> {
    summary.recordings = count(conn, "recordings")?;
    summary.catalog_listings = count(conn, "catalog_listings")?;
    summary.collections = count(conn, "collections")?;
    summary.assets = count(conn, "local_assets")?;
    summary.playlist_occurrences = count(conn, "playlist_ordered_entries")?;
    Ok(())
}
fn count(conn: &Connection, table: &str) -> Result<usize> {
    if !table_exists(conn, table)? {
        return Ok(0);
    }
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n.max(0) as usize)
    .map_err(err)
}
fn delete_rows_if_table_exists(conn: &Connection, table: &str) -> Result<usize> {
    if !table_exists(conn, table)? {
        return Ok(0);
    }
    Ok(conn
        .execute(&format!("DELETE FROM {table}"), [])
        .map_err(err)?)
}
fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
    .map_err(err)
}
fn object_exists(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
        [name],
        |r| r.get(0),
    )
    .map_err(err)
}
fn is_plain_table(conn: &Connection, table: &str) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_list").map_err(err)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(err)?;
    for row in rows {
        let (name, kind) = row.map_err(err)?;
        if name == table {
            return Ok(kind == "table");
        }
    }
    Ok(false)
}
fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    if !table_exists(conn, table)? {
        return Ok(HashSet::new());
    }
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql).map_err(err)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(err)?;
    rows.collect::<rusqlite::Result<HashSet<_>>>().map_err(err)
}
fn quoted_columns(cols: &[&str]) -> String {
    cols.iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",")
}
fn read_preference(conn: &Connection, key: &str) -> Result<Option<Value>> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(err)?
    .map(|s| parse_bounded_json(&s))
    .transpose()
}
fn text<'a>(row: &'a HashMap<String, SqlValue>, key: &str) -> Option<&'a String> {
    match row.get(key) {
        Some(SqlValue::Text(s)) => Some(s),
        _ => None,
    }
}
fn sql_text(value: &SqlValue) -> Option<&str> {
    if let SqlValue::Text(s) = value {
        Some(s)
    } else {
        None
    }
}
fn sql_integer(value: &SqlValue) -> Option<i64> {
    if let SqlValue::Integer(n) = value {
        Some(*n)
    } else {
        None
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn is_preference_allowlisted(key: &str) -> bool {
    PREFERENCE_KEYS.contains(&key)
        || key.starts_with("radioPlaylistCapture.v1:")
        || lyrics_choice_scope_from_key(key).is_some()
}
fn sanitize_download_watches(value: Value) -> Option<Value> {
    let items = value.as_array()?;
    if items.len() > 500 {
        return None;
    }
    let items = items
        .iter()
        .filter_map(|item| {
            // Typed validation strips unknown fields. Copies never resume automatic
            // transfers against the source computer's destination or in-flight job.
            let item: crate::acquisition::DownloadWatch =
                serde_json::from_value(item.clone()).ok()?;
            let mut value = serde_json::to_value(item).ok()?;
            let track = sanitize_track_value(&value["request"]["track"]);
            value["request"]["track"] = track;
            value["musicDirectory"] = serde_json::json!("");
            value["jobId"] = Value::Null;
            if !matches!(value["state"].as_str(), Some("completed" | "cancelled")) {
                value["state"] = serde_json::json!("paused");
            }
            value["revision"] =
                serde_json::json!(value["revision"].as_u64().unwrap_or(0).saturating_add(1));
            Some(value)
        })
        .collect();
    Some(Value::Array(items))
}

fn sanitize_preference(key: &str, value: Value) -> Option<Value> {
    match key{
    "soulseekDownloadWatches.v1"=>sanitize_download_watches(value),
    "playlistPreferences.v1"=>sanitize_playlist_preferences(value),
    "playlistMirrorOptions.v1"=>sanitize_playlist_mirror_options(value),
    "playlistHierarchy.v1"=>sanitize_playlist_hierarchy(value),
    "playlistSync.v1"=>sanitize_playlist_sync(value),
    "sidebarLayout.v1"=>sanitize_sidebar_layout(value),
    "radioStations.v1"=>sanitize_radio_stations(value),
    "hiddenRadioStations.v1"=>value.as_array().map(|a|Value::Array(a.iter().filter_map(|v|valid_id(v.as_str()?).then(||v.clone())).collect())),
    "radioStationNames.v1"=>value.as_object().map(|m|Value::Object(m.iter().filter(|(k,v)|valid_id(k)&&v.as_str().is_some_and(|s|safe_text(s,120))).map(|(k,v)|(k.clone(),v.clone())).collect())),
    "radioHistory.v1"=>sanitize_radio_history(value),
    "includeRadioSongsInPlaybackHistory.v1"|"radioRecognitionEnabled.v1"=>value.as_bool().map(Value::Bool),
    "loudness.v1"=>sanitize_loudness_preference(value),
    "streamAnalysisCache.v1"=>sanitize_stream_analysis_cache(value),
    // Full-profile backups retain bounded per-source observations, but an
    // imported archive must never opt a resource back into recognition.
    "playingTrackDetectionCache.v1"=>crate::playing_track_detections::sanitize_backup_value(value),
    "playingTrackDetectionPolicy.v1"=>crate::playing_track_detections::sanitize_policy_backup_value(value),
    "playlistHistoryMirror.v1"=>sanitize_history_mirror_options(value),
    "backupSchedule.v1"=>sanitize_backup_schedule(value),
    "autotag.settings"=>value.as_object().map(|m|{let allowed=["enabled","embedArtwork","embedLyrics","embedReplayGain","overwriteExisting"];Value::Object(m.iter().filter(|(k,v)|allowed.contains(&k.as_str())&&v.is_boolean()).map(|(k,v)|(k.clone(),v.clone())).collect())}),
    key if key.starts_with("radioPlaylistCapture.v1:")=>value.as_object().map(|m|serde_json::json!({"enabled":m.get("enabled").and_then(Value::as_bool).unwrap_or(false),"collectionId":m.get("collectionId").and_then(Value::as_str).filter(|s|valid_id(s)).map(str::to_owned)})),
    key if lyrics_choice_scope_from_key(key).is_some()=>sanitize_lyrics_choice(value),
    _=>None}
}

fn lyrics_choice_scope_from_key(key: &str) -> Option<&str> {
    let scope = key.strip_prefix("lyrics.choice.v1.")?;
    (scope.len() == 64
        && scope
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(scope)
}

/// Keep only the explicit LRCLIB selection. Provider URLs are reconstructed
/// from a canonical numeric record ID, never copied from the stored candidate.
fn sanitize_lyrics_choice(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    if object
        .get("schemaVersion")
        .is_some_and(|value| value.as_u64() != Some(1))
        || object
            .get("provider")
            .is_some_and(|value| value.as_str() != Some("lrclib"))
    {
        return None;
    }
    let candidate = object.get("candidate")?.as_object()?;
    let record = object.get("record")?.as_object()?;
    let id = candidate.get("id")?.as_str()?;
    let numeric_id = id.parse::<u64>().ok()?;
    if numeric_id == 0 || numeric_id.to_string() != id || id.len() > 32 {
        return None;
    }

    let safe_metadata = |object: &serde_json::Map<String, Value>, key: &str| -> Option<String> {
        let value = object.get(key)?.as_str()?;
        safe_text(value, 512).then(|| value.to_owned())
    };
    let optional_metadata =
        |object: &serde_json::Map<String, Value>, key: &str| -> Option<Option<String>> {
            match object.get(key) {
                None | Some(Value::Null) => Some(None),
                Some(Value::String(value)) if safe_text(value, 512) => Some(Some(value.to_owned())),
                _ => None,
            }
        };
    let track_name = safe_metadata(record, "trackName")?;
    let artist_name = safe_metadata(record, "artistName")?;
    let album_name = optional_metadata(record, "albumName")?;
    if safe_metadata(candidate, "trackName")? != track_name
        || safe_metadata(candidate, "artistName")? != artist_name
        || optional_metadata(candidate, "albumName")? != album_name
    {
        return None;
    }
    let duration = record.get("duration")?.as_f64()?;
    let candidate_duration = candidate.get("durationSeconds")?.as_f64()?;
    if !duration.is_finite()
        || duration <= 0.0
        || duration > 86_400.0
        || !candidate_duration.is_finite()
        || (duration - candidate_duration).abs() > 0.000_001
        || record.get("instrumental")?.as_bool()?
    {
        return None;
    }
    let safe_body = |key: &str| -> Option<Option<String>> {
        match record.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(body)) if valid_lyrics_backup_body(body) => Some(Some(body.clone())),
            _ => None,
        }
    };
    let synced_lyrics = safe_body("syncedLyrics")?;
    let plain_lyrics = safe_body("plainLyrics")?;
    if synced_lyrics.is_none() && plain_lyrics.is_none() {
        return None;
    }
    let has_synced = synced_lyrics.is_some();
    if candidate.get("hasSynced")?.as_bool()? != has_synced {
        return None;
    }
    let automatic_match = candidate.get("automaticMatch")?.as_bool()?;
    let plain_text_match = candidate.get("plainTextMatch")?.as_bool()?;
    if automatic_match && plain_text_match {
        return None;
    }
    let explanation = if automatic_match {
        "Recording metadata or verified artist alias matches"
    } else if plain_text_match {
        "Same song and performer; displayed without timing for this version"
    } else {
        "Related lyrics; performance or artist credit requires your choice"
    };
    Some(serde_json::json!({
        "schemaVersion": 1,
        "provider": "lrclib",
        "candidate": {
            "id": id,
            "trackName": track_name,
            "artistName": artist_name,
            "albumName": album_name,
            "durationSeconds": duration,
            "hasSynced": has_synced,
            "automaticMatch": automatic_match,
            "plainTextMatch": plain_text_match,
            "explanation": explanation,
            "sourceUrl": format!("https://lrclib.net/api/get/{id}"),
        },
        "record": {
            "trackName": track_name,
            "artistName": artist_name,
            "albumName": album_name,
            "duration": duration,
            "instrumental": false,
            "syncedLyrics": synced_lyrics,
            "plainLyrics": plain_lyrics,
        }
    }))
}

fn valid_lyrics_backup_body(body: &str) -> bool {
    !body.trim().is_empty()
        && body.len() <= MAX_LYRIC_CHOICE_BODY_BYTES
        && body.lines().count() <= MAX_LYRIC_CHOICE_LINES
        && !body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn sanitize_playlist_hierarchy(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.get("schemaVersion")?.as_u64()? != 1 {
        return None;
    }
    let folders = object.get("folders")?.as_array()?;
    let playlists = object.get("playlists")?.as_array()?;
    if folders.len() > 10_000 || playlists.len() > 10_000 {
        return None;
    }
    let safe_items = |items: &[Value], folder: bool| -> Vec<Value> {
        items
            .iter()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?;
                let name = item.get("name")?.as_str()?;
                let parent = item.get("parentId").and_then(Value::as_str);
                let position = item.get("position").and_then(Value::as_u64)?;
                if !valid_id(id)
                    || !safe_hierarchy_name(name)
                    || position > u64::from(u32::MAX)
                    || (folder && parent.is_some_and(|value| !valid_id(value)))
                {
                    return None;
                }
                let mut result = serde_json::json!({"id":id,"name":name,"position":position});
                if folder {
                    if let Some(parent) = parent {
                        result["parentId"] = Value::String(parent.into());
                    }
                }
                Some(result)
            })
            .collect()
    };
    let safe_folders = safe_items(folders, true);
    let safe_playlists = playlists.iter().filter_map(|playlist| {
        let collection_id = playlist.get("collectionId")?.as_str()?;
        let position = playlist.get("position")?.as_u64()?;
        let folder_id = playlist.get("folderId").and_then(Value::as_str);
        let sections = playlist.get("sections")?.as_array()?;
        let occurrences = playlist.get("occurrences")?.as_array()?;
        if !valid_id(collection_id) || position > u64::from(u32::MAX)
            || folder_id.is_some_and(|id| !valid_id(id))
            || sections.len() > 10_000
        { return None; }
        let safe_sections = sections.iter().filter_map(|section| {
            let id = section.get("id")?.as_str()?;
            let name = section.get("name")?.as_str()?;
            let position = section.get("position")?.as_u64()?;
            (valid_id(id) && safe_hierarchy_name(name) && position <= u64::from(u32::MAX)).then(|| serde_json::json!({"id":id,"name":name,"position":position}))
        }).collect::<Vec<_>>();
        let safe_occurrences = occurrences.iter().filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            if !valid_id(id) { return None; }
            let section_id = entry.get("sectionId").and_then(Value::as_str);
            if section_id.is_some_and(|id| !valid_id(id)) { return None; }
            let mut safe = serde_json::json!({"id":id});
            if let Some(section_id) = section_id { safe["sectionId"] = Value::String(section_id.into()); }
            if let Some(note) = entry.get("note").and_then(Value::as_str).filter(|s| valid_backup_note(s)) {
                safe["note"] = Value::String(note.into());
            }
            if let Some(points) = entry.get("cuePoints").and_then(Value::as_array) {
                // Hierarchy storage accepts finite signed cue values. Preserve
                // every JSON number; portable-share validation is separate.
                let points = points.iter().filter_map(Value::as_f64).collect::<Vec<_>>();
                safe["cuePoints"] = serde_json::json!(points);
            }
            if let Some(range) = entry.get("playbackRange").and_then(Value::as_object) {
                let start = range.get("startSeconds").and_then(Value::as_f64);
                let end = range.get("endSeconds").and_then(Value::as_f64);
                if start.is_some_and(f64::is_finite) && end.is_some_and(f64::is_finite) {
                    safe["playbackRange"] = sanitize_json(&Value::Object(range.to_owned()));
                }
            }
            Some(safe)
        }).collect::<Vec<_>>();
        let mut safe = serde_json::json!({"collectionId":collection_id,"position":position,"sections":safe_sections,"occurrences":safe_occurrences});
        if let Some(folder_id) = folder_id { safe["folderId"] = Value::String(folder_id.into()); }
        Some(safe)
    }).collect::<Vec<_>>();
    Some(serde_json::json!({"schemaVersion":1,"folders":safe_folders,"playlists":safe_playlists}))
}

/// Preserve playlist mirror identity and edit snapshots as inert data. Local
/// paths become basename-only relink hints, and snapshot M3U content is
/// reserialized through a path-free private portable format before export.
fn sanitize_playlist_sync(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.contains_key("backupProjection") {
        return sanitize_playlist_sync_projection(value);
    }
    let mut sources = Vec::new();
    if let Some(map) = object.get("sources").and_then(Value::as_object) {
        for (source_path, state) in map {
            let state = state.as_object()?;
            let binding_id = format!(
                "b-{}",
                &encode_hex(Sha256::digest(source_path.as_bytes()))[..24]
            );
            let mut item = serde_json::json!({"bindingId":binding_id,"snapshots":sanitize_source_snapshots(state.get("snapshots"))});
            for (input, output) in [
                ("collectionId", "collectionId"),
                ("personalCollectionId", "personalCollectionId"),
            ] {
                if let Some(id) = state
                    .get(input)
                    .and_then(Value::as_str)
                    .filter(|id| valid_id(id))
                {
                    item[output] = Value::String(id.into());
                }
            }
            if let Some(name) = state
                .get("originalName")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= MAX_JSON_BYTES)
            {
                item["originalName"] = Value::String(name.into());
            }
            for (input, output) in [
                ("lastSourceHash", "sourceHash"),
                ("lastMirrorHash", "mirrorHash"),
            ] {
                if let Some(hash) = state
                    .get(input)
                    .and_then(Value::as_str)
                    .filter(|s| valid_sha256(s))
                {
                    item[output] = Value::String(hash.to_ascii_lowercase());
                }
            }
            if let Some(name) = safe_path_basename(source_path) {
                item["relinkHint"] = serde_json::json!({"basename":name});
            }
            sources.push(item);
        }
    }
    let mut mirrors = Vec::new();
    if let Some(hashes) = object.get("mirrorHashes").and_then(Value::as_object) {
        for (collection_id, hash) in hashes {
            if !valid_id(collection_id) || parse_section_mirror_key(collection_id).is_some() {
                continue;
            }
            let mut item = serde_json::json!({"collectionId":collection_id});
            if let Some(hash) = hash.as_str().filter(|s| valid_sha256(s)) {
                item["hash"] = Value::String(hash.to_ascii_lowercase());
            }
            if let Some(snapshots) = object
                .get("mirrorSnapshots")
                .and_then(Value::as_object)
                .and_then(|map| map.get(collection_id))
            {
                item["snapshots"] = Value::Array(sanitize_source_snapshots(Some(snapshots)));
            }
            if let Some(path) = object
                .get("mirrorPaths")
                .and_then(Value::as_object)
                .and_then(|map| map.get(collection_id))
                .and_then(Value::as_str)
                .and_then(safe_path_basename)
            {
                item["relinkHint"] = serde_json::json!({"basename":path});
            }
            mirrors.push(item);
        }
    }
    let mut section_mirrors = Vec::new();
    let mut section_ids = std::collections::HashSet::new();
    if let Some(hashes) = object.get("mirrorHashes").and_then(Value::as_object) {
        for (key, hash) in hashes {
            let Some((collection_id, section_id)) = parse_section_mirror_key(key) else {
                continue;
            };
            let Some(hash) = hash.as_str().filter(|hash| valid_sha256(hash)) else {
                continue;
            };
            if !section_ids.insert((collection_id.to_owned(), section_id.to_owned())) {
                continue;
            }
            let mut item = serde_json::json!({
                "collectionId": collection_id,
                "sectionId": section_id,
                "hash": hash.to_ascii_lowercase(),
            });
            let snapshot_key = section_mirror_snapshot_key(collection_id, section_id);
            if let Some(snapshots) = object
                .get("mirrorSnapshots")
                .and_then(Value::as_object)
                .and_then(|map| map.get(&snapshot_key))
            {
                item["snapshots"] = Value::Array(sanitize_source_snapshots(Some(snapshots)));
            }
            if let Some(path) = object
                .get("mirrorPaths")
                .and_then(Value::as_object)
                .and_then(|map| map.get(key))
                .and_then(Value::as_str)
                .and_then(safe_path_basename)
                .filter(|name| safe_section_basename_hint(name))
            {
                item["relinkHint"] = serde_json::json!({"basename":path});
            }
            section_mirrors.push(item);
        }
    }
    section_mirrors.sort_by(|left, right| {
        left["collectionId"]
            .as_str()
            .cmp(&right["collectionId"].as_str())
            .then_with(|| left["sectionId"].as_str().cmp(&right["sectionId"].as_str()))
    });
    let protected_copies = object.get("protectedCopies").and_then(Value::as_object).map(|map| map.iter().filter_map(|(path, value)| {
        let personal_collection_id = value.as_str().filter(|id| valid_id(id))?;
        let mut entry = serde_json::json!({"bindingId":format!("b-{}", &encode_hex(Sha256::digest(path.as_bytes()))[..24]),"personalCollectionId":personal_collection_id,"protected":true});
        if let Some(name) = safe_path_basename(path) { entry["relinkHint"] = serde_json::json!({"basename":name}); }
        Some(entry)
    }).collect::<Vec<_>>()).unwrap_or_default();
    Some(serde_json::json!({
        "sources": {}, "mirrorHashes": {}, "mirrorPaths": {}, "mirrorSnapshots": {}, "protectedCopies": {},
        "writesPaused": true,
        "backupProjection": {"schemaVersion":1,"writesPaused":true,"sources":sources,"mirrors":mirrors,"sectionMirrors":section_mirrors,"protectedCopies":protected_copies}
    }))
}

fn sanitize_playlist_sync_projection(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    let projection = object.get("backupProjection")?.as_object()?;
    let safe_bindings = |field: &str| -> Vec<Value> {
        projection
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.as_object()?;
                let id = entry.get("bindingId").and_then(Value::as_str)?;
                if !valid_id(id) { return None; }
                let mut safe = serde_json::json!({"bindingId":id,"snapshots":sanitize_projected_snapshots(entry.get("snapshots"))});
                for key in ["collectionId", "personalCollectionId"] {
                    if let Some(value) = entry.get(key).and_then(Value::as_str).filter(|s| valid_id(s)) { safe[key] = Value::String(value.into()); }
                }
                for key in ["sourceHash", "mirrorHash"] {
                    if let Some(value) = entry.get(key).and_then(Value::as_str).filter(|s| valid_sha256(s)) { safe[key] = Value::String(value.to_ascii_lowercase()); }
                }
                if let Some(name) = entry.get("originalName").and_then(Value::as_str).filter(|s| s.len() <= MAX_JSON_BYTES) { safe["originalName"] = Value::String(name.into()); }
                if let Some(name) = entry.get("relinkHint").and_then(|v| v.get("basename")).and_then(Value::as_str).filter(|s| safe_path_component_hint(s)) { safe["relinkHint"] = serde_json::json!({"basename":name}); }
                Some(safe)
            })
            .collect()
    };
    let sources = safe_bindings("sources");
    let mut mirrors = Vec::new();
    let mut section_entries = HashMap::<(String, String), Vec<Value>>::new();
    if let Some(items) = projection.get("mirrors").and_then(Value::as_array) {
        for entry in items {
            let entry = entry.as_object()?;
            let id = entry.get("collectionId").and_then(Value::as_str)?;
            if !valid_id(id) {
                continue;
            }
            if let Some((collection_id, section_id)) = parse_section_mirror_key(id) {
                if let Some(hash) = entry
                    .get("hash")
                    .and_then(Value::as_str)
                    .filter(|hash| valid_sha256(hash))
                {
                    let mut safe = serde_json::json!({
                        "collectionId": collection_id,
                        "sectionId": section_id,
                        "hash": hash.to_ascii_lowercase(),
                        "snapshots": sanitize_projected_snapshots(entry.get("snapshots")),
                    });
                    if let Some(name) = entry
                        .get("relinkHint")
                        .and_then(|value| value.get("basename"))
                        .and_then(Value::as_str)
                        .filter(|name| safe_section_basename_hint(name))
                    {
                        safe["relinkHint"] = serde_json::json!({"basename":name});
                    }
                    section_entries
                        .entry((collection_id.to_owned(), section_id.to_owned()))
                        .or_default()
                        .push(safe);
                }
                continue;
            }
            let mut safe = serde_json::json!({"collectionId":id,"snapshots":sanitize_projected_snapshots(entry.get("snapshots"))});
            if let Some(hash) = entry
                .get("hash")
                .and_then(Value::as_str)
                .filter(|s| valid_sha256(s))
            {
                safe["hash"] = Value::String(hash.to_ascii_lowercase());
            }
            if let Some(name) = entry
                .get("relinkHint")
                .and_then(|v| v.get("basename"))
                .and_then(Value::as_str)
                .filter(|s| safe_path_component_hint(s))
            {
                safe["relinkHint"] = serde_json::json!({"basename":name});
            }
            mirrors.push(safe);
        }
    }
    if let Some(items) = projection.get("sectionMirrors").and_then(Value::as_array) {
        for entry in items {
            let Some(collection_id) = entry.get("collectionId").and_then(Value::as_str) else {
                continue;
            };
            let Some(section_id) = entry.get("sectionId").and_then(Value::as_str) else {
                continue;
            };
            let Some(hash) = entry
                .get("hash")
                .and_then(Value::as_str)
                .filter(|hash| valid_sha256(hash))
            else {
                continue;
            };
            if !valid_section_mirror_identity(collection_id, section_id) {
                continue;
            }
            let mut safe = serde_json::json!({
                "collectionId": collection_id,
                "sectionId": section_id,
                "hash": hash.to_ascii_lowercase(),
                "snapshots": sanitize_projected_snapshots(entry.get("snapshots")),
            });
            if let Some(name) = entry
                .get("relinkHint")
                .and_then(|value| value.get("basename"))
                .and_then(Value::as_str)
                .filter(|name| safe_section_basename_hint(name))
            {
                safe["relinkHint"] = serde_json::json!({"basename":name});
            }
            section_entries
                .entry((collection_id.to_owned(), section_id.to_owned()))
                .or_default()
                .push(safe);
        }
    }
    // Conflicting legacy and typed entries for the same section are ambiguous;
    // leave that section unbound rather than choosing an archive record.
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
    let protected_copies = projection.get("protectedCopies").and_then(Value::as_array).into_iter().flatten().filter_map(|entry| {
        let id = entry.get("bindingId")?.as_str()?;
        let collection = entry.get("personalCollectionId")?.as_str()?;
        if !valid_id(id) || !valid_id(collection) { return None; }
        let mut safe = serde_json::json!({"bindingId":id,"personalCollectionId":collection,"protected":true});
        if let Some(basename) = entry.get("relinkHint").and_then(|v| v.get("basename")).and_then(Value::as_str).filter(|s| safe_path_component_hint(s)) { safe["relinkHint"] = serde_json::json!({"basename":basename}); }
        Some(safe)
    }).collect::<Vec<_>>();
    Some(serde_json::json!({
        "sources": {}, "mirrorHashes": {}, "mirrorPaths": {}, "mirrorSnapshots": {}, "protectedCopies": {},
        "writesPaused": true,
        "backupProjection": {"schemaVersion":1,"writesPaused":true,"sources":sources,"mirrors":mirrors,"sectionMirrors":section_mirrors,"protectedCopies":protected_copies}
    }))
}

fn parse_section_mirror_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("section:")?;
    let (collection_id, section_id) = rest.split_once(':')?;
    if section_id.contains(':') || !valid_section_mirror_identity(collection_id, section_id) {
        return None;
    }
    Some((collection_id, section_id))
}

fn valid_section_mirror_identity(collection_id: &str, section_id: &str) -> bool {
    [collection_id, section_id].into_iter().all(|id| {
        valid_id(id)
            && id != "."
            && id != ".."
            && !id.contains([':', '/', '\\'])
            && !looks_like_absolute_path(id)
            && !looks_like_url(id)
    })
}

fn safe_section_basename_hint(name: &str) -> bool {
    safe_path_component_hint(name)
        && !name.contains(['/', '\\', ':'])
        && name.to_ascii_lowercase().ends_with(".m3u8")
}

fn section_mirror_snapshot_key(collection_id: &str, section_id: &str) -> String {
    format!("section__{collection_id}__{section_id}")
}

fn sanitize_projected_snapshots(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(|snapshot| {
        let source_hash = snapshot.get("sourceOriginalSha256")?.as_str()?;
        let captured_at = snapshot.get("capturedAt")?.as_str()?;
        let contents = snapshot.get("contents")?.as_str()?;
        if !valid_sha256(source_hash) || captured_at.len() > 64 || contents.len() > 2 * 1024 * 1024 { return None; }
        let parsed = crate::portable_playlist::parse(contents).ok()?;
        let contents = crate::portable_playlist::serialize(&parsed, &private_snapshot_export_options()).ok()?;
        Some(serde_json::json!({"sourceOriginalSha256":source_hash.to_ascii_lowercase(),"contentsSha256":encode_hex(Sha256::digest(contents.as_bytes())),"capturedAt":captured_at,"contents":contents}))
    }).collect()
}

fn sanitize_source_snapshots(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|snapshot| {
            let revision = snapshot.get("revision")?.as_str()?;
            let captured_at = snapshot.get("capturedAt")?.as_str()?;
            let contents = snapshot.get("contents")?.as_str()?;
            if !valid_sha256(revision) || captured_at.len() > 64 || contents.len() > 2 * 1024 * 1024
            {
                return None;
            }
            let parsed = crate::portable_playlist::parse(contents).ok()?;
            let contents =
                crate::portable_playlist::serialize(&parsed, &private_snapshot_export_options())
                    .ok()?;
            Some(serde_json::json!({
                "sourceOriginalSha256":revision.to_ascii_lowercase(),
                "contentsSha256":encode_hex(Sha256::digest(contents.as_bytes())),
                "capturedAt":captured_at,
                "contents":contents
            }))
        })
        .collect()
}

/// Sync snapshots are private backup data, not shared playlist exports. Keep
/// annotations and user tags while Share purpose continues to replace local
/// resource lines with inert identity placeholders.
fn private_snapshot_export_options() -> crate::portable_playlist::PlaylistExportOptions {
    crate::portable_playlist::PlaylistExportOptions {
        purpose: crate::portable_playlist::PlaylistPurpose::Share,
        include_notes: true,
        include_tags: true,
        include_ratings: true,
        ..Default::default()
    }
}

/// Validate every saved sync snapshot before projecting it. The caller fails
/// backup preparation with a clear error rather than silently omitting a
/// snapshot that is beyond the portable format's supported bounds.
fn validate_playlist_sync_snapshot_limits(value: &Value) -> Result<()> {
    let mut contents = Vec::<&str>::new();
    if let Some(projection) = value.get("backupProjection").and_then(Value::as_object) {
        for field in ["sources", "mirrors", "sectionMirrors"] {
            for entry in projection
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(snapshots) = entry.get("snapshots").and_then(Value::as_array) {
                    for snapshot in snapshots {
                        contents.push(
                            snapshot
                                .get("contents")
                                .and_then(Value::as_str)
                                .ok_or("Playlist sync snapshot is missing its contents")?,
                        );
                    }
                }
            }
        }
    } else if let Some(sources) = value.get("sources").and_then(Value::as_object) {
        for source in sources.values() {
            if let Some(snapshots) = source.get("snapshots").and_then(Value::as_array) {
                for snapshot in snapshots {
                    contents.push(
                        snapshot
                            .get("contents")
                            .and_then(Value::as_str)
                            .ok_or("Playlist sync snapshot is missing its contents")?,
                    );
                }
            }
        }
        if let Some(mirrors) = value.get("mirrorSnapshots").and_then(Value::as_object) {
            for snapshots in mirrors.values().filter_map(Value::as_array) {
                for snapshot in snapshots {
                    contents.push(
                        snapshot
                            .get("contents")
                            .and_then(Value::as_str)
                            .ok_or("Playlist mirror snapshot is missing its contents")?,
                    );
                }
            }
        }
    }
    for content in contents {
        if content.len() > 2 * 1024 * 1024 {
            return Err("A playlist sync snapshot exceeds the 2 MiB format limit; backup stopped without omitting it".into());
        }
        crate::portable_playlist::parse(content).map_err(|error| {
            format!("A playlist sync snapshot cannot be safely backed up: {error}")
        })?;
    }
    Ok(())
}

fn safe_path_basename(path: &str) -> Option<String> {
    let basename = Path::new(path).file_name()?.to_str()?;
    safe_path_component_hint(basename).then(|| basename.to_owned())
}

/// A relink hint is inert display data. Preserve the platform's complete
/// filename component rather than imposing a smaller arbitrary character cap.
fn safe_path_component_hint(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_JSON_BYTES
        && name != "."
        && name != ".."
        && Path::new(name).file_name().and_then(|value| value.to_str()) == Some(name)
}

fn sanitize_radio_history(value: Value) -> Option<Value> {
    let stations = value.get("stations")?.as_object()?;
    if stations.len() > 200 {
        return None;
    }
    let mut safe_stations = serde_json::Map::new();
    for (station_id, tracks) in stations {
        if !valid_id(station_id) {
            continue;
        }
        let Some(tracks) = tracks.as_array() else {
            continue;
        };
        let clean = tracks.iter().take(50).filter_map(|track| {
            let title = track.get("title")?.as_str()?;
            let artist = track.get("artist")?.as_str()?;
            if !safe_text(title, 300) || artist.chars().count() > 300 || artist.chars().any(char::is_control) { return None; }
            let safe_station = track.get("stationId").and_then(Value::as_str).filter(|s| valid_id(s)).unwrap_or(station_id);
            let mut item = serde_json::json!({"stationId":safe_station,"title":sanitize_json(&Value::String(title.into())),"artist":sanitize_json(&Value::String(artist.into())),"sourceUrl":"","artworkUrl":null});
            for field in ["album", "year"] {
                if let Some(text) = track.get(field).and_then(Value::as_str).filter(|s| safe_text(s, 120)) { item[field] = sanitize_json(&Value::String(text.into())); }
            }
            if let Some(timestamp) = track.get("observedAt").and_then(Value::as_str).filter(|s| s.len() <= 64 && !s.chars().any(char::is_control)) { item["observedAt"] = Value::String(timestamp.into()); }
            let confidence = track.get("metadataConfidence").and_then(Value::as_str).filter(|s| safe_text(s, 240)).unwrap_or("Imported radio history; stream URL removed");
            item["metadataConfidence"] = sanitize_json(&Value::String(confidence.into()));
            Some(item)
        }).collect::<Vec<_>>();
        safe_stations.insert(station_id.clone(), Value::Array(clean));
    }
    Some(serde_json::json!({"stations":safe_stations}))
}

fn sanitize_loudness_preference(value: Value) -> Option<Value> {
    let mode = value.get("mode")?.as_str()?;
    if !matches!(mode, "off" | "track" | "album") {
        return None;
    }
    let target = value
        .get("targetLufs")
        .and_then(Value::as_f64)
        .unwrap_or(-18.0);
    let ceiling = value
        .get("ceilingDbtp")
        .and_then(Value::as_f64)
        .unwrap_or(-1.0);
    let boost = value
        .get("maximumBoostDb")
        .and_then(Value::as_f64)
        .unwrap_or(6.0);
    let album_track_fallback = value
        .get("albumTrackFallback")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !target.is_finite()
        || !(-24.0..=-14.0).contains(&target)
        || !ceiling.is_finite()
        || !(-3.0..=0.0).contains(&ceiling)
        || !boost.is_finite()
        || !(0.0..=6.0).contains(&boost)
    {
        return None;
    }
    Some(
        serde_json::json!({"mode":mode,"targetLufs":target,"ceilingDbtp":ceiling,"maximumBoostDb":boost,"albumTrackFallback":album_track_fallback}),
    )
}

/// Keep only validated, versioned stream-analysis records. The typed record
/// contains stable provider/identity digests and inferred measurements; it has
/// no fields for a playable URL, headers, path, or credentials.
fn sanitize_stream_analysis_cache(value: Value) -> Option<Value> {
    use crate::stream_analysis::{
        AnalysisResource, StreamAnalysisCache, WholeTrackAnalysis, MAX_CACHE_RECORDS,
        SCHEMA_VERSION,
    };

    let object = value.as_object()?;
    if object.get("schemaVersion")?.as_u64()? != u64::from(SCHEMA_VERSION) {
        return None;
    }
    let raw_records = object.get("records")?.as_array()?;
    if raw_records.len() > MAX_CACHE_RECORDS {
        return None;
    }
    let records = raw_records
        .iter()
        .filter_map(|raw| {
            let record: WholeTrackAnalysis = serde_json::from_value(raw.clone()).ok()?;
            record.validate().ok()?;
            let AnalysisResource::Stream {
                recording_id,
                provider,
                source_identity_sha256,
                ..
            } = &record.resource
            else {
                return None;
            };
            if !safe_opaque_identity(recording_id)
                || !matches!(
                    provider.as_str(),
                    "youtube" | "soundcloud" | "bandcamp" | "monochrome"
                )
                || !valid_sha256(source_identity_sha256)
                || source_identity_sha256
                    .bytes()
                    .any(|byte| byte.is_ascii_uppercase())
                || chrono::DateTime::parse_from_rfc3339(&record.measured_at).is_err()
            {
                return None;
            }
            serde_json::to_value(record).ok()
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({
        "schemaVersion": SCHEMA_VERSION,
        "records": records,
    }))
}

fn sanitize_history_mirror_options(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    let schema_version = object
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if !matches!(schema_version, 0 | 1) {
        return None;
    }
    let recent_days = match object.get("recentDays") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let days = u32::try_from(value.as_u64()?).ok()?;
            if !(1..=3650).contains(&days) {
                return None;
            }
            Some(days)
        }
    };
    Some(serde_json::json!({
        "schemaVersion": 1,
        "recentDays": recent_days,
    }))
}

fn sanitize_backup_schedule(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    let interval = object.get("interval").and_then(Value::as_str)?;
    if !matches!(interval, "daily" | "weekly" | "custom") {
        return None;
    }
    let interval_days = object
        .get("intervalDays")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 365);
    let daily = object
        .get("retentionDaily")
        .and_then(Value::as_u64)
        .unwrap_or(7)
        .clamp(1, 365);
    let weekly = object
        .get("retentionWeekly")
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 104);
    // Device paths and scheduler state do not travel. A restored schedule is
    // reviewable but disabled until the user selects a destination again.
    Some(serde_json::json!({
        "enabled":false,
        "destination":null,
        "interval":interval,
        "intervalDays":interval_days,
        "includeMedia":object.get("includeMedia").and_then(Value::as_bool).unwrap_or(false),
        "retentionDaily":daily,
        "retentionWeekly":weekly,
        "lastSuccessAt":null,
        "enabledAt":null,
        "lastError":null
    }))
}

fn sanitize_frontend_preference(key: &str, value: Value) -> Option<Value> {
    if !FRONTEND_PREFERENCE_KEYS.contains(&key) {
        return None;
    }
    match key {
        "burrata.playlist.include-ytdlp-search.v1" => value.as_bool().map(Value::Bool),
        "burrata.discovery.search-history.v1" => {
            let entries = value.get("entries")?.as_array()?;
            if value.get("version")?.as_u64()? != 1 || entries.len() > 20 {
                return None;
            }
            let kinds = [
                "all",
                "tracks",
                "artists",
                "albums",
                "videos",
                "playlists",
                "profiles",
                "channels",
            ];
            let mut clean = Vec::new();
            for entry in entries {
                let query = entry.get("query").and_then(Value::as_str)?;
                let kind = entry.get("kind").and_then(Value::as_str)?;
                let mode = entry.get("mode").and_then(Value::as_str)?;
                let source_choice = entry.get("sourceChoice").and_then(Value::as_str)?;
                let saved_at = entry.get("savedAt").and_then(Value::as_f64)?;
                if !safe_text(query, 500)
                    || !kinds.contains(&kind)
                    || !matches!(mode, "online" | "library")
                    || !saved_at.is_finite()
                    || saved_at < 0.
                {
                    continue;
                }
                let source_choice = if source_choice == "all"
                    || source_choice == "custom"
                    || source_choice == "library"
                    || portable_search_source(source_choice)
                {
                    source_choice
                } else {
                    "all"
                };
                let ids = entry
                    .get("customSourceIds")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .filter(|s| portable_search_source(s))
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                clean.push(serde_json::json!({"query":query,"kind":kind,"mode":mode,"sourceChoice":source_choice,"customSourceIds":ids,"savedAt":saved_at}));
            }
            Some(serde_json::json!({"version":1,"entries":clean}))
        }
        "burrata.soulseek.wishlist.v1" => {
            if value.get("version")?.as_u64()? != 1 {
                return None;
            }
            let entries = value.get("entries")?.as_array()?;
            if entries.len() > 50 {
                return None;
            }
            let mut clean = Vec::new();
            for e in entries {
                let id = e.get("id").and_then(Value::as_str)?;
                let query = e.get("query").and_then(Value::as_str)?;
                if !valid_id(id) || !safe_text(query, 500) {
                    continue;
                }
                clean.push(serde_json::json!({"id":id,"query":query,"enabled":false,"nextRun":0,"failures":e.get("failures").and_then(Value::as_u64).unwrap_or(0).min(10),"lastChecked":e.get("lastChecked").cloned().filter(|v|v.is_null()||v.as_u64().is_some()),"resultCount":e.get("resultCount").cloned().filter(|v|v.is_null()||v.as_u64().is_some_and(|n|n<=10_000)),"outcome":e.get("outcome").and_then(Value::as_str).filter(|s|matches!(*s,"unchecked"|"results"|"empty"|"unavailable")).unwrap_or("unchecked")}));
            }
            Some(serde_json::json!({"version":1,"entries":clean,"nextSearch":0}))
        }
        "burrata.discovery.search-scope.v2" => {
            let mode = value.get("mode")?.as_str()?;
            let source = value.get("sourceId")?.as_str()?;
            if !matches!(mode, "online" | "library") {
                return None;
            }
            let source = if matches!(source, "all" | "custom" | "library")
                || portable_search_source(source)
            {
                source
            } else {
                "all"
            };
            let ids = value
                .get("sourceIds")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .filter(|s| portable_search_source(s))
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Some(serde_json::json!({"mode":mode,"sourceId":source,"sourceIds":ids}))
        }
        "burrata.discovery.sources.v1" => value.as_array().map(|a| {
            Value::Array(
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| *s == "library" || portable_search_source(s))
                    .map(|s| Value::String(s.into()))
                    .collect(),
            )
        }),
        "burrata.discovery.continue-search.v1"
        | "burrata.queue-end-radio.v1"
        | "burrata.waveform-enabled.v1"
        | "burrata.stream-media-analysis-enabled.v1" => value.as_bool().map(Value::Bool),
        "burrata.layout.sidebar.v1" => value
            .as_u64()
            .filter(|n| (160..=320).contains(n))
            .map(Value::from),
        "burrata.layout.info.v1" => value
            .as_u64()
            .filter(|n| (280..=420).contains(n))
            .map(Value::from),
        "burrata.lyrics-timing.v1" => {
            let object = value.as_object()?;
            if object.len() > 10_000 {
                return None;
            }
            Some(Value::Object(
                object
                    .iter()
                    .filter_map(|(id, e)| {
                        if !valid_id(id)
                            || looks_sensitive_inline(id)
                            || looks_like_absolute_path(id)
                        {
                            return None;
                        }
                        let seconds = e.get("seconds")?.as_f64()?;
                        let updated = e.get("updated")?.as_f64()?;
                        (seconds.is_finite()
                            && seconds.abs() <= 30.
                            && updated.is_finite()
                            && updated >= 0.)
                            .then(|| {
                                (
                                    id.clone(),
                                    serde_json::json!({"seconds":seconds,"updated":updated}),
                                )
                            })
                    })
                    .collect(),
            ))
        }
        _ => None,
    }
}

fn portable_search_source(source: &str) -> bool {
    matches!(
        source,
        "spotify"
            | "musicbrainz"
            | "discogs"
            | "youtube"
            | "soundcloud"
            | "soulseek"
            | "monochrome"
            | "bandcamp"
    )
}

fn sanitize_application(value: &Value) -> Value {
    let defaults = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    let mut out = defaults.as_object().cloned().unwrap_or_default();
    let Some(source) = value.as_object() else {
        return Value::Object(out);
    };
    for key in PORTABLE_APPLICATION_FIELDS {
        if let Some(v) = source.get(*key) {
            if safe_setting_value(key, v) {
                out.insert((*key).into(), sanitize_json(v));
            }
        }
    }
    // Restore harmless defaults for every machine-bound or sensitive field.
    for key in [
        "musicDirectory",
        "ytDlpPath",
        "ffmpegPath",
        "ffprobePath",
        "jsRuntimePath",
        "playlistDirectories",
        "playbackDiagnosticsEnabled",
        "downloadAudioVerificationEnabled",
    ] {
        if let Some(default) = defaults.get(key) {
            out.insert(key.into(), default.clone());
        }
    }
    Value::Object(out)
}
fn safe_setting_value(key: &str, value: &Value) -> bool {
    match key {
        "downloadLayout" => matches!(value.as_str(), Some("flat" | "artistAlbum" | "playlist")),
        "playlistMirroringEnabled" => value.is_boolean(),
        "compactPlaybackBar" => value.is_boolean(),
        "matchingLevel" => matches!(value.as_str(), Some("exact" | "balanced" | "flexible")),
        "theme" => matches!(value.as_str(), Some("dark" | "light" | "system")),
        "previewDelayMs" => value.as_u64().is_some_and(|n| n <= 5000),
        "playbackStartupTimeoutSeconds" => matches!(value.as_u64(), Some(20 | 30 | 45)),
        "streamingFallbackDelayMs" => value.as_u64().is_some_and(|n| n <= 15000),
        "streamingFallbackPolicyVersion" => value.as_u64() == Some(1),
        "streamingFallbackSources" | "downloadFallbackSources" => {
            value.as_array().is_some_and(|a| {
                a.len() <= 20
                    && a.iter()
                        .all(|v| v.as_str().is_some_and(portable_provider_id))
            })
        }
        "preferredDownloadSource" | "preferredStreamingSource" => {
            value.as_str().is_some_and(portable_provider_id)
        }
        "autoPreview"
        | "liveBpmEnabled"
        | "pauseOnSourceOpen"
        | "sourceOpenAtPosition"
        | "overridePlaylistTrackUrls"
        | "streamingFallbackEnabled"
        | "bandcampFallbackEnabled"
        | "backgroundSourcePreparation"
        | "downloadFallbackEnabled"
        | "downloadFallbackAllowLossy"
        | "soulseekWaitForSource"
        | "soulseekLosslessUpgrades"
        | "embedLyricsOnDownload" => value.is_boolean(),
        _ => false,
    }
}
fn application_from_archive(value: &Value) -> Result<Settings> {
    if value
        .get("playlistMirroringEnabled")
        .is_some_and(|enabled| !enabled.is_boolean())
    {
        return Err("Backup settings are invalid".into());
    }
    let sanitized = sanitize_application(value);
    serde_json::from_value(sanitized).map_err(|_| "Backup settings are invalid".into())
}
fn sanitize_playlist_preferences(value: Value) -> Option<Value> {
    let m = value.as_object()?;
    let shortcuts = m.get("shortcuts")?.as_array()?;
    if shortcuts.len() > 9 {
        return None;
    }
    let clean = shortcuts
        .iter()
        .take(9)
        .map(|v| {
            v.as_str()
                .filter(|s| valid_id(s))
                .map(|s| Value::String(s.into()))
                .unwrap_or(Value::Null)
        })
        .collect::<Vec<_>>();
    let bindings = m
        .get("shortcutBindings")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 9)
        .map(|a| {
            Value::Array(
                a.iter()
                    .map(|v| {
                        if v.as_str().is_some_and(|s| safe_text(s, 32)) {
                            v.clone()
                        } else {
                            Value::Null
                        }
                    })
                    .collect(),
            )
        })
        .unwrap_or(Value::Null);
    Some(
        serde_json::json!({"favoritesPlaylistId":m.get("favoritesPlaylistId").and_then(Value::as_str).filter(|s|valid_id(s)).unwrap_or(""),"shortcuts":clean,"shortcutBindings":bindings}),
    )
}
fn sanitize_playlist_mirror_options(value: Value) -> Option<Value> {
    const MAX_PLAYLIST_MIRROR_POLICIES: usize = 10_000;
    let object = value.as_object()?;
    if object.get("schemaVersion")?.as_u64()? != 1 {
        return None;
    }
    let playlists = object.get("playlists")?.as_object()?;
    if playlists.len() > MAX_PLAYLIST_MIRROR_POLICIES {
        return None;
    }
    let mut clean = Map::new();
    for (id, options) in playlists {
        if !matches!(id.as_str(), "favorites" | "history") && !valid_id(id) {
            continue;
        }
        let Some(options) = options.as_object() else {
            continue;
        };
        let included = match options.get("included") {
            Some(Value::Bool(value)) => *value,
            Some(_) => continue,
            None => true,
        };
        let optional_override = |key: &str| -> Option<Option<bool>> {
            match options.get(key) {
                None | Some(Value::Null) => Some(None),
                Some(Value::Bool(value)) => Some(Some(*value)),
                Some(_) => None,
            }
        };
        let Some(include_media_links) = optional_override("includeMediaLinks") else {
            continue;
        };
        let Some(include_ytdlp_search) = optional_override("includeYtdlpSearch") else {
            continue;
        };
        clean.insert(
            id.clone(),
            serde_json::json!({
                "included": included,
                "includeMediaLinks": include_media_links,
                "includeYtdlpSearch": include_ytdlp_search,
            }),
        );
    }
    Some(serde_json::json!({"schemaVersion":1,"playlists":clean}))
}
fn sanitize_sidebar_layout(value: Value) -> Option<Value> {
    let m = value.as_object()?;
    let sections = m.get("sections")?.as_array()?;
    if sections.len() > 100 {
        return None;
    }
    let clean_sections = sections
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?;
            let name = s.get("name")?.as_str()?;
            (valid_id(id) && safe_text(name, 80)).then(|| serde_json::json!({"id":id,"name":name}))
        })
        .collect::<Vec<_>>();
    let allowed = clean_sections
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect::<HashSet<_>>();
    let assignments = m
        .get("assignments")
        .and_then(Value::as_object)
        .map(|a| {
            Value::Object(
                a.iter()
                    .filter(|(k, v)| valid_id(k) && v.as_str().is_some_and(|s| allowed.contains(s)))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        })
        .unwrap_or(serde_json::json!({}));
    let names = m
        .get("names")
        .and_then(Value::as_object)
        .map(|a| {
            Value::Object(
                a.iter()
                    .filter(|(k, v)| valid_id(k) && v.as_str().is_some_and(|s| safe_text(s, 160)))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        })
        .unwrap_or(serde_json::json!({}));
    Some(
        serde_json::json!({"sections":clean_sections,"assignments":assignments,"names":names,"showAccountPlaylists":{"spotify":false,"youtube":false}}),
    )
}
fn sanitize_radio_stations(value: Value) -> Option<Value> {
    let stations = value.as_array()?;
    if stations.len() > 500 {
        return None;
    }
    let clean = stations
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?;
            let name = s.get("name")?.as_str()?;
            let stream = s.get("streamUrl")?.as_str()?;
            let source = s.get("sourceUrl").and_then(Value::as_str).unwrap_or("");
            if !valid_id(id)
                || !safe_text(name, 160)
                || !public_http_url(stream)
                || (!source.is_empty() && !public_http_url(source))
            {
                return None;
            }
            let mut station = Map::new();
            station.insert("id".into(), Value::String(id.into()));
            station.insert("name".into(), Value::String(name.into()));
            station.insert("streamUrl".into(), Value::String(safe_public_url(stream)));
            station.insert("sourceUrl".into(), Value::String(safe_public_url(source)));
            for key in ["provider", "country", "tags", "codec", "language"] {
                if let Some(raw) = s
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| safe_text(v, 512))
                {
                    station.insert(key.into(), Value::String(raw.into()));
                }
            }
            for key in ["homepageUrl", "faviconUrl"] {
                if let Some(raw) = s
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| public_http_url(v))
                {
                    station.insert(key.into(), Value::String(safe_public_url(raw)));
                }
            }
            if let Some(n) = s
                .get("bitrateKbps")
                .and_then(Value::as_u64)
                .filter(|n| *n <= 2_000)
            {
                station.insert("bitrateKbps".into(), Value::from(n));
            }
            if let Some(v) = s.get("isCustom").and_then(Value::as_bool) {
                station.insert("isCustom".into(), Value::Bool(v));
            }
            Some(Value::Object(station))
        })
        .collect::<Vec<_>>();
    Some(Value::Array(clean))
}
fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn safe_opaque_identity(s: &str) -> bool {
    valid_id(s) && !looks_like_url(s) && !looks_like_absolute_path(s) && !looks_sensitive_inline(s)
}
fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn encode_hex(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}
fn portable_provider_id(s: &str) -> bool {
    matches!(
        s,
        "youtube" | "soundcloud" | "bandcamp" | "monochrome" | "slskd" | "direct" | "local"
    )
}
fn safe_text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
}

fn valid_private_note(s: &str) -> bool {
    s.len() <= 16 * 1024
        && !s.chars().any(|character| {
            character == '\0'
                || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        })
}

fn valid_backup_note(s: &str) -> bool {
    // Runtime hierarchy storage has no note-size cap. The archive component's
    // 8 MiB JSON boundary is the actual backup bound; fail the whole component
    // there rather than dropping a large user's note here.
    s.len() <= MAX_JSON_BYTES
}

fn safe_hierarchy_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512
}

fn is_manual_text_field(key: &str) -> bool {
    matches!(
        key,
        "artist"
            | "title"
            | "version"
            | "album"
            | "albumArtist"
            | "recordingDate"
            | "label"
            | "genre"
            | "normalizedGenre"
            | "key"
            | "releaseDate"
            | "isrc"
            | "catalogNumber"
    )
}

fn manual_text_value(value: &Value) -> Option<Value> {
    match value {
        Value::Null => Some(Value::Null),
        // Values are already bounded by parse_bounded_json. Keep known user
        // text literal here, including paths, URLs, and control characters;
        // it is inert metadata and the enclosing component budget is enforced
        // before publication.
        Value::String(text) => Some(Value::String(text.clone())),
        _ => None,
    }
}

/// `manual_metadata` contains typed user overrides. Preserve supported text
/// values literally as inert data while projecting only known Track fields.
fn sanitize_manual_metadata(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Object(Map::new());
    };
    let allowed = [
        "artist",
        "title",
        "version",
        "album",
        "albumArtist",
        "recordingDate",
        "trackNumber",
        "trackTotal",
        "discNumber",
        "discTotal",
        "remixers",
        "label",
        "genre",
        "normalizedGenre",
        "durationSeconds",
        "bpm",
        "key",
        "releaseDate",
        "dateAdded",
        "isrc",
        "catalogNumber",
        "disposition",
        "provenance",
    ];
    let mut clean = Map::new();
    for key in allowed {
        let Some(value) = object.get(key) else {
            continue;
        };
        let safe_value = if is_manual_text_field(key) {
            manual_text_value(value)
        } else if key == "remixers" {
            if value.is_null() {
                Some(Value::Null)
            } else {
                value.as_array().map(|values| {
                    Value::Array(values.iter().filter_map(manual_text_value).collect())
                })
            }
        } else if key == "provenance" {
            Some(sanitize_manual_provenance(value))
        } else {
            Some(sanitize_json(value))
        };
        if let Some(safe_value) = safe_value {
            clean.insert(key.into(), safe_value);
        }
    }
    Value::Object(clean)
}

fn sanitize_track_value(value: &Value) -> Value {
    let Some(m) = value.as_object() else {
        return Value::Null;
    };
    let keys = [
        "id",
        "provider",
        "providerId",
        "artist",
        "title",
        "version",
        "album",
        "albumArtist",
        "recordingDate",
        "trackNumber",
        "trackTotal",
        "discNumber",
        "discTotal",
        "remixers",
        "playbackRange",
        "assetId",
        "label",
        "genre",
        "normalizedGenre",
        "durationSeconds",
        "bpm",
        "key",
        "releaseDate",
        "dateAdded",
        "isrc",
        "catalogNumber",
        "favorite",
        "disposition",
    ];
    let mut out = Map::new();
    for key in keys {
        if let Some(v) = m.get(key) {
            if key == "playbackRange" {
                if let Some(range) = sanitize_track_playback_range(v) {
                    out.insert(key.into(), range);
                }
                continue;
            }
            if key == "assetId" && !v.as_str().is_some_and(safe_opaque_identity) {
                continue;
            }
            if key == "dateAdded" && !v.is_string() && !v.is_number() {
                continue;
            }
            let manually_owned = m
                .get("provenance")
                .and_then(|provenance| provenance.get("manualFields"))
                .and_then(Value::as_object)
                .and_then(|fields| fields.get(key))
                .and_then(Value::as_bool)
                == Some(true);
            let clean = if manually_owned {
                if is_manual_text_field(key) {
                    manual_text_value(v)
                } else if key == "remixers" {
                    v.as_array().map(|values| {
                        Value::Array(values.iter().filter_map(manual_text_value).collect())
                    })
                } else {
                    Some(sanitize_json(v))
                }
            } else {
                Some(sanitize_json(v))
            };
            if let Some(clean) = clean {
                out.insert(key.into(), clean);
            }
        }
    }
    for key in ["id", "provider", "providerId", "artist", "title"] {
        out.entry(key.to_owned())
            .or_insert_with(|| Value::String(String::new()));
    }
    out.insert("sourceUrl".into(), Value::String(String::new()));
    out.insert("artworkUrl".into(), Value::Null);
    out.insert("previewUrl".into(), Value::Null);
    out.insert("localPath".into(), Value::Null);
    out.insert("localAvailability".into(), Value::String("none".into()));
    out.entry("assetId").or_insert(Value::Null);
    let provenance = m
        .get("provenance")
        .map(sanitize_manual_provenance)
        .unwrap_or_else(|| Value::Object(Map::new()));
    out.insert("provenance".into(), provenance);
    Value::Object(out)
}

fn sanitize_track_playback_range(value: &Value) -> Option<Value> {
    let range = value.as_object()?;
    let parent_track_id = range.get("parentTrackId")?.as_str()?;
    let chapter_id = range.get("chapterId")?.as_str()?;
    let parent_title = range.get("parentTitle")?.as_str()?;
    let start = range.get("startTime")?.as_f64()?;
    let end = range.get("endTime")?.as_f64()?;
    if !safe_opaque_identity(parent_track_id)
        || !safe_opaque_identity(chapter_id)
        || !safe_text(parent_title, 500)
        || !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end <= start
    {
        return None;
    }
    let mut safe = serde_json::json!({
        "parentTrackId": parent_track_id,
        "chapterId": chapter_id,
        "sourceUrl": "",
        "startTime": start,
        "endTime": end,
        "parentTitle": sanitize_json(&Value::String(parent_title.into())),
    });
    if let Some(duration) = range
        .get("parentDurationSeconds")
        .and_then(Value::as_f64)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
    {
        safe["parentDurationSeconds"] = Value::from(duration);
    }
    Some(safe)
}
fn sanitize_json(value: &Value) -> Value {
    match value {
        Value::Array(a) => Value::Array(a.iter().map(sanitize_json).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .filter_map(|(k, v)| {
                    let normalized = k.to_ascii_lowercase().replace('_', "").replace('-', "");
                    if is_private_field(&normalized) {
                        return None;
                    }
                    let clean = if normalized == "provenance" {
                        sanitize_manual_provenance(v)
                    } else if v.as_str().is_some_and(|s| looks_like_url(s)) {
                        Value::String(safe_public_url(v.as_str().unwrap_or_default()))
                    } else {
                        sanitize_json(v)
                    };
                    Some((k.clone(), clean))
                })
                .collect(),
        ),
        Value::String(s) => {
            if looks_like_url(s) {
                Value::String(safe_public_url(s))
            } else if looks_sensitive_inline(s) || looks_like_absolute_path(s) {
                Value::String(String::new())
            } else if s.contains('\0') {
                Value::String(s.replace('\0', ""))
            } else {
                Value::String(s.clone())
            }
        }
        _ => value.clone(),
    }
}

fn sanitize_manual_provenance(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Object(Map::new());
    };
    let mut clean = Map::new();
    for key in [
        "manualFields",
        "dateAdded",
        "addedAt",
        "importedAt",
        "createdAt",
    ] {
        if let Some(value) = object.get(key) {
            let value = if key == "manualFields" {
                let Some(fields) = value.as_object() else {
                    continue;
                };
                Value::Object(
                    fields
                        .iter()
                        .filter(|(field, enabled)| valid_id(field) && enabled.is_boolean())
                        .map(|(field, enabled)| (field.clone(), enabled.clone()))
                        .collect(),
                )
            } else {
                sanitize_json(value)
            };
            clean.insert(key.into(), value);
        }
    }
    if let Some(binding) = object
        .get("portablePlaybackSource")
        .and_then(sanitize_portable_playback_source)
    {
        clean.insert("portablePlaybackSource".into(), binding);
    }
    Value::Object(clean)
}

fn sanitize_portable_playback_source(value: &Value) -> Option<Value> {
    let binding = value.as_object()?;
    let provider = binding.get("provider")?.as_str()?;
    let source_id = binding.get("sourceId")?.as_str()?;
    let matching_level = binding.get("matchingLevel")?.as_str()?;
    let source_hash = binding.get("sourceHash")?.as_str()?;
    if !matches!(provider, "youtube" | "soundcloud" | "monochrome")
        || source_id.is_empty()
        || source_id.len() > 256
        || !source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        || !matches!(matching_level, "exact" | "balanced" | "flexible")
        || !valid_sha256(source_hash)
    {
        return None;
    }
    Some(serde_json::json!({
        "provider": provider,
        "sourceId": source_id,
        "matchingLevel": matching_level,
        "sourceHash": source_hash,
    }))
}
fn is_private_field(k: &str) -> bool {
    [
        "token",
        "secret",
        "cookie",
        "authorization",
        "credential",
        "password",
        "session",
        "streamurl",
        "playbackurl",
        "previewurl",
        "localpath",
        "filepath",
        "absolutepath",
        "signedurl",
        "signature",
        "expires",
        "privateurl",
        "artworkurl",
        "cachepath",
        "profilepath",
        "localfileseal",
    ]
    .iter()
    .any(|p| k.contains(p))
        || k.contains("path")
        || k.contains("filename")
}
fn looks_sensitive_inline(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "bearer ",
        "access_token=",
        "api_key=",
        "apikey=",
        "client_secret=",
        "cookie:",
        "authorization:",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}
fn looks_like_absolute_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || (value.len() > 2
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'\\' | b'/'))
}
fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("file://")
        || s.starts_with("ytdl://")
}
fn public_http_url(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        if !matches!(u.scheme(), "http" | "https")
            || !u.username().is_empty()
            || u.password().is_some()
        {
            return false;
        }
        let Some(host) = u.host_str() else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
            return false;
        }
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            return match ip {
                std::net::IpAddr::V4(ip) => {
                    !ip.is_private()
                        && !ip.is_loopback()
                        && !ip.is_link_local()
                        && !ip.is_unspecified()
                }
                std::net::IpAddr::V6(ip) => {
                    !ip.is_loopback()
                        && !ip.is_unique_local()
                        && !ip.is_unicast_link_local()
                        && !ip.is_unspecified()
                }
            };
        }
        true
    })
}
fn safe_public_url(s: &str) -> String {
    if !public_http_url(s) {
        return String::new();
    }
    let Ok(mut u) = url::Url::parse(s) else {
        return String::new();
    };
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}
fn archive_asset_path(path: &str, music_root: Option<&str>, asset_id: &str) -> String {
    // Merge staging may sanitize an already projected current-profile snapshot.
    // Keep a valid root token stable instead of turning it into an external
    // placeholder and losing the reviewed mapping key on a subsequent restore.
    if let Some((root, relative)) = split_asset_hint(path) {
        return format!("{ROOT_TOKEN_PREFIX}{root}/{relative}");
    }
    let target = Path::new(path);
    if let Some(root) = music_root.filter(|s| !s.is_empty()) {
        if let Ok(relative) = target.strip_prefix(Path::new(root)) {
            if relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                return format!(
                    "{ROOT_TOKEN_PREFIX}music/{}",
                    relative.to_string_lossy().replace('\\', "/")
                );
            }
        }
    }
    let filename = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "unknown".into());
    format!(
        "{ROOT_TOKEN_PREFIX}external/{}/{filename}",
        asset_token(asset_id)
    )
}
fn resolve_asset_path(
    id: &str,
    stored: &str,
    expected_hash: &str,
    mappings: &HashMap<&str, &Path>,
) -> String {
    let Some((root, rel)) = split_asset_hint(stored) else {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    };
    let Some(base) = mappings.get(root.as_str()) else {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    };
    let relative = Path::new(&rel);
    if relative
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    }
    let candidate = base.join(relative);
    let Ok(base_canonical) = std::fs::canonicalize(base) else {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    };
    let Ok(candidate_canonical) = std::fs::canonicalize(&candidate) else {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    };
    if !candidate_canonical.starts_with(&base_canonical) {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    }
    if expected_hash.len() != 64 || !file_matches_sha256(&candidate, expected_hash) {
        return format!("{MISSING_ASSET_PREFIX}{id}");
    }
    candidate_canonical.to_string_lossy().into_owned()
}

fn split_asset_hint(path: &str) -> Option<(String, String)> {
    let hint = path.strip_prefix(ROOT_TOKEN_PREFIX)?;
    let (root, relative) = hint.split_once('/')?;
    if root == "external" {
        let (asset_id, filename) = relative.split_once('/')?;
        if asset_id.len() != 64
            || !asset_id.bytes().all(|b| b.is_ascii_hexdigit())
            || filename.is_empty()
        {
            return None;
        }
        Some((format!("external/{asset_id}"), filename.to_owned()))
    } else if root == "music" && !relative.is_empty() {
        Some((root.to_owned(), relative.to_owned()))
    } else {
        None
    }
}

fn asset_token(asset_id: &str) -> String {
    format!("{:x}", Sha256::digest(asset_id.as_bytes()))
}

fn file_matches_sha256(path: &Path, expected: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        use std::io::Read;
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(expected)
}
fn finite_nonnegative(n: f64) -> f64 {
    if n.is_finite() {
        n.max(0.)
    } else {
        0.
    }
}
fn validate_playback(p: &PlaybackArchive) -> Result<()> {
    if p.queue.len() > 10_000
        || p.entry_ids.len() != p.queue.len()
        || p.entry_ids.iter().any(|s| !valid_id(s))
        || p.entry_ids.iter().collect::<HashSet<_>>().len() != p.entry_ids.len()
        || (!p.queue.is_empty() && !(0..p.queue.len() as i64).contains(&p.current_index))
        || (p.queue.is_empty() && p.current_index != -1)
        || !p.position_seconds.is_finite()
        || p.position_seconds < 0.
        || !p.volume.is_finite()
        || !(0.0..=1.0).contains(&p.volume)
        || !matches!(p.repeat.as_str(), "off" | "all" | "one")
    {
        return Err("Saved playback state is invalid".into());
    }
    Ok(())
}

/// Keep only album contexts whose opaque binding targets an occurrence in the
/// captured queue and whose typed resource identity verifies its own key.
fn sanitize_queue_loudness_contexts(
    value: Option<&Value>,
    entry_ids: &[String],
) -> BTreeMap<String, Value> {
    let Some(bindings) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let valid_entries = entry_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    bindings
        .iter()
        .take(10_000)
        .filter_map(|(entry_id, value)| {
            if !valid_id(entry_id) || !valid_entries.contains(entry_id.as_str()) {
                return None;
            }
            let resource =
                serde_json::from_value::<crate::loudness::LoudnessResourceIdentity>(value.clone())
                    .ok()?;
            if resource.scope != crate::loudness::ResourceScope::Album
                || resource.validate().is_err()
                || resource.members.iter().any(|member| {
                    !safe_opaque_identity(&member.asset_id) || !valid_sha256(&member.file_hash)
                })
            {
                return None;
            }
            serde_json::to_value(resource)
                .ok()
                .map(|resource| (entry_id.clone(), resource))
        })
        .collect()
}

fn merge_manual_metadata(current: &Value, imported: &Value) -> Value {
    let mut out = current.as_object().cloned().unwrap_or_default();
    if let Some(incoming) = imported.as_object() {
        for (k, v) in incoming {
            out.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Value::Object(out)
}
fn snapshot_already_applied(path: &Path, snapshot_id: &str) -> Result<bool> {
    let conn = Connection::open(path).map_err(err)?;
    let Some(value) = read_preference(&conn, "backupRestore.appliedSnapshots.v1")? else {
        return Ok(false);
    };
    Ok(value
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(snapshot_id))))
}
fn mark_snapshot_applied(tx: &rusqlite::Transaction<'_>, snapshot_id: &str) -> Result<()> {
    let current: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key='backupRestore.appliedSnapshots.v1'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    let mut ids = current
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    if !ids.iter().any(|id| id == snapshot_id) {
        ids.push(snapshot_id.to_owned());
    }
    if ids.len() > 256 {
        ids.drain(..ids.len() - 256);
    }
    tx.execute("INSERT INTO settings(key,value) VALUES('backupRestore.appliedSnapshots.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(&ids).map_err(err)?]).map_err(err)?;
    Ok(())
}
fn parse_bounded_json(raw: &str) -> Result<Value> {
    if raw.len() > MAX_JSON_BYTES {
        return Err(format!("Backup JSON field exceeds the {MAX_JSON_BYTES}-byte limit").into());
    }
    serde_json::from_str(raw).map_err(|_| "Backup JSON field is invalid".into())
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(err)?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(
            format!("Backup JSON component exceeds the {MAX_JSON_BYTES}-byte limit").into(),
        );
    }
    std::fs::write(path, bytes).map_err(err)
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let mut bytes = Vec::with_capacity(MAX_JSON_BYTES + 1);
    std::fs::File::open(path)
        .map_err(err)?
        .take((MAX_JSON_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(
            format!("Backup JSON component exceeds the {MAX_JSON_BYTES}-byte limit").into(),
        );
    }
    serde_json::from_slice(&bytes).map_err(|_| "Backup JSON component is invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use sha2::Digest;

    #[test]
    fn backup_merge_remaps_waveform_cache_asset_and_preserves_verified_library_ref() {
        let source = Connection::open_in_memory().unwrap();
        let target = Connection::open_in_memory().unwrap();
        source
            .execute_batch(
                "CREATE TABLE analysis_cache_records(
                namespace TEXT NOT NULL, cache_key TEXT NOT NULL, payload TEXT NOT NULL,
                payload_bytes INTEGER NOT NULL, updated_at TEXT NOT NULL,
                last_used_at TEXT NOT NULL, protected INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(namespace,cache_key));
             CREATE TABLE analysis_cache_record_refs(
                namespace TEXT NOT NULL, cache_key TEXT NOT NULL, recording_id TEXT NOT NULL);
             CREATE TABLE local_assets(id TEXT PRIMARY KEY, hash TEXT NOT NULL);",
            )
            .unwrap();
        target.execute_batch(
            "CREATE TABLE analysis_cache_records(
                namespace TEXT NOT NULL, cache_key TEXT NOT NULL, payload TEXT NOT NULL,
                payload_bytes INTEGER NOT NULL, updated_at TEXT NOT NULL,
                last_used_at TEXT NOT NULL, protected INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(namespace,cache_key));
             CREATE TABLE analysis_cache_record_refs(
                namespace TEXT NOT NULL, cache_key TEXT NOT NULL, recording_id TEXT NOT NULL,
                PRIMARY KEY(namespace,cache_key,recording_id));
             CREATE TABLE catalog_listings(id TEXT PRIMARY KEY, recording_id TEXT NOT NULL, in_library INTEGER NOT NULL);
             CREATE TABLE local_assets(id TEXT PRIMARY KEY, hash TEXT NOT NULL);",
        ).unwrap();

        let payload = serde_json::json!({
            "schemaVersion": 1,
            "assetId": "archive-asset",
            "fileHash": "a".repeat(64),
            "binCount": 2,
            "analyzerVersion": "waveform-v1",
            "durationSeconds": 8.0,
            "bins": [{"min": -0.5, "max": 0.5}, {"min": -0.25, "max": 0.25}],
        });
        source
            .execute(
                "INSERT INTO local_assets(id,hash) VALUES('archive-asset',?1)",
                ["a".repeat(64)],
            )
            .unwrap();
        let encoded = payload.to_string();
        let old_key = "b".repeat(64);
        source.execute(
            "INSERT INTO analysis_cache_records(namespace,cache_key,payload,payload_bytes,updated_at,last_used_at,protected)
             VALUES('local-waveform-v1',?1,?2,?3,'2026-10-27T00:00:00Z','2026-10-27T00:00:00Z',1)",
            params![old_key, encoded, encoded.len() as i64],
        ).unwrap();
        source
            .execute(
                "INSERT INTO analysis_cache_record_refs(namespace,cache_key,recording_id)
             VALUES('local-waveform-v1',?1,'archive-recording')",
                [&old_key],
            )
            .unwrap();
        let mismatched_payload = serde_json::json!({
            "assetId": "archive-asset", "fileHash": "c".repeat(64), "binCount": 2,
            "analyzerVersion": "waveform-v1", "durationSeconds": 8.0,
            "bins": [{"min": -0.5, "max": 0.5}, {"min": -0.25, "max": 0.25}],
        })
        .to_string();
        let mismatched_payload_bytes = mismatched_payload.len() as i64;
        source.execute(
            "INSERT INTO analysis_cache_records(namespace,cache_key,payload,payload_bytes,updated_at,last_used_at,protected)
             VALUES('local-waveform-v1',?1,?2,?3,'2026-10-27T00:00:00Z','2026-10-27T00:00:00Z',1)",
            params!["d".repeat(64), mismatched_payload, mismatched_payload_bytes],
        ).unwrap();
        target
            .execute(
                "INSERT INTO local_assets(id,hash) VALUES('target-asset',?1)",
                ["a".repeat(64)],
            )
            .unwrap();
        target.execute(
            "INSERT INTO catalog_listings(id,recording_id,in_library) VALUES('target-recording','target-recording',1)",
            [],
        ).unwrap();

        let asset_mapping =
            HashMap::from([("archive-asset".to_owned(), "target-asset".to_owned())]);
        let recording_mapping = HashMap::from([(
            "archive-recording".to_owned(),
            "target-recording".to_owned(),
        )]);
        let transaction = target.unchecked_transaction().unwrap();
        let key_mapping =
            merge_analysis_cache_records(&source, &transaction, &recording_mapping, &asset_mapping)
                .unwrap();
        merge_analysis_cache_refs(&source, &transaction, &recording_mapping, &key_mapping).unwrap();

        let (new_key, restored_payload, protected): (String, String, i64) = transaction.query_row(
            "SELECT cache_key,payload,protected FROM analysis_cache_records WHERE namespace='local-waveform-v1'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        let expected_identity = format!(
            "burrata-local-waveform-content-v1\n{}\n2\nwaveform-v1",
            "a".repeat(64)
        );
        assert_eq!(
            new_key,
            format!("{:x}", sha2::Sha256::digest(expected_identity.as_bytes()))
        );
        assert_eq!(
            serde_json::from_str::<Value>(&restored_payload).unwrap()["assetId"],
            "target-asset"
        );
        assert_eq!(protected, 1);
        let reference: String = transaction.query_row(
            "SELECT recording_id FROM analysis_cache_record_refs WHERE namespace='local-waveform-v1' AND cache_key=?1",
            [&new_key], |row| row.get(0),
        ).unwrap();
        assert_eq!(reference, "target-recording");
        let imported_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM analysis_cache_records WHERE namespace='local-waveform-v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            imported_count, 1,
            "hash-mismatched waveform cache rows are not restored"
        );
    }

    fn loudness_test_record(
        resource: crate::loudness::LoudnessResourceIdentity,
    ) -> crate::loudness::LoudnessRecord {
        crate::loudness::LoudnessRecord {
            schema_version: crate::loudness::LOUDNESS_SCHEMA_VERSION,
            resource,
            analyzer_version: "fixture-loudness-v2".into(),
            provenance: crate::loudness::MeasurementProvenance::FfmpegAnalysis,
            integrated_lufs: Some(-18.0),
            true_peak_dbtp: Some(-1.0),
            sample_rate: Some(48_000),
            channels: 2,
            measured_at: "2026-10-27T00:00:00Z".into(),
            stereo_mix_true_peak_dbtp: None,
            stereo_mix_integrated_lufs: None,
            stereo_mix_matrix: None,
            stereo_mix_layout: None,
            maximum_member_safe_true_peak_dbtp: None,
            maximum_member_native_safe_true_peak_dbtp: None,
            conservative_channel_sum_bound: false,
            local_file_seals: vec![crate::loudness::LocalFileSeal {
                asset_id: "source-a".into(),
                file_hash: "a".repeat(64),
                size_bytes: 100,
                modified_unix_ns: Some(1_700_000_000_000_000_000),
                file_identity: Some("device:source-inode".into()),
                verified_in_process: "source-process".into(),
            }],
            opus_decoder_proofs: vec![],
        }
    }

    fn loudness_test_target(assets: &[(&str, &str)]) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE local_assets(id TEXT PRIMARY KEY, hash TEXT NOT NULL);")
            .unwrap();
        for (asset_id, hash) in assets {
            connection
                .execute(
                    "INSERT INTO local_assets(id,hash) VALUES(?1,?2)",
                    params![*asset_id, *hash],
                )
                .unwrap();
        }
        connection
    }

    #[test]
    fn loudness_track_analysis_remaps_same_hash_alias_and_recomputes_resource_key() {
        let hash = "a".repeat(64);
        let target = loudness_test_target(&[("target-a", &hash)]);
        let source_hashes = HashMap::from([("source-a".to_owned(), hash.clone())]);
        let asset_map = HashMap::from([("source-a".to_owned(), "target-a".to_owned())]);
        let resource = crate::loudness::LoudnessResourceIdentity::track(
            "source-a",
            hash.clone(),
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        let mut record = loudness_test_record(resource);
        record
            .opus_decoder_proofs
            .push(crate::loudness::OpusDecoderProof {
                asset_id: "source-a".into(),
                file_hash: hash.clone(),
                decoder_sha256: "b".repeat(64),
                probe_schema_version: 1,
            });
        let remapped = remap_loudness_analysis_payload(
            &serde_json::to_string(&record).unwrap(),
            crate::loudness_manager::ANALYSIS_CACHE_VERSION,
            "source-a",
            &hash,
            "target-a",
            &asset_map,
            &source_hashes,
            &target,
        )
        .unwrap()
        .unwrap();
        let remapped: crate::loudness::LoudnessRecord = serde_json::from_value(remapped).unwrap();
        let expected = crate::loudness::LoudnessResourceIdentity::track(
            "target-a",
            hash.clone(),
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        assert_eq!(remapped.resource, expected);
        assert!(remapped.local_file_seals.is_empty());
        assert_eq!(remapped.integrated_lufs, Some(-18.0));
        assert_eq!(remapped.opus_decoder_proofs.len(), 1);
        assert!(remapped.opus_decoder_proofs[0].validates_for("target-a", &hash, &"b".repeat(64)));
        assert!(remapped.validate().is_ok());
    }

    #[tokio::test]
    async fn remapped_loudness_cache_requires_fresh_local_file_verification() {
        let directory = tempfile::tempdir().unwrap();
        let (_, _, hash, _) = insert_fixture(directory.path());
        let store = std::sync::Arc::new(
            crate::storage::Store::open(&directory.path().join("burrata.sqlite")).unwrap(),
        );
        let target = loudness_test_target(&[("asset-1", &hash)]);
        let source_hashes = HashMap::from([("source-a".to_owned(), hash.clone())]);
        let asset_map = HashMap::from([("source-a".to_owned(), "asset-1".to_owned())]);
        let source_resource = crate::loudness::LoudnessResourceIdentity::track(
            "source-a",
            hash.clone(),
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        let mut source_record = loudness_test_record(source_resource);
        source_record.local_file_seals[0].file_hash = hash.clone();
        let remapped = remap_loudness_analysis_payload(
            &serde_json::to_string(&source_record).unwrap(),
            crate::loudness_manager::ANALYSIS_CACHE_VERSION,
            "source-a",
            &hash,
            "asset-1",
            &asset_map,
            &source_hashes,
            &target,
        )
        .unwrap()
        .unwrap();
        store
            .save_analysis_record(
                "asset-1",
                &hash,
                crate::loudness_manager::ANALYSIS_CACHE_VERSION,
                "completed",
                &remapped,
            )
            .unwrap();
        let expected = crate::loudness::LoudnessResourceIdentity::track(
            "asset-1",
            hash,
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        let cached =
            crate::loudness_manager::cached_record(&store, &expected, Some("fixture-loudness-v2"))
                .unwrap()
                .unwrap();
        let memo = crate::loudness_manager::LocalVerificationMemo::default();
        assert!(!memo
            .is_currently_verified(&store, &expected, Some(&cached))
            .unwrap());
        memo.verify_and_remember(store.clone(), expected.clone(), None)
            .await
            .unwrap();
        assert!(memo
            .is_currently_verified(&store, &expected, Some(&cached))
            .unwrap());
    }

    #[test]
    fn loudness_album_analysis_remaps_every_member_and_recomputes_resource_key() {
        let hash_a = "a".repeat(64);
        let hash_b = "b".repeat(64);
        let target = loudness_test_target(&[("target-a", &hash_a), ("target-b", &hash_b)]);
        let source_hashes = HashMap::from([
            ("source-a".to_owned(), hash_a.clone()),
            ("source-b".to_owned(), hash_b.clone()),
        ]);
        let asset_map = HashMap::from([
            ("source-a".to_owned(), "target-a".to_owned()),
            ("source-b".to_owned(), "target-b".to_owned()),
        ]);
        let resource = crate::loudness::LoudnessResourceIdentity::album(
            vec![
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "source-a".into(),
                    file_hash: hash_a.clone(),
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "source-b".into(),
                    file_hash: hash_b.clone(),
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
            ],
            vec![0.75],
        );
        let mut record = loudness_test_record(resource);
        record.local_file_seals = vec![
            crate::loudness::LocalFileSeal {
                asset_id: "source-a".into(),
                file_hash: hash_a.clone(),
                size_bytes: 100,
                modified_unix_ns: Some(1_700_000_000_000_000_000),
                file_identity: Some("device:source-a".into()),
                verified_in_process: "source-process".into(),
            },
            crate::loudness::LocalFileSeal {
                asset_id: "source-b".into(),
                file_hash: hash_b.clone(),
                size_bytes: 100,
                modified_unix_ns: Some(1_700_000_000_000_000_001),
                file_identity: Some("device:source-b".into()),
                verified_in_process: "source-process".into(),
            },
        ];
        let remapped = remap_loudness_analysis_payload(
            &serde_json::to_string(&record).unwrap(),
            crate::loudness_manager::ALBUM_ANALYSIS_CACHE_VERSION,
            "source-a",
            &hash_a,
            "target-a",
            &asset_map,
            &source_hashes,
            &target,
        )
        .unwrap()
        .unwrap();
        let remapped: crate::loudness::LoudnessRecord = serde_json::from_value(remapped).unwrap();
        let expected = crate::loudness::LoudnessResourceIdentity::album(
            vec![
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "target-a".into(),
                    file_hash: hash_a,
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "target-b".into(),
                    file_hash: hash_b,
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
            ],
            vec![0.75],
        );
        assert_eq!(remapped.resource, expected);
        assert!(remapped.local_file_seals.is_empty());
        assert_eq!(remapped.integrated_lufs, Some(-18.0));
        assert!(remapped.validate().is_ok());
    }

    #[test]
    fn loudness_analysis_is_omitted_when_a_mapped_target_hash_differs() {
        let source_hash = "a".repeat(64);
        let target = loudness_test_target(&[("target-a", &"b".repeat(64))]);
        let source_hashes = HashMap::from([("source-a".to_owned(), source_hash.clone())]);
        let asset_map = HashMap::from([("source-a".to_owned(), "target-a".to_owned())]);
        let resource = crate::loudness::LoudnessResourceIdentity::track(
            "source-a",
            source_hash.clone(),
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        let record = loudness_test_record(resource);
        assert!(remap_loudness_analysis_payload(
            &serde_json::to_string(&record).unwrap(),
            crate::loudness_manager::ANALYSIS_CACHE_VERSION,
            "source-a",
            &source_hash,
            "target-a",
            &asset_map,
            &source_hashes,
            &target,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn loudness_sanitizer_keeps_maximum_supported_album_layout_and_omits_seals() {
        let members = (0..200)
            .map(|index| crate::loudness::LoudnessAssetIdentity {
                asset_id: format!("asset-{index}"),
                file_hash: format!("{:064x}", index + 1),
                range: crate::loudness::AudioRange::WHOLE_FILE,
            })
            .collect::<Vec<_>>();
        let resource = crate::loudness::LoudnessResourceIdentity::album(members, vec![0.0; 199]);
        let record = crate::loudness::LoudnessRecord {
            schema_version: crate::loudness::LOUDNESS_SCHEMA_VERSION,
            resource,
            analyzer_version: "fixture-loudness-v2".into(),
            provenance: crate::loudness::MeasurementProvenance::FfmpegAnalysis,
            integrated_lufs: Some(-18.0),
            true_peak_dbtp: Some(-1.0),
            sample_rate: Some(48_000),
            channels: 2,
            measured_at: "2026-10-27T00:00:00Z".into(),
            stereo_mix_true_peak_dbtp: Some(-1.0),
            stereo_mix_integrated_lufs: Some(-18.0),
            stereo_mix_matrix: Some("web-audio-album-program-v1".into()),
            stereo_mix_layout: Some(format!("programme:{}", vec!["5.1(side)"; 200].join(","))),
            maximum_member_safe_true_peak_dbtp: None,
            maximum_member_native_safe_true_peak_dbtp: None,
            conservative_channel_sum_bound: false,
            local_file_seals: Vec::new(),
            opus_decoder_proofs: vec![],
        };
        record.validate().unwrap();
        let sanitized = sanitize_loudness_analysis_payload(
            &serde_json::to_string(&record).unwrap(),
            crate::loudness_manager::ALBUM_ANALYSIS_CACHE_VERSION,
        )
        .unwrap();
        assert_eq!(
            sanitized["stereoMixLayout"],
            record.stereo_mix_layout.as_deref().unwrap()
        );
        assert_eq!(sanitized["provenance"], "ffmpegAnalysis");
        assert!(sanitized.get("localFileSeals").is_none());
        let restored: crate::loudness::LoudnessRecord = serde_json::from_value(sanitized).unwrap();
        restored.validate().unwrap();
    }

    #[test]
    fn backup_manual_projection_preserves_runtime_manual_text_without_old_1000_char_cap() {
        let directory = tempfile::tempdir().unwrap();
        insert_fixture(directory.path());
        let store = crate::storage::Store::open(&directory.path().join("burrata.sqlite")).unwrap();
        let mut track = crate::models::Track {
            id: "fixture:long-manual".into(),
            provider: "fixture".into(),
            provider_id: "long-manual".into(),
            artist: "Catalog artist".into(),
            title: "Catalog title".into(),
            ..Default::default()
        };
        store.save_track(&track).unwrap();
        let artist = format!("{}\u{1}artist", "A".repeat(1_200));
        let title = format!("{} title", "T".repeat(1_200));
        track.artist = artist.clone();
        track.title = title.clone();
        let saved = store.update_track(&track).unwrap();
        assert_eq!(saved.artist, artist);
        assert_eq!(saved.title, title);

        let metadata: String = store
            .with_backup_connection(|connection| {
                connection
                    .query_row(
                        "SELECT manual_metadata FROM recordings WHERE id=?1",
                        [&saved.id],
                        |row| row.get(0),
                    )
                    .map_err(err)
            })
            .unwrap();
        let metadata: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(metadata["artist"], artist);
        assert_eq!(metadata["title"], title);
        let metadata_projection = sanitize_manual_metadata(&metadata);
        assert_eq!(metadata_projection["artist"], artist);
        assert_eq!(metadata_projection["title"], title);

        let track_projection = sanitize_track_value(&serde_json::to_value(&saved).unwrap());
        assert_eq!(track_projection["artist"], artist);
        assert_eq!(track_projection["title"], title);
        assert_eq!(
            track_projection["provenance"]["manualFields"]["artist"],
            true
        );
        assert_eq!(
            track_projection["provenance"]["manualFields"]["title"],
            true
        );

        let prepared = prepare_backup(
            directory.path(),
            Uuid::new_v4(),
            BackupStateOptions::default(),
        )
        .unwrap();
        let stage = tempfile::tempdir().unwrap();
        reconstruct_staged(
            Some(&prepared.library_sqlite),
            None,
            None,
            None,
            stage.path(),
            &RestoreStateOptions {
                mode: RestoreMode::Replace,
                snapshot_id: prepared.snapshot_id.to_string(),
                include_library: true,
                ..RestoreStateOptions::default()
            },
        )
        .unwrap();
        let restored_store =
            crate::storage::Store::open(&stage.path().join("burrata.sqlite")).unwrap();
        let restored_track = restored_store.get_track(&saved.id).unwrap().unwrap();
        assert_eq!(restored_track.artist, artist);
        assert_eq!(restored_track.title, title);
        assert_eq!(restored_track.provenance["manualFields"]["artist"], true);
        assert_eq!(restored_track.provenance["manualFields"]["title"], true);
        let restored_manual: String = restored_store
            .with_backup_connection(|connection| {
                connection
                    .query_row(
                        "SELECT manual_metadata FROM recordings WHERE id=?1",
                        [&saved.id],
                        |row| row.get(0),
                    )
                    .map_err(err)
            })
            .unwrap();
        let restored_manual: Value = serde_json::from_str(&restored_manual).unwrap();
        assert_eq!(restored_manual["artist"], artist);
        assert_eq!(restored_manual["title"], title);
    }

    fn playback_fixture(queue_ids: &[&str], volume: f64) -> PlaybackArchive {
        let entry_ids = (0..queue_ids.len())
            .map(|_| Uuid::new_v4().to_string())
            .collect::<Vec<_>>();
        let current_index = if queue_ids.is_empty() {
            -1
        } else {
            1.min(queue_ids.len() as i64 - 1)
        };
        PlaybackArchive {
            format: "org.burrata.playback".into(),
            schema_version: PLAYBACK_SCHEMA_VERSION,
            snapshot_id: Uuid::new_v4().to_string(),
            queue: queue_ids
                .iter()
                .map(|id| {
                    serde_json::json!({
                        "id": id,
                        "provider": "fixture",
                        "providerId": id,
                        "title": format!("Track {id}"),
                        "artist": "Fixture artist"
                    })
                })
                .collect(),
            entry_ids: entry_ids.clone(),
            queue_loudness_contexts: BTreeMap::new(),
            current_index,
            current_entry_id: (current_index >= 0)
                .then(|| entry_ids[current_index as usize].clone()),
            queue_revision: 42,
            position_seconds: 37.25,
            repeat: "all".into(),
            shuffle: true,
            volume,
            saved_at: Some("2026-10-27T12:00:00Z".into()),
            restore_paused: true,
        }
    }

    #[test]
    fn selected_playback_handoff_preserves_zero_volume_materialized_paths_and_occurrence_ids() {
        let archive = playback_fixture(&["local:duplicate", "local:duplicate"], 0.0);
        let mut tracks = archive.queue.clone();
        tracks[0]["sourceUrl"] = Value::String("local:duplicate".into());
        tracks[1]["sourceUrl"] = Value::String("local:duplicate".into());
        tracks[0]["favorite"] = Value::Bool(false);
        tracks[1]["favorite"] = Value::Bool(false);
        tracks[0]["assetId"] = Value::String("target-asset".into());
        tracks[0]["localPath"] = Value::String("/verified/destination/audio.wav".into());
        tracks[1]["assetId"] = Value::String("target-asset".into());
        tracks[1]["localPath"] = Value::String("/verified/destination/audio.wav".into());
        let queue = serde_json::json!({"tracks":tracks,"entryIds":archive.entry_ids,"queueRevision":archive.queue_revision,"queueLoudnessContexts":{}});
        let session = serde_json::json!({"index":1,"position":37.25,"volume":0.0,"repeat":"all","shuffle":true});
        let handoff =
            make_backup_playback_handoff(&queue, &session, &Uuid::new_v4().to_string()).unwrap();
        assert_eq!(handoff["version"], 1);
        assert_eq!(handoff["paused"], true);
        assert_eq!(handoff["queue"]["currentIndex"], 1);
        assert_eq!(
            handoff["queue"]["entryIds"],
            serde_json::to_value(archive.entry_ids).unwrap()
        );
        assert_eq!(
            handoff["queue"]["tracks"][1]["localPath"],
            "/verified/destination/audio.wav"
        );
        assert_eq!(handoff["position"], 37.25);
        assert_eq!(handoff["volume"], 0.0);
        assert_eq!(handoff["repeat"], "all");
        assert_eq!(handoff["shuffle"], true);
    }

    #[test]
    fn library_only_merge_preserves_current_playback_snapshot_paused() {
        let target = tempfile::tempdir().unwrap();
        insert_fixture(target.path());
        let playback_path = target.path().join("current-playback.json");
        let current = playback_fixture(&["fixture:first", "fixture:second"], 0.0);
        write_json(&playback_path, &current).unwrap();

        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: Uuid::new_v4().to_string(),
            current_playback_snapshot: Some(playback_path),
            include_library: true,
            ..RestoreStateOptions::default()
        };
        reconstruct_staged(None, None, None, None, target.path(), &options).unwrap();

        let connection = Connection::open(target.path().join("burrata.sqlite")).unwrap();
        let queue: Value = connection
            .query_row(
                "SELECT value FROM settings WHERE key='nativePlaybackQueue.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();
        let session: Value = connection
            .query_row(
                "SELECT value FROM settings WHERE key='nativePlaybackSession.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();

        assert_eq!(
            queue["entryIds"],
            serde_json::to_value(&current.entry_ids).unwrap()
        );
        assert_eq!(queue["queueRevision"], 42);
        assert_eq!(queue["tracks"].as_array().unwrap().len(), 2);
        assert_eq!(queue["tracks"][0]["id"], "fixture:first");
        assert_eq!(queue["tracks"][1]["id"], "fixture:second");
        assert_eq!(session["index"], current.current_index);
        assert_eq!(
            session["currentEntryId"],
            current.current_entry_id.clone().unwrap()
        );
        assert_eq!(session["position"], 37.25);
        assert_eq!(session["volume"], 0.0);
        assert_eq!(session["repeat"], "all");
        assert_eq!(session["shuffle"], true);
        assert_eq!(session["finished"], false);
    }

    #[test]
    fn selected_archive_playback_is_not_replaced_by_current_merge_snapshot() {
        let target = tempfile::tempdir().unwrap();
        let archive_path = target.path().join("archive-playback.json");
        let archive_playback = playback_fixture(&["fixture:archive"], 0.65);
        write_json(&archive_path, &archive_playback).unwrap();
        let current_path = target.path().join("current-playback.json");
        write_json(&current_path, &playback_fixture(&["fixture:current"], 0.0)).unwrap();

        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: Uuid::new_v4().to_string(),
            current_playback_snapshot: Some(current_path),
            include_playback: true,
            ..RestoreStateOptions::default()
        };
        reconstruct_staged(
            None,
            None,
            None,
            Some(&archive_path),
            target.path(),
            &options,
        )
        .unwrap();

        let connection = Connection::open(target.path().join("burrata.sqlite")).unwrap();
        let session: Value = connection
            .query_row(
                "SELECT value FROM settings WHERE key='nativePlaybackSession.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();
        assert_eq!(session["tracks"][0]["id"], "fixture:archive");
        assert_eq!(session["volume"], 0.65);
    }

    #[test]
    fn imported_queue_asset_requires_the_queued_recordings_own_asset() {
        let target = tempfile::tempdir().unwrap();
        insert_fixture(target.path());
        let mut connection = Connection::open(target.path().join("burrata.sqlite")).unwrap();
        connection
            .execute(
                "INSERT INTO works(id,title,artist) VALUES('work-2','Other','Artist')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO recordings(id,work_id,version,favorite,disposition,manual_metadata) VALUES('recording-2','work-2','',0,NULL,'{}')", []).unwrap();
        connection.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES('youtube:other123','youtube','other123','recording-2','','{}','2026-10-27T12:00:00Z',1)", []).unwrap();
        let tx = connection.transaction().unwrap();
        let mapping = HashMap::from([("asset-1".to_owned(), "asset-1".to_owned())]);
        let mut correct = serde_json::json!({"id":"youtube:abc123","assetId":"asset-1"});
        materialize_restored_queue_track_asset(&tx, &mut correct, &mapping).unwrap();
        assert_eq!(correct["assetId"], "asset-1");
        assert!(correct["localPath"].as_str().is_some());
        let mut different_recording =
            serde_json::json!({"id":"youtube:other123","assetId":"asset-1"});
        materialize_restored_queue_track_asset(&tx, &mut different_recording, &mapping).unwrap();
        assert!(different_recording["assetId"].is_null());
        assert!(different_recording["localPath"].is_null());
        let mut legacy = serde_json::json!({"id":"recording-1","assetId":"asset-1"});
        materialize_restored_queue_track_asset(&tx, &mut legacy, &mapping).unwrap();
        assert_eq!(legacy["assetId"], "asset-1");
    }

    #[test]
    fn merged_archive_queue_remaps_listing_alias_before_asset_owner_check() {
        let target = tempfile::tempdir().unwrap();
        insert_fixture(target.path());
        let target_library = target.path().join("burrata.sqlite");
        let target_connection = Connection::open(&target_library).unwrap();
        target_connection
            .execute(
                "INSERT INTO works(id,title,artist) VALUES('work-2','Other','Artist')",
                [],
            )
            .unwrap();
        target_connection.execute("INSERT INTO recordings(id,work_id,version,favorite,disposition,manual_metadata) VALUES('recording-2','work-2','',0,NULL,'{}')", []).unwrap();
        target_connection.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES('youtube:other123','youtube','other123','recording-2','','{}','2026-10-27T12:00:00Z',1)", []).unwrap();
        target_connection
            .execute(
                "DELETE FROM settings WHERE key IN ('nativePlaybackQueue.v1','nativePlaybackSession.v1')",
                [],
            )
            .unwrap();
        drop(target_connection);

        let archive = tempfile::tempdir().unwrap();
        insert_fixture(archive.path());
        let archive_library = archive.path().join("burrata.sqlite");
        let archive_connection = Connection::open(&archive_library).unwrap();
        // Re-key the complete fixture relationship atomically, as a library
        // from another device would, without leaving dangling ordered entries.
        archive_connection.execute_batch(
            "BEGIN;
             PRAGMA defer_foreign_keys=ON;
             UPDATE catalog_listings SET id='youtube:archive-alias',metadata=json_set(metadata,'$.id','youtube:archive-alias') WHERE id='youtube:abc123';
             UPDATE playlist_ordered_entries SET listing_id='youtube:archive-alias' WHERE listing_id='youtube:abc123';
             UPDATE local_assets SET payload=json_set(payload,'$.track.id','youtube:archive-alias') WHERE recording_id='recording-1';
             COMMIT;"
        ).unwrap();
        archive_connection
            .execute(
                "INSERT INTO works(id,title,artist) VALUES('work-2','Other','Artist')",
                [],
            )
            .unwrap();
        archive_connection.execute("INSERT INTO recordings(id,work_id,version,favorite,disposition,manual_metadata) VALUES('recording-2','work-2','',0,NULL,'{}')", []).unwrap();
        archive_connection.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES('youtube:archive-wrong-alias','youtube','other123','recording-2','','{}','2026-10-27T12:00:00Z',1)", []).unwrap();
        drop(archive_connection);

        let playback_path = archive.path().join("playback.json");
        std::fs::write(
            &playback_path,
            serde_json::to_vec(&PlaybackArchive {
                format: "org.burrata.playback".into(),
                schema_version: PLAYBACK_SCHEMA_VERSION,
                snapshot_id: Uuid::new_v4().to_string(),
                queue: vec![
                    serde_json::json!({"id":"youtube:archive-alias","provider":"youtube","providerId":"abc123","assetId":"asset-1"}),
                    serde_json::json!({"id":"youtube:archive-wrong-alias","provider":"youtube","providerId":"other123","assetId":"asset-1"}),
                ],
                entry_ids: vec!["entry-1".into(), "entry-2".into()],
                current_index: 0,
                current_entry_id: Some("entry-1".into()),
                queue_revision: 1,
                position_seconds: 0.0,
                repeat: "off".into(),
                shuffle: false,
                volume: 0.4,
                saved_at: None,
                restore_paused: true,
                ..PlaybackArchive::default()
            })
            .unwrap(),
        )
        .unwrap();

        let history_target = archive.path().join("history.sqlite");
        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: Uuid::new_v4().to_string(),
            include_library: true,
            include_playback: true,
            ..RestoreStateOptions::default()
        };
        let mut summary = RestoreStateSummary::default();
        apply_components(
            Some(&archive_library),
            None,
            None,
            Some(&playback_path),
            &target_library,
            &history_target,
            &options,
            &mut summary,
        )
        .unwrap();

        let restored = Connection::open(&target_library).unwrap();
        let queue: Value = restored
            .query_row(
                "SELECT value FROM settings WHERE key='nativePlaybackQueue.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();
        let tracks = queue["tracks"].as_array().unwrap();
        assert_eq!(tracks[0]["id"], "youtube:abc123");
        assert_eq!(tracks[0]["assetId"], "asset-1");
        assert!(tracks[0]["localPath"]
            .as_str()
            .is_some_and(|path| Path::new(path).is_file()));

        assert_eq!(tracks[1]["id"], "youtube:other123");
        assert!(tracks[1]["assetId"].is_null());
        assert!(tracks[1]["localPath"].is_null());
    }

    #[test]
    fn preserved_queue_keeps_registered_missing_asset_id_without_trusting_path() {
        let target = tempfile::tempdir().unwrap();
        insert_fixture(target.path());
        let missing_media = std::fs::canonicalize(target.path())
            .unwrap()
            .join("Music")
            .join("Artist")
            .join("Album")
            .join("song.flac");
        std::fs::remove_file(&missing_media).unwrap();
        let wrong_hash_media = target.path().join("wrong-hash.flac");
        std::fs::write(&wrong_hash_media, b"different fixture bytes").unwrap();
        let fixture_db = Connection::open(target.path().join("burrata.sqlite")).unwrap();
        fixture_db
            .execute(
                "INSERT INTO works(id,title,artist) VALUES('work-2','Other','Artist')",
                [],
            )
            .unwrap();
        fixture_db
            .execute(
                "INSERT INTO recordings(id,work_id,version,favorite,disposition,manual_metadata) VALUES('recording-2','work-2','',0,NULL,'{}')",
                [],
            )
            .unwrap();
        fixture_db
            .execute(
                "INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES('youtube:other123','youtube','other123','recording-2','https://youtube.com/watch?v=other123','{}','2026-10-27T12:00:00Z',1)",
                [],
            )
            .unwrap();
        let mut wrong_hash_payload: Value = fixture_db
            .query_row(
                "SELECT payload FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();
        wrong_hash_payload["id"] = Value::String("asset-bad-hash".into());
        wrong_hash_payload["path"] = Value::String(wrong_hash_media.to_string_lossy().into_owned());
        wrong_hash_payload["hash"] = Value::String("a".repeat(64));
        fixture_db
            .execute(
                "INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES('asset-bad-hash','recording-1',?1,?2,?3)",
                params![wrong_hash_media.to_string_lossy(), "a".repeat(64), serde_json::to_string(&wrong_hash_payload).unwrap()],
            )
            .unwrap();

        let mut current = playback_fixture(
            &[
                "youtube:abc123",
                "youtube:other123",
                "fixture:unregistered",
                "youtube:abc123",
                "recording-1",
            ],
            0.0,
        );
        current.queue[0]["assetId"] = Value::String("asset-1".into());
        current.queue[1]["assetId"] = Value::String("asset-1".into());
        current.queue[2]["assetId"] = Value::String("unknown-asset".into());
        current.queue[3]["assetId"] = Value::String("asset-bad-hash".into());
        current.queue[4]["assetId"] = Value::String("asset-1".into());
        let playback_path = target.path().join("current-playback.json");
        write_json(&playback_path, &current).unwrap();

        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: Uuid::new_v4().to_string(),
            current_playback_snapshot: Some(playback_path),
            ..RestoreStateOptions::default()
        };
        reconstruct_staged(None, None, None, None, target.path(), &options).unwrap();

        let connection = Connection::open(target.path().join("burrata.sqlite")).unwrap();
        let queue: Value = connection
            .query_row(
                "SELECT value FROM settings WHERE key='nativePlaybackQueue.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();
        assert_eq!(queue["tracks"][0]["assetId"], "asset-1");
        assert_eq!(queue["tracks"][0]["localPath"], Value::Null);
        assert_eq!(queue["tracks"][0]["localAvailability"], "none");
        assert_eq!(queue["tracks"][1]["assetId"], Value::Null);
        assert_eq!(queue["tracks"][1]["localPath"], Value::Null);
        assert_eq!(queue["tracks"][2]["assetId"], Value::Null);
        assert_eq!(queue["tracks"][2]["localPath"], Value::Null);
        assert_eq!(queue["tracks"][3]["assetId"], "asset-bad-hash");
        assert_eq!(queue["tracks"][3]["localPath"], Value::Null);
        assert_eq!(queue["tracks"][3]["localAvailability"], "none");
        assert_eq!(queue["tracks"][4]["assetId"], "asset-1");
        assert_eq!(queue["tracks"][4]["localPath"], Value::Null);
        assert!(!queue["tracks"][0]
            .as_object()
            .unwrap()
            .values()
            .any(|value| value.as_str() == Some(missing_media.to_string_lossy().as_ref())));
    }

    fn stream_analysis_fixture() -> crate::stream_analysis::WholeTrackAnalysis {
        use crate::stream_analysis::{
            AnalysisResource, BpmMeasurement, WholeTrackAnalysis, ANALYZER_VERSION, SCHEMA_VERSION,
        };
        WholeTrackAnalysis {
            schema_version: SCHEMA_VERSION,
            analyzer_version: ANALYZER_VERSION.into(),
            resource: AnalysisResource::Stream {
                recording_id: "catalog:recording-1".into(),
                provider: "youtube".into(),
                source_identity_sha256: "a".repeat(64),
                audio_variant_sha256: None,
            },
            duration_seconds: 180.0,
            scope: "wholeTrack".into(),
            coverage: 1.0,
            measured_at: "2026-10-25T12:00:00Z".into(),
            bpm: Some(BpmMeasurement {
                bpm: 120.0,
                confidence: 0.85,
                related_tempo_alternative_bpm: Some(60.0),
            }),
            key: None,
            waveform: None,
        }
    }

    #[test]
    fn imported_autotag_jobs_keep_only_inert_candidates_and_require_fresh_retry() {
        let source = serde_json::json!({
            "assetId":"asset-1",
            "inputHash":"a".repeat(64),
            "version":"autotag-v1",
            "status":"matching",
            "attempts":4,
            "nextAttemptAt":"2026-10-25T12:00:00Z",
            "updatedAt":"2026-10-25T11:00:00Z",
            "message":"provider lookup https://example.com/private?token=secret",
            "chosen":{"provider":"musicbrainz","providerId":"recording-1"},
            "approvedFields":["artist","title"],
            "recognition":{"provider":"songrec","artist":"Old Artist","title":"Old Title","isrc":"USABC2600001","excerptCount":2},
            "candidates":[{
                "provider":"musicbrainz",
                "providerId":"recording-1",
                "sourceUrl":"https://musicbrainz.org/recording/recording-1?token=secret",
                "title":"Safe Title",
                "artist":"Safe Artist",
                "score":0.99,
                "durationSeconds":180.0,
                "evidence":["AcoustID audio fingerprint score 0.99"],
                "fields":{"artist":"Safe Artist","title":"Safe Title","provenance":{"sourceUrl":"https://example.com/private?token=secret"}}
            }]
        });
        let safe = sanitize_autotag_backup_payload(&source, "matching");
        assert_eq!(safe["status"], "restore_review");
        assert_eq!(safe["attempts"], 0);
        assert!(safe["chosen"].is_null());
        assert!(safe["recognition"].is_null());
        assert_eq!(safe["approvedFields"], serde_json::json!([]));
        assert_eq!(safe["candidates"][0]["title"], "Safe Title");
        // AutoTagCandidate contains a required Track value with a sourceUrl
        // field. The projection keeps that typed field empty, never its URL.
        assert_eq!(safe["candidates"][0]["sourceUrl"], "");
        assert_eq!(safe["candidates"][0]["fields"]["sourceUrl"], "");
        // Evidence is a required Vec in the typed candidate DTO, so preserve
        // only the empty container and never the imported proof contents.
        assert_eq!(safe["candidates"][0]["evidence"], serde_json::json!([]));
        assert!(!safe.to_string().contains("AcoustID"));
        assert!(!safe.to_string().contains("musicbrainz.org/recording"));
        assert!(safe["candidates"][0]["fields"].get("provenance").is_none());
        assert!(!safe.to_string().contains("token=secret"));
        assert_eq!(restored_autotag_status("queued"), "restore_review");
        assert_eq!(restored_autotag_status("completed"), "completed");
        let decoded: crate::autotag::AutoTagJob = serde_json::from_value(safe).unwrap();
        assert_eq!(decoded.status, "restore_review");
        assert_eq!(decoded.candidates.len(), 1);
        assert_eq!(decoded.candidates[0].title, "Safe Title");
    }

    #[test]
    fn portable_playback_source_projection_keeps_only_valid_typed_identity() {
        let binding = serde_json::json!({
            "provider":"youtube",
            "sourceId":"video_123",
            "matchingLevel":"balanced",
            "sourceHash":"a".repeat(64),
            "sourceUrl":"https://cdn.example/stream?signature=private",
            "candidate":{"url":"https://cdn.example/audio?token=private"},
        });
        assert_eq!(
            sanitize_portable_playback_source(&binding).unwrap(),
            serde_json::json!({
                "provider":"youtube",
                "sourceId":"video_123",
                "matchingLevel":"balanced",
                "sourceHash":"a".repeat(64),
            })
        );
        let mut invalid_id = binding.clone();
        invalid_id["sourceId"] = Value::String("https://example.invalid/video?token=x".into());
        assert!(sanitize_portable_playback_source(&invalid_id).is_none());
        let mut invalid_policy = binding.clone();
        invalid_policy["matchingLevel"] = Value::String("invented".into());
        assert!(sanitize_portable_playback_source(&invalid_policy).is_none());
    }

    #[test]
    fn legacy_asset_payload_without_path_is_repaired_from_verified_sql_path() {
        let target = tempfile::tempdir().unwrap();
        let audio = target.path().join("restored.wav");
        std::fs::write(&audio, b"legacy audio").unwrap();
        let audio_path = audio.to_string_lossy().into_owned();
        let hash = format!("{:x}", Sha256::digest(b"legacy audio"));
        let legacy = serde_json::json!({
            "id":"asset-legacy",
            "recordingId":"recording-legacy",
            "hash":hash,
            "sourceCodec":"wav",
            "finalCodec":"wav",
            "sourceBitrate":null,
            "finalBitrate":null,
            "sampleRate":44100,
            "channels":2,
            "bitDepth":16,
            "durationSeconds":2.0,
            "sizeBytes":12,
            "bitrateOrigin":"format",
            "processing":"complete",
            "track":{
                "id":"youtube:legacy",
                "provider":"youtube",
                "providerId":"legacy",
                "title":"Legacy track",
                "artist":"Artist",
                "favorite":false,
                "sourceUrl":"",
                "localPath":null,
                "assetId":null,
                "localAvailability":"none",
                "provenance":{}
            }
        });
        assert!(legacy.get("path").is_none());
        let repaired = materialize_restored_asset_payload(
            &legacy.to_string(),
            "asset-legacy",
            "recording-legacy",
            &hash,
            &audio_path,
        )
        .unwrap();
        let asset: crate::models::LocalAsset = serde_json::from_value(repaired).unwrap();
        assert_eq!(asset.path, audio_path);
        assert_eq!(asset.track.local_path.as_deref(), Some(audio_path.as_str()));
        assert_eq!(asset.track.asset_id.as_deref(), Some("asset-legacy"));
    }

    #[test]
    fn stream_analysis_backup_projection_preserves_only_valid_safe_records() {
        use crate::stream_analysis::{StreamAnalysisCache, MAX_CACHE_RECORDS, SCHEMA_VERSION};

        let mut valid = serde_json::to_value(stream_analysis_fixture()).unwrap();
        valid["sourceUrl"] = Value::String("https://media.example/signed?token=secret".into());
        valid["headers"] = serde_json::json!({"Authorization":"Bearer private"});
        let mut invalid = serde_json::to_value(stream_analysis_fixture()).unwrap();
        invalid["bpm"]["confidence"] = Value::from(4.0);
        let mut invalid_waveform = serde_json::to_value(stream_analysis_fixture()).unwrap();
        let invalid_bins = vec![serde_json::json!({"min":-0.25,"max":0.25}); 512];
        invalid_waveform["waveform"] = serde_json::json!({
            "analyzerVersion":crate::stream_analysis::STREAM_WAVEFORM_ANALYZER_VERSION,
            "binCount":513,
            "durationSeconds":180.0,
            "bins":invalid_bins,
            "scope":"wholeTrack",
            "complete":true,
            "measuredAt":"2026-10-25T12:00:00Z"
        });
        let valid_bins = vec![
            serde_json::json!({"min":-0.25,"max":0.25});
            crate::stream_analysis::STREAM_WAVEFORM_BINS
        ];
        valid["waveform"] = serde_json::json!({
            "analyzerVersion":crate::stream_analysis::STREAM_WAVEFORM_ANALYZER_VERSION,
            "binCount":crate::stream_analysis::STREAM_WAVEFORM_BINS,
            "durationSeconds":180.0,
            "bins":valid_bins,
            "scope":"wholeTrack",
            "complete":true,
            "measuredAt":"2026-10-25T12:00:00Z"
        });
        let local_asset = serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "analyzerVersion": crate::stream_analysis::ANALYZER_VERSION,
            "resource": {
                "kind":"registeredAsset",
                "assetId":"asset-a",
                "recordingId":"catalog:recording-1",
                "fileHash":"b".repeat(64)
            },
            "durationSeconds":180.0,
            "scope":"wholeTrack",
            "coverage":1.0,
            "measuredAt":"2026-10-25T12:00:00Z",
            "bpm":{"bpm":120.0,"confidence":0.85,"relatedTempoAlternativeBpm":60.0},
            "key":null
        });
        let input = serde_json::json!({
            "schemaVersion":SCHEMA_VERSION,
            "records":[valid, invalid, invalid_waveform, local_asset]
        });
        let clean = sanitize_preference("streamAnalysisCache.v1", input).unwrap();
        let cache: StreamAnalysisCache = serde_json::from_value(clean.clone()).unwrap();
        assert_eq!(cache.schema_version, SCHEMA_VERSION);
        assert_eq!(cache.records.len(), 1);
        assert_eq!(cache.records[0].bpm.as_ref().unwrap().bpm, 120.0);
        assert_eq!(
            cache.records[0].waveform.as_ref().unwrap().bins.len(),
            crate::stream_analysis::STREAM_WAVEFORM_BINS
        );
        assert!(!clean.to_string().contains("media.example"));
        assert!(!clean.to_string().contains("secret"));
        assert!(!clean.to_string().contains("Authorization"));

        let oversized = serde_json::json!({
            "schemaVersion":SCHEMA_VERSION,
            "records":vec![serde_json::to_value(stream_analysis_fixture()).unwrap(); MAX_CACHE_RECORDS + 1]
        });
        assert!(sanitize_preference("streamAnalysisCache.v1", oversized).is_none());
        assert!(sanitize_preference(
            "streamAnalysisCache.v1",
            serde_json::json!({"schemaVersion":99,"records":[]})
        )
        .is_none());
    }

    #[test]
    fn history_mirror_backup_projection_retains_only_bounded_scope_preference() {
        assert!(is_preference_allowlisted("playlistHistoryMirror.v1"));
        let clean = sanitize_preference(
            "playlistHistoryMirror.v1",
            serde_json::json!({
                "schemaVersion":1,
                "recentDays":30,
                "destination":"/private/path",
                "sourceUrl":"https://secret.example"
            }),
        )
        .unwrap();
        assert_eq!(
            clean,
            serde_json::json!({"schemaVersion":1,"recentDays":30})
        );
        assert!(sanitize_preference(
            "playlistHistoryMirror.v1",
            serde_json::json!({"schemaVersion":1,"recentDays":0})
        )
        .is_none());
        assert!(sanitize_preference(
            "playlistHistoryMirror.v1",
            serde_json::json!({"schemaVersion":2,"recentDays":30})
        )
        .is_none());
    }

    #[test]
    fn detected_track_backup_keeps_observations_and_disables_restored_policy() {
        let cache_key = "playingTrackDetectionCache.v1";
        let policy_key = "playingTrackDetectionPolicy.v1";
        assert!(is_preference_allowlisted(cache_key));
        assert!(is_preference_allowlisted(policy_key));
        let resource_key = "a".repeat(64);
        let clean = sanitize_preference(
            cache_key,
            serde_json::json!({
                "version":1,
                "resources":[{
                    "key":resource_key,
                    "lastUsedAt":"2026-10-27T12:00:00Z",
                    "observations":[{
                        "title":"Track A",
                        "artist":"Artist",
                        "detectedAtSeconds":22.0,
                        "lastConfirmedAtSeconds":22.0,
                        "capturedStartSeconds":16.0,
                        "capturedEndSeconds":28.0,
                        "observedAt":"2026-10-27T12:00:00Z",
                        "identifiers":{"isrc":"USABC2600001","shazamId":"12345","appleMusicUrl":"https://private.example/"},
                        "sourceUrl":"https://private.example/?token=secret",
                        "localPath":"/private/audio.flac"
                    }]
                }]
            }),
        )
        .unwrap();
        assert_eq!(
            clean["resources"][0]["observations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            clean["resources"][0]["observations"][0]["detectedAtSeconds"],
            22.0
        );
        assert!(clean.to_string().contains("USABC2600001"));
        assert!(!clean.to_string().contains("appleMusicUrl"));
        assert!(!clean.to_string().contains("private.example"));
        assert!(!clean.to_string().contains("/private/audio.flac"));
        let policy = sanitize_preference(
            policy_key,
            serde_json::json!({
                "version":1,
                "resources":[{
                    "key":"a".repeat(64),
                    "enabled":true,
                    "lastUsedAt":"2026-10-27T12:00:00Z"
                }]
            }),
        )
        .unwrap();
        assert_eq!(policy["resources"][0]["enabled"], false);
        assert!(
            sanitize_preference(cache_key, serde_json::json!({"version":99,"resources":[]}))
                .is_none()
        );
    }

    fn lyric_choice_fixture(source_url: &str) -> Value {
        serde_json::json!({
            "candidate": {
                "id": "990031",
                "trackName": "Synthetic Song",
                "artistName": "Synthetic Artist",
                "albumName": "Synthetic Album",
                "durationSeconds": 180.0,
                "hasSynced": true,
                "automaticMatch": true,
                "plainTextMatch": false,
                "explanation": "untrusted explanation",
                "sourceUrl": source_url,
                "headers": {"Authorization": "Bearer private"}
            },
            "record": {
                "trackName": "Synthetic Song",
                "artistName": "Synthetic Artist",
                "albumName": "Synthetic Album",
                "duration": 180.0,
                "instrumental": false,
                "syncedLyrics": "[00:01.00]First synthetic line\n[00:04.00]Second synthetic line",
                "plainLyrics": null,
                "sourceUrl": source_url,
                "apiToken": "private"
            }
        })
    }

    #[test]
    fn lyrics_choice_projection_rebuilds_public_url_and_preserves_source_hash() {
        let source_url = "https://evil.example/api/get/990031?token=private";
        let preference_key = format!("lyrics.choice.v1.{}", "a".repeat(64));
        assert!(is_preference_allowlisted(&preference_key));
        assert!(!is_preference_allowlisted(&format!(
            "lyrics.choice.v1.{}",
            "A".repeat(64)
        )));
        let projected =
            sanitize_preference(&preference_key, lyric_choice_fixture(source_url)).unwrap();
        assert_eq!(
            projected["candidate"]["sourceUrl"],
            "https://lrclib.net/api/get/990031"
        );
        assert!(!projected.to_string().contains("evil.example"));
        assert!(!projected.to_string().contains("private"));
        assert_eq!(
            projected["candidate"]["explanation"],
            "Recording metadata or verified artist alias matches"
        );

        let request = crate::lyrics::LyricsRequest {
            track_name: "Synthetic Song".into(),
            artist_name: "Synthetic Artist".into(),
            album_name: Some("Synthetic Album".into()),
            duration_seconds: 180.0,
        };
        let original: crate::lyrics::ApiRecord =
            serde_json::from_value(lyric_choice_fixture(source_url)["record"].clone()).unwrap();
        let restored: crate::lyrics::ApiRecord =
            serde_json::from_value(projected["record"].clone()).unwrap();
        let original_document = crate::lyrics::classify_record(&request, original);
        let restored_document = crate::lyrics::classify_record(&request, restored);
        assert_eq!(original_document.status, "synced");
        assert_eq!(restored_document.status, "synced");
        assert_eq!(original_document.source_hash, restored_document.source_hash);
        assert_eq!(projected["candidate"]["id"], "990031");

        // Rehydrate through the real playback-scoped remembered-choice path.
        let snapshot = serde_json::json!({
            "engine": "native",
            "status": "paused",
            "track": {
                "id": "lyrics-backup-fixture",
                "title": "Synthetic Song",
                "artist": "Synthetic Artist",
                "album": "Synthetic Album",
                "durationSeconds": 180.0
            }
        });
        let (real_scope, _, _) = crate::lyrics_discovery::context(&snapshot, None).unwrap();
        let real_preference_key = format!("lyrics.choice.v1.{real_scope}");
        let real_projection =
            sanitize_preference(&real_preference_key, lyric_choice_fixture(source_url)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store =
            crate::storage::Store::open(&directory.path().join("lyrics-backup.sqlite")).unwrap();
        store
            .set_json_preference(&real_preference_key, &real_projection)
            .unwrap();
        let remembered = crate::lyrics_discovery::remembered(&store, &snapshot, None).unwrap();
        assert_eq!(remembered.document.status, "synced");
        assert_eq!(
            remembered.document.source_hash,
            original_document.source_hash
        );
        assert_eq!(remembered.candidates[0].id, "990031");
        assert_eq!(
            remembered.candidates[0].source_url,
            "https://lrclib.net/api/get/990031"
        );

        // Explicit related-version choices are displayed untimed, but their
        // source hash remains the key used by manual timing adjustments.
        let mut manual_choice = lyric_choice_fixture(source_url);
        manual_choice["candidate"]["automaticMatch"] = Value::Bool(false);
        let projected_manual = sanitize_preference(&preference_key, manual_choice.clone()).unwrap();
        let original_manual: crate::lyrics::ApiRecord =
            serde_json::from_value(manual_choice["record"].clone()).unwrap();
        let restored_manual: crate::lyrics::ApiRecord =
            serde_json::from_value(projected_manual["record"].clone()).unwrap();
        let original_manual_document = crate::lyrics::untimed_record(&request, original_manual);
        let restored_manual_document = crate::lyrics::untimed_record(&request, restored_manual);
        assert_eq!(original_manual_document.status, "unsynced");
        assert_eq!(restored_manual_document.status, "unsynced");
        assert_eq!(
            original_manual_document.source_hash,
            restored_manual_document.source_hash
        );

        let mut malicious_body = lyric_choice_fixture(source_url);
        let inert_lyric_text = "[00:01.00] line https://evil.example/?token=secret";
        malicious_body["record"]["syncedLyrics"] = Value::String(inert_lyric_text.into());
        let inert_projection = sanitize_preference(&preference_key, malicious_body).unwrap();
        assert_eq!(inert_projection["record"]["syncedLyrics"], inert_lyric_text);
        assert_eq!(
            inert_projection["candidate"]["sourceUrl"],
            "https://lrclib.net/api/get/990031"
        );
        let mut wrong_provider = projected;
        wrong_provider["provider"] = Value::String("untrusted".into());
        assert!(sanitize_preference(&preference_key, wrong_provider).is_none());
    }

    #[test]
    fn playlist_mirroring_setting_round_trips_and_legacy_default_is_merge_safe() {
        let source = tempfile::tempdir().unwrap();
        insert_fixture(source.path());
        let library = source.path().join("burrata.sqlite");
        let connection = Connection::open(&library).unwrap();
        let raw: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key='application'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut application: Value = serde_json::from_str(&raw).unwrap();
        application["playlistMirroringEnabled"] = Value::Bool(false);
        application["compactPlaybackBar"] = Value::Bool(false);
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key='application'",
                [application.to_string()],
            )
            .unwrap();
        drop(connection);

        // This is the durable settings.json component used by full and
        // settings-only backup archives.
        let prepared =
            prepare_backup(source.path(), Uuid::new_v4(), BackupStateOptions::default()).unwrap();
        let archive: SettingsArchive = read_json(&prepared.settings_json).unwrap();
        assert_eq!(archive.application["playlistMirroringEnabled"], false);
        assert_eq!(archive.application["compactPlaybackBar"], false);
        let preview = inspect_settings_archive(&prepared.settings_json).unwrap();
        assert!(preview
            .application_fields
            .contains(&"playlistMirroringEnabled".into()));
        let projection = project_settings_archive(
            &prepared.settings_json,
            &SettingsImportSelection {
                application: true,
                ..SettingsImportSelection::default()
            },
        )
        .unwrap();
        assert_eq!(
            projection
                .application_fields
                .get("playlistMirroringEnabled"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            projection.application_fields.get("compactPlaybackBar"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            serde_json::to_value(projection.application_settings.unwrap()).unwrap()
                ["playlistMirroringEnabled"],
            false
        );

        // Older archives omitted the setting: Replace/default migration uses
        // enabled=true, while sparse Merge leaves a target's explicit value
        // untouched. A malformed present value is rejected on Replace.
        let legacy = serde_json::json!({"theme":"light"});
        assert_eq!(
            serde_json::to_value(application_from_archive(&legacy).unwrap()).unwrap()
                ["playlistMirroringEnabled"],
            true
        );
        assert!(!portable_application_fields(&legacy).contains_key("playlistMirroringEnabled"));
        assert!(!portable_application_fields(&legacy).contains_key("compactPlaybackBar"));
        assert_eq!(
            serde_json::to_value(application_from_archive(&legacy).unwrap()).unwrap()
                ["compactPlaybackBar"],
            false
        );

        let malformed = serde_json::json!({"playlistMirroringEnabled":"false"});
        assert!(!portable_application_fields(&malformed).contains_key("playlistMirroringEnabled"));
        assert!(application_from_archive(&malformed).is_err());

        let legacy_path = source.path().join("legacy-application-settings.json");
        write_json(
            &legacy_path,
            &SettingsArchive {
                format: "org.burrata.settings".into(),
                schema_version: SETTINGS_SCHEMA_VERSION,
                snapshot_id: Uuid::new_v4().to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                application: legacy,
                ..SettingsArchive::default()
            },
        )
        .unwrap();
        let mut connection = Connection::open(&library).unwrap();
        let current_json: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key='application'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut current: Value = serde_json::from_str(&current_json).unwrap();
        current["playlistMirroringEnabled"] = Value::Bool(false);
        current["compactPlaybackBar"] = Value::Bool(true);
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key='application'",
                [current.to_string()],
            )
            .unwrap();
        let tx = connection.transaction().unwrap();
        merge_settings_component(
            &tx,
            &legacy_path,
            RestoreMode::Merge,
            &HashMap::new(),
            &mut RestoreStateSummary::default(),
        )
        .unwrap();
        tx.commit().unwrap();
        let merged_json: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key='application'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let merged: Value = serde_json::from_str(&merged_json).unwrap();
        assert_eq!(merged["playlistMirroringEnabled"], false);
        assert_eq!(merged["compactPlaybackBar"], true);
    }

    #[test]
    fn playlist_mirror_policy_round_trips_and_reconciles_collection_references() {
        let source = tempfile::tempdir().unwrap();
        insert_fixture(source.path());
        let library = source.path().join("burrata.sqlite");
        let policy = serde_json::json!({
            "schemaVersion":1,
            "playlists":{
                "playlist-1":{"included":false,"includeMediaLinks":false,"includeYtdlpSearch":null},
                "favorites":{"included":true,"includeMediaLinks":null,"includeYtdlpSearch":false},
                "history":{"included":false,"includeMediaLinks":true,"includeYtdlpSearch":null},
                "missing-playlist":{"included":true,"includeMediaLinks":null,"includeYtdlpSearch":null}
            }
        });
        Connection::open(&library)
            .unwrap()
            .execute(
                "INSERT INTO settings(key,value) VALUES('playlistMirrorOptions.v1',?1)",
                [policy.to_string()],
            )
            .unwrap();

        let prepared =
            prepare_backup(source.path(), Uuid::new_v4(), BackupStateOptions::default()).unwrap();
        let archive: SettingsArchive = read_json(&prepared.settings_json).unwrap();
        assert_eq!(
            archive.preferences.get("playlistMirrorOptions.v1"),
            Some(&policy)
        );
        let projection = project_settings_archive(
            &prepared.settings_json,
            &SettingsImportSelection {
                preference_keys: vec!["playlistMirrorOptions.v1".into()],
                ..SettingsImportSelection::default()
            },
        )
        .unwrap();
        assert_eq!(
            projection.preferences.get("playlistMirrorOptions.v1"),
            Some(&policy)
        );

        let mut connection = Connection::open(&library).unwrap();
        let tx = connection.transaction().unwrap();
        let reconciled = reconcile_preference_references(
            &tx,
            "playlistMirrorOptions.v1",
            policy,
            &HashMap::new(),
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(reconciled["playlists"].get("playlist-1").is_some());
        assert!(reconciled["playlists"].get("favorites").is_some());
        assert!(reconciled["playlists"].get("history").is_some());
        assert!(reconciled["playlists"].get("missing-playlist").is_none());

        let sanitized = sanitize_preference(
            "playlistMirrorOptions.v1",
            serde_json::json!({
                "schemaVersion":1,
                "playlists":{
                    "favorites":{"included":false,"includeMediaLinks":false,"includeYtdlpSearch":null},
                    "history":{"includeMediaLinks":null,"includeYtdlpSearch":false},
                    "playlist-1":{"included":"no","includeMediaLinks":true,"includeYtdlpSearch":false}
                }
            }),
        )
        .unwrap();
        assert_eq!(sanitized["playlists"]["favorites"]["included"], false);
        assert_eq!(
            sanitized["playlists"]["favorites"]["includeMediaLinks"],
            false
        );
        assert_eq!(
            sanitized["playlists"]["favorites"]["includeYtdlpSearch"],
            Value::Null
        );
        assert_eq!(sanitized["playlists"]["history"]["included"], true);
        assert!(sanitized["playlists"].get("playlist-1").is_none());
        assert!(sanitize_preference(
            "playlistMirrorOptions.v1",
            serde_json::json!({"schemaVersion":2,"playlists":{}})
        )
        .is_none());
        assert!(sanitize_preference(
            "playlistMirrorOptions.v1",
            serde_json::json!({"schemaVersion":1,"playlists":{"x":{"included":1}}})
        )
        .unwrap()["playlists"]
            .as_object()
            .unwrap()
            .is_empty());

        let target_policy = serde_json::json!({
            "schemaVersion":1,
            "playlists":{"favorites":{"included":false,"includeMediaLinks":null,"includeYtdlpSearch":null}}
        });
        let settings_path = source.path().join("legacy-settings.json");
        write_json(
            &settings_path,
            &SettingsArchive {
                format: "org.burrata.settings".into(),
                schema_version: SETTINGS_SCHEMA_VERSION,
                snapshot_id: Uuid::new_v4().to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                application: serde_json::json!({"theme":"light"}),
                ..SettingsArchive::default()
            },
        )
        .unwrap();
        connection
            .execute(
                "INSERT INTO settings(key,value) VALUES('playlistMirrorOptions.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [target_policy.to_string()],
            )
            .unwrap();
        let tx = connection.transaction().unwrap();
        merge_settings_component(
            &tx,
            &settings_path,
            RestoreMode::Merge,
            &HashMap::new(),
            &mut RestoreStateSummary::default(),
        )
        .unwrap();
        tx.commit().unwrap();
        let retained: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key='playlistMirrorOptions.v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&retained).unwrap(),
            target_policy
        );
    }

    #[test]
    fn standalone_settings_preview_and_projection_exclude_device_paths_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("settings.json");
        let archive = SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: SETTINGS_SCHEMA_VERSION,
            snapshot_id: Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            application: serde_json::json!({
                "theme":"light",
                "musicDirectory":"/private/Music",
                "apiToken":"do-not-import"
            }),
            preferences: BTreeMap::from([
                (
                    "includeRadioSongsInPlaybackHistory.v1".into(),
                    Value::Bool(true),
                ),
                ("private.token".into(), Value::String("secret".into())),
            ]),
            frontend_preferences: BTreeMap::from([
                ("burrata.waveform-enabled.v1".into(), Value::Bool(true)),
                (
                    "burrata.stream-media-analysis-enabled.v1".into(),
                    Value::Bool(true),
                ),
                (
                    "burrata.playlist.include-ytdlp-search.v1".into(),
                    Value::Bool(true),
                ),
                (
                    "burrata.private-origin.v1".into(),
                    Value::String("https://private.invalid".into()),
                ),
            ]),
            asset_path_hints: vec![AssetPathHint {
                asset_id: "asset-1".into(),
                root_key: "music".into(),
                relative_path: "Artist/track.flac".into(),
            }],
            projection_summary: SettingsProjectionSummary::default(),
        };
        write_json(&archive_path, &archive).unwrap();

        let preview = inspect_settings_archive(&archive_path).unwrap();
        assert!(preview.application_fields.contains(&"theme".into()));
        assert_eq!(preview.excluded_application_fields, 2);
        assert_eq!(
            preview.preference_keys,
            vec!["includeRadioSongsInPlaybackHistory.v1"]
        );
        assert_eq!(preview.excluded_preference_keys, 1);
        assert!(preview
            .frontend_preference_keys
            .contains(&"burrata.waveform-enabled.v1".into()));
        assert!(preview
            .frontend_preference_keys
            .contains(&"burrata.stream-media-analysis-enabled.v1".into()));
        assert!(preview
            .frontend_preference_keys
            .contains(&"burrata.playlist.include-ytdlp-search.v1".into()));
        assert_eq!(preview.excluded_frontend_preference_keys, 1);
        assert_eq!(preview.excluded_asset_path_hints, 1);
        assert!(preview.archive_bytes > 0);

        let projection = project_settings_archive(
            &archive_path,
            &SettingsImportSelection {
                application: true,
                preference_keys: vec![
                    "includeRadioSongsInPlaybackHistory.v1".into(),
                    "private.token".into(),
                ],
                frontend_preference_keys: vec![
                    "burrata.waveform-enabled.v1".into(),
                    "burrata.stream-media-analysis-enabled.v1".into(),
                    "burrata.private-origin.v1".into(),
                ],
            },
        )
        .unwrap();
        assert_eq!(
            projection.application_fields.get("theme"),
            Some(&Value::String("light".into()))
        );
        assert!(!projection.application_fields.contains_key("musicDirectory"));
        assert_eq!(projection.preferences.len(), 1);
        assert_eq!(
            projection.frontend_preferences,
            BTreeMap::from([
                ("burrata.waveform-enabled.v1".into(), Value::Bool(true)),
                (
                    "burrata.stream-media-analysis-enabled.v1".into(),
                    Value::Bool(true),
                ),
            ])
        );
        assert!(projection.application_settings.is_some());
        assert_eq!(projection.summary.application_fields, 1);
        assert_eq!(projection.summary.preferences, 1);
        assert_eq!(projection.summary.frontend_preferences, 2);
    }

    #[test]
    fn merge_preview_accepts_first_run_profile_without_application_settings_row() {
        let current = tempfile::tempdir().unwrap();
        let library = current.path().join("burrata.sqlite");
        drop(Store::open(&library).unwrap());
        Connection::open(&library)
            .unwrap()
            .execute("DELETE FROM settings WHERE key='application'", [])
            .unwrap();

        let settings_path = current.path().join("incoming-settings.json");
        write_json(
            &settings_path,
            &SettingsArchive {
                format: "org.burrata.settings".into(),
                schema_version: SETTINGS_SCHEMA_VERSION,
                snapshot_id: Uuid::new_v4().to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                application: serde_json::json!({"theme":"light"}),
                ..SettingsArchive::default()
            },
        )
        .unwrap();
        let archive: SettingsArchive = read_json(&settings_path).unwrap();
        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: archive.snapshot_id,
            include_library: false,
            include_history: false,
            include_settings: true,
            include_playback: false,
            ..RestoreStateOptions::default()
        };

        // This is the same merge-preview path used when inspecting a backup
        // against a first-run profile that has no persisted `application` row.
        preview_restore(
            None,
            None,
            Some(&settings_path),
            None,
            Some(current.path()),
            &options,
        )
        .unwrap();
    }

    #[test]
    fn production_scratch_variants_keep_prepared_and_preview_files_under_stage() {
        let source = tempfile::tempdir().unwrap();
        insert_fixture(source.path());
        let library = source.path().join("burrata.sqlite");
        let history = source.path().join("playback-history.sqlite");
        let scratch_parent = tempfile::tempdir().unwrap();

        let prepared = prepare_from_snapshots_in(
            &library,
            &history,
            Uuid::new_v4(),
            BackupStateOptions::default(),
            scratch_parent.path(),
        )
        .unwrap();
        assert!(prepared.directory.path().starts_with(scratch_parent.path()));

        let restore_options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: prepared.snapshot_id.to_string(),
            include_library: true,
            include_history: true,
            include_settings: true,
            include_playback: true,
            ..RestoreStateOptions::default()
        };
        let _preview = preview_restore_in(
            Some(&prepared.library_sqlite),
            Some(&prepared.history_sqlite),
            Some(&prepared.settings_json),
            Some(&prepared.playback_json),
            Some(source.path()),
            &restore_options,
            scratch_parent.path(),
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(scratch_parent.path()).unwrap().count(), 1);

        let _selected_preview = preview_restore_selection_in(
            Some(&prepared.library_sqlite),
            Some(&prepared.history_sqlite),
            Some(&prepared.settings_json),
            Some(&prepared.playback_json),
            Some(source.path()),
            &prepared.snapshot_id.to_string(),
            RestoreMode::Merge,
            &["library".into(), "history".into()],
            Vec::new(),
            scratch_parent.path(),
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(scratch_parent.path()).unwrap().count(), 1);
    }

    #[test]
    fn backup_json_writer_and_reader_share_the_same_component_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounded.json");
        let exactly_at_limit = Value::String("x".repeat(MAX_JSON_BYTES - 2));
        write_json(&path, &exactly_at_limit).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            MAX_JSON_BYTES as u64
        );
        let read_back: Value = read_json(&path).unwrap();
        assert_eq!(read_back, exactly_at_limit);

        let over_limit = Value::String("x".repeat(MAX_JSON_BYTES - 1));
        let error = write_json(&path, &over_limit).unwrap_err();
        assert!(error.to_string().contains(&format!(
            "Backup JSON component exceeds the {MAX_JSON_BYTES}-byte limit"
        )));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            MAX_JSON_BYTES as u64
        );
        std::fs::write(&path, serde_json::to_vec(&over_limit).unwrap()).unwrap();
        let read_error = read_json::<Value>(&path).unwrap_err();
        assert!(read_error.to_string().contains(&format!(
            "Backup JSON component exceeds the {MAX_JSON_BYTES}-byte limit"
        )));
    }

    fn insert_fixture(data_dir: &Path) -> (String, String, String, String) {
        std::fs::create_dir_all(data_dir).unwrap();
        // Production path consumers use canonical registered paths. On macOS,
        // tempfile may return /var/folders/... even though the real path is
        // /private/var/folders/....
        let data_dir = std::fs::canonicalize(data_dir).unwrap();
        let library = data_dir.join("burrata.sqlite");
        drop(Store::open(&library).unwrap());
        let music = data_dir.join("Music");
        let media = music.join("Artist").join("Album").join("song.flac");
        std::fs::create_dir_all(media.parent().unwrap()).unwrap();
        std::fs::write(&media, b"fixture audio bytes").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"fixture audio bytes"));
        let connection = Connection::open(&library).unwrap();
        let mut app = serde_json::to_value(Settings::default()).unwrap();
        app["musicDirectory"] = Value::String(music.to_string_lossy().into_owned());
        app["theme"] = Value::String("light".into());
        connection
            .execute(
                "INSERT INTO works(id,title,artist) VALUES('work-1','Song','Artist')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO recordings(id,work_id,version,favorite,disposition,manual_metadata) VALUES('recording-1','work-1','Live',1,NULL,?1)", [r#"{"title":"Corrected Song","provenance":{"manualFields":{"title":true}}}"#]).unwrap();
        connection.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES('youtube:abc123','youtube','abc123','recording-1','https://youtube.com/watch?v=abc123&token=private','{\"title\":\"Song\",\"previewUrl\":\"https://media.example/stream?sig=private\",\"artist\":\"Artist\",\"provenance\":{\"portablePlaybackSource\":{\"provider\":\"youtube\",\"sourceId\":\"abc123\",\"matchingLevel\":\"exact\",\"sourceHash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"sourceUrl\":\"https://cdn.example/video?signature=private\",\"candidate\":{\"url\":\"https://cdn.example/stream?token=private\"}}}}','2026-10-25T00:00:00Z',1)", []).unwrap();
        connection
            .execute(
                "INSERT INTO collections(id,name) VALUES('playlist-1','Set')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO collection_members(collection_id,recording_id,position) VALUES('playlist-1','recording-1',0)", []).unwrap();
        connection
            .execute(
                "INSERT INTO playlist_ordered_collections(collection_id) VALUES('playlist-1')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO playlist_ordered_entries(collection_id,position,listing_id,occurrence_id) VALUES('playlist-1',0,'youtube:abc123','11111111-1111-4111-8111-111111111111'),('playlist-1',1,'youtube:abc123','11111111-1111-4111-8111-111111111112')", []).unwrap();
        let asset_payload = serde_json::json!({
            "id":"asset-1",
            "recordingId":"recording-1",
            "path":media,
            "hash":hash,
            "sourceCodec":"flac",
            "finalCodec":"flac",
            "sourceBitrate":null,
            "finalBitrate":null,
            "sampleRate":44100,
            "channels":2,
            "bitDepth":16,
            "durationSeconds":180.0,
            "sizeBytes":19,
            "bitrateOrigin":"format",
            "processing":"complete",
            "track":{
                "id":"youtube:abc123",
                "provider":"youtube",
                "providerId":"abc123",
                "title":"Song",
                "artist":"Artist",
                "favorite":false,
                "sourceUrl":"https://youtube.com/watch?v=abc123&token=private",
                "localPath":"/secret/path",
                "assetId":"asset-1",
                "localAvailability":"available",
                "provenance":{"portablePlaybackSource":{"provider":"youtube","sourceId":"abc123","matchingLevel":"exact","sourceHash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sourceUrl":"https://cdn.example/video?signature=private","candidate":{"url":"https://cdn.example/stream?token=private"}}}
            }
        })
        .to_string();
        connection.execute("INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES('asset-1','recording-1',?1,?2,?3)", params![media.to_string_lossy(),hash,asset_payload]).unwrap();
        connection.execute("INSERT INTO provider_cache(namespace,key,payload,expires_at) VALUES('private','cache-key','{}','2099-01-01')", []).unwrap();
        let job_payload = serde_json::to_string(&crate::models::DownloadJob {
            id: "job-1".into(),
            track: crate::models::Track {
                id: "youtube:abc123".into(),
                provider: "youtube".into(),
                provider_id: "abc123".into(),
                source_url: "https://www.youtube.com/watch?v=abc123".into(),
                artist: "Artist".into(),
                title: "Song".into(),
                ..crate::models::Track::default()
            },
            candidate: crate::models::Candidate {
                id: "youtube:abc123".into(),
                provider: "youtube".into(),
                url: "https://www.youtube.com/watch?v=abc123".into(),
                title: "Song".into(),
                permission: "unknown".into(),
                confidence: "low".into(),
                ..crate::models::Candidate::default()
            },
            state: "queued".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: "2026-10-25T00:00:00Z".into(),
            output_profile: "source".into(),
            disposition: Default::default(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
            download_layout: Default::default(),
            playlist_context: None,
        })
        .unwrap();
        connection
            .execute(
                "INSERT INTO download_jobs(id,state,payload) VALUES('job-1','queued',?1)",
                [job_payload],
            )
            .unwrap();
        let album_resource = crate::loudness::LoudnessResourceIdentity::album(
            vec![
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "asset-1".into(),
                    file_hash: hash.clone(),
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "asset-2".into(),
                    file_hash: "a".repeat(64),
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
            ],
            vec![1.25],
        );
        let mut saved_queue = serde_json::json!({"tracks":[{"id":"youtube:abc123","provider":"youtube","providerId":"abc123","sourceUrl":"https://youtube.com/watch?v=abc123&token=private","artist":"Artist","title":"Song","assetId":"asset-1","provenance":{"cookie":"private"}}],"entryIds":["entry-1"],"queueRevision":9,"queueLoudnessContexts":{"entry-1":album_resource,"orphan-entry":{"scope":"track"}}});
        saved_queue["queueLoudnessContexts"]["missing-entry"] =
            serde_json::to_value(&album_resource).unwrap();
        connection.execute("INSERT INTO settings(key,value) VALUES('application',?1),('connections',?2),('playlistPreferences.v1',?3),('loudness.v1',?4),('nativePlaybackQueue.v1',?5),('nativePlaybackSession.v1',?6),('playlistHierarchy.v1',?7)", params![app.to_string(),r#"{"discogsToken":"secret"}"#,r#"{"favoritesPlaylistId":"playlist-1","shortcuts":["playlist-1"],"shortcutBindings":["Alt+1"]}"#,r#"{"mode":"album","targetLufs":-18.0,"ceilingDbtp":-1.0,"maximumBoostDb":6.0}"#,serde_json::to_string(&saved_queue).unwrap(),r#"{"tracks":[{"id":"youtube:abc123"}],"entryIds":["entry-1"],"index":0,"currentEntryId":"entry-1","queueRevision":9,"position":12.5,"volume":0.7,"repeat":"all","shuffle":true,"status":"playing"}"#,r#"{"schemaVersion":1,"folders":[],"playlists":[{"collectionId":"playlist-1","folderId":null,"position":0,"sections":[],"occurrences":[{"id":"11111111-1111-4111-8111-111111111111","sectionId":null,"note":"First duplicate"},{"id":"11111111-1111-4111-8111-111111111112","sectionId":null,"note":"Second duplicate"}]}]}"#]).unwrap();
        connection
            .execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)",
                params![
                    format!("lyrics.choice.v1.{}", "b".repeat(64)),
                    serde_json::to_string(&lyric_choice_fixture(
                        "https://evil.example/api/get/990031?token=private"
                    ))
                    .unwrap()
                ],
            )
            .unwrap();
        drop(connection);
        let history_path = data_dir.join("playback-history.sqlite");
        drop(crate::playback_history::PlaybackHistory::open(&history_path).unwrap());
        let history = Connection::open(&history_path).unwrap();
        history.execute("INSERT INTO playback_history(session_id,played_at,provider,track_json,chapter_json) VALUES('session-1','2026-10-24T12:00:00Z','youtube',?1,NULL)", [r#"{"id":"youtube:abc123","provider":"youtube","providerId":"abc123","sourceUrl":"https://youtube.com/watch?v=abc123&token=private","artist":"Artist","title":"Song","provenance":{"secret":"private"}}"#]).unwrap();
        (
            "playlist-1".into(),
            "recording-1".into(),
            hash,
            media.to_string_lossy().into_owned(),
        )
    }

    #[test]
    fn populated_merge_rebinds_current_token_assets_and_repeat_merge_keeps_them_available() {
        let current = tempfile::tempdir().unwrap();
        let (_, _, current_hash, current_media) = insert_fixture(current.path());
        let current_library = current.path().join("burrata.sqlite");
        let incoming_source = tempfile::tempdir().unwrap();
        let (_, _, incoming_hash, incoming_media) = insert_fixture(incoming_source.path());
        let incoming_library = incoming_source.path().join("burrata.sqlite");
        let incoming_history = incoming_source.path().join("playback-history.sqlite");
        let listing_track = serde_json::to_string(&crate::models::Track {
            id: "youtube:abc123".into(),
            provider: "youtube".into(),
            provider_id: "abc123".into(),
            source_url: "https://www.youtube.com/watch?v=abc123".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(180.0),
            ..crate::models::Track::default()
        })
        .unwrap();
        for database in [&current_library, &incoming_library] {
            Connection::open(database)
                .unwrap()
                .execute(
                    "UPDATE catalog_listings SET metadata=?1 WHERE id='youtube:abc123'",
                    [&listing_track],
                )
                .unwrap();
        }
        // Match the canonical music root persisted by insert_fixture. On
        // macOS tempfile's /var path is an alias for /private/var, and an
        // otherwise valid sibling asset would be classified as external if
        // this test constructs its path from the noncanonical alias.
        let incoming_music_root =
            std::fs::canonicalize(incoming_source.path().join("Music")).unwrap();
        let second_media = incoming_music_root
            .join("Artist")
            .join("Album")
            .join("second.flac");
        std::fs::copy(&incoming_media, &second_media).unwrap();
        let incoming_connection = Connection::open(&incoming_library).unwrap();
        let first_payload: String = incoming_connection
            .query_row(
                "SELECT payload FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut second_payload = parse_bounded_json(&first_payload).unwrap();
        second_payload["id"] = Value::String("asset-2".into());
        second_payload["path"] = Value::String(second_media.to_string_lossy().into_owned());
        second_payload["hash"] = Value::String(incoming_hash.clone());
        second_payload["track"]["assetId"] = Value::String("asset-2".into());
        second_payload["track"]["localPath"] =
            Value::String(second_media.to_string_lossy().into_owned());
        incoming_connection
            .execute(
                "INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES('asset-2','recording-1',?1,?2,?3)",
                params![second_media.to_string_lossy(), incoming_hash, second_payload.to_string()],
            )
            .unwrap();
        drop(incoming_connection);

        // The target has the same asset identity, but its old file is gone.
        // The incoming archive can safely repair it from the separately
        // reviewed, hash-matching media mapping.
        let current_connection = Connection::open(&current_library).unwrap();
        let current_payload: String = current_connection
            .query_row(
                "SELECT payload FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let stale_current_media = current.path().join("no-longer-present.flac");
        let mut stale_payload = parse_bounded_json(&current_payload).unwrap();
        stale_payload["id"] = Value::String("asset-2".into());
        stale_payload["path"] = Value::String(stale_current_media.to_string_lossy().into_owned());
        stale_payload["hash"] = Value::String(current_hash.clone());
        stale_payload["track"]["assetId"] = Value::String("asset-2".into());
        stale_payload["track"]["localPath"] =
            Value::String(stale_current_media.to_string_lossy().into_owned());
        current_connection
            .execute(
                "INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES('asset-2','recording-1',?1,?2,?3)",
                params![stale_current_media.to_string_lossy(), current_hash, stale_payload.to_string()],
            )
            .unwrap();
        let tokenized_current_media = format!("{ROOT_TOKEN_PREFIX}music/Artist/Album/song.flac");
        let mut tokenized_payload = parse_bounded_json(&current_payload).unwrap();
        tokenized_payload["path"] = Value::String(tokenized_current_media.clone());
        tokenized_payload["track"]["localPath"] = Value::String(tokenized_current_media.clone());
        tokenized_payload["track"]["localAvailability"] = Value::String("none".into());
        current_connection
            .execute(
                "UPDATE local_assets SET path=?1,payload=?2 WHERE id='asset-1'",
                params![tokenized_current_media, tokenized_payload.to_string()],
            )
            .unwrap();
        drop(current_connection);
        let scratch = tempfile::tempdir().unwrap();
        let incoming = prepare_from_snapshots_in(
            &incoming_library,
            &incoming_history,
            Uuid::new_v4(),
            BackupStateOptions::default(),
            scratch.path(),
        )
        .unwrap();

        let target_snapshot = prepare_from_snapshots_in(
            &current_library,
            &current.path().join("playback-history.sqlite"),
            Uuid::new_v4(),
            BackupStateOptions::default(),
            scratch.path(),
        )
        .unwrap();
        let target_snapshot_connection = Connection::open(&target_snapshot.library_sqlite).unwrap();
        let sanitized_current_path: String = target_snapshot_connection
            .query_row(
                "SELECT path FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            sanitized_current_path,
            format!("{ROOT_TOKEN_PREFIX}music/Artist/Album/song.flac")
        );
        drop(target_snapshot_connection);
        let stage = tempfile::tempdir().unwrap();
        let target_database = stage.path().join("burrata.sqlite");
        std::fs::copy(&target_snapshot.library_sqlite, &target_database).unwrap();
        let remapped_root = stage.path().join("restored-media");
        std::fs::create_dir_all(remapped_root.join("Artist").join("Album")).unwrap();
        let remapped_root = std::fs::canonicalize(remapped_root).unwrap();
        let current_media_path = PathBuf::from(&current_media);
        let restored_duplicate_media = remapped_root.join("Artist").join("Album").join("song.flac");
        std::fs::copy(&incoming_media, &restored_duplicate_media).unwrap();
        let restored_incoming_media = remapped_root
            .join("Artist")
            .join("Album")
            .join("second.flac");
        std::fs::create_dir_all(restored_incoming_media.parent().unwrap()).unwrap();
        std::fs::copy(&second_media, &restored_incoming_media).unwrap();
        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: incoming.snapshot_id.to_string(),
            path_mappings: vec![PathMapping {
                root_key: "music".into(),
                destination: remapped_root.clone(),
            }],
            current_asset_paths: current_asset_paths_from_snapshot(&current_library).unwrap(),
            current_music_root: current_music_root_from_snapshot(&current_library).unwrap(),
            include_library: true,
            ..RestoreStateOptions::default()
        };

        // Without a destination mapping, the populated target still retains
        // its verified current asset. The second asset has neither a valid
        // current file nor an incoming mapping, so it remains unavailable.
        let missing_stage = tempfile::tempdir().unwrap();
        let missing_database = missing_stage.path().join("burrata.sqlite");
        std::fs::copy(&target_snapshot.library_sqlite, &missing_database).unwrap();
        let mut missing_options = options.clone();
        missing_options.path_mappings.clear();
        let missing_summary = reconstruct_staged(
            Some(&incoming.library_sqlite),
            None,
            None,
            None,
            missing_stage.path(),
            &missing_options,
        )
        .unwrap();
        assert!(missing_summary.unavailable_assets > 0);

        let first = reconstruct_staged(
            Some(&incoming.library_sqlite),
            None,
            None,
            None,
            stage.path(),
            &options,
        )
        .unwrap();
        assert!(first.skipped_conflicts >= 1);
        assert_eq!(first.unavailable_assets, 0);
        let reopened = Store::open(&target_database).unwrap();
        reopened.ensure_downloads_collection().unwrap();
        reopened.backfill_managed_downloads().unwrap();
        let assets = reopened.list_assets().unwrap();
        assert_eq!(assets.len(), 2);
        let current_asset = assets.iter().find(|asset| asset.id == "asset-1").unwrap();
        let incoming_asset = assets.iter().find(|asset| asset.id == "asset-2").unwrap();
        assert_eq!(current_asset.path, current_media_path.to_string_lossy());
        assert_eq!(
            incoming_asset.path,
            restored_incoming_media.to_string_lossy()
        );
        assert!(Path::new(&current_asset.path).is_absolute());
        assert!(file_matches_sha256(
            Path::new(&current_asset.path),
            &current_hash
        ));
        assert!(file_matches_sha256(
            Path::new(&incoming_asset.path),
            &incoming_hash
        ));
        let track = reopened.get_track("youtube:abc123").unwrap().unwrap();
        assert_eq!(
            track.local_path.as_deref(),
            Some(current_asset.path.as_str())
        );
        assert_eq!(track.local_availability, "available");

        let repeated = reconstruct_staged(
            Some(&incoming.library_sqlite),
            None,
            None,
            None,
            stage.path(),
            &options,
        )
        .unwrap();
        assert!(repeated.already_applied);
        let reopened_again = Store::open(&target_database).unwrap();
        reopened_again.ensure_downloads_collection().unwrap();
        reopened_again.backfill_managed_downloads().unwrap();
        let assets_again = reopened_again.list_assets().unwrap();
        assert_eq!(assets_again.len(), 2);
        let current_asset_again = assets_again
            .iter()
            .find(|asset| asset.id == "asset-1")
            .unwrap();
        let incoming_asset_again = assets_again
            .iter()
            .find(|asset| asset.id == "asset-2")
            .unwrap();
        assert_eq!(
            current_asset_again.path,
            current_media_path.to_string_lossy()
        );
        assert_eq!(
            incoming_asset_again.path,
            restored_incoming_media.to_string_lossy()
        );
        assert!(file_matches_sha256(
            Path::new(&current_asset_again.path),
            &current_hash
        ));
        assert!(file_matches_sha256(
            Path::new(&incoming_asset_again.path),
            &incoming_hash
        ));
        let track_again = reopened_again.get_track("youtube:abc123").unwrap().unwrap();
        assert_eq!(
            track_again.local_path.as_deref(),
            Some(current_asset_again.path.as_str())
        );
        assert_eq!(track_again.local_availability, "available");
    }

    #[test]
    fn populated_merge_without_incoming_path_mapping_counts_retained_asset_as_available() {
        let profile = tempfile::tempdir().unwrap();
        insert_fixture(profile.path());
        let library = profile.path().join("burrata.sqlite");
        let history = profile.path().join("playback-history.sqlite");
        let scratch = tempfile::tempdir().unwrap();
        let incoming = prepare_from_snapshots_in(
            &library,
            &history,
            Uuid::new_v4(),
            BackupStateOptions::default(),
            scratch.path(),
        )
        .unwrap();
        let target = prepare_from_snapshots_in(
            &library,
            &history,
            Uuid::new_v4(),
            BackupStateOptions::default(),
            scratch.path(),
        )
        .unwrap();
        let stage = tempfile::tempdir().unwrap();
        std::fs::copy(&target.library_sqlite, stage.path().join("burrata.sqlite")).unwrap();
        let options = RestoreStateOptions {
            mode: RestoreMode::Merge,
            snapshot_id: incoming.snapshot_id.to_string(),
            current_asset_paths: current_asset_paths_from_snapshot(&library).unwrap(),
            current_music_root: current_music_root_from_snapshot(&library).unwrap(),
            include_library: true,
            ..RestoreStateOptions::default()
        };

        let summary = reconstruct_staged(
            Some(&incoming.library_sqlite),
            None,
            None,
            None,
            stage.path(),
            &options,
        )
        .unwrap();

        assert_eq!(summary.unavailable_assets, 0);
        let restored = Store::open(&stage.path().join("burrata.sqlite")).unwrap();
        let assets = restored.list_assets().unwrap();
        assert_eq!(assets.len(), 1);
        assert!(file_matches_sha256(
            Path::new(&assets[0].path),
            &assets[0].hash
        ));
    }

    #[test]
    fn backup_projection_keeps_full_native_checkpoint_and_scheduled_frontend_preferences() {
        let source = tempfile::tempdir().unwrap();
        insert_fixture(source.path());
        let library = source.path().join("burrata.sqlite");
        let connection = Connection::open(&library).unwrap();
        let tracks = (0..257)
            .map(|_| serde_json::json!({"id":"youtube:abc123","provider":"youtube","providerId":"abc123","title":"Song","artist":"Artist"}))
            .collect::<Vec<_>>();
        let entry_ids = (0..257)
            .map(|index| format!("native-entry-{index}"))
            .collect::<Vec<_>>();
        let queue = serde_json::json!({"tracks":tracks,"entryIds":entry_ids,"queueRevision":42});
        let session = serde_json::json!({
            "index":201,"position":38.5,"repeat":"all","shuffle":true,"volume":0.65,"queueRevision":42
        });
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key='nativePlaybackQueue.v1'",
                [queue.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key='nativePlaybackSession.v1'",
                [session.to_string()],
            )
            .unwrap();
        let internal_pending = crate::backup_frontend_preferences::PendingEnvelope::new(
            BTreeMap::from([("burrata.waveform-enabled.v1".into(), Value::Bool(false))]),
        );
        connection.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![crate::backup_frontend_preferences::PENDING_KEY, serde_json::to_string(&internal_pending).unwrap()],
        ).unwrap();
        drop(connection);

        let scheduled_preferences = serde_json::json!({
            "burrata.waveform-enabled.v1":true,
            "burrata.playlist.include-ytdlp-search.v1":true,
            "burrata.discovery.continue-search.v1":false
        });
        let prepared = prepare_backup(
            source.path(),
            Uuid::new_v4(),
            BackupStateOptions {
                frontend_preferences: Some(scheduled_preferences),
                ..BackupStateOptions::default()
            },
        )
        .unwrap();

        let playback: PlaybackArchive = read_json(&prepared.playback_json).unwrap();
        assert_eq!(playback.queue.len(), 257);
        assert_eq!(playback.entry_ids.len(), 257);
        assert_eq!(playback.current_index, 201);
        assert_eq!(playback.position_seconds, 38.5);
        assert_eq!(playback.repeat, "all");
        assert!(playback.shuffle);
        assert_eq!(playback.volume, 0.65);
        let settings: SettingsArchive = read_json(&prepared.settings_json).unwrap();
        assert_eq!(settings.frontend_preferences.len(), 3);
        assert_eq!(
            settings.frontend_preferences["burrata.waveform-enabled.v1"],
            true
        );
        assert_eq!(
            settings.frontend_preferences["burrata.playlist.include-ytdlp-search.v1"],
            true
        );
        assert_eq!(
            settings.frontend_preferences["burrata.discovery.continue-search.v1"],
            false
        );
        assert!(prepared.summary.omitted_private_settings >= 1);
        let sanitized_library = Connection::open(&prepared.library_sqlite).unwrap();
        let pending_is_private: bool = sanitized_library
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key=?1)",
                [crate::backup_frontend_preferences::PENDING_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!pending_is_private);
    }

    #[test]
    fn restore_strips_forged_loudness_process_seals_and_keeps_schema2_measurements() {
        let source = tempfile::tempdir().unwrap();
        let (_, _, hash, _) = insert_fixture(source.path());
        let resource = crate::loudness::LoudnessResourceIdentity::track(
            "asset-1",
            hash.clone(),
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        let record = crate::loudness::LoudnessRecord {
            schema_version: crate::loudness::LOUDNESS_SCHEMA_VERSION,
            resource,
            analyzer_version: "synthetic-loudness-v2".into(),
            provenance: crate::loudness::MeasurementProvenance::FfmpegAnalysis,
            integrated_lufs: Some(-18.25),
            true_peak_dbtp: Some(-1.25),
            sample_rate: Some(48_000),
            channels: 2,
            measured_at: "2026-10-27T00:00:00Z".into(),
            stereo_mix_true_peak_dbtp: Some(-1.5),
            stereo_mix_integrated_lufs: Some(-18.5),
            stereo_mix_matrix: Some("web-audio-stereo-identity-v1".into()),
            stereo_mix_layout: Some("stereo".into()),
            maximum_member_safe_true_peak_dbtp: Some(-1.5),
            maximum_member_native_safe_true_peak_dbtp: Some(-1.5),
            conservative_channel_sum_bound: false,
            local_file_seals: vec![crate::loudness::LocalFileSeal {
                asset_id: "asset-1".into(),
                file_hash: hash.clone(),
                size_bytes: 19,
                modified_unix_ns: Some(1_762_000_000_000_000_000),
                file_identity: Some("device:synthetic-inode".into()),
                verified_in_process: "source-process-uuid".into(),
            }],
            opus_decoder_proofs: vec![],
        };
        record.validate().unwrap();
        let source_library = source.path().join("burrata.sqlite");
        Connection::open(&source_library)
            .unwrap()
            .execute(
                "INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,?3,'completed',?4,?5)",
                params![
                    "asset-1",
                    hash,
                    crate::loudness_manager::ANALYSIS_CACHE_VERSION,
                    serde_json::to_string(&record).unwrap(),
                    "2026-10-27T00:00:00Z"
                ],
            )
            .unwrap();

        // Build the normal sanitized projection first, then reintroduce a
        // forged seal as an untrusted archive could. Reconstruction must apply
        // the privacy projection again without losing the durable measurement.
        let prepared =
            prepare_backup(source.path(), Uuid::new_v4(), BackupStateOptions::default()).unwrap();
        let exported_payload: String = Connection::open(&prepared.library_sqlite)
            .unwrap()
            .query_row(
                "SELECT payload FROM asset_analyses WHERE asset_id='asset-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let exported_value: Value = serde_json::from_str(&exported_payload).unwrap();
        assert!(exported_value.get("localFileSeals").is_none());
        assert_eq!(exported_value["provenance"], "ffmpegAnalysis");
        assert_eq!(exported_value["integratedLufs"], -18.25);
        let mut forged_payload = serde_json::to_value(&record).unwrap();
        forged_payload["localFileSeals"][0]["verifiedInProcess"] =
            Value::String("forged-current-process".into());
        forged_payload["localFileSeals"][0]["fileIdentity"] =
            Value::String("/machine/device/inode".into());
        Connection::open(&prepared.library_sqlite)
            .unwrap()
            .execute(
                "UPDATE asset_analyses SET payload=?1 WHERE asset_id='asset-1' AND version=?2",
                params![
                    serde_json::to_string(&forged_payload).unwrap(),
                    crate::loudness_manager::ANALYSIS_CACHE_VERSION
                ],
            )
            .unwrap();

        let target = tempfile::tempdir().unwrap();
        let summary = reconstruct_staged(
            Some(&prepared.library_sqlite),
            None,
            None,
            None,
            target.path(),
            &RestoreStateOptions {
                mode: RestoreMode::Replace,
                snapshot_id: prepared.snapshot_id.to_string(),
                include_library: true,
                ..RestoreStateOptions::default()
            },
        )
        .unwrap();
        assert_eq!(summary.imported_assets, 1);
        let restored_connection = Connection::open(target.path().join("burrata.sqlite")).unwrap();
        let (asset_id, file_hash, version, raw): (String, String, String, String) =
            restored_connection
                .query_row(
                    "SELECT asset_id,file_hash,version,payload FROM asset_analyses WHERE asset_id='asset-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
        let restored_value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(asset_id, "asset-1");
        assert_eq!(file_hash, hash);
        assert_eq!(version, crate::loudness_manager::ANALYSIS_CACHE_VERSION);
        assert!(restored_value.get("localFileSeals").is_none());
        assert_eq!(restored_value["integratedLufs"], -18.25);
        assert_eq!(restored_value["truePeakDbtp"], -1.25);
        assert_eq!(restored_value["stereoMixIntegratedLufs"], -18.5);
        assert_eq!(restored_value["stereoMixTruePeakDbtp"], -1.5);

        let restored: crate::loudness::LoudnessRecord =
            serde_json::from_value(restored_value).unwrap();
        restored.validate().unwrap();
        assert_eq!(
            restored.schema_version,
            crate::loudness::LOUDNESS_SCHEMA_VERSION
        );
        assert!(restored.local_file_seals.is_empty());
        // This checks inert cache structure only. It does not claim playback
        // reuse without fresh local-file verification in the current process.
    }

    #[tokio::test]
    async fn backup_restore_round_trip_projects_private_state_and_is_idempotent() {
        let source = tempfile::tempdir().unwrap();
        let (playlist_id, recording_id, hash, _source_media) = insert_fixture(source.path());
        let private_note = "Reference: https://example.invalid/score\nFile: /Music/DJ set/Track.flac\nKeep exactly.\0control";
        let mut source_library = Connection::open(source.path().join("burrata.sqlite")).unwrap();
        let hierarchy_raw: String = source_library
            .query_row(
                "SELECT value FROM settings WHERE key='playlistHierarchy.v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut hierarchy: Value = serde_json::from_str(&hierarchy_raw).unwrap();
        let long_unicode_name = format!("{}{}\n", "界".repeat(120), "x".repeat(90));
        let folder_name = format!("/House\n{long_unicode_name}");
        hierarchy["folders"] = serde_json::json!([
            {"id":"33333333-3333-4333-8333-333333333333","name":folder_name,"position":0}
        ]);
        hierarchy["playlists"][0]["folderId"] =
            Value::String("33333333-3333-4333-8333-333333333333".into());
        hierarchy["playlists"][0]["sections"] = serde_json::json!([
            {"id":"44444444-4444-4444-8444-444444444444","name":"/Set A","position":0}
        ]);
        hierarchy["playlists"][0]["occurrences"][0]["sectionId"] =
            Value::String("44444444-4444-4444-8444-444444444444".into());
        hierarchy["playlists"][0]["occurrences"][0]["note"] = Value::String(private_note.into());
        hierarchy["playlists"][0]["sections"][0]["name"] = Value::String(long_unicode_name.clone());
        hierarchy["playlists"][0]["occurrences"][0]["cuePoints"] = Value::Array(
            std::iter::once(Value::from(-0.25))
                .chain((0..127).map(|index| Value::from(index as f64 * 0.75)))
                .collect(),
        );
        hierarchy["playlists"][0]["occurrences"][0]["playbackRange"] =
            serde_json::json!({"startSeconds":86400.25,"endSeconds":86401.5});
        source_library
            .execute(
                "UPDATE settings SET value=?1 WHERE key='playlistHierarchy.v1'",
                [hierarchy.to_string()],
            )
            .unwrap();
        let manual_correction = serde_json::json!({
            "title":"/House/Corrected Song",
            "album":"https://example.invalid/manual-album",
            "apiToken":"synthetic-secret-must-be-dropped",
            "provenance":{
                "manualFields":{"title":true},
                "sourceUrl":"https://example.invalid/private-source?token=synthetic"
            }
        });
        source_library
            .execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
                params![manual_correction.to_string(), recording_id],
            )
            .unwrap();
        source_library
            .execute(
                "INSERT OR REPLACE INTO settings(key,value) VALUES('radioStationNames.v1',?1)",
                [serde_json::json!({"custom-one":"/Favourite house station"}).to_string()],
            )
            .unwrap();
        source_library
            .execute(
                "INSERT OR REPLACE INTO settings(key,value) VALUES('radioStations.v1',?1)",
                [serde_json::json!([{
                    "id":"custom-one",
                    "name":"/House Radio",
                    "streamUrl":"https://radio.example.invalid/listen?token=synthetic-radio-token",
                    "sourceUrl":"https://radio.example.invalid/source?auth=synthetic-radio-auth",
                    "isCustom":true
                }])
                .to_string()],
            )
            .unwrap();
        let mut sync_snapshot = crate::portable_playlist::PlaylistInput::from_tracks(
            "Snapshot playlist",
            &[crate::models::Track {
                id: "youtube:snapshot-track".into(),
                provider: "youtube".into(),
                provider_id: "snapshot-track".into(),
                artist: "Example artist".into(),
                title: "Snapshot track".into(),
                source_url: "https://youtube.com/watch?v=snapshot-track".into(),
                ..crate::models::Track::default()
            }],
        );
        sync_snapshot.entries[0].note = Some("Protected private note".into());
        sync_snapshot.entries[0].cue_points = vec![3.5, 18.25, 42.0];
        sync_snapshot.entries[0].playback_range = Some(crate::portable_playlist::PlaybackRange {
            start_seconds: 18.25,
            end_seconds: 42.0,
            ..Default::default()
        });
        let sync_snapshots = (0..501)
            .map(|index| {
                let mut versioned_snapshot = sync_snapshot.clone();
                versioned_snapshot.description = Some(format!("Revision {index}"));
                let sync_contents = crate::portable_playlist::serialize(
                    &versioned_snapshot,
                    &private_snapshot_export_options(),
                )
                .unwrap();
                serde_json::json!({
                    "revision": encode_hex(Sha256::digest(sync_contents.as_bytes())),
                    "capturedAt": format!("2026-10-25T00:{:02}:00Z", index % 60),
                    "contents": sync_contents
                })
            })
            .collect::<Vec<_>>();
        let latest_sync_hash = sync_snapshots.last().unwrap()["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        source_library
            .execute(
                "INSERT OR REPLACE INTO settings(key,value) VALUES('playlistSync.v1',?1)",
                [serde_json::json!({"sources":{
                    "/home/alice/Playlists/source.m3u8":{
                        "collectionId":playlist_id,
                        "lastSourceHash":latest_sync_hash,
                        "snapshots":sync_snapshots
                    }
                }})
                .to_string()],
            )
            .unwrap();
        drop(source_library);
        let snapshot_id = Uuid::new_v4();
        let lyric_request = crate::lyrics::LyricsRequest {
            track_name: "Synthetic Song".into(),
            artist_name: "Synthetic Artist".into(),
            album_name: Some("Synthetic Album".into()),
            duration_seconds: 180.0,
        };
        let lyric_record: crate::lyrics::ApiRecord = serde_json::from_value(
            lyric_choice_fixture("https://lrclib.net/api/get/990031")["record"].clone(),
        )
        .unwrap();
        let lyric_source_hash = crate::lyrics::classify_record(&lyric_request, lyric_record)
            .source_hash
            .unwrap();
        let lyric_timing_key = serde_json::to_string(&serde_json::json!([
            "youtube",
            "abc123",
            lyric_source_hash,
            0
        ]))
        .unwrap();
        let frontend_preferences = serde_json::json!({
            "burrata.discovery.search-history.v1":{"version":1,"entries":[{"query":"https://search.invalid/search?term=%2FHouse","kind":"tracks","mode":"online","sourceChoice":"all","customSourceIds":["youtube"],"savedAt":1792886400000_i64}]},
            "burrata.soulseek.wishlist.v1":{"version":1,"entries":[{"id":"wish-1","query":"/House/wishlist/Track.flac","enabled":true,"nextRun":1792886400000_i64,"failures":1,"lastChecked":null,"resultCount":null,"outcome":"unchecked"}],"nextSearch":0},
            "burrata.lyrics-timing.v1":{(lyric_timing_key.clone()):{"seconds":-1.7,"updated":1792886400000_i64}},
            "burrata.engine-switch-controls.v1":{"volume":1.0}
        });
        let prepared = prepare_backup(
            source.path(),
            snapshot_id,
            BackupStateOptions {
                frontend_preferences: Some(frontend_preferences),
                ..BackupStateOptions::default()
            },
        )
        .unwrap();
        assert_eq!(prepared.summary.recordings, 1);
        assert_eq!(prepared.summary.playlist_occurrences, 2);
        assert_eq!(prepared.summary.history_events, 1);
        assert_eq!(prepared.summary.omitted_jobs, 1);
        assert!(prepared.summary.omitted_cache_rows > 0);
        let prepared_playback: PlaybackArchive = read_json(&prepared.playback_json).unwrap();
        assert_eq!(prepared_playback.queue_loudness_contexts.len(), 1);
        assert!(prepared_playback
            .queue_loudness_contexts
            .contains_key("entry-1"));

        let safe = open_archive_db(&prepared.library_sqlite).unwrap();
        let source_url: String = safe
            .query_row(
                "SELECT source_url FROM catalog_listings WHERE id='youtube:abc123'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source_url, "https://youtube.com/watch");
        let asset_path: String = safe
            .query_row(
                "SELECT path FROM local_assets WHERE id='asset-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(asset_path, "@burrata-root/music/Artist/Album/song.flac");
        let archived_asset_payload: Value = safe
            .query_row(
                "SELECT payload FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| parse_bounded_json(&raw).unwrap())
            .unwrap();
        assert_eq!(
            archived_asset_payload["path"].as_str(),
            Some(asset_path.as_str())
        );
        let connection_pref: bool = safe
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key='connections')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!connection_pref);
        let cache_count: i64 = safe
            .query_row("SELECT COUNT(*) FROM provider_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cache_count, 0);

        let playback: PlaybackArchive = read_json(&prepared.playback_json).unwrap();
        assert!(playback.restore_paused);
        assert_eq!(playback.position_seconds, 12.5);
        assert_eq!(playback.queue.len(), 1);
        assert_eq!(playback.queue[0]["sourceUrl"], "");
        assert!(playback.queue[0]["provenance"]
            .as_object()
            .unwrap()
            .is_empty());
        let settings: SettingsArchive = read_json(&prepared.settings_json).unwrap();
        let settings_text = serde_json::to_string(&settings).unwrap();
        let lyric_key = format!("lyrics.choice.v1.{}", "b".repeat(64));
        assert_eq!(
            settings.preferences[&lyric_key]["candidate"]["sourceUrl"],
            "https://lrclib.net/api/get/990031"
        );
        assert_eq!(
            settings.preferences[&lyric_key]["record"]["syncedLyrics"],
            "[00:01.00]First synthetic line\n[00:04.00]Second synthetic line"
        );
        assert_eq!(
            settings.frontend_preferences["burrata.lyrics-timing.v1"][&lyric_timing_key]["seconds"],
            -1.7
        );
        assert_eq!(
            settings.preferences["playlistHierarchy.v1"]["playlists"][0]["occurrences"][0]["note"],
            private_note
        );
        assert!(!settings_text.contains("/secret"));
        assert!(!settings_text.contains("private-source"));
        assert!(!settings_text.contains("synthetic-secret-must-be-dropped"));
        assert!(!settings_text.contains("token=synthetic"));
        assert_eq!(settings.preferences["loudness.v1"]["mode"], "album");
        assert_eq!(
            settings.preferences["radioStationNames.v1"]["custom-one"],
            "/Favourite house station"
        );
        assert_eq!(
            settings.preferences["radioStations.v1"][0]["name"],
            "/House Radio"
        );
        assert_eq!(
            settings.preferences["radioStations.v1"][0]["streamUrl"],
            "https://radio.example.invalid/listen"
        );
        assert_eq!(
            settings.frontend_preferences["burrata.discovery.search-history.v1"]["entries"][0]
                ["query"],
            "https://search.invalid/search?term=%2FHouse"
        );
        assert_eq!(
            settings.frontend_preferences["burrata.soulseek.wishlist.v1"]["entries"][0]["enabled"],
            false
        );
        assert_eq!(
            settings.frontend_preferences["burrata.soulseek.wishlist.v1"]["entries"][0]["query"],
            "/House/wishlist/Track.flac"
        );
        assert!(!settings_text.contains("synthetic-radio-token"));
        assert!(!settings_text.contains("synthetic-radio-auth"));
        assert!(!settings
            .frontend_preferences
            .contains_key("burrata.engine-switch-controls.v1"));

        // Imported schema objects remain inert data and are never copied to the
        // trusted destination schema or executed during reconstruction.
        let archive_mutation = Connection::open(&prepared.library_sqlite).unwrap();
        archive_mutation.execute_batch("CREATE TABLE hostile_marker(value TEXT); CREATE TRIGGER hostile_trigger AFTER INSERT ON recordings BEGIN INSERT INTO hostile_marker VALUES('executed'); END;").unwrap();
        drop(archive_mutation);
        let restore_root = tempfile::tempdir().unwrap();
        let remapped_root = restore_root.path().join("new-music");
        std::fs::create_dir_all(remapped_root.join("Artist").join("Album")).unwrap();
        let remapped_root = std::fs::canonicalize(remapped_root).unwrap();
        std::fs::write(
            remapped_root.join("Artist").join("Album").join("song.flac"),
            b"fixture audio bytes",
        )
        .unwrap();
        let options = RestoreStateOptions {
            mode: RestoreMode::Replace,
            snapshot_id: snapshot_id.to_string(),
            path_mappings: vec![PathMapping {
                root_key: "music".into(),
                destination: remapped_root.clone(),
            }],
            current_asset_paths: Vec::new(),
            current_music_root: None,
            current_playback_snapshot: None,
            include_library: true,
            include_history: true,
            include_settings: true,
            include_playback: true,
        };
        let archive_path = restore_root
            .path()
            .join("consumer-roundtrip.burrata-backup");
        let inputs = vec![
            crate::backup_archive::ArchiveInput {
                id: "library".into(),
                schema_version: 11,
                source_path: prepared.library_sqlite.clone(),
            },
            crate::backup_archive::ArchiveInput {
                id: "history".into(),
                schema_version: 1,
                source_path: prepared.history_sqlite.clone(),
            },
            crate::backup_archive::ArchiveInput {
                id: "settings".into(),
                schema_version: 1,
                source_path: prepared.settings_json.clone(),
            },
            crate::backup_archive::ArchiveInput {
                id: "playback".into(),
                schema_version: 1,
                source_path: prepared.playback_json.clone(),
            },
        ];
        let archive_cancel = std::sync::atomic::AtomicBool::new(false);
        crate::backup_archive::create_archive(
            &inputs,
            &[crate::backup_archive::MediaInput {
                asset_id: "asset-1".into(),
                source_path: PathBuf::from(_source_media.as_str()),
                expected_sha256: Some(hash.clone()),
            }],
            crate::backup::BackupMediaSummary {
                included: true,
                assets: 1,
                missing: 0,
                changed: 0,
                omitted: 0,
            },
            &archive_path,
            "test",
            snapshot_id,
            &archive_cancel,
            None,
        )
        .unwrap();
        let validated_archive =
            crate::backup_archive::inspect_archive(&archive_path, &archive_cancel, None).unwrap();
        let archive_library = validated_archive.component_path("library").unwrap();
        let archive_history = validated_archive.component_path("history").unwrap();
        let archive_settings = validated_archive.component_path("settings").unwrap();
        let archive_playback = validated_archive.component_path("playback").unwrap();
        let result = reconstruct_staged(
            Some(archive_library),
            Some(archive_history),
            Some(archive_settings),
            Some(archive_playback),
            restore_root.path(),
            &options,
        )
        .unwrap();
        assert_eq!(result.imported_recordings, 1);
        assert_eq!(result.imported_collections, 1);
        assert_eq!(result.imported_playlist_occurrences, 2);
        assert_eq!(result.imported_history_events, 1);
        assert_eq!(result.unavailable_assets, 0);
        let library = Connection::open(restore_root.path().join("burrata.sqlite")).unwrap();
        let (favorite, manual): (i64, String) = library
            .query_row(
                "SELECT favorite,manual_metadata FROM recordings WHERE id=?1",
                [&recording_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(favorite, 1);
        let restored_manual = parse_bounded_json(&manual).unwrap();
        assert_eq!(restored_manual["title"], "/House/Corrected Song");
        assert_eq!(
            restored_manual["album"],
            "https://example.invalid/manual-album"
        );
        assert!(restored_manual.get("apiToken").is_none());
        assert!(restored_manual["provenance"].get("sourceUrl").is_none());
        let occurrences: i64 = library
            .query_row(
                "SELECT COUNT(*) FROM playlist_ordered_entries WHERE collection_id=?1",
                [&playlist_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(occurrences, 2);
        let stable_occurrences: Vec<(i64, String)> = library
            .prepare("SELECT position,occurrence_id FROM playlist_ordered_entries WHERE collection_id=?1 ORDER BY position")
            .unwrap()
            .query_map([&playlist_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            stable_occurrences,
            vec![
                (0, "11111111-1111-4111-8111-111111111111".into()),
                (1, "11111111-1111-4111-8111-111111111112".into()),
            ]
        );
        let restored_hierarchy: Value = library
            .query_row(
                "SELECT value FROM settings WHERE key='playlistHierarchy.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| parse_bounded_json(&raw).unwrap())
            .unwrap();
        let hierarchy_occurrences = restored_hierarchy["playlists"][0]["occurrences"]
            .as_array()
            .unwrap();
        assert_eq!(hierarchy_occurrences.len(), 2);
        assert_eq!(restored_hierarchy["folders"][0]["name"], folder_name);
        assert_eq!(
            restored_hierarchy["playlists"][0]["sections"][0]["name"],
            long_unicode_name
        );
        assert_eq!(hierarchy_occurrences[0]["id"], stable_occurrences[0].1);
        assert_eq!(hierarchy_occurrences[0]["note"], private_note);
        assert_eq!(
            hierarchy_occurrences[0]["cuePoints"]
                .as_array()
                .unwrap()
                .len(),
            128
        );
        assert_eq!(hierarchy_occurrences[0]["cuePoints"][0], -0.25);
        assert_eq!(hierarchy_occurrences[0]["cuePoints"][127], 94.5);
        assert_eq!(
            hierarchy_occurrences[0]["playbackRange"]["endSeconds"],
            86401.5
        );
        let restored_sync: Value = library
            .query_row(
                "SELECT value FROM settings WHERE key='playlistSync.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| parse_bounded_json(&raw).unwrap())
            .unwrap();
        let sync_history = restored_sync["backupProjection"]["sources"][0]["snapshots"]
            .as_array()
            .unwrap();
        assert_eq!(sync_history.len(), 501);
        let restored_snapshot =
            crate::portable_playlist::parse(sync_history[500]["contents"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            restored_snapshot.entries[0].note.as_deref(),
            Some("Protected private note")
        );
        assert_eq!(
            restored_snapshot.entries[0].cue_points,
            vec![3.5, 18.25, 42.0]
        );
        assert_eq!(
            restored_snapshot.entries[0]
                .playback_range
                .as_ref()
                .map(|range| (range.start_seconds, range.end_seconds)),
            Some((18.25, 42.0))
        );
        let sync_json = serde_json::to_string(&restored_sync).unwrap();
        assert!(!sync_json.contains("/home/alice/Playlists/source.m3u8"));
        assert_eq!(hierarchy_occurrences[1]["id"], stable_occurrences[1].1);
        assert_eq!(hierarchy_occurrences[1]["note"], "Second duplicate");
        let restored_path: String = library
            .query_row(
                "SELECT path FROM local_assets WHERE id='asset-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            restored_path,
            remapped_root
                .join("Artist")
                .join("Album")
                .join("song.flac")
                .to_string_lossy()
        );
        let restored_payload: Value = library
            .query_row(
                "SELECT payload FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| parse_bounded_json(&raw).unwrap())
            .unwrap();
        assert_eq!(restored_payload["path"], restored_path);
        assert_eq!(restored_payload["track"]["localPath"], restored_path);
        assert_eq!(restored_payload["track"]["assetId"], "asset-1");
        assert_eq!(restored_payload["track"]["localAvailability"], "available");
        assert_eq!(
            restored_payload["track"]["provenance"]["portablePlaybackSource"]["sourceId"],
            "abc123"
        );
        assert!(restored_payload["track"]
            .to_string()
            .find("signature")
            .is_none());
        let restored_metadata: Value = library
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id='youtube:abc123'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| parse_bounded_json(&raw).unwrap())
            .unwrap();
        let restored_binding = &restored_metadata["provenance"]["portablePlaybackSource"];
        assert_eq!(restored_binding["provider"], "youtube");
        assert_eq!(restored_binding["sourceId"], "abc123");
        assert_eq!(restored_binding["matchingLevel"], "exact");
        let expected_source_hash = "a".repeat(64);
        assert_eq!(
            restored_binding["sourceHash"].as_str(),
            Some(expected_source_hash.as_str())
        );
        assert_eq!(restored_binding.as_object().unwrap().len(), 4);
        let metadata_text = restored_metadata.to_string();
        assert!(!metadata_text.contains("sourceUrl"));
        assert!(!metadata_text.contains("signature"));
        assert!(!metadata_text.contains("candidate"));
        let restored_store = std::sync::Arc::new(
            crate::storage::Store::open(&restore_root.path().join("burrata.sqlite")).unwrap(),
        );
        let runtime_hierarchy = crate::playlist_hierarchy::load(&restored_store).unwrap();
        assert_eq!(runtime_hierarchy.folders[0].name, folder_name);
        assert_eq!(
            runtime_hierarchy.playlists[0].sections[0].name,
            long_unicode_name
        );
        assert_eq!(
            runtime_hierarchy.playlists[0].occurrences[0]
                .cue_points
                .len(),
            128
        );
        assert_eq!(
            runtime_hierarchy.playlists[0].occurrences[0].cue_points[0],
            -0.25
        );
        restored_store.ensure_downloads_collection().unwrap();
        restored_store.backfill_managed_downloads().unwrap();
        let restored_assets = restored_store.list_assets().unwrap();
        assert_eq!(restored_assets.len(), 1);
        assert_eq!(restored_assets[0].id, "asset-1");
        assert_eq!(restored_assets[0].path, restored_path);
        assert_eq!(
            restored_assets[0].track.local_path.as_deref(),
            Some(restored_path.as_str())
        );
        assert_eq!(
            restored_assets[0].track.asset_id.as_deref(),
            Some("asset-1")
        );
        // Historical archives used the same typed LocalAsset envelope but
        // omitted its top-level `path`; restore must recover it from the
        // separately checksummed/tokenized local_assets.path column.
        let legacy_dir = tempfile::tempdir().unwrap();
        let legacy_library = legacy_dir.path().join("legacy-library.sqlite");
        std::fs::copy(&prepared.library_sqlite, &legacy_library).unwrap();
        let legacy_conn = Connection::open(&legacy_library).unwrap();
        let legacy_payload: String = legacy_conn
            .query_row(
                "SELECT payload FROM local_assets WHERE id='asset-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut legacy_payload = parse_bounded_json(&legacy_payload).unwrap();
        legacy_payload.as_object_mut().unwrap().remove("path");
        legacy_conn
            .execute(
                "UPDATE local_assets SET payload=?1 WHERE id='asset-1'",
                [legacy_payload.to_string()],
            )
            .unwrap();
        drop(legacy_conn);
        let mut legacy_inputs = inputs.clone();
        legacy_inputs[0].source_path = legacy_library;
        let legacy_archive_path = legacy_dir.path().join("old-format.burrata-backup");
        crate::backup_archive::create_archive(
            &legacy_inputs,
            &[crate::backup_archive::MediaInput {
                asset_id: "asset-1".into(),
                source_path: PathBuf::from(_source_media.as_str()),
                expected_sha256: Some(hash.clone()),
            }],
            crate::backup::BackupMediaSummary {
                included: true,
                assets: 1,
                missing: 0,
                changed: 0,
                omitted: 0,
            },
            &legacy_archive_path,
            "legacy-test",
            snapshot_id,
            &archive_cancel,
            None,
        )
        .unwrap();
        let legacy_archive =
            crate::backup_archive::inspect_archive(&legacy_archive_path, &archive_cancel, None)
                .unwrap();
        let legacy_profile = tempfile::tempdir().unwrap();
        let legacy_music = legacy_profile.path().join("restored-music");
        std::fs::create_dir_all(legacy_music.join("Artist").join("Album")).unwrap();
        let legacy_music = std::fs::canonicalize(legacy_music).unwrap();
        std::fs::copy(
            &_source_media,
            legacy_music.join("Artist").join("Album").join("song.flac"),
        )
        .unwrap();
        let legacy_options = RestoreStateOptions {
            mode: RestoreMode::Replace,
            snapshot_id: snapshot_id.to_string(),
            path_mappings: vec![PathMapping {
                root_key: "music".into(),
                destination: legacy_music.clone(),
            }],
            current_asset_paths: Vec::new(),
            current_music_root: None,
            current_playback_snapshot: None,
            include_library: true,
            include_history: true,
            include_settings: true,
            include_playback: true,
        };
        reconstruct_staged(
            legacy_archive.component_path("library"),
            legacy_archive.component_path("history"),
            legacy_archive.component_path("settings"),
            legacy_archive.component_path("playback"),
            legacy_profile.path(),
            &legacy_options,
        )
        .unwrap();
        let legacy_store = Store::open(&legacy_profile.path().join("burrata.sqlite")).unwrap();
        legacy_store.ensure_downloads_collection().unwrap();
        legacy_store.backfill_managed_downloads().unwrap();
        let legacy_assets = legacy_store.list_assets().unwrap();
        assert_eq!(legacy_assets.len(), 1);
        assert_eq!(
            legacy_assets[0].path,
            legacy_music
                .join("Artist")
                .join("Album")
                .join("song.flac")
                .to_string_lossy()
        );
        let connections = std::sync::Arc::new(crate::connections::ConnectionManager::new(
            restored_store.clone(),
        ));
        let acquisition = crate::acquisition::AcquisitionManager::new_for_diagnostics(
            restored_store.clone(),
            connections,
        );
        let restored_track = crate::models::Track {
            id: "youtube:abc123".into(),
            provider: "youtube".into(),
            provider_id: "abc123".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            provenance: restored_metadata["provenance"].clone(),
            ..Default::default()
        };
        let unconfigured = Settings {
            preferred_streaming_source: "disabled".into(),
            streaming_fallback_enabled: false,
            ..Settings::default()
        };
        let (_cancel_sender, cancel) = tokio::sync::watch::channel(false);
        let binding_error = crate::playlist_sync::resolve_portable_playback_candidate(
            &restored_store,
            &acquisition,
            &restored_track,
            &unconfigured,
            cancel,
        )
        .await
        .unwrap_err();
        assert!(binding_error.contains("provider is no longer configured"));
        let hostile: bool=library.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('hostile_marker','hostile_trigger'))",[],|r|r.get(0)).unwrap();
        assert!(!hostile);
        let queue: Value = library
            .query_row(
                "SELECT value FROM settings WHERE key='nativePlaybackQueue.v1'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|s| serde_json::from_str(&s).unwrap())
            .unwrap();
        assert_eq!(queue["tracks"][0]["sourceUrl"], "");
        assert_eq!(queue["tracks"][0]["assetId"], "asset-1");
        assert_eq!(queue["tracks"][0]["localPath"], restored_path);
        assert_eq!(queue["tracks"][0]["localAvailability"], "available");
        assert_eq!(queue["queueLoudnessContexts"]["entry-1"]["scope"], "album");
        assert_eq!(
            queue["queueLoudnessContexts"]["entry-1"]["resourceKey"],
            prepared_playback.queue_loudness_contexts["entry-1"]["resourceKey"]
        );
        assert!(queue["queueLoudnessContexts"]
            .get("missing-entry")
            .is_none());
        let frontend: Value = library
            .query_row(
                "SELECT value FROM settings WHERE key='backupFrontendPreferences.v1'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|s| serde_json::from_str(&s).unwrap())
            .unwrap();
        assert_eq!(
            frontend["preferences"]["burrata.discovery.search-history.v1"]["entries"][0]["query"],
            "https://search.invalid/search?term=%2FHouse"
        );
        assert_eq!(
            frontend["preferences"]["burrata.soulseek.wishlist.v1"]["entries"][0]["enabled"],
            false
        );
        assert_eq!(
            frontend["preferences"]["burrata.soulseek.wishlist.v1"]["entries"][0]["query"],
            "/House/wishlist/Track.flac"
        );
        assert!(frontend.get("currentSessionId").is_none());
        let restored_station_names: Value = library
            .query_row(
                "SELECT value FROM settings WHERE key='radioStationNames.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| serde_json::from_str(&raw).unwrap())
            .unwrap();
        assert_eq!(
            restored_station_names["custom-one"],
            "/Favourite house station"
        );
        let restored_radio_stations: Value = library
            .query_row(
                "SELECT value FROM settings WHERE key='radioStations.v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| serde_json::from_str(&raw).unwrap())
            .unwrap();
        assert_eq!(restored_radio_stations[0]["name"], "/House Radio");
        assert_eq!(
            restored_radio_stations[0]["streamUrl"],
            "https://radio.example.invalid/listen"
        );

        let repeated = RestoreStateOptions {
            mode: RestoreMode::Merge,
            ..options
        };
        let second = reconstruct_staged(
            Some(archive_library),
            Some(archive_history),
            Some(archive_settings),
            Some(archive_playback),
            restore_root.path(),
            &repeated,
        )
        .unwrap();
        assert!(second.already_applied);
        let count: i64 = library
            .query_row(
                "SELECT COUNT(*) FROM playlist_ordered_entries WHERE collection_id=?1",
                [&playlist_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert!(hash.len() == 64);
    }

    #[test]
    fn durable_preference_projection_keeps_hierarchy_and_pauses_mirror_state() {
        let hierarchy = sanitize_playlist_hierarchy(serde_json::json!({
            "schemaVersion":1,
            "folders":[{"id":"folder-a","name":"Dance","position":0,"path":"/private"}],
            "playlists":[{"collectionId":"collection-a","folderId":"folder-a","position":0,
                "sections":[{"id":"section-a","name":"Side A","position":0}],
                "occurrences":[{"id":"occurrence-a","sectionId":"section-a","note":"Keep this","path":"/private"},
                    {"id":"occurrence-b"}]}]
        })).unwrap();
        assert_eq!(
            hierarchy["playlists"][0]["occurrences"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(!serde_json::to_string(&hierarchy)
            .unwrap()
            .contains("/private"));

        let original = "#EXTM3U\n#EXTINF:123,Artist - Song\n/home/alice/Music/song.flac\n";
        let original_hash = encode_hex(Sha256::digest(original.as_bytes()));
        let long_unbound_basename = format!("{}.m3u8", "N".repeat(201));
        let long_unbound_path = format!("/home/alice/Playlists/{long_unbound_basename}");
        let sync = sanitize_playlist_sync(serde_json::json!({
            "sources":{"/home/alice/Playlists/source.m3u8":{
                "collectionId":"collection-a","originalName":"/House.m3u8",
                "lastSourceHash":original_hash,
                "snapshots":[{"revision":original_hash,"capturedAt":"2026-10-25T00:00:00Z","contents":original}]
            },(long_unbound_path.clone()):{"originalName":long_unbound_basename}},
            "mirrorHashes":{
                "collection-a":encode_hex(Sha256::digest(b"mirror")),
                "section:collection-a:section-a":original_hash,
                "section:collection-a:section-bad/../x":"not-a-hash",
                "section:collection-a:section-a:another":original_hash
            },
            "mirrorPaths":{
                "collection-a":"/home/alice/Music/Playlists/source.m3u8",
                "section:collection-a:section-a":"/home/alice/Music/Playlists/Local/collection-a/Sections/section-a.m3u8"
            },
            "mirrorSnapshots":{"section__collection-a__section-a":[{
                "revision":original_hash,
                "capturedAt":"2026-10-25T00:00:00Z",
                "contents":original
            }]},
            "protectedCopies":{"/home/alice/Playlists/source.m3u8":"personal-a"}
        })).unwrap();
        let bound_source = |value: &Value| {
            value["backupProjection"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|source| source["collectionId"] == "collection-a")
                .unwrap()
                .clone()
        };
        let bound = bound_source(&sync);
        assert_eq!(sync["writesPaused"], true);
        let text = serde_json::to_string(&sync).unwrap();
        assert!(!text.contains("/home/alice"));
        assert!(!text.contains("song.flac"));
        assert_eq!(bound["relinkHint"]["basename"], "source.m3u8");
        assert_eq!(bound["originalName"], "/House.m3u8");
        assert_eq!(
            sync["backupProjection"]["protectedCopies"][0]["personalCollectionId"],
            "personal-a"
        );
        assert_eq!(
            sync["backupProjection"]["sectionMirrors"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let section = &sync["backupProjection"]["sectionMirrors"][0];
        assert_eq!(section["collectionId"], "collection-a");
        assert_eq!(section["sectionId"], "section-a");
        assert_eq!(section["hash"], original_hash);
        assert_eq!(section["relinkHint"]["basename"], "section-a.m3u8");
        assert_eq!(
            section["snapshots"][0]["sourceOriginalSha256"],
            original_hash
        );
        let snapshot = &bound["snapshots"][0];
        assert_eq!(snapshot["sourceOriginalSha256"], original_hash);
        assert_ne!(snapshot["contentsSha256"], snapshot["sourceOriginalSha256"]);
        let restored_projection = sanitize_playlist_sync(sync.clone()).unwrap();
        assert_eq!(restored_projection["writesPaused"], true);
        let restored_bound = bound_source(&restored_projection);
        assert_eq!(
            restored_bound["snapshots"][0]["sourceOriginalSha256"],
            original_hash
        );
        assert_eq!(restored_bound["originalName"], "/House.m3u8");
        let find_unbound = |value: &Value| {
            value["backupProjection"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|source| source["relinkHint"]["basename"] == long_unbound_basename)
                .unwrap()
                .clone()
        };
        let unbound = find_unbound(&sync);
        assert_eq!(unbound["originalName"].as_str().unwrap().len(), 206);
        assert_eq!(
            find_unbound(&restored_projection)["originalName"],
            unbound["originalName"]
        );
        assert_eq!(
            restored_bound["snapshots"][0]["contentsSha256"],
            snapshot["contentsSha256"]
        );
        let restored_section = &restored_projection["backupProjection"]["sectionMirrors"][0];
        assert_eq!(restored_section["collectionId"], "collection-a");
        assert_eq!(restored_section["sectionId"], "section-a");
        assert_eq!(restored_section["hash"], original_hash);
        assert_eq!(restored_section["relinkHint"]["basename"], "section-a.m3u8");
        assert_eq!(
            restored_section["snapshots"][0]["sourceOriginalSha256"],
            original_hash
        );
        assert!(!serde_json::to_string(&restored_section)
            .unwrap()
            .contains("/home/alice"));

        let legacy_hash = encode_hex(Sha256::digest(b"legacy section bytes"));
        let legacy_projection = sanitize_playlist_sync(serde_json::json!({
            "sources": {},
            "mirrorHashes": {},
            "mirrorPaths": {},
            "mirrorSnapshots": {},
            "protectedCopies": {},
            "writesPaused": true,
            "backupProjection": {
                "schemaVersion": 1,
                "writesPaused": true,
                "sources": [],
                "mirrors": [{
                    "collectionId": "section:collection-a:section-a",
                    "hash": legacy_hash,
                    "relinkHint": {"basename": "section-a.m3u8"},
                    "snapshots": []
                }],
                "protectedCopies": []
            }
        }))
        .unwrap();
        assert!(legacy_projection["backupProjection"]["mirrors"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            legacy_projection["backupProjection"]["sectionMirrors"][0]["collectionId"],
            "collection-a"
        );
        assert_eq!(
            legacy_projection["backupProjection"]["sectionMirrors"][0]["sectionId"],
            "section-a"
        );
        assert_eq!(
            legacy_projection["backupProjection"]["sectionMirrors"][0]["hash"],
            legacy_hash
        );
        assert_eq!(
            legacy_projection["backupProjection"]["sectionMirrors"][0]["relinkHint"]["basename"],
            "section-a.m3u8"
        );

        let conflicting_legacy_and_typed = sanitize_playlist_sync(serde_json::json!({
            "backupProjection": {
                "schemaVersion": 1,
                "writesPaused": true,
                "sources": [],
                "mirrors": [{
                    "collectionId": "section:collection-a:section-a",
                    "hash": legacy_hash,
                    "relinkHint": {"basename": "section-a.m3u8"}
                }],
                "sectionMirrors": [{
                    "collectionId": "collection-a",
                    "sectionId": "section-a",
                    "hash": encode_hex(Sha256::digest(b"conflicting section bytes")),
                    "relinkHint": {"basename": "section-a.m3u8"}
                }],
                "protectedCopies": []
            }
        }))
        .unwrap();
        assert!(
            conflicting_legacy_and_typed["backupProjection"]["sectionMirrors"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let too_large = format!("#EXTM3U\n{}", "x".repeat(2 * 1024 * 1024));
        let error = validate_playlist_sync_snapshot_limits(&serde_json::json!({
            "sources":{"/private/source.m3u8":{"snapshots":[{
                "contents":too_large
            }]}}
        }))
        .unwrap_err();
        assert!(error.contains("2 MiB format limit"));

        let radio = sanitize_radio_history(serde_json::json!({"stations":{"custom-1":[{
            "stationId":"custom-1","artist":"Artist","title":"Song","album":"Album","year":"2026",
            "sourceUrl":"http://192.168.1.10:8000/stream?token=secret","artworkUrl":"file:///private/art.jpg",
            "observedAt":"2026-10-25T00:00:00Z","metadataConfidence":"reported"
        }]}})).unwrap();
        let radio_text = serde_json::to_string(&radio).unwrap();
        assert!(!radio_text.contains("192.168.1.10"));
        assert!(!radio_text.contains("secret"));
        assert!(!radio_text.contains("/private"));
        assert_eq!(radio["stations"]["custom-1"][0]["title"], "Song");
    }

    #[test]
    fn portable_urls_reject_local_hosts_and_strip_credentials_and_queries() {
        assert_eq!(
            safe_public_url("https://user:pass@example.com/song?token=secret#fragment"),
            ""
        );
        assert_eq!(safe_public_url("http://127.0.0.1:8080/secret"), "");
        assert_eq!(
            safe_public_url("https://example.com/song?token=secret#fragment"),
            "https://example.com/song"
        );
    }
}
