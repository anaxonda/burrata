//! Durable, bounded Soulseek requests. No transfer slot is held while searching.
//! Only accepted recording matches may enqueue; uncertain results require review.
use super::*;
use serde::Deserialize;
use tauri::Manager;

pub(super) const KEY: &str = "soulseekDownloadWatches.v1";
const LIMIT: usize = 500;
const HOUR: i64 = 3_600;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadWatchRequest {
    pub track: Track,
    pub output_profile: String,
    #[serde(default)]
    pub playlist_context: Option<DownloadPlaylistContext>,
    #[serde(default)]
    pub requires_lossless: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadWatch {
    pub id: String,
    pub request: DownloadWatchRequest,
    pub kind: String,
    pub state: String,
    pub next_check: i64,
    pub last_checked: Option<i64>,
    pub revision: u64,
    pub job_id: Option<String>,
    pub target_asset_id: Option<String>,
    pub expected_hash: Option<String>,
    #[serde(default)]
    failures: u32,
    music_directory: String,
    layout: DownloadLayout,
    matching_level: crate::recording_match::MatchLevel,
}
fn active(item: &DownloadWatch) -> bool {
    !matches!(item.state.as_str(), "completed" | "cancelled")
}
fn next_delay(failures: u32) -> i64 {
    HOUR * (1_i64 << failures.min(3))
}
fn lossy(asset: &LocalAsset) -> bool {
    matches!(
        asset.final_codec.to_ascii_lowercase().as_str(),
        "mp3" | "aac" | "opus" | "vorbis" | "mp4a" | "mp4a.40.2"
    )
}
impl AcquisitionManager {
    pub fn list_download_watches(&self) -> Result<Vec<DownloadWatch>, String> {
        self.inner
            .store
            .get_json_preference(KEY)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "Download watch storage is invalid".to_owned())
            .map(Option::unwrap_or_default)
    }
    fn save_download_watches(&self, items: &[DownloadWatch]) -> Result<(), String> {
        let value = serde_json::to_value(items).map_err(|e| e.to_string())?;
        if self.inner.store.get_json_preference(KEY)?.as_ref() == Some(&value) {
            return Ok(());
        }
        self.inner.store.set_json_preference(KEY, &value)?;
        if let Some(app) = &self.inner.app {
            let _ = app.emit("download-watches-changed", ());
        }
        Ok(())
    }
    pub async fn queue_download_watch(
        &self,
        request: DownloadWatchRequest,
        settings: &Settings,
    ) -> Result<DownloadWatch, String> {
        if !settings.soulseek_wait_for_source {
            return Err("Enable waiting for Soulseek in Downloads settings first".into());
        }
        self.add_download_watch(request, settings, None).await
    }
    async fn add_download_watch(
        &self,
        mut request: DownloadWatchRequest,
        settings: &Settings,
        asset: Option<&LocalAsset>,
    ) -> Result<DownloadWatch, String> {
        let _lease = self.inner.store.try_library_write()?;
        if settings.music_directory.trim().is_empty()
            || !matches!(request.output_profile.as_str(), "source" | "mp3")
        {
            return Err("Configure a download destination and valid output profile first".into());
        }
        if request.track.id.trim().is_empty()
            || request.track.title.trim().is_empty()
            || request.track.artist.trim().is_empty()
            || request.track.playback_range.is_some()
        {
            return Err("Soulseek waiting needs an identified whole recording".into());
        }
        if request.requires_lossless && request.output_profile == "mp3" {
            return Err("Lossless waiting requires source output".into());
        }
        let _gate = self.inner.watch_gate.lock().await;
        let mut items = self.list_download_watches()?;
        let kind = if asset.is_some() {
            "upgrade"
        } else {
            "waiting"
        };
        if let Some(existing) = items.iter().find(|item| {
            active(item)
                && item.request.track.id == request.track.id
                && item.kind == kind
                && item.request.output_profile == request.output_profile
                && item.request.playlist_context == request.playlist_context
                && item.request.requires_lossless == request.requires_lossless
                && item.target_asset_id.as_deref() == asset.map(|a| a.id.as_str())
        }) {
            return Ok(existing.clone());
        }
        items.retain(active);
        if items.len() >= LIMIT {
            return Err("Soulseek waiting list is full; cancel a request first".into());
        }
        if !request.track.provenance.is_object() {
            request.track.provenance = serde_json::json!({});
        }
        request.track.provenance["downloadMatchingLevel"] =
            serde_json::to_value(settings.matching_level).map_err(|e| e.to_string())?;
        // A saved source choice cannot authorize an unrelated future peer file.
        for key in [
            "userSelectedSource",
            "audioCandidate",
            "downloadUserSelectedSource",
        ] {
            request
                .track
                .provenance
                .as_object_mut()
                .unwrap()
                .remove(key);
        }
        let item = DownloadWatch {
            id: Uuid::new_v4().to_string(),
            request,
            kind: kind.into(),
            state: "waiting".into(),
            next_check: Utc::now().timestamp() + HOUR,
            last_checked: None,
            revision: 0,
            job_id: None,
            target_asset_id: asset.map(|a| a.id.clone()),
            expected_hash: asset.map(|a| a.hash.clone()),
            failures: 0,
            music_directory: settings.music_directory.clone(),
            layout: settings.download_layout,
            matching_level: settings.matching_level,
        };
        items.push(item.clone());
        self.save_download_watches(&items)?;
        self.inner.watch_notify.notify_one();
        Ok(item)
    }
    pub async fn control_download_watch(&self, id: &str, action: &str) -> Result<(), String> {
        if !matches!(action, "pause" | "cancel" | "check" | "resume") {
            return Err("Unknown download watch action".into());
        }
        let _lease = self.inner.store.try_library_write()?;
        let _gate = self.inner.watch_gate.lock().await;
        let mut items = self.list_download_watches()?;
        let item = items
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or("Download watch not found")?;
        if !active(item) {
            return Err("This request has already ended".into());
        }
        if let Some(job_id) = &item.job_id {
            // Publication owns its final short critical section. Do not claim
            // cancellation after the file has already been committed.
            self.cancel(job_id).await?;
            item.job_id = None;
        }
        item.state = match action {
            "pause" => "paused",
            "cancel" => "cancelled",
            "check" | "resume" => "waiting",
            _ => return Err("Unknown download watch action".into()),
        }
        .into();
        if matches!(action, "check" | "resume") && item.music_directory.is_empty() {
            let settings = self.inner.store.settings()?;
            if settings.music_directory.trim().is_empty() {
                return Err(
                    "Configure this computer's download destination before resuming".into(),
                );
            }
            item.music_directory = settings.music_directory;
        }
        item.revision = item.revision.wrapping_add(1);
        if matches!(action, "check" | "resume") {
            item.next_check = Utc::now().timestamp();
        }
        self.save_download_watches(&items)?;
        self.inner.watch_notify.notify_one();
        Ok(())
    }
    pub fn start_download_watch_scheduler(&self) {
        if self
            .inner
            .watch_scheduler_started
            .swap(true, Ordering::AcqRel)
        {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            // Reconcile interrupted searches; jobs remain durable and are never duplicated.
            if let Ok(_gate) = manager.inner.watch_gate.try_lock() {
                if let Ok(mut items) = manager.list_download_watches() {
                    for item in &mut items {
                        if item.state == "checking" {
                            item.state = "waiting".into();
                        }
                    }
                    let _ = manager.save_download_watches(&items);
                }
            }
            loop {
                let sleep = manager
                    .check_next_download_watch()
                    .await
                    .unwrap_or(HOUR as u64);
                tokio::select! { _ = manager.inner.watch_notify.notified()=>{}, _ = tokio::time::sleep(Duration::from_secs(sleep.max(60)))=>{} }
            }
        });
    }
    async fn check_next_download_watch(&self) -> Result<u64, String> {
        let now = Utc::now().timestamp();
        let item = {
            let _gate = self.inner.watch_gate.lock().await;
            let mut items = self.list_download_watches()?;
            let jobs = self.inner.store.list_jobs()?;
            for item in items.iter_mut().filter(|item| item.state == "queued") {
                if let Some(job) = jobs
                    .iter()
                    .find(|job| Some(&job.id) == item.job_id.as_ref())
                {
                    match job.state.as_str() {
                        "completed" => item.state = "completed".into(),
                        "cancelled" => item.state = "cancelled".into(),
                        "failed" | "interrupted" => {
                            item.state = "waiting".into();
                            item.job_id = None;
                            item.failures = item.failures.saturating_add(1);
                            item.next_check = now + next_delay(item.failures);
                        }
                        _ => {}
                    }
                }
            }
            // Crash recovery between enqueue and saving job_id: the watch stamp
            // on the durable job is the idempotency key.
            for item in items.iter_mut().filter(|item| item.state == "waiting") {
                if let Some(job) = jobs.iter().rev().find(|job| {
                    job.track.provenance["soulseekDownloadWatchId"].as_str() == Some(&item.id)
                        && !matches!(job.state.as_str(), "failed" | "cancelled" | "interrupted")
                }) {
                    item.job_id = Some(job.id.clone());
                    item.state = if job.state == "completed" {
                        "completed"
                    } else {
                        "queued"
                    }
                    .into();
                }
            }
            let selected = items.iter().position(|item| {
                item.state == "waiting"
                    && item.next_check <= now
                    && self.inner.watch_search_not_before.load(Ordering::Acquire)
                        <= now.max(0) as u64
            });
            let item = selected.map(|index| {
                items[index].state = "checking".into();
                items[index].clone()
            });
            self.save_download_watches(&items)?;
            if item.is_none() {
                return Ok(items
                    .iter()
                    .filter(|i| i.state == "waiting")
                    .map(|i| (i.next_check - now).max(60) as u64)
                    .min()
                    .unwrap_or(HOUR as u64)
                    .min(HOUR as u64));
            }
            item.unwrap()
        };
        self.inner
            .watch_search_not_before
            .store(now.max(0) as u64 + 60, Ordering::Release);
        let result = self.search_download_watch(&item).await;
        let _gate = self.inner.watch_gate.lock().await;
        let mut items = self.list_download_watches()?;
        let Some(current) = items
            .iter_mut()
            .find(|i| i.id == item.id && i.revision == item.revision && i.state == "checking")
        else {
            return Ok(60);
        };
        current.last_checked = Some(now);
        match result {
            Ok(Some(candidate)) => {
                let mut settings = self.inner.store.settings()?;
                settings.music_directory = item.music_directory.clone();
                settings.download_layout = item.layout;
                settings.matching_level = item.matching_level;
                settings.download_fallback_enabled = false; // This request holds at Soulseek.
                let mut track = item.request.track.clone();
                track.provenance["soulseekDownloadWatchId"] = serde_json::json!(item.id);
                track.provenance["soulseekDownloadWatchKind"] = serde_json::json!(item.kind);
                track.provenance["downloadFallbackRequiresLossless"] =
                    serde_json::json!(item.request.requires_lossless);
                let disposition = match (&item.target_asset_id, &item.expected_hash) {
                    (Some(id), Some(hash)) => ReplacementDisposition::Replace {
                        target_asset_id: id.clone(),
                        expected_hash: hash.clone(),
                    },
                    _ => ReplacementDisposition::Keep,
                };
                let format_id = if item.request.requires_lossless {
                    candidate
                        .formats
                        .iter()
                        .find(|f| f.lossless)
                        .map(|f| f.id.clone())
                } else {
                    None
                };
                let request = DownloadRequest {
                    track,
                    candidate,
                    format_id,
                    output_profile: item.request.output_profile.clone(),
                    disposition,
                    chapter_ids: None,
                    playlist_context: item.request.playlist_context.clone(),
                };
                match self.enqueue(request, settings).await {
                    Ok(job) => {
                        current.job_id = Some(job.id);
                        current.state = "queued".into();
                    }
                    Err(_) => {
                        current.state = "review".into();
                    }
                }
            }
            Ok(None) => {
                current.state = "waiting".into();
                current.next_check = now + HOUR;
                current.failures = 0;
            }
            Err(reason) => {
                current.state = if reason == "review" {
                    "review"
                } else {
                    "waiting"
                }
                .into();
                current.failures = current.failures.saturating_add(1);
                current.next_check = now + next_delay(current.failures);
            }
        }
        self.save_download_watches(&items)?;
        Ok(60)
    }
    async fn search_download_watch(
        &self,
        item: &DownloadWatch,
    ) -> Result<Option<Candidate>, String> {
        let _lease = self.inner.store.try_library_write()?;
        if self.inner.connections.slskd()?.is_none() {
            return Ok(None);
        }
        if let Some(id) = &item.target_asset_id {
            let asset = self.inner.store.get_asset_by_id(id)?.ok_or("review")?;
            if Some(&asset.hash) != item.expected_hash.as_ref() || !lossy(&asset) {
                return Err("review".into());
            }
            if let Some(app) = &self.inner.app {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    if state.playback.recording_in_use(&asset.recording_id)? {
                        return Ok(None);
                    }
                }
            }
        }
        let candidates = self
            .find_slskd_candidates_for_track(&item.request.track)
            .await?;
        let mut review = false;
        for candidate in candidates {
            if candidate.provider != "slskd"
                || candidate.permission == "restricted"
                || (item.request.requires_lossless && !candidate.formats.iter().any(|f| f.lossless))
            {
                continue;
            }
            let assessment = crate::recording_match_context::assess(
                &self.inner.store,
                &item.request.track,
                &candidate,
                item.matching_level,
                crate::recording_match::MatchStage::Resolved,
            );
            if assessment.action == crate::recording_match::MatchAction::Accept {
                return Ok(Some(candidate));
            }
            if assessment.action != crate::recording_match::MatchAction::Reject {
                review = true;
            }
        }
        if review {
            Err("review".into())
        } else {
            Ok(None)
        }
    }
    pub(super) fn commit_download_result(
        &self,
        job: &DownloadJob,
        asset: &LocalAsset,
    ) -> Result<LocalAsset, String> {
        if job.track.provenance["soulseekDownloadWatchKind"].as_str() == Some("upgrade") {
            if let Some(app) = &self.inner.app {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    return state
                        .playback
                        .commit_background_upgrade(asset, &job.disposition);
                }
            }
        }
        self.inner
            .store
            .commit_download_asset(asset, &job.disposition)
    }
    pub(super) async fn wait_for_upgrade_playback(
        &self,
        job: &mut DownloadJob,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        if job.track.provenance["soulseekDownloadWatchKind"].as_str() != Some("upgrade") {
            return Ok(());
        }
        let recording = self.inner.store.recording_id(&job.track.id)?;
        if let Some(app) = &self.inner.app {
            if let Some(state) = app.try_state::<crate::AppState>() {
                let mut updates = state.playback.subscribe();
                while state.playback.recording_in_use(&recording)? {
                    job.state = "waiting_for_playback".into();
                    self.update(job)?;
                    tokio::select! {_ = crate::playback_sources::wait_for_cancellation(cancel.clone())=>return Err("Cancelled".into()), result=updates.changed()=>{result.map_err(|_|"Playback ownership is unavailable")?;}}
                }
            }
        }
        Ok(())
    }
    pub(super) async fn defer_soulseek_job(
        &self,
        job: &mut DownloadJob,
        settings: &Settings,
        lossless: bool,
    ) -> Result<(), String> {
        if job.chapter_ids.as_ref().is_some_and(|ids| !ids.is_empty())
            || job.disposition != ReplacementDisposition::Keep
        {
            return Err("This download needs an explicit source selection".into());
        }
        let request = DownloadWatchRequest {
            track: job.track.clone(),
            output_profile: job.output_profile.clone(),
            playlist_context: job.playlist_context.clone(),
            requires_lossless: lossless && !settings.download_fallback_allow_lossy,
        };
        let mut snapshot = settings.clone();
        snapshot.download_layout = job.download_layout;
        snapshot.matching_level = download_matching_level(&job.track);
        let item = self.add_download_watch(request, &snapshot, None).await?;
        job.track.provenance["deferredSoulseekWatchId"] = serde_json::json!(item.id);
        Ok(())
    }
    pub(super) async fn remember_lossless_upgrade(
        &self,
        job: &DownloadJob,
        asset: &LocalAsset,
        settings: &Settings,
    ) {
        if settings.soulseek_lossless_upgrades
            && job.output_profile == "source"
            && job.chapter_ids.as_ref().is_none_or(Vec::is_empty)
            && job.track.provenance["soulseekDownloadWatchId"].is_null()
            && lossy(asset)
        {
            let request = DownloadWatchRequest {
                track: job.track.clone(),
                output_profile: "source".into(),
                playlist_context: job.playlist_context.clone(),
                requires_lossless: true,
            };
            if let Err(error) = self
                .add_download_watch(request, settings, Some(asset))
                .await
            {
                eprintln!("Could not schedule lossless upgrade: {error}");
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_are_bounded_and_back_off() {
        assert_eq!(next_delay(0), HOUR);
        assert_eq!(next_delay(100), HOUR * 8);
    }
    #[test]
    fn old_settings_do_not_opt_into_background_downloads() {
        let settings = Settings::default();
        assert!(!settings.soulseek_wait_for_source);
        assert!(!settings.soulseek_lossless_upgrades);
    }
    #[tokio::test]
    async fn requests_persist_policy_and_cancel_without_a_transfer_slot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watches.sqlite");
        let store = Arc::new(Store::open(&path).unwrap());
        let manager = AcquisitionManager::new_for_playback_test(
            store.clone(),
            Arc::new(ConnectionManager::new(store.clone())),
        );
        let settings = Settings {
            soulseek_wait_for_source: true,
            music_directory: dir.path().to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let request = DownloadWatchRequest {
            track: Track {
                id: "catalog:recording".into(),
                artist: "Artist".into(),
                title: "Recording".into(),
                ..Track::default()
            },
            output_profile: "source".into(),
            requires_lossless: true,
            playlist_context: None,
        };
        let item = manager
            .queue_download_watch(request.clone(), &settings)
            .await
            .unwrap();
        assert_eq!(manager.inner.slots.available_permits(), 2);
        assert_eq!(
            manager
                .queue_download_watch(request, &settings)
                .await
                .unwrap()
                .id,
            item.id
        );
        assert_eq!(manager.list_download_watches().unwrap().len(), 1);
        let restored = AcquisitionManager::new_for_playback_test(
            store.clone(),
            Arc::new(ConnectionManager::new(store.clone())),
        );
        let saved = restored.list_download_watches().unwrap().pop().unwrap();
        assert!(saved.request.requires_lossless);
        assert_eq!(saved.music_directory, settings.music_directory);
        assert_eq!(saved.matching_level, settings.matching_level);
        restored
            .control_download_watch(&item.id, "cancel")
            .await
            .unwrap();
        assert_eq!(
            restored.list_download_watches().unwrap()[0].state,
            "cancelled"
        );
        assert!(restored.inner.active.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn opt_in_and_whole_recording_validation_do_not_create_phantom_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("invalid.sqlite")).unwrap());
        let manager = AcquisitionManager::new_for_playback_test(
            store.clone(),
            Arc::new(ConnectionManager::new(store.clone())),
        );
        let request = DownloadWatchRequest {
            track: Track::default(),
            output_profile: "source".into(),
            requires_lossless: false,
            playlist_context: None,
        };
        assert!(manager
            .queue_download_watch(request.clone(), &Settings::default())
            .await
            .is_err());
        let settings = Settings {
            soulseek_wait_for_source: true,
            music_directory: dir.path().to_string_lossy().into_owned(),
            ..Settings::default()
        };
        assert!(manager
            .queue_download_watch(request, &settings)
            .await
            .is_err());
        assert!(store.list_jobs().unwrap().is_empty());
        assert!(manager.list_download_watches().unwrap().is_empty());
    }
}
