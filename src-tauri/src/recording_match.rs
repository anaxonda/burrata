//! Typed, conservative comparison of a requested recording with provider evidence.
//! Legacy confidence and reason strings are intentionally not consulted here.
use crate::{
    models::{Candidate, Track},
    recording_identity::{normalize_isrc, parse_title, RecordingIdentity},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const EVALUATOR_VERSION: &str = "1";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchLevel {
    Exact,
    #[default]
    Balanced,
    Flexible,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchStage {
    Search,
    Resolved,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Comparison {
    Agree,
    Unknown,
    Conflict,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchRelationship {
    Exact,
    Probable,
    Alternative,
    Unresolved,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchAction {
    Inspect,
    Accept,
    Enrich,
    Reject,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchReason {
    TitleAgrees,
    TitleConflict,
    ArtistAgrees,
    ArtistConflict,
    GuestAgrees,
    GuestMissing,
    GuestConflict,
    VersionAgrees,
    VersionConflict,
    RuntimeAgrees,
    RuntimeConflict,
    RuntimeVarianceAccepted,
    IsrcAgrees,
    IsrcConflict,
    WorkLinked,
    InsufficientEvidence,
    ExtractorNeedsInspection,
    PermissionRestricted,
    PreviewTooShort,
    ConflictingRequestIdentifiers,
    TitleCreditHintUnverified,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MatchDimensions {
    pub title: Comparison,
    pub artists: Comparison,
    pub guests: Comparison,
    pub version: Comparison,
    pub runtime: Comparison,
    pub isrc: Comparison,
    pub work: Comparison,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MatchAssessment {
    pub relationship: MatchRelationship,
    pub action: MatchAction,
    pub reasons: Vec<MatchReason>,
    pub dimensions: MatchDimensions,
}

fn norm(s: &str) -> String {
    crate::matching::normalize(s)
}
fn norm_artist(s: &str) -> String {
    crate::matching::normalize_single_artist_name(s)
}
fn label_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn valid_duration(d: Option<f64>) -> Option<f64> {
    d.filter(|x| x.is_finite() && (1.0..=86400.0).contains(x))
}
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 2.0_f64.max(a.min(b) * 0.02)
}
fn field_artist_names(meta: &Value) -> Vec<String> {
    for key in ["artists", "artistNames"] {
        if let Some(values) = meta[key].as_array() {
            // A malformed/oversized list is not partial credit evidence.
            if values.is_empty() || values.len() > 32 {
                return Vec::new();
            }
            let mut names = Vec::new();
            for value in values {
                if let Some(name) = value.as_str() {
                    if name.trim().is_empty() || name.len() > 512 {
                        return Vec::new();
                    }
                    names.push(name.to_owned());
                } else if let Some(name) = value["name"].as_str() {
                    if name.trim().is_empty() || name.len() > 512 {
                        return Vec::new();
                    }
                    let role = value["role"].as_str().unwrap_or("artist").to_lowercase();
                    // Composer and conductor credits describe the work, not its performers.
                    if !matches!(
                        role.as_str(),
                        "composer" | "conductor" | "lyricist" | "writer" | "producer"
                    ) {
                        names.push(name.to_owned());
                    }
                } else {
                    return Vec::new();
                }
            }
            return names;
        }
    }
    // A singular catalog artist is structured evidence; uploader/channel is not.
    ["artist", "catalogArtist"]
        .iter()
        .filter_map(|k| meta[*k].as_str())
        .map(str::to_owned)
        .collect()
}
fn guest_from_title(raw: &str) -> Vec<String> {
    let parsed = parse_title(raw);
    let mut found = parsed.featured_credit.into_iter().collect::<Vec<_>>();
    let lower = raw.to_lowercase();
    for marker in [
        " - with ",
        " with ",
        " feat. ",
        " feat ",
        " featuring ",
        " ft. ",
        " ft ",
        " w/ ",
        " w/",
        "(w/ ",
        " (cover by ",
    ] {
        if let Some(i) = lower.rfind(marker) {
            let suffix = raw[i + marker.len()..].trim();
            let suffix = suffix.trim_end_matches([')', ']']).trim();
            if !suffix.is_empty() && !suffix.starts_with('(') && !suffix.starts_with('[') {
                found.push(suffix.to_owned());
                break;
            }
        }
    }
    found
}
fn base_title(raw: &str) -> String {
    let parsed = parse_title(raw);
    let mut t = parsed.title;
    if let Some((before, tail)) = t.rsplit_once(" (") {
        let marker = version_kind(tail.trim_end_matches(')'));
        if !marker.is_empty() && t.ends_with(')') {
            t = before.trim().to_owned();
        }
    }
    // Providers commonly append “- with Guest” to a catalog track title.
    let low = t.to_lowercase();
    for marker in [" - with ", " feat. ", " featuring ", " ft. "] {
        if let Some(i) = low.rfind(marker) {
            t = t[..i].trim().into();
            break;
        }
    }
    let mut t = t.trim_end_matches([')', ']']).trim();
    let low = t.to_lowercase();
    for marker in [
        " - with ",
        " feat. ",
        " featuring ",
        " ft. ",
        " w/ ",
        " w/",
        "(w/ ",
        " (cover by ",
    ] {
        if let Some(i) = low.rfind(marker) {
            t = t[..i].trim();
            break;
        }
    }
    let mut normalized = crate::matching::normalize_title_ignoring_intraword_apostrophes(t);
    for suffix in [
        " (original vocal)",
        " original vocal",
        " official audio",
        " official video",
        " official music video",
        " official lyric video",
        " visualizer",
        " preview",
    ] {
        if let Some(prefix) = normalized.strip_suffix(suffix) {
            normalized = prefix.trim().to_owned();
        }
    }
    normalized
}
fn title_key(raw: &str) -> String {
    base_title(raw).replace('\'', "")
}
fn features_match(expected: &[String], actual: &[String]) -> Comparison {
    if expected.is_empty() {
        return Comparison::Unknown;
    }
    if actual.is_empty() {
        return Comparison::Unknown;
    }
    let a = actual.iter().map(|s| norm_artist(s)).collect::<Vec<_>>();
    if expected.iter().all(|x| {
        let x = norm_artist(x);
        !x.is_empty() && a.iter().any(|y| y == &x)
    }) {
        Comparison::Agree
    } else {
        Comparison::Conflict
    }
}
fn provider_version(meta: &Value) -> Option<String> {
    meta["version"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| meta["mix"].as_str().map(str::to_owned))
        .or_else(|| {
            parse_title(
                meta["track"]
                    .as_str()
                    .or(meta["catalogTitle"].as_str())
                    .unwrap_or(&meta["title"].as_str().unwrap_or_default()),
            )
            .version
        })
}
fn candidate_version(meta: &Value, caption: &str, label: Option<&str>) -> String {
    let from_meta = provider_version(meta);
    if let Some(v) = from_meta.filter(|v| !v.is_empty()) {
        return v;
    }
    let mut cleaned = caption.trim();
    if let Some(label) = label.filter(|s| !s.trim().is_empty()) {
        let norm_label = label_key(label);
        if let Some((before, tail)) = cleaned.rsplit_once(" [") {
            if tail.ends_with(']') && label_key(&tail[..tail.len() - 1]) == norm_label {
                cleaned = before;
            }
        }
    }
    let presentation = norm(cleaned);
    if presentation.contains("cover by") {
        return "Cover".into();
    }
    if presentation.ends_with(" original vocal") || presentation.ends_with("(original vocal)") {
        return "Original Vocal".into();
    }
    if [
        "official music video",
        "official video",
        "official audio",
        "official lyric video",
        "preview",
        "visualizer",
    ]
    .iter()
    .any(|s| presentation.ends_with(s))
    {
        return "presentation video".into();
    }
    parse_title(cleaned).version.unwrap_or_default()
}
fn split_credits(value: &str) -> Vec<String> {
    let mut text = value.to_owned();
    for marker in [" featuring ", " feat. ", " feat ", " ft. ", " ft "] {
        text = text.replace(marker, ",");
    }
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn requested_credit_parts(artist: &str, guests: &[String]) -> Vec<String> {
    split_credits(&artist.replace(" & ", ",").replace(" and ", ","))
        .into_iter()
        .chain(guests.iter().cloned())
        .collect()
}
/// Expand a composite provider credit only when its components uniquely cover
/// the requested performer set. Exact single-act names are preserved first
/// (e.g. "Earth, Wind & Fire").
fn expanded_credits(names: &[String], artist: &str, guests: &[String]) -> Vec<String> {
    let expected = requested_credit_parts(artist, guests)
        .into_iter()
        .map(|name| norm_artist(&name))
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    for name in names {
        let whole = norm_artist(name);
        if expected.contains(&whole) {
            result.push(name.clone());
            continue;
        }
        let pieces = split_credits(&name.replace(" & ", ",").replace(" and ", ","));
        let normalized = pieces.iter().map(|p| norm_artist(p)).collect::<Vec<_>>();
        if normalized.len() > 1 && normalized.iter().all(|piece| expected.contains(piece)) {
            for piece in pieces {
                if !result
                    .iter()
                    .any(|existing| norm_artist(existing) == norm_artist(&piece))
                {
                    result.push(piece);
                }
            }
        } else {
            result.push(name.clone());
        }
    }
    result
}
fn artist_match(
    artist: &str,
    names: &[String],
    guests: &[String],
    level: MatchLevel,
    complete: bool,
) -> Comparison {
    if names.is_empty() {
        return Comparison::Unknown;
    }
    let mut actual = names.iter().map(|s| norm_artist(s)).collect::<Vec<_>>();
    actual.sort();
    actual.dedup();
    let whole = norm_artist(artist);
    let primary = if actual.contains(&whole) {
        vec![whole]
    } else {
        // Treat an ampersand as a multi-artist delimiter only when the
        // structured provider list can verify each resulting credit.
        let split = artist.replace(" & ", ",").replace(" and ", ",");
        let split_names = split_credits(&split);
        let split_norm = split_names
            .iter()
            .map(|s| norm_artist(s))
            .collect::<Vec<_>>();
        if split_norm.len() > 1 && split_norm.iter().all(|s| actual.contains(s)) {
            split_norm
        } else {
            split_credits(artist)
                .iter()
                .map(|s| norm_artist(s))
                .collect::<Vec<_>>()
        }
    };
    let mut expected = primary.clone();
    expected.extend(guests.iter().map(|s| norm_artist(s)));
    expected.sort();
    expected.dedup();
    if expected.is_empty() {
        return Comparison::Unknown;
    }
    let missing_guest = guests
        .iter()
        .map(|g| norm_artist(g))
        .any(|g| !actual.contains(&g));
    let extra_credit = actual.iter().any(|a| !primary.contains(a));
    if missing_guest && extra_credit {
        return Comparison::Conflict;
    }
    if missing_guest {
        return Comparison::Unknown;
    }
    if expected.len() == 1 && actual.len() > 1 && norm_artist(&names.join(", ")) == expected[0] {
        return Comparison::Agree;
    }
    if expected.iter().all(|e| actual.contains(e))
        && (level != MatchLevel::Exact || !complete || actual == expected)
    {
        Comparison::Agree
    } else if level == MatchLevel::Exact && complete {
        Comparison::Conflict
    } else if expected.iter().any(|e| actual.contains(e)) {
        Comparison::Unknown
    } else {
        Comparison::Conflict
    }
}
fn version_kind(value: &str) -> String {
    let n = norm(value);
    let words = n.split_whitespace().collect::<Vec<_>>();
    if words.first() == Some(&"live")
        || words.iter().any(|w| *w == "live" && n.starts_with("live "))
    {
        "live".into()
    } else if words.contains(&"remix") {
        "remix".into()
    } else if words.first() == Some(&"cover") || n.starts_with("cover by ") {
        "cover".into()
    } else if n == "radio edit" || n == "extended mix" {
        n
    } else if words.contains(&"acoustic") {
        "acoustic".into()
    } else if words.contains(&"instrumental") {
        "instrumental".into()
    } else if words.contains(&"demo") {
        "demo".into()
    } else if words.contains(&"karaoke") {
        "karaoke".into()
    } else if words.contains(&"edit") {
        "edit".into()
    } else {
        String::new()
    }
}
fn difficulty_label(value: &str) -> Option<&'static str> {
    let normalized = norm(value);
    let normalized = normalized
        .strip_suffix(" difficulty")
        .unwrap_or(normalized.as_str());
    match normalized {
        "easy" => Some("easy"),
        "normal" => Some("normal"),
        "hard" => Some("hard"),
        "insane" => Some("insane"),
        "expert" => Some("expert"),
        "nightmare" => Some("nightmare"),
        "extreme" => Some("extreme"),
        _ => None,
    }
}
fn level_work(value: &str) -> Option<(String, String)> {
    let lower = value.to_ascii_lowercase();
    let after = lower.strip_prefix("level ")?;
    let count = after.chars().take_while(char::is_ascii_digit).count();
    if count == 0 {
        return None;
    }
    let level = after[..count].to_owned();
    let remainder = value["level ".len() + count..].trim_start();
    let work = remainder
        .strip_prefix('-')
        .or_else(|| remainder.strip_prefix(':'))?
        .trim()
        .to_owned();
    (!work.is_empty()).then_some((level, work))
}
fn album_prefix_matches(album: &str, candidate_prefix: &str) -> bool {
    let album = norm(album);
    let prefix = norm(candidate_prefix);
    let album_words = album.split_whitespace().collect::<Vec<_>>();
    let prefix_words = prefix.split_whitespace().collect::<Vec<_>>();
    prefix_words.len() >= 2
        && prefix_words.len() <= album_words.len()
        && album_words[..prefix_words.len()] == prefix_words[..]
}
fn contextual_game_level_title(track: &Track, candidate: &str) -> Option<String> {
    let requested = parse_title(&track.title);
    let (requested_level, requested_work) = level_work(&requested.title)?;
    let requested_difficulty = difficulty_label(requested.version.as_deref()?)?;
    let album = track.album.as_deref()?;
    let lower = candidate.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(relative) = lower[search_from..].find("level ") {
        let start = search_from + relative;
        let end = candidate[start..]
            .find('-')
            .map(|offset| start + offset)
            .or_else(|| candidate[start..].find(':').map(|offset| start + offset))?;
        let prefix = candidate[..start].trim_end();
        if !album_prefix_matches(album, prefix) {
            search_from = start + "level ".len();
            continue;
        }
        let level_text = candidate[start..end].to_owned() + " - placeholder";
        let (level, _) = level_work(&level_text)?;
        if level != requested_level {
            return None;
        }
        let mut suffix = candidate[end + 1..].trim();
        let Some((before_difficulty, difficulty_text)) = suffix.rsplit_once('|') else {
            return None;
        };
        let actual_difficulty = difficulty_label(difficulty_text.trim())?;
        if actual_difficulty != requested_difficulty {
            return None;
        }
        suffix = before_difficulty.trim();
        if let Some((before_hint, hint)) = suffix.rsplit_once(" (by ") {
            if hint.ends_with(')') {
                suffix = before_hint.trim();
            }
        }
        let actual_work = crate::matching::normalize_title_ignoring_intraword_apostrophes(suffix);
        let expected_work =
            crate::matching::normalize_title_ignoring_intraword_apostrophes(&requested_work);
        if actual_work != expected_work {
            return None;
        }
        return Some(format!(
            "Level {requested_level} - {requested_work} ({})",
            requested.version?
        ));
    }
    None
}
fn unverified_by_credit_hint(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let marker = "(by ";
    let start = lower.find(marker)? + marker.len();
    let end = raw[start..].find(')')? + start;
    let name = raw[start..end].trim();
    (!name.is_empty()).then(|| name.to_owned())
}
fn semantic_version(title: &str, version: &str) -> String {
    if level_work(&parse_title(title).title).is_some() {
        if let Some(difficulty) = difficulty_label(version) {
            return format!("{difficulty} difficulty");
        }
    }
    let normalized = crate::matching::normalize_version_for_title(title, version);
    if matches!(
        normalized.as_str(),
        "original" | "original mix" | "original vocal" | "album version" | "main version"
    ) {
        String::new()
    } else {
        normalized
    }
}
fn catalog_metadata_bound(candidate: &Candidate, meta: &Value) -> bool {
    if candidate.provider == "monochrome" {
        return meta["provider"].as_str() == Some("monochrome")
            && meta["trackId"]
                .as_str()
                .is_some_and(|id| candidate.id == format!("monochrome:{id}"))
            && meta["catalogTitle"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
            && !field_artist_names(meta).is_empty();
    }
    matches!(
        meta["provider"].as_str(),
        Some("spotify" | "apple" | "deezer")
    ) && meta["catalog"].as_bool() == Some(true)
        && meta["catalogTitle"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        && !field_artist_names(meta).is_empty()
}
fn topic_metadata_complete(track: &Track, candidate: &Candidate, meta: &Value) -> bool {
    if candidate.provider != "youtube" || meta["provider"].as_str() != Some("youtube") {
        return false;
    }
    let channel = meta["channel"].as_str().unwrap_or_default();
    let topic_artist = channel
        .trim()
        .strip_suffix(" - Topic")
        .or_else(|| channel.trim().strip_suffix("- Topic"))
        .map(str::trim)
        .unwrap_or_default();
    let channel_id = meta["channelId"].as_str().unwrap_or_default();
    if topic_artist.is_empty()
        || channel_id.len() != 24
        || !channel_id.starts_with("UC")
        || !channel_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        || !meta["artistCount"]
            .as_u64()
            .is_some_and(|n| n as usize == field_artist_names(meta).len())
    {
        return false;
    }
    let expected_title = parse_title(&track.title).title;
    let actual_title = meta["track"].as_str().unwrap_or_default();
    if title_key(actual_title) != title_key(&expected_title)
        || norm_artist(topic_artist) != norm_artist(&track.artist)
        || norm_artist(meta["artist"].as_str().unwrap_or_default()) != norm_artist(&track.artist)
    {
        return false;
    }
    let artists = field_artist_names(meta);
    let required = requested_credit_parts(&track.artist, &guest_from_title(&track.title));
    if required.iter().any(|credit| {
        !artists
            .iter()
            .any(|name| norm_artist(name) == norm_artist(credit))
    }) {
        return false;
    }
    let credit_line = norm(meta["descriptionCreditLine"].as_str().unwrap_or_default());
    !credit_line.is_empty()
        && credit_line.contains(&norm(&expected_title))
        && required
            .iter()
            .all(|credit| credit_line.contains(&norm(credit)))
}
fn caption_track_title(raw: &str, requested: &str, caption: &str, label: Option<&str>) -> String {
    if title_key(raw) == title_key(requested) {
        return raw.to_owned();
    }
    for source in [raw, caption] {
        for separator in [" - ", " – ", " — ", " | "] {
            for (index, _) in source.match_indices(separator) {
                let suffix = &source[index + separator.len()..];
                let cleaned = suffix.trim();
                if title_key(cleaned) == title_key(requested) {
                    return cleaned.to_owned();
                }
                if let Some((without_label, tail)) = cleaned.rsplit_once(" [") {
                    let known = label.is_some_and(|known| {
                        tail.ends_with(']')
                            && label_key(&tail[..tail.len() - 1]) == label_key(known)
                    });
                    if (known || tail.ends_with(']'))
                        && title_key(without_label.trim()) == title_key(requested)
                    {
                        return without_label.trim().to_owned();
                    }
                }
            }
        }
    }
    raw.to_owned()
}
fn caption_artist_names(caption: &str, expected_artist: &str, guests: &[String]) -> Vec<String> {
    for separator in [" - ", " – ", " — ", " | "] {
        if let Some((prefix, suffix)) = caption.split_once(separator) {
            let suffix = base_title(suffix);
            let expected = requested_credit_parts(expected_artist, guests)
                .into_iter()
                .map(|name| norm_artist(&name))
                .collect::<Vec<_>>();
            let prefix_credits = split_credits(&prefix.replace(" & ", ",").replace(" and ", ","));
            let credits_match = norm_artist(prefix) == norm_artist(expected_artist)
                || (prefix_credits.len() > 1
                    && prefix_credits
                        .iter()
                        .map(|p| norm_artist(p))
                        .all(|p| expected.contains(&p))
                    && expected
                        .iter()
                        .all(|p| prefix_credits.iter().any(|c| norm_artist(c) == *p)));
            if credits_match && !suffix.is_empty() {
                return vec![prefix.trim().to_owned()];
            }
        }
    }
    Vec::new()
}
fn explicit_original_cover(caption: &str, original: &str, performers: &[String]) -> bool {
    let lower = caption.to_lowercase();
    if !lower.contains("cover by") {
        return false;
    }
    let prefix = caption
        .split(['-', '–', '—', '|'])
        .next()
        .unwrap_or_default()
        .trim();
    if norm_artist(prefix) != norm_artist(original) {
        return false;
    }
    let after = lower
        .rsplit_once("cover by")
        .map(|(_, s)| s.trim().trim_end_matches([')', ']']))
        .unwrap_or_default();
    !after.is_empty()
        && performers
            .iter()
            .any(|p| norm_artist(p) == norm_artist(after))
}

pub fn assess(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
) -> MatchAssessment {
    let request_identity = RecordingIdentity::from_track(track);
    if request_identity.identifier_conflict {
        return MatchAssessment {
            relationship: MatchRelationship::Unresolved,
            action: MatchAction::Enrich,
            reasons: vec![MatchReason::ConflictingRequestIdentifiers],
            dimensions: MatchDimensions {
                title: Comparison::Unknown,
                artists: Comparison::Unknown,
                guests: Comparison::Unknown,
                version: Comparison::Unknown,
                runtime: Comparison::Unknown,
                isrc: Comparison::Conflict,
                work: Comparison::Unknown,
            },
        };
    }
    let meta = candidate.source_metadata.as_ref().unwrap_or(&Value::Null);
    let request_title = parse_title(&track.title);
    let raw_candidate = meta["track"]
        .as_str()
        .or(meta["catalogTitle"].as_str())
        .unwrap_or(&candidate.title);
    let wanted_title = base_title(&request_title.title);
    let known_label = track.label.as_deref().or(meta["label"].as_str());
    let contextual_title = contextual_game_level_title(track, raw_candidate)
        .or_else(|| contextual_game_level_title(track, &candidate.title));
    let contextual_game_title = contextual_title.is_some();
    let candidate_title = contextual_title.unwrap_or_else(|| {
        caption_track_title(raw_candidate, &wanted_title, &candidate.title, known_label)
    });
    let requested_version = track
        .version
        .as_deref()
        .or(request_title.version.as_deref())
        .unwrap_or("");
    let mut actual_version = candidate_version(meta, &candidate_title, known_label);
    if actual_version == "presentation video" {
        actual_version.clear();
    }
    let requested_semantic = semantic_version(&track.title, requested_version);
    let actual_semantic = semantic_version(&candidate_title, &actual_version);
    let same_remix = version_kind(&requested_semantic) == "remix"
        && version_kind(&actual_semantic) == "remix"
        && requested_semantic == actual_semantic;
    let mut comparison_artist = track.artist.clone();
    if same_remix {
        let pieces = split_credits(&comparison_artist);
        if pieces.len() > 1 && norm(&requested_semantic).contains(&norm(&pieces[1])) {
            comparison_artist = pieces[0].clone();
        }
    }
    let title = if title_key(&request_title.title) == title_key(&candidate_title) {
        Comparison::Agree
    } else {
        Comparison::Conflict
    };
    let mut requested_guests = request_title
        .featured_credit
        .clone()
        .into_iter()
        .chain(guest_from_title(&track.title))
        .collect::<Vec<_>>();
    requested_guests.sort();
    requested_guests.dedup();
    let mut names = field_artist_names(meta);
    // Providers sometimes serialize absent credit arrays as JSON null. Null is
    // no evidence and may fall back to an independent exact caption prefix;
    // a malformed non-null structure remains a blocker to avoid truncating it.
    let metadata_declares_artists = ["artists", "artistNames"]
        .iter()
        .any(|key| meta.get(*key).is_some_and(|value| !value.is_null()));
    let mut caption_names =
        caption_artist_names(&candidate.title, &comparison_artist, &requested_guests);
    let mut caption_full_credits = !caption_names.is_empty()
        && requested_credit_parts(&comparison_artist, &requested_guests).len() > 1;
    if names.is_empty() && !metadata_declares_artists {
        names = std::mem::take(&mut caption_names);
    } else if !caption_names.is_empty() {
        // Caption corroborates a composite provider field only when it carries
        // the complete requested credit list, including any named guest.
        names.extend(caption_names);
    }
    names = expanded_credits(&names, &comparison_artist, &requested_guests);
    let mut seen_credit_keys = std::collections::HashSet::new();
    names.retain(|name| seen_credit_keys.insert(norm_artist(name)));
    caption_full_credits |= names.len() > 1
        && requested_credit_parts(&comparison_artist, &requested_guests)
            .into_iter()
            .map(|name| norm_artist(&name))
            .all(|expected| names.iter().any(|name| norm_artist(name) == expected));
    let actual_guests = guest_from_title(raw_candidate);
    let mut guests = features_match(&requested_guests, &actual_guests);
    if !requested_guests.is_empty() && guests == Comparison::Unknown && !names.is_empty() {
        let all_known = requested_guests.iter().all(|guest| {
            names
                .iter()
                .any(|name| norm_artist(name) == norm_artist(guest))
        });
        if all_known {
            guests = Comparison::Agree;
        }
    }
    if !requested_guests.is_empty() && guests == Comparison::Unknown && !names.is_empty() {
        let main = track.artist.split(',').map(norm_artist).collect::<Vec<_>>();
        let extras = names
            .iter()
            .filter(|name| !main.contains(&norm_artist(name)))
            .cloned()
            .collect::<Vec<_>>();
        if !extras.is_empty() {
            guests = features_match(&requested_guests, &extras);
        }
    }
    // Extra structured credits are permitted if they do not contradict a named guest.
    if requested_guests.is_empty() {
        guests = Comparison::Unknown;
    }
    let exact_recording_bound = meta["recordingId"]
        .as_str()
        .is_some_and(|id| !id.is_empty() && track.provenance["recordingId"].as_str() == Some(id));
    let structured_credits_complete = exact_recording_bound
        || meta["artistCount"]
            .as_u64()
            .is_some_and(|n| n as usize == names.len())
        || (meta["provider"].as_str() == Some("monochrome")
            && meta["trackId"]
                .as_str()
                .is_some_and(|id| candidate.id == format!("monochrome:{id}"))
            && !names.is_empty());
    let credits_complete = structured_credits_complete || caption_full_credits;
    let artist_guests = if level == MatchLevel::Flexible && guests == Comparison::Unknown {
        &[][..]
    } else {
        &requested_guests[..]
    };
    let mut artists = if names.is_empty() && exact_recording_bound {
        Comparison::Agree
    } else {
        artist_match(
            &comparison_artist,
            &names,
            artist_guests,
            level,
            credits_complete,
        )
    };
    if level == MatchLevel::Exact && !structured_credits_complete {
        artists = Comparison::Unknown;
    }
    let actual_kind = version_kind(&actual_version);
    let requested_kind = version_kind(&requested_semantic);
    let version_is_requested = !requested_semantic.is_empty();
    let version = if requested_semantic.is_empty() && actual_semantic.is_empty() {
        Comparison::Unknown
    } else if requested_semantic == actual_semantic {
        Comparison::Agree
    } else if actual_semantic.is_empty() {
        Comparison::Unknown
    } else {
        Comparison::Conflict
    };
    let target_runtime = valid_duration(track.duration_seconds);
    let candidate_runtime =
        valid_duration(meta["duration"].as_f64().or(candidate.duration_seconds));
    let runtime = match (target_runtime, candidate_runtime) {
        (Some(a), Some(b)) if close(a, b) => Comparison::Agree,
        (Some(_), Some(_)) => Comparison::Conflict,
        _ => Comparison::Unknown,
    };
    let flexible_runtime_close = level == MatchLevel::Flexible
        && target_runtime
            .zip(candidate_runtime)
            .is_some_and(|(a, b)| (a - b).abs() <= 15.0_f64.max((a * 0.15).min(45.0)));
    let authoritative = catalog_metadata_bound(candidate, meta);
    let topic_full = topic_metadata_complete(track, candidate, meta);
    let balanced_runtime_variance = level == MatchLevel::Balanced
        && runtime == Comparison::Conflict
        && target_runtime
            .zip(candidate_runtime)
            .is_some_and(|(expected, actual)| {
                (expected - actual).abs() <= 2.0_f64.max((expected * 0.03).min(8.0))
            })
        && (authoritative && credits_complete || topic_full)
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && version != Comparison::Conflict;
    let balanced_search_variance = level == MatchLevel::Balanced
        && stage == MatchStage::Search
        && meta["playbackMusicCatalogCandidate"].as_bool() == Some(true)
        && runtime == Comparison::Conflict
        && target_runtime
            .zip(candidate_runtime)
            .is_some_and(|(expected, actual)| {
                (expected - actual).abs() <= 2.0_f64.max((expected * 0.03).min(8.0))
            })
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && version != Comparison::Conflict;
    let contextual_game_runtime_plausible = contextual_game_title
        && title == Comparison::Agree
        && target_runtime
            .zip(candidate_runtime)
            .is_some_and(|(expected, actual)| {
                let tolerance = if level == MatchLevel::Flexible {
                    15.0_f64.max((expected * 0.15).min(45.0))
                } else {
                    2.0_f64.max(expected * 0.02)
                };
                (expected - actual).abs() <= tolerance
            });
    let preview_too_short = candidate_runtime.is_some_and(|d| d <= 30.0)
        && target_runtime.is_none_or(|d| d > 30.0)
        || meta["isPreview"].as_bool() == Some(true) && target_runtime.is_none_or(|d| d > 30.0);
    let target_isrc = track.isrc.as_deref().and_then(normalize_isrc);
    let candidate_isrc = meta["isrc"].as_str().and_then(normalize_isrc);
    let isrc = match (target_isrc, candidate_isrc) {
        (Some(a), Some(b)) if a == b => Comparison::Agree,
        (Some(_), Some(_)) => Comparison::Conflict,
        _ => Comparison::Unknown,
    };
    let work_ids = [meta["workId"].as_str(), meta["work_id"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let linked = !work_ids.is_empty()
        && (request_identity
            .work_ids
            .iter()
            .any(|x| work_ids.contains(&x.as_str()))
            || track.provenance["workIds"].as_array().is_some_and(|xs| {
                xs.iter()
                    .filter_map(Value::as_str)
                    .any(|x| work_ids.contains(&x))
            }));
    let work = if linked {
        Comparison::Agree
    } else {
        Comparison::Unknown
    };
    let explicit_alt = matches!(actual_kind.as_str(), "live" | "remix" | "cover");
    let cover_evidence = actual_kind == "cover"
        && (linked || explicit_original_cover(&candidate.title, &track.artist, &names));
    let alternative = level == MatchLevel::Flexible
        && explicit_alt
        && version != Comparison::Agree
        && title == Comparison::Agree
        && (artists == Comparison::Agree || cover_evidence)
        && (linked || (actual_kind != "cover" && !actual_kind.is_empty()) || cover_evidence)
        && (requested_guests.is_empty()
            || guests == Comparison::Agree
            || guests == Comparison::Unknown);
    let mut reasons = Vec::new();
    if unverified_by_credit_hint(raw_candidate).is_some()
        || unverified_by_credit_hint(&candidate.title).is_some()
    {
        reasons.push(MatchReason::TitleCreditHintUnverified);
    }
    for (c, r) in [
        (title, MatchReason::TitleAgrees),
        (artists, MatchReason::ArtistAgrees),
        (guests, MatchReason::GuestAgrees),
        (version, MatchReason::VersionAgrees),
        (runtime, MatchReason::RuntimeAgrees),
        (isrc, MatchReason::IsrcAgrees),
        (work, MatchReason::WorkLinked),
    ] {
        if c == Comparison::Agree {
            reasons.push(r);
        }
    }
    if guests == Comparison::Conflict {
        reasons.push(MatchReason::GuestConflict);
    }
    if !requested_guests.is_empty() && guests == Comparison::Unknown {
        reasons.push(MatchReason::GuestMissing);
    }
    if version == Comparison::Conflict {
        reasons.push(MatchReason::VersionConflict);
    }
    if runtime == Comparison::Conflict {
        reasons.push(MatchReason::RuntimeConflict);
        if balanced_runtime_variance {
            reasons.push(MatchReason::RuntimeVarianceAccepted);
        }
    }
    if isrc == Comparison::Conflict {
        reasons.push(MatchReason::IsrcConflict);
    }
    if title == Comparison::Conflict {
        reasons.push(MatchReason::TitleConflict);
    }
    if artists == Comparison::Conflict {
        reasons.push(MatchReason::ArtistConflict);
    }
    if candidate.permission == "restricted" {
        reasons.push(MatchReason::PermissionRestricted);
    }
    if preview_too_short {
        reasons.push(MatchReason::PreviewTooShort);
    }
    let dimensions = MatchDimensions {
        title,
        artists,
        guests,
        version,
        runtime,
        isrc,
        work,
    };
    let hard_conflict = candidate.permission == "restricted"
        || preview_too_short
        || title == Comparison::Conflict
        || (artists == Comparison::Conflict && !alternative)
        || guests == Comparison::Conflict
        || (runtime == Comparison::Conflict
            && !alternative
            && !flexible_runtime_close
            && !balanced_runtime_variance
            && !balanced_search_variance)
        || (isrc == Comparison::Conflict && !alternative)
        || (version == Comparison::Conflict && !alternative);
    let enough = title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty()
            || guests == Comparison::Agree
            || level == MatchLevel::Flexible && guests == Comparison::Unknown);
    let flexible_evidence = level == MatchLevel::Flexible
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty()
            || guests == Comparison::Agree
            || guests == Comparison::Unknown)
        && guests != Comparison::Conflict
        && isrc != Comparison::Conflict
        && (version != Comparison::Conflict || alternative)
        && (runtime == Comparison::Agree
            || flexible_runtime_close
            || target_runtime.is_none()
                && candidate_runtime.is_some_and(|duration| duration > 30.0));
    let full_credits = credits_complete && !names.is_empty();
    let balanced_catalog_ok = level == MatchLevel::Balanced && authoritative && full_credits;
    let balanced_missing_runtime_ok = level == MatchLevel::Balanced
        && target_runtime.is_none()
        && candidate_runtime.is_some_and(|duration| duration > 30.0)
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && credits_complete
        && version != Comparison::Conflict;
    let identity_exact =
        (exact_recording_bound || isrc == Comparison::Agree || runtime == Comparison::Agree)
            && (!version_is_requested || version == Comparison::Agree);
    let mut relationship = if hard_conflict {
        MatchRelationship::Unresolved
    } else if alternative {
        MatchRelationship::Alternative
    } else if enough
        && identity_exact
        && (requested_guests.is_empty() || guests == Comparison::Agree)
    {
        MatchRelationship::Exact
    } else if enough && identity_exact {
        MatchRelationship::Probable
    } else if enough
        && (balanced_runtime_variance
            || balanced_search_variance
            || balanced_catalog_ok
            || balanced_missing_runtime_ok
            || version == Comparison::Agree
            || authoritative)
    {
        MatchRelationship::Probable
    } else if flexible_evidence {
        MatchRelationship::Probable
    } else {
        MatchRelationship::Unresolved
    };
    if level == MatchLevel::Exact
        && (runtime == Comparison::Unknown && isrc != Comparison::Agree && !exact_recording_bound
            || version_is_requested && version == Comparison::Unknown)
    {
        relationship = MatchRelationship::Unresolved;
    }
    let action = if hard_conflict {
        MatchAction::Reject
    } else if contextual_game_title
        && stage == MatchStage::Search
        && !contextual_game_runtime_plausible
    {
        MatchAction::Reject
    } else if stage == MatchStage::Search {
        if title == Comparison::Agree && artists != Comparison::Conflict {
            MatchAction::Inspect
        } else {
            MatchAction::Reject
        }
    } else if requested_kind.is_empty() && !actual_kind.is_empty() && !alternative {
        MatchAction::Reject
    } else if version_is_requested && actual_version.is_empty() && level != MatchLevel::Flexible {
        MatchAction::Enrich
    } else if level == MatchLevel::Exact && version_is_requested && version == Comparison::Unknown {
        MatchAction::Inspect
    } else if level == MatchLevel::Exact
        && !requested_version.is_empty()
        && requested_semantic.is_empty()
    {
        MatchAction::Inspect
    } else if level == MatchLevel::Exact && version_is_requested && artists == Comparison::Unknown {
        MatchAction::Inspect
    } else {
        match relationship {
            MatchRelationship::Exact => MatchAction::Accept,
            MatchRelationship::Probable
                if authoritative && full_credits && level == MatchLevel::Balanced =>
            {
                MatchAction::Accept
            }
            MatchRelationship::Probable if balanced_missing_runtime_ok => MatchAction::Accept,
            MatchRelationship::Probable if balanced_runtime_variance => MatchAction::Accept,
            MatchRelationship::Probable
                if level == MatchLevel::Flexible
                    && (flexible_evidence
                        || !requested_guests.is_empty() && guests == Comparison::Unknown) =>
            {
                MatchAction::Accept
            }
            MatchRelationship::Probable => MatchAction::Enrich,
            MatchRelationship::Alternative => MatchAction::Accept,
            MatchRelationship::Unresolved
                if title == Comparison::Agree && artists == Comparison::Unknown =>
            {
                MatchAction::Enrich
            }
            MatchRelationship::Unresolved => MatchAction::Inspect,
        }
    };
    if relationship == MatchRelationship::Unresolved && reasons.is_empty() {
        reasons.push(MatchReason::InsufficientEvidence);
    }
    MatchAssessment {
        relationship,
        action,
        reasons,
        dimensions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn track() -> Track {
        Track {
            artist: "Main Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(200.),
            isrc: Some("USABC2600001".into()),
            ..Track::default()
        }
    }
    fn cand() -> Candidate {
        Candidate {
            provider: "catalog".into(),
            title: "Song".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(
                json!({"catalogTitle":"Song","artists":["Main Artist"],"artistCount":1,"isrc":"USABC2600001","verified":true}),
            ),
            ..Candidate::default()
        }
    }
    #[test]
    fn all_three_levels_serialize_and_assess() {
        for l in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let a = assess(&track(), &cand(), l, MatchStage::Resolved);
            assert_eq!(a.relationship, MatchRelationship::Exact);
            assert_eq!(a.action, MatchAction::Accept);
        }
    }
    #[test]
    fn conflicting_request_isrcs_require_enrichment_at_every_level() {
        let mut t = track();
        t.provenance = json!({"remoteCatalog":{"isrc":"USBBB2600002"}});
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let assessment = assess(&t, &cand(), level, MatchStage::Resolved);
            assert_eq!(assessment.relationship, MatchRelationship::Unresolved);
            assert_eq!(assessment.action, MatchAction::Enrich);
            assert!(assessment
                .reasons
                .contains(&MatchReason::ConflictingRequestIdentifiers));
        }
    }
    #[test]
    fn exact_recording_binding_can_supply_missing_runtime_but_not_override_conflicts() {
        let mut t = track();
        t.duration_seconds = None;
        t.isrc = None;
        t.provenance["recordingId"] = json!("recording-proof-123");
        let mut c = cand();
        c.duration_seconds = None;
        c.source_metadata.as_mut().unwrap()["duration"] = Value::Null;
        c.source_metadata.as_mut().unwrap()["isrc"] = Value::Null;
        c.source_metadata.as_mut().unwrap()["recordingId"] = json!("recording-proof-123");
        for level in [MatchLevel::Exact, MatchLevel::Balanced] {
            let assessment = assess(&t, &c, level, MatchStage::Resolved);
            assert_eq!(assessment.relationship, MatchRelationship::Exact);
            assert_eq!(assessment.action, MatchAction::Accept);
        }

        let mut wrong_artist = c.clone();
        wrong_artist.source_metadata.as_mut().unwrap()["artists"] = json!(["Other Artist"]);
        let assessment = assess(&t, &wrong_artist, MatchLevel::Exact, MatchStage::Resolved);
        assert_eq!(assessment.action, MatchAction::Reject);
        assert_eq!(assessment.dimensions.artists, Comparison::Conflict);

        let mut wrong_version = c;
        t.version = Some("Remix".into());
        wrong_version.source_metadata.as_mut().unwrap()["version"] = json!("Live");
        let assessment = assess(&t, &wrong_version, MatchLevel::Exact, MatchStage::Resolved);
        assert_eq!(assessment.action, MatchAction::Reject);
        assert_eq!(assessment.dimensions.version, Comparison::Conflict);
    }
    #[test]
    fn wrong_guest_is_rejected() {
        let mut t = track();
        t.title = "Song (feat. Guest)".into();
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Main Artist", "Other Guest"]);
        assert_eq!(
            assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn missing_duration_can_be_balanced_with_verified_full_credits() {
        let mut c = cand();
        c.duration_seconds = None;
        c.source_metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("duration");
        assert_ne!(
            assess(&track(), &c, MatchLevel::Balanced, MatchStage::Resolved).relationship,
            MatchRelationship::Unresolved
        );
    }
    #[test]
    fn balanced_accepts_complete_caption_credits_when_request_runtime_is_missing() {
        let mut t = track();
        t.artist = "Holger Czukay, Jah Wobble & Jaki Liebezeit".into();
        t.title = "How Much Are They?".into();
        t.duration_seconds = None;
        t.isrc = None;
        let c = Candidate {
            provider: "youtube".into(),
            title: "Holger Czukay, Jah Wobble, Jaki Liebezeit - How Much Are They?".into(),
            duration_seconds: Some(289.0),
            source_metadata: Some(json!({"provider":"youtube","artists":null,"channelId":"UCx"})),
            ..Candidate::default()
        };
        let balanced = assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(balanced.relationship, MatchRelationship::Probable);
        assert_eq!(balanced.action, MatchAction::Accept);
        let exact = assess(&t, &c, MatchLevel::Exact, MatchStage::Resolved);
        assert_ne!(exact.action, MatchAction::Accept);
        assert_ne!(exact.relationship, MatchRelationship::Exact);
    }

    #[test]
    fn composite_provider_credit_is_expanded_only_to_requested_people() {
        let mut t = track();
        t.artist = "Nipsey Hussle, Bino Rideaux, Static Major".into();
        t.title = "Reckless (feat. Static Major)".into();
        t.duration_seconds = None;
        t.isrc = None;
        let candidate = |artist: &str, title: &str| Candidate {
            provider: "soundcloud".into(),
            title: title.into(),
            duration_seconds: Some(139.0),
            source_metadata: Some(
                json!({"provider":"soundcloud","artists":[artist],"artistCount":1}),
            ),
            ..Candidate::default()
        };
        let good = candidate(
            "Nipsey Hussle, Bino Rideaux",
            "Nipsey Hussle, Bino Rideaux & Static Major - Reckless (feat. Static Major)",
        );
        let assessment = assess(&t, &good, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(assessment.relationship, MatchRelationship::Probable);
        assert_eq!(assessment.action, MatchAction::Accept);

        let missing = candidate(
            "Nipsey Hussle, Bino Rideaux",
            "Nipsey Hussle & Bino Rideaux - Reckless",
        );
        let assessment = assess(&t, &missing, MatchLevel::Balanced, MatchStage::Resolved);
        assert_ne!(assessment.action, MatchAction::Accept);

        let wrong = candidate(
            "Nipsey Hussle, Bino Rideaux, Other Guest",
            "Nipsey Hussle, Bino Rideaux & Other Guest - Reckless (feat. Other Guest)",
        );
        assert_eq!(
            assess(&t, &wrong, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }

    #[test]
    fn comma_in_single_band_name_is_not_split_into_people() {
        let mut t = track();
        t.artist = "Earth, Wind & Fire".into();
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Earth, Wind & Fire"]);
        c.source_metadata.as_mut().unwrap()["artistCount"] = json!(1);
        assert_eq!(
            assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Accept
        );
    }

    #[test]
    fn balanced_allows_only_small_timing_variance_with_full_authoritative_identity() {
        let mut t = track();
        t.artist = "John Denver".into();
        t.title = "Matthew".into();
        t.duration_seconds = Some(222.426);
        t.isrc = None;
        let topic = Candidate {
            provider: "youtube".into(),
            title: "John Denver - Matthew".into(),
            duration_seconds: Some(228.0),
            uploader: Some("John Denver - Topic".into()),
            source_metadata: Some(json!({
                "provider":"youtube", "channel":"John Denver - Topic",
                "channelId":"UC-cnCe5zradDZlcI8YZboBg", "artist":"John Denver",
                "artists":["John Denver"], "artistCount":1, "track":"Matthew",
                "descriptionCreditLine":"Matthew · John Denver", "duration":228.0
            })),
            ..Candidate::default()
        };
        let accepted = assess(&t, &topic, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(accepted.relationship, MatchRelationship::Probable);
        assert_eq!(accepted.action, MatchAction::Accept);
        assert!(accepted
            .reasons
            .contains(&MatchReason::RuntimeVarianceAccepted));
        assert_eq!(accepted.dimensions.runtime, Comparison::Conflict);
        assert_ne!(
            assess(&t, &topic, MatchLevel::Exact, MatchStage::Resolved).action,
            MatchAction::Accept
        );

        let flat = Candidate {
            provider: "youtube".into(),
            title: "John Denver - Matthew".into(),
            duration_seconds: Some(216.0),
            source_metadata: Some(
                json!({"provider":"youtube","artists":["John Denver"],"artistCount":1,"track":"Matthew","duration":216.0}),
            ),
            ..Candidate::default()
        };
        assert_eq!(
            assess(&t, &flat, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );

        let locator = Candidate {
            provider: "youtube".into(),
            title: "John Denver - Matthew".into(),
            duration_seconds: Some(228.0),
            source_metadata: Some(json!({
                "provider":"youtube", "playbackMusicCatalogCandidate":true,
                "catalogTitle":"John Denver - Matthew", "catalogArtist":"John Denver",
                "duration":228.0
            })),
            ..Candidate::default()
        };
        let provisional = assess(&t, &locator, MatchLevel::Balanced, MatchStage::Search);
        assert_eq!(provisional.relationship, MatchRelationship::Probable);
        assert_eq!(provisional.action, MatchAction::Inspect);
        assert_eq!(
            assess(&t, &locator, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );

        let mut wrong_version = topic.clone();
        wrong_version.source_metadata.as_mut().unwrap()["track"] = json!("Matthew (Live)");
        assert_eq!(
            assess(
                &t,
                &wrong_version,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Reject
        );
        assert_eq!(
            assess(&t, &wrong_version, MatchLevel::Balanced, MatchStage::Search).action,
            MatchAction::Reject
        );
        let mut wrong_guest_track = t.clone();
        wrong_guest_track.title = "Matthew (feat. Guest)".into();
        let mut wrong_guest = topic.clone();
        wrong_guest.title = "John Denver - Matthew (feat. Other Guest)".into();
        wrong_guest.source_metadata.as_mut().unwrap()["track"] =
            json!("Matthew (feat. Other Guest)");
        wrong_guest.source_metadata.as_mut().unwrap()["descriptionCreditLine"] =
            json!("Matthew (feat. Other Guest) · John Denver");
        assert_eq!(
            assess(
                &wrong_guest_track,
                &wrong_guest,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Reject
        );

        let mut too_far = topic;
        too_far.duration_seconds = Some(210.0);
        too_far.source_metadata.as_mut().unwrap()["duration"] = json!(210.0);
        assert_eq!(
            assess(&t, &too_far, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }

    #[test]
    fn game_level_caption_is_contextual_title_evidence_not_artist_proof() {
        let mut t = track();
        t.artist = "Pengus".into();
        t.title = "Level 2 - Miami (Insane)".into();
        t.album = Some("Neon Drive Game Soundtrack".into());
        t.duration_seconds = Some(98.0);
        t.isrc = None;
        let candidate = Candidate {
            provider: "youtube".into(),
            title: "Miami (by Pengus) | Insane Difficulty".into(),
            duration_seconds: Some(110.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"NEON DRIVE Level 2 - Miami (by Pengus) | Insane Difficulty",
                "catalogTitle":"NEON DRIVE Level 2 - Miami (by Pengus) | Insane Difficulty",
                "duration":110.0,
                "sourceMetadataOrigin":"ytDlpCandidateMetadata"
            })),
            ..Candidate::default()
        };
        let search = assess(&t, &candidate, MatchLevel::Flexible, MatchStage::Search);
        assert_eq!(search.action, MatchAction::Inspect);
        assert_eq!(search.dimensions.title, Comparison::Agree);
        assert_eq!(search.dimensions.version, Comparison::Agree);
        assert_eq!(search.dimensions.artists, Comparison::Unknown);
        assert!(search
            .reasons
            .contains(&MatchReason::TitleCreditHintUnverified));
        assert_ne!(search.relationship, MatchRelationship::Exact);

        let resolved = assess(&t, &candidate, MatchLevel::Flexible, MatchStage::Resolved);
        assert_ne!(resolved.action, MatchAction::Accept);

        let mut wrong_album = t.clone();
        wrong_album.album = Some("Unrelated Game Soundtrack".into());
        assert_eq!(
            assess(
                &wrong_album,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Search
            )
            .action,
            MatchAction::Reject
        );
        let wrong_level = Candidate {
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"NEON DRIVE Level 3 - Miami (by Pengus) | Insane Difficulty",
                "duration":110.0
            })),
            ..candidate.clone()
        };
        assert_eq!(
            assess(&t, &wrong_level, MatchLevel::Flexible, MatchStage::Search).action,
            MatchAction::Reject
        );
        let wrong_difficulty = Candidate {
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"NEON DRIVE Level 2 - Miami (by Pengus) | Hard Difficulty",
                "duration":110.0
            })),
            ..candidate.clone()
        };
        assert_eq!(
            assess(
                &t,
                &wrong_difficulty,
                MatchLevel::Flexible,
                MatchStage::Search
            )
            .action,
            MatchAction::Reject
        );
        let no_runtime = Candidate {
            duration_seconds: None,
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"NEON DRIVE Level 2 - Miami (by Pengus) | Insane Difficulty"
            })),
            ..candidate
        };
        assert_eq!(
            assess(&t, &no_runtime, MatchLevel::Flexible, MatchStage::Search).action,
            MatchAction::Reject
        );
    }

    #[test]
    fn game_difficulty_normalization_does_not_relabel_ordinary_mix_versions() {
        assert_eq!(
            semantic_version("Level 2 - Miami (Hard)", "Hard"),
            semantic_version("Level 2 - Miami (Hard Difficulty)", "Hard Difficulty")
        );
        assert_ne!(
            semantic_version("Song", "Hard Mix"),
            semantic_version("Song", "Hard Difficulty")
        );
    }
    #[test]
    fn whitelisted_bound_catalog_without_runtime_is_accepted_at_balanced() {
        let mut t = track();
        t.duration_seconds = None;
        t.isrc = None;
        let c = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:12345678901234567".into(),
            title: "Lucas Santtana & Arto Lindsay - Nighttime in the Backyard".into(),
            source_metadata: Some(
                json!({"provider":"monochrome","trackId":"12345678901234567","catalogTitle":"Nighttime in the Backyard","artistNames":["Lucas Santtana","Arto Lindsay"],"artistCount":2}),
            ),
            ..Candidate::default()
        };
        t.artist = "Lucas Santtana & Arto Lindsay".into();
        t.title = "Nighttime in the Backyard".into();
        let a = assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(a.action, MatchAction::Accept);
        assert_eq!(a.relationship, MatchRelationship::Probable);
        assert_eq!(
            assess(&t, &c, MatchLevel::Exact, MatchStage::Resolved).action,
            MatchAction::Inspect
        );
    }
    #[test]
    fn resolved_youtube_with_structured_identity_can_be_accepted() {
        let c = Candidate {
            provider: "youtube".into(),
            title: "Main Artist - Song".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(
                json!({"provider":"youtube","track":"Song","artists":["Main Artist"],"duration":200.}),
            ),
            ..Candidate::default()
        };
        assert_eq!(
            assess(&track(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        assert_eq!(
            assess(&track(), &c, MatchLevel::Balanced, MatchStage::Search).action,
            MatchAction::Inspect
        );
    }
    #[test]
    fn null_artist_arrays_allow_independent_caption_prefix_evidence() {
        let c = Candidate {
            provider: "youtube".into(),
            title: "Main Artist - Song".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(
                json!({"provider":"youtube","track":"Song","artists":null,"artistNames":null,"duration":200.}),
            ),
            ..Candidate::default()
        };
        let assessment = assess(&track(), &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(assessment.dimensions.artists, Comparison::Agree);
        assert_eq!(assessment.action, MatchAction::Accept);
    }
    #[test]
    fn flexible_accepts_full_caption_when_request_duration_and_version_evidence_are_missing() {
        let mut t = track();
        t.duration_seconds = None;
        t.version = Some("Live Remix".into());
        let c = Candidate {
            provider: "youtube".into(),
            title: "Main Artist - Song".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(
                json!({"provider":"youtube","track":"Song","artists":null,"artistNames":null,"duration":200.}),
            ),
            ..Candidate::default()
        };
        let balanced = assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(balanced.action, MatchAction::Enrich);
        assert_eq!(balanced.relationship, MatchRelationship::Unresolved);
        let flexible = assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved);
        assert_eq!(flexible.action, MatchAction::Accept);
        assert_eq!(flexible.relationship, MatchRelationship::Probable);
        assert_eq!(flexible.dimensions.version, Comparison::Unknown);
    }
    #[test]
    fn flexible_tolerates_moderate_runtime_differences_as_probable() {
        let t = track();
        let mut c = cand();
        c.duration_seconds = Some(220.);
        c.source_metadata.as_mut().unwrap()["duration"] = json!(220.);
        c.source_metadata.as_mut().unwrap()["isrc"] = Value::Null;
        let balanced = assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(balanced.action, MatchAction::Reject);
        let flexible = assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved);
        assert_eq!(flexible.action, MatchAction::Accept);
        assert_eq!(flexible.relationship, MatchRelationship::Probable);
        assert_eq!(flexible.dimensions.runtime, Comparison::Conflict);
    }
    #[test]
    fn canonical_caption_prefix_keeps_artist_credit_and_strips_guest_trailer() {
        let mut t = track();
        t.artist = "Ali Farka Touré".into();
        t.title = "Diaraby (w/ Ry Cooder)".into();
        t.duration_seconds = Some(446.826);
        let c = Candidate {
            provider: "spotify".into(),
            title: "Ali Farka Touré, Ry Cooder - Diaraby - with Ry Cooder".into(),
            duration_seconds: Some(446.826),
            source_metadata: Some(
                json!({"provider":"spotify","catalog":true,"catalogTitle":"Ali Farka Touré, Ry Cooder - Diaraby - with Ry Cooder","artistNames":["Ali Farka Touré","Ry Cooder"],"artistCount":2,"duration":446.826}),
            ),
            ..Candidate::default()
        };
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_eq!(
                assess(&t, &c, level, MatchStage::Resolved).relationship,
                MatchRelationship::Exact
            );
        }
    }
    #[test]
    fn unrequested_live_mix_is_rejected_and_caption_prefix_is_not_uploader_evidence() {
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["track"] = json!("Song (Live)");
        assert_eq!(
            assess(&track(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
        let mut c = cand();
        c.source_metadata = None;
        c.title = "Wrong Artist - Song".into();
        c.uploader = Some("Main Artist".into());
        assert_eq!(
            assess(&track(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Enrich
        );
    }
    #[test]
    fn wrong_isrc_and_mix_and_artist_are_hard_conflicts() {
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        assert_eq!(
            assess(&track(), &c, MatchLevel::Flexible, MatchStage::Resolved).action,
            MatchAction::Reject
        );
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["track"] = json!("Song (Radio Edit)");
        let mut t = track();
        t.version = Some("Extended Mix".into());
        assert_eq!(
            assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn live_alternative_requires_song_and_artist_link() {
        let mut t = track();
        t.provenance = json!({"workIds":["work-1"]});
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["track"] = json!("Song (Live)");
        c.source_metadata.as_mut().unwrap()["isrc"] = Value::Null;
        c.source_metadata.as_mut().unwrap()["workId"] = json!("work-1");
        let a = assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved);
        assert_eq!(a.relationship, MatchRelationship::Alternative);
        c.source_metadata.as_mut().unwrap()["workId"] = json!("other-work");
        assert_eq!(
            assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved).relationship,
            MatchRelationship::Alternative
        );
        c.source_metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("workId");
        c.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        let alternative = assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved);
        assert_eq!(alternative.relationship, MatchRelationship::Alternative);
        assert_eq!(alternative.action, MatchAction::Accept);
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Different Artist"]);
        assert_eq!(
            assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn work_context_roles_do_not_replace_artist_and_topic_binding_does_not_trust_uploader() {
        let mut t = track();
        t.artist = "Composer Name".into();
        t.provenance =
            json!({"recordingRelations":[{"type":"composer","artist":{"name":"Composer Name"}}]});
        let mut c = cand();
        c.source_metadata = Some(
            json!({"catalogTitle":"Song","artists":["Performer"],"channel":"Performer - Topic"}),
        );
        assert_eq!(
            assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn classical_composer_credit_does_not_substitute_for_performer() {
        let mut t = track();
        t.artist = "Composer Name".into();
        let mut c = cand();
        c.source_metadata = Some(
            json!({"catalogTitle":"Song","artists":[{"name":"Performer Name","role":"performer"},{"name":"Composer Name","role":"composer"}],"artistCount":2}),
        );
        assert_eq!(
            assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn flexible_missing_secondary_credit_is_allowed_but_known_wrong_guest_is_rejected() {
        let mut t = track();
        t.title = "Song (feat. Guest)".into();
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Main Artist"]);
        let partial = assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved);
        assert_eq!(partial.action, MatchAction::Accept);
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Main Artist", "Other Guest"]);
        assert_eq!(
            assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn unknown_artist_evidence_at_resolution_requests_enrichment() {
        let mut c = cand();
        c.source_metadata = Some(json!({"catalogTitle":"Song"}));
        c.uploader = Some("Main Artist".into());
        let a = assess(&track(), &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(a.relationship, MatchRelationship::Unresolved);
        assert_eq!(a.action, MatchAction::Enrich);
    }
    #[test]
    fn flexible_variant_allows_duration_and_isrc_differences_but_rejects_preview_and_restricted() {
        let mut t = track();
        let mut c = cand();
        c.source_metadata.as_mut().unwrap()["track"] = json!("Song (Live)");
        c.duration_seconds = Some(260.);
        c.source_metadata.as_mut().unwrap()["duration"] = json!(260.);
        c.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        assert_eq!(
            assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        c.duration_seconds = Some(20.);
        c.source_metadata.as_mut().unwrap()["duration"] = json!(20.);
        assert_eq!(
            assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved).action,
            MatchAction::Reject
        );
        c.duration_seconds = Some(260.);
        c.source_metadata.as_mut().unwrap()["duration"] = json!(260.);
        c.permission = "restricted".into();
        let denied = assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved);
        assert_eq!(denied.action, MatchAction::Reject);
        assert!(denied.reasons.contains(&MatchReason::PermissionRestricted));
    }
}
