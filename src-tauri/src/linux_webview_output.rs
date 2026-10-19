//! Fail-closed Linux capture of the only currently audible Burrata WebView.
//! Callers must refresh the UI status while capture runs.
use serde_json::Value;
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::watch, time::Instant};

const RATE: u32 = 48_000;
#[cfg(test)]
const BYTES: usize = RATE as usize * 2 * 8;

/// Private test seam only: Tauri commands never accept executable paths or
/// environment overrides. Production always uses the system Pulse utilities.
#[derive(Clone)]
struct CaptureTools {
    pactl: PathBuf,
    parec: PathBuf,
    env: Vec<(OsString, OsString)>,
    recheck_interval: Duration,
    read_timeout: Duration,
    startup_timeout: Duration,
    capture_seconds: usize,
}

impl CaptureTools {
    fn system() -> Self {
        Self {
            pactl: PathBuf::from("pactl"),
            parec: PathBuf::from("parec"),
            env: Vec::new(),
            recheck_interval: Duration::from_millis(500),
            read_timeout: Duration::from_millis(750),
            startup_timeout: Duration::from_secs(2),
            capture_seconds: 8,
        }
    }

    fn command(&self, executable: &OsStr) -> Command {
        let mut command = Command::new(executable);
        command.envs(self.env.iter().cloned());
        command
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ViewAudio {
    pub main: bool,
    pub others: usize,
    pub known: bool,
}
fn validate(s: ViewAudio) -> Result<(), String> {
    if !s.known {
        return Err("Could not verify audio state for every Burrata playback window. Try again in a moment.".into());
    }
    if !s.main {
        return Err("Start playback in Burrata before identifying audio.".into());
    }
    if s.others != 0 {
        return Err("Pause audio in the other Burrata window, then try again.".into());
    }
    Ok(())
}

/// Process table reads only `/proc/*/status` Name and PPid, never command lines
/// or unrelated Pulse metadata. The root and all WebKit process descendants
/// are the app ownership boundary.
pub fn owned_webkit_pids(root: u32) -> Result<Vec<(u32, u64)>, String> {
    let mut parents = Vec::new();
    for entry in std::fs::read_dir("/proc")
        .map_err(|_| "Could not inspect owned WebKit processes")?
        .flatten()
    {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let name = status
            .lines()
            .find_map(|l| l.strip_prefix("Name:\t"))
            .unwrap_or("")
            .to_owned();
        let ppid = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:\t"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let starttime = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')').map(|(_, tail)| tail.to_owned()))
            .and_then(|tail| {
                tail.split_whitespace()
                    .nth(19)
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .unwrap_or(0);
        parents.push((pid, ppid, name, starttime));
    }
    let mut descendants = HashSet::from([root]);
    loop {
        let n = descendants.len();
        for (pid, ppid, _, _) in &parents {
            if descendants.contains(ppid) {
                descendants.insert(*pid);
            }
        }
        if n == descendants.len() {
            break;
        }
    }
    let result: Vec<_> = parents
        .into_iter()
        .filter(|(pid, _, name, _)| {
            descendants.contains(pid) && name.starts_with("WebKitWebProces")
        })
        .map(|(pid, _, _, start)| (pid, start))
        .collect();
    if result.is_empty() {
        Err("No Burrata-owned WebKit process is available".into())
    } else if result.iter().any(|(_, start)| *start == 0) {
        Err("Could not verify Burrata WebKit process identity".into())
    } else {
        Ok(result)
    }
}

fn one_stream(rows: &Value, pids: &[(u32, u64)]) -> Result<(u32, u32, u32), String> {
    let allow: HashSet<_> = pids.iter().map(|(pid, _)| *pid).collect();
    let mut found = None;
    for row in rows.as_array().ok_or("Invalid Pulse sink-input list")? {
        let props = &row["properties"];
        let pid = props["application.process.id"]
            .as_str()
            .and_then(|v| v.parse::<u32>().ok());
        if let Some(pid) = pid.filter(|p| allow.contains(p)) {
            let index = row["index"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or("Invalid Pulse stream index")?;
            let sink = row["sink"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v != u32::MAX)
                    .ok_or("Playback has not reached an audio output yet. Wait for it to start, then try again.")?;
            if found.is_some() {
                return Err("Audio is playing in more than one Burrata window. Pause audio in the other window, then try again.".into());
            }
            found = Some((index, sink, pid));
        }
    }
    found.ok_or_else(|| {
        "No Burrata audio stream is active. Start playback in Burrata and try again.".into()
    })
}

/// Explicitly stop and reap recorder children on controlled early exits.
/// `kill_on_drop` remains a final guard for cancellation/deadline task drops.
async fn kill_and_reap(child: &mut tokio::process::Child) {
    // kill can fail when the child already exited; wait is still required to
    // collect its exit status and release the process table entry.
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn pulse_json(
    args: &[&str],
    tools: &CaptureTools,
    deadline: Instant,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Value, String> {
    if *cancellation.borrow() {
        return Err("Audio identification was cancelled".into());
    }
    let command_deadline = (Instant::now() + Duration::from_secs(2)).min(deadline);
    let mut child = tools
        .command(tools.pactl.as_os_str())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Linux output capture requires pactl and parec (install pulseaudio-utils)")?;
    let Some(mut stdout) = child.stdout.take() else {
        kill_and_reap(&mut child).await;
        return Err("Could not inspect the selected Burrata audio output".into());
    };
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if *cancellation.borrow() {
            kill_and_reap(&mut child).await;
            return Err("Audio identification was cancelled".into());
        }
        if Instant::now() >= command_deadline {
            kill_and_reap(&mut child).await;
            return Err(if command_deadline == deadline {
                "Selected audio capture exceeded its 12-second limit".into()
            } else {
                "Timed out checking owned audio output".into()
            });
        }
        let read = tokio::select! {
            biased;
            _ = cancellation.changed() => {
                kill_and_reap(&mut child).await;
                return Err("Audio identification was cancelled".into());
            }
            _ = tokio::time::sleep_until(command_deadline) => {
                kill_and_reap(&mut child).await;
                return Err(if command_deadline == deadline {
                    "Selected audio capture exceeded its 12-second limit".into()
                } else {
                    "Timed out checking owned audio output".into()
                });
            }
            read = stdout.read(&mut buffer) => read,
        };
        let got = match read {
            Ok(got) => got,
            Err(_) => {
                kill_and_reap(&mut child).await;
                return Err("Could not inspect the selected Burrata audio output".into());
            }
        };
        if got == 0 {
            break;
        }
        if output.len().saturating_add(got) > 1_048_576 {
            kill_and_reap(&mut child).await;
            return Err("Pulse audio output metadata is too large".into());
        }
        output.extend_from_slice(&buffer[..got]);
    }
    let status_result = tokio::select! {
        biased;
        result = child.wait() => result,
        _ = cancellation.changed() => {
            kill_and_reap(&mut child).await;
            return Err("Audio identification was cancelled".into());
        }
        _ = tokio::time::sleep_until(command_deadline) => {
            kill_and_reap(&mut child).await;
            return Err(if command_deadline == deadline {
                "Selected audio capture exceeded its 12-second limit".into()
            } else {
                "Timed out checking owned audio output".into()
            });
        }
    };
    let status = match status_result {
        Ok(status) => status,
        Err(_) => {
            kill_and_reap(&mut child).await;
            return Err("Could not inspect the selected Burrata audio output".into());
        }
    };
    if !status.success() {
        return Err("Could not inspect the selected Burrata audio output".into());
    }
    serde_json::from_slice(&output)
        .map_err(|_| "Pulse returned malformed audio output metadata".into())
}

async fn selected(
    pids: &[(u32, u64)],
    tools: &CaptureTools,
    deadline: Instant,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(u32, u32, u32, String), String> {
    let rows = pulse_json(
        &["--format=json", "list", "sink-inputs"],
        tools,
        deadline,
        cancellation,
    )
    .await?;
    let (input, sink, pid) = one_stream(&rows, pids)?;
    let sinks = pulse_json(
        &["--format=json", "list", "sinks"],
        tools,
        deadline,
        cancellation,
    )
    .await?;
    let row = sinks
        .as_array()
        .and_then(|v| v.iter().find(|r| r["index"].as_u64() == Some(sink as u64)))
        .ok_or("Owned stream output disappeared")?;
    let monitor = row["monitor_source"]
        .as_str()
        .or_else(|| row["monitor_source_name"].as_str())
        .filter(|s| !s.is_empty())
        .ok_or("Owned output has no monitor source")?;
    Ok((input, sink, pid, monitor.to_owned()))
}

pub async fn capture_wav(
    root_pid: u32,
    max_capture: Duration,
    status: impl Fn() -> ViewAudio,
    current: impl Fn() -> bool,
) -> Result<Vec<u8>, String> {
    let (_cancel_tx, cancellation) = watch::channel(false);
    capture_wav_cancellable(root_pid, max_capture, status, current, cancellation).await
}

/// Capture a bounded caller-selected excerpt of the owned audible output.
pub async fn capture_wav_seconds_cancellable(
    root_pid: u32,
    max_capture: Duration,
    seconds: usize,
    status: impl Fn() -> ViewAudio,
    current: impl Fn() -> bool,
    cancellation: watch::Receiver<bool>,
) -> Result<Vec<u8>, String> {
    if !(3..=12).contains(&seconds) {
        return Err("Audio capture must contain three to twelve seconds".into());
    }
    let mut tools = CaptureTools::system();
    tools.capture_seconds = seconds;
    capture_wav_with_tools_cancellable(root_pid, max_capture, status, current, cancellation, tools)
        .await
}

pub async fn capture_wav_cancellable(
    root_pid: u32,
    max_capture: Duration,
    status: impl Fn() -> ViewAudio,
    current: impl Fn() -> bool,
    cancellation: watch::Receiver<bool>,
) -> Result<Vec<u8>, String> {
    capture_wav_with_tools_cancellable(
        root_pid,
        max_capture,
        status,
        current,
        cancellation,
        CaptureTools::system(),
    )
    .await
}

async fn capture_wav_with_tools(
    root_pid: u32,
    max_capture: Duration,
    status: impl Fn() -> ViewAudio,
    current: impl Fn() -> bool,
    tools: CaptureTools,
) -> Result<Vec<u8>, String> {
    let (_cancel_tx, cancellation) = watch::channel(false);
    capture_wav_with_tools_cancellable(root_pid, max_capture, status, current, cancellation, tools)
        .await
}

async fn capture_wav_with_tools_cancellable(
    root_pid: u32,
    max_capture: Duration,
    status: impl Fn() -> ViewAudio,
    current: impl Fn() -> bool,
    mut cancellation: watch::Receiver<bool>,
    tools: CaptureTools,
) -> Result<Vec<u8>, String> {
    if max_capture.is_zero() {
        return Err("Audio identification exceeded its 60-second limit".into());
    }
    let capture_bytes = RATE as usize * 2 * tools.capture_seconds;
    let deadline =
        Instant::now() + max_capture.min(Duration::from_secs(tools.capture_seconds as u64 + 4));
    if *cancellation.borrow() {
        return Err("Audio identification was cancelled".into());
    }
    validate(status())?;
    if !current() {
        return Err("Playback changed before capture".into());
    }
    let pids = owned_webkit_pids(root_pid)?;
    let (input, sink, selected_pid, device) =
        selected(&pids, &tools, deadline, &mut cancellation).await?;
    let selected_start = pids
        .iter()
        .find(|(pid, _)| *pid == selected_pid)
        .map(|(_, start)| *start)
        .unwrap_or(0);
    if Instant::now() >= deadline {
        return Err("Selected audio capture exceeded its 12-second limit".into());
    }
    validate(status())?;
    if !current() || *cancellation.borrow() {
        return Err("Playback changed before capture".into());
    }
    let mut child = tools
        .command(tools.parec.as_os_str())
        .args([
            "--raw",
            "--format=s16le",
            "--rate=48000",
            "--channels=1",
            "--latency-msec=100",
            "--process-time-msec=50",
            "--device",
            &device,
            "--monitor-stream",
        ])
        .arg(input.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Linux output capture requires pactl and parec (install pulseaudio-utils)")?;
    let Some(mut stdout) = child.stdout.take() else {
        kill_and_reap(&mut child).await;
        return Err("Could not read selected output monitor".into());
    };
    // Pulse may buffer before its first packet. Keep checking ownership while
    // allowing bounded recorder startup; all exits explicitly reap this child.
    let startup_deadline = Instant::now() + tools.startup_timeout;
    let mut pcm = Vec::with_capacity(capture_bytes);
    let mut scratch = vec![0u8; 24000];
    let mut next = Instant::now() + tools.recheck_interval;
    while pcm.len() < capture_bytes {
        if *cancellation.borrow() {
            kill_and_reap(&mut child).await;
            return Err("Audio identification was cancelled".into());
        }
        if Instant::now() >= deadline {
            kill_and_reap(&mut child).await;
            return Err("Selected audio capture exceeded its 12-second limit".into());
        }
        if !current() || validate(status()).is_err() {
            kill_and_reap(&mut child).await;
            return Err("Playback or WebView audio ownership changed during capture".into());
        }
        if Instant::now() >= next {
            let live_pids = owned_webkit_pids(root_pid);
            match live_pids.and_then(|p| {
                if p.iter()
                    .any(|(pid, start)| *pid == selected_pid && *start == selected_start)
                {
                    Ok(p)
                } else {
                    Err("Selected WebKit process ended or its PID was reused".into())
                }
            }) {
                Ok(live_pids) => {
                    match selected(&live_pids, &tools, deadline, &mut cancellation).await {
                        Ok((i, s, pid, _)) if i == input && s == sink && pid == selected_pid => {}
                        Ok(_) => {
                            kill_and_reap(&mut child).await;
                            return Err("Owned WebKit audio stream changed during capture".into());
                        }
                        Err(error) => {
                            kill_and_reap(&mut child).await;
                            return Err(error);
                        }
                    }
                }
                Err(error) => {
                    kill_and_reap(&mut child).await;
                    return Err(error);
                }
            }
            next = Instant::now() + tools.recheck_interval;
        }

        let n = (capture_bytes - pcm.len()).min(scratch.len());
        let deadline_remaining = deadline.saturating_duration_since(Instant::now());
        let startup_remaining = startup_deadline.saturating_duration_since(Instant::now());
        let mut read_timeout = tools.read_timeout.min(deadline_remaining);
        if pcm.is_empty() {
            read_timeout = read_timeout.min(startup_remaining);
        }
        if read_timeout.is_zero() {
            kill_and_reap(&mut child).await;
            return Err(if Instant::now() >= deadline {
                "Selected audio capture exceeded its 12-second limit".into()
            } else {
                "Selected stream stopped producing audio".into()
            });
        }
        let read = tokio::select! {
            biased;
            _ = cancellation.changed() => {
                kill_and_reap(&mut child).await;
                return Err("Audio identification was cancelled".into());
            }
            read = tokio::time::timeout(read_timeout, stdout.read(&mut scratch[..n])) => read,
        };
        let got = match read {
            Ok(Ok(got)) => got,
            Ok(Err(_)) => {
                kill_and_reap(&mut child).await;
                return Err("Could not read selected output monitor".into());
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    kill_and_reap(&mut child).await;
                    return Err("Selected audio capture exceeded its 12-second limit".into());
                }
                if pcm.is_empty() && Instant::now() < startup_deadline {
                    continue;
                }
                kill_and_reap(&mut child).await;
                return Err("Selected stream stopped producing audio".into());
            }
        };
        if got == 0 {
            kill_and_reap(&mut child).await;
            return Err("Selected Burrata output ended before capture completed".into());
        }
        pcm.extend_from_slice(&scratch[..got]);
    }
    kill_and_reap(&mut child).await;
    if !current() {
        return Err("Playback changed after capture".into());
    }
    validate(status())?;
    if pcm.iter().all(|byte| *byte == 0) {
        return Err("Captured Burrata output is silent. Check the app volume and selected output, then try again; external audio will not be substituted.".into());
    }
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    async fn owned_webkit_child() -> (tokio::process::Child, u32) {
        let mut child = Command::new("python3")
            .args([
                "-c",
                "import ctypes,time; ctypes.CDLL(None).prctl(15,b'WebKitWebProces',0,0,0); time.sleep(60)",
            ])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !owned_webkit_pids(std::process::id())
                .is_ok_and(|pids| pids.iter().any(|(owned, _)| *owned == pid))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("test child must appear as an owned WebKit process");
        (child, pid)
    }

    fn fake_tools(directory: &std::path::Path, pid: u32, mode: &str) -> CaptureTools {
        let pactl = directory.join("pactl-fake");
        let parec = directory.join("parec-fake");
        std::fs::write(
            &pactl,
            "#!/usr/bin/env python3\nimport json,os,sys,time\nif os.environ.get('TEST_PACTL_MODE')=='hold':\n open(os.environ['TEST_PACTL_PID_PATH'],'w').write(str(os.getpid()))\n time.sleep(60)\nelif sys.argv[-1]=='sink-inputs': print(json.dumps([{'index':8,'sink':4,'properties':{'application.process.id':os.environ['TEST_WEBKIT_PID']}}]))\nelse: print(json.dumps([{'index':4,'monitor_source':'burrata-test.monitor'}]))\n",
        )
        .unwrap();
        std::fs::write(
            &parec,
            "#!/usr/bin/env python3\nimport json,os,sys,time\nopen(os.environ['TEST_ARGS_PATH'],'w').write(json.dumps(sys.argv[1:]))\nopen(os.environ['TEST_PAREC_PID_PATH'],'w').write(str(os.getpid()))\nmode=os.environ['TEST_PAREC_MODE']\nif mode=='pcm': sys.stdout.buffer.write(bytes([1,0])*384000); sys.stdout.buffer.flush()\nelif mode=='delayed':\n time.sleep(1.0)\n sys.stdout.buffer.write(bytes([1,0])*384000); sys.stdout.buffer.flush()\nelif mode=='eof': pass\nelif mode=='cancel':\n deadline=time.monotonic()+5\n while not os.path.exists(os.environ['TEST_RELEASE_PATH']) and time.monotonic()<deadline: time.sleep(0.005)\n sys.stdout.buffer.write(bytes([1])); sys.stdout.buffer.flush(); time.sleep(60)\nelse: time.sleep(60)\n",
        )
        .unwrap();
        for path in [&pactl, &parec] {
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(path, permissions).unwrap();
        }
        CaptureTools {
            capture_seconds: 8,
            pactl,
            parec,
            env: vec![
                ("TEST_WEBKIT_PID".into(), pid.to_string().into()),
                (
                    "TEST_ARGS_PATH".into(),
                    directory.join("parec-args.json").into_os_string(),
                ),
                (
                    "TEST_PAREC_PID_PATH".into(),
                    directory.join("parec-child.pid").into_os_string(),
                ),
                (
                    "TEST_PACTL_PID_PATH".into(),
                    directory.join("pactl-child.pid").into_os_string(),
                ),
                (
                    "TEST_RELEASE_PATH".into(),
                    directory.join("release-recorder.txt").into_os_string(),
                ),
                ("TEST_PAREC_MODE".into(), mode.into()),
                ("TEST_PACTL_MODE".into(), "json".into()),
            ],
            recheck_interval: Duration::from_millis(10),
            // Leave room for Python startup and scheduling under the full native suite;
            // timeout tests set this deliberately shorter.
            read_timeout: Duration::from_secs(2),
            startup_timeout: Duration::from_secs(2),
        }
    }

    fn hold_pactl(tools: &mut CaptureTools) {
        if let Some((_, value)) = tools
            .env
            .iter_mut()
            .find(|(key, _)| key.to_string_lossy() == "TEST_PACTL_MODE")
        {
            *value = "hold".into();
        }
    }

    async fn wait_for_pid(path: &std::path::Path) -> u32 {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(path) {
                    if let Ok(pid) = text.parse() {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake subprocess must start")
    }

    async fn wait_until_reaped(pid: u32) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while std::path::Path::new(&format!("/proc/{pid}")).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake capture process must be reaped");
    }

    fn assert_no_wav_files(directory: &std::path::Path) {
        assert!(std::fs::read_dir(directory).unwrap().all(|entry| entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|ext| ext != "wav")));
    }
    #[test]
    fn fails_closed_on_webview_audio_ambiguity() {
        assert!(validate(ViewAudio {
            main: true,
            others: 0,
            known: true
        })
        .is_ok());
        assert!(validate(ViewAudio {
            main: false,
            others: 0,
            known: true
        })
        .is_err());
        assert!(validate(ViewAudio {
            main: true,
            others: 1,
            known: true
        })
        .is_err());
        assert!(validate(ViewAudio {
            main: true,
            others: 0,
            known: false
        })
        .is_err());
    }

    #[tokio::test]
    async fn pulse_inspection_cancel_kills_and_reaps_child_before_returning() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let mut tools = fake_tools(dir.path(), webkit_pid, "pcm");
        hold_pactl(&mut tools);
        let (cancel_tx, cancellation) = watch::channel(false);
        let task = tokio::spawn(async move {
            capture_wav_with_tools_cancellable(
                std::process::id(),
                Duration::from_secs(5),
                || ViewAudio {
                    main: true,
                    others: 0,
                    known: true,
                },
                || true,
                cancellation,
                tools,
            )
            .await
        });
        let pactl_pid = wait_for_pid(&dir.path().join("pactl-child.pid")).await;
        cancel_tx.send_replace(true);
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            "Audio identification was cancelled"
        );
        assert!(!std::path::Path::new(&format!("/proc/{pactl_pid}")).exists());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn shared_capture_deadline_kills_and_reaps_pulse_inspection_child() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let mut tools = fake_tools(dir.path(), webkit_pid, "pcm");
        hold_pactl(&mut tools);
        let result = tokio::spawn(async move {
            capture_wav_with_tools_cancellable(
                std::process::id(),
                Duration::from_millis(750),
                || ViewAudio {
                    main: true,
                    others: 0,
                    known: true,
                },
                || true,
                watch::channel(false).1,
                tools,
            )
            .await
        });
        let pactl_pid = wait_for_pid(&dir.path().join("pactl-child.pid")).await;
        let error = result.await.unwrap().unwrap_err();
        assert!(
            error.contains("12-second limit"),
            "unexpected deadline result: {error}"
        );
        wait_until_reaped(pactl_pid).await;
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }
    #[test]
    fn selects_only_one_app_owned_webkit_stream_and_rejects_unrouted() {
        let p = [(237122, 1)];
        let rows = serde_json::json!([
         {"index":2,"sink":4,"properties":{"application.process.id":"991"}},
         {"index":8,"sink":4,"properties":{"application.process.id":"237122"}},
         {"index":9,"sink":5,"properties":{"application.process.id":"238399"}}
        ]);
        assert_eq!(one_stream(&rows, &p).unwrap(), (8, 4, 237122));
        let ambiguous = serde_json::json!([
         {"index":8,"sink":4,"properties":{"application.process.id":"237122"}},
         {"index":9,"sink":5,"properties":{"application.process.id":"238399"}}
        ]);
        let both = [(237122, 1), (238399, 1)];
        assert!(one_stream(&ambiguous, &both)
            .unwrap_err()
            .contains("Pause audio in the other window"));
        let unrouted = serde_json::json!([{"index":8,"sink":4294967295u64,"properties":{"application.process.id":"237122"}}]);
        assert!(one_stream(&unrouted, &p)
            .unwrap_err()
            .contains("not reached an audio output"));
    }

    #[tokio::test]
    async fn selected_output_can_capture_three_seconds_without_waiting_for_eight() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let mut tools = fake_tools(dir.path(), webkit_pid, "pcm");
        tools.capture_seconds = 3;
        let wav = capture_wav_with_tools(
            std::process::id(),
            Duration::from_secs(3),
            || ViewAudio {
                main: true,
                others: 0,
                known: true,
            },
            || true,
            tools,
        )
        .await
        .unwrap();
        assert_eq!(wav.len(), 44 + RATE as usize * 2 * 3);
        crate::webview_radio_pcm::validate_wav(&wav).unwrap();
        kill_and_reap(&mut webkit).await;
    }

    #[tokio::test]
    async fn selected_output_uses_explicit_monitor_and_stream_and_reaps_child() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let tools = fake_tools(dir.path(), webkit_pid, "pcm");
        let wav = capture_wav_with_tools(
            std::process::id(),
            Duration::from_secs(3),
            || ViewAudio {
                main: true,
                others: 0,
                known: true,
            },
            || true,
            tools,
        )
        .await
        .unwrap();
        assert_eq!(wav.len(), 44 + BYTES);
        let args: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parec-args.json")).unwrap(),
        )
        .unwrap();
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--device" && w[1] == "burrata-test.monitor"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--monitor-stream" && w[1] == "8"));
        assert!(args.iter().any(|arg| arg == "--latency-msec=100"));
        assert!(args.iter().any(|arg| arg == "--process-time-msec=50"));
        let parec_pid: u32 = std::fs::read_to_string(dir.path().join("parec-child.pid"))
            .unwrap()
            .parse()
            .unwrap();
        wait_until_reaped(parec_pid).await;
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn playback_cancel_kills_and_reaps_selected_capture_without_wav() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let tools = fake_tools(dir.path(), webkit_pid, "cancel");
        let cancelled = Arc::new(AtomicBool::new(false));
        let current = cancelled.clone();
        let task = tokio::spawn(async move {
            capture_wav_with_tools(
                std::process::id(),
                Duration::from_secs(4),
                || ViewAudio {
                    main: true,
                    others: 0,
                    known: true,
                },
                move || !current.load(Ordering::SeqCst),
                tools,
            )
            .await
        });
        let parec_pid = wait_for_pid(&dir.path().join("parec-child.pid")).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancelled.store(true, Ordering::SeqCst);
        std::fs::write(dir.path().join("release-recorder.txt"), "release").unwrap();
        let error = task.await.unwrap().unwrap_err();
        assert!(
            error.contains("Playback or WebView audio ownership changed during capture"),
            "unexpected cancellation result: {error}"
        );
        wait_until_reaped(parec_pid).await;
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn explicit_cancel_reaps_recorder_before_capture_returns() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let tools = fake_tools(dir.path(), webkit_pid, "hold");
        let (cancel_tx, cancellation) = watch::channel(false);
        let task = tokio::spawn(async move {
            capture_wav_with_tools_cancellable(
                std::process::id(),
                Duration::from_secs(4),
                || ViewAudio {
                    main: true,
                    others: 0,
                    known: true,
                },
                || true,
                cancellation,
                tools,
            )
            .await
        });
        let parec_pid = wait_for_pid(&dir.path().join("parec-child.pid")).await;
        cancel_tx.send_replace(true);
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error, "Audio identification was cancelled");
        assert!(
            !std::path::Path::new(&format!("/proc/{parec_pid}")).exists(),
            "capture must not return until parec has been reaped"
        );
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn recorder_eof_is_explicitly_reaped_without_wav() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let tools = fake_tools(dir.path(), webkit_pid, "eof");
        let result = capture_wav_with_tools(
            std::process::id(),
            Duration::from_secs(3),
            || ViewAudio {
                main: true,
                others: 0,
                known: true,
            },
            || true,
            tools,
        )
        .await;
        let error = result.unwrap_err();
        assert!(
            error.contains("ended before capture completed"),
            "unexpected EOF result: {error}"
        );
        let parec_pid = wait_for_pid(&dir.path().join("parec-child.pid")).await;
        wait_until_reaped(parec_pid).await;
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn delayed_first_packet_is_not_reported_as_stopped_playback() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let mut tools = fake_tools(dir.path(), webkit_pid, "delayed");
        tools.read_timeout = Duration::from_millis(100);
        let wav = capture_wav_with_tools(
            std::process::id(),
            Duration::from_secs(5),
            || ViewAudio {
                main: true,
                others: 0,
                known: true,
            },
            || true,
            tools,
        )
        .await
        .unwrap();
        assert_eq!(wav.len(), 44 + BYTES);
        let pid = wait_for_pid(&dir.path().join("parec-child.pid")).await;
        wait_until_reaped(pid).await;
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn recorder_read_timeout_is_explicitly_reaped_without_wav() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let mut tools = fake_tools(dir.path(), webkit_pid, "hold");
        tools.read_timeout = Duration::from_millis(500);
        tools.startup_timeout = Duration::from_millis(500);
        let result = capture_wav_with_tools(
            std::process::id(),
            Duration::from_secs(3),
            || ViewAudio {
                main: true,
                others: 0,
                known: true,
            },
            || true,
            tools,
        )
        .await;
        assert!(result.unwrap_err().contains("stopped producing audio"));
        let parec_pid = wait_for_pid(&dir.path().join("parec-child.pid")).await;
        wait_until_reaped(parec_pid).await;
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }

    #[tokio::test]
    async fn capture_deadline_kills_and_reaps_selected_child() {
        let dir = tempfile::tempdir().unwrap();
        let (mut webkit, webkit_pid) = owned_webkit_child().await;
        let mut tools = fake_tools(dir.path(), webkit_pid, "hold");
        tools.read_timeout = Duration::from_secs(2);
        let task = tokio::spawn(async move {
            capture_wav_with_tools(
                std::process::id(),
                Duration::from_secs(1),
                || ViewAudio {
                    main: true,
                    others: 0,
                    known: true,
                },
                || true,
                tools,
            )
            .await
        });
        let parec_pid = wait_for_pid(&dir.path().join("parec-child.pid")).await;
        assert!(task.await.unwrap().is_err());
        wait_until_reaped(parec_pid).await;
        assert_no_wav_files(dir.path());
        let _ = webkit.kill().await;
        let _ = webkit.wait().await;
    }
}
