//! Crash-cleanable, app-owned staging identities for backup operations.
//!
//! Profile stages are direct UUID children of `.backup-staging` and carry a
//! versioned ownership marker. External archive temporaries are registered by
//! exact UUID filename together with the canonical parent directory identity.
//! Cleanup never searches generic temporary-file patterns.
//! Windows identity uses volume serial and file index from a directory handle;
//! that implementation still needs qualification on a Windows runner.

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const ROOT_NAME: &str = ".backup-staging";
const MARKER_NAME: &str = ".burrata-stage-owner";
const JOURNAL_VERSION: u32 = 1;
const MARKER_PREFIX: &str = "burrata-backup-stage-v1\n";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub profile_stages_removed: usize,
    pub external_stages_removed: usize,
    pub entries_retained: usize,
}

/// Profile-private stage manager. Callers must hold the app's profile lock
/// while invoking cleanup or mutating durable backup state.
#[derive(Debug, Clone)]
pub struct BackupStaging {
    profile: PathBuf,
    root: PathBuf,
    profile_identity: DirectoryIdentity,
    root_identity: DirectoryIdentity,
}

impl BackupStaging {
    pub fn open(profile_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(profile_dir)?;
        reject_symlink_directory(profile_dir)?;
        let profile = fs::canonicalize(profile_dir)?;
        let profile_identity = directory_identity(&profile)?;
        let root = profile.join(ROOT_NAME);
        match fs::create_dir(&root) {
            Ok(()) => set_private_directory_permissions(&root)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        reject_symlink_directory(&root)?;
        set_private_directory_permissions(&root)?;
        let canonical_root = fs::canonicalize(&root)?;
        if canonical_root.parent() != Some(profile.as_path()) {
            return Err(invalid_data("backup staging root escaped the profile"));
        }
        let root_identity = directory_identity(&canonical_root)?;
        Ok(Self {
            profile,
            root: canonical_root,
            profile_identity,
            root_identity,
        })
    }

    pub fn create_profile_stage(&self, kind: &str) -> io::Result<ProfileStage> {
        if !valid_kind(kind) {
            return Err(invalid_input("invalid backup staging kind"));
        }
        self.check_profile_and_root()?;
        let id = Uuid::new_v4();
        let path = self.root.join(id.to_string());
        fs::create_dir(&path)?;
        set_private_directory_permissions(&path)?;
        let marker_path = path.join(MARKER_NAME);
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker_path)?;
        marker.write_all(format!("{MARKER_PREFIX}{kind}\n{id}\n").as_bytes())?;
        marker.sync_all()?;
        sync_directory(&path)?;
        sync_directory(&self.root)?;
        let identity = directory_identity(&path)?;
        Ok(ProfileStage {
            manager: self.clone(),
            id,
            kind: kind.to_owned(),
            path,
            identity,
            active: true,
        })
    }

    /// Create an app-owned archive temporary beside its final destination.
    /// The registry is durable before the temporary file is created, so a
    /// crash at any point leaves enough information for conservative cleanup.
    pub fn create_external_file(&self, parent: &Path) -> io::Result<ExternalStageFile> {
        self.check_profile_and_root()?;
        let canonical_parent = fs::canonicalize(parent)?;
        reject_symlink_directory(&canonical_parent)?;
        let parent_identity = directory_identity(&canonical_parent)?;
        let id = Uuid::new_v4();
        let file_name = format!("burrata-backup-stage-{id}.tmp");
        let path = canonical_parent.join(&file_name);
        let journal_path = self.root.join(format!("external-{id}.json"));
        let journal = ExternalStageJournal {
            version: JOURNAL_VERSION,
            id,
            parent: canonical_parent.clone(),
            parent_identity,
            file_name,
        };
        let mut journal_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&journal_path)?;
        serde_json::to_writer(&mut journal_file, &journal).map_err(io::Error::other)?;
        journal_file.sync_all()?;
        sync_directory(&self.root)?;

        let file = match create_private_external_file(&path) {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_file(&journal_path);
                return Err(error);
            }
        };
        sync_directory(&canonical_parent)?;
        Ok(ExternalStageFile {
            manager: self.clone(),
            id,
            path,
            parent: canonical_parent,
            parent_identity,
            journal_path,
            file: Some(file),
            active: true,
        })
    }

    /// Remove only marker-owned profile stages and registered exact external
    /// archive temporaries. Unknown files and restore recovery artifacts are
    /// left untouched. Invoke while holding the profile lock.
    pub fn cleanup_abandoned(&self) -> io::Result<CleanupReport> {
        self.check_profile_and_root()?;
        let mut report = CleanupReport::default();
        for entry in fs::read_dir(&self.root)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    report.entries_retained += 1;
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                report.entries_retained += 1;
                continue;
            };
            if let Some(id) = parse_external_journal_name(name) {
                if self.cleanup_external(&entry.path(), id)? {
                    report.external_stages_removed += 1;
                } else {
                    report.entries_retained += 1;
                }
            } else if let Ok(id) = Uuid::parse_str(name) {
                if self.cleanup_profile_stage(&entry.path(), id)? {
                    report.profile_stages_removed += 1;
                } else {
                    report.entries_retained += 1;
                }
            } else {
                report.entries_retained += 1;
            }
        }
        sync_directory(&self.root)?;
        Ok(report)
    }

    fn cleanup_profile_stage(&self, path: &Path, id: Uuid) -> io::Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Ok(false);
        }
        let marker_path = path.join(MARKER_NAME);
        let marker_metadata = match fs::symlink_metadata(&marker_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if marker_metadata.file_type().is_symlink() || !marker_metadata.is_file() {
            return Ok(false);
        }
        let marker = fs::read_to_string(marker_path)?;
        let mut lines = marker.lines();
        if lines.next() != Some("burrata-backup-stage-v1")
            || !lines.next().is_some_and(valid_kind)
            || lines.next().and_then(|value| Uuid::parse_str(value).ok()) != Some(id)
            || lines.next().is_some()
        {
            return Ok(false);
        }
        fs::remove_dir_all(path)?;
        Ok(true)
    }

    fn cleanup_external(&self, journal_path: &Path, expected_id: Uuid) -> io::Result<bool> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (journal_path, expected_id);
            // std has no portable directory identity suitable for proving a
            // removable destination still refers to the same mounted volume.
            return Ok(false);
        }
        #[cfg(any(unix, windows))]
        {
            let metadata = match fs::symlink_metadata(journal_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Ok(false);
            }
            let journal: ExternalStageJournal = match File::open(journal_path)
                .and_then(|file| serde_json::from_reader(file).map_err(io::Error::other))
            {
                Ok(journal) => journal,
                Err(_) => return Ok(false),
            };
            if journal.version != JOURNAL_VERSION
                || journal.id != expected_id
                || journal.file_name != format!("burrata-backup-stage-{expected_id}.tmp")
            {
                return Ok(false);
            }
            let current_parent = match fs::canonicalize(&journal.parent) {
                Ok(parent) => parent,
                Err(_) => return Ok(false),
            };
            if current_parent != journal.parent
                || directory_identity(&current_parent)? != journal.parent_identity
            {
                return Ok(false);
            }
            let path = current_parent.join(&journal.file_name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    return Ok(false)
                }
                Ok(_) => fs::remove_file(&path)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::remove_file(journal_path)?;
            sync_directory(&current_parent)?;
            sync_directory(&self.root)?;
            Ok(true)
        }
    }

    fn check_profile_and_root(&self) -> io::Result<()> {
        if fs::canonicalize(&self.profile)? != self.profile
            || directory_identity(&self.profile)? != self.profile_identity
        {
            return Err(invalid_data("backup profile identity changed"));
        }
        reject_symlink_directory(&self.root)?;
        if fs::canonicalize(&self.root)? != self.root
            || directory_identity(&self.root)? != self.root_identity
            || self.root.parent() != Some(self.profile.as_path())
        {
            return Err(invalid_data("backup staging root identity changed"));
        }
        Ok(())
    }

    fn remove_profile_stage(&self, path: &Path, id: Uuid, identity: DirectoryIdentity) {
        if self.check_profile_and_root().is_err()
            || path.parent() != Some(self.root.as_path())
            || fs::symlink_metadata(path)
                .ok()
                .is_none_or(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
            || directory_identity(path).ok() != Some(identity)
            || !self.stage_marker_matches(path, id)
        {
            return;
        }
        if fs::remove_dir_all(path).is_ok() {
            let _ = sync_directory(&self.root);
        }
    }

    fn stage_marker_matches(&self, path: &Path, id: Uuid) -> bool {
        fs::read_to_string(path.join(MARKER_NAME)).is_ok_and(|marker| {
            let mut lines = marker.lines();
            lines.next() == Some("burrata-backup-stage-v1")
                && lines.next().is_some_and(valid_kind)
                && lines.next().and_then(|value| Uuid::parse_str(value).ok()) == Some(id)
                && lines.next().is_none()
        })
    }
}

pub struct ProfileStage {
    manager: BackupStaging,
    id: Uuid,
    kind: String,
    path: PathBuf,
    identity: DirectoryIdentity,
    active: bool,
}

impl ProfileStage {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn keep(mut self) -> PathBuf {
        self.active = false;
        self.path.clone()
    }
}

impl Drop for ProfileStage {
    fn drop(&mut self) {
        if self.active {
            self.manager
                .remove_profile_stage(&self.path, self.id, self.identity);
        }
    }
}

pub struct ExternalStageFile {
    manager: BackupStaging,
    id: Uuid,
    path: PathBuf,
    parent: PathBuf,
    parent_identity: DirectoryIdentity,
    journal_path: PathBuf,
    file: Option<File>,
    active: bool,
}

impl ExternalStageFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_mut(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| invalid_data("staging file is already closed"))
    }

    /// Close and atomically publish the same-directory stage. If publication
    /// fails, Drop removes only the exact app-owned staging filename.
    pub fn publish(mut self, destination: &Path) -> io::Result<()> {
        if self.manager.check_profile_and_root().is_err()
            || fs::canonicalize(&self.parent)? != self.parent
            || directory_identity(&self.parent)? != self.parent_identity
        {
            return Err(invalid_data("backup destination parent identity changed"));
        }
        let destination_parent = destination.parent().unwrap_or_else(|| Path::new("."));
        if fs::canonicalize(destination_parent)? != self.parent {
            return Err(invalid_data("backup destination parent identity changed"));
        }
        let destination_name = destination
            .file_name()
            .ok_or_else(|| invalid_input("backup destination has no filename"))?;
        if destination_name == self.path.file_name().unwrap_or_default() {
            return Err(invalid_input("backup destination matches its staging file"));
        }
        if let Some(file) = self.file.take() {
            file.sync_all()?;
            drop(file);
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(invalid_data("backup staging file identity changed"));
        }
        fs::rename(&self.path, self.parent.join(destination_name))?;
        sync_directory(&self.parent)?;
        fs::remove_file(&self.journal_path)?;
        sync_directory(&self.manager.root)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for ExternalStageFile {
    fn drop(&mut self) {
        self.file.take();
        if !self.active || self.manager.check_profile_and_root().is_err() {
            return;
        }
        if fs::canonicalize(&self.parent).ok().as_deref() == Some(&self.parent)
            && directory_identity(&self.parent).ok() == Some(self.parent_identity)
        {
            let metadata = fs::symlink_metadata(&self.path);
            if metadata
                .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
            {
                let _ = fs::remove_file(&self.path);
                let _ = sync_directory(&self.parent);
            }
            if !self.path.exists() {
                let _ = fs::remove_file(&self.journal_path);
                let _ = sync_directory(&self.manager.root);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExternalStageJournal {
    version: u32,
    id: Uuid,
    parent: PathBuf,
    parent_identity: DirectoryIdentity,
    file_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial: u32,
    #[cfg(windows)]
    file_index: u64,
}

fn directory_identity(path: &Path) -> io::Result<DirectoryIdentity> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(invalid_data("staging parent is not a directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(DirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::{os::windows::ffi::OsStrExt, ptr};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
                FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
                FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            },
        };

        let wide_path = path
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
            return Err(io::Error::last_os_error());
        }
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let queried = unsafe { GetFileInformationByHandle(handle, &mut info) } != 0;
        let query_error = (!queried).then(io::Error::last_os_error);
        let closed = unsafe { CloseHandle(handle) } != 0;
        if let Some(error) = query_error {
            return Err(error);
        }
        if !closed {
            return Err(io::Error::last_os_error());
        }
        let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        if info.dwVolumeSerialNumber == 0 || file_index == 0 {
            return Err(invalid_data(
                "Windows did not provide a stable directory identity",
            ));
        }
        Ok(DirectoryIdentity {
            volume_serial: info.dwVolumeSerialNumber,
            file_index,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        // Unsupported platforms cannot prove a stable volume/file identity;
        // external crash cleanup therefore fails closed.
        Ok(DirectoryIdentity {})
    }
}

fn reject_symlink_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_data("staging path is not a real directory"));
    }
    Ok(())
}

fn set_private_directory_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create_private_external_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn sync_directory(path: &Path) -> io::Result<()> {
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

fn valid_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 48
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn parse_external_journal_name(name: &str) -> Option<Uuid> {
    let id = name.strip_prefix("external-")?.strip_suffix(".json")?;
    let parsed = Uuid::parse_str(id).ok()?;
    (parsed.to_string() == id).then_some(parsed)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_cleanup_removes_only_marked_profile_stages() {
        let profile = tempfile::tempdir().unwrap();
        let staging = BackupStaging::open(profile.path()).unwrap();
        let interrupted = staging.create_profile_stage("snapshot").unwrap();
        let interrupted_path = interrupted.path().to_path_buf();
        std::mem::forget(interrupted);

        let unrelated = staging.root.join("user-owned-note.tmp");
        fs::write(&unrelated, b"leave me alone").unwrap();
        let restore_stage = profile.path().join(".backup-restore-stage");
        fs::create_dir_all(&restore_stage).unwrap();
        fs::write(restore_stage.join("keep.db"), b"recovery").unwrap();
        let recovery_archive = profile.path().join("backup-recovery");
        fs::create_dir_all(&recovery_archive).unwrap();
        fs::write(recovery_archive.join("known-good.burrata-backup"), b"good").unwrap();

        let report = staging.cleanup_abandoned().unwrap();
        assert_eq!(report.profile_stages_removed, 1);
        assert!(!interrupted_path.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"leave me alone");
        assert_eq!(
            fs::read(restore_stage.join("keep.db")).unwrap(),
            b"recovery"
        );
        assert_eq!(
            fs::read(recovery_archive.join("known-good.burrata-backup")).unwrap(),
            b"good"
        );
    }

    #[test]
    fn normal_drop_removes_only_its_own_profile_stage() {
        let profile = tempfile::tempdir().unwrap();
        let staging = BackupStaging::open(profile.path()).unwrap();
        let stage = staging.create_profile_stage("restore-preview").unwrap();
        let path = stage.path().to_path_buf();
        drop(stage);
        assert!(!path.exists());
    }

    #[test]
    fn external_stage_cleanup_preserves_destination_and_unrelated_files() {
        let profile = tempfile::tempdir().unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let staging = BackupStaging::open(profile.path()).unwrap();
        let destination = destination_dir.path().join("backup.burrata-backup");
        fs::write(&destination, b"existing good archive").unwrap();
        let unrelated = destination_dir.path().join("someone.tmp");
        fs::write(&unrelated, b"unrelated").unwrap();

        let mut stage = staging
            .create_external_file(destination_dir.path())
            .unwrap();
        let stage_path = stage.path().to_path_buf();
        stage
            .file_mut()
            .unwrap()
            .write_all(b"partial archive")
            .unwrap();
        drop(stage.file.take());
        std::mem::forget(stage);

        let report = staging.cleanup_abandoned().unwrap();
        assert_eq!(report.external_stages_removed, 1);
        assert!(!stage_path.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"existing good archive");
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
    }

    #[test]
    fn external_parent_identity_change_fails_closed() {
        let profile = tempfile::tempdir().unwrap();
        let original_parent = tempfile::tempdir().unwrap();
        let replacement_parent = tempfile::tempdir().unwrap();
        let staging = BackupStaging::open(profile.path()).unwrap();
        let mut stage = staging
            .create_external_file(original_parent.path())
            .unwrap();
        stage.file_mut().unwrap().write_all(b"partial").unwrap();
        let staged_path = stage.path().to_path_buf();
        drop(stage.file.take());
        std::mem::forget(stage);

        let moved_parent = original_parent.path().with_extension("moved");
        let staged_name = staged_path.file_name().unwrap().to_owned();
        fs::rename(original_parent.path(), &moved_parent).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(replacement_parent.path(), original_parent.path()).unwrap();

        let report = staging.cleanup_abandoned().unwrap();
        assert_eq!(report.external_stages_removed, 0);
        assert!(moved_parent.join(staged_name).exists());
        fs::remove_dir_all(moved_parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_identity_distinguishes_profile_parents() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        assert_ne!(
            directory_identity(left.path()).unwrap(),
            directory_identity(right.path()).unwrap()
        );
    }

    #[test]
    fn publish_moves_only_stage_and_removes_its_journal() {
        let profile = tempfile::tempdir().unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let staging = BackupStaging::open(profile.path()).unwrap();
        let destination = destination_dir.path().join("complete.burrata-backup");
        let mut stage = staging
            .create_external_file(destination_dir.path())
            .unwrap();
        stage
            .file_mut()
            .unwrap()
            .write_all(b"verified archive")
            .unwrap();
        let stage_path = stage.path().to_path_buf();
        stage.publish(&destination).unwrap();
        assert!(!stage_path.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"verified archive");
        assert_eq!(
            staging.cleanup_abandoned().unwrap().external_stages_removed,
            0
        );
    }
}
