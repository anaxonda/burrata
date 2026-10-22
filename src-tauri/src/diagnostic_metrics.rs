//! Opt-in counters for isolated playback diagnostics.
//!
//! These process-wide windows count instrumented Rust request-send attempts and
//! successful yt-dlp child starts. They do not represent physical HTTP requests:
//! reqwest automatic redirect hops and yt-dlp's internal network traffic are
//! opaque. Snapshots are cumulative; consumers compute per-window deltas.

use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    OnceLock,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub http_send_attempts: u64,
    pub yt_dlp_successful_spawns: u64,
    pub match_prepare_elapsed_ns: u64,
    pub match_assess_elapsed_ns: u64,
    /// Thread CPU elapsed is absent when this window had no successful clock
    /// observations (unsupported platform or clock failure).
    pub match_prepare_cpu_elapsed_ns: Option<u64>,
    pub match_prepare_cpu_observations: u64,
    pub match_assess_cpu_elapsed_ns: Option<u64>,
    pub match_assess_cpu_observations: u64,
}

impl Snapshot {
    /// Return nonnegative deltas for cumulative snapshots, including if a
    /// counter ever wraps during an unusually long diagnostic process.
    pub fn delta_since(self, earlier: Self) -> Self {
        Self {
            http_send_attempts: self
                .http_send_attempts
                .saturating_sub(earlier.http_send_attempts),
            yt_dlp_successful_spawns: self
                .yt_dlp_successful_spawns
                .saturating_sub(earlier.yt_dlp_successful_spawns),
            match_prepare_elapsed_ns: self
                .match_prepare_elapsed_ns
                .saturating_sub(earlier.match_prepare_elapsed_ns),
            match_assess_elapsed_ns: self
                .match_assess_elapsed_ns
                .saturating_sub(earlier.match_assess_elapsed_ns),
            match_prepare_cpu_elapsed_ns: cpu_delta(
                self.match_prepare_cpu_elapsed_ns,
                earlier.match_prepare_cpu_elapsed_ns,
                self.match_prepare_cpu_observations,
                earlier.match_prepare_cpu_observations,
            ),
            match_prepare_cpu_observations: self
                .match_prepare_cpu_observations
                .saturating_sub(earlier.match_prepare_cpu_observations),
            match_assess_cpu_elapsed_ns: cpu_delta(
                self.match_assess_cpu_elapsed_ns,
                earlier.match_assess_cpu_elapsed_ns,
                self.match_assess_cpu_observations,
                earlier.match_assess_cpu_observations,
            ),
            match_assess_cpu_observations: self
                .match_assess_cpu_observations
                .saturating_sub(earlier.match_assess_cpu_observations),
        }
    }
}

fn cpu_delta(
    current: Option<u64>,
    earlier: Option<u64>,
    current_observations: u64,
    earlier_observations: u64,
) -> Option<u64> {
    (current_observations > earlier_observations)
        .then(|| current.unwrap_or(0).saturating_sub(earlier.unwrap_or(0)))
}

#[derive(Default)]
struct Meter {
    http_send_attempts: AtomicU64,
    yt_dlp_successful_spawns: AtomicU64,
    match_prepare_elapsed_ns: AtomicU64,
    match_assess_elapsed_ns: AtomicU64,
    match_prepare_cpu_elapsed_ns: AtomicU64,
    match_prepare_cpu_observations: AtomicU64,
    match_assess_cpu_elapsed_ns: AtomicU64,
    match_assess_cpu_observations: AtomicU64,
}

impl Meter {
    fn add(&self, counter: &AtomicU64, value: u64) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(value))
        });
    }

    fn snapshot(&self) -> Snapshot {
        let match_prepare_cpu_observations =
            self.match_prepare_cpu_observations.load(Ordering::Acquire);
        let match_assess_cpu_observations =
            self.match_assess_cpu_observations.load(Ordering::Acquire);
        Snapshot {
            http_send_attempts: self.http_send_attempts.load(Ordering::Relaxed),
            yt_dlp_successful_spawns: self.yt_dlp_successful_spawns.load(Ordering::Relaxed),
            match_prepare_elapsed_ns: self.match_prepare_elapsed_ns.load(Ordering::Relaxed),
            match_assess_elapsed_ns: self.match_assess_elapsed_ns.load(Ordering::Relaxed),
            match_prepare_cpu_elapsed_ns: (match_prepare_cpu_observations > 0)
                .then(|| self.match_prepare_cpu_elapsed_ns.load(Ordering::Relaxed)),
            match_prepare_cpu_observations,
            match_assess_cpu_elapsed_ns: (match_assess_cpu_observations > 0)
                .then(|| self.match_assess_cpu_elapsed_ns.load(Ordering::Relaxed)),
            match_assess_cpu_observations,
        }
    }
}

static GLOBAL: OnceLock<Meter> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

fn global() -> &'static Meter {
    GLOBAL.get_or_init(Meter::default)
}

/// Enable process-window metrics for an explicitly requested diagnostic run.
/// This is deliberately not environment-variable or normal-app enabled.
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
    let _ = global();
}

/// Cheap gate for avoiding timing setup on ordinary matching calls.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Return cumulative counters. Take snapshots immediately before and after a
/// serial diagnostic window and subtract them with [`Snapshot::delta_since`].
pub fn snapshot() -> Snapshot {
    global().snapshot()
}

pub fn record_http_send_attempt() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let meter = global();
    meter.add(&meter.http_send_attempts, 1);
}

pub fn record_yt_dlp_successful_spawn() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let meter = global();
    meter.add(&meter.yt_dlp_successful_spawns, 1);
}

pub fn record_match_prepare_elapsed(elapsed: Duration) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let meter = global();
    let ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    meter.add(&meter.match_prepare_elapsed_ns, ns);
}

pub fn record_match_assess_elapsed(elapsed: Duration) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let meter = global();
    let ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    meter.add(&meter.match_assess_elapsed_ns, ns);
}

/// Read CPU time consumed by the calling thread. Waiting and sleep time are
/// excluded. Unsupported platforms and clock failures remain unavailable.
pub fn thread_cpu_time_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // CLOCK_THREAD_CPUTIME_ID reports CPU time for this thread only.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0
            || time.tv_sec < 0
            || time.tv_nsec < 0
        {
            return None;
        }
        let ns = (time.tv_sec as u128)
            .saturating_mul(1_000_000_000)
            .saturating_add(time.tv_nsec as u128);
        Some(ns.min(u64::MAX as u128) as u64)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn record_match_cpu_elapsed(counter: &AtomicU64, observations: &AtomicU64, elapsed_ns: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(elapsed_ns))
    });
    let _ = observations.fetch_update(Ordering::Release, Ordering::Relaxed, |current| {
        Some(current.saturating_add(1))
    });
}

pub fn record_match_prepare_cpu_elapsed(elapsed_ns: u64) {
    if ENABLED.load(Ordering::Relaxed) {
        let meter = global();
        record_match_cpu_elapsed(
            &meter.match_prepare_cpu_elapsed_ns,
            &meter.match_prepare_cpu_observations,
            elapsed_ns,
        );
    }
}

pub fn record_match_assess_cpu_elapsed(elapsed_ns: u64) {
    if ENABLED.load(Ordering::Relaxed) {
        let meter = global();
        record_match_cpu_elapsed(
            &meter.match_assess_cpu_elapsed_ns,
            &meter.match_assess_cpu_observations,
            elapsed_ns,
        );
    }
}

/// Describe exactly what process-window counters include and leave opaque.
pub fn coverage() -> Value {
    json!({
        "scope": "process_window",
        "scopeNote": "Any overlapping instrumented work in this process is included; windows are not exclusive operation attribution",
        "httpSendAttempts": {
            "meaning": "instrumented Rust reqwest send invocations",
            "instrumentedModules": [
                "acquisition", "artist_alias_discovery", "artists",
                "bandcamp", "bandcamp_fallback", "monochrome",
                "monochrome_resolver", "youtube_direct_experiment",
                "youtube_music", "youtube_web_search", "remote_playlists"
            ],
            "excludes": [
                "automatic reqwest redirect hops",
                "yt-dlp internal requests, retries, and redirects",
                "uninstrumented modules"
            ]
        },
        "ytDlpSuccessfulSpawns": {
            "meaning": "successful child process starts from acquisition run_tool_until",
            "excludes": ["HTTP requests made inside yt-dlp", "other subprocess types"]
        },
        "matchingElapsed": {
            "unit": "nanoseconds",
            "meaning": "synchronous wall-clock elapsed in prepare and assess_prepared",
            "notCpuTime": true
        },
        "matchingThreadCpu": {
            "unit": "nanoseconds",
            "clock": "CLOCK_THREAD_CPUTIME_ID",
            "scope": "calling thread during synchronous prepare and assess_prepared",
            "unavailable": "null elapsed and zero observations when unsupported or clock reads fail"
        },
        "physicalOutboundRequestCount": null
    })
}

#[cfg(test)]
mod tests {
    use super::{thread_cpu_time_ns, Meter, Snapshot};
    use std::time::Duration;

    #[test]
    fn local_meter_is_opt_in_and_snapshot_deltas_are_bounded() {
        let meter = Meter::default();
        let before = meter.snapshot();
        meter.add(&meter.http_send_attempts, 3);
        meter.add(&meter.yt_dlp_successful_spawns, 2);
        meter.add(
            &meter.match_prepare_elapsed_ns,
            Duration::from_millis(4).as_nanos() as u64,
        );
        meter.add(
            &meter.match_assess_elapsed_ns,
            Duration::from_micros(750).as_nanos() as u64,
        );
        let after = meter.snapshot();
        assert_eq!(
            after.delta_since(before),
            Snapshot {
                http_send_attempts: 3,
                yt_dlp_successful_spawns: 2,
                match_prepare_elapsed_ns: 4_000_000,
                match_assess_elapsed_ns: 750_000,
                ..Snapshot::default()
            }
        );
        assert_eq!(Snapshot::default().delta_since(after).http_send_attempts, 0);

        meter.add(&meter.http_send_attempts, u64::MAX);
        meter.add(&meter.http_send_attempts, 1);
        assert_eq!(meter.snapshot().http_send_attempts, u64::MAX);
    }

    #[test]
    fn cpu_observations_are_local_and_delta_preserves_unavailable() {
        let meter = Meter::default();
        let before = meter.snapshot();
        assert_eq!(before.match_prepare_cpu_elapsed_ns, None);
        assert_eq!(before.match_prepare_cpu_observations, 0);

        let cpu_before = thread_cpu_time_ns();
        let mut value = 1u64;
        for i in 1..100_000u64 {
            value = value.wrapping_mul(i | 1).rotate_left(7);
        }
        std::hint::black_box(value);
        let cpu_after = thread_cpu_time_ns();
        if let (Some(start), Some(end)) = (cpu_before, cpu_after) {
            assert!(end >= start);
            let elapsed = end - start;
            assert!(elapsed <= 10_000_000_000);
            super::record_match_cpu_elapsed(
                &meter.match_prepare_cpu_elapsed_ns,
                &meter.match_prepare_cpu_observations,
                elapsed,
            );
            let delta = meter.snapshot().delta_since(before);
            assert_eq!(delta.match_prepare_cpu_elapsed_ns, Some(elapsed));
            assert_eq!(delta.match_prepare_cpu_observations, 1);
        } else {
            assert_eq!(
                meter
                    .snapshot()
                    .delta_since(before)
                    .match_prepare_cpu_elapsed_ns,
                None
            );
            assert_eq!(
                meter
                    .snapshot()
                    .delta_since(before)
                    .match_prepare_cpu_observations,
                0
            );
        }
        assert_eq!(Snapshot::default().match_assess_cpu_elapsed_ns, None);
    }
}
