//! Offline paired check for finite Opus/WebM routing. No device or live service.
use crate::native_output::{
    direct_pcm_experiment::collect_until_with_opus_control, AudioSource, AudioSourceDecoderHint,
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::watch;

struct RangeServer {
    address: std::net::SocketAddr,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RangeServer {
    fn start(bytes: Arc<Vec<u8>>, stall_from: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            let mut connections = Vec::new();
            while !thread_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                let bytes = bytes.clone();
                let requests = thread_requests.clone();
                let stop = thread_stop.clone();
                connections.push(thread::spawn(move || {
                    // The production access validator issues head and tail range
                    // requests concurrently; service each independently so the
                    // fixture does not rely on runner-specific socket scheduling.
                    // Some platforms inherit the listener's nonblocking mode on
                    // accepted sockets; read_headers expects a blocking stream.
                    stream
                        .set_nonblocking(false)
                        .expect("configure accepted fixture socket as blocking");
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                    let Some(request) = read_headers(&mut stream) else {
                        return;
                    };
                    let index = requests.fetch_add(1, Ordering::Relaxed) + 1;
                    if stall_from.is_some_and(|threshold| index >= threshold) {
                        while !stop.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(5));
                        }
                        return;
                    }
                    let range = request
                        .lines()
                        // HTTP field names are case-insensitive. Do not depend on
                        // reqwest/hyper choosing the same wire casing on every OS.
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("range").then_some(value)
                        })
                        .and_then(|line| line.trim().strip_prefix("bytes="))
                        .and_then(|value| value.split_once('-'))
                        .and_then(|(start, end)| {
                            Some((
                                start.parse::<usize>().ok()?,
                                if end.is_empty() {
                                    bytes.len() - 1
                                } else {
                                    end.parse::<usize>().ok()?
                                },
                            ))
                        });
                    let (start, end, status) = if let Some((start, end)) = range {
                        if start >= bytes.len() || end < start {
                            let _ = stream.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            return;
                        }
                        (start, end.min(bytes.len() - 1), "206 Partial Content")
                    } else {
                        (0, bytes.len() - 1, "200 OK")
                    };
                    let payload = &bytes[start..=end];
                    let headers = if status.starts_with("206") {
                        format!("HTTP/1.1 {status}\r\nContent-Type: audio/webm\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n", payload.len(), bytes.len())
                    } else {
                        format!("HTTP/1.1 {status}\r\nContent-Type: audio/webm\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len())
                    };
                    if stream.write_all(headers.as_bytes()).is_ok() {
                        let _ = stream.write_all(payload);
                    }
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                }));
            }
            for connection in connections {
                let _ = connection.join();
            }
        });
        Self {
            address,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/fixture.webm", self.address)
    }
}

impl Drop for RangeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_headers(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") && bytes.len() < 16 * 1024 {
        let count = stream.read(&mut buffer).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes).ok()
}

#[test]
fn range_server_handles_fragmented_case_insensitive_concurrent_requests() {
    let bytes = Arc::new(vec![b'x'; 4096]);
    let server = RangeServer::start(bytes, None);
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let clients = ["rAnGe", "RANGE"].map(|field| {
        let address = server.address;
        let barrier = barrier.clone();
        thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            barrier.wait();
            let request = format!("{field}: bytes=0-1\r\nConnection: close\r\n");
            stream
                .write_all(b"GET /fixture.webm HTTP/1.1\r\nHost: localhost\r\n")
                .unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            stream.write_all(b"\r\n").unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).unwrap();
            String::from_utf8(response).unwrap()
        })
    });
    barrier.wait();
    for client in clients {
        let response = client.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(response.contains("Content-Range: bytes 0-1/4096\r\n"));
        assert!(response.ends_with("\r\n\r\nxx"));
    }
    assert_eq!(server.requests.load(Ordering::Relaxed), 2);
}

struct Fixture {
    webm_path: PathBuf,
    ffmpeg: PathBuf,
    bytes: Arc<Vec<u8>>,
}

impl Fixture {
    fn create() -> Self {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let bundled_ffmpeg = directory
            .join("resources/tools")
            .join(crate::tools::target_triple())
            .join(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            });
        // Unit CI installs FFmpeg on PATH; packaged-resource staging is optional.
        let ffmpeg = if bundled_ffmpeg.is_file() {
            bundled_ffmpeg
        } else {
            PathBuf::from(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            })
        };
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let source = std::env::temp_dir().join(format!(
            "burrata-opus-source-{}-{nonce}.ogg",
            std::process::id()
        ));
        let webm_path = std::env::temp_dir().join(format!(
            "burrata-opus-paired-{}-{nonce}.webm",
            std::process::id()
        ));
        std::fs::write(
            &source,
            include_bytes!("../tests/fixtures/opus-fixture.ogg"),
        )
        .unwrap();
        let output = Command::new(&ffmpeg)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&source)
            .args(["-map", "0:a:0", "-c:a", "copy", "-f", "webm"])
            .arg(&webm_path)
            .output()
            .expect("run bundled FFmpeg remux");
        let _ = std::fs::remove_file(source);
        assert!(output.status.success(), "WebM fixture remux failed");
        let bytes = Arc::new(std::fs::read(&webm_path).unwrap());
        assert!(bytes.len() > 1024);
        Self {
            webm_path,
            ffmpeg,
            bytes,
        }
    }

    fn source(&self, url: String) -> AudioSource {
        AudioSource {
            input: url,
            ffmpeg: self.ffmpeg.clone(),
            headers: Vec::new(),
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some("audio/webm".into()),
                codec: Some("opus".into()),
                duration_seconds: Some(4.0),
                content_length: Some(self.bytes.len() as u64),
                has_video: false,
            }),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.webm_path);
    }
}

async fn verify_local_access(url: &str, length: u64, cancel: &mut watch::Receiver<bool>) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = url::Url::parse(url).unwrap();
    crate::acquisition::diagnostic_verify_preview_audio_access(&client, &url, length, cancel)
        .await
        .expect("production range validator accepts local Opus fixture");
}

#[tokio::test]
async fn finite_opus_webm_paired_route_counts_same_pcm_and_seek_is_bounded() {
    let fixture = Fixture::create();
    let server = RangeServer::start(fixture.bytes.clone(), None);
    let shared_url = server.url();
    let mut reports = Vec::new();
    let mut request_deltas = Vec::new();
    for direct in [false, true] {
        let requests_before = server.requests.load(Ordering::Relaxed);
        let (stop, mut cancel) = watch::channel(false);
        verify_local_access(&shared_url, fixture.bytes.len() as u64, &mut cancel).await;
        assert!(server.requests.load(Ordering::Relaxed) >= 1);
        let deadline = Instant::now() + Duration::from_secs(8);
        let source = fixture.source(shared_url.clone());
        let report = collect_until_with_opus_control(
            source,
            2.0,
            1.0,
            deadline,
            cancel.clone(),
            !direct,
            direct,
        )
        .await;
        assert!(report.ok, "{report:?}");
        assert!(report.decoded_frames >= 48_000);
        assert!(report.decoded_frames <= 60_000);
        assert!(report.pcm_rms.is_some_and(|rms| rms > 0.01));
        let diagnostics = serde_json::to_value(report.decoder.as_ref()).unwrap();
        if direct {
            assert!(
                diagnostics["failedNativeAttempt"].is_null(),
                "{diagnostics}"
            );
        } else {
            assert!(
                !diagnostics["failedNativeAttempt"].is_null(),
                "{diagnostics}"
            );
            let failed_attempt = &diagnostics["failedNativeAttempt"];
            assert!(
                diagnostics["firstDecodedBlockMs"].as_u64().unwrap()
                    >= failed_attempt["durationMs"].as_u64().unwrap()
            );
            assert!(failed_attempt["readerBytesRead"].as_u64().unwrap() > 0);
        }
        reports.push(report);
        request_deltas.push(server.requests.load(Ordering::Relaxed) - requests_before);
        stop.send(false).unwrap();
    }
    assert_eq!(reports[0].decoded_frames, reports[1].decoded_frames);
    let rms_a = reports[0].pcm_rms.unwrap();
    let rms_b = reports[1].pcm_rms.unwrap();
    assert!(
        (rms_a - rms_b).abs() <= 0.002,
        "legacy={rms_a} direct={rms_b}"
    );
    eprintln!(
        "{}",
        serde_json::json!({
            "experiment":"offline-opus-webm-route-pair-v1",
            "sameFixtureAndUrl":true,
            "legacyRequestCount":request_deltas[0],
            "directRequestCount":request_deltas[1],
            "legacyPcmFrames":reports[0].decoded_frames,
            "directPcmFrames":reports[1].decoded_frames,
            "legacyPcmRms":rms_a,
            "directPcmRms":rms_b,
            "firstPcmMs":[reports[0].first_pcm_ms,reports[1].first_pcm_ms],
            "qualification":false
        })
    );

    let server = RangeServer::start(fixture.bytes.clone(), None);
    let (_stop, mut cancel) = watch::channel(false);
    let url = server.url();
    verify_local_access(&url, fixture.bytes.len() as u64, &mut cancel).await;
    let short = collect_until_with_opus_control(
        fixture.source(url),
        3.5,
        1.0,
        Instant::now() + Duration::from_secs(8),
        cancel,
        false,
        true,
    )
    .await;
    assert!(!short.ok);
    assert_eq!(short.failure, Some("SHORT_PCM"));
    assert!((20_000..=30_000).contains(&short.decoded_frames));
}

#[tokio::test]
async fn loopback_direct_opus_decode_cancels_during_stalled_range_read() {
    let fixture = Fixture::create();
    let server = RangeServer::start(fixture.bytes.clone(), Some(2));
    let (stop, mut cancel) = watch::channel(false);
    let url = server.url();
    verify_local_access(&url, fixture.bytes.len() as u64, &mut cancel).await;
    let pending = tokio::spawn(collect_until_with_opus_control(
        fixture.source(url),
        0.0,
        2.0,
        Instant::now() + Duration::from_secs(8),
        cancel.clone(),
        false,
        true,
    ));
    let requests = server.requests.clone();
    let cancel_task = tokio::spawn(async move {
        for _ in 0..200 {
            if server.requests.load(Ordering::Relaxed) >= 2 {
                stop.send(true).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        stop.send(true).unwrap();
    });
    let report = pending.await.unwrap();
    cancel_task.await.unwrap();
    assert!(
        requests.load(Ordering::Relaxed) >= 2,
        "cancel must follow a stalled decoder range request"
    );
    assert_eq!(report.failure, Some("CANCELLED"), "{report:?}");
}

#[tokio::test]
async fn missing_codec_hint_cannot_force_direct_route_and_still_decodes_pcm() {
    let fixture = Fixture::create();
    let server = RangeServer::start(fixture.bytes.clone(), None);
    let (_stop, mut cancel) = watch::channel(false);
    let url = server.url();
    verify_local_access(&url, fixture.bytes.len() as u64, &mut cancel).await;
    let mut source = fixture.source(url);
    source.decoder_hint.as_mut().unwrap().codec = None;
    let report = collect_until_with_opus_control(
        source,
        0.,
        1.,
        Instant::now() + Duration::from_secs(8),
        cancel,
        false,
        true,
    )
    .await;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.decoded_frames, 48_000);
    let decoder = report.decoder.unwrap();
    assert_eq!(decoder.decoder_kind, "ffmpeg");
    assert!(
        decoder.failed_native_attempt.is_some(),
        "invalid hint must retain the ordinary native probe"
    );
}

#[test]
fn ffmpeg_setup_phase_excludes_injected_failed_native_time() {
    use super::{
        decode_ffmpeg_with_diagnostics, prepare_ffmpeg_fallback_diagnostics,
        FailedNativeAttemptDiagnostics, FailedNativeAttemptReason,
    };
    use std::sync::{
        atomic::{AtomicU32, AtomicU64},
        mpsc, Mutex,
    };
    let fixture = Fixture::create();
    let source = fixture.source(fixture.webm_path.to_string_lossy().into_owned());
    let diagnostics = Arc::new(Mutex::new(None));
    // Inject an earlier diagnostic origin; this is not a measured delay or a
    // timeout. The old bug counted this native phase twice as FFmpeg setup.
    let failed_ms = 60_000;
    let origin = Instant::now() - Duration::from_millis(failed_ms);
    prepare_ffmpeg_fallback_diagnostics(
        Some(&diagnostics),
        &source,
        48_000,
        2,
        Some(FailedNativeAttemptDiagnostics {
            reason: FailedNativeAttemptReason::DecoderSetup,
            duration_ms: failed_ms,
            reader_setup_ms: Some(10),
            native_decoder_setup_ms: Some(failed_ms - 10),
            reader_bytes_read: 1024,
            downloaded_range_end: 2048,
        }),
    );
    let (tx, rx) = mpsc::sync_channel(64);
    decode_ffmpeg_with_diagnostics(
        source,
        0.,
        Some(1.),
        48_000,
        2,
        tx,
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicBool::new(false)),
        Some(diagnostics.clone()),
        origin,
    )
    .unwrap();
    assert_eq!(
        rx.into_iter().map(|block| block.len()).sum::<usize>(),
        96_000
    );
    let decoder = diagnostics.lock().unwrap().clone().unwrap();
    assert!(decoder.first_decoded_block_ms.unwrap() >= failed_ms);
    assert!(
        decoder.decoder_setup_ms.unwrap() < failed_ms,
        "FFmpeg setup must not include the injected failed-native duration"
    );
    assert_eq!(
        decoder.failed_native_attempt.unwrap().duration_ms,
        failed_ms
    );
}
