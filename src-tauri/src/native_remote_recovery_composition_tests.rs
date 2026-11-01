//! Real resolver/preflight/NativeHttp/decoder composition, with synthetic TLS.
//! Recovery is explicitly driven by this test via the production fresh/excluding
//! resolver APIs. This is NOT PlaybackService automatic recovery, RPC, device or
//! installed-package qualification. Run alone in a no-egress network namespace.

use super::*;
use crate::{
    acquisition::{playback_test_transport, AcquisitionManager},
    models::Track,
    playback_sources::{self, FailedPlaybackCandidate, PlaybackDeadline},
};
use std::{
    process::{Child, Command, Stdio},
    sync::Mutex,
};
use tokio::sync::watch;

fn checked(mut command: Command) {
    let output = command.output().expect("fixture command available");
    assert!(
        output.status.success(),
        "fixture command: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn no_egress_namespace() {
    assert_eq!(
        std::env::var("BURRATA_REMOTE_FIXTURE_NAMESPACE").as_deref(),
        Ok("1"),
        "run only through the documented isolated namespace wrapper"
    );
    let links = Command::new("ip").args(["-j", "link"]).output().unwrap();
    let links: serde_json::Value = serde_json::from_slice(&links.stdout).unwrap();
    assert_eq!(
        links.as_array().unwrap().len(),
        1,
        "namespace must have only lo"
    );
    assert_eq!(links[0]["ifname"], "lo");
    let addresses = Command::new("ip")
        .args(["-j", "addr", "show", "dev", "lo"])
        .output()
        .unwrap();
    let addresses: serde_json::Value = serde_json::from_slice(&addresses.stdout).unwrap();
    assert!(
        addresses[0]["addr_info"]
            .as_array()
            .unwrap()
            .iter()
            .any(|address| address["local"] == "8.8.8.8"),
        "synthetic public VIP must belong to private lo"
    );
    let routes = Command::new("ip").args(["-j", "route"]).output().unwrap();
    let routes: serde_json::Value = serde_json::from_slice(&routes.stdout).unwrap();
    assert!(
        routes.as_array().unwrap().is_empty(),
        "no egress routes allowed"
    );
    let probe =
        std::net::TcpListener::bind("8.8.8.8:443").expect("isolated public VIP must belong to lo");
    drop(probe);
}
struct Fixture {
    _server: ServerProcess,
    _registration: playback_test_transport::Registration,
    directory: tempfile::TempDir,
    host: String,
    ca: Vec<u8>,
    store: Arc<crate::Store>,
    acquisition: AcquisitionManager,
    track: Track,
    saved: serde_json::Value,
    recording_id: String,
}
struct ServerProcess(Child);
impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Fixture {
    fn new() -> Self {
        Self::new_with_media_suffix("sndcdn.com")
    }
    fn new_with_media_suffix(suffix: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        no_egress_namespace();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let host = format!("{}.fixture.{suffix}", uuid::Uuid::new_v4().simple());
        let ca_key = root.join("ca.key");
        let ca = root.join("ca.pem");
        let key = root.join("leaf.key");
        let csr = root.join("leaf.csr");
        let cert = root.join("leaf.pem");
        let ext = root.join("leaf.ext");
        std::fs::write(&ext, format!("subjectAltName=DNS:{host}\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\n")).unwrap();
        let mut cmd = Command::new("openssl");
        cmd.args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=Burrata Synthetic Test CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
            "-keyout",
        ])
        .arg(&ca_key)
        .arg("-out")
        .arg(&ca);
        checked(cmd);
        let mut cmd = Command::new("openssl");
        cmd.args([
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=Burrata Synthetic Test Leaf",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&csr);
        checked(cmd);
        let mut cmd = Command::new("openssl");
        cmd.args(["x509", "-req", "-days", "1", "-in"])
            .arg(&csr)
            .arg("-CA")
            .arg(&ca)
            .arg("-CAkey")
            .arg(&ca_key)
            .arg("-CAcreateserial")
            .arg("-extfile")
            .arg(&ext)
            .arg("-out")
            .arg(&cert);
        checked(cmd);
        let media = root.join("fixture.mp3");
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.2*sin(2*PI*440*t)|0.2*sin(2*PI*660*t):s=48000:d=4",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "192k",
        ])
        .arg(&media);
        checked(cmd);
        let size = std::fs::metadata(&media).unwrap().len();
        let server_script = root.join("server.py");
        std::fs::write(
            &server_script,
            include_str!("../tests/fixtures/remote_tls_fixture_server.py"),
        )
        .unwrap();
        let server = ServerProcess(
            Command::new("python3")
                .arg(&server_script)
                .arg(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        for _ in 0..100 {
            if root.join("ready").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(root.join("ready").exists(), "TLS server failed to start");
        let ca_bytes = std::fs::read(&ca).unwrap();
        let registration = playback_test_transport::register(
            host.clone(),
            ([8, 8, 8, 8], 443).into(),
            ca_bytes.clone(),
        );
        let helper = root.join("fake-yt-dlp");
        let script = format!(
            r##"#!/usr/bin/python3
import json,os,pathlib,sys,time
root=pathlib.Path({root:?})
q=sys.argv[-1]
with open(root/'helper.jsonl','a') as f:f.write(json.dumps({{'query':q,'pid':os.getpid()}})+'\n')
if q.startswith('scsearch'):
 print(json.dumps({{'entries':[{{'id':i,'title':'Fixture Artist - Synthetic Stereo Fixture','track':'Synthetic Stereo Fixture','artist':'Fixture Artist','artists':['Fixture Artist'],'uploader':'Fixture Artist','duration':4,'webpage_url':'https://soundcloud.com/synthetic-fixture/'+i}} for i in ['111','222']]}}))
else:
 i=q.rsplit('/',1)[-1]
 count=root/('extract-'+i)
 n=int(count.read_text())+1 if count.exists() else 1
 count.write_text(str(n))
 path=('a-'+str(n)) if i=='111' else 'b'
 print(json.dumps({{'id':i,'title':'Fixture Artist - Synthetic Stereo Fixture','track':'Synthetic Stereo Fixture','artist':'Fixture Artist','artists':['Fixture Artist'],'uploader':'Fixture Artist','duration':4,'webpage_url':q,'url':'https://{host}/'+path+'.mp3','acodec':'mp3','vcodec':'none','protocol':'https','ext':'mp3','filesize':{size}}}))
"##,
            root = root.to_string_lossy()
        );
        std::fs::write(&helper, script).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Arc::new(crate::Store::open(&root.join("fixture.sqlite")).unwrap());
        let mut settings = store.settings().unwrap();
        settings.preferred_streaming_source = "soundcloud".into();
        settings.streaming_fallback_enabled = false;
        settings.yt_dlp_path = helper.to_string_lossy().into();
        settings.matching_level = crate::recording_match::MatchLevel::Balanced;
        store.save_settings(&settings).unwrap();
        let track = Track {
            id: "spotify:1234567890123456789012".into(),
            provider: "spotify".into(),
            provider_id: "1234567890123456789012".into(),
            source_url: "https://open.spotify.com/track/1234567890123456789012".into(),
            title: "Synthetic Stereo Fixture".into(),
            artist: "Fixture Artist".into(),
            duration_seconds: Some(4.),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let track = store.get_track(&track.id).unwrap().unwrap();
        let saved = serde_json::to_value(&track).unwrap();
        let recording_id = store.recording_id(&track.id).unwrap();
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
        Self {
            directory,
            host,
            ca: ca_bytes,
            _registration: registration,
            _server: server,
            store,
            acquisition,
            track,
            saved,
            recording_id,
        }
    }
    fn mode(&self, mode: &str) {
        std::fs::write(self.directory.path().join("mode"), mode).unwrap();
        for name in [
            "extract-111",
            "extract-222",
            "requests.jsonl",
            "helper.jsonl",
            "release",
        ] {
            let _ = std::fs::remove_file(self.directory.path().join(name));
        }
    }
    fn requests(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.directory.path().join("requests.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn assert_saved(&self) {
        assert_eq!(
            serde_json::to_value(self.store.get_track(&self.track.id).unwrap().unwrap()).unwrap(),
            self.saved
        );
        assert_eq!(
            self.store.recording_id(&self.track.id).unwrap(),
            self.recording_id
        );
        for call in std::fs::read_to_string(self.directory.path().join("helper.jsonl"))
            .unwrap_or_default()
            .lines()
        {
            let call: serde_json::Value = serde_json::from_str(call).unwrap();
            assert!(
                !std::path::Path::new(&format!("/proc/{}", call["pid"].as_u64().unwrap())).exists(),
                "extractor helper reaped"
            );
        }
    }
    async fn resolve(
        &self,
        cancel: watch::Receiver<bool>,
        failed: &[FailedPlaybackCandidate],
        deadline: PlaybackDeadline,
    ) -> Result<playback_sources::ResolvedSource, String> {
        crate::acquisition::with_playback_test_public_addresses(
            playback_sources::resolve_with_trace_context_excluding_until(
                &self.store,
                &self.acquisition,
                &self.track,
                None,
                cancel,
                Some("native-synthetic-remote-composition".into()),
                None,
                None,
                failed,
                deadline,
            ),
        )
        .await
    }
    async fn fresh(
        &self,
        candidate: crate::models::Candidate,
        cancel: watch::Receiver<bool>,
        deadline: PlaybackDeadline,
    ) -> Result<playback_sources::ResolvedSource, String> {
        self.acquisition
            .invalidate_preview_audio(&candidate, &self.store.settings().unwrap());
        crate::acquisition::with_playback_test_public_addresses(
            playback_sources::resolve_candidate_fresh_until(
                &self.store,
                &self.acquisition,
                &self.track,
                candidate,
                false,
                cancel,
                1,
                deadline,
                false,
            ),
        )
        .await
    }
}
struct DecoderRun {
    cancel: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    worker: thread::JoinHandle<Result<(), String>>,
    collector: thread::JoinHandle<()>,
}
fn start_decode(source: AudioSource, deadline: PlaybackDeadline) -> DecoderRun {
    assert!(
        !should_use_ffmpeg_for_source(&source),
        "fixture must use actual NativeHttp reader path"
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let cc = cancel.clone();
    let session = Arc::new(DecoderSession::default());
    session.bind(&cancel);
    session.set_startup_deadline(Some(deadline.instant().into_std()));
    let (tx, rx) = std_mpsc::sync_channel(16);
    let samples = Arc::new(Mutex::new(Vec::new()));
    let captured = samples.clone();
    let collector = thread::spawn(move || {
        for block in rx {
            captured.lock().unwrap().extend(block);
        }
    });
    let worker = thread::spawn(move || {
        decode_with_session(
            source,
            0.,
            None,
            48000,
            2,
            tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU64::new(f64::NAN.to_bits())),
            cc,
            session,
        )
    });
    DecoderRun {
        cancel,
        samples,
        worker,
        collector,
    }
}
fn finish_decode(run: DecoderRun) -> (Result<(), String>, Vec<f32>) {
    let result = run.worker.join().unwrap();
    run.collector.join().unwrap();
    let samples = run.samples.lock().unwrap().clone();
    (result, samples)
}
fn stereo_tones(samples: &[f32]) {
    let seconds = samples.len() as f64 / 96000.;
    assert!(
        (3.95..=4.05).contains(&seconds),
        "whole four-second programme: {seconds}s"
    );
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let mut windows = Vec::new();
    for window in samples.chunks(48000).take(8) {
        let frames = window.len() / 2;
        assert!(
            frames >= 21600,
            "each half-second window complete, with bounded codec edge tolerance"
        );
        let energy = |channel: usize, hz: f64| {
            let mut sin = 0.;
            let mut cos = 0.;
            for (i, frame) in window.chunks_exact(2).enumerate() {
                let phase = 2. * std::f64::consts::PI * hz * i as f64 / 48000.;
                sin += frame[channel] as f64 * phase.sin();
                cos += frame[channel] as f64 * phase.cos();
            }
            (sin * sin + cos * cos).sqrt() / frames as f64
        };
        let rms = |channel: usize| {
            (window
                .chunks_exact(2)
                .map(|frame| (frame[channel] as f64).powi(2))
                .sum::<f64>()
                / frames as f64)
                .sqrt()
        };
        let metrics = [
            energy(0, 440.),
            energy(1, 660.),
            energy(0, 660.),
            energy(1, 440.),
            rms(0),
            rms(1),
        ];
        assert!(
            metrics[0] > 0.045 && metrics[1] > 0.045,
            "expected independent channel tones throughout: {metrics:?}"
        );
        assert!(
            metrics[2] < 0.01 && metrics[3] < 0.01,
            "stereo channel identities, no swapped or doubled lanes: {metrics:?}"
        );
        assert!(
            (0.08..0.2).contains(&metrics[4]) && (0.08..0.2).contains(&metrics[5]),
            "audible full programme: {metrics:?}"
        );
        windows.push(metrics);
    }
    assert_eq!(windows.len(), 8, "no missing later programme windows");
    eprintln!("REMOTE_COMPOSITION_PCM seconds={seconds} windows={windows:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires documented isolated network namespace, local openssl/ffmpeg/python3; no public egress"]
async fn controlled_remote_tls_resolver_decoder_composition() {
    {
        let fixture = Fixture::new();
        let url = url::Url::parse(&format!("https://{}/b.mp3", fixture.host)).unwrap();
        let trusted = playback_test_transport::configure_preflight(
            reqwest::Client::builder()
                .no_proxy()
                .resolve(&fixture.host, ([8, 8, 8, 8], 443).into())
                .timeout(Duration::from_secs(2)),
            &url,
        )
        .unwrap()
        .build()
        .unwrap();
        let positive = trusted
            .get(url.clone())
            .header("Range", "bytes=0-1023")
            .send()
            .await
            .expect("correct leaf/CA/hostname reaches real fixture");
        assert_eq!(positive.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(positive.bytes().await.unwrap().len(), 1024);
        let ordinary = reqwest::Client::builder()
            .no_proxy()
            .resolve(&fixture.host, ([8, 8, 8, 8], 443).into())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let rejected = ordinary.get(url.clone()).send().await.unwrap_err();
        let detail = format!("{rejected:?}").to_ascii_lowercase();
        assert!(
            detail.contains("certificate")
                && (detail.contains("unknownissuer") || detail.contains("unknown issuer")),
            "ordinary client must reject synthetic CA, not merely fail connection: {detail}"
        );
        eprintln!("REMOTE_COMPOSITION_TLS default_ca_rejected=true");
        let wrong = reqwest::Client::builder()
            .no_proxy()
            .resolve("wrong.fixture.sndcdn.com", ([8, 8, 8, 8], 443).into())
            .add_root_certificate(reqwest::Certificate::from_pem(&fixture.ca).unwrap())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let rejected = wrong
            .get("https://wrong.fixture.sndcdn.com/b.mp3")
            .send()
            .await
            .unwrap_err();
        let detail = format!("{rejected:?}").to_ascii_lowercase();
        assert!(
            detail.contains("certificate")
                && (detail.contains("notvalidforname")
                    || detail.contains("not valid for name")
                    || detail.contains("not valid for")),
            "fixture CA must retain hostname validation: {detail}"
        );
        eprintln!("REMOTE_COMPOSITION_TLS wrong_hostname_rejected=true");
    }
    for mode in ["refresh", "fallback"] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let (_stop, cancel) = watch::channel(false);
        let deadline = PlaybackDeadline::after(Duration::from_secs(15));
        let first = fixture
            .resolve(cancel.clone(), &[], deadline)
            .await
            .expect("real resolver and valid preflight A");
        let candidate = first.candidate.clone().unwrap();
        assert_eq!(candidate.id, "soundcloud:111");
        let (failed, pcm) = finish_decode(start_decode(first.source, deadline));
        assert!(failed.unwrap_err().contains("403"));
        assert!(pcm.is_empty());
        let fresh = fixture
            .fresh(candidate.clone(), cancel.clone(), deadline)
            .await
            .expect("same candidate fresh valid preflight");
        assert_eq!(fresh.candidate.as_ref().unwrap().id, candidate.id);
        let (result, pcm) = finish_decode(start_decode(fresh.source, deadline));
        if mode == "refresh" {
            result.expect("fresh A URL decode");
            stereo_tones(&pcm);
        } else {
            assert!(result.unwrap_err().contains("403"));
            assert!(pcm.is_empty());
            fixture
                .acquisition
                .invalidate_preview_audio(&candidate, &fixture.store.settings().unwrap());
            let replacement = fixture
                .resolve(
                    cancel.clone(),
                    &[FailedPlaybackCandidate {
                        provider: "soundcloud".into(),
                        id: candidate.id.clone(),
                    }],
                    deadline,
                )
                .await
                .expect("replacement excludes failed A");
            assert_eq!(replacement.candidate.as_ref().unwrap().id, "soundcloud:222");
            let (result, pcm) = finish_decode(start_decode(replacement.source, deadline));
            result.expect("B real decoder");
            stereo_tones(&pcm);
            assert_eq!(
                std::fs::read_to_string(fixture.directory.path().join("extract-111")).unwrap(),
                "2",
                "A must not be extracted a third time during replacement"
            );
        }
        let requests = fixture.requests();
        let actual_reader_responses = requests
            .iter()
            .filter(|request| request["range"].is_null() && request["stage"] == "response")
            .map(|request| {
                (
                    request["path"].as_str().unwrap(),
                    request["status"].as_u64().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let expected_reader_responses = if mode == "refresh" {
            vec![("/a-1.mp3", 403), ("/a-2.mp3", 200)]
        } else {
            vec![("/a-1.mp3", 403), ("/a-2.mp3", 403), ("/b.mp3", 200)]
        };
        assert_eq!(
            actual_reader_responses, expected_reader_responses,
            "exact reader attempt order; no late failed-A resurrection"
        );
        for path in ["/a-1.mp3", "/a-2.mp3"] {
            let preflight = requests
                .iter()
                .position(|request| {
                    request["path"] == path
                        && request["range"].is_string()
                        && request["status"] == 206
                })
                .unwrap();
            let reader = requests
                .iter()
                .position(|request| {
                    request["path"] == path
                        && request["range"].is_null()
                        && request["stage"] == "response"
                })
                .unwrap();
            assert!(
                preflight < reader,
                "real valid preflight precedes decoder request for {path}"
            );
        }
        assert!(requests
            .iter()
            .any(|r| r["path"] == "/a-1.mp3" && r["range"].is_string() && r["status"] == 206));
        assert!(requests
            .iter()
            .any(|r| r["path"] == "/a-1.mp3" && r["range"].is_null() && r["status"] == 403));
        assert!(requests
            .iter()
            .any(|r| r["path"] == "/a-2.mp3" && r["range"].is_string() && r["status"] == 206));
        fixture.assert_saved();
        eprintln!(
            "REMOTE_COMPOSITION {mode} {}",
            serde_json::to_string(&requests).unwrap()
        );
    }
    for mode in ["cancel", "deadline"] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let (_stop, cancel) = watch::channel(false);
        let deadline =
            PlaybackDeadline::after(Duration::from_secs(if mode == "cancel" { 5 } else { 2 }));
        let resolved = fixture
            .resolve(cancel, &[], deadline)
            .await
            .expect("preflight succeeds before blocked decoder response");
        let started = Instant::now();
        let run = start_decode(resolved.source, deadline);
        for _ in 0..200 {
            if fixture
                .requests()
                .iter()
                .any(|r| r["range"].is_null() && r["stage"] == "entered")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(fixture
            .requests()
            .iter()
            .any(|r| r["range"].is_null() && r["stage"] == "entered"));
        if mode == "cancel" {
            run.cancel.store(true, Ordering::Release);
        }
        let (result, pcm) = finish_decode(run);
        assert!(result.is_err());
        assert!(pcm.is_empty());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "reader respects original deadline/cancel"
        );
        std::fs::write(fixture.directory.path().join("release"), "1").unwrap();
        for _ in 0..100 {
            if fixture
                .requests()
                .iter()
                .any(|r| r["range"].is_null() && r["stage"] == "handler_finished")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            fixture
                .requests()
                .iter()
                .any(|r| r["range"].is_null() && r["stage"] == "handler_finished"),
            "cancelled/timed-out reader connection closes after fixture release"
        );
        assert!(deadline.remaining() <= Duration::from_secs(if mode == "cancel" { 5 } else { 2 }));
        if mode == "deadline" {
            let before = fixture.requests().len();
            let (_stop, cancel) = watch::channel(false);
            assert!(
                fixture.resolve(cancel, &[], deadline).await.is_err(),
                "replacement cannot reset exhausted deadline"
            );
            assert_eq!(
                fixture.requests().len(),
                before,
                "expired budget creates no new media access"
            );
        }
        fixture.assert_saved();
        eprintln!(
            "REMOTE_COMPOSITION {mode} {}",
            serde_json::to_string(&fixture.requests()).unwrap()
        );
    }
}

#[test]
fn remote_composition_pcm_gate_rejects_truncation_late_silence_and_swapped_lanes() {
    let samples = (0..4 * 48000)
        .flat_map(|i| {
            [
                (0.2 * (2. * std::f64::consts::PI * 440. * i as f64 / 48000.).sin()) as f32,
                (0.2 * (2. * std::f64::consts::PI * 660. * i as f64 / 48000.).sin()) as f32,
            ]
        })
        .collect::<Vec<_>>();
    stereo_tones(&samples);
    assert!(std::panic::catch_unwind(|| stereo_tones(&samples[..3 * 96000])).is_err());
    let mut silent = samples.clone();
    silent[2 * 96000..].fill(0.);
    assert!(std::panic::catch_unwind(|| stereo_tones(&silent)).is_err());
    let mut swapped = samples;
    for frame in swapped.chunks_exact_mut(2) {
        frame.swap(0, 1);
    }
    assert!(std::panic::catch_unwind(|| stereo_tones(&swapped)).is_err());
}

include!("native_pinned_preflight_recovery_tests.rs");
