//! Process-memory Spotify token cache coordinator prototype.
//! The refresh closure must use a dedicated private worker IPC path and must not
//! return/log provider diagnostics containing credentials.
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

const EXPIRY_SKEW: Duration = Duration::from_secs(30);
const REFRESH_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthScope {
    pub generation: u64,
    pub account_id: String,
    pub credential_fingerprint: [u8; 32],
}

#[derive(Clone)]
pub(crate) struct AuthEntry {
    pub scope: AuthScope,
    pub access_token: SecretToken,
    pub expires_at: Instant,
    pub expires_at_unix_ms: u64,
    pub cacheable: bool,
}

#[derive(Clone)]
pub(crate) struct SecretToken(Arc<SecretBytes>);
struct SecretBytes(Vec<u8>);
impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
impl SecretToken {
    pub fn new(value: String) -> Result<Self, String> {
        if value.is_empty()
            || value.len() > 16 * 1024
            || value.bytes().any(|b| b.is_ascii_control())
        {
            return Err("Spotify returned an invalid access token".into());
        }
        Ok(Self(Arc::new(SecretBytes(value.into_bytes()))))
    }
    pub fn expose(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.0 .0)
    }
    fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl std::fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretToken([redacted])")
    }
}

struct RefreshFlight {
    scope: AuthScope,
    epoch: u64,
    result: watch::Sender<Option<Result<AuthEntry, String>>>,
    cancelled: Arc<AtomicBool>,
}
#[derive(Default)]
struct State {
    entry: Option<AuthEntry>,
    flight: Option<Arc<RefreshFlight>>,
}
#[derive(Clone, Default)]
pub(crate) struct SpotifyAuthCache {
    state: Arc<std::sync::Mutex<State>>,
    epoch: Arc<AtomicU64>,
}

impl SpotifyAuthCache {
    /// Returns a scoped cached token or joins/starts one refresh. `refresh` is
    /// detached from individual waiters; only `stop` or the bounded deadline
    /// can terminate it. Caller cancellation only abandons that caller's wait.
    pub async fn get_or_refresh<F, Fut>(
        &self,
        scope: AuthScope,
        caller_cancel: Option<Arc<AtomicBool>>,
        stop: Arc<AtomicBool>,
        refresh: F,
    ) -> Result<AuthEntry, String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(SecretToken, Instant, u64, bool), String>> + Send + 'static,
    {
        if caller_cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
            || stop.load(Ordering::SeqCst)
        {
            return Err("Provider request was cancelled".into());
        }
        let mut requested_epoch = self.epoch.load(Ordering::SeqCst);
        let flight = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Spotify token cache is unavailable".to_owned())?;
            if self.epoch.load(Ordering::SeqCst) != requested_epoch {
                return Err("Provider request was cancelled".into());
            }
            if state
                .entry
                .as_ref()
                .is_some_and(|entry| entry.scope != scope)
                || state
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.scope != scope)
            {
                self.invalidate_state(&mut state);
                requested_epoch = self.epoch.load(Ordering::SeqCst);
            }
            if let Some(entry) = state.entry.as_ref() {
                if entry.scope == scope && entry.expires_at > Instant::now() + EXPIRY_SKEW {
                    if self.epoch.load(Ordering::SeqCst) != requested_epoch
                        || caller_cancel
                            .as_ref()
                            .is_some_and(|flag| flag.load(Ordering::SeqCst))
                        || stop.load(Ordering::SeqCst)
                    {
                        return Err("Provider request was cancelled".into());
                    }
                    return Ok(entry.clone());
                }
            }
            if state
                .entry
                .as_ref()
                .is_some_and(|entry| entry.scope == scope)
            {
                state.entry = None;
            }
            if let Some(flight) = state.flight.as_ref().filter(|flight| flight.scope == scope) {
                flight.clone()
            } else {
                let (result_tx, _result_rx) = watch::channel(None);
                let cancelled = Arc::new(AtomicBool::new(false));
                let flight = Arc::new(RefreshFlight {
                    scope: scope.clone(),
                    epoch: requested_epoch,
                    result: result_tx,
                    cancelled: cancelled.clone(),
                });
                state.flight = Some(flight.clone());
                let cache = self.clone();
                let refresh_flight = flight.clone();
                let stop_for_select = stop.clone();
                let flight_cancelled = cancelled.clone();
                tokio::spawn(async move {
                    let refreshed = tokio::select! {
                        biased;
                        _ = wait_flag(stop_for_select.clone()) => Err("Provider request was cancelled".to_owned()),
                        _ = wait_flag(flight_cancelled.clone()) => Err("Provider request was cancelled".to_owned()),
                        result = tokio::time::timeout(REFRESH_DEADLINE, refresh()) => match result {
                            Ok(value) => value,
                            Err(_) => Err("Spotify token refresh timed out".to_owned()),
                        },
                    }.and_then(|(token, expires_at, expires_at_unix_ms, cacheable)| {
                        if expires_at <= Instant::now() + EXPIRY_SKEW {
                            Err("Spotify returned an expired access token".to_owned())
                        } else {
                            Ok(AuthEntry { scope: refresh_flight.scope.clone(), access_token: token, expires_at, expires_at_unix_ms, cacheable })
                        }
                    });
                    {
                        let Ok(mut state) = cache.state.lock() else {
                            refresh_flight.result.send_replace(Some(Err(
                                "Spotify token cache is unavailable".into(),
                            )));
                            return;
                        };
                        // A later scope may have replaced this flight. Never publish an
                        // old generation/account/cookie refresh into the active cache.
                        if state
                            .flight
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &refresh_flight))
                            && cache.epoch.load(Ordering::SeqCst) == refresh_flight.epoch
                        {
                            if let Ok(entry) = refreshed.as_ref() {
                                if entry.cacheable {
                                    state.entry = Some(entry.clone());
                                }
                            }
                            state.flight = None;
                        }
                    }
                    refresh_flight.result.send_replace(Some(refreshed));
                });
                flight
            }
        };
        let mut result_rx = flight.result.subscribe();
        loop {
            if caller_cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
                || stop.load(Ordering::SeqCst)
                || self.epoch.load(Ordering::SeqCst) != flight.epoch
            {
                return Err("Provider request was cancelled".into());
            }
            if let Some(result) = result_rx.borrow().clone() {
                if caller_cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                    || stop.load(Ordering::SeqCst)
                    || self.epoch.load(Ordering::SeqCst) != flight.epoch
                {
                    return Err("Provider request was cancelled".into());
                }
                return result;
            }
            tokio::select! {
                biased;
                _ = wait_optional_flag(caller_cancel.clone()) => return Err("Provider request was cancelled".into()),
                _ = wait_flag(stop.clone()) => return Err("Provider request was cancelled".into()),
                changed = result_rx.changed() => if changed.is_err() { return Err("Spotify token refresh failed".into()); },
            }
        }
    }

    fn invalidate_state(&self, state: &mut State) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        state.entry = None;
        if let Some(flight) = state.flight.take() {
            flight.cancelled.store(true, Ordering::SeqCst);
            flight
                .result
                .send_replace(Some(Err("Provider request was cancelled".into())));
        }
    }

    /// Invalidate only the exact cached bearer used by the failing request. A
    /// late 401 from an older request must not evict a newer successful refresh.
    pub fn invalidate_if_current(&self, used: &AuthEntry) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let matches = state.entry.as_ref().is_some_and(|current| {
            current.scope == used.scope && current.access_token.same_identity(&used.access_token)
        });
        if matches {
            state.entry = None;
        }
        matches
    }

    pub fn invalidate_now(&self) {
        if let Ok(mut state) = self.state.lock() {
            self.invalidate_state(&mut state);
        } else {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub async fn invalidate(&self) {
        self.invalidate_now();
    }
}

async fn wait_flag(flag: Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
async fn wait_optional_flag(flag: Option<Arc<AtomicBool>>) {
    if let Some(flag) = flag {
        wait_flag(flag).await
    } else {
        std::future::pending().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::oneshot;

    fn scope(account: &str, generation: u64, fingerprint: u8) -> AuthScope {
        AuthScope {
            account_id: account.into(),
            generation,
            credential_fingerprint: [fingerprint; 32],
        }
    }
    fn token() -> SecretToken {
        SecretToken::new("fixture-secret-token".into()).unwrap()
    }

    #[tokio::test]
    async fn same_scope_uses_cache_and_scope_changes_refresh() {
        let cache = SpotifyAuthCache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let first_calls = calls.clone();
        let first = cache
            .get_or_refresh(
                scope("acct", 1, 1),
                None,
                stop.clone(),
                move || async move {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    Ok((
                        token(),
                        Instant::now() + Duration::from_secs(120),
                        1_900_000_000_000,
                        true,
                    ))
                },
            )
            .await
            .unwrap();
        let second_calls = calls.clone();
        let second = cache
            .get_or_refresh(
                scope("acct", 1, 1),
                None,
                stop.clone(),
                move || async move {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    Ok((
                        token(),
                        Instant::now() + Duration::from_secs(120),
                        1_900_000_000_000,
                        true,
                    ))
                },
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            first.access_token.expose().unwrap(),
            second.access_token.expose().unwrap()
        );
        let changed = calls.clone();
        cache
            .get_or_refresh(scope("acct", 2, 1), None, stop, move || async move {
                changed.fetch_add(1, Ordering::SeqCst);
                Ok((
                    token(),
                    Instant::now() + Duration::from_secs(120),
                    1_900_000_000_000,
                    true,
                ))
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_waiters_share_refresh_and_one_cancellation_is_local() {
        let cache = SpotifyAuthCache::default();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let c1 = cache.clone();
        let s1 = stop.clone();
        let x1 = cancelled.clone();
        let n1 = calls.clone();
        let waiter1 = tokio::spawn(async move {
            c1.get_or_refresh(scope("acct", 4, 7), Some(x1), s1, move || async move {
                n1.fetch_add(1, Ordering::SeqCst);
                let _ = release_rx.await;
                Ok((
                    token(),
                    Instant::now() + Duration::from_secs(120),
                    1_900_000_000_000,
                    true,
                ))
            })
            .await
        });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let c2 = cache.clone();
        let s2 = stop.clone();
        let n2 = calls.clone();
        let waiter2 = tokio::spawn(async move {
            c2.get_or_refresh(scope("acct", 4, 7), None, s2, move || async move {
                n2.fetch_add(1, Ordering::SeqCst);
                Ok((
                    token(),
                    Instant::now() + Duration::from_secs(120),
                    1_900_000_000_000,
                    true,
                ))
            })
            .await
        });
        tokio::task::yield_now().await;
        cancelled.store(true, Ordering::SeqCst);
        assert!(waiter1.await.unwrap().is_err());
        let _ = release_tx.send(());
        assert!(waiter2.await.unwrap().is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stop_cancels_shared_refresh_and_wakes_waiter() {
        let cache = SpotifyAuthCache::default();
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let task_cache = cache.clone();
        let task = tokio::spawn(async move {
            task_cache
                .get_or_refresh(scope("acct", 1, 1), None, task_stop, || async {
                    std::future::pending::<Result<(SecretToken, Instant, u64, bool), String>>()
                        .await
                })
                .await
        });
        tokio::task::yield_now().await;
        stop.store(true, Ordering::SeqCst);
        assert!(tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }

    #[tokio::test]
    async fn ready_cache_does_not_return_to_cancelled_waiter() {
        let cache = SpotifyAuthCache::default();
        let stop = Arc::new(AtomicBool::new(false));
        cache
            .get_or_refresh(scope("acct", 1, 1), None, stop.clone(), || async {
                Ok((
                    token(),
                    Instant::now() + Duration::from_secs(120),
                    1_900_000_000_000,
                    true,
                ))
            })
            .await
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(true));
        let result = cache
            .get_or_refresh(scope("acct", 1, 1), Some(cancelled), stop, || async {
                panic!("cached token should avoid refresh")
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn late_old_token_rejection_does_not_evict_newer_entry() {
        let cache = SpotifyAuthCache::default();
        let stop = Arc::new(AtomicBool::new(false));
        let old = cache
            .get_or_refresh(scope("acct", 5, 8), None, stop.clone(), || async {
                Ok((
                    token(),
                    Instant::now() + Duration::from_secs(120),
                    1_900_000_000_000,
                    true,
                ))
            })
            .await
            .unwrap();
        cache.invalidate_now();
        let fresh = cache
            .get_or_refresh(scope("acct", 5, 8), None, stop, || async {
                Ok((
                    token(),
                    Instant::now() + Duration::from_secs(120),
                    1_900_000_000_000,
                    true,
                ))
            })
            .await
            .unwrap();
        assert!(!cache.invalidate_if_current(&old));
        assert!(cache
            .state
            .lock()
            .unwrap()
            .entry
            .as_ref()
            .is_some_and(|entry| entry.access_token.same_identity(&fresh.access_token)));
    }

    #[tokio::test]
    async fn invalidation_rejects_old_flight_for_waiters_and_cache() {
        let cache = SpotifyAuthCache::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let started = Arc::new(AtomicBool::new(false));
        let task_cache = cache.clone();
        let task_stop = stop.clone();
        let task_started = started.clone();
        let waiter = tokio::spawn(async move {
            task_cache
                .get_or_refresh(scope("acct", 9, 3), None, task_stop, move || async move {
                    task_started.store(true, Ordering::SeqCst);
                    let _ = release_rx.await;
                    Ok((
                        token(),
                        Instant::now() + Duration::from_secs(120),
                        1_900_000_000_000,
                        true,
                    ))
                })
                .await
        });
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        cache.invalidate_now();
        assert!(waiter.await.unwrap().is_err());
        let _ = release_tx.send(());
        tokio::task::yield_now().await;
        assert!(cache.state.lock().unwrap().entry.is_none());
    }
}
