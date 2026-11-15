//! Production PlaybackService + resolver + HTTP + native output, without a webview.
//! Test-only TLS registration; all network traffic stays in a no-egress namespace.
use super::*;
use crate::{playback::PlaybackService, playback_history::PlaybackHistory};
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;

async fn until(
    service: &PlaybackService,
    budget: u64,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(budget);
    loop {
        let state = service.snapshot();
        assert_eq!(state["volume"], 0.0, "fixture must stay muted");
        if predicate(&state) {
            return state;
        }
        assert!(
            !matches!(state["status"].as_str(), Some("failed" | "ended")),
            "Unexpected terminal network playback state: {state}"
        );
        assert!(
            Instant::now() < deadline,
            "network controller deadline: {state}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real controller network faults; private X display/audio and no-egress namespace required"]
async fn network_faults_use_controller_recovery_and_preserve_user_intent() {
    no_egress_namespace();
    assert_eq!(
        std::env::var("BURRATA_NETWORK_CONTROLLER_AUDIO").as_deref(),
        Ok("private-muted")
    );
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().identifier = "org.burrata.network-fault-test".into();
    let app = tauri::Builder::<tauri::Wry>::default()
        .any_thread()
        .build(context)
        .unwrap();
    let mut failures = Vec::new();
    for mode in [
        "403",
        "429",
        "503",
        "truncate",
        "stall",
        "bad-range",
        "cancel",
        "pause",
        "fallback",
        "no-length-truncate",
        "short-body",
    ] {
        let fixture = Fixture::new_with_duration("sndcdn.com", 24);
        fixture.mode(&format!("controller-{mode}"));
        for reference in [
            "https://soundcloud.com/synthetic-fixture/111",
            "https://soundcloud.com/synthetic-fixture/222",
        ] {
            playback_test_transport::register_reference(&fixture._registration, reference);
        }
        fixture
            .store
            .save_playback_checkpoint_and_engine(None, None, Some(&json!("native")))
            .unwrap();
        let history = Arc::new(
            PlaybackHistory::open(&fixture.directory.path().join("history.sqlite")).unwrap(),
        );
        let service = PlaybackService::new(
            app.handle().clone(),
            fixture.store.clone(),
            Arc::new(fixture.acquisition.clone()),
            history,
        )
        .unwrap();
        service
            .execute(json!({"action":"volume","volume":0.0}))
            .await
            .unwrap();
        let started = Instant::now();
        service
            .execute(json!({"action":"play","tracks":[fixture.track],"index":0}))
            .await
            .unwrap();
        let outcome = AssertUnwindSafe(async {
            if matches!(mode, "cancel" | "pause") {
                until(&service, 12, |s| s["status"] == "playing").await;
                until(&service, 10, |s| s["status"] == "buffering").await;
                let action = if mode == "cancel" { "stop" } else { "pause" };
                service.execute(json!({"action":action})).await.unwrap();
                let held = service.snapshot();
                std::fs::write(fixture.directory.path().join("release"), b"1").unwrap();
                tokio::time::sleep(Duration::from_secs(2)).await;
                let after = service.snapshot();
                assert_eq!(
                    after["status"],
                    if mode == "cancel" { "idle" } else { "paused" }
                );
                assert_eq!(
                    after["generation"], held["generation"],
                    "late response must not restart playback"
                );
                if mode == "pause" {
                    service.execute(json!({"action":"resume"})).await.unwrap();
                    let resumed = until(&service, 12, |s| s["status"] == "playing").await;
                    let position = resumed["position"].as_f64().unwrap();
                    until(&service, 4, |s| {
                        s["position"].as_f64().unwrap_or(0.) > position + 0.5
                    })
                    .await;
                }
            } else {
                if mode == "bad-range" {
                    until(&service, 12, |s| s["status"] == "playing").await;
                    service
                        .execute(json!({"action":"seek","position":18.0}))
                        .await
                        .unwrap();
                }
                let recovered = until(&service, 32, |s| {
                    let transport_can_repair = matches!(
                        mode,
                        "truncate" | "stall" | "no-length-truncate" | "short-body"
                    );
                    s["status"] == "playing"
                        && (s["diagnostics"]["recoveryCount"].as_u64().unwrap_or(0) > 0
                            || (transport_can_repair && s["position"].as_f64().unwrap_or(0.) > 6.0))
                })
                .await;
                assert_eq!(recovered["trackId"], fixture.track.id);
                assert_eq!(recovered["currentIndex"], 0);
                if mode == "fallback" {
                    assert_eq!(recovered["candidate"]["id"], "soundcloud:222");
                }
                assert!(recovered["diagnostics"]["lastSkippedTrackFailure"].is_null());
                let position = recovered["position"].as_f64().unwrap();
                until(&service, 4, |s| {
                    s["status"] == "playing"
                        && s["position"].as_f64().unwrap_or(0.) > position + 0.5
                })
                .await;
                if mode == "bad-range" {
                    assert!(
                        fixture
                            .requests()
                            .iter()
                            .any(|r| r["stage"] == "injected_range"),
                        "must exercise a real malformed Range response"
                    );
                }
            }
        })
        .catch_unwind()
        .await;
        let observed = service.snapshot();
        service.execute(json!({"action":"stop"})).await.unwrap();
        fixture.assert_saved();
        let responses = fixture
            .requests()
            .iter()
            .filter(|r| {
                matches!(
                    r["stage"].as_str(),
                    Some("response" | "injected_status" | "injected_range")
                )
            })
            .count();
        assert!(
            responses <= 16,
            "HTTP retries must stay bounded: {responses}"
        );
        eprintln!(
            "NETWORK_CONTROLLER_RESULT {}",
            json!({"mode":mode,"passed":outcome.is_ok(),"elapsedSeconds":started.elapsed().as_secs_f64(),"snapshot":observed,"requests":fixture.requests()})
        );
        if outcome.is_err() {
            failures.push(mode);
        }
    }
    assert!(
        failures.is_empty(),
        "Network controller failures: {failures:?}"
    );
}
