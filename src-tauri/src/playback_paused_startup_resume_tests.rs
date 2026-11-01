mod paused_startup_resume {
    use super::*;

    fn interrupted_startup() -> State {
        let mut state = State::default();
        state.queue = vec![Track {
            id: "saved-recording".into(),
            title: "Bacc Of The Bach".into(),
            local_path: Some("registered-download.flac".into()),
            asset_id: Some("saved-asset".into()),
            local_availability: "available".into(),
            ..Track::default()
        }];
        state.entries = vec!["same-preview-occurrence".into()];
        state.preview_source_entry = Some(state.entries[0].clone());
        state.candidate = Some(Candidate {
            provider: "youtube".into(),
            id: "youtube:rpCswezRrLs".into(),
            ..Candidate::default()
        });
        state.explicit_source = true;
        state.session = "existing-preview-session".into();
        state.generation_counter = 5;
        state.generation = 5;
        state.status = "paused".into();
        state.output_ready = false;
        state.first_audio_ms = None;
        state.startup_budget = Duration::from_secs(20);
        state.request_started = Instant::now() - Duration::from_secs(26);
        state
    }

    #[test]
    fn paused_startup_resume_renews_expired_budget_without_replacing_preview_owner_or_download() {
        for budget in [Duration::from_secs(20), Duration::from_secs(45)] {
            let mut state = interrupted_startup();
            state.startup_budget = budget;
            state.request_started = Instant::now() - budget - Duration::from_secs(6);
            let original = serde_json::to_value(&state.queue[0]).unwrap();
            let candidate = serde_json::to_value(&state.candidate).unwrap();
            let old_deadline = restart_deadline(&state);
            let resumed_at = Instant::now();
            assert!(old_deadline < resumed_at);
            assert_eq!(state.resume_target(), (0., false, true));
            arm_paused_startup_resume(&mut state, resumed_at);
            assert_eq!(restart_deadline(&state), resumed_at + budget);
            assert_eq!(state.request_started, resumed_at);
            assert_eq!(serde_json::to_value(&state.queue[0]).unwrap(), original);
            assert_eq!(serde_json::to_value(&state.candidate).unwrap(), candidate);
            assert_eq!(state.preview_source_entry.as_ref(), state.entries.first());
            assert_eq!(state.entries, vec!["same-preview-occurrence"]);
            assert_eq!(state.session, "existing-preview-session");
            assert_eq!(state.generation, 5);
            assert!(state.explicit_source);
            assert!(!state.output_ready);
            assert_eq!(state.status, "paused");
            assert!(state.first_audio_ms.is_none());
            // start() advances the cancelled pause generation. The old startup
            // watchdog must not expire the fresh resumed owner.
            state.generation_counter += 1;
            state.generation = state.generation_counter;
            state.status = "resolving".into();
            assert!(!deadline_owned_by_active_operation(&state, 4, None));
            assert!(deadline_owned_by_active_operation(&state, 6, None));
            assert_eq!(restart_deadline(&state), resumed_at + budget);
        }
    }

    #[test]
    fn paused_startup_resume_keeps_background_recovery_bounded_and_exclusions_intact() {
        let mut state = interrupted_startup();
        state.recovery = 1;
        state.recurring_source_swaps = 1;
        state.failed_recurring_candidates = vec![("youtube".into(), "youtube:failed".into())];
        state.recovery_deadline = Some(state.request_started + state.startup_budget);
        let resumed_at = Instant::now();
        arm_paused_startup_resume(&mut state, resumed_at);
        let resumed_deadline = restart_deadline(&state);
        assert_eq!(resumed_deadline, resumed_at + state.startup_budget);
        assert!(can_retry_failed_stream_at_stage(
            &state,
            "Native HTTP request failed: connection reset",
            true
        ));
        assert_eq!(state.recovery, 1);
        assert_eq!(state.recurring_source_swaps, 1);
        assert_eq!(
            state.failed_recurring_candidates,
            vec![("youtube".into(), "youtube:failed".into())]
        );
        for status in ["resolving", "buffering"] {
            state.status = status.into();
            state.recovery_deadline = Some(resumed_deadline);
            // Background work cannot renew the window, even if it reached this
            // guard. Only paused explicit user Resume is eligible.
            arm_paused_startup_resume(&mut state, resumed_at + Duration::from_secs(60));
            assert_eq!(restart_deadline(&state), resumed_deadline);
            assert_eq!(state.request_started, resumed_at);
        }
        state.recovery = 2;
        assert!(!can_retry_failed_stream_at_stage(
            &state,
            "Native HTTP request failed: connection reset",
            true
        ));
        state.recovery = 1;
        state.request_started = Instant::now() - state.startup_budget - Duration::from_secs(1);
        let expired_request = state.request_started;
        arm_paused_startup_resume(&mut state, Instant::now());
        assert_eq!(state.request_started, expired_request);
        assert!(!can_retry_failed_stream_at_stage(
            &state,
            "Native HTTP request failed: connection reset",
            true
        ));
    }

    #[test]
    fn paused_startup_resume_does_not_rearm_ready_or_already_audible_playback() {
        for (ready, first_audio) in [(true, None), (false, Some(10)), (true, Some(10))] {
            let mut state = interrupted_startup();
            state.output_ready = ready;
            state.first_audio_ms = first_audio;
            let request_started = state.request_started;
            let recovery_deadline = request_started + state.startup_budget;
            state.recovery_deadline = Some(recovery_deadline);
            arm_paused_startup_resume(&mut state, Instant::now());
            assert_eq!(state.request_started, request_started);
            assert_eq!(state.recovery_deadline, Some(recovery_deadline));
        }
    }

    #[test]
    fn paused_startup_resume_preserves_deferred_seek_epoch_and_latency_owner() {
        let mut state = interrupted_startup();
        state.pending_seek_generation = Some(state.generation);
        state.seek_serial = 3;
        state.seek_operation_id = Some("owned-seek".into());
        let received = Instant::now() - Duration::from_secs(10);
        state.seek_operation_received_at = Some(received);
        let resumed_at = Instant::now();
        arm_paused_startup_resume(&mut state, resumed_at);
        arm_resumed_seek_metrics(&mut state, resumed_at);
        assert_eq!(state.seek_requested, Some(resumed_at));
        assert_eq!(restart_deadline(&state), resumed_at + state.startup_budget);
        assert_eq!(state.pending_seek_generation, Some(5));
        assert_eq!(state.seek_serial, 3);
        assert_eq!(state.seek_operation_id.as_deref(), Some("owned-seek"));
        assert_eq!(state.seek_operation_received_at, Some(received));
    }
}
