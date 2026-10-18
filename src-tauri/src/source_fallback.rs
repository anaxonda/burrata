use crate::models::{Candidate, DownloadJob, Settings, Track};

pub fn provider_order(
    primary: &str,
    enabled: bool,
    configured: &[String],
    download: bool,
) -> Vec<String> {
    if !enabled {
        return vec![];
    }
    let allowed: &[&str] = if download {
        &["monochrome", "youtube", "soundcloud", "slskd"]
    } else {
        &["monochrome", "youtube", "soundcloud"]
    };
    let mut result = vec![];
    for id in configured {
        if id != primary && allowed.contains(&id.as_str()) && !result.contains(id) {
            result.push(id.clone());
        }
    }
    result
}
pub fn validate(settings: &Settings) -> Result<(), String> {
    if settings.streaming_fallback_delay_ms > 10_000 {
        return Err("Streaming fallback delay must be between 0 and 10000 ms".into());
    }
    for (sources, download) in [
        (&settings.streaming_fallback_sources, false),
        (&settings.download_fallback_sources, true),
    ] {
        let allowed: &[&str] = if download {
            &["monochrome", "youtube", "soundcloud", "slskd"]
        } else {
            &["monochrome", "youtube", "soundcloud"]
        };
        if sources.len() > allowed.len()
            || sources.iter().any(|s| !allowed.contains(&s.as_str()))
            || sources
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != sources.len()
        {
            return Err("Fallback sources must be supported and listed once".into());
        }
    }
    Ok(())
}
pub fn may_switch(job: &DownloadJob, error: &str, settings: &Settings, cancelled: bool) -> bool {
    settings.download_fallback_enabled
        && !cancelled
        && error != "Cancelled"
        && job.chapter_ids.as_ref().is_none_or(|ids| ids.is_empty())
        && ["downloading", "waiting_for_peer", "stalled"].contains(&job.state.as_str())
        && ![
            "Cannot create staging",
            "outside its staging",
            "Permission denied",
            "No space left",
            "Selected audio format",
        ]
        .iter()
        .any(|s| error.contains(s))
}
fn clean_title(raw: &str) -> String {
    let normalized = crate::matching::normalize(raw);
    for suffix in [
        " official video clip",
        " official lyric video",
        " official audio",
        " official video",
        " official music video",
    ] {
        if let Some(title) = normalized.strip_suffix(suffix) {
            return title.trim().into();
        }
    }
    normalized
}

/// Canonicalize common artist-credit joiners. Catalog services vary between
/// `&`, `and`, and `feat.` when they flatten a multi-artist credit, while the
/// order and the credited names still identify the same act.
fn normalize_artist_credit(raw: &str) -> String {
    let normalized = raw
        .to_lowercase()
        .replace('&', " and ")
        .replace(',', " and ");
    let normalized = crate::matching::normalize(&normalized);
    normalized
        .split_whitespace()
        .map(|word| match word {
            "feat" | "featuring" => "and",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Exact artist/title matching for the deliberately narrow no-catalog-duration
/// YouTube path. Only presentation suffixes are removed from the title; artist
/// joiners are normalized in the artist segment alone.
fn exact_flat_youtube_identity_title(track: &Track, raw_title: &str) -> bool {
    let expected_title = crate::matching::normalize(&track.title);
    if expected_title.is_empty() {
        return false;
    }
    let expected_artist = normalize_artist_credit(&track.artist);
    if expected_artist.is_empty() {
        return false;
    }
    [" - ", " – ", " — ", " | "]
        .iter()
        .filter_map(|separator| raw_title.split_once(separator))
        .any(|(artist, title)| {
            let mut actual_title = crate::matching::normalize(title);
            for suffix in [" official visualizer", " visualizer"] {
                if let Some(cleaned) = actual_title.strip_suffix(suffix) {
                    actual_title = cleaned.trim().to_owned();
                    break;
                }
            }
            normalize_artist_credit(artist) == expected_artist && actual_title == expected_title
        })
}

fn flat_chapterless_track(track: &Track, candidate: &Candidate) -> bool {
    track.playback_range.is_none()
        && !track.provenance["chapters"]
            .as_array()
            .is_some_and(|chapters| !chapters.is_empty())
        && !candidate
            .chapters
            .as_ref()
            .is_some_and(|chapters| !chapters.is_empty())
}

fn original_or_unversioned(track: &Track) -> bool {
    track.version.as_deref().is_none_or(|version| {
        matches!(
            crate::matching::normalize_version(version).as_str(),
            "" | "original" | "original mix"
        )
    })
}

fn explicit_candidate_version_conflict(candidate: &Candidate) -> bool {
    candidate.reasons.iter().any(|reason| {
        let reason = crate::matching::normalize(reason);
        [
            "version mismatch",
            "wrong version",
            "different version",
            "version differs",
        ]
        .iter()
        .any(|marker| reason.contains(marker))
    })
}

fn verified_channel_matches_saved_artist(track: &Track, candidate: &Candidate) -> bool {
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    if metadata["provider"].as_str() != Some("youtube")
        || metadata["channelIsVerified"].as_bool() != Some(true)
    {
        return false;
    }
    let channel_id = metadata["channelId"].as_str().unwrap_or_default();
    if channel_id.len() != 24
        || !channel_id.starts_with("UC")
        || !channel_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return false;
    }
    let channel = metadata["channel"]
        .as_str()
        .or_else(|| metadata["uploader"].as_str())
        .unwrap_or_default()
        .trim();
    if channel.is_empty() || channel.len() > 200 {
        return false;
    }
    let normalized_channel = normalize_artist_credit(channel);
    let full_artist = normalize_artist_credit(&track.artist);
    let first_artist = track
        .artist
        .split([',', '&'])
        .next()
        .map(normalize_artist_credit)
        .unwrap_or_default();
    !normalized_channel.is_empty()
        && (normalized_channel == full_artist || normalized_channel == first_artist)
}

fn verified_missing_duration_youtube_identity(track: &Track, candidate: &Candidate) -> bool {
    candidate.provider == "youtube"
        && candidate.permission != "restricted"
        && candidate.confidence != "low"
        && !explicit_candidate_version_conflict(candidate)
        && !candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
        && original_or_unversioned(track)
        && flat_chapterless_track(track, candidate)
        && exact_flat_youtube_identity_title(track, &candidate.title)
        && candidate
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && duration > 30.0 && duration <= 86_400.0)
        && verified_channel_matches_saved_artist(track, candidate)
}

fn explicit_credit_parts(raw: &str) -> Vec<String> {
    raw.split([',', '&'])
        .map(crate::matching::normalize_single_artist_name)
        .filter(|part| !part.is_empty())
        .collect()
}

/// This provider-tagged payload permits one extractor request only. The marker
/// itself is not identity proof; final acceptance uses fresh extractor fields.
pub(crate) fn is_provisional_youtube_music_catalog_candidate(
    track: &Track,
    candidate: &Candidate,
) -> bool {
    let normalized_title = crate::matching::normalize(&track.title);
    if candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || explicit_candidate_version_conflict(candidate)
        || !original_or_unversioned(track)
        || !flat_chapterless_track(track, candidate)
        || !candidate
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && duration > 30.0 && duration <= 86_400.0)
        || !candidate.id.strip_prefix("youtube:").is_some_and(|id| {
            id.len() == 11
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        || normalized_title.is_empty()
    {
        return false;
    }
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    metadata["provider"].as_str() == Some("youtube")
        && metadata["playbackMusicCatalogCandidate"].as_bool() == Some(true)
        && crate::matching::normalize(metadata["catalogTitle"].as_str().unwrap_or_default())
            == normalized_title
        && crate::matching::normalize(&candidate.title) == normalized_title
        && explicit_credit_parts(metadata["catalogArtist"].as_str().unwrap_or_default())
            == explicit_credit_parts(&track.artist)
        && !explicit_credit_parts(&track.artist).is_empty()
}

/// Fresh yt-dlp structured fields can verify a title-only YouTube Music
/// candidate, but only when all ordered artist credits exactly match the saved
/// explicit comma/ampersand credit. Identical repeated artist entries are
/// deduplicated; distinct extra or missing artists fail closed.
pub(crate) fn playback_music_catalog_recording_matches_verified_extractor(
    track: &Track,
    candidate: &Candidate,
) -> bool {
    let normalized_title = crate::matching::normalize(&track.title);
    if track.duration_seconds.is_some()
        || candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || explicit_candidate_version_conflict(candidate)
        || !original_or_unversioned(track)
        || !flat_chapterless_track(track, candidate)
        || !candidate
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && duration > 30.0 && duration <= 86_400.0)
        || normalized_title.is_empty()
        || crate::matching::normalize(&candidate.title) != normalized_title
        || !verified_channel_matches_saved_artist(track, candidate)
    {
        return false;
    }
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    if metadata["provider"].as_str() != Some("youtube")
        || crate::matching::normalize(metadata["track"].as_str().unwrap_or_default())
            != normalized_title
    {
        return false;
    }
    let Some(artists) = metadata["artists"]
        .as_array()
        .filter(|artists| !artists.is_empty() && artists.len() <= 8)
    else {
        return false;
    };
    let mut observed = Vec::new();
    for artist in artists {
        let Some(artist) = artist
            .as_str()
            .map(str::trim)
            .filter(|artist| !artist.is_empty() && artist.len() <= 120)
        else {
            return false;
        };
        let normalized = crate::matching::normalize_single_artist_name(artist);
        if !normalized.is_empty() && !observed.contains(&normalized) {
            observed.push(normalized);
        }
    }
    let expected = explicit_credit_parts(&track.artist);
    !expected.is_empty() && observed == expected
}

fn monochrome_catalog_artist_credit_matches(track_artist: &str, candidate: &Candidate) -> bool {
    if track_artist.trim().is_empty() || crate::matching::normalize(track_artist).is_empty() {
        return false;
    }
    let Some(raw_names) = candidate
        .source_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("artistNames"))
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    let Some(names) = raw_names
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if names.is_empty() {
        return false;
    }
    if names.len() == 1 {
        // Preserve a single credited artist's own punctuation. Never split one
        // artist name into an apparent multi-artist credit.
        return crate::matching::normalize_single_artist_name(track_artist)
            == crate::matching::normalize_single_artist_name(names[0]);
    }
    let catalog_credit = names
        .iter()
        .map(|name| crate::matching::normalize(name))
        .collect::<Vec<_>>()
        .join(" and ");
    normalize_artist_credit(track_artist) == catalog_credit
}

fn monochrome_catalog_recording_matches(track: &Track, candidate: &Candidate) -> bool {
    if candidate.provider != "monochrome"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
    {
        return false;
    }
    let Some(catalog_title) = candidate
        .source_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("catalogTitle"))
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    let catalog_title = crate::matching::normalize(catalog_title);
    let requested_title = crate::matching::normalize(&track.title);
    if catalog_title.is_empty()
        || requested_title.is_empty()
        || catalog_title != requested_title
        || !requested_version_matches(track, candidate)
        || !monochrome_catalog_artist_credit_matches(&track.artist, candidate)
    {
        return false;
    }
    let Some((expected, actual)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    if !expected.is_finite()
        || !actual.is_finite()
        || expected <= 0.0
        || actual <= 0.0
        || !playback_duration_matches(expected, actual, false)
    {
        return false;
    }
    !track
        .isrc
        .as_deref()
        .zip(
            candidate
                .source_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("isrc"))
                .and_then(serde_json::Value::as_str),
        )
        .is_some_and(|(expected, actual)| {
            !expected.is_empty() && !actual.is_empty() && !expected.eq_ignore_ascii_case(actual)
        })
}

pub fn recording_matches(track: &Track, candidate: &Candidate) -> bool {
    recording_matches_with_duration_rounding(track, candidate, false)
}

fn recording_matches_with_duration_rounding(
    track: &Track,
    candidate: &Candidate,
    allow_integer_rounding: bool,
) -> bool {
    if candidate.permission == "restricted"
        || candidate.confidence == "low"
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
    {
        return false;
    }
    if candidate.confidence == "high" {
        if !requested_version_matches(track, candidate) {
            return false;
        }
        return match (track.duration_seconds, candidate.duration_seconds) {
            (Some(expected), Some(actual)) if expected.is_finite() && actual.is_finite() => {
                playback_duration_matches(expected, actual, allow_integer_rounding)
            }
            _ => true,
        };
    }
    // Spotify playlist captures can omit duration entirely. In that case a normal
    // provider search gets score "review" even for an exact Monochrome catalog
    // recording. Trust only Monochrome's exact catalog title and artist identity;
    // do not infer a two-minute minimum from a missing Spotify runtime. Monochrome
    // serves the catalog recording itself, whereas short preview results from
    // other providers do not carry this catalog metadata and cannot pass here.
    if track
        .duration_seconds
        .is_none_or(|duration| !duration.is_finite() || duration <= 0.0)
        && track
            .version
            .as_deref()
            .is_none_or(|version| version.trim().is_empty())
        && candidate.provider == "monochrome"
    {
        if let (Some(catalog_title), Some(duration)) = (
            candidate
                .source_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("catalogTitle"))
                .and_then(serde_json::Value::as_str),
            candidate.duration_seconds,
        ) {
            let artist = normalize_artist_credit(&track.artist);
            let exact_title = crate::matching::normalize(catalog_title)
                == crate::matching::normalize(&track.title);
            let uploader = candidate.uploader.as_deref().unwrap_or_default();
            let exact_artist = normalize_artist_credit(uploader) == artist;
            if exact_title
                && exact_artist
                && duration.is_finite()
                // SoundCloud-style 30-second preview entries must never qualify
                // as a full recording. A shorter real catalog track can still be
                // accepted from an authoritative Monochrome catalog entry.
                && duration > 30.0
            {
                return true;
            }
        }
    }
    let Some((a, b)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 {
        return false;
    }
    if !playback_duration_matches(a, b, allow_integer_rounding) {
        return false;
    }
    let artist = crate::matching::normalize(&track.artist);
    let title = clean_title(&track.title);
    let actual = clean_title(&candidate.title);
    if artist.is_empty() || title.is_empty() {
        return false;
    }
    let exact = actual == format!("{artist} {title}");
    let version = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    let exact_with_requested_version =
        !version.is_empty() && actual == format!("{artist} {title} {version}");
    let version_ok = version.is_empty()
        || exact_with_requested_version
        || (matches!(version.as_str(), "original" | "original mix") && exact);
    (exact || exact_with_requested_version)
        && version_ok
        && !candidate
            .reasons
            .iter()
            .any(|r| r.contains("version conflicts"))
}

/// Search results keep the strict 5-second rule. Only full extractor metadata
/// with a verified provider ID/title may use up to one extra second when its
/// integer duration is visibly quantized; this accommodates sources reported
/// as 251s whose selected audio is independently measured at about 250s.
fn playback_duration_matches(expected: f64, actual: f64, allow_integer_rounding: bool) -> bool {
    let difference = (expected - actual).abs();
    if difference <= 5.0 {
        return true;
    }
    allow_integer_rounding && actual.fract().abs() < f64::EPSILON && difference <= 6.0
}

fn requested_version_matches(track: &Track, candidate: &Candidate) -> bool {
    let expected = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    if expected.is_empty() || matches!(expected.as_str(), "original" | "original mix") {
        return true;
    }
    let actual = crate::matching::normalize(&candidate.title);
    format!(" {actual} ").contains(&format!(" {expected} "))
}

/// Playback may use a verified full-length title containing an untranslated title
/// before the exact Latin artist/title credit. Keep this looser path out of download
/// matching, where an ambiguous capture should require review.
pub fn playback_recording_matches(track: &Track, candidate: &Candidate) -> bool {
    playback_recording_matches_with_rounding(track, candidate, false)
}

/// This path is callable only after `accept_resolved_identity` has verified a
/// fresh YouTube provider ID and title plus an integer duration from the full
/// extractor response. It keeps the one-second rounding allowance out of
/// search results and persisted candidate metadata.
pub(crate) fn playback_recording_matches_verified_extractor(
    track: &Track,
    candidate: &Candidate,
) -> bool {
    let identity = crate::matching::ephemeral_playback_identity(track);
    let track = identity.as_ref();
    if track.duration_seconds.is_none() {
        return verified_missing_duration_youtube_identity(track, candidate);
    }
    playback_recording_matches_with_rounding(track, candidate, true)
}

fn playback_recording_matches_with_rounding(
    track: &Track,
    candidate: &Candidate,
    allow_integer_rounding: bool,
) -> bool {
    let inferred_track = crate::matching::ephemeral_playback_identity(track);
    let track = inferred_track.as_ref();
    if recording_matches_with_duration_rounding(track, candidate, allow_integer_rounding) {
        return true;
    }
    if monochrome_catalog_recording_matches(track, candidate) {
        return true;
    }
    if topic_original_playback_matches(track, candidate) {
        return true;
    }
    if candidate.permission == "restricted"
        || candidate.confidence == "low"
        || track
            .version
            .as_deref()
            .is_some_and(|version| !version.trim().is_empty())
    {
        return false;
    }

    let artist = crate::matching::normalize(&track.artist);
    let title = crate::matching::normalize(&track.title);
    let expected = format!("{artist} {title}");
    let actual = clean_title(&candidate.title);
    let suffix = format!(" {expected}");
    let Some(prefix) = actual.strip_suffix(suffix.as_str()) else {
        return false;
    };
    let prefix = prefix.trim();
    if prefix.is_empty()
        || !prefix
            .chars()
            .any(|character| character.is_alphabetic() && !character.is_ascii())
        || prefix
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
    {
        return false;
    }

    let Some(candidate_duration) = candidate
        .duration_seconds
        .filter(|duration| duration.is_finite() && *duration > 30.0)
    else {
        return false;
    };
    match track.duration_seconds {
        Some(duration) if duration.is_finite() && duration > 0.0 => {
            playback_duration_matches(duration, candidate_duration, allow_integer_rounding)
        }
        Some(_) => false,
        None => true,
    }
}

/// A narrowly matched flat YouTube result may be extracted once to let the
/// provider's full metadata establish duration and recording identity. This is
/// only a search gate; playback still requires `playback_recording_matches`
/// after the extractor returns the resolved title and duration.
pub fn is_provisional_playback_candidate(track: &Track, candidate: &Candidate) -> bool {
    let inferred_track = crate::matching::ephemeral_playback_identity(track);
    let track = inferred_track.as_ref();
    if !matches!(candidate.provider.as_str(), "youtube" | "soundcloud")
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || !requested_version_matches(track, candidate)
    {
        return false;
    }

    if track.duration_seconds.is_none() {
        let duration_ok = candidate.duration_seconds.is_some_and(|duration| {
            duration.is_finite() && duration > 30.0 && duration <= 86_400.0
        });
        return duration_ok
            && (is_provisional_youtube_music_catalog_candidate(track, candidate)
                || (candidate.provider == "youtube"
                    && original_or_unversioned(track)
                    && !explicit_candidate_version_conflict(candidate)
                    && flat_chapterless_track(track, candidate)
                    && exact_flat_youtube_identity_title(track, &candidate.title)));
    }

    let artist = crate::matching::normalize(&track.artist);
    let title = crate::matching::normalize(&track.title);
    if artist.is_empty() || title.is_empty() {
        return false;
    }
    let expected = format!("{artist} {title}");
    let actual = clean_title(&candidate.title);
    let version = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    let exact_requested_version = !version.is_empty() && actual == format!("{expected} {version}");
    let prefix = if actual == expected || exact_requested_version {
        ""
    } else {
        let suffix = format!(" {expected}");
        let Some(prefix) = actual.strip_suffix(&suffix) else {
            return false;
        };
        prefix.trim()
    };
    if !prefix.is_empty()
        && (!prefix
            .chars()
            .any(|character| character.is_alphabetic() && !character.is_ascii())
            || prefix
                .chars()
                .any(|character| character.is_ascii_alphanumeric()))
    {
        return false;
    }

    let Some((expected, actual)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    expected.is_finite()
        && actual.is_finite()
        && expected > 0.0
        && actual > 30.0
        // Flat YouTube metadata is occasionally rounded more than the final
        // extractor metadata. Allow only a 2s provisional margin; the existing
        // 5s limit still applies after full metadata is resolved.
        && (expected - actual).abs() <= 7.0
}

/// A YouTube Music Topic result may be considered for playback only after
/// its full metadata confirms the catalog identity. This shallow gate is used
/// before audio resolution so a low-confidence flat search result can reach
/// that one full extraction; it does not itself approve playback.
pub fn is_topic_playback_candidate(track: &Track, candidate: &Candidate) -> bool {
    let inferred_track = crate::matching::ephemeral_playback_identity(track);
    let track = inferred_track.as_ref();
    if candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || !candidate
            .uploader
            .as_deref()
            .is_some_and(|name| name.trim_end().ends_with("- Topic"))
        || crate::matching::normalize(&candidate.title) != crate::matching::normalize(&track.title)
        || track.version.as_deref().is_some_and(|version| {
            let version = crate::matching::normalize_version(version);
            !version.is_empty() && !matches!(version.as_str(), "original" | "original mix")
        })
    {
        return false;
    }
    let Some((expected, actual)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    expected.is_finite() && actual.is_finite() && expected > 0.0 && (expected - actual).abs() <= 2.0
}

fn topic_original_playback_matches(track: &Track, candidate: &Candidate) -> bool {
    if !is_topic_playback_candidate(track, candidate) {
        return false;
    }
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    if metadata["provider"].as_str() != Some("youtube") {
        return false;
    }
    let channel = metadata["channel"].as_str().unwrap_or_default();
    if !channel.trim_end().ends_with("- Topic") {
        return false;
    }
    let channel_id = metadata["channelId"].as_str().unwrap_or_default();
    if channel_id.len() != 24
        || !channel_id.starts_with("UC")
        || !channel_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return false;
    }
    let expected_artist = normalize_artist_credit(&track.artist);
    let artist = metadata["artist"].as_str().unwrap_or_default();
    let artists = metadata["artists"]
        .as_array()
        .map(|artists| {
            artists
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(" and ")
        })
        .unwrap_or_default();
    if expected_artist.is_empty()
        || normalize_artist_credit(artist) != expected_artist
        || normalize_artist_credit(&artists) != expected_artist
    {
        return false;
    }
    let title = metadata["track"].as_str().unwrap_or_default();
    let credit = metadata["descriptionCreditLine"]
        .as_str()
        .unwrap_or_default();
    if crate::matching::normalize(title) != crate::matching::normalize(&track.title)
        || !crate::matching::normalize(credit).contains(&crate::matching::normalize(&track.title))
        || !track
            .artist
            .split([',', '&'])
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .all(|name| {
                crate::matching::normalize(credit).contains(&crate::matching::normalize(name))
            })
    {
        return false;
    }
    let Some(duration) = metadata["duration"].as_f64() else {
        return false;
    };
    if !duration.is_finite()
        || track
            .duration_seconds
            .is_none_or(|expected| !expected.is_finite() || (duration - expected).abs() > 2.0)
    {
        return false;
    }
    let version = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    if !version.is_empty() && !matches!(version.as_str(), "original" | "original mix") {
        return false;
    }
    let combined = [candidate.title.as_str(), title, credit]
        .join(" ")
        .to_lowercase();
    ![
        "radio edit",
        "radio version",
        "remix",
        "rework",
        "live",
        "instrumental",
        "acapella",
        "acappella",
        "dub version",
        "cover",
    ]
    .iter()
    .any(|marker| combined.contains(marker))
}
pub fn requires_lossless(job: &DownloadJob) -> bool {
    job.output_profile != "mp3"
        && if let Some(id) = &job.format_id {
            job.candidate
                .formats
                .iter()
                .any(|f| &f.id == id && f.lossless)
        } else {
            job.candidate.formats.iter().any(|f| f.lossless)
        }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_order_is_bounded_and_disabled_means_none() {
        let list = vec![
            "youtube".into(),
            "monochrome".into(),
            "slskd".into(),
            "youtube".into(),
            "soundcloud".into(),
        ];
        assert_eq!(
            provider_order("monochrome", true, &list, true),
            vec!["youtube", "slskd", "soundcloud"]
        );
        assert_eq!(
            provider_order("monochrome", true, &list, false),
            vec!["youtube", "soundcloud"]
        );
        assert!(provider_order("monochrome", false, &list, true).is_empty());
    }
    fn job() -> DownloadJob {
        DownloadJob {
            disposition: Default::default(),
            id: "test".into(),
            track: Track::default(),
            candidate: Candidate::default(),
            state: "downloading".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: String::new(),
            output_profile: "source".into(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
        }
    }
    #[test]
    fn switches_only_source_failures_and_never_cancelled_or_processing_jobs() {
        let mut settings = Settings::default();
        settings.download_fallback_enabled = true;
        let mut job = job();
        assert!(may_switch(&job, "HTTP 404", &settings, false));
        assert!(!may_switch(&job, "HTTP 404", &settings, true));
        assert!(!may_switch(&job, "Cancelled", &settings, false));
        for state in ["validating", "tagging", "publishing", "completed"] {
            job.state = state.into();
            assert!(!may_switch(&job, "failure", &settings, false));
        }
        job.state = "downloading".into();
        job.chapter_ids = Some(vec!["chapter".into()]);
        assert!(!may_switch(&job, "HTTP 404", &settings, false));
        job.chapter_ids = None;
        settings.download_fallback_enabled = false;
        assert!(!may_switch(&job, "HTTP 404", &settings, false));
    }
    #[test]
    fn source_quality_is_preserved_unless_mp3_is_explicitly_requested() {
        let mut job = job();
        job.candidate.formats = vec![crate::models::AudioFormat {
            bit_depth: None,
            channels: None,
            size_bytes: None,
            id: "flac".into(),
            codec: "flac".into(),
            extension: "flac".into(),
            lossless: true,
            bitrate: None,
            sample_rate: Some(44100),
        }];
        assert!(requires_lossless(&job));
        job.output_profile = "mp3".into();
        assert!(!requires_lossless(&job));
    }
    #[tokio::test]
    async fn lossless_guard_checks_decoded_file_codec_not_filename_claims() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::default();
        for (name, codec, expected) in
            [("lossless.flac", "flac", true), ("lossy.m4a", "aac", false)]
        {
            let path = temp.path().join(name);
            let result = std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=44100:cl=stereo",
                    "-t",
                    "0.1",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(result.status.success());
            assert_eq!(
                crate::media::verify_lossless_fallback(&path, &settings)
                    .await
                    .is_ok(),
                expected
            );
        }
    }
    #[test]
    fn fallback_does_not_substitute_live_versions_or_covers() {
        let t = Track {
            artist: "Sui Generis".into(),
            title: "Quizás, Porque".into(),
            duration_seconds: Some(132.973),
            ..Track::default()
        };
        let c = Candidate {
            title: "Sui Generis - Quizás, Porque (Official Audio)".into(),
            duration_seconds: Some(135.0),
            confidence: "review".into(),
            ..Candidate::default()
        };
        assert!(recording_matches(&t, &c));
        assert!(!recording_matches(
            &t,
            &Candidate {
                title: "Quizás, Porque (Official Audio)".into(),
                uploader: Some("Sui Generis".into()),
                ..c.clone()
            }
        ));
        for title in [
            "Sui Generis - Quizás, Porque (Live)",
            "Sui Generis - Quizás, Porque Cover",
        ] {
            assert!(!recording_matches(
                &t,
                &Candidate {
                    title: title.into(),
                    ..c.clone()
                }
            ));
        }
        assert!(!recording_matches(
            &t,
            &Candidate {
                duration_seconds: Some(200.0),
                ..c
            }
        ));
    }

    #[test]
    fn exact_monochrome_catalog_candidate_matches_spotify_capture_without_duration() {
        let track = Track {
            provider: "spotify".into(),
            provider_id: "0Kvqc2zK0UKPTK6pAgyv1v".into(),
            artist: "Stamatis Kraounakis".into(),
            title: "Poso S' Agapo".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            title: "Stamatis Kraounakis - Poso S' Agapo".into(),
            uploader: Some("Stamatis Kraounakis".into()),
            duration_seconds: Some(245.48),
            confidence: "review".into(),
            score: 0.7,
            source_metadata: Some(serde_json::json!({
                "catalogTitle": "Poso S' Agapo",
                "isrc": "QMFMF1300359"
            })),
            ..Candidate::default()
        };
        assert!(recording_matches(&track, &candidate));
        assert!(!recording_matches(
            &track,
            &Candidate {
                source_metadata: Some(serde_json::json!({
                    "catalogTitle": "Poso S' Agapo (Emigre Remix) - Live"
                })),
                ..candidate.clone()
            }
        ));
        assert!(!recording_matches(
            &track,
            &Candidate {
                uploader: Some("Another Artist".into()),
                ..candidate.clone()
            }
        ));
        assert!(!recording_matches(
            &track,
            &Candidate {
                duration_seconds: Some(30.0),
                ..candidate
            }
        ));
    }

    #[test]
    fn exact_catalog_identity_handles_joined_artist_credits_and_short_tracks() {
        let track = Track {
            artist: "Peter, Paul & Mary".into(),
            title: "500 Miles".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            title: "Peter, Paul and Mary - 500 Miles".into(),
            uploader: Some("Peter, Paul and Mary".into()),
            duration_seconds: Some(82.0),
            confidence: "review".into(),
            source_metadata: Some(serde_json::json!({
                "catalogTitle": "500 Miles",
                "isrc": "USWB16500047",
                "provider": "monochrome"
            })),
            ..Candidate::default()
        };
        assert!(recording_matches(&track, &candidate));
        assert!(!recording_matches(
            &track,
            &Candidate {
                uploader: Some("Peter Paul cover project".into()),
                ..candidate.clone()
            }
        ));
        assert!(!recording_matches(
            &track,
            &Candidate {
                duration_seconds: Some(30.0),
                ..candidate
            }
        ));
    }

    #[test]
    fn youtube_topic_original_playback_requires_exact_full_identity() {
        let track = Track {
            artist: "Massivedrum, Bruno Rosado".into(),
            title: "Make Me Sway".into(),
            version: Some("Original Mix (4:23)".into()),
            duration_seconds: Some(263.0),
            ..Track::default()
        };
        let mut candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:fsT7iQxn1m4".into(),
            title: "Make Me Sway".into(),
            uploader: Some("Massivedrum - Topic".into()),
            duration_seconds: Some(263.0),
            confidence: "low".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "channelId":"UCOoxO3pt54tPbTMmAj16c9A",
                "channel":"Massivedrum - Topic",
                "track":"Make Me Sway",
                "artist":"Massivedrum, Bruno Rosado",
                "artists":["Massivedrum","Bruno Rosado"],
                "descriptionCreditLine":"Make Me Sway · Massivedrum · Bruno Rosado",
                "duration":263.0
            })),
            ..Candidate::default()
        };
        assert!(is_topic_playback_candidate(&track, &candidate));
        assert!(playback_recording_matches(&track, &candidate));
        assert!(!recording_matches(&track, &candidate));

        candidate.source_metadata.as_mut().unwrap()["track"] =
            serde_json::json!("Make Me Sway (Radio Edit)");
        assert!(
            !playback_recording_matches(&track, &candidate),
            "a candidate with the wrong requested version must be rejected"
        );
        candidate.source_metadata.as_mut().unwrap()["track"] = serde_json::json!("Make Me Sway");
        candidate.source_metadata.as_mut().unwrap()["artist"] = serde_json::json!("Wrong Artist");
        assert!(!playback_recording_matches(&track, &candidate));
        candidate.source_metadata.as_mut().unwrap()["artist"] =
            serde_json::json!("Massivedrum, Bruno Rosado");
        candidate.title = "Make Me Sway (Radio Edit)".into();
        assert!(!is_topic_playback_candidate(&track, &candidate));
        assert!(!playback_recording_matches(&track, &candidate));
    }

    #[test]
    fn sasa_live_metadata_fixture_never_substitutes_an_unrelated_recording() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/playback/sasa-unavailable.json"
        ))
        .unwrap();
        let recording = &fixture["recording"];
        let track = Track {
            artist: recording["artist"].as_str().unwrap().into(),
            title: recording["title"].as_str().unwrap().into(),
            version: Some(recording["version"].as_str().unwrap().into()),
            duration_seconds: recording["durationSeconds"].as_f64(),
            ..Track::default()
        };
        let mut checked = 0;
        for attempt in fixture["attempts"].as_array().unwrap() {
            for row in attempt["candidates"].as_array().unwrap() {
                let candidate = Candidate {
                    provider: attempt["provider"].as_str().unwrap().into(),
                    title: row["title"].as_str().unwrap().into(),
                    uploader: row["uploader"].as_str().map(str::to_owned),
                    duration_seconds: row["durationSeconds"].as_f64(),
                    ..Candidate::default()
                };
                assert!(
                    !playback_recording_matches(&track, &candidate),
                    "{}",
                    candidate.title
                );
                checked += 1;
            }
        }
        assert!(checked >= 10);
    }

    #[test]
    fn imported_sasa_shape_matches_only_the_exact_main_version_ephemerally() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/playback/sasa-metadata-shapes.json"
        ))
        .unwrap();
        let row = &fixture["shapes"][1];
        let track = Track {
            id: "import:preserved".into(),
            artist: row["artist"].as_str().unwrap().into(),
            title: row["title"].as_str().unwrap().into(),
            version: None,
            duration_seconds: None,
            provenance: serde_json::json!({"sourceKind":"import"}),
            ..Track::default()
        };
        let main = Candidate {
            provider: "youtube".into(),
            title: "Timmy Regisford - Sasa (Main Version)".into(),
            duration_seconds: Some(281.0),
            confidence: "review".into(),
            ..Candidate::default()
        };
        assert!(playback_recording_matches(&track, &main));
        assert!(!recording_matches(&track, &main));
        assert!(!playback_recording_matches(
            &track,
            &Candidate {
                title: "Timmy Regisford - Sasa".into(),
                ..main.clone()
            }
        ));
        for wrong in [
            "Timmy Regisford - Sasa (Radio Edit)",
            "Timmy Regisford - Sasa (Remix Version)",
        ] {
            assert!(!playback_recording_matches(
                &track,
                &Candidate {
                    title: wrong.into(),
                    ..main.clone()
                }
            ));
        }
        assert_eq!(track.id, "import:preserved");
        assert_eq!(track.title, row["title"].as_str().unwrap());
        assert_eq!(track.version, None);
        assert_eq!(track.duration_seconds, None);
    }

    #[test]
    fn playback_requires_the_requested_main_version_and_rejects_radio_edit() {
        let track = Track {
            artist: "Timmy Regisford".into(),
            title: "Sasa".into(),
            version: Some("Main Version (4:41)".into()),
            duration_seconds: Some(281.0),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            title: "Timmy Regisford - Sasa (Main Version)".into(),
            duration_seconds: Some(281.0),
            confidence: "review".into(),
            ..Candidate::default()
        };
        assert!(playback_recording_matches(&track, &candidate));
        assert!(
            playback_recording_matches(&track, &candidate)
                || is_provisional_playback_candidate(&track, &candidate)
                || is_topic_playback_candidate(&track, &candidate),
            "the exact requested version must pass the pre-extraction search gate"
        );
        let unversioned = Candidate {
            title: "Timmy Regisford - Sasa".into(),
            ..candidate.clone()
        };
        assert!(
            !playback_recording_matches(&track, &unversioned),
            "matching artist, title and duration cannot replace a requested version"
        );
        assert!(
            !playback_recording_matches(&track, &unversioned)
                && !is_provisional_playback_candidate(&track, &unversioned)
                && !is_topic_playback_candidate(&track, &unversioned),
            "an unversioned same-title result must not pass any playback search gate"
        );
        let radio_edit = Candidate {
            title: "Timmy Regisford - Sasa (Radio Edit)".into(),
            ..candidate.clone()
        };
        assert!(
            !playback_recording_matches(&track, &radio_edit)
                && !is_provisional_playback_candidate(&track, &radio_edit)
                && !is_topic_playback_candidate(&track, &radio_edit),
            "the radio edit must not pass any playback search gate"
        );
        assert!(!recording_matches(
            &track,
            &Candidate {
                title: "Timmy Regisford - Sasa (Radio Edit)".into(),
                confidence: "high".into(),
                ..Candidate::default()
            }
        ));
    }

    #[test]
    fn playback_accepts_exact_latin_credit_after_foreign_title_prefix_only() {
        let track = Track {
            artist: "Nama".into(),
            title: "Odysseas".into(),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            title: "Νάμα - Οδυσσέας | Nama - Odysseas".into(),
            uploader: Some("NAMA Official channel".into()),
            duration_seconds: Some(191.0),
            confidence: "review".into(),
            reasons: vec![
                "Title token match: 100%".into(),
                "Artist token match: 100%".into(),
            ],
            ..Candidate::default()
        };

        assert!(!recording_matches(&track, &candidate));
        assert!(playback_recording_matches(&track, &candidate));
        assert!(playback_recording_matches(
            &track,
            &Candidate {
                title: "Νάμα - Οδυσσέας | Nama - Odysseas - Official Video Clip".into(),
                ..candidate.clone()
            }
        ));
        assert!(playback_recording_matches(
            &Track {
                duration_seconds: Some(191.0),
                ..track.clone()
            },
            &candidate
        ));
        for title in [
            "Νάμα - Οδυσσέας | Nama - Odysseas Remix",
            "Νάμα - Οδυσσέας | Nama - Odysseas Live",
            "Νάμα - Οδυσσέας | Nama - Odysseas Cover",
            "Νάμα - Οδυσσέας | Nama - Odysseas Extra",
            "Νάμα - Οδυσσέας | Somebody - Odysseas",
            "Νάμα - Οδυσσέας | Nama - Different Song",
        ] {
            assert!(
                !playback_recording_matches(
                    &track,
                    &Candidate {
                        title: title.into(),
                        ..candidate.clone()
                    }
                ),
                "unexpected match for {title}"
            );
        }
        assert!(!playback_recording_matches(
            &track,
            &Candidate {
                duration_seconds: Some(30.0),
                ..candidate.clone()
            }
        ));
        assert!(!playback_recording_matches(
            &Track {
                duration_seconds: Some(95.0),
                ..track
            },
            &candidate
        ));
    }

    #[test]
    fn lyric_video_identity_is_provisional_until_full_duration_is_verified() {
        let track = Track {
            artist: "Stamatis Kraounakis".into(),
            title: "Poso S' Agapo".into(),
            duration_seconds: Some(245.48),
            ..Track::default()
        };
        let flat = Candidate {
            provider: "youtube".into(),
            id: "youtube:hRNx674Z4QE".into(),
            title: "Σταμάτης Κραουνάκης - Πόσο Σ' Αγαπώ | Stamatis Kraounakis - Poso S' Agapo (Official Lyric Video)".into(),
            uploader: Some("MLK".into()),
            duration_seconds: Some(251.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };

        assert!(is_provisional_playback_candidate(&track, &flat));
        assert!(
            !playback_recording_matches(&track, &flat),
            "flat duration remains outside the strict five-second match"
        );

        let extracted = Candidate {
            duration_seconds: Some(250.0),
            ..flat.clone()
        };
        assert!(playback_recording_matches(&track, &extracted));

        let wrong_duration = Candidate {
            duration_seconds: Some(251.0),
            ..flat.clone()
        };
        assert!(is_provisional_playback_candidate(&track, &wrong_duration));
        assert!(
            !playback_recording_matches(&track, &wrong_duration),
            "full metadata that remains more than five seconds off is rejected"
        );

        for rejected in [
            Candidate {
                title: "Σταμάτης Κραουνάκης - Πόσο Σ' Αγαπώ (Live) | Stamatis Kraounakis - Poso S' Agapo (Official Video)".into(),
                ..flat.clone()
            },
            Candidate {
                title: "Σταμάτης Κραουνάκης - Πόσο Σ' Αγαπώ | Stamatis Kraounakis - Poso S' Agapo Remix (Official Lyric Video)".into(),
                ..flat.clone()
            },
            Candidate {
                duration_seconds: Some(253.0),
                ..flat.clone()
            },
        ] {
            assert!(!is_provisional_playback_candidate(&track, &rejected));
        }
    }

    #[test]
    fn missing_duration_requires_exact_flat_youtube_artist_title() {
        let track = Track {
            artist: "KAROL G, Judeline & rusowsky".into(),
            title: "BbY WOW".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let exact = Candidate {
            provider: "youtube".into(),
            id: "youtube:fRIhCiUVaKs".into(),
            title: "KAROL G, Judeline, rusowsky - BbY WOW (Visualizer)".into(),
            duration_seconds: Some(226.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        assert!(is_provisional_playback_candidate(&track, &exact));
        assert!(exact_flat_youtube_identity_title(&track, &exact.title));
        let version_conflict = Track {
            version: Some("Live".into()),
            ..track.clone()
        };
        assert!(!is_provisional_playback_candidate(
            &version_conflict,
            &exact
        ));
        let mut version_candidate = exact.clone();
        version_candidate.source_metadata = Some(serde_json::json!({
            "provider":"youtube",
            "channelId":"UC1234567890123456789012",
            "channel":"KAROL G",
            "channelIsVerified":true
        }));
        assert!(!verified_missing_duration_youtube_identity(
            &version_conflict,
            &version_candidate
        ));

        let bad_times = Track {
            artist: "Imael Angel".into(),
            title: "BAD TIMES".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let descriptive = Candidate {
            title: "Imael Angel - BAD TIMES (Deep Emotional Song)".into(),
            ..exact.clone()
        };
        assert!(!is_provisional_playback_candidate(&bad_times, &descriptive));

        for rejected in [
            Candidate {
                title: "KAROL G, Judeline, rusowsky - BbY WOW (Remix Visualizer)".into(),
                ..exact.clone()
            },
            Candidate {
                title: "KAROL G, Judeline, rusowsky - BbY WOW (Live Visualizer)".into(),
                ..exact.clone()
            },
            Candidate {
                title: "KAROL G, Judeline, rusowsky - BbY WOW (Preview)".into(),
                ..exact.clone()
            },
            Candidate {
                provider: "soundcloud".into(),
                ..exact.clone()
            },
            Candidate {
                duration_seconds: Some(29.0),
                ..exact.clone()
            },
            Candidate {
                chapters: Some(vec![crate::models::Chapter {
                    id: "chapter-1".into(),
                    title: "BbY WOW".into(),
                    start_seconds: 0.0,
                    end_seconds: 20.0,
                    confidence: "source_metadata_unverified".into(),
                    suggested_track: None,
                    suggestion_confidence: None,
                }]),
                ..exact.clone()
            },
        ] {
            assert!(!is_provisional_playback_candidate(&track, &rejected));
        }
    }

    #[test]
    fn no_duration_verified_youtube_path_requires_verified_exact_channel() {
        let track = Track {
            artist: "KAROL G, Judeline & rusowsky".into(),
            title: "BbY WOW".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let mut candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:fRIhCiUVaKs".into(),
            title: "KAROL G, Judeline, rusowsky - BbY WOW (Visualizer)".into(),
            duration_seconds: Some(226.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "channelId":"UC1234567890123456789012",
                "channel":"KAROL G",
                "channelIsVerified":true
            })),
            ..Candidate::default()
        };
        assert!(verified_missing_duration_youtube_identity(
            &track, &candidate
        ));
        candidate
            .reasons
            .push("Recording version conflicts with catalog".into());
        assert!(!verified_missing_duration_youtube_identity(
            &track, &candidate
        ));
        candidate.reasons.clear();

        candidate.source_metadata.as_mut().unwrap()["channel"] =
            serde_json::json!("Other Official");
        assert!(!verified_missing_duration_youtube_identity(
            &track, &candidate
        ));
        candidate.source_metadata.as_mut().unwrap()["channel"] = serde_json::json!("KAROL G");
        candidate.source_metadata.as_mut().unwrap()["channelIsVerified"] = serde_json::json!(false);
        assert!(!verified_missing_duration_youtube_identity(
            &track, &candidate
        ));
        candidate.source_metadata.as_mut().unwrap()["channelIsVerified"] = serde_json::json!(true);
        candidate.source_metadata.as_mut().unwrap()["channelId"] =
            serde_json::json!("not-a-channel-id");
        assert!(!verified_missing_duration_youtube_identity(
            &track, &candidate
        ));
    }

    #[test]
    fn youtube_music_catalog_tag_only_opens_one_exact_extractor_check() {
        let track = Track {
            artist: "Imael Angel".into(),
            title: "BAD TIMES".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:Hqp8vxkPrI8".into(),
            title: "BAD TIMES".into(),
            duration_seconds: Some(168.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "playbackMusicCatalogCandidate":true,
                "catalogTitle":"BAD TIMES",
                "catalogArtist":"Imael Angel"
            })),
            ..Candidate::default()
        };
        assert!(is_provisional_youtube_music_catalog_candidate(
            &track, &candidate
        ));
        assert!(is_provisional_playback_candidate(&track, &candidate));
        let mut version_reason = candidate.clone();
        version_reason
            .reasons
            .push("Version mismatch: long version".into());
        assert!(!is_provisional_youtube_music_catalog_candidate(
            &track,
            &version_reason
        ));
        assert!(!is_provisional_playback_candidate(&track, &version_reason));

        let mut fresh = Candidate {
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "channelId":"UC1234567890123456789012",
                "channel":"Imael Angel",
                "channelIsVerified":true,
                "track":"BAD TIMES",
                "artists":["Imael Angel","Imael Angel"]
            })),
            ..candidate.clone()
        };
        assert!(playback_music_catalog_recording_matches_verified_extractor(
            &track, &fresh
        ));
        fresh.reasons.push("Version mismatch: long version".into());
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &fresh));
        fresh.reasons.clear();
        fresh.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["Imael Angel", "Different Artist"]);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &fresh));
        fresh.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["Imael Angel", "Imael Angel"]);
        fresh.source_metadata.as_mut().unwrap()["channelIsVerified"] = serde_json::json!(false);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &fresh));

        let wrong_version = Track {
            title: "401".into(),
            version: Some("Long Version".into()),
            ..track.clone()
        };
        let long_version_tag = Candidate {
            title: "401".into(),
            duration_seconds: Some(241.0),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "playbackMusicCatalogCandidate":true,
                "catalogTitle":"401",
                "catalogArtist":"Imael Angel"
            })),
            ..candidate
        };
        assert!(!is_provisional_playback_candidate(
            &wrong_version,
            &long_version_tag
        ));
    }

    #[test]
    fn integer_quantized_duration_slack_requires_verified_full_metadata() {
        let track = Track {
            artist: "Stamatis Kraounakis".into(),
            title: "Poso S' Agapo".into(),
            duration_seconds: Some(245.48),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:hRNx674Z4QE".into(),
            title: "Stamatis Kraounakis - Poso S' Agapo".into(),
            duration_seconds: Some(251.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        assert!(!playback_recording_matches(&track, &candidate));

        assert!(playback_recording_matches_verified_extractor(
            &track, &candidate
        ));
        assert!(!playback_recording_matches_verified_extractor(
            &track,
            &Candidate {
                duration_seconds: Some(251.001),
                ..candidate.clone()
            }
        ));
        assert!(!playback_recording_matches(
            &Track {
                duration_seconds: Some(170.0),
                ..track.clone()
            },
            &candidate
        ));
    }

    #[test]
    fn high_confidence_does_not_skip_known_duration_validation() {
        let track = Track {
            artist: "Stamatis Kraounakis".into(),
            title: "Poso S' Agapo".into(),
            duration_seconds: Some(245.48),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:hRNx674Z4QE".into(),
            title: "Stamatis Kraounakis - Poso S' Agapo".into(),
            duration_seconds: Some(170.0),
            confidence: "high".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        assert!(!recording_matches(&track, &candidate));
        assert!(playback_recording_matches_verified_extractor(
            &track,
            &Candidate {
                duration_seconds: Some(251.0),
                ..candidate.clone()
            }
        ));
    }
}

#[cfg(test)]
mod delay_settings_tests {
    use super::*;
    #[test]
    fn fallback_delay_defaults_and_boundaries() {
        let mut settings = Settings::default();
        assert_eq!(settings.streaming_fallback_delay_ms, 1500);
        for delay in [0, 1500, 10_000] {
            settings.streaming_fallback_delay_ms = delay;
            assert!(validate(&settings).is_ok());
        }
        settings.streaming_fallback_delay_ms = 10_001;
        assert!(validate(&settings).is_err());
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("streamingFallbackDelayMs");
        let restored: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(restored.streaming_fallback_delay_ms, 1500);
    }
    #[test]
    fn exact_requested_version_may_resolve_provisional_duration_drift_only() {
        let track = Track {
            artist: "Timmy Regisford".into(),
            title: "Sasa".into(),
            version: Some("Main Version (4:41)".into()),
            duration_seconds: Some(281.0),
            ..Track::default()
        };
        let base = Candidate {
            provider: "youtube".into(),
            title: "Timmy Regisford - Sasa (Main Version)".into(),
            confidence: "review".into(),
            duration_seconds: Some(281.0),
            ..Candidate::default()
        };
        for provider in ["youtube", "soundcloud"] {
            for drift in [-7.0, -6.0, 6.0, 7.0] {
                let flat = Candidate {
                    provider: provider.into(),
                    duration_seconds: Some(281.0 + drift),
                    ..base.clone()
                };
                assert!(is_provisional_playback_candidate(&track, &flat));
                // Search permission is not final recording permission.
                assert!(!playback_recording_matches(&track, &flat));
            }
        }
        for drift in [-5.0, 0.0, 5.0] {
            assert!(playback_recording_matches(
                &track,
                &Candidate {
                    duration_seconds: Some(281.0 + drift),
                    ..base.clone()
                }
            ));
        }
        for title in [
            "Timmy Regisford - Sasa (Radio Edit)",
            "Timmy Regisford - Sasa (Main Version Remix)",
            "Timmy Regisford - Sasa",
            "Other Artist - Sasa (Main Version)",
        ] {
            assert!(!is_provisional_playback_candidate(
                &track,
                &Candidate {
                    title: title.into(),
                    duration_seconds: Some(287.0),
                    ..base.clone()
                }
            ));
        }
        for duration in [None, Some(f64::NAN), Some(0.0), Some(289.0)] {
            assert!(!is_provisional_playback_candidate(
                &track,
                &Candidate {
                    duration_seconds: duration,
                    ..base.clone()
                }
            ));
        }
        assert!(!is_provisional_playback_candidate(
            &Track {
                duration_seconds: None,
                version: Some("Main Version".into()),
                ..track
            },
            &base
        ));
    }

    #[test]
    fn monochrome_full_artist_credit_joiners_preserve_strict_identity_gates() {
        use serde_json::json;

        let track = Track {
            artist: "Artist feat. Guest".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            isrc: Some("USAAA0000001".into()),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "monochrome:155142501534011392".into(),
            provider: "monochrome".into(),
            title: "Artist, Guest - Song".into(),
            uploader: Some("Artist, Guest".into()),
            duration_seconds: Some(200.0),
            permission: "unknown".into(),
            confidence: "review".into(),
            source_metadata: Some(json!({
                "catalogTitle":"Song",
                "artistNames":["Artist", "Guest"],
                "isrc":"USAAA0000001"
            })),
            ..Candidate::default()
        };
        assert!(!recording_matches_with_duration_rounding(
            &track, &candidate, false
        ));
        assert!(playback_recording_matches(&track, &candidate));

        let mut cases = Vec::new();
        cases.push(Candidate {
            source_metadata: Some(json!({"catalogTitle":"Song", "artistNames":["Artist", "Wrong Guest"], "isrc":"USAAA0000001"})),
            ..candidate.clone()
        });
        cases.push(Candidate {
            source_metadata: Some(json!({"catalogTitle":"Song", "artistNames":["Guest", "Artist"], "isrc":"USAAA0000001"})),
            ..candidate.clone()
        });
        cases.push(Candidate {
            source_metadata: Some(json!({"catalogTitle":"Different Song", "artistNames":["Artist", "Guest"], "isrc":"USAAA0000001"})),
            ..candidate.clone()
        });
        cases.push(Candidate {
            duration_seconds: Some(206.0),
            ..candidate.clone()
        });
        cases.push(Candidate {
            permission: "restricted".into(),
            ..candidate.clone()
        });
        cases.push(Candidate {
            confidence: "low".into(),
            ..candidate.clone()
        });
        cases.push(Candidate {
            source_metadata: Some(json!({"catalogTitle":"Song", "artistNames":["Artist", "Guest"], "isrc":"USBBB0000002"})),
            ..candidate.clone()
        });
        cases.push(Candidate {
            source_metadata: Some(
                json!({"catalogTitle":"Song", "artistNames":[], "isrc":"USAAA0000001"}),
            ),
            ..candidate.clone()
        });
        cases.push(Candidate {
            source_metadata: Some(json!({"catalogTitle":"Song", "artistNames":["Artist", null], "isrc":"USAAA0000001"})),
            ..candidate.clone()
        });
        // A single artist name containing a comma remains a single name; it is
        // not split to impersonate the requested two-artist feat. credit.
        cases.push(Candidate {
            source_metadata: Some(json!({"catalogTitle":"Song", "artistNames":["Artist, Guest"], "isrc":"USAAA0000001"})),
            ..candidate.clone()
        });
        let versioned = Track {
            version: Some("Extended Mix".into()),
            ..track.clone()
        };
        assert!(cases.iter().all(|candidate| {
            !monochrome_catalog_recording_matches(&track, candidate)
                && !playback_recording_matches(&track, candidate)
        }));
        assert!(!monochrome_catalog_recording_matches(
            &versioned, &candidate
        ));
        assert!(!playback_recording_matches(&versioned, &candidate));
        let punctuation_only_title = Candidate {
            source_metadata: Some(json!({"catalogTitle":"!!!", "artistNames":["Artist", "Guest"]})),
            ..candidate.clone()
        };
        let empty_title = Track {
            title: "!!!".into(),
            ..track.clone()
        };
        assert!(!monochrome_catalog_recording_matches(
            &empty_title,
            &punctuation_only_title
        ));
        assert!(!playback_recording_matches(
            &empty_title,
            &punctuation_only_title
        ));

        let conflict = Candidate {
            reasons: vec!["Recording version conflicts with catalog".into()],
            ..candidate.clone()
        };
        assert!(!monochrome_catalog_recording_matches(&track, &conflict));
        assert!(!playback_recording_matches(&track, &conflict));

        // Download matching keeps its original exact flattened-title rule.
        assert!(!recording_matches(&track, &candidate));
    }

    #[test]
    fn monochrome_single_artist_ampersand_and_spelling_matches_exact_catalog_identity() {
        let track = Track {
            artist: "Peter, Paul & Mary".into(),
            title: "500 Miles".into(),
            duration_seconds: Some(162.0),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "monochrome:168503514673217536".into(),
            provider: "monochrome".into(),
            title: "Peter, Paul and Mary - 500 Miles".into(),
            uploader: Some("Peter, Paul and Mary".into()),
            duration_seconds: Some(162.933),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "catalogTitle":"500 Miles",
                "artistNames":["Peter, Paul and Mary"],
                "isrc":"USWB16500047"
            })),
            ..Candidate::default()
        };
        assert!(monochrome_catalog_recording_matches(&track, &candidate));
        assert!(playback_recording_matches(&track, &candidate));

        let comma_name_is_not_split_into_a_feat_credit = Candidate {
            provider: "monochrome".into(),
            duration_seconds: Some(200.0),
            confidence: "review".into(),
            source_metadata: Some(serde_json::json!({
                "catalogTitle":"Song",
                "artistNames":["Artist, Guest"]
            })),
            ..Candidate::default()
        };
        let feat_track = Track {
            artist: "Artist feat. Guest".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        assert!(!monochrome_catalog_recording_matches(
            &feat_track,
            &comma_name_is_not_split_into_a_feat_credit
        ));
    }
}
