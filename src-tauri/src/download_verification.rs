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

pub(crate) fn needs_staged_verification(
    track: &crate::models::Track,
    candidate: &crate::models::Candidate,
    assessment: &MatchAssessment,
    settings: &crate::models::Settings,
    has_chapters: bool,
) -> bool {
    track.provenance["downloadAudioVerificationEnabled"] == true
        && settings.download_audio_verification_enabled
        && track.provenance["downloadAudioVerificationAttempted"] != true
        && !has_chapters
        && track.playback_range.is_none()
        && track.provenance["isExcerpt"] != true
        && !track.provenance["chapter"].is_object()
        && candidate.permission != "restricted"
        && track
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && (50.0..=86_400.0).contains(&duration))
        && eligible_for_identification(assessment)
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

/// In-memory identity result only. Publication additionally requires the bound
/// staged bytes, unchanged locator and live consent/cancellation checks.
pub(crate) struct VerifiedRecognitionIdentity {
    _link: IdentifierLink,
}

pub(crate) fn verify_recognized_recording(
    store: &crate::Store,
    requested: &crate::models::Track,
    recognition: &crate::autotag::AudioRecognition,
    measured_duration: f64,
    level: crate::recording_match::MatchLevel,
) -> Option<VerifiedRecognitionIdentity> {
    use crate::recording_match::{MatchLevel, MatchRelationship, MatchStage};
    let link = recognized_identifier_link(store, requested, recognition)?;
    if !measured_duration.is_finite()
        || !(50.0..=86_400.0).contains(&measured_duration)
        || recognition.title.trim().is_empty()
        || recognition.artist.trim().is_empty()
        || recognition.title.len() > 512
        || recognition.artist.len() > 512
    {
        return None;
    }
    let observed = crate::models::Candidate {
        id: "songrec:staged-recognition".into(),
        provider: "songrec".into(),
        title: recognition.title.clone(),
        duration_seconds: Some(measured_duration),
        source_metadata: Some(serde_json::json!({
            "sourceMetadataOrigin":"songrecRecognition", "track":recognition.title,
            "artist":recognition.artist, "isrc":recognition.agreeing_identifiers.isrc,
        })),
        ..Default::default()
    };
    // Flexible permits identified alternatives for ordinary selection, but an
    // audio verification claim must establish the requested recording itself.
    let verification_level = if level == MatchLevel::Flexible {
        MatchLevel::Balanced
    } else {
        level
    };
    let assessment = crate::recording_match_context::assess(
        store,
        requested,
        &observed,
        verification_level,
        MatchStage::Resolved,
    );
    let d = &assessment.dimensions;
    (assessment.action == MatchAction::Accept
        && matches!(
            assessment.relationship,
            MatchRelationship::Exact | MatchRelationship::Probable
        )
        && d.title == Comparison::Agree
        && d.artists == Comparison::Agree
        && d.runtime == Comparison::Agree
        && [d.guests, d.version, d.isrc, d.work]
            .iter()
            .all(|value| *value != Comparison::Conflict))
    .then_some(VerifiedRecognitionIdentity { _link: link })
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
    fn admission_needs_queued_and_live_consent_and_never_replays_or_identifies_excerpts() {
        let mut track = crate::models::Track {
            duration_seconds: Some(180.0),
            provenance: serde_json::json!({}),
            ..Default::default()
        };
        let candidate = crate::models::Candidate::default();
        let mut settings = crate::models::Settings::default();
        settings.download_audio_verification_enabled = true;
        assert!(!needs_staged_verification(
            &track,
            &candidate,
            &weak(),
            &settings,
            false
        ));
        track.provenance["downloadAudioVerificationEnabled"] = serde_json::json!(true);
        assert!(needs_staged_verification(
            &track,
            &candidate,
            &weak(),
            &settings,
            false
        ));
        settings.download_audio_verification_enabled = false;
        assert!(!needs_staged_verification(
            &track,
            &candidate,
            &weak(),
            &settings,
            false
        ));
        settings.download_audio_verification_enabled = true;
        assert!(!needs_staged_verification(
            &track,
            &candidate,
            &weak(),
            &settings,
            true
        ));
        track.provenance["downloadAudioVerificationAttempted"] = serde_json::json!(true);
        assert!(!needs_staged_verification(
            &track,
            &candidate,
            &weak(),
            &settings,
            false
        ));
        track.provenance["downloadAudioVerificationAttempted"] = serde_json::json!(false);
        track.provenance["isExcerpt"] = serde_json::json!(true);
        assert!(!needs_staged_verification(
            &track,
            &candidate,
            &weak(),
            &settings,
            false
        ));
    }

    #[test]
    fn shared_identifier_never_overrides_recording_policy_or_measured_runtime() {
        let root = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&root.path().join("policy.sqlite")).unwrap();
        let requested = crate::models::Track {
            title: "Synthetic Song".into(),
            artist: "Synthetic Artist".into(),
            duration_seconds: Some(180.0),
            isrc: Some("USABC2600001".into()),
            ..Default::default()
        };
        let recognition = crate::autotag::AudioRecognition {
            provider: "songrec".into(),
            title: requested.title.clone(),
            artist: requested.artist.clone(),
            isrc: requested.isrc.clone(),
            shazam_id: None,
            excerpt_count: 2,
            agreeing_identifiers: crate::recording_identity::RecognitionIdentifiers {
                isrc: requested.isrc.clone(),
                ..Default::default()
            },
        };
        let verify = |request: &crate::models::Track,
                      result: &crate::autotag::AudioRecognition,
                      duration| {
            verify_recognized_recording(
                &store,
                request,
                result,
                duration,
                crate::recording_match::MatchLevel::Balanced,
            )
            .is_some()
        };
        assert!(verify(&requested, &recognition, 180.0));
        for duration in [30.0, 250.0, f64::NAN] {
            assert!(!verify(&requested, &recognition, duration));
        }
        for (title, artist) in [
            ("Other Song", "Synthetic Artist"),
            ("Synthetic Song (Live)", "Synthetic Artist"),
            ("Synthetic Song", "Other Artist"),
        ] {
            let mut different = recognition.clone();
            different.title = title.into();
            different.artist = artist.into();
            assert!(!verify(&requested, &different, 180.0), "{title} / {artist}");
        }
        let mut featured = requested.clone();
        featured.title = "Synthetic Song (feat. Guest Artist)".into();
        assert!(!verify(&featured, &recognition, 180.0));
        assert!(verify_recognized_recording(
            &store,
            &featured,
            &recognition,
            180.0,
            crate::recording_match::MatchLevel::Flexible
        )
        .is_none());
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
