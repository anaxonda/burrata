//! Bounded cancellation registry for independent, read-only provider requests.
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

const MAX_ACTIVE_READS: usize = 64;

#[derive(Default)]
struct RegistryState {
    active: HashMap<String, watch::Sender<bool>>,
    cancelled_before_start: HashSet<String>,
}

#[derive(Default)]
pub struct ProviderReadRequests {
    state: Mutex<RegistryState>,
}

pub struct ReadGuard {
    id: String,
    owner: Arc<ProviderReadRequests>,
    cancelled: watch::Receiver<bool>,
}

impl ProviderReadRequests {
    pub fn begin(self: &Arc<Self>, id: String) -> Result<ReadGuard, String> {
        if id.len() > 64 || uuid::Uuid::parse_str(&id).is_err() {
            return Err("Remote read request ID must be a UUID".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Remote read registry is unavailable")?;
        if state.active.len() >= MAX_ACTIVE_READS {
            return Err("Too many remote reads are already active".into());
        }
        if state.active.contains_key(&id) {
            return Err("Remote read request ID is already active".into());
        }
        let cancelled_early = state.cancelled_before_start.remove(&id);
        let (sender, cancelled) = watch::channel(cancelled_early);
        state.active.insert(id.clone(), sender);
        Ok(ReadGuard {
            id,
            owner: self.clone(),
            cancelled,
        })
    }

    pub fn cancel(&self, id: &str) -> bool {
        if id.len() > 64 || uuid::Uuid::parse_str(id).is_err() {
            return false;
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if let Some(sender) = state.active.get(id).cloned() {
            return sender.send(true).is_ok();
        }
        if state.cancelled_before_start.len() >= MAX_ACTIVE_READS {
            state.cancelled_before_start.clear();
        }
        state.cancelled_before_start.insert(id.to_owned());
        true
    }

    fn finish(&self, id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.active.remove(id);
        }
    }

    pub async fn run<T, F, Fut>(
        self: &Arc<Self>,
        id: Option<String>,
        operation: F,
    ) -> Result<T, String>
    where
        T: Send,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, String>> + Send,
    {
        let Some(id) = id else {
            return operation().await;
        };
        let mut guard = self.begin(id)?;
        if guard.is_cancelled() {
            return Err("Remote read was canceled".into());
        }
        let operation = operation();
        tokio::select! {
            biased;
            _ = guard.wait_cancelled() => Err("Remote read was canceled".into()),
            result = operation => result,
        }
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.state.lock().unwrap().active.len()
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        self.owner.finish(&self.id);
    }
}

impl ReadGuard {
    pub(crate) fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.clone()
    }
    fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }
    async fn wait_cancelled(&mut self) {
        if *self.cancelled.borrow_and_update() {
            return;
        }
        let _ = self.cancelled.changed().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn search_guard_propagates_only_its_uuid_and_remembers_early_cancellation() {
        let registry = Arc::new(ProviderReadRequests::default());
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        assert!(registry.cancel(&first));
        let cancelled = registry.begin(first).unwrap();
        let independent = registry.begin(second.clone()).unwrap();
        assert!(*cancelled.cancellation().borrow());
        assert!(!*independent.cancellation().borrow());
        assert!(registry.cancel(&second));
        assert!(*independent.cancellation().borrow());
    }

    #[tokio::test]
    async fn cancellation_drops_only_the_named_read_and_cleans_registry() {
        let registry = Arc::new(ProviderReadRequests::default());
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let dropped = Arc::new(AtomicBool::new(false));
        let marker = Dropped(dropped.clone());
        let registry_for_task = registry.clone();
        let first_for_task = first.clone();
        let running = tokio::spawn(async move {
            registry_for_task
                .run(Some(first_for_task), || async move {
                    let _marker = marker;
                    std::future::pending::<Result<(), String>>().await
                })
                .await
        });
        let second_guard = registry.begin(second.clone()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while registry.active_count() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("read requests should register");
        assert!(registry.cancel(&first));
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), running)
                .await
                .unwrap()
                .unwrap(),
            Err("Remote read was canceled".into())
        );
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(registry.active_count(), 1);
        drop(second_guard);
        assert_eq!(registry.active_count(), 0);
    }

    #[tokio::test]
    async fn cancellation_that_arrives_before_registration_is_honored() {
        let registry = Arc::new(ProviderReadRequests::default());
        let id = uuid::Uuid::new_v4().to_string();
        assert!(registry.cancel(&id));
        let dropped = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let marker = Dropped(dropped.clone());
        let started_in_operation = started.clone();
        let result = registry
            .run(Some(id), move || {
                started_in_operation.store(true, Ordering::SeqCst);
                async move {
                    let _marker = marker;
                    std::future::pending::<Result<(), String>>().await
                }
            })
            .await;
        assert_eq!(result, Err("Remote read was canceled".into()));
        assert!(!started.load(Ordering::SeqCst));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(registry.active_count(), 0);
    }

    #[test]
    fn registry_rejects_duplicate_invalid_and_excess_requests() {
        let registry = Arc::new(ProviderReadRequests::default());
        assert!(registry.begin("not-a-uuid".into()).is_err());
        let first = uuid::Uuid::new_v4().to_string();
        let guard = registry.begin(first.clone()).unwrap();
        assert!(registry.begin(first).is_err());
        let guards = (0..(MAX_ACTIVE_READS - 1))
            .map(|_| registry.begin(uuid::Uuid::new_v4().to_string()).unwrap())
            .collect::<Vec<_>>();
        assert!(registry.begin(uuid::Uuid::new_v4().to_string()).is_err());
        drop(guards);
        drop(guard);
        assert_eq!(registry.active_count(), 0);
    }
}
