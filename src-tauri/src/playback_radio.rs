//! Radio metadata/history worker attached to a playback session.
//!
//! Recognition prefers station-network metadata and falls back to SongRec on
//! the generation-scoped PCM that Burrata's output mixer actually consumes.

use crate::{
    pcm_tap::PcmTap,
    playback_history::{PlaybackHistory, PlaybackHistorySearchOptions},
    radio::{self, RadioStation, RadioTrackMetadata},
    storage::Store,
};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const HISTORY_LIMIT: usize = 100;

pub enum RadioEvent {
    Metadata {
        generation: u64,
        station: RadioStation,
        metadata: Option<RadioTrackMetadata>,
        history: Vec<Value>,
    },
}

/// Start station metadata polling for one playback generation. The caller owns
/// the audible signal and must drive it from actual output state (including
/// buffering and pause), rather than from the selected station UI state.
pub fn spawn(
    generation: u64,
    station: RadioStation,
    listening_session_id: String,
    tap: Arc<PcmTap>,
    store: Arc<Store>,
    history: Arc<PlaybackHistory>,
    mut audible: watch::Receiver<bool>,
    mut cancel: watch::Receiver<bool>,
    events: mpsc::UnboundedSender<RadioEvent>,
) {
    tauri::async_runtime::spawn(async move {
        let mut transitions = MetadataTransitions::default();
        let mut live_history: Vec<Value> = Vec::new();
        let mut audible_since: Option<Instant> = None;

        loop {
            if cancelled(&cancel) || events.is_closed() {
                break;
            }
            if !*audible.borrow() {
                // Require a fresh eight seconds of continuous output after a
                // pause/buffer gap before PCM can be sampled again.
                audible_since = None;
                tokio::select! {
                    biased;
                    changed = cancel.changed() => if changed.is_err() || *cancel.borrow() { break; },
                    changed = audible.changed() => if changed.is_err() { break; },
                }
                continue;
            }
            let audible_since = *audible_since.get_or_insert_with(Instant::now);

            // Treat each newly observed title/artist pair as one occurrence.
            // The value is retained through metadata expiry so repeated poll
            // responses cannot create duplicate history entries.
            let should_recognize = store
                .get_json_preference("radioRecognitionEnabled.v1")
                .ok()
                .flatten()
                .and_then(|v| v.as_bool().map(|enabled| enabled.to_owned()))
                .unwrap_or(true);
            let poll = radio::poll_radio_station(station.id.clone(), false, &store);
            let result = tokio::select! {
                biased;
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() { break; }
                    continue;
                }
                changed = audible.changed() => {
                    if changed.is_err() { break; }
                    continue;
                }
                result = poll => result,
            };
            if cancelled(&cancel) || !*audible.borrow() || events.is_closed() {
                continue;
            }

            let mut metadata = result.ok().and_then(|response| response.now_playing);
            let needs_pcm_identification = metadata
                .as_ref()
                .map_or(true, |item| item.artist.trim().is_empty());
            if needs_pcm_identification && should_recognize && pcm_window_ready(audible_since) {
                let station_reported = metadata.clone();
                let path = std::env::temp_dir()
                    .join(format!("burrata-played-radio-{}.wav", uuid::Uuid::new_v4()));
                let _cleanup = Arc::new(TemporaryPcmFile(path.clone()));
                let capture_tap = tap.clone();
                let capture_path = path.clone();
                let capture_cleanup = _cleanup.clone();
                let recognition = radio::recognize_played_sample_with_capture(
                    &station,
                    false,
                    move || async move {
                        let capture_path_for_worker = capture_path.clone();
                        tokio::task::spawn_blocking(move || {
                            // The worker clone owns cleanup if this async future
                            // is cancelled while capture_wav is still writing.
                            let _cleanup = capture_cleanup;
                            capture_tap.capture_wav(generation, &capture_path_for_worker, 12.0)
                        })
                        .await
                        .map_err(|error| format!("PCM capture worker stopped: {error}"))??;
                        Ok(capture_path)
                    },
                );
                let recognized = tokio::select! {
                    biased;
                    changed = cancel.changed() => {
                        if changed.is_err() || *cancel.borrow() { break; }
                        continue;
                    }
                    changed = audible.changed() => {
                        if changed.is_err() { break; }
                        continue;
                    }
                    result = recognition => result,
                };
                if cancelled(&cancel) || !*audible.borrow() || events.is_closed() {
                    continue;
                }
                metadata = recognized
                    .ok()
                    .and_then(|(track, _note)| track)
                    .or(station_reported);
            }
            if let Some(occurrence) = transitions.observe(metadata.as_ref()) {
                if let Some(item) = metadata.as_ref() {
                    // Timestamp the moment this generation observes the song
                    // while output is audible; station program timestamps may
                    // precede the listener actually hearing it.
                    let listened_at = chrono::Utc::now().to_rfc3339();
                    let occurrence_key = format!("{}:{}", generation, occurrence);
                    // Persistence is gated by the existing user preference in
                    // record_radio_occurrence, and uses the canonical radio song
                    // Track builder there.
                    let persisted = history
                        .record_radio_occurrence(
                            &store,
                            &listening_session_id,
                            &occurrence_key,
                            &listened_at,
                            &station,
                            item,
                        )
                        .unwrap_or(false);
                    if !persisted {
                        live_history.insert(
                            0,
                            json!({
                                "id": format!("radio-live:{listening_session_id}:{occurrence}"),
                                "track": crate::playback_history::radio_song_track(&station, item),
                                "playedAt": listened_at,
                                "stationName": station.name,
                            }),
                        );
                        live_history.truncate(HISTORY_LIMIT);
                    }
                }
            }

            if cancelled(&cancel) || !*audible.borrow() || events.is_closed() {
                continue;
            }
            let mut recent_history = history_for_station(&history, &store, &station);
            for entry in live_history.iter().rev() {
                if !recent_history
                    .iter()
                    .any(|saved| saved["id"] == entry["id"])
                {
                    recent_history.insert(0, entry.clone());
                }
            }
            recent_history.truncate(HISTORY_LIMIT);
            if cancelled(&cancel) || !*audible.borrow() || events.is_closed() {
                continue;
            }
            let _ = events.send(RadioEvent::Metadata {
                generation,
                station: station.clone(),
                metadata,
                history: recent_history,
            });

            tokio::select! {
                biased;
                changed = cancel.changed() => if changed.is_err() || *cancel.borrow() { break; },
                changed = audible.changed() => if changed.is_err() { break; },
                _ = tokio::time::sleep(POLL_INTERVAL) => {},
            }
        }
    });
}

fn cancelled(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

fn pcm_window_ready(audible_since: Instant) -> bool {
    audible_since.elapsed() >= Duration::from_secs(8)
}

struct TemporaryPcmFile(std::path::PathBuf);

impl Drop for TemporaryPcmFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn valid_track(metadata: &RadioTrackMetadata) -> bool {
    !metadata.title.trim().is_empty()
}

#[derive(Default)]
struct MetadataTransitions {
    current: Option<(String, String)>,
    occurrence: u64,
}

impl MetadataTransitions {
    /// Returns the new occurrence number only on a track transition. Callers
    /// skip this while paused or buffering, so silence in the observation stream
    /// does not create an artificial expiry.
    fn observe(&mut self, metadata: Option<&RadioTrackMetadata>) -> Option<u64> {
        let Some(metadata) = metadata.filter(|item| valid_track(item)) else {
            // Missing metadata on an active poll ends this occurrence. If the
            // same song later reappears, it receives a new occurrence key.
            self.current = None;
            return None;
        };
        let identity = (
            metadata.artist.trim().to_lowercase(),
            metadata.title.trim().to_lowercase(),
        );
        if self.current.as_ref() == Some(&identity) {
            return None;
        }
        self.current = Some(identity);
        self.occurrence = self.occurrence.saturating_add(1);
        Some(self.occurrence)
    }
}

pub fn history_for_station(
    history: &PlaybackHistory,
    store: &Store,
    station: &RadioStation,
) -> Vec<Value> {
    history
        .search(
            store,
            PlaybackHistorySearchOptions {
                provider: Some("radio".into()),
                station_id: Some(station.id.clone()),
                limit: HISTORY_LIMIT as u32,
                ..PlaybackHistorySearchOptions::default()
            },
        )
        .map(|page| page.items)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.session_id,
                "track": entry.track,
                "playedAt": entry.played_at,
                "stationName": station.name,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str) -> RadioTrackMetadata {
        RadioTrackMetadata {
            station_id: "test".into(),
            artist: artist.into(),
            title: title.into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: "https://example.test/metadata".into(),
            observed_at: None,
            metadata_confidence: "station metadata".into(),
        }
    }

    #[test]
    fn repeated_metadata_is_one_occurrence_and_expiry_allows_reappearance() {
        let song = track("Artist", "Song");
        let mut transitions = MetadataTransitions::default();
        assert_eq!(transitions.observe(Some(&song)), Some(1));
        assert_eq!(transitions.observe(Some(&song)), None);
        assert_eq!(transitions.observe(None), None);
        assert_eq!(transitions.observe(Some(&song)), Some(2));
    }

    #[test]
    fn track_change_starts_new_occurrence_but_unobserved_pause_preserves_it() {
        let first = track("Artist", "First");
        let second = track("Artist", "Second");
        let mut transitions = MetadataTransitions::default();
        assert_eq!(transitions.observe(Some(&first)), Some(1));
        // While paused/buffering the worker does not call observe.
        assert_eq!(transitions.observe(Some(&first)), None);
        assert_eq!(transitions.observe(Some(&second)), Some(2));
    }

    #[test]
    fn pcm_capture_waits_for_eight_seconds_of_continuous_audible_output() {
        let now = Instant::now();
        assert!(!pcm_window_ready(now));
        assert!(pcm_window_ready(now - Duration::from_secs(8)));
    }
}
