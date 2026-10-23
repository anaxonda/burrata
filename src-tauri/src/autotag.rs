//! Conservative catalog matching for newly downloaded managed assets.
//!
//! Queue records use the dedicated durable autotag table and are enqueued only
//! after a newly downloaded asset has been committed to the managed library.
use crate::{
    artists::ArtistService,
    media,
    models::{Candidate, DownloadJob, LocalAsset, Settings, Track},
    Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, process::Stdio, sync::Arc, time::Duration};
use strsim::jaro_winkler;
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex, Semaphore};

pub const VERSION: &str = "autotag-v1";
const SETTINGS_KEY: &str = "autotag.settings";
const MAX_DURATION_DELTA: f64 = 10.0;
const MAX_DURATION_RATIO: f64 = 0.03;
const MAX_PROVIDER_ATTEMPTS: u32 = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoTagJob {
    pub asset_id: String,
    pub input_hash: String,
    pub version: String,
    pub status: String,
    pub attempts: u32,
    pub next_attempt_at: Option<String>,
    pub message: Option<String>,
    pub candidates: Vec<AutoTagCandidate>,
    /// Audio-recognition evidence is separate from catalog candidates: it can
    /// support human review, but never by itself authorizes catalog tagging.
    #[serde(default)]
    pub recognition: Option<AudioRecognition>,
    pub chosen: Option<AutoTagCandidate>,
    #[serde(default)]
    pub approved_fields: Vec<String>,
    #[serde(default)]
    pub excerpt_only: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSourceTagRepairResult {
    pub asset_id: String,
    pub changed: bool,
    pub fields: Vec<String>,
    pub identity_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioRecognition {
    pub provider: String,
    pub artist: String,
    pub title: String,
    pub isrc: Option<String>,
    pub shazam_id: Option<String>,
    pub excerpt_count: u8,
    /// Identifiers explicitly observed with the same value in both excerpts.
    /// Legacy records default to no agreement, regardless of excerpt_count.
    #[serde(default)]
    pub agreeing_identifiers: crate::recording_identity::RecognitionIdentifiers,
}

struct SongRecOutcome {
    recognition: AudioRecognition,
    candidate: Option<AutoTagCandidate>,
}

// Recognition agreement is independent of optional catalog enrichment.
// The result alone does not authorize catalog tags or identity binding.
pub(crate) fn consistent_songrec_excerpts(
    first: Option<crate::radio::RadioTrackMetadata>,
    second: Option<crate::radio::RadioTrackMetadata>,
) -> Option<AudioRecognition> {
    let (Some(artist_a), Some(artist_b)) = (first, second) else {
        return None;
    };
    if artist_a.artist.trim().is_empty()
        || artist_a.title.trim().is_empty()
        || normalize(&artist_a.artist) != normalize(&artist_b.artist)
        || normalize(&artist_a.title) != normalize(&artist_b.title)
    {
        return None;
    }
    let identifiers_a = artist_a.identifiers.clone().unwrap_or_default();
    let identifiers_b = artist_b.identifiers.clone().unwrap_or_default();
    if identifiers_a
        .isrc
        .as_deref()
        .zip(identifiers_b.isrc.as_deref())
        .is_some_and(|(a, b)| !crate::recording_identity::same_isrc(a, b))
    {
        return None;
    }
    if identifiers_a
        .shazam_id
        .as_deref()
        .zip(identifiers_b.shazam_id.as_deref())
        .is_some_and(|(a, b)| a != b)
    {
        return None;
    }
    if identifiers_a
        .apple_music_url
        .as_deref()
        .zip(identifiers_b.apple_music_url.as_deref())
        .is_some_and(|(a, b)| a != b)
    {
        return None;
    }
    let agreeing_identifiers = crate::recording_identity::RecognitionIdentifiers {
        isrc: identifiers_a
            .isrc
            .as_deref()
            .zip(identifiers_b.isrc.as_deref())
            .filter(|(a, b)| crate::recording_identity::same_isrc(a, b))
            .and_then(|(a, _)| crate::recording_identity::normalize_isrc(a)),
        shazam_id: identifiers_a
            .shazam_id
            .as_ref()
            .zip(identifiers_b.shazam_id.as_ref())
            .filter(|(a, b)| a == b)
            .map(|(a, _)| a.clone()),
        apple_music_url: identifiers_a
            .apple_music_url
            .as_ref()
            .zip(identifiers_b.apple_music_url.as_ref())
            .filter(|(a, b)| a == b)
            .map(|(a, _)| a.clone()),
    };
    let recognized_isrc = identifiers_a
        .isrc
        .or(identifiers_b.isrc)
        .and_then(|isrc| crate::recording_identity::normalize_isrc(&isrc));
    Some(AudioRecognition {
        provider: "songrec".into(),
        artist: artist_a.artist.clone(),
        title: artist_a.title.clone(),
        isrc: recognized_isrc.clone(),
        shazam_id: identifiers_a.shazam_id.or(identifiers_b.shazam_id),
        excerpt_count: 2,
        agreeing_identifiers,
    })
}

fn songrec_catalog_outcome(
    source: &Track,
    recognition: AudioRecognition,
    rows: Result<Value, String>,
) -> SongRecOutcome {
    let Ok(rows) = rows else {
        return SongRecOutcome {
            recognition,
            candidate: None,
        };
    };
    let recognized_isrc = recognition.isrc.clone();
    let identified = Track {
        artist: recognition.artist.clone(),
        title: recognition.title.clone(),
        duration_seconds: source.duration_seconds,
        isrc: recognized_isrc.clone(),
        ..Track::default()
    };
    let candidate_rows = if let Some(isrc) = recognized_isrc.as_deref() {
        unique_musicbrainz_rows_for_isrc(&rows, isrc)
    } else {
        rows["recordings"].as_array().cloned().unwrap_or_default()
    };
    let mut candidates = candidate_rows
        .iter()
        .filter_map(|row| candidate_from_mb(row, &identified))
        .collect::<Vec<_>>();
    if let Some(isrc) = recognized_isrc.as_ref() {
        for candidate in &mut candidates {
            candidate.fields.isrc = Some(isrc.clone());
        }
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    if candidates.len() != 1 || candidates[0].score < 0.96 {
        return SongRecOutcome {
            recognition,
            candidate: None,
        };
    }
    let mut candidate = candidates.remove(0);
    candidate.evidence.push(if recognized_isrc.is_some() {
            "SongRec recognized two non-overlapping excerpts consistently; exact ISRC resolved one matching MusicBrainz recording.".into()
        } else {
            "SongRec recognized two non-overlapping excerpts consistently; MusicBrainz artist, title, and duration also matched.".into()
        });
    if source_conflicts(source, &candidate) {
        candidate.evidence.push(
            "Recognized artist or title conflicts with the downloaded source; review required."
                .into(),
        );
    }
    SongRecOutcome {
        recognition,
        candidate: Some(candidate),
    }
}

fn catalog_result_or_empty<T: Default>(result: Result<T, String>, errors: &mut Vec<String>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            errors.push(error);
            T::default()
        }
    }
}

fn finish_autotag_lookup(
    candidates: Vec<AutoTagCandidate>,
    recognition: Option<AudioRecognition>,
    mut errors: Vec<String>,
) -> Result<AutoTagLookup, String> {
    if candidates.is_empty() && recognition.is_none() && !errors.is_empty() {
        return Err(errors.remove(0));
    }
    Ok(AutoTagLookup {
        candidates,
        recognition,
    })
}

fn mark_release_context_unavailable(candidate: &mut AutoTagCandidate) {
    if !candidate.fields.provenance.is_object() {
        candidate.fields.provenance = json!({});
    }
    candidate.fields.provenance["releaseContextUnavailable"] = json!(true);
    candidate.evidence.push("Catalog release context is unavailable; review required without inferring missing release fields.".into());
}

struct AutoTagLookup {
    candidates: Vec<AutoTagCandidate>,
    recognition: Option<AudioRecognition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoTagCandidate {
    pub provider: String,
    pub provider_id: String,
    pub source_url: String,
    pub title: String,
    pub artist: String,
    pub duration_seconds: Option<f64>,
    pub score: f64,
    pub fields: Track,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutoTagSettings {
    pub enabled: bool,
    pub shazam_fallback: bool,
    pub fpcalc_available: bool,
    pub songrec_available: bool,
    pub acoustid_configured: bool,
}

#[derive(Clone)]
pub struct AutoTagger {
    store: Arc<Store>,
    artists: Arc<ArtistService>,
    connections: Arc<crate::connections::ConnectionManager>,
    app: tauri::AppHandle,
    sender: mpsc::UnboundedSender<String>,
    workers: Arc<Semaphore>,
    fingerprint_gate: Arc<Mutex<()>>,
    fingerprint_cpu: Arc<Semaphore>,
    inflight: Arc<Mutex<HashSet<String>>>,
    pending: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl AutoTagger {
    pub fn new(
        app: tauri::AppHandle,
        store: Arc<Store>,
        artists: Arc<ArtistService>,
        connections: Arc<crate::connections::ConnectionManager>,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded_channel::<String>();
        let manager = Arc::new(Self {
            store,
            artists,
            connections,
            app,
            sender,
            workers: Arc::new(Semaphore::new(2)),
            fingerprint_gate: Arc::new(Mutex::new(())),
            fingerprint_cpu: Arc::new(Semaphore::new(1)),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            pending: Arc::new(std::sync::Mutex::new(HashSet::new())),
        });
        manager.start_workers(receiver);
        manager
    }

    fn start_workers(self: &Arc<Self>, receiver: mpsc::UnboundedReceiver<String>) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut receiver = receiver;
            while let Some(asset_id) = receiver.recv().await {
                let worker = manager.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = worker.process_asset(&asset_id).await {
                        eprintln!("Autotag job for {asset_id} failed: {error}");
                        if let Ok(Some(job)) = worker.records().map(|jobs| {
                            jobs.into_iter()
                                .find(|job| job.asset_id == asset_id && job.status == "retry")
                        }) {
                            if let Ok(at) = chrono::DateTime::parse_from_rfc3339(
                                job.next_attempt_at.as_deref().unwrap_or(""),
                            ) {
                                let delay = (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
                                    .to_std()
                                    .unwrap_or_default();
                                let delayed = worker.clone();
                                tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(delay).await;
                                    delayed.schedule(&asset_id);
                                });
                            }
                        }
                    }
                });
            }
        });
        // Resume queued and due retry records after app startup.
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(jobs) = manager.records() {
                for job in jobs.into_iter().filter(|job| {
                    job.status == "queued" || job.status == "retry" || job.status == "matching"
                }) {
                    if job.status == "retry" {
                        if manager.retry_due(&job).await {
                            manager.schedule(&job.asset_id);
                        } else if let Ok(at) = chrono::DateTime::parse_from_rfc3339(
                            job.next_attempt_at.as_deref().unwrap_or(""),
                        ) {
                            let delay = (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
                                .to_std()
                                .unwrap_or_default();
                            let delayed = manager.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(delay).await;
                                delayed.schedule(&job.asset_id);
                            });
                        }
                    } else {
                        manager.schedule(&job.asset_id);
                    }
                }
            }
        });
    }

    fn schedule(&self, asset_id: &str) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !pending.insert(asset_id.to_owned()) {
            return false;
        }
        if self.sender.send(asset_id.to_owned()).is_err() {
            pending.remove(asset_id);
            return false;
        }
        true
    }

    pub fn queue_new_download(&self, asset_id: &str) -> Result<bool, String> {
        let asset = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .ok_or("Downloaded asset is not yet registered")?;
        let provenance = &asset.track.provenance;
        if asset.processing == "external-readonly"
            || !(provenance["publicationJobId"].is_string()
                || provenance["acquisition"].is_object())
        {
            return Ok(false);
        }
        if let Some(existing) = self.records()?.into_iter().find(|job| {
            job.asset_id == asset.id && job.input_hash == asset.hash && job.version == VERSION
        }) {
            if existing.status != "retry" && existing.status != "queued" {
                return Ok(false);
            }
        } else {
            self.enqueue_new_download(&asset)?;
        }
        if !self.schedule(asset_id) {
            return Ok(false);
        }
        if let Some(job) = self
            .records()?
            .into_iter()
            .find(|job| job.asset_id == asset_id && job.input_hash == asset.hash)
        {
            let _ = self.app.emit("autotag-status", &job);
        }
        Ok(true)
    }

    pub fn settings(&self) -> Result<AutoTagSettings, String> {
        let saved = self
            .store
            .get_json_preference(SETTINGS_KEY)
            .map_err(|e| e.to_string())?
            .unwrap_or(Value::Null);
        Ok(AutoTagSettings {
            enabled: saved["enabled"].as_bool().unwrap_or(true),
            shazam_fallback: saved["shazamFallback"].as_bool().unwrap_or(true),
            fpcalc_available: executable_available("fpcalc"),
            songrec_available: executable_available("songrec") || executable_available("SongRec"),
            acoustid_configured: self
                .connections
                .settings()
                .ok()
                .is_some_and(|settings| settings.acoustid_enabled)
                && self
                    .connections
                    .credential("acoustid")
                    .ok()
                    .flatten()
                    .is_some_and(|key| !key.trim().is_empty()),
        })
    }

    pub fn save_settings(
        &self,
        enabled: bool,
        shazam_fallback: bool,
    ) -> Result<AutoTagSettings, String> {
        self.store
            .set_json_preference(
                SETTINGS_KEY,
                &json!({"enabled":enabled,"shazamFallback":shazam_fallback}),
            )
            .map_err(|e| e.to_string())?;
        if enabled {
            for mut job in self.records()? {
                if job.status == "paused" {
                    job.status = "queued".into();
                    job.updated_at = chrono::Utc::now().to_rfc3339();
                    self.persist(&job)?;
                    self.schedule(&job.asset_id);
                }
            }
        }
        Ok(self.settings()?)
    }

    pub fn retry(&self, asset_id: &str) -> Result<bool, String> {
        let (_, job) = current_asset_retry_job(&self.store, asset_id)?;
        reserve_retry_slot(&self.pending, &self.inflight, asset_id)?;
        if let Err(error) = self.persist(&job) {
            release_retry_slot(&self.pending, asset_id);
            return Err(error);
        }
        if self.sender.send(asset_id.to_owned()).is_err() {
            release_retry_slot(&self.pending, asset_id);
            return Err("Autotag worker is not running".into());
        }
        let _ = self.app.emit("autotag-status", &job);
        Ok(true)
    }

    async fn process_asset(&self, asset_id: &str) -> Result<(), String> {
        {
            let mut inflight = self.inflight.lock().await;
            if !inflight.insert(asset_id.to_owned()) {
                return Ok(());
            }
        }
        let result = self.process_asset_inner(asset_id).await;
        self.inflight.lock().await.remove(asset_id);
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(asset_id);
        result
    }

    async fn process_asset_inner(&self, asset_id: &str) -> Result<(), String> {
        let asset = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .ok_or("Autotag asset was removed")?;
        let mut job = self
            .records()?
            .into_iter()
            .find(|j| j.asset_id == asset.id && j.input_hash == asset.hash && j.version == VERSION)
            .ok_or("Autotag job record is missing")?;
        if !["queued", "retry", "matching"].contains(&job.status.as_str()) {
            return Ok(());
        }
        if job.status == "retry" && !self.retry_due(&job).await {
            return Ok(());
        }
        let enabled = self.settings()?.enabled;
        if !enabled {
            job.status = "paused".into();
            job.updated_at = chrono::Utc::now().to_rfc3339();
            self.persist(&job)?;
            let _ = self.app.emit("autotag-status", &job);
            return Ok(());
        }
        let connection_settings = self.connections.settings().unwrap_or_default();
        let credentials = crate::artists::ArtistCredentials {
            discogs_token: if connection_settings.discogs_enabled {
                self.connections.credential("discogs").ok().flatten()
            } else {
                None
            },
            lastfm_api_key: self.connections.credential("lastfm").ok().flatten(),
            warnings: vec![],
        };
        let result = self.run_job(&mut job, &asset, &credentials).await;
        let _ = self.app.emit("autotag-status", &job);
        result
    }

    /// Persist a job for a just-downloaded managed asset. Callers must not use
    /// this for pre-existing library scans or external-readonly assets.
    pub fn enqueue_new_download(&self, asset: &LocalAsset) -> Result<AutoTagJob, String> {
        if asset.processing == "external-readonly" {
            return Err("Autotag queue accepts newly downloaded managed assets only".into());
        }
        let job = AutoTagJob {
            asset_id: asset.id.clone(),
            input_hash: asset.hash.clone(),
            version: VERSION.into(),
            status: "queued".into(),
            attempts: 0,
            next_attempt_at: None,
            message: None,
            candidates: vec![],
            chosen: None,
            recognition: None,
            approved_fields: vec![],
            excerpt_only: false,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        self.persist(&job)?;
        Ok(job)
    }

    pub fn records(&self) -> Result<Vec<AutoTagJob>, String> {
        let rows = self
            .store
            .list_autotag_records()
            .map_err(|e| e.to_string())?;
        Ok(decode_job_records(rows))
    }

    fn persist(&self, job: &AutoTagJob) -> Result<(), String> {
        self.store
            .save_autotag_record(
                &job.asset_id,
                &job.input_hash,
                VERSION,
                &job.status,
                &serde_json::to_value(job).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
    }

    /// Run one persisted job. Provider outages become retryable errors and do
    /// not alter the source file or invalidate the completed download.
    pub async fn run_job(
        &self,
        job: &mut AutoTagJob,
        asset: &LocalAsset,
        credentials: &crate::artists::ArtistCredentials,
    ) -> Result<(), String> {
        if job.asset_id != asset.id || job.input_hash != asset.hash || job.version != VERSION {
            return Err("Autotag job no longer matches the asset content hash".into());
        }
        let _permit = self.workers.acquire().await.map_err(|e| e.to_string())?;
        job.status = "matching".into();
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.persist(job)?;
        if is_live_performance(&asset.track) {
            job.status = "review".into();
            job.message = Some("Explicit live-performance identity requires review; studio fingerprint metadata was not applied.".into());
            job.updated_at = chrono::Utc::now().to_rfc3339();
            return self.persist(job);
        }
        let mut working_asset = asset.clone();
        if let Some(repaired) = self
            .repair_acquisition_catalog_metadata(job, &working_asset)
            .await?
        {
            working_asset = repaired;
        }
        if let Some(repaired) = self
            .repair_source_reported_metadata(job, &working_asset)
            .await?
        {
            working_asset = repaired;
        }
        let excerpt_only = is_excerpt(&working_asset.track);
        job.excerpt_only = excerpt_only;
        let acoustid_enabled = self
            .connections
            .settings()
            .map(|settings| settings.acoustid_enabled)
            .unwrap_or(false);
        let acoustid_key = if acoustid_enabled {
            self.connections.credential("acoustid").ok().flatten()
        } else {
            None
        };
        match self
            .lookup(
                &working_asset.track,
                &working_asset.path,
                acoustid_key.as_deref(),
                credentials,
            )
            .await
        {
            Ok(lookup) => {
                let candidates = lookup.candidates;
                job.recognition = lookup.recognition;
                job.candidates = candidates.clone();
                if candidates.len() == 1
                    && can_auto_apply(&working_asset.track, &candidates[0], excerpt_only)
                {
                    job.chosen = Some(candidates[0].clone());
                    job.status = "ready".into();
                    job.approved_fields = strong_field_keys(&candidates[0].fields);
                    let chosen = candidates[0].clone();
                    let latest = self
                        .store
                        .get_asset_by_id(&working_asset.id)
                        .map_err(|e| e.to_string())?
                        .ok_or("Autotag asset was removed")?;
                    if latest.hash != working_asset.hash {
                        job.input_hash = latest.hash;
                        job.status = "stale".into();
                        job.message = Some("Audio content changed while matching; run a new download or retry from the current asset.".into());
                        job.updated_at = chrono::Utc::now().to_rfc3339();
                        return self.persist(job);
                    }
                    let updated = self
                        .apply(
                            job,
                            &latest,
                            &chosen,
                            &self.store.settings().map_err(|e| e.to_string())?,
                        )
                        .await?;
                    job.input_hash = updated.hash.clone();
                    job.status = "applied".into();
                    job.updated_at = chrono::Utc::now().to_rfc3339();
                    self.store
                        .save_autotagged_asset(
                            &updated,
                            &working_asset.hash,
                            VERSION,
                            &serde_json::to_value(&*job).map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?;
                    if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
                        media::acknowledge_retag_journal(
                            &self.store.settings().map_err(|e| e.to_string())?,
                            id,
                        )?;
                    }
                } else {
                    let (status, message) = classify_lookup_result(
                        &working_asset.track,
                        candidates.len(),
                        job.recognition.is_some(),
                    );
                    job.status = status.into();
                    job.message = Some(message.into());
                }
                job.updated_at = chrono::Utc::now().to_rfc3339();
                self.persist(job)
            }
            Err(error) => {
                mark_retry(job, &error);
                self.persist(job)?;
                Err(error)
            }
        }
    }

    /// Backfill only missing optional tags from the exact catalog item used to
    /// acquire this managed asset. Search metadata is not trusted until both
    /// the stored locator and shared resolved recording policy agree.
    async fn repair_acquisition_catalog_metadata(
        &self,
        job: &mut AutoTagJob,
        asset: &LocalAsset,
    ) -> Result<Option<LocalAsset>, String> {
        if asset.processing == "external-readonly" || is_excerpt(&asset.track) {
            return Ok(None);
        }
        let Some(locator) = monochrome_acquisition_locator(&asset.track) else {
            return Ok(None);
        };
        let Some(requested) = requested_identity_for_catalog_backfill(&asset.track) else {
            return Ok(None);
        };
        let query = format!("{} {}", requested.artist.trim(), requested.title.trim());
        if requested.artist.trim().is_empty() || requested.title.trim().is_empty() {
            return Ok(None);
        }
        let client = match crate::monochrome::Client::new() {
            Ok(client) => client,
            Err(_) => return Ok(None),
        };
        let rows = match tokio::time::timeout(
            Duration::from_secs(12),
            client.search_with_limit(&query, 12),
        )
        .await
        {
            Ok(Ok((rows, _))) => rows,
            // A narrow metadata backfill must never make the normal autotag job
            // fail; retain its existing lookup/recognition fallback when offline.
            _ => return Ok(None),
        };
        let candidates = rows
            .into_iter()
            .map(|(_, candidate)| candidate)
            .filter(|candidate| exact_monochrome_locator_match(&locator, candidate))
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Ok(None);
        }
        let candidate = &candidates[0];
        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let assessment = crate::recording_match_context::assess(
            &self.store,
            &requested,
            candidate,
            settings.matching_level,
            crate::recording_match::MatchStage::Resolved,
        );
        let Some(metadata_track) =
            accepted_acquisition_catalog_metadata(&asset.track, &locator, candidate, &assessment)
        else {
            return Ok(None);
        };
        let fields = metadata_track.provenance["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let Some(current) = self
            .store
            .get_asset_by_id(&asset.id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        if current.hash != asset.hash
            || current.path != asset.path
            || current.recording_id != asset.recording_id
        {
            return Ok(None);
        }
        let updated = media::autotag_asset(asset, &metadata_track, &settings).await?;
        job.input_hash = updated.hash.clone();
        job.status = "matching".into();
        job.message = Some(format!(
            "Filled missing {} from the exact Monochrome acquisition source; requested recording identity was verified by the shared match policy.",
            fields.join(", ")
        ));
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.store
            .save_autotagged_asset(
                &updated,
                &asset.hash,
                VERSION,
                &serde_json::to_value(&*job).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        self.persist(job)?;
        if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
            media::acknowledge_retag_journal(&settings, id)?;
        }
        Ok(Some(updated))
    }

    async fn repair_source_reported_metadata(
        &self,
        job: &mut AutoTagJob,
        asset: &LocalAsset,
    ) -> Result<Option<LocalAsset>, String> {
        if asset.processing == "external-readonly" {
            return Ok(None);
        }
        let Some(candidate) = asset
            .track
            .provenance
            .get("audioCandidate")
            .cloned()
            .and_then(|value| serde_json::from_value::<Candidate>(value).ok())
        else {
            return Ok(None);
        };
        let (reported, fields) =
            crate::acquisition::source_reported_track_for_lookup_from_candidate(
                &asset.track,
                &candidate,
            );
        self.apply_source_reported_metadata(
            job,
            asset,
            reported,
            fields,
            "matching",
            "Filled missing fields from the exact source's reported metadata; recording identity remains unverified.",
            false,
        )
        .await
    }

    async fn apply_source_reported_metadata(
        &self,
        job: &mut AutoTagJob,
        asset: &LocalAsset,
        reported: Track,
        fields: Vec<String>,
        status: &str,
        message: &str,
        refresh_directory_fingerprint: bool,
    ) -> Result<Option<LocalAsset>, String> {
        let fields = fields
            .into_iter()
            .filter(|field| source_field_needs_repair(&asset.track, &reported, field))
            .collect::<Vec<_>>();
        if fields.is_empty() {
            return Ok(None);
        }

        let mut metadata = reported.clone();
        if !fields.iter().any(|field| field == "artist") {
            metadata.artist.clear();
        }
        if !fields.iter().any(|field| field == "title") {
            metadata.title.clear();
        }
        if !fields.iter().any(|field| field == "album") {
            metadata.album = None;
        }
        if !fields.iter().any(|field| field == "albumArtist") {
            metadata.album_artist = None;
        }
        if !fields.iter().any(|field| field == "genre") {
            metadata.genre = None;
        }
        if !fields.iter().any(|field| field == "isrc") {
            metadata.isrc = None;
        }
        metadata.provenance = json!({
            "source":"exact_source_reported_metadata",
            "identityStatus":"source_reported_unverified",
            "sourceReportedMetadata":reported.provenance["sourceReportedMetadata"],
            "sourceReportedFields":fields,
            "version":VERSION
        });
        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let mut updated = media::autotag_asset(asset, &metadata, &settings).await?;
        if refresh_directory_fingerprint {
            // Retagging can change the file's modification time while leaving
            // its audio bytes/hash untouched. Refresh the cheap directory-
            // browser stat stamp before persisting the asset so folder
            // browsing does not treat this just-retagged row as stale.
            let _ = crate::directory_browser::stamp_registered_asset_fingerprint(
                &self.store,
                &mut updated,
            );
        }
        job.input_hash = updated.hash.clone();
        job.status = status.into();
        job.message = Some(message.into());
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.store
            .save_autotagged_asset(
                &updated,
                &asset.hash,
                VERSION,
                &serde_json::to_value(&*job).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        self.persist(job)?;
        if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
            media::acknowledge_retag_journal(&settings, id)?;
        }
        Ok(Some(updated))
    }

    /// Fill missing tags from a completed managed download's exact source
    /// metadata. This local-only path never performs catalog lookup or audio
    /// recognition, and leaves the recording identity unverified.
    pub async fn repair_download_source_tags(
        &self,
        asset_id: &str,
    ) -> Result<DownloadSourceTagRepairResult, String> {
        let asset_id = asset_id.trim();
        if asset_id.is_empty() || asset_id.len() > 300 {
            return Err("Asset ID must be between 1 and 300 characters".into());
        }
        reserve_retry_slot(&self.pending, &self.inflight, asset_id)?;
        let inserted = self.inflight.lock().await.insert(asset_id.to_owned());
        if !inserted {
            release_retry_slot(&self.pending, asset_id);
            return Err("Autotag job is currently running for this asset".into());
        }
        let result = self.repair_download_source_tags_inner(asset_id).await;
        self.inflight.lock().await.remove(asset_id);
        release_retry_slot(&self.pending, asset_id);
        result
    }

    async fn repair_download_source_tags_inner(
        &self,
        asset_id: &str,
    ) -> Result<DownloadSourceTagRepairResult, String> {
        let asset = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|error| error.to_string())?
            .ok_or("Autotag asset was removed")?;
        if asset.processing == "external-readonly" {
            return Ok(source_tag_repair_noop(&asset, "not_applicable"));
        }
        let Some(publication_job_id) = asset.track.provenance["publicationJobId"].as_str() else {
            return Ok(source_tag_repair_noop(
                &asset,
                "source_metadata_unavailable",
            ));
        };
        let Some(download_job) = self
            .store
            .get_job_if_exists(publication_job_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(source_tag_repair_noop(
                &asset,
                "source_metadata_unavailable",
            ));
        };
        let Some(candidate) = completed_source_candidate(&asset, &download_job) else {
            return Ok(source_tag_repair_noop(&asset, "source_binding_unverified"));
        };
        let (reported, fields) =
            crate::acquisition::source_reported_track_for_lookup_from_candidate(
                &asset.track,
                candidate,
            );
        let fields = fields
            .into_iter()
            .filter(|field| source_field_needs_repair(&asset.track, &reported, field))
            .collect::<Vec<_>>();
        if fields.is_empty() {
            let status = if has_bound_yt_dlp_metadata(&asset.track, candidate) {
                "source_reported_unverified"
            } else {
                "source_metadata_unavailable"
            };
            return Ok(source_tag_repair_noop(&asset, status));
        }

        let mut job = self
            .store
            .get_autotag_record(&asset.id, &asset.hash, VERSION)
            .map_err(|error| error.to_string())?
            .and_then(|value| serde_json::from_value::<AutoTagJob>(value).ok())
            .unwrap_or_else(|| new_source_tag_repair_job(&asset));
        job.input_hash = asset.hash.clone();
        job.status = "review".into();
        job.updated_at = chrono::Utc::now().to_rfc3339();
        let updated = self
            .apply_source_reported_metadata(
                &mut job,
                &asset,
                reported,
                fields.clone(),
                "review",
                "Filled missing tags from the exact completed download source. Catalog identity remains unverified; no recognition or catalog lookup was run.",
                true,
            )
            .await?;
        if updated.is_some() {
            Ok(DownloadSourceTagRepairResult {
                asset_id: asset.id,
                changed: true,
                fields,
                identity_status: "source_reported_unverified".into(),
            })
        } else {
            Ok(source_tag_repair_noop(&asset, "source_reported_unverified"))
        }
    }

    async fn lookup(
        &self,
        source: &Track,
        path: &str,
        acoustid_key: Option<&str>,
        credentials: &crate::artists::ArtistCredentials,
    ) -> Result<AutoTagLookup, String> {
        // Use exact-source metadata to form a catalog query when downloaded
        // track fields are incomplete. Auto-apply is still checked against the
        // original track, so this lookup alone cannot verify its identity.
        let (search_source, source_reported_fields) =
            crate::acquisition::source_reported_track_for_lookup(source);
        let source_reported_identity = source_reported_fields
            .iter()
            .any(|field| matches!(field.as_str(), "artist" | "title"));
        let excerpt_only = is_excerpt(source);
        // The exact acquired catalog locator is already the source identity;
        // do not re-recognize the same asset with AcoustID/SongRec.
        let allow_fingerprinting = !accepted_acquisition_isrc_backfill(source);
        let mut recognition = None;
        if excerpt_only {
            let mut candidates = Vec::new();
            if allow_fingerprinting {
                if let Some(key) = acoustid_key.filter(|key| !key.trim().is_empty()) {
                    if let Some(candidate) = self.acoustid_match(source, path, key).await? {
                        candidates.push(candidate);
                    }
                }
                if candidates.is_empty() && self.settings()?.shazam_fallback {
                    if let Some(outcome) = self.songrec_match(source, path).await? {
                        recognition = Some(outcome.recognition);
                        if let Some(candidate) = outcome.candidate {
                            candidates.push(candidate);
                        }
                    }
                }
            }
            for candidate in &mut candidates {
                restrict_excerpt_fields(candidate);
                add_source_reported_lookup_evidence(candidate, &source_reported_fields);
            }
            return Ok(AutoTagLookup {
                candidates,
                recognition,
            });
        }
        let mut catalog_errors = Vec::new();
        let results = catalog_result_or_empty(
            self.artists.recording_search(&search_source).await,
            &mut catalog_errors,
        );
        let rows = results["recordings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut matches = rows
            .iter()
            .filter_map(|row| candidate_from_mb(row, &search_source))
            .collect::<Vec<_>>();
        matches.sort_by(|a, b| b.score.total_cmp(&a.score));
        matches.truncate(10);
        if search_source
            .album
            .as_deref()
            .is_some_and(|album| !album.trim().is_empty())
        {
            for candidate in matches.iter_mut().take(3) {
                if let Err(error) = self.enrich_release_context(candidate, &search_source).await {
                    catalog_errors.push(error);
                    mark_release_context_unavailable(candidate);
                }
            }
        }
        if matches.is_empty() {
            matches.extend(catalog_result_or_empty(
                self.discogs_match(&search_source, credentials).await,
                &mut catalog_errors,
            ));
        }
        if matches.is_empty() && !source_reported_identity && allow_fingerprinting {
            if let Some(key) = acoustid_key.filter(|key| !key.trim().is_empty()) {
                if let Some(candidate) = self.acoustid_match(source, path, key).await? {
                    matches.push(candidate);
                }
            }
            if matches.is_empty() && self.settings()?.shazam_fallback {
                if let Some(outcome) = self.songrec_match(source, path).await? {
                    recognition = Some(outcome.recognition);
                    if let Some(candidate) = outcome.candidate {
                        matches.push(candidate);
                    }
                }
            }
        }
        // Source-reported names can seed catalog search, but a catalog hit
        // based on those same names is not independent identity evidence.
        // Also try the configured audio recognizers so a high-confidence
        // fingerprint/recognition result can repair incomplete downloaded tags.
        if source_reported_identity && allow_fingerprinting {
            let mut recognized = Vec::new();
            if let Some(key) = acoustid_key.filter(|key| !key.trim().is_empty()) {
                if let Some(candidate) = self.acoustid_match(source, path, key).await? {
                    recognized.push(candidate);
                }
            }
            if recognized.is_empty() && self.settings()?.shazam_fallback {
                if let Some(outcome) = self.songrec_match(source, path).await? {
                    recognition = Some(outcome.recognition);
                    if let Some(candidate) = outcome.candidate {
                        recognized.push(candidate);
                    }
                }
            }
            if !recognized.is_empty() {
                // The recognizer has already required a unique high-confidence
                // catalog identity. Keep that independent result as the lead
                // candidate instead of letting hint-only search rows make the
                // job ambiguous or block automatic repair.
                matches = recognized;
            }
        }
        if search_source
            .album
            .as_deref()
            .is_some_and(|album| !album.trim().is_empty())
        {
            for candidate in matches.iter_mut().take(3) {
                if candidate.fields.provenance["releaseContextAttempted"] != true {
                    if let Err(error) = self.enrich_release_context(candidate, &search_source).await
                    {
                        catalog_errors.push(error);
                        mark_release_context_unavailable(candidate);
                    }
                }
            }
        }
        for candidate in &mut matches {
            add_source_reported_lookup_evidence(candidate, &source_reported_fields);
        }
        finish_autotag_lookup(matches, recognition, catalog_errors)
    }

    async fn enrich_release_context(
        &self,
        candidate: &mut AutoTagCandidate,
        source: &Track,
    ) -> Result<(), String> {
        candidate.fields.provenance["releaseContextAttempted"] = json!(true);
        let recording = self
            .artists
            .json_for_metadata(
                "musicbrainz",
                &format!(
                    "recording/{}?inc=artist-credits+isrcs+genres+releases",
                    candidate.provider_id
                ),
                "autotag.recording-details",
                &candidate.provider_id,
                None,
            )
            .await?;
        let isrcs = recording["isrcs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if isrcs.len() == 1 {
            candidate.fields.isrc = Some(isrcs[0].to_owned());
        }
        candidate.fields.genre = recording["genres"]
            .as_array()
            .and_then(|genres| genres.first())
            .and_then(|genre| genre["name"].as_str())
            .map(str::to_owned);
        let Some(album) = source
            .album
            .as_deref()
            .filter(|album| !album.trim().is_empty())
        else {
            return Ok(());
        };
        let releases = recording["releases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|release| {
                release["title"]
                    .as_str()
                    .is_some_and(|title| normalize(title) == normalize(album))
                    && render_artist_credit(&release["artist-credit"])
                        .is_some_and(|artist| normalize(&artist) == normalize(&candidate.artist))
            })
            .collect::<Vec<_>>();
        if releases.len() != 1 {
            return Ok(());
        }
        let release_id = releases[0]["id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or("MusicBrainz release omitted its identity")?;
        let release = self
            .artists
            .json_for_metadata(
                "musicbrainz",
                &format!(
                    "release/{release_id}?inc=artist-credits+labels+recordings+media+isrcs+genres"
                ),
                "autotag.release-details",
                release_id,
                None,
            )
            .await?;
        let candidate_id = candidate.provider_id.as_str();
        let matching = release["media"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|medium| {
                medium["tracks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(move |track| {
                        (track["recording"]["id"].as_str() == Some(candidate_id))
                            .then_some((medium, track))
                    })
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Ok(());
        }
        let (medium, track) = matching[0];
        candidate.fields.album = release["title"].as_str().map(str::to_owned);
        candidate.fields.album_artist = render_artist_credit(&release["artist-credit"]);
        candidate.fields.release_date = release["date"].as_str().map(str::to_owned);
        candidate.fields.label = release["label-info"]
            .as_array()
            .and_then(|labels| labels.first())
            .and_then(|entry| entry["label"]["name"].as_str())
            .map(str::to_owned);
        candidate.fields.catalog_number = release["label-info"]
            .as_array()
            .and_then(|labels| labels.first())
            .and_then(|entry| entry["catalog-number"].as_str())
            .map(str::to_owned);
        candidate.fields.track_number = track["position"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok());
        candidate.fields.track_total = medium["tracks"]
            .as_array()
            .and_then(|v| u32::try_from(v.len()).ok());
        candidate.fields.disc_number = medium["position"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok());
        candidate.fields.disc_total = release["media"]
            .as_array()
            .and_then(|v| u32::try_from(v.len()).ok());
        candidate.fields.provenance["release"] = json!({"provider":"musicbrainz","providerId":release_id,"sourceUrl":format!("https://musicbrainz.org/release/{release_id}")});
        candidate.evidence.push("Unique exact artist and album release; its tracklist contains the matched recording identity.".into());
        Ok(())
    }

    async fn discogs_match(
        &self,
        source: &Track,
        credentials: &crate::artists::ArtistCredentials,
    ) -> Result<Vec<AutoTagCandidate>, String> {
        let Some(album) = source
            .album
            .as_deref()
            .filter(|album| !album.trim().is_empty())
        else {
            return Ok(vec![]);
        };
        let Some(token) = credentials
            .discogs_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        else {
            return Ok(vec![]);
        };
        let page = self.artists.search_albums(album, 1, credentials).await?;
        let mut candidates = Vec::new();
        for release in page
            .items
            .into_iter()
            .filter(|release| {
                release.release.provider == "discogs"
                    && normalize(&release.title) == normalize(album)
            })
            .take(3)
        {
            let master = release.source_url.contains("/master/");
            let route = if master {
                format!("masters/{}", release.release.id)
            } else {
                format!("releases/{}", release.release.id)
            };
            let detail = self
                .artists
                .json_for_metadata(
                    "discogs",
                    &route,
                    "autotag.discogs.release",
                    &release.release.id,
                    Some(token),
                )
                .await?;
            let artist_id = detail["artists"]
                .as_array()
                .and_then(|artists| artists.first())
                .and_then(|artist| artist["id"].as_i64())
                .map(|id| id.to_string());
            let Some(artist_id) = artist_id else { continue };
            let selected_artist = crate::artists::ArtistRef {
                provider: "discogs".into(),
                id: artist_id,
            };
            let tracks = self
                .artists
                .get_release_tracks(&release, &selected_artist, credentials)
                .await?;
            let album_artist = detail["artists"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|artist| artist["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let track_total = u32::try_from(tracks.len()).ok();
            let disc_total = tracks
                .iter()
                .filter_map(|track| track.provenance["mediumPosition"].as_u64())
                .max()
                .and_then(|v| u32::try_from(v).ok());
            for mut track in tracks {
                let title_score = jaro_winkler(&normalize(&track.title), &normalize(&source.title));
                let artist_score =
                    jaro_winkler(&normalize(&track.artist), &normalize(&source.artist));
                if title_score < 0.94
                    || artist_score < 0.94
                    || !compatible_version(
                        &source.title,
                        source.version.as_deref(),
                        &track.title,
                        track.version.as_deref(),
                    )
                {
                    continue;
                }
                if let (Some(track_duration), Some(source_duration)) =
                    (track.duration_seconds, source.duration_seconds)
                {
                    let delta = (track_duration - source_duration).abs();
                    if delta > MAX_DURATION_DELTA
                        && delta / source_duration.max(1.0) > MAX_DURATION_RATIO
                    {
                        continue;
                    }
                }
                let medium_position = track.provenance["mediumPosition"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok());
                let track_position = track.provenance["trackPosition"].as_str().unwrap_or("?");
                let key = format!(
                    "{}:{}:{}",
                    release.release.id,
                    medium_position.unwrap_or_default(),
                    track_position
                );
                track.album_artist = (!album_artist.is_empty()).then(|| album_artist.clone());
                track.track_number = track_position.parse::<u32>().ok();
                track.track_total = track_total;
                track.disc_number = medium_position;
                track.disc_total = disc_total;
                let score = title_score * 0.55
                    + artist_score * 0.4
                    + if track.duration_seconds.is_some() {
                        0.05
                    } else {
                        0.0
                    };
                candidates.push(AutoTagCandidate {
                    provider: "discogs".into(),
                    provider_id: key,
                    source_url: release.source_url.clone(),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    duration_seconds: track.duration_seconds,
                    score,
                    fields: track,
                    evidence: vec![
                        format!("exact Discogs album release {}", release.title),
                        format!("artist similarity {artist_score:.3}"),
                        format!("title similarity {title_score:.3}"),
                        "release tracklist identity and duration are compatible".into(),
                    ],
                });
            }
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(candidates)
    }

    async fn acoustid_match(
        &self,
        source: &Track,
        path: &str,
        key: &str,
    ) -> Result<Option<AutoTagCandidate>, String> {
        let _cpu_permit = self
            .fingerprint_cpu
            .acquire()
            .await
            .map_err(|e| e.to_string())?;
        let mut command =
            tokio::process::Command::new(crate::tools::executable("fpcalc", "fpcalc"));
        crate::tools::configure_no_console_tokio(&mut command);
        command
            .args(["-json", "-length", "120", path])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
            Ok(Ok(output)) if output.status.success() => output,
            Ok(Ok(_)) => return Ok(None),
            Ok(Err(_)) | Err(_) => return Ok(None),
        };
        let fingerprint: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("fpcalc returned invalid output: {e}"))?;
        let Some(fp) = fingerprint["fingerprint"].as_str() else {
            return Ok(None);
        };
        let Some(duration) = fingerprint["duration"].as_f64() else {
            return Ok(None);
        };
        drop(_cpu_permit);
        let _guard = self.fingerprint_gate.lock().await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("Burrata/0.1.0")
            .build()
            .map_err(|e| e.to_string())?;
        let duration_text = duration.to_string();
        let response: Value = client
            .post("https://api.acoustid.org/v2/lookup")
            .form(&[
                ("client", key),
                ("meta", "recordings"),
                ("duration", duration_text.as_str()),
                ("fingerprint", fp),
            ])
            .send()
            .await
            .map_err(|e| format!("AcoustID lookup failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("AcoustID lookup failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("AcoustID returned invalid data: {e}"))?;
        if response["status"] != "ok" {
            return Ok(None);
        }
        let mut ids = Vec::new();
        for result in response["results"].as_array().into_iter().flatten() {
            let score = result["score"].as_f64().unwrap_or(0.0);
            if score < 0.95 {
                continue;
            }
            for recording in result["recordings"].as_array().into_iter().flatten() {
                if let Some(id) = recording["id"].as_str() {
                    ids.push((id.to_owned(), score));
                }
            }
        }
        ids.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut seen_ids = HashSet::new();
        ids.retain(|(id, _)| seen_ids.insert(id.clone()));
        if ids.len() != 1 {
            return Ok(None);
        }
        let (id, audio_score) = &ids[0];
        let mb = self
            .artists
            .json_for_metadata(
                "musicbrainz",
                &format!("recording/{id}?inc=artist-credits+isrcs+genres"),
                "autotag.acoustid.recording",
                id,
                None,
            )
            .await?;
        let Some(mut candidate) =
            candidate_from_mb(&mb, source).or_else(|| candidate_from_fingerprint_mb(&mb))
        else {
            return Ok(None);
        };
        candidate.score = *audio_score;
        candidate
            .evidence
            .push(format!("AcoustID audio fingerprint score {audio_score:.3}"));
        if source_conflicts(source, &candidate) {
            candidate.evidence.push(
                "Recognized artist or title conflicts with the downloaded source; review required."
                    .into(),
            );
        }
        Ok(Some(candidate))
    }

    async fn recognize_file_without_catalog(
        &self,
        source: &Track,
        path: &str,
    ) -> Result<Option<AudioRecognition>, String> {
        if !executable_available("songrec") && !executable_available("SongRec") {
            return Ok(None);
        }
        let duration = source.duration_seconds.unwrap_or(0.0);
        if !duration.is_finite() || duration < 50.0 {
            return Ok(None);
        }
        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let asset = self
            .store
            .get_asset_by_path(path)
            .map_err(|error| error.to_string())?
            .ok_or("Recognition input is no longer a registered library file")?;
        let binding = crate::recognition_input::RecognitionInputBinding::capture(
            source,
            source,
            std::path::Path::new(path),
            settings.matching_level,
            Some(&asset.hash),
        )
        .await?;
        let first = songrec_excerpt(path, 5.0, &settings.ffmpeg_path).await?;
        if first.is_none() {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
        // Refuse another submission if bytes or the matching policy changed
        // while waiting between excerpts.
        let current_level = self
            .store
            .settings()
            .map_err(|error| error.to_string())?
            .matching_level;
        binding
            .recheck(source, source, std::path::Path::new(path), current_level)
            .await?;
        let second = songrec_excerpt(path, duration - 25.0, &settings.ffmpeg_path).await?;
        let Some(recognition) = consistent_songrec_excerpts(first, second) else {
            return Ok(None);
        };
        binding
            .recheck(
                source,
                source,
                std::path::Path::new(path),
                self.store
                    .settings()
                    .map_err(|error| error.to_string())?
                    .matching_level,
            )
            .await?;
        Ok(Some(recognition))
    }

    async fn songrec_match(
        &self,
        source: &Track,
        path: &str,
    ) -> Result<Option<SongRecOutcome>, String> {
        let Some(recognition) = self.recognize_file_without_catalog(source, path).await? else {
            return Ok(None);
        };
        let recognized_isrc = recognition.isrc.as_deref();
        let identified = Track {
            artist: recognition.artist.clone(),
            title: recognition.title.clone(),
            duration_seconds: source.duration_seconds,
            isrc: recognition.isrc.clone(),
            ..Track::default()
        };
        let rows = if let Some(isrc) = recognized_isrc {
            let query =
                format!("recording?query=isrc%3A{isrc}&limit=100&inc=artist-credits+isrcs+genres");
            self.artists
                .json_for_metadata("musicbrainz", &query, "autotag.songrec.isrc", isrc, None)
                .await
        } else {
            self.artists.recording_search(&identified).await
        };
        Ok(Some(songrec_catalog_outcome(source, recognition, rows)))
    }

    pub async fn apply(
        &self,
        job: &mut AutoTagJob,
        asset: &LocalAsset,
        candidate: &AutoTagCandidate,
        settings: &Settings,
    ) -> Result<LocalAsset, String> {
        if !["musicbrainz", "discogs"].contains(&candidate.provider.as_str()) {
            return Err("Unsupported catalog provider".into());
        }
        let mut candidate = candidate.clone();
        if job.excerpt_only {
            restrict_excerpt_fields(&mut candidate);
        }
        if job.input_hash != asset.hash || job.asset_id != asset.id {
            return Err("Autotag job is stale".into());
        }
        let mut metadata = candidate.fields.clone();
        metadata.provenance = json!({"provider":candidate.provider,"providerId":candidate.provider_id,"sourceUrl":candidate.source_url,"release":candidate.fields.provenance["release"],"catalogTrackProvenance":candidate.fields.provenance,"evidence":candidate.evidence,"score":candidate.score,"version":VERSION,"userApprovedFields":if job.status == "approved" { json!(&job.approved_fields) } else { json!([]) }});
        let updated = media::autotag_asset(asset, &metadata, settings).await?;
        job.status = "applied".into();
        job.chosen = Some(candidate.clone());
        job.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(updated)
    }

    pub fn review(&self, asset_id: &str, selected_provider_id: &str) -> Result<AutoTagJob, String> {
        let current_hash = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .map(|asset| asset.hash)
            .ok_or("Autotag asset was removed")?;
        let mut job = self
            .records()?
            .into_iter()
            .find(|j| j.asset_id == asset_id && j.input_hash == current_hash)
            .ok_or("Autotag review job not found")?;
        let candidate = job
            .candidates
            .iter()
            .find(|c| c.provider_id == selected_provider_id)
            .cloned()
            .ok_or("Selected catalog candidate is not part of this review")?;
        job.chosen = Some(candidate);
        job.approved_fields.clear();
        job.status = "approved".into();
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.persist(&job)?;
        Ok(job)
    }

    pub async fn apply_review(
        &self,
        asset_id: &str,
        field_keys: &[String],
    ) -> Result<LocalAsset, String> {
        let _permit = self.workers.acquire().await.map_err(|e| e.to_string())?;
        let mut job = self
            .records()?
            .into_iter()
            .find(|j| j.asset_id == asset_id)
            .ok_or("Autotag review job not found")?;
        if job.status != "approved" {
            return Err("Review a candidate before applying metadata".into());
        }
        let candidate = job
            .chosen
            .clone()
            .ok_or("The selected candidate does not match this review")?;
        let mut approved = field_keys.iter().cloned().collect::<Vec<_>>();
        approved.sort();
        approved.dedup();
        if approved.is_empty()
            || approved
                .iter()
                .any(|key| !SUPPORTED_FIELDS.contains(&key.as_str()))
        {
            return Err("Choose one or more supported metadata fields".into());
        }
        if job.excerpt_only
            && approved
                .iter()
                .any(|key| !["artist", "title", "genre"].contains(&key.as_str()))
        {
            return Err("Chapter or excerpt review supports artist, title, and genre only".into());
        }
        if approved
            .iter()
            .any(|key| !candidate_provides_field(&candidate, key))
        {
            return Err(
                "The selected candidate has no value for one or more approved fields".into(),
            );
        }
        let asset = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .ok_or("Autotag asset was removed")?;
        let mut selected = candidate.clone();
        if !approved.iter().any(|key| key == "artist") {
            selected.fields.artist.clear();
        }
        if !approved.iter().any(|key| key == "title") {
            selected.fields.title.clear();
        }
        if !approved.iter().any(|key| key == "album") {
            selected.fields.album = None;
        }
        if !approved.iter().any(|key| key == "albumArtist") {
            selected.fields.album_artist = None;
        }
        if !approved.iter().any(|key| key == "releaseDate") {
            selected.fields.release_date = None;
        }
        if !approved.iter().any(|key| key == "isrc") {
            selected.fields.isrc = None;
        }
        if !approved.iter().any(|key| key == "genre") {
            selected.fields.genre = None;
        }
        if !approved.iter().any(|key| key == "label") {
            selected.fields.label = None;
        }
        if !approved.iter().any(|key| key == "catalogNumber") {
            selected.fields.catalog_number = None;
        }
        if !approved.iter().any(|key| key == "trackNumber") {
            selected.fields.track_number = None;
        }
        if !approved.iter().any(|key| key == "trackTotal") {
            selected.fields.track_total = None;
        }
        if !approved.iter().any(|key| key == "discNumber") {
            selected.fields.disc_number = None;
        }
        if !approved.iter().any(|key| key == "discTotal") {
            selected.fields.disc_total = None;
        }
        job.approved_fields = approved;
        let updated = self
            .apply(
                &mut job,
                &asset,
                &selected,
                &self.store.settings().map_err(|e| e.to_string())?,
            )
            .await?;
        job.input_hash = updated.hash.clone();
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.store
            .save_autotagged_asset(
                &updated,
                &asset.hash,
                VERSION,
                &serde_json::to_value(&job).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
            media::acknowledge_retag_journal(
                &self.store.settings().map_err(|e| e.to_string())?,
                id,
            )?;
        }
        self.persist(&job)?;
        let _ = self.app.emit("autotag-status", &job);
        Ok(updated)
    }

    /// Apply only the user-selected fields from a two-excerpt recognition
    /// result. Recognition never changes the library recording identity.
    pub async fn apply_recognition_review(
        &self,
        asset_id: &str,
        field_keys: &[String],
    ) -> Result<LocalAsset, String> {
        let _permit = self.workers.acquire().await.map_err(|e| e.to_string())?;
        let mut job = self
            .records()?
            .into_iter()
            .find(|job| job.asset_id == asset_id)
            .ok_or("Autotag review job not found")?;
        if job.status != "review" {
            return Err("Recognition can only be applied from an active review".into());
        }
        let recognition = job
            .recognition
            .as_ref()
            .ok_or("This review has no audio recognition result")?;
        if recognition.excerpt_count < 2 {
            return Err("Recognition must agree across two excerpts before applying tags".into());
        }
        let mut approved = field_keys.to_vec();
        approved.sort();
        approved.dedup();
        let metadata = recognition_metadata(recognition, &approved)?;
        let asset = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .ok_or("Autotag asset was removed")?;
        if job.input_hash != asset.hash {
            return Err("Autotag job is stale; retry recognition before applying tags".into());
        }
        if asset.processing == "external-readonly" {
            return Err("External audio is indexed read-only; recognition tagging requires a managed download".into());
        }
        if approved
            .iter()
            .any(|field| asset.track.provenance["manualFields"][field] == true)
        {
            return Err(
                "A manually tagged field cannot be replaced by recognition metadata".into(),
            );
        }

        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let updated = media::autotag_asset(&asset, &metadata, &settings).await?;
        if updated.id != asset.id
            || updated.recording_id != asset.recording_id
            || updated.track.id != asset.track.id
            || updated.track.provider != asset.track.provider
            || updated.track.provider_id != asset.track.provider_id
            || updated.track.source_url != asset.track.source_url
        {
            return Err(
                "Recognition tagging unexpectedly changed the library recording identity".into(),
            );
        }

        job.input_hash = updated.hash.clone();
        job.approved_fields = approved;
        job.status = "recognition_applied".into();
        job.message = Some(
            "Selected audio-recognized tags were applied; catalog recording identity remains unverified.".into(),
        );
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.store
            .save_autotagged_asset(
                &updated,
                &asset.hash,
                VERSION,
                &serde_json::to_value(&job).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
            media::acknowledge_retag_journal(&settings, id)?;
        }
        self.persist(&job)?;
        let _ = self.app.emit("autotag-status", &job);
        Ok(updated)
    }

    pub async fn retry_due(&self, job: &AutoTagJob) -> bool {
        let Some(next) = &job.next_attempt_at else {
            return true;
        };
        chrono::DateTime::parse_from_rfc3339(next)
            .map(|at| at <= chrono::Utc::now())
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MonochromeAcquisitionLocator {
    provider_id: String,
    url: String,
}

fn monochrome_acquisition_locator(track: &Track) -> Option<MonochromeAcquisitionLocator> {
    let acquisition = &track.provenance["acquisition"];
    if acquisition["provider"].as_str() != Some("monochrome") {
        return None;
    }
    let id = acquisition["id"].as_str()?.strip_prefix("monochrome:")?;
    if !(17..=20).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let raw_url = acquisition["url"].as_str()?;
    let parsed = crate::acquisition::validate_source_url(raw_url, "monochrome").ok()?;
    if parsed.host_str() != Some("tracks.monochrome.st")
        || parsed.path() != format!("/track/{id}")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(MonochromeAcquisitionLocator {
        provider_id: id.to_owned(),
        url: parsed.to_string(),
    })
}

fn accepted_acquisition_catalog_match(
    candidate: &crate::models::Candidate,
    assessment: &crate::recording_match::MatchAssessment,
) -> bool {
    assessment.action == crate::recording_match::MatchAction::Accept
        && matches!(
            assessment.relationship,
            crate::recording_match::MatchRelationship::Exact
                | crate::recording_match::MatchRelationship::Probable
        )
        && candidate.permission != "restricted"
}

fn accepted_acquisition_catalog_metadata(
    source: &Track,
    locator: &MonochromeAcquisitionLocator,
    candidate: &crate::models::Candidate,
    assessment: &crate::recording_match::MatchAssessment,
) -> Option<Track> {
    if !exact_monochrome_locator_match(locator, candidate)
        || !accepted_acquisition_catalog_match(candidate, assessment)
    {
        return None;
    }
    let metadata = candidate.source_metadata.as_ref()?;
    if metadata["provider"].as_str() != Some("monochrome")
        || metadata["trackId"].as_str() != Some(locator.provider_id.as_str())
    {
        return None;
    }
    let missing = |value: Option<&str>| value.is_none_or(|value| value.trim().is_empty());
    let manual = &source.provenance["manualFields"];
    let mut promoted = Track::default();
    let mut fields = Vec::new();

    if missing(source.album.as_deref()) && manual["album"] != true {
        promoted.album = bounded_catalog_text(&metadata["album"], 512);
        if promoted.album.is_some() {
            fields.push("album");
        }
    }
    if missing(source.album_artist.as_deref()) && manual["albumArtist"] != true {
        promoted.album_artist = bounded_catalog_text(&metadata["albumArtist"], 512);
        if promoted.album_artist.is_some() {
            fields.push("albumArtist");
        }
    }
    if missing(source.release_date.as_deref()) && manual["releaseDate"] != true {
        promoted.release_date = bounded_catalog_text(&metadata["releaseDate"], 64);
        if promoted.release_date.is_some() {
            fields.push("releaseDate");
        }
    }
    if missing(source.genre.as_deref()) && manual["genre"] != true {
        promoted.genre = bounded_catalog_text(&metadata["genre"], 256);
        if promoted.genre.is_some() {
            fields.push("genre");
        }
    }
    if missing(source.label.as_deref()) && manual["label"] != true {
        promoted.label = bounded_catalog_text(&metadata["label"], 256);
        if promoted.label.is_some() {
            fields.push("label");
        }
    }
    if source.isrc.is_none() && manual["isrc"] != true {
        promoted.isrc = metadata["isrc"]
            .as_str()
            .and_then(crate::recording_identity::normalize_isrc);
        if promoted.isrc.is_some() {
            fields.push("isrc");
        }
    }
    if fields.is_empty() {
        return None;
    }
    promoted.provenance = json!({
        "source":"accepted_acquisition_catalog_metadata",
        "identityStatus":"accepted_recording_match",
        "provider":"monochrome",
        "providerId":locator.provider_id,
        "sourceUrl":locator.url,
        "relationship":assessment.relationship,
        "fields":fields,
    });
    Some(promoted)
}

fn bounded_catalog_text(value: &Value, max_len: usize) -> Option<String> {
    let value = value.as_str()?.trim();
    (!value.is_empty() && value.len() <= max_len).then(|| value.to_owned())
}

fn exact_monochrome_locator_match(
    locator: &MonochromeAcquisitionLocator,
    candidate: &crate::models::Candidate,
) -> bool {
    candidate.provider == "monochrome"
        && candidate.id == format!("monochrome:{}", locator.provider_id)
        && url::Url::parse(&candidate.url)
            .ok()
            .is_some_and(|url| url.to_string() == locator.url)
}

fn accepted_acquisition_isrc_backfill(source: &Track) -> bool {
    let marker = &source.provenance["autotag"];
    verified_acquisition_catalog_marker(source, marker)
        && marker["fields"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field.as_str() == Some("isrc")))
        && source
            .isrc
            .as_deref()
            .and_then(crate::recording_identity::normalize_isrc)
            .is_some()
}

fn accepted_acquisition_catalog_metadata_backfill(source: &Track) -> bool {
    let marker = &source.provenance["autotag"];
    verified_acquisition_catalog_marker(source, marker)
        && marker["fields"].as_array().is_some_and(|fields| {
            fields.iter().any(|field| match field.as_str() {
                Some("album") => source
                    .album
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                Some("albumArtist") => source
                    .album_artist
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                Some("releaseDate") => source
                    .release_date
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                Some("genre") => source
                    .genre
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                Some("label") => source
                    .label
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                Some("isrc") => source
                    .isrc
                    .as_deref()
                    .and_then(crate::recording_identity::normalize_isrc)
                    .is_some(),
                _ => false,
            })
        })
}

fn verified_acquisition_catalog_marker(source: &Track, marker: &Value) -> bool {
    let Some(locator) = monochrome_acquisition_locator(source) else {
        return false;
    };
    matches!(
        marker["source"].as_str(),
        Some("accepted_acquisition_catalog_metadata" | "accepted_acquisition_catalog_isrc")
    ) && marker["identityStatus"].as_str() == Some("accepted_recording_match")
        && marker["provider"].as_str() == Some("monochrome")
        && marker["providerId"].as_str() == Some(locator.provider_id.as_str())
        && marker["sourceUrl"].as_str() == Some(locator.url.as_str())
        && matches!(marker["relationship"].as_str(), Some("exact" | "probable"))
}

fn requested_identity_for_catalog_backfill(source: &Track) -> Option<Track> {
    let requested = source.provenance.get("requestedTrackIdentity");
    if let Some(requested) = requested {
        let id = requested["originalId"]
            .as_str()
            .or_else(|| requested["id"].as_str())?;
        let provider = requested["provider"].as_str()?;
        let provider_id = requested["providerId"].as_str()?;
        let title = requested["title"].as_str()?.trim();
        let artist = requested["artist"].as_str()?.trim();
        if id != format!("{provider}:{provider_id}")
            || requested["id"]
                .as_str()
                .is_some_and(|stored_id| stored_id != id)
            || provider.is_empty()
            || provider_id.is_empty()
            || title.is_empty()
            || artist.is_empty()
        {
            return None;
        }
        return Some(Track {
            id: id.to_owned(),
            provider: provider.to_owned(),
            provider_id: provider_id.to_owned(),
            title: title.to_owned(),
            artist: artist.to_owned(),
            version: requested["version"].as_str().map(str::to_owned),
            duration_seconds: source.duration_seconds,
            provenance: source.provenance.clone(),
            ..Track::default()
        });
    }
    (!source.id.trim().is_empty()
        && !source.provider.trim().is_empty()
        && !source.provider_id.trim().is_empty()
        && !source.title.trim().is_empty()
        && !source.artist.trim().is_empty())
    .then(|| source.clone())
}

pub fn job_key(asset_id: &str, content_hash: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(asset_id);
    digest.update([0]);
    digest.update(content_hash);
    digest.update([0]);
    digest.update(VERSION);
    format!("{:x}", digest.finalize())
}

fn decode_job_records(rows: Vec<Value>) -> Vec<AutoTagJob> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|row| row["status"] != "superseded" && row["version"] == VERSION)
        .filter_map(|row| serde_json::from_value::<AutoTagJob>(row["payload"].clone()).ok())
        .filter(|job| {
            job.version == VERSION
                && seen.insert((
                    job.asset_id.clone(),
                    job.input_hash.clone(),
                    job.version.clone(),
                ))
        })
        .collect()
}

fn candidate_from_mb(row: &Value, source: &Track) -> Option<AutoTagCandidate> {
    let id = row["id"].as_str()?;
    let title = row["title"].as_str()?.trim();
    let artist = row["artist-credit"]
        .as_array()?
        .iter()
        .map(|credit| {
            credit["name"]
                .as_str()
                .or_else(|| credit["artist"]["name"].as_str())
                .unwrap_or("")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("");
    if title.is_empty() || artist.is_empty() {
        return None;
    }
    let dur = row["length"].as_f64().map(|ms| ms / 1000.0);
    let isrcs = row["isrcs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let title_score = jaro_winkler(&normalize(title), &normalize(&source.title));
    let artist_score = jaro_winkler(&normalize(&artist), &normalize(&source.artist));
    if title_score < 0.94
        || artist_score < 0.94
        || !compatible_version(
            &source.title,
            source.version.as_deref(),
            title,
            row["version"].as_str(),
        )
    {
        return None;
    }
    if let (Some(a), Some(b)) = (dur, source.duration_seconds) {
        let delta = (a - b).abs();
        if delta > MAX_DURATION_DELTA && delta / b.max(1.0) > MAX_DURATION_RATIO {
            return None;
        }
    }
    let score = title_score * 0.55 + artist_score * 0.4 + if dur.is_some() { 0.05 } else { 0.0 };
    let url = format!("https://musicbrainz.org/recording/{id}");
    let fields = Track {
        artist: artist.clone(),
        title: title.to_owned(),
        duration_seconds: dur,
        isrc: (isrcs.len() == 1).then(|| isrcs[0].to_owned()),
        genre: row["genres"]
            .as_array()
            .and_then(|genres| genres.first())
            .and_then(|genre| genre["name"].as_str())
            .map(str::to_owned),
        provider: "musicbrainz".into(),
        provider_id: id.into(),
        source_url: url.clone(),
        ..Track::default()
    };
    Some(AutoTagCandidate {
        provider: "musicbrainz".into(),
        provider_id: id.into(),
        source_url: url,
        title: title.into(),
        artist,
        duration_seconds: dur,
        score,
        fields,
        evidence: vec![
            format!("artist similarity {artist_score:.3}"),
            format!("title similarity {title_score:.3}"),
            if dur.is_some() {
                "duration is compatible".into()
            } else {
                "catalog duration unavailable".into()
            },
        ],
    })
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compatible_version(
    source_title: &str,
    source_version: Option<&str>,
    candidate_title: &str,
    candidate_version: Option<&str>,
) -> bool {
    fn signature(title: &str, version: Option<&str>) -> Vec<&'static str> {
        const MARKERS: [&str; 15] = [
            "live",
            "remix",
            "edit",
            "mix",
            "version",
            "instrumental",
            "acapella",
            "acoustic",
            "dub",
            "radio",
            "extended",
            "club",
            "remaster",
            "clean",
            "explicit",
        ];
        let text = format!(
            "{} {}",
            normalize(title),
            normalize(version.unwrap_or_default())
        );
        MARKERS
            .into_iter()
            .filter(|marker| text.split_whitespace().any(|word| word == *marker))
            .collect()
    }
    signature(source_title, source_version) == signature(candidate_title, candidate_version)
}

fn strong_catalog_match(source: &Track, candidate: &AutoTagCandidate) -> bool {
    normalize(&source.artist) == normalize(&candidate.artist)
        && normalize(&source.title) == normalize(&candidate.title)
        && normalize(source.version.as_deref().unwrap_or(""))
            == normalize(candidate.fields.version.as_deref().unwrap_or(""))
        && compatible_version(
            &source.title,
            source.version.as_deref(),
            &candidate.title,
            candidate.fields.version.as_deref(),
        )
        && source
            .duration_seconds
            .zip(candidate.duration_seconds)
            .is_some_and(|(a, b)| duration_compatible(a, b))
}

fn duration_compatible(a: f64, b: f64) -> bool {
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 {
        return false;
    }
    let delta = (a - b).abs();
    delta <= MAX_DURATION_DELTA || delta / a.max(1.0) <= MAX_DURATION_RATIO
}

fn can_auto_apply(source: &Track, candidate: &AutoTagCandidate, excerpt_only: bool) -> bool {
    if candidate.fields.provenance["releaseContextUnavailable"] == true
        || source_conflicts(source, candidate)
        || source_reported_identity_requires_review(source, candidate)
    {
        return false;
    }
    if excerpt_only {
        return candidate.evidence.iter().any(|e| {
            e.starts_with("AcoustID audio fingerprint score ")
                && e.rsplit_once(' ')
                    .and_then(|(_, n)| n.parse::<f64>().ok())
                    .is_some_and(|score| score >= 0.95)
        }) || candidate.evidence.iter().any(|e| {
            e.starts_with("SongRec recognized two non-overlapping excerpts consistently")
        });
    }
    let uses_unverified_source_identity = ["artist", "title"]
        .iter()
        .any(|field| source_field_is_unverified_report(source, field))
        || candidate_used_source_reported_lookup(candidate);
    (candidate.provider == "musicbrainz" || candidate.provider == "discogs")
        && if uses_unverified_source_identity {
            audio_recognition_verified(candidate)
        } else {
            candidate.score >= 0.96 && strong_catalog_match(source, candidate)
        }
}

fn autotag_job_retryable(job: &AutoTagJob) -> bool {
    ["retry", "no_match", "failed"].contains(&job.status.as_str())
        || (job.status == "review" && job.chosen.is_none() && job.approved_fields.is_empty())
}

fn classify_lookup_result(
    source: &Track,
    candidate_count: usize,
    recognized: bool,
) -> (&'static str, &'static str) {
    if candidate_count == 0 && accepted_acquisition_catalog_metadata_backfill(source) {
        (
            "applied",
            "Verified metadata is tagged from the acquisition source; no additional catalog metadata was found.",
        )
    } else if candidate_count == 0 && recognized {
        (
            "review",
            "Audio recognition matched two separate excerpts, but no unique catalog recording was resolved. Review the recognized artist and title; catalog identity is unverified.",
        )
    } else if candidate_count == 0 {
        (
            "no_match",
            "No sufficiently compatible catalog match was found.",
        )
    } else {
        (
            "review",
            "Catalog candidates are ambiguous; select one in review.",
        )
    }
}

fn current_asset_retry_job(
    store: &Store,
    asset_id: &str,
) -> Result<(LocalAsset, AutoTagJob), String> {
    let asset = store
        .get_asset_by_id(asset_id)
        .map_err(|error| error.to_string())?
        .ok_or("Autotag asset was removed")?;
    let rows = store
        .list_autotag_records()
        .map_err(|error| error.to_string())?;
    let jobs = decode_job_records(rows);
    let previous = jobs
        .iter()
        .find(|job| job.asset_id == asset.id && job.input_hash == asset.hash)
        .or_else(|| jobs.iter().find(|job| job.asset_id == asset.id))
        .ok_or("Autotag job not found")?;
    if !autotag_job_retryable(previous) {
        return Err("Only failed, unmatched, or unapproved review jobs can be retried".into());
    }
    // Always build fresh evidence for current bytes. Rows for previous hashes
    // remain as history; none of their candidates or approvals cross hashes.
    Ok((
        asset.clone(),
        AutoTagJob {
            asset_id: asset.id,
            input_hash: asset.hash,
            version: VERSION.into(),
            status: "queued".into(),
            attempts: 0,
            next_attempt_at: None,
            message: None,
            candidates: vec![],
            chosen: None,
            recognition: None,
            approved_fields: vec![],
            excerpt_only: false,
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    ))
}

fn reserve_retry_slot(
    pending: &std::sync::Mutex<HashSet<String>>,
    inflight: &Mutex<HashSet<String>>,
    asset_id: &str,
) -> Result<(), String> {
    let mut pending = pending.lock().unwrap_or_else(|error| error.into_inner());
    if pending.contains(asset_id) {
        return Err("Autotag job is already queued for this asset".into());
    }
    let inflight = inflight
        .try_lock()
        .map_err(|_| "Autotag job is currently running for this asset")?;
    if inflight.contains(asset_id) {
        return Err("Autotag job is currently running for this asset".into());
    }
    pending.insert(asset_id.to_owned());
    Ok(())
}

fn release_retry_slot(pending: &std::sync::Mutex<HashSet<String>>, asset_id: &str) {
    pending
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(asset_id);
}

fn source_reported_identity_requires_review(source: &Track, candidate: &AutoTagCandidate) -> bool {
    let has_unverified_identity = ["artist", "title"]
        .iter()
        .any(|field| source_field_is_unverified_report(source, field))
        || candidate_used_source_reported_lookup(candidate);
    has_unverified_identity && !audio_recognition_verified(candidate)
}

fn candidate_used_source_reported_lookup(candidate: &AutoTagCandidate) -> bool {
    candidate.evidence.iter().any(|evidence| {
        evidence
            .strip_prefix("Catalog lookup used exact-source reported field(s):")
            .and_then(|details| details.split_once(';'))
            .is_some_and(|(fields, _)| {
                fields
                    .split(',')
                    .map(str::trim)
                    .any(|field| matches!(field, "artist" | "title"))
            })
    })
}

fn source_field_is_unverified_report(source: &Track, field: &str) -> bool {
    if source.provenance["sourceReportedMetadata"]["identityStatus"] != "source_reported_unverified"
        || source.provenance["sourceReportedMetadata"]["provider"].as_str()
            != Some(source.provider.as_str())
        || source.provenance["sourceReportedMetadata"]["providerId"].as_str()
            != Some(source.provider_id.as_str())
        || source.provenance["sourceReportedMetadata"]["sourceUrl"].as_str()
            != Some(source.source_url.as_str())
    {
        return false;
    }
    source.provenance["manualFields"][field] != true
        && source.provenance["sourceReportedMetadata"]["fields"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|value| value.as_str() == Some(field)))
}

fn audio_recognition_verified(candidate: &AutoTagCandidate) -> bool {
    candidate.evidence.iter().any(|evidence| {
        evidence.starts_with("SongRec recognized two non-overlapping excerpts consistently")
            || (evidence.starts_with("AcoustID audio fingerprint score ")
                && evidence
                    .rsplit_once(' ')
                    .and_then(|(_, score)| score.parse::<f64>().ok())
                    .is_some_and(|score| score >= 0.95))
    })
}

fn add_source_reported_lookup_evidence(
    candidate: &mut AutoTagCandidate,
    source_reported_fields: &[String],
) {
    if !source_reported_fields.is_empty() {
        candidate.evidence.push(format!(
            "Catalog lookup used exact-source reported field(s): {}; source metadata alone does not verify recording identity.",
            source_reported_fields.join(", ")
        ));
    }
}

fn source_conflicts(source: &Track, candidate: &AutoTagCandidate) -> bool {
    (!source.artist.trim().is_empty()
        && !source_field_is_unverified_report(source, "artist")
        && normalize(&source.artist) != normalize(&candidate.artist))
        || (!source.title.trim().is_empty()
            && !source_field_is_unverified_report(source, "title")
            && normalize(&source.title) != normalize(&candidate.title))
}

fn candidate_provides_field(candidate: &AutoTagCandidate, key: &str) -> bool {
    match key {
        "artist" => !candidate.fields.artist.trim().is_empty(),
        "title" => !candidate.fields.title.trim().is_empty(),
        "album" => candidate.fields.album.is_some(),
        "albumArtist" => candidate.fields.album_artist.is_some(),
        "releaseDate" => candidate.fields.release_date.is_some(),
        "isrc" => candidate.fields.isrc.is_some(),
        "genre" => candidate.fields.genre.is_some(),
        "label" => candidate.fields.label.is_some(),
        "catalogNumber" => candidate.fields.catalog_number.is_some(),
        "trackNumber" => candidate.fields.track_number.is_some(),
        "trackTotal" => candidate.fields.track_total.is_some(),
        "discNumber" => candidate.fields.disc_number.is_some(),
        "discTotal" => candidate.fields.disc_total.is_some(),
        _ => false,
    }
}

fn recognition_metadata(
    recognition: &AudioRecognition,
    approved_fields: &[String],
) -> Result<Track, String> {
    const FIELDS: &[&str] = &["artist", "title", "isrc"];
    let mut approved = approved_fields.to_vec();
    approved.sort();
    approved.dedup();
    if approved.is_empty()
        || approved
            .iter()
            .any(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err("Recognition review supports artist, title, and ISRC only".into());
    }
    if recognition.provider != "songrec" || recognition.excerpt_count < 2 {
        return Err("Recognition must be a consistent two-excerpt SongRec result".into());
    }
    if approved.iter().any(|field| match field.as_str() {
        "artist" => recognition.artist.trim().is_empty(),
        "title" => recognition.title.trim().is_empty(),
        "isrc" => recognition
            .isrc
            .as_deref()
            .and_then(crate::recording_identity::normalize_isrc)
            .is_none(),
        _ => true,
    }) {
        return Err("Recognition has no value for one or more selected fields".into());
    }
    Ok(Track {
        artist: approved
            .iter()
            .any(|field| field == "artist")
            .then(|| recognition.artist.trim().to_owned())
            .unwrap_or_default(),
        title: approved
            .iter()
            .any(|field| field == "title")
            .then(|| recognition.title.trim().to_owned())
            .unwrap_or_default(),
        isrc: approved
            .iter()
            .any(|field| field == "isrc")
            .then(|| {
                recognition
                    .isrc
                    .as_deref()
                    .and_then(crate::recording_identity::normalize_isrc)
            })
            .flatten(),
        provenance: json!({
            "source":"audio_recognition_user_approved",
            "identityStatus":"source_reported_unverified",
            "recognitionProvider":recognition.provider,
            "shazamId":recognition.shazam_id,
            "excerptCount":recognition.excerpt_count,
            "userApprovedFields":approved,
        }),
        ..Track::default()
    })
}

fn mark_retry(job: &mut AutoTagJob, error: &str) {
    job.attempts = job.attempts.saturating_add(1);
    job.status = if job.attempts >= MAX_PROVIDER_ATTEMPTS {
        "failed"
    } else {
        "retry"
    }
    .into();
    let delay = (30u64.saturating_mul(2u64.saturating_pow(job.attempts.min(10)))).min(86_400);
    job.next_attempt_at = if job.status == "retry" {
        Some((chrono::Utc::now() + chrono::Duration::seconds(delay as i64)).to_rfc3339())
    } else {
        None
    };
    job.message = Some(error.to_owned());
    job.updated_at = chrono::Utc::now().to_rfc3339();
}

fn candidate_from_fingerprint_mb(row: &Value) -> Option<AutoTagCandidate> {
    let id = row["id"].as_str()?;
    let title = row["title"].as_str()?.trim();
    let artist = render_artist_credit(&row["artist-credit"])?;
    if title.is_empty() || artist.trim().is_empty() {
        return None;
    }
    let duration = row["length"].as_f64().map(|ms| ms / 1000.0);
    let fields = Track {
        artist: artist.clone(),
        title: title.into(),
        duration_seconds: duration,
        genre: row["genres"]
            .as_array()
            .and_then(|v| v.first())
            .and_then(|v| v["name"].as_str())
            .map(str::to_owned),
        provider: "musicbrainz".into(),
        provider_id: id.into(),
        source_url: format!("https://musicbrainz.org/recording/{id}"),
        ..Track::default()
    };
    Some(AutoTagCandidate {
        provider: "musicbrainz".into(),
        provider_id: id.into(),
        source_url: format!("https://musicbrainz.org/recording/{id}"),
        title: title.into(),
        artist,
        duration_seconds: duration,
        score: 0.95,
        fields,
        evidence: vec![
            "MusicBrainz recording identity returned by strong audio fingerprint".into(),
        ],
    })
}

fn unique_musicbrainz_rows_for_isrc(results: &Value, isrc: &str) -> Vec<Value> {
    let mut seen = HashSet::new();
    results["recordings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| {
            row["isrcs"].as_array().is_some_and(|codes| {
                codes
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|code| crate::recording_identity::same_isrc(code, isrc))
            })
        })
        .filter(|row| {
            row["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty() && seen.insert(id.to_owned()))
        })
        .cloned()
        .collect()
}

fn restrict_excerpt_fields(candidate: &mut AutoTagCandidate) {
    candidate.fields.album = None;
    candidate.fields.album_artist = None;
    candidate.fields.release_date = None;
    candidate.fields.recording_date = None;
    candidate.fields.isrc = None;
    candidate.fields.label = None;
    candidate.fields.catalog_number = None;
    candidate.fields.track_number = None;
    candidate.fields.track_total = None;
    candidate.fields.disc_number = None;
    candidate.fields.disc_total = None;
    candidate.fields.version = None;
    candidate.fields.remixers = None;
    candidate.fields.duration_seconds = None;
}

const SUPPORTED_FIELDS: &[&str] = &[
    "artist",
    "title",
    "album",
    "albumArtist",
    "releaseDate",
    "isrc",
    "genre",
    "label",
    "catalogNumber",
    "trackNumber",
    "trackTotal",
    "discNumber",
    "discTotal",
];

fn strong_field_keys(track: &Track) -> Vec<String> {
    [
        (!track.artist.is_empty()).then_some("artist"),
        (!track.title.is_empty()).then_some("title"),
        track.album.as_ref().map(|_| "album"),
        track.album_artist.as_ref().map(|_| "albumArtist"),
        track.release_date.as_ref().map(|_| "releaseDate"),
        track.isrc.as_ref().map(|_| "isrc"),
        track.genre.as_ref().map(|_| "genre"),
        track.label.as_ref().map(|_| "label"),
        track.catalog_number.as_ref().map(|_| "catalogNumber"),
        track.track_number.map(|_| "trackNumber"),
        track.track_total.map(|_| "trackTotal"),
        track.disc_number.map(|_| "discNumber"),
        track.disc_total.map(|_| "discTotal"),
    ]
    .into_iter()
    .flatten()
    .map(str::to_owned)
    .collect()
}

fn render_artist_credit(value: &Value) -> Option<String> {
    let credits = value.as_array()?;
    let mut rendered = String::new();
    for credit in credits {
        if let Some(name) = credit["name"]
            .as_str()
            .or_else(|| credit["artist"]["name"].as_str())
        {
            rendered.push_str(name);
        }
        if let Some(join) = credit["joinphrase"].as_str() {
            rendered.push_str(join);
        }
    }
    (!rendered.trim().is_empty()).then_some(rendered)
}

fn executable_available(name: &str) -> bool {
    crate::tools::executable(name, name).is_file()
}

fn completed_source_candidate<'a>(
    asset: &LocalAsset,
    job: &'a DownloadJob,
) -> Option<&'a Candidate> {
    let publication_job_id = asset.track.provenance["publicationJobId"].as_str()?;
    let candidate = &job.candidate;
    let acquisition = &asset.track.provenance["acquisition"];
    (!publication_job_id.is_empty()
        && job.id == publication_job_id
        && job.state == "completed"
        && job.error.is_none()
        && job.output_path.as_deref() == Some(asset.path.as_str())
        && !candidate.provider.is_empty()
        && !candidate.id.is_empty()
        && !candidate.url.is_empty()
        && acquisition["provider"].as_str() == Some(candidate.provider.as_str())
        && acquisition["id"].as_str() == Some(candidate.id.as_str())
        && acquisition["url"].as_str() == Some(candidate.url.as_str()))
    .then_some(candidate)
}

fn has_bound_yt_dlp_metadata(track: &Track, candidate: &Candidate) -> bool {
    let metadata = candidate.source_metadata.as_ref().unwrap_or(&Value::Null);
    candidate.provider == track.provider
        && candidate.id == track.id
        && candidate.url == track.source_url
        && candidate
            .id
            .strip_prefix(&format!("{}:", candidate.provider))
            == Some(track.provider_id.as_str())
        && metadata["resolvedProviderId"].as_str() == Some(track.provider_id.as_str())
        && metadata["sourceMetadataOrigin"]
            .as_str()
            .is_some_and(|origin| origin.starts_with("ytDlp"))
}

fn source_tag_repair_noop(
    asset: &LocalAsset,
    identity_status: &str,
) -> DownloadSourceTagRepairResult {
    DownloadSourceTagRepairResult {
        asset_id: asset.id.clone(),
        changed: false,
        fields: Vec::new(),
        identity_status: identity_status.into(),
    }
}

fn new_source_tag_repair_job(asset: &LocalAsset) -> AutoTagJob {
    AutoTagJob {
        asset_id: asset.id.clone(),
        input_hash: asset.hash.clone(),
        version: VERSION.into(),
        status: "review".into(),
        attempts: 0,
        next_attempt_at: None,
        message: None,
        candidates: Vec::new(),
        recognition: None,
        chosen: None,
        approved_fields: Vec::new(),
        excerpt_only: false,
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn source_field_needs_repair(source: &Track, reported: &Track, field: &str) -> bool {
    if source.provenance["manualFields"][field] == true {
        return false;
    }
    match field {
        "artist" => {
            !reported.artist.trim().is_empty()
                && (source.artist.trim().is_empty()
                    || source.provenance["weakFields"][field] == true)
        }
        "title" => {
            !reported.title.trim().is_empty()
                && (source.title.trim().is_empty()
                    || source.provenance["weakFields"][field] == true)
        }
        "album" => {
            source
                .album
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
                && reported.album.is_some()
        }
        "albumArtist" => {
            source
                .album_artist
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
                && reported.album_artist.is_some()
        }
        "genre" => {
            source
                .genre
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
                && reported.genre.is_some()
        }
        "isrc" => source.isrc.is_none() && reported.isrc.is_some(),
        _ => false,
    }
}

async fn songrec_excerpt(
    path: &str,
    start: f64,
    ffmpeg_path: &str,
) -> Result<Option<crate::radio::RadioTrackMetadata>, String> {
    songrec_excerpt_guarded(path, start, ffmpeg_path, || Ok(())).await
}

pub(crate) async fn songrec_excerpt_guarded(
    path: &str,
    start: f64,
    ffmpeg_path: &str,
    check: impl Fn() -> Result<(), String>,
) -> Result<Option<crate::radio::RadioTrackMetadata>, String> {
    check()?;
    // Own the entire private directory across both awaits. Dropping a cancelled
    // request also drops its excerpt, rather than skipping manual cleanup.
    let temporary = tempfile::Builder::new()
        .prefix("burrata-autotag-")
        .tempdir()
        .map_err(|error| format!("Cannot create recognition excerpt directory: {error}"))?;
    let wav = temporary.path().join("excerpt.wav");
    let mut ffmpeg = tokio::process::Command::new(crate::tools::executable(ffmpeg_path, "ffmpeg"));
    crate::tools::configure_no_console_tokio(&mut ffmpeg);
    ffmpeg
        .args(["-nostdin", "-v", "error", "-y", "-ss"])
        .arg(format!("{start:.3}"))
        .arg("-i")
        .arg(path)
        .args(["-t", "20", "-vn", "-ac", "1", "-ar", "16000", "-f", "wav"])
        .arg(&wav)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let extracted = tokio::time::timeout(Duration::from_secs(30), ffmpeg.status()).await;
    if !matches!(extracted, Ok(Ok(status)) if status.success()) {
        return Ok(None);
    }
    check()?;
    let station = crate::radio::RadioStation {
        id: "autotag-recognition".into(),
        name: "Local autotag recognition".into(),
        stream_url: String::new(),
        source_url: String::new(),
        provider: None,
        country: None,
        tags: None,
        homepage_url: None,
        favicon_url: None,
        bitrate_kbps: None,
        codec: None,
        language: None,
        is_custom: true,
    };
    let output = tokio::time::timeout(
        Duration::from_secs(50),
        crate::radio::recognize_captured_sample_uncached(&station, &wav),
    )
    .await;
    match output {
        Ok(Ok(result)) => Ok(result),
        _ => Ok(None),
    }
}

fn is_live_performance(track: &Track) -> bool {
    track.provenance["titleInference"]["type"] == "livePerformance"
}

fn is_excerpt(track: &Track) -> bool {
    track.provenance["chapter"].is_object()
        || track.provenance["isExcerpt"] == true
        || track.provenance["sourceKind"] == "chapter"
        || track.playback_range.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str, duration: Option<f64>) -> Track {
        Track {
            artist: artist.into(),
            title: title.into(),
            duration_seconds: duration,
            ..Track::default()
        }
    }

    fn candidate(artist: &str, title: &str, duration: Option<f64>) -> AutoTagCandidate {
        AutoTagCandidate {
            provider: "musicbrainz".into(),
            provider_id: "actual-mbid".into(),
            source_url: "https://musicbrainz.org/recording/actual-mbid".into(),
            title: title.into(),
            artist: artist.into(),
            duration_seconds: duration,
            score: 0.99,
            fields: track(artist, title, duration),
            evidence: vec![],
        }
    }

    fn persisted_job(hash: &str, status: &str) -> AutoTagJob {
        AutoTagJob {
            asset_id: "asset-1".into(),
            input_hash: hash.into(),
            version: VERSION.into(),
            status: status.into(),
            attempts: 0,
            next_attempt_at: None,
            message: None,
            candidates: vec![],
            chosen: None,
            recognition: None,
            approved_fields: vec![],
            excerpt_only: false,
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn two_excerpt_recognition_without_unique_catalog_record_enters_review() {
        let source = track("YouTube Guy Bertrand", "Por ella", Some(213.5));
        let (status, message) = classify_lookup_result(&source, 0, true);
        assert_eq!(status, "review");
        assert!(message.contains("catalog identity is unverified"));

        let (status, _) = classify_lookup_result(&source, 0, false);
        assert_eq!(status, "no_match");
        let (status, message) = classify_lookup_result(&source, 2, true);
        assert_eq!(status, "review");
        assert!(message.contains("candidates are ambiguous"));
    }

    #[test]
    fn recognition_is_persisted_separately_and_old_jobs_still_decode() {
        let mut job = persisted_job("hash", "review");
        job.recognition = Some(AudioRecognition {
            provider: "songrec".into(),
            artist: "Tékaméli".into(),
            title: "Por Ella".into(),
            isrc: Some("FRZ089900030".into()),
            shazam_id: Some("10222041".into()),
            excerpt_count: 2,
            agreeing_identifiers: Default::default(),
        });
        let encoded = serde_json::to_value(&job).unwrap();
        let decoded: AutoTagJob = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded.recognition, job.recognition);
        assert!(decoded.candidates.is_empty());
        assert!(decoded.chosen.is_none());

        let mut legacy = encoded;
        legacy.as_object_mut().unwrap().remove("recognition");
        let decoded_legacy: AutoTagJob = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded_legacy.recognition, None);
    }

    #[test]
    fn recognition_review_metadata_contains_only_checked_tags_and_unverified_provenance() {
        let recognition = AudioRecognition {
            provider: "songrec".into(),
            artist: "Tékaméli".into(),
            title: "Por Ella".into(),
            isrc: Some("FR-Z08-99-00030".into()),
            shazam_id: Some("10222041".into()),
            excerpt_count: 2,
            agreeing_identifiers: Default::default(),
        };
        let metadata = recognition_metadata(
            &recognition,
            &["title".into(), "isrc".into(), "title".into()],
        )
        .unwrap();
        assert!(metadata.artist.is_empty());
        assert_eq!(metadata.title, "Por Ella");
        assert_eq!(metadata.isrc.as_deref(), Some("FRZ089900030"));
        assert_eq!(
            metadata.provenance["userApprovedFields"],
            json!(["isrc", "title"])
        );
        assert_eq!(
            metadata.provenance["identityStatus"],
            "source_reported_unverified"
        );
        assert_eq!(metadata.provider, "");
        assert!(recognition_metadata(&recognition, &["album".into()]).is_err());
        assert!(recognition_metadata(&recognition, &["artist".into(), "album".into()]).is_err());
    }

    #[test]
    fn recognition_review_requires_values_and_two_excerpt_songrec_evidence() {
        let mut recognition = AudioRecognition {
            provider: "songrec".into(),
            artist: "Recognized Artist".into(),
            title: "Recognized Title".into(),
            isrc: None,
            shazam_id: None,
            excerpt_count: 2,
            agreeing_identifiers: Default::default(),
        };
        assert!(recognition_metadata(&recognition, &[]).is_err());
        assert!(recognition_metadata(&recognition, &["isrc".into()]).is_err());
        recognition.excerpt_count = 1;
        assert!(recognition_metadata(&recognition, &["artist".into()]).is_err());
        recognition.excerpt_count = 2;
        recognition.provider = "unknown".into();
        assert!(recognition_metadata(&recognition, &["artist".into()]).is_err());
    }

    fn retry_store_asset(id: &str, hash: &str, track: Track) -> LocalAsset {
        LocalAsset {
            id: id.into(),
            recording_id: track.id.clone(),
            path: format!("/tmp/{id}.flac"),
            hash: hash.into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(44_100),
            channels: Some(2),
            bit_depth: Some(16),
            duration_seconds: Some(180.0),
            size_bytes: Some(1),
            bitrate_origin: None,
            processing: "managed".into(),
            track,
        }
    }

    fn retry_test_store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("retry.sqlite")).unwrap();
        (directory, store)
    }

    fn retry_test_track() -> Track {
        Track {
            id: "fixture:retry-track".into(),
            provider: "fixture".into(),
            provider_id: "retry-track".into(),
            source_url: "https://example.test/retry-track".into(),
            artist: "Artist".into(),
            title: "Title".into(),
            provenance: json!({"manualFields":{"album":true},"requestedTrackIdentity":{"id":"fixture:retry-track"}}),
            ..Track::default()
        }
    }

    #[test]
    fn retry_uses_current_store_hash_and_resets_old_evidence() {
        let (_directory, store) = retry_test_store();
        let track = retry_test_track();
        let old_asset = retry_store_asset("retry-asset", "old-hash", track.clone());
        store.save_asset(&old_asset).unwrap();
        let mut old_job = persisted_job("old-hash", "no_match");
        old_job.asset_id = old_asset.id.clone();
        old_job
            .candidates
            .push(candidate("Old Artist", "Old Title", Some(180.0)));
        old_job.chosen = old_job.candidates.first().cloned();
        old_job.approved_fields = vec!["title".into()];
        store
            .save_autotag_record(
                &old_job.asset_id,
                &old_job.input_hash,
                VERSION,
                &old_job.status,
                &serde_json::to_value(&old_job).unwrap(),
            )
            .unwrap();

        let current_asset = retry_store_asset("retry-asset", "current-hash", track.clone());
        store.save_asset(&current_asset).unwrap();
        let (loaded, fresh_job) = current_asset_retry_job(&store, &old_asset.id).unwrap();
        assert_eq!(loaded.hash, "current-hash");
        assert_eq!(fresh_job.input_hash, "current-hash");
        assert_eq!(fresh_job.status, "queued");
        assert_eq!(fresh_job.attempts, 0);
        assert!(fresh_job.candidates.is_empty());
        assert!(fresh_job.chosen.is_none());
        assert!(fresh_job.approved_fields.is_empty());
        assert_eq!(loaded.track.provenance, track.provenance);

        store
            .save_autotag_record(
                &fresh_job.asset_id,
                &fresh_job.input_hash,
                VERSION,
                &fresh_job.status,
                &serde_json::to_value(&fresh_job).unwrap(),
            )
            .unwrap();
        let records = decode_job_records(store.list_autotag_records().unwrap());
        assert!(records.iter().any(|job| job.input_hash == "old-hash"));
        assert!(records.iter().any(|job| {
            job.input_hash == "current-hash" && job.status == "queued" && job.candidates.is_empty()
        }));
    }

    #[test]
    fn retry_requires_existing_asset_and_retryable_previous_job() {
        let (_directory, store) = retry_test_store();
        assert_eq!(
            current_asset_retry_job(&store, "removed-asset").unwrap_err(),
            "Autotag asset was removed"
        );

        let asset = retry_store_asset("applied-asset", "current-hash", retry_test_track());
        store.save_asset(&asset).unwrap();
        let mut applied = persisted_job("current-hash", "applied");
        applied.asset_id = asset.id.clone();
        store
            .save_autotag_record(
                &asset.id,
                &applied.input_hash,
                VERSION,
                &applied.status,
                &serde_json::to_value(&applied).unwrap(),
            )
            .unwrap();
        assert!(current_asset_retry_job(&store, &asset.id)
            .unwrap_err()
            .contains("Only failed"));
    }

    #[test]
    fn retry_slot_rejects_pending_and_inflight_asset_overlap() {
        let pending = std::sync::Mutex::new(HashSet::from(["asset-1".to_owned()]));
        let inflight = Mutex::new(HashSet::new());
        assert!(reserve_retry_slot(&pending, &inflight, "asset-1")
            .unwrap_err()
            .contains("already queued"));
        assert!(pending.lock().unwrap().contains("asset-1"));

        let pending = std::sync::Mutex::new(HashSet::new());
        let inflight = Mutex::new(HashSet::from(["asset-1".to_owned()]));
        assert!(reserve_retry_slot(&pending, &inflight, "asset-1")
            .unwrap_err()
            .contains("currently running"));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn monochrome_backfill_candidate_must_pass_shared_resolved_policy() {
        let request = Track {
            id: "spotify:requested".into(),
            provider: "spotify".into(),
            provider_id: "requested".into(),
            artist: "Artist".into(),
            title: "Title".into(),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        let exact = crate::models::Candidate {
            id: "monochrome:12345678901234567".into(),
            provider: "monochrome".into(),
            url: "https://tracks.monochrome.st/track/12345678901234567".into(),
            title: "Artist - Title".into(),
            uploader: Some("Artist".into()),
            duration_seconds: Some(180.0),
            permission: "unknown".into(),
            source_metadata: Some(json!({
                "provider":"monochrome", "trackId":"12345678901234567",
                "catalogTitle":"Title", "artistNames":["Artist"], "isrc":"USABC2600001"
            })),
            ..crate::models::Candidate::default()
        };
        let assessment = crate::recording_match::assess(
            &request,
            &exact,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert!(accepted_acquisition_catalog_match(&exact, &assessment));

        let mut wrong_recording = exact.clone();
        wrong_recording.source_metadata.as_mut().unwrap()["catalogTitle"] = json!("Other Title");
        // Candidate.title is also derived from the canonical Monochrome result;
        // keep the fixture internally consistent so it cannot leak the requested title.
        wrong_recording.title = "Artist - Other Title".into();
        let rejected = crate::recording_match::assess(
            &request,
            &wrong_recording,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert!(!accepted_acquisition_catalog_match(
            &wrong_recording,
            &rejected
        ));
    }

    #[test]
    fn accepted_acquisition_metadata_backfills_only_missing_nonmanual_fields() {
        let mut source = track("Artist", "Title", Some(180.0));
        source.provenance = json!({"manualFields":{"albumArtist":true,"label":true}});
        source.album_artist = Some("Manual Album Artist".into());
        source.genre = Some("Existing Genre".into());
        let locator = MonochromeAcquisitionLocator {
            provider_id: "12345678901234567".into(),
            url: "https://tracks.monochrome.st/track/12345678901234567".into(),
        };
        let candidate = crate::models::Candidate {
            id: "monochrome:12345678901234567".into(),
            provider: "monochrome".into(),
            url: locator.url.clone(),
            title: "Artist - Title".into(),
            uploader: Some("Artist".into()),
            duration_seconds: Some(180.0),
            permission: "unknown".into(),
            source_metadata: Some(json!({
                "provider":"monochrome", "trackId":"12345678901234567",
                "catalogTitle":"Title", "artistNames":["Artist"],
                "isrc":"USABC2600001", "album":"Release", "albumArtist":"Wrong Album Artist",
                "releaseDate":"2024-03-01", "genre":"Wrong Genre", "label":"Wrong Label"
            })),
            ..crate::models::Candidate::default()
        };
        let assessment = crate::recording_match::assess(
            &source,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        let metadata =
            accepted_acquisition_catalog_metadata(&source, &locator, &candidate, &assessment)
                .unwrap();

        assert_eq!(metadata.album.as_deref(), Some("Release"));
        assert_eq!(metadata.album_artist, None);
        assert_eq!(metadata.release_date.as_deref(), Some("2024-03-01"));
        assert_eq!(metadata.genre, None);
        assert_eq!(metadata.label, None);
        assert_eq!(metadata.isrc.as_deref(), Some("USABC2600001"));
        assert!(metadata.artist.is_empty() && metadata.title.is_empty());
        assert_eq!(source.artist, "Artist");
        assert_eq!(source.title, "Title");
        assert_eq!(
            metadata.provenance["fields"],
            json!(["album", "releaseDate", "isrc"])
        );
    }

    #[test]
    fn accepted_acquisition_metadata_rejects_wrong_recording_or_locator() {
        let source = track("Artist", "Title", Some(180.0));
        let locator = MonochromeAcquisitionLocator {
            provider_id: "12345678901234567".into(),
            url: "https://tracks.monochrome.st/track/12345678901234567".into(),
        };
        let mut candidate = crate::models::Candidate {
            id: "monochrome:12345678901234567".into(),
            provider: "monochrome".into(),
            url: locator.url.clone(),
            title: "Artist - Other Title".into(),
            uploader: Some("Artist".into()),
            duration_seconds: Some(180.0),
            permission: "unknown".into(),
            source_metadata: Some(json!({
                "provider":"monochrome", "trackId":"12345678901234567",
                "catalogTitle":"Other Title", "artistNames":["Artist"],
                "album":"Wrong Release", "isrc":"USABC2600001"
            })),
            ..crate::models::Candidate::default()
        };
        let rejected = crate::recording_match::assess(
            &source,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert!(
            accepted_acquisition_catalog_metadata(&source, &locator, &candidate, &rejected)
                .is_none()
        );

        candidate.title = "Artist - Title".into();
        candidate.source_metadata.as_mut().unwrap()["catalogTitle"] = json!("Title");
        let accepted = crate::recording_match::assess(
            &source,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        let mut wrong_locator = locator;
        wrong_locator.provider_id = "12345678901234568".into();
        assert!(accepted_acquisition_catalog_metadata(
            &source,
            &wrong_locator,
            &candidate,
            &accepted
        )
        .is_none());
    }

    #[test]
    fn monochrome_backfill_requires_exact_stored_acquisition_locator() {
        let mut source = track("Artist", "Title", Some(180.0));
        source.provenance = json!({"acquisition":{
            "provider":"monochrome",
            "id":"monochrome:12345678901234567",
            "url":"https://tracks.monochrome.st/track/12345678901234567"
        }});
        let locator = monochrome_acquisition_locator(&source).unwrap();
        let matching = crate::models::Candidate {
            id: "monochrome:12345678901234567".into(),
            provider: "monochrome".into(),
            url: "https://tracks.monochrome.st/track/12345678901234567".into(),
            ..crate::models::Candidate::default()
        };
        assert!(exact_monochrome_locator_match(&locator, &matching));
        let mut wrong_id = matching.clone();
        wrong_id.id = "monochrome:12345678901234568".into();
        assert!(!exact_monochrome_locator_match(&locator, &wrong_id));
        let mut wrong_url = matching;
        wrong_url.url = "https://tracks.monochrome.st/track/12345678901234568".into();
        assert!(!exact_monochrome_locator_match(&locator, &wrong_url));
    }

    #[test]
    fn recognition_suppression_requires_successful_accepted_locator_isrc_backfill() {
        let mut source = track("Artist", "Title", Some(180.0));
        source.provenance = json!({"acquisition":{
            "provider":"monochrome",
            "id":"monochrome:12345678901234567",
            "url":"https://tracks.monochrome.st/track/12345678901234567"
        }});
        assert!(!accepted_acquisition_isrc_backfill(&source));

        source.isrc = Some("US-ABC-26-00001".into());
        source.provenance["autotag"] = json!({
            "source":"accepted_acquisition_catalog_metadata",
            "identityStatus":"accepted_recording_match",
            "provider":"monochrome",
            "providerId":"12345678901234567",
            "sourceUrl":"https://tracks.monochrome.st/track/12345678901234567",
            "relationship":"exact",
            "fields":["isrc"]
        });
        assert!(accepted_acquisition_isrc_backfill(&source));
        source.provenance["autotag"]["sourceUrl"] =
            json!("https://tracks.monochrome.st/track/12345678901234568");
        assert!(!accepted_acquisition_isrc_backfill(&source));
    }

    #[test]
    fn recognition_suppression_keeps_historical_verified_isrc_marker_compatible() {
        let mut source = track("Artist", "Title", Some(180.0));
        source.isrc = Some("US-ABC-26-00001".into());
        source.provenance = json!({
            "acquisition":{
                "provider":"monochrome",
                "id":"monochrome:12345678901234567",
                "url":"https://tracks.monochrome.st/track/12345678901234567"
            },
            "autotag":{
                "source":"accepted_acquisition_catalog_isrc",
                "identityStatus":"accepted_recording_match",
                "provider":"monochrome",
                "providerId":"12345678901234567",
                "sourceUrl":"https://tracks.monochrome.st/track/12345678901234567",
                "relationship":"exact",
                "fields":["isrc"]
            }
        });
        assert!(accepted_acquisition_isrc_backfill(&source));

        source.provenance["autotag"]["fields"] = json!([]);
        assert!(!accepted_acquisition_isrc_backfill(&source));
        source.provenance["autotag"]["fields"] = json!(["isrc"]);
        source.provenance["autotag"]["sourceUrl"] =
            json!("https://tracks.monochrome.st/track/12345678901234568");
        assert!(!accepted_acquisition_isrc_backfill(&source));
    }

    #[test]
    fn album_only_acquisition_backfill_counts_as_applied_without_suppressing_fingerprinting() {
        let mut source = track("Artist", "Title", Some(180.0));
        source.album = Some("Verified Release".into());
        source.provenance = json!({
            "acquisition":{
                "provider":"monochrome",
                "id":"monochrome:12345678901234567",
                "url":"https://tracks.monochrome.st/track/12345678901234567"
            },
            "autotag":{
                "source":"accepted_acquisition_catalog_metadata",
                "identityStatus":"accepted_recording_match",
                "provider":"monochrome",
                "providerId":"12345678901234567",
                "sourceUrl":"https://tracks.monochrome.st/track/12345678901234567",
                "relationship":"exact",
                "fields":["album"]
            }
        });

        assert!(accepted_acquisition_catalog_metadata_backfill(&source));
        assert!(!accepted_acquisition_isrc_backfill(&source));

        source.album = None;
        assert!(!accepted_acquisition_catalog_metadata_backfill(&source));
    }

    #[test]
    fn monochrome_backfill_uses_requested_identity_for_alternative_source_assets() {
        let mut source = track("Monochrome Credit", "Catalog Title", Some(180.0));
        source.id = "monochrome:12345678901234567".into();
        source.provider = "monochrome".into();
        source.provider_id = "12345678901234567".into();
        source.provenance = json!({"requestedTrackIdentity":{
            "id":"spotify:requested-id", "originalId":"spotify:requested-id",
            "provider":"spotify", "providerId":"requested-id",
            "artist":"Requested Artist", "title":"Requested Title"
        }});
        let requested = requested_identity_for_catalog_backfill(&source).unwrap();
        assert_eq!(requested.id, "spotify:requested-id");
        assert_eq!(requested.artist, "Requested Artist");
        assert_eq!(requested.title, "Requested Title");
    }

    #[test]
    fn malformed_requested_identity_never_falls_back_to_alternative_source_identity() {
        let mut source = track("Alternative Artist", "Alternative Title", Some(180.0));
        source.provenance = json!({"requestedTrackIdentity":{"id":"spotify:missing-fields"}});
        assert!(requested_identity_for_catalog_backfill(&source).is_none());
    }

    #[test]
    fn accepted_backfill_requires_shared_accept_and_exact_or_probable_relationship() {
        let candidate = crate::models::Candidate {
            permission: "unknown".into(),
            ..crate::models::Candidate::default()
        };
        let mut assessment = crate::recording_match::MatchAssessment {
            action: crate::recording_match::MatchAction::Accept,
            relationship: crate::recording_match::MatchRelationship::Exact,
            proof_strength: crate::recording_match::ProofStrength::MetadataOnly,
            evidence: None,
            reasons: vec![],
            dimensions: crate::recording_match::MatchDimensions {
                title: crate::recording_match::Comparison::Agree,
                artists: crate::recording_match::Comparison::Agree,
                guests: crate::recording_match::Comparison::Unknown,
                version: crate::recording_match::Comparison::Unknown,
                runtime: crate::recording_match::Comparison::Agree,
                isrc: crate::recording_match::Comparison::Unknown,
                work: crate::recording_match::Comparison::Unknown,
            },
        };
        assert!(accepted_acquisition_catalog_match(&candidate, &assessment));
        assessment.relationship = crate::recording_match::MatchRelationship::Alternative;
        assert!(!accepted_acquisition_catalog_match(&candidate, &assessment));
        assessment.relationship = crate::recording_match::MatchRelationship::Probable;
        assessment.action = crate::recording_match::MatchAction::Inspect;
        assert!(!accepted_acquisition_catalog_match(&candidate, &assessment));
        assessment.action = crate::recording_match::MatchAction::Accept;
        let mut restricted = candidate;
        restricted.permission = "restricted".into();
        assert!(!accepted_acquisition_catalog_match(
            &restricted,
            &assessment
        ));
    }

    #[test]
    fn strong_catalog_match_requires_exact_identity_and_duration() {
        let src = track("The Example", "Blue Sky", Some(200.0));
        assert!(strong_catalog_match(
            &src,
            &candidate("The Example", "Blue Sky", Some(205.0))
        ));
        assert!(!strong_catalog_match(
            &src,
            &candidate("The Example", "Blue Skies", Some(200.0))
        ));
        assert!(!strong_catalog_match(
            &src,
            &candidate("The Example", "Blue Sky", None)
        ));
        assert!(!strong_catalog_match(
            &track("The Example", "Blue Sky", None),
            &candidate("The Example", "Blue Sky", Some(200.0))
        ));
        assert!(!duration_compatible(200.0, 212.0));
    }

    #[test]
    fn named_remix_identity_must_match_for_automatic_application() {
        let mut source = track("DJ A", "Night (Club Mix)", Some(200.0));
        source.version = Some("Club Mix".into());
        let mut remix = candidate("DJ A", "Night (Dub Mix)", Some(200.0));
        remix.fields.version = Some("Dub Mix".into());
        remix.title = "Night (Dub Mix)".into();
        assert!(!strong_catalog_match(&source, &remix));
        assert!(!can_auto_apply(&source, &remix, false));
    }

    #[test]
    fn high_fuzzy_score_does_not_override_exact_match_gate() {
        let source = track("Artist", "Hello World", Some(180.0));
        let near = candidate("Artist", "Hello Worlds", Some(180.0));
        assert!(near.score >= 0.94);
        assert!(!can_auto_apply(&source, &near, false));
    }

    #[test]
    fn unapproved_reviews_can_retry_without_resetting_approved_or_active_jobs() {
        let mut job = AutoTagJob {
            asset_id: "asset".into(),
            input_hash: "hash".into(),
            version: VERSION.into(),
            status: "review".into(),
            attempts: 0,
            next_attempt_at: None,
            message: None,
            candidates: vec![],
            chosen: None,
            recognition: None,
            approved_fields: vec![],
            excerpt_only: false,
            updated_at: String::new(),
        };
        assert!(autotag_job_retryable(&job));
        job.approved_fields.push("artist".into());
        assert!(!autotag_job_retryable(&job));
        job.approved_fields.clear();
        for status in ["approved", "matching", "queued", "applied"] {
            job.status = status.into();
            assert!(!autotag_job_retryable(&job));
        }
        for status in ["failed", "no_match", "retry"] {
            job.status = status.into();
            assert!(autotag_job_retryable(&job));
        }
    }

    #[test]
    fn source_reported_identity_can_find_catalog_candidates_but_requires_review() {
        let mut source = track("", "La Cubanita", Some(190.0));
        source.id = "youtube:video-123".into();
        source.provider = "youtube".into();
        source.provider_id = "video-123".into();
        source.source_url = "https://youtube.com/watch?v=video-123".into();
        let audio_candidate = crate::models::Candidate {
            id: source.id.clone(),
            provider: source.provider.clone(),
            url: source.source_url.clone(),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "resolvedProviderId":"video-123",
                "artists":["Los Niños de Sara"],
                "catalogTitle":"La Cubanita"
            })),
            ..crate::models::Candidate::default()
        };
        source.provenance = json!({"audioCandidate":audio_candidate});
        let (search_source, fields) = crate::acquisition::source_reported_track_for_lookup(&source);
        assert_eq!(search_source.artist, "Los Niños de Sara");
        assert_eq!(search_source.title, "La Cubanita");
        assert_eq!(fields, vec!["artist".to_owned()]);

        let mut catalog_match = candidate("Los Niños de Sara", "La Cubanita", Some(190.0));
        catalog_match.fields.provenance = search_source.provenance.clone();
        assert!(!can_auto_apply(&search_source, &catalog_match, false));
        catalog_match
            .evidence
            .push("SongRec recognized two non-overlapping excerpts consistently; MusicBrainz artist, title, and duration also matched.".into());
        assert!(can_auto_apply(&search_source, &catalog_match, false));
        let historical_source = track("", "La Cubanita", Some(190.0));
        let mut historical_match = catalog_match.clone();
        historical_match.evidence.push(
            "Catalog lookup used exact-source reported field(s): artist; source metadata alone does not verify recording identity.".into(),
        );
        assert!(can_auto_apply(&historical_source, &historical_match, false));

        let mut manually_identified = search_source.clone();
        manually_identified.artist = "Manually Chosen Artist".into();
        manually_identified.provenance["manualFields"]["artist"] = json!(true);
        assert!(!can_auto_apply(&manually_identified, &catalog_match, false));

        catalog_match.evidence[0] = "AcoustID audio fingerprint score 0.940".into();
        catalog_match.evidence.truncate(1);
        assert!(!can_auto_apply(&search_source, &catalog_match, false));
        assert!(can_auto_apply(
            &track("Los Niños de Sara", "La Cubanita", Some(190.0)),
            &catalog_match,
            false
        ));
    }

    #[test]
    fn source_reported_repair_only_fills_missing_or_weak_nonmanual_fields() {
        let source = track("", "La Cubanita", Some(190.0));
        let reported = Track {
            artist: "Los Niños de Sara".into(),
            title: "La Cubanita".into(),
            album: Some("Gipsyolé".into()),
            ..Track::default()
        };
        assert!(source_field_needs_repair(&source, &reported, "artist"));
        assert!(!source_field_needs_repair(&source, &reported, "title"));
        assert!(source_field_needs_repair(&source, &reported, "album"));

        let mut protected = source.clone();
        protected.provenance = json!({"manualFields":{"artist":true}});
        assert!(!source_field_needs_repair(&protected, &reported, "artist"));
        protected.album = Some("Existing Album".into());
        assert!(!source_field_needs_repair(&protected, &reported, "album"));
    }

    fn completed_source_fixture() -> (LocalAsset, DownloadJob) {
        let candidate = Candidate {
            id: "youtube:video-123".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=video-123".into(),
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "resolvedProviderId":"video-123",
                "artist":"Los Niños de Sara",
                "album":"Gipsyolé",
                "catalogTitle":"La Cubanita"
            })),
            ..Candidate::default()
        };
        let mut track = Track {
            id: "youtube:video-123".into(),
            provider: "youtube".into(),
            provider_id: "video-123".into(),
            source_url: candidate.url.clone(),
            title: "La Cubanita".into(),
            ..Track::default()
        };
        track.provenance = json!({
            "publicationJobId":"job-1",
            "acquisition":{
                "provider":candidate.provider.clone(),
                "id":candidate.id.clone(),
                "url":candidate.url.clone()
            },
            "audioCandidate":{"provider":"youtube","id":"youtube:video-123"}
        });
        let asset = LocalAsset {
            id: "asset-1".into(),
            recording_id: "recording-1".into(),
            path: "/music/La Cubanita.mp3".into(),
            hash: "hash-1".into(),
            source_codec: "opus".into(),
            final_codec: "mp3".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: Some(190.0),
            size_bytes: None,
            bitrate_origin: None,
            processing: "managed".into(),
            track,
        };
        let job = DownloadJob {
            id: "job-1".into(),
            track: asset.track.clone(),
            candidate,
            state: "completed".into(),
            progress: 100.0,
            error: None,
            output_path: Some(asset.path.clone()),
            created_at: "2026-10-21T00:00:00Z".into(),
            output_profile: "source".into(),
            disposition: Default::default(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
            download_layout: Default::default(),
            playlist_context: None,
        };
        (asset, job)
    }

    #[test]
    fn completed_source_metadata_requires_job_path_and_exact_acquisition_binding() {
        let (asset, job) = completed_source_fixture();
        assert!(completed_source_candidate(&asset, &job).is_some());

        let mut wrong_job = job.clone();
        wrong_job.id = "other-job".into();
        assert!(completed_source_candidate(&asset, &wrong_job).is_none());

        let mut wrong_path = job.clone();
        wrong_path.output_path = Some("/music/other.mp3".into());
        assert!(completed_source_candidate(&asset, &wrong_path).is_none());

        let mut incomplete = job.clone();
        incomplete.state = "failed".into();
        assert!(completed_source_candidate(&asset, &incomplete).is_none());

        let mut wrong_provider = job.clone();
        wrong_provider.candidate.provider = "soundcloud".into();
        assert!(completed_source_candidate(&asset, &wrong_provider).is_none());

        let mut wrong_id = job.clone();
        wrong_id.candidate.id = "youtube:other".into();
        assert!(completed_source_candidate(&asset, &wrong_id).is_none());

        let mut wrong_url = job;
        wrong_url.candidate.url = "https://www.youtube.com/watch?v=other".into();
        assert!(completed_source_candidate(&asset, &wrong_url).is_none());
    }

    #[test]
    fn completed_source_promotion_keeps_manual_fields_and_reports_only_unverified_missing_tags() {
        let (mut asset, job) = completed_source_fixture();
        asset.track.provenance["manualFields"]["artist"] = json!(true);
        asset.track.artist = "User chosen artist".into();
        let candidate = completed_source_candidate(&asset, &job).unwrap();
        assert!(has_bound_yt_dlp_metadata(&asset.track, candidate));
        let (reported, fields) =
            crate::acquisition::source_reported_track_for_lookup_from_candidate(
                &asset.track,
                candidate,
            );
        assert_eq!(reported.artist, "User chosen artist");
        assert_eq!(reported.album.as_deref(), Some("Gipsyolé"));
        assert_eq!(fields, vec!["album".to_owned()]);
        let fields = fields
            .into_iter()
            .filter(|field| source_field_needs_repair(&asset.track, &reported, field))
            .collect::<Vec<_>>();
        assert_eq!(fields, vec!["album".to_owned()]);
        assert_eq!(
            reported.provenance["sourceReportedMetadata"]["identityStatus"],
            "source_reported_unverified"
        );
    }

    fn synthetic_songrec_metadata() -> crate::radio::RadioTrackMetadata {
        crate::radio::RadioTrackMetadata {
            identifiers: Some(crate::recording_identity::RecognitionIdentifiers {
                isrc: Some("USABC2600001".into()),
                shazam_id: Some("12345".into()),
                apple_music_url: Some("https://music.apple.com/us/song/12345".into()),
            }),
            station_id: "synthetic".into(),
            artist: "Synthetic artist".into(),
            title: "Synthetic title".into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: String::new(),
            observed_at: None,
            metadata_confidence: "synthetic fixture".into(),
        }
    }

    #[test]
    fn identifier_agreement_requires_observation_in_both_excerpts() {
        let first = synthetic_songrec_metadata();
        let mut second = first.clone();
        second.identifiers.as_mut().unwrap().isrc = Some("US-ABC-26-00001".into());
        let agreed =
            consistent_songrec_excerpts(Some(first.clone()), Some(second.clone())).unwrap();
        assert_eq!(
            agreed.agreeing_identifiers.isrc.as_deref(),
            Some("USABC2600001")
        );
        assert_eq!(
            agreed.agreeing_identifiers.shazam_id.as_deref(),
            Some("12345")
        );
        second.identifiers = None;
        let single = consistent_songrec_excerpts(Some(first), Some(second)).unwrap();
        assert_eq!(single.isrc.as_deref(), Some("USABC2600001"));
        assert_eq!(single.agreeing_identifiers, Default::default());
        let mut old = serde_json::to_value(single).unwrap();
        old.as_object_mut().unwrap().remove("agreeingIdentifiers");
        let legacy: AudioRecognition = serde_json::from_value(old).unwrap();
        assert_eq!(legacy.excerpt_count, 2);
        assert_eq!(legacy.agreeing_identifiers, Default::default());
    }

    #[test]
    fn two_excerpt_agreement_is_available_without_catalog_and_rejects_conflicts() {
        let first = synthetic_songrec_metadata();
        let result = consistent_songrec_excerpts(Some(first.clone()), Some(first.clone())).unwrap();
        assert_eq!(result.excerpt_count, 2);
        assert_eq!(result.isrc.as_deref(), Some("USABC2600001"));
        assert!(consistent_songrec_excerpts(Some(first.clone()), None).is_none());
        for field in ["artist", "title", "isrc", "shazam", "apple"] {
            let mut second = first.clone();
            match field {
                "artist" => second.artist = "Other artist".into(),
                "title" => second.title = "Other title".into(),
                "isrc" => second.identifiers.as_mut().unwrap().isrc = Some("USABC2600002".into()),
                "shazam" => second.identifiers.as_mut().unwrap().shazam_id = Some("67890".into()),
                _ => {
                    second.identifiers.as_mut().unwrap().apple_music_url =
                        Some("https://music.apple.com/us/song/67890".into())
                }
            }
            assert!(
                consistent_songrec_excerpts(Some(first.clone()), Some(second)).is_none(),
                "{field}"
            );
        }
        let mut blank = first;
        blank.title.clear();
        assert!(consistent_songrec_excerpts(Some(blank.clone()), Some(blank)).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_excerpt_extraction_removes_owned_temporary_audio() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let marker = fixture.path().join("output-path");
        let script = fixture.path().join("fake-ffmpeg");
        std::fs::write(&script, format!("#!/bin/sh\nfor output do :; done\nprintf %s \"$output\" > '{}'\nprintf fixture > \"$output\"\nexec sleep 30\n", marker.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let task = tokio::spawn(async move {
            songrec_excerpt("unused-synthetic-input", 5.0, script.to_str().unwrap()).await
        });
        let mut output = None;
        for _ in 0..100 {
            if let Ok(path) = std::fs::read_to_string(&marker) {
                if !path.is_empty() && std::path::Path::new(&path).is_file() {
                    output = Some(std::path::PathBuf::from(path));
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let wav = output.expect("Synthetic extractor did not produce its fixture");
        let directory = wav.parent().unwrap().to_owned();
        assert!(
            !directory.exists(),
            "Cancelled extraction retained temporary material"
        );
    }

    #[test]
    fn catalog_outage_allows_recognition_fallback_but_does_not_hide_total_failure() {
        let mut errors = Vec::new();
        let rows: Value = catalog_result_or_empty(Err("catalog offline".into()), &mut errors);
        assert!(rows.is_null());
        let recognized = AudioRecognition {
            provider: "songrec".into(),
            artist: "Synthetic artist".into(),
            title: "Synthetic title".into(),
            isrc: None,
            shazam_id: None,
            excerpt_count: 2,
            agreeing_identifiers: Default::default(),
        };
        let result =
            finish_autotag_lookup(vec![], Some(recognized.clone()), errors.clone()).unwrap();
        assert_eq!(result.recognition, Some(recognized));
        assert!(result.candidates.is_empty());
        assert!(
            matches!(finish_autotag_lookup(vec![], None, errors), Err(error) if error == "catalog offline")
        );
        assert!(finish_autotag_lookup(vec![], None, vec![]).is_ok());
        let source = track("Synthetic artist", "Synthetic title", Some(180.0));
        let mut partial = candidate("Synthetic artist", "Synthetic title", Some(180.0));
        assert!(can_auto_apply(&source, &partial, false));
        mark_release_context_unavailable(&mut partial);
        assert!(!can_auto_apply(&source, &partial, false));
    }

    #[test]
    fn successful_songrec_recognition_survives_catalog_outage_without_catalog_binding() {
        let source = track(
            "Synthetic source artist",
            "Synthetic source title",
            Some(180.0),
        );
        let recognition = AudioRecognition {
            provider: "songrec".into(),
            artist: "Synthetic recognized artist".into(),
            title: "Synthetic recognized title".into(),
            isrc: Some("USABC2600001".into()),
            shazam_id: Some("synthetic-recognition".into()),
            excerpt_count: 2,
            agreeing_identifiers: Default::default(),
        };
        for rows in [
            Err("MusicBrainz temporarily unavailable".into()),
            Ok(json!({"recordings":[]})),
        ] {
            let outcome = songrec_catalog_outcome(&source, recognition.clone(), rows);
            assert_eq!(outcome.recognition, recognition);
            assert!(outcome.candidate.is_none());
            assert_eq!(source.artist, "Synthetic source artist");
        }
    }

    #[test]
    fn songrec_isrc_query_filters_exact_codes_without_choosing_ambiguous_rows() {
        let results = json!({"recordings":[
            {"id":"mb-a","isrcs":["USABC2600001"]},
            {"id":"mb-b","isrcs":["USABC2600002"]},
            {"id":"mb-c","isrcs":["USABC2600001"]},
            {"id":"mb-a","isrcs":["USABC2600001"]}
        ]});
        let exact = unique_musicbrainz_rows_for_isrc(&results, "US-ABC-26-00001");
        assert_eq!(exact.len(), 2);
        assert_eq!(exact[0]["id"], "mb-a");
        assert_eq!(exact[1]["id"], "mb-c");
        assert!(unique_musicbrainz_rows_for_isrc(&results, "USABC2600003").is_empty());
    }

    #[test]
    fn first_release_date_is_not_a_recording_date() {
        let row = json!({
            "id":"mb-recording", "title":"Song", "length":180000,
            "first-release-date":"1967-02-01",
            "artist-credit":[{"name":"Artist","artist":{"name":"Artist"}}],
            "isrcs":["USABC6700001"]
        });
        let source = track("Artist", "Song", Some(180.0));
        let result = candidate_from_mb(&row, &source).unwrap();
        assert_eq!(result.fields.isrc.as_deref(), Some("USABC6700001"));
        assert_eq!(result.fields.recording_date, None);
        assert_eq!(result.fields.release_date, None);
    }

    #[test]
    fn excerpt_candidate_cannot_carry_full_recording_metadata() {
        let mut result = candidate("Artist", "Chapter Song", Some(240.0));
        result.fields.album = Some("Studio LP".into());
        result.fields.album_artist = Some("Various".into());
        result.fields.release_date = Some("2020-01-01".into());
        result.fields.recording_date = Some("2019".into());
        result.fields.isrc = Some("USABC0000001".into());
        result.fields.label = Some("Label".into());
        result.fields.catalog_number = Some("CAT-1".into());
        result.fields.track_number = Some(1);
        result.fields.disc_number = Some(1);
        result.fields.genre = Some("house".into());
        restrict_excerpt_fields(&mut result);
        assert!(result.fields.album.is_none() && result.fields.album_artist.is_none());
        assert!(result.fields.release_date.is_none() && result.fields.recording_date.is_none());
        assert!(result.fields.isrc.is_none() && result.fields.label.is_none());
        assert!(result.fields.catalog_number.is_none() && result.fields.track_number.is_none());
        assert!(result.fields.disc_number.is_none() && result.fields.duration_seconds.is_none());
        assert_eq!(result.fields.genre.as_deref(), Some("house"));
    }

    #[test]
    fn raw_fingerprint_threshold_and_source_conflicts_force_review() {
        let source = track("Artist", "Song", Some(180.0));
        let mut fingerprint = candidate("Artist", "Song", Some(180.0));
        fingerprint
            .evidence
            .push("AcoustID audio fingerprint score 0.940".into());
        assert!(!can_auto_apply(&source, &fingerprint, true));
        fingerprint.evidence[0] = "AcoustID audio fingerprint score 0.960".into();
        assert!(can_auto_apply(&source, &fingerprint, true));
        fingerprint.artist = "Different Artist".into();
        assert!(!can_auto_apply(&source, &fingerprint, true));
    }

    #[test]
    fn provider_errors_are_retryable_and_leave_source_metadata_untouched() {
        let source = track("Artist", "Song", Some(180.0));
        let mut job = AutoTagJob {
            asset_id: "asset".into(),
            input_hash: "hash".into(),
            version: VERSION.into(),
            status: "matching".into(),
            attempts: 0,
            next_attempt_at: None,
            message: None,
            candidates: vec![],
            chosen: None,
            recognition: None,
            approved_fields: vec![],
            excerpt_only: false,
            updated_at: String::new(),
        };
        mark_retry(&mut job, "AcoustID unavailable");
        assert_eq!(job.status, "retry");
        assert_eq!(job.attempts, 1);
        assert!(job.next_attempt_at.is_some());
        assert_eq!(source.artist, "Artist");
        assert_eq!(source.title, "Song");
        assert!(source.album.is_none() && source.isrc.is_none());
        while job.attempts < MAX_PROVIDER_ATTEMPTS {
            mark_retry(&mut job, "provider remains unavailable");
        }
        assert_eq!(job.status, "failed");
        assert!(job.next_attempt_at.is_none());
    }

    #[test]
    fn records_ignore_superseded_hash_and_deduplicate_current_rows() {
        let old = persisted_job("old-hash", "applied");
        let current = persisted_job("new-hash", "applied");
        let duplicate = persisted_job("new-hash", "applied");
        let records = decode_job_records(vec![
            json!({"version":VERSION,"status":"completed","payload":current}),
            json!({"version":VERSION,"status":"superseded","payload":old}),
            json!({"version":VERSION,"status":"completed","payload":duplicate}),
        ]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_hash, "new-hash");
    }
}
