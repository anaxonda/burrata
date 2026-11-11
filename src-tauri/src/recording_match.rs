//! Typed, conservative comparison of a requested recording with provider evidence.
//! Legacy confidence and reason strings are intentionally not consulted here.
use crate::{
    models::{Candidate, CandidateSource, Track},
    recording_identity::{
        normalize_isrc, parse_recording_text, parse_title, AnnotationKind, AnnotationObservation,
        DurationObservation, EvidenceAuthority, EvidenceOrigin, IdentifierObservation,
        PerformerObservation, PreparedRecordingEvidence, RecordingIdentity, TextObservation,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const EVALUATOR_VERSION: &str = "30";

/// Independently selected categories of alternatives for Flexible matching.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct FlexibleMatchingOptions {
    pub version: u8,
    pub allow_other_releases: bool,
    pub allow_other_performances: bool,
    pub allow_other_mixes: bool,
    pub allow_other_artists: bool,
    pub allow_different_lengths: bool,
    /// Maximum runtime difference when the candidate is shorter than requested.
    pub max_shorter_by_seconds: u16,
    /// Maximum runtime difference when the candidate is longer than requested.
    pub max_longer_by_seconds: u16,
}

impl Default for FlexibleMatchingOptions {
    fn default() -> Self {
        Self {
            version: 1,
            allow_other_releases: true,
            allow_other_performances: false,
            allow_other_mixes: false,
            allow_other_artists: false,
            allow_different_lengths: false,
            max_shorter_by_seconds: 30,
            max_longer_by_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchLevel {
    Exact,
    #[default]
    Balanced,
    Flexible,
}
/// Product policy, separate from the observed comparisons. Flexible inherits
/// Balanced eligibility while explicitly admitting identified alternatives.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MatchPolicy {
    pub require_structured_credits: bool,
    pub allow_missing_guest_evidence: bool,
    pub allow_identified_alternatives: bool,
    pub allow_catalog_runtime_exception: bool,
}
impl MatchLevel {
    pub fn policy(self) -> MatchPolicy {
        match self {
            Self::Exact => MatchPolicy {
                require_structured_credits: true,
                allow_missing_guest_evidence: false,
                allow_identified_alternatives: false,
                allow_catalog_runtime_exception: false,
            },
            Self::Balanced => MatchPolicy {
                require_structured_credits: false,
                allow_missing_guest_evidence: false,
                allow_identified_alternatives: false,
                allow_catalog_runtime_exception: true,
            },
            Self::Flexible => MatchPolicy {
                require_structured_credits: false,
                allow_missing_guest_evidence: true,
                allow_identified_alternatives: true,
                allow_catalog_runtime_exception: true,
            },
        }
    }
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
pub enum AlternativeKind {
    Release,
    Performance,
    Remix,
    OtherArtist,
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
    SourceIdentityConflict,
    GuestAgrees,
    GuestMissing,
    GuestConflict,
    VersionAgrees,
    VersionConflict,
    UnspecifiedVersionAllowed,
    RuntimeAgrees,
    RuntimeConflict,
    DurationLimitExceeded,
    DurationLimitUnavailable,
    RuntimeVarianceAccepted,
    RecordingScopedRuntimeVariant,
    RecordingScopedTitleAlias,
    FilenameCaptionOnly,
    IsrcAgrees,
    IsrcConflict,
    WorkLinked,
    InsufficientEvidence,
    ExtractorNeedsInspection,
    PermissionRestricted,
    PreviewTooShort,
    FullAlbumContent,
    ConflictingRequestIdentifiers,
    TitleCreditHintUnverified,
    UnresolvedAnnotation,
    EmptyIdentityKey,
    ConflictingRequestVersions,
    ConflictingCandidateVersions,
    ParsingIncomplete,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ProofStrength {
    #[default]
    MetadataOnly,
    RecordingIdentifier,
    AudioFingerprint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatchAssessment {
    pub relationship: MatchRelationship,
    pub action: MatchAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternative_kind: Option<AlternativeKind>,
    pub reasons: Vec<MatchReason>,
    pub dimensions: MatchDimensions,
    #[serde(default)]
    pub proof_strength: ProofStrength,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ComparedEvidence>,
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
#[derive(Default)]
struct ArtistArrayObservation {
    names: Vec<String>,
    declared: bool,
    uncertain: bool,
}

fn artist_array_observation(meta: &Value) -> ArtistArrayObservation {
    let mut observation = ArtistArrayObservation::default();
    let mut selected_performers: Option<Vec<String>> = None;
    for key in ["artists", "artistNames"] {
        if let Some(value) = meta.get(key) {
            if value.is_null() {
                continue;
            }
            let Some(values) = value.as_array() else {
                // A malformed non-null credit field is uncertainty, not
                // permission to fall through to a different field.
                observation.declared = true;
                observation.uncertain = true;
                continue;
            };
            // A malformed/oversized list is not partial credit evidence.
            if values.is_empty() {
                continue;
            }
            observation.declared = true;
            if values.len() > 32 {
                observation.uncertain = true;
                continue;
            }
            let mut names = Vec::new();
            let mut malformed = false;
            for value in values {
                if let Some(name) = value.as_str() {
                    if name.trim().is_empty() || name.len() > 512 {
                        malformed = true;
                        break;
                    }
                    names.push(name.to_owned());
                } else if let Some(name) = value["name"].as_str() {
                    if name.trim().is_empty() || name.len() > 512 {
                        malformed = true;
                        break;
                    }
                    let role = match value.get("role") {
                        None | Some(Value::Null) => "artist",
                        Some(Value::String(role)) => role.as_str(),
                        Some(_) => {
                            malformed = true;
                            break;
                        }
                    };
                    if crate::recording_identity::is_performer_credit_role(role) {
                        names.push(name.to_owned());
                    } else if !crate::recording_identity::is_non_performer_credit_role(role) {
                        // Preserve an unfamiliar role as uncertainty rather
                        // than silently treating its artist as a nonperformer.
                        malformed = true;
                        break;
                    }
                } else {
                    malformed = true;
                    break;
                }
            }
            if malformed {
                observation.uncertain = true;
                continue;
            }
            // Separate composer or other non-performer rows do not contradict
            // a performer row for the same person. Compare only performer sets.
            if names.is_empty() {
                continue;
            }
            if let Some(existing) = &selected_performers {
                let mut existing_keys = existing
                    .iter()
                    .map(|name| norm_artist(name))
                    .collect::<Vec<_>>();
                let mut current_keys = names
                    .iter()
                    .map(|name| norm_artist(name))
                    .collect::<Vec<_>>();
                existing_keys.sort();
                current_keys.sort();
                if existing_keys != current_keys {
                    observation.uncertain = true;
                }
            } else {
                selected_performers = Some(names);
            }
        }
    }
    if observation.uncertain {
        observation.names.clear();
    } else if let Some(names) = selected_performers {
        observation.names = names;
    }
    observation
}

fn field_artist_names(meta: &Value) -> Vec<String> {
    let observation = artist_array_observation(meta);
    if observation.declared {
        return observation.names;
    }
    // A singular catalog artist is structured evidence; uploader/channel is not.
    ["artist", "catalogArtist"]
        .iter()
        .filter_map(|k| meta[*k].as_str())
        .map(str::to_owned)
        .collect()
}

fn explicit_performer_credit_completeness(candidate: &Candidate, meta: &Value) -> bool {
    if meta["provider"].as_str() != Some(candidate.provider.as_str())
        || meta["performerCreditsComplete"] != true
        || field_artist_names(meta).is_empty()
    {
        return false;
    }
    match candidate.provider.as_str() {
        "monochrome" => meta["trackId"].as_str().is_some_and(|id| {
            candidate.id == format!("monochrome:{id}")
                && meta["catalogTitle"]
                    .as_str()
                    .is_some_and(|title| !title.trim().is_empty())
        }),
        "youtube" => {
            let id = candidate.id.strip_prefix("youtube:").unwrap_or_default();
            !id.is_empty()
                && meta["resolvedProviderId"].as_str() == Some(id)
                && matches!(
                    meta["sourceMetadataOrigin"].as_str(),
                    Some("ytDlpCandidateMetadata" | "ytDlpFullInfo")
                )
        }
        _ => false,
    }
}

fn verified_musicbrainz_performer_credits_complete(
    track: &Track,
    candidate: &Candidate,
    meta: &Value,
) -> bool {
    if !matches!(candidate.provider.as_str(), "catalog" | "musicbrainz")
        || meta["provider"].as_str() != Some("musicbrainz")
        || meta["verified"] != true
        || meta["performerCreditsComplete"] != true
        || field_artist_names(meta).is_empty()
    {
        return false;
    }
    let Some(recording_id) = meta["recordingId"].as_str() else {
        return false;
    };
    uuid::Uuid::parse_str(recording_id).is_ok()
        && track.provenance["recordingId"].as_str() == Some(recording_id)
}

fn verified_recording_binding(track: &Track, candidate: &Candidate, meta: &Value) -> bool {
    let Some(recording_id) = meta["recordingId"].as_str() else {
        return false;
    };
    if recording_id.is_empty() || track.provenance["recordingId"].as_str() != Some(recording_id) {
        return false;
    }
    match candidate.provider.as_str() {
        "catalog" | "musicbrainz" => {
            meta["provider"].as_str() == Some("musicbrainz")
                && meta["verified"] == true
                && valid_musicbrainz_id(recording_id)
                && candidate.id.split(':').next_back() == Some(recording_id)
        }
        "bandcamp" => {
            meta["provider"].as_str() == Some("bandcamp")
                && meta["recordingProof"].as_str() == Some("strongBound")
        }
        _ => false,
    }
}

#[derive(Debug, Clone)]
struct VerifiedSoulseekFilenameObservations {
    normalized_title: String,
    folder_artist: Option<String>,
}

/// Peer filename observations are accepted only on a typed Soulseek candidate
/// when the complete observation object exactly matches a fresh derivation from
/// that candidate's typed filename. They remain uploader-caption evidence.
fn verified_soulseek_filename_observations(
    candidate: &Candidate,
    metadata: &Value,
) -> Option<VerifiedSoulseekFilenameObservations> {
    if candidate.provider != "slskd"
        || metadata["sourceMetadataOrigin"].as_str() != Some("slskdFilename")
    {
        return None;
    }
    let CandidateSource::Soulseek { filename, .. } = candidate.source.as_ref()? else {
        return None;
    };
    let supplied = metadata.get("filenameObservations")?;
    let raw_basename = supplied["rawBasename"].as_str()?;
    if raw_basename != candidate.title
        || supplied["authority"].as_str() != Some("uploaderCaption")
        || supplied["creditsComplete"] != false
    {
        return None;
    }
    let expected =
        crate::recording_filename::verified_filename_observations(filename, raw_basename)?;
    if supplied != &expected {
        return None;
    }
    let normalized_title = expected["normalizedTitle"].as_str()?;
    if normalized_title.trim().is_empty() || normalized_title.len() > 512 {
        return None;
    }
    let folder_artist = expected["folderArtist"]
        .as_str()
        .filter(|artist| !artist.trim().is_empty() && artist.len() <= 256)
        .map(str::to_owned);
    Some(VerifiedSoulseekFilenameObservations {
        normalized_title: normalized_title.to_owned(),
        folder_artist,
    })
}

pub(crate) fn verified_soulseek_folder_artist(candidate: &Candidate) -> Option<String> {
    let metadata = candidate.source_metadata.as_ref()?;
    verified_soulseek_filename_observations(candidate, metadata)?.folder_artist
}
/// Remove only balanced, known presentation labels. Semantic labels such as
/// Live, Instrumental, remixes or tuning annotations remain recording evidence.
fn strip_caption_presentation(mut raw: &str) -> &str {
    for _ in 0..4 {
        let bracketed = crate::matching::split_balanced_trailing_parenthetical(raw)
            .or_else(|| raw.strip_suffix(']').and_then(|s| s.rsplit_once('[')));
        let Some((base, label)) = bracketed else {
            break;
        };
        if !matches!(
            norm(label).as_str(),
            "audio"
                | "official audio"
                | "audio oficial"
                | "official video"
                | "official music video"
                | "official lyric video"
                | "lyric video"
                | "lyrics"
                | "visualizer"
        ) {
            break;
        }
        raw = base.trim_end();
    }
    raw
}

fn caption_semantic_version(raw: &str) -> Option<String> {
    let mut remaining = strip_caption_presentation(raw).to_owned();
    for _ in 0..4 {
        let parsed = parse_title(&remaining);
        if parsed.version.is_some() {
            return parsed.version;
        }
        if parsed.featured_credit.is_none() || parsed.title == remaining {
            break;
        }
        remaining = parsed.title;
    }
    None
}

fn caption_feature_credits(raw: &str) -> Vec<String> {
    let mut remaining = strip_caption_presentation(raw).to_owned();
    let mut credits = Vec::new();
    for _ in 0..4 {
        let parsed = parse_title(&remaining);
        let recognized = parsed.featured_credit.is_some() || parsed.version.is_some();
        if let Some(credit) = parsed.featured_credit {
            credits.push(credit);
        }
        if !recognized || parsed.title == remaining {
            break;
        }
        remaining = parsed.title;
    }
    credits
}

fn caption_base_without_features(raw: &str) -> String {
    let mut remaining = strip_caption_presentation(raw).to_owned();
    for _ in 0..4 {
        let parsed = parse_title(&remaining);
        if parsed.featured_credit.is_none() || parsed.title == remaining {
            break;
        }
        remaining = parsed.title;
    }
    remaining.trim().to_owned()
}

/// Read feature names from the side of a caption separator whose base matches
/// the requested recording title. This avoids treating an artist name such as
/// "Band Featuring Friends" as a guest annotation in artist-first captions.
fn caption_features_for_requested_title(
    raw: &str,
    requested_title: &str,
    expected_credits: &[String],
) -> Vec<String> {
    let wanted = title_key(requested_title);
    if wanted.is_empty() || raw.len() > 2048 {
        return Vec::new();
    }
    for separator in [" - ", " – ", " — ", " | "] {
        for (index, _) in raw.match_indices(separator) {
            let prefix = raw[..index].trim();
            let suffix = raw[index + separator.len()..].trim();
            if title_key(&caption_base_without_features(suffix)) == wanted {
                let prefix_is_expected_whole_credit = expected_credits
                    .iter()
                    .any(|credit| norm_artist(credit) == norm_artist(prefix));
                let mut features = if prefix_is_expected_whole_credit {
                    Vec::new()
                } else {
                    caption_feature_credits(prefix)
                };
                features.extend(caption_feature_credits(suffix));
                return features;
            }
        }
    }
    guest_from_title(raw)
}

/// Interpret the less common `Title - Artist, Guest` filename ordering only
/// for a Soulseek filename caption whose left side is the exact requested
/// base title and whose right side names every requested performer. This is
/// request-scoped uploader-caption evidence; extra names remain open-world and
/// never establish complete credits.
fn title_first_filename_caption(track: &Track, raw: &str) -> Option<(String, Vec<String>)> {
    let (raw_title, raw_credits) = raw.split_once(" - ")?;
    if raw_title.trim().is_empty()
        || raw_credits.trim().is_empty()
        || raw.len() > 2048
        || raw_credits.len() > 1536
    {
        return None;
    }

    let requested = parse_title(&track.title);
    // A filename cannot establish a requested or candidate edition. Leave any
    // semantic annotation for the ordinary evaluator to inspect or reject.
    if requested.version.is_some()
        || track
            .version
            .as_deref()
            .is_some_and(|version| !version.trim().is_empty())
    {
        return None;
    }
    let title = parse_recording_text(raw_title.trim());
    let parsed_credits = parse_recording_text(raw_credits.trim());
    if title.parsing_incomplete
        || !title.annotations.is_empty()
        || parsed_credits.parsing_incomplete
        || !parsed_credits.annotations.is_empty()
        || title_key(&title.base_title) != title_key(&requested.title)
    {
        return None;
    }

    let requested_guests = requested
        .featured_credit
        .into_iter()
        .chain(guest_from_title(&track.title))
        .collect::<Vec<_>>();
    let required = requested_credit_parts_for_track(track, &track.artist, &requested_guests);
    if required.is_empty() || required.len() > 16 {
        return None;
    }

    let raw_parts = split_credits(raw_credits.trim());
    if raw_parts.is_empty() || raw_parts.len() > 32 || raw_parts.iter().any(|part| part.len() > 256)
    {
        return None;
    }
    // Expand a compound caption credit only when its components independently
    // cover requested credits; otherwise preserve it as one open-world credit.
    let mut credits = expanded_credits(&raw_parts, &required);
    let mut seen = std::collections::HashSet::new();
    credits.retain(|credit| seen.insert(norm_artist(credit)));
    let actual = credits
        .iter()
        .map(|credit| norm_artist(credit))
        .collect::<Vec<_>>();
    if required
        .iter()
        .map(|credit| norm_artist(credit))
        .any(|credit| credit.is_empty() || !actual.contains(&credit))
    {
        return None;
    }
    Some((title.base_title, credits))
}

fn guest_from_title(raw: &str) -> Vec<String> {
    let raw = strip_caption_presentation(raw);
    let parsed = parse_recording_text(raw);
    let mut found = parsed
        .annotations
        .iter()
        .filter(|annotation| annotation.kind == AnnotationKind::FeaturedCredit)
        .map(|annotation| annotation.value.clone())
        .collect::<Vec<_>>();
    let lower = raw.to_ascii_lowercase();
    for marker in [
        // Bare "with" is ordinary title text (e.g. "Vibe With Me").
        // A credit needs an explicit annotation such as " - with Guest".
        " - with ",
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
    let mut t = parse_recording_text(raw).title_without_recognized_annotations();
    // Providers commonly append “- with Guest” to a catalog track title.
    let low = t.to_ascii_lowercase();
    for marker in [" - with ", " feat. ", " featuring ", " ft. "] {
        if let Some(i) = low.rfind(marker) {
            t = t[..i].trim().into();
            break;
        }
    }
    let mut t = t.trim_end_matches([')', ']']).trim();
    let low = t.to_ascii_lowercase();
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

/// Detect one-to-one accent-folded performer coverage without changing the
/// canonical artist key used anywhere else in matching. Callers must apply
/// the strict recording/title/runtime guards before using this as evidence.
fn accent_folded_covers_requested_credits(expected: &[String], actual: &[String]) -> bool {
    if expected.is_empty() || actual.is_empty() {
        return false;
    }
    let mut expected_seen = std::collections::HashSet::new();
    let mut matched_actual = std::collections::HashSet::new();
    let mut used_accent_variant = false;
    for requested in expected {
        let requested_key = norm_artist(requested);
        let folded_key = crate::recording_identity::search_fold(requested);
        if requested_key.is_empty() || folded_key.is_empty() {
            return false;
        }
        if !expected_seen.insert(requested_key.clone()) {
            continue;
        }
        let matches = actual
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                crate::recording_identity::search_fold(candidate) == folded_key
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 || !matched_actual.insert(matches[0].0) {
            return false;
        }
        used_accent_variant |= norm_artist(matches[0].1) != requested_key;
    }
    used_accent_variant
}

/// One-to-one accent-insensitive coverage used only when a validated caption
/// and uploader independently corroborate the requested act. This does not
/// create a general artist alias: the caller also requires exact title,
/// matching runtime, nonconflicting edition, and a source-ID-bound caption.
fn search_folded_covers_requested_credits(expected: &[String], actual: &[String]) -> bool {
    if expected.is_empty() || actual.is_empty() {
        return false;
    }
    let mut expected_seen = std::collections::HashSet::new();
    let mut matched_actual = std::collections::HashSet::new();
    for requested in expected {
        let requested_key = norm_artist(requested);
        let folded_key = crate::recording_identity::search_fold(requested);
        if requested_key.is_empty() || folded_key.is_empty() {
            return false;
        }
        if !expected_seen.insert(requested_key) {
            continue;
        }
        let matches = actual
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                crate::recording_identity::search_fold(candidate) == folded_key
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 || !matched_actual.insert(matches[0].0) {
            return false;
        }
    }
    true
}

fn provider_version(meta: &Value) -> Option<String> {
    meta["version"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            meta["mix"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            caption_semantic_version(
                meta["track"]
                    .as_str()
                    .or(meta["catalogTitle"].as_str())
                    .unwrap_or(meta["title"].as_str().unwrap_or_default()),
            )
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
    if let Some(version) = caption_semantic_version(cleaned) {
        return version;
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
    caption_semantic_version(cleaned).unwrap_or_default()
}
fn prepared_semantic_observations(
    evidence: &PreparedRecordingEvidence,
    requested: &str,
    context_title: &str,
    label: Option<&str>,
) -> Vec<String> {
    let mut values = Vec::new();
    for observation in &evidence.texts {
        let field = observation.origin.field.as_str();
        if matches!(
            field,
            "version" | "sourceMetadata.version" | "sourceMetadata.mix"
        ) {
            if !observation.parsed.original.trim().is_empty() {
                values.push(semantic_version(
                    context_title,
                    &observation.parsed.original,
                ));
            }
            continue;
        }
        let base = caption_track_title(
            &observation.parsed.base_title,
            requested,
            &observation.parsed.base_title,
            label,
        );
        if field != "title" && !field.starts_with("sourceMetadata.") {
            continue;
        }
        if !title_key(requested).is_empty() && title_key(&base) == title_key(requested) {
            values.extend(
                observation
                    .parsed
                    .semantic_versions()
                    .map(|version| semantic_version(context_title, version)),
            );
        }
    }
    // Original/album labels can accompany mastering labels, but an explicit
    // Original observation still contradicts Live or a named alternate mix.
    // Keep that empty semantic key as a conflict witness in the latter case.
    if !values
        .iter()
        .any(|value| !value.is_empty() && !crate::recording_identity::is_release_version(value))
    {
        values.retain(|value| !value.is_empty());
    }
    values.sort();
    values.dedup();
    // Generic "Remix" and a named remix are a compatible refinement; Original
    // versus Live is not. Preserve the specific observation for comparison.
    let generic = values
        .iter()
        .filter(|value| !version_kind(value).is_empty() && value.as_str() == version_kind(value))
        .filter(|value| {
            values
                .iter()
                .any(|other| other != *value && version_kind(other) == version_kind(value))
        })
        .cloned()
        .collect::<Vec<_>>();
    values.retain(|value| !generic.contains(value));
    values
}
/// A mastering/release label can describe the same live performance or mix.
/// Continue rejecting contradictory labels within either dimension. This
/// intentionally does not infer that two different performance/mix labels
/// identify one recording just because they occur together.
fn conflicting_semantic_observations(values: &[String]) -> bool {
    if values.len() <= 1 {
        return false;
    }
    let mastering = values
        .iter()
        .filter(|value| crate::recording_identity::is_release_version(value))
        .count();
    !(values.len() == 2 && mastering == 1)
}

fn prepared_unknown_annotation(
    evidence: &PreparedRecordingEvidence,
    requested: &str,
    label: Option<&str>,
    canonical: Option<&str>,
) -> bool {
    evidence
        .texts
        .iter()
        .filter(|observation| {
            matches!(
                observation.origin.field.as_str(),
                "title" | "sourceMetadata.track" | "sourceMetadata.catalogTitle"
            ) && (canonical.is_none() || observation.origin.field != "sourceMetadata.track")
        })
        .any(|observation| {
            let parsed = &observation.parsed;
            let base =
                caption_track_title(&parsed.base_title, requested, &parsed.base_title, label);
            !title_key(requested).is_empty()
                && title_key(&base) == title_key(requested)
                && (parsed.parsing_incomplete
                    || parsed.annotations.iter().any(|annotation| {
                        annotation.kind == AnnotationKind::Unresolved
                            && !label.is_some_and(|known| {
                                !label_key(known).is_empty()
                                    && label_key(known) == label_key(&annotation.value)
                            })
                    }))
        })
}

fn unresolved_candidate_gloss_matches_base_title(
    request: &PreparedRecordingEvidence,
    candidate: &PreparedRecordingEvidence,
    requested_title: &str,
    candidate_title: &str,
) -> bool {
    if request.texts.iter().any(|text| {
        text.parsed
            .annotations
            .iter()
            .any(|annotation| annotation.kind == AnnotationKind::Unresolved)
    }) || candidate
        .texts
        .iter()
        .any(|text| text.parsed.parsing_incomplete)
    {
        return false;
    }
    let requested_key = title_key(requested_title);
    // The ordinary comparison deliberately leaves unknown parenthetical text
    // attached to the title. Here we test the base title only after the exact
    // identifier/runtime gates and the single unresolved annotation check.
    let candidate_base = parse_recording_text(candidate_title).base_title;
    if requested_key.is_empty() || requested_key != title_key(&candidate_base) {
        return false;
    }
    let unknown = candidate
        .texts
        .iter()
        .filter(|text| {
            matches!(
                text.origin.field.as_str(),
                "title" | "sourceMetadata.track" | "sourceMetadata.catalogTitle"
            )
        })
        .flat_map(|text| {
            text.parsed
                .annotations
                .iter()
                .filter(|annotation| annotation.kind == AnnotationKind::Unresolved)
                .map(|annotation| crate::matching::normalize(&annotation.value))
        })
        .filter(|value| !value.is_empty())
        .collect::<std::collections::HashSet<_>>();
    unknown.len() == 1
}

fn split_credits(value: &str) -> Vec<String> {
    let mut text = value.to_owned();
    for marker in [" featuring ", " feat. ", " feat ", " ft. ", " ft ", " w/"] {
        let mut normalized = String::with_capacity(text.len());
        let mut cursor = 0;
        while cursor < text.len() {
            let next = text[cursor..].char_indices().find_map(|(offset, _)| {
                let start = cursor + offset;
                let end = start.saturating_add(marker.len());
                text.get(start..end)
                    .filter(|slice| slice.eq_ignore_ascii_case(marker))
                    .map(|_| start)
            });
            let Some(start) = next else {
                normalized.push_str(&text[cursor..]);
                break;
            };
            normalized.push_str(&text[cursor..start]);
            normalized.push(',');
            cursor = start + marker.len();
        }
        text = normalized;
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
fn valid_artist_credit_set_alias(
    track: &Track,
    alias: &crate::recording_identity::VerifiedArtistCreditSetVariant,
) -> bool {
    let expected_provider = match alias.provider.as_str() {
        "discogs" => "discogs",
        "musicbrainz" => "musicbrainz",
        _ => return false,
    };
    let expected_scope = match alias.provider.as_str() {
        "discogs" => "exactDiscogsReleaseArtistNameVariationsOnly",
        "musicbrainz" => "musicBrainzArtistProfileNameEquivalenceOnly",
        _ => return false,
    };
    if track.provider != "catalog"
        || track.provenance["catalogProvider"] != expected_provider
        || track.provenance["releaseId"].as_str() != Some(alias.release_id.as_str())
        || alias.evidence_scope != expected_scope
        || expected_provider == "musicbrainz"
            && (!valid_musicbrainz_id(&alias.release_id)
                || track.source_url
                    != format!("https://musicbrainz.org/release/{}", alias.release_id)
                || track.provenance["sourceUrl"].as_str() != Some(track.source_url.as_str()))
        || track.version.is_some()
        || track.playback_range.is_some()
        || track.provenance["manualFields"]["artist"] == true
        || track.provenance["manualFields"]["artistCredits"] == true
        || alias.requested_credits.is_empty()
        || alias.requested_credits.len() > 16
        || alias.requested_credits.len() != alias.candidate_credits.len()
        || alias.requested_credits.len() != alias.artist_ids.len()
        || alias.artist_ids.iter().any(|id| match expected_provider {
            "discogs" => {
                id.is_empty() || id.len() > 20 || !id.bytes().all(|byte| byte.is_ascii_digit())
            }
            "musicbrainz" => !valid_musicbrainz_id(id),
            _ => true,
        })
        || alias
            .requested_credits
            .iter()
            .chain(&alias.candidate_credits)
            .any(|name| name.trim().is_empty() || name.len() > 256)
    {
        return false;
    }
    let Some(credits) = track.provenance["artistCredits"].as_array() else {
        return false;
    };
    let expected = credits
        .iter()
        .filter(|credit| {
            credit["role"]
                .as_str()
                .is_some_and(crate::recording_identity::is_primary_performer_role)
        })
        .collect::<Vec<_>>();
    if expected.len() != alias.requested_credits.len() {
        return false;
    }
    for ((requested, artist_id), credit) in alias
        .requested_credits
        .iter()
        .zip(&alias.artist_ids)
        .zip(&expected)
    {
        if crate::matching::normalize(requested)
            != crate::matching::normalize(credit["name"].as_str().unwrap_or_default())
            || credit["artist"]["provider"] != expected_provider
            || credit["artist"]["id"].as_str() != Some(artist_id.as_str())
            || expected_provider == "musicbrainz"
                && credit["sourceUrl"].as_str() != Some(track.source_url.as_str())
        {
            return false;
        }
    }
    true
}
fn valid_musicbrainz_id(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(parts).all(|(length, part)| {
            part.len() == *length && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}
fn structured_primary_credits(track: &Track, fallback_artist: &str) -> Vec<String> {
    if track.provenance["manualFields"]["artist"] == true {
        // A user-edited artist supersedes the provider's older credit rows.
        // Keep the display name atomic unless separately edited credits are
        // present and demonstrably render the same complete name.
        if track.provenance["manualFields"]["artistCredits"] == true {
            if let Some(credits) = crate::recording_identity::declared_credit_names(track) {
                return credits;
            }
        }
        return (!fallback_artist.trim().is_empty())
            .then(|| vec![fallback_artist.trim().to_owned()])
            .unwrap_or_default();
    }
    let identity = RecordingIdentity::from_track(track);
    let credits = identity
        .credits
        .into_iter()
        .filter(|credit| crate::recording_identity::is_primary_performer_role(&credit.role))
        .map(|credit| credit.name)
        .filter(|name| !name.trim().is_empty())
        .collect::<Vec<_>>();
    // A structured row preserves each credited artist as an atomic name even
    // when it is not complete enough to close the request's performer set.
    if !credits.is_empty() {
        return credits;
    }
    split_credits(&fallback_artist.replace(" & ", ",").replace(" and ", ","))
}
fn requested_credit_parts_for_track(track: &Track, artist: &str, guests: &[String]) -> Vec<String> {
    structured_primary_credits(track, artist)
        .into_iter()
        .chain(guests.iter().cloned())
        .collect()
}
/// Expand a composite provider credit only when its components uniquely cover
/// the requested performer set. Exact single-act names are preserved first
/// (e.g. "Earth, Wind & Fire").
fn expanded_credits(names: &[String], expected_credits: &[String]) -> Vec<String> {
    let expected = expected_credits
        .iter()
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
    structured_primary: &[String],
    names: &[String],
    guests: &[String],
    level: MatchLevel,
    candidate_credits_complete: bool,
    request_credits_complete: bool,
    exact_recording_bound: bool,
    verified_name_alias: Option<(&str, &str)>,
    verified_credit_set_alias: Option<&crate::recording_identity::VerifiedArtistCreditSetVariant>,
) -> Comparison {
    if names.is_empty()
        || norm_artist(artist).is_empty()
        || names.iter().any(|name| norm_artist(name).is_empty())
    {
        return Comparison::Unknown;
    }
    let mut actual = names.iter().map(|s| norm_artist(s)).collect::<Vec<_>>();
    actual.sort();
    actual.dedup();
    let whole = norm_artist(artist);
    let primary = if !structured_primary.is_empty() {
        structured_primary
            .iter()
            .map(|name| norm_artist(name))
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
    } else if actual.contains(&whole) {
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
    let mut aliases = verified_name_alias
        .map(|(requested, candidate)| vec![(norm_artist(requested), norm_artist(candidate))])
        .unwrap_or_default();
    if let Some(verified) = verified_credit_set_alias {
        aliases.extend(
            verified
                .requested_credits
                .iter()
                .zip(&verified.candidate_credits)
                .map(|(requested, candidate)| (norm_artist(requested), norm_artist(candidate))),
        );
    }
    let expected = expected
        .into_iter()
        .map(|credit| {
            if actual.contains(&credit) {
                return credit;
            }
            aliases
                .iter()
                .find(|(requested, candidate)| *requested == credit && actual.contains(candidate))
                .map(|(_, candidate)| candidate.clone())
                .unwrap_or(credit)
        })
        .collect::<Vec<_>>();
    let extra_credit = actual.iter().any(|credit| !expected.contains(credit));
    let missing_requested_credit = expected.iter().any(|credit| !actual.contains(credit));
    // Candidate completeness proves the candidate's list is exhaustive. It
    // does not prove the request's artist field lists every performer. Only a
    // closed-world request can make an unrequested candidate credit a conflict.
    if request_credits_complete && extra_credit {
        return Comparison::Conflict;
    }
    if candidate_credits_complete && missing_requested_credit {
        return Comparison::Conflict;
    }
    let missing_guest = guests
        .iter()
        .map(|g| norm_artist(g))
        .any(|g| !actual.contains(&g));
    let extra_primary_credit = actual.iter().any(|a| !primary.contains(a));
    if missing_guest && extra_primary_credit {
        return Comparison::Conflict;
    }
    if missing_guest {
        return Comparison::Unknown;
    }
    if expected.len() == 1 && actual.len() > 1 && norm_artist(&names.join(", ")) == expected[0] {
        return Comparison::Agree;
    }
    if expected.iter().all(|e| actual.contains(e))
        && (!extra_credit
            || level != MatchLevel::Exact
            || exact_recording_bound
            || actual == expected
            // Exact mode still reports performer coverage when the request
            // has an open credit set. The caller will keep the result at
            // Inspect when candidate credits add unbound performers.
            || !request_credits_complete)
    {
        Comparison::Agree
    } else if level == MatchLevel::Exact
        && extra_credit
        && expected.iter().any(|credit| actual.contains(credit))
        && !request_credits_complete
        && !exact_recording_bound
    {
        Comparison::Unknown
    } else if level == MatchLevel::Exact && candidate_credits_complete {
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
    } else if words.contains(&"dub") {
        "dub".into()
    } else if matches!(
        n.as_str(),
        "a cappella"
            | "acapella"
            | "acappella"
            | "a capella"
            | "extended version"
            | "mono"
            | "stereo"
            | "sped up"
            | "slowed"
            | "slowed reverb"
            | "slowed and reverb"
    ) {
        n
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
        .or_else(|| remainder.strip_prefix(':'))
        .unwrap_or(remainder)
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
    let requested_version = track.version.as_deref().or(requested.version.as_deref())?;
    let requested_difficulty = difficulty_label(requested_version)?;

    // Catalog metadata frequently writes the same level/work without the
    // dash or colon present in the requested title. Treat that spelling as
    // equivalent only when both sides parse as numbered game levels and both
    // carry the same explicit, recognized difficulty label.
    let parsed_candidate = parse_title(candidate);
    if let (Some(actual_version), Some((actual_level, actual_work))) = (
        parsed_candidate.version.as_deref(),
        level_work(&parsed_candidate.title),
    ) {
        let actual_difficulty = difficulty_label(actual_version)?;
        if actual_level == requested_level
            && actual_difficulty == requested_difficulty
            && crate::matching::normalize_title_ignoring_intraword_apostrophes(&actual_work)
                == crate::matching::normalize_title_ignoring_intraword_apostrophes(&requested_work)
        {
            return Some(format!(
                "Level {requested_level} - {requested_work} ({requested_version})"
            ));
        }
    }
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
pub(crate) fn semantic_version(title: &str, version: &str) -> String {
    if level_work(&parse_title(title).title).is_some() {
        if let Some(difficulty) = difficulty_label(version) {
            return format!("{difficulty} difficulty");
        }
    }
    let normalized = crate::matching::normalize_version_for_title(title, version);
    let normalized = crate::recording_identity::canonical_version_key(&normalized);
    if matches!(
        normalized.as_str(),
        "original" | "original mix" | "original vocal" | "album version" | "main version"
    ) {
        String::new()
    } else {
        normalized
    }
}
fn identity_bound_catalog_title<'a>(candidate: &Candidate, meta: &'a Value) -> Option<&'a str> {
    let bound = if candidate.provider == "monochrome" {
        meta["provider"].as_str() == Some("monochrome")
            && meta["trackId"]
                .as_str()
                .is_some_and(|id| candidate.id == format!("monochrome:{id}"))
    } else {
        matches!(
            meta["provider"].as_str(),
            Some("spotify" | "apple" | "deezer")
        ) && meta["catalog"].as_bool() == Some(true)
    };
    bound
        .then(|| meta["catalogTitle"].as_str())
        .flatten()
        .filter(|title| !title.trim().is_empty())
}
fn catalog_metadata_bound(candidate: &Candidate, meta: &Value) -> bool {
    identity_bound_catalog_title(candidate, meta).is_some() && !field_artist_names(meta).is_empty()
}

fn topic_metadata_complete(track: &Track, candidate: &Candidate, meta: &Value) -> bool {
    if candidate.provider != "youtube"
        || meta["provider"].as_str() != Some("youtube")
        || meta["identityMetadataUncertain"] == true
    {
        return false;
    }
    let video_id = candidate.id.strip_prefix("youtube:").unwrap_or_default();
    if video_id.is_empty()
        || meta["resolvedProviderId"].as_str() != Some(video_id)
        || !matches!(
            meta["sourceMetadataOrigin"].as_str(),
            Some("ytDlpCandidateMetadata" | "ytDlpFullInfo")
        )
    {
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
    let required =
        requested_credit_parts_for_track(track, &track.artist, &guest_from_title(&track.title));
    if required.iter().any(|credit| {
        !artists
            .iter()
            .any(|name| norm_artist(name) == norm_artist(credit))
    }) {
        return false;
    }
    let expected_credit_keys = required
        .iter()
        .map(|credit| norm_artist(credit))
        .collect::<std::collections::HashSet<_>>();
    let actual_credit_keys = expanded_credits(&artists, &required)
        .iter()
        .map(|credit| norm_artist(credit))
        .collect::<std::collections::HashSet<_>>();
    if expected_credit_keys.is_empty() || expected_credit_keys != actual_credit_keys {
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
                    if known && title_key(without_label.trim()) == title_key(requested) {
                        return without_label.trim().to_owned();
                    }
                }
            }
        }
    }
    raw.to_owned()
}
fn caption_artist_names(
    caption: &str,
    expected_artist: &str,
    guests: &[String],
    expected_override: Option<&[String]>,
) -> Vec<String> {
    for separator in [" - ", " – ", " — ", " | "] {
        if let Some((prefix, suffix)) = caption.split_once(separator) {
            let expected_names = expected_override
                .map(|credits| credits.to_vec())
                .unwrap_or_else(|| requested_credit_parts(expected_artist, guests));
            // A bare feature phrase in the artist field can be an atomic act
            // name (for example, "Band Featuring Friends"). Preserve that
            // whole name when no separately verified/structured credit list
            // was supplied; otherwise a caption can split the name into a
            // false main-artist/guest pair.
            let expected_is_atomic_feature_name = expected_override.is_none()
                && guests.is_empty()
                && parse_title(expected_artist).featured_credit.is_some();
            let expected = if expected_is_atomic_feature_name {
                vec![norm_artist(expected_artist)]
            } else {
                expected_names
                    .into_iter()
                    .map(|name| norm_artist(&name))
                    .collect::<Vec<_>>()
            };
            // A structured expected credit that matches the whole caption
            // prefix is atomic, even if its proper name contains words such
            // as "Featuring" or "with".
            let prefix_is_expected_whole_credit = expected_override
                .map(|credits| {
                    credits.iter().any(|credit| {
                        !credit.trim().is_empty() && norm_artist(prefix) == norm_artist(credit)
                    })
                })
                .unwrap_or_else(|| {
                    guests.is_empty()
                        && !expected_artist.trim().is_empty()
                        && norm_artist(prefix) == norm_artist(expected_artist)
                });
            let mut featured = if prefix_is_expected_whole_credit {
                Vec::new()
            } else {
                caption_feature_credits(prefix)
            };
            featured.extend(caption_feature_credits(suffix));
            let suffix = base_title(suffix);
            if !featured.is_empty() {
                let parts = |credit: &str| {
                    // Preserve an exact whole-act spelling before interpreting
                    // standalone collaboration separators, including x and ×.
                    if expected.contains(&norm_artist(credit)) {
                        return vec![credit.trim().to_owned()];
                    }
                    split_credits(
                        &credit
                            .replace(" & ", ",")
                            .replace(" and ", ",")
                            .replace(" x ", ",")
                            .replace(" X ", ",")
                            .replace(" × ", ","),
                    )
                };
                let main_prefix = caption_base_without_features(prefix);
                let mut combined = parts(&main_prefix);
                combined.extend(featured.iter().flat_map(|guest| parts(guest)));
                let mut keys = std::collections::HashSet::new();
                combined.retain(|credit| keys.insert(norm_artist(credit)));
                let expected = expected
                    .into_iter()
                    .collect::<std::collections::HashSet<_>>();
                if !suffix.is_empty()
                    && !keys.contains("")
                    && keys.len() == expected.len()
                    && keys.iter().all(|credit| expected.contains(credit))
                {
                    return combined;
                }
                // An explicit extra/wrong guest cannot be discarded in favor
                // of a partial prefix that happens to name the main performer.
                return Vec::new();
            }
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

#[derive(Debug, Clone)]
struct RequestCorroboratedCaptionOrientation {
    title: String,
    artist: String,
}

fn uploader_matches_request_artist(uploader: &str, requested_artist: &str) -> bool {
    let requested = crate::recording_identity::search_fold(
        &crate::matching::normalize_single_artist_name(requested_artist),
    );
    if requested.is_empty() {
        return false;
    }
    let uploader = crate::matching::normalize_single_artist_name(uploader);
    if uploader.is_empty() {
        return false;
    }
    if crate::recording_identity::search_fold(&uploader) == requested {
        return true;
    }
    for suffix in [" official", " oficial", " topic"] {
        if let Some(base) = uploader.strip_suffix(suffix) {
            if !base.trim().is_empty()
                && crate::recording_identity::search_fold(base.trim()) == requested
            {
                return true;
            }
        }
    }
    false
}

/// Orient a title-first provider caption only when the right-hand credit is
/// the exact requested act, the source uploader independently corroborates
/// that act, and the left-hand text matches the requested title. This is a
/// request-scoped interpretation; it never rewrites provider metadata/cache.
fn request_corroborated_caption_orientation(
    track: &Track,
    candidate: &Candidate,
    meta: &Value,
) -> Option<RequestCorroboratedCaptionOrientation> {
    if !matches!(candidate.provider.as_str(), "youtube" | "soundcloud")
        || meta["provider"]
            .as_str()
            .is_some_and(|provider| provider != candidate.provider)
        || meta["identityMetadataUncertain"] == true
        || !matches!(
            meta["sourceMetadataOrigin"].as_str(),
            Some("ytDlpCandidateMetadata" | "ytDlpFullInfo")
        )
    {
        return None;
    }
    let provider_id = candidate
        .id
        .strip_prefix(&format!("{}:", candidate.provider))?;
    if provider_id.is_empty() || meta["resolvedProviderId"].as_str() != Some(provider_id) {
        return None;
    }
    // A structured provider credit list has its own orientation and remains
    // authoritative. This rule is for otherwise unstructured captions only.
    for key in ["artists", "artistNames"] {
        if let Some(value) = meta.get(key).filter(|value| !value.is_null()) {
            match value.as_array() {
                // Providers sometimes serialize no credits as an empty list.
                // Empty is absence; nonempty arrays remain authoritative and
                // malformed shapes remain a blocker to alternate parsing.
                Some(values) if values.is_empty() => {}
                Some(_) | None => return None,
            }
        }
    }
    let requested_artist = track.artist.trim();
    let requested_artist_key = crate::recording_identity::search_fold(requested_artist);
    if requested_artist_key.is_empty()
        || matches!(
            requested_artist_key.as_str(),
            "unknown" | "unknown artist" | "n a"
        )
    {
        return None;
    }
    let uploader_values = [
        candidate.uploader.as_deref(),
        meta["uploader"].as_str(),
        meta["channel"].as_str(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    if uploader_values.is_empty()
        || uploader_values
            .iter()
            .any(|uploader| !uploader_matches_request_artist(uploader, requested_artist))
    {
        return None;
    }

    let track_caption = meta["track"]
        .as_str()
        .filter(|value| !value.trim().is_empty());
    let catalog_caption = meta["catalogTitle"]
        .as_str()
        .filter(|value| !value.trim().is_empty());
    if track_caption
        .zip(catalog_caption)
        .is_some_and(|(left, right)| {
            crate::matching::normalize(left) != crate::matching::normalize(right)
        })
    {
        return None;
    }
    let caption = track_caption
        .or(catalog_caption)
        .unwrap_or(&candidate.title);
    if caption.trim().is_empty() || caption.len() > 2048 {
        return None;
    }

    let requested_title = parse_title(&track.title).title;
    let requested_title_key = title_key(&requested_title);
    if requested_title_key.is_empty() {
        return None;
    }
    let mut matches = Vec::new();
    for separator in [" - ", " – ", " — ", " | "] {
        for (index, _) in caption.match_indices(separator) {
            let left = caption[..index].trim();
            let right = caption[index + separator.len()..].trim();
            if left.is_empty() || right.is_empty() {
                continue;
            }
            let right_artist_matches =
                crate::recording_identity::search_fold(right) == requested_artist_key;
            let left_title_matches = crate::recording_identity::search_fold(&title_key(left))
                == crate::recording_identity::search_fold(&requested_title_key);
            if !right_artist_matches || !left_title_matches {
                continue;
            }
            // Refuse this alternate reading if the same boundary also supports
            // the ordinary artist-first orientation. That protects short or
            // identical artist/title captions from ambiguous reinterpretation.
            let left_artist_matches =
                crate::recording_identity::search_fold(left) == requested_artist_key;
            let right_title_matches = crate::recording_identity::search_fold(&title_key(right))
                == crate::recording_identity::search_fold(&requested_title_key);
            if left_artist_matches && right_title_matches {
                continue;
            }
            matches.push(RequestCorroboratedCaptionOrientation {
                title: left.to_owned(),
                artist: right.to_owned(),
            });
        }
    }
    matches.dedup_by(|a, b| {
        title_key(&a.title) == title_key(&b.title)
            && crate::matching::normalize_single_artist_name(&a.artist)
                == crate::matching::normalize_single_artist_name(&b.artist)
    });
    if matches.len() != 1 {
        return None;
    }
    let orientation = matches.remove(0);
    // A singular provider artist field can itself be the caption parser's
    // mistaken left-side value. Ignore only that value or the corroborated
    // right-side artist; any third artist remains a contradiction.
    if field_artist_names(meta).iter().any(|name| {
        crate::recording_identity::search_fold(name)
            != crate::recording_identity::search_fold(&orientation.title)
            && crate::recording_identity::search_fold(name) != requested_artist_key
    }) {
        return None;
    }
    Some(orientation)
}

fn bound_catalog_title(raw: &str, requested: &str, meta: &Value, label: Option<&str>) -> String {
    if title_key(raw) == title_key(requested) {
        return raw.to_owned();
    }
    // A canonical catalog field can itself contain a full caption, but only
    // its independently declared performers can authorize stripping a prefix.
    let credits = field_artist_names(meta).join(", ");
    if caption_artist_names(raw, &credits, &[], None).is_empty() {
        return raw.to_owned();
    }
    caption_track_title(raw, requested, raw, label)
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

#[derive(Debug, Clone)]
struct ComparisonFacts {
    dimensions: MatchDimensions,
    proof_strength: ProofStrength,
    artists_exact: Comparison,
    artists_flexible: Comparison,
    requested_guests: Vec<String>,
    requested_performer_count: usize,
    names: Vec<String>,
    actual_version: String,
    actual_version_components: Vec<String>,
    requested_version_components: Vec<String>,
    requested_semantic: String,
    requested_version: String,
    actual_kind: String,
    requested_kind: String,
    target_runtime: Option<f64>,
    candidate_runtime: Option<f64>,
    authoritative: bool,
    credits_complete: bool,
    verified_musicbrainz_credits_complete: bool,
    request_credits_complete: bool,
    lexical_credit_override: bool,
    exact_recording_bound: bool,
    topic_full: bool,
    contextual_game_title: bool,
    caption_orientation_requires_full_info: bool,
    linked: bool,
    explicit_alt: bool,
    full_album_content: bool,
    cover_evidence: bool,
    catalog_id_conflict: bool,
    preview_too_short: bool,
    complete_single_recording: bool,
    complete_fresh_extractor_recording: bool,
    restricted: bool,
    search_music_catalog: bool,
    unresolved_annotation: bool,
    unresolved_candidate_title_gloss: bool,
    empty_title: bool,
    empty_artist: bool,
    conflicting_request_versions: bool,
    conflicting_candidate_versions: bool,
    request_identifier_conflict: bool,
    parsing_incomplete: bool,
    unverified_credit_hint: bool,
    filename_observations_used: bool,
    accent_folded_performer_match: bool,
    missing_performers: Vec<String>,
    extra_performers: Vec<String>,
    scoped_artist_credit_set_alias: Option<ScopedArtistCreditSetAliasEvidence>,
}

#[derive(Debug, Clone)]
pub struct PreparedMatchInput {
    request: PreparedRecordingEvidence,
    candidate: PreparedRecordingEvidence,
    facts: ComparisonFacts,
}
impl PreparedMatchInput {
    pub fn request_evidence(&self) -> &PreparedRecordingEvidence {
        &self.request
    }
    pub fn candidate_evidence(&self) -> &PreparedRecordingEvidence {
        &self.candidate
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComparedEvidence {
    pub dimensions: MatchDimensions,
    pub proof_strength: ProofStrength,
    pub runtime_delta_seconds: Option<f64>,
    pub missing_performers: Vec<String>,
    pub extra_performers: Vec<String>,
    pub unresolved_annotations: Vec<AnnotationObservation>,
    pub identifier_conflict: bool,
    pub complete_performer_credits: bool,
    pub requested_performer_credits_complete: bool,
    #[serde(default)]
    pub accent_folded_performer_match: bool,
    pub scoped_artist_name_alias: Option<ScopedArtistNameAliasEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_artist_credit_set_alias: Option<ScopedArtistCreditSetAliasEvidence>,
    pub parsing_incomplete: bool,
    pub conflicting_version_observations: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_scoped_runtime_variant: Option<RecordingScopedRuntimeVariantEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_scoped_credit_variant: Option<RecordingScopedCreditVariantEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_scoped_isrc_membership: Option<RecordingScopedIsrcMembershipEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_scoped_title_alias: Option<RecordingScopedTitleAliasEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingScopedIsrcMembershipEvidence {
    pub recording_id: String,
    pub release_id: String,
    pub requested_isrc: Option<String>,
    pub selected_isrc: String,
    pub source_isrcs: Vec<String>,
    pub authority: RecordingScopedIsrcAuthority,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingScopedTitleAliasEvidence {
    pub recording_id: String,
    pub release_id: String,
    pub isrc: String,
    pub requested_title: String,
    pub recording_title: String,
    pub release_track_title: String,
    pub candidate_title: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecordingScopedIsrcAuthority {
    VerifiedMusicBrainzOccurrence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingScopedCreditVariantEvidence {
    pub recording_id: String,
    pub release_id: String,
    pub isrc: String,
    pub requested_artist_credit: String,
    pub release_artist_credit: String,
    pub release_artist_names: Vec<String>,
    pub release_artist_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingScopedRuntimeVariantEvidence {
    pub recording_id: String,
    pub release_id: String,
    pub isrc: String,
    pub recording_title: String,
    pub release_track_title: String,
    pub requested_artist_credit: String,
    pub release_artist_credit: String,
    pub requested_runtime_seconds: Option<f64>,
    pub release_runtime_seconds: f64,
    pub candidate_runtime_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScopedArtistNameAliasEvidence {
    pub requested_name: String,
    pub candidate_primary_name: String,
    pub musicbrainz_artist_id: String,
    /// This establishes name equivalence only; it is not a recording or
    /// complete-credit identity proof.
    pub evidence_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScopedArtistCreditSetAliasEvidence {
    pub provider: String,
    pub release_id: String,
    pub requested_credits: Vec<String>,
    pub candidate_credits: Vec<String>,
    pub artist_ids: Vec<String>,
    pub evidence_scope: String,
}

fn prepare_observations(
    track: &Track,
    candidate: &Candidate,
) -> (PreparedRecordingEvidence, PreparedRecordingEvidence) {
    let source_metadata = candidate.source_metadata.as_ref().unwrap_or(&Value::Null);
    let filename_observations = verified_soulseek_filename_observations(candidate, source_metadata);
    let empty_metadata = Value::Null;
    let meta = if filename_observations.is_some() {
        &empty_metadata
    } else {
        source_metadata
    };
    let caption_orientation = filename_observations
        .is_none()
        .then(|| request_corroborated_caption_orientation(track, candidate, meta))
        .flatten();
    let known_label = track.label.as_deref().or(meta["label"].as_str());
    let parse_text = |raw: &str| {
        let mut parsed = parse_recording_text(raw);
        for annotation in &mut parsed.annotations {
            if annotation.kind == AnnotationKind::Unresolved
                && known_label.is_some_and(|label| {
                    !label_key(label).is_empty() && label_key(label) == label_key(&annotation.value)
                })
            {
                annotation.kind = AnnotationKind::EstablishedLabel;
            }
        }
        parsed
    };
    let request_group = format!("request:{}:{}", track.provider, track.provider_id);
    let candidate_group = format!("candidate:{}:{}", candidate.provider, candidate.id);
    let origin = |field: &str, source_group: &str, authority| EvidenceOrigin {
        field: field.to_owned(),
        source_group: source_group.to_owned(),
        authority,
    };
    let mut request = PreparedRecordingEvidence::default();
    let mut actual = PreparedRecordingEvidence::default();
    for (field, text) in [
        ("title", Some(track.title.as_str())),
        ("version", track.version.as_deref()),
    ] {
        if let Some(text) = text {
            request.texts.push(TextObservation {
                origin: origin(field, &request_group, EvidenceAuthority::RequestMetadata),
                parsed: parse_text(text),
            });
        }
    }
    let identity = RecordingIdentity::from_track(track);
    let manual_isrc = track.provenance["manualFields"]["isrc"].as_bool() == Some(true);
    for (field, raw, active) in [
        ("isrc", track.isrc.as_deref(), true),
        (
            "provenance.remoteCatalog.isrc",
            track.provenance["remoteCatalog"]["isrc"].as_str(),
            !manual_isrc,
        ),
        (
            "provenance.recordingContext.isrc",
            track.provenance["recordingContext"]["isrc"].as_str(),
            !manual_isrc,
        ),
        (
            "provenance.recognitionIdentifiers.isrc",
            track.provenance["recognitionIdentifiers"]["isrc"].as_str(),
            !manual_isrc,
        ),
    ] {
        if let Some(value) = raw.and_then(normalize_isrc) {
            request.identifiers.push(IdentifierObservation {
                origin: origin(field, &request_group, EvidenceAuthority::RequestMetadata),
                kind: "isrc".into(),
                value,
                active,
            });
        }
    }
    if let Some(recording_id) = identity.recording_id {
        request.identifiers.push(IdentifierObservation {
            origin: origin(
                "provenance.recordingId",
                &request_group,
                EvidenceAuthority::RequestMetadata,
            ),
            kind: "recordingId".into(),
            value: recording_id,
            active: true,
        });
    }
    if identity.credits.is_empty() {
        let key = norm_artist(&track.artist);
        request.performers.push(PerformerObservation {
            origin: origin("artist", &request_group, EvidenceAuthority::RequestMetadata),
            name: track.artist.clone(),
            normalized_key: (!key.is_empty()).then_some(key),
            role: "performer".into(),
            complete: false,
        });
    } else {
        for credit in identity.credits {
            let key = norm_artist(&credit.name);
            request.performers.push(PerformerObservation {
                origin: origin(
                    "provenance.recordingContext.artistCredits",
                    &request_group,
                    EvidenceAuthority::RequestMetadata,
                ),
                name: credit.name,
                normalized_key: (!key.is_empty()).then_some(key),
                role: credit.role,
                complete: identity.credits_complete,
            });
        }
    }
    if let Some(seconds) = valid_duration(track.duration_seconds) {
        request.durations.push(DurationObservation {
            origin: origin(
                "durationSeconds",
                &request_group,
                EvidenceAuthority::RequestMetadata,
            ),
            seconds,
        });
    }

    if let Some(observations) = &filename_observations {
        actual.texts.push(TextObservation {
            origin: origin(
                "sourceMetadata.filenameObservations.normalizedTitle",
                &candidate_group,
                EvidenceAuthority::UploaderCaption,
            ),
            parsed: parse_text(&observations.normalized_title),
        });
        if let Some((title, credits)) =
            title_first_filename_caption(track, &observations.normalized_title)
        {
            actual.texts.push(TextObservation {
                origin: origin(
                    "sourceMetadata.filenameObservations.titleFirstTitle",
                    &candidate_group,
                    EvidenceAuthority::UploaderCaption,
                ),
                parsed: parse_text(&title),
            });
            for credit in credits {
                let key = norm_artist(&credit);
                actual.performers.push(PerformerObservation {
                    origin: origin(
                        "sourceMetadata.filenameObservations.titleFirstCredit",
                        &candidate_group,
                        EvidenceAuthority::UploaderCaption,
                    ),
                    name: credit,
                    normalized_key: (!key.is_empty()).then_some(key),
                    role: "performer".into(),
                    complete: false,
                });
            }
        }
        if let Some(artist) = &observations.folder_artist {
            let key = norm_artist(artist);
            actual.performers.push(PerformerObservation {
                origin: origin(
                    "sourceMetadata.filenameObservations.folderArtist",
                    &candidate_group,
                    EvidenceAuthority::UploaderCaption,
                ),
                name: artist.clone(),
                normalized_key: (!key.is_empty()).then_some(key),
                role: "performer".into(),
                complete: false,
            });
        }
    } else {
        actual.texts.push(TextObservation {
            origin: origin(
                "title",
                &candidate_group,
                EvidenceAuthority::UploaderCaption,
            ),
            parsed: parse_text(&candidate.title),
        });
    }
    if let Some(orientation) = &caption_orientation {
        actual.texts.push(TextObservation {
            origin: origin(
                "sourceMetadata.requestCorroboratedCaptionTitle",
                &candidate_group,
                EvidenceAuthority::UploaderCaption,
            ),
            parsed: parse_text(&orientation.title),
        });
        let key = norm_artist(&orientation.artist);
        actual.performers.push(PerformerObservation {
            origin: origin(
                "sourceMetadata.requestCorroboratedCaptionArtist",
                &candidate_group,
                EvidenceAuthority::UploaderCaption,
            ),
            name: orientation.artist.clone(),
            normalized_key: (!key.is_empty()).then_some(key),
            role: "performer".into(),
            complete: false,
        });
    }
    for field in ["track", "catalogTitle", "version", "mix"] {
        if let Some(text) = meta[field].as_str() {
            actual.texts.push(TextObservation {
                origin: origin(
                    &format!("sourceMetadata.{field}"),
                    &candidate_group,
                    if identity_bound_catalog_title(candidate, meta).is_some() {
                        EvidenceAuthority::ProviderCatalog
                    } else {
                        EvidenceAuthority::ProviderMetadata
                    },
                ),
                parsed: parse_text(text),
            });
        }
    }
    let names = if caption_orientation.is_some() {
        Vec::new()
    } else {
        field_artist_names(meta)
    };
    // Indexed artist arrays and counts are retrieval metadata, not a closed-
    // world assertion about every credited performer on the recording.
    // Monochrome IDs and artistCount are deliberately insufficient.
    let complete = explicit_performer_credit_completeness(candidate, meta);
    for name in names {
        let key = norm_artist(&name);
        actual.performers.push(PerformerObservation {
            origin: origin(
                "sourceMetadata.artists",
                &candidate_group,
                EvidenceAuthority::ProviderMetadata,
            ),
            name,
            normalized_key: (!key.is_empty()).then_some(key),
            role: "performer".into(),
            complete,
        });
    }
    for field in ["isrc", "recordingId", "workId"] {
        if let Some(raw) = meta[field].as_str() {
            let value = if field == "isrc" {
                normalize_isrc(raw)
            } else {
                (!raw.trim().is_empty()).then(|| raw.to_owned())
            };
            if let Some(value) = value {
                actual.identifiers.push(IdentifierObservation {
                    origin: origin(
                        &format!("sourceMetadata.{field}"),
                        &candidate_group,
                        EvidenceAuthority::ProviderMetadata,
                    ),
                    kind: field.into(),
                    value,
                    active: true,
                });
            }
        }
    }
    for (field, value) in [
        ("durationSeconds", candidate.duration_seconds),
        ("sourceMetadata.duration", meta["duration"].as_f64()),
    ] {
        if let Some(seconds) = valid_duration(value) {
            actual.durations.push(DurationObservation {
                origin: origin(field, &candidate_group, EvidenceAuthority::ProviderMetadata),
                seconds,
            });
        }
    }
    (request, actual)
}

pub fn prepare(track: &Track, candidate: &Candidate) -> PreparedMatchInput {
    prepare_with_primary_credit_override(track, candidate, None, false, None, None)
}

/// Prepare comparison facts from an exact-recording-bound release rendering
/// while retaining the untouched source observations as evidence.
fn prepare_with_primary_credit_override(
    track: &Track,
    candidate: &Candidate,
    primary_credit_override: Option<&[String]>,
    recording_scoped_title_alias: bool,
    verified_name_alias: Option<(&str, &str)>,
    verified_credit_set_alias: Option<&crate::recording_identity::VerifiedArtistCreditSetVariant>,
) -> PreparedMatchInput {
    prepare_with_primary_credit_override_and_mode(
        track,
        candidate,
        primary_credit_override,
        recording_scoped_title_alias,
        verified_name_alias,
        verified_credit_set_alias,
        false,
    )
}

fn prepare_with_lexical_credit_override(
    track: &Track,
    candidate: &Candidate,
    lexical_credit_override: &[String],
) -> PreparedMatchInput {
    prepare_with_primary_credit_override_and_mode(
        track,
        candidate,
        Some(lexical_credit_override),
        false,
        None,
        None,
        true,
    )
}

fn prepare_with_primary_credit_override_and_mode(
    track: &Track,
    candidate: &Candidate,
    primary_credit_override: Option<&[String]>,
    recording_scoped_title_alias: bool,
    verified_name_alias: Option<(&str, &str)>,
    verified_credit_set_alias: Option<&crate::recording_identity::VerifiedArtistCreditSetVariant>,
    lexical_credit_override: bool,
) -> PreparedMatchInput {
    let enabled = crate::diagnostic_metrics::is_enabled();
    let started = enabled.then(std::time::Instant::now);
    let cpu_started = enabled
        .then(crate::diagnostic_metrics::thread_cpu_time_ns)
        .flatten();
    let (request, candidate_evidence) = prepare_observations(track, candidate);
    let facts = build_comparison(
        track,
        candidate,
        &request,
        &candidate_evidence,
        primary_credit_override,
        recording_scoped_title_alias,
        verified_name_alias,
        verified_credit_set_alias,
        lexical_credit_override,
    );
    let prepared = PreparedMatchInput {
        request,
        candidate: candidate_evidence,
        facts,
    };
    if let Some(started) = started {
        crate::diagnostic_metrics::record_match_prepare_elapsed(started.elapsed());
    }
    if let (Some(start), Some(end)) = (
        cpu_started,
        enabled
            .then(crate::diagnostic_metrics::thread_cpu_time_ns)
            .flatten(),
    ) {
        crate::diagnostic_metrics::record_match_prepare_cpu_elapsed(end.saturating_sub(start));
    }
    prepared
}
/// Facts and proof are independent of user-selected permissiveness and stage.
pub fn compare_prepared(input: &PreparedMatchInput) -> ComparedEvidence {
    let facts = &input.facts;
    ComparedEvidence {
        dimensions: facts.dimensions.clone(),
        proof_strength: facts.proof_strength,
        runtime_delta_seconds: facts
            .target_runtime
            .zip(facts.candidate_runtime)
            .map(|(a, b)| b - a),
        missing_performers: facts.missing_performers.clone(),
        extra_performers: facts.extra_performers.clone(),
        unresolved_annotations: input
            .request
            .texts
            .iter()
            .chain(&input.candidate.texts)
            .flat_map(|text| {
                text.parsed
                    .annotations
                    .iter()
                    .filter(|a| a.kind == AnnotationKind::Unresolved)
                    .map(|annotation| AnnotationObservation {
                        origin: text.origin.clone(),
                        annotation: annotation.clone(),
                    })
            })
            .collect(),
        identifier_conflict: facts.request_identifier_conflict
            || facts.dimensions.isrc == Comparison::Conflict
            || facts.catalog_id_conflict,
        complete_performer_credits: facts.credits_complete && !facts.names.is_empty(),
        requested_performer_credits_complete: facts.request_credits_complete,
        accent_folded_performer_match: facts.accent_folded_performer_match,
        scoped_artist_name_alias: None,
        scoped_artist_credit_set_alias: facts.scoped_artist_credit_set_alias.clone(),
        parsing_incomplete: facts.parsing_incomplete,
        conflicting_version_observations: facts.conflicting_request_versions
            || facts.conflicting_candidate_versions,
        recording_scoped_runtime_variant: None,
        recording_scoped_credit_variant: None,
        recording_scoped_isrc_membership: None,
        recording_scoped_title_alias: None,
    }
}

fn build_comparison(
    track: &Track,
    candidate: &Candidate,
    request_evidence: &PreparedRecordingEvidence,
    candidate_evidence: &PreparedRecordingEvidence,
    primary_credit_override: Option<&[String]>,
    recording_scoped_title_alias: bool,
    verified_name_alias: Option<(&str, &str)>,
    verified_credit_set_alias: Option<&crate::recording_identity::VerifiedArtistCreditSetVariant>,
    lexical_credit_override: bool,
) -> ComparisonFacts {
    let request_identity = RecordingIdentity::from_track(track);
    let manual_artist_override_supersedes_imported_credits =
        track.provenance["manualFields"]["artist"] == true
            && track.provenance["manualFields"]["artistCredits"] != true;
    let request_credit_source_uncertain =
        request_identity.credits_uncertain && !manual_artist_override_supersedes_imported_credits;
    let verified_credit_set_alias =
        verified_credit_set_alias.filter(|alias| valid_artist_credit_set_alias(track, alias));
    let source_metadata = candidate.source_metadata.as_ref().unwrap_or(&Value::Null);
    let filename_observations = verified_soulseek_filename_observations(candidate, source_metadata);
    let empty_metadata = Value::Null;
    let meta = if filename_observations.is_some() {
        &empty_metadata
    } else {
        source_metadata
    };
    let caption_orientation = filename_observations
        .is_none()
        .then(|| request_corroborated_caption_orientation(track, candidate, meta))
        .flatten();
    let caption_title = filename_observations
        .as_ref()
        .map(|observations| observations.normalized_title.as_str())
        .unwrap_or(&candidate.title);
    let request_title = parse_title(&track.title);
    let canonical_catalog = identity_bound_catalog_title(candidate, meta);
    let raw_candidate = canonical_catalog
        .or_else(|| meta["track"].as_str())
        .or_else(|| meta["catalogTitle"].as_str())
        .or_else(|| {
            filename_observations
                .as_ref()
                .map(|value| value.normalized_title.as_str())
        })
        .unwrap_or(&candidate.title);
    let wanted_title = base_title(&request_title.title);
    let title_first_caption = filename_observations.as_ref().and_then(|observations| {
        title_first_filename_caption(track, &observations.normalized_title)
    });
    let known_label = track.label.as_deref().or(meta["label"].as_str());
    let contextual_candidate = caption_orientation
        .as_ref()
        .map(|orientation| orientation.title.as_str())
        .unwrap_or(raw_candidate);
    let contextual_title = contextual_game_level_title(track, contextual_candidate).or_else(|| {
        if canonical_catalog.is_none() {
            contextual_game_level_title(
                track,
                caption_orientation
                    .as_ref()
                    .map(|orientation| orientation.title.as_str())
                    .unwrap_or(caption_title),
            )
        } else {
            None
        }
    });
    let contextual_game_title = contextual_title.is_some();
    let candidate_title = contextual_title
        .or_else(|| {
            caption_orientation
                .as_ref()
                .map(|orientation| orientation.title.clone())
        })
        .or_else(|| title_first_caption.as_ref().map(|(title, _)| title.clone()))
        .unwrap_or_else(|| {
            if canonical_catalog.is_some() {
                bound_catalog_title(raw_candidate, &wanted_title, meta, known_label)
            } else {
                caption_track_title(raw_candidate, &wanted_title, caption_title, known_label)
            }
        });
    let requested_version = track
        .version
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(request_title.version.as_deref())
        .unwrap_or("");
    let observations = prepared_semantic_observations(
        candidate_evidence,
        &wanted_title,
        &candidate_title,
        known_label,
    );
    let request_observations =
        prepared_semantic_observations(request_evidence, &wanted_title, &track.title, known_label);
    let conflicting_candidate_versions = conflicting_semantic_observations(&observations);
    let conflicting_request_versions = conflicting_semantic_observations(&request_observations);
    let mut actual_version = if let Some(orientation) = &caption_orientation {
        meta["version"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                meta["mix"]
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
            })
            .map(str::to_owned)
            .or_else(|| caption_semantic_version(&orientation.title))
            .unwrap_or_default()
    } else {
        candidate_version(meta, &candidate_title, known_label)
    };
    if !conflicting_candidate_versions && !observations.is_empty() {
        actual_version = observations.join(" ");
    }
    if actual_version == "presentation video" {
        actual_version.clear();
    }
    let requested_semantic = if request_observations.is_empty() {
        semantic_version(&track.title, requested_version)
    } else {
        request_observations.join(" ")
    };
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
    let unresolved_annotation = !recording_scoped_title_alias
        && prepared_unknown_annotation(
            candidate_evidence,
            &wanted_title,
            known_label,
            canonical_catalog,
        );
    let unresolved_candidate_title_gloss = unresolved_annotation
        && unresolved_candidate_gloss_matches_base_title(
            request_evidence,
            candidate_evidence,
            &request_title.title,
            &candidate_title,
        );
    let empty_title =
        title_key(&request_title.title).is_empty() || title_key(&candidate_title).is_empty();
    let title = if recording_scoped_title_alias {
        Comparison::Agree
    } else if empty_title || unresolved_annotation {
        Comparison::Unknown
    } else if title_key(&request_title.title) == title_key(&candidate_title)
        || caption_orientation.is_some()
            && crate::recording_identity::search_fold(&title_key(&request_title.title))
                == crate::recording_identity::search_fold(&title_key(&candidate_title))
    {
        Comparison::Agree
    } else {
        Comparison::Conflict
    };
    let identity_guests = RecordingIdentity::from_track(track)
        .credits
        .into_iter()
        .filter(|credit| crate::recording_identity::is_guest_performer_role(&credit.role))
        .map(|credit| credit.name)
        .collect::<Vec<_>>();
    let mut requested_guests = request_title
        .featured_credit
        .clone()
        .into_iter()
        .chain(identity_guests)
        .chain(guest_from_title(&track.title))
        .collect::<Vec<_>>();
    requested_guests.sort();
    requested_guests.dedup();
    let mut request_primary_credits = primary_credit_override
        .filter(|credits| !credits.is_empty() && credits.len() <= 16)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| structured_primary_credits(track, &comparison_artist));
    let requested_performer_count = request_primary_credits
        .iter()
        .chain(requested_guests.iter())
        .map(|credit| norm_artist(credit))
        .filter(|credit| !credit.is_empty())
        .collect::<std::collections::HashSet<_>>()
        .len();
    // A named remixer may also appear as a catalog artist. When both sides
    // name the same remix, its version already identifies that credit; the
    // caption need only identify the remaining performers.
    if same_remix && request_primary_credits.len() > 1 {
        let remixer = requested_semantic.strip_suffix(" remix").unwrap_or("");
        let retained = request_primary_credits
            .iter()
            .filter(|credit| norm_artist(credit) != norm_artist(remixer))
            .cloned()
            .collect::<Vec<_>>();
        if !retained.is_empty() {
            request_primary_credits = retained;
        }
    }
    let request_credit_parts = request_primary_credits
        .iter()
        .cloned()
        .chain(requested_guests.iter().cloned())
        .collect::<Vec<_>>();
    let comparison_credit_parts = request_credit_parts
        .iter()
        .map(|credit| {
            verified_credit_set_alias
                .and_then(|alias| {
                    alias
                        .requested_credits
                        .iter()
                        .zip(&alias.candidate_credits)
                        .find(|(requested, _)| norm_artist(requested) == norm_artist(credit))
                        .map(|(_, candidate)| candidate.clone())
                })
                .unwrap_or_else(|| credit.clone())
        })
        .collect::<Vec<_>>();
    let artist_array_observation = artist_array_observation(meta);
    let mut names = if artist_array_observation.uncertain {
        Vec::new()
    } else if let Some(orientation) = &caption_orientation {
        expanded_credits(&[orientation.artist.clone()], &comparison_credit_parts)
    } else if filename_observations.is_some() {
        Vec::new()
    } else {
        field_artist_names(meta)
    };
    // Providers sometimes serialize absent credit arrays as JSON null or an
    // empty array. Both are no evidence and may fall back to an independent
    // exact caption prefix; malformed non-null structures remain blockers.
    let metadata_declares_artists = filename_observations.is_none()
        && (meta["identityMetadataUncertain"] == true || artist_array_observation.declared);
    let mut caption_names = if artist_array_observation.uncertain || caption_orientation.is_some() {
        Vec::new()
    } else {
        caption_artist_names(
            canonical_catalog.unwrap_or(caption_title),
            &comparison_artist,
            &requested_guests,
            Some(&comparison_credit_parts),
        )
    };
    // A fresh yt-dlp YouTube or SoundCloud caption can close credits only when its
    // structured artist prefix and feature annotation parse to exactly the
    // requested performer set. Public search display strings, stale catalog
    // rows, and captions with contradictory structured artist metadata remain
    // open-world evidence. Lexical-only aliases can resolve names for this
    // request but do not authorize either caption-derived completeness path.
    let fresh_caption_credits_complete = !lexical_credit_override
        && filename_observations.is_none()
        && meta["identityMetadataUncertain"] != true
        && canonical_catalog.is_none()
        && !caption_names.is_empty()
        && matches!(candidate.provider.as_str(), "youtube" | "soundcloud")
        && candidate
            .id
            .strip_prefix(&format!("{}:", candidate.provider))
            .is_some_and(|id| !id.is_empty() && meta["resolvedProviderId"].as_str() == Some(id))
        && matches!(
            meta["sourceMetadataOrigin"].as_str(),
            Some("ytDlpCandidateMetadata" | "ytDlpFullInfo")
        )
        && (!metadata_declares_artists || {
            let parsed_structured = expanded_credits(&names, &comparison_credit_parts)
                .iter()
                .map(|name| norm_artist(name))
                .collect::<std::collections::HashSet<_>>();
            !parsed_structured.is_empty()
                && parsed_structured.iter().all(|name| {
                    caption_names
                        .iter()
                        .any(|credit| norm_artist(credit) == *name)
                })
        });
    // A bound catalog row can also corroborate a full caption when both the
    // structured artist rows and the explicitly parsed caption name exactly
    // the same requested set. A flat array alone still cannot close credits.
    let bound_caption_credits_complete = !lexical_credit_override
        && canonical_catalog.is_some()
        && meta["identityMetadataUncertain"] != true
        && !names.is_empty()
        && !caption_names.is_empty()
        && {
            let mut structured = expanded_credits(&names, &comparison_credit_parts)
                .iter()
                .map(|name| norm_artist(name))
                .collect::<Vec<_>>();
            let mut caption = expanded_credits(&caption_names, &comparison_credit_parts)
                .iter()
                .map(|name| norm_artist(name))
                .collect::<Vec<_>>();
            structured.sort();
            structured.dedup();
            caption.sort();
            caption.dedup();
            structured == caption
        };
    if !artist_array_observation.uncertain {
        if let Some((_, credits)) = &title_first_caption {
            caption_names.extend(credits.iter().cloned());
        }
    }
    if let Some(folder_artist) = filename_observations
        .as_ref()
        .and_then(|observations| observations.folder_artist.as_ref())
    {
        caption_names.push(folder_artist.clone());
    }
    if names.is_empty() && !metadata_declares_artists {
        names = std::mem::take(&mut caption_names);
    } else if canonical_catalog.is_none() && !caption_names.is_empty() {
        // Caption corroborates a composite provider field only when it carries
        // the complete requested credit list, including any named guest.
        names.extend(caption_names);
    }
    names = expanded_credits(&names, &comparison_credit_parts);
    let mut seen_credit_keys = std::collections::HashSet::new();
    names.retain(|name| seen_credit_keys.insert(norm_artist(name)));
    let actual_guests = if lexical_credit_override {
        caption_features_for_requested_title(raw_candidate, &track.title, &comparison_credit_parts)
    } else {
        guest_from_title(raw_candidate)
    };
    let mut guests = features_match(&requested_guests, &actual_guests);
    if lexical_credit_override && requested_guests.is_empty() && !actual_guests.is_empty() {
        let known_lexical_credits = comparison_credit_parts
            .iter()
            .map(|credit| norm_artist(credit))
            .filter(|credit| !credit.is_empty())
            .collect::<std::collections::HashSet<_>>();
        let mut observed_lexical_guests =
            expanded_credits(&actual_guests, &comparison_credit_parts)
                .into_iter()
                .map(|credit| norm_artist(&credit))
                .filter(|credit| !credit.is_empty())
                .collect::<Vec<_>>();
        observed_lexical_guests.sort();
        observed_lexical_guests.dedup();
        guests = if observed_lexical_guests
            .iter()
            .all(|guest| known_lexical_credits.contains(guest))
        {
            Comparison::Agree
        } else {
            Comparison::Conflict
        };
    }
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
        let main = request_primary_credits
            .iter()
            .map(|name| norm_artist(name))
            .collect::<Vec<_>>();
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
    if requested_guests.is_empty() && !(lexical_credit_override && !actual_guests.is_empty()) {
        guests = Comparison::Unknown;
    }
    let exact_recording_bound = verified_recording_binding(track, candidate, meta);
    let verified_musicbrainz_credits_complete = filename_observations.is_none()
        && verified_musicbrainz_performer_credits_complete(track, candidate, meta);
    let structured_credits_complete = filename_observations.is_none()
        && (explicit_performer_credit_completeness(candidate, meta)
            || verified_musicbrainz_credits_complete);
    // Flat artist arrays and generic captions stay open-world. The narrowly
    // validated full-caption case above is an explicit structure witness.
    let credits_complete = structured_credits_complete
        || fresh_caption_credits_complete
        || bound_caption_credits_complete;
    let request_credits_complete = (request_identity.credits_complete
        && track.provenance["manualFields"]["artist"] != true)
        || track.provenance["performerCreditsComplete"] == true
            && track.provenance["manualFields"]["performerCreditsComplete"] == true
        || track.provenance["manualFields"]["artist"] == true && request_primary_credits.len() > 1;
    let artist_comparison = |level, flexible_guests| {
        let artist_guests = if flexible_guests && guests == Comparison::Unknown {
            &[][..]
        } else {
            &requested_guests[..]
        };
        if names.is_empty() && exact_recording_bound {
            Comparison::Agree
        } else {
            artist_match(
                &comparison_artist,
                &request_primary_credits,
                &names,
                artist_guests,
                level,
                credits_complete,
                request_credits_complete,
                exact_recording_bound,
                verified_name_alias,
                verified_credit_set_alias,
            )
        }
    };
    let mut artists = artist_comparison(MatchLevel::Balanced, false);
    let mut artists_flexible = artist_comparison(MatchLevel::Flexible, true);
    // Compare the performer evidence that is actually present even when it
    // is not exhaustive. Closed-world completeness controls whether extra or
    // missing credits can prove a conflict; it must not erase a known wrong
    // performer (or an exact coverage match) from the comparison itself.
    let mut artists_exact = artist_comparison(MatchLevel::Exact, false);
    let actual_kind = version_kind(&actual_version);
    let requested_kind = version_kind(&requested_semantic);
    let version = if conflicting_request_versions || conflicting_candidate_versions {
        Comparison::Conflict
    } else if requested_semantic.is_empty() && actual_semantic.is_empty() {
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
    let authoritative = catalog_metadata_bound(candidate, meta);
    let topic_full = topic_metadata_complete(track, candidate, meta);
    let preview_too_short = candidate_runtime.is_some_and(|d| d <= 30.0)
        && target_runtime.is_none_or(|d| d > 30.0)
        || meta["isPreview"].as_bool() == Some(true) && target_runtime.is_none_or(|d| d > 30.0);
    let target_isrc = request_identity.isrc.clone();
    let candidate_isrc = meta["isrc"].as_str().and_then(normalize_isrc);
    let isrc = if request_identity.identifier_conflict {
        Comparison::Conflict
    } else {
        match (target_isrc, candidate_isrc) {
            (Some(a), Some(b)) if a == b => Comparison::Agree,
            (Some(_), Some(_)) => Comparison::Conflict,
            _ => Comparison::Unknown,
        }
    };
    // Accent folding is normally search-only. Permit it as comparison-only
    // performer coverage when exact title, runtime, and ISRC independently
    // bind the candidate to this recording. This does not change canonical
    // names, aliases, or Exact-mode decisions, and it cannot override a
    // closed-world request's additional-credit conflict or edition mismatch.
    let accent_folded_performer_match = title == Comparison::Agree
        && runtime == Comparison::Agree
        && isrc == Comparison::Agree
        && version != Comparison::Conflict
        && !request_credits_complete
        && accent_folded_covers_requested_credits(&request_credit_parts, &names);
    // A title-first caption can establish the performer spelling when its
    // right-hand credit and independent uploader both match the request. Keep
    // the accent-fold allowance request-scoped and require title, runtime, and
    // edition agreement before it affects any matching level.
    let request_corroborated_caption_artist_match = caption_orientation.is_some()
        && title == Comparison::Agree
        && runtime == Comparison::Agree
        && version != Comparison::Conflict
        && !request_credits_complete
        && search_folded_covers_requested_credits(&request_credit_parts, &names);
    if accent_folded_performer_match {
        artists = Comparison::Agree;
        artists_flexible = Comparison::Agree;
    }
    if request_corroborated_caption_artist_match {
        artists = Comparison::Agree;
        artists_flexible = Comparison::Agree;
        artists_exact = Comparison::Agree;
    }
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
    // The matcher has no Settings/selected argument. Mirror the explicit
    // pinned-source markers here, then require the exact provider ID so an
    // automatic browser import cannot bypass this identity check.
    let source_locked_to_this_youtube_video = candidate.provider == "youtube"
        && track.provider == "youtube"
        && (track.provenance["explicitSourceLock"] == true
            || track.provenance["userSelectedSource"] == true
            || (track.provenance["playbackMode"].as_str() == Some("source")
                && !(track.provenance["browserImport"] == true
                    && track.provenance["captureProvider"].as_str() == Some("youtube")
                    && track.provenance["manualFields"]["playbackMode"] != true)))
        && candidate.id.strip_prefix("youtube:") == Some(track.provider_id.as_str());
    // Explicit yt-dlp metadata says this video is an album, not an individual
    // recording. Missing request duration must not turn that into a track
    // match. Preserve a long-form request and its exact manually locked video.
    let full_album_content = meta["isFullAlbum"] == true
        && candidate.provider == "youtube"
        && meta["provider"].as_str() == Some("youtube")
        && candidate
            .id
            .strip_prefix("youtube:")
            .is_some_and(|id| meta["resolvedProviderId"].as_str() == Some(id))
        && matches!(
            meta["sourceMetadataOrigin"].as_str(),
            Some("ytDlpCandidateMetadata" | "ytDlpFullInfo")
        )
        && !track.title.to_ascii_lowercase().contains("full album")
        && !track
            .version
            .as_deref()
            .is_some_and(|version| version.to_ascii_lowercase().contains("full album"))
        && !source_locked_to_this_youtube_video;
    let cover_evidence = actual_kind == "cover"
        && (linked || explicit_original_cover(&candidate.title, &track.artist, &names));
    let dimensions = MatchDimensions {
        title,
        artists,
        guests,
        version,
        runtime,
        isrc,
        work,
    };
    let catalog_id_conflict = candidate.provider == "monochrome"
        && meta["provider"].as_str() == Some("monochrome")
        && meta["trackId"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && candidate.id != format!("monochrome:{id}"));
    let proof_strength = if !request_identity.identifier_conflict
        && !catalog_id_conflict
        && (exact_recording_bound || isrc == Comparison::Agree)
    {
        ProofStrength::RecordingIdentifier
    } else {
        ProofStrength::MetadataOnly
    };
    let mut expected = request_credit_parts
        .iter()
        .map(|s| norm_artist(s))
        .collect::<Vec<_>>();
    let mut actual = names.iter().map(|s| norm_artist(s)).collect::<Vec<_>>();
    if request_corroborated_caption_artist_match {
        // Preserve provider spelling in `names` and evidence, but compare the
        // single uploader-and-caption-verified accent variant as the request
        // credit. This closes missing/extra bookkeeping only for the same
        // request-scoped path that already passed title, runtime, version,
        // source-ID and uploader checks above.
        for requested in &expected {
            let folded = crate::recording_identity::search_fold(requested);
            let matches = actual
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    crate::recording_identity::search_fold(candidate) == folded
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                actual[matches[0]] = requested.clone();
            }
        }
    }
    let mut credit_aliases = verified_name_alias
        .map(|(requested, candidate)| vec![(norm_artist(requested), norm_artist(candidate))])
        .unwrap_or_default();
    if let Some(alias) = verified_credit_set_alias {
        credit_aliases.extend(
            alias
                .requested_credits
                .iter()
                .zip(&alias.candidate_credits)
                .map(|(requested, candidate)| (norm_artist(requested), norm_artist(candidate))),
        );
    }
    for credit in &mut expected {
        if actual.contains(credit) {
            continue;
        }
        if let Some((_, candidate)) = credit_aliases
            .iter()
            .find(|(requested, candidate)| requested == credit && actual.contains(candidate))
        {
            *credit = candidate.clone();
        }
    }
    let missing_performers = expected
        .iter()
        .filter(|key| !key.is_empty() && !actual.contains(key))
        .cloned()
        .collect();
    let extra_performers = actual
        .iter()
        .filter(|key| !key.is_empty() && !expected.contains(key))
        .cloned()
        .collect();
    let empty_artist = norm_artist(&track.artist).is_empty()
        || names.iter().any(|name| norm_artist(name).is_empty());
    let unverified_credit_hint = unverified_by_credit_hint(raw_candidate).is_some()
        || unverified_by_credit_hint(&candidate.title).is_some();
    ComparisonFacts {
        dimensions,
        proof_strength,
        artists_exact,
        artists_flexible,
        requested_guests,
        requested_performer_count,
        names,
        actual_version_components: if observations.is_empty() && !actual_semantic.is_empty() {
            vec![actual_semantic]
        } else {
            observations
        },
        requested_version_components: if request_observations.is_empty()
            && !requested_semantic.is_empty()
        {
            vec![requested_semantic.clone()]
        } else {
            request_observations
        },
        actual_version,
        requested_semantic,
        requested_version: requested_version.to_owned(),
        actual_kind,
        requested_kind,
        target_runtime,
        candidate_runtime,
        authoritative,
        credits_complete,
        verified_musicbrainz_credits_complete,
        request_credits_complete,
        lexical_credit_override,
        exact_recording_bound,
        topic_full,
        contextual_game_title,
        caption_orientation_requires_full_info: caption_orientation.is_some()
            && meta["sourceMetadataOrigin"].as_str() != Some("ytDlpFullInfo"),
        linked,
        explicit_alt,
        full_album_content,
        cover_evidence,
        catalog_id_conflict,
        preview_too_short,
        complete_single_recording: authoritative
            && !candidate
                .chapters
                .as_ref()
                .is_some_and(|chapters| !chapters.is_empty())
            && meta["isPreview"].as_bool() != Some(true)
            && meta["isFullAlbum"].as_bool() != Some(true)
            && !["full album", "full concert", "full set", "medley"]
                .iter()
                .any(|phrase| norm(&candidate.title).contains(phrase)),
        complete_fresh_extractor_recording: candidate.provider == "youtube"
            && meta["provider"].as_str() == Some("youtube")
            && candidate.id.strip_prefix("youtube:").is_some_and(|id| {
                !id.is_empty() && meta["resolvedProviderId"].as_str() == Some(id)
            })
            && matches!(
                meta["sourceMetadataOrigin"].as_str(),
                Some("ytDlpCandidateMetadata" | "ytDlpFullInfo")
            )
            && !candidate
                .chapters
                .as_ref()
                .is_some_and(|chapters| !chapters.is_empty())
            && meta["isPreview"].as_bool() != Some(true)
            && meta["isFullAlbum"].as_bool() != Some(true)
            && !["full album", "full concert", "full set", "medley"]
                .iter()
                .any(|phrase| norm(&candidate.title).contains(phrase)),
        restricted: candidate.permission == "restricted",
        search_music_catalog: meta["playbackMusicCatalogCandidate"].as_bool() == Some(true),
        unresolved_annotation,
        unresolved_candidate_title_gloss,
        empty_title,
        empty_artist,
        conflicting_request_versions,
        conflicting_candidate_versions,
        request_identifier_conflict: request_identity.identifier_conflict,
        filename_observations_used: filename_observations.is_some(),
        accent_folded_performer_match,
        parsing_incomplete: request_evidence
            .texts
            .iter()
            .chain(&candidate_evidence.texts)
            .any(|text| text.parsed.parsing_incomplete)
            || meta["identityMetadataUncertain"] == true
            || request_credit_source_uncertain,
        unverified_credit_hint,
        missing_performers,
        extra_performers,
        scoped_artist_credit_set_alias: verified_credit_set_alias.map(|alias| {
            ScopedArtistCreditSetAliasEvidence {
                provider: alias.provider.clone(),
                release_id: alias.release_id.clone(),
                requested_credits: alias.requested_credits.clone(),
                candidate_credits: alias.candidate_credits.clone(),
                artist_ids: alias.artist_ids.clone(),
                evidence_scope: alias.evidence_scope.clone(),
            }
        }),
    }
}

/// Apply eligibility policy to prepared facts. This never reparses text or does I/O.
pub fn assess_prepared(
    input: &PreparedMatchInput,
    level: MatchLevel,
    stage: MatchStage,
) -> MatchAssessment {
    assess_prepared_with_options(input, level, stage, &FlexibleMatchingOptions::default())
}

pub fn assess_prepared_with_options(
    input: &PreparedMatchInput,
    level: MatchLevel,
    stage: MatchStage,
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    let enabled = crate::diagnostic_metrics::is_enabled();
    let started = enabled.then(std::time::Instant::now);
    let cpu_started = enabled
        .then(crate::diagnostic_metrics::thread_cpu_time_ns)
        .flatten();
    let assessment = assess_prepared_inner(input, level, stage, options);
    if let Some(started) = started {
        crate::diagnostic_metrics::record_match_assess_elapsed(started.elapsed());
    }
    if let (Some(start), Some(end)) = (
        cpu_started,
        enabled
            .then(crate::diagnostic_metrics::thread_cpu_time_ns)
            .flatten(),
    ) {
        crate::diagnostic_metrics::record_match_assess_cpu_elapsed(end.saturating_sub(start));
    }
    assessment
}

fn assess_prepared_inner(
    input: &PreparedMatchInput,
    level: MatchLevel,
    stage: MatchStage,
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    let safe_options = FlexibleMatchingOptions::default();
    let options = if options.version == 1 {
        options
    } else {
        &safe_options
    };
    let facts = &input.facts;
    let policy = level.policy();
    if facts.request_identifier_conflict {
        return MatchAssessment {
            relationship: MatchRelationship::Unresolved,
            action: MatchAction::Enrich,
            alternative_kind: None,
            reasons: vec![MatchReason::ConflictingRequestIdentifiers],
            dimensions: facts.dimensions.clone(),
            proof_strength: ProofStrength::MetadataOnly,
            evidence: Some(compare_prepared(input)),
        };
    }
    let MatchDimensions {
        title,
        guests,
        version,
        runtime,
        isrc,
        work,
        ..
    } = facts.dimensions;
    let artists = match level {
        MatchLevel::Exact => facts.artists_exact,
        MatchLevel::Balanced => facts.dimensions.artists,
        MatchLevel::Flexible => facts.artists_flexible,
    };
    let unresolved_gloss_identity = policy.allow_catalog_runtime_exception
        && stage == MatchStage::Resolved
        && facts.unresolved_candidate_title_gloss
        && isrc == Comparison::Agree
        && runtime == Comparison::Agree
        && artists == Comparison::Agree
        && guests != Comparison::Conflict
        && version != Comparison::Conflict;
    let title_identity_matches = title == Comparison::Agree || unresolved_gloss_identity;
    let ComparisonFacts {
        requested_guests,
        names,
        actual_version,
        requested_semantic,
        requested_version,
        actual_kind,
        requested_kind,
        target_runtime,
        candidate_runtime,
        authoritative,
        credits_complete,
        verified_musicbrainz_credits_complete,
        request_credits_complete,
        lexical_credit_override,
        exact_recording_bound,
        topic_full,
        contextual_game_title,
        linked,
        explicit_alt,
        cover_evidence,
        catalog_id_conflict,
        preview_too_short,
        complete_single_recording,
        complete_fresh_extractor_recording,
        unresolved_annotation,
        empty_title,
        empty_artist,
        conflicting_request_versions,
        conflicting_candidate_versions,
        ..
    } = facts.clone();
    let version_is_requested = !requested_semantic.is_empty();
    // Explicit length alternatives are bounded independently in each direction.
    // Missing/invalid target or candidate runtime cannot satisfy this exception.
    let configured_length_alternative =
        target_runtime
            .zip(candidate_runtime)
            .is_some_and(|(requested, candidate)| {
                if !requested.is_finite()
                    || !candidate.is_finite()
                    || requested <= 0.0
                    || candidate <= 0.0
                {
                    return false;
                }
                let delta = candidate - requested;
                if delta < 0.0 {
                    -delta <= f64::from(options.max_shorter_by_seconds)
                } else {
                    delta <= f64::from(options.max_longer_by_seconds)
                }
            });
    let flexible_runtime_close = policy.allow_missing_guest_evidence
        && target_runtime
            .zip(candidate_runtime)
            .is_some_and(|(a, b)| (a - b).abs() <= 15.0_f64.max((a * 0.15).min(45.0)))
        && (!options.allow_different_lengths || configured_length_alternative);
    let flexible_runtime_eligible = (runtime == Comparison::Agree || flexible_runtime_close)
        && (!options.allow_different_lengths || configured_length_alternative);
    let configured_length_conflict = level == MatchLevel::Flexible
        && options.allow_different_lengths
        && target_runtime.zip(candidate_runtime).is_some()
        && !configured_length_alternative;
    let timing_near = target_runtime
        .zip(candidate_runtime)
        .is_some_and(|(a, b)| (a - b).abs() <= 2.0_f64.max((a * 0.03).min(8.0)));
    let credit_timing_agree = title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && version != Comparison::Conflict;
    let balanced_runtime_variance = policy.allow_catalog_runtime_exception
        && runtime == Comparison::Conflict
        && timing_near
        && (authoritative && credits_complete || topic_full)
        && credit_timing_agree;
    let balanced_search_variance = policy.allow_catalog_runtime_exception
        && stage == MatchStage::Search
        && facts.search_music_catalog
        && runtime == Comparison::Conflict
        && timing_near
        && credit_timing_agree;
    let contextual_game_runtime_plausible = contextual_game_title
        && title == Comparison::Agree
        && target_runtime.zip(candidate_runtime).is_some_and(|(a, b)| {
            (a - b).abs()
                <= if policy.allow_missing_guest_evidence {
                    15.0_f64.max((a * 0.15).min(45.0))
                } else {
                    2.0_f64.max(a * 0.02)
                }
        });
    let category_allowed = |version: &str| {
        let category = crate::recording_identity::version_categories(version);
        category.known()
            && (!category.release || options.allow_other_releases)
            && (!category.performance || options.allow_other_performances)
            && (!category.mix || options.allow_other_mixes)
            && (!category.cover || options.allow_other_artists)
    };
    // Only changed dimensions need permission: a different remaster of the
    // same live performance must not require permission to change performance.
    let changed_versions = facts
        .requested_version_components
        .iter()
        .filter(|value| !facts.actual_version_components.contains(value))
        .chain(
            facts
                .actual_version_components
                .iter()
                .filter(|value| !facts.requested_version_components.contains(value)),
        )
        .collect::<Vec<_>>();
    let version_alternative_allowed = version != Comparison::Conflict
        || changed_versions.iter().all(|value| category_allowed(value));
    let artist_alternative_allowed = (artists != Comparison::Conflict
        && guests != Comparison::Conflict)
        || options.allow_other_artists;
    let unknown_version_substitute = level == MatchLevel::Flexible
        && stage == MatchStage::Resolved
        && version_is_requested
        && version == Comparison::Unknown
        && actual_version.is_empty()
        // Apply only the option categories named by the requested version.
        // A restored/remastered release alternative must not require unrelated
        // permissions for live performances and mixes.
        && category_allowed(&requested_semantic)
        && (complete_single_recording
            || complete_fresh_extractor_recording
            || exact_recording_bound)
        && (exact_recording_bound || credits_complete)
        && artists == Comparison::Agree
        && guests != Comparison::Conflict
        && title == Comparison::Agree
        && !unresolved_annotation
        && !facts.parsing_incomplete
        && isrc != Comparison::Conflict
        && flexible_runtime_eligible;
    // When every version category is explicitly enabled, a fresh, source-ID
    // bound single YouTube recording with exact known title/performer evidence
    // and runtime agreement or an explicitly allowed bounded duration may be
    // accepted as an unspecified-version alternative. This does not claim its
    // performer list is complete or unlock artist changes, missing runtime,
    // or weakly sourced candidates.
    let broad_unversioned_alternative = level == MatchLevel::Flexible
        && stage == MatchStage::Resolved
        && options.allow_other_releases
        && options.allow_other_performances
        && options.allow_other_mixes
        && version_is_requested
        && version == Comparison::Unknown
        && actual_version.is_empty()
        && facts.complete_fresh_extractor_recording
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && (runtime == Comparison::Agree
            || options.allow_different_lengths && configured_length_alternative)
        && target_runtime.is_some()
        && candidate_runtime.is_some()
        && facts.missing_performers.is_empty()
        && facts.extra_performers.is_empty()
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && !facts.request_identifier_conflict
        && !catalog_id_conflict
        && isrc != Comparison::Conflict
        && !conflicting_request_versions
        && !conflicting_candidate_versions
        && !facts.parsing_incomplete
        && !unresolved_annotation
        && (!options.allow_different_lengths || configured_length_alternative);
    let identified_alternative = unknown_version_substitute
        || broad_unversioned_alternative
        || version == Comparison::Conflict
        || artists == Comparison::Conflict
        || guests == Comparison::Conflict
        || (runtime == Comparison::Conflict
            && !flexible_runtime_close
            && options.allow_different_lengths
            && configured_length_alternative);
    let alternative = policy.allow_identified_alternatives
        && identified_alternative
        && version_alternative_allowed
        && artist_alternative_allowed
        && title == Comparison::Agree
        && (!(artists == Comparison::Conflict
            || guests == Comparison::Conflict
            || actual_kind == "cover")
            || linked
            || cover_evidence)
        && (artists == Comparison::Agree || options.allow_other_artists && credits_complete)
        && (linked
            || explicit_alt
            || authoritative
            || exact_recording_bound
            || credits_complete
            || broad_unversioned_alternative)
        && (flexible_runtime_eligible
            || options.allow_different_lengths
                && complete_single_recording
                && credits_complete
                && configured_length_alternative
                && candidate_runtime.is_some_and(|seconds| seconds > 30.0))
        && (options.allow_other_artists && credits_complete
            || requested_guests.is_empty()
            || guests == Comparison::Agree
            || guests == Comparison::Unknown);
    let mut reasons = Vec::new();
    if facts.unverified_credit_hint {
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
    if configured_length_conflict {
        reasons.push(MatchReason::DurationLimitExceeded);
    }
    // This describes unavailable policy evidence, not a new rejection rule.
    if level == MatchLevel::Flexible
        && options.allow_different_lengths
        && target_runtime.zip(candidate_runtime).is_none()
    {
        reasons.push(MatchReason::DurationLimitUnavailable);
    }
    if broad_unversioned_alternative {
        reasons.push(MatchReason::UnspecifiedVersionAllowed);
    }
    if isrc == Comparison::Conflict {
        reasons.push(MatchReason::IsrcConflict);
    }
    if facts.filename_observations_used {
        reasons.push(MatchReason::FilenameCaptionOnly);
    }
    if title == Comparison::Conflict {
        reasons.push(MatchReason::TitleConflict);
    }
    if artists == Comparison::Conflict {
        reasons.push(MatchReason::ArtistConflict);
    }
    if facts.restricted {
        reasons.push(MatchReason::PermissionRestricted);
    }
    if preview_too_short {
        reasons.push(MatchReason::PreviewTooShort);
    }
    if facts.full_album_content {
        reasons.push(MatchReason::FullAlbumContent);
    }
    if catalog_id_conflict {
        reasons.push(MatchReason::SourceIdentityConflict);
    }
    let hard_conflict = catalog_id_conflict
        || facts.full_album_content
        || facts.restricted
        || preview_too_short
        || configured_length_conflict
        || title == Comparison::Conflict
        || (artists == Comparison::Conflict && !alternative)
        || (guests == Comparison::Conflict && !alternative)
        || (runtime == Comparison::Conflict
            && !alternative
            && !flexible_runtime_close
            && !balanced_runtime_variance
            && !balanced_search_variance)
        || (isrc == Comparison::Conflict && !alternative)
        || (version == Comparison::Conflict
            && !alternative
            && !conflicting_request_versions
            && !conflicting_candidate_versions
            && !unresolved_annotation);
    let enough = title_identity_matches
        && artists == Comparison::Agree
        && (requested_guests.is_empty()
            || guests == Comparison::Agree
            || policy.allow_missing_guest_evidence && guests == Comparison::Unknown);
    let flexible_evidence = policy.allow_missing_guest_evidence
        && title_identity_matches
        && artists == Comparison::Agree
        && (requested_guests.is_empty()
            || guests == Comparison::Agree
            || guests == Comparison::Unknown)
        && guests != Comparison::Conflict
        && isrc != Comparison::Conflict
        && (version != Comparison::Conflict || alternative)
        && ((!options.allow_different_lengths
            && (runtime == Comparison::Agree
                || flexible_runtime_close
                || target_runtime.is_none()
                    && candidate_runtime.is_some_and(|duration| duration > 30.0)))
            || options.allow_different_lengths && configured_length_alternative);
    let full_credits = credits_complete && !names.is_empty();
    let balanced_catalog_ok =
        policy.allow_catalog_runtime_exception && authoritative && full_credits;
    let balanced_missing_runtime_ok = policy.allow_catalog_runtime_exception
        && target_runtime.is_none()
        && candidate_runtime.is_some_and(|duration| duration > 30.0)
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && credits_complete
        && version != Comparison::Conflict;
    let identity_exact = (!facts.filename_observations_used
        && (exact_recording_bound || isrc == Comparison::Agree || runtime == Comparison::Agree))
        && (!version_is_requested || version == Comparison::Agree);
    // A validated Soulseek path and filename are caption evidence, not a
    // recording identifier. Balanced/Flexible may still accept the usual
    // title, performer, runtime and requested-version/guest agreement as Probable.
    let filename_caption_match = facts.filename_observations_used
        && stage == MatchStage::Resolved
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && runtime == Comparison::Agree
        && (requested_guests.is_empty() || guests == Comparison::Agree)
        && (!version_is_requested || version == Comparison::Agree)
        && !alternative;
    let unbound_unrequested_credits = !facts.extra_performers.is_empty()
        && !request_credits_complete
        && !exact_recording_bound
        && isrc != Comparison::Agree;
    let balanced_open_credit_match = unbound_unrequested_credits
        && facts.credits_complete
        && title == Comparison::Agree
        && artists == Comparison::Agree
        && runtime == Comparison::Agree
        && guests != Comparison::Conflict
        && version != Comparison::Conflict
        && isrc != Comparison::Conflict
        && !facts.request_identifier_conflict
        && !facts.catalog_id_conflict;
    let mut relationship = if hard_conflict {
        MatchRelationship::Unresolved
    } else if alternative {
        MatchRelationship::Alternative
    } else if filename_caption_match && level != MatchLevel::Exact {
        MatchRelationship::Probable
    } else if enough
        && identity_exact
        && !unresolved_gloss_identity
        && !unbound_unrequested_credits
        && (requested_guests.is_empty() || guests == Comparison::Agree)
    {
        MatchRelationship::Exact
    } else if enough && identity_exact {
        MatchRelationship::Probable
    } else if balanced_open_credit_match {
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
    // In Exact mode, matching the known members of a multi-performer request
    // does not prove that either side supplied the whole credit set. Preserve
    // the artist dimension as Agree, but require candidate-side completeness
    // or a strong recording identifier before lexical-only evidence can call
    // that coverage Exact.
    let multi_performer_request = facts.requested_performer_count > 1;
    if level == MatchLevel::Exact
        && relationship == MatchRelationship::Exact
        && multi_performer_request
        && !credits_complete
        && !exact_recording_bound
        && isrc != Comparison::Agree
        && (!request_credits_complete || lexical_credit_override)
    {
        relationship = MatchRelationship::Unresolved;
    }
    if policy.require_structured_credits
        && (runtime == Comparison::Unknown && isrc != Comparison::Agree && !exact_recording_bound
            || version_is_requested && version == Comparison::Unknown)
    {
        relationship = MatchRelationship::Unresolved;
    }
    let mut action = if hard_conflict {
        MatchAction::Reject
    } else if contextual_game_title
        && stage == MatchStage::Search
        && !contextual_game_runtime_plausible
    {
        MatchAction::Reject
    } else if stage == MatchStage::Search {
        if title == Comparison::Agree && (artists != Comparison::Conflict || alternative) {
            MatchAction::Inspect
        } else {
            MatchAction::Reject
        }
    } else if requested_kind.is_empty() && !actual_kind.is_empty() && !alternative {
        MatchAction::Reject
    } else if version_is_requested
        && actual_version.is_empty()
        && !unknown_version_substitute
        && !broad_unversioned_alternative
    {
        MatchAction::Enrich
    } else if policy.require_structured_credits
        && version_is_requested
        && version == Comparison::Unknown
    {
        MatchAction::Inspect
    } else if policy.require_structured_credits
        && !requested_version.is_empty()
        && requested_semantic.is_empty()
    {
        MatchAction::Inspect
    } else if policy.require_structured_credits
        && version_is_requested
        && artists == Comparison::Unknown
    {
        MatchAction::Inspect
    } else if level == MatchLevel::Exact
        && !facts.extra_performers.is_empty()
        && !exact_recording_bound
        && relationship == MatchRelationship::Probable
    {
        MatchAction::Inspect
    } else {
        match relationship {
            MatchRelationship::Exact => MatchAction::Accept,
            MatchRelationship::Probable
                if authoritative && full_credits && policy.allow_catalog_runtime_exception =>
            {
                MatchAction::Accept
            }
            MatchRelationship::Probable if balanced_missing_runtime_ok => MatchAction::Accept,
            MatchRelationship::Probable if balanced_runtime_variance => MatchAction::Accept,
            MatchRelationship::Probable if balanced_open_credit_match => MatchAction::Accept,
            MatchRelationship::Probable if unresolved_gloss_identity => MatchAction::Accept,
            MatchRelationship::Probable if filename_caption_match && level != MatchLevel::Exact => {
                MatchAction::Accept
            }
            MatchRelationship::Probable
                if policy.allow_missing_guest_evidence
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
    if stage == MatchStage::Resolved
        && facts.caption_orientation_requires_full_info
        && action != MatchAction::Reject
    {
        action = MatchAction::Enrich;
    }
    let uncertainty = facts.parsing_incomplete
        || unresolved_annotation && !unresolved_gloss_identity
        || empty_title
        || empty_artist
        || conflicting_request_versions
        || conflicting_candidate_versions;
    if facts.parsing_incomplete {
        reasons.push(MatchReason::ParsingIncomplete);
    }
    if unresolved_annotation {
        reasons.push(MatchReason::UnresolvedAnnotation);
    }
    if empty_title || empty_artist {
        reasons.push(MatchReason::EmptyIdentityKey);
    }
    if conflicting_request_versions {
        reasons.push(MatchReason::ConflictingRequestVersions);
    }
    if conflicting_candidate_versions {
        reasons.push(MatchReason::ConflictingCandidateVersions);
    }
    if uncertainty && !hard_conflict {
        relationship = MatchRelationship::Unresolved;
        action = if stage == MatchStage::Search {
            MatchAction::Inspect
        } else {
            MatchAction::Enrich
        };
    }
    if relationship == MatchRelationship::Unresolved && reasons.is_empty() {
        reasons.push(MatchReason::InsufficientEvidence);
    }
    let unknown_release_substitute = unknown_version_substitute
        && (norm(&requested_semantic).contains("restored edition")
            || norm(&requested_semantic).contains("remaster"));
    let alternative_kind = if relationship != MatchRelationship::Alternative
        || action != MatchAction::Accept
        || broad_unversioned_alternative
        || unknown_version_substitute && !unknown_release_substitute
    {
        None
    } else if artists == Comparison::Conflict
        || guests == Comparison::Conflict
        || actual_kind == "cover"
    {
        Some(AlternativeKind::OtherArtist)
    } else {
        if changed_versions
            .iter()
            .any(|value| crate::recording_identity::version_categories(value).mix)
        {
            Some(AlternativeKind::Remix)
        } else if changed_versions
            .iter()
            .any(|value| crate::recording_identity::version_categories(value).performance)
        {
            Some(AlternativeKind::Performance)
        } else if changed_versions
            .iter()
            .any(|value| crate::recording_identity::version_categories(value).release)
        {
            Some(AlternativeKind::Release)
        } else {
            None
        }
    };
    MatchAssessment {
        relationship,
        action,
        alternative_kind,
        reasons,
        dimensions: facts.dimensions.clone(),
        proof_strength: facts.proof_strength,
        evidence: Some(compare_prepared(input)),
    }
}

pub fn assess(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare(track, candidate),
        level,
        stage,
        &FlexibleMatchingOptions::default(),
    )
}

pub fn assess_with_options(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    assess_prepared_with_options(&prepare(track, candidate), level, stage, options)
}

/// Order eligible alternatives before proof and runtime tie-breakers.
pub(crate) fn alternative_rank(
    assessment: &MatchAssessment,
    track: &Track,
    candidate: &Candidate,
) -> u8 {
    if assessment.relationship != MatchRelationship::Alternative {
        return 0;
    }
    if let Some(kind) = assessment.alternative_kind {
        return match kind {
            AlternativeKind::Release => 1,
            AlternativeKind::Performance => 2,
            AlternativeKind::Remix => 3,
            AlternativeKind::OtherArtist => 4,
        };
    }
    if assessment.dimensions.artists == Comparison::Conflict
        || assessment.dimensions.guests == Comparison::Conflict
    {
        return 4;
    }
    let metadata = candidate.source_metadata.as_ref().unwrap_or(&Value::Null);
    let title = metadata["track"]
        .as_str()
        .or(metadata["catalogTitle"].as_str())
        .unwrap_or(&candidate.title);
    let mut labels = parse_recording_text(title)
        .semantic_versions()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for field in ["version", "mix"] {
        if let Some(value) = metadata[field].as_str() {
            labels.push(value.to_owned());
        }
    }
    if let Some(version) = track.version.as_deref() {
        labels.push(version.to_owned());
    }
    labels.extend(
        parse_recording_text(&track.title)
            .semantic_versions()
            .map(str::to_owned),
    );
    if labels
        .iter()
        .any(|value| crate::recording_identity::version_categories(value).mix)
    {
        3
    } else if labels.iter().any(|value| {
        let c = crate::recording_identity::version_categories(value);
        c.performance || c.cover
    }) {
        2
    } else {
        1
    }
}

/// Assess against an exact-recording-bound release rendering. This changes
/// comparison facts only; evidence retains the original request metadata.
pub(crate) fn assess_with_primary_credit_override(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    primary_credit_override: &[String],
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(
            track,
            candidate,
            Some(primary_credit_override),
            false,
            None,
            None,
        ),
        level,
        stage,
        &FlexibleMatchingOptions::default(),
    )
}

pub(crate) fn assess_with_primary_credit_override_and_options(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    primary_credit_override: &[String],
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(
            track,
            candidate,
            Some(primary_credit_override),
            false,
            None,
            None,
        ),
        level,
        stage,
        options,
    )
}

/// Assess using a request-scoped lexical artist rendering. Its names may
/// resolve artist spelling and guest identity for matching, but they cannot
/// establish source completeness by themselves.
pub(crate) fn assess_with_lexical_credit_override_and_options(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    lexical_credit_names: &[String],
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_lexical_credit_override(track, candidate, lexical_credit_names),
        level,
        stage,
        options,
    )
}

/// Compare against an exact recording's validated release title and ordered
/// credits while leaving the source and candidate observations untouched.
pub(crate) fn assess_with_scoped_occurrence_title_and_credit(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    primary_credit_override: &[String],
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(
            track,
            candidate,
            Some(primary_credit_override),
            true,
            None,
            None,
        ),
        level,
        stage,
        &FlexibleMatchingOptions::default(),
    )
}

pub(crate) fn assess_with_scoped_occurrence_title_and_credit_and_options(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    primary_credit_override: &[String],
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(
            track,
            candidate,
            Some(primary_credit_override),
            true,
            None,
            None,
        ),
        level,
        stage,
        options,
    )
}

/// Compare a request performer against one candidate performer only when the
/// caller has verified that both exact names belong to the same unique artist
/// profile. The request's original structured credits remain in evidence.
pub(crate) fn assess_with_scoped_artist_name_alias(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    requested_name: &str,
    candidate_name: &str,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(
            track,
            candidate,
            None,
            false,
            Some((requested_name, candidate_name)),
            None,
        ),
        level,
        stage,
        &FlexibleMatchingOptions::default(),
    )
}

pub(crate) fn assess_with_scoped_artist_name_alias_and_options(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    requested_name: &str,
    candidate_name: &str,
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(
            track,
            candidate,
            None,
            false,
            Some((requested_name, candidate_name)),
            None,
        ),
        level,
        stage,
        options,
    )
}

/// Assess a candidate credit rendering only when a private Discogs context
/// has matched the full ordered request credit set to this exact caption.
pub(crate) fn assess_with_scoped_artist_credit_set_variant(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    variant: &crate::recording_identity::VerifiedArtistCreditSetVariant,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(track, candidate, None, false, None, Some(variant)),
        level,
        stage,
        &FlexibleMatchingOptions::default(),
    )
}

pub(crate) fn assess_with_scoped_artist_credit_set_variant_and_options(
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    variant: &crate::recording_identity::VerifiedArtistCreditSetVariant,
    options: &FlexibleMatchingOptions,
) -> MatchAssessment {
    assess_prepared_with_options(
        &prepare_with_primary_credit_override(track, candidate, None, false, None, Some(variant)),
        level,
        stage,
        options,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_release_lexical_artist_spelling_matches_title_only_candidate_without_identity_proof() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../fixtures/travelling-song-spotify-request.json"
        ))
        .unwrap();
        let request: Track = serde_json::from_value(fixture["tracks"][0].clone()).unwrap();
        let candidate_title = "Quantic, Alice Russell - Travelling Song - feat. The Combo Bárbaro";
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:x5JgogqFYOM".into(),
            url: "https://www.youtube.com/watch?v=x5JgogqFYOM".into(),
            title: candidate_title.into(),
            uploader: Some("TruThoughtsJukeBox".into()),
            duration_seconds: Some(284.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"x5JgogqFYOM",
                "sourceMetadataOrigin":"ytDlpCandidateMetadata",
                "duration":284.0,
                "artists":null,"artist":null,"album":null,"isrc":null
            })),
            ..Candidate::default()
        };
        let mut resolved_candidate = candidate.clone();
        // Model the production Search -> Resolved metadata-origin transition
        // without adding artist, album, ISRC, track, or catalogTitle facts.
        resolved_candidate.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
            json!("ytDlpFullInfo");
        let lexical_credits = vec![
            "Quantic".to_owned(),
            "Alice Russell".to_owned(),
            "The Combo Bárbaro".to_owned(),
        ];
        let options = FlexibleMatchingOptions::default();
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let search = assess_with_lexical_credit_override_and_options(
                &request,
                &candidate,
                level,
                MatchStage::Search,
                &lexical_credits,
                &options,
            );
            assert_ne!(search.action, MatchAction::Reject, "{level:?}: {search:?}");
            let resolved = assess_with_lexical_credit_override_and_options(
                &request,
                &resolved_candidate,
                level,
                MatchStage::Resolved,
                &lexical_credits,
                &options,
            );
            match level {
                MatchLevel::Flexible => assert_eq!(
                    resolved.action,
                    MatchAction::Accept,
                    "{level:?}: {resolved:?}"
                ),
                MatchLevel::Exact => assert_ne!(
                    resolved.action,
                    MatchAction::Accept,
                    "{level:?}: {resolved:?}"
                ),
                MatchLevel::Balanced => assert_ne!(
                    resolved.action,
                    MatchAction::Reject,
                    "{level:?}: {resolved:?}"
                ),
            }
            assert_ne!(resolved.dimensions.artists, Comparison::Conflict);
            assert_eq!(resolved.dimensions.title, Comparison::Agree);
            assert_eq!(resolved.dimensions.guests, Comparison::Agree);
            assert_eq!(resolved.dimensions.version, Comparison::Unknown);
            assert!(
                !resolved
                    .evidence
                    .as_ref()
                    .unwrap()
                    .complete_performer_credits,
                "lexical spelling evidence cannot close source credits: {resolved:?}"
            );
        }

        for wrong_guest_title in [
            "Quantic, Alice Russell - Travelling Song - feat. Another Artist",
            "Quantic, Alice Russell - Travelling Song - feat. The Combo Bárbaro & Another Artist",
        ] {
            let mut wrong_guest = resolved_candidate.clone();
            wrong_guest.title = wrong_guest_title.into();
            let rejected = assess_with_lexical_credit_override_and_options(
                &request,
                &wrong_guest,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &lexical_credits,
                &options,
            );
            assert_ne!(rejected.action, MatchAction::Accept, "{rejected:?}");
            assert_eq!(
                rejected.dimensions.guests,
                Comparison::Conflict,
                "{rejected:?}"
            );
        }

        let mut multiple_known_guests = resolved_candidate.clone();
        multiple_known_guests.title =
            "Quantic, Alice Russell - Travelling Song - feat. The Combo Bárbaro & Alice Russell"
                .into();
        let multiple_guest_result = assess_with_lexical_credit_override_and_options(
            &request,
            &multiple_known_guests,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &lexical_credits,
            &options,
        );
        assert_eq!(
            multiple_guest_result.action,
            MatchAction::Accept,
            "{multiple_guest_result:?}"
        );
        assert_eq!(multiple_guest_result.dimensions.guests, Comparison::Agree);

        let atomic_guest_request = Track {
            title: "Song".into(),
            artist: "Primary, Atomic Band & Friends".into(),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        let atomic_guest_candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:atomic-guest".into(),
            title: "Primary - Song feat. Atomic Band & Friends".into(),
            duration_seconds: Some(180.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"atomic-guest",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "duration":180.0,
                "artists":null,"artist":null,"album":null,"isrc":null
            })),
            ..Candidate::default()
        };
        let atomic_guest_result = assess_with_lexical_credit_override_and_options(
            &atomic_guest_request,
            &atomic_guest_candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &["Primary".into(), "Atomic Band & Friends".into()],
            &options,
        );
        assert_eq!(
            atomic_guest_result.action,
            MatchAction::Accept,
            "{atomic_guest_result:?}"
        );
        assert_eq!(atomic_guest_result.dimensions.guests, Comparison::Agree);

        let mut prefix_feature = atomic_guest_candidate.clone();
        prefix_feature.title = "Primary feat. Atomic Band & Friends - Song".into();
        let prefix_guest_result = assess_with_lexical_credit_override_and_options(
            &atomic_guest_request,
            &prefix_feature,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &["Primary".into(), "Atomic Band & Friends".into()],
            &options,
        );
        assert_eq!(
            prefix_guest_result.action,
            MatchAction::Accept,
            "{prefix_guest_result:?}"
        );
        assert_eq!(prefix_guest_result.dimensions.guests, Comparison::Agree);

        let mut wrong_prefix_feature = atomic_guest_candidate;
        wrong_prefix_feature.title = "Primary feat. Unknown Guest - Song".into();
        let wrong_prefix_result = assess_with_lexical_credit_override_and_options(
            &atomic_guest_request,
            &wrong_prefix_feature,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &["Primary".into(), "Atomic Band & Friends".into()],
            &options,
        );
        assert_ne!(
            wrong_prefix_result.action,
            MatchAction::Accept,
            "{wrong_prefix_result:?}"
        );
        assert_eq!(wrong_prefix_result.dimensions.guests, Comparison::Conflict);

        let mut wrong_artist = resolved_candidate.clone();
        wrong_artist.title = "Different Artist - Travelling Song".into();
        let rejected = assess_with_lexical_credit_override_and_options(
            &request,
            &wrong_artist,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &lexical_credits,
            &options,
        );
        assert_ne!(rejected.action, MatchAction::Accept, "{rejected:?}");

        let mut wrong_version = resolved_candidate.clone();
        wrong_version.title = format!("{candidate_title} (Remix)");
        let rejected = assess_with_lexical_credit_override_and_options(
            &request,
            &wrong_version,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &lexical_credits,
            &options,
        );
        assert_ne!(rejected.action, MatchAction::Accept, "{rejected:?}");

        let mut request_with_isrc = request;
        request_with_isrc.isrc = Some("USABC2600001".into());
        let mut wrong_isrc = resolved_candidate;
        wrong_isrc.source_metadata.as_mut().unwrap()["isrc"] = json!("GBXYZ9876543");
        let rejected = assess_with_lexical_credit_override_and_options(
            &request_with_isrc,
            &wrong_isrc,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &lexical_credits,
            &options,
        );
        assert_ne!(rejected.action, MatchAction::Accept, "{rejected:?}");
        assert_eq!(rejected.dimensions.isrc, Comparison::Conflict);
    }

    #[test]
    fn caption_whole_band_name_is_atomic_before_trailing_feature_parsing() {
        let track = Track {
            title: "Song".into(),
            artist: "Band Featuring Friends".into(),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:atomic-band".into(),
            title: "Band Featuring Friends - Song".into(),
            duration_seconds: Some(180.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"atomic-band",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "duration":180.0,
                "artists":null,"artist":null,"album":null,"isrc":null
            })),
            ..Candidate::default()
        };
        let assessment = assess(
            &track,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            assessment.dimensions.artists,
            Comparison::Agree,
            "{assessment:?}"
        );
        assert_eq!(
            caption_artist_names(
                "Band Featuring Friends - Song",
                "Band Featuring Friends",
                &[],
                None
            ),
            vec!["Band Featuring Friends".to_owned()]
        );
        assert!(caption_artist_names(
            "Band - Song feat. Friends",
            "Band Featuring Friends",
            &[],
            None,
        )
        .is_empty());
        assert!(caption_artist_names(
            "Band - Song feat. Wrong Guest",
            "Band",
            &["Requested Guest".into()],
            None,
        )
        .is_empty());

        let structured = vec!["Band".to_owned(), "Friends".to_owned()];
        let grouped_caption = caption_artist_names(
            "Band & Friends - Song",
            "Band & Friends",
            &[],
            Some(&structured),
        );
        assert_eq!(expanded_credits(&grouped_caption, &structured), structured);
    }
    #[test]
    fn uploader_corroborated_caption_orientation_is_request_scoped_and_keeps_identity_guards() {
        macro_rules! assert_assessment_action {
            ($assessment:expr, $expected:expr) => {{
                let assessment = $assessment;
                assert_eq!(assessment.action, $expected, "{assessment:?}");
            }};
        }
        assert!(
            uploader_matches_request_artist("Earth, Wind & Fire Official", "Earth, Wind & Fire"),
            "ampersand punctuation should normalize consistently"
        );
        assert!(
            uploader_matches_request_artist("Earth, Wind and Fire Official", "Earth, Wind & Fire"),
            "and/ampersand spelling should identify the same atomic act"
        );
        assert!(
            !uploader_matches_request_artist("Different Artist Official", "Earth, Wind & Fire"),
            "unrelated uploader must not corroborate the request"
        );
        let request = Track {
            provider: "spotify".into(),
            provider_id: "6wu6sIm3bL3KKRVXfNoaAl".into(),
            title: "Ámame".into(),
            artist: "Maryto y su Salsón".into(),
            duration_seconds: Some(237.257),
            ..Track::default()
        };
        let caption = "Amame - Maryto y su Salson";
        let official = "Maryto y su Salson Oficial";
        let reversed = Candidate {
            provider: "youtube".into(),
            id: "youtube:StdOHMvDlas".into(),
            url: "https://www.youtube.com/watch?v=StdOHMvDlas".into(),
            title: caption.into(),
            uploader: Some(official.into()),
            duration_seconds: Some(240.0),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpCandidateMetadata",
                "resolvedProviderId":"StdOHMvDlas",
                "track":caption,
                "catalogTitle":caption,
                "duration":240.0
            })),
            ..Candidate::default()
        };
        let raw_candidate = reversed.clone();
        let search_assessment = assess(&request, &reversed, MatchLevel::Exact, MatchStage::Search);
        assert_eq!(
            search_assessment.action,
            MatchAction::Inspect,
            "{search_assessment:?}"
        );
        assert_eq!(
            search_assessment.dimensions.title,
            Comparison::Agree,
            "{search_assessment:?}"
        );
        assert_eq!(
            search_assessment.dimensions.artists,
            Comparison::Agree,
            "{search_assessment:?}"
        );
        // The user-visible search row is the same occurrence after its
        // caption parser emits the reversed title/artist fields.
        let mut parsed_search_row = reversed.clone();
        parsed_search_row.title = "Maryto y su Salson".into();
        parsed_search_row.source_metadata.as_mut().unwrap()["artist"] = json!("Amame");
        assert_assessment_action!(
            assess(
                &request,
                &parsed_search_row,
                MatchLevel::Exact,
                MatchStage::Search
            ),
            MatchAction::Inspect
        );
        assert_assessment_action!(
            assess(
                &request,
                &parsed_search_row,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Enrich
        );
        let assessment = assess(&request, &reversed, MatchLevel::Exact, MatchStage::Resolved);
        assert_eq!(assessment.action, MatchAction::Enrich, "{assessment:?}");
        assert_eq!(
            assessment.dimensions.title,
            Comparison::Agree,
            "{assessment:?}"
        );
        assert_eq!(
            assessment.dimensions.artists,
            Comparison::Agree,
            "{assessment:?}"
        );
        assert_eq!(
            assessment.dimensions.runtime,
            Comparison::Agree,
            "{assessment:?}"
        );
        assert_eq!(
            assessment.dimensions.version,
            Comparison::Unknown,
            "{assessment:?}"
        );
        assert_eq!(
            reversed.source_metadata, raw_candidate.source_metadata,
            "raw candidate metadata must remain request-independent: left={:?}, right={:?}",
            reversed.source_metadata, raw_candidate.source_metadata
        );
        let mut resolved = reversed.clone();
        resolved.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
        let assessment = assess(&request, &resolved, MatchLevel::Exact, MatchStage::Resolved);
        assert_eq!(assessment.action, MatchAction::Accept, "{assessment:?}");

        // This reproduces the live candidate metadata, which has an empty
        // credit list and no singular parsed artist field. Empty provider
        // arrays are absence and still permit the corroborated title caption.
        let mut empty_credits = reversed.clone();
        empty_credits.source_metadata.as_mut().unwrap()["artists"] = json!([]);
        let empty_credit_search = assess(
            &request,
            &empty_credits,
            MatchLevel::Exact,
            MatchStage::Search,
        );
        assert_eq!(
            empty_credit_search.action,
            MatchAction::Inspect,
            "{empty_credit_search:?}"
        );
        assert_eq!(
            empty_credit_search.dimensions.artists,
            Comparison::Agree,
            "{empty_credit_search:?}"
        );
        let mut empty_credits_resolved = empty_credits.clone();
        empty_credits_resolved.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
            json!("ytDlpFullInfo");
        assert_assessment_action!(
            assess(
                &request,
                &empty_credits_resolved,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Accept
        );

        let mut parsed_artist = reversed.clone();
        parsed_artist.source_metadata.as_mut().unwrap()["artist"] = json!("Amame");
        parsed_artist.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
            json!("ytDlpFullInfo");
        assert_assessment_action!(
            assess(
                &request,
                &parsed_artist,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Accept
        );

        // Malformed arrays and contradictory structured artist data stay
        // authoritative blockers, even if the caption and uploader agree.
        let mut malformed_artists = reversed.clone();
        malformed_artists.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
            json!("ytDlpFullInfo");
        malformed_artists.source_metadata.as_mut().unwrap()["artists"] = json!("Amame");
        let malformed_assessment = assess(
            &request,
            &malformed_artists,
            MatchLevel::Exact,
            MatchStage::Resolved,
        );
        assert_ne!(
            malformed_assessment.action,
            MatchAction::Accept,
            "{malformed_assessment:?}"
        );
        let mut contradictory_artists = reversed.clone();
        contradictory_artists.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
            json!("ytDlpFullInfo");
        contradictory_artists.source_metadata.as_mut().unwrap()["artists"] =
            json!(["Different Artist"]);
        assert_assessment_action!(
            assess(
                &request,
                &contradictory_artists,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Reject
        );

        // The same title in normal artist-first orientation still matches from
        // independent provider artist metadata without invoking the alternate.
        let normal = Candidate {
            provider: "youtube".into(),
            id: "youtube:NormalCap01".into(),
            url: "https://www.youtube.com/watch?v=NormalCap01".into(),
            title: "Ámame".into(),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "resolvedProviderId":"NormalCap01",
                "track":"Maryto y su Salson - Ámame",
                "catalogTitle":"Ámame",
                "artistNames":["Maryto y su Salsón"],
                "duration":240.0
            })),
            duration_seconds: Some(240.0),
            ..Candidate::default()
        };
        assert_assessment_action!(
            assess(
                &request,
                &normal,
                MatchLevel::Balanced,
                MatchStage::Resolved
            ),
            MatchAction::Accept
        );

        // The right-hand caption and uploader must both corroborate the
        // requested artist. A different uploader does not orient the caption.
        let mut wrong_uploader = reversed.clone();
        wrong_uploader.uploader = Some("Different Artist Official".into());
        wrong_uploader.source_metadata.as_mut().unwrap()["uploader"] =
            json!("Different Artist Official");
        wrong_uploader.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
            json!("ytDlpFullInfo");
        assert_assessment_action!(
            assess(
                &request,
                &wrong_uploader,
                MatchLevel::Balanced,
                MatchStage::Resolved
            ),
            MatchAction::Reject
        );
        assert_assessment_action!(
            assess(
                &request,
                &wrong_uploader,
                MatchLevel::Balanced,
                MatchStage::Search
            ),
            MatchAction::Reject
        );
        let wrong_artist_request = Track {
            artist: "Different Artist".into(),
            ..request.clone()
        };
        assert_assessment_action!(
            assess(
                &wrong_artist_request,
                &resolved,
                MatchLevel::Balanced,
                MatchStage::Resolved
            ),
            MatchAction::Reject
        );

        let wrong_version_request = Track {
            version: Some("Radio Edit".into()),
            ..request.clone()
        };
        let live_caption = "Amame (Live) - Maryto y su Salson";
        let wrong_version = Candidate {
            provider: "youtube".into(),
            id: "youtube:LiveVers001".into(),
            url: "https://www.youtube.com/watch?v=LiveVers001".into(),
            title: "Maryto y su Salson (Live)".into(),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "resolvedProviderId":"LiveVers001",
                "track":live_caption,
                "catalogTitle":live_caption,
                "duration":240.0,
                "uploader":official
            })),
            uploader: Some(official.into()),
            duration_seconds: Some(240.0),
            ..Candidate::default()
        };
        let wrong_version_assessment = assess(
            &wrong_version_request,
            &wrong_version,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(
            wrong_version_assessment.dimensions.version,
            Comparison::Conflict,
            "{wrong_version_assessment:?}"
        );
        assert_eq!(
            wrong_version_assessment.action,
            MatchAction::Reject,
            "{wrong_version_assessment:?}"
        );
        assert_assessment_action!(
            assess(
                &wrong_version_request,
                &wrong_version,
                MatchLevel::Balanced,
                MatchStage::Search
            ),
            MatchAction::Reject
        );

        // Whole band names remain atomic, and ordinary internal dashes remain
        // part of the requested title when the reversed caption is corroborated.
        let band_request = Track {
            title: "September - At Dawn".into(),
            artist: "Earth, Wind & Fire".into(),
            duration_seconds: Some(210.0),
            provenance: json!({
                "artistCredits":[{
                    "name":"Earth, Wind & Fire",
                    "role":"artist credit"
                }]
            }),
            ..Track::default()
        };
        let band_caption = "September - At Dawn - Earth, Wind & Fire";
        let band_search = Candidate {
            provider: "youtube".into(),
            id: "youtube:BandDash001".into(),
            url: "https://www.youtube.com/watch?v=BandDash001".into(),
            title: band_caption.into(),
            uploader: Some("Earth, Wind & Fire Official".into()),
            duration_seconds: Some(210.0),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpCandidateMetadata",
                "resolvedProviderId":"BandDash001",
                "track":band_caption,
                "catalogTitle":band_caption,
                "duration":210.0,
                "uploader":"Earth, Wind & Fire Official"
            })),
            ..Candidate::default()
        };
        let band_search_assessment = assess(
            &band_request,
            &band_search,
            MatchLevel::Exact,
            MatchStage::Search,
        );
        assert_eq!(
            band_search_assessment.action,
            MatchAction::Inspect,
            "{band_search_assessment:?}"
        );
        let band_provisional_assessment = assess(
            &band_request,
            &band_search,
            MatchLevel::Exact,
            MatchStage::Resolved,
        );
        assert_eq!(
            band_provisional_assessment.action,
            MatchAction::Enrich,
            "{band_provisional_assessment:?}"
        );
        let mut band = band_search.clone();
        band.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
        let band_assessment = assess(
            &band_request,
            &band,
            MatchLevel::Exact,
            MatchStage::Resolved,
        );
        assert_eq!(
            band_assessment.dimensions.title,
            Comparison::Agree,
            "{band_assessment:?}"
        );
        assert_eq!(
            band_assessment.dimensions.artists,
            Comparison::Agree,
            "{band_assessment:?}"
        );
        assert_eq!(
            band_assessment.action,
            MatchAction::Accept,
            "{band_assessment:?}"
        );

        // The result-row parser can expose the right-hand band as its title
        // and the left-hand song as its artist. Fresh, source-bound caption
        // fields still recover the intended orientation without splitting
        // the structured atomic band credit.
        let mut parsed_band_row = band.clone();
        parsed_band_row.title = "Earth, Wind & Fire".into();
        parsed_band_row.source_metadata.as_mut().unwrap()["artist"] = json!("September - At Dawn");
        assert_assessment_action!(
            assess(
                &band_request,
                &parsed_band_row,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Accept
        );

        let mut wrong_band_uploader = parsed_band_row.clone();
        wrong_band_uploader.uploader = Some("Different Artist Official".into());
        wrong_band_uploader.source_metadata.as_mut().unwrap()["uploader"] =
            json!("Different Artist Official");
        assert_assessment_action!(
            assess(
                &band_request,
                &wrong_band_uploader,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Reject
        );

        let mut wrong_band_recording = band.clone();
        let other_caption = "Another Song - Earth, Wind & Fire";
        wrong_band_recording.title = other_caption.into();
        wrong_band_recording.source_metadata.as_mut().unwrap()["track"] = json!(other_caption);
        wrong_band_recording.source_metadata.as_mut().unwrap()["catalogTitle"] =
            json!(other_caption);
        assert_assessment_action!(
            assess(
                &band_request,
                &wrong_band_recording,
                MatchLevel::Exact,
                MatchStage::Resolved
            ),
            MatchAction::Reject
        );
    }

    #[test]
    fn spanish_official_audio_full_caption_supplies_source_bound_credit_completeness() {
        let track = Track {
            provider: "radio_song".into(),
            title: "Cumbia Sobre El Río".into(),
            artist: "Celso Piña y Su Ronda Bogotá".into(),
            isrc: Some("QZDQH1814866".into()),
            ..Track::default()
        };
        let title = "Celso Piña Y Su Ronda Bogotá - Cumbia Sobre El Río (Audio Oficial)";
        let original = Candidate {
            provider: "youtube".into(),
            id: "youtube:1guXeejD3G4".into(),
            title: title.into(),
            uploader: Some("Dame Regional".into()),
            duration_seconds: Some(191.0),
            source_metadata: Some(
                json!({"provider":"youtube", "sourceMetadataOrigin":"ytDlpFullInfo", "resolvedProviderId":"1guXeejD3G4", "extractor":true, "catalogTitle":title, "track":title, "duration":191.0}),
            ),
            ..Candidate::default()
        };
        let evaluate = |candidate: &Candidate| {
            assess(
                &track,
                candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved,
            )
        };
        let observed = evaluate(&original);
        assert_eq!(observed.dimensions.title, Comparison::Agree);
        assert_eq!(observed.dimensions.artists, Comparison::Agree);
        assert_eq!(observed.action, MatchAction::Accept);
        assert!(
            observed
                .evidence
                .as_ref()
                .unwrap()
                .complete_performer_credits
        );
        assert!(!observed
            .reasons
            .contains(&MatchReason::UnresolvedAnnotation));
        let mut complete = original.clone();
        complete.source_metadata.as_mut().unwrap()["artists"] = json!([track.artist]);
        complete.source_metadata.as_mut().unwrap()["artistCount"] = json!(1);
        complete.source_metadata.as_mut().unwrap()["performerCreditsComplete"] = json!(true);
        assert_eq!(evaluate(&complete).action, MatchAction::Accept);
        let with_title = |value: &str| {
            let mut candidate = complete.clone();
            candidate.title = value.into();
            candidate.source_metadata.as_mut().unwrap()["track"] = json!(value);
            candidate.source_metadata.as_mut().unwrap()["catalogTitle"] = json!(value);
            candidate
        };
        assert_eq!(
            evaluate(&with_title(
                &title.replace("Audio Oficial", "Official Audio")
            ))
            .action,
            MatchAction::Accept
        );
        for suffix in [
            "(Audio Oficial) (Live)",
            "(Audio Oficial Remix)",
            "(Audio Oficial) (Unknown Label)",
            "(Audio Oficial) (Club Remix)",
        ] {
            assert_ne!(
                evaluate(&with_title(&title.replace("(Audio Oficial)", suffix))).action,
                MatchAction::Accept,
                "{suffix}"
            );
        }
        let mut wrong_artist =
            with_title(&title.replace("Celso Piña Y Su Ronda Bogotá", "Different Artist"));
        wrong_artist.source_metadata.as_mut().unwrap()["artists"] = json!(["Different Artist"]);
        assert_ne!(evaluate(&wrong_artist).action, MatchAction::Accept);
        let mut wrong_isrc = complete;
        wrong_isrc.source_metadata.as_mut().unwrap()["isrc"] = json!("USRC17607839");
        assert_ne!(evaluate(&wrong_isrc).action, MatchAction::Accept);
    }

    #[test]
    fn manual_artist_supersedes_stale_provider_credits_without_weakening_identity() {
        let requested = Track {
            artist: "New Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            provenance: json!({"manualFields":{"artist":true},"catalog":{"provider":"spotify","verified":true,"artists":[{"name":"Old Artist"}],"artistCount":1}}),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            title: "New Artist - Song".into(),
            duration_seconds: Some(200.0),
            ..Candidate::default()
        };
        let result = assess(
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(result.action, MatchAction::Accept, "{result:?}");
        for title in [
            "Old Artist - Song",
            "Different Artist - Song",
            "New Artist - Song (Live)",
            "New Artist - Song (Other Remix)",
            "New Artist - Other Song",
        ] {
            let other = Candidate {
                title: title.into(),
                ..candidate.clone()
            };
            assert_ne!(
                assess(
                    &requested,
                    &other,
                    MatchLevel::Balanced,
                    MatchStage::Resolved
                )
                .action,
                MatchAction::Accept,
                "{title}"
            );
        }
        let mut atomic = requested.clone();
        atomic.artist = "Earth, Wind & Fire".into();
        assert_eq!(
            structured_primary_credits(&atomic, &atomic.artist),
            vec!["Earth, Wind & Fire"]
        );
        atomic.provenance["manualFields"]["artistCredits"] = json!(true);
        atomic.provenance["artistCredits"] = json!([{"name":"Earth, Wind & Fire","role":"artist"}]);
        assert_eq!(
            structured_primary_credits(&atomic, &atomic.artist),
            vec!["Earth, Wind & Fire"]
        );
    }

    #[test]
    fn youtube_caption_parses_featured_artist_from_artist_prefix() {
        let expected = vec![
            "Nipsey Hussle".to_owned(),
            "Bino Rideaux".to_owned(),
            "James Fauntleroy".to_owned(),
        ];
        let caption = "Nipsey Hussle and Bino Rideaux (feat. James Fauntleroy) - Sacrifices (Official Music Video)";
        assert_eq!(
            caption_artist_names(
                caption,
                "Nipsey Hussle and Bino Rideaux",
                &[],
                Some(&expected)
            ),
            expected
        );

        // A suffix credit does not authorize a similarly spelled, unverified
        // request alias such as a legal suffix; alias evidence is handled by
        // its own verified profile path.
        let different_request = vec![
            "Nipsey Hussle".to_owned(),
            "Bino Rideaux".to_owned(),
            "James Fauntleroy II".to_owned(),
        ];
        assert!(caption_artist_names(
            caption,
            "Nipsey Hussle and Bino Rideaux",
            &[],
            Some(&different_request),
        )
        .is_empty());
    }

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
    fn named_remix_dash_title_matches_caption_with_catalog_remixer_credit() {
        let mut request = Track {
            artist: "Bomba Estéreo, Tainy".into(),
            title: "To My Love - Tainy Remix".into(),
            duration_seconds: Some(251.0),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            title: "Bomba Estéreo - To My Love (Tainy Remix)".into(),
            duration_seconds: Some(252.0),
            ..Candidate::default()
        };
        for structured in [false, true] {
            if structured {
                request.provenance = json!({"artistCredits":[
                    {"name":"Bomba Estéreo","role":"artist"},
                    {"name":"Tainy","role":"artist"}
                ]});
            }
            let result = assess(
                &request,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Search,
            );
            assert!(
                matches!(result.action, MatchAction::Accept | MatchAction::Inspect),
                "{result:?}"
            );
            assert_eq!(result.dimensions.title, Comparison::Agree, "{result:?}");
            assert_eq!(result.dimensions.version, Comparison::Agree, "{result:?}");
            assert_eq!(result.dimensions.artists, Comparison::Agree, "{result:?}");
            let audio = Candidate {
                title: "Bomba Estéreo - To My Love (Tainy Remix)[Audio]".into(),
                duration_seconds: Some(254.0),
                ..candidate.clone()
            };
            let result = assess(&request, &audio, MatchLevel::Balanced, MatchStage::Search);
            assert!(
                matches!(result.action, MatchAction::Accept | MatchAction::Inspect),
                "{result:?}"
            );
            for suffix in ["Other Remix", "Live", "Cover"] {
                let other = Candidate {
                    title: format!("Bomba Estéreo - To My Love ({suffix})"),
                    ..candidate.clone()
                };
                let result = assess(&request, &other, MatchLevel::Balanced, MatchStage::Search);
                assert_ne!(result.action, MatchAction::Accept, "{suffix}: {result:?}");
            }
        }
    }

    #[test]
    fn vocal_edit_full_metadata_identifies_all_requested_performers() {
        let request = Track {
            artist: "El Búho, Gotopo".into(),
            title: "Mañana Tepotzlan - Vocal Edit".into(),
            duration_seconds: Some(270.0),
            provenance: json!({"catalog":{"artistCount":2,"artists":[
                {"name":"El Búho","provider":"spotify","id":"1I7FVmvisCtSFzmm87mbLR"},
                {"name":"Gotopo","provider":"spotify","id":"0rmwkyTOL6DJ3GISXikmUF"}
            ],"provider":"spotify","verified":true}}),
            ..Track::default()
        };
        let mut candidate = Candidate {
            id: "youtube:2fT_ojy_tYc".into(),
            provider: "youtube".into(),
            title: "Mañana Tepotzlan (Vocal Edit)".into(),
            duration_seconds: Some(270.0),
            confidence: "low".into(),
            source_metadata: Some(json!({"artistCount":2,"artists":["El Búho","Gotopo"],
                "catalogTitle":"Mañana Tepotzlan (Vocal Edit)","track":"Mañana Tepotzlan (Vocal Edit)",
                "extractor":true,"resolvedProviderId":"2fT_ojy_tYc","sourceMetadataOrigin":"ytDlpFullInfo"})),
            ..Candidate::default()
        };
        let resolved = assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(resolved.action, MatchAction::Accept, "{resolved:?}");
        candidate.title = "Mañana Tepotzlan (Radio Edit)".into();
        candidate.source_metadata.as_mut().unwrap()["catalogTitle"] =
            json!("Mañana Tepotzlan (Radio Edit)");
        candidate.source_metadata.as_mut().unwrap()["track"] =
            json!("Mañana Tepotzlan (Radio Edit)");
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
    }

    fn soulseek_candidate(filename: &str) -> Candidate {
        let basename = filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap()
            .rsplit_once('.')
            .unwrap()
            .0;
        let observations =
            crate::recording_filename::verified_filename_observations(filename, basename).unwrap();
        Candidate {
            provider: "slskd".into(),
            id: "slskd:fixture".into(),
            title: basename.into(),
            duration_seconds: Some(141.5),
            source_metadata: Some(serde_json::json!({
                "sourceMetadataOrigin":"slskdFilename",
                "filenameObservations":observations
            })),
            source: Some(CandidateSource::Soulseek {
                username: "peer".into(),
                filename: filename.into(),
                size_bytes: 1_000_000,
                bitrate_kbps: None,
                search_id: None,
                batch_id: None,
                transfer_id: None,
            }),
            ..Candidate::default()
        }
    }

    #[test]
    fn soulseek_filename_observations_are_verified_caption_evidence_only() {
        let request = Track {
            artist: "Bulgarian State Television Female Choir".into(),
            title: "Pilentze Pee".into(),
            duration_seconds: Some(141.5),
            ..Track::default()
        };
        let candidate = soulseek_candidate(
            "Music/Bulgarian State Television Female Choir/Le Mystere/01 - Pilentze Pee.flac",
        );
        let prepared = prepare(&request, &candidate);
        let candidate_evidence = prepared.candidate_evidence();
        let filename_title = candidate_evidence
            .texts
            .iter()
            .find(|observation| {
                observation.origin.field == "sourceMetadata.filenameObservations.normalizedTitle"
            })
            .unwrap();
        let folder_artist = candidate_evidence
            .performers
            .iter()
            .find(|observation| {
                observation.origin.field == "sourceMetadata.filenameObservations.folderArtist"
            })
            .unwrap();
        assert_eq!(
            filename_title.origin.source_group,
            folder_artist.origin.source_group
        );
        assert_eq!(
            filename_title.origin.authority,
            EvidenceAuthority::UploaderCaption
        );
        assert_eq!(
            folder_artist.origin.authority,
            EvidenceAuthority::UploaderCaption
        );
        assert!(!folder_artist.complete);

        let balanced = assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(balanced.dimensions.title, Comparison::Agree);
        assert_eq!(balanced.dimensions.artists, Comparison::Agree);
        assert_eq!(balanced.relationship, MatchRelationship::Probable);
        assert_eq!(balanced.action, MatchAction::Accept);
        assert!(balanced.reasons.contains(&MatchReason::FilenameCaptionOnly));
        let evidence = balanced.evidence.unwrap();
        assert_eq!(evidence.proof_strength, ProofStrength::MetadataOnly);
        assert!(!evidence.complete_performer_credits);
        assert!(!evidence.requested_performer_credits_complete);
        assert_eq!(
            evidence.dimensions,
            MatchDimensions {
                title: Comparison::Agree,
                artists: Comparison::Agree,
                guests: Comparison::Unknown,
                version: Comparison::Unknown,
                runtime: Comparison::Agree,
                isrc: Comparison::Unknown,
                work: Comparison::Unknown,
            }
        );

        let exact = assess(
            &request,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
        );
        assert_ne!(exact.action, MatchAction::Accept);

        let mut no_runtime = request.clone();
        no_runtime.duration_seconds = None;
        assert_ne!(
            assess(
                &no_runtime,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "a matching filename and folder alone do not identify the recording"
        );
        let wrong_title = soulseek_candidate(
            "Music/Bulgarian State Television Female Choir/Le Mystere/01 - Different Song.flac",
        );
        assert_ne!(
            assess(
                &request,
                &wrong_title,
                MatchLevel::Flexible,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "folder artist alone is insufficient"
        );
        let mut missing_guest = request.clone();
        missing_guest.title = "Pilentze Pee (feat. Guest Singer)".into();
        assert_ne!(
            assess(
                &missing_guest,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "a requested guest must be supported by the filename evidence"
        );

        let mut wrong_version = soulseek_candidate(
            "Music/Bulgarian State Television Female Choir/Le Mystere/01 - Pilentze Pee (Live).flac",
        );
        let version = assess(
            &request,
            &wrong_version,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(version.dimensions.version, Comparison::Conflict);
        assert_eq!(version.action, MatchAction::Reject);

        let mut stale = soulseek_candidate(
            "Music/Bulgarian State Television Female Choir/Le Mystere/03 - Other Song.flac",
        );
        stale.source_metadata.as_mut().unwrap()["filenameObservations"]["normalizedTitle"] =
            serde_json::json!("Pilentze Pee");
        let stale_assessment = assess(&request, &stale, MatchLevel::Balanced, MatchStage::Resolved);
        assert_ne!(stale_assessment.dimensions.title, Comparison::Agree);

        // The same fields on an untyped/other-provider candidate are ignored.
        wrong_version.provider = "youtube".into();
        wrong_version.source = None;
        let forged = assess(
            &request,
            &wrong_version,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(forged.dimensions.version, Comparison::Conflict);
        assert_eq!(forged.action, MatchAction::Reject);
        let forged_input = prepare(&request, &wrong_version);
        assert!(!forged_input
            .candidate_evidence()
            .texts
            .iter()
            .any(|observation| observation.origin.field.contains("filenameObservations")));
        assert!(!forged_input
            .candidate_evidence()
            .performers
            .iter()
            .any(|observation| observation.origin.field.contains("filenameObservations")));
    }

    #[test]
    fn soulseek_title_first_caption_is_weak_exact_title_and_open_credit_evidence() {
        let request = Track {
            artist: "Lamomali & Yamê".into(),
            title: "Totem".into(),
            duration_seconds: Some(179.582),
            isrc: Some("FR11Y2500030".into()),
            provenance: json!({
                "artistCredits":[
                    {"name":"Lamomali","role":"artist credit"},
                    {"name":"Yamê","role":"artist credit"}
                ],
                "manualFields":{}
            }),
            ..Track::default()
        };
        let filename = "Music/Lamomali/Lamomali Totem/1-57. Totem - Lamomali, -M-, Yamê, Fatoumata Diawara, Toumani Diabaté, Balla Diabaté.flac";
        let mut candidate = soulseek_candidate(filename);
        candidate.duration_seconds = Some(179.5);

        let prepared = prepare(&request, &candidate);
        let title_evidence = prepared
            .candidate_evidence()
            .texts
            .iter()
            .find(|observation| {
                observation.origin.field == "sourceMetadata.filenameObservations.titleFirstTitle"
            })
            .expect("exact left-side title is retained as its own weak observation");
        assert_eq!(title_evidence.parsed.base_title, "Totem");
        assert_eq!(
            title_evidence.origin.authority,
            EvidenceAuthority::UploaderCaption
        );
        let title_first_credits = prepared
            .candidate_evidence()
            .performers
            .iter()
            .filter(|observation| {
                observation.origin.field == "sourceMetadata.filenameObservations.titleFirstCredit"
            })
            .collect::<Vec<_>>();
        assert!(title_first_credits.len() >= 5);
        assert!(title_first_credits.iter().all(|credit| {
            credit.origin.authority == EvidenceAuthority::UploaderCaption && !credit.complete
        }));

        let balanced = assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(balanced.dimensions.title, Comparison::Agree);
        assert_eq!(balanced.dimensions.artists, Comparison::Agree);
        assert_eq!(balanced.dimensions.runtime, Comparison::Agree);
        assert_eq!(balanced.relationship, MatchRelationship::Probable);
        assert_eq!(balanced.action, MatchAction::Accept);
        assert!(balanced.reasons.contains(&MatchReason::FilenameCaptionOnly));
        let evidence = balanced.evidence.unwrap();
        assert_eq!(evidence.proof_strength, ProofStrength::MetadataOnly);
        assert!(!evidence.complete_performer_credits);
        assert!(!evidence.requested_performer_credits_complete);
        assert!(!evidence.extra_performers.is_empty());
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "filename captions cannot prove the complete recording credit set"
        );
        let flexible = assess(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(flexible.dimensions.title, Comparison::Agree);
        assert_eq!(flexible.dimensions.artists, Comparison::Agree);
        assert_eq!(flexible.dimensions.runtime, Comparison::Agree);
        assert_eq!(flexible.relationship, MatchRelationship::Probable);
        assert_eq!(flexible.action, MatchAction::Accept);
        let flexible_evidence = flexible.evidence.unwrap();
        assert_eq!(
            flexible_evidence.proof_strength,
            ProofStrength::MetadataOnly
        );
        assert!(!flexible_evidence.complete_performer_credits);
        assert!(!flexible_evidence.requested_performer_credits_complete);

        let wrong_title = soulseek_candidate(
            "Music/Lamomali/Lamomali Totem/1-57. Other Song - Lamomali, Yamê.flac",
        );
        assert_ne!(
            assess(
                &request,
                &wrong_title,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "artist text cannot override a different left-side title"
        );

        let missing_requested_credit = soulseek_candidate(
            "Music/Lamomali/Lamomali Totem/1-57. Totem - Lamomali, Fatoumata Diawara.flac",
        );
        assert_ne!(
            assess(
                &request,
                &missing_requested_credit,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "a title-first caption must account for every requested performer"
        );

        let conflicting_edition = soulseek_candidate(
            "Music/Lamomali/Lamomali Totem/1-57. Totem (Live) - Lamomali, Yamê.flac",
        );
        assert_ne!(
            assess(
                &request,
                &conflicting_edition,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "the title-first parser cannot peel an edition annotation"
        );
    }

    #[test]
    fn browsed_soulseek_folder_row_keeps_raw_caption_assessable_for_explicit_selection() {
        // browse_soulseek_directory returns an artistless row whose title is
        // the verified normalized filename caption. The actual source locator
        // remains attached as audioCandidate and is later checked by the
        // explicit-folder selector; this assessment must preserve title and
        // runtime agreement without inventing canonical performer credits.
        let filename = "Music/Lamomali/Lamomali Totem/1-57. Totem - Lamomali, -M-, Yamê, Fatoumata Diawara, Toumani Diabaté, Balla Diabaté.flac";
        let mut candidate = soulseek_candidate(filename);
        candidate.duration_seconds = Some(179.5);
        let normalized_title = candidate.source_metadata.as_ref().unwrap()["filenameObservations"]
            ["normalizedTitle"]
            .as_str()
            .unwrap()
            .to_owned();
        let browsed_row = Track {
            id: candidate.id.clone(),
            provider: "slskd".into(),
            provider_id: "fixture".into(),
            artist: String::new(),
            title: normalized_title,
            duration_seconds: candidate.duration_seconds,
            provenance: json!({
                "sourceKind":"soulseekFolder",
                "audioCandidate":candidate,
                "identity":"unverified peer filename"
            }),
            ..Track::default()
        };

        let assessment = assess(
            &browsed_row,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(assessment.dimensions.title, Comparison::Agree);
        assert_eq!(assessment.dimensions.runtime, Comparison::Agree);
        assert_eq!(assessment.dimensions.artists, Comparison::Unknown);
        assert_ne!(assessment.action, MatchAction::Reject);
        assert_ne!(assessment.action, MatchAction::Accept);
        assert_eq!(browsed_row.provider, "slskd");
        assert_eq!(browsed_row.id, candidate.id);
        assert_eq!(browsed_row.provenance["sourceKind"], "soulseekFolder");
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
        let recording_id = "ec12bebd-e1b9-42ff-892b-4aa4825ac8ea";
        t.provenance["recordingId"] = json!(recording_id);
        let mut c = cand();
        c.id = format!("musicbrainz:{recording_id}");
        c.source_metadata.as_mut().unwrap()["provider"] = json!("musicbrainz");
        c.source_metadata.as_mut().unwrap()["verified"] = json!(true);
        c.duration_seconds = None;
        c.source_metadata.as_mut().unwrap()["duration"] = Value::Null;
        c.source_metadata.as_mut().unwrap()["isrc"] = Value::Null;
        c.source_metadata.as_mut().unwrap()["recordingId"] = json!(recording_id);
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
            id: "youtube:caption-complete".into(),
            title: "Holger Czukay, Jah Wobble, Jaki Liebezeit - How Much Are They?".into(),
            duration_seconds: Some(289.0),
            source_metadata: Some(
                json!({"provider":"youtube","resolvedProviderId":"caption-complete","sourceMetadataOrigin":"ytDlpCandidateMetadata","artists":null,"channelId":"UCx"}),
            ),
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
            id: "soundcloud:caption-composite".into(),
            provider: "soundcloud".into(),
            title: title.into(),
            duration_seconds: Some(139.0),
            source_metadata: Some(
                json!({"provider":"soundcloud","resolvedProviderId":"caption-composite","sourceMetadataOrigin":"ytDlpCandidateMetadata","artists":[artist],"artistCount":1}),
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
            id: "youtube:MatthewTop1".into(),
            title: "John Denver - Matthew".into(),
            duration_seconds: Some(228.0),
            uploader: Some("John Denver - Topic".into()),
            source_metadata: Some(json!({
                "provider":"youtube", "channel":"John Denver - Topic",
                "resolvedProviderId":"MatthewTop1", "sourceMetadataOrigin":"ytDlpFullInfo",
                "channelId":"UC-cnCe5zradDZlcI8YZboBg", "artist":"John Denver",
                "artists":["John Denver"], "artistCount":1,
                "performerCreditsComplete":true, "track":"Matthew",
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
                json!({"provider":"monochrome","trackId":"12345678901234567","catalogTitle":"Nighttime in the Backyard","artistNames":["Lucas Santtana","Arto Lindsay"],"artistCount":2,"performerCreditsComplete":true}),
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
    fn monochrome_isrc_identity_accepts_unknown_version_but_requested_version_enriches() {
        let t = track();
        let c = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:recording-1".into(),
            title: "Main Artist - Song".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(json!({
                "provider":"monochrome", "track":"Song", "artists":["Main Artist"],
                "duration":200.0, "isrc":"USABC2600001"
            })),
            ..Candidate::default()
        };
        let assessment = assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(assessment.dimensions.isrc, Comparison::Agree);
        assert_eq!(assessment.dimensions.version, Comparison::Unknown);
        assert_eq!(assessment.relationship, MatchRelationship::Exact);
        assert_eq!(assessment.action, MatchAction::Accept);

        let mut requested_version = t;
        requested_version.version = Some("Live".into());
        let assessment = assess(
            &requested_version,
            &c,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(assessment.action, MatchAction::Enrich);
    }

    #[test]
    fn exact_recording_evidence_allows_only_comparison_level_accent_credit_variant() {
        let mut request = Track {
            artist: "Lamomali feat. Seu Jorge, Angélique Kidjo & Ibrahim Maalouf".into(),
            title: "Le peuple qui danse".into(),
            duration_seconds: Some(247.944),
            isrc: Some("FR11Y2500100".into()),
            provenance: json!({"artistCredits":[
                {"name":"Lamomali","role":"artist"},
                {"name":"Seu Jorge","role":"artist"},
                {"name":"Angélique Kidjo","role":"artist"},
                {"name":"Ibrahim Maalouf","role":"artist"}
            ]}),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:176810682602233856".into(),
            title: "Lamomali, -M-, Seu Jorge, Ibrahim Maalouf, Angelique Kidjo, Fatoumata Diawara, Toumani Diabaté, Balla Diabaté - Le peuple qui danse".into(),
            duration_seconds: Some(247.943),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"176810682602233856",
                "catalogTitle":"Le peuple qui danse",
                "artistNames":["Lamomali","-M-","Seu Jorge","Ibrahim Maalouf","Angelique Kidjo","Fatoumata Diawara","Toumani Diabaté","Balla Diabaté"],
                "artistCount":8,
                "duration":247.943,
                "isrc":"FR11Y2500100"
            })),
            ..Candidate::default()
        };
        let le_peuple_request = request.clone();

        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            let assessment = assess(&request, &candidate, level, MatchStage::Resolved);
            assert_eq!(assessment.dimensions.title, Comparison::Agree);
            assert_eq!(assessment.dimensions.runtime, Comparison::Agree);
            assert_eq!(assessment.dimensions.isrc, Comparison::Agree);
            assert_eq!(assessment.dimensions.artists, Comparison::Agree);
            assert_eq!(assessment.action, MatchAction::Accept);
            let evidence = assessment.evidence.as_ref().unwrap();
            assert!(evidence
                .extra_performers
                .iter()
                .any(|credit| credit == "angelique kidjo"));
            assert!(evidence
                .extra_performers
                .iter()
                .any(|credit| credit == "fatoumata diawara"));
            assert!(
                assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .accent_folded_performer_match
            );
        }
        let exact = assess(
            &request,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
        );
        assert_ne!(exact.action, MatchAction::Accept);

        // A different performer is not made equivalent by the accent fallback.
        request.artist = "Lamomali feat. Patrick Wilson".into();
        request.provenance = json!({"artistCredits":[
            {"name":"Lamomali","role":"artist"},
            {"name":"Patrick Wilson","role":"artist"}
        ]});
        request.title = "Secret Garden".into();
        request.isrc = Some("FR11Y2500120".into());
        request.duration_seconds = Some(251.68);
        let wrong_performer = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:secret-garden".into(),
            title: "Lamomali - Secret Garden".into(),
            duration_seconds: Some(251.68),
            source_metadata: Some(json!({
                "provider":"monochrome","trackId":"secret-garden",
                "catalogTitle":"Secret Garden","artistNames":["Lamomali","Patrick Watson"],
                "artistCount":2,"duration":251.68,"isrc":"FR11Y2500120"
            })),
            ..Candidate::default()
        };
        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            let assessment = assess(&request, &wrong_performer, level, MatchStage::Resolved);
            // Monochrome ID/count plus a partial artist array cannot prove
            // Patrick Watson is an exhaustive replacement for Patrick Wilson.
            assert_eq!(assessment.dimensions.artists, Comparison::Unknown);
            assert_ne!(assessment.action, MatchAction::Accept);
            assert!(
                !assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .accent_folded_performer_match
            );
        }

        // Without the exact identifier, a similar accent spelling is not
        // enough to resolve otherwise plausible performer credits.
        let mut unbound = candidate.clone();
        unbound.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            let assessment = assess(&le_peuple_request, &unbound, level, MatchStage::Resolved);
            assert_eq!(assessment.dimensions.artists, Comparison::Unknown);
            assert_eq!(assessment.dimensions.isrc, Comparison::Conflict);
            assert_ne!(assessment.action, MatchAction::Accept);
            assert!(
                !assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .accent_folded_performer_match
            );
        }

        // Multiple candidate credits with one folded key are ambiguous and
        // cannot satisfy a missing exact spelling through the fallback.
        let ambiguous_request = Track {
            artist: "José Álvarez".into(),
            title: "El Canto".into(),
            duration_seconds: Some(181.0),
            isrc: Some("FR11Y2500100".into()),
            ..Track::default()
        };
        let ambiguous_candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:ambiguous-accent-credits".into(),
            title: "José Álvarez - El Canto".into(),
            duration_seconds: Some(181.0),
            source_metadata: Some(json!({
                "provider":"monochrome","trackId":"ambiguous-accent-credits",
                "catalogTitle":"El Canto","artistNames":["Jose Alvarez","José Alvarez"],
                "artistCount":2,"duration":181.0,"isrc":"FR11Y2500100"
            })),
            ..Candidate::default()
        };
        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            let assessment = assess(
                &ambiguous_request,
                &ambiguous_candidate,
                level,
                MatchStage::Resolved,
            );
            assert_eq!(assessment.dimensions.artists, Comparison::Conflict);
            assert!(
                !assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .accent_folded_performer_match
            );
        }

        // Exact identifiers cannot suppress an explicit edition conflict.
        let mut remix = candidate.clone();
        remix.title = "Lamomali - Le peuple qui danse (Remix)".into();
        remix.source_metadata.as_mut().unwrap()["catalogTitle"] =
            json!("Le peuple qui danse (Remix)");
        remix.source_metadata.as_mut().unwrap()["version"] = json!("Remix");
        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            let requested_original = Track {
                version: Some("Original".into()),
                ..le_peuple_request.clone()
            };
            let assessment = assess(&requested_original, &remix, level, MatchStage::Resolved);
            assert_eq!(assessment.dimensions.version, Comparison::Conflict);
            assert_ne!(assessment.action, MatchAction::Accept);
            assert!(
                !assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .accent_folded_performer_match
            );
        }
    }
    #[test]
    fn exact_isrc_can_resolve_matching_base_title_with_unclassified_gloss() {
        let mut candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:gloss-row".into(),
            title: "Main Artist - Song (The Evening Gathering)".into(),
            duration_seconds: Some(200.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "trackId":"gloss-row",
                "track":"Song (The Evening Gathering)",
                "catalogTitle":"Song (The Evening Gathering)",
                "artistNames":["Main Artist"],
                "duration":200.0,
                "isrc":"USABC2600001"
            })),
            ..Candidate::default()
        };
        let balanced = assess(
            &track(),
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(balanced.dimensions.title, Comparison::Unknown);
        assert_eq!(balanced.dimensions.isrc, Comparison::Agree);
        assert_eq!(balanced.relationship, MatchRelationship::Probable);
        assert_eq!(balanced.action, MatchAction::Accept);
        assert!(balanced
            .reasons
            .contains(&MatchReason::UnresolvedAnnotation));
        assert_ne!(
            assess(
                &track(),
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );

        candidate.title = "Wrong Artist - Song (The Evening Gathering)".into();
        candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Wrong Artist"]);
        let wrong_artist = assess(
            &track(),
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(wrong_artist.dimensions.artists, Comparison::Conflict);
        assert_eq!(wrong_artist.action, MatchAction::Reject);

        candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Main Artist"]);
        candidate.title = "Main Artist - Song (Live)".into();
        candidate.source_metadata.as_mut().unwrap()["track"] = json!("Song (Live)");
        candidate.source_metadata.as_mut().unwrap()["catalogTitle"] = json!("Song (Live)");
        let wrong_version = assess(
            &track(),
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(wrong_version.dimensions.version, Comparison::Conflict);
        assert_eq!(wrong_version.action, MatchAction::Reject);
    }

    #[test]
    fn structured_single_artist_credit_keeps_and_inside_the_artist_name() {
        let mut request = track();
        request.artist = "The Bulgarian State Radio and Television Female Vocal Choir".into();
        request.provenance = json!({
            "artistCredits":[{
                "name":"The Bulgarian State Radio and Television Female Vocal Choir",
                "role":"artist credit",
                "artist":{"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea"}
            }]
        });
        let requested = requested_credit_parts_for_track(&request, &request.artist, &[]);
        assert_eq!(
            requested,
            vec!["The Bulgarian State Radio and Television Female Vocal Choir".to_owned()]
        );
        let mut candidate = cand();
        candidate.source_metadata.as_mut().unwrap()["artists"] =
            json!(["Bulgarian State Television Female Choir"]);
        let evidence = compare_prepared(&prepare(&request, &candidate));
        assert_eq!(
            evidence.missing_performers,
            vec!["the bulgarian state radio and television female vocal choir".to_owned()]
        );
        assert_eq!(
            evidence.extra_performers,
            vec!["bulgarian state television female choir".to_owned()]
        );
        assert_eq!(evidence.dimensions.artists, Comparison::Conflict);
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
    fn contradictory_or_malformed_artist_arrays_cannot_be_replaced_by_a_caption() {
        let request = Track {
            title: "Song".into(),
            artist: "Main Artist".into(),
            duration_seconds: Some(200.),
            ..Track::default()
        };
        for fields in [
            json!({"artists":["Main Artist"],"artistNames":["Wrong Artist"]}),
            json!({"artists":["Wrong Artist"],"artistNames":["Main Artist"]}),
            json!({"artists":["Main Artist"],"artistNames":7}),
            json!({"artists":"malformed","artistNames":["Main Artist"]}),
            json!({"artists":["Main Artist"],"artistNames":[{"name":"Main Artist","role":7}]}),
            json!({"artists":["Main Artist"],"artistNames":[{"name":"Main Artist","role":"unrecognized role"}]}),
        ] {
            for title in ["Song", "Main Artist - Song"] {
                let mut metadata = fields.clone();
                metadata["provider"] = json!("youtube");
                metadata["track"] = json!("Song");
                let candidate = Candidate {
                    provider: "youtube".into(),
                    id: "youtube:contradictory-credits".into(),
                    title: title.into(),
                    duration_seconds: Some(200.),
                    source_metadata: Some(metadata),
                    ..Candidate::default()
                };
                for stage in [MatchStage::Search, MatchStage::Resolved] {
                    for level in [
                        MatchLevel::Exact,
                        MatchLevel::Balanced,
                        MatchLevel::Flexible,
                    ] {
                        let assessment = assess(&request, &candidate, level, stage);
                        assert_eq!(
                            assessment.dimensions.artists,
                            Comparison::Unknown,
                            "{assessment:?}"
                        );
                        assert_eq!(
                            assessment.action,
                            if stage == MatchStage::Search {
                                MatchAction::Inspect
                            } else {
                                MatchAction::Enrich
                            },
                            "{assessment:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn artist_array_order_and_same_person_multiple_roles_preserve_valid_evidence() {
        for (requested, fields) in [
            (
                vec!["Main Artist"],
                json!({"artists":[{"name":"Main Artist","role":"artist"}],"artistNames":[{"name":"Main Artist","role":"composer"}]}),
            ),
            (
                vec!["Main Artist"],
                json!({"artists":[{"name":"Main Artist","role":"composer"}],"artistNames":[{"name":"Main Artist","role":"artist"}]}),
            ),
            (
                vec!["Main Artist", "Guest"],
                json!({"artists":["Main Artist","Guest"],"artistNames":["Guest","Main Artist"]}),
            ),
        ] {
            let request = Track {
                title: "Song".into(),
                artist: requested.join(", "),
                duration_seconds: Some(200.),
                provenance: json!({"artistCredits":requested.iter().map(|name| json!({"name":name,"role":"artist"})).collect::<Vec<_>>()}),
                ..Track::default()
            };
            let mut metadata = fields;
            metadata["track"] = json!("Song");
            let candidate = Candidate {
                provider: "youtube".into(),
                title: "Song".into(),
                duration_seconds: Some(200.),
                source_metadata: Some(metadata),
                ..Candidate::default()
            };
            let mut baseline = candidate.clone();
            baseline.source_metadata = Some(json!({"track":"Song","artists":requested}));
            for stage in [MatchStage::Search, MatchStage::Resolved] {
                for level in [
                    MatchLevel::Exact,
                    MatchLevel::Balanced,
                    MatchLevel::Flexible,
                ] {
                    let assessment = assess(&request, &candidate, level, stage);
                    let expected = assess(&request, &baseline, level, stage);
                    assert_eq!(
                        assessment.dimensions.artists,
                        Comparison::Agree,
                        "{assessment:?}"
                    );
                    assert_eq!(assessment.action, expected.action, "{assessment:?}");
                    assert_eq!(
                        assessment.relationship, expected.relationship,
                        "{assessment:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn empty_artist_arrays_fall_through_to_other_valid_artist_fields() {
        let mut candidates = Vec::new();
        for mut metadata in [
            json!({"provider":"youtube","track":"Song","duration":200.,"artist":"Main Artist"}),
            json!({"provider":"youtube","track":"Song","duration":200.,"artists":null,"artistNames":null,"artist":"Main Artist"}),
            json!({"provider":"youtube","track":"Song","duration":200.,"artists":[],"artistNames":[],"artist":"Main Artist"}),
            json!({"provider":"youtube","track":"Song","duration":200.,"artists":[],"artistNames":["Main Artist"]}),
            json!({"provider":"youtube","track":"Song","duration":200.,"artists":["Main Artist"],"artistNames":[]}),
        ] {
            // The input fields intentionally cover missing, null, empty and
            // alternate-array cases; each yields the same positive credit.
            metadata["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
            candidates.push(Candidate {
                provider: "youtube".into(),
                id: "youtube:artist-fields".into(),
                title: "Song".into(),
                duration_seconds: Some(200.),
                source_metadata: Some(metadata),
                ..Candidate::default()
            });
        }

        for candidate in candidates {
            for stage in [MatchStage::Search, MatchStage::Resolved] {
                for level in [
                    MatchLevel::Exact,
                    MatchLevel::Balanced,
                    MatchLevel::Flexible,
                ] {
                    let assessment = assess(&track(), &candidate, level, stage);
                    assert_eq!(
                        assessment.dimensions.artists,
                        Comparison::Agree,
                        "{assessment:?}"
                    );
                    assert_eq!(
                        assessment.relationship,
                        MatchRelationship::Exact,
                        "{assessment:?}"
                    );
                    assert_eq!(
                        assessment.action,
                        if stage == MatchStage::Search {
                            MatchAction::Inspect
                        } else {
                            MatchAction::Accept
                        },
                        "{assessment:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_artist_arrays_block_fallback_but_empty_arrays_do_not_hide_conflicts() {
        let malformed = [
            json!("not-an-array"),
            json!([""]),
            json!([{"unexpected":"shape"}]),
            json!([null]),
            json!([42]),
        ];
        for artists in malformed {
            let candidate = Candidate {
                provider: "youtube".into(),
                title: "Song".into(),
                duration_seconds: Some(200.),
                source_metadata: Some(json!({
                    "provider":"youtube",
                    "track":"Song",
                    "duration":200.,
                    "artists":artists,
                    "artistNames":["Main Artist"],
                    "artist":"Main Artist"
                })),
                ..Candidate::default()
            };
            for stage in [MatchStage::Search, MatchStage::Resolved] {
                for level in [
                    MatchLevel::Exact,
                    MatchLevel::Balanced,
                    MatchLevel::Flexible,
                ] {
                    let assessment = assess(&track(), &candidate, level, stage);
                    assert_eq!(
                        assessment.dimensions.artists,
                        Comparison::Unknown,
                        "{assessment:?}"
                    );
                    assert_eq!(
                        assessment.action,
                        if stage == MatchStage::Search {
                            MatchAction::Inspect
                        } else {
                            MatchAction::Enrich
                        },
                        "{assessment:?}"
                    );
                }
            }
        }

        let no_artist_evidence = Candidate {
            provider: "youtube".into(),
            title: "Song".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"Song",
                "duration":200.,
                "artists":[],
                "artistNames":[]
            })),
            ..Candidate::default()
        };
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let assessment = assess(&track(), &no_artist_evidence, level, stage);
                assert_eq!(
                    assessment.dimensions.artists,
                    Comparison::Unknown,
                    "{assessment:?}"
                );
                assert_eq!(
                    assessment.action,
                    if stage == MatchStage::Search {
                        MatchAction::Inspect
                    } else {
                        MatchAction::Enrich
                    },
                    "{assessment:?}"
                );
            }
        }

        let mut conflicting = cand();
        let metadata = conflicting.source_metadata.as_mut().unwrap();
        metadata["artists"] = json!([]);
        metadata["artistNames"] = json!(["Wrong Artist"]);
        metadata["artist"] = json!("Main Artist");
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let assessment = assess(&track(), &conflicting, level, stage);
                assert_eq!(
                    assessment.dimensions.artists,
                    Comparison::Conflict,
                    "{assessment:?}"
                );
                assert_eq!(assessment.action, MatchAction::Reject, "{assessment:?}");
            }
        }
    }

    #[test]
    fn empty_artist_arrays_do_not_weaken_guest_version_or_isrc_conflicts() {
        let request = Track {
            title: "Song feat. Guest One".into(),
            ..track()
        };
        let mut wrong_guest = Candidate {
            provider: "youtube".into(),
            title: "Song feat. Wrong Guest".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"Song feat. Wrong Guest",
                "artists":[],
                "artist":"Main Artist",
                "duration":200.
            })),
            ..Candidate::default()
        };
        wrong_guest.id = "youtube:guest-conflict".into();
        let guest_assessment = assess(
            &request,
            &wrong_guest,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            guest_assessment.dimensions.guests,
            Comparison::Conflict,
            "{guest_assessment:?}"
        );
        assert_eq!(
            guest_assessment.action,
            MatchAction::Reject,
            "{guest_assessment:?}"
        );

        let mut wrong_version = cand();
        wrong_version.source_metadata.as_mut().unwrap()["artists"] = json!([]);
        wrong_version.source_metadata.as_mut().unwrap()["artist"] = json!("Main Artist");
        wrong_version.source_metadata.as_mut().unwrap()["track"] = json!("Song (Radio Edit)");
        let version_request = Track {
            version: Some("Extended Mix".into()),
            ..track()
        };
        let version_assessment = assess(
            &version_request,
            &wrong_version,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            version_assessment.dimensions.version,
            Comparison::Conflict,
            "{version_assessment:?}"
        );
        assert_eq!(
            version_assessment.action,
            MatchAction::Reject,
            "{version_assessment:?}"
        );

        let mut wrong_isrc = cand();
        wrong_isrc.source_metadata.as_mut().unwrap()["artists"] = json!([]);
        wrong_isrc.source_metadata.as_mut().unwrap()["artist"] = json!("Main Artist");
        wrong_isrc.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        let isrc_assessment = assess(
            &track(),
            &wrong_isrc,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            isrc_assessment.dimensions.isrc,
            Comparison::Conflict,
            "{isrc_assessment:?}"
        );
        assert_eq!(
            isrc_assessment.action,
            MatchAction::Reject,
            "{isrc_assessment:?}"
        );
    }
    #[test]
    fn flexible_enriches_when_requested_version_evidence_is_missing() {
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
        assert_eq!(flexible.action, MatchAction::Enrich);
        assert_eq!(flexible.relationship, MatchRelationship::Probable);
        assert_eq!(flexible.dimensions.version, Comparison::Unknown);
    }

    #[test]
    fn explicit_full_album_metadata_blocks_track_substitutions_but_preserves_exact_requests() {
        let request = Track {
            title: "Song".into(),
            artist: "Main Artist".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let album_candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:album-video".into(),
            title: "Song".into(),
            duration_seconds: Some(3_600.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"album-video",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "track":"Song",
                "catalogTitle":"Song",
                "artistNames":["Main Artist"],
                "duration":3600.0,
                "isFullAlbum":true
            })),
            ..Candidate::default()
        };
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let assessment = assess(&request, &album_candidate, level, stage);
                assert_eq!(
                    assessment.action,
                    MatchAction::Reject,
                    "{stage:?}/{level:?}: {assessment:?}"
                );
                assert!(assessment.reasons.contains(&MatchReason::FullAlbumContent));
            }
        }

        // A user explicitly requesting a long-form object can still select it.
        let long_form_request = Track {
            title: "Song Full Album".into(),
            ..request.clone()
        };
        let long_form = Candidate {
            title: "Song Full Album".into(),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"album-video",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "track":"Song Full Album",
                "catalogTitle":"Song Full Album",
                "artistNames":["Main Artist"],
                "duration":3600.0,
                "isFullAlbum":true
            })),
            ..album_candidate.clone()
        };
        let requested_long_form = assess(
            &long_form_request,
            &long_form,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            requested_long_form.action,
            MatchAction::Accept,
            "{requested_long_form:?}"
        );
        assert!(!requested_long_form
            .reasons
            .contains(&MatchReason::FullAlbumContent));

        // Likewise, this guard does not replace an explicitly locked exact video.
        let locked_request = Track {
            provider: "youtube".into(),
            provider_id: "album-video".into(),
            provenance: json!({"explicitSourceLock":true}),
            ..request.clone()
        };
        let exact_locked = assess(
            &locked_request,
            &album_candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(exact_locked.action, MatchAction::Accept, "{exact_locked:?}");
        assert!(!exact_locked
            .reasons
            .contains(&MatchReason::FullAlbumContent));

        let automatic_browser_import = Track {
            provider: "youtube".into(),
            provider_id: "album-video".into(),
            provenance: json!({
                "browserImport":true,
                "captureProvider":"youtube",
                "playbackMode":"source"
            }),
            ..request.clone()
        };
        let not_manually_locked = assess(
            &automatic_browser_import,
            &album_candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            not_manually_locked.action,
            MatchAction::Reject,
            "{not_manually_locked:?}"
        );
        assert!(not_manually_locked
            .reasons
            .contains(&MatchReason::FullAlbumContent));

        let mut mismatched_marker = album_candidate.clone();
        mismatched_marker.source_metadata.as_mut().unwrap()["resolvedProviderId"] =
            json!("different-video");
        let unbound_marker = assess(
            &request,
            &mismatched_marker,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert!(!unbound_marker
            .reasons
            .contains(&MatchReason::FullAlbumContent));

        // Titles alone are not trusted as aggregate metadata; Medley can be
        // part of an ordinary named recording without triggering this guard.
        let medley_track = Track {
            title: "Medley".into(),
            ..request.clone()
        };
        let mut named_medley = album_candidate;
        named_medley.title = "Medley".into();
        named_medley.source_metadata.as_mut().unwrap()["track"] = json!("Medley");
        named_medley.source_metadata.as_mut().unwrap()["catalogTitle"] = json!("Medley");
        named_medley.source_metadata.as_mut().unwrap()["isFullAlbum"] = json!(false);
        let named_recording = assess(
            &medley_track,
            &named_medley,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert!(!named_recording
            .reasons
            .contains(&MatchReason::FullAlbumContent));

        // Flexible performance permission does not let an explicit album
        // marker turn an individual track request into a long-form match.
        let mut mismatched_runtime = request.clone();
        mismatched_runtime.duration_seconds = Some(240.0);
        let album_with_duration = assess(
            &mismatched_runtime,
            &long_form,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            album_with_duration.action,
            MatchAction::Reject,
            "{album_with_duration:?}"
        );
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
        let options = FlexibleMatchingOptions {
            allow_other_performances: true,
            ..Default::default()
        };
        assert_eq!(
            assess(&t, &c, MatchLevel::Flexible, MatchStage::Resolved).action,
            MatchAction::Reject
        );
        let a = assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options);
        assert_eq!(a.relationship, MatchRelationship::Alternative);
        c.source_metadata.as_mut().unwrap()["workId"] = json!("other-work");
        assert_eq!(
            assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options)
                .relationship,
            MatchRelationship::Alternative
        );
        c.source_metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("workId");
        c.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        let alternative =
            assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options);
        assert_eq!(alternative.relationship, MatchRelationship::Alternative);
        assert_eq!(alternative.action, MatchAction::Accept);
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Different Artist"]);
        assert_eq!(
            assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options)
                .action,
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
            MatchAction::Reject
        );
        let options = FlexibleMatchingOptions {
            allow_other_performances: true,
            allow_different_lengths: true,
            max_longer_by_seconds: 60,
            ..Default::default()
        };
        // A long alternative must be a bound, complete catalog track.
        c.provider = "monochrome".into();
        c.id = "monochrome:123".into();
        c.source_metadata = Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Live)","artistNames":["Main Artist"],"artistCount":1,"performerCreditsComplete":true,"duration":260.,"isrc":"USZZZ2600001"}),
        );
        assert_eq!(
            assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options)
                .action,
            MatchAction::Accept
        );
        c.duration_seconds = Some(20.);
        c.source_metadata.as_mut().unwrap()["duration"] = json!(20.);
        assert_eq!(
            assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options)
                .action,
            MatchAction::Reject
        );
        c.duration_seconds = Some(260.);
        c.source_metadata.as_mut().unwrap()["duration"] = json!(260.);
        c.permission = "restricted".into();
        let denied =
            assess_with_options(&t, &c, MatchLevel::Flexible, MatchStage::Resolved, &options);
        assert_eq!(denied.action, MatchAction::Reject);
        assert!(denied.reasons.contains(&MatchReason::PermissionRestricted));
    }

    #[test]
    fn configured_duration_limits_report_missing_runtime_only_when_enabled() {
        let mut request = track();
        request.duration_seconds = None;
        let candidate = cand();
        let options = FlexibleMatchingOptions {
            allow_different_lengths: true,
            ..Default::default()
        };
        let bounded = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert!(bounded
            .reasons
            .contains(&MatchReason::DurationLimitUnavailable));
        assert!(!bounded
            .reasons
            .contains(&MatchReason::DurationLimitExceeded));
        assert_eq!(
            serde_json::to_value(MatchReason::DurationLimitUnavailable).unwrap(),
            serde_json::json!("durationLimitUnavailable")
        );
        for level in [MatchLevel::Exact, MatchLevel::Balanced] {
            let other =
                assess_with_options(&request, &candidate, level, MatchStage::Resolved, &options);
            assert!(!other
                .reasons
                .contains(&MatchReason::DurationLimitUnavailable));
        }
        let unbounded = assess(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert!(!unbounded
            .reasons
            .contains(&MatchReason::DurationLimitUnavailable));
        request.duration_seconds = candidate.duration_seconds;
        let known = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert!(!known
            .reasons
            .contains(&MatchReason::DurationLimitUnavailable));
    }

    #[test]
    fn flexible_duration_alternatives_obey_asymmetric_seconds_limits() {
        let request = track();
        let options = FlexibleMatchingOptions {
            allow_other_performances: true,
            allow_different_lengths: true,
            max_shorter_by_seconds: 10,
            max_longer_by_seconds: 45,
            ..Default::default()
        };
        let mut candidate = cand();
        candidate.provider = "monochrome".into();
        candidate.id = "monochrome:duration-bound".into();
        candidate.title = "Song (Live)".into();
        candidate.source_metadata = Some(json!({
            "provider":"monochrome",
            "trackId":"duration-bound",
            "catalogTitle":"Song (Live)",
            "artistNames":["Main Artist"],
            "performerCreditsComplete":true,
            "duration":200.0,
            "isrc":"USABC2600001"
        }));

        for (duration, expected) in [
            (190.0, MatchAction::Accept),
            (189.0, MatchAction::Reject),
            (245.0, MatchAction::Accept),
            (246.0, MatchAction::Reject),
        ] {
            candidate.duration_seconds = Some(duration);
            candidate.source_metadata.as_mut().unwrap()["duration"] = json!(duration);
            assert_eq!(
                assess_with_options(
                    &request,
                    &candidate,
                    MatchLevel::Flexible,
                    MatchStage::Resolved,
                    &options
                )
                .action,
                expected,
                "candidate runtime {duration} seconds"
            );
        }

        let zero_tolerance = FlexibleMatchingOptions {
            max_shorter_by_seconds: 0,
            max_longer_by_seconds: 0,
            ..options
        };
        candidate.duration_seconds = Some(201.0);
        candidate.source_metadata.as_mut().unwrap()["duration"] = json!(201.0);
        let zero_assessment = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &zero_tolerance,
        );
        assert_eq!(
            zero_assessment.action,
            MatchAction::Reject,
            "zero limits apply to the total delta, even inside the ordinary runtime tolerance"
        );
        assert_eq!(zero_assessment.dimensions.runtime, Comparison::Agree);
        assert!(zero_assessment
            .reasons
            .contains(&MatchReason::DurationLimitExceeded));

        candidate.duration_seconds = None;
        candidate
            .source_metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("duration");
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options,
            )
            .action,
            MatchAction::Accept,
            "unknown candidate duration cannot satisfy a configured length alternative"
        );
    }

    #[test]
    fn fresh_restored_edition_substitute_respects_explicit_runtime_bounds_and_guards() {
        let request = Track {
            id: "catalog:musicbrainz:4d654402-6a53-46c5-bf68-26c19e4d0fa9:1:5:1:078741e5-6a67-411e-b071-80ccc96586d2".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz:4d654402-6a53-46c5-bf68-26c19e4d0fa9:1:5:1:078741e5-6a67-411e-b071-80ccc96586d2".into(),
            title: "Out of Nowhere (Restored Edition '25)".into(),
            artist: "Django Reinhardt & Coleman Hawkins".into(),
            duration_seconds: Some(196.0),
            provenance: json!({
                "catalogProvider":"musicbrainz",
                "recordingId":"078741e5-6a67-411e-b071-80ccc96586d2",
                "artistCredits":[
                    {
                        "artist":{"id":"650bf385-6f6d-4992-a3b9-779d144920a4","provider":"musicbrainz"},
                        "creditedAs":"Django Reinhardt",
                        "joinPhrase":" & ",
                        "name":"Django Reinhardt",
                        "role":"artist credit"
                    },
                    {
                        "artist":{"id":"5c8cb181-38fe-4300-8153-650b2ed0258f","provider":"musicbrainz"},
                        "creditedAs":"Coleman Hawkins",
                        "joinPhrase":"",
                        "name":"Coleman Hawkins",
                        "role":"artist credit"
                    }
                ]
            }),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:OutOfNow001".into(),
            title: "Out of Nowhere".into(),
            duration_seconds: Some(200.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"OutOfNow001",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "track":"Out of Nowhere",
                "catalogTitle":"Out of Nowhere",
                "artistNames":["Django Reinhardt","Coleman Hawkins"],
                "duration":200.0
            })),
            ..Candidate::default()
        };
        let options = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_different_lengths: true,
            max_longer_by_seconds: 30,
            ..Default::default()
        };

        let positive = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(positive.dimensions.title, Comparison::Agree, "{positive:?}");
        assert_eq!(
            positive.dimensions.artists,
            Comparison::Agree,
            "{positive:?}"
        );
        assert_eq!(
            positive.dimensions.runtime,
            Comparison::Conflict,
            "{positive:?}"
        );
        assert_eq!(
            positive.dimensions.version,
            Comparison::Unknown,
            "{positive:?}"
        );
        assert_eq!(
            positive.relationship,
            MatchRelationship::Alternative,
            "{positive:?}"
        );
        assert_eq!(positive.action, MatchAction::Accept, "{positive:?}");

        let mut too_long = options;
        too_long.max_longer_by_seconds = 3;
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &too_long,
            )
            .action,
            MatchAction::Accept,
            "a four-second delta exceeds the saved three-second bound"
        );
        let mut lengths_disabled = options;
        lengths_disabled.allow_different_lengths = false;
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &lengths_disabled,
            )
            .action,
            MatchAction::Accept,
            "the unspecified-version path cannot bypass the disabled length option"
        );
        for level in [MatchLevel::Exact, MatchLevel::Balanced] {
            assert_ne!(
                assess_with_options(&request, &candidate, level, MatchStage::Resolved, &options,)
                    .action,
                MatchAction::Accept,
                "the saved Flexible alternative does not alter {level:?}"
            );
        }

        let mut wrong_artist = candidate.clone();
        wrong_artist.source_metadata.as_mut().unwrap()["artistNames"] =
            json!(["Different Performer"]);
        let wrong_artist_assessment = assess_with_options(
            &request,
            &wrong_artist,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            wrong_artist_assessment.dimensions.artists,
            Comparison::Conflict,
            "{wrong_artist_assessment:?}"
        );
        assert_ne!(
            wrong_artist_assessment.action,
            MatchAction::Accept,
            "an unrelated performer must not be admitted by the duration option"
        );

        let mut wrong_source = candidate.clone();
        wrong_source.source_metadata.as_mut().unwrap()["resolvedProviderId"] = json!("Different01");
        assert_ne!(
            assess_with_options(
                &request,
                &wrong_source,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options,
            )
            .action,
            MatchAction::Accept,
            "fresh metadata must remain bound to the selected YouTube source"
        );

        let mut wrong_version = candidate.clone();
        wrong_version.title = "Out of Nowhere (Original Version)".into();
        wrong_version.source_metadata.as_mut().unwrap()["track"] =
            json!("Out of Nowhere (Original Version)");
        wrong_version.source_metadata.as_mut().unwrap()["catalogTitle"] =
            json!("Out of Nowhere (Original Version)");
        wrong_version.source_metadata.as_mut().unwrap()["version"] = json!("Original Version");
        let wrong_version_assessment = assess_with_options(
            &request,
            &wrong_version,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            wrong_version_assessment.dimensions.version,
            Comparison::Conflict,
            "{wrong_version_assessment:?}"
        );
        assert_ne!(
            wrong_version_assessment.action,
            MatchAction::Accept,
            "a known contradictory version must not use the unspecified-version path"
        );

        let mut wrong_isrc_request = request.clone();
        wrong_isrc_request.isrc = Some("USABC2600001".into());
        let mut wrong_isrc = candidate.clone();
        wrong_isrc.source_metadata.as_mut().unwrap()["isrc"] = json!("USXYZ2600002");
        let wrong_isrc_assessment = assess_with_options(
            &wrong_isrc_request,
            &wrong_isrc,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            wrong_isrc_assessment.dimensions.isrc,
            Comparison::Conflict,
            "{wrong_isrc_assessment:?}"
        );
        assert_ne!(
            wrong_isrc_assessment.action,
            MatchAction::Accept,
            "a conflicting ISRC remains a hard identity guard"
        );

        for artists in [
            json!(["Django Reinhardt"]),
            json!([
                "Django Reinhardt",
                "Coleman Hawkins",
                "Unexpected Performer"
            ]),
        ] {
            let mut incomplete_or_extra = candidate.clone();
            incomplete_or_extra.source_metadata.as_mut().unwrap()["artistNames"] = artists;
            let assessment = assess_with_options(
                &request,
                &incomplete_or_extra,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options,
            );
            assert!(
                !assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .missing_performers
                    .is_empty()
                    || !assessment
                        .evidence
                        .as_ref()
                        .unwrap()
                        .extra_performers
                        .is_empty(),
                "expected a performer coverage difference: {assessment:?}"
            );
            assert_ne!(
                assessment.action,
                MatchAction::Accept,
                "missing or extra performer evidence must remain outside this exception"
            );
        }
    }

    #[test]
    fn restored_edition_unknown_version_requires_complete_credits_for_selective_release_permission()
    {
        let request = Track {
            id: "catalog:musicbrainz:4d654402-6a53-46c5-bf68-26c19e4d0fa9:1:5:1:078741e5-6a67-411e-b071-80ccc96586d2".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz:4d654402-6a53-46c5-bf68-26c19e4d0fa9:1:5:1:078741e5-6a67-411e-b071-80ccc96586d2".into(),
            title: "Out of Nowhere (Restored Edition '25)".into(),
            artist: "Django Reinhardt & Coleman Hawkins".into(),
            duration_seconds: Some(196.0),
            provenance: json!({
                "catalogProvider":"musicbrainz",
                "recordingId":"078741e5-6a67-411e-b071-80ccc96586d2",
                "artistCredits":[
                    {"artist":{"id":"650bf385-6f6d-4992-a3b9-779d144920a4","provider":"musicbrainz"},
                     "creditedAs":"Django Reinhardt","joinPhrase":" & ","name":"Django Reinhardt","role":"artist credit"},
                    {"artist":{"id":"5c8cb181-38fe-4300-8153-650b2ed0258f","provider":"musicbrainz"},
                     "creditedAs":"Coleman Hawkins","joinPhrase":"","name":"Coleman Hawkins","role":"artist credit"}
                ]
            }),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:OutOfNow001".into(),
            title: "Out of Nowhere".into(),
            duration_seconds: Some(200.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"OutOfNow001",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "track":"Out of Nowhere",
                "catalogTitle":"Out of Nowhere",
                "artistNames":["Django Reinhardt","Coleman Hawkins"],
                "duration":200.0
            })),
            ..Candidate::default()
        };
        let release_only = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: false,
            allow_other_mixes: false,
            allow_other_artists: false,
            allow_different_lengths: true,
            max_longer_by_seconds: 30,
            ..Default::default()
        };
        let selective = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &release_only,
        );
        assert_eq!(
            selective.dimensions.title,
            Comparison::Agree,
            "{selective:?}"
        );
        assert_eq!(
            selective.dimensions.artists,
            Comparison::Agree,
            "{selective:?}"
        );
        assert_eq!(
            selective.dimensions.runtime,
            Comparison::Conflict,
            "{selective:?}"
        );
        assert_eq!(
            selective.dimensions.version,
            Comparison::Unknown,
            "{selective:?}"
        );
        assert_eq!(selective.action, MatchAction::Enrich, "{selective:?}");
        let evidence = selective.evidence.as_ref().expect("resolved evidence");
        assert!(evidence.missing_performers.is_empty(), "{selective:?}");
        assert!(evidence.extra_performers.is_empty(), "{selective:?}");
        assert!(!evidence.complete_performer_credits, "{selective:?}");

        // The explicit broad all-version policy may accept this source-bound,
        // same-title/same-performer unversioned candidate without asserting
        // that its performer list is complete. This remains an alternative,
        // not proof of the requested restored mastering.
        let all_version_categories = FlexibleMatchingOptions {
            allow_other_performances: true,
            allow_other_mixes: true,
            ..release_only
        };
        let broad = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &all_version_categories,
        );
        assert_eq!(broad.dimensions.title, Comparison::Agree, "{broad:?}");
        assert_eq!(broad.dimensions.artists, Comparison::Agree, "{broad:?}");
        assert_eq!(broad.dimensions.version, Comparison::Unknown, "{broad:?}");
        assert_eq!(broad.action, MatchAction::Accept, "{broad:?}");
        assert_eq!(
            broad.relationship,
            MatchRelationship::Alternative,
            "{broad:?}"
        );
        assert!(!broad.evidence.as_ref().unwrap().complete_performer_credits);

        // Selective release-only permission can accept an unknown release once
        // the same candidate actually provides complete performer credits.
        let mut complete_candidate = candidate;
        complete_candidate.source_metadata.as_mut().unwrap()["performerCreditsComplete"] =
            json!(true);
        let complete = assess_with_options(
            &request,
            &complete_candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &release_only,
        );
        assert_eq!(complete.action, MatchAction::Accept, "{complete:?}");
        assert_eq!(
            complete.relationship,
            MatchRelationship::Alternative,
            "{complete:?}"
        );
        assert!(
            complete
                .evidence
                .as_ref()
                .unwrap()
                .complete_performer_credits
        );
    }

    #[test]
    fn flexible_duration_caps_do_not_change_exact_or_balanced_and_cannot_be_bypassed_by_recording_id(
    ) {
        let request = track();
        let mut candidate = cand();
        candidate.duration_seconds = Some(201.0);
        candidate.source_metadata.as_mut().unwrap()["duration"] = json!(201.0);
        let options = FlexibleMatchingOptions {
            allow_different_lengths: true,
            max_shorter_by_seconds: 0,
            max_longer_by_seconds: 0,
            ..Default::default()
        };
        for level in [MatchLevel::Exact, MatchLevel::Balanced] {
            assert_eq!(
                assess_with_options(&request, &candidate, level, MatchStage::Resolved, &options)
                    .action,
                MatchAction::Accept,
                "saved Flexible limits do not alter {level:?}"
            );
        }

        let recording_id = "ec12bebd-e1b9-42ff-892b-4aa4825ac8ea";
        let mut request = request;
        request.provenance["recordingId"] = json!(recording_id);
        let mut candidate = candidate;
        candidate.id = format!("musicbrainz:{recording_id}");
        candidate.provider = "musicbrainz".into();
        candidate.source_metadata.as_mut().unwrap()["provider"] = json!("musicbrainz");
        candidate.source_metadata.as_mut().unwrap()["verified"] = json!(true);
        candidate.source_metadata.as_mut().unwrap()["recordingId"] = json!(recording_id);
        candidate.duration_seconds = Some(250.0);
        candidate.source_metadata.as_mut().unwrap()["duration"] = json!(250.0);
        assert_eq!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options,
            )
            .action,
            MatchAction::Reject,
            "same-recording IDs do not bypass an enabled, exceeded runtime cap"
        );
    }

    #[test]
    fn bacc_unknown_request_runtime_does_not_mean_over_cap_but_known_runtime_obeys_cap() {
        // Mirrors the saved Discogs row: exact title and two complete artist
        // credits, but no release duration, version, or ISRC. The candidate is
        // fresh yt-dlp evidence for the already identified YouTube recording.
        let request = |duration_seconds| Track {
            title: "Bacc Of The Bach".into(),
            artist: "Nipsey Hussle,Bino Rideaux".into(),
            duration_seconds,
            provenance: json!({
                "artistCredits":[
                    {"name":"Nipsey Hussle","creditedAs":"Nipsey Hussle","role":"Artist","artist":{"provider":"discogs","id":"1195171"}},
                    {"name":"Bino Rideaux","creditedAs":"Bino Rideaux","role":"Artist","artist":{"provider":"discogs","id":"4908344"}}
                ]
            }),
            ..Track::default()
        };
        let candidate = |duration_seconds| Candidate {
            provider: "youtube".into(),
            id: "youtube:rpCswezRrLs".into(),
            url: "https://www.youtube.com/watch?v=rpCswezRrLs".into(),
            title: "Bacc of the Bach".into(),
            duration_seconds,
            source_metadata: Some(json!({
                "provider":"youtube",
                "resolvedProviderId":"rpCswezRrLs",
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "extractor":true,
                "track":"Bacc of the Bach",
                "catalogTitle":"Bacc of the Bach",
                "artists":["Nipsey Hussle","Bino Rideaux"],
                "artistCount":2,
                "duration":duration_seconds
            })),
            ..Candidate::default()
        };
        // A provider-bound catalog recording is needed to exercise accepted
        // duration alternatives; fresh YouTube evidence alone does not prove
        // a different-length performance.
        let catalog_candidate = |duration_seconds| Candidate {
            provider: "monochrome".into(),
            id: "monochrome:12345678901234567".into(),
            title: "Bacc of the Bach".into(),
            duration_seconds,
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"12345678901234567",
                "catalogTitle":"Bacc of the Bach",
                "artistNames":["Nipsey Hussle","Bino Rideaux"],
                "performerCreditsComplete":true,
                "duration":duration_seconds
            })),
            ..Candidate::default()
        };
        let options = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: false,
            allow_different_lengths: true,
            max_shorter_by_seconds: 30,
            max_longer_by_seconds: 30,
            ..Default::default()
        };

        let mut no_length_cap = options;
        no_length_cap.allow_different_lengths = false;
        let unconstrained_unknown_runtime = assess_with_options(
            &request(None),
            &candidate(Some(198.0)),
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &no_length_cap,
        );
        assert_eq!(
            unconstrained_unknown_runtime.dimensions.title,
            Comparison::Agree
        );
        assert_eq!(
            unconstrained_unknown_runtime.dimensions.artists,
            Comparison::Agree
        );
        assert_eq!(
            unconstrained_unknown_runtime.dimensions.runtime,
            Comparison::Unknown
        );
        assert_eq!(
            unconstrained_unknown_runtime.action,
            MatchAction::Accept,
            "{unconstrained_unknown_runtime:?}"
        );

        let missing_runtime = assess_with_options(
            &request(None),
            &candidate(Some(198.0)),
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(missing_runtime.dimensions.title, Comparison::Agree);
        assert_eq!(missing_runtime.dimensions.artists, Comparison::Agree);
        assert_eq!(missing_runtime.dimensions.runtime, Comparison::Unknown);
        assert_eq!(missing_runtime.dimensions.version, Comparison::Unknown);
        assert_eq!(
            missing_runtime.action,
            MatchAction::Inspect,
            "{missing_runtime:?}"
        );
        assert_eq!(missing_runtime.relationship, MatchRelationship::Unresolved);
        assert!(!missing_runtime
            .reasons
            .contains(&MatchReason::RuntimeConflict));
        assert!(!missing_runtime
            .reasons
            .contains(&MatchReason::VersionConflict));

        // Complete typed provider credits open the existing missing-runtime
        // catalog exception. That path is independent of the length cap and
        // remains valid when the candidate explicitly proves full credits.
        let mut complete_fresh_candidate = candidate(Some(198.0));
        complete_fresh_candidate.source_metadata.as_mut().unwrap()["performerCreditsComplete"] =
            json!(true);
        let complete_credits = assess_with_options(
            &request(None),
            &complete_fresh_candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(complete_credits.dimensions.title, Comparison::Agree);
        assert_eq!(complete_credits.dimensions.artists, Comparison::Agree);
        assert_eq!(complete_credits.dimensions.runtime, Comparison::Unknown);
        assert_eq!(
            complete_credits.action,
            MatchAction::Accept,
            "{complete_credits:?}"
        );

        let known_runtime = request(Some(198.0));
        for delta in [30.0, -30.0] {
            let within = assess_with_options(
                &known_runtime,
                &catalog_candidate(Some(198.0 + delta)),
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options,
            );
            assert_eq!(within.dimensions.title, Comparison::Agree, "{within:?}");
            assert_eq!(within.dimensions.artists, Comparison::Agree, "{within:?}");
            assert_eq!(
                within.dimensions.runtime,
                Comparison::Conflict,
                "{within:?}"
            );
            assert_eq!(within.action, MatchAction::Accept, "{within:?}");
        }
        let over = assess_with_options(
            &known_runtime,
            &catalog_candidate(Some(229.0)),
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(over.dimensions.runtime, Comparison::Conflict, "{over:?}");
        assert_eq!(over.action, MatchAction::Reject, "{over:?}");
        assert!(over.reasons.contains(&MatchReason::DurationLimitExceeded));
    }

    #[test]
    fn enabling_length_limits_does_not_require_complete_credits_for_in_bound_close_alternatives() {
        let request = track();
        let mut candidate = cand();
        candidate.duration_seconds = Some(201.0);
        candidate.source_metadata.as_mut().unwrap()["duration"] = json!(201.0);
        candidate.source_metadata.as_mut().unwrap()["track"] = json!("Song (Live)");
        let options = FlexibleMatchingOptions {
            allow_other_performances: true,
            allow_different_lengths: true,
            max_shorter_by_seconds: 30,
            max_longer_by_seconds: 30,
            ..Default::default()
        };
        assert_eq!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options,
            )
            .action,
            MatchAction::Accept
        );
    }
}

#[cfg(test)]
mod complete_caption_credit_tests {
    use super::*;
    use serde_json::json;
    fn requested() -> Track {
        Track {
            title: "Vibe With Me".into(),
            artist: "Nipsey Hussle,Bino Rideaux,Leon Thomas".into(),
            ..Track::default()
        }
    }
    fn caption(title: &str) -> Candidate {
        Candidate {
            id: "youtube:ExdT24gJngk".into(),
            provider: "youtube".into(),
            title: title.into(),
            uploader: Some("Nipsey Hussle".into()),
            duration_seconds: Some(232.),
            source_metadata: Some(json!({"catalogTitle":title,"track":title,"duration":232,
                "extractor":true,"resolvedProviderId":"ExdT24gJngk","sourceMetadataOrigin":"ytDlpCandidateMetadata"})),
            ..Candidate::default()
        }
    }
    #[test]
    fn ordinary_with_in_a_title_does_not_invent_a_featured_performer() {
        for title in ["Vibe With Me", "Come With Me", "Dancing With Myself"] {
            assert!(guest_from_title(title).is_empty(), "{title}");
        }
        assert_eq!(
            guest_from_title("Song - with Guest Artist"),
            vec!["Guest Artist"]
        );
        assert_eq!(
            guest_from_title("Song (feat. Guest Artist) (Official Audio)"),
            vec!["Guest Artist"]
        );
    }
    #[test]
    fn captured_vibe_official_caption_has_complete_credits_at_search_and_resolution() {
        let candidate = caption(
            "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)",
        );
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let result = assess(&requested(), &candidate, MatchLevel::Balanced, stage);
            assert_eq!(result.dimensions.title, Comparison::Agree);
            assert_eq!(result.dimensions.artists, Comparison::Agree);
            assert!(result.evidence.as_ref().unwrap().complete_performer_credits);
            assert_eq!(
                result.action,
                match stage {
                    MatchStage::Search => MatchAction::Inspect,
                    MatchStage::Resolved => MatchAction::Accept,
                }
            );
            assert_eq!(result.relationship, MatchRelationship::Probable);
            assert_ne!(
                assess(&requested(), &candidate, MatchLevel::Exact, stage).action,
                MatchAction::Accept
            );
        }
    }
    #[test]
    fn explicit_feature_and_collaboration_punctuation_is_equivalent_without_alias_guessing() {
        for join in [" & ", " and ", " x ", " X ", " × ", ", "] {
            let text = format!(
                "Nipsey Hussle{join}Bino Rideaux – Vibe With Me [featuring Leon Thomas] [Official Audio]"
            );
            assert_eq!(
                assess(
                    &requested(),
                    &caption(&text),
                    MatchLevel::Balanced,
                    MatchStage::Resolved
                )
                .action,
                MatchAction::Accept,
                "{join}"
            );
        }
        let candidate = caption(
            "Nipsey Hussle & Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)",
        );
        let mut canonical = requested();
        canonical.artist = "Nipsey Hussle,Bino Rideaux,Leon Thomas III".into();
        assert_ne!(
            assess(
                &canonical,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
        // A separately verified alias variant may use the exact source spelling;
        // the evaluator itself never infers an artist identity from a suffix.
        let mut verified_variant = canonical;
        verified_variant.artist = requested().artist;
        assert_eq!(
            assess(
                &verified_variant,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
    }
    #[test]
    fn incomplete_wrong_extra_or_substring_credits_cannot_complete_a_caption() {
        for text in [
            "Nipsey Hussle & Bino Rideaux - Vibe With Me (Official Audio)",
            "Nipsey Hussle & Bino Rideaux - Vibe With Me (feat. Another Guest) (Official Audio)",
            "Nipsey Hussle & Bino Rideaux - Vibe With Me (feat. Leon Thomas & Another Guest) (Official Audio)",
            "Nipsey Hussle & Bino Rideaux & Another Artist - Vibe With Me (feat. Leon Thomas) (Official Audio)",
            "Nipsey Hussle Tribute & Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)",
        ] {
            let result = assess(
                &requested(),
                &caption(text),
                MatchLevel::Balanced,
                MatchStage::Resolved,
            );
            assert_ne!(result.action, MatchAction::Accept, "{text}");
            assert!(!result
                .evidence
                .as_ref()
                .unwrap()
                .complete_performer_credits, "{text}");
        }
    }
    #[test]
    fn presentation_stripping_does_not_hide_semantic_version_conflicts() {
        for version in ["Live", "Instrumental", "Remix", "432Hz"] {
            for text in [
                format!(
                    "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas) ({version}) (Official Audio)"
                ),
                format!(
                    "Nipsey Hussle x Bino Rideaux - Vibe With Me ({version}) (feat. Leon Thomas) (Official Audio)"
                ),
            ] {
                assert_ne!(
                    assess(
                        &requested(),
                        &caption(&text),
                        MatchLevel::Balanced,
                        MatchStage::Resolved
                    )
                    .action,
                    MatchAction::Accept,
                    "{text}"
                );
            }
        }
    }
}

#[cfg(test)]
mod canonical_catalog_contradiction_tests {
    use super::*;
    use serde_json::json;
    fn requested() -> Track {
        Track {
            title: "Title".into(),
            artist: "Artist".into(),
            duration_seconds: Some(200.),
            ..Track::default()
        }
    }
    fn candidate(canonical: &str) -> Candidate {
        Candidate {
            id: "monochrome:42".into(),
            provider: "monochrome".into(),
            title: "Artist - Title".into(),
            duration_seconds: Some(200.),
            source_metadata: Some(
                json!({"provider":"monochrome","trackId":"42","catalogTitle":canonical,"artistNames":["Artist"],"artistCount":1}),
            ),
            ..Candidate::default()
        }
    }
    #[test]
    fn stale_display_and_track_fields_cannot_override_bound_catalog_title() {
        for canonical in ["Other Title", "Unrelated Literal - Title"] {
            for track_field in [None, Some("Artist - Title")] {
                let mut c = candidate(canonical);
                if let Some(value) = track_field {
                    c.source_metadata.as_mut().unwrap()["track"] = json!(value);
                }
                for level in [
                    MatchLevel::Exact,
                    MatchLevel::Balanced,
                    MatchLevel::Flexible,
                ] {
                    let result = assess(&requested(), &c, level, MatchStage::Resolved);
                    assert_eq!(result.dimensions.title, Comparison::Conflict, "{canonical}");
                    assert_eq!(result.action, MatchAction::Reject);
                }
            }
        }
        let mut incomplete = candidate("Other Title");
        incomplete
            .source_metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("artistNames");
        assert_eq!(
            assess(
                &requested(),
                &incomplete,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Reject
        );
    }
    #[test]
    fn matching_display_cannot_repair_different_bound_performer_or_source_id() {
        let mut artist = candidate("Title");
        artist.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Different Performer"]);
        let mut wrong_id = candidate("Title");
        wrong_id.source_metadata.as_mut().unwrap()["trackId"] = json!("99");
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let wrong_artist = assess(&requested(), &artist, level, MatchStage::Resolved);
            assert_eq!(wrong_artist.action, MatchAction::Reject, "{wrong_artist:?}");
            let result = assess(&requested(), &wrong_id, level, MatchStage::Resolved);
            assert_eq!(result.action, MatchAction::Reject);
            assert!(result
                .reasons
                .contains(&MatchReason::SourceIdentityConflict));
        }
    }
    #[test]
    fn fresh_canonical_title_wins_and_full_caption_needs_provider_credit_corroboration() {
        let mut c = candidate("Title");
        c.title = "Unrelated stale display".into();
        c.source_metadata.as_mut().unwrap()["track"] = json!("Different stale track field");
        assert_eq!(
            assess(&requested(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        c.source_metadata.as_mut().unwrap()["catalogTitle"] = json!("Artist - Title");
        assert_eq!(
            assess(&requested(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        c.source_metadata.as_mut().unwrap()["catalogTitle"] = json!("Different Performer - Title");
        assert_eq!(
            assess(&requested(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn canonical_nested_version_cannot_be_hidden_by_stale_track_display() {
        let mut c = candidate("Artist - Title (Live) (Official Audio)");
        c.source_metadata.as_mut().unwrap()["track"] = json!("Artist - Title");
        assert_eq!(
            assess(&requested(), &c, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    #[test]
    fn bound_canonical_featured_caption_keeps_complete_vibe_credit_path() {
        let track = Track {
            title: "Vibe With Me".into(),
            artist: "Nipsey Hussle,Bino Rideaux,Leon Thomas".into(),
            ..Track::default()
        };
        let full =
            "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)";
        let c = Candidate {
            provider: "spotify".into(),
            title: "Unrelated stale display".into(),
            duration_seconds: Some(231.),
            source_metadata: Some(
                json!({"provider":"spotify","catalog":true,"catalogTitle":full,"track":"Unrelated stale display","artistNames":["Nipsey Hussle","Bino Rideaux","Leon Thomas"],"artistCount":3}),
            ),
            ..Candidate::default()
        };
        let result = assess(&track, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(result.dimensions.title, Comparison::Agree);
        assert_eq!(result.dimensions.artists, Comparison::Agree);
        assert!(result.evidence.as_ref().unwrap().complete_performer_credits);
        assert_eq!(result.action, MatchAction::Accept);
    }
}

#[cfg(test)]
mod stage_one_regressions {
    use super::*;
    use serde_json::json;
    fn request() -> Track {
        Track {
            title: "Song".into(),
            artist: "Artist".into(),
            duration_seconds: Some(200.0),
            ..Default::default()
        }
    }
    fn candidate() -> Candidate {
        Candidate {
            provider: "youtube".into(),
            title: "Artist - Song".into(),
            duration_seconds: Some(200.0),
            source_metadata: Some(
                json!({"provider":"youtube","track":"Song","artists":["Artist"],"artistCount":1}),
            ),
            ..Default::default()
        }
    }
    fn cells(mut test: impl FnMut(MatchLevel, MatchStage)) {
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            for stage in [MatchStage::Search, MatchStage::Resolved] {
                test(level, stage);
            }
        }
    }
    #[test]
    fn unknown_annotations_remain_uncertain_in_all_modes_and_stages() {
        for suffix in ["[432Hz]", "(Unclassified Edition)"] {
            let mut c = candidate();
            c.title = format!("Artist - Song {suffix}");
            c.source_metadata.as_mut().unwrap()["track"] = json!(c.title);
            cells(|level, stage| {
                let a = assess(&request(), &c, level, stage);
                assert_eq!(
                    a.relationship,
                    MatchRelationship::Unresolved,
                    "{suffix} {level:?} {stage:?}: {a:?}"
                );
                assert_eq!(
                    a.action,
                    if stage == MatchStage::Search {
                        MatchAction::Inspect
                    } else {
                        MatchAction::Enrich
                    }
                );
                assert!(a.reasons.contains(&MatchReason::UnresolvedAnnotation));
            });
        }
    }
    #[test]
    fn explicit_tempo_mix_is_refused_without_mix_permission() {
        let mut c = candidate();
        c.title = "Artist - Song [Slowed + Reverb]".into();
        c.source_metadata.as_mut().unwrap()["track"] = json!(c.title);
        cells(|level, stage| {
            let assessment = assess(&request(), &c, level, stage);
            assert_eq!(
                assessment.action,
                MatchAction::Reject,
                "{level:?} {stage:?}: {assessment:?}"
            );
            assert!(assessment.reasons.contains(&MatchReason::VersionConflict));
        });
    }
    #[test]
    fn caption_semantics_survive_clean_structured_fields_and_nested_presentation() {
        for caption in [
            "Artist - Song (Live)",
            "Artist - Song (Live) (Official Audio)",
            "Artist - Song (Official Audio) (Live)",
        ] {
            let mut c = candidate();
            c.title = caption.into();
            cells(|level, stage| {
                let a = assess(&request(), &c, level, stage);
                if level == MatchLevel::Flexible {
                    assert_eq!(a.action, MatchAction::Reject, "{caption} {a:?}");
                    let options = FlexibleMatchingOptions {
                        allow_other_performances: true,
                        ..Default::default()
                    };
                    let a = assess_with_options(&request(), &c, level, stage, &options);
                    assert_eq!(
                        a.relationship,
                        MatchRelationship::Alternative,
                        "{caption} {a:?}"
                    );
                    assert_eq!(
                        a.action,
                        if stage == MatchStage::Search {
                            MatchAction::Inspect
                        } else {
                            MatchAction::Accept
                        }
                    );
                } else {
                    assert_eq!(a.relationship, MatchRelationship::Unresolved);
                    assert_eq!(a.action, MatchAction::Reject);
                }
            });
        }
    }
    #[test]
    fn contradictory_explicit_versions_require_enrichment() {
        let mut t = request();
        t.title = "Song (Live)".into();
        t.version = Some("Original Mix".into());
        cells(|level, stage| {
            let a = assess(&t, &candidate(), level, stage);
            assert_eq!(a.relationship, MatchRelationship::Unresolved);
            assert_eq!(
                a.action,
                if stage == MatchStage::Search {
                    MatchAction::Inspect
                } else {
                    MatchAction::Enrich
                }
            );
            assert!(a.reasons.contains(&MatchReason::ConflictingRequestVersions));
        });
        let mut c = candidate();
        c.title = "Artist - Song (Live)".into();
        c.source_metadata.as_mut().unwrap()["version"] = json!("Original Mix");
        cells(|level, stage| {
            let a = assess(&request(), &c, level, stage);
            assert_eq!(a.relationship, MatchRelationship::Unresolved);
            assert_eq!(
                a.action,
                if stage == MatchStage::Search {
                    MatchAction::Inspect
                } else {
                    MatchAction::Enrich
                }
            );
            assert!(a
                .reasons
                .contains(&MatchReason::ConflictingCandidateVersions));
        });
    }
    #[test]
    fn unicode_marker_positions_never_slice_expanded_lowercase_bytes() {
        for text in [
            "İ feat. Å",
            "İ ft. Å",
            "İ - with Å",
            "İ w/Å",
            "\u{1f600} feat. Å",
            "İ (",
            "İ [feat. Å]",
            "ÅßΣ\u{301} ft. İ",
        ] {
            let mut t = request();
            t.title = text.into();
            let mut c = candidate();
            c.title = text.into();
            c.source_metadata.as_mut().unwrap()["track"] = json!(text);
            cells(|level, stage| {
                let _ = assess(&t, &c, level, stage);
            });
        }
    }
    #[test]
    fn empty_normalized_title_and_artist_are_uncertain_not_agreement() {
        for artist_case in [false, true] {
            let mut t = request();
            let mut c = candidate();
            if artist_case {
                t.artist = "!!!".into();
                c.source_metadata.as_mut().unwrap()["artists"] = json!(["???"]);
            } else {
                t.title = "!!!".into();
                c.title = "???".into();
                c.source_metadata.as_mut().unwrap()["track"] = json!("???");
            }
            cells(|level, stage| {
                let a = assess(&t, &c, level, stage);
                assert_eq!(a.relationship, MatchRelationship::Unresolved);
                assert_ne!(a.action, MatchAction::Accept);
                assert!(a.reasons.contains(&MatchReason::EmptyIdentityKey));
                assert_eq!(
                    if artist_case {
                        a.dimensions.artists
                    } else {
                        a.dimensions.title
                    },
                    Comparison::Unknown
                );
            });
        }
    }
    #[test]
    fn complete_extra_performer_is_a_conflict_in_every_mode_and_stage() {
        let mut c = candidate();
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Artist", "Other Singer"]);
        c.source_metadata.as_mut().unwrap()["artistCount"] = json!(2);
        let mut t = request();
        t.provenance = json!({"performerCreditsComplete":true,"manualFields":{"performerCreditsComplete":true}});
        cells(|level, stage| {
            let a = assess(&t, &c, level, stage);
            assert_eq!(a.dimensions.artists, Comparison::Conflict);
            assert_eq!(a.relationship, MatchRelationship::Unresolved);
            assert_eq!(a.action, MatchAction::Reject);
            assert!(a.reasons.contains(&MatchReason::ArtistConflict));
        });
    }

    #[test]
    fn indexed_candidate_extras_are_uncertain_without_closed_world_request_credits() {
        let mut c = candidate();
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Artist", "Other Singer"]);
        c.source_metadata.as_mut().unwrap()["artistCount"] = json!(2);
        let t = request();
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let exact = assess(&t, &c, MatchLevel::Exact, stage);
            assert_eq!(exact.dimensions.artists, Comparison::Agree);
            assert!(exact
                .evidence
                .as_ref()
                .unwrap()
                .extra_performers
                .contains(&"other singer".to_owned()));
            assert_eq!(exact.action, MatchAction::Inspect, "{stage:?}: {exact:?}");
            assert_eq!(
                exact.relationship,
                MatchRelationship::Probable,
                "{stage:?}: {exact:?}"
            );
            assert_eq!(exact.proof_strength, ProofStrength::MetadataOnly);

            let balanced = assess(&t, &c, MatchLevel::Balanced, stage);
            assert_eq!(balanced.dimensions.artists, Comparison::Agree);
            assert_eq!(balanced.relationship, MatchRelationship::Probable);

            let flexible = assess(&t, &c, MatchLevel::Flexible, stage);
            assert_eq!(flexible.relationship, MatchRelationship::Probable);
            assert_eq!(
                flexible.action,
                if stage == MatchStage::Search {
                    MatchAction::Inspect
                } else {
                    MatchAction::Accept
                }
            );
        }
    }

    #[test]
    fn manual_completeness_authority_needs_an_explicit_true_assertion() {
        let mut c = candidate();
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Artist", "Other Singer"]);
        c.source_metadata.as_mut().unwrap()["artistCount"] = json!(2);
        for provenance in [
            json!({"manualFields":{"performerCreditsComplete":true}}),
            json!({"performerCreditsComplete":false,"manualFields":{"performerCreditsComplete":true}}),
        ] {
            let t = Track {
                provenance,
                ..request()
            };
            cells(|level, stage| {
                let a = assess(&t, &c, level, stage);
                assert!(
                    !a.evidence
                        .as_ref()
                        .unwrap()
                        .requested_performer_credits_complete
                );
                assert_ne!(a.action, MatchAction::Reject);
            });
        }
    }
    #[test]
    fn canonical_nested_isrc_is_compared_and_manual_precedence_preserved() {
        let mut t = request();
        t.provenance = json!({"remoteCatalog":{"isrc":"USAAA2400001"}});
        let mut c = candidate();
        c.source_metadata.as_mut().unwrap()["isrc"] = json!("USBBB2400002");
        cells(|level, stage| {
            let a = assess(&t, &c, level, stage);
            assert_eq!(a.dimensions.isrc, Comparison::Conflict);
            assert_eq!(a.action, MatchAction::Reject);
            assert!(a.reasons.contains(&MatchReason::IsrcConflict));
        });
        t.isrc = Some("USBBB2400002".into());
        t.provenance["manualFields"] = json!({"isrc":true});
        cells(|level, stage| {
            let a = assess(&t, &c, level, stage);
            assert_eq!(a.dimensions.isrc, Comparison::Agree);
            assert!(!a
                .reasons
                .contains(&MatchReason::ConflictingRequestIdentifiers));
        });
    }
    #[test]
    fn balanced_catalog_eligibility_is_inherited_by_flexible() {
        let mut t = request();
        t.duration_seconds = None;
        let mut c = candidate();
        c.provider = "monochrome".into();
        c.id = "monochrome:1".into();
        c.duration_seconds = None;
        c.source_metadata = Some(
            json!({"provider":"monochrome","trackId":"1","catalogTitle":"Song","artists":["Artist"],"artistCount":1,"performerCreditsComplete":true}),
        );
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let balanced = assess(&t, &c, MatchLevel::Balanced, stage);
            let flexible = assess(&t, &c, MatchLevel::Flexible, stage);
            assert_eq!(balanced.relationship, MatchRelationship::Probable);
            assert_eq!(flexible.relationship, balanced.relationship);
            assert_eq!(flexible.action, balanced.action);
            assert_eq!(
                flexible.action,
                if stage == MatchStage::Search {
                    MatchAction::Inspect
                } else {
                    MatchAction::Accept
                }
            );
        }
    }
}

#[cfg(test)]
mod prepared_evidence_tests {
    use super::*;
    use serde_json::json;
    fn input() -> (Track, Candidate) {
        (
            Track {
                title: "Song".into(),
                artist: "Artist".into(),
                duration_seconds: Some(200.),
                ..Default::default()
            },
            Candidate {
                provider: "youtube".into(),
                id: "video1".into(),
                title: "Artist - Song".into(),
                duration_seconds: Some(207.),
                source_metadata: Some(
                    json!({"provider":"youtube","track":"Song","catalogTitle":"Song","artists":["Artist"],"artistCount":1}),
                ),
                ..Default::default()
            },
        )
    }
    #[test]
    fn prepared_facts_are_mode_independent_and_policy_does_not_reparse() {
        let (t, c) = input();
        let prepared = prepare(&t, &c);
        let facts = compare_prepared(&prepared);
        assert_eq!(facts.runtime_delta_seconds, Some(7.));
        assert_eq!(facts.dimensions.runtime, Comparison::Conflict);
        assert_eq!(facts.proof_strength, ProofStrength::MetadataOnly);
        let before = assess_prepared(&prepared, MatchLevel::Flexible, MatchStage::Resolved);
        // Reusing a prepared input only applies policy to the same observed facts.
        assert_eq!(
            before,
            assess_prepared(&prepared, MatchLevel::Flexible, MatchStage::Resolved)
        );
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let strict = assess_prepared(&prepared, MatchLevel::Exact, stage);
            let flexible = assess_prepared(&prepared, MatchLevel::Flexible, stage);
            assert_eq!(strict.dimensions, flexible.dimensions);
            assert_eq!(strict.proof_strength, flexible.proof_strength);
            assert_eq!(strict.action, MatchAction::Reject);
            assert_eq!(
                flexible.action,
                if stage == MatchStage::Search {
                    MatchAction::Inspect
                } else {
                    MatchAction::Accept
                }
            );
        }
    }
    #[test]
    fn repeated_provider_fields_share_source_group_and_do_not_upgrade_proof() {
        let (t, mut c) = input();
        c.duration_seconds = Some(200.);
        let prepared = prepare(&t, &c);
        let groups = prepared
            .candidate
            .texts
            .iter()
            .map(|text| text.origin.source_group.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(groups.len(), 1);
        assert_eq!(
            assess_prepared(&prepared, MatchLevel::Exact, MatchStage::Resolved).relationship,
            MatchRelationship::Exact
        );
        assert_eq!(
            compare_prepared(&prepared).proof_strength,
            ProofStrength::MetadataOnly
        );
        c.provider = "monochrome".into();
        c.id = "monochrome:123".into();
        c.source_metadata = Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song","artists":["Artist"],"artistCount":1}),
        );
        assert_eq!(
            compare_prepared(&prepare(&t, &c)).proof_strength,
            ProofStrength::MetadataOnly
        );
    }
    #[test]
    fn shared_identifiers_upgrade_proof_but_never_override_conflicting_fields() {
        let (mut t, mut c) = input();
        t.isrc = Some("USAAA2400001".into());
        c.source_metadata.as_mut().unwrap()["isrc"] = json!("USAAA2400001");
        let prepared = prepare(&t, &c);
        assert_eq!(
            compare_prepared(&prepared).proof_strength,
            ProofStrength::RecordingIdentifier
        );
        c.source_metadata.as_mut().unwrap()["artists"] = json!(["Wrong Artist"]);
        let a = assess(&t, &c, MatchLevel::Balanced, MatchStage::Resolved);
        assert_eq!(a.action, MatchAction::Reject);
        assert_ne!(a.proof_strength, ProofStrength::AudioFingerprint);
    }
    #[test]
    fn nested_semantic_conflicts_and_unknown_inner_annotations_survive_presentation() {
        let (t, mut c) = input();
        c.duration_seconds = Some(200.);
        for caption in [
            "Artist - Song (Live) (Original Mix) (Official Audio)",
            "Artist - Song (Original Mix) (Live)",
        ] {
            c.title = caption.into();
            let prepared = prepare(&t, &c);
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let a = assess_prepared(&prepared, level, MatchStage::Resolved);
                assert_eq!(a.action, MatchAction::Enrich, "{caption} {a:?}");
                assert!(a
                    .reasons
                    .contains(&MatchReason::ConflictingCandidateVersions));
            }
        }
        c.title = "Artist - Song [432Hz] (Live) (Official Audio)".into();
        let prepared = prepare(&t, &c);
        let facts = compare_prepared(&prepared);
        assert_eq!(facts.unresolved_annotations.len(), 1);
        assert_eq!(facts.unresolved_annotations[0].origin.field, "title");
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_ne!(
                assess_prepared(&prepared, level, MatchStage::Resolved).action,
                MatchAction::Accept
            );
        }
    }
    #[test]
    fn manual_identifier_observations_preserve_but_deactivate_overridden_values() {
        let (mut t, c) = input();
        t.isrc = Some("USAAA2400001".into());
        t.provenance =
            json!({"manualFields":{"isrc":true},"remoteCatalog":{"isrc":"USBBB2400002"}});
        let prepared = prepare(&t, &c);
        assert_eq!(prepared.request.identifiers.len(), 2);
        assert!(prepared
            .request
            .identifiers
            .iter()
            .any(|id| id.value == "USBBB2400002" && !id.active));
        assert!(!compare_prepared(&prepared).identifier_conflict);
    }
}

#[cfg(test)]
#[path = "recording_match_pattern_tests.rs"]
mod recording_match_pattern_tests;

#[cfg(test)]
#[path = "recording_match_parser_regressions.rs"]
mod parser_regressions;
