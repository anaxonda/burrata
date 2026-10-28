//! Explicit, bounded waveform analysis for one registered local audio asset.
//! The worker streams FFmpeg output into amplitude bins; it never creates an
//! intermediate audio file or retains decoded PCM beyond one small read chunk.

use crate::{models::LocalAsset, storage::Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command};

pub const ANALYZER_VERSION: &str = "ffmpeg-mono-peaks-8khz-v1";
const CACHE_KEY: &str = "waveformAnalysisCache.v1";
const MAX_CACHE_RECORDS: usize = 128;
const MAX_ACTIVE_JOBS: usize = 2;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_DURATION_SECONDS: f64 = 4.0 * 60.0 * 60.0;
const OUTPUT_SAMPLE_RATE: u32 = 8_000;
const MAX_BIN_COUNT: usize = 4_096;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const HASH_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformBin {
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalWaveform {
    pub asset_id: String,
    pub file_hash: String,
    pub analyzer_version: String,
    pub bin_count: usize,
    pub duration_seconds: f64,
    pub bins: Vec<Option<WaveformBin>>,
    pub scope: String,
    pub complete: bool,
    pub measured_at: String,
}

impl LocalWaveform {
    fn validate(&self) -> Result<(), String> {
        if self.asset_id.trim().is_empty()
            || self.file_hash.len() != 64
            || !self.file_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.analyzer_version != ANALYZER_VERSION
            || !(1..=MAX_BIN_COUNT).contains(&self.bin_count)
            || self.bins.len() != self.bin_count
            || !self.duration_seconds.is_finite()
            || self.duration_seconds <= 0.0
            || self.duration_seconds > MAX_DURATION_SECONDS
            || self.scope != "wholeTrack"
            || !self.complete
            || self.bins.iter().any(|bin| {
                (*bin).map_or(true, |value| {
                    !value.min.is_finite()
                        || !value.max.is_finite()
                        || value.min < -1.0
                        || value.max > 1.0
                        || value.min > value.max
                })
            })
        {
            return Err("Cached waveform record is invalid".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformJobStatus {
    pub job_id: String,
    pub status: String,
    pub asset_id: String,
    pub file_hash: String,
    pub analyzer_version: String,
    pub bin_count: usize,
    pub duration_seconds: Option<f64>,
    pub processed_seconds: f64,
    pub error: Option<String>,
}

struct Job {
    status: Arc<Mutex<WaveformJobStatus>>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct WaveformAnalysisJobs {
    jobs: Mutex<HashMap<String, Job>>,
    cache_lock: Mutex<()>,
}

impl WaveformAnalysisJobs {
    pub fn start(
        self: &Arc<Self>,
        store: Arc<Store>,
        asset_id: String,
        expected_hash: String,
        bin_count: usize,
        ffmpeg: PathBuf,
    ) -> Result<String, String> {
        if !(1..=MAX_BIN_COUNT).contains(&bin_count) {
            return Err(format!(
                "Waveform bin count must be from 1 to {MAX_BIN_COUNT}"
            ));
        }
        let asset = store
            .get_asset_by_id(&asset_id)?
            .ok_or("Selected local audio asset is no longer registered")?;
        if expected_hash != asset.hash {
            return Err(
                "Selected audio hash is stale; reload the registered asset before analysis".into(),
            );
        }
        let cached = get_cached_record(&store, &asset_id, &expected_hash, bin_count)?;
        if cached.is_some() {
            return Err("A complete waveform is already cached for this audio file".into());
        }
        let job_id = uuid::Uuid::new_v4().to_string();
        let status = Arc::new(Mutex::new(WaveformJobStatus {
            job_id: job_id.clone(),
            status: "queued".into(),
            asset_id: asset_id.clone(),
            file_hash: expected_hash.clone(),
            analyzer_version: ANALYZER_VERSION.into(),
            bin_count,
            duration_seconds: asset.duration_seconds,
            processed_seconds: 0.0,
            error: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let writer_lease = store.try_library_write()?;
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "Waveform analysis jobs are unavailable")?;
            ensure_active_job_capacity(&jobs)?;
            if jobs.len() >= 16 {
                let finished = jobs.iter().find_map(|(id, job)| {
                    job.status.lock().ok().and_then(|status| {
                        matches!(status.status.as_str(), "completed" | "cancelled" | "failed")
                            .then(|| id.clone())
                    })
                });
                if let Some(id) = finished {
                    jobs.remove(&id);
                }
                if jobs.len() >= 16 {
                    return Err("Too many waveform analysis jobs are already active".into());
                }
            }
            jobs.insert(
                job_id.clone(),
                Job {
                    status: status.clone(),
                    cancelled: cancelled.clone(),
                },
            );
        }
        let jobs = self.clone();
        let task_id = job_id.clone();
        tauri::async_runtime::spawn(async move {
            let _writer_lease = writer_lease;
            if let Ok(mut value) = status.lock() {
                value.status = "running".into();
            }
            let progress_jobs = jobs.clone();
            let progress_id = task_id.clone();
            let result = analyze_registered_asset(
                store.clone(),
                asset,
                expected_hash,
                bin_count,
                ffmpeg,
                cancelled.clone(),
                move |processed, duration| {
                    let _ = progress_jobs.update(&progress_id, |value| {
                        value.processed_seconds = processed;
                        value.duration_seconds = duration.or(value.duration_seconds);
                    });
                },
            )
            .await;
            match result {
                Ok(record) => {
                    let stored = jobs
                        .cache_lock
                        .lock()
                        .map_err(|_| "Waveform cache is unavailable".to_owned())
                        .and_then(|_guard| {
                            let current = store.get_json_preference(CACHE_KEY)?;
                            store.set_json_preference(
                                CACHE_KEY,
                                &upsert_cache(current, record.clone()),
                            )
                        });
                    let _ = jobs.update(&task_id, |value| {
                        value.status = if stored.is_ok() {
                            "completed"
                        } else {
                            "failed"
                        }
                        .into();
                        value.duration_seconds = Some(record.duration_seconds);
                        value.processed_seconds = record.duration_seconds;
                        if let Err(error) = stored {
                            value.error = Some(error.to_string());
                        }
                    });
                }
                Err(error) => {
                    let cancelled = cancelled.load(Ordering::Acquire);
                    let _ = jobs.update(&task_id, |value| {
                        value.status = if cancelled { "cancelled" } else { "failed" }.into();
                        value.error = (!cancelled).then_some(error);
                    });
                }
            }
        });
        Ok(job_id)
    }

    pub fn status(&self, job_id: &str) -> Result<WaveformJobStatus, String> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "Waveform analysis jobs are unavailable")?;
        let job = jobs
            .get(job_id)
            .ok_or("Waveform analysis job was not found")?;
        job.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| "Waveform analysis status is unavailable".into())
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "Waveform analysis jobs are unavailable")?;
        let job = jobs
            .get(job_id)
            .ok_or("Waveform analysis job was not found")?;
        job.cancelled.store(true, Ordering::Release);
        Ok(())
    }

    fn update(
        &self,
        job_id: &str,
        update: impl FnOnce(&mut WaveformJobStatus),
    ) -> Result<(), String> {
        let status = {
            let jobs = self
                .jobs
                .lock()
                .map_err(|_| "Waveform analysis jobs are unavailable")?;
            jobs.get(job_id)
                .ok_or("Waveform analysis job was not found")?
                .status
                .clone()
        };
        let mut guard = status
            .lock()
            .map_err(|_| "Waveform analysis status is unavailable")?;
        update(&mut guard);
        Ok(())
    }
}

fn ensure_active_job_capacity(jobs: &HashMap<String, Job>) -> Result<(), String> {
    let active = jobs
        .values()
        .filter(|job| {
            job.status.lock().map_or(true, |status| {
                matches!(status.status.as_str(), "queued" | "running")
            })
        })
        .count();
    if active >= MAX_ACTIVE_JOBS {
        return Err("Finish or cancel another waveform analysis before starting a new one".into());
    }
    Ok(())
}

pub async fn cached_waveform(
    store: Arc<Store>,
    asset_id: String,
    expected_hash: String,
    bin_count: usize,
) -> Result<Option<LocalWaveform>, String> {
    if !(1..=MAX_BIN_COUNT).contains(&bin_count) {
        return Err(format!(
            "Waveform bin count must be from 1 to {MAX_BIN_COUNT}"
        ));
    }
    let asset = store
        .get_asset_by_id(&asset_id)?
        .ok_or("Selected local audio asset is no longer registered")?;
    if asset.hash != expected_hash {
        return Err("Selected audio hash is stale; reload the registered asset".into());
    }
    let cached = get_cached_record(&store, &asset_id, &expected_hash, bin_count)?;
    if cached.is_none() {
        return Ok(None);
    }
    let path = verified_asset_path(&asset)?;
    let expected = expected_hash.clone();
    let actual = tokio::task::spawn_blocking(move || hash_file(&path))
        .await
        .map_err(|e| e.to_string())??;
    if actual != expected {
        return Err("Audio file changed since waveform analysis".into());
    }
    Ok(cached)
}

fn get_cached_record(
    store: &Store,
    asset_id: &str,
    file_hash: &str,
    bin_count: usize,
) -> Result<Option<LocalWaveform>, String> {
    let Some(value) = store.get_json_preference(CACHE_KEY)? else {
        return Ok(None);
    };
    let cache: WaveformCache = serde_json::from_value(value).unwrap_or_default();
    Ok(cache.records.into_iter().find(|record| {
        record.asset_id == asset_id
            && record.file_hash == file_hash
            && record.bin_count == bin_count
            && record.analyzer_version == ANALYZER_VERSION
            && record.validate().is_ok()
    }))
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaveformCache {
    records: Vec<LocalWaveform>,
}

fn upsert_cache(value: Option<serde_json::Value>, record: LocalWaveform) -> serde_json::Value {
    let mut cache: WaveformCache = value
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    cache
        .records
        .retain(|item| !(item.asset_id == record.asset_id && item.bin_count == record.bin_count));
    cache.records.push(record);
    if cache.records.len() > MAX_CACHE_RECORDS {
        let drop_count = cache.records.len() - MAX_CACHE_RECORDS;
        cache.records.drain(0..drop_count);
    }
    serde_json::to_value(cache).unwrap_or_else(|_| serde_json::json!({"records": []}))
}

async fn analyze_registered_asset(
    store: Arc<Store>,
    asset: LocalAsset,
    expected_hash: String,
    bin_count: usize,
    ffmpeg: PathBuf,
    cancelled: Arc<AtomicBool>,
    mut progress: impl FnMut(f64, Option<f64>),
) -> Result<LocalWaveform, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Cancelled".into());
    }
    let path = verified_asset_path(&asset)?;
    let bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if bytes > MAX_FILE_BYTES {
        return Err("Audio file exceeds the 8 GB waveform-analysis limit".into());
    }
    let expected_duration = asset
        .duration_seconds
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or("Registered asset has no verified audio duration")?;
    if expected_duration > MAX_DURATION_SECONDS {
        return Err("Waveform analysis supports audio up to four hours".into());
    }
    let hash_path = path.clone();
    let hash_cancel = cancelled.clone();
    let before =
        tokio::task::spawn_blocking(move || hash_file_cancellable(&hash_path, &hash_cancel))
            .await
            .map_err(|e| e.to_string())??;
    if before != expected_hash || before != asset.hash {
        return Err("Audio file changed before waveform analysis".into());
    }

    let output = decode_mono_peaks(
        &path,
        &ffmpeg,
        bin_count,
        expected_duration,
        cancelled.clone(),
        &mut progress,
    )
    .await?;
    let hash_path = path.clone();
    let hash_cancel = cancelled.clone();
    let after =
        tokio::task::spawn_blocking(move || hash_file_cancellable(&hash_path, &hash_cancel))
            .await
            .map_err(|e| e.to_string())??;
    if after != expected_hash || before != after {
        return Err("Audio file changed during waveform analysis".into());
    }
    if cancelled.load(Ordering::Acquire) {
        return Err("Cancelled".into());
    }
    let record = LocalWaveform {
        asset_id: asset.id,
        file_hash: expected_hash,
        analyzer_version: ANALYZER_VERSION.into(),
        bin_count,
        duration_seconds: output.duration_seconds,
        bins: output.bins,
        scope: "wholeTrack".into(),
        complete: true,
        measured_at: chrono::Utc::now().to_rfc3339(),
    };
    record.validate()?;
    let still_registered = store
        .get_asset_by_id(&record.asset_id)?
        .is_some_and(|current| current.hash == record.file_hash);
    if !still_registered {
        return Err("Selected asset changed registration during waveform analysis".into());
    }
    Ok(record)
}

#[derive(Debug)]
struct DecodedPeaks {
    bins: Vec<Option<WaveformBin>>,
    duration_seconds: f64,
}

async fn decode_mono_peaks(
    path: &Path,
    ffmpeg: &Path,
    bin_count: usize,
    expected_duration: f64,
    cancelled: Arc<AtomicBool>,
    progress: &mut impl FnMut(f64, Option<f64>),
) -> Result<DecodedPeaks, String> {
    let mut child = Command::new(ffmpeg)
        .args([
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path)
        .args([
            "-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-ar", "8000", "-f", "f32le",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg for waveform analysis: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("FFmpeg waveform output is unavailable")?;
    let started = Instant::now();
    let mut read_buffer = vec![0u8; 32 * 1024];
    let mut carry = Vec::<u8>::with_capacity(3);
    let mut bins = vec![None::<WaveformBin>; bin_count];
    let mut sample_count = 0u64;
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill().await;
            return Err("Cancelled".into());
        }
        if started.elapsed() > PROCESS_TIMEOUT {
            let _ = child.kill().await;
            return Err("Waveform analysis exceeded its 20 minute limit".into());
        }
        let read = tokio::select! {
            read = stdout.read(&mut read_buffer) => read.map_err(|e| format!("Could not read FFmpeg waveform output: {e}"))?,
            _ = tokio::time::sleep(Duration::from_millis(30)) => continue,
        };
        if read == 0 {
            break;
        }
        carry.extend_from_slice(&read_buffer[..read]);
        let whole_bytes = carry.len() / 4 * 4;
        for bytes in carry[..whole_bytes].chunks_exact(4) {
            let sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if !sample.is_finite() {
                let _ = child.kill().await;
                return Err("FFmpeg returned a non-finite waveform sample".into());
            }
            let bounded = sample.clamp(-1.0, 1.0);
            let second = sample_count as f64 / f64::from(OUTPUT_SAMPLE_RATE);
            let expected_tolerance = 0.5_f64.max(expected_duration * 0.005);
            if second > expected_duration + expected_tolerance {
                let _ = child.kill().await;
                return Err("Decoded audio exceeded the registered asset duration".into());
            }
            let index = ((second / expected_duration) * bin_count as f64).floor() as usize;
            if let Some(bin) = bins.get_mut(index.min(bin_count - 1)) {
                match bin {
                    Some(existing) => {
                        existing.min = existing.min.min(bounded);
                        existing.max = existing.max.max(bounded);
                    }
                    None => {
                        *bin = Some(WaveformBin {
                            min: bounded,
                            max: bounded,
                        })
                    }
                }
            }
            sample_count = sample_count.saturating_add(1);
            if sample_count > (MAX_DURATION_SECONDS * f64::from(OUTPUT_SAMPLE_RATE)) as u64 {
                let _ = child.kill().await;
                return Err("Decoded audio exceeded the four hour waveform limit".into());
            }
        }
        carry.drain(..whole_bytes);
        progress(
            sample_count as f64 / f64::from(OUTPUT_SAMPLE_RATE),
            Some(expected_duration),
        );
    }
    let status = child
        .wait()
        .await
        .map_err(|e| format!("Could not finish FFmpeg waveform analysis: {e}"))?;
    if !status.success() {
        return Err("FFmpeg could not decode the selected audio file".into());
    }
    if !carry.is_empty() || sample_count == 0 {
        return Err("FFmpeg returned incomplete waveform samples".into());
    }
    let duration_seconds = sample_count as f64 / f64::from(OUTPUT_SAMPLE_RATE);
    let tolerance = 0.5_f64.max(expected_duration * 0.005);
    if (duration_seconds - expected_duration).abs() > tolerance || bins.iter().any(Option::is_none)
    {
        return Err("Decoded audio did not cover the complete registered asset duration".into());
    }
    progress(duration_seconds, Some(duration_seconds));
    Ok(DecodedPeaks {
        bins,
        duration_seconds,
    })
}

fn verified_asset_path(asset: &LocalAsset) -> Result<PathBuf, String> {
    let path = Path::new(&asset.path);
    if !path.is_absolute() {
        return Err("Registered audio path is not absolute".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Registered audio file is unavailable: {e}"))?;
    if !canonical.is_file() {
        return Err("Registered audio path is not a regular file".into());
    }
    Ok(canonical)
}

fn hash_file(path: &Path) -> Result<String, String> {
    hash_file_cancellable(path, &AtomicBool::new(false))
}

fn hash_file_cancellable(path: &Path, cancelled: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Track;

    fn write_fixture_wav(path: &Path, seconds: u32) {
        let sample_rate = 8_000u32;
        let samples = sample_rate * seconds;
        let data_bytes = samples * 2;
        let mut wav = Vec::with_capacity(44 + data_bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for index in 0..samples {
            let amplitude = if index < sample_rate {
                0.25f32
            } else {
                -0.5f32
            };
            let sample = (amplitude * i16::MAX as f32).round() as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, wav).unwrap();
    }

    fn registered_asset(store: &Store, path: &Path, id: &str, duration: f64) -> LocalAsset {
        let track = Track {
            id: format!("fixture:{id}"),
            provider: "fixture".into(),
            provider_id: id.into(),
            source_url: String::new(),
            artist: "Synthetic fixture".into(),
            title: "Waveform test".into(),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let hash = crate::media::hash_file(path).unwrap();
        let metadata = std::fs::metadata(path).unwrap();
        let asset = LocalAsset {
            id: id.into(),
            recording_id: track.id.clone(),
            path: path.to_string_lossy().into_owned(),
            hash,
            source_codec: "wav".into(),
            final_codec: "wav".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_depth: Some(16),
            duration_seconds: Some(duration),
            size_bytes: Some(metadata.len()),
            bitrate_origin: Some("format".into()),
            processing: "ready".into(),
            track,
        };
        store.save_asset(&asset).unwrap();
        asset
    }

    fn valid_record(asset_id: &str, bin_count: usize) -> LocalWaveform {
        LocalWaveform {
            asset_id: asset_id.into(),
            file_hash: "a".repeat(64),
            analyzer_version: ANALYZER_VERSION.into(),
            bin_count,
            duration_seconds: 2.0,
            bins: vec![
                Some(WaveformBin {
                    min: -0.5,
                    max: 0.25
                });
                bin_count
            ],
            scope: "wholeTrack".into(),
            complete: true,
            measured_at: "2026-10-25T00:00:00Z".into(),
        }
    }

    #[test]
    fn active_waveform_job_capacity_is_two_and_terminal_jobs_do_not_block() {
        let make_job = |id: &str, state: &str| {
            let status = WaveformJobStatus {
                job_id: id.into(),
                status: state.into(),
                asset_id: format!("asset-{id}"),
                file_hash: "a".repeat(64),
                analyzer_version: ANALYZER_VERSION.into(),
                bin_count: 128,
                duration_seconds: Some(60.0),
                processed_seconds: 0.0,
                error: None,
            };
            Job {
                status: Arc::new(Mutex::new(status)),
                cancelled: Arc::new(AtomicBool::new(false)),
            }
        };
        let mut jobs = HashMap::new();
        jobs.insert("one".into(), make_job("one", "running"));
        jobs.insert("two".into(), make_job("two", "queued"));
        assert!(ensure_active_job_capacity(&jobs)
            .unwrap_err()
            .contains("another waveform analysis"));

        jobs.get("one").unwrap().status.lock().unwrap().status = "completed".into();
        assert!(ensure_active_job_capacity(&jobs).is_ok());
    }

    #[tokio::test]
    async fn registered_synthetic_wav_decodes_complete_peaks_and_cache_is_hash_bound() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let audio_path = directory.path().join("synthetic.wav");
        write_fixture_wav(&audio_path, 2);
        let asset = registered_asset(&store, &audio_path, "waveform-asset", 2.0);
        let registered = store.get_asset_by_id(&asset.id).unwrap().unwrap();

        let record = analyze_registered_asset(
            store.clone(),
            registered,
            asset.hash.clone(),
            20,
            crate::tools::executable("", "ffmpeg"),
            Arc::new(AtomicBool::new(false)),
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(record.complete);
        assert_eq!(record.scope, "wholeTrack");
        assert_eq!(record.bins.len(), 20);
        assert!((record.duration_seconds - 2.0).abs() < 0.01);
        assert!(record.bins.iter().all(Option::is_some));
        for bin in record.bins.iter().take(10).flatten() {
            assert!(
                bin.min > 0.24 && bin.max > 0.24,
                "unexpected positive peak bin: {bin:?}"
            );
        }
        for bin in record.bins.iter().skip(10).flatten() {
            assert!(
                bin.min < -0.49 && bin.max < -0.49,
                "unexpected negative peak bin: {bin:?}"
            );
        }

        store
            .set_json_preference(CACHE_KEY, &upsert_cache(None, record.clone()))
            .unwrap();
        let cached = cached_waveform(store.clone(), asset.id.clone(), asset.hash.clone(), 20)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached, record);

        write_fixture_wav(&audio_path, 2);
        let mut changed = std::fs::read(&audio_path).unwrap();
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(&audio_path, changed).unwrap();
        let error = cached_waveform(store, asset.id, asset.hash, 20)
            .await
            .unwrap_err();
        assert!(error.contains("changed since waveform analysis"));
    }

    #[tokio::test]
    async fn short_file_with_a_long_registered_duration_cannot_produce_complete_bins() {
        let directory = tempfile::tempdir().unwrap();
        let audio_path = directory.path().join("short-synthetic.wav");
        write_fixture_wav(&audio_path, 2);
        let result = decode_mono_peaks(
            &audio_path,
            &crate::tools::executable("", "ffmpeg"),
            20,
            30.0,
            Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
        )
        .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("complete registered asset duration"));
    }

    #[test]
    fn malformed_bins_and_incomplete_records_are_rejected() {
        let valid = valid_record("asset", 2);
        assert!(valid.validate().is_ok());

        let mut missing = valid.clone();
        missing.bins[0] = None;
        assert!(missing.validate().is_err());

        let mut out_of_range = valid.clone();
        out_of_range.bins[0] = Some(WaveformBin {
            min: -1.1,
            max: 0.0,
        });
        assert!(out_of_range.validate().is_err());

        let mut reversed = valid.clone();
        reversed.bins[0] = Some(WaveformBin {
            min: 0.5,
            max: -0.5,
        });
        assert!(reversed.validate().is_err());

        let mut non_finite = valid.clone();
        non_finite.bins[0] = Some(WaveformBin {
            min: f32::NAN,
            max: 0.5,
        });
        assert!(non_finite.validate().is_err());

        let mut inconsistent_count = valid;
        inconsistent_count.bin_count = 3;
        assert!(inconsistent_count.validate().is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_decode_kills_worker_and_returns_no_waveform() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let fake_ffmpeg = directory.path().join("streaming-ffmpeg");
        std::fs::write(
            &fake_ffmpeg,
            "#!/bin/sh\nprintf '\\000\\000\\000\\000'\nexec sleep 10\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_ffmpeg).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_ffmpeg, permissions).unwrap();

        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel_after_start = cancellation.clone();
        let cancellation_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_after_start.store(true, Ordering::Release);
        });
        let result = decode_mono_peaks(
            Path::new("unused-synthetic-input.wav"),
            &fake_ffmpeg,
            10,
            1.0,
            cancellation,
            &mut |_, _| {},
        )
        .await;
        cancellation_task.await.unwrap();
        assert_eq!(result.unwrap_err(), "Cancelled");
    }
}
