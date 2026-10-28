use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Take},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::Serialize;
use uuid::Uuid;

const MAX_PENDING_DROPS: usize = 8;
const DROP_TOKEN_TTL: Duration = Duration::from_secs(30);
pub const MAX_PLAYLIST_DROP_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
struct PendingDrop {
    registered_at: Instant,
    selection: DropSelection,
}

#[derive(Debug)]
enum DropSelection {
    One(PathBuf),
    Multiple(usize),
    Empty,
}

#[derive(Debug)]
pub(crate) enum ImportDropClaim {
    One(PathBuf),
    Multiple(usize),
    Empty,
}

#[derive(Default)]
pub(crate) struct ImportDropRegistry {
    pending: Mutex<HashMap<String, PendingDrop>>,
}

impl ImportDropRegistry {
    /// Register paths supplied only by Tauri's native OS drop event.
    /// Multi-file drops retain only their count, never the arbitrary path list.
    pub(crate) fn register(&self, paths: &[PathBuf]) -> String {
        self.register_at(paths, Instant::now())
    }

    fn register_at(&self, paths: &[PathBuf], now: Instant) -> String {
        let selection = match paths {
            [] => DropSelection::Empty,
            [path] => DropSelection::One(path.clone()),
            _ => DropSelection::Multiple(paths.len()),
        };
        let token = Uuid::new_v4().simple().to_string();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending
            .retain(|_, entry| now.saturating_duration_since(entry.registered_at) < DROP_TOKEN_TTL);
        while pending.len() >= MAX_PENDING_DROPS {
            let oldest = pending
                .iter()
                .min_by_key(|(_, entry)| entry.registered_at)
                .map(|(token, _)| token.clone());
            let Some(oldest) = oldest else { break };
            pending.remove(&oldest);
        }
        pending.insert(
            token.clone(),
            PendingDrop {
                registered_at: now,
                selection,
            },
        );
        token
    }

    /// Consume before any file work; even invalid/multi-file drops are one-shot.
    pub(crate) fn consume(&self, token: &str) -> Result<ImportDropClaim, String> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        pending
            .retain(|_, entry| now.saturating_duration_since(entry.registered_at) < DROP_TOKEN_TTL);
        let entry = pending.remove(token).ok_or_else(|| {
            "Dropped file request expired or was already used. Drop the file again.".to_owned()
        })?;
        Ok(match entry.selection {
            DropSelection::One(path) => ImportDropClaim::One(path),
            DropSelection::Multiple(count) => ImportDropClaim::Multiple(count),
            DropSelection::Empty => ImportDropClaim::Empty,
        })
    }

    pub(crate) fn discard(&self, token: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(token);
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct DropPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportFileDropEvent {
    pub token: String,
    pub kind: ImportFileDropKindHint,
    pub coordinate_space: DropCoordinateSpace,
    pub position: DropPoint,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DropCoordinateSpace {
    Logical,
    Physical,
}

pub(crate) const fn native_drop_coordinate_space() -> DropCoordinateSpace {
    if cfg!(target_os = "windows") {
        DropCoordinateSpace::Physical
    } else {
        // GTK uses window/widget units and Cocoa uses points; both map to CSS
        // pixels without applying devicePixelRatio.
        DropCoordinateSpace::Logical
    }
}

/// Filename-only routing hint for the UI. `consume_import_drop` remains the
/// authority and revalidates the extension and file before reading it.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImportFileDropKindHint {
    Playlist,
    Backup,
    Unsupported,
}

pub(crate) fn event_kind_hint(paths: &[PathBuf]) -> ImportFileDropKindHint {
    if let [path] = paths {
        return match classify_path(path) {
            Ok(ImportDropKind::Playlist) => ImportFileDropKindHint::Playlist,
            Ok(ImportDropKind::Backup) => ImportFileDropKindHint::Backup,
            Err(_) => ImportFileDropKindHint::Unsupported,
        };
    }
    ImportFileDropKindHint::Unsupported
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ConsumedImportDrop {
    Playlist {
        name: String,
        text: String,
    },
    Backup {
        preview: crate::backup_manager::BackupPreview,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportDropKind {
    Playlist,
    Backup,
}

pub(crate) fn classify_path(path: &Path) -> Result<ImportDropKind, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(unsupported_file_error)?;
    match extension.as_str() {
        "m3u" | "m3u8" | "csv" | "txt" => Ok(ImportDropKind::Playlist),
        "burrata-backup" => Ok(ImportDropKind::Backup),
        _ => Err(unsupported_file_error()),
    }
}

pub(crate) fn prepare_backup_path(path: &Path) -> Result<PathBuf, String> {
    if classify_path(path)? != ImportDropKind::Backup {
        return Err(unsupported_file_error());
    }
    canonical_regular_file(path)
}

pub(crate) fn read_playlist_file(path: &Path) -> Result<(String, String), String> {
    if classify_path(path)? != ImportDropKind::Playlist {
        return Err(unsupported_file_error());
    }
    let canonical = canonical_regular_file(path)?;
    let before_open = fs::metadata(&canonical).map_err(|_| dropped_file_unavailable())?;
    if before_open.len() > MAX_PLAYLIST_DROP_BYTES {
        return Err("Playlist file exceeds the 2 MiB import limit.".into());
    }
    let file = File::open(&canonical).map_err(|_| dropped_file_unavailable())?;
    let opened = file.metadata().map_err(|_| dropped_file_unavailable())?;
    if !opened.is_file() || !same_file_snapshot(&before_open, &opened) {
        return Err("Dropped file changed while it was being opened. Drop it again.".into());
    }
    if opened.len() > MAX_PLAYLIST_DROP_BYTES {
        return Err("Playlist file exceeds the 2 MiB import limit.".into());
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    let mut limited: Take<File> = file.take(MAX_PLAYLIST_DROP_BYTES + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the dropped playlist file.".to_owned())?;
    if bytes.len() as u64 > MAX_PLAYLIST_DROP_BYTES {
        return Err("Playlist file exceeds the 2 MiB import limit.".into());
    }
    let after_read = limited
        .get_ref()
        .metadata()
        .map_err(|_| dropped_file_unavailable())?;
    let after_path = fs::metadata(&canonical).map_err(|_| dropped_file_unavailable())?;
    if !same_file_snapshot(&opened, &after_read) || !same_file_snapshot(&opened, &after_path) {
        return Err("Dropped file changed while it was being read. Drop it again.".into());
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| "Playlist file must contain valid UTF-8 text.".to_owned())?;
    let name = canonical
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Dropped playlist".to_owned());
    Ok((name, text))
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, String> {
    let original = fs::symlink_metadata(path).map_err(|_| dropped_file_unavailable())?;
    if original.file_type().is_symlink() {
        return Err("Symbolic links cannot be imported by dropping them.".into());
    }
    if !original.is_file() {
        return Err("Drop a regular file, not a folder or special file.".into());
    }
    let canonical = fs::canonicalize(path).map_err(|_| dropped_file_unavailable())?;
    let resolved = fs::metadata(&canonical).map_err(|_| dropped_file_unavailable())?;
    if !resolved.is_file() || !same_file_snapshot(&original, &resolved) {
        return Err("Dropped file changed while it was being checked. Drop it again.".into());
    }
    Ok(canonical)
}

#[cfg(unix)]
fn same_file_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn same_file_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.created().ok() == right.created().ok()
}

fn dropped_file_unavailable() -> String {
    "Dropped file is unavailable. Drop it again and retry.".into()
}

fn unsupported_file_error() -> String {
    "Unsupported file. Choose an M3U, M3U8, CSV, TXT or Burrata backup file.".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn registry_token_is_one_shot_and_multi_file_drop_does_not_retain_paths() {
        let registry = ImportDropRegistry::default();
        let temp = tempfile::tempdir().unwrap();
        let first_path = temp.path().join("one.m3u");
        let first = registry.register(&[first_path]);
        assert!(matches!(
            registry.consume(&first).unwrap(),
            ImportDropClaim::One(_)
        ));
        assert!(registry
            .consume(&first)
            .unwrap_err()
            .contains("already used"));

        let multiple_paths = [temp.path().join("one.m3u"), temp.path().join("two.m3u")];
        let multiple = registry.register(&multiple_paths);
        assert!(matches!(
            registry.consume(&multiple).unwrap(),
            ImportDropClaim::Multiple(2)
        ));
    }

    #[test]
    fn event_kind_hint_routes_only_single_supported_extensions() {
        let playlist = PathBuf::from("set.m3u8");
        let backup = PathBuf::from("state.burrata-backup");
        let unknown = PathBuf::from("song.wav");
        assert!(matches!(
            event_kind_hint(std::slice::from_ref(&playlist)),
            ImportFileDropKindHint::Playlist
        ));
        assert!(matches!(
            event_kind_hint(std::slice::from_ref(&backup)),
            ImportFileDropKindHint::Backup
        ));
        assert!(matches!(
            event_kind_hint(std::slice::from_ref(&unknown)),
            ImportFileDropKindHint::Unsupported
        ));
        assert!(matches!(
            event_kind_hint(&[]),
            ImportFileDropKindHint::Unsupported
        ));
        assert!(matches!(
            event_kind_hint(&[playlist, backup]),
            ImportFileDropKindHint::Unsupported
        ));
    }

    #[test]
    fn native_drop_coordinate_space_matches_platform_adapter_units() {
        assert!(
            matches!(
                native_drop_coordinate_space(),
                DropCoordinateSpace::Physical
            ) == cfg!(target_os = "windows")
        );
    }

    #[test]
    fn registry_expires_tokens_and_evicts_oldest_at_capacity() {
        let registry = ImportDropRegistry::default();
        let now = Instant::now();
        let expired = registry.register_at(&[], now - DROP_TOKEN_TTL);
        assert!(registry.consume(&expired).unwrap_err().contains("expired"));

        let tokens = (0..=MAX_PENDING_DROPS)
            .map(|_| registry.register(&[]))
            .collect::<Vec<_>>();
        assert!(registry
            .consume(&tokens[0])
            .unwrap_err()
            .contains("expired"));
        for token in &tokens[1..] {
            assert!(matches!(
                registry.consume(token).unwrap(),
                ImportDropClaim::Empty
            ));
        }
    }

    #[test]
    fn reads_bounded_utf8_playlist_and_preserves_filename() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("list.M3U8");
        fs::write(&path, "#EXTM3U\n♫.mp3\n").unwrap();
        assert_eq!(classify_path(&path).unwrap(), ImportDropKind::Playlist);
        assert_eq!(
            read_playlist_file(&path).unwrap(),
            ("list.M3U8".into(), "#EXTM3U\n♫.mp3\n".into())
        );
    }

    #[test]
    fn rejects_unsupported_nonregular_oversized_and_invalid_utf8_files() {
        let temp = tempfile::tempdir().unwrap();
        let unsupported = temp.path().join("track.wav");
        fs::write(&unsupported, b"audio").unwrap();
        assert!(classify_path(&unsupported)
            .unwrap_err()
            .contains("Unsupported"));
        assert!(read_playlist_file(&unsupported).is_err());
        let directory = temp.path().join("folder.m3u8");
        fs::create_dir(&directory).unwrap();
        assert!(read_playlist_file(&directory)
            .unwrap_err()
            .contains("regular file"));

        let oversized = temp.path().join("large.txt");
        fs::write(&oversized, vec![b'a'; MAX_PLAYLIST_DROP_BYTES as usize + 1]).unwrap();
        assert!(read_playlist_file(&oversized)
            .unwrap_err()
            .contains("2 MiB"));

        let invalid_utf8 = temp.path().join("invalid.csv");
        fs::write(&invalid_utf8, [0xff, 0xfe]).unwrap();
        assert!(read_playlist_file(&invalid_utf8)
            .unwrap_err()
            .contains("UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_dropped_file() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.m3u");
        let link = temp.path().join("linked.m3u");
        fs::write(&target, "#EXTM3U\n").unwrap();
        symlink(&target, &link).unwrap();
        assert!(read_playlist_file(&link)
            .unwrap_err()
            .contains("Symbolic links"));
    }

    #[cfg(unix)]
    #[test]
    fn file_identity_check_rejects_path_replacement() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("list.m3u");
        let replacement = temp.path().join("replacement.m3u");
        fs::write(&path, "same length").unwrap();
        let opened = File::open(&path).unwrap();
        let before = opened.metadata().unwrap();
        fs::write(&replacement, "same length").unwrap();
        fs::rename(&replacement, &path).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_ne!(before.ino(), after.ino());
        assert!(!same_file_snapshot(&before, &after));
    }

    #[test]
    fn accepts_backup_extension_without_reading_archive_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("backup.burrata-backup");
        fs::write(&path, b"not inspected by drop bridge").unwrap();
        assert_eq!(classify_path(&path).unwrap(), ImportDropKind::Backup);
        assert_eq!(
            prepare_backup_path(&path).unwrap(),
            fs::canonicalize(&path).unwrap()
        );
    }
}
