//! Shared interpretation of provider evidence. Identifiers locate recordings;
//! they do not authorize an upload, a different performance, or a stream URL.
use crate::models::Track;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub fn normalize_isrc(value: &str) -> Option<String> {
    let value = value.trim();
    // Accept the standard printed XX-XXX-YY-NNNNN form, not arbitrary punctuation.
    let code = if value.len() == 15
        && value.as_bytes()[2] == b'-'
        && value.as_bytes()[6] == b'-'
        && value.as_bytes()[9] == b'-'
    {
        value.replace('-', "")
    } else {
        value.to_owned()
    }
    .to_ascii_uppercase();
    let b = code.as_bytes();
    (b.len() == 12
        && b[..2].iter().all(u8::is_ascii_alphabetic)
        && b[2..5].iter().all(u8::is_ascii_alphanumeric)
        && b[5..].iter().all(u8::is_ascii_digit))
    .then_some(code)
}

/// Invalid strings never count as matching identifiers.
pub fn same_isrc(a: &str, b: &str) -> bool {
    normalize_isrc(a)
        .zip(normalize_isrc(b))
        .is_some_and(|(a, b)| a == b)
}

/// Accent folding is a search/album comparison aid, never an identity proof.
pub fn search_fold(value: &str) -> String {
    let decomposed = icu_normalizer::DecomposingNormalizerBorrowed::new_nfd().normalize(value);
    crate::matching::normalize(
        &decomposed
            .chars()
            .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
            .collect::<String>(),
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecognitionIdentifiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shazam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apple_music_url: Option<String>,
}
impl RecognitionIdentifiers {
    pub fn from_songrec(track: &Value) -> Self {
        let mut result = Self {
            isrc: track["isrc"].as_str().and_then(normalize_isrc),
            shazam_id: track["key"]
                .as_str()
                .filter(|s| numeric_id(s))
                .map(str::to_owned),
            apple_music_url: None,
        };
        // Only explicit catalog links are retained. Artwork names and Shazam IDs
        // must never be interpreted as Apple recording IDs.
        for option in track["hub"]["options"]
            .as_array()
            .into_iter()
            .flatten()
            .take(16)
        {
            for action in option["actions"].as_array().into_iter().flatten().take(16) {
                if let Some(url) = action["uri"].as_str().and_then(apple_music_url) {
                    result.apple_music_url = Some(url);
                    return result;
                }
            }
        }
        result
    }
}
fn numeric_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}
fn apple_music_url(s: &str) -> Option<String> {
    if s.len() > 2048 {
        return None;
    }
    let u = url::Url::parse(s).ok()?;
    if u.scheme() != "https"
        || u.host_str() != Some("music.apple.com")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
    {
        return None;
    }
    let song = u.path().split('/').any(|part| part == "song")
        && u.path_segments()?.next_back().is_some_and(numeric_id);
    let album_song = u.path().split('/').any(|part| part == "album")
        && u.query_pairs().any(|(k, v)| k == "i" && numeric_id(&v));
    (song || album_song).then(|| u.into())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingCredit {
    pub name: String,
    pub role: String,
    pub provider_id: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingIdentity {
    pub isrc: Option<String>,
    pub identifier_conflict: bool,
    pub credits: Vec<RecordingCredit>,
    pub credits_complete: bool,
    pub recording_id: Option<String>,
    pub work_ids: Vec<String>,
    pub release_group_id: Option<String>,
    pub album_id: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
}
impl RecordingIdentity {
    pub fn from_track(track: &Track) -> Self {
        let p = &track.provenance;
        let codes: BTreeSet<_> = [
            track.isrc.as_deref(),
            p["remoteCatalog"]["isrc"].as_str(),
            p["recordingContext"]["isrc"].as_str(),
            p["recognitionIdentifiers"]["isrc"].as_str(),
        ]
        .into_iter()
        .enumerate()
        .filter(|(index, _)| *index == 0 || p["manualFields"]["isrc"] != true)
        .filter_map(|(_, value)| value)
        .filter_map(normalize_isrc)
        .collect();
        let refs = p["catalog"]["artists"]
            .as_array()
            .or_else(|| p["remoteCatalog"]["artistRefs"].as_array())
            .or_else(|| p["artistCredits"].as_array());
        let mut credits = Vec::new();
        let mut complete = refs.is_some_and(|r| !r.is_empty() && r.len() <= 32);
        for r in refs.into_iter().flatten().take(32) {
            let name = r["name"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 512);
            if let Some(name) = name {
                credits.push(RecordingCredit {
                    name: name.to_owned(),
                    role: r["role"].as_str().unwrap_or("artist").to_owned(),
                    provider_id: r["id"]
                        .as_str()
                        .or_else(|| r["artist"]["id"].as_str())
                        .map(str::to_owned),
                });
            } else {
                complete = false;
            }
        }
        let declared = p["catalog"]["artistCount"]
            .as_u64()
            .or_else(|| p["remoteCatalog"]["artistCount"].as_u64());
        if declared.is_some_and(|n| n != credits.len() as u64) {
            complete = false;
        }
        // Contributor roles inform search; they never stand in for performer credits.
        for relation in p["recordingRelations"]
            .as_array()
            .into_iter()
            .flatten()
            .take(64)
        {
            if let (Some(role), Some(name)) = (
                relation["type"].as_str(),
                relation["artist"]["name"].as_str(),
            ) {
                if matches!(
                    role,
                    "composer" | "conductor" | "orchestra" | "instrument" | "vocal" | "performer"
                ) && !name.trim().is_empty()
                    && name.len() <= 512
                {
                    credits.push(RecordingCredit {
                        name: name.to_owned(),
                        role: role.to_owned(),
                        provider_id: relation["artist"]["id"].as_str().map(str::to_owned),
                    });
                }
            }
        }
        let bounded = |v: &Value| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .map(str::to_owned)
        };
        Self {
            isrc: (codes.len() == 1).then(|| codes.iter().next().unwrap().clone()),
            identifier_conflict: codes.len() > 1,
            credits,
            credits_complete: complete,
            recording_id: bounded(&p["recordingId"]),
            work_ids: p["recordingRelations"]
                .as_array()
                .into_iter()
                .flatten()
                .take(64)
                .filter_map(|relation| relation["work"]["id"].as_str())
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                .map(str::to_owned)
                .collect(),
            release_group_id: bounded(&p["releaseGroupId"]),
            album_id: bounded(&p["recordingContext"]["albumId"]),
            track_number: track.track_number,
            disc_number: track.disc_number,
        }
    }
}

/// Promote compatible nested evidence without changing user-entered tags.
pub fn hydrate_identifiers(track: &mut Track) {
    if track.provenance["manualFields"]["isrc"] == true {
        return;
    }
    let identity = RecordingIdentity::from_track(track);
    if !identity.identifier_conflict {
        if let Some(code) = identity.isrc {
            track.isrc = Some(code);
        }
    }
}

pub fn placeholder_title(title: &str) -> bool {
    matches!(
        crate::matching::normalize(title).as_str(),
        "unknown" | "data track" | "untitled data track"
    )
}

/// Complete album timeline correspondence. A caller must additionally bind
/// album IDs, artists, titles and target recording; durations alone prove nothing.
pub fn complete_album_timeline(left: &[f64], right: &[f64]) -> Option<Vec<usize>> {
    if !(8..=50).contains(&left.len())
        || left.len() != right.len()
        || left
            .iter()
            .chain(right)
            .any(|d| !d.is_finite() || !(1.0..=600.0).contains(d))
    {
        return None;
    }
    let mut used = BTreeSet::new();
    left.iter()
        .map(|a| {
            let hits: Vec<_> = right
                .iter()
                .enumerate()
                .filter(|(_, b)| (*a - **b).abs() <= 0.01)
                .map(|(i, _)| i)
                .collect();
            if hits.len() != 1 || !used.insert(hits[0]) {
                return None;
            }
            Some(hits[0])
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedRecordingTitle {
    pub title: String,
    pub featured_credit: Option<String>,
    pub version: Option<String>,
    pub soundtrack: Option<String>,
}
fn game_level_difficulty(base: &str, label: &str) -> bool {
    let lower = base.trim().to_ascii_lowercase();
    let Some(after_level) = lower.strip_prefix("level ") else {
        return false;
    };
    let digit_count = after_level.chars().take_while(char::is_ascii_digit).count();
    if digit_count == 0 {
        return false;
    }
    let remainder = after_level[digit_count..].trim_start();
    let has_separator = remainder.starts_with('-') || remainder.starts_with(':');
    // Catalogs commonly omit the punctuation used by the original game
    // caption ("Level 2 Miami") while retaining the same level/work token.
    // This remains a contextual rule: it only classifies a trailing known
    // difficulty label on a numbered-level title.
    let work = remainder
        .strip_prefix('-')
        .or_else(|| remainder.strip_prefix(':'))
        .unwrap_or(remainder)
        .trim();
    if work.is_empty()
        || (!has_separator && !crate::matching::normalize(label).ends_with(" difficulty"))
    {
        return false;
    }
    let mut difficulty = crate::matching::normalize(label);
    if let Some(prefix) = difficulty.strip_suffix(" difficulty") {
        difficulty = prefix.to_owned();
    }
    matches!(
        difficulty.as_str(),
        "easy" | "normal" | "hard" | "insane" | "expert" | "nightmare" | "extreme"
    )
}
/// Only explicit trailing labels are split. Work/movement names, dedications
/// and ambiguous parentheses remain part of the title. Original tags are kept.
pub fn parse_title(value: &str) -> ParsedRecordingTitle {
    let mut result = ParsedRecordingTitle {
        title: value.to_owned(),
        featured_credit: None,
        version: None,
        soundtrack: None,
    };
    if value.len() > 2048 {
        return result;
    }
    if let Some((base, label)) = crate::matching::split_balanced_trailing_parenthetical(value) {
        if game_level_difficulty(base, label) {
            result.title = base.trim().to_owned();
            result.version = Some(label.trim().to_owned());
            return result;
        }
    }
    let bracketed = crate::matching::split_balanced_trailing_parenthetical(value)
        .or_else(|| value.strip_suffix(']').and_then(|s| s.rsplit_once(" [")));
    if let Some((base, label)) = bracketed {
        let lower = label.to_ascii_lowercase();
        if let Some(credit) = lower
            .strip_prefix("feat. ")
            .or_else(|| lower.strip_prefix("feat "))
            .or_else(|| lower.strip_prefix("featuring "))
            .or_else(|| lower.strip_prefix("w/").map(str::trim_start))
        {
            let offset = label.len() - credit.len();
            if !credit.trim().is_empty() {
                result.title = base.trim().to_owned();
                result.featured_credit = Some(label[offset..].trim().to_owned());
            }
        } else if lower.starts_with("from \"") && lower.ends_with('"') {
            result.title = base.trim().to_owned();
            result.soundtrack = Some(label.to_owned());
        } else if [
            "live",
            "live at ",
            "live on ",
            "live in ",
            "instrumental",
            "acoustic",
            "karaoke",
            "demo",
            "radio edit",
            "remix",
            "extended mix",
        ]
        .iter()
        .any(|label| lower == *label || (label.ends_with(' ') && lower.starts_with(label)))
            || [" remix", " mix", " edit", " version"]
                .iter()
                .any(|suffix| lower.ends_with(suffix))
        {
            result.title = base.trim().to_owned();
            result.version = Some(label.to_owned());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reported_titles_preserve_guests_versions_and_work_names() {
        let t = parse_title("Turtledove (Epiros) [feat. Michalis Zambas]");
        assert_eq!(t.title, "Turtledove (Epiros)");
        assert_eq!(t.featured_credit.as_deref(), Some("Michalis Zambas"));
        assert_eq!(
            parse_title("Matthew (Live at Red Rocks)")
                .version
                .as_deref(),
            Some("Live at Red Rocks")
        );
        assert_eq!(
            parse_title("Marion's Theme (From \"Raiders of the Lost Ark\")").title,
            "Marion's Theme"
        );
        assert_eq!(
            parse_title("Symphony No. 5 (II. Andante)").title,
            "Symphony No. 5 (II. Andante)"
        );
        assert_eq!(
            parse_title("Ripple (Para Nana)").title,
            "Ripple (Para Nana)"
        );
        for raw in ["Diaraby (w/ Ry Cooder)", "Diaraby [w/ Ry Cooder]"] {
            let parsed = parse_title(raw);
            assert_eq!(parsed.title, "Diaraby");
            assert_eq!(parsed.featured_credit.as_deref(), Some("Ry Cooder"));
            assert!(parsed.version.is_none());
        }
        assert_eq!(parse_title("Diaraby (w/ )").title, "Diaraby (w/ )");
    }
    #[test]
    fn game_level_difficulty_is_version_only_in_level_track_context() {
        let parsed = parse_title("Level 2 - Miami (Insane)");
        assert_eq!(parsed.title, "Level 2 - Miami");
        assert_eq!(parsed.version.as_deref(), Some("Insane"));
        let parsed = parse_title("Level 2 - Miami (Insane Difficulty)");
        assert_eq!(parsed.title, "Level 2 - Miami");
        assert_eq!(parsed.version.as_deref(), Some("Insane Difficulty"));
        let catalog = parse_title("Level 2 Miami (Insane Difficulty)");
        assert_eq!(catalog.title, "Level 2 Miami");
        assert_eq!(catalog.version.as_deref(), Some("Insane Difficulty"));
        assert_eq!(
            parse_title("Level 2 Miami (Insane)").title,
            "Level 2 Miami (Insane)"
        );
        let ordinary = parse_title("Song (Insane)");
        assert_eq!(ordinary.title, "Song (Insane)");
        assert!(ordinary.version.is_none());
        let non_level = parse_title("Miami - Level 2 (Insane)");
        assert_eq!(non_level.title, "Miami - Level 2 (Insane)");
        assert!(non_level.version.is_none());
    }
    #[test]
    fn complete_album_mapping_requires_every_unique_timeline() {
        let a: Vec<_> = (0..12).map(|i| 100.0 + i as f64 * 10.0).collect();
        let mut b = a.clone();
        b.reverse();
        assert_eq!(complete_album_timeline(&a, &b).unwrap()[0], 11);
        b[0] = b[1];
        assert!(complete_album_timeline(&a, &b).is_none());
        assert!(complete_album_timeline(&a, &a[..11]).is_none());
    }
    #[test]
    fn manual_identifiers_and_legacy_raw_values_survive_hydration() {
        let mut t = Track {
            isrc: Some("USABC2600001".into()),
            provenance: json!({"manualFields":{"isrc":true},"recordingContext":{"isrc":"BRL6M0900003"}}),
            ..Default::default()
        };
        assert!(!RecordingIdentity::from_track(&t).identifier_conflict);
        hydrate_identifiers(&mut t);
        assert_eq!(t.isrc.as_deref(), Some("USABC2600001"));
        t.provenance = json!({});
        t.isrc = Some("legacy unknown".into());
        hydrate_identifiers(&mut t);
        assert_eq!(t.isrc.as_deref(), Some("legacy unknown"));
    }
    #[test]
    fn composition_identity_does_not_become_a_performer_credit() {
        let t = Track {
            provenance: json!({"recordingRelations":[{"type":"composer","artist":{"name":"John Williams"}},{"type":"performance","work":{"id":"00000000-0000-4000-8000-000000000001"}}]}),
            ..Default::default()
        };
        let i = RecordingIdentity::from_track(&t);
        assert!(!i.credits_complete);
        assert_eq!(i.credits[0].role, "composer");
        assert_eq!(i.work_ids.len(), 1);
    }
    #[test]
    fn canonical_isrc_accepts_printed_form_and_rejects_garbage() {
        assert_eq!(
            normalize_isrc("br-l6m-09-00003").as_deref(),
            Some("BRL6M0900003")
        );
        for value in [
            "BR L6M0900003",
            "BRL6M090000",
            "BRL6M0900003extra",
            "BЯL6M0900003",
        ] {
            assert!(normalize_isrc(value).is_none());
        }
    }
    #[test]
    fn nested_ids_are_promoted_but_conflicts_and_manual_values_are_not_overwritten() {
        let mut t = Track {
            provenance: json!({"recordingContext":{"isrc":"BR-L6M-09-00003"}}),
            ..Default::default()
        };
        hydrate_identifiers(&mut t);
        assert_eq!(t.isrc.as_deref(), Some("BRL6M0900003"));
        t.provenance["remoteCatalog"] = json!({"isrc":"USABC2600001"});
        assert!(RecordingIdentity::from_track(&t).identifier_conflict);
        hydrate_identifiers(&mut t);
        assert_eq!(t.isrc.as_deref(), Some("BRL6M0900003"));
    }
    #[test]
    fn normalization_preserves_unicode_identity_and_uses_accents_only_for_search() {
        assert_eq!(
            crate::matching::normalize("Cá"),
            crate::matching::normalize("Ca\u{301}")
        );
        assert_eq!(search_fold("Cá Pra Nós"), "ca pra nos");
        assert_ne!(
            crate::matching::normalize("Cá"),
            crate::matching::normalize("Ca")
        );
    }
    #[test]
    fn recognition_links_are_explicit_and_track_specific() {
        let r = RecognitionIdentifiers::from_songrec(
            &json!({"key":"123", "isrc":"BRL6M0900003", "hub":{"options":[{"actions":[{"uri":"https://music.apple.com/fr/album/title/123?i=456"}]}]}}),
        );
        assert_eq!(r.shazam_id.as_deref(), Some("123"));
        assert!(r.apple_music_url.is_some());
        assert!(apple_music_url("https://music.apple.com.evil.test/song/123").is_none());
        assert!(apple_music_url("https://music.apple.com/album/title/123").is_none());
    }
    #[test]
    fn malformed_or_truncated_credits_are_not_complete() {
        let t = Track {
            provenance: json!({"catalog":{"artists":[{"name":"A","role":"composer"},{}],"artistCount":3}}),
            ..Default::default()
        };
        let i = RecordingIdentity::from_track(&t);
        assert!(!i.credits_complete);
        assert_eq!(i.credits[0].role, "composer");
    }
}
