//! Eligibility is only permission to consider identification, never publication.
use crate::recording_match::{Comparison, MatchAction, MatchAssessment, MatchReason};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadAssessment {
    #[serde(flatten)]
    assessment: MatchAssessment,
    identification_eligible: bool,
}

impl DownloadAssessment {
    pub(crate) fn new(
        assessment: MatchAssessment,
        track: &crate::models::Track,
        candidate: &crate::models::Candidate,
    ) -> Self {
        let identification_eligible = candidate.permission != "restricted"
            && track.duration_seconds.is_some_and(|duration| {
                duration.is_finite() && (50.0..=86_400.0).contains(&duration)
            })
            && eligible_for_identification(&assessment);
        Self {
            assessment,
            identification_eligible,
        }
    }
}

pub(crate) fn eligible_for_identification(assessment: &MatchAssessment) -> bool {
    use Comparison::{Agree, Conflict};
    use MatchReason::*;
    let d = &assessment.dimensions;
    assessment.evidence.as_ref().is_none_or(|evidence| {
        !evidence.identifier_conflict
            && !evidence.conflicting_version_observations
            && !evidence.parsing_incomplete
            && evidence.unresolved_annotations.is_empty()
    }) && assessment.action != MatchAction::Accept
        && d.title == Agree
        && d.runtime == Agree
        && [d.artists, d.guests, d.version, d.isrc, d.work]
            .iter()
            .all(|value| *value != Conflict)
        && !assessment.reasons.iter().any(|reason| {
            matches!(
                reason,
                TitleConflict
                    | ArtistConflict
                    | SourceIdentityConflict
                    | GuestConflict
                    | VersionConflict
                    | RuntimeConflict
                    | IsrcConflict
                    | PermissionRestricted
                    | PreviewTooShort
                    | ConflictingRequestIdentifiers
                    | ConflictingRequestVersions
                    | ConflictingCandidateVersions
                    | UnresolvedAnnotation
                    | EmptyIdentityKey
                    | ParsingIncomplete
            )
        })
}

/// Identifier linkage is one prerequisite, not a recording-policy or file proof.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IdentifierLink {
    SharedIsrc,
    VerifiedCatalogRecording { recording_id: String },
}

pub(crate) fn recognized_identifier_link(
    store: &crate::Store,
    requested: &crate::models::Track,
    recognition: &crate::autotag::AudioRecognition,
) -> Option<IdentifierLink> {
    use crate::recording_identity::{normalize_isrc, RecordingIdentity};
    if recognition.provider != "songrec" || recognition.excerpt_count != 2 {
        return None;
    }
    let agreed = normalize_isrc(recognition.agreeing_identifiers.isrc.as_deref()?)?;
    if recognition
        .isrc
        .as_deref()
        .and_then(normalize_isrc)
        .as_ref()
        != Some(&agreed)
    {
        return None;
    }
    let identity = RecordingIdentity::from_track(requested);
    if identity.identifier_conflict {
        return None;
    }
    if let Some(isrc) = identity.isrc {
        return (isrc == agreed).then_some(IdentifierLink::SharedIsrc);
    }
    // Missing requested ISRC may be resolved only through an exact cached
    // catalog occurrence revalidated against the original release response.
    let recording_id =
        crate::artist_credit_aliases::verified_musicbrainz_recording_isrc_membership(
            store, requested, &agreed,
        )?;
    Some(IdentifierLink::VerifiedCatalogRecording { recording_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording_match::{MatchDimensions, MatchRelationship, ProofStrength};
    fn weak() -> MatchAssessment {
        use Comparison::*;
        MatchAssessment {
            action: MatchAction::Enrich,
            relationship: MatchRelationship::Unresolved,
            reasons: vec![MatchReason::InsufficientEvidence],
            dimensions: MatchDimensions {
                title: Agree,
                runtime: Agree,
                artists: Unknown,
                guests: Unknown,
                version: Agree,
                isrc: Unknown,
                work: Unknown,
            },
            proof_strength: ProofStrength::MetadataOnly,
            evidence: None,
        }
    }
    #[test]
    fn identifier_link_requires_two_explicit_agreeing_observations() {
        let root = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&root.path().join("verification.sqlite")).unwrap();
        let mut requested = crate::models::Track {
            isrc: Some("USABC2600001".into()),
            ..Default::default()
        };
        let mut recognition = crate::autotag::AudioRecognition {
            provider: "songrec".into(),
            title: "Synthetic".into(),
            artist: "Synthetic".into(),
            isrc: Some("USABC2600001".into()),
            shazam_id: None,
            excerpt_count: 2,
            agreeing_identifiers: Default::default(),
        };
        assert_eq!(
            recognized_identifier_link(&store, &requested, &recognition),
            None
        );
        recognition.agreeing_identifiers.isrc = recognition.isrc.clone();
        assert_eq!(
            recognized_identifier_link(&store, &requested, &recognition),
            Some(IdentifierLink::SharedIsrc)
        );
        recognition.excerpt_count = 1;
        assert_eq!(
            recognized_identifier_link(&store, &requested, &recognition),
            None
        );
        recognition.excerpt_count = 2;
        requested.isrc = Some("USABC2600002".into());
        assert_eq!(
            recognized_identifier_link(&store, &requested, &recognition),
            None
        );
        requested.isrc = None;
        assert_eq!(
            recognized_identifier_link(&store, &requested, &recognition),
            None
        );
    }

    #[test]
    fn response_preserves_assessment_and_bounds_sampling_eligibility() {
        let mut track = crate::models::Track {
            duration_seconds: Some(180.0),
            ..Default::default()
        };
        let mut candidate = crate::models::Candidate::default();
        let value =
            serde_json::to_value(DownloadAssessment::new(weak(), &track, &candidate)).unwrap();
        assert_eq!(value["action"], "enrich");
        assert_eq!(value["identificationEligible"], true);
        candidate.permission = "restricted".into();
        assert!(!DownloadAssessment::new(weak(), &track, &candidate).identification_eligible);
        candidate.permission.clear();
        for duration in [None, Some(49.0), Some(86_401.0), Some(f64::NAN)] {
            track.duration_seconds = duration;
            assert!(!DownloadAssessment::new(weak(), &track, &candidate).identification_eligible);
        }
    }
    #[test]
    fn weak_evidence_is_eligible_but_accepted_sources_skip_identification() {
        let mut value = weak();
        assert!(eligible_for_identification(&value));
        value.action = MatchAction::Accept;
        assert!(!eligible_for_identification(&value));
    }
    #[test]
    fn uncertain_title_or_runtime_and_all_identity_conflicts_refuse_identification() {
        for dimension in ["title", "runtime", "artists", "guests", "version", "isrc"] {
            let mut value = weak();
            let field = match dimension {
                "title" => &mut value.dimensions.title,
                "runtime" => &mut value.dimensions.runtime,
                "artists" => &mut value.dimensions.artists,
                "guests" => &mut value.dimensions.guests,
                "version" => &mut value.dimensions.version,
                _ => &mut value.dimensions.isrc,
            };
            *field = Comparison::Conflict;
            assert!(!eligible_for_identification(&value), "{dimension}");
        }
        for dimension in ["title", "runtime"] {
            let mut value = weak();
            if dimension == "title" {
                value.dimensions.title = Comparison::Unknown;
            } else {
                value.dimensions.runtime = Comparison::Unknown;
            }
            assert!(!eligible_for_identification(&value));
        }
        for reason in [
            MatchReason::PermissionRestricted,
            MatchReason::PreviewTooShort,
            MatchReason::ConflictingRequestIdentifiers,
            MatchReason::SourceIdentityConflict,
            MatchReason::ConflictingCandidateVersions,
            MatchReason::ParsingIncomplete,
        ] {
            let mut value = weak();
            value.reasons.push(reason);
            assert!(!eligible_for_identification(&value), "{reason:?}");
        }
    }
}
