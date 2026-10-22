//! Offline wiring regressions for the experimental explicit-source opportunity.
//! Only the HTTP resolver and access worker are replaced; request budgets,
//! recording identity, cancellation and manager-owned epoch fences are real.
use super::*;
use crate::youtube_direct_experiment::{DirectFailure, DirectResolved};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn fixture() -> (
    tempfile::TempDir,
    AcquisitionManager,
    Track,
    Candidate,
    SourceTrace,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("direct-wiring.sqlite")).unwrap());
    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let manager = AcquisitionManager::new_for_playback_test(store, connections);
    let track = Track {
        id: "youtube:uZGRATckoBM".into(),
        provider: "youtube".into(),
        provider_id: "uZGRATckoBM".into(),
        source_url: "https://www.youtube.com/watch?v=uZGRATckoBM".into(),
        title: "Matthew".into(),
        duration_seconds: Some(228.),
        ..Track::default()
    };
    let candidate = Candidate {
        id: track.id.clone(),
        provider: "youtube".into(),
        url: track.source_url.clone(),
        title: track.title.clone(),
        duration_seconds: track.duration_seconds,
        ..Candidate::default()
    };
    let trace = SourceTrace::new(Some("native-direct-offline".into()), None, None);
    (directory, manager, track, candidate, trace)
}

fn response() -> DirectResolved {
    DirectResolved {
        source_channel_id: None,
        audio: acquisition::PreviewAudio {
            provider_session: None,
            url: "https://r1---fixture.googlevideo.com/videoplayback?sig=private-fixture".into(),
            mime_type: "audio/webm".into(),
            codec: Some("opus".into()),
            bitrate_kbps: None,
            duration_seconds: Some(228.),
            selected_format_id: Some("251".into()),
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: Some("Matthew".into()),
            resolved_provider_id: Some("uZGRATckoBM".into()),
            chapters: None,
            expected_content_length: Some(4_000_000),
            headers: vec![],
        },
        valid_until_unix_ms: chrono::Utc::now().timestamp_millis() as f64 + 120_000.,
    }
}

fn network_failure() -> DirectFailure {
    DirectFailure {
        code: "NETWORK_FAILED",
        stage: "player",
        http_status: None,
    }
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn direct_wiring_success_binds_epoch_and_preserves_reduced_validation_deadline() {
    let (_directory, manager, track, candidate, trace) = fixture();
    let (_stop, cancel) = watch::channel(false);
    let deadline = PlaybackDeadline::after(Duration::from_secs(10));
    let calls = AtomicUsize::new(0);
    let audio = try_direct_youtube_explicit_with(
        &manager,
        &track,
        &candidate,
        cancel,
        &trace,
        deadline,
        |id, budget, cancel| async move {
            assert_eq!(id, "uZGRATckoBM");
            assert!((1..=2_000).contains(&budget));
            assert!(!*cancel.borrow());
            Ok(response())
        },
        |audio, cancel, validation_trace, validation_deadline| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(
                    validation_deadline.instant(),
                    deadline.instant() - Duration::from_secs(8)
                );
                assert_eq!(validation_trace.extraction_attempts_used(), 1);
                assert!(audio.provider_session.is_some());
                assert!(audio.provider_session_current());
                assert!(!*cancel.borrow());
                Ok(audio)
            }
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(trace.extraction_attempts_used(), 1);
    assert_eq!(audio.resolved_provider_id.as_deref(), Some("uZGRATckoBM"));
    assert_eq!(audio.resolved_title.as_deref(), Some("Matthew"));
    assert_eq!(audio.selected_format_id.as_deref(), Some("251"));
    assert!(audio.chapters.is_none() && audio.source_metadata.is_none());
    manager.invalidate_playback_provider_session("youtube");
    assert!(
        !audio.provider_session_current(),
        "the returned stamp belongs to the real manager"
    );
}

#[tokio::test]
async fn seek_recovery_aac_is_exact_video_bounded_and_validated_before_use() {
    let (_directory, manager, track, candidate, trace) = fixture();
    let (_stop, cancel) = watch::channel(false);
    let deadline = PlaybackDeadline::after(Duration::from_secs(20));
    let validations = AtomicUsize::new(0);
    let audio = try_direct_youtube_explicit_with_opportunity(
        &manager,
        &track,
        &candidate,
        cancel,
        &trace,
        deadline,
        direct_youtube_recovery_deadline,
        |id, budget, _| async move {
            assert_eq!(id, "uZGRATckoBM");
            assert!((1..=6_000).contains(&budget));
            let mut resolved = response();
            resolved.audio.mime_type = "audio/mp4".into();
            resolved.audio.codec = Some("mp4a.40.2".into());
            resolved.audio.selected_format_id = Some("140".into());
            Ok(resolved)
        },
        |audio, _, _, validation_deadline| {
            validations.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(
                    validation_deadline.instant() < deadline.instant() - Duration::from_secs(13)
                );
                assert!(validation_deadline.remaining() <= Duration::from_secs(6));
                Ok(audio)
            }
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(validations.load(Ordering::SeqCst), 1);
    assert_eq!(audio.selected_format_id.as_deref(), Some("140"));
    assert!(audio.provider_session_current());
}

#[tokio::test]
async fn seek_recovery_aac_rejects_wrong_video_or_duration_before_validation() {
    for mismatch in ["id", "duration"] {
        let (_directory, manager, track, candidate, trace) = fixture();
        let (_stop, cancel) = watch::channel(false);
        let mut resolved = response();
        if mismatch == "id" {
            resolved.audio.resolved_provider_id = Some("abcdefghijk".into());
        } else {
            resolved.audio.duration_seconds = Some(240.);
        }
        let validations = AtomicUsize::new(0);
        let result = try_direct_youtube_explicit_with_opportunity(
            &manager,
            &track,
            &candidate,
            cancel,
            &trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
            direct_youtube_recovery_deadline,
            move |_, _, _| async move { Ok(resolved) },
            |audio, _, _, _| {
                validations.fetch_add(1, Ordering::SeqCst);
                async { Ok(audio) }
            },
        )
        .await
        .unwrap();
        assert!(result.is_none(), "{mismatch}");
        assert_eq!(validations.load(Ordering::SeqCst), 0, "{mismatch}");
    }
}

#[tokio::test]
async fn seek_recovery_aac_keeps_provider_session_change_terminal() {
    let (_directory, manager, track, candidate, trace) = fixture();
    let (_stop, cancel) = watch::channel(false);
    let reset = manager.clone();
    let result = try_direct_youtube_explicit_with_opportunity(
        &manager,
        &track,
        &candidate,
        cancel,
        &trace,
        PlaybackDeadline::after(Duration::from_secs(20)),
        direct_youtube_recovery_deadline,
        |_, _, _| async { Ok(response()) },
        move |audio, _, _, _| async move {
            reset.invalidate_playback_provider_session("youtube");
            Ok(audio)
        },
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        "Audio source provider session changed during resolution"
    );
}

#[tokio::test]
async fn direct_wiring_failure_leaves_exactly_one_shared_fallback_attempt() {
    let (_directory, manager, track, candidate, trace) = fixture();
    let (_stop, cancel) = watch::channel(false);
    let calls = AtomicUsize::new(0);
    let result = try_direct_youtube_explicit_with(
        &manager,
        &track,
        &candidate,
        cancel,
        &trace,
        PlaybackDeadline::after(Duration::from_secs(20)),
        |_, _, _| async { Err(network_failure()) },
        |audio, _, _, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(audio) }
        },
    )
    .await
    .unwrap();
    assert!(
        result.is_none(),
        "a source failure permits the ordinary fallback"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(trace.extraction_attempts_used(), 1);
    assert!(trace
        .clone()
        .with_event_provider("youtube")
        .try_spend_extraction());
    assert!(
        !trace.try_spend_extraction(),
        "direct plus fallback exhaust the request budget"
    );
}

#[tokio::test]
async fn direct_wiring_identity_rejection_never_enters_access_validation() {
    let (_directory, manager, track, candidate, trace) = fixture();
    for mismatch in ["id", "title", "duration", "missing_title"] {
        let (_stop, cancel) = watch::channel(false);
        let mut resolved = response();
        match mismatch {
            "id" => resolved.audio.resolved_provider_id = Some("abcdefghijk".into()),
            "title" => resolved.audio.resolved_title = Some("Another recording".into()),
            "duration" => resolved.audio.duration_seconds = Some(240.),
            _ => resolved.audio.resolved_title = None,
        }
        let calls = AtomicUsize::new(0);
        let attempt_trace = SourceTrace::new(trace.native_operation_id.clone(), None, None);
        let result = try_direct_youtube_explicit_with(
            &manager,
            &track,
            &candidate,
            cancel,
            &attempt_trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
            |_, _, _| async move { Ok(resolved) },
            |audio, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(audio) }
            },
        )
        .await
        .unwrap();
        assert!(result.is_none(), "{mismatch}");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "{mismatch}");
        assert_eq!(attempt_trace.extraction_attempts_used(), 1);
    }
}

#[tokio::test]
async fn direct_wiring_cancellation_drops_held_worker_or_validator_without_fallback() {
    for hold_resolver in [true, false] {
        let (_directory, manager, track, candidate, trace) = fixture();
        let (stop, cancel) = watch::channel(false);
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = Arc::new(std::sync::Mutex::new(Some(entered_tx)));
        let dropped = Arc::new(AtomicBool::new(false));
        let resolve_entered = entered.clone();
        let resolve_dropped = dropped.clone();
        let validate_dropped = dropped.clone();
        let work = try_direct_youtube_explicit_with(
            &manager,
            &track,
            &candidate,
            cancel,
            &trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
            move |_, _, _| async move {
                if hold_resolver {
                    let _guard = Dropped(resolve_dropped);
                    resolve_entered
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(())
                        .unwrap();
                    std::future::pending().await
                } else {
                    Ok(response())
                }
            },
            move |_, _, _, _| async move {
                assert!(
                    !hold_resolver,
                    "a held resolver cannot reach access validation"
                );
                let _guard = Dropped(validate_dropped);
                entered.lock().unwrap().take().unwrap().send(()).unwrap();
                std::future::pending().await
            },
        );
        let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(work, async {
                entered_rx.await.unwrap();
                stop.send_replace(true);
            })
        })
        .await
        .expect("cancellation did not interrupt the held opportunity");
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(trace.extraction_attempts_used(), 1);
    }
}

#[tokio::test]
async fn direct_wiring_epoch_reset_during_worker_is_terminal_even_if_worker_fails() {
    for worker_succeeds in [true, false] {
        let (_directory, manager, track, candidate, trace) = fixture();
        let (_stop, cancel) = watch::channel(false);
        let reset = manager.clone();
        let calls = AtomicUsize::new(0);
        let result = try_direct_youtube_explicit_with(
            &manager,
            &track,
            &candidate,
            cancel,
            &trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
            move |_, _, _| async move {
                reset.invalidate_playback_provider_session("youtube");
                if worker_succeeds {
                    Ok(response())
                } else {
                    Err(network_failure())
                }
            },
            |audio, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(audio) }
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Audio source provider session changed during resolution"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(trace.extraction_attempts_used(), 1);
    }
}

#[tokio::test]
async fn direct_wiring_epoch_reset_during_validator_is_terminal_even_if_validator_fails() {
    for validation_succeeds in [true, false] {
        let (_directory, manager, track, candidate, trace) = fixture();
        let (_stop, cancel) = watch::channel(false);
        let reset = manager.clone();
        let result = try_direct_youtube_explicit_with(
            &manager,
            &track,
            &candidate,
            cancel,
            &trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
            |_, _, _| async { Ok(response()) },
            move |audio, _, _, _| async move {
                assert!(audio.provider_session_current());
                reset.invalidate_playback_provider_session("youtube");
                if validation_succeeds {
                    Ok(audio)
                } else {
                    Err("Access check failed".into())
                }
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Audio source provider session changed during resolution"
        );
        assert_eq!(trace.extraction_attempts_used(), 1);
    }
}

#[tokio::test]
async fn direct_wiring_resolver_and_validation_timeouts_preserve_fallback_reserve() {
    for hold_resolver in [true, false] {
        let (_directory, manager, track, candidate, trace) = fixture();
        let (_stop, cancel) = watch::channel(false);
        let dropped = Arc::new(AtomicBool::new(false));
        let resolve_dropped = dropped.clone();
        let validate_dropped = dropped.clone();
        let deadline = PlaybackDeadline::after(Duration::from_millis(8_500));
        let work = try_direct_youtube_explicit_with(
            &manager,
            &track,
            &candidate,
            cancel,
            &trace,
            deadline,
            move |_, _, _| async move {
                if hold_resolver {
                    let _guard = Dropped(resolve_dropped);
                    std::future::pending().await
                } else {
                    Ok(response())
                }
            },
            move |_, _, _, validation_deadline| async move {
                assert!(!hold_resolver);
                assert_eq!(
                    validation_deadline.instant(),
                    deadline.instant() - Duration::from_secs(8)
                );
                let _guard = Dropped(validate_dropped);
                std::future::pending().await
            },
        );
        let result = tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .expect("opportunity extended beyond its reduced deadline")
            .unwrap();
        assert!(result.is_none());
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(trace.extraction_attempts_used(), 1);
        assert!(deadline.remaining() > Duration::from_secs(7));
    }
}

#[tokio::test]
#[ignore = "explicit public-network opt-in; production resolver plus finite decoder PCM, no device/catalog proof"]
async fn live_direct_youtube_foreground_matthew_production_resolver_pcm() {
    run_live_direct_foreground(Track {
        id: "youtube:uZGRATckoBM".into(),
        provider: "youtube".into(),
        provider_id: "uZGRATckoBM".into(),
        source_url: "https://www.youtube.com/watch?v=uZGRATckoBM".into(),
        title: "Matthew".into(),
        duration_seconds: Some(228.),
        ..Track::default()
    })
    .await;
}

#[tokio::test]
#[ignore = "explicit public-network and source-fixture opt-in; no device/catalog proof"]
async fn live_direct_youtube_foreground_source_fixture_pcm() {
    let fixture: serde_json::Value = serde_json::from_str(
        &std::env::var("BURRATA_DIRECT_YOUTUBE_SOURCE_FIXTURE")
            .expect("requires an explicit public source fixture"),
    )
    .expect("source fixture must be JSON");
    let id = fixture["videoId"].as_str().expect("videoId required");
    assert!(valid_youtube_video_id(id));
    let title = fixture["title"].as_str().expect("source title required");
    assert!(valid_anchor_title(title));
    let duration = fixture["durationSeconds"]
        .as_f64()
        .expect("source duration required");
    assert!(valid_anchor_duration(duration));
    run_live_direct_foreground(Track {
        id: format!("youtube:{id}"),
        provider: "youtube".into(),
        provider_id: id.into(),
        source_url: format!("https://www.youtube.com/watch?v={id}"),
        title: title.into(),
        duration_seconds: Some(duration),
        ..Track::default()
    })
    .await;
}

async fn run_live_direct_foreground(track: Track) {
    assert_eq!(
        std::env::var("BURRATA_DIRECT_YOUTUBE_LIVE").as_deref(),
        Ok("1"),
        "requires explicit live diagnostic opt-in"
    );
    assert_eq!(
        std::env::var("BURRATA_DIRECT_YOUTUBE_EXPERIMENT").as_deref(),
        Ok("1"),
        "requires explicit direct resolver opt-in"
    );
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("live-direct.sqlite")).unwrap());
    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let manager = AcquisitionManager::new_for_playback_test(store.clone(), connections);
    let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources/tools")
        .join(crate::tools::target_triple())
        .join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
    assert!(ffmpeg.is_file(), "bundled decoder asset is required");
    let mut settings = store.settings().unwrap();
    settings.ffmpeg_path = ffmpeg.to_string_lossy().into_owned();
    store.save_settings(&settings).unwrap();
    let (_stop, cancel) = watch::channel(false);
    let started = Instant::now();
    let original_deadline = started + Duration::from_secs(20);
    let result = resolve_with_trace_context_excluding_until_attempt_limit(
        &store,
        &manager,
        &track,
        None,
        cancel.clone(),
        Some("native-direct-live-matthew".into()),
        None,
        None,
        &[],
        PlaybackDeadline::at(tokio::time::Instant::from_std(original_deadline)),
        2,
    )
    .await;
    let resolve_ms = started.elapsed().as_secs_f64() * 1000.;
    let resolved = match result {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!(
                "{}",
                json!({"experiment":"production-direct-youtube-foreground-v1",
                    "videoId":track.provider_id,"ok":false,"origin":null,
                    "failure":"RESOLUTION_FAILED","policy":crate::playback_failure::classify(&error),
                    "resolveMs":resolve_ms,"firstPcmFromStartMs":null,"decodedFrames":0,
                    "qualification":false,"physicalAudibilityMeasured":false,"catalogProof":false})
            );
            panic!("production direct resolution failed; safe failure evidence retained above");
        }
    };
    let direct = resolved.origin == ResolvedSourceOrigin::DirectYoutube;
    let exact_unknown_chapters = resolved
        .candidate
        .as_ref()
        .is_some_and(|candidate| candidate.id == track.id && candidate.chapters.is_none());
    if !direct || !exact_unknown_chapters {
        eprintln!(
            "{}",
            json!({"experiment":"production-direct-youtube-foreground-v1",
                "videoId":track.provider_id,"ok":false,
                "origin":if direct {"direct_youtube"} else {"existing"},
                "failure":if direct {"DIRECT_IDENTITY_OR_CHAPTER_STATE_CHANGED"} else {"DIRECT_PATH_FELL_BACK"},
                "resolveMs":resolve_ms,"firstPcmFromStartMs":null,"decodedFrames":0,
                "qualification":false,"physicalAudibilityMeasured":false,"catalogProof":false})
        );
        panic!("direct path did not qualify; original fallback/state evidence retained above");
    }
    let before_decode_ms = started.elapsed().as_secs_f64() * 1000.;
    let pcm = crate::native_output::direct_pcm_experiment::collect_until(
        resolved.source,
        0.,
        5.,
        original_deadline,
        cancel,
    )
    .await;
    let ok = pcm.ok && pcm.decoded_frames >= 240_000 && pcm.first_pcm_ms.is_some();
    eprintln!(
        "{}",
        json!({"experiment":"production-direct-youtube-foreground-v1",
            "videoId":track.provider_id,"ok":ok,"origin":"direct_youtube",
            "resolveMs":resolve_ms,
            "firstPcmFromStartMs":pcm.first_pcm_ms.map(|first|before_decode_ms+first),
            "decodedFrames":pcm.decoded_frames,"failure":pcm.failure,
            "wallMs":started.elapsed().as_secs_f64()*1000.,
            "qualification":false,"physicalAudibilityMeasured":false,"catalogProof":false})
    );
    assert!(
        ok,
        "finite PCM failed; safe original outcome retained above"
    );
}
