//! Per-request cancellation for identifying the currently playing native track.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

const MAX_CANCELLED_IDS: usize = 128;
const MAX_ACTIVE_JOBS: usize = 4;

struct Inner {
    next_job: u64,
    active: HashMap<String, (u64, watch::Sender<bool>)>,
    cancelled: HashSet<String>,
    cancelled_order: VecDeque<String>,
    closed: bool,
    active_count: watch::Sender<usize>,
}

/// Request registry. A caller ID names one active request; duplicate active IDs
/// are rejected and never replace the original caller's cancellation channel.
#[derive(Clone)]
pub struct PlayingTrackIdentifications(Arc<Mutex<Inner>>);

impl Default for PlayingTrackIdentifications {
    fn default() -> Self {
        let (active_count, _) = watch::channel(0);
        Self(Arc::new(Mutex::new(Inner {
            next_job: 0,
            active: HashMap::new(),
            cancelled: HashSet::new(),
            cancelled_order: VecDeque::new(),
            closed: false,
            active_count,
        })))
    }
}

pub struct PlayingTrackIdentificationJob {
    inner: Arc<Mutex<Inner>>,
    id: String,
    job_id: u64,
    cancelled: watch::Receiver<bool>,
}

impl PlayingTrackIdentifications {
    /// Start a request, generating an ID for legacy callers that pass `None`.
    pub fn begin(
        &self,
        caller_id: Option<String>,
    ) -> Result<PlayingTrackIdentificationJob, String> {
        let id = match caller_id {
            Some(id) => normalize_id(&id)?,
            None => uuid::Uuid::new_v4().to_string(),
        };
        let mut inner = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if inner.closed {
            return Err("Audio identification is shutting down".into());
        }
        if inner.cancelled.contains(&id) {
            return Err("Audio identification was cancelled before it began".into());
        }
        if inner.active.contains_key(&id) {
            return Err("An audio identification request with this ID is already active".into());
        }
        if inner.active.len() >= MAX_ACTIVE_JOBS {
            return Err("Too many audio identification requests are already active".into());
        }
        inner.next_job = inner.next_job.wrapping_add(1);
        let job_id = inner.next_job;
        let (sender, cancelled) = watch::channel(false);
        inner.active.insert(id.clone(), (job_id, sender));
        let active_count = inner.active.len();
        inner.active_count.send_replace(active_count);
        Ok(PlayingTrackIdentificationJob {
            inner: self.0.clone(),
            id,
            job_id,
            cancelled,
        })
    }

    /// Cancel just this ID. Unknown valid IDs are retained briefly so a cancel
    /// that races ahead of `begin` cannot accidentally start recognition.
    pub fn cancel(&self, id: &str) -> bool {
        let Ok(id) = normalize_id(id) else {
            return false;
        };
        let mut inner = self.0.lock().unwrap_or_else(|error| error.into_inner());
        remember_cancelled(&mut inner, id.clone());
        if let Some((_, sender)) = inner.active.get(&id) {
            sender.send_replace(true);
        }
        true
    }

    /// Close admission, cancel every active request, and return a receiver
    /// that observes when all request guards have completed cleanup.
    pub fn close_and_cancel_all(&self) -> watch::Receiver<usize> {
        let mut inner = self.0.lock().unwrap_or_else(|error| error.into_inner());
        inner.closed = true;
        for (_, sender) in inner.active.values() {
            sender.send_replace(true);
        }
        inner.active_count.subscribe()
    }
}

impl PlayingTrackIdentificationJob {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    pub fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.clone()
    }
}

impl Drop for PlayingTrackIdentificationJob {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner
            .active
            .get(&self.id)
            .is_some_and(|(job_id, _)| *job_id == self.job_id)
        {
            inner.active.remove(&self.id);
            let active_count = inner.active.len();
            inner.active_count.send_replace(active_count);
        }
    }
}

fn normalize_id(id: &str) -> Result<String, String> {
    if id.is_empty() || id.len() > 64 {
        return Err("Audio identification request ID is invalid".into());
    }
    uuid::Uuid::parse_str(id)
        .map(|uuid| uuid.to_string())
        .map_err(|_| "Audio identification request ID is invalid".into())
}

fn remember_cancelled(inner: &mut Inner, id: String) {
    if inner.cancelled.insert(id.clone()) {
        inner.cancelled_order.push_back(id);
    }
    while inner.cancelled_order.len() > MAX_CANCELLED_IDS {
        if let Some(expired) = inner.cancelled_order.pop_front() {
            inner.cancelled.remove(&expired);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll},
    };

    fn id(n: u128) -> String {
        uuid::Uuid::from_u128(n).to_string()
    }

    #[test]
    fn cancellation_is_scoped_and_duplicate_ids_do_not_replace_callers() {
        let requests = PlayingTrackIdentifications::default();
        let mut first = requests.begin(Some(id(1))).unwrap();
        let second = requests.begin(Some(id(2))).unwrap();
        assert!(requests.begin(Some(id(1))).is_err());
        assert!(!first.is_cancelled());
        assert!(requests.cancel(&id(1)));
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }

    #[test]
    fn cancel_before_begin_is_remembered_and_tombstones_are_bounded() {
        let requests = PlayingTrackIdentifications::default();
        assert!(requests.cancel(&id(1)));
        assert!(requests.begin(Some(id(1))).is_err());
        for n in 2..=MAX_CANCELLED_IDS as u128 + 1 {
            assert!(requests.cancel(&id(n)));
        }
        assert!(requests.begin(Some(id(1))).is_ok());
        assert!(requests
            .begin(Some(id(MAX_CANCELLED_IDS as u128 + 1)))
            .is_err());
    }

    #[test]
    fn dropping_a_request_releases_only_its_registration() {
        let requests = PlayingTrackIdentifications::default();
        let first = requests.begin(Some(id(1))).unwrap();
        let other = requests.begin(Some(id(2))).unwrap();
        drop(first);
        assert!(requests.begin(Some(id(1))).is_ok());
        assert!(!other.is_cancelled());
    }

    #[test]
    fn active_identification_requests_are_bounded() {
        let requests = PlayingTrackIdentifications::default();
        let jobs = (1..=MAX_ACTIVE_JOBS)
            .map(|n| requests.begin(Some(id(n as u128))).unwrap())
            .collect::<Vec<_>>();
        assert!(requests.begin(Some(id(99))).is_err());
        drop(jobs);
        assert!(requests.begin(Some(id(99))).is_ok());
    }

    #[tokio::test]
    async fn shutdown_closes_admission_cancels_active_and_notifies_when_drained() {
        let requests = PlayingTrackIdentifications::default();
        let job = requests.begin(Some(id(10))).unwrap();
        let mut cancelled = job.cancellation();
        let mut drained = requests.close_and_cancel_all();
        assert!(cancelled.changed().await.is_ok());
        assert!(*cancelled.borrow());
        assert!(requests.begin(Some(id(11))).is_err());

        let release = tokio::spawn(async move {
            tokio::task::yield_now().await;
            drop(job);
        });
        while *drained.borrow_and_update() != 0 {
            drained.changed().await.unwrap();
        }
        release.await.unwrap();
        assert!(requests.begin(None).is_err());
    }

    #[tokio::test]
    async fn cancellation_drops_only_the_matching_recognition_future() {
        struct DropProbe(Arc<AtomicBool>);
        impl Future for DropProbe {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Pending
            }
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let requests = PlayingTrackIdentifications::default();
        let job = requests.begin(Some(id(1))).unwrap();
        let mut cancelled = job.cancellation();
        let dropped = Arc::new(AtomicBool::new(false));
        let mut work = Box::pin(DropProbe(dropped.clone()));
        let request_id = id(1);
        let cancel_requests = requests.clone();
        let cancel = tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel_requests.cancel(&request_id);
        });
        tokio::select! {
            changed = cancelled.changed() => assert!(changed.is_ok()),
            _ = &mut work => panic!("recognition future should remain pending until cancelled"),
        }
        drop(work);
        cancel.await.unwrap();
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn distinct_requests_can_repeat_for_separate_segments_of_one_queue_item() {
        let requests = PlayingTrackIdentifications::default();
        let first = requests.begin(None).unwrap();
        let first_id = first.id().to_owned();
        drop(first);
        let second = requests.begin(None).unwrap();
        assert_ne!(first_id, second.id());
    }
}
