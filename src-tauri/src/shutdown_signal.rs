//! Route Unix SIGTERM through Tauri's existing exit/cleanup lifecycle.
use tokio::sync::watch;

pub(crate) struct ShutdownSignalGuard {
    cancel: Option<watch::Sender<bool>>,
}
impl Drop for ShutdownSignalGuard {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            let _ = cancel.send(true);
        }
    }
}

pub(crate) fn install(app: tauri::AppHandle) -> std::io::Result<ShutdownSignalGuard> {
    #[cfg(unix)]
    {
        // Register synchronously inside the Tokio context before setup exposes
        // IPC/helper admission. Spawning registration itself would leave a gap.
        let mut signal = tauri::async_runtime::block_on(async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        })?;
        let (cancel, stop) = watch::channel(false);
        tauri::async_runtime::spawn(handoff_once(
            async move { signal.recv().await.is_some() },
            stop,
            move || app.exit(0),
        ));
        Ok(ShutdownSignalGuard {
            cancel: Some(cancel),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = app;
        Ok(ShutdownSignalGuard { cancel: None })
    }
}

#[cfg(unix)]
pub(crate) async fn handoff_once(
    termination: impl std::future::Future<Output = bool>,
    mut stop: watch::Receiver<bool>,
    request_exit: impl FnOnce(),
) -> bool {
    let cancelled = async {
        loop {
            if *stop.borrow() {
                break;
            }
            if stop.changed().await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        biased;
        _ = cancelled => false,
        received = termination => {
            if received { request_exit(); }
            received
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    #[tokio::test]
    async fn cancellation_and_closed_owner_do_not_request_exit_or_hang() {
        for closed in [false, true] {
            let (tx, rx) = watch::channel(false);
            if closed {
                drop(tx);
            } else {
                tx.send(true).unwrap();
            }
            let requested = Arc::new(AtomicUsize::new(0));
            let count = requested.clone();
            let result = tokio::time::timeout(
                Duration::from_millis(100),
                handoff_once(std::future::pending(), rx, move || {
                    count.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await
            .unwrap();
            assert!(!result);
            assert_eq!(requested.load(Ordering::SeqCst), 0);
        }
    }
    #[tokio::test]
    async fn signal_handoff_calls_exit_once_and_closed_signal_is_not_an_exit() {
        for received in [true, false] {
            let (_tx, rx) = watch::channel(false);
            let requested = Arc::new(AtomicUsize::new(0));
            let count = requested.clone();
            assert_eq!(
                handoff_once(async move { received }, rx, move || {
                    count.fetch_add(1, Ordering::SeqCst);
                })
                .await,
                received
            );
            assert_eq!(requested.load(Ordering::SeqCst), usize::from(received));
        }
    }
}
