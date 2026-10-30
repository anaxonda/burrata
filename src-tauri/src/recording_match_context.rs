//! Gather retained recording evidence for every consumer of the policy evaluator.
//! These scoped variants are inputs to one evaluator, never independent gates.
use crate::{
    models::{Candidate, Track},
    recording_match::{
        self, Comparison, MatchAction, MatchAssessment, MatchLevel, MatchRelationship, MatchStage,
    },
    storage::Store,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

/// Reusable only within one matching operation. Cache profile query rows, not
/// inferred aliases or assessment decisions; the Store lookup still applies
/// the persisted profile expiry when each distinct name is first requested.
#[derive(Default)]
pub(crate) struct AssessmentContext {
    artist_profiles: ArtistProfileLookupMemo,
    verified_artist_aliases:
        HashMap<(String, String), crate::artist_credit_aliases::VerifiedPlaybackArtistAliases>,
}

#[derive(Default)]
struct ArtistProfileLookupMemo {
    by_query: HashMap<String, Arc<[Value]>>,
}

impl ArtistProfileLookupMemo {
    fn get_or_load(&mut self, name: &str, load: impl FnOnce() -> Vec<Value>) -> Arc<[Value]> {
        // Match the input-side semantics of cached_musicbrainz_artist_profiles_for_name:
        // Rust trims the query, and SQLite lower() folds ASCII only. Do not use
        // matching::normalize_single_artist_name here: it has broader semantics.
        let key = name.trim().to_ascii_lowercase();
        self.by_query
            .entry(key)
            .or_insert_with(|| load().into())
            .clone()
    }
}

impl AssessmentContext {
    fn artist_profiles_for_name(&mut self, store: &Store, name: &str) -> Arc<[Value]> {
        self.artist_profiles.get_or_load(name, || {
            store
                .cached_musicbrainz_artist_profiles_for_name(name)
                .unwrap_or_default()
        })
    }

    fn musicbrainz_credit_variant(
        &mut self,
        store: &Store,
        track: &Track,
        candidate_artist: &str,
    ) -> Option<crate::recording_identity::VerifiedArtistCreditSetVariant> {
        // The verified credit object is bound to the full request Track, not
        // just its ID. Candidate-specific identity variants can share an ID;
        // caching by ID alone can retain an alias object for the wrong Track.
        let serialized_track =
            serde_json::to_vec(track).unwrap_or_else(|_| format!("{track:?}").into_bytes());
        use sha2::Digest;
        let fingerprint = format!("{:x}", sha2::Sha256::digest(serialized_track));
        let key = (track.id.clone(), fingerprint);
        self.verified_artist_aliases
            .entry(key)
            .or_insert_with(|| {
                crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
            })
            .musicbrainz_credit_variant(track, candidate_artist)
    }
}

fn candidate_isrc(candidate: &Candidate) -> Option<String> {
    let metadata = candidate.source_metadata.as_ref()?;
    if metadata["provider"].as_str()? != candidate.provider {
        return None;
    }
    let provider_id = candidate
        .id
        .strip_prefix(&format!("{}:", candidate.provider))?;
    if metadata["trackId"].as_str()? != provider_id {
        return None;
    }
    metadata["isrc"]
        .as_str()
        .and_then(crate::recording_identity::normalize_isrc)
}

fn candidate_runtime(candidate: &Candidate) -> Option<f64> {
    candidate
        .duration_seconds
        .or_else(|| {
            candidate
                .source_metadata
                .as_ref()?
                .get("duration")?
                .as_f64()
        })
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 86_400.0)
}

fn close_to_release_runtime(candidate: f64, release: f64) -> bool {
    (candidate - release).abs() <= 2.0_f64.max(candidate.min(release) * 0.02)
}

fn scoped_occurrence_title_alias(
    track: &Track,
    candidate: &Candidate,
    variant: &crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant,
) -> Option<String> {
    let candidate_title = candidate
        .source_metadata
        .as_ref()
        .and_then(|metadata| {
            metadata["catalogTitle"]
                .as_str()
                .or(metadata["track"].as_str())
        })
        .unwrap_or(&candidate.title);
    (crate::artist_alias_discovery::recording_title_variant_compatible(
        &track.title,
        &variant.recording_title,
    ) && crate::artist_alias_discovery::occurrence_bound_title_rendering_compatible(
        candidate_title,
        &variant.release_track_title,
    ))
    .then(|| candidate_title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_lookup_memo_caches_successes_empty_results_and_ambiguity_by_query() {
        let mut memo = ArtistProfileLookupMemo::default();
        let mut calls = 0;
        let ambiguous = vec![json!({"id":"one"}), json!({"id":"two"})];
        let first = memo.get_or_load(" Stage Name ", || {
            calls += 1;
            ambiguous.clone()
        });
        let second = memo.get_or_load("stage name", || {
            calls += 1;
            Vec::new()
        });
        assert_eq!(calls, 1);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.len(), 2); // Ambiguity is retained, never collapsed to a guessed match.

        let empty = memo.get_or_load("No Cached Artist", || {
            calls += 1;
            Vec::new()
        });
        let empty_again = memo.get_or_load(" no cached artist ", || {
            calls += 1;
            vec![json!({"id":"unexpected"})]
        });
        assert!(empty.is_empty());
        assert!(Arc::ptr_eq(&empty, &empty_again));
        assert_eq!(calls, 2);

        // SQLite lower() is ASCII-only; do not merge Unicode query spellings
        // that the underlying exact-name SQL would keep distinct.
        memo.get_or_load("École", || {
            calls += 1;
            Vec::new()
        });
        memo.get_or_load("école", || {
            calls += 1;
            Vec::new()
        });
        assert_eq!(calls, 4);
    }

    #[test]
    fn verified_artist_credit_alias_cache_is_scoped_to_full_track_input() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("credit-alias-context.sqlite")).unwrap();
        let artist_id = "80fb9c75-af15-4a6b-9528-715f9ef4ebea";
        let release_id = "1f602996-f661-3611-b25a-93c3337fd76e";
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                artist_id,
                &json!({
                    "id":artist_id,
                    "name":"Bulgarian State Radio and Television Female Vocal Choir",
                    "aliases":[{"name":"Bulgarian State Radio & Television Female Vocal Choir","type":"Artist name"}],
                }),
            )
            .unwrap();
        let source_url = format!("https://musicbrainz.org/release/{release_id}");
        let track = Track {
            id: "catalog:musicbrainz:context-track".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz:context-track".into(),
            source_url: source_url.clone(),
            artist: "The Bulgarian State Radio and Television Female Vocal Choir".into(),
            title: "Pilentze Pee".into(),
            album: Some("Le Mystère des voix bulgares".into()),
            duration_seconds: Some(146.666),
            provenance: json!({
                "artistCredits":[{
                    "artist":{"provider":"musicbrainz","id":artist_id},
                    "creditedAs":"The Bulgarian State Radio and Television Female Vocal Choir",
                    "joinPhrase":"",
                    "name":"The Bulgarian State Radio and Television Female Vocal Choir",
                    "role":"artist credit",
                    "sourceUrl":source_url,
                }],
                "catalogProvider":"musicbrainz",
                "manualFields":{},
                "releaseId":release_id,
                "sourceKind":"catalog",
                "sourceUrl":source_url,
            }),
            ..Track::default()
        };
        let mut candidate_specific = track.clone();
        candidate_specific.duration_seconds = Some(145.0);
        candidate_specific.isrc = Some("USNO18716501".into());
        let folder_artist = "Bulgarian State Radio & Television Female Vocal Choir";
        let mut context = AssessmentContext::default();

        assert!(context
            .musicbrainz_credit_variant(&store, &candidate_specific, folder_artist)
            .is_some());
        assert!(context
            .musicbrainz_credit_variant(&store, &track, folder_artist)
            .is_some());
        assert!(context
            .musicbrainz_credit_variant(&store, &candidate_specific, folder_artist)
            .is_some());
    }

    #[test]
    fn typed_folder_credit_fast_path_matches_full_variant_search_for_metadata_only_album_track() {
        use crate::{models::CandidateSource, recording_match::MatchStage};

        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("folder-credit-fast-path.sqlite")).unwrap();
        let artist_id = "80fb9c75-af15-4a6b-9528-715f9ef4ebea";
        let release_id = "1f602996-f661-3611-b25a-93c3337fd76e";
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                artist_id,
                &json!({
                    "id":artist_id,
                    "name":"Bulgarian State Radio and Television Female Vocal Choir",
                    "aliases":[{"name":"Bulgarian State Radio & Television Female Vocal Choir","type":"Artist name"}],
                }),
            )
            .unwrap();
        let source_url = format!("https://musicbrainz.org/release/{release_id}");
        let requested = Track {
            id: "catalog:musicbrainz:fast-path-track".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz:fast-path-track".into(),
            source_url: source_url.clone(),
            artist: "The Bulgarian State Radio and Television Female Vocal Choir".into(),
            title: "Pilentze Pee".into(),
            album: Some("Le Mystère des voix bulgares".into()),
            isrc: Some("USNO18716501".into()),
            duration_seconds: Some(146.666),
            provenance: json!({
                "artistCredits":[{
                    "artist":{"provider":"musicbrainz","id":artist_id},
                    "creditedAs":"The Bulgarian State Radio and Television Female Vocal Choir",
                    "joinPhrase":"",
                    "name":"The Bulgarian State Radio and Television Female Vocal Choir",
                    "role":"artist credit",
                    "sourceUrl":source_url,
                }],
                "catalogProvider":"musicbrainz",
                "manualFields":{},
                "releaseId":release_id,
                "sourceKind":"catalog",
                "sourceUrl":source_url,
            }),
            ..Track::default()
        };
        let folder_artist = "Bulgarian State Radio & Television Female Vocal Choir";
        let filename =
            format!("Root/{folder_artist}/Le Mystère des voix bulgares/01 Pilentze Pee.flac");
        let basename = "01 Pilentze Pee";
        let candidate = Candidate {
            id: "slskd:fixture:bulgarian-pilentze".into(),
            provider: "slskd".into(),
            title: basename.into(),
            duration_seconds: Some(146.666),
            permission: "unknown".into(),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"slskdFilename",
                "filenameObservations":crate::recording_filename::filename_observations(&filename, basename),
            })),
            source: Some(CandidateSource::Soulseek {
                username: "fixture-peer".into(),
                filename,
                size_bytes: 1_000_000,
                bitrate_kbps: Some(900),
                search_id: None,
                batch_id: None,
                transfer_id: None,
            }),
            ..Candidate::default()
        };
        let aliases = (0..24)
            .map(|index| {
                let mut alias = requested.clone();
                alias.artist = format!("Unrelated Retained Alias {index}");
                alias
            })
            .collect::<Vec<_>>();
        let runtime_variants = (0..10)
            .map(
                |index| crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant {
                    recording_id: format!("00000000-0000-4000-8000-{index:012x}"),
                    release_id: release_id.into(),
                    isrc: "USNO18716501".into(),
                    source_isrcs: vec!["USNO18716501".into()],
                    recording_title: "Pilentze Pee".into(),
                    release_track_title: "Pilentze Pee".into(),
                    release_artist_credit: requested.artist.clone(),
                    release_artist_names: vec![
                        "Bulgarian State Radio and Television Female Vocal Choir".into(),
                    ],
                    release_artist_ids: vec![artist_id.into()],
                    duration_seconds: 146.666,
                },
            )
            .collect::<Vec<_>>();
        let slow = assess_with_aliases_and_context_impl(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &aliases,
            &[],
            requested.isrc.as_deref(),
            &runtime_variants,
            &mut AssessmentContext::default(),
            false,
        );
        let fast = assess_with_aliases_and_context_impl(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &aliases,
            &[],
            requested.isrc.as_deref(),
            &runtime_variants,
            &mut AssessmentContext::default(),
            true,
        );
        assert_eq!(slow, fast);
        assert_eq!(fast.action, MatchAction::Accept);
        assert_eq!(fast.relationship, MatchRelationship::Probable);
        assert_eq!(
            fast.proof_strength,
            crate::recording_match::ProofStrength::MetadataOnly
        );
        assert!(fast
            .evidence
            .as_ref()
            .is_some_and(|evidence| evidence.scoped_artist_credit_set_alias.is_some()));

        // Exact matching remains non-accepting for metadata-only proof, so
        // the optimization must leave it on the full path. This fixture's
        // richer catalog input may request enrichment rather than inspection.
        let exact = assess_with_aliases_and_context_impl(
            &store,
            &requested,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
            &aliases,
            &[],
            requested.isrc.as_deref(),
            &runtime_variants,
            &mut AssessmentContext::default(),
            true,
        );
        assert_ne!(exact.action, MatchAction::Accept);
        assert_eq!(
            exact.proof_strength,
            crate::recording_match::ProofStrength::MetadataOnly
        );
        assert!(!safe_metadata_only_folder_credit_accept(&exact));

        let live_filename = format!(
            "Root/{folder_artist}/Le Mystère des voix bulgares/01 Pilentze Pee (Live).flac"
        );
        let live_basename = "01 Pilentze Pee (Live)";
        let mut explicit_live = candidate.clone();
        explicit_live.title = live_basename.into();
        if let Some(CandidateSource::Soulseek { filename, .. }) = explicit_live.source.as_mut() {
            *filename = live_filename.clone();
        }
        explicit_live.source_metadata.as_mut().unwrap()["filenameObservations"] =
            crate::recording_filename::filename_observations(&live_filename, live_basename);
        let mut requested_studio = requested.clone();
        requested_studio.version = Some("Studio".into());
        let slow_live = assess_with_aliases_and_context_impl(
            &store,
            &requested_studio,
            &explicit_live,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &aliases,
            &[],
            requested_studio.isrc.as_deref(),
            &runtime_variants,
            &mut AssessmentContext::default(),
            false,
        );
        let fast_live = assess_with_aliases_and_context_impl(
            &store,
            &requested_studio,
            &explicit_live,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &aliases,
            &[],
            requested_studio.isrc.as_deref(),
            &runtime_variants,
            &mut AssessmentContext::default(),
            true,
        );
        assert_eq!(slow_live, fast_live);
        assert_ne!(fast_live.action, MatchAction::Accept);

        let mut unresolved = fast.clone();
        unresolved
            .evidence
            .as_mut()
            .unwrap()
            .unresolved_annotations
            .push(crate::recording_identity::AnnotationObservation {
                origin: crate::recording_identity::EvidenceOrigin {
                    field: "sourceMetadata.catalogTitle".into(),
                    source_group: "slskd:fixture".into(),
                    authority: crate::recording_identity::EvidenceAuthority::UploaderCaption,
                },
                annotation: crate::recording_identity::TextAnnotation {
                    span: crate::recording_identity::TextSpan { start: 0, end: 1 },
                    content_span: crate::recording_identity::TextSpan { start: 0, end: 1 },
                    kind: crate::recording_identity::AnnotationKind::Unresolved,
                    value: "unknown edition".into(),
                },
            });
        assert!(!safe_metadata_only_folder_credit_accept(&unresolved));
    }

    #[test]
    fn exact_isrc_and_runtime_allow_scoped_alias_check_under_unknown_title_gloss() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("alias-gloss.sqlite")).unwrap();
        let artist_id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                artist_id,
                &json!({
                    "id":artist_id,
                    "name":"Lakis Halkias",
                    "aliases":[{"name":"Lakis Chalkias"}]
                }),
            )
            .unwrap();
        let requested = Track {
            provider: "musicbrainz".into(),
            provider_id: "recording-request".into(),
            artist: "Lakis Chalkias".into(),
            title: "To Miroloi Tis Manas".into(),
            isrc: Some("USABC2600001".into()),
            duration_seconds: Some(128.0),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:gloss-row".into(),
            title: "Lakis Halkias - To Miroloi Tis Manas (The Mother's Lament)".into(),
            duration_seconds: Some(128.0),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"gloss-row",
                "catalogTitle":"To Miroloi Tis Manas (The Mother's Lament)",
                "artistNames":["Lakis Halkias"],
                "duration":128.0,
                "isrc":"USABC2600001"
            })),
            ..Candidate::default()
        };
        let assessment = assess_with_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &mut AssessmentContext::default(),
        );
        assert_eq!(assessment.dimensions.title, Comparison::Unknown);
        assert_eq!(assessment.dimensions.artists, Comparison::Agree);
        assert_eq!(assessment.action, MatchAction::Accept);
        assert_eq!(
            assessment
                .evidence
                .as_ref()
                .unwrap()
                .scoped_artist_name_alias
                .as_ref()
                .map(|alias| alias.musicbrainz_artist_id.as_str()),
            Some(artist_id)
        );

        let mut without_identifier = requested;
        without_identifier.isrc = None;
        let unbound = assess_with_context(
            &store,
            &without_identifier,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &mut AssessmentContext::default(),
        );
        assert!(unbound
            .evidence
            .as_ref()
            .unwrap()
            .scoped_artist_name_alias
            .is_none());
        assert_ne!(unbound.action, MatchAction::Accept);
    }

    #[test]
    fn verified_alias_covers_requested_credit_while_open_ensemble_credit_stays_uncertain() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("open-credit-alias.sqlite")).unwrap();
        let artist_id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                artist_id,
                &json!({
                    "id":artist_id,
                    "name":"Lakis Halkias",
                    "aliases":[{"name":"Lakis Chalkias"}]
                }),
            )
            .unwrap();
        let requested = Track {
            provider: "spotify".into(),
            provider_id: "spotify:2Q9DpDilWFbnGzEbX5AIBS".into(),
            artist: "Lakis Chalkias".into(),
            title: "To Miroloi Tis Manas".into(),
            album: Some("36 Tragoudia Tis Ipirou Me Ton Laki Chalkia".into()),
            duration_seconds: Some(127.626),
            provenance: json!({"artistCredits":[{
                "name":"Lakis Chalkias",
                "role":"artist credit",
                "artist":{"id":artist_id}
            }]}),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:264536741330165760".into(),
            title: "Lakis Halkias, Dimotiko Paradosiako Sygrotima Elatos - To Miroloi Tis Manas"
                .into(),
            duration_seconds: Some(127.626),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"264536741330165760",
                "catalogTitle":"To Miroloi Tis Manas",
                "artistNames":["Lakis Halkias","Dimotiko Paradosiako Sygrotima Elatos"],
                "duration":127.626,
                "isrc":"USA371646567"
            })),
            ..Candidate::default()
        };
        let balanced = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(balanced.relationship, MatchRelationship::Probable);
        assert_eq!(balanced.action, MatchAction::Accept);
        let evidence = balanced.evidence.as_ref().unwrap();
        assert_eq!(evidence.dimensions.title, Comparison::Agree);
        assert_eq!(evidence.dimensions.artists, Comparison::Agree);
        assert_eq!(evidence.dimensions.runtime, Comparison::Agree);
        assert_eq!(
            evidence.proof_strength,
            recording_match::ProofStrength::MetadataOnly
        );
        assert_eq!(
            evidence
                .scoped_artist_name_alias
                .as_ref()
                .unwrap()
                .musicbrainz_artist_id,
            artist_id
        );
        assert_eq!(
            evidence.extra_performers,
            vec!["dimotiko paradosiako sygrotima elatos"]
        );
        assert!(!evidence.requested_performer_credits_complete);
        assert_eq!(requested.artist, "Lakis Chalkias");

        let exact = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(
            exact.relationship,
            MatchRelationship::Probable,
            "{exact:#?}"
        );
        assert_eq!(exact.action, MatchAction::Inspect, "{exact:#?}");
        let exact_evidence = exact.evidence.as_ref().unwrap();
        assert!(exact_evidence.scoped_artist_name_alias.is_some());
        assert_eq!(exact.dimensions.artists, Comparison::Agree, "{exact:#?}");
        assert_eq!(
            exact_evidence.extra_performers,
            vec!["dimotiko paradosiako sygrotima elatos"]
        );

        let mut closed = requested.clone();
        closed.provenance = json!({
            "artistCredits":[{"name":"Lakis Chalkias","role":"artist credit"}],
            "performerCreditsComplete":true,
            "manualFields":{"performerCreditsComplete":true}
        });
        let closed = assess_with_aliases_and_context(
            &store,
            &closed,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(closed.action, MatchAction::Reject);

        let mut replaced = candidate.clone();
        replaced.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Another Singer"]);
        replaced.title = "Another Singer - To Miroloi Tis Manas".into();
        let replaced = assess_with_aliases_and_context(
            &store,
            &requested,
            &replaced,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(replaced.action, MatchAction::Reject);

        let mut wrong_isrc_request = requested.clone();
        wrong_isrc_request.isrc = Some("USABC2600001".into());
        let wrong_isrc = assess_with_aliases_and_context(
            &store,
            &wrong_isrc_request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(wrong_isrc.action, MatchAction::Reject);

        let mut wrong_version = candidate.clone();
        wrong_version.title = "Lakis Halkias - To Miroloi Tis Manas (Live)".into();
        wrong_version.source_metadata.as_mut().unwrap()["catalogTitle"] =
            json!("To Miroloi Tis Manas (Live)");
        wrong_version.source_metadata.as_mut().unwrap()["track"] =
            json!("To Miroloi Tis Manas (Live)");
        let wrong_version = assess_with_aliases_and_context(
            &store,
            &requested,
            &wrong_version,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(wrong_version.action, MatchAction::Reject);
    }

    #[test]
    fn soulseek_caption_uses_cached_mb_artist_id_name_variant_without_article_heuristic() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("mb-folder-credit.sqlite")).unwrap();
        let release_id = "4f7f6950-29cc-448d-a25b-445e383612f1";
        let artist_id = "80fb9c75-af15-4a6b-9528-715f9ef4ebea";
        let source_url = format!("https://musicbrainz.org/release/{release_id}");
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                artist_id,
                &json!({"id":artist_id,"name":"Bulgarian State Radio and Television Female Vocal Choir"}),
            )
            .unwrap();
        let requested = Track {
            id: "musicbrainz:album-track".into(),
            provider: "catalog".into(),
            provider_id: "album-track".into(),
            source_url: source_url.clone(),
            title: "Pilentze Pee".into(),
            artist: "The Bulgarian State Radio and Television Female Vocal Choir".into(),
            duration_seconds: Some(141.506),
            provenance: json!({
                "sourceKind":"catalog",
                "catalogProvider":"musicbrainz",
                "sourceUrl":source_url,
                "releaseId":release_id,
                "artistCredits":[{
                    "name":"The Bulgarian State Radio and Television Female Vocal Choir",
                    "role":"artist credit",
                    "artist":{"provider":"musicbrainz","id":artist_id},
                    "sourceUrl":source_url,
                    "joinPhrase":""
                }]
            }),
            ..Track::default()
        };
        let filename = "Music/Bulgarian State Radio and Television Female Vocal Choir/Album/01 - Pilentze Pee.flac";
        let raw_basename = "01 - Pilentze Pee";
        let observations =
            crate::recording_filename::verified_filename_observations(filename, raw_basename)
                .unwrap();
        assert_eq!(
            observations["folderArtist"],
            "Bulgarian State Radio and Television Female Vocal Choir"
        );
        let candidate = Candidate {
            provider: "slskd".into(),
            id: "slskd:bulgarian-fixture".into(),
            title: raw_basename.into(),
            duration_seconds: Some(141.506),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"slskdFilename",
                "filenameObservations":observations
            })),
            source: Some(crate::models::CandidateSource::Soulseek {
                username: "peer".into(),
                filename: filename.into(),
                size_bytes: 1_000_000,
                bitrate_kbps: None,
                search_id: None,
                batch_id: None,
                transfer_id: None,
            }),
            ..Candidate::default()
        };
        let assessment = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(
            assessment.relationship,
            MatchRelationship::Probable,
            "{assessment:#?}"
        );
        assert_eq!(assessment.action, MatchAction::Accept, "{assessment:#?}");
        assert_eq!(assessment.dimensions.artists, Comparison::Agree);
        let evidence = assessment.evidence.unwrap();
        assert_eq!(
            evidence.proof_strength,
            recording_match::ProofStrength::MetadataOnly
        );
        let credit_alias = evidence.scoped_artist_credit_set_alias.unwrap();
        assert_eq!(credit_alias.provider, "musicbrainz");
        assert_eq!(credit_alias.artist_ids, vec![artist_id]);
        assert_eq!(
            credit_alias.evidence_scope,
            "musicBrainzArtistProfileNameEquivalenceOnly"
        );

        let exact = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_ne!(exact.action, MatchAction::Accept);
        assert_eq!(
            exact.proof_strength,
            recording_match::ProofStrength::MetadataOnly
        );

        let wrong_artist = "Music/Unrelated Choir/Album/01 - Pilentze Pee.flac";
        let mut wrong_candidate = candidate.clone();
        wrong_candidate.source_metadata.as_mut().unwrap()["filenameObservations"] =
            crate::recording_filename::verified_filename_observations(wrong_artist, raw_basename)
                .unwrap();
        wrong_candidate.source = Some(crate::models::CandidateSource::Soulseek {
            username: "peer".into(),
            filename: wrong_artist.into(),
            size_bytes: 1_000_000,
            bitrate_kbps: None,
            search_id: None,
            batch_id: None,
            transfer_id: None,
        });
        let no_alias = assess_with_aliases_and_context(
            &store,
            &requested,
            &wrong_candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(no_alias.action, MatchAction::Reject);

        let mut manual = requested.clone();
        manual.provenance["manualFields"] = json!({"artist":true});
        let manual = assess_with_aliases_and_context(
            &store,
            &manual,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            &[],
            &mut AssessmentContext::default(),
        );
        assert_eq!(manual.action, MatchAction::Reject);
    }

    #[test]
    fn exact_recording_release_runtime_is_probable_and_keeps_original_durations() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("release-runtime.sqlite")).unwrap();
        let requested = Track {
            provider: "spotify".into(),
            provider_id: "spotify:source".into(),
            artist: "Bulgarian State Radio and Television Female Vocal Choir".into(),
            title: "Pilentze Pee".into(),
            isrc: Some("USNO18716501".into()),
            duration_seconds: Some(146.666),
            provenance: json!({"artistCredits":[{
                "name":"Bulgarian State Radio and Television Female Vocal Choir",
                "role":"artist credit",
                "artist":{"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea"}
            }]}),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:recording".into(),
            title: "Bulgarian State Television Female Choir - Pilentze Pee".into(),
            duration_seconds: Some(141.506),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "track":"Pilentze Pee",
                "catalogTitle":"Pilentze Pee",
                "artistNames":["Bulgarian State Television Female Choir"],
                "duration":141.506,
                "isrc":"USNO18716501"
            })),
            ..Candidate::default()
        };
        let alias = Track {
            artist: "Bulgarian State Television Female Choir".into(),
            ..requested.clone()
        };
        let variant = crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant {
            recording_id: "00000000-0000-4000-8000-000000000001".into(),
            release_id: "00000000-0000-4000-8000-000000000002".into(),
            isrc: "USNO18716501".into(),
            source_isrcs: vec!["USNO18716501".into()],
            recording_title: "Pilentze Pee".into(),
            release_track_title: "Pilentze Pee".into(),
            release_artist_credit: "Bulgarian State Television Female Choir".into(),
            release_artist_names: vec!["Bulgarian State Television Female Choir".into()],
            release_artist_ids: vec!["80fb9c75-af15-4a6b-9528-715f9ef4ebea".into()],
            duration_seconds: 141.0,
        };
        let mut context = AssessmentContext::default();
        let assessment = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            std::slice::from_ref(&alias),
            std::slice::from_ref(&alias),
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut context,
        );
        assert_eq!(assessment.relationship, MatchRelationship::Probable);
        assert_eq!(assessment.action, MatchAction::Accept);
        let evidence = assessment.evidence.unwrap();
        assert_eq!(evidence.dimensions.runtime, Comparison::Agree);
        let runtime = evidence.recording_scoped_runtime_variant.as_ref().unwrap();
        assert_eq!(runtime.requested_runtime_seconds, Some(146.666));
        assert_eq!(runtime.release_runtime_seconds, 141.0);
        assert_eq!(runtime.candidate_runtime_seconds, Some(141.506));
        assert_eq!(
            runtime.requested_artist_credit,
            "Bulgarian State Radio and Television Female Vocal Choir"
        );
        assert_eq!(
            runtime.release_artist_credit,
            "Bulgarian State Television Female Choir"
        );
        let credit = evidence.recording_scoped_credit_variant.as_ref().unwrap();
        assert_eq!(
            credit.requested_artist_credit,
            "Bulgarian State Radio and Television Female Vocal Choir"
        );
        assert_eq!(
            credit.release_artist_names,
            vec!["Bulgarian State Television Female Choir".to_owned()]
        );

        let mut other_isrc_candidate = candidate.clone();
        other_isrc_candidate.source_metadata.as_mut().unwrap()["isrc"] = json!("CH1648980010");
        let wrong_code = assess_with_aliases_and_context(
            &store,
            &requested,
            &other_isrc_candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            std::slice::from_ref(&alias),
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_eq!(wrong_code.action, MatchAction::Reject);

        let exact = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
            std::slice::from_ref(&alias),
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_eq!(exact.relationship, MatchRelationship::Probable);
        assert_eq!(exact.action, MatchAction::Inspect);
    }

    #[test]
    fn exact_occurrence_scopes_transliterated_title_and_unclassified_gloss() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("occurrence-title.sqlite")).unwrap();
        let requested = Track {
            provider: "catalog".into(),
            provider_id: "musicbrainz:request".into(),
            artist: "Bulgarian State Radio and Television Female Vocal Choir".into(),
            title: "Breï Yvane".into(),
            isrc: Some("USNO18716506".into()),
            duration_seconds: Some(94.973),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:occurrence-row".into(),
            title: "Bre_Yvance (Dancing Song)".into(),
            duration_seconds: Some(90.84),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"occurrence-row",
                "catalogTitle":"Bre_Yvance (Dancing Song)",
                "artistNames":["Bulgarian State Television Female Choir"],
                "duration":90.84,
                "isrc":"USNO18716506"
            })),
            ..Candidate::default()
        };
        let variant = crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant {
            recording_id: "00000000-0000-4000-8000-000000000001".into(),
            release_id: "00000000-0000-4000-8000-000000000002".into(),
            isrc: "USNO18716506".into(),
            source_isrcs: vec!["USNO18716506".into()],
            recording_title: "Breï Yvane".into(),
            release_track_title: "Breï Yvane (Dancing Song)".into(),
            release_artist_credit: "Bulgarian State Television Female Choir".into(),
            release_artist_names: vec!["Bulgarian State Television Female Choir".into()],
            release_artist_ids: vec!["80fb9c75-af15-4a6b-9528-715f9ef4ebea".into()],
            duration_seconds: 90.0,
        };
        let balanced = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_eq!(balanced.action, MatchAction::Accept);
        assert_eq!(balanced.relationship, MatchRelationship::Probable);
        let evidence = balanced.evidence.unwrap();
        assert_eq!(evidence.dimensions.title, Comparison::Agree);
        assert_eq!(evidence.dimensions.artists, Comparison::Agree);
        assert_eq!(evidence.dimensions.runtime, Comparison::Agree);
        assert_eq!(
            evidence
                .recording_scoped_title_alias
                .unwrap()
                .requested_title,
            "Breï Yvane"
        );
        assert_eq!(
            evidence
                .recording_scoped_runtime_variant
                .unwrap()
                .requested_runtime_seconds,
            Some(94.973)
        );
        assert_eq!(requested.title, "Breï Yvane");
        assert_eq!(requested.duration_seconds, Some(94.973));

        let exact = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved,
            &[],
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_eq!(exact.action, MatchAction::Inspect);
        assert_eq!(exact.relationship, MatchRelationship::Probable);

        let mut wrong_code = candidate.clone();
        wrong_code.source_metadata.as_mut().unwrap()["isrc"] = json!("CH1648980010");
        let wrong_code = assess_with_aliases_and_context(
            &store,
            &requested,
            &wrong_code,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_ne!(wrong_code.action, MatchAction::Accept);

        let mut explicit_live = candidate.clone();
        explicit_live.title = "Breï Yvane (Live)".into();
        explicit_live.source_metadata.as_mut().unwrap()["catalogTitle"] =
            json!("Breï Yvane (Live)");
        explicit_live.source_metadata.as_mut().unwrap()["track"] = json!("Breï Yvane (Live)");
        let explicit_live = assess_with_aliases_and_context(
            &store,
            &requested,
            &explicit_live,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_ne!(explicit_live.action, MatchAction::Accept);

        let mut manually_cleared = requested.clone();
        manually_cleared.isrc = None;
        manually_cleared.provenance = json!({"manualFields":{"isrc":true}});
        let manually_cleared = assess_with_aliases_and_context(
            &store,
            &manually_cleared,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            None,
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_ne!(manually_cleared.action, MatchAction::Accept);
    }

    #[test]
    fn recording_scoped_credit_variant_resolves_without_runtime_change() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("release-credit.sqlite")).unwrap();
        let requested = Track {
            provider: "musicbrainz".into(),
            provider_id: "musicbrainz:recording".into(),
            artist: "The Bulgarian State Radio and Television Female Vocal Choir".into(),
            title: "Pilentze Pee".into(),
            isrc: Some("USNO18716501".into()),
            duration_seconds: Some(141.5),
            provenance: json!({"artistCredits":[{
                "name":"The Bulgarian State Radio and Television Female Vocal Choir",
                "role":"artist credit",
                "artist":{"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea"}
            }]}),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:recording".into(),
            title: "Bulgarian State Television Female Choir - Pilentze Pee (The Evening Gathering)"
                .into(),
            duration_seconds: Some(141.5),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "track":"Pilentze Pee (The Evening Gathering)",
                "catalogTitle":"Pilentze Pee (The Evening Gathering)",
                "artistNames":["Bulgarian State Television Female Choir"],
                "duration":141.5,
                "isrc":"USNO18716501"
            })),
            ..Candidate::default()
        };
        let variant = crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant {
            recording_id: "00000000-0000-4000-8000-000000000001".into(),
            release_id: "00000000-0000-4000-8000-000000000002".into(),
            isrc: "USNO18716501".into(),
            source_isrcs: vec!["USNO18716501".into()],
            recording_title: "Pilentze Pee".into(),
            release_track_title: "Pilentze Pee".into(),
            release_artist_credit: "Bulgarian State Television Female Choir".into(),
            release_artist_names: vec!["Bulgarian State Television Female Choir".into()],
            release_artist_ids: vec!["80fb9c75-af15-4a6b-9528-715f9ef4ebea".into()],
            duration_seconds: 141.5,
        };
        let assessment = assess_with_aliases_and_context(
            &store,
            &requested,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            &[],
            &[],
            requested.isrc.as_deref(),
            std::slice::from_ref(&variant),
            &mut AssessmentContext::default(),
        );
        assert_eq!(assessment.action, MatchAction::Accept);
        assert_eq!(assessment.relationship, MatchRelationship::Probable);
        let evidence = assessment.evidence.unwrap();
        assert_eq!(evidence.dimensions.title, Comparison::Unknown);
        assert_eq!(evidence.dimensions.runtime, Comparison::Agree);
        let credit = evidence.recording_scoped_credit_variant.unwrap();
        assert_eq!(credit.release_artist_names, variant.release_artist_names);
        assert_eq!(
            credit.requested_artist_credit,
            "The Bulgarian State Radio and Television Female Vocal Choir"
        );
    }
}

pub(crate) fn priority(a: &MatchAssessment) -> (u8, u8) {
    let relationship = match a.relationship {
        MatchRelationship::Exact => 0,
        MatchRelationship::Probable => 1,
        MatchRelationship::Alternative => 2,
        MatchRelationship::Unresolved => 3,
    };
    let action = match a.action {
        MatchAction::Accept => 0,
        MatchAction::Inspect => 1,
        MatchAction::Enrich => 2,
        MatchAction::Reject => 3,
    };
    (
        if a.action == MatchAction::Reject {
            4
        } else {
            relationship
        },
        action,
    )
}
pub(crate) fn assess(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
) -> MatchAssessment {
    assess_with_context(
        store,
        track,
        candidate,
        level,
        stage,
        &mut AssessmentContext::default(),
    )
}

/// Assess a candidate using a caller-owned operation context. Batch callers
/// should reuse one context across candidate retrieval, sorting and policy
/// checks to avoid repeated identical profile database reads.
pub(crate) fn assess_with_context(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    context: &mut AssessmentContext,
) -> MatchAssessment {
    let aliases =
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track);
    assess_with_aliases_and_context(
        store,
        track,
        candidate,
        level,
        stage,
        aliases.tracks(),
        aliases.isrc_enriched(),
        aliases.verified_recording_isrc(),
        aliases.release_runtime_variants(),
        context,
    )
}
pub(crate) fn assess_with_aliases(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
) -> MatchAssessment {
    assess_with_aliases_and_context(
        store,
        track,
        candidate,
        level,
        stage,
        aliases,
        isrc_aliases,
        known_isrc,
        &[],
        &mut AssessmentContext::default(),
    )
}

pub(crate) fn assess_with_aliases_and_context(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    runtime_variants: &[crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant],
    context: &mut AssessmentContext,
) -> MatchAssessment {
    assess_with_aliases_and_context_impl(
        store,
        track,
        candidate,
        level,
        stage,
        aliases,
        isrc_aliases,
        known_isrc,
        runtime_variants,
        context,
        true,
    )
}

fn assess_with_aliases_and_context_impl(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    runtime_variants: &[crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant],
    context: &mut AssessmentContext,
    allow_folder_artist_fast_path: bool,
) -> MatchAssessment {
    let mut requested = track.clone();
    let manually_owned_isrc = track.provenance["manualFields"]["isrc"].as_bool() == Some(true);
    if requested.isrc.is_none() && !manually_owned_isrc {
        requested.isrc = known_isrc.map(str::to_owned);
    }
    let mut scoped_isrc_membership = None;
    if requested.isrc.is_none() && !manually_owned_isrc {
        if let Some(candidate_code) = candidate_isrc(candidate) {
            if let Some(variant) = runtime_variants.iter().find(|variant| {
                variant.source_isrcs.contains(&candidate_code) && variant.source_isrcs.len() <= 16
            }) {
                requested.isrc = Some(candidate_code.clone());
                scoped_isrc_membership = Some(
                    recording_match::RecordingScopedIsrcMembershipEvidence {
                        recording_id: variant.recording_id.clone(),
                        release_id: variant.release_id.clone(),
                        requested_isrc: track.isrc.clone(),
                        selected_isrc: candidate_code,
                        source_isrcs: variant.source_isrcs.clone(),
                        authority: recording_match::RecordingScopedIsrcAuthority::VerifiedMusicBrainzOccurrence,
                    },
                );
            }
        }
    }
    let mut best = recording_match::assess(&requested, candidate, level, stage);
    let mut best_request = requested.clone();
    let early_folder_credit_assessment = if allow_folder_artist_fast_path
        && candidate_isrc(candidate).is_none()
        && !candidate_has_recording_identifier(candidate)
        && requested.isrc.is_some()
    {
        soulseek_folder_credit_assessment(store, track, candidate, level, stage, context)
            .filter(|assessment| safe_metadata_only_folder_credit_accept(assessment))
    } else {
        None
    };
    if let Some(assessment) = early_folder_credit_assessment.clone() {
        return assessment;
    }
    let verified_credits =
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track);
    for variant in aliases {
        let mut same = track.clone();
        same.artist = variant.artist.clone();
        if !manually_owned_isrc && isrc_aliases.contains(variant) {
            same.isrc = variant.isrc.clone();
        }
        if &same != variant {
            continue;
        }
        let next =
            if let Some(credits) = verified_credits.matching_credit_names(track, &variant.artist) {
                recording_match::assess_with_primary_credit_override(
                    variant, candidate, level, stage, &credits,
                )
            } else {
                recording_match::assess(variant, candidate, level, stage)
            };
        if priority(&next) < priority(&best) {
            best = next;
            best_request = variant.clone();
        }
    }
    let mut selected_credit_variant = None;
    for variant in runtime_variants.iter().filter(|variant| {
        requested
            .isrc
            .as_deref()
            .is_some_and(|isrc| variant.source_isrcs.iter().any(|code| code == isrc))
            && !variant.release_artist_names.is_empty()
            && variant.release_artist_names.len() <= 16
            && variant.release_artist_ids.len() == variant.release_artist_names.len()
            && variant
                .release_artist_ids
                .iter()
                .all(|id| crate::artist_credit_aliases::valid_musicbrainz_recording_id(id))
            && variant
                .release_artist_names
                .iter()
                .all(|name| !name.trim().is_empty() && name.len() <= 256)
    }) {
        let mut variant_request = best_request.clone();
        variant_request.artist = variant.release_artist_credit.clone();
        let next = recording_match::assess_with_primary_credit_override(
            &variant_request,
            candidate,
            level,
            stage,
            &variant.release_artist_names,
        );
        let recording_credit_resolves_artist = next.dimensions.artists == Comparison::Agree
            && best.dimensions.artists != Comparison::Agree
            && next.dimensions.isrc == Comparison::Agree
            && next.dimensions.title != Comparison::Conflict
            && next.dimensions.guests != Comparison::Conflict
            && next.dimensions.version != Comparison::Conflict;
        if priority(&next) < priority(&best) || recording_credit_resolves_artist {
            best = next;
            best_request = variant_request;
            selected_credit_variant = Some(variant);
        }
    }
    if let (Some(variant), Some(evidence)) = (selected_credit_variant, best.evidence.as_mut()) {
        evidence.recording_scoped_credit_variant =
            Some(recording_match::RecordingScopedCreditVariantEvidence {
                recording_id: variant.recording_id.clone(),
                release_id: variant.release_id.clone(),
                isrc: requested
                    .isrc
                    .clone()
                    .unwrap_or_else(|| variant.isrc.clone()),
                requested_artist_credit: track.artist.clone(),
                release_artist_credit: variant.release_artist_credit.clone(),
                release_artist_names: variant.release_artist_names.clone(),
                release_artist_ids: variant.release_artist_ids.clone(),
            });
    }
    let title_scoped_alias_is_supported = best.dimensions.title == Comparison::Agree
        || best.dimensions.title == Comparison::Unknown
            && best.dimensions.isrc == Comparison::Agree
            && best.dimensions.runtime == Comparison::Agree
            && best.dimensions.version != Comparison::Conflict
            && best.evidence.as_ref().is_some_and(|evidence| {
                !evidence.unresolved_annotations.is_empty()
                    && evidence.unresolved_annotations.iter().all(|annotation| {
                        matches!(
                            annotation.origin.field.as_str(),
                            "title" | "sourceMetadata.track" | "sourceMetadata.catalogTitle"
                        )
                    })
            });
    if title_scoped_alias_is_supported && best.dimensions.artists != Comparison::Agree {
        let mut lookup = |name: &str| context.artist_profiles_for_name(store, name);
        if let Some((variant, musicbrainz_artist_id)) =
            crate::recording_match_aliases::scoped_name_alias_with_lookup(
                track,
                candidate,
                &mut lookup,
            )
        {
            let mut next = recording_match::assess_with_scoped_artist_name_alias(
                track,
                candidate,
                level,
                stage,
                &track.artist,
                &variant.artist,
            );
            if let Some(evidence) = next.evidence.as_mut() {
                evidence.scoped_artist_name_alias =
                    Some(recording_match::ScopedArtistNameAliasEvidence {
                        requested_name: track.artist.clone(),
                        candidate_primary_name: variant.artist.clone(),
                        musicbrainz_artist_id,
                        evidence_scope: "uniqueArtistProfileNameEquivalenceOnly".into(),
                    });
            }
            // A lexical alias can resolve a spelling ambiguity, but it cannot
            // turn metadata-only evidence into Exact identity. Exact may use
            // it to report Inspect rather than a false artist conflict; only a
            // recording identifier can authorize Exact through this branch.
            if level == MatchLevel::Exact
                && next.proof_strength != recording_match::ProofStrength::RecordingIdentifier
                && next.action == MatchAction::Accept
            {
                next.action = MatchAction::Inspect;
            }
            // Keep the scoped profile evidence even when Exact must still
            // inspect/enrich and therefore has the same action priority.
            if priority(&next) <= priority(&best) {
                best = next;
                best_request = variant;
            }
        }
    }
    if best.dimensions.runtime == Comparison::Conflict
        && (best.dimensions.title == Comparison::Agree
            || runtime_variants.iter().any(|variant| {
                crate::artist_alias_discovery::same_recording_release_title(
                    &track.title,
                    &variant.recording_title,
                ) && candidate
                    .source_metadata
                    .as_ref()
                    .and_then(|metadata| {
                        metadata["catalogTitle"]
                            .as_str()
                            .or(metadata["track"].as_str())
                    })
                    .is_some_and(|candidate_title| {
                        crate::artist_alias_discovery::same_recording_release_title(
                            &variant.recording_title,
                            candidate_title,
                        )
                    })
            }))
        && best.dimensions.artists == Comparison::Agree
        && best.dimensions.isrc == Comparison::Agree
        && best.dimensions.guests != Comparison::Conflict
        && best.dimensions.version != Comparison::Conflict
        && best.proof_strength == recording_match::ProofStrength::RecordingIdentifier
        && best.evidence.as_ref().is_some_and(|evidence| {
            !evidence.identifier_conflict
                && !evidence.parsing_incomplete
                && !evidence.conflicting_version_observations
        })
    {
        let candidate_runtime = candidate
            .duration_seconds
            .or_else(|| {
                candidate
                    .source_metadata
                    .as_ref()?
                    .get("duration")?
                    .as_f64()
            })
            .filter(|value| value.is_finite() && *value > 0.0 && *value <= 86_400.0);
        if let Some(candidate_runtime) = candidate_runtime {
            for variant in runtime_variants.iter().filter(|variant| {
                requested
                    .isrc
                    .as_deref()
                    .is_some_and(|isrc| variant.source_isrcs.iter().any(|code| code == isrc))
                    && crate::source_fallback::normalize_artist_credit(&best_request.artist)
                        == crate::source_fallback::normalize_artist_credit(
                            &variant.release_artist_credit,
                        )
                    && (candidate_runtime - variant.duration_seconds).abs()
                        <= 2.0_f64.max(candidate_runtime.min(variant.duration_seconds) * 0.02)
            }) {
                let mut variant_request = best_request.clone();
                variant_request.isrc = requested.isrc.clone();
                let original_request_runtime = variant_request.duration_seconds;
                variant_request.duration_seconds = Some(variant.duration_seconds);
                let mut resolved = recording_match::assess_with_primary_credit_override(
                    &variant_request,
                    candidate,
                    level,
                    stage,
                    &variant.release_artist_names,
                );
                let title_is_same_recording_gloss = resolved.dimensions.title
                    == Comparison::Unknown
                    && crate::artist_alias_discovery::same_recording_release_title(
                        &track.title,
                        &variant.recording_title,
                    )
                    && candidate
                        .source_metadata
                        .as_ref()
                        .and_then(|metadata| {
                            metadata["catalogTitle"]
                                .as_str()
                                .or(metadata["track"].as_str())
                        })
                        .is_some_and(|candidate_title| {
                            crate::artist_alias_discovery::same_recording_release_title(
                                &variant.recording_title,
                                candidate_title,
                            )
                        });
                if resolved.action == MatchAction::Reject
                    || resolved.dimensions.title != Comparison::Agree
                        && !title_is_same_recording_gloss
                    || resolved.dimensions.artists != Comparison::Agree
                    || resolved.dimensions.isrc != Comparison::Agree
                    || resolved.dimensions.runtime != Comparison::Agree
                    || resolved.dimensions.guests == Comparison::Conflict
                    || resolved.dimensions.version == Comparison::Conflict
                {
                    continue;
                }
                resolved.relationship = MatchRelationship::Probable;
                if level == MatchLevel::Exact {
                    resolved.action = MatchAction::Inspect;
                } else if title_is_same_recording_gloss {
                    resolved.action = MatchAction::Accept;
                }
                if !resolved
                    .reasons
                    .contains(&recording_match::MatchReason::RecordingScopedRuntimeVariant)
                {
                    resolved
                        .reasons
                        .push(recording_match::MatchReason::RecordingScopedRuntimeVariant);
                }
                if let Some(evidence) = resolved.evidence.as_mut() {
                    evidence.runtime_delta_seconds =
                        original_request_runtime.map(|requested| candidate_runtime - requested);
                    evidence.recording_scoped_runtime_variant =
                        Some(recording_match::RecordingScopedRuntimeVariantEvidence {
                            recording_id: variant.recording_id.clone(),
                            release_id: variant.release_id.clone(),
                            isrc: requested
                                .isrc
                                .clone()
                                .unwrap_or_else(|| variant.isrc.clone()),
                            recording_title: variant.recording_title.clone(),
                            release_track_title: variant.release_track_title.clone(),
                            requested_artist_credit: track.artist.clone(),
                            release_artist_credit: variant.release_artist_credit.clone(),
                            requested_runtime_seconds: original_request_runtime,
                            release_runtime_seconds: variant.duration_seconds,
                            candidate_runtime_seconds: Some(candidate_runtime),
                        });
                    if let Some(membership) = scoped_isrc_membership.clone() {
                        evidence.recording_scoped_isrc_membership = Some(membership);
                    }
                    evidence.recording_scoped_credit_variant =
                        Some(recording_match::RecordingScopedCreditVariantEvidence {
                            recording_id: variant.recording_id.clone(),
                            release_id: variant.release_id.clone(),
                            isrc: requested
                                .isrc
                                .clone()
                                .unwrap_or_else(|| variant.isrc.clone()),
                            requested_artist_credit: track.artist.clone(),
                            release_artist_credit: variant.release_artist_credit.clone(),
                            release_artist_names: variant.release_artist_names.clone(),
                            release_artist_ids: variant.release_artist_ids.clone(),
                        });
                }
                best = resolved;
                break;
            }
        }
    }
    // The same exact-recording release occurrence can also bind title and
    // credit renderings when the indexed candidate retains a translated gloss
    // or an alternate catalog spelling. Keep this path tied to both ISRCs and
    // the validated release runtime; it does not promote a title alias into
    // globally reusable identity evidence.
    if let Some(candidate_code) = candidate_isrc(candidate) {
        for variant in runtime_variants.iter().filter(|variant| {
            requested
                .isrc
                .as_deref()
                .is_some_and(|code| code == &candidate_code)
                && variant.source_isrcs.len() <= 16
                && variant.source_isrcs.contains(&candidate_code)
                && variant.release_artist_names.len() == variant.release_artist_ids.len()
                && !variant.release_artist_names.is_empty()
                && variant.release_artist_names.len() <= 16
                && variant
                    .release_artist_ids
                    .iter()
                    .all(|id| crate::artist_credit_aliases::valid_musicbrainz_recording_id(id))
                && variant
                    .release_artist_names
                    .iter()
                    .all(|name| !name.trim().is_empty() && name.len() <= 256)
        }) {
            let Some(candidate_title) = scoped_occurrence_title_alias(track, candidate, variant)
            else {
                continue;
            };
            let Some(candidate_runtime) = candidate_runtime(candidate) else {
                continue;
            };
            if !close_to_release_runtime(candidate_runtime, variant.duration_seconds) {
                continue;
            }
            let mut occurrence_request = track.clone();
            occurrence_request.artist = variant.release_artist_credit.clone();
            occurrence_request.duration_seconds = Some(variant.duration_seconds);
            let original_request_runtime = track.duration_seconds;
            let mut resolved = recording_match::assess_with_scoped_occurrence_title_and_credit(
                &occurrence_request,
                candidate,
                level,
                stage,
                &variant.release_artist_names,
            );
            if resolved.action == MatchAction::Reject
                || resolved.dimensions.title != Comparison::Agree
                || resolved.dimensions.artists != Comparison::Agree
                || resolved.dimensions.isrc != Comparison::Agree
                || resolved.dimensions.runtime != Comparison::Agree
                || resolved.dimensions.guests == Comparison::Conflict
                || resolved.dimensions.version == Comparison::Conflict
                || resolved.proof_strength != recording_match::ProofStrength::RecordingIdentifier
                || resolved.evidence.as_ref().is_some_and(|evidence| {
                    evidence.identifier_conflict
                        || evidence.parsing_incomplete
                        || evidence.conflicting_version_observations
                })
            {
                continue;
            }
            resolved.relationship = MatchRelationship::Probable;
            if level == MatchLevel::Exact {
                resolved.action = MatchAction::Inspect;
            }
            if !resolved
                .reasons
                .contains(&recording_match::MatchReason::RecordingScopedTitleAlias)
            {
                resolved
                    .reasons
                    .push(recording_match::MatchReason::RecordingScopedTitleAlias);
            }
            if !resolved
                .reasons
                .contains(&recording_match::MatchReason::RecordingScopedRuntimeVariant)
            {
                resolved
                    .reasons
                    .push(recording_match::MatchReason::RecordingScopedRuntimeVariant);
            }
            if let Some(evidence) = resolved.evidence.as_mut() {
                evidence.recording_scoped_title_alias =
                    Some(recording_match::RecordingScopedTitleAliasEvidence {
                        recording_id: variant.recording_id.clone(),
                        release_id: variant.release_id.clone(),
                        isrc: candidate_code.clone(),
                        requested_title: track.title.clone(),
                        recording_title: variant.recording_title.clone(),
                        release_track_title: variant.release_track_title.clone(),
                        candidate_title,
                    });
                evidence.recording_scoped_runtime_variant =
                    Some(recording_match::RecordingScopedRuntimeVariantEvidence {
                        recording_id: variant.recording_id.clone(),
                        release_id: variant.release_id.clone(),
                        isrc: candidate_code.clone(),
                        recording_title: variant.recording_title.clone(),
                        release_track_title: variant.release_track_title.clone(),
                        requested_artist_credit: track.artist.clone(),
                        release_artist_credit: variant.release_artist_credit.clone(),
                        requested_runtime_seconds: original_request_runtime,
                        release_runtime_seconds: variant.duration_seconds,
                        candidate_runtime_seconds: Some(candidate_runtime),
                    });
                evidence.recording_scoped_credit_variant =
                    Some(recording_match::RecordingScopedCreditVariantEvidence {
                        recording_id: variant.recording_id.clone(),
                        release_id: variant.release_id.clone(),
                        isrc: candidate_code.clone(),
                        requested_artist_credit: track.artist.clone(),
                        release_artist_credit: variant.release_artist_credit.clone(),
                        release_artist_names: variant.release_artist_names.clone(),
                        release_artist_ids: variant.release_artist_ids.clone(),
                    });
                if let Some(membership) = scoped_isrc_membership.clone() {
                    evidence.recording_scoped_isrc_membership = Some(membership);
                }
            }
            if priority(&resolved) < priority(&best) || best.action == MatchAction::Reject {
                best = resolved;
                best_request = occurrence_request;
            }
        }
    }
    // A validated Soulseek filename is only uploader-caption evidence. When
    // its folder artist is an exact name owned by the request's MusicBrainz
    // artist ID, compare that spelling through a typed, cached profile alias.
    // The caption path stays Probable/Inspect under the ordinary scorer policy.
    if let Some(next) = early_folder_credit_assessment.or_else(|| {
        soulseek_folder_credit_assessment(store, track, candidate, level, stage, context)
    }) {
        if next.dimensions.artists == Comparison::Agree
            && next.dimensions.title == Comparison::Agree
            && next.dimensions.version != Comparison::Conflict
            && next.dimensions.guests != Comparison::Conflict
            && (priority(&next) <= priority(&best) || best.action == MatchAction::Reject)
        {
            best = next;
        }
    }
    if let (Some(membership), Some(evidence)) = (scoped_isrc_membership, best.evidence.as_mut()) {
        evidence.recording_scoped_isrc_membership = Some(membership);
    }
    best
}

fn soulseek_folder_credit_assessment(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    context: &mut AssessmentContext,
) -> Option<MatchAssessment> {
    if candidate.provider != "slskd" || candidate.permission == "restricted" {
        return None;
    }
    let folder_artist = recording_match::verified_soulseek_folder_artist(candidate)?;
    let variant = context.musicbrainz_credit_variant(store, track, &folder_artist)?;
    Some(
        recording_match::assess_with_scoped_artist_credit_set_variant(
            track, candidate, level, stage, &variant,
        ),
    )
}

fn safe_metadata_only_folder_credit_accept(assessment: &MatchAssessment) -> bool {
    use recording_match::{Comparison, MatchAction, MatchRelationship, ProofStrength};

    let Some(evidence) = assessment.evidence.as_ref() else {
        return false;
    };
    assessment.action == MatchAction::Accept
        && assessment.relationship == MatchRelationship::Probable
        && assessment.proof_strength == ProofStrength::MetadataOnly
        && evidence.proof_strength == ProofStrength::MetadataOnly
        && evidence.scoped_artist_credit_set_alias.is_some()
        && !evidence.identifier_conflict
        && !evidence.parsing_incomplete
        && !evidence.conflicting_version_observations
        && evidence.unresolved_annotations.is_empty()
        && assessment.dimensions.title == Comparison::Agree
        && assessment.dimensions.artists == Comparison::Agree
        && assessment.dimensions.runtime == Comparison::Agree
        && assessment.dimensions.isrc == Comparison::Unknown
        && assessment.dimensions.guests != Comparison::Conflict
        && assessment.dimensions.version != Comparison::Conflict
}

fn candidate_has_recording_identifier(candidate: &Candidate) -> bool {
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return false;
    };
    ["isrc", "recordingId", "workId"].iter().any(|field| {
        metadata[*field]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
    }) || metadata["identifiers"]
        .as_array()
        .is_some_and(|identifiers| !identifiers.is_empty())
}

pub(crate) fn assess_with_artist_credit_set_variant(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    variant: &crate::recording_identity::VerifiedArtistCreditSetVariant,
) -> MatchAssessment {
    let base = assess_with_aliases(
        store,
        track,
        candidate,
        level,
        stage,
        aliases,
        isrc_aliases,
        known_isrc,
    );
    let scoped = recording_match::assess_with_scoped_artist_credit_set_variant(
        track, candidate, level, stage, variant,
    );
    if priority(&scoped) <= priority(&base) || base.action == MatchAction::Reject {
        scoped
    } else {
        base
    }
}
