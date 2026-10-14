//! Discovery and import of user-configured local M3U/M3U8 playlist files.
//!
//! Playlist text is treated as an index into already registered local assets.
//! Entry paths are never opened or imported as arbitrary files.
use crate::{
    models::{Candidate, Collection, Settings, Track},
    storage::Store,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

type Result<T> = std::result::Result<T, String>;
const MAX_PLAYLIST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PLAYLIST_ENTRIES: usize = 10_000;
const MAX_SCAN_DEPTH: usize = 8;
const MAX_PLAYLISTS: usize = 5000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlaylistFile {
    pub path: String,
    pub name: String,
    pub modified: Option<String>,
    pub track_count: usize,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlaylistImport {
    pub collection: Collection,
    pub imported_count: usize,
    pub unmatched_count: usize,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlaylistScan {
    pub files: Vec<LocalPlaylistFile>,
    pub roots: Vec<PlaylistRootStatus>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistRootStatus {
    pub path: String,
    pub available: bool,
    pub readable: bool,
}

/// Rich scan result for settings and sync UI. It includes explicit configured roots,
/// the standard ~/Music root, and diagnostics when a root is unavailable or unreadable.
pub fn scan_local_playlists(settings: &Settings) -> Result<LocalPlaylistScan> {
    let roots = effective_directories(settings)
        .into_iter()
        .map(|path| {
            let path_text = path.to_string_lossy().into_owned();
            let canonical = fs::canonicalize(&path).ok().filter(|p| p.is_dir());
            let available = canonical.is_some();
            let readable = canonical.as_ref().is_some_and(|p| fs::read_dir(p).is_ok());
            PlaylistRootStatus {
                path: path_text,
                available,
                readable,
            }
        })
        .collect::<Vec<_>>();
    let warnings = roots
        .iter()
        .filter_map(|root| {
            if !root.available {
                Some(format!("Playlist directory is unavailable: {}", root.path))
            } else if !root.readable {
                Some(format!("Playlist directory is unreadable: {}", root.path))
            } else {
                None
            }
        })
        .collect();
    Ok(LocalPlaylistScan {
        files: list_local_playlists(settings)?,
        roots,
        warnings,
    })
}

/// Scan only configured roots. Symlinks are not followed, nesting is bounded,
/// and results have a stable, case-insensitive path order.
pub fn list_local_playlists(settings: &Settings) -> Result<Vec<LocalPlaylistFile>> {
    let mut files = Vec::new();
    for configured in effective_directories(settings) {
        let root = match fs::canonicalize(configured) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => continue,
            Err(_) => continue,
        };
        scan_directory(&root, &root, 0, &mut files)?;
        if files.len() > MAX_PLAYLISTS {
            return Err("Playlist scan exceeded 5,000 files".into());
        }
    }
    if !settings.music_directory.trim().is_empty() {
        let configured_mirror_root = Path::new(&settings.music_directory).join("Playlists");
        let mirror_root =
            fs::canonicalize(&configured_mirror_root).unwrap_or(configured_mirror_root);
        files.retain(|file| !Path::new(&file.path).starts_with(&mirror_root));
    }
    files.sort_by(|a, b| {
        a.path
            .to_lowercase()
            .cmp(&b.path.to_lowercase())
            .then(a.path.cmp(&b.path))
    });
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<LocalPlaylistFile>,
) -> Result<()> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    let mut entries = entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            scan_directory(root, &path, depth + 1, files)?;
        } else if file_type.is_file() && is_playlist(&path) {
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let (count, warning) = if metadata.len() > MAX_PLAYLIST_BYTES {
                (0, Some("Playlist exceeds 2 MB".into()))
            } else {
                match fs::read(&path) {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => {
                            let count = playlist_entry_count(&text);
                            (
                                count,
                                (count > MAX_PLAYLIST_ENTRIES)
                                    .then(|| "Playlist exceeds the 10,000 entry limit".into()),
                            )
                        }
                        Err(_) => (0, Some("Playlist is not valid UTF-8".into())),
                    },
                    Err(error) => (0, Some(format!("Playlist is unreadable: {error}"))),
                }
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| {
                    chrono::DateTime::<chrono::Utc>::from(UNIX_EPOCH + duration).to_rfc3339()
                });
            files.push(LocalPlaylistFile {
                path: path.to_string_lossy().into_owned(),
                name: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                modified,
                track_count: count,
                warning: warning
                    .or_else(|| (count == 0).then(|| "Playlist has no entry lines".into())),
            });
            if files.len() > MAX_PLAYLISTS {
                return Err("Playlist scan exceeded 5,000 files".into());
            }
            let canonical = fs::canonicalize(&path).map_err(|e| e.to_string())?;
            if !canonical.starts_with(root) {
                return Err("Playlist file escaped its configured directory".into());
            }
        }
    }
    Ok(())
}

fn is_playlist(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
}

fn playlist_entry_count(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .count()
}

/// Import a playlist by matching its entries to existing registered local
/// assets. The playlist itself must be inside a configured playlist directory.
pub fn import_local_playlist(
    settings: &Settings,
    store: &Store,
    requested_path: &str,
) -> Result<LocalPlaylistImport> {
    let path = Path::new(requested_path);
    if !is_playlist(path) {
        return Err("Choose an M3U or M3U8 playlist".into());
    }
    let canonical_file = fs::canonicalize(path).map_err(|_| "Playlist file is unavailable")?;
    let roots = configured_roots(settings);
    if !roots.iter().any(|root| canonical_file.starts_with(root)) {
        return Err("Playlist file must be inside a configured playlist directory".into());
    }
    let metadata = fs::metadata(&canonical_file).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_PLAYLIST_BYTES {
        return Err("Playlist must be a file no larger than 2 MB".into());
    }
    let bytes = fs::read(&canonical_file).map_err(|e| e.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "Playlist must be UTF-8 text")?;
    if playlist_entry_count(text) > MAX_PLAYLIST_ENTRIES {
        return Err("Playlist import is limited to 10,000 entries".into());
    }
    let assets = store.list_assets()?;
    let mut by_path: HashMap<PathBuf, Track> = HashMap::new();
    let music_roots = approved_audio_roots(settings);
    for asset in assets {
        if let Ok(real) = fs::canonicalize(&asset.path) {
            if real.is_file() && music_roots.iter().any(|root| real.starts_with(root)) {
                by_path.insert(real, asset.track);
            }
        }
    }
    let mut tracks = Vec::new();
    let mut unmatched = 0;
    let mut metadata_only = 0;
    let base = canonical_file
        .parent()
        .ok_or("Playlist has no parent directory")?;
    for (entry, metadata_track) in playlist_entries_with_metadata(text) {
        let matched = resolve_entry(base, entry)
            .and_then(|candidate| fs::canonicalize(candidate).ok())
            .and_then(|real| by_path.get(&real));
        if let Some(track) = matched {
            tracks.push(track.clone());
        } else {
            unmatched += 1;
            if let Some(mut track) = metadata_track
                .or_else(|| filename_track(entry))
                .or_else(|| unverified_url_track(entry))
            {
                if is_uri_entry(entry) {
                    track.source_url = entry.to_owned();
                    track.provenance["playlistEntry"] = serde_json::json!(entry);
                    attach_playlist_audio_candidate(&mut track, entry);
                }
                if track.provenance.get("importFormat").is_none() {
                    track.provenance["importFormat"] = serde_json::json!("m3u-filename");
                    track.provenance["identity"] =
                        serde_json::json!("unverified filename metadata");
                }
                tracks.push(track);
                metadata_only += 1;
            }
        }
    }
    if tracks.is_empty() {
        return Err("No playlist entries matched registered local audio files".into());
    }
    let collection = store.create_collection(&playlist_name(&canonical_file))?;
    store.replace_playlist_entries(&collection.id, &tracks)?;
    Ok(LocalPlaylistImport {
        collection: Collection { track_count: tracks.len() as u32, ..collection },
        imported_count: tracks.len(),
        unmatched_count: unmatched,
        warning: (unmatched > 0).then(|| format!("{unmatched} playlist entries did not match registered local files; {metadata_only} were imported from EXTINF or filename metadata only")),
    })
}

fn effective_directories(settings: &Settings) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = settings
        .playlist_directories
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    if let Some(home_music) = dirs::home_dir().map(|home| home.join("Music")) {
        dirs.push(home_music);
    }
    if !settings.music_directory.trim().is_empty() {
        dirs.push(PathBuf::from(&settings.music_directory));
    }
    dirs
}

fn configured_roots(settings: &Settings) -> Vec<PathBuf> {
    effective_directories(settings)
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .filter(|path| path.is_dir())
        .collect()
}

fn playlist_name(path: &Path) -> String {
    let name = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .trim()
        .to_owned();
    if name.is_empty() {
        "Imported Playlist".into()
    } else {
        name
    }
}

fn playlist_entries_with_metadata(text: &str) -> Vec<(&str, Option<Track>)> {
    let mut result = Vec::new();
    let mut extinf: Option<&str> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXTINF:") {
            extinf = Some(line);
        } else if line.is_empty() || line.starts_with('#') {
            continue;
        } else {
            let metadata = extinf.take().and_then(|info| {
                let snippet = format!("#EXTM3U\n{info}\n{line}");
                crate::imports::parse(&snippet, "m3u")
                    .ok()
                    .and_then(|mut tracks| tracks.pop())
            });
            result.push((line, metadata));
        }
    }
    result
}

/// Existing audio files referenced by a playlist, restricted to approved local roots.
/// The returned paths are canonical and suitable for explicit read-only indexing by the
/// async command layer, which also grants the asset protocol file scope.
pub fn referenced_local_files(settings: &Settings, requested_path: &str) -> Result<Vec<PathBuf>> {
    let path = Path::new(requested_path);
    let canonical_file = fs::canonicalize(path).map_err(|_| "Playlist file is unavailable")?;
    if !configured_roots(settings)
        .iter()
        .any(|root| canonical_file.starts_with(root))
    {
        return Err("Playlist file must be inside a configured playlist directory".into());
    }
    let metadata =
        fs::metadata(&canonical_file).map_err(|e| format!("Cannot inspect playlist: {e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_PLAYLIST_BYTES {
        return Err("Playlist must be a file no larger than 2 MB".into());
    }
    let text =
        fs::read_to_string(&canonical_file).map_err(|e| format!("Cannot read playlist: {e}"))?;
    if text.len() as u64 > MAX_PLAYLIST_BYTES || playlist_entry_count(&text) > MAX_PLAYLIST_ENTRIES
    {
        return Err("Playlist exceeds the supported size or entry limit".into());
    }
    let roots = approved_audio_roots(settings);
    let mut files = Vec::new();
    for (entry, _) in playlist_entries_with_metadata(&text) {
        if entry.contains("://") {
            continue;
        }
        let Some(candidate) =
            resolve_entry(canonical_file.parent().unwrap_or(Path::new(".")), entry)
        else {
            continue;
        };
        let Ok(real) = fs::canonicalize(candidate) else {
            continue;
        };
        if real.is_file()
            && roots.iter().any(|root| real.starts_with(root))
            && !files.contains(&real)
        {
            files.push(real);
        }
    }
    Ok(files)
}

fn approved_audio_roots(settings: &Settings) -> Vec<PathBuf> {
    let mut raw = Vec::new();
    if !settings.music_directory.trim().is_empty() {
        raw.push(PathBuf::from(&settings.music_directory));
    }
    raw.extend(
        settings
            .playlist_directories
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from),
    );
    if let Some(home_music) = dirs::home_dir().map(|home| home.join("Music")) {
        raw.push(home_music);
    }
    raw.into_iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .filter(|path| path.is_dir())
        .collect()
}

/// For ordinary path-only M3Us, infer a track only from a filename that already
/// follows the app's explicit `Artist - Title` import convention. Never inspect
/// the referenced audio file; this identity remains explicitly unverified.
fn filename_track(entry: &str) -> Option<Track> {
    let filename = Path::new(entry).file_stem()?.to_str()?.trim();
    if !filename.contains(" - ") && !filename.contains(" — ") && !filename.contains(" – ") {
        return None;
    }
    crate::imports::parse(filename, "tracklist")
        .ok()?
        .into_iter()
        .next()
}

fn is_uri_entry(entry: &str) -> bool {
    url::Url::parse(entry)
        .ok()
        .is_some_and(|url| url.scheme() != "file")
}

fn unverified_url_track(entry: &str) -> Option<Track> {
    if !is_uri_entry(entry) {
        return None;
    }
    let hash = format!("{:x}", Sha256::digest(entry.as_bytes()));
    Some(Track {
        id: format!("import:{hash}"),
        provider: "import".into(),
        provider_id: hash,
        source_url: entry.into(),
        artist: "Unknown artist".into(),
        title: entry.into(),
        provenance: serde_json::json!({"identity":"unverified playlist URL", "importFormat":"m3u-url", "playlistEntry":entry}),
        ..Track::default()
    })
}

/// Attach an explicit but unverified player candidate only for supported public audio hosts.
/// The original M3U URI remains in `source_url`; `candidateUrl` is normalized for playback.
pub(crate) fn attach_playlist_audio_candidate(track: &mut Track, raw: &str) -> bool {
    let raw = raw.trim();
    let source = raw.strip_prefix("ytdl://").unwrap_or(raw);
    let Ok(parsed) = url::Url::parse(source) else {
        return false;
    };
    let provider = if crate::acquisition::validate_source_url(source, "youtube").is_ok() {
        "youtube"
    } else if crate::acquisition::validate_source_url(source, "soundcloud").is_ok() {
        "soundcloud"
    } else {
        return false;
    };
    let (id, candidate_url) = if provider == "youtube" {
        let path_parts = parsed
            .path_segments()
            .map(|parts| parts.collect::<Vec<_>>())
            .unwrap_or_default();
        let video_id = if parsed.host_str() == Some("youtu.be") {
            path_parts.first().map(|value| (*value).to_owned())
        } else if parsed.path() == "/watch" {
            parsed
                .query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())
        } else if matches!(
            path_parts.first().copied(),
            Some("shorts" | "embed" | "live")
        ) {
            path_parts.get(1).map(|value| (*value).to_owned())
        } else {
            None
        };
        let Some(video_id) = video_id.filter(|id| {
            id.len() == 11
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }) else {
            return false;
        };
        (
            format!("youtube:{video_id}"),
            format!("https://www.youtube.com/watch?v={video_id}"),
        )
    } else {
        if parsed.path().trim_matches('/').is_empty() {
            return false;
        }
        let mut normalized = parsed.clone();
        normalized.set_query(None);
        normalized.set_fragment(None);
        let candidate_url = normalized.to_string();
        (format!("soundcloud:{candidate_url}"), candidate_url)
    };
    let candidate = Candidate {
        id,
        provider: provider.to_owned(),
        url: candidate_url.clone(),
        title: if track.title.trim().is_empty() || track.title == raw {
            format!("Imported {provider} playlist source")
        } else {
            track.title.clone()
        },
        duration_seconds: track.duration_seconds,
        formats: Vec::new(),
        permission: "unknown".into(),
        confidence: "low".into(),
        score: 0.0,
        reasons: vec![
            "Explicit playlist URL; recording identity and permission are unverified".into(),
        ],
        official: false,
        ..Candidate::default()
    };
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    track.provenance["sourceKind"] = serde_json::json!("audioSearch");
    track.provenance["identity"] = serde_json::json!("unverified playlist audio source");
    track.provenance["audioCandidate"] =
        serde_json::to_value(candidate).unwrap_or(serde_json::Value::Null);
    track.provenance["candidateUrl"] = serde_json::json!(candidate_url);
    true
}

fn resolve_entry(base: &Path, entry: &str) -> Option<PathBuf> {
    if entry.is_empty() || entry.contains('\0') {
        return None;
    }
    let path = Path::new(entry);
    if path
        .components()
        .any(|part| matches!(part, Component::Prefix(_)))
    {
        return None;
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    Some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LocalAsset;

    fn settings(root: &Path) -> Settings {
        Settings {
            music_directory: root.parent().unwrap_or(root).to_string_lossy().into_owned(),
            playlist_directories: vec![root.to_string_lossy().into_owned()],
            ..Settings::default()
        }
    }

    #[test]
    fn scans_configured_roots_without_following_symlinks_and_sorts_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("z.M3U8"), "#EXTM3U\n/a.mp3\n").unwrap();
        fs::write(root.join("nested/a.m3u"), "#EXTM3U\n/b.mp3\n").unwrap();
        fs::write(root.join("skip.txt"), "x").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("bad.m3u"), "#EXTM3U\n/x\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        let found = list_local_playlists(&settings(&root)).unwrap();
        let mut local: Vec<_> = found
            .iter()
            .filter(|file| file.path.starts_with(root.to_string_lossy().as_ref()))
            .collect();
        local.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(local.len(), 2);
        assert!(local[0].path.ends_with("nested/a.m3u"));
        assert_eq!(local[0].track_count, 1);
        assert!(local[1].path.ends_with("z.M3U8"));
    }

    #[test]
    fn imports_only_registered_entries_inside_configured_playlist_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        let audio = dir.path().join("song.mp3");
        fs::create_dir_all(&root).unwrap();
        fs::write(&audio, b"audio").unwrap();
        let playlist = root.join("Set.m3u");
        fs::write(&playlist, "#EXTM3U\n#EXTINF:200,Artist - Song\n../song.mp3\n#EXTINF:180,Other Artist - Unknown Track\n../missing.mp3\nhttps://example.com/track\n").unwrap();
        let db = dir.path().join("db.sqlite");
        let store = Store::open(&db).unwrap();
        let track = Track {
            id: "local:1".into(),
            provider: "local".into(),
            provider_id: "1".into(),
            artist: "Artist".into(),
            title: "Song".into(),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        store
            .save_asset(&LocalAsset {
                id: "asset-1".into(),
                recording_id: track.id.clone(),
                path: audio.to_string_lossy().into_owned(),
                hash: "hash".into(),
                source_codec: "mp3".into(),
                final_codec: "mp3".into(),
                source_bitrate: None,
                final_bitrate: None,
                processing: "imported".into(),
                track: track.clone(),
            })
            .unwrap();
        let result =
            import_local_playlist(&settings(&root), &store, &playlist.to_string_lossy()).unwrap();
        assert_eq!(result.imported_count, 3);
        assert_eq!(result.unmatched_count, 2);
        let imported = store
            .list_tracks("collections", Some(&result.collection.id))
            .unwrap();
        assert_eq!(imported.len(), 3);
        assert!(imported
            .iter()
            .any(|item| item.id == track.id && item.local_availability == "available"));
        assert!(imported
            .iter()
            .any(|t| t.artist == "Other Artist" && t.title == "Unknown Track"));
        assert!(imported
            .iter()
            .any(|t| t.source_url == "https://example.com/track"));
        let outside = dir.path().join("external.m3u");
        fs::write(&outside, "#EXTM3U\n../song.mp3\n").unwrap();
        assert!(
            import_local_playlist(&settings(&root), &store, &outside.to_string_lossy()).is_err()
        );
    }

    #[test]
    fn imported_playlist_urls_get_candidates_only_for_validated_youtube_and_soundcloud_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        fs::create_dir_all(&root).unwrap();
        let playlist = root.join("Streams.m3u8");
        fs::write(
            &playlist,
            "#EXTM3U\n#EXTINF:-1,Artist - Watch\nhttps://www.youtube.com/watch?v=ABCDEF12345\n#EXTINF:-1,Artist - Short URL\nhttps://youtu.be/ZYXWV987654\n#EXTINF:-1,Artist - Shorts\nhttps://youtube.com/shorts/Shorts12345\n#EXTINF:-1,Artist - SoundCloud\nhttps://soundcloud.com/dj/example-track\n#EXTINF:-1,Artist - Wrapped YouTube\nytdl://https://youtu.be/ABCDEF12345\n#EXTINF:-1,Artist - Lookalike\nhttps://youtube.com.attacker.test/watch?v=ABCDEF12345\n#EXTINF:-1,Artist - Arbitrary\nhttps://example.com/music\n",
        )
        .unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let imported =
            import_local_playlist(&settings(&root), &store, &playlist.to_string_lossy()).unwrap();
        let tracks = store
            .list_tracks("collection", Some(&imported.collection.id))
            .unwrap();
        assert_eq!(tracks.len(), 7);
        for track in &tracks[..5] {
            assert_eq!(
                track.provider, "import",
                "source identity remains unverified"
            );
            assert_eq!(track.provenance["sourceKind"], "audioSearch");
            assert_eq!(
                track.provenance["identity"],
                "unverified playlist audio source"
            );
            assert_eq!(track.provenance["audioCandidate"]["confidence"], "low");
            assert_eq!(track.provenance["audioCandidate"]["permission"], "unknown");
            assert_eq!(track.provenance["audioCandidate"]["score"], 0.0);
            assert_eq!(track.source_url, track.provenance["playlistEntry"]);
        }
        assert_eq!(
            tracks[0].provenance["audioCandidate"]["provider"],
            "youtube"
        );
        assert_eq!(
            tracks[0].provenance["candidateUrl"],
            "https://www.youtube.com/watch?v=ABCDEF12345"
        );
        assert_eq!(
            tracks[1].provenance["candidateUrl"],
            "https://www.youtube.com/watch?v=ZYXWV987654"
        );
        assert_eq!(
            tracks[2].provenance["candidateUrl"],
            "https://www.youtube.com/watch?v=Shorts12345"
        );
        assert_eq!(
            tracks[3].provenance["audioCandidate"]["provider"],
            "soundcloud"
        );
        assert_eq!(tracks[4].source_url, "ytdl://https://youtu.be/ABCDEF12345");
        assert!(tracks[5].provenance.get("audioCandidate").is_none());
        assert!(tracks[6].provenance.get("audioCandidate").is_none());
    }

    #[test]
    fn imports_metadata_from_unregistered_artist_title_filenames_without_opening_audio() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        fs::create_dir_all(&root).unwrap();
        let playlist = root.join("Plain.m3u");
        fs::write(&playlist, "#EXTM3U\n/approved/music/Artist - Title (Extended Mix).flac\n/approved/music/unknown-name.flac\n").unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let result =
            import_local_playlist(&settings(&root), &store, &playlist.to_string_lossy()).unwrap();
        assert_eq!(result.imported_count, 1);
        assert_eq!(result.unmatched_count, 2);
        let imported = store
            .list_tracks("collections", Some(&result.collection.id))
            .unwrap();
        assert_eq!(imported[0].artist, "Artist");
        assert_eq!(imported[0].title, "Title");
        assert_eq!(imported[0].version.as_deref(), Some("Extended Mix"));
        assert_eq!(imported[0].provenance["importFormat"], "m3u-filename");
    }

    #[test]
    fn ordered_playlist_entries_preserve_repeated_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        fs::create_dir_all(&root).unwrap();
        let audio = dir.path().join("song.mp3");
        fs::write(&audio, b"audio").unwrap();
        let playlist = root.join("Repeats.m3u");
        fs::write(&playlist, "#EXTM3U\n../song.mp3\n../song.mp3\n").unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let track = Track {
            id: "local:repeat".into(),
            provider: "local".into(),
            provider_id: "repeat".into(),
            artist: "Artist".into(),
            title: "Repeat".into(),
            local_path: Some(audio.to_string_lossy().into_owned()),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        store
            .save_asset(&LocalAsset {
                id: "asset-repeat".into(),
                recording_id: track.id.clone(),
                path: audio.to_string_lossy().into_owned(),
                hash: "repeat".into(),
                source_codec: "mp3".into(),
                final_codec: "mp3".into(),
                source_bitrate: None,
                final_bitrate: None,
                processing: "external-readonly".into(),
                track: track.clone(),
            })
            .unwrap();
        let imported =
            import_local_playlist(&settings(&root), &store, &playlist.to_string_lossy()).unwrap();
        let members = store
            .list_tracks("collection", Some(&imported.collection.id))
            .unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id, members[1].id);
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .iter()
                .find(|c| c.id == imported.collection.id)
                .unwrap()
                .track_count,
            2
        );
    }
}
