//! Playback orchestration for the shared policy evaluator. Transport verification
//! stays here; provider-specific matching decisions belong to recording_match.
use super::*;
use crate::recording_match::{
    self, MatchAction, MatchAssessment, MatchLevel, MatchRelationship, MatchStage,
};
pub(super) const VERSION: &str = crate::recording_match::EVALUATOR_VERSION;

pub(super) fn fingerprint(identity: &str, level: MatchLevel) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("recording-match:{VERSION}:{level:?}:{identity}").as_bytes())
    )
}
fn rank(a: &MatchAssessment) -> (u8, u8) {
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
) -> CandidateGatePath {
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
    if !matches!(a.action, MatchAction::Inspect | MatchAction::Accept) {
        return CandidateGatePath::Rejected;
    }
    match a.relationship {
        MatchRelationship::Exact => CandidateGatePath::PolicyExact,
        MatchRelationship::Probable => CandidateGatePath::PolicyProbable,
        MatchRelationship::Alternative => CandidateGatePath::PolicyAlternative,
        MatchRelationship::Unresolved => CandidateGatePath::PolicyInspect,
    }
}
pub(super) fn priority(
    _track: &Track,
    _candidate: &Candidate,
    path: CandidateGatePath,
    _level: MatchLevel,
) -> (u8, u8) {
    (path.evidence_priority(), 0)
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
    let a = assessment(
        store,
        track,
        aliases,
        isrc_aliases,
        known_isrc,
        &resolved,
        level,
        MatchStage::Resolved,
    );
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
            evidence: json!({"matching":metadata["matching"],"title":resolved.title,"artist":metadata["artist"],"artists":metadata["artists"],"artistNames":metadata["artistNames"],"artistCount":metadata["artistCount"],"version":metadata["version"],"track":metadata["track"],"duration":resolved.duration_seconds,"isrc":metadata["isrc"]}),
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
    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("test.sqlite")).unwrap();
        (dir, s)
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
    fn scoped_profile_alias_and_added_ensemble_are_balanced_evidence_only() {
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
                json!({"provider":"monochrome","trackId":"177409920331984896","catalogTitle":t.title,"artistNames":["Lakis Halkias","Dimotiko Paradosiako Sygrotima Elatos"]}),
            ),
            ..Candidate::default()
        };
        assert_eq!(
            assessment(
                &s,
                &t,
                &[],
                &[],
                None,
                &c,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
        assert_ne!(
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
            MatchAction::Accept
        );
        assert_eq!(t.artist, "Lakis Chalkias");
    }
}

const LOCATORS: &str = "playback.matching-locators.v1";
pub(super) fn locator(
    store: &Store,
    track: &Track,
    key: &str,
    provider: &str,
) -> Option<crate::storage::VerifiedPlaybackSource> {
    let value = store
        .get_provider_cache(LOCATORS, &format!("{key}:{provider}"))
        .ok()??;
    if value["provider"].as_str() != Some(provider) {
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
    key: &str,
    provider: &str,
    id: &str,
    url: &str,
    proof: &Value,
) {
    let _ = store.set_retained_provider_cache(
        LOCATORS,
        &format!("{key}:{provider}"),
        &json!({"provider":provider,"providerId":id,"permanentUrl":url,"proof":proof}),
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
