//! Session-local change journal and bounded metadata fingerprints for playlist sync.
//! TEMP objects never enter backups and invalidate automatically on a cold start.
use crate::playlist_sync::PlaylistSyncStatus;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::Path, time::Instant};

#[derive(Default)]
pub(crate) struct Cache {
    pub pass: Option<(String, PlaylistSyncStatus)>,
    pub mirrors: HashMap<String, String>,
    pub verified_at: Option<Instant>,
    pub mirror_work: usize,
    pub noops: usize,
}
#[derive(Default)]
pub(crate) struct Revisions {
    pub generation: i64,
    pub external: i64,
}

pub(crate) fn install(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TEMP TABLE playlist_sync_epoch(id INTEGER PRIMARY KEY,generation INTEGER NOT NULL,global INTEGER NOT NULL);
        INSERT INTO playlist_sync_epoch VALUES(1,0,0);
        CREATE TEMP TABLE playlist_sync_dirty(collection_id TEXT PRIMARY KEY,revision INTEGER NOT NULL);")
        .map_err(|e| e.to_string())?;
    let bump = "UPDATE playlist_sync_epoch SET generation=generation+1 WHERE id=1;";
    let mark = |select: &str| {
        format!("INSERT INTO playlist_sync_dirty SELECT collection_id,(SELECT generation FROM playlist_sync_epoch WHERE id=1) FROM ({select}) WHERE collection_id IS NOT NULL ON CONFLICT(collection_id) DO UPDATE SET revision=excluded.revision;")
    };
    for table in [
        "collection_members",
        "playlist_ordered_entries",
        "collections",
        "recordings",
        "catalog_listings",
        "local_assets",
        "asset_analyses",
        "asset_autotags",
        "browser_sources",
        "chart_snapshots",
        "settings",
    ] {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            let refs = if event == "INSERT" {
                vec!["NEW"]
            } else if event == "DELETE" {
                vec!["OLD"]
            } else {
                vec!["OLD", "NEW"]
            };
            let mut body = String::from(bump);
            let mut condition = String::new();
            match table {
                "collection_members" | "playlist_ordered_entries" => {
                    for row in &refs {
                        body.push_str(&mark(&format!(
                            "SELECT {row}.collection_id AS collection_id"
                        )));
                    }
                }
                "collections" => {
                    for row in &refs {
                        body.push_str(&mark(&format!("SELECT {row}.id AS collection_id")));
                    }
                    if event == "DELETE" {
                        body.push_str(
                            "DELETE FROM playlist_sync_dirty WHERE collection_id=OLD.id;",
                        );
                    }
                }
                "recordings" | "catalog_listings" | "local_assets" | "asset_analyses"
                | "asset_autotags" => {
                    if event == "UPDATE" {
                        condition = match table {
                            "recordings" => " WHEN OLD.manual_metadata IS NOT NEW.manual_metadata OR OLD.favorite IS NOT NEW.favorite OR OLD.version IS NOT NEW.version".into(),
                            "catalog_listings" => " WHEN OLD.metadata IS NOT NEW.metadata OR OLD.recording_id IS NOT NEW.recording_id OR OLD.source_url IS NOT NEW.source_url".into(),
                            "local_assets" => " WHEN OLD.payload IS NOT NEW.payload OR OLD.path IS NOT NEW.path OR OLD.recording_id IS NOT NEW.recording_id".into(),
                            _ => " WHEN OLD.payload IS NOT NEW.payload OR OLD.status IS NOT NEW.status".into(),
                        };
                    }
                    let mut affected = Vec::new();
                    for row in &refs {
                        let recording = if table == "recordings" {
                            format!("{row}.id")
                        } else if table == "asset_analyses" || table == "asset_autotags" {
                            format!(
                                "(SELECT recording_id FROM local_assets WHERE id={row}.asset_id)"
                            )
                        } else {
                            format!("{row}.recording_id")
                        };
                        let select = format!("SELECT collection_id FROM collection_members WHERE recording_id={recording}
                            UNION SELECT e.collection_id FROM playlist_ordered_entries e JOIN catalog_listings l ON l.id=e.listing_id WHERE l.recording_id={recording}
                            UNION SELECT 'favorites' AS collection_id WHERE EXISTS(SELECT 1 FROM recordings WHERE id={recording} AND favorite=1)");
                        body.push_str(&mark(&select));
                        affected.push(select);
                    }
                    // Removing favorite status must also invalidate the old favorites mirror.
                    if table == "recordings" {
                        let favorite = refs
                            .iter()
                            .map(|row| format!("{row}.favorite=1"))
                            .collect::<Vec<_>>()
                            .join(" OR ");
                        let select =
                            format!("SELECT 'favorites' AS collection_id WHERE {favorite}");
                        body.push_str(&mark(&select));
                        affected.push(select);
                    }
                    let relevant = format!("EXISTS(SELECT 1 FROM ({}))", affected.join(" UNION "));
                    condition = if condition.is_empty() {
                        format!(" WHEN {relevant}")
                    } else {
                        format!(
                            " WHEN ({}) AND {relevant}",
                            condition.trim_start_matches(" WHEN ")
                        )
                    };
                }
                "browser_sources" => {
                    for row in &refs {
                        body.push_str(&mark(&format!(
                            "SELECT {row}.collection_id AS collection_id"
                        )));
                    }
                }
                "chart_snapshots" => {
                    for row in &refs {
                        body.push_str(&mark(&format!("SELECT collection_id FROM browser_sources WHERE provider={row}.provider AND source_url={row}.source_url")));
                    }
                }
                "settings" => {
                    let row = if event == "DELETE" { "OLD" } else { "NEW" };
                    // Transport, playback bookmarks, caches and volume changes do not dirty playlists.
                    condition = format!(" WHEN {row}.key IN ('playlistMirrorOptions.v1','playlistHierarchy.v1','sidebarLayout.v1','playlistPreferences.v1','playlistHistoryMirror.v1','includeRadioSongsInPlaybackHistory.v1')");
                    if event == "UPDATE" {
                        condition.push_str(" OR (NEW.key='application' AND (json_extract(OLD.value,'$.musicDirectory') IS NOT json_extract(NEW.value,'$.musicDirectory') OR json_extract(OLD.value,'$.playlistDirectories') IS NOT json_extract(NEW.value,'$.playlistDirectories') OR json_extract(OLD.value,'$.playlistMirroringEnabled') IS NOT json_extract(NEW.value,'$.playlistMirroringEnabled')))
                            OR (NEW.key='playlistSync.v1' AND json_extract(OLD.value,'$.writesPaused') IS NOT json_extract(NEW.value,'$.writesPaused'))");
                    } else {
                        condition.push_str(&format!(
                            " OR {row}.key='application' OR {row}.key='playlistSync.v1'"
                        ));
                    }
                    condition.push_str(&format!(" OR {row}.key GLOB 'preferredAsset.*'"));
                    body.push_str(&format!("UPDATE playlist_sync_epoch SET global=generation WHERE id=1 AND {row}.key NOT GLOB 'preferredAsset.*';"));
                    body.push_str(&mark(&format!("SELECT collection_id FROM collection_members WHERE recording_id=substr({row}.key,16)
                        UNION SELECT e.collection_id FROM playlist_ordered_entries e JOIN catalog_listings l ON l.id=e.listing_id WHERE l.recording_id=substr({row}.key,16)
                        UNION SELECT 'favorites' WHERE EXISTS(SELECT 1 FROM recordings WHERE id=substr({row}.key,16) AND favorite=1)")));
                }
                _ => unreachable!(),
            }
            conn.execute_batch(&format!("CREATE TEMP TRIGGER playlist_sync_{}_{} AFTER {event} ON main.{table}{condition} BEGIN {body} END;", table, event.to_lowercase())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(crate) fn revisions(conn: &Connection) -> Result<Revisions, String> {
    let generation = conn
        .query_row(
            "SELECT generation FROM playlist_sync_epoch WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let external = conn
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    Ok(Revisions {
        generation,
        external,
    })
}

pub(crate) fn stamp_path(hash: &mut Sha256, path: &Path) {
    hash.update(path.to_string_lossy().as_bytes());
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            hash.update(metadata.len().to_le_bytes());
            hash.update(format!("{:?}:{:?}", metadata.modified(), metadata.created()).as_bytes());
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                hash.update(metadata.ctime().to_le_bytes());
                hash.update(metadata.ctime_nsec().to_le_bytes());
            }
        }
        Err(error) => hash.update(format!("{:?}", error.kind()).as_bytes()),
    }
}

/// Walk metadata only. Never read audio, playlist bodies, or immutable archives.
pub(crate) fn fingerprint(
    roots: &[std::path::PathBuf],
    archive_root: Option<&Path>,
) -> Result<String, String> {
    fn walk(
        hash: &mut Sha256,
        path: &Path,
        archive: Option<&Path>,
        depth: usize,
        count: &mut usize,
    ) -> Result<(), String> {
        if depth > 8 {
            return Ok(());
        }
        *count += 1;
        if *count > 100_000 {
            return Err("Playlist metadata scan exceeded its safety limit".into());
        }
        let Ok(metadata) = fs::symlink_metadata(path) else {
            stamp_path(hash, path);
            return Ok(());
        };
        let in_archive = archive.is_some_and(|archive| path.starts_with(archive));
        if in_archive
            && metadata.is_file()
            && archive
                .and_then(Path::parent)
                .is_some_and(|root| crate::playlist_files::is_legacy_burrata_playlist(path, root))
        {
            return Ok(());
        }
        // Immutable archive writes must not wake current playlist work. Still
        // discover user-authored files placed beside generated archives.
        if in_archive && metadata.is_dir() {
            hash.update(path.to_string_lossy().as_bytes());
        } else {
            stamp_path(hash, path);
        }
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Ok(());
        }
        let Ok(entries) = fs::read_dir(path) else {
            return Ok(());
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    || entry
                        .path()
                        .extension()
                        .and_then(|v| v.to_str())
                        .is_some_and(|v| {
                            v.eq_ignore_ascii_case("m3u") || v.eq_ignore_ascii_case("m3u8")
                        })
            })
            .take(100_001)
            .collect::<Vec<_>>();
        if entries.len() > 100_000 {
            return Err("Playlist metadata scan exceeded its safety limit".into());
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir()
                || child.extension().and_then(|v| v.to_str()).is_some_and(|v| {
                    v.eq_ignore_ascii_case("m3u") || v.eq_ignore_ascii_case("m3u8")
                })
            {
                walk(hash, &child, archive, depth + 1, count)?;
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    let mut count = 0;
    let archive_root =
        archive_root.map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
    let mut roots = roots
        .iter()
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect::<Vec<_>>();
    roots.sort_by_key(|path| path.components().count());
    let mut scanned = Vec::<std::path::PathBuf>::new();
    for root in roots {
        if scanned.iter().any(|parent| root.starts_with(parent)) {
            continue;
        }
        walk(&mut hash, &root, archive_root.as_deref(), 0, &mut count)?;
        scanned.push(root);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_fingerprint_ignores_generated_archives_but_detects_user_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Playlists");
        let archive = root.join("Archives");
        let owner = archive
            .join("Personal")
            .join("00000000-0000-4000-8000-000000000000");
        fs::create_dir_all(&owner).unwrap();
        let generated = owner.join(format!("20261028000000-{}.m3u8", "a".repeat(64)));
        fs::write(&generated, b"original").unwrap();
        let before = fingerprint(&[root.clone()], Some(&archive)).unwrap();
        fs::write(&generated, b"changed archive body").unwrap();
        assert_eq!(
            fingerprint(&[root.clone()], Some(&archive)).unwrap(),
            before
        );
        let manual = archive.join("user-set.m3u8");
        fs::write(&manual, b"#EXTM3U\n").unwrap();
        let added = fingerprint(&[root.clone()], Some(&archive)).unwrap();
        assert_ne!(added, before);
        fs::write(&manual, b"#EXTM3U\nhttps://example.invalid/track\n").unwrap();
        assert_ne!(fingerprint(&[root], Some(&archive)).unwrap(), added);
    }
}
