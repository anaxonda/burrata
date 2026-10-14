//! Conservative recording matching. A known version mismatch is never a recommendation.
use crate::models::{Candidate, Track};
use std::collections::HashSet;

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
    let artist_match = coverage(&artist, &format!("{upload} {uploader}"));
    let mut reasons = vec![
        format!("Title token match: {:.0}%", title_match * 100.0),
        format!("Artist token match: {:.0}%", artist_match * 100.0),
    ];
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
        && contains_phrase(&format!("{upload} {uploader}"), &artist)
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
