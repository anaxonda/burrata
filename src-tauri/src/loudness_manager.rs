//! Loudness settings, source-status reporting and measurement selection.

use crate::{loudness::*, storage::Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::watch;

pub const SETTINGS_KEY: &str = "loudness.v1";
pub const ANALYSIS_CACHE_VERSION: &str = "loudness-ebur128-output-mix-v2";
pub const ALBUM_ANALYSIS_CACHE_VERSION: &str = "loudness-ebur128-album-output-mix-v2";

const LEGACY_ANALYSIS_CACHE_VERSION: &str = "loudness-ebur128-v1";
const LEGACY_ALBUM_ANALYSIS_CACHE_VERSION: &str = "loudness-ebur128-album-programme-v1";
const MAX_ACTIVE_ANALYSIS_JOBS: usize = 2;

fn analysis_cache_version(scope: ResourceScope) -> &'static str {
    match scope {
        ResourceScope::Track => ANALYSIS_CACHE_VERSION,
        ResourceScope::Album => ALBUM_ANALYSIS_CACHE_VERSION,
    }
}

fn cancel_requested(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessAnalysisJobStatus {
    pub job_id: String,
    pub status: String,
    pub total: usize,
    pub completed: usize,
    pub error: Option<String>,
    #[serde(default)]
    pub resource: Option<LoudnessResourceIdentity>,
}

struct LoudnessAnalysisJob {
    status: Arc<Mutex<LoudnessAnalysisJobStatus>>,
    cancel: watch::Sender<bool>,
}

/// Bounded in-memory lifecycle for explicitly selected loudness analyses.
/// Jobs are not persisted; completed measurements are stored in asset_analyses.
#[derive(Default)]
pub struct LoudnessAnalysisJobs {
    jobs: Mutex<HashMap<String, LoudnessAnalysisJob>>,
}

impl LoudnessAnalysisJobs {
    pub fn insert(
        &self,
        status: LoudnessAnalysisJobStatus,
        cancel: watch::Sender<bool>,
    ) -> Result<(), String> {
        let job_id = status.job_id.clone();
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| "Loudness analysis job registry is unavailable")?;
        let active = jobs
            .values()
            .filter(|job| {
                job.status.lock().map_or(true, |status| {
                    matches!(status.status.as_str(), "queued" | "running" | "cancelling")
                })
            })
            .count();
        if active >= MAX_ACTIVE_ANALYSIS_JOBS {
            return Err(
                "Finish or cancel another loudness analysis before starting a new one".into(),
            );
        }
        if jobs.len() >= 32 {
            let finished = jobs.iter().find_map(|(id, job)| {
                job.status.lock().ok().and_then(|status| {
                    matches!(status.status.as_str(), "completed" | "cancelled" | "failed")
                        .then(|| id.clone())
                })
            });
            if let Some(finished) = finished {
                jobs.remove(&finished);
            } else {
                return Err("Too many loudness analysis jobs are already active".into());
            }
        }
        jobs.insert(
            job_id,
            LoudnessAnalysisJob {
                status: Arc::new(Mutex::new(status)),
                cancel,
            },
        );
        Ok(())
    }

    pub fn status(&self, job_id: &str) -> Result<LoudnessAnalysisJobStatus, String> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "Loudness analysis job registry is unavailable")?;
        let job = jobs
            .get(job_id)
            .ok_or("Loudness analysis job was not found")?;
        job.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| "Loudness analysis job status is unavailable".into())
    }

    pub fn update(
        &self,
        job_id: &str,
        update: impl FnOnce(&mut LoudnessAnalysisJobStatus),
    ) -> Result<(), String> {
        let status = {
            let jobs = self
                .jobs
                .lock()
                .map_err(|_| "Loudness analysis job registry is unavailable")?;
            jobs.get(job_id)
                .ok_or("Loudness analysis job was not found")?
                .status
                .clone()
        };
        let mut guard = status
            .lock()
            .map_err(|_| "Loudness analysis job status is unavailable")?;
        update(&mut guard);
        Ok(())
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "Loudness analysis job registry is unavailable")?;
        jobs.get(job_id)
            .ok_or("Loudness analysis job was not found")?
            .cancel
            .send(true)
            .map_err(|_| "Loudness analysis job has already stopped".into())
    }
}

#[derive(Default)]
pub struct LocalVerificationMemo {
    seals: Mutex<HashMap<String, Vec<LocalFileSeal>>>,
    tags: Mutex<HashMap<String, crate::loudness_analysis::LocalTagMeasurements>>,
    tag_verification_in_flight: Mutex<HashSet<String>>,
}

impl LocalVerificationMemo {
    // One active track and one prepared successor may need tag verification.
    const MAX_TAG_VERIFICATIONS_IN_FLIGHT: usize = 2;

    /// O(1) cache use is allowed only when this process already cryptographically
    /// verified the exact member hashes and the current file stat still matches.
    pub fn is_currently_verified(
        &self,
        store: &Store,
        resource: &LoudnessResourceIdentity,
        record: Option<&LoudnessRecord>,
    ) -> Result<bool, String> {
        resource
            .validate()
            .map_err(|_| "Loudness resource identity is invalid".to_owned())?;
        let current = current_seals(store, resource)?;
        if let Some(memoized) = self
            .seals
            .lock()
            .map_err(|_| "Loudness verification cache is unavailable")?
            .get(&resource.resource_key)
        {
            if seals_match(memoized, &current) {
                return Ok(true);
            }
        }
        if let Some(record) = record {
            if record.local_file_seals.len() == current.len()
                && record
                    .local_file_seals
                    .iter()
                    .all(|seal| seal.verified_in_process == process_verification_id())
                && seals_match(&record.local_file_seals, &current)
            {
                self.seals
                    .lock()
                    .map_err(|_| "Loudness verification cache is unavailable")?
                    .insert(resource.resource_key.clone(), current);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn cached_tags(
        &self,
        store: &Store,
        resource: &LoudnessResourceIdentity,
    ) -> Result<Option<crate::loudness_analysis::LocalTagMeasurements>, String> {
        if !self.is_currently_verified(store, resource, None)? {
            return Ok(None);
        }
        Ok(self
            .tags
            .lock()
            .map_err(|_| "Loudness tag cache is unavailable")?
            .get(&resource.resource_key)
            .cloned())
    }

    /// Reserve a single background tag read for a current whole-file track.
    /// Same-resource requests coalesce, and rapid track changes cannot create
    /// an unbounded set of file readers.
    pub fn begin_tag_verification(
        &self,
        store: &Store,
        resource: &LoudnessResourceIdentity,
    ) -> Result<bool, String> {
        resource
            .validate()
            .map_err(|_| "Loudness resource identity is invalid".to_owned())?;
        if resource.scope != ResourceScope::Track
            || resource.members.len() != 1
            || resource.members[0].range != AudioRange::WHOLE_FILE
        {
            return Ok(false);
        }
        if self.cached_tags(store, resource)?.is_some() {
            return Ok(false);
        }
        let mut in_flight = self
            .tag_verification_in_flight
            .lock()
            .map_err(|_| "Loudness tag verification queue is unavailable")?;
        if in_flight.contains(&resource.resource_key)
            || in_flight.len() >= Self::MAX_TAG_VERIFICATIONS_IN_FLIGHT
        {
            return Ok(false);
        }
        in_flight.insert(resource.resource_key.clone());
        Ok(true)
    }

    pub fn finish_tag_verification(&self, resource: &LoudnessResourceIdentity) {
        if let Ok(mut in_flight) = self.tag_verification_in_flight.lock() {
            in_flight.remove(&resource.resource_key);
        }
    }

    pub async fn prepare_local_tags(
        &self,
        store: Arc<Store>,
        resource: LoudnessResourceIdentity,
        mut cancel: Option<watch::Receiver<bool>>,
    ) -> Result<(), String> {
        if resource.scope != ResourceScope::Track || resource.members.len() != 1 {
            return Err("Tag verification currently supports one local track at a time".into());
        }
        let asset = assets_for_resource(&store, &resource)?
            .into_iter()
            .next()
            .ok_or("Local loudness asset is unavailable")?;
        if let Some(seal) = self
            .seals
            .lock()
            .map_err(|_| "Loudness verification cache is unavailable")?
            .get(&resource.resource_key)
            .and_then(|seals| seals.first())
            .cloned()
        {
            if current_seals(&store, &resource)
                .ok()
                .is_some_and(|current| seals_match(&[seal], &current))
                && self
                    .tags
                    .lock()
                    .map_err(|_| "Loudness tag cache is unavailable")?
                    .contains_key(&resource.resource_key)
            {
                return Ok(());
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            if worker_cancel.load(Ordering::Acquire) {
                return Err("Loudness tag verification was cancelled".to_owned());
            }
            crate::loudness_analysis::read_local_tags_cancellable(&asset, &worker_cancel)
                .map_err(|error| error.to_string())
        });
        let tags = if let Some(cancel) = cancel.as_mut() {
            tokio::select! {
                biased;
                _ = cancel.wait_for(|value| *value) => {
                    cancelled.store(true, Ordering::Release);
                    return Err("Loudness tag verification was cancelled".into());
                }
                result = &mut worker => result
                    .map_err(|_| "Loudness tag worker stopped".to_owned())??,
            }
        } else {
            (&mut worker)
                .await
                .map_err(|_| "Loudness tag worker stopped".to_owned())??
        };
        let seal = tags
            .file_seal
            .clone()
            .ok_or("Local loudness tag metadata has not been verified")?;
        if !seals_match(
            std::slice::from_ref(&seal),
            &current_seals(&store, &resource)?,
        ) {
            return Err("Audio changed during loudness tag verification".into());
        }
        self.seals
            .lock()
            .map_err(|_| "Loudness verification cache is unavailable")?
            .insert(resource.resource_key.clone(), vec![seal]);
        self.tags
            .lock()
            .map_err(|_| "Loudness tag cache is unavailable")?
            .insert(resource.resource_key, tags);
        Ok(())
    }

    /// Verify selected local member hashes off the async/runtime/audio path.
    /// A fresh run is required after process restart, restore, or a stat change.
    pub async fn verify_and_remember(
        &self,
        store: Arc<Store>,
        resource: LoudnessResourceIdentity,
        mut cancel: Option<watch::Receiver<bool>>,
    ) -> Result<(), String> {
        resource
            .validate()
            .map_err(|_| "Loudness resource identity is invalid".to_owned())?;
        let assets = assets_for_resource(&store, &resource)?;
        if let Ok(current) = current_seals(&store, &resource) {
            if self
                .seals
                .lock()
                .map_err(|_| "Loudness verification cache is unavailable")?
                .get(&resource.resource_key)
                .is_some_and(|memoized| seals_match(memoized, &current))
            {
                return Ok(());
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            let mut seals = Vec::with_capacity(assets.len());
            for asset in &assets {
                if worker_cancel.load(Ordering::Acquire) {
                    return Err("Loudness verification was cancelled".to_owned());
                }
                let seal = crate::loudness_analysis::verify_local_asset_seal(asset, &worker_cancel)
                    .map_err(|error| error.to_string())?;
                seals.push(seal);
            }
            Ok(seals)
        });
        let seals = if let Some(cancel) = cancel.as_mut() {
            tokio::select! {
                biased;
                _ = cancel.wait_for(|value| *value) => {
                    cancelled.store(true, Ordering::Release);
                    return Err("Loudness verification was cancelled".into());
                }
                result = &mut worker => result
                    .map_err(|_| "Loudness verification worker stopped".to_owned())??,
            }
        } else {
            (&mut worker)
                .await
                .map_err(|_| "Loudness verification worker stopped".to_owned())??
        };
        let still_current = current_seals(&store, &resource)?;
        if !seals_match(&seals, &still_current) {
            return Err("Audio changed during loudness verification".into());
        }
        self.seals
            .lock()
            .map_err(|_| "Loudness verification cache is unavailable".to_owned())?
            .insert(resource.resource_key, seals);
        Ok(())
    }
}

async fn analysis_decoder_for_asset(
    asset: &crate::models::LocalAsset,
    ffprobe: &std::path::Path,
    ffmpeg: &std::path::Path,
    cancel: &mut watch::Receiver<bool>,
) -> Result<
    (
        std::path::PathBuf,
        Option<crate::loudness::OpusDecoderProof>,
    ),
    String,
> {
    let extension_may_be_opus = std::path::Path::new(&asset.path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ogg" | "opus" | "webm"
            )
        });
    if !asset.final_codec.eq_ignore_ascii_case("opus") && !extension_may_be_opus {
        return Ok((ffmpeg.to_owned(), None));
    }
    let Some(is_ogg_opus) = crate::opus_decoder_capability::probe_ogg_opus(
        ffprobe,
        std::path::Path::new(&asset.path),
        cancel,
    )
    .await
    else {
        return Err(if cancel_requested(cancel) {
            "Loudness analysis was cancelled".into()
        } else {
            "Could not verify the selected local audio codec".into()
        });
    };
    if !is_ogg_opus {
        return Ok((ffmpeg.to_owned(), None));
    }
    let Some(qualified) =
        crate::opus_decoder_capability::wait_for_qualified_decoder(ffmpeg, cancel).await
    else {
        return Err(if cancel_requested(cancel) {
            "Loudness analysis was cancelled".into()
        } else {
            "The configured decoder could not verify Ogg Opus header-gain handling".into()
        });
    };
    Ok((
        qualified.executable_path,
        Some(crate::loudness::OpusDecoderProof {
            asset_id: asset.id.clone(),
            file_hash: asset.hash.clone(),
            decoder_sha256: qualified.executable_sha256,
            probe_schema_version: qualified.probe_schema_version,
        }),
    ))
}

/// Analyze only explicitly selected local asset IDs. Work is serial, cancellable,
/// resource bounded in `loudness_analysis`, and never queues the whole library.
pub async fn analyze_selected_assets(
    store: Arc<Store>,
    asset_ids: Vec<String>,
    ffmpeg: std::path::PathBuf,
    ffprobe: std::path::PathBuf,
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<LoudnessRecord>, String> {
    if asset_ids.is_empty() || asset_ids.len() > 50 {
        return Err("Select 1–50 local audio assets for loudness analysis".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut records = Vec::new();
    for asset_id in asset_ids {
        if cancel_requested(&cancel) {
            return Err("Loudness analysis was cancelled".into());
        }
        if !seen.insert(asset_id.clone()) {
            continue;
        }
        let asset = store
            .get_asset_by_id(&asset_id)
            .map_err(|error| error.to_string())?
            .ok_or("Selected local audio asset is no longer registered")?;
        let (worker_ffmpeg, opus_decoder_proof) =
            analysis_decoder_for_asset(&asset, &ffprobe, &ffmpeg, &mut cancel).await?;
        let worker_asset = asset.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            crate::loudness_analysis::analyze_local_asset(
                &worker_asset,
                &worker_ffmpeg,
                worker_cancel,
                opus_decoder_proof,
            )
            .map_err(|error| error.to_string())
        });
        let record = tokio::select! {
            biased;
            _ = cancel.wait_for(|value| *value) => {
                cancelled.store(true, Ordering::Release);
                return Err("Loudness analysis was cancelled".into());
            }
            result = &mut worker => result
                .map_err(|_| "Loudness analysis worker stopped".to_owned())??,
        };
        save_record(&store, &record)?;
        records.push(record);
    }
    Ok(records)
}

/// Analyze a caller-selected, ordered programme album using gapless boundaries,
/// matching queue playback. No library scan or inferred membership occurs.
pub async fn analyze_selected_album(
    store: Arc<Store>,
    asset_ids: Vec<String>,
    inter_track_gaps_seconds: Vec<f64>,
    ffmpeg: std::path::PathBuf,
    ffprobe: std::path::PathBuf,
    mut cancel: watch::Receiver<bool>,
) -> Result<LoudnessRecord, String> {
    if !(2..=200).contains(&asset_ids.len())
        || inter_track_gaps_seconds.len() + 1 != asset_ids.len()
        || inter_track_gaps_seconds
            .iter()
            .any(|gap| !gap.is_finite() || *gap != 0.0)
    {
        return Err("Select 2–200 ordered tracks with zero gaps between queue transitions".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut assets = Vec::with_capacity(asset_ids.len());
    for asset_id in asset_ids {
        if !seen.insert(asset_id.clone()) {
            return Err("An album programme cannot contain the same local asset twice".into());
        }
        let asset = store
            .get_asset_by_id(&asset_id)
            .map_err(|error| error.to_string())?
            .ok_or("Selected local audio asset is no longer registered")?;
        assets.push(asset);
    }
    let mut proofs = Vec::new();
    let mut worker_ffmpeg = ffmpeg.clone();
    let mut decoder_sha = None::<String>;
    for asset in &assets {
        let (selected_ffmpeg, proof) =
            analysis_decoder_for_asset(asset, &ffprobe, &ffmpeg, &mut cancel).await?;
        if let Some(proof) = proof {
            if decoder_sha
                .as_ref()
                .is_some_and(|sha| !sha.eq_ignore_ascii_case(&proof.decoder_sha256))
            {
                return Err("The configured decoder changed during album analysis".into());
            }
            decoder_sha = Some(proof.decoder_sha256.clone());
            worker_ffmpeg = selected_ffmpeg;
            proofs.push(proof);
        }
    }
    let worker_assets = assets;
    let worker_gaps = inter_track_gaps_seconds;
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancelled.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        crate::loudness_analysis::analyze_local_album(
            &worker_assets,
            &worker_gaps,
            &worker_ffmpeg,
            worker_cancel,
            proofs,
        )
        .map_err(|error| error.to_string())
    });
    let record = tokio::select! {
        biased;
        _ = cancel.wait_for(|value| *value) => {
            cancelled.store(true, Ordering::Release);
            return Err("Loudness analysis was cancelled".into());
        }
        result = &mut worker => result
            .map_err(|_| "Loudness analysis worker stopped".to_owned())??,
    };
    save_record(&store, &record)?;
    Ok(record)
}

fn assets_for_resource(
    store: &Store,
    resource: &LoudnessResourceIdentity,
) -> Result<Vec<crate::models::LocalAsset>, String> {
    resource
        .members
        .iter()
        .map(|member| {
            let asset = store
                .get_asset_by_id(&member.asset_id)
                .map_err(|error| error.to_string())?
                .ok_or("A loudness resource asset is no longer registered")?;
            if asset.hash != member.file_hash {
                return Err("Registered audio changed since loudness analysis".into());
            }
            Ok(asset)
        })
        .collect()
}

fn current_seals(
    store: &Store,
    resource: &LoudnessResourceIdentity,
) -> Result<Vec<LocalFileSeal>, String> {
    assets_for_resource(store, resource)?
        .iter()
        .zip(resource.members.iter())
        .map(|(asset, member)| {
            crate::loudness_analysis::local_file_seal(
                std::path::Path::new(&asset.path),
                &member.asset_id,
                &member.file_hash,
            )
            .map_err(|error| error.to_string())
        })
        .collect()
}

fn seals_match(expected: &[LocalFileSeal], current: &[LocalFileSeal]) -> bool {
    expected.len() == current.len()
        && expected.iter().zip(current).all(|(expected, current)| {
            expected.asset_id == current.asset_id
                && expected.file_hash == current.file_hash
                && expected.size_bytes == current.size_bytes
                && expected.modified_unix_ns == current.modified_unix_ns
                && expected.file_identity == current.file_identity
                && expected.verified_in_process == process_verification_id()
                && current.verified_in_process == process_verification_id()
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessSettings {
    #[serde(default)]
    pub mode: NormalizationMode,
    #[serde(default = "default_target_lufs")]
    pub target_lufs: f64,
    #[serde(default = "default_ceiling_dbtp")]
    pub ceiling_dbtp: f64,
    #[serde(default = "default_maximum_boost_db")]
    pub maximum_boost_db: f64,
    #[serde(default = "default_album_track_fallback")]
    pub album_track_fallback: bool,
    #[serde(default)]
    pub analyze_when_idle: bool,
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            mode: NormalizationMode::Off,
            target_lufs: default_target_lufs(),
            ceiling_dbtp: default_ceiling_dbtp(),
            maximum_boost_db: default_maximum_boost_db(),
            album_track_fallback: default_album_track_fallback(),
            analyze_when_idle: false,
        }
    }
}

impl LoudnessSettings {
    pub fn gain_policy(&self) -> Result<GainPolicy, MeasurementError> {
        GainPolicy {
            target_lufs: self.target_lufs,
            ceiling_dbtp: self.ceiling_dbtp,
            maximum_boost_db: self.maximum_boost_db,
        }
        .validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessStatusKind {
    Off,
    Applied,
    AlbumFallbackTrack,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessStatus {
    pub mode: NormalizationMode,
    pub status: LoudnessStatusKind,
    pub applied_gain_db: f64,
    pub measurement_source: Option<MeasurementProvenance>,
    pub reason: Option<String>,
    pub source_resource_key: Option<String>,
}

impl LoudnessStatus {
    pub fn off() -> Self {
        Self {
            mode: NormalizationMode::Off,
            status: LoudnessStatusKind::Off,
            applied_gain_db: 0.0,
            measurement_source: None,
            reason: None,
            source_resource_key: None,
        }
    }

    pub fn unavailable(
        mode: NormalizationMode,
        reason: impl Into<String>,
        source_resource_key: Option<String>,
    ) -> Self {
        Self {
            mode,
            status: LoudnessStatusKind::Unavailable,
            applied_gain_db: 0.0,
            measurement_source: None,
            reason: Some(reason.into()),
            source_resource_key,
        }
    }
}

/// Resource-bound gain alternatives for native playback. The output worker
/// selects a route only after opening the actual device, before it queues PCM.
#[derive(Debug, Clone)]
pub struct NativeNormalizationGainPlan {
    input_channels: Option<u16>,
    source_route: LoudnessStatus,
    mono_to_stereo_route: Option<LoudnessStatus>,
}

impl NativeNormalizationGainPlan {
    pub fn fixed(status: LoudnessStatus) -> Self {
        Self {
            input_channels: None,
            source_route: status,
            mono_to_stereo_route: None,
        }
    }

    pub fn mono(
        source_route: LoudnessStatus,
        mono_to_stereo_route: Option<LoudnessStatus>,
    ) -> Self {
        Self {
            input_channels: Some(1),
            source_route,
            mono_to_stereo_route,
        }
    }

    pub fn select(&self, output_channels: u16) -> LoudnessStatus {
        if self.source_route.mode == NormalizationMode::Off {
            return self.source_route.clone();
        }
        if self.input_channels != Some(1) {
            return self.source_route.clone();
        }
        match output_channels {
            1 => self.source_route.clone(),
            2 => self.mono_to_stereo_route.clone().unwrap_or_else(|| {
                LoudnessStatus::unavailable(
                    self.source_route.mode,
                    "Analyze this mono track again for stereo playback",
                    self.source_route.source_resource_key.clone(),
                )
            }),
            _ => LoudnessStatus::unavailable(
                self.source_route.mode,
                "Normalization is unavailable for this native mono output layout",
                self.source_route.source_resource_key.clone(),
            ),
        }
    }

    pub fn pending_status(&self) -> LoudnessStatus {
        if self.source_route.mode == NormalizationMode::Off {
            LoudnessStatus::off()
        } else {
            LoudnessStatus::unavailable(
                self.source_route.mode,
                "Selecting normalization for the native audio output",
                self.source_route.source_resource_key.clone(),
            )
        }
    }
}

/// Load persisted values while treating malformed/legacy rows as Off. The
/// setting is independent of the broad application settings struct.
pub fn load_settings(store: &Store) -> Result<LoudnessSettings, String> {
    let Some(value) = store
        .get_json_preference(SETTINGS_KEY)
        .map_err(|error| error.to_string())?
    else {
        return Ok(LoudnessSettings::default());
    };
    let settings: LoudnessSettings = serde_json::from_value(value)
        .map_err(|_| "Saved loudness settings are invalid; normalization is off".to_owned())?;
    settings
        .gain_policy()
        .map_err(|_| "Saved loudness settings are outside the supported range".to_owned())?;
    Ok(settings)
}

pub fn save_settings(store: &Store, settings: &LoudnessSettings) -> Result<(), String> {
    settings
        .gain_policy()
        .map_err(|_| "Loudness target or safety limits are invalid".to_owned())?;
    let value = serde_json::to_value(settings).map_err(|error| error.to_string())?;
    store
        .set_json_preference(SETTINGS_KEY, &value)
        .map_err(|error| error.to_string())
}

pub fn cached_record(
    store: &Store,
    expected_resource: &LoudnessResourceIdentity,
    expected_analyzer_version: Option<&str>,
) -> Result<Option<LoudnessRecord>, String> {
    expected_resource
        .validate()
        .map_err(|_| "Loudness resource identity is invalid".to_owned())?;
    for member in &expected_resource.members {
        let current = store
            .get_asset_by_id(&member.asset_id)
            .map_err(|error| error.to_string())?
            .ok_or("A loudness resource asset is no longer registered")?;
        if current.hash != member.file_hash {
            return Ok(None);
        }
    }
    let ids = expected_resource
        .members
        .iter()
        .map(|member| member.asset_id.clone())
        .collect::<Vec<_>>();
    let rows = store
        .list_analysis_records_for_assets(&ids)
        .map_err(|error| error.to_string())?;
    let current_version = analysis_cache_version(expected_resource.scope);
    let legacy_version = match expected_resource.scope {
        ResourceScope::Track => LEGACY_ANALYSIS_CACHE_VERSION,
        ResourceScope::Album => LEGACY_ALBUM_ANALYSIS_CACHE_VERSION,
    };
    for preferred_version in [current_version, legacy_version] {
        for row in &rows {
            let row_version = row.get("version").and_then(Value::as_str);
            if row_version != Some(preferred_version)
                || row.get("status").and_then(Value::as_str) != Some("completed")
            {
                continue;
            }
            let anchor_id = row.get("assetId").and_then(Value::as_str);
            let anchor_hash = row.get("inputHash").and_then(Value::as_str);
            if !expected_resource.members.iter().any(|member| {
                Some(member.asset_id.as_str()) == anchor_id
                    && Some(member.file_hash.as_str()) == anchor_hash
            }) {
                continue;
            }
            let Some(payload) = row.get("payload") else {
                continue;
            };
            let Ok(record) = serde_json::from_value::<LoudnessRecord>(payload.clone()) else {
                continue;
            };
            if (row_version == Some(legacy_version)
                && record.schema_version != LEGACY_LOUDNESS_SCHEMA_VERSION)
                || (row_version == Some(current_version)
                    && record.schema_version != LOUDNESS_SCHEMA_VERSION)
            {
                continue;
            }
            if record.validate().is_err()
                || record.resource != *expected_resource
                || expected_analyzer_version
                    .is_some_and(|version| record.analyzer_version != version)
            {
                continue;
            }
            return Ok(Some(record));
        }
    }
    Ok(None)
}

pub fn save_record(store: &Store, record: &LoudnessRecord) -> Result<(), String> {
    record
        .validate()
        .map_err(|_| "Loudness measurement is invalid".to_owned())?;
    let Some(anchor) = record.resource.members.first() else {
        return Err("Loudness record has no resource members".into());
    };
    for member in &record.resource.members {
        let current = store
            .get_asset_by_id(&member.asset_id)
            .map_err(|error| error.to_string())?
            .ok_or("A loudness resource asset is no longer registered")?;
        if current.hash != member.file_hash {
            return Err("Audio changed while loudness analysis was running".into());
        }
    }
    let payload = serde_json::to_value(record).map_err(|error| error.to_string())?;
    store
        .save_analysis_record(
            &anchor.asset_id,
            &anchor.file_hash,
            analysis_cache_version(record.resource.scope),
            "completed",
            &payload,
        )
        .map_err(|error| error.to_string())
}

pub fn decide_for_measurements(
    settings: &LoudnessSettings,
    track: Option<&LoudnessRecord>,
    album: Option<&LoudnessRecord>,
    resource_key: Option<String>,
) -> LoudnessStatus {
    if settings.mode == NormalizationMode::Off {
        return LoudnessStatus::off();
    }
    let Ok(policy) = settings.gain_policy() else {
        return LoudnessStatus::unavailable(
            settings.mode,
            "Loudness settings are invalid",
            resource_key,
        );
    };

    let (record, status) = match settings.mode {
        NormalizationMode::Off => return LoudnessStatus::off(),
        NormalizationMode::Track => (track, LoudnessStatusKind::Applied),
        NormalizationMode::Album => match album {
            Some(record) => (Some(record), LoudnessStatusKind::Applied),
            None if settings.album_track_fallback && track.is_some() => {
                (track, LoudnessStatusKind::AlbumFallbackTrack)
            }
            None => {
                return LoudnessStatus::unavailable(
                    NormalizationMode::Album,
                    "No trusted album-programme measurement is available",
                    resource_key,
                )
            }
        },
    };
    let Some(record) = record else {
        return LoudnessStatus::unavailable(
            settings.mode,
            "No trusted measurement is available for this audio resource",
            resource_key,
        );
    };
    if record.validate().is_err() {
        return LoudnessStatus::unavailable(
            settings.mode,
            "The loudness measurement is invalid",
            resource_key,
        );
    }
    let Some(integrated_lufs) = record.playback_integrated_lufs_for_route(true) else {
        return LoudnessStatus::unavailable(
            settings.mode,
            "Analyze this mono track again for stereo playback",
            resource_key,
        );
    };
    let decision = LoudnessMeasurement {
        integrated_lufs,
        true_peak_dbtp: None,
    }
    .decide_gain_with_output_peak_bound(
        record.playback_true_peak_bound_dbtp(true),
        policy,
        settings.mode,
    );
    match decision {
        GainDecision::ApplyDb(gain) => {
            let requested = Some(policy.target_lufs - integrated_lufs);
            let channel_safety_limited =
                record
                    .playback_true_peak_bound_dbtp(true)
                    .is_some_and(|peak| {
                        let cap = policy.ceiling_dbtp - peak;
                        record.uses_conservative_channel_sum_bound(true)
                            && requested.is_some_and(|value| value > cap + 0.01)
                            && (gain - cap).abs() <= 0.01
                    });
            LoudnessStatus {
                mode: settings.mode,
                status,
                applied_gain_db: gain,
                measurement_source: Some(record.provenance),
                reason: match (
                    status == LoudnessStatusKind::AlbumFallbackTrack,
                    channel_safety_limited,
                ) {
                    (true, true) => Some("Album measurement unavailable; using track measurement. Gain was limited to prevent overload when mixing channels".to_owned()),
                    (true, false) => Some("Album measurement unavailable; using track measurement".to_owned()),
                    (false, true) => Some("Gain was limited to prevent overload when mixing channels".to_owned()),
                    (false, false) => None,
                },
                source_resource_key: resource_key,
            }
        }
        GainDecision::Disabled => LoudnessStatus::off(),
        GainDecision::Unavailable => LoudnessStatus::unavailable(
            settings.mode,
            "The loudness measurement cannot be used safely",
            resource_key,
        ),
    }
}

const fn default_target_lufs() -> f64 {
    -18.0
}
const fn default_ceiling_dbtp() -> f64 {
    -1.0
}
const fn default_maximum_boost_db() -> f64 {
    6.0
}
const fn default_album_track_fallback() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalAsset, Track};
    use sha2::{Digest, Sha256};
    use std::path::Path;

    #[test]
    fn loudness_job_registry_caps_active_work_but_reclaims_terminal_slots() {
        let jobs = LoudnessAnalysisJobs::default();
        let status = |job_id: &str| LoudnessAnalysisJobStatus {
            job_id: job_id.into(),
            status: "queued".into(),
            total: 1,
            completed: 0,
            error: None,
            resource: None,
        };
        let (cancel_one, _) = watch::channel(false);
        let (cancel_two, _) = watch::channel(false);
        jobs.insert(status("one"), cancel_one).unwrap();
        jobs.insert(status("two"), cancel_two).unwrap();
        let (cancel_three, _) = watch::channel(false);
        assert!(jobs
            .insert(status("three"), cancel_three)
            .unwrap_err()
            .contains("another loudness analysis"));

        jobs.update("one", |job| job.status = "completed".into())
            .unwrap();
        let (cancel_three, _) = watch::channel(false);
        jobs.insert(status("three"), cancel_three).unwrap();
    }

    fn verification_fixture(
        size: usize,
    ) -> (tempfile::TempDir, Arc<Store>, LoudnessResourceIdentity) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("verification-fixture.wav");
        let bytes = vec![0x5a; size];
        std::fs::write(&path, &bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let track = Track {
            id: "local:verification-fixture".into(),
            provider: "local".into(),
            provider_id: "verification-fixture".into(),
            artist: "Fixture".into(),
            title: "Verification fixture".into(),
            ..Track::default()
        };
        let store = Arc::new(Store::open(Path::new(":memory:")).unwrap());
        store
            .save_asset(&LocalAsset {
                id: "verification-fixture".into(),
                recording_id: track.id.clone(),
                path: path.to_string_lossy().into_owned(),
                hash: hash.clone(),
                source_codec: "wav".into(),
                final_codec: "wav".into(),
                source_bitrate: None,
                final_bitrate: None,
                sample_rate: Some(48_000),
                channels: Some(2),
                bit_depth: Some(16),
                duration_seconds: Some(1.0),
                size_bytes: Some(size as u64),
                bitrate_origin: Some("format".into()),
                processing: "ready".into(),
                track,
            })
            .unwrap();
        let resource =
            LoudnessResourceIdentity::track("verification-fixture", &hash, AudioRange::WHOLE_FILE);
        (directory, store, resource)
    }

    fn record(
        provenance: MeasurementProvenance,
        lufs: f64,
        peak: f64,
        asset_id: &str,
    ) -> LoudnessRecord {
        LoudnessRecord {
            schema_version: LOUDNESS_SCHEMA_VERSION,
            resource: LoudnessResourceIdentity::track(
                asset_id,
                "a".repeat(64),
                AudioRange::WHOLE_FILE,
            ),
            analyzer_version: "test-analyzer-v1".into(),
            provenance,
            integrated_lufs: Some(lufs),
            true_peak_dbtp: Some(peak),
            sample_rate: Some(48_000),
            channels: 2,
            measured_at: "2026-10-25T00:00:00Z".into(),
            stereo_mix_integrated_lufs: Some(lufs),
            stereo_mix_true_peak_dbtp: Some(peak),
            stereo_mix_matrix: Some("web-audio-stereo-identity-v1".into()),
            stereo_mix_layout: Some("stereo".into()),
            maximum_member_safe_true_peak_dbtp: None,
            maximum_member_native_safe_true_peak_dbtp: None,
            conservative_channel_sum_bound: false,
            local_file_seals: vec![],
            opus_decoder_proofs: vec![],
        }
    }

    #[test]
    fn settings_default_off_and_json_preferences_round_trip_shape() {
        let settings = LoudnessSettings::default();
        assert_eq!(settings.mode, NormalizationMode::Off);
        assert!(settings.gain_policy().is_ok());
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy.as_object_mut().unwrap().remove("albumTrackFallback");
        assert!(
            serde_json::from_value::<LoudnessSettings>(legacy)
                .unwrap()
                .album_track_fallback
        );
    }

    #[test]
    fn current_loudness_cache_wins_over_legacy_and_legacy_is_safe_fallback() {
        let (_directory, store, resource) = verification_fixture(128 * 1024);
        let member = &resource.members[0];
        let mut legacy = record(
            MeasurementProvenance::FfmpegAnalysis,
            -24.0,
            -9.0,
            &member.asset_id,
        );
        legacy.schema_version = LEGACY_LOUDNESS_SCHEMA_VERSION;
        legacy.resource = resource.clone();
        legacy.channels = 6;
        legacy.stereo_mix_integrated_lufs = None;
        legacy.stereo_mix_true_peak_dbtp = None;
        legacy.stereo_mix_matrix = None;
        legacy.stereo_mix_layout = None;
        let payload = serde_json::to_value(&legacy).unwrap();
        store
            .save_analysis_record(
                &member.asset_id,
                &member.file_hash,
                LEGACY_ANALYSIS_CACHE_VERSION,
                "completed",
                &payload,
            )
            .unwrap();
        let loaded = cached_record(&store, &resource, None).unwrap().unwrap();
        assert_eq!(loaded.schema_version, LEGACY_LOUDNESS_SCHEMA_VERSION);
        assert_eq!(
            loaded.playback_true_peak_bound_dbtp(true),
            Some(-9.0 + 20.0 * 6.0_f64.log10())
        );

        let mut current = record(
            MeasurementProvenance::FfmpegAnalysis,
            -20.0,
            -1.5,
            &member.asset_id,
        );
        current.resource = resource.clone();
        current.stereo_mix_integrated_lufs = Some(-20.0);
        current.stereo_mix_true_peak_dbtp = Some(-1.5);
        store
            .save_analysis_record(
                &member.asset_id,
                &member.file_hash,
                ANALYSIS_CACHE_VERSION,
                "completed",
                &serde_json::to_value(current).unwrap(),
            )
            .unwrap();
        let loaded = cached_record(&store, &resource, None).unwrap().unwrap();
        assert_eq!(loaded.schema_version, LOUDNESS_SCHEMA_VERSION);
        assert_eq!(loaded.stereo_mix_true_peak_dbtp, Some(-1.5));
    }

    #[test]
    fn album_mode_uses_real_album_record_or_explicit_track_fallback() {
        let mut settings = LoudnessSettings::default();
        settings.mode = NormalizationMode::Album;
        let track = record(MeasurementProvenance::FfmpegAnalysis, -24.0, -3.0, "track");
        let album = record(MeasurementProvenance::FfmpegAnalysis, -20.0, -2.0, "album");
        let status =
            decide_for_measurements(&settings, Some(&track), Some(&album), Some("k".into()));
        assert_eq!(status.applied_gain_db, 1.0);
        assert_eq!(status.status, LoudnessStatusKind::Applied);

        let fallback = decide_for_measurements(&settings, Some(&track), None, Some("k".into()));
        assert_eq!(fallback.status, LoudnessStatusKind::AlbumFallbackTrack);
        assert_eq!(fallback.applied_gain_db, 2.0);
    }

    #[test]
    fn unknown_measurement_is_unavailable_and_unity() {
        let mut settings = LoudnessSettings::default();
        settings.mode = NormalizationMode::Track;
        let result = decide_for_measurements(&settings, None, None, Some("remote".into()));
        assert_eq!(result.status, LoudnessStatusKind::Unavailable);
        assert_eq!(result.applied_gain_db, 0.0);
    }

    #[test]
    fn web_audio_mono_requires_a_stereo_route_analysis() {
        let mut settings = LoudnessSettings::default();
        settings.mode = NormalizationMode::Track;
        let mut mono = record(MeasurementProvenance::FfmpegAnalysis, -16.67, -5.90, "mono");
        mono.channels = 1;
        mono.stereo_mix_integrated_lufs = None;
        mono.stereo_mix_true_peak_dbtp = None;
        mono.stereo_mix_matrix = None;
        mono.stereo_mix_layout = None;

        let unavailable =
            decide_for_measurements(&settings, Some(&mono), None, Some("mono".into()));
        assert_eq!(unavailable.status, LoudnessStatusKind::Unavailable);
        assert_eq!(
            unavailable.reason.as_deref(),
            Some("Analyze this mono track again for stereo playback")
        );

        mono.stereo_mix_integrated_lufs = Some(-13.67);
        mono.stereo_mix_true_peak_dbtp = Some(-5.90);
        mono.stereo_mix_matrix = Some("web-audio-mono-to-stereo-v1".into());
        mono.stereo_mix_layout = Some("mono".into());
        let measured = decide_for_measurements(&settings, Some(&mono), None, Some("mono".into()));
        assert_eq!(measured.status, LoudnessStatusKind::Applied);
        assert!((measured.applied_gain_db - (-4.33)).abs() < 0.001);
    }

    #[test]
    fn native_gain_plan_selects_only_verified_mono_output_routes() {
        let source = LoudnessStatus {
            mode: NormalizationMode::Track,
            status: LoudnessStatusKind::Applied,
            applied_gain_db: -1.33,
            measurement_source: Some(MeasurementProvenance::FfmpegAnalysis),
            reason: None,
            source_resource_key: Some("mono-resource".into()),
        };
        let stereo = LoudnessStatus {
            applied_gain_db: -4.33,
            ..source.clone()
        };
        let plan = NativeNormalizationGainPlan::mono(source.clone(), Some(stereo));
        assert_eq!(plan.select(1).applied_gain_db, -1.33);
        assert_eq!(plan.select(2).applied_gain_db, -4.33);
        assert_eq!(plan.select(6).status, LoudnessStatusKind::Unavailable);

        let missing = NativeNormalizationGainPlan::mono(source.clone(), None);
        let unavailable = missing.select(2);
        assert_eq!(unavailable.status, LoudnessStatusKind::Unavailable);
        assert_eq!(
            unavailable.reason.as_deref(),
            Some("Analyze this mono track again for stereo playback")
        );

        let multichannel = NativeNormalizationGainPlan::fixed(source.clone());
        assert_eq!(multichannel.select(2).applied_gain_db, -1.33);
        let off = NativeNormalizationGainPlan::mono(LoudnessStatus::off(), None);
        assert_eq!(off.select(2).status, LoudnessStatusKind::Off);
    }

    #[tokio::test]
    async fn verification_without_a_cancellation_owner_finishes_and_memoizes_the_seal() {
        let (_directory, store, resource) = verification_fixture(128 * 1024);
        let memo = LocalVerificationMemo::default();
        memo.verify_and_remember(store.clone(), resource.clone(), None)
            .await
            .unwrap();
        assert!(memo.is_currently_verified(&store, &resource, None).unwrap());
    }

    #[test]
    fn tag_verification_reservation_coalesces_and_is_bounded_to_whole_track() {
        let (_directory, store, resource) = verification_fixture(128 * 1024);
        let memo = LocalVerificationMemo::default();
        assert!(memo.begin_tag_verification(&store, &resource).unwrap());
        assert!(!memo.begin_tag_verification(&store, &resource).unwrap());
        memo.finish_tag_verification(&resource);
        assert!(memo.begin_tag_verification(&store, &resource).unwrap());

        let excerpt = LoudnessResourceIdentity::track(
            "verification-fixture",
            &resource.members[0].file_hash,
            AudioRange {
                start_seconds: 1.0,
                end_seconds: Some(2.0),
            },
        );
        assert!(!memo.begin_tag_verification(&store, &excerpt).unwrap());
        memo.finish_tag_verification(&resource);

        {
            let mut in_flight = memo.tag_verification_in_flight.lock().unwrap();
            for index in 0..LocalVerificationMemo::MAX_TAG_VERIFICATIONS_IN_FLIGHT {
                in_flight.insert(format!("reserved-{index}"));
            }
        }
        assert!(!memo.begin_tag_verification(&store, &resource).unwrap());
    }

    #[tokio::test]
    async fn closed_false_owned_verification_receiver_remains_a_cancellation() {
        let (_directory, store, resource) = verification_fixture(128 * 1024);
        let memo = LocalVerificationMemo::default();
        let (sender, receiver) = watch::channel(false);
        drop(sender);
        let error = memo
            .verify_and_remember(store, resource, Some(receiver))
            .await
            .unwrap_err();
        assert!(error.contains("cancelled"));
    }

    #[tokio::test]
    async fn live_true_owned_verification_cancellation_stops_the_hash_worker() {
        let (_directory, store, resource) = verification_fixture(64 * 1024 * 1024);
        let memo = Arc::new(LocalVerificationMemo::default());
        let (sender, receiver) = watch::channel(false);
        let work = tokio::spawn({
            let memo = memo.clone();
            async move {
                memo.verify_and_remember(store, resource, Some(receiver))
                    .await
            }
        });
        tokio::task::yield_now().await;
        sender.send(true).unwrap();
        drop(sender); // A true value remains a cancellation after sender closure.
        let error = work.await.unwrap().unwrap_err();
        assert!(error.contains("cancelled"));
    }
}
