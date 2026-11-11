//! Bounded retrieval alternatives derived from recording identity evidence.
//!
//! A SearchPlan only widens provider queries. It never changes the request
//! Track or the strict identity assessment applied to returned candidates.
use crate::models::Track;
use serde::{Deserialize, Serialize};

pub(crate) const MAX_SEARCH_QUERIES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SearchQueryReason {
    CanonicalIdentity,
    PrimaryPerformer,
    VerifiedArtistAlternative,
    VerifiedTitleAlternative,
    SoundtrackContextFallback,
    ReleaseContextFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchQuery {
    pub(crate) text: String,
    pub(crate) artist: String,
    pub(crate) title: String,
    pub(crate) reason: SearchQueryReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchPlan {
    pub(crate) queries: Vec<SearchQuery>,
}

fn push_query(
    queries: &mut Vec<SearchQuery>,
    artist: &str,
    title: &str,
    reason: SearchQueryReason,
) {
    let artist = artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() || queries.len() >= MAX_SEARCH_QUERIES {
        return;
    }
    let text = format!("{artist} {title}");
    let key = crate::matching::normalize(&text);
    if key.is_empty()
        || queries
            .iter()
            .any(|query| crate::matching::normalize(&query.text) == key)
    {
        return;
    }
    queries.push(SearchQuery {
        text,
        artist: artist.to_owned(),
        title: title.to_owned(),
        reason,
    });
}

fn placeholder_artist(value: &str) -> bool {
    matches!(
        crate::matching::normalize(value).as_str(),
        "" | "unknown" | "unknown artist" | "unknownartist" | "n a" | "na" | "not available"
    )
}

fn primary_performer_credit(track: &Track) -> Option<String> {
    if track.provenance["manualFields"]["artist"]
        .as_bool()
        .unwrap_or(false)
    {
        return None;
    }
    let identity = crate::recording_identity::RecordingIdentity::from_track(track);
    let primary = identity
        .credits
        .iter()
        .find(|credit| crate::recording_identity::is_primary_performer_role(&credit.role))
        .map(|credit| credit.name.clone())
        .filter(|name| !placeholder_artist(name));
    primary.or_else(|| {
        crate::legacy_discogs_credit::legacy_discogs_roleless_primary_performer_names(track)
            .and_then(|names| names.into_iter().next())
    })
}

impl SearchPlan {
    /// Build a canonical query and one alternate within the fixed provider
    /// budget. Combine source-verified title and artist spellings in the
    /// alternate slot; otherwise use an artist alias or role-aware lead credit.
    pub(crate) fn for_track(track: &Track, verified_artist_alternatives: &[String]) -> Self {
        Self::for_track_with_release_policy(track, verified_artist_alternatives, false)
    }

    /// As above, optionally add a retrieval-only fallback for a recognized
    /// restoration/remaster label. The original Track and match evidence stay
    /// unchanged; callers should enable it only when Flexible other-release
    /// matching is enabled.
    pub(crate) fn for_track_with_release_policy(
        track: &Track,
        verified_artist_alternatives: &[String],
        allow_other_releases: bool,
    ) -> Self {
        Self::plan(
            track,
            verified_artist_alternatives,
            &[],
            allow_other_releases,
        )
    }

    /// As above, with caller-supplied title renderings backed by retained
    /// source evidence. Each alternative shares the same global query cap.
    pub(crate) fn for_track_with_alternatives(
        track: &Track,
        verified_artist_alternatives: &[String],
        verified_title_alternatives: &[String],
    ) -> Self {
        Self::plan(
            track,
            verified_artist_alternatives,
            verified_title_alternatives,
            false,
        )
    }

    fn plan(
        track: &Track,
        verified_artist_alternatives: &[String],
        verified_title_alternatives: &[String],
        allow_other_releases: bool,
    ) -> Self {
        let mut queries = Vec::with_capacity(MAX_SEARCH_QUERIES);
        let manual_artist = track.provenance["manualFields"]["artist"]
            .as_bool()
            .unwrap_or(false);
        let verified_artist_alternatives = if manual_artist {
            &[]
        } else {
            verified_artist_alternatives
        };
        let rendered_title = search_title(track);
        let lead = primary_performer_credit(track);
        let canonical_artist = if placeholder_artist(&track.artist) {
            (!track.provenance["manualFields"]["artist"]
                .as_bool()
                .unwrap_or(false))
            .then_some(lead.clone())
            .flatten()
        } else {
            Some(track.artist.clone())
        };
        let canonical_reason = if canonical_artist
            .as_deref()
            .is_some_and(|artist| artist != track.artist)
        {
            SearchQueryReason::PrimaryPerformer
        } else {
            SearchQueryReason::CanonicalIdentity
        };
        if let Some(artist) = canonical_artist.as_deref() {
            push_query(&mut queries, artist, &rendered_title, canonical_reason);
        }

        // One alternate can carry both verified spellings. Otherwise an
        // artist alias consumes the budget before any title alias is tried.
        // Equivalent punctuation variants do not displace a useful fallback.
        if track.provenance["manualFields"]["title"] != true {
            let title_alternative = verified_title_alternatives
                .iter()
                .take(2)
                .map(|title| search_title_with_alternative(track, title))
                .find(|title| {
                    !title.is_empty()
                        && crate::matching::normalize(title)
                            != crate::matching::normalize(&rendered_title)
                });
            if let (Some(title), Some(artist)) = (
                title_alternative,
                verified_artist_alternatives
                    .first()
                    .map(String::as_str)
                    .or(canonical_artist.as_deref())
                    .or(lead.as_deref()),
            ) {
                push_query(
                    &mut queries,
                    artist,
                    &title,
                    SearchQueryReason::VerifiedTitleAlternative,
                );
            }
        }

        let release_fallback = allow_other_releases
            .then(|| search_title_without_release(track))
            .flatten();
        if let Some(fallback_title) = release_fallback {
            let artist = lead
                .as_deref()
                .or_else(|| verified_artist_alternatives.first().map(String::as_str))
                .or(canonical_artist.as_deref());
            if let Some(artist) = artist {
                push_query(
                    &mut queries,
                    artist,
                    &fallback_title,
                    SearchQueryReason::ReleaseContextFallback,
                );
            }
        } else {
            for alternative in verified_artist_alternatives.iter().take(2) {
                push_query(
                    &mut queries,
                    alternative,
                    &rendered_title,
                    SearchQueryReason::VerifiedArtistAlternative,
                );
                if queries.len() == MAX_SEARCH_QUERIES {
                    break;
                }
            }
        }

        if queries.len() < MAX_SEARCH_QUERIES {
            let fallback_title = search_title_without_soundtrack(track);
            let reason = if fallback_title != rendered_title {
                SearchQueryReason::SoundtrackContextFallback
            } else if lead.is_some() {
                SearchQueryReason::PrimaryPerformer
            } else {
                SearchQueryReason::CanonicalIdentity
            };
            if let Some(artist) = lead.as_deref().or(canonical_artist.as_deref()) {
                push_query(&mut queries, artist, &fallback_title, reason);
            }
        }
        Self { queries }
    }
}

fn search_title(track: &Track) -> String {
    render_search_title(&track.title, track.version.as_deref())
}

fn search_title_with_alternative(track: &Track, alternative: &str) -> String {
    render_search_title(alternative, track.version.as_deref())
}

fn render_search_title(title: &str, version: Option<&str>) -> String {
    let title = title.trim();
    if title.is_empty() {
        return String::new();
    }
    let Some(version) = version.map(str::trim).filter(|value| !value.is_empty()) else {
        return title.to_owned();
    };
    let version = crate::matching::strip_trailing_display_runtime(version);
    let key = |value: &str| {
        crate::recording_identity::canonical_version_key(
            &crate::matching::normalize_version_for_title(title, value),
        )
    };
    let parsed = crate::recording_identity::parse_recording_text(title);
    let requested_key = key(version);
    if parsed
        .semantic_versions()
        .any(|label| key(label) == requested_key)
    {
        title.to_owned()
    } else {
        format!("{title} {version}")
    }
}

fn release_label(value: &str) -> bool {
    crate::recording_identity::is_release_version(value)
}

/// Return a retrieval-only title with recognized restoration/remaster labels
/// removed. Live, remix, soundtrack, and unresolved annotations remain intact.
fn search_title_without_release(track: &Track) -> Option<String> {
    let parsed = crate::recording_identity::parse_recording_text(track.title.trim());
    if parsed.parsing_incomplete {
        return None;
    }
    let mut spans = parsed
        .annotations
        .iter()
        .filter(|annotation| {
            annotation.kind == crate::recording_identity::AnnotationKind::SemanticVersion
                && release_label(&annotation.value)
        })
        .map(|annotation| annotation.span.clone())
        .collect::<Vec<_>>();
    let separate_version = track
        .version
        .as_deref()
        .map(str::trim)
        .filter(|version| !version.is_empty());
    let parsed_title_version = crate::recording_identity::parse_title(track.title.trim())
        .version
        .unwrap_or_default();
    let separate_release = separate_version.is_some_and(release_label)
        && crate::matching::normalize(&parsed_title_version)
            != crate::matching::normalize(separate_version.unwrap_or_default());
    if spans.is_empty() && !separate_release {
        return None;
    }
    spans.sort_by_key(|span| std::cmp::Reverse(span.start));
    let mut title = track.title.trim().to_owned();
    for span in spans {
        if span.start <= span.end
            && span.end <= title.len()
            && title.is_char_boundary(span.start)
            && title.is_char_boundary(span.end)
        {
            title.replace_range(span.start..span.end, "");
        }
    }
    title = title.trim().to_owned();
    let keep_separate_version = separate_version.filter(|version| !release_label(version));
    Some(render_search_title(&title, keep_separate_version))
}

fn search_title_without_soundtrack(track: &Track) -> String {
    let rendered = search_title(track);
    let parsed = crate::recording_identity::parse_recording_text(track.title.trim());
    let mut spans = parsed
        .annotations
        .iter()
        .filter(|annotation| {
            annotation.kind == crate::recording_identity::AnnotationKind::Soundtrack
        })
        .map(|annotation| annotation.span.clone())
        .collect::<Vec<_>>();
    spans.sort_by_key(|span| std::cmp::Reverse(span.start));
    if spans.is_empty() {
        return rendered;
    }
    // Spans index the original Track.title. Remove only recognized soundtrack
    // annotations, leaving semantic-version and unknown evidence intact.
    let mut title = track.title.trim().to_owned();
    for span in spans {
        let start = span.start;
        let end = span.end;
        if start <= end
            && end <= title.len()
            && title.is_char_boundary(start)
            && title.is_char_boundary(end)
        {
            title.replace_range(start..end, "");
        }
    }
    let title = title.trim().to_owned();
    render_search_title(&title, track.version.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bounded_queries_keep_soundtrack_context_and_only_use_structured_primary_credits() {
        let track = Track {
            title: "Halka Halka (From 'Fanney Khan')".into(),
            artist: "Sunidhi Chauhan, Vishal-Shekhar, Amit Trivedi".into(),
            provenance: json!({"artistCredits":[
                {"name":"Sunidhi Chauhan","role":"artist"},
                {"name":"Vishal-Shekhar","role":"performer"},
                {"name":"Amit Trivedi","role":"composer"}
            ]}),
            ..Track::default()
        };
        let alternatives = vec!["Sunidhi Chouhan".to_owned(), "Ignored Extra".to_owned()];
        let plan = SearchPlan::for_track(&track, &alternatives);
        assert_eq!(plan.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(plan.queries[0].reason, SearchQueryReason::CanonicalIdentity);
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::VerifiedArtistAlternative
        );
        assert!(plan
            .queries
            .iter()
            .all(|query| query.text.contains("Halka Halka (From 'Fanney Khan')")));
        assert!(plan.queries[1].text.starts_with("Sunidhi Chouhan "));
        assert!(!plan.queries[1].text.contains("Amit Trivedi"));
        assert!(!plan
            .queries
            .iter()
            .any(|query| query.text.contains("Ignored Extra")));
    }

    #[test]
    fn no_unverified_artist_split_is_invented_for_composite_display_text() {
        let track = Track {
            title: "Song (Soundtrack Version)".into(),
            artist: "Artist A, Artist B".into(),
            ..Track::default()
        };
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries.len(), 1);
        assert_eq!(
            plan.queries[0].text,
            "Artist A, Artist B Song (Soundtrack Version)"
        );
    }

    #[test]
    fn retrieval_query_drops_only_trailing_version_runtime_and_keeps_mix_name() {
        let track = Track {
            title: "Lady Soul".into(),
            artist: "Envie, Oscar P".into(),
            version: Some("Oscar P Dub (6:07)".into()),
            duration_seconds: Some(367.0),
            ..Track::default()
        };
        let original_version = track.version.clone();
        let plan = SearchPlan::for_track(&track, &[]);

        assert_eq!(plan.queries[0].text, "Envie, Oscar P Lady Soul Oscar P Dub");
        assert_eq!(track.version, original_version);
        assert_eq!(track.version.as_deref(), Some("Oscar P Dub (6:07)"));
    }

    #[test]
    fn retrieval_query_keeps_meaningful_version_and_nonterminal_clock_text() {
        for version in [
            "Extended Mix",
            "Remix 2",
            "Extended Mix (6:60)",
            "Dub (6:07) Live",
            "Main Version (6:07) (Live)",
        ] {
            let track = Track {
                title: "Song".into(),
                artist: "Artist".into(),
                version: Some(version.into()),
                ..Track::default()
            };
            let query = &SearchPlan::for_track(&track, &[]).queries[0].text;
            assert!(query.ends_with(version), "{version}: {query}");
        }
    }

    #[test]
    fn primary_performer_query_uses_only_explicit_performer_roles() {
        let track = Track {
            title: "Song".into(),
            artist: "Lead, Guest, Composer".into(),
            provenance: json!({"artistCredits":[
                {"name":"Lead","role":"MAIN_ARTIST"},
                {"name":"Guest","role":"featured"},
                {"name":"Composer","role":"composer"},
                {"name":"Lyricist","role":"lyricist"}
            ]}),
            ..Track::default()
        };
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries.len(), 2);
        assert_eq!(plan.queries[1].reason, SearchQueryReason::PrimaryPerformer);
        assert_eq!(plan.queries[1].text, "Lead Song");
    }

    #[test]
    fn placeholder_artist_uses_a_structured_primary_performer_as_the_first_query() {
        let track = Track {
            id: "catalog:discogs:fixture".into(),
            provider: "catalog".into(),
            provider_id: "discogs:fixture".into(),
            title: "April The 14th Part 1".into(),
            artist: "Unknown artist".into(),
            duration_seconds: Some(221.0),
            version: Some("Studio Recording".into()),
            provenance: json!({
                "catalogProvider":"discogs",
                "artistCredits":[
                    {"name":"Gillian Welch","role":"","artist":{"id":"255403","provider":"discogs"}},
                    {"name":"David Rawlings","role":"Written By","artist":{"id":"0","provider":"discogs"}}
                ]
            }),
            ..Track::default()
        };
        let original = track.clone();
        let plan = SearchPlan::for_track(&track, &["Gillian Welch verified spelling".into()]);

        assert_eq!(plan.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(plan.queries[0].reason, SearchQueryReason::PrimaryPerformer);
        assert_eq!(
            plan.queries[0].text,
            "Gillian Welch April The 14th Part 1 Studio Recording"
        );
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::VerifiedArtistAlternative
        );
        assert_eq!(
            plan.queries[1].text,
            "Gillian Welch verified spelling April The 14th Part 1 Studio Recording"
        );
        assert!(plan
            .queries
            .iter()
            .all(|query| !query.text.contains("Unknown artist")
                && !query.text.contains("David Rawlings")));

        // The same structured primary performer used to build the query is
        // still available to final source acceptance for an unhydrated queue
        // row. A correct recording with matching duration and version passes.
        let correct_candidate = crate::models::Candidate {
            provider: "youtube".into(),
            id: "youtube:correct-fixture".into(),
            title: "April The 14th Part 1".into(),
            duration_seconds: Some(221.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"April The 14th Part 1",
                "catalogTitle":"April The 14th Part 1",
                "artistNames":["Gillian Welch"],
                "version":"Studio Recording"
            })),
            ..crate::models::Candidate::default()
        };
        let correct_assessment = crate::recording_match::assess(
            &track,
            &correct_candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_eq!(
            correct_assessment.dimensions.artists,
            crate::recording_match::Comparison::Agree
        );
        assert_eq!(
            correct_assessment.action,
            crate::recording_match::MatchAction::Accept,
            "correct structured primary performer should validate: {correct_assessment:?}"
        );

        let mut missing_display_artist = track.clone();
        missing_display_artist.artist.clear();
        let missing_artist_plan = SearchPlan::for_track(&missing_display_artist, &[]);
        assert_eq!(missing_artist_plan.queries.len(), 1);
        assert_eq!(
            missing_artist_plan.queries[0].text,
            "Gillian Welch April The 14th Part 1 Studio Recording"
        );

        // Retrieval uses the primary credit, while final source acceptance
        // still evaluates the unchanged request and rejects a different act.
        let wrong_artist = crate::models::Candidate {
            provider: "youtube".into(),
            id: "youtube:wrong-artist-fixture".into(),
            title: "April The 14th Part 1".into(),
            duration_seconds: Some(221.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"April The 14th Part 1",
                "catalogTitle":"April The 14th Part 1",
                "artistNames":["Different Artist"],
                "version":"Studio Recording"
            })),
            ..crate::models::Candidate::default()
        };
        let assessment = crate::recording_match::assess(
            &track,
            &wrong_artist,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_eq!(
            assessment.dimensions.artists,
            crate::recording_match::Comparison::Conflict
        );
        assert_eq!(
            assessment.action,
            crate::recording_match::MatchAction::Reject
        );

        let wrong_version = crate::models::Candidate {
            source_metadata: Some(json!({
                "provider":"youtube",
                "track":"April The 14th Part 1",
                "catalogTitle":"April The 14th Part 1",
                "artistNames":["Gillian Welch"],
                "version":"Live"
            })),
            ..correct_candidate.clone()
        };
        let assessment = crate::recording_match::assess(
            &track,
            &wrong_version,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_eq!(
            assessment.dimensions.version,
            crate::recording_match::Comparison::Conflict
        );
        assert_eq!(
            assessment.action,
            crate::recording_match::MatchAction::Reject
        );

        assert_eq!(track, original);
    }

    #[test]
    fn absent_artist_search_skips_placeholders_and_composer_only_credits() {
        for artist in ["", "Unknown artist", "Unknownartist", "N/A"] {
            let track = Track {
                title: "April The 14th Part 1".into(),
                artist: artist.into(),
                provenance: json!({
                    "artistCredits":[{"name":"Gillian Welch","role":"writer"}]
                }),
                ..Track::default()
            };
            let plan = SearchPlan::for_track(&track, &[]);
            assert!(plan.queries.is_empty(), "{artist:?}: {plan:?}");
            assert!(!plan
                .queries
                .iter()
                .any(|query| query.text.contains("April The 14th Part 1")));
        }

        let missing = Track {
            title: "April The 14th Part 1".into(),
            artist: "Unknown artist".into(),
            ..Track::default()
        };
        let no_title_only = SearchPlan::for_track_with_alternatives(
            &missing,
            &[],
            &["Verified title spelling".into()],
        );
        assert!(no_title_only.queries.is_empty());

        let invalid_discogs_credit = Track {
            provider: "catalog".into(),
            title: "April The 14th Part 1".into(),
            artist: "Unknown artist".into(),
            provenance: json!({
                "catalogProvider":"discogs",
                "artistCredits":[{"name":"Unknown Artist Alias","role":"","artist":{"id":"0","provider":"discogs"}}]
            }),
            ..Track::default()
        };
        assert!(SearchPlan::for_track(&invalid_discogs_credit, &[])
            .queries
            .is_empty());

        let manually_overridden = Track {
            provider: "catalog".into(),
            title: "April The 14th Part 1".into(),
            artist: "Unknown artist".into(),
            provenance: json!({
                "catalogProvider":"discogs",
                "manualFields":{"artist":true},
                "artistCredits":[{"name":"Gillian Welch","role":"","artist":{"id":"255403","provider":"discogs"}}]
            }),
            ..Track::default()
        };
        assert!(SearchPlan::for_track(&manually_overridden, &[])
            .queries
            .is_empty());
    }

    #[test]
    fn separate_version_field_is_retained_in_provider_search_text() {
        let track = Track {
            title: "Song (From 'Film')".into(),
            version: Some("Live at Studio A".into()),
            artist: "Artist".into(),
            ..Track::default()
        };
        let plan = SearchPlan::for_track(&track, &[]);
        assert!(plan.queries[0]
            .text
            .contains("Song (From 'Film') Live at Studio A"));
    }

    #[test]
    fn release_fallback_is_opt_in_uses_verified_alias_and_preserves_track_version() {
        let track = Track {
            title: "Swing 42 (Restored Edition '25)".into(),
            version: Some("Restored Edition '25".into()),
            artist: "Django Reinhardt & Quintette du Hot Club de France".into(),
            provenance: json!({"artistCredits":[
                {"name":"Django Reinhardt","role":"artist credit","joinPhrase":" & "},
                {"name":"Quintette du Hot Club de France","role":"artist credit","joinPhrase":""}
            ]}),
            ..Track::default()
        };
        let unchanged = track.clone();
        let aliases = vec!["Django Reinhardt Verified Alias".to_owned()];
        let strict = SearchPlan::for_track_with_release_policy(&track, &aliases, false);
        assert_eq!(
            strict.queries[1].reason,
            SearchQueryReason::VerifiedArtistAlternative
        );
        assert!(strict.queries[1].text.contains("Restored Edition '25"));

        let flexible = SearchPlan::for_track_with_release_policy(&track, &aliases, true);
        assert_eq!(flexible.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(
            flexible.queries[0].reason,
            SearchQueryReason::CanonicalIdentity
        );
        assert_eq!(
            flexible.queries[1].reason,
            SearchQueryReason::ReleaseContextFallback
        );
        assert_eq!(
            flexible.queries[1].text,
            "Django Reinhardt Verified Alias Swing 42"
        );
        assert_eq!(track, unchanged);
    }

    #[test]
    fn release_fallback_preserves_request_identity_while_querying_structured_primary() {
        let track = Track {
            title: "Minor Swing (Restored Edition '25)".into(),
            version: Some("Restored Edition '25".into()),
            artist: "Django Reinhardt & Stéphane Grappelli".into(),
            provenance: json!({"artistCredits":[
                {"name":"Django Reinhardt","role":"artist credit","joinPhrase":" & "},
                {"name":"Stéphane Grappelli","role":"artist credit","joinPhrase":""}
            ]}),
            ..Track::default()
        };
        let unchanged = track.clone();
        let plan = SearchPlan::for_track_with_release_policy(&track, &[], true);

        assert_eq!(plan.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(
            plan.queries[0].text,
            "Django Reinhardt & Stéphane Grappelli Minor Swing (Restored Edition '25)"
        );
        assert_eq!(plan.queries[1].text, "Django Reinhardt Minor Swing");
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::ReleaseContextFallback
        );
        assert_eq!(track, unchanged);
    }

    #[test]
    fn release_fallback_keeps_single_artist_and_manual_artist_precedence() {
        let single = Track {
            title: "Minor Swing (Restored Edition '25)".into(),
            artist: "Django Reinhardt".into(),
            ..Track::default()
        };
        let plan = SearchPlan::for_track_with_release_policy(&single, &[], true);
        assert_eq!(plan.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(plan.queries[1].text, "Django Reinhardt Minor Swing");

        let manual = Track {
            title: "Minor Swing (Restored Edition '25)".into(),
            artist: "Manual saved artist".into(),
            provenance: json!({
                "manualFields":{"artist":true},
                "artistCredits":[{"name":"Django Reinhardt","role":"artist credit"}]
            }),
            ..Track::default()
        };
        let ordinary_manual = SearchPlan::for_track(&manual, &["Verified alias".into()]);
        assert_eq!(ordinary_manual.queries.len(), 1);
        assert_eq!(
            ordinary_manual.queries[0].text,
            "Manual saved artist Minor Swing (Restored Edition '25)"
        );
        let plan =
            SearchPlan::for_track_with_release_policy(&manual, &["Verified alias".into()], true);
        assert_eq!(
            plan.queries[0].text,
            "Manual saved artist Minor Swing (Restored Edition '25)"
        );
        assert_eq!(plan.queries[1].text, "Manual saved artist Minor Swing");

        let manual_placeholder = Track {
            title: "Minor Swing (Restored Edition '25)".into(),
            artist: "Unknown artist".into(),
            provenance: json!({
                "manualFields":{"artist":true},
                "artistCredits":[{"name":"Django Reinhardt","role":"artist credit"}]
            }),
            ..Track::default()
        };
        assert!(SearchPlan::for_track(&manual_placeholder, &[])
            .queries
            .is_empty());
        assert!(
            SearchPlan::for_track_with_release_policy(&manual_placeholder, &[], true)
                .queries
                .is_empty()
        );
    }

    #[test]
    fn flexible_release_fallback_uses_ordered_structured_primary_credit() {
        let track = Track {
            title: "Swing 42 (Restored Edition '25)".into(),
            version: Some("Restored Edition '25".into()),
            artist: "Django Reinhardt & Quintette du Hot Club de France".into(),
            provenance: json!({"artistCredits":[
                {"name":"Django Reinhardt","role":"artist credit","joinPhrase":" & ",
                 "artist":{"provider":"musicbrainz","id":"650bf385-6f6d-4992-a3b9-779d144920a4"}},
                {"name":"Quintette du Hot Club de France","role":"artist credit","joinPhrase":"",
                 "artist":{"provider":"musicbrainz","id":"ee55e4e8-807d-49b1-8470-d1c0898ed7cb"}}
            ]}),
            ..Track::default()
        };
        let original = track.clone();

        let flexible = SearchPlan::for_track_with_release_policy(&track, &[], true);
        assert_eq!(flexible.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(
            flexible.queries[0].text,
            "Django Reinhardt & Quintette du Hot Club de France Swing 42 (Restored Edition '25)"
        );
        assert_eq!(
            flexible.queries[1].reason,
            SearchQueryReason::ReleaseContextFallback
        );
        assert_eq!(flexible.queries[1].artist, "Django Reinhardt");
        assert_eq!(flexible.queries[1].title, "Swing 42");
        assert_eq!(flexible.queries[1].text, "Django Reinhardt Swing 42");
        // A reordered complete alias must not displace the useful lead-artist
        // query in the only alternate slot.
        let reordered = SearchPlan::for_track_with_release_policy(
            &track,
            &["Quintette du Hot Club de France, Django Reinhardt".into()],
            true,
        );
        assert_eq!(reordered.queries[1].text, "Django Reinhardt Swing 42");

        let strict = SearchPlan::for_track_with_release_policy(&track, &[], false);
        assert_eq!(strict.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(
            strict.queries[1].reason,
            SearchQueryReason::PrimaryPerformer
        );
        assert_eq!(
            strict.queries[1].text,
            "Django Reinhardt Swing 42 (Restored Edition '25)"
        );

        let live_remix = Track {
            title: "Swing 42 (Live Remix) (Restored Edition '25)".into(),
            ..track.clone()
        };
        let flexible_live_remix = SearchPlan::for_track_with_release_policy(&live_remix, &[], true);
        assert_eq!(
            flexible_live_remix.queries[1].reason,
            SearchQueryReason::ReleaseContextFallback
        );
        assert_eq!(flexible_live_remix.queries[1].artist, "Django Reinhardt");
        assert_eq!(
            flexible_live_remix.queries[1].title,
            "Swing 42 (Live Remix)"
        );
        assert_eq!(track, original);
    }

    #[test]
    fn manual_artist_edit_is_not_replaced_by_structured_primary_credit() {
        let track = Track {
            title: "Swing 42 (Restored Edition '25)".into(),
            artist: "User Chosen Artist".into(),
            provenance: json!({
                "manualFields":{"artist":true},
                "artistCredits":[
                    {"name":"Django Reinhardt","role":"artist credit"},
                    {"name":"Quintette du Hot Club de France","role":"artist credit"}
                ]
            }),
            ..Track::default()
        };
        let plan = SearchPlan::for_track_with_release_policy(&track, &[], true);
        assert_eq!(plan.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(plan.queries[0].artist, "User Chosen Artist");
        assert_eq!(plan.queries[1].artist, "User Chosen Artist");
        assert_eq!(plan.queries[1].title, "Swing 42");
        assert!(!plan.queries[1].text.contains("Django Reinhardt"));
    }

    #[test]
    fn release_fallback_keeps_live_remix_and_unknown_annotations() {
        let track = Track {
            title: "Swing 42 (Live) (Studio Session) (Restored Edition '25)".into(),
            artist: "Django Reinhardt".into(),
            ..Track::default()
        };
        let plan = SearchPlan::for_track_with_release_policy(&track, &[], true);
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::ReleaseContextFallback
        );
        assert!(plan.queries[1].text.contains("Studio Session"));
        assert!(plan.queries[1].text.contains("Live"));
        assert!(!plan.queries[1].text.contains("Restored Edition '25"));

        let recognized = Track {
            title: "Swing 42 (Live Remix) (Restored Edition '25)".into(),
            artist: "Django Reinhardt".into(),
            ..Track::default()
        };
        let plan = SearchPlan::for_track_with_release_policy(&recognized, &[], true);
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::ReleaseContextFallback
        );
        assert!(plan.queries[1].text.contains("Live Remix"));
        assert!(!plan.queries[1].text.contains("Restored Edition"));
    }

    #[test]
    fn query_budget_holds_when_both_artist_and_title_alternatives_are_supplied() {
        let track = Track {
            title: "Song (From 'Film')".into(),
            artist: "Artist".into(),
            ..Track::default()
        };
        let artists = vec!["Verified Alias".to_owned()];
        let titles = vec!["Song (From ‘Film’)".to_owned()];
        let plan = SearchPlan::for_track_with_alternatives(&track, &artists, &titles);
        assert_eq!(plan.queries.len(), MAX_SEARCH_QUERIES);
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::VerifiedArtistAlternative
        );
    }

    #[test]
    fn soundtrack_search_fallback_removes_only_recognized_work_context() {
        let track = Track {
            title: "Song (From ‘Film’) (Live)".into(),
            artist: "Lead & Guest".into(),
            provenance: json!({"artistCredits":[
                {"name":"Lead","role":"artist"},
                {"name":"Guest","role":"featured"}
            ]}),
            ..Track::default()
        };
        let original = track.title.clone();
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries.len(), 2);
        assert!(plan.queries[0].text.contains("From ‘Film’"));
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::SoundtrackContextFallback
        );
        assert!(!plan.queries[1].text.contains("From ‘Film’"));
        assert!(plan.queries[1].text.contains("Live"));
        assert_eq!(track.title, original);
    }

    #[test]
    fn soundtrack_fallback_uses_canonical_artist_without_structured_lead_and_keeps_version() {
        let track = Track {
            title: "Song (From 'Film')".into(),
            version: Some("Live".into()),
            artist: "Artist A, Artist B".into(),
            ..Track::default()
        };
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries.len(), 2);
        assert_eq!(
            plan.queries[1].reason,
            SearchQueryReason::SoundtrackContextFallback
        );
        assert_eq!(plan.queries[1].text, "Artist A, Artist B Song Live");
        assert_eq!(track.title, "Song (From 'Film')");
    }
}
