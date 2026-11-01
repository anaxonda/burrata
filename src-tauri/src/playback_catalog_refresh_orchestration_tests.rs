// Offline tests of resolve_inner itself. The accepted second source is cancelled
// before CDN access: these qualify orchestration, not successful remote playback.
#[cfg(unix)]
mod exact_refresh_orchestration {
    use super::*;
    use crate::browser_bridge::ProviderTransport;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };
    use tokio::sync::Notify;

    const ID: &str = "1234567890123456789012";
    const OLD: &str = "Stale Catalog Title";
    const NEW: &str = "Correct Catalog Title";

    struct RefreshTransport {
        calls: Mutex<Vec<(String, Value)>>,
        entered: Notify,
        release: Notify,
        blocked: bool,
        delay: Duration,
        cancel_on_response: Option<watch::Sender<bool>>,
        dropped: Arc<AtomicBool>,
    }
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl ProviderTransport for RefreshTransport {
        fn provider_operations_supported(&self) -> bool {
            true
        }
        async fn provider_operation(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
        ) -> Result<Value, String> {
            assert_eq!(provider, "spotify");
            self.calls
                .lock()
                .unwrap()
                .push((operation.into(), args.clone()));
            match operation {
                "status" => Ok(json!({"accountId":"fixture-account","displayName":"Fixture"})),
                "getTrack" => {
                    assert_eq!(args, json!({"trackId":ID}));
                    let _drop = DropFlag(self.dropped.clone());
                    self.entered.notify_one();
                    if self.blocked {
                        self.release.notified().await;
                    }
                    tokio::time::sleep(self.delay).await;
                    if let Some(cancel) = &self.cancel_on_response {
                        cancel.send_replace(true);
                    }
                    Ok(json!({"accountId":"fixture-account","items":[{
                        "kind":"track","id":ID,"title":NEW,
                        "url":format!("https://open.spotify.com/track/{ID}"),
                        "data":{"id":ID,"title":NEW,"artist":"Provider credit must not replace edit",
                            "album":"Provider album must not replace edit","durationSeconds":240,
                            "url":format!("https://open.spotify.com/track/{ID}")}
                    }]}))
                }
                _ => panic!("Unexpected provider operation {operation}"),
            }
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            _cancel: Option<Arc<AtomicBool>>,
        ) -> Result<Value, String> {
            self.provider_operation(provider, operation, args).await
        }
    }

    struct Fixture {
        directory: tempfile::TempDir,
        store: Arc<Store>,
        acquisition: AcquisitionManager,
        track: Track,
        saved: Value,
        recording_id: String,
        settings: Value,
        transport: Arc<RefreshTransport>,
        stop: watch::Sender<bool>,
        cancel: watch::Receiver<bool>,
        trace: SourceTrace,
        events: Arc<Mutex<Vec<Value>>>,
        helper_warmup: Duration,
    }
    impl Fixture {
        fn new(blocked: bool, cancel_on_response: bool, timeout_case: bool) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let directory = tempfile::tempdir().unwrap();
            let store =
                Arc::new(Store::open(&directory.path().join("orchestration.sqlite")).unwrap());
            let helper = directory.path().join("fake-yt-dlp");
            let log = directory.path().join("helper.jsonl");
            let pid = directory.path().join("blocked-helper.pid");
            let script = format!(
                r##"#!/usr/bin/python3
import json,os,sys,time
args=sys.argv[1:]
log={log:?}
if args==['--fixture-warmup']:
    with open({ready:?},'w') as f:json.dump({{'ready':True,'python':sys.executable}},f)
    sys.exit(0)
query=args[-1]
search=query.startswith('scsearch')
corrected='correct catalog title' in query.lower()
with open(log,'a') as f:f.write(json.dumps({{'kind':'search' if search else 'extract','query':query}})+'\n')
if search:
    title='Correct Catalog Title' if corrected else 'Stale Catalog Title'
    ids=['111','222'] if corrected else ['111']
    print(json.dumps({{'entries':[{{'id':i,'title':title,'uploader':'Fixture Artist','artist':'Fixture Artist','duration':240,'webpage_url':'https://soundcloud.com/fixture/'+i}} for i in ids]}}))
else:
    i=query.rsplit('/',1)[-1]
    if i=='222' and {timeout_case}:
        with open({pid:?},'w') as f:f.write(str(os.getpid()))
        time.sleep(60)
    print(json.dumps({{'id':i,'title':'Fixture Artist - Correct Catalog Title','track':'Correct Catalog Title','artist':'Fixture Artist','artists':['Fixture Artist'],'uploader':'Fixture Artist','duration':240,'webpage_url':query,'url':'https://cf-media.sndcdn.com/audio.m4a','acodec':'aac','vcodec':'none','protocol':'https','ext':'m4a'}}))
"##,
                log = log.to_string_lossy(),
                pid = pid.to_string_lossy(),
                ready = directory.path().join("helper-ready.json").to_string_lossy(),
                timeout_case = if timeout_case { "True" } else { "False" }
            );
            std::fs::write(&helper, script).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            // Interpreter startup is fixture preparation, not the invariant
            // under test. Keep the exact helper path/interpreter and warm it
            // before creating the measured request's absolute deadline.
            let helper_warmup_started = Instant::now();
            let mut warmup = std::process::Command::new(&helper)
                .arg("--fixture-warmup")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("fake extractor warmup must start");
            loop {
                if let Some(status) = warmup.try_wait().unwrap() {
                    assert!(status.success(), "fake extractor warmup failed");
                    break;
                }
                if helper_warmup_started.elapsed() > Duration::from_secs(15) {
                    let _ = warmup.kill();
                    let _ = warmup.wait();
                    panic!("fake extractor warmup exceeded fixture preparation bound");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let helper_warmup = helper_warmup_started.elapsed();
            let mut settings = store.settings().unwrap();
            settings.preferred_streaming_source = "soundcloud".into();
            settings.streaming_fallback_enabled = false;
            settings.yt_dlp_path = helper.to_string_lossy().into();
            settings.matching_level = crate::recording_match::MatchLevel::Balanced;
            store.save_settings(&settings).unwrap();
            let original = Track {
                id: format!("spotify:{ID}"),
                provider: "spotify".into(),
                provider_id: ID.into(),
                source_url: format!("https://open.spotify.com/track/{ID}"),
                title: OLD.into(),
                artist: "Fixture Artist".into(),
                album: Some("My edited album".into()),
                duration_seconds: Some(240.),
                provenance: json!({"manualFields":{"artist":true,"album":true}}),
                ..Track::default()
            };
            store.save_track(&original).unwrap();
            let track = store.get_track(&original.id).unwrap().unwrap();
            let saved = serde_json::to_value(&track).unwrap();
            let recording_id = store.recording_id(&track.id).unwrap();
            let settings = serde_json::to_value(store.settings().unwrap()).unwrap();
            let (stop, cancel) = watch::channel(false);
            let transport = Arc::new(RefreshTransport {
                calls: Mutex::new(vec![]),
                entered: Notify::new(),
                release: Notify::new(),
                blocked,
                delay: if timeout_case {
                    Duration::from_millis(200)
                } else {
                    Duration::ZERO
                },
                cancel_on_response: cancel_on_response.then(|| stop.clone()),
                dropped: Arc::new(AtomicBool::new(false)),
            });
            let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
            let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
            acquisition.set_remote_playlist_service(Arc::new(
                crate::remote_playlists::RemotePlaylistService::new(
                    store.clone(),
                    transport.clone(),
                ),
            ));
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = events.clone();
            let cancel_at_accept = stop.clone();
            let trace = SourceTrace::new(Some("native-exact-refresh-test".into()), None, None)
                .with_test_sink(Arc::new(move |event| {
                    if !timeout_case
                        && event["phase"] == "full_identity_check"
                        && event["details"]["ok"] == true
                    {
                        cancel_at_accept.send_replace(true);
                    }
                    sink.lock().unwrap().push(event);
                }));
            Self {
                directory,
                store,
                acquisition,
                track,
                saved,
                recording_id,
                settings,
                transport,
                stop,
                cancel,
                trace,
                events,
                helper_warmup,
            }
        }
        async fn second_extraction_entered(&self) -> u32 {
            loop {
                if let Ok(pid) =
                    std::fs::read_to_string(self.directory.path().join("blocked-helper.pid"))
                {
                    return pid.parse().expect("fake extractor PID");
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        async fn assert_helper_stopped(&self, pid: u32) {
            // /proc is Linux-only: its absence on macOS is not exit proof.
            // Query only the recorded synthetic helper PID, without signaling.
            let helper = self.directory.path().join("fake-yt-dlp");
            let canonical_helper = helper.canonicalize().unwrap();
            loop {
                let output = tokio::process::Command::new("/bin/ps")
                    .args(["-ww", "-p", &pid.to_string(), "-o", "pid=,stat=,command="])
                    .kill_on_drop(true)
                    .output()
                    .await
                    .expect("owned helper process status must be observable");
                if output.status.code() == Some(1)
                    && output.stdout.iter().all(u8::is_ascii_whitespace)
                {
                    return;
                }
                assert!(output.status.success(), "owned helper process query failed");
                let output = String::from_utf8(output.stdout).unwrap();
                let mut fields = output.split_whitespace();
                assert_eq!(fields.next().and_then(|v| v.parse::<u32>().ok()), Some(pid));
                let state = fields.next().expect("owned helper process state");
                if state.contains('Z') {
                    return;
                }
                assert!(
                    output.contains(helper.to_string_lossy().as_ref())
                        || output.contains(canonical_helper.to_string_lossy().as_ref()),
                    "recorded helper PID is live but its identity cannot be verified"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        async fn run(&self, deadline: PlaybackDeadline) -> Result<ResolvedSource, String> {
            acquisition::with_playback_test_public_addresses(within_shared_deadline(
                deadline,
                Box::pin(resolve_inner(
                    &self.store,
                    &self.acquisition,
                    &self.track,
                    None,
                    self.cancel.clone(),
                    &self.trace,
                    &[],
                    deadline,
                    2,
                    false,
                )),
            ))
            .await
        }
        fn unchanged(&self) {
            assert_eq!(serde_json::to_value(&self.track).unwrap(), self.saved);
            assert_eq!(
                serde_json::to_value(self.store.get_track(&self.track.id).unwrap().unwrap())
                    .unwrap(),
                self.saved
            );
            assert_eq!(
                self.store.recording_id(&self.track.id).unwrap(),
                self.recording_id
            );
            assert_eq!(
                serde_json::to_value(self.store.settings().unwrap()).unwrap(),
                self.settings
            );
        }
        fn helper_calls(&self) -> Vec<Value> {
            std::fs::read_to_string(self.directory.path().join("helper.jsonl"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
        fn phase_count(&self, phase: &str) -> usize {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["phase"] == phase)
                .count()
        }
        fn accepted_refresh_count(&self) -> usize {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| {
                    e["phase"] == "catalog_request_refresh"
                        && e["details"] == json!({"accepted":true})
                })
                .count()
        }
        fn assert_no_media_access(&self) {
            let events = self.events.lock().unwrap();
            assert!(
                !events.iter().any(|e| matches!(
                    e["phase"].as_str(),
                    Some("dns_stream_start" | "stream_probe_start" | "alias_metadata_request_end")
                )),
                "{events:#?}"
            );
        }
        fn assert_exact_read(&self) {
            let calls = self.transport.calls.lock().unwrap();
            assert_eq!(
                calls.iter().filter(|(name, _)| name == "getTrack").count(),
                1,
                "{calls:?}"
            );
        }
    }

    #[tokio::test]
    async fn exact_refresh_orchestration_preserves_edits_ids_and_excludes_attempted_source() {
        let f = Fixture::new(false, false, false);
        let result = f.run(PlaybackDeadline::after(Duration::from_secs(6))).await;
        assert_eq!(
            result.err().as_deref(),
            Some("Cancelled"),
            "{:#?}",
            f.events.lock().unwrap()
        );
        f.assert_exact_read();
        f.unchanged();
        f.assert_no_media_access();
        assert_eq!(f.accepted_refresh_count(), 1);
        let calls = f.helper_calls();
        let extracts = calls
            .iter()
            .filter(|e| e["kind"] == "extract")
            .collect::<Vec<_>>();
        assert_eq!(extracts.len(), 2, "{calls:#?}");
        assert!(extracts[0]["query"].as_str().unwrap().ends_with("/111"));
        assert!(extracts[1]["query"].as_str().unwrap().ends_with("/222"));
        let events = f.events.lock().unwrap();
        let checks = events
            .iter()
            .filter(|e| e["phase"] == "full_identity_check")
            .collect::<Vec<_>>();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0]["details"]["ok"], false);
        assert_eq!(checks[1]["details"]["ok"], true);
        assert_eq!(f.trace.extraction_attempts_used(), 2);
        let attempted = f.trace.attempted_sources.lock().unwrap();
        assert_eq!(
            attempted.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["soundcloud:111", "soundcloud:222"]
        );
    }

    #[tokio::test]
    async fn exact_refresh_orchestration_keeps_original_deadline() {
        let f = Fixture::new(false, false, true);
        // Use the same six-second allowance as the sibling orchestration
        // tests. Four real helper launches must reach the controlled blocked
        // second extractor; a pre-lookup timeout does not exercise refresh.
        let deadline = PlaybackDeadline::after(Duration::from_secs(6));
        let original = deadline.instant();
        let run = f.run(deadline);
        tokio::pin!(run);
        let pid = tokio::select! {
            biased;
            pid=f.second_extraction_entered()=>pid,
            result=&mut run=>panic!("second extraction not reached before request ended: {:?}; helper {:?}; calls {:#?}; events {:#?}",result.err(),f.helper_warmup,f.helper_calls(),f.events.lock().unwrap()),
        };
        assert!(
            !deadline.remaining().is_zero(),
            "phase entered after original deadline"
        );
        eprintln!(
            "refresh deadline phase entered: warmupMs={}, remainingMs={}, attempts={}",
            f.helper_warmup.as_millis(),
            deadline.remaining().as_millis(),
            f.trace.extraction_attempts_used()
        );
        // The original deadline is never replaced after refresh or handoff.
        let result = run.await;
        assert!(deadline.remaining().is_zero(), "{:?}", result.err());
        assert!(tokio::time::Instant::now() <= original + Duration::from_millis(400));
        assert_eq!(deadline.instant(), original);
        f.stop.send_replace(true);
        f.assert_exact_read();
        f.unchanged();
        f.assert_no_media_access();
        assert_eq!(f.accepted_refresh_count(), 1);
        assert_eq!(f.trace.extraction_attempts_used(), 2);
        tokio::time::timeout(Duration::from_secs(2), f.assert_helper_stopped(pid))
            .await
            .expect("timed-out fake extractor remains running");
    }

    #[tokio::test]
    async fn exact_refresh_orchestration_cancellation_drops_pending_exact_lookup() {
        let f = Fixture::new(true, false, false);
        let deadline = PlaybackDeadline::after(Duration::from_secs(6));
        let run = f.run(deadline);
        tokio::pin!(run);
        tokio::select! {
            _=f.transport.entered.notified()=>{},
            result=&mut run=>panic!("resolver completed before exact lookup: {:?}; events {:#?}",result.err(),f.events.lock().unwrap()),
            _=tokio::time::sleep_until(deadline.instant())=>panic!("exact lookup not reached by original deadline; helper {:?}; calls {:#?}; events {:#?}",f.helper_warmup,f.helper_calls(),f.events.lock().unwrap()),
        }
        f.assert_exact_read();
        assert!(
            deadline.remaining() > Duration::from_millis(500),
            "fixture must reach blocked lookup with an independent cancellation window"
        );
        let cancelled_at = Instant::now();
        f.stop.send_replace(true);
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(500), run)
                .await
                .expect("pending exact lookup cancellation must be prompt")
                .err()
                .as_deref(),
            Some("Cancelled")
        );
        eprintln!(
            "refresh cancellation observed: warmupMs={}, cancelMs={}",
            f.helper_warmup.as_millis(),
            cancelled_at.elapsed().as_millis()
        );
        assert!(f.transport.dropped.load(Ordering::SeqCst));
        f.assert_exact_read();
        f.unchanged();
        f.assert_no_media_access();
        assert_eq!(f.accepted_refresh_count(), 0);
        assert_eq!(
            f.helper_calls()
                .iter()
                .filter(|e| e["kind"] == "extract")
                .count(),
            1
        );
    }

    #[test]
    fn fixture_warmup_never_spends_search_extraction_or_catalog_attempts() {
        let f = Fixture::new(false, false, false);
        assert!(f.helper_calls().is_empty());
        assert!(f.transport.calls.lock().unwrap().is_empty());
        let ready: Value = serde_json::from_slice(
            &std::fs::read(f.directory.path().join("helper-ready.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ready["ready"], true);
        f.unchanged();
    }

    #[tokio::test]
    async fn exact_refresh_orchestration_ready_metadata_and_cancel_cannot_publish_retry() {
        let f = Fixture::new(false, true, false);
        assert_eq!(
            f.run(PlaybackDeadline::after(Duration::from_secs(6)))
                .await
                .err()
                .as_deref(),
            Some("Cancelled")
        );
        f.assert_exact_read();
        f.unchanged();
        f.assert_no_media_access();
        assert!(f.transport.dropped.load(Ordering::SeqCst));
        assert_eq!(
            f.helper_calls()
                .iter()
                .filter(|e| e["kind"] == "extract")
                .count(),
            1
        );
        assert_eq!(f.phase_count("full_identity_check"), 1);
    }
}
