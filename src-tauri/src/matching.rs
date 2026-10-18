//! Conservative recording matching. A known version mismatch is never a recommendation.
use crate::models::{Candidate, Track};
use std::{borrow::Cow, collections::HashSet};

pub fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Split one balanced parenthetical group at the end of a display string.
/// Returns the trimmed prefix and the full parenthetical contents without delimiters.
pub fn split_balanced_trailing_parenthetical(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_end();
    if !value.ends_with(')') {
        return None;
    }
    let mut depth = 0usize;
    let mut opening = None;
    for (index, character) in value.char_indices().rev() {
        match character {
            ')' => depth = depth.checked_add(1)?,
            '(' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    opening = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let opening = opening?;
    let prefix = value[..opening].trim_end();
    let mut prefix_depth = 0usize;
    for character in prefix.chars() {
        match character {
            '(' => prefix_depth = prefix_depth.checked_add(1)?,
            ')' => prefix_depth = prefix_depth.checked_sub(1)?,
            _ => {}
        }
    }
    if prefix_depth != 0 {
        return None;
    }
    let contents = value[opening + 1..value.len() - 1].trim();
    (!prefix.is_empty() && !contents.is_empty()).then_some((prefix, contents))
}

fn parse_display_runtime(value: &str) -> Option<f64> {
    let (minutes, seconds) = value.split_once(':')?;
    if minutes.is_empty()
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
        || seconds.len() != 2
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 {
        return None;
    }
    let total = minutes.checked_mul(60)?.checked_add(seconds)?;
    (total > 0 && total <= 86_400).then_some(total as f64)
}

fn strict_display_version(value: &str) -> bool {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "main version" | "original version" | "album version"
    )
}

/// Parse only an explicit, whitelisted version label with a balanced trailing m:ss
/// display runtime, such as `Main Version (4:41)`.
pub fn parse_strict_version_runtime(value: &str) -> Option<(String, f64)> {
    let (label, runtime) = split_balanced_trailing_parenthetical(value)?;
    if !strict_display_version(label) {
        return None;
    }
    Some((label.to_owned(), parse_display_runtime(runtime)?))
}

/// Build a transient matching view for legacy/imported titles that contain a known
/// display version and runtime. This clone is never persisted and respects manual tags.
pub fn ephemeral_playback_identity(track: &Track) -> Cow<'_, Track> {
    let manual = &track.provenance["manualFields"];
    if track.playback_range.is_some()
        || track.duration_seconds.is_some()
        || manual["title"] == true
        || manual["version"] == true
        || manual["durationSeconds"] == true
        || track.provenance["chapters"]
            .as_array()
            .is_some_and(|values| !values.is_empty())
    {
        return Cow::Borrowed(track);
    }

    if let Some(version) = track.version.as_deref() {
        let Some((_, seconds)) = parse_strict_version_runtime(version) else {
            return Cow::Borrowed(track);
        };
        let mut identity = track.clone();
        identity.duration_seconds = Some(seconds);
        return Cow::Owned(identity);
    }
    let Some((title, suffix)) = split_balanced_trailing_parenthetical(&track.title) else {
        return Cow::Borrowed(track);
    };
    let Some((_, seconds)) = parse_strict_version_runtime(suffix) else {
        return Cow::Borrowed(track);
    };
    let mut identity = track.clone();
    identity.title = title.to_owned();
    identity.version = Some(suffix.to_owned());
    identity.duration_seconds = Some(seconds);
    // Do not add inferred fields to persisted provenance.
    Cow::Owned(identity)
}

/// Catalog imports sometimes store a displayed run time at the end of the version label.
/// Keep that source value intact, but exclude a trailing `(m:ss)` from matching and search.
pub fn normalize_version(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(open) = trimmed.rfind('(') {
        if trimmed.ends_with(')') {
            let duration = &trimmed[open + 1..trimmed.len() - 1];
            let mut parts = duration.split(':');
            let minutes = parts.next().unwrap_or_default();
            let seconds = parts.next().unwrap_or_default();
            if parts.next().is_none()
                && !minutes.is_empty()
                && minutes.chars().all(|c| c.is_ascii_digit())
                && seconds.len() == 2
                && seconds.chars().all(|c| c.is_ascii_digit())
                && seconds.parse::<u8>().is_ok_and(|n| n < 60)
            {
                return normalize(trimmed[..open].trim());
            }
        }
    }
    normalize(trimmed)
}

fn coverage(needle: &str, haystack: &str) -> f64 {
    let words: HashSet<_> = needle.split_whitespace().collect();
    let hay: HashSet<_> = haystack.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    words.intersection(&hay).count() as f64 / words.len() as f64
}

fn version_kinds(value: &str) -> HashSet<&str> {
    value
        .split_whitespace()
        .filter(|word| {
            matches!(
                *word,
                "radio"
                    | "extended"
                    | "remix"
                    | "dub"
                    | "acoustic"
                    | "live"
                    | "original"
                    | "instrumental"
                    | "edit"
                    | "vocal"
                    | "club"
                    | "bootleg"
                    | "remaster"
                    | "remastered"
            )
        })
        .collect()
}

fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    !phrase.is_empty() && format!(" {haystack} ").contains(&format!(" {phrase} "))
}

/// Scores describe evidence, not a probability that an upload is correct or authorized.
pub fn score_candidate(track: &Track, candidate: &mut Candidate) {
    let title = normalize(&track.title);
    let artist = normalize(&track.artist);
    let upload = normalize(&candidate.title);
    let uploader = normalize(candidate.uploader.as_deref().unwrap_or(""));
    let expected = normalize_version(track.version.as_deref().unwrap_or(""));
    let title_match = coverage(&title, &upload);
    let artist_match = coverage(&artist, &upload);
    let mut reasons = vec![
        format!("Title token match: {:.0}%", title_match * 100.0),
        format!("Artist token match: {:.0}%", artist_match * 100.0),
    ];
    if !artist.is_empty()
        && contains_phrase(&uploader, &artist)
        && !contains_phrase(&upload, &artist)
    {
        reasons.push(
            "Uploader name resembles the artist but is not counted as artist identity".into(),
        );
    }
    let mut mismatch = false;
    let mut version_verified = false;
    // Artist/title words may include "Radio" or "Live" without describing the version.
    let version_text = format!(" {upload} ")
        .replace(&format!(" {title} "), " ")
        .replace(&format!(" {artist} "), " ");
    let expected_kinds = version_kinds(&expected);
    let upload_kinds = version_kinds(&version_text);
    let version_score = if expected.is_empty() {
        reasons.push("Catalog recording version is unknown; review required".into());
        0.0
    } else if !upload_kinds.is_subset(&expected_kinds) {
        // Check all explicit labels before substring matching: "Extended Mix Radio Edit"
        // must not pass merely because it contains "Extended Mix".
        mismatch = true;
        reasons.push("Recording version conflicts with catalog".into());
        0.0
    } else if contains_phrase(&upload, &expected) {
        version_verified = true;
        reasons.push("Catalog version is present in upload title".into());
        1.0
    } else if expected_kinds.contains("original") && upload_kinds.is_empty() {
        reasons.push("Original version not explicitly identified by upload".into());
        0.5
    } else {
        reasons.push("Exact remix/version is not verified".into());
        0.0
    };
    let mut duration_verified = false;
    let duration_score = match (track.duration_seconds, candidate.duration_seconds) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 => {
            let difference = (a - b).abs();
            reasons.push(format!("Duration difference: {difference:.1}s"));
            let tolerance = 10.0_f64.max(a * 0.03);
            duration_verified = difference <= tolerance;
            if difference > 30.0_f64.max(a * 0.12) {
                mismatch = true;
                0.0
            } else if difference <= tolerance {
                1.0
            } else {
                0.4
            }
        }
        _ => {
            reasons.push("Full recording duration is unavailable; review required".into());
            0.0
        }
    };
    candidate.score = (title_match * 0.4
        + artist_match * 0.3
        + version_score * 0.2
        + duration_score * 0.1
        + if candidate.official { 0.02 } else { 0.0 })
    .min(1.0);
    candidate.confidence = if mismatch || title_match < 0.6 || artist_match < 0.5 {
        "low"
    } else if candidate.score >= 0.9
        && title_match == 1.0
        && artist_match == 1.0
        && contains_phrase(&upload, &title)
        && contains_phrase(&upload, &artist)
        && version_verified
        && duration_verified
    {
        "high"
    } else {
        "review"
    }
    .into();
    if mismatch {
        candidate.score = candidate.score.min(0.49);
    }
    candidate.reasons = reasons;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        }
    }
    fn candidate(title: &str, duration: f64) -> Candidate {
        Candidate {
            title: title.into(),
            duration_seconds: Some(duration),
            ..Default::default()
        }
    }

    #[test]
    fn uploader_name_alone_does_not_verify_the_artist() {
        let mut candidate = Candidate {
            title: "Night Drive Extended Mix".into(),
            uploader: Some("Artist One".into()),
            duration_seconds: Some(360.0),
            ..Candidate::default()
        };
        score_candidate(&track(), &mut candidate);
        assert_eq!(candidate.confidence, "low");
        assert!(candidate
            .reasons
            .iter()
            .any(|reason| reason.contains("not counted as artist identity")));
    }
    #[test]
    fn exact_version_and_duration_are_high() {
        let mut c = candidate("Artist One - Night Drive (Extended Mix)", 361.0);
        score_candidate(&track(), &mut c);
        assert_eq!(c.confidence, "high");
    }
    #[test]
    fn radio_cannot_replace_extended() {
        let mut c = candidate("Artist One - Night Drive (Radio Edit)", 360.0);
        score_candidate(&track(), &mut c);
        assert_eq!(c.confidence, "low");
    }
    #[test]
    fn remix_identity_is_not_just_remix_word() {
        let t = Track {
            version: Some("Alice Remix".into()),
            ..track()
        };
        let mut c = candidate("Artist One - Night Drive (Bob Remix)", 360.0);
        score_candidate(&t, &mut c);
        assert_ne!(c.confidence, "high");
    }
    #[test]
    fn wrong_artist_is_low() {
        let mut c = candidate("Other Person - Night Drive (Extended Mix)", 360.0);
        score_candidate(&track(), &mut c);
        assert_eq!(c.confidence, "low");
    }
    #[test]
    fn duration_mismatch_is_low() {
        let mut c = candidate("Artist One - Night Drive (Extended Mix)", 190.0);
        score_candidate(&track(), &mut c);
        assert_eq!(c.confidence, "low");
    }
    #[test]
    fn missing_duration_needs_review() {
        let mut c = candidate("Artist One - Night Drive (Extended Mix)", 360.0);
        c.duration_seconds = None;
        score_candidate(&track(), &mut c);
        assert_eq!(c.confidence, "review");
    }
    #[test]
    fn missing_catalog_version_needs_review() {
        let t = Track {
            version: None,
            ..track()
        };
        let mut c = candidate("Artist One - Night Drive", 360.0);
        score_candidate(&t, &mut c);
        assert_eq!(c.confidence, "review");
    }
    #[test]
    fn conflicting_labels_override_expected_version_substring() {
        for label in ["Radio Edit", "Instrumental", "Live", "Remix", "Edit"] {
            let mut c = candidate(
                &format!("Artist One - Night Drive (Extended Mix {label})"),
                360.0,
            );
            score_candidate(&track(), &mut c);
            assert_eq!(c.confidence, "low", "{label}");
        }
    }
    #[test]
    fn remix_artist_requires_word_boundaries() {
        let t = Track {
            version: Some("Alice Remix".into()),
            ..track()
        };
        let mut c = candidate("Artist One - Night Drive (Malice Remix)", 360.0);
        score_candidate(&t, &mut c);
        assert_eq!(c.confidence, "review");
    }
    #[test]
    fn song_title_is_not_a_version_label() {
        let t = Track {
            title: "Live Radio".into(),
            ..track()
        };
        let mut c = candidate("Artist One - Live Radio (Extended Mix)", 360.0);
        score_candidate(&t, &mut c);
        assert_eq!(c.confidence, "high");
    }

    #[test]
    fn nested_main_version_runtime_is_parsed_only_for_a_strict_balanced_suffix() {
        assert_eq!(
            split_balanced_trailing_parenthetical("Sasa (Main Version (4:41))"),
            Some(("Sasa", "Main Version (4:41)"))
        );
        assert_eq!(
            parse_strict_version_runtime("Main Version (4:41)"),
            Some(("Main Version".into(), 281.0))
        );
        for rejected in [
            "Radio Edit (4:41)",
            "Radio Version (4:41)",
            "Remix Version (4:41)",
            "Main Mix (4:41)",
            "Main Version (4:60)",
            "Main Version (4:41",
            "Sasa (Unclosed (Main Version (4:41))",
        ] {
            assert!(
                parse_strict_version_runtime(rejected).is_none(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn ephemeral_playback_identity_preserves_original_id_and_manual_or_chapter_fields() {
        let track = Track {
            id: "import:stable".into(),
            title: "Sasa (Main Version (4:41))".into(),
            version: None,
            duration_seconds: None,
            provenance: serde_json::json!({"sourceRank":7}),
            ..Track::default()
        };
        let identity = ephemeral_playback_identity(&track);
        assert!(matches!(&identity, Cow::Owned(_)));
        assert_eq!(identity.id, track.id);
        assert_eq!(identity.title, "Sasa");
        assert_eq!(identity.version.as_deref(), Some("Main Version (4:41)"));
        assert_eq!(identity.duration_seconds, Some(281.0));
        assert_eq!(identity.provenance, track.provenance);
        assert_eq!(track.duration_seconds, None);
        let manually_titled = Track {
            provenance: serde_json::json!({"manualFields":{"title":true}}),
            ..track.clone()
        };
        assert!(matches!(
            ephemeral_playback_identity(&manually_titled),
            Cow::Borrowed(_)
        ));
        let chapter = Track {
            provenance: serde_json::json!({"chapters":[{"id":"chapter"}]}),
            ..track.clone()
        };
        assert!(matches!(
            ephemeral_playback_identity(&chapter),
            Cow::Borrowed(_)
        ));
        let has_duration = Track {
            duration_seconds: Some(280.0),
            ..track.clone()
        };
        assert!(matches!(
            ephemeral_playback_identity(&has_duration),
            Cow::Borrowed(_)
        ));
        for field in ["title", "version", "durationSeconds"] {
            let manual = Track {
                provenance: serde_json::json!({"manualFields":{field:true}}),
                ..track.clone()
            };
            assert!(matches!(
                ephemeral_playback_identity(&manual),
                Cow::Borrowed(_)
            ));
        }
        assert_eq!(
            parse_strict_version_runtime("Main Version (1440:00)"),
            Some(("Main Version".into(), 86_400.0))
        );
        assert!(parse_strict_version_runtime("Main Version (1440:01)").is_none());
        assert!(parse_strict_version_runtime("Main[Version] (4:41)").is_none());
    }

    #[test]
    fn trailing_catalog_duration_is_not_part_of_version_identity() {
        assert_eq!(normalize_version("Main Vox(5:25)"), "main vox");
        assert_eq!(normalize_version("Afro Mix (7:41)"), "afro mix");
        assert_eq!(normalize_version("Remix 2"), "remix 2");
    }

    #[test]
    fn display_duration_does_not_prevent_exact_mix_match() {
        let t = Track {
            version: Some("Extended Mix (5:25)".into()),
            ..track()
        };
        let mut c = candidate("Artist One - Night Drive (Extended Mix)", 360.0);
        score_candidate(&t, &mut c);
        assert_eq!(c.confidence, "high");
    }
}
