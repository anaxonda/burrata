//! Bounded blocking OS credential reads. Workers return values, never mutate session state.
use std::{future::Future, sync::Arc};
use tokio::sync::Semaphore;

pub(super) const CANCELLED: &str = "Provider request was cancelled";

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Binding {
    pub generation: u64,
    pub session_id: String,
    pub account_id: String,
    pub account_revision: u64,
}

// Caller holds the session control/state locks while comparing and committing.
// A newer confirmation may populate the cache without changing account identity.
// In that case its value wins over the earlier OS read.
pub(super) fn commit<T: Clone>(
    requested: &Binding,
    current: &Binding,
    cache: &mut Option<T>,
    loaded: T,
) -> Result<T, String> {
    if requested != current {
        return Err(CANCELLED.into());
    }
    Ok(cache.get_or_insert(loaded).clone())
}

pub(super) async fn read<T, F, C>(slots: Arc<Semaphore>, cancelled: C, read: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
    C: Future<Output = ()>,
{
    tokio::pin!(cancelled);
    let permit = tokio::select! {
        biased;
        _ = &mut cancelled => return Err(CANCELLED.into()),
        permit = slots.acquire_owned() => permit.map_err(|_| "Provider credential reader is unavailable")?,
    };
    // Dropping a caller cannot stop an OS keyring call. Keep capacity occupied
    // until the worker exits, and let an abandoned return value drop/zeroize.
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read()
    });
    tokio::select! {
        biased;
        _ = &mut cancelled => Err(CANCELLED.into()),
        result = worker => result.map_err(|_| "Provider credential reader failed".to_owned())?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    fn binding(generation: u64) -> Binding {
        Binding {
            generation,
            session_id: "test-session".into(),
            account_id: "test-account".into(),
            account_revision: 1,
        }
    }

    #[test]
    fn stale_completion_never_populates_or_clears_current_cache() {
        for changed in 0..4 {
            let requested = binding(1);
            let mut current = requested.clone();
            match changed {
                0 => current.generation += 1,
                1 => current.session_id.push_str("-new"),
                2 => current.account_id.push_str("-new"),
                _ => current.account_revision += 1,
            }
            for mut cache in [None, Some("new-confirmation")] {
                let before = cache;
                assert!(commit(&requested, &current, &mut cache, "stale-read").is_err());
                assert_eq!(cache, before);
            }
        }
        let current = binding(1);
        let mut cache = Some("new-confirmation");
        assert_eq!(
            commit(&current, &current, &mut cache, "older-read").unwrap(),
            "new-confirmation"
        );
        let mut cache = None;
        assert_eq!(
            commit(&current, &current, &mut cache, "valid-read").unwrap(),
            "valid-read"
        );
    }

    #[tokio::test]
    async fn blocked_os_read_does_not_block_runtime_or_outer_deadline() {
        let slots = Arc::new(Semaphore::new(1));
        let (release, released) = mpsc::channel();
        let (entered, started) = tokio::sync::oneshot::channel();
        let mut reader = tokio::spawn(read(slots.clone(), std::future::pending(), move || {
            let _ = entered.send(());
            released.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(())
        }));
        started.await.unwrap();
        // This timer must run while the fake OS call remains blocked.
        let result = tokio::time::timeout(Duration::from_millis(10), &mut reader).await;
        assert!(result.is_err());
        reader.abort();
        let _ = reader.await;
        assert_eq!(slots.available_permits(), 0);
        release.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(1), slots.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
    }

    #[tokio::test]
    async fn cancellation_keeps_capacity_until_os_read_finishes_and_drops_return_value() {
        struct SecretDrop(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for SecretDrop {
            fn drop(&mut self) {
                if let Some(done) = self.0.take() {
                    let _ = done.send(());
                }
            }
        }
        let slots = Arc::new(Semaphore::new(1));
        let (release, released) = mpsc::channel();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let (dropped, was_dropped) = tokio::sync::oneshot::channel();
        let reader = tokio::spawn(read(
            slots.clone(),
            async {
                let _ = cancelled.await;
            },
            move || {
                let _ = entered.send(());
                released.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(SecretDrop(Some(dropped)))
            },
        ));
        started.await.unwrap();
        cancel.send(()).unwrap();
        assert!(reader.await.unwrap().is_err());
        assert_eq!(slots.available_permits(), 0);
        let second_started = Arc::new(AtomicBool::new(false));
        let observed = second_started.clone();
        let second = read(slots.clone(), std::future::pending(), move || {
            observed.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(tokio::time::timeout(Duration::from_millis(10), second)
            .await
            .is_err());
        assert!(!second_started.load(Ordering::SeqCst));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), was_dropped)
            .await
            .unwrap()
            .unwrap();
        read(slots, std::future::pending(), || Ok(()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancellation_before_admission_never_calls_reader_and_errors_are_preserved() {
        let slots = Arc::new(Semaphore::new(1));
        let result: Result<(), String> =
            read(slots.clone(), async {}, || panic!("cancelled read started")).await;
        assert_eq!(result.err().as_deref(), Some(CANCELLED));
        let result: Result<(), String> = read(slots.clone(), std::future::pending(), || {
            Err("fixed read failure".into())
        })
        .await;
        assert_eq!(result.err().as_deref(), Some("fixed read failure"));
        assert_eq!(slots.available_permits(), 1);
    }
}
