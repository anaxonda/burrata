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
    queries.push(SearchQuery { text, reason });
}

impl SearchPlan {
    /// Build a canonical query and one alternate within the fixed provider
    /// budget. A source-verified artist spelling takes priority; when none is
    /// available, use the first role-aware lead-performer credit.
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
        let rendered_title = search_title(track);
        push_query(
            &mut queries,
            &track.artist,
            &rendered_title,
            SearchQueryReason::CanonicalIdentity,
        );

        let release_fallback = allow_other_releases
            .then(|| search_title_without_release(track))
            .flatten();
        if let Some(fallback_title) = release_fallback {
            let identity = crate::recording_identity::RecordingIdentity::from_track(track);
            let lead = identity
                .credits
                .iter()
                .find(|credit| crate::recording_identity::is_primary_performer_role(&credit.role))
                .map(|credit| credit.name.as_str());
            push_query(
                &mut queries,
                verified_artist_alternatives
                    .first()
                    .map(String::as_str)
                    .or(lead)
                    .unwrap_or(&track.artist),
                &fallback_title,
                SearchQueryReason::ReleaseContextFallback,
            );
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
            let identity = crate::recording_identity::RecordingIdentity::from_track(track);
            let lead = identity
                .credits
                .iter()
                .find(|credit| crate::recording_identity::is_primary_performer_role(&credit.role))
                .map(|credit| credit.name.as_str());
            let fallback_title = search_title_without_soundtrack(track);
            let reason = if fallback_title != rendered_title {
                SearchQueryReason::SoundtrackContextFallback
            } else if lead.is_some() {
                SearchQueryReason::PrimaryPerformer
            } else {
                SearchQueryReason::CanonicalIdentity
            };
            push_query(
                &mut queries,
                lead.unwrap_or(&track.artist),
                &fallback_title,
                reason,
            );
        }
        if queries.len() < MAX_SEARCH_QUERIES {
            for alternative in verified_title_alternatives.iter().take(2) {
                let title = search_title_with_alternative(track, alternative);
                push_query(
                    &mut queries,
                    &track.artist,
                    &title,
                    SearchQueryReason::VerifiedTitleAlternative,
                );
                if queries.len() == MAX_SEARCH_QUERIES {
                    break;
                }
            }
        }
        Self { queries }
    }
}

fn search_title(track: &Track) -> String {
    let title = track.title.trim();
    let Some(version) = track
        .version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return title.to_owned();
    };
    let parsed = crate::recording_identity::parse_title(title);
    let title_version = parsed.version.as_deref().unwrap_or_default();
    if crate::matching::normalize(title_version) == crate::matching::normalize(version) {
        title.to_owned()
    } else {
        format!("{title} {version}")
    }
}

fn search_title_with_alternative(track: &Track, alternative: &str) -> String {
    let title = alternative.trim();
    if title.is_empty() {
        return String::new();
    }
    let separate_version = track.version.as_deref().map(str::trim).filter(|value| {
        !value.is_empty()
            && crate::matching::normalize(
                crate::recording_identity::parse_title(title)
                    .version
                    .as_deref()
                    .unwrap_or_default(),
            ) != crate::matching::normalize(value)
    });
    separate_version.map_or_else(|| title.to_owned(), |version| format!("{title} {version}"))
}

fn release_label(value: &str) -> bool {
    let normalized = crate::matching::normalize(value);
    normalized.contains("restored edition") || normalized.contains("remaster")
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
    let keep_separate_version = separate_version.filter(|version| {
        !release_label(version)
            && crate::matching::normalize(&parsed_title_version)
                != crate::matching::normalize(version)
    });
    keep_separate_version.map_or(Some(title.clone()), |version| {
        Some(format!("{title} {version}"))
    })
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
    let separate_version = track.version.as_deref().map(str::trim).filter(|value| {
        !value.is_empty()
            && crate::matching::normalize(
                crate::recording_identity::parse_title(&title)
                    .version
                    .as_deref()
                    .unwrap_or_default(),
            ) != crate::matching::normalize(value)
    });
    separate_version.map_or(title.clone(), |version| format!("{title} {version}"))
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
    fn primary_performer_query_uses_only_explicit_performer_roles() {
        let track = Track {
            title: "Song".into(),
            artist: "Lead, Guest, Composer".into(),
            provenance: json!({"artistCredits":[
                {"name":"Lead","role":"MAIN_ARTIST"},
                {"name":"Guest","role":"featured"},
                {"name":"Composer","role":"composer"}
            ]}),
            ..Track::default()
        };
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries.len(), 2);
        assert_eq!(plan.queries[1].reason, SearchQueryReason::PrimaryPerformer);
        assert_eq!(plan.queries[1].text, "Lead Song");
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
