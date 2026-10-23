//! Ownership-marked verification transfers and conservative startup scavenging.
use crate::models::DownloadJob;
use std::{collections::HashSet, io::Write, path::Path};
const MARKER: &str = ".burrata-verification-owner.json";
const OWNER: &str = "burrata.staged-verification.v1";

pub(crate) fn create(parent: &Path, job_id: &str) -> Result<tempfile::TempDir, String> {
    let id = uuid::Uuid::parse_str(job_id).map_err(|_| "Invalid verification job identity")?;
    if id.to_string() != job_id {
        return Err("Invalid verification job identity".into());
    }
    let owned = tempfile::Builder::new()
        .prefix(&format!("verification-{job_id}-"))
        .tempdir_in(parent)
        .map_err(|_| "Cannot create verification staging directory")?;
    let folder = owned
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid verification staging directory")?;
    let marker = serde_json::json!({"owner":OWNER,"jobId":job_id,"directory":folder});
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(owned.path().join(MARKER))
        .map_err(|_| "Cannot mark verification staging ownership")?;
    file.write_all(marker.to_string().as_bytes())
        .map_err(|_| "Cannot mark verification staging ownership")?;
    file.sync_all()
        .map_err(|_| "Cannot persist verification staging ownership")?;
    Ok(owned)
}

pub(crate) fn cleanup_interrupted(music: &Path, jobs: &[DownloadJob]) -> Result<usize, String> {
    let root = music.join(".burrata-staging");
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err("Cannot inspect verification staging root".into()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(0);
    }
    let music = music
        .canonicalize()
        .map_err(|_| "Cannot inspect music directory")?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "Cannot inspect verification staging root")?;
    if canonical_root.parent() != Some(music.as_path()) {
        return Ok(0);
    }
    let eligible: HashSet<&str> = jobs
        .iter()
        .filter(|job| {
            ["interrupted", "failed", "cancelled", "completed"].contains(&job.state.as_str())
        })
        .map(|job| job.id.as_str())
        .collect();
    let mut removed = 0;
    for entry in
        std::fs::read_dir(&canonical_root).map_err(|_| "Cannot list verification staging")?
    {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let Some(folder) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !folder.starts_with("verification-") {
            continue;
        }
        let marker_path = path.join(MARKER);
        let Ok(metadata) = std::fs::symlink_metadata(&marker_path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&marker_path) else {
            continue;
        };
        let Ok(marker) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(job_id) = marker["jobId"].as_str() else {
            continue;
        };
        if marker["owner"] != OWNER || marker["directory"] != folder || !eligible.contains(job_id) {
            continue;
        }
        let Ok(id) = uuid::Uuid::parse_str(job_id) else {
            continue;
        };
        if id.to_string() != job_id || !folder.starts_with(&format!("verification-{job_id}-")) {
            continue;
        }
        if path.canonicalize().ok().as_deref().and_then(Path::parent)
            != Some(canonical_root.as_path())
        {
            continue;
        }
        std::fs::remove_dir_all(&path)
            .map_err(|_| "Cannot remove abandoned verification transfer")?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(id: &str, state: &str) -> DownloadJob {
        serde_json::from_value(serde_json::json!({"id":id,"state":state,"track":crate::models::Track::default(),
            "candidate":crate::models::Candidate::default(),"progress":0,"error":null,"outputPath":null,
            "createdAt":"","outputProfile":"source"})).unwrap()
    }
    #[test]
    fn removes_only_marked_known_inactive_transfers_and_preserves_unknown_and_active_files() {
        let music = tempfile::tempdir().unwrap();
        let root = music.path().join(".burrata-staging");
        std::fs::create_dir(&root).unwrap();
        let old_id = uuid::Uuid::new_v4().to_string();
        let active_id = uuid::Uuid::new_v4().to_string();
        let old = create(&root, &old_id).unwrap().keep();
        std::fs::write(old.join("transfer.flac"), b"synthetic").unwrap();
        let active = create(&root, &active_id).unwrap().keep();
        let unknown = create(&root, &uuid::Uuid::new_v4().to_string())
            .unwrap()
            .keep();
        let unmarked = root.join("verification-user-folder");
        std::fs::create_dir(&unmarked).unwrap();
        std::fs::write(unmarked.join("user.flac"), b"preserve").unwrap();
        let library = music.path().join("library.flac");
        std::fs::write(&library, b"preserve").unwrap();
        assert_eq!(
            cleanup_interrupted(
                music.path(),
                &[job(&old_id, "interrupted"), job(&active_id, "identifying")]
            )
            .unwrap(),
            1
        );
        assert!(!old.exists());
        assert!(active.exists());
        assert!(unknown.exists());
        assert!(unmarked.join("user.flac").exists());
        assert_eq!(std::fs::read(library).unwrap(), b"preserve");
    }
    #[cfg(unix)]
    #[test]
    fn does_not_follow_staging_or_marker_symlinks() {
        let music = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = music.path().join(".burrata-staging");
        std::os::unix::fs::symlink(outside.path(), &root).unwrap();
        assert_eq!(cleanup_interrupted(music.path(), &[]).unwrap(), 0);
        std::fs::remove_file(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = create(&root, &id).unwrap().keep();
        let marker = path.join(MARKER);
        let copy = outside.path().join("marker.json");
        std::fs::copy(&marker, &copy).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink(&copy, &marker).unwrap();
        assert_eq!(
            cleanup_interrupted(music.path(), &[job(&id, "interrupted")]).unwrap(),
            0
        );
        assert!(path.exists());
        assert!(copy.exists());
        // A link inside an otherwise owned abandoned transfer is removed as
        // a link; cleanup must never traverse into its target directory.
        std::fs::remove_file(&marker).unwrap();
        std::fs::copy(&copy, &marker).unwrap();
        let preserved = outside.path().join("library.flac");
        std::fs::write(&preserved, b"preserve outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), path.join("external-link")).unwrap();
        assert_eq!(
            cleanup_interrupted(music.path(), &[job(&id, "interrupted")]).unwrap(),
            1
        );
        assert_eq!(std::fs::read(preserved).unwrap(), b"preserve outside");
        assert!(copy.exists());
    }
}
