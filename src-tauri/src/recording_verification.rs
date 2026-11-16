//! Shared admission control for expensive recording evidence work.
//! Foreground resolution never waits for admission. No queue accumulates when
//! the worker is busy; callers can retry after their ordinary cache cooldown.
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Default)]
pub(crate) struct VerificationGate {
    active: Mutex<GateState>,
}
#[derive(Default)]
struct GateState {
    keys: HashSet<String>,
    stop: Option<tokio::sync::watch::Sender<bool>>,
    policy: crate::background_work_policy::BackgroundWorkPolicy,
}
pub(crate) struct VerificationLease {
    gate: Arc<VerificationGate>,
    key: String,
    pub(crate) cancel: tokio::sync::watch::Receiver<bool>,
}
impl VerificationGate {
    pub(crate) fn try_start(self: &Arc<Self>, key: String) -> Option<VerificationLease> {
        let mut active = self.active.lock().ok()?;
        if !active.keys.is_empty() {
            return None;
        }
        let (stop, cancel) = tokio::sync::watch::channel(false);
        active.stop = Some(stop);
        active.keys.insert(key.clone());
        Some(VerificationLease {
            gate: self.clone(),
            key,
            cancel,
        })
    }
}
impl Drop for VerificationLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.active.lock() {
            active.keys.remove(&self.key);
            active.stop = None;
        }
    }
}
fn global_gate() -> &'static Arc<VerificationGate> {
    static GATE: OnceLock<Arc<VerificationGate>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(VerificationGate::default()))
}
pub(crate) fn try_start(key: String) -> Option<VerificationLease> {
    global_gate().try_start(key)
}
impl VerificationGate {
    fn foreground(&self) {
        if let Ok(mut state) = self.active.lock() {
            state.policy.foreground(std::time::Instant::now());
            if let Some(stop) = &state.stop {
                stop.send_replace(true);
            }
        }
    }
    fn observe_playback(&self, status: &str) {
        if let Ok(mut state) = self.active.lock() {
            if state
                .policy
                .observe_playback(status, std::time::Instant::now())
            {
                if let Some(stop) = &state.stop {
                    stop.send_replace(true);
                }
            }
        }
    }
}
pub(crate) fn foreground() {
    global_gate().foreground();
}
pub(crate) fn observe_playback(status: &str) {
    global_gate().observe_playback(status);
}
impl VerificationLease {
    pub(crate) async fn wait_until_quiet(&self) -> bool {
        let end = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            if *self.cancel.borrow() || tokio::time::Instant::now() >= end {
                return false;
            }
            let quiet = self.gate.active.lock().ok().is_some_and(|state| {
                state
                    .policy
                    .quiet_remaining(std::time::Instant::now())
                    .is_zero()
            });
            if quiet {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

/// Evidence is reusable across modes; an acceptance decision is not. Include
/// all request identity fields and parser version, but never the selected mode.
pub(crate) fn request_fingerprint(track: &crate::models::Track) -> String {
    use sha2::{Digest, Sha256};
    let manual_fields = track
        .provenance
        .get("manualFields")
        .and_then(|v| v.as_object())
        .map(|fields| {
            fields
                .iter()
                .filter(|(_, value)| **value == serde_json::Value::Bool(true))
                .map(|(name, _)| name.clone())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::json!({
                "parser":crate::recording_identity::PARSER_VERSION,
                "id":track.id,"provider":track.provider,"providerId":track.provider_id,
                "sourceUrl":track.source_url,"title":track.title,"artist":track.artist,
                "album":track.album,"version":track.version,"duration":track.duration_seconds,
                "isrc":track.isrc,"manualFields":manual_fields
            })
            .to_string()
            .as_bytes()
        )
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_is_nonblocking_deduplicated_and_released_on_cancellation() {
        let gate = Arc::new(VerificationGate::default());
        let lease = gate.try_start("first".into()).unwrap();
        assert!(gate.try_start("first".into()).is_none());
        assert!(gate.try_start("other".into()).is_none());
        drop(lease);
        assert!(gate.try_start("other".into()).is_some());
    }

    #[tokio::test]
    async fn quiet_admission_is_immediate_until_foreground_work_defers_it() {
        let gate = Arc::new(VerificationGate::default());
        let lease = gate.try_start("quiet".into()).unwrap();
        assert!(lease.wait_until_quiet().await);
        drop(lease);

        let deferred = gate.try_start("deferred".into()).unwrap();
        gate.foreground();
        assert!(*deferred.cancel.borrow());
        assert!(!deferred.wait_until_quiet().await);
    }

    #[tokio::test]
    async fn entering_playback_cancels_admitted_work_and_resume_cancels_again() {
        let gate = Arc::new(VerificationGate::default());
        let first = gate.try_start("first".into()).unwrap();
        gate.observe_playback("playing");
        assert!(*first.cancel.borrow());
        assert!(!first.wait_until_quiet().await);
        drop(first);

        gate.observe_playback("paused");
        let resumed = gate.try_start("resumed".into()).unwrap();
        gate.observe_playback("buffering");
        assert!(*resumed.cancel.borrow());
        assert!(!resumed.wait_until_quiet().await);
    }
    #[test]
    fn changed_recording_evidence_invalidates_cache_identity() {
        let mut track = crate::models::Track::default();
        let first = request_fingerprint(&track);
        track.provenance = serde_json::json!({"manualFields":{"artist":true}});
        assert_ne!(first, request_fingerprint(&track));
        let second = request_fingerprint(&track);
        track.isrc = Some("GBABC2400001".into());
        assert_ne!(second, request_fingerprint(&track));
    }

    #[test]
    fn absent_and_empty_manual_flags_have_the_same_fingerprint() {
        let mut track = crate::models::Track::default();
        let missing = request_fingerprint(&track);
        track.provenance = serde_json::json!({"manualFields":{}});
        assert_eq!(missing, request_fingerprint(&track));
        track.provenance = serde_json::json!({});
        assert_eq!(missing, request_fingerprint(&track));
    }
}
