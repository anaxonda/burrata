//! Small executable gate importing production modules unchanged. The only
//! adapter substitutes the host Tokio runtime for Tauri's global runtime.
//! This checks PCM capture and diagnostics, not a device or PlaybackService.
#![allow(dead_code)]
extern crate self as tauri;
pub mod async_runtime {
    pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future)
    }
}
#[path = "../../../src-tauri/src/output_progress.rs"]
mod output_progress;
#[path = "../../../src-tauri/src/pcm_tap.rs"]
mod pcm_tap;
#[path = "../../../src-tauri/src/playback_diagnostics_context.rs"]
mod playback_diagnostics_context;
#[path = "../../../src-tauri/src/playback_failure.rs"]
mod playback_failure;
#[path = "../../../src-tauri/src/playback_policy.rs"]
mod playback_policy;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let tap = pcm_tap::PcmTap::new();
    let mut writer = tap.writer(7, 8_000, 2);
    // Feed the real producer, pool, worker and retained PCM window. Let the
    // worker consume each bounded burst, as during ordinary device callbacks.
    for _ in 0..100 {
        for _ in 0..800 {
            writer.push(0.25);
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    drop(writer);
    let (_, _, samples) = tokio::task::spawn_blocking(move || tap.capture_pcm(7, 3.0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(samples.len(), 3 * 8_000 * 2);
    assert!(samples.iter().all(|sample| *sample == 0.25));

    let latest = output_progress::LatestProgress::default();
    assert!(latest.update(7, 0.0));
    for position in 1..10_000 {
        assert!(!latest.update(7, position as f64));
    }
    assert_eq!(latest.take(), Some((7, 9999.0)));
    assert_eq!(latest.take(), None);

    let mut watch = output_progress::OutputProgressWatch::default();
    let now = std::time::Instant::now();
    assert!(!watch.stalled(7, 10, true, now));
    assert!(watch.stalled(7, 10, true, now + playback_policy::OUTPUT_PROGRESS_TIMEOUT));
    assert!(!watch.stalled(7, 10, false, now + playback_policy::OUTPUT_PROGRESS_TIMEOUT));
    assert!(!watch.stalled(8, 10, true, now + playback_policy::OUTPUT_PROGRESS_TIMEOUT));

    let denied =
        playback_failure::classify("Native HTTP byte range: HTTP 403 on initial media response");
    assert_eq!(denied.http_status, Some(403));
    let timeout = playback_failure::classify("Audio source extraction timed out");
    let context = playback_diagnostics_context::project(
        &serde_json::json!({
            "engine":"native", "status":"failed", "diagnostics":{"lastRecovery":{
                "trigger":"source_resolution", "causes":[denied,timeout], "secret":"discard"
            }}
        }),
        "x86_64-unknown-linux-gnu",
    );
    assert_eq!(
        context["lastRecovery"]["causes"].as_array().unwrap().len(),
        2
    );
    assert_eq!(context["lastRecovery"]["causes"][0]["httpStatus"], 403);
    assert!(context["lastRecovery"].get("secret").is_none());
    println!("PCM capture, buffer recycling, output progress and failure-chain projection passed");
}
