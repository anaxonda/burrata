//! Explicit selected-track analysis from resolver-authenticated remote streams.
//!
//! The UI submits canonical recording IDs only. Resolution, transport URLs,
//! request headers, decoding, and cache identity remain backend-owned.

use crate::{
    acquisition::AcquisitionManager,
    models::{Candidate, Track},
    observed_analysis::ObservedAnalysis,
    playback_sources::{self, ResolvedSource},
    stream_analysis::{
        AnalysisMetric, AnalysisResource, ApplyAnalysisSelection, PcmBatch,
        StreamDurationPrecision, WholeTrackAnalysis, WholeTrackCapture, KEY_PCM_RATE,
        MAX_TRACK_SECONDS,
    },
    Store,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::watch};

pub const MAX_SELECTED_STREAM_TRACKS: usize = 25;
const MAX_RETAINED_JOBS: usize = 12;
const MAX_ACTIVE_JOBS: usize = 1;
const MAX_REMOTE_DECODE_SECONDS: u64 = 20 * 60;
const MAX_PCM_BYTES: usize = KEY_PCM_RATE as usize * MAX_TRACK_SECONDS as usize * 4;
const PCM_READ_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedStreamAnalysisResult {
    pub track_id: String,
    pub recording_id: String,
    pub status: String,
    pub provider: Option<String>,
    pub source_id: Option<String>,
    pub analysis: Option<WholeTrackAnalysis>,
    pub error: Option<String>,
    #[serde(default)]
    pub cache_hit: bool,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(skip)]
    canonical_identity: Option<String>,
    #[serde(skip)]
    verified_mapping_fingerprint: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedStreamAnalysisJob {
    pub job_id: String,
    pub state: String,
    pub total: usize,
    pub completed: usize,
    pub results: Vec<SelectedStreamAnalysisResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedStreamMediaAnalysisRequest {
    pub track: StreamMediaTrackContext,
    pub provider: String,
    pub source_id: String,
    pub duration_seconds: f64,
    #[serde(default)]
    pub audio_variant: Option<crate::stream_analysis::StreamAudioVariant>,
    /// Identifies the active WebView stream generation for resolver routing.
    /// It is not proof of source identity; the resolved provider/id is checked below.
    pub webview_generation: Option<u64>,
}

const MAX_SAFE_WEBVIEW_GENERATION: u64 = 9_007_199_254_740_991;

fn valid_webview_generation(generation: Option<u64>) -> bool {
    generation.map_or(true, |value| {
        value > 0 && value <= MAX_SAFE_WEBVIEW_GENERATION
    })
}

/// URL-free playback metadata needed to resolve an unsaved current track.
/// The server reconstructs a fresh Track with every local path, URL, tag,
/// provenance, and manual field omitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamMediaTrackContext {
    pub id: String,
    pub provider: String,
    pub provider_id: String,
    #[serde(default)]
    pub has_playback_range: bool,
    pub artist: String,
    pub title: String,
    pub source_page_url: Option<String>,
    pub version: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration_seconds: Option<f64>,
    pub isrc: Option<String>,
    pub catalog_number: Option<String>,
    pub release_date: Option<String>,
    pub recording_date: Option<String>,
}

impl StreamMediaTrackContext {
    pub fn into_sanitized_track(
        self,
        source_duration: f64,
        playback_provider: &str,
        source_id: &str,
    ) -> Result<Track, String> {
        self.into_sanitized_track_with_artist_policy(
            source_duration,
            playback_provider,
            source_id,
            false,
        )
    }

    /// Build the same URL-free recording projection when an exact, server-bound
    /// YouTube playback handoff has no catalog artist field. Callers must still
    /// require the current generation/source lease before accepting this track.
    pub(crate) fn into_active_youtube_handoff_track(
        self,
        source_duration: f64,
        source_id: &str,
    ) -> Result<Track, String> {
        let video_id = source_id
            .strip_prefix("youtube:")
            .ok_or("Active YouTube source identity is invalid")?;
        if video_id.len() != 11
            || !video_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || !source_duration.is_finite()
            || !(8.0..=MAX_TRACK_SECONDS).contains(&source_duration)
        {
            return Err("Active YouTube source identity is invalid".into());
        }
        self.into_sanitized_track_with_artist_policy(source_duration, "youtube", source_id, true)
    }

    fn into_sanitized_track_with_artist_policy(
        self,
        source_duration: f64,
        playback_provider: &str,
        source_id: &str,
        allow_empty_artist: bool,
    ) -> Result<Track, String> {
        let youtube_source_id = source_id.strip_prefix("youtube:").unwrap_or(source_id);
        let bounded = |value: &str, max: usize| {
            !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
        };
        if !bounded(&self.id, 512)
            || !bounded(&self.provider, 64)
            || !bounded(&self.provider_id, 512)
            || self.provider_id.contains("://")
            || self.provider_id.contains('?')
            || self.provider_id.contains('#')
            || (!allow_empty_artist && !bounded(&self.artist, 512))
            || (allow_empty_artist && !self.artist.is_empty() && !bounded(&self.artist, 512))
            || !bounded(&self.title, 512)
            || self.provider == "radio"
            || self.has_playback_range
            || self.duration_seconds.is_some_and(|duration| {
                !duration.is_finite() || !(8.0..=MAX_TRACK_SECONDS).contains(&duration)
            })
            || (self.provider == "youtube"
                && playback_provider == "youtube"
                && (youtube_source_id.len() != 11 || self.provider_id != youtube_source_id))
        {
            return Err("Current streamed-track metadata is invalid".into());
        }
        let optional_safe = |value: Option<String>, max: usize| -> Result<Option<String>, String> {
            match value {
                Some(value) if value.len() <= max && !value.chars().any(char::is_control) => {
                    Ok(Some(value))
                }
                Some(_) => Err("Current streamed-track metadata exceeds its bounds".into()),
                None => Ok(None),
            }
        };
        let source_url = match self.source_page_url {
            None => String::new(),
            Some(raw) if raw.len() <= 2048 && !raw.chars().any(char::is_control) => {
                match (self.provider.as_str(), playback_provider) {
                    ("bandcamp", "bandcamp") => crate::bandcamp::validate_track_page_url(&raw)
                        .map_err(|_| "Bandcamp source page is not canonical")?
                        .to_string(),
                    ("youtube", "youtube") => {
                        let parsed = url::Url::parse(&raw)
                            .map_err(|_| "YouTube source page is not canonical")?;
                        let host = parsed.host_str().unwrap_or_default();
                        let mut query = parsed.query_pairs();
                        let (key, value) =
                            query.next().ok_or("YouTube source page is not canonical")?;
                        if parsed.scheme() != "https"
                            || !matches!(host, "youtube.com" | "www.youtube.com")
                            || parsed.port().is_some()
                            || !parsed.username().is_empty()
                            || parsed.password().is_some()
                            || parsed.path() != "/watch"
                            || parsed.fragment().is_some()
                            || key != "v"
                            || value != youtube_source_id
                            || query.next().is_some()
                            || youtube_source_id.len() != 11
                            || !youtube_source_id.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                            })
                        {
                            return Err(
                                "YouTube source page does not match the active source".into()
                            );
                        }
                        format!("https://www.youtube.com/watch?v={youtube_source_id}")
                    }
                    _ => return Err(
                        "Source pages are only accepted for matching Bandcamp or YouTube tracks"
                            .into(),
                    ),
                }
            }
            Some(_) => return Err("Current source page exceeds its safety bounds".into()),
        };
        Ok(Track {
            id: self.id,
            provider: self.provider,
            provider_id: self.provider_id,
            source_url,
            artist: self.artist,
            title: self.title,
            version: optional_safe(self.version, 256)?,
            album: optional_safe(self.album, 512)?,
            album_artist: optional_safe(self.album_artist, 512)?,
            duration_seconds: Some(source_duration),
            isrc: optional_safe(self.isrc, 32)?,
            catalog_number: optional_safe(self.catalog_number, 128)?,
            release_date: optional_safe(self.release_date, 64)?,
            recording_date: optional_safe(self.recording_date, 64)?,
            ..Track::default()
        })
    }
}

const FULL_MEDIA_ANALYSIS_FIELDS: [AnalysisMetric; 2] = [AnalysisMetric::Bpm, AnalysisMetric::Key];

struct JobEntry {
    snapshot: SelectedStreamAnalysisJob,
    cancel: watch::Sender<bool>,
    fields: BTreeSet<AnalysisMetric>,
    allow_metadata_apply: bool,
}

#[derive(Default)]
pub struct SelectedStreamAnalysisJobs {
    jobs: Mutex<HashMap<String, JobEntry>>,
}

impl SelectedStreamAnalysisJobs {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn start(
        self: &Arc<Self>,
        store: Arc<Store>,
        acquisition: Arc<AcquisitionManager>,
        observed: Arc<ObservedAnalysis>,
        track_ids: Vec<String>,
        fields: BTreeSet<AnalysisMetric>,
    ) -> Result<SelectedStreamAnalysisJob, String> {
        self.start_internal(
            store,
            acquisition,
            observed,
            track_ids,
            fields,
            None,
            None,
            None,
            false,
            None,
            None,
        )
    }

    pub fn start_media_analysis(
        self: &Arc<Self>,
        store: Arc<Store>,
        acquisition: Arc<AcquisitionManager>,
        observed: Arc<ObservedAnalysis>,
        request: SelectedStreamMediaAnalysisRequest,
        selected_candidate: Option<Candidate>,
    ) -> Result<SelectedStreamAnalysisJob, String> {
        if !matches!(
            request.provider.as_str(),
            "youtube" | "soundcloud" | "bandcamp" | "monochrome"
        ) || request.source_id.trim().is_empty()
            || request.source_id.len() > 512
            || request.source_id.chars().any(char::is_control)
            || request.source_id.contains("://")
            || request.source_id.contains('?')
            || request.source_id.contains('#')
            || !request.duration_seconds.is_finite()
            || !(8.0..=MAX_TRACK_SECONDS).contains(&request.duration_seconds)
            || !valid_webview_generation(request.webview_generation)
            || request
                .audio_variant
                .as_ref()
                .is_some_and(|variant| variant.identity_sha256().is_none())
        {
            return Err("Full stream analysis source identity is invalid".into());
        }
        let source_duration = request.duration_seconds;
        let playback_provider = request.provider.clone();
        let source_id = request.source_id.clone();
        let track = match request.track.clone().into_sanitized_track(
            source_duration,
            &playback_provider,
            &source_id,
        ) {
            Ok(track) => track,
            Err(strict_error) => {
                let trusted_youtube_handoff = request.webview_generation.is_some()
                    && selected_candidate.as_ref().is_some_and(|candidate| {
                        exact_youtube_handoff_candidate(candidate, &playback_provider, &source_id)
                    });
                if !trusted_youtube_handoff {
                    return Err(strict_error);
                }
                request
                    .track
                    .clone()
                    .into_active_youtube_handoff_track(source_duration, &source_id)?
            }
        };
        if selected_candidate.as_ref().is_some_and(|candidate| {
            !matches_expected_source(
                candidate,
                Some((&playback_provider, &source_id, source_duration)),
            )
        }) {
            return Err("Active playback source identity changed before analysis".into());
        }
        let track_id = track.id.clone();
        self.start_internal(
            store,
            acquisition,
            observed,
            vec![track_id],
            FULL_MEDIA_ANALYSIS_FIELDS.into_iter().collect(),
            Some((request.provider, request.source_id, source_duration)),
            Some(track),
            request.webview_generation,
            true,
            selected_candidate,
            request.audio_variant,
        )
    }

    fn start_internal(
        self: &Arc<Self>,
        store: Arc<Store>,
        acquisition: Arc<AcquisitionManager>,
        observed: Arc<ObservedAnalysis>,
        track_ids: Vec<String>,
        fields: BTreeSet<AnalysisMetric>,
        exact_source: Option<(String, String, f64)>,
        media_track: Option<Track>,
        webview_generation: Option<u64>,
        full_media: bool,
        selected_candidate: Option<Candidate>,
        expected_audio_variant: Option<crate::stream_analysis::StreamAudioVariant>,
    ) -> Result<SelectedStreamAnalysisJob, String> {
        if track_ids.is_empty() || track_ids.len() > MAX_SELECTED_STREAM_TRACKS {
            return Err(format!(
                "Select 1–{MAX_SELECTED_STREAM_TRACKS} streamed recordings"
            ));
        }
        if fields.is_empty() {
            return Err("Choose BPM, key, or both for analysis".into());
        }
        if track_ids
            .iter()
            .any(|id| id.trim().is_empty() || id.len() > 512)
        {
            return Err("Selected recording IDs are invalid".into());
        }
        let unique = track_ids.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != track_ids.len() {
            return Err("Selected recordings must be unique".into());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let (cancel, cancel_rx) = watch::channel(false);
        let snapshot = SelectedStreamAnalysisJob {
            job_id: job_id.clone(),
            state: "queued".into(),
            total: track_ids.len(),
            completed: 0,
            results: track_ids
                .iter()
                .map(|track_id| SelectedStreamAnalysisResult {
                    track_id: track_id.clone(),
                    recording_id: track_id.clone(),
                    status: "queued".into(),
                    provider: None,
                    source_id: None,
                    analysis: None,
                    error: None,
                    cache_hit: false,
                    elapsed_ms: None,
                    canonical_identity: None,
                    verified_mapping_fingerprint: None,
                })
                .collect(),
        };
        let writer_lease = store.try_library_write()?;
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "Stream analysis job list is unavailable")?;
            let active = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.snapshot.state.as_str(),
                        "queued" | "running" | "cancelling"
                    )
                })
                .count();
            if active >= MAX_ACTIVE_JOBS {
                return Err("Finish or cancel the active stream analysis first".into());
            }
            if jobs.len() >= MAX_RETAINED_JOBS {
                let oldest = jobs
                    .iter()
                    .find(|(_, entry)| {
                        matches!(
                            entry.snapshot.state.as_str(),
                            "completed" | "cancelled" | "failed"
                        )
                    })
                    .map(|(id, _)| id.clone())
                    .ok_or("Stream analysis job history is full")?;
                jobs.remove(&oldest);
            }
            jobs.insert(
                job_id.clone(),
                JobEntry {
                    snapshot: snapshot.clone(),
                    cancel,
                    fields: fields.clone(),
                    allow_metadata_apply: !full_media,
                },
            );
        }

        let manager = self.clone();
        let worker_job_id = job_id.clone();
        tauri::async_runtime::spawn(async move {
            let _writer_lease = writer_lease;
            manager.set_state(&worker_job_id, "running");
            for (index, track_id) in track_ids.iter().enumerate() {
                if *cancel_rx.borrow() {
                    break;
                }
                manager.set_item_state(&worker_job_id, index, "running");
                let item_started = std::time::Instant::now();
                let mut result = analyze_one(
                    &store,
                    &acquisition,
                    &observed,
                    track_id,
                    &fields,
                    index as u64 + 1,
                    cancel_rx.clone(),
                    full_media,
                    media_track.as_ref(),
                    exact_source
                        .as_ref()
                        .map(|(provider, source_id, duration)| {
                            (provider.as_str(), source_id.as_str(), *duration)
                        }),
                    webview_generation,
                    selected_candidate.as_ref(),
                    expected_audio_variant.as_ref(),
                )
                .await;
                result.elapsed_ms =
                    Some(item_started.elapsed().as_millis().min(u64::MAX as u128) as u64);
                manager.set_result(&worker_job_id, index, result);
            }
            let state = if *cancel_rx.borrow() {
                "cancelled"
            } else {
                "completed"
            };
            manager.set_state(&worker_job_id, state);
        });
        Ok(snapshot)
    }

    pub fn get(&self, job_id: &str) -> Result<SelectedStreamAnalysisJob, String> {
        self.jobs
            .lock()
            .map_err(|_| "Stream analysis job list is unavailable")?
            .get(job_id)
            .map(|entry| entry.snapshot.clone())
            .ok_or_else(|| "Stream analysis job not found".into())
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| "Stream analysis job list is unavailable")?;
        let entry = jobs
            .get_mut(job_id)
            .ok_or_else(|| "Stream analysis job not found".to_owned())?;
        if matches!(entry.snapshot.state.as_str(), "queued" | "running") {
            entry.snapshot.state = "cancelling".into();
            entry.cancel.send_replace(true);
        }
        Ok(())
    }

    pub async fn apply(
        &self,
        store: &Store,
        job_id: &str,
        track_id: &str,
        selection: &ApplyAnalysisSelection,
    ) -> Result<Track, String> {
        let job = self.get(job_id)?;
        if job.state != "completed" {
            return Err("Stream analysis is not complete".into());
        }
        let job_policy = self
            .jobs
            .lock()
            .map_err(|_| "Stream analysis job list is unavailable")?
            .get(job_id)
            .map(|entry| (entry.allow_metadata_apply, entry.fields.clone()))
            .ok_or_else(|| "Stream analysis job not found".to_owned())?;
        if !job_policy.0 {
            return Err(
                "Full stream media analysis never applies BPM or key to saved metadata".into(),
            );
        }
        let requested_fields = job_policy.1;
        if selection.fields.is_empty() || !selection.fields.is_subset(&requested_fields) {
            return Err("Apply only fields selected for this analysis".into());
        }
        let row = job
            .results
            .iter()
            .find(|row| row.track_id == track_id)
            .ok_or_else(|| "Recording is not part of this reviewed analysis".to_owned())?;
        if row.status != "completed" {
            return Err("This recording has no completed analysis to apply".into());
        }
        let analysis = row
            .analysis
            .as_ref()
            .ok_or_else(|| "Reviewed stream analysis is unavailable".to_owned())?;
        if analysis.validate().is_err() || analysis.resource.recording_id() != track_id {
            return Err("Analysis recording identity changed".into());
        }
        if !matches!(
            &analysis.resource,
            AnalysisResource::Stream { recording_id, .. } if recording_id == track_id
        ) {
            return Err("Analysis no longer matches a verified stream recording".into());
        }
        let current = store
            .get_library_track(track_id)
            .map_err(|_| "Could not verify the saved recording")?
            .ok_or_else(|| "Saved recording not found".to_owned())?;
        if current.id != track_id || current.playback_range.is_some() {
            return Err("Saved recording identity changed".into());
        }
        let expected_identity = row
            .canonical_identity
            .as_deref()
            .ok_or_else(|| "Reviewed source identity is unavailable".to_owned())?;
        if canonical_analysis_identity(&current, &analysis.resource)? != expected_identity {
            return Err("Saved recording or resolved source changed after analysis".into());
        }
        let expected_mapping_fingerprint = row
            .verified_mapping_fingerprint
            .as_ref()
            .ok_or_else(|| "Reviewed source mapping identity is unavailable".to_owned())?;
        let provider = row
            .provider
            .as_deref()
            .ok_or_else(|| "Reviewed source provider is unavailable".to_owned())?;
        let recording_fingerprint = playback_sources::recording_identity_fingerprint(&current)
            .ok_or_else(|| "Could not verify the resolved source mapping".to_owned())?;
        let current_mapping_fingerprint = store
            .verified_playback_source_mapping_fingerprint(
                track_id,
                &recording_fingerprint,
                provider,
            )
            .map_err(|_| "Could not verify the resolved source mapping")?;
        if &current_mapping_fingerprint != expected_mapping_fingerprint {
            return Err("Verified playback source mapping changed after analysis".into());
        }
        store
            .apply_stream_analyzed_track(
                track_id,
                expected_identity,
                provider,
                expected_mapping_fingerprint.as_deref(),
                analysis,
                selection,
            )
            .map_err(|error| error.to_string())
    }

    fn set_state(&self, job_id: &str, state: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(entry) = jobs.get_mut(job_id) {
                entry.snapshot.state = state.into();
            }
        }
    }

    fn set_result(&self, job_id: &str, index: usize, result: SelectedStreamAnalysisResult) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(entry) = jobs.get_mut(job_id) {
                if let Some(slot) = entry.snapshot.results.get_mut(index) {
                    *slot = result;
                    entry.snapshot.completed += 1;
                }
            }
        }
    }

    fn set_item_state(&self, job_id: &str, index: usize, state: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(row) = jobs
                .get_mut(job_id)
                .and_then(|entry| entry.snapshot.results.get_mut(index))
            {
                row.status = state.into();
            }
        }
    }
}

async fn analyze_one(
    store: &Store,
    acquisition: &AcquisitionManager,
    observed: &ObservedAnalysis,
    track_id: &str,
    fields: &BTreeSet<AnalysisMetric>,
    generation: u64,
    mut cancel: watch::Receiver<bool>,
    include_waveform: bool,
    media_track: Option<&Track>,
    exact_source: Option<(&str, &str, f64)>,
    webview_generation: Option<u64>,
    selected_candidate: Option<&Candidate>,
    expected_audio_variant: Option<&crate::stream_analysis::StreamAudioVariant>,
) -> SelectedStreamAnalysisResult {
    let mut result = SelectedStreamAnalysisResult {
        track_id: track_id.into(),
        recording_id: track_id.into(),
        status: "failed".into(),
        provider: None,
        source_id: None,
        analysis: None,
        error: None,
        cache_hit: false,
        elapsed_ms: None,
        canonical_identity: None,
        verified_mapping_fingerprint: None,
    };
    let track = match media_track {
        Some(track) if include_waveform && track.id == track_id => track.clone(),
        Some(_) => {
            result.error = Some("Current streamed-track identity changed".into());
            return result;
        }
        None => match store.get_library_track(track_id) {
            Ok(Some(track)) if track.id == track_id => track,
            Ok(_) => {
                result.error = Some("Saved recording not found".into());
                return result;
            }
            Err(_) => {
                result.error = Some("Could not load the saved recording".into());
                return result;
            }
        },
    };
    result.recording_id = track.id.clone();
    if track.playback_range.is_some() {
        result.error = Some("Analyze the full recording, not a selected excerpt".into());
        return result;
    }
    if *cancel.borrow() {
        result.status = "cancelled".into();
        return result;
    }
    let trusted_active_handoff = selected_candidate.is_some();
    let selected_source = selected_candidate.cloned().or_else(|| {
        exact_source.and_then(|(provider, source_id, _)| {
            if provider != "youtube" {
                return None;
            }
            let fingerprint = playback_sources::recording_identity_fingerprint(&track)?;
            let mapping = store
                .verified_playback_source(&track.id, &fingerprint, "youtube")
                .ok()??;
            verified_youtube_candidate(&track, source_id, &mapping)
        })
    });
    let resolver_track = selected_candidate
        .and_then(|candidate| active_youtube_resolver_track(&track, candidate))
        .unwrap_or_else(|| track.clone());
    let resolved = match playback_sources::resolve_with_trace_context(
        store,
        acquisition,
        &resolver_track,
        selected_source,
        cancel.clone(),
        None,
        None,
        webview_generation,
    )
    .await
    {
        Ok(resolved) => resolved,
        Err(_) => {
            if *cancel.borrow() {
                result.status = "cancelled".into();
                return result;
            }
            result.error = Some("No verified full-recording stream could be resolved".into());
            return result;
        }
    };
    let Some(candidate) = resolved.candidate.as_ref() else {
        result.error = Some("Resolved source has no stable provider recording identity".into());
        return result;
    };
    if !matches_expected_source(candidate, exact_source) {
        result.error = Some("Resolved playback source changed; no analysis was cached".into());
        return result;
    }
    let resolved_variant =
        crate::stream_analysis::StreamAudioVariant::from_resolver_quality(&resolved.quality);
    let resolved_variant_digest = resolved_variant
        .as_ref()
        .and_then(crate::stream_analysis::StreamAudioVariant::identity_sha256);
    if expected_audio_variant
        .and_then(crate::stream_analysis::StreamAudioVariant::identity_sha256)
        .is_some_and(|expected| Some(expected) != resolved_variant_digest)
    {
        result.error = Some("Resolved audio format changed; no analysis was cached".into());
        return result;
    }
    let Some(resource) = crate::observed_analysis::resource_for_variant(
        &track,
        Some(candidate),
        resolved_variant.as_ref(),
    ) else {
        result.error = Some("Resolved provider does not support stable stream analysis".into());
        return result;
    };
    result.provider = Some(candidate.provider.clone());
    result.source_id = Some(candidate.id.clone());
    result.canonical_identity = match canonical_analysis_identity(&track, &resource) {
        Ok(identity) => Some(identity),
        Err(_) => {
            result.error = Some("Resolved source identity is invalid".into());
            return result;
        }
    };
    if !include_waveform {
        let mapping_fingerprint = match playback_sources::recording_identity_fingerprint(&track) {
            Some(fingerprint) => fingerprint,
            None => {
                result.error = Some("Could not verify the resolved source mapping".into());
                return result;
            }
        };
        result.verified_mapping_fingerprint = match store
            .verified_playback_source_mapping_fingerprint(
                &track.id,
                &mapping_fingerprint,
                &candidate.provider,
            ) {
            Ok(fingerprint) => Some(fingerprint),
            Err(_) => {
                result.error = Some("Could not verify the resolved source mapping".into());
                return result;
            }
        };
    }
    if resolved.station.is_some() || resolved.source.cenc.is_some() {
        result.error =
            Some("Radio and encrypted sources are not supported for selected analysis".into());
        return result;
    }
    let resolved_duration = resolved.duration.filter(|value| value.is_finite());
    if let Some((_, _, expected_source_duration)) = exact_source {
        let Some(resolved_duration) = resolved_duration else {
            result.error = Some("Resolved playback source has no verified duration".into());
            return result;
        };
        if !resolved_metadata_matches_active_duration(
            resolved_duration,
            expected_source_duration,
            trusted_active_handoff,
        ) {
            result.error =
                Some("Resolved playback source duration changed; no analysis was cached".into());
            return result;
        }
    }
    let duration = exact_source
        .filter(|_| trusted_active_handoff)
        .map(|(_, _, expected)| expected)
        .or(resolved_duration)
        .or_else(|| track.duration_seconds.filter(|value| value.is_finite()));
    let Some(duration) = duration.filter(|value| (8.0..=MAX_TRACK_SECONDS).contains(value)) else {
        result.error =
            Some("Resolved recording duration is outside the 8 second to 15 minute limit".into());
        return result;
    };
    let duration_precision = analysis_duration_precision(duration, trusted_active_handoff);
    if let Some(cached) = observed.cached_resource(&resource).filter(|cached| {
        !include_waveform
            || (duration_precision.accepts(duration, cached.duration_seconds)
                && cached
                    .waveform
                    .as_ref()
                    .is_some_and(|waveform| waveform.validate(cached.duration_seconds).is_ok()))
    }) {
        result.status = "completed".into();
        result.cache_hit = true;
        result.analysis = Some(cached);
        return result;
    }
    let capture_result = if duration_precision == StreamDurationPrecision::RoundedToSecond {
        WholeTrackCapture::new_with_rounded_duration(
            generation,
            resource,
            duration,
            0.0,
            include_waveform,
        )
    } else if include_waveform {
        WholeTrackCapture::new_with_waveform(generation, resource, duration, 0.0)
    } else {
        WholeTrackCapture::new(generation, resource, duration, 0.0)
    };
    let capture = match capture_result {
        Ok(capture) => capture,
        Err(_) => {
            result.error = Some("Could not initialize full-recording analysis".into());
            return result;
        }
    };
    match decode_resolved_stream(
        &resolved,
        capture,
        generation,
        duration,
        duration_precision,
        &mut cancel,
    )
    .await
    {
        Ok(analysis) => {
            if *cancel.borrow() {
                result.status = "cancelled".into();
                return result;
            }
            if analysis.resource.has_stable_audio_variant()
                && observed.cache_record(analysis.clone()).is_err()
            {
                result.error =
                    Some("Analysis completed but could not be saved to the local cache".into());
                return result;
            }
            result.status = "completed".into();
            result.analysis = Some(analysis);
        }
        Err(StreamDecodeError::Cancelled) => result.status = "cancelled".into(),
        Err(StreamDecodeError::Failed(message)) => result.error = Some(message),
    }
    result
}

fn active_youtube_resolver_track(track: &Track, candidate: &Candidate) -> Option<Track> {
    if track.local_path.is_some()
        || track.asset_id.is_some()
        || track.playback_range.is_some()
        || candidate.provider != "youtube"
    {
        return None;
    }
    let video_id = candidate.id.strip_prefix("youtube:")?;
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || crate::acquisition::validate_source_url(&candidate.url, "youtube").is_err()
        || crate::acquisition::youtube_video_id(&candidate.url).as_deref() != Some(video_id)
    {
        return None;
    }
    let mut resolver_track = track.clone();
    resolver_track.provider = "youtube".into();
    resolver_track.provider_id = video_id.to_owned();
    resolver_track.source_url = format!("https://www.youtube.com/watch?v={video_id}");
    Some(resolver_track)
}

/// Turn an exact active YouTube source into a resolver selection only when the
/// source is already bound to this recording by the backend's verified map.
/// The request's source ID never supplies a URL or recording attestation.
fn verified_youtube_candidate(
    track: &Track,
    requested_source_id: &str,
    mapping: &crate::storage::VerifiedPlaybackSource,
) -> Option<Candidate> {
    if track.playback_range.is_some()
        || track.local_path.is_some()
        || mapping.provider != "youtube"
        || !requested_source_id.starts_with("youtube:")
    {
        return None;
    }
    let video_id = requested_source_id.strip_prefix("youtube:")?;
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || mapping.provider_id != video_id
        || mapping.permanent_url != format!("https://www.youtube.com/watch?v={video_id}")
        || crate::acquisition::youtube_video_id(&mapping.permanent_url).as_deref() != Some(video_id)
    {
        return None;
    }
    Some(Candidate {
        id: requested_source_id.to_owned(),
        provider: "youtube".into(),
        url: mapping.permanent_url.clone(),
        title: track.title.clone(),
        duration_seconds: track.duration_seconds,
        permission: "unknown".into(),
        confidence: "review".into(),
        ..Candidate::default()
    })
}

pub(crate) fn stream_media_identity_fingerprint(track: &Track) -> Option<String> {
    let mut identity_track = track.clone();
    // Playback and the analysis request may report duration from different
    // metadata layers; the active source duration is independently checked.
    identity_track.duration_seconds = None;
    playback_sources::recording_identity_fingerprint(&identity_track)
}

fn matches_expected_source(
    candidate: &crate::models::Candidate,
    expected: Option<(&str, &str, f64)>,
) -> bool {
    expected.map_or(true, |(provider, source_id, _)| {
        candidate.provider == provider && candidate.id == source_id
    })
}

fn exact_youtube_handoff_candidate(
    candidate: &crate::models::Candidate,
    provider: &str,
    source_id: &str,
) -> bool {
    if provider != "youtube" || candidate.provider != "youtube" || candidate.id != source_id {
        return false;
    }
    let Some(video_id) = source_id.strip_prefix("youtube:") else {
        return false;
    };
    video_id.len() == 11
        && video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && candidate.url == format!("https://www.youtube.com/watch?v={video_id}")
        && crate::acquisition::youtube_video_id(&candidate.url).as_deref() == Some(video_id)
}

fn resolved_metadata_matches_active_duration(
    resolved_duration: f64,
    active_duration: f64,
    trusted_active_handoff: bool,
) -> bool {
    if !resolved_duration.is_finite()
        || !active_duration.is_finite()
        || !(8.0..=MAX_TRACK_SECONDS).contains(&resolved_duration)
        || !(8.0..=MAX_TRACK_SECONDS).contains(&active_duration)
    {
        return false;
    }
    let delta = (resolved_duration - active_duration).abs();
    if trusted_active_handoff && (resolved_duration - resolved_duration.round()).abs() < 1e-6 {
        delta < 1.0
    } else {
        delta <= 0.05_f64.max(active_duration * 0.001)
    }
}

pub(crate) fn canonical_analysis_identity(
    track: &Track,
    resource: &AnalysisResource,
) -> Result<String, String> {
    let payload = serde_json::json!({
        "recordingId": track.id,
        "provider": track.provider,
        "providerId": track.provider_id,
        "title": track.title,
        "artist": track.artist,
        "version": track.version,
        "sourceUrl": track.source_url,
        "playbackRange": track.playback_range,
        "resourceIdentity": resource.cache_identity().map_err(|_| "Invalid source identity")?,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|_| "Could not hash the selected source")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[derive(Debug)]
enum StreamDecodeError {
    Cancelled,
    Failed(String),
}

async fn decode_resolved_stream(
    resolved: &ResolvedSource,
    mut capture: WholeTrackCapture,
    generation: u64,
    expected_duration: f64,
    duration_precision: StreamDurationPrecision,
    cancel: &mut watch::Receiver<bool>,
) -> Result<WholeTrackAnalysis, StreamDecodeError> {
    let input = url::Url::parse(&resolved.source.input)
        .map_err(|_| StreamDecodeError::Failed("Resolved stream URL is invalid".into()))?;
    if input.scheme() != "https"
        || input.host_str().is_none()
        || !input.username().is_empty()
        || input.password().is_some()
    {
        return Err(StreamDecodeError::Failed(
            "Only resolver-authenticated HTTPS streams are supported".into(),
        ));
    }
    let headers = format_headers(&resolved.source.headers)?;
    let mut command = Command::new(&resolved.source.ffmpeg);
    crate::tools::configure_no_console_tokio(&mut command);
    let max_duration = duration_precision.maximum_duration(expected_duration);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .args([
            "-nostdin",
            "-v",
            "error",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-max_alloc",
            "134217728",
            "-probesize",
            "2000000",
            "-analyzeduration",
            "3000000",
            "-protocol_whitelist",
            "http,https,tls,tcp,crypto,data",
            "-rw_timeout",
            "10000000",
        ]);
    if let Some(headers) = headers.as_deref() {
        command.args(["-headers", headers]);
    }
    command.arg("-i").arg(&resolved.source.input).args([
        "-map",
        "0:a:0",
        "-vn",
        "-ac",
        "1",
        "-ar",
        &KEY_PCM_RATE.to_string(),
        "-threads",
        "1",
        "-f",
        "f32le",
        "pipe:1",
    ]);
    let mut natural_end_position = None;
    let decode = async {
        let mut child = command
            .spawn()
            .map_err(|_| StreamDecodeError::Failed("Could not start local audio decoder".into()))?;
        let Some(mut stdout) = child.stdout.take() else {
            return Err(StreamDecodeError::Failed(
                "Local audio decoder has no PCM output".into(),
            ));
        };
        let mut bytes = Vec::with_capacity(PCM_READ_BYTES);
        let mut buffer = vec![0u8; PCM_READ_BYTES];
        let mut sample_count = 0usize;
        loop {
            let count = stdout.read(&mut buffer).await.map_err(|_| {
                StreamDecodeError::Failed("Could not read local decoded PCM".into())
            })?;
            if count == 0 {
                break;
            }
            if bytes.len().saturating_add(count) > PCM_READ_BYTES + 3 {
                return Err(StreamDecodeError::Failed(
                    "PCM decoder chunk exceeded its bound".into(),
                ));
            }
            bytes.extend_from_slice(&buffer[..count]);
            let aligned = bytes.len() / 4 * 4;
            if aligned == 0 {
                continue;
            }
            let samples = bytes[..aligned]
                .chunks_exact(4)
                .map(|chunk| {
                    let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if value.is_finite() {
                        value.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>();
            bytes.drain(..aligned);
            sample_count = sample_count.saturating_add(samples.len());
            if decoded_pcm_exceeds_limits(sample_count, max_duration) {
                let _ = child.kill().await;
                return Err(StreamDecodeError::Failed(
                    "Decoded stream exceeds the verified duration or 15 minute PCM limit".into(),
                ));
            }
            let end_position_seconds = sample_count as f64 / KEY_PCM_RATE as f64;
            capture
                .push(PcmBatch {
                    generation,
                    sample_rate: KEY_PCM_RATE,
                    channels: 1,
                    end_position_seconds,
                    interleaved: &samples,
                })
                .map_err(|_| {
                    StreamDecodeError::Failed(
                        "Decoded stream contains a gap or unsupported PCM format".into(),
                    )
                })?;
        }
        if !bytes.is_empty() || sample_count == 0 {
            let _ = child.kill().await;
            return Err(StreamDecodeError::Failed(
                "Decoded stream ended with incomplete PCM".into(),
            ));
        }
        let status = child.wait().await.map_err(|_| {
            StreamDecodeError::Failed("Could not finish local audio decoder".into())
        })?;
        if !status.success() {
            return Err(StreamDecodeError::Failed(
                "Resolved stream could not be decoded completely".into(),
            ));
        }
        let actual_duration = sample_count as f64 / KEY_PCM_RATE as f64;
        if !duration_precision.accepts(expected_duration, actual_duration) {
            return Err(StreamDecodeError::Failed(
                "Resolved stream ended before verified full-recording coverage".into(),
            ));
        }
        natural_end_position = Some(actual_duration);
        Ok(())
    };
    let outcome = tokio::select! {
        biased;
        _ = cancel.wait_for(|cancelled| *cancelled) => Err(StreamDecodeError::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(MAX_REMOTE_DECODE_SECONDS), decode) => match result {
            Ok(result) => result,
            Err(_) => Err(StreamDecodeError::Failed("Stream analysis exceeded its 20 minute time limit".into())),
        },
    };
    outcome?;
    let natural_end_position = natural_end_position.ok_or_else(|| {
        StreamDecodeError::Failed("Resolved stream ended without measured PCM coverage".into())
    })?;
    tokio::task::spawn_blocking(move || {
        let measured_at = chrono::Utc::now().to_rfc3339();
        let finished = match duration_precision {
            StreamDurationPrecision::Exact => capture.finish(natural_end_position, measured_at),
            StreamDurationPrecision::RoundedToSecond => {
                capture.finish_with_rounded_duration(natural_end_position, measured_at)
            }
        };
        finished.map_err(|error| {
            StreamDecodeError::Failed(format!("Full stream analysis could not finish: {error}"))
        })
    })
    .await
    .map_err(|_| StreamDecodeError::Failed("Whole-recording analysis worker stopped".into()))?
}

fn decoded_pcm_exceeds_limits(sample_count: usize, max_duration: f64) -> bool {
    sample_count.saturating_mul(4) > MAX_PCM_BYTES
        || sample_count as f64 / KEY_PCM_RATE as f64 > max_duration + (1.0 / KEY_PCM_RATE as f64)
}

fn analysis_duration_precision(
    duration_seconds: f64,
    trusted_active_handoff: bool,
) -> StreamDurationPrecision {
    if !trusted_active_handoff
        && duration_seconds.is_finite()
        && (duration_seconds - duration_seconds.round()).abs() < 1e-6
    {
        StreamDurationPrecision::RoundedToSecond
    } else {
        StreamDurationPrecision::Exact
    }
}

fn format_headers(headers: &[(String, String)]) -> Result<Option<String>, StreamDecodeError> {
    if headers.is_empty() {
        return Ok(None);
    }
    if headers.len() > 32 {
        return Err(StreamDecodeError::Failed(
            "Resolved stream has too many request headers".into(),
        ));
    }
    let mut out = String::new();
    for (name, value) in headers {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || value.len() > 8192
            || value.contains('\r')
            || value.contains('\n')
        {
            return Err(StreamDecodeError::Failed(
                "Resolved stream request headers are invalid".into(),
            ));
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
        if out.len() > 32 * 8192 {
            return Err(StreamDecodeError::Failed(
                "Resolved stream request headers exceed the size limit".into(),
            ));
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::Track,
        stream_analysis::{BpmMeasurement, KeyMeasurement},
    };

    fn track() -> Track {
        Track {
            id: "fixture:stream-1".into(),
            provider: "fixture".into(),
            provider_id: "stream-1".into(),
            source_url: "https://catalog.example/stream-1".into(),
            title: "Fixture Recording".into(),
            artist: "Fixture Artist".into(),
            bpm: Some(100.0),
            key: Some("C".into()),
            provenance: serde_json::json!({"bpmSource":"catalog","keySource":"import","manualFields":{"key":true}}),
            ..Track::default()
        }
    }

    fn analysis() -> WholeTrackAnalysis {
        WholeTrackAnalysis {
            schema_version: crate::stream_analysis::SCHEMA_VERSION,
            analyzer_version: crate::stream_analysis::ANALYZER_VERSION.into(),
            resource: AnalysisResource::Stream {
                recording_id: "fixture:stream-1".into(),
                provider: "youtube".into(),
                source_identity_sha256: "a".repeat(64),
                audio_variant_sha256: None,
            },
            duration_seconds: 180.0,
            scope: "wholeTrack".into(),
            coverage: 1.0,
            measured_at: "2026-10-25T12:00:00Z".into(),
            bpm: Some(BpmMeasurement {
                bpm: 124.0,
                confidence: 0.9,
                related_tempo_alternative_bpm: Some(62.0),
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
    fn stream_revision_fences_track_edits_and_source_performance_identity() {
        let original = track();
        let record = analysis();
        let identity = canonical_analysis_identity(&original, &record.resource).unwrap();
        let mut edited = original.clone();
        edited.title.push_str(" remaster");
        assert_ne!(
            canonical_analysis_identity(&edited, &record.resource).unwrap(),
            identity
        );
        let other_performance = AnalysisResource::Stream {
            recording_id: original.id.clone(),
            provider: "youtube".into(),
            source_identity_sha256: "b".repeat(64),
            audio_variant_sha256: None,
        };
        assert_ne!(
            canonical_analysis_identity(&original, &other_performance).unwrap(),
            identity
        );
    }

    #[test]
    fn full_media_job_requires_the_exact_resolved_provider_source() {
        let candidate: crate::models::Candidate = serde_json::from_value(serde_json::json!({
            "id":"bandcamp-track-2039312545",
            "provider":"bandcamp",
            "url":"https://private.example/signed?token=not-persisted",
            "title":"Fixture Recording",
            "uploader":"Fixture Artist",
            "durationSeconds":180.0,
            "artworkUrl":null,
            "formats":[],
            "permission":"",
            "confidence":"",
            "score":1.0,
            "reasons":[],
            "official":false
        }))
        .unwrap();
        assert!(matches_expected_source(
            &candidate,
            Some(("bandcamp", "bandcamp-track-2039312545", 180.0))
        ));
        assert!(!matches_expected_source(
            &candidate,
            Some(("youtube", "bandcamp-track-2039312545", 180.0))
        ));
        assert!(!matches_expected_source(
            &candidate,
            Some(("bandcamp", "different-version", 180.0))
        ));
        // Resolver routing context never relaxes this identity check.
        for generation in [Some(1), Some(42), Some(MAX_SAFE_WEBVIEW_GENERATION)] {
            assert!(valid_webview_generation(generation));
            assert!(matches_expected_source(
                &candidate,
                Some(("bandcamp", "bandcamp-track-2039312545", 180.0))
            ));
            assert!(!matches_expected_source(
                &candidate,
                Some(("youtube", "bandcamp-track-2039312545", 180.0))
            ));
        }
    }

    #[test]
    fn active_youtube_handoff_requires_the_canonical_server_candidate() {
        let candidate = crate::models::Candidate {
            id: "youtube:lSmAz2rIBnE".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=lSmAz2rIBnE".into(),
            ..crate::models::Candidate::default()
        };
        assert!(exact_youtube_handoff_candidate(
            &candidate,
            "youtube",
            "youtube:lSmAz2rIBnE"
        ));
        assert!(!exact_youtube_handoff_candidate(
            &candidate,
            "youtube",
            "youtube:aaaaaaaaaaa"
        ));
        assert!(!exact_youtube_handoff_candidate(
            &candidate,
            "soundcloud",
            "youtube:lSmAz2rIBnE"
        ));
        let mut foreign_url = candidate.clone();
        foreign_url.url = "https://attacker.invalid/audio".into();
        assert!(!exact_youtube_handoff_candidate(
            &foreign_url,
            "youtube",
            "youtube:lSmAz2rIBnE"
        ));
    }

    #[test]
    fn rounded_resolver_duration_is_limited_to_trusted_active_source() {
        assert!(resolved_metadata_matches_active_duration(
            155.0,
            155.364_716_553,
            true,
        ));
        assert!(!resolved_metadata_matches_active_duration(
            155.0,
            155.364_716_553,
            false,
        ));
        assert!(!resolved_metadata_matches_active_duration(
            155.1,
            155.364_716_553,
            true,
        ));
        assert!(!resolved_metadata_matches_active_duration(
            156.5,
            155.364_716_553,
            true,
        ));
    }

    #[test]
    fn active_unsaved_catalog_source_gets_youtube_only_for_resolver_routing() {
        let mut original = track();
        original.duration_seconds = Some(155.364_716_553);
        let candidate = Candidate {
            id: "youtube:lSmAz2rIBnE".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=lSmAz2rIBnE".into(),
            title: "Resolver-owned title".into(),
            ..Candidate::default()
        };
        let resolver_track = active_youtube_resolver_track(&original, &candidate).unwrap();
        assert_eq!(resolver_track.provider, "youtube");
        assert_eq!(resolver_track.provider_id, "lSmAz2rIBnE");
        assert_eq!(
            resolver_track.source_url,
            "https://www.youtube.com/watch?v=lSmAz2rIBnE"
        );
        assert_eq!(resolver_track.id, original.id);
        assert_eq!(resolver_track.title, original.title);
        assert_eq!(original.provider, "fixture");
        assert_eq!(
            crate::playback_sources::direct_youtube_explicit_id(&resolver_track, &candidate, 2),
            Some("lSmAz2rIBnE"),
            "the existing direct-Youtube resolver branch accepts this exact active-source clone"
        );
        let mut versioned = resolver_track.clone();
        versioned.version = Some("Live".into());
        assert!(
            crate::playback_sources::direct_youtube_explicit_id(&versioned, &candidate, 2)
                .is_none()
        );
        assert!(active_youtube_resolver_track(
            &original,
            &Candidate {
                url: "https://attacker.invalid/watch?v=lSmAz2rIBnE".into(),
                ..candidate.clone()
            }
        )
        .is_none());
        let mut excerpt = original;
        excerpt.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "parent".into(),
            chapter_id: "clip".into(),
            source_url: candidate.url.clone(),
            start_time: 1.0,
            end_time: 5.0,
            parent_title: "Fixture Recording".into(),
            parent_duration_seconds: Some(180.0),
        });
        assert!(active_youtube_resolver_track(&excerpt, &candidate).is_none());
        let excerpt_resolver = crate::playback_sources::direct_youtube_explicit_id(
            &crate::models::Track {
                playback_range: excerpt.playback_range,
                ..resolver_track
            },
            &candidate,
            2,
        );
        assert!(excerpt_resolver.is_none());
    }

    #[test]
    fn exact_youtube_selection_requires_matching_backend_mapping_and_full_track() {
        let track = track();
        let mapping = crate::storage::VerifiedPlaybackSource {
            provider: "youtube".into(),
            provider_id: "lSmAz2rIBnE".into(),
            permanent_url: "https://www.youtube.com/watch?v=lSmAz2rIBnE".into(),
            verification_proof: None,
        };
        let candidate = verified_youtube_candidate(&track, "youtube:lSmAz2rIBnE", &mapping)
            .expect("matching exact source should use the verified mapping");
        assert_eq!(candidate.provider, "youtube");
        assert_eq!(candidate.id, "youtube:lSmAz2rIBnE");
        assert_eq!(candidate.url, mapping.permanent_url);
        assert_eq!(candidate.confidence, "review");

        assert!(verified_youtube_candidate(&track, "youtube:aaaaaaaaaaa", &mapping).is_none());
        assert!(verified_youtube_candidate(
            &track,
            "youtube:lSmAz2rIBnE",
            &crate::storage::VerifiedPlaybackSource {
                provider: "soundcloud".into(),
                ..mapping.clone()
            }
        )
        .is_none());
        assert!(verified_youtube_candidate(
            &track,
            "youtube:lSmAz2rIBnE",
            &crate::storage::VerifiedPlaybackSource {
                permanent_url: "https://www.youtube.com/watch?v=aaaaaaaaaaa".into(),
                ..mapping.clone()
            }
        )
        .is_none());

        let mut excerpt = track;
        excerpt.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "fixture:parent".into(),
            chapter_id: "chapter-1".into(),
            source_url: mapping.permanent_url.clone(),
            start_time: 10.0,
            end_time: 20.0,
            parent_title: "Fixture Recording".into(),
            parent_duration_seconds: Some(180.0),
        });
        assert!(verified_youtube_candidate(&excerpt, "youtube:lSmAz2rIBnE", &mapping).is_none());
    }

    #[test]
    fn webview_generation_is_optional_but_bounded_and_nonzero() {
        assert!(valid_webview_generation(None));
        assert!(!valid_webview_generation(Some(0)));
        assert!(valid_webview_generation(Some(1)));
        assert!(valid_webview_generation(Some(MAX_SAFE_WEBVIEW_GENERATION)));
        assert!(!valid_webview_generation(Some(
            MAX_SAFE_WEBVIEW_GENERATION + 1
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn decoder_requires_natural_eof_and_rejects_early_or_overlong_pcm() {
        use std::os::unix::fs::PermissionsExt;

        async fn decode_fixture(
            actual_seconds: f64,
            expected_duration: f64,
            duration_precision: StreamDurationPrecision,
        ) -> Result<WholeTrackAnalysis, StreamDecodeError> {
            let root = std::env::temp_dir().join(format!("burrata-pcm-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let raw = root.join("audio.pcm");
            let script = root.join("decoder.sh");
            let sample_count = (actual_seconds * KEY_PCM_RATE as f64).round() as usize;
            let pulse_period = KEY_PCM_RATE as usize / 2;
            let pulse_length = KEY_PCM_RATE as usize / 12;
            let mut pcm = Vec::with_capacity(sample_count * 4);
            for index in 0..sample_count {
                let within_beat = index % pulse_period;
                let sample = if within_beat < pulse_length {
                    let time = index as f64 / KEY_PCM_RATE as f64;
                    ((2.0 * std::f64::consts::PI * 80.0 * time).sin()
                        * (-(within_beat as f64) / (KEY_PCM_RATE as f64 * 0.04)).exp())
                        as f32
                } else {
                    0.0
                };
                pcm.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(&raw, pcm).unwrap();
            std::fs::write(&script, format!("#!/bin/sh\ncat '{}'\n", raw.display())).unwrap();
            let mut permissions = std::fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&script, permissions).unwrap();
            let resolved = ResolvedSource {
                origin: playback_sources::ResolvedSourceOrigin::Existing,
                source: crate::native_output::AudioSource {
                    input: "https://source.example/audio".into(),
                    ffmpeg: script,
                    headers: vec![],
                    decoder_hint: None,
                    cenc: None,
                },
                candidate: None,
                quality: serde_json::json!({}),
                station: None,
                duration: Some(expected_duration),
            };
            let (_cancel, mut cancel) = watch::channel(false);
            let resource = AnalysisResource::Stream {
                recording_id: "fixture:decoder".into(),
                provider: "youtube".into(),
                source_identity_sha256: "c".repeat(64),
                audio_variant_sha256: None,
            };
            let capture = match duration_precision {
                StreamDurationPrecision::Exact => {
                    WholeTrackCapture::new_with_waveform(7, resource, expected_duration, 0.0)
                }
                StreamDurationPrecision::RoundedToSecond => {
                    WholeTrackCapture::new_with_rounded_duration(
                        7,
                        resource,
                        expected_duration,
                        0.0,
                        true,
                    )
                }
            }
            .unwrap();
            let result = decode_resolved_stream(
                &resolved,
                capture,
                7,
                expected_duration,
                duration_precision,
                &mut cancel,
            )
            .await;
            let _ = std::fs::remove_dir_all(root);
            result
        }

        let exact = decode_fixture(32.0, 32.0, StreamDurationPrecision::Exact).await;
        assert!(
            exact.is_ok(),
            "natural exact EOF should complete: {exact:?}"
        );
        let early = decode_fixture(31.0, 32.0, StreamDurationPrecision::Exact).await;
        assert!(
            matches!(early, Err(StreamDecodeError::Failed(message)) if message.contains("ended before verified full-recording coverage"))
        );
        let overlong = decode_fixture(32.2, 32.0, StreamDurationPrecision::Exact).await;
        assert!(
            matches!(overlong, Err(StreamDecodeError::Failed(message)) if message.contains("exceeds the verified duration"))
        );

        let rounded = decode_fixture(32.364, 32.0, StreamDurationPrecision::RoundedToSecond).await;
        let analysis = rounded.expect("rounded integer metadata allows verified fractional EOF");
        assert!((analysis.duration_seconds - 32.364).abs() < 1.0 / KEY_PCM_RATE as f64);
        assert!((analysis.coverage - 1.0).abs() < 1e-6);
        let waveform = analysis.waveform.as_ref().unwrap();
        assert!((waveform.duration_seconds - analysis.duration_seconds).abs() < 1e-9);
        assert_eq!(
            waveform.bins.len(),
            crate::stream_analysis::STREAM_WAVEFORM_BINS
        );
        assert!(waveform.bins.iter().all(|bin| bin.min <= bin.max));

        let rounded_overrun =
            decode_fixture(33.001, 32.0, StreamDurationPrecision::RoundedToSecond).await;
        assert!(matches!(
            rounded_overrun,
            Err(StreamDecodeError::Failed(message))
                if message.contains("exceeds the verified duration")
                    || message.contains("ended before verified full-recording coverage")
        ));
    }

    #[test]
    fn rounded_duration_policy_is_provenance_bounded_and_keeps_the_900_second_cap() {
        let rounded = StreamDurationPrecision::RoundedToSecond;
        let exact = StreamDurationPrecision::Exact;
        assert_eq!(
            analysis_duration_precision(155.0, false),
            StreamDurationPrecision::RoundedToSecond
        );
        assert_eq!(
            analysis_duration_precision(155.0, true),
            StreamDurationPrecision::Exact
        );
        assert_eq!(
            analysis_duration_precision(155.3647, false),
            StreamDurationPrecision::Exact
        );
        assert!(rounded.accepts(155.0, 155.3647));
        assert!(!rounded.accepts(155.0, 156.0001));
        assert!(!exact.accepts(155.0, 155.3647));
        assert!(exact.accepts(155.3647, 155.3647));
        assert!(!rounded.accepts(155.3647, 155.5));
        assert_eq!(rounded.maximum_duration(900.0), 900.0);

        let max_samples = (MAX_TRACK_SECONDS * KEY_PCM_RATE as f64) as usize;
        assert!(!decoded_pcm_exceeds_limits(max_samples, 900.0));
        assert!(decoded_pcm_exceeds_limits(max_samples + 1, 900.0));
    }

    #[test]
    fn unsaved_media_context_is_stripped_of_urls_local_files_and_track_values() {
        let context = StreamMediaTrackContext {
            id: "search:recording-1".into(),
            provider: "catalog".into(),
            provider_id: "recording-1".into(),
            has_playback_range: false,
            artist: "Fixture Artist".into(),
            title: "Fixture Recording".into(),
            source_page_url: None,
            version: Some("Original Mix".into()),
            album: Some("Fixture Album".into()),
            album_artist: None,
            duration_seconds: Some(180.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        let track = context
            .clone()
            .into_sanitized_track(180.0, "youtube", "verified-video-id")
            .unwrap();
        assert_eq!(track.id, "search:recording-1");
        assert_eq!(track.source_url, "");
        assert!(track.preview_url.is_none());
        assert!(track.artwork_url.is_none());
        assert!(track.local_path.is_none());
        assert!(track.asset_id.is_none());
        assert!(track.playback_range.is_none());
        assert!(track.bpm.is_none());
        assert!(track.key.is_none());
        assert!(track.provenance.is_null());
        assert!(StreamMediaTrackContext {
            has_playback_range: true,
            ..context
        }
        .into_sanitized_track(180.0, "youtube", "verified-video-id")
        .is_err());

        let youtube_context = StreamMediaTrackContext {
            id: "search:youtube-recording".into(),
            provider: "youtube".into(),
            provider_id: "lSmAz2rIBnE".into(),
            has_playback_range: false,
            artist: "Chico and The Gypsies".into(),
            title: "Tengo tengo".into(),
            source_page_url: Some("https://www.youtube.com/watch?v=lSmAz2rIBnE".into()),
            version: None,
            album: None,
            album_artist: None,
            duration_seconds: Some(155.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        let youtube_track = youtube_context
            .clone()
            .into_sanitized_track(155.364_716_553, "youtube", "youtube:lSmAz2rIBnE")
            .unwrap();
        assert_eq!(
            youtube_track.source_url,
            "https://www.youtube.com/watch?v=lSmAz2rIBnE"
        );
        assert!(youtube_context
            .into_sanitized_track(155.364_716_553, "youtube", "youtube:foreignvideo1")
            .is_err());

        let bandcamp_context = StreamMediaTrackContext {
            id: "search:bandcamp-recording".into(),
            provider: "bandcamp".into(),
            provider_id: "2039312545".into(),
            has_playback_range: false,
            artist: "Amynedd".into(),
            title: "Dingle Dell".into(),
            source_page_url: Some("https://amynedd.bandcamp.com/track/dingle-dell".into()),
            version: None,
            album: None,
            album_artist: None,
            duration_seconds: Some(210.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        let bandcamp_track = bandcamp_context
            .into_sanitized_track(210.0, "bandcamp", "2039312545")
            .unwrap();
        assert_eq!(
            bandcamp_track.source_url,
            "https://amynedd.bandcamp.com/track/dingle-dell"
        );
        let unsafe_page = StreamMediaTrackContext {
            id: "search:bandcamp-recording".into(),
            provider: "bandcamp".into(),
            provider_id: "2039312545".into(),
            has_playback_range: false,
            artist: "Amynedd".into(),
            title: "Dingle Dell".into(),
            source_page_url: Some(
                "https://amynedd.bandcamp.com/track/dingle-dell?token=secret".into(),
            ),
            version: None,
            album: None,
            album_artist: None,
            duration_seconds: Some(210.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        assert!(unsafe_page
            .into_sanitized_track(210.0, "bandcamp", "2039312545")
            .is_err());

        assert!(
            serde_json::from_value::<StreamMediaTrackContext>(serde_json::json!({
                "id":"search:recording-1",
                "provider":"catalog",
                "providerId":"recording-1",
                "artist":"Fixture Artist",
                "title":"Fixture Recording",
                "sourceUrl":"https://secret.example/audio?token=private"
            }))
            .is_err()
        );
    }

    #[test]
    fn blank_artist_projection_requires_exact_active_youtube_source_context() {
        let context = StreamMediaTrackContext {
            id: "search:musicbrainz-123".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz-123".into(),
            has_playback_range: false,
            artist: String::new(),
            title: "Chico And The Gypsies - Tengo Tengo".into(),
            source_page_url: None,
            version: None,
            album: None,
            album_artist: None,
            duration_seconds: Some(155.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        assert!(context
            .clone()
            .into_sanitized_track(155.364_716_553, "youtube", "youtube:lSmAz2rIBnE")
            .is_err());
        let active = context
            .clone()
            .into_active_youtube_handoff_track(155.364_716_553, "youtube:lSmAz2rIBnE")
            .unwrap();
        assert_eq!(active.id, "search:musicbrainz-123");
        assert!(active.artist.is_empty());
        assert!(active.local_path.is_none());
        assert!(active.asset_id.is_none());
        assert!(active.provenance.is_null());
        assert!(context
            .clone()
            .into_active_youtube_handoff_track(155.364_716_553, "lSmAz2rIBnE")
            .is_err());
        assert!(context
            .clone()
            .into_active_youtube_handoff_track(155.364_716_553, "youtube:foreign-video")
            .is_err());
        let ranged = StreamMediaTrackContext {
            has_playback_range: true,
            ..context.clone()
        };
        assert!(ranged
            .into_active_youtube_handoff_track(155.364_716_553, "youtube:lSmAz2rIBnE")
            .is_err());
        let malformed_page = StreamMediaTrackContext {
            source_page_url: Some("https://youtube.com/watch?v=foreignvideo1".into()),
            ..context.clone()
        };
        assert!(malformed_page
            .into_active_youtube_handoff_track(155.364_716_553, "youtube:lSmAz2rIBnE")
            .is_err());
        let missing_title = StreamMediaTrackContext {
            title: String::new(),
            ..context
        };
        assert!(missing_title
            .into_active_youtube_handoff_track(155.364_716_553, "youtube:lSmAz2rIBnE")
            .is_err());
    }

    #[tokio::test]
    async fn full_media_handoff_analysis_cannot_apply_library_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let jobs = SelectedStreamAnalysisJobs::new();
        let (cancel, _receiver) = watch::channel(false);
        let job_id = "full-media-cache-only".to_owned();
        jobs.jobs.lock().unwrap().insert(
            job_id.clone(),
            JobEntry {
                snapshot: SelectedStreamAnalysisJob {
                    job_id: job_id.clone(),
                    state: "completed".into(),
                    total: 1,
                    completed: 1,
                    results: vec![],
                },
                cancel,
                fields: [AnalysisMetric::Bpm].into_iter().collect(),
                allow_metadata_apply: false,
            },
        );
        let error = jobs
            .apply(
                &store,
                &job_id,
                "search:musicbrainz-123",
                &ApplyAnalysisSelection {
                    fields: [AnalysisMetric::Bpm].into_iter().collect(),
                    policies: Default::default(),
                },
            )
            .await
            .unwrap_err();
        assert!(error.contains("never applies BPM or key to saved metadata"));
    }

    #[test]
    fn request_headers_reject_line_injection_and_never_enter_job_dto() {
        assert!(
            format_headers(&[("Authorization".into(), "Bearer secret\r\nHost: evil".into())])
                .is_err()
        );
        let row = SelectedStreamAnalysisResult {
            track_id: "fixture:stream-1".into(),
            recording_id: "fixture:stream-1".into(),
            status: "failed".into(),
            provider: Some("youtube".into()),
            source_id: Some("stable-video-id".into()),
            analysis: None,
            error: Some("No verified full-recording stream could be resolved".into()),
            cache_hit: false,
            elapsed_ms: None,
            canonical_identity: Some("private-hash".into()),
            verified_mapping_fingerprint: Some(None),
        };
        let encoded = serde_json::to_string(&row).unwrap();
        assert!(!encoded.contains("https://"));
        assert!(!encoded.contains("Authorization"));
        assert!(!encoded.contains("private-hash"));
    }

    #[test]
    fn stream_apply_keeps_imported_fields_unless_replace_is_selected() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let current = track();
        store.save_track(&current).unwrap();
        let record = analysis();
        let identity = canonical_analysis_identity(&current, &record.resource).unwrap();
        let fill_missing = ApplyAnalysisSelection {
            fields: [AnalysisMetric::Bpm, AnalysisMetric::Key]
                .into_iter()
                .collect(),
            policies: Default::default(),
        };
        let unchanged = store
            .apply_stream_analyzed_track(
                &current.id,
                &identity,
                "youtube",
                None,
                &record,
                &fill_missing,
            )
            .unwrap();
        assert_eq!(unchanged.bpm, Some(100.0));
        assert_eq!(unchanged.key.as_deref(), Some("C"));
        assert_eq!(unchanged.provenance["bpmSource"], "catalog");
        assert_eq!(unchanged.provenance["manualFields"]["key"], true);

        let replace_bpm = ApplyAnalysisSelection {
            fields: [AnalysisMetric::Bpm].into_iter().collect(),
            policies: [(
                AnalysisMetric::Bpm,
                crate::stream_analysis::FieldPolicy::Replace,
            )]
            .into_iter()
            .collect(),
        };
        let replaced = store
            .apply_stream_analyzed_track(
                &current.id,
                &identity,
                "youtube",
                None,
                &record,
                &replace_bpm,
            )
            .unwrap();
        assert_eq!(replaced.bpm, Some(124.0));
        assert_eq!(replaced.key.as_deref(), Some("C"));
        assert_eq!(replaced.provenance["manualFields"]["bpm"], true);

        let mut edited = store.get_track(&current.id).unwrap().unwrap();
        edited.title = "User-edited title".into();
        store.update_track(&edited).unwrap();
        assert!(store
            .apply_stream_analyzed_track(
                &current.id,
                &identity,
                "youtube",
                None,
                &record,
                &replace_bpm,
            )
            .is_err());
    }

    #[test]
    fn stream_apply_rejects_verified_source_mapping_changes() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let current = track();
        store.save_track(&current).unwrap();
        let recording_fingerprint =
            playback_sources::recording_identity_fingerprint(&current).unwrap();
        store
            .save_verified_playback_source(
                &current.id,
                &recording_fingerprint,
                "youtube",
                "abcdefghijk",
                "https://www.youtube.com/watch?v=abcdefghijk",
            )
            .unwrap();
        let reviewed_mapping = store
            .verified_playback_source_mapping_fingerprint(
                &current.id,
                &recording_fingerprint,
                "youtube",
            )
            .unwrap()
            .unwrap();

        // A routine refresh of the same stable mapping changes expiry and
        // refresh time only, so it must not invalidate the reviewed result.
        store
            .save_verified_playback_source(
                &current.id,
                &recording_fingerprint,
                "youtube",
                "abcdefghijk",
                "https://www.youtube.com/watch?v=abcdefghijk",
            )
            .unwrap();
        assert_eq!(
            store
                .verified_playback_source_mapping_fingerprint(
                    &current.id,
                    &recording_fingerprint,
                    "youtube",
                )
                .unwrap()
                .as_deref(),
            Some(reviewed_mapping.as_str())
        );

        store
            .save_verified_playback_source(
                &current.id,
                &recording_fingerprint,
                "youtube",
                "lmnopqrstuv",
                "https://www.youtube.com/watch?v=lmnopqrstuv",
            )
            .unwrap();
        let changed_mapping = store
            .verified_playback_source_mapping_fingerprint(
                &current.id,
                &recording_fingerprint,
                "youtube",
            )
            .unwrap()
            .unwrap();
        assert_ne!(changed_mapping, reviewed_mapping);

        let record = analysis();
        let identity = canonical_analysis_identity(&current, &record.resource).unwrap();
        let selection = ApplyAnalysisSelection {
            fields: [AnalysisMetric::Bpm].into_iter().collect(),
            policies: Default::default(),
        };
        let error = store
            .apply_stream_analyzed_track(
                &current.id,
                &identity,
                "youtube",
                Some(&reviewed_mapping),
                &record,
                &selection,
            )
            .unwrap_err();
        assert!(error.to_string().contains("mapping changed"));
    }
}
