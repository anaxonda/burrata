//! Local-file playlist mirror in the iTunes library XML format.

use crate::models::Track;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use url::Url;

pub(super) const OWNER: &str = "<!-- Burrata managed DJ playlist mirror -->";
pub(super) const ITUNES_NAME: &str = "Burrata iTunes Library.xml";

pub(super) struct Document {
    pub body: String,
    pub skipped: usize,
}

pub(super) fn target_root(value: &str) -> Result<Option<Url>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let error = || {
        "Enter an absolute file URL for the DJ music folder, such as file:///D:/Music".to_owned()
    };
    let mut root = Url::parse(value).map_err(|_| error())?;
    if root.scheme() != "file"
        || root.cannot_be_a_base()
        || !root.path().starts_with('/')
        || root.query().is_some()
        || root.fragment().is_some()
    {
        return Err(error());
    }
    let mut path = root.path_segments_mut().map_err(|_| error())?;
    path.pop_if_empty().push("");
    drop(path);
    Ok(Some(root))
}

fn location(path: &Path, music_root: &Path, destination: Option<&Url>) -> Option<Url> {
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    let Some(destination) = destination else {
        return Url::from_file_path(path).ok();
    };
    let relative = path.canonicalize().ok()?;
    let relative = relative.strip_prefix(music_root).ok()?;
    let mut url = destination.clone();
    let mut segments = url.path_segments_mut().ok()?;
    segments.pop_if_empty();
    for component in relative.components() {
        segments.push(component.as_os_str().to_str()?);
    }
    drop(segments);
    Some(url)
}

fn xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' => escaped.push_str("&#9;"),
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}' => {
                escaped.push(character);
            }
            _ => escaped.push('\u{fffd}'),
        }
    }
    escaped
}

fn number(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn persistent_id(value: &str) -> String {
    Sha256::digest(value.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

fn numeric_id(key: &str, taken: &mut HashSet<u32>) -> u32 {
    let hash = Sha256::digest(key.as_bytes());
    let mut id = (u32::from_be_bytes(hash[..4].try_into().unwrap()) & 0x7fff_ffff).max(1);
    while !taken.insert(id) {
        id = if id == 0x7fff_ffff { 1 } else { id + 1 };
    }
    id
}

fn plist_string(output: &mut String, key: &str, value: &str) {
    write!(output, "<key>{key}</key><string>{}</string>", xml(value)).unwrap();
}

fn playlist_name(name: &str, seen: &mut HashSet<String>) -> String {
    let base = if name.trim().is_empty() {
        "Playlist"
    } else {
        name.trim()
    };
    let mut current = base.to_owned();
    let mut suffix = 2;
    while !seen.insert(current.to_lowercase()) {
        current = format!("{base} ({suffix})");
        suffix += 1;
    }
    current
}

pub(super) fn render(
    playlists: impl IntoIterator<Item = (String, String, Vec<Track>)>,
    music_root: &Path,
    destination: Option<&Url>,
) -> Result<Document, String> {
    let mut playlists = playlists.into_iter().peekable();
    let canonical_root = if destination.is_some() && playlists.peek().is_some() {
        music_root
            .canonicalize()
            .map_err(|error| format!("Cannot resolve the DJ mirror music folder: {error}"))?
    } else {
        music_root.to_owned()
    };
    let mut tracks = BTreeMap::new();
    let mut ordered = Vec::new();
    let mut names = HashSet::new();
    let mut skipped = 0;
    for (playlist_id, name, rows) in playlists {
        let mut entries = Vec::new();
        for row in rows {
            let url = row
                .local_path
                .as_deref()
                .and_then(|path| location(Path::new(path), &canonical_root, destination));
            if let Some(url) = url {
                let url = url.to_string();
                entries.push(url.clone());
                tracks.entry(url).or_insert(row);
            } else {
                skipped += 1;
            }
        }
        ordered.push((playlist_id, playlist_name(&name, &mut names), entries));
    }

    let mut itunes = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{OWNER}\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Major Version</key><integer>1</integer><key>Minor Version</key><integer>1</integer><key>Application Version</key><string>12.0</string><key>Library Persistent ID</key><string>{}</string><key>Tracks</key><dict>\n",
        persistent_id("Burrata DJ Library")
    );

    let mut taken = HashSet::new();
    let mut ids = BTreeMap::new();
    for (location, track) in tracks {
        let id = numeric_id(&location, &mut taken);
        write!(
            itunes,
            "<key>{id}</key><dict><key>Track ID</key><integer>{id}</integer>"
        )
        .unwrap();
        plist_string(&mut itunes, "Name", &track.title);
        plist_string(&mut itunes, "Artist", &track.artist);
        plist_string(&mut itunes, "Location", &location);
        plist_string(&mut itunes, "Track Type", "File");
        plist_string(&mut itunes, "Persistent ID", &persistent_id(&location));
        for (key, value) in [
            ("Album", track.album.as_deref()),
            ("Genre", track.genre.as_deref()),
        ] {
            if let Some(value) = value {
                plist_string(&mut itunes, key, value);
            }
        }
        if let Some(bpm) = number(track.bpm).filter(|value| *value > 0.0 && *value <= 1_000.0) {
            write!(
                itunes,
                "<key>BPM</key><integer>{}</integer>",
                bpm.round() as u32
            )
            .unwrap();
        }
        if let Some(seconds) = number(track.duration_seconds) {
            write!(
                itunes,
                "<key>Total Time</key><integer>{}</integer>",
                (seconds * 1000.0).round() as i32
            )
            .unwrap();
        }
        if let Some(number) = track.track_number {
            write!(itunes, "<key>Track Number</key><integer>{number}</integer>").unwrap();
        }
        itunes.push_str("</dict>\n");
        ids.insert(location, id);
    }
    itunes.push_str("</dict><key>Playlists</key><array>\n");
    taken.clear();
    for (playlist_id, name, entries) in ordered {
        let id = numeric_id(&playlist_id, &mut taken);
        itunes.push_str("<dict>");
        plist_string(&mut itunes, "Name", &name);
        plist_string(
            &mut itunes,
            "Playlist Persistent ID",
            &persistent_id(&playlist_id),
        );
        write!(itunes, "<key>Playlist ID</key><integer>{id}</integer><key>All Items</key><true/><key>Playlist Items</key><array>").unwrap();
        for entry in entries {
            let id = ids[&entry];
            write!(
                itunes,
                "<dict><key>Track ID</key><integer>{id}</integer></dict>"
            )
            .unwrap();
        }
        itunes.push_str("</array></dict>\n");
    }
    itunes.push_str("</array></dict></plist>\n");
    Ok(Document {
        body: itunes,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_metadata_and_keeps_playlist_occurrence_order() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Música #1 & 2.mp3");
        std::fs::write(&path, b"fixture").unwrap();
        let track = Track {
            local_path: Some(path.to_string_lossy().into()),
            title: "A & <B> \"live\"\u{0001}".into(),
            artist: "Ana".into(),
            bpm: Some(119.5),
            duration_seconds: Some(182.4),
            ..Track::default()
        };
        let playlists = vec![
            (
                "a".into(),
                "Mix & edits".into(),
                vec![track.clone(), Track::default(), track.clone()],
            ),
            ("b".into(), "mix & edits".into(), vec![track]),
        ];
        let target = target_root("file:///D:/DJ%20Music").unwrap().unwrap();
        let documents = render(playlists, root.path(), Some(&target)).unwrap();
        assert_eq!(documents.skipped, 1);
        assert!(documents.body.contains("<string>Mix &amp; edits</string>"));
        assert!(documents
            .body
            .contains("<string>mix &amp; edits (2)</string>"));
        assert!(documents
            .body
            .contains("A &amp; &lt;B&gt; &quot;live&quot;�"));
        assert!(documents
            .body
            .contains("file:///D:/DJ%20Music/M%C3%BAsica%20%231%20&amp;"));
        assert!(documents
            .body
            .contains("<key>Total Time</key><integer>182400</integer>"));
        assert_eq!(documents.body.matches("<key>Track ID</key>").count(), 4);
    }

    #[test]
    fn path_remapping_refuses_external_and_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let external = other.path().join("outside.mp3");
        std::fs::write(&external, b"fixture").unwrap();
        let target = target_root("file://dj-host/share/Music").unwrap().unwrap();
        assert!(location(&external, root.path(), Some(&target)).is_none());
        assert!(location(&root.path().join("missing.mp3"), root.path(), None).is_none());
        assert!(location(&external, root.path(), None).is_some());
        for bad in [
            "https://example.org/Music",
            "file:///Music?v=2",
            "file:///Music#x",
        ] {
            assert!(target_root(bad).is_err(), "{bad}");
        }
    }
}
