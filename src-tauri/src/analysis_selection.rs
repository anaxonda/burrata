//! Explicit, read-only analysis jobs for selected registered audio assets.
//!
//! This path writes the hash-bound analysis cache only. Track metadata changes
//! require a separate reviewed apply command and never retag source audio.

use crate::{
    stream_analysis::{
        analyze_registered_asset, AnalysisMetric, AnalysisResource, ApplyAnalysisSelection,
        WholeTrackAnalysis, MAX_ASSET_BYTES,
    },
    Store,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::Read,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub const MAX_SELECTED_ASSETS: usize = 200;
const MAX_RETAINED_JOBS: usize = 24;
const MAX_ACTIVE_JOBS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedAssetAnalysisResult {
    pub asset_id: String,
    pub recording_id: String,
    pub status: String,
    pub analysis: Option<WholeTrackAnalysis>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedAssetAnalysisJob {
    pub job_id: String,
    pub state: String,
    pub total: usize,
    pub completed: usize,
    pub results: Vec<SelectedAssetAnalysisResult>,
}

struct JobEntry {
    snapshot: SelectedAssetAnalysisJob,
    cancel: watch::Sender<bool>,
}

#[derive(Default)]
pub struct AnalysisSelectionJobs {
    jobs: Mutex<HashMap<String, JobEntry>>,
}

impl AnalysisSelectionJobs {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl AnalysisSelectionJobs {
    pub fn start(
        self: &Arc<Self>,
        store: Arc<Store>,
        asset_ids: Vec<String>,
        fields: BTreeSet<AnalysisMetric>,
    ) -> Result<SelectedAssetAnalysisJob, String> {
        if asset_ids.is_empty() || asset_ids.len() > MAX_SELECTED_ASSETS {
            return Err(format!("Select 1–{MAX_SELECTED_ASSETS} local audio assets"));
        }
        if fields.is_empty() {
            return Err("Choose BPM, key, or both for analysis".into());
        }
        if asset_ids.iter().any(|id| id.trim().is_empty()) {
            return Err("Selected audio asset IDs cannot be empty".into());
        }
        let unique = asset_ids.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != asset_ids.len() {
            return Err("Selected audio assets must be unique".into());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let (cancel, mut cancel_rx) = watch::channel(false);
        let snapshot = SelectedAssetAnalysisJob {
            job_id: job_id.clone(),
            state: "queued".into(),
            total: asset_ids.len(),
            completed: 0,
            results: Vec::with_capacity(asset_ids.len()),
        };
        let writer_lease = store.try_library_write()?;
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "Analysis job list is unavailable")?;
            let active_count = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.snapshot.state.as_str(),
                        "queued" | "running" | "cancelling"
                    )
                })
                .count();
            if active_count >= MAX_ACTIVE_JOBS {
                return Err(
                    "Finish or cancel another selected analysis before starting a new one".into(),
                );
            }
            if jobs.len() >= MAX_RETAINED_JOBS {
                let remove_id = jobs
                    .iter()
                    .find(|(_, entry)| {
                        matches!(entry.snapshot.state.as_str(), "completed" | "cancelled")
                    })
                    .map(|(id, _)| id.clone())
                    .ok_or("Analysis job history is full")?;
                jobs.remove(&remove_id);
            }
            jobs.insert(
                job_id.clone(),
                JobEntry {
                    snapshot: snapshot.clone(),
                    cancel,
                },
            );
        }

        let jobs = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let _writer_lease = writer_lease;
            jobs.update(&job_id, |job| job.state = "running".into());
            for asset_id in asset_ids {
                if *cancel_rx.borrow() {
                    break;
                }
                let recording_id = store
                    .get_asset_by_id(&asset_id)
                    .ok()
                    .flatten()
                    .map(|asset| asset.recording_id)
                    .unwrap_or_default();
                let result =
                    analyze_registered_asset(store.clone(), &asset_id, &fields, &mut cancel_rx)
                        .await;
                let row = match result {
                    Ok(analysis) => SelectedAssetAnalysisResult {
                        asset_id,
                        recording_id: analysis.resource.recording_id().to_owned(),
                        status: "completed".into(),
                        analysis: Some(analysis),
                        error: None,
                    },
                    Err(error) => SelectedAssetAnalysisResult {
                        asset_id,
                        recording_id,
                        status: if error == crate::stream_analysis::AnalysisError::Cancelled {
                            "cancelled"
                        } else {
                            "failed"
                        }
                        .into(),
                        analysis: None,
                        error: Some(safe_analysis_error(&error)),
                    },
                };
                let cancelled = row.status == "cancelled";
                jobs.update(&job_id, |job| {
                    job.completed = job.completed.saturating_add(1);
                    job.results.push(row);
                    if cancelled {
                        job.state = "cancelled".into();
                    }
                });
                if cancelled {
                    break;
                }
            }
            jobs.update(&job_id, |job| match job.state.as_str() {
                "cancelled" => {}
                "cancelling" => job.state = "cancelled".into(),
                _ => job.state = "completed".into(),
            });
        });
        Ok(snapshot)
    }

    pub fn get(&self, job_id: &str) -> Result<SelectedAssetAnalysisJob, String> {
        self.jobs
            .lock()
            .map_err(|_| "Analysis job list is unavailable")?
            .get(job_id)
            .map(|entry| entry.snapshot.clone())
            .ok_or_else(|| "Selected analysis job was not found".into())
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| "Analysis job list is unavailable")?;
        let entry = jobs
            .get_mut(job_id)
            .ok_or_else(|| "Selected analysis job was not found".to_owned())?;
        if !matches!(
            entry.snapshot.state.as_str(),
            "queued" | "running" | "cancelling"
        ) {
            return Ok(());
        }
        entry.snapshot.state = "cancelling".into();
        entry.cancel.send_replace(true);
        Ok(())
    }

    pub async fn apply(
        &self,
        store: &Store,
        job_id: &str,
        asset_id: &str,
        selection: &ApplyAnalysisSelection,
    ) -> Result<crate::models::Track, String> {
        let job = self.get(job_id)?;
        if !matches!(job.state.as_str(), "completed" | "running") {
            return Err("Wait for the selected analysis to finish before applying results".into());
        }
        let analysis = job
            .results
            .iter()
            .find(|result| result.asset_id == asset_id && result.status == "completed")
            .and_then(|result| result.analysis.as_ref())
            .ok_or_else(|| "This asset has no completed analysis result".to_owned())?;
        let asset = store
            .get_asset_by_id(asset_id)
            .map_err(|_| "Could not read the registered audio asset".to_owned())?
            .ok_or_else(|| "The registered audio asset no longer exists".to_owned())?;
        match &analysis.resource {
            AnalysisResource::RegisteredAsset {
                asset_id: result_asset_id,
                recording_id,
                file_hash,
            } if result_asset_id == asset_id
                && recording_id == &asset.recording_id
                && file_hash == &asset.hash => {}
            _ => return Err("The analysis no longer matches this audio asset".into()),
        }
        if analysis.resource.recording_id() != asset.track.id {
            return Err("The analysis recording identity changed".into());
        }
        verify_current_file(asset.path.clone(), asset.hash.clone()).await?;
        store
            .apply_analyzed_track(asset_id, &asset.hash, analysis, selection)
            .map_err(|_| "Could not save the reviewed BPM/key values".into())
    }

    fn update(&self, job_id: &str, update: impl FnOnce(&mut SelectedAssetAnalysisJob)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(entry) = jobs.get_mut(job_id) {
                update(&mut entry.snapshot);
            }
        }
    }
}

fn safe_analysis_error(error: &crate::stream_analysis::AnalysisError) -> String {
    use crate::stream_analysis::AnalysisError as E;
    match error {
        E::InvalidResource => "Audio asset identity is invalid".into(),
        E::InvalidMeasurement => "Analysis did not produce a valid measurement".into(),
        E::WrongRecording => "Analysis belongs to another recording".into(),
        E::IncompleteCapture => "Playback did not cover the full audio track".into(),
        E::DiscontinuousCapture => "Playback had a gap, seek, or source change".into(),
        E::UnsupportedDuration => "Supported duration is 8 seconds to 15 minutes".into(),
        E::UnsupportedFormat => "Audio format is not supported for analysis".into(),
        E::CaptureNotStartedAtTrackBeginning => {
            "Start playback from the beginning to analyze the full track".into()
        }
        E::NotRegisteredAsset => "Select a registered local audio asset".into(),
        E::ResourceChanged => "Audio changed during analysis; results were discarded".into(),
        E::FileTooLarge => "Audio file exceeds the 8 GB analysis limit".into(),
        E::Cancelled => "Analysis was cancelled".into(),
        E::DecodeFailed(_) => "Audio could not be decoded for analysis".into(),
    }
}

async fn verify_current_file(path: String, expected_hash: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || verify_current_file_blocking(&path, &expected_hash))
        .await
        .map_err(|_| "Could not verify the registered audio file".to_owned())?
}

fn verify_current_file_blocking(path: &str, expected_hash: &str) -> Result<(), String> {
    let supplied = Path::new(path);
    if !supplied.is_absolute()
        || std::fs::symlink_metadata(supplied)
            .map_err(|_| "The registered audio file is unavailable".to_owned())?
            .file_type()
            .is_symlink()
    {
        return Err("The registered audio file is unavailable".into());
    }
    let canonical = std::fs::canonicalize(supplied)
        .map_err(|_| "The registered audio file is unavailable".to_owned())?;
    if canonical != supplied || !canonical.is_file() {
        return Err("The registered audio file is unavailable".into());
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|_| "The registered audio file is unavailable".to_owned())?;
    if metadata.len() > MAX_ASSET_BYTES {
        return Err("Audio file exceeds the 8 GB analysis limit".into());
    }
    let actual = hash_file(&canonical)?;
    if actual != expected_hash {
        return Err("Audio changed after analysis; results were discarded".into());
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = File::open(path).map_err(|_| "The registered audio file is unavailable")?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "Could not verify the registered audio file")?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{LocalAsset, Track},
        stream_analysis::{BpmMeasurement, KeyMeasurement},
    };
    use std::collections::BTreeMap;

    fn track() -> Track {
        Track {
            id: "fixture:recording-1".into(),
            provider: "fixture".into(),
            provider_id: "recording-1".into(),
            source_url: "https://example.test/recording-1".into(),
            artist: "Fixture Artist".into(),
            title: "Fixture Recording".into(),
            bpm: Some(100.0),
            key: Some("C".into()),
            provenance: serde_json::json!({"bpmSource":"catalog", "keySource":"import", "manualFields":{"key":true}}),
            ..Track::default()
        }
    }

    fn result(asset_id: &str, file_hash: &str) -> WholeTrackAnalysis {
        WholeTrackAnalysis {
            schema_version: crate::stream_analysis::SCHEMA_VERSION,
            analyzer_version: crate::stream_analysis::ANALYZER_VERSION.into(),
            resource: AnalysisResource::RegisteredAsset {
                asset_id: asset_id.into(),
                recording_id: "fixture:recording-1".into(),
                file_hash: file_hash.into(),
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

    fn job_with(asset_id: &str, analysis: WholeTrackAnalysis) -> JobEntry {
        let (cancel, _) = watch::channel(false);
        JobEntry {
            snapshot: SelectedAssetAnalysisJob {
                job_id: "reviewed-job".into(),
                state: "completed".into(),
                total: 1,
                completed: 1,
                results: vec![SelectedAssetAnalysisResult {
                    asset_id: asset_id.into(),
                    recording_id: analysis.resource.recording_id().into(),
                    status: "completed".into(),
                    analysis: Some(analysis),
                    error: None,
                }],
            },
            cancel,
        }
    }

    fn missing_only_selection() -> ApplyAnalysisSelection {
        ApplyAnalysisSelection {
            fields: [AnalysisMetric::Bpm, AnalysisMetric::Key]
                .into_iter()
                .collect(),
            policies: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn forged_job_and_wrong_asset_cannot_apply_a_measurement() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let jobs = AnalysisSelectionJobs::default();
        assert!(jobs
            .apply(&store, "forged-job", "asset-a", &missing_only_selection())
            .await
            .is_err());
        jobs.jobs.lock().unwrap().insert(
            "reviewed-job".into(),
            job_with("asset-a", result("asset-a", &"a".repeat(64))),
        );
        assert!(jobs
            .apply(&store, "reviewed-job", "asset-b", &missing_only_selection())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn apply_rejects_audio_that_changed_after_analysis() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let mut saved_track = track();
        saved_track.bpm = Some(100.0);
        saved_track.key = Some("C".into());
        store.save_track(&saved_track).unwrap();
        let audio_path = directory.path().join("fixture.wav");
        std::fs::write(&audio_path, b"original-analysis-input").unwrap();
        let original_hash = hash_file(&audio_path).unwrap();
        let asset = LocalAsset {
            id: "asset-a".into(),
            recording_id: saved_track.id.clone(),
            // Registered assets store the canonical native path. macOS tempdir
            // paths may be exposed as /var/... while canonicalize resolves the
            // system alias to /private/var/....
            path: std::fs::canonicalize(&audio_path)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            hash: original_hash.clone(),
            source_codec: "wav".into(),
            final_codec: "wav".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(44_100),
            channels: Some(2),
            bit_depth: Some(16),
            duration_seconds: Some(180.0),
            size_bytes: Some(24),
            bitrate_origin: None,
            processing: "ready".into(),
            track: saved_track.clone(),
        };
        store.save_asset(&asset).unwrap();
        let jobs = AnalysisSelectionJobs::default();
        jobs.jobs.lock().unwrap().insert(
            "reviewed-job".into(),
            job_with("asset-a", result("asset-a", &original_hash)),
        );
        std::fs::write(&audio_path, b"changed-after-analysis").unwrap();
        let error = jobs
            .apply(&store, "reviewed-job", "asset-a", &missing_only_selection())
            .await
            .unwrap_err();
        assert!(error.contains("Audio changed after analysis"));
        let unchanged = store.get_asset_by_id("asset-a").unwrap().unwrap();
        assert_eq!(unchanged.track.bpm, Some(100.0));
        assert_eq!(unchanged.track.key.as_deref(), Some("C"));
    }

    #[test]
    fn fill_missing_keeps_non_null_imported_fields() {
        let measured = result("asset-a", &"a".repeat(64));
        let current = track();
        let applied =
            crate::stream_analysis::apply_analysis(&current, &measured, &missing_only_selection())
                .unwrap();
        assert_eq!(applied.bpm, Some(100.0));
        assert_eq!(applied.key.as_deref(), Some("C"));
        assert_eq!(applied.provenance["bpmSource"], "catalog");
        assert_eq!(applied.provenance["keySource"], "import");
        assert_eq!(applied.provenance["manualFields"]["key"], true);
    }
}
