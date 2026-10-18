//! User imports preserve uncertainty: text names do not prove catalog identity.
use crate::models::Track;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub fn parse(text: &str, kind: &str) -> Result<Vec<Track>, String> {
    if text.len() > 2 * 1024 * 1024 {
        return Err("Imports are limited to 2 MB".into());
    }
    let text = text.trim_start_matches('\u{feff}');
    match kind {
        "csv" => csv_tracks(text),
        "tracklist" => tracklist(text),
        "m3u" | "m3u8" => m3u_tracks(text),
        _ => Err(
            "Use CSV, M3U, M3U8, or artist - title tracklist imports; chart URLs belong in Charts"
                .into(),
        ),
    }
}
/// M3U entries can contain local paths and network URLs. Import only the
/// human-readable EXTINF metadata; entry locations never become track fields.
fn m3u_tracks(text: &str) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut ids = HashSet::new();
    let mut entry_number = 0usize;
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if !line.starts_with("#EXTINF:") {
            continue;
        }
        entry_number += 1;
        let info = line
            .split_once(',')
            .map(|(_, value)| value.trim())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("Line {}: EXTINF entry has no track name", line_no + 1))?;
        let parts = info
            .split_once(" - ")
            .or_else(|| info.split_once(" — "))
            .or_else(|| info.split_once(" – "))
            .ok_or_else(|| format!("Line {}: EXTINF name must use Artist - Title", line_no + 1))?;
        let (title, version) = split_version(parts.1);
        let mut track = make_track(parts.0, &title, version)
            .map_err(|e| format!("Line {}: {e}", line_no + 1))?;
        track.provenance["importFormat"] = serde_json::json!("m3u");
        track.provenance["playlistPosition"] = serde_json::json!(entry_number);
        if ids.insert(track.id.clone()) {
            tracks.push(track);
        }
        if tracks.len() > 10000 {
            return Err("Import is limited to 10,000 tracks".into());
        }
    }
    if tracks.is_empty() {
        return Err("M3U playlist contains no EXTINF track metadata".into());
    }
    Ok(tracks)
}
fn make_track(artist: &str, title: &str, version: Option<String>) -> Result<Track, String> {
    make_track_with_missing_artist(artist, title, version, false)
}
fn make_track_with_missing_artist(
    artist: &str,
    title: &str,
    version: Option<String>,
    allow_missing_artist: bool,
) -> Result<Track, String> {
    let artist = artist.trim();
    let title = title.trim();
    if (!allow_missing_artist && artist.is_empty()) || title.is_empty() {
        return Err(if title.is_empty() {
            "Track title is required".into()
        } else {
            "Artist is required".into()
        });
    }
    let identity = format!(
        "{}\n{}\n{}",
        artist.to_lowercase(),
        title.to_lowercase(),
        version.as_deref().unwrap_or("").trim().to_lowercase()
    );
    let id = format!("{:x}", Sha256::digest(identity.as_bytes()));
    Ok(Track {
        id: format!("import:{id}"),
        provider: "import".into(),
        provider_id: id,
        artist: artist.into(),
        title: title.into(),
        version,
        provenance: serde_json::json!({"identity":"unverified user import"}),
        ..Default::default()
    })
}
fn split_version(title: &str) -> (String, Option<String>) {
    let title = title.trim();
    if let Some((prefix, version)) = crate::matching::split_balanced_trailing_parenthetical(title) {
        let tokens = version
            .to_lowercase()
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if [
            "mix",
            "remix",
            "edit",
            "version",
            "dub",
            "instrumental",
            "extended",
            "original",
        ]
        .iter()
        .any(|marker| tokens.iter().any(|token| token == marker))
        {
            return (prefix.into(), Some(version.into()));
        }
    }
    (title.into(), None)
}
fn tracklist(text: &str) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::new();
    let mut ids = HashSet::new();
    for (line_no, line) in text.lines().enumerate() {
        let mut line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(index) = line.find(|c: char| !c.is_ascii_digit()) {
            let tail = &line[index..];
            if index > 0 && (tail.starts_with(". ") || tail.starts_with(") ")) {
                line = tail[2..].trim();
            }
        }
        let parts = line
            .split_once(" - ")
            .or_else(|| line.split_once(" — "))
            .or_else(|| line.split_once(" – "))
            .ok_or_else(|| format!("Line {}: expected Artist - Title (Version)", line_no + 1))?;
        let (title, version) = split_version(parts.1);
        let t = make_track(parts.0, &title, version)
            .map_err(|e| format!("Line {}: {e}", line_no + 1))?;
        if ids.insert(t.id.clone()) {
            tracks.push(t)
        }
        if tracks.len() > 10000 {
            return Err("Import is limited to 10,000 tracks".into());
        }
    }
    if tracks.is_empty() {
        return Err("No tracks found in the import".into());
    }
    Ok(tracks)
}
fn csv_tracks(text: &str) -> Result<Vec<Track>, String> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let column = |names: &[&str]| {
        headers
            .iter()
            .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
    };
    let artist = column(&["artist", "artists", "artist name", "artist name(s)"])
        .ok_or("CSV requires an artist column")?;
    let title = column(&["title", "track", "track title", "track name"])
        .ok_or("CSV requires a title column")?;
    let spotify_uri = column(&[
        "track uri",
        "track_uri",
        "spotify uri",
        "spotify_uri",
        "spotify track id",
    ]);
    let album = column(&["album", "album name"]);
    let release_date = column(&["release date", "album release date", "release_date"]);
    let artwork = column(&["artwork_url", "album image url"]);
    let isrc = column(&["isrc"]);
    let duration_ms = column(&["track duration (ms)", "duration (ms)", "duration_ms"]);
    let rank = column(&["rank", "chart rank", "chart position"]);
    let position = column(&["position", "track position", "track number", "track no."]);
    let spotify_export = spotify_uri.is_some() || column(&["artist name(s)"]).is_some();
    let version = column(&["version", "remix", "mix"]);
    let label = column(&["label"]);
    let genre = column(&["genre"]);
    let bpm = column(&["bpm"]);
    let key = column(&["key"]);
    let duration = column(&["duration", "duration_seconds"]);
    let url = column(&["url", "source_url", "sourceurl"]);
    let mut result = Vec::new();
    let mut ids = HashSet::new();
    for (n, row) in reader.records().enumerate() {
        let row = row.map_err(|e| format!("CSV row {}: {e}", n + 2))?;
        let value = |idx: Option<usize>| {
            idx.and_then(|i| row.get(i))
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let (title, parsed_version) = split_version(row.get(title).unwrap_or(""));
        // Real exports may leave artist credits blank for individual rows. Keep
        // the missing value empty instead of inventing an artist or rejecting
        // otherwise usable rows from the entire file.
        let mut t = make_track_with_missing_artist(
            row.get(artist).unwrap_or(""),
            &title,
            value(version).or(parsed_version),
            true,
        )?;
        t.album = value(album);
        t.release_date = value(release_date);
        t.isrc = value(isrc);
        if let Some(raw_rank) = value(rank) {
            t.provenance["sourceRank"] = serde_json::json!(raw_rank);
            if let Ok(value) = raw_rank.parse::<u32>() {
                if value > 0 {
                    t.rank = Some(value);
                }
            }
        }
        if let Some(raw_position) = value(position) {
            // Keep source notation such as Discogs' A1/B2 intact; it is not a
            // verified catalog rank and must not be replaced by row order.
            t.provenance["sourcePosition"] = serde_json::json!(raw_position);
        }
        if spotify_export {
            t.provenance["importFormat"] = serde_json::json!("spotify-csv");
            t.provenance["playlistPosition"] = serde_json::json!(n + 1);
        }
        if let Some(reference) = value(spotify_uri) {
            t.provenance["originalSpotifyReference"] = serde_json::json!(&reference);
            if let Some(id) =
                spotify_track_id(&reference).map_err(|e| format!("CSV row {}: {e}", n + 2))?
            {
                t.id = format!("spotify:{id}");
                t.provider = "spotify".into();
                t.provider_id = id.clone();
                t.source_url = format!("https://open.spotify.com/track/{id}");
            }
        }
        if let Some(image) = value(artwork) {
            let u = url::Url::parse(&image)
                .map_err(|_| format!("CSV row {}: invalid artwork URL", n + 2))?;
            if !matches!(u.scheme(), "https" | "http") {
                return Err("Artwork URLs must be HTTP(S)".into());
            }
            t.artwork_url = Some(image);
        }
        t.label = value(label);
        t.genre = value(genre);
        t.key = value(key).filter(|v| !spotify_export || v.parse::<i32>().is_err());
        if spotify_export {
            if let Some(raw_key) = value(key) {
                t.provenance["originalKey"] = serde_json::json!(raw_key);
            }
        }

        if let Some(v) = value(bpm) {
            let parsed_bpm: f64 = v
                .parse()
                .map_err(|_| format!("CSV row {}: invalid BPM", n + 2))?;
            if !parsed_bpm.is_finite() || parsed_bpm <= 0.0 {
                return Err(format!("CSV row {}: BPM must be positive", n + 2));
            }
            t.bpm = Some(parsed_bpm);
        }
        if let Some(v) = value(duration) {
            let n: f64 = if let Some((m, s)) = v.split_once(':') {
                m.parse::<f64>().map_err(|_| "Invalid duration minutes")? * 60.0
                    + s.parse::<f64>().map_err(|_| "Invalid duration seconds")?
            } else {
                v.parse().map_err(|_| "Invalid duration")?
            };
            if !n.is_finite() || n <= 0.0 {
                return Err("Duration must be positive".into());
            }
            t.duration_seconds = Some(n);
        }
        if let Some(ms) = value(duration_ms) {
            let milliseconds: f64 = ms
                .parse()
                .map_err(|_| format!("CSV row {}: invalid duration milliseconds", n + 2))?;
            if !milliseconds.is_finite() || milliseconds <= 0.0 {
                return Err(format!(
                    "CSV row {}: duration milliseconds must be positive",
                    n + 2
                ));
            }
            t.duration_seconds = Some(milliseconds / 1000.0);
        }
        if let Some(v) = value(url) {
            let parsed =
                url::Url::parse(&v).map_err(|_| format!("CSV row {}: invalid URL", n + 2))?;
            if !matches!(parsed.scheme(), "https" | "http") {
                return Err("CSV source URLs must be HTTP(S)".into());
            }
            if t.provider == "spotify" {
                t.provenance["additionalSourceUrl"] = serde_json::json!(v);
            } else {
                t.source_url = v;
            }
        }
        if ids.insert(t.id.clone()) {
            result.push(t)
        }
        if result.len() > 10000 {
            return Err("Import is limited to 10,000 tracks".into());
        }
    }
    if result.is_empty() {
        return Err("CSV contains no tracks".into());
    }
    Ok(result)
}
/// CSV reference IDs are checked locally; this does not authenticate Spotify access.
fn spotify_track_id(reference: &str) -> Result<Option<String>, String> {
    let reference = reference.trim();
    if reference.starts_with("spotify:local:") {
        return Ok(None);
    }
    let id = if let Some(id) = reference.strip_prefix("spotify:track:") {
        id.to_string()
    } else if reference.len() == 22 && reference.bytes().all(|b| b.is_ascii_alphanumeric()) {
        reference.to_string()
    } else {
        let parsed =
            url::Url::parse(reference).map_err(|_| "Invalid Spotify track URI, URL, or ID")?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("open.spotify.com")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some_and(|p| p != 443)
        {
            return Err("Spotify track URLs must use https://open.spotify.com".into());
        }
        let parts = parsed
            .path_segments()
            .ok_or("Invalid Spotify track URL")?
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        match parts.as_slice() {
            ["track", id] => id.to_string(),
            [locale, "track", id] if locale.starts_with("intl-") => id.to_string(),
            _ => return Err("Expected an individual Spotify music track reference".into()),
        }
    };
    if id.len() != 22 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Spotify track IDs must contain 22 letters or digits".into());
    }
    Ok(Some(id))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_splits_a_balanced_nested_main_version_without_inventing_duration() {
        let rows = parse(
            "Artist,Title\nTimmy Regisford,\"Sasa (Main Version (4:41))\"",
            "csv",
        )
        .unwrap();
        assert_eq!(rows[0].title, "Sasa");
        assert_eq!(rows[0].version.as_deref(), Some("Main Version (4:41)"));
        assert_eq!(rows[0].duration_seconds, None);
        assert_eq!(rows[0].provenance["identity"], "unverified user import");
    }

    #[test]
    fn import_version_markers_must_be_whole_tokens() {
        let rows = parse("Artist,Title\nArtist,\"Song (Diversion (4:41))\"", "csv").unwrap();
        assert_eq!(rows[0].title, "Song (Diversion (4:41))");
        assert_eq!(rows[0].version, None);
    }

    #[test]
    fn duplicates_are_exact_and_versions_distinct() {
        let t = parse(
            "1. A - B (Extended Mix)\nA - B (Extended Mix)\nA - B (Radio Edit)",
            "tracklist",
        )
        .unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].title, "B");
        assert_eq!(t[0].version.as_deref(), Some("Extended Mix"));
    }
    #[test]
    fn m3u_imports_extinf_metadata_and_discards_entry_locations() {
        let tracks = parse(
            "#EXTM3U\n#EXTINF:245,Artist - Night (Extended Mix)\n../private/Night.flac\n#EXTINF:-1,Other — Dawn\nhttps://example.test/audio.mp3?token=secret",
            "m3u8",
        ).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].artist, "Artist");
        assert_eq!(tracks[0].title, "Night");
        assert_eq!(tracks[0].version.as_deref(), Some("Extended Mix"));
        assert_eq!(tracks[0].provenance["playlistPosition"], 1);
        assert!(tracks
            .iter()
            .all(|t| t.local_path.is_none() && t.source_url.is_empty()));
        let serialized = serde_json::to_string(&tracks).unwrap();
        assert!(!serialized.contains("private"));
        assert!(!serialized.contains("token"));
    }
    #[test]
    fn m3u_without_extinf_metadata_is_rejected() {
        assert!(parse("#EXTM3U\n/song.flac", "m3u").is_err());
        assert!(parse("#EXTINF:123,Missing separator\n/song.flac", "m3u").is_err());
    }
    #[test]
    fn csv_handles_commas_and_missing_fields() {
        let t = parse(
            "artist,title,version,bpm,key\n\"Artist, Other\",Title,Original Mix,123.5,Am",
            "csv",
        )
        .unwrap();
        assert_eq!(t[0].artist, "Artist, Other");
        assert_eq!(t[0].bpm, Some(123.5));
        assert_eq!(t[0].duration_seconds, None);
    }
    #[test]
    fn csv_preserves_title_when_artist_credit_is_blank_without_guessing() {
        let tracks = parse(
            "Artist,Title,Position\n,Uncredited recording,1\nKnown Artist,Known recording,2",
            "csv",
        )
        .unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks[0].artist.is_empty());
        assert_eq!(tracks[0].title, "Uncredited recording");
        assert_eq!(tracks[0].provenance["identity"], "unverified user import");
        assert_eq!(tracks[1].artist, "Known Artist");
        assert!(parse("Artist,Title\nArtist,\n", "csv").is_err());
    }
    #[test]
    fn csv_preserves_explicit_rank_and_disc_position_notation() {
        let tracks = parse(
            "Artist,Title,Rank,Position\nArtist,Track one,3,A1\nArtist,Track two,0,B2",
            "csv",
        )
        .unwrap();
        assert_eq!(tracks[0].rank, Some(3));
        assert_eq!(tracks[0].provenance["sourceRank"], "3");
        assert_eq!(tracks[0].provenance["sourcePosition"], "A1");
        assert_eq!(tracks[1].rank, None);
        assert_eq!(tracks[1].provenance["sourcePosition"], "B2");
    }
    #[test]
    fn bad_row_fails_without_partial_import() {
        assert!(parse("A - B\ninvalid", "tracklist").is_err());
        assert!(parse("artist,title,bpm\nA,B,NaN", "csv").is_err());
    }
    #[test]
    fn spotify_exports_keep_identity_order_and_metadata() {
        let text = "Track URI,Track Name,Artist Name(s),Album Name,Track Duration (ms),ISRC,Album Release Date,Key\nspotify:track:0123456789ABCDEFGHIJKL,Night (Extended Mix),Artist;Other,Release,360500,TESTISRC,2026-01-01,0\nspotify:track:ABCDEFGHIJKLMNOPQRSTUV,Night (Extended Mix),Artist;Other,Release,360500,,,";
        let tracks = parse(text, "csv").unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].provider, "spotify");
        assert_eq!(tracks[0].id, "spotify:0123456789ABCDEFGHIJKL");
        assert_eq!(tracks[0].title, "Night");
        assert_eq!(tracks[0].version.as_deref(), Some("Extended Mix"));
        assert_eq!(tracks[0].duration_seconds, Some(360.5));
        assert_eq!(tracks[0].album.as_deref(), Some("Release"));
        assert_eq!(tracks[0].isrc.as_deref(), Some("TESTISRC"));
        assert_eq!(tracks[0].rank, None);
        assert_eq!(tracks[0].key, None);
        assert_eq!(tracks[0].provenance["playlistPosition"], 1);
        assert_eq!(tracks[1].provenance["playlistPosition"], 2);
    }
    #[test]
    fn spotify_links_are_canonical_and_untrusted_hosts_rejected() {
        assert_eq!(
            spotify_track_id(
                "https://open.spotify.com/intl-en/track/0123456789ABCDEFGHIJKL?si=ignored"
            )
            .unwrap()
            .as_deref(),
            Some("0123456789ABCDEFGHIJKL")
        );
        assert!(spotify_track_id(
            "https://open.spotify.com.attacker.test/track/0123456789ABCDEFGHIJKL"
        )
        .is_err());
        assert!(spotify_track_id("spotify:episode:0123456789ABCDEFGHIJKL").is_err());
        assert!(spotify_track_id("spotify:track:short").is_err());
    }
    #[test]
    fn repeated_spotify_identity_deduplicates_and_local_files_remain_unverified() {
        let tracks=parse("Track URI,Track Name,Artist Name(s)\nspotify:track:0123456789ABCDEFGHIJKL,Title,Artist\nspotify:track:0123456789ABCDEFGHIJKL,Title,Artist\nspotify:local:Artist:Album:Local:240,Local,Artist","csv").unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].provider, "import");
        assert!(tracks[1].source_url.is_empty());
    }
    #[test]
    fn invalid_spotify_duration_fails_the_import() {
        assert!(parse("Track URI,Track Name,Artist Name(s),Track Duration (ms)\nspotify:track:0123456789ABCDEFGHIJKL,Title,Artist,NaN","csv").is_err());
    }
}
