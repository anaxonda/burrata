//! Conservative title parsing for explicitly named live performances.
use crate::models::Track;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePerformanceInference {
    pub artist: String,
    pub title: String,
    pub version: String,
    pub confidence: &'static str,
}

/// Parse explicit performer/song/event titles; never infer identity from an uploader.
/// The quoted work and named performer are required; uploaders and channel names
/// are never considered artist evidence.
pub fn infer_live_performance(
    source_title: &str,
    expected_artist: Option<&str>,
    expected_title: Option<&str>,
) -> Option<LivePerformanceInference> {
    let source_title = source_title.trim();
    if let Some(inferred) = infer_named_live_version(source_title, expected_artist, expected_title)
    {
        return Some(inferred);
    }
    let lower = source_title.to_ascii_lowercase();
    let perform_at = [" performs ", " perform ", " performing "]
        .iter()
        .filter_map(|needle| lower.find(needle).map(|index| (index, *needle)))
        .min_by_key(|(index, _)| *index)?;
    let performer = source_title[..perform_at.0].trim();
    if performer.is_empty() || performer.contains(['|', ':']) {
        return None;
    }
    if let Some(expected_artist) = expected_artist.filter(|value| !value.trim().is_empty()) {
        if normalize(performer) != normalize(expected_artist) {
            return None;
        }
    }

    let rest = source_title[perform_at.0 + perform_at.1.len()..].trim();
    let quote_start = rest.find(['“', '"', '‘', '\''])?;
    let quote_open = rest[quote_start..].chars().next()?;
    let quote_close = match quote_open {
        '“' => '”',
        '‘' => '’',
        other => other,
    };
    let quoted = &rest[quote_start + quote_open.len_utf8()..];
    let quote_end = quoted.find(quote_close)?;
    let title = quoted[..quote_end].trim();
    if title.is_empty()
        || expected_title.is_some_and(|expected| normalize(title) != normalize(expected))
    {
        return None;
    }
    let suffix = quoted[quote_end + quote_close.len_utf8()..]
        .trim()
        .trim_start_matches(['|', '—', '-', ':'])
        .trim();
    let suffix = suffix
        .strip_prefix("at ")
        .or_else(|| suffix.strip_prefix("At "))
        .unwrap_or(suffix)
        .trim();
    let suffix_lower = suffix.to_ascii_lowercase();
    if suffix.is_empty()
        || !suffix_lower.contains("festival")
        || !suffix
            .split(|character: char| !character.is_ascii_digit())
            .any(|part| {
                part.len() == 4 && part.starts_with("19")
                    || part.len() == 4 && part.starts_with("20")
            })
    {
        return None;
    }
    Some(LivePerformanceInference {
        artist: performer.to_owned(),
        title: title.to_owned(),
        version: format!("Live at {suffix}"),
        confidence: "explicit_performer_quoted_track_festival_title",
    })
}

/// A named event/venue is required. Bare `(Live)` and unmarked titles remain
/// ambiguous, and a selected studio/remix title must match before any inference.
fn infer_named_live_version(
    source_title: &str,
    expected_artist: Option<&str>,
    expected_title: Option<&str>,
) -> Option<LivePerformanceInference> {
    let separators = [" — ", " – ", " - "];
    let (separator_at, separator) = separators
        .iter()
        .filter_map(|separator| {
            source_title
                .find(*separator)
                .map(|index| (index, *separator))
        })
        .min_by_key(|(index, _)| *index)?;
    let artist = &source_title[..separator_at];
    let rest = &source_title[separator_at + separator.len()..];
    let artist = artist.trim();
    if artist.is_empty() || artist.contains(['|', ':']) {
        return None;
    }
    if expected_artist
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|expected| normalize(expected) != normalize(artist))
    {
        return None;
    }
    let (title, version) = rest.rsplit_once(" (")?;
    let title = strip_surrounding_quotes(title.trim());
    let version = version.strip_suffix(')')?.trim();
    let lower = version.to_ascii_lowercase();
    let event = if lower.starts_with("live at ") {
        &version[8..]
    } else if lower.starts_with("live on ") {
        &version[8..]
    } else {
        return None;
    };
    if title.is_empty()
        || event.trim().is_empty()
        || expected_title
            .filter(|value| !value.trim().is_empty())
            .is_some_and(|expected| normalize(expected) != normalize(title))
    {
        return None;
    }
    Some(LivePerformanceInference {
        artist: artist.to_owned(),
        title: title.to_owned(),
        version: version.to_owned(),
        confidence: "explicit_artist_track_named_live_version",
    })
}

fn strip_surrounding_quotes(value: &str) -> &str {
    let pairs = [("\"", "\""), ("'", "'"), ("“", "”"), ("‘", "’")];
    pairs
        .iter()
        .find_map(|(open, close)| {
            value
                .strip_prefix(*open)
                .and_then(|inner| inner.strip_suffix(*close))
        })
        .unwrap_or(value)
        .trim()
}

/// Add a performance version without replacing any selected/catalog identity.
/// Source title and URL are retained for review; event dates are not release dates.
pub fn normalize_selected_track(
    track: &mut Track,
    source_title: &str,
    source_url: &str,
) -> Option<LivePerformanceInference> {
    let expected_artist = (!track.artist.trim().is_empty()).then_some(track.artist.as_str());
    let title_is_source = normalize(&track.title) == normalize(source_title);
    let expected_title =
        (!track.title.trim().is_empty() && !title_is_source).then_some(track.title.as_str());
    let inferred = infer_live_performance(source_title, expected_artist, expected_title)?;
    if !track.provenance.is_object() {
        track.provenance = Value::Object(Default::default());
    }
    if track.artist.trim().is_empty() {
        track.artist = inferred.artist.clone();
        track.provenance["artistSource"] = json!("explicit_performance_title");
    }
    if track.title.trim().is_empty() || title_is_source {
        track.title = inferred.title.clone();
    }
    if track
        .version
        .as_deref()
        .map_or(true, |version| version.trim().is_empty())
    {
        track.version = Some(inferred.version.clone());
    }
    track.provenance["sourceTitle"] = json!(source_title);
    track.provenance["sourceURL"] = json!(source_url);
    track.provenance["titleInference"] = json!({
        "type":"livePerformance",
        "version":inferred.version,
        "confidence":inferred.confidence,
        "eventDateIsReleaseDate":false
    });
    Some(inferred)
}

/// Create a source-specific YouTube identity for a live performance selected
/// from a catalog recording. The catalog listing remains untouched in storage;
/// its identity and metadata are copied into provenance for review.
pub fn normalize_catalog_live_download(
    track: &mut Track,
    source_title: &str,
    source_url: &str,
    video_id: &str,
    source_artwork_url: Option<&str>,
    duration_seconds: Option<f64>,
) -> Result<bool, String> {
    let Some(inferred) = infer_live_performance(source_title, None, None) else {
        return Ok(false);
    };
    if normalize(&track.artist) != normalize(&inferred.artist)
        || normalize(&track.title) != normalize(&inferred.title)
    {
        return Err(format!(
            "The live source identifies {} — {}, not the selected {} — {}; review the recording match before downloading",
            inferred.artist, inferred.title, track.artist, track.title
        ));
    }
    if let Some(existing_version) = track
        .version
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if normalize(existing_version) != normalize(&inferred.version) {
            return Err(format!(
                "The selected recording version ({existing_version}) does not match the live source ({}); review the version before downloading",
                inferred.version
            ));
        }
    }
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Invalid YouTube video identity".into());
    }
    let selected_catalog = serde_json::to_value(&*track).map_err(|error| error.to_string())?;
    track.id = format!("youtube:{video_id}");
    track.provider = "youtube".into();
    track.provider_id = video_id.into();
    track.source_url = source_url.into();
    track.artist = inferred.artist.clone();
    track.title = inferred.title.clone();
    track.version = Some(inferred.version.clone());
    track.album = None;
    track.release_date = None;
    track.isrc = None;
    track.label = None;
    track.catalog_number = None;
    track.bpm = None;
    track.key = None;
    track.artwork_url = source_artwork_url.map(str::to_owned);
    track.duration_seconds = duration_seconds;
    track.favorite = false;
    track.disposition = None;
    track.local_path = None;
    track.asset_id = None;
    track.local_availability = "none".into();
    track.provenance = json!({
        "sourceKind":"youtubeLivePerformance",
        "identity":"source-specific live performance; composition relation is unverified",
        "sourceTitle":source_title,
        "sourceURL":source_url,
        "titleInference":{
            "type":"livePerformance",
            "version":inferred.version,
            "confidence":inferred.confidence,
            "eventDateIsReleaseDate":false
        },
        "selectedCatalogTrack":selected_catalog,
        "catalogRelation":"selected composition metadata retained for review; not applied to live performance"
    });
    Ok(true)
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_live_versions_are_inferred_without_guessing_a_release() {
        for separator in [" - ", " — ", " – "] {
            let source =
                format!("Big Thief{separator}Mary (Live at Pitchfork Music Festival 2018)");
            let inferred =
                infer_live_performance(&source, Some("Big Thief"), Some("Mary")).unwrap();
            assert_eq!(inferred.artist, "Big Thief");
            assert_eq!(inferred.title, "Mary");
            assert_eq!(inferred.version, "Live at Pitchfork Music Festival 2018");
        }
        let mixed_separators = infer_live_performance(
            "Big Thief – Mary — Part (Live at Pitchfork Music Festival 2018)",
            Some("Big Thief"),
            Some("Mary — Part"),
        )
        .unwrap();
        assert_eq!(mixed_separators.artist, "Big Thief");
        assert_eq!(mixed_separators.title, "Mary — Part");
        let quoted = infer_live_performance(
            "Big Thief - ‘Mary’ (Live at Pitchfork Music Festival 2018)",
            Some("Big Thief"),
            Some("Mary"),
        )
        .unwrap();
        assert_eq!(quoted.title, "Mary");
        assert!(infer_live_performance("Big Thief - Mary (Live on KEXP)", None, None).is_some());
        for source in [
            "Big Thief - Mary (Live)",
            "Big Thief - Mary (Live at )",
            "Big Thief - Mary (Official Video)",
            "Big Thief - Paul (Live on KEXP)",
            "Other Artist - Mary (Live on KEXP)",
        ] {
            assert!(infer_live_performance(source, Some("Big Thief"), Some("Mary")).is_none());
        }
    }

    #[test]
    fn infers_pitchfork_live_version_without_treating_event_year_as_release_date() {
        let inference = infer_live_performance(
            "Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            Some("Big Thief"),
            Some("Mary"),
        )
        .unwrap();
        assert_eq!(inference.artist, "Big Thief");
        assert_eq!(inference.title, "Mary");
        assert_eq!(inference.version, "Live at Pitchfork Music Festival 2018");

        let mut track = Track {
            artist: "Big Thief".into(),
            title: "Mary".into(),
            ..Default::default()
        };
        normalize_selected_track(
            &mut track,
            "Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            "https://www.youtube.com/watch?v=abc",
        );
        assert_eq!(track.artist, "Big Thief");
        assert_eq!(track.title, "Mary");
        assert_eq!(
            track.version.as_deref(),
            Some("Live at Pitchfork Music Festival 2018")
        );
        assert!(track.release_date.is_none());
        assert_eq!(
            track.provenance["sourceURL"],
            "https://www.youtube.com/watch?v=abc"
        );
        assert_eq!(
            track.provenance["titleInference"]["eventDateIsReleaseDate"],
            false
        );
    }

    #[test]
    fn rejects_ambiguous_titles_and_preserves_existing_catalog_version() {
        for title in [
            "Pitchfork Music Festival 2018: Mary by Big Thief",
            "Big Thief - Mary (Live)",
            "Big Thief Performs “Mary” | Official Video",
            "Channel Performs “Mary” | Music Festival 2018",
        ] {
            assert!(infer_live_performance(title, Some("Big Thief"), Some("Mary")).is_none());
        }
        let mut track = Track {
            artist: "Big Thief".into(),
            title: "Mary".into(),
            version: Some("Live on KEXP".into()),
            ..Default::default()
        };
        normalize_selected_track(
            &mut track,
            "Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            "source",
        );
        assert_eq!(track.version.as_deref(), Some("Live on KEXP"));
    }

    #[test]
    fn catalog_live_download_gets_source_identity_and_drops_studio_recording_metadata() {
        let mut track = Track {
            id: "musicbrainz:recording-1".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-1".into(),
            source_url: "https://musicbrainz.org/recording/recording-1".into(),
            artist: "Big Thief".into(),
            title: "Mary".into(),
            album: Some("Capacity".into()),
            release_date: Some("2017-06-09".into()),
            isrc: Some("USXX11700001".into()),
            label: Some("Label".into()),
            catalog_number: Some("CAT-1".into()),
            bpm: Some(120.0),
            key: Some("Am".into()),
            artwork_url: Some("https://coverartarchive.org/studio-cover".into()),
            ..Default::default()
        };
        let selected_catalog_before = track.clone();
        let temp = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&temp.path().join("catalog-identity.sqlite")).unwrap();
        store.save_track(&track).unwrap();
        assert!(normalize_catalog_live_download(
            &mut track,
            "Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            "https://www.youtube.com/watch?v=video123456",
            "video123456",
            Some("https://i.ytimg.com/live-source.jpg"),
            Some(240.0),
        )
        .unwrap());
        assert_eq!(track.id, "youtube:video123456");
        assert_eq!(track.artist, "Big Thief");
        assert_eq!(track.title, "Mary");
        assert_eq!(
            track.version.as_deref(),
            Some("Live at Pitchfork Music Festival 2018")
        );
        assert!(track.album.is_none() && track.release_date.is_none() && track.isrc.is_none());
        assert!(
            track.label.is_none()
                && track.catalog_number.is_none()
                && track.bpm.is_none()
                && track.key.is_none()
        );
        assert_eq!(
            track.artwork_url.as_deref(),
            Some("https://i.ytimg.com/live-source.jpg")
        );
        assert_eq!(
            track.provenance["selectedCatalogTrack"]["id"],
            selected_catalog_before.id
        );
        assert_eq!(selected_catalog_before.album.as_deref(), Some("Capacity"));
        assert_eq!(
            selected_catalog_before.release_date.as_deref(),
            Some("2017-06-09")
        );
        let stored_catalog = store.get_track("musicbrainz:recording-1").unwrap().unwrap();
        assert_eq!(stored_catalog.album.as_deref(), Some("Capacity"));
        assert_eq!(stored_catalog.release_date.as_deref(), Some("2017-06-09"));
        assert_eq!(stored_catalog.provider, "musicbrainz");
    }

    #[test]
    fn explicit_catalog_version_mismatch_is_rejected_without_mutation() {
        let mut track = Track {
            id: "musicbrainz:recording-1".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-1".into(),
            artist: "Big Thief".into(),
            title: "Mary".into(),
            version: Some("Live on KEXP".into()),
            album: Some("Capacity".into()),
            ..Default::default()
        };
        let before = track.clone();
        assert!(normalize_catalog_live_download(
            &mut track,
            "Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            "https://www.youtube.com/watch?v=video123456",
            "video123456",
            None,
            Some(240.0),
        )
        .is_err());
        assert_eq!(track.id, before.id);
        assert_eq!(track.version, before.version);
        assert_eq!(track.album, before.album);
    }

    #[test]
    fn explicit_live_source_for_different_song_is_rejected() {
        let mut track = Track {
            id: "musicbrainz:recording-1".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-1".into(),
            artist: "Big Thief".into(),
            title: "Mary".into(),
            ..Default::default()
        };
        assert!(normalize_catalog_live_download(
            &mut track,
            "Big Thief Perform “Masterpiece” | Pitchfork Music Festival 2018",
            "https://www.youtube.com/watch?v=video123456",
            "video123456",
            None,
            Some(240.0),
        )
        .is_err());
        assert_eq!(track.id, "musicbrainz:recording-1");
    }
}
