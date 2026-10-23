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

fn clean_candidate_title_for_track(track: &Track, candidate: &Candidate) -> String {
    clean_title(candidate_caption_without_known_label(track, candidate))
}

fn candidate_caption_without_known_label<'a>(track: &Track, candidate: &'a Candidate) -> &'a str {
    let mut title = candidate.title.as_str();
    if let Some(label) = track.label.as_deref().map(crate::matching::normalize) {
        if !label.is_empty() {
            if let Some((before, trailer)) = title.rsplit_once(" [") {
                if trailer.ends_with(']')
                    && crate::matching::normalize(&trailer[..trailer.len() - 1]) == label
                {
                    title = before;
                }
            }
        }
    }
    title
}

/// Canonicalize common artist-credit joiners. Catalog services vary between
/// `&`, `and`, and `feat.` when they flatten a multi-artist credit, while the
/// order and the credited names still identify the same act.
pub(crate) fn normalize_artist_credit(raw: &str) -> String {
    let normalized = raw
        .to_lowercase()
        .replace('&', " and ")
        .replace(',', " and ");
    let normalized = crate::matching::normalize(&normalized);
    normalized
        .split_whitespace()
        .map(|word| match word {
            "feat" | "featuring" | "ft" => "and",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Feature decorations are artist credits, not recording versions. Only a final
/// bracket containing an explicit feature marker is recognized; all other title
/// text (including remix/live/edit qualifiers) remains part of recording identity.
fn split_feature_credit(raw: &str) -> (&str, Option<&str>) {
    let value = raw.trim();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if !value.ends_with(close) {
            continue;
        }
        let Some(start) = value.rfind(open) else {
            continue;
        };
        let inner = value[start + 1..value.len() - 1].trim();
        let lower = inner.to_ascii_lowercase();
        for marker in ["feat. ", "feat ", "featuring ", "ft. ", "ft "] {
            if lower.starts_with(marker) {
                let artist = inner[marker.len()..].trim();
                if !artist.is_empty() {
                    return (value[..start].trim(), Some(artist));
                }
            }
        }
    }
    (value, None)
}

fn clean_youtube_presentation(raw: &str) -> &str {
    let value = raw.trim();
    let lower = value.to_ascii_lowercase();
    for suffix in [
        " (official audio)",
        " [official audio]",
        " (official lyric video)",
        " [official lyric video]",
        " (official visualizer)",
        " [official visualizer]",
        " (visualizer)",
        " [visualizer]",
    ] {
        if lower.ends_with(suffix) {
            return value[..value.len() - suffix.len()].trim();
        }
    }
    value
}

pub(crate) fn youtube_flat_artist_credit(raw_title: &str) -> Option<String> {
    let title = clean_youtube_presentation(raw_title);
    for separator in [" - ", " – ", " — ", " | "] {
        if let Some((artist, title)) = title.split_once(separator) {
            let (artist, artist_feature) = split_feature_credit(artist);
            let (_, title_feature) = split_feature_credit(title);
            let mut credit = artist.to_owned();
            for feature in [artist_feature, title_feature].into_iter().flatten() {
                credit.push_str(", ");
                credit.push_str(feature);
            }
            return Some(credit);
        }
    }
    None
}

/// Music catalog rows often list only the main acts in artist links and put
/// guest performers in the explicit title feature decoration. Both fields are
/// needed to retain the full credit; this function never supplies absent names.
pub(crate) fn youtube_catalog_artist_credit(raw_title: &str, artists: &str) -> String {
    let (_, feature) = split_feature_credit(clean_youtube_presentation(raw_title));
    let Some(feature) = feature else {
        return artists.to_owned();
    };
    let normalized_feature = normalize_artist_credit(feature);
    let full = format!(" and {} and ", normalize_artist_credit(artists));
    if !normalized_feature.is_empty() && full.contains(&format!(" and {normalized_feature} and ")) {
        artists.to_owned()
    } else {
        format!("{artists}, {feature}")
    }
}

/// Exact base title plus complete performer credits. A feature suffix may only
/// disappear when every named feature is independently present in that full
/// artist credit. No prefix/primary-artist matching or fuzzy aliases are used.
pub(crate) fn youtube_catalog_title_matches(track: &Track, raw: &str, artists: &str) -> bool {
    let (expected, expected_features) = split_feature_credit(&track.title);
    let (actual, actual_features) = split_feature_credit(clean_youtube_presentation(raw));
    if crate::matching::normalize(expected).is_empty()
        || crate::matching::normalize(expected) != crate::matching::normalize(actual)
        || normalize_artist_credit(&youtube_catalog_artist_credit(raw, artists))
            != normalize_artist_credit(&track.artist)
    {
        return false;
    }
    let full = format!(
        " and {} and ",
        normalize_artist_credit(&youtube_catalog_artist_credit(raw, artists))
    );
    [expected_features, actual_features]
        .into_iter()
        .flatten()
        .all(|feature| {
            let feature = normalize_artist_credit(feature);
            !feature.is_empty() && full.contains(&format!(" and {feature} and "))
        })
}

fn exact_flat_youtube_identity_title(track: &Track, raw_title: &str) -> bool {
    let Some(artist) = youtube_flat_artist_credit(raw_title) else {
        return false;
    };
    let raw_title = clean_youtube_presentation(raw_title);
    [" - ", " – ", " — ", " | "]
        .iter()
        .filter_map(|separator| raw_title.split_once(separator))
        .any(|(_, title)| youtube_catalog_title_matches(track, title, &artist))
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

pub(crate) fn playback_music_catalog_has_explicit_version(track: &Track) -> bool {
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let version = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
    !version.is_empty() && !matches!(version.as_str(), "original" | "original mix")
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
    let first_artist = full_artist.split(" and ").next().unwrap_or_default();
    !normalized_channel.is_empty()
        && (normalized_channel == full_artist || normalized_channel == first_artist)
}

fn fresh_topic_credit_line_matches_saved_artist(
    track: &Track,
    candidate: &Candidate,
    expected_title: &str,
) -> bool {
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    let channel = metadata["channel"].as_str().unwrap_or_default().trim();
    let topic_artist = topic_channel_artist(channel);
    let channel_id = metadata["channelId"].as_str().unwrap_or_default();
    let credit = crate::matching::normalize(
        metadata["descriptionCreditLine"]
            .as_str()
            .unwrap_or_default(),
    );
    let contains_phrase = |phrase: &str| {
        let phrase = crate::matching::normalize(phrase);
        !phrase.is_empty() && format!(" {credit} ").contains(&format!(" {phrase} "))
    };
    metadata["provider"].as_str() == Some("youtube")
        && topic_artist.is_some()
        && channel.len() <= 200
        && topic_artist
            .is_some_and(|topic_artist| topic_channel_matches_artist(track, topic_artist))
        && channel_id.len() == 24
        && channel_id.starts_with("UC")
        && channel_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && normalize_artist_credit(metadata["artist"].as_str().unwrap_or_default())
            == normalize_artist_credit(&track.artist)
        && contains_phrase(expected_title)
        && explicit_credit_parts(&track.artist)
            .iter()
            .all(|artist| contains_phrase(artist))
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
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
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
        && youtube_catalog_title_matches(
            track,
            metadata["catalogTitle"].as_str().unwrap_or_default(),
            metadata["catalogArtist"].as_str().unwrap_or_default(),
        )
        && youtube_catalog_title_matches(
            track,
            &candidate.title,
            metadata["catalogArtist"].as_str().unwrap_or_default(),
        )
        && !explicit_credit_parts(&track.artist).is_empty()
}

/// Fresh yt-dlp structured fields can verify a title-only YouTube Music
/// candidate or a known-duration explicit mix. Every ordered artist credit
/// must exactly match the saved explicit comma/ampersand credit. Identical
/// repeated artist entries are deduplicated; distinct extra or missing artists
/// fail closed.
pub(crate) fn playback_music_catalog_recording_matches_verified_extractor(
    track: &Track,
    candidate: &Candidate,
) -> bool {
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let version = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
    let explicit_version =
        !version.is_empty() && !matches!(version.as_str(), "original" | "original mix");
    let normalized_title = crate::matching::normalize(&parsed_title.title);
    let expected_catalog_title = if explicit_version {
        format!("{} {}", parsed_title.title, version)
    } else {
        parsed_title.title.clone()
    };
    if candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || explicit_candidate_version_conflict(candidate)
        || (explicit_version && track.duration_seconds.is_none())
        || (!explicit_version && !original_or_unversioned(track))
        || !flat_chapterless_track(track, candidate)
        || !candidate
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && duration > 30.0 && duration <= 86_400.0)
        || normalized_title.is_empty()
        || !(verified_channel_matches_saved_artist(track, candidate)
            || (explicit_version
                && fresh_topic_credit_line_matches_saved_artist(
                    track,
                    candidate,
                    &expected_catalog_title,
                )))
    {
        return false;
    }
    if let Some(expected) = track.duration_seconds {
        let Some(actual) = candidate.duration_seconds else {
            return false;
        };
        if !expected.is_finite()
            || expected <= 0.0
            || !playback_duration_matches(expected, actual, true)
        {
            return false;
        }
    }
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    if metadata["provider"].as_str() != Some("youtube") {
        return false;
    }
    let title_matches = |actual: &str| {
        if explicit_version {
            let expected =
                crate::matching::normalize(&format!("{} {}", parsed_title.title, version));
            crate::matching::normalize(&clean_youtube_presentation(actual)) == expected
        } else {
            youtube_catalog_title_matches(track, actual, &track.artist)
        }
    };
    if !title_matches(&candidate.title)
        || !title_matches(metadata["track"].as_str().unwrap_or_default())
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
    !observed.is_empty()
        && normalize_artist_credit(&youtube_catalog_artist_credit(
            metadata["track"].as_str().unwrap_or_default(),
            &observed.join(", "),
        )) == normalize_artist_credit(&track.artist)
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
    if monochrome_explicit_guest_without_saved_duration_matches(track, candidate, catalog_title) {
        return true;
    }
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
            !expected.is_empty()
                && !actual.is_empty()
                && !crate::recording_identity::same_isrc(expected, actual)
        })
}

fn monochrome_explicit_guest_without_saved_duration_matches(
    track: &Track,
    candidate: &Candidate,
    catalog_title: &str,
) -> bool {
    if track.duration_seconds.is_some()
        || track.playback_range.is_some()
        || track.provenance["chapters"]
            .as_array()
            .is_some_and(|chapters| !chapters.is_empty())
        || candidate
            .chapters
            .as_ref()
            .is_some_and(|chapters| !chapters.is_empty())
        || track
            .version
            .as_deref()
            .is_some_and(|version| !version.trim().is_empty())
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
        || explicit_candidate_version_conflict(candidate)
    {
        return false;
    }
    let parsed = crate::recording_identity::parse_title(&track.title);
    let Some(guest) = parsed.featured_credit.as_deref() else {
        return false;
    };
    if parsed.version.is_some()
        || crate::matching::normalize(parsed.title.trim()).is_empty()
        || crate::matching::normalize(guest).is_empty()
    {
        return false;
    }
    let expected_catalog_title =
        crate::matching::normalize(&format!("{} with {}", parsed.title, guest));
    if expected_catalog_title.is_empty()
        || crate::matching::normalize(catalog_title) != expected_catalog_title
    {
        return false;
    }

    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    let Some((provider, provider_id)) = candidate.id.split_once(':') else {
        return false;
    };
    if provider != "monochrome"
        || metadata["provider"].as_str() != Some("monochrome")
        || metadata["trackId"].as_str() != Some(provider_id)
    {
        return false;
    }
    let Some(raw_names) = metadata["artistNames"]
        .as_array()
        .filter(|names| !names.is_empty() && names.len() <= 8)
    else {
        return false;
    };
    let Some(observed) = raw_names
        .iter()
        .map(|name| name.as_str().map(str::trim).filter(|name| !name.is_empty()))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let primary = track.artist.trim();
    if primary.is_empty() {
        return false;
    }
    let normalized_guest = crate::matching::normalize_single_artist_name(guest);
    let mut main_credit_options = vec![vec![primary.to_owned()]];
    let split_credits = explicit_credit_parts(primary);
    if split_credits.len() > 1 {
        main_credit_options.push(split_credits);
    }
    let full_credit_matches = main_credit_options.into_iter().any(|mut expected| {
        if !expected
            .iter()
            .any(|artist| crate::matching::normalize_single_artist_name(artist) == normalized_guest)
        {
            expected.push(guest.to_owned());
        }
        observed.len() == expected.len()
            && observed.iter().zip(&expected).all(|(actual, wanted)| {
                crate::matching::normalize_single_artist_name(actual)
                    == crate::matching::normalize_single_artist_name(wanted)
            })
    });
    if !full_credit_matches {
        return false;
    }
    if candidate
        .duration_seconds
        .is_none_or(|duration| !duration.is_finite() || duration <= 30.0 || duration > 86_400.0)
    {
        return false;
    }
    !track
        .isrc
        .as_deref()
        .zip(metadata["isrc"].as_str())
        .is_some_and(|(expected, actual)| {
            !expected.is_empty()
                && !actual.is_empty()
                && !crate::recording_identity::same_isrc(expected, actual)
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
    if candidate
        .title
        .to_ascii_lowercase()
        .contains("official music video")
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
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let title = clean_title(if parsed_title.version.is_some() {
        &parsed_title.title
    } else {
        &track.title
    });
    let actual = clean_candidate_title_for_track(track, candidate);
    if artist.is_empty() || title.is_empty() {
        return false;
    }
    let exact = actual == format!("{artist} {title}");
    let version = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
    let exact_with_requested_version =
        !version.is_empty() && actual == format!("{artist} {title} {version}");
    let version_ok = version.is_empty()
        || exact_with_requested_version
        || (matches!(version.as_str(), "original" | "original mix") && exact);
    exact_flat_remix_credit_caption_matches(track, candidate)
        || exact_default_original_vocal_caption_matches(track, candidate)
        || ((exact || exact_with_requested_version)
            && version_ok
            && !candidate
                .reasons
                .iter()
                .any(|r| r.contains("version conflicts")))
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
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let expected = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
    if expected.is_empty() || matches!(expected.as_str(), "original" | "original mix") {
        return true;
    }
    let actual = crate::matching::normalize(&candidate.title);
    format!(" {actual} ").contains(&format!(" {expected} "))
}

/// Match flat captions that credit a requested remixer in the exact version
/// name rather than in the performer prefix. This is intentionally exact: all
/// other credited performers must remain in order before the title, the
/// requested mix label must match in full, and only the track's exact label may
/// be removed from a trailing bracket.
fn exact_flat_remix_credit_caption_matches(track: &Track, candidate: &Candidate) -> bool {
    if candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || explicit_candidate_version_conflict(candidate)
        || !flat_chapterless_track(track, candidate)
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
    {
        return false;
    }
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let base_title = crate::matching::normalize(&parsed_title.title);
    let version = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
    if parsed_title.featured_credit.is_some()
        || base_title.is_empty()
        || version.is_empty()
        || !(version.ends_with(" remix") || version.ends_with(" mix"))
    {
        return false;
    }

    let actual = clean_candidate_title_for_track(track, candidate);
    // This narrow rule is for comma-separated catalog artist credits. Other
    // punctuation stays on the ordinary exact matching path.
    let credits = track
        .artist
        .split(',')
        .map(crate::matching::normalize)
        .collect::<Vec<_>>();
    if credits.len() < 2 || credits.iter().any(String::is_empty) {
        return false;
    }
    for moved_index in 1..credits.len() {
        let moved_credit = &credits[moved_index];
        if !version.starts_with(&format!("{moved_credit} ")) {
            continue;
        }
        let performers = credits
            .iter()
            .enumerate()
            .filter_map(|(index, credit)| (index != moved_index).then_some(credit.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        if actual == format!("{performers} {base_title} {version}") {
            return true;
        }
    }
    false
}

fn exact_default_original_vocal_caption_matches(track: &Track, candidate: &Candidate) -> bool {
    if candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || explicit_candidate_version_conflict(candidate)
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
        || !original_or_unversioned_or_vocal(track)
        || crate::recording_identity::parse_title(&track.title)
            .version
            .is_some()
        || !flat_chapterless_track(track, candidate)
    {
        return false;
    }

    let caption = candidate_caption_without_known_label(track, candidate);
    let Some(caption) = strip_original_vocal_caption_suffix(caption) else {
        return false;
    };
    let Some((actual_artists, actual_title)) = [" - ", " – ", " — ", " | ", "/"]
        .iter()
        .find_map(|separator| caption.split_once(separator))
    else {
        return false;
    };
    if normalize_artist_credit(&actual_artists) != normalize_artist_credit(&track.artist) {
        return false;
    }
    crate::matching::normalize_title_ignoring_intraword_apostrophes(actual_title)
        == crate::matching::normalize_title_ignoring_intraword_apostrophes(&track.title)
}

fn original_or_unversioned_or_vocal(track: &Track) -> bool {
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let version = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
    matches!(
        version.as_str(),
        "" | "original" | "original mix" | "original vocal"
    )
}

fn strip_original_vocal_caption_suffix(value: &str) -> Option<&str> {
    for suffix in ["(Original Vocal)", "[Original Vocal]"] {
        if value
            .get(value.len().saturating_sub(suffix.len())..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
        {
            return Some(value[..value.len() - suffix.len()].trim_end());
        }
    }
    None
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

/// A complete artist/title prefix with explicit feature-credit normalization
/// and the existing bounded catalog duration is sufficient for known-duration
/// YouTube playback. Uploader display names are not recording artist aliases.
fn known_duration_youtube_complete_credit_matches(
    track: &Track,
    candidate: &Candidate,
    allow_integer_rounding: bool,
) -> bool {
    if candidate.provider != "youtube"
        || candidate.permission == "restricted"
        || candidate.confidence == "low"
        || explicit_candidate_version_conflict(candidate)
        || candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
        || !original_or_unversioned(track)
        || !flat_chapterless_track(track, candidate)
        || !exact_flat_youtube_identity_title(track, &candidate.title)
    {
        return false;
    }
    let Some((expected, actual)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    expected.is_finite()
        && expected > 0.0
        && actual.is_finite()
        && actual > 30.0
        && playback_duration_matches(expected, actual, allow_integer_rounding)
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
    if exact_default_original_vocal_caption_matches(track, candidate)
        && track
            .duration_seconds
            .zip(candidate.duration_seconds)
            .is_some_and(|(expected, actual)| {
                expected.is_finite()
                    && expected > 0.0
                    && actual.is_finite()
                    && actual > 30.0
                    && playback_duration_matches(expected, actual, allow_integer_rounding)
            })
    {
        return true;
    }
    if monochrome_catalog_recording_matches(track, candidate) {
        return true;
    }
    if topic_original_playback_matches(track, candidate) {
        return true;
    }
    if known_duration_youtube_complete_credit_matches(track, candidate, allow_integer_rounding) {
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
    let actual = clean_candidate_title_for_track(track, candidate);
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

    // A single exact YouTube Music catalog locator may be extracted for
    // verification when its integer catalog duration is within the existing
    // six-second quantization allowance. The locator alone never authorizes
    // playback; fresh extractor identity is checked after preparation.
    if is_provisional_youtube_music_catalog_candidate(track, candidate)
        && track
            .duration_seconds
            .zip(candidate.duration_seconds)
            .is_some_and(|(expected, actual)| {
                expected.is_finite()
                    && expected > 0.0
                    && actual.fract().abs() < f64::EPSILON
                    && playback_duration_matches(expected, actual, true)
            })
    {
        return true;
    }

    if exact_flat_remix_credit_caption_matches(track, candidate)
        && track
            .duration_seconds
            .zip(candidate.duration_seconds)
            .is_some_and(|(expected, actual)| {
                expected.is_finite()
                    && expected > 0.0
                    && actual.is_finite()
                    && actual > 30.0
                    && (expected - actual).abs() <= 7.0
            })
    {
        return true;
    }

    if exact_default_original_vocal_caption_matches(track, candidate)
        && track
            .duration_seconds
            .zip(candidate.duration_seconds)
            .is_some_and(|(expected, actual)| {
                expected.is_finite()
                    && expected > 0.0
                    && actual.is_finite()
                    && actual > 30.0
                    && (expected - actual).abs() <= 7.0
            })
    {
        return true;
    }

    if known_duration_youtube_complete_credit_matches(track, candidate, false) {
        return true;
    }
    let artist = crate::matching::normalize(&track.artist);
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let title = crate::matching::normalize(if parsed_title.version.is_some() {
        &parsed_title.title
    } else {
        &track.title
    });
    if artist.is_empty() || title.is_empty() {
        return false;
    }
    let expected = format!("{artist} {title}");
    let actual = clean_candidate_title_for_track(track, candidate);
    let version = crate::matching::normalize_version_for_title(
        &track.title,
        track
            .version
            .as_deref()
            .or(parsed_title.version.as_deref())
            .unwrap_or(""),
    );
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
    if candidate.provider != "youtube" || candidate.permission == "restricted" {
        return false;
    }
    let Some(channel) = candidate.uploader.as_deref().and_then(topic_channel_artist) else {
        return false;
    };
    if !topic_channel_matches_artist(track, channel)
        || !flat_chapterless_track(track, candidate)
        || explicit_candidate_version_conflict(candidate)
    {
        return false;
    }
    let parsed_title = crate::recording_identity::parse_title(&track.title);
    let explicit_version = playback_music_catalog_has_explicit_version(track);
    let expected_title = if explicit_version {
        let version = crate::matching::normalize_version_for_title(
            &track.title,
            track
                .version
                .as_deref()
                .or(parsed_title.version.as_deref())
                .unwrap_or(""),
        );
        format!("{} {}", parsed_title.title, version)
    } else {
        if !original_or_unversioned(track) {
            return false;
        }
        track.title.clone()
    };
    if crate::matching::normalize(&candidate.title) != crate::matching::normalize(&expected_title) {
        return false;
    }
    let Some((expected, actual)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    expected.is_finite()
        && actual.is_finite()
        && expected > 0.0
        && actual > 30.0
        && (expected - actual).abs() <= 2.0
}

fn topic_channel_artist(channel: &str) -> Option<&str> {
    let channel = channel.trim();
    channel
        .strip_suffix("- Topic")
        .or_else(|| channel.strip_suffix(" - Topic"))
        .or_else(|| channel.strip_suffix("-Topic"))
        .map(str::trim)
        .filter(|artist| !artist.is_empty())
}

fn topic_channel_matches_artist(track: &Track, topic_artist: &str) -> bool {
    let topic_artist = crate::matching::normalize_single_artist_name(topic_artist);
    !topic_artist.is_empty()
        && (normalize_artist_credit(topic_artist.as_str())
            == normalize_artist_credit(&track.artist)
            || explicit_credit_parts(&track.artist)
                .iter()
                .any(|artist| artist == &topic_artist))
}

pub(crate) fn is_exact_versioned_topic_candidate(track: &Track, candidate: &Candidate) -> bool {
    playback_music_catalog_has_explicit_version(track)
        && is_topic_playback_candidate(track, candidate)
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
    if topic_channel_artist(channel).is_none() {
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
    fn known_catalog_feature_title_matches_complete_lyric_video_credit_and_bounded_duration() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/youtube-feature-lyric-known-duration.json"
        ))
        .unwrap();
        let track: Track = serde_json::from_value(fixture["track"].clone()).unwrap();
        let make = |title: &str, duration: f64| Candidate {
            provider: "youtube".into(),
            id: "youtube:qND7vxsWE2Y".into(),
            title: title.into(),
            duration_seconds: Some(duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let original = fixture["extractor"]["title"].as_str().unwrap();
        let good = make(original, 219.0);
        assert!(is_provisional_playback_candidate(&track, &good));
        assert!(playback_recording_matches_verified_extractor(&track, &good));
        for wrong in [
            "Kacey Musgraves - Everybody Wants To Be A Cowboy (Official Lyric Video)",
            "Kacey Musgraves, Wrong Guest - Everybody Wants To Be A Cowboy (Official Lyric Video)",
            "Kacey Musgraves, Billy Strings - Everybody Wants To Be A Cowboy (Live)",
            "Kacey Musgraves, Billy Strings - Everybody Wants To Be A Cowboy (Remix)",
            "Kacey Musgraves, Billy Strings - Everybody Wants To Be A Cowboy (Official Music Video)",
            "Kacey Musgraves, Billy Strings - Everybody Wants To Be A Cowboy (Preview)",
        ] {
            assert!(
                !playback_recording_matches_verified_extractor(&track, &make(wrong, 219.0)),
                "{wrong}"
            );
        }
        assert!(!playback_recording_matches_verified_extractor(
            &track,
            &make(original, 180.0)
        ));
        let mut requested_remix = track.clone();
        requested_remix.version = Some("Remix".into());
        assert!(!playback_recording_matches_verified_extractor(
            &requested_remix,
            &good
        ));
        assert_eq!(
            track.title,
            "Everybody Wants To Be A Cowboy (feat. Billy Strings)"
        );
    }

    #[test]
    fn missing_duration_feature_decorations_require_complete_verified_performer_identity() {
        let track = Track {
            title: "Reckless".into(),
            artist: "Nipsey Hussle,Bino Rideaux,Static Major".into(),
            ..Track::default()
        };
        let make = |title: &str| Candidate {
            provider: "youtube".into(),
            id: "youtube:abcdefghijk".into(),
            title: title.into(),
            duration_seconds: Some(180.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(
                serde_json::json!({"provider":"youtube", "channelId":"UC1234567890123456789012", "channel":"Nipsey Hussle", "channelIsVerified":true}),
            ),
            ..Candidate::default()
        };
        let good =
            make("Nipsey Hussle and Bino Rideaux (feat. Static Major) - Reckless (Official Audio)");
        assert!(is_provisional_playback_candidate(&track, &good));
        assert!(playback_recording_matches_verified_extractor(&track, &good));
        for title in [
            "Nipsey Hussle and Bino Rideaux - Reckless (Official Audio)",
            "Nipsey Hussle and Bino Rideaux (feat. Wrong Guest) - Reckless (Official Audio)",
            "Nipsey Hussle and Bino Rideaux (feat. Static Major) - Reckless (Remix)",
            "Nipsey Hussle and Bino Rideaux (feat. Static Major) - Reckless (Live)",
            "Nipsey Hussle and Bino Rideaux (feat. Static Major) - Reckless (Preview)",
        ] {
            assert!(
                !playback_recording_matches_verified_extractor(&track, &make(title)),
                "{title}"
            );
        }
        let mut unverified = good.clone();
        unverified.source_metadata.as_mut().unwrap()["channelIsVerified"] =
            serde_json::json!(false);
        assert!(!playback_recording_matches_verified_extractor(
            &track,
            &unverified
        ));
        let mut preview = good.clone();
        preview.duration_seconds = Some(30.0);
        assert!(!playback_recording_matches_verified_extractor(
            &track, &preview
        ));
        let mut version = track.clone();
        version.version = Some("Remix".into());
        assert!(!playback_recording_matches_verified_extractor(
            &version, &good
        ));
    }

    #[test]
    fn music_catalog_feature_title_requires_complete_fresh_joint_performer_evidence() {
        let track = Track {
            title: "Sacrifices".into(),
            artist: "Nipsey Hussle, Bino Rideaux, James Fauntleroy".into(),
            ..Track::default()
        };
        let mut candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:Ln1vb0eLVUU".into(),
            title: "Sacrifices (feat. James Fauntleroy)".into(),
            duration_seconds: Some(180.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(
                serde_json::json!({"provider":"youtube","channelId":"UC1234567890123456789012","channel":"Nipsey Hussle","channelIsVerified":true,"track":"Sacrifices (feat. James Fauntleroy)","artists":["Nipsey Hussle","Bino Rideaux","James Fauntleroy"]}),
            ),
            ..Candidate::default()
        };
        assert!(playback_music_catalog_recording_matches_verified_extractor(
            &track, &candidate
        ));
        candidate.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["Nipsey Hussle", "Bino Rideaux"]);
        assert!(playback_music_catalog_recording_matches_verified_extractor(
            &track, &candidate
        ));
        candidate.source_metadata.as_mut().unwrap()["track"] = serde_json::json!("Sacrifices");
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &candidate));
        candidate.source_metadata.as_mut().unwrap()["track"] =
            serde_json::json!("Sacrifices (feat. James Fauntleroy)");
        candidate.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["Nipsey Hussle"]);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &candidate));
        candidate.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["Nipsey Hussle", "Bino Rideaux", "Wrong Guest"]);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &candidate));
        candidate.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["Nipsey Hussle", "Bino Rideaux", "James Fauntleroy"]);
        let mut roman = track.clone();
        roman.artist.push_str(" II");
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&roman, &candidate));
    }

    #[test]
    fn known_duration_music_locator_requires_fresh_exact_identity_and_bounded_duration() {
        let track = Track {
            title: "Matthew".into(),
            artist: "John Denver".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        let mut candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:uZGRATckoBM".into(),
            title: "Matthew".into(),
            duration_seconds: Some(228.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "channelId":"UC-cnCe5zradDZlcI8YZboBg",
                "channel":"John Denver",
                "channelIsVerified":true,
                "track":"Matthew",
                "artists":["John Denver"]
            })),
            ..Candidate::default()
        };
        assert!(playback_music_catalog_recording_matches_verified_extractor(
            &track, &candidate
        ));
        assert!(is_provisional_playback_candidate(
            &track,
            &Candidate {
                source_metadata: Some(serde_json::json!({
                    "provider":"youtube",
                    "playbackMusicCatalogCandidate":true,
                    "catalogTitle":"Matthew",
                    "catalogArtist":"John Denver"
                })),
                ..candidate.clone()
            }
        ));

        candidate.duration_seconds = Some(226.75);
        assert!(playback_music_catalog_recording_matches_verified_extractor(
            &track, &candidate
        ));
        candidate.duration_seconds = Some(227.75);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &candidate));
        candidate.duration_seconds = Some(229.0);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &candidate));
        candidate.duration_seconds = Some(228.0);
        candidate.source_metadata.as_mut().unwrap()["artists"] =
            serde_json::json!(["John Denver", "Wrong Artist"]);
        assert!(!playback_music_catalog_recording_matches_verified_extractor(&track, &candidate));
    }

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
            download_layout: Default::default(),
            playlist_context: None,
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
    fn versioned_topic_shallow_gate_requires_exact_channel_title_and_duration() {
        let track = Track {
            artist: "Ralmm, Emil Lassaria".into(),
            title: "Baba (Extended Mix)".into(),
            version: Some("Extended Mix (4:54)".into()),
            duration_seconds: Some(294.0),
            ..Track::default()
        };
        let topic = Candidate {
            provider: "youtube".into(),
            id: "youtube:CZg0jfBo8Gc".into(),
            title: "Baba (Extended Mix)".into(),
            uploader: Some("Ralmm-Topic".into()),
            duration_seconds: Some(294.0),
            confidence: "low".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        assert!(is_topic_playback_candidate(&track, &topic));
        assert!(is_exact_versioned_topic_candidate(&track, &topic));
        assert!(!is_topic_playback_candidate(
            &track,
            &Candidate {
                title: "Baba".into(),
                ..topic.clone()
            }
        ));
        assert!(!is_topic_playback_candidate(
            &track,
            &Candidate {
                uploader: Some("FanUpload-Topic".into()),
                ..topic.clone()
            }
        ));
        assert!(!is_topic_playback_candidate(
            &track,
            &Candidate {
                duration_seconds: Some(301.0),
                ..topic.clone()
            }
        ));
        let no_duration = Track {
            duration_seconds: None,
            ..track
        };
        assert!(!is_topic_playback_candidate(&no_duration, &topic));
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
    fn captured_repeated_title_version_matches_exact_dj_tai_remix_and_guest() {
        let track = Track {
            provider: "traxsource".into(),
            provider_id: "15057930".into(),
            artist: "Mike Dunn, DJ Tai".into(),
            title: "(I'm Sick) I've Been Infected Wit House Musick (Remix)".into(),
            version: Some(
                "I'm Sick I've Been Infected Wit House Musick [DJ Tai Vocal Remix] (8:14)".into(),
            ),
            duration_seconds: Some(494.0),
            ..Track::default()
        };
        let candidate = |title: &str| Candidate {
            provider: "youtube".into(),
            id: "youtube:captured-remix".into(),
            title: title.into(),
            duration_seconds: Some(494.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        assert!(playback_recording_matches(
            &track,
            &candidate(
                "Mike Dunn, DJ Tai - (I'm Sick) I've Been Infected Wit House Musick (DJ Tai Vocal Remix)"
            )
        ));
        assert!(!playback_recording_matches(
            &track,
            &candidate(
                "Mike Dunn, DJ Tai - (I'm Sick) I've Been Infected Wit House Musick (DJ Tai Remix)"
            )
        ));
        assert!(!playback_recording_matches(
            &track,
            &candidate("Mike Dunn - (I'm Sick) I've Been Infected Wit House Musick (Vocal Remix)")
        ));
    }

    #[test]
    fn captured_duration_only_version_does_not_block_exact_title_match() {
        let track = Track {
            provider: "traxsource".into(),
            provider_id: "15019328".into(),
            artist: "Masaki Morii".into(),
            title: "Untitled Dub 1".into(),
            version: Some("(5:47)".into()),
            duration_seconds: Some(347.0),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:untitled-dub-1".into(),
            title: "Masaki Morii - Untitled Dub 1".into(),
            duration_seconds: Some(347.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        assert!(playback_recording_matches(&track, &candidate));
    }

    #[test]
    fn title_only_live_and_remix_labels_constrain_review_and_high_matches() {
        for (label, base_title) in [("Live", "Quiet Song"), ("Remix", "Quiet Song")] {
            let track = Track {
                provider: "traxsource".into(),
                provider_id: "title-only-version".into(),
                artist: "The Artist".into(),
                title: format!("{base_title} ({label})"),
                version: None,
                duration_seconds: Some(240.0),
                ..Track::default()
            };
            for confidence in ["review", "high"] {
                let candidate = |title: &str| Candidate {
                    provider: "youtube".into(),
                    id: "youtube:title-only-version".into(),
                    title: title.into(),
                    duration_seconds: Some(240.0),
                    confidence: confidence.into(),
                    permission: "unknown".into(),
                    ..Candidate::default()
                };
                assert!(
                    playback_recording_matches(
                        &track,
                        &candidate(&format!("The Artist - {base_title} ({label})")),
                    ),
                    "matching {label} candidate should pass at {confidence} confidence"
                );
                assert!(
                    !playback_recording_matches(
                        &track,
                        &candidate(&format!("The Artist - {base_title}")),
                    ),
                    "studio candidate should fail for title-only {label} at {confidence} confidence"
                );
            }
        }
    }

    #[test]
    fn exact_remix_credit_caption_matches_remixer_in_version_and_known_label() {
        let track = Track {
            provider: "traxsource".into(),
            provider_id: "15057930".into(),
            artist: "Mike Dunn, DJ Tai".into(),
            title: "(I'm Sick) I've Been Infected Wit House Musick (Remix)".into(),
            version: Some(
                "I'm Sick I've Been Infected Wit House Musick [DJ Tai Vocal Remix] (8:14)".into(),
            ),
            label: Some("Blackball Muzik".into()),
            duration_seconds: Some(494.0),
            ..Track::default()
        };
        let caption = "Mike Dunn - I'm Sick I've Been Infected Wit House Musick (DJ Tai Vocal Remix) [Blackball Muzik]";
        let make = |title: &str, duration: f64| Candidate {
            provider: "youtube".into(),
            id: "youtube:mVvwOJJRwXo".into(),
            title: title.into(),
            duration_seconds: Some(duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let exact = make(caption, 494.0);
        assert!(is_provisional_playback_candidate(&track, &exact));
        assert!(playback_recording_matches(&track, &exact));
        assert!(playback_recording_matches_verified_extractor(
            &track, &exact
        ));

        for (title, duration) in [
            (
                "Mike Dunn - I'm Sick I've Been Infected Wit House Musick (DJ Tai Instrumental Remix) [Blackball Muzik]",
                492.0,
            ),
            (
                "Mike Dunn - I'm Sick I've Been Infected Wit House Musick (DJ Somebody Vocal Remix) [Blackball Muzik]",
                494.0,
            ),
            (
                "I'm Sick I've Been Infected Wit House Musick (DJ Tai Vocal Remix) [Blackball Muzik]",
                494.0,
            ),
            (
                "Mike Dunn - I'm Sick I've Been Infected Wit House Musick (DJ Tai Vocal Remix) [Other Label]",
                494.0,
            ),
        ] {
            let wrong = make(title, duration);
            assert!(
                !is_provisional_playback_candidate(&track, &wrong),
                "{title}"
            );
            assert!(!playback_recording_matches(&track, &wrong), "{title}");
            assert!(
                !playback_recording_matches_verified_extractor(&track, &wrong),
                "{title}"
            );
        }
        let wrong_runtime = make(caption, 501.0);
        assert!(!playback_recording_matches(&track, &wrong_runtime));
        assert!(!playback_recording_matches_verified_extractor(
            &track,
            &wrong_runtime
        ));
    }

    #[test]
    fn exact_known_label_caption_is_stripped_for_flat_and_extracted_matching() {
        let track = Track {
            provider: "traxsource".into(),
            provider_id: "masaki-untitled-dub-1".into(),
            artist: "Masaki Morii".into(),
            title: "Untitled Dub 1".into(),
            label: Some("M2SOUL Music".into()),
            duration_seconds: Some(347.0),
            ..Track::default()
        };
        let make = |title: &str, duration: f64| Candidate {
            provider: "youtube".into(),
            id: "youtube:Se4-OPD-nEs".into(),
            title: title.into(),
            duration_seconds: Some(duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let exact = make("Masaki Morii - Untitled Dub 1 [M2SOUL Music]", 348.0);
        assert!(is_provisional_playback_candidate(&track, &exact));
        assert!(playback_recording_matches(&track, &exact));
        assert!(playback_recording_matches_verified_extractor(
            &track, &exact
        ));

        for (title, duration) in [
            ("Masaki Morii - Untitled Dub 1 [Wrong Label]", 348.0),
            ("Masaki Morii - Untitled Dub 2 [M2SOUL Music]", 348.0),
            ("Untitled Dub 1 [M2SOUL Music]", 348.0),
            ("Masaki Morii - Untitled Dub 1 [M2SOUL Music]", 360.0),
        ] {
            let wrong = make(title, duration);
            assert!(!playback_recording_matches(&track, &wrong), "{title}");
            assert!(
                !playback_recording_matches_verified_extractor(&track, &wrong),
                "{title}"
            );
        }
    }

    #[test]
    fn exact_default_original_vocal_caption_requires_full_identity_and_runtime() {
        let track = Track {
            provider: "traxsource".into(),
            provider_id: "14798487".into(),
            artist: "Larry La Birt, Corey Holmes".into(),
            title: "It's All About The Love For You".into(),
            version: Some("(5:30)".into()),
            label: Some("Digital Generation".into()),
            duration_seconds: Some(330.0),
            ..Track::default()
        };
        let make = |title: &str, duration: f64| Candidate {
            provider: "youtube".into(),
            id: "youtube:iHugvFBKbF0".into(),
            title: title.into(),
            duration_seconds: Some(duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let exact = make(
            "Larry La Birt, Corey Holmes - Its All About The Love For You (Original Vocal) [Digital Generation]",
            330.0,
        );
        assert!(is_provisional_playback_candidate(&track, &exact));
        assert!(playback_recording_matches(&track, &exact));
        assert!(playback_recording_matches_verified_extractor(
            &track, &exact
        ));
        assert!(playback_recording_matches(
            &track,
            &make(
                "Larry La Birt,Corey Holmes/Its All About The Love For You(Original Vocal)",
                331.0,
            )
        ));
        assert!(playback_recording_matches(
            &track,
            &make(
                "Larry La Birt, Corey Holmes - It’s All About The Love For You [Original Vocal]",
                330.0,
            )
        ));
        assert!(playback_recording_matches(
            &track,
            &make(
                "Larry La Birt, Corey Holmes - I'ts All About The Love For You (Original Vocal)",
                330.0,
            )
        ));

        for (title, duration) in [
            (
                "Larry La Birt, Corey Holmes - Its All About The Love For You (Remix) [Digital Generation]",
                330.0,
            ),
            (
                "Larry La Birt, Corey Holmes - Its All About The Love For You (Live) [Digital Generation]",
                330.0,
            ),
            (
                "Larry La Birt, Corey Holmes - Its All About The Love For You (Instrumental) [Digital Generation]",
                330.0,
            ),
            (
                "Larry La Birt, Corey Holmes - Its All About The Love For You (Original Vocal Mix) [Digital Generation]",
                330.0,
            ),
            (
                "Larry La Birt, Wrong Guest - Its All About The Love For You (Original Vocal) [Digital Generation]",
                330.0,
            ),
            (
                "Larry La Birt - Its All About The Love For You (Original Vocal) [Digital Generation]",
                330.0,
            ),
            (
                "Larry La Birt, Corey Holmes - Its All About The Love For You (Original Vocal) [Wrong Label]",
                330.0,
            ),
            (
                "Larry La Birt, Corey Holmes - Its All About The Love For You (Original Vocal) [Digital Generation]",
                342.0,
            ),
        ] {
            let wrong = make(title, duration);
            assert!(
                !is_provisional_playback_candidate(&track, &wrong),
                "{title}"
            );
            assert!(!playback_recording_matches(&track, &wrong), "{title}");
            assert!(
                !playback_recording_matches_verified_extractor(&track, &wrong),
                "{title}"
            );
        }

        let explicitly_instrumental = Track {
            version: Some("Instrumental".into()),
            ..track
        };
        assert!(!is_provisional_playback_candidate(
            &explicitly_instrumental,
            &exact
        ));
        assert!(!playback_recording_matches_verified_extractor(
            &explicitly_instrumental,
            &exact
        ));
        let explicit_original_vocal = Track {
            version: Some("Original Vocal".into()),
            ..explicitly_instrumental.clone()
        };
        assert!(playback_recording_matches_verified_extractor(
            &explicit_original_vocal,
            &exact
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

    #[test]
    fn monochrome_explicit_guest_matches_only_exact_full_credit_missing_duration_recording() {
        let track = Track {
            provider: "radio_song".into(),
            title: "Diaraby  (w/ Ry Cooder)".into(),
            artist: "Ali Farka Touré".into(),
            album: Some("Ali Farka Touré".into()),
            ..Track::default()
        };
        let candidate = || Candidate {
            id: "monochrome:177409920331984896".into(),
            provider: "monochrome".into(),
            title: "Ali Farka Touré, Ry Cooder - Diaraby - with Ry Cooder".into(),
            uploader: Some("Ali Farka Touré, Ry Cooder".into()),
            duration_seconds: Some(446.826),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"monochrome",
                "trackId":"177409920331984896",
                "catalogTitle":"Diaraby - with Ry Cooder",
                "artistNames":["Ali Farka Touré", "Ry Cooder"],
                "isrc":"GBBGU0640010"
            })),
            ..Candidate::default()
        };
        let exact = candidate();
        assert!(monochrome_catalog_recording_matches(&track, &exact));
        assert!(playback_recording_matches(&track, &exact));
        assert!(playback_recording_matches(
            &Track {
                title: "Diaraby [w/ Ry Cooder]".into(),
                ..track.clone()
            },
            &exact
        ));
        for title in [
            "Diaraby (feat. Ry Cooder)",
            "Diaraby [feat. Ry Cooder]",
            "Diaraby (featuring Ry Cooder)",
            "Diaraby [featuring Ry Cooder]",
        ] {
            assert!(
                playback_recording_matches(
                    &Track {
                        title: title.into(),
                        ..track.clone()
                    },
                    &exact
                ),
                "expected explicit guest form to match: {title}"
            );
        }

        let mut wrong_guest = candidate();
        wrong_guest.source_metadata.as_mut().unwrap()["artistNames"] =
            serde_json::json!(["Ali Farka Touré", "Other Guest"]);
        wrong_guest.title = "Ali Farka Touré, Other Guest - Diaraby - with Ry Cooder".into();
        wrong_guest.uploader = Some("Ali Farka Touré, Other Guest".into());
        assert!(!playback_recording_matches(&track, &wrong_guest));

        let mut missing_guest = candidate();
        missing_guest.source_metadata.as_mut().unwrap()["artistNames"] =
            serde_json::json!(["Ali Farka Touré"]);
        missing_guest.title = "Ali Farka Touré - Diaraby - with Ry Cooder".into();
        assert!(!playback_recording_matches(&track, &missing_guest));

        let mut extra_artist = candidate();
        extra_artist.source_metadata.as_mut().unwrap()["artistNames"] =
            serde_json::json!(["Ali Farka Touré", "Ry Cooder", "Extra Artist"]);
        extra_artist.title =
            "Ali Farka Touré, Ry Cooder, Extra Artist - Diaraby - with Ry Cooder".into();
        assert!(!playback_recording_matches(&track, &extra_artist));

        let mut wrong_order = candidate();
        wrong_order.source_metadata.as_mut().unwrap()["artistNames"] =
            serde_json::json!(["Ry Cooder", "Ali Farka Touré"]);
        wrong_order.title = "Ry Cooder, Ali Farka Touré - Diaraby - with Ry Cooder".into();
        assert!(!playback_recording_matches(&track, &wrong_order));

        let mut wrong_title = candidate();
        wrong_title.source_metadata.as_mut().unwrap()["catalogTitle"] =
            serde_json::json!("Different Song - with Ry Cooder");
        assert!(!playback_recording_matches(&track, &wrong_title));

        let mut live = candidate();
        live.source_metadata.as_mut().unwrap()["catalogTitle"] =
            serde_json::json!("Diaraby - with Ry Cooder Live");
        live.title = "Ali Farka Touré, Ry Cooder - Diaraby - with Ry Cooder Live".into();
        assert!(!playback_recording_matches(&track, &live));
        assert!(!playback_recording_matches(
            &Track {
                title: "Diaraby (w/ Ry Cooder) (Live)".into(),
                ..track.clone()
            },
            &exact
        ));
        assert!(!playback_recording_matches(
            &Track {
                version: Some("Live".into()),
                ..track.clone()
            },
            &exact
        ));
        assert!(!playback_recording_matches(
            &Track {
                playback_range: Some(crate::models::PlaybackRange {
                    parent_track_id: "parent".into(),
                    chapter_id: "chapter".into(),
                    source_url: "https://example.test/audio".into(),
                    start_time: 0.0,
                    end_time: 30.0,
                    parent_title: "Diaraby".into(),
                    parent_duration_seconds: Some(446.826),
                }),
                ..track.clone()
            },
            &exact
        ));
        assert!(!playback_recording_matches(
            &Track {
                provenance: serde_json::json!({"chapters":[{"startTime":0}]}),
                ..track.clone()
            },
            &exact
        ));

        let mut isrc_conflict = Track {
            isrc: Some("USAAA0000001".into()),
            ..track.clone()
        };
        assert!(!playback_recording_matches(&isrc_conflict, &exact));
        isrc_conflict.isrc = None;
        for duration in [Some(30.0), Some(0.0), Some(f64::NAN), Some(86_401.0), None] {
            let bad_runtime = Candidate {
                duration_seconds: duration,
                ..exact.clone()
            };
            assert!(!playback_recording_matches(&track, &bad_runtime));
        }

        let bad_provider_track = Candidate {
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "trackId":"177409920331984896",
                "catalogTitle":"Diaraby - with Ry Cooder",
                "artistNames":["Ali Farka Touré", "Ry Cooder"],
                "isrc":"GBBGU0640010"
            })),
            ..exact
        };
        assert!(!playback_recording_matches(&track, &bad_provider_track));
    }
}
