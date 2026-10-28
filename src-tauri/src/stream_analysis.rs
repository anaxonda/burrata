//! Local-only whole-track BPM/key analysis for consumed playback PCM.
//! This module never fetches streams, writes audio tags, or mutates saved tracks.

use crate::models::Track;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub const SCHEMA_VERSION: u32 = 1;
pub const ANALYZER_VERSION: &str = "burrata-whole-track-analysis-v1-bpm-v4-keyfinder-2.2.8";
pub const BPM_PCM_RATE: u32 = 8_000;
pub const KEY_PCM_RATE: u32 = 22_050;
pub const MAX_TRACK_SECONDS: f64 = 900.0;
pub const ROUNDED_DURATION_TOLERANCE_SECONDS: f64 = 1.0;
pub const MAX_CACHE_RECORDS: usize = 128;
pub const STREAM_WAVEFORM_BINS: usize = 512;
pub const STREAM_WAVEFORM_ANALYZER_VERSION: &str = "stream-waveform-peak-512-v1";
pub const ASSET_ANALYSIS_VERSION: &str =
    "burrata-selected-asset-whole-track-v1-bpm-v4-keyfinder-2.2.8";
pub const MAX_ASSET_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Resolver metadata that identifies the selected encoded audio variant.
/// Signed URLs and request headers are deliberately not representable here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamAudioVariant {
    pub format_id: Option<String>,
    pub codec: Option<String>,
    pub mime_type: Option<String>,
    pub bitrate_kbps: Option<f64>,
    pub expected_content_length: Option<u64>,
    pub contains_video: Option<bool>,
}

impl StreamAudioVariant {
    /// Stable variants need the resolver's format identifier and at least one
    /// codec/container discriminator. Length, when known, distinguishes a
    /// replaced encoding; bitrate is excluded because resolver values can be
    /// rounded or estimated.
    pub fn identity_sha256(&self) -> Option<String> {
        let expected_content_length = match self.expected_content_length {
            Some(length) if length > 0 && length <= 9_007_199_254_740_991 => {
                Some(length.to_string())
            }
            Some(_) => return None,
            None => None,
        };
        let format_id = normalized_variant_token(self.format_id.as_deref()?, 96)?;
        let codec = self
            .codec
            .as_deref()
            .and_then(|value| normalized_variant_token(value, 96));
        let mime_type = self
            .mime_type
            .as_deref()
            .and_then(|value| normalized_mime_type(value));
        if codec.is_none() && mime_type.is_none() {
            return None;
        }
        let identity = format!(
            "burrata-stream-audio-variant-v1\nformat={format_id}\ncodec={}\nmime={}\nvideo={}\nlength={}",
            codec.as_deref().unwrap_or(""),
            mime_type.as_deref().unwrap_or(""),
            self.contains_video.map_or("unknown", |value| if value { "yes" } else { "no" }),
            expected_content_length.unwrap_or_default(),
        );
        Some(hex_sha256(identity.as_bytes()))
    }

    pub fn from_resolver_quality(value: &Value) -> Option<Self> {
        let variant = Self {
            format_id: value
                .get("formatId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            codec: value
                .get("codec")
                .and_then(Value::as_str)
                .map(str::to_owned),
            mime_type: value
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_owned),
            bitrate_kbps: value.get("bitrateKbps").and_then(Value::as_f64),
            expected_content_length: value.get("expectedContentLength").and_then(Value::as_u64),
            contains_video: value.get("formatHasVideo").and_then(Value::as_bool),
        };
        variant.identity_sha256().map(|_| variant)
    }
}

fn normalized_variant_token(value: &str, max_len: usize) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > max_len
        || !normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        return None;
    }
    Some(normalized)
}

fn normalized_mime_type(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 128
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+')
        })
        || !normalized.contains('/')
    {
        return None;
    }
    Some(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AnalysisResource {
    RegisteredAsset {
        #[serde(rename = "assetId", alias = "asset_id")]
        asset_id: String,
        #[serde(rename = "recordingId", alias = "recording_id")]
        recording_id: String,
        #[serde(rename = "fileHash", alias = "file_hash")]
        file_hash: String,
    },
    Stream {
        #[serde(rename = "recordingId", alias = "recording_id")]
        recording_id: String,
        provider: String,
        #[serde(rename = "sourceIdentitySha256", alias = "source_identity_sha256")]
        source_identity_sha256: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "audioVariantSha256",
            alias = "audio_variant_sha256"
        )]
        audio_variant_sha256: Option<String>,
    },
}

impl AnalysisResource {
    pub fn recording_id(&self) -> &str {
        match self {
            Self::RegisteredAsset { recording_id, .. } | Self::Stream { recording_id, .. } => {
                recording_id
            }
        }
    }

    /// Raw stream URLs and credentials are not accepted or serialized.
    pub fn cache_identity(&self) -> Result<String, AnalysisError> {
        match self {
            Self::RegisteredAsset {
                asset_id,
                recording_id,
                file_hash,
            } if asset_id.is_empty() || recording_id.is_empty() || !valid_sha256(file_hash) => {
                return Err(AnalysisError::InvalidResource)
            }
            Self::Stream {
                recording_id,
                provider,
                source_identity_sha256,
                audio_variant_sha256,
            } if recording_id.is_empty()
                || !valid_token(provider, 64)
                || !valid_sha256(source_identity_sha256)
                || audio_variant_sha256
                    .as_deref()
                    .is_some_and(|value| !valid_sha256(value)) =>
            {
                return Err(AnalysisError::InvalidResource)
            }
            _ => {}
        }
        let bytes = serde_json::to_vec(self).map_err(|_| AnalysisError::InvalidResource)?;
        Ok(hex_sha256(&bytes))
    }

    pub fn has_stable_audio_variant(&self) -> bool {
        matches!(
            self,
            Self::Stream {
                audio_variant_sha256: Some(_),
                ..
            }
        )
    }

    pub fn with_stream_audio_variant(mut self, variant: Option<&StreamAudioVariant>) -> Self {
        if let Self::Stream {
            audio_variant_sha256,
            ..
        } = &mut self
        {
            *audio_variant_sha256 = variant.and_then(StreamAudioVariant::identity_sha256);
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnalysisMetric {
    Bpm,
    Key,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BpmMeasurement {
    pub bpm: f64,
    /// Normalized onset autocorrelation strength; not a calibrated probability.
    pub confidence: f64,
    /// The detector's near-tied half/double-time alternative is retained.
    pub related_tempo_alternative_bpm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyMeasurement {
    pub key: String,
    pub confidence: f64,
    pub whole_track_key: String,
    pub window_keys: Vec<Option<String>>,
    pub agreement_count: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WholeTrackAnalysis {
    pub schema_version: u32,
    pub analyzer_version: String,
    pub resource: AnalysisResource,
    pub duration_seconds: f64,
    pub scope: String,
    pub coverage: f64,
    pub measured_at: String,
    pub bpm: Option<BpmMeasurement>,
    pub key: Option<KeyMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waveform: Option<StreamWaveform>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamWaveformBin {
    pub min: f32,
    pub max: f32,
}

/// Bounded full-track peak envelope produced alongside BPM/key in the same
/// decode pass. It contains min/max summaries only, never retained PCM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamWaveform {
    pub analyzer_version: String,
    pub bin_count: usize,
    pub duration_seconds: f64,
    pub bins: Vec<StreamWaveformBin>,
    pub scope: String,
    pub complete: bool,
    pub measured_at: String,
}

impl StreamWaveform {
    pub fn validate(&self, analysis_duration_seconds: f64) -> Result<(), AnalysisError> {
        if self.analyzer_version != STREAM_WAVEFORM_ANALYZER_VERSION
            || self.bin_count != STREAM_WAVEFORM_BINS
            || self.bins.len() != STREAM_WAVEFORM_BINS
            || !self.duration_seconds.is_finite()
            || (self.duration_seconds - analysis_duration_seconds).abs()
                > 0.05_f64.max(analysis_duration_seconds * 0.001)
            || self.scope != "wholeTrack"
            || !self.complete
            || self.measured_at.is_empty()
            || self.bins.iter().any(|bin| {
                !bin.min.is_finite()
                    || !bin.max.is_finite()
                    || bin.min < -1.0
                    || bin.max > 1.0
                    || bin.min > bin.max
            })
        {
            return Err(AnalysisError::InvalidMeasurement);
        }
        Ok(())
    }
}

impl WholeTrackAnalysis {
    pub fn validate(&self) -> Result<(), AnalysisError> {
        if self.schema_version != SCHEMA_VERSION
            || self.analyzer_version != ANALYZER_VERSION
            || self.scope != "wholeTrack"
            || !self.duration_seconds.is_finite()
            || !(8.0..=MAX_TRACK_SECONDS).contains(&self.duration_seconds)
            || !self.coverage.is_finite()
            || !(0.99..=1.001).contains(&self.coverage)
            || self.measured_at.is_empty()
            || (self.bpm.is_none() && self.key.is_none() && self.waveform.is_none())
        {
            return Err(AnalysisError::InvalidMeasurement);
        }
        self.resource.cache_identity()?;
        if let Some(bpm) = &self.bpm {
            validate_bpm(bpm)?;
        }
        if let Some(key) = &self.key {
            validate_key(key)?;
        }
        if let Some(waveform) = &self.waveform {
            waveform.validate(self.duration_seconds)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldPolicy {
    FillMissing,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyAnalysisSelection {
    pub fields: BTreeSet<AnalysisMetric>,
    /// Replace is an explicit per-field overwrite choice shown in preview.
    pub policies: BTreeMap<AnalysisMetric, FieldPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    InvalidResource,
    InvalidMeasurement,
    WrongRecording,
    IncompleteCapture,
    DiscontinuousCapture,
    UnsupportedDuration,
    UnsupportedFormat,
    CaptureNotStartedAtTrackBeginning,
    NotRegisteredAsset,
    ResourceChanged,
    FileTooLarge,
    Cancelled,
    DecodeFailed(String),
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidResource => f.write_str("Analysis resource identity is invalid"),
            Self::InvalidMeasurement => f.write_str("Analysis result is invalid"),
            Self::WrongRecording => f.write_str("Analysis belongs to a different recording"),
            Self::IncompleteCapture => f.write_str("Playback did not cover the complete track"),
            Self::DiscontinuousCapture => {
                f.write_str("Playback PCM has a gap, seek, or source change")
            }
            Self::UnsupportedDuration => {
                f.write_str("Whole-track analysis supports 8 second to 15 minute tracks")
            }
            Self::UnsupportedFormat => f.write_str("PCM format is not supported"),
            Self::CaptureNotStartedAtTrackBeginning => {
                f.write_str("Start analysis from the beginning of the track")
            }
            Self::NotRegisteredAsset => {
                f.write_str("Analysis requires a selected registered local asset")
            }
            Self::ResourceChanged => {
                f.write_str("Audio changed during analysis; the result was discarded")
            }
            Self::FileTooLarge => f.write_str("Audio file exceeds the 8 GB analysis limit"),
            Self::Cancelled => f.write_str("Analysis was cancelled"),
            Self::DecodeFailed(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for AnalysisError {}

/// Analyze one explicitly selected registered local asset. This writes only a
/// hash/version-bound analysis cache record; it never edits audio tags or Track
/// metadata. The cancellation watch drops FFmpeg children through kill-on-drop.
pub async fn analyze_registered_asset(
    store: std::sync::Arc<crate::Store>,
    asset_id: &str,
    fields: &BTreeSet<AnalysisMetric>,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<WholeTrackAnalysis, AnalysisError> {
    if fields.is_empty() {
        return Err(AnalysisError::InvalidMeasurement);
    }
    if *cancel.borrow() {
        return Err(AnalysisError::Cancelled);
    }
    let asset = store
        .get_asset_by_id(asset_id)
        .map_err(|e| AnalysisError::DecodeFailed(e.to_string()))?
        .ok_or(AnalysisError::NotRegisteredAsset)?;
    if asset.track.id != asset.recording_id || asset.track.playback_range.is_some() {
        return Err(AnalysisError::InvalidResource);
    }
    let path = verified_asset_path(&asset.path)?;
    if std::fs::metadata(&path)
        .map_err(|_| AnalysisError::NotRegisteredAsset)?
        .len()
        > MAX_ASSET_BYTES
    {
        return Err(AnalysisError::FileTooLarge);
    }
    let duration = asset
        .duration_seconds
        .filter(|value| value.is_finite())
        .ok_or(AnalysisError::UnsupportedDuration)?;
    if !(8.0..=MAX_TRACK_SECONDS).contains(&duration) {
        return Err(AnalysisError::UnsupportedDuration);
    }
    let before = hash_file_cancellable(&path, cancel).await?;
    if before != asset.hash {
        return Err(AnalysisError::ResourceChanged);
    }
    let cached = read_asset_cache(&store, &asset.id, &asset.hash)?;
    let cached_has_requested = cached.as_ref().is_some_and(|record| {
        (!fields.contains(&AnalysisMetric::Bpm) || record.bpm.is_some())
            && (!fields.contains(&AnalysisMetric::Key) || record.key.is_some())
    });
    if cached_has_requested {
        return Ok(cached.expect("checked cached asset analysis"));
    }

    let settings = store
        .settings()
        .map_err(|e| AnalysisError::DecodeFailed(e.to_string()))?;
    let bpm = if fields.contains(&AnalysisMetric::Bpm)
        && cached.as_ref().is_none_or(|record| record.bpm.is_none())
    {
        Some(detect_with_cancel(cancel, crate::media::detect_bpm(&path, &settings)).await?)
    } else {
        None
    };
    let key = if fields.contains(&AnalysisMetric::Key)
        && cached.as_ref().is_none_or(|record| record.key.is_none())
    {
        Some(detect_with_cancel(cancel, crate::media::detect_key(&path, &settings)).await?)
    } else {
        None
    };
    if *cancel.borrow() {
        return Err(AnalysisError::Cancelled);
    }
    let after = hash_file_cancellable(&path, cancel).await?;
    if after != asset.hash || after != before {
        return Err(AnalysisError::ResourceChanged);
    }

    let record = WholeTrackAnalysis {
        schema_version: SCHEMA_VERSION,
        analyzer_version: ANALYZER_VERSION.into(),
        resource: AnalysisResource::RegisteredAsset {
            asset_id: asset.id.clone(),
            recording_id: asset.recording_id.clone(),
            file_hash: asset.hash.clone(),
        },
        duration_seconds: duration,
        scope: "wholeTrack".into(),
        coverage: 1.0,
        measured_at: chrono::Utc::now().to_rfc3339(),
        bpm: bpm
            .flatten()
            .map(|value| BpmMeasurement {
                bpm: value.bpm,
                confidence: value.confidence,
                related_tempo_alternative_bpm: value.related_tempo_bpm,
            })
            .or_else(|| cached.as_ref().and_then(|record| record.bpm.clone())),
        key: key
            .flatten()
            .map(|value| KeyMeasurement {
                key: value.key,
                confidence: value.confidence,
                whole_track_key: value.whole_track_key,
                agreement_count: (value.confidence * 5.0).round() as u8,
                window_keys: value.window_keys,
            })
            .or_else(|| cached.as_ref().and_then(|record| record.key.clone())),
        waveform: None,
    };
    record.validate()?;
    let payload = json!({
        "bpm":record.bpm.as_ref().map(|value| value.bpm),
        "key":record.key.as_ref().map(|value| value.key.clone()),
        "analysis":record,
    });
    store
        .save_analysis_record(
            &asset.id,
            &asset.hash,
            ASSET_ANALYSIS_VERSION,
            "completed",
            &payload,
        )
        .map_err(|e| AnalysisError::DecodeFailed(e.to_string()))?;
    Ok(record)
}

fn verified_asset_path(path: &str) -> Result<PathBuf, AnalysisError> {
    let supplied = Path::new(path);
    if !supplied.is_absolute()
        || std::fs::symlink_metadata(supplied)
            .map_err(|_| AnalysisError::NotRegisteredAsset)?
            .file_type()
            .is_symlink()
    {
        return Err(AnalysisError::NotRegisteredAsset);
    }
    let canonical =
        std::fs::canonicalize(supplied).map_err(|_| AnalysisError::NotRegisteredAsset)?;
    if canonical != supplied || !canonical.is_file() {
        return Err(AnalysisError::NotRegisteredAsset);
    }
    Ok(canonical)
}

fn read_asset_cache(
    store: &crate::Store,
    asset_id: &str,
    file_hash: &str,
) -> Result<Option<WholeTrackAnalysis>, AnalysisError> {
    let rows = store
        .list_analysis_records_for_assets(&[asset_id.to_owned()])
        .map_err(|e| AnalysisError::DecodeFailed(e.to_string()))?;
    for row in rows {
        if row["inputHash"] != file_hash
            || row["version"] != ASSET_ANALYSIS_VERSION
            || row["status"] != "completed"
        {
            continue;
        }
        let Ok(record) =
            serde_json::from_value::<WholeTrackAnalysis>(row["payload"]["analysis"].clone())
        else {
            continue;
        };
        if record.validate().is_err()
            || !matches!(
                &record.resource,
                AnalysisResource::RegisteredAsset {
                    asset_id: cached_id,
                    file_hash: cached_hash,
                    ..
                } if cached_id == asset_id && cached_hash == file_hash
            )
        {
            continue;
        }
        return Ok(Some(record));
    }
    Ok(None)
}

async fn detect_with_cancel<T>(
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    future: impl std::future::Future<Output = Result<Option<T>, String>>,
) -> Result<Option<T>, AnalysisError> {
    if *cancel.borrow() {
        return Err(AnalysisError::Cancelled);
    }
    tokio::select! {
        result = future => result.map_err(AnalysisError::DecodeFailed),
        _ = cancel.changed() => Err(AnalysisError::Cancelled),
    }
}

async fn hash_file_cancellable(
    path: &Path,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<String, AnalysisError> {
    let path = path.to_path_buf();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    tokio::select! {
        result = tokio::task::spawn_blocking(move || hash_file(&path, &worker_cancelled)) => {
            result.map_err(|error| AnalysisError::DecodeFailed(error.to_string()))?
        }
        _ = cancel.changed() => {
            cancelled.store(true, Ordering::Release);
            Err(AnalysisError::Cancelled)
        },
    }
}

fn hash_file(path: &Path, cancelled: &AtomicBool) -> Result<String, AnalysisError> {
    let mut file = File::open(path).map_err(|_| AnalysisError::NotRegisteredAsset)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(AnalysisError::Cancelled);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|_| AnalysisError::NotRegisteredAsset)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Resampling by temporal area averaging. This bounds a 15-minute stereo stream
/// to at most 27 million mono float samples across the 8 kHz and 22.05 kHz lanes.
#[derive(Debug, Clone)]
pub struct MonoDownsampler {
    source_rate: u32,
    target_rate: u32,
    channels: u16,
    output_time_units: u64,
    weighted_sum: f64,
    output: Vec<f32>,
}

impl MonoDownsampler {
    pub fn new(source_rate: u32, target_rate: u32, channels: u16) -> Result<Self, AnalysisError> {
        if source_rate == 0 || target_rate == 0 || channels == 0 || channels > 32 {
            return Err(AnalysisError::UnsupportedFormat);
        }
        Ok(Self {
            source_rate,
            target_rate,
            channels,
            output_time_units: 0,
            weighted_sum: 0.0,
            output: Vec::new(),
        })
    }

    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<(), AnalysisError> {
        let channels = usize::from(self.channels);
        if samples.len() % channels != 0 {
            return Err(AnalysisError::UnsupportedFormat);
        }
        for frame in samples.chunks_exact(channels) {
            let mono = frame
                .iter()
                .map(|sample| {
                    if sample.is_finite() {
                        f64::from(*sample).clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .sum::<f64>()
                / channels as f64;
            let mut input_remaining = u64::from(self.target_rate);
            while input_remaining > 0 {
                let output_remaining = u64::from(self.source_rate) - self.output_time_units;
                let overlap = input_remaining.min(output_remaining);
                self.weighted_sum += mono * overlap as f64;
                self.output_time_units += overlap;
                input_remaining -= overlap;
                if self.output_time_units == u64::from(self.source_rate) {
                    self.output
                        .push((self.weighted_sum / f64::from(self.source_rate)) as f32);
                    self.output_time_units = 0;
                    self.weighted_sum = 0.0;
                }
            }
        }
        Ok(())
    }

    pub fn output(&self) -> &[f32] {
        &self.output
    }
}

#[derive(Debug, Clone)]
pub struct PcmBatch<'a> {
    pub generation: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub end_position_seconds: f64,
    pub interleaved: &'a [f32],
}

/// Playback observations are explicitly window-scoped until natural end proves
/// continuous full-track coverage. They are progress evidence only and never a
/// cacheable BPM/key measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackWindowProgress {
    pub scope: String,
    pub generation: u64,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub expected_duration_seconds: f64,
    pub continuous_so_far: bool,
    pub cacheable: bool,
}

/// Captures only audio already consumed by playback. The caller invokes finish
/// only at natural end-of-track; seeks, queue changes, and dropped batches fail
/// continuity checks and cannot produce a whole-track result.
#[derive(Debug)]
pub struct WholeTrackCapture {
    generation: u64,
    resource: AnalysisResource,
    expected_duration: f64,
    duration_precision: StreamDurationPrecision,
    first_position: Option<f64>,
    last_position: Option<f64>,
    format: Option<(u32, u16)>,
    key_pcm: Option<MonoDownsampler>,
    bpm_pcm: Option<MonoDownsampler>,
    waveform_bins: Option<Vec<Option<StreamWaveformBin>>>,
    invalid: Option<AnalysisError>,
}

/// Duration provenance used only by resolver-backed whole-recording analysis.
/// Exact active-media durations stay strict; integer-second source metadata may
/// differ from decoded EOF by less than one second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDurationPrecision {
    Exact,
    RoundedToSecond,
}

impl StreamDurationPrecision {
    pub fn accepts(self, expected: f64, actual: f64) -> bool {
        if !expected.is_finite() || !actual.is_finite() {
            return false;
        }
        match self {
            Self::Exact => (actual - expected).abs() <= 0.05,
            Self::RoundedToSecond => {
                is_whole_second(expected)
                    && (actual - expected).abs() < ROUNDED_DURATION_TOLERANCE_SECONDS
            }
        }
    }

    pub fn maximum_duration(self, expected: f64) -> f64 {
        let tolerance = match self {
            Self::Exact => 0.05_f64,
            Self::RoundedToSecond if is_whole_second(expected) => {
                ROUNDED_DURATION_TOLERANCE_SECONDS
            }
            Self::RoundedToSecond => 0.0,
        };
        (expected + tolerance).min(MAX_TRACK_SECONDS)
    }
}

fn is_whole_second(value: f64) -> bool {
    value.is_finite() && (value - value.round()).abs() < 1e-6
}

impl WholeTrackCapture {
    pub fn new(
        generation: u64,
        resource: AnalysisResource,
        duration: f64,
        initial_position: f64,
    ) -> Result<Self, AnalysisError> {
        Self::new_inner(
            generation,
            resource,
            duration,
            initial_position,
            false,
            StreamDurationPrecision::Exact,
        )
    }

    /// Create a full-track capture that also summarizes a fixed-size waveform
    /// while PCM is already flowing through BPM/key analysis.
    pub fn new_with_waveform(
        generation: u64,
        resource: AnalysisResource,
        duration: f64,
        initial_position: f64,
    ) -> Result<Self, AnalysisError> {
        Self::new_inner(
            generation,
            resource,
            duration,
            initial_position,
            true,
            StreamDurationPrecision::Exact,
        )
    }

    pub fn new_with_rounded_duration(
        generation: u64,
        resource: AnalysisResource,
        duration: f64,
        initial_position: f64,
        include_waveform: bool,
    ) -> Result<Self, AnalysisError> {
        Self::new_inner(
            generation,
            resource,
            duration,
            initial_position,
            include_waveform,
            StreamDurationPrecision::RoundedToSecond,
        )
    }

    fn new_inner(
        generation: u64,
        resource: AnalysisResource,
        duration: f64,
        initial_position: f64,
        include_waveform: bool,
        duration_precision: StreamDurationPrecision,
    ) -> Result<Self, AnalysisError> {
        resource.cache_identity()?;
        if !duration.is_finite() || !(8.0..=MAX_TRACK_SECONDS).contains(&duration) {
            return Err(AnalysisError::UnsupportedDuration);
        }
        if duration_precision == StreamDurationPrecision::RoundedToSecond
            && !is_whole_second(duration)
        {
            return Err(AnalysisError::UnsupportedDuration);
        }
        if !initial_position.is_finite() || initial_position > 0.05 {
            return Err(AnalysisError::CaptureNotStartedAtTrackBeginning);
        }
        Ok(Self {
            generation,
            resource,
            expected_duration: duration,
            duration_precision,
            first_position: None,
            last_position: None,
            format: None,
            key_pcm: None,
            bpm_pcm: None,
            waveform_bins: include_waveform.then(|| vec![None; STREAM_WAVEFORM_BINS]),
            invalid: None,
        })
    }

    pub fn push(&mut self, batch: PcmBatch<'_>) -> Result<(), AnalysisError> {
        if let Some(error) = &self.invalid {
            return Err(error.clone());
        }
        if batch.generation != self.generation
            || batch.sample_rate == 0
            || batch.channels == 0
            || batch.channels > 32
            || batch.interleaved.len() % usize::from(batch.channels) != 0
            || !batch.end_position_seconds.is_finite()
        {
            self.invalid = Some(AnalysisError::UnsupportedFormat);
            return Err(AnalysisError::UnsupportedFormat);
        }
        if self
            .format
            .is_some_and(|format| format != (batch.sample_rate, batch.channels))
        {
            self.invalid = Some(AnalysisError::DiscontinuousCapture);
            return Err(AnalysisError::DiscontinuousCapture);
        }
        let frames = batch.interleaved.len() / usize::from(batch.channels);
        let seconds = frames as f64 / f64::from(batch.sample_rate);
        let start = batch.end_position_seconds - seconds;
        let allowed_end = self
            .duration_precision
            .maximum_duration(self.expected_duration);
        if start < -0.05 || start > allowed_end + 0.05 {
            self.invalid = Some(AnalysisError::DiscontinuousCapture);
            return Err(AnalysisError::DiscontinuousCapture);
        }
        if let Some(previous) = self.last_position {
            if (start - previous).abs() > (2.0 / f64::from(batch.sample_rate)).max(0.004) {
                self.invalid = Some(AnalysisError::DiscontinuousCapture);
                return Err(AnalysisError::DiscontinuousCapture);
            }
        } else if start > 0.05 {
            self.invalid = Some(AnalysisError::CaptureNotStartedAtTrackBeginning);
            return Err(AnalysisError::CaptureNotStartedAtTrackBeginning);
        }
        if self.format.is_none() {
            self.format = Some((batch.sample_rate, batch.channels));
            self.key_pcm = Some(MonoDownsampler::new(
                batch.sample_rate,
                KEY_PCM_RATE,
                batch.channels,
            )?);
            self.bpm_pcm = Some(MonoDownsampler::new(
                batch.sample_rate,
                BPM_PCM_RATE,
                batch.channels,
            )?);
        }
        self.key_pcm
            .as_mut()
            .unwrap()
            .push_interleaved(batch.interleaved)?;
        self.bpm_pcm
            .as_mut()
            .unwrap()
            .push_interleaved(batch.interleaved)?;
        if let Some(bins) = self.waveform_bins.as_mut() {
            let channels = usize::from(batch.channels);
            for (frame_index, frame) in batch.interleaved.chunks_exact(channels).enumerate() {
                let position = (start.max(0.0) + frame_index as f64 / f64::from(batch.sample_rate))
                    .min(self.expected_duration);
                let bin_index = ((position / self.expected_duration) * STREAM_WAVEFORM_BINS as f64)
                    .floor() as usize;
                let bin = &mut bins[bin_index.min(STREAM_WAVEFORM_BINS - 1)];
                for sample in frame {
                    let sample = if sample.is_finite() {
                        sample.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    };
                    match bin {
                        Some(existing) => {
                            existing.min = existing.min.min(sample);
                            existing.max = existing.max.max(sample);
                        }
                        None => {
                            *bin = Some(StreamWaveformBin {
                                min: sample,
                                max: sample,
                            });
                        }
                    }
                }
            }
        }
        self.first_position.get_or_insert(start.max(0.0));
        self.last_position = Some(batch.end_position_seconds);
        Ok(())
    }

    /// Mark any tap-reported gap, overload, seek, or generation discontinuity.
    pub fn invalidate(&mut self) {
        self.invalid = Some(AnalysisError::DiscontinuousCapture);
    }

    pub fn progress(&self) -> PlaybackWindowProgress {
        PlaybackWindowProgress {
            scope: "playbackWindow".into(),
            generation: self.generation,
            start_seconds: self.first_position,
            end_seconds: self.last_position,
            expected_duration_seconds: self.expected_duration,
            continuous_so_far: self.invalid.is_none(),
            cacheable: false,
        }
    }

    pub fn finish(
        self,
        natural_end_position: f64,
        measured_at: String,
    ) -> Result<WholeTrackAnalysis, AnalysisError> {
        if self.duration_precision != StreamDurationPrecision::Exact {
            return Err(AnalysisError::UnsupportedDuration);
        }
        self.finish_inner(natural_end_position, measured_at, false)
    }

    pub fn finish_with_rounded_duration(
        self,
        natural_end_position: f64,
        measured_at: String,
    ) -> Result<WholeTrackAnalysis, AnalysisError> {
        if self.duration_precision != StreamDurationPrecision::RoundedToSecond {
            return Err(AnalysisError::UnsupportedDuration);
        }
        self.finish_inner(natural_end_position, measured_at, true)
    }

    fn finish_inner(
        self,
        natural_end_position: f64,
        measured_at: String,
        use_measured_duration: bool,
    ) -> Result<WholeTrackAnalysis, AnalysisError> {
        if let Some(error) = self.invalid {
            return Err(error);
        }
        let start = self
            .first_position
            .ok_or(AnalysisError::IncompleteCapture)?;
        let end = self.last_position.ok_or(AnalysisError::IncompleteCapture)?;
        let endpoint_is_valid = if use_measured_duration {
            self.duration_precision
                .accepts(self.expected_duration, natural_end_position)
                && (end - natural_end_position).abs() <= 0.05_f64.max(natural_end_position * 0.001)
        } else {
            let tolerance = 0.05_f64.max(self.expected_duration * 0.001);
            (natural_end_position - self.expected_duration).abs() <= tolerance
                && (end - self.expected_duration).abs() <= tolerance
        };
        if !natural_end_position.is_finite() || !endpoint_is_valid || start > 0.05 {
            return Err(AnalysisError::IncompleteCapture);
        }
        let key_pcm = self.key_pcm.ok_or(AnalysisError::IncompleteCapture)?;
        let bpm_pcm = self.bpm_pcm.ok_or(AnalysisError::IncompleteCapture)?.output;
        let analysis_duration = if use_measured_duration {
            natural_end_position
        } else {
            self.expected_duration
        };
        let (bpm, key) = crate::media::analyze_resampled_mono_pcm(&bpm_pcm, &key_pcm.output);
        let waveform = self
            .waveform_bins
            .map(|bins| -> Result<StreamWaveform, AnalysisError> {
                let bins = if use_measured_duration {
                    waveform_bins_from_mono_pcm(&key_pcm.output, analysis_duration)?
                } else {
                    bins.into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or(AnalysisError::IncompleteCapture)?
                };
                Ok(StreamWaveform {
                    analyzer_version: STREAM_WAVEFORM_ANALYZER_VERSION.into(),
                    bin_count: STREAM_WAVEFORM_BINS,
                    duration_seconds: analysis_duration,
                    bins,
                    scope: "wholeTrack".into(),
                    complete: true,
                    measured_at: measured_at.clone(),
                })
            })
            .transpose()?;
        let record = WholeTrackAnalysis {
            schema_version: SCHEMA_VERSION,
            analyzer_version: ANALYZER_VERSION.into(),
            resource: self.resource,
            duration_seconds: analysis_duration,
            scope: "wholeTrack".into(),
            coverage: (end - start) / analysis_duration,
            measured_at,
            bpm: bpm.map(|value| BpmMeasurement {
                bpm: value.bpm,
                confidence: value.confidence,
                related_tempo_alternative_bpm: value.related_tempo_bpm,
            }),
            key: key.map(|value| KeyMeasurement {
                key: value.key,
                confidence: value.confidence,
                whole_track_key: value.whole_track_key,
                agreement_count: (value.confidence * 5.0).round() as u8,
                window_keys: value.window_keys,
            }),
            waveform,
        };
        record.validate()?;
        Ok(record)
    }
}

fn waveform_bins_from_mono_pcm(
    samples: &[f32],
    duration_seconds: f64,
) -> Result<Vec<StreamWaveformBin>, AnalysisError> {
    if samples.is_empty()
        || !duration_seconds.is_finite()
        || !(8.0..=MAX_TRACK_SECONDS).contains(&duration_seconds)
    {
        return Err(AnalysisError::IncompleteCapture);
    }
    let mut bins: Vec<Option<StreamWaveformBin>> = vec![None; STREAM_WAVEFORM_BINS];
    for (sample_index, sample) in samples.iter().copied().enumerate() {
        let position = (sample_index as f64 / f64::from(KEY_PCM_RATE)).min(duration_seconds);
        let bin_index =
            ((position / duration_seconds) * STREAM_WAVEFORM_BINS as f64).floor() as usize;
        let slot = &mut bins[bin_index.min(STREAM_WAVEFORM_BINS - 1)];
        let value = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        match slot {
            Some(existing) => {
                existing.min = existing.min.min(value);
                existing.max = existing.max.max(value);
            }
            None => {
                *slot = Some(StreamWaveformBin {
                    min: value,
                    max: value,
                });
            }
        }
    }
    bins.into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(AnalysisError::IncompleteCapture)
}

/// Apply only reviewed fields. FillMissing protects all non-null imported or
/// manual values. Replace changes only the field explicitly selected by the
/// user and keeps a bounded copy of the replaced value in provenance.
pub fn apply_analysis(
    track: &Track,
    analysis: &WholeTrackAnalysis,
    selection: &ApplyAnalysisSelection,
) -> Result<Track, AnalysisError> {
    analysis.validate()?;
    if track.id != analysis.resource.recording_id() {
        return Err(AnalysisError::WrongRecording);
    }
    let mut next = track.clone();
    if !next.provenance.is_object() {
        next.provenance = json!({});
    }
    let source = match &analysis.resource {
        AnalysisResource::RegisteredAsset { .. } => "decodedFilePcm",
        AnalysisResource::Stream { .. } => "decodedPlaybackPcm",
    };
    for metric in &selection.fields {
        let policy = selection
            .policies
            .get(metric)
            .copied()
            .unwrap_or(FieldPolicy::FillMissing);
        match metric {
            AnalysisMetric::Bpm => {
                let Some(value) = &analysis.bpm else { continue };
                let current = next.bpm;
                let manual = next.provenance["manualFields"]["bpm"] == true;
                if (current.is_some() || manual) && policy != FieldPolicy::Replace {
                    continue;
                }
                if policy == FieldPolicy::Replace && (current.is_some() || manual) {
                    preserve_replaced_value(&mut next, "bpm", current.map(Value::from));
                }
                next.bpm = Some(value.bpm);
                next.provenance["bpmAnalysis"] = json!({
                    "source":source, "resourceIdentity":analysis.resource.cache_identity().map_err(|_| AnalysisError::InvalidResource)?,
                    "scope":"wholeTrack", "durationSeconds":analysis.duration_seconds, "coverage":analysis.coverage,
                    "analyzerVersion":analysis.analyzer_version, "measuredAt":analysis.measured_at,
                    "confidence":value.confidence, "confidenceMeaning":"normalized onset autocorrelation; not calibrated",
                    "relatedTempoAlternativeBpm":value.related_tempo_alternative_bpm
                });
            }
            AnalysisMetric::Key => {
                let Some(value) = &analysis.key else { continue };
                let current = next.key.clone();
                let manual = next.provenance["manualFields"]["key"] == true;
                if (current.is_some() || manual) && policy != FieldPolicy::Replace {
                    continue;
                }
                if policy == FieldPolicy::Replace && (current.is_some() || manual) {
                    preserve_replaced_value(&mut next, "key", current.map(Value::String));
                }
                next.key = Some(value.key.clone());
                next.provenance["keyAnalysis"] = json!({
                    "source":source, "resourceIdentity":analysis.resource.cache_identity().map_err(|_| AnalysisError::InvalidResource)?,
                    "scope":"wholeTrack", "durationSeconds":analysis.duration_seconds, "coverage":analysis.coverage,
                    "analyzerVersion":analysis.analyzer_version, "measuredAt":analysis.measured_at,
                    "confidence":value.confidence, "confidenceMeaning":"fraction of five windows agreeing with whole-track classifier; not calibrated",
                    "wholeTrackKey":value.whole_track_key, "windowKeys":value.window_keys, "agreementCount":value.agreement_count
                });
            }
        }
    }
    Ok(next)
}

fn preserve_replaced_value(track: &mut Track, field: &str, value: Option<Value>) {
    if !track.provenance["analysisReplacements"].is_array() {
        track.provenance["analysisReplacements"] = json!([]);
    }
    let replacements = track.provenance["analysisReplacements"]
        .as_array_mut()
        .unwrap();
    replacements
        .push(json!({"field":field,"previousValue":value,"source":"explicitUserOverwrite"}));
    if replacements.len() > 32 {
        replacements.remove(0);
    }
    if let Some(manual) = track
        .provenance
        .get_mut("manualFields")
        .and_then(Value::as_object_mut)
    {
        manual.remove(field);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamAnalysisCache {
    pub schema_version: u32,
    pub records: Vec<WholeTrackAnalysis>,
}

/// Completed stream results whose encoded variant is unknown may be shown to
/// the current playback session, but must never enter the persistent cache.
/// This deliberately has no serde implementation and is keyed by both the
/// playback generation and the exact URL-free stream resource identity.
pub const MAX_SESSION_ONLY_STREAM_ANALYSES: usize = 8;

#[derive(Debug, Default)]
pub struct SessionOnlyStreamAnalysisResults {
    entries: VecDeque<SessionOnlyStreamAnalysisEntry>,
}

#[derive(Debug, Clone)]
struct SessionOnlyStreamAnalysisEntry {
    generation: u64,
    resource_identity: String,
    analysis: WholeTrackAnalysis,
}

impl SessionOnlyStreamAnalysisResults {
    /// Retain one completed full-recording result in memory for its owning
    /// playback generation. Stable-variant results belong in
    /// `StreamAnalysisCache`; excerpts, live/radio resources, and partial
    /// observations are rejected here.
    pub fn insert(
        &mut self,
        generation: u64,
        analysis: WholeTrackAnalysis,
    ) -> Result<(), AnalysisError> {
        if generation == 0 {
            return Err(AnalysisError::InvalidResource);
        }
        analysis.validate()?;
        let AnalysisResource::Stream {
            provider,
            audio_variant_sha256: None,
            ..
        } = &analysis.resource
        else {
            return Err(AnalysisError::InvalidResource);
        };
        if !matches!(
            provider.as_str(),
            "youtube" | "soundcloud" | "bandcamp" | "monochrome"
        ) {
            return Err(AnalysisError::InvalidResource);
        }
        let resource_identity = analysis.resource.cache_identity()?;
        self.entries.retain(|entry| {
            entry.generation != generation || entry.resource_identity != resource_identity
        });
        while self.entries.len() >= MAX_SESSION_ONLY_STREAM_ANALYSES {
            self.entries.pop_front();
        }
        self.entries.push_back(SessionOnlyStreamAnalysisEntry {
            generation,
            resource_identity,
            analysis,
        });
        Ok(())
    }

    /// Read a result only for the exact current generation and source resource.
    /// The caller must still compare the generation with its active playback
    /// owner before returning this cloned DTO to the UI.
    pub fn get(&self, generation: u64, resource: &AnalysisResource) -> Option<WholeTrackAnalysis> {
        if generation == 0 || resource.has_stable_audio_variant() {
            return None;
        }
        let resource_identity = resource.cache_identity().ok()?;
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.generation == generation && entry.resource_identity == resource_identity
            })
            .map(|entry| entry.analysis.clone())
    }

    /// Drop one generation when playback is cancelled or its source is retired.
    pub fn remove_generation(&mut self, generation: u64) {
        self.entries.retain(|entry| entry.generation != generation);
    }

    /// Bound lifetime across monotonic playback generations. Callers may keep
    /// a just-ended gapless generation while it is still the published owner;
    /// otherwise this removes every older session result.
    pub fn remove_older_than(&mut self, generation: u64) {
        self.entries.retain(|entry| entry.generation >= generation);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl StreamAnalysisCache {
    pub fn insert(&mut self, mut record: WholeTrackAnalysis) -> Result<(), AnalysisError> {
        record.validate()?;
        if matches!(&record.resource, AnalysisResource::Stream { .. })
            && !record.resource.has_stable_audio_variant()
        {
            return Err(AnalysisError::InvalidResource);
        }
        let identity = record.resource.cache_identity()?;
        if record.waveform.is_none() {
            let tolerance = 0.05_f64.max(record.duration_seconds * 0.001);
            if let Some(existing) = self.records.iter().find(|existing| {
                existing.analyzer_version == record.analyzer_version
                    && existing.resource.cache_identity().ok().as_deref() == Some(identity.as_str())
                    && (existing.duration_seconds - record.duration_seconds).abs() <= tolerance
                    && existing.waveform.as_ref().is_some_and(|waveform| {
                        waveform.validate(existing.duration_seconds).is_ok()
                    })
            }) {
                record.waveform = existing.waveform.clone();
            }
        }
        record.validate()?;
        self.records.retain(|existing| {
            existing.resource.cache_identity().ok().as_deref() != Some(identity.as_str())
                || existing.analyzer_version != record.analyzer_version
        });
        self.records.push(record);
        self.records
            .sort_by(|left, right| right.measured_at.cmp(&left.measured_at));
        self.records.truncate(MAX_CACHE_RECORDS);
        self.schema_version = SCHEMA_VERSION;
        Ok(())
    }

    pub fn get(&self, resource: &AnalysisResource) -> Option<&WholeTrackAnalysis> {
        if matches!(resource, AnalysisResource::Stream { .. })
            && !resource.has_stable_audio_variant()
        {
            return None;
        }
        let identity = resource.cache_identity().ok()?;
        self.records.iter().find(|record| {
            record.analyzer_version == ANALYZER_VERSION
                && record.resource.cache_identity().ok().as_deref() == Some(identity.as_str())
                && record.validate().is_ok()
        })
    }

    pub fn validate(&self) -> Result<(), AnalysisError> {
        if self.schema_version != SCHEMA_VERSION || self.records.len() > MAX_CACHE_RECORDS {
            return Err(AnalysisError::InvalidMeasurement);
        }
        for record in &self.records {
            record.validate()?;
        }
        Ok(())
    }
}

/// Stable source ID hash. Callers pass a durable provider recording ID, never
/// a media URL, headers, cookies, or other signed resource data.
pub fn stream_source_identity_digest(
    provider: &str,
    provider_track_id: &str,
) -> Result<String, AnalysisError> {
    if !valid_token(provider, 64)
        || provider_track_id.trim().is_empty()
        || provider_track_id.len() > 512
    {
        return Err(AnalysisError::InvalidResource);
    }
    Ok(hex_sha256(
        format!("{provider}\n{}", provider_track_id.trim()).as_bytes(),
    ))
}

fn validate_bpm(value: &BpmMeasurement) -> Result<(), AnalysisError> {
    if !value.bpm.is_finite()
        || !(60.0..=200.0).contains(&value.bpm)
        || !value.confidence.is_finite()
        || !(0.0..=1.0).contains(&value.confidence)
        || value
            .related_tempo_alternative_bpm
            .is_some_and(|bpm| !bpm.is_finite() || !(60.0..=200.0).contains(&bpm))
    {
        return Err(AnalysisError::InvalidMeasurement);
    }
    Ok(())
}

fn validate_key(value: &KeyMeasurement) -> Result<(), AnalysisError> {
    const KEYS: [&str; 24] = [
        "A", "Am", "Bb", "Bbm", "B", "Bm", "C", "Cm", "Db", "Dbm", "D", "Dm", "Eb", "Ebm", "E",
        "Em", "F", "Fm", "Gb", "Gbm", "G", "Gm", "Ab", "Abm",
    ];
    if !KEYS.contains(&value.key.as_str())
        || !KEYS.contains(&value.whole_track_key.as_str())
        || value.window_keys.len() != 5
        || value
            .window_keys
            .iter()
            .flatten()
            .any(|key| !KEYS.contains(&key.as_str()))
        || !value.confidence.is_finite()
        || !(0.6..=1.0).contains(&value.confidence)
        || !(3..=5).contains(&value.agreement_count)
        || (f64::from(value.agreement_count) / 5.0 - value.confidence).abs() > 0.001
    {
        return Err(AnalysisError::InvalidMeasurement);
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn valid_token(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
}
fn hex_sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource() -> AnalysisResource {
        AnalysisResource::Stream {
            recording_id: "catalog:recording-1".into(),
            provider: "youtube".into(),
            source_identity_sha256: "a".repeat(64),
            audio_variant_sha256: Some("c".repeat(64)),
        }
    }

    fn result() -> WholeTrackAnalysis {
        WholeTrackAnalysis {
            schema_version: SCHEMA_VERSION,
            analyzer_version: ANALYZER_VERSION.into(),
            resource: resource(),
            duration_seconds: 180.0,
            scope: "wholeTrack".into(),
            coverage: 1.0,
            measured_at: "2026-10-25T12:00:00Z".into(),
            bpm: Some(BpmMeasurement {
                bpm: 120.0,
                confidence: 0.85,
                related_tempo_alternative_bpm: Some(60.0),
            }),
            key: Some(KeyMeasurement {
                key: "Am".into(),
                confidence: 0.8,
                whole_track_key: "Am".into(),
                window_keys: vec![
                    Some("Am".into()),
                    Some("Am".into()),
                    Some("Am".into()),
                    Some("Am".into()),
                    None,
                ],
                agreement_count: 4,
            }),
            waveform: None,
        }
    }

    #[test]
    fn resampler_preserves_duration_and_constant_signal_across_chunks() {
        let mut resampler = MonoDownsampler::new(48_000, 22_050, 2).unwrap();
        for _ in 0..10 {
            resampler.push_interleaved(&vec![0.25; 9_600]).unwrap();
        }
        assert_eq!(resampler.output().len(), 22_050);
        assert!(resampler
            .output()
            .iter()
            .all(|value| (*value - 0.25).abs() < 1e-6));
    }

    #[test]
    fn real_existing_bpm_estimator_runs_on_bounded_pcm_lane() {
        let sample_rate = BPM_PCM_RATE as usize;
        let period = sample_rate / 2;
        let pulse_samples = (0..sample_rate * 32)
            .map(|index| {
                let within_beat = index % period;
                if within_beat < sample_rate / 12 {
                    let time = index as f64 / sample_rate as f64;
                    ((2.0 * std::f64::consts::PI * 80.0 * time).sin()
                        * (-(within_beat as f64) / (sample_rate as f64 * 0.04)).exp())
                        as f32
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let (bpm, _) = crate::media::analyze_resampled_mono_pcm(&pulse_samples, &[]);
        let bpm = bpm.expect("regular pulse PCM should produce a whole-track estimate");
        assert!((bpm.bpm - 120.0).abs() <= 1.0, "{bpm:?}");
        assert!(bpm.confidence >= 0.9, "{bpm:?}");
        assert!((bpm.related_tempo_bpm.unwrap_or_default() - 60.0).abs() <= 1.0);
    }

    #[test]
    fn single_capture_produces_a_complete_bounded_waveform_with_bpm_key_record() {
        let sample_rate = BPM_PCM_RATE as usize;
        let period = sample_rate / 2;
        let samples = (0..sample_rate * 32)
            .map(|index| {
                let within_beat = index % period;
                if within_beat < sample_rate / 12 {
                    let time = index as f64 / sample_rate as f64;
                    ((2.0 * std::f64::consts::PI * 80.0 * time).sin()
                        * (-(within_beat as f64) / (sample_rate as f64 * 0.04)).exp())
                        as f32
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let mut capture = WholeTrackCapture::new_with_waveform(4, resource(), 32.0, 0.0).unwrap();
        capture
            .push(PcmBatch {
                generation: 4,
                sample_rate: BPM_PCM_RATE,
                channels: 1,
                end_position_seconds: 32.0,
                interleaved: &samples,
            })
            .unwrap();
        let record = capture.finish(32.0, "2026-10-25T12:00:00Z".into()).unwrap();
        let bpm = record
            .bpm
            .as_ref()
            .expect("known 120 BPM pulse should survive the stream capture pipeline");
        assert!((bpm.bpm - 120.0).abs() <= 1.0, "{bpm:?}");
        let waveform = record
            .waveform
            .as_ref()
            .expect("full-media capture has a waveform");
        assert_eq!(waveform.bin_count, STREAM_WAVEFORM_BINS);
        assert_eq!(waveform.bins.len(), STREAM_WAVEFORM_BINS);
        assert!(waveform.bins.iter().all(|bin| bin.min <= bin.max));
        record.validate().unwrap();
    }

    #[test]
    fn chunked_stereo_capture_recovers_known_a_minor_and_complete_waveform() {
        let sample_rate = 48_000_usize;
        let channels = 2_usize;
        let chunk_frames = sample_rate * 2;
        let chord_frequencies = [220.0, 261.6256, 329.6276];
        let mut capture = WholeTrackCapture::new_with_waveform(5, resource(), 20.0, 0.0).unwrap();

        for chunk in 0..10 {
            let chunk_start_frame = chunk * chunk_frames;
            let mut interleaved = Vec::with_capacity(chunk_frames * channels);
            for frame in 0..chunk_frames {
                let absolute_frame = chunk_start_frame + frame;
                let time = absolute_frame as f64 / sample_rate as f64;
                let sample = (chord_frequencies
                    .iter()
                    .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
                    .sum::<f64>()
                    / chord_frequencies.len() as f64) as f32;
                interleaved.extend_from_slice(&[sample, sample]);
            }
            let end_position_seconds = (chunk + 1) as f64 * 2.0;
            capture
                .push(PcmBatch {
                    generation: 5,
                    sample_rate: sample_rate as u32,
                    channels: channels as u16,
                    end_position_seconds,
                    interleaved: &interleaved,
                })
                .unwrap();
        }

        let record = capture.finish(20.0, "2026-10-27T00:00:00Z".into()).unwrap();
        let key = record
            .key
            .as_ref()
            .expect("known sustained A-minor chord should survive stereo chunked capture");
        assert_eq!(key.key, "Am", "{key:?}");
        assert_eq!(key.whole_track_key, "Am", "{key:?}");
        assert!(
            key.window_keys
                .iter()
                .flatten()
                .filter(|value| value.as_str() == "Am")
                .count()
                >= 3,
            "key consensus requires at least three matching windows: {key:?}"
        );
        let waveform = record.waveform.as_ref().expect("full capture has waveform");
        assert!(waveform.complete);
        assert_eq!(waveform.bin_count, STREAM_WAVEFORM_BINS);
        assert!(waveform.bins.iter().all(|bin| bin.min <= bin.max));
        record.validate().unwrap();
    }

    #[test]
    fn complete_waveform_survives_when_audio_has_no_confident_bpm_or_key() {
        let samples = vec![0.0; KEY_PCM_RATE as usize * 8];
        let mut capture = WholeTrackCapture::new_with_waveform(4, resource(), 8.0, 0.0).unwrap();
        capture
            .push(PcmBatch {
                generation: 4,
                sample_rate: KEY_PCM_RATE,
                channels: 1,
                end_position_seconds: 8.0,
                interleaved: &samples,
            })
            .unwrap();
        let record = capture.finish(8.0, "2026-10-26T00:00:00Z".into()).unwrap();
        assert!(record.bpm.is_none());
        assert!(record.key.is_none());
        assert!(record.waveform.as_ref().unwrap().complete);
        record.validate().unwrap();
        let mut no_measurements = record;
        no_measurements.waveform = None;
        assert_eq!(
            no_measurements.validate(),
            Err(AnalysisError::InvalidMeasurement)
        );
    }

    #[test]
    fn source_identity_digest_is_stable_and_does_not_take_a_url() {
        let one = stream_source_identity_digest("youtube", "video-1").unwrap();
        let two = stream_source_identity_digest("youtube", "video-2").unwrap();
        assert_ne!(one, two);
        assert_eq!(one.len(), 64);
        assert!(stream_source_identity_digest("youtube", " ").is_err());
    }

    #[test]
    fn stream_audio_variant_digest_is_stable_across_signed_refresh_metadata() {
        let first = StreamAudioVariant {
            format_id: Some("251".into()),
            codec: Some("OPUS".into()),
            mime_type: Some("Audio/WebM".into()),
            bitrate_kbps: Some(128.0),
            expected_content_length: Some(12_000_000),
            contains_video: Some(false),
        };
        let refreshed = StreamAudioVariant {
            bitrate_kbps: Some(127.5),
            expected_content_length: Some(12_000_000),
            ..first.clone()
        };
        assert_eq!(first.identity_sha256(), refreshed.identity_sha256());
        let changed_length = StreamAudioVariant {
            expected_content_length: Some(11_999_200),
            ..first.clone()
        };
        assert_ne!(first.identity_sha256(), changed_length.identity_sha256());

        let changed_format = StreamAudioVariant {
            format_id: Some("140".into()),
            codec: Some("aac".into()),
            mime_type: Some("audio/mp4".into()),
            ..first.clone()
        };
        assert_ne!(first.identity_sha256(), changed_format.identity_sha256());
        assert!(StreamAudioVariant::default().identity_sha256().is_none());
        assert!(StreamAudioVariant {
            format_id: Some("140".into()),
            codec: Some("aac".into()),
            expected_content_length: Some(0),
            ..StreamAudioVariant::default()
        }
        .identity_sha256()
        .is_none());
        assert!(StreamAudioVariant {
            format_id: Some("bad/id".into()),
            codec: Some("opus".into()),
            ..StreamAudioVariant::default()
        }
        .identity_sha256()
        .is_none());
    }

    #[test]
    fn stream_audio_variant_digest_matches_frontend_golden_value() {
        let variant = StreamAudioVariant {
            format_id: Some("140".into()),
            codec: Some("mp4a.40.2".into()),
            mime_type: Some("audio/mp4".into()),
            expected_content_length: Some(123_456),
            contains_video: Some(false),
            ..StreamAudioVariant::default()
        };
        assert_eq!(
            variant.identity_sha256().as_deref(),
            Some("d39cf402ad2591ff335487243ebe454a6972fc1ef87a43e72d1dad74392e60be")
        );
    }

    #[test]
    fn stream_audio_variant_uses_only_allowlisted_resolver_fields() {
        let variant = StreamAudioVariant::from_resolver_quality(&serde_json::json!({
            "formatId": "251",
            "codec": "Opus",
            "mimeType": "Audio/WebM",
            "bitrateKbps": 128,
            "expectedContentLength": 12000,
            "formatHasVideo": false,
            "url": "https://media.example.invalid/audio?token=secret",
            "headers": {"Authorization": "secret"}
        }))
        .unwrap();
        assert_eq!(variant.format_id.as_deref(), Some("251"));
        assert_eq!(variant.expected_content_length, Some(12_000));
        assert!(variant.identity_sha256().is_some());
        assert!(
            StreamAudioVariant::from_resolver_quality(&serde_json::json!({
                "formatId": "251",
                "codec": "Opus",
                "mimeType": "Audio/WebM",
                "expectedContentLength": "12000"
            }))
            .is_some()
        );
    }

    #[test]
    fn cache_is_version_and_source_bound_and_bounded() {
        let mut cache = StreamAnalysisCache::default();
        let first = result();
        first.validate().unwrap();
        cache.insert(first.clone()).unwrap();
        assert!(cache.get(&first.resource).is_some());
        let different = AnalysisResource::Stream {
            recording_id: first.resource.recording_id().into(),
            provider: "youtube".into(),
            source_identity_sha256: "b".repeat(64),
            audio_variant_sha256: Some("c".repeat(64)),
        };
        assert!(cache.get(&different).is_none());
        for index in 0..MAX_CACHE_RECORDS + 4 {
            let mut row = result();
            row.resource = AnalysisResource::Stream {
                recording_id: format!("recording-{index}"),
                provider: "youtube".into(),
                source_identity_sha256: format!("{:064x}", index + 1),
                audio_variant_sha256: Some("c".repeat(64)),
            };
            row.measured_at = format!("2026-10-25T12:{:02}:00Z", index % 60);
            cache.insert(row).unwrap();
        }
        assert_eq!(cache.records.len(), MAX_CACHE_RECORDS);
        cache.validate().unwrap();
    }

    #[test]
    fn variant_unknown_legacy_stream_rows_are_not_reused_or_rewritten() {
        let mut legacy = result();
        legacy.resource = AnalysisResource::Stream {
            recording_id: "catalog:recording-1".into(),
            provider: "youtube".into(),
            source_identity_sha256: "a".repeat(64),
            audio_variant_sha256: None,
        };
        let persisted = serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "records": [legacy],
        });
        let cache: StreamAnalysisCache = serde_json::from_value(persisted).unwrap();
        cache.validate().unwrap();
        assert!(cache.get(&cache.records[0].resource).is_none());
        let mut new_cache = StreamAnalysisCache::default();
        assert!(new_cache.insert(cache.records[0].clone()).is_err());
    }

    fn variant_unknown_result() -> WholeTrackAnalysis {
        let mut record = result();
        if let AnalysisResource::Stream {
            audio_variant_sha256,
            ..
        } = &mut record.resource
        {
            *audio_variant_sha256 = None;
        }
        record
    }

    #[test]
    fn session_only_stream_results_are_generation_and_resource_bound() {
        let mut sessions = SessionOnlyStreamAnalysisResults::default();
        let record = variant_unknown_result();
        let owner_generation = 41;
        let source = record.resource.clone();
        sessions.insert(owner_generation, record.clone()).unwrap();

        assert_eq!(
            sessions.get(owner_generation, &source),
            Some(record.clone())
        );
        assert!(sessions.get(owner_generation + 1, &source).is_none());
        assert!(sessions.get(0, &source).is_none());

        let other_source = AnalysisResource::Stream {
            recording_id: record.resource.recording_id().into(),
            provider: "youtube".into(),
            source_identity_sha256: "b".repeat(64),
            audio_variant_sha256: None,
        };
        assert!(sessions.get(owner_generation, &other_source).is_none());

        let stable_variant =
            record
                .resource
                .clone()
                .with_stream_audio_variant(Some(&StreamAudioVariant {
                    format_id: Some("140".into()),
                    codec: Some("aac".into()),
                    mime_type: Some("audio/mp4".into()),
                    ..StreamAudioVariant::default()
                }));
        assert!(stable_variant.has_stable_audio_variant());
        assert!(sessions.get(owner_generation, &stable_variant).is_none());

        // Session-only records must remain ineligible for durable reuse.
        let mut persistent = StreamAnalysisCache::default();
        assert_eq!(
            persistent.insert(record),
            Err(AnalysisError::InvalidResource)
        );
        assert!(persistent.records.is_empty());

        sessions.remove_generation(owner_generation);
        assert!(sessions.is_empty());
    }

    #[test]
    fn session_only_results_reject_invalid_scope_radio_and_stable_variants() {
        let mut sessions = SessionOnlyStreamAnalysisResults::default();
        let valid = variant_unknown_result();
        assert_eq!(
            sessions.insert(0, valid.clone()),
            Err(AnalysisError::InvalidResource)
        );

        let mut radio = valid.clone();
        if let AnalysisResource::Stream { provider, .. } = &mut radio.resource {
            *provider = "radio".into();
        }
        assert_eq!(
            sessions.insert(42, radio),
            Err(AnalysisError::InvalidResource)
        );

        assert_eq!(
            sessions.insert(42, result()),
            Err(AnalysisError::InvalidResource)
        );

        let mut partial = valid;
        partial.coverage = 0.75;
        assert_eq!(
            sessions.insert(42, partial),
            Err(AnalysisError::InvalidMeasurement)
        );
        assert!(sessions.is_empty());
    }

    #[test]
    fn session_only_results_have_a_fixed_capacity_and_generation_pruning() {
        let mut sessions = SessionOnlyStreamAnalysisResults::default();
        let mut first_resource = None;
        for index in 1..=(MAX_SESSION_ONLY_STREAM_ANALYSES as u64 + 1) {
            let mut record = variant_unknown_result();
            record.resource = AnalysisResource::Stream {
                recording_id: format!("catalog:recording-{index}"),
                provider: "youtube".into(),
                source_identity_sha256: format!("{index:064x}"),
                audio_variant_sha256: None,
            };
            if index == 1 {
                first_resource = Some(record.resource.clone());
            }
            sessions.insert(index, record).unwrap();
        }
        assert_eq!(sessions.len(), MAX_SESSION_ONLY_STREAM_ANALYSES);
        assert!(sessions.get(1, first_resource.as_ref().unwrap()).is_none());

        sessions.remove_older_than(MAX_SESSION_ONLY_STREAM_ANALYSES as u64);
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn partial_playback_measurement_does_not_erase_valid_stream_waveform() {
        let mut cache = StreamAnalysisCache::default();
        let mut full = result();
        full.waveform = Some(StreamWaveform {
            analyzer_version: STREAM_WAVEFORM_ANALYZER_VERSION.into(),
            bin_count: STREAM_WAVEFORM_BINS,
            duration_seconds: full.duration_seconds,
            bins: vec![
                StreamWaveformBin {
                    min: -0.5,
                    max: 0.5
                };
                STREAM_WAVEFORM_BINS
            ],
            scope: "wholeTrack".into(),
            complete: true,
            measured_at: full.measured_at.clone(),
        });
        cache.insert(full).unwrap();

        let mut playback_only = result();
        playback_only.waveform = None;
        playback_only.measured_at = "2026-10-25T12:01:00Z".into();
        cache.insert(playback_only).unwrap();
        assert!(cache.records[0].waveform.is_some());

        let mut different_duration = result();
        different_duration.duration_seconds = 181.0;
        different_duration.waveform = None;
        cache.insert(different_duration).unwrap();
        assert!(cache.records[0].waveform.is_none());
    }

    #[test]
    fn analysis_resource_serializes_camel_case_and_reads_legacy_snake_case_fields() {
        let stream = resource();
        let serialized = serde_json::to_value(&stream).unwrap();
        assert_eq!(serialized["recordingId"], "catalog:recording-1");
        assert_eq!(serialized["sourceIdentitySha256"], "a".repeat(64));
        assert!(serialized.get("recording_id").is_none());
        assert!(serialized.get("source_identity_sha256").is_none());
        assert_eq!(
            serde_json::from_value::<AnalysisResource>(serde_json::json!({
                "kind":"stream",
                "recording_id":"catalog:recording-1",
                "provider":"youtube",
                "source_identity_sha256":"a".repeat(64)
            }))
            .unwrap(),
            stream.clone().with_stream_audio_variant(None)
        );
        assert_eq!(serialized["audioVariantSha256"], "c".repeat(64));
        assert!(!stream
            .clone()
            .with_stream_audio_variant(None)
            .has_stable_audio_variant());

        let asset = AnalysisResource::RegisteredAsset {
            asset_id: "asset-1".into(),
            recording_id: "catalog:recording-1".into(),
            file_hash: "b".repeat(64),
        };
        let serialized = serde_json::to_value(&asset).unwrap();
        assert_eq!(serialized["assetId"], "asset-1");
        assert_eq!(serialized["recordingId"], "catalog:recording-1");
        assert_eq!(serialized["fileHash"], "b".repeat(64));
        assert!(serialized.get("asset_id").is_none());
        assert!(serialized.get("recording_id").is_none());
        assert!(serialized.get("file_hash").is_none());
        assert_eq!(
            serde_json::from_value::<AnalysisResource>(serde_json::json!({
                "kind":"registeredAsset",
                "asset_id":"asset-1",
                "recording_id":"catalog:recording-1",
                "file_hash":"b".repeat(64)
            }))
            .unwrap(),
            asset
        );
    }

    #[test]
    fn legacy_snake_case_cache_vector_recomputes_identity_and_roundtrips_as_camel_case() {
        let mut row = result();
        row.resource = row.resource.with_stream_audio_variant(None);
        let legacy = serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "records": [{
                "schemaVersion": row.schema_version,
                "analyzerVersion": row.analyzer_version.clone(),
                "resource": {
                    "kind": "stream",
                    "recording_id": "catalog:recording-1",
                    "provider": "youtube",
                    "source_identity_sha256": "a".repeat(64)
                },
                "durationSeconds": row.duration_seconds,
                "scope": row.scope.clone(),
                "coverage": row.coverage,
                "measuredAt": row.measured_at.clone(),
                "bpm": row.bpm.clone(),
                "key": row.key.clone()
            }]
        });
        let mut cache: StreamAnalysisCache = serde_json::from_value(legacy).unwrap();
        cache.validate().unwrap();
        assert_eq!(cache.records.len(), 1);
        assert_eq!(cache.records[0].resource, row.resource);
        let identity_before_roundtrip = cache.records[0].resource.cache_identity().unwrap();
        assert!(cache.get(&row.resource).is_none());

        let reserialized = serde_json::to_value(&cache).unwrap();
        let serialized_resource = &reserialized["records"][0]["resource"];
        assert_eq!(serialized_resource["recordingId"], "catalog:recording-1");
        assert_eq!(serialized_resource["sourceIdentitySha256"], "a".repeat(64));
        assert!(serialized_resource.get("recording_id").is_none());
        assert!(serialized_resource.get("source_identity_sha256").is_none());

        let restored: StreamAnalysisCache = serde_json::from_value(reserialized).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.records.len(), 1);
        assert_eq!(
            restored.records[0].resource.cache_identity().unwrap(),
            identity_before_roundtrip
        );
        assert!(restored.get(&row.resource).is_none());
    }

    #[test]
    fn imported_and_manual_fields_are_preserved_without_explicit_replace() {
        let analysis = result();
        let track = Track {
            id: analysis.resource.recording_id().into(),
            bpm: Some(126.0),
            key: Some("C".into()),
            provenance: json!({"manualFields":{"key":true},"bpmSource":"catalog"}),
            ..Track::default()
        };
        let selection = ApplyAnalysisSelection {
            fields: [AnalysisMetric::Bpm, AnalysisMetric::Key]
                .into_iter()
                .collect(),
            policies: BTreeMap::new(),
        };
        let applied = apply_analysis(&track, &analysis, &selection).unwrap();
        assert_eq!(applied.bpm, Some(126.0));
        assert_eq!(applied.key.as_deref(), Some("C"));
        assert_eq!(applied.provenance["manualFields"]["key"], true);
    }

    #[test]
    fn even_a_blank_non_null_imported_key_is_preserved() {
        let analysis = result();
        let track = Track {
            id: analysis.resource.recording_id().into(),
            key: Some(String::new()),
            provenance: json!({"keySource":"catalog"}),
            ..Track::default()
        };
        let selection = ApplyAnalysisSelection {
            fields: [AnalysisMetric::Key].into_iter().collect(),
            policies: BTreeMap::new(),
        };
        let applied = apply_analysis(&track, &analysis, &selection).unwrap();
        assert_eq!(applied.key.as_deref(), Some(""));
        assert_eq!(applied.provenance["keySource"], "catalog");
    }

    #[test]
    fn explicit_replace_only_changes_selected_field_and_retains_previous_value() {
        let analysis = result();
        let track = Track {
            id: analysis.resource.recording_id().into(),
            bpm: Some(126.0),
            key: Some("C".into()),
            provenance: json!({"manualFields":{"key":true}}),
            ..Track::default()
        };
        let selection = ApplyAnalysisSelection {
            fields: [AnalysisMetric::Bpm, AnalysisMetric::Key]
                .into_iter()
                .collect(),
            policies: [(AnalysisMetric::Key, FieldPolicy::Replace)]
                .into_iter()
                .collect(),
        };
        let applied = apply_analysis(&track, &analysis, &selection).unwrap();
        assert_eq!(applied.bpm, Some(126.0));
        assert_eq!(applied.key.as_deref(), Some("Am"));
        assert!(applied.provenance["manualFields"].get("key").is_none());
        assert_eq!(
            applied.provenance["analysisReplacements"][0]["previousValue"],
            "C"
        );
    }

    #[test]
    fn capture_must_start_at_zero_and_reach_natural_end() {
        assert_eq!(
            WholeTrackCapture::new(1, resource(), 180.0, 12.0).unwrap_err(),
            AnalysisError::CaptureNotStartedAtTrackBeginning
        );
        let capture = WholeTrackCapture::new(1, resource(), 180.0, 0.0).unwrap();
        assert_eq!(
            capture
                .finish(0.0, "2026-10-25T12:00:00Z".into())
                .unwrap_err(),
            AnalysisError::IncompleteCapture
        );
    }

    #[test]
    fn capture_rejects_pcm_position_gap() {
        let mut capture = WholeTrackCapture::new(1, resource(), 180.0, 0.0).unwrap();
        let samples = vec![0.0_f32; 48_000 * 2];
        capture
            .push(PcmBatch {
                generation: 1,
                sample_rate: 48_000,
                channels: 2,
                end_position_seconds: 1.0,
                interleaved: &samples,
            })
            .unwrap();
        assert_eq!(
            capture
                .push(PcmBatch {
                    generation: 1,
                    sample_rate: 48_000,
                    channels: 2,
                    end_position_seconds: 3.0,
                    interleaved: &samples
                })
                .unwrap_err(),
            AnalysisError::DiscontinuousCapture
        );
    }

    #[test]
    fn progress_is_window_scoped_and_never_cacheable_before_verified_end() {
        let mut capture = WholeTrackCapture::new(2, resource(), 180.0, 0.0).unwrap();
        let samples = vec![0.0_f32; 48_000 * 2];
        capture
            .push(PcmBatch {
                generation: 2,
                sample_rate: 48_000,
                channels: 2,
                end_position_seconds: 1.0,
                interleaved: &samples,
            })
            .unwrap();
        let progress = capture.progress();
        assert_eq!(progress.scope, "playbackWindow");
        assert_eq!(progress.start_seconds, Some(0.0));
        assert_eq!(progress.end_seconds, Some(1.0));
        assert!(progress.continuous_so_far);
        assert!(!progress.cacheable);
        capture.invalidate();
        assert!(!capture.progress().continuous_so_far);
    }
}
