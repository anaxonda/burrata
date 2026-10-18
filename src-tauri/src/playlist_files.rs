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
const BURRATA_MIRROR_MARKER: &str = "#BURRATA-MIRROR";

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
        if let Ok(mirror_root) = fs::canonicalize(&configured_mirror_root) {
            files.retain(|file| {
                let path = Path::new(&file.path);
                !path.starts_with(&mirror_root)
                    || !is_burrata_generated_playlist(path, &mirror_root)
            });
        }
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

fn is_burrata_mirror(path: &Path) -> bool {
    fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.len() <= MAX_PLAYLIST_BYTES)
        .and_then(|_| fs::read(path).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|text| {
            text.lines()
                .any(|line| line.trim() == BURRATA_MIRROR_MARKER)
        })
}

/// Recognize pre-marker Burrata files by their narrowly defined generated path shape.
/// Ordinary playlists elsewhere under Music/Playlists remain discoverable.
fn is_legacy_burrata_playlist(path: &Path, mirror_root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(mirror_root) else {
        return false;
    };
    let parts = relative
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.len() == 2
        && generated_group(&parts[0])
        && Path::new(&parts[1])
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(is_uuid)
    {
        return true;
    }
    if parts.len() != 4 || !parts[0].eq_ignore_ascii_case("Archives") {
        return false;
    }
    let favorites =
        parts[1].eq_ignore_ascii_case("Favorites") && parts[2].eq_ignore_ascii_case("favorites");
    if !(favorites || (generated_group(&parts[1]) && is_uuid(&parts[2]))) {
        return false;
    }
    let Some(stem) = Path::new(&parts[3])
        .file_stem()
        .and_then(|stem| stem.to_str())
    else {
        return false;
    };
    let Some((timestamp, revision)) = stem.split_once('-') else {
        return false;
    };
    timestamp.len() == 14
        && timestamp.bytes().all(|byte| byte.is_ascii_digit())
        && (is_uuid(revision)
            || (revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())))
}

pub(crate) fn is_burrata_generated_playlist(path: &Path, mirror_root: &Path) -> bool {
    is_burrata_mirror(path) || is_legacy_burrata_playlist(path, mirror_root)
}

fn generated_group(group: &str) -> bool {
    [
        "Favorites",
        "Personal",
        "Spotify",
        "Beatport",
        "Traxsource",
        "Local",
        "Mixxx",
    ]
    .iter()
    .any(|known| group.eq_ignore_ascii_case(known))
}

fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
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
                warning,
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
    let (tracks, unmatched, metadata_only) =
        preview_playlist_tracks(settings, store, &canonical_file, text)?;
    if tracks.is_empty() {
        return Err("No playlist entries matched registered local audio files".into());
    }
    let collection = store.create_collection(&playlist_name(&canonical_file))?;
    store.replace_playlist_entries(&collection.id, &tracks)?;
    Ok(LocalPlaylistImport {
        collection: Collection {
            track_count: tracks.len() as u32,
            ..collection
        },
        imported_count: tracks.len(),
        unmatched_count: unmatched,
        warning: (unmatched > 0).then(|| format!("{unmatched} playlist entries did not match registered local files; {metadata_only} were imported from EXTINF or filename metadata only")),
    })
}

/// Reproduce the import resolver without creating or changing a collection.
pub(crate) fn preview_playlist_tracks(
    settings: &Settings,
    store: &Store,
    canonical_file: &Path,
    text: &str,
) -> Result<(Vec<Track>, usize, usize)> {
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
    for (entry, metadata_track, is_extinf) in playlist_entries_with_metadata(text) {
        let matched = resolve_entry(base, entry)
            .and_then(|candidate| fs::canonicalize(candidate).ok())
            .and_then(|real| by_path.get(&real));
        if let Some(track) = matched {
            tracks.push(track.clone());
        } else {
            unmatched += 1;
            if let Some(mut track) = if is_uri_entry(entry) && !is_extinf {
                // URL identity is stable on the exact source URL. Playlist comments
                // supply display metadata without changing that identity.
                unverified_url_track(entry).map(|mut url_track| {
                    if let Some(comment) = metadata_track.as_ref() {
                        apply_playlist_comment(&mut url_track, comment);
                    }
                    url_track
                })
            } else {
                metadata_track
                    .or_else(|| filename_track(entry))
                    .or_else(|| unverified_url_track(entry))
            } {
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
    Ok((tracks, unmatched, metadata_only))
}

// Unit tests configure scratch roots explicitly. Never scan the developer's
// real playlists while exercising imports and synchronization in a test database.
fn standard_music_root() -> Option<PathBuf> {
    // A supplied isolation root is authoritative: reject it closed rather
    // than falling back to the user's real Music directory.
    if let Some(value) = std::env::var_os("BURRATA_ISOLATED_MUSIC_ROOT") {
        return value.to_str().and_then(isolated_music_root_from);
    }
    #[cfg(not(test))]
    {
        dirs::home_dir().map(|home| home.join("Music"))
    }
    #[cfg(test)]
    {
        None
    }
}

fn isolated_music_root_from(value: &str) -> Option<PathBuf> {
    let supplied = PathBuf::from(value);
    let root = fs::canonicalize(&supplied).ok()?;
    // Require the exact canonical spelling and reject symlinks at every
    // component so an apparently isolated path cannot escape its profile.
    if root != supplied || fs::canonicalize("/tmp").ok()?.as_path() != Path::new("/tmp") {
        return None;
    }
    let music = root.file_name()?;
    if music != "Music" || !root.is_dir() {
        return None;
    }
    let fixture = root.parent()?;
    if fixture.file_name()? != "fixture" {
        return None;
    }
    let profile = fixture.parent()?;
    if profile.parent()? != Path::new("/tmp") {
        return None;
    }
    let profile_name = profile.file_name()?.to_str()?;
    let suffix = profile_name.strip_prefix("burrata-launch.")?;
    if suffix.is_empty()
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }

    for path in [profile, fixture, root.as_path()] {
        let metadata = fs::symlink_metadata(path).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return None;
        }
    }
    let marker = profile.join(".burrata-isolated-profile");
    let marker_metadata = fs::symlink_metadata(&marker).ok()?;
    if marker_metadata.file_type().is_symlink() || !marker_metadata.is_file() {
        return None;
    }
    if fs::read_to_string(marker).ok()?.as_str() != "burrata isolated memory run\n" {
        return None;
    }
    Some(root)
}

pub(crate) fn effective_directories(settings: &Settings) -> Vec<PathBuf> {
    effective_directories_with_standard_root(settings, standard_music_root())
}

fn effective_directories_with_standard_root(
    settings: &Settings,
    standard_root: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = settings
        .playlist_directories
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    if let Some(standard_root) = standard_root {
        dirs.push(standard_root);
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

fn playlist_entries_with_metadata(text: &str) -> Vec<(&str, Option<Track>, bool)> {
    let mut result = Vec::new();
    let mut extinf: Option<&str> = None;
    let mut comment: Option<Track> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXTINF:") {
            extinf = Some(line);
        } else if let Some(value) = line.strip_prefix('#') {
            // Ordinary comments may carry an explicit “Artist - Title” hint.
            // Ignore all M3U directives and require the same conservative
            // separators as the app's tracklist import format.
            if !value.trim_start().starts_with("EXT") {
                comment = parse_playlist_comment(value);
            }
        } else if line.is_empty() || line.starts_with('#') {
            continue;
        } else {
            let from_extinf = extinf.is_some();
            let metadata = extinf
                .take()
                .and_then(|info| {
                    let snippet = format!("#EXTM3U\n{info}\n{line}");
                    crate::imports::parse(&snippet, "m3u")
                        .ok()
                        .and_then(|mut tracks| tracks.pop())
                })
                .or_else(|| comment.take());
            comment = None;
            result.push((line, metadata, from_extinf));
        }
    }
    result
}

fn parse_playlist_comment(value: &str) -> Option<Track> {
    let value = value.trim();
    let upper = value.to_ascii_uppercase();
    if value.is_empty()
        || value.contains("://")
        || upper.starts_with("EXT")
        || upper.starts_with("PLAYLIST:")
        || upper.starts_with("BURRATA-")
        || upper.starts_with("KODIPROP:")
        || upper.starts_with("VLCOPT:")
    {
        return None;
    }
    crate::imports::parse(value, "tracklist")
        .ok()?
        .into_iter()
        .next()
}

fn apply_playlist_comment(track: &mut Track, comment: &Track) {
    track.artist = comment.artist.clone();
    track.title = comment.title.clone();
    track.version = comment.version.clone();
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    track.provenance["identity"] = serde_json::json!("unverified playlist comment");
    track.provenance["playlistComment"] = serde_json::json!({
        "artist": comment.artist,
        "title": comment.title,
        "version": comment.version,
        "catalogVerified": false
    });
}

/// Repair only URL-only placeholder metadata from an unchanged playlist source.
/// This updates track metadata in place, preserving stable URL IDs and all playlist
/// memberships/occurrences. User-edited fields are protected by the manualFields overlay.
pub(crate) fn repair_placeholder_url_metadata(
    store: &Store,
    collection_id: &str,
    text: &str,
) -> Result<usize> {
    // Most collections were imported with metadata already populated. Check
    // whether repair can do anything before reparsing every playlist comment
    // (which invokes the full tracklist parser) on each watcher reconciliation.
    let has_comment_hint = text.lines().any(|line| {
        line.trim().strip_prefix('#').is_some_and(|value| {
            !value.trim_start().starts_with("EXT")
                && [" - ", " — ", " – "]
                    .iter()
                    .any(|separator| value.contains(separator))
        })
    });
    if !text.contains("://") || !has_comment_hint {
        return Ok(0);
    }
    let collection_tracks = store.list_tracks("collection", Some(collection_id))?;
    if !collection_tracks.iter().any(|track| {
        track.provider == "import"
            && track.artist == "Unknown artist"
            && track.title == track.source_url
    }) {
        return Ok(0);
    }
    let comments = playlist_entries_with_metadata(text)
        .into_iter()
        .filter_map(|(entry, comment, is_extinf)| {
            if is_extinf || !is_uri_entry(entry) {
                return None;
            }
            let mut track = unverified_url_track(entry)?;
            apply_playlist_comment(&mut track, comment.as_ref()?);
            Some((entry.to_owned(), track))
        })
        .collect::<HashMap<_, _>>();
    if comments.is_empty() {
        return Ok(0);
    }
    // Require the bound collection to still contain the stable URL-only placeholder
    // before propagating the same source comment to duplicate catalog rows created
    // by earlier generated-mirror imports.
    let bound_urls = collection_tracks
        .iter()
        .filter(|track| {
            track.provider == "import"
                && track.artist == "Unknown artist"
                && track.title == track.source_url
                && comments
                    .get(&track.source_url)
                    .is_some_and(|comment| track.id == comment.id)
        })
        .map(|track| track.source_url.clone())
        .collect::<std::collections::HashSet<_>>();
    if bound_urls.is_empty() {
        return Ok(0);
    }
    let urls = bound_urls.iter().cloned().collect::<Vec<_>>();
    let tracks = store.list_import_tracks_by_source_urls(&urls)?;
    let mut repaired = 0;
    let mut updated_ids = std::collections::HashSet::new();
    for mut track in tracks {
        if !bound_urls.contains(&track.source_url) {
            continue;
        }
        let Some(comment_track) = comments.get(&track.source_url) else {
            continue;
        };
        if track.provider != "import"
            || track.artist != "Unknown artist"
            || track.title != track.source_url
            || track.provenance["manualFields"]["artist"] == true
            || track.provenance["manualFields"]["title"] == true
            || !updated_ids.insert(track.id.clone())
        {
            continue;
        }
        apply_playlist_comment(&mut track, comment_track);
        // Retain the audio candidate and source evidence already attached at import.
        track.provenance["playlistEntry"] = serde_json::json!(track.source_url);
        store.cache_track(&track)?;
        repaired += 1;
    }
    Ok(repaired)
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
    for (entry, _, _) in playlist_entries_with_metadata(&text) {
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
    if let Some(home_music) = standard_music_root() {
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

pub(crate) fn unverified_url_track(entry: &str) -> Option<Track> {
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
    if track.provenance["playlistComment"].is_null() {
        track.provenance["identity"] = serde_json::json!("unverified playlist audio source");
    }
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

    #[cfg(target_os = "linux")]
    fn isolated_profile() -> (tempfile::TempDir, PathBuf) {
        let profile = tempfile::Builder::new()
            .prefix("burrata-launch.")
            .tempdir_in("/tmp")
            .unwrap();
        fs::write(
            profile.path().join(".burrata-isolated-profile"),
            "burrata isolated memory run\n",
        )
        .unwrap();
        let music = profile.path().join("fixture/Music");
        fs::create_dir_all(&music).unwrap();
        (profile, music)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn isolated_music_root_requires_the_exact_profile_marker() {
        let (profile, music) = isolated_profile();
        fs::write(
            profile.path().join(".burrata-isolated-profile"),
            "burrata isolated memory run",
        )
        .unwrap();
        assert_eq!(isolated_music_root_from(music.to_str().unwrap()), None);
        fs::write(
            profile.path().join(".burrata-isolated-profile"),
            "burrata isolated memory run\n",
        )
        .unwrap();
        assert_eq!(
            isolated_music_root_from(music.to_str().unwrap()),
            Some(music.clone())
        );
        fs::remove_file(profile.path().join(".burrata-isolated-profile")).unwrap();
        assert_eq!(isolated_music_root_from(music.to_str().unwrap()), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn isolated_music_root_accepts_launcher_suffix_underscores_and_hyphens_only() {
        for prefix in [
            "burrata-launch.profile_test_",
            "burrata-launch.profile-test-",
        ] {
            let profile = tempfile::Builder::new()
                .prefix(prefix)
                .tempdir_in("/tmp")
                .unwrap();
            fs::write(
                profile.path().join(".burrata-isolated-profile"),
                "burrata isolated memory run\n",
            )
            .unwrap();
            let music = profile.path().join("fixture/Music");
            fs::create_dir_all(&music).unwrap();
            assert_eq!(
                isolated_music_root_from(music.to_str().unwrap()),
                Some(music)
            );
        }

        let dotted_profile = tempfile::Builder::new()
            .prefix("burrata-launch.profile.test-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::write(
            dotted_profile.path().join(".burrata-isolated-profile"),
            "burrata isolated memory run\n",
        )
        .unwrap();
        let dotted_music = dotted_profile.path().join("fixture/Music");
        fs::create_dir_all(&dotted_music).unwrap();
        assert_eq!(
            isolated_music_root_from(dotted_music.to_str().unwrap()),
            None
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn effective_directories_combine_configured_and_validated_standard_roots() {
        let (_profile, standard_music) = isolated_profile();
        let custom = standard_music.parent().unwrap().join("CustomPlaylists");
        let configured_music = standard_music.parent().unwrap().join("ConfiguredMusic");
        fs::create_dir_all(&custom).unwrap();
        fs::create_dir_all(&configured_music).unwrap();
        let validated_standard = isolated_music_root_from(standard_music.to_str().unwrap());
        assert_eq!(
            validated_standard.as_deref(),
            Some(standard_music.as_path())
        );

        let settings = Settings {
            playlist_directories: vec![custom.to_string_lossy().into_owned()],
            music_directory: configured_music.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        assert_eq!(
            effective_directories_with_standard_root(&settings, validated_standard),
            vec![custom, standard_music, configured_music]
        );
    }

    #[cfg(unix)]
    #[test]
    #[cfg(target_os = "linux")]
    fn isolated_music_root_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let (_profile, music) = isolated_profile();
        let alias_profile = tempfile::Builder::new()
            .prefix("burrata-launch.")
            .tempdir_in("/tmp")
            .unwrap();
        fs::write(
            alias_profile.path().join(".burrata-isolated-profile"),
            "burrata isolated memory run\n",
        )
        .unwrap();
        let alias_fixture = alias_profile.path().join("fixture");
        fs::create_dir_all(&alias_fixture).unwrap();
        symlink(&music, alias_fixture.join("Music")).unwrap();
        assert_eq!(
            isolated_music_root_from(alias_fixture.join("Music").to_str().unwrap()),
            None
        );
    }
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
        // Keep this distinct from the implicit music_directory/Playlists mirror
        // path. macOS filesystems are case-insensitive, so "playlists" aliases it.
        let root = dir.path().join("configured-playlists");
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
        // Compare the scanner's canonical output against the canonical root,
        // since macOS may resolve temp directories through a symlinked prefix.
        let canonical_root = fs::canonicalize(&root).unwrap();
        let mut local: Vec<_> = found
            .iter()
            .filter(|file| Path::new(&file.path).starts_with(&canonical_root))
            .collect();
        local.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(local.len(), 2);
        assert!(Path::new(&local[0].path).ends_with(Path::new("nested").join("a.m3u")));
        assert_eq!(local[0].track_count, 1);
        assert!(Path::new(&local[1].path).ends_with("z.M3U8"));
    }

    #[test]
    fn keeps_user_playlists_beside_burrata_mirrors_visible() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let mirror_root = music.join("Playlists");
        fs::create_dir_all(&mirror_root).unwrap();
        let user_playlist = mirror_root.join("My playlist.m3u");
        fs::write(&user_playlist, "#EXTM3U\n/song.mp3\n").unwrap();
        let empty_playlist = mirror_root.join("Empty.m3u");
        fs::write(&empty_playlist, "#EXTM3U\n").unwrap();
        let user_archive_folder = mirror_root.join("Archives");
        fs::create_dir_all(&user_archive_folder).unwrap();
        fs::write(
            user_archive_folder.join("My archive.m3u"),
            "#EXTM3U\n/song.mp3\n",
        )
        .unwrap();
        let mirror = mirror_root.join("generated.m3u8");
        fs::write(&mirror, "#EXTM3U\n#BURRATA-MIRROR\n#PLAYLIST:Generated\n").unwrap();
        let legacy_collection_id = uuid::Uuid::new_v4();
        let legacy_mirror = mirror_root
            .join("Spotify")
            .join(format!("{legacy_collection_id}.m3u8"));
        fs::create_dir_all(legacy_mirror.parent().unwrap()).unwrap();
        fs::write(
            &legacy_mirror,
            "#EXTM3U\n#PLAYLIST:Legacy mirror\n/song.mp3\n",
        )
        .unwrap();
        let legacy_mixxx = mirror_root
            .join("Mixxx")
            .join(format!("{}.m3u8", uuid::Uuid::new_v4()));
        fs::create_dir_all(legacy_mixxx.parent().unwrap()).unwrap();
        fs::write(
            &legacy_mixxx,
            "#EXTM3U\n#PLAYLIST:Legacy Mixxx export\n/song.mp3\n",
        )
        .unwrap();
        let archive_id = uuid::Uuid::new_v4();
        let legacy_archive = mirror_root
            .join("Archives/Traxsource")
            .join(archive_id.to_string())
            .join(format!("20261013010101-{}.m3u8", "a".repeat(64)));
        fs::create_dir_all(legacy_archive.parent().unwrap()).unwrap();
        fs::write(
            &legacy_archive,
            "#EXTM3U\n#PLAYLIST:Legacy archive\n/song.mp3\n",
        )
        .unwrap();

        let settings = Settings {
            music_directory: music.to_string_lossy().into_owned(),
            playlist_directories: vec![mirror_root.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        let found = list_local_playlists(&settings).unwrap();
        let canonical_mirror_root = fs::canonicalize(&mirror_root).unwrap();
        let found = found
            .into_iter()
            .filter(|file| Path::new(&file.path).starts_with(&canonical_mirror_root))
            .collect::<Vec<_>>();
        assert_eq!(found.len(), 3);
        let found_paths = found
            .iter()
            .map(|file| PathBuf::from(&file.path))
            .collect::<Vec<_>>();
        assert!(found_paths.contains(&fs::canonicalize(user_playlist).unwrap()));
        assert!(found_paths.contains(&fs::canonicalize(empty_playlist).unwrap()));
        assert!(found_paths
            .contains(&fs::canonicalize(user_archive_folder.join("My archive.m3u")).unwrap()));
        assert!(found.iter().any(|file| {
            file.name == "Empty" && file.track_count == 0 && file.warning.is_none()
        }));
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
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_seconds: None,
                size_bytes: None,
                bitrate_origin: None,
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
    fn plain_artist_title_comments_enrich_url_tracks_without_changing_url_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        fs::create_dir_all(&root).unwrap();
        let playlist = root.join("Comments.m3u");
        let url = "https://www.youtube.com/watch?v=ABCDEF12345";
        fs::write(
            &playlist,
            format!("#EXTM3U\n# Mercedes Sosa - Alfonsina y el Mar (Audio)\n{url}\n"),
        )
        .unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();

        let imported =
            import_local_playlist(&settings(&root), &store, &playlist.to_string_lossy()).unwrap();
        let track = store
            .list_tracks("collection", Some(&imported.collection.id))
            .unwrap()
            .remove(0);
        let expected_id = format!("import:{:x}", Sha256::digest(url.as_bytes()));

        assert_eq!(track.id, expected_id);
        assert_eq!(track.provider, "import");
        assert_eq!(track.source_url, url);
        assert_eq!(track.artist, "Mercedes Sosa");
        assert_eq!(track.title, "Alfonsina y el Mar (Audio)");
        assert_eq!(track.version, None);
        assert_eq!(track.provenance["identity"], "unverified playlist comment");
        assert_eq!(
            track.provenance["playlistComment"]["catalogVerified"],
            false
        );
        assert_eq!(track.provenance["audioCandidate"]["provider"], "youtube");
    }

    #[test]
    fn later_bare_url_playlist_reuses_metadata_from_earlier_annotated_import() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("playlists");
        fs::create_dir_all(&root).unwrap();
        let url = "https://www.youtube.com/watch?v=ABCDEF12345";
        let annotated = root.join("Annotated.m3u");
        let bare = root.join("Bare.m3u");
        fs::write(
            &annotated,
            format!("#EXTM3U\n# Mercedes Sosa - Alfonsina y el Mar\n{url}\n"),
        )
        .unwrap();
        fs::write(&bare, format!("#EXTM3U\n{url}\n")).unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();

        let first =
            import_local_playlist(&settings(&root), &store, &annotated.to_string_lossy()).unwrap();
        let second =
            import_local_playlist(&settings(&root), &store, &bare.to_string_lossy()).unwrap();
        let first_track = store
            .list_tracks("collection", Some(&first.collection.id))
            .unwrap()
            .remove(0);
        let second_track = store
            .list_tracks("collection", Some(&second.collection.id))
            .unwrap()
            .remove(0);

        assert_eq!(first_track.id, second_track.id);
        assert_eq!(first_track.artist, "Mercedes Sosa");
        assert_eq!(second_track.artist, first_track.artist);
        assert_eq!(second_track.title, first_track.title);
        assert_eq!(second_track.source_url, url);
        assert_eq!(
            first_track.provenance["playlistComment"]["catalogVerified"],
            false
        );
    }

    #[test]
    fn extinf_metadata_takes_precedence_over_plain_comment() {
        let entries = playlist_entries_with_metadata(
            "#EXTM3U\n# Comment Artist - Comment Title\n#EXTINF:180,EXTINF Artist - EXTINF Title\nhttps://example.com/track\n",
        );
        let (_, metadata, is_extinf) = &entries[0];
        assert!(is_extinf);
        assert_eq!(metadata.as_ref().unwrap().artist, "EXTINF Artist");
        assert_eq!(metadata.as_ref().unwrap().title, "EXTINF Title");
    }

    #[test]
    fn playlist_header_with_artist_separator_is_not_attached_to_first_url() {
        let entries = playlist_entries_with_metadata(
            "#EXTM3U\n#PLAYLIST: Artist - Album\nhttps://example.com/track\n",
        );
        assert!(entries[0].1.is_none());
    }

    #[test]
    fn unchanged_comment_repair_preserves_url_ids_memberships_and_manual_fields() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let first_url = "https://youtu.be/ABCDEF12345";
        let second_url = "https://youtu.be/ZYXWV987654";
        let text = format!(
            "#EXTM3U\n# Manual Artist - Manual Title\n{first_url}\n# Béla Fleck - Drive\n{second_url}\n"
        );
        let first = unverified_url_track(first_url).unwrap();
        let second = unverified_url_track(second_url).unwrap();
        let archive_copy = Track {
            id: "import:archive-copy".into(),
            provider: "import".into(),
            provider_id: "archive-copy".into(),
            source_url: second_url.into(),
            artist: "Unknown artist".into(),
            title: second_url.into(),
            provenance: serde_json::json!({"importFormat":"m3u-url", "playlistEntry":second_url}),
            ..Track::default()
        };
        store.save_track(&first).unwrap();
        store.save_track(&second).unwrap();
        store.save_track(&archive_copy).unwrap();
        let collection = store.create_collection("Comment repair").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[first.clone(), second.clone()])
            .unwrap();
        let archive_collection = store.create_collection("Generated archive copy").unwrap();
        store
            .replace_playlist_entries(&archive_collection.id, &[archive_copy.clone()])
            .unwrap();
        store.delete_collection(&archive_collection.id).unwrap();

        // Even a manual override whose visible value is still the placeholder is
        // protected; repair should only change the untouched second record.
        let mut manually_edited = store.get_track(&first.id).unwrap().unwrap();
        manually_edited.artist = "Temporary edit".into();
        store.update_track(&manually_edited).unwrap();
        manually_edited = store.get_track(&first.id).unwrap().unwrap();
        manually_edited.artist = "Unknown artist".into();
        store.update_track(&manually_edited).unwrap();
        let repaired = repair_placeholder_url_metadata(&store, &collection.id, &text).unwrap();

        let members = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(repaired, 2);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id, first.id);
        assert_eq!(members[0].artist, "Unknown artist");
        assert_eq!(members[1].id, second.id);
        assert_eq!(members[1].artist, "Béla Fleck");
        assert_eq!(members[1].title, "Drive");
        assert_eq!(members[1].source_url, second_url);
        let archive_copy = store.get_track(&archive_copy.id).unwrap().unwrap();
        assert_eq!(archive_copy.artist, "Béla Fleck");
        assert_eq!(archive_copy.title, "Drive");
        assert_eq!(store.list_deleted_collections().unwrap()[0].track_count, 1);
        assert_eq!(store.list_collections().unwrap()[0].track_count, 2);
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
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_seconds: None,
                size_bytes: None,
                bitrate_origin: None,
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
