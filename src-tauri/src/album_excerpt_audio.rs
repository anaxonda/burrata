//! Bounded audio evidence for extracting a track from a public full-album
//! source. The module stores the original encoded bytes in an owned temporary
//! directory and never exposes signed URLs in errors or diagnostics.

use sha2::{Digest, Sha256};
use std::{io::Write, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, sync::watch};

const RATE: usize = 8_000;
const RANGE_CHUNK: usize = 512 * 1024;
const MAX_SOURCE_BYTES: usize = 25 * 1024 * 1024;
const MAX_RANGE_REQUESTS: usize = (MAX_SOURCE_BYTES + RANGE_CHUNK - 1) / RANGE_CHUNK;
const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
const MAX_AUDIO_SECONDS: usize = 3_000;
const MAX_PCM_BYTES: usize = 96 * 1024 * 1024;
const OVERALL_BUDGET: Duration = Duration::from_secs(150);
const SOURCE_FETCH_BUDGET: Duration = Duration::from_secs(90);
const QUIET_TO_ACTIVE_RMS_RATIO_MAX: f64 = 0.10;
const MIN_SURROUNDING_RMS: f64 = 0.005;

#[derive(Clone, Debug)]
pub struct AlbumAudioReference {
    pub catalog_track_id: u64,
    pub preview_url: String,
    pub expected_start_seconds: f64,
    pub duration_seconds: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AlbumAudioMatch {
    pub catalog_track_id: u64,
    pub source_start_seconds: f64,
    pub score: f64,
    pub thirds: [f64; 3],
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct BoundaryCue {
    pub expected_seconds: f64,
    pub cue_seconds: Option<f64>,
    pub displacement_seconds: Option<f64>,
    pub quiet_window_rms: Option<f64>,
    pub surrounding_rms: Option<f64>,
    pub confident: bool,
}

/// Owns both the original public source bytes and the decoded proof's lifetime.
/// Dropping this value removes the private temporary directory.
pub struct OwnedAlbumAudioSource {
    _directory: tempfile::TempDir,
    pub source_path: PathBuf,
    pub sha256: String,
    pub duration_seconds: f64,
    pub sample_rate: u32,
}

pub struct AlbumExcerptAudioProof {
    pub source: OwnedAlbumAudioSource,
    pub matches: Vec<AlbumAudioMatch>,
    pub target_track_id: u64,
    pub target_score: f64,
    pub target_thirds: [f64; 3],
    pub target_start_seconds: f64,
    pub target_end_seconds: f64,
    /// Cue locations are diagnostic. This is the worst cue displacement when
    /// both ends have cues, not a claim that the acoustic cue is a sample-exact
    /// track boundary.
    pub boundary_uncertainty_seconds: f64,
    pub start_boundary: BoundaryCue,
    pub end_boundary: BoundaryCue,
}

#[derive(Debug)]
pub enum AlbumExcerptAudioError {
    Cancelled,
    Deadline,
    InvalidSource,
    InvalidReferences,
    Network,
    RangeResponse,
    SizeLimit,
    Decode,
    InsufficientMatch,
    Worker,
    Io,
}

impl std::fmt::Display for AlbumExcerptAudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Cancelled => "Album audio evidence was cancelled",
            Self::Deadline => "Album audio evidence exceeded its time limit",
            Self::InvalidSource => "The public album audio source is unsupported",
            Self::InvalidReferences => "The album audio references are incomplete",
            Self::Network => "Public album audio could not be fetched",
            Self::RangeResponse => "The public audio server returned an invalid byte range",
            Self::SizeLimit => "Public album audio exceeds the configured size limit",
            Self::Decode => "Public album audio could not be decoded",
            Self::InsufficientMatch => "Public album audio did not produce usable evidence",
            Self::Worker => "Album audio evidence worker stopped unexpectedly",
            Self::Io => "Temporary album audio storage is unavailable",
        };
        f.write_str(message)
    }
}

impl std::error::Error for AlbumExcerptAudioError {}

type Result<T> = std::result::Result<T, AlbumExcerptAudioError>;

fn source_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|_| AlbumExcerptAudioError::InvalidSource)?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !(host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
    {
        return Err(AlbumExcerptAudioError::InvalidSource);
    }
    Ok(url)
}

fn preview_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|_| AlbumExcerptAudioError::InvalidReferences)?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !(host.ends_with(".itunes.apple.com") || host.ends_with(".mzstatic.com"))
    {
        return Err(AlbumExcerptAudioError::InvalidReferences);
    }
    Ok(url)
}

fn client(url: &reqwest::Url, addresses: &[std::net::SocketAddr]) -> Result<reqwest::Client> {
    let host = url
        .host_str()
        .ok_or(AlbumExcerptAudioError::InvalidSource)?;
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .resolve_to_addrs(host, addresses)
        .user_agent("Burrata/0.1 (public album audio evidence)")
        .build()
        .map_err(|_| AlbumExcerptAudioError::Network)
}

async fn bounded<T>(
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    if *cancel.borrow() {
        return Err(AlbumExcerptAudioError::Cancelled);
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(AlbumExcerptAudioError::Deadline);
    }
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => Err(AlbumExcerptAudioError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(AlbumExcerptAudioError::Deadline),
        result = future => result,
    }
}

async fn checked_response_bytes(
    response: reqwest::Response,
    max: usize,
    expected_url: &reqwest::Url,
) -> Result<Vec<u8>> {
    if response.status().is_redirection()
        || !response.status().is_success()
        || response.url().origin() != expected_url.origin()
        || response.content_length().is_some_and(|n| n > max as u64)
    {
        return Err(AlbumExcerptAudioError::Network);
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AlbumExcerptAudioError::Network)?
    {
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(AlbumExcerptAudioError::SizeLimit);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_content_range(raw: &str) -> Option<(usize, usize, usize)> {
    let rest = raw.strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    let total = total.parse().ok()?;
    (start <= end && end < total).then_some((start, end, total))
}

fn validate_range_response(
    status: reqwest::StatusCode,
    raw_content_range: Option<&str>,
    requested_start: usize,
    requested_end: usize,
    expected_total: Option<usize>,
    body_len: usize,
) -> Result<usize> {
    if status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(AlbumExcerptAudioError::RangeResponse);
    }
    let (start, end, total) = raw_content_range
        .and_then(parse_content_range)
        .ok_or(AlbumExcerptAudioError::RangeResponse)?;
    if start != requested_start
        || end != requested_end.min(total.saturating_sub(1))
        || expected_total.is_some_and(|expected| expected != total)
        || body_len != end - start + 1
        || total > MAX_SOURCE_BYTES
    {
        return Err(if total > MAX_SOURCE_BYTES {
            AlbumExcerptAudioError::SizeLimit
        } else {
            AlbumExcerptAudioError::RangeResponse
        });
    }
    Ok(total)
}

fn validate_chunk_metadata(
    status: reqwest::StatusCode,
    response_url: &reqwest::Url,
    expected_url: &reqwest::Url,
    content_encoding: Option<&str>,
    content_range: Option<&str>,
    requested_start: usize,
    requested_end: usize,
    expected_total: Option<usize>,
    body_len: usize,
) -> Result<usize> {
    if response_url.origin() != expected_url.origin()
        || response_url.host_str() != expected_url.host_str()
        || content_encoding.is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(AlbumExcerptAudioError::RangeResponse);
    }
    validate_range_response(
        status,
        content_range,
        requested_start,
        requested_end,
        expected_total,
        body_len,
    )
}

async fn download_preview(
    raw: &str,
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>> {
    let url = preview_url(raw)?;
    bounded(cancel, deadline, async move {
        let addresses = crate::acquisition::public_addresses(&url)
            .await
            .map_err(|_| AlbumExcerptAudioError::Network)?;
        let client = client(&url, &addresses)?;
        checked_response_bytes(
            client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| AlbumExcerptAudioError::Network)?,
            MAX_PREVIEW_BYTES,
            &url,
        )
        .await
    })
    .await
}

async fn download_source(
    raw: &str,
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>> {
    let url = source_url(raw)?;
    let source_deadline = deadline.min(tokio::time::Instant::now() + SOURCE_FETCH_BUDGET);
    bounded(cancel, source_deadline, async move {
        let addresses = crate::acquisition::public_addresses(&url)
            .await
            .map_err(|_| AlbumExcerptAudioError::Network)?;
        let client = client(&url, &addresses)?;
        let mut all = Vec::new();
        let mut total = None;
        let mut start = 0usize;
        let mut requests = 0usize;
        loop {
            if requests >= MAX_RANGE_REQUESTS {
                return Err(AlbumExcerptAudioError::SizeLimit);
            }
            if all.len() >= MAX_SOURCE_BYTES {
                return Err(AlbumExcerptAudioError::SizeLimit);
            }
            requests += 1;
            let end = start
                .saturating_add(RANGE_CHUNK - 1)
                .min(MAX_SOURCE_BYTES - 1);
            let response = client
                .get(url.clone())
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .send()
                .await
                .map_err(|_| AlbumExcerptAudioError::Network)?;
            let status = response.status();
            let response_url = response.url().clone();
            let content_encoding = response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let chunk = checked_response_bytes(response, RANGE_CHUNK, &url).await?;
            let actual_end = start.saturating_add(chunk.len().saturating_sub(1));
            let content_total = validate_chunk_metadata(
                status,
                &response_url,
                &url,
                content_encoding.as_deref(),
                range.as_deref(),
                start,
                end,
                total,
                chunk.len(),
            )?;
            if chunk.is_empty() || chunk.len() > RANGE_CHUNK {
                return Err(AlbumExcerptAudioError::RangeResponse);
            }
            total = Some(content_total);
            all.extend_from_slice(&chunk);
            if actual_end + 1 == content_total {
                break;
            }
            start = actual_end + 1;
        }
        if total != Some(all.len()) || all.is_empty() {
            return Err(AlbumExcerptAudioError::RangeResponse);
        }
        Ok(all)
    })
    .await
}

struct DecodeChild {
    child: Option<tokio::process::Child>,
    input: Option<tempfile::NamedTempFile>,
}

impl Drop for DecodeChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let input = self.input.take();
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                    drop(input);
                });
            } else {
                std::thread::spawn(move || {
                    loop {
                        match child.try_wait() {
                            Ok(Some(_)) | Err(_) => break,
                            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                        }
                    }
                    drop(input);
                });
            }
        }
    }
}

async fn decode_bytes(
    bytes: &[u8],
    ffmpeg_path: &str,
    max_seconds: usize,
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<Vec<f32>> {
    if bytes.is_empty()
        || bytes.len() > MAX_SOURCE_BYTES
        || max_seconds == 0
        || max_seconds > MAX_AUDIO_SECONDS
    {
        return Err(AlbumExcerptAudioError::SizeLimit);
    }
    let max_pcm_bytes = RATE * max_seconds * 4;
    if max_pcm_bytes > MAX_PCM_BYTES {
        return Err(AlbumExcerptAudioError::SizeLimit);
    }
    let path = tempfile::NamedTempFile::new().map_err(|_| AlbumExcerptAudioError::Io)?;
    std::fs::write(path.path(), bytes).map_err(|_| AlbumExcerptAudioError::Io)?;
    bounded(cancel, deadline, async move {
        let mut command = tokio::process::Command::new(crate::tools::executable(ffmpeg_path, "ffmpeg"));
        command
            .args(["-nostdin", "-v", "error", "-protocol_whitelist", "file,crypto,data", "-threads", "1", "-i"])
            .arg(path.path())
            .args(["-t", &(max_seconds + 1).to_string(), "-vn", "-ac", "1", "-ar", "8000", "-f", "f32le", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| AlbumExcerptAudioError::Decode)?;
        let mut process = DecodeChild {
            child: Some(child),
            input: Some(path),
        };
        let stdout = process
            .child
            .as_mut()
            .and_then(|child| child.stdout.take())
            .ok_or(AlbumExcerptAudioError::Decode)?;
        let mut output = Vec::new();
        let mut reader = stdout.take((max_pcm_bytes + 4) as u64);
        let read_result = reader.read_to_end(&mut output);
        tokio::select! {
            biased;
            result = read_result => {
                result.map_err(|_| AlbumExcerptAudioError::Decode)?;
                if output.len() > max_pcm_bytes {
                    if let Some(child) = process.child.as_mut() { let _ = child.kill().await; let _ = child.wait().await; }
                    process.child.take();
                    return Err(AlbumExcerptAudioError::SizeLimit);
                }
                let status = process.child.as_mut().ok_or(AlbumExcerptAudioError::Decode)?
                    .wait().await.map_err(|_| AlbumExcerptAudioError::Decode)?;
                process.child.take();
                if !status.success() || output.len() % 4 != 0 {
                    return Err(AlbumExcerptAudioError::Decode);
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                if let Some(child) = process.child.as_mut() { let _ = child.kill().await; let _ = child.wait().await; }
                process.child.take();
                return Err(AlbumExcerptAudioError::Deadline);
            }
        }
        if output.len() > max_pcm_bytes || output.len() % 4 != 0 {
            return Err(AlbumExcerptAudioError::SizeLimit);
        }
        let samples = output.chunks_exact(4).map(|v| f32::from_le_bytes(v.try_into().unwrap())).collect::<Vec<_>>();
        if samples.is_empty() || samples.len() > RATE * max_seconds || samples.iter().any(|v| !v.is_finite()) {
            return Err(AlbumExcerptAudioError::SizeLimit);
        }
        Ok(samples)
    }).await
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut aa, mut bb, mut ab) = (0.0f64, 0.0f64, 0.0f64);
    for (&a, &b) in a.iter().zip(b) {
        let (a, b) = (a as f64, b as f64);
        aa += a * a;
        bb += b * b;
        ab += a * b;
    }
    if aa < 1e-8 || bb < 1e-8 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

fn downsample(audio: &[f32]) -> Vec<f32> {
    audio
        .chunks_exact(64)
        .map(|chunk| chunk.iter().sum::<f32>() / 64.0)
        .collect()
}

fn match_reference(
    reference: &[f32],
    source: &[f32],
    track_start: usize,
    track_end: usize,
    cancel: &watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<(usize, f64, [f64; 3])> {
    if reference.len() < RATE * 20
        || reference.len() > RATE * 120
        || track_end <= track_start
        || track_end > source.len()
    {
        return Err(AlbumExcerptAudioError::InvalidReferences);
    }
    let search_start = track_start.saturating_sub(RATE * 2);
    let search_end = track_end.saturating_add(RATE * 2).min(source.len());
    if search_end.saturating_sub(search_start) < reference.len() {
        return Err(AlbumExcerptAudioError::InsufficientMatch);
    }
    let down_ref = downsample(reference);
    let down_source = downsample(&source[search_start..search_end]);
    let seed_len = (RATE * 4 / 64).min(down_ref.len() / 3).max(125);
    let seed = (0..=down_ref.len().saturating_sub(seed_len))
        .step_by(64)
        .max_by(|&a, &b| {
            down_ref[a..a + seed_len]
                .iter()
                .map(|v| v * v)
                .sum::<f32>()
                .total_cmp(&down_ref[b..b + seed_len].iter().map(|v| v * v).sum::<f32>())
        })
        .ok_or(AlbumExcerptAudioError::InsufficientMatch)?;
    let mut peaks: Vec<(f64, usize)> = Vec::new();
    for (scan_index, offset) in (0..=down_source.len() - down_ref.len()).enumerate() {
        if scan_index % 256 == 0 {
            check_work(cancel, deadline)?;
        }
        let score = correlation(
            &down_ref[seed..seed + seed_len],
            &down_source[offset + seed..offset + seed + seed_len],
        )
        .abs();
        if peaks.last().is_none_or(|peak| score > peak.0) {
            peaks.push((score, offset));
            peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut kept = Vec::new();
            for peak in peaks.drain(..) {
                if kept
                    .iter()
                    .all(|p: &(f64, usize)| p.1.abs_diff(peak.1) > 32)
                {
                    kept.push(peak);
                }
                if kept.len() >= 6 {
                    break;
                }
            }
            peaks = kept;
        }
    }
    let mut refined = Vec::new();
    let seed_start = seed * 64;
    let seed_end = (seed_start + RATE * 4).min(reference.len());
    for (_, rough) in peaks {
        check_work(cancel, deadline)?;
        let center = rough * 64;
        let lo = center.saturating_sub(RATE / 32).saturating_add(seed_start);
        let hi = (center + RATE / 32 + seed_start)
            .min(search_end - search_start - (seed_end - seed_start));
        let a = &reference[seed_start..seed_end];
        let mut local = (0.0f64, center + seed_start);
        for (refine_index, offset) in (lo..=hi).enumerate() {
            if refine_index % 128 == 0 {
                check_work(cancel, deadline)?;
            }
            let score = correlation(
                a,
                &source[search_start + offset..search_start + offset + a.len()],
            )
            .abs();
            if score > local.0 {
                local = (score, offset);
            }
        }
        refined.push(local.1.saturating_sub(seed_start));
    }
    let mut best = (0.0f64, search_start);
    for offset in refined {
        check_work(cancel, deadline)?;
        let source_offset = search_start + offset;
        if source_offset + reference.len() > search_end {
            continue;
        }
        let score = correlation(
            reference,
            &source[source_offset..source_offset + reference.len()],
        )
        .abs();
        if score > best.0 {
            best = (score, source_offset);
        }
    }
    if best.0 == 0.0 {
        return Err(AlbumExcerptAudioError::InsufficientMatch);
    }
    let aligned = &source[best.1..best.1 + reference.len()];
    let thirds = std::array::from_fn(|i| {
        let start = reference.len() * i / 3;
        let end = reference.len() * (i + 1) / 3;
        correlation(&reference[start..end], &aligned[start..end]).abs()
    });
    Ok((best.1, best.0, thirds))
}

fn rms_window(source: &[f32], center: usize, width: usize) -> f64 {
    let start = center.saturating_sub(width / 2);
    let end = center.saturating_add(width / 2).min(source.len());
    if end <= start {
        return 0.0;
    }
    (source[start..end]
        .iter()
        .map(|v| (*v as f64) * (*v as f64))
        .sum::<f64>()
        / (end - start) as f64)
        .sqrt()
}

fn check_work(cancel: &watch::Receiver<bool>, deadline: tokio::time::Instant) -> Result<()> {
    if *cancel.borrow() {
        Err(AlbumExcerptAudioError::Cancelled)
    } else if tokio::time::Instant::now() >= deadline {
        Err(AlbumExcerptAudioError::Deadline)
    } else {
        Ok(())
    }
}

fn boundary_cue(
    source: &[f32],
    expected_seconds: f64,
    cancel: &watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<BoundaryCue> {
    let center = (expected_seconds * RATE as f64).round().max(0.0) as usize;
    let radius = 2 * RATE;
    let lo = center.saturating_sub(radius);
    let hi = center.saturating_add(radius).min(source.len());
    let width = RATE / 10;
    if hi <= lo + width {
        return Ok(BoundaryCue {
            expected_seconds,
            cue_seconds: None,
            displacement_seconds: None,
            quiet_window_rms: None,
            surrounding_rms: None,
            confident: false,
        });
    }
    let mut best = (f64::INFINITY, lo);
    let mut pos = lo;
    let mut window_index = 0usize;
    while pos + width <= hi {
        if window_index % 16 == 0 {
            check_work(cancel, deadline)?;
        }
        let score = rms_window(source, pos + width / 2, width);
        if score < best.0 {
            best = (score, pos + width / 2);
        }
        pos += RATE / 50;
        window_index += 1;
    }
    let before = rms_window(source, best.1.saturating_sub(RATE), RATE);
    let after = rms_window(source, best.1.saturating_add(RATE), RATE);
    let surrounding = before.max(after);
    let confident = best.0.is_finite()
        && surrounding >= MIN_SURROUNDING_RMS
        && best.0 <= surrounding * QUIET_TO_ACTIVE_RMS_RATIO_MAX;
    let cue = best.0.is_finite().then_some(best.1 as f64 / RATE as f64);
    Ok(BoundaryCue {
        expected_seconds,
        cue_seconds: cue,
        displacement_seconds: cue.map(|v| v - expected_seconds),
        quiet_window_rms: best.0.is_finite().then_some(best.0),
        surrounding_rms: (surrounding > 0.0).then_some(surrounding),
        confident,
    })
}

pub async fn verify_album_excerpt(
    raw_source_url: &str,
    references: &[AlbumAudioReference],
    target_index: usize,
    expected_album_duration_seconds: f64,
    ffmpeg_path: &str,
    cancel: watch::Receiver<bool>,
) -> Result<AlbumExcerptAudioProof> {
    let overall_deadline = tokio::time::Instant::now() + OVERALL_BUDGET;
    if !(3..=4).contains(&references.len())
        || target_index >= references.len()
        || !expected_album_duration_seconds.is_finite()
        || !(60.0..=MAX_AUDIO_SECONDS as f64).contains(&expected_album_duration_seconds)
    {
        return Err(AlbumExcerptAudioError::InvalidReferences);
    }
    let mut last_end = 0.0;
    let mut ids = std::collections::HashSet::new();
    for reference in references {
        if reference.catalog_track_id == 0
            || !ids.insert(reference.catalog_track_id)
            || reference.preview_url.is_empty()
            || !reference.expected_start_seconds.is_finite()
            || !reference.duration_seconds.is_finite()
            || !(1.0..=600.0).contains(&reference.duration_seconds)
            || reference.expected_start_seconds < last_end
            || reference.expected_start_seconds + reference.duration_seconds
                > expected_album_duration_seconds
        {
            return Err(AlbumExcerptAudioError::InvalidReferences);
        }
        last_end = reference.expected_start_seconds + reference.duration_seconds;
    }

    let mut reference_pcm = Vec::with_capacity(references.len());
    for reference in references {
        let encoded =
            download_preview(&reference.preview_url, cancel.clone(), overall_deadline).await?;
        reference_pcm.push(
            decode_bytes(&encoded, ffmpeg_path, 120, cancel.clone(), overall_deadline).await?,
        );
    }

    let encoded = download_source(raw_source_url, cancel.clone(), overall_deadline).await?;
    let source_hash = format!("{:x}", Sha256::digest(&encoded));
    let directory = tempfile::Builder::new()
        .prefix("burrata-album-audio-")
        .tempdir()
        .map_err(|_| AlbumExcerptAudioError::Io)?;
    let source_path = directory.path().join("public-source-audio.bin");
    let mut file = std::fs::File::create(&source_path).map_err(|_| AlbumExcerptAudioError::Io)?;
    file.write_all(&encoded)
        .map_err(|_| AlbumExcerptAudioError::Io)?;
    file.flush().map_err(|_| AlbumExcerptAudioError::Io)?;
    drop(file);
    let source_pcm = decode_bytes(
        &encoded,
        ffmpeg_path,
        MAX_AUDIO_SECONDS,
        cancel.clone(),
        overall_deadline,
    )
    .await?;
    let source_duration = source_pcm.len() as f64 / RATE as f64;
    if source_duration <= 0.0
        || source_duration > MAX_AUDIO_SECONDS as f64
        || !source_duration.is_finite()
    {
        return Err(AlbumExcerptAudioError::SizeLimit);
    }
    let scale = source_duration / expected_album_duration_seconds;
    let target = &references[target_index];
    let target_start = target.expected_start_seconds * scale;
    let target_end = (target.expected_start_seconds + target.duration_seconds) * scale;
    let match_inputs = references
        .iter()
        .zip(reference_pcm)
        .map(|(reference, pcm)| {
            (
                reference.catalog_track_id,
                reference.expected_start_seconds,
                reference.duration_seconds,
                pcm,
            )
        })
        .collect::<Vec<_>>();
    let target_id = target.catalog_track_id;
    let work_cancel = cancel;
    let work_deadline = overall_deadline;
    let (matches, start_boundary, end_boundary) = tokio::task::spawn_blocking(move || {
        let mut matches = Vec::with_capacity(match_inputs.len());
        for (catalog_track_id, expected_start, duration, pcm) in match_inputs {
            check_work(&work_cancel, work_deadline)?;
            let start = (expected_start * scale * RATE as f64).round() as usize;
            let end = ((expected_start + duration) * scale * RATE as f64).round() as usize;
            let (offset, score, thirds) =
                match_reference(&pcm, &source_pcm, start, end, &work_cancel, work_deadline)?;
            matches.push(AlbumAudioMatch {
                catalog_track_id,
                source_start_seconds: offset as f64 / RATE as f64,
                score,
                thirds,
            });
        }
        let start_boundary = boundary_cue(&source_pcm, target_start, &work_cancel, work_deadline)?;
        let end_boundary = boundary_cue(&source_pcm, target_end, &work_cancel, work_deadline)?;
        Ok::<_, AlbumExcerptAudioError>((matches, start_boundary, end_boundary))
    })
    .await
    .map_err(|_| AlbumExcerptAudioError::Worker)??;
    // The quiet-cue displacement is a diagnostic for where a low-energy
    // join may lie; it does not measure the actual track boundary. Keep the
    // accepted timeline uncertainty conservative at the full search window.
    let cues_within_window = start_boundary.confident
        && end_boundary.confident
        && start_boundary
            .displacement_seconds
            .is_some_and(|value| value.abs() <= 2.0)
        && end_boundary
            .displacement_seconds
            .is_some_and(|value| value.abs() <= 2.0);
    if !cues_within_window {
        return Err(AlbumExcerptAudioError::InsufficientMatch);
    }
    let boundary_uncertainty = 2.0;
    let target_match = matches
        .iter()
        .find(|audio_match| audio_match.catalog_track_id == target_id)
        .ok_or(AlbumExcerptAudioError::InsufficientMatch)?;
    let target_score = target_match.score;
    let target_thirds = target_match.thirds;

    Ok(AlbumExcerptAudioProof {
        source: OwnedAlbumAudioSource {
            _directory: directory,
            source_path,
            sha256: source_hash,
            duration_seconds: source_duration,
            sample_rate: RATE as u32,
        },
        matches,
        target_track_id: target_id,
        target_score,
        target_thirds,
        target_start_seconds: target_start,
        target_end_seconds: target_end,
        boundary_uncertainty_seconds: boundary_uncertainty,
        start_boundary,
        end_boundary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_exact_bounded_range_response() {
        let status = reqwest::StatusCode::PARTIAL_CONTENT;
        assert_eq!(
            validate_range_response(status, Some("bytes 0-3/8"), 0, 3, None, 4).unwrap(),
            8
        );
        assert!(validate_range_response(status, Some("bytes 1-4/8"), 0, 3, None, 4).is_err());
        assert!(validate_range_response(status, Some("bytes 0-3/9"), 0, 3, Some(8), 4).is_err());
        assert!(validate_range_response(
            reqwest::StatusCode::OK,
            Some("bytes 0-3/8"),
            0,
            3,
            None,
            4
        )
        .is_err());
        assert!(validate_range_response(status, Some("bytes 0-2/8"), 0, 3, None, 3).is_err());
        assert!(validate_range_response(status, Some("bytes 0-3/8"), 0, 3, None, 3).is_err());
    }

    #[test]
    fn rejects_non_google_source_and_non_catalog_preview_hosts() {
        assert!(source_url("https://example.org/audio").is_err());
        assert!(source_url("https://user:pass@r.googlevideo.com/audio").is_err());
        assert!(source_url("https://r.googlevideo.com:443/audio").is_ok());
        assert!(source_url("https://r.googlevideo.com/audio?sig=secret").is_ok());
        assert!(preview_url("https://example.org/preview").is_err());
        assert!(preview_url("https://audio.mzstatic.com/preview").is_ok());
    }

    #[test]
    fn catches_cross_chunk_total_mismatch_and_aggregate_limit() {
        assert!(validate_range_response(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 524288-524291/9"),
            524288,
            524291,
            Some(8),
            4
        )
        .is_err());
        assert!(matches!(
            validate_range_response(
                reqwest::StatusCode::PARTIAL_CONTENT,
                Some("bytes 0-3/26214401"),
                0,
                3,
                None,
                4
            ),
            Err(AlbumExcerptAudioError::SizeLimit)
        ));
        assert_eq!(MAX_RANGE_REQUESTS, 50);
    }

    #[test]
    fn mocked_range_metadata_rejects_host_identity_encoding_and_short_chunks() {
        let expected = reqwest::Url::parse("https://r.googlevideo.com/audio?sig=secret").unwrap();
        let same_host = reqwest::Url::parse("https://r.googlevideo.com/audio?sig=secret").unwrap();
        let other_host =
            reqwest::Url::parse("https://other.googlevideo.com/audio?sig=secret").unwrap();
        let status = reqwest::StatusCode::PARTIAL_CONTENT;
        assert_eq!(
            validate_chunk_metadata(
                status,
                &same_host,
                &expected,
                Some("identity"),
                Some("bytes 0-3/8"),
                0,
                3,
                None,
                4
            )
            .unwrap(),
            8
        );
        assert!(validate_chunk_metadata(
            status,
            &other_host,
            &expected,
            Some("identity"),
            Some("bytes 0-3/8"),
            0,
            3,
            None,
            4
        )
        .is_err());
        assert!(validate_chunk_metadata(
            status,
            &same_host,
            &expected,
            Some("gzip"),
            Some("bytes 0-3/8"),
            0,
            3,
            None,
            4
        )
        .is_err());
        assert!(validate_chunk_metadata(
            status,
            &same_host,
            &expected,
            None,
            Some("bytes 0-2/8"),
            0,
            3,
            None,
            3
        )
        .is_err());
    }

    #[test]
    fn sample_match_preserves_exact_recording_and_wrong_variant_control() {
        let mut seed = 0x5f3759dfu32;
        let base = (0..RATE * 26)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (((seed >> 8) as f32 / 16_777_215.0) * 2.0 - 1.0) * 0.2
            })
            .collect::<Vec<_>>();
        let mut source = vec![0.0; RATE * 170];
        source[RATE * 123..RATE * 123 + base.len()].copy_from_slice(&base);
        let (_stop, cancel) = watch::channel(false);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let (offset, score, thirds) =
            match_reference(&base, &source, RATE * 100, RATE * 150, &cancel, deadline).unwrap();
        assert_eq!(offset, RATE * 123);
        assert!(score >= 0.999);
        assert!(thirds.iter().all(|score| *score >= 0.999));
        let altered = base
            .iter()
            .enumerate()
            .map(|(i, v)| if (i / 1000) % 2 == 0 { -*v } else { *v })
            .collect::<Vec<_>>();
        let (_, control, _) =
            match_reference(&altered, &source, RATE * 100, RATE * 150, &cancel, deadline).unwrap();
        assert!(control < 0.96);
    }

    #[test]
    fn boundary_cue_requires_a_quiet_window_with_active_neighbor() {
        let mut source = vec![0.2f32; RATE * 200];
        source[RATE * 100 - RATE / 10..RATE * 100 + RATE / 10].fill(0.0);
        let (_stop, cancel) = watch::channel(false);
        let cue = boundary_cue(
            &source,
            100.0,
            &cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(cue.confident);
        assert!(cue
            .displacement_seconds
            .is_some_and(|value| value.abs() <= 2.0));
        let quiet = vec![0.0f32; RATE * 200];
        let no_cue = boundary_cue(
            &quiet,
            100.0,
            &cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(!no_cue.confident);
    }

    #[tokio::test]
    async fn observes_pre_cancelled_request_without_network_access() {
        let (tx, rx) = watch::channel(true);
        let result = download_source(
            "https://r.googlevideo.com/a?sig=secret",
            rx,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;
        assert!(matches!(result, Err(AlbumExcerptAudioError::Cancelled)));
        drop(tx);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_decoder_keeps_input_until_child_is_reaped() {
        let input = tempfile::NamedTempFile::new().unwrap();
        let input_path = input.path().to_path_buf();
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        drop(DecodeChild {
            child: Some(child),
            input: Some(input),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while input_path.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!input_path.exists());
    }

    #[test]
    fn matching_work_observes_cancellation_and_absolute_deadline() {
        let reference = vec![0.1f32; RATE * 25];
        let source = vec![0.1f32; RATE * 80];
        let (_stop, cancelled) = watch::channel(true);
        assert!(matches!(
            match_reference(
                &reference,
                &source,
                RATE * 10,
                RATE * 70,
                &cancelled,
                tokio::time::Instant::now() + Duration::from_secs(2),
            ),
            Err(AlbumExcerptAudioError::Cancelled)
        ));
        let (_stop, active) = watch::channel(false);
        assert!(matches!(
            match_reference(
                &reference,
                &source,
                RATE * 10,
                RATE * 70,
                &active,
                tokio::time::Instant::now() - Duration::from_millis(1),
            ),
            Err(AlbumExcerptAudioError::Deadline)
        ));
    }
}
