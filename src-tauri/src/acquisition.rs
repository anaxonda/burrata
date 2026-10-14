//! Subprocess acquisition stays in Rust; only explicit queue requests download media.
use crate::{connections::ConnectionManager, matching, media, models::*, Store};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::Emitter;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, watch, Semaphore, SemaphorePermit},
};
use uuid::Uuid;

// Peer queues must not consume the workers used for direct downloads or file processing.
const SOULSEEK_PENDING_LIMIT: usize = 32;

struct DownloadAdmission<'a> {
    processing: Option<SemaphorePermit<'a>>,
    peer: Option<SemaphorePermit<'a>>,
}

async fn cancellable_slot<'a>(
    slots: &'a Semaphore,
    cancel: &mut watch::Receiver<bool>,
) -> Result<SemaphorePermit<'a>, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    tokio::select! {
        permit = slots.acquire() => permit.map_err(|_| "Download queue closed".into()),
        _ = cancel.changed() => Err("Cancelled".into()),
    }
}

async fn download_admission<'a>(
    provider: &str,
    processing: &'a Semaphore,
    peers: &'a Semaphore,
    cancel: &mut watch::Receiver<bool>,
) -> Result<DownloadAdmission<'a>, String> {
    if provider == "slskd" {
        Ok(DownloadAdmission {
            processing: None,
            peer: Some(cancellable_slot(peers, cancel).await?),
        })
    } else {
        Ok(DownloadAdmission {
            processing: Some(cancellable_slot(processing, cancel).await?),
            peer: None,
        })
    }
}

impl<'a> DownloadAdmission<'a> {
    async fn begin_processing(
        &mut self,
        processing: &'a Semaphore,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        // slskd enforces its own transfer limit; this permit only bounded pending monitors.
        if self.peer.take().is_some() {
            self.processing = Some(cancellable_slot(processing, cancel).await?);
        }
        Ok(())
    }
}

const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const SEARCH_CACHE_MAX: usize = 128;
const RAW_AUDIO_SEARCH_CACHE_TTL: Duration = Duration::from_secs(2 * 60);
const RAW_AUDIO_SEARCH_CACHE_MAX: usize = 64;
const PREVIEW_SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
const PREVIEW_AUDIO_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const PREVIEW_AUDIO_CACHE_MAX: usize = 64;
const CHAPTER_SOURCE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const CHAPTER_SOURCE_CACHE_MAX: usize = 16;
const INSPECTION_CACHE_TTL: Duration = Duration::from_secs(90);
const INSPECTION_CACHE_MAX: usize = 128;
const INSPECTION_PROFILE: &str = "acquisition-inspect-v3-youtube-support";
const PREVIEW_AUDIO_FORMAT_SELECTOR: &str = "bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/bestaudio[protocol^=http][protocol!*=dash]/bestaudio";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAudio {
    pub url: String,
    pub mime_type: String,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<f64>,
    pub duration_seconds: Option<f64>,
}

/// Independent acquisition capability; catalog and preview providers need not implement it.
#[async_trait::async_trait]
pub trait AcquisitionProvider: Send + Sync {
    async fn find_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Candidate>, String>;
    async fn inspect_formats(
        &self,
        track: &Track,
        candidate: &Candidate,
        settings: &Settings,
    ) -> Result<Candidate, String>;
    async fn download(
        &self,
        request: DownloadRequest,
        settings: Settings,
    ) -> Result<DownloadJob, String>;
}

fn base_args(settings: &Settings) -> Vec<String> {
    let mut args = vec![
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-remote-components",
        "--no-js-runtimes",
        "--no-color",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if !settings.js_runtime_path.trim().is_empty() {
        let configured = settings
            .js_runtime_path
            .trim()
            .strip_prefix("system:")
            .or_else(|| settings.js_runtime_path.trim().strip_prefix("bundled:"))
            .unwrap_or(settings.js_runtime_path.trim());
        let name = Path::new(configured)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("deno");
        let runtime = match name {
            "node" | "nodejs" => "node",
            "bun" => "bun",
            "qjs" | "quickjs" => "quickjs",
            _ => "deno",
        };
        args.extend([
            "--js-runtimes".into(),
            format!(
                "{runtime}:{}",
                crate::tools::executable(&settings.js_runtime_path, runtime).display()
            ),
        ]);
    }
    args
}

fn uses_managed_ytdlp(configured: &str) -> bool {
    let configured = configured.trim();
    configured.is_empty() || configured == "yt-dlp" || configured.starts_with("bundled:")
}

async fn add_youtube_support_args(args: &mut Vec<String>, provider: &str) -> Result<(), String> {
    if provider == "youtube" && crate::youtube_support::global().is_some() {
        args.extend(
            crate::youtube_support::yt_dlp_provider_args()
                .await?
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
    }
    Ok(())
}

fn ffmpeg_location(configured: &str) -> Option<String> {
    // yt-dlp's --ffmpeg-location checks filesystem existence, unlike Command's PATH lookup.
    // Passing the default bare "ffmpeg" incorrectly disables an installed PATH executable.
    if matches!(configured, "ffmpeg" | "ffmpeg.exe") {
        return None;
    }
    let path = Path::new(configured);
    if path.is_absolute() || path.components().count() > 1 {
        return Some(configured.into());
    }
    if let Some(search) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&search) {
            let candidate = directory.join(configured);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into());
            }
            #[cfg(windows)]
            {
                let candidate = directory.join(format!("{configured}.exe"));
                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().into());
                }
            }
        }
    }
    Some(configured.into())
}

fn is_preserved_source(asset: &LocalAsset, candidate: &Candidate) -> bool {
    let same_source = if candidate.provider == "slskd" {
        asset.track.provenance["acquisition"]["id"].as_str() == Some(candidate.id.as_str())
    } else {
        asset.track.provenance["acquisition"]["url"].as_str() == Some(candidate.url.as_str())
    };
    asset.track.provenance.get("derivedFromAssetId").is_none()
        && asset.track.provenance.get("chapter").is_none()
        && asset.track.provenance["acquisition"]["provider"].as_str()
            == Some(candidate.provider.as_str())
        && same_source
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

pub fn validate_source_url(raw: &str, provider: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| "Invalid source URL".to_string())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
    {
        return Err("Sources must use public HTTPS URLs without credentials".into());
    }
    let host = url.host_str().ok_or("Missing source hostname")?;
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
    {
        return Err("Local and IP-literal source URLs are not supported".into());
    }
    let allowed = match provider {
        "youtube" => host_matches(host, "youtube.com") || host == "youtu.be",
        "soundcloud" => host_matches(host, "soundcloud.com"),
        "bandcamp" => host_matches(host, "bandcamp.com"),
        "direct" => true,
        "monochrome" => host == "tracks.monochrome.st",
        _ => false,
    };
    if !allowed {
        return Err("Source host does not match the acquisition provider".into());
    }
    Ok(url)
}

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            !v.is_private()
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_unspecified()
                && !v.is_multicast()
                && !v.is_broadcast()
                && v.octets()[0] != 0
                && v.octets()[0] < 224
                && !(v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        std::net::IpAddr::V6(v) => {
            !v.is_loopback()
                && !v.is_unspecified()
                && !v.is_multicast()
                && (v.segments()[0] & 0xfe00) != 0xfc00
                && (v.segments()[0] & 0xffc0) != 0xfe80
                && v.to_ipv4_mapped()
                    .map(|x| is_public_ip(x.into()))
                    .unwrap_or(true)
        }
    }
}

async fn public_addresses(url: &url::Url) -> Result<Vec<std::net::SocketAddr>, String> {
    let host = url.host_str().ok_or("Missing hostname")?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, 443)),
    )
    .await
    .map_err(|_| "DNS lookup timed out")?
    .map_err(|e| format!("DNS lookup failed: {e}"))?
    .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|a| !is_public_ip(a.ip())) {
        return Err("Source resolves to a nonpublic address".into());
    }
    Ok(addresses)
}

async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output()
                .await;
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn pump<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stdout: bool,
    tx: mpsc::Sender<(bool, String)>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stdout, line)).await.is_err() {
            break;
        }
    }
}

async fn run_tool<F: FnMut(&str)>(
    settings: &Settings,
    args: Vec<String>,
    mut cancel: watch::Receiver<bool>,
    limit: Duration,
    mut on_line: F,
) -> Result<String, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let yt_dlp = if crate::youtube_support::global().is_some()
        && uses_managed_ytdlp(&settings.yt_dlp_path)
    {
        crate::youtube_support::active_executable()
    } else {
        crate::tools::executable(&settings.yt_dlp_path, "yt-dlp")
    };
    let mut command = Command::new(yt_dlp);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start yt-dlp: {e}"))?;
    let (tx, mut rx) = mpsc::channel(32);
    let out = tokio::spawn(pump(
        child.stdout.take().ok_or("Missing stdout")?,
        true,
        tx.clone(),
    ));
    let err = tokio::spawn(pump(
        child.stderr.take().ok_or("Missing stderr")?,
        false,
        tx,
    ));
    let timeout = tokio::time::sleep(limit);
    tokio::pin!(timeout);
    let mut output = String::new();
    let mut errors = String::new();
    let mut status = None;
    let mut stream_open = true;
    let result = loop {
        tokio::select! {
            _ = &mut timeout => { terminate(&mut child).await; break Err("yt-dlp operation timed out".into()); }
            changed = cancel.changed(), if !cancel.has_changed().is_err() => {
                if changed.is_ok() && *cancel.borrow() { terminate(&mut child).await; break Err("Cancelled".into()); }
            }
            exit = child.wait(), if status.is_none() => {
                match exit {
                    Ok(s) => {
                        status=Some(s);
                        if !stream_open { break if s.success() { Ok(output) } else { Err(format!("yt-dlp failed: {}", errors.trim())) }; }
                    }
                    Err(e)=>break Err(format!("yt-dlp wait failed: {e}"))
                }
            }
            line = rx.recv(), if stream_open => match line {
                Some((stdout,line)) => {
                    on_line(&line);
                    if stdout {
                        if output.len()+line.len() > MAX_OUTPUT { terminate(&mut child).await; break Err("yt-dlp response exceeded size limit".into()); }
                        output.push_str(&line); output.push('\n');
                    } else {
                        errors.push_str(&line); errors.push('\n');
                        if errors.len()>8192 { errors=errors.chars().rev().take(4096).collect::<String>().chars().rev().collect(); }
                    }
                }
                None => {
                    stream_open=false;
                    if let Some(exit)=status { break if exit.success() { Ok(output) } else { Err(format!("yt-dlp failed: {}",errors.trim())) }; }
                }
            }
        }
    };
    out.abort();
    err.abort();
    result
}

fn idle_cancel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Resolve the bounded flat-playlist entries from YouTube's per-video Mix
/// (`RD<video-id>`). It never requests audio formats or downloads media.
pub(crate) async fn fetch_youtube_mix_entries(
    seed_url: &str,
    settings: &Settings,
    recommendation_limit: usize,
) -> Result<Vec<Value>, String> {
    if !(1..=50).contains(&recommendation_limit) {
        return Err("Radio recommendation limit must be between 1 and 50".into());
    }
    let seed_id = youtube_video_id(seed_url).ok_or("Radio seed must be a valid YouTube video")?;
    let mix_url =
        format!("https://www.youtube.com/watch?v={seed_id}&list=RD{seed_id}&start_radio=1");
    let mut args = base_args(settings);
    add_youtube_support_args(&mut args, "youtube").await?;
    args.extend([
        "--flat-playlist".into(),
        "--dump-single-json".into(),
        "--skip-download".into(),
        "--playlist-end".into(),
        (recommendation_limit + 1).to_string(),
        "--".into(),
        mix_url,
    ]);
    let (_, cancel) = idle_cancel();
    let raw = run_tool(settings, args, cancel, Duration::from_secs(35), |_| {}).await?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("yt-dlp returned invalid YouTube Mix metadata: {e}"))?;
    let entries = value["entries"]
        .as_array()
        .ok_or("yt-dlp did not return YouTube Mix playlist entries")?;
    Ok(entries
        .iter()
        .take(recommendation_limit.saturating_add(1))
        .cloned()
        .collect())
}

pub(crate) fn youtube_video_id(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.scheme(), "http" | "https")
    {
        return None;
    }
    let host = url.host_str()?;
    let id = if matches!(
        host,
        "youtube.com" | "www.youtube.com" | "music.youtube.com"
    ) {
        url.query_pairs()
            .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))?
    } else if matches!(host, "youtu.be" | "www.youtu.be") {
        url.path_segments()?.next()?.to_owned()
    } else {
        return None;
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return None;
    }
    Some(id)
}

fn candidate_from_json(info: &Value, provider: &str) -> Candidate {
    let id = info["id"].as_str().unwrap_or_default();
    let url = info["webpage_url"]
        .as_str()
        .or_else(|| info["original_url"].as_str())
        .or_else(|| info["url"].as_str())
        .unwrap_or_default();
    let availability = info["availability"].as_str().unwrap_or("");
    let restricted =
        ["private", "premium_only", "subscriber_only", "needs_auth"].contains(&availability);
    let formats = info["formats"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|f| {
                    f["vcodec"].as_str() == Some("none")
                        && f["acodec"].as_str().is_some_and(|v| v != "none")
                })
                .map(|f| {
                    let codec = f["acodec"].as_str().unwrap_or("unknown").to_string();
                    AudioFormat {
                        id: f["format_id"].as_str().unwrap_or_default().into(),
                        lossless: ["flac", "alac", "wav", "pcm"]
                            .iter()
                            .any(|c| codec.starts_with(c)),
                        codec,
                        extension: f["ext"].as_str().unwrap_or_default().into(),
                        bitrate: f["abr"].as_f64(),
                        sample_rate: f["asr"].as_u64().map(|x| x as u32),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let valid_artwork = |value: &str| {
        let parsed = url::Url::parse(value).ok()?;
        (matches!(parsed.scheme(), "http" | "https")
            && parsed.username().is_empty()
            && parsed.password().is_none())
        .then(|| value.to_owned())
    };
    let artwork_url = info["thumbnail"]
        .as_str()
        .and_then(valid_artwork)
        .or_else(|| {
            info["thumbnails"].as_array().and_then(|items| {
                items
                    .iter()
                    .rev()
                    .find_map(|item| item["url"].as_str().and_then(valid_artwork))
            })
        });
    let chapters = info["chapters"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let start_seconds = item["start_time"].as_f64()?;
                    let end_seconds = item["end_time"].as_f64()?;
                    let title = item["title"].as_str()?.trim();
                    if !start_seconds.is_finite()
                        || !end_seconds.is_finite()
                        || start_seconds < 0.0
                        || end_seconds <= start_seconds
                        || title.is_empty()
                    {
                        return None;
                    }
                    Some(Chapter {
                        id: format!("chapter-{index}-{}", start_seconds.to_bits()),
                        title: title.to_owned(),
                        start_seconds,
                        end_seconds,
                        confidence: "source_metadata_unverified".into(),
                        suggested_track: None,
                        suggestion_confidence: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|chapters| !chapters.is_empty());
    Candidate {
        id: format!("{provider}:{id}"),
        provider: provider.into(),
        url: url.into(),
        title: info["title"].as_str().unwrap_or("Untitled").into(),
        uploader: info["uploader"].as_str().map(str::to_owned),
        duration_seconds: info["duration"].as_f64(),
        artwork_url,
        chapters,
        formats,
        permission: if restricted { "restricted" } else { "unknown" }.into(),
        ..Default::default()
    }
}

pub async fn find_candidates(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
) -> Result<Vec<Candidate>, String> {
    let (_tx, cancel) = idle_cancel();
    find_candidates_with_cancel(
        track,
        provider_ids,
        settings,
        cancel,
        10,
        Duration::from_secs(60),
    )
    .await
}

async fn find_candidates_with_cancel(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
) -> Result<Vec<Candidate>, String> {
    if let Some(candidate) = track
        .provenance
        .get("audioCandidate")
        .and_then(|value| serde_json::from_value::<Candidate>(value.clone()).ok())
    {
        if validate_source_url(&candidate.url, &candidate.provider).is_ok()
            && provider_ids
                .iter()
                .any(|provider| provider == &candidate.provider)
        {
            return Ok(vec![candidate]);
        }
    }
    use futures_util::StreamExt;
    let provider_results =
        futures_util::stream::iter(provider_ids.iter().cloned().map(|provider| {
            let cancel = cancel.clone();
            async move {
                search_provider_candidates(
                    &provider,
                    track,
                    settings,
                    cancel,
                    result_limit,
                    per_provider_timeout,
                )
                .await
            }
        }))
        .buffered(2)
        .collect::<Vec<_>>()
        .await;
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for (found, provider_errors) in provider_results {
        candidates.extend(found);
        errors.extend(provider_errors);
    }
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    if candidates.is_empty() && !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        if !errors.is_empty() {
            for candidate in &mut candidates {
                candidate
                    .reasons
                    .push(format!("Some providers failed: {}", errors.join("; ")));
            }
        }
        Ok(candidates)
    }
}

async fn search_provider_candidates(
    provider: &str,
    track: &Track,
    settings: &Settings,
    mut cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
) -> (Vec<Candidate>, Vec<String>) {
    if provider == "monochrome" {
        let result = tokio::select! {
            result = async { crate::monochrome::Client::new()?.candidates(track).await } => result,
            _ = cancel.changed() => Err("Cancelled".into()),
        };
        return match result {
            Ok(found) => (found, Vec::new()),
            Err(error) => (Vec::new(), vec![error]),
        };
    }
    let prefix = match provider {
        "youtube" => format!("ytsearch{result_limit}"),
        "soundcloud" => format!("scsearch{result_limit}"),
        _ => return (Vec::new(), Vec::new()),
    };
    let query = search_query(track, true);
    let fallback_query = search_query(track, false);
    let deadline = Instant::now() + per_provider_timeout;
    let mut queries = vec![query.as_str()];
    if !fallback_query.is_empty() && fallback_query != query {
        queries.push(fallback_query.as_str());
    }
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for query in queries {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            errors.push(format!("{provider}: search timed out"));
            break;
        }
        let mut args = base_args(settings);
        if let Err(error) = add_youtube_support_args(&mut args, provider).await {
            return (Vec::new(), vec![format!("{provider}: {error}")]);
        }
        args.extend([
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--skip-download".into(),
            "--".into(),
            format!("{prefix}:{query}"),
        ]);
        match run_tool(settings, args, cancel.clone(), remaining, |_| {}).await {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(info) => {
                    if let Some(entries) = info["entries"].as_array() {
                        for entry in entries.iter().take(result_limit) {
                            let mut candidate = candidate_from_json(entry, provider);
                            if provider == "youtube" && candidate.url.is_empty() {
                                candidate.url = format!(
                                    "https://www.youtube.com/watch?v={}",
                                    entry["id"].as_str().unwrap_or("")
                                );
                            }
                            if validate_source_url(&candidate.url, provider).is_err() {
                                continue;
                            }
                            matching::score_candidate(track, &mut candidate);
                            candidate
                                .reasons
                                .push("Download permission has not been established".into());
                            candidates.push(candidate);
                        }
                    }
                }
                Err(error) => errors.push(format!("{provider}: invalid metadata: {error}")),
            },
            Err(error) => errors.push(format!("{provider}: {error}")),
        }
        if !candidates.is_empty() {
            break;
        }
    }
    (candidates, errors)
}

fn metadata_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => Some(text.trim().to_owned()),
        Value::Array(values) => {
            let artists = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|artist| !artist.is_empty())
                .collect::<Vec<_>>();
            (!artists.is_empty()).then(|| artists.join(", "))
        }
        _ => None,
    }?;
    (!text.is_empty()).then_some(text)
}

fn raw_search_track(candidate: Candidate, info: &Value) -> Track {
    let provider_id = info["id"].as_str().unwrap_or_default().trim().to_owned();
    let provided_artist =
        metadata_text(&info["artist"]).or_else(|| metadata_text(&info["artists"]));
    let provided_title = metadata_text(&info["track"]);
    let raw_title = provided_title
        .clone()
        .unwrap_or_else(|| candidate.title.clone());
    let (artist, title, artist_source) = if let Some(artist) = provided_artist {
        (artist, raw_title, "metadata")
    } else if let Some((artist, title)) = raw_title.split_once(" - ") {
        let artist = artist.trim();
        let title = title.trim();
        if !artist.is_empty() && !title.is_empty() && !title.contains(" - ") {
            (artist.to_owned(), title.to_owned(), "inferred_from_title")
        } else {
            (String::new(), raw_title, "unknown")
        }
    } else {
        (String::new(), raw_title, "unknown")
    };
    let provenance_candidate = serde_json::to_value(&candidate).unwrap_or(Value::Null);
    let mut track = Track {
        id: format!("{}:{provider_id}", candidate.provider),
        provider: candidate.provider.clone(),
        provider_id,
        source_url: candidate.url.clone(),
        artist,
        title,
        album: metadata_text(&info["album"]),
        genre: metadata_text(&info["genre"]),
        duration_seconds: candidate.duration_seconds,
        release_date: metadata_text(&info["release_date"]),
        artwork_url: candidate.artwork_url.clone(),
        provenance: serde_json::json!({
            "sourceKind": "audioSearch",
            "identity": "unverified source search result",
            "artistSource": artist_source,
            "weakFields": {"artist": artist_source != "metadata", "title": provided_title.is_none()},
            "uploader": candidate.uploader,
            "uploadDate": metadata_text(&info["upload_date"]),
            "audioCandidate": provenance_candidate,
        }),
        ..Track::default()
    };
    if candidate.provider == "youtube" {
        crate::metadata_normalization::normalize_selected_track(
            &mut track,
            &candidate.title,
            &candidate.url,
        );
    }
    track
}

async fn search_audio_tracks_raw(
    query: &str,
    provider_ids: &[String],
    settings: &Settings,
) -> Result<Vec<Track>, String> {
    let (_tx, cancel) = idle_cancel();
    let mut tracks = Vec::new();
    let mut errors = Vec::new();
    for provider in provider_ids {
        let prefix = match provider.as_str() {
            "youtube" => "ytsearch10",
            "soundcloud" => "scsearch10",
            _ => continue,
        };
        let mut args = base_args(settings);
        add_youtube_support_args(&mut args, provider).await?;
        args.extend([
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--skip-download".into(),
            "--".into(),
            format!("{prefix}:{query}"),
        ]);
        match run_tool(
            settings,
            args,
            cancel.clone(),
            Duration::from_secs(20),
            |_| {},
        )
        .await
        {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(info) => {
                    if let Some(entries) = info["entries"].as_array() {
                        for entry in entries.iter().take(10) {
                            let mut candidate = candidate_from_json(entry, provider);
                            if provider == "youtube" && candidate.url.is_empty() {
                                if let Some(id) = entry["id"].as_str() {
                                    candidate.url = format!("https://www.youtube.com/watch?v={id}");
                                }
                            }
                            if validate_source_url(&candidate.url, provider).is_err()
                                || candidate.id.ends_with(':')
                            {
                                continue;
                            }
                            candidate.score = 0.8;
                            candidate.confidence = "review".into();
                            candidate.reasons = vec![
                                "Direct source result; track identity has not been verified".into(),
                                "Download permission has not been established".into(),
                            ];
                            tracks.push(raw_search_track(candidate, entry));
                        }
                    }
                }
                Err(error) => errors.push(format!("{provider}: invalid metadata: {error}")),
            },
            Err(error) => errors.push(format!("{provider}: {error}")),
        }
    }
    if tracks.is_empty() && !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Ok(tracks)
    }
}

fn search_query(track: &Track, include_version: bool) -> String {
    let mut parts = vec![
        matching::normalize(&track.artist),
        matching::normalize(&track.title),
    ];
    if include_version {
        let version = matching::normalize_version(track.version.as_deref().unwrap_or(""));
        if !version.is_empty() {
            parts.push(version);
        }
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
struct CachedCandidates {
    cached_at: Instant,
    candidates: Vec<Candidate>,
}

#[derive(Clone)]
struct CachedPreviewAudio {
    cached_at: Instant,
    ttl: Duration,
    audio: PreviewAudio,
}

#[derive(Clone)]
struct CachedTracks {
    cached_at: Instant,
    tracks: Vec<Track>,
}

#[derive(Clone)]
struct CachedChapterSource {
    cached_at: Instant,
    path: PathBuf,
    cache_root: PathBuf,
    staging_root: PathBuf,
}

#[derive(Default)]
struct ChapterSourceCacheState {
    entries: HashMap<String, CachedChapterSource>,
    in_flight: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

#[derive(Default)]
struct ChapterSourceCache {
    state: Mutex<ChapterSourceCacheState>,
}

struct ChapterSourceCacheGate {
    cache: Arc<ChapterSourceCache>,
    key: String,
    gate: Arc<tokio::sync::Mutex<()>>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for ChapterSourceCacheGate {
    fn drop(&mut self) {
        self.guard.take();
        if let Ok(mut state) = self.cache.state.lock() {
            if state.in_flight.get(&self.key).is_some_and(|current| {
                Arc::ptr_eq(current, &self.gate) && Arc::strong_count(current) == 2
            }) {
                state.in_flight.remove(&self.key);
            }
        }
    }
}

impl ChapterSourceCache {
    async fn gate(self: &Arc<Self>, key: &str) -> Result<ChapterSourceCacheGate, String> {
        let gate = self
            .state
            .lock()
            .map_err(|_| "Chapter source cache lock poisoned")?
            .in_flight
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.clone().lock_owned().await;
        Ok(ChapterSourceCacheGate {
            cache: self.clone(),
            key: key.to_owned(),
            gate,
            guard: Some(guard),
        })
    }

    async fn copy_to_staging(&self, key: &str, staging: &Path) -> Option<PathBuf> {
        let (cached, expired) = {
            let mut state = self.state.lock().ok()?;
            let expired_keys = state
                .entries
                .iter()
                .filter(|(_, value)| value.cached_at.elapsed() >= CHAPTER_SOURCE_CACHE_TTL)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            let expired = expired_keys
                .into_iter()
                .filter_map(|key| state.entries.remove(&key).map(|value| value.path))
                .collect::<Vec<_>>();
            (state.entries.get(key).cloned(), expired)
        };
        for path in expired {
            let _ = tokio::fs::remove_file(path).await;
        }
        let cached = cached?;
        let result = async {
            let real_root = tokio::fs::canonicalize(&cached.cache_root).await.ok()?;
            let real_staging_parent = tokio::fs::canonicalize(&cached.staging_root).await.ok()?;
            if !real_root.starts_with(&real_staging_parent) {
                return None;
            }
            let real_source = tokio::fs::canonicalize(&cached.path).await.ok()?;
            if !real_source.starts_with(&real_root)
                || !tokio::fs::metadata(&real_source).await.ok()?.is_file()
            {
                return None;
            }
            let real_staging = tokio::fs::canonicalize(staging).await.ok()?;
            if !real_staging.starts_with(&real_staging_parent) {
                return None;
            }
            let extension = safe_audio_extension(&real_source);
            let target = real_staging.join(format!("chapter-source.{extension}"));
            if tokio::fs::copy(&real_source, &target).await.is_err() {
                let _ = tokio::fs::remove_file(&target).await;
                return None;
            }
            let real_target = tokio::fs::canonicalize(&target).await.ok()?;
            if !real_target.starts_with(&real_staging)
                || !tokio::fs::metadata(&real_target).await.ok()?.is_file()
            {
                let _ = tokio::fs::remove_file(&target).await;
                return None;
            }
            Some(real_target)
        }
        .await;
        if result.is_none() {
            if let Ok(mut state) = self.state.lock() {
                state.entries.remove(key);
            }
            let _ = tokio::fs::remove_file(cached.path).await;
        }
        result
    }

    async fn insert_from_staging(
        &self,
        key: &str,
        source: &Path,
        job_staging: &Path,
        music_directory: &Path,
    ) {
        let cache_root = music_directory
            .join(".burrata-staging")
            .join(".chapter-source-cache");
        if tokio::fs::create_dir_all(&cache_root).await.is_err() {
            return;
        }
        let Some(real_cache_root) = tokio::fs::canonicalize(&cache_root).await.ok() else {
            return;
        };
        let staging_root = music_directory.join(".burrata-staging");
        let Some(real_staging_parent) = tokio::fs::canonicalize(&staging_root).await.ok() else {
            return;
        };
        if !real_cache_root.starts_with(&real_staging_parent) {
            return;
        }
        let Some(real_source) = tokio::fs::canonicalize(source).await.ok() else {
            return;
        };
        let Some(real_job_staging) = tokio::fs::canonicalize(job_staging).await.ok() else {
            return;
        };
        if !real_job_staging.starts_with(&real_staging_parent)
            || !real_source.starts_with(&real_job_staging)
            || !tokio::fs::metadata(&real_source)
                .await
                .is_ok_and(|m| m.is_file())
        {
            return;
        }
        let extension = safe_audio_extension(&real_source);
        let target =
            real_cache_root.join(format!("{}.{}", chapter_source_key_hash(key), extension));
        let temp = real_cache_root.join(format!(
            "{}.{}.tmp",
            chapter_source_key_hash(key),
            Uuid::new_v4()
        ));
        if tokio::fs::copy(&real_source, &temp).await.is_err() {
            let _ = tokio::fs::remove_file(temp).await;
            return;
        }
        let _ = tokio::fs::remove_file(&target).await;
        if tokio::fs::rename(&temp, &target).await.is_err() {
            let _ = tokio::fs::remove_file(temp).await;
            return;
        }
        let mut evicted = Vec::new();
        if let Ok(mut state) = self.state.lock() {
            let expired_keys = state
                .entries
                .iter()
                .filter(|(_, value)| value.cached_at.elapsed() >= CHAPTER_SOURCE_CACHE_TTL)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            evicted.extend(
                expired_keys
                    .into_iter()
                    .filter_map(|key| state.entries.remove(&key).map(|value| value.path)),
            );
            if let Some(previous) = state.entries.remove(key) {
                evicted.push(previous.path);
            }
            state.entries.insert(
                key.to_owned(),
                CachedChapterSource {
                    cached_at: Instant::now(),
                    path: target.clone(),
                    cache_root: real_cache_root.clone(),
                    staging_root: real_staging_parent.clone(),
                },
            );
            while state.entries.len() > CHAPTER_SOURCE_CACHE_MAX {
                let Some(oldest) = state
                    .entries
                    .iter()
                    .min_by_key(|(_, value)| value.cached_at)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                if let Some(entry) = state.entries.remove(&oldest) {
                    evicted.push(entry.path);
                }
            }
        }
        for path in evicted {
            if path != target {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
    }
}

fn safe_audio_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 12
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .unwrap_or("media")
        .to_ascii_lowercase()
}

fn chapter_source_key_hash(key: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Default)]
struct PreviewSearchState {
    latest_generation: AtomicU64,
    active: Mutex<Option<(u64, watch::Sender<bool>)>>,
}

impl PreviewSearchState {
    fn begin(&self, generation: u64) -> Result<watch::Receiver<bool>, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Preview search lock poisoned")?;
        let latest = self.latest_generation.load(Ordering::SeqCst);
        if generation < latest
            || (generation == latest
                && active
                    .as_ref()
                    .is_some_and(|(active_generation, _)| *active_generation == generation))
        {
            return Err("Preview search superseded by a newer request".into());
        }
        if let Some((_, previous)) = active.take() {
            let _ = previous.send(true);
        }
        self.latest_generation.store(generation, Ordering::SeqCst);
        let (tx, rx) = idle_cancel();
        *active = Some((generation, tx));
        Ok(rx)
    }

    fn finish(&self, generation: u64) -> Result<bool, String> {
        let current = self.latest_generation.load(Ordering::SeqCst) == generation;
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Preview search lock poisoned")?;
        if active
            .as_ref()
            .is_some_and(|(active_generation, _)| *active_generation == generation)
        {
            active.take();
        }
        Ok(current)
    }
}

#[derive(Default)]
struct CandidateSearchCache {
    entries: Mutex<HashMap<String, CachedCandidates>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct InspectionCache {
    entries: Mutex<HashMap<String, (Instant, Candidate)>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

static INSPECTION_CACHE: std::sync::OnceLock<InspectionCache> = std::sync::OnceLock::new();

fn inspection_cache_key(candidate: &Candidate, settings: &Settings) -> String {
    serde_json::json!([
        INSPECTION_PROFILE,
        candidate.provider,
        candidate.url,
        crate::youtube_support::active_executable().to_string_lossy(),
        settings.yt_dlp_path,
        settings.js_runtime_path
    ])
    .to_string()
}

impl InspectionCache {
    async fn get_or_inspect<F, Fut>(&self, key: String, inspect: F) -> Result<Candidate, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Candidate, String>>,
    {
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Inspection cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.lock().await;
        if let Some(cached) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, (at, _)| at.elapsed() < INSPECTION_CACHE_TTL);
            entries.get(&key).map(|(_, candidate)| candidate.clone())
        }) {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Ok(cached);
        }
        let candidate = match inspect().await {
            Ok(candidate) => candidate,
            Err(error) => {
                drop(guard);
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, (at, _)| at.elapsed() < INSPECTION_CACHE_TTL);
            if entries.len() >= INSPECTION_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, (at, _))| at)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(key.clone(), (Instant::now(), candidate.clone()));
        }
        drop(guard);
        remove_flight_if_unused(&self.in_flight, &key, &gate);
        Ok(candidate)
    }
}

#[derive(Default)]
struct PreviewAudioCache {
    entries: Mutex<HashMap<String, CachedPreviewAudio>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct RawAudioSearchCache {
    entries: Mutex<HashMap<String, CachedTracks>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RawAudioSearchCache {
    async fn get_or_search<F, Fut>(&self, key: String, search: F) -> Result<Vec<Track>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Track>, String>>,
    {
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Audio search cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.lock().await;
        if let Some(tracks) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, value| value.cached_at.elapsed() < RAW_AUDIO_SEARCH_CACHE_TTL);
            entries.get(&key).map(|value| value.tracks.clone())
        }) {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Ok(tracks);
        }
        let tracks = match search().await {
            Ok(tracks) => tracks,
            Err(error) => {
                drop(guard);
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, value| value.cached_at.elapsed() < RAW_AUDIO_SEARCH_CACHE_TTL);
            if entries.len() >= RAW_AUDIO_SEARCH_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, value)| value.cached_at)
                    .map(|(key, _)| key.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(
                key.clone(),
                CachedTracks {
                    cached_at: Instant::now(),
                    tracks: tracks.clone(),
                },
            );
        }
        drop(guard);
        remove_flight_if_unused(&self.in_flight, &key, &gate);
        Ok(tracks)
    }
}

impl PreviewAudioCache {
    async fn get_or_resolve<F, Fut>(&self, key: String, resolve: F) -> Result<PreviewAudio, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<PreviewAudio, String>>,
    {
        let (_tx, cancel) = idle_cancel();
        self.get_or_resolve_cancelled(key, cancel, resolve).await
    }

    async fn get_or_resolve_cancelled<F, Fut>(
        &self,
        key: String,
        mut cancel: watch::Receiver<bool>,
        resolve: F,
    ) -> Result<PreviewAudio, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<PreviewAudio, String>>,
    {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Preview audio cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = tokio::select! {
            guard = gate.lock() => guard,
            _ = cancel.changed() => {
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err("Cancelled".into());
            }
        };
        if *cancel.borrow() {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Err("Cancelled".into());
        }
        if let Some(cached) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, v| v.cached_at.elapsed() < v.ttl);
            entries.get(&key).map(|v| v.audio.clone())
        }) {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Ok(cached);
        }
        let audio = match tokio::select! {
            result = resolve() => result,
            _ = cancel.changed() => Err("Cancelled".into()),
        } {
            Ok(audio) => audio,
            Err(error) => {
                drop(guard);
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, v| v.cached_at.elapsed() < v.ttl);
            if entries.len() >= PREVIEW_AUDIO_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, v)| v.cached_at)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(
                key.clone(),
                CachedPreviewAudio {
                    cached_at: Instant::now(),
                    ttl: preview_audio_cache_ttl(&audio.url),
                    audio: audio.clone(),
                },
            );
        }
        drop(guard);
        remove_flight_if_unused(&self.in_flight, &key, &gate);
        Ok(audio)
    }
}

fn preview_audio_cache_ttl(raw_url: &str) -> Duration {
    let Some(expiry) = url::Url::parse(raw_url).ok().and_then(|url| {
        url.query_pairs().find_map(|(key, value)| {
            matches!(key.as_ref(), "expire" | "Expires" | "expires")
                .then(|| value.parse::<i64>().ok())
                .flatten()
        })
    }) else {
        return PREVIEW_AUDIO_CACHE_TTL;
    };
    let seconds = expiry
        .saturating_sub(Utc::now().timestamp())
        .saturating_sub(30);
    Duration::from_secs(seconds.max(0) as u64).min(PREVIEW_AUDIO_CACHE_TTL)
}

fn remove_flight_if_unused(
    flights: &Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    key: &str,
    gate: &Arc<tokio::sync::Mutex<()>>,
) {
    if let Ok(mut in_flight) = flights.lock() {
        if in_flight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, gate) && Arc::strong_count(current) == 2)
        {
            in_flight.remove(key);
        }
    }
}

impl CandidateSearchCache {
    async fn get_or_search<F, Fut>(&self, key: String, load: F) -> Result<Vec<Candidate>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
    {
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Search cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.lock().await;
        if let Some(cached) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, v| v.cached_at.elapsed() < SEARCH_CACHE_TTL);
            entries.get(&key).map(|v| v.candidates.clone())
        }) {
            drop(guard);
            self.remove_flight_if_unused(&key, &gate);
            return Ok(cached);
        }
        let candidates = match load().await {
            Ok(candidates) => candidates,
            Err(error) => {
                drop(guard);
                self.remove_flight_if_unused(&key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, v| v.cached_at.elapsed() < SEARCH_CACHE_TTL);
            if entries.len() >= SEARCH_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, v)| v.cached_at)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(
                key.clone(),
                CachedCandidates {
                    cached_at: Instant::now(),
                    candidates: candidates.clone(),
                },
            );
        }
        drop(guard);
        self.remove_flight_if_unused(&key, &gate);
        Ok(candidates)
    }

    fn remove_flight_if_unused(&self, key: &str, gate: &Arc<tokio::sync::Mutex<()>>) {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            if in_flight.get(key).is_some_and(|current| {
                Arc::ptr_eq(current, gate) && Arc::strong_count(current) == 2
            }) {
                in_flight.remove(key);
            }
        }
    }
}

fn search_cache_key(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    result_limit: usize,
) -> String {
    serde_json::json!([
        track.artist,
        track.title,
        track.version,
        track.duration_seconds,
        provider_ids,
        result_limit,
        settings.yt_dlp_path,
        settings.js_runtime_path,
    ])
    .to_string()
}

pub async fn inspect_candidate(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
) -> Result<Candidate, String> {
    let (_tx, cancel) = idle_cancel();
    inspect_with_cancel(track, candidate, settings, cancel).await
}

pub async fn inspect_chapters(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
) -> Result<Vec<Chapter>, String> {
    let inspected = inspect_candidate(track, candidate, settings).await?;
    inspected
        .chapters
        .ok_or_else(|| "This source does not provide chapter metadata".into())
}

fn chapter_artist_title(label: &str) -> (String, String, &'static str) {
    if let Some((artist, title)) = label.split_once(" - ") {
        let artist = artist.trim();
        let title = title.trim();
        if !artist.is_empty() && !title.is_empty() {
            return (artist.to_owned(), title.to_owned(), "chapter_label");
        }
    }
    (String::new(), label.trim().to_owned(), "unknown")
}

fn chapter_track_identity(parent_id: &str, chapter_id: &str) -> (String, String) {
    let provider_id = format!("{parent_id}:{chapter_id}");
    (format!("chapter:{provider_id}"), provider_id)
}

fn verified_chapter_suggestion<'a>(
    chapter: &'a Chapter,
    required_version: Option<&str>,
) -> Option<&'a Track> {
    let track = chapter.suggested_track.as_deref()?;
    let expected_url = format!("https://musicbrainz.org/recording/{}", track.provider_id);
    (chapter.suggestion_confidence.as_deref() == Some("exact_unique_catalog_match")
        && track.provider == "musicbrainz"
        && !track.provider_id.is_empty()
        && track.id == format!("musicbrainz:{}", track.provider_id)
        && track.source_url == expected_url
        && !track.artist.trim().is_empty()
        && !track.title.trim().is_empty()
        && required_version.is_none_or(|version| {
            track.version.as_deref().is_some_and(|catalog_version| {
                matching::normalize_version(catalog_version) == matching::normalize_version(version)
            })
        }))
    .then_some(track)
}

async fn resolve_preview_audio(
    candidate: &Candidate,
    settings: &Settings,
    mut cancel: watch::Receiver<bool>,
) -> Result<PreviewAudio, String> {
    if candidate.provider == "monochrome" {
        crate::monochrome::validate_url(&candidate.url)?;
        let client = crate::monochrome::Client::new()?;
        let inspected = tokio::select! {
            result = client.inspect(candidate) => result?,
            _ = cancel.changed() => return Err("Cancelled".into()),
        };
        return Ok(PreviewAudio {
            url: inspected.url,
            codec: inspected.formats.first().map(|format| format.codec.clone()),
            bitrate_kbps: inspected.formats.first().and_then(|format| format.bitrate),
            duration_seconds: inspected.duration_seconds,
            mime_type: if inspected.formats[0].lossless {
                "audio/flac"
            } else {
                "audio/mp4"
            }
            .into(),
        });
    }
    if !matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
        return Err("Audio preview is available only for YouTube and SoundCloud candidates".into());
    }
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let source = validate_source_url(&candidate.url, &candidate.provider)?;
    let _addresses = tokio::select! {
        result = public_addresses(&source) => result?,
        _ = cancel.changed() => return Err("Cancelled".into()),
    };
    let mut args = base_args(settings);
    tokio::select! {
        result = add_youtube_support_args(&mut args, &candidate.provider) => result?,
        _ = cancel.changed() => return Err("Cancelled".into()),
    }
    args.extend([
        "--dump-single-json".into(),
        "--no-playlist".into(),
        "--skip-download".into(),
        "--format".into(),
        PREVIEW_AUDIO_FORMAT_SELECTOR.into(),
        "--".into(),
        candidate.url.clone(),
    ]);
    let raw = run_tool(
        settings,
        args,
        cancel.clone(),
        PREVIEW_SEARCH_TIMEOUT,
        |_| {},
    )
    .await
    .map_err(|error| {
        if error == "Cancelled" {
            "Cancelled".to_string()
        } else if error.contains("timed out") {
            "Audio stream resolution timed out".to_string()
        } else {
            "yt-dlp could not resolve an audio-only stream".to_string()
        }
    })?;
    let info: Value = serde_json::from_str(&raw)
        .map_err(|_| "yt-dlp returned invalid audio stream metadata".to_string())?;
    let audio = preview_audio_from_metadata(candidate, &info)?;
    let stream = validate_source_url(&audio.url, "direct")?;
    let _stream_addresses = tokio::select! {
        result = public_addresses(&stream) => result?,
        _ = cancel.changed() => return Err("Cancelled".into()),
    };
    Ok(audio)
}

fn preview_audio_from_metadata(
    candidate: &Candidate,
    info: &Value,
) -> Result<PreviewAudio, String> {
    if info["entries"].is_array()
        || matches!(info["_type"].as_str(), Some("playlist" | "multi_video"))
    {
        return Err("Audio preview requires one individual recording".into());
    }
    let stream_url = info["url"]
        .as_str()
        .ok_or("yt-dlp returned no audio stream URL")?;
    let codec = info["acodec"]
        .as_str()
        .filter(|codec| *codec != "none")
        .ok_or("yt-dlp did not select an audio-only stream")?;
    let progressive = matches!(info["protocol"].as_str(), Some("http" | "https"));
    let soundcloud_hls = candidate.provider == "soundcloud"
        && matches!(info["protocol"].as_str(), Some("m3u8" | "m3u8_native"));
    if !progressive && !soundcloud_hls {
        return Err("This source has no supported audio stream".into());
    }
    if info["vcodec"].as_str().is_some_and(|codec| codec != "none") {
        return Err("yt-dlp selected a video stream instead of audio-only".into());
    }
    if !candidate.id.is_empty() {
        let resolved_id = info["id"].as_str().unwrap_or_default();
        if candidate.id != format!("{}:{resolved_id}", candidate.provider) {
            return Err("Source identity changed during audio resolution; select it again".into());
        }
    }
    let stream = validate_source_url(stream_url, "direct")?;
    if stream.scheme() != "https" {
        return Err("Audio stream must use public HTTPS".into());
    }
    let host = stream.host_str().ok_or("Missing audio stream hostname")?;
    let allowed_cdn = match candidate.provider.as_str() {
        "youtube" => host_matches(host, "googlevideo.com"),
        "soundcloud" => host_matches(host, "sndcdn.com") || host_matches(host, "soundcloud.com"),
        _ => false,
    };
    if !allowed_cdn {
        return Err("yt-dlp returned a stream outside the provider media hosts".into());
    }
    let mime_type = audio_mime_type(info["ext"].as_str().unwrap_or_default(), codec)?;
    let duration_seconds = info["duration"]
        .as_f64()
        .filter(|duration| duration.is_finite() && *duration > 0.0);
    Ok(PreviewAudio {
        url: stream_url.into(),
        mime_type,
        codec: Some(codec.to_owned()),
        bitrate_kbps: info["abr"]
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0),
        duration_seconds,
    })
}

fn audio_mime_type(extension: &str, codec: &str) -> Result<String, String> {
    let mime = match extension.to_ascii_lowercase().as_str() {
        "m4a" | "mp4" => "audio/mp4",
        "webm" => "audio/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        _ if codec.starts_with("mp4a.") => "audio/mp4",
        _ => return Err("Unsupported audio preview format".into()),
    };
    Ok(mime.into())
}

async fn inspect_with_cancel(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    let key = inspection_cache_key(candidate, settings);
    let cache = INSPECTION_CACHE.get_or_init(InspectionCache::default);
    let mut inspected = cache
        .get_or_inspect(key, || {
            inspect_metadata_with_cancel(candidate, settings, cancel)
        })
        .await?;
    matching::score_candidate(track, &mut inspected);
    if candidate.provider == "direct" {
        inspected
            .reasons
            .push("Direct download permission must be verified by you".into());
    } else {
        inspected
            .reasons
            .push("Streaming availability does not establish download permission".into());
    }
    Ok(inspected)
}

async fn inspect_metadata_with_cancel(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    if candidate.provider == "monochrome" {
        crate::monochrome::validate_url(&candidate.url)?;
        let client = crate::monochrome::Client::new()?;
        let mut cancel = cancel;
        let mut result = tokio::select! { result = client.inspect(candidate) => result, _ = cancel.changed() => Err("Cancelled".into()) }?;
        result.score = 0.0;
        result.reasons.clear();
        return Ok(result);
    }
    let url = validate_source_url(&candidate.url, &candidate.provider)?;
    public_addresses(&url).await?;
    if candidate.provider == "direct" {
        let mut result = candidate.clone();
        result.formats = Vec::new();
        result.official = false;
        result.permission = "unknown".into();
        return Ok(result);
    }
    inspect_ytdlp_metadata(candidate, settings, cancel).await
}

async fn inspect_ytdlp_metadata(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    let mut args = base_args(settings);
    add_youtube_support_args(&mut args, &candidate.provider).await?;
    args.extend([
        "--dump-single-json".into(),
        "--no-playlist".into(),
        "--skip-download".into(),
        "--".into(),
        candidate.url.clone(),
    ]);
    let raw = run_tool(settings, args, cancel, Duration::from_secs(60), |_| {}).await?;
    let info: Value =
        serde_json::from_str(&raw).map_err(|e| format!("Invalid source metadata: {e}"))?;
    if info["entries"].is_array()
        || matches!(info["_type"].as_str(), Some("playlist" | "multi_video"))
    {
        return Err("This URL contains multiple recordings. Choose an individual track; collections never download implicitly.".into());
    }
    let mut result = candidate_from_json(&info, &candidate.provider);
    if result.url.is_empty() {
        result.url = candidate.url.clone();
    }
    validate_source_url(&result.url, &result.provider)?;
    if !candidate.id.is_empty() && candidate.id != result.id {
        return Err("Source identity changed during inspection; select it again".into());
    }
    result.score = 0.0;
    result.reasons.clear();
    Ok(result)
}

pub async fn check_dependencies(settings: &Settings) -> Vec<Dependency> {
    let mut result = Vec::new();
    for (name, path, default_name) in [
        ("yt-dlp", &settings.yt_dlp_path, "yt-dlp"),
        ("ffmpeg", &settings.ffmpeg_path, "ffmpeg"),
        ("ffprobe", &settings.ffprobe_path, "ffprobe"),
        ("JavaScript runtime", &settings.js_runtime_path, "deno"),
    ] {
        let executable = crate::tools::executable(path, default_name);
        let mut command = Command::new(executable);
        command.arg(if name == "ffmpeg" || name == "ffprobe" {
            "-version"
        } else {
            "--version"
        });
        command
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
        }
        let dependency = match tokio::time::timeout(Duration::from_secs(5), command.output()).await
        {
            Ok(Ok(out)) if out.status.success() => Dependency {
                name: name.into(),
                available: true,
                version: Some(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("Available")
                        .chars()
                        .take(200)
                        .collect(),
                ),
                message: if name == "yt-dlp" {
                    "Executable available; YouTube also requires EJS and a supported JavaScript runtime".into()
                } else {
                    "Available".into()
                },
            },
            Ok(Ok(_)) => Dependency {
                name: name.into(),
                available: false,
                version: None,
                message: format!("{path} returned a failed version check"),
            },
            Ok(Err(e)) => Dependency {
                name: name.into(),
                available: false,
                version: None,
                message: format!("Configure an installed {name} executable: {e}"),
            },
            Err(_) => Dependency {
                name: name.into(),
                available: false,
                version: None,
                message: "Version check timed out after 5 seconds".into(),
            },
        };
        result.push(dependency);
    }
    result.push(Dependency { name: "yt-dlp-ejs".into(), available: false, version: None,
        message: "Unknown: EJS availability cannot be established from executable version checks. Official yt-dlp binaries include it; package installs need yt-dlp-ejs. A real YouTube inspection validates support.".into() });
    result
}

#[derive(Clone)]
pub struct AcquisitionManager {
    inner: Arc<Inner>,
}
struct Inner {
    app: tauri::AppHandle,
    store: Arc<Store>,
    connections: Arc<ConnectionManager>,
    analysis: Mutex<Option<Arc<crate::analysis::AnalysisManager>>>,
    autotag: Mutex<Option<Arc<crate::autotag::AutoTagger>>>,
    slskd_searches: Mutex<HashMap<String, SlskdSearchSession>>,
    slskd_completed: Mutex<HashMap<String, (Instant, SlskdSearchPoll)>>,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    search_cache: CandidateSearchCache,
    raw_audio_search_cache: RawAudioSearchCache,
    preview_audio_cache: PreviewAudioCache,
    chapter_source_cache: Arc<ChapterSourceCache>,
    preview_search: PreviewSearchState,
    slots: Semaphore,
    soulseek_slots: Semaphore,
}
struct SlskdSearchSession {
    started: Instant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlskdSearchPoll {
    pub id: String,
    pub state: String,
    pub tracks: Vec<Track>,
    pub candidates: Vec<Candidate>,
    pub complete: bool,
    pub warnings: Vec<String>,
}

impl AcquisitionManager {
    pub fn new(
        app: tauri::AppHandle,
        store: Arc<Store>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                app,
                store,
                connections,
                analysis: Mutex::new(None),
                autotag: Mutex::new(None),
                slskd_searches: Mutex::new(HashMap::new()),
                slskd_completed: Mutex::new(HashMap::new()),
                active: Mutex::new(HashMap::new()),
                search_cache: CandidateSearchCache::default(),
                raw_audio_search_cache: RawAudioSearchCache::default(),
                preview_audio_cache: PreviewAudioCache::default(),
                chapter_source_cache: Arc::new(ChapterSourceCache::default()),
                preview_search: PreviewSearchState::default(),
                slots: Semaphore::new(2),
                soulseek_slots: Semaphore::new(SOULSEEK_PENDING_LIMIT),
            }),
        }
    }
    pub async fn resume_interrupted_soulseek(&self, settings: Settings) -> Result<usize, String> {
        if settings.music_directory.trim().is_empty() || self.inner.connections.slskd()?.is_none() {
            return Ok(0);
        }
        let mut resumed = 0;
        for job in self.inner.store.list_jobs()? {
            if resumable_soulseek_job(&job) {
                match self.retry(&job.id, settings.clone()).await {
                    Ok(_) => resumed += 1,
                    Err(error) => eprintln!("Soulseek job {} recovery failed: {error}", job.id),
                }
            }
        }
        Ok(resumed)
    }

    pub fn set_analysis_manager(&self, analysis: Arc<crate::analysis::AnalysisManager>) {
        if let Ok(mut slot) = self.inner.analysis.lock() {
            *slot = Some(analysis);
        }
    }
    pub fn set_autotag_manager(&self, manager: Arc<crate::autotag::AutoTagger>) {
        if let Ok(mut slot) = self.inner.autotag.lock() {
            *slot = Some(manager);
        }
    }
    fn autotag_download(&self, asset: &LocalAsset) {
        if let Ok(slot) = self.inner.autotag.lock() {
            if let Some(manager) = slot.as_ref() {
                let _ = manager.queue_new_download(&asset.id);
            }
        }
    }
    pub async fn search_audio_tracks(
        &self,
        query: &str,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Track>, String> {
        if provider_ids.len() == 1 && provider_ids[0] == "monochrome" {
            let rows = crate::monochrome::Client::new()?.search(query).await?;
            return rows
                .into_iter()
                .map(|(track, _)| self.inner.store.save_track(&track))
                .collect();
        }
        let query = query.trim();
        if query.is_empty() {
            return Err("Enter a search query".into());
        }
        if query.chars().count() > 500 {
            return Err("Search query must be 500 characters or fewer".into());
        }
        let providers = provider_ids
            .iter()
            .filter(|provider| matches!(provider.as_str(), "youtube" | "soundcloud"))
            .cloned()
            .collect::<Vec<_>>();
        if providers.is_empty() {
            return Err("Choose YouTube or SoundCloud for audio search".into());
        }
        let key = serde_json::json!([
            "raw-audio-search-v1",
            query,
            providers,
            settings.yt_dlp_path,
            settings.js_runtime_path,
        ])
        .to_string();
        self.inner
            .raw_audio_search_cache
            .get_or_search(key, || search_audio_tracks_raw(query, &providers, settings))
            .await
    }

    pub async fn start_slskd_search(&self, query: &str) -> Result<String, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err("Search query must contain 1 to 500 characters".into());
        }
        let connection = self
            .inner
            .connections
            .slskd()?
            .ok_or("Enable and configure slskd in Connections first")?;
        let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
        let id = client.start_search(query, 15).await?;
        self.inner
            .slskd_searches
            .lock()
            .map_err(|_| "slskd search state lock poisoned")?
            .insert(
                id.clone(),
                SlskdSearchSession {
                    started: Instant::now(),
                },
            );
        Ok(id)
    }

    pub async fn poll_slskd_search(&self, search_id: &str) -> Result<SlskdSearchPoll, String> {
        {
            let mut completed = self
                .inner
                .slskd_completed
                .lock()
                .map_err(|_| "Soulseek result cache lock poisoned")?;
            completed.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
            if let Some((_, result)) = completed.get(search_id) {
                return Ok(result.clone());
            }
        }
        let session = self
            .inner
            .slskd_searches
            .lock()
            .map_err(|_| "slskd search state lock poisoned")?
            .get(search_id)
            .map(|session| session.started)
            .ok_or("Unknown or expired slskd search")?;
        let connection = self
            .inner
            .connections
            .slskd()?
            .ok_or("slskd is disabled in Connections")?;
        let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
        let status = client.search_status(search_id).await?;
        let responses = match client.search_responses(search_id).await {
            Ok(responses) => responses,
            Err(_) => status.responses.clone(),
        };
        let mut candidates = crate::slskd::candidates_from_responses(&responses, search_id);
        // Keep recording identity/confidence unchanged; availability orders peer copies only.
        let history = self.inner.store.list_jobs()?;
        candidates.sort_by_key(|candidate| {
            (
                slskd_peer_failure_order(candidate, &history),
                slskd_availability_order(candidate),
            )
        });
        let tracks = candidates
            .iter()
            .map(slskd_candidate_track)
            .collect::<Vec<_>>();
        let state = status.state.unwrap_or_else(|| {
            if status.is_complete.unwrap_or(false) {
                "complete"
            } else {
                "searching"
            }
            .into()
        });
        let state_lower = state.to_ascii_lowercase();
        let complete = status.is_complete.unwrap_or(false)
            || state_lower.contains("complete")
            || state_lower.contains("timedout")
            || state_lower.contains("cancelled")
            || session.elapsed() >= Duration::from_secs(125);
        let mut warnings = Vec::new();
        if session.elapsed() >= Duration::from_secs(120) && !complete {
            warnings.push("The slskd search timed out while waiting for results".into());
        }
        if candidates.is_empty() && complete {
            warnings.push("No audio files were returned by this search".into());
        }
        if complete {
            let _ = client.delete_search(search_id).await;
            self.inner
                .slskd_searches
                .lock()
                .map_err(|_| "slskd search state lock poisoned")?
                .remove(search_id);
        }
        let result = SlskdSearchPoll {
            id: search_id.into(),
            state,
            tracks,
            candidates,
            complete,
            warnings,
        };
        if complete {
            let mut completed = self
                .inner
                .slskd_completed
                .lock()
                .map_err(|_| "Soulseek result cache lock poisoned")?;
            while completed.len() >= 4 {
                if let Some(oldest) = completed
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(id, _)| id.clone())
                {
                    completed.remove(&oldest);
                } else {
                    break;
                }
            }
            completed.insert(search_id.to_owned(), (Instant::now(), result.clone()));
        }
        Ok(result)
    }

    async fn find_slskd_candidates_for_track(
        &self,
        track: &Track,
    ) -> Result<Vec<Candidate>, String> {
        let query = [
            track.artist.as_str(),
            track.title.as_str(),
            track.version.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        let id = self.start_slskd_search(&query).await?;
        let deadline = Instant::now() + Duration::from_secs(18);
        let mut seen = HashMap::<String, Candidate>::new();
        loop {
            let page = self.poll_slskd_search(&id).await?;
            for mut candidate in page.candidates {
                let availability = candidate
                    .reasons
                    .iter()
                    .filter(|reason| reason.starts_with("availability:"))
                    .cloned()
                    .collect::<Vec<_>>();
                matching::score_candidate(track, &mut candidate);
                candidate.reasons.extend(availability);
                candidate.confidence = "review".into();
                candidate.permission = "unknown".into();
                seen.insert(candidate.id.clone(), candidate);
            }
            if page.complete {
                let mut candidates = seen.into_values().collect::<Vec<_>>();
                let history = self.inner.store.list_jobs()?;
                // This is a tie-breaker after retaining all identity scores:
                // peers with an open upload slot are more likely to start now.
                candidates.sort_by(|a, b| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| {
                            slskd_peer_failure_order(a, &history)
                                .cmp(&slskd_peer_failure_order(b, &history))
                        })
                        .then_with(|| slskd_availability_order(a).cmp(&slskd_availability_order(b)))
                });
                return Ok(candidates);
            }
            if Instant::now() >= deadline {
                let _ = self.cancel_slskd_search(&id).await;
                let mut candidates = seen.into_values().collect::<Vec<_>>();
                let history = self.inner.store.list_jobs()?;
                candidates.sort_by(|a, b| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| {
                            slskd_peer_failure_order(a, &history)
                                .cmp(&slskd_peer_failure_order(b, &history))
                        })
                        .then_with(|| slskd_availability_order(a).cmp(&slskd_availability_order(b)))
                });
                return Ok(candidates);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    pub async fn cancel_slskd_search(&self, search_id: &str) -> Result<(), String> {
        let connection = self
            .inner
            .connections
            .slskd()?
            .ok_or("slskd is disabled in Connections")?;
        let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
        client.cancel_search(search_id).await?;
        let _ = client.delete_search(search_id).await;
        self.inner
            .slskd_searches
            .lock()
            .map_err(|_| "slskd search state lock poisoned")?
            .remove(search_id);
        Ok(())
    }
    fn update(&self, job: &DownloadJob) -> Result<(), String> {
        self.inner.store.save_job(job)?;
        let _ = self.inner.app.emit("download-progress", job);
        Ok(())
    }
    async fn cached_preview_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
    ) -> Result<Vec<Candidate>, String> {
        let key = search_cache_key(track, provider_ids, settings, 3);
        self.inner
            .search_cache
            .get_or_search(key, || {
                find_candidates_with_cancel(
                    track,
                    provider_ids,
                    settings,
                    cancel,
                    3,
                    PREVIEW_SEARCH_TIMEOUT,
                )
            })
            .await
    }

    pub async fn prefetch_preview_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Candidate>, String> {
        let (_tx, cancel) = idle_cancel();
        self.prefetch_preview_candidates_cancelled(track, provider_ids, settings, cancel)
            .await
    }

    pub async fn prefetch_preview_candidates_cancelled(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
    ) -> Result<Vec<Candidate>, String> {
        self.cached_preview_candidates(track, provider_ids, settings, cancel)
            .await
    }

    pub async fn find_preview_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
        generation: u64,
    ) -> Result<Vec<Candidate>, String> {
        let cancel = self.inner.preview_search.begin(generation)?;
        let result = self
            .cached_preview_candidates(track, provider_ids, settings, cancel)
            .await;
        if !self.inner.preview_search.finish(generation)? {
            return Err("Preview search superseded by a newer request".into());
        }
        result
    }

    pub async fn resolve_preview_audio(
        &self,
        candidate: &Candidate,
        settings: &Settings,
    ) -> Result<PreviewAudio, String> {
        let (_tx, cancel) = idle_cancel();
        self.resolve_preview_audio_cancelled(candidate, settings, cancel)
            .await
    }

    pub async fn resolve_preview_audio_cancelled(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
    ) -> Result<PreviewAudio, String> {
        let key = serde_json::json!([
            candidate.provider,
            candidate.url,
            settings.yt_dlp_path,
            settings.js_runtime_path,
        ])
        .to_string();
        self.inner
            .preview_audio_cache
            .get_or_resolve_cancelled(key, cancel.clone(), || {
                resolve_preview_audio(candidate, settings, cancel)
            })
            .await
    }
    pub async fn enqueue(
        &self,
        mut request: DownloadRequest,
        settings: Settings,
    ) -> Result<DownloadJob, String> {
        if let Some(stored) = self.inner.store.get_track(&request.track.id)? {
            let before = serde_json::to_value(stored).map_err(|e| e.to_string())?;
            let requested = serde_json::to_value(&request.track).map_err(|e| e.to_string())?;
            if !request.track.provenance.is_object() {
                request.track.provenance = serde_json::json!({});
            }
            for field in [
                "artist",
                "title",
                "album",
                "albumArtist",
                "version",
                "genre",
                "label",
                "releaseDate",
                "recordingDate",
                "isrc",
                "catalogNumber",
                "bpm",
                "key",
            ] {
                if before[field] != requested[field] {
                    request.track.provenance["manualFields"][field] = serde_json::json!(true);
                }
            }
        }
        if request.candidate.provider == "youtube" {
            let is_catalog_identity = matches!(
                request.track.provider.as_str(),
                "catalog" | "musicbrainz" | "discogs" | "spotify" | "beatport" | "traxsource"
            ) || request.track.provenance["sourceKind"].as_str()
                == Some("catalog");
            if is_catalog_identity {
                let video_id = youtube_video_id(&request.candidate.url)
                    .ok_or("Selected YouTube candidate has no valid video identity")?;
                crate::metadata_normalization::normalize_catalog_live_download(
                    &mut request.track,
                    &request.candidate.title,
                    &request.candidate.url,
                    &video_id,
                    request.candidate.artwork_url.as_deref(),
                    request.candidate.duration_seconds,
                )?;
            } else {
                crate::metadata_normalization::normalize_selected_track(
                    &mut request.track,
                    &request.candidate.title,
                    &request.candidate.url,
                );
            }
        }
        if request.track.provenance["sourceKind"] == "audioSearch" {
            request.track.id = format!("{}:{}", request.track.provider, request.track.provider_id);
        }
        if request.track.id != format!("{}:{}", request.track.provider, request.track.provider_id) {
            return Err("Track identity must be a canonical provider identity".into());
        }
        if let Some(ids) = request.chapter_ids.as_ref().filter(|ids| !ids.is_empty()) {
            let chapters = request
                .candidate
                .chapters
                .as_ref()
                .ok_or("Inspect source chapters before requesting a chapter download")?;
            if ids
                .iter()
                .any(|id| !chapters.iter().any(|chapter| &chapter.id == id))
                || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
            {
                return Err("Chapter selection is invalid; inspect the source again".into());
            }
        }
        if request.candidate.provider == "slskd" {
            let source = request
                .candidate
                .source
                .as_ref()
                .ok_or("Soulseek candidate is missing its typed source")?;
            let CandidateSource::Soulseek { filename, .. } = source;
            crate::slskd::validate_remote_path(filename)?;
            if self.inner.connections.slskd()?.is_none() {
                return Err("Enable and configure slskd in Connections first".into());
            }
        } else {
            validate_source_url(&request.candidate.url, &request.candidate.provider)?;
        }
        if request.candidate.permission == "restricted" {
            return Err("This source is restricted and cannot be acquired".into());
        }
        if settings.music_directory.trim().is_empty() {
            return Err("Configure a music directory before downloading".into());
        }
        if !["source", "mp3"].contains(&request.output_profile.as_str()) {
            return Err("Unknown output profile".into());
        }
        let id = Uuid::new_v4().to_string();
        let mut candidate = request.candidate;
        if let Some(CandidateSource::Soulseek {
            batch_id,
            search_id,
            transfer_id,
            ..
        }) = candidate.source.as_mut()
        {
            *batch_id = Some(id.clone());
            // Search sessions are cleaned up as soon as the result set is final;
            // the selected peer path can be downloaded independently afterwards.
            *search_id = None;
            *transfer_id = None;
        }
        let job = DownloadJob {
            id,
            track: request.track,
            candidate,
            state: "queued".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: Utc::now().to_rfc3339(),
            output_profile: request.output_profile,
            format_id: request.format_id,
            chapter_ids: request.chapter_ids,
            soulseek: None,
        };
        self.start(job, settings).await
    }
    async fn start(&self, job: DownloadJob, settings: Settings) -> Result<DownloadJob, String> {
        let (tx, rx) = idle_cancel();
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "Queue lock poisoned")?;
            if active.contains_key(&job.id) {
                return Err("Job is already active".into());
            }
            for other in self.inner.store.list_jobs()? {
                if other.id != job.id
                    && !["completed", "failed", "cancelled", "interrupted"]
                        .contains(&other.state.as_str())
                    && other.candidate.provider == job.candidate.provider
                    && if job.candidate.provider == "slskd" {
                        other.candidate.id == job.candidate.id
                    } else {
                        other.candidate.url == job.candidate.url
                    }
                {
                    return Err("This source already has an active acquisition job".into());
                }
            }
            self.update(&job)?;
            active.insert(job.id.clone(), tx);
        }
        let manager = self.clone();
        let work = job.clone();
        tokio::spawn(async move {
            manager.work(work, settings, rx).await;
        });
        Ok(job)
    }
    pub async fn cancel(&self, job_id: &str) -> Result<(), String> {
        let job = self.inner.store.get_job(job_id)?;
        if [
            "validating",
            "tagging",
            "publishing",
            "finalizing",
            "completed",
        ]
        .contains(&job.state.as_str())
        {
            return Err(
                "Publication has started; the completed file must be managed from the library"
                    .into(),
            );
        }
        self.inner
            .active
            .lock()
            .map_err(|_| "Queue lock poisoned")?
            .get(job_id)
            .ok_or("Job is not active")?
            .send(true)
            .map_err(|_| "Job has already stopped".to_string())
    }
    pub async fn retry(&self, job_id: &str, settings: Settings) -> Result<DownloadJob, String> {
        let mut job = self.inner.store.get_job(job_id)?;
        if !can_retry(&job.state) {
            return Err("Only failed, cancelled or interrupted jobs can be retried".into());
        }
        if self
            .inner
            .active
            .lock()
            .map_err(|_| "Queue lock poisoned")?
            .contains_key(job_id)
        {
            return Err("Job is already active".into());
        }
        if let Some(CandidateSource::Soulseek { search_id, .. }) = job.candidate.source.as_mut() {
            *search_id = None;
        }
        if job.candidate.provider == "slskd" {
            let connection = self
                .inner
                .connections
                .slskd()?
                .ok_or("slskd is disabled in Connections")?;
            let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
            if let Some(source) = job.candidate.source.as_mut() {
                client.prepare_retry(source).await?;
            }
        }
        job.soulseek = None;
        job.state = "queued".into();
        job.progress = 0.0;
        job.error = None;
        job.output_path = None;
        self.start(job, settings).await
    }
    async fn work(&self, mut job: DownloadJob, settings: Settings, rx: watch::Receiver<bool>) {
        let result = self
            .acquire_with_fallback(&mut job, &settings, rx.clone())
            .await;
        match result {
            Ok(asset) => {
                job.output_path = Some(asset.path.clone());
                match self.inner.store.save_asset(&asset) {
                    Ok(()) => {
                        if let Err(e) = self.inner.store.add_asset_to_downloads(&asset) {
                            job.state = "failed".into();
                            job.error = Some(format!(
                                "File and library record were saved at {}, but Downloads membership failed: {e}",
                                asset.path
                            ));
                        } else {
                            self.autotag_download(&asset);
                            if let Some(publication_id) =
                                asset.track.provenance["publicationJobId"].as_str()
                            {
                                let _ = media::acknowledge_publication(&settings, publication_id);
                            }
                            job.state = "completed".into();
                            job.progress = 100.0;
                        }
                    }
                    Err(e) => {
                        job.state = "failed".into();
                        job.error = Some(format!(
                            "File published at {} but library record failed: {e}",
                            asset.path
                        ));
                    }
                }
            }
            Err(e) => {
                job.state = if *rx.borrow() || e == "Cancelled" {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                job.error = Some(e);
            }
        }
        let _ = self.update(&job);
        if let Ok(mut active) = self.inner.active.lock() {
            active.remove(&job.id);
        }
    }
    async fn acquire_with_fallback(
        &self,
        job: &mut DownloadJob,
        settings: &Settings,
        mut rx: watch::Receiver<bool>,
    ) -> Result<LocalAsset, String> {
        let original_track = job.track.clone();
        let original_provider = job.candidate.provider.clone();
        let lossless = crate::source_fallback::requires_lossless(job);
        let mut expected = original_track.clone();
        expected.duration_seconds = expected.duration_seconds.or(job.candidate.duration_seconds);
        let first_error = match self.acquire(job, settings, rx.clone()).await {
            Ok(asset) => return Ok(asset),
            Err(error) => error,
        };
        let lossless = lossless || crate::source_fallback::requires_lossless(job);
        if !crate::source_fallback::may_switch(job, &first_error, settings, *rx.borrow()) {
            return Err(first_error);
        }
        let providers = crate::source_fallback::provider_order(
            &original_provider,
            true,
            &settings.download_fallback_sources,
            true,
        );
        let mut failures = vec![
            serde_json::json!({"provider":original_provider,"candidateId":job.candidate.id,"error":first_error}),
        ];
        for provider in providers {
            if *rx.borrow() {
                return Err("Cancelled".into());
            }
            let ids = vec![provider.clone()];
            let found = tokio::select! {
                found = self.find_candidates(&expected, &ids, settings) => found,
                _ = rx.changed() => return Err("Cancelled".into()),
            };
            let candidates = match found {
                Ok(found) => found,
                Err(error) => {
                    failures.push(serde_json::json!({"provider":provider,"error":error}));
                    continue;
                }
            };
            let mut attempted = false;
            for candidate in candidates
                .into_iter()
                .filter(|c| {
                    c.provider == provider
                        && crate::source_fallback::recording_matches(&expected, c)
                })
                .take(2)
            {
                let mut candidate = if provider == "slskd" {
                    candidate
                } else {
                    match inspect_with_cancel(&expected, &candidate, settings, rx.clone()).await {
                        Ok(inspected) => inspected,
                        Err(error) => {
                            if *rx.borrow() || error == "Cancelled" {
                                return Err("Cancelled".into());
                            }
                            failures.push(serde_json::json!({"provider":provider,"candidateId":candidate.id,"error":error}));
                            continue;
                        }
                    }
                };
                if candidate.permission == "restricted" {
                    continue;
                }
                let format_id = if lossless && !settings.download_fallback_allow_lossy {
                    let Some(format) = candidate.formats.iter().find(|f| f.lossless) else {
                        continue;
                    };
                    Some(format.id.clone())
                } else {
                    None
                };
                let staging = PathBuf::from(&settings.music_directory)
                    .join(".burrata-staging")
                    .join(&job.id);
                match tokio::fs::remove_dir_all(&staging).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("Cannot clear failed source staging: {error}"))
                    }
                }
                if let Some(CandidateSource::Soulseek {
                    batch_id,
                    search_id,
                    transfer_id,
                    ..
                }) = candidate.source.as_mut()
                {
                    *batch_id = Some(Uuid::new_v4().to_string());
                    *search_id = None;
                    *transfer_id = None;
                }
                candidate
                    .reasons
                    .push(format!("Automatic fallback from {original_provider}"));
                job.track = original_track.clone();
                if !job.track.provenance.is_object() {
                    job.track.provenance = serde_json::json!({});
                }
                job.track.provenance["downloadFallbackAttempts"] = serde_json::json!(failures);
                job.track.provenance["downloadFallbackRequiresLossless"] =
                    serde_json::json!(lossless && !settings.download_fallback_allow_lossy);
                job.candidate = candidate;
                job.format_id = format_id;
                job.soulseek = None;
                job.state = "queued".into();
                job.progress = 0.0;
                job.error = None;
                self.update(job)?;
                attempted = true;
                match self.acquire(job, settings, rx.clone()).await {
                    Ok(asset) => {
                        job.error = None;
                        return Ok(asset);
                    }
                    Err(error) => {
                        if !crate::source_fallback::may_switch(job, &error, settings, *rx.borrow())
                        {
                            return Err(error);
                        }
                        failures.push(serde_json::json!({"provider":provider,"candidateId":job.candidate.id,"error":error}));
                    }
                }
            }
            if !attempted {
                failures.push(serde_json::json!({"provider":provider,"error":"No permitted recording match meeting the fallback quality policy"}));
            }
        }
        if !job.track.provenance.is_object() {
            job.track.provenance = serde_json::json!({});
        }
        job.track.provenance["downloadFallbackAttempts"] = serde_json::json!(failures);
        Err(format!("{first_error}; configured download fallbacks exhausted (no successful recording/quality match)"))
    }

    async fn acquire(
        &self,
        job: &mut DownloadJob,
        settings: &Settings,
        mut rx: watch::Receiver<bool>,
    ) -> Result<LocalAsset, String> {
        if *rx.borrow() {
            return Err("Cancelled".into());
        }
        let mut admission = download_admission(
            &job.candidate.provider,
            &self.inner.slots,
            &self.inner.soulseek_slots,
            &mut rx,
        )
        .await?;
        if let Some(existing) = self
            .inner
            .store
            .list_assets()?
            .into_iter()
            .find(|a| Path::new(&a.path).is_file() && is_preserved_source(a, &job.candidate))
        {
            admission
                .begin_processing(&self.inner.slots, &mut rx)
                .await?;
            media::verify_asset(&existing).await?;
            if existing.recording_id != self.inner.store.recording_id(&job.track.id)? {
                return Err(format!(
                    "This source already exists at {} for another catalog entry; review its recording identity before acquiring it again",
                    existing.path
                ));
            }
            if job.track.provenance["downloadFallbackRequiresLossless"] == true {
                media::verify_lossless_fallback(Path::new(&existing.path), settings).await?;
            }
            self.inner.store.add_asset_to_downloads(&existing)?;
            job.output_path = Some(existing.path.clone());
            {
                let mut active = self
                    .inner
                    .active
                    .lock()
                    .map_err(|_| "Queue lock poisoned")?;
                if *rx.borrow() {
                    return Err("Cancelled".into());
                }
                active.remove(&job.id);
                job.state = "publishing".into();
                self.update(job)?;
            }
            return if job.output_profile == "mp3" {
                media::export_mp3(&existing, settings).await
            } else {
                Ok(existing)
            };
        }
        job.state = "downloading".into();
        self.update(job)?;
        // Re-extract formats immediately before acquisition; never reuse expired stream URLs.
        if job.candidate.provider != "slskd" {
            job.candidate =
                inspect_with_cancel(&job.track, &job.candidate, settings, rx.clone()).await?;
        }
        if *rx.borrow() {
            return Err("Cancelled".into());
        }
        if job.candidate.permission == "restricted" {
            return Err("Source now reports restricted access".into());
        }
        let mut provenance = job
            .track
            .provenance
            .as_object()
            .cloned()
            .unwrap_or_default();
        provenance.insert(
            "acquisition".into(),
            serde_json::json!({
                "provider": job.candidate.provider, "url": job.candidate.url,
                "id": job.candidate.id, "permission": job.candidate.permission,
                "formatId": job.format_id,
                "source": job.candidate.source,
            }),
        );
        job.track.provenance = Value::Object(provenance);
        let staging = PathBuf::from(&settings.music_directory)
            .join(".burrata-staging")
            .join(&job.id);
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| format!("Cannot create staging directory: {e}"))?;
        let chapter_source_key = if job.chapter_ids.as_ref().is_some_and(|ids| !ids.is_empty())
            && job.candidate.provider != "slskd"
        {
            Some(
                serde_json::json!([
                    "chapter-source-v1",
                    job.candidate.provider,
                    job.candidate.url,
                    job.format_id.as_deref().unwrap_or("bestaudio"),
                    settings.music_directory,
                ])
                .to_string(),
            )
        } else {
            None
        };
        let _chapter_source_gate = if let Some(key) = chapter_source_key.as_deref() {
            Some(self.inner.chapter_source_cache.gate(key).await?)
        } else {
            None
        };
        let cached_chapter_source = if let Some(key) = chapter_source_key.as_deref() {
            self.inner
                .chapter_source_cache
                .copy_to_staging(key, &staging)
                .await
        } else {
            None
        };
        let used_cached_chapter_source = cached_chapter_source.is_some();
        let path = if let Some(cached) = cached_chapter_source {
            cached
        } else if job.candidate.provider == "slskd" {
            let connection = self
                .inner
                .connections
                .slskd()?
                .ok_or("slskd is disabled in Connections")?;
            let source = job
                .candidate
                .source
                .clone()
                .ok_or("Soulseek candidate is missing its typed source")?;
            let (username, batch_id) = match &source {
                CandidateSource::Soulseek {
                    username, batch_id, ..
                } => (
                    username.clone(),
                    batch_id.clone().unwrap_or_else(|| job.id.clone()),
                ),
            };
            let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
            crate::slskd::wait_for_batch_file(
                &client,
                &batch_id,
                &username,
                &source,
                Path::new(&connection.downloads_path),
                &staging,
                &mut rx,
                |progress, transfer_id, transfer_state, details| {
                    job.soulseek = Some(details.clone());
                    if let Some(transfer_id) = transfer_id {
                        if let Some(CandidateSource::Soulseek {
                            transfer_id: stored,
                            ..
                        }) = job.candidate.source.as_mut()
                        {
                            if stored.as_deref() != Some(transfer_id.as_str()) {
                                *stored = Some(transfer_id);
                            }
                        }
                    }
                    if ["waiting_for_peer", "queued", "stalled"].contains(&transfer_state) {
                        job.state = transfer_state.to_owned();
                    } else {
                        job.state = "downloading".into();
                    }
                    job.progress = progress;
                    let _ = self.update(job);
                },
            )
            .await?
        } else if matches!(job.candidate.provider.as_str(), "direct" | "monochrome") {
            download_direct(&job.candidate.url, &staging, rx.clone()).await?
        } else {
            let format = if let Some(id) = &job.format_id {
                if !job.candidate.formats.iter().any(|f| &f.id == id) {
                    return Err(
                        "Selected audio format is no longer available; inspect and choose again"
                            .into(),
                    );
                }
                id.clone()
            } else {
                "bestaudio".into()
            };
            let mut args = base_args(settings);
            add_youtube_support_args(&mut args, &job.candidate.provider).await?;
            args.extend([
                "--no-playlist".into(),
                "--no-simulate".into(),
                "--newline".into(),
                "--progress".into(),
                "--progress-delta".into(),
                "0.5".into(),
                "--progress-template".into(),
                "download:BURRATA_PROGRESS %(progress)j".into(),
                "--print".into(),
                "after_move:BURRATA_FILE %(filepath)j".into(),
                "--retries".into(),
                "3".into(),
                "--fragment-retries".into(),
                "3".into(),
                "--socket-timeout".into(),
                "20".into(),
                "-f".into(),
                format,
                "-x".into(),
                "--audio-format".into(),
                "best".into(),
                "-o".into(),
                staging
                    .join("source.%(ext)s")
                    .to_string_lossy()
                    .into_owned(),
                "--".into(),
                job.candidate.url.clone(),
            ]);
            let ffmpeg = crate::tools::executable(&settings.ffmpeg_path, "ffmpeg");
            if let Some(location) = ffmpeg_location(&ffmpeg.to_string_lossy()) {
                // Insert before the URL separator so configuration is never treated as input URLs.
                let position = args.len() - 2;
                args.splice(position..position, ["--ffmpeg-location".into(), location]);
            }
            let raw = run_tool(
                settings,
                args,
                rx.clone(),
                Duration::from_secs(3600),
                |line| {
                    if let Some(raw) = line.strip_prefix("BURRATA_PROGRESS ") {
                        if let Ok(progress) = serde_json::from_str::<Value>(raw) {
                            let total = progress["total_bytes"]
                                .as_f64()
                                .or_else(|| progress["total_bytes_estimate"].as_f64());
                            if let (Some(n), Some(d)) =
                                (progress["downloaded_bytes"].as_f64(), total)
                            {
                                if d > 0.0 {
                                    job.progress = (n / d * 90.0).clamp(0.0, 90.0);
                                    let _ = self.update(job);
                                }
                            }
                        }
                    }
                },
            )
            .await?;
            let filename = raw
                .lines()
                .filter_map(|line| line.strip_prefix("BURRATA_FILE "))
                .filter_map(|s| serde_json::from_str::<String>(s).ok())
                .last()
                .ok_or("Downloader did not report a completed audio file")?;
            let path = PathBuf::from(filename);
            let real = tokio::fs::canonicalize(&path)
                .await
                .map_err(|e| e.to_string())?;
            let root = tokio::fs::canonicalize(&staging)
                .await
                .map_err(|e| e.to_string())?;
            if !real.starts_with(root) {
                return Err("Downloader returned a file outside its staging directory".into());
            }
            real
        };
        if job.track.provenance["downloadFallbackRequiresLossless"] == true {
            media::verify_lossless_fallback(&path, settings).await?;
        }
        job.state = "validating".into();
        self.update(job)?;
        admission
            .begin_processing(&self.inner.slots, &mut rx)
            .await?;
        if !used_cached_chapter_source {
            if let Some(key) = chapter_source_key.as_deref() {
                self.inner
                    .chapter_source_cache
                    .insert_from_staging(key, &path, &staging, Path::new(&settings.music_directory))
                    .await;
            }
        }
        drop(_chapter_source_gate);
        let selected_chapters = if let Some(ids) =
            job.chapter_ids.clone().filter(|ids| !ids.is_empty())
        {
            let chapters = job
                .candidate
                .chapters
                .clone()
                .ok_or("Selected source has no inspected chapter metadata")?;
            let assets = self.inner.store.list_assets()?;
            let mut selected = Vec::new();
            for chapter_id in ids {
                let chapter = chapters
                    .iter()
                    .find(|chapter| chapter.id == chapter_id)
                    .cloned()
                    .ok_or("Selected chapter is no longer available; inspect the source again")?;
                let existing = assets
                    .iter()
                    .find(|asset| {
                        asset.track.provenance["chapter"]["jobId"] == job.id
                            && asset.track.provenance["chapter"]["id"] == chapter.id
                            && Path::new(&asset.path).is_file()
                            && if job.output_profile == "mp3" {
                                asset.track.provenance.get("derivedFromAssetId").is_some()
                            } else {
                                asset.track.provenance.get("derivedFromAssetId").is_none()
                            }
                    })
                    .cloned();
                let segment = if existing.is_none() {
                    if *rx.borrow() {
                        return Err("Cancelled".into());
                    }
                    Some(
                        media::split_chapter(&path, &chapter, settings, &job.id, selected.len())
                            .await?,
                    )
                } else {
                    None
                };
                selected.push((chapter, segment, existing));
            }
            Some(selected)
        } else {
            None
        };
        // Make the cancellation boundary atomic with cancel(): after publication starts,
        // removing the signal prevents a racing request from reporting false cancellation.
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "Queue lock poisoned")?;
            if *rx.borrow() {
                return Err("Cancelled".into());
            }
            active.remove(&job.id);
            job.state = "validating".into();
            job.progress = 92.0;
            self.update(job)?;
        }
        if let Some(selected) = selected_chapters {
            let mut assets = Vec::new();
            for (position, (chapter, segment, existing)) in selected.into_iter().enumerate() {
                if let Some(existing) = existing {
                    self.inner.store.add_asset_to_downloads(&existing)?;
                    if position == 0 {
                        job.output_path = Some(existing.path.clone());
                    }
                    assets.push(existing);
                    continue;
                }
                let segment = segment.ok_or("Chapter segment staging failed")?;
                let mut chapter_track = job.track.clone();
                let parent_title = chapter_track.title.clone();
                let source_label = chapter.title.clone();
                let (label_artist, label_title, _) = chapter_artist_title(&source_label);
                let catalog_suggestion =
                    verified_chapter_suggestion(&chapter, job.track.version.as_deref());
                let artist_source = if catalog_suggestion.is_some() {
                    "exact_unique_catalog_match"
                } else if !label_artist.is_empty() {
                    "chapter_label"
                } else {
                    "unknown"
                };
                let (chapter_track_id, chapter_provider_id) =
                    chapter_track_identity(&job.track.id, &chapter.id);
                chapter_track.id = chapter_track_id;
                chapter_track.provider = "chapter".into();
                chapter_track.provider_id = chapter_provider_id;
                chapter_track.source_url = job.candidate.url.clone();
                chapter_track.artist = catalog_suggestion
                    .map(|track| track.artist.clone())
                    .unwrap_or(label_artist);
                chapter_track.title = catalog_suggestion
                    .map(|track| track.title.clone())
                    .unwrap_or(label_title);
                chapter_track.version = None;
                chapter_track.album = Some(parent_title.clone());
                chapter_track.label = None;
                chapter_track.genre = catalog_suggestion.and_then(|track| track.genre.clone());
                chapter_track.normalized_genre = None;
                chapter_track.release_date =
                    catalog_suggestion.and_then(|track| track.release_date.clone());
                chapter_track.catalog_number = None;
                chapter_track.duration_seconds = Some(chapter.end_seconds - chapter.start_seconds);
                chapter_track.isrc = None;
                chapter_track.bpm = None;
                chapter_track.key = None;
                chapter_track.provenance["chapter"] = serde_json::json!({
                        "id": chapter.id,
                        "jobId": job.id,
                        "parentTrackId": job.track.id,
                        "parentTitle": parent_title,
                        "startSeconds": chapter.start_seconds,
                        "endSeconds": chapter.end_seconds,
                        "label": source_label,
                        "confidence": chapter.confidence,
                        "identity": "unverified chapter-derived recording",
                        "artistSource": artist_source,
                "weakFields": {"artist": artist_source == "chapter_label", "title": true},
                        "artistIdentity": "excerpt identity remains distinct from any full recording match",
                        "catalogSuggestion": catalog_suggestion.map(|track| serde_json::json!({
                            "provider": track.provider,
                            "recordingId": track.provider_id,
                            "confidence": "exact_unique_catalog_match"
                        }))
                    });
                let mut chapter_candidate = job.candidate.clone();
                chapter_candidate.duration_seconds = chapter_track.duration_seconds;
                let publication_id = format!("{}-chapter-{}", job.id, position);
                let asset = media::finalize_download(
                    &segment,
                    &chapter_track,
                    &chapter_candidate,
                    settings,
                    "source",
                    &publication_id,
                )
                .await?;
                self.inner.store.save_asset(&asset).map_err(|e| {
                    format!(
                        "Chapter published at {} but library record failed: {e}",
                        asset.path
                    )
                })?;
                self.inner
                    .store
                    .add_asset_to_downloads(&asset)
                    .map_err(|e| {
                        format!(
                            "Chapter was saved at {}, but Downloads membership failed: {e}",
                            asset.path
                        )
                    })?;
                media::acknowledge_publication(settings, &publication_id)?;
                let result_asset = if job.output_profile == "mp3" {
                    let exported = media::export_mp3(&asset, settings).await?;
                    self.inner.store.save_asset(&exported).map_err(|e| {
                        format!(
                            "MP3 published at {} but library record failed: {e}",
                            exported.path
                        )
                    })?;
                    self.inner
                        .store
                        .add_asset_to_downloads(&exported)
                        .map_err(|e| {
                            format!(
                                "MP3 was saved at {}, but Downloads membership failed: {e}",
                                exported.path
                            )
                        })?;
                    exported
                } else {
                    asset
                };
                if position == 0 {
                    job.output_path = Some(result_asset.path.clone());
                }
                self.autotag_download(&result_asset);
                assets.push(result_asset);
                job.progress = 92.0
                    + (position + 1) as f64 / job.chapter_ids.as_ref().unwrap().len() as f64 * 8.0;
                let _ = self.update(job);
            }
            if let Ok(analysis) = self.inner.analysis.lock() {
                if let Some(analysis) = analysis.as_ref() {
                    let asset_ids = assets.iter().map(|asset| asset.id.clone()).collect();
                    let _ = analysis.queue(Some(asset_ids), false);
                }
            }
            return assets
                .into_iter()
                .next()
                .ok_or_else(|| "No chapters selected".into());
        }
        let source = media::finalize_download(
            &path,
            &job.track,
            &job.candidate,
            settings,
            "source",
            &job.id,
        )
        .await?;
        self.inner.store.save_asset(&source).map_err(|e| {
            format!(
                "File published at {} but source library record failed: {e}",
                source.path
            )
        })?;
        self.inner
            .store
            .add_asset_to_downloads(&source)
            .map_err(|e| {
                format!(
                    "File was saved at {}, but Downloads membership failed: {e}",
                    source.path
                )
            })?;
        job.output_path = Some(source.path.clone());
        job.track = source.track.clone();
        media::acknowledge_publication(settings, &job.id)?;
        if job.output_profile == "mp3" {
            job.state = "publishing".into();
            job.progress = 96.0;
            self.update(job)?;
            media::export_mp3(&source, settings)
                .await
                .map_err(|e| format!("Source retained at {}; MP3 export failed: {e}", source.path))
        } else {
            Ok(source)
        }
    }
}

fn slskd_availability_order(candidate: &Candidate) -> (u8, u8, u32, std::cmp::Reverse<u64>) {
    let free_slot = candidate
        .reasons
        .iter()
        .any(|r| r == "availability:free-upload-slot");
    let queued = candidate
        .reasons
        .iter()
        .any(|r| r == "availability:queued-remotely");
    let rank = if free_slot {
        0
    } else if queued {
        2
    } else {
        1
    };
    let queue = candidate
        .reasons
        .iter()
        .find_map(|reason| {
            reason
                .strip_prefix("availability:queue-length:")?
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(u32::MAX);
    // A free-slot hint must not hide a queue of thousands of files.
    let queue_band = match queue {
        0..=100 => 0,
        101..=1000 => 1,
        1001..=u32::MAX => 2,
    };
    let speed = candidate
        .reasons
        .iter()
        .find_map(|reason| {
            reason
                .strip_prefix("availability:upload-speed:")?
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0);
    (queue_band, rank, queue, std::cmp::Reverse(speed))
}

fn slskd_peer_failure_order(candidate: &Candidate, history: &[DownloadJob]) -> (bool, usize) {
    let Some(CandidateSource::Soulseek { username, .. }) = candidate.source.as_ref() else {
        return (false, 0);
    };
    let mut restricted = false;
    let mut failures = 0;
    // Only the most recent few outcomes affect ranking, so old failures do not
    // permanently blacklist a peer. Application shutdown is not a peer failure.
    for job in history.iter().filter(|job| {
        job.candidate.provider == "slskd" && matches!(job.candidate.source.as_ref(), Some(CandidateSource::Soulseek { username: peer, .. }) if peer == username)
    }).take(5) {
        let error = job.error.as_deref().unwrap_or_default().to_ascii_lowercase();
        if job.state == "failed" && !error.contains("application shut down") {
            failures += 1;
            restricted |= error.contains("banned") || error.contains("sharing") || error.contains("rejected");
        }
    }
    (restricted, failures)
}

fn slskd_candidate_track(candidate: &Candidate) -> Track {
    let provider_id = candidate
        .id
        .strip_prefix("slskd:")
        .unwrap_or(&candidate.id)
        .to_owned();
    let (artist, title) = split_remote_title(&candidate.title);
    let mut track = Track {
        id: format!("slskd:{provider_id}"),
        provider: "slskd".into(),
        provider_id,
        source_url: String::new(),
        artist,
        title,
        duration_seconds: candidate.duration_seconds,
        ..Default::default()
    };
    track.provenance = serde_json::json!({
        "identity": "unverified peer filename",
        "artistSource": if track.artist.is_empty() { "unknown" } else { "filename" },
        "uploader": candidate.uploader,
        "audioCandidate": candidate,
        "sourceProvider": "slskd",
    });
    track
}

fn split_remote_title(value: &str) -> (String, String) {
    let mut parts = value.split(" - ");
    let first = parts.next().unwrap_or_default().trim();
    let second = parts.next().unwrap_or_default().trim();
    if !first.is_empty() && !second.is_empty() && parts.next().is_none() {
        (first.to_owned(), second.to_owned())
    } else {
        (String::new(), value.trim().to_owned())
    }
}

#[async_trait::async_trait]
impl AcquisitionProvider for AcquisitionManager {
    async fn find_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Candidate>, String> {
        let web_providers = provider_ids
            .iter()
            .filter(|id| matches!(id.as_str(), "youtube" | "soundcloud" | "monochrome"))
            .cloned()
            .collect::<Vec<_>>();
        let wants_slskd = provider_ids.iter().any(|id| id == "slskd");
        let mut found = Vec::new();
        let mut errors = Vec::new();
        let selected_slskd = track
            .provenance
            .get("audioCandidate")
            .and_then(|value| serde_json::from_value::<Candidate>(value.clone()).ok())
            .filter(|candidate| candidate.provider == "slskd" && candidate.source.is_some());
        if let Some(candidate) = &selected_slskd {
            found.push(candidate.clone());
        }
        if !web_providers.is_empty() {
            let key = search_cache_key(track, &web_providers, settings, 10);
            match self
                .inner
                .search_cache
                .get_or_search(key, || find_candidates(track, &web_providers, settings))
                .await
            {
                Ok(candidates) => found.extend(candidates),
                Err(error) => errors.push(error),
            }
        }
        if wants_slskd {
            let slskd_key = search_cache_key(track, &["slskd".into()], settings, 10);
            match self
                .inner
                .search_cache
                .get_or_search(slskd_key, || self.find_slskd_candidates_for_track(track))
                .await
            {
                Ok(candidates) => found.extend(candidates),
                Err(error) => errors.push(error),
            }
        }
        if found.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let history = self.inner.store.list_jobs()?;
        found.sort_by(|a, b| {
            b.score.total_cmp(&a.score).then_with(|| {
                if a.provider == "slskd" && b.provider == "slskd" {
                    slskd_peer_failure_order(a, &history)
                        .cmp(&slskd_peer_failure_order(b, &history))
                        .then_with(|| slskd_availability_order(a).cmp(&slskd_availability_order(b)))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        let mut ids = std::collections::HashSet::new();
        found.retain(|candidate| ids.insert(candidate.id.clone()));
        Ok(found)
    }
    async fn inspect_formats(
        &self,
        track: &Track,
        candidate: &Candidate,
        settings: &Settings,
    ) -> Result<Candidate, String> {
        if candidate.provider == "slskd" {
            if candidate.source.is_none() {
                return Err("Soulseek candidate is missing its typed source".into());
            }
            Ok(candidate.clone())
        } else {
            inspect_candidate(track, candidate, settings).await
        }
    }
    async fn download(
        &self,
        request: DownloadRequest,
        settings: Settings,
    ) -> Result<DownloadJob, String> {
        self.enqueue(request, settings).await
    }
}

fn resumable_soulseek_job(job: &DownloadJob) -> bool {
    job.state == "interrupted"
        && job.candidate.provider == "slskd"
        && job.candidate.permission != "restricted"
        && matches!(
            job.candidate.source.as_ref(),
            Some(CandidateSource::Soulseek { .. })
        )
}

fn can_retry(state: &str) -> bool {
    ["failed", "cancelled", "interrupted"].contains(&state)
}

async fn download_direct(
    raw: &str,
    staging: &Path,
    mut cancel: watch::Receiver<bool>,
) -> Result<PathBuf, String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let mut url = validate_source_url(raw, "direct")?;
    let mut response = None;
    for _ in 0..=5 {
        let addresses = public_addresses(&url).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(url.host_str().ok_or("Missing host")?, &addresses)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(1800))
            .build()
            .map_err(|e| e.to_string())?;
        let value = tokio::select! {
            value=client.get(url.clone()).send()=>value.map_err(|e|format!("Direct download failed: {e}"))?,
            _=cancel.changed()=>return Err("Cancelled".into()),
        };
        if value.status().is_redirection() {
            let next = value
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|x| x.to_str().ok())
                .ok_or("Invalid download redirect")?;
            url = validate_source_url(
                url.join(next).map_err(|e| e.to_string())?.as_str(),
                "direct",
            )?;
        } else {
            response = Some(value.error_for_status().map_err(|e| e.to_string())?);
            break;
        }
    }
    let response = response.ok_or("Too many direct download redirects")?;
    if response
        .content_length()
        .is_some_and(|x| x > 2 * 1024 * 1024 * 1024)
    {
        return Err("Audio file exceeds 2 GB limit".into());
    }
    let extension = url
        .path()
        .rsplit('.')
        .next()
        .filter(|x| ["mp3", "flac", "opus", "m4a", "aac", "wav", "aiff", "ogg"].contains(x))
        .unwrap_or("audio");
    let path = staging.join(format!("source.{extension}"));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut size = 0u64;
    loop {
        let chunk = tokio::select! { value=stream.next()=>value, _=cancel.changed()=>return Err("Cancelled".into()) };
        match chunk {
            Some(Ok(bytes)) => {
                size += bytes.len() as u64;
                if size > 2 * 1024 * 1024 * 1024 {
                    return Err("Audio file exceeds 2 GB limit".into());
                }
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
            }
            Some(Err(e)) => return Err(e.to_string()),
            None => break,
        }
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_cache_key_tracks_source_and_effective_tool_profile() {
        let mut candidate = Candidate {
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=a".into(),
            ..Default::default()
        };
        let settings = Settings::default();
        let original = inspection_cache_key(&candidate, &settings);
        candidate.url = "https://youtube.com/watch?v=b".into();
        assert_ne!(original, inspection_cache_key(&candidate, &settings));
        candidate.url = "https://youtube.com/watch?v=a".into();
        let mut changed = settings.clone();
        changed.js_runtime_path = "deno".into();
        assert_ne!(original, inspection_cache_key(&candidate, &changed));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cached_inspection_is_singleflight_and_rescored_for_each_track() {
        use std::os::unix::fs::PermissionsExt;
        let cache = InspectionCache::default();
        let directory = tempfile::tempdir().unwrap();
        let counter = directory.path().join("inspection-count");
        let executable = directory.path().join("fake-yt-dlp");
        let output = r#"{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist - Night Drive","duration":180}"#;
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\necho inspected >> '{}'\necho '{}'\n",
                counter.display(),
                output
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Default::default()
        };
        let candidate = Candidate {
            id: "youtube:x".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=x".into(),
            ..Default::default()
        };
        let key = inspection_cache_key(&candidate, &settings);
        let load = || inspect_ytdlp_metadata(&candidate, &settings, idle_cancel().1);
        let mut first = cache.get_or_inspect(key.clone(), load).await.unwrap();
        let mut second = cache.get_or_inspect(key, load).await.unwrap();
        let matching_track = Track {
            artist: "Artist".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let other_track = Track {
            artist: "Different Artist".into(),
            title: "Different Song".into(),
            ..Default::default()
        };
        matching::score_candidate(&matching_track, &mut first);
        matching::score_candidate(&other_track, &mut second);
        assert!(first.score > second.score);
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 1);
    }

    #[tokio::test]
    async fn waiting_peers_do_not_consume_processing_workers() {
        let processing = Semaphore::new(2);
        let peers = Semaphore::new(SOULSEEK_PENDING_LIMIT);
        let (_tx, mut cancel) = watch::channel(false);
        let mut admissions = Vec::new();
        for _ in 0..20 {
            admissions.push(
                download_admission("slskd", &processing, &peers, &mut cancel)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(processing.available_permits(), 2);
        let direct = download_admission("youtube", &processing, &peers, &mut cancel)
            .await
            .unwrap();
        assert_eq!(processing.available_permits(), 1);
        admissions[0]
            .begin_processing(&processing, &mut cancel)
            .await
            .unwrap();
        assert_eq!(processing.available_permits(), 0);
        assert_eq!(peers.available_permits(), SOULSEEK_PENDING_LIMIT - 19);
        drop(direct);
        drop(admissions);
        assert_eq!(processing.available_permits(), 2);
        assert_eq!(peers.available_permits(), SOULSEEK_PENDING_LIMIT);
    }

    #[tokio::test]
    async fn peer_monitor_limit_can_be_cancelled_without_leaking_slots() {
        let processing = Arc::new(Semaphore::new(2));
        let peers = Arc::new(Semaphore::new(1));
        let (_tx, mut first_cancel) = watch::channel(false);
        let first = download_admission("slskd", &processing, &peers, &mut first_cancel)
            .await
            .unwrap();
        let (tx, mut cancel) = watch::channel(false);
        let waiter = {
            let processing = processing.clone();
            let peers = peers.clone();
            tokio::spawn(async move {
                download_admission("slskd", &processing, &peers, &mut cancel)
                    .await
                    .map(|_| ())
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(processing.available_permits(), 2);
        tx.send(true).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        drop(first);
        assert_eq!(peers.available_permits(), 1);
    }

    #[test]
    fn startup_recovers_only_interrupted_authorized_soulseek_jobs() {
        let mut job = DownloadJob {
            id: "job".into(),
            track: Track::default(),
            candidate: Candidate {
                provider: "slskd".into(),
                permission: "unknown".into(),
                source: Some(CandidateSource::Soulseek {
                    username: "peer".into(),
                    filename: "song.flac".into(),
                    size_bytes: 123,
                    bitrate_kbps: None,
                    search_id: None,
                    batch_id: Some("job".into()),
                    transfer_id: None,
                }),
                ..Candidate::default()
            },
            state: "interrupted".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: String::new(),
            output_profile: "source".into(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
        };
        assert!(resumable_soulseek_job(&job));
        for state in ["failed", "cancelled", "completed"] {
            job.state = state.into();
            assert!(!resumable_soulseek_job(&job));
        }
        job.state = "interrupted".into();
        job.candidate.permission = "restricted".into();
        assert!(!resumable_soulseek_job(&job));
        job.candidate.permission = "unknown".into();
        job.candidate.provider = "youtube".into();
        assert!(!resumable_soulseek_job(&job));
    }

    #[tokio::test]
    async fn chapter_raw_source_is_copied_to_each_jobs_confined_staging() {
        let temp = tempfile::tempdir().unwrap();
        let music = temp.path().join("music");
        let first_stage = music.join(".burrata-staging").join("job-one");
        let second_stage = music.join(".burrata-staging").join("job-two");
        tokio::fs::create_dir_all(&first_stage).await.unwrap();
        tokio::fs::create_dir_all(&second_stage).await.unwrap();
        let source = first_stage.join("source.webm");
        tokio::fs::write(&source, b"raw audio bytes").await.unwrap();
        let cache = ChapterSourceCache::default();
        cache
            .insert_from_staging("candidate-format-dir", &source, &first_stage, &music)
            .await;
        let copied = cache
            .copy_to_staging("candidate-format-dir", &second_stage)
            .await
            .unwrap();
        assert!(copied.starts_with(tokio::fs::canonicalize(&second_stage).await.unwrap()));
        assert_eq!(tokio::fs::read(copied).await.unwrap(), b"raw audio bytes");
        assert!(cache
            .copy_to_staging("missing", &second_stage)
            .await
            .is_none());
    }

    #[test]
    fn slskd_free_slot_is_availability_tiebreaker_and_not_identity_score() {
        let free = Candidate {
            score: 0.4,
            reasons: vec![
                "availability:free-upload-slot".into(),
                "availability:queue-length:0".into(),
            ],
            ..Candidate::default()
        };
        let queued = Candidate {
            score: 0.99,
            reasons: vec![
                "availability:queued-remotely".into(),
                "availability:queue-length:5".into(),
            ],
            ..Candidate::default()
        };
        assert!(slskd_availability_order(&free) < slskd_availability_order(&queued));
        assert_eq!(free.score, 0.4);
        assert_eq!(queued.score, 0.99);
        let overloaded = Candidate {
            reasons: vec![
                "availability:free-upload-slot".into(),
                "availability:queue-length:10000".into(),
            ],
            ..Candidate::default()
        };
        assert!(slskd_availability_order(&queued) < slskd_availability_order(&overloaded));
    }

    #[test]
    fn rejected_peers_are_downranked_without_blaming_them_for_shutdown() {
        let candidate = Candidate {
            provider: "slskd".into(),
            score: 0.9,
            source: Some(CandidateSource::Soulseek {
                username: "peer".into(),
                filename: "song.mp3".into(),
                size_bytes: 100,
                bitrate_kbps: None,
                search_id: None,
                batch_id: None,
                transfer_id: None,
            }),
            ..Candidate::default()
        };
        // Old saved jobs did not have telemetry; deserialize one as a regression.
        let mut job: DownloadJob = serde_json::from_value(serde_json::json!({
            "id":"test", "track":Track::default(), "candidate":candidate,
            "state":"failed", "progress":0, "error":"Banned (Please consider sharing)",
            "outputPath":null, "createdAt":"", "outputProfile":"source",
        }))
        .unwrap();
        assert!(job.soulseek.is_none());
        assert_eq!(
            slskd_peer_failure_order(&candidate, &[job.clone()]),
            (true, 1)
        );
        job.error = Some("slskd transfer failed: Application shut down".into());
        assert_eq!(
            slskd_peer_failure_order(&candidate, &[job.clone()]),
            (false, 0)
        );
        job.state = "completed".into();
        assert_eq!(slskd_peer_failure_order(&candidate, &[job]), (false, 0));
        assert_eq!(candidate.score, 0.9);
    }
    #[test]
    fn parses_chapters_and_thumbnail_fallback_as_unverified_metadata() {
        let info = serde_json::json!({
            "id":"abc", "thumbnail":"javascript:alert(1)", "thumbnails":[{"url":"javascript:alert(1)"},{"url":"https://img.youtube.com/cover.jpg"}],
            "chapters":[{"start_time":0.0,"end_time":12.5,"title":"Intro"},{"start_time":12.5,"end_time":40.0,"title":" Song "},{"start_time":40.0,"end_time":30.0,"title":"bad"}]
        });
        let candidate = candidate_from_json(&info, "youtube");
        assert_eq!(
            candidate.artwork_url.as_deref(),
            Some("https://img.youtube.com/cover.jpg")
        );
        let chapters = candidate.chapters.unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].title, "Song");
        assert_eq!(chapters[1].confidence, "source_metadata_unverified");
    }
    #[test]
    fn chapter_label_artist_and_title_are_unverified_and_conservative() {
        assert_eq!(
            chapter_artist_title("Artist Name - Night Drive"),
            ("Artist Name".into(), "Night Drive".into(), "chapter_label")
        );
        assert_eq!(
            chapter_artist_title("Night Drive"),
            (String::new(), "Night Drive".into(), "unknown")
        );
    }
    #[test]
    fn chapter_metadata_is_used_only_for_a_valid_unique_catalog_match() {
        let track = Track {
            id: "musicbrainz:recording-id".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-id".into(),
            source_url: "https://musicbrainz.org/recording/recording-id".into(),
            artist: "Verified Artist Credit".into(),
            title: "Verified Song Title".into(),
            isrc: Some("FULL-RECORDING-ISRC".into()),
            ..Track::default()
        };
        let mut chapter = Chapter {
            id: "chapter-1".into(),
            title: "Raw Artist - Raw Song".into(),
            start_seconds: 0.0,
            end_seconds: 180.0,
            confidence: "source_metadata_unverified".into(),
            suggested_track: Some(Box::new(track)),
            suggestion_confidence: Some("exact_unique_catalog_match".into()),
        };
        assert_eq!(
            verified_chapter_suggestion(&chapter, None).unwrap().title,
            "Verified Song Title"
        );
        chapter.suggestion_confidence = Some("ambiguous_catalog_matches".into());
        assert!(verified_chapter_suggestion(&chapter, None).is_none());
        chapter.suggestion_confidence = Some("exact_unique_catalog_match".into());
        chapter.suggested_track.as_mut().unwrap().source_url =
            "https://attacker.invalid/recording/recording-id".into();
        assert!(verified_chapter_suggestion(&chapter, None).is_none());
        chapter.suggested_track.as_mut().unwrap().source_url =
            "https://musicbrainz.org/recording/recording-id".into();
        assert!(verified_chapter_suggestion(
            &chapter,
            Some("Live at Pitchfork Music Festival 2018")
        )
        .is_none());
        chapter.suggested_track.as_mut().unwrap().version =
            Some("Live at Pitchfork Music Festival 2018".into());
        assert!(verified_chapter_suggestion(
            &chapter,
            Some("Live at Pitchfork Music Festival 2018")
        )
        .is_some());
    }
    #[test]
    fn chapter_track_identity_is_accepted_by_asset_registration() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let (id, provider_id) = chapter_track_identity("youtube:parent", "chapter-1");
        let track = Track {
            id: id.clone(),
            provider: "chapter".into(),
            provider_id,
            title: "A song excerpt".into(),
            ..Track::default()
        };
        let asset = LocalAsset {
            id: "asset-1".into(),
            recording_id: id,
            path: directory
                .path()
                .join("chapter.flac")
                .to_string_lossy()
                .into(),
            hash: "chapter-hash".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "excerpt".into(),
            track,
        };
        store.save_asset(&asset).unwrap();
        assert_eq!(store.list_assets().unwrap()[0].track.id, asset.track.id);
    }
    #[test]
    fn provider_host_boundaries() {
        assert!(validate_source_url("https://www.youtube.com/watch?v=a", "youtube").is_ok());
        assert!(
            validate_source_url("https://youtube.com.attacker.test/watch?v=a", "youtube").is_err()
        );
        assert!(validate_source_url("https://localhost/a.mp3", "direct").is_err());
        assert!(validate_source_url("https://127.0.0.1/a.mp3", "direct").is_err());
        assert!(
            validate_source_url("https://user:password@soundcloud.com/a", "soundcloud").is_err()
        );
    }
    #[test]
    fn retry_only_terminal_errors() {
        for state in ["failed", "cancelled", "interrupted"] {
            assert!(can_retry(state));
        }
        for state in ["queued", "downloading", "publishing", "completed"] {
            assert!(!can_retry(state));
        }
    }
    #[test]
    fn default_ffmpeg_uses_path_lookup() {
        assert_eq!(ffmpeg_location("ffmpeg"), None);
        assert_eq!(
            ffmpeg_location("/opt/tools/ffmpeg"),
            Some("/opt/tools/ffmpeg".into())
        );
    }
    #[test]
    fn archive_reuses_preserved_source_instead_of_newer_mp3_derivative() {
        let candidate = Candidate {
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=a".into(),
            ..Default::default()
        };
        let source = LocalAsset {
            id: "source".into(),
            recording_id: "recording".into(),
            path: "/music/source.opus".into(),
            hash: "source-hash".into(),
            source_codec: "opus".into(),
            final_codec: "opus".into(),
            source_bitrate: Some(128.0),
            final_bitrate: Some(128.0),
            processing: "original audio codec preserved".into(),
            track: Track {
                provenance: serde_json::json!({"acquisition": {"provider": candidate.provider, "url": candidate.url}}),
                ..Default::default()
            },
        };
        let mut derivative = source.clone();
        derivative.id = "derivative".into();
        derivative.final_codec = "mp3".into();
        derivative.processing = "MP3320 derivative".into();
        derivative.track.provenance["derivedFromAssetId"] = Value::String(source.id.clone());
        let newest_first = [derivative, source.clone()];
        let selected = newest_first
            .iter()
            .find(|a| is_preserved_source(a, &candidate))
            .unwrap();
        assert_eq!(selected.id, source.id);
        assert_eq!(selected.final_codec, "opus");
    }
    #[test]
    fn restrictions_are_not_inferred_as_permissions() {
        let public = candidate_from_json(
            &serde_json::json!({"id":"a","availability":"public"}),
            "youtube",
        );
        assert_eq!(public.permission, "unknown");
        let private = candidate_from_json(
            &serde_json::json!({"id":"b","availability":"private"}),
            "youtube",
        );
        assert_eq!(private.permission, "restricted");
    }
    #[test]
    fn raw_search_track_preserves_known_artist_and_does_not_promote_uploader() {
        let metadata = serde_json::json!({"id":"abc","artist":"Artist Name","track":"Track Name","title":"Artist Name - Track Name","uploader":"Label Channel","album":"EP","genre":"House","release_date":"20250101"});
        let mut candidate = candidate_from_json(&metadata, "youtube");
        candidate.url = "https://www.youtube.com/watch?v=abc".into();
        candidate.score = 0.8;
        candidate.confidence = "review".into();
        let track = raw_search_track(candidate.clone(), &metadata);
        assert_eq!(track.artist, "Artist Name");
        assert_eq!(track.title, "Track Name");
        assert_eq!(track.album.as_deref(), Some("EP"));
        assert_eq!(
            track.provenance["identity"],
            "unverified source search result"
        );
        assert_eq!(track.provenance["audioCandidate"]["url"], candidate.url);
        assert_eq!(track.provenance["uploader"], "Label Channel");

        let unknown = serde_json::json!({"id":"def","title":"Ambient Session","uploader":"Uploader Channel","upload_date":"20261012"});
        let candidate = candidate_from_json(&unknown, "soundcloud");
        let track = raw_search_track(candidate, &unknown);
        assert!(track.artist.is_empty());
        assert_eq!(track.title, "Ambient Session");
        assert_eq!(track.provenance["uploader"], "Uploader Channel");
        assert_eq!(track.provenance["artistSource"], "unknown");
        assert!(track.release_date.is_none());
        assert_eq!(track.provenance["uploadDate"], "20261012");
    }
    #[test]
    fn raw_youtube_performance_title_gets_live_version_without_promoting_uploader() {
        let info = serde_json::json!({
            "id":"video12345",
            "title":"Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            "uploader":"Pitchfork",
            "upload_date":"20261012"
        });
        let mut candidate = candidate_from_json(&info, "youtube");
        candidate.url = "https://www.youtube.com/watch?v=video12345".into();
        let track = raw_search_track(candidate, &info);
        assert_eq!(track.artist, "Big Thief");
        assert_eq!(track.title, "Mary");
        assert_eq!(
            track.version.as_deref(),
            Some("Live at Pitchfork Music Festival 2018")
        );
        assert_eq!(track.provenance["uploader"], "Pitchfork");
        assert_eq!(track.provenance["sourceTitle"], info["title"]);
        assert_eq!(track.release_date, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_audio_search_uses_query_directly_and_maps_unverified_tracks() {
        let entry = serde_json::json!({
            "id":"abc123", "url":"https://www.youtube.com/watch?v=abc123",
            "title":"Luna Vale - Night Drive (Extended Mix)", "artist":"Luna Vale",
            "track":"Night Drive (Extended Mix)", "uploader":"Luna Vale - Topic",
            "duration":367, "thumbnail":"https://i.ytimg.com/vi/abc123/hqdefault.jpg"
        });
        let script = format!("echo '{}'", serde_json::json!({"entries":[entry]}));
        let (_directory, settings) = fake_tool(&script);
        let found =
            search_audio_tracks_raw("night drive luna vale", &["youtube".into()], &settings)
                .await
                .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artist, "Luna Vale");
        assert_eq!(found[0].title, "Night Drive (Extended Mix)");
        assert_eq!(found[0].provider, "youtube");
        assert_eq!(found[0].provider_id, "abc123");
        assert_eq!(
            found[0].provenance["audioCandidate"]["confidence"],
            "review"
        );

        let direct = find_candidates_with_cancel(
            &found[0],
            &["youtube".into()],
            &settings,
            idle_cancel().1,
            3,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].url, found[0].source_url);
    }

    #[tokio::test]
    async fn concurrent_raw_audio_searches_share_results_and_cache_empty_successes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = RawAudioSearchCache::default();
        let calls = AtomicUsize::new(0);
        let search = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(Vec::<Track>::new())
        };
        let (first, second) = tokio::join!(
            cache.get_or_search("same-query".into(), search),
            cache.get_or_search("same-query".into(), search),
        );
        assert!(first.unwrap().is_empty());
        assert!(second.unwrap().is_empty());
        assert!(cache
            .get_or_search("same-query".into(), search)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn private_networks_rejected() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "100.64.0.1",
            "169.254.1.1",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
    }

    #[cfg(unix)]
    fn fake_tool(script: &str) -> (tempfile::TempDir, Settings) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-yt-dlp");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Default::default()
        };
        (directory, settings)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_cancellation_is_prompt() {
        let (_directory, settings) = fake_tool("sleep 60");
        let (tx, rx) = idle_cancel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(true);
        });
        let started = std::time::Instant::now();
        let result = run_tool(&settings, vec![], rx, Duration::from_secs(10), |_| {}).await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_timeout_terminates_instead_of_waiting_for_output() {
        let (_directory, settings) = fake_tool("sleep 60");
        let (_tx, rx) = idle_cancel();
        let result = run_tool(&settings, vec![], rx, Duration::from_millis(50), |_| {}).await;
        let error = result.unwrap_err();
        assert!(
            error.contains("timed out"),
            "Unexpected subprocess error: {error}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_progress_and_final_path_are_separate_messages() {
        let (_directory, settings) = fake_tool(
            "printf '%s\\n' 'BURRATA_PROGRESS {\"downloaded_bytes\":5,\"total_bytes\":10}' 'BURRATA_FILE \"/tmp/source.opus\"'",
        );
        let (_tx, rx) = idle_cancel();
        let mut messages = Vec::new();
        let result = run_tool(&settings, vec![], rx, Duration::from_secs(5), |line| {
            messages.push(line.to_string())
        })
        .await
        .unwrap();
        assert_eq!(messages.len(), 2);
        assert!(result.contains("BURRATA_FILE"));
        let progress: Value =
            serde_json::from_str(messages[0].strip_prefix("BURRATA_PROGRESS ").unwrap()).unwrap();
        assert_eq!(progress["downloaded_bytes"], 5);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_exact_version_search_retries_without_version_and_keeps_low_match_uncertain() {
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Radio Edit)","duration":360,"uploader":"Artist One"}]}"#;
        let script = format!(
            "case \"$*\" in *extended\\ mix*) echo '{{\"entries\":[]}}' ;; *) echo '{}' ;; esac",
            output
        );
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let found = find_candidates(&track, &["youtube".into()], &settings)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].confidence, "low");
        assert_eq!(found[0].permission, "unknown");
        assert!(found[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("conflicts with catalog")));
    }

    #[test]
    fn spotify_catalog_recording_has_a_plausible_review_candidate() {
        let track = Track {
            artist: "HUGEL, SOLTO (FR)".into(),
            title: "Jamaican (Bam Bam)".into(),
            duration_seconds: Some(156.0),
            ..Default::default()
        };
        let mut candidate = candidate_from_json(
            &serde_json::json!({
                "id": "hghqd1eBTYQ",
                "url": "https://www.youtube.com/watch?v=hghqd1eBTYQ",
                "title": "Hugel, SOLTO (FR) - Jamaican (Bam Bam) (Original Mix)",
                "uploader": "MoBlack Records",
                "duration": 157
            }),
            "youtube",
        );
        matching::score_candidate(&track, &mut candidate);
        assert!(validate_source_url(&candidate.url, "youtube").is_ok());
        assert!((candidate.score - 0.8).abs() < 0.001);
        assert_eq!(candidate.confidence, "review");
        assert_eq!(candidate.permission, "unknown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn one_provider_failure_does_not_discard_other_provider_candidates() {
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let script = format!(
            "case \"$*\" in *scsearch10:*) echo 'SoundCloud unavailable' >&2; exit 1 ;; *) echo '{}' ;; esac",
            output
        );
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let found = find_candidates(&track, &["youtube".into(), "soundcloud".into()], &settings)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("Some providers failed")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn independent_web_provider_queries_run_concurrently() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let youtube_marker = directory.path().join("youtube-started");
        let soundcloud_marker = directory.path().join("soundcloud-started");
        let executable = directory.path().join("fake-yt-dlp");
        let script = format!(
            r#"
case "$*" in
  *ytsearch*) own='{}'; other='{}'; result='{{"entries":[{{"id":"yt","url":"https://www.youtube.com/watch?v=yt","title":"Artist - Night Drive","duration":180}}]}}' ;;
  *scsearch*) own='{}'; other='{}'; result='{{"entries":[{{"id":"sc","url":"https://soundcloud.com/artist/night-drive","title":"Artist - Night Drive","duration":180}}]}}' ;;
  *) exit 2 ;;
esac
touch "$own"
i=0
while [ ! -e "$other" ] && [ "$i" -lt 300 ]; do
  sleep 0.01
  i=$((i + 1))
done
[ -e "$other" ] || {{ echo 'provider searches did not overlap' >&2; exit 9; }}
echo "$result"
"#,
            youtube_marker.display(),
            soundcloud_marker.display(),
            soundcloud_marker.display(),
            youtube_marker.display(),
        );
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Default::default()
        };
        let track = Track {
            artist: "Artist".into(),
            title: "Night Drive".into(),
            duration_seconds: Some(180.0),
            ..Default::default()
        };
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into(), "soundcloud".into()],
            &settings,
            idle_cancel().1,
            3,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preview_search_uses_three_result_query_and_caps_candidates() {
        let entries = (0..4)
            .map(|n| {
                serde_json::json!({
                    "id": format!("track{n}"),
                    "url": format!("https://www.youtube.com/watch?v=track{n}"),
                    "title": format!("Artist One - Night Drive (Mix {n})"),
                    "duration": 360,
                    "uploader": "Artist One"
                })
            })
            .collect::<Vec<_>>();
        let output = serde_json::json!({ "entries": entries }).to_string();
        let script = format!(
            "case \"$*\" in *ytsearch3:*) echo '{}' ;; *) echo '{{\"entries\":[]}}' ;; esac",
            output
        );
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let (_tx, cancel) = idle_cancel();
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &settings,
            cancel,
            3,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn preview_audio_metadata_requires_audio_only_and_trusted_public_hosts() {
        let candidate = Candidate {
            id: "youtube:hghqd1eBTYQ".into(),
            provider: "youtube".into(),
            ..Default::default()
        };
        let info = serde_json::json!({
            "id":"hghqd1eBTYQ",
            "url":"https://rr5---sn.googlevideo.com/videoplayback?signature=redacted",
            "ext":"m4a",
            "protocol":"https",
            "acodec":"mp4a.40.2",
            "vcodec":"none",
            "duration":156
        });
        let audio = preview_audio_from_metadata(&candidate, &info).unwrap();
        assert_eq!(audio.mime_type, "audio/mp4");
        assert_eq!(audio.duration_seconds, Some(156.0));
        let soundcloud = Candidate {
            id: "soundcloud:123".into(),
            provider: "soundcloud".into(),
            ..Default::default()
        };
        assert!(preview_audio_from_metadata(
            &soundcloud,
            &serde_json::json!({"id":"123","url":"https://cf-media.sndcdn.com/audio.m3u8","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).is_ok());
        assert!(preview_audio_from_metadata(
            &soundcloud,
            &serde_json::json!({"id":"123","url":"https://attacker.example/audio.m3u8","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://rr5---sn.googlevideo.com/a","ext":"mp4","acodec":"mp4a.40.2","vcodec":"avc1"})
        )
        .is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://attacker.example/a","ext":"mp4","acodec":"mp4a.40.2","vcodec":"none"})
        )
        .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_candidate_search_uses_cached_fake_subprocess_result() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let (_directory, settings) = fake_tool(&format!("echo '{}'", output));
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let providers = vec!["youtube".to_string()];
        let calls = AtomicUsize::new(0);
        let cache = CandidateSearchCache::default();
        for _ in 0..2 {
            let found = cache
                .get_or_search("same-track-and-provider".into(), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    find_candidates(&track, &providers, &settings)
                })
                .await
                .unwrap();
            assert_eq!(found.len(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_preview_cache_misses_share_one_search() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let (_directory, settings) = fake_tool(&format!("sleep 0.05; echo '{}'", output));
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let providers = vec!["youtube".to_string()];
        let calls = AtomicUsize::new(0);
        let cache = CandidateSearchCache::default();
        let first = cache.get_or_search("same-preview-key".into(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            find_candidates(&track, &providers, &settings)
        });
        let second = cache.get_or_search("same-preview-key".into(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            find_candidates(&track, &providers, &settings)
        });
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().len(), 1);
        assert_eq!(second.unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_preview_audio_resolutions_share_one_lookup() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let cache = PreviewAudioCache::default();
        let resolve = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(PreviewAudio {
                url: "https://rr5---sn.googlevideo.com/audio".into(),
                mime_type: "audio/mp4".into(),
                codec: None,
                bitrate_kbps: None,
                duration_seconds: Some(156.0),
            })
        };
        let first = cache.get_or_resolve("same-candidate".into(), resolve);
        let second = cache.get_or_resolve("same-candidate".into(), resolve);
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().mime_type, "audio/mp4");
        assert_eq!(second.unwrap().duration_seconds, Some(156.0));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn preview_audio_cache_respects_signed_url_expiry() {
        let expiry = Utc::now().timestamp() + 90;
        let url = format!("https://cdn.example/audio?expire={expiry}");
        let ttl = preview_audio_cache_ttl(&url);
        assert!(ttl <= Duration::from_secs(90));
        assert!(ttl >= Duration::from_secs(58));
        assert_eq!(
            preview_audio_cache_ttl("https://cdn.example/audio"),
            PREVIEW_AUDIO_CACHE_TTL
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_search_result_is_cached_to_avoid_repeating_failed_lookups() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (_directory, settings) = fake_tool(r#"echo '{"entries":[]}'"#);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let providers = vec!["youtube".to_string()];
        let calls = AtomicUsize::new(0);
        let cache = CandidateSearchCache::default();
        for _ in 0..2 {
            let found = cache
                .get_or_search("empty-search".into(), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    find_candidates(&track, &providers, &settings)
                })
                .await
                .unwrap();
            assert!(found.is_empty());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn newer_preview_generation_cancels_old_search_and_rejects_late_result() {
        let (_directory, settings) = fake_tool("sleep 60");
        let state = Arc::new(PreviewSearchState::default());
        let old_cancel = state.begin(1_000).unwrap();
        let old_state = state.clone();
        let old_search = tokio::spawn(async move {
            run_tool(
                &settings,
                vec![],
                old_cancel,
                Duration::from_secs(30),
                |_| {},
            )
            .await
            .map(|_| ())
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _new_cancel = state.begin(2_000).unwrap();
        assert!(old_search.await.unwrap().unwrap_err().contains("Cancelled"));
        assert!(!old_state.finish(1_000).unwrap());
        assert!(state.finish(2_000).unwrap());
        assert!(state.begin(1_500).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preview_search_honors_provider_timeout() {
        let (_directory, settings) = fake_tool("sleep 60");
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let started = Instant::now();
        let (_tx, cancel) = idle_cancel();
        let result = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &settings,
            cancel,
            3,
            Duration::from_millis(50),
        )
        .await;
        let error = result.unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn sequential_provider_fallback_can_reuse_generation() {
        let state = PreviewSearchState::default();
        let _youtube = state.begin(2_000).unwrap();
        assert!(state.begin(2_000).is_err());
        assert!(state.finish(2_000).unwrap());
        let _soundcloud = state.begin(2_000).unwrap();
        assert!(state.finish(2_000).unwrap());
        assert!(state.begin(1_999).is_err());
    }
}
