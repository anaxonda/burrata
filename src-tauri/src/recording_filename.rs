//! Pure parsing and validation for peer-supplied audio filenames.
//!
//! These observations stay separate from catalog metadata. Callers must retain
//! raw names and use the verified helper before treating them as weak evidence.
use serde_json::Value;

/// Weak, explicitly unverified observations parsed from a peer's shared path.
/// Keep the original Candidate.title and CandidateSource filename intact. These
/// fields are consumed only by the shared matcher through its Soulseek-specific
/// uploader-caption evidence path; they are not catalog metadata.
pub(crate) fn filename_observations(filename: &str, stem: &str) -> Value {
    let parsed = parse_track_prefix(stem);
    let path_components = filename
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let components = if filename.len() <= 2048
        && path_components.len() <= 32
        && path_components
            .iter()
            .all(|component| component.len() <= 256)
    {
        path_components
    } else {
        Vec::new()
    };
    let path_dirs = components
        .iter()
        .take(components.len().saturating_sub(1))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let music_index = path_dirs
        .iter()
        .position(|component| component.eq_ignore_ascii_case("music"));
    let context_indices = match music_index {
        Some(index) => (index + 2 < path_dirs.len()).then_some((index + 1, index + 2)),
        None => (|| {
            let last = path_dirs.len().checked_sub(1)?;
            if is_disc_directory(path_dirs[last]) {
                let album = last.checked_sub(1)?;
                let artist = last.checked_sub(2)?;
                Some((artist, album))
            } else {
                let album = last;
                let artist = last.checked_sub(1)?;
                Some((artist, album))
            }
        })(),
    };
    let (folder_artist, folder_album) = context_indices
        .and_then(|(artist_index, album_index)| {
            let artist = path_dirs.get(artist_index)?;
            let album = path_dirs.get(album_index)?;
            (!artist.trim().is_empty() && !album.trim().is_empty())
                .then(|| (Some((*artist).to_owned()), Some((*album).to_owned())))
        })
        .unwrap_or((None, None));

    serde_json::json!({
        "rawBasename": stem,
        "normalizedTitle": parsed.as_ref().map(|prefix| prefix.title.as_str()).unwrap_or(stem),
        "trackNumberPrefix": parsed.as_ref().map(|prefix| prefix.raw_prefix.as_str()),
        "discNumber": parsed.as_ref().and_then(|prefix| prefix.disc_number),
        "trackNumber": parsed.as_ref().map(|prefix| prefix.track_number),
        "folderArtist": folder_artist,
        "folderAlbum": folder_album,
        "pathComponents": components,
        "authority": "uploaderCaption",
        "creditsComplete": false,
    })
}

fn is_disc_directory(component: &str) -> bool {
    let component = component.trim();
    let lower = component.to_ascii_lowercase();
    let Some(suffix) = ["disc", "disk", "cd"]
        .into_iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
    else {
        return false;
    };
    let number = suffix.trim_start_matches([' ', '-', '_']);
    !number.is_empty()
        && number.len() <= 2
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && number
            .parse::<u8>()
            .is_ok_and(|number| (1..=99).contains(&number))
}

/// Recompute the bounded weak filename observations only when the caller's raw
/// Candidate.title is exactly the stem of the validated Soulseek source path.
/// Shared matching uses this to reject forged or stale observation metadata.
pub(crate) fn verified_filename_observations(filename: &str, raw_basename: &str) -> Option<Value> {
    validate_remote_path(filename).ok()?;
    audio_extension(filename).ok()?;
    let actual_stem = filename.rsplit(['/', '\\']).next()?.rsplit_once('.')?.0;
    (actual_stem == raw_basename).then(|| filename_observations(filename, raw_basename))
}

/// Return an exact peer-directory grouping key after validating the same
/// remote path grammar used by transfers. Separators are canonicalized while
/// component spelling and case remain intact.
pub(crate) fn remote_parent_directory_key(filename: &str) -> Option<String> {
    validate_remote_path(filename).ok()?;
    let components = filename
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.len() < 2 {
        return None;
    }
    Some(components[..components.len() - 1].join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackPrefix {
    raw_prefix: String,
    disc_number: Option<u32>,
    track_number: u32,
    title: String,
}

/// Parse only conventional leading track-number forms. Ambiguous single-digit
/// and non-zero two-digit prefixes followed by a space remain part of the title.
fn parse_track_prefix(value: &str) -> Option<TrackPrefix> {
    fn digits(input: &str, max: usize) -> Option<(u32, usize)> {
        let count = input.bytes().take_while(u8::is_ascii_digit).count();
        if count == 0 || count > max {
            return None;
        }
        Some((input[..count].parse().ok()?, count))
    }
    fn valid_title(value: &str) -> bool {
        value
            .trim_start()
            .chars()
            .next()
            .is_some_and(char::is_alphabetic)
    }
    fn separated_title(value: &str, zero_padded: bool) -> Option<(&str, usize)> {
        for separator in [" - ", ". "] {
            if let Some(title) = value.strip_prefix(separator) {
                if valid_title(title) {
                    return Some((title.trim(), separator.len()));
                }
            }
        }
        if zero_padded {
            let title = value.strip_prefix(' ')?;
            if valid_title(title) {
                return Some((title.trim(), 1));
            }
        }
        None
    }

    // Disc-track form, for example `1-01 - Title` or `2-03. Title`.
    if let Some((disc, disc_len)) = digits(value, 2) {
        if let Some(after_disc_dash) = value[disc_len..].strip_prefix('-') {
            if let Some((track, track_len)) = digits(after_disc_dash, 2) {
                let raw_end = disc_len + 1 + track_len;
                if let Some((title, _)) = separated_title(
                    &value[raw_end..],
                    track_len == 2 && after_disc_dash.starts_with('0'),
                ) {
                    return Some(TrackPrefix {
                        raw_prefix: value[..raw_end].to_owned(),
                        disc_number: Some(disc),
                        track_number: track,
                        title: title.to_owned(),
                    });
                }
            }
        }
    }

    let (track, count) = digits(value, 2)?;
    let suffix = &value[count..];
    let zero_padded = count == 2 && value.starts_with('0') && track > 0;
    let (title, separator_len) = separated_title(suffix, zero_padded)?;
    Some(TrackPrefix {
        raw_prefix: value[..count + separator_len].trim_end().to_owned(),
        disc_number: None,
        track_number: track,
        title: title.to_owned(),
    })
}
pub fn validate_remote_path(path: &str) -> Result<(), String> {
    let path = path.replace('\\', "/");
    if path.trim().is_empty()
        || path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
        || path.split('/').any(|part| part == ".." || part.is_empty())
        || path.contains('\0')
    {
        return Err("Invalid Soulseek remote filename".into());
    }
    Ok(())
}

pub(crate) fn audio_extension(path: &str) -> Result<&str, String> {
    let basename = path.rsplit('/').next().ok_or("Invalid Soulseek filename")?;
    let extension = basename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .ok_or("Selected Soulseek file has no extension")?;
    if ![
        "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "aiff", "wma",
    ]
    .iter()
    .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    {
        return Err("Selected Soulseek file is not a supported audio format".into());
    }
    Ok(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_peer_filename_prefixes_are_conservative() {
        for (raw, expected, disc, track) in [
            ("01 Pilentze Pee", "Pilentze Pee", None, 1),
            ("12 - Pilentze Pee", "Pilentze Pee", None, 12),
            ("12. Pilentze Pee", "Pilentze Pee", None, 12),
            ("1-01 - Pilentze Pee", "Pilentze Pee", Some(1), 1),
        ] {
            let parsed = parse_track_prefix(raw).expect(raw);
            assert_eq!(parsed.title, expected, "{raw}");
            assert_eq!(parsed.disc_number, disc, "{raw}");
            assert_eq!(parsed.track_number, track, "{raw}");
        }

        for meaningful in ["1 Thing", "99 Problems", "1979", "1, 2, 3", "1 - 2, 3"] {
            assert!(parse_track_prefix(meaningful).is_none(), "{meaningful}");
        }
    }

    #[test]
    fn filename_observations_keep_raw_text_and_use_weak_bounded_folder_context() {
        let observations = filename_observations(
            r"Music\Bulgarian State Television Female Choir\Le Mystere\01 - Pilentze Pee.flac",
            "01 - Pilentze Pee",
        );
        assert_eq!(observations["rawBasename"], "01 - Pilentze Pee");
        assert_eq!(observations["normalizedTitle"], "Pilentze Pee");
        assert_eq!(observations["trackNumber"], 1);
        assert_eq!(
            observations["folderArtist"],
            "Bulgarian State Television Female Choir"
        );
        assert_eq!(observations["folderAlbum"], "Le Mystere");
        assert_eq!(observations["authority"], "uploaderCaption");
        assert_eq!(observations["creditsComplete"], false);
        assert_eq!(
            verified_filename_observations(
                r"Music\Bulgarian State Television Female Choir\Le Mystere\01 - Pilentze Pee.flac",
                "01 - Pilentze Pee",
            ),
            Some(observations.clone())
        );
        assert!(verified_filename_observations(
            r"Music\Bulgarian State Television Female Choir\Le Mystere\01 - Pilentze Pee.flac",
            "Pilentze Pee",
        )
        .is_none());

        let no_music_root = filename_observations(
            "Root/Artist/Album/01 - Pilentze Pee.flac",
            "01 - Pilentze Pee",
        );
        assert_eq!(no_music_root["folderArtist"], "Artist");
        assert_eq!(no_music_root["folderAlbum"], "Album");
        assert_eq!(no_music_root["authority"], "uploaderCaption");
        assert_eq!(no_music_root["creditsComplete"], false);

        let disc_folder = filename_observations(
            "Root/Artist/Album/Disc 1/01 - Pilentze Pee.flac",
            "01 - Pilentze Pee",
        );
        assert_eq!(disc_folder["folderArtist"], "Artist");
        assert_eq!(disc_folder["folderAlbum"], "Album");
        assert!(
            filename_observations("Album/Disc 1/01 - Song.flac", "01 - Song")["folderArtist"]
                .is_null()
        );
        // A literal Music component remains authoritative; if its pair is
        // incomplete, don't reinterpret unrelated parent directories.
        let incomplete_music_anchor = filename_observations(
            "Root/Music/Artist/01 - Pilentze Pee.flac",
            "01 - Pilentze Pee",
        );
        assert!(incomplete_music_anchor["folderArtist"].is_null());
        assert!(incomplete_music_anchor["folderAlbum"].is_null());
    }

    #[test]
    fn parent_directory_key_canonicalizes_separators_without_merging_distinct_directories() {
        assert_eq!(
            remote_parent_directory_key(r"Music\Artist\Album\01 - Song.flac"),
            Some("Music/Artist/Album".into())
        );
        assert_eq!(
            remote_parent_directory_key("Music/Artist/Album/02 - Song.flac"),
            Some("Music/Artist/Album".into())
        );
        assert_ne!(
            remote_parent_directory_key("Music/Artist/Album Deluxe/02 - Song.flac"),
            Some("Music/Artist/Album".into())
        );
        assert!(remote_parent_directory_key("../Album/song.flac").is_none());
    }
}
