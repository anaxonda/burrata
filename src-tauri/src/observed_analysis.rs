//! Whole-track analysis of audio already consumed by native playback.
//! No network requests, audio uploads, retagging, or library publication.
use crate::{
    models::{Candidate, Track},
    pcm_tap::{PcmTap, PcmTapEvent},
    stream_analysis::{
        AnalysisResource, PcmBatch, SessionOnlyStreamAnalysisResults, StreamAnalysisCache,
        StreamAudioVariant, WholeTrackAnalysis, WholeTrackCapture, KEY_PCM_RATE,
    },
    Store,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const CACHE_KEY: &str = "streamAnalysisCache.v1";
const MAX_ACTIVE_CAPTURES: usize = 2;
const SUPERSEDED_END_GRACE: Duration = Duration::from_secs(10);

pub struct ObservedAnalysis {
    store: Arc<Store>,
    cache: Mutex<StreamAnalysisCache>,
    session_only: Mutex<SessionOnlyStreamAnalysisResults>,
    captures: Mutex<HashMap<u64, ActiveCapture>>,
    webview_captures: Mutex<HashMap<String, WebviewCaptureSession>>,
}

struct ActiveCapture {
    owner: uuid::Uuid,
    resource_identity: String,
    cancel: tokio::sync::watch::Sender<bool>,
}

struct WebviewCaptureSession {
    generation: u64,
    expires_at: Instant,
    capture: WholeTrackCapture,
}

fn prune_expired_webview_captures(
    sessions: &mut HashMap<String, WebviewCaptureSession>,
    now: Instant,
) -> usize {
    let before = sessions.len();
    sessions.retain(|_, session| session.expires_at > now);
    before - sessions.len()
}

pub fn resource_for(track: &Track, candidate: Option<&Candidate>) -> Option<AnalysisResource> {
    // Only a resolver-provided durable audio identity qualifies. A catalog
    // recording ID alone cannot distinguish different acquired performances.
    let candidate = candidate?;
    if track.playback_range.is_some()
        || !matches!(
            candidate.provider.as_str(),
            "youtube" | "soundcloud" | "bandcamp" | "monochrome"
        )
    {
        return None;
    }
    let id = candidate.id.trim();
    if id.is_empty() || id.contains("://") || id.contains('?') || id.len() > 512 {
        return None;
    }
    Some(AnalysisResource::Stream {
        recording_id: track.id.clone(),
        provider: candidate.provider.clone(),
        source_identity_sha256: crate::stream_analysis::stream_source_identity_digest(
            &candidate.provider,
            id,
        )
        .ok()?,
        audio_variant_sha256: None,
    })
}

pub fn resource_for_variant(
    track: &Track,
    candidate: Option<&Candidate>,
    variant: Option<&StreamAudioVariant>,
) -> Option<AnalysisResource> {
    Some(resource_for(track, candidate)?.with_stream_audio_variant(variant))
}

impl ObservedAnalysis {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        let cache = store
            .get_json_preference(CACHE_KEY)
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_value::<StreamAnalysisCache>(value).ok())
            .filter(|cache| cache.validate().is_ok())
            .unwrap_or_default();
        Arc::new(Self {
            store,
            cache: Mutex::new(cache),
            session_only: Mutex::new(SessionOnlyStreamAnalysisResults::default()),
            captures: Mutex::new(HashMap::new()),
            webview_captures: Mutex::new(HashMap::new()),
        })
    }

    /// Start an explicit capture from the current WebView source. The caller
    /// supplies only a resolver candidate ID; raw URLs and request headers are
    /// never accepted or retained.
    pub fn begin_webview_capture(
        &self,
        session_id: &str,
        generation: u64,
        recording_id: &str,
        provider: &str,
        source_id: &str,
        duration_seconds: f64,
        initial_position_seconds: f64,
        audio_variant: Option<&StreamAudioVariant>,
    ) -> Result<(), String> {
        if uuid::Uuid::parse_str(session_id).is_err()
            || recording_id.trim().is_empty()
            || recording_id.len() > 512
            || !matches!(
                provider,
                "youtube" | "soundcloud" | "bandcamp" | "monochrome"
            )
            || source_id.trim().is_empty()
            || source_id.len() > 512
            || source_id.chars().any(char::is_control)
            || source_id.contains("://")
            || source_id.contains('?')
            || source_id.contains('#')
            || audio_variant.is_some_and(|variant| variant.identity_sha256().is_none())
        {
            return Err("WebView stream identity is not eligible for whole-track analysis".into());
        }
        let resource = AnalysisResource::Stream {
            recording_id: recording_id.to_owned(),
            provider: provider.to_owned(),
            source_identity_sha256: crate::stream_analysis::stream_source_identity_digest(
                provider, source_id,
            )
            .map_err(|error| error.to_string())?,
            audio_variant_sha256: None,
        }
        .with_stream_audio_variant(audio_variant);
        let capture = WholeTrackCapture::new(
            generation,
            resource,
            duration_seconds,
            initial_position_seconds,
        )
        .map_err(|error| error.to_string())?;
        let mut sessions = self
            .webview_captures
            .lock()
            .map_err(|_| "WebView analysis capture is unavailable")?;
        let now = Instant::now();
        prune_expired_webview_captures(&mut sessions, now);
        if sessions.contains_key(session_id) {
            return Err("WebView analysis capture ID is already active".into());
        }
        if sessions.len() >= MAX_ACTIVE_CAPTURES {
            return Err("Too many WebView analysis captures are active".into());
        }
        sessions.insert(
            session_id.to_owned(),
            WebviewCaptureSession {
                generation,
                expires_at: now
                    + Duration::from_secs((duration_seconds.ceil() as u64).saturating_add(300)),
                capture,
            },
        );
        Ok(())
    }

    /// Accept one bounded mono PCM chunk from the active WebAudio observer.
    /// Any malformed or discontinuous chunk destroys the session so it cannot
    /// later be mistaken for a complete resource.
    pub fn push_webview_capture(
        &self,
        session_id: &str,
        generation: u64,
        end_position_seconds: f64,
        sample_rate: u32,
        samples: Vec<f32>,
    ) -> Result<(), String> {
        if sample_rate != KEY_PCM_RATE
            || samples.is_empty()
            || samples.len() > KEY_PCM_RATE as usize
        {
            self.cancel_webview_capture(session_id);
            return Err("WebView analysis PCM chunk exceeds its format or size bound".into());
        }
        let mut sessions = self
            .webview_captures
            .lock()
            .map_err(|_| "WebView analysis capture is unavailable")?;
        let Some(session) = sessions.get_mut(session_id) else {
            return Err("WebView analysis capture is no longer active".into());
        };
        if session.expires_at <= Instant::now() {
            sessions.remove(session_id);
            return Err("WebView analysis capture expired before completion".into());
        }
        if session.generation != generation {
            sessions.remove(session_id);
            return Err("WebView playback generation changed during analysis".into());
        }
        let result = session.capture.push(PcmBatch {
            generation,
            sample_rate,
            channels: 1,
            end_position_seconds,
            interleaved: &samples,
        });
        if let Err(error) = result {
            sessions.remove(session_id);
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn finish_webview_capture(
        &self,
        session_id: &str,
        generation: u64,
        natural_end_position_seconds: f64,
    ) -> Result<WholeTrackAnalysis, String> {
        let session = {
            let mut sessions = self
                .webview_captures
                .lock()
                .map_err(|_| "WebView analysis capture is unavailable")?;
            let session = sessions
                .remove(session_id)
                .ok_or("WebView analysis capture is no longer active")?;
            if session.expires_at <= Instant::now() {
                return Err("WebView analysis capture expired before completion".into());
            }
            session
        };
        if session.generation != generation {
            return Err("WebView playback generation changed before analysis completed".into());
        }
        let record = session
            .capture
            .finish(
                natural_end_position_seconds,
                chrono::Utc::now().to_rfc3339(),
            )
            .map_err(|error| error.to_string())?;
        self.cache_record(record.clone())?;
        Ok(record)
    }

    pub fn cancel_webview_capture(&self, session_id: &str) {
        if let Ok(mut sessions) = self.webview_captures.lock() {
            sessions.remove(session_id);
        }
    }

    pub fn cached(
        &self,
        track: &Track,
        candidate: Option<&Candidate>,
    ) -> Option<WholeTrackAnalysis> {
        let resource = resource_for(track, candidate)?;
        self.cache.lock().ok()?.get(&resource).cloned()
    }

    /// Return a completed result only for the exact native playback generation.
    /// Variant-unknown records are session-only and never enter durable cache.
    pub fn session_cached(
        &self,
        generation: u64,
        track: &Track,
        candidate: Option<&Candidate>,
    ) -> Option<WholeTrackAnalysis> {
        let resource = resource_for(track, candidate)?;
        self.session_only.lock().ok()?.get(generation, &resource)
    }

    /// Return a cached combined BPM/key/waveform result for the exact current
    /// stable provider identity. The source identifier is hashed immediately
    /// and is never stored as a URL or included in the response.
    pub fn cached_stream_media(
        &self,
        track: &Track,
        provider: &str,
        source_id: &str,
        duration_seconds: f64,
        audio_variant: Option<&StreamAudioVariant>,
    ) -> Option<WholeTrackAnalysis> {
        if track.id.trim().is_empty()
            || track.playback_range.is_some()
            || track.local_path.is_some()
            || track.asset_id.is_some()
            || source_id.trim().is_empty()
            || source_id.len() > 512
            || source_id.contains("://")
            || source_id.contains('?')
            || source_id.contains('#')
            || !duration_seconds.is_finite()
            || !(8.0..=crate::stream_analysis::MAX_TRACK_SECONDS).contains(&duration_seconds)
            || !matches!(
                provider,
                "youtube" | "soundcloud" | "bandcamp" | "monochrome"
            )
        {
            return None;
        }
        let resource = AnalysisResource::Stream {
            recording_id: track.id.clone(),
            provider: provider.to_owned(),
            source_identity_sha256: crate::stream_analysis::stream_source_identity_digest(
                provider, source_id,
            )
            .ok()?,
            audio_variant_sha256: None,
        }
        .with_stream_audio_variant(audio_variant);
        let record = self.cache.lock().ok()?.get(&resource)?.clone();
        let tolerance = 0.05_f64.max(duration_seconds * 0.001);
        if (record.duration_seconds - duration_seconds).abs() > tolerance {
            return None;
        }
        let waveform = record.waveform.as_ref()?;
        waveform.validate(record.duration_seconds).ok()?;
        Some(record)
    }

    /// Persist an explicitly reviewed whole-track stream measurement in the
    /// same versioned cache used by complete native playback captures.
    pub fn cache_record(&self, record: WholeTrackAnalysis) -> Result<(), String> {
        record.validate().map_err(|error| error.to_string())?;
        // Old persisted stream records are retained for compatibility, but a
        // recording ID without a stable encoded-variant identity is not safe
        // to reuse or replace in the persistent cache.
        if matches!(&record.resource, AnalysisResource::Stream { .. })
            && !record.resource.has_stable_audio_variant()
        {
            return Ok(());
        }
        let _writer_lease = self.store.try_library_write()?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| "Stream analysis cache is unavailable")?;
        cache.insert(record).map_err(|error| error.to_string())?;
        let value = serde_json::to_value(&*cache)
            .map_err(|_| "Could not serialize stream analysis cache")?;
        self.store
            .set_json_preference(CACHE_KEY, &value)
            .map_err(|_| "Could not save stream analysis cache".to_owned())
    }

    pub fn cached_resource(&self, resource: &AnalysisResource) -> Option<WholeTrackAnalysis> {
        self.cache.lock().ok()?.get(resource).cloned()
    }

    pub fn begin(
        self: &Arc<Self>,
        tap: Arc<PcmTap>,
        generation: u64,
        track: &Track,
        candidate: Option<&Candidate>,
        duration: f64,
        initial_position: f64,
    ) {
        self.begin_inner(
            tap,
            generation,
            track,
            candidate,
            duration,
            initial_position,
        );
    }

    /// Start observing a speculative gapless generation before its decoder can
    /// produce PCM. It must not supersede the currently playing generation.
    pub fn begin_preload(
        self: &Arc<Self>,
        tap: Arc<PcmTap>,
        generation: u64,
        track: &Track,
        candidate: Option<&Candidate>,
        duration: f64,
        initial_position: f64,
    ) {
        self.begin_inner(
            tap,
            generation,
            track,
            candidate,
            duration,
            initial_position,
        );
    }

    /// Explicitly discard a generation that cannot reach verified natural end
    /// (for example, a failed or abandoned load). Supersession alone does not
    /// call this because gapless output may have ended the prior track first.
    pub fn cancel_generation(&self, generation: u64) {
        if let Ok(mut captures) = self.captures.lock() {
            if let Some(cancel) = captures.remove(&generation) {
                cancel.cancel.send_replace(true);
            }
            // Keep the owner registry locked while removing its result. A
            // concurrent finisher must either commit before this point (and
            // then be removed) or observe that its owner has been revoked.
            if let Ok(mut results) = self.session_only.lock() {
                results.remove_generation(generation);
            }
        }
    }

    fn register_capture(
        &self,
        generation: u64,
        resource_identity: String,
        cancel: tokio::sync::watch::Sender<bool>,
    ) -> Option<uuid::Uuid> {
        let mut captures = self.captures.lock().ok()?;
        if captures.contains_key(&generation) || captures.len() >= MAX_ACTIVE_CAPTURES {
            return None;
        }
        let owner = uuid::Uuid::new_v4();
        captures.insert(
            generation,
            ActiveCapture {
                owner,
                resource_identity,
                cancel,
            },
        );
        Some(owner)
    }

    fn release_capture(&self, generation: u64, owner: uuid::Uuid) {
        if let Ok(mut captures) = self.captures.lock() {
            if captures
                .get(&generation)
                .is_some_and(|capture| capture.owner == owner)
            {
                captures.remove(&generation);
            }
        }
    }

    fn commit_owned(&self, generation: u64, owner: uuid::Uuid, record: WholeTrackAnalysis) -> bool {
        let Ok(captures) = self.captures.lock() else {
            return false;
        };
        if !captures.get(&generation).is_some_and(|capture| {
            capture.owner == owner
                && record.resource.cache_identity().ok().as_deref()
                    == Some(capture.resource_identity.as_str())
        }) {
            return false;
        }
        if matches!(&record.resource, AnalysisResource::Stream { .. })
            && !record.resource.has_stable_audio_variant()
        {
            return match self.session_only.lock() {
                Ok(mut results) => results.insert(generation, record).is_ok(),
                Err(_) => false,
            };
        }
        self.cache_record(record).is_ok()
    }

    fn begin_inner(
        self: &Arc<Self>,
        tap: Arc<PcmTap>,
        generation: u64,
        track: &Track,
        candidate: Option<&Candidate>,
        duration: f64,
        initial_position: f64,
    ) {
        let Some(resource) = resource_for(track, candidate) else {
            return;
        };
        let Ok(resource_identity) = resource.cache_identity() else {
            return;
        };
        // A generation may be reused by a resolver refresh. Revoke any prior
        // capture/result for it before binding a new source owner.
        self.cancel_generation(generation);
        if self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&resource).cloned())
            .is_some()
        {
            return;
        }
        let Ok(mut capture) =
            WholeTrackCapture::new(generation, resource, duration, initial_position)
        else {
            return;
        };
        let capture_deadline = tokio::time::Instant::now()
            + Duration::from_secs((duration.ceil() as u64).saturating_add(300));
        let Ok(mut subscription) = tap.subscribe_until_verified_end(generation, 512) else {
            return;
        };
        let (cancel, mut cancel_rx) = tokio::sync::watch::channel(false);
        let Some(owner) = self.register_capture(generation, resource_identity, cancel) else {
            return;
        };
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut superseded_deadline: Option<tokio::time::Instant> = None;
            loop {
                let event = tokio::select! {
                    biased;
                    _ = cancel_rx.wait_for(|cancelled| *cancelled) => break,
                    _ = async {
                        tokio::time::sleep_until(
                            superseded_deadline
                                .map_or(capture_deadline, |deadline| deadline.min(capture_deadline)),
                        )
                        .await
                    } => break,
                    event = subscription.recv() => match event {
                        Some(event) => event,
                        None => break,
                    },
                };
                match event {
                    PcmTapEvent::Samples {
                        generation,
                        sample_rate,
                        channels,
                        end_position_seconds: Some(end),
                        samples,
                        ..
                    } => {
                        if capture
                            .push(PcmBatch {
                                generation,
                                sample_rate,
                                channels,
                                end_position_seconds: end,
                                interleaved: &samples,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    PcmTapEvent::Superseded { generation: ended } if ended == generation => {
                        superseded_deadline.get_or_insert_with(|| {
                            tokio::time::Instant::now() + SUPERSEDED_END_GRACE
                        });
                    }
                    PcmTapEvent::VerifiedEnd {
                        generation: ended,
                        duration_seconds,
                    } if ended == generation => {
                        // Key estimation is CPU work; do not run it on the audio
                        // worker or async runtime. Cache commits remain serialized.
                        let commit_manager = manager.clone();
                        let _ = tauri::async_runtime::spawn_blocking(move || {
                            let Ok(record) =
                                capture.finish(duration_seconds, chrono::Utc::now().to_rfc3339())
                            else {
                                return;
                            };
                            commit_manager.commit_owned(generation, owner, record);
                        })
                        .await;
                        break;
                    }
                    _ => break, // Gaps, unpositioned audio, or generation mismatch.
                }
            }
            manager.release_capture(generation, owner);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn session_only_fixture_record() -> WholeTrackAnalysis {
        use crate::stream_analysis::{
            BpmMeasurement, KeyMeasurement, ANALYZER_VERSION, SCHEMA_VERSION,
        };

        WholeTrackAnalysis {
            schema_version: SCHEMA_VERSION,
            analyzer_version: ANALYZER_VERSION.into(),
            resource: AnalysisResource::Stream {
                recording_id: "fixture:recording-1".into(),
                provider: "youtube".into(),
                source_identity_sha256: crate::stream_analysis::stream_source_identity_digest(
                    "youtube",
                    "youtube:fixture-video-1",
                )
                .unwrap(),
                audio_variant_sha256: None,
            },
            duration_seconds: 32.0,
            scope: "wholeTrack".into(),
            coverage: 1.0,
            measured_at: "2026-10-27T00:00:00Z".into(),
            bpm: Some(BpmMeasurement {
                bpm: 120.0,
                confidence: 0.8,
                related_tempo_alternative_bpm: Some(60.0),
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

    fn session_fixture_candidate() -> Candidate {
        serde_json::from_value(serde_json::json!({
            "id":"youtube:fixture-video-1",
            "provider":"youtube",
            "url":"",
            "title":"Fixture Recording",
            "uploader":null,
            "durationSeconds":32.0,
            "artworkUrl":null,
            "formats":[],
            "permission":"",
            "confidence":"",
            "score":0,
            "reasons":[],
            "official":false
        }))
        .unwrap()
    }

    #[test]
    fn catalog_identity_or_signed_url_cannot_authorize_stream_cache() {
        let track = Track {
            id: "recording".into(),
            ..Default::default()
        };
        assert!(resource_for(&track, None).is_none());
        let mut candidate: Candidate = serde_json::from_value(serde_json::json!({"id":"https://example.com/audio?token=secret","provider":"youtube","url":"","title":"","uploader":null,"durationSeconds":null,"artworkUrl":null,"formats":[],"permission":"","confidence":"","score":0,"reasons":[],"official":false})).unwrap();
        assert!(resource_for(&track, Some(&candidate)).is_none());
        candidate.id = "verified-video-id".into();
        assert!(resource_for(&track, Some(&candidate)).is_some());
    }

    #[test]
    fn native_session_result_is_generation_and_candidate_bound_without_store_write() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let observed = ObservedAnalysis::new(store.clone());
        let track = Track {
            id: "fixture:recording-1".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-1".into(),
            ..Track::default()
        };
        let candidate = session_fixture_candidate();
        let record = session_only_fixture_record();
        let mut replacement_candidate = candidate.clone();
        replacement_candidate.id = "youtube:fixture-video-2".into();
        let mut replacement_record = record.clone();
        replacement_record.resource = resource_for(&track, Some(&replacement_candidate)).unwrap();

        let (old_cancel, _old_cancel_rx) = tokio::sync::watch::channel(false);
        let old_owner = observed
            .register_capture(73, record.resource.cache_identity().unwrap(), old_cancel)
            .unwrap();
        observed.cancel_generation(73);

        // A replacement may reuse a generation while finishing work from the
        // previous owner. That old work must never publish into the new owner.
        let (new_cancel, _new_cancel_rx) = tokio::sync::watch::channel(false);
        let new_owner = observed
            .register_capture(
                73,
                replacement_record.resource.cache_identity().unwrap(),
                new_cancel,
            )
            .unwrap();
        assert!(!observed.commit_owned(73, old_owner, record.clone()));
        assert!(!observed.commit_owned(73, new_owner, record.clone()));
        assert!(observed.commit_owned(73, new_owner, replacement_record.clone()));
        observed.release_capture(73, new_owner);
        assert_eq!(
            observed.session_cached(73, &track, Some(&replacement_candidate)),
            Some(replacement_record)
        );
        assert!(observed
            .session_cached(74, &track, Some(&candidate))
            .is_none());

        let mut different_candidate = candidate.clone();
        different_candidate.id = "youtube:other-video".into();
        assert!(observed
            .session_cached(73, &track, Some(&different_candidate))
            .is_none());
        assert!(observed.cached(&track, Some(&candidate)).is_none());

        let excerpt = Track {
            playback_range: Some(crate::models::PlaybackRange {
                parent_track_id: track.id.clone(),
                chapter_id: "chapter-1".into(),
                source_url: "https://example.invalid/ignored".into(),
                start_time: 4.0,
                end_time: 12.0,
                parent_title: "Fixture Recording".into(),
                parent_duration_seconds: Some(32.0),
            }),
            ..track.clone()
        };
        assert!(observed
            .session_cached(73, &excerpt, Some(&candidate))
            .is_none());

        let mut radio_candidate = candidate.clone();
        radio_candidate.provider = "radio".into();
        assert!(observed
            .session_cached(73, &track, Some(&radio_candidate))
            .is_none());

        assert!(store.get_json_preference(CACHE_KEY).unwrap().is_none());
        observed.cancel_generation(73);
        assert!(observed
            .session_cached(73, &track, Some(&candidate))
            .is_none());
        assert!(store.get_json_preference(CACHE_KEY).unwrap().is_none());
    }

    #[test]
    fn explicit_webview_capture_rejects_urls_and_drops_discontinuous_pcm() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let observed = ObservedAnalysis::new(store);
        let session = uuid::Uuid::new_v4().to_string();
        assert!(observed
            .begin_webview_capture(
                &session,
                7,
                "recording",
                "youtube",
                "https://example.test/audio?token=secret",
                8.0,
                0.0,
                None,
            )
            .is_err());

        observed
            .begin_webview_capture(
                &session,
                7,
                "recording",
                "youtube",
                "durable-video-id",
                8.0,
                0.0,
                None,
            )
            .unwrap();
        let first = vec![0.0; KEY_PCM_RATE as usize / 4];
        observed
            .push_webview_capture(&session, 7, 0.25, KEY_PCM_RATE, first)
            .unwrap();
        let skipped = vec![0.0; KEY_PCM_RATE as usize / 4];
        assert!(observed
            .push_webview_capture(&session, 7, 0.75, KEY_PCM_RATE, skipped)
            .is_err());
        assert!(observed.finish_webview_capture(&session, 7, 8.0).is_err());
    }

    #[test]
    fn expired_webview_sessions_are_pruned_before_capacity_check() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let observed = ObservedAnalysis::new(store);
        let expired_id = uuid::Uuid::new_v4().to_string();
        observed
            .begin_webview_capture(
                &expired_id,
                1,
                "recording-old",
                "youtube",
                "durable-old-id",
                8.0,
                0.0,
                None,
            )
            .unwrap();
        {
            let mut sessions = observed.webview_captures.lock().unwrap();
            sessions.get_mut(&expired_id).unwrap().expires_at =
                Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        }

        let replacement_id = uuid::Uuid::new_v4().to_string();
        observed
            .begin_webview_capture(
                &replacement_id,
                2,
                "recording-new",
                "youtube",
                "durable-new-id",
                8.0,
                0.0,
                None,
            )
            .unwrap();
        let sessions = observed.webview_captures.lock().unwrap();
        assert!(!sessions.contains_key(&expired_id));
        assert!(sessions.contains_key(&replacement_id));
    }

    #[test]
    fn full_stream_media_cache_is_available_only_for_exact_provider_resource() {
        use crate::stream_analysis::{
            AnalysisResource, BpmMeasurement, KeyMeasurement, StreamAudioVariant,
            StreamDurationPrecision, StreamWaveform, StreamWaveformBin, WholeTrackAnalysis,
            ANALYZER_VERSION, SCHEMA_VERSION, STREAM_WAVEFORM_ANALYZER_VERSION,
            STREAM_WAVEFORM_BINS,
        };

        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let observed = ObservedAnalysis::new(store);
        let track = Track {
            id: "fixture:recording-1".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-1".into(),
            title: "Fixture Recording".into(),
            artist: "Fixture Artist".into(),
            ..Track::default()
        };
        let variant = StreamAudioVariant {
            format_id: Some("140".into()),
            codec: Some("aac".into()),
            mime_type: Some("audio/mp4".into()),
            expected_content_length: Some(4_096),
            contains_video: Some(false),
            ..StreamAudioVariant::default()
        };
        let resource = AnalysisResource::Stream {
            recording_id: "fixture:recording-1".into(),
            provider: "youtube".into(),
            source_identity_sha256: crate::stream_analysis::stream_source_identity_digest(
                "youtube",
                "youtube:video-1",
            )
            .unwrap(),
            audio_variant_sha256: variant.identity_sha256(),
        };
        observed
            .cache_record(WholeTrackAnalysis {
                schema_version: SCHEMA_VERSION,
                analyzer_version: ANALYZER_VERSION.into(),
                resource,
                duration_seconds: 32.3647,
                scope: "wholeTrack".into(),
                coverage: 1.0,
                measured_at: "2026-10-25T12:00:00Z".into(),
                bpm: Some(BpmMeasurement {
                    bpm: 120.0,
                    confidence: 0.9,
                    related_tempo_alternative_bpm: Some(60.0),
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
                waveform: Some(StreamWaveform {
                    analyzer_version: STREAM_WAVEFORM_ANALYZER_VERSION.into(),
                    bin_count: STREAM_WAVEFORM_BINS,
                    duration_seconds: 32.3647,
                    bins: vec![
                        StreamWaveformBin {
                            min: -0.25,
                            max: 0.25
                        };
                        STREAM_WAVEFORM_BINS
                    ],
                    scope: "wholeTrack".into(),
                    complete: true,
                    measured_at: "2026-10-25T12:00:00Z".into(),
                }),
            })
            .unwrap();

        assert!(observed
            .cached_stream_media(
                &track,
                "youtube",
                "youtube:video-1",
                32.3647,
                Some(&variant)
            )
            .is_some());
        assert!(observed
            .cached_stream_media(&track, "youtube", "youtube:video-1", 32.0, Some(&variant))
            .is_none());
        assert!(observed
            .cached_stream_media(
                &track,
                "youtube",
                "youtube:video-2",
                32.3647,
                Some(&variant)
            )
            .is_none());
        assert!(observed
            .cached_stream_media(
                &track,
                "soundcloud",
                "youtube:video-1",
                32.3647,
                Some(&variant)
            )
            .is_none());
        let other_track = Track {
            id: "fixture:recording-2".into(),
            ..track.clone()
        };
        assert!(observed
            .cached_stream_media(
                &other_track,
                "youtube",
                "youtube:video-1",
                32.3647,
                Some(&variant)
            )
            .is_none());
        assert!(observed
            .cached_stream_media(&track, "youtube", "youtube:video-1", 33.0, Some(&variant))
            .is_none());
        let changed_variant = StreamAudioVariant {
            format_id: Some("251".into()),
            codec: Some("opus".into()),
            mime_type: Some("audio/webm".into()),
            ..StreamAudioVariant::default()
        };
        assert!(observed
            .cached_stream_media(
                &track,
                "youtube",
                "youtube:video-1",
                32.3647,
                Some(&changed_variant),
            )
            .is_none());
        let changed_length = StreamAudioVariant {
            expected_content_length: Some(4_097),
            ..variant.clone()
        };
        assert!(observed
            .cached_stream_media(
                &track,
                "youtube",
                "youtube:video-1",
                32.3647,
                Some(&changed_length),
            )
            .is_none());
        let refreshed_same_variant = StreamAudioVariant {
            bitrate_kbps: Some(128.5),
            ..variant.clone()
        };
        assert!(observed
            .cached_stream_media(
                &track,
                "youtube",
                "youtube:video-1",
                32.3647,
                Some(&refreshed_same_variant),
            )
            .is_some());
        assert!(observed
            .cached_stream_media(&track, "youtube", "youtube:video-1", 32.3647, None)
            .is_none());

        let candidate = Candidate {
            id: "youtube:video-1".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let rounded_metadata_cache = observed
            .cached_stream_media(&track, "youtube", &candidate.id, 32.3647, Some(&variant))
            .unwrap();
        assert!(StreamDurationPrecision::RoundedToSecond
            .accepts(32.0, rounded_metadata_cache.duration_seconds));
        assert!(
            !StreamDurationPrecision::Exact.accepts(32.0, rounded_metadata_cache.duration_seconds)
        );

        let other_candidate = Candidate {
            id: "youtube:video-2".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        assert!(observed.cached(&track, Some(&other_candidate)).is_none());
        let wrong_provider_candidate = Candidate {
            id: "youtube:video-1".into(),
            provider: "soundcloud".into(),
            ..Candidate::default()
        };
        assert!(observed
            .cached(&track, Some(&wrong_provider_candidate))
            .is_none());
    }
}
