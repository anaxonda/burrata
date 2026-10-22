//! Playback orchestration for the shared policy evaluator. Transport verification
//! stays here; provider-specific matching decisions belong to recording_match.
use super::*;
use crate::recording_match::{
    self, Comparison, MatchAction, MatchAssessment, MatchLevel, MatchRelationship, MatchStage,
    ProofStrength,
};
pub(super) const VERSION: &str = crate::recording_match::EVALUATOR_VERSION;

pub(super) fn fingerprint(identity: &str, level: MatchLevel) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("recording-match:{VERSION}:{level:?}:{identity}").as_bytes())
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SharedEvidenceRank {
    pub(super) relationship: MatchRelationship,
    pub(super) action: MatchAction,
    pub(super) proof_strength: ProofStrength,
    pub(super) ambiguity_count: u16,
}

impl SharedEvidenceRank {
    fn from_assessment(assessment: &MatchAssessment) -> Self {
        let evidence = assessment.evidence.as_ref();
        let dimensions = evidence
            .map(|evidence| &evidence.dimensions)
            .unwrap_or(&assessment.dimensions);
        let unknown_dimensions = [
            dimensions.title,
            dimensions.artists,
            dimensions.guests,
            dimensions.version,
            dimensions.runtime,
            dimensions.isrc,
            dimensions.work,
        ]
        .into_iter()
        .filter(|dimension| *dimension == Comparison::Unknown)
        .count() as u16;

        let mut ambiguity_count = unknown_dimensions;
        if let Some(evidence) = evidence {
            // Count each source-group/span once so duplicate provider fields do
            // not masquerade as independent unresolved evidence.
            let annotations = evidence
                .unresolved_annotations
                .iter()
                .map(|observation| {
                    (
                        observation.origin.source_group.clone(),
                        observation.annotation.span.start,
                        observation.annotation.span.end,
                        observation.annotation.value.clone(),
                    )
                })
                .collect::<std::collections::BTreeSet<_>>();
            let missing = evidence
                .missing_performers
                .iter()
                .map(|name| crate::matching::normalize_single_artist_name(name))
                .filter(|name| !name.is_empty())
                .collect::<std::collections::BTreeSet<_>>();
            let extra = evidence
                .extra_performers
                .iter()
                .map(|name| crate::matching::normalize_single_artist_name(name))
                .filter(|name| !name.is_empty())
                .collect::<std::collections::BTreeSet<_>>();
            ambiguity_count = ambiguity_count
                .saturating_add(annotations.len() as u16)
                .saturating_add(missing.len() as u16)
                .saturating_add(extra.len() as u16)
                .saturating_add(u16::from(evidence.parsing_incomplete));
        }

        Self {
            relationship: assessment.relationship,
            action: assessment.action,
            proof_strength: evidence
                .map(|evidence| evidence.proof_strength)
                .unwrap_or(assessment.proof_strength),
            ambiguity_count,
        }
    }

    fn relationship_priority(self) -> u8 {
        match self.relationship {
            MatchRelationship::Exact => 0,
            MatchRelationship::Probable => 1,
            MatchRelationship::Alternative => 2,
            MatchRelationship::Unresolved => 3,
        }
    }

    fn action_priority(self) -> u8 {
        match self.action {
            MatchAction::Accept => 0,
            MatchAction::Inspect => 1,
            MatchAction::Enrich => 2,
            MatchAction::Reject => 3,
        }
    }

    fn proof_priority(self) -> u8 {
        match self.proof_strength {
            ProofStrength::AudioFingerprint => 0,
            ProofStrength::RecordingIdentifier => 1,
            ProofStrength::MetadataOnly => 2,
        }
    }
}
fn assessment(
    store: &Store,
    track: &Track,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
) -> MatchAssessment {
    crate::recording_match_context::assess_with_aliases(
        store,
        track,
        candidate,
        level,
        stage,
        aliases,
        isrc_aliases,
        known_isrc,
    )
}
pub(super) fn evaluate(
    track: &Track,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    candidate: &Candidate,
    level: MatchLevel,
    store: &Store,
) -> CandidateGateResult {
    let a = assessment(
        store,
        track,
        aliases,
        isrc_aliases,
        known_isrc,
        candidate,
        level,
        MatchStage::Search,
    );
    let path = if !matches!(a.action, MatchAction::Inspect | MatchAction::Accept) {
        CandidateGatePath::Rejected
    } else {
        // Keep ranking tied to the shared assessment's relationship/action facts.
        // In particular, a numerically weaker alternative may be inspected or
        // accepted in Flexible mode, but cannot outrank an exact same recording.
        match a.relationship {
            MatchRelationship::Exact => CandidateGatePath::PolicyExact,
            MatchRelationship::Probable => CandidateGatePath::PolicyProbable,
            MatchRelationship::Alternative => CandidateGatePath::PolicyAlternative,
            MatchRelationship::Unresolved => CandidateGatePath::PolicyInspect,
        }
    };
    CandidateGateResult {
        path,
        shared: SharedEvidenceRank::from_assessment(&a),
    }
}
pub(super) fn priority(
    _track: &Track,
    candidate: &Candidate,
    result: CandidateGateResult,
    _level: MatchLevel,
) -> (u8, u8, u8, u16, u8, u8) {
    let topic_priority =
        if crate::source_fallback::is_exact_versioned_topic_candidate(_track, candidate) {
            0
        } else {
            1
        };
    shared_priority(result.shared, result.path, topic_priority)
}

fn shared_priority(
    shared: SharedEvidenceRank,
    path: CandidateGatePath,
    topic_priority: u8,
) -> (u8, u8, u8, u16, u8, u8) {
    (
        shared.relationship_priority(),
        shared.action_priority(),
        shared.proof_priority(),
        shared.ambiguity_count,
        path.evidence_priority(),
        topic_priority,
    )
}
pub(super) fn accept(
    track: &Track,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
    level: MatchLevel,
    store: &Store,
) -> bool {
    verify(
        track,
        aliases,
        isrc_aliases,
        known_isrc,
        candidate,
        audio,
        required,
        level,
        store,
    )
    .is_some()
}
pub(super) fn verify(
    track: &Track,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
    level: MatchLevel,
    store: &Store,
) -> Option<VerifiedRecordingProof> {
    verify_with_discogs_credit_variant(
        track,
        aliases,
        isrc_aliases,
        known_isrc,
        candidate,
        audio,
        required,
        level,
        store,
        None,
    )
}

pub(super) fn verify_with_discogs_credit_variant(
    track: &Track,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
    level: MatchLevel,
    store: &Store,
    discogs_credit_variant: Option<&crate::recording_identity::VerifiedArtistCreditSetVariant>,
) -> Option<VerifiedRecordingProof> {
    let mut resolved = candidate.clone();
    // Public Bandcamp links carry a page URL before its numeric track ID is
    // available. Bind that ID only after the page resolver returns it.
    if resolved.provider == "bandcamp" && resolved.id.is_empty() {
        crate::bandcamp::validate_track_page_url(&resolved.url).ok()?;
        let id = audio.resolved_provider_id.as_deref()?;
        if id.is_empty()
            || id.len() > 20
            || !id.bytes().all(|b| b.is_ascii_digit())
            || id.parse::<u64>().ok()? == 0
        {
            return None;
        }
        resolved.id = format!("bandcamp:{id}");
    }
    let extractor = matches!(
        candidate.provider.as_str(),
        "youtube" | "soundcloud" | "bandcamp"
    );
    let expected_id = resolved.id.split_once(':')?.1.to_owned();
    if audio
        .resolved_provider_id
        .as_deref()
        .is_some_and(|id| id != expected_id)
    {
        return None;
    }
    if extractor {
        if audio.resolved_provider_id.as_deref() != Some(expected_id.as_str())
            || audio.resolved_title.as_deref().is_none_or(str::is_empty)
        {
            return None;
        }
        resolved.source_metadata = audio.source_metadata.clone();
        resolved.title = audio.resolved_title.clone()?;
        resolved.duration_seconds = audio.duration_seconds;
        resolved.uploader = audio
            .source_metadata
            .as_ref()
            .and_then(|m| m["channel"].as_str().or(m["uploader"].as_str()))
            .map(str::to_owned);
    }
    if let Some(chapters) = &audio.chapters {
        resolved.chapters = Some(chapters.clone());
    }
    if !required {
        *candidate = resolved;
        return Some(VerifiedRecordingProof {
            kind: RecordingProofKind::ExplicitSource,
        });
    }
    let a = if let Some(variant) = discogs_credit_variant {
        crate::recording_match_context::assess_with_artist_credit_set_variant(
            store,
            track,
            &resolved,
            level,
            MatchStage::Resolved,
            aliases,
            isrc_aliases,
            known_isrc,
            variant,
        )
    } else {
        assessment(
            store,
            track,
            aliases,
            isrc_aliases,
            known_isrc,
            &resolved,
            level,
            MatchStage::Resolved,
        )
    };
    if a.action != MatchAction::Accept {
        return None;
    }
    let metadata = resolved.source_metadata.get_or_insert_with(|| json!({}));
    if !metadata.is_object() {
        *metadata = json!({});
    }
    let artist = metadata["artist"].as_str().map(str::to_owned).or_else(|| {
        metadata["artistNames"]
            .as_array()
            .or_else(|| metadata["artists"].as_array())
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
    });
    metadata["matching"] = json!({"evaluatorVersion":VERSION,"level":level,"assessment":a,"resolvedRecording":{"title":resolved.title,"artist":artist,"version":crate::recording_identity::parse_title(&resolved.title).version,"sourceId":resolved.id}});
    // Persist observations without stream URLs, credentials or raw provider responses.
    if let Some(identity) = recording_identity_fingerprint(track) {
        let record = crate::storage::MatchingEvidenceRecord {
            schema_version: 1,
            evaluator_version: VERSION.into(),
            requested_track_id: track.id.clone(),
            requested_fingerprint: identity,
            source_provider: resolved.provider.clone(),
            source_id: expected_id.into(),
            policy: level,
            relationship: a.relationship,
            evidence: json!({"matching":metadata["matching"],"title":resolved.title,"artist":metadata["artist"],"artists":metadata["artists"],"artistNames":metadata["artistNames"],"artistCount":metadata["artistCount"],"version":metadata["version"],"track":metadata["track"],"duration":resolved.duration_seconds,"isrc":metadata["isrc"],"identityMetadata":bounded_identity_metadata(metadata)}),
        };
        let _ = store.save_matching_evidence(&record);
    }
    *candidate = resolved;
    Some(VerifiedRecordingProof {
        kind: if extractor {
            RecordingProofKind::VerifiedExtractor
        } else {
            RecordingProofKind::Recording
        },
    })
}

pub(super) fn request_fingerprint(track: &Track, level: MatchLevel) -> Option<String> {
    let evidence = json!({"requestedId":track.id,"requestedProvider":track.provider,"requestedProviderId":track.provider_id,"legacy":recording_identity_fingerprint(track)?,"identity":crate::recording_identity::RecordingIdentity::from_track(track),"album":track.album,"label":track.label});
    Some(fingerprint(&evidence.to_string(), level))
}
pub(super) fn restore_evidence(
    store: &Store,
    track: &Track,
    candidate: &mut Candidate,
    level: MatchLevel,
) {
    let Some(identity) = recording_identity_fingerprint(track) else {
        return;
    };
    let Some((_, id)) = candidate.id.split_once(':') else {
        return;
    };
    let Ok(Some(record)) = store.get_matching_evidence(
        &track.id,
        &identity,
        &candidate.provider,
        id,
        VERSION,
        level,
    ) else {
        return;
    };
    let e = &record.evidence;
    if let Some(title) = e["title"].as_str() {
        candidate.title = title.into();
    }
    candidate.duration_seconds = e["duration"].as_f64().or(candidate.duration_seconds);
    let metadata = candidate.source_metadata.get_or_insert_with(|| json!({}));
    if !metadata.is_object() {
        *metadata = json!({});
    }
    if let Some(identity_metadata) = e["identityMetadata"].as_object() {
        for (field, value) in identity_metadata {
            metadata[field] = value.clone();
        }
    }
    for field in [
        "artist",
        "artists",
        "artistNames",
        "artistCount",
        "version",
        "track",
        "isrc",
        "matching",
    ] {
        if !e[field].is_null() {
            metadata[field] = e[field].clone();
        }
    }
}

/// Preserve only the small, structured fields the evaluator needs to reproduce
/// a fresh decision. Raw provider payloads, URLs, credentials and large text
/// blobs never enter this record.
fn bounded_identity_metadata(metadata: &Value) -> Value {
    let mut result = serde_json::Map::new();
    for field in [
        "provider",
        "artist",
        "artists",
        "artistNames",
        "artistCount",
        "channel",
        "channelId",
        "channelIsVerified",
        "catalog",
        "catalogTitle",
        "catalogArtist",
        "descriptionCreditLine",
        "duration",
        "isPreview",
        "isrc",
        "label",
        "mix",
        "playbackMusicCatalogCandidate",
        "recordingId",
        "title",
        "track",
        "trackId",
        "version",
        "workId",
    ] {
        let Some(value) = metadata.get(field) else {
            continue;
        };
        let bounded = match value {
            Value::String(text)
                if text.chars().count() <= 1024 && !text.chars().any(char::is_control) =>
            {
                Some(value.clone())
            }
            Value::Array(values) if values.len() <= 32 => {
                let items = values
                    .iter()
                    .filter(|item| {
                        item.as_str().is_some_and(|text| {
                            text.chars().count() <= 256 && !text.chars().any(char::is_control)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Some(Value::Array(items))
            }
            Value::Bool(_) | Value::Number(_) => Some(value.clone()),
            _ => None,
        };
        if let Some(value) = bounded {
            result.insert(field.to_owned(), value);
        }
    }
    Value::Object(result)
}
pub(super) fn cached_direct_allowed(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
) -> bool {
    let Some(m) = candidate
        .source_metadata
        .as_ref()
        .and_then(|m| m.get("matching"))
    else {
        return false;
    };
    if m["evaluatorVersion"] != VERSION || m["level"] != json!(level) {
        return false;
    }
    let a = crate::recording_match_context::assess(
        store,
        track,
        candidate,
        level,
        MatchStage::Resolved,
    );
    a.action == MatchAction::Accept && a.relationship != MatchRelationship::Alternative
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relationship_tier_precedes_topic_preference() {
        let alternative_topic = SharedEvidenceRank {
            relationship: MatchRelationship::Alternative,
            action: MatchAction::Inspect,
            proof_strength: ProofStrength::MetadataOnly,
            ambiguity_count: 0,
        };
        let exact_generic = SharedEvidenceRank {
            relationship: MatchRelationship::Exact,
            action: MatchAction::Accept,
            proof_strength: ProofStrength::MetadataOnly,
            ambiguity_count: 0,
        };
        assert!(
            shared_priority(exact_generic, CandidateGatePath::PolicyExact, 1,)
                < shared_priority(alternative_topic, CandidateGatePath::PolicyAlternative, 0,)
        );
        assert!(
            shared_priority(exact_generic, CandidateGatePath::PolicyExact, 0,)
                < shared_priority(exact_generic, CandidateGatePath::PolicyExact, 1,)
        );
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("test.sqlite")).unwrap();
        (dir, s)
    }

    #[test]
    fn shared_verifier_uses_only_exact_discogs_credit_variation_for_caption() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../fixtures/discogs-prolific-credit-identity.json"
        ))
        .unwrap();
        let track = crate::artists::parse_discogs_release_tracks(
            &fixture["release"],
            "38549214",
            "https://www.discogs.com/release/38549214",
            false,
            None,
        )
        .remove(0);
        let (directory, store) = store();
        store
            .set_retained_provider_cache(
                crate::artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE,
                "38549214",
                &fixture["release"],
            )
            .unwrap();
        for profile in fixture["profiles"].as_array().unwrap() {
            store
                .set_retained_provider_cache(
                    crate::artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE,
                    &profile["id"].as_u64().unwrap().to_string(),
                    profile,
                )
                .unwrap();
        }
        let mut aliases =
            crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, &track);
        let title = fixture["officialTitle"].as_str().unwrap();
        let candidate = || Candidate {
            provider: "youtube".into(),
            id: "youtube:vqDoiUMuqQA".into(),
            title: title.into(),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let audio = |resolved_title: &str| acquisition::PreviewAudio {
            provider_session: None,
            url: "https://example.invalid/audio".into(),
            mime_type: "audio/mp4".into(),
            codec: Some("aac".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(180.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "channel":"Nipsey Hussle",
                "channelId":"UC1234567890123456789012",
                "channelIsVerified":true
            })),
            resolved_title: Some(resolved_title.to_owned()),
            resolved_provider_id: Some("vqDoiUMuqQA".into()),
            chapters: Some(vec![]),
            selected_format_id: Some("140".into()),
            selected_format_has_video: false,
            expected_content_length: None,
            headers: vec![],
        };
        let mut matched_candidate = candidate();
        super::super::include_candidate_aliases(
            &mut aliases,
            &track,
            &matched_candidate,
            Some(&audio(title)),
        );
        let typed_alias = super::super::discogs_credit_variant_for_fresh_caption(
            &aliases,
            &track,
            &matched_candidate,
            &audio(title),
        )
        .expect("the full caption should match the exact cached Discogs credit set");
        assert_eq!(
            typed_alias.requested_credits,
            vec!["Nipsey Hussle", "Bino Rideaux", "James Fauntleroy II"]
        );
        assert_eq!(
            typed_alias.candidate_credits,
            vec!["Nipsey Hussle", "Bino Rideaux", "James Fauntleroy"]
        );
        let proof = verify_with_discogs_credit_variant(
            &track,
            aliases.tracks(),
            aliases.isrc_enriched(),
            aliases.verified_recording_isrc(),
            &mut matched_candidate,
            &audio(title),
            true,
            MatchLevel::Balanced,
            &store,
            Some(&typed_alias),
        );
        assert!(proof.is_some());
        let matched = crate::recording_match_context::assess_with_artist_credit_set_variant(
            &store,
            &track,
            &matched_candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
            aliases.tracks(),
            aliases.isrc_enriched(),
            aliases.verified_recording_isrc(),
            &typed_alias,
        );
        assert_eq!(matched.relationship, MatchRelationship::Probable);
        assert_eq!(matched.action, MatchAction::Accept);
        assert_eq!(matched.proof_strength, ProofStrength::MetadataOnly);
        let evidence = matched.evidence.unwrap();
        assert_eq!(evidence.dimensions.runtime, Comparison::Unknown);
        assert_eq!(
            evidence
                .scoped_artist_credit_set_alias
                .unwrap()
                .evidence_scope,
            "exactDiscogsReleaseArtistNameVariationsOnly"
        );
        assert!(track.duration_seconds.is_none());

        for rejected_title in [
            "Nipsey Hussle and Bino Rideaux (feat. Other Guest) - Sacrifices (Official Audio)",
            "Nipsey Hussle and Bino Rideaux - Sacrifices (Official Audio)",
        ] {
            let mut rejected_candidate = candidate();
            let rejected_audio = audio(rejected_title);
            super::super::include_candidate_aliases(
                &mut aliases,
                &track,
                &rejected_candidate,
                Some(&rejected_audio),
            );
            let variant = super::super::discogs_credit_variant_for_fresh_caption(
                &aliases,
                &track,
                &rejected_candidate,
                &rejected_audio,
            );
            assert!(variant.is_none());
            assert!(verify_with_discogs_credit_variant(
                &track,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
                &mut rejected_candidate,
                &rejected_audio,
                true,
                MatchLevel::Balanced,
                &store,
                variant.as_ref(),
            )
            .is_none());
        }

        let mut manual = track.clone();
        manual.provenance["manualFields"] = json!({"artist":true});
        let manual_aliases =
            crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, &manual);
        assert!(super::super::discogs_credit_variant_for_fresh_caption(
            &manual_aliases,
            &manual,
            &candidate(),
            &audio(title),
        )
        .is_none());
        let mut manual_candidate = candidate();
        assert!(verify_with_discogs_credit_variant(
            &manual,
            manual_aliases.tracks(),
            manual_aliases.isrc_enriched(),
            manual_aliases.verified_recording_isrc(),
            &mut manual_candidate,
            &audio(title),
            true,
            MatchLevel::Balanced,
            &store,
            None,
        )
        .is_none());
        drop(directory);
    }
    fn request() -> Track {
        Track {
            id: "spotify:abcdefghijklmnopqrstuv".into(),
            provider: "spotify".into(),
            provider_id: "abcdefghijklmnopqrstuv".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            duration_seconds: Some(180.),
            provenance: json!({}),
            ..Track::default()
        }
    }
    fn candidate() -> Candidate {
        Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Song".into(),
            duration_seconds: Some(180.),
            source_metadata: Some(json!({"artists":["Artist"],"artistCount":1})),
            ..Candidate::default()
        }
    }
    #[test]
    fn flexible_shared_rank_keeps_same_recording_ahead_of_version_alternative() {
        let (_dir, store) = store();
        let track = Track {
            version: Some("Original Mix".into()),
            ..request()
        };
        let mut same = candidate();
        same.title = "Song (Original Mix)".into();
        let mut alternative = candidate();
        alternative.title = "Song (Alice Remix)".into();

        let same_assessment = assessment(
            &store,
            &track,
            &[],
            &[],
            None,
            &same,
            MatchLevel::Flexible,
            MatchStage::Search,
        );
        let alternative_assessment = assessment(
            &store,
            &track,
            &[],
            &[],
            None,
            &alternative,
            MatchLevel::Flexible,
            MatchStage::Search,
        );

        assert_eq!(same_assessment.relationship, MatchRelationship::Exact);
        assert_eq!(
            alternative_assessment.relationship,
            MatchRelationship::Alternative
        );
        assert_eq!(alternative_assessment.action, MatchAction::Inspect);
        let same_rank = SharedEvidenceRank::from_assessment(&same_assessment);
        let alternative_rank = SharedEvidenceRank::from_assessment(&alternative_assessment);
        assert!(
            shared_priority(same_rank, CandidateGatePath::PolicyExact, 1)
                < shared_priority(alternative_rank, CandidateGatePath::PolicyAlternative, 0)
        );
    }

    #[test]
    fn competing_versions_remain_alternatives_and_missing_version_stays_unknown() {
        let (_dir, store) = store();
        let request_without_version = request();
        let canonical = candidate();
        let mut alice_remix = candidate();
        alice_remix.title = "Song (Alice Remix)".into();
        let mut bob_remix = candidate();
        bob_remix.title = "Song (Bob Remix)".into();
        let probable = Candidate {
            id: "monochrome:catalog-1".into(),
            provider: "monochrome".into(),
            title: "Song".into(),
            duration_seconds: Some(185.0),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"catalog-1",
                "catalogTitle":"Song",
                "artistNames":["Artist"],
                "artistCount":1,
                "duration":185.0
            })),
            ..Candidate::default()
        };

        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let canonical_assessment = assessment(
                &store,
                &request_without_version,
                &[],
                &[],
                None,
                &canonical,
                level,
                MatchStage::Resolved,
            );
            let alice_assessment = assessment(
                &store,
                &request_without_version,
                &[],
                &[],
                None,
                &alice_remix,
                level,
                MatchStage::Resolved,
            );
            let bob_assessment = assessment(
                &store,
                &request_without_version,
                &[],
                &[],
                None,
                &bob_remix,
                level,
                MatchStage::Resolved,
            );
            let probable_assessment = assessment(
                &store,
                &request_without_version,
                &[],
                &[],
                None,
                &probable,
                level,
                MatchStage::Resolved,
            );

            assert_eq!(canonical_assessment.relationship, MatchRelationship::Exact);
            assert_ne!(alice_assessment.relationship, MatchRelationship::Exact);
            assert_ne!(bob_assessment.relationship, MatchRelationship::Exact);
            if level == MatchLevel::Flexible {
                assert_eq!(
                    alice_assessment.relationship,
                    MatchRelationship::Alternative
                );
                assert_eq!(bob_assessment.relationship, MatchRelationship::Alternative);
            } else {
                assert_eq!(alice_assessment.action, MatchAction::Reject);
                assert_eq!(bob_assessment.action, MatchAction::Reject);
            }
            if level == MatchLevel::Balanced || level == MatchLevel::Flexible {
                assert_eq!(
                    probable_assessment.relationship,
                    MatchRelationship::Probable
                );
                let probable_rank = SharedEvidenceRank::from_assessment(&probable_assessment);
                assert!(
                    shared_priority(probable_rank, CandidateGatePath::PolicyProbable, 1)
                        < shared_priority(
                            SharedEvidenceRank::from_assessment(&alice_assessment),
                            CandidateGatePath::PolicyAlternative,
                            1,
                        )
                );
            }

            let canonical_rank = SharedEvidenceRank::from_assessment(&canonical_assessment);
            let alice_rank = SharedEvidenceRank::from_assessment(&alice_assessment);
            let bob_rank = SharedEvidenceRank::from_assessment(&bob_assessment);
            assert!(
                shared_priority(canonical_rank, CandidateGatePath::PolicyExact, 1)
                    < shared_priority(alice_rank, CandidateGatePath::PolicyAlternative, 0)
            );
            // Neither named remix has stronger request-specific evidence; don't
            // let a legacy numeric score manufacture a preferred edition.
            assert_eq!(
                shared_priority(alice_rank, CandidateGatePath::PolicyAlternative, 1),
                shared_priority(bob_rank, CandidateGatePath::PolicyAlternative, 1)
            );
        }

        let mut request_for_live = request_without_version;
        request_for_live.version = Some("Live".into());
        let missing_version = candidate();
        let mut conflicting_version = candidate();
        conflicting_version.title = "Song (Remix)".into();
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let missing = assessment(
                &store,
                &request_for_live,
                &[],
                &[],
                None,
                &missing_version,
                level,
                MatchStage::Resolved,
            );
            let conflict = assessment(
                &store,
                &request_for_live,
                &[],
                &[],
                None,
                &conflicting_version,
                level,
                MatchStage::Resolved,
            );
            assert_eq!(missing.dimensions.version, Comparison::Unknown);
            assert_ne!(missing.dimensions.version, Comparison::Conflict);
            assert_eq!(conflict.dimensions.version, Comparison::Conflict);
            if level == MatchLevel::Flexible {
                assert_eq!(missing.relationship, MatchRelationship::Probable);
                assert_eq!(conflict.relationship, MatchRelationship::Alternative);
            } else {
                assert_eq!(missing.action, MatchAction::Enrich);
                assert_eq!(conflict.action, MatchAction::Reject);
            }
        }
    }

    fn audio(title: &str, names: Value) -> acquisition::PreviewAudio {
        acquisition::PreviewAudio {
            provider_session: None,
            url: "https://example.invalid/audio".into(),
            mime_type: "audio/mp4".into(),
            codec: Some("aac".into()),
            bitrate_kbps: None,
            duration_seconds: Some(180.),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: Some(
                json!({"provider":"youtube","artists":names,"artistCount":1,"track":title}),
            ),
            resolved_title: Some(title.into()),
            resolved_provider_id: Some("abcdefghijk".into()),
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
        }
    }
    #[test]
    fn explicit_bandcamp_page_binds_numeric_source_before_playback() {
        let (_dir, s) = store();
        let mut c = Candidate {
            provider: "bandcamp".into(),
            url: "https://artist.bandcamp.com/track/song".into(),
            ..Default::default()
        };
        let mut a = audio("Song", json!(["Artist"]));
        a.resolved_provider_id = Some("123456".into());
        assert!(verify(
            &request(),
            &[],
            &[],
            None,
            &mut c,
            &a,
            false,
            MatchLevel::Balanced,
            &s
        )
        .is_some());
        assert_eq!(c.id, "bandcamp:123456");
        a.resolved_provider_id = Some("not-a-track-id".into());
        c.id.clear();
        assert!(verify(
            &request(),
            &[],
            &[],
            None,
            &mut c,
            &a,
            false,
            MatchLevel::Balanced,
            &s
        )
        .is_none());
    }

    #[test]
    fn fresh_evidence_replaces_search_identity_and_binds_provider_id() {
        let (_dir, s) = store();
        let t = request();
        let mut c = candidate();
        assert!(verify(
            &t,
            &[],
            &[],
            None,
            &mut c,
            &audio("Song", json!(["Wrong Artist"])),
            true,
            MatchLevel::Balanced,
            &s
        )
        .is_none());
        let mut a = audio("Song", json!(["Artist"]));
        a.resolved_provider_id = Some("differentid".into());
        assert!(verify(
            &t,
            &[],
            &[],
            None,
            &mut c,
            &a,
            false,
            MatchLevel::Flexible,
            &s
        )
        .is_none());
        assert!(verify(
            &t,
            &[],
            &[],
            None,
            &mut c,
            &audio("Song", json!(["Artist"])),
            true,
            MatchLevel::Balanced,
            &s
        )
        .is_some());
        assert_eq!(
            c.source_metadata.as_ref().unwrap()["matching"]["level"],
            "balanced"
        );
    }
    #[test]
    fn fresh_and_reopened_evidence_produce_the_same_balanced_decision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fresh-restored-evidence.sqlite");
        let track = request();
        let fresh_store = Store::open(&path).unwrap();
        let mut fresh = candidate();
        assert!(verify(
            &track,
            &[],
            &[],
            None,
            &mut fresh,
            &audio("Song", json!(["Artist"])),
            true,
            MatchLevel::Balanced,
            &fresh_store,
        )
        .is_some());
        let fresh_decision = assessment(
            &fresh_store,
            &track,
            &[],
            &[],
            None,
            &fresh,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        drop(fresh_store);

        let reopened = Store::open(&path).unwrap();
        let mut restored = candidate();
        restore_evidence(&reopened, &track, &mut restored, MatchLevel::Balanced);
        let restored_decision = assessment(
            &reopened,
            &track,
            &[],
            &[],
            None,
            &restored,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(restored_decision, fresh_decision);
    }

    #[test]
    fn alternative_cache_cannot_be_reused_at_exact_and_keeps_actual_title() {
        let (_dir, s) = store();
        let t = request();
        let mut c = candidate();
        let mut a = audio("Song (Live)", json!(["Artist"]));
        a.duration_seconds = Some(240.);
        assert!(verify(
            &t,
            &[],
            &[],
            None,
            &mut c,
            &a,
            true,
            MatchLevel::Flexible,
            &s
        )
        .is_some());
        assert_eq!(
            c.source_metadata.as_ref().unwrap()["matching"]["assessment"]["relationship"],
            "alternative"
        );
        assert_eq!(c.title, "Song (Live)");
        let identity = recording_identity_fingerprint(&t).unwrap();
        assert!(s
            .get_matching_evidence(
                &t.id,
                &identity,
                "youtube",
                "abcdefghijk",
                VERSION,
                MatchLevel::Exact
            )
            .unwrap()
            .is_none());
        assert_ne!(
            request_fingerprint(&t, MatchLevel::Exact),
            request_fingerprint(&t, MatchLevel::Flexible)
        );
        assert!(!cached_direct_allowed(&s, &t, &c, MatchLevel::Exact));
        assert_eq!(t.title, "Song");
    }
    #[test]
    fn scoped_profile_alias_keeps_added_ensemble_credit_as_unresolved_metadata() {
        let (_dir, s) = store();
        s.set_provider_cache("artists.profile.musicbrainz","612d1573-bdea-4657-bfc9-f78593c9ab09",&json!({"id":"612d1573-bdea-4657-bfc9-f78593c9ab09","name":"Lakis Halkias","aliases":[{"name":"Lakis Chalkias"}]}),3600).unwrap();
        let t = Track {
            title: "To Miroloi Tis Manas".into(),
            artist: "Lakis Chalkias".into(),
            duration_seconds: Some(127.626),
            ..request()
        };
        let c = Candidate {
            id: "monochrome:177409920331984896".into(),
            provider: "monochrome".into(),
            title: t.title.clone(),
            duration_seconds: Some(127.626),
            source_metadata: Some(
                json!({"provider":"monochrome","trackId":"177409920331984896","catalogTitle":t.title,"artistNames":["Lakis Halkias"]}),
            ),
            ..Candidate::default()
        };
        let single_credit = assessment(
            &s,
            &t,
            &[],
            &[],
            None,
            &c,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(single_credit.action, MatchAction::Accept);
        let alias_evidence = single_credit
            .evidence
            .as_ref()
            .unwrap()
            .scoped_artist_name_alias
            .as_ref()
            .unwrap();
        assert_eq!(alias_evidence.requested_name, "Lakis Chalkias");
        assert_eq!(alias_evidence.candidate_primary_name, "Lakis Halkias");
        assert_eq!(
            alias_evidence.musicbrainz_artist_id,
            "612d1573-bdea-4657-bfc9-f78593c9ab09"
        );
        assert_eq!(single_credit.proof_strength, ProofStrength::MetadataOnly);
        let added_ensemble = Candidate {
            source_metadata: Some(
                json!({"provider":"monochrome","trackId":"177409920331984896","catalogTitle":t.title,"artistNames":["Lakis Halkias","Dimotiko Paradosiako Sygrotima Elatos"]}),
            ),
            ..c.clone()
        };
        for (level, expected_action) in [
            (MatchLevel::Exact, MatchAction::Inspect),
            (MatchLevel::Balanced, MatchAction::Accept),
            (MatchLevel::Flexible, MatchAction::Accept),
        ] {
            let checked = assessment(
                &s,
                &t,
                &[],
                &[],
                None,
                &added_ensemble,
                level,
                MatchStage::Resolved,
            );
            assert_eq!(checked.action, expected_action, "{level:?}: {checked:?}");
            assert_eq!(
                checked.relationship,
                MatchRelationship::Probable,
                "{level:?}: {checked:?}"
            );
            assert!(
                checked
                    .evidence
                    .as_ref()
                    .unwrap()
                    .scoped_artist_name_alias
                    .is_some(),
                "{level:?}: {checked:?}"
            );
            assert_eq!(checked.proof_strength, ProofStrength::MetadataOnly);
            assert!(checked
                .evidence
                .as_ref()
                .unwrap()
                .extra_performers
                .contains(&"dimotiko paradosiako sygrotima elatos".to_owned()));
        }
        assert_eq!(
            assessment(
                &s,
                &t,
                &[],
                &[],
                None,
                &c,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Inspect
        );
        assert_eq!(t.artist, "Lakis Chalkias");
    }
}

const LOCATORS: &str = crate::storage::PLAYBACK_LOCATOR_CACHE_NAMESPACE;
pub(super) fn locator(
    store: &Store,
    track: &Track,
    key: &str,
    provider: &str,
) -> Option<crate::storage::VerifiedPlaybackSource> {
    let value = store
        .get_provider_cache(LOCATORS, &format!("{key}:{provider}"))
        .ok()??;
    let level: MatchLevel = serde_json::from_value(value["matchingLevel"].clone()).ok()?;
    if value["schemaVersion"].as_u64() != Some(2)
        || value["evaluatorVersion"].as_str() != Some(VERSION)
        || value["requestFingerprint"].as_str() != Some(key)
        || request_fingerprint(track, level).as_deref() != Some(key)
        || value["provider"].as_str() != Some(provider)
        || !value["proof"].is_object()
    {
        return None;
    }
    let source = crate::storage::VerifiedPlaybackSource {
        provider: provider.into(),
        provider_id: value["providerId"].as_str()?.into(),
        permanent_url: value["permanentUrl"].as_str()?.into(),
        verification_proof: Some(value["proof"].clone()),
    };
    remembered_candidate(track, &source)?;
    Some(source)
}
pub(super) fn save_locator(
    store: &Store,
    track: &Track,
    key: &str,
    provider: &str,
    id: &str,
    url: &str,
    proof: &Value,
) {
    let Some(level) = [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ]
    .into_iter()
    .find(|level| request_fingerprint(track, *level).as_deref() == Some(key)) else {
        return;
    };
    let source = crate::storage::VerifiedPlaybackSource {
        provider: provider.to_owned(),
        provider_id: id.to_owned(),
        permanent_url: url.to_owned(),
        verification_proof: Some(proof.clone()),
    };
    if !proof.is_object() || remembered_candidate(track, &source).is_none() {
        return;
    }
    let _ = store.set_retained_playback_locator(
        &format!("{key}:{provider}"),
        &json!({"schemaVersion":2,"evaluatorVersion":VERSION,"matchingLevel":level,"requestFingerprint":key,
            "provider":provider,"providerId":id,"permanentUrl":url,"proof":proof}),
    );
}
pub(super) fn invalidate_locator(
    store: &Store,
    track: &Track,
    key: &str,
    provider: &str,
    id: &str,
) {
    let _ = store.set_provider_cache(LOCATORS, &format!("{key}:{provider}"), &Value::Null, 0);
    if let Some(legacy) = recording_identity_fingerprint(track) {
        let _ = store.invalidate_verified_playback_source(&track.id, &legacy, provider, id);
    }
}

#[cfg(test)]
mod retained_locator_tests {
    use super::*;
    fn request() -> Track {
        Track {
            id: "fixture:song".into(),
            provider: "fixture".into(),
            provider_id: "song".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            duration_seconds: Some(200.),
            ..Default::default()
        }
    }
    fn write(store: &Store, track: &Track, level: MatchLevel) -> String {
        let key = request_fingerprint(track, level).unwrap();
        save_locator(
            store,
            track,
            &key,
            "youtube",
            "abcdefghijk",
            "https://www.youtube.com/watch?v=abcdefghijk",
            &json!({"version":1,"kind":"recording"}),
        );
        key
    }
    #[test]
    fn locator_roundtrip_survives_restart_without_becoming_a_direct_acceptance() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locator.sqlite");
        let store = Store::open(&path).unwrap();
        let track = request();
        let key = write(&store, &track, MatchLevel::Balanced);
        let stored = store
            .get_provider_cache(LOCATORS, &format!("{key}:youtube"))
            .unwrap()
            .unwrap();
        assert_eq!(stored["schemaVersion"], 2);
        assert_eq!(stored["evaluatorVersion"], VERSION);
        assert_eq!(stored["matchingLevel"], json!(MatchLevel::Balanced));
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let source = locator(&reopened, &track, &key, "youtube").unwrap();
        assert_eq!(source.provider_id, "abcdefghijk");
        let mut candidate = remembered_candidate(&track, &source).unwrap();
        restore_evidence(&reopened, &track, &mut candidate, MatchLevel::Balanced);
        assert!(!cached_direct_allowed(
            &reopened,
            &track,
            &candidate,
            MatchLevel::Balanced
        ));
        // A source locator locates an extraction; it never invents missing
        // performer evidence or carries a resolved-stream acceptance.
        assert_ne!(
            crate::recording_match::assess(
                &track,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
        invalidate_locator(&reopened, &track, &key, "youtube", "abcdefghijk");
        assert!(locator(&reopened, &track, &key, "youtube").is_none());
    }
    #[test]
    fn locator_policy_evaluator_and_request_binding_cannot_be_relabelled() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("locator.sqlite")).unwrap();
        let track = request();
        let flexible = write(&store, &track, MatchLevel::Flexible);
        let exact = request_fingerprint(&track, MatchLevel::Exact).unwrap();
        assert!(locator(&store, &track, &flexible, "youtube").is_some());
        assert!(locator(&store, &track, &exact, "youtube").is_none());
        let original = store
            .get_provider_cache(LOCATORS, &format!("{flexible}:youtube"))
            .unwrap()
            .unwrap();
        let mut relabelled = original.clone();
        relabelled["requestFingerprint"] = json!(exact);
        store
            .set_retained_playback_locator(&format!("{exact}:youtube"), &relabelled)
            .unwrap();
        assert!(locator(&store, &track, &exact, "youtube").is_none());
        for (field, value) in [
            ("evaluatorVersion", json!("obsolete")),
            ("schemaVersion", json!(1)),
            ("matchingLevel", json!(MatchLevel::Exact)),
            ("provider", json!("soundcloud")),
            ("proof", Value::Null),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            store
                .set_retained_playback_locator(&format!("{flexible}:youtube"), &changed)
                .unwrap();
            assert!(
                locator(&store, &track, &flexible, "youtube").is_none(),
                "{field}"
            );
        }
        store
            .set_retained_playback_locator(&format!("{flexible}:youtube"), &original)
            .unwrap();
        let mut edited = track.clone();
        edited.provenance = json!({"remoteCatalog":{"isrc":"USAAA2400001"}});
        assert!(locator(&store, &edited, &flexible, "youtube").is_none());
        let mut edited = track.clone();
        edited.title = "Other Song".into();
        assert!(locator(&store, &edited, &flexible, "youtube").is_none());
    }
    #[test]
    fn locator_writer_rejects_invalid_source_and_stale_request_fingerprints() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("locator.sqlite")).unwrap();
        let track = request();
        let key = request_fingerprint(&track, MatchLevel::Balanced).unwrap();
        let proof = json!({"version":1,"kind":"recording"});
        save_locator(
            &store,
            &track,
            &key,
            "youtube",
            "abcdefghijk",
            "https://www.youtube.com/watch?v=other_id123",
            &proof,
        );
        assert!(locator(&store, &track, &key, "youtube").is_none());
        let mut edited = track.clone();
        edited.artist = "Other Artist".into();
        save_locator(
            &store,
            &edited,
            &key,
            "youtube",
            "abcdefghijk",
            "https://www.youtube.com/watch?v=abcdefghijk",
            &proof,
        );
        assert!(store
            .get_provider_cache(LOCATORS, &format!("{key}:youtube"))
            .unwrap()
            .is_none());
    }
}
