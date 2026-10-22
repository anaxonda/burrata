//! Media validation, selected metadata tagging and collision-safe library publication.
use crate::models::{AudioFacts, Candidate, Chapter, LocalAsset, Settings, Track};
use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    picture::{MimeType, Picture, PictureType},
    prelude::Accessor,
    probe::Probe,
    tag::{ItemKey, ItemValue, Tag, TagItem, TagType},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::Duration,
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::OwnedMutexGuard;

const BPM_SAMPLE_RATE: u32 = 8_000;
const BPM_FRAME_SAMPLES: usize = 80;
const KEY_SAMPLE_RATE: u32 = 22_050;
const KEY_MAX_SECONDS: usize = 15 * 60;
const KEY_WINDOW_SECONDS: usize = 30;
const KEY_MIN_WINDOW_SECONDS: usize = 8;
// LibKeyFinder's cosine fit is typically around .25-.4 for ordinary music.
// A high fit threshold rejects clean tonal fixtures; the runner-up margin and
// cross-section agreement provide the useful ambiguity guards.
const KEY_MIN_PROFILE_FIT: f64 = 0.20;
const KEY_MIN_PROFILE_MARGIN: f64 = 0.01;
const BURRATA_DATE_KIND_PREFIX: &str = "BURRATA_DATE_KIND=";

fn append_date_kind_marker(tag: &mut Tag, marker: &str) {
    let existing = tag.get_string(&ItemKey::Comment).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim() == format!("{BURRATA_DATE_KIND_PREFIX}{marker}"))
    {
        return;
    }
    let mut comment = existing.to_owned();
    if !comment.is_empty() {
        comment.push('\n');
    }
    comment.push_str(BURRATA_DATE_KIND_PREFIX);
    comment.push_str(marker);
    tag.insert_text(ItemKey::Comment, comment);
}

fn date_kind_marker(comment: Option<String>) -> Option<String> {
    comment?.lines().find_map(|line| {
        line.trim()
            .strip_prefix(BURRATA_DATE_KIND_PREFIX)
            .map(str::to_owned)
    })
}

static RETAG_LOCKS: OnceLock<StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

async fn lock_asset_retag(asset_id: &str) -> OwnedMutexGuard<()> {
    let locks = RETAG_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let lock = {
        let mut locks = locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(asset_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BpmAnalysis {
    pub bpm: f64,
    /// Normalized onset autocorrelation strength, from 0 to 1.
    pub confidence: f64,
    /// A near-tied half/double-time candidate, retained as uncertainty rather than applied.
    pub related_tempo_bpm: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyAnalysis {
    pub key: String,
    pub confidence: f64,
    pub whole_track_key: String,
    pub window_keys: Vec<Option<String>>,
}

unsafe extern "C" {
    fn burrata_keyfinder_analyze(
        samples: *const f32,
        sample_count: usize,
        sample_rate: u32,
        profile_fit: *mut f64,
        profile_margin: *mut f64,
    ) -> i32;
}

/// Estimate a tempo from decoded audio. Returns no estimate when the signal does
/// not contain a sufficiently regular beat pattern.
pub async fn detect_bpm(path: &Path, settings: &Settings) -> Result<Option<BpmAnalysis>, String> {
    let mut command = Command::new(crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"));
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            "1",
            "-ar",
            &BPM_SAMPLE_RATE.to_string(),
            "-f",
            "s16le",
            "pipe:1",
        ]);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot run FFmpeg for BPM analysis: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("FFmpeg BPM analysis has no audio output")?;
    let decoded = tokio::time::timeout(Duration::from_secs(600), async {
        let mut envelope = Vec::new();
        let mut frame_energy = 0.0;
        let mut frame_count = 0usize;
        let mut previous_level = 0.0;
        let mut frame_samples = 0usize;
        let mut bytes = [0u8; 8192];
        let mut pending_byte = None;
        loop {
            let count = stdout
                .read(&mut bytes)
                .await
                .map_err(|e| format!("Cannot read FFmpeg BPM audio: {e}"))?;
            if count == 0 {
                break;
            }
            let mut offset = 0;
            if let Some(first) = pending_byte.take() {
                let sample = i16::from_le_bytes([first, bytes[0]]);
                add_bpm_sample(
                    sample,
                    &mut frame_energy,
                    &mut frame_count,
                    &mut frame_samples,
                    &mut previous_level,
                    &mut envelope,
                );
                offset = 1;
            }
            while offset + 1 < count {
                let sample = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                add_bpm_sample(
                    sample,
                    &mut frame_energy,
                    &mut frame_count,
                    &mut frame_samples,
                    &mut previous_level,
                    &mut envelope,
                );
                offset += 2;
            }
            if offset < count {
                pending_byte = Some(bytes[offset]);
            }
        }
        if frame_count > 0 {
            finish_bpm_frame(
                frame_energy,
                frame_count,
                &mut previous_level,
                &mut envelope,
            );
        }
        let status = child
            .wait()
            .await
            .map_err(|e| format!("Cannot finish FFmpeg BPM analysis: {e}"))?;
        if !status.success() {
            return Err("FFmpeg could not decode audio for BPM analysis".into());
        }
        Ok::<_, String>(envelope)
    })
    .await
    .map_err(|_| "BPM analysis timed out after 10 minutes".to_string())??;
    tokio::task::spawn_blocking(move || estimate_bpm(&decoded))
        .await
        .map_err(|e| format!("BPM analysis worker failed: {e}"))
}

/// Estimate a global key only when the full-track classifier agrees with at
/// least three of five evenly distributed excerpts. LibKeyFinder reports
/// SILENCE for non-tonal input; disagreement, short audio and silence remain
/// unknown instead of being written as a guessed key.
pub async fn detect_key(path: &Path, settings: &Settings) -> Result<Option<KeyAnalysis>, String> {
    let mut command = Command::new(crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"));
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            "1",
            "-ar",
            &KEY_SAMPLE_RATE.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ]);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot run FFmpeg for key analysis: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("FFmpeg key analysis has no audio output")?;
    let decoded = tokio::time::timeout(Duration::from_secs(600), async {
        let max_bytes = KEY_SAMPLE_RATE as usize * KEY_MAX_SECONDS * 4;
        let mut bytes = Vec::with_capacity((KEY_SAMPLE_RATE as usize * 60).min(max_bytes));
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let count = stdout
                .read(&mut chunk)
                .await
                .map_err(|e| format!("Cannot read decoded audio for key analysis: {e}"))?;
            if count == 0 {
                break;
            }
            if bytes.len().saturating_add(count) > max_bytes {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err("Key analysis is limited to audio up to 15 minutes".to_string());
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        let status = child
            .wait()
            .await
            .map_err(|e| format!("Cannot finish FFmpeg key analysis: {e}"))?;
        if !status.success() {
            return Err("FFmpeg could not decode audio for key analysis".into());
        }
        let samples = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>();
        Ok::<_, String>(samples)
    })
    .await
    .map_err(|_| "Key analysis timed out after 10 minutes".to_string())??;
    tokio::task::spawn_blocking(move || estimate_key(&decoded))
        .await
        .map_err(|e| format!("Key analysis worker failed: {e}"))
}

fn key_name(key: i32) -> Option<&'static str> {
    const KEYS: [&str; 24] = [
        "A", "Am", "Bb", "Bbm", "B", "Bm", "C", "Cm", "Db", "Dbm", "D", "Dm", "Eb", "Ebm", "E",
        "Em", "F", "Fm", "Gb", "Gbm", "G", "Gm", "Ab", "Abm",
    ];
    usize::try_from(key)
        .ok()
        .and_then(|key| KEYS.get(key).copied())
}

fn keyfinder_analyze(samples: &[f32]) -> Option<(&'static str, f64, f64)> {
    if samples.is_empty()
        || samples
            .iter()
            .all(|sample| !sample.is_finite() || sample.abs() < 1e-7)
    {
        return None;
    }
    // LibKeyFinder's enum uses 24 major/minor identities followed by SILENCE.
    // Its wrapper returns SILENCE as 24 and failures as -1.
    let (mut profile_fit, mut profile_margin) = (0.0, 0.0);
    let value = unsafe {
        burrata_keyfinder_analyze(
            samples.as_ptr(),
            samples.len(),
            KEY_SAMPLE_RATE,
            &mut profile_fit,
            &mut profile_margin,
        )
    };
    key_name(value).map(|name| (name, profile_fit, profile_margin))
}

fn estimate_key(samples: &[f32]) -> Option<KeyAnalysis> {
    let minimum = KEY_SAMPLE_RATE as usize * KEY_MIN_WINDOW_SECONDS;
    if samples.len() < minimum {
        return None;
    }
    let (whole_track_key, whole_fit, whole_margin) = keyfinder_analyze(samples)?;
    if whole_fit < KEY_MIN_PROFILE_FIT || whole_margin < KEY_MIN_PROFILE_MARGIN {
        return None;
    }
    let whole_track_key = whole_track_key.to_string();
    let window_len = (samples.len() / 5).min(KEY_SAMPLE_RATE as usize * KEY_WINDOW_SECONDS);
    if window_len < minimum {
        return None;
    }
    let mut window_keys = Vec::with_capacity(5);
    for part in 0..5 {
        let center = samples.len() * (part * 2 + 1) / 10;
        let start = center
            .saturating_sub(window_len / 2)
            .min(samples.len() - window_len);
        let key = keyfinder_analyze(&samples[start..start + window_len])
            .filter(|(_, fit, margin)| {
                *fit >= KEY_MIN_PROFILE_FIT && *margin >= KEY_MIN_PROFILE_MARGIN
            })
            .map(|(key, _, _)| key.to_owned());
        window_keys.push(key);
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for key in window_keys.iter().flatten() {
        *counts.entry(key.clone()).or_default() += 1;
    }
    let Some((key, votes)) = counts.into_iter().max_by_key(|(_, votes)| *votes) else {
        return None;
    };
    if votes < 3 || key != whole_track_key {
        return None;
    }
    Some(KeyAnalysis {
        key,
        confidence: votes as f64 / 5.0,
        whole_track_key,
        window_keys,
    })
}

fn add_bpm_sample(
    sample: i16,
    frame_energy: &mut f64,
    frame_count: &mut usize,
    frame_samples: &mut usize,
    previous_level: &mut f64,
    envelope: &mut Vec<f64>,
) {
    let sample = f64::from(sample) / f64::from(i16::MAX);
    *frame_energy += sample * sample;
    *frame_count += 1;
    *frame_samples += 1;
    if *frame_samples == BPM_FRAME_SAMPLES {
        finish_bpm_frame(*frame_energy, *frame_count, previous_level, envelope);
        *frame_energy = 0.0;
        *frame_count = 0;
        *frame_samples = 0;
    }
}

fn finish_bpm_frame(energy: f64, count: usize, previous_level: &mut f64, envelope: &mut Vec<f64>) {
    let level = (energy / count as f64).sqrt();
    envelope.push((level - *previous_level).max(0.0));
    *previous_level = level;
}

fn estimate_bpm(envelope: &[f64]) -> Option<BpmAnalysis> {
    let frame_seconds = BPM_FRAME_SAMPLES as f64 / f64::from(BPM_SAMPLE_RATE);
    let min_lag = (60.0 / (200.0 * frame_seconds)).round() as usize;
    let max_lag = (60.0 / (60.0 * frame_seconds)).round() as usize;
    if envelope.len() <= max_lag + min_lag * 8 {
        return None;
    }
    let energy: f64 = envelope.iter().map(|v| v * v).sum();
    if energy <= f64::EPSILON {
        return None;
    }
    let mut correlations = vec![0.0; max_lag + 1];
    for lag in min_lag..=max_lag {
        if envelope.len() / lag < 8 {
            continue;
        }
        let (mut dot, mut left_energy, mut right_energy) = (0.0, 0.0, 0.0);
        for index in lag..envelope.len() {
            let left = envelope[index];
            let right = envelope[index - lag];
            dot += left * right;
            left_energy += left * left;
            right_energy += right * right;
        }
        let denominator = (left_energy * right_energy).sqrt();
        if denominator > f64::EPSILON {
            correlations[lag] = dot / denominator;
        }
    }
    let (_, strongest_value) = correlations[min_lag..=max_lag]
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))?;
    let strongest = *strongest_value;
    if !strongest.is_finite() || strongest < 0.35 {
        return None;
    }
    // Preserve the previous conservative pick when two octave-related
    // periods are nearly tied. Otherwise, consider supported local peaks
    // before a longer global peak: exact high tempos can have a stronger peak
    // at a three-beat multiple and would otherwise collapse to 60-70 BPM.
    let conventional_lag =
        (min_lag..=max_lag).find(|&lag| correlations[lag] >= strongest * 0.97)?;
    let candidate_lag = (min_lag..conventional_lag)
        .find(|&lag| {
            let value = correlations[lag];
            // A faster peak can explain a longer period only when that period
            // spans a whole number of its beats. Independent rhythms (for
            // example 32 and 43 frames) must not replace the stronger peak.
            // Rounding each measured period to a frame permits at most
            // (multiple + 1) / 2 frames of disagreement.
            let multiple = (conventional_lag + lag / 2) / lag;
            let subperiod = (2..=3).contains(&multiple)
                && conventional_lag.abs_diff(lag * multiple) * 2 <= multiple + 1;
            subperiod
                && value >= 0.35
                && value >= strongest * 0.65
                && value >= correlations[lag.saturating_sub(1)]
                && value >= correlations.get(lag + 1).copied().unwrap_or(0.0)
                && onset_occupancy_per_period(envelope, lag) >= 0.70
        })
        .unwrap_or(conventional_lag);
    let octave_ambiguous = candidate_lag < conventional_lag
        && conventional_lag.abs_diff(candidate_lag.saturating_mul(2)) <= 1
        && correlations[candidate_lag] >= correlations[conventional_lag] * 0.90;
    let lag = if octave_ambiguous {
        conventional_lag
    } else {
        candidate_lag
    };
    let selected_score = correlations[lag];
    let related_lag = [lag / 2, lag.saturating_mul(2)]
        .into_iter()
        .filter(|related| *related >= min_lag && *related <= max_lag && *related != lag)
        .max_by(|left, right| correlations[*left].total_cmp(&correlations[*right]));
    let related_tempo_bpm = related_lag
        .filter(|related| {
            let related_score = correlations[*related];
            related_score.min(selected_score) >= related_score.max(selected_score) * 0.90
        })
        .map(|lag| bpm_for_lag(&correlations, lag, frame_seconds))
        .filter(|bpm| (60.0..=200.0).contains(bpm));
    let bpm = bpm_for_lag(&correlations, lag, frame_seconds);
    (60.0..=200.0).contains(&bpm).then_some(BpmAnalysis {
        bpm,
        confidence: correlations[lag].clamp(0.0, 1.0),
        related_tempo_bpm,
    })
}

fn bpm_for_lag(correlations: &[f64], lag: usize, frame_seconds: f64) -> f64 {
    let left = correlations
        .get(lag.wrapping_sub(1))
        .copied()
        .unwrap_or(0.0);
    let right = correlations.get(lag + 1).copied().unwrap_or(0.0);
    let curvature = left - 2.0 * correlations[lag] + right;
    let shift = if curvature.abs() > 1e-12 {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (60.0 / ((lag as f64 + shift) * frame_seconds) * 10.0).round() / 10.0
}

/// Require a shorter-lag candidate to contain high-energy onset peaks in most
/// lag-sized bins. This is a coverage gate, not independent beat-phase
/// detection; autocorrelation still provides the periodic alignment.
fn onset_occupancy_per_period(envelope: &[f64], lag: usize) -> f64 {
    if lag == 0 || envelope.is_empty() {
        return 0.0;
    }
    let threshold = envelope.iter().copied().fold(0.0_f64, f64::max) * 0.30;
    if threshold <= f64::EPSILON {
        return 0.0;
    }
    let full_cycles = envelope.len() / lag;
    if full_cycles == 0 {
        return 0.0;
    }
    let peaks = (0..envelope.len())
        .filter(|index| {
            let value = envelope[*index];
            let previous = (*index).checked_sub(1).map_or(0.0, |at| envelope[at]);
            let next = envelope.get(*index + 1).copied().unwrap_or(0.0);
            value >= threshold && value >= previous && value > next
        })
        .collect::<Vec<_>>();
    let mut best = 0.0_f64;
    for phase in 0..lag {
        let mut occupied_cycles = vec![false; full_cycles];
        for peak in peaks.iter().copied().filter(|peak| *peak >= phase) {
            let cycle = (peak - phase) / lag;
            if let Some(occupied) = occupied_cycles.get_mut(cycle) {
                *occupied = true;
            }
        }
        let occupied = occupied_cycles.into_iter().filter(|value| *value).count();
        best = best.max(occupied as f64 / full_cycles as f64);
    }
    best
}

fn add_bpm_analysis(track: &Track, analysis: Option<BpmAnalysis>, error: Option<&str>) -> Track {
    if track.bpm.is_some() {
        return track.clone();
    }
    let mut track = track.clone();
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    if let Some(analysis) = analysis {
        track.bpm = Some(analysis.bpm);
        track.provenance["bpmAnalysis"] = serde_json::json!({
            "source": "decoded audio",
            "method": "10ms onset autocorrelation",
            "confidence": analysis.confidence,
            "confidenceMeaning": "normalized onset-autocorrelation strength, not a probability",
            "relatedTempoAlternativeBpm": analysis.related_tempo_bpm,
            "tempoAmbiguity": analysis.related_tempo_bpm.is_some(),
            "bpm": analysis.bpm
        });
    } else if let Some(error) = error {
        track.provenance["bpmAnalysis"] = serde_json::json!({
            "source": "decoded audio",
            "method": "10ms onset autocorrelation",
            "status": "unavailable",
            "reason": error
        });
    } else {
        track.provenance["bpmAnalysis"] = serde_json::json!({
            "source": "decoded audio",
            "method": "10ms onset autocorrelation",
            "status": "no confident estimate"
        });
    }
    track
}

fn add_key_analysis(track: &Track, analysis: Option<KeyAnalysis>, error: Option<&str>) -> Track {
    if track
        .key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
    {
        return track.clone();
    }
    let mut track = track.clone();
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    let method = "LibKeyFinder 2.2.8; whole track and five distributed excerpts";
    if let Some(analysis) = analysis {
        track.key = Some(analysis.key.clone());
        let agreement_count = (analysis.confidence * 5.0).round() as usize;
        track.provenance["keyAnalysis"] = serde_json::json!({
            "source": "decoded audio",
            "method": method,
            "version": "libkeyfinder-2.2.8",
            "status": "complete",
            "confidence": analysis.confidence,
            "agreementCount": agreement_count,
            "wholeTrackKey": analysis.whole_track_key,
            "windowKeys": analysis.window_keys
        });
    } else if let Some(error) = error {
        track.provenance["keyAnalysis"] = serde_json::json!({
            "source": "decoded audio",
            "method": method,
            "version": "libkeyfinder-2.2.8",
            "status": "unavailable",
            "reason": error
        });
    } else {
        track.provenance["keyAnalysis"] = serde_json::json!({
            "source": "decoded audio",
            "method": method,
            "version": "libkeyfinder-2.2.8",
            "status": "no_confident_estimate"
        });
    }
    track
}

fn is_manual_field(track: &Track, field: &str) -> bool {
    track.provenance["manualFields"][field] == true
}

async fn analyze_missing_fields(
    path: &Path,
    track: &Track,
    settings: &Settings,
    force: bool,
) -> Track {
    let manual_bpm = is_manual_field(track, "bpm");
    let manual_key = is_manual_field(track, "key");
    let mut analyzed = track.clone();
    if force && !manual_bpm {
        analyzed.bpm = None;
        if let Some(provenance) = analyzed.provenance.as_object_mut() {
            provenance.remove("bpmAnalysis");
        }
    }
    if force && !manual_key {
        analyzed.key = None;
        if let Some(provenance) = analyzed.provenance.as_object_mut() {
            provenance.remove("keyAnalysis");
        }
    }
    if !manual_bpm && analyzed.bpm.is_none() {
        analyzed = match detect_bpm(path, settings).await {
            Ok(analysis) => add_bpm_analysis(&analyzed, analysis, None),
            Err(error) => add_bpm_analysis(&analyzed, None, Some(&error)),
        };
    }
    if !manual_key
        && analyzed
            .key
            .as_deref()
            .is_none_or(|key| key.trim().is_empty())
    {
        analyzed = match detect_key(path, settings).await {
            Ok(analysis) => add_key_analysis(&analyzed, analysis, None),
            Err(error) => add_key_analysis(&analyzed, None, Some(&error)),
        };
    }
    analyzed
}

fn tag_version(version: &str) -> String {
    let version = version.trim();
    if let Some((prefix, duration)) = version
        .strip_suffix(')')
        .and_then(|value| value.rsplit_once(" ("))
    {
        let segments = duration.split(':').collect::<Vec<_>>();
        let is_clock = (2..=3).contains(&segments.len())
            && segments
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            && segments
                .last()
                .and_then(|v| v.parse::<u8>().ok())
                .is_some_and(|v| v < 60)
            && (segments.len() == 2 || segments[1].parse::<u8>().ok().is_some_and(|v| v < 60));
        if is_clock && !prefix.trim().is_empty() {
            return prefix.trim_end().to_string();
        }
    }
    version.to_string()
}

fn tag_title(track: &Track) -> String {
    let version = track
        .version
        .as_deref()
        .map(tag_version)
        .unwrap_or_default();
    match track.version.as_deref().filter(|v| !v.trim().is_empty()) {
        Some(_) if !track.title.to_lowercase().contains(&version.to_lowercase()) => {
            format!("{} ({version})", track.title)
        }
        _ => track.title.clone(),
    }
}

#[derive(Debug)]
struct MediaInfo {
    codec: String,
    bitrate: Option<f64>,
    bitrate_origin: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u32>,
    bit_depth: Option<u32>,
    duration: f64,
    size_bytes: Option<u64>,
}

pub async fn inspect_quality_facts(path: &Path, settings: &Settings) -> Result<AudioFacts, String> {
    let info = probe(path, settings).await?;
    Ok(AudioFacts {
        codec: info.codec,
        bitrate_kbps: info.bitrate,
        bitrate_origin: info.bitrate_origin,
        sample_rate: info.sample_rate,
        channels: info.channels,
        bit_depth: info.bit_depth,
        duration_seconds: Some(info.duration),
        size_bytes: info.size_bytes,
    })
}

fn resolve_bitrate(
    stream_bitrate_kbps: Option<f64>,
    format_bitrate_kbps: Option<f64>,
    size_bytes: Option<u64>,
    duration_seconds: f64,
) -> (Option<f64>, Option<String>) {
    if let Some(value) = stream_bitrate_kbps.filter(|value| value.is_finite() && *value > 0.0) {
        (Some(value), Some("stream".into()))
    } else if let Some(value) =
        format_bitrate_kbps.filter(|value| value.is_finite() && *value > 0.0)
    {
        (Some(value), Some("format-average".into()))
    } else if let (Some(size), duration) = (size_bytes, duration_seconds) {
        (
            Some(size as f64 * 8.0 / duration / 1000.0),
            Some("file-size-estimate".into()),
        )
    } else {
        (None, None)
    }
}

pub async fn verify_lossless_fallback(path: &Path, settings: &Settings) -> Result<(), String> {
    let info = probe(path, settings).await?;
    if matches!(
        info.codec.as_str(),
        "flac" | "alac" | "wavpack" | "ape" | "tta"
    ) || info.codec.starts_with("pcm_")
    {
        Ok(())
    } else {
        Err(format!(
            "Fallback audio is {}, but lossless audio is required",
            info.codec
        ))
    }
}

async fn probe(path: &Path, settings: &Settings) -> Result<MediaInfo, String> {
    let mut command = Command::new(crate::tools::executable(&settings.ffprobe_path, "ffprobe"));
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,bit_rate,duration,sample_rate,channels,bits_per_sample,bits_per_raw_sample:format=duration,bit_rate",
            "-of",
            "json",
        ])
        .arg(path);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| "Media inspection timed out".to_string())?
        .map_err(|e| format!("Cannot run ffprobe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Invalid media: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Invalid ffprobe output: {e}"))?;
    let stream = value["streams"]
        .as_array()
        .and_then(|v| v.first())
        .ok_or("File contains no audio stream")?;
    let number = |v: &serde_json::Value| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    let duration = number(&stream["duration"])
        .or_else(|| number(&value["format"]["duration"]))
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or("Audio duration is missing or invalid")?;
    let codec = stream["codec_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Audio codec is missing")?
        .to_owned();
    let stream_bitrate = number(&stream["bit_rate"])
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n / 1000.0);
    let format_bitrate = number(&value["format"]["bit_rate"])
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n / 1000.0);
    let size_bytes = std::fs::metadata(path).ok().map(|metadata| metadata.len());
    let (bitrate, bitrate_origin) =
        resolve_bitrate(stream_bitrate, format_bitrate, size_bytes, duration);
    let parse_u32 = |value: &serde_json::Value| {
        value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| value.as_str()?.parse().ok())
    };
    let sample_rate =
        parse_u32(&stream["sample_rate"]).or_else(|| (codec == "opus").then_some(48_000));
    let channels = parse_u32(&stream["channels"]).or_else(|| (codec == "opus").then_some(2));
    let bit_depth = parse_u32(&stream["bits_per_raw_sample"])
        .filter(|value| *value > 0)
        .or_else(|| parse_u32(&stream["bits_per_sample"]).filter(|value| *value > 0));
    Ok(MediaInfo {
        codec,
        bitrate,
        bitrate_origin,
        sample_rate,
        channels,
        bit_depth,
        duration,
        size_bytes,
    })
}

/// Return the canonical library extension for a probed audio codec.
pub fn published_audio_extension(codec: &str, source_path: &Path) -> Result<&'static str, String> {
    match codec {
        "mp3" => Ok("mp3"),
        "aac" | "alac" => Ok("m4a"),
        "flac" => Ok("flac"),
        "opus" => Ok("opus"),
        "vorbis" => Ok("ogg"),
        value if value.starts_with("pcm_") => {
            if value.ends_with("be")
                || source_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(extension.to_ascii_lowercase().as_str(), "aif" | "aiff")
                    })
            {
                Ok("aiff")
            } else {
                Ok("wav")
            }
        }
        _ => Err(format!("Unsupported tagging codec: {codec}")),
    }
}

fn safe_component(value: &str) -> String {
    let mut result: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(100)
        .collect();
    // Keep the complete filename below common 255-byte filesystem limits,
    // including multibyte artist names and collision suffixes.
    while result.len() > 90 {
        result.pop();
    }
    result = result.trim().trim_end_matches(['.', ' ']).to_string();
    if result.is_empty() {
        result = "Unknown".into();
    }
    let first = result.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(
        first.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        result.insert(0, '_');
    }
    result
}
fn file_stem(track: &Track) -> String {
    let title = match track.version.as_deref().filter(|v| !v.trim().is_empty()) {
        Some(v) if !track.title.to_lowercase().contains(&v.to_lowercase()) => {
            format!("{} ({})", track.title, v)
        }
        _ => track.title.clone(),
    };
    format!(
        "{} - {}",
        safe_component(&track.artist),
        safe_component(&title)
    )
}
fn staging(settings: &Settings, id: &str) -> Result<PathBuf, String> {
    if settings.music_directory.trim().is_empty() {
        return Err("Choose a music directory in Settings first".into());
    }
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Invalid staging job identifier".into());
    }
    let directory = Path::new(&settings.music_directory)
        .join(".burrata-staging")
        .join(id);
    fs::create_dir_all(&directory).map_err(|e| format!("Cannot create staging directory: {e}"))?;
    Ok(directory)
}
fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || octets[0] == 0
                || octets[0] >= 240
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 198 && matches!(octets[1], 18 | 19))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || ip.is_documentation())
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_address(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            !(ip.is_loopback()
                || (segments[0] & 0xe000) != 0x2000
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0xdb8))
        }
    }
}
pub(crate) async fn artwork(track: &Track) -> Result<Option<Vec<u8>>, String> {
    let Some(url) = track.artwork_url.as_deref() else {
        return Ok(None);
    };
    let mut parsed = url::Url::parse(url).map_err(|_| "Invalid artwork URL")?;
    let mut response = None;
    for _ in 0..=5 {
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("Artwork must use public HTTP or HTTPS without credentials".into());
        }
        let host = parsed
            .host_str()
            .ok_or("Artwork URL has no host")?
            .to_owned();
        let port = parsed
            .port_or_known_default()
            .ok_or("Artwork URL has no port")?;
        // Pin vetted DNS results per request, including redirected URLs. This
        // prevents artwork metadata from reaching local network services.
        let addresses: Vec<SocketAddr> = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| "Artwork DNS lookup timed out")?
        .map_err(|e| format!("Artwork DNS lookup failed: {e}"))?
        .collect();
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|address| !public_address(address.ip()))
        {
            return Err("Artwork host must resolve exclusively to public network addresses".into());
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(&host, &addresses)
            .build()
            .map_err(|e| e.to_string())?;
        let fetched = client
            .get(parsed.clone())
            .send()
            .await
            .map_err(|e| format!("Artwork download failed: {e}"))?;
        if fetched.status().is_redirection() {
            let location = fetched
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or("Artwork redirect has no location")?
                .to_str()
                .map_err(|_| "Invalid artwork redirect")?;
            parsed = parsed
                .join(location)
                .map_err(|_| "Invalid artwork redirect URL")?;
            continue;
        }
        response = Some(fetched.error_for_status().map_err(|e| e.to_string())?);
        break;
    }
    let mut response = response.ok_or("Artwork exceeded redirect limit")?;
    const MAX: usize = 8 * 1024 * 1024;
    if response.content_length().is_some_and(|n| n > MAX as u64) {
        return Err("Artwork exceeds 8 MB".into());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if data.len() + chunk.len() > MAX {
            return Err("Artwork exceeds 8 MB".into());
        }
        data.extend_from_slice(&chunk);
    }
    artwork_extension(&data)?;
    Ok(Some(data))
}

/// Artwork enriches a download but must not block publishing its audio or
/// selected text tags. Keep a valid cover already embedded in the staged
/// source when the optional remote lookup is absent or fails.
fn optional_artwork_or_embedded(
    fetched: Result<Option<Vec<u8>>, String>,
    staged_audio: &Path,
) -> Option<Vec<u8>> {
    fn valid(bytes: Vec<u8>) -> Option<Vec<u8>> {
        const MAX_EMBEDDED_ARTWORK: usize = 8 * 1024 * 1024;
        (bytes.len() <= MAX_EMBEDDED_ARTWORK && artwork_extension(&bytes).is_ok()).then_some(bytes)
    }
    fetched
        .ok()
        .flatten()
        .and_then(valid)
        .or_else(|| embedded_artwork(staged_audio).and_then(valid))
}

pub fn artwork_extension(data: &[u8]) -> Result<&'static str, String> {
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok("jpg")
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("png")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Ok("webp")
    } else {
        Err("Artwork is not a supported JPEG, PNG or WebP image".into())
    }
}
fn tag_media(path: &Path, track: &Track, cover: Option<Vec<u8>>) -> Result<(), String> {
    if track
        .bpm
        .is_some_and(|bpm| !bpm.is_finite() || bpm <= 0.0 || bpm > 1000.0)
    {
        return Err("BPM must be a finite value between 0 and 1000".into());
    }
    let mut tagged = Probe::open(path)
        .map_err(|e| e.to_string())?
        .guess_file_type()
        .map_err(|e| e.to_string())?
        .read()
        .map_err(|e| format!("Cannot read audio tags: {e}"))?;
    let tag_type = match tagged.file_type() {
        // RIFF INFO and AIFF text chunks cannot carry the full DJ metadata and
        // artwork; these containers support embedded ID3v2 instead.
        FileType::Wav | FileType::Aiff => TagType::Id3v2,
        _ => tagged.primary_tag_type(),
    };
    // Remove stale upload metadata; apply the selected track's fields. Recording
    // identity and mix review belong to the source-selection flow, not the writer.
    let types: Vec<_> = tagged.tags().iter().map(|t| t.tag_type()).collect();
    for kind in types {
        if kind != tag_type {
            kind.remove_from_path(path)
                .map_err(|e| format!("Cannot remove stale secondary tags: {e}"))?;
        }
        tagged.remove(kind);
    }
    let had_cover = cover.is_some();
    let mut tag = Tag::new(tag_type);
    if !track.artist.is_empty() {
        tag.set_artist(track.artist.clone());
    }
    let title = tag_title(track);
    if !title.is_empty() {
        tag.set_title(title.clone());
    }
    if let Some(album) = &track.album {
        tag.set_album(album.clone());
    }
    if let Some(genre) = &track.genre {
        tag.set_genre(genre.clone());
    }
    // Keep live-source evidence in the exported file as well as Burrata's DB.
    // This is an inferred performance identity, not a verified catalog match.
    if track.provenance["titleInference"]["type"] == "livePerformance" {
        let mut details = vec![
            "Live performance; inferred from source title, catalog identity unverified.".to_owned(),
        ];
        if let Some(title) = track.provenance["sourceTitle"].as_str() {
            details.push(format!("Source title: {title}"));
        }
        if let Some(url) = track.provenance["sourceURL"].as_str() {
            details.push(format!("Source URL: {url}"));
        }
        tag.insert_text(ItemKey::Comment, details.join("\n"));
    }
    for (key, value) in [
        (ItemKey::Label, track.label.clone()),
        (ItemKey::Bpm, track.bpm.map(|n| n.to_string())),
        (
            ItemKey::IntegerBpm,
            track.bpm.map(|n| n.round().to_string()),
        ),
        (ItemKey::InitialKey, track.key.clone()),
        (ItemKey::Isrc, track.isrc.clone()),
        (ItemKey::CatalogNumber, track.catalog_number.clone()),
    ] {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            tag.insert_text(key, value);
        }
    }
    if tag_type == TagType::Id3v2 {
        if let Some(bpm) = track.bpm {
            // ID3v2's standard TBPM frame only stores integers. Keep that
            // interoperable rounded value above and preserve the exact BPM in
            // a user-text TXXX:BPM frame for readers that support decimals.
            tag.push_unchecked(TagItem::new(
                ItemKey::Unknown("BPM".into()),
                ItemValue::Text(bpm.to_string()),
            ));
        }
    }
    if let Some(release_date) = track.release_date.as_deref() {
        if !release_date.trim().is_empty() {
            tag.insert_text(ItemKey::ReleaseDate, release_date.to_string());
        }
    }
    if let Some(recording_date) = track.recording_date.as_deref() {
        if !recording_date.trim().is_empty() {
            tag.insert_text(ItemKey::RecordingDate, recording_date.to_string());
            append_date_kind_marker(&mut tag, "recording");
        }
    }
    if let Some(bytes) = cover {
        let mut picture = if artwork_extension(&bytes)? == "webp" {
            Picture::new_unchecked(
                PictureType::CoverFront,
                Some(MimeType::Unknown("image/webp".into())),
                Some(String::new()),
                bytes,
            )
        } else {
            let mut reader = std::io::Cursor::new(bytes);
            Picture::from_reader(&mut reader).map_err(|e| format!("Invalid artwork: {e}"))?
        };
        picture.set_pic_type(PictureType::CoverFront);
        // Give APIC an explicit empty description so the image frame is
        // encoded consistently and can be reopened by Lofty.
        picture.set_description(Some(String::new()));
        tag.push_picture(picture);
    }
    tagged.insert_tag(tag);
    tagged
        // ID3v2.4 keeps full ISO recording dates in TDRC. Lofty's public
        // high-level Tag API cannot represent the decomposed TYER/TDAT/TIME
        // frames required to preserve full dates in ID3v2.3.
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("Cannot write audio tags: {e}"))?;
    let verified = Probe::open(path)
        .map_err(|e| e.to_string())?
        .guess_file_type()
        .map_err(|e| e.to_string())?
        .read()
        .map_err(|e| format!("Cannot reopen tagged media: {e}"))?;
    let tag = verified
        .tag(tag_type)
        .ok_or("Written tags could not be reopened")?;
    if !track.artist.is_empty() && tag.artist().as_deref() != Some(track.artist.as_str()) {
        return Err("Artist tag did not round-trip".into());
    }
    if !title.is_empty() && tag.title().as_deref() != Some(title.as_str()) {
        return Err("Title tag did not round-trip".into());
    }
    if let Some(album) = &track.album {
        if tag.album().as_deref() != Some(album.as_str()) {
            return Err("Album tag did not round-trip".into());
        }
    }
    if let Some(bpm) = track.bpm {
        let persisted_bpm = tag
            .get_string(&ItemKey::Bpm)
            .or_else(|| tag.get_string(&ItemKey::Unknown("BPM".into())))
            .or_else(|| tag.get_string(&ItemKey::IntegerBpm))
            .and_then(|value| value.parse::<f64>().ok());
        if persisted_bpm.is_none_or(|value| (value - bpm).abs() > 0.000_001) {
            return Err(format!(
                "BPM tag did not round-trip at its stored precision for {tag_type:?}: expected {bpm}, got {persisted_bpm:?}"
            ));
        }
    }
    if let Some(genre) = &track.genre {
        if tag.genre().as_deref() != Some(genre.as_str()) {
            return Err("Genre tag did not round-trip".into());
        }
    }
    if had_cover && tag.pictures().is_empty() {
        return Err("Artwork did not round-trip".into());
    }
    Ok(())
}

/// Merge only catalog identity fields into an existing managed audio file.
/// Existing tags, custom DJ frames, comments, and artwork remain intact.
/// Callers must pass a track containing only fields that have passed the
/// autotag confidence and provenance checks.
fn merge_autotag_tags(path: &Path, track: &Track) -> Result<(), String> {
    let mut tagged = Probe::open(path)
        .map_err(|e| e.to_string())?
        .guess_file_type()
        .map_err(|e| e.to_string())?
        .read()
        .map_err(|e| format!("Cannot read audio tags: {e}"))?;
    let tag_type = match tagged.file_type() {
        FileType::Wav | FileType::Aiff => TagType::Id3v2,
        _ => tagged.primary_tag_type(),
    };
    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .tag_mut(tag_type)
        .ok_or("Cannot open primary audio tag")?;
    if !track.artist.trim().is_empty() {
        tag.set_artist(track.artist.clone());
    }
    if !track.title.trim().is_empty() {
        tag.set_title(tag_title(track));
    }
    if let Some(value) = &track.album {
        tag.set_album(value.clone());
    }
    if let Some(value) = &track.genre {
        tag.set_genre(value.clone());
    }
    if let Some(value) = &track.label {
        tag.insert_text(ItemKey::Label, value.clone());
    }
    if let Some(value) = &track.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, value.clone());
    }
    if let Some(value) = &track.isrc {
        tag.insert_text(ItemKey::Isrc, value.clone());
    }
    if let Some(value) = &track.catalog_number {
        tag.insert_text(ItemKey::CatalogNumber, value.clone());
    }
    if let Some(value) = &track.release_date {
        tag.insert_text(ItemKey::ReleaseDate, value.clone());
    }
    if let Some(value) = &track.recording_date {
        tag.insert_text(ItemKey::RecordingDate, value.clone());
        append_date_kind_marker(tag, "recording");
    } else if track.release_date.is_some() && tag.get_string(&ItemKey::RecordingDate).is_some() {
        // Old Burrata versions wrote releaseDate to generic DATE/RecordingDate.
        // Preserve that raw frame without reinterpreting it as performance date.
        append_date_kind_marker(tag, "legacy");
    }
    if let Some(value) = &track.track_number {
        tag.insert_text(ItemKey::TrackNumber, value.to_string());
    }
    if let Some(value) = &track.track_total {
        tag.insert_text(ItemKey::TrackTotal, value.to_string());
    }
    if let Some(value) = &track.disc_number {
        tag.insert_text(ItemKey::DiscNumber, value.to_string());
    }
    if let Some(value) = &track.disc_total {
        tag.insert_text(ItemKey::DiscTotal, value.to_string());
    }
    if let Some(value) = &track.remixers {
        if !value.is_empty() {
            tag.insert_text(ItemKey::Remixer, value.join(", "));
        }
    }
    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("Cannot merge catalog tags: {e}"))
}

fn autotag_field_approved(metadata: &Track, key: &str) -> bool {
    let contains = |name: &str| {
        metadata.provenance[name]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field.as_str() == Some(key)))
    };
    contains("userApprovedFields")
        || (metadata.provenance["source"] == "exact_source_reported_metadata"
            && metadata.provenance["identityStatus"] == "source_reported_unverified"
            && ["artist", "title", "album", "albumArtist", "genre", "isrc"].contains(&key)
            && contains("sourceReportedFields"))
}

fn source_reported_metadata_provenance(
    asset_track: &Track,
    metadata: &Track,
) -> Option<serde_json::Value> {
    const FIELDS: &[&str] = &[
        "artist",
        "title",
        "album",
        "albumArtist",
        "version",
        "genre",
        "isrc",
    ];
    let provenance = &metadata.provenance;
    let reported = provenance.get("sourceReportedMetadata")?;
    let provider = reported.get("provider")?.as_str()?;
    let provider_id = reported.get("providerId")?.as_str()?;
    let source_url = reported.get("sourceUrl")?.as_str()?;
    let origin = reported.get("origin")?.as_str()?;
    let fields = reported.get("fields")?.as_array()?;
    let repaired_fields = provenance.get("sourceReportedFields")?.as_array()?;
    let valid_fields = |values: &[serde_json::Value]| {
        !values.is_empty()
            && values.len() <= FIELDS.len()
            && values
                .iter()
                .all(|value| value.as_str().is_some_and(|field| FIELDS.contains(&field)))
    };
    if provenance.get("source").and_then(serde_json::Value::as_str)
        != Some("exact_source_reported_metadata")
        || provenance
            .get("identityStatus")
            .and_then(serde_json::Value::as_str)
            != Some("source_reported_unverified")
        || reported
            .get("identityStatus")
            .and_then(serde_json::Value::as_str)
            != Some("source_reported_unverified")
        || provider.is_empty()
        || provider_id.is_empty()
        || source_url.is_empty()
        || !origin.starts_with("ytDlp")
        || asset_track.id != format!("{provider}:{provider_id}")
        || asset_track.provider != provider
        || asset_track.provider_id != provider_id
        || asset_track.source_url != source_url
        || !valid_fields(fields)
        || !valid_fields(repaired_fields)
        || repaired_fields.iter().any(|field| !fields.contains(field))
    {
        return None;
    }
    Some(reported.clone())
}

/// Apply vetted catalog identity fields to a registered managed asset. The
/// returned asset has its updated content hash; callers must persist it through
/// Store before considering the job complete.
pub async fn autotag_asset(
    asset: &LocalAsset,
    metadata: &Track,
    settings: &Settings,
) -> Result<LocalAsset, String> {
    let _retag_guard = lock_asset_retag(&asset.id).await;
    if asset.processing == "external-readonly" {
        return Err(
            "External audio is indexed read-only; autotagging requires a managed download".into(),
        );
    }
    let target = PathBuf::from(&asset.path);
    if !target.is_absolute()
        || fs::symlink_metadata(&target)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("Autotagging requires an existing, non-symlink audio file".into());
    }
    let library = fs::canonicalize(&settings.music_directory).map_err(|e| e.to_string())?;
    let canonical = fs::canonicalize(&target).map_err(|e| e.to_string())?;
    if canonical != target || !canonical.starts_with(&library) {
        return Err(
            "Autotagging requires an audio file inside the configured music directory".into(),
        );
    }
    if hash_file(&target)? != asset.hash {
        return Err("Audio changed after autotag job was queued".into());
    }
    let extension = target
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| !v.is_empty())
        .ok_or("Audio file needs an extension")?;
    let temporary = target
        .parent()
        .ok_or("Audio file has no parent directory")?
        .join(format!(
            ".burrata-autotag-{}.{}",
            uuid::Uuid::new_v4(),
            extension
        ));
    tokio::fs::copy(&target, &temporary)
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        if hash_file(&temporary)? != asset.hash || hash_file(&target)? != asset.hash {
            return Err("Audio changed while autotagging was starting".to_string());
        }
        let mut merged = asset.track.clone();
        if merged.artist.trim().is_empty()
            && asset.track.provenance["manualFields"]["artist"] != true
        {
            merged.artist = metadata.artist.clone();
        }
        if merged.title.trim().is_empty() && asset.track.provenance["manualFields"]["title"] != true
        {
            merged.title = metadata.title.clone();
        }
        // Source identity is retained unless its field was marked weak during
        // acquisition. Missing values are always safe to fill.
        for field in ["artist", "title"] {
            if asset.track.provenance["manualFields"][field] == true
                || (asset.track.provenance["weakFields"][field] != true
                    && !autotag_field_approved(metadata, field))
            {
                continue;
            }
            match field {
                "artist" if !metadata.artist.trim().is_empty() => {
                    merged.artist = metadata.artist.clone()
                }
                "title" if !metadata.title.trim().is_empty() => {
                    merged.title = metadata.title.clone()
                }
                _ => {}
            }
        }
        if (merged.album.is_none() || autotag_field_approved(metadata, "album"))
            && asset.track.provenance["manualFields"]["album"] != true
        {
            merged.album = metadata.album.clone();
        }
        if (merged.release_date.is_none() || autotag_field_approved(metadata, "releaseDate"))
            && asset.track.provenance["manualFields"]["releaseDate"] != true
        {
            merged.release_date = metadata.release_date.clone();
        }
        if (merged.genre.is_none() || autotag_field_approved(metadata, "genre"))
            && asset.track.provenance["manualFields"]["genre"] != true
        {
            merged.genre = metadata.genre.clone();
        }
        if (merged.album_artist.is_none() || autotag_field_approved(metadata, "albumArtist"))
            && asset.track.provenance["manualFields"]["albumArtist"] != true
        {
            merged.album_artist = metadata.album_artist.clone();
        }
        if (merged.isrc.is_none() || autotag_field_approved(metadata, "isrc"))
            && asset.track.provenance["manualFields"]["isrc"] != true
        {
            merged.isrc = metadata.isrc.clone();
        }
        if (merged.label.is_none() || autotag_field_approved(metadata, "label"))
            && asset.track.provenance["manualFields"]["label"] != true
        {
            merged.label = metadata.label.clone();
        }
        if (merged.catalog_number.is_none() || autotag_field_approved(metadata, "catalogNumber"))
            && asset.track.provenance["manualFields"]["catalogNumber"] != true
        {
            merged.catalog_number = metadata.catalog_number.clone();
        }
        if (merged.track_number.is_none() || autotag_field_approved(metadata, "trackNumber"))
            && asset.track.provenance["manualFields"]["trackNumber"] != true
        {
            merged.track_number = metadata.track_number;
        }
        if (merged.track_total.is_none() || autotag_field_approved(metadata, "trackTotal"))
            && asset.track.provenance["manualFields"]["trackTotal"] != true
        {
            merged.track_total = metadata.track_total;
        }
        if (merged.disc_number.is_none() || autotag_field_approved(metadata, "discNumber"))
            && asset.track.provenance["manualFields"]["discNumber"] != true
        {
            merged.disc_number = metadata.disc_number;
        }
        if (merged.disc_total.is_none() || autotag_field_approved(metadata, "discTotal"))
            && asset.track.provenance["manualFields"]["discTotal"] != true
        {
            merged.disc_total = metadata.disc_total;
        }
        let stage = temporary.clone();
        tokio::task::spawn_blocking(move || merge_autotag_tags(&stage, &merged))
            .await
            .map_err(|e| e.to_string())??;
        let (before, after) =
            tokio::try_join!(probe(&target, settings), probe(&temporary, settings))?;
        if before.codec != after.codec || (before.duration - after.duration).abs() > 0.05 {
            return Err("Autotagging unexpectedly changed the audio codec or duration".into());
        }
        if hash_file(&target)? != asset.hash {
            return Err("Audio changed during autotagging; replacement refused".to_string());
        }
        let mut updated = asset.clone();
        updated.hash = hash_file(&temporary)?;
        // Store the catalog provenance without replacing download lineage.
        updated.track = asset.track.clone();
        if !updated.track.provenance.is_object() {
            updated.track.provenance = serde_json::json!({});
        }
        updated.track.provenance["autotag"] = metadata.provenance.clone();
        if let Some(reported) = source_reported_metadata_provenance(&asset.track, metadata) {
            updated.track.provenance["sourceReportedMetadata"] = reported;
        }
        if (updated.track.album.is_none() || autotag_field_approved(metadata, "album"))
            && asset.track.provenance["manualFields"]["album"] != true
        {
            updated.track.album = metadata.album.clone();
        }
        if (updated.track.album_artist.is_none() || autotag_field_approved(metadata, "albumArtist"))
            && asset.track.provenance["manualFields"]["albumArtist"] != true
        {
            updated.track.album_artist = metadata.album_artist.clone();
        }
        if (updated.track.isrc.is_none() || autotag_field_approved(metadata, "isrc"))
            && asset.track.provenance["manualFields"]["isrc"] != true
        {
            updated.track.isrc = metadata.isrc.clone();
        }
        if (updated.track.label.is_none() || autotag_field_approved(metadata, "label"))
            && asset.track.provenance["manualFields"]["label"] != true
        {
            updated.track.label = metadata.label.clone();
        }
        if (updated.track.catalog_number.is_none()
            || autotag_field_approved(metadata, "catalogNumber"))
            && asset.track.provenance["manualFields"]["catalogNumber"] != true
        {
            updated.track.catalog_number = metadata.catalog_number.clone();
        }
        if (updated.track.track_number.is_none() || autotag_field_approved(metadata, "trackNumber"))
            && asset.track.provenance["manualFields"]["trackNumber"] != true
        {
            updated.track.track_number = metadata.track_number;
        }
        if (updated.track.track_total.is_none() || autotag_field_approved(metadata, "trackTotal"))
            && asset.track.provenance["manualFields"]["trackTotal"] != true
        {
            updated.track.track_total = metadata.track_total;
        }
        if (updated.track.disc_number.is_none() || autotag_field_approved(metadata, "discNumber"))
            && asset.track.provenance["manualFields"]["discNumber"] != true
        {
            updated.track.disc_number = metadata.disc_number;
        }
        if (updated.track.disc_total.is_none() || autotag_field_approved(metadata, "discTotal"))
            && asset.track.provenance["manualFields"]["discTotal"] != true
        {
            updated.track.disc_total = metadata.disc_total;
        }
        if (updated.track.release_date.is_none() || autotag_field_approved(metadata, "releaseDate"))
            && asset.track.provenance["manualFields"]["releaseDate"] != true
        {
            updated.track.release_date = metadata.release_date.clone();
        }
        if (updated.track.genre.is_none() || autotag_field_approved(metadata, "genre"))
            && asset.track.provenance["manualFields"]["genre"] != true
        {
            updated.track.genre = metadata.genre.clone();
        }
        if (updated.track.artist.trim().is_empty()
            || autotag_field_approved(metadata, "artist")
            || asset.track.provenance["weakFields"]["artist"] == true)
            && asset.track.provenance["manualFields"]["artist"] != true
            && !metadata.artist.is_empty()
        {
            updated.track.artist = metadata.artist.clone();
        }
        if (updated.track.title.trim().is_empty()
            || autotag_field_approved(metadata, "title")
            || asset.track.provenance["weakFields"]["title"] == true)
            && asset.track.provenance["manualFields"]["title"] != true
            && !metadata.title.is_empty()
        {
            updated.track.title = metadata.title.clone();
        }
        if !updated.track.provenance.is_object() {
            updated.track.provenance = serde_json::json!({});
        }
        updated.track.provenance["retagKind"] = serde_json::Value::String("autotag".into());
        let id = uuid::Uuid::new_v4().to_string();
        updated.track.provenance["retagJournalId"] = serde_json::Value::String(id.clone());
        let journal_directory = Path::new(&settings.music_directory)
            .join(".burrata-staging")
            .join("analysis");
        write_retag_journal(
            &journal_directory,
            &RetagJournal {
                id,
                previous_hash: asset.hash.clone(),
                temporary_path: temporary.to_string_lossy().into_owned(),
                asset: updated.clone(),
            },
        )?;
        fs::rename(&temporary, &target)
            .map_err(|e| format!("Cannot publish autotagged audio: {e}"))?;
        Ok(updated)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn flush_staged_audio(path: &Path) -> std::io::Result<()> {
    // Windows requires a write-capable handle for FlushFileBuffers, which is
    // what File::sync_all uses. Staged audio is owned by this process, so open
    // it read/write before linking it into the library.
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}
#[cfg(test)]
fn publish(
    path: &Path,
    directory: &Path,
    stem: &str,
    extension: &str,
    hash: &str,
) -> Result<PathBuf, String> {
    flush_staged_audio(path).map_err(|e| format!("Cannot flush staged audio: {e}"))?;
    for index in 0..1000 {
        let suffix = if index == 0 {
            String::new()
        } else if index == 1 {
            format!(" [{}]", &hash[..12])
        } else {
            format!(" [{}-{index}]", &hash[..12])
        };
        let target = directory.join(format!("{stem}{suffix}.{extension}"));
        match fs::hard_link(path, &target) {
            Ok(()) => {
                let _ = fs::remove_file(path);
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Cannot atomically publish audio: {e}")),
        }
    }
    Err("Too many filename collisions".into())
}
#[derive(serde::Serialize, serde::Deserialize)]
struct PublicationJournal {
    job_id: String,
    asset: LocalAsset,
    candidate: Option<Candidate>,
}

// Persist the intent before linking the final file. Recovery checks its checksum,
// so a crash between filesystem publication and the SQLite commit is recoverable.
pub(crate) fn journaled_publish(
    path: &Path,
    settings: &Settings,
    job_id: &str,
    stem: &str,
    extension: &str,
    asset: &mut LocalAsset,
    candidate: Option<&Candidate>,
) -> Result<(), String> {
    let stage = staging(settings, job_id)?;
    let library = fs::canonicalize(&settings.music_directory).map_err(|e| e.to_string())?;
    flush_staged_audio(path).map_err(|e| format!("Cannot flush staged audio: {e}"))?;
    for index in 0..1000 {
        let suffix = if index == 0 {
            String::new()
        } else if index == 1 {
            format!(" [{}]", &asset.hash[..12])
        } else {
            format!(" [{}-{index}]", &asset.hash[..12])
        };
        let target = library.join(format!("{stem}{suffix}.{extension}"));
        if target.exists() {
            continue;
        }
        asset.path = target.to_string_lossy().into();
        asset.track.local_path = Some(asset.path.clone());
        if !asset.track.provenance.is_object() {
            asset.track.provenance = serde_json::json!({});
        }
        asset.track.provenance["publicationJobId"] = serde_json::json!(job_id);
        let journal = PublicationJournal {
            job_id: job_id.into(),
            asset: asset.clone(),
            candidate: candidate.cloned(),
        };
        let temporary = stage.join("publication.json.tmp");
        let journal_path = stage.join("publication.json");
        let bytes = serde_json::to_vec(&journal).map_err(|e| e.to_string())?;
        let mut file = File::create(&temporary)
            .map_err(|e| format!("Cannot create publication journal: {e}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Cannot flush publication journal: {e}"))?;
        drop(file);
        // The final journal is immutable while publication is in progress. A
        // collision discards this uncommitted intent before choosing a new name.
        if journal_path.exists() {
            return Err("Unacknowledged publication exists; recover it before retrying".into());
        }
        fs::rename(&temporary, &journal_path)
            .map_err(|e| format!("Cannot publish recovery journal: {e}"))?;
        #[cfg(unix)]
        File::open(&stage)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| format!("Cannot flush recovery directory: {e}"))?;
        match fs::hard_link(path, &target) {
            Ok(()) => {
                #[cfg(unix)]
                File::open(&library)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|e| {
                        format!(
                            "Audio was published but directory flush failed; recovery required: {e}"
                        )
                    })?;
                let _ = fs::remove_file(path);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(&journal_path).map_err(|e| e.to_string())?;
            }
            Err(e) => {
                let _ = fs::remove_file(&journal_path);
                return Err(format!("Cannot atomically publish audio: {e}"));
            }
        }
    }
    Err("Too many filename collisions".into())
}

/// Reconcile these assets into SQLite and acknowledge each journal only after
/// the asset and corresponding download job have been committed successfully.
pub fn recover_publications(settings: &Settings) -> Result<Vec<(String, LocalAsset)>, String> {
    if settings.music_directory.trim().is_empty() {
        return Ok(Vec::new());
    }
    let library = match fs::canonicalize(&settings.music_directory) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let stage = library.join(".burrata-staging");
    if !stage.exists() {
        return Ok(Vec::new());
    }
    let mut recovered = Vec::new();
    for entry in fs::read_dir(stage).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let path = entry.path().join("publication.json");
        if !path.exists() {
            continue;
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("Publication journal exceeds allowed size".into());
        }
        let journal: PublicationJournal = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid publication journal {}: {e}", path.display()))?;
        if entry.file_name().to_string_lossy() != journal.job_id
            || !journal
                .job_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("Publication journal has invalid job identity".into());
        }
        let target = Path::new(&journal.asset.path);
        if !target.exists() {
            // The crash happened before the atomic link. There is no asset to
            // recover; clear the intent so an explicit retry can publish again.
            fs::remove_file(&path).map_err(|e| e.to_string())?;
            continue;
        }
        // Canonicalization prevents journal paths escaping the music directory,
        // including symlink replacements of a previously published file.
        let canonical = fs::canonicalize(target).map_err(|e| e.to_string())?;
        if canonical.parent() != Some(library.as_path()) {
            return Err("Publication journal points outside the music directory".into());
        }
        if hash_file(&canonical)? != journal.asset.hash {
            return Err(format!(
                "Published file changed; recovery requires review: {}",
                canonical.display()
            ));
        }
        recovered.push((journal.job_id, journal.asset));
    }
    Ok(recovered)
}

pub fn acknowledge_publication(settings: &Settings, job_id: &str) -> Result<(), String> {
    if job_id.is_empty()
        || !job_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("Invalid publication job identifier".into());
    }
    let directory = Path::new(&settings.music_directory)
        .join(".burrata-staging")
        .join(job_id);
    match fs::remove_file(directory.join("publication.json")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Cannot acknowledge publication: {e}")),
    }
    #[cfg(unix)]
    if directory.exists() {
        File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| e.to_string())?;
    }
    let _ = fs::remove_dir(&directory);
    Ok(())
}

async fn transcode(
    input: &Path,
    output: &Path,
    settings: &Settings,
    mp3: bool,
) -> Result<(), String> {
    let mut command = Command::new(crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"));
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .kill_on_drop(true)
        .args(["-nostdin", "-v", "error", "-n", "-i"])
        .arg(input)
        .args(["-map", "0:a:0", "-vn", "-map_metadata", "-1"]);
    if mp3 {
        command.args(["-c:a", "libmp3lame", "-b:a", "320k"]);
    } else {
        command.args(["-c:a", "copy"]);
    }
    let output = tokio::time::timeout(Duration::from_secs(600), command.arg(output).output())
        .await
        .map_err(|_| "Audio processing timed out".to_string())?
        .map_err(|e| format!("Cannot run FFmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Audio processing failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        ));
    }
    Ok(())
}

pub async fn finalize_download(
    path: &Path,
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    profile: &str,
    job_id: &str,
) -> Result<LocalAsset, String> {
    finalize_download_inner(path, track, candidate, settings, profile, job_id, None).await
}

pub(crate) async fn finalize_download_with_diagnostic(
    path: &Path,
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    profile: &str,
    job_id: &str,
    failure_stage: &mut &'static str,
) -> Result<LocalAsset, String> {
    finalize_download_inner(
        path,
        track,
        candidate,
        settings,
        profile,
        job_id,
        Some(failure_stage),
    )
    .await
}

fn set_finalize_diagnostic_stage(
    failure_stage: &mut Option<&mut &'static str>,
    stage: &'static str,
) {
    if let Some(failure_stage) = failure_stage.as_mut() {
        **failure_stage = stage;
    }
}

async fn finalize_download_inner(
    path: &Path,
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    profile: &str,
    job_id: &str,
    mut failure_stage: Option<&mut &'static str>,
) -> Result<LocalAsset, String> {
    set_finalize_diagnostic_stage(&mut failure_stage, "finalize_validate_profile");
    if !matches!(profile, "source" | "mp3") {
        return Err("Unknown output profile".into());
    }
    set_finalize_diagnostic_stage(&mut failure_stage, "finalize_probe_source");
    let original = probe(path, settings).await?;
    set_finalize_diagnostic_stage(&mut failure_stage, "finalize_duration_gate");
    if let Some(expected) = candidate.duration_seconds {
        if expected.is_finite()
            && expected > 0.0
            && (original.duration - expected).abs() > (expected * 0.03).max(10.0)
        {
            return Err(
                "Downloaded duration differs from the inspected source; review before publishing"
                    .into(),
            );
        }
    }
    let mut source_track = track.clone();
    if source_track.artwork_url.is_none() {
        source_track.artwork_url = candidate.artwork_url.clone();
        if source_track.artwork_url.is_some() {
            source_track.provenance["artworkSource"] = serde_json::json!("audio source thumbnail");
        }
    }
    set_finalize_diagnostic_stage(&mut failure_stage, "finalize_analysis");
    let tagged_track = analyze_missing_fields(path, &source_track, settings, false).await;
    set_finalize_diagnostic_stage(&mut failure_stage, "finalize_staging_directory");
    let stage = staging(settings, job_id)?;
    set_finalize_diagnostic_stage(&mut failure_stage, "finalize_output_extension");
    let extension = published_audio_extension(&original.codec, path)?;
    let staged = stage.join(format!("tagged-{}.{}", uuid::Uuid::new_v4(), extension));
    let remux = path
        .extension()
        .and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case(extension));
    let result: Result<LocalAsset, String> = async {
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_remux");
        if remux {
            transcode(path, &staged, settings, false).await?;
        } else {
            tokio::fs::copy(path, &staged)
                .await
                .map_err(|e| format!("Cannot stage audio: {e}"))?;
        }
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_artwork");
        let cover = optional_artwork_or_embedded(artwork(&source_track).await, &staged);
        let p = staged.clone();
        let t = tagged_track.clone();
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_tag");
        tokio::task::spawn_blocking(move || tag_media(&p, &t, cover))
            .await
            .map_err(|e| e.to_string())??;
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_probe_output");
        let final_info = probe(&staged, settings).await?;
        if final_info.codec != original.codec
            || (final_info.duration - original.duration).abs() > 1.0
        {
            return Err("Tagging or remux unexpectedly changed audio".into());
        }
        let p = staged.clone();
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_hash");
        let hash = tokio::task::spawn_blocking(move || hash_file(&p))
            .await
            .map_err(|e| e.to_string())??;
        let mut tagged_track = tagged_track.clone();
        if let Some(provenance) = tagged_track.provenance.as_object_mut() {
            provenance.remove("derivedFromAssetId");
        }
        tagged_track.duration_seconds = Some(final_info.duration);
        let mut asset = LocalAsset {
            id: uuid::Uuid::new_v4().to_string(),
            recording_id: track.id.clone(),
            path: String::new(),
            hash,
            source_codec: original.codec,
            final_codec: final_info.codec,
            source_bitrate: original.bitrate,
            final_bitrate: final_info.bitrate,
            sample_rate: final_info.sample_rate,
            channels: final_info.channels,
            bit_depth: final_info.bit_depth,
            duration_seconds: Some(final_info.duration),
            size_bytes: final_info.size_bytes,
            bitrate_origin: final_info.bitrate_origin.clone(),
            processing: if remux {
                "remuxed; original audio codec preserved".into()
            } else {
                "original audio codec preserved".into()
            },
            track: tagged_track,
        };
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_publish");
        journaled_publish(
            &staged,
            settings,
            job_id,
            &file_stem(track),
            extension,
            &mut asset,
            Some(candidate),
        )?;
        Ok(asset)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    let asset = result?;
    if profile == "mp3" {
        set_finalize_diagnostic_stage(&mut failure_stage, "finalize_mp3_export");
        export_mp3(&asset, settings).await
    } else {
        Ok(asset)
    }
}

/// Extract one selected chapter into a temporary audio file. The input is the
/// single acquired parent recording; this function never publishes the parent.
pub async fn split_chapter(
    path: &Path,
    chapter: &Chapter,
    settings: &Settings,
    job_id: &str,
    index: usize,
) -> Result<PathBuf, String> {
    if !chapter.start_seconds.is_finite()
        || !chapter.end_seconds.is_finite()
        || chapter.start_seconds < 0.0
        || chapter.end_seconds <= chapter.start_seconds
    {
        return Err("Invalid chapter time range".into());
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("m4a");
    let directory = staging(settings, job_id)?;
    fs::create_dir_all(&directory)
        .map_err(|e| format!("Cannot create chapter staging directory: {e}"))?;
    let target = directory.join(format!("chapter-{index}.{}", extension));
    let duration = chapter.end_seconds - chapter.start_seconds;
    let mut command = Command::new(crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"));
    crate::tools::configure_no_console_tokio(&mut command);
    let output = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .args(["-nostdin", "-v", "error", "-ss"])
        .arg(chapter.start_seconds.to_string())
        .arg("-i")
        .arg(path)
        .args(["-t"])
        .arg(duration.to_string())
        .args(["-map", "0:a:0", "-vn", "-c:a", "copy", "-y"])
        .arg(&target)
        .output()
        .await
        .map_err(|e| format!("Cannot split chapter: {e}"))?;
    if !output.status.success() || !target.is_file() {
        return Err(format!(
            "FFmpeg could not split chapter: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(target)
}

/// Verify an archived asset before reuse; externally changed files require review.
pub async fn verify_asset(asset: &LocalAsset) -> Result<(), String> {
    let path = PathBuf::from(&asset.path);
    let expected = asset.hash.clone();
    tokio::task::spawn_blocking(move || {
        let actual = hash_file(&path).map_err(|e| format!("Archived audio is unavailable: {e}"))?;
        if actual != expected {
            return Err(
                "Archived audio changed outside Burrata; review or reimport it before reuse".into(),
            );
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub const ANALYSIS_VERSION: &str = "burrata-media-analysis-v1-keyfinder-2.2.8-bpm-v3";

#[derive(serde::Serialize, serde::Deserialize)]
struct RetagJournal {
    id: String,
    previous_hash: String,
    temporary_path: String,
    asset: LocalAsset,
}

pub(crate) fn embedded_artwork(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    tagged
        .tags()
        .iter()
        .flat_map(|tag| tag.pictures())
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tagged.tags().iter().flat_map(|tag| tag.pictures()).next())
        .map(|picture| picture.data().to_vec())
}

/// Return cover bytes only for a registered asset whose stored file still
/// matches its recorded hash and is a regular, non-symlink file.
pub async fn embedded_artwork_for_asset(asset: &LocalAsset) -> Result<Option<Vec<u8>>, String> {
    let path = PathBuf::from(&asset.path);
    if !path.is_absolute()
        || fs::symlink_metadata(&path)
            .map_err(|e| format!("Cannot inspect registered audio path: {e}"))?
            .file_type()
            .is_symlink()
    {
        return Err("Registered audio path must be an absolute non-symlink file".into());
    }
    let canonical = fs::canonicalize(&path).map_err(|e| e.to_string())?;
    if canonical != path || !canonical.is_file() {
        return Err("Registered audio path is not a regular canonical file".into());
    }
    verify_asset(asset).await?;
    let Some(bytes) = embedded_artwork(&canonical) else {
        return Ok(None);
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Embedded artwork exceeds 8 MB".into());
    }
    artwork_extension(&bytes)?;
    Ok(Some(bytes))
}

fn write_retag_journal(directory: &Path, journal: &RetagJournal) -> Result<PathBuf, String> {
    fs::create_dir_all(directory).map_err(|e| format!("Cannot create analysis journal: {e}"))?;
    let temporary = directory.join(format!("{}.json.tmp", journal.id));
    let path = directory.join(format!("{}.json", journal.id));
    let bytes = serde_json::to_vec(journal).map_err(|e| e.to_string())?;
    let mut file =
        File::create(&temporary).map_err(|e| format!("Cannot create analysis journal: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Cannot flush analysis journal: {e}"))?;
    drop(file);
    fs::rename(&temporary, &path).map_err(|e| format!("Cannot publish analysis journal: {e}"))?;
    #[cfg(unix)]
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("Cannot flush analysis journal directory: {e}"))?;
    Ok(path)
}

/// Analyze missing tempo/key fields and atomically retag a registered asset in
/// place. The path remains unchanged. A committed journal lets startup finish
/// the operation if the process exits after file replacement but before SQLite
/// records the new content hash.
pub async fn analyze_and_retag_asset(
    asset: &LocalAsset,
    settings: &Settings,
) -> Result<LocalAsset, String> {
    analyze_and_retag_asset_with_force(asset, settings, false).await
}

pub async fn analyze_and_retag_asset_with_force(
    asset: &LocalAsset,
    settings: &Settings,
    force: bool,
) -> Result<LocalAsset, String> {
    let _retag_guard = lock_asset_retag(&asset.id).await;
    if asset.processing == "external-readonly" {
        return Err("External audio is indexed read-only; copy it into the managed library before retagging".into());
    }
    if asset.track.id != asset.recording_id {
        return Err("Asset recording identity does not match its selected metadata".into());
    }
    if settings.music_directory.trim().is_empty() {
        return Err("Choose a music directory before analyzing library audio".into());
    }
    let target = PathBuf::from(&asset.path);
    if !target.is_absolute()
        || fs::symlink_metadata(&target)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("Analysis requires an existing, non-symlink audio file".into());
    }
    let library = fs::canonicalize(&settings.music_directory).map_err(|e| e.to_string())?;
    let canonical_target = fs::canonicalize(&target).map_err(|e| e.to_string())?;
    if canonical_target != target || !canonical_target.starts_with(&library) {
        return Err("Analysis requires an audio file inside the configured music directory".into());
    }
    if hash_file(&target)? != asset.hash {
        return Err(
            "Archived audio changed outside Burrata; analysis and retagging were refused".into(),
        );
    }
    let parent = target
        .parent()
        .ok_or("Audio file has no parent directory")?;
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or("Audio file needs a supported extension to retag")?;
    let temporary = parent.join(format!(
        ".burrata-retag-{}.{}",
        uuid::Uuid::new_v4(),
        extension
    ));
    tokio::fs::copy(&target, &temporary)
        .await
        .map_err(|e| format!("Cannot stage audio for analysis: {e}"))?;
    let result = async {
        if hash_file(&temporary)? != asset.hash || hash_file(&target)? != asset.hash {
            return Err("Archived audio changed while analysis was starting".into());
        }
        let track = analyze_missing_fields(&target, &asset.track, settings, force).await;
        let cover = embedded_artwork(&target);
        let cover = match cover {
            Some(bytes) => Some(bytes),
            None => artwork(&track).await?,
        };
        let path_for_tagging = temporary.clone();
        let track_for_tagging = track.clone();
        tokio::task::spawn_blocking(move || {
            tag_media(&path_for_tagging, &track_for_tagging, cover)
        })
        .await
        .map_err(|e| e.to_string())??;

        let original_info = probe(&target, settings).await?;
        let tagged_info = probe(&temporary, settings).await?;
        if original_info.codec != tagged_info.codec
            || (original_info.duration - tagged_info.duration).abs() > 0.05
        {
            return Err("Retagging unexpectedly changed the audio codec or duration".into());
        }
        let new_hash = hash_file(&temporary)?;
        if new_hash == asset.hash {
            let mut updated = asset.clone();
            updated.track = track;
            if !updated.track.provenance.is_object() {
                updated.track.provenance = serde_json::json!({});
            }
            updated
                .track
                .provenance
                .as_object_mut()
                .expect("object was initialized")
                .remove("retagJournalId");
            updated.track.local_path = Some(asset.path.clone());
            return Ok(updated);
        }
        if hash_file(&target)? != asset.hash {
            return Err("Archived audio changed during analysis; replacement was refused".into());
        }
        let mut updated = asset.clone();
        updated.hash = new_hash;
        updated.track = track;
        updated.track.local_path = Some(asset.path.clone());
        updated.final_codec = tagged_info.codec;
        updated.final_bitrate = tagged_info.bitrate;

        let id = uuid::Uuid::new_v4().to_string();
        if !updated.track.provenance.is_object() {
            updated.track.provenance = serde_json::json!({});
        }
        updated.track.provenance["retagJournalId"] = serde_json::Value::String(id.clone());
        let journal_directory = Path::new(&settings.music_directory)
            .join(".burrata-staging")
            .join("analysis");
        let journal = RetagJournal {
            id,
            previous_hash: asset.hash.clone(),
            temporary_path: temporary.to_string_lossy().into_owned(),
            asset: updated.clone(),
        };
        write_retag_journal(&journal_directory, &journal)?;
        fs::rename(&temporary, &target)
            .map_err(|e| format!("Cannot atomically replace retagged audio: {e}"))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| {
                format!("Audio was retagged but directory flush failed; recovery required: {e}")
            })?;
        Ok(updated)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Complete durable retags after a crash. Returned journal ids must be
/// acknowledged only after the storage transaction records the new asset hash.
pub fn recover_retag_journals(settings: &Settings) -> Result<Vec<(String, LocalAsset)>, String> {
    if settings.music_directory.trim().is_empty() {
        return Ok(Vec::new());
    }
    let directory = Path::new(&settings.music_directory)
        .join(".burrata-staging")
        .join("analysis");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut recovered = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("Analysis journal exceeds allowed size".into());
        }
        let journal: RetagJournal = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid analysis journal {}: {e}", entry.path().display()))?;
        if !journal
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
            || entry.file_name().to_string_lossy() != format!("{}.json", journal.id)
            || journal.asset.path.is_empty()
            || !Path::new(&journal.asset.path).is_absolute()
            || journal.asset.track.local_path.as_deref() != Some(journal.asset.path.as_str())
        {
            return Err("Analysis journal has invalid asset identity".into());
        }
        let target = Path::new(&journal.asset.path);
        let temporary = Path::new(&journal.temporary_path);
        let temporary_name = temporary
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if temporary.parent() != target.parent()
            || temporary.extension() != target.extension()
            || !(temporary_name.starts_with(".burrata-retag-")
                || temporary_name.starts_with(".burrata-autotag-"))
            || journal.asset.track.id != journal.asset.recording_id
            || journal.asset.hash.len() != 64
            || !journal
                .asset
                .hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || journal.previous_hash.len() != 64
            || !journal
                .previous_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Analysis journal has an invalid temporary path".into());
        }
        let library = fs::canonicalize(&settings.music_directory).map_err(|e| e.to_string())?;
        let target_parent = target
            .parent()
            .ok_or("Analysis journal target has no parent")?;
        let canonical_parent = fs::canonicalize(target_parent).map_err(|e| e.to_string())?;
        if !canonical_parent.starts_with(&library)
            || (target.exists()
                && fs::symlink_metadata(target)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink())
        {
            return Err(
                "Analysis journal points outside the music directory or to a symlink".into(),
            );
        }
        let target_hash = if target.exists() {
            Some(hash_file(target)?)
        } else {
            None
        };
        if target.exists()
            && fs::symlink_metadata(target)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err("Analysis journal target became a symlink; review required".into());
        }
        if temporary.exists()
            && fs::symlink_metadata(temporary)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err("Analysis journal temporary file became a symlink; review required".into());
        }
        let temporary_hash = if temporary.exists() {
            Some(hash_file(temporary)?)
        } else {
            None
        };
        if target_hash.as_deref() == Some(journal.asset.hash.as_str()) {
            recovered.push((journal.id, journal.asset));
        } else if temporary_hash.as_deref() == Some(journal.asset.hash.as_str())
            && (target_hash.as_deref() == Some(journal.previous_hash.as_str())
                || target_hash.is_none())
        {
            fs::rename(temporary, target)
                .map_err(|e| format!("Cannot recover atomic retag: {e}"))?;
            recovered.push((journal.id, journal.asset));
        } else if target_hash.as_deref() == Some(journal.previous_hash.as_str())
            && temporary_hash.is_none()
        {
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        } else {
            return Err(format!(
                "Retag recovery found externally changed or missing audio; review required: {}",
                target.display()
            ));
        }
    }
    Ok(recovered)
}

pub fn acknowledge_retag_journal(settings: &Settings, journal_id: &str) -> Result<(), String> {
    if journal_id.is_empty()
        || !journal_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("Invalid analysis journal identifier".into());
    }
    let path = Path::new(&settings.music_directory)
        .join(".burrata-staging")
        .join("analysis")
        .join(format!("{journal_id}.json"));
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot acknowledge analysis journal: {error}")),
    }
}

fn export_processing(codec: &str, source: &str) -> String {
    if codec == "mp3" {
        format!(
            "MP3 compatibility copy; original encoding and bitrate preserved without re-encoding; source preserved at {source}"
        )
    } else if matches!(codec, "flac" | "alac") || codec.starts_with("pcm_") {
        format!(
            "MP3 320 kbps CBR derivative encoded from {codec} for compatibility; source preserved at {source}"
        )
    } else {
        format!(
            "Lossy {codec} source transcoded to MP3 320 kbps CBR for compatibility; audio quality is not upgraded; source preserved at {source}"
        )
    }
}

pub async fn export_mp3(asset: &LocalAsset, settings: &Settings) -> Result<LocalAsset, String> {
    verify_asset(asset).await?;
    let original = probe(Path::new(&asset.path), settings).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let stage = staging(settings, &id)?;
    let output = stage.join("export.mp3");
    let result: Result<LocalAsset, String> = async {
        if original.codec == "mp3" {
            tokio::fs::copy(&asset.path, &output)
                .await
                .map_err(|e| format!("Cannot stage MP3 compatibility copy: {e}"))?;
        } else {
            transcode(Path::new(&asset.path), &output, settings, true).await?;
        }
        let cover = artwork(&asset.track).await?;
        let p = output.clone();
        let t = asset.track.clone();
        tokio::task::spawn_blocking(move || tag_media(&p, &t, cover))
            .await
            .map_err(|e| e.to_string())??;
        let info = probe(&output, settings).await?;
        if info.codec != "mp3" {
            return Err("Compatibility export is not MP3".into());
        }
        if let Some(duration) = asset.track.duration_seconds {
            if (info.duration - duration).abs() > 1.0 {
                return Err("MP3 export unexpectedly changed audio duration".into());
            }
        }
        let p = output.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&p))
            .await
            .map_err(|e| e.to_string())??;
        let mut track = asset.track.clone();
        if !track.provenance.is_object() {
            track.provenance = serde_json::json!({});
        }
        track.provenance["derivedFromAssetId"] = serde_json::json!(asset.id);
        let mut exported = LocalAsset {
            id: id.clone(),
            recording_id: asset.recording_id.clone(),
            path: String::new(),
            hash,
            source_codec: original.codec.clone(),
            final_codec: info.codec,
            source_bitrate: original.bitrate,
            final_bitrate: info.bitrate,
            sample_rate: info.sample_rate,
            channels: info.channels,
            bit_depth: info.bit_depth,
            duration_seconds: Some(info.duration),
            size_bytes: info.size_bytes,
            bitrate_origin: info.bitrate_origin.clone(),
            processing: export_processing(&original.codec, &asset.path),
            track,
        };
        journaled_publish(
            &output,
            settings,
            &id,
            &format!("{} [MP3 export]", file_stem(&asset.track)),
            "mp3",
            &mut exported,
            None,
        )?;
        Ok(exported)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(output);
    }
    result
}

pub async fn inspect_local_file(path: &Path, settings: &Settings) -> Result<Track, String> {
    let info = probe(path, settings).await?;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = Probe::open(&path).map_err(|e| e.to_string())?.guess_file_type().map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
        let tag = file.primary_tag().or_else(|| file.first_tag());
        let text = |key: ItemKey| tag.and_then(|t| t.get_string(&key)).map(str::to_owned);
        let (release_date, recording_date) = local_date_fields(
            text(ItemKey::ReleaseDate),
            text(ItemKey::RecordingDate),
            date_kind_marker(text(ItemKey::Comment)).as_deref(),
        );
        Ok(Track { id: format!("local:{}", hash_file(&path)?), provider: "local".into(), provider_id: path.to_string_lossy().into(), source_url: String::new(), artist: tag.and_then(|t| t.artist()).map(|s| s.into_owned()).unwrap_or_default(), title: tag.and_then(|t| t.title()).map(|s| s.into_owned()).unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into()), album: tag.and_then(|t| t.album()).map(|s| s.into_owned()), genre: tag.and_then(|t| t.genre()).map(|s| s.into_owned()), label: text(ItemKey::Label), bpm: text(ItemKey::Bpm).or_else(|| text(ItemKey::Unknown("BPM".into()))).or_else(|| text(ItemKey::IntegerBpm)).and_then(|s| s.parse::<f64>().ok()).filter(|bpm| bpm.is_finite() && *bpm > 0.0 && *bpm <= 1000.0), key: text(ItemKey::InitialKey), release_date, recording_date, isrc: text(ItemKey::Isrc), catalog_number: text(ItemKey::CatalogNumber), duration_seconds: Some(info.duration), local_path: Some(path.to_string_lossy().into()), provenance: serde_json::json!({"source":"local file tags", "codec":info.codec, "bitrateKbps":info.bitrate}), ..Track::default() })
    }).await.map_err(|e| e.to_string())?
}

fn local_date_fields(
    explicit_release_date: Option<String>,
    generic_date: Option<String>,
    burrata_date_kind: Option<&str>,
) -> (Option<String>, Option<String>) {
    match burrata_date_kind {
        Some("recording") => (explicit_release_date, generic_date),
        Some("legacy") => (explicit_release_date.or(generic_date), None),
        _ => match explicit_release_date {
            Some(release) => (Some(release), generic_date),
            None => (generic_date, None),
        },
    }
}

pub async fn open_in_file_manager(path: &Path) -> Result<(), String> {
    let target = if path.is_dir() {
        path
    } else {
        path.parent().ok_or("File has no parent directory")?
    };
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    command
        .arg(target)
        .spawn()
        .map_err(|e| format!("Cannot open file manager: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_opus_stream_bitrate_is_reported_as_estimated_when_available() {
        let (bitrate, origin) = super::resolve_bitrate(None, None, Some(1_200_000), 60.0);
        assert_eq!(bitrate, Some(160.0));
        assert_eq!(origin.as_deref(), Some("file-size-estimate"));

        let (bitrate, origin) = super::resolve_bitrate(None, Some(128.0), Some(1_200_000), 60.0);
        assert_eq!(bitrate, Some(128.0));
        assert_eq!(origin.as_deref(), Some("format-average"));
    }

    #[tokio::test]
    async fn release_and_recording_dates_round_trip_without_legacy_date_confusion() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::default();
        let make_audio = |path: &Path| {
            std::process::Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.1",
                    "-c:a",
                    "flac",
                ])
                .arg(path)
                .output()
                .unwrap()
        };

        let dated_path = dir.path().join("dated.flac");
        let output = make_audio(&dated_path);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let dated = Track {
            artist: "Artist".into(),
            title: "Song".into(),
            release_date: Some("2024-06-12".into()),
            recording_date: Some("1968-02-03".into()),
            ..Track::default()
        };
        tag_media(&dated_path, &dated, None).unwrap();
        let read = inspect_local_file(&dated_path, &settings).await.unwrap();
        assert_eq!(read.release_date.as_deref(), Some("2024-06-12"));
        assert_eq!(read.recording_date.as_deref(), Some("1968-02-03"));

        let recording_only_path = dir.path().join("recording-only.flac");
        let output = make_audio(&recording_only_path);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let recording_only = Track {
            artist: "Artist".into(),
            title: "Performance".into(),
            recording_date: Some("1968-02-03".into()),
            ..Track::default()
        };
        tag_media(&recording_only_path, &recording_only, None).unwrap();
        let tagged = Probe::open(&recording_only_path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();
        assert!(
            date_kind_marker(
                tag.get_string(&ItemKey::Comment)
                    .map(|value| value.to_owned())
            )
            .as_deref()
                == Some("recording")
        );
        let read = inspect_local_file(&recording_only_path, &settings)
            .await
            .unwrap();
        assert_eq!(read.release_date, None);
        assert_eq!(read.recording_date.as_deref(), Some("1968-02-03"));

        let legacy_path = dir.path().join("legacy-date.flac");
        let output = make_audio(&legacy_path);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut tagged = Probe::open(&legacy_path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let tag_type = tagged.primary_tag_type();
        if tagged.tag(tag_type).is_none() {
            tagged.insert_tag(Tag::new(tag_type));
        }
        tagged
            .tag_mut(tag_type)
            .unwrap()
            .insert_text(ItemKey::RecordingDate, "1991-07-08".to_string());
        tagged
            .save_to_path(&legacy_path, WriteOptions::default())
            .unwrap();
        let read = inspect_local_file(&legacy_path, &settings).await.unwrap();
        assert_eq!(read.release_date.as_deref(), Some("1991-07-08"));
        assert_eq!(read.recording_date, None);
    }

    #[tokio::test]
    async fn inferred_live_tags_preserve_source_without_studio_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let source_title = "Big Thief - Mary (Live at Pitchfork Music Festival 2018)";
        let source_url = "https://www.youtube.com/watch?v=hcPiH6u8wmg";
        let mut track = Track {
            id: "spotify:studio-mary".into(),
            provider: "spotify".into(),
            provider_id: "studio-mary".into(),
            artist: "Big Thief".into(),
            title: "Mary".into(),
            album: Some("Capacity".into()),
            release_date: Some("2017-06-09".into()),
            isrc: Some("studio-isrc".into()),
            ..Track::default()
        };
        crate::metadata_normalization::normalize_catalog_live_download(
            &mut track,
            source_title,
            source_url,
            "hcPiH6u8wmg",
            None,
            None,
        )
        .unwrap();
        for (extension, codec) in [("mp3", "libmp3lame"), ("flac", "flac")] {
            let path = dir.path().join(format!("live.{extension}"));
            let output = Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.1",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            tag_media(&path, &track, None).unwrap();
            let tagged = Probe::open(&path)
                .unwrap()
                .guess_file_type()
                .unwrap()
                .read()
                .unwrap();
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();
            assert_eq!(tag.artist().as_deref(), Some("Big Thief"));
            assert_eq!(
                tag.title().as_deref(),
                Some("Mary (Live at Pitchfork Music Festival 2018)")
            );
            assert!(tag.album().is_none());
            assert!(tag.get_string(&ItemKey::RecordingDate).is_none());
            assert!(tag.get_string(&ItemKey::Isrc).is_none());
            let comment = tag.get_string(&ItemKey::Comment).unwrap();
            assert!(comment.contains(source_title));
            assert!(comment.contains(source_url));
            assert!(comment.contains("unverified"));
        }
    }
    use super::*;

    #[tokio::test]
    #[ignore = "requires root-provided scratch AAC excerpt and catalog fixture; never touches app library"]
    async fn scratch_pengus_excerpt_finalization_diagnostic() {
        let track_path = std::env::var("BURRATA_EXCERPT_REPRO_TRACK_JSON")
            .expect("set path to the sanitized catalog track JSON fixture");
        let excerpt_path = std::env::var("BURRATA_EXCERPT_REPRO_AUDIO")
            .expect("set path to the local AAC excerpt scratch file");
        let track: Track = serde_json::from_slice(&fs::read(track_path).unwrap()).unwrap();
        let mut track = track;
        // Keep this diagnostic focused on finalization, not optional key/BPM
        // analysis; these values are outside the recording identity gate.
        track.bpm = Some(120.0);
        track.key = Some("C".into());
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings {
            music_directory: directory.path().to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let candidate = Candidate {
            id: "youtube:U6bGnc68PSM".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=U6bGnc68PSM".into(),
            title: "Neon Drive - Full Original Soundtrack by Pengus".into(),
            duration_seconds: Some(97.714),
            ..Candidate::default()
        };
        let mut stage = "scratch_start";
        match finalize_download_with_diagnostic(
            Path::new(&excerpt_path),
            &track,
            &candidate,
            &settings,
            "source",
            "scratch-pengus-excerpt",
            &mut stage,
        )
        .await
        {
            Ok(asset) => eprintln!(
                "SCRATCH_EXCERPT_FINALIZE_OK stage={stage} codec={} duration={:?}",
                asset.final_codec, asset.duration_seconds
            ),
            Err(error) => {
                let bounded_error = error.chars().take(240).collect::<String>();
                eprintln!("SCRATCH_EXCERPT_FINALIZE_FAILED stage={stage} error={bounded_error}");
                panic!("scratch excerpt finalization failed at {stage}");
            }
        }
    }

    #[test]
    fn sanitizes_portable_names() {
        assert_eq!(safe_component("../CON: /?"), ".._CON_ __");
        assert_eq!(safe_component("CON"), "_CON");
        assert_eq!(safe_component("..."), "Unknown");
        assert_eq!(safe_component("label\nartist"), "label_artist");
    }
    #[test]
    fn rejects_private_artwork_networks() {
        for address in [
            "127.0.0.1",
            "192.168.1.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(!public_address(address.parse().unwrap()), "{address}");
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
        assert!(public_address("2606:4700:4700::1111".parse().unwrap()));
    }
    #[test]
    fn detects_supported_artwork_extensions_from_content() {
        assert_eq!(artwork_extension(&[0xff, 0xd8, 0xff, 0]).unwrap(), "jpg");
        assert_eq!(artwork_extension(b"\x89PNG\r\n\x1a\n").unwrap(), "png");
        assert_eq!(artwork_extension(b"RIFF0000WEBP").unwrap(), "webp");
        assert!(artwork_extension(b"not an image").is_err());
    }
    #[tokio::test]
    async fn optional_artwork_failure_keeps_existing_cover_and_does_not_block_text_tags() {
        if Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let cover_path = directory.path().join("cover.png");
        let cover_result = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=16x16",
                "-frames:v",
                "1",
            ])
            .arg(&cover_path)
            .output()
            .await
            .unwrap();
        assert!(cover_result.status.success());
        let cover = fs::read(&cover_path).unwrap();
        let with_cover = directory.path().join("with-cover.flac");
        let without_cover = directory.path().join("without-cover.flac");
        for path in [&with_cover, &without_cover] {
            let result = Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-c:a",
                    "flac",
                ])
                .arg(path)
                .output()
                .await
                .unwrap();
            assert!(result.status.success());
        }

        let old_track = Track {
            artist: "Old Artist".into(),
            title: "Old Title".into(),
            album: Some("Old Album".into()),
            ..Track::default()
        };
        tag_media(&with_cover, &old_track, Some(cover.clone())).unwrap();
        let selected = Track {
            artist: "Bulgarian Choir".into(),
            title: "Svatba".into(),
            album: Some("Le Mystère des voix bulgares".into()),
            ..Track::default()
        };

        // This simulates an optional artwork request returning HTTP 500.
        let fallback =
            optional_artwork_or_embedded(Err("HTTP 500 from artwork service".into()), &with_cover);
        assert_eq!(fallback.as_deref(), Some(cover.as_slice()));
        // A missing remote URL/result is also optional and retains the local cover.
        assert_eq!(
            optional_artwork_or_embedded(Ok(None), &with_cover).as_deref(),
            Some(cover.as_slice())
        );
        tag_media(&with_cover, &selected, fallback).unwrap();
        let read = inspect_local_file(&with_cover, &Settings::default())
            .await
            .unwrap();
        assert_eq!(read.artist, selected.artist);
        assert_eq!(read.title, selected.title);
        assert_eq!(read.album, selected.album);
        assert_eq!(
            embedded_artwork(&with_cover).as_deref(),
            Some(cover.as_slice())
        );

        // With neither remote nor embedded artwork, text tagging still succeeds.
        let no_cover = optional_artwork_or_embedded(
            Err("HTTP 500 from artwork service".into()),
            &without_cover,
        );
        assert!(no_cover.is_none());
        tag_media(&without_cover, &selected, no_cover).unwrap();
        let read = inspect_local_file(&without_cover, &Settings::default())
            .await
            .unwrap();
        assert_eq!(read.artist, selected.artist);
        assert_eq!(read.title, selected.title);
        let tagged = Probe::open(&without_cover)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        assert!(tagged
            .tags()
            .iter()
            .flat_map(|tag| tag.pictures())
            .next()
            .is_none());
    }
    #[test]
    fn unicode_filenames_fit_filesystem_limits() {
        assert!(safe_component(&"🎵".repeat(100)).len() <= 90);
    }

    #[test]
    fn publication_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged");
        fs::write(&staged, b"new").unwrap();
        let existing = dir.path().join("Artist - Title.mp3");
        fs::write(&existing, b"old").unwrap();
        let target = publish(
            &staged,
            dir.path(),
            "Artist - Title",
            "mp3",
            "123456789012abcdef",
        )
        .unwrap();
        assert_ne!(target, existing);
        assert_eq!(fs::read(existing).unwrap(), b"old");
        assert_eq!(fs::read(target).unwrap(), b"new");
    }
    #[test]
    fn publication_recovers_after_database_commit_gap() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings {
            music_directory: directory.path().to_string_lossy().into(),
            ..Settings::default()
        };
        let job = "test-recovery";
        let stage = staging(&settings, job).unwrap();
        let audio = stage.join("audio.mp3");
        fs::write(&audio, b"fixture audio").unwrap();
        let mut asset = LocalAsset {
            id: "asset".into(),
            recording_id: "recording".into(),
            path: String::new(),
            hash: hash_file(&audio).unwrap(),
            source_codec: "mp3".into(),
            final_codec: "mp3".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            processing: "preserved".into(),
            track: Track::default(),
        };
        journaled_publish(
            &audio,
            &settings,
            job,
            "Artist - Title",
            "mp3",
            &mut asset,
            None,
        )
        .unwrap();
        let recovered = recover_publications(&settings).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].0, job);
        assert_eq!(recovered[0].1.path, asset.path);
        assert_eq!(recovered[0].1.hash, asset.hash);
        acknowledge_publication(&settings, job).unwrap();
        assert!(recover_publications(&settings).unwrap().is_empty());
        assert!(Path::new(&asset.path).exists());
    }

    #[test]
    fn publication_recovery_clears_missing_target_intent_before_same_job_retry() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings {
            music_directory: directory.path().to_string_lossy().into(),
            ..Settings::default()
        };
        let job = "album-excerpt-retry";
        let stage = staging(&settings, job).unwrap();
        let first_audio = stage.join("first.m4a");
        fs::write(&first_audio, b"first attempt").unwrap();
        let mut first_asset = LocalAsset {
            id: "first-asset".into(),
            recording_id: "recording".into(),
            path: String::new(),
            hash: hash_file(&first_audio).unwrap(),
            source_codec: "aac".into(),
            final_codec: "aac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: Some(90.0),
            size_bytes: None,
            bitrate_origin: None,
            processing: "fixture".into(),
            track: Track::default(),
        };
        journaled_publish(
            &first_audio,
            &settings,
            job,
            "Artist - Title",
            "m4a",
            &mut first_asset,
            None,
        )
        .unwrap();
        let journal = directory
            .path()
            .join(".burrata-staging")
            .join(job)
            .join("publication.json");
        assert!(journal.is_file());

        // Simulate a crash after journaling but before the final hard-link.
        fs::remove_file(&first_asset.path).unwrap();
        assert!(recover_publications(&settings).unwrap().is_empty());
        assert!(!journal.exists());

        let retry_audio = stage.join("retry.m4a");
        fs::write(&retry_audio, b"retry attempt").unwrap();
        let mut retry_asset = LocalAsset {
            id: "retry-asset".into(),
            recording_id: "recording".into(),
            path: String::new(),
            hash: hash_file(&retry_audio).unwrap(),
            source_codec: "aac".into(),
            final_codec: "aac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: Some(90.0),
            size_bytes: None,
            bitrate_origin: None,
            processing: "fixture".into(),
            track: Track::default(),
        };
        journaled_publish(
            &retry_audio,
            &settings,
            job,
            "Artist - Title",
            "m4a",
            &mut retry_asset,
            None,
        )
        .unwrap();
        assert_eq!(recover_publications(&settings).unwrap().len(), 1);
        assert!(Path::new(&retry_asset.path).is_file());
    }

    #[test]
    fn compatibility_export_labels_do_not_upgrade_quality() {
        let copy = export_processing("mp3", "/source.mp3");
        assert!(copy.contains("without re-encoding"));
        assert!(!copy.contains("320"));
        let transcoded = export_processing("opus", "/source.opus");
        assert!(transcoded.contains("Lossy opus"));
        assert!(transcoded.contains("quality is not upgraded"));
        assert!(export_processing("flac", "/source.flac").contains("encoded from flac"));
    }
    #[tokio::test]
    async fn archive_reuse_detects_external_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audio.mp3");
        fs::write(&path, b"original").unwrap();
        let asset = LocalAsset {
            id: "asset".into(),
            recording_id: "recording".into(),
            path: path.to_string_lossy().into(),
            hash: hash_file(&path).unwrap(),
            source_codec: "mp3".into(),
            final_codec: "mp3".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            processing: String::new(),
            track: Track::default(),
        };
        verify_asset(&asset).await.unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(verify_asset(&asset).await.is_err());
    }

    #[tokio::test]
    async fn selected_chapters_publish_independent_tagged_segments_only() {
        if Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .is_err()
            || Command::new("ffprobe")
                .arg("-version")
                .output()
                .await
                .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            music_directory: dir.path().to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let source = dir.path().join("parent.wav");
        let generated = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-c:a",
                "pcm_s16le",
                "-y",
            ])
            .arg(&source)
            .output()
            .await
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );

        let candidate = Candidate {
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=parent".into(),
            duration_seconds: Some(2.0),
            ..Default::default()
        };
        let chapters = [
            Chapter {
                id: "one".into(),
                title: "Artist One - Track One".into(),
                start_seconds: 0.0,
                end_seconds: 2.0,
                confidence: "source_metadata_unverified".into(),
                suggested_track: None,
                suggestion_confidence: None,
            },
            Chapter {
                id: "two".into(),
                title: "Artist Two - Track Two".into(),
                start_seconds: 2.0,
                end_seconds: 4.0,
                confidence: "source_metadata_unverified".into(),
                suggested_track: None,
                suggestion_confidence: None,
            },
        ];
        let mut published = Vec::new();
        for (index, chapter) in chapters.iter().enumerate() {
            let segment = split_chapter(&source, chapter, &settings, "chapter-fixture", index)
                .await
                .unwrap();
            let (artist, title) = chapter.title.split_once(" - ").unwrap();
            let provider_id = format!("youtube:parent:{}", chapter.id);
            let track = Track {
                id: format!("chapter:{provider_id}"),
                provider: "chapter".into(),
                provider_id,
                source_url: candidate.url.clone(),
                artist: artist.into(),
                title: title.into(),
                album: Some("Parent recording".into()),
                duration_seconds: Some(2.0),
                isrc: None,
                provenance: serde_json::json!({"chapter":{"parentTitle":"Parent recording","startSeconds":chapter.start_seconds,"endSeconds":chapter.end_seconds,"identity":"unverified chapter-derived recording"}}),
                ..Track::default()
            };
            let asset = finalize_download(
                &segment,
                &track,
                &candidate,
                &settings,
                "source",
                &format!("chapter-fixture-{index}"),
            )
            .await
            .unwrap();
            let read = inspect_local_file(Path::new(&asset.path), &settings)
                .await
                .unwrap();
            assert_eq!(read.artist, artist);
            assert_eq!(read.title, title);
            assert!(read.isrc.is_none());
            assert!((read.duration_seconds.unwrap() - 2.0).abs() < 0.2);
            published.push(asset);
        }
        assert_eq!(published.len(), 2);
        assert!(published
            .iter()
            .all(|asset| !asset.path.contains("Parent recording")));
        assert!(!published.iter().any(|asset| asset.track.id == "parent"));
    }

    #[tokio::test]
    async fn tags_round_trip_supported_formats() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let artwork_path = dir.path().join("cover.png");
        let artwork_result = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=16x16",
                "-frames:v",
                "1",
            ])
            .arg(&artwork_path)
            .output()
            .await
            .unwrap();
        assert!(
            artwork_result.status.success(),
            "PNG fixture: {}",
            String::from_utf8_lossy(&artwork_result.stderr)
        );
        let cover = fs::read(&artwork_path).unwrap();
        let track = Track {
            artist: "Test artist".into(),
            title: "Test title".into(),
            version: Some("Extended Mix".into()),
            album: Some("Test release".into()),
            label: Some("Burrata Records".into()),
            genre: Some("House".into()),
            bpm: Some(123.4),
            key: Some("Am".into()),
            release_date: Some("2024-06-12".into()),
            isrc: Some("US-ABC-24-00001".into()),
            catalog_number: Some("BR-2024-007".into()),
            ..Track::default()
        };
        for (extension, codec) in [
            ("mp3", "libmp3lame"),
            ("flac", "flac"),
            ("opus", "libopus"),
            ("m4a", "aac"),
            ("wav", "pcm_s16le"),
            ("aiff", "pcm_s16be"),
        ] {
            let path = dir.path().join(format!("test.{extension}"));
            let result = Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .output()
                .await
                .unwrap();
            assert!(
                result.status.success(),
                "{extension}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            tag_media(&path, &track, Some(cover.clone())).unwrap();
            if extension == "mp3" {
                let asset = LocalAsset {
                    id: "embedded-cover-test".into(),
                    recording_id: track.id.clone(),
                    path: fs::canonicalize(&path)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    hash: hash_file(&path).unwrap(),
                    source_codec: "mp3".into(),
                    final_codec: "mp3".into(),
                    source_bitrate: None,
                    final_bitrate: None,
                    sample_rate: None,
                    channels: None,
                    bit_depth: None,
                    duration_seconds: None,
                    size_bytes: None,
                    bitrate_origin: None,
                    processing: "fixture".into(),
                    track: track.clone(),
                };
                assert_eq!(
                    embedded_artwork_for_asset(&asset).await.unwrap().unwrap(),
                    cover
                );
            }
            let read = inspect_local_file(&path, &Settings::default())
                .await
                .unwrap();
            assert_eq!(read.artist, track.artist);
            assert_eq!(read.title, "Test title (Extended Mix)", "{extension}");
            assert_eq!(read.album, track.album, "{extension}");
            assert_eq!(read.label, track.label, "{extension}");
            assert_eq!(read.genre, track.genre, "{extension}");
            assert_eq!(read.bpm, track.bpm, "{extension}");
            assert_eq!(read.bpm, track.bpm);
            assert_eq!(read.key, track.key, "{extension}");
            assert_eq!(read.release_date, track.release_date, "{extension}");
            assert_eq!(read.isrc, track.isrc, "{extension}");
            assert_eq!(read.catalog_number, track.catalog_number, "{extension}");

            let tagged = Probe::open(&path)
                .unwrap()
                .guess_file_type()
                .unwrap()
                .read()
                .unwrap();
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();
            let picture = tag
                .pictures()
                .first()
                .unwrap_or_else(|| panic!("{extension}: artwork was not persisted"));
            assert_eq!(picture.data(), cover, "{extension}");
        }
    }

    #[tokio::test]
    async fn autotag_merge_writes_isrc_and_catalog_number_without_replacing_other_tags() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autotag-merge.flac");
        let result = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:a",
                "flac",
            ])
            .arg(&path)
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "FLAC fixture: {}",
            String::from_utf8_lossy(&result.stderr)
        );

        let mut track = Track {
            artist: "Curated Artist".into(),
            title: "Curated Title".into(),
            album: Some("Curated Album".into()),
            isrc: Some("USABC2400001".into()),
            catalog_number: Some("CURATED-42".into()),
            ..Track::default()
        };
        tag_media(&path, &track, None).unwrap();
        let mut tagged = Probe::open(&path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let primary_tag_type = tagged.primary_tag_type();
        let tag = tagged
            .tag_mut(primary_tag_type)
            .expect("primary tag exists");
        tag.insert_text(ItemKey::Comment, "Keep this producer note".into());
        tagged.save_to_path(&path, WriteOptions::default()).unwrap();

        // The merged Track is authoritative, including manually retained values.
        track.provenance = serde_json::json!({"manualFields":{"isrc":true,"catalogNumber":true}});
        merge_autotag_tags(&path, &track).unwrap();
        let read = inspect_local_file(&path, &Settings::default())
            .await
            .unwrap();
        assert_eq!(read.isrc.as_deref(), Some("USABC2400001"));
        assert_eq!(read.catalog_number.as_deref(), Some("CURATED-42"));
        assert_eq!(read.artist, "Curated Artist");
        assert_eq!(read.title, "Curated Title");
        assert_eq!(read.album.as_deref(), Some("Curated Album"));
        let tagged = Probe::open(&path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();
        assert_eq!(
            tag.get_string(&ItemKey::Comment).as_deref(),
            Some("Keep this producer note")
        );
    }

    #[test]
    fn key_estimate_accepts_tonal_audio_and_rejects_silence_and_clicks() {
        let sample_rate = KEY_SAMPLE_RATE as usize;
        let duration = sample_rate * 40;
        let chord = |frequencies: [f64; 3]| {
            (0..duration)
                .map(|index| {
                    let time = index as f64 / sample_rate as f64;
                    (frequencies
                        .iter()
                        .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
                        .sum::<f64>()
                        / 3.0) as f32
                })
                .collect::<Vec<_>>()
        };
        for (expected, frequencies) in [
            ("C", [130.8128, 164.8138, 196.0]),
            ("Am", [220.0, 261.6256, 329.6276]),
        ] {
            let detected = estimate_key(&chord(frequencies))
                .unwrap_or_else(|| panic!("clear {expected} fixture should be classified"));
            assert_eq!(detected.key, expected, "{detected:?}");
            assert_eq!(detected.window_keys.iter().flatten().count(), 5);
        }

        assert!(estimate_key(&vec![0.0; duration]).is_none());
        let mut clicks = vec![0.0; duration];
        for offset in (0..duration).step_by(sample_rate / 2) {
            for (sample_index, sample) in clicks[offset..offset + 64].iter_mut().enumerate() {
                *sample = if sample_index % 2 == 0 { 0.8 } else { -0.8 };
            }
        }
        assert!(
            estimate_key(&clicks).is_none(),
            "percussion-only signal was labeled a key"
        );
    }

    #[tokio::test]
    async fn asset_retag_preserves_audio_and_recovers_the_committed_journal() {
        let dir = tempfile::tempdir().unwrap();
        let library = fs::canonicalize(dir.path()).unwrap();
        let settings = Settings {
            music_directory: library.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let path = library.join("retag-fixture.wav");
        let result = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=0.3*sin(2*PI*130.8128*t)+0.3*sin(2*PI*164.8138*t)+0.3*sin(2*PI*196*t):s=44100:d=40",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&path)
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "Audio fixture: {}",
            String::from_utf8_lossy(&result.stderr)
        );

        let old_track = Track {
            id: "recording-1".into(),
            artist: "Old artist".into(),
            title: "Old title".into(),
            ..Track::default()
        };
        tag_media(&path, &old_track, None).unwrap();
        let original_audio = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&path)
            .args(["-map", "0:a:0", "-f", "s16le", "pipe:1"])
            .output()
            .await
            .unwrap();
        assert!(original_audio.status.success());
        let original_hash = hash_file(&path).unwrap();
        let asset = LocalAsset {
            id: "asset-1".into(),
            recording_id: "recording-1".into(),
            path: path.to_string_lossy().into_owned(),
            hash: original_hash.clone(),
            source_codec: "wav".into(),
            final_codec: "wav".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            processing: "preserved".into(),
            track: Track {
                id: "recording-1".into(),
                artist: "Corrected artist".into(),
                title: "Corrected title".into(),
                version: Some("Radio Edit (3:45)".into()),
                bpm: Some(125.3),
                ..Track::default()
            },
        };

        let updated = analyze_and_retag_asset(&asset, &settings).await.unwrap();
        assert_eq!(updated.path, asset.path);
        assert_ne!(updated.hash, original_hash);
        assert_eq!(updated.track.artist, "Corrected artist");
        assert_eq!(updated.track.title, "Corrected title");
        assert_eq!(updated.track.version.as_deref(), Some("Radio Edit (3:45)"));
        assert_eq!(
            updated.track.bpm,
            Some(125.3),
            "provided BPM must be preserved"
        );
        assert_eq!(updated.track.key.as_deref(), Some("C"));
        assert_eq!(
            updated.track.provenance["keyAnalysis"]["status"],
            "complete"
        );
        let journal_id = updated.track.provenance["retagJournalId"]
            .as_str()
            .expect("retag journal id should be returned");

        let inspected = inspect_local_file(&path, &settings).await.unwrap();
        assert_eq!(inspected.artist, "Corrected artist");
        assert_eq!(inspected.title, "Corrected title (Radio Edit)");
        assert_eq!(inspected.bpm, Some(125.3));
        assert_eq!(inspected.key.as_deref(), Some("C"));
        let current_audio = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&path)
            .args(["-map", "0:a:0", "-f", "s16le", "pipe:1"])
            .output()
            .await
            .unwrap();
        assert!(current_audio.status.success());
        assert_eq!(
            current_audio.stdout, original_audio.stdout,
            "retag changed decoded audio"
        );

        let recovered = recover_retag_journals(&settings).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].0, journal_id);
        assert_eq!(recovered[0].1.hash, updated.hash);
        acknowledge_retag_journal(&settings, journal_id).unwrap();
        assert!(recover_retag_journals(&settings).unwrap().is_empty());
    }

    #[tokio::test]
    async fn asset_retag_refuses_an_externally_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            music_directory: dir.path().to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let path = dir.path().join("externally-changed.wav");
        fs::write(&path, b"initial file bytes").unwrap();
        let asset = LocalAsset {
            id: "asset-2".into(),
            recording_id: "recording-2".into(),
            path: path.to_string_lossy().into_owned(),
            hash: hash_file(&path).unwrap(),
            source_codec: "wav".into(),
            final_codec: "wav".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            processing: String::new(),
            track: Track {
                id: "recording-2".into(),
                artist: "Artist".into(),
                title: "Title".into(),
                ..Track::default()
            },
        };
        fs::write(&path, b"changed outside Burrata").unwrap();
        let changed_hash = hash_file(&path).unwrap();
        assert!(analyze_and_retag_asset(&asset, &settings).await.is_err());
        assert_eq!(hash_file(&path).unwrap(), changed_hash);
        assert!(recover_retag_journals(&settings).unwrap().is_empty());
    }

    #[tokio::test]
    async fn detects_bpm_from_decoded_pulse_audio_and_persists_it_as_a_tag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pulse.wav");
        let result = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=exprs='if(lt(mod(t,0.5),0.08),sin(2*PI*80*t)*exp(-mod(t,0.5)*25),0)':s=44100:d=32",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&path)
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "Synthetic pulse audio: {}",
            String::from_utf8_lossy(&result.stderr)
        );

        let analysis = detect_bpm(&path, &Settings::default())
            .await
            .unwrap()
            .expect("regular 120 BPM pulses should produce an estimate");
        assert!((analysis.bpm - 120.0).abs() <= 1.0, "{analysis:?}");
        assert!(analysis.confidence >= 0.9, "{analysis:?}");
        assert!(
            (analysis.related_tempo_bpm.unwrap_or_default() - 60.0).abs() <= 1.0,
            "{analysis:?}"
        );

        let mut track = Track {
            artist: "Synthetic artist".into(),
            title: "Pulse test".into(),
            ..Track::default()
        };
        track = add_bpm_analysis(&track, Some(analysis), None);
        assert_eq!(track.bpm, Some(analysis.bpm));
        assert_eq!(track.provenance["bpmAnalysis"]["bpm"], analysis.bpm);
        assert_eq!(
            track.provenance["bpmAnalysis"]["relatedTempoAlternativeBpm"],
            serde_json::json!(analysis.related_tempo_bpm)
        );
        assert_eq!(track.provenance["bpmAnalysis"]["tempoAmbiguity"], true);
        tag_media(&path, &track, None).unwrap();
        let inspected = inspect_local_file(&path, &Settings::default())
            .await
            .unwrap();
        assert_eq!(inspected.bpm, Some(analysis.bpm));
    }

    #[test]
    fn detected_bpm_never_replaces_catalog_bpm() {
        let catalog = Track {
            bpm: Some(123.5),
            ..Track::default()
        };
        let result = add_bpm_analysis(
            &catalog,
            Some(BpmAnalysis {
                bpm: 128.0,
                confidence: 0.91,
                related_tempo_bpm: None,
            }),
            None,
        );
        assert_eq!(result.bpm, Some(123.5));
        assert!(result.provenance.get("bpmAnalysis").is_none());
    }

    fn periodic_pulse_envelope(bpm: f64, seconds: usize) -> Vec<f64> {
        let mut envelope = vec![0.0; seconds * 100];
        let period_frames = 6000.0 / bpm;
        let pulses = (envelope.len() as f64 / period_frames).floor() as usize;
        for pulse in 0..pulses {
            let frame = (pulse as f64 * period_frames).round() as usize;
            if let Some(value) = envelope.get_mut(frame) {
                *value = 1.0;
            }
        }
        envelope
    }

    #[test]
    fn unrelated_fast_peak_does_not_replace_stronger_tempo() {
        let mut envelope = vec![0.0; 3200];
        for frame in (0..envelope.len()).step_by(43) {
            envelope[frame] += 1.0;
        }
        for frame in (0..envelope.len()).step_by(32) {
            envelope[frame] += 0.7;
        }
        let estimate = estimate_bpm(&envelope).expect("dominant regular tempo");
        assert!(
            (estimate.bpm - 6000.0 / 43.0).abs() < 1.0,
            "an unrelated faster rhythm replaced the stronger tempo: {estimate:?}"
        );
    }

    #[test]
    fn bpm_local_peaks_recover_fast_periods_and_expose_octave_ambiguity() {
        for bpm in [60.0, 63.0, 75.0, 90.0, 120.0, 150.0, 180.0, 196.0] {
            let estimate = estimate_bpm(&periodic_pulse_envelope(bpm, 32))
                .unwrap_or_else(|| panic!("regular {bpm} BPM pulses should be estimated"));
            let tolerance = if bpm >= 180.0 { 3.0 } else { 1.0 };
            assert!(
                (estimate.bpm - bpm).abs() <= tolerance,
                "expected {bpm}, got {estimate:?}"
            );
        }

        let ambiguous = estimate_bpm(&periodic_pulse_envelope(120.0, 32)).unwrap();
        assert!((ambiguous.bpm - 120.0).abs() <= 1.0, "{ambiguous:?}");
        assert!(
            (ambiguous.related_tempo_bpm.unwrap_or_default() - 60.0).abs() <= 1.0,
            "{ambiguous:?}"
        );

        let mut slow_with_weak_subbeats = vec![0.0; 3200];
        for beat in 0..32 {
            let main = beat * 100;
            slow_with_weak_subbeats[main] = [1.0, 0.8, 0.9, 0.7][beat % 4];
            let jitter = [0_i32, 1, -1, 2, -2][beat % 5];
            let subbeat = (main as i32 + 50 + jitter) as usize;
            slow_with_weak_subbeats[subbeat] = 0.25;
        }
        for (index, value) in slow_with_weak_subbeats.iter_mut().enumerate() {
            *value += ((index * 37 % 101) as f64 / 101.0) * 0.015;
        }
        let slow =
            estimate_bpm(&slow_with_weak_subbeats).expect("regular main beats remain detectable");
        assert!(
            (slow.bpm - 60.0).abs() <= 1.0,
            "weak jittered subdivisions became the tempo: {slow:?}"
        );
        assert!(
            slow.related_tempo_bpm.is_none(),
            "weak subdivisions should not be presented as an equally likely tempo: {slow:?}"
        );
    }

    #[test]
    fn automatic_source_reported_fields_are_scoped_without_claiming_user_approval() {
        let mut metadata = Track::default();
        metadata.provenance = serde_json::json!({"source":"exact_source_reported_metadata","identityStatus":"source_reported_unverified","sourceReportedFields":["artist","album","label"]});
        assert!(autotag_field_approved(&metadata, "artist"));
        assert!(autotag_field_approved(&metadata, "album"));
        assert!(!autotag_field_approved(&metadata, "label"));
        assert!(!autotag_field_approved(&metadata, "title"));
        assert!(metadata.provenance.get("userApprovedFields").is_none());
        metadata.provenance["source"] = serde_json::json!("catalog lookup");
        assert!(!autotag_field_approved(&metadata, "artist"));
    }

    fn source_reported_test_track() -> Track {
        Track {
            id: "youtube:video-123".into(),
            provider: "youtube".into(),
            provider_id: "video-123".into(),
            source_url: "https://youtube.com/watch?v=video-123".into(),
            ..Track::default()
        }
    }

    fn source_reported_test_metadata() -> Track {
        let marker = serde_json::json!({
            "identityStatus":"source_reported_unverified",
            "provider":"youtube",
            "providerId":"video-123",
            "sourceUrl":"https://youtube.com/watch?v=video-123",
            "origin":"ytDlpFullInfo",
            "fields":["artist","album"]
        });
        Track {
            artist: "Los Niños de Sara".into(),
            album: Some("Gipsyolé".into()),
            provenance: serde_json::json!({
                "source":"exact_source_reported_metadata",
                "identityStatus":"source_reported_unverified",
                "sourceReportedFields":["artist","album"],
                "sourceReportedMetadata":marker
            }),
            ..Track::default()
        }
    }

    #[test]
    fn source_reported_provenance_requires_exact_stable_source_identity() {
        let source = source_reported_test_track();
        let metadata = source_reported_test_metadata();
        assert!(source_reported_metadata_provenance(&source, &metadata).is_some());

        let mut stale = metadata.clone();
        stale.provenance["sourceReportedMetadata"]["sourceUrl"] =
            serde_json::json!("https://youtube.com/watch?v=other-video");
        assert!(source_reported_metadata_provenance(&source, &stale).is_none());

        let mut unsupported_field = metadata.clone();
        unsupported_field.provenance["sourceReportedFields"] = serde_json::json!(["providerId"]);
        assert!(source_reported_metadata_provenance(&source, &unsupported_field).is_none());

        let mut tampered_inner_status = metadata.clone();
        tampered_inner_status.provenance["sourceReportedMetadata"]["identityStatus"] =
            serde_json::json!("catalog_verified");
        assert!(source_reported_metadata_provenance(&source, &tampered_inner_status).is_none());
    }

    #[tokio::test]
    async fn source_reported_marker_is_in_returned_asset_and_committed_recovery_journal() {
        let directory = tempfile::tempdir().unwrap();
        let library = fs::canonicalize(directory.path()).unwrap();
        let settings = Settings {
            music_directory: library.to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let path = library.join("source-reported-fixture.wav");
        let generated = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&path)
            .output()
            .await
            .unwrap();
        assert!(generated.status.success());
        let source_track = Track {
            artist: String::new(),
            title: "La Cubanita".into(),
            local_path: Some(path.to_string_lossy().into_owned()),
            ..source_reported_test_track()
        };
        let asset = LocalAsset {
            id: "asset-source-reported".into(),
            recording_id: source_track.id.clone(),
            path: path.to_string_lossy().into_owned(),
            hash: hash_file(&path).unwrap(),
            source_codec: "wav".into(),
            final_codec: "wav".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: Some(3.0),
            size_bytes: None,
            bitrate_origin: None,
            processing: "preserved".into(),
            track: source_track,
        };
        let metadata = source_reported_test_metadata();
        let expected_marker = metadata.provenance["sourceReportedMetadata"].clone();
        let updated = autotag_asset(&asset, &metadata, &settings).await.unwrap();
        assert_eq!(updated.track.artist, "Los Niños de Sara");
        assert_eq!(updated.track.album.as_deref(), Some("Gipsyolé"));
        assert_eq!(
            updated.track.provenance["sourceReportedMetadata"],
            expected_marker
        );

        let recovered = recover_retag_journals(&settings).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].1.track.provenance["sourceReportedMetadata"],
            expected_marker
        );
    }

    #[tokio::test]
    async fn force_recomputes_provider_bpm_and_key_but_preserves_manual_values_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("silence.wav");
        let generated = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo:d=3",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&path)
            .output()
            .await
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let settings = Settings::default();
        let provider = Track {
            title: "Analysis fixture".into(),
            artist: "Fixture performer".into(),
            bpm: Some(60.2),
            key: Some("Dm".into()),
            provenance: serde_json::json!({"bpmAnalysis":{"source":"provider"},"keyAnalysis":{"source":"provider"}}),
            ..Track::default()
        };
        let ordinary = analyze_missing_fields(&path, &provider, &settings, false).await;
        assert_eq!(ordinary.bpm, Some(60.2));
        assert_eq!(ordinary.key.as_deref(), Some("Dm"));
        assert_eq!(ordinary.provenance["bpmAnalysis"]["source"], "provider");
        assert_eq!(ordinary.provenance["keyAnalysis"]["source"], "provider");

        let forced = analyze_missing_fields(&path, &provider, &settings, true).await;
        assert_eq!(forced.bpm, None);
        assert_eq!(forced.key, None);
        assert_eq!(
            forced.provenance["bpmAnalysis"]["status"],
            "no confident estimate"
        );
        assert_eq!(
            forced.provenance["keyAnalysis"]["status"],
            "no_confident_estimate"
        );
        tag_media(&path, &provider, None).unwrap();
        tag_media(&path, &forced, None).unwrap();
        let inspected = inspect_local_file(&path, &settings).await.unwrap();
        assert_eq!(inspected.bpm, None);
        assert_eq!(inspected.key, None);

        let manual = Track {
            bpm: Some(133.0),
            key: Some("F#m".into()),
            provenance: serde_json::json!({"manualFields":{"bpm":true,"key":true}}),
            ..Track::default()
        };
        let forced_manual = analyze_missing_fields(&path, &manual, &settings, true).await;
        assert_eq!(forced_manual.bpm, Some(133.0));
        assert_eq!(forced_manual.key.as_deref(), Some("F#m"));

        let manual_blank = Track {
            bpm: None,
            key: Some(String::new()),
            provenance: serde_json::json!({"manualFields":{"bpm":true,"key":true}}),
            ..Track::default()
        };
        let forced_blank = analyze_missing_fields(&path, &manual_blank, &settings, true).await;
        assert_eq!(forced_blank.bpm, None);
        assert_eq!(forced_blank.key.as_deref(), Some(""));
    }
}
