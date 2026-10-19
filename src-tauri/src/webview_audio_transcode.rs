//! Compatibility encoding for live Ogg on WebViews without the source codec.
//! FFmpeg can read only an already registered, DNS-pinned loopback relay; this
//! service never accepts an upstream URL from an HTTP client.
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, Method, Response, StatusCode},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{watch, Semaphore},
};

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebviewAudioCapabilities {
    #[serde(default)]
    pub ogg_vorbis: bool,
    #[serde(default)]
    pub ogg_opus: bool,
}
pub(crate) fn needs_live_ogg_conversion(
    mime: Option<&str>,
    codec: Option<&str>,
    capabilities: Option<&WebviewAudioCapabilities>,
) -> bool {
    let Some(capabilities) = capabilities else {
        return false;
    };
    let mime = mime
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let codec = codec.unwrap_or_default().to_ascii_lowercase();
    if !matches!(mime.as_str(), "audio/ogg" | "application/ogg") {
        return false;
    }
    if codec.contains("opus") {
        !capabilities.ogg_opus
    } else if codec.contains("vorbis") {
        !capabilities.ogg_vorbis
    } else {
        !capabilities.ogg_opus || !capabilities.ogg_vorbis
    }
}

struct Session {
    id: String,
    token: String,
    input: String,
    ffmpeg: PathBuf,
    stop: watch::Sender<bool>,
    slot: Arc<Semaphore>,
    startup: tokio::time::Instant,
    started: AtomicBool,
    processes: Arc<AtomicUsize>,
}
#[derive(Default)]
struct Registry {
    generation: u64,
    active: Option<Arc<Session>>,
}
#[derive(Default)]
pub(crate) struct WebviewAudioTranscoder {
    registry: Mutex<Registry>,
    base_url: Mutex<Option<String>>,
    processes: Arc<AtomicUsize>,
}
impl WebviewAudioTranscoder {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub(crate) fn set_base_url(&self, base: String) {
        *self.base_url.lock().unwrap() = Some(base.trim_end_matches('/').to_owned());
    }
    pub(crate) fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/webview-radio-mp3/{id}", get(audio).head(audio))
            .with_state(self.clone())
    }
    pub(crate) fn cancel_older_than(&self, generation: u64) {
        if let Ok(mut registry) = self.registry.lock() {
            if generation > registry.generation {
                registry.generation = generation;
                if let Some(old) = registry.active.take() {
                    old.stop.send_replace(true);
                }
            }
        }
    }
    pub(crate) fn register(
        &self,
        generation: u64,
        input: String,
        ffmpeg: PathBuf,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<String, String> {
        let base = self
            .base_url
            .lock()
            .map_err(|_| "Audio compatibility transport unavailable")?
            .clone()
            .ok_or("Audio compatibility server is not running")?;
        let parsed = url::Url::parse(&input).map_err(|_| "Invalid registered audio transport")?;
        let expected = url::Url::parse(&base).map_err(|_| "Invalid audio compatibility server")?;
        if parsed.origin() != expected.origin()
            || parsed.scheme() != "http"
            || parsed.host_str() != Some("127.0.0.1")
            || !parsed.path().starts_with("/webview-audio/")
            || !parsed
                .query_pairs()
                .any(|(k, v)| k == "token" && uuid::Uuid::parse_str(&v).is_ok())
        {
            return Err("Compatibility encoding requires a registered audio relay".into());
        }
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| "Audio compatibility transport unavailable")?;
        if generation < registry.generation {
            return Err("Audio compatibility request was superseded".into());
        }
        if tokio::time::Instant::now() >= deadline.instant() {
            return Err("Audio compatibility startup deadline exceeded".into());
        }
        registry.generation = generation;
        if let Some(old) = registry.active.take() {
            old.stop.send_replace(true);
        }
        let (stop, _) = watch::channel(false);
        let session = Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            token: uuid::Uuid::new_v4().to_string(),
            input,
            ffmpeg,
            stop,
            slot: Arc::new(Semaphore::new(1)),
            startup: deadline.instant(),
            started: AtomicBool::new(false),
            processes: self.processes.clone(),
        });
        let result = format!(
            "{base}/webview-radio-mp3/{}?token={}",
            session.id, session.token
        );
        registry.active = Some(session);
        Ok(result)
    }
}
#[derive(Deserialize)]
struct Secret {
    token: String,
}
fn response(status: StatusCode, headers: &HeaderMap, body: Body) -> Response<Body> {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "audio/mpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::VARY, "Origin");
    if let Some(origin) = headers.get(header::ORIGIN) {
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    builder.body(body).unwrap()
}
async fn reap(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
}
/// Reap even when the HTTP handler is dropped before its first response byte.
struct EncoderChild {
    child: Option<tokio::process::Child>,
    processes: Arc<AtomicUsize>,
}
impl std::ops::Deref for EncoderChild {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        self.child.as_ref().expect("active encoder")
    }
}
impl std::ops::DerefMut for EncoderChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.child.as_mut().expect("active encoder")
    }
}
impl EncoderChild {
    async fn finish(&mut self) {
        if let Some(mut child) = self.child.take() {
            reap(&mut child).await;
            self.processes.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
impl Drop for EncoderChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let processes = self.processes.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    reap(&mut child).await;
                    processes.fetch_sub(1, Ordering::SeqCst);
                });
            } else {
                let _ = child.start_kill();
                processes.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}
async fn audio(
    State(service): State<Arc<WebviewAudioTranscoder>>,
    Path(id): Path<String>,
    Query(secret): Query<Secret>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    if !crate::webview_audio_transport::allowed_origin(&headers) {
        return response(StatusCode::FORBIDDEN, &HeaderMap::new(), Body::empty());
    }
    let session = service
        .registry
        .lock()
        .ok()
        .and_then(|r| r.active.clone())
        .filter(|s| {
            s.id == id && crate::webview_audio_transport::equal_token(&s.token, &secret.token)
        });
    let Some(session) = session else {
        return response(StatusCode::NOT_FOUND, &headers, Body::empty());
    };
    if *session.stop.borrow() {
        return response(StatusCode::GONE, &headers, Body::empty());
    }
    // A live compatibility stream has no byte-addressable timeline. Accept the
    // browser's initial open-ended probe, but never pretend a later seek works.
    if headers.get(header::RANGE).is_some_and(|v| v != "bytes=0-") {
        return response(StatusCode::RANGE_NOT_SATISFIABLE, &headers, Body::empty());
    }
    if method == Method::HEAD {
        return response(StatusCode::OK, &headers, Body::empty());
    }
    if !session.started.load(Ordering::SeqCst) && tokio::time::Instant::now() >= session.startup {
        return response(StatusCode::GATEWAY_TIMEOUT, &headers, Body::empty());
    }
    let Ok(permit) = session.slot.clone().try_acquire_owned() else {
        return response(StatusCode::TOO_MANY_REQUESTS, &headers, Body::empty());
    };
    let mut command = tokio::process::Command::new(&session.ffmpeg);
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-xerror",
            "-protocol_whitelist",
            "http,tcp",
            "-f",
            "ogg",
            "-i",
        ])
        .arg(&session.input)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            "2",
            "-ar",
            "44100",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "192k",
            "-write_xing",
            "0",
            "-id3v2_version",
            "0",
            "-write_id3v1",
            "0",
            "-f",
            "mp3",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return response(StatusCode::BAD_GATEWAY, &headers, Body::empty());
    };
    session.processes.fetch_add(1, Ordering::SeqCst);
    let mut child = EncoderChild {
        child: Some(child),
        processes: session.processes.clone(),
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut cancel = session.stop.subscribe();
    let initial_deadline = (tokio::time::Instant::now() + Duration::from_secs(5)).min(
        if session.started.load(Ordering::SeqCst) {
            tokio::time::Instant::now() + Duration::from_secs(5)
        } else {
            session.startup
        },
    );
    let mut first = vec![0; 32 * 1024];
    let first_read = tokio::select! {biased;
        _=crate::playback_sources::wait_for_cancellation(cancel.clone())=>None,
        result=tokio::time::timeout_at(initial_deadline,stdout.read(&mut first))=>result.ok().and_then(Result::ok),
    };
    let Some(length) = first_read.filter(|n| *n > 0) else {
        child.finish().await;
        return response(StatusCode::BAD_GATEWAY, &headers, Body::empty());
    };
    first.truncate(length);
    session.started.store(true, Ordering::SeqCst);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    let _ = tx.try_send(Ok(first));
    tokio::spawn(async move {
        let _permit = permit;
        loop {
            let mut bytes = vec![0; 32 * 1024];
            let read = tokio::select! {biased;
                _=crate::playback_sources::wait_for_cancellation(cancel.clone())=>break,
                _=tx.closed()=>break,
                result=tokio::time::timeout(Duration::from_secs(15),stdout.read(&mut bytes))=>result,
            };
            match read {
                Ok(Ok(0)) => {
                    let success = tokio::select! {biased;
                        _=crate::playback_sources::wait_for_cancellation(cancel.clone())=>false,
                        result=tokio::time::timeout(Duration::from_secs(2),child.wait())=>result.ok().and_then(Result::ok).is_some_and(|status|status.success()),
                    };
                    if !success {
                        let _ = tx.try_send(Err(std::io::Error::other(
                            "Live audio compatibility encoder failed",
                        )));
                    }
                    break;
                }
                Ok(Ok(length)) => {
                    bytes.truncate(length);
                    tokio::select! {biased;
                        _=crate::playback_sources::wait_for_cancellation(cancel.clone())=>break,
                        result=tx.send(Ok(bytes))=>if result.is_err(){break;},
                    }
                }
                _ => {
                    let _ = tx.try_send(Err(std::io::Error::other(
                        "Live audio compatibility encoding stopped",
                    )));
                    break;
                }
            }
        }
        child.finish().await;
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    });
    response(StatusCode::OK, &headers, Body::from_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Server {
        base: String,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn serve(router: Router) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Server { base, task }
    }
    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }
    async fn ogg() -> Vec<u8> {
        let mut command = tokio::process::Command::new(crate::tools::executable("", "ffmpeg"));
        crate::tools::configure_no_console_tokio(&mut command);
        let output = command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=12",
                "-c:a",
                "vorbis",
                "-strict",
                "-2",
                "-ac",
                "2",
                "-f",
                "ogg",
                "pipe:1",
            ])
            .output()
            .await
            .expect("packaged FFmpeg must generate Vorbis");
        assert!(output.status.success());
        output.stdout
    }
    async fn fixture(bytes: Vec<u8>, slow: bool) -> (Arc<WebviewAudioTranscoder>, Server, String) {
        let service = WebviewAudioTranscoder::new();
        let bytes = Arc::new(bytes);
        let router = service.router().route(
            "/webview-audio/input",
            get(move || {
                let bytes = bytes.clone();
                async move {
                    let body = if slow {
                        Body::from_stream(futures_util::stream::unfold(
                            (bytes, 0usize),
                            |(bytes, offset)| async move {
                                if offset >= bytes.len() {
                                    return None;
                                }
                                tokio::time::sleep(Duration::from_millis(150)).await;
                                let end = (offset + 4096).min(bytes.len());
                                Some((
                                    Ok::<_, std::io::Error>(bytes[offset..end].to_vec()),
                                    (bytes, end),
                                ))
                            },
                        ))
                    } else {
                        Body::from(bytes.as_ref().clone())
                    };
                    Response::builder()
                        .header(header::CONTENT_TYPE, "audio/ogg")
                        .body(body)
                        .unwrap()
                }
            }),
        );
        let server = serve(router).await;
        service.set_base_url(server.base.clone());
        let input = format!(
            "{}/webview-audio/input?token={}",
            server.base,
            uuid::Uuid::new_v4()
        );
        (service, server, input)
    }
    fn register(service: &WebviewAudioTranscoder, input: String, budget: Duration) -> String {
        service
            .register(
                1,
                input,
                crate::tools::executable("", "ffmpeg"),
                crate::playback_sources::PlaybackDeadline::after(budget),
            )
            .unwrap()
    }
    async fn idle(service: &WebviewAudioTranscoder) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while service.processes.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("conversion child must be reaped");
    }
    #[test]
    fn conversion_is_capability_gated_and_rejects_foreign_or_unregistered_transports() {
        let unsupported = WebviewAudioCapabilities::default();
        let supported = WebviewAudioCapabilities {
            ogg_opus: true,
            ogg_vorbis: true,
        };
        assert!(needs_live_ogg_conversion(
            Some("audio/ogg"),
            Some("Vorbis"),
            Some(&unsupported)
        ));
        assert!(!needs_live_ogg_conversion(
            Some("audio/ogg"),
            Some("vorbis"),
            Some(&supported)
        ));
        assert!(!needs_live_ogg_conversion(
            Some("audio/mpeg"),
            Some("mp3"),
            Some(&unsupported)
        ));
        assert!(!needs_live_ogg_conversion(
            Some("audio/ogg"),
            Some("opus"),
            None
        ));
        let service = WebviewAudioTranscoder::new();
        service.set_base_url("http://127.0.0.1:9".into());
        for input in [
            "https://radio.example/live.ogg",
            "http://127.0.0.1:8/webview-audio/id?token=bad",
            "http://127.0.0.1:9/other?token=bad",
            "http://127.0.0.1:9/webview-audio/id?token=bad",
        ] {
            assert!(service
                .register(
                    1,
                    input.into(),
                    PathBuf::new(),
                    crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(1))
                )
                .is_err());
        }
    }
    #[tokio::test]
    async fn head_bad_auth_origin_and_seeks_never_start_child() {
        let (service, _server, input) = fixture(Vec::new(), false).await;
        let url = register(&service, input, Duration::from_secs(10));
        assert_eq!(client().head(&url).send().await.unwrap().status(), 200);
        assert_eq!(
            client()
                .get(format!("{url}bad"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(
            client()
                .get(&url)
                .header(header::ORIGIN, "https://evil.example")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client()
                .get(&url)
                .header(header::RANGE, "bytes=12-")
                .send()
                .await
                .unwrap()
                .status(),
            416
        );
        assert_eq!(service.processes.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn real_vorbis_converts_to_decodable_mp3_and_reconnect_outlives_startup() {
        let (service, _server, input) = fixture(ogg().await, false).await;
        let url = register(&service, input, Duration::from_secs(5));
        let response = client()
            .get(&url)
            .header(header::ORIGIN, "tauri://localhost")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/mpeg");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "tauri://localhost"
        );
        let encoded = response.bytes().await.unwrap();
        assert!(encoded.len() > 100000);
        idle(&service).await;
        let mut command = tokio::process::Command::new(crate::tools::executable("", "ffmpeg"));
        crate::tools::configure_no_console_tokio(&mut command);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "mp3",
                "-i",
                "pipe:0",
                "-ar",
                "8000",
                "-ac",
                "1",
                "-f",
                "s16le",
                "pipe:1",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(&encoded).await.unwrap();
        });
        let decoded = child.wait_with_output().await.unwrap();
        writer.await.unwrap();
        assert!(
            decoded.status.success(),
            "{}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        assert!(decoded.stdout.len() > 11 * 8000 * 2);
        // A successful session may reconnect after its original startup budget.
        let session = service.registry.lock().unwrap().active.clone().unwrap();
        tokio::time::sleep_until(session.startup + Duration::from_millis(10)).await;
        let response = client().get(&url).send().await.unwrap();
        assert_eq!(response.status(), 200);
        drop(response);
        idle(&service).await;
    }
    #[tokio::test]
    async fn disconnect_and_generation_cancel_reap_active_conversion() {
        let (service, _server, input) = fixture(ogg().await, true).await;
        let url = register(&service, input.clone(), Duration::from_secs(5));
        let response = client().get(&url).send().await.unwrap();
        assert_eq!(response.status(), 200);
        drop(response);
        idle(&service).await;
        let url = register(&service, input, Duration::from_secs(5));
        let response = client().get(&url).send().await.unwrap();
        assert_eq!(response.status(), 200);
        service.cancel_older_than(2);
        let _ = response.bytes().await;
        idle(&service).await;
        assert_eq!(client().get(&url).send().await.unwrap().status(), 404);
    }
    #[tokio::test]
    async fn expired_initial_budget_fails_bounded_without_persistent_child() {
        let (service, _server, input) = fixture(Vec::new(), true).await;
        let url = register(&service, input, Duration::from_millis(20));
        tokio::time::sleep(Duration::from_millis(30)).await;
        let response = tokio::time::timeout(Duration::from_secs(2), client().get(url).send())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 504);
        idle(&service).await;
    }
}
