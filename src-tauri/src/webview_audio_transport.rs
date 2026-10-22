//! Opaque, generation-owned transport for already verified WebView audio.
//! No endpoint accepts an upstream URL. The only upstreams are registered by
//! trusted resolver code; DNS is checked and pinned once per registration.
#[path = "webview_hls_transport.rs"]
mod hls;
use crate::{acquisition, native_output::AudioSource, playback_sources::PlaybackDeadline};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode},
    routing::get,
    Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use tokio::sync::watch;

static NEXT_RANGE_TRACE_ID: AtomicU64 = AtomicU64::new(1);

fn normalized_request_range(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "none".into();
    };
    if !valid_range(value) {
        return "invalid".into();
    }
    let Some(range) = value.strip_prefix("bytes=") else {
        return "invalid".into();
    };
    let Some((start, end)) = range.split_once('-') else {
        return "invalid".into();
    };
    match (start.parse::<u64>().ok(), end.parse::<u64>().ok()) {
        (Some(start), Some(end)) => format!("{start}-{end}"),
        (Some(start), None) => format!("{start}-"),
        (None, Some(end)) => format!("-{end}"),
        _ => "invalid".into(),
    }
}

fn normalized_content_range(value: Option<&HeaderValue>) -> String {
    let Some(value) = value.and_then(|value| value.to_str().ok()) else {
        return "none".into();
    };
    let Some(value) = value.strip_prefix("bytes ") else {
        return "invalid".into();
    };
    let Some((range, total)) = value.split_once('/') else {
        return "invalid".into();
    };
    let total = match total.parse::<u64>() {
        Ok(total) => total.to_string(),
        Err(_) => return "invalid".into(),
    };
    if let Some((start, end)) = range.split_once('-') {
        if let (Ok(start), Ok(end)) = (start.parse::<u64>(), end.parse::<u64>()) {
            if start <= end {
                return format!("{start}-{end}/{total}");
            }
        }
    }
    if range == "*" {
        return format!("*/{total}");
    }
    "invalid".into()
}

fn normalized_content_length(value: Option<&HeaderValue>) -> String {
    let Some(value) = value.and_then(|value| value.to_str().ok()) else {
        return "none".into();
    };
    value
        .parse::<u64>()
        .map(|length| length.to_string())
        .unwrap_or_else(|_| "invalid".into())
}

struct RangeTrace {
    id: u64,
    generation: u64,
    range: String,
    started: tokio::time::Instant,
    bytes: u64,
    first_byte_ms: Option<u128>,
    finished: bool,
}

impl RangeTrace {
    fn begin(session: &Session, method: &Method, range: Option<&HeaderValue>) -> Option<Self> {
        if std::env::var_os("BURRATA_TRACE_AUDIO_RANGES").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return None;
        }
        let id = NEXT_RANGE_TRACE_ID.fetch_add(1, Ordering::Relaxed);
        let range = normalized_request_range(range.and_then(|value| value.to_str().ok()));
        eprintln!(
            "[audio-range] id={id} generation={} phase=request method={} range={range}",
            session.generation,
            method.as_str()
        );
        Some(Self {
            id,
            generation: session.generation,
            range,
            started: tokio::time::Instant::now(),
            bytes: 0,
            first_byte_ms: None,
            finished: false,
        })
    }

    fn headers(&self, status: StatusCode, headers: &HeaderMap) {
        eprintln!("[audio-range] id={} generation={} phase=response status={} range={} status_range={} content_range={} content_length={} elapsed_ms={}",
            self.id, self.generation, status.as_u16(), self.range,
            if self.range == "none" { "not_requested" } else { "requested" },
            normalized_content_range(headers.get(header::CONTENT_RANGE)),
            normalized_content_length(headers.get(header::CONTENT_LENGTH)), self.started.elapsed().as_millis());
    }

    fn add_bytes(&mut self, length: usize) {
        self.bytes = self.bytes.saturating_add(length as u64);
        if self.first_byte_ms.is_none() {
            self.first_byte_ms = Some(self.started.elapsed().as_millis());
        }
    }

    fn finish(&mut self, outcome: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        eprintln!("[audio-range] id={} generation={} phase=body_end outcome={} bytes={} first_byte_ms={} elapsed_ms={}",
            self.id, self.generation, outcome, self.bytes,
            self.first_byte_ms.map_or_else(|| "none".into(), |value| value.to_string()), self.started.elapsed().as_millis());
    }
}

impl Drop for RangeTrace {
    fn drop(&mut self) {
        self.finish("dropped");
    }
}

struct Session {
    id: String,
    token: String,
    generation: u64,
    _track_id: String,
    source: url::Url,
    headers: reqwest::header::HeaderMap,
    client: reqwest::Client,
    stop: watch::Sender<bool>,
    slots: Arc<tokio::sync::Semaphore>,
    hls: Mutex<hls::Graph>,
    allow_radio_http: bool,
}
#[derive(Default)]
struct Registry {
    generation: u64,
    epoch: u64,
    active: Option<Arc<Session>>,
}
#[derive(Default)]
pub(crate) struct WebviewAudioTransport {
    registry: Mutex<Registry>,
    // Native CENC relay sessions have independent ownership from the single
    // WebView playback session. A decoder/preload lease controls their lifetime.
    native_sessions: Mutex<HashMap<String, Arc<Session>>>,
    base_url: Mutex<Option<String>>,
}

struct NativeLeaseInner {
    id: String,
    session: Arc<Session>,
    owner: Weak<WebviewAudioTransport>,
    loopback_url: String,
}

impl Drop for NativeLeaseInner {
    fn drop(&mut self) {
        self.session.stop.send_replace(true);
        if let Some(owner) = self.owner.upgrade() {
            if let Ok(mut sessions) = owner.native_sessions.lock() {
                sessions.remove(&self.id);
            }
        }
    }
}

/// Opaque capability for one pinned native CENC relay session.
/// The URL is intentionally only available inside the crate and Debug never
/// reveals its random authorization token.
#[derive(Clone)]
pub(crate) struct RegisteredNativeAudioLease(Arc<NativeLeaseInner>);

impl RegisteredNativeAudioLease {
    pub(crate) fn loopback_url(&self) -> &str {
        &self.0.loopback_url
    }

    pub(crate) fn matches_input(&self, input: &str) -> bool {
        self.0.loopback_url == input
    }
}

impl std::fmt::Debug for RegisteredNativeAudioLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RegisteredNativeAudioLease([REDACTED])")
    }
}
impl WebviewAudioTransport {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub(crate) fn set_base_url(&self, url: String) {
        *self.base_url.lock().unwrap() = Some(url.trim_end_matches('/').into());
    }
    fn commit_native_session(
        self: &Arc<Self>,
        session: Arc<Session>,
    ) -> Result<RegisteredNativeAudioLease, String> {
        let base = self
            .base_url
            .lock()
            .map_err(|_| "Audio transport unavailable")?
            .clone()
            .ok_or("Audio transport server is not running")?;
        let loopback_url = format!(
            "{base}/webview-audio/{}?token={}",
            session.id, session.token
        );
        let mut sessions = self
            .native_sessions
            .lock()
            .map_err(|_| "Audio transport unavailable")?;
        if sessions.len() >= 4 {
            return Err("Native audio relay capacity is full".into());
        }
        sessions.insert(session.id.clone(), session.clone());
        Ok(RegisteredNativeAudioLease(Arc::new(NativeLeaseInner {
            id: session.id.clone(),
            session,
            owner: Arc::downgrade(self),
            loopback_url,
        })))
    }
    pub(crate) fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/webview-audio/{id}", get(audio).head(audio))
            .route(
                "/webview-audio/{id}/{resource}",
                get(hls::audio_child).head(hls::audio_child),
            )
            .with_state(self.clone())
    }
    pub(crate) fn begin(&self, generation: u64) -> Result<u64, String> {
        let mut state = self
            .registry
            .lock()
            .map_err(|_| "Audio transport unavailable")?;
        if generation < state.generation {
            return Err("Audio transport was superseded".into());
        }
        state.generation = generation;
        state.epoch += 1;
        if let Some(old) = state.active.take() {
            old.stop.send_replace(true);
        }
        Ok(state.epoch)
    }
    pub(crate) fn cancel_older_than(&self, generation: u64) {
        if let Ok(mut state) = self.registry.lock() {
            if generation > state.generation {
                state.generation = generation;
                state.epoch += 1;
                if let Some(old) = state.active.take() {
                    old.stop.send_replace(true);
                }
            }
        }
    }
    pub(crate) async fn register(
        &self,
        generation: u64,
        track_id: &str,
        source: &AudioSource,
        allow_saved_radio_http: bool,
        deadline: PlaybackDeadline,
        cancel: watch::Receiver<bool>,
    ) -> Result<String, String> {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let epoch = self.begin(generation)?;
        let setup = async {
            let url = validated_url(&source.input, allow_saved_radio_http)?;
            let mut addresses = acquisition::public_addresses(&url).await?;
            for address in &mut addresses {
                address.set_port(
                    url.port_or_known_default()
                        .ok_or("Audio transport port is missing")?,
                );
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .read_timeout(Duration::from_secs(15))
                .resolve_to_addrs(
                    url.host_str().ok_or("Audio transport hostname missing")?,
                    &addresses,
                )
                .build()
                .map_err(|_| "Audio transport client unavailable")?;
            self.commit(epoch, generation, track_id, url, source, client)
        };
        tokio::select! {biased;
            _=crate::playback_sources::wait_for_cancellation(cancel)=>Err("Cancelled".into()),
            _=tokio::time::sleep_until(deadline.instant())=>Err("Audio transport exceeded startup deadline".into()),
            result=setup=>result,
        }
    }

    /// Register a public HTTPS CENC source without replacing the WebView
    /// transport. The returned lease is the sole lifetime capability for the
    /// loopback input; resolver credentials are deliberately not accepted.
    pub(crate) async fn register_native_cenc_input(
        self: &Arc<Self>,
        generation: u64,
        track_id: &str,
        remote_input: &str,
        source_headers: &[(String, String)],
        deadline: PlaybackDeadline,
        cancel: watch::Receiver<bool>,
    ) -> Result<RegisteredNativeAudioLease, String> {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        for (name, _) in source_headers {
            if ["authorization", "proxy-authorization", "cookie"]
                .iter()
                .any(|blocked| name.eq_ignore_ascii_case(blocked))
            {
                return Err("Native encrypted media cannot use credential headers".into());
            }
        }
        let setup = async {
            let url = validated_url(remote_input, false)?;
            if url.scheme() != "https" {
                return Err("Native encrypted media requires HTTPS".to_owned());
            }
            let mut addresses = acquisition::public_addresses(&url).await?;
            for address in &mut addresses {
                address.set_port(
                    url.port_or_known_default()
                        .ok_or("Audio transport port is missing")?,
                );
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .read_timeout(Duration::from_secs(15))
                .resolve_to_addrs(
                    url.host_str().ok_or("Audio transport hostname missing")?,
                    &addresses,
                )
                .build()
                .map_err(|_| "Audio transport client unavailable")?;
            let mut headers = reqwest::header::HeaderMap::new();
            for (name, value) in source_headers {
                if ["user-agent", "referer", "origin", "accept-language"]
                    .iter()
                    .any(|allowed| name.eq_ignore_ascii_case(allowed))
                {
                    headers.insert(
                        reqwest::header::HeaderName::from_bytes(name.as_bytes())
                            .map_err(|_| "Invalid audio header")?,
                        HeaderValue::from_str(value).map_err(|_| "Invalid audio header")?,
                    );
                }
            }
            headers.insert(
                reqwest::header::ACCEPT_ENCODING,
                HeaderValue::from_static("identity"),
            );
            let (stop, _) = watch::channel(false);
            let session = Arc::new(Session {
                id: uuid::Uuid::new_v4().to_string(),
                token: uuid::Uuid::new_v4().to_string(),
                generation,
                _track_id: track_id.into(),
                source: url,
                headers,
                client,
                stop,
                slots: Arc::new(tokio::sync::Semaphore::new(4)),
                hls: Mutex::new(hls::Graph::default()),
                allow_radio_http: false,
            });
            self.commit_native_session(session)
        };
        tokio::select! {biased;
            _=crate::playback_sources::wait_for_cancellation(cancel)=>Err("Cancelled".into()),
            _=tokio::time::sleep_until(deadline.instant())=>Err("Audio transport exceeded startup deadline".into()),
            result=setup=>result,
        }
    }
    fn commit(
        &self,
        epoch: u64,
        generation: u64,
        track_id: &str,
        url: url::Url,
        source: &AudioSource,
        client: reqwest::Client,
    ) -> Result<String, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &source.headers {
            // Resolver-owned headers only; never accept request framing/routing.
            if [
                "user-agent",
                "referer",
                "origin",
                "accept-language",
                "authorization",
                "cookie",
            ]
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))
            {
                headers.insert(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| "Invalid audio header")?,
                    HeaderValue::from_str(value).map_err(|_| "Invalid audio header")?,
                );
            }
        }
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        let (stop, _) = watch::channel(false);
        let session = Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            token: uuid::Uuid::new_v4().to_string(),
            generation,
            _track_id: track_id.into(),
            source: url.clone(),
            headers,
            client,
            stop,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
            hls: Mutex::new(hls::Graph::default()),
            allow_radio_http: url.scheme() == "http",
        });
        let base = self
            .base_url
            .lock()
            .map_err(|_| "Audio transport unavailable")?
            .clone()
            .ok_or("Audio transport server is not running")?;
        let result = format!(
            "{base}/webview-audio/{}?token={}",
            session.id, session.token
        );
        let mut state = self
            .registry
            .lock()
            .map_err(|_| "Audio transport unavailable")?;
        if state.generation != generation || state.epoch != epoch {
            return Err("Audio transport was superseded".into());
        }
        state.active = Some(session);
        Ok(result)
    }
}
fn validated_url(raw: &str, allow_radio_http: bool) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| "Invalid audio transport URL")?;
    if allow_radio_http && url.scheme() == "http" {
        let mut secure = url.clone();
        secure
            .set_scheme("https")
            .map_err(|_| "Invalid radio scheme")?;
        secure.set_port(None).map_err(|_| "Invalid radio port")?;
        acquisition::validate_source_url(secure.as_str(), "direct")?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("Audio transport credentials are forbidden".into());
        }
        Ok(url)
    } else {
        acquisition::validate_source_url(raw, "direct")
    }
}
pub(crate) fn is_hls(source: &AudioSource) -> bool {
    source
        .decoder_hint
        .as_ref()
        .and_then(|hint| hint.mime_type.as_deref())
        .is_some_and(|mime| mime.to_ascii_lowercase().contains("mpegurl"))
        || source
            .input
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .ends_with(".m3u8")
}
pub(crate) fn needs_relay(source: &AudioSource) -> bool {
    is_hls(source) || source.input.starts_with("http://") || !source.headers.is_empty()
}
pub(crate) fn allowed_origin(headers: &HeaderMap) -> bool {
    matches!(
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        None | Some("tauri://localhost")
            | Some("http://tauri.localhost")
            | Some("https://tauri.localhost")
            | Some("http://localhost:1420")
            | Some("http://127.0.0.1:1420")
    )
}
pub(crate) fn equal_token(a: &str, b: &str) -> bool {
    let mut difference = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        difference |=
            usize::from(*a.as_bytes().get(i).unwrap_or(&0) ^ *b.as_bytes().get(i).unwrap_or(&0));
    }
    difference == 0
}
fn valid_range(value: &str) -> bool {
    let Some(range) = value.strip_prefix("bytes=") else {
        return false;
    };
    if range.contains(',') {
        return false;
    }
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    match (start.parse::<u64>().ok(), end.parse::<u64>().ok()) {
        (Some(start), Some(end)) => start <= end,
        (Some(_), None) => end.is_empty(),
        (None, Some(end)) => start.is_empty() && end > 0,
        _ => false,
    }
}
#[derive(Deserialize)]
struct Auth {
    token: String,
}
async fn audio(
    State(manager): State<Arc<WebviewAudioTransport>>,
    Path(id): Path<String>,
    Query(auth): Query<Auth>,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    hls::serve(manager, id, None, auth, headers, method).await
}
async fn direct_audio(
    manager: Arc<WebviewAudioTransport>,
    id: String,
    resource: Option<String>,
    auth: Auth,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    let missing = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()
    };
    if !allowed_origin(&headers) {
        return missing();
    }
    let session = manager
        .registry
        .lock()
        .ok()
        .and_then(|state| {
            state
                .active
                .as_ref()
                .filter(|session| {
                    session.id == id
                        && session.generation == state.generation
                        && equal_token(&session.token, &auth.token)
                })
                .cloned()
        })
        .or_else(|| {
            manager.native_sessions.lock().ok().and_then(|sessions| {
                sessions
                    .get(&id)
                    .filter(|session| equal_token(&session.token, &auth.token))
                    .cloned()
            })
        });
    let Some(session) = session else {
        return missing();
    };
    let mut range_trace = RangeTrace::begin(&session, &method, headers.get(header::RANGE));
    let Ok(permit) = session.slots.clone().try_acquire_owned() else {
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("concurrency_limit");
        }
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .body(Body::empty())
            .unwrap();
    };
    let mut stop = session.stop.subscribe();
    if *stop.borrow() {
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("revoked_before_request");
        }
        return missing();
    }
    let (target, depth) = match resource.as_deref() {
        None => (session.source.clone(), 0),
        Some(resource) => match session
            .hls
            .lock()
            .ok()
            .and_then(|mut graph| graph.lookup(resource))
        {
            Some(value) => value,
            None => return missing(),
        },
    };
    let client = if target.origin() == session.source.origin() {
        session.client.clone()
    } else {
        match tokio::select! { biased; _ = stop.changed() => return missing(), result = tokio::time::timeout(Duration::from_secs(5), hls::public_client(&target)) => result }
        {
            Ok(Ok(client)) => client,
            _ => return hls::error("HLS resource has an inaccessible address"),
        }
    };
    let mut upstream_headers = session.headers.clone();
    if target.origin() != session.source.origin() {
        for name in [
            header::AUTHORIZATION,
            header::COOKIE,
            header::ORIGIN,
            header::REFERER,
        ] {
            upstream_headers.remove(name);
        }
    }
    // Fetch a manifest body even for HEAD so relative references are validated.
    let mut request = client
        .request(
            if method == Method::HEAD && !target.path().ends_with(".m3u8") {
                Method::HEAD
            } else {
                Method::GET
            },
            target.clone(),
        )
        .headers(upstream_headers);
    if let Some(range) = headers.get(header::RANGE) {
        let Ok(value) = range.to_str() else {
            return missing();
        };
        if !valid_range(value) {
            if let Some(trace) = range_trace.as_mut() {
                trace.finish("invalid_request_range");
            }
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .body(Body::empty())
                .unwrap();
        }
        request = request.header(header::RANGE, range);
    }
    let response = tokio::select! {
        biased;
        _ = stop.changed() => {
            if let Some(trace) = range_trace.as_mut() { trace.finish("revoked_before_headers"); }
            return missing();
        },
        result = tokio::time::timeout(Duration::from_secs(5), request.send()) => match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                if let Some(trace) = range_trace.as_mut() { trace.finish("request_error"); }
                return Response::builder().status(StatusCode::BAD_GATEWAY).body(Body::empty()).unwrap();
            }
            Err(_) => {
                if let Some(trace) = range_trace.as_mut() { trace.finish("request_timeout"); }
                return Response::builder().status(StatusCode::BAD_GATEWAY).body(Body::empty()).unwrap();
            }
        }
    };
    if let Some(trace) = range_trace.as_ref() {
        trace.headers(response.status(), response.headers());
    }
    if hls::is_manifest(&target, response.headers()) {
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("manifest_response");
        }
        return hls::manifest_response(
            &manager, &session, &target, depth, response, stop, headers, method,
        )
        .await;
    }
    if response.status().is_success()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|mime| {
                let mime = mime
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                !(mime.starts_with("audio/")
                    || matches!(
                        mime.as_str(),
                        "video/mp4"
                            | "video/mp2t"
                            | "video/webm"
                            | "application/octet-stream"
                            | "binary/octet-stream"
                            | "application/ogg"
                    ))
            })
    {
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("response_mime_rejected");
        }
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::empty())
            .unwrap();
    }
    if !matches!(response.status().as_u16(), 200 | 206 | 416) {
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("upstream_status_rejected");
        }
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::empty())
            .unwrap();
    }
    let mut builder = Response::builder().status(response.status());
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(value) = response.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    builder = builder
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(origin) = headers.get(header::ORIGIN) {
        builder = builder
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
            .header(header::VARY, "Origin")
            .header(
                header::ACCESS_CONTROL_EXPOSE_HEADERS,
                "Content-Range, Accept-Ranges, Content-Length",
            );
    }
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        // Preserve the range total without serving an upstream HTML error page
        // under the authenticated local server's origin.
        let mut response = builder.body(Body::empty()).unwrap();
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("range_not_satisfiable");
        }
        return response;
    }
    if method == Method::HEAD {
        if let Some(trace) = range_trace.as_mut() {
            trace.finish("head_complete");
        }
        return builder.body(Body::empty()).unwrap();
    }
    let stream = futures_util::stream::unfold(
        (response.bytes_stream(), stop, permit, range_trace),
        |(mut upstream, mut stop, permit, mut range_trace)| async move {
            if *stop.borrow() {
                if let Some(trace) = range_trace.as_mut() {
                    trace.finish("revoked_during_body");
                }
                return None;
            }
            tokio::select! {
                biased;
                _ = stop.changed() => {
                    if let Some(trace) = range_trace.as_mut() { trace.finish("revoked_during_body"); }
                    None
                },
                chunk = upstream.next() => match chunk {
                    Some(Ok(bytes)) => {
                        if let Some(trace) = range_trace.as_mut() { trace.add_bytes(bytes.len()); }
                        Some((Ok(bytes), (upstream, stop, permit, range_trace)))
                    }
                    Some(Err(_)) => {
                        if let Some(trace) = range_trace.as_mut() { trace.finish("body_error"); }
                        Some((Err(std::io::Error::other("Audio transport interrupted")), (upstream, stop, permit, range_trace)))
                    }
                    None => {
                        if let Some(trace) = range_trace.as_mut() { trace.finish("body_eof"); }
                        None
                    }
                }
            }
        },
    );
    builder.body(Body::from_stream(stream)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Server {
        base: String,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn server(router: Router) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Server { base, task }
    }
    async fn upstream(
        State(calls): State<Arc<AtomicUsize>>,
        headers: HeaderMap,
        method: Method,
    ) -> Response<Body> {
        calls.fetch_add(1, Ordering::SeqCst);
        if headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            != Some("fixture-agent")
            || headers.get(header::REFERER).and_then(|v| v.to_str().ok())
                != Some("https://catalog.example/")
            || headers.contains_key(header::AUTHORIZATION)
            || headers.contains_key(header::COOKIE)
        {
            return Response::builder().status(403).body(Body::empty()).unwrap();
        }
        let bytes = b"0123456789abcdef";
        let mut response = Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "audio/mpeg")
            .header(header::ACCEPT_RANGES, "bytes");
        let (start, end) =
            if let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
                let (start, end) = range
                    .strip_prefix("bytes=")
                    .unwrap()
                    .split_once('-')
                    .unwrap();
                let start = start.parse::<usize>().unwrap();
                let end = if end.is_empty() {
                    15
                } else {
                    end.parse::<usize>().unwrap()
                };
                response = response
                    .status(206)
                    .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/16"));
                (start, end)
            } else {
                (0, 15)
            };
        response
            .header(header::CONTENT_LENGTH, end - start + 1)
            .body(if method == Method::HEAD {
                Body::empty()
            } else {
                Body::from(bytes[start..=end].to_vec())
            })
            .unwrap()
    }
    fn source(url: String) -> AudioSource {
        AudioSource {
            input: url,
            ffmpeg: std::path::PathBuf::new(),
            headers: vec![
                ("User-Agent".into(), "fixture-agent".into()),
                ("Referer".into(), "https://catalog.example/".into()),
            ],
            decoder_hint: None,
            cenc: None,
        }
    }
    fn fixture_register(
        manager: &WebviewAudioTransport,
        generation: u64,
        source: &AudioSource,
    ) -> String {
        // Only this test helper accepts literal loopback upstreams. Production
        // register always validates public hosts and pins checked DNS addresses.
        let epoch = manager.begin(generation).unwrap();
        manager
            .commit(
                epoch,
                generation,
                "fixture",
                url::Url::parse(&source.input).unwrap(),
                source,
                reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
            )
            .unwrap()
    }
    fn fixture_native_lease(
        manager: &Arc<WebviewAudioTransport>,
        upstream: &Server,
        path: &str,
    ) -> RegisteredNativeAudioLease {
        manager
            .commit_native_session(fixture_native_session(upstream, path))
            .unwrap()
    }
    fn fixture_native_session(upstream: &Server, path: &str) -> Arc<Session> {
        let url = url::Url::parse(&format!("{}{path}", upstream.base)).unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("fixture-agent"),
        );
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://catalog.example/"),
        );
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        let (stop, _) = watch::channel(false);
        let session = Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            token: uuid::Uuid::new_v4().to_string(),
            generation: 1,
            _track_id: "fixture-native".into(),
            source: url,
            headers,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            stop,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
            hls: Mutex::new(hls::Graph::default()),
            allow_radio_http: false,
        });
        session
    }
    async fn fixture() -> (
        Arc<WebviewAudioTransport>,
        Server,
        Server,
        Arc<AtomicUsize>,
        String,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let upstream = server(
            Router::new()
                .route("/required", get(self::upstream).head(self::upstream))
                .with_state(calls.clone()),
        )
        .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let url = fixture_register(&manager, 1, &source(format!("{}/required", upstream.base)));
        (manager, upstream, relay, calls, url)
    }
    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }
    #[tokio::test]
    async fn exact_headers_ranges_and_head_are_forwarded_with_cors() {
        let (_manager, _upstream, _relay, calls, url) = fixture().await;
        let response = client()
            .get(&url)
            .header(header::ORIGIN, "tauri://localhost")
            .header(header::RANGE, "bytes=3-6")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 206);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 3-6/16");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "tauri://localhost"
        );
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"3456");
        let response = client().head(&url).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "16");
        assert!(response.bytes().await.unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for origin in ["http://localhost:1420", "http://127.0.0.1:1420"] {
            let response = client()
                .head(&url)
                .header(header::ORIGIN, origin)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                origin
            );
        }
    }

    #[tokio::test]
    async fn native_lease_relays_ranges_without_replacing_webview_session() {
        let (manager, _upstream, relay, calls, webview_url) = fixture().await;
        let native = fixture_native_lease(&manager, &_upstream, "/required");
        assert!(native.matches_input(native.loopback_url()));
        let range = client()
            .get(native.loopback_url())
            .header(header::RANGE, "bytes=4-7")
            .send()
            .await
            .unwrap();
        assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(range.headers()[header::CONTENT_RANGE], "bytes 4-7/16");
        assert_eq!(range.bytes().await.unwrap().as_ref(), b"4567");
        let head = client().head(native.loopback_url()).send().await.unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "16");
        assert!(head.bytes().await.unwrap().is_empty());

        // Registering the independent native input must leave the single
        // WebView source playable and its generation ownership unchanged.
        let webview = client().get(webview_url).send().await.unwrap();
        assert_eq!(webview.status(), StatusCode::OK);
        assert_eq!(webview.bytes().await.unwrap().as_ref(), b"0123456789abcdef");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn native_lease_never_follows_upstream_redirects() {
        let upstream = server(
            Router::new()
                .route(
                    "/redirect",
                    get(|| async {
                        Response::builder()
                            .status(StatusCode::FOUND)
                            .header(header::LOCATION, "/private")
                            .body(Body::empty())
                            .unwrap()
                    }),
                )
                .route(
                    "/private",
                    get(|| async {
                        Response::builder()
                            .header(header::CONTENT_TYPE, "audio/mpeg")
                            .body(Body::from("must not be reached"))
                            .unwrap()
                    }),
                ),
        )
        .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let lease = fixture_native_lease(&manager, &upstream, "/redirect");
        let response = client().get(lease.loopback_url()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn native_lease_drop_revokes_registered_route() {
        let calls = Arc::new(AtomicUsize::new(0));
        let upstream = server(
            Router::new()
                .route("/required", get(self::upstream).head(self::upstream))
                .with_state(calls),
        )
        .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let lease = fixture_native_lease(&manager, &upstream, "/required");
        let route = lease.loopback_url().to_owned();
        assert_eq!(
            client().get(&route).send().await.unwrap().status(),
            StatusCode::OK
        );
        drop(lease);
        assert_eq!(
            client().get(&route).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert!(manager.native_sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropping_native_lease_interrupts_an_inflight_body() {
        let upstream = server(Router::new().route(
            "/slow",
            get(|| async {
                let chunks = futures_util::stream::once(async {
                    Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"first"))
                })
                .chain(futures_util::stream::pending::<
                    Result<axum::body::Bytes, std::io::Error>,
                >());
                Response::builder()
                    .header(header::CONTENT_TYPE, "audio/mpeg")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }),
        ))
        .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let lease = fixture_native_lease(&manager, &upstream, "/slow");
        let response = client().get(lease.loopback_url()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::spawn(async move { response.bytes().await });
        drop(lease);
        assert!(tokio::time::timeout(Duration::from_secs(1), body)
            .await
            .is_ok());
        assert!(manager.native_sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn native_registry_is_bounded_and_releases_capacity_on_final_drop() {
        let upstream = server(Router::new().route("/required", get(|| async { "ok" }))).await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let mut leases = (0..4)
            .map(|_| {
                manager
                    .commit_native_session(fixture_native_session(&upstream, "/required"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(manager.native_sessions.lock().unwrap().len(), 4);
        assert!(manager
            .commit_native_session(fixture_native_session(&upstream, "/required"))
            .unwrap_err()
            .contains("capacity"));
        drop(leases.pop());
        let replacement = manager
            .commit_native_session(fixture_native_session(&upstream, "/required"))
            .unwrap();
        assert_eq!(manager.native_sessions.lock().unwrap().len(), 4);
        drop(replacement);
    }

    #[tokio::test]
    async fn native_registration_rejects_http_and_credentials_before_network_access() {
        let manager = WebviewAudioTransport::new();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let deadline = PlaybackDeadline::after(Duration::from_secs(2));
        let http = manager
            .register_native_cenc_input(
                1,
                "fixture",
                "http://127.0.0.1:1/media.m4a",
                &[],
                deadline,
                cancel_rx.clone(),
            )
            .await;
        assert!(http.unwrap_err().contains("HTTPS"));
        let private_address = manager
            .register_native_cenc_input(
                1,
                "fixture",
                "https://127.0.0.1/media.m4a",
                &[],
                deadline,
                cancel_rx.clone(),
            )
            .await;
        assert!(private_address.is_err());
        let credentials = manager
            .register_native_cenc_input(
                1,
                "fixture",
                "https://media.example/media.m4a",
                &[("Authorization".into(), "Bearer secret".into())],
                deadline,
                cancel_rx,
            )
            .await;
        assert!(credentials.unwrap_err().contains("credential headers"));
        assert!(manager.native_sessions.lock().unwrap().is_empty());
    }
    #[test]
    fn range_trace_normalizes_only_numeric_ranges_and_lengths() {
        assert_eq!(normalized_request_range(Some("bytes=12-34")), "12-34");
        assert_eq!(normalized_request_range(Some("bytes=12-")), "12-");
        assert_eq!(normalized_request_range(Some("bytes=-34")), "-34");
        assert_eq!(
            normalized_request_range(Some("bytes=12-34\r\nAuthorization: secret")),
            "invalid"
        );
        assert_eq!(normalized_request_range(Some("Bearer secret")), "invalid");
        assert_eq!(
            normalized_content_range(Some(&HeaderValue::from_static("bytes 10-19/200"))),
            "10-19/200"
        );
        assert_eq!(
            normalized_content_range(Some(&HeaderValue::from_static("bytes */200"))),
            "*/200"
        );
        assert_eq!(
            normalized_content_range(Some(&HeaderValue::from_static("Bearer secret"))),
            "invalid"
        );
        assert_eq!(
            normalized_content_length(Some(&HeaderValue::from_static("1234"))),
            "1234"
        );
        assert_eq!(
            normalized_content_length(Some(&HeaderValue::from_static("Bearer secret"))),
            "invalid"
        );
    }

    #[tokio::test]
    async fn range_passthrough_currently_accepts_ignored_and_mismatched_upstream_ranges() {
        let upstream = server(
            Router::new()
                .route(
                    "/ignore",
                    get(|| async {
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Body::from("0123456789"))
                            .unwrap()
                    }),
                )
                .route(
                    "/mismatch",
                    get(|| async {
                        Response::builder()
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header(header::CONTENT_RANGE, "bytes 0-3/10")
                            .header(header::CONTENT_LENGTH, "4")
                            .body(Body::from("0123"))
                            .unwrap()
                    }),
                ),
        )
        .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());

        let ignored = fixture_register(&manager, 1, &source(format!("{}/ignore", upstream.base)));
        let response = client()
            .get(ignored)
            .header(header::RANGE, "bytes=4-7")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"0123456789");

        let mismatched =
            fixture_register(&manager, 2, &source(format!("{}/mismatch", upstream.base)));
        let response = client()
            .get(mismatched)
            .header(header::RANGE, "bytes=4-7")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 0-3/10");
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"0123");
    }

    #[tokio::test]
    async fn header_required_relay_delivers_real_aac_that_decodes_without_device() {
        use rodio::Source;
        let media = server(Router::new().route(
            "/aac",
            get(|headers: HeaderMap| async move {
                if headers
                    .get(header::USER_AGENT)
                    .and_then(|value| value.to_str().ok())
                    != Some("fixture-agent")
                    || headers
                        .get(header::REFERER)
                        .and_then(|value| value.to_str().ok())
                        != Some("https://catalog.example/")
                {
                    return Response::builder().status(403).body(Body::empty()).unwrap();
                }
                let bytes = include_bytes!("../tests/fixtures/aac-startup-fixture.m4a");
                Response::builder()
                    .header(header::CONTENT_TYPE, "audio/mp4")
                    .header(header::CONTENT_LENGTH, bytes.len())
                    .body(Body::from(bytes.as_slice()))
                    .unwrap()
            }),
        ))
        .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let url = fixture_register(&manager, 1, &source(format!("{}/aac", media.base)));
        let response = client()
            .get(url)
            .header(header::ORIGIN, "tauri://localhost")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let received = response.bytes().await.unwrap();
        assert_eq!(
            received.as_ref(),
            include_bytes!("../tests/fixtures/aac-startup-fixture.m4a")
        );
        let decoder = rodio::Decoder::new(std::io::Cursor::new(received)).unwrap();
        let rate = decoder.sample_rate().get();
        let channels = decoder.channels().get();
        let duration = decoder.count() as f64 / (rate as f64 * channels as f64);
        assert!(
            (3.9..4.1).contains(&duration),
            "relayed generated AAC must decode four seconds of PCM"
        );
    }

    #[tokio::test]
    async fn invalid_origin_token_and_revoked_session_never_reach_upstream() {
        let (manager, _upstream, _relay, calls, url) = fixture().await;
        assert_eq!(
            client()
                .get(&url)
                .header(header::ORIGIN, "https://evil.example")
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(
            client()
                .get(format!("{url}-wrong"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(
            client()
                .get(&url)
                .header(header::ORIGIN, "null")
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        manager.begin(2).unwrap();
        assert_eq!(client().get(&url).send().await.unwrap().status(), 404);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn malformed_multipart_ranges_and_redirects_are_rejected() {
        let (manager, _upstream, _relay, calls, url) = fixture().await;
        assert_eq!(
            client()
                .get(url)
                .header(header::RANGE, "bytes=0-1,3-4")
                .send()
                .await
                .unwrap()
                .status(),
            416
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let redirect = server(Router::new().route(
            "/redirect",
            get(|| async {
                Response::builder()
                    .status(302)
                    .header(header::LOCATION, "http://127.0.0.1/private")
                    .body(Body::empty())
                    .unwrap()
            }),
        ))
        .await;
        let url = fixture_register(&manager, 2, &source(format!("{}/redirect", redirect.base)));
        assert_eq!(client().get(url).send().await.unwrap().status(), 502);
        let non_audio = server(
            Router::new()
                .route(
                    "/html",
                    get(|| async {
                        Response::builder()
                            .header(header::CONTENT_TYPE, "text/html")
                            .body(Body::from("not audio"))
                            .unwrap()
                    }),
                )
                .route(
                    "/hls",
                    get(|| async {
                        Response::builder()
                            .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
                            .body(Body::from("#EXTM3U"))
                            .unwrap()
                    }),
                ),
        )
        .await;
        for (generation, path) in [(3, "html"), (4, "hls")] {
            let url = fixture_register(
                &manager,
                generation,
                &source(format!("{}/{path}", non_audio.base)),
            );
            assert_eq!(client().get(url).send().await.unwrap().status(), 502);
        }
    }
    #[tokio::test]
    async fn superseded_headers_and_streaming_body_are_cancelled() {
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let upstream = server(
            Router::new()
                .route(
                    "/headers",
                    get(|| async { std::future::pending::<Response<Body>>().await }),
                )
                .route(
                    "/body",
                    get(|| async {
                        Response::builder()
                            .header(header::CONTENT_LENGTH, "16")
                            .body(Body::from_stream(futures_util::stream::pending::<
                                Result<axum::body::Bytes, std::io::Error>,
                            >()))
                            .unwrap()
                    }),
                ),
        )
        .await;
        let url = fixture_register(&manager, 1, &source(format!("{}/headers", upstream.base)));
        let pending = tokio::spawn(async move { client().get(url).send().await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        manager.begin(2).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.status(), 404);
        let url = fixture_register(&manager, 3, &source(format!("{}/body", upstream.base)));
        let response = client().get(url).send().await.unwrap();
        let mut stream = response.bytes_stream();
        manager.begin(4).unwrap();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("revocation terminates blocked body");
        assert!(chunk.is_none() || chunk.unwrap().is_err());
    }
    #[test]
    fn url_validation_and_registration_epoch_fail_closed() {
        for raw in [
            "http://127.0.0.1/stream",
            "http://localhost/stream",
            "file:///tmp/audio",
            "https://user:password@public.example/audio",
        ] {
            assert!(validated_url(raw, true).is_err());
        }
        assert!(validated_url("http://radio.example:8000/live", false).is_err());
        assert!(validated_url("http://radio.example:8000/live", true).is_ok());
        let manager = WebviewAudioTransport::new();
        manager.set_base_url("http://127.0.0.1:1".into());
        let epoch = manager.begin(1).unwrap();
        manager.begin(2).unwrap();
        let registered_source = source("https://cdn.example/audio".into());
        assert!(manager
            .commit(
                epoch,
                1,
                "old",
                url::Url::parse(&registered_source.input).unwrap(),
                &registered_source,
                client()
            )
            .is_err());
        assert!(needs_relay(&source("https://cdn.example/live.m3u8".into())));
    }
    #[tokio::test]
    async fn hls_nested_graph_ranges_and_revocation_work_without_upstream_cors() {
        let media = server(Router::new()
            .route("/master.m3u8", get(|| async { ([(header::CONTENT_TYPE,"application/vnd.apple.mpegurl")], "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nnested/media.m3u8\n") }))
            .route("/nested/media.m3u8", get(|| async { ([(header::CONTENT_TYPE,"application/vnd.apple.mpegurl")], "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4,\n../required\n#EXT-X-ENDLIST\n") }))
            .route("/required", get(self::upstream).head(self::upstream))
            .with_state(Arc::new(AtomicUsize::new(0))))
            .await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let root = fixture_register(&manager, 1, &source(format!("{}/master.m3u8", media.base)));
        let master = client()
            .get(&root)
            .header(header::ORIGIN, "tauri://localhost")
            .send()
            .await
            .unwrap();
        assert_eq!(master.status(), 200);
        assert_eq!(
            master.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "tauri://localhost"
        );
        let master = master.text().await.unwrap();
        let nested = master
            .lines()
            .find(|line| line.starts_with("http"))
            .unwrap();
        assert!(!nested.contains(&media.base));
        let playlist = client().get(nested).send().await.unwrap();
        assert_eq!(playlist.status(), 200);
        let playlist = playlist.text().await.unwrap();
        let segment = playlist
            .lines()
            .find(|line| line.starts_with("http"))
            .unwrap();
        let bytes = client()
            .get(segment)
            .header(header::RANGE, "bytes=3-6")
            .send()
            .await
            .unwrap();
        assert_eq!(bytes.status(), 206);
        assert_eq!(bytes.bytes().await.unwrap().as_ref(), b"3456");
        let head = client().head(segment).send().await.unwrap();
        assert_eq!(head.status(), 200);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "16");
        manager.begin(2).unwrap();
        assert_eq!(client().get(segment).send().await.unwrap().status(), 404);
    }
    #[tokio::test]
    async fn finite_hls_relay_decodes_and_seeks_exact_audio_fixture() {
        // The packaged tool is required for this transport regression, just as
        // release smoke requires it; no sound device or public network is used.
        let tool = crate::tools::executable("", "ffmpeg");
        let encoded = tokio::process::Command::new(&tool)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-c:a",
                "aac",
                "-b:a",
                "64k",
                "-f",
                "adts",
                "pipe:1",
            ])
            .output()
            .await
            .expect("bundled FFmpeg must run");
        assert!(encoded.status.success());
        let audio = Arc::new(encoded.stdout);
        let media=server(Router::new()
            .route("/live.m3u8",get(||async{([(header::CONTENT_TYPE,"application/vnd.apple.mpegurl")],"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4,\naudio.aac\n#EXT-X-ENDLIST\n")}))
            .route("/audio.aac",get(move ||{let audio=audio.clone();async move{Response::builder().header(header::CONTENT_TYPE,"audio/aac").header(header::CONTENT_LENGTH,audio.len()).body(Body::from(audio.as_ref().clone())).unwrap()}}))).await;
        let manager = WebviewAudioTransport::new();
        let relay = server(manager.router()).await;
        manager.set_base_url(relay.base.clone());
        let root = fixture_register(&manager, 1, &source(format!("{}/live.m3u8", media.base)));
        for seek in ["0", "2"] {
            let mut command = tokio::process::Command::new(&tool);
            crate::tools::configure_no_console_tokio(&mut command);
            command.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                seek,
                "-i",
                &root,
                "-t",
                "1",
                "-vn",
                "-ar",
                "8000",
                "-ac",
                "1",
                "-f",
                "s16le",
                "pipe:1",
            ]);
            let decoded = tokio::time::timeout(Duration::from_secs(10), command.output())
                .await
                .expect("HLS decode must be bounded")
                .unwrap();
            assert!(
                decoded.status.success(),
                "{}",
                String::from_utf8_lossy(&decoded.stderr)
            );
            assert!(
                decoded.stdout.len() >= 14000,
                "finite HLS must decode one second after seek={seek}"
            );
        }
    }
}
