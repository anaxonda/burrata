//! Bounded ZIP creation, inspection, and extraction for `.burrata-backup`.
//!
//! This layer only handles validated files. It never opens an extracted SQL
//! file for execution or copies anything into a live profile.

use crate::backup::{
    auxiliary_component_path, validate_archive_path, AuxiliaryAttachment, AuxiliaryInput,
    BackupComponent, BackupManifest, BackupMediaSummary, ManifestError, BACKUP_FORMAT,
    BACKUP_SCHEMA_VERSION, MAX_COMPONENT_BYTES, MAX_MANIFEST_BYTES, MAX_TOTAL_UNCOMPRESSED_BYTES,
};
use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::{NamedTempFile, TempDir};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Make a transactionally consistent SQLite online snapshot to a new staged
/// file. The source connection remains owned by the caller, which must hold
/// the relevant store lock/checkpoint coordinator while calling this helper.
pub fn sqlite_online_snapshot(
    source: &rusqlite::Connection,
    destination: &Path,
) -> Result<(), ArchiveError> {
    use rusqlite::{backup::Backup, Connection, DatabaseName};
    use std::time::Duration;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        let mut staged = Connection::open(destination)?;
        let backup =
            Backup::new_with_names(source, DatabaseName::Main, &mut staged, DatabaseName::Main)?;
        backup.run_to_completion(128, Duration::from_millis(20), None)?;
        drop(backup);
        let integrity: String = staged.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(ArchiveError::Invalid(format!(
                "SQLite snapshot failed integrity check: {integrity}"
            )));
        }
        drop(staged);
        File::open(destination)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

pub const MANIFEST_ENTRY: &str = "manifest.json";
const MAX_ARCHIVE_BYTES: u64 = MAX_TOTAL_UNCOMPRESSED_BYTES + 64 * 1024 * 1024;

/// Sync directory metadata where the platform supports opening directories as
/// files. Windows does not expose a portable directory-flush operation through
/// std, so callers still sync each file and use same-directory atomic renames.
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveInput {
    pub id: String,
    pub schema_version: u32,
    pub source_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct MediaInput {
    /// Opaque path-free asset token used in the archive manifest.
    pub asset_id: String,
    pub source_path: PathBuf,
    /// When supplied, the staged bytes must still match the registered asset hash.
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInspection {
    pub snapshot_id: String,
    pub schema_version: u32,
    pub created_at: String,
    pub components: Vec<BackupComponentSummary>,
    pub media: BackupMediaSummary,
    pub archive_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupComponentSummary {
    pub id: String,
    pub bytes: u64,
    pub schema_version: Option<u32>,
    pub description: String,
}

/// Holds files extracted only after the complete archive passed validation.
/// Its temporary directory is removed when this value is dropped.
pub struct ValidatedArchive {
    pub inspection: BackupInspection,
    pub manifest: BackupManifest,
    staging: TempDir,
    paths: HashMap<String, PathBuf>,
}

impl ValidatedArchive {
    pub fn component_path(&self, id: &str) -> Option<&Path> {
        self.paths.get(id).map(PathBuf::as_path)
    }

    pub fn staging_path(&self) -> &Path {
        self.staging.path()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("backup archive is too large")]
    ArchiveTooLarge,
    #[error("backup archive is invalid: {0}")]
    Invalid(String),
    #[error("backup archive manifest is invalid: {0}")]
    Manifest(#[from] ManifestError),
    #[error("backup operation was canceled")]
    Canceled,
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("SQLite snapshot error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupPhase {
    Scanning,
    Writing,
    Verifying,
}

pub struct BackupControl<'a> {
    pub canceled: &'a AtomicBool,
    pub progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
}

impl BackupControl<'_> {
    fn check(&self) -> Result<(), ArchiveError> {
        if self.canceled.load(Ordering::Relaxed) {
            Err(ArchiveError::Canceled)
        } else {
            Ok(())
        }
    }

    fn report(&mut self, phase: BackupPhase, done: u64, total: u64) {
        if let Some(callback) = self.progress.as_deref_mut() {
            callback(phase, done, total);
        }
    }
}

/// Create a ZIP to a same-directory temporary file, inspect it fully, then
/// atomically publish it. Inputs must already be sanitized, transaction-safe
/// staging copies; this function never reads live application stores.
pub fn create_archive<'a>(
    inputs: &[ArchiveInput],
    media_inputs: &[MediaInput],
    media_summary: BackupMediaSummary,
    destination: &Path,
    app_version: &str,
    snapshot_id: Uuid,
    canceled: &'a AtomicBool,
    progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
) -> Result<BackupInspection, ArchiveError> {
    create_archive_with_auxiliary(
        inputs,
        media_inputs,
        &[],
        media_summary,
        destination,
        app_version,
        snapshot_id,
        canceled,
        progress,
    )
}

pub fn create_archive_with_auxiliary<'a>(
    inputs: &[ArchiveInput],
    media_inputs: &[MediaInput],
    auxiliary_inputs: &[AuxiliaryInput],
    media_summary: BackupMediaSummary,
    destination: &Path,
    app_version: &str,
    snapshot_id: Uuid,
    canceled: &'a AtomicBool,
    progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
) -> Result<BackupInspection, ArchiveError> {
    create_archive_with_auxiliary_inner(
        inputs,
        media_inputs,
        auxiliary_inputs,
        media_summary,
        destination,
        app_version,
        snapshot_id,
        canceled,
        progress,
        None,
        None,
    )
}

/// Production variant: registers the exact same-directory temporary with the
/// profile staging manager and extracts verification data under owned scratch.
pub fn create_archive_with_auxiliary_managed<'a>(
    inputs: &[ArchiveInput],
    media_inputs: &[MediaInput],
    auxiliary_inputs: &[AuxiliaryInput],
    media_summary: BackupMediaSummary,
    destination: &Path,
    app_version: &str,
    snapshot_id: Uuid,
    canceled: &'a AtomicBool,
    progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
    staging: &crate::backup_staging::BackupStaging,
    scratch_parent: &Path,
) -> Result<BackupInspection, ArchiveError> {
    create_archive_with_auxiliary_inner(
        inputs,
        media_inputs,
        auxiliary_inputs,
        media_summary,
        destination,
        app_version,
        snapshot_id,
        canceled,
        progress,
        Some(staging),
        Some(scratch_parent),
    )
}

fn create_archive_with_auxiliary_inner<'a>(
    inputs: &[ArchiveInput],
    media_inputs: &[MediaInput],
    auxiliary_inputs: &[AuxiliaryInput],
    media_summary: BackupMediaSummary,
    destination: &Path,
    app_version: &str,
    snapshot_id: Uuid,
    canceled: &'a AtomicBool,
    progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
    staging: Option<&crate::backup_staging::BackupStaging>,
    scratch_parent: Option<&Path>,
) -> Result<BackupInspection, ArchiveError> {
    if inputs.len() != 4
        || media_inputs.len() > 10_000
        || auxiliary_inputs.len() > crate::backup::MAX_AUXILIARY_ATTACHMENTS
    {
        return Err(ArchiveError::Invalid(
            "unsupported component or media count".into(),
        ));
    }
    if media_summary.assets != media_inputs.len() as u64
        || media_summary.included != !media_inputs.is_empty()
    {
        return Err(ArchiveError::Invalid(
            "media summary does not match included media".into(),
        ));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut control = BackupControl { canceled, progress };
    let mut seen_ids = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut manifest_components =
        Vec::with_capacity(inputs.len() + media_inputs.len() + auxiliary_inputs.len());
    let mut auxiliary_attachments = Vec::with_capacity(auxiliary_inputs.len());
    let mut total = 0u64;
    let total_items = (inputs.len() + media_inputs.len() + auxiliary_inputs.len()) as u64;
    for (index, input) in inputs.iter().enumerate() {
        control.check()?;
        let (path, encoding) = component_path_encoding(&input.id)?;
        if !seen_ids.insert(input.id.clone()) || !seen_paths.insert(path.to_owned()) {
            return Err(ArchiveError::Invalid("duplicate component input".into()));
        }
        let (bytes, sha256) = hash_file(&input.source_path, MAX_COMPONENT_BYTES, &mut control)?;
        control.report(BackupPhase::Scanning, index as u64 + 1, total_items);
        total = total
            .checked_add(bytes)
            .ok_or_else(|| ArchiveError::Invalid("size overflow".into()))?;
        manifest_components.push(BackupComponent {
            id: input.id.clone(),
            path: path.to_owned(),
            encoding: encoding.to_owned(),
            bytes,
            sha256,
            schema_version: Some(input.schema_version),
        });
    }
    let mut media_included = false;
    for (index, media) in media_inputs.iter().enumerate() {
        control.check()?;
        let id = format!("media:{}", media.asset_id);
        let path = format!("media/assets/{}", media.asset_id);
        if !seen_ids.insert(id.clone()) || !seen_paths.insert(path.clone()) {
            return Err(ArchiveError::Invalid("duplicate media asset".into()));
        }
        let (bytes, sha256) = hash_file(&media.source_path, MAX_COMPONENT_BYTES, &mut control)?;
        if media
            .expected_sha256
            .as_deref()
            .is_some_and(|expected| !expected.eq_ignore_ascii_case(&sha256))
        {
            return Err(ArchiveError::Invalid(format!(
                "registered media asset {} changed before backup",
                media.asset_id
            )));
        }
        control.report(
            BackupPhase::Scanning,
            inputs.len() as u64 + index as u64 + 1,
            total_items,
        );
        total = total
            .checked_add(bytes)
            .ok_or_else(|| ArchiveError::Invalid("size overflow".into()))?;
        manifest_components.push(BackupComponent {
            id,
            path,
            encoding: "binary".into(),
            bytes,
            sha256,
            schema_version: None,
        });
        media_included = true;
    }
    for (index, auxiliary) in auxiliary_inputs.iter().enumerate() {
        control.check()?;
        let attachment = auxiliary.attachment.clone();
        let id = attachment.component_id.clone();
        let path = auxiliary_component_path(&attachment.asset_id);
        if !seen_ids.insert(id.clone()) || !seen_paths.insert(path.clone()) {
            return Err(ArchiveError::Invalid(
                "duplicate auxiliary asset input".into(),
            ));
        }
        let (bytes, sha256) = hash_file(
            &auxiliary.source_path,
            crate::backup::MAX_LYRICS_SIDECAR_BYTES,
            &mut control,
        )?;
        if !sha256.eq_ignore_ascii_case(&auxiliary.expected_sha256) {
            return Err(ArchiveError::Invalid(format!(
                "auxiliary asset {} changed before backup",
                attachment.asset_id
            )));
        }
        total = total
            .checked_add(bytes)
            .ok_or_else(|| ArchiveError::Invalid("size overflow".into()))?;
        if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(ArchiveError::Invalid(
                "total uncompressed size exceeds the limit".into(),
            ));
        }
        manifest_components.push(BackupComponent {
            id,
            path,
            encoding: "binary".into(),
            bytes,
            sha256,
            schema_version: None,
        });
        auxiliary_attachments.push(attachment);
        control.report(
            BackupPhase::Scanning,
            (inputs.len() + media_inputs.len() + index + 1) as u64,
            total_items,
        );
    }
    if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
        return Err(ArchiveError::Invalid(
            "total uncompressed size exceeds the limit".into(),
        ));
    }
    let manifest = BackupManifest {
        format: BACKUP_FORMAT.into(),
        schema_version: BACKUP_SCHEMA_VERSION,
        app_version: app_version.to_owned(),
        created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        snapshot_id: snapshot_id.to_string(),
        components: manifest_components,
        media: BackupMediaSummary {
            included: media_included,
            ..media_summary
        },
        auxiliary_assets: auxiliary_attachments,
    };
    manifest.validate()?;
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ArchiveError::Invalid("manifest exceeds size limit".into()));
    }

    enum ArchiveTemp {
        Plain(NamedTempFile),
        Registered(crate::backup_staging::ExternalStageFile),
    }
    impl ArchiveTemp {
        fn path(&self) -> &Path {
            match self {
                Self::Plain(file) => file.path(),
                Self::Registered(file) => file.path(),
            }
        }
        fn file_mut(&mut self) -> Result<&mut File, ArchiveError> {
            match self {
                Self::Plain(file) => Ok(file.as_file_mut()),
                Self::Registered(file) => file.file_mut().map_err(ArchiveError::Io),
            }
        }
        fn publish(self, destination: &Path) -> Result<(), ArchiveError> {
            match self {
                Self::Plain(file) => file
                    .persist(destination)
                    .map(|_| ())
                    .map_err(|e| ArchiveError::Io(e.error)),
                Self::Registered(file) => file.publish(destination).map_err(ArchiveError::Io),
            }
        }
    }
    let mut temporary = match staging {
        Some(staging) => ArchiveTemp::Registered(staging.create_external_file(parent)?),
        None => ArchiveTemp::Plain(NamedTempFile::new_in(parent)?),
    };
    {
        let mut writer = ZipWriter::new(temporary.file_mut()?);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true)
            .unix_permissions(0o600);
        writer.start_file(MANIFEST_ENTRY, options)?;
        writer.write_all(&manifest_bytes)?;
        for (index, component) in manifest.components.iter().enumerate() {
            control.check()?;
            writer.start_file(component.path.clone(), options)?;
            let input = if component.id.starts_with("aux-lyrics:") {
                &auxiliary_inputs[auxiliary_inputs
                    .iter()
                    .position(|auxiliary| auxiliary.attachment.component_id == component.id)
                    .ok_or_else(|| ArchiveError::Invalid("missing auxiliary input".into()))?]
                .source_path
            } else if component.id.starts_with("media:") {
                &media_inputs[media_inputs
                    .iter()
                    .position(|m| m.asset_id == component.id[6..])
                    .ok_or_else(|| ArchiveError::Invalid("missing media input".into()))?]
                .source_path
            } else {
                &inputs
                    .iter()
                    .find(|input| input.id == component.id)
                    .ok_or_else(|| ArchiveError::Invalid("missing component input".into()))?
                    .source_path
            };
            let mut file = File::open(input)?;
            let (bytes, digest) = copy_hash(&mut file, &mut writer, component.bytes, &mut control)?;
            if bytes != component.bytes || !digest.eq_ignore_ascii_case(&component.sha256) {
                return Err(ArchiveError::Invalid(format!(
                    "staged component {} changed while backing up",
                    component.id
                )));
            }
            control.report(BackupPhase::Writing, index as u64 + 1, total_items);
        }
        let _ = writer.finish()?;
    }
    if let ArchiveTemp::Plain(file) = &temporary {
        file.as_file().sync_all()?;
    }
    control.check()?;
    control.report(BackupPhase::Verifying, 0, 1);
    let verified = inspect_archive_in(temporary.path(), canceled, None, scratch_parent)?;
    drop(verified);
    control.check()?;
    let archive_bytes = fs::metadata(temporary.path())?.len();
    if archive_bytes > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::ArchiveTooLarge);
    }
    temporary.publish(destination)?;
    if staging.is_none() {
        sync_directory(parent)?;
    }
    control.report(BackupPhase::Verifying, 1, 1);
    Ok(BackupInspection {
        snapshot_id: manifest.snapshot_id,
        schema_version: manifest.schema_version,
        created_at: manifest.created_at,
        components: manifest.components.iter().map(component_summary).collect(),
        media: manifest.media,
        archive_bytes,
        warnings: vec![
            "Checksums detect accidental corruption; they do not establish sender trust.".into(),
        ],
    })
}

/// Fully validate a ZIP archive and extract declared data into an isolated
/// temporary directory. No caller-visible file is published on failure.
pub fn inspect_archive<'a>(
    path: &Path,
    canceled: &'a AtomicBool,
    progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
) -> Result<ValidatedArchive, ArchiveError> {
    inspect_archive_in(path, canceled, progress, None)
}

pub fn inspect_archive_in<'a>(
    path: &Path,
    canceled: &'a AtomicBool,
    progress: Option<&'a mut dyn FnMut(BackupPhase, u64, u64)>,
    scratch_parent: Option<&Path>,
) -> Result<ValidatedArchive, ArchiveError> {
    let archive_bytes = fs::metadata(path)?.len();
    if archive_bytes == 0 || archive_bytes > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::ArchiveTooLarge);
    }
    validate_first_local_entry(path)?;
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    // ZIP readers index entries by name and may collapse duplicate central
    // directory entries before by_index/len expose them. Reject the physical
    // duplicate signal before inspecting the indexed entries.
    if archive.has_duplicate_names() {
        return Err(ArchiveError::Invalid(
            "duplicate ZIP path in central directory".into(),
        ));
    }
    if archive.len() < 5 || archive.len() > crate::backup::MAX_COMPONENTS + 1 {
        return Err(ArchiveError::Invalid("unexpected ZIP entry count".into()));
    }
    let mut seen = HashSet::new();
    let mut manifest_bytes = None;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        validate_archive_path(&name)?;
        if entry.is_dir()
            || entry.is_symlink()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err(ArchiveError::Invalid(format!(
                "unsupported ZIP entry type: {name}"
            )));
        }
        if entry.encrypted() {
            return Err(ArchiveError::Invalid(format!(
                "encrypted ZIP entry is unsupported: {name}"
            )));
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ArchiveError::Invalid(format!(
                "unsupported compression method: {name}"
            )));
        }
        if !seen.insert(name.clone()) {
            return Err(ArchiveError::Invalid(format!("duplicate ZIP path: {name}")));
        }
        if entry.size() > MAX_COMPONENT_BYTES || entry.compressed_size() > archive_bytes {
            return Err(ArchiveError::Invalid(format!(
                "ZIP entry exceeds size limit: {name}"
            )));
        }
        if index == 0 && name != MANIFEST_ENTRY {
            return Err(ArchiveError::Invalid(
                "manifest must be the first ZIP entry".into(),
            ));
        }
        if name == MANIFEST_ENTRY {
            if index != 0 || entry.size() > MAX_MANIFEST_BYTES as u64 {
                return Err(ArchiveError::Invalid(
                    "manifest must be first and stay within the byte limit".into(),
                ));
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .take(MAX_MANIFEST_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_MANIFEST_BYTES {
                return Err(ArchiveError::Invalid("manifest exceeds size limit".into()));
            }
            manifest_bytes = Some(bytes);
        }
    }
    let manifest = BackupManifest::parse(
        manifest_bytes
            .as_deref()
            .ok_or_else(|| ArchiveError::Invalid("missing manifest".into()))?,
    )?;
    let expected: HashSet<_> = manifest
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect();
    if seen.len() != expected.len() + 1 || !expected.iter().all(|name| seen.contains(*name)) {
        return Err(ArchiveError::Invalid(
            "ZIP entries do not match the manifest".into(),
        ));
    }
    let total_size = manifest
        .components
        .iter()
        .try_fold(0u64, |acc, component| {
            acc.checked_add(component.bytes)
                .ok_or(ArchiveError::Invalid("total size overflow".into()))
        })?;
    if total_size > MAX_TOTAL_UNCOMPRESSED_BYTES {
        return Err(ArchiveError::Invalid(
            "total uncompressed size exceeds the limit".into(),
        ));
    }

    let staging = match scratch_parent {
        Some(parent) => tempfile::Builder::new()
            .prefix("archive-inspect-")
            .tempdir_in(parent)?,
        None => tempfile::Builder::new()
            .prefix("burrata-backup-inspect-")
            .tempdir()?,
    };
    let mut paths = HashMap::new();
    let mut control = BackupControl { canceled, progress };
    for (index, component) in manifest.components.iter().enumerate() {
        control.check()?;
        let output_path = staging.path().join(&component.path);
        let parent = output_path
            .parent()
            .ok_or_else(|| ArchiveError::Invalid("invalid staged path".into()))?;
        fs::create_dir_all(parent)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)?;
        let mut zip_file = archive.by_name(&component.path)?;
        if zip_file.size() != component.bytes {
            return Err(ArchiveError::Invalid(format!(
                "declared size mismatch for {}",
                component.id
            )));
        }
        let (bytes, digest) = copy_hash(&mut zip_file, &mut output, component.bytes, &mut control)?;
        output.sync_all()?;
        if bytes != component.bytes || !digest.eq_ignore_ascii_case(&component.sha256) {
            return Err(ArchiveError::Invalid(format!(
                "checksum mismatch for {}",
                component.id
            )));
        }
        paths.insert(component.id.clone(), output_path);
        control.report(
            BackupPhase::Verifying,
            index as u64 + 1,
            manifest.components.len() as u64,
        );
    }
    Ok(ValidatedArchive {
        inspection: BackupInspection {
            snapshot_id: manifest.snapshot_id.clone(),
            schema_version: manifest.schema_version,
            created_at: manifest.created_at.clone(),
            components: manifest.components.iter().map(component_summary).collect(),
            media: manifest.media.clone(),
            archive_bytes,
            warnings: vec![
                "Checksums detect accidental corruption; they do not establish sender trust."
                    .into(),
            ],
        },
        manifest,
        staging,
        paths,
    })
}

fn validate_first_local_entry(path: &Path) -> Result<(), ArchiveError> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 30];
    file.read_exact(&mut header)?;
    if &header[..4] != b"PK\x03\x04" {
        return Err(ArchiveError::Invalid(
            "manifest is not the first local ZIP entry".into(),
        ));
    }
    let flags = u16::from_le_bytes([header[6], header[7]]);
    let method = u16::from_le_bytes([header[8], header[9]]);
    let name_len = u16::from_le_bytes([header[26], header[27]]) as usize;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as usize;
    if flags & 0x0001 != 0 || !matches!(method, 0 | 8) || name_len > 128 || extra_len > 16 * 1024 {
        return Err(ArchiveError::Invalid(
            "unsupported manifest ZIP header".into(),
        ));
    }
    let mut name = vec![0; name_len];
    file.read_exact(&mut name)?;
    if name != MANIFEST_ENTRY.as_bytes() {
        return Err(ArchiveError::Invalid(
            "manifest is not the first local ZIP entry".into(),
        ));
    }
    Ok(())
}

fn component_path_encoding(id: &str) -> Result<(&'static str, &'static str), ArchiveError> {
    match id {
        "library" => Ok(("data/library.sqlite", "sqlite")),
        "history" => Ok(("data/playback-history.sqlite", "sqlite")),
        "settings" => Ok(("data/settings.json", "json")),
        "playback" => Ok(("data/playback.json", "json")),
        _ => Err(ArchiveError::Invalid(format!("unsupported component {id}"))),
    }
}

fn component_summary(component: &BackupComponent) -> BackupComponentSummary {
    let description = match component.id.as_str() {
        "library" => "Library and playlists",
        "history" => "Listening history",
        "settings" => "Portable settings",
        "playback" => "Paused queue and resume position",
        _ if component.id.starts_with("media:") => "Verified audio asset",
        _ if component.id.starts_with("aux-lyrics:") => "Associated lyrics sidecar",
        _ => "Backup data",
    };
    BackupComponentSummary {
        id: component.id.clone(),
        bytes: component.bytes,
        schema_version: component.schema_version,
        description: description.into(),
    }
}

fn hash_file(
    path: &Path,
    max_bytes: u64,
    control: &mut BackupControl<'_>,
) -> Result<(u64, String), ArchiveError> {
    let mut file = File::open(path)?;
    let expected = file.metadata()?.len();
    if expected > max_bytes {
        return Err(ArchiveError::ArchiveTooLarge);
    }
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        control.check()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| ArchiveError::Invalid("size overflow".into()))?;
        if total > max_bytes {
            return Err(ArchiveError::ArchiveTooLarge);
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected {
        return Err(ArchiveError::Invalid(
            "source changed while being hashed".into(),
        ));
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn copy_hash<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    max_bytes: u64,
    control: &mut BackupControl<'_>,
) -> Result<(u64, String), ArchiveError> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        control.check()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| ArchiveError::Invalid("size overflow".into()))?;
        if total > max_bytes || total > MAX_COMPONENT_BYTES {
            return Err(ArchiveError::Invalid(
                "component exceeds declared size".into(),
            ));
        }
        writer.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn component_files(dir: &Path) -> Vec<ArchiveInput> {
        [
            ("library", "data-library.sqlite", 10),
            ("history", "data-history.sqlite", 1),
            ("settings", "data-settings.json", 1),
            ("playback", "data-playback.json", 1),
        ]
        .into_iter()
        .map(|(id, name, schema_version)| {
            let path = dir.join(name);
            fs::write(&path, format!("staged {id} component")).unwrap();
            ArchiveInput {
                id: id.into(),
                schema_version,
                source_path: path,
            }
        })
        .collect()
    }

    fn media_summary(assets: u64) -> BackupMediaSummary {
        BackupMediaSummary {
            included: assets > 0,
            assets,
            missing: 0,
            changed: 0,
            omitted: 0,
        }
    }

    fn local_entry_data_offset(bytes: &[u8], wanted_name: &[u8]) -> Option<usize> {
        let mut offset = 0usize;
        while offset + 46 <= bytes.len() {
            if &bytes[offset..offset + 4] != b"PK\x01\x02" {
                offset += 1;
                continue;
            }
            let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[offset + 32], bytes[offset + 33]]) as usize;
            let name_start = offset + 46;
            let name_end = name_start.checked_add(name_len)?;
            if name_end > bytes.len() {
                return None;
            }
            if &bytes[name_start..name_end] == wanted_name {
                let local_offset = u32::from_le_bytes([
                    bytes[offset + 42],
                    bytes[offset + 43],
                    bytes[offset + 44],
                    bytes[offset + 45],
                ]) as usize;
                if local_offset + 30 > bytes.len()
                    || &bytes[local_offset..local_offset + 4] != b"PK\x03\x04"
                {
                    return None;
                }
                let local_name_len =
                    u16::from_le_bytes([bytes[local_offset + 26], bytes[local_offset + 27]])
                        as usize;
                let local_extra_len =
                    u16::from_le_bytes([bytes[local_offset + 28], bytes[local_offset + 29]])
                        as usize;
                return local_offset
                    .checked_add(30)?
                    .checked_add(local_name_len)?
                    .checked_add(local_extra_len);
            }
            offset = name_end.checked_add(extra_len)?.checked_add(comment_len)?;
        }
        None
    }

    fn duplicate_central_directory_name(bytes: &mut [u8], original: &[u8], duplicate: &[u8]) {
        assert_eq!(original.len(), duplicate.len());
        let mut offset = 0usize;
        let mut changed = false;
        while offset + 46 <= bytes.len() {
            if &bytes[offset..offset + 4] != b"PK\x01\x02" {
                offset += 1;
                continue;
            }
            let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[offset + 32], bytes[offset + 33]]) as usize;
            let name_start = offset + 46;
            let name_end = name_start.checked_add(name_len).unwrap();
            if name_end > bytes.len() {
                panic!("truncated central directory fixture");
            }
            if !changed && &bytes[name_start..name_end] == original {
                bytes[name_start..name_end].copy_from_slice(duplicate);
                let local_offset = u32::from_le_bytes([
                    bytes[offset + 42],
                    bytes[offset + 43],
                    bytes[offset + 44],
                    bytes[offset + 45],
                ]) as usize;
                assert!(
                    local_offset + 30 <= bytes.len(),
                    "truncated local header fixture"
                );
                assert_eq!(&bytes[local_offset..local_offset + 4], b"PK\x03\x04");
                let local_name_len =
                    u16::from_le_bytes([bytes[local_offset + 26], bytes[local_offset + 27]])
                        as usize;
                assert_eq!(local_name_len, original.len());
                let local_name_start = local_offset + 30;
                assert_eq!(
                    &bytes[local_name_start..local_name_start + local_name_len],
                    original
                );
                bytes[local_name_start..local_name_start + local_name_len]
                    .copy_from_slice(duplicate);
                changed = true;
            }
            offset = name_end + extra_len + comment_len;
        }
        assert!(changed, "fixture central directory name was not found");
    }

    #[test]
    fn archive_round_trip_streams_components_and_media() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("audio.ogg");
        let components = component_files(dir.path());
        fs::write(&media, vec![7u8; 2_000_000]).unwrap();
        let destination = dir.path().join("roundtrip.burrata-backup");
        let cancel = no_cancel();
        let report = create_archive(
            &components,
            &[MediaInput {
                asset_id: Uuid::new_v4().to_string(),
                source_path: media,
                expected_sha256: None,
            }],
            media_summary(1),
            &destination,
            "0.1.8",
            Uuid::new_v4(),
            &cancel,
            None,
        )
        .unwrap();
        assert_eq!(report.components.len(), 5);
        assert!(report.media.included);
        let verified = inspect_archive(&destination, &cancel, None).unwrap();
        assert_eq!(verified.inspection.components.len(), 5);
        assert!(verified.component_path("library").unwrap().exists());
    }

    #[test]
    fn archive_round_trip_includes_hash_bound_lyrics_auxiliary_component() {
        let dir = tempfile::tempdir().unwrap();
        let components = component_files(dir.path());
        let audio_path = dir.path().join("track.flac");
        let lyrics_path = dir.path().join("track.lrc");
        fs::write(&audio_path, b"audio fixture").unwrap();
        fs::write(&lyrics_path, "[00:01.00] lyric line\n").unwrap();
        let audio_hash = format!("{:x}", Sha256::digest(b"audio fixture"));
        let asset = crate::models::LocalAsset {
            id: "asset-fixture".into(),
            recording_id: "recording-fixture".into(),
            path: audio_path.to_string_lossy().into_owned(),
            hash: audio_hash.clone(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            processing: "fixture".into(),
            track: crate::models::Track::default(),
        };
        let crate::backup::AuxiliaryDiscovery::Included(auxiliary) =
            crate::backup::snapshot_lyrics_sidecar(&asset, &audio_hash)
        else {
            panic!("valid exact sibling lyrics file should be included");
        };
        let media = MediaInput {
            asset_id: asset.id,
            source_path: audio_path,
            expected_sha256: Some(audio_hash),
        };
        let archive_path = dir.path().join("lyrics.burrata-backup");
        let cancellation = no_cancel();
        create_archive_with_auxiliary(
            &components,
            &[media],
            &[auxiliary],
            media_summary(1),
            &archive_path,
            "0.1.8",
            Uuid::new_v4(),
            &cancellation,
            None,
        )
        .unwrap();
        let validated = inspect_archive(&archive_path, &cancellation, None).unwrap();
        assert_eq!(validated.manifest.schema_version, 2);
        assert_eq!(validated.manifest.auxiliary_assets.len(), 1);
        let aux_id = &validated.manifest.auxiliary_assets[0].component_id;
        assert_eq!(
            fs::read(validated.component_path(aux_id).unwrap()).unwrap(),
            b"[00:01.00] lyric line\n"
        );
        drop(validated);
        let mut bytes = fs::read(&archive_path).unwrap();
        let aux_path = auxiliary_component_path("asset-fixture");
        let offset = local_entry_data_offset(&bytes, aux_path.as_bytes()).unwrap();
        bytes[offset] ^= 0x20;
        fs::write(&archive_path, bytes).unwrap();
        assert!(inspect_archive(&archive_path, &cancellation, None).is_err());
    }

    #[test]
    fn inspector_keeps_reading_schema_one_archives_without_auxiliary_entries() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = component_files(dir.path());
        let declarations = inputs
            .iter()
            .map(|input| {
                let bytes = fs::read(&input.source_path).unwrap();
                let (path, encoding) = component_path_encoding(&input.id).unwrap();
                BackupComponent {
                    id: input.id.clone(),
                    path: path.into(),
                    encoding: encoding.into(),
                    bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    schema_version: Some(input.schema_version),
                }
            })
            .collect::<Vec<_>>();
        let manifest = BackupManifest {
            format: BACKUP_FORMAT.into(),
            schema_version: 1,
            app_version: "0.1.8".into(),
            created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            snapshot_id: Uuid::new_v4().to_string(),
            components: declarations,
            media: media_summary(0),
            auxiliary_assets: vec![],
        };
        let archive_path = dir.path().join("legacy-v1.burrata-backup");
        let mut writer = ZipWriter::new(File::create(&archive_path).unwrap());
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o600);
        writer.start_file(MANIFEST_ENTRY, options).unwrap();
        writer
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        for component in &inputs {
            let path = component_path_encoding(&component.id).unwrap().0;
            writer.start_file(path, options).unwrap();
            writer
                .write_all(&fs::read(&component.source_path).unwrap())
                .unwrap();
        }
        writer.finish().unwrap();

        let validated = inspect_archive(&archive_path, &no_cancel(), None).unwrap();
        assert_eq!(validated.inspection.schema_version, 1);
        assert!(validated.manifest.auxiliary_assets.is_empty());
    }

    #[test]
    fn failed_replacement_keeps_previous_archive() {
        let dir = tempfile::tempdir().unwrap();
        let components = component_files(dir.path());
        let destination = dir.path().join("existing.burrata-backup");
        fs::write(&destination, b"previous good archive").unwrap();
        let cancel = AtomicBool::new(true);
        let result = create_archive(
            &components,
            &[],
            media_summary(0),
            &destination,
            "0.1.8",
            Uuid::new_v4(),
            &cancel,
            None,
        );
        assert!(matches!(result, Err(ArchiveError::Canceled)));
        assert_eq!(fs::read(destination).unwrap(), b"previous good archive");
    }

    #[test]
    fn managed_writer_cancellation_after_component_copy_preserves_previous_archive_and_cleans_owned_stage(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let components = component_files(dir.path());
        let destination = dir.path().join("existing.burrata-backup");
        let uncanceled = no_cancel();
        create_archive(
            &components,
            &[],
            media_summary(0),
            &destination,
            "0.1.8",
            Uuid::new_v4(),
            &uncanceled,
            None,
        )
        .unwrap();
        let previous_bytes = fs::read(&destination).unwrap();
        let previous_hash = Sha256::digest(&previous_bytes);

        let staging = crate::backup_staging::BackupStaging::open(dir.path()).unwrap();
        let stage_root = dir.path().join(".backup-staging");
        let sentinel = stage_root.join("unowned-sentinel.txt");
        fs::write(&sentinel, b"leave unowned staging entries untouched").unwrap();
        let sentinel_bytes = fs::read(&sentinel).unwrap();
        let canceled = AtomicBool::new(false);
        let mut writing_progress = 0;
        let mut progress = |phase, done, _total| {
            if phase == BackupPhase::Writing {
                writing_progress = done;
                canceled.store(true, Ordering::Relaxed);
            }
        };

        let result = create_archive_with_auxiliary_managed(
            &components,
            &[],
            &[],
            media_summary(0),
            &destination,
            "0.1.8",
            Uuid::new_v4(),
            &canceled,
            Some(&mut progress),
            &staging,
            dir.path(),
        );

        assert!(matches!(result, Err(ArchiveError::Canceled)));
        assert!(
            writing_progress > 0,
            "cancellation must follow a copied component"
        );
        let current_bytes = fs::read(&destination).unwrap();
        assert_eq!(Sha256::digest(&current_bytes), previous_hash);
        assert_eq!(current_bytes, previous_bytes);
        assert_eq!(fs::read(&sentinel).unwrap(), sentinel_bytes);
        let validated = inspect_archive(&destination, &uncanceled, None).unwrap();
        assert_eq!(validated.inspection.components.len(), 4);
        let remaining_stage_entries = fs::read_dir(&stage_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            remaining_stage_entries,
            vec![sentinel.file_name().unwrap().to_os_string()]
        );
        assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("burrata-backup-stage-")
        }));
    }

    #[test]
    fn rejects_path_traversal_and_corrupt_data() {
        let dir = tempfile::tempdir().unwrap();
        let components = component_files(dir.path());
        let destination = dir.path().join("backup.zip");
        let cancel = no_cancel();
        create_archive(
            &components,
            &[],
            media_summary(0),
            &destination,
            "0.1.8",
            Uuid::new_v4(),
            &cancel,
            None,
        )
        .unwrap();
        let mut bytes = fs::read(&destination).unwrap();
        let data = local_entry_data_offset(&bytes, b"data/settings.json").unwrap();
        bytes[data] ^= 0x80;
        fs::write(&destination, bytes).unwrap();
        assert!(inspect_archive(&destination, &cancel, None).is_err());
    }

    #[test]
    fn rejects_traversal_and_symlink_entries_before_extraction() {
        for (name, mode) in [("../outside", 0o100600), ("media/assets/evil", 0o120777)] {
            let dir = tempfile::tempdir().unwrap();
            let archive_path = dir.path().join("malicious.zip");
            let file = File::create(&archive_path).unwrap();
            let mut writer = ZipWriter::new(file);
            let opts = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(0o600);
            writer.start_file(MANIFEST_ENTRY, opts).unwrap();
            writer.write_all(b"{}").unwrap();
            writer
                .start_file(name, opts.unix_permissions(mode))
                .unwrap();
            writer.write_all(b"target").unwrap();
            writer.finish().unwrap();
            let cancel = no_cancel();
            assert!(inspect_archive(&archive_path, &cancel, None).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_zip_names() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("duplicate.zip");
        let mut writer = ZipWriter::new(File::create(&archive_path).unwrap());
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o600);
        writer.start_file(MANIFEST_ENTRY, opts).unwrap();
        writer.write_all(b"{}").unwrap();
        for name in [
            "data/library.sqlite",
            "data/playback-history.sqlite",
            "data/settings.json",
            "data/playback.json",
        ] {
            writer.start_file(name, opts).unwrap();
            writer.write_all(b"component").unwrap();
        }
        writer.finish().unwrap();
        let mut bytes = fs::read(&archive_path).unwrap();
        duplicate_central_directory_name(&mut bytes, b"data/playback.json", b"data/settings.json");
        fs::write(&archive_path, bytes).unwrap();
        let cancel = no_cancel();
        assert!(matches!(
            inspect_archive(&archive_path, &cancel, None),
            Err(ArchiveError::Invalid(message)) if message.contains("duplicate ZIP path")
        ));
    }

    #[test]
    fn cancellation_during_inspection_discards_staging() {
        let dir = tempfile::tempdir().unwrap();
        let components = component_files(dir.path());
        let destination = dir.path().join("backup.zip");
        let cancel = no_cancel();
        create_archive(
            &components,
            &[],
            media_summary(0),
            &destination,
            "0.1.8",
            Uuid::new_v4(),
            &cancel,
            None,
        )
        .unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            inspect_archive(&destination, &cancel, None),
            Err(ArchiveError::Canceled)
        ));
    }

    #[test]
    fn online_snapshot_includes_committed_wal_pages_without_touching_source() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.sqlite");
        let source = rusqlite::Connection::open(&source_path).unwrap();
        source.pragma_update(None, "journal_mode", "WAL").unwrap();
        source
            .execute_batch(
                "CREATE TABLE fixture(value TEXT); INSERT INTO fixture VALUES ('committed');",
            )
            .unwrap();
        let snapshot = dir.path().join("snapshot.sqlite");
        sqlite_online_snapshot(&source, &snapshot).unwrap();
        let copied = rusqlite::Connection::open(&snapshot).unwrap();
        let value: String = copied
            .query_row("SELECT value FROM fixture", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "committed");
        let live_value: String = source
            .query_row("SELECT value FROM fixture", [], |row| row.get(0))
            .unwrap();
        assert_eq!(live_value, "committed");
    }
}
