//! On-demand identification for audio played through the WebView element.
//!
//! The renderer never supplies a source URL or local path. The resolver registers
//! its already validated source here, and the short-lived source stays in memory.

use crate::{
    native_output::AudioSource, playback::PlayingTrackIdentification, radio::RadioStation,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

const MAX_ACTIVE_JOBS: usize = 4;
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Check the server's WebView snapshot against the source and playback identity
/// captured when the user requested identification.
pub fn snapshot_matches_playback(
    snapshot: &serde_json::Value,
    native_enabled: bool,
    track_id: &str,
    generation: u64,
    seek_serial: Option<u64>,
    chapter_id: Option<&str>,
) -> bool {
    let active_track = snapshot["trackId"]
        .as_str()
        .or_else(|| snapshot["track"]["id"].as_str());
    !native_enabled
        && snapshot["status"].as_str() == Some("playing")
        && active_track == Some(track_id)
        && snapshot["sourceGeneration"].as_u64() == Some(generation)
        && seek_serial.is_none_or(|serial| snapshot["seekSerial"].as_u64() == Some(serial))
        && snapshot
            .get("chapterId")
            .and_then(serde_json::Value::as_str)
            == chapter_id
}

#[derive(Clone)]
struct RegisteredSource {
    generation: u64,
    track_id: String,
    source: AudioSource,
    resource_identity: Option<crate::playing_track_detections::SourceIdentity>,
    live: bool,
    bounds: Option<(f64, f64)>,
}

struct Inner {
    source: Option<RegisteredSource>,
    epoch: u64,
    next_job: u64,
    jobs: HashMap<u64, watch::Sender<bool>>,
    pcm_capture_ids: HashMap<String, u64>,
    cancelled_pcm_capture_ids: HashSet<String>,
    cancelled_pcm_capture_order: VecDeque<String>,
    closed: bool,
    active_jobs: watch::Sender<usize>,
}

impl Default for Inner {
    fn default() -> Self {
        let (active_jobs, _) = watch::channel(0);
        Self {
            source: None,
            epoch: 0,
            next_job: 0,
            jobs: HashMap::new(),
            pcm_capture_ids: HashMap::new(),
            cancelled_pcm_capture_ids: HashSet::new(),
            cancelled_pcm_capture_order: VecDeque::new(),
            closed: false,
            active_jobs,
        }
    }
}

#[derive(Clone, Default)]
pub struct WebViewIdentification(Arc<Mutex<Inner>>);

impl WebViewIdentification {
    /// Check that the currently registered WebView source belongs to this exact
    /// playback generation and a direct YouTube media URL. The source URL stays
    /// private; callers receive only a boolean ownership result.
    pub fn matches_registered_youtube_source(&self, generation: u64, track_id: &str) -> bool {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(source) = inner
            .source
            .as_ref()
            .filter(|source| source.generation == generation && source.track_id == track_id)
        else {
            return false;
        };
        if source.live || source.bounds.is_some() {
            return false;
        }
        crate::acquisition::validate_source_url(&source.source.input, "direct")
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .is_some_and(|host| host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
    }

    /// True only for a server-registered finite source owned by this exact
    /// playback generation. Live radio is excluded; bounded excerpts retain
    /// their source identity so detections can be stored on the parent source
    /// clock when the extractor reports an absolute position.
    pub fn matches_registered_finite_source(&self, generation: u64, track_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .source
            .as_ref()
            .is_some_and(|source| {
                source.generation == generation
                    && source.track_id == track_id
                    && !source.live
                    && source.bounds.is_none_or(|(start, end)| {
                        start.is_finite() && end.is_finite() && start >= 0.0 && end > start
                    })
            })
    }

    pub fn registered_resource_identity(
        &self,
        generation: u64,
        track_id: &str,
    ) -> Option<crate::playing_track_detections::SourceIdentity> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .source
            .as_ref()
            .filter(|source| {
                source.generation == generation
                    && source.track_id == track_id
                    && !source.live
                    && source.bounds.is_none_or(|(start, end)| {
                        start.is_finite() && end.is_finite() && start >= 0.0 && end > start
                    })
            })
            .and_then(|source| source.resource_identity.clone())
    }

    pub fn registered_finite_source_epoch(&self, generation: u64, track_id: &str) -> Option<u64> {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .source
            .as_ref()
            .filter(|source| {
                source.generation == generation
                    && source.track_id == track_id
                    && !source.live
                    && source.bounds.is_none_or(|(start, end)| {
                        start.is_finite() && end.is_finite() && start >= 0.0 && end > start
                    })
            })
            .map(|_| inner.epoch)
    }

    /// Call only after the resolver has validated this source and the request is current.
    pub fn register(
        &self,
        generation: u64,
        track_id: String,
        source: AudioSource,
        resource_identity: Option<crate::playing_track_detections::SourceIdentity>,
        live: bool,
        bounds: Option<(f64, f64)>,
    ) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        cancel_jobs(&mut inner);
        inner.epoch = inner.epoch.wrapping_add(1);
        inner.source = Some(RegisteredSource {
            generation,
            track_id,
            source,
            resource_identity,
            live,
            // Preserve invalid bounds as invalid. Filtering them to `None`
            // would incorrectly promote a malformed excerpt to a full source.
            bounds,
        });
    }

    /// Invalidate any source left by an older WebView playback request.
    pub fn clear(&self) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        cancel_jobs(&mut inner);
        inner.epoch = inner.epoch.wrapping_add(1);
        inner.source = None;
    }

    /// Cancel active extraction/recognition while retaining the current source
    /// so playback can resume and Identify can be used again without resolving.
    pub fn cancel_capture(&self) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.epoch = inner.epoch.wrapping_add(1);
        cancel_jobs(&mut inner);
    }

    /// Register an identity-scoped captured-PCM recognition so playback changes
    /// and an explicit Cancel can drop SongRec and its private temporary WAV.
    pub fn begin_pcm_capture(&self, capture_id: String) -> Result<PcmCaptureJob, String> {
        if capture_id.is_empty() || capture_id.len() > 128 {
            return Err("Audio capture identity is invalid".into());
        }
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if inner.closed {
            return Err("Audio identification is shutting down".into());
        }
        if inner.jobs.len() >= MAX_ACTIVE_JOBS {
            return Err("Too many audio identification requests are already active".into());
        }
        if inner.cancelled_pcm_capture_ids.contains(&capture_id) {
            return Err("Audio capture was cancelled before recognition began".into());
        }
        if inner.pcm_capture_ids.contains_key(&capture_id) {
            return Err("Audio capture identity is already active".into());
        }
        inner.next_job = inner.next_job.wrapping_add(1);
        let job_id = inner.next_job;
        let (tx, rx) = watch::channel(false);
        inner.jobs.insert(job_id, tx);
        inner.pcm_capture_ids.insert(capture_id.clone(), job_id);
        let active = inner.jobs.len();
        inner.active_jobs.send_replace(active);
        Ok(PcmCaptureJob {
            inner: self.0.clone(),
            capture_id,
            job_id,
            cancelled: rx,
        })
    }

    /// Stop admission, cancel active WebView identification work, and wait for
    /// the worker-owned WAV/job guards to drop after child-process cleanup.
    pub async fn shutdown_for_exit(&self) -> Result<(), String> {
        let mut active_jobs = {
            let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            inner.closed = true;
            inner.epoch = inner.epoch.wrapping_add(1);
            inner.source = None;
            cancel_jobs(&mut inner);
            inner.active_jobs.subscribe()
        };
        let drain = async {
            loop {
                if *active_jobs.borrow_and_update() == 0 {
                    return;
                }
                if active_jobs.changed().await.is_err() {
                    return;
                }
            }
        };
        tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, drain)
            .await
            .map_err(|_| {
                "WebView audio identification cleanup did not finish before shutdown".to_owned()
            })
    }

    /// Cancel only the matching capture nonce, so a stale frontend Cancel cannot
    /// interrupt a newer Identify request.
    pub fn cancel_pcm_capture(&self, capture_id: &str) -> bool {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job_id) = inner.pcm_capture_ids.get(capture_id) {
            if let Some(sender) = inner.jobs.get(job_id) {
                sender.send_replace(true);
                return true;
            }
        }
        if capture_id.is_empty() || capture_id.len() > 128 {
            return false;
        }
        if inner
            .cancelled_pcm_capture_ids
            .insert(capture_id.to_owned())
        {
            inner
                .cancelled_pcm_capture_order
                .push_back(capture_id.to_owned());
            while inner.cancelled_pcm_capture_order.len() > 128 {
                if let Some(expired) = inner.cancelled_pcm_capture_order.pop_front() {
                    inner.cancelled_pcm_capture_ids.remove(&expired);
                }
            }
        }
        true
    }

    pub async fn identify(
        &self,
        generation: u64,
        track_id: &str,
        position_seconds: f64,
    ) -> Result<PlayingTrackIdentification, String> {
        self.identify_with_capture(generation, track_id, position_seconds, None)
            .await
    }

    pub async fn identify_with_capture(
        &self,
        generation: u64,
        track_id: &str,
        position_seconds: f64,
        capture_id: Option<String>,
    ) -> Result<PlayingTrackIdentification, String> {
        let (registered, epoch, job_id, mut cancelled, capture_id) = {
            let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if inner.closed {
                return Err("Audio identification is shutting down".into());
            }
            if inner.jobs.len() >= MAX_ACTIVE_JOBS {
                return Err("Too many audio identification requests are already active".into());
            }
            let registered = inner
                .source
                .clone()
                .filter(|r| r.generation == generation && r.track_id == track_id)
                .ok_or("Playback source is unavailable for identification")?;
            if let Some(capture_id) = capture_id.as_deref() {
                if capture_id.is_empty() || capture_id.len() > 128 {
                    return Err("Audio capture identity is invalid".into());
                }
                if inner.cancelled_pcm_capture_ids.contains(capture_id) {
                    return Err("Audio capture was cancelled before recognition began".into());
                }
                if inner.pcm_capture_ids.contains_key(capture_id) {
                    return Err("Audio capture identity is already active".into());
                }
            }
            let epoch = inner.epoch;
            inner.next_job = inner.next_job.wrapping_add(1);
            let job_id = inner.next_job;
            let (tx, rx) = watch::channel(false);
            inner.jobs.insert(job_id, tx);
            if let Some(capture_id) = capture_id.as_ref() {
                inner.pcm_capture_ids.insert(capture_id.clone(), job_id);
            }
            let active = inner.jobs.len();
            inner.active_jobs.send_replace(active);
            (registered, epoch, job_id, rx, capture_id)
        };
        let _job = IdentificationJob(self.0.clone(), job_id, capture_id);
        if !position_seconds.is_finite() || position_seconds < 0.0 {
            return Err("Current playback position is unavailable".into());
        }
        let station = RadioStation {
            id: format!("webview:{generation}"),
            name: "Currently playing audio".into(),
            stream_url: String::new(),
            source_url: String::new(),
            provider: Some("playback_webview".into()),
            country: None,
            tags: None,
            homepage_url: None,
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: true,
        };
        let (start, seconds) =
            excerpt_window(position_seconds, registered.live, registered.bounds)?;
        let source = registered.source.clone();
        let path = std::env::temp_dir().join(format!(
            "burrata-webview-recognition-{}.wav",
            uuid::Uuid::new_v4()
        ));
        let cleanup = TemporaryPcmFile(path.clone());
        let capture_path = path.clone();
        let recognition = Box::pin(async {
            let recognized =
                crate::radio::recognize_played_sample_with_capture(&station, true, || async move {
                    extract_wav(source, start, seconds, capture_path).await
                })
                .await?;
            Ok::<_, String>((recognized, (start, seconds)))
        });
        let recognized = tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => return Err("Track identification exceeded 60 seconds".into()),
            _ = cancelled.changed() => return Err("Track identification was cancelled".into()),
            result = recognition => result?,
        };
        drop(cleanup);
        let still_current = {
            let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            inner.epoch == epoch
                && inner
                    .source
                    .as_ref()
                    .is_some_and(|r| r.generation == generation && r.track_id == track_id)
        };
        if !still_current {
            return Err("Playback changed during identification".into());
        }
        let ((identified, note), captured_audio_interval) = recognized;
        let result_title = identified.as_ref().map(|m| m.title.clone());
        let artwork_url = identified.as_ref().and_then(|m| m.artwork_url.clone());
        let detection_metadata = identified.clone();
        let result_artist = identified.map(|m| m.artist);
        let note = note.unwrap_or_else(|| {
            if result_title.is_some() {
                "SongRec identified the current audio; catalog identity is unverified.".into()
            } else {
                "SongRec did not identify a track in this audio excerpt.".into()
            }
        });
        Ok(PlayingTrackIdentification {
            track_id: track_id.to_owned(),
            title: result_title,
            artist: result_artist,
            artwork_url,
            note,
            timing_anchor: None,
            detection_metadata,
            captured_audio_interval: Some(captured_audio_interval),
            detection_source_identity: registered.resource_identity,
            detection_source_epoch: Some(epoch),
        })
    }
}

fn excerpt_window(
    position: f64,
    live: bool,
    bounds: Option<(f64, f64)>,
) -> Result<(f64, f64), String> {
    if !position.is_finite() || position < 0.0 {
        return Err("Current playback position is unavailable".into());
    }
    if live {
        return Ok((0.0, 12.0));
    }
    let mut start = (position - 6.0).max(0.0);
    if let Some((lower, upper)) = bounds {
        if !lower.is_finite() || !upper.is_finite() || lower < 0.0 || upper <= lower {
            return Err("Current chapter bounds are invalid".into());
        }
        if position < lower || position > upper {
            return Err("Playback position is outside the current chapter".into());
        }
        start = start.max(lower);
        if start + 12.0 > upper {
            start = (upper - 12.0).max(lower);
        }
        let seconds = (upper - start).min(12.0);
        if seconds < 1.0 {
            return Err("Not enough audio remains in this chapter to identify".into());
        }
        return Ok((start, seconds));
    }
    Ok((start, 12.0))
}

async fn extract_wav(
    source: AudioSource,
    start_seconds: f64,
    seconds: f64,
    path: PathBuf,
) -> Result<PathBuf, String> {
    let mut command = tokio::process::Command::new(source.ffmpeg);
    crate::tools::configure_no_console_tokio(&mut command);
    command
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error");
    if !source.headers.is_empty() {
        let headers = source
            .headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect::<String>();
        command.arg("-headers").arg(headers);
    }
    if start_seconds > 0.0 {
        command.arg("-ss").arg(format!("{start_seconds:.3}"));
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        command
            .arg("-i")
            .arg(source.input)
            .arg("-t")
            .arg(format!("{seconds:.3}"))
            .args(["-vn", "-ac", "1", "-ar", "44100", "-c:a", "pcm_s16le", "-y"])
            .arg(&path)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Audio excerpt extraction timed out".to_owned())?
    .map_err(|_| "Could not extract an audio excerpt".to_owned())?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&path);
        // Never return or log FFmpeg stderr: it can contain signed URLs or headers.
        return Err("Could not extract an audio excerpt from the current source".into());
    }
    Ok(path)
}

fn cancel_jobs(inner: &mut Inner) {
    for sender in inner.jobs.values() {
        sender.send_replace(true);
    }
}

pub struct PcmCaptureJob {
    inner: Arc<Mutex<Inner>>,
    capture_id: String,
    job_id: u64,
    cancelled: watch::Receiver<bool>,
}
impl PcmCaptureJob {
    pub fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.clone()
    }
}
impl Drop for PcmCaptureJob {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.jobs.remove(&self.job_id);
        if inner.pcm_capture_ids.get(&self.capture_id) == Some(&self.job_id) {
            inner.pcm_capture_ids.remove(&self.capture_id);
        }
        let active = inner.jobs.len();
        inner.active_jobs.send_replace(active);
    }
}

struct IdentificationJob(Arc<Mutex<Inner>>, u64, Option<String>);
impl Drop for IdentificationJob {
    fn drop(&mut self) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.jobs.remove(&self.1);
        if let Some(capture_id) = self.2.as_deref() {
            if inner.pcm_capture_ids.get(capture_id) == Some(&self.1) {
                inner.pcm_capture_ids.remove(capture_id);
            }
        }
        let active = inner.jobs.len();
        inner.active_jobs.send_replace(active);
    }
}

struct TemporaryPcmFile(PathBuf);
impl Drop for TemporaryPcmFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> AudioSource {
        AudioSource {
            input: "https://private.example/stream?token=secret".into(),
            ffmpeg: PathBuf::from("ffmpeg"),
            headers: vec![],
            decoder_hint: None,
            cenc: None,
        }
    }

    #[tokio::test]
    async fn requires_registered_current_generation_and_track() {
        let state = WebViewIdentification::default();
        assert!(state.identify(4, "track", 12.0).await.is_err());
        state.register(5, "track".into(), source(), None, false, None);
        assert!(state.identify(4, "track", 12.0).await.is_err());
        assert!(state.identify(5, "other", 12.0).await.is_err());
        state.clear();
        assert!(state.identify(5, "track", 12.0).await.is_err());
    }

    #[test]
    fn source_identity_is_available_for_current_finite_full_and_bounded_resources() {
        let state = WebViewIdentification::default();
        let identity =
            crate::playing_track_detections::SourceIdentity::new("youtube", "video-a").unwrap();
        state.register(
            5,
            "track".into(),
            source(),
            Some(identity.clone()),
            false,
            None,
        );
        assert!(state.matches_registered_finite_source(5, "track"));
        assert_eq!(
            state.registered_resource_identity(5, "track"),
            Some(identity.clone())
        );
        assert!(state.registered_resource_identity(4, "track").is_none());
        assert!(state.registered_resource_identity(5, "other").is_none());
        let captured_epoch = state.registered_finite_source_epoch(5, "track").unwrap();
        let replacement =
            crate::playing_track_detections::SourceIdentity::new("bandcamp", "track-b").unwrap();
        state.register(
            5,
            "track".into(),
            source(),
            Some(replacement.clone()),
            false,
            None,
        );
        assert_ne!(
            state.registered_finite_source_epoch(5, "track"),
            Some(captured_epoch)
        );
        assert_eq!(
            state.registered_resource_identity(5, "track"),
            Some(replacement.clone())
        );

        state.register(
            7,
            "chapter-track".into(),
            source(),
            Some(identity.clone()),
            false,
            Some((60.0, 90.0)),
        );
        assert!(state.matches_registered_finite_source(7, "chapter-track"));
        assert_eq!(
            state.registered_resource_identity(7, "chapter-track"),
            Some(identity)
        );
        state.register(
            8,
            "bad-bounds".into(),
            source(),
            Some(replacement),
            false,
            Some((90.0, 60.0)),
        );
        assert!(!state.matches_registered_finite_source(8, "bad-bounds"));

        state.register(6, "track".into(), source(), None, true, None);
        assert!(!state.matches_registered_finite_source(6, "track"));
        assert!(state.registered_resource_identity(6, "track").is_none());
    }

    #[tokio::test]
    async fn cancel_capture_keeps_source_and_cancels_jobs() {
        let state = WebViewIdentification::default();
        state.register(5, "track".into(), source(), None, false, None);
        let (tx, mut rx) = watch::channel(false);
        {
            let mut inner = state.0.lock().unwrap();
            inner.jobs.insert(99, tx);
        }
        state.cancel_capture();
        assert!(*rx.borrow_and_update());
        let inner = state.0.lock().unwrap();
        assert!(inner
            .source
            .as_ref()
            .is_some_and(|source| source.track_id == "track"));
    }

    #[test]
    fn explicit_capture_id_cancels_and_releases_a_finite_source_identification_job() {
        let state = WebViewIdentification::default();
        let capture_id = "playing-detection-capture".to_owned();
        let (sender, mut cancelled) = watch::channel(false);
        let job_id = 41;
        {
            let mut inner = state.0.lock().unwrap();
            inner.jobs.insert(job_id, sender);
            inner.pcm_capture_ids.insert(capture_id.clone(), job_id);
        }
        let guard = IdentificationJob(state.0.clone(), job_id, Some(capture_id.clone()));
        assert!(state.cancel_pcm_capture(&capture_id));
        assert!(*cancelled.borrow_and_update());
        drop(guard);
        let inner = state.0.lock().unwrap();
        assert!(!inner.jobs.contains_key(&job_id));
        assert!(!inner.pcm_capture_ids.contains_key(&capture_id));
    }

    #[tokio::test]
    async fn pcm_cancel_is_nonce_scoped_and_job_drop_cleans_registration() {
        let state = WebViewIdentification::default();
        let first = state.begin_pcm_capture("capture-1".into()).unwrap();
        let mut first_cancelled = first.cancellation();
        let second = state.begin_pcm_capture("capture-2".into()).unwrap();
        assert!(state.cancel_pcm_capture("capture-1"));
        assert!(*first_cancelled.borrow_and_update());
        assert!(!*second.cancellation().borrow());
        assert!(state.cancel_pcm_capture("stale-capture"));
        assert!(state.begin_pcm_capture("stale-capture".into()).is_err());
        drop(first);
        assert!(state.cancel_pcm_capture("capture-1"));
        assert!(state.begin_pcm_capture("capture-1".into()).is_err());
        assert!(state.cancel_pcm_capture("capture-2"));
    }

    #[tokio::test]
    async fn pcm_admission_is_bounded_and_reopens_after_worker_guard_drops() {
        let state = WebViewIdentification::default();
        let jobs = (0..MAX_ACTIVE_JOBS)
            .map(|index| state.begin_pcm_capture(format!("cap-{index}")).unwrap())
            .collect::<Vec<_>>();
        assert!(state.begin_pcm_capture("over-cap".into()).is_err());
        drop(jobs.into_iter().next());
        assert!(state.begin_pcm_capture("after-drop".into()).is_ok());
    }

    #[tokio::test]
    async fn shutdown_closes_admission_cancels_and_waits_for_guards_to_drop() {
        let state = WebViewIdentification::default();
        let job = state.begin_pcm_capture("shutdown-cap".into()).unwrap();
        let mut cancellation = job.cancellation();
        let shutdown_state = state.clone();
        let mut shutdown = tokio::spawn(async move { shutdown_state.shutdown_for_exit().await });
        tokio::time::timeout(Duration::from_secs(1), cancellation.changed())
            .await
            .expect("shutdown should cancel active work")
            .expect("registry remains alive");
        assert!(*cancellation.borrow());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut shutdown)
                .await
                .is_err(),
            "shutdown must not report drained before the worker guard drops"
        );
        drop(job);
        shutdown.await.unwrap().unwrap();
        assert!(state.begin_pcm_capture("after-shutdown".into()).is_err());
    }

    #[tokio::test]
    async fn cancelling_one_capture_does_not_cancel_an_independent_capture() {
        let state = WebViewIdentification::default();
        let first = state.begin_pcm_capture("first-independent".into()).unwrap();
        let second = state
            .begin_pcm_capture("second-independent".into())
            .unwrap();
        state.cancel_pcm_capture("first-independent");
        assert!(*first.cancellation().borrow());
        assert!(!*second.cancellation().borrow());
    }

    #[test]
    fn excerpt_window_stays_inside_absolute_chapter_bounds() {
        assert_eq!(
            excerpt_window(100.0, false, Some((90.0, 108.0))).unwrap(),
            (94.0, 12.0)
        );
        assert_eq!(
            excerpt_window(107.0, false, Some((90.0, 108.0))).unwrap(),
            (96.0, 12.0)
        );
        assert_eq!(excerpt_window(5.0, false, None).unwrap(), (0.0, 12.0));
        assert!(excerpt_window(110.0, false, Some((90.0, 108.0))).is_err());
    }

    #[test]
    fn playback_snapshot_rejects_seek_restart_or_engine_change() {
        let snapshot = serde_json::json!({
            "status": "playing", "trackId": "track", "sourceGeneration": 7,
            "seekSerial": 3, "chapterId": "chapter-a", "absolutePosition": 95.0
        });
        assert!(snapshot_matches_playback(
            &snapshot,
            false,
            "track",
            7,
            Some(3),
            Some("chapter-a")
        ));
        assert!(!snapshot_matches_playback(
            &snapshot,
            true,
            "track",
            7,
            Some(3),
            Some("chapter-a")
        ));
        assert!(!snapshot_matches_playback(
            &snapshot,
            false,
            "track",
            8,
            Some(3),
            Some("chapter-a")
        ));
        assert!(!snapshot_matches_playback(
            &snapshot,
            false,
            "track",
            7,
            Some(4),
            Some("chapter-a")
        ));
        assert!(!snapshot_matches_playback(
            &snapshot,
            false,
            "other",
            7,
            Some(3),
            Some("chapter-a")
        ));
        let mut next_chapter = snapshot.clone();
        next_chapter["chapterId"] = serde_json::json!("chapter-b");
        assert!(!snapshot_matches_playback(
            &next_chapter,
            false,
            "track",
            7,
            Some(3),
            Some("chapter-a")
        ));
    }

    #[test]
    fn handoff_registry_guard_requires_current_full_direct_youtube_source() {
        let registry = WebViewIdentification::default();
        let source = AudioSource {
            input: "https://rr1---sn.googlevideo.com/videoplayback?itag=140".into(),
            ffmpeg: PathBuf::from("ffmpeg"),
            headers: Vec::new(),
            decoder_hint: None,

            cenc: None,
        };
        registry.register(7, "youtube:video12345".into(), source, None, false, None);
        assert!(registry.matches_registered_youtube_source(7, "youtube:video12345"));
        assert!(!registry.matches_registered_youtube_source(8, "youtube:video12345"));
        assert!(!registry.matches_registered_youtube_source(7, "youtube:other"));

        let radio = AudioSource {
            input: "https://rr1---sn.googlevideo.com/videoplayback?itag=140".into(),
            ffmpeg: PathBuf::from("ffmpeg"),
            headers: Vec::new(),
            decoder_hint: None,

            cenc: None,
        };
        registry.register(9, "youtube:video12345".into(), radio, None, true, None);
        assert!(!registry.matches_registered_youtube_source(9, "youtube:video12345"));
    }

    #[test]
    fn temporary_capture_is_removed_on_drop() {
        let path = std::env::temp_dir().join(format!(
            "burrata-identification-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"temporary").unwrap();
        drop(TemporaryPcmFile(path.clone()));
        assert!(!path.exists());
    }
}
