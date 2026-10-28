//! Bounded local loudness tag reads and FFmpeg EBU R128 measurements.
//!
//! The caller must select the asset explicitly. This module never accepts a
//! URL, launches a downloader, or modifies the audio file.

use crate::{loudness::*, models::LocalAsset, tools};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::Accessor,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(180);
pub const MAX_LOCAL_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 128 * 1024;
const MAX_CHANNELS: u32 = 32;
const CHANNEL_LAYOUT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const ALBUM_LAYOUT_PROBE_BUDGET: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct LocalTagMeasurements {
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_replaygain: Option<ReplayGainTags>,
    pub album_replaygain: Option<ReplayGainTags>,
    /// At least one ReplayGain comment existed, even if malformed. This prevents
    /// selecting R128 while silently ignoring a conflicting scheme.
    pub replaygain_comment_present: bool,
    pub opus_r128_track_gain_db: Option<f64>,
    pub opus_r128_album_gain_db: Option<f64>,
    /// Presence is tracked separately so malformed/conflicting R128 comments
    /// cannot silently fall back to a different normalization scheme.
    pub opus_r128_track_tag_present: bool,
    pub opus_r128_album_tag_present: bool,
    /// Optional textual annotation only; OpusHead's binary header gain is not
    /// read from Vorbis comments and is handled by the decoder itself.
    pub opus_output_gain_tag_db: Option<f64>,
    pub file_seal: Option<LocalFileSeal>,
}

impl LocalTagMeasurements {
    /// ReplayGain and R128 comments describe the encoded asset as a whole.
    /// They do not carry time-range coverage, so they cannot authorize gain
    /// for an excerpt or chapter cut from that asset.
    pub fn applies_to_range(&self, range: AudioRange) -> bool {
        range == AudioRange::WHOLE_FILE
    }

    pub fn album_identity_matches(
        &self,
        expected_album: Option<&str>,
        expected_album_artist: Option<&str>,
    ) -> bool {
        let Some(expected_album) = expected_album
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return false;
        };
        let Some(found_album) = self
            .album
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return false;
        };
        if !found_album.eq_ignore_ascii_case(expected_album) {
            return false;
        }
        match (
            self.album_artist
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            expected_album_artist
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        ) {
            (Some(found), Some(expected)) => found.eq_ignore_ascii_case(expected),
            (None, _) => true,
            (Some(_), None) => false,
        }
    }
}

pub fn opus_comment_schemes_are_mixed(tags: &LocalTagMeasurements) -> bool {
    (tags.opus_r128_track_tag_present || tags.opus_r128_album_tag_present)
        && tags.replaygain_comment_present
}

/// Decide one verified Opus R128 comment. RFC 7845 requires R128 gain to be
/// applied in addition to OpusHead output gain. Coexisting ReplayGain and R128
/// tags are ambiguous, so this helper fails closed instead of inventing a
/// precedence between schemes.
pub fn decide_verified_opus_r128_tag(
    tags: &LocalTagMeasurements,
    album_scope: bool,
    album_identity_matches: bool,
    range: AudioRange,
    post_header_true_peak_dbtp: Option<f64>,
    header_gain_applied_once: bool,
    policy: GainPolicy,
    mode: NormalizationMode,
) -> GainDecision {
    if !header_gain_applied_once || range != AudioRange::WHOLE_FILE {
        return GainDecision::Unavailable;
    }
    let (present, gain_db, competing_replaygain) = if album_scope {
        if !album_identity_matches {
            return GainDecision::Unavailable;
        }
        (
            tags.opus_r128_album_tag_present,
            tags.opus_r128_album_gain_db,
            tags.replaygain_comment_present,
        )
    } else {
        (
            tags.opus_r128_track_tag_present,
            tags.opus_r128_track_gain_db,
            tags.replaygain_comment_present,
        )
    };
    if !present || competing_replaygain || opus_comment_schemes_are_mixed(tags) {
        return GainDecision::Unavailable;
    }
    let Some(gain_db) = gain_db else {
        return GainDecision::Unavailable;
    };
    crate::loudness::decide_opus_r128_gain_db(
        gain_db,
        post_header_true_peak_dbtp,
        header_gain_applied_once,
        policy,
        mode,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct EbuR128Result {
    pub integrated_lufs: f64,
    pub true_peak_dbtp: f64,
    pub loudness_range_lu: Option<f64>,
    pub target_offset_db: Option<f64>,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    Cancelled,
    NotLocalFile,
    FileTooLarge,
    ResourceChanged,
    Spawn(String),
    Timeout,
    Failed(String),
    InvalidOutput,
    Unmeasurable,
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Loudness analysis was cancelled"),
            Self::NotLocalFile => f.write_str("Loudness analysis requires a regular local file"),
            Self::FileTooLarge => f.write_str("Audio file exceeds the analysis size limit"),
            Self::ResourceChanged => f.write_str("Audio file changed during loudness analysis"),
            Self::Spawn(message) | Self::Failed(message) => f.write_str(message),
            Self::Timeout => f.write_str("Loudness analysis exceeded its time limit"),
            Self::InvalidOutput => f.write_str("FFmpeg returned invalid loudness measurements"),
            Self::Unmeasurable => {
                f.write_str("Loudness could not be measured; the audio may be silent or too short")
            }
        }
    }
}

impl std::error::Error for AnalysisError {}

#[derive(Debug, Default)]
struct OpusR128Comment {
    count: u8,
    raw_value: Option<String>,
}

fn record_opus_r128_comment(
    comments: &mut std::collections::HashMap<String, OpusR128Comment>,
    key: &str,
    raw_value: &str,
) {
    let comment = comments.entry(key.to_owned()).or_default();
    if comment.count == 0 {
        comment.raw_value = Some(raw_value.to_owned());
    } else {
        // RFC 7845 permits at most one tag of each kind, even if duplicates
        // happen to carry identical values.
        comment.raw_value = None;
    }
    comment.count = comment.count.saturating_add(1).min(2);
}

fn parse_strict_opus_r128_comment(value: &str) -> Option<f64> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 6 || !value.is_ascii() {
        return None;
    }
    let digits = match bytes[0] {
        b'+' | b'-' => &bytes[1..],
        _ => bytes,
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let raw = value.parse::<i32>().ok()?;
    (-32_768..=32_767)
        .contains(&raw)
        .then_some(f64::from(raw) / 256.0)
}

/// Read and validate ReplayGain/R128/Opus header tags from one registered local
/// asset. The caller must separately decide whether the active decoder already
/// applies OpusHead output gain; this reader only reports it.
pub fn read_local_tags(asset: &LocalAsset) -> Result<LocalTagMeasurements, AnalysisError> {
    read_local_tags_cancellable(asset, &AtomicBool::new(false))
}

pub fn read_local_tags_cancellable(
    asset: &LocalAsset,
    cancelled: &AtomicBool,
) -> Result<LocalTagMeasurements, AnalysisError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(AnalysisError::Cancelled);
    }
    let path = verified_local_asset_path(asset)?;
    let before_seal = local_file_seal(&path, &asset.id, &asset.hash)?;
    let probe = Probe::open(&path)
        .map_err(|error| AnalysisError::Failed(format!("Could not open audio tags: {error}")))?;
    let probe = probe.guess_file_type().map_err(|error| {
        AnalysisError::Failed(format!("Could not identify audio tags: {error}"))
    })?;
    let tagged = probe
        .read()
        .map_err(|error| AnalysisError::Failed(format!("Could not read audio tags: {error}")))?;

    let mut values = std::collections::HashMap::<String, Option<String>>::new();
    let mut replaygain_comment_present = false;
    let mut opus_r128_comments = std::collections::HashMap::<String, OpusR128Comment>::new();
    for tag in tagged.tags() {
        for item in tag.items() {
            let Some(key) = item.key().map_key(tag.tag_type(), true) else {
                continue;
            };
            let Some(value) = item.value().text() else {
                continue;
            };
            let normalized = normalize_tag_key(key);
            if matches!(normalized.as_str(), "R128_TRACK_GAIN" | "R128_ALBUM_GAIN") {
                record_opus_r128_comment(&mut opus_r128_comments, &normalized, value);
            }
            if normalized.starts_with("REPLAYGAIN_") {
                replaygain_comment_present = true;
            }
            if is_loudness_key(&normalized) {
                // Conflicting duplicates are ambiguous and therefore become
                // unavailable rather than trusting an arbitrary tag order.
                if !value.trim().is_empty() {
                    values
                        .entry(normalized)
                        .and_modify(|prior| {
                            if prior.as_deref().is_some_and(|prior| prior != value.trim()) {
                                *prior = None;
                            }
                        })
                        .or_insert_with(|| Some(value.trim().to_owned()));
                }
            }
        }
    }

    let album = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .and_then(|tag| tag.album().map(|value| value.into_owned()));
    let album_artist = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .and_then(|tag| {
            tag.get_string(&lofty::tag::ItemKey::AlbumArtist)
                .map(str::to_owned)
        });

    let pair = |gain_key: &str, peak_key: &str| {
        values
            .get(gain_key)
            .and_then(Option::as_deref)
            .and_then(|gain| {
                ReplayGainTags::parse(gain, values.get(peak_key).and_then(Option::as_deref)).ok()
            })
    };
    let opus_gain = |key: &str| {
        opus_r128_comments
            .get(key)
            .filter(|comment| comment.count == 1)
            .and_then(|comment| comment.raw_value.as_deref())
            .and_then(parse_strict_opus_r128_comment)
    };
    let opus_output_gain_tag_db = values
        .get("OPUS_OUTPUT_GAIN")
        .and_then(Option::as_deref)
        .and_then(|value| value.trim().parse::<i32>().ok())
        .and_then(|raw| {
            (-32768..=32767)
                .contains(&raw)
                .then_some(f64::from(raw) / 256.0)
        })
        .filter(|gain| (-128.0..=128.0).contains(gain));

    let result = LocalTagMeasurements {
        album,
        album_artist,
        track_replaygain: pair("REPLAYGAIN_TRACK_GAIN", "REPLAYGAIN_TRACK_PEAK"),
        album_replaygain: pair("REPLAYGAIN_ALBUM_GAIN", "REPLAYGAIN_ALBUM_PEAK"),
        replaygain_comment_present,
        opus_r128_track_gain_db: opus_gain("R128_TRACK_GAIN"),
        opus_r128_album_gain_db: opus_gain("R128_ALBUM_GAIN"),
        opus_r128_track_tag_present: opus_r128_comments
            .get("R128_TRACK_GAIN")
            .is_some_and(|comment| comment.count > 0),
        opus_r128_album_tag_present: opus_r128_comments
            .get("R128_ALBUM_GAIN")
            .is_some_and(|comment| comment.count > 0),
        opus_output_gain_tag_db,
        file_seal: None,
    };
    if hash_file_cancelable(&path, cancelled)? != asset.hash {
        return Err(AnalysisError::ResourceChanged);
    }
    let after_seal = local_file_seal(&path, &asset.id, &asset.hash)?;
    if !same_file_seal(&before_seal, &after_seal) {
        return Err(AnalysisError::ResourceChanged);
    }
    let mut result = result;
    result.file_seal = Some(after_seal);
    Ok(result)
}

/// Analyze one exact local asset with FFmpeg's `loudnorm` analysis-only pass.
/// No output file is created and the input is re-hashed after the process exits
/// so a concurrent replacement cannot publish measurements for stale bytes.
pub fn analyze_local_asset(
    asset: &LocalAsset,
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
    opus_decoder_proof: Option<crate::loudness::OpusDecoderProof>,
) -> Result<LoudnessRecord, AnalysisError> {
    let path = verified_local_asset_path(asset)?;
    let file_size = std::fs::metadata(&path)
        .map_err(|_| AnalysisError::NotLocalFile)?
        .len();
    if file_size > MAX_LOCAL_FILE_BYTES {
        return Err(AnalysisError::FileTooLarge);
    }
    let before_seal = local_file_seal(&path, &asset.id, &asset.hash)?;
    let before_hash = hash_file_cancelable(&path, &cancelled)?;
    if before_hash != asset.hash {
        return Err(AnalysisError::ResourceChanged);
    }

    let result = run_loudnorm(&path, ffmpeg, cancelled.clone())?;
    if cancelled.load(Ordering::Acquire) {
        return Err(AnalysisError::Cancelled);
    }
    let after_hash = hash_file_cancelable(&path, &cancelled)?;
    let after_seal = local_file_seal(&path, &asset.id, &asset.hash)?;
    if after_hash != asset.hash
        || before_hash != after_hash
        || !same_file_seal(&before_seal, &after_seal)
    {
        return Err(AnalysisError::ResourceChanged);
    }

    let probe = Probe::open(&path).map_err(|error| {
        AnalysisError::Failed(format!("Could not open audio properties: {error}"))
    })?;
    let probe = probe.guess_file_type().map_err(|error| {
        AnalysisError::Failed(format!("Could not identify audio properties: {error}"))
    })?;
    let tagged = probe.read().map_err(|error| {
        AnalysisError::Failed(format!("Could not read audio properties: {error}"))
    })?;
    let properties = tagged.properties();
    let channels = u32::from(properties.channels().unwrap_or(0));
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AnalysisError::InvalidOutput);
    }
    let sample_rate = properties.sample_rate().filter(|rate| *rate > 0);
    let input_layout = local_channel_layout(&path, &cancelled);
    let stereo_mix = match (channels, input_layout.as_deref()) {
        (1, Some("mono")) => Some((
            run_stereo_mix_loudnorm(&path, ffmpeg, cancelled.clone(), "mono")?,
            "web-audio-mono-to-stereo-v1",
            "mono",
        )),
        (2, Some("stereo")) => Some((result.clone(), "web-audio-stereo-identity-v1", "stereo")),
        (6, Some("5.1")) => Some((
            run_stereo_mix_loudnorm(&path, ffmpeg, cancelled.clone(), "5.1")?,
            "web-audio-5.1-to-stereo-v1",
            "5.1",
        )),
        (6, Some("5.1(side)")) => Some((
            run_stereo_mix_loudnorm(&path, ffmpeg, cancelled.clone(), "5.1(side)")?,
            "web-audio-5.1-side-to-stereo-v1",
            "5.1(side)",
        )),
        _ => None,
    };
    let record = LoudnessRecord {
        schema_version: LOUDNESS_SCHEMA_VERSION,
        resource: LoudnessResourceIdentity::track(
            asset.id.clone(),
            asset.hash.clone(),
            AudioRange::WHOLE_FILE,
        ),
        analyzer_version: result.analyzer_version.clone(),
        provenance: MeasurementProvenance::FfmpegAnalysis,
        integrated_lufs: Some(result.integrated_lufs),
        true_peak_dbtp: Some(result.true_peak_dbtp),
        sample_rate,
        channels,
        measured_at: chrono::Utc::now().to_rfc3339(),
        stereo_mix_integrated_lufs: stereo_mix
            .as_ref()
            .map(|(value, _, _)| value.integrated_lufs),
        stereo_mix_true_peak_dbtp: stereo_mix
            .as_ref()
            .map(|(value, _, _)| value.true_peak_dbtp),
        stereo_mix_matrix: stereo_mix
            .as_ref()
            .map(|(_, matrix, _)| (*matrix).to_owned()),
        stereo_mix_layout: stereo_mix.map(|(_, _, layout)| layout.to_owned()),
        maximum_member_safe_true_peak_dbtp: None,
        maximum_member_native_safe_true_peak_dbtp: None,
        conservative_channel_sum_bound: channels > 2 && input_layout.is_none(),
        local_file_seals: vec![after_seal],
        opus_decoder_proofs: opus_decoder_proof.into_iter().collect(),
    };
    record
        .validate()
        .map_err(|_| AnalysisError::InvalidOutput)?;
    Ok(record)
}

/// Analyze an explicitly selected ordered album programme. Each inter-track
/// silence duration is supplied by the caller and is part of the resource key;
/// this function never guesses gaps from tag or playback metadata.
pub fn analyze_local_album(
    assets: &[LocalAsset],
    inter_track_gaps_seconds: &[f64],
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
    opus_decoder_proofs: Vec<crate::loudness::OpusDecoderProof>,
) -> Result<LoudnessRecord, AnalysisError> {
    if !(2..=200).contains(&assets.len())
        || inter_track_gaps_seconds.len() + 1 != assets.len()
        || inter_track_gaps_seconds
            .iter()
            .any(|gap| !gap.is_finite() || !(0.0..=120.0).contains(gap))
    {
        return Err(AnalysisError::InvalidOutput);
    }

    let mut paths = Vec::with_capacity(assets.len());
    let mut input_layouts = Vec::with_capacity(assets.len());
    let mut members = Vec::with_capacity(assets.len());
    let mut before_seals = Vec::with_capacity(assets.len());
    let mut before_hashes = Vec::with_capacity(assets.len());
    let mut total_bytes = 0_u64;
    let layout_probe_started = Instant::now();
    for asset in assets {
        if cancelled.load(Ordering::Acquire) {
            return Err(AnalysisError::Cancelled);
        }
        let path = verified_local_asset_path(asset)?;
        let metadata = std::fs::metadata(&path).map_err(|_| AnalysisError::NotLocalFile)?;
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_LOCAL_FILE_BYTES {
            return Err(AnalysisError::FileTooLarge);
        }
        let seal = local_file_seal(&path, &asset.id, &asset.hash)?;
        let hash = hash_file_cancelable(&path, &cancelled)?;
        if hash != asset.hash {
            return Err(AnalysisError::ResourceChanged);
        }
        paths.push(path);
        let layout = if layout_probe_started.elapsed() < ALBUM_LAYOUT_PROBE_BUDGET {
            local_channel_layout(paths.last().expect("path was just pushed"), &cancelled)
        } else {
            None
        };
        input_layouts.push(layout);
        before_seals.push(seal);
        before_hashes.push(hash.clone());
        members.push(LoudnessAssetIdentity {
            asset_id: asset.id.clone(),
            file_hash: hash,
            range: AudioRange::WHOLE_FILE,
        });
    }

    let mut command = Command::new(ffmpeg);
    tools::configure_no_console(&mut command);
    command
        .arg("-nostdin")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("info")
        .arg("-threads")
        .arg("1")
        .arg("-filter_threads")
        .arg("1")
        .arg("-protocol_whitelist")
        .arg("file,pipe");
    for path in &paths {
        command.arg("-i").arg(path);
    }
    let mut graph = String::new();
    for (index, gap) in inter_track_gaps_seconds.iter().enumerate() {
        let mix = input_layouts[index]
            .as_deref()
            .and_then(web_audio_stereo_pan_filter)
            .map(|pan| format!("{pan},aformat=sample_fmts=fltp"))
            .unwrap_or_else(|| "aformat=sample_fmts=fltp:channel_layouts=stereo".to_owned());
        graph.push_str(&format!(
            "[{index}:a:0]aresample=48000,{mix},apad=pad_dur={gap:.6}[a{index}];"
        ));
    }
    let last = assets.len() - 1;
    let last_mix = input_layouts[last]
        .as_deref()
        .and_then(web_audio_stereo_pan_filter)
        .map(|pan| format!("{pan},aformat=sample_fmts=fltp"))
        .unwrap_or_else(|| "aformat=sample_fmts=fltp:channel_layouts=stereo".to_owned());
    graph.push_str(&format!("[{last}:a:0]aresample=48000,{last_mix}[a{last}];"));
    for index in 0..assets.len() {
        graph.push_str(&format!("[a{index}]"));
    }
    graph.push_str(&format!(
        "concat=n={}:v=0:a=1,loudnorm=I=-18:TP=-1:LRA=11:print_format=json[out]",
        assets.len()
    ));
    command
        .arg("-filter_complex")
        .arg(graph)
        .arg("-map")
        .arg("[out]")
        .arg("-vn")
        .arg("-sn")
        .arg("-dn")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = run_bounded_ffmpeg(command, cancelled.clone(), ANALYSIS_TIMEOUT)?;
    let result =
        parse_loudnorm_stereo_mix_result(&output).ok_or_else(|| loudnorm_output_error(&output))?;
    if cancelled.load(Ordering::Acquire) {
        return Err(AnalysisError::Cancelled);
    }

    // Keep separate member bounds for the WebAudio stereo route and native
    // output. WebAudio-compatible matrices are explicit for known layouts;
    // unknown layouts use a coherent channel-sum bound. Native multichannel
    // playback keeps that bound because its decoder may use a different mix.
    let mut maximum_member_safe_true_peak_dbtp: Option<f64> = None;
    let mut maximum_member_native_safe_true_peak_dbtp: Option<f64> = None;
    let mut conservative_channel_sum_bound = false;
    let member_analysis_started = Instant::now();
    for (index, path) in paths.iter().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            return Err(AnalysisError::Cancelled);
        }
        let remaining = ANALYSIS_TIMEOUT.saturating_sub(member_analysis_started.elapsed());
        if remaining.is_zero() {
            return Err(AnalysisError::Timeout);
        }
        let member_result = run_loudnorm_with_timeout(path, ffmpeg, cancelled.clone(), remaining)?;
        let member_channels = local_channel_count(path)?;
        if member_channels == 0 || member_channels > MAX_CHANNELS {
            return Err(AnalysisError::InvalidOutput);
        }
        let member_route_supported = input_layouts[index]
            .as_deref()
            .is_some_and(|layout| web_audio_stereo_pan_filter(layout).is_some());
        let safe_peak = if member_channels > 2 && !member_route_supported {
            conservative_channel_sum_bound = true;
            member_result.true_peak_dbtp + 20.0 * f64::from(member_channels).log10()
        } else {
            member_result.true_peak_dbtp
        };
        maximum_member_safe_true_peak_dbtp = Some(
            maximum_member_safe_true_peak_dbtp.map_or(safe_peak, |current| current.max(safe_peak)),
        );
        let native_safe_peak = if member_channels > 2 {
            member_result.true_peak_dbtp + 20.0 * f64::from(member_channels).log10()
        } else {
            member_result.true_peak_dbtp
        };
        maximum_member_native_safe_true_peak_dbtp = Some(
            maximum_member_native_safe_true_peak_dbtp
                .map_or(native_safe_peak, |current| current.max(native_safe_peak)),
        );
    }

    let mut seals = Vec::with_capacity(assets.len());
    for (index, asset) in assets.iter().enumerate() {
        let after_hash = hash_file_cancelable(&paths[index], &cancelled)?;
        let after_seal = local_file_seal(&paths[index], &asset.id, &asset.hash)?;
        if after_hash != before_hashes[index] || !same_file_seal(&before_seals[index], &after_seal)
        {
            return Err(AnalysisError::ResourceChanged);
        }
        seals.push(after_seal);
    }
    let resource = LoudnessResourceIdentity::album(members, inter_track_gaps_seconds.to_vec());
    let programme_layouts: Option<Vec<&str>> = input_layouts
        .iter()
        .map(|layout| {
            layout
                .as_deref()
                .filter(|value| web_audio_stereo_pan_filter(value).is_some())
        })
        .collect();
    let stereo_mix_supported = programme_layouts.is_some();
    let record = LoudnessRecord {
        schema_version: LOUDNESS_SCHEMA_VERSION,
        resource,
        analyzer_version: result.analyzer_version,
        provenance: MeasurementProvenance::FfmpegAnalysis,
        integrated_lufs: Some(result.integrated_lufs),
        true_peak_dbtp: Some(result.true_peak_dbtp),
        sample_rate: Some(48_000),
        channels: 2,
        measured_at: chrono::Utc::now().to_rfc3339(),
        stereo_mix_integrated_lufs: stereo_mix_supported.then_some(result.integrated_lufs),
        stereo_mix_true_peak_dbtp: stereo_mix_supported.then_some(result.true_peak_dbtp),
        stereo_mix_matrix: stereo_mix_supported.then(|| "web-audio-album-program-v1".to_owned()),
        stereo_mix_layout: programme_layouts
            .map(|layouts| format!("programme:{}", layouts.join(","))),
        maximum_member_safe_true_peak_dbtp,
        maximum_member_native_safe_true_peak_dbtp,
        conservative_channel_sum_bound,
        local_file_seals: seals,
        opus_decoder_proofs,
    };
    record
        .validate()
        .map_err(|_| AnalysisError::InvalidOutput)?;
    Ok(record)
}

fn local_channel_count(path: &Path) -> Result<u32, AnalysisError> {
    let probe = Probe::open(path).map_err(|error| {
        AnalysisError::Failed(format!("Could not open audio properties: {error}"))
    })?;
    let probe = probe.guess_file_type().map_err(|error| {
        AnalysisError::Failed(format!("Could not identify audio properties: {error}"))
    })?;
    let tagged = probe.read().map_err(|error| {
        AnalysisError::Failed(format!("Could not read audio properties: {error}"))
    })?;
    Ok(u32::from(tagged.properties().channels().unwrap_or(0)))
}

fn local_channel_layout(path: &Path, cancelled: &AtomicBool) -> Option<String> {
    let mut command = Command::new(tools::executable("ffprobe", "ffprobe"));
    tools::configure_no_console(&mut command);
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=channel_layout",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "-protocol_whitelist",
            "file,pipe",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        if cancelled.load(Ordering::Acquire) || started.elapsed() >= CHANNEL_LAYOUT_PROBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return None;
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    let layout = output.trim();
    let channels = local_channel_count(path).ok()?;
    validated_channel_layout((!layout.is_empty()).then_some(layout), channels).map(str::to_owned)
}

fn validated_channel_layout(layout: Option<&str>, channels: u32) -> Option<&'static str> {
    match (layout, channels) {
        (Some("mono"), 1) | (None, 1) | (Some("unknown"), 1) => Some("mono"),
        (Some("stereo"), 2) | (None, 2) | (Some("unknown"), 2) => Some("stereo"),
        (Some("5.1"), 6) => Some("5.1"),
        (Some("5.1(side)"), 6) => Some("5.1(side)"),
        _ => None,
    }
}

/// Explicit WebAudio speakers-mode downmixes for the supported input layouts.
/// The 5.1 filters implement W3C Web Audio 4.5's speakers downmix: L/R plus
/// sqrt(1/2) center and surround, with LFE omitted and no FFmpeg renormalizing.
fn web_audio_stereo_pan_filter(layout: &str) -> Option<&'static str> {
    match layout {
        "mono" => Some("pan=stereo|FL=c0|FR=c0"),
        "stereo" => Some("pan=stereo|FL=c0|FR=c1"),
        "5.1" | "5.1(side)" => Some(
            "pan=stereo|FL=c0+0.7071067811865476*c2+0.7071067811865476*c4|FR=c1+0.7071067811865476*c2+0.7071067811865476*c5",
        ),
        _ => None,
    }
}

pub fn local_file_seal(
    path: &Path,
    asset_id: &str,
    file_hash: &str,
) -> Result<LocalFileSeal, AnalysisError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| AnalysisError::NotLocalFile)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AnalysisError::NotLocalFile);
    }
    let modified_unix_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64);
    #[cfg(unix)]
    let file_identity = {
        use std::os::unix::fs::MetadataExt;
        Some(format!("{}:{}", metadata.dev(), metadata.ino()))
    };
    #[cfg(not(unix))]
    let file_identity = None;
    Ok(LocalFileSeal {
        asset_id: asset_id.to_owned(),
        file_hash: file_hash.to_owned(),
        size_bytes: metadata.len(),
        modified_unix_ns,
        file_identity,
        verified_in_process: process_verification_id().to_owned(),
    })
}

pub fn verify_local_asset_seal(
    asset: &LocalAsset,
    cancelled: &AtomicBool,
) -> Result<LocalFileSeal, AnalysisError> {
    let path = verified_local_asset_path(asset)?;
    let before = local_file_seal(&path, &asset.id, &asset.hash)?;
    let actual_hash = hash_file_cancelable(&path, cancelled)?;
    if actual_hash != asset.hash {
        return Err(AnalysisError::ResourceChanged);
    }
    let after = local_file_seal(&path, &asset.id, &asset.hash)?;
    if !same_file_seal(&before, &after) {
        return Err(AnalysisError::ResourceChanged);
    }
    Ok(after)
}

fn same_file_seal(left: &LocalFileSeal, right: &LocalFileSeal) -> bool {
    left.size_bytes == right.size_bytes
        && left.modified_unix_ns == right.modified_unix_ns
        && left.file_identity == right.file_identity
}

fn hash_file_cancelable(path: &Path, cancelled: &AtomicBool) -> Result<String, AnalysisError> {
    let mut file = File::open(path).map_err(|_| AnalysisError::NotLocalFile)?;
    let started = Instant::now();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(AnalysisError::Cancelled);
        }
        if started.elapsed() >= ANALYSIS_TIMEOUT {
            return Err(AnalysisError::Timeout);
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| AnalysisError::NotLocalFile)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verified_local_asset_path(asset: &LocalAsset) -> Result<PathBuf, AnalysisError> {
    let supplied = Path::new(&asset.path);
    let metadata = std::fs::symlink_metadata(supplied).map_err(|_| AnalysisError::NotLocalFile)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AnalysisError::NotLocalFile);
    }
    let canonical = std::fs::canonicalize(supplied).map_err(|_| AnalysisError::NotLocalFile)?;
    if !canonical.is_absolute() || canonical != supplied {
        return Err(AnalysisError::NotLocalFile);
    }
    if canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "m3u" | "m3u8" | "pls" | "cue" | "ffconcat"
            )
        })
    {
        return Err(AnalysisError::NotLocalFile);
    }
    if metadata.len() > MAX_LOCAL_FILE_BYTES {
        return Err(AnalysisError::FileTooLarge);
    }
    Ok(canonical)
}

fn normalize_tag_key(key: &str) -> String {
    let upper = key.trim().to_ascii_uppercase();
    if let Some((_, property)) = upper.rsplit_once(':') {
        property.to_owned()
    } else {
        upper
    }
}

fn is_loudness_key(key: &str) -> bool {
    matches!(
        key,
        "REPLAYGAIN_TRACK_GAIN"
            | "REPLAYGAIN_TRACK_PEAK"
            | "REPLAYGAIN_ALBUM_GAIN"
            | "REPLAYGAIN_ALBUM_PEAK"
            | "R128_TRACK_GAIN"
            | "R128_ALBUM_GAIN"
            | "OPUS_OUTPUT_GAIN"
    )
}

fn run_loudnorm(
    path: &Path,
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<EbuR128Result, AnalysisError> {
    run_loudnorm_with_timeout(path, ffmpeg, cancelled, ANALYSIS_TIMEOUT)
}

fn run_loudnorm_with_timeout(
    path: &Path,
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<EbuR128Result, AnalysisError> {
    run_loudnorm_with_filter(
        path,
        ffmpeg,
        cancelled,
        timeout,
        "loudnorm=I=-18:TP=-1:LRA=11:print_format=json",
    )
}

fn run_stereo_mix_loudnorm(
    path: &Path,
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
    layout: &str,
) -> Result<EbuR128Result, AnalysisError> {
    let pan = web_audio_stereo_pan_filter(layout).ok_or(AnalysisError::InvalidOutput)?;
    let output = run_loudnorm_output_with_filter(
        path,
        ffmpeg,
        cancelled,
        ANALYSIS_TIMEOUT,
        &format!("{pan},loudnorm=I=-18:TP=-1:LRA=11:print_format=json"),
    )?;
    parse_loudnorm_stereo_mix_result(&output).ok_or_else(|| loudnorm_output_error(&output))
}

fn run_loudnorm_with_filter(
    path: &Path,
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
    filter: &str,
) -> Result<EbuR128Result, AnalysisError> {
    let output = run_loudnorm_output_with_filter(path, ffmpeg, cancelled, timeout, filter)?;
    parse_loudnorm_result(&output).ok_or_else(|| loudnorm_output_error(&output))
}

fn loudnorm_output_error(stderr: &[u8]) -> AnalysisError {
    let text = String::from_utf8_lossy(&stderr[..stderr.len().min(MAX_DIAGNOSTIC_BYTES)]);
    let Some(start) = text.rfind('{') else {
        return AnalysisError::InvalidOutput;
    };
    let Some(relative_end) = text[start..].find('}') else {
        return AnalysisError::InvalidOutput;
    };
    let Ok(parsed) = serde_json::from_str::<LoudnormJson>(&text[start..start + relative_end + 1])
    else {
        return AnalysisError::InvalidOutput;
    };
    if parsed
        .input_i
        .parse::<f64>()
        .is_ok_and(|value| value == f64::NEG_INFINITY)
        || parsed
            .input_tp
            .parse::<f64>()
            .is_ok_and(|value| value == f64::NEG_INFINITY)
    {
        AnalysisError::Unmeasurable
    } else {
        AnalysisError::InvalidOutput
    }
}

fn run_loudnorm_output_with_filter(
    path: &Path,
    ffmpeg: &Path,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
    filter: &str,
) -> Result<Vec<u8>, AnalysisError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(AnalysisError::Cancelled);
    }
    let mut command = Command::new(ffmpeg);
    tools::configure_no_console(&mut command);
    command
        .arg("-nostdin")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("info")
        .arg("-threads")
        .arg("1")
        .arg("-filter_threads")
        .arg("1")
        .arg("-protocol_whitelist")
        .arg("file,pipe")
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0:a:0")
        .arg("-vn")
        .arg("-sn")
        .arg("-dn")
        .arg("-af")
        .arg(filter)
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    run_bounded_ffmpeg(command, cancelled, timeout)
}

fn run_bounded_ffmpeg(
    mut command: Command,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Vec<u8>, AnalysisError> {
    let mut child = command
        .spawn()
        .map_err(|_| AnalysisError::Spawn("Could not start FFmpeg analysis".into()))?;
    let stderr = child.stderr.take().ok_or(AnalysisError::InvalidOutput)?;
    let diagnostics = Arc::new(Mutex::new(Vec::with_capacity(MAX_DIAGNOSTIC_BYTES)));
    let read_diagnostics = diagnostics.clone();
    let reader = match thread::Builder::new()
        .name("burrata-loudness-stderr".into())
        .spawn(move || drain_diagnostics(stderr, read_diagnostics))
    {
        Ok(reader) => reader,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AnalysisError::Spawn(
                "Could not read FFmpeg analysis output".into(),
            ));
        }
    };

    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(AnalysisError::Cancelled);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(AnalysisError::Timeout);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(AnalysisError::Failed(
                    "Could not wait for FFmpeg analysis".into(),
                ));
            }
        }
    };
    let _ = reader.join();
    let output = diagnostics
        .lock()
        .map_err(|_| AnalysisError::InvalidOutput)?
        .clone();
    if !status.success() {
        return Err(AnalysisError::Failed(safe_diagnostic(&output)));
    }
    Ok(output)
}

fn drain_diagnostics(mut stderr: impl Read, collected: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0_u8; 4096];
    loop {
        let read = match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if let Ok(mut output) = collected.lock() {
            let remaining = MAX_DIAGNOSTIC_BYTES.saturating_sub(output.len());
            output.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
}

fn safe_diagnostic(bytes: &[u8]) -> String {
    let _ = bytes;
    "FFmpeg could not analyze the selected local audio file".into()
}

#[derive(Debug, Deserialize)]
struct LoudnormJson {
    input_i: String,
    input_tp: String,
    #[serde(default)]
    input_lra: Option<String>,
    #[serde(default)]
    target_offset: Option<String>,
}

pub fn parse_loudnorm_result(stderr: &[u8]) -> Option<EbuR128Result> {
    parse_loudnorm_result_with_bounds(stderr, 0.0, 12.0)
}

pub fn parse_loudnorm_stereo_mix_result(stderr: &[u8]) -> Option<EbuR128Result> {
    parse_loudnorm_result_with_bounds(stderr, 12.0, 44.0)
}

fn parse_loudnorm_result_with_bounds(
    stderr: &[u8],
    max_integrated_lufs: f64,
    max_true_peak_dbtp: f64,
) -> Option<EbuR128Result> {
    let text = String::from_utf8_lossy(&stderr[..stderr.len().min(MAX_DIAGNOSTIC_BYTES)]);
    let start = text.rfind('{')?;
    let end = text[start..].find('}')? + start + 1;
    let parsed: LoudnormJson = serde_json::from_str(&text[start..end]).ok()?;
    let integrated_lufs = parsed.input_i.parse::<f64>().ok()?;
    let true_peak_dbtp = parsed.input_tp.parse::<f64>().ok()?;
    let value = EbuR128Result {
        integrated_lufs,
        true_peak_dbtp,
        loudness_range_lu: parsed.input_lra.and_then(|value| value.parse().ok()),
        target_offset_db: parsed.target_offset.and_then(|value| value.parse().ok()),
        analyzer_version: text
            .lines()
            .find(|line| {
                line.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("ffmpeg version")
            })
            .map(|line| line.trim().chars().take(96).collect::<String>())
            .map(|version| format!("{LOUDNESS_ANALYZER_VERSION}:{version}"))
            .unwrap_or_else(|| format!("{LOUDNESS_ANALYZER_VERSION}:unknown-build")),
    };
    if !value.integrated_lufs.is_finite()
        || !(-70.0..=max_integrated_lufs).contains(&value.integrated_lufs)
        || !value.true_peak_dbtp.is_finite()
        || !(-30.0..=max_true_peak_dbtp).contains(&value.true_peak_dbtp)
    {
        return None;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    #[test]
    fn opus_r128_comments_follow_rfc_integer_and_duplicate_rules() {
        assert_eq!(parse_strict_opus_r128_comment("-573"), Some(-573.0 / 256.0));
        assert_eq!(parse_strict_opus_r128_comment("+001"), Some(1.0 / 256.0));
        for invalid in ["", " 1", "1 ", "1.0", "1e2", "1234567", "32768", "-32769"] {
            assert_eq!(parse_strict_opus_r128_comment(invalid), None, "{invalid:?}");
        }
        let mut comments = std::collections::HashMap::new();
        record_opus_r128_comment(&mut comments, "R128_TRACK_GAIN", "-573");
        assert_eq!(comments["R128_TRACK_GAIN"].count, 1);
        record_opus_r128_comment(&mut comments, "R128_TRACK_GAIN", "-573");
        assert_eq!(comments["R128_TRACK_GAIN"].count, 2);
        assert!(comments["R128_TRACK_GAIN"].raw_value.is_none());
    }

    use super::*;

    fn read_bundled_opus_tags_fixture(bytes: &[u8]) -> LocalTagMeasurements {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.ogg");
        std::fs::write(&path, bytes).unwrap();
        let mut file = File::open(&path).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 1024];
        loop {
            let count = file.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let asset = LocalAsset {
            id: "synthetic-opus-fixture".into(),
            recording_id: "synthetic-recording".into(),
            path: std::fs::canonicalize(&path)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            hash: format!("{:x}", digest.finalize()),
            source_codec: "opus".into(),
            final_codec: "opus".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(48_000),
            channels: Some(2),
            bit_depth: None,
            duration_seconds: Some(0.5),
            size_bytes: Some(bytes.len() as u64),
            bitrate_origin: None,
            processing: "ready".into(),
            track: crate::models::Track::default(),
        };
        read_local_tags(&asset).expect("Lofty reads the real Ogg OpusTags packet")
    }

    #[test]
    fn opus_r128_file_reader_rejects_duplicate_and_malformed_tags() {
        let single =
            read_bundled_opus_tags_fixture(include_bytes!("../fixtures/opus-r128-single.ogg"));
        assert!(single.opus_r128_track_tag_present);
        assert_eq!(single.opus_r128_track_gain_db, Some(6.0));

        let duplicate = read_bundled_opus_tags_fixture(include_bytes!(
            "../fixtures/opus-r128-duplicate-identical.ogg"
        ));
        assert!(duplicate.opus_r128_track_tag_present);
        assert_eq!(duplicate.opus_r128_track_gain_db, None);

        let conflicting = read_bundled_opus_tags_fixture(include_bytes!(
            "../fixtures/opus-r128-duplicate-conflicting.ogg"
        ));
        assert!(conflicting.opus_r128_track_tag_present);
        assert_eq!(conflicting.opus_r128_track_gain_db, None);

        let malformed = read_bundled_opus_tags_fixture(include_bytes!(
            "../fixtures/opus-r128-invalid-whitespace.ogg"
        ));
        assert!(malformed.opus_r128_track_tag_present);
        assert_eq!(malformed.opus_r128_track_gain_db, None);
    }

    #[test]
    fn only_explicit_web_audio_matrices_are_authorized_for_stereo_metrics() {
        assert_eq!(
            web_audio_stereo_pan_filter("mono"),
            Some("pan=stereo|FL=c0|FR=c0")
        );
        assert_eq!(
            web_audio_stereo_pan_filter("stereo"),
            Some("pan=stereo|FL=c0|FR=c1")
        );
        let five_one = web_audio_stereo_pan_filter("5.1").unwrap();
        assert!(five_one.contains("0.7071067811865476*c2"));
        assert!(five_one.contains("0.7071067811865476*c4"));
        assert!(five_one.contains("0.7071067811865476*c5"));
        assert!(!five_one.contains("<"));
        assert_eq!(web_audio_stereo_pan_filter("5.1(side)"), Some(five_one));
        assert_eq!(web_audio_stereo_pan_filter("7.1"), None);
        assert_eq!(web_audio_stereo_pan_filter("unknown"), None);
    }

    #[test]
    fn ffprobe_missing_layout_uses_only_unambiguous_one_or_two_channel_fallbacks() {
        assert_eq!(validated_channel_layout(None, 1), Some("mono"));
        assert_eq!(validated_channel_layout(None, 2), Some("stereo"));
        assert_eq!(validated_channel_layout(Some("unknown"), 1), Some("mono"));
        assert_eq!(validated_channel_layout(Some("unknown"), 2), Some("stereo"));
        assert_eq!(validated_channel_layout(Some("5.1"), 6), Some("5.1"));
        assert_eq!(
            validated_channel_layout(Some("5.1(side)"), 6),
            Some("5.1(side)")
        );
        assert_eq!(validated_channel_layout(None, 6), None);
        assert_eq!(validated_channel_layout(Some("unknown"), 6), None);
        assert_eq!(validated_channel_layout(Some("5.1"), 2), None);
        assert_eq!(validated_channel_layout(Some("5.1"), 1), None);
    }

    #[test]
    fn parses_only_finite_bounded_loudnorm_measurements() {
        let stderr = br#"noise
{"input_i":"-20.5","input_tp":"-1.25","input_lra":"3.1","target_offset":"-2.5"}
"#;
        assert_eq!(
            parse_loudnorm_result(stderr),
            Some(EbuR128Result {
                integrated_lufs: -20.5,
                true_peak_dbtp: -1.25,
                loudness_range_lu: Some(3.1),
                target_offset_db: Some(-2.5),
                analyzer_version: format!("{LOUDNESS_ANALYZER_VERSION}:unknown-build"),
            })
        );
        assert!(parse_loudnorm_result(br#"{"input_i":"-inf","input_tp":"-1"}"#).is_none());
        assert!(parse_loudnorm_result(br#"{"input_i":"-20","input_tp":"NaN"}"#).is_none());
    }

    #[test]
    fn explicit_negative_infinity_gets_unmeasurable_error_but_malformed_output_stays_invalid() {
        assert_eq!(
            loudnorm_output_error(br#"{"input_i":"-inf","input_tp":"-inf"}"#),
            AnalysisError::Unmeasurable
        );
        assert_eq!(
            loudnorm_output_error(br#"{"input_i":"-inf","input_tp":"-1.2"}"#),
            AnalysisError::Unmeasurable
        );
        assert_eq!(
            loudnorm_output_error(br#"{"input_i":"NaN","input_tp":"-1.2"}"#),
            AnalysisError::InvalidOutput
        );
        assert_eq!(
            loudnorm_output_error(b"not ffmpeg json"),
            AnalysisError::InvalidOutput
        );
        assert_eq!(
            AnalysisError::Unmeasurable.to_string(),
            "Loudness could not be measured; the audio may be silent or too short"
        );
    }

    #[test]
    fn stereo_route_metrics_allow_derived_headroom_without_weakening_source_ranges() {
        let stereo =
            br#"{"input_i":"2.1","input_tp":"13.2","input_lra":"3.1","target_offset":"-2.5"}"#;
        assert!(parse_loudnorm_result(stereo).is_none());
        let result = parse_loudnorm_stereo_mix_result(stereo).unwrap();
        assert_eq!(result.integrated_lufs, 2.1);
        assert_eq!(result.true_peak_dbtp, 13.2);
    }

    #[test]
    fn range_identity_rejects_invalid_bounds() {
        assert!(AudioRange::WHOLE_FILE.validate().is_ok());
        assert!(AudioRange {
            start_seconds: -1.0,
            end_seconds: None
        }
        .validate()
        .is_err());
        assert!(AudioRange {
            start_seconds: 10.0,
            end_seconds: Some(5.0)
        }
        .validate()
        .is_err());
    }

    #[test]
    fn opus_r128_requires_exact_full_file_decoder_and_unambiguous_scheme() {
        let tags = LocalTagMeasurements {
            album: Some("Night Drive".into()),
            album_artist: Some("The Example Band".into()),
            track_replaygain: None,
            album_replaygain: None,
            replaygain_comment_present: false,
            opus_r128_track_gain_db: Some(-2.0),
            opus_r128_album_gain_db: Some(-1.0),
            opus_r128_track_tag_present: true,
            opus_r128_album_tag_present: true,
            opus_output_gain_tag_db: Some(3.0),
            file_seal: None,
        };
        let policy = GainPolicy::default();
        assert_eq!(
            decide_verified_opus_r128_tag(
                &tags,
                false,
                true,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::ApplyDb(3.0),
            "R128 adjustment is applied in addition to decoded OpusHead gain"
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &tags,
                false,
                true,
                AudioRange {
                    start_seconds: 4.0,
                    end_seconds: None
                },
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::Unavailable
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &tags,
                false,
                true,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                false,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::Unavailable,
            "R128 is not authorized when the active decoder does not apply OpusHead gain"
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &tags,
                true,
                false,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Album,
            ),
            GainDecision::Unavailable
        );
        let mut conflicting = tags.clone();
        conflicting.track_replaygain = Some(ReplayGainTags {
            gain_db: -4.0,
            peak_linear: None,
        });
        conflicting.replaygain_comment_present = true;
        assert!(opus_comment_schemes_are_mixed(&conflicting));
        let mut cross_scope_conflict = tags.clone();
        cross_scope_conflict.album_replaygain = Some(ReplayGainTags {
            gain_db: -3.0,
            peak_linear: None,
        });
        cross_scope_conflict.replaygain_comment_present = true;
        assert!(opus_comment_schemes_are_mixed(&cross_scope_conflict));
        assert_eq!(
            decide_verified_opus_r128_tag(
                &cross_scope_conflict,
                true,
                true,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Album,
            ),
            GainDecision::Unavailable,
            "mixed schemes fail closed across track/album tag scopes"
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &conflicting,
                false,
                true,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::Unavailable,
            "RFC 7845 warns against simultaneous R128 and ReplayGain schemes"
        );
        let mut malformed = tags.clone();
        malformed.opus_r128_track_gain_db = None;
        assert_eq!(
            decide_verified_opus_r128_tag(
                &malformed,
                false,
                true,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::Unavailable,
            "a present but malformed R128 comment cannot fall through"
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &tags,
                false,
                true,
                AudioRange::WHOLE_FILE,
                None,
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::Unavailable,
            "a positive request without a peak cannot boost"
        );
        let mut peak_limited = tags.clone();
        peak_limited.opus_r128_track_gain_db = Some(5.0);
        assert_eq!(
            decide_verified_opus_r128_tag(
                &peak_limited,
                false,
                true,
                AudioRange::WHOLE_FILE,
                Some(-2.0),
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::ApplyDb(1.0),
            "the exact post-header true peak limits the requested R128 gain"
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &peak_limited,
                false,
                true,
                AudioRange::WHOLE_FILE,
                Some(13.0),
                true,
                policy,
                NormalizationMode::Track,
            ),
            GainDecision::Unavailable,
            "an out-of-range decoder peak fails closed"
        );
        assert_eq!(
            decide_verified_opus_r128_tag(
                &tags,
                true,
                true,
                AudioRange::WHOLE_FILE,
                Some(-5.0),
                true,
                policy,
                NormalizationMode::Album,
            ),
            GainDecision::ApplyDb(4.0),
            "matching album R128 gain is separately applied and peak bounded"
        );
    }

    #[test]
    fn album_tags_need_a_matching_album_identity() {
        let tags = LocalTagMeasurements {
            album: Some("Night Drive".into()),
            album_artist: Some("The Example Band".into()),
            track_replaygain: None,
            album_replaygain: Some(ReplayGainTags {
                gain_db: -4.0,
                peak_linear: Some(0.8),
            }),
            replaygain_comment_present: true,
            opus_r128_track_gain_db: None,
            opus_r128_album_gain_db: Some(-4.0),
            opus_r128_track_tag_present: false,
            opus_r128_album_tag_present: true,
            opus_output_gain_tag_db: None,
            file_seal: None,
        };
        assert!(tags.album_identity_matches(Some("night drive"), Some("the example band")));
        assert!(
            !tags.album_identity_matches(Some("Night Drive (Remaster)"), Some("The Example Band"))
        );
        assert!(!tags.album_identity_matches(None, Some("The Example Band")));
    }

    #[test]
    fn whole_file_tags_do_not_authorize_gain_for_cropped_playback_ranges() {
        let tags = LocalTagMeasurements {
            album: None,
            album_artist: None,
            track_replaygain: Some(ReplayGainTags {
                gain_db: -5.0,
                peak_linear: Some(0.7),
            }),
            album_replaygain: None,
            replaygain_comment_present: false,
            opus_r128_track_gain_db: None,
            opus_r128_album_gain_db: None,
            opus_r128_track_tag_present: false,
            opus_r128_album_tag_present: false,
            opus_output_gain_tag_db: None,
            file_seal: None,
        };

        assert!(tags.applies_to_range(AudioRange::WHOLE_FILE));
        assert!(!tags.applies_to_range(AudioRange {
            start_seconds: 10.0,
            end_seconds: Some(25.0),
        }));
        assert!(!tags.applies_to_range(AudioRange {
            start_seconds: 10.0,
            end_seconds: None,
        }));
    }

    #[cfg(unix)]
    #[test]
    fn analysis_timeout_kills_a_stalled_ffmpeg_child() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("stalled-ffmpeg");
        std::fs::write(&script, b"#!/bin/sh\nexec sleep 10\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        let result = run_loudnorm_with_timeout(
            Path::new("/dev/null"),
            &script,
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(50),
        );
        assert_eq!(result, Err(AnalysisError::Timeout));
    }

    #[cfg(unix)]
    #[test]
    fn analysis_cancellation_kills_a_running_ffmpeg_child() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("cancelled-ffmpeg");
        std::fs::write(&script, b"#!/bin/sh\nexec sleep 10\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_worker = cancelled.clone();
        let script_path = script.clone();
        let worker = thread::spawn(move || {
            run_loudnorm_with_timeout(
                Path::new("/dev/null"),
                &script_path,
                cancelled,
                Duration::from_secs(2),
            )
        });
        thread::sleep(Duration::from_millis(50));
        cancel_worker.store(true, Ordering::Release);
        assert_eq!(worker.join().unwrap(), Err(AnalysisError::Cancelled));
    }
}
