//! Cold-start discard of Burrata's own profile, before SQLite is opened.
use crate::backup_archive::sync_directory;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use uuid::Uuid;

const JOURNAL: &str = ".burrata-factory-reset.json";
const MAX_JOURNAL_BYTES: u64 = 8192;
const FILES: &[&str] = &[
    "burrata.sqlite",
    "burrata.sqlite-wal",
    "burrata.sqlite-shm",
    "burrata.sqlite-journal",
    "playback-history.sqlite",
    "playback-history.sqlite-wal",
    "playback-history.sqlite-shm",
    "playback-history.sqlite-journal",
    "automation.json",
    "browser-bridge.json",
];
const DIRECTORIES: &[&str] = &["managed-provider-session", "youtube-support", "artwork"];

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    reset_id: Option<Uuid>,
    warning: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    id: Uuid,
    phase: Phase,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", deny_unknown_fields)]
enum Phase {
    Staged,
    Moving { entries: Vec<String> },
    Applied,
    Finished { warning: Option<String> },
    Failed { warning: String },
}

pub(crate) enum Startup {
    Idle,
    ApplyCredentials { quarantine: PathBuf },
}

pub(crate) fn validate_confirmation(confirmation: &str) -> Result<(), String> {
    if confirmation == "RESET" {
        Ok(())
    } else {
        Err("Type RESET to confirm the factory reset.".into())
    }
}

pub(crate) fn stage(profile: &Path, protected_paths: &[PathBuf]) -> Result<(), String> {
    for path in protected_paths {
        // Check both the configured name and its resolved location when it
        // exists. Existing download/backup folders cannot be part of the reset.
        let resolved = path.canonicalize().ok();
        for directory in DIRECTORIES {
            let owned = profile.join(directory);
            if path.starts_with(&owned)
                || resolved
                    .as_ref()
                    .is_some_and(|path| path.starts_with(&owned))
            {
                return Err(format!("Move the library or backup destination out of Burrata's {directory} folder before resetting."));
            }
        }
    }
    inventory(profile)?;
    let existing = read_journal(profile)?;
    if existing
        .as_ref()
        .is_some_and(|journal| !matches!(journal.phase, Phase::Failed { .. }))
    {
        return Err("A factory reset is already pending. Restart Burrata to finish it.".into());
    }
    write_journal(
        profile,
        &Journal {
            id: Uuid::new_v4(),
            version: 1,
            phase: Phase::Staged,
        },
        existing.is_none(),
    )
}

pub(crate) fn prepare_startup(profile: &Path, restore_pending: bool) -> Result<Startup, String> {
    let Some(mut journal) = read_journal(profile)? else {
        return Ok(Startup::Idle);
    };
    let quarantine = quarantine_dir(profile, journal.id);
    if matches!(journal.phase, Phase::Finished { .. }) {
        clean_finished_quarantine(profile, &mut journal, &quarantine)?;
        return Ok(Startup::Idle);
    }
    if matches!(journal.phase, Phase::Staged) {
        let prepare = || {
            if restore_pending {
                return Err("A library restore was already staged. Finish that restore and retry the factory reset.".into());
            }
            let entries = inventory(profile)?;
            ensure_quarantine(&quarantine)?;
            if fs::read_dir(&quarantine)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
            {
                return Err(
                    "Unexpected files are present in the factory reset work folder.".into(),
                );
            }
            Ok(entries)
        };
        let entries = match prepare() {
            Ok(entries) => entries,
            Err(warning) => return fail(profile, &mut journal, warning),
        };
        journal.phase = Phase::Moving { entries };
        write_journal(profile, &journal, false)?;
    }
    if let Phase::Moving { ref entries } = journal.phase {
        require_quarantine(&quarantine)?;
        let result = if restore_pending {
            Err("A library restore is pending.".into())
        } else {
            move_entries(profile, &quarantine, entries)
        };
        if let Err(error) = result {
            rollback_entries(profile, &quarantine, entries).map_err(|rollback_error| {
                format!("Factory reset stopped and could not restore the previous profile: {error}. {rollback_error}. Keep {} and contact support.", quarantine.display())
            })?;
            let _ = fs::remove_dir(&quarantine);
            return fail(profile, &mut journal, error);
        }
        journal.phase = Phase::Applied;
        write_journal(profile, &journal, false)?;
    }
    Ok(if matches!(journal.phase, Phase::Applied) {
        require_quarantine(&quarantine)?;
        Startup::ApplyCredentials { quarantine }
    } else {
        Startup::Idle
    })
}

fn fail(profile: &Path, journal: &mut Journal, warning: String) -> Result<Startup, String> {
    journal.phase = Phase::Failed {
        warning: format!(
            "Burrata kept the previous profile. Factory reset could not proceed: {warning}"
        ),
    };
    write_journal(profile, journal, false)?;
    Ok(Startup::Idle)
}

pub(crate) fn finish(profile: &Path, warnings: Vec<String>) -> Result<(), String> {
    let mut journal = read_journal(profile)?.ok_or("Factory reset startup marker is missing.")?;
    if !matches!(journal.phase, Phase::Applied) {
        return Err("Factory reset startup marker is in an unexpected state.".into());
    }
    let quarantine = quarantine_dir(profile, journal.id);
    require_quarantine(&quarantine)?;
    // Mark the new profile before deleting old data. Future starts can retry
    // this cleanup without moving fresh files or clearing newly saved logins.
    journal.phase = Phase::Finished {
        warning: joined_warning(&warnings),
    };
    write_journal(profile, &journal, false)?;
    clean_finished_quarantine(profile, &mut journal, &quarantine)
}

fn clean_finished_quarantine(
    profile: &Path,
    journal: &mut Journal,
    quarantine: &Path,
) -> Result<(), String> {
    if metadata(quarantine)?.is_none() {
        return Ok(());
    }
    let cleanup = || {
        require_quarantine(quarantine)?;
        let expected = inventory(quarantine)?;
        let actual = fs::read_dir(quarantine)
            .and_then(|entries| entries.collect::<std::io::Result<Vec<_>>>())
            .map_err(|error| error.to_string())?;
        if actual.len() != expected.len() {
            return Err("The work folder contains unexpected files.".into());
        }
        fs::remove_dir_all(quarantine).map_err(|error| error.to_string())
    };
    if let Err(error) = cleanup() {
        let Phase::Finished { warning } = &mut journal.phase else {
            unreachable!()
        };
        let message = format!(
            "The reset completed, but Burrata could not remove the old profile data in {}: {error}",
            quarantine.display()
        );
        if warning
            .as_ref()
            .is_none_or(|value| !value.contains(&message))
        {
            *warning = joined_warning(
                &warning
                    .take()
                    .into_iter()
                    .chain([message])
                    .collect::<Vec<_>>(),
            );
            write_journal(profile, journal, false)?;
        }
    }
    Ok(())
}

fn joined_warning(warnings: &[String]) -> Option<String> {
    (!warnings.is_empty()).then(|| warnings.join(" "))
}

pub(crate) fn status(profile: &Path) -> Result<Status, String> {
    let Some(journal) = read_journal(profile)? else {
        return Ok(Status::default());
    };
    Ok(match journal.phase {
        Phase::Finished { warning } => Status {
            reset_id: Some(journal.id),
            warning,
        },
        Phase::Failed { warning } => Status {
            reset_id: None,
            warning: Some(warning),
        },
        _ => Status::default(),
    })
}

pub(crate) fn acknowledge(profile: &Path, reset_id: Uuid) -> Result<(), String> {
    let Some(journal) = read_journal(profile)? else {
        return Ok(());
    };
    if journal.id != reset_id || !matches!(journal.phase, Phase::Finished { .. }) {
        return Err("The factory reset receipt no longer matches this startup.".into());
    }
    unlink_journal(profile)
}

fn unlink_journal(profile: &Path) -> Result<(), String> {
    fs::remove_file(profile.join(JOURNAL))
        .map_err(|error| format!("Cannot remove factory reset marker: {error}"))?;
    sync_directory(profile).map_err(|error| format!("Cannot sync factory reset marker: {error}"))
}

fn read_journal(profile: &Path) -> Result<Option<Journal>, String> {
    let path = profile.join(JOURNAL);
    let Some(metadata) = metadata(&path)? else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        return Err("Factory reset marker has an unexpected file type or size.".into());
    }
    let mut raw = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut raw))
        .map_err(|error| format!("Cannot read factory reset marker: {error}"))?;
    if raw.len() as u64 > MAX_JOURNAL_BYTES {
        return Err("Factory reset marker is too large.".into());
    }
    let journal: Journal = serde_json::from_slice(&raw)
        .map_err(|_| "Factory reset marker is invalid; keep it and contact support.".to_owned())?;
    if journal.version != 1 || journal.id.is_nil() {
        return Err("Factory reset marker is incompatible; keep it and contact support.".into());
    }
    if let Phase::Moving { entries } = &journal.phase {
        let mut seen = std::collections::HashSet::new();
        if entries.iter().any(|name| {
            let name = name.as_str();
            (!FILES.contains(&name) && !DIRECTORIES.contains(&name)) || !seen.insert(name)
        }) {
            return Err("Factory reset marker contains invalid profile entries.".into());
        }
    }
    Ok(Some(journal))
}

fn write_journal(profile: &Path, journal: &Journal, new: bool) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        let mut file = NamedTempFile::new_in(profile)?;
        serde_json::to_writer(&mut file, journal)?;
        file.flush()?;
        file.as_file().sync_all()?;
        let target = profile.join(JOURNAL);
        if new {
            file.persist_noclobber(target)?;
        } else {
            file.persist(target)?;
        }
        sync_directory(profile)
    };
    write().map_err(|error| format!("Cannot write the factory reset marker: {error}"))
}

fn quarantine_dir(profile: &Path, id: Uuid) -> PathBuf {
    profile.join(format!(".burrata-factory-reset-{id}"))
}

fn ensure_quarantine(path: &Path) -> Result<(), String> {
    if metadata(path)?.is_none() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .map_err(|e| format!("Cannot create factory reset work folder: {e}"))?;
    }
    require_quarantine(path)
}

fn require_quarantine(path: &Path) -> Result<(), String> {
    if metadata(path)?.is_some_and(|meta| meta.is_dir()) {
        Ok(())
    } else {
        Err("Factory reset work folder is missing or has an unexpected file type.".into())
    }
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not check {}: {error}", path.display())),
    }
}

fn inventory(profile: &Path) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    for (names, directory) in [(FILES, false), (DIRECTORIES, true)] {
        for &name in names {
            let path = profile.join(name);
            let Some(meta) = metadata(&path)? else {
                continue;
            };
            if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
                return Err(format!("Burrata's {name} has an unexpected file type. Restore the regular profile entry and retry."));
            }
            if directory {
                protect_user_files(&path)?;
            }
            entries.push(name.to_owned());
        }
    }
    Ok(entries)
}

fn protect_user_files(directory: &Path) -> Result<(), String> {
    let mut folders = vec![directory.to_path_buf()];
    while let Some(path) = folders.pop() {
        let entries = fs::read_dir(path).map_err(|e| format!("Cannot check reset folder: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let archive = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("burrata-backup") || ext.eq_ignore_ascii_case("zip")
                });
            if archive || crate::is_supported_import_audio(&path) {
                return Err(format!("Move the music file or backup outside Burrata's reset folders before retrying: {}", path.display()));
            }
            if kind.is_dir() {
                folders.push(path);
            }
        }
    }
    Ok(())
}

fn move_entries(profile: &Path, quarantine: &Path, entries: &[String]) -> Result<(), String> {
    for name in entries {
        let source = profile.join(name);
        let dest = quarantine.join(name);
        match (metadata(&source)?, metadata(&dest)?) {
            (Some(meta), None) if FILES.contains(&name.as_str()) && meta.is_file() => rename(&source, &dest)?,
            (Some(meta), None) if DIRECTORIES.contains(&name.as_str()) && meta.is_dir() => {
                protect_user_files(&source)?;
                rename(&source, &dest)?;
            }
            (None, Some(meta)) if FILES.contains(&name.as_str()) && meta.is_file() => {},
            (None, Some(meta)) if DIRECTORIES.contains(&name.as_str()) && meta.is_dir() => protect_user_files(&dest)?,
            _ => return Err(format!("Burrata's {name} changed while resetting. Close other programs that use the profile and retry.")),
        }
    }
    sync_reset_dirs(profile, quarantine)
}

fn rollback_entries(profile: &Path, quarantine: &Path, entries: &[String]) -> Result<(), String> {
    for name in entries.iter().rev() {
        let source = quarantine.join(name);
        if metadata(&source)?.is_none() {
            continue;
        }
        let dest = profile.join(name);
        if metadata(&dest)?.is_some() {
            return Err(format!(
                "Cannot restore {name} without overwriting another file."
            ));
        }
        rename(&source, &dest)?;
    }
    sync_reset_dirs(profile, quarantine)
}

fn rename(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("Could not move {}: {error}", source.display()))
}

fn sync_reset_dirs(profile: &Path, quarantine: &Path) -> Result<(), String> {
    sync_directory(profile)
        .and_then(|_| sync_directory(quarantine))
        .map_err(|error| format!("Could not sync factory reset files: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_confirmation_must_be_exact() {
        assert!(validate_confirmation("RESET").is_ok());
        for value in ["Reset", "reset", " RESET", "RESET ", ""] {
            assert!(validate_confirmation(value).is_err());
        }
    }

    #[test]
    fn startup_moves_only_owned_state_without_a_backup_and_requires_matching_ui_receipt() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path();
        for file in [
            "burrata.sqlite",
            "burrata.sqlite-wal",
            "playback-history.sqlite",
            "automation.json",
        ] {
            fs::write(profile.join(file), "old Burrata state").unwrap();
        }
        fs::create_dir(profile.join("managed-provider-session")).unwrap();
        fs::write(profile.join("managed-provider-session/session.json"), "{}").unwrap();
        fs::create_dir(profile.join("slskd")).unwrap();
        fs::write(profile.join("slskd/transfer.sqlite"), "other application").unwrap();
        fs::create_dir(profile.join("Downloads")).unwrap();
        fs::write(profile.join("Downloads/song.opus"), "song").unwrap();
        fs::write(profile.join("my-backup.burrata-backup"), "archive").unwrap();

        stage(profile, &[profile.join("Downloads")]).unwrap();
        assert!(profile.join("burrata.sqlite").exists());
        assert!(stage(profile, &[]).is_err());
        let Startup::ApplyCredentials { quarantine } = prepare_startup(profile, false).unwrap()
        else {
            panic!("expected a new profile")
        };
        assert!(!profile.join("burrata.sqlite").exists());
        assert!(!profile.join("burrata.sqlite-wal").exists());
        assert!(!profile.join("playback-history.sqlite").exists());
        assert!(!profile.join("managed-provider-session").exists());
        assert!(quarantine
            .join("managed-provider-session/session.json")
            .is_file());
        assert_eq!(
            fs::read(profile.join("slskd/transfer.sqlite")).unwrap(),
            b"other application"
        );
        assert_eq!(
            fs::read(profile.join("Downloads/song.opus")).unwrap(),
            b"song"
        );
        assert_eq!(
            fs::read(profile.join("my-backup.burrata-backup")).unwrap(),
            b"archive"
        );

        // A crash after creating the new database must not remove it again.
        fs::write(profile.join("burrata.sqlite"), "fresh").unwrap();
        assert!(matches!(
            prepare_startup(profile, false).unwrap(),
            Startup::ApplyCredentials { .. }
        ));
        assert_eq!(fs::read(profile.join("burrata.sqlite")).unwrap(), b"fresh");
        finish(profile, Vec::new()).unwrap();
        assert!(!quarantine.exists());
        let token = status(profile).unwrap().reset_id.unwrap();
        assert!(acknowledge(profile, Uuid::new_v4()).is_err());
        assert!(matches!(
            prepare_startup(profile, false).unwrap(),
            Startup::Idle
        ));
        acknowledge(profile, token).unwrap();
        assert!(status(profile).unwrap().reset_id.is_none());
        assert_eq!(fs::read(profile.join("burrata.sqlite")).unwrap(), b"fresh");
    }

    #[test]
    fn a_failed_interrupted_move_rolls_back_and_reports_what_to_retry() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path();
        fs::write(profile.join("burrata.sqlite"), "recover me").unwrap();
        let id = Uuid::new_v4();
        let quarantine = quarantine_dir(profile, id);
        ensure_quarantine(&quarantine).unwrap();
        rename(
            &profile.join("burrata.sqlite"),
            &quarantine.join("burrata.sqlite"),
        )
        .unwrap();
        write_journal(
            profile,
            &Journal {
                id,
                version: 1,
                phase: Phase::Moving {
                    entries: vec!["burrata.sqlite".into(), "playback-history.sqlite".into()],
                },
            },
            true,
        )
        .unwrap();
        assert!(matches!(
            prepare_startup(profile, false).unwrap(),
            Startup::Idle
        ));
        assert_eq!(
            fs::read(profile.join("burrata.sqlite")).unwrap(),
            b"recover me"
        );
        assert!(!quarantine.exists());
        let report = status(profile).unwrap();
        assert!(report.reset_id.is_none());
        assert!(report
            .warning
            .unwrap()
            .contains("kept the previous profile"));
        stage(profile, &[]).unwrap();
    }

    #[test]
    fn protected_destinations_and_archives_inside_managed_directories_block_the_reset() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path();
        let managed = profile.join("managed-provider-session");
        fs::create_dir(&managed).unwrap();
        let backup = managed.join("backups");
        assert!(stage(profile, &[backup]).is_err());
        fs::write(managed.join("keep.burrata-backup"), "archive").unwrap();
        assert!(stage(profile, &[]).is_err());
        assert!(!profile.join(JOURNAL).exists());
        fs::remove_file(managed.join("keep.burrata-backup")).unwrap();
        stage(profile, &[]).unwrap();
        // A user file can appear between consent and the cold-start pass.
        fs::write(managed.join("keep.flac"), "song").unwrap();
        assert!(matches!(
            prepare_startup(profile, false).unwrap(),
            Startup::Idle
        ));
        assert!(status(profile)
            .unwrap()
            .warning
            .unwrap()
            .contains("keep.flac"));
        assert_eq!(fs::read(managed.join("keep.flac")).unwrap(), b"song");
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_profile_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("third-party.db");
        fs::write(&external, "other").unwrap();
        std::os::unix::fs::symlink(&external, root.path().join("burrata.sqlite")).unwrap();
        assert!(stage(root.path(), &[]).is_err());
        assert_eq!(fs::read(external).unwrap(), b"other");
    }
}
