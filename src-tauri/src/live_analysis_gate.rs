//! Shared admission gate for CPU-bound live playback estimators.
//!
//! The permit is moved into the blocking worker so cancelling its async caller
//! cannot admit another expensive analysis while the first worker still runs.

use std::sync::{Arc, OnceLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(crate) struct LiveAnalysisGate(Arc<Semaphore>);

impl LiveAnalysisGate {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Semaphore::new(1)))
    }

    pub(crate) fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        self.0.clone().try_acquire_owned().ok()
    }
}

static GLOBAL: OnceLock<LiveAnalysisGate> = OnceLock::new();

pub(crate) fn try_acquire() -> Option<OwnedSemaphorePermit> {
    GLOBAL.get_or_init(LiveAnalysisGate::new).try_acquire()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_worker_is_admitted_until_its_owned_permit_is_dropped() {
        let gate = LiveAnalysisGate::new();
        let worker = {
            let worker_permit = gate.try_acquire().expect("first worker admitted");
            move || drop(worker_permit)
        };
        assert!(
            gate.try_acquire().is_none(),
            "concurrent worker must not queue"
        );
        drop(worker);
        assert!(
            gate.try_acquire().is_some(),
            "a completed worker releases admission"
        );
    }
}
