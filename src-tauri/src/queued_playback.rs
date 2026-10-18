//! Transport-independent, bounded playback operation ownership. No runtime dependencies.
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Preparing,
    Accepted,
    Playing,
    Paused,
    Failed,
    Exhausted,
    Cancelled,
    Superseded,
}
impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Accepted => "accepted",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Exhausted => "exhausted",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Exhausted | Self::Cancelled | Self::Superseded
        )
    }
}
#[derive(Clone, Debug)]
pub struct Operation<T> {
    pub id: String,
    pub transport: String,
    pub tracks: Option<Vec<String>>,
    pub status: Status,
    pub receipt: Option<T>,
    pub detail: Option<T>,
}
pub struct Registry<T> {
    rows: VecDeque<Operation<T>>,
    capacity: usize,
    seen: Vec<u64>,
}
impl<T: Clone + PartialEq> Registry<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            rows: VecDeque::new(),
            capacity,
            seen: vec![0; 16384], // Fixed 128 KiB, four-hash fail-closed seen-ID filter.
        }
    }
    fn seen_indices(id: &str) -> [usize; 4] {
        use std::hash::{Hash, Hasher};
        std::array::from_fn(|salt| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            salt.hash(&mut h);
            id.hash(&mut h);
            (h.finish() as usize) % 1_048_576
        })
    }
    fn was_seen(&self, id: &str) -> bool {
        Self::seen_indices(id)
            .iter()
            .all(|bit| self.seen[bit / 64] & (1u64 << (bit % 64)) != 0)
    }
    fn remember(&mut self, id: &str) {
        for bit in Self::seen_indices(id) {
            self.seen[bit / 64] |= 1u64 << (bit % 64);
        }
    }
    fn room(&mut self) -> Result<(), String> {
        if self.rows.len() < self.capacity {
            return Ok(());
        }
        if let Some(i) = self.rows.iter().position(|r| r.status.terminal()) {
            self.rows.remove(i);
            Ok(())
        } else {
            Err("Playback operation registry is full".into())
        }
    }
    pub fn preparing(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .filter(|row| row.status == Status::Preparing)
            .map(|row| (row.id.clone(), row.transport.clone()))
            .collect()
    }
    pub fn get(&self, id: &str) -> Option<Operation<T>> {
        self.rows.iter().find(|r| r.id == id).cloned()
    }
    /// Returns false for an exact retry, including a cancelled/pre-cancelled operation.
    pub fn begin(
        &mut self,
        id: String,
        transport: String,
        tracks: Vec<String>,
    ) -> Result<bool, String> {
        if let Some(row) = self.rows.iter().find(|r| r.id == id) {
            if row.tracks.as_ref().is_some_and(|old| old != &tracks) {
                return Err("operationId already belongs to a different track sequence".into());
            }
            return Ok(false);
        }
        if self.was_seen(&id) {
            return Err("Playback operation ID is unknown or expired; use a new UUID (retired IDs cannot restart playback)".into());
        }
        self.room()?;
        self.remember(&id);
        for row in &mut self.rows {
            if row.status == Status::Preparing {
                row.status = Status::Superseded;
            }
        }
        self.rows.push_back(Operation {
            id,
            transport,
            tracks: Some(tracks),
            status: Status::Preparing,
            receipt: None,
            detail: None,
        });
        Ok(true)
    }
    /// Accept and timeout/cancel must be called under the same external mutex.
    pub fn accept(&mut self, id: &str, transport: &str, receipt: T) -> bool {
        let Some(index) = self
            .rows
            .iter()
            .position(|r| r.id == id && r.transport == transport)
        else {
            return false;
        };
        if let Some(existing) = &self.rows[index].receipt {
            return existing == &receipt && !self.rows[index].status.terminal();
        }
        if self.rows[index].status != Status::Preparing {
            return false;
        }
        for (i, row) in self.rows.iter_mut().enumerate() {
            if i != index
                && matches!(
                    row.status,
                    Status::Accepted | Status::Playing | Status::Paused
                )
            {
                row.status = Status::Superseded;
            }
        }
        let row = &mut self.rows[index];
        row.receipt = Some(receipt);
        row.status = Status::Accepted;
        true
    }
    /// Dropping a waiter can expire preparation, but never an accepted playback owner.
    pub fn expire(&mut self, id: &str, transport: &str) -> bool {
        let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.id == id && r.transport == transport)
        else {
            return false;
        };
        if row.status != Status::Preparing {
            return false;
        };
        row.status = Status::Cancelled;
        true
    }
    pub fn cancel(&mut self, id: &str) -> Result<bool, String> {
        if let Some(row) = self.rows.iter_mut().find(|r| r.id == id) {
            if row.status.terminal() {
                return Ok(false);
            };
            row.status = Status::Cancelled;
            return Ok(true);
        }
        if self.was_seen(id) {
            return Ok(false);
        }
        self.room()?;
        self.remember(id);
        self.rows.push_back(Operation {
            id: id.into(),
            transport: String::new(),
            tracks: None,
            status: Status::Cancelled,
            receipt: None,
            detail: None,
        });
        Ok(true)
    }
    pub fn update(&mut self, id: &str, transport: &str, status: Status, detail: T) -> bool {
        let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.id == id && r.transport == transport)
        else {
            return false;
        };
        if row.status.terminal() || matches!(status, Status::Preparing | Status::Accepted) {
            return false;
        }
        // Preparation errors are reportable, but playback cannot be marked playing before acceptance.
        if row.status == Status::Preparing && matches!(status, Status::Playing | Status::Paused) {
            return false;
        }
        row.status = status;
        row.detail = Some(detail);
        true
    }
}
pub struct ReceiptIdentity<'a> {
    pub tracks: &'a [&'a str],
    pub entries: &'a [&'a str],
    pub revision: &'a str,
    pub generation: Option<u64>,
}
pub fn valid_webview_receipt(expected: &[String], identity: ReceiptIdentity<'_>) -> bool {
    if expected.is_empty()
        || expected.len() > 500
        || identity.tracks.len() != expected.len()
        || identity.entries.len() != expected.len()
        || identity.generation.is_none()
    {
        return false;
    }
    if !expected
        .iter()
        .map(String::as_str)
        .eq(identity.tracks.iter().copied())
    {
        return false;
    }
    if identity.revision.len() > 64 {
        return false;
    }
    let parts = identity.revision.split(':').collect::<Vec<_>>();
    if parts.len() != 5
        || parts[0].parse::<usize>().ok() != Some(expected.len())
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
        || parts[1..].iter().any(|p| p.parse::<u32>().is_err())
    {
        return false;
    }
    let mut unique = std::collections::HashSet::new();
    identity.entries.iter().all(|id| {
        !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control) && unique.insert(*id)
    })
}
pub fn native_failure_status(queue_exhausted: bool) -> Status {
    if queue_exhausted {
        Status::Exhausted
    } else {
        Status::Failed
    }
}
pub fn generation_is_current(
    incoming: Option<u64>,
    accepted: Option<u64>,
    previous: Option<u64>,
) -> bool {
    match accepted {
        None => true,
        Some(accepted) => incoming.is_some_and(|value| {
            value >= accepted && previous.is_none_or(|previous| value >= previous)
        }),
    }
}
pub fn native_command_preserves_root(action: &str) -> bool {
    matches!(action, "play" | "pause" | "resume" | "seek" | "volume")
}
pub fn native_start_is_current(expected: u64, current: u64, permitted: bool) -> bool {
    expected == current && permitted
}
pub fn native_stop_is_owned(expected: &str, current: Option<&str>) -> bool {
    current == Some(expected)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn begin(r: &mut Registry<&'static str>, id: &str) {
        assert!(r
            .begin(
                id.into(),
                format!("rpc-{id}"),
                vec!["a".into(), "a".into(), "b".into()]
            )
            .unwrap());
    }
    #[test]
    fn receipt_retry_requires_identical_occurrences_revision_and_generation() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        assert!(r.accept("one", "rpc-one", "receipt-generation-1"));
        assert!(r.accept("one", "rpc-one", "receipt-generation-1"));
        assert!(!r.accept("one", "rpc-one", "tampered-generation-2"));
        assert_eq!(r.get("one").unwrap().receipt, Some("receipt-generation-1"));
    }
    #[test]
    fn receipt_shape_enforces_exact_duplicates_unique_occurrences_and_bounded_fingerprint() {
        let expected = vec!["a".into(), "a".into(), "b".into()];
        let tracks = ["a", "a", "b"];
        let entries = ["a-1", "a-2", "b-1"];
        let oversized_revision = "x".repeat(100000);
        let valid = |tracks, entries, revision, generation| {
            valid_webview_receipt(
                &expected,
                ReceiptIdentity {
                    tracks,
                    entries,
                    revision,
                    generation,
                },
            )
        };
        assert!(valid(&tracks, &entries, "3:1:2:3:4294967295", Some(1)));
        assert!(!valid(&["a", "b", "b"], &entries, "3:1:2:3:4", Some(1)));
        assert!(!valid(
            &tracks,
            &["same", "same", "other"],
            "3:1:2:3:4",
            Some(1)
        ));
        assert!(!valid(&tracks, &entries, "2:1:2:3:4", Some(1)));
        assert!(!valid(&tracks, &entries, "3:1:2:3:4294967296", Some(1)));
        assert!(!valid(&tracks, &entries, &oversized_revision, Some(1)));
        assert!(!valid(&tracks, &entries, "3:1:2:3:4", None));
    }
    #[test]
    fn accepted_owner_survives_lost_receipt_timeout_and_exact_retry() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        assert!(r.accept("one", "rpc-one", "exact-3-occurrences"));
        assert!(!r.expire("one", "rpc-one"));
        assert!(!r
            .begin(
                "one".into(),
                "retry".into(),
                vec!["a".into(), "a".into(), "b".into()]
            )
            .unwrap());
        assert_eq!(r.get("one").unwrap().receipt, Some("exact-3-occurrences"));
        assert_eq!(r.get("one").unwrap().status, Status::Accepted);
    }
    #[test]
    fn deadline_winning_rejects_late_acceptance() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        assert!(r.expire("one", "rpc-one"));
        assert!(!r.accept("one", "rpc-one", "late"));
        assert_eq!(r.get("one").unwrap().receipt, None);
    }
    #[test]
    fn precancel_and_payload_collision_are_not_new_starts() {
        let mut r = Registry::new(4);
        assert!(r.cancel("one").unwrap());
        assert!(!r
            .begin("one".into(), "rpc".into(), vec!["a".into()])
            .unwrap());
        assert!(!r.accept("one", "rpc", "late"));
        begin(&mut r, "two");
        assert!(r
            .begin("two".into(), "retry".into(), vec!["b".into()])
            .is_err());
    }
    #[test]
    fn old_completion_cannot_revive_or_replace_new_owner() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        r.accept("one", "rpc-one", "q1");
        begin(&mut r, "two");
        r.accept("two", "rpc-two", "q2");
        assert_eq!(r.get("one").unwrap().status, Status::Superseded);
        assert!(!r.update("one", "rpc-one", Status::Playing, "late"));
        assert!(!r.update("two", "wrong-rpc", Status::Failed, "late"));
        assert_eq!(r.get("two").unwrap().status, Status::Accepted);
    }
    #[test]
    fn cancellation_and_exhaustion_are_terminal() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        r.accept("one", "rpc-one", "q1");
        assert!(r.cancel("one").unwrap());
        assert!(!r.cancel("one").unwrap());
        assert!(!r.update("one", "rpc-one", Status::Playing, "late"));
        begin(&mut r, "two");
        r.accept("two", "rpc-two", "q2");
        assert!(r.update("two", "rpc-two", Status::Exhausted, "19-owned-failures"));
        assert!(!r.update("two", "rpc-two", Status::Playing, "late"));
    }
    #[test]
    fn evicted_accepted_uuid_never_replays_after_more_than_registry_capacity() {
        let mut r = Registry::new(2);
        begin(&mut r, "first");
        assert!(r.accept("first", "rpc-first", "q"));
        r.cancel("first").unwrap();
        for i in 0..200 {
            let id = format!("later-{i}");
            begin(&mut r, &id);
            r.cancel(&id).unwrap();
        }
        assert!(r.get("first").is_none());
        assert!(r
            .begin(
                "first".into(),
                "retry".into(),
                vec!["a".into(), "a".into(), "b".into()]
            )
            .unwrap_err()
            .contains("unknown or expired"));
        assert!(!r.cancel("first").unwrap());
        assert!(!r.accept("first", "retry", "replayed"));
    }
    #[test]
    fn bounded_registry_never_evicts_live_operations() {
        let mut r = Registry::new(1);
        begin(&mut r, "one");
        assert!(r
            .begin("two".into(), "rpc-two".into(), vec!["a".into()])
            .is_err());
        r.cancel("one").unwrap();
        begin(&mut r, "two");
        assert!(r.get("one").is_none());
    }
    #[test]
    fn paused_owner_is_superseded_by_replacement_and_cancel_is_terminal() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        r.accept("one", "rpc-one", "q1");
        r.update("one", "rpc-one", Status::Paused, "paused");
        begin(&mut r, "two");
        r.accept("two", "rpc-two", "q2");
        assert_eq!(r.get("one").unwrap().status, Status::Superseded);
        r.update("two", "rpc-two", Status::Paused, "paused");
        assert!(r.cancel("two").unwrap());
        assert!(!r.update("two", "rpc-two", Status::Playing, "late resume"));
    }
    #[test]
    fn delayed_phase_updates_cannot_overwrite_a_newer_attempt() {
        assert!(generation_is_current(Some(12), Some(10), Some(12)));
        assert!(!generation_is_current(Some(11), Some(10), Some(12)));
        assert!(!generation_is_current(None, Some(10), None));
        assert!(generation_is_current(None, None, None));
    }
    #[test]
    fn paused_seek_and_resume_keep_the_existing_root_active() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        r.accept("one", "rpc-one", "q");
        assert!(r.update("one", "rpc-one", Status::Paused, "same owner"));
        assert!(!r.get("one").unwrap().status.terminal());
        assert!(!r.expire("one", "rpc-one"));
        assert!(r.update("one", "rpc-one", Status::Playing, "same owner"));
        for action in ["pause", "resume", "seek", "volume"] {
            assert!(native_command_preserves_root(action));
        }
        assert!(!native_command_preserves_root("stop"));
        assert!(!native_command_preserves_root("select"));
    }
    #[test]
    fn native_root_cancel_never_stops_a_replacement() {
        assert!(native_stop_is_owned("album-a", Some("album-a")));
        assert!(!native_stop_is_owned("album-a", Some("album-b")));
        assert!(!native_stop_is_owned("album-a", None));
    }
    #[test]
    fn native_start_gate_rechecks_epoch_and_cancel_after_command_lock_wait() {
        assert!(native_start_is_current(7, 7, true));
        assert!(!native_start_is_current(7, 8, true));
        assert!(!native_start_is_current(7, 7, false));
    }
    #[test]
    fn newer_preparation_invalidates_delayed_start_without_retiring_accepted_owner() {
        let mut r = Registry::new(4);
        begin(&mut r, "one");
        r.accept("one", "rpc-one", "q1");
        begin(&mut r, "two");
        assert_eq!(r.get("one").unwrap().status, Status::Accepted);
        begin(&mut r, "three");
        assert_eq!(r.get("two").unwrap().status, Status::Superseded);
        assert!(!r.accept("two", "rpc-two", "late"));
        assert!(r.accept("three", "rpc-three", "q3"));
        assert_eq!(r.get("one").unwrap().status, Status::Superseded);
    }
    #[test]
    fn native_failure_requires_explicit_queue_exhaustion_marker() {
        assert_eq!(native_failure_status(false), Status::Failed);
        assert_eq!(native_failure_status(true), Status::Exhausted);
    }
    #[test]
    fn both_mutex_linearizations_of_accept_and_expire() {
        for accept_first in [true, false] {
            let r = std::sync::Mutex::new(Registry::new(2));
            begin(&mut r.lock().unwrap(), "one");
            if accept_first {
                assert!(r.lock().unwrap().accept("one", "rpc-one", "q"));
                assert!(!r.lock().unwrap().expire("one", "rpc-one"));
            } else {
                assert!(r.lock().unwrap().expire("one", "rpc-one"));
                assert!(!r.lock().unwrap().accept("one", "rpc-one", "q"));
            }
        }
    }
}
