//! Runtime capability check for a configured FFmpeg's Ogg Opus gain behavior.
//!
//! This is intentionally selected-source-only. Unknown binaries cannot unlock
//! R128 normalization, and the result is scoped to the exact executable file.

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::watch,
    time::timeout,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(4);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_STDOUT_BYTES: usize = 512 * 1024;
const MAX_STDERR_BYTES: usize = 4 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const CAPABILITY_PROBE_SCHEMA_VERSION: u32 = 1;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;

const FIXTURE_ZERO: &[u8] = include_bytes!("../fixtures/opus-zero.ogg");
const FIXTURE_PLUS_6_DB: &[u8] = include_bytes!("../fixtures/opus-plus6.ogg");
const FIXTURE_R128_COMMENT: &[u8] = include_bytes!("../fixtures/opus-r128-comment.ogg");

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExecutableIdentity {
    canonical_path: PathBuf,
    length: u64,
    modified_nanos: u128,
    created_nanos: u128,
    sha256: String,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(windows)]
    file_attributes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capability {
    Pending,
    Supported,
    Unsupported,
}

fn releases_normalization(capability: Option<Capability>) -> bool {
    capability == Some(Capability::Supported)
}

static CAPABILITIES: OnceLock<Mutex<HashMap<ExecutableIdentity, Capability>>> = OnceLock::new();

fn capabilities() -> &'static Mutex<HashMap<ExecutableIdentity, Capability>> {
    CAPABILITIES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn resolve_executable(path: &Path) -> Option<PathBuf> {
    if path.components().count() > 1 || path.is_absolute() {
        return is_runnable_file(path).then(|| path.to_owned());
    }
    let path_value = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_value) {
        let candidate = directory.join(path);
        if is_runnable_file(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        if candidate.extension().is_none() {
            for extension in ["exe", "com", "bat", "cmd"] {
                let candidate = candidate.with_extension(extension);
                if is_runnable_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn is_runnable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn same_binary_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    if left.len() != right.len()
        || left.modified().ok() != right.modified().ok()
        || left.created().ok() != right.created().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino() && left.mode() == right.mode()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        left.file_attributes() == right.file_attributes()
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

fn executable_identity_sync(path: &Path) -> Option<ExecutableIdentity> {
    let resolved_path = resolve_executable(&path)?;
    let canonical_path = std::fs::canonicalize(&resolved_path).ok()?;
    let mut file = std::fs::File::open(&canonical_path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_EXECUTABLE_BYTES {
        return None;
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes_read = 0u64;
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read.checked_add(count as u64)?;
        if bytes_read > MAX_EXECUTABLE_BYTES {
            return None;
        }
        digest.update(&buffer[..count]);
    }
    let after = file.metadata().ok()?;
    let path_after = std::fs::metadata(&canonical_path).ok()?;
    if bytes_read != metadata.len()
        || !same_binary_metadata(&metadata, &after)
        || !same_binary_metadata(&metadata, &path_after)
    {
        return None;
    }
    let sha256 = format!("{:x}", digest.finalize());
    let modified_nanos = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let created_nanos = metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |created| created.as_nanos());
    Some(ExecutableIdentity {
        canonical_path,
        length: metadata.len(),
        modified_nanos,
        created_nanos,
        sha256,
        #[cfg(unix)]
        device: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
        #[cfg(unix)]
        mode: {
            use std::os::unix::fs::MetadataExt;
            metadata.mode()
        },
        #[cfg(windows)]
        file_attributes: {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes()
        },
    })
}

async fn executable_identity(path: PathBuf) -> Option<ExecutableIdentity> {
    tokio::task::spawn_blocking(move || executable_identity_sync(&path))
        .await
        .ok()
        .flatten()
}

/// Rehashes the exact path at decoder launch, after policy planning, so a
/// same-path replacement cannot keep using the stale capability fingerprint.
pub(crate) fn executable_matches_fingerprint(path: &Path, fingerprint: &str) -> bool {
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    executable_identity_sync(path)
        .is_some_and(|identity| identity.sha256.eq_ignore_ascii_case(fingerprint))
}

fn configure_probe_process_group(command: &mut Command) {
    crate::tools::configure_no_console_tokio(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
}

struct ProcessFamilyGuard {
    pid: Option<u32>,
    armed: bool,
    #[cfg(windows)]
    job: Option<windows_sys::Win32::Foundation::HANDLE>,
}

impl ProcessFamilyGuard {
    fn new(pid: Option<u32>) -> Self {
        Self {
            pid,
            armed: true,
            #[cfg(windows)]
            job: None,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn terminate(&mut self) {
        if !self.armed {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        #[cfg(windows)]
        if let Some(job) = self.job {
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
            }
        }
    }
}

impl Drop for ProcessFamilyGuard {
    fn drop(&mut self) {
        self.terminate();
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE terminates every descendant
            // still in the job, even when its original parent already exited.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        }
    }
}

#[cfg(windows)]
fn create_kill_on_close_job() -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use std::mem::size_of;
    use std::ptr::null;
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return None;
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } != 0;
    if !configured {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        return None;
    }
    Some(job)
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process_handle: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

/// Create a process in a kill-on-close job before it can run. Windows starts
/// it suspended, assigns it to the job, then resumes; any failure aborts it.
async fn spawn_probe_child(
    command: &mut Command,
    stdin: std::process::Stdio,
) -> Option<(Child, ProcessFamilyGuard)> {
    configure_probe_process_group(command);
    #[cfg(unix)]
    {
        let child = command
            .kill_on_drop(true)
            .stdin(stdin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let family = ProcessFamilyGuard::new(child.id());
        Some((child, family))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
            PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE,
        };
        let job = create_kill_on_close_job()?;
        command
            .as_std_mut()
            .creation_flags(CREATE_SUSPENDED | 0x08000000); // CREATE_NO_WINDOW
        let mut child = match command
            .kill_on_drop(true)
            .stdin(stdin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
                return None;
            }
        };
        let mut family = ProcessFamilyGuard::new(child.id());
        family.job = Some(job);
        let Some(pid) = child.id() else {
            family.terminate();
            let _ = child.start_kill();
            let _ = timeout(CLEANUP_TIMEOUT, child.wait()).await;
            return None;
        };
        let process = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA
                    | PROCESS_TERMINATE
                    | PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_SUSPEND_RESUME,
                0,
                pid,
            )
        };
        if process.is_null() {
            family.terminate();
            let _ = child.start_kill();
            let _ = timeout(CLEANUP_TIMEOUT, child.wait()).await;
            return None;
        }
        let assigned = unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(job, process)
        } != 0;
        let resumed = assigned && unsafe { NtResumeProcess(process) } >= 0;
        unsafe { windows_sys::Win32::Foundation::CloseHandle(process) };
        if !resumed {
            family.terminate();
            let _ = child.start_kill();
            let _ = timeout(CLEANUP_TIMEOUT, child.wait()).await;
            return None;
        }
        Some((child, family))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let child = command
            .kill_on_drop(true)
            .stdin(stdin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        Some((child, ProcessFamilyGuard::new(child.id())))
    }
}

async fn kill_and_reap_family(child: &mut Child, process_family: &mut ProcessFamilyGuard) {
    process_family.terminate();
    let _ = child.start_kill();
    let _ = timeout(CLEANUP_TIMEOUT, child.wait()).await;
}

/// Checks a capability only for the configured executable used by this
/// selected local source. A missing identity or pending/unsupported probe is
/// always fail-closed. Unknown capability starts one detached bounded probe.
pub(crate) async fn bounded_command_output(
    command: &mut Command,
    stdout_limit: usize,
    stderr_limit: usize,
    timeout_duration: Duration,
) -> Option<std::process::Output> {
    let (mut child, mut process_family) =
        spawn_probe_child(command, std::process::Stdio::null()).await?;
    let operation = async {
        let stdout = child.stdout.take()?;
        let stderr = child.stderr.take()?;
        let (stdout, stderr) = tokio::join!(
            read_limited(stdout, stdout_limit),
            read_limited(stderr, stderr_limit),
        );
        let stdout = stdout.ok()?;
        let stderr = stderr.ok()?;
        if stdout.len() > stdout_limit || stderr.len() > stderr_limit {
            return None;
        }
        let status = child.wait().await.ok()?;
        Some(std::process::Output {
            status,
            stdout,
            stderr,
        })
    };
    match timeout(timeout_duration, operation).await {
        Ok(Some(output)) => {
            process_family.terminate();
            process_family.disarm();
            Some(output)
        }
        _ => {
            kill_and_reap_family(&mut child, &mut process_family).await;
            process_family.disarm();
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualifiedDecoder {
    pub executable_path: PathBuf,
    pub executable_sha256: String,
    pub probe_schema_version: u32,
}

fn qualified(identity: &ExecutableIdentity) -> QualifiedDecoder {
    QualifiedDecoder {
        executable_path: identity.canonical_path.clone(),
        executable_sha256: identity.sha256.clone(),
        probe_schema_version: CAPABILITY_PROBE_SCHEMA_VERSION,
    }
}

fn ensure_probe_started(identity: ExecutableIdentity) {
    let start = {
        let mut cache = capabilities()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cache.contains_key(&identity) {
            false
        } else {
            cache.retain(|known, _| known.canonical_path != identity.canonical_path);
            if cache.len() >= 16 {
                if let Some(oldest_key) = cache.keys().next().cloned() {
                    cache.remove(&oldest_key);
                }
            }
            cache.insert(identity.clone(), Capability::Pending);
            true
        }
    };
    if !start {
        return;
    }
    let identity_for_probe = identity.clone();
    let path = identity.canonical_path.clone();
    tokio::spawn(async move {
        let supported = run_probe(&path).await;
        let current_identity = executable_identity(path).await;
        let result = match (supported, current_identity) {
            (true, Some(current)) if current == identity_for_probe => Capability::Supported,
            (false, Some(current)) if current == identity_for_probe => Capability::Unsupported,
            _ => return,
        };
        let mut cache = capabilities()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cache.get(&identity_for_probe) == Some(&Capability::Pending) {
            cache.insert(identity_for_probe, result);
        }
    });
}

/// Probe the final local file with a caller-configured ffprobe. This only
/// identifies codec/container; it never trusts source metadata or tag text.
pub(crate) async fn probe_ogg_opus(
    ffprobe: &Path,
    input: &Path,
    cancel: &mut watch::Receiver<bool>,
) -> Option<bool> {
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name:format=format_name",
            "-of",
            "json",
        ])
        .arg(input);
    let output = tokio::select! {
        biased;
        _ = cancel.wait_for(|cancelled| *cancelled) => return None,
        output = bounded_command_output(
            &mut command,
            4096,
            4096,
            Duration::from_secs(2),
        ) => output?,
    };
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let codec = value["streams"]
        .as_array()?
        .first()?
        .get("codec_name")?
        .as_str()?;
    let container = value["format"].get("format_name")?.as_str()?;
    Some(
        codec.eq_ignore_ascii_case("opus")
            && container
                .split(',')
                .any(|name| name.eq_ignore_ascii_case("ogg")),
    )
}

pub(crate) async fn confirmed_executable(executable: &Path) -> Option<QualifiedDecoder> {
    let identity = executable_identity(executable.to_owned()).await?;
    ensure_probe_started(identity.clone());
    let cache = capabilities()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    releases_normalization(cache.get(&identity).copied()).then(|| qualified(&identity))
}

/// Explicit analysis may wait for the same bounded background test, but it
/// remains cancellable and never records an unqualified Opus analysis.
pub(crate) async fn wait_for_qualified_decoder(
    executable: &Path,
    cancel: &mut watch::Receiver<bool>,
) -> Option<QualifiedDecoder> {
    let identity = executable_identity(executable.to_owned()).await?;
    ensure_probe_started(identity.clone());
    let deadline = Instant::now() + PROBE_TIMEOUT + CLEANUP_TIMEOUT + Duration::from_secs(1);
    loop {
        if *cancel.borrow() {
            return None;
        }
        let capability = capabilities()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&identity)
            .copied();
        match capability {
            Some(Capability::Supported) => {
                let current = executable_identity(identity.canonical_path.clone()).await?;
                return (current == identity).then(|| qualified(&identity));
            }
            Some(Capability::Unsupported) => return None,
            Some(Capability::Pending) | None => {}
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return None;
                }
            }
        }
    }
}

async fn run_probe(executable: &Path) -> bool {
    if !fixtures_are_valid() {
        return false;
    }
    let Ok((zero, plus_six, tagged)) = timeout(PROBE_TIMEOUT, async {
        tokio::join!(
            decode_fixture(executable, FIXTURE_ZERO),
            decode_fixture(executable, FIXTURE_PLUS_6_DB),
            decode_fixture(executable, FIXTURE_R128_COMMENT),
        )
    })
    .await
    else {
        return false;
    };
    match (zero, plus_six, tagged) {
        (Ok(zero), Ok(plus_six), Ok(tagged)) => behaviors_match(&zero, &plus_six, &tagged),
        _ => false,
    }
}

async fn decode_fixture(executable: &Path, fixture: &'static [u8]) -> Result<Vec<f32>, ()> {
    let mut command = Command::new(executable);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        "pipe:0",
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-f",
        "f32le",
        "-ac",
        "2",
        "-ar",
        "48000",
        "pipe:1",
    ]);
    // Capability fixtures are tiny and static; use the common bounded child
    // launcher so Windows assigns the decoder process to its Job before run.
    let (mut child, mut process_family) =
        spawn_probe_child(&mut command, std::process::Stdio::piped())
            .await
            .ok_or(())?;

    let operation = async {
        let mut stdin = child.stdin.take().ok_or(())?;
        let stdout = child.stdout.take().ok_or(())?;
        let stderr = child.stderr.take().ok_or(())?;
        let write_fixture = async move {
            let mut stdin = stdin;
            stdin.write_all(fixture).await.map_err(|_| ())?;
            drop(stdin);
            Ok::<(), ()>(())
        };
        let (write_result, stdout, stderr) = tokio::join!(
            write_fixture,
            read_limited(stdout, MAX_STDOUT_BYTES),
            read_limited(stderr, MAX_STDERR_BYTES),
        );
        write_result?;
        let stdout = stdout.map_err(|_| ())?;
        let stderr = stderr.map_err(|_| ())?;
        if stdout.len() > MAX_STDOUT_BYTES || stderr.len() > MAX_STDERR_BYTES {
            return Err(());
        }
        let status = child.wait().await.map_err(|_| ())?;
        if !status.success() || stdout.len() % std::mem::size_of::<f32>() != 0 {
            return Err(());
        }
        let samples = stdout
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>();
        validate_samples(&samples).then_some(samples).ok_or(())
    };

    match timeout(FIXTURE_TIMEOUT, operation).await {
        Ok(Ok(samples)) => {
            process_family.terminate();
            process_family.disarm();
            Ok(samples)
        }
        _ => {
            kill_and_reap_family(&mut child, &mut process_family).await;
            process_family.disarm();
            Err(())
        }
    }
}

async fn read_limited<R: AsyncRead + Unpin>(reader: R, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut output)
        .await?;
    Ok(output)
}

fn fixtures_are_valid() -> bool {
    let Some(zero_pages) = parse_ogg_pages(FIXTURE_ZERO) else {
        return false;
    };
    let Some(plus_pages) = parse_ogg_pages(FIXTURE_PLUS_6_DB) else {
        return false;
    };
    let Some(tag_pages) = parse_ogg_pages(FIXTURE_R128_COMMENT) else {
        return false;
    };
    if zero_pages.len() < 3
        || plus_pages.len() != zero_pages.len()
        || tag_pages.len() != zero_pages.len()
    {
        return false;
    }
    let zero_head = zero_pages[0].payload;
    let plus_head = plus_pages[0].payload;
    if zero_head.len() != 19
        || plus_head.len() != 19
        || !zero_head.starts_with(b"OpusHead")
        || !plus_head.starts_with(b"OpusHead")
        || zero_head[..16] != plus_head[..16]
        || zero_head[18..] != plus_head[18..]
        || i16::from_le_bytes([zero_head[16], zero_head[17]]) != 0
        || i16::from_le_bytes([plus_head[16], plus_head[17]]) != 1536
    {
        return false;
    }
    let zero_tags = zero_pages[1].payload;
    let plus_tags = plus_pages[1].payload;
    let tagged_tags = tag_pages[1].payload;
    if !zero_tags.starts_with(b"OpusTags")
        || zero_tags != plus_tags
        || !plus_tags.starts_with(b"OpusTags")
        || !tagged_tags
            .windows(b"R128_TRACK_GAIN=1536".len())
            .any(|window| window == b"R128_TRACK_GAIN=1536")
    {
        return false;
    }
    let zero_audio = zero_pages[2..]
        .iter()
        .map(|page| page.full)
        .collect::<Vec<_>>();
    let plus_audio = plus_pages[2..]
        .iter()
        .map(|page| page.full)
        .collect::<Vec<_>>();
    let tag_audio = tag_pages[2..]
        .iter()
        .map(|page| page.full)
        .collect::<Vec<_>>();
    zero_audio == plus_audio && zero_audio == tag_audio
}

struct OggPage<'a> {
    full: &'a [u8],
    payload: &'a [u8],
}

fn parse_ogg_pages(input: &[u8]) -> Option<Vec<OggPage<'_>>> {
    let mut pages = Vec::new();
    let mut offset = 0usize;
    while offset < input.len() {
        let header = input.get(offset..offset.checked_add(27)?)?;
        if &header[..4] != b"OggS" || header[4] != 0 {
            return None;
        }
        let segment_count = usize::from(header[26]);
        let lacing = input.get(offset + 27..offset + 27 + segment_count)?;
        let payload_len = lacing.iter().map(|lace| usize::from(*lace)).sum::<usize>();
        let page_end = offset.checked_add(27 + segment_count + payload_len)?;
        let full = input.get(offset..page_end)?;
        let mut checksum_page = full.to_vec();
        let stored_checksum = u32::from_le_bytes(checksum_page[22..26].try_into().ok()?);
        checksum_page[22..26].fill(0);
        if ogg_crc(&checksum_page) != stored_checksum {
            return None;
        }
        let payload_start = offset + 27 + segment_count;
        pages.push(OggPage {
            full,
            payload: input.get(payload_start..page_end)?,
        });
        offset = page_end;
    }
    (offset == input.len()).then_some(pages)
}

fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for byte in bytes {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn validate_samples(samples: &[f32]) -> bool {
    let frame_count = samples.len() / CHANNELS;
    frame_count >= (SAMPLE_RATE as usize / 10)
        && frame_count <= (SAMPLE_RATE as usize * 2)
        && samples.iter().all(|sample| sample.is_finite())
}

fn behaviors_match(zero: &[f32], plus_six: &[f32], tagged: &[f32]) -> bool {
    let Some(zero_rms) = steady_rms(zero) else {
        return false;
    };
    let Some(plus_six_rms) = steady_rms(plus_six) else {
        return false;
    };
    let Some(tagged_rms) = steady_rms(tagged) else {
        return false;
    };
    let header_gain_ratio = plus_six_rms / zero_rms;
    let tag_only_ratio = tagged_rms / zero_rms;
    (1.92..=2.07).contains(&header_gain_ratio) && (0.97..=1.03).contains(&tag_only_ratio)
}

fn steady_rms(samples: &[f32]) -> Option<f64> {
    let frames = samples.len() / CHANNELS;
    if frames < 10 {
        return None;
    }
    let start = frames / 10;
    let end = frames - start;
    let selected = &samples[start * CHANNELS..end * CHANNELS];
    let peak = selected
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    if !peak.is_finite() || peak <= 0.0 || peak > 1.5 {
        return None;
    }
    let mean_square = selected
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>()
        / selected.len() as f64;
    (mean_square.is_finite() && mean_square > 0.0).then(|| mean_square.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_synthetic_fixture_set_proves_expected_pcm_relationships() {
        assert!(fixtures_are_valid());
        // This tests the fixture expectations with observed reference PCM
        // values from the fixture-generation manifest, independent of a host
        // FFmpeg binary. The live probe uses the same acceptance windows.
        assert!(FIXTURE_ZERO.len() < 8 * 1024);
        assert!(FIXTURE_PLUS_6_DB.len() < 8 * 1024);
        assert!(FIXTURE_R128_COMMENT.len() < 8 * 1024);
        let zero = [0.06163605294453309f32; 2_000];
        let plus_six = [0.12298009059963344f32; 2_000];
        let tagged = [0.06163605294453309f32; 2_000];
        assert!(behaviors_match(&zero, &plus_six, &tagged));
        assert!(!behaviors_match(&zero, &zero, &tagged));
        assert!(!behaviors_match(&zero, &plus_six, &plus_six));
    }

    #[test]
    fn executable_identity_must_be_supported_before_policy_is_released() {
        let mut cache = HashMap::new();
        let identity = ExecutableIdentity {
            canonical_path: PathBuf::from("/configured/ffmpeg"),
            length: 100,
            modified_nanos: 1,
            created_nanos: 1,
            sha256: "0".repeat(64),
            #[cfg(unix)]
            device: 1,
            #[cfg(unix)]
            inode: 2,
            #[cfg(unix)]
            mode: 0o755,
            #[cfg(windows)]
            file_attributes: 0,
        };
        assert!(!releases_normalization(None));
        assert!(!releases_normalization(cache.get(&identity).copied()));
        cache.insert(identity.clone(), Capability::Pending);
        assert!(!releases_normalization(cache.get(&identity).copied()));
        cache.insert(identity.clone(), Capability::Unsupported);
        assert!(!releases_normalization(cache.get(&identity).copied()));
        cache.insert(identity.clone(), Capability::Supported);
        assert!(releases_normalization(cache.get(&identity).copied()));
        let replaced = ExecutableIdentity {
            length: 101,
            ..identity.clone()
        };
        assert!(!releases_normalization(cache.get(&replaced).copied()));
        let same_metadata_replacement = ExecutableIdentity {
            sha256: "f".repeat(64),
            ..identity.clone()
        };
        assert!(!releases_normalization(
            cache.get(&same_metadata_replacement).copied()
        ));
    }

    #[tokio::test]
    async fn missing_configured_executable_cannot_release_normalization() {
        assert!(confirmed_executable(Path::new(
            "/no/such/burrata-ffmpeg-capability-fixture-7f4d9c2a"
        ))
        .await
        .is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_child_timeout_kills_and_reaps_the_probe_process() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 5"]);
        let started = std::time::Instant::now();
        assert!(bounded_command_output(
            &mut command,
            MAX_STDOUT_BYTES,
            MAX_STDERR_BYTES,
            Duration::from_millis(50),
        )
        .await
        .is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_child_timeout_kills_wrapper_descendants() {
        let pid_path = std::env::temp_dir().join(format!(
            "burrata-opus-probe-child-{}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let pid_path_text = pid_path.to_string_lossy().into_owned();
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                r#"sleep 30 & child=$!; printf '%s' "$child" > "$1"; wait"#,
                "burrata-probe-wrapper",
            ])
            .arg(&pid_path_text);
        let result = bounded_command_output(
            &mut command,
            MAX_STDOUT_BYTES,
            MAX_STDERR_BYTES,
            Duration::from_millis(250),
        )
        .await;
        assert!(result.is_none());
        let child_pid = std::fs::read_to_string(&pid_path)
            .expect("wrapper started its child")
            .parse::<i32>()
            .expect("child pid is numeric");
        let _ = std::fs::remove_file(&pid_path);
        for _ in 0..20 {
            let proc_stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat"));
            match proc_stat {
                Err(_) => return,
                Ok(stat) => {
                    // A killed orphan may briefly remain as a zombie until init
                    // reaps it; it cannot continue executing or hold resources.
                    let state = stat.split_whitespace().nth(2).unwrap_or("?");
                    if state == "Z" {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("wrapper descendant remained alive after timeout");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_job_kills_grandchild_after_wrapper_exits_with_pipe_open() {
        let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let mut command = Command::new(shell);
        command.args([
            "/C",
            r#"start "" /B cmd.exe /C "ping -n 30 127.0.0.1 >NUL 2>&1 & echo descendant-output""#,
        ]);
        let started = Instant::now();
        assert!(bounded_command_output(
            &mut command,
            MAX_STDOUT_BYTES,
            MAX_STDERR_BYTES,
            Duration::from_millis(250),
        )
        .await
        .is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_child_rejects_oversized_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 1234567890"]);
        assert!(
            bounded_command_output(&mut command, 4, MAX_STDERR_BYTES, Duration::from_secs(1),)
                .await
                .is_none()
        );
    }

    #[test]
    fn output_shape_and_rms_rejects_silent_or_malformed_decoders() {
        assert!(!validate_samples(&[]));
        assert!(!validate_samples(&[f32::NAN; 10_000]));
        assert!(!validate_samples(&[f32::INFINITY; 10_000]));
        assert!(steady_rms(&[0.0; 10_000]).is_none());
    }
}
