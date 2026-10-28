//! Backup jobs, archive previews, opt-in scheduling, and cold-start restore.

use crate::{
    backup::BackupMediaSummary,
    backup_archive::{
        create_archive, create_archive_with_auxiliary_managed, inspect_archive_in,
        sqlite_online_snapshot, sync_directory, ArchiveInput, BackupInspection, BackupPhase,
        MediaInput, ValidatedArchive,
    },
    backup_staging::{BackupStaging, ProfileStage},
    backup_state::{
        self, BackupStateOptions, PathMapping, RestoreMode, RestoreStateOptions,
        RestoreStateSummary,
    },
    media,
    playback::{PlaybackBackupCheckpoint, PlaybackService},
    playback_history::PlaybackHistory,
    storage::{RestorePermit, Store, StoreRestoreFreeze},
};
use chrono::{DateTime, Datelike, Duration, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter};
use tempfile::NamedTempFile;
use uuid::Uuid;

const SCHEDULE_KEY: &str = "backupSchedule.v1";
const OWNED_ARCHIVES_KEY: &str = "backupOwnedArchives.v1";
const PENDING_RESTORE_FILE: &str = ".burrata-pending-restore.json";
const PROFILE_LOCK_FILE: &str = ".burrata-profile.lock";
const MAX_ARCHIVE_FILE_NAME: usize = 128;
const PROFILE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const FRONTEND_CHECKPOINT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_FRONTEND_CHECKPOINT_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRONTEND_CHECKPOINTS: usize = 4;

/// An exclusive advisory lock held for the lifetime of the application
/// process. Tauri restart starts the replacement process before the old
/// process exits, so cold restore waits on this lock before touching stores.
pub struct ProfileLock {
    _file: File,
}

pub fn acquire_profile_lock(data_dir: &Path) -> Result<ProfileLock, String> {
    acquire_profile_lock_with_timeout(data_dir, PROFILE_LOCK_WAIT)
}

fn acquire_profile_lock_with_timeout(
    data_dir: &Path,
    timeout: std::time::Duration,
) -> Result<ProfileLock, String> {
    let profile_dir = fs::canonicalize(data_dir)
        .map_err(|error| format!("Cannot resolve Burrata profile directory: {error}"))?;
    let path = profile_dir.join(PROFILE_LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("Cannot open Burrata profile lock: {error}"))?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ProfileLock { _file: file }),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("Cannot lock Burrata profile: {error}"));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if started.elapsed() >= timeout {
                    return Err(
                        "Another Burrata process still owns this profile; retry after it closes."
                            .into(),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSchedule {
    pub enabled: bool,
    pub destination: Option<PathBuf>,
    #[serde(default)]
    pub destination_marker: Option<String>,
    pub interval: BackupInterval,
    #[serde(default = "default_interval_days")]
    pub interval_days: u16,
    #[serde(default)]
    pub include_media: bool,
    pub retention_daily: usize,
    pub retention_weekly: usize,
    pub last_success_at: Option<String>,
    pub enabled_at: Option<String>,
    #[serde(default)]
    pub next_due_at: Option<String>,
    #[serde(default)]
    pub retry_after_at: Option<String>,
    #[serde(default)]
    pub retry_count: u8,
    #[serde(default)]
    pub last_archive_bytes: Option<u64>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub destination_available_bytes: Option<u64>,
    pub last_error: Option<String>,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            destination: None,
            destination_marker: None,
            interval: BackupInterval::Daily,
            interval_days: 1,
            include_media: false,
            retention_daily: 7,
            retention_weekly: 4,
            last_success_at: None,
            enabled_at: None,
            next_due_at: None,
            retry_after_at: None,
            retry_count: 0,
            last_archive_bytes: None,
            last_status: None,
            destination_available_bytes: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BackupInterval {
    Daily,
    Weekly,
    Custom,
}

fn default_interval_days() -> u16 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupJobStatus {
    pub job_id: String,
    #[serde(default)]
    pub automatic: bool,
    pub state: String,
    pub phase: Option<String>,
    pub completed: Option<u64>,
    pub total: Option<u64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEstimate {
    pub estimated_bytes: u64,
    pub registered_assets: usize,
    pub eligible_media_assets: usize,
    pub missing_assets: usize,
    pub changed_assets: usize,
    pub media_included: bool,
    pub eligible_sidecars: usize,
    pub missing_sidecars: usize,
    pub changed_sidecars: usize,
    pub sidecar_bytes: u64,
}

impl BackupJobStatus {
    fn new(job_id: String, automatic: bool) -> Self {
        Self {
            job_id,
            automatic,
            state: "queued".into(),
            phase: Some("Preparing backup".into()),
            completed: Some(0),
            total: Some(1),
            message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    pub preview_id: String,
    pub snapshot_id: String,
    pub schema_version: u32,
    pub created_at: String,
    pub components: Vec<crate::backup_archive::BackupComponentSummary>,
    pub media: BackupMediaSummary,
    pub archive_bytes: u64,
    pub estimated_bytes: u64,
    pub warnings: Vec<String>,
    pub summary: RestoreStateSummary,
    pub media_roots: Vec<BackupMediaRootPreview>,
    pub settings_projection: backup_state::SettingsProjectionSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMediaRootPreview {
    pub root_key: String,
    pub label: String,
    pub assets: usize,
    pub required_bytes: u64,
    pub destination_token: Option<String>,
    pub destination_label: Option<String>,
    pub reusable_same_hash: usize,
    pub copy_count: usize,
    pub conflicts: usize,
    pub missing_destination: bool,
    pub available_bytes: Option<u64>,
    pub lyrics_sidecars: usize,
    pub lyrics_copy_count: usize,
    pub lyrics_reusable_same_hash: usize,
    pub lyrics_conflicts: usize,
}

impl BackupMediaRootPreview {
    fn reset_destination_counts(&mut self) {
        self.reusable_same_hash = 0;
        self.copy_count = 0;
        self.conflicts = 0;
        self.lyrics_copy_count = 0;
        self.lyrics_reusable_same_hash = 0;
        self.lyrics_conflicts = 0;
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub mode: RestoreMode,
    pub component_ids: Vec<String>,
    #[serde(default)]
    pub path_mappings: Vec<RestorePathToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendCheckpointResponse {
    pub request_id: String,
    pub frontend_preferences: Value,
    #[serde(default)]
    pub webview_playback: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreviewOptions {
    pub mode: RestoreMode,
    pub component_ids: Vec<String>,
    #[serde(default)]
    pub path_mappings: Vec<RestorePathToken>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePathToken {
    pub root_key: String,
    pub destination_token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportResult {
    pub application_fields: usize,
    pub preferences: usize,
    pub frontend_preferences: HashMap<String, Value>,
}

#[derive(Clone)]
struct SettingsImportRecord {
    path: PathBuf,
    sha256: String,
    preview: backup_state::SettingsImportPreview,
    _scratch: Arc<ProfileStage>,
}

#[derive(Debug)]
struct JobRecord {
    cancel: Arc<AtomicBool>,
    status: Mutex<BackupJobStatus>,
}

struct PreviewRecord {
    archive: ValidatedArchive,
    _scratch: ProfileStage,
    preview: BackupPreview,
    destination_tokens: HashMap<String, RegisteredRestoreRoot>,
}

#[derive(Debug, Clone)]
struct RegisteredRestoreRoot {
    canonical_path: PathBuf,
    identity: RestoreRootIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RestoreRootIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume_serial: u32, file_index: u64 },
    #[cfg(not(any(unix, windows)))]
    Created(std::time::SystemTime),
}

/// The sanitized projection and its parent marker remain alive together so
/// the projection's nested TempDir is removed before its ownership guard.
struct ManagedPreparedBackup {
    prepared: backup_state::PreparedBackup,
    source_library_snapshot: PathBuf,
    library_change_count: u64,
    history_change_count: u64,
    _stage: ProfileStage,
}

struct RestoreFreeze {
    _permit: RestorePermit,
    _library: StoreRestoreFreeze,
    _history: crate::playback_history::HistoryRestoreFreeze,
}

impl std::ops::Deref for ManagedPreparedBackup {
    type Target = backup_state::PreparedBackup;
    fn deref(&self) -> &Self::Target {
        &self.prepared
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnedArchive {
    file_name: String,
    created_at: String,
    cadence: String,
    #[serde(default)]
    destination: PathBuf,
}

pub struct BackupManager {
    data_dir: PathBuf,
    store: Arc<Store>,
    history: Arc<PlaybackHistory>,
    playback: Option<Arc<PlaybackService>>,
    staging: BackupStaging,
    frontend_checkpoints: Mutex<HashMap<String, SyncSender<FrontendCheckpointResponse>>>,
    jobs: Mutex<HashMap<String, Arc<JobRecord>>>,
    previews: Mutex<HashMap<String, PreviewRecord>>,
    settings_previews: Mutex<HashMap<String, SettingsImportRecord>>,
    schedule_job_running: AtomicBool,
    active_job: Mutex<Option<String>>,
    restore_freeze: Mutex<Option<RestoreFreeze>>,
}

impl BackupManager {
    pub fn new(data_dir: PathBuf, store: Arc<Store>, history: Arc<PlaybackHistory>) -> Self {
        let staging =
            BackupStaging::open(&data_dir).expect("backup staging directory is available");
        Self::build(data_dir, store, history, None, staging)
    }

    pub fn with_playback(
        data_dir: PathBuf,
        store: Arc<Store>,
        history: Arc<PlaybackHistory>,
        playback: Arc<PlaybackService>,
    ) -> Self {
        let staging =
            BackupStaging::open(&data_dir).expect("backup staging directory is available");
        Self::build(data_dir, store, history, Some(playback), staging)
    }

    pub fn with_playback_and_staging(
        data_dir: PathBuf,
        store: Arc<Store>,
        history: Arc<PlaybackHistory>,
        playback: Arc<PlaybackService>,
        staging: BackupStaging,
    ) -> Self {
        Self::build(data_dir, store, history, Some(playback), staging)
    }

    fn build(
        data_dir: PathBuf,
        store: Arc<Store>,
        history: Arc<PlaybackHistory>,
        playback: Option<Arc<PlaybackService>>,
        staging: BackupStaging,
    ) -> Self {
        Self {
            data_dir,
            store,
            history,
            playback,
            staging,
            frontend_checkpoints: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            previews: Mutex::new(HashMap::new()),
            settings_previews: Mutex::new(HashMap::new()),
            schedule_job_running: AtomicBool::new(false),
            active_job: Mutex::new(None),
            restore_freeze: Mutex::new(None),
        }
    }

    /// Capture both live SQLite databases under a deterministic lock order,
    /// then project the private online snapshots into sanitized backup files.
    /// The Store mutex remains held until the History mutex has been acquired
    /// and both SQLite backup APIs have completed.
    fn prepare_coordinated_backup(
        &self,
        snapshot_id: Uuid,
        options: BackupStateOptions,
        expected_native_checkpoint: Option<&PlaybackBackupCheckpoint>,
    ) -> Result<ManagedPreparedBackup, String> {
        let stage = self
            .staging
            .create_profile_stage("sqlite-snapshot")
            .map_err(|e| e.to_string())?;
        let library = stage.path().join("library.sqlite");
        let history = stage.path().join("history.sqlite");
        let (library_change_count, history_change_count) =
            self.store.with_backup_connection(|library_connection| {
                let library_before = sqlite_total_changes(library_connection)?;
                self.history.with_backup_connection(|history_connection| {
                    let history_before = sqlite_total_changes(history_connection)?;
                    sqlite_online_snapshot(library_connection, &library)
                        .map_err(|error| error.to_string())?;
                    sqlite_online_snapshot(history_connection, &history)
                        .map_err(|error| error.to_string())?;
                    let library_after = sqlite_total_changes(library_connection)?;
                    let history_after = sqlite_total_changes(history_connection)?;
                    if library_before != library_after || history_before != history_after {
                        return Err("A database changed during coordinated backup snapshot".into());
                    }
                    Ok((library_after, history_after))
                })
            })?;
        if let Some(checkpoint) = expected_native_checkpoint {
            validate_native_checkpoint_snapshot(&library, checkpoint)?;
        }
        let prepared = backup_state::prepare_from_snapshots_in(
            &library,
            &history,
            snapshot_id,
            options,
            stage.path(),
        )
        .map_err(|e| e.to_string())?;
        Ok(ManagedPreparedBackup {
            prepared,
            source_library_snapshot: library,
            library_change_count,
            history_change_count,
            _stage: stage,
        })
    }

    fn request_frontend_checkpoint(
        &self,
        app: &AppHandle,
        include_webview_playback: bool,
        restore_purpose: bool,
        cancel: Option<&AtomicBool>,
    ) -> Result<FrontendCheckpointResponse, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let request_id = Uuid::new_v4().to_string();
        {
            let mut pending = self
                .frontend_checkpoints
                .lock()
                .map_err(|_| "Backup checkpoint map was poisoned")?;
            if pending.len() >= MAX_FRONTEND_CHECKPOINTS {
                return Err("Another frontend backup checkpoint is still pending".into());
            }
            pending.insert(request_id.clone(), sender);
        }
        if let Err(error) = app.emit(
            "backup-checkpoint-request",
            serde_json::json!({
                "requestId":request_id,
                "includeWebviewPlayback":include_webview_playback,
                "restorePurpose":restore_purpose,
                "deadlineMs":FRONTEND_CHECKPOINT_WAIT.as_millis() as u64
            }),
        ) {
            self.frontend_checkpoints
                .lock()
                .ok()
                .map(|mut pending| pending.remove(&request_id));
            return Err(format!(
                "Cannot request a fresh frontend checkpoint: {error}"
            ));
        }
        let started = std::time::Instant::now();
        let response = loop {
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                self.frontend_checkpoints
                    .lock()
                    .ok()
                    .map(|mut pending| pending.remove(&request_id));
                return Err("Backup checkpoint canceled".into());
            }
            let remaining = FRONTEND_CHECKPOINT_WAIT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                self.frontend_checkpoints
                    .lock()
                    .ok()
                    .map(|mut pending| pending.remove(&request_id));
                return Err("Frontend backup checkpoint timed out; no archive was created".into());
            }
            match receiver.recv_timeout(remaining.min(std::time::Duration::from_millis(100))) {
                Ok(response) => break response,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.frontend_checkpoints
                        .lock()
                        .ok()
                        .map(|mut pending| pending.remove(&request_id));
                    return Err("Frontend backup checkpoint was canceled".into());
                }
            }
        };
        if response.request_id != request_id {
            return Err("Frontend backup checkpoint ID did not match".into());
        }
        Ok(response)
    }

    pub fn submit_frontend_checkpoint(
        &self,
        response: FrontendCheckpointResponse,
    ) -> Result<(), String> {
        if response.request_id.len() > 128
            || response.request_id.is_empty()
            || response.request_id.chars().any(char::is_control)
        {
            return Err("Backup checkpoint request ID is invalid".into());
        }
        let payload_size = serde_json::to_vec(&response)
            .map_err(|error| error.to_string())?
            .len();
        if payload_size > MAX_FRONTEND_CHECKPOINT_BYTES {
            return Err("Frontend backup checkpoint exceeds the size limit".into());
        }
        let sender = self
            .frontend_checkpoints
            .lock()
            .map_err(|_| "Backup checkpoint map was poisoned")?
            .remove(&response.request_id)
            .ok_or("Backup checkpoint request is missing or expired")?;
        sender
            .try_send(response)
            .map_err(|_| "Backup checkpoint response was already submitted".to_owned())
    }

    fn capture_backup_checkpoint(
        &self,
        app: &AppHandle,
        cancel: Option<&AtomicBool>,
        include_playback: bool,
        restore_purpose: bool,
    ) -> Result<(BackupStateOptions, Option<PlaybackBackupCheckpoint>), String> {
        let playback = self
            .playback
            .as_ref()
            .ok_or("Playback checkpoint service is unavailable")?;
        let requested_webview = include_playback && !playback.native_enabled();
        let response =
            self.request_frontend_checkpoint(app, requested_webview, restore_purpose, cancel)?;
        if !response.frontend_preferences.is_object() {
            return Err("Frontend backup preferences checkpoint is missing".into());
        }
        let preferences =
            backup_state::sanitize_frontend_preferences(&response.frontend_preferences);
        if !include_playback {
            return Ok((
                BackupStateOptions {
                    webview_playback: None,
                    frontend_preferences: Some(
                        serde_json::to_value(preferences).map_err(|error| error.to_string())?,
                    ),
                },
                None,
            ));
        }
        let checkpoint = playback.checkpoint_for_backup()?;
        let webview_playback = if checkpoint.engine == "webview" {
            let payload = response
                .webview_playback
                .ok_or("Fresh WebView playback state is unavailable; no archive was created")?;
            validate_webview_playback_checkpoint(&payload)?;
            Some(payload)
        } else if checkpoint.engine == "native" {
            None
        } else {
            return Err("Active playback engine is unsupported for backup".into());
        };
        Ok((
            BackupStateOptions {
                webview_playback,
                frontend_preferences: Some(
                    serde_json::to_value(preferences).map_err(|e| e.to_string())?,
                ),
            },
            (checkpoint.engine == "native").then_some(checkpoint),
        ))
    }

    pub fn inspect_path(&self, path: &Path) -> Result<BackupPreview, String> {
        let scratch = self
            .staging
            .create_profile_stage("archive-inspect")
            .map_err(|e| e.to_string())?;
        let cancel = AtomicBool::new(false);
        let archive = inspect_archive_in(path, &cancel, None, Some(scratch.path()))
            .map_err(|e| e.to_string())?;
        let mut options = restore_options(
            &archive.inspection.snapshot_id,
            RestoreMode::Merge,
            &archive
                .inspection
                .components
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>(),
            Vec::new(),
        );
        let summary = backup_state::preview_restore_in(
            archive.component_path("library"),
            archive.component_path("history"),
            archive.component_path("settings"),
            archive.component_path("playback"),
            Some(&self.data_dir),
            &options,
            scratch.path(),
        )
        .map_err(|e| format!("Backup data validation failed: {e}"))?;
        options.path_mappings.clear();
        let settings_archive: backup_state::SettingsArchive = serde_json::from_reader(
            File::open(
                archive
                    .component_path("settings")
                    .ok_or("Backup has no settings component")?,
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let settings_projection = settings_archive.projection_summary;
        let media_roots = media_root_previews(&archive)?;
        let preview_id = Uuid::new_v4().to_string();
        let preview = BackupPreview {
            preview_id: preview_id.clone(),
            snapshot_id: archive.inspection.snapshot_id.clone(),
            schema_version: archive.inspection.schema_version,
            created_at: archive.inspection.created_at.clone(),
            components: archive.inspection.components.clone(),
            media: archive.inspection.media.clone(),
            archive_bytes: archive.inspection.archive_bytes,
            estimated_bytes: archive.inspection.components.iter().map(|c| c.bytes).sum(),
            warnings: {
                let mut warnings = archive.inspection.warnings.clone();
                let skipped = settings_projection.omitted_cache_rows
                    + settings_projection.omitted_jobs
                    + settings_projection.omitted_private_settings
                    + settings_projection.excluded_application_fields;
                if skipped > 0 || settings_projection.asset_path_hints > 0 {
                    warnings.push(format!(
                        "Backup projection omitted {} transient cache rows, {} jobs, {} private settings, {} device-specific settings, and retained {} media path hints for verified audio restore.",
                        settings_projection.omitted_cache_rows,
                        settings_projection.omitted_jobs,
                        settings_projection.omitted_private_settings,
                        settings_projection.excluded_application_fields,
                        settings_projection.asset_path_hints,
                    ));
                }
                warnings
            },
            summary,
            media_roots,
            settings_projection,
        };
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| "Backup preview lock was poisoned")?;
        if previews.len() >= 3 {
            if let Some(expired) = previews.keys().next().cloned() {
                previews.remove(&expired);
            }
        }
        previews.insert(
            preview_id,
            PreviewRecord {
                archive,
                _scratch: scratch,
                preview: preview.clone(),
                destination_tokens: HashMap::new(),
            },
        );
        Ok(preview)
    }

    pub fn register_restore_root(
        &self,
        preview_id: &str,
        root_key: &str,
        destination: &Path,
    ) -> Result<BackupMediaRootPreview, String> {
        fs::create_dir_all(destination)
            .map_err(|e| format!("Cannot use destination folder: {e}"))?;
        let canonical = fs::canonicalize(destination).map_err(|e| e.to_string())?;
        let registered_root = seal_restore_root(&canonical)?;
        validate_restore_root(&registered_root)?;
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| "Backup previews lock was poisoned")?;
        let record = previews
            .get_mut(preview_id)
            .ok_or("Backup preview expired; inspect again")?;
        let mut root = record
            .preview
            .media_roots
            .iter()
            .find(|row| row.root_key == root_key)
            .cloned()
            .ok_or("Backup does not contain this media root")?;
        let token = Uuid::new_v4().to_string();
        record
            .destination_tokens
            .insert(token.clone(), registered_root.clone());
        root.destination_token = Some(token.clone());
        root.destination_label = Some(
            canonical
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| root.label.clone()),
        );
        root.missing_destination = false;
        root.available_bytes = free_bytes_available(&canonical);
        root.reset_destination_counts();
        validate_restore_root(&registered_root)?;
        analyze_restore_root(record, &mut root, &canonical)?;
        if let Some(existing) = record
            .preview
            .media_roots
            .iter_mut()
            .find(|row| row.root_key == root_key)
        {
            *existing = root.clone();
        }
        Ok(root)
    }

    pub fn preview_restore_selection(
        &self,
        preview_id: &str,
        options: RestorePreviewOptions,
    ) -> Result<RestoreStateSummary, String> {
        let previews = self
            .previews
            .lock()
            .map_err(|_| "Backup previews lock was poisoned")?;
        let record = previews
            .get(preview_id)
            .ok_or("Backup preview expired; inspect the archive again")?;
        let selected = options
            .component_ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        if selected.len() != options.component_ids.len()
            || selected.is_empty()
            || selected.iter().any(|id| {
                !record
                    .archive
                    .manifest
                    .components
                    .iter()
                    .any(|component| &component.id == id)
            })
        {
            return Err("Restore component selection is invalid".into());
        }
        if selected.iter().any(|id| id.starts_with("media:")) && !selected.contains("library") {
            return Err("Select the library component to restore audio assets".into());
        }
        for attachment in &record.archive.manifest.auxiliary_assets {
            if selected.contains(&attachment.component_id)
                && (!selected.contains("library")
                    || !selected.contains(&format!("media:{}", attachment.asset_id)))
            {
                return Err("Select the companion audio file to restore its lyrics sidecar".into());
            }
        }
        if options.mode == RestoreMode::Replace
            && ["library", "history", "settings", "playback"]
                .iter()
                .any(|required| !selected.contains(*required))
        {
            return Err("Replace restore requires selecting all four data components".into());
        }
        let mut roots = std::collections::HashSet::new();
        let mut mappings = Vec::new();
        for mapping in &options.path_mappings {
            if !roots.insert(mapping.root_key.as_str()) {
                return Err("Restore destination token is invalid".into());
            }
            let destination = record
                .destination_tokens
                .get(&mapping.destination_token)
                .ok_or("Restore destination token is invalid")?;
            if !record.preview.media_roots.iter().any(|row| {
                row.root_key == mapping.root_key
                    && row.destination_token.as_deref() == Some(mapping.destination_token.as_str())
            }) {
                return Err("Restore destination token is invalid".into());
            }
            validate_restore_root(destination)?;
            mappings.push(PathMapping {
                root_key: mapping.root_key.clone(),
                destination: destination.canonical_path.clone(),
            });
        }
        backup_state::preview_restore_selection_in(
            record.archive.component_path("library"),
            record.archive.component_path("history"),
            record.archive.component_path("settings"),
            record.archive.component_path("playback"),
            Some(&self.data_dir),
            &record.preview.snapshot_id,
            options.mode,
            &options.component_ids,
            mappings,
            record._scratch.path(),
        )
        .map_err(|error| format!("Cannot preview restore: {error}"))
    }

    pub fn status(&self, job_id: &str) -> Result<BackupJobStatus, String> {
        self.jobs
            .lock()
            .map_err(|_| "Backup jobs lock was poisoned")?
            .get(job_id)
            .ok_or("Backup job was not found")?
            .status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| "Backup job status lock was poisoned".into())
    }

    pub fn active_job_status(&self) -> Result<Option<BackupJobStatus>, String> {
        let active_job = self
            .active_job
            .lock()
            .map_err(|_| "Backup active-job lock was poisoned")?
            .clone();
        let Some(job_id) = active_job else {
            return Ok(None);
        };
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "Backup jobs lock was poisoned")?;
        let Some(record) = jobs.get(&job_id) else {
            // A job can finish and release its active slot between the two
            // lookups. In that case there is no longer an active job to report.
            return Ok(None);
        };
        let status = record
            .status
            .lock()
            .map_err(|_| "Backup job status lock was poisoned")?
            .clone();
        Ok(Some(status))
    }

    pub fn estimate(&self, include_media: bool) -> Result<BackupEstimate, String> {
        let mut bytes = 0u64;
        for name in ["burrata.sqlite", "playback-history.sqlite"] {
            bytes = bytes.saturating_add(
                fs::metadata(self.data_dir.join(name))
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
        }
        let assets = self.store.list_assets()?;
        let mut eligible = 0usize;
        let mut missing = 0usize;
        let mut changed = 0usize;
        let mut eligible_sidecars = 0usize;
        let mut missing_sidecars = 0usize;
        let mut changed_sidecars = 0usize;
        let mut sidecar_bytes = 0u64;
        if include_media {
            for asset in &assets {
                let path = PathBuf::from(&asset.path);
                if !path.is_file() {
                    missing += 1;
                    continue;
                }
                let actual = media::hash_file(&path).map_err(|e| e.to_string())?;
                if actual.eq_ignore_ascii_case(&asset.hash) {
                    eligible += 1;
                    bytes = bytes.saturating_add(
                        fs::metadata(path)
                            .map(|metadata| metadata.len())
                            .unwrap_or(0),
                    );
                    match crate::backup::snapshot_lyrics_sidecar(asset, &actual) {
                        crate::backup::AuxiliaryDiscovery::Included(sidecar) => {
                            eligible_sidecars += 1;
                            let size = fs::metadata(sidecar.source_path)
                                .map(|metadata| metadata.len())
                                .unwrap_or(0);
                            sidecar_bytes = sidecar_bytes.saturating_add(size);
                            bytes = bytes.saturating_add(size);
                        }
                        crate::backup::AuxiliaryDiscovery::Missing => missing_sidecars += 1,
                        crate::backup::AuxiliaryDiscovery::Invalid => changed_sidecars += 1,
                    }
                } else {
                    changed += 1;
                }
            }
        }
        Ok(BackupEstimate {
            estimated_bytes: bytes,
            registered_assets: assets.len(),
            eligible_media_assets: eligible,
            missing_assets: missing,
            changed_assets: changed,
            media_included: include_media,
            eligible_sidecars,
            missing_sidecars,
            changed_sidecars,
            sidecar_bytes,
        })
    }

    /// Write the same versioned allowlisted settings projection used by full
    /// backups, omitting all asset path hints for portable settings exports.
    pub fn export_settings(&self, app: &AppHandle, destination: &Path) -> Result<(), String> {
        let (options, native_checkpoint) =
            self.capture_backup_checkpoint(app, None, false, false)?;
        let prepared =
            self.prepare_coordinated_backup(Uuid::new_v4(), options, native_checkpoint.as_ref())?;
        self.write_settings_export(destination, prepared)
    }

    fn write_settings_export(
        &self,
        destination: &Path,
        prepared: ManagedPreparedBackup,
    ) -> Result<(), String> {
        let mut archive: backup_state::SettingsArchive = serde_json::from_reader(
            File::open(&prepared.settings_json).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let remembered_lyrics = archive
            .preferences
            .keys()
            .filter(|key| key.starts_with("lyrics.choice.v1."))
            .cloned()
            .collect::<Vec<_>>();
        for key in &remembered_lyrics {
            archive.preferences.remove(key);
        }
        archive.asset_path_hints.clear();
        archive.projection_summary = backup_state::SettingsProjectionSummary::default();
        archive.projection_summary.omitted_private_settings = remembered_lyrics.len();
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut staged = self
            .staging
            .create_external_file(parent)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer(
            staged.file_mut().map_err(|error| error.to_string())?,
            &archive,
        )
        .map_err(|error| error.to_string())?;
        staged
            .publish(destination)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn inspect_settings_path(
        &self,
        path: &Path,
    ) -> Result<backup_state::SettingsImportPreview, String> {
        let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
        let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 16 * 1024 * 1024 {
            return Err("Settings file is empty or exceeds the import size limit".into());
        }
        let scratch = Arc::new(
            self.staging
                .create_profile_stage("settings-inspect")
                .map_err(|e| e.to_string())?,
        );
        let staged_path = scratch.path().join("settings.json");
        let source = File::open(&canonical).map_err(|error| error.to_string())?;
        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_path)
            .map_err(|error| error.to_string())?;
        let copied = std::io::copy(&mut source.take(16 * 1024 * 1024 + 1), &mut staged)
            .map_err(|error| error.to_string())?;
        if copied > 16 * 1024 * 1024 {
            return Err("Settings file exceeds the import size limit".into());
        }
        staged.sync_all().map_err(|error| error.to_string())?;
        drop(staged);
        let preview = backup_state::inspect_settings_archive(&staged_path)?;
        let cancel = AtomicBool::new(false);
        let sha256 = hash_file_cancellable(&staged_path, &cancel)?;
        let record = SettingsImportRecord {
            path: staged_path,
            sha256,
            preview: preview.clone(),
            _scratch: scratch,
        };
        let mut previews = self
            .settings_previews
            .lock()
            .map_err(|_| "Settings preview lock was poisoned")?;
        if previews.len() >= 8 {
            if let Some(expired) = previews.keys().next().cloned() {
                previews.remove(&expired);
            }
        }
        previews.insert(preview.preview_id.clone(), record);
        Ok(preview)
    }

    pub fn apply_settings_import(
        &self,
        preview_id: &str,
        selection: backup_state::SettingsImportSelection,
    ) -> Result<SettingsImportResult, String> {
        let record = self
            .settings_previews
            .lock()
            .map_err(|_| "Settings preview lock was poisoned")?
            .get(preview_id)
            .cloned()
            .ok_or("Settings preview expired; inspect the file again")?;
        if selection
            .preference_keys
            .iter()
            .any(|key| !record.preview.preference_keys.contains(key))
            || selection
                .frontend_preference_keys
                .iter()
                .any(|key| !record.preview.frontend_preference_keys.contains(key))
            || (selection.application && record.preview.application_fields.is_empty())
        {
            return Err("Settings selection is invalid".into());
        }
        // Settings import updates the same durable library state as restore;
        // block it before hashing/projecting the reviewed input so a restore
        // cannot publish over an in-flight import.
        let _write_permit = self.store.try_library_write()?;
        let cancel = AtomicBool::new(false);
        let current_hash = hash_file_cancellable(&record.path, &cancel)?;
        if !current_hash.eq_ignore_ascii_case(&record.sha256) {
            return Err("Settings file changed after preview; inspect it again".into());
        }
        let projection = backup_state::project_settings_archive(&record.path, &selection)?;
        if fs::canonicalize(&record.path).ok().as_deref() != Some(record.path.as_path())
            || !hash_file_cancellable(&record.path, &cancel)?.eq_ignore_ascii_case(&record.sha256)
        {
            return Err("Settings file changed during import; inspect it again".into());
        }
        let application_fields = projection.application_fields;
        let preferences = projection.preferences;
        let frontend_preferences = projection.frontend_preferences;
        let application_field_count = projection.summary.application_fields;
        let preference_count = projection.summary.preferences;
        let mut pause_categories = Vec::new();
        if application_fields
            .get("backgroundSourcePreparation")
            .and_then(Value::as_bool)
            == Some(true)
        {
            pause_categories
                .push(crate::restore_work_policy::AutomaticWorkKind::BackgroundSourcePreparation);
        }
        if preferences
            .get("autotag.settings")
            .and_then(|value| value.get("enabled"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            pause_categories.push(crate::restore_work_policy::AutomaticWorkKind::AssetAutotag);
        }
        if frontend_preferences
            .get("burrata.stream-media-analysis-enabled.v1")
            .and_then(Value::as_bool)
            == Some(true)
        {
            pause_categories
                .push(crate::restore_work_policy::AutomaticWorkKind::BackgroundStreamMediaAnalysis);
        }
        self.store.with_backup_connection(|connection| {
            connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| error.to_string())?;
            let write_result = (|| {
                if !application_fields.is_empty() {
                    let raw: Option<String> = connection
                        .query_row(
                            "SELECT value FROM settings WHERE key='application'",
                            [],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|error| error.to_string())?;
                    let current = raw
                        .map(|raw| serde_json::from_str::<crate::models::Settings>(&raw))
                        .transpose()
                        .map_err(|error| error.to_string())?
                        .unwrap_or_default();
                    let mut merged = serde_json::to_value(current)
                        .map_err(|error| error.to_string())?
                        .as_object()
                        .cloned()
                        .ok_or("Current settings are invalid")?;
                    for (key, value) in &application_fields {
                        merged.insert(key.clone(), value.clone());
                    }
                    let settings: crate::models::Settings =
                        serde_json::from_value(Value::Object(merged))
                            .map_err(|error| error.to_string())?;
                    settings
                        .validate_playback_startup_timeout()
                        .map_err(|error| error.to_string())?;
                    let serialized = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
                    connection
                        .execute(
                            "INSERT INTO settings(key,value) VALUES('application',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                            [serialized],
                        )
                        .map_err(|error| error.to_string())?;
                }
                for (key, value) in &preferences {
                    let serialized = serde_json::to_string(value).map_err(|e| e.to_string())?;
                    connection
                        .execute(
                            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                            rusqlite::params![key, serialized],
                        )
                        .map_err(|error| error.to_string())?;
                }
                if !frontend_preferences.is_empty() {
                    let envelope =
                        crate::backup_frontend_preferences::PendingEnvelope::new(frontend_preferences.clone());
                    let serialized = serde_json::to_string(&envelope)
                        .map_err(|e| e.to_string())?;
                    connection
                        .execute(
                            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                            rusqlite::params![crate::backup_frontend_preferences::PENDING_KEY, serialized],
                        )
                        .map_err(|error| error.to_string())?;
                }
                if !pause_categories.is_empty() {
                    let current_raw: Option<String> = connection
                        .query_row(
                            "SELECT value FROM settings WHERE key=?1",
                            [crate::restore_work_policy::PREFERENCE_KEY],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|error| error.to_string())?;
                    let current_value = current_raw
                        .map(|raw| {
                            if raw.len() > 64 * 1024 {
                                return Err("Current automatic-work policy exceeds its size limit".to_owned());
                            }
                            serde_json::from_str::<Value>(&raw)
                                .map_err(|error| error.to_string())
                        })
                        .transpose()?;
                    let policy = crate::restore_work_policy::pause_selected_for_settings_import(
                        current_value.as_ref(),
                        &pause_categories,
                    )?
                    .ok_or("Automatic-work pause selection disappeared")?;
                    connection
                        .execute(
                            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                            rusqlite::params![
                                crate::restore_work_policy::PREFERENCE_KEY,
                                serde_json::to_string(&policy).map_err(|error| error.to_string())?
                            ],
                        )
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            })();
            match write_result {
                Ok(()) => connection
                    .execute_batch("COMMIT")
                    .map_err(|error| error.to_string()),
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        Ok(SettingsImportResult {
            application_fields: application_field_count,
            preferences: preference_count,
            frontend_preferences: frontend_preferences.into_iter().collect(),
        })
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "Backup jobs lock was poisoned")?;
        let job = jobs.get(job_id).ok_or("Backup job was not found")?;
        let status = job
            .status
            .lock()
            .map_err(|_| "Backup job status lock was poisoned")?;
        if status.phase.as_deref() == Some("Restore staged; restarting Burrata") {
            return Err("Restore is staged for restart and can no longer be canceled".into());
        }
        drop(status);
        job.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn start_create(
        self: &Arc<Self>,
        app: AppHandle,
        destination: PathBuf,
        include_media: bool,
        automatic: bool,
    ) -> Result<String, String> {
        self.start_create_inner(app, destination, include_media, automatic)
    }

    fn start_create_inner(
        self: &Arc<Self>,
        app: AppHandle,
        destination: PathBuf,
        include_media: bool,
        automatic: bool,
    ) -> Result<String, String> {
        let job_id = Uuid::new_v4().to_string();
        self.reserve_job(&job_id)?;
        let record = Arc::new(JobRecord {
            cancel: Arc::new(AtomicBool::new(false)),
            status: Mutex::new(BackupJobStatus::new(job_id.clone(), automatic)),
        });
        if self
            .jobs
            .lock()
            .map(|mut jobs| jobs.insert(job_id.clone(), record.clone()))
            .is_err()
        {
            self.release_job(&job_id);
            return Err("Backup jobs lock was poisoned".into());
        }
        self.trim_finished_jobs();
        let manager = self.clone();
        let task_id = job_id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            manager.run_create(
                &app,
                &task_id,
                record,
                &destination,
                include_media,
                automatic,
            );
        });
        Ok(job_id)
    }

    fn run_create(
        &self,
        app: &AppHandle,
        job_id: &str,
        job: Arc<JobRecord>,
        destination: &Path,
        include_media: bool,
        automatic: bool,
    ) {
        self.update(
            app,
            &job,
            "running",
            "Preparing a consistent snapshot",
            None,
            None,
            None,
        );
        let result = (|| -> Result<BackupInspection, String> {
            let snapshot_id = Uuid::new_v4();
            let (state_options, native_checkpoint) =
                self.capture_backup_checkpoint(app, Some(&job.cancel), true, false)?;
            let prepared = self.prepare_coordinated_backup(
                snapshot_id,
                state_options,
                native_checkpoint.as_ref(),
            )?;
            let snapshot_id = prepared.snapshot_id;
            if job.cancel.load(Ordering::Relaxed) {
                return Err("Backup canceled".into());
            }
            let inputs = vec![
                ArchiveInput {
                    id: "library".into(),
                    schema_version: sqlite_user_version(&prepared.library_sqlite)?,
                    source_path: prepared.library_sqlite.clone(),
                },
                ArchiveInput {
                    id: "history".into(),
                    schema_version: 1,
                    source_path: prepared.history_sqlite.clone(),
                },
                ArchiveInput {
                    id: "settings".into(),
                    schema_version: 1,
                    source_path: prepared.settings_json.clone(),
                },
                ArchiveInput {
                    id: "playback".into(),
                    schema_version: 1,
                    source_path: prepared.playback_json.clone(),
                },
            ];
            let (media_inputs, auxiliary_inputs, media_summary, auxiliary_counts) =
                self.media_inputs(include_media, &job.cancel)?;
            let mut progress = |phase: BackupPhase, completed: u64, total: u64| {
                let phase = match phase {
                    BackupPhase::Scanning => "Checking staged data",
                    BackupPhase::Writing => "Writing backup archive",
                    BackupPhase::Verifying => "Verifying backup archive",
                };
                self.update(
                    app,
                    &job,
                    "running",
                    phase,
                    Some(completed),
                    Some(total),
                    None,
                );
            };
            let mut inspection = create_archive_with_auxiliary_managed(
                &inputs,
                &media_inputs,
                &auxiliary_inputs,
                media_summary,
                destination,
                env!("CARGO_PKG_VERSION"),
                snapshot_id,
                &job.cancel,
                Some(&mut progress),
                &self.staging,
                prepared._stage.path(),
            )
            .map_err(|e| e.to_string())?;
            if auxiliary_counts.2 > 0 {
                inspection.warnings.push(format!(
                    "Adjacent lyrics sidecars: {} included; {} existing sidecars were invalid and omitted.",
                    auxiliary_counts.0, auxiliary_counts.2
                ));
            }
            if automatic {
                self.record_automatic_archive(
                    destination,
                    &inspection.created_at,
                    inspection.archive_bytes,
                )?;
            }
            Ok(inspection)
        })();
        match result {
            Ok(inspection) => {
                let mut details = inspection
                    .warnings
                    .iter()
                    .filter(|warning| warning.starts_with("Adjacent lyrics sidecars:"))
                    .cloned()
                    .collect::<Vec<_>>();
                if inspection.media.missing > 0 || inspection.media.changed > 0 {
                    details.push(format!(
                        "Audio assets: {} included, {} missing, {} changed, {} omitted.",
                        inspection.media.assets,
                        inspection.media.missing,
                        inspection.media.changed,
                        inspection.media.omitted,
                    ));
                }
                self.update(
                    app,
                    &job,
                    "complete",
                    "Backup created and verified",
                    Some(1),
                    Some(1),
                    (!details.is_empty()).then(|| details.join(" ")),
                );
            }
            Err(error) if job.cancel.load(Ordering::Relaxed) => {
                if automatic {
                    self.set_schedule_canceled(&error);
                }
                self.update(
                    app,
                    &job,
                    "canceled",
                    "Backup canceled",
                    None,
                    None,
                    Some(error),
                );
            }
            Err(error) => {
                if automatic {
                    self.set_schedule_error(&error);
                }
                self.update(
                    app,
                    &job,
                    "failed",
                    "Backup failed",
                    None,
                    None,
                    Some(error),
                );
            }
        }
        self.release_job(job_id);
    }

    fn media_inputs(
        &self,
        include_media: bool,
        canceled: &AtomicBool,
    ) -> Result<
        (
            Vec<MediaInput>,
            Vec<crate::backup::AuxiliaryInput>,
            BackupMediaSummary,
            (usize, usize, usize),
        ),
        String,
    > {
        let assets = self.store.list_assets()?;
        if !include_media {
            return Ok((
                Vec::new(),
                Vec::new(),
                BackupMediaSummary {
                    included: false,
                    assets: 0,
                    missing: 0,
                    changed: 0,
                    omitted: assets.len() as u64,
                },
                (0, 0, 0),
            ));
        }
        let mut input = Vec::new();
        let mut auxiliary = Vec::new();
        let mut auxiliary_omitted = 0usize;
        let mut auxiliary_changed = 0usize;
        let mut missing = 0u64;
        let mut changed = 0u64;
        for asset in &assets {
            if canceled.load(Ordering::Relaxed) {
                return Err("Backup canceled".into());
            }
            let path = PathBuf::from(&asset.path);
            if !path.is_file() {
                missing += 1;
                continue;
            }
            let actual = hash_file_cancellable(&path, canceled)?;
            if actual.eq_ignore_ascii_case(&asset.hash) {
                match crate::backup::snapshot_lyrics_sidecar(asset, &actual) {
                    crate::backup::AuxiliaryDiscovery::Included(sidecar) => auxiliary.push(sidecar),
                    crate::backup::AuxiliaryDiscovery::Missing => auxiliary_omitted += 1,
                    crate::backup::AuxiliaryDiscovery::Invalid => auxiliary_changed += 1,
                }
                input.push(MediaInput {
                    asset_id: asset.id.clone(),
                    source_path: path,
                    expected_sha256: Some(asset.hash.clone()),
                });
            } else {
                changed += 1;
                auxiliary_changed += 1;
            }
        }
        let count = input.len() as u64;
        let auxiliary_counts = (auxiliary.len(), auxiliary_omitted, auxiliary_changed);
        Ok((
            input,
            auxiliary,
            BackupMediaSummary {
                included: count > 0,
                assets: count,
                missing,
                changed,
                omitted: 0,
            },
            auxiliary_counts,
        ))
    }

    fn update(
        &self,
        app: &AppHandle,
        job: &JobRecord,
        state: &str,
        phase: &str,
        completed: Option<u64>,
        total: Option<u64>,
        message: Option<String>,
    ) {
        if let Ok(mut status) = job.status.lock() {
            status.state = state.into();
            status.phase = Some(phase.into());
            status.completed = completed;
            status.total = total;
            status.message = message;
            let _ = app.emit("backup-progress", status.clone());
        }
    }

    pub fn get_schedule(&self) -> Result<BackupSchedule, String> {
        let mut schedule: BackupSchedule = self
            .store
            .get_json_preference(SCHEDULE_KEY)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        schedule.next_due_at = next_due(&schedule).map(|date| date.to_rfc3339());
        schedule.destination_available_bytes = schedule
            .destination
            .as_deref()
            .and_then(free_bytes_available);
        Ok(schedule)
    }

    pub fn set_schedule(&self, mut schedule: BackupSchedule) -> Result<BackupSchedule, String> {
        schedule.retention_daily = schedule.retention_daily.clamp(1, 365);
        schedule.retention_weekly = schedule.retention_weekly.clamp(1, 104);
        schedule.interval_days = schedule.interval_days.clamp(1, 365);
        if schedule.enabled {
            let destination = schedule
                .destination
                .as_ref()
                .ok_or("Choose a backup destination first")?;
            fs::create_dir_all(destination)
                .map_err(|e| format!("Cannot create backup destination: {e}"))?;
            let canonical = fs::canonicalize(destination).map_err(|e| e.to_string())?;
            let previous = self.get_schedule().ok();
            let unchanged = previous
                .as_ref()
                .is_some_and(|old| old.destination.as_ref() == Some(&canonical));
            if let Some(old) = previous.as_ref().filter(|old| !unchanged) {
                if let (Some(old_destination), Some(old_marker)) =
                    (&old.destination, &old.destination_marker)
                {
                    if let Ok(old_path) = destination_marker_path(old_destination, old_marker) {
                        if fs::read_to_string(&old_path).ok().as_deref()
                            == Some(old_marker.as_str())
                        {
                            let _ = fs::remove_file(old_path);
                        }
                    }
                }
            }
            let marker = if unchanged {
                previous.and_then(|old| old.destination_marker)
            } else {
                None
            }
            .unwrap_or_else(|| Uuid::new_v4().to_string());
            let marker_path = destination_marker_path(&canonical, &marker)?;
            if marker_path.exists() {
                if fs::read_to_string(&marker_path).ok().as_deref() != Some(marker.as_str()) {
                    return Err("Automatic backup destination marker is invalid".into());
                }
            } else {
                let mut marker_file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&marker_path)
                    .map_err(|e| format!("Backup destination is not writable: {e}"))?;
                marker_file
                    .write_all(marker.as_bytes())
                    .map_err(|e| e.to_string())?;
                marker_file.sync_all().map_err(|e| e.to_string())?;
            }
            let probe = canonical.join(format!(".burrata-write-check-{}", Uuid::new_v4()));
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)
                .map_err(|e| format!("Backup destination is not writable: {e}"))?;
            fs::remove_file(probe).map_err(|e| e.to_string())?;
            schedule.destination = Some(canonical);
            schedule.destination_available_bytes = free_bytes_available(
                schedule
                    .destination
                    .as_deref()
                    .expect("destination was set"),
            );
            schedule.destination_marker = Some(marker);
            if schedule.enabled_at.is_none() {
                schedule.enabled_at = Some(Utc::now().to_rfc3339());
            }
        } else {
            schedule.enabled_at = None;
            schedule.destination_available_bytes = schedule
                .destination
                .as_deref()
                .and_then(free_bytes_available);
            if let (Some(destination), Some(marker)) =
                (&schedule.destination, &schedule.destination_marker)
            {
                if let Ok(marker_path) = destination_marker_path(destination, marker) {
                    if fs::read_to_string(&marker_path).ok().as_deref() == Some(marker.as_str()) {
                        let _ = fs::remove_file(marker_path);
                    }
                }
            }
            schedule.destination_marker = None;
        }
        schedule.next_due_at = next_due(&schedule).map(|date| date.to_rfc3339());
        self.store.set_json_preference(
            SCHEDULE_KEY,
            &serde_json::to_value(&schedule).map_err(|e| e.to_string())?,
        )?;
        Ok(schedule)
    }

    pub fn start_scheduler(self: &Arc<Self>, app: AppHandle) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.tick().await;
            loop {
                interval.tick().await;
                if manager.schedule_job_running.load(Ordering::Acquire) {
                    continue;
                }
                let Ok(schedule) = manager.get_schedule() else {
                    continue;
                };
                if !schedule.enabled || !schedule_due(&schedule, Utc::now()) {
                    continue;
                }
                let Some(destination) = schedule.destination.clone() else {
                    continue;
                };
                if !destination_is_mounted(&destination, schedule.destination_marker.as_deref()) {
                    manager.set_schedule_error("Automatic backup destination is unavailable.");
                    continue;
                }
                manager.set_schedule_status("running");
                manager.schedule_job_running.store(true, Ordering::Release);
                let file =
                    destination.join(format!("burrata-auto-{}.burrata-backup", Uuid::new_v4()));
                let job_id =
                    match manager.start_create(app.clone(), file, schedule.include_media, true) {
                        Ok(id) => id,
                        Err(error) => {
                            manager.set_schedule_error(&error);
                            manager.schedule_job_running.store(false, Ordering::Release);
                            continue;
                        }
                    };
                let manager_for_wait = manager.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        match manager_for_wait.status(&job_id).map(|status| status.state) {
                            Ok(state)
                                if matches!(state.as_str(), "complete" | "failed" | "canceled") =>
                            {
                                break
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    manager_for_wait
                        .schedule_job_running
                        .store(false, Ordering::Release);
                });
            }
        });
    }

    fn set_schedule_error(&self, error: &str) {
        self.record_schedule_failure(error, "failed");
    }

    fn set_schedule_canceled(&self, message: &str) {
        self.record_schedule_failure(message, "canceled");
    }

    fn record_schedule_failure(&self, error: &str, status: &str) {
        if let Ok(mut schedule) = self.get_schedule() {
            schedule.last_error = Some(error.chars().take(512).collect());
            schedule.last_status = Some(status.into());
            schedule.retry_count = schedule.retry_count.saturating_add(1).min(8);
            let minutes = 10u64.saturating_mul(1u64 << schedule.retry_count.saturating_sub(1));
            let backoff = std::time::Duration::from_secs(minutes.min(24 * 60) * 60);
            schedule.retry_after_at = Some(
                (Utc::now() + Duration::from_std(backoff).unwrap_or(Duration::hours(24)))
                    .to_rfc3339(),
            );
            schedule.next_due_at = next_due(&schedule).map(|date| date.to_rfc3339());
            let _ = self.store.set_json_preference(
                SCHEDULE_KEY,
                &serde_json::to_value(schedule).unwrap_or(Value::Null),
            );
        }
    }

    fn set_schedule_status(&self, status: &str) {
        if let Ok(mut schedule) = self.get_schedule() {
            schedule.last_status = Some(status.into());
            let _ = self.store.set_json_preference(
                SCHEDULE_KEY,
                &serde_json::to_value(schedule).unwrap_or(Value::Null),
            );
        }
    }

    fn reserve_job(&self, job_id: &str) -> Result<(), String> {
        let mut active = self
            .active_job
            .lock()
            .map_err(|_| "Backup active-job lock was poisoned")?;
        if active.is_some() {
            return Err("Another backup or restore is already running".into());
        }
        *active = Some(job_id.into());
        Ok(())
    }

    fn trim_finished_jobs(&self) {
        let Ok(mut jobs) = self.jobs.lock() else {
            return;
        };
        while jobs.len() > 100 {
            let removable = jobs.iter().find_map(|(id, record)| {
                record.status.lock().ok().and_then(|status| {
                    if matches!(status.state.as_str(), "complete" | "failed" | "canceled") {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
            });
            let Some(removable) = removable else {
                break;
            };
            jobs.remove(&removable);
        }
    }

    fn release_job(&self, job_id: &str) {
        if let Ok(mut active) = self.active_job.lock() {
            if active.as_deref() == Some(job_id) {
                *active = None;
            }
        }
    }

    fn record_automatic_archive(
        &self,
        path: &Path,
        created_at: &str,
        archive_bytes: u64,
    ) -> Result<(), String> {
        let mut owned: Vec<OwnedArchive> = self
            .store
            .get_json_preference(OWNED_ARCHIVES_KEY)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let schedule = self.get_schedule()?;
        let cadence = match schedule.interval {
            BackupInterval::Weekly => "weekly",
            BackupInterval::Daily | BackupInterval::Custom => {
                let current =
                    DateTime::parse_from_rfc3339(created_at).map_err(|e| e.to_string())?;
                let has_weekly = owned.iter().any(|row| {
                    DateTime::parse_from_rfc3339(&row.created_at)
                        .ok()
                        .is_some_and(|old| old.iso_week() == current.iso_week())
                });
                if schedule.interval == BackupInterval::Custom && schedule.interval_days < 7 {
                    "daily"
                } else if has_weekly {
                    "daily"
                } else {
                    "weekly"
                }
            }
        };
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid archive filename")?
            .to_owned();
        if !file_name.starts_with("burrata-auto-")
            || !file_name.ends_with(".burrata-backup")
            || file_name.len() > MAX_ARCHIVE_FILE_NAME
        {
            return Err("Automatic backup filename is invalid".into());
        }
        let archive_destination = path
            .parent()
            .ok_or("Automatic backup path has no destination")?;
        let archive_destination =
            fs::canonicalize(archive_destination).map_err(|e| e.to_string())?;
        owned.push(OwnedArchive {
            file_name,
            created_at: created_at.into(),
            cadence: cadence.into(),
            destination: archive_destination,
        });
        owned.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let mut kept = Vec::new();
        for row in owned.iter().rev() {
            let limit = if row.cadence == "weekly" {
                schedule.retention_weekly
            } else {
                schedule.retention_daily
            };
            let count = kept
                .iter()
                .filter(|old: &&OwnedArchive| {
                    old.cadence == row.cadence && old.destination == row.destination
                })
                .count();
            let path = row.destination.join(&row.file_name);
            if count < limit {
                kept.push(row.clone());
            } else if path.parent() == Some(row.destination.as_path())
                && row.file_name.starts_with("burrata-auto-")
                && row.file_name.ends_with(".burrata-backup")
                && path.is_file()
            {
                let _ = fs::remove_file(path);
            }
        }
        kept.reverse();
        self.store.set_json_preference(
            OWNED_ARCHIVES_KEY,
            &serde_json::to_value(&kept).map_err(|e| e.to_string())?,
        )?;
        let mut schedule = schedule;
        schedule.last_success_at = Some(created_at.into());
        schedule.last_error = None;
        schedule.last_status = Some("complete".into());
        schedule.last_archive_bytes = Some(archive_bytes);
        schedule.retry_count = 0;
        schedule.retry_after_at = None;
        schedule.next_due_at = next_due(&schedule).map(|date| date.to_rfc3339());
        self.store.set_json_preference(
            SCHEDULE_KEY,
            &serde_json::to_value(schedule).map_err(|e| e.to_string())?,
        )?;
        Ok(())
    }
}

fn hash_file_cancellable(path: &Path, canceled: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        if canceled.load(Ordering::Relaxed) {
            return Err("Backup canceled".into());
        }
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn free_bytes_available(path: &Path) -> Option<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a NUL-terminated path and `stats` points to writable
    // storage of the exact statvfs type. The pointer is read only on success.
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    u64::try_from(stats.f_bavail)
        .ok()?
        .checked_mul(u64::try_from(stats.f_frsize).ok()?)
}

#[cfg(not(unix))]
fn free_bytes_available(_path: &Path) -> Option<u64> {
    None
}

fn sqlite_user_version(path: &Path) -> Result<u32, String> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("Cannot read staged library schema version: {error}"))?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("Cannot read staged library schema version: {error}"))?;
    u32::try_from(version).map_err(|_| "Staged library schema version is invalid".into())
}

fn sqlite_total_changes(connection: &rusqlite::Connection) -> Result<u64, String> {
    connection
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .map_err(|error| error.to_string())
}

fn restore_options(
    snapshot_id: &str,
    mode: RestoreMode,
    component_ids: &[String],
    path_mappings: Vec<PathMapping>,
) -> RestoreStateOptions {
    RestoreStateOptions {
        mode,
        snapshot_id: snapshot_id.into(),
        path_mappings,
        current_asset_paths: Vec::new(),
        current_music_root: None,
        current_playback_snapshot: None,
        include_library: component_ids.iter().any(|id| id == "library"),
        include_history: component_ids.iter().any(|id| id == "history"),
        include_settings: component_ids.iter().any(|id| id == "settings"),
        include_playback: component_ids.iter().any(|id| id == "playback"),
    }
}

fn schedule_due(schedule: &BackupSchedule, now: DateTime<Utc>) -> bool {
    if let Some(retry) = schedule.retry_after_at.as_deref() {
        if let Ok(retry) = DateTime::parse_from_rfc3339(retry) {
            return now >= retry.with_timezone(&Utc);
        }
    }
    next_regular_due(schedule).is_some_and(|due| now >= due)
}

fn next_regular_due(schedule: &BackupSchedule) -> Option<DateTime<Utc>> {
    let interval = match schedule.interval {
        BackupInterval::Daily => Duration::days(1),
        BackupInterval::Weekly => Duration::days(7),
        BackupInterval::Custom => Duration::days(i64::from(schedule.interval_days.clamp(1, 365))),
    };
    let reference = schedule
        .last_success_at
        .as_deref()
        .or(schedule.enabled_at.as_deref());
    let Some(reference) = reference else {
        return Some(Utc::now());
    };
    DateTime::parse_from_rfc3339(reference)
        .ok()
        .map(|date| date.with_timezone(&Utc) + interval)
}

fn next_due(schedule: &BackupSchedule) -> Option<DateTime<Utc>> {
    if !schedule.enabled {
        return None;
    }
    if let Some(retry) = schedule.retry_after_at.as_deref() {
        if let Ok(date) = DateTime::parse_from_rfc3339(retry) {
            return Some(date.with_timezone(&Utc));
        }
    }
    next_regular_due(schedule)
}

fn destination_marker_path(destination: &Path, marker: &str) -> Result<PathBuf, String> {
    let id =
        Uuid::parse_str(marker).map_err(|_| "Automatic backup destination marker is invalid")?;
    if id.to_string() != marker {
        return Err("Automatic backup destination marker is invalid".into());
    }
    Ok(destination.join(format!(".burrata-backup-target-{marker}")))
}

fn destination_is_mounted(destination: &Path, marker: Option<&str>) -> bool {
    let Some(marker) = marker else {
        return false;
    };
    let Ok(marker_path) = destination_marker_path(destination, marker) else {
        return false;
    };
    fs::read_to_string(marker_path).ok().as_deref() == Some(marker)
}

fn media_root_previews(archive: &ValidatedArchive) -> Result<Vec<BackupMediaRootPreview>, String> {
    let settings_path = archive
        .component_path("settings")
        .ok_or("Backup has no settings component")?;
    let settings: backup_state::SettingsArchive =
        serde_json::from_reader(File::open(settings_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    // The library database is the authority for an asset's expected hash.
    // Path hints are portable, but are not allowed to supply integrity data.
    let library_hashes = archive_library_asset_hashes(archive)?;
    let media_ids: HashMap<String, (u64, String)> = archive
        .manifest
        .components
        .iter()
        .filter(|component| component.id.starts_with("media:"))
        .map(|component| {
            (
                component.id[6..].to_owned(),
                (component.bytes, component.sha256.to_ascii_lowercase()),
            )
        })
        .collect();
    let mut roots: HashMap<String, BackupMediaRootPreview> = HashMap::new();
    for hint in settings.asset_path_hints {
        if !library_hashes.contains_key(&hint.asset_id)
            || !valid_asset_root_key(&hint.root_key)
            || hint.relative_path.len() > 4096
            || safe_relative_path(&hint.relative_path).is_none()
        {
            continue;
        }
        let expected_hash = &library_hashes[&hint.asset_id];
        let bytes = media_ids
            .get(&hint.asset_id)
            .filter(|(_, hash)| hash.eq_ignore_ascii_case(expected_hash))
            .map(|(bytes, _)| *bytes);
        let row = roots
            .entry(hint.root_key.clone())
            .or_insert_with(|| BackupMediaRootPreview {
                root_key: hint.root_key.clone(),
                label: if hint.root_key == "music" {
                    "Music folder".into()
                } else {
                    "External media folder".into()
                },
                assets: 0,
                required_bytes: 0,
                destination_token: None,
                destination_label: None,
                reusable_same_hash: 0,
                copy_count: 0,
                conflicts: 0,
                missing_destination: true,
                available_bytes: None,
                lyrics_sidecars: 0,
                lyrics_copy_count: 0,
                lyrics_reusable_same_hash: 0,
                lyrics_conflicts: 0,
            });
        row.assets += 1;
        // `requiredBytes` is the copy plan for media components actually
        // carried by this archive. Metadata-only hints are link candidates,
        // not payloads to copy.
        if let Some(bytes) = bytes {
            row.required_bytes = row.required_bytes.saturating_add(bytes);
        }
        if bytes.is_some() {
            if let Some(sidecar) = archive
                .manifest
                .auxiliary_assets
                .iter()
                .find(|sidecar| sidecar.asset_id == hint.asset_id)
            {
                if let Some(sidecar_bytes) = archive
                    .manifest
                    .components
                    .iter()
                    .find(|component| component.id == sidecar.component_id)
                    .map(|component| component.bytes)
                {
                    row.lyrics_sidecars += 1;
                    row.required_bytes = row.required_bytes.saturating_add(sidecar_bytes);
                }
            }
        }
    }
    let mut values = roots.into_values().collect::<Vec<_>>();
    values.sort_by(|a, b| a.root_key.cmp(&b.root_key));
    Ok(values)
}

fn valid_asset_root_key(root_key: &str) -> bool {
    if root_key == "music" {
        return true;
    }
    root_key.strip_prefix("external/").is_some_and(|token| {
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn archive_library_asset_hashes(
    archive: &ValidatedArchive,
) -> Result<HashMap<String, String>, String> {
    let Some(path) = archive.component_path("library") else {
        return Ok(HashMap::new());
    };
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Cannot read backup audio metadata: {error}"))?;
    // The normal archive validator also caps table rows, but keep this direct
    // integrity lookup bounded as a defense in depth for future callers.
    const MAX_ARCHIVE_ASSETS: usize = 2_000_000;
    let mut statement = connection
        .prepare(
            "SELECT id,hash FROM local_assets WHERE length(id)<=256 AND length(hash)=64 ORDER BY id LIMIT 2000001",
        )
        .map_err(|error| format!("Cannot read backup audio metadata: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("Cannot read backup audio metadata: {error}"))?;
    let mut hashes = HashMap::new();
    for (index, row) in rows.enumerate() {
        if index >= MAX_ARCHIVE_ASSETS {
            return Err("Backup audio metadata exceeds the asset limit".into());
        }
        let (id, hash) = row.map_err(|error| error.to_string())?;
        if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            hashes.insert(id, hash.to_ascii_lowercase());
        }
    }
    Ok(hashes)
}

fn safe_relative_path(value: &str) -> Option<PathBuf> {
    // Path::components is platform-dependent. Reject both separator styles and
    // drive/stream syntax so an archive made on one OS cannot become traversal
    // when restored on another.
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains(':')
    {
        return None;
    }
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_owned())
}

fn seal_restore_root(canonical_path: &Path) -> Result<RegisteredRestoreRoot, String> {
    let metadata = fs::symlink_metadata(canonical_path)
        .map_err(|_| "Selected audio folder changed; choose the folder again")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Selected audio folder changed; choose the folder again".into());
    }
    let resolved = fs::canonicalize(canonical_path)
        .map_err(|_| "Selected audio folder changed; choose the folder again")?;
    if resolved != canonical_path {
        return Err("Selected audio folder changed; choose the folder again".into());
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        RestoreRootIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    };
    #[cfg(windows)]
    let identity = windows_restore_root_identity(canonical_path)?;
    #[cfg(not(any(unix, windows)))]
    let identity = RestoreRootIdentity::Created(
        metadata
            .created()
            .map_err(|_| "Cannot verify selected audio folder identity; choose it again")?,
    );
    Ok(RegisteredRestoreRoot {
        canonical_path: canonical_path.to_owned(),
        identity,
    })
}

#[cfg(windows)]
fn windows_restore_root_identity(canonical_path: &Path) -> Result<RestoreRootIdentity, String> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    };

    let wide_path = canonical_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Cannot verify selected audio folder identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let queried = unsafe { GetFileInformationByHandle(handle, &mut info) } != 0;
    let query_error = (!queried).then(std::io::Error::last_os_error);
    let closed = unsafe { CloseHandle(handle) } != 0;
    if let Some(error) = query_error {
        return Err(format!(
            "Cannot verify selected audio folder identity: {error}"
        ));
    }
    if !closed {
        return Err(format!(
            "Cannot verify selected audio folder identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    if info.dwVolumeSerialNumber == 0 || file_index == 0 {
        return Err("Windows did not provide a stable audio folder identity".into());
    }
    Ok(RestoreRootIdentity::Windows {
        volume_serial: info.dwVolumeSerialNumber,
        file_index,
    })
}

fn validate_restore_root(root: &RegisteredRestoreRoot) -> Result<(), String> {
    let current = seal_restore_root(&root.canonical_path)?;
    if current.identity != root.identity {
        return Err("Selected audio folder changed; choose the folder again".into());
    }
    Ok(())
}

/// Create each directory beneath a selected root without following a symlink
/// already present in the path. The final canonical parent must remain within
/// the canonical root before an asset is published.
fn ensure_safe_media_parent(base: &Path, relative: &Path) -> Result<PathBuf, String> {
    let canonical_base = fs::canonicalize(base).map_err(|e| e.to_string())?;
    let parent = relative.parent().ok_or("Audio destination has no parent")?;
    let mut current = canonical_base.clone();
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("Audio destination contains an unsafe path".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("Audio destination contains a symlink or non-folder".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|e| e.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
        let canonical = fs::canonicalize(&current).map_err(|e| e.to_string())?;
        if !canonical.starts_with(&canonical_base) {
            return Err("Audio destination escapes its selected folder".into());
        }
        current = canonical;
    }
    Ok(current)
}

fn analyze_restore_root(
    record: &PreviewRecord,
    preview: &mut BackupMediaRootPreview,
    destination: &Path,
) -> Result<(), String> {
    let settings_path = record
        .archive
        .component_path("settings")
        .ok_or("Backup has no settings component")?;
    let settings: backup_state::SettingsArchive =
        serde_json::from_reader(File::open(settings_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let hints = settings
        .asset_path_hints
        .into_iter()
        .filter(|hint| hint.root_key == preview.root_key)
        .map(|hint| (hint.asset_id, hint.relative_path))
        .collect::<HashMap<_, _>>();
    let library_hashes = archive_library_asset_hashes(&record.archive)?;
    let canonical_base = fs::canonicalize(destination).map_err(|error| error.to_string())?;
    let mut audio_installable = HashMap::<String, bool>::new();
    for (asset_id, relative_hint) in hints.iter() {
        let Some(expected_hash) = library_hashes.get(asset_id) else {
            continue;
        };
        let Some(relative) = safe_relative_path(relative_hint) else {
            continue;
        };
        let target = destination.join(relative);
        let packaged = record
            .archive
            .manifest
            .components
            .iter()
            .find(|component| component.id == format!("media:{asset_id}"));
        // Included payloads must agree with the hash in the validated library
        // row. A disagreement is not usable as either a copy or link source.
        let packaged_valid =
            packaged.filter(|component| component.sha256.eq_ignore_ascii_case(expected_hash));
        if packaged.is_some() && packaged_valid.is_none() {
            preview.conflicts += 1;
            audio_installable.insert(asset_id.to_owned(), false);
            continue;
        }
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if packaged_valid.is_some() {
                    preview.copy_count += 1;
                    audio_installable.insert(asset_id.to_owned(), true);
                }
            }
            Ok(_) => {
                let reusable = fs::canonicalize(&target)
                    .ok()
                    .is_some_and(|path| path.starts_with(&canonical_base))
                    && target.is_file()
                    && media::hash_file(&target)
                        .is_ok_and(|hash| hash.eq_ignore_ascii_case(expected_hash));
                if reusable {
                    preview.reusable_same_hash += 1;
                } else {
                    preview.conflicts += 1;
                }
                if packaged_valid.is_some() {
                    audio_installable.insert(asset_id.to_owned(), reusable);
                }
            }
            Err(_) => {
                if packaged_valid.is_some() {
                    preview.conflicts += 1;
                    audio_installable.insert(asset_id.to_owned(), false);
                }
            }
        }
    }
    for attachment in &record.archive.manifest.auxiliary_assets {
        let Some(relative) = hints
            .get(&attachment.asset_id)
            .and_then(|relative| safe_relative_path(relative))
        else {
            continue;
        };
        let Some(component) = record
            .archive
            .manifest
            .components
            .iter()
            .find(|component| component.id == attachment.component_id)
        else {
            continue;
        };
        if !audio_installable
            .get(&attachment.asset_id)
            .copied()
            .unwrap_or(false)
        {
            // A sidecar can only be published if its parent audio asset will
            // be installed or is already verified at the destination.
            preview.lyrics_conflicts += 1;
            continue;
        }
        let target = destination.join(relative).with_extension("lrc");
        match fs::symlink_metadata(&target) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && media::hash_file(&target)
                        .is_ok_and(|hash| hash.eq_ignore_ascii_case(&component.sha256)) =>
            {
                preview.lyrics_reusable_same_hash += 1;
            }
            Ok(_) => preview.lyrics_conflicts += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                preview.lyrics_copy_count += 1;
            }
            Err(_) => preview.lyrics_conflicts += 1,
        }
    }
    Ok(())
}

fn copy_verified_media(
    record: &PreviewRecord,
    request: &RestoreRequest,
    canceled: &AtomicBool,
    mut progress: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<(Vec<PathMapping>, Vec<String>), String> {
    let settings_path = record
        .archive
        .component_path("settings")
        .ok_or("Backup has no settings component")?;
    let settings: backup_state::SettingsArchive =
        serde_json::from_reader(File::open(settings_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let library_hashes = archive_library_asset_hashes(&record.archive)?;
    let hints = settings
        .asset_path_hints
        .into_iter()
        .map(|hint| (hint.asset_id, (hint.root_key, hint.relative_path)))
        .collect::<HashMap<_, _>>();
    let mut mappings = Vec::new();
    let mut root_bindings = HashMap::new();
    for mapping in &request.path_mappings {
        let registered = record
            .destination_tokens
            .get(&mapping.destination_token)
            .ok_or("Restore destination token is invalid")?;
        validate_restore_root(registered)?;
        root_bindings.insert(mapping.root_key.as_str(), registered);
        mappings.push(PathMapping {
            root_key: mapping.root_key.clone(),
            destination: registered.canonical_path.clone(),
        });
    }
    let roots = mappings
        .iter()
        .map(|mapping| (mapping.root_key.as_str(), mapping.destination.as_path()))
        .collect::<HashMap<_, _>>();
    let selected = request
        .component_ids
        .iter()
        .filter(|id| id.starts_with("media:"))
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    let mut installed_media = HashMap::<String, PathBuf>::new();
    for (index, id) in selected.iter().enumerate() {
        if canceled.load(Ordering::Relaxed) {
            return Err("Restore canceled".into());
        }
        let asset_id = &id[6..];
        let Some(component) = record
            .archive
            .manifest
            .components
            .iter()
            .find(|component| &component.id == *id)
        else {
            return Err("Selected media component is missing".into());
        };
        if !library_hashes
            .get(asset_id)
            .is_some_and(|hash| component.sha256.eq_ignore_ascii_case(hash))
        {
            warnings.push(format!(
                "Audio asset {asset_id} payload does not match its library hash and was not copied."
            ));
            if let Some(report) = progress.as_deref_mut() {
                report(index + 1, selected.len());
            }
            continue;
        }
        let Some((root_key, relative)) = hints.get(asset_id) else {
            warnings.push(format!(
                "Audio asset {asset_id} has no safe folder mapping and remains unavailable."
            ));
            if let Some(report) = progress.as_deref_mut() {
                report(index + 1, selected.len());
            }
            continue;
        };
        let Some(base) = roots.get(root_key.as_str()).copied() else {
            warnings.push(format!(
                "Choose a destination for {root_key} to restore its audio."
            ));
            if let Some(report) = progress.as_deref_mut() {
                report(index + 1, selected.len());
            }
            continue;
        };
        let registered_root = root_bindings
            .get(root_key.as_str())
            .copied()
            .ok_or("Restore destination token is invalid")?;
        validate_restore_root(registered_root)?;
        let Some(relative) = safe_relative_path(relative) else {
            warnings.push(format!(
                "Audio asset {asset_id} has an unsafe path hint and remains unavailable."
            ));
            if let Some(report) = progress.as_deref_mut() {
                report(index + 1, selected.len());
            }
            continue;
        };
        let target = base.join(&relative);
        let canonical_base = fs::canonicalize(base).map_err(|e| e.to_string())?;
        let canonical_parent = ensure_safe_media_parent(base, &relative)?;
        if !canonical_parent.starts_with(&canonical_base) {
            warnings.push(format!(
                "Audio asset {asset_id} destination escapes its selected folder and was skipped."
            ));
            if let Some(report) = progress.as_deref_mut() {
                report(index + 1, selected.len());
            }
            continue;
        }
        if target.exists() {
            let canonical_target = fs::canonicalize(&target).ok();
            if canonical_target
                .as_ref()
                .is_some_and(|path| path.starts_with(&canonical_base))
                && target.is_file()
                && media::hash_file(&target)
                    .is_ok_and(|hash| hash.eq_ignore_ascii_case(&component.sha256))
            {
                validate_restore_root(registered_root)?;
                installed_media.insert(asset_id.to_owned(), target.clone());
                if let Some(report) = progress.as_deref_mut() {
                    report(index + 1, selected.len());
                }
                continue;
            }
            warnings.push(format!("Audio asset {asset_id} was not overwritten because another file occupies its destination."));
            if let Some(report) = progress.as_deref_mut() {
                report(index + 1, selected.len());
            }
            continue;
        }
        let source = record
            .archive
            .component_path(id)
            .ok_or("Audio component file is missing")?;
        let mut source = File::open(source).map_err(|e| e.to_string())?;
        let mut temp = NamedTempFile::new_in(&canonical_parent).map_err(|e| e.to_string())?;
        let mut hasher = sha2::Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 128 * 1024];
        loop {
            if canceled.load(Ordering::Relaxed) {
                return Err("Restore canceled".into());
            }
            let read = source.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            bytes = bytes
                .checked_add(read as u64)
                .ok_or("Audio size overflow")?;
            if bytes > component.bytes {
                return Err("Audio file exceeded verified size".into());
            }
            temp.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
            hasher.update(&buffer[..read]);
        }
        let hash = format!("{:x}", hasher.finalize());
        if bytes != component.bytes || !hash.eq_ignore_ascii_case(&component.sha256) {
            return Err("Audio bytes changed after archive validation".into());
        }
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        validate_restore_root(registered_root)?;
        match temp.persist_noclobber(&target) {
            Ok(_) => {
                sync_directory(&canonical_parent).map_err(|e| e.to_string())?;
                installed_media.insert(asset_id.to_owned(), target.clone());
            }
            Err(error) => {
                if target.is_file()
                    && media::hash_file(&target)
                        .is_ok_and(|hash| hash.eq_ignore_ascii_case(&component.sha256))
                {
                    installed_media.insert(asset_id.to_owned(), target.clone());
                    if let Some(report) = progress.as_deref_mut() {
                        report(index + 1, selected.len());
                    }
                    continue;
                }
                warnings.push(format!(
                    "Audio asset {asset_id} was not copied because its destination changed."
                ));
                let _ = error.file.close();
            }
        }
        if let Some(report) = progress.as_deref_mut() {
            report(index + 1, selected.len());
        }
    }
    for attachment in &record.archive.manifest.auxiliary_assets {
        if !request
            .component_ids
            .iter()
            .any(|id| id == &attachment.component_id)
            || !selected
                .iter()
                .any(|id| id.as_str() == format!("media:{}", attachment.asset_id))
        {
            continue;
        }
        let Some(audio_target) = installed_media.get(&attachment.asset_id) else {
            warnings.push(format!(
                "Lyrics sidecar for audio asset {} was not restored because its audio file was not installed or verified.",
                attachment.asset_id
            ));
            continue;
        };
        let Some(component) = record
            .archive
            .manifest
            .components
            .iter()
            .find(|component| component.id == attachment.component_id)
        else {
            warnings.push(format!(
                "Lyrics sidecar for audio asset {} has no valid archive component.",
                attachment.asset_id
            ));
            continue;
        };
        let target = audio_target.with_extension("lrc");
        let parent = target
            .parent()
            .ok_or("Lyrics sidecar destination has no parent")?;
        let Some((root_key, _)) = hints.get(&attachment.asset_id) else {
            warnings.push(format!(
                "Lyrics sidecar for audio asset {} has no safe folder mapping.",
                attachment.asset_id
            ));
            continue;
        };
        let Some(base) = roots.get(root_key.as_str()).copied() else {
            continue;
        };
        let registered_root = root_bindings
            .get(root_key.as_str())
            .copied()
            .ok_or("Restore destination token is invalid")?;
        validate_restore_root(registered_root)?;
        let canonical_base = fs::canonicalize(base).map_err(|error| error.to_string())?;
        let canonical_parent = fs::canonicalize(parent).map_err(|error| error.to_string())?;
        if !canonical_parent.starts_with(&canonical_base) {
            warnings.push(format!(
                "Lyrics sidecar for audio asset {} escapes its selected folder and was skipped.",
                attachment.asset_id
            ));
            continue;
        }
        match fs::symlink_metadata(&target) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && media::hash_file(&target)
                        .is_ok_and(|hash| hash.eq_ignore_ascii_case(&component.sha256)) =>
            {
                continue;
            }
            Ok(_) => {
                warnings.push(format!(
                    "Lyrics sidecar for audio asset {} was not overwritten because its destination is occupied.",
                    attachment.asset_id
                ));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                warnings.push(format!(
                    "Lyrics sidecar for audio asset {} could not be checked: {error}",
                    attachment.asset_id
                ));
                continue;
            }
        }
        let source = record
            .archive
            .component_path(&attachment.component_id)
            .ok_or("Lyrics sidecar archive data is missing")?;
        validate_restore_root(registered_root)?;
        if let Err(error) = copy_auxiliary_noclobber(source, &target, component) {
            warnings.push(format!(
                "Lyrics sidecar for audio asset {} was not restored: {error}",
                attachment.asset_id
            ));
        }
    }
    for root in root_bindings.values() {
        validate_restore_root(root)?;
    }
    Ok((mappings, warnings))
}

fn copy_auxiliary_noclobber(
    source_path: &Path,
    target: &Path,
    component: &crate::backup::BackupComponent,
) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or("Lyrics sidecar destination has no parent")?;
    let mut source = File::open(source_path).map_err(|error| error.to_string())?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    let mut digest = sha2::Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or("Lyrics sidecar size overflow")?;
        if bytes > crate::backup::MAX_LYRICS_SIDECAR_BYTES || bytes > component.bytes {
            return Err("Lyrics sidecar exceeded its verified size".into());
        }
        temporary
            .write_all(&buffer[..read])
            .map_err(|error| error.to_string())?;
        digest.update(&buffer[..read]);
    }
    let digest = format!("{:x}", digest.finalize());
    if bytes != component.bytes || !digest.eq_ignore_ascii_case(&component.sha256) {
        return Err("Lyrics sidecar checksum changed after archive validation".into());
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary.persist_noclobber(target).map_err(|error| {
        let message = format!(
            "Cannot publish lyrics sidecar without overwriting: {}",
            error.error
        );
        let _ = error.file.close();
        message
    })?;
    sync_directory(parent).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingRestoreWire {
    transaction_id: String,
    snapshot_id: String,
    stage_directory: String,
    recovery_directory: String,
    recovery_archive: String,
    mode: RestoreMode,
    phase: String,
    had_library: bool,
    had_history: bool,
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("Journal path has no parent")?;
    let mut temp = NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut temp, value).map_err(|e| e.to_string())?;
    temp.flush().map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.error.to_string())?;
    crate::backup_archive::sync_directory(parent).map_err(|e| e.to_string())
}

/// Apply a staged, trusted reconstruction before any Store or history
/// connection is opened. A crash during the DB-pair swap is rolled back from
/// the same-directory recovery files.
pub fn apply_pending_restore(data_dir: &Path) -> Result<bool, String> {
    let staging = BackupStaging::open(data_dir).map_err(|e| e.to_string())?;
    apply_pending_restore_with_staging(data_dir, &staging)
}

pub fn apply_pending_restore_with_staging(
    data_dir: &Path,
    staging: &BackupStaging,
) -> Result<bool, String> {
    let journal_path = data_dir.join(PENDING_RESTORE_FILE);
    if !journal_path.exists() {
        return Ok(false);
    }
    let mut pending: PendingRestoreWire =
        serde_json::from_reader(File::open(&journal_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let transaction_id =
        Uuid::parse_str(&pending.transaction_id).map_err(|_| "Pending restore ID is invalid")?;
    Uuid::parse_str(&pending.snapshot_id).map_err(|_| "Pending backup ID is invalid")?;
    let expected_stage = data_dir
        .join(".backup-restore-stage")
        .join(transaction_id.to_string());
    let expected_recovery = data_dir
        .join(".backup-restore-recovery")
        .join(transaction_id.to_string());
    if PathBuf::from(&pending.stage_directory) != expected_stage
        || PathBuf::from(&pending.recovery_directory) != expected_recovery
    {
        return Err("Pending restore paths are invalid".into());
    }
    let expected_recovery_archive = data_dir
        .join("backup-recovery")
        .join(format!("pre-restore-{}.burrata-backup", transaction_id));
    if PathBuf::from(&pending.recovery_archive) != expected_recovery_archive {
        return Err("Pre-restore recovery archive is missing".into());
    }
    match pending.phase.as_str() {
        "prepared" => {
            if !expected_recovery_archive.is_file() {
                return Err("Pre-restore recovery archive is missing".into());
            }
            let recovery_cancel = AtomicBool::new(false);
            let scratch = staging
                .create_profile_stage("pending-restore-check")
                .map_err(|e| e.to_string())?;
            let verified_recovery = inspect_archive_in(
                &expected_recovery_archive,
                &recovery_cancel,
                None,
                Some(scratch.path()),
            )
            .map_err(|error| format!("Pre-restore recovery archive failed validation: {error}"))?;
            if verified_recovery.inspection.snapshot_id.is_empty() {
                return Err("Pre-restore recovery archive identity is invalid".into());
            }
            drop(verified_recovery);
            prepare_recovery_pair(data_dir, &pending)?;
            pending.phase = "swapping".into();
            atomic_json(&journal_path, &pending)?;
            install_staged_pair(data_dir, &pending)?;
            pending.phase = "committed".into();
            atomic_json(&journal_path, &pending)?;
            Ok(true)
        }
        "swapping" => {
            rollback_pair(data_dir, &pending)?;
            // Keep a durable terminal phase until both databases have opened.
            // Deleting the journal here orphaned the staged/recovery copies;
            // deleting those copies first would make another crash unrecoverable.
            pending.phase = "rolled_back".into();
            atomic_json(&journal_path, &pending)?;
            Ok(false)
        }
        "rolled_back" => {
            validate_available_pair(data_dir)?;
            Ok(false)
        }
        "committed" => {
            validate_installed_pair(data_dir)?;
            Ok(true)
        }
        _ => Err("Pending restore phase is invalid".into()),
    }
}

/// Call after Store and PlaybackHistory have both opened successfully.
pub fn finalize_pending_restore(data_dir: &Path) -> Result<(), String> {
    let journal_path = data_dir.join(PENDING_RESTORE_FILE);
    if !journal_path.exists() {
        return Ok(());
    }
    let pending: PendingRestoreWire =
        serde_json::from_reader(File::open(&journal_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if !matches!(pending.phase.as_str(), "committed" | "rolled_back") {
        return Err("Restore journal is not ready for cleanup".into());
    }
    let transaction_id =
        Uuid::parse_str(&pending.transaction_id).map_err(|_| "Pending restore ID is invalid")?;
    let recovery_dir = data_dir
        .join(".backup-restore-recovery")
        .join(transaction_id.to_string());
    let stage_dir = data_dir
        .join(".backup-restore-stage")
        .join(transaction_id.to_string());
    for directory in [&stage_dir, &recovery_dir] {
        match fs::remove_dir_all(directory) {
            Ok(()) => {}
            // A previous cleanup may have stopped after removing either copy.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    fs::remove_file(journal_path).map_err(|e| e.to_string())?;
    sync_directory(data_dir).map_err(|e| e.to_string())
}

fn install_staged_pair(data_dir: &Path, pending: &PendingRestoreWire) -> Result<(), String> {
    let stage = PathBuf::from(&pending.stage_directory);
    let names = [
        "burrata.sqlite",
        "burrata.sqlite-wal",
        "burrata.sqlite-shm",
        "playback-history.sqlite",
        "playback-history.sqlite-wal",
        "playback-history.sqlite-shm",
    ];
    for name in names {
        let _ = fs::remove_file(data_dir.join(name));
    }
    for name in ["burrata.sqlite", "playback-history.sqlite"] {
        fs::rename(stage.join(name), data_dir.join(name)).map_err(|e| e.to_string())?;
    }
    sync_directory(data_dir).map_err(|e| e.to_string())?;
    validate_installed_pair(data_dir)
}

/// Copy and validate the old DB pair while the live files are still untouched.
/// The journal remains `prepared` until these copies are durable, so a crash
/// during this step simply retries the copy from the intact live profile.
fn prepare_recovery_pair(data_dir: &Path, pending: &PendingRestoreWire) -> Result<(), String> {
    let recovery = PathBuf::from(&pending.recovery_directory);
    fs::create_dir_all(&recovery).map_err(|e| e.to_string())?;
    for name in [
        "burrata.sqlite",
        "burrata.sqlite-wal",
        "burrata.sqlite-shm",
        "playback-history.sqlite",
        "playback-history.sqlite-wal",
        "playback-history.sqlite-shm",
    ] {
        let source = data_dir.join(name);
        let destination = recovery.join(name);
        if source.is_file() {
            fs::copy(&source, &destination).map_err(|e| e.to_string())?;
            // Windows requires a write-capable handle for FlushFileBuffers;
            // opening the freshly copied backup read-only reports ACCESS_DENIED.
            OpenOptions::new()
                .write(true)
                .open(&destination)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
        } else {
            let _ = fs::remove_file(destination);
        }
    }
    for (name, expected) in [
        ("burrata.sqlite", pending.had_library),
        ("playback-history.sqlite", pending.had_history),
    ] {
        if recovery.join(name).is_file() != expected {
            return Err("Live database set changed before restore could be applied".into());
        }
    }
    validate_available_pair(&recovery)?;
    sync_directory(&recovery).map_err(|e| e.to_string())
}

fn rollback_pair(data_dir: &Path, pending: &PendingRestoreWire) -> Result<(), String> {
    let recovery = PathBuf::from(&pending.recovery_directory);
    for (db, existed) in [
        ("burrata.sqlite", pending.had_library),
        ("playback-history.sqlite", pending.had_history),
    ] {
        if existed && !recovery.join(db).is_file() {
            return Err(format!("Recovery database {db} is missing"));
        }
        for suffix in ["", "-wal", "-shm"] {
            let name = format!("{db}{suffix}");
            let target = data_dir.join(&name);
            let backup = recovery.join(&name);
            match fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            if existed && backup.is_file() {
                fs::copy(&backup, &target).map_err(|e| e.to_string())?;
                // Match prepare_recovery_pair: FlushFileBuffers needs a
                // write-capable handle on Windows, without truncating data.
                OpenOptions::new()
                    .write(true)
                    .open(&target)
                    .and_then(|file| file.sync_all())
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    sync_directory(data_dir).map_err(|e| e.to_string())?;
    validate_available_pair(data_dir)
}

fn validate_available_pair(data_dir: &Path) -> Result<(), String> {
    for name in ["burrata.sqlite", "playback-history.sqlite"] {
        if !data_dir.join(name).exists() {
            continue;
        }
        let conn = rusqlite::Connection::open_with_flags(
            data_dir.join(name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
            .map_err(|e| e.to_string())?;
        let result: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if result != "ok" {
            return Err(format!("Restored database {name} failed integrity_check"));
        }
    }
    Ok(())
}

fn validate_installed_pair(data_dir: &Path) -> Result<(), String> {
    for name in ["burrata.sqlite", "playback-history.sqlite"] {
        let conn = rusqlite::Connection::open_with_flags(
            data_dir.join(name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
            .map_err(|e| e.to_string())?;
        let result: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if result != "ok" {
            return Err(format!("Restored database {name} failed integrity_check"));
        }
    }
    Ok(())
}

impl BackupManager {
    pub fn start_restore(
        self: &Arc<Self>,
        app: AppHandle,
        preview_id: String,
        request: RestoreRequest,
    ) -> Result<String, String> {
        if self.pending_exists() {
            return Err("A restore is already waiting for restart".into());
        }
        let preview = self
            .previews
            .lock()
            .map_err(|_| "Backup previews lock was poisoned")?
            .get(&preview_id)
            .map(|record| {
                (
                    record.preview.clone(),
                    record.destination_tokens.clone(),
                    record.archive.manifest.auxiliary_assets.clone(),
                )
            })
            .ok_or("Backup preview expired; inspect the archive again")?;
        let ids = request
            .component_ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        if ids.len() != request.component_ids.len()
            || ids.is_empty()
            || ids.iter().any(|id| {
                !preview
                    .0
                    .components
                    .iter()
                    .any(|component| &component.id == id)
            })
        {
            return Err("Restore component selection is invalid".into());
        }
        if ids.iter().any(|id| id.starts_with("media:")) && !ids.contains("library") {
            return Err("Select the library component to restore audio assets".into());
        }
        for attachment in &preview.2 {
            if ids.contains(&attachment.component_id)
                && (!ids.contains("library")
                    || !ids.contains(&format!("media:{}", attachment.asset_id)))
            {
                return Err("Select the companion audio file to restore its lyrics sidecar".into());
            }
        }
        if request.mode == RestoreMode::Replace
            && ["library", "history", "settings", "playback"]
                .iter()
                .any(|required| !ids.contains(*required))
        {
            return Err("Replace restore requires selecting all four data components".into());
        }
        let job_id = Uuid::new_v4().to_string();
        let tokens = request
            .path_mappings
            .iter()
            .map(|mapping| (&mapping.root_key, &mapping.destination_token))
            .collect::<HashMap<_, _>>();
        if request.path_mappings.len() != tokens.len() {
            return Err("Duplicate restore path mapping".into());
        }
        for (root, token) in &tokens {
            let Some(registered_root) = preview.1.get(*token) else {
                return Err("Restore destination token is invalid".into());
            };
            if !preview.0.media_roots.iter().any(|row| {
                &row.root_key == *root && row.destination_token.as_deref() == Some(token)
            }) {
                return Err("Restore destination token is invalid".into());
            }
            validate_restore_root(registered_root)?;
        }
        self.reserve_job(&job_id)?;
        let job = Arc::new(JobRecord {
            cancel: Arc::new(AtomicBool::new(false)),
            status: Mutex::new(BackupJobStatus::new(job_id.clone(), false)),
        });
        if let Err(_) = self
            .jobs
            .lock()
            .map(|mut jobs| jobs.insert(job_id.clone(), job.clone()))
        {
            self.release_job(&job_id);
            return Err("Backup jobs lock was poisoned".into());
        }
        self.trim_finished_jobs();
        let manager = self.clone();
        let task_id = job_id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            manager.run_restore(&app, &task_id, job, &preview_id, request)
        });
        Ok(job_id)
    }

    fn run_restore(
        &self,
        app: &AppHandle,
        job_id: &str,
        job: Arc<JobRecord>,
        preview_id: &str,
        request: RestoreRequest,
    ) {
        self.update(
            app,
            &job,
            "running",
            "Preparing isolated restore",
            Some(0),
            Some(5),
            None,
        );
        let result = self.prepare_restore(app, &job, preview_id, &request);
        match result {
            Ok(()) => self.update(
                app,
                &job,
                "complete",
                "Restore staged; restarting Burrata",
                Some(5),
                Some(5),
                None,
            ),
            Err(error) => self.update(
                app,
                &job,
                if job.cancel.load(Ordering::Relaxed) {
                    "canceled"
                } else {
                    "failed"
                },
                "Restore failed",
                None,
                None,
                Some(error),
            ),
        }
        if self.pending_exists() {
            app.restart();
        }
        self.release_job(job_id);
    }

    fn pending_exists(&self) -> bool {
        self.data_dir.join(PENDING_RESTORE_FILE).is_file()
    }

    fn prepare_restore(
        &self,
        app: &AppHandle,
        job: &JobRecord,
        preview_id: &str,
        request: &RestoreRequest,
    ) -> Result<(), String> {
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| "Backup previews lock was poisoned")?;
        let record = previews
            .get_mut(preview_id)
            .ok_or("Backup preview expired")?;
        let transaction_id = Uuid::new_v4();
        let snapshot_id = record.preview.snapshot_id.clone();
        let stage_dir = self
            .data_dir
            .join(".backup-restore-stage")
            .join(transaction_id.to_string());
        let recovery_dir = self
            .data_dir
            .join(".backup-restore-recovery")
            .join(transaction_id.to_string());
        fs::create_dir_all(&stage_dir).map_err(|e| e.to_string())?;
        let recovery_snapshot_id = Uuid::new_v4();
        let (state_options, native_checkpoint) =
            self.capture_backup_checkpoint(app, Some(&job.cancel), true, true)?;
        let current = self.prepare_coordinated_backup(
            recovery_snapshot_id,
            state_options,
            native_checkpoint.as_ref(),
        )?;
        let recovery_path = self
            .data_dir
            .join("backup-recovery")
            .join(format!("pre-restore-{}.burrata-backup", transaction_id));
        fs::create_dir_all(recovery_path.parent().unwrap()).map_err(|e| e.to_string())?;
        let recovery_components = vec![
            ArchiveInput {
                id: "library".into(),
                schema_version: sqlite_user_version(&current.library_sqlite)?,
                source_path: current.library_sqlite.clone(),
            },
            ArchiveInput {
                id: "history".into(),
                schema_version: 1,
                source_path: current.history_sqlite.clone(),
            },
            ArchiveInput {
                id: "settings".into(),
                schema_version: 1,
                source_path: current.settings_json.clone(),
            },
            ArchiveInput {
                id: "playback".into(),
                schema_version: 1,
                source_path: current.playback_json.clone(),
            },
        ];
        create_archive_with_auxiliary_managed(
            &recovery_components,
            &[],
            &[],
            BackupMediaSummary {
                included: false,
                assets: 0,
                missing: 0,
                changed: 0,
                omitted: self.store.list_assets()?.len() as u64,
            },
            &recovery_path,
            env!("CARGO_PKG_VERSION"),
            recovery_snapshot_id,
            &job.cancel,
            None,
            &self.staging,
            current._stage.path(),
        )
        .map_err(|e| e.to_string())?;
        // The recovery archive is complete. Exclude leased writers, freeze
        // both live connections, and reject any DB edits made since the
        // snapshot used to build that recovery archive.
        let restore_permit = self.store.try_restore_permit()?;
        let mut library_freeze = self
            .store
            .freeze_for_restore(&restore_permit, current.library_change_count)?;
        let mut history_freeze = self
            .history
            .freeze_for_restore(&restore_permit, current.history_change_count)?;
        self.update(
            app,
            job,
            "running",
            "Created and verified recovery snapshot",
            Some(1),
            Some(5),
            None,
        );
        if request.mode == RestoreMode::Merge {
            fs::copy(&current.library_sqlite, stage_dir.join("burrata.sqlite"))
                .map_err(|e| e.to_string())?;
            fs::copy(
                &current.history_sqlite,
                stage_dir.join("playback-history.sqlite"),
            )
            .map_err(|e| e.to_string())?;
        }
        let mut media_progress = |completed: usize, total: usize| {
            self.update(
                app,
                job,
                "running",
                "Copying verified audio files",
                Some(completed as u64),
                Some(total as u64),
                None,
            );
        };
        let (mappings, media_warnings) =
            copy_verified_media(record, request, &job.cancel, Some(&mut media_progress))?;
        if !media_warnings.is_empty() {
            self.update(
                app,
                job,
                "running",
                "Some audio files remain unavailable",
                Some(2),
                Some(5),
                Some(media_warnings.join(" ")),
            );
        } else {
            self.update(
                app,
                job,
                "running",
                "Restored verified audio files",
                Some(2),
                Some(5),
                None,
            );
        }
        let mut options =
            restore_options(&snapshot_id, request.mode, &request.component_ids, mappings);
        if request.mode == RestoreMode::Merge {
            options.current_asset_paths =
                backup_state::current_asset_paths_from_snapshot(&current.source_library_snapshot)?;
            options.current_music_root =
                backup_state::current_music_root_from_snapshot(&current.source_library_snapshot)?;
            if !options.include_playback {
                options.current_playback_snapshot = Some(current.playback_json.clone());
            }
        }
        let archive = &record.archive;
        // This check is intentionally adjacent to staged library reconstruction:
        // even a metadata-only mapping must still refer to the exact folder the
        // user selected when the preview token was issued.
        for mapping in &request.path_mappings {
            let registered = record
                .destination_tokens
                .get(&mapping.destination_token)
                .ok_or("Restore destination token is invalid")?;
            validate_restore_root(registered)?;
        }
        let summary = backup_state::reconstruct_staged(
            if ids_selected(&request.component_ids, "library") {
                archive.component_path("library")
            } else {
                None
            },
            if ids_selected(&request.component_ids, "history") {
                archive.component_path("history")
            } else {
                None
            },
            if ids_selected(&request.component_ids, "settings") {
                archive.component_path("settings")
            } else {
                None
            },
            if ids_selected(&request.component_ids, "playback") {
                archive.component_path("playback")
            } else {
                None
            },
            &stage_dir,
            &options,
        )?;
        let staged_database = stage_dir.join("burrata.sqlite");
        let staged_store = Store::open(&staged_database).map_err(|error| error.to_string())?;
        crate::restore_work_policy::pause_all_after_restore(&staged_store, &snapshot_id)?;
        preserve_current_machine_state(&self.store, &staged_store)?;
        let native_playback_engine = match std::env::var("BURRATA_PLAYBACK_ENGINE").ok() {
            Some(value) => value == "native",
            None => {
                staged_store
                    .get_json_preference("playbackEngine.v1")?
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    == Some("native")
            }
        };
        if options.include_playback && !native_playback_engine {
            // Only WebView mode needs a frontend one-time handoff. Native mode
            // already reads these validated staged database rows on startup.
            let queue = staged_store
                .get_json_preference("nativePlaybackQueue.v1")?
                .ok_or("Restored playback queue was not staged")?;
            let session = staged_store
                .get_json_preference("nativePlaybackSession.v1")?
                .ok_or("Restored playback session was not staged")?;
            let handoff = backup_state::make_backup_playback_handoff(
                &queue,
                &session,
                &Uuid::new_v4().to_string(),
            )?;
            staged_store
                .set_json_preference(backup_state::BACKUP_PLAYBACK_HANDOFF_KEY, &handoff)?;
        }
        drop(staged_store);
        checkpoint_restore_database(&staged_database)?;
        self.update(
            app,
            job,
            "running",
            "Validated reconstructed database pair",
            Some(2),
            Some(5),
            Some(format!(
                "{} recordings, {} history events",
                summary.imported_recordings, summary.imported_history_events
            )),
        );
        validate_installed_pair(&stage_dir)?;
        if job.cancel.load(Ordering::Relaxed) {
            return Err("Restore canceled".into());
        }
        let pending = PendingRestoreWire {
            transaction_id: transaction_id.to_string(),
            snapshot_id,
            stage_directory: stage_dir.to_string_lossy().into_owned(),
            recovery_directory: recovery_dir.to_string_lossy().into_owned(),
            recovery_archive: recovery_path.to_string_lossy().into_owned(),
            mode: request.mode,
            phase: "prepared".into(),
            had_library: self.data_dir.join("burrata.sqlite").exists(),
            had_history: self.data_dir.join("playback-history.sqlite").exists(),
        };
        self.update(
            app,
            job,
            "running",
            "Restore staged; restarting Burrata",
            Some(4),
            Some(5),
            None,
        );
        let mut freeze_slot = self
            .restore_freeze
            .lock()
            .map_err(|_| "Restore freeze slot was poisoned")?;
        if freeze_slot.is_some() {
            return Err("Another restore already owns the profile freeze".into());
        }
        atomic_json(&self.data_dir.join(PENDING_RESTORE_FILE), &pending)?;
        library_freeze.keep_frozen();
        history_freeze.keep_frozen();
        *freeze_slot = Some(RestoreFreeze {
            _permit: restore_permit,
            _library: library_freeze,
            _history: history_freeze,
        });
        Ok(())
    }
}

fn ids_selected(ids: &[String], id: &str) -> bool {
    ids.iter().any(|selected| selected == id)
}

fn validate_webview_playback_checkpoint(value: &Value) -> Result<(), String> {
    let queue = value
        .get("queue")
        .and_then(Value::as_object)
        .ok_or("WebView playback queue checkpoint is missing")?;
    let session = value
        .get("session")
        .and_then(Value::as_object)
        .ok_or("WebView playback session checkpoint is missing")?;
    let tracks = queue
        .get("tracks")
        .and_then(Value::as_array)
        .ok_or("WebView playback tracks are missing")?;
    let entry_ids = queue
        .get("entryIds")
        .and_then(Value::as_array)
        .ok_or("WebView playback occurrence IDs are missing")?;
    if tracks.len() > 10_000 || tracks.len() != entry_ids.len() {
        return Err("WebView playback queue is incomplete or exceeds the backup limit".into());
    }
    let ids = entry_ids
        .iter()
        .map(|value| value.as_str().ok_or("WebView occurrence ID is invalid"))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.iter().any(|id| id.is_empty() || id.len() > 128)
        || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
    {
        return Err("WebView playback occurrence IDs are invalid".into());
    }
    let index = session
        .get("index")
        .and_then(Value::as_i64)
        .ok_or("WebView playback cursor is missing")?;
    if (tracks.is_empty() && index != -1)
        || (!tracks.is_empty() && !(0..tracks.len() as i64).contains(&index))
        || !session
            .get("position")
            .and_then(Value::as_f64)
            .is_some_and(|position| position.is_finite() && position >= 0.0)
        || !session
            .get("volume")
            .and_then(Value::as_f64)
            .is_some_and(|volume| volume.is_finite() && (0.0..=1.0).contains(&volume))
        || session
            .get("queueRevision")
            .and_then(Value::as_u64)
            .is_none()
        || !session
            .get("repeat")
            .and_then(Value::as_str)
            .is_some_and(|mode| matches!(mode, "off" | "all" | "one"))
        || session.get("shuffle").and_then(Value::as_bool).is_none()
    {
        return Err("WebView playback session checkpoint is invalid".into());
    }
    Ok(())
}

fn validate_native_checkpoint_snapshot(
    library_snapshot: &Path,
    checkpoint: &PlaybackBackupCheckpoint,
) -> Result<(), String> {
    if checkpoint.engine != "native" {
        return Err("Native playback checkpoint has the wrong engine".into());
    }
    let connection = rusqlite::Connection::open_with_flags(
        library_snapshot,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
        .map_err(|error| error.to_string())?;
    for (key, expected) in [
        ("nativePlaybackQueue.v1", &checkpoint.queue),
        ("nativePlaybackSession.v1", &checkpoint.session),
    ] {
        let raw: Option<String> = connection
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(expected) = expected.as_ref() else {
            return Err("Native playback checkpoint is incomplete".into());
        };
        let actual = raw
            .map(|raw| serde_json::from_str::<Value>(&raw))
            .transpose()
            .map_err(|error| error.to_string())?;
        if actual.as_ref() != Some(expected) {
            return Err("Playback changed during backup capture; retry the backup".into());
        }
    }
    Ok(())
}

fn checkpoint_restore_database(path: &Path) -> Result<(), String> {
    let connection = rusqlite::Connection::open(path).map_err(|error| error.to_string())?;
    let (busy, _log_frames, _checkpointed_frames): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?;
    if busy != 0 {
        return Err("Could not checkpoint the staged replacement database".into());
    }
    drop(connection);
    Ok(())
}

/// Restore imports portable library state but keeps trusted machine-local
/// configuration from the profile that initiated the restore. Never copy
/// arbitrary preference rows: several of them contain credentials, tokens,
/// caches, or transient session state.
fn preserve_current_machine_state(current: &Store, staged: &Store) -> Result<(), String> {
    let current_settings = current.settings()?;
    let mut staged_settings = staged.settings()?;
    staged_settings.music_directory = current_settings.music_directory;
    staged_settings.yt_dlp_path = current_settings.yt_dlp_path;
    staged_settings.ffmpeg_path = current_settings.ffmpeg_path;
    staged_settings.ffprobe_path = current_settings.ffprobe_path;
    staged_settings.js_runtime_path = current_settings.js_runtime_path;
    staged_settings.playlist_directories = current_settings.playlist_directories;
    staged_settings.download_audio_verification_enabled =
        current_settings.download_audio_verification_enabled;
    staged_settings.playback_diagnostics_enabled = current_settings.playback_diagnostics_enabled;
    staged.save_settings(&staged_settings)?;

    if let Some(raw) = current.get_json_preference("connections")? {
        // Deserialize and reserialize the closed application type to discard
        // legacy/unknown fields that might contain a token or secret.
        let typed: crate::connections::ConnectionSettings = serde_json::from_value(raw)
            .map_err(|error| format!("Current connection settings are invalid: {error}"))?;
        let safe = serde_json::to_value(typed).map_err(|error| error.to_string())?;
        staged.set_json_preference("connections", &safe)?;
    }

    if let Some(raw) = current.get_json_preference("connections_session_only")? {
        let accepted = [
            "slskd",
            "discogs",
            "lastfm",
            "acoustid",
            "lyrics_translation",
        ];
        let safe = raw
            .as_array()
            .map(|values| {
                serde_json::Value::Array(
                    values
                        .iter()
                        .filter_map(|value| {
                            let provider = value.as_str()?;
                            accepted
                                .contains(&provider)
                                .then(|| serde_json::Value::String(provider.to_owned()))
                        })
                        .collect(),
                )
            })
            .unwrap_or_else(|| serde_json::json!([]));
        staged.set_json_preference("connections_session_only", &safe)?;
    }

    if let Some(raw) = current.get_json_preference("playbackEngine.v1")? {
        if matches!(raw.as_str(), Some("native" | "webview")) {
            staged.set_json_preference("playbackEngine.v1", &raw)?;
        }
    }
    if let Some(raw) = current.get_json_preference("directoryBrowserRoots.v1")? {
        let safe = sanitize_current_directory_roots(raw);
        staged.set_json_preference("directoryBrowserRoots.v1", &safe)?;
    }
    if let Some(raw) = current.get_json_preference("automation.permissions.v1")? {
        if let Some(safe) = sanitize_current_automation_permissions(&raw) {
            staged.set_json_preference("automation.permissions.v1", &safe)?;
        }
    }
    for key in ["releaseSetupComplete.v1", "radioRecognitionEnabled.v1"] {
        if let Some(raw) = current.get_json_preference(key)? {
            if let Some(value) = raw.as_bool() {
                staged.set_json_preference(key, &serde_json::Value::Bool(value))?;
            }
        }
    }
    Ok(())
}

fn sanitize_current_directory_roots(value: serde_json::Value) -> serde_json::Value {
    const MAX_ROOTS: usize = 32;
    let roots = value
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ROOTS)
        .filter_map(|root| {
            let object = root.as_object()?;
            let id = object.get("id")?.as_str()?;
            let name = object.get("name")?.as_str()?;
            let path = object.get("path")?.as_str()?;
            if id.is_empty()
                || id.len() > 128
                || !id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-_".contains(ch))
                || name.trim().is_empty()
                || name.len() > 256
                || path.is_empty()
                || path.len() > 4096
                || !Path::new(path).is_absolute()
            {
                return None;
            }
            Some(serde_json::json!({"id":id,"name":name,"path":path}))
        })
        .collect::<Vec<_>>();
    serde_json::Value::Array(roots)
}

fn sanitize_current_automation_permissions(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    Some(serde_json::json!({
        "read": object.get("read")?.as_bool()?,
        "control": object.get("control")?.as_bool()?,
        "manage": object.get("manage")?.as_bool()?
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup_archive::{inspect_archive, ArchiveInput};
    use rusqlite::Connection;

    fn media_restore_record(root: &Path, media_bytes: &[u8]) -> PreviewRecord {
        media_restore_record_mode(root, media_bytes, true)
    }

    fn media_restore_record_mode(
        root: &Path,
        media_bytes: &[u8],
        include_media: bool,
    ) -> PreviewRecord {
        let inputs_dir = root.join("archive-inputs");
        fs::create_dir_all(&inputs_dir).unwrap();
        let mut inputs = Vec::new();
        for (id, filename) in [
            ("library", "library.sqlite"),
            ("history", "history.sqlite"),
            ("playback", "playback.json"),
        ] {
            let path = inputs_dir.join(filename);
            if id == "library" || id == "history" {
                let connection = Connection::open(&path).unwrap();
                if id == "library" {
                    connection
                        .execute_batch(
                            "CREATE TABLE local_assets(id TEXT PRIMARY KEY, hash TEXT NOT NULL);",
                        )
                        .unwrap();
                } else {
                    connection
                        .execute_batch("CREATE TABLE fixture(value TEXT);")
                        .unwrap();
                }
            } else {
                fs::write(&path, b"{}").unwrap();
            }
            inputs.push(ArchiveInput {
                id: id.into(),
                schema_version: 1,
                source_path: path,
            });
        }
        let asset_id = "asset-test";
        let media_path = inputs_dir.join("asset.wav");
        fs::write(&media_path, media_bytes).unwrap();
        let hash = format!("{:x}", sha2::Sha256::digest(media_bytes));
        Connection::open(inputs_dir.join("library.sqlite"))
            .unwrap()
            .execute(
                "INSERT INTO local_assets(id,hash) VALUES(?1,?2)",
                rusqlite::params![asset_id, hash],
            )
            .unwrap();
        let settings_path = inputs_dir.join("settings.json");
        let settings = backup_state::SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: 1,
            snapshot_id: Uuid::new_v4().to_string(),
            created_at: "2026-10-27T00:00:00Z".into(),
            application: serde_json::json!({}),
            preferences: Default::default(),
            frontend_preferences: Default::default(),
            asset_path_hints: vec![backup_state::AssetPathHint {
                asset_id: asset_id.into(),
                root_key: "music".into(),
                relative_path: "Artist/Track.wav".into(),
            }],
            projection_summary: Default::default(),
        };
        serde_json::to_writer(File::create(&settings_path).unwrap(), &settings).unwrap();
        inputs.push(ArchiveInput {
            id: "settings".into(),
            schema_version: 1,
            source_path: settings_path,
        });
        let archive_path = root.join("fixture.burrata-backup");
        let cancel = AtomicBool::new(false);
        let media_inputs_owned = [MediaInput {
            asset_id: asset_id.into(),
            source_path: media_path,
            expected_sha256: Some(hash),
        }];
        let media_inputs: &[MediaInput] = if include_media {
            &media_inputs_owned
        } else {
            &[]
        };
        create_archive(
            &inputs,
            media_inputs,
            BackupMediaSummary {
                included: include_media,
                assets: if include_media { 1 } else { 0 },
                missing: 0,
                changed: 0,
                omitted: if include_media { 0 } else { 1 },
            },
            &archive_path,
            "test",
            Uuid::new_v4(),
            &cancel,
            None,
        )
        .unwrap();
        let archive = inspect_archive(&archive_path, &cancel, None).unwrap();
        let inspection = archive.inspection.clone();
        let scratch_root = root.join("profile");
        let staging = BackupStaging::open(&scratch_root).unwrap();
        let scratch = staging.create_profile_stage("media-copy-test").unwrap();
        let destination = root.join("destination");
        fs::create_dir_all(&destination).unwrap();
        let preview_id = Uuid::new_v4().to_string();
        let token = "test-destination-token".to_owned();
        let preview = BackupPreview {
            preview_id: preview_id.clone(),
            snapshot_id: inspection.snapshot_id.clone(),
            schema_version: inspection.schema_version,
            created_at: inspection.created_at.clone(),
            components: inspection.components.clone(),
            media: inspection.media.clone(),
            archive_bytes: inspection.archive_bytes,
            estimated_bytes: 0,
            warnings: Vec::new(),
            summary: RestoreStateSummary::default(),
            media_roots: vec![BackupMediaRootPreview {
                root_key: "music".into(),
                label: "Music folder".into(),
                assets: 1,
                required_bytes: if include_media {
                    media_bytes.len() as u64
                } else {
                    0
                },
                destination_token: Some(token.clone()),
                destination_label: Some("destination".into()),
                reusable_same_hash: 0,
                copy_count: if include_media { 1 } else { 0 },
                conflicts: 0,
                missing_destination: false,
                available_bytes: None,
                lyrics_sidecars: 0,
                lyrics_copy_count: 0,
                lyrics_reusable_same_hash: 0,
                lyrics_conflicts: 0,
            }],
            settings_projection: Default::default(),
        };
        let mut destination_tokens = HashMap::new();
        let canonical_destination = fs::canonicalize(&destination).unwrap();
        destination_tokens.insert(token, seal_restore_root(&canonical_destination).unwrap());
        PreviewRecord {
            archive,
            _scratch: scratch,
            preview,
            destination_tokens,
        }
    }

    fn media_restore_record_with_sidecars(root: &Path) -> PreviewRecord {
        let inputs_dir = root.join("sidecar-archive-inputs");
        fs::create_dir_all(&inputs_dir).unwrap();
        let mut inputs = Vec::new();
        for (id, filename) in [("library", "library.sqlite"), ("history", "history.sqlite")] {
            let path = inputs_dir.join(filename);
            let connection = Connection::open(&path).unwrap();
            if id == "library" {
                connection
                    .execute_batch(
                        "CREATE TABLE local_assets(id TEXT PRIMARY KEY, hash TEXT NOT NULL);",
                    )
                    .unwrap();
            } else {
                connection
                    .execute_batch("CREATE TABLE fixture(value TEXT);")
                    .unwrap();
            }
            inputs.push(ArchiveInput {
                id: id.into(),
                schema_version: 1,
                source_path: path,
            });
        }
        let playback_path = inputs_dir.join("playback.json");
        fs::write(&playback_path, b"{}").unwrap();
        inputs.push(ArchiveInput {
            id: "playback".into(),
            schema_version: 1,
            source_path: playback_path,
        });

        let asset_specs = [
            (
                "asset-match",
                "Artist/Match.wav",
                b"matching audio".as_slice(),
            ),
            (
                "asset-conflict",
                "Artist/Conflict.wav",
                b"conflicting audio".as_slice(),
            ),
        ];
        let mut asset_path_hints = Vec::new();
        let mut media_inputs = Vec::new();
        let mut auxiliary_inputs = Vec::new();
        for (asset_id, relative_path, audio_bytes) in asset_specs {
            let media_path = inputs_dir.join(format!("{asset_id}.wav"));
            fs::write(&media_path, audio_bytes).unwrap();
            let audio_hash = format!("{:x}", sha2::Sha256::digest(audio_bytes));
            Connection::open(inputs_dir.join("library.sqlite"))
                .unwrap()
                .execute(
                    "INSERT INTO local_assets(id,hash) VALUES(?1,?2)",
                    rusqlite::params![asset_id, audio_hash],
                )
                .unwrap();
            media_inputs.push(MediaInput {
                asset_id: asset_id.into(),
                source_path: media_path,
                expected_sha256: Some(audio_hash.clone()),
            });
            asset_path_hints.push(backup_state::AssetPathHint {
                asset_id: asset_id.into(),
                root_key: "music".into(),
                relative_path: relative_path.into(),
            });

            let lrc_path = inputs_dir.join(format!("{asset_id}.lrc"));
            let lrc_bytes = format!("[00:01.00] lyrics for {asset_id}\n").into_bytes();
            fs::write(&lrc_path, &lrc_bytes).unwrap();
            auxiliary_inputs.push(crate::backup_auxiliary::AuxiliaryInput {
                attachment: crate::backup_auxiliary::AuxiliaryAttachment {
                    schema_version: 1,
                    kind: crate::backup_auxiliary::AuxiliaryAssetKind::LyricsLrc,
                    asset_id: asset_id.into(),
                    audio_sha256: audio_hash,
                    component_id: crate::backup_auxiliary::auxiliary_component_id(asset_id),
                },
                source_path: lrc_path,
                expected_sha256: format!("{:x}", sha2::Sha256::digest(&lrc_bytes)),
            });
        }

        let snapshot_id = Uuid::new_v4();
        let settings_path = inputs_dir.join("settings.json");
        let settings = backup_state::SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: 1,
            snapshot_id: snapshot_id.to_string(),
            created_at: "2026-10-27T00:00:00Z".into(),
            application: serde_json::json!({}),
            preferences: Default::default(),
            frontend_preferences: Default::default(),
            asset_path_hints,
            projection_summary: Default::default(),
        };
        serde_json::to_writer(File::create(&settings_path).unwrap(), &settings).unwrap();
        inputs.push(ArchiveInput {
            id: "settings".into(),
            schema_version: 1,
            source_path: settings_path,
        });

        let archive_path = root.join("sidecar-fixture.burrata-backup");
        let canceled = AtomicBool::new(false);
        crate::backup_archive::create_archive_with_auxiliary(
            &inputs,
            &media_inputs,
            &auxiliary_inputs,
            BackupMediaSummary {
                included: true,
                assets: media_inputs.len() as u64,
                missing: 0,
                changed: 0,
                omitted: 0,
            },
            &archive_path,
            "test",
            snapshot_id,
            &canceled,
            None,
        )
        .unwrap();
        let archive = inspect_archive(&archive_path, &canceled, None).unwrap();
        let inspection = archive.inspection.clone();
        let scratch_root = root.join("sidecar-profile");
        let staging = BackupStaging::open(&scratch_root).unwrap();
        let scratch = staging
            .create_profile_stage("media-copy-sidecar-test")
            .unwrap();
        let destination = root.join("sidecar-destination");
        fs::create_dir_all(&destination).unwrap();
        let token = "sidecar-destination-token".to_owned();
        let preview_id = Uuid::new_v4().to_string();
        let preview = BackupPreview {
            preview_id: preview_id.clone(),
            snapshot_id: inspection.snapshot_id.clone(),
            schema_version: inspection.schema_version,
            created_at: inspection.created_at.clone(),
            components: inspection.components.clone(),
            media: inspection.media.clone(),
            archive_bytes: inspection.archive_bytes,
            estimated_bytes: 0,
            warnings: Vec::new(),
            summary: RestoreStateSummary::default(),
            media_roots: vec![BackupMediaRootPreview {
                root_key: "music".into(),
                label: "Music folder".into(),
                assets: 2,
                required_bytes: 0,
                destination_token: Some(token.clone()),
                destination_label: Some("sidecar-destination".into()),
                reusable_same_hash: 0,
                copy_count: 2,
                conflicts: 0,
                missing_destination: false,
                available_bytes: None,
                lyrics_sidecars: 2,
                lyrics_copy_count: 2,
                lyrics_reusable_same_hash: 0,
                lyrics_conflicts: 0,
            }],
            settings_projection: Default::default(),
        };
        PreviewRecord {
            archive,
            _scratch: scratch,
            preview,
            destination_tokens: HashMap::from([(
                token,
                seal_restore_root(&fs::canonicalize(&destination).unwrap()).unwrap(),
            )]),
        }
    }

    fn media_restore_request() -> RestoreRequest {
        RestoreRequest {
            mode: RestoreMode::Merge,
            component_ids: vec!["library".into(), "media:asset-test".into()],
            path_mappings: vec![RestorePathToken {
                root_key: "music".into(),
                destination_token: "test-destination-token".into(),
            }],
        }
    }

    fn write_db(directory: &Path, file_name: &str, value: &str) {
        let connection = Connection::open(directory.join(file_name)).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE marker(value TEXT NOT NULL);
                 INSERT INTO marker(value) VALUES('placeholder');",
            )
            .unwrap();
        connection
            .execute("UPDATE marker SET value=?1", [value])
            .unwrap();
    }

    fn read_marker(directory: &Path, file_name: &str) -> String {
        Connection::open(directory.join(file_name))
            .unwrap()
            .query_row("SELECT value FROM marker", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn verified_media_copy_maps_and_publishes_only_archive_bytes() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let record = media_restore_record(root.path(), bytes);
        let cancel = AtomicBool::new(false);
        let (mappings, warnings) =
            copy_verified_media(&record, &media_restore_request(), &cancel, None).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].root_key, "music");
        let destination = root.path().join("destination/Artist/Track.wav");
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(
            media::hash_file(&destination).unwrap(),
            format!("{:x}", sha2::Sha256::digest(bytes))
        );
    }

    #[test]
    fn metadata_only_archive_offers_verified_existing_audio_without_copying() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let record = media_restore_record_mode(root.path(), bytes, false);
        let roots = media_root_previews(&record.archive).unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].root_key, "music");
        assert_eq!(roots[0].assets, 1);
        assert_eq!(roots[0].required_bytes, 0);

        let destination = root.path().join("destination");
        let audio = destination.join("Artist/Track.wav");
        fs::create_dir_all(audio.parent().unwrap()).unwrap();
        fs::write(&audio, bytes).unwrap();
        let before = media::hash_file(&audio).unwrap();
        let mut preview = roots[0].clone();
        analyze_restore_root(&record, &mut preview, &destination).unwrap();
        assert_eq!(preview.reusable_same_hash, 1);
        assert_eq!(preview.copy_count, 0);
        assert_eq!(preview.conflicts, 0);
        assert_eq!(media::hash_file(&audio).unwrap(), before);

        let cancel = AtomicBool::new(false);
        let request = RestoreRequest {
            mode: RestoreMode::Merge,
            component_ids: vec!["library".into()],
            path_mappings: vec![RestorePathToken {
                root_key: "music".into(),
                destination_token: "test-destination-token".into(),
            }],
        };
        let (mappings, warnings) = copy_verified_media(&record, &request, &cancel, None).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].root_key, "music");
        assert_eq!(fs::read(&audio).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn registering_read_only_root_allows_metadata_only_existing_audio_link() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let record = media_restore_record_mode(root.path(), bytes, false);
        let preview_id = record.preview.preview_id.clone();
        let destination = root.path().join("destination");
        let audio = destination.join("Artist/Track.wav");
        fs::create_dir_all(audio.parent().unwrap()).unwrap();
        fs::write(&audio, bytes).unwrap();
        let original_permissions = fs::metadata(&destination).unwrap().permissions();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o555)).unwrap();

        let manager = create_manager(root.path());
        manager
            .previews
            .lock()
            .unwrap()
            .insert(preview_id.clone(), record);
        let registered = manager
            .register_restore_root(&preview_id, "music", &destination)
            .expect("read-only root is valid when its matching asset can be linked");

        assert_eq!(registered.reusable_same_hash, 1);
        assert_eq!(registered.copy_count, 0);
        assert_eq!(registered.conflicts, 0);
        assert_eq!(fs::read(&audio).unwrap(), bytes);
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);

        fs::set_permissions(&destination, original_permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verified_copy_still_requires_writable_destination_parent() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let record = media_restore_record(root.path(), bytes);
        let destination = root.path().join("destination");
        let artist = destination.join("Artist");
        fs::create_dir_all(&artist).unwrap();
        let original_destination_permissions = fs::metadata(&destination).unwrap().permissions();
        let original_artist_permissions = fs::metadata(&artist).unwrap().permissions();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&artist, fs::Permissions::from_mode(0o555)).unwrap();

        let cancel = AtomicBool::new(false);
        let error = copy_verified_media(&record, &media_restore_request(), &cancel, None)
            .expect_err("a new payload cannot be restored into a read-only folder");
        assert!(!error.is_empty());
        assert!(!destination.join("Artist/Track.wav").exists());

        fs::set_permissions(&artist, original_artist_permissions).unwrap();
        fs::set_permissions(&destination, original_destination_permissions).unwrap();
    }

    #[test]
    fn metadata_only_audio_link_preview_fails_closed_for_mismatch_missing_and_outside_symlink() {
        let bytes = b"verified synthetic audio payload";

        let mismatch_root = tempfile::tempdir().unwrap();
        let record = media_restore_record_mode(mismatch_root.path(), bytes, false);
        let mismatch_destination = mismatch_root.path().join("destination");
        let mismatch_audio = mismatch_destination.join("Artist/Track.wav");
        fs::create_dir_all(mismatch_audio.parent().unwrap()).unwrap();
        fs::write(&mismatch_audio, b"different user audio").unwrap();
        let mismatch_before = fs::read(&mismatch_audio).unwrap();
        let mut preview = media_root_previews(&record.archive).unwrap().remove(0);
        analyze_restore_root(&record, &mut preview, &mismatch_destination).unwrap();
        assert_eq!(preview.reusable_same_hash, 0);
        assert_eq!(preview.copy_count, 0);
        assert_eq!(preview.conflicts, 1);
        assert_eq!(fs::read(&mismatch_audio).unwrap(), mismatch_before);

        let missing_root = tempfile::tempdir().unwrap();
        let missing_record = media_restore_record_mode(missing_root.path(), bytes, false);
        let missing_destination = missing_root.path().join("destination");
        fs::create_dir_all(&missing_destination).unwrap();
        let mut preview = media_root_previews(&missing_record.archive)
            .unwrap()
            .remove(0);
        analyze_restore_root(&missing_record, &mut preview, &missing_destination).unwrap();
        assert_eq!(preview.reusable_same_hash, 0);
        assert_eq!(preview.copy_count, 0);
        assert_eq!(preview.conflicts, 0);

        #[cfg(unix)]
        {
            let symlink_root = tempfile::tempdir().unwrap();
            let symlink_record = media_restore_record_mode(symlink_root.path(), bytes, false);
            let symlink_destination = symlink_root.path().join("destination");
            let outside = symlink_root.path().join("outside.wav");
            fs::write(&outside, bytes).unwrap();
            let symlink_audio = symlink_destination.join("Artist/Track.wav");
            fs::create_dir_all(symlink_audio.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&outside, &symlink_audio).unwrap();
            let mut preview = media_root_previews(&symlink_record.archive)
                .unwrap()
                .remove(0);
            analyze_restore_root(&symlink_record, &mut preview, &symlink_destination).unwrap();
            assert_eq!(preview.reusable_same_hash, 0);
            assert_eq!(preview.copy_count, 0);
            assert_eq!(preview.conflicts, 1);
            assert_eq!(fs::read(outside).unwrap(), bytes);
        }
    }

    #[test]
    fn archive_media_component_hash_cannot_override_library_hash_for_copy() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let mut record = media_restore_record(root.path(), bytes);
        let component = record
            .archive
            .manifest
            .components
            .iter_mut()
            .find(|component| component.id == "media:asset-test")
            .unwrap();
        component.sha256 = "0".repeat(64);
        let cancel = AtomicBool::new(false);
        let (mappings, warnings) =
            copy_verified_media(&record, &media_restore_request(), &cancel, None).unwrap();
        assert_eq!(mappings.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("does not match its library hash"));
        assert!(!root.path().join("destination/Artist/Track.wav").exists());
    }

    #[test]
    fn replacing_a_registered_restore_root_invalidates_its_token_before_copy() {
        let bytes = b"verified synthetic audio payload";

        let replaced_root = tempfile::tempdir().unwrap();
        let record = media_restore_record(replaced_root.path(), bytes);
        let destination = replaced_root.path().join("destination");
        let renamed = replaced_root.path().join("destination-original");
        fs::rename(&destination, &renamed).unwrap();
        fs::create_dir(&destination).unwrap();
        let cancel = AtomicBool::new(false);
        let error =
            copy_verified_media(&record, &media_restore_request(), &cancel, None).unwrap_err();
        assert!(error.contains("folder changed"));
        assert!(!destination.join("Artist/Track.wav").exists());
        assert!(!renamed.join("Artist/Track.wav").exists());

        #[cfg(unix)]
        {
            let symlink_root = tempfile::tempdir().unwrap();
            let symlink_record = media_restore_record(symlink_root.path(), bytes);
            let destination = symlink_root.path().join("destination");
            let renamed = symlink_root.path().join("destination-original");
            let outside = symlink_root.path().join("outside");
            fs::create_dir(&outside).unwrap();
            fs::rename(&destination, &renamed).unwrap();
            std::os::unix::fs::symlink(&outside, &destination).unwrap();
            let error =
                copy_verified_media(&symlink_record, &media_restore_request(), &cancel, None)
                    .unwrap_err();
            assert!(error.contains("folder changed"));
            assert!(!outside.join("Artist/Track.wav").exists());
        }
    }

    #[test]
    fn verified_media_copy_reuses_matching_file_and_preserves_conflicting_file() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let record = media_restore_record(root.path(), bytes);
        let destination = root.path().join("destination/Artist/Track.wav");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, bytes).unwrap();
        let cancel = AtomicBool::new(false);
        let (_, warnings) =
            copy_verified_media(&record, &media_restore_request(), &cancel, None).unwrap();
        assert!(warnings.is_empty(), "identical media should be reusable");
        assert_eq!(fs::read(&destination).unwrap(), bytes);

        let conflict = b"user-owned destination content";
        fs::write(&destination, conflict).unwrap();
        let (_, warnings) =
            copy_verified_media(&record, &media_restore_request(), &cancel, None).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), conflict);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("was not overwritten"));
    }

    #[test]
    fn restore_preview_counts_sidecars_only_when_their_audio_can_be_installed() {
        let root = tempfile::tempdir().unwrap();
        let record = media_restore_record_with_sidecars(root.path());
        let destination = root.path().join("sidecar-destination");
        let matching_audio = b"matching audio";
        let conflicting_audio = b"preexisting user-owned audio";
        fs::create_dir_all(destination.join("Artist")).unwrap();
        fs::write(destination.join("Artist/Match.wav"), matching_audio).unwrap();
        fs::write(destination.join("Artist/Conflict.wav"), conflicting_audio).unwrap();

        let mut root_preview = record.preview.media_roots[0].clone();
        root_preview.reset_destination_counts();
        analyze_restore_root(&record, &mut root_preview, &destination).unwrap();
        assert_eq!(root_preview.reusable_same_hash, 1);
        assert_eq!(root_preview.copy_count, 0);
        assert_eq!(root_preview.conflicts, 1);
        assert_eq!(root_preview.lyrics_copy_count, 1);
        assert_eq!(root_preview.lyrics_reusable_same_hash, 0);
        assert_eq!(root_preview.lyrics_conflicts, 1);

        let mut component_ids = vec!["library".to_owned()];
        for asset_id in ["asset-match", "asset-conflict"] {
            component_ids.push(format!("media:{asset_id}"));
            component_ids.push(crate::backup_auxiliary::auxiliary_component_id(asset_id));
        }
        let request = RestoreRequest {
            mode: RestoreMode::Merge,
            component_ids,
            path_mappings: vec![RestorePathToken {
                root_key: "music".into(),
                destination_token: "sidecar-destination-token".into(),
            }],
        };
        let cancel = AtomicBool::new(false);
        let (_, warnings) = copy_verified_media(&record, &request, &cancel, None).unwrap();
        assert_eq!(
            fs::read(destination.join("Artist/Match.lrc")).unwrap(),
            b"[00:01.00] lyrics for asset-match\n"
        );
        assert!(!destination.join("Artist/Conflict.lrc").exists());
        assert_eq!(
            fs::read(destination.join("Artist/Conflict.wav")).unwrap(),
            conflicting_audio
        );
        assert!(warnings.iter().any(|warning| {
            warning.contains("Lyrics sidecar for audio asset asset-conflict")
                && warning.contains("audio file was not installed or verified")
        }));
    }

    #[test]
    fn verified_media_copy_rejects_changed_archive_bytes_and_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified synthetic audio payload";
        let record = media_restore_record(root.path(), bytes);
        let component_path = record.archive.component_path("media:asset-test").unwrap();
        fs::write(component_path, b"changed after archive validation").unwrap();
        let cancel = AtomicBool::new(false);
        let error =
            copy_verified_media(&record, &media_restore_request(), &cancel, None).unwrap_err();
        assert!(error.contains("Audio bytes changed after archive validation"));
        assert!(!root.path().join("destination/Artist/Track.wav").exists());

        let canceled_root = tempfile::tempdir().unwrap();
        let canceled_record = media_restore_record(canceled_root.path(), bytes);
        let canceled = AtomicBool::new(true);
        let error =
            copy_verified_media(&canceled_record, &media_restore_request(), &canceled, None)
                .unwrap_err();
        assert_eq!(error, "Restore canceled");
        assert!(!canceled_root
            .path()
            .join("destination/Artist/Track.wav")
            .exists());
    }

    #[test]
    fn verified_media_copy_without_mapping_leaves_asset_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let record = media_restore_record(root.path(), b"verified synthetic audio payload");
        let cancel = AtomicBool::new(false);
        let mut request = media_restore_request();
        request.path_mappings.clear();
        let (mappings, warnings) = copy_verified_media(&record, &request, &cancel, None).unwrap();
        assert!(mappings.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Choose a destination for music"));
        assert!(!root.path().join("destination/Artist/Track.wav").exists());
    }

    fn recovery_archive(data_dir: &Path, transaction_id: Uuid) -> PathBuf {
        let sources = tempfile::tempdir_in(data_dir).unwrap();
        let components = [
            ("library", "library.sqlite"),
            ("history", "history.sqlite"),
            ("settings", "settings.json"),
            ("playback", "playback.json"),
        ]
        .into_iter()
        .map(|(id, name)| {
            let source_path = sources.path().join(name);
            fs::write(&source_path, format!("test {id} component")).unwrap();
            ArchiveInput {
                id: id.into(),
                schema_version: 1,
                source_path,
            }
        })
        .collect::<Vec<_>>();
        let destination = data_dir
            .join("backup-recovery")
            .join(format!("pre-restore-{transaction_id}.burrata-backup"));
        let cancellation = AtomicBool::new(false);
        create_archive(
            &components,
            &[],
            BackupMediaSummary {
                included: false,
                assets: 0,
                missing: 0,
                changed: 0,
                omitted: 0,
            },
            &destination,
            "test",
            Uuid::new_v4(),
            &cancellation,
            None,
        )
        .unwrap();
        destination
    }

    fn write_pending(data_dir: &Path, transaction_id: Uuid, phase: &str) -> PendingRestoreWire {
        let stage = data_dir
            .join(".backup-restore-stage")
            .join(transaction_id.to_string());
        let recovery = data_dir
            .join(".backup-restore-recovery")
            .join(transaction_id.to_string());
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&recovery).unwrap();
        let recovery_archive = recovery_archive(data_dir, transaction_id);
        let pending = PendingRestoreWire {
            transaction_id: transaction_id.to_string(),
            snapshot_id: Uuid::new_v4().to_string(),
            stage_directory: stage.to_string_lossy().into_owned(),
            recovery_directory: recovery.to_string_lossy().into_owned(),
            recovery_archive: recovery_archive.to_string_lossy().into_owned(),
            mode: RestoreMode::Replace,
            phase: phase.into(),
            had_library: true,
            had_history: true,
        };
        atomic_json(&data_dir.join(PENDING_RESTORE_FILE), &pending).unwrap();
        pending
    }

    fn create_manager(data_dir: &Path) -> BackupManager {
        let store = Arc::new(Store::open(&data_dir.join("manager.sqlite")).unwrap());
        let history = Arc::new(
            PlaybackHistory::open_with_barrier(
                &data_dir.join("manager-history.sqlite"),
                store.library_write_barrier(),
            )
            .unwrap(),
        );
        BackupManager::new(data_dir.to_path_buf(), store, history)
    }

    #[test]
    fn active_job_snapshot_reports_automatic_kind_and_only_while_reserved() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());

        for (job_id, automatic) in [("automatic-job", true), ("manual-job", false)] {
            manager.reserve_job(job_id).unwrap();
            let record = Arc::new(JobRecord {
                cancel: Arc::new(AtomicBool::new(false)),
                status: Mutex::new(BackupJobStatus::new(job_id.into(), automatic)),
            });
            manager.jobs.lock().unwrap().insert(job_id.into(), record);

            let status = manager.active_job_status().unwrap().unwrap();
            assert_eq!(status.job_id, job_id);
            assert_eq!(status.automatic, automatic);

            {
                let jobs = manager.jobs.lock().unwrap();
                let record = jobs.get(job_id).unwrap();
                record.status.lock().unwrap().state = "complete".into();
            }
            manager.release_job(job_id);
            assert!(manager.active_job_status().unwrap().is_none());
        }
    }

    #[test]
    fn legacy_backup_job_status_defaults_to_manual() {
        let status: BackupJobStatus = serde_json::from_value(serde_json::json!({
            "jobId": "legacy-job",
            "state": "complete",
            "phase": null,
            "completed": 1,
            "total": 1,
            "message": null
        }))
        .unwrap();
        assert!(!status.automatic);
    }

    #[test]
    fn frontend_checkpoint_channel_accepts_only_fresh_bounded_nonce_responses() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let (sender, receiver) = mpsc::sync_channel(1);
        manager
            .frontend_checkpoints
            .lock()
            .unwrap()
            .insert("fresh-request".into(), sender);
        let response = FrontendCheckpointResponse {
            request_id: "fresh-request".into(),
            frontend_preferences: serde_json::json!({}),
            webview_playback: None,
        };
        manager
            .submit_frontend_checkpoint(response.clone())
            .unwrap();
        assert_eq!(receiver.recv().unwrap().request_id, "fresh-request");
        assert!(manager.submit_frontend_checkpoint(response).is_err());

        let (sender, _receiver) = mpsc::sync_channel(1);
        manager
            .frontend_checkpoints
            .lock()
            .unwrap()
            .insert("large-request".into(), sender);
        let too_large = FrontendCheckpointResponse {
            request_id: "large-request".into(),
            frontend_preferences: serde_json::json!({"oversized":"x".repeat(MAX_FRONTEND_CHECKPOINT_BYTES)}),
            webview_playback: None,
        };
        assert!(manager.submit_frontend_checkpoint(too_large).is_err());
        assert!(manager
            .frontend_checkpoints
            .lock()
            .unwrap()
            .contains_key("large-request"));
    }

    #[test]
    fn webview_checkpoint_requires_complete_occurrence_queue_and_session() {
        let track = serde_json::json!({"id":"local:one","provider":"local","providerId":"one","title":"One"});
        let valid = serde_json::json!({
            "queue":{"tracks":[track.clone(),track],"entryIds":["occurrence-a","occurrence-b"],"currentIndex":1,"repeat":"all"},
            "session":{"index":1,"position":4.5,"repeat":"all","shuffle":false,"volume":0.0,"queueRevision":12}
        });
        validate_webview_playback_checkpoint(&valid).unwrap();
        let missing_duplicate_identity = serde_json::json!({
            "queue":{"tracks":[{"id":"local:one"},{"id":"local:one"}],"entryIds":["same","same"],"currentIndex":1},
            "session":{"index":1,"position":0.0,"repeat":"off","shuffle":false,"volume":0.0,"queueRevision":1}
        });
        assert!(validate_webview_playback_checkpoint(&missing_duplicate_identity).is_err());
        let no_occurrences = serde_json::json!({
            "queue":{"tracks":[{"id":"local:one"}],"entryIds":[],"currentIndex":0},
            "session":{"index":0,"position":0.0,"repeat":"off","shuffle":false,"volume":0.0,"queueRevision":1}
        });
        assert!(validate_webview_playback_checkpoint(&no_occurrences).is_err());
    }

    #[test]
    fn replace_preserves_current_machine_state_and_uses_portable_archive_data() {
        let archive_dir = tempfile::tempdir().unwrap();
        let archive_store = Store::open(&archive_dir.path().join("burrata.sqlite")).unwrap();
        let _archive_history =
            PlaybackHistory::open(&archive_dir.path().join("playback-history.sqlite")).unwrap();
        let archived_track = crate::models::Track {
            id: "local:archive-track".into(),
            provider: "local".into(),
            provider_id: "archive-track".into(),
            title: "Portable archive track".into(),
            artist: "Synthetic artist".into(),
            ..Default::default()
        };
        archive_store
            .save_library_tracks(std::slice::from_ref(&archived_track))
            .unwrap();
        let mut archive_application = archive_store.settings().unwrap();
        archive_application.theme = "light".into();
        archive_store.save_settings(&archive_application).unwrap();
        archive_store
            .set_json_preference(
                "connections",
                &serde_json::json!({"discogsEnabled":true,"discogsToken":"archive-secret"}),
            )
            .unwrap();
        let archive_id = Uuid::new_v4();
        let archive = backup_state::prepare_backup(
            archive_dir.path(),
            archive_id,
            BackupStateOptions::default(),
        )
        .unwrap();
        let archive_settings = fs::read_to_string(&archive.settings_json).unwrap();
        assert!(!archive_settings.contains("archive-secret"));
        assert!(!archive_settings.contains("discogsToken"));
        let archive_connection = Connection::open(&archive.library_sqlite).unwrap();
        let has_connections: bool = archive_connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key='connections')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_connections);

        let current_dir = tempfile::tempdir().unwrap();
        let current = Arc::new(Store::open(&current_dir.path().join("burrata.sqlite")).unwrap());
        let current_history = Arc::new(
            PlaybackHistory::open(&current_dir.path().join("playback-history.sqlite")).unwrap(),
        );
        let current_track = crate::models::Track {
            id: "local:current-only".into(),
            provider: "local".into(),
            provider_id: "current-only".into(),
            title: "Current only track".into(),
            artist: "Synthetic artist".into(),
            ..Default::default()
        };
        current
            .save_library_tracks(std::slice::from_ref(&current_track))
            .unwrap();
        let mut current_settings = current.settings().unwrap();
        current_settings.music_directory = current_dir.path().join("music").display().to_string();
        current_settings.yt_dlp_path = current_dir
            .path()
            .join("tools/yt-dlp")
            .display()
            .to_string();
        current_settings.ffmpeg_path = current_dir
            .path()
            .join("tools/ffmpeg")
            .display()
            .to_string();
        current_settings.ffprobe_path = current_dir
            .path()
            .join("tools/ffprobe")
            .display()
            .to_string();
        current_settings.js_runtime_path =
            current_dir.path().join("tools/node").display().to_string();
        current_settings.playlist_directories =
            vec![current_dir.path().join("playlists").display().to_string()];
        current_settings.download_audio_verification_enabled = true;
        current_settings.playback_diagnostics_enabled = true;
        current.save_settings(&current_settings).unwrap();
        current
            .set_json_preference(
                "connections",
                &serde_json::json!({
                    "slskd":{"enabled":true,"baseUrl":"http://127.0.0.1:5030","downloadsPath":"/trusted/downloads"},
                    "discogsEnabled":true,
                    "discogsToken":"current-secret-must-not-be-copied"
                }),
            )
            .unwrap();
        current
            .set_json_preference(
                "connections_session_only",
                &serde_json::json!(["discogs", "unknown-provider", "current-secret"]),
            )
            .unwrap();
        current
            .set_json_preference("playbackEngine.v1", &serde_json::json!("native"))
            .unwrap();
        current
            .set_json_preference(
                "directoryBrowserRoots.v1",
                &serde_json::json!([{
                    "id":"current-root",
                    "name":"Current music root",
                    "path":current_dir.path().display().to_string()
                }]),
            )
            .unwrap();
        current
            .set_json_preference(
                "automation.permissions.v1",
                &serde_json::json!({"read":true,"control":false,"manage":false,"token":"drop-me"}),
            )
            .unwrap();
        current
            .set_json_preference("releaseSetupComplete.v1", &serde_json::json!(true))
            .unwrap();
        current
            .set_json_preference("radioRecognitionEnabled.v1", &serde_json::json!(false))
            .unwrap();

        let stage_dir = tempfile::tempdir().unwrap();
        let options = RestoreStateOptions {
            mode: RestoreMode::Replace,
            snapshot_id: archive_id.to_string(),
            include_library: true,
            include_history: true,
            include_settings: true,
            include_playback: true,
            ..Default::default()
        };
        backup_state::reconstruct_staged(
            Some(&archive.library_sqlite),
            Some(&archive.history_sqlite),
            Some(&archive.settings_json),
            Some(&archive.playback_json),
            stage_dir.path(),
            &options,
        )
        .unwrap();
        let staged = Store::open(&stage_dir.path().join("burrata.sqlite")).unwrap();
        preserve_current_machine_state(&current, &staged).unwrap();

        assert!(staged
            .get_library_track("local:archive-track")
            .unwrap()
            .is_some());
        assert!(staged
            .get_library_track("local:current-only")
            .unwrap()
            .is_none());
        let restored_settings = staged.settings().unwrap();
        assert_eq!(
            restored_settings.music_directory,
            current_settings.music_directory
        );
        assert_eq!(restored_settings.yt_dlp_path, current_settings.yt_dlp_path);
        assert_eq!(restored_settings.ffmpeg_path, current_settings.ffmpeg_path);
        assert_eq!(
            restored_settings.ffprobe_path,
            current_settings.ffprobe_path
        );
        assert_eq!(
            restored_settings.js_runtime_path,
            current_settings.js_runtime_path
        );
        assert_eq!(
            restored_settings.playlist_directories,
            current_settings.playlist_directories
        );
        assert!(restored_settings.download_audio_verification_enabled);
        assert!(restored_settings.playback_diagnostics_enabled);
        assert_eq!(restored_settings.theme, "light");

        let connections = staged.get_json_preference("connections").unwrap().unwrap();
        assert_eq!(connections["slskd"]["downloadsPath"], "/trusted/downloads");
        assert_eq!(connections["discogsEnabled"], true);
        assert!(connections.get("discogsToken").is_none());
        assert_eq!(
            staged
                .get_json_preference("connections_session_only")
                .unwrap(),
            Some(serde_json::json!(["discogs"]))
        );
        assert_eq!(
            staged.get_json_preference("playbackEngine.v1").unwrap(),
            Some(serde_json::json!("native"))
        );
        assert_eq!(
            staged
                .get_json_preference("directoryBrowserRoots.v1")
                .unwrap(),
            current
                .get_json_preference("directoryBrowserRoots.v1")
                .unwrap()
        );
        assert_eq!(
            staged
                .get_json_preference("automation.permissions.v1")
                .unwrap(),
            Some(serde_json::json!({"read":true,"control":false,"manage":false}))
        );
        assert_eq!(
            staged
                .get_json_preference("releaseSetupComplete.v1")
                .unwrap(),
            Some(serde_json::json!(true))
        );
        assert_eq!(
            staged
                .get_json_preference("radioRecognitionEnabled.v1")
                .unwrap(),
            Some(serde_json::json!(false))
        );
    }

    #[test]
    fn staged_merge_preserves_current_machine_state_from_sanitized_snapshot() {
        let incoming_dir = tempfile::tempdir().unwrap();
        let incoming = Store::open(&incoming_dir.path().join("burrata.sqlite")).unwrap();
        let _incoming_history =
            PlaybackHistory::open(&incoming_dir.path().join("playback-history.sqlite")).unwrap();
        let imported_track = crate::models::Track {
            id: "local:incoming-merge-track".into(),
            provider: "local".into(),
            provider_id: "incoming-merge-track".into(),
            title: "Incoming portable track".into(),
            artist: "Synthetic artist".into(),
            ..Default::default()
        };
        incoming
            .save_library_tracks(std::slice::from_ref(&imported_track))
            .unwrap();
        let mut incoming_settings = incoming.settings().unwrap();
        incoming_settings.theme = "light".into();
        incoming.save_settings(&incoming_settings).unwrap();
        let incoming_snapshot_id = Uuid::new_v4();
        let incoming_prepared = backup_state::prepare_backup(
            incoming_dir.path(),
            incoming_snapshot_id,
            BackupStateOptions::default(),
        )
        .unwrap();
        // An absent application field in a Merge archive must leave the
        // destination's existing value intact while present portable fields
        // (theme below) are imported.
        let mut incoming_settings_archive: backup_state::SettingsArchive =
            serde_json::from_reader(File::open(&incoming_prepared.settings_json).unwrap()).unwrap();
        incoming_settings_archive
            .application
            .as_object_mut()
            .unwrap()
            .remove("autoPreview");
        serde_json::to_writer(
            File::create(&incoming_prepared.settings_json).unwrap(),
            &incoming_settings_archive,
        )
        .unwrap();

        let current_dir = tempfile::tempdir().unwrap();
        let current = Arc::new(Store::open(&current_dir.path().join("burrata.sqlite")).unwrap());
        let current_history = Arc::new(
            PlaybackHistory::open_with_barrier(
                &current_dir.path().join("playback-history.sqlite"),
                current.library_write_barrier(),
            )
            .unwrap(),
        );
        let current_track = crate::models::Track {
            id: "local:existing-merge-track".into(),
            provider: "local".into(),
            provider_id: "existing-merge-track".into(),
            title: "Existing local track".into(),
            artist: "Synthetic artist".into(),
            ..Default::default()
        };
        current
            .save_library_tracks(std::slice::from_ref(&current_track))
            .unwrap();
        let mut trusted_settings = current.settings().unwrap();
        trusted_settings.music_directory = current_dir.path().join("music").display().to_string();
        trusted_settings.yt_dlp_path = current_dir
            .path()
            .join("tools/yt-dlp")
            .display()
            .to_string();
        trusted_settings.ffmpeg_path = current_dir
            .path()
            .join("tools/ffmpeg")
            .display()
            .to_string();
        trusted_settings.ffprobe_path = current_dir
            .path()
            .join("tools/ffprobe")
            .display()
            .to_string();
        trusted_settings.js_runtime_path =
            current_dir.path().join("tools/node").display().to_string();
        trusted_settings.playlist_directories =
            vec![current_dir.path().join("playlists").display().to_string()];
        trusted_settings.download_audio_verification_enabled = true;
        trusted_settings.playback_diagnostics_enabled = true;
        trusted_settings.auto_preview = false;
        current.save_settings(&trusted_settings).unwrap();
        current
            .set_json_preference(
                "connections",
                &serde_json::json!({
                    "slskd":{"enabled":true,"baseUrl":"http://127.0.0.1:5030","downloadsPath":"/trusted/downloads"},
                    "discogsEnabled":true,
                    "discogsToken":"current-secret-must-not-be-copied"
                }),
            )
            .unwrap();
        current
            .set_json_preference("releaseSetupComplete.v1", &serde_json::json!(true))
            .unwrap();
        current
            .set_json_preference("radioRecognitionEnabled.v1", &serde_json::json!(false))
            .unwrap();

        // Match the production Merge flow: start from the current sanitized
        // projection, reconstruct imported typed state into it, then restore
        // only trusted machine-local fields from the live Store.
        let manager = BackupManager::new(
            current_dir.path().to_path_buf(),
            current.clone(),
            current_history,
        );
        let current_snapshot = manager
            .prepare_coordinated_backup(Uuid::new_v4(), BackupStateOptions::default(), None)
            .unwrap();
        let stage_dir = tempfile::tempdir().unwrap();
        fs::copy(
            &current_snapshot.library_sqlite,
            stage_dir.path().join("burrata.sqlite"),
        )
        .unwrap();
        fs::copy(
            &current_snapshot.history_sqlite,
            stage_dir.path().join("playback-history.sqlite"),
        )
        .unwrap();
        let selected = vec![
            "library".to_owned(),
            "history".to_owned(),
            "settings".to_owned(),
            "playback".to_owned(),
        ];
        let mut options = restore_options(
            &incoming_snapshot_id.to_string(),
            RestoreMode::Merge,
            &selected,
            Vec::new(),
        );
        options.current_asset_paths = backup_state::current_asset_paths_from_snapshot(
            &current_snapshot.source_library_snapshot,
        )
        .unwrap();
        options.current_music_root = backup_state::current_music_root_from_snapshot(
            &current_snapshot.source_library_snapshot,
        )
        .unwrap();
        backup_state::reconstruct_staged(
            Some(&incoming_prepared.library_sqlite),
            Some(&incoming_prepared.history_sqlite),
            Some(&incoming_prepared.settings_json),
            Some(&incoming_prepared.playback_json),
            stage_dir.path(),
            &options,
        )
        .unwrap();
        let staged_database = stage_dir.path().join("burrata.sqlite");
        let staged = Store::open(&staged_database).unwrap();
        preserve_current_machine_state(&manager.store, &staged).unwrap();
        drop(staged);
        checkpoint_restore_database(&staged_database).unwrap();

        let restored = Store::open(&staged_database).unwrap();
        assert!(restored
            .get_library_track("local:incoming-merge-track")
            .unwrap()
            .is_some());
        assert!(restored
            .get_library_track("local:existing-merge-track")
            .unwrap()
            .is_some());
        let restored_settings = restored.settings().unwrap();
        assert_eq!(restored_settings.theme, "light");
        assert!(!restored_settings.auto_preview);
        assert_eq!(
            restored_settings.music_directory,
            trusted_settings.music_directory
        );
        assert_eq!(restored_settings.yt_dlp_path, trusted_settings.yt_dlp_path);
        assert_eq!(restored_settings.ffmpeg_path, trusted_settings.ffmpeg_path);
        assert_eq!(
            restored_settings.ffprobe_path,
            trusted_settings.ffprobe_path
        );
        assert_eq!(
            restored_settings.js_runtime_path,
            trusted_settings.js_runtime_path
        );
        assert_eq!(
            restored_settings.playlist_directories,
            trusted_settings.playlist_directories
        );
        assert!(restored_settings.download_audio_verification_enabled);
        assert!(restored_settings.playback_diagnostics_enabled);
        let connections = restored
            .get_json_preference("connections")
            .unwrap()
            .unwrap();
        assert_eq!(connections["slskd"]["downloadsPath"], "/trusted/downloads");
        assert_eq!(connections["discogsEnabled"], true);
        assert!(connections.get("discogsToken").is_none());
        assert_eq!(
            restored
                .get_json_preference("releaseSetupComplete.v1")
                .unwrap(),
            Some(serde_json::json!(true))
        );
        assert_eq!(
            restored
                .get_json_preference("radioRecognitionEnabled.v1")
                .unwrap(),
            Some(serde_json::json!(false))
        );
    }

    #[test]
    fn cold_restore_prepared_phase_installs_pair_and_commits_journal() {
        let data = tempfile::tempdir().unwrap();
        write_db(data.path(), "burrata.sqlite", "old-library");
        write_db(data.path(), "playback-history.sqlite", "old-history");
        let transaction_id = Uuid::new_v4();
        let pending = write_pending(data.path(), transaction_id, "prepared");
        write_db(
            Path::new(&pending.stage_directory),
            "burrata.sqlite",
            "new-library",
        );
        write_db(
            Path::new(&pending.stage_directory),
            "playback-history.sqlite",
            "new-history",
        );

        assert!(apply_pending_restore(data.path()).unwrap());
        assert_eq!(read_marker(data.path(), "burrata.sqlite"), "new-library");
        assert_eq!(
            read_marker(data.path(), "playback-history.sqlite"),
            "new-history"
        );
        let journal: PendingRestoreWire =
            serde_json::from_reader(File::open(data.path().join(PENDING_RESTORE_FILE)).unwrap())
                .unwrap();
        assert_eq!(journal.phase, "committed");
        finalize_pending_restore(data.path()).unwrap();
        assert!(!data.path().join(PENDING_RESTORE_FILE).exists());
        assert!(!Path::new(&pending.recovery_directory).exists());
    }

    #[test]
    fn cold_restore_swapping_phase_rolls_back_interrupted_install() {
        let data = tempfile::tempdir().unwrap();
        let transaction_id = Uuid::new_v4();
        let pending = write_pending(data.path(), transaction_id, "swapping");
        write_db(
            Path::new(&pending.recovery_directory),
            "burrata.sqlite",
            "old-library",
        );
        write_db(
            Path::new(&pending.recovery_directory),
            "playback-history.sqlite",
            "old-history",
        );
        write_db(data.path(), "burrata.sqlite", "partially-installed-library");
        // Simulate a crash after removing the old history database.

        assert!(!apply_pending_restore(data.path()).unwrap());
        assert_eq!(read_marker(data.path(), "burrata.sqlite"), "old-library");
        assert_eq!(
            read_marker(data.path(), "playback-history.sqlite"),
            "old-history"
        );
        let journal: PendingRestoreWire =
            serde_json::from_reader(File::open(data.path().join(PENDING_RESTORE_FILE)).unwrap())
                .unwrap();
        assert_eq!(journal.phase, "rolled_back");
        // Restart after rollback, before cleanup: no second DB swap is needed.
        assert!(!apply_pending_restore(data.path()).unwrap());
        finalize_pending_restore(data.path()).unwrap();
        assert!(!data.path().join(PENDING_RESTORE_FILE).exists());
        assert!(!Path::new(&pending.stage_directory).exists());
        assert!(!Path::new(&pending.recovery_directory).exists());
    }

    #[test]
    fn swapping_rollback_removes_foreign_wal_when_recovery_had_no_wal() {
        let data = tempfile::tempdir().unwrap();
        let transaction_id = Uuid::new_v4();
        let pending = write_pending(data.path(), transaction_id, "swapping");
        write_db(
            Path::new(&pending.recovery_directory),
            "burrata.sqlite",
            "old-library",
        );
        write_db(
            Path::new(&pending.recovery_directory),
            "playback-history.sqlite",
            "old-history",
        );
        write_db(data.path(), "burrata.sqlite", "new-library");
        fs::write(data.path().join("burrata.sqlite-wal"), b"foreign wal bytes").unwrap();
        fs::write(data.path().join("burrata.sqlite-shm"), b"foreign shm bytes").unwrap();
        assert!(!Path::new(&pending.recovery_directory)
            .join("burrata.sqlite-wal")
            .exists());

        assert!(!apply_pending_restore(data.path()).unwrap());
        assert_eq!(read_marker(data.path(), "burrata.sqlite"), "old-library");
        assert!(!data.path().join("burrata.sqlite-wal").exists());
        assert!(!data.path().join("burrata.sqlite-shm").exists());
    }

    #[test]
    fn cold_restore_committed_phase_is_idempotent_until_profile_opens() {
        let data = tempfile::tempdir().unwrap();
        write_db(data.path(), "burrata.sqlite", "installed-library");
        write_db(data.path(), "playback-history.sqlite", "installed-history");
        let transaction_id = Uuid::new_v4();
        let pending = write_pending(data.path(), transaction_id, "committed");

        assert!(apply_pending_restore(data.path()).unwrap());
        assert_eq!(
            read_marker(data.path(), "burrata.sqlite"),
            "installed-library"
        );
        finalize_pending_restore(data.path()).unwrap();
        assert!(!data.path().join(PENDING_RESTORE_FILE).exists());
        assert!(!Path::new(&pending.recovery_directory).exists());
    }

    #[test]
    fn terminal_restore_cleanup_resumes_after_either_directory_was_removed() {
        for phase in ["committed", "rolled_back"] {
            for removed in ["stage", "recovery"] {
                let data = tempfile::tempdir().unwrap();
                write_db(data.path(), "burrata.sqlite", "stable-library");
                write_db(data.path(), "playback-history.sqlite", "stable-history");
                let pending = write_pending(data.path(), Uuid::new_v4(), phase);
                let directory = if removed == "stage" {
                    &pending.stage_directory
                } else {
                    &pending.recovery_directory
                };
                fs::remove_dir_all(directory).unwrap();
                assert_eq!(
                    apply_pending_restore(data.path()).unwrap(),
                    phase == "committed"
                );
                finalize_pending_restore(data.path()).unwrap();
                // A subsequent launch/finalization is also harmless.
                assert!(!apply_pending_restore(data.path()).unwrap());
                finalize_pending_restore(data.path()).unwrap();
                assert_eq!(read_marker(data.path(), "burrata.sqlite"), "stable-library");
                assert_eq!(
                    read_marker(data.path(), "playback-history.sqlite"),
                    "stable-history"
                );
                assert!(!Path::new(&pending.stage_directory).exists());
                assert!(!Path::new(&pending.recovery_directory).exists());
                assert!(!data.path().join(PENDING_RESTORE_FILE).exists());
            }
        }
    }

    #[test]
    fn corrupt_staged_database_is_rejected_and_next_start_rolls_back() {
        let data = tempfile::tempdir().unwrap();
        write_db(data.path(), "burrata.sqlite", "old-library");
        write_db(data.path(), "playback-history.sqlite", "old-history");
        let transaction_id = Uuid::new_v4();
        let pending = write_pending(data.path(), transaction_id, "prepared");
        fs::write(
            Path::new(&pending.stage_directory).join("burrata.sqlite"),
            b"corrupt database bytes",
        )
        .unwrap();
        write_db(
            Path::new(&pending.stage_directory),
            "playback-history.sqlite",
            "new-history",
        );

        assert!(apply_pending_restore(data.path()).is_err());
        assert_eq!(
            fs::read(data.path().join("burrata.sqlite"))
                .unwrap()
                .as_slice(),
            b"corrupt database bytes"
        );
        assert!(!apply_pending_restore(data.path()).unwrap());
        assert_eq!(read_marker(data.path(), "burrata.sqlite"), "old-library");
        assert_eq!(
            read_marker(data.path(), "playback-history.sqlite"),
            "old-history"
        );
    }

    #[test]
    fn unavailable_automatic_backup_mount_is_rejected_by_marker() {
        let destination = tempfile::tempdir().unwrap();
        let marker = Uuid::new_v4().to_string();
        let marker_path = destination_marker_path(destination.path(), &marker).unwrap();
        fs::write(&marker_path, &marker).unwrap();
        assert!(destination_is_mounted(destination.path(), Some(&marker)));
        fs::remove_file(marker_path).unwrap();
        assert!(!destination_is_mounted(destination.path(), Some(&marker)));
        assert!(!destination_is_mounted(destination.path(), None));
    }

    #[test]
    fn profile_lock_reports_contention_and_releases_with_owner() {
        let profile = tempfile::tempdir().unwrap();
        let owner =
            acquire_profile_lock_with_timeout(profile.path(), std::time::Duration::ZERO).unwrap();
        assert!(
            acquire_profile_lock_with_timeout(profile.path(), std::time::Duration::ZERO).is_err()
        );
        drop(owner);
        assert!(
            acquire_profile_lock_with_timeout(profile.path(), std::time::Duration::ZERO).is_ok()
        );
    }

    #[test]
    fn automatic_retention_removes_only_registered_owned_archives() {
        let data = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let mut schedule = BackupSchedule::default();
        schedule.enabled = true;
        schedule.destination = Some(destination.path().to_path_buf());
        schedule.retention_daily = 1;
        schedule.retention_weekly = 1;
        manager.set_schedule(schedule).unwrap();

        let owned_name = format!("burrata-auto-{}.burrata-backup", Uuid::new_v4());
        let manual_name = format!("user-burrata-auto-{}.burrata-backup", Uuid::new_v4());
        fs::write(destination.path().join(&owned_name), b"owned archive").unwrap();
        fs::write(destination.path().join(&manual_name), b"user archive").unwrap();
        let owned = vec![OwnedArchive {
            file_name: owned_name.clone(),
            created_at: "2025-01-01T00:00:00Z".into(),
            cadence: "daily".into(),
            destination: fs::canonicalize(destination.path()).unwrap(),
        }];
        manager
            .store
            .set_json_preference(OWNED_ARCHIVES_KEY, &serde_json::to_value(owned).unwrap())
            .unwrap();
        let new_archive = destination
            .path()
            .join(format!("burrata-auto-{}.burrata-backup", Uuid::new_v4()));
        fs::write(&new_archive, b"new archive").unwrap();

        manager
            .record_automatic_archive(&new_archive, "2025-01-02T00:00:00Z", 1234)
            .unwrap();
        assert!(!destination.path().join(owned_name).exists());
        assert!(destination.path().join(manual_name).exists());
        assert!(new_archive.exists());
        let saved = manager.get_schedule().unwrap();
        assert_eq!(saved.last_archive_bytes, Some(1234));
        assert_eq!(saved.last_status.as_deref(), Some("complete"));
    }

    #[test]
    fn due_backup_catches_up_until_success_and_failure_does_not_advance_clock() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let now = DateTime::parse_from_rfc3339("2025-04-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut schedule = BackupSchedule {
            enabled: true,
            interval: BackupInterval::Daily,
            enabled_at: Some("2025-04-01T12:00:00Z".into()),
            last_success_at: Some("2025-04-01T12:00:00Z".into()),
            ..BackupSchedule::default()
        };
        manager
            .store
            .set_json_preference(SCHEDULE_KEY, &serde_json::to_value(&schedule).unwrap())
            .unwrap();
        assert!(schedule_due(&schedule, now));
        manager.set_schedule_error("simulated backup failure");
        let saved = manager.get_schedule().unwrap();
        assert_eq!(
            saved.last_success_at.as_deref(),
            Some("2025-04-01T12:00:00Z")
        );
        assert_eq!(
            saved.last_error.as_deref(),
            Some("simulated backup failure")
        );
        assert!(!schedule_due(&saved, Utc::now()));
        let retry_at = DateTime::parse_from_rfc3339(saved.retry_after_at.as_deref().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        assert!(!schedule_due(&saved, retry_at - Duration::seconds(1)));
        assert!(schedule_due(&saved, retry_at));
        assert_eq!(
            saved.next_due_at.as_deref(),
            saved.retry_after_at.as_deref()
        );

        schedule.last_success_at = Some(now.to_rfc3339());
        assert!(!schedule_due(&schedule, now));
    }

    #[test]
    fn daily_schedule_uses_utc_across_dst_clock_changes_and_sleep_wake() {
        let parse = |value: &str| {
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc)
        };
        let mut schedule = BackupSchedule {
            enabled: true,
            interval: BackupInterval::Daily,
            enabled_at: Some("2025-03-08T16:30:00Z".into()),
            ..BackupSchedule::default()
        };

        // Denver moves from MST to MDT on this date. A daily interval remains
        // 24 hours in UTC, so its displayed local hour moves from 09:30 to
        // 10:30 rather than being recalculated from the machine timezone.
        assert_eq!(next_due(&schedule), Some(parse("2025-03-09T16:30:00Z")));
        assert!(!schedule_due(&schedule, parse("2025-03-09T16:29:59Z")));
        assert!(schedule_due(&schedule, parse("2025-03-09T16:30:00Z")));

        // Moving the wall clock backwards after a successful run cannot make
        // the next daily backup due early.
        schedule.last_success_at = Some("2025-03-09T16:31:00Z".into());
        assert_eq!(next_due(&schedule), Some(parse("2025-03-10T16:31:00Z")));
        assert!(!schedule_due(&schedule, parse("2025-03-09T16:30:30Z")));

        // Waking after several missed intervals produces one due decision;
        // recording that successful run resets the next due time from the
        // success timestamp instead of replaying each missed day.
        let wake = parse("2025-03-18T12:00:00Z");
        assert!(schedule_due(&schedule, wake));
        schedule.last_success_at = Some(wake.to_rfc3339());
        assert!(!schedule_due(&schedule, wake));
        assert_eq!(next_due(&schedule), Some(wake + Duration::days(1)));

        // Fall-back is also anchored to UTC: the local clock repeats an hour,
        // but the due instant is still exactly 24 hours after last success.
        schedule.last_success_at = Some("2025-11-01T16:30:00Z".into());
        assert_eq!(next_due(&schedule), Some(parse("2025-11-02T16:30:00Z")));
    }

    #[test]
    fn schedule_persists_media_choice_and_legacy_defaults_to_metadata_only() {
        let old: BackupSchedule = serde_json::from_value(serde_json::json!({
            "enabled": false,
            "destination": null,
            "interval": "daily",
            "retentionDaily": 7,
            "retentionWeekly": 4,
            "lastSuccessAt": null,
            "enabledAt": null,
            "lastError": null
        }))
        .unwrap();
        assert!(!old.include_media);

        let data = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let schedule = BackupSchedule {
            enabled: true,
            destination: Some(destination.path().to_path_buf()),
            include_media: true,
            ..BackupSchedule::default()
        };
        let saved = manager.set_schedule(schedule).unwrap();
        assert!(saved.include_media);
        #[cfg(unix)]
        assert!(saved.destination_available_bytes.is_some());
        assert!(manager.get_schedule().unwrap().include_media);
    }

    #[test]
    fn library_component_version_comes_from_staged_sqlite_user_version() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("staged.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA user_version=11;").unwrap();
        drop(connection);
        assert_eq!(sqlite_user_version(&path).unwrap(), 11);
    }

    #[test]
    fn auxiliary_restore_never_overwrites_an_existing_lyrics_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.lrc");
        let destination = directory.path().join("track.lrc");
        fs::write(&source, b"[00:01.00] restored lyric").unwrap();
        fs::write(&destination, b"user edited lyric").unwrap();
        let component = crate::backup::BackupComponent {
            id: "aux-lyrics:test".into(),
            path: "aux/lyrics/test.lrc".into(),
            encoding: "binary".into(),
            bytes: b"[00:01.00] restored lyric".len() as u64,
            sha256: format!("{:x}", sha2::Sha256::digest(b"[00:01.00] restored lyric")),
            schema_version: None,
        };

        assert!(copy_auxiliary_noclobber(&source, &destination, &component).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"user edited lyric");
    }

    #[test]
    fn settings_import_applies_only_selected_allowlisted_fields_and_preserves_paths() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let mut existing = manager.store.settings().unwrap();
        existing.music_directory = "/profile/music".into();
        existing.theme = "dark".into();
        manager.store.save_settings(&existing).unwrap();
        let settings_path = data.path().join("portable-settings.json");
        let settings = backup_state::SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: 1,
            snapshot_id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            application: serde_json::json!({
                "theme": "light",
                "musicDirectory": "/untrusted/imported/path",
                "playbackDiagnosticsEnabled": true
            }),
            preferences: [
                (
                    "loudness.v1".to_owned(),
                    serde_json::json!({"mode":"album","targetLufs":-18.0,"ceilingDbtp":-1.0,"maximumBoostDb":6.0}),
                ),
                (
                    "connections".to_owned(),
                    serde_json::json!({"discogsToken":"must-not-import"}),
                ),
            ]
            .into_iter()
            .collect(),
            frontend_preferences: [
                (
                    "burrata.waveform-enabled.v1".to_owned(),
                    Value::Bool(true),
                ),
                ("untrusted.secret".to_owned(), Value::String("omit".into())),
            ]
            .into_iter()
            .collect(),
            asset_path_hints: vec![backup_state::AssetPathHint {
                asset_id: "asset-1".into(),
                root_key: "music".into(),
                relative_path: "song.flac".into(),
            }],
            projection_summary: backup_state::SettingsProjectionSummary::default(),
        };
        fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();

        let preview = manager.inspect_settings_path(&settings_path).unwrap();
        assert_eq!(preview.application_fields, vec!["theme"]);
        assert!(preview.preference_keys.contains(&"loudness.v1".into()));
        assert_eq!(preview.excluded_asset_path_hints, 1);
        assert_eq!(preview.excluded_preference_keys, 1);
        let result = manager
            .apply_settings_import(
                &preview.preview_id,
                backup_state::SettingsImportSelection {
                    application: true,
                    preference_keys: vec!["loudness.v1".into()],
                    frontend_preference_keys: vec!["burrata.waveform-enabled.v1".into()],
                },
            )
            .unwrap();
        let after = manager.store.settings().unwrap();
        assert_eq!(after.theme, "light");
        assert_eq!(after.music_directory, "/profile/music");
        assert!(!after.playback_diagnostics_enabled);
        assert_eq!(result.application_fields, 1);
        assert_eq!(result.preferences, 1);
        assert_eq!(
            result
                .frontend_preferences
                .get("burrata.waveform-enabled.v1"),
            Some(&Value::Bool(true))
        );
        let pending = manager
            .store
            .get_json_preference(crate::backup_frontend_preferences::PENDING_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(pending["schemaVersion"], 1);
        assert!(Uuid::parse_str(pending["nonce"].as_str().unwrap()).is_ok());
        assert_eq!(pending["preferences"]["burrata.waveform-enabled.v1"], true);
        assert!(pending.get("currentSessionId").is_none());
        assert!(manager
            .store
            .get_json_preference("connections")
            .unwrap()
            .is_none());
        assert_eq!(
            manager
                .store
                .get_json_preference("loudness.v1")
                .unwrap()
                .unwrap()["targetLufs"],
            -18.0
        );
        assert!(manager
            .store
            .get_json_preference(crate::restore_work_policy::PREFERENCE_KEY)
            .unwrap()
            .is_none());
        let policy = crate::restore_work_policy::state(&manager.store).unwrap();
        assert!(!policy.review_required);
        assert!(policy.categories.iter().all(|category| !category.paused));
    }

    #[test]
    fn settings_import_pauses_only_selected_automatic_enable_categories() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        manager
            .store
            .set_json_preference(
                crate::restore_work_policy::PREFERENCE_KEY,
                &serde_json::json!({
                    "schemaVersion":1,
                    "reviewRequired":false,
                    "snapshotId":"previously-reviewed",
                    "metadataRetagAnalysisPaused":true,
                    "assetAutotagPaused":false,
                    "backgroundSourcePreparationPaused":true,
                    "backgroundStreamMediaAnalysisPaused":false
                }),
            )
            .unwrap();
        let settings_path = data.path().join("portable-settings.json");
        let settings = backup_state::SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: 1,
            snapshot_id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            application: serde_json::json!({
                "backgroundSourcePreparation": true,
                "theme": "light"
            }),
            preferences: std::collections::BTreeMap::from([(
                "autotag.settings".into(),
                serde_json::json!({"enabled":true,"embedArtwork":false}),
            )]),
            frontend_preferences: std::collections::BTreeMap::from([(
                "burrata.stream-media-analysis-enabled.v1".into(),
                Value::Bool(true),
            )]),
            ..backup_state::SettingsArchive::default()
        };
        fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let preview = manager.inspect_settings_path(&settings_path).unwrap();
        let result = manager
            .apply_settings_import(
                &preview.preview_id,
                backup_state::SettingsImportSelection {
                    application: true,
                    preference_keys: vec!["autotag.settings".into()],
                    frontend_preference_keys: vec![
                        "burrata.stream-media-analysis-enabled.v1".into()
                    ],
                },
            )
            .unwrap();
        assert_eq!(result.application_fields, 2);
        assert_eq!(result.preferences, 1);
        assert_eq!(
            result
                .frontend_preferences
                .get("burrata.stream-media-analysis-enabled.v1"),
            Some(&Value::Bool(true))
        );
        let policy = crate::restore_work_policy::state(&manager.store).unwrap();
        assert!(policy.review_required);
        assert_eq!(
            policy
                .categories
                .iter()
                .find(|category| category.kind
                    == crate::restore_work_policy::AutomaticWorkKind::AssetAutotag)
                .unwrap()
                .paused,
            true
        );
        assert_eq!(
            policy
                .categories
                .iter()
                .find(|category| category.kind
                    == crate::restore_work_policy::AutomaticWorkKind::BackgroundSourcePreparation)
                .unwrap()
                .paused,
            true
        );
        assert_eq!(
            policy
                .categories
                .iter()
                .find(|category| category.kind
                    == crate::restore_work_policy::AutomaticWorkKind::BackgroundStreamMediaAnalysis)
                .unwrap()
                .paused,
            true
        );
        assert_eq!(
            policy
                .categories
                .iter()
                .find(|category| category.kind
                    == crate::restore_work_policy::AutomaticWorkKind::MetadataRetagAnalysis)
                .unwrap()
                .paused,
            true,
            "pre-existing reviewed category choice is preserved"
        );
        assert!(crate::restore_work_policy::is_paused(
            &manager.store,
            crate::restore_work_policy::AutomaticWorkKind::AssetAutotag
        ));
        assert!(crate::restore_work_policy::ensure_stream_media_analysis_allowed(
            &manager.store,
            false,
        )
        .is_ok());
    }

    #[test]
    fn settings_import_is_rejected_before_writes_during_restore_exclusivity() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let settings_path = data.path().join("portable-settings.json");
        let settings = backup_state::SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: 1,
            snapshot_id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            application: serde_json::json!({"theme":"light"}),
            ..backup_state::SettingsArchive::default()
        };
        fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let preview = manager.inspect_settings_path(&settings_path).unwrap();
        let restore_permit = manager.store.try_restore_permit().unwrap();
        let result = manager.apply_settings_import(
            &preview.preview_id,
            backup_state::SettingsImportSelection {
                application: true,
                ..backup_state::SettingsImportSelection::default()
            },
        );
        assert!(result.unwrap_err().contains("restore is in progress"));
        assert_eq!(manager.store.settings().unwrap().theme, "dark");
        drop(restore_permit);
    }

    #[test]
    fn settings_import_uses_immutable_reviewed_copy_and_rejects_staged_tampering() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let settings_path = data.path().join("portable-settings.json");
        let settings = backup_state::SettingsArchive {
            format: "org.burrata.settings".into(),
            schema_version: 1,
            snapshot_id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            application: serde_json::json!({"theme":"light"}),
            ..backup_state::SettingsArchive::default()
        };
        fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let preview = manager.inspect_settings_path(&settings_path).unwrap();
        fs::write(&settings_path, b"changed after preview").unwrap();
        manager
            .apply_settings_import(
                &preview.preview_id,
                backup_state::SettingsImportSelection {
                    application: true,
                    ..backup_state::SettingsImportSelection::default()
                },
            )
            .unwrap();
        assert_eq!(manager.store.settings().unwrap().theme, "light");

        let settings = backup_state::SettingsArchive {
            application: serde_json::json!({"theme":"system"}),
            ..settings
        };
        fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let tampered_preview = manager.inspect_settings_path(&settings_path).unwrap();
        let staged_path = manager
            .settings_previews
            .lock()
            .unwrap()
            .get(&tampered_preview.preview_id)
            .unwrap()
            .path
            .clone();
        fs::write(&staged_path, b"tampered staged review snapshot").unwrap();
        assert!(manager
            .apply_settings_import(
                &tampered_preview.preview_id,
                backup_state::SettingsImportSelection {
                    application: true,
                    ..backup_state::SettingsImportSelection::default()
                }
            )
            .is_err());
        assert_eq!(manager.store.settings().unwrap().theme, "light");
    }

    #[test]
    fn standalone_settings_export_omits_machine_paths_and_unallowlisted_secrets() {
        let data = tempfile::tempdir().unwrap();
        let manager = create_manager(data.path());
        let mut current = manager.store.settings().unwrap();
        current.music_directory = "/private/music".into();
        current.yt_dlp_path = "/private/tool/path".into();
        manager.store.save_settings(&current).unwrap();
        manager
            .store
            .set_json_preference(
                "connections",
                &serde_json::json!({"discogsToken":"must-not-export"}),
            )
            .unwrap();
        let lyric_choice_key = format!("lyrics.choice.v1.{}", "c".repeat(64));
        manager
            .store
            .set_json_preference(
                &lyric_choice_key,
                &serde_json::json!({
                    "candidate": {
                        "id": "990031",
                        "trackName": "Synthetic Song",
                        "artistName": "Synthetic Artist",
                        "albumName": "Synthetic Album",
                        "durationSeconds": 180.0,
                        "hasSynced": true,
                        "automaticMatch": true,
                        "plainTextMatch": false,
                        "explanation": "match",
                        "sourceUrl": "https://private.example/lyrics?id=990031&token=secret"
                    },
                    "record": {
                        "trackName": "Synthetic Song",
                        "artistName": "Synthetic Artist",
                        "albumName": "Synthetic Album",
                        "duration": 180.0,
                        "instrumental": false,
                        "syncedLyrics": "[00:01.00]Private lyric body",
                        "plainLyrics": null
                    }
                }),
            )
            .unwrap();
        let destination = data.path().join("portable-settings.json");

        let prepared = manager
            .prepare_coordinated_backup(Uuid::new_v4(), BackupStateOptions::default(), None)
            .unwrap();
        manager
            .write_settings_export(&destination, prepared)
            .unwrap();
        let exported: backup_state::SettingsArchive =
            serde_json::from_reader(File::open(destination).unwrap()).unwrap();
        assert!(exported.asset_path_hints.is_empty());
        assert_eq!(exported.projection_summary.asset_path_hints, 0);
        assert_eq!(exported.application["musicDirectory"], "");
        assert_eq!(exported.application["ytDlpPath"], "yt-dlp");
        assert!(!exported.preferences.contains_key("connections"));
        assert!(!exported.preferences.contains_key(&lyric_choice_key));
        assert_eq!(exported.projection_summary.omitted_private_settings, 1);
        assert!(!serde_json::to_string(&exported)
            .unwrap()
            .contains("must-not-export"));
        assert!(!serde_json::to_string(&exported)
            .unwrap()
            .contains("Private lyric body"));
        assert!(!serde_json::to_string(&exported)
            .unwrap()
            .contains("private.example"));
        assert!(!serde_json::to_string(&exported)
            .unwrap()
            .contains("/private/"));
    }
}
