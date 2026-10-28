//! Bounded preparation for saved radio songs. No queue, history, tags or account writes.
use crate::{
    acquisition::AcquisitionManager,
    models::Track,
    playback_sources::{PlaybackDeadline, SourceTrace},
    restore_work_policy::AutomaticWorkKind,
    storage::{LibraryWritePermit, Store},
};
use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Semaphore};

const FOREGROUND_QUIET_PERIOD: Duration = Duration::from_secs(30);
const MAX_QUIET_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct PreparationGate {
    active: Option<(u64, watch::Sender<bool>)>,
    last_foreground: Option<Instant>,
    next_job_id: u64,
}

impl PreparationGate {
    fn foreground(&mut self, now: Instant) {
        self.last_foreground = Some(now);
        if let Some((_, stop)) = &self.active {
            stop.send_replace(true);
        }
    }

    fn quiet_remaining(&self, now: Instant) -> Duration {
        self.last_foreground
            .map(|last| FOREGROUND_QUIET_PERIOD.saturating_sub(now.saturating_duration_since(last)))
            .unwrap_or_default()
    }

    /// Called while holding the same gate lock as `foreground`, so a play
    /// request cannot land between the quiet check and active-job publication.
    fn register_if_quiet(&mut self, now: Instant, stop: watch::Sender<bool>) -> Option<u64> {
        if !self.quiet_remaining(now).is_zero() {
            return None;
        }
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        let id = self.next_job_id;
        self.active = Some((id, stop));
        Some(id)
    }

    fn clear_active(&mut self, id: u64) {
        if self
            .active
            .as_ref()
            .is_some_and(|(active_id, _)| *active_id == id)
        {
            self.active = None;
        }
    }
}

fn gate() -> &'static Mutex<PreparationGate> {
    static GATE: OnceLock<Mutex<PreparationGate>> = OnceLock::new();
    GATE.get_or_init(Default::default)
}
pub(crate) fn yield_to_playback() {
    crate::compilation_excerpt::yield_to_playback();
    if let Ok(mut state) = gate().lock() {
        state.foreground(Instant::now());
    }
}

async fn wait_for_quiet_and_register(stop: watch::Sender<bool>) -> Option<u64> {
    loop {
        let remaining = gate()
            .lock()
            .map(|state| state.quiet_remaining(Instant::now()))
            .unwrap_or(FOREGROUND_QUIET_PERIOD);
        if !remaining.is_zero() {
            tokio::time::sleep(remaining.min(MAX_QUIET_WAIT)).await;
            continue;
        }
        let mut state = gate().lock().ok()?;
        if let Some(id) = state.register_if_quiet(Instant::now(), stop.clone()) {
            return Some(id);
        }
    }
}
fn eligible(track: &Track) -> bool {
    track.provider == "radio_song"
        && track.local_path.is_none()
        && track.asset_id.is_none()
        && track.playback_range.is_none()
        && !track.artist.trim().is_empty()
        && !track.title.trim().is_empty()
        && !crate::recording_identity::placeholder_title(&track.title)
}

fn acquire_worker_write(store: &Store) -> Option<LibraryWritePermit> {
    if crate::restore_work_policy::is_paused(store, AutomaticWorkKind::BackgroundSourcePreparation)
    {
        return None;
    }
    store.try_library_write().ok()
}
/// At most 100 queued jobs, one provider operation at a time. A play click
/// cancels the active job and keeps queued work deferred until 30 seconds pass.
pub(crate) fn schedule(acquisition: Arc<AcquisitionManager>, tracks: Vec<Track>) {
    let store = acquisition.alias_store();
    if crate::restore_work_policy::is_paused(&store, AutomaticWorkKind::BackgroundSourcePreparation)
    {
        return;
    }
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    static QUEUED: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let slots = SLOTS.get_or_init(|| Arc::new(Semaphore::new(1))).clone();
    let queue = QUEUED.get_or_init(|| Arc::new(Semaphore::new(100))).clone();
    for track in tracks.into_iter().filter(eligible).take(100) {
        let Ok(queued) = queue.clone().try_acquire_owned() else {
            break;
        };
        let acquisition = acquisition.clone();
        let slots = slots.clone();
        tauri::async_runtime::spawn(async move {
            let _queued = queued;
            // Keep launch/UI work ahead of background metadata reads.
            tokio::time::sleep(Duration::from_secs(15)).await;
            let Ok(_permit) = slots.acquire_owned().await else {
                return;
            };
            let (stop, cancel) = watch::channel(false);
            let Some(active_id) = wait_for_quiet_and_register(stop).await else {
                return;
            };
            // Re-read after the quiet period, since the player/library/settings
            // may have changed while this queued task was waiting.
            let store = acquisition.alias_store();
            // Restore staging must exclude the whole provider-backed worker,
            // not just its final cache write. Acquire only after the foreground
            // quiet wait so ordinary playback keeps restore responsive.
            let Some(_library_write) = acquire_worker_write(&store) else {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            };
            let Ok(Some(saved)) = store.get_library_track(&track.id) else {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            };
            if !eligible(&saved) {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            }
            let Ok(settings) = store.settings() else {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            };
            if !settings.background_source_preparation || !settings.streaming_fallback_enabled {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            }
            let Ok(key) = crate::remote_playlists::alias_hydration_key(&saved) else {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            };
            let key = format!("matching-v1:{:?}:{key}", settings.matching_level);
            const CACHE: &str = "playback.background-preparation.v2";
            if store
                .get_provider_cache(CACHE, &key)
                .ok()
                .flatten()
                .is_some()
            {
                if let Ok(mut state) = gate().lock() {
                    state.clear_active(active_id);
                }
                return;
            }
            let started = std::time::Instant::now();
            let deadline = PlaybackDeadline::after(Duration::from_secs(20));
            let result = tokio::time::timeout_at(
                deadline.instant(),
                crate::playback_sources::resolve_with_trace_context(
                    &store,
                    &acquisition,
                    &saved,
                    None,
                    cancel.clone(),
                    Some("preparation:radio-source".into()),
                    None,
                    None,
                ),
            )
            .await;
            let mut provider = result
                .as_ref()
                .ok()
                .and_then(|r| r.as_ref().ok())
                .and_then(|s| s.candidate.as_ref())
                .map(|c| c.provider.clone())
                .unwrap_or_else(|| "catalog".into());
            let mut ready = matches!(result, Ok(Ok(_)));
            if !ready
                && !*cancel.borrow()
                && crate::bandcamp_fallback::eligible(&saved, &settings, false, 2)
            {
                let deadline = PlaybackDeadline::after(Duration::from_secs(45));
                provider = "bandcamp".into();
                ready = matches!(
                    tokio::time::timeout_at(
                        deadline.instant(),
                        crate::bandcamp_fallback::resolve(
                            &store,
                            &acquisition,
                            &saved,
                            &settings,
                            cancel.clone(),
                            SourceTrace::new(None, None, None),
                            deadline,
                            &[],
                            true
                        )
                    )
                    .await,
                    Ok(Ok(_))
                );
            }
            if !*cancel.borrow() {
                // Fingerprint includes identity/manual edits; failures expire sooner.
                let unchanged = store
                    .get_library_track(&saved.id)
                    .ok()
                    .flatten()
                    .and_then(|t| crate::remote_playlists::alias_hydration_key(&t).ok())
                    == Some(key.rsplit(':').next().unwrap_or_default().to_owned());
                if unchanged {
                    let _ = store.set_provider_cache(
                        CACHE,
                        &key,
                        &json!({"ready":ready,"elapsedMs":started.elapsed().as_millis()}),
                        if ready { 7 * 86400 } else { 3600 },
                    );
                    if ready {
                        acquisition.notify_source_ready(&saved, &provider);
                    }
                }
            }
            if let Ok(mut current) = gate().lock() {
                current.clear_active(active_id);
            }
        });
    }
}

pub(crate) fn schedule_saved(acquisition: Arc<AcquisitionManager>) {
    let store = acquisition.alias_store();
    if crate::restore_work_policy::is_paused(&store, AutomaticWorkKind::BackgroundSourcePreparation)
        || !store
            .settings()
            .is_ok_and(|s| s.background_source_preparation)
    {
        return;
    }
    let offset = store
        .get_json_preference("sourcePreparation.scanOffset.v1")
        .ok()
        .flatten()
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(1_000_000) as usize;
    if let Ok((tracks, total)) = store.list_tracks_page(
        "library",
        None,
        100,
        offset,
        None,
        Some("dateAdded"),
        Some("desc"),
        None,
        None,
        None,
        Some("radio_song"),
    ) {
        let next = if offset.saturating_add(100) >= total {
            0
        } else {
            offset + 100
        };
        let _ = store.set_json_preference("sourcePreparation.scanOffset.v1", &json!(next));
        schedule(acquisition, tracks);
    }
}

/// Number of saved radio recordings that can still be queued for source
/// preparation. This follows the scheduler's settings, identity, eligibility,
/// and cache checks so restore review reports actionable work only.
pub(crate) fn pending_count(store: &Store) -> Result<usize, String> {
    let settings = store.settings().map_err(|error| error.to_string())?;
    if !settings.background_source_preparation || !settings.streaming_fallback_enabled {
        return Ok(0);
    }
    let tracks = store
        .list_tracks("library", None)
        .map_err(|error| error.to_string())?;
    let mut pending = 0usize;
    for track in tracks.into_iter().filter(eligible) {
        let Ok(identity) = crate::remote_playlists::alias_hydration_key(&track) else {
            continue;
        };
        let key = format!("matching-v1:{:?}:{identity}", settings.matching_level);
        let cached = store
            .get_provider_cache("playback.background-preparation.v2", &key)
            .map_err(|error| error.to_string())?;
        if cached.is_none() {
            pending += 1;
        }
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_only_accepts_reusable_radio_recordings() {
        let mut track = Track {
            provider: "radio_song".into(),
            title: "Nighttime in the Backyard".into(),
            artist: "Lucas Santtana & Arto Lindsay".into(),
            ..Default::default()
        };
        assert!(eligible(&track));
        track.title = "[data track]".into();
        assert!(!eligible(&track));
        track.title = "Matthew".into();
        track.provider = "spotify".into();
        assert!(!eligible(&track));
    }

    #[test]
    fn source_preparation_worker_cannot_start_during_restore_staging() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let _restore = store.try_restore_permit().unwrap();

        assert!(acquire_worker_write(&store).is_none());
    }

    #[test]
    fn foreground_before_registration_defers_background_work() {
        let now = Instant::now();
        let mut state = PreparationGate::default();
        state.foreground(now);
        let (stop, _) = watch::channel(false);

        assert!(state.register_if_quiet(now, stop.clone()).is_none());
        assert_eq!(state.quiet_remaining(now), FOREGROUND_QUIET_PERIOD);
        assert!(state
            .register_if_quiet(now + FOREGROUND_QUIET_PERIOD, stop)
            .is_some());
    }

    #[test]
    fn foreground_after_registration_cancels_the_active_job() {
        let now = Instant::now();
        let mut state = PreparationGate::default();
        let (stop, mut cancel) = watch::channel(false);

        assert!(state.register_if_quiet(now, stop).is_some());
        assert!(!*cancel.borrow());
        state.foreground(now + Duration::from_secs(1));

        assert!(*cancel.borrow());
    }

    #[test]
    fn next_queued_job_waits_for_quiet_period_after_foreground_yield() {
        let now = Instant::now();
        let mut state = PreparationGate::default();
        let (first_stop, mut first_cancel) = watch::channel(false);
        let first_id = state.register_if_quiet(now, first_stop).unwrap();

        state.foreground(now + Duration::from_secs(5));
        assert!(*first_cancel.borrow());
        state.clear_active(first_id);
        let (next_stop, _) = watch::channel(false);
        assert!(state
            .register_if_quiet(now + Duration::from_secs(34), next_stop.clone())
            .is_none());
        assert_eq!(
            state.quiet_remaining(now + Duration::from_secs(34)),
            Duration::from_secs(1)
        );
        assert!(state
            .register_if_quiet(now + Duration::from_secs(35), next_stop.clone())
            .is_some());
    }
}
