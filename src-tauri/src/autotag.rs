//! Conservative catalog matching for newly downloaded managed assets.
//!
//! Queue records use the dedicated durable autotag table and are enqueued only
//! after a newly downloaded asset has been committed to the managed library.
use crate::{
    artists::ArtistService,
    media,
    models::{LocalAsset, Settings, Track},
    Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, env, fs, process::Stdio, sync::Arc, time::Duration};
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
    pub chosen: Option<AutoTagCandidate>,
    #[serde(default)]
    pub approved_fields: Vec<String>,
    #[serde(default)]
    pub excerpt_only: bool,
    pub updated_at: String,
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
    songrec_gate: Arc<Mutex<()>>,
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
            songrec_gate: Arc::new(Mutex::new(())),
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
        let current_hash = self
            .store
            .get_asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .map(|asset| asset.hash)
            .ok_or("Autotag asset was removed")?;
        let mut job = self
            .records()?
            .into_iter()
            .find(|job| job.asset_id == asset_id && job.input_hash == current_hash)
            .ok_or("Autotag job not found")?;
        if !["retry", "no_match", "failed"].contains(&job.status.as_str()) {
            return Err("Only failed or unmatched autotag jobs can be retried".into());
        }
        job.status = "queued".into();
        job.attempts = 0;
        job.next_attempt_at = None;
        job.message = None;
        job.updated_at = chrono::Utc::now().to_rfc3339();
        self.persist(&job)?;
        Ok(self.schedule(asset_id))
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
        let excerpt_only = is_excerpt(&asset.track);
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
                &asset.track,
                &asset.path,
                acoustid_key.as_deref(),
                credentials,
            )
            .await
        {
            Ok(candidates) => {
                job.candidates = candidates.clone();
                if candidates.len() == 1
                    && can_auto_apply(&asset.track, &candidates[0], excerpt_only)
                {
                    job.chosen = Some(candidates[0].clone());
                    job.status = "ready".into();
                    job.approved_fields = strong_field_keys(&candidates[0].fields);
                    let chosen = candidates[0].clone();
                    let latest = self
                        .store
                        .get_asset_by_id(&asset.id)
                        .map_err(|e| e.to_string())?
                        .ok_or("Autotag asset was removed")?;
                    if latest.hash != asset.hash {
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
                            &asset.hash,
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
                    job.status = if candidates.is_empty() {
                        "no_match"
                    } else {
                        "review"
                    }
                    .into();
                    job.message = Some(
                        if candidates.is_empty() {
                            "No sufficiently compatible catalog match was found."
                        } else {
                            "Catalog candidates are ambiguous; select one in review."
                        }
                        .into(),
                    );
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

    async fn lookup(
        &self,
        source: &Track,
        path: &str,
        acoustid_key: Option<&str>,
        credentials: &crate::artists::ArtistCredentials,
    ) -> Result<Vec<AutoTagCandidate>, String> {
        let excerpt_only = is_excerpt(source);
        if excerpt_only {
            let mut candidates = Vec::new();
            if let Some(key) = acoustid_key.filter(|key| !key.trim().is_empty()) {
                if let Some(candidate) = self.acoustid_match(source, path, key).await? {
                    candidates.push(candidate);
                }
            }
            if candidates.is_empty() && self.settings()?.shazam_fallback {
                if let Some(candidate) = self.songrec_match(source, path).await? {
                    candidates.push(candidate);
                }
            }
            for candidate in &mut candidates {
                restrict_excerpt_fields(candidate);
            }
            return Ok(candidates);
        }
        let results = self.artists.recording_search(source).await?;
        let rows = results["recordings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut matches = rows
            .iter()
            .filter_map(|row| candidate_from_mb(row, source))
            .collect::<Vec<_>>();
        matches.sort_by(|a, b| b.score.total_cmp(&a.score));
        matches.truncate(10);
        if source
            .album
            .as_deref()
            .is_some_and(|album| !album.trim().is_empty())
        {
            for candidate in matches.iter_mut().take(3) {
                self.enrich_release_context(candidate, source).await?;
            }
        }
        if matches.is_empty() {
            matches.extend(self.discogs_match(source, credentials).await?);
        }
        if matches.is_empty() {
            if let Some(key) = acoustid_key.filter(|key| !key.trim().is_empty()) {
                if let Some(candidate) = self.acoustid_match(source, path, key).await? {
                    matches.push(candidate);
                }
            }
            if matches.is_empty() && self.settings()?.shazam_fallback {
                if let Some(candidate) = self.songrec_match(source, path).await? {
                    matches.push(candidate);
                }
            }
        }
        if source
            .album
            .as_deref()
            .is_some_and(|album| !album.trim().is_empty())
        {
            for candidate in matches.iter_mut().take(3) {
                if candidate.fields.provenance["releaseContextAttempted"] != true {
                    self.enrich_release_context(candidate, source).await?;
                }
            }
        }
        Ok(matches)
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

    async fn songrec_match(
        &self,
        source: &Track,
        path: &str,
    ) -> Result<Option<AutoTagCandidate>, String> {
        if !executable_available("songrec") && !executable_available("SongRec") {
            return Ok(None);
        }
        let duration = source.duration_seconds.unwrap_or(0.0);
        if !duration.is_finite() || duration < 50.0 {
            return Ok(None);
        }
        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let _songrec_permit = self.songrec_gate.lock().await;
        let first = songrec_excerpt(path, 5.0, &settings.ffmpeg_path).await?;
        tokio::time::sleep(Duration::from_secs(10)).await;
        let second = songrec_excerpt(path, duration - 25.0, &settings.ffmpeg_path).await?;
        let (Some((artist_a, title_a)), Some((artist_b, title_b))) = (first, second) else {
            return Ok(None);
        };
        if normalize(&artist_a) != normalize(&artist_b)
            || normalize(&title_a) != normalize(&title_b)
        {
            return Ok(None);
        }
        let identified = Track {
            artist: artist_a,
            title: title_a,
            duration_seconds: source.duration_seconds,
            ..Track::default()
        };
        let rows = self.artists.recording_search(&identified).await?;
        let mut candidates = rows["recordings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| candidate_from_mb(row, &identified))
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        if candidates.len() != 1 || candidates[0].score < 0.96 {
            return Ok(None);
        }
        let mut candidate = candidates.remove(0);
        candidate.evidence.push("SongRec recognized two non-overlapping excerpts consistently; MusicBrainz artist, title, and duration also matched.".into());
        if source_conflicts(source, &candidate) {
            candidate.evidence.push(
                "Recognized artist or title conflicts with the downloaded source; review required."
                    .into(),
            );
        }
        Ok(Some(candidate))
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

    pub async fn retry_due(&self, job: &AutoTagJob) -> bool {
        let Some(next) = &job.next_attempt_at else {
            return true;
        };
        chrono::DateTime::parse_from_rfc3339(next)
            .map(|at| at <= chrono::Utc::now())
            .unwrap_or(true)
    }
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
    if source_conflicts(source, candidate) {
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
    (candidate.provider == "musicbrainz" || candidate.provider == "discogs")
        && candidate.score >= 0.96
        && strong_catalog_match(source, candidate)
}

fn source_conflicts(source: &Track, candidate: &AutoTagCandidate) -> bool {
    (!source.artist.trim().is_empty() && normalize(&source.artist) != normalize(&candidate.artist))
        || (!source.title.trim().is_empty()
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

async fn songrec_excerpt(
    path: &str,
    start: f64,
    ffmpeg_path: &str,
) -> Result<Option<(String, String)>, String> {
    let wav = env::temp_dir().join(format!("burrata-autotag-{}.wav", uuid::Uuid::new_v4()));
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
        let _ = fs::remove_file(&wav);
        return Ok(None);
    }
    let binary = if executable_available("songrec") {
        "songrec"
    } else {
        "SongRec"
    };
    let mut command = tokio::process::Command::new(crate::tools::executable(binary, binary));
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .arg("audio-file-to-recognized-song")
        .arg(&wav)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(45), command.output()).await;
    let _ = fs::remove_file(&wav);
    let Ok(Ok(output)) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let value: Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let track = value.get("track").unwrap_or(&value);
    let title = track["title"]
        .as_str()
        .or_else(|| track["heading"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let artist = track["subtitle"]
        .as_str()
        .or_else(|| track["artist"].as_str())
        .or_else(|| {
            track["artists"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|a| a["name"].as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Ok(match (artist, title) {
        (Some(artist), Some(title)) => Some((artist.to_owned(), title.to_owned())),
        _ => None,
    })
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
            approved_fields: vec![],
            excerpt_only: false,
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
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
