//! Explicit public-network regression for a two-pass verified YouTube mapping.
//! The first foreground resolve creates v2 proof through yt-dlp, the normal
//! strong identity gate and uncached access. The second exercises mapped
//! direct playback and verifies its access check leaves the original TTL intact.
use super::*;
use rusqlite::OptionalExtension;
use std::{path::Path, time::Instant};

fn safe_failure_code(error: &str) -> &'static str {
    use crate::playback_failure::FailureCategory as C;
    match crate::playback_failure::classify(error).category {
        C::Cancelled => "CANCELLED",
        C::Timeout => "TIMEOUT",
        C::Network => "NETWORK",
        C::RateLimited => "RATE_LIMITED",
        C::SourceAccess => "SOURCE_ACCESS",
        C::SourceUnavailable => "SOURCE_UNAVAILABLE",
        C::SourcePrivate => "SOURCE_PRIVATE",
        C::SourceDeleted => "SOURCE_DELETED",
        C::NoMatch => "NO_MATCH",
        C::RecordingIdentityMismatch => "IDENTITY_MISMATCH",
        C::UnsupportedAudio => "UNSUPPORTED_AUDIO",
        C::Output => "OUTPUT",
        C::Storage => "STORAGE",
        C::Unknown => "UNKNOWN",
    }
}

fn mapping_timestamps(database_path: &Path, track_id: &str, fingerprint: &str) -> (String, String) {
    let connection = rusqlite::Connection::open(database_path).unwrap();
    connection
        .query_row(
            "SELECT expires_at,updated_at FROM verified_playback_sources
             WHERE recording_id=(SELECT recording_id FROM catalog_listings WHERE id=?1)
               AND fingerprint=?2 AND provider='youtube'",
            rusqlite::params![track_id, fingerprint],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap()
        .expect("verified YouTube mapping row must exist")
}

#[tokio::test]
#[ignore = "explicit public-network opt-in; fresh catalog proof, mapped direct resolution and decoder PCM only"]
async fn live_mapped_direct_youtube_reuses_fresh_v2_catalog_mapping_without_refreshing_ttl() {
    assert_eq!(
        std::env::var("BURRATA_DIRECT_YOUTUBE_MAPPED_LIVE").as_deref(),
        Ok("1"),
        "requires explicit mapped live diagnostic opt-in"
    );
    assert_eq!(
        std::env::var("BURRATA_DIRECT_YOUTUBE_EXPERIMENT").as_deref(),
        Ok("1"),
        "requires explicit direct resolver opt-in"
    );

    // Reuse the existing catalog fixture, preserving its original 222.426s
    // duration. Live search may choose a different valid recording, so the
    // first fresh result below is the authority for the v2 anchors.
    let (track, _, _) = super::tests::matthew_catalog_verification_fixture();
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("mapped-direct-live.sqlite");
    let store = Arc::new(Store::open(&database_path).unwrap());
    store.save_track(&track).unwrap();
    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let manager = AcquisitionManager::new_for_playback_test(store.clone(), connections);
    let fingerprint = recording_identity_fingerprint(&track).expect("track fingerprint");
    let (_stop, cancel) = watch::channel(false);
    let started = Instant::now();
    let global_deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    // No mapping exists yet, so this necessarily uses the ordinary fresh
    // yt-dlp path even when the feature's runtime opt-in is enabled.
    let first_started = Instant::now();
    let first_deadline = PlaybackDeadline::at(
        (tokio::time::Instant::now() + Duration::from_secs(20)).min(global_deadline),
    );
    let first = tokio::time::timeout_at(
        global_deadline,
        resolve_with_trace_context_excluding_until_attempt_limit(
            &store,
            &manager,
            &track,
            None,
            cancel.clone(),
            Some("native-mapped-direct-live-first".into()),
            None,
            None,
            &[],
            first_deadline,
            2,
        ),
    )
    .await;
    let first_ms = first_started.elapsed().as_secs_f64() * 1000.0;
    let first = match first {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(error)) => {
            eprintln!(
                "{}",
                json!({"experiment":"mapped-direct-youtube-v2-live",
                "phase":"catalog_mapping_creation","ok":false,
                "failureCode":safe_failure_code(&error),"elapsedMs":first_ms})
            );
            panic!("fresh catalog mapping creation failed; fixed failure evidence retained");
        }
        Err(_) => {
            eprintln!(
                "{}",
                json!({"experiment":"mapped-direct-youtube-v2-live",
                "phase":"catalog_mapping_creation","ok":false,
                "failureCode":"GLOBAL_DEADLINE","elapsedMs":first_ms})
            );
            panic!("fresh catalog mapping creation exceeded the 60 second test deadline");
        }
    };
    if first.origin != ResolvedSourceOrigin::Existing {
        eprintln!(
            "{}",
            json!({"experiment":"mapped-direct-youtube-v2-live",
            "phase":"catalog_mapping_creation","ok":false,
            "failureCode":"FIRST_PASS_NOT_EXISTING","elapsedMs":first_ms})
        );
        panic!("first pass did not use the normal extractor path");
    }
    let first_candidate = first
        .candidate
        .as_ref()
        .expect("fresh resolution must return its selected candidate");
    let first_video_id = first_candidate
        .id
        .strip_prefix("youtube:")
        .expect("fresh candidate must be a YouTube source")
        .to_owned();
    let first_title = first_candidate.title.clone();
    let first_duration = first_candidate
        .duration_seconds
        .expect("fresh candidate must include resolved duration");
    drop(first);

    let mapping = match store.verified_playback_source(&track.id, &fingerprint, "youtube") {
        Ok(Some(mapping)) => mapping,
        _ => {
            eprintln!(
                "{}",
                json!({"experiment":"mapped-direct-youtube-v2-live",
                "phase":"catalog_mapping_creation","ok":false,
                "failureCode":"V2_MAPPING_MISSING","elapsedMs":first_ms})
            );
            panic!("fresh catalog mapping was not persisted");
        }
    };
    let video_id = mapping.provider_id.as_str();
    let anchors = parse_v2_youtube_recording_anchors(
        mapping.verification_proof.as_ref().unwrap_or(&Value::Null),
        video_id,
    );
    let anchors = match anchors {
        Some(anchors) => anchors,
        None => {
            eprintln!(
                "{}",
                json!({"experiment":"mapped-direct-youtube-v2-live",
                "phase":"catalog_mapping_creation","ok":false,
                "failureCode":"V2_PROOF_INVALID","elapsedMs":first_ms})
            );
            panic!("fresh mapping did not contain valid v2 proof anchors");
        }
    };
    assert_eq!(anchors.video_id, mapping.provider_id);
    assert_eq!(anchors.video_id, first_video_id);
    assert_eq!(anchors.title, first_title);
    assert!(anchors.duration_seconds.is_finite());
    assert_eq!(anchors.duration_seconds, first_duration);
    let before = mapping_timestamps(&database_path, &track.id, &fingerprint);
    let expires = chrono::DateTime::parse_from_rfc3339(&before.0).unwrap();
    let updated = chrono::DateTime::parse_from_rfc3339(&before.1).unwrap();
    assert_eq!(expires - updated, chrono::Duration::days(45));
    eprintln!(
        "{}",
        json!({"experiment":"mapped-direct-youtube-v2-live",
        "phase":"catalog_mapping_creation","ok":true,"origin":"existing",
        "proofVersion":2,"elapsedMs":first_ms})
    );

    let second_started = Instant::now();
    let second_deadline = PlaybackDeadline::at(
        (tokio::time::Instant::now() + Duration::from_secs(20)).min(global_deadline),
    );
    let second = tokio::time::timeout_at(
        global_deadline,
        resolve_with_trace_context_excluding_until_attempt_limit(
            &store,
            &manager,
            &track,
            None,
            cancel.clone(),
            Some("native-mapped-direct-live-second".into()),
            None,
            None,
            &[],
            second_deadline,
            2,
        ),
    )
    .await;
    let second_ms = second_started.elapsed().as_secs_f64() * 1000.0;
    let second = match second {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(error)) => {
            eprintln!(
                "{}",
                json!({"experiment":"mapped-direct-youtube-v2-live",
                "phase":"mapped_direct_resolution","ok":false,
                "failureCode":safe_failure_code(&error),"elapsedMs":second_ms})
            );
            panic!("mapped direct resolution failed; fixed failure evidence retained");
        }
        Err(_) => {
            eprintln!(
                "{}",
                json!({"experiment":"mapped-direct-youtube-v2-live",
                "phase":"mapped_direct_resolution","ok":false,
                "failureCode":"GLOBAL_DEADLINE","elapsedMs":second_ms})
            );
            panic!("mapped direct resolution exceeded the 60 second test deadline");
        }
    };
    let exact_direct_source = second.origin == ResolvedSourceOrigin::DirectYoutube
        && second.candidate.as_ref().is_some_and(|candidate| {
            candidate.id == format!("youtube:{}", anchors.video_id)
                && candidate.chapters.is_none()
                && candidate.source_metadata.is_none()
        });
    if !exact_direct_source {
        eprintln!(
            "{}",
            json!({"experiment":"mapped-direct-youtube-v2-live",
            "phase":"mapped_direct_resolution","ok":false,
            "origin":if second.origin == ResolvedSourceOrigin::DirectYoutube {"direct_youtube"} else {"existing"},
            "failureCode":"MAPPED_DIRECT_FELL_BACK","elapsedMs":second_ms})
        );
        panic!("stored v2 mapping did not produce the direct source");
    }

    let pcm = crate::native_output::direct_pcm_experiment::collect_until(
        second.source,
        0.0,
        5.0,
        started + Duration::from_secs(60),
        cancel,
    )
    .await;
    let after = mapping_timestamps(&database_path, &track.id, &fingerprint);
    let ttl_unchanged = after == before;
    let pcm_ok = pcm.ok && pcm.decoded_frames >= 240_000 && pcm.first_pcm_ms.is_some();
    let ok = ttl_unchanged && pcm_ok;
    eprintln!(
        "{}",
        json!({"experiment":"mapped-direct-youtube-v2-live",
        "phase":"mapped_direct_decode_and_ttl","ok":ok,"origin":"direct_youtube",
        "resolveMs":second_ms,"firstPcmFromStartMs":pcm.first_pcm_ms.map(|first|second_ms+first),
        "decodedFrames":pcm.decoded_frames,"mappingTtlUnchanged":ttl_unchanged,
        "failureCode":if ok {Value::Null} else {json!(if !ttl_unchanged {"MAPPING_TTL_REFRESHED"} else {"DECODE_FAILED"})},
        "wallMs":started.elapsed().as_secs_f64()*1000.0,
        "qualification":false,"physicalAudibilityMeasured":false,"proofVersion":2})
    );
    assert!(
        ttl_unchanged,
        "direct playback must not refresh mapping TTL"
    );
    assert!(
        pcm_ok,
        "mapped direct source must decode five seconds of audio"
    );
}
