// Pinned resolver preflight recovery, distinct from Native-reader GET recovery.
// Synthetic TLS/real extractor process/real preflight/real decoder; no device.
const PINNED_FIXTURE_ID: &str = "Syn11111111";
struct PinnedPreflightFixture {
    fixture: Fixture,
    candidate: crate::models::Candidate,
}
impl PinnedPreflightFixture {
    fn new(mode: &str) -> Self {
        let fixture = Fixture::new_with_media_suffix("googlevideo.com");
        fixture.mode(mode);
        let root = fixture.directory.path();
        // A real, valid ID3 padding area makes this short recording large
        // enough to exercise production's >1MiB paired head/tail preflight.
        // Encoded audio frames and the existing tag frames/flags stay intact.
        let original = std::fs::read(root.join("fixture.mp3")).unwrap();
        assert_eq!(&original[..3], b"ID3");
        assert!(matches!(original[3], 3 | 4));
        assert_eq!(original[5] & 0x10, 0, "generated tag has no footer");
        let old_size = original[6..10].iter().fold(0usize, |size, byte| {
            assert_eq!(byte & 0x80, 0, "ID3 size is synchsafe");
            (size << 7) | *byte as usize
        });
        let old_end = 10 + old_size;
        assert!(old_end <= original.len());
        let tag_size = 1_000_000usize;
        assert!(old_size < tag_size && tag_size < 1024 * 1024);
        let mut padded = original[..6].to_vec();
        padded.extend(
            (0..4)
                .rev()
                .map(|shift| ((tag_size >> (7 * shift)) & 0x7f) as u8),
        );
        padded.extend_from_slice(&original[10..old_end]);
        padded.resize(10 + tag_size, 0);
        padded.extend_from_slice(&original[old_end..]);
        assert_eq!(&padded[..6], &original[..6]);
        assert_eq!(&padded[10..10 + old_size], &original[10..old_end]);
        assert_eq!(&padded[10 + tag_size..], &original[old_end..]);
        assert!(padded.len() > 1024 * 1024);
        std::fs::write(root.join("fixture-original.mp3"), &original).unwrap();
        std::fs::write(root.join("fixture.mp3"), &padded).unwrap();
        std::fs::write(root.join("media-fixture.json"), serde_json::json!({
            "originalBytes":original.len(),"actualBytes":padded.len(),
            "id3TagBytes":tag_size,"id3Version":[original[3],original[4]],
            "id3Flags":original[5],"existingTagFramesAndEncodedAudioUnchanged":true,
            "scope":"Valid ID3 padding, no added audio/silence; short genuine recording crosses tail-probe threshold"
        }).to_string()).unwrap();
        let mut opus = Command::new("ffmpeg");
        opus.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(root.join("fixture.mp3"))
            .args(["-c:a", "libopus", "-b:a", "160k"])
            .arg(root.join("fixture.webm"));
        checked(opus);
        let mp3_size = std::fs::metadata(root.join("fixture.mp3")).unwrap().len();
        let opus_size = std::fs::metadata(root.join("fixture.webm")).unwrap().len();
        // Both formats are from one fresh metadata response. Only a SECOND
        // invocation can produce /preflight-2.mp3; prepared metadata cannot.
        let script = format!(
            r##"#!/usr/bin/python3
import json,os,pathlib,sys,time
root=pathlib.Path({root:?})
q=sys.argv[-1]
assert q=='https://www.youtube.com/watch?v={id}'
stat=pathlib.Path('/proc/self/stat').read_text().rsplit(')',1)[1].split()
with open(root/'helper.jsonl','a') as f:f.write(json.dumps({{'query':q,'pid':os.getpid(),'startTimeTicks':stat[19],'processGroup':os.getpgrp()}})+'\n')
count=root/'extract-youtube'
n=int(count.read_text())+1 if count.exists() else 1
count.write_text(str(n))
mode=(root/'mode').read_text()
if n==2 and mode in ('pinned-cancel','pinned-deadline'):
 (root/'second-extraction-entered').touch()
 until=time.monotonic()+10
 while not (root/'release').exists() and time.monotonic()<until:time.sleep(.01)
i='Wrong111111' if n==2 and mode=='pinned-wrong-id' else '{id}'
print(json.dumps({{'id':i,'title':'Fixture Artist - Synthetic Stereo Fixture','track':'Synthetic Stereo Fixture','artist':'Fixture Artist','artists':['Fixture Artist'],'uploader':'Fixture Artist','duration':4,'webpage_url':q,'url':'https://{host}/preflight-'+str(n)+'.mp3','acodec':'mp3','vcodec':'none','protocol':'https','ext':'mp3','format_id':'primary','filesize':{mp3_size},'formats':[{{'url':'https://{host}/prepared-alt.webm','acodec':'opus','vcodec':'none','protocol':'https','ext':'webm','format_id':'prepared-opus','abr':160,'filesize':{opus_size}}}]}}))
"##,
            root = root.to_string_lossy(),
            id = PINNED_FIXTURE_ID,
            host = fixture.host
        );
        std::fs::write(root.join("fake-yt-dlp"), script).unwrap();
        let mut settings = fixture.store.settings().unwrap();
        settings.preferred_streaming_source = "youtube".into();
        settings.streaming_fallback_enabled = true;
        settings.streaming_fallback_sources = vec!["soundcloud".into()];
        fixture.store.save_settings(&settings).unwrap();
        let candidate = crate::models::Candidate {
            id: format!("youtube:{PINNED_FIXTURE_ID}"),
            provider: "youtube".into(),
            url: format!("https://www.youtube.com/watch?v={PINNED_FIXTURE_ID}"),
            title: "Fixture Artist - Synthetic Stereo Fixture".into(),
            duration_seconds: Some(4.),
            ..crate::models::Candidate::default()
        };
        Self { fixture, candidate }
    }
    async fn resolve(
        &self,
        cancel: watch::Receiver<bool>,
        deadline: PlaybackDeadline,
    ) -> Result<playback_sources::ResolvedSource, String> {
        let request =
            playback_sources::selected_preview_track(&self.fixture.track, &self.candidate);
        crate::acquisition::with_playback_test_public_addresses(
            playback_sources::resolve_with_trace_context_excluding_until(
                &self.fixture.store,
                &self.fixture.acquisition,
                &request,
                Some(self.candidate.clone()),
                cancel,
                Some("native-pinned-preflight-composition".into()),
                None,
                None,
                &[],
                deadline,
            ),
        )
        .await
    }
    fn helper_observations(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.fixture.directory.path().join("helper.jsonl"))
            .unwrap_or_default()
            .lines()
            .take(8)
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|call| {
                let pid = call["pid"].as_u64()?;
                let spawn_start = call["startTimeTicks"].as_str();
                Some(match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Ok(stat) => {
                        let fields = stat.rsplit_once(')').map(|(_, fields)| fields.split_whitespace().collect::<Vec<_>>()).unwrap_or_default();
                        let current_start = fields.get(19).copied();
                        let state = fields.first().copied().unwrap_or("unavailable");
                        serde_json::json!({"pid":pid,"spawnStartTimeTicks":spawn_start,
                            "currentStartTimeTicks":current_start,"state":state,
                            "zombie":state=="Z","identityMatches":spawn_start.zip(current_start).map(|(a,b)|a==b),
                            "spawnProcessGroup":call["processGroup"],"currentProcessGroup":fields.get(2).and_then(|value|value.parse::<u64>().ok())})
                    }
                    Err(error) => serde_json::json!({"pid":pid,"spawnStartTimeTicks":spawn_start,
                        "spawnProcessGroup":call["processGroup"],"state":if error.kind()==std::io::ErrorKind::NotFound {None} else {Some("unavailable")},
                        "readErrorKind":format!("{:?}",error.kind())}),
                })
            })
            .collect()
    }
    fn assert_calls(&self, expected: usize) {
        let calls = std::fs::read_to_string(self.fixture.directory.path().join("helper.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            calls.len(),
            expected,
            "exact extraction limit; no search or replacement"
        );
        assert!(calls.iter().all(|call| call["query"] == self.candidate.url));
        self.fixture.assert_saved();
    }
}

impl Drop for PinnedPreflightFixture {
    fn drop(&mut self) {
        // Preserve synthetic transport facts on a baseline assertion failure,
        // before Fixture removes its owned temporary files.
        let count = std::fs::read_to_string(self.fixture.directory.path().join("extract-youtube"))
            .unwrap_or_default();
        eprintln!(
            "PINNED_PREFLIGHT_RECEIPT {}",
            serde_json::json!({
                "mode": std::fs::read_to_string(self.fixture.directory.path().join("mode")).unwrap_or_default(),
                "exactCandidateId": self.candidate.id,
                "helperInvocationCount": count.trim(),
            "helperProcesses": self.helper_observations(),
                "requests": self.fixture.requests(),
            "mediaFixture": std::fs::read_to_string(self.fixture.directory.path().join("media-fixture.json")).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()),
            })
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated no-egress user/net/mount namespace and synthetic TLS; run alone"]
async fn pinned_preflight_403_spends_second_attempt_on_fresh_exact_id_and_decodes_complete_pcm() {
    let fixture = PinnedPreflightFixture::new("pinned-refresh");
    let (_stop, cancel) = watch::channel(false);
    let deadline = PlaybackDeadline::after(Duration::from_secs(12));
    let source = fixture
        .resolve(cancel, deadline)
        .await
        .expect("fresh exact-ID URL after preflight403");
    assert_eq!(source.candidate.as_ref().unwrap().id, fixture.candidate.id);
    assert!(source.source.input.ends_with("/preflight-2.mp3"));
    let (decoded, samples) = finish_decode(start_decode(source.source, deadline));
    decoded.expect("real Native reader/decoder succeeds");
    stereo_tones(&samples);
    fixture.assert_calls(2);
    let requests = fixture.fixture.requests();
    assert!(requests
        .iter()
        .any(|r| r["path"] == "/preflight-1.mp3" && r["status"] == 403));
    assert!(requests
        .iter()
        .any(|r| r["path"] == "/preflight-1.mp3" && r["status"] == 206));
    assert!(
        !requests.iter().any(|r| r["path"] == "/prepared-alt.webm"),
        "403 must not spend retry on same metadata's alternate"
    );
    assert!(requests
        .iter()
        .any(|r| r["path"] == "/preflight-2.mp3" && r["range"].is_null() && r["status"] == 200));
    assert!(requests
        .iter()
        .all(|r| !r["path"].as_str().unwrap().contains("preflight-3")));
}

#[tokio::test]
#[ignore = "requires isolated no-egress user/net/mount namespace and synthetic TLS; run alone"]
async fn pinned_preflight_recovery_preserves_identity_attempt_cap_and_non403_alternate() {
    for mode in ["pinned-wrong-id", "pinned-second-denied", "pinned-non403"] {
        let fixture = PinnedPreflightFixture::new(mode);
        let (_stop, cancel) = watch::channel(false);
        let result = fixture
            .resolve(cancel, PlaybackDeadline::after(Duration::from_secs(12)))
            .await;
        if mode == "pinned-non403" {
            let source = result.expect("non403 retains existing prepared alternate behavior");
            assert!(source.source.input.ends_with("/prepared-alt.webm"));
            assert_eq!(source.candidate.unwrap().id, fixture.candidate.id);
            fixture.assert_calls(1);
        } else {
            assert!(
                result.is_err(),
                "wrong recording and repeated403 remain failures"
            );
            fixture.assert_calls(2);
            assert!(
                !fixture
                    .fixture
                    .requests()
                    .iter()
                    .any(|r| r["range"].is_null()),
                "failed preflight/identity never reaches Native media GET"
            );
            if mode == "pinned-wrong-id" {
                assert!(
                    !fixture
                        .fixture
                        .requests()
                        .iter()
                        .any(|r| r["path"] == "/preflight-2.mp3"),
                    "conflicting ID rejected before touching media URL"
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires isolated no-egress user/net/mount namespace and synthetic TLS; run alone"]
async fn pinned_preflight_fresh_retry_obeys_original_deadline_and_cancellation() {
    for mode in ["pinned-cancel", "pinned-deadline"] {
        let fixture = Arc::new(PinnedPreflightFixture::new(mode));
        let (stop, cancel) = watch::channel(false);
        let started = tokio::time::Instant::now();
        let deadline = PlaybackDeadline::after(Duration::from_secs(if mode == "pinned-cancel" {
            8
        } else {
            3
        }));
        let worker_fixture = fixture.clone();
        let worker = tokio::spawn(async move { worker_fixture.resolve(cancel, deadline).await });
        while !fixture
            .fixture
            .directory
            .path()
            .join("second-extraction-entered")
            .exists()
            && started.elapsed() < Duration::from_secs(3)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            fixture
                .fixture
                .directory
                .path()
                .join("second-extraction-entered")
                .exists(),
            "negative case must reach actual fresh helper, not expire at another stage"
        );
        let canceled_at = tokio::time::Instant::now();
        if mode == "pinned-cancel" {
            stop.send_replace(true);
        }
        let result = worker.await.unwrap();
        let original_attempt_elapsed = started.elapsed();
        let cancel_latency = canceled_at.elapsed();
        assert!(result.is_err());
        if mode == "pinned-cancel" {
            assert_eq!(result.err().unwrap(), "Cancelled");
            assert!(cancel_latency < Duration::from_secs(1));
        } else {
            assert!(original_attempt_elapsed < Duration::from_millis(3400));
            assert!(
                deadline.remaining().is_zero(),
                "retry must retain original deadline"
            );
        }
        // Cancellation/attempt timing above is measured BEFORE this separate
        // resource-cleanup observation. ToolProcess::drop kills synchronously
        // but deliberately reaps in a spawned task; let that task run.
        eprintln!(
            "PINNED_PREFLIGHT_TIMING {}",
            serde_json::json!({"mode":mode,
            "originalAttemptElapsedMs":original_attempt_elapsed.as_secs_f64()*1000.,
            "cancelLatencyMs":(mode=="pinned-cancel").then(||cancel_latency.as_secs_f64()*1000.),
            "cleanupObservationBudgetMs":1000})
        );
        let reap_started = tokio::time::Instant::now();
        loop {
            let helpers = fixture.helper_observations();
            if helpers.iter().all(|helper| helper["state"].is_null()) {
                break;
            }
            assert!(
                reap_started.elapsed() < Duration::from_secs(1),
                "owned helper must be fully reaped within bounded cleanup observation: {helpers:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        eprintln!(
            "PINNED_PREFLIGHT_CLEANUP {}",
            serde_json::json!({"mode":mode,
            "elapsedMs":reap_started.elapsed().as_secs_f64()*1000.,"helpers":fixture.helper_observations()})
        );
        fixture.assert_calls(2);
        assert!(!fixture
            .fixture
            .requests()
            .iter()
            .any(|r| r["path"] == "/preflight-2.mp3"));
    }
}
