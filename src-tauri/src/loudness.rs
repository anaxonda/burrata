//! Pure loudness normalization policy and tag validation.
//!
//! This module intentionally has no playback or storage dependencies. Callers
//! must bind every measurement to the exact playable resource and select the
//! decoder-specific Opus output-gain behavior before asking for a gain.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const LOUDNESS_SCHEMA_VERSION: u32 = 2;
pub const LEGACY_LOUDNESS_SCHEMA_VERSION: u32 = 1;
pub const LOUDNESS_ANALYZER_VERSION: &str = "ffmpeg-loudnorm-ebur128-web-audio-mix-v2";

pub fn process_verification_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Exact portion of a resource covered by a measurement. `end_seconds: None`
/// means the full file beginning at `start_seconds` (normally zero).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioRange {
    pub start_seconds: f64,
    pub end_seconds: Option<f64>,
}

impl AudioRange {
    pub const WHOLE_FILE: Self = Self {
        start_seconds: 0.0,
        end_seconds: None,
    };

    pub fn validate(self) -> Result<Self, MeasurementError> {
        if !self.start_seconds.is_finite()
            || self
                .end_seconds
                .is_some_and(|end| !end.is_finite() || end <= self.start_seconds)
        {
            return Err(MeasurementError::OutOfRange);
        }
        if self.start_seconds < 0.0
            || self
                .end_seconds
                .is_some_and(|end| end - self.start_seconds > 24.0 * 60.0 * 60.0)
        {
            return Err(MeasurementError::OutOfRange);
        }
        Ok(self)
    }
}

/// Stable local asset identity and content hash. Never replace this with a
/// catalog recording ID: a re-encode or excerpt is a different resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessResourceIdentity {
    pub scope: ResourceScope,
    pub members: Vec<LoudnessAssetIdentity>,
    pub inter_track_gaps_seconds: Vec<f64>,
    pub resource_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceScope {
    Track,
    Album,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessAssetIdentity {
    pub asset_id: String,
    pub file_hash: String,
    pub range: AudioRange,
}

impl LoudnessResourceIdentity {
    pub fn track(
        asset_id: impl Into<String>,
        file_hash: impl Into<String>,
        range: AudioRange,
    ) -> Self {
        let mut resource = Self {
            scope: ResourceScope::Track,
            members: vec![LoudnessAssetIdentity {
                asset_id: asset_id.into(),
                file_hash: file_hash.into(),
                range,
            }],
            inter_track_gaps_seconds: vec![],
            resource_key: String::new(),
        };
        resource.resource_key = resource.compute_resource_key();
        resource
    }

    pub fn album(members: Vec<LoudnessAssetIdentity>, inter_track_gaps_seconds: Vec<f64>) -> Self {
        let mut resource = Self {
            scope: ResourceScope::Album,
            members,
            inter_track_gaps_seconds,
            resource_key: String::new(),
        };
        resource.resource_key = resource.compute_resource_key();
        resource
    }

    pub fn compute_resource_key(&self) -> String {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct KeyMaterial<'a> {
            scope: ResourceScope,
            members: &'a [LoudnessAssetIdentity],
            inter_track_gaps_seconds: &'a [f64],
        }
        let material = KeyMaterial {
            scope: self.scope,
            members: &self.members,
            inter_track_gaps_seconds: &self.inter_track_gaps_seconds,
        };
        let bytes = serde_json::to_vec(&material).unwrap_or_default();
        format!("{:x}", Sha256::digest(bytes))
    }

    pub fn validate(&self) -> Result<(), MeasurementError> {
        if self.members.is_empty() || self.members.len() > 200 || self.resource_key.len() != 64 {
            return Err(MeasurementError::OutOfRange);
        }
        match self.scope {
            ResourceScope::Track
                if self.members.len() != 1 || !self.inter_track_gaps_seconds.is_empty() =>
            {
                return Err(MeasurementError::OutOfRange)
            }
            ResourceScope::Album
                if self.members.len() < 2
                    || self.inter_track_gaps_seconds.len() + 1 != self.members.len()
                    || self
                        .inter_track_gaps_seconds
                        .iter()
                        .any(|gap| !gap.is_finite() || !(0.0..=120.0).contains(gap)) =>
            {
                return Err(MeasurementError::OutOfRange)
            }
            _ => {}
        }
        for member in &self.members {
            if member.asset_id.trim().is_empty()
                || member.file_hash.trim().is_empty()
                || member.asset_id.len() > 256
                || member.file_hash.len() > 256
            {
                return Err(MeasurementError::OutOfRange);
            }
            member.range.validate()?;
        }
        if self.compute_resource_key() != self.resource_key {
            return Err(MeasurementError::OutOfRange);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeasurementProvenance {
    FfmpegAnalysis,
    ReplayGainTrack,
    ReplayGainAlbum,
    OpusR128Track,
    OpusR128Album,
}

/// Privacy-safe proof that a specific Ogg Opus asset was analyzed by a
/// decoder which passed the bundled header-gain and R128-ignore self-test.
/// This intentionally stores only content hashes and a small test version,
/// never a local executable path or file seal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpusDecoderProof {
    pub asset_id: String,
    pub file_hash: String,
    pub decoder_sha256: String,
    pub probe_schema_version: u32,
}

impl OpusDecoderProof {
    pub fn validates_for(&self, asset_id: &str, file_hash: &str, decoder_sha256: &str) -> bool {
        self.probe_schema_version == 1
            && self.asset_id == asset_id
            && self.file_hash.eq_ignore_ascii_case(file_hash)
            && self.decoder_sha256.eq_ignore_ascii_case(decoder_sha256)
            && is_sha256(&self.file_hash)
            && is_sha256(&self.decoder_sha256)
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Cached loudness result. It is only reusable when the resource identity,
/// range, schema and analyzer version all match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessRecord {
    pub schema_version: u32,
    pub resource: LoudnessResourceIdentity,
    pub analyzer_version: String,
    pub provenance: MeasurementProvenance,
    pub integrated_lufs: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: u32,
    pub measured_at: String,
    /// Optional peak measured for the exact stereo signal delivered to the
    /// WebAudio output. Older records omit this and use a conservative bound.
    #[serde(default)]
    pub stereo_mix_true_peak_dbtp: Option<f64>,
    #[serde(default)]
    pub stereo_mix_integrated_lufs: Option<f64>,
    /// Identifies the known matrix/layout used for `stereo_mix_true_peak_dbtp`.
    #[serde(default)]
    pub stereo_mix_matrix: Option<String>,
    /// Input layout for the exact WebAudio-compatible stereo mix measurement.
    #[serde(default)]
    pub stereo_mix_layout: Option<String>,
    /// For album programmes, the worst member peak after a conservative
    /// channel-sum bound. This prevents programme gain from clipping any
    /// member when playback switches from the concatenated analysis to tracks.
    #[serde(default)]
    pub maximum_member_safe_true_peak_dbtp: Option<f64>,
    /// Peak bound for the native decoder's potentially different multichannel
    /// path. It uses a channel-sum bound for every multichannel member.
    #[serde(default)]
    pub maximum_member_native_safe_true_peak_dbtp: Option<f64>,
    /// True when an unknown multichannel layout required a coherent channel
    /// sum bound instead of a verified stereo matrix.
    #[serde(default)]
    pub conservative_channel_sum_bound: bool,
    /// Local file seals are cache invalidation hints. They never replace the
    /// content hash; imported records need fresh verification in the current
    /// process before playback gain is enabled.
    #[serde(default)]
    pub local_file_seals: Vec<LocalFileSeal>,
    /// Present only for Opus members actually analyzed by a capability-proven
    /// Ogg Opus decoder. Older records safely omit this proof.
    #[serde(default)]
    pub opus_decoder_proofs: Vec<OpusDecoderProof>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileSeal {
    pub asset_id: String,
    pub file_hash: String,
    pub size_bytes: u64,
    pub modified_unix_ns: Option<u64>,
    pub file_identity: Option<String>,
    /// In-memory process identity; not a durable trust token. A new process
    /// must rehash the file before accepting a restored or stale seal.
    pub verified_in_process: String,
}

impl LoudnessRecord {
    pub fn validate(&self) -> Result<(), MeasurementError> {
        self.resource.validate()?;
        if !matches!(
            self.schema_version,
            LEGACY_LOUDNESS_SCHEMA_VERSION | LOUDNESS_SCHEMA_VERSION
        ) || self.analyzer_version.trim().is_empty()
            || self.analyzer_version.len() > 128
            || self.measured_at.trim().is_empty()
            || self.channels == 0
            || self.channels > 32
            || self
                .sample_rate
                .is_some_and(|rate| !(8_000..=768_000).contains(&rate))
        {
            return Err(MeasurementError::OutOfRange);
        }
        if self.schema_version == LEGACY_LOUDNESS_SCHEMA_VERSION
            && (self.stereo_mix_integrated_lufs.is_some()
                || self.stereo_mix_true_peak_dbtp.is_some()
                || self.stereo_mix_matrix.is_some()
                || self.stereo_mix_layout.is_some()
                || self.maximum_member_safe_true_peak_dbtp.is_some()
                || self.maximum_member_native_safe_true_peak_dbtp.is_some()
                || self.conservative_channel_sum_bound)
        {
            return Err(MeasurementError::OutOfRange);
        }
        if self.opus_decoder_proofs.len() > self.resource.members.len()
            || self
                .opus_decoder_proofs
                .iter()
                .enumerate()
                .any(|(index, proof)| {
                    !is_sha256(&proof.file_hash)
                        || !is_sha256(&proof.decoder_sha256)
                        || proof.probe_schema_version != 1
                        || proof.asset_id.trim().is_empty()
                        || !self.resource.members.iter().any(|member| {
                            member.asset_id == proof.asset_id
                                && member.file_hash.eq_ignore_ascii_case(&proof.file_hash)
                        })
                        || self.opus_decoder_proofs[..index]
                            .iter()
                            .any(|previous| previous.asset_id == proof.asset_id)
                })
        {
            return Err(MeasurementError::OutOfRange);
        }
        if self.schema_version == LEGACY_LOUDNESS_SCHEMA_VERSION
            && !self.opus_decoder_proofs.is_empty()
        {
            return Err(MeasurementError::OutOfRange);
        }
        if self.true_peak_dbtp.is_some_and(|peak| {
            !peak.is_finite()
                || !(-30.0..=if self.resource.scope == ResourceScope::Album {
                    44.0
                } else {
                    12.0
                })
                    .contains(&peak)
        }) {
            return Err(MeasurementError::InvalidPeak);
        }
        match self.integrated_lufs {
            Some(lufs)
                if lufs.is_finite()
                    && (-70.0..=if self.resource.scope == ResourceScope::Album {
                        12.0
                    } else {
                        0.0
                    })
                        .contains(&lufs) => {}
            Some(_) => return Err(MeasurementError::OutOfRange),
            None if self.true_peak_dbtp.is_some() => {}
            None => return Err(MeasurementError::OutOfRange),
        }
        if !self.local_file_seals.is_empty() {
            if self.local_file_seals.len() != self.resource.members.len()
                || self.local_file_seals.iter().any(|seal| {
                    !self.resource.members.iter().any(|member| {
                        member.asset_id == seal.asset_id && member.file_hash == seal.file_hash
                    }) || seal.verified_in_process.is_empty()
                        || seal.verified_in_process.len() > 128
                })
            {
                return Err(MeasurementError::OutOfRange);
            }
        }
        if self
            .stereo_mix_true_peak_dbtp
            .is_some_and(|peak| !peak.is_finite() || !(-30.0..=44.0).contains(&peak))
            || self
                .stereo_mix_integrated_lufs
                .is_some_and(|lufs| !lufs.is_finite() || !(-70.0..=12.0).contains(&lufs))
            || self
                .maximum_member_safe_true_peak_dbtp
                .is_some_and(|peak| !peak.is_finite() || !(-30.0..=44.0).contains(&peak))
            || self
                .maximum_member_native_safe_true_peak_dbtp
                .is_some_and(|peak| !peak.is_finite() || !(-30.0..=44.0).contains(&peak))
            || (self.conservative_channel_sum_bound && self.stereo_mix_matrix.is_some())
            || (self.maximum_member_safe_true_peak_dbtp.is_some()
                != self.maximum_member_native_safe_true_peak_dbtp.is_some())
            || match (
                self.stereo_mix_integrated_lufs,
                self.stereo_mix_true_peak_dbtp,
                self.stereo_mix_matrix.as_deref(),
                self.stereo_mix_layout.as_deref(),
            ) {
                (None, None, None, None) => false,
                (Some(_), Some(_), Some("web-audio-mono-to-stereo-v1"), Some("mono")) => {
                    self.channels != 1
                }
                (Some(_), Some(_), Some("web-audio-stereo-identity-v1"), Some("stereo")) => {
                    self.channels != 2
                }
                (Some(_), Some(_), Some("web-audio-5.1-to-stereo-v1"), Some("5.1")) => {
                    self.channels != 6
                }
                (Some(_), Some(_), Some("web-audio-5.1-side-to-stereo-v1"), Some("5.1(side)")) => {
                    self.channels != 6
                }
                (Some(_), Some(_), Some("web-audio-album-program-v1"), Some(layouts))
                    if self.resource.scope == ResourceScope::Album && self.channels == 2 =>
                {
                    let values: Vec<_> = layouts
                        .strip_prefix("programme:")
                        .unwrap_or("")
                        .split(',')
                        .collect();
                    values.len() != self.resource.members.len()
                        || values.iter().any(|layout| {
                            !matches!(*layout, "mono" | "stereo" | "5.1" | "5.1(side)")
                        })
                }
                _ => true,
            }
        {
            return Err(MeasurementError::InvalidPeak);
        }
        Ok(())
    }

    /// Return the peak bound used by every playback normalization path.
    /// Unknown multichannel layouts use the worst-case coherent sum of all
    /// channels. Legacy album records lack per-member source peaks, so they
    /// cannot authorize positive gain.
    pub fn playback_true_peak_bound_dbtp(&self, web_audio_stereo_route: bool) -> Option<f64> {
        if self.resource.scope == ResourceScope::Album
            && if web_audio_stereo_route {
                self.maximum_member_safe_true_peak_dbtp.is_none()
            } else {
                self.maximum_member_native_safe_true_peak_dbtp.is_none()
            }
        {
            return None;
        }
        let mut peak = self.true_peak_dbtp;
        if web_audio_stereo_route {
            if let Some(stereo_peak) = self.stereo_mix_true_peak_dbtp {
                peak = Some(peak.map_or(stereo_peak, |value| value.max(stereo_peak)));
            }
        }
        let member_peak = if web_audio_stereo_route {
            self.maximum_member_safe_true_peak_dbtp
        } else {
            self.maximum_member_native_safe_true_peak_dbtp
        };
        if let Some(member_peak) = member_peak {
            peak = Some(peak.map_or(member_peak, |value| value.max(member_peak)));
        }
        if self.resource.scope == ResourceScope::Track
            && self.channels > 2
            && (!web_audio_stereo_route || self.stereo_mix_true_peak_dbtp.is_none())
        {
            if let Some(source_peak) = peak {
                let channel_sum_bound = source_peak + 20.0 * f64::from(self.channels).log10();
                peak = Some(peak.map_or(channel_sum_bound, |value| value.max(channel_sum_bound)));
            }
        }
        peak
    }

    pub fn playback_integrated_lufs(&self) -> Option<f64> {
        self.stereo_mix_integrated_lufs.or(self.integrated_lufs)
    }

    /// Return the integrated loudness for the output route used by playback.
    /// Mono analysis without a measured Web Audio stereo mix is ambiguous:
    /// duplicating mono into two output channels changes integrated loudness
    /// by 3.01 LU, so source LUFS must not silently authorize Web Audio gain.
    pub fn playback_integrated_lufs_for_route(&self, web_audio_stereo_route: bool) -> Option<f64> {
        if web_audio_stereo_route && self.channels == 1 {
            return self.stereo_mix_integrated_lufs;
        }
        if web_audio_stereo_route {
            self.playback_integrated_lufs()
        } else {
            self.integrated_lufs
        }
    }

    pub fn uses_conservative_channel_sum_bound(&self, web_audio_stereo_route: bool) -> bool {
        match self.resource.scope {
            ResourceScope::Track => {
                self.channels > 2 && (!web_audio_stereo_route || self.stereo_mix_matrix.is_none())
            }
            ResourceScope::Album => {
                self.conservative_channel_sum_bound
                    || (!web_audio_stereo_route
                        && self
                            .stereo_mix_layout
                            .as_deref()
                            .is_some_and(|layout| layout.contains("5.1")))
            }
        }
    }
}

/// Playback normalization mode. Persisted settings can map to this type without
/// making the policy depend on the settings schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NormalizationMode {
    Off,
    Track,
    Album,
}

impl Default for NormalizationMode {
    fn default() -> Self {
        Self::Off
    }
}

/// A measured programme value. Values are in EBU R128 units, not ReplayGain
/// tag units. `true_peak_dbtp` may be absent; without it, positive gain is
/// forbidden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessMeasurement {
    pub integrated_lufs: f64,
    pub true_peak_dbtp: Option<f64>,
}

/// User-facing normalization target and output guard rails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainPolicy {
    pub target_lufs: f64,
    pub ceiling_dbtp: f64,
    pub maximum_boost_db: f64,
}

impl Default for GainPolicy {
    fn default() -> Self {
        Self {
            target_lufs: -18.0,
            ceiling_dbtp: -1.0,
            maximum_boost_db: 6.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GainDecision {
    Disabled,
    Unavailable,
    ApplyDb(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementError {
    NonFinite,
    OutOfRange,
    InvalidPeak,
}

impl GainPolicy {
    /// Validate the small configurable operating range. The defaults are
    /// product starting points, while these bounds prevent unsafe/extreme UI
    /// or imported settings from creating large boosts.
    pub fn validate(self) -> Result<Self, MeasurementError> {
        if !self.target_lufs.is_finite()
            || !self.ceiling_dbtp.is_finite()
            || !self.maximum_boost_db.is_finite()
        {
            return Err(MeasurementError::NonFinite);
        }
        if !(-24.0..=-14.0).contains(&self.target_lufs)
            || !(-3.0..=0.0).contains(&self.ceiling_dbtp)
            || !(0.0..=6.0).contains(&self.maximum_boost_db)
        {
            return Err(MeasurementError::OutOfRange);
        }
        Ok(self)
    }
}

impl LoudnessMeasurement {
    /// Keep only plausible finite programme measurements. `-70 LUFS` is the
    /// silence/measurement floor for this policy. A measured true peak above
    /// 0 dBTP is valid data and must remain usable to request attenuation.
    pub fn validate(self) -> Result<Self, MeasurementError> {
        if !self.integrated_lufs.is_finite()
            || self.true_peak_dbtp.is_some_and(|peak| !peak.is_finite())
        {
            return Err(MeasurementError::NonFinite);
        }
        if !(-70.0..=0.0).contains(&self.integrated_lufs) {
            return Err(MeasurementError::OutOfRange);
        }
        if self
            .true_peak_dbtp
            .is_some_and(|peak| !(-30.0..=12.0).contains(&peak))
        {
            return Err(MeasurementError::InvalidPeak);
        }
        Ok(self)
    }

    pub fn decide_gain(self, policy: GainPolicy, mode: NormalizationMode) -> GainDecision {
        if self.validate().is_err() {
            return GainDecision::Unavailable;
        }
        self.decide_gain_with_output_peak_bound(self.true_peak_dbtp, policy, mode)
    }

    /// Decide gain using a separately verified output-path bound. Computed
    /// coherent channel-sum bounds may exceed the range of a raw measured
    /// true peak, so they are validated here rather than treated as source data.
    pub fn decide_gain_with_output_peak_bound(
        self,
        peak_bound_dbtp: Option<f64>,
        policy: GainPolicy,
        mode: NormalizationMode,
    ) -> GainDecision {
        if mode == NormalizationMode::Off {
            return GainDecision::Disabled;
        }
        if !self.integrated_lufs.is_finite() || !(-70.0..=12.0).contains(&self.integrated_lufs) {
            return GainDecision::Unavailable;
        }
        if peak_bound_dbtp.is_some_and(|peak| !peak.is_finite() || !(-30.0..=44.0).contains(&peak))
        {
            return GainDecision::Unavailable;
        }
        let Ok(policy) = policy.validate() else {
            return GainDecision::Unavailable;
        };

        let requested = policy.target_lufs - self.integrated_lufs;
        let gain = if let Some(peak) = peak_bound_dbtp {
            requested
                .min(policy.ceiling_dbtp - peak)
                .min(policy.maximum_boost_db)
        } else {
            // Peak-unknown tags/measurements can reduce level, never boost it.
            requested.min(0.0)
        };
        if !gain.is_finite() {
            GainDecision::Unavailable
        } else {
            GainDecision::ApplyDb(gain)
        }
    }
}

/// Parsed ReplayGain pair. ReplayGain gain is already an adjustment in dB;
/// ReplayGain peak is a linear sample-peak amplitude ratio, not dBTP or LUFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayGainTags {
    pub gain_db: f64,
    pub peak_linear: Option<f64>,
}

impl ReplayGainTags {
    pub fn parse(gain: &str, peak: Option<&str>) -> Result<Self, MeasurementError> {
        let gain_db = parse_finite_decimal(gain)?;
        if !(-60.0..=30.0).contains(&gain_db) {
            return Err(MeasurementError::OutOfRange);
        }
        let peak_linear = peak.map(parse_finite_decimal).transpose()?;
        if peak_linear.is_some_and(|value| !(0.0..=16.0).contains(&value)) {
            return Err(MeasurementError::InvalidPeak);
        }
        Ok(Self {
            gain_db,
            peak_linear,
        })
    }

    /// Apply a ReplayGain recommendation. Its peak tag is a sample peak, not a
    /// true-peak measurement, so it cannot authorize positive gain under a
    /// dBTP ceiling. Attenuation is still safe and positive gain needs a
    /// separate true-peak measurement for this exact resource.
    pub fn decide_gain(self, policy: GainPolicy, mode: NormalizationMode) -> GainDecision {
        self.decide_gain_with_true_peak(policy, mode, None)
    }

    pub fn decide_gain_with_true_peak(
        self,
        policy: GainPolicy,
        mode: NormalizationMode,
        true_peak_dbtp: Option<f64>,
    ) -> GainDecision {
        self.decide_gain_with_peak_bound(true_peak_dbtp, policy, mode, 12.0)
    }

    pub fn decide_gain_with_output_peak_bound(
        self,
        policy: GainPolicy,
        mode: NormalizationMode,
        peak_bound_dbtp: Option<f64>,
    ) -> GainDecision {
        self.decide_gain_with_peak_bound(peak_bound_dbtp, policy, mode, 44.0)
    }

    fn decide_gain_with_peak_bound(
        self,
        true_peak_dbtp: Option<f64>,
        policy: GainPolicy,
        mode: NormalizationMode,
        maximum_peak: f64,
    ) -> GainDecision {
        if mode == NormalizationMode::Off {
            return GainDecision::Disabled;
        }
        let Ok(policy) = policy.validate() else {
            return GainDecision::Unavailable;
        };
        if !self.gain_db.is_finite()
            || !(-60.0..=30.0).contains(&self.gain_db)
            || self
                .peak_linear
                .is_some_and(|peak| !peak.is_finite() || !(0.0..=16.0).contains(&peak))
            || true_peak_dbtp
                .is_some_and(|peak| !peak.is_finite() || !(-30.0..=maximum_peak).contains(&peak))
        {
            return GainDecision::Unavailable;
        }
        // ReplayGain 2.0 retains the classic -18 LUFS reference. Apply the
        // user's target as a preamp delta while preserving the tag's original
        // recommendation at the default target; this is not an EBU R128
        // measurement. See https://wiki.hydrogenaudio.org/index.php?title=ReplayGain_2.0_specification
        let requested = self.gain_db + policy.target_lufs + 18.0;
        let gain = match true_peak_dbtp {
            None if requested > 0.0 => return GainDecision::Unavailable,
            None => requested,
            Some(peak) => requested
                .min(policy.ceiling_dbtp - peak)
                .min(policy.maximum_boost_db),
        };
        GainDecision::ApplyDb(gain)
    }
}

/// Opus R128 tag values use signed Q7.8 fixed-point dB, and describe a gain
/// adjustment relative to the R128 reference programme loudness (-23 LUFS).
/// This returns that adjustment; it does not reinterpret it as measured LUFS
/// or account for any decoder-applied OpusHead output gain.
pub fn opus_r128_gain_db(raw_q78: i32) -> Result<f64, MeasurementError> {
    if !(-32_768..=32_767).contains(&raw_q78) {
        return Err(MeasurementError::OutOfRange);
    }
    let gain = f64::from(raw_q78) / 256.0;
    if !(-64.0..=64.0).contains(&gain) {
        return Err(MeasurementError::OutOfRange);
    }
    Ok(gain)
}

/// Convert an Opus R128 track/album gain into the app's requested target.
/// R128 tags are adjustments relative to -23 LUFS; the OpusHead output gain is
/// not part of this value. This helper only permits use when the caller has
/// established that the active decoder applies OpusHead gain exactly once and
/// supplies a true peak measured through a compatible decoder path.
pub fn decide_opus_r128_gain(
    raw_q78: i32,
    true_peak_dbtp_after_header_gain: Option<f64>,
    header_gain_applied_once: bool,
    policy: GainPolicy,
    mode: NormalizationMode,
) -> GainDecision {
    match opus_r128_gain_db(raw_q78) {
        Ok(tag_gain_db) => decide_opus_r128_gain_db(
            tag_gain_db,
            true_peak_dbtp_after_header_gain,
            header_gain_applied_once,
            policy,
            mode,
        ),
        Err(_) => GainDecision::Unavailable,
    }
}

/// R128 tag counterpart for callers that have already parsed and validated the
/// Q7.8 comment. The decoded true peak must include OpusHead output gain.
pub fn decide_opus_r128_gain_db(
    tag_gain_db: f64,
    true_peak_dbtp_after_header_gain: Option<f64>,
    header_gain_applied_once: bool,
    policy: GainPolicy,
    mode: NormalizationMode,
) -> GainDecision {
    if mode == NormalizationMode::Off {
        return GainDecision::Disabled;
    }
    if !header_gain_applied_once
        || !tag_gain_db.is_finite()
        || !(-64.0..=64.0).contains(&tag_gain_db)
    {
        return GainDecision::Unavailable;
    }
    let Ok(policy) = policy.validate() else {
        return GainDecision::Unavailable;
    };
    let requested = tag_gain_db + policy.target_lufs + 23.0;
    let Some(peak) = true_peak_dbtp_after_header_gain else {
        return if requested > 0.0 {
            GainDecision::Unavailable
        } else {
            GainDecision::ApplyDb(requested)
        };
    };
    if !peak.is_finite() || !(-30.0..=12.0).contains(&peak) {
        return GainDecision::Unavailable;
    }
    GainDecision::ApplyDb(
        requested
            .min(policy.ceiling_dbtp - peak)
            .min(policy.maximum_boost_db),
    )
}

/// Convert a valid linear ReplayGain sample peak to dBFS. It is not a true
/// peak (dBTP) measurement. Zero indicates silence and has no finite
/// logarithm, so it is rejected.
pub fn replaygain_peak_db(peak_linear: f64) -> Result<f64, MeasurementError> {
    if !peak_linear.is_finite() {
        return Err(MeasurementError::NonFinite);
    }
    if !(0.0..=16.0).contains(&peak_linear) || peak_linear == 0.0 {
        return Err(MeasurementError::InvalidPeak);
    }
    Ok(20.0 * peak_linear.log10())
}

fn parse_finite_decimal(value: &str) -> Result<f64, MeasurementError> {
    let value = value.trim().trim_end_matches("dB").trim();
    let parsed: f64 = value.parse().map_err(|_| MeasurementError::OutOfRange)?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err(MeasurementError::NonFinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_record(channels: u32) -> LoudnessRecord {
        LoudnessRecord {
            schema_version: LOUDNESS_SCHEMA_VERSION,
            resource: LoudnessResourceIdentity::track(
                "fixture",
                "a".repeat(64),
                AudioRange::WHOLE_FILE,
            ),
            analyzer_version: "test-v1".into(),
            provenance: MeasurementProvenance::FfmpegAnalysis,
            integrated_lufs: Some(-24.0),
            true_peak_dbtp: Some(-9.42),
            sample_rate: Some(48_000),
            channels,
            measured_at: "2026-10-27T00:00:00Z".into(),
            stereo_mix_integrated_lufs: None,
            stereo_mix_true_peak_dbtp: None,
            stereo_mix_matrix: None,
            stereo_mix_layout: None,
            maximum_member_safe_true_peak_dbtp: None,
            maximum_member_native_safe_true_peak_dbtp: None,
            conservative_channel_sum_bound: false,
            local_file_seals: vec![],
            opus_decoder_proofs: vec![],
        }
    }

    fn track_mode() -> NormalizationMode {
        NormalizationMode::Track
    }

    #[test]
    fn target_and_peak_ceiling_both_constrain_gain() {
        let normal = LoudnessMeasurement {
            integrated_lufs: -24.0,
            true_peak_dbtp: Some(-3.0),
        };
        assert_eq!(
            normal.decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::ApplyDb(2.0)
        );

        let already_loud = LoudnessMeasurement {
            integrated_lufs: -12.0,
            true_peak_dbtp: Some(0.5),
        };
        assert_eq!(
            already_loud.decide_gain(GainPolicy::default(), track_mode()),
            // Targeting -18 LUFS requests -6 dB, which also clears the
            // -1 dBTP ceiling from this +0.5 dBTP source.
            GainDecision::ApplyDb(-6.0)
        );
        let peak_limited = LoudnessMeasurement {
            integrated_lufs: -17.0,
            true_peak_dbtp: Some(0.5),
        };
        assert_eq!(
            peak_limited.decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::ApplyDb(-1.5)
        );
    }

    #[test]
    fn unknown_peak_never_allows_positive_gain() {
        let measurement = LoudnessMeasurement {
            integrated_lufs: -30.0,
            true_peak_dbtp: None,
        };
        assert_eq!(
            measurement.decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::ApplyDb(0.0)
        );
    }

    #[test]
    fn invalid_measurements_and_disabled_mode_are_safe() {
        assert_eq!(
            LoudnessMeasurement {
                integrated_lufs: f64::NAN,
                true_peak_dbtp: Some(-3.0),
            }
            .decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::Unavailable
        );
        assert_eq!(
            LoudnessMeasurement {
                integrated_lufs: -18.0,
                true_peak_dbtp: Some(-1.0),
            }
            .decide_gain(GainPolicy::default(), NormalizationMode::Off),
            GainDecision::Disabled
        );
    }

    #[test]
    fn replaygain_peak_is_linear_and_sample_peak_does_not_authorize_boost() {
        let tags = ReplayGainTags::parse("+8.0 dB", Some("0.5")).unwrap();
        assert_eq!(
            tags.decide_gain(GainPolicy::default(), track_mode()),
            // ReplayGain peak is sample peak, not true peak. It cannot
            // establish headroom for the requested positive gain.
            GainDecision::Unavailable
        );
        assert!((replaygain_peak_db(0.5).unwrap() - -6.020599913).abs() < 1e-6);
        assert_eq!(
            ReplayGainTags::parse("-4.0 dB", Some("1.5"))
                .unwrap()
                .decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::ApplyDb(-4.0)
        );
        assert!(ReplayGainTags::parse("NaN dB", Some("0.5")).is_err());
        assert!(ReplayGainTags::parse("+2 dB", Some("-0.1")).is_err());
        assert_eq!(
            ReplayGainTags::parse("-4 dB", None)
                .unwrap()
                .decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::ApplyDb(-4.0)
        );
        assert_eq!(
            ReplayGainTags::parse("+4 dB", None)
                .unwrap()
                .decide_gain(GainPolicy::default(), track_mode()),
            GainDecision::Unavailable
        );
    }

    #[test]
    fn replaygain_gain_tracks_configurable_target_without_claiming_ebu_provenance() {
        let tags = ReplayGainTags::parse("-4.0 dB", Some("1.0")).unwrap();
        let mut louder_target = GainPolicy::default();
        louder_target.target_lufs = -14.0;
        assert_eq!(
            tags.decide_gain_with_true_peak(louder_target, track_mode(), Some(-3.0)),
            GainDecision::ApplyDb(0.0)
        );

        let mut quieter_target = GainPolicy::default();
        quieter_target.target_lufs = -24.0;
        assert_eq!(
            tags.decide_gain_with_true_peak(quieter_target, track_mode(), Some(-3.0)),
            GainDecision::ApplyDb(-10.0)
        );

        let positive_at_louder_target = ReplayGainTags::parse("+2.0 dB", None).unwrap();
        assert_eq!(
            positive_at_louder_target.decide_gain(louder_target, track_mode()),
            GainDecision::Unavailable
        );
    }

    #[test]
    fn opus_r128_is_fixed_point_gain_not_integrated_loudness() {
        assert_eq!(opus_r128_gain_db(-1280).unwrap(), -5.0);
        assert_eq!(opus_r128_gain_db(256).unwrap(), 1.0);
        assert!(opus_r128_gain_db(i32::MAX).is_err());
        assert_eq!(
            decide_opus_r128_gain(-1280, Some(-5.0), true, GainPolicy::default(), track_mode(),),
            GainDecision::ApplyDb(0.0)
        );
        assert_eq!(
            decide_opus_r128_gain(
                -1280,
                Some(-5.0),
                false,
                GainPolicy::default(),
                track_mode(),
            ),
            GainDecision::Unavailable
        );
    }

    #[test]
    fn programme_identity_binds_member_order_and_explicit_gaps() {
        let one = LoudnessAssetIdentity {
            asset_id: "a".into(),
            file_hash: "hash-a".into(),
            range: AudioRange::WHOLE_FILE,
        };
        let two = LoudnessAssetIdentity {
            asset_id: "b".into(),
            file_hash: "hash-b".into(),
            range: AudioRange::WHOLE_FILE,
        };
        let album = LoudnessResourceIdentity::album(vec![one.clone(), two.clone()], vec![2.5]);
        assert!(album.validate().is_ok());
        assert_ne!(
            album.resource_key,
            LoudnessResourceIdentity::album(vec![two, one], vec![2.5]).resource_key
        );
        assert_ne!(
            album.resource_key,
            LoudnessResourceIdentity::album(album.members.clone(), vec![0.0]).resource_key
        );
        let forged = LoudnessResourceIdentity {
            resource_key: "0".repeat(64),
            ..album
        };
        assert!(forged.validate().is_err());
    }

    #[test]
    fn track_resource_identity_binds_playback_range() {
        let full = LoudnessResourceIdentity::track("asset-a", "hash-a", AudioRange::WHOLE_FILE);
        let excerpt = LoudnessResourceIdentity::track(
            "asset-a",
            "hash-a",
            AudioRange {
                start_seconds: 30.0,
                end_seconds: Some(60.0),
            },
        );

        assert!(full.validate().is_ok());
        assert!(excerpt.validate().is_ok());
        assert_ne!(full.resource_key, excerpt.resource_key);
        assert_ne!(full, excerpt);
    }

    #[test]
    fn trusted_true_peak_measurement_can_limit_tag_gain() {
        let tags = ReplayGainTags::parse("+8.0 dB", Some("0.5")).unwrap();
        assert_eq!(
            tags.decide_gain_with_true_peak(GainPolicy::default(), track_mode(), Some(-4.0)),
            GainDecision::ApplyDb(3.0)
        );
    }

    #[test]
    fn unknown_multichannel_route_uses_coherent_channel_sum_bound() {
        let record = test_record(6);
        let bound = record.playback_true_peak_bound_dbtp(false).unwrap();
        assert!((bound - (-9.42 + 20.0 * 6.0_f64.log10())).abs() < 0.001);
        let GainDecision::ApplyDb(gain) = LoudnessMeasurement {
            integrated_lufs: record.integrated_lufs.unwrap(),
            true_peak_dbtp: None,
        }
        .decide_gain_with_output_peak_bound(
            Some(bound),
            GainPolicy::default(),
            NormalizationMode::Track,
        ) else {
            panic!("expected conservative attenuation");
        };
        assert!((gain - (GainPolicy::default().ceiling_dbtp - bound)).abs() < 0.001);
        assert_eq!(
            LoudnessMeasurement {
                integrated_lufs: -24.0,
                true_peak_dbtp: Some(13.0),
            }
            .validate(),
            Err(MeasurementError::InvalidPeak)
        );
    }

    #[test]
    fn verified_stereo_mix_metric_avoids_unknown_matrix_fallback() {
        let mut record = test_record(6);
        record.stereo_mix_integrated_lufs = Some(-20.65);
        record.stereo_mix_true_peak_dbtp = Some(-1.87);
        record.stereo_mix_matrix = Some("web-audio-5.1-to-stereo-v1".into());
        record.stereo_mix_layout = Some("5.1".into());
        assert!(record.validate().is_ok());
        assert_eq!(record.playback_true_peak_bound_dbtp(true), Some(-1.87));
        assert_eq!(
            record.playback_true_peak_bound_dbtp(false),
            Some(-9.42 + 20.0 * 6.0_f64.log10())
        );
        assert_eq!(record.playback_integrated_lufs(), Some(-20.65));
        assert!(!record.uses_conservative_channel_sum_bound(true));

        record.stereo_mix_matrix = Some("unverified-matrix".into());
        assert!(record.validate().is_err());
    }

    #[test]
    fn mono_analysis_requires_stereo_route_metric_for_web_audio_gain() {
        let mut record = test_record(1);
        record.integrated_lufs = Some(-16.67);
        record.true_peak_dbtp = Some(-5.90);

        assert_eq!(
            record.playback_integrated_lufs_for_route(false),
            Some(-16.67)
        );
        assert_eq!(record.playback_integrated_lufs_for_route(true), None);

        record.stereo_mix_integrated_lufs = Some(-13.67);
        record.stereo_mix_true_peak_dbtp = Some(-5.90);
        record.stereo_mix_matrix = Some("web-audio-mono-to-stereo-v1".into());
        record.stereo_mix_layout = Some("mono".into());
        assert!(record.validate().is_ok());
        assert_eq!(
            record.playback_integrated_lufs_for_route(true),
            Some(-13.67)
        );
        let GainDecision::ApplyDb(gain) = LoudnessMeasurement {
            integrated_lufs: record.playback_integrated_lufs_for_route(true).unwrap(),
            true_peak_dbtp: None,
        }
        .decide_gain_with_output_peak_bound(
            record.playback_true_peak_bound_dbtp(true),
            GainPolicy::default(),
            NormalizationMode::Track,
        ) else {
            panic!("expected the route-specific mono recommendation");
        };
        assert!((gain - (-4.33)).abs() < 0.001);
        assert_eq!(
            record.playback_integrated_lufs_for_route(false),
            Some(-16.67)
        );
    }

    #[test]
    fn legacy_album_measurement_without_member_bounds_cannot_boost() {
        let mut record = test_record(2);
        let first = LoudnessAssetIdentity {
            asset_id: "one".into(),
            file_hash: "b".repeat(64),
            range: AudioRange::WHOLE_FILE,
        };
        let second = LoudnessAssetIdentity {
            asset_id: "two".into(),
            file_hash: "c".repeat(64),
            range: AudioRange::WHOLE_FILE,
        };
        record.resource = LoudnessResourceIdentity::album(vec![first, second], vec![0.0]);
        assert_eq!(record.playback_true_peak_bound_dbtp(true), None);
        record.maximum_member_safe_true_peak_dbtp = Some(-3.0);
        assert_eq!(record.playback_true_peak_bound_dbtp(true), Some(-3.0));
        record.maximum_member_native_safe_true_peak_dbtp = Some(2.0);
        assert_eq!(record.playback_true_peak_bound_dbtp(false), Some(2.0));
    }

    #[test]
    fn old_records_without_output_provenance_still_deserialize() {
        let mut value = serde_json::to_value(test_record(2)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert(
            "schemaVersion".into(),
            serde_json::Value::from(LEGACY_LOUDNESS_SCHEMA_VERSION),
        );
        object.remove("stereoMixTruePeakDbtp");
        object.remove("stereoMixIntegratedLufs");
        object.remove("stereoMixMatrix");
        object.remove("stereoMixLayout");
        object.remove("maximumMemberSafeTruePeakDbtp");
        object.remove("maximumMemberNativeSafeTruePeakDbtp");
        object.remove("conservativeChannelSumBound");
        let restored: LoudnessRecord = serde_json::from_value(value).unwrap();
        assert!(restored.validate().is_ok());
        assert_eq!(restored.stereo_mix_true_peak_dbtp, None);
    }

    #[test]
    fn legacy_multichannel_schema_uses_only_conservative_peak_fallback() {
        let mut record = test_record(6);
        record.schema_version = LEGACY_LOUDNESS_SCHEMA_VERSION;
        assert!(record.validate().is_ok());
        assert_eq!(
            record.playback_true_peak_bound_dbtp(true),
            Some(-9.42 + 20.0 * 6.0_f64.log10())
        );
        record.stereo_mix_integrated_lufs = Some(-20.0);
        assert!(record.validate().is_err());
    }

    #[test]
    fn unknown_loudness_schema_versions_are_rejected() {
        let mut record = test_record(2);
        record.schema_version = LOUDNESS_SCHEMA_VERSION + 1;
        assert!(record.validate().is_err());
    }
}
