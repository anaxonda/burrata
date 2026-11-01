// Offline gates for ephemeral explicit-preview transport authority. No network.
fn explicit_preview_fixture() -> (
    tempfile::TempDir,
    Arc<Store>,
    AcquisitionManager,
    Track,
    Candidate,
    acquisition::PreviewAudio,
    SourceTrace,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("explicit-preview.sqlite")).unwrap());
    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
    let track = Track {
        id: "catalog:preview".into(),
        provider: "catalog".into(),
        provider_id: "edited-catalog".into(),
        title: "User title".into(),
        artist: "User artist".into(),
        asset_id: Some("owned-asset".into()),
        local_path: Some("/owned/retained.wav".into()),
        provenance: json!({"manualFields":{"title":true},"explicitSourceLock":true,"recordingId":"recording-keep"}),
        ..Track::default()
    };
    let candidate = Candidate {
        id: "youtube:rpCswezRrLs".into(),
        provider: "youtube".into(),
        url: "https://www.youtube.com/watch?v=rpCswezRrLs".into(),
        title: "Bacc of the Bach".into(),
        duration_seconds: Some(198.0),
        // inspect_candidate populates this options inventory automatically;
        // Preview does not pass the separate download format selection.
        formats: vec![
            crate::models::AudioFormat {
                id: "140".into(),
                ..Default::default()
            },
            crate::models::AudioFormat {
                id: "251".into(),
                ..Default::default()
            },
        ],
        ..Candidate::default()
    };
    let audio = acquisition::PreviewAudio {
        url: "https://r1---fixture.googlevideo.com/videoplayback".into(),
        mime_type: "audio/mp4".into(),
        codec: Some("aac".into()),
        bitrate_kbps: None,
        duration_seconds: Some(198.0),
        source_metadata: Some(json!({"provider":"youtube","channelId":"UCzyMypIMUep5W0J3R3PC87Q"})),
        resolved_title: Some("Bacc of the Bach".into()),
        resolved_provider_id: Some("rpCswezRrLs".into()),
        chapters: None,
        selected_format_id: Some("140".into()),
        selected_format_has_video: false,
        expected_content_length: Some(3_205_828),
        provider_session: Some(acquisition.playback_provider_stamp("youtube").unwrap()),
        headers: vec![],
    };
    let trace = SourceTrace::new(Some("native-preview".into()), None, None);
    assert!(trace.try_spend_extraction_for("youtube"));
    (
        directory,
        store,
        acquisition,
        track,
        candidate,
        audio,
        trace,
    )
}
fn explicit_preview_anchors(
    track: &Track,
    candidate: &Candidate,
    audio: &acquisition::PreviewAudio,
    trace: &SourceTrace,
) -> Option<VerifiedYoutubeRecordingAnchors> {
    fresh_explicit_preview_transport_anchors(
        track,
        candidate,
        audio,
        Some(VerifiedRecordingProof {
            kind: RecordingProofKind::ExplicitSource,
        }),
        true,
        true,
        2,
        trace,
        &Settings::default(),
        true,
        PlaybackDeadline::after(Duration::from_secs(20)),
    )
}
#[test]
fn explicit_preview_fresh_authority_preserves_assets_edits_and_is_not_persistable() {
    let (_dir, _store, _acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    let before = serde_json::to_value(&track).unwrap();
    let source = serde_json::to_value(&candidate).unwrap();
    let anchors = explicit_preview_anchors(&track, &candidate, &audio, &trace).unwrap();
    assert_eq!(anchors.video_id, "rpCswezRrLs");
    assert_eq!(anchors.kind, RecordingProofKind::ExplicitSource);
    assert_eq!(serde_json::to_value(&track).unwrap(), before);
    assert_eq!(serde_json::to_value(&candidate).unwrap(), source);
    assert!(VerifiedRecordingProof { kind: anchors.kind }
        .persisted(&candidate, &audio)
        .is_none());
    assert!(direct_youtube_explicit_id(&track, &candidate, 2).is_none());
    assert!(direct_youtube_mapped_anchors(&track, &candidate, None, 2).is_none());
}
#[test]
fn explicit_preview_authority_rejects_nonselected_wrong_proof_and_constraints() {
    let (_dir, _store, _acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    let proof = Some(VerifiedRecordingProof {
        kind: RecordingProofKind::ExplicitSource,
    });
    for (selected, pinned, limit) in [(false, true, 2), (true, false, 2), (true, true, 1)] {
        assert!(fresh_explicit_preview_transport_anchors(
            &track,
            &candidate,
            &audio,
            proof,
            selected,
            pinned,
            limit,
            &trace,
            &Settings::default(),
            true,
            PlaybackDeadline::after(Duration::from_secs(20))
        )
        .is_none());
    }
    assert!(fresh_explicit_preview_transport_anchors(
        &track,
        &candidate,
        &audio,
        Some(VerifiedRecordingProof {
            kind: RecordingProofKind::Recording
        }),
        true,
        true,
        2,
        &trace,
        &Settings::default(),
        true,
        PlaybackDeadline::after(Duration::from_secs(20))
    )
    .is_none());
    for changed in [
        Track {
            provider: "youtube".into(),
            ..track.clone()
        },
        Track {
            provenance: json!({"chapters":[{}]}),
            ..track.clone()
        },
        Track {
            playback_range: Some(crate::models::PlaybackRange {
                parent_track_id: "parent".into(),
                chapter_id: "chapter".into(),
                source_url: candidate.url.clone(),
                start_time: 2.0,
                end_time: 5.0,
                parent_title: "Parent".into(),
                parent_duration_seconds: Some(198.0),
            }),
            ..track.clone()
        },
    ] {
        assert!(explicit_preview_anchors(&changed, &candidate, &audio, &trace).is_none());
    }
    for changed in [
        Candidate {
            provider: "soundcloud".into(),
            ..candidate.clone()
        },
        Candidate {
            url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into(),
            ..candidate.clone()
        },
        Candidate {
            id: "youtube:aqz-KE-bpKQ".into(),
            ..candidate.clone()
        },
    ] {
        assert!(explicit_preview_anchors(&track, &changed, &audio, &trace).is_none());
    }
    for changed in [
        acquisition::PreviewAudio {
            resolved_provider_id: Some("aqz-KE-bpKQ".into()),
            ..audio.clone()
        },
        acquisition::PreviewAudio {
            resolved_title: Some("Other performance".into()),
            ..audio.clone()
        },
        acquisition::PreviewAudio {
            selected_format_id: Some("251".into()),
            ..audio.clone()
        },
        acquisition::PreviewAudio {
            provider_session: None,
            ..audio.clone()
        },
    ] {
        assert!(explicit_preview_anchors(&track, &candidate, &changed, &trace).is_none());
    }
}
#[test]
fn explicit_preview_requires_live_primary_session_remaining_deadline_and_second_slot() {
    let (_dir, _store, acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    let proof = Some(VerifiedRecordingProof {
        kind: RecordingProofKind::ExplicitSource,
    });
    assert!(fresh_explicit_preview_transport_anchors(
        &track,
        &candidate,
        &audio,
        proof,
        true,
        true,
        2,
        &trace,
        &Settings::default(),
        true,
        PlaybackDeadline::after(Duration::from_millis(999))
    )
    .is_none());
    assert!(trace.try_spend_extraction_for("youtube"));
    assert!(explicit_preview_anchors(&track, &candidate, &audio, &trace).is_none());
    assert!(!trace.try_spend_extraction_for("youtube"));
    acquisition.invalidate_playback_provider_session("youtube");
    assert!(!audio.provider_session_current());
    let fresh_trace = SourceTrace::new(Some("native-preview".into()), None, None);
    assert!(fresh_trace.try_spend_extraction_for("youtube"));
    assert!(explicit_preview_anchors(&track, &candidate, &audio, &fresh_trace).is_none());
}
#[tokio::test]
async fn explicit_preview_second_slot_compares_all_anchors_and_preflight_without_third_slot() {
    for mismatch in [
        "none", "id", "channel", "title", "duration", "format", "access",
    ] {
        let (_dir, _store, acquisition, track, candidate, audio, trace) =
            explicit_preview_fixture();
        let anchors = explicit_preview_anchors(&track, &candidate, &audio, &trace).unwrap();
        assert!(
            !candidate.formats.is_empty(),
            "Actual inspected chooser inventory must remain eligible"
        );
        let before = serde_json::to_value(&track).unwrap();
        let candidate_before = serde_json::to_value(&candidate).unwrap();
        let mut fresh = audio.clone();
        let mut channel = anchors.channel_id.clone();
        match mismatch {
            "id" => fresh.resolved_provider_id = Some("aqz-KE-bpKQ".into()),
            "channel" => channel = "UC1234567890123456789012".into(),
            "title" => fresh.resolved_title = Some("Other performance".into()),
            "duration" => fresh.duration_seconds = Some(202.0),
            "format" => fresh.selected_format_id = Some("251".into()),
            _ => {}
        }
        let (_stop, cancel) = watch::channel(false);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = try_direct_youtube_mapped_with_opportunity(
            &acquisition,
            &candidate,
            &anchors,
            cancel,
            &trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
            direct_youtube_recovery_deadline,
            false,
            move |id, budget, _| async move {
                assert_eq!(id, "rpCswezRrLs");
                assert!((1..=6000).contains(&budget));
                Ok(crate::youtube_direct_experiment::DirectResolved {
                    audio: fresh,
                    valid_until_unix_ms: chrono::Utc::now().timestamp_millis() as f64 + 60_000.0,
                    source_channel_id: Some(channel),
                })
            },
            |audio, _, _, _| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if mismatch == "access" {
                        Err("HTTP403".into())
                    } else {
                        Ok(audio)
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(result.is_some(), mismatch == "none");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(matches!(mismatch, "none" | "access"))
        );
        assert_eq!(trace.extraction_attempts_used(), 2);
        assert!(!trace.try_spend_extraction_for("youtube"));
        assert_eq!(
            trace
                .recovery_attempts
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert_eq!(serde_json::to_value(&track).unwrap(), before);
        assert_eq!(serde_json::to_value(&candidate).unwrap(), candidate_before);
    }
}
#[tokio::test]
async fn explicit_preview_second_slot_obeys_original_deadline_and_cancel() {
    for cancel_now in [false, true] {
        let (_dir, _store, acquisition, track, candidate, audio, trace) =
            explicit_preview_fixture();
        let anchors = explicit_preview_anchors(&track, &candidate, &audio, &trace).unwrap();
        let (stop, cancel) = watch::channel(cancel_now);
        let deadline = PlaybackDeadline::after(Duration::from_millis(1300));
        let started = std::time::Instant::now();
        let result = try_direct_youtube_mapped_with_opportunity(
            &acquisition,
            &candidate,
            &anchors,
            cancel,
            &trace,
            deadline,
            direct_youtube_recovery_deadline,
            false,
            |_, _, _| async {
                std::future::pending::<
                    Result<
                        crate::youtube_direct_experiment::DirectResolved,
                        crate::youtube_direct_experiment::DirectFailure,
                    >,
                >()
                .await
            },
            |audio, _, _, _| async { Ok(audio) },
        )
        .await;
        if cancel_now {
            assert_eq!(result.unwrap_err(), "Cancelled");
            assert_eq!(trace.extraction_attempts_used(), 1);
        } else {
            assert!(result.unwrap().is_none());
            assert_eq!(trace.extraction_attempts_used(), 2);
            assert!(deadline.remaining() > Duration::from_millis(500));
        }
        assert!(started.elapsed() < Duration::from_millis(900));
        drop(stop);
    }
}

#[test]
fn explicit_preview_fresh_authority_rejects_candidate_and_extractor_chapters() {
    let (_dir, _store, _acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    let chapter = crate::models::Chapter {
        id: "c".into(),
        title: "Part".into(),
        start_seconds: 0.0,
        end_seconds: 12.0,
        confidence: "known".into(),
        suggested_track: None,
        suggestion_confidence: None,
    };
    assert!(explicit_preview_anchors(
        &track,
        &Candidate {
            chapters: Some(vec![chapter.clone()]),
            ..candidate.clone()
        },
        &audio,
        &trace
    )
    .is_none());
    assert!(explicit_preview_anchors(
        &track,
        &candidate,
        &acquisition::PreviewAudio {
            chapters: Some(vec![chapter]),
            ..audio
        },
        &trace
    )
    .is_none());
}
#[tokio::test]
async fn explicit_preview_second_slot_cancellation_drops_running_transport() {
    let (_dir, _store, acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    let anchors = explicit_preview_anchors(&track, &candidate, &audio, &trace).unwrap();
    let (stop, cancel) = watch::channel(false);
    let entered = tokio::sync::Notify::new();
    let dropped = std::sync::atomic::AtomicBool::new(false);
    struct Dropped<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for Dropped<'_> {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let operation = try_direct_youtube_mapped_with_opportunity(
        &acquisition,
        &candidate,
        &anchors,
        cancel,
        &trace,
        PlaybackDeadline::after(Duration::from_secs(20)),
        direct_youtube_recovery_deadline,
        false,
        |_, _, _| async {
            let _drop = Dropped(&dropped);
            entered.notify_one();
            std::future::pending::<
                Result<
                    crate::youtube_direct_experiment::DirectResolved,
                    crate::youtube_direct_experiment::DirectFailure,
                >,
            >()
            .await
        },
        |audio, _, _, _| async { Ok(audio) },
    );
    let cancellation = async {
        entered.notified().await;
        stop.send_replace(true);
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(operation, cancellation)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err(), "Cancelled");
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(trace.extraction_attempts_used(), 2);
    assert!(!trace.try_spend_extraction_for("youtube"));
}

#[test]
fn explicit_preview_inspected_format_inventory_is_not_a_preview_format_lock() {
    let (_dir, _store, _acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    assert_eq!(
        candidate
            .formats
            .iter()
            .map(|format| format.id.as_str())
            .collect::<Vec<_>>(),
        vec!["140", "251"]
    );
    let before = serde_json::to_value(&candidate).unwrap();
    assert!(explicit_preview_anchors(&track, &candidate, &audio, &trace).is_some());
    assert_eq!(serde_json::to_value(&candidate).unwrap(), before);
    assert!(explicit_preview_anchors(
        &track,
        &candidate,
        &acquisition::PreviewAudio {
            selected_format_id: Some("251".into()),
            ..audio
        },
        &trace
    )
    .is_none());
}

#[test]
fn explicit_preview_disabled_production_policy_rejects_without_global_env_mutation() {
    let (_dir, _store, _acquisition, track, candidate, audio, trace) = explicit_preview_fixture();
    assert!(!direct_youtube_policy(Some("0")));
    assert!(fresh_explicit_preview_transport_anchors(
        &track,
        &candidate,
        &audio,
        Some(VerifiedRecordingProof {
            kind: RecordingProofKind::ExplicitSource
        }),
        true,
        true,
        2,
        &trace,
        &Settings::default(),
        direct_youtube_policy(Some("0")),
        PlaybackDeadline::after(Duration::from_secs(20))
    )
    .is_none());
    // Positive fake-resolver tests inject true; release CI may keep its global
    // BURRATA_DIRECT_YOUTUBE=0 guard without enabling any actual network.
    assert!(explicit_preview_anchors(&track, &candidate, &audio, &trace).is_some());
}
