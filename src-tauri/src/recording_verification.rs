//! Shared admission control for expensive recording evidence work.
//! Foreground resolution never waits for admission. No queue accumulates when
//! the worker is busy; callers can retry after their ordinary cache cooldown.
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Default)]
pub(crate) struct VerificationGate {
    active: Mutex<HashSet<String>>,
}
pub(crate) struct VerificationLease {
    gate: Arc<VerificationGate>,
    key: String,
}
impl VerificationGate {
    pub(crate) fn try_start(self: &Arc<Self>, key: String) -> Option<VerificationLease> {
        let mut active = self.active.lock().ok()?;
        if !active.is_empty() {
            return None;
        }
        active.insert(key.clone());
        Some(VerificationLease {
            gate: self.clone(),
            key,
        })
    }
}
impl Drop for VerificationLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.active.lock() {
            active.remove(&self.key);
        }
    }
}
pub(crate) fn try_start(key: String) -> Option<VerificationLease> {
    static GATE: OnceLock<Arc<VerificationGate>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(VerificationGate::default()))
        .try_start(key)
}

/// Evidence is reusable across modes; an acceptance decision is not. Include
/// all request identity fields and parser version, but never the selected mode.
pub(crate) fn request_fingerprint(track: &crate::models::Track) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::json!({
                "parser":crate::recording_identity::PARSER_VERSION,
                "id":track.id,"provider":track.provider,"providerId":track.provider_id,
                "sourceUrl":track.source_url,"title":track.title,"artist":track.artist,
                "album":track.album,"version":track.version,"duration":track.duration_seconds,
                "isrc":track.isrc,"provenance":track.provenance
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
}
