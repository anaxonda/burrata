//! Per-invocation readiness gate. The upstream helper and token validation stay
//! unchanged; only managed extraction can overlap helper loading with metadata.
use super::YouTubeSupportManager;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::{Request, Response, StatusCode},
    routing::get,
    Router,
};
use tokio::sync::watch;

#[derive(Default)]
pub(super) struct Uses {
    pub count: usize,
    pub abandoned: bool,
}
pub(crate) struct ProviderInvocation {
    pub args: Vec<std::ffi::OsString>,
    usage: HelperUse,
    _broker: Option<BrokerTasks>,
}
impl Drop for ProviderInvocation {
    fn drop(&mut self) {
        // Abort admission/readiness before final helper-use cleanup can run.
        drop(self._broker.take());
    }
}
impl ProviderInvocation {
    pub(crate) fn complete(&mut self) {
        self.usage.completed = true;
        self.usage
            .manager
            .last_used
            .store(super::now_secs(), std::sync::atomic::Ordering::Relaxed);
    }
}
struct HelperUse {
    manager: YouTubeSupportManager,
    completed: bool,
}
impl Drop for HelperUse {
    fn drop(&mut self) {
        let cleanup = {
            let mut uses = self.manager.uses.lock().unwrap_or_else(|e| e.into_inner());
            uses.count = uses.count.saturating_sub(1);
            uses.abandoned |= !self.completed;
            uses.count == 0 && uses.abandoned
        };
        if cleanup {
            let manager = self.manager.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let mut helper = manager.helper.lock().await;
                    // Registration and the final kill decision share this lock, so
                    // a newly admitted caller cannot lose its helper to old cleanup.
                    let mut uses = manager.uses.lock().unwrap_or_else(|e| e.into_inner());
                    if uses.count == 0 && uses.abandoned {
                        if let Some(child) = helper.as_mut() {
                            let _ = child.start_kill();
                        }
                        *helper = None;
                        uses.abandoned = false;
                    }
                });
            }
        }
    }
}
struct BrokerTasks {
    stop: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for BrokerTasks {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        for task in &self.tasks {
            task.abort();
        }
    }
}
fn register(manager: &YouTubeSupportManager) -> HelperUse {
    manager.uses.lock().unwrap_or_else(|e| e.into_inner()).count += 1;
    HelperUse {
        manager: manager.clone(),
        completed: false,
    }
}
pub(super) async fn ordinary(
    manager: &YouTubeSupportManager,
) -> Result<ProviderInvocation, String> {
    let usage = register(manager);
    let args = manager.provider_args().await?;
    Ok(ProviderInvocation {
        args,
        usage,
        _broker: None,
    })
}
#[derive(Clone)]
struct Gate {
    ready: watch::Receiver<Option<Result<String, String>>>,
    cancel: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    token: String,
    client: reqwest::Client,
}
async fn cancelled(mut receiver: watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
async fn bounded<T>(
    gate: &Gate,
    future: impl std::future::Future<Output = T>,
) -> Result<T, StatusCode> {
    tokio::select! {biased;_=cancelled(gate.cancel.clone())=>Err(StatusCode::REQUEST_TIMEOUT),_=cancelled(gate.stop.clone())=>Err(StatusCode::REQUEST_TIMEOUT),_=tokio::time::sleep_until(gate.deadline)=>Err(StatusCode::GATEWAY_TIMEOUT),result=future=>Ok(result)}
}
async fn ready_url(gate: &Gate) -> Result<String, StatusCode> {
    let mut receiver = gate.ready.clone();
    bounded(gate, async move {
        loop {
            let current = receiver.borrow().clone();
            if let Some(result) = current {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Err("Provider readiness stopped".into());
            }
        }
    })
    .await?
    .map_err(|_| StatusCode::BAD_GATEWAY)
}
fn empty(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}
async fn forward(
    State(gate): State<Gate>,
    Path((token, kind)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    if token != gate.token {
        return empty(StatusCode::NOT_FOUND);
    }
    if request.headers().contains_key("origin")
        || request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
    {
        return empty(StatusCode::FORBIDDEN);
    }
    if !matches!(
        (request.method().as_str(), kind.as_str()),
        ("GET", "ping") | ("POST", "get_pot")
    ) {
        return empty(StatusCode::NOT_FOUND);
    }
    let upstream = match ready_url(&gate).await {
        Ok(url) => url,
        Err(status) => return empty(status),
    };
    let (parts, body) = request.into_parts();
    let body = match bounded(&gate, to_bytes(body, 256 * 1024)).await {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return empty(StatusCode::PAYLOAD_TOO_LARGE),
        Err(status) => return empty(status),
    };
    let mut outgoing = gate
        .client
        .request(parts.method, format!("{upstream}/{kind}"));
    for (key, value) in &parts.headers {
        if !matches!(
            key.as_str(),
            "host" | "connection" | "content-length" | "transfer-encoding"
        ) {
            outgoing = outgoing.header(key, value);
        }
    }
    let mut response = match bounded(&gate, outgoing.body(body).send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => return empty(StatusCode::BAD_GATEWAY),
        Err(status) => return empty(status),
    };
    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    let read = async {
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if data.len() + chunk.len() > 1024 * 1024 {
                return Err(());
            }
            data.extend_from_slice(&chunk);
        }
        Ok::<_, ()>(data)
    };
    let data = match bounded(&gate, read).await {
        Ok(Ok(data)) => data,
        Ok(Err(_)) => return empty(StatusCode::BAD_GATEWAY),
        Err(status) => return empty(status),
    };
    let mut result = Response::builder().status(status);
    if let Some(value) = content_type {
        result = result.header("content-type", value);
    }
    result.body(Body::from(data)).unwrap()
}
pub(super) async fn playback(
    manager: &YouTubeSupportManager,
    deadline: tokio::time::Instant,
    cancel: watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
) -> Result<ProviderInvocation, String> {
    manager.ensure_helper_admission()?;
    if *cancel.borrow() || deadline <= tokio::time::Instant::now() {
        return Err("Cancelled".into());
    }
    // Legacy/missing optional resources preserve serial readiness, rather than
    // exposing the stock provider's shorter ping timeout to a pending gate.
    if !manager.provider_assets_present() {
        return ordinary(manager).await;
    }
    if !manager.provider_assets_ready() {
        return Err("Managed provider bundle is incomplete".into());
    }
    let root = manager
        .plugin_search_root()
        .ok_or("Trusted provider plugin root unavailable")?;
    if !root
        .join("burrata/yt_dlp_plugins/extractor/burrata_gate.py")
        .is_file()
    {
        return ordinary(manager).await;
    }
    let usage = register(manager);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| "Could not bind provider readiness gate")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Provider gate address unavailable")?
        .port();
    let token = uuid::Uuid::new_v4().to_string();
    let (stop, stopped) = watch::channel(false);
    let (ready_tx, ready) = watch::channel(None);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not prepare provider gate client")?;
    let gate = Gate {
        ready,
        cancel: cancel.clone(),
        stop: stopped.clone(),
        deadline,
        token: token.clone(),
        client,
    };
    let manager_ready = manager.clone();
    let gate_ready = gate.clone();
    let prepare = tokio::spawn(async move {
        let started = std::time::Instant::now();
        let result = bounded(&gate_ready, manager_ready.ensure_provider())
            .await
            .map_err(|_| "Provider readiness cancelled or deadline exceeded".to_string())
            .and_then(|value| value);
        if let Some(trace) = trace {
            trace.event(
                "provider_helper_ready_end",
                "youtube",
                serde_json::json!({"durationMs":started.elapsed().as_millis(),"ok":result.is_ok()}),
            );
        }
        ready_tx.send_replace(Some(result));
    });
    let server = tokio::spawn(async move {
        let app = Router::new()
            .route("/{token}/{kind}", get(forward).post(forward))
            .with_state(gate);
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(cancelled(stopped))
            .await;
    });
    let args = vec![
        "--plugin-dirs".into(),
        root.as_os_str().to_owned(),
        "--js-runtimes".into(),
        format!(
            "node:{}",
            manager
                .node_executable
                .as_ref()
                .expect("validated provider assets")
                .display()
        )
        .into(),
        "--extractor-args".into(),
        format!("youtubepot-burratagate:base_url=http://127.0.0.1:{port}/{token}").into(),
        // Even fallback HTTP must use this lease, never the default helper port.
        "--extractor-args".into(),
        format!("youtubepot-bgutilhttp:base_url=http://127.0.0.1:{port}/{token}").into(),
        // Disable standalone script discovery without changing upstream code.
        "--extractor-args".into(),
        format!(
            "youtubepot-bgutilscript:server_home={}",
            manager
                .data_dir
                .join(format!("no-script-{token}"))
                .display()
        )
        .into(),
    ];
    Ok(ProviderInvocation {
        args,
        usage,
        _broker: Some(BrokerTasks {
            stop,
            tasks: vec![prepare, server],
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fixture(
        deadline: tokio::time::Instant,
    ) -> (
        Gate,
        watch::Sender<Option<Result<String, String>>>,
        watch::Sender<bool>,
        watch::Sender<bool>,
    ) {
        let (ready_tx, ready) = watch::channel(None);
        let (cancel_tx, cancel) = watch::channel(false);
        let (stop_tx, stop) = watch::channel(false);
        (
            Gate {
                ready,
                cancel,
                stop,
                deadline,
                token: "lease".into(),
                client: reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
            },
            ready_tx,
            cancel_tx,
            stop_tx,
        )
    }
    #[tokio::test]
    async fn readiness_never_reports_healthy_before_helper_and_uses_original_deadline() {
        let (gate, _ready, _cancel, _stop) =
            fixture(tokio::time::Instant::now() + Duration::from_millis(30));
        assert!(
            tokio::time::timeout(Duration::from_millis(5), ready_url(&gate))
                .await
                .is_err()
        );
        assert_eq!(ready_url(&gate).await, Err(StatusCode::GATEWAY_TIMEOUT));
        assert_eq!(ready_url(&gate).await, Err(StatusCode::GATEWAY_TIMEOUT));
    }
    #[tokio::test]
    async fn cancellation_and_lease_drop_interrupt_readiness() {
        for lease_drop in [false, true] {
            let (gate, _ready, cancel, stop) =
                fixture(tokio::time::Instant::now() + Duration::from_secs(20));
            if lease_drop {
                stop.send_replace(true);
            } else {
                cancel.send_replace(true);
            }
            assert_eq!(ready_url(&gate).await, Err(StatusCode::REQUEST_TIMEOUT));
        }
    }
    #[tokio::test]
    async fn forwards_exact_token_request_only_after_actual_readiness() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, mut seen) = tokio::sync::mpsc::channel(1);
        let app = Router::new().route(
            "/get_pot",
            axum::routing::post(move |request: Request<Body>| {
                let seen_tx = seen_tx.clone();
                async move {
                    let body = to_bytes(request.into_body(), 1024).await.unwrap();
                    seen_tx.send(body.to_vec()).await.unwrap();
                    (
                        [("content-type", "application/json")],
                        r#"{"poToken":"exact","expiresAt":42}"#,
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (gate, ready, _cancel, _stop) =
            fixture(tokio::time::Instant::now() + Duration::from_secs(2));
        let request = Request::builder()
            .method("POST")
            .body(Body::from(
                r#"{"contentBinding":"fixture","requestKey":"unchanged"}"#,
            ))
            .unwrap();
        let pending = tokio::spawn(forward(
            State(gate),
            Path(("lease".into(), "get_pot".into())),
            request,
        ));
        assert!(tokio::time::timeout(Duration::from_millis(15), seen.recv())
            .await
            .is_err());
        ready.send_replace(Some(Ok(format!("http://{address}"))));
        let response = pending.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            seen.recv().await.unwrap(),
            br#"{"contentBinding":"fixture","requestKey":"unchanged"}"#
        );
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            r#"{"poToken":"exact","expiresAt":42}"#
        );
        server.abort();
    }
    #[tokio::test]
    async fn cancellation_interrupts_an_inflight_token_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (entered_tx, mut entered) = tokio::sync::mpsc::channel(1);
        let app = Router::new().route(
            "/get_pot",
            axum::routing::post(move || {
                let entered_tx = entered_tx.clone();
                async move {
                    entered_tx.send(()).await.unwrap();
                    std::future::pending::<String>().await
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (gate, ready, cancel, _stop) =
            fixture(tokio::time::Instant::now() + Duration::from_secs(20));
        ready.send_replace(Some(Ok(format!("http://{address}"))));
        let request = Request::builder()
            .method("POST")
            .body(Body::from("fixture"))
            .unwrap();
        let pending = tokio::spawn(forward(
            State(gate),
            Path(("lease".into(), "get_pot".into())),
            request,
        ));
        entered.recv().await.unwrap();
        cancel.send_replace(true);
        let response = tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        server.abort();
    }

    #[tokio::test]
    async fn rejects_browser_cross_origin_and_wrong_lease_without_waiting() {
        let (gate, _ready, _cancel, _stop) =
            fixture(tokio::time::Instant::now() + Duration::from_secs(20));
        let request = Request::builder()
            .header("origin", "https://example.org")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            forward(
                State(gate.clone()),
                Path(("lease".into(), "ping".into())),
                request
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            forward(
                State(gate),
                Path(("wrong".into(), "ping".into())),
                Request::new(Body::empty())
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn abandoned_client_does_not_kill_concurrent_client_and_last_drop_cleans_worker() {
        let dir = tempfile::tempdir().unwrap();
        let manager =
            YouTubeSupportManager::new(dir.path(), dir.path().join("unused"), None, None, None);
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        *manager.helper.lock().await = Some(child);
        let abandoned = register(&manager);
        let surviving = register(&manager);
        drop(abandoned);
        tokio::task::yield_now().await;
        assert!(manager
            .helper
            .lock()
            .await
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none());
        // Hold the helper ownership lock so cleanup queues, then admit a new
        // invocation before its final decision. It must recheck admission.
        let ownership = manager.helper.lock().await;
        drop(surviving);
        let mut newly_admitted = register(&manager);
        drop(ownership);
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert!(manager
            .helper
            .lock()
            .await
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none());
        newly_admitted.completed = true;
        drop(newly_admitted);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager.helper.lock().await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
