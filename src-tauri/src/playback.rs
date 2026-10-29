//! Authoritative native playback session shared by the UI, automation and media controls.
use crate::{
    acquisition::AcquisitionManager,
    models::{Candidate, Track},
    native_output::{
        BufferingDiagnostics, DecoderDiagnostics, NativeOutput, OutputCommand, OutputEvent,
    },
    playback_history::PlaybackHistory,
    playback_radio::RadioEvent,
    playing_track_identification::PlayingTrackIdentifications,
    radio::RadioStation,
    storage::Store,
};
use serde::Serialize;
use serde_json::{json, Value};
#[path = "failed_track_traversal.rs"]
mod failed_track_traversal;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::Emitter;
use tokio::sync::{mpsc, watch};

const SESSION_KEY: &str = "nativePlaybackSession.v1";
const QUEUE_KEY: &str = "nativePlaybackQueue.v1";
const ENGINE_KEY: &str = "playbackEngine.v1";
const STEADY_BUFFERING_RECOVERY_DELAY: Duration = Duration::from_secs(5);
const RECURRING_STALL_WINDOW: Duration = Duration::from_secs(90);
const RECOVERY_HEALTHY_RESET: Duration = Duration::from_secs(60);
const MAX_RECURRING_SOURCE_SWAPS: u8 = 2;
const MAX_PENDING_NATIVE_SOURCES: usize = 4;
const PENDING_NATIVE_SOURCE_TTL: Duration = Duration::from_secs(30);

fn preparation_window_seconds(override_value: Option<&str>) -> f64 {
    match override_value {
        Some("45") => 45.,
        _ => 30.,
    }
}

/// Queue-owned preparation only: stable identity facts and anonymous search
/// rows. It never resolves an expiring URL, extracts audio or starts a decoder.
pub(crate) async fn warm_upcoming_playback_identity(
    acquisition: &AcquisitionManager,
    store: &Store,
    track: &Track,
    settings: &crate::models::Settings,
    providers: &[String],
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
    trace: &crate::playback_sources::SourceTrace,
) -> Result<usize, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Playback candidate preparation deadline expired".into());
    }
    if track.local_path.is_some()
        || track.playback_range.is_some()
        || crate::playback_sources::source_resolution_is_pinned(track, false, settings, true)
    {
        return Ok(0);
    }
    let identity = crate::recording_identity::RecordingIdentity::from_track(track);
    let aliases_missing =
        !crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
            .has_verified_credit_sets();
    let alias_eligible = crate::artist_alias_lookup::enabled()
        && aliases_missing
        && (track.provider == "spotify"
            || track.provider == "catalog"
                && matches!(
                    track.provenance["catalogProvider"].as_str(),
                    Some("musicbrainz" | "discogs")
                )
            || matches!(
                track.provider.as_str(),
                "beatport" | "traxsource" | "local" | "radio_song"
            ) && identity.isrc.is_some()
                && !identity.identifier_conflict);
    let duration_missing = track.duration_seconds.is_none()
        && track.provenance["manualFields"]["durationSeconds"].as_bool() != Some(true);
    let raw_search = async {
        let mut count = 0;
        // Keep the configured provider order; each provider warms just its
        // canonical query so late aliases retain the existing expansion slot.
        for provider in providers {
            if !matches!(provider.as_str(), "youtube" | "soundcloud" | "monochrome") {
                continue;
            }
            count += acquisition
                .warm_canonical_playback_query_cancelled(
                    track,
                    provider,
                    settings,
                    cancel.clone(),
                    deadline,
                )
                .await?;
        }
        Ok(count)
    };
    let identity_preparation = async {
        let alias_work = async {
            if alias_eligible {
                let completed = crate::artist_alias_lookup::lookup(
                    acquisition,
                    track,
                    cancel.clone(),
                    deadline,
                    trace,
                )
                .await
                .is_ok();
                completed
                    && crate::artist_credit_aliases::cached_verified_playback_artist_aliases(
                        store, track,
                    )
                    .has_verified_credit_sets()
            } else {
                !aliases_missing
            }
        };
        let duration_work = async {
            if duration_missing {
                matches!(
                    acquisition
                        .prepare_catalog_duration_track(track, cancel.clone(), deadline.instant())
                        .await,
                    Ok(Some(_))
                )
            } else {
                false
            }
        };
        let (alias_ready, duration_ready) = tokio::join!(alias_work, duration_work);
        if !*cancel.borrow() && !deadline.remaining().is_zero() {
            trace.event("next_preparation_identity_end",&track.provider,json!({"aliasesRequested":alias_eligible,"aliasesReady":alias_ready,"durationRequested":duration_missing,"durationReady":duration_ready}));
        }
    };
    finish_upcoming_preparation(cancel.clone(), deadline, raw_search, identity_preparation).await
}

async fn finish_upcoming_preparation(
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
    raw_search: impl std::future::Future<Output = Result<usize, String>>,
    identity_preparation: impl std::future::Future<Output = ()>,
) -> Result<usize, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Playback candidate preparation deadline expired".into());
    }
    let result = tokio::select! {
        biased;
        _=crate::playback_sources::wait_for_cancellation(cancel.clone())=>return Err("Cancelled".into()),
        _=tokio::time::sleep_until(deadline.instant())=>return Err("Playback candidate preparation deadline expired".into()),
        result=async { let (raw,())=tokio::join!(raw_search,identity_preparation);raw }=>result,
    };
    // Optional identity services validate their own app-owned fingerprints and
    // cancellation before retaining proof. This fence also prevents a stale
    // occurrence from reporting readiness to its owner.
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Playback candidate preparation deadline expired".into());
    }
    result
}

fn configured_preparation_window_seconds() -> f64 {
    #[cfg(debug_assertions)]
    {
        preparation_window_seconds(
            std::env::var("BURRATA_DIAGNOSTIC_NEXT_PREPARATION_SECONDS")
                .ok()
                .as_deref(),
        )
    }
    #[cfg(not(debug_assertions))]
    {
        30.
    }
}

fn startup_deadline(request_started: Instant, first_audio_seen: bool) -> Instant {
    if first_audio_seen {
        Instant::now() + crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE
    } else {
        request_started + crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE
    }
}
fn steady_buffering_deadline(buffering_started: Instant) -> Instant {
    buffering_started + STEADY_BUFFERING_RECOVERY_DELAY
}
fn operation_deadline(operation_started: Instant) -> Instant {
    operation_started + crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE
}
fn ready_seek_watchdog_deadline(status: &str, operation_started: Instant) -> Option<Instant> {
    (status != "paused").then(|| operation_deadline(operation_started))
}
fn seek_watchdog_waiting_for_audio(
    current_generation: u64,
    status: &str,
    watched_generation: u64,
    current_epoch: u64,
    watched_epoch: u64,
) -> bool {
    current_generation == watched_generation
        && current_epoch == watched_epoch
        && status == "buffering"
}
async fn wait_for_ready_seek_audio(
    mut updates: watch::Receiver<Value>,
    generation: u64,
    epoch: u64,
    deadline: Instant,
    current: impl Fn() -> (u64, String, u64) + Send + Sync + 'static,
) -> bool {
    let waiting_for_audio = async {
        loop {
            let (current_generation, status, current_epoch) = current();
            if !seek_watchdog_waiting_for_audio(
                current_generation,
                &status,
                generation,
                current_epoch,
                epoch,
            ) || updates.changed().await.is_err()
            {
                return;
            }
        }
    };
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), waiting_for_audio)
        .await
        .is_err()
        && {
            let (current_generation, status, current_epoch) = current();
            seek_watchdog_waiting_for_audio(
                current_generation,
                &status,
                generation,
                current_epoch,
                epoch,
            )
        }
}
const MAX_PLAYBACK_QUEUE_PAGE: usize = 200;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebViewPlaybackHandoff {
    pub handoff_id: String,
    pub source_generation: u64,
    pub seek_serial: u64,
    pub target_position: f64,
    pub tracks: Vec<Track>,
    pub entry_ids: Vec<String>,
    pub index: usize,
    pub session_id: String,
    pub entry_id: String,
    pub paused: bool,
    pub volume: f64,
    pub repeat: String,
    pub shuffle: bool,
    pub candidate: Candidate,
    pub recorded: bool,
}

/// One-use handoff envelope for a source already resolved and retained in Rust.
/// It intentionally carries no URL, headers, key, or serialized AudioSource.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedSourceHandoff {
    pub handoff_id: String,
    pub source_handle: String,
    pub source_generation: u64,
    pub seek_serial: u64,
    pub target_position: f64,
    pub tracks: Vec<Track>,
    pub entry_ids: Vec<String>,
    pub index: usize,
    pub session_id: String,
    pub entry_id: String,
    pub paused: bool,
    pub volume: f64,
    pub repeat: String,
    pub shuffle: bool,
    pub recorded: bool,
}

struct PendingNativeSource {
    source_generation: u64,
    request_track: Track,
    explicit_source: bool,
    matching_level: crate::recording_match::MatchLevel,
    expires_at: Instant,
    resolved: crate::playback_sources::ResolvedSource,
}

fn pending_source_owner_matches(
    source: &PendingNativeSource,
    handoff: &ResolvedSourceHandoff,
    target_track: &Track,
    now: Instant,
) -> bool {
    source.expires_at > now
        && source.source_generation == handoff.source_generation
        && native_handoff_track_identity_matches(&source.request_track, target_track)
}

fn native_handoff_track_identity_matches(expected: &Track, actual: &Track) -> bool {
    expected.id == actual.id
        && expected.provider == actual.provider
        && expected.provider_id == actual.provider_id
        && expected.source_url == actual.source_url
        && expected.artist == actual.artist
        && expected.title == actual.title
        && expected.version == actual.version
        && expected.album == actual.album
        && expected.album_artist == actual.album_artist
        && expected.recording_date == actual.recording_date
        && expected.track_number == actual.track_number
        && expected.track_total == actual.track_total
        && expected.disc_number == actual.disc_number
        && expected.disc_total == actual.disc_total
        && expected.remixers == actual.remixers
        && expected.label == actual.label
        && expected.release_date == actual.release_date
        && expected.catalog_number == actual.catalog_number
        && expected.duration_seconds == actual.duration_seconds
        && expected.isrc == actual.isrc
        && expected.playback_range == actual.playback_range
        && expected.asset_id == actual.asset_id
        && expected.local_path == actual.local_path
        // Provenance carries the authority and source context for identity
        // evidence. Other mutable library state (favorite/local availability,
        // artwork, play count) is intentionally outside this binding.
        && expected.provenance == actual.provenance
}

fn resolved_match_level(
    resolved: &crate::playback_sources::ResolvedSource,
) -> Option<crate::recording_match::MatchLevel> {
    let value = resolved.quality.get("matchLevel")?.as_str()?;
    match value {
        "exact" => Some(crate::recording_match::MatchLevel::Exact),
        "balanced" => Some(crate::recording_match::MatchLevel::Balanced),
        "flexible" => Some(crate::recording_match::MatchLevel::Flexible),
        _ => None,
    }
}

fn pending_source_policy_matches(
    source: &PendingNativeSource,
    current: crate::recording_match::MatchLevel,
) -> bool {
    source.matching_level == current && resolved_match_level(&source.resolved) == Some(current)
}

fn validate_resolved_handoff_occurrence(handoff: &ResolvedSourceHandoff) -> Result<&Track, String> {
    if uuid::Uuid::parse_str(&handoff.handoff_id).is_err()
        || uuid::Uuid::parse_str(&handoff.source_handle).is_err()
        || handoff.session_id.trim().is_empty()
        || handoff.session_id.len() > 128
        || handoff.session_id.chars().any(char::is_control)
        || handoff.entry_id.trim().is_empty()
        || handoff.entry_id.len() > 200
        || handoff.entry_id.chars().any(char::is_control)
    {
        return Err("Invalid native source handoff identity".into());
    }
    if handoff.index >= handoff.tracks.len()
        || handoff.entry_ids.len() != handoff.tracks.len()
        || handoff.entry_ids.get(handoff.index) != Some(&handoff.entry_id)
        || handoff
            .entry_ids
            .iter()
            .any(|id| id.trim().is_empty() || id.len() > 200 || id.chars().any(char::is_control))
        || handoff.entry_ids.iter().collect::<HashSet<_>>().len() != handoff.entry_ids.len()
    {
        return Err("Playback queue occurrence identity is inconsistent".into());
    }
    handoff
        .tracks
        .get(handoff.index)
        .ok_or_else(|| "Playback queue position is out of range".to_owned())
}

fn take_pending_native_source(
    pending: &mut HashMap<String, PendingNativeSource>,
    handle: &str,
    now: Instant,
) -> Option<PendingNativeSource> {
    pending.retain(|_, source| source.expires_at > now);
    pending.remove(handle)
}

fn purge_pending_native_sources_older_than(
    pending: &mut HashMap<String, PendingNativeSource>,
    generation: u64,
) -> usize {
    let previous_len = pending.len();
    pending.retain(|_, source| source.source_generation >= generation);
    previous_len - pending.len()
}

fn exact_handoff_youtube_id<'a>(
    track: &Track,
    candidate: &'a Candidate,
    target_position: f64,
) -> Option<&'a str> {
    let id = candidate.id.strip_prefix("youtube:")?;
    let duration = track.duration_seconds?;
    (track.provider == "youtube"
        && track.provider_id == id
        && track.local_path.is_none()
        && track.asset_id.is_none()
        && track.playback_range.is_none()
        && track.version.as_deref().is_none_or(str::is_empty)
        && track.provenance["chapters"]
            .as_array()
            .is_none_or(Vec::is_empty)
        && candidate.provider == "youtube"
        && candidate.source.is_none()
        && candidate.chapters.as_ref().is_none_or(Vec::is_empty)
        && duration.is_finite()
        && (5.0..=86_400.0).contains(&duration)
        && target_position.is_finite()
        && target_position >= 0.0
        && target_position < duration
        && crate::acquisition::youtube_video_id(&track.source_url).as_deref() == Some(id)
        && crate::acquisition::youtube_video_id(&candidate.url).as_deref() == Some(id))
    .then_some(id)
}

fn handoff_stop_is_owned(expected: &str, current: Option<&str>) -> bool {
    current == Some(expected)
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayingTrackIdentification {
    pub track_id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artwork_url: Option<String>,
    pub note: String,
    pub timing_anchor: Option<RecognitionTimingAnchor>,
    #[serde(skip)]
    pub(crate) detection_metadata: Option<crate::radio::RadioTrackMetadata>,
    #[serde(skip)]
    pub(crate) captured_audio_interval: Option<(f64, f64)>,
    #[serde(skip)]
    pub(crate) detection_source_identity: Option<crate::playing_track_detections::SourceIdentity>,
    #[serde(skip)]
    pub(crate) detection_source_epoch: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecognitionTimingAnchor {
    /// Shazam's reference-track offset for the first match, in seconds.
    pub shazam_match_offset_seconds: f64,
    /// Absolute player-clock position at the first captured PCM sample.
    pub captured_audio_start_position_seconds: f64,
    pub captured_audio_duration_seconds: f64,
    pub source_generation: u64,
    pub seek_serial: u64,
    pub track_id: String,
    pub entry_id: String,
}

#[derive(Debug, Clone, PartialEq)]
struct PersistedSessionKey {
    queue_revision: u64,
    index: usize,
    position_bits: u64,
    volume_bits: u64,
    repeat: String,
    shuffle: bool,
    finished: bool,
}

struct Preloaded {
    preparation_trace: Option<crate::playback_sources::SourceTrace>,
    generation: u64,
    index: usize,
    entry: String,
    candidate: Option<Candidate>,
    quality: Value,
    duration: Option<f64>,
    steady_http_stream: bool,
    offset: f64,
    end: Option<f64>,
    loudness_status: crate::loudness_manager::LoudnessStatus,
    loudness_album_context: Option<crate::loudness::LoudnessResourceIdentity>,
    opus_decoder_capability: Option<ActiveOpusDecoderCapability>,
}

/// Identity of one speculative preparation. Queue position and generation alone
/// can be reused after pause/resume; the watch channel distinguishes its owner.
struct PreloadPreparationOwner {
    generation: u64,
    index: usize,
    entry: String,
    cancel: watch::Sender<bool>,
}

fn preload_preparation_is_current(state: &State, owner: &PreloadPreparationOwner) -> bool {
    state.generation == owner.generation
        && state.entries.get(owner.index) == Some(&owner.entry)
        && state.status == "playing"
        && !*owner.cancel.borrow()
        && state
            .preload_cancel
            .as_ref()
            .is_some_and(|current| current.same_channel(&owner.cancel))
}

/// Called under the ownership lock after asynchronous resolution. In particular,
/// a late failure cannot clear the replacement preparation's cancellation slot.
fn finish_preload_resolution<T>(
    state: &mut State,
    owner: &PreloadPreparationOwner,
    result: Result<T, String>,
) -> Option<T> {
    if !preload_preparation_is_current(state, owner) {
        return None;
    }
    match result {
        Ok(value) => Some(value),
        Err(_) => {
            state.preload_cancel = None;
            None
        }
    }
}

fn cancel_preload_state(state: &mut State) {
    if let Some(cancel) = state.preload_cancel.take() {
        cancel.send_replace(true);
    }
    state.preload = None;
}

fn fence_unresolved_preload_for_settings_change(state: &mut State, settings_changed: bool) -> bool {
    if settings_changed && state.preload.is_none() && state.preload_cancel.is_some() {
        cancel_preload_state(state);
        true
    } else {
        false
    }
}

fn apply_device_lost(state: &mut State, generation: u64, position: f64, reason: &str) -> bool {
    if state.generation != generation {
        return false;
    }
    // Output discarded its queued readers with the lost device. Retire their
    // service-side owner too, so reconnect can prepare the next occurrence again.
    cancel_preload_state(state);
    state.position = (position - state.offset).max(0.);
    state.output_ready = false;
    if state.status != "paused" {
        state.status = "buffering".into();
        state.message = format!("Audio output interrupted ({reason}); reconnecting…");
    } else {
        state.message = format!("Audio output interrupted ({reason}); reconnecting while paused…");
    }
    state.revision += 1;
    true
}

#[derive(Clone)]
struct NativeDecoderDiagnosticContext {
    resolution_id: String,
    native_operation_id: Option<String>,
    provider: String,
    generation: u64,
    request_started: Instant,
    decoder_started: Instant,
}

struct State {
    revision: u64,
    generation: u64,
    generation_counter: u64,
    status: String,
    natural_queue_end: bool,
    queue: Vec<Track>,
    entries: Vec<String>,
    loudness_queue_contexts: HashMap<String, crate::loudness::LoudnessResourceIdentity>,
    queue_revision: u64,
    published_queue_revision: Option<u64>,
    persisted_queue_revision: Option<u64>,
    persisted_session: Option<PersistedSessionKey>,
    index: usize,
    position: f64,
    duration: f64,
    duration_verified: bool,
    volume: f64,
    repeat: String,
    shuffle: bool,
    candidate: Option<Candidate>,
    explicit_source: bool,
    quality: Value,
    loudness_status: crate::loudness_manager::LoudnessStatus,
    /// Exact configured decoder bytes/path authorized for this active source.
    /// Refresh/adoption can reuse it without falling back to mutable PATH.
    opus_decoder_capability: Option<ActiveOpusDecoderCapability>,
    decoder_diagnostics: Option<DecoderDiagnostics>,
    last_buffering: Option<Value>,
    last_recovery: Option<Value>,
    artwork_cache: Option<(String, String)>,
    message: String,
    session: String,
    recorded: bool,
    offset: f64,
    end: Option<f64>,
    cancel: Option<watch::Sender<bool>>,
    station: Option<RadioStation>,
    steady_http_stream: bool,
    radio_now: Value,
    radio_history: Vec<Value>,
    last_saved: Instant,
    last_local_storage_failure: Option<Value>,
    last_source_failure: Option<Value>,
    last_skipped_track_failure: Option<Value>,
    recent_track_failures: Vec<Value>,
    handoff_id: Option<String>,
    pause_when_ready: bool,
    seek_serial: u64,
    pending_seek_generation: Option<u64>,
    seek_watchdog_epoch: u64,
    recovery: u8,
    startup_budget: Duration,
    recovery_deadline: Option<Instant>,
    recovered_audio_since: Option<(Instant, f64)>,
    healthy_output_since: Option<(Instant, f64, u64)>,
    recent_stall: Option<(String, String, Instant)>,
    recurring_source_swaps: u8,
    failed_recurring_candidates: Vec<(String, String)>,
    replace_source_on_recovery: bool,
    recovery_failure_generation: Option<u64>,
    failed_occurrence_ids: HashSet<String>,
    request_started: Instant,
    first_audio_ms: Option<u64>,
    source_ready_ms: Option<u64>,
    seek_requested: Option<Instant>,
    seek_first_audio_ms: Option<u64>,
    queued_intent_epoch: u64,
    queued_operation_id: Option<String>,
    operation_id: Option<String>,
    operation_kind: Option<String>,
    operation_received_at: Option<Instant>,
    operation_client_started_at_ms: Option<u64>,
    operation_click_to_ipc_ms: Option<u64>,
    operation_command_bytes: Option<usize>,
    operation_source_ready_ms: Option<u64>,
    operation_first_audio_ms: Option<u64>,
    seek_operation_id: Option<String>,
    seek_operation_received_at: Option<Instant>,
    seek_client_started_at_ms: Option<u64>,
    seek_click_to_ipc_ms: Option<u64>,
    seek_command_bytes: Option<usize>,
    seek_operation_first_audio_ms: Option<u64>,
    buffering_count: u64,
    output_ready: bool,
    preload: Option<Preloaded>,
    preload_cancel: Option<watch::Sender<bool>>,
    native_decoder_diagnostic: Option<NativeDecoderDiagnosticContext>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            revision: 0,
            generation: 0,
            generation_counter: 0,
            status: "idle".into(),
            natural_queue_end: false,
            queue: vec![],
            entries: vec![],
            loudness_queue_contexts: HashMap::new(),
            queue_revision: 0,
            published_queue_revision: None,
            persisted_queue_revision: None,
            persisted_session: None,
            index: 0,
            position: 0.,
            duration: 0.,
            duration_verified: false,
            volume: 0.8,
            repeat: "off".into(),
            shuffle: false,
            candidate: None,
            explicit_source: false,
            quality: json!({}),
            loudness_status: crate::loudness_manager::LoudnessStatus::off(),
            opus_decoder_capability: None,
            decoder_diagnostics: None,
            last_buffering: None,
            last_recovery: None,
            artwork_cache: None,
            message: String::new(),
            session: String::new(),
            recorded: false,
            offset: 0.,
            end: None,
            cancel: None,
            station: None,
            steady_http_stream: false,
            radio_now: Value::Null,
            radio_history: vec![],
            last_saved: Instant::now(),
            last_local_storage_failure: None,
            last_source_failure: None,
            last_skipped_track_failure: None,
            recent_track_failures: Vec::new(),
            handoff_id: None,
            pause_when_ready: false,
            seek_serial: 0,
            pending_seek_generation: None,
            seek_watchdog_epoch: 0,
            recovery: 0,
            startup_budget: crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE,
            recovery_deadline: None,
            recovered_audio_since: None,
            healthy_output_since: None,
            recent_stall: None,
            recurring_source_swaps: 0,
            failed_recurring_candidates: Vec::new(),
            replace_source_on_recovery: false,
            recovery_failure_generation: None,
            failed_occurrence_ids: HashSet::new(),
            request_started: Instant::now(),
            first_audio_ms: None,
            source_ready_ms: None,
            seek_requested: None,
            seek_first_audio_ms: None,
            queued_intent_epoch: 0,
            queued_operation_id: None,
            operation_id: None,
            operation_kind: None,
            operation_received_at: None,
            operation_client_started_at_ms: None,
            operation_click_to_ipc_ms: None,
            operation_command_bytes: None,
            operation_source_ready_ms: None,
            operation_first_audio_ms: None,
            seek_operation_id: None,
            seek_operation_received_at: None,
            seek_client_started_at_ms: None,
            seek_click_to_ipc_ms: None,
            seek_command_bytes: None,
            seek_operation_first_audio_ms: None,
            buffering_count: 0,
            output_ready: false,
            preload: None,
            preload_cancel: None,
            native_decoder_diagnostic: None,
        }
    }
}
impl State {
    fn resume_target(&self) -> (f64, bool, bool) {
        let finished = self.status == "ended";
        (
            if finished { 0. } else { self.position },
            self.output_ready && !finished,
            !finished,
        )
    }
    fn track(&self) -> Option<&Track> {
        self.queue.get(self.index)
    }
    fn snapshot(&self, native: bool, queue: bool) -> Value {
        let operation_trace = [
            Some(json!({"phase":"ipc_received","atMs":0}))
                .filter(|_| self.operation_received_at.is_some()),
            self.operation_source_ready_ms
                .map(|at| json!({"phase":"source_ready","atMs":at})),
            self.operation_first_audio_ms
                .map(|at| json!({"phase":"first_audio","atMs":at})),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let seek_trace = [
            Some(json!({"phase":"ipc_received","atMs":0}))
                .filter(|_| self.seek_operation_received_at.is_some()),
            self.seek_operation_first_audio_ms
                .map(|at| json!({"phase":"first_audio","atMs":at})),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let mut value = json!({"engine":if native{"native"}else{"webview"},"revision":self.revision,"queueRevision":self.queue_revision,"queuedOperationId":self.queued_operation_id,"generation":self.generation,"status":self.status,"trackId":self.track().map(|t|&t.id),"track":self.track(),"entryId":self.entries.get(self.index),"candidate":self.candidate,"currentIndex":if self.queue.is_empty(){-1}else{self.index as i64},"position":self.position,"duration":self.duration,"volume":self.volume,"repeat":self.repeat,"shuffle":self.shuffle,"paused":self.status!="playing","canSeek":self.station.is_none()&&self.duration>0.,"canVolume":true,"playerMessage":self.message,"quality":self.quality,"loudnessNormalization":self.loudness_status,"chapter":self.chapter(),"radioNowPlaying":self.radio_now,"radioHistory":self.radio_history,"seekSerial":self.seek_serial,"previewKind":"native","diagnostics":{"firstAudioMs":self.first_audio_ms,"sourceReadyMs":self.source_ready_ms,"seekFirstAudioMs":self.seek_first_audio_ms,"operationId":self.operation_id,"operationKind":self.operation_kind,"operationTrace":operation_trace,"clientStartedAtMs":self.operation_client_started_at_ms,"clickToIpcMs":self.operation_click_to_ipc_ms,"commandSerializedBytes":self.operation_command_bytes,"seekOperationId":self.seek_operation_id,"seekTrace":seek_trace,"seekClientStartedAtMs":self.seek_client_started_at_ms,"seekClickToIpcMs":self.seek_click_to_ipc_ms,"seekCommandSerializedBytes":self.seek_command_bytes,"seekOperationFirstAudioMs":self.seek_operation_first_audio_ms,"recoveryCount":self.recovery,"bufferingCount":self.buffering_count,"decoder":self.decoder_diagnostics,"lastBuffering":self.last_buffering,"lastRecovery":self.last_recovery,"lastLocalStorageFailure":self.last_local_storage_failure,"lastSourceFailure":self.last_source_failure,"lastSkippedTrackFailure":self.last_skipped_track_failure}});
        value["handoffId"] = self
            .handoff_id
            .as_ref()
            .map_or(Value::Null, |handoff_id| json!(handoff_id));
        value["naturalQueueEnd"] = json!(self.natural_queue_end);
        value["failure"] = if self.status == "failed" {
            json!(crate::playback_failure::classify(&self.message))
        } else {
            Value::Null
        };
        value["diagnostics"]["startupTimeoutSeconds"] = json!(self.startup_budget.as_secs());
        value["queueTotal"] = json!(self.queue.len());
        if let Some(chapter) = value["chapter"]["title"].as_str().map(str::to_owned) {
            if let Some(track) = value["track"].as_object_mut() {
                track.insert("title".into(), json!(chapter));
            }
        }
        if let Some(title) = self.radio_now["title"].as_str() {
            value["track"]["title"] = json!(title);
            value["track"]["artist"] = self.radio_now["artist"].clone();
            if let Some(art) = self.radio_now["metadata"]["artworkUrl"].as_str() {
                value["track"]["artworkUrl"] = json!(art);
            }
        }
        if value["track"].is_object() {
            if value["track"]["artworkUrl"]
                .as_str()
                .is_none_or(str::is_empty)
            {
                if let Some(art) = self.candidate.as_ref().and_then(|c| c.artwork_url.as_ref()) {
                    value["track"]["artworkUrl"] = json!(art);
                }
            }
            if let Some((url, path)) = &self.artwork_cache {
                if value["track"]["artworkUrl"].as_str() == Some(url.as_str()) {
                    value["track"]["artworkFilePath"] = json!(path);
                }
            }
        }
        if queue {
            value["queue"] = json!(self.queue);
            value["queueEntryIds"] = json!(self.entries);
            value["queueLoudnessContexts"] = json!(self.loudness_queue_contexts);
        }
        value
    }

    fn backup_checkpoint_values(&self) -> (Value, Value) {
        let queue = json!({
            "tracks": self.queue,
            "entryIds": self.entries,
            "queueRevision": self.queue_revision,
            "queueLoudnessContexts": self.loudness_queue_contexts,
        });
        let session = json!({
            "index": self.index,
            "currentEntryId": self.entries.get(self.index),
            "queueRevision": self.queue_revision,
            "position": self.position,
            "volume": self.volume,
            "repeat": self.repeat,
            "shuffle": self.shuffle,
            "finished": self.status == "ended",
        });
        (queue, session)
    }
    /// Explicit status/MCP queries include the bounded history. Routine snapshots
    /// never construct it, so emit/watch/control pulses cannot clone or serialize it.
    fn snapshot_with_failure_history(&self, native: bool, queue: bool) -> Value {
        let mut value = self.snapshot(native, queue);
        value["diagnostics"]["recentTrackFailures"] = json!(self.recent_track_failures);
        value
    }
    fn snapshot_page(&self, native: bool, offset: usize, limit: usize) -> Value {
        let limit = limit.clamp(1, MAX_PLAYBACK_QUEUE_PAGE);
        let start = offset.min(self.queue.len());
        let end = start.saturating_add(limit).min(self.queue.len());
        let mut value = self.snapshot_with_failure_history(native, false);
        value["queue"] = json!(self.queue[start..end]);
        value["queueEntryIds"] = json!(self.entries[start..end]);
        let page_entries: HashSet<&str> = self.entries[start..end]
            .iter()
            .map(String::as_str)
            .collect();
        let page_contexts: HashMap<&str, &crate::loudness::LoudnessResourceIdentity> = self
            .loudness_queue_contexts
            .iter()
            .filter(|(entry_id, _)| page_entries.contains(entry_id.as_str()))
            .map(|(entry_id, resource)| (entry_id.as_str(), resource))
            .collect();
        value["queueLoudnessContexts"] = json!(page_contexts);
        value["queueTotal"] = json!(self.queue.len());
        value["queueOffset"] = json!(start);
        value["queueLimit"] = json!(limit);
        value["queueHasMore"] = json!(end < self.queue.len());
        value
    }
    fn chapter(&self) -> Value {
        if let Some(r) = self.track().and_then(|t| t.playback_range.as_ref()) {
            return json!({"title":self.track().map(|t|t.title.as_str()),"startSeconds":r.start_time,"endSeconds":r.end_time});
        }
        self.candidate
            .as_ref()
            .and_then(|c| c.chapters.as_ref())
            .and_then(|cs| {
                cs.iter()
                    .find(|c| c.start_seconds <= self.position && c.end_seconds > self.position)
            })
            .map(|c| json!({"title":c.title,"startSeconds":c.start_seconds,"endSeconds":c.end_seconds}))
            .unwrap_or(Value::Null)
    }
}

/// Revision and index validation share the exact mutation lock at the caller.
fn reorder_queue_state(
    s: &mut State,
    from: usize,
    to: usize,
    expected: Option<u64>,
) -> Result<(), String> {
    if expected.is_some_and(|revision| revision != s.queue_revision) {
        return Err("Queue revision changed before reorder".into());
    }
    if from >= s.queue.len() || to >= s.queue.len() {
        return Err("Queue position is out of range".into());
    }
    let selected = s.entries[s.index].clone();
    let track = s.queue.remove(from);
    let entry = s.entries.remove(from);
    s.queue.insert(to, track);
    s.entries.insert(to, entry);
    mark_queue_changed(s);
    s.index = s.entries.iter().position(|id| *id == selected).unwrap_or(0);
    Ok(())
}

/// Remove a selected set of occurrences in one revision, retaining the playing occurrence.
fn remove_queue_occurrences(
    s: &mut State,
    indices: &[usize],
    expected: Option<u64>,
) -> Result<(), String> {
    if expected.is_some_and(|revision| revision != s.queue_revision) {
        return Err("Queue revision changed before removal".into());
    }
    let mut indices = indices.to_vec();
    indices.sort_unstable();
    indices.dedup();
    if indices
        .iter()
        .any(|&index| index >= s.queue.len() || index == s.index)
    {
        return Err("Queue removal must reference existing non-playing occurrences".into());
    }
    if indices.is_empty() {
        return Ok(());
    }
    let selected = s.entries.get(s.index).cloned();
    for index in indices.into_iter().rev() {
        s.queue.remove(index);
        s.entries.remove(index);
    }
    s.index = selected
        .and_then(|id| s.entries.iter().position(|entry| *entry == id))
        .unwrap_or(0);
    mark_queue_changed(s);
    Ok(())
}

fn mark_queue_changed(state: &mut State) {
    state.queue_revision = state.queue_revision.wrapping_add(1);
    let entries: HashSet<&str> = state.entries.iter().map(String::as_str).collect();
    state
        .loudness_queue_contexts
        .retain(|entry_id, _| entries.contains(entry_id.as_str()));
    state.recent_track_failures.clear();
    state.failed_occurrence_ids.clear();
}

fn validate_loudness_album_queue_binding(
    resource: &crate::loudness::LoudnessResourceIdentity,
    tracks: &[Track],
) -> Result<(), String> {
    resource
        .validate()
        .map_err(|_| "Album loudness queue identity is invalid".to_owned())?;
    if resource.scope != crate::loudness::ResourceScope::Album
        || resource.members.len() != tracks.len()
        || tracks
            .iter()
            .zip(&resource.members)
            .any(|(track, member)| !loudness_album_resource_matches_track(resource, track, member))
    {
        return Err("Album loudness identity must match every queued occurrence in order".into());
    }
    Ok(())
}

fn validate_loudness_album_resource_member(
    resource: &crate::loudness::LoudnessResourceIdentity,
    asset_id: &str,
    file_hash: &str,
    range: crate::loudness::AudioRange,
) -> Result<(), String> {
    resource
        .validate()
        .map_err(|_| "Album loudness resource identity is invalid".to_owned())?;
    if resource.scope != crate::loudness::ResourceScope::Album
        || resource
            .inter_track_gaps_seconds
            .iter()
            .any(|gap| *gap != 0.0)
    {
        return Err("Album loudness resource does not match gapless playback".into());
    }
    let mut member_ids = HashSet::with_capacity(resource.members.len());
    if resource
        .members
        .iter()
        .any(|member| !member_ids.insert(member.asset_id.as_str()))
    {
        return Err("Album loudness resource contains duplicate assets".into());
    }
    if resource
        .members
        .iter()
        .filter(|member| {
            member.asset_id == asset_id && member.file_hash == file_hash && member.range == range
        })
        .count()
        != 1
    {
        return Err("Album loudness resource does not match this exact asset and range".into());
    }
    Ok(())
}

fn loudness_album_resource_matches_track(
    resource: &crate::loudness::LoudnessResourceIdentity,
    track: &Track,
    member: &crate::loudness::LoudnessAssetIdentity,
) -> bool {
    if resource.scope != crate::loudness::ResourceScope::Album
        || resource
            .inter_track_gaps_seconds
            .iter()
            .any(|gap| *gap != 0.0)
        || track.asset_id.as_deref() != Some(member.asset_id.as_str())
    {
        return false;
    }
    let expected =
        track
            .playback_range
            .as_ref()
            .map_or(crate::loudness::AudioRange::WHOLE_FILE, |range| {
                crate::loudness::AudioRange {
                    start_seconds: range.start_time,
                    end_seconds: Some(range.end_time),
                }
            });
    member.range == expected
}

fn bind_loudness_context_to_queue_range(
    state: &mut State,
    start: usize,
    count: usize,
    resource: Option<&crate::loudness::LoudnessResourceIdentity>,
) {
    if let Some(resource) = resource {
        for entry_id in state.entries.iter().skip(start).take(count) {
            state
                .loudness_queue_contexts
                .insert(entry_id.clone(), resource.clone());
        }
    }
}

fn bind_loudness_context_to_matching_queue(
    state: &mut State,
    resource: &crate::loudness::LoudnessResourceIdentity,
) -> Result<(), String> {
    resource
        .validate()
        .map_err(|_| "Album loudness queue identity is invalid".to_owned())?;
    if resource.scope != crate::loudness::ResourceScope::Album
        || resource.members.len() > state.queue.len()
    {
        return Err("Album loudness identity does not match the current queue".into());
    }
    let start = state
        .queue
        .windows(resource.members.len())
        .position(|tracks| {
            tracks.iter().zip(&resource.members).all(|(track, member)| {
                loudness_album_resource_matches_track(resource, track, member)
            })
        });
    let Some(start) = start else {
        return Err("Album loudness identity does not match an ordered queue occurrence".into());
    };
    bind_loudness_context_to_queue_range(state, start, resource.members.len(), Some(resource));
    Ok(())
}

// Terminal local failure: no source-cache invalidation, recovery, or queue advancement.
fn apply_local_storage_failure(
    state: &mut State,
    generation: u64,
    fault: crate::native_output::storage_fault::Fault,
) -> bool {
    if state.generation != generation
        || matches!(state.status.as_str(), "failed" | "idle" | "ended")
        || state.cancel.as_ref().is_some_and(|cancel| *cancel.borrow())
    {
        return false;
    }
    if let Some(cancel) = state.cancel.take() {
        let _ = cancel.send(true);
    }
    if let Some(cancel) = state.preload_cancel.take() {
        let _ = cancel.send(true);
    }
    state.preload = None;
    state.generation_counter += 1;
    state.generation = state.generation_counter;
    state.output_ready = false;
    state.pending_seek_generation = None;
    state.steady_http_stream = false;
    state.seek_watchdog_epoch = state.seek_watchdog_epoch.wrapping_add(1);
    state.status = "failed".into();
    state.natural_queue_end = false;
    state.last_local_storage_failure = Some(json!(fault));
    state.message = match fault.kind {
        crate::native_output::storage_fault::Kind::QuotaExceeded | crate::native_output::storage_fault::Kind::NoSpace => "Playback stopped because local temporary storage is full. Free space, then retry.",
        _ => "Playback stopped because local temporary storage failed. Check available storage and permissions, then retry.",
    }.into();
    state.revision += 1;
    true
}

fn apply_normalization_selection(
    state: &mut State,
    generation: u64,
    status: crate::loudness_manager::LoudnessStatus,
) -> bool {
    if state.generation == generation {
        state.loudness_status = status;
        state.revision = state.revision.wrapping_add(1);
        return true;
    }
    let preload_matches = state.preload.as_ref().is_some_and(|preload| {
        preload.generation == generation && state.entries.get(preload.index) == Some(&preload.entry)
    });
    if preload_matches {
        state.preload.as_mut().unwrap().loudness_status = status;
        return true;
    }
    false
}

fn apply_buffering_event(
    state: &mut State,
    generation: u64,
    diagnostics: Option<BufferingDiagnostics>,
) -> bool {
    if state.generation != generation
        || matches!(state.status.as_str(), "paused" | "idle" | "failed")
    {
        return false;
    }
    if let Some(diagnostics) = diagnostics {
        state.last_buffering = Some(
            json!({"generation":generation,"seekSerial":state.seek_serial,"stream":diagnostics}),
        );
    }
    state.buffering_count += 1;
    state.status = "buffering".into();
    state.message = "Buffering audio…".into();
    state.revision += 1;
    true
}

fn steady_http_stream(input: &str, station: bool, known_duration: Option<f64>) -> bool {
    !station
        && (input.starts_with("http://") || input.starts_with("https://"))
        && known_duration.is_some_and(|seconds| seconds.is_finite() && seconds > 0.)
}

fn steady_buffering_watch_matches(
    state: &State,
    watched_generation: u64,
    watched_seek_serial: u64,
    watched_buffering_count: u64,
) -> bool {
    state.generation == watched_generation
        && state.seek_serial == watched_seek_serial
        && state.buffering_count == watched_buffering_count
        && state.status == "buffering"
        && state.first_audio_ms.is_some()
        && state.output_ready
        && state.pending_seek_generation.is_none()
        && (state.station.is_some() || state.steady_http_stream)
}

async fn wait_for_steady_buffering_deadline(
    mut updates: watch::Receiver<Value>,
    deadline: Instant,
    current: impl Fn() -> bool + Send + Sync + 'static,
) -> bool {
    let current: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(current);
    let waiter_current = current.clone();
    let owner_invalidated = async move {
        loop {
            if !waiter_current() || updates.changed().await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        biased;
        _ = owner_invalidated => false,
        _ = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            std::future::pending::<()>(),
        ) => current(),
    }
}

fn clear_playback_failures(state: &mut State) {
    clear_current_source_failure(state);
    state.last_skipped_track_failure = None;
    state.last_recovery = None;
    clear_native_stall_history(state);
    state.healthy_output_since = None;
    state.replace_source_on_recovery = false;
}

fn clear_current_source_failure(state: &mut State) {
    state.last_buffering = None;
    state.last_local_storage_failure = None;
    state.last_source_failure = None;
}

fn persisted_session_key(state: &State) -> PersistedSessionKey {
    PersistedSessionKey {
        queue_revision: state.queue_revision,
        index: state.index,
        position_bits: state.position.to_bits(),
        volume_bits: state.volume.to_bits(),
        repeat: state.repeat.clone(),
        shuffle: state.shuffle,
        finished: state.status == "ended",
    }
}

// The hook keeps the production snapshot/write ordering directly testable without an app.
fn persist_playback_state<E>(
    state: &Mutex<State>,
    persistence: &Mutex<()>,
    write: impl Fn(Option<&Value>, Option<&Value>) -> Result<(), E>,
    after_snapshot: impl FnOnce(),
) {
    // Acquire before snapshotting: locking only the writes still permits stale overwrites.
    let _persistence = persistence.lock().unwrap_or_else(|e| e.into_inner());
    let (queue_revision, queue, session_key, session) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        let queue = (s.persisted_queue_revision != Some(s.queue_revision)).then(
            || json!({"tracks":s.queue,"entryIds":s.entries,"queueRevision":s.queue_revision,"queueLoudnessContexts":s.loudness_queue_contexts}),
        );
        let key = persisted_session_key(&s);
        let session = (s.persisted_session.as_ref() != Some(&key)).then(|| {
                json!({"index":s.index,"currentEntryId":s.entries.get(s.index),"queueRevision":s.queue_revision,"position":s.position,"volume":s.volume,"repeat":s.repeat,"shuffle":s.shuffle,"finished":s.status=="ended"})
            });
        (s.queue_revision, queue, key, session)
    };
    after_snapshot();
    if queue.is_none() && session.is_none() {
        return;
    }
    if write(queue.as_ref(), session.as_ref()).is_err() {
        return;
    }
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    if queue.is_some() && s.queue_revision == queue_revision {
        s.persisted_queue_revision = Some(queue_revision);
    }
    if session.is_some() && persisted_session_key(&s) == session_key {
        s.persisted_session = Some(session_key);
    }
}

struct Inner {
    app: tauri::AppHandle,
    store: Arc<Store>,
    acquisition: Arc<AcquisitionManager>,
    history: Arc<PlaybackHistory>,
    state: Mutex<State>,
    pending_native_sources: Mutex<HashMap<String, PendingNativeSource>>,
    persistence: Mutex<()>,
    commands: tokio::sync::Mutex<()>,
    native: AtomicBool,
    output: NativeOutput,
    observed_analysis: Arc<crate::observed_analysis::ObservedAnalysis>,
    loudness_verification: Arc<crate::loudness_manager::LocalVerificationMemo>,
    loudness_album_context: Mutex<Option<crate::loudness::LoudnessResourceIdentity>>,
    updates: watch::Sender<Value>,
    audible: watch::Sender<bool>,
    playing_track_identifications: PlayingTrackIdentifications,
    radio_events: mpsc::UnboundedSender<RadioEvent>,
}
#[derive(Clone)]
pub struct PlaybackService {
    inner: Arc<Inner>,
}

/// Full native playback state captured for a backup checkpoint. Routine
/// playback snapshots are paginated; backup callers must use this seam instead.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackBackupCheckpoint {
    pub engine: String,
    pub revision: u64,
    pub queue_revision: u64,
    /// Present only when native playback is the active engine.
    pub queue: Option<Value>,
    /// Present only when native playback is the active engine.
    pub session: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBpmPlaybackEstimate {
    pub generation: u64,
    pub seek_serial: u64,
    pub track_id: String,
    #[serde(flatten)]
    pub estimate: crate::live_bpm::LiveBpmEstimate,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveKeyPlaybackEstimate {
    pub generation: u64,
    pub seek_serial: u64,
    pub track_id: String,
    #[serde(flatten)]
    pub estimate: crate::live_key::LiveKeyEstimate,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePlaybackEstimates {
    pub bpm: Option<LiveBpmPlaybackEstimate>,
    pub key: Option<LiveKeyPlaybackEstimate>,
}
type QueuedPlaybackCommit = Box<dyn FnOnce(Value) -> Result<(), String> + Send>;

/// The guard is required at this boundary so snapshot capture and receipt commit
/// cannot be moved outside command serialization accidentally.
fn commit_queued_playback(
    _command: &tokio::sync::MutexGuard<'_, ()>,
    snapshot: impl FnOnce() -> Value,
    commit: Option<QueuedPlaybackCommit>,
) -> Result<(), String> {
    if let Some(commit) = commit {
        commit(snapshot())?;
    }
    Ok(())
}

fn opus_r128_album_identity_matches(
    tags: &crate::loudness_analysis::LocalTagMeasurements,
    track: &Track,
) -> bool {
    let Some(tag_album) = tags
        .album
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(expected_album) = track
        .album
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(tag_artist) = tags
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(expected_artist) = track
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    tag_album.eq_ignore_ascii_case(expected_album)
        && tag_artist.eq_ignore_ascii_case(expected_artist)
}

fn opus_r128_album_context_matches(
    tags: &crate::loudness_analysis::LocalTagMeasurements,
    track: &Track,
    context: Option<&crate::loudness::LoudnessResourceIdentity>,
    asset_id: &str,
    file_hash: &str,
    range: crate::loudness::AudioRange,
) -> bool {
    context.is_some_and(|context| {
        context.scope == crate::loudness::ResourceScope::Album
            && context.members.iter().any(|member| {
                member.asset_id == asset_id
                    && member.file_hash == file_hash
                    && member.range == range
            })
    }) && opus_r128_album_identity_matches(tags, track)
}

fn local_track_normalization_decision(
    tags: Option<&crate::loudness_analysis::LocalTagMeasurements>,
    is_opus: bool,
    range: crate::loudness::AudioRange,
    exact_true_peak: Option<f64>,
    header_gain_applied_once: bool,
    record: Option<&crate::loudness::LoudnessRecord>,
    policy: crate::loudness::GainPolicy,
    mode: crate::loudness::NormalizationMode,
    web_audio_stereo_route: bool,
) -> Option<(f64, crate::loudness::MeasurementProvenance)> {
    let mixed_opus_tags =
        is_opus && tags.is_some_and(crate::loudness_analysis::opus_comment_schemes_are_mixed);
    if !mixed_opus_tags {
        if is_opus && tags.is_some_and(|tags| tags.opus_r128_track_tag_present) {
            if let Some(tags) = tags {
                if let crate::loudness::GainDecision::ApplyDb(gain) =
                    crate::loudness_analysis::decide_verified_opus_r128_tag(
                        tags,
                        false,
                        true,
                        range,
                        exact_true_peak,
                        header_gain_applied_once,
                        policy,
                        mode,
                    )
                {
                    return Some((gain, crate::loudness::MeasurementProvenance::OpusR128Track));
                }
            }
        }
        if let Some(tag) = tags.and_then(|tags| tags.track_replaygain) {
            if let crate::loudness::GainDecision::ApplyDb(gain) =
                tag.decide_gain_with_output_peak_bound(policy, mode, exact_true_peak)
            {
                return Some((
                    gain,
                    crate::loudness::MeasurementProvenance::ReplayGainTrack,
                ));
            }
        }
    }
    // Malformed/duplicate/mixed comments never authorize tag gain. A current
    // resource-bound LUFS/peak analysis is independent evidence and remains usable.
    record.and_then(|record| {
        let integrated = record.playback_integrated_lufs_for_route(web_audio_stereo_route)?;
        match (crate::loudness::LoudnessMeasurement {
            integrated_lufs: integrated,
            true_peak_dbtp: None,
        })
        .decide_gain_with_output_peak_bound(
            record.playback_true_peak_bound_dbtp(web_audio_stereo_route),
            policy,
            mode,
        ) {
            crate::loudness::GainDecision::ApplyDb(gain) => Some((gain, record.provenance)),
            _ => None,
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveOpusDecoderCapability {
    executable_path: std::path::PathBuf,
    executable_sha256: String,
}

fn active_opus_decoder_capability(
    source: &crate::native_output::AudioSource,
) -> Option<ActiveOpusDecoderCapability> {
    let hint = source.decoder_hint.as_ref()?;
    let fingerprint = hint.decoder_fingerprint.as_deref()?;
    if hint.decode_policy != crate::native_output::AudioSourceDecodePolicy::FfmpegOggOpusHeadGain
        || hint.mime_type.as_deref() != Some("audio/ogg")
        || !hint
            .codec
            .as_deref()
            .is_some_and(|codec| codec.eq_ignore_ascii_case("opus"))
        || !source.ffmpeg.is_absolute()
        || fingerprint.len() != 64
        || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(ActiveOpusDecoderCapability {
        executable_path: source.ffmpeg.clone(),
        executable_sha256: fingerprint.to_ascii_lowercase(),
    })
}

fn attach_active_opus_decoder_capability(
    source: &mut crate::native_output::AudioSource,
    capability: Option<&ActiveOpusDecoderCapability>,
) {
    let Some(capability) = capability else {
        return;
    };
    source.ffmpeg = capability.executable_path.clone();
    source.decoder_hint = Some(crate::native_output::AudioSourceDecoderHint {
        mime_type: Some("audio/ogg".into()),
        codec: Some("opus".into()),
        duration_seconds: None,
        content_length: None,
        has_video: false,
        decode_policy: crate::native_output::AudioSourceDecodePolicy::FfmpegOggOpusHeadGain,
        decoder_fingerprint: Some(capability.executable_sha256.clone()),
    });
}

fn registered_source_with_opus_capability(
    input: &str,
    capability: Option<&ActiveOpusDecoderCapability>,
) -> crate::native_output::AudioSource {
    let mut source = crate::native_output::AudioSource {
        input: input.to_owned(),
        ffmpeg: "ffmpeg".into(),
        headers: vec![],
        decoder_hint: None,
        cenc: None,
    };
    attach_active_opus_decoder_capability(&mut source, capability);
    source
}

fn adopt_preloaded_opus_capability(state: &mut State, preload: &Preloaded) {
    state.opus_decoder_capability = preload.opus_decoder_capability.clone();
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentPlayableCodec {
    codec: String,
    container: String,
}

fn current_file_is_ogg_opus(
    asset: &crate::models::LocalAsset,
    probed: &CurrentPlayableCodec,
) -> bool {
    asset.final_codec.eq_ignore_ascii_case("opus")
        && probed.codec.eq_ignore_ascii_case("opus")
        && probed
            .container
            .split(',')
            .any(|container| container.eq_ignore_ascii_case("ogg"))
}

fn native_opus_decode_preprobe_eligible(
    source: &crate::native_output::AudioSource,
    asset: &crate::models::LocalAsset,
    mode: crate::loudness::NormalizationMode,
    range: crate::loudness::AudioRange,
) -> bool {
    mode != crate::loudness::NormalizationMode::Off
        && range == crate::loudness::AudioRange::WHOLE_FILE
        && source.cenc.is_none()
        && !source.input.starts_with("http://")
        && !source.input.starts_with("https://")
        && !source.input.starts_with("ytdl://")
        && source.input == asset.path
        && asset.final_codec.eq_ignore_ascii_case("opus")
}

fn native_opus_decode_policy_eligible(
    source: &crate::native_output::AudioSource,
    asset: &crate::models::LocalAsset,
    probed: &CurrentPlayableCodec,
    mode: crate::loudness::NormalizationMode,
    range: crate::loudness::AudioRange,
) -> bool {
    native_opus_decode_preprobe_eligible(source, asset, mode, range)
        && current_file_is_ogg_opus(asset, probed)
}

fn apply_native_opus_decode_policy(
    source: &mut crate::native_output::AudioSource,
    asset: &crate::models::LocalAsset,
    decoder_fingerprint: String,
) {
    let codec = asset.final_codec.clone();
    source.decoder_hint = Some(crate::native_output::AudioSourceDecoderHint {
        mime_type: Some("audio/ogg".into()),
        codec: Some(codec),
        duration_seconds: None,
        content_length: None,
        has_video: false,
        decode_policy: crate::native_output::AudioSourceDecodePolicy::FfmpegOggOpusHeadGain,
        decoder_fingerprint: Some(decoder_fingerprint),
    });
}

async fn probe_current_playable_codec(
    store: &crate::storage::Store,
    asset: &crate::models::LocalAsset,
) -> Option<CurrentPlayableCodec> {
    let current = store.get_asset_by_id(&asset.id).ok().flatten()?;
    if current.path != asset.path || current.hash != asset.hash {
        return None;
    }
    let settings = store.settings().ok()?;
    let mut command =
        tokio::process::Command::new(crate::tools::executable(&settings.ffprobe_path, "ffprobe"));
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name:format=format_name",
            "-of",
            "json",
        ])
        .arg(&asset.path);
    crate::tools::configure_no_console_tokio(&mut command);
    let output = crate::opus_decoder_capability::bounded_command_output(
        &mut command,
        4096,
        4096,
        std::time::Duration::from_secs(2),
    )
    .await?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let codec = value["streams"]
        .as_array()?
        .first()?
        .get("codec_name")?
        .as_str()?
        .to_owned();
    let container = value["format"].get("format_name")?.as_str()?.to_owned();
    Some(CurrentPlayableCodec { codec, container })
}

async fn set_registered_native_opus_decode_policy(
    store: &crate::storage::Store,
    source: &mut crate::native_output::AudioSource,
    mode: crate::loudness::NormalizationMode,
    range: crate::loudness::AudioRange,
) -> bool {
    if mode == crate::loudness::NormalizationMode::Off
        || range != crate::loudness::AudioRange::WHOLE_FILE
        || source.cenc.is_some()
        || source.input.starts_with("http://")
        || source.input.starts_with("https://")
        || source.input.starts_with("ytdl://")
    {
        return false;
    }
    let Ok(Some(asset)) = store.get_asset_by_path(&source.input) else {
        return false;
    };
    if !native_opus_decode_preprobe_eligible(source, &asset, mode, range) {
        return false;
    }
    let Some(probed) = probe_current_playable_codec(store, &asset).await else {
        return false;
    };
    if !native_opus_decode_policy_eligible(source, &asset, &probed, mode, range) {
        return false;
    }
    // Capability is cached only after a selected local source has triggered a
    // bounded self-test of this exact configured FFmpeg executable. The first
    // play remains unnormalized; only a later play can consume a passed result.
    let Some(confirmed_ffmpeg) =
        crate::opus_decoder_capability::confirmed_executable(&source.ffmpeg).await
    else {
        return false;
    };
    // Bind playback to the exact canonical executable whose identity passed,
    // rather than resolving a mutable PATH name again in the decoder worker.
    source.ffmpeg = confirmed_ffmpeg.executable_path;
    apply_native_opus_decode_policy(source, &asset, confirmed_ffmpeg.executable_sha256);
    true
}

impl PlaybackService {
    pub fn new(
        app: tauri::AppHandle,
        store: Arc<Store>,
        acquisition: Arc<AcquisitionManager>,
        history: Arc<PlaybackHistory>,
    ) -> Result<Arc<Self>, String> {
        let native = std::env::var("BURRATA_PLAYBACK_ENGINE")
            .ok()
            .map(|v| v == "native")
            .unwrap_or_else(|| {
                store
                    .get_json_preference(ENGINE_KEY)
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_str().map(|v| v == "native"))
                    .unwrap_or(false)
            });
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let output = NativeOutput::new(events_tx)?;
        let (updates, _) =
            watch::channel(json!({"engine":if native{"native"}else{"webview"},"status":"idle"}));
        let (audible, _) = watch::channel(false);
        let (radio_events, mut radio_rx) = mpsc::unbounded_channel();
        let mut state = State::default();
        if let Some(mut saved) = store.get_json_preference(SESSION_KEY)? {
            let merged = if saved.get("tracks").and_then(Value::as_array).is_none() {
                match store.get_json_preference(QUEUE_KEY)? {
                    Some(queue) => merge_persisted_queue(&mut saved, &queue),
                    None => Err("Saved playback queue is missing".into()),
                }
            } else {
                Ok(())
            };
            if merged
                .and_then(|()| restore_state(&mut state, &saved))
                .is_err()
            {
                state = State::default();
                state.message = "Saved playback state could not be restored".into();
            }
        }
        let service = Arc::new(Self {
            inner: Arc::new(Inner {
                app,
                observed_analysis: crate::observed_analysis::ObservedAnalysis::new(store.clone()),
                store,
                acquisition,
                history,
                state: Mutex::new(state),
                pending_native_sources: Mutex::new(HashMap::new()),
                persistence: Mutex::new(()),
                commands: tokio::sync::Mutex::new(()),
                native: AtomicBool::new(native),
                output,
                loudness_verification: Arc::new(
                    crate::loudness_manager::LocalVerificationMemo::default(),
                ),
                loudness_album_context: Mutex::new(None),
                updates,
                audible,
                playing_track_identifications: PlayingTrackIdentifications::default(),
                radio_events,
            }),
        });
        let weak = Arc::downgrade(&service);
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                let Some(service) = weak.upgrade() else { break };
                service.output_event(event).await;
            }
        });
        let weak = Arc::downgrade(&service);
        tauri::async_runtime::spawn(async move {
            while let Some(event) = radio_rx.recv().await {
                let Some(service) = weak.upgrade() else { break };
                service.radio_event(event);
            }
        });
        service.publish();
        Ok(service)
    }
    pub fn native_enabled(&self) -> bool {
        self.inner.native.load(Ordering::Acquire)
    }

    pub fn loudness_settings(&self) -> Result<crate::loudness_manager::LoudnessSettings, String> {
        crate::loudness_manager::load_settings(&self.inner.store)
    }

    pub fn set_loudness_album_context(
        &self,
        resource: Option<crate::loudness::LoudnessResourceIdentity>,
    ) -> Result<(), String> {
        if let Some(resource) = resource.as_ref() {
            resource
                .validate()
                .map_err(|_| "Album loudness resource identity is invalid".to_owned())?;
            if resource.scope != crate::loudness::ResourceScope::Album {
                return Err("Album context must identify an ordered album programme".into());
            }
            if resource
                .inter_track_gaps_seconds
                .iter()
                .any(|gap| *gap != 0.0)
            {
                return Err("Album loudness context must match gapless queue playback".into());
            }
        }
        *self
            .inner
            .loudness_album_context
            .lock()
            .map_err(|_| "Album loudness context is unavailable")? = resource;
        Ok(())
    }

    /// Return the shared normalization decision for one exact registered local
    /// asset. WebView callers use the same trusted tags, cache and verification
    /// policy as native output; caller supplied paths and metadata are ignored.
    pub fn loudness_decision_for_registered_asset(
        &self,
        asset_id: &str,
        range: crate::loudness::AudioRange,
        album_resource: Option<crate::loudness::LoudnessResourceIdentity>,
    ) -> Result<crate::loudness_manager::LoudnessStatus, String> {
        let settings = crate::loudness_manager::load_settings(&self.inner.store)?;
        if settings.mode == crate::loudness::NormalizationMode::Off {
            return Ok(crate::loudness_manager::LoudnessStatus::off());
        }
        range
            .validate()
            .map_err(|_| "Loudness playback range is invalid".to_owned())?;
        let asset = self
            .inner
            .store
            .get_asset_by_id(asset_id)
            .map_err(|error| error.to_string())?
            .ok_or("Selected local audio asset is no longer registered")?;
        let effective_range = if is_cropped_chapter_asset(&asset.track, &asset) {
            crate::loudness::AudioRange::WHOLE_FILE
        } else {
            range
        };
        if let Some(resource) = album_resource.as_ref() {
            validate_loudness_album_resource_member(
                resource,
                &asset.id,
                &asset.hash,
                effective_range,
            )?;
            for member in &resource.members {
                let registered = self
                    .inner
                    .store
                    .get_asset_by_id(&member.asset_id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Album loudness member is no longer registered")?;
                if registered.hash != member.file_hash {
                    return Err("Album loudness member changed since programme selection".into());
                }
            }
        }
        let source = crate::native_output::AudioSource {
            input: asset.path.clone(),
            ffmpeg: self
                .inner
                .store
                .settings()
                .map(|settings| crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"))
                .unwrap_or_else(|_| "ffmpeg".into()),
            headers: vec![],
            decoder_hint: None,
            cenc: None,
        };
        let (_, status) = self.resolved_source_loudness_with_album_context(
            &settings,
            &source,
            &asset.track,
            None,
            None,
            range,
            album_resource,
            false,
            true,
            true,
        );
        Ok(status)
    }

    pub fn set_loudness_settings(
        &self,
        settings: crate::loudness_manager::LoudnessSettings,
    ) -> Result<crate::loudness_manager::LoudnessStatus, String> {
        let previous =
            crate::loudness_manager::load_settings(&self.inner.store).unwrap_or_default();
        let settings_changed = previous != settings;
        crate::loudness_manager::save_settings(&self.inner.store, &settings)?;
        self.refresh_normalization_with_settings(settings, settings_changed)
    }

    pub fn refresh_normalization(&self) -> Result<crate::loudness_manager::LoudnessStatus, String> {
        let settings = crate::loudness_manager::load_settings(&self.inner.store)?;
        self.refresh_normalization_with_settings(settings, false)
    }

    fn refresh_normalization_with_settings(
        &self,
        settings: crate::loudness_manager::LoudnessSettings,
        settings_changed: bool,
    ) -> Result<crate::loudness_manager::LoudnessStatus, String> {
        let (
            generation,
            track,
            candidate,
            station,
            cancel,
            active_opus_decoder_capability,
            queued_preload,
            unresolved_preload,
        ) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Fence resolver-only work while holding the same state lock it
            // must acquire to publish its resolved source and queued PCM.
            let unresolved_preload =
                fence_unresolved_preload_for_settings_change(&mut state, settings_changed);
            let queued_preload = state.preload.as_ref().and_then(|preload| {
                Some((
                    preload.generation,
                    preload.index,
                    preload.entry.clone(),
                    state.queue.get(preload.index)?.clone(),
                    preload.candidate.is_some(),
                    crate::loudness::AudioRange {
                        start_seconds: preload.offset,
                        end_seconds: preload.end,
                    },
                    preload.loudness_album_context.clone(),
                    preload.opus_decoder_capability.clone(),
                    state
                        .preload_cancel
                        .as_ref()
                        .map(|sender| sender.subscribe()),
                ))
            });
            (
                state.generation,
                state.track().cloned(),
                state.candidate.clone(),
                state.station.clone(),
                state.cancel.as_ref().map(|sender| sender.subscribe()),
                state.opus_decoder_capability.clone(),
                queued_preload,
                unresolved_preload,
            )
        };
        let plan = if settings.mode == crate::loudness::NormalizationMode::Off {
            crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                crate::loudness_manager::LoudnessStatus::off(),
            )
        } else if !self.native_enabled() {
            crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                crate::loudness_manager::LoudnessStatus::unavailable(
                    settings.mode,
                    "The active WebView audio route has not reported normalization capability",
                    None,
                ),
            )
        } else if station.is_some() || candidate.is_some() {
            crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                crate::loudness_manager::LoudnessStatus::unavailable(
                    settings.mode,
                    "This remote or live audio source has no trusted resource-bound measurement",
                    None,
                ),
            )
        } else if let Some(track) = track.as_ref() {
            self.native_loudness_plan_for_registered_track(
                &settings,
                track,
                cancel,
                active_opus_decoder_capability,
            )
        } else {
            crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                crate::loudness_manager::LoudnessStatus::unavailable(
                    settings.mode,
                    "No active audio resource is selected",
                    None,
                ),
            )
        };
        let queued_plan = queued_preload.as_ref().map(
            |(
                queued_generation,
                _index,
                _entry,
                queued_track,
                queued_has_candidate,
                queued_range,
                queued_album_context,
                queued_opus_decoder_capability,
                queued_cancel,
            )| {
                let plan = if settings.mode == crate::loudness::NormalizationMode::Off {
                    crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                        crate::loudness_manager::LoudnessStatus::off(),
                    )
                } else if !self.native_enabled() {
                    crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                        crate::loudness_manager::LoudnessStatus::unavailable(
                            settings.mode,
                            "The active WebView audio route has not reported normalization capability",
                            None,
                        ),
                    )
                } else if *queued_has_candidate || queued_track.local_path.is_none() {
                    crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                        crate::loudness_manager::LoudnessStatus::unavailable(
                            settings.mode,
                            "This remote or live audio source has no trusted resource-bound measurement",
                            None,
                        ),
                    )
                } else {
                    let queued_source = registered_source_with_opus_capability(
                        queued_track.local_path.as_deref().unwrap_or_default(),
                        queued_opus_decoder_capability.as_ref(),
                    );
                    self.resolved_native_loudness_plan(
                        &settings,
                        &queued_source,
                        queued_track,
                        None,
                        queued_cancel.clone(),
                        *queued_range,
                        queued_album_context.clone(),
                        false,
                    )
                };
                (*queued_generation, plan)
            },
        );
        let status = plan.pending_status();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.generation == generation {
                state.loudness_status = status.clone();
                state.revision += 1;
            }
            if let (
                Some((queued_generation, queued_plan)),
                Some((_, queued_index, queued_entry, _, _, _, _, _, _)),
            ) = (queued_plan.as_ref(), queued_preload.as_ref())
            {
                let occurrence_matches = state.entries.get(*queued_index) == Some(queued_entry);
                if occurrence_matches {
                    if let Some(preload) = state.preload.as_mut().filter(|preload| {
                        preload.generation == *queued_generation
                            && preload.index == *queued_index
                            && preload.entry == *queued_entry
                    }) {
                        preload.loudness_status = queued_plan.pending_status();
                    }
                }
            }
        }
        self.publish();
        let active_update = if self.native_enabled() && generation > 0 {
            self.inner
                .output
                .send(OutputCommand::NormalizationGainPlan {
                    generation,
                    plan,
                    ramp_ms: 80,
                })
        } else {
            Ok(())
        };
        let queued_update = if self.native_enabled() {
            if let Some((queued_generation, queued_plan)) = queued_plan {
                self.inner
                    .output
                    .send(OutputCommand::NormalizationGainPlan {
                        generation: queued_generation,
                        plan: queued_plan,
                        ramp_ms: 80,
                    })
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        if unresolved_preload {
            // No queued PCM existed at the locked fence; the canceled owner
            // cannot enqueue after it. A fresh prepare may already own the slot.
            self.prepare_next();
        }
        active_update?;
        queued_update?;
        if settings_changed {
            let current_generation = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .generation;
            if current_generation != generation {
                let current_settings =
                    crate::loudness_manager::load_settings(&self.inner.store).unwrap_or(settings);
                return self.refresh_normalization_with_settings(current_settings, false);
            }
        }
        Ok(status)
    }

    fn native_loudness_plan_for_registered_track(
        &self,
        settings: &crate::loudness_manager::LoudnessSettings,
        track: &Track,
        cancel: Option<watch::Receiver<bool>>,
        opus_decoder_capability: Option<ActiveOpusDecoderCapability>,
    ) -> crate::loudness_manager::NativeNormalizationGainPlan {
        let album_context = self
            .inner
            .loudness_album_context
            .lock()
            .ok()
            .and_then(|context| context.clone());
        self.native_loudness_plan_for_track_context(
            settings,
            track,
            cancel,
            album_context,
            true,
            opus_decoder_capability,
        )
    }

    fn native_loudness_plan_for_track_context(
        &self,
        settings: &crate::loudness_manager::LoudnessSettings,
        track: &Track,
        cancel: Option<watch::Receiver<bool>>,
        album_context: Option<crate::loudness::LoudnessResourceIdentity>,
        clear_active_context_on_mismatch: bool,
        opus_decoder_capability: Option<ActiveOpusDecoderCapability>,
    ) -> crate::loudness_manager::NativeNormalizationGainPlan {
        let Some(path) = track.local_path.as_deref() else {
            return crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                crate::loudness_manager::LoudnessStatus::unavailable(
                    settings.mode,
                    "The active audio resource is not a registered local asset",
                    None,
                ),
            );
        };
        let (start_seconds, end_seconds) = self.source_bounds(track);
        let source = registered_source_with_opus_capability(path, opus_decoder_capability.as_ref());
        self.resolved_native_loudness_plan(
            settings,
            &source,
            track,
            None,
            cancel,
            crate::loudness::AudioRange {
                start_seconds,
                end_seconds,
            },
            album_context,
            clear_active_context_on_mismatch,
        )
    }

    fn schedule_local_tag_verification(
        &self,
        resource: crate::loudness::LoudnessResourceIdentity,
        cancel: Option<watch::Receiver<bool>>,
    ) {
        let store = self.inner.store.clone();
        let memo = self.inner.loudness_verification.clone();
        if !memo
            .begin_tag_verification(&store, &resource)
            .unwrap_or(false)
        {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let _ = memo
                .prepare_local_tags(store, resource.clone(), cancel)
                .await;
            memo.finish_tag_verification(&resource);
        });
    }

    fn schedule_local_resource_verification(
        &self,
        resource: crate::loudness::LoudnessResourceIdentity,
        cancel: Option<watch::Receiver<bool>>,
    ) {
        let store = self.inner.store.clone();
        let memo = self.inner.loudness_verification.clone();
        tauri::async_runtime::spawn(async move {
            let _ = memo.verify_and_remember(store, resource, cancel).await;
        });
    }

    fn resolved_source_loudness(
        &self,
        settings: &crate::loudness_manager::LoudnessSettings,
        source: &crate::native_output::AudioSource,
        track: &Track,
        station: Option<&RadioStation>,
        cancel: Option<watch::Receiver<bool>>,
        range: crate::loudness::AudioRange,
    ) -> (f32, crate::loudness_manager::LoudnessStatus) {
        let album_context = self
            .inner
            .loudness_album_context
            .lock()
            .ok()
            .and_then(|context| context.clone());
        self.resolved_source_loudness_with_album_context(
            settings,
            source,
            track,
            station,
            cancel,
            range,
            album_context,
            true,
            false,
            true,
        )
    }

    fn resolved_native_loudness_plan(
        &self,
        settings: &crate::loudness_manager::LoudnessSettings,
        source: &crate::native_output::AudioSource,
        track: &Track,
        station: Option<&RadioStation>,
        cancel: Option<watch::Receiver<bool>>,
        range: crate::loudness::AudioRange,
        album_context: Option<crate::loudness::LoudnessResourceIdentity>,
        clear_active_context_on_mismatch: bool,
    ) -> crate::loudness_manager::NativeNormalizationGainPlan {
        let source_route = self.resolved_source_loudness_with_album_context(
            settings,
            source,
            track,
            station,
            cancel.clone(),
            range,
            album_context.clone(),
            clear_active_context_on_mismatch,
            false,
            true,
        );
        if settings.mode == crate::loudness::NormalizationMode::Off {
            return crate::loudness_manager::NativeNormalizationGainPlan::fixed(source_route.1);
        }
        let registered_mono = self
            .inner
            .store
            .get_asset_by_path(&source.input)
            .ok()
            .flatten()
            .filter(|asset| asset.path == source.input && asset.channels == Some(1))
            .is_some_and(|asset| {
                let resource =
                    crate::loudness::LoudnessResourceIdentity::track(&asset.id, &asset.hash, range);
                crate::loudness_manager::cached_record(&self.inner.store, &resource, None)
                    .ok()
                    .flatten()
                    .is_some_and(|record| record.channels == 1)
            });
        if !registered_mono {
            return crate::loudness_manager::NativeNormalizationGainPlan::fixed(source_route.1);
        }
        let stereo_route = self.resolved_source_loudness_with_album_context(
            settings,
            source,
            track,
            station,
            cancel,
            range,
            album_context,
            false,
            true,
            false,
        );
        crate::loudness_manager::NativeNormalizationGainPlan::mono(
            source_route.1,
            Some(stereo_route.1),
        )
    }

    fn resolved_source_loudness_with_album_context(
        &self,
        settings: &crate::loudness_manager::LoudnessSettings,
        source: &crate::native_output::AudioSource,
        track: &Track,
        station: Option<&RadioStation>,
        cancel: Option<watch::Receiver<bool>>,
        range: crate::loudness::AudioRange,
        album_context: Option<crate::loudness::LoudnessResourceIdentity>,
        clear_active_context_on_mismatch: bool,
        web_audio_stereo_route: bool,
        schedule_verification: bool,
    ) -> (f32, crate::loudness_manager::LoudnessStatus) {
        use crate::loudness::{GainDecision, MeasurementProvenance, NormalizationMode};
        use crate::loudness_manager::{LoudnessStatus, LoudnessStatusKind};

        let input = source.input.as_str();
        if station.is_some()
            || input.starts_with("http://")
            || input.starts_with("https://")
            || input.starts_with("ytdl://")
        {
            if clear_active_context_on_mismatch {
                if let Ok(mut context) = self.inner.loudness_album_context.lock() {
                    *context = None;
                }
            }
            return if settings.mode == NormalizationMode::Off {
                (0.0, LoudnessStatus::off())
            } else {
                (
                    0.0,
                    LoudnessStatus::unavailable(
                        settings.mode,
                        "This remote or live audio source has no resource-bound measurement",
                        None,
                    ),
                )
            };
        }
        let asset = match self.inner.store.get_asset_by_path(input) {
            Ok(Some(asset)) if asset.path == input => asset,
            _ => {
                if clear_active_context_on_mismatch {
                    if let Ok(mut context) = self.inner.loudness_album_context.lock() {
                        *context = None;
                    }
                }
                return if settings.mode == NormalizationMode::Off {
                    (0.0, LoudnessStatus::off())
                } else {
                    (
                        0.0,
                        LoudnessStatus::unavailable(
                            settings.mode,
                            "This audio source is not a registered local asset",
                            None,
                        ),
                    )
                };
            }
        };
        let range = if is_cropped_chapter_asset(track, &asset) {
            crate::loudness::AudioRange::WHOLE_FILE
        } else {
            range
        };
        let resource = crate::loudness::LoudnessResourceIdentity::track(
            asset.id.clone(),
            asset.hash.clone(),
            range,
        );
        let resource_key = Some(resource.resource_key.clone());
        let is_opus = asset.final_codec.eq_ignore_ascii_case("opus")
            || std::path::Path::new(input)
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "opus" | "webm")
                });
        // Cache lookup compares the full typed resource (including range), so
        // a future range analyzer can authorize only its exact measured cut.
        let cached = crate::loudness_manager::cached_record(&self.inner.store, &resource, None)
            .ok()
            .flatten();
        let album_resource = album_context.clone().filter(|context| {
            context.scope == crate::loudness::ResourceScope::Album
                && context.members.iter().any(|member| {
                    member.asset_id == asset.id
                        && member.file_hash == asset.hash
                        && member.range == range
                })
        });
        if clear_active_context_on_mismatch && album_context.is_some() && album_resource.is_none() {
            if let Ok(mut context) = self.inner.loudness_album_context.lock() {
                if context.as_ref().is_some_and(|current| {
                    Some(current.resource_key.as_str())
                        == album_context
                            .as_ref()
                            .map(|value| value.resource_key.as_str())
                }) {
                    *context = None;
                }
            }
        }
        if settings.mode == NormalizationMode::Off {
            return (0.0, LoudnessStatus::off());
        }
        let album_cached = album_resource.as_ref().and_then(|album_resource| {
            crate::loudness_manager::cached_record(&self.inner.store, album_resource, None)
                .ok()
                .flatten()
        });
        let album_verified = album_resource.as_ref().is_some_and(|album_resource| {
            self.inner
                .loudness_verification
                .is_currently_verified(&self.inner.store, album_resource, album_cached.as_ref())
                .unwrap_or(false)
        });
        if schedule_verification && album_cached.is_some() && !album_verified {
            self.schedule_local_resource_verification(
                album_resource.clone().unwrap(),
                cancel.clone(),
            );
        }
        let track_verified = self
            .inner
            .loudness_verification
            .is_currently_verified(&self.inner.store, &resource, cached.as_ref())
            .unwrap_or(false);
        // A complete programme seal authenticates every member. Album mode
        // can use that analysis on the first start even before a separate
        // per-track tag-verification memo has been populated.
        let verified = track_verified || album_verified;
        if !verified {
            if schedule_verification {
                if cached.is_some() {
                    self.schedule_local_resource_verification(resource.clone(), cancel);
                } else if range == crate::loudness::AudioRange::WHOLE_FILE {
                    self.schedule_local_tag_verification(resource.clone(), cancel);
                }
            }
            return (
                0.0,
                LoudnessStatus::unavailable(
                    settings.mode,
                    if range == crate::loudness::AudioRange::WHOLE_FILE {
                        "Local audio hash has not been verified in this session; normalization is available on the next play"
                    } else if cached.is_some() {
                        "The exact excerpt measurement is awaiting local file verification"
                    } else {
                        "No trusted measurement covers this exact playback range"
                    },
                    resource_key,
                ),
            );
        }
        let decoder_fingerprint = source
            .decoder_hint
            .as_ref()
            .and_then(|hint| hint.decoder_fingerprint.as_deref())
            .filter(|value| {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        let opus_header_gain_decoder = source.cenc.is_none()
            && !source.input.starts_with("http://")
            && !source.input.starts_with("https://")
            && !source.input.starts_with("ytdl://")
            && source.input == asset.path
            && decoder_fingerprint.is_some()
            && source.decoder_hint.as_ref().is_some_and(|hint| {
                hint.decode_policy
                    == crate::native_output::AudioSourceDecodePolicy::FfmpegOggOpusHeadGain
                    && hint.mime_type.as_deref() == Some("audio/ogg")
                    && hint
                        .codec
                        .as_deref()
                        .is_some_and(|codec| codec.eq_ignore_ascii_case("opus"))
            });
        let track_opus_proof = is_opus
            && decoder_fingerprint.is_some_and(|fingerprint| {
                cached.as_ref().is_some_and(|record| {
                    record
                        .opus_decoder_proofs
                        .iter()
                        .any(|proof| proof.validates_for(&asset.id, &asset.hash, fingerprint))
                })
            });
        let album_opus_proof = is_opus
            && decoder_fingerprint.is_some_and(|fingerprint| {
                album_cached.as_ref().is_some_and(|record| {
                    record
                        .opus_decoder_proofs
                        .iter()
                        .any(|proof| proof.validates_for(&asset.id, &asset.hash, fingerprint))
                })
            });
        if is_opus && (!opus_header_gain_decoder || (!track_opus_proof && !album_opus_proof)) {
            return (
                0.0,
                LoudnessStatus::unavailable(
                    settings.mode,
                    if range != crate::loudness::AudioRange::WHOLE_FILE {
                        "Opus normalization requires a verified whole-file measurement"
                    } else if !opus_header_gain_decoder {
                        "This playback started without a verified Opus decoder. After successful analysis, replay the track to enable normalization."
                    } else {
                        "This Opus measurement lacks proof for the active decoder; analyze the local track again"
                    },
                    resource_key,
                ),
            );
        }
        // A current analysis proof is per resource and decoder. Never use an
        // older Opus track/album LUFS or peak simply because another resource
        // (or another member in the programme) was analyzed by this decoder.
        let cached = if is_opus && !track_opus_proof {
            None
        } else {
            cached
        };
        let album_cached = if is_opus && !album_opus_proof {
            None
        } else {
            album_cached
        };
        let album_verified = album_verified && (!is_opus || album_opus_proof);
        let tags = self
            .inner
            .loudness_verification
            .cached_tags(&self.inner.store, &resource)
            .ok()
            .flatten()
            .filter(|tags| tags.applies_to_range(range));
        if tags.is_none()
            && range == crate::loudness::AudioRange::WHOLE_FILE
            && schedule_verification
        {
            // A verified analysis can be used immediately, while a bounded
            // tag read prepares valid ReplayGain/R128 metadata for the next
            // play. This never changes the active gain mid-track.
            self.schedule_local_tag_verification(resource.clone(), cancel.clone());
        }
        let policy = match settings.gain_policy() {
            Ok(policy) => policy,
            Err(_) => {
                return (
                    0.0,
                    LoudnessStatus::unavailable(
                        settings.mode,
                        "Loudness settings are invalid",
                        resource_key,
                    ),
                )
            }
        };
        let exact_true_peak = cached
            .as_ref()
            .and_then(|record| record.playback_true_peak_bound_dbtp(web_audio_stereo_route))
            .or_else(|| {
                album_verified
                    .then_some(album_cached.as_ref())
                    .flatten()
                    .and_then(|record| record.playback_true_peak_bound_dbtp(web_audio_stereo_route))
            });
        let track_tag = tags.as_ref().and_then(|tags| tags.track_replaygain);
        let album_tag = tags.as_ref().and_then(|tags| {
            tags.album_identity_matches(track.album.as_deref(), track.album_artist.as_deref())
                .then_some(tags.album_replaygain)
                .flatten()
        });

        let track_decision = || {
            local_track_normalization_decision(
                tags.as_ref(),
                is_opus,
                range,
                exact_true_peak,
                opus_header_gain_decoder,
                cached.as_ref(),
                policy,
                NormalizationMode::Track,
                web_audio_stereo_route,
            )
        };
        let selected = match settings.mode {
            NormalizationMode::Off => None,
            NormalizationMode::Track => {
                track_decision().map(|(gain, source)| (gain, source, LoudnessStatusKind::Applied))
            }
            NormalizationMode::Album => {
                if album_verified {
                    album_cached.as_ref().and_then(|record| {
                        let integrated = if web_audio_stereo_route {
                            record.playback_integrated_lufs_for_route(true)?
                        } else {
                            record.playback_integrated_lufs_for_route(false)?
                        };
                        match (crate::loudness::LoudnessMeasurement {
                            integrated_lufs: integrated,
                            true_peak_dbtp: None,
                        })
                        .decide_gain_with_output_peak_bound(
                            record.playback_true_peak_bound_dbtp(web_audio_stereo_route),
                            policy,
                            NormalizationMode::Album,
                        ) {
                            GainDecision::ApplyDb(gain) => {
                                Some((gain, record.provenance, LoudnessStatusKind::Applied))
                            }
                            _ => None,
                        }
                    })
                } else if is_opus
                    && tags
                        .as_ref()
                        .is_some_and(crate::loudness_analysis::opus_comment_schemes_are_mixed)
                {
                    if settings.album_track_fallback {
                        track_decision().map(|(gain, source)| {
                            (gain, source, LoudnessStatusKind::AlbumFallbackTrack)
                        })
                    } else {
                        None
                    }
                } else if is_opus
                    && tags
                        .as_ref()
                        .is_some_and(|tags| tags.opus_r128_album_tag_present)
                {
                    let opus_tags = tags.as_ref().expect("presence checked above");
                    if !opus_r128_album_context_matches(
                        opus_tags,
                        track,
                        album_resource.as_ref(),
                        &asset.id,
                        &asset.hash,
                        range,
                    ) {
                        None
                    } else {
                        match crate::loudness_analysis::decide_verified_opus_r128_tag(
                            opus_tags,
                            true,
                            true,
                            range,
                            exact_true_peak,
                            opus_header_gain_decoder,
                            policy,
                            NormalizationMode::Album,
                        ) {
                            GainDecision::ApplyDb(gain) => Some((
                                gain,
                                MeasurementProvenance::OpusR128Album,
                                LoudnessStatusKind::Applied,
                            )),
                            _ => None,
                        }
                    }
                } else if let Some(tags) = album_tag {
                    match tags.decide_gain_with_true_peak(policy, NormalizationMode::Album, None) {
                        GainDecision::ApplyDb(gain) => Some((
                            gain,
                            MeasurementProvenance::ReplayGainAlbum,
                            LoudnessStatusKind::Applied,
                        )),
                        _ => None,
                    }
                } else if settings.album_track_fallback {
                    track_decision().map(|(gain, source)| {
                        (gain, source, LoudnessStatusKind::AlbumFallbackTrack)
                    })
                } else {
                    None
                }
            }
        };
        if let Some((gain, source, status)) = selected {
            let safety_record = if settings.mode == NormalizationMode::Album
                && status == LoudnessStatusKind::Applied
                && album_verified
            {
                album_cached.as_ref()
            } else {
                cached.as_ref()
            };
            let requested_gain = match source {
                MeasurementProvenance::ReplayGainTrack => {
                    track_tag.map(|tag| tag.gain_db + policy.target_lufs + 18.0)
                }
                MeasurementProvenance::ReplayGainAlbum => None,
                MeasurementProvenance::OpusR128Track => tags
                    .as_ref()
                    .and_then(|tags| tags.opus_r128_track_gain_db)
                    .map(|gain| gain + policy.target_lufs + 23.0),
                MeasurementProvenance::OpusR128Album => tags
                    .as_ref()
                    .and_then(|tags| tags.opus_r128_album_gain_db)
                    .map(|gain| gain + policy.target_lufs + 23.0),
                _ => safety_record
                    .and_then(|record| record.integrated_lufs)
                    .map(|integrated| policy.target_lufs - integrated),
            };
            let channel_safety_limited = safety_record.is_some_and(|record| {
                let Some(bound) = record.playback_true_peak_bound_dbtp(web_audio_stereo_route)
                else {
                    return false;
                };
                let cap = policy.ceiling_dbtp - bound;
                record.uses_conservative_channel_sum_bound(web_audio_stereo_route)
                    && requested_gain.is_some_and(|requested| requested > cap + 0.01)
                    && (gain - cap).abs() <= 0.01
            });
            let selected_resource_key = if settings.mode == NormalizationMode::Album
                && status == LoudnessStatusKind::Applied
                && album_verified
            {
                album_resource
                    .as_ref()
                    .map(|album_resource| album_resource.resource_key.clone())
                    .unwrap_or_else(|| resource.resource_key.clone())
            } else {
                resource.resource_key.clone()
            };
            return (
                gain as f32,
                LoudnessStatus {
                    mode: settings.mode,
                    status,
                    applied_gain_db: gain,
                    measurement_source: Some(source),
                    reason: match (
                        status == LoudnessStatusKind::AlbumFallbackTrack,
                        channel_safety_limited,
                    ) {
                        (true, true) => Some("Album measurement unavailable; using track measurement. Gain was limited to prevent overload when mixing channels".to_owned()),
                        (true, false) => Some("Album measurement unavailable; using track measurement".to_owned()),
                        (false, true) => Some("Gain was limited to prevent overload when mixing channels".to_owned()),
                        (false, false) => None,
                    },
                    source_resource_key: Some(selected_resource_key),
                },
            );
        }
        let mono_stereo_analysis_missing = web_audio_stereo_route
            && match settings.mode {
                NormalizationMode::Off => false,
                NormalizationMode::Track => {
                    track_tag.is_none()
                        && cached.as_ref().is_some_and(|record| {
                            record.channels == 1 && record.stereo_mix_integrated_lufs.is_none()
                        })
                }
                NormalizationMode::Album if album_verified => {
                    album_tag.is_none()
                        && album_cached.as_ref().is_some_and(|record| {
                            record.channels == 1 && record.stereo_mix_integrated_lufs.is_none()
                        })
                }
                NormalizationMode::Album if settings.album_track_fallback => {
                    track_tag.is_none()
                        && cached.as_ref().is_some_and(|record| {
                            record.channels == 1 && record.stereo_mix_integrated_lufs.is_none()
                        })
                }
                NormalizationMode::Album => false,
            };
        (
            0.0,
            LoudnessStatus::unavailable(
                settings.mode,
                if mono_stereo_analysis_missing {
                    "Analyze this mono track again for stereo playback"
                } else if settings.mode == NormalizationMode::Album {
                    "No trusted album measurement is available"
                } else {
                    "No trusted measurement is available for this audio resource"
                },
                resource_key,
            ),
        )
    }

    /// Release unresolved CENC source leases owned by superseded requests.
    /// A new playback generation or pause/stop invalidation should call this
    /// alongside cancellation of resolver and WebView transport work.
    pub(crate) fn cancel_pending_native_sources_older_than(&self, generation: u64) -> usize {
        self.inner
            .pending_native_sources
            .lock()
            .map(|mut pending| purge_pending_native_sources_older_than(&mut pending, generation))
            .unwrap_or(0)
    }

    /// Provider-session changes invalidate every pending resolved source.
    pub(crate) fn cancel_all_pending_native_sources(&self) -> usize {
        self.inner
            .pending_native_sources
            .lock()
            .map(|mut pending| {
                let count = pending.len();
                pending.clear();
                count
            })
            .unwrap_or(0)
    }
    /// Explicit user action: checkpoint both engine stores, then restart while
    /// holding persistence serialization so an older session cannot overwrite it.
    pub async fn restart_with_engine(
        &self,
        engine: crate::playback_engine_settings::Engine,
        saved: Value,
    ) -> Result<(), String> {
        if std::env::var("BURRATA_PLAYBACK_ENGINE").is_ok() {
            return Err("This launch uses a fixed audio engine".into());
        }
        let _commands = self.inner.commands.lock().await;
        let _persistence = self
            .inner
            .persistence
            .lock()
            .map_err(|_| "Playback persistence unavailable")?;
        let (queue, session) = engine_switch_checkpoint(&saved)?;
        if self.native_enabled() {
            let current = self
                .inner
                .state
                .lock()
                .map_err(|_| "Playback state unavailable")?;
            if current.status == "playing"
                || current.status == "resolving"
                || current.status == "buffering"
                || saved["entryIds"] != json!(current.entries)
                || saved
                    .get("queueLoudnessContexts")
                    .cloned()
                    .unwrap_or_else(|| json!({}))
                    != json!(current.loudness_queue_contexts)
                || saved["currentEntryId"] != json!(current.entries.get(current.index))
                || saved["queueRevision"].as_u64() != Some(current.queue_revision)
                || (saved["position"].as_f64().unwrap_or(-1.) - current.position).abs() > 0.05
            {
                return Err("Playback changed during engine switching. Try again.".into());
            }
        }
        let selected = serde_json::to_value(engine).map_err(|_| "Invalid audio engine")?;
        self.inner.store.save_playback_checkpoint_and_engine(
            Some(&queue),
            Some(&session),
            Some(&selected),
        )?;
        self.inner.app.restart();
    }

    /// Serialize a background file replacement with playback ownership. A new
    /// play request cannot acquire this recording between this check and commit.
    pub(crate) fn commit_background_upgrade(
        &self,
        asset: &crate::models::LocalAsset,
        disposition: &crate::models::ReplacementDisposition,
    ) -> Result<crate::models::LocalAsset, String> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| "Playback ownership is unavailable")?;
        if self.recording_in_use_locked(&state, &asset.recording_id)? {
            return Err(
                "Lossless upgrade needs review because this recording became active".into(),
            );
        }
        self.inner.store.commit_download_asset(asset, disposition)
    }
    fn recording_in_use_locked(&self, state: &State, recording_id: &str) -> Result<bool, String> {
        if !self.native_enabled() {
            return Ok(true);
        }
        if matches!(state.status.as_str(), "idle" | "failed") {
            return Ok(false);
        }
        match state.track() {
            Some(track) => Ok(self.inner.store.recording_id(&track.id)? == recording_id),
            None => Ok(false),
        }
    }
    pub(crate) fn recording_in_use(&self, recording_id: &str) -> Result<bool, String> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| "Playback ownership is unavailable")?;
        self.recording_in_use_locked(&state, recording_id)
    }

    pub fn snapshot(&self) -> Value {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot_with_failure_history(self.native_enabled(), true)
    }

    /// Persist and return an authoritative, unpaged native playback checkpoint.
    /// WebView queue collection belongs to the frontend; when WebView is active,
    /// this reports that engine and revisions without exporting the native
    /// service's stale or partial queue as a replacement.
    pub fn checkpoint_for_backup(&self) -> Result<PlaybackBackupCheckpoint, String> {
        let _persistence = self
            .inner
            .persistence
            .lock()
            .map_err(|_| "Playback persistence unavailable")?;
        let native = self.native_enabled();
        let (revision, queue_revision, queue, session, session_key) = {
            let state = self
                .inner
                .state
                .lock()
                .map_err(|_| "Playback state unavailable")?;
            if !native {
                return Ok(PlaybackBackupCheckpoint {
                    engine: "webview".into(),
                    revision: state.revision,
                    queue_revision: state.queue_revision,
                    queue: None,
                    session: None,
                });
            }
            let (queue, session) = state.backup_checkpoint_values();
            (
                state.revision,
                state.queue_revision,
                queue,
                session,
                persisted_session_key(&state),
            )
        };
        self.inner.store.save_playback_checkpoint_and_engine(
            Some(&queue),
            Some(&session),
            Some(&json!("native")),
        )?;
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| "Playback state unavailable")?;
        if state.queue_revision == queue_revision {
            state.persisted_queue_revision = Some(queue_revision);
        }
        if persisted_session_key(&state) == session_key {
            state.persisted_session = Some(session_key);
        }
        Ok(PlaybackBackupCheckpoint {
            engine: "native".into(),
            revision,
            queue_revision,
            queue: Some(queue),
            session: Some(session),
        })
    }

    /// Estimate BPM and provisional key only from the current native playback
    /// generation. These are ephemeral display values, never Track metadata.
    pub async fn live_playback_estimates(&self) -> Result<Option<LivePlaybackEstimates>, String> {
        if !self.native_enabled() || !self.inner.store.settings()?.live_bpm_enabled {
            return Ok(None);
        }
        let (generation, seek_serial, track_id) = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.status != "playing" {
                return Ok(None);
            }
            let Some(track) = state.track() else {
                return Ok(None);
            };
            (state.generation, state.seek_serial, track.id.clone())
        };
        let tap = self.inner.output.tap();
        if tap.available_seconds(generation) < crate::live_bpm::LIVE_BPM_WINDOW_SECONDS as f64 {
            return Ok(None);
        }
        let Some(worker_permit) = crate::live_analysis_gate::try_acquire() else {
            return Ok(None);
        };

        let analyzed = tokio::task::spawn_blocking(move || {
            let _worker_permit = worker_permit;
            if tap.available_seconds(generation) < crate::live_bpm::LIVE_BPM_WINDOW_SECONDS as f64 {
                return None;
            }
            let (sample_rate, channels, samples) = tap
                .capture_pcm(generation, crate::live_bpm::LIVE_BPM_WINDOW_SECONDS as f64)
                .ok()?;
            let bpm = crate::live_bpm::estimate_interleaved_pcm(&samples, sample_rate, channels);
            let key = crate::live_key::estimate_interleaved_pcm(&samples, sample_rate, channels);
            Some((bpm, key))
        })
        .await
        .map_err(|error| format!("Live BPM analysis worker failed: {error}"))?;
        let Some((bpm, key)) = analyzed else {
            return Ok(None);
        };
        if bpm.is_none() && key.is_none() {
            return Ok(None);
        }

        if !self.native_enabled() || !self.inner.store.settings()?.live_bpm_enabled {
            return Ok(None);
        }
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status != "playing"
            || state.generation != generation
            || state.seek_serial != seek_serial
            || state.track().map(|track| track.id.as_str()) != Some(track_id.as_str())
        {
            return Ok(None);
        }
        Ok(Some(LivePlaybackEstimates {
            bpm: bpm.map(|estimate| LiveBpmPlaybackEstimate {
                generation,
                seek_serial,
                track_id: track_id.clone(),
                estimate,
            }),
            key: key.map(|estimate| LiveKeyPlaybackEstimate {
                generation,
                seek_serial,
                track_id,
                estimate,
            }),
        }))
    }

    /// Compatibility getter for clients that still request only live BPM.
    pub async fn live_bpm(&self) -> Result<Option<LiveBpmPlaybackEstimate>, String> {
        Ok(self
            .live_playback_estimates()
            .await?
            .and_then(|estimates| estimates.bpm))
    }
    pub fn cached_stream_analysis(
        &self,
        track_id: &str,
        generation: u64,
    ) -> Option<crate::stream_analysis::WholeTrackAnalysis> {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation || state.track()?.id != track_id {
            return None;
        }
        self.inner
            .observed_analysis
            .cached(state.track()?, state.candidate.as_ref())
            .or_else(|| {
                if state.station.is_some() {
                    return None;
                }
                self.inner.observed_analysis.session_cached(
                    generation,
                    state.track()?,
                    state.candidate.as_ref(),
                )
            })
    }

    pub(crate) fn observed_analysis_manager(
        &self,
    ) -> Arc<crate::observed_analysis::ObservedAnalysis> {
        self.inner.observed_analysis.clone()
    }

    /// Return bounded summaries; raw audio stays in the native tap worker.
    pub fn waveform_snapshot(
        &self,
        start_seconds: f64,
        duration_seconds: f64,
    ) -> Result<crate::pcm_tap::ObservedWaveform, String> {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let generation = state.generation;
        drop(state);
        self.inner.output.tap().waveform_snapshot_range(
            generation,
            start_seconds,
            duration_seconds,
            1200,
        )
    }

    fn snapshot_without_queue(&self) -> Value {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot(self.native_enabled(), false)
    }
    /// A bounded queued command has at most 500 tracks; capture all occurrence IDs
    /// atomically rather than accidentally truncating its receipt to a 200-row page.
    pub(crate) fn queued_playback_acceptance(&self) -> Value {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        json!({"queuedOperationId":state.queued_operation_id,"queueRevision":state.queue_revision,"queueEntryIds":state.entries,"generation":state.generation,"trackIds":state.queue.iter().map(|t|t.id.as_str()).collect::<Vec<_>>()})
    }

    /// Keep a CENC-capable resolved source in Rust and return only a short-lived
    /// opaque handle to the UI. Dropping expired/evicted source records releases
    /// their relay leases and key material.
    pub(crate) fn register_native_resolved_source(
        self: &Arc<Self>,
        source_generation: u64,
        track: &Track,
        explicit_source: bool,
        resolved: crate::playback_sources::ResolvedSource,
    ) -> Result<String, String> {
        let candidate = resolved
            .candidate
            .as_ref()
            .ok_or("Native lossless source has no verified candidate")?;
        let matching_level = self
            .inner
            .store
            .settings()
            .map_err(|_| "Playback matching policy unavailable")?
            .matching_level;
        if source_generation == 0
            || track.id.trim().is_empty()
            || track.id.len() > 200
            || track.id.chars().any(char::is_control)
            || resolved_match_level(&resolved) != Some(matching_level)
            || candidate.provider != "monochrome"
            || !resolved
                .source
                .cenc
                .as_ref()
                .is_some_and(|cenc| cenc.has_registered_lease_for_input(&resolved.source.input))
            || resolved.station.is_some()
        {
            return Err("Resolved source is not eligible for native lossless handoff".into());
        }
        let now = Instant::now();
        let mut pending = self
            .inner
            .pending_native_sources
            .lock()
            .map_err(|_| "Native playback source registry unavailable")?;
        pending.retain(|_, source| source.expires_at > now);
        if pending.len() >= MAX_PENDING_NATIVE_SOURCES {
            return Err("Native playback source handoff capacity is full".into());
        }
        let handle = uuid::Uuid::new_v4().to_string();
        let expires_at = now + PENDING_NATIVE_SOURCE_TTL;
        pending.insert(
            handle.clone(),
            PendingNativeSource {
                source_generation,
                request_track: track.clone(),
                explicit_source,
                matching_level,
                expires_at,
                resolved,
            },
        );
        let weak = Arc::downgrade(self);
        let expiry_handle = handle.clone();
        let expiry_generation = source_generation;
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(PENDING_NATIVE_SOURCE_TTL).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            if let Ok(mut pending) = service.inner.pending_native_sources.lock() {
                if pending.get(&expiry_handle).is_some_and(|source| {
                    source.expires_at == expires_at && source.source_generation == expiry_generation
                }) {
                    pending.remove(&expiry_handle);
                }
            };
        });
        Ok(handle)
    }

    /// Transfer the exact currently owned WebView occurrence to the native
    /// decoder using the source held under `source_handle`. The handle is
    /// consumed once and the queue/session/entry envelope is validated before
    /// any native ownership change.
    pub async fn handoff_webview_resolved_source(
        &self,
        handoff: ResolvedSourceHandoff,
        source_is_current: impl FnOnce(&ResolvedSourceHandoff, &str) -> bool,
    ) -> Result<Value, String> {
        let _command = self.inner.commands.lock().await;
        if self.native_enabled() {
            return Err("Playback ownership has already transferred".into());
        }
        let track = validate_resolved_handoff_occurrence(&handoff)?;
        self.validate_tracks(&handoff.tracks)?;
        if !(0.0..=1.0).contains(&handoff.volume)
            || !matches!(handoff.repeat.as_str(), "off" | "all" | "one")
            || !handoff.target_position.is_finite()
            || handoff.target_position < 0.0
            || track
                .duration_seconds
                .is_some_and(|duration| handoff.target_position >= duration)
        {
            return Err("Native source handoff position or playback settings are invalid".into());
        }
        let now = Instant::now();
        let (owner_matches, eligible, matching_level) = {
            let mut pending = self
                .inner
                .pending_native_sources
                .lock()
                .map_err(|_| "Native playback source registry unavailable")?;
            pending.retain(|_, source| source.expires_at > now);
            let source = pending
                .get(&handoff.source_handle)
                .ok_or("Native playback source handle expired or was already used")?;
            let eligible = source.resolved.source.cenc.as_ref().is_some_and(|cenc| {
                cenc.has_registered_lease_for_input(&source.resolved.source.input)
            }) && source.resolved.station.is_none()
                && source
                    .resolved
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.provider == "monochrome")
                && source
                    .resolved
                    .duration
                    .or(track.duration_seconds)
                    .is_some_and(|duration| {
                        duration.is_finite() && duration > 0.0 && handoff.target_position < duration
                    });
            let owner_matches = pending_source_owner_matches(source, &handoff, track, now)
                && pending_source_policy_matches(source, source.matching_level);
            (owner_matches, eligible, source.matching_level)
        };
        let current_matching_level = self
            .inner
            .store
            .settings()
            .map_err(|_| "Playback matching policy unavailable")?
            .matching_level;
        if !eligible
            || !owner_matches
            || current_matching_level != matching_level
            || !source_is_current(&handoff, &track.id)
        {
            return Err("Native source handoff no longer owns this playback occurrence".into());
        }
        let mut pending = self
            .inner
            .pending_native_sources
            .lock()
            .map_err(|_| "Native playback source registry unavailable")?;
        let source =
            take_pending_native_source(&mut pending, &handoff.source_handle, Instant::now())
                .ok_or("Native playback source handle expired or was already used")?;
        let candidate = source.resolved.candidate.clone();
        let explicit_source = source.explicit_source;
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.queue = handoff.tracks;
            state.entries = handoff.entry_ids;
            state.index = handoff.index;
            state.queue_revision = state.queue_revision.wrapping_add(1);
            state.published_queue_revision = None;
            state.session = handoff.session_id;
            state.recorded = handoff.recorded;
            state.volume = handoff.volume;
            state.repeat = handoff.repeat;
            state.shuffle = handoff.shuffle;
            state.candidate = candidate.clone();
            state.explicit_source = explicit_source;
            state.handoff_id = Some(handoff.handoff_id);
            state.pause_when_ready = handoff.paused;
            state.request_started = Instant::now();
            state.first_audio_ms = None;
            state.source_ready_ms = None;
            state.operation_source_ready_ms = None;
            state.operation_first_audio_ms = None;
            state.seek_serial = handoff.seek_serial;
            state.seek_requested = Some(Instant::now());
            state.seek_first_audio_ms = None;
            state.recovery_deadline = None;
            state.recovery = 0;
            state.queued_operation_id = None;
            state.queued_intent_epoch = state.queued_intent_epoch.wrapping_add(1);
            state.revision = state.revision.wrapping_add(1);
        }
        drop(pending);
        self.inner.native.store(true, Ordering::Release);
        self.start_with_resolved_source(source.resolved, handoff.target_position, handoff.paused);
        Ok(self.snapshot())
    }
    pub fn snapshot_page(&self, offset: usize, limit: usize) -> Value {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot_page(self.native_enabled(), offset, limit)
    }
    pub fn subscribe(&self) -> watch::Receiver<Value> {
        self.inner.updates.subscribe()
    }

    /// Transfer a verified, currently playing WebView YouTube occurrence to the
    /// native resolver without changing the queue/session identity.
    pub async fn handoff_webview(
        &self,
        handoff: WebViewPlaybackHandoff,
        source_is_current: impl FnOnce(u64, &str) -> bool,
    ) -> Result<Value, String> {
        let _command = self.inner.commands.lock().await;
        if self.native_enabled() {
            return Err("Playback ownership has already transferred".into());
        }
        let handoff_id = handoff.handoff_id.trim();
        if handoff_id.is_empty()
            || handoff_id.len() > 128
            || handoff_id.chars().any(char::is_control)
        {
            return Err("Invalid playback handoff ID".into());
        }
        if handoff.session_id.trim().is_empty()
            || handoff.session_id.len() > 128
            || handoff.session_id.chars().any(char::is_control)
            || handoff.entry_id.trim().is_empty()
            || handoff.entry_id.len() > 200
            || handoff.entry_id.chars().any(char::is_control)
        {
            return Err("Invalid playback session or queue occurrence".into());
        }
        if handoff.index >= handoff.tracks.len()
            || handoff.entry_ids.len() != handoff.tracks.len()
            || handoff.entry_ids.get(handoff.index) != Some(&handoff.entry_id)
            || handoff.entry_ids.iter().any(|id| {
                id.trim().is_empty() || id.len() > 200 || id.chars().any(char::is_control)
            })
            || handoff.entry_ids.iter().collect::<HashSet<_>>().len() != handoff.entry_ids.len()
        {
            return Err("Playback queue occurrence identity is inconsistent".into());
        }
        self.validate_tracks(&handoff.tracks)?;
        let track = &handoff.tracks[handoff.index];
        if exact_handoff_youtube_id(track, &handoff.candidate, handoff.target_position).is_none()
            || !(0.0..=1.0).contains(&handoff.volume)
            || !matches!(handoff.repeat.as_str(), "off" | "all" | "one")
            || !source_is_current(handoff.source_generation, &track.id)
        {
            return Err(
                "Playback handoff source does not match the current full YouTube recording".into(),
            );
        }
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.queue = handoff.tracks;
            state.entries = handoff.entry_ids;
            state.index = handoff.index;
            state.queue_revision = state.queue_revision.wrapping_add(1);
            state.published_queue_revision = None;
            state.session = handoff.session_id;
            state.recorded = handoff.recorded;
            state.volume = handoff.volume;
            state.repeat = handoff.repeat;
            state.shuffle = handoff.shuffle;
            state.candidate = Some(handoff.candidate.clone());
            state.handoff_id = Some(handoff.handoff_id);
            state.pause_when_ready = handoff.paused;
            state.request_started = Instant::now();
            state.first_audio_ms = None;
            state.source_ready_ms = None;
            state.operation_source_ready_ms = None;
            state.operation_first_audio_ms = None;
            state.seek_serial = handoff.seek_serial;
            state.seek_requested = Some(Instant::now());
            state.seek_first_audio_ms = None;
            state.recovery_deadline = None;
            state.recovery = 0;
            state.queued_operation_id = None;
            state.queued_intent_epoch = state.queued_intent_epoch.wrapping_add(1);
            state.revision = state.revision.wrapping_add(1);
        }
        self.inner.native.store(true, Ordering::Release);
        self.start_with_pause(
            Some(handoff.candidate),
            handoff.target_position,
            true,
            handoff.paused,
        );
        Ok(self.snapshot())
    }
    fn publish(&self) {
        let (full, lite, queue_revision, include_queue) = {
            let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let include_queue = s.published_queue_revision != Some(s.queue_revision);
            let revision = s.queue_revision;
            (
                s.snapshot(self.native_enabled(), include_queue),
                s.snapshot(self.native_enabled(), false),
                revision,
                include_queue,
            )
        };
        self.inner.updates.send_replace(lite);
        if self.native_enabled() {
            if self.inner.app.emit("native-playback-state", full).is_ok() && include_queue {
                let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if s.queue_revision == queue_revision {
                    s.published_queue_revision = Some(queue_revision);
                }
            }
            self.persist();
        }
    }
    fn change(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
        s.revision += 1;
        drop(s);
        self.publish();
    }
    fn cancel(&self) {
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = s.cancel.take() {
            let _ = tx.send(true);
        }
        s.generation_counter += 1;
        s.generation = s.generation_counter;
        s.output_ready = false;
        s.opus_decoder_capability = None;
        s.pending_seek_generation = None;
        s.steady_http_stream = false;
        s.seek_watchdog_epoch = s.seek_watchdog_epoch.wrapping_add(1);
        if let Some(cancel) = s.preload_cancel.take() {
            let _ = cancel.send(true);
        }
        s.preload = None;
        self.inner.audible.send_replace(false);
        let _ = self.inner.output.send(OutputCommand::Stop);
    }
    fn cancel_timed_out_generation(&self, generation: u64) -> bool {
        self.expire_generation(generation, false)
    }
    fn expire_generation(&self, generation: u64, startup_only: bool) -> bool {
        self.expire_generation_checked(generation, startup_only, None)
    }
    fn expire_generation_checked(
        &self,
        generation: u64,
        startup_only: bool,
        expected_seek_epoch: Option<u64>,
    ) -> bool {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation
            || (startup_only && !matches!(state.status.as_str(), "resolving" | "buffering"))
            || expected_seek_epoch.is_some_and(|epoch| state.seek_watchdog_epoch != epoch)
        {
            return false;
        }
        let decoder_diagnostic = state.native_decoder_diagnostic.take();
        if let Some(cancel) = state.cancel.take() {
            let _ = cancel.send(true);
        }
        if let Some(cancel) = state.preload_cancel.take() {
            let _ = cancel.send(true);
        }
        state.preload = None;
        state.output_ready = false;
        state.opus_decoder_capability = None;
        state.pending_seek_generation = None;
        state.steady_http_stream = false;
        state.seek_watchdog_epoch = state.seek_watchdog_epoch.wrapping_add(1);
        state.generation_counter += 1;
        state.generation = state.generation_counter;
        state.status = "failed".into();
        state.message = if expected_seek_epoch.is_some() {
            format!(
                "The seek did not become ready before its {} second deadline. Try another position or restart playback.",
                state.startup_budget.as_secs()
            )
        } else if startup_only {
            format!(
                "Playback startup exceeded {} seconds before audio became ready. Check provider availability or choose another source.",
                state.startup_budget.as_secs()
            )
        } else {
            "Playback timed out".into()
        };
        state.revision += 1;
        self.inner.audible.send_replace(false);
        // Keep the stop under the generation lock so a newer request cannot be stopped.
        let _ = self.inner.output.send(OutputCommand::Stop);
        drop(state);
        if let Some(context) = decoder_diagnostic.as_ref() {
            record_native_decoder_diagnostic(
                context,
                "decoder_end",
                Some("deadline"),
                Some("timeout"),
            );
        }
        self.publish();
        true
    }
    fn watch_ready_seek(&self, generation: u64, epoch: u64, deadline: Instant) {
        let service = self.clone();
        let updates = self.subscribe();
        tauri::async_runtime::spawn(async move {
            let state_service = service.clone();
            if wait_for_ready_seek_audio(updates, generation, epoch, deadline, move || {
                let state = state_service
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (
                    state.generation,
                    state.status.clone(),
                    state.seek_watchdog_epoch,
                )
            })
            .await
            {
                service.expire_generation_checked(generation, true, Some(epoch));
            }
        });
    }
    fn current(&self, g: u64) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation
            == g
    }
    pub async fn execute(&self, command: Value) -> Result<Value, String> {
        self.execute_checked(command, None, None, None).await
    }
    pub(crate) fn queued_playback_epoch(&self) -> u64 {
        self.inner.state.lock().unwrap().queued_intent_epoch
    }
    pub(crate) async fn execute_guarded_committed(
        &self,
        command: Value,
        expected_epoch: u64,
        permit: impl FnOnce() -> bool + Send + 'static,
        commit: impl FnOnce(Value) -> Result<(), String> + Send + 'static,
    ) -> Result<Value, String> {
        self.execute_checked(
            command,
            Some(Box::new(permit)),
            Some(expected_epoch),
            Some(Box::new(commit)),
        )
        .await
    }
    async fn execute_checked(
        &self,
        command: Value,
        permit: Option<Box<dyn FnOnce() -> bool + Send>>,
        expected_epoch: Option<u64>,
        commit: Option<QueuedPlaybackCommit>,
    ) -> Result<Value, String> {
        let operation_received_at = Instant::now();
        let operation_received_wall_ms = system_time_ms();
        let _command = self.inner.commands.lock().await;
        let budget = self.inner.store.settings()?.playback_startup_timeout();
        self.inner.state.lock().unwrap().startup_budget = budget;
        if let Some(expected) = expected_epoch {
            let current = self.inner.state.lock().unwrap().queued_intent_epoch;
            if !crate::queued_playback::native_start_is_current(
                expected,
                current,
                permit.is_some_and(|permit| permit()),
            ) {
                return Err("Queued playback operation was cancelled or superseded".into());
            }
        }
        if !self.native_enabled() {
            return Err("Native playback is not enabled".into());
        }
        let action = command["action"]
            .as_str()
            .ok_or("Playback action is missing")?;
        // Compare while holding commands lock, so a stale cancel cannot stop a
        // newer command between its ownership check and stop mutation.
        if let Some(expected) = command
            .get("expectedQueuedOperationId")
            .and_then(Value::as_str)
        {
            if action != "stop" {
                return Err("Expected queued owner is supported only for stop".into());
            }
            let owned = crate::queued_playback::native_stop_is_owned(
                expected,
                self.inner
                    .state
                    .lock()
                    .unwrap()
                    .queued_operation_id
                    .as_deref(),
            );
            if !owned {
                return Ok(self.snapshot_without_queue());
            }
        }
        if let Some(expected) = command.get("expectedHandoffId").and_then(Value::as_str) {
            if action != "stop" {
                return Err("Expected handoff owner is supported only for stop".into());
            }
            if !handoff_stop_is_owned(
                expected,
                self.inner.state.lock().unwrap().handoff_id.as_deref(),
            ) {
                return Ok(self.snapshot_without_queue());
            }
        }
        self.inner.state.lock().unwrap().handoff_id = None;
        let queued_operation_id = parse_operation_id(command.get("queuedOperationId"))?;
        let client_operation_id = parse_operation_id(command.get("operationId"))?;
        let client_started_at_ms = parse_client_started_at_ms(command.get("clientStartedAtMs"))?;
        let operation_id = ensure_command_operation_id(client_operation_id, action);
        let command_bytes = (operation_id.is_some() || client_started_at_ms.is_some())
            .then(|| command.to_string().len());
        let click_to_ipc_ms = client_started_at_ms
            .and_then(|started| operation_received_wall_ms.checked_sub(started));
        if matches!(
            action,
            "queue"
                | "remove"
                | "remove_many"
                | "reorder"
                | "sort_upcoming"
                | "shuffle"
                | "repeat"
                | "pause"
                | "seek"
        ) {
            self.clear_preload();
        }
        match action {
            "play" => {
                let tracks: Vec<Track> = serde_json::from_value(command["tracks"].clone())
                    .map_err(|_| "Invalid playback tracks")?;
                self.validate_tracks(&tracks)?;
                let album_context = command
                    .get("loudnessAlbumResource")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        serde_json::from_value::<crate::loudness::LoudnessResourceIdentity>(
                            value.clone(),
                        )
                        .map_err(|_| "Invalid album loudness queue identity")
                    })
                    .transpose()?;
                if let Some(resource) = album_context.as_ref() {
                    validate_loudness_album_queue_binding(resource, &tracks)?;
                }
                let index = command["index"].as_u64().unwrap_or(0) as usize;
                if index >= tracks.len() {
                    return Err("Queue position is out of range".into());
                }
                let candidate = command
                    .get("candidate")
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        serde_json::from_value::<Candidate>(v.clone())
                            .map_err(|_| "Invalid audio candidate")
                    })
                    .transpose()?;
                let position = positive(command.get("position"), 0.)?;
                self.cancel();
                self.change(|s| {
                    begin_operation(
                        s,
                        operation_id.as_deref(),
                        "startup",
                        operation_received_at,
                        client_started_at_ms,
                        click_to_ipc_ms,
                        command_bytes,
                    );
                    s.queued_intent_epoch = s.queued_intent_epoch.wrapping_add(1);
                    s.queued_operation_id = queued_operation_id.clone();
                    replace_play_queue(s, tracks, index);
                    s.loudness_queue_contexts.clear();
                    let queue_count = s.queue.len();
                    bind_loudness_context_to_queue_range(s, 0, queue_count, album_context.as_ref());
                    clear_playback_failures(s);
                    s.position = position;
                    s.recovery = 0;
                    s.failed_occurrence_ids.clear();
                });
                self.start(candidate, position, false);
            }
            "queue" => {
                let tracks: Vec<Track> = serde_json::from_value(command["tracks"].clone())
                    .map_err(|_| "Invalid queue tracks")?;
                self.validate_tracks(&tracks)?;
                let album_context = command
                    .get("loudnessAlbumResource")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        serde_json::from_value::<crate::loudness::LoudnessResourceIdentity>(
                            value.clone(),
                        )
                        .map_err(|_| "Invalid album loudness queue identity")
                    })
                    .transpose()?;
                if let Some(resource) = album_context.as_ref() {
                    validate_loudness_album_queue_binding(resource, &tracks)?;
                }
                let track_count = tracks.len();
                let mode = command["mode"].as_str().unwrap_or("append");
                if !["append", "replace", "next"].contains(&mode) {
                    return Err("Unknown queue mode".into());
                }
                if mode != "replace"
                    && self.inner.state.lock().unwrap().queue.len() + tracks.len() > 10000
                {
                    return Err("Queue is limited to 10000 tracks".into());
                }
                let preserve = command["preserveCurrent"].as_bool().unwrap_or(false) && {
                    let s = self.inner.state.lock().unwrap();
                    s.track()
                        .is_some_and(|t| tracks.first().is_some_and(|next| next.id == t.id))
                };
                if mode == "replace" && !preserve {
                    self.cancel();
                }
                self.change(|s| {
                    if mode == "replace" {
                        clear_playback_failures(s);
                        s.queue.clear();
                        s.entries.clear();
                        s.index = 0;
                        if !preserve {
                            s.status = "idle".into();
                            s.position = 0.;
                            s.duration = 0.;
                            s.station = None;
                            s.radio_now = Value::Null;
                            s.radio_history.clear();
                            s.candidate = None;
                            s.quality = json!({});
                            s.session.clear();
                            s.recorded = false;
                            s.offset = 0.;
                            s.end = None;
                            s.message = "Queue ready".into();
                        }
                    }
                    let at = if mode == "next" && !s.queue.is_empty() {
                        s.index + 1
                    } else {
                        s.queue.len()
                    };
                    if mode == "replace" {
                        s.loudness_queue_contexts.clear();
                    }
                    for (offset, track) in tracks.into_iter().enumerate() {
                        s.queue.insert(at + offset, track);
                        s.entries
                            .insert(at + offset, uuid::Uuid::new_v4().to_string());
                    }
                    bind_loudness_context_to_queue_range(
                        s,
                        at,
                        track_count,
                        album_context.as_ref(),
                    );
                    mark_queue_changed(s);
                });
            }
            "select" => {
                let index = command["index"].as_u64().ok_or("Queue position missing")? as usize;
                let album_context = command
                    .get("loudnessAlbumResource")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        serde_json::from_value::<crate::loudness::LoudnessResourceIdentity>(
                            value.clone(),
                        )
                        .map_err(|_| "Invalid album loudness queue identity")
                    })
                    .transpose()?;
                if index >= self.inner.state.lock().unwrap().queue.len() {
                    return Err("Queue position is out of range".into());
                }
                if let Some(resource) = album_context.as_ref() {
                    bind_loudness_context_to_matching_queue(
                        &mut self.inner.state.lock().unwrap(),
                        resource,
                    )?;
                }
                self.cancel();
                self.change(|s| {
                    begin_operation(
                        s,
                        operation_id.as_deref(),
                        "transition",
                        operation_received_at,
                        client_started_at_ms,
                        click_to_ipc_ms,
                        command_bytes,
                    );
                    s.index = index;
                    s.recovery = 0;
                    s.failed_occurrence_ids.clear();
                    clear_current_source_failure(s);
                });
                self.start(None, 0., false);
            }
            "pause" => {
                self.clear_preload();
                let resolving = {
                    let s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    ["resolving", "buffering"].contains(&s.status.as_str())
                };
                if resolving {
                    self.cancel();
                } else {
                    self.inner.output.send(OutputCommand::Pause)?;
                    self.inner.audible.send_replace(false);
                }
                self.change(|s| {
                    s.seek_watchdog_epoch = s.seek_watchdog_epoch.wrapping_add(1);
                    s.recovered_audio_since = None;
                    s.healthy_output_since = None;
                    s.status = "paused".into();
                    s.message = "Paused".into();
                });
            }
            "resume" => {
                let (
                    status,
                    position,
                    candidate,
                    ready,
                    continuing,
                    pending_seek,
                    local_storage_failed,
                ) = {
                    let s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    let (position, ready, continuing) = s.resume_target();
                    (
                        s.status.clone(),
                        position,
                        s.candidate.clone(),
                        ready,
                        continuing,
                        s.pending_seek_generation.filter(|g| *g == s.generation),
                        s.last_local_storage_failure.is_some(),
                    )
                };
                if status == "playing" {
                    return Ok(self.snapshot_without_queue());
                }
                if ready {
                    self.inner.output.send(OutputCommand::Resume)?;
                    let mut seek_watchdog = None;
                    self.change(|s| {
                        self.inner.audible.send_replace(s.volume > 0.);
                        if has_native_stall_history(s) {
                            let now = Instant::now();
                            s.recovery_deadline = Some(now + s.startup_budget);
                            s.recovered_audio_since = Some((now, s.position));
                            s.healthy_output_since = Some((now, s.position, s.seek_serial));
                        }
                        if pending_seek.is_some() {
                            arm_resumed_seek_metrics(s, Instant::now());
                            s.seek_watchdog_epoch = s.seek_watchdog_epoch.wrapping_add(1);
                            seek_watchdog =
                                pending_seek.map(|generation| (generation, s.seek_watchdog_epoch));
                            s.status = "buffering".into();
                            s.message = "Buffering audio…".into();
                        } else {
                            s.status = "playing".into();
                            s.message = "Playing audio".into();
                        }
                    });
                    if let Some((generation, epoch)) = seek_watchdog {
                        self.watch_ready_seek(
                            generation,
                            epoch,
                            Instant::now() + self.inner.state.lock().unwrap().startup_budget,
                        );
                    }
                    self.record_started();
                } else {
                    let failed_resume = status == "failed";
                    if failed_resume {
                        if !local_storage_failed {
                            self.invalidate_candidate_preview_audio(candidate.as_ref());
                        }
                        self.change(|s| {
                            reset_failed_resume_diagnostics(s);
                        });
                    } else if !continuing {
                        self.change(|s| {
                            s.recovery = 0;
                            s.recovery_deadline = None;
                            s.recovered_audio_since = None;
                        });
                    }
                    if continuing {
                        self.change(|s| {
                            if has_native_stall_history(s) {
                                s.recovery_deadline = Some(Instant::now() + s.startup_budget);
                                s.recovered_audio_since = None;
                                s.healthy_output_since = None;
                            }
                        });
                    }
                    if pending_seek.is_some() {
                        self.change(|s| arm_resumed_seek_metrics(s, Instant::now()));
                    }
                    self.start(candidate, position, continuing);
                }
            }
            "stop" => {
                self.cancel();
                self.change(|s| {
                    s.status = "idle".into();
                    s.natural_queue_end = false;
                    s.position = 0.;
                    s.message = "Stopped".into();
                });
            }
            "seek" => {
                let position = positive(command.get("position"), 0.)?;
                let (duration, offset, station, ready, status, candidate, pause_when_ready) = {
                    let s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        s.duration,
                        s.offset,
                        s.station.is_some(),
                        s.output_ready,
                        s.status.clone(),
                        s.candidate.clone(),
                        s.pause_when_ready,
                    )
                };
                if station {
                    return Err("Live radio cannot seek".into());
                }
                if duration > 0. && position > duration {
                    return Err("Position exceeds track duration".into());
                }
                let mut seek_watchdog = None;
                let deferred_seek = deferred_seek_status(&status, ready)
                    && self.inner.state.lock().unwrap().track().is_some();
                if ready && !deferred_seek {
                    let mut s = self.inner.state.lock().unwrap();
                    record_seek_context(
                        &mut s,
                        operation_id.clone(),
                        operation_received_at,
                        client_started_at_ms,
                        click_to_ipc_ms,
                        command_bytes,
                    );
                    s.generation_counter += 1;
                    s.generation = s.generation_counter;
                    s.seek_watchdog_epoch = s.seek_watchdog_epoch.wrapping_add(1);
                    s.pending_seek_generation = Some(s.generation);
                    s.seek_requested = (status != "paused").then(Instant::now);
                    s.seek_first_audio_ms = None;
                    s.seek_operation_first_audio_ms = None;
                    s.recovered_audio_since = None;
                    s.healthy_output_since = None;
                    self.inner.output.send(OutputCommand::Seek {
                        generation: s.generation,
                        position: position + offset,
                    })?;
                    seek_watchdog = (status != "paused")
                        .then_some(operation_received_at + s.startup_budget)
                        .map(|deadline| (s.generation, s.seek_watchdog_epoch, deadline));
                    if seek_watchdog.is_some() {
                        s.status = "buffering".into();
                        s.message = "Buffering audio…".into();
                    }
                } else if matches!(status.as_str(), "resolving" | "buffering" | "playing") {
                    self.change(|s| {
                        record_seek_context(
                            s,
                            operation_id.clone(),
                            operation_received_at,
                            client_started_at_ms,
                            click_to_ipc_ms,
                            command_bytes,
                        );
                        s.seek_requested = (status != "paused").then(Instant::now);
                        s.seek_first_audio_ms = None;
                        s.seek_operation_first_audio_ms = None;
                        s.recovered_audio_since = None;
                        s.healthy_output_since = None;
                    });
                    self.start_with_pause(candidate, position, true, pause_when_ready);
                } else if deferred_seek {
                    // Retained stopped/ended output must not restart from a seek. Invalidate old callbacks now,
                    // retain this request while paused, and resolve only on Resume.
                    self.cancel();
                    self.change(|s| {
                        record_seek_context(
                            s,
                            operation_id.clone(),
                            operation_received_at,
                            client_started_at_ms,
                            click_to_ipc_ms,
                            command_bytes,
                        );
                        s.recovered_audio_since = None;
                        s.healthy_output_since = None;
                        defer_paused_seek(s);
                    });
                }
                self.change(|s| {
                    s.position = position;
                    s.seek_serial += 1;
                });
                if let Some((generation, epoch, deadline)) = seek_watchdog {
                    self.watch_ready_seek(generation, epoch, deadline);
                }
            }
            "volume" => {
                let volume = positive(command.get("volume"), 0.8)?;
                if volume > 1. {
                    return Err("Volume must be from zero to one".into());
                }
                self.inner
                    .output
                    .send(OutputCommand::Volume(volume as f32))?;
                self.change(|s| {
                    s.volume = volume;
                    self.inner
                        .audible
                        .send_replace(s.status == "playing" && volume > 0.);
                });
            }
            "repeat" => {
                let mode = command["mode"].as_str().ok_or("Repeat mode missing")?;
                if !["off", "one", "all"].contains(&mode) {
                    return Err("Invalid repeat mode".into());
                }
                self.change(|s| s.repeat = mode.into());
            }
            "shuffle" => {
                let enabled = command["enabled"]
                    .as_bool()
                    .ok_or("Shuffle setting missing")?;
                self.change(|s| {
                    if enabled && !s.shuffle {
                        shuffle_upcoming(s);
                        mark_queue_changed(s);
                    }
                    s.shuffle = enabled;
                });
            }
            "sort_upcoming" => {
                let field = command["field"].as_str().ok_or("Sort field is missing")?;
                let direction = command["direction"].as_str().unwrap_or("asc");
                if !["asc", "desc"].contains(&direction) {
                    return Err("Sort direction must be asc or desc".into());
                }
                if ![
                    "title",
                    "artist",
                    "album",
                    "duration",
                    "releaseDate",
                    "dateAdded",
                    "sourceOrder",
                    "bpm",
                    "key",
                    "plays",
                    "views",
                ]
                .contains(&field)
                {
                    return Err("Unsupported playback sort field".into());
                }
                let collator = crate::track_sort_text::collator()?;
                self.change(|state| sort_upcoming(state, field, direction == "desc", collator));
            }
            "next" | "previous" => {
                self.change(|s| {
                    begin_operation(
                        s,
                        operation_id.as_deref(),
                        "transition",
                        operation_received_at,
                        client_started_at_ms,
                        click_to_ipc_ms,
                        command_bytes,
                    )
                });
                self.advance(if action == "next" { 1 } else { -1 }, false);
            }
            "remove" => {
                let at = command["index"].as_u64().ok_or("Queue position missing")? as usize;
                let current = {
                    let s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    if at >= s.queue.len() {
                        return Err("Queue position is out of range".into());
                    }
                    at == s.index
                };
                if current {
                    self.cancel();
                }
                self.change(|s| {
                    s.queue.remove(at);
                    s.entries.remove(at);
                    mark_queue_changed(s);
                    if at < s.index {
                        s.index -= 1;
                    }
                    s.index = s.index.min(s.queue.len().saturating_sub(1));
                    if current {
                        s.status = "idle".into();
                    }
                });
                if current && !self.inner.state.lock().unwrap().queue.is_empty() {
                    self.start(None, 0., false);
                }
            }
            "remove_many" => {
                let indices: Vec<usize> = serde_json::from_value(command["indices"].clone())
                    .map_err(|_| "Queue positions missing or invalid")?;
                let expected = command.get("expectedQueueRevision").and_then(Value::as_u64);
                {
                    let mut s = self.inner.state.lock().unwrap();
                    remove_queue_occurrences(&mut s, &indices, expected)?;
                    s.revision += 1;
                }
                self.publish();
            }
            "reorder" => {
                let from = command["from"].as_u64().ok_or("Queue position missing")? as usize;
                let to = command["to"].as_u64().ok_or("Queue position missing")? as usize;
                let expected = command
                    .get("expectedQueueRevision")
                    .map(|v| v.as_u64().ok_or("Invalid expected queue revision"))
                    .transpose()?;
                {
                    let mut s = self.inner.state.lock().unwrap();
                    reorder_queue_state(&mut s, from, to, expected)?;
                    s.revision += 1;
                }
                self.publish();
            }
            "restore" => {
                let empty = self.inner.state.lock().unwrap().queue.is_empty();
                if empty {
                    let mut s = self.inner.state.lock().unwrap();
                    restore_state(&mut s, &command)?;
                    drop(s);
                    self.publish();
                }
            }
            _ => return Err("Unsupported playback action".into()),
        }
        if matches!(
            action,
            "queue"
                | "remove"
                | "remove_many"
                | "reorder"
                | "sort_upcoming"
                | "shuffle"
                | "repeat"
                | "resume"
        ) {
            self.prepare_next();
        }
        if action != "play" && !matches!(action, "seek" | "volume") {
            self.change(|s| finish_external_queued_owner(s, action));
        }
        // Commit acceptance before releasing the command mutex. External play,
        // stop, and queue edits cannot replace this owner between its exact
        // occurrence snapshot and the registry commit. The callback follows the
        // same command -> registry lock order as the preparation permit above.
        commit_queued_playback(&_command, || self.queued_playback_acceptance(), commit)?;
        Ok(self.snapshot_without_queue())
    }
    fn validate_tracks(&self, tracks: &[Track]) -> Result<(), String> {
        if tracks.is_empty() || tracks.len() > 10000 {
            return Err("A playback queue must contain 1–10000 tracks".into());
        }
        for track in tracks {
            if track.id.trim().is_empty() || track.title.trim().is_empty() {
                return Err("Track identity or title is missing".into());
            }
            if let Some(path) = &track.local_path {
                if self.inner.store.get_asset_by_path(path)?.is_none() {
                    return Err("Local audio must be registered in the library".into());
                }
            }
        }
        Ok(())
    }
    fn start(&self, candidate: Option<Candidate>, position: f64, resume: bool) {
        self.start_with_pause(candidate, position, resume, false);
    }
    fn start_with_pause(
        &self,
        candidate: Option<Candidate>,
        position: f64,
        resume: bool,
        pause_when_ready: bool,
    ) {
        self.start_with_pause_and_resolved_source(
            candidate,
            position,
            resume,
            pause_when_ready,
            None,
        );
    }
    fn start_with_resolved_source(
        &self,
        resolved: crate::playback_sources::ResolvedSource,
        position: f64,
        pause_when_ready: bool,
    ) {
        let candidate = resolved.candidate.clone();
        self.start_with_pause_and_resolved_source(
            candidate,
            position,
            true,
            pause_when_ready,
            Some(resolved),
        );
    }
    fn start_with_pause_and_resolved_source(
        &self,
        candidate: Option<Candidate>,
        position: f64,
        resume: bool,
        pause_when_ready: bool,
        resolved_source: Option<crate::playback_sources::ResolvedSource>,
    ) {
        let startup_budget = self
            .inner
            .store
            .settings()
            .map(|settings| settings.playback_startup_timeout())
            .unwrap_or(crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE);
        let position = if self
            .inner
            .state
            .lock()
            .unwrap()
            .track()
            .is_some_and(|t| t.provider == "radio")
        {
            0.
        } else {
            position
        };
        self.cancel();
        let (tx, rx) = watch::channel(false);
        let (g, track, session, recovery, startup_deadline, album_context) = {
            let mut s = self.inner.state.lock().unwrap();
            let Some(track) = s.track().cloned() else {
                return;
            };
            s.startup_budget = startup_budget;
            s.pause_when_ready = pause_when_ready;
            s.cancel = Some(tx);
            // A superseded decoder attempt must never report against the next
            // generation's operation or first-audio boundary.
            s.native_decoder_diagnostic = None;
            s.last_buffering = None;
            s.status = "resolving".into();
            s.natural_queue_end = false;
            s.message = "Finding audio…".into();
            s.position = position;
            s.offset = track.playback_range.as_ref().map_or(0., |r| r.start_time);
            s.end = track.playback_range.as_ref().map(|r| r.end_time);
            s.duration_verified = track.playback_range.is_some();
            s.duration = track
                .playback_range
                .as_ref()
                .map_or(track.duration_seconds.unwrap_or(0.), |r| {
                    r.end_time - r.start_time
                });
            if !resume {
                s.explicit_source = candidate.is_some();
                clear_native_stall_history(&mut s);
                s.healthy_output_since = None;
                s.replace_source_on_recovery = false;
            }
            s.candidate = candidate.clone();
            s.steady_http_stream = false;
            if !resume {
                s.quality = json!({});
            }
            s.station = None;
            s.radio_now = Value::Null;
            s.radio_history.clear();
            if !resume || s.session.is_empty() {
                s.recovery_deadline = None;
                s.recovered_audio_since = None;
                s.request_started = Instant::now();
                s.first_audio_ms = None;
                s.source_ready_ms = None;
                s.operation_source_ready_ms = None;
                s.operation_first_audio_ms = None;
                reset_seek_metrics_for_start(&mut s, resume);
                s.buffering_count = 0;
                s.session = uuid::Uuid::new_v4().to_string();
                s.recorded = false;
            }
            s.revision += 1;
            (
                s.generation,
                track,
                s.session.clone(),
                s.recovery,
                restart_deadline(&s),
                s.entries
                    .get(s.index)
                    .and_then(|entry_id| s.loudness_queue_contexts.get(entry_id))
                    .cloned(),
            )
        };
        if let Ok(mut context) = self.inner.loudness_album_context.lock() {
            *context = album_context;
        }
        self.inner.output.tap().supersede_older_subscribers(g);
        let track = self.track_with_registered_chapter_asset(&track);
        self.publish();
        // This deadline includes reader/decoder readiness after URL resolution.
        // A successful first-audio event ends the watchdog permanently, so later
        // steady-playback buffering is not mistaken for a startup timeout.
        let deadline_service = self.clone();
        let mut startup_updates = self.subscribe();
        let mut startup_cancel = rx.clone();
        tauri::async_runtime::spawn(async move {
            let ready = async {
                loop {
                    let finished = {
                        let value = startup_updates.borrow();
                        value["generation"].as_u64() != Some(g)
                            || !matches!(value["status"].as_str(), Some("resolving" | "buffering"))
                    };
                    if finished {
                        return;
                    }
                    if startup_updates.changed().await.is_err() {
                        return;
                    }
                }
            };
            tokio::select! {
                biased;
                _=startup_cancel.wait_for(|cancelled|*cancelled)=>{},
                result=tokio::time::timeout_at(tokio::time::Instant::from_std(startup_deadline),ready)=>{
                    if result.is_err(){deadline_service.expire_generation(g,true);}
                }
            }
        });
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            let result = if let Some(resolved) = resolved_source {
                Ok(resolved)
            } else if recovery > 0 {
                if let Some(candidate) = candidate.as_ref() {
                    let (
                        operation_id,
                        seek_operation_id,
                        replace_on_recurrence,
                        source_swaps,
                        failed_candidates,
                    ) = {
                        let mut state = service.inner.state.lock().unwrap();
                        let matches_request = state.generation == g
                            && state.track().is_some_and(|current| current.id == track.id);
                        if matches_request && state.seek_requested.is_some() {
                            let recurrence = std::mem::take(&mut state.replace_source_on_recovery);
                            (
                                None,
                                state.seek_operation_id.clone(),
                                recurrence,
                                state.recurring_source_swaps,
                                state.failed_recurring_candidates.clone(),
                            )
                        } else if matches_request {
                            let recurrence = std::mem::take(&mut state.replace_source_on_recovery);
                            (
                                state.operation_id.clone(),
                                None,
                                recurrence,
                                state.recurring_source_swaps,
                                state.failed_recurring_candidates.clone(),
                            )
                        } else {
                            (None, None, false, 0, Vec::new())
                        }
                    };
                    let strict_recording_verification =
                        !service.inner.state.lock().unwrap().explicit_source;
                    let deadline = crate::playback_sources::PlaybackDeadline::at(
                        tokio::time::Instant::from_std(startup_deadline),
                    );
                    let replace_source = service.inner.store.settings().is_ok_and(|settings| {
                        recovery_should_replace_source(
                            recovery,
                            !strict_recording_verification,
                            &track,
                            &settings,
                            replace_on_recurrence,
                            source_swaps,
                        )
                    });
                    if replace_source {
                        if let Ok(mut state) = service.inner.state.lock() {
                            if state.generation == g {
                                remember_failed_recurring_candidate(&mut state, candidate);
                                state.recurring_source_swaps = state
                                    .recurring_source_swaps
                                    .saturating_add(1)
                                    .min(MAX_RECURRING_SOURCE_SWAPS);
                            }
                        }
                        let mut excluded = failed_candidates;
                        let key = (candidate.provider.clone(), candidate.id.clone());
                        excluded.retain(|entry| entry != &key);
                        excluded.push(key);
                        if excluded.len() > MAX_RECURRING_SOURCE_SWAPS as usize {
                            excluded.remove(0);
                        }
                        let failed_candidates = excluded
                            .into_iter()
                            .map(|(provider, id)| {
                                crate::playback_sources::FailedPlaybackCandidate { provider, id }
                            })
                            .collect::<Vec<_>>();
                        // One refresh has failed. Spend the final recovery
                        // extraction on another verified source, excluding the
                        // failed ID, rather than repeating that ID indefinitely.
                        crate::playback_sources::resolve_with_trace_context_excluding_until_attempt_limit(
                            &service.inner.store, &service.inner.acquisition, &track, None,
                            rx.clone(), operation_id, seek_operation_id, None,
                            &failed_candidates, deadline, 1,
                        ).await
                    } else {
                        crate::playback_sources::recover_with_trace_context_until(
                            &service.inner.store,
                            &service.inner.acquisition,
                            &track,
                            candidate,
                            recovery >= 2,
                            rx.clone(),
                            operation_id,
                            seek_operation_id,
                            None,
                            deadline,
                            strict_recording_verification,
                        )
                        .await
                    }
                } else {
                    service.resolve(g, &track, candidate, rx.clone()).await
                }
            } else {
                service.resolve(g, &track, candidate, rx.clone()).await
            };
            if !service.current(g) {
                return;
            }
            match result {
                Ok(resolved) => {
                    let crate::playback_sources::ResolvedSource {
                        mut source,
                        candidate,
                        quality,
                        station,
                        duration,
                        origin,
                    } = resolved;
                    let known_duration = duration
                        .or_else(|| {
                            track
                                .playback_range
                                .as_ref()
                                .map(|range| range.end_time - range.start_time)
                        })
                        .or(track.duration_seconds);
                    let steady_http_stream =
                        steady_http_stream(&source.input, station.is_some(), known_duration);
                    let (offset, end) = service.source_bounds(&track);
                    let loudness_settings =
                        crate::loudness_manager::load_settings(&service.inner.store)
                            .unwrap_or_default();
                    if service.native_enabled()
                        && loudness_settings.mode != crate::loudness::NormalizationMode::Off
                        && offset == 0.0
                        && end.is_none()
                    {
                        set_registered_native_opus_decode_policy(
                            &service.inner.store,
                            &mut source,
                            loudness_settings.mode,
                            crate::loudness::AudioRange {
                                start_seconds: offset,
                                end_seconds: end,
                            },
                        )
                        .await;
                    }
                    let (loudness_plan, loudness_status) = if service.native_enabled() {
                        let plan = service.resolved_native_loudness_plan(
                            &loudness_settings,
                            &source,
                            &track,
                            station.as_ref(),
                            Some(rx.clone()),
                            crate::loudness::AudioRange {
                                start_seconds: offset,
                                end_seconds: end,
                            },
                            service
                                .inner
                                .loudness_album_context
                                .lock()
                                .ok()
                                .and_then(|context| context.clone()),
                            true,
                        );
                        let pending = plan.pending_status();
                        (plan, pending)
                    } else if loudness_settings.mode == crate::loudness::NormalizationMode::Off {
                        let status = crate::loudness_manager::LoudnessStatus::off();
                        (
                            crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                                status.clone(),
                            ),
                            status,
                        )
                    } else {
                        let status = crate::loudness_manager::LoudnessStatus::unavailable(
                                loudness_settings.mode,
                                "The active WebView audio route has not reported normalization capability",
                                None,
                            );
                        (
                            crate::loudness_manager::NativeNormalizationGainPlan::fixed(
                                status.clone(),
                            ),
                            status,
                        )
                    };
                    let volume = {
                        let s = service.inner.state.lock().unwrap();
                        if s.pause_when_ready {
                            0.0
                        } else {
                            s.volume
                        }
                    };
                    let chapter_candidate = origin
                        .permits_automatic_chapter_inspection()
                        .then(|| candidate.clone())
                        .flatten();
                    {
                        let mut s = service.inner.state.lock().unwrap();
                        if s.generation != g {
                            return;
                        }
                        let decoder_diagnostic =
                            crate::playback_diagnostics::is_enabled().then(|| {
                                NativeDecoderDiagnosticContext {
                                    resolution_id: uuid::Uuid::new_v4().to_string(),
                                    native_operation_id: s.operation_id.clone(),
                                    provider: candidate
                                        .as_ref()
                                        .map(|candidate| candidate.provider.clone())
                                        .unwrap_or_else(|| track.provider.clone()),
                                    generation: g,
                                    request_started: s.request_started,
                                    decoder_started: Instant::now(),
                                }
                            });
                        s.native_decoder_diagnostic = decoder_diagnostic.clone();
                        s.source_ready_ms = Some(s.request_started.elapsed().as_millis() as u64);
                        s.operation_source_ready_ms = s
                            .operation_received_at
                            .map(|at| at.elapsed().as_millis() as u64);
                        if station.is_none() && offset == 0.0 && end.is_none() {
                            service.inner.observed_analysis.begin(
                                service.inner.output.tap(),
                                g,
                                &track,
                                candidate.as_ref(),
                                known_duration.unwrap_or(0.0),
                                position,
                            );
                        }
                        s.candidate = candidate;
                        s.quality = quality;
                        s.buffering_count += 1;
                        s.status = "buffering".into();
                        s.message = "Buffering audio…".into();
                        s.station = station.clone();
                        s.loudness_status = loudness_status;
                        s.opus_decoder_capability = active_opus_decoder_capability(&source);
                        s.steady_http_stream = steady_http_stream;
                        s.offset = offset;
                        s.end = end;
                        if s.end.is_none() {
                            if let Some(duration) = duration {
                                s.duration = duration;
                            }
                        }
                        s.revision += 1;
                        let _ = service
                            .inner
                            .output
                            .send(OutputCommand::Volume(volume as f32));
                        if let Some(context) = decoder_diagnostic.as_ref() {
                            record_native_decoder_diagnostic(context, "decoder_start", None, None);
                        }
                        if let Err(message) =
                            service
                                .inner
                                .output
                                .send(OutputCommand::LoadWithNormalizationPlan {
                                    generation: g,
                                    startup_deadline,
                                    allow_startup_reserve: !resume
                                        && offset == 0.
                                        && end.is_none()
                                        && station.is_none(),
                                    source,
                                    position: position + offset,
                                    end,
                                    plan: loudness_plan,
                                })
                        {
                            s.status = "failed".into();
                            s.opus_decoder_capability = None;
                            s.message = message;
                            if let Some(context) = s.native_decoder_diagnostic.take() {
                                record_native_decoder_diagnostic(
                                    &context,
                                    "decoder_end",
                                    Some("failed"),
                                    Some("decoder"),
                                );
                            }
                        }
                    }
                    service.publish();
                    if let Some(station) = station {
                        crate::playback_radio::spawn(
                            g,
                            station,
                            session.clone(),
                            service.inner.output.tap(),
                            service.inner.store.clone(),
                            service.inner.history.clone(),
                            service.inner.audible.subscribe(),
                            rx.clone(),
                            service.inner.radio_events.clone(),
                        );
                    }
                    if let (Some(candidate), Ok(settings)) =
                        (chapter_candidate, service.inner.store.settings())
                    {
                        let enriched_service = service.clone();
                        let track = track.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Some(candidate) =
                                crate::playback_sources::enrich_candidate_chapters(
                                    &track, candidate, &settings, rx,
                                )
                                .await
                            {
                                let mut state = enriched_service.inner.state.lock().unwrap();
                                if state.generation != g {
                                    return;
                                }
                                state.candidate = Some(candidate);
                                state.revision += 1;
                                drop(state);
                                enriched_service.publish();
                            }
                        });
                    }
                }
                Err(error) => {
                    let _command = service.inner.commands.lock().await;
                    service.recover_failure(g, &error, RecoveryTrigger::SourceResolution);
                }
            }
            drop(session);
        });
    }
    async fn resolve(
        &self,
        g: u64,
        track: &Track,
        selected: Option<Candidate>,
        cancel: watch::Receiver<bool>,
    ) -> Result<crate::playback_sources::ResolvedSource, String> {
        if !self.current(g) {
            return Err("Cancelled".into());
        }
        let (operation_id, seek_operation_id) = {
            let state = self.inner.state.lock().unwrap();
            let matches_request = state.generation == g
                && state.track().is_some_and(|current| current.id == track.id);
            if matches_request && state.seek_requested.is_some() {
                (None, state.seek_operation_id.clone())
            } else if matches_request {
                (state.operation_id.clone(), None)
            } else {
                (None, None)
            }
        };
        let deadline = restart_deadline(&self.inner.state.lock().unwrap());
        let selected = if selected.is_some() {
            selected
        } else {
            let settings = self.inner.store.settings()?;
            crate::playlist_sync::resolve_portable_playback_candidate(
                &self.inner.store,
                &self.inner.acquisition,
                track,
                &settings,
                cancel.clone(),
            )
            .await?
        };
        crate::playback_sources::resolve_with_trace_context_excluding_until(
            &self.inner.store,
            &self.inner.acquisition,
            track,
            selected,
            cancel,
            operation_id,
            seek_operation_id,
            None,
            &[],
            crate::playback_sources::PlaybackDeadline::at(tokio::time::Instant::from_std(deadline)),
        )
        .await
    }
    /// A queued chapter inherits its parent's path for immediate UI playback. If a
    /// chapter-specific excerpt has since been saved, prefer that registered asset.
    fn track_with_registered_chapter_asset(&self, track: &Track) -> Track {
        if track.provider != "chapter" || track.playback_range.is_none() {
            return track.clone();
        }
        if let Ok(Some(registered)) = self.inner.store.get_track(&track.id) {
            if registered.local_path.is_some() {
                let mut effective = track.clone();
                effective.local_path = registered.local_path;
                effective.asset_id = registered.asset_id;
                effective.local_availability = registered.local_availability;
                return effective;
            }
        }
        track.clone()
    }

    /// Return source-file bounds and classify cropped excerpt assets explicitly.
    /// Parent files use absolute chapter times; saved chapter excerpts use 0..length.
    fn source_bounds(&self, track: &Track) -> (f64, Option<f64>) {
        let asset = track
            .local_path
            .as_deref()
            .and_then(|path| self.inner.store.get_asset_by_path(path).ok().flatten());
        source_bounds_for_asset(track, asset.as_ref())
    }

    fn watch_steady_buffering(
        &self,
        generation: u64,
        seek_serial: u64,
        buffering_count: u64,
        buffering_started: Instant,
    ) {
        let service = self.clone();
        let deadline = steady_buffering_deadline(buffering_started);
        let updates = service.inner.updates.subscribe();
        tauri::async_runtime::spawn(async move {
            let watch_service = service.clone();
            let expired_owned_buffer =
                wait_for_steady_buffering_deadline(updates, deadline, move || {
                    let state = watch_service
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    steady_buffering_watch_matches(&state, generation, seek_serial, buffering_count)
                })
                .await;
            if !expired_owned_buffer {
                return;
            }
            let _command = service.inner.commands.lock().await;
            let still_owned = {
                let state = service
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                steady_buffering_watch_matches(&state, generation, seek_serial, buffering_count)
            };
            if still_owned {
                service.recover_failure(
                    generation,
                    "Audio stalled for five seconds after playback started. Check the connection or choose another source.",
                    RecoveryTrigger::SteadyBufferingTimeout,
                );
            }
        });
    }
    fn recover_failure(&self, g: u64, message: &str, trigger: RecoveryTrigger) {
        let settings = self.inner.store.settings().ok();
        let (recovery, locked_recurring_stall) = {
            let mut s = self.inner.state.lock().unwrap();
            if !claim_recovery_failure_generation(&mut s, g) {
                return;
            }
            // Resolution exhausts its own attempts. A decoder transport failure
            // can still happen between successful preflight and first audio, so
            // permit a bounded refresh within the original startup deadline.
            let can_recover = can_retry_failed_stream_at_stage(
                &s,
                message,
                trigger == RecoveryTrigger::OutputFailed,
            );
            let recurring_decision =
                if can_recover && failure_counts_as_native_transport_stall(trigger, message) {
                    record_recurring_transport_stall(&mut s, Instant::now())
                } else {
                    RecurringStallDecision::NoRepeatedStall
                };
            let locked_recurring_stall = can_recover
                && s.track().is_some_and(|track| {
                    recurring_stall_requires_terminal_failure(
                        recurring_decision,
                        track,
                        s.explicit_source,
                        settings.as_ref(),
                        s.station.is_some(),
                        s.seek_requested.is_some(),
                    )
                });
            if can_recover
                && recurring_decision != RecurringStallDecision::Exhausted
                && !locked_recurring_stall
            {
                let budget = s.startup_budget;
                let deadline = if s.first_audio_ms.is_none() {
                    s.request_started + budget
                } else {
                    Instant::now() + budget
                };
                s.recovery_deadline.get_or_insert(deadline);
                s.recovered_audio_since = None;
                s.healthy_output_since = None;
                s.replace_source_on_recovery =
                    recurring_decision == RecurringStallDecision::ReplaceSource;
                record_recovery_facts(&mut s, trigger);
                s.recovery += 1;
                (Some((s.candidate.clone(), s.position)), false)
            } else {
                (None, locked_recurring_stall)
            }
        };
        if let Some((candidate, position)) = recovery {
            self.invalidate_candidate_preview_audio(candidate.as_ref());
            self.start(candidate, position, true);
        } else if locked_recurring_stall {
            self.fail(
                g,
                "The selected source repeatedly stalled. Choose a different source.",
            );
        } else if let Some(next) = self.failed_track_skip(g, message) {
            let failure = {
                let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                state
                    .track()
                    .map(|track| source_failure_snapshot(track, message))
            };
            self.cancel();
            self.change(|s| {
                begin_operation(
                    s,
                    Some(&format!("native-{}", uuid::Uuid::new_v4())),
                    "transition",
                    Instant::now(),
                    None,
                    None,
                    None,
                );
                s.index = next;
                s.recovery = 0;
                clear_current_source_failure(s);
                s.last_skipped_track_failure = failure;
            });
            self.start(None, 0., false);
        } else {
            self.fail(g, message);
        }
    }
    fn failed_track_skip(&self, generation: u64, message: &str) -> Option<usize> {
        if !should_skip_failed_track(message) {
            return None;
        }
        let mut s = self.inner.state.lock().unwrap();
        match failed_track_next_index(&mut s, generation, message) {
            Ok(Some(next)) => {
                record_track_failure(&mut s, generation, message, "skipped");
                Some(next)
            }
            Ok(None) => {
                record_track_failure(&mut s, generation, message, "queue_exhausted");
                None
            }
            Err(()) => None,
        }
    }
    fn invalidate_candidate_preview_audio(&self, candidate: Option<&Candidate>) {
        let (Some(candidate), Ok(settings)) = (candidate, self.inner.store.settings()) else {
            return;
        };
        self.inner
            .acquisition
            .invalidate_preview_audio(candidate, &settings);
    }
    fn fail(&self, g: u64, error: &str) {
        let mut s = self.inner.state.lock().unwrap();
        if s.generation != g {
            return;
        }
        self.inner.audible.send_replace(false);
        if let Some(cancel) = s.cancel.take() {
            cancel.send_replace(true);
        }
        if let Some(cancel) = s.preload_cancel.take() {
            cancel.send_replace(true);
        }
        s.preload = None;
        s.recovered_audio_since = None;
        s.healthy_output_since = None;
        s.replace_source_on_recovery = false;
        // Keep stop under the ownership lock so an older failure cannot stop a
        // newer load, and failed status cannot coexist with queued audible PCM.
        let _ = self.inner.output.send(OutputCommand::Stop);
        s.output_ready = false;
        s.opus_decoder_capability = None;
        s.pending_seek_generation = None;
        s.steady_http_stream = false;
        s.seek_watchdog_epoch = s.seek_watchdog_epoch.wrapping_add(1);
        s.status = "failed".into();
        s.natural_queue_end = false;
        s.message = clean_error(error);
        s.last_source_failure = s.track().map(|track| source_failure_snapshot(track, error));
        s.revision += 1;
        drop(s);
        self.publish();
    }
    async fn output_event(&self, event: OutputEvent) {
        let _command = self.inner.commands.lock().await;
        match event {
            OutputEvent::NormalizationSelected {
                generation,
                output_channels: _,
                status,
            } => {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if apply_normalization_selection(&mut state, generation, status) {
                    let publish = state.generation == generation;
                    drop(state);
                    if publish {
                        self.publish();
                    }
                }
            }
            OutputEvent::Started {
                generation,
                duration,
                sample_rate,
                channels,
                decoder,
            } => {
                if !self.current(generation) && !self.adopt_preload(generation) {
                    return;
                }
                let mut s = self.inner.state.lock().unwrap();
                if s.generation != generation || matches!(s.status.as_str(), "idle" | "failed") {
                    return;
                }
                let pause_on_ready = s.pause_when_ready;
                let restore_volume = s.volume;
                let decoder_diagnostic = s.native_decoder_diagnostic.take();
                self.inner
                    .audible
                    .send_replace(!pause_on_ready && s.volume > 0.);
                {
                    s.output_ready = true;
                    s.pending_seek_generation = None;
                    s.seek_watchdog_epoch = s.seek_watchdog_epoch.wrapping_add(1);
                    note_audio_started(&mut s);
                    if s.first_audio_ms.is_none() {
                        s.first_audio_ms = Some(s.request_started.elapsed().as_millis() as u64);
                    }
                    if s.operation_first_audio_ms.is_none() {
                        s.operation_first_audio_ms = s
                            .operation_received_at
                            .map(|at| at.elapsed().as_millis() as u64);
                    }
                    s.status = if pause_on_ready { "paused" } else { "playing" }.into();
                    if s.recovery > 0 && s.recovered_audio_since.is_none() {
                        let position = s.position;
                        s.recovered_audio_since = Some((Instant::now(), position));
                    }
                    if has_native_stall_history(&s) && s.healthy_output_since.is_none() {
                        s.healthy_output_since = Some((Instant::now(), s.position, s.seek_serial));
                    }
                    s.message = if pause_on_ready {
                        "Paused"
                    } else {
                        "Playing audio"
                    }
                    .into();
                    s.pause_when_ready = false;
                    if s.end.is_none() {
                        if let Some(duration) = duration {
                            s.duration = duration;
                            s.duration_verified = true;
                        }
                    }
                    s.quality["sampleRate"] = json!(sample_rate);
                    s.quality["channels"] = json!(channels);
                    if let Some(mut decoder) = decoder {
                        decoder.codec_hint = s.quality["codec"]
                            .as_str()
                            .map(str::to_owned)
                            .or(decoder.codec_hint);
                        s.decoder_diagnostics = Some(decoder);
                    }
                }
                if pause_on_ready {
                    let _ = self.inner.output.send(OutputCommand::Pause);
                    let _ = self
                        .inner
                        .output
                        .send(OutputCommand::Volume(restore_volume as f32));
                }
                s.revision += 1;
                drop(s);
                if let Some(context) = decoder_diagnostic.as_ref() {
                    record_native_decoder_diagnostic(
                        context,
                        "decoder_end",
                        Some("success"),
                        Some("decoder"),
                    );
                }
                self.publish();
                if !pause_on_ready {
                    self.record_started();
                    self.cache_current_artwork(generation);
                    self.prepare_next();
                }
            }
            OutputEvent::Position {
                generation,
                seconds,
            } => {
                if !self.current(generation) {
                    return;
                }
                let (position, duration, revision, save, lite) = {
                    let mut s = self.inner.state.lock().unwrap();
                    if s.generation != generation || s.status != "playing" {
                        return;
                    }
                    s.position = (seconds - s.offset).max(0.);
                    if s.recovery > 0 && s.recovered_audio_since.is_none() {
                        s.recovered_audio_since = Some((Instant::now(), s.position));
                    }
                    if has_native_stall_history(&s) && s.healthy_output_since.is_none() {
                        s.healthy_output_since = Some((Instant::now(), s.position, s.seek_serial));
                    }
                    if s.recovered_audio_since.is_some_and(|(started, position)| {
                        started.elapsed() >= Duration::from_secs(5) && s.position - position >= 5.
                    }) {
                        s.recovery = 0;
                        s.recovery_deadline = None;
                        s.recovered_audio_since = None;
                    }
                    if native_recovery_output_is_healthy(
                        s.healthy_output_since,
                        Instant::now(),
                        s.position,
                        s.seek_serial,
                    ) {
                        clear_native_stall_history(&mut s);
                        s.healthy_output_since = None;
                    }
                    s.revision += 1;
                    let save = s.last_saved.elapsed() > Duration::from_secs(10);
                    if save {
                        s.last_saved = Instant::now();
                    }
                    (s.position, s.duration, s.revision, save, s.chapter())
                };
                let _=self.inner.app.emit("native-playback-position",json!({"generation":generation,"revision":revision,"position":position,"duration":duration,"buffering":false,"chapter":lite}));
                self.inner.updates.send_modify(|snapshot| {
                    if snapshot["generation"].as_u64() == Some(generation) {
                        snapshot["position"] = json!(position);
                        snapshot["duration"] = json!(duration);
                        snapshot["revision"] = json!(revision);
                        snapshot["chapter"] = lite;
                    }
                });
                if save {
                    self.persist();
                }
            }
            OutputEvent::Buffering {
                generation,
                diagnostics,
            } => {
                let adopted = !self.current(generation) && self.adopt_preload(generation);
                let mut s = self.inner.state.lock().unwrap();
                if !apply_buffering_event(&mut s, generation, diagnostics) {
                    return;
                }
                s.recovered_audio_since = None;
                s.healthy_output_since = None;
                let preload_deadline = adopted.then(|| s.request_started + s.startup_budget);
                let steady_watch = steady_buffering_watch_matches(
                    &s,
                    generation,
                    s.seek_serial,
                    s.buffering_count,
                )
                .then_some((
                    s.generation,
                    s.seek_serial,
                    s.buffering_count,
                    Instant::now(),
                ));
                self.inner.audible.send_replace(false);
                drop(s);
                self.publish();
                if let Some(deadline) = preload_deadline {
                    self.watch_preload_startup(generation, deadline);
                } else {
                    // Speculative readers never compete with recovery of the
                    // active occurrence. Started will prepare the next again.
                    self.clear_preload();
                }
                if let Some((generation, seek_serial, buffering_count, buffering_started)) =
                    steady_watch
                {
                    self.watch_steady_buffering(
                        generation,
                        seek_serial,
                        buffering_count,
                        buffering_started,
                    );
                }
            }
            OutputEvent::Ended { generation } => {
                if self.current(generation) {
                    let premature = {
                        let s = self.inner.state.lock().unwrap();
                        if matches!(s.status.as_str(), "failed" | "paused" | "idle") {
                            return;
                        }
                        ended_prematurely(&s)
                    };
                    if premature {
                        self.recover_failure(
                            generation,
                            "Audio stream ended before the recording finished",
                            RecoveryTrigger::PrematureEnded,
                        );
                    } else {
                        let duration = {
                            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                            // A chapter/range end and a radio disconnect do not
                            // establish whole-track coverage.
                            (state.offset == 0.0 && state.end.is_none() && state.station.is_none())
                                .then_some(state.duration)
                        };
                        if let Some(duration) = duration {
                            let tap = self.inner.output.tap();
                            let _ = tauri::async_runtime::spawn_blocking(move || {
                                tap.mark_verified_end(generation, duration)
                            })
                            .await;
                        }
                        if !self.current(generation) {
                            return;
                        }
                        self.advance(1, true);
                    }
                }
            }
            OutputEvent::DeviceLost {
                generation,
                position,
                reason,
            } => {
                if self.current(generation) {
                    let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    if !apply_device_lost(&mut s, generation, position, &reason) {
                        return;
                    }
                    self.inner.audible.send_replace(false);
                    drop(s);
                    self.publish();
                }
            }
            OutputEvent::Interrupted { generation } => {
                let restart = {
                    let mut s = self.inner.state.lock().unwrap();
                    if s.generation != generation {
                        return;
                    }
                    s.output_ready = false;
                    if matches!(s.status.as_str(), "playing" | "buffering") {
                        Some((s.candidate.clone(), s.position))
                    } else {
                        None
                    }
                };
                if let Some((candidate, position)) = restart {
                    self.start(candidate, position, true);
                }
            }
            OutputEvent::LocalStorageFailed { generation, fault } => {
                // Future local faults are only emitted once output selected that preload.
                if !self.current(generation) && !self.adopt_preload(generation) {
                    return;
                }
                let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if apply_local_storage_failure(&mut s, generation, fault) {
                    self.inner.audible.send_replace(false);
                    let _ = self.inner.output.send(OutputCommand::Stop);
                    drop(s);
                    self.publish();
                }
            }
            OutputEvent::Failed {
                generation,
                message,
            } => {
                if !self.current(generation) && !self.adopt_preload(generation) {
                    return;
                }
                let decoder_diagnostic = {
                    let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    (state.generation == generation)
                        .then(|| state.native_decoder_diagnostic.take())
                        .flatten()
                };
                if let Some(context) = decoder_diagnostic.as_ref() {
                    record_native_decoder_diagnostic(
                        context,
                        "decoder_end",
                        Some("failed"),
                        Some("decoder"),
                    );
                }
                self.recover_failure(generation, &message, RecoveryTrigger::OutputFailed);
            }
        }
    }
    fn clear_preload(&self) {
        let mut s = self.inner.state.lock().unwrap();
        let abandoned_generation = s.preload.as_ref().map(|preload| preload.generation);
        let active_generation = s.generation;
        cancel_preload_state(&mut s);
        drop(s);
        if let Some(generation) = abandoned_generation {
            self.inner.observed_analysis.cancel_generation(generation);
        }
        let _ = self.inner.output.send(OutputCommand::ClearPreload {
            generation: active_generation,
        });
    }
    fn watch_preload_startup(&self, generation: u64, deadline: Instant) {
        let service = self.clone();
        let mut updates = self.subscribe();
        tauri::async_runtime::spawn(async move {
            let wait = async {
                loop {
                    {
                        let value = updates.borrow();
                        if value["generation"].as_u64() != Some(generation)
                            || !matches!(value["status"].as_str(), Some("resolving" | "buffering"))
                        {
                            return;
                        }
                    }
                    if updates.changed().await.is_err() {
                        return;
                    }
                }
            };
            if tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), wait)
                .await
                .is_err()
            {
                service.expire_generation(generation, true);
            }
        });
    }
    fn prepare_next(&self) {
        let (owner, track, cancel) = {
            let mut s = self.inner.state.lock().unwrap();
            if s.status != "playing" || s.station.is_some() || s.preload_cancel.is_some() {
                return;
            }
            let Some(index) = queue_next(s.queue.len(), s.index, &s.repeat, 1, true) else {
                return;
            };
            let track = s.queue[index].clone();
            if track.provider == "radio" {
                return;
            }
            let (tx, rx) = watch::channel(false);
            s.preload_cancel = Some(tx.clone());
            let owner = PreloadPreparationOwner {
                generation: s.generation,
                index,
                entry: s.entries[index].clone(),
                cancel: tx,
            };
            (owner, track, rx)
        };
        let track = self.track_with_registered_chapter_asset(&track);
        let service = self.clone();
        let window_seconds = configured_preparation_window_seconds();
        let operation_id = format!("preparation:{}", uuid::Uuid::new_v4());
        let trace =
            crate::playback_sources::SourceTrace::new(Some(operation_id.clone()), None, None);
        tauri::async_runtime::spawn(async move {
            let span = crate::playback_sources::TraceSpan::begin(
                trace.clone(),
                "other",
                "next_preparation_start",
                "next_preparation_end",
            );
            trace.event(
                "next_preparation_policy",
                "other",
                json!({
                    "windowSeconds":window_seconds,"generation":owner.generation,
                    "localSource":track.local_path.is_some(),
                }),
            );
            let mut updates = service.subscribe();
            let mut stop_discovery = None;
            // Use the full idle lead time to warm stable provider results.
            // This remains explicitly search/identity-only; the 45s deadline
            // does not authorize returning an old signed URL at handoff.
            if track.local_path.is_none() {
                let background_service = service.clone();
                let background_track = track.clone();
                let background_owner_cancel = cancel.clone();
                let background_trace = trace.clone();
                let (discovery_stop, discovery_cancel) = watch::channel(false);
                stop_discovery = Some(discovery_stop.clone());
                let discovery_stop_for_owner = discovery_stop.clone();
                tauri::async_runtime::spawn(async move {
                    let discovery = async {
                        let settings = background_service.inner.store.settings()?;
                        let mut providers = vec![settings.preferred_streaming_source.clone()];
                        if settings.streaming_fallback_enabled {
                            providers.extend(settings.streaming_fallback_sources.iter().cloned());
                        }
                        let mut seen = HashSet::new();
                        providers.retain(|provider| seen.insert(provider.clone()));
                        warm_upcoming_playback_identity(
                            &background_service.inner.acquisition,
                            &background_service.inner.store,
                            &background_track,
                            &settings,
                            &providers,
                            discovery_cancel.clone(),
                            crate::playback_sources::PlaybackDeadline::after(
                                crate::playback_sources::NEXT_SOURCE_DISCOVERY_BUDGET,
                            ),
                            &background_trace,
                        )
                        .await
                    };
                    let result = tokio::select! {
                        biased;
                        _ = crate::playback_sources::wait_for_cancellation(background_owner_cancel) => {
                            discovery_stop_for_owner.send_replace(true);
                            Err("Cancelled".into())
                        },
                        result = discovery => result,
                    };
                    background_trace.event(
                        "next_preparation_discovery_end",
                        "other",
                        json!({
                            "preparationKind":"candidate_search_only",
                            "budgetMs":crate::playback_sources::NEXT_SOURCE_DISCOVERY_BUDGET.as_millis(),
                            "ok":result.is_ok(),"resultCount":result.ok(),
                        }),
                    );
                });
                trace.event(
                    "next_preparation_discovery_start",
                    "other",
                    json!({
                        "preparationKind":"candidate_search_only",
                        "budgetMs":crate::playback_sources::NEXT_SOURCE_DISCOVERY_BUDGET.as_millis(),
                    }),
                );
            }
            loop {
                let prepare_now = {
                    let state = service.inner.state.lock().unwrap();
                    if !preload_preparation_is_current(&state, &owner) {
                        span.finish(if *cancel.borrow() {
                            "cancelled"
                        } else {
                            "obsolete"
                        });
                        return;
                    }
                    state.duration <= 0. || state.duration - state.position <= window_seconds
                };
                if prepare_now {
                    break;
                }
                tokio::select! {
                    biased;
                    _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => {
                        span.finish("cancelled"); return;
                    },
                    changed = updates.changed() => {
                        if changed.is_err() { span.finish("updates_closed"); return; }
                    },
                }
            }
            // Stop background identity queries before starting expiring URL
            // resolution. The queue owner still controls the audio preload.
            if let Some(stop) = stop_discovery.take() {
                stop.send_replace(true);
            }
            trace.event("next_preparation_resolve_start", "other", json!({}));
            let remaining_seconds = {
                let state = service
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.duration.is_finite() && state.duration > state.position {
                    (state.duration - state.position).min(45.0).max(0.0)
                } else {
                    20.0
                }
            };
            let handoff =
                tokio::time::Instant::now() + Duration::from_secs_f64(remaining_seconds.max(0.05));
            let mut resolved = crate::playback_sources::resolve_for_preload_until(
                &service.inner.store,
                &service.inner.acquisition,
                &track,
                cancel.clone(),
                operation_id,
                handoff,
            )
            .await;
            let resolved_ok = resolved.is_ok();
            trace.event(
                "next_preparation_resolve_end",
                "other",
                json!({"ok":resolved_ok}),
            );
            let (offset, end) = service.source_bounds(&track);
            let loudness_settings =
                crate::loudness_manager::load_settings(&service.inner.store).unwrap_or_default();
            // Never await codec probing under PlaybackService::state. Check the
            // preparation owner first, do the bounded probe outside the lock,
            // then revalidate through finish_preload_resolution before commit.
            let owner_current = {
                let state = service
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                preload_preparation_is_current(&state, &owner)
            };
            if owner_current
                && service.native_enabled()
                && loudness_settings.mode != crate::loudness::NormalizationMode::Off
                && offset == 0.0
                && end.is_none()
            {
                if let Ok(resolved_source) = resolved.as_mut() {
                    set_registered_native_opus_decode_policy(
                        &service.inner.store,
                        &mut resolved_source.source,
                        loudness_settings.mode,
                        crate::loudness::AudioRange {
                            start_seconds: offset,
                            end_seconds: end,
                        },
                    )
                    .await;
                }
            }
            let mut s = service.inner.state.lock().unwrap();
            let Some(resolved) = finish_preload_resolution(&mut s, &owner, resolved) else {
                span.finish(if *cancel.borrow() {
                    "cancelled"
                } else if resolved_ok {
                    "obsolete"
                } else {
                    "failed"
                });
                return;
            };
            let queue_album_context = s.loudness_queue_contexts.get(&owner.entry).cloned();
            let (loudness_plan, loudness_status) = if service.native_enabled() {
                let plan = service.resolved_native_loudness_plan(
                    &loudness_settings,
                    &resolved.source,
                    &track,
                    resolved.station.as_ref(),
                    Some(cancel.clone()),
                    crate::loudness::AudioRange {
                        start_seconds: offset,
                        end_seconds: end,
                    },
                    queue_album_context.clone(),
                    false,
                );
                let pending = plan.pending_status();
                (plan, pending)
            } else {
                let status = crate::loudness_manager::LoudnessStatus::off();
                (
                    crate::loudness_manager::NativeNormalizationGainPlan::fixed(status.clone()),
                    status,
                )
            };
            let known_duration = resolved
                .duration
                .or_else(|| {
                    track
                        .playback_range
                        .as_ref()
                        .map(|range| range.end_time - range.start_time)
                })
                .or(track.duration_seconds);
            let steady_http_stream = steady_http_stream(
                &resolved.source.input,
                resolved.station.is_some(),
                known_duration,
            );
            s.generation_counter += 1;
            let generation = s.generation_counter;
            if resolved.station.is_none() && offset == 0.0 && end.is_none() {
                service.inner.observed_analysis.begin_preload(
                    service.inner.output.tap(),
                    generation,
                    &track,
                    resolved.candidate.as_ref(),
                    known_duration.unwrap_or(0.0),
                    offset,
                );
            }
            let opus_decoder_capability = active_opus_decoder_capability(&resolved.source);
            s.preload = Some(Preloaded {
                preparation_trace: Some(trace.clone()),
                generation,
                index: owner.index,
                entry: owner.entry,
                candidate: resolved.candidate,
                quality: resolved.quality,
                duration: resolved.duration,
                steady_http_stream,
                offset,
                end,
                loudness_status,
                loudness_album_context: queue_album_context,
                opus_decoder_capability,
            });
            let queued = service
                .inner
                .output
                .send(OutputCommand::PreloadWithNormalizationPlan {
                    generation: owner.generation,
                    next_generation: generation,
                    source: resolved.source,
                    position: offset,
                    end,
                    plan: loudness_plan,
                });
            if queued.is_err() {
                service
                    .inner
                    .observed_analysis
                    .cancel_generation(generation);
                if s.preload
                    .as_ref()
                    .is_some_and(|preload| preload.generation == generation)
                {
                    s.preload = None;
                }
            }
            trace.event(
                "next_preparation_queued",
                "other",
                json!({"ok":queued.is_ok(),"generation":generation}),
            );
            span.finish(if queued.is_ok() {
                "queued"
            } else {
                "output_unavailable"
            });
        });
    }
    fn adopt_preload(&self, generation: u64) -> bool {
        let mut s = self.inner.state.lock().unwrap();
        if !s
            .preload
            .as_ref()
            .is_some_and(|p| p.generation == generation && s.entries.get(p.index) == Some(&p.entry))
        {
            return false;
        }
        let p = s.preload.take().unwrap();
        adopt_preloaded_opus_capability(&mut s, &p);
        if let Some(trace) = p.preparation_trace.as_ref() {
            trace.event(
                "next_preparation_adopted",
                "other",
                json!({"generation":generation}),
            );
        }
        s.preload_cancel = None;
        if let Some(cancel) = s.cancel.take() {
            let _ = cancel.send(true);
        }
        s.generation = generation;
        s.last_buffering = None;
        s.index = p.index;
        s.position = 0.;
        s.offset = p.offset;
        s.end = p.end;
        let range_duration = s
            .track()
            .and_then(|t| t.playback_range.as_ref())
            .map(|r| r.end_time - r.start_time);
        s.duration_verified = range_duration.is_some();
        s.duration = range_duration.or(p.duration).unwrap_or(0.);
        s.candidate = p.candidate;
        s.explicit_source = false;
        s.recovery_deadline = None;
        s.recovered_audio_since = None;
        s.quality = p.quality;
        s.loudness_status = p.loudness_status;
        let active_album_context = p.loudness_album_context;
        s.steady_http_stream = p.steady_http_stream;
        s.session = uuid::Uuid::new_v4().to_string();
        s.recorded = false;
        reset_failed_resume_diagnostics(&mut s);
        begin_operation(&mut s, None, "transition", Instant::now(), None, None, None);
        s.output_ready = true;
        s.revision += 1;
        drop(s);
        if let Ok(mut context) = self.inner.loudness_album_context.lock() {
            *context = active_album_context;
        }
        true
    }
    fn cache_current_artwork(&self, generation: u64) {
        let url = {
            let state = self.inner.state.lock().unwrap();
            if state.generation != generation {
                return;
            }
            let value = state.snapshot(self.native_enabled(), false);
            let Some(url) = value["track"]["artworkUrl"]
                .as_str()
                .filter(|u| u.starts_with("https://"))
            else {
                return;
            };
            if state
                .artwork_cache
                .as_ref()
                .is_some_and(|(cached, _)| cached == url)
            {
                return;
            }
            url.to_owned()
        };
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(path) = crate::cache_artwork(service.inner.app.clone(), url.clone()).await
            else {
                return;
            };
            let mut state = service.inner.state.lock().unwrap();
            if state.generation != generation
                || state.snapshot(service.native_enabled(), false)["track"]["artworkUrl"].as_str()
                    != Some(url.as_str())
            {
                return;
            }
            state.artwork_cache = Some((url, path));
            state.revision += 1;
            drop(state);
            service.publish();
        });
    }
    fn record_started(&self) {
        let data = {
            let mut s = self.inner.state.lock().unwrap();
            if s.recorded || s.station.is_some() {
                return;
            }
            s.recorded = true;
            s.track().cloned().map(|track| {
                (
                    s.session.clone(),
                    track,
                    s.chapter(),
                    s.candidate.as_ref().map(|c| c.provider.clone()),
                )
            })
        };
        if let Some((session, track, chapter, provider)) = data {
            let _ = self.inner.history.record_enriched(
                &self.inner.store,
                &session,
                provider.as_deref().unwrap_or(&track.provider),
                &track,
                if chapter.is_null() {
                    None
                } else {
                    Some(chapter)
                },
            );
        }
    }
    fn persist(&self) {
        persist_playback_state(
            &self.inner.state,
            &self.inner.persistence,
            |queue, session| self.inner.store.save_playback_checkpoint(queue, session),
            || {},
        );
    }
    fn advance(&self, direction: i32, ended: bool) {
        let (next, natural_queue_end) = {
            let s = self.inner.state.lock().unwrap();
            (
                queue_next(s.queue.len(), s.index, &s.repeat, direction, ended),
                is_natural_queue_end(s.queue.len(), s.index, &s.repeat, ended),
            )
        };
        if ended {
            self.change(|s| {
                begin_operation(
                    s,
                    Some(&format!("native-{}", uuid::Uuid::new_v4())),
                    "transition",
                    Instant::now(),
                    None,
                    None,
                    None,
                )
            });
        }
        self.cancel();
        if let Some(index) = next {
            self.change(|s| {
                s.index = index;
                s.recovery = 0;
                clear_current_source_failure(s);
            });
            self.start(None, 0., false);
        } else {
            self.change(|s| {
                s.status = "ended".into();
                s.natural_queue_end = natural_queue_end;
                s.message = "Queue finished".into();
            });
        }
    }
    fn radio_event(&self, event: RadioEvent) {
        let RadioEvent::Metadata {
            generation,
            station: _,
            metadata,
            history,
        } = event;
        let mut s = self.inner.state.lock().unwrap();
        if s.generation != generation || s.station.is_none() {
            return;
        }
        let station = s.station.as_ref().unwrap();
        s.radio_now = metadata
            .as_ref()
            .map(|m| {
                json!({
                    "id":crate::playback_history::radio_song_track(station,m).id,
                    "title":m.title,"artist":m.artist,"stationName":station.name,"metadata":m
                })
            })
            .unwrap_or(Value::Null);
        s.radio_history = history;
        s.revision += 1;
        drop(s);
        self.publish();
        self.cache_current_artwork(generation);
    }
    pub async fn identify_radio(
        &self,
        station_id: &str,
    ) -> Result<crate::radio::RadioMetadata, String> {
        self.identify_radio_for(station_id, None).await
    }

    pub async fn identify_radio_for(
        &self,
        station_id: &str,
        caller_id: Option<String>,
    ) -> Result<crate::radio::RadioMetadata, String> {
        let (generation, station, session) = {
            let state = self.inner.state.lock().unwrap();
            let station = state
                .station
                .clone()
                .filter(|s| s.id == station_id)
                .ok_or("This station is not playing")?;
            if state.status != "playing" {
                return Err("Play the station before identifying its audio".into());
            }
            (state.generation, station, state.session.clone())
        };
        let job = self.inner.playing_track_identifications.begin(caller_id)?;
        let request_id = job.id().to_owned();
        let cancelled = job.cancellation();
        let capture_cancelled = cancelled.clone();
        let path =
            std::env::temp_dir().join(format!("burrata-recognition-{}.wav", uuid::Uuid::new_v4()));
        let cleanup = Arc::new(TemporaryPcmFile(path.clone()));
        let capture_cleanup = cleanup.clone();
        let tap = self.inner.output.tap();
        let service = self.clone();
        let worker_station = station.clone();
        let updates = self.subscribe();
        let seek_serial = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .seek_serial;
        // The worker owns both guards even if the command caller disappears.
        // Its own deadline and source watcher keep detached work bounded.
        let worker = tokio::spawn(async move {
            let _job = job;
            let _cleanup = cleanup;
            let work = crate::radio::recognize_played_samples_progressively_manual(
                &worker_station,
                cancelled.clone(),
                move |seconds| {
                    let path = path.clone();
                    let tap = tap.clone();
                    let cleanup = capture_cleanup.clone();
                    let cancelled = capture_cancelled.clone();
                    async move {
                        while tap.available_seconds(generation) < seconds {
                            if *cancelled.borrow() {
                                return Err("Audio identification was cancelled".into());
                            }
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                        if *cancelled.borrow() {
                            return Err("Audio identification was cancelled".into());
                        }
                        tauri::async_runtime::spawn_blocking(move || {
                            let _cleanup = cleanup;
                            tap.capture_wav(generation, &path, seconds)?;
                            Ok::<_, String>(path)
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    }
                },
            );
            let mut work = Box::pin(work);
            let mut invalidated = Box::pin(wait_for_native_radio_identification_invalidation(
                cancelled,
                updates,
                worker_station.id.clone(),
                generation,
                seek_serial,
            ));
            let terminal = tokio::select! {
                result = &mut work => return result,
                error = &mut invalidated => error,
                _ = tokio::time::sleep(Duration::from_secs(60)) => "Radio identification exceeded 60 seconds".into(),
            };
            service
                .inner
                .playing_track_identifications
                .cancel(&request_id);
            // Cancellation interrupts gate waits and kills/reaps SongRec before
            // the task-owned WAV/job guards can be released.
            let _ = work.await;
            Err(terminal)
        });
        let (metadata, note) = worker.await.map_err(|error| error.to_string())??;
        if !native_radio_identification_matches(
            &self.snapshot_without_queue(),
            &station.id,
            generation,
            seek_serial,
        ) || !*self.inner.audible.borrow()
        {
            return Err("Playback changed during identification".into());
        }
        let already_current = metadata.as_ref().is_some_and(|identified| {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.station.as_ref().is_some_and(|current| {
                current.id == station.id && same_radio_song(&state.radio_now, identified)
            })
        });
        let mut history = if already_current {
            self.inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .radio_history
                .clone()
        } else {
            crate::playback_radio::history_for_station(
                &self.inner.history,
                &self.inner.store,
                &station,
            )
        };
        if let Some(metadata) = metadata.as_ref().filter(|_| !already_current) {
            let now = chrono::Utc::now().to_rfc3339();
            let _ = self.inner.history.record_radio_occurrence(
                &self.inner.store,
                &session,
                &format!("manual:{}", uuid::Uuid::new_v4()),
                &now,
                &station,
                metadata,
            );
            let track = crate::playback_history::radio_song_track(&station, metadata);
            history.insert(0,json!({"id":uuid::Uuid::new_v4().to_string(),"track":track,"playedAt":now,"stationName":station.name}));
        }
        crate::radio::record_played_radio_identification(
            &station.id,
            generation,
            metadata.as_ref(),
        );
        self.radio_event(RadioEvent::Metadata {
            generation,
            station: station.clone(),
            metadata: metadata.clone(),
            history,
        });
        Ok(crate::radio::RadioMetadata {
            station_id: station.id,
            now_playing: metadata,
            seconds_remaining: None,
            history: vec![],
            history_available: false,
            note: note.unwrap_or_else(|| {
                "Identified from audio played by Burrata; catalog identity is unverified.".into()
            }),
        })
    }

    /// Identify the current pre-volume PCM without changing track metadata, queue,
    /// or radio history. Legacy automation callers receive a private request ID.
    pub async fn identify_playing_track(&self) -> Result<PlayingTrackIdentification, String> {
        self.identify_playing_track_for(None).await
    }

    /// Caller-scoped variant used by UI callers that support explicit cancel.
    pub async fn identify_playing_track_for(
        &self,
        caller_id: Option<String>,
    ) -> Result<PlayingTrackIdentification, String> {
        self.identify_playing_track_inner(caller_id, None).await
    }

    pub async fn identify_playing_track_for_owner(
        &self,
        caller_id: Option<String>,
        expected_track_id: &str,
        expected_generation: u64,
        expected_seek_serial: u64,
        expected_candidate: Option<(&str, &str)>,
    ) -> Result<PlayingTrackIdentification, String> {
        self.identify_playing_track_inner(
            caller_id,
            Some((
                expected_track_id,
                expected_generation,
                expected_seek_serial,
                expected_candidate,
            )),
        )
        .await
    }

    async fn identify_playing_track_inner(
        &self,
        caller_id: Option<String>,
        expected_owner: Option<(&str, u64, u64, Option<(&str, &str)>)>,
    ) -> Result<PlayingTrackIdentification, String> {
        if !self.native_enabled() {
            return Err("Native playback identification is unavailable".into());
        }
        let job = self.inner.playing_track_identifications.begin(caller_id)?;
        let cancelled = job.cancellation();
        let result_cancelled = cancelled.clone();
        let recognition_cancelled = cancelled.clone();
        let request_id = job.id().to_owned();
        let (
            generation,
            track_id,
            entry_id,
            requested_track,
            active_candidate,
            seek_serial,
            chapter,
            capture_seconds,
            timing_anchor_eligible,
            active_local_asset_id,
        ) = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.status != "playing" {
                return Err("Play audio before identifying the current track".into());
            }
            let track = state.track().ok_or("There is no current playback track")?;
            if expected_owner.is_some_and(
                |(expected_track_id, generation, seek_serial, expected_candidate)| {
                    track.id != expected_track_id
                        || state.generation != generation
                        || state.seek_serial != seek_serial
                        || !same_detection_candidate_identity(
                            state.candidate.as_ref(),
                            expected_candidate,
                        )
                },
            ) {
                return Err("Playback changed before identification started".into());
            }
            let chapter = state.chapter();
            let capture_seconds = native_identification_capture_seconds(
                track.playback_range.is_some(),
                chapter["startSeconds"].as_f64(),
                state.position,
            )?;
            let timing_anchor_eligible = track.playback_range.is_none() && chapter.is_null();
            let active_local_asset_id = (state.quality["sourceKind"].as_str()
                == Some("localAsset"))
            .then(|| state.quality["assetId"].as_str())
            .flatten()
            .map(str::to_owned);
            (
                state.generation,
                track.id.clone(),
                state.entries.get(state.index).cloned().unwrap_or_default(),
                track.clone(),
                state.candidate.clone(),
                state.seek_serial,
                chapter,
                capture_seconds,
                timing_anchor_eligible,
                active_local_asset_id,
            )
        };
        let station = RadioStation {
            // This is intentionally unique per request. Native current-track
            // identification must not reuse radio station cooldowns/results,
            // since one DJ/concert queue item may contain many recordings.
            id: format!("playing:{}", job.id()),
            name: "Currently playing audio".into(),
            stream_url: String::new(),
            source_url: String::new(),
            provider: Some("playback_pcm".into()),
            country: None,
            tags: None,
            homepage_url: None,
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: true,
        };
        let active_source_identity = native_detection_source_identity(
            active_local_asset_id.as_deref(),
            active_candidate.as_ref(),
        );
        let detection_store = self.inner.store.clone();
        let detection_track = requested_track.clone();
        let detection_identity = active_source_identity.clone();
        let detection_asset_id = active_local_asset_id.clone();
        let detection_resource = tokio::task::spawn_blocking(move || {
            let verified_local_hash =
                if crate::playing_track_detections::requires_verified_local_hash(
                    &detection_track,
                    detection_identity.as_ref(),
                    detection_asset_id.is_some(),
                ) {
                    detection_asset_id
                        .as_deref()
                        .or(detection_track.asset_id.as_deref())
                        .and_then(|asset_id| {
                            crate::playing_track_detections::verified_local_asset_hash(
                                detection_store.as_ref(),
                                asset_id,
                            )
                        })
                } else {
                    None
                };
            crate::playing_track_detections::DetectionResourceKey::for_playback_source(
                &detection_track,
                detection_identity.as_ref(),
                verified_local_hash.as_deref(),
            )
        })
        .await
        .map_err(|error| error.to_string())?;
        let path =
            std::env::temp_dir().join(format!("burrata-recognition-{}.wav", uuid::Uuid::new_v4()));
        let cleanup = Arc::new(TemporaryPcmFile(path.clone()));
        let recognition_cleanup = cleanup.clone();
        let capture_path = path.clone();
        let tap = self.inner.output.tap();
        let recognition = async move {
            // Try short fingerprints first; only a clean no-match warrants a
            // larger excerpt. SongRec calls remain serial and share its gate.
            let mut last_attempt = None;
            let capture_started = std::time::Instant::now();
            for seconds in crate::pcm_tap::RECOGNITION_CAPTURE_WINDOWS {
                loop {
                    if *recognition_cancelled.borrow() {
                        return Err("Audio identification was cancelled".into());
                    }
                    let spaced = last_attempt.is_none_or(|at: std::time::Instant| {
                        at.elapsed() >= Duration::from_secs(10)
                    });
                    if tap.available_seconds(generation) >= seconds
                        && seconds <= capture_seconds + capture_started.elapsed().as_secs_f64()
                        && spaced
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let capture_cleanup = recognition_cleanup.clone();
                let capture_path = capture_path.clone();
                let tap = tap.clone();
                let capture = tauri::async_runtime::spawn_blocking(move || {
                    let _cleanup = capture_cleanup;
                    let timing = tap.capture_wav_with_timing(generation, &capture_path, seconds)?;
                    Ok::<_, String>((capture_path, timing))
                })
                .await
                .map_err(|error| error.to_string())??;
                if *recognition_cancelled.borrow() {
                    return Err("Audio identification was cancelled".into());
                }
                last_attempt = Some(std::time::Instant::now());
                let (capture, timing) = capture;
                let identified = crate::radio::recognize_captured_sample_with_offset_cancellable(
                    &station,
                    &capture,
                    recognition_cancelled.clone(),
                )
                .await?;
                if identified.is_some() {
                    return Ok(identified.map(|identified| (identified, timing)));
                }
            }
            Ok(None)
        };
        let updates = self.subscribe();
        if !native_identification_matches(
            &self.snapshot_without_queue(),
            &track_id,
            &entry_id,
            generation,
            seek_serial,
            &chapter,
        ) {
            return Err("Playback changed before identification started".into());
        }
        let mut invalidated = Box::pin(wait_for_native_identification_invalidation(
            cancelled,
            updates,
            track_id.clone(),
            entry_id.clone(),
            generation,
            seek_serial,
            chapter.clone(),
        ));
        // The worker owns the registry guard and temporary WAV until its
        // capture/recognizer cleanup completes. If this command caller goes
        // away, the request remains visible to explicit cancellation and
        // application shutdown rather than orphaning an untracked child.
        let mut recognition = tokio::spawn(async move {
            let _job = job;
            let _cleanup = cleanup;
            recognition.await
        });
        enum RecognitionOutcome {
            Complete(
                Result<
                    Option<(
                        crate::radio::SongRecRecognition,
                        crate::pcm_tap::PcmCaptureTiming,
                    )>,
                    String,
                >,
            ),
            Invalidated(String),
            TimedOut,
        }
        let outcome = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(60), &mut recognition) => {
                match result {
                    Ok(Ok(result)) => RecognitionOutcome::Complete(result),
                    Ok(Err(error)) => RecognitionOutcome::Complete(Err(error.to_string())),
                    Err(_) => RecognitionOutcome::TimedOut,
                }
            }
            error = &mut invalidated => RecognitionOutcome::Invalidated(error),
        };
        let recognized = match outcome {
            RecognitionOutcome::Complete(result) => result?,
            RecognitionOutcome::Invalidated(error) => {
                self.inner.playing_track_identifications.cancel(&request_id);
                // SongRec cancellation kills and reaps its child before the
                // helper returns. Keep the task detached if this defensive
                // bound expires so its registry guard still tracks cleanup.
                let _ = tokio::time::timeout(Duration::from_secs(3), &mut recognition).await;
                return Err(error);
            }
            RecognitionOutcome::TimedOut => {
                self.inner.playing_track_identifications.cancel(&request_id);
                let _ = tokio::time::timeout(Duration::from_secs(3), &mut recognition).await;
                return Err("Track identification exceeded 60 seconds".into());
            }
        };
        if *result_cancelled.borrow()
            || !native_identification_matches(
                &self.snapshot_without_queue(),
                &track_id,
                &entry_id,
                generation,
                seek_serial,
                &chapter,
            )
        {
            return Err("Playback changed during identification".into());
        }
        let (recognized, capture_timing) = recognized.unzip();
        let title = recognized
            .as_ref()
            .map(|result| result.metadata.title.clone());
        let artwork_url = recognized
            .as_ref()
            .and_then(|result| result.metadata.artwork_url.clone());
        let artist = recognized
            .as_ref()
            .map(|result| result.metadata.artist.clone());
        let timing_identity_matches = recognized.as_ref().is_some_and(|result| {
            crate::recording_match::recognition_matches_timing_identity(
                self.inner.store.as_ref(),
                &requested_track,
                &result.metadata.title,
                &result.metadata.artist,
            )
        });
        let timing_anchor = recognized
            .as_ref()
            .and_then(|result| result.match_offset_seconds)
            .zip(capture_timing.and_then(|timing| timing.start_position_seconds))
            .filter(|_| timing_anchor_eligible && timing_identity_matches)
            .map(
                |(shazam_match_offset_seconds, captured_audio_start_position_seconds)| {
                    RecognitionTimingAnchor {
                        shazam_match_offset_seconds,
                        captured_audio_start_position_seconds,
                        captured_audio_duration_seconds: capture_timing
                            .map(|timing| timing.duration_seconds)
                            .unwrap_or_default(),
                        source_generation: generation,
                        seek_serial,
                        track_id: track_id.clone(),
                        entry_id: entry_id.clone(),
                    }
                },
            );
        // The detection cache is keyed to the source resource, so every
        // observation must use source-absolute time even when this queue item
        // is an explicit range whose PCM tap clock starts at zero.
        let range_offset = requested_track
            .playback_range
            .as_ref()
            .map_or(0.0, |range| range.start_time);
        let captured_audio_interval = capture_timing.and_then(|timing| {
            timing.start_position_seconds.and_then(|start| {
                source_absolute_detection_interval(start, timing.duration_seconds, range_offset)
            })
        });
        if let (Some(resource), Some(metadata), Some((start, duration))) = (
            detection_resource.as_ref(),
            recognized.as_ref().map(|result| &result.metadata),
            captured_audio_interval,
        ) {
            // The same owner fence used for the returned identification also
            // protects this optional cache write. Disabled resources are no-op.
            let candidate_still_current = {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.generation == generation
                    && same_detection_candidate(state.candidate.as_ref(), active_candidate.as_ref())
            };
            if candidate_still_current
                && native_identification_matches(
                    &self.snapshot_without_queue(),
                    &track_id,
                    &entry_id,
                    generation,
                    seek_serial,
                    &chapter,
                )
            {
                let _ = crate::playing_track_detections::record_if_enabled(
                    self.inner.store.as_ref(),
                    resource,
                    metadata,
                    Some(&track_id),
                    start,
                    duration,
                );
            }
        }
        let note = if title.is_some() {
            "SongRec identified the current audio; catalog identity is unverified.".into()
        } else {
            "SongRec did not identify a track in this audio excerpt.".into()
        };
        Ok(PlayingTrackIdentification {
            track_id,
            title,
            artist,
            artwork_url,
            note,
            timing_anchor,
            detection_metadata: recognized.map(|result| result.metadata),
            captured_audio_interval,
            detection_source_identity: active_source_identity,
            detection_source_epoch: None,
        })
    }

    pub fn cancel_playing_track_identification(&self, caller_id: &str) -> bool {
        self.inner.playing_track_identifications.cancel(caller_id)
    }

    /// Stop admitting new native identification work and give active jobs time
    /// to reap any SongRec child and remove temporary PCM files before exit.
    pub async fn shutdown_identification_for_exit(&self) -> Result<(), String> {
        let mut drained = self
            .inner
            .playing_track_identifications
            .close_and_cancel_all();
        let wait = async {
            loop {
                if *drained.borrow_and_update() == 0 {
                    return Ok(());
                }
                drained
                    .changed()
                    .await
                    .map_err(|_| "Identification shutdown state was lost".to_owned())?;
            }
        };
        tokio::time::timeout(Duration::from_secs(4), wait)
            .await
            .map_err(|_| {
                "Timed out waiting for audio identification cleanup during shutdown".to_owned()
            })?
    }

    pub async fn execute_ids(
        &self,
        action: &str,
        track_ids: Vec<String>,
        mode: Option<String>,
        position: Option<f64>,
        volume: Option<f64>,
    ) -> Result<Value, String> {
        let command = match action {
            "play" | "queue" if !track_ids.is_empty() => {
                if track_ids.len() > 500 {
                    return Err("At most 500 tracks can be requested".into());
                }
                let tracks = track_ids
                    .iter()
                    .map(|id| {
                        self.inner
                            .store
                            .get_track(id)?
                            .ok_or_else(|| format!("Track not found: {id}"))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                json!({"action":if action=="play"{"play"}else{"queue"},"tracks":tracks,"mode":mode.unwrap_or_else(||"append".into()),"index":0})
            }
            "resume" | "play" => json!({"action":"resume"}),
            "pause" | "stop" | "next" | "previous" => json!({"action":action}),
            "seek" => json!({"action":"seek","position":position.ok_or("Position missing")?}),
            "volume" => json!({"action":"volume","volume":volume.ok_or("Volume missing")?}),
            "shuffle" => json!({"action":"shuffle","enabled":mode.as_deref()==Some("true")}),
            "repeat" => json!({"action":"repeat","mode":mode.unwrap_or_else(||"off".into())}),
            "openUri" => {
                let raw = mode.ok_or("Source URL missing")?;
                crate::acquisition::validate_source_url(&raw, "direct")?;
                let track = Track {
                    id: format!("uri:{}", uuid::Uuid::new_v4()),
                    provider: if raw.contains("soundcloud.com") {
                        "soundcloud".into()
                    } else {
                        "youtube".into()
                    },
                    source_url: raw.clone(),
                    title: raw,
                    ..Default::default()
                };
                json!({"action":"play","tracks":[track],"index":0})
            }
            _ => return Err("Unsupported playback action".into()),
        };
        let should_wait = matches!(action, "play" | "resume" | "next" | "previous" | "openUri");
        let snapshot = self.execute(command).await?;
        if !should_wait {
            return Ok(self.snapshot_page(0, 50));
        }
        let generation = snapshot["generation"].as_u64().unwrap_or(0);
        let mut updates = self.subscribe();
        match tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let value = updates.borrow().clone();
                if value["generation"].as_u64() != Some(generation) {
                    return Err("Playback request was superseded".into());
                }
                match value["status"].as_str() {
                    Some("playing") => return Ok(self.snapshot_page(0, 50)),
                    Some("failed") => {
                        return Err(value["playerMessage"]
                            .as_str()
                            .unwrap_or("Playback failed")
                            .to_owned());
                    }
                    Some("paused" | "idle" | "ended") => {
                        return Err("Playback did not start".into());
                    }
                    _ => {}
                }
                updates
                    .changed()
                    .await
                    .map_err(|_| "Player stopped".to_owned())?;
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                self.cancel_timed_out_generation(generation);
                Err("Playback timed out".into())
            }
        }
    }
}
fn positive(value: Option<&Value>, default: f64) -> Result<f64, String> {
    let value = match value {
        None => default,
        Some(value) => value
            .as_f64()
            .ok_or_else(|| "Playback value must be finite and nonnegative".to_owned())?,
    };
    if !value.is_finite() || value < 0. {
        return Err("Playback value must be finite and nonnegative".into());
    }
    Ok(value)
}
fn same_radio_song(current: &Value, identified: &crate::radio::RadioTrackMetadata) -> bool {
    let metadata = &current["metadata"];
    let normalize = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    metadata["stationId"].as_str() == Some(identified.station_id.as_str())
        && current["artist"]
            .as_str()
            .is_some_and(|artist| normalize(artist) == normalize(&identified.artist))
        && current["title"]
            .as_str()
            .is_some_and(|title| normalize(title) == normalize(&identified.title))
}
fn native_identification_matches(
    snapshot: &Value,
    track_id: &str,
    entry_id: &str,
    generation: u64,
    seek_serial: u64,
    chapter: &Value,
) -> bool {
    snapshot["status"].as_str() == Some("playing")
        && snapshot["trackId"].as_str() == Some(track_id)
        && snapshot["entryId"].as_str() == Some(entry_id)
        && snapshot["generation"].as_u64() == Some(generation)
        && snapshot["seekSerial"].as_u64() == Some(seek_serial)
        && snapshot["chapter"] == *chapter
}

fn same_detection_candidate(current: Option<&Candidate>, expected: Option<&Candidate>) -> bool {
    match (current, expected) {
        (Some(current), Some(expected)) => {
            current.provider == expected.provider && current.id == expected.id
        }
        (None, None) => true,
        _ => false,
    }
}

fn native_detection_source_identity(
    active_local_asset_id: Option<&str>,
    candidate: Option<&Candidate>,
) -> Option<crate::playing_track_detections::SourceIdentity> {
    if let Some(asset_id) = active_local_asset_id {
        return crate::playing_track_detections::SourceIdentity::new("local", asset_id);
    }
    candidate.and_then(|candidate| {
        crate::playing_track_detections::SourceIdentity::new(&candidate.provider, &candidate.id)
            .map(|identity| identity.with_duration_seconds(candidate.duration_seconds))
    })
}

fn same_detection_candidate_identity(
    current: Option<&Candidate>,
    expected: Option<(&str, &str)>,
) -> bool {
    match (current, expected) {
        (Some(current), Some((provider, id))) => current.provider == provider && current.id == id,
        (None, None) => true,
        _ => false,
    }
}

/// Choose the largest safe excerpt supported by the tap. For ordinary tracks,
/// chapter elapsed time comes from the playback snapshot and may trail the live
/// PCM edge slightly; this bounds the requested excerpt but is not a sample-
/// exact chapter boundary guarantee. Explicit playback ranges remain tied to
/// their generation and use the full tap window.
fn native_identification_capture_seconds(
    has_explicit_playback_range: bool,
    natural_chapter_start_seconds: Option<f64>,
    playback_position_seconds: f64,
) -> Result<f64, String> {
    const MIN_CAPTURE_SECONDS: f64 = 3.0;
    const MAX_CAPTURE_SECONDS: f64 = 12.0;
    if has_explicit_playback_range {
        return Ok(MAX_CAPTURE_SECONDS);
    }
    let Some(start) = natural_chapter_start_seconds else {
        return Ok(MAX_CAPTURE_SECONDS);
    };
    let elapsed = (playback_position_seconds - start).max(0.0);
    if elapsed < MIN_CAPTURE_SECONDS {
        return Err(
            "This chapter is too short to identify yet; wait until at least three seconds have played".into(),
        );
    }
    Ok(elapsed.min(MAX_CAPTURE_SECONDS))
}

fn source_absolute_detection_interval(
    start: f64,
    duration: f64,
    range_offset: f64,
) -> Option<(f64, f64)> {
    let absolute_start = start + range_offset;
    (start.is_finite()
        && start >= 0.0
        && duration.is_finite()
        && duration > 0.0
        && range_offset.is_finite()
        && range_offset >= 0.0
        && absolute_start.is_finite())
    .then_some((absolute_start, duration))
}

fn native_radio_identification_matches(
    snapshot: &Value,
    station_id: &str,
    generation: u64,
    seek_serial: u64,
) -> bool {
    snapshot["status"].as_str() == Some("playing")
        && snapshot["generation"].as_u64() == Some(generation)
        && snapshot["seekSerial"].as_u64() == Some(seek_serial)
        && snapshot["trackId"].as_str() == Some(format!("radio:{station_id}").as_str())
}

async fn wait_for_native_radio_identification_invalidation(
    mut cancelled: watch::Receiver<bool>,
    mut updates: watch::Receiver<Value>,
    station_id: String,
    generation: u64,
    seek_serial: u64,
) -> String {
    loop {
        if *cancelled.borrow() {
            return "Audio identification was cancelled".into();
        }
        if !native_radio_identification_matches(
            &updates.borrow_and_update(),
            &station_id,
            generation,
            seek_serial,
        ) {
            return "Playback changed during identification".into();
        }
        tokio::select! {
            changed = cancelled.changed() => {
                if changed.is_err() { std::future::pending::<()>().await; }
            }
            changed = updates.changed() => {
                if changed.is_err() { return "Playback changed during identification".into(); }
            }
        }
    }
}

async fn wait_for_native_identification_invalidation(
    mut cancelled: watch::Receiver<bool>,
    mut updates: watch::Receiver<Value>,
    track_id: String,
    entry_id: String,
    generation: u64,
    seek_serial: u64,
    chapter: Value,
) -> String {
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            changed = cancelled.changed(), if cancellation_open => {
                if *cancelled.borrow() {
                    return "Audio identification was cancelled".into();
                }
                // The recognition worker owns the registry job. Successful
                // completion drops its sender before the JoinHandle is selected.
                // False closure is completion, not an explicit cancellation.
                if changed.is_err() {
                    cancellation_open = false;
                }
            }
            changed = updates.changed() => {
                if changed.is_err()
                    || !native_identification_matches(
                        &updates.borrow_and_update(),
                        &track_id,
                        &entry_id,
                        generation,
                        seek_serial,
                        &chapter,
                    )
                {
                    return "Playback changed during identification".into();
                }
            }
        }
    }
}

struct TemporaryPcmFile(std::path::PathBuf);
impl Drop for TemporaryPcmFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn clean_error(raw: &str) -> String {
    safe_failure_text(raw, 250)
}

fn merge_persisted_queue(saved: &mut Value, queue: &Value) -> Result<(), String> {
    if !saved.is_object() || !queue.is_object() {
        return Err("Saved playback checkpoint is invalid".into());
    }
    // Legacy pairs have neither revision. Once either component uses occurrence
    // identity, both revisions must match before any session position is reused.
    let has_identity = queue.get("entryIds").is_some()
        || queue.get("queueRevision").is_some()
        || saved.get("queueRevision").is_some()
        || saved.get("currentEntryId").is_some();
    if has_identity {
        let revision = queue["queueRevision"]
            .as_u64()
            .ok_or("Saved playback queue revision is invalid")?;
        if saved["queueRevision"].as_u64() != Some(revision) {
            return Err("Saved playback queue and session revisions do not match".into());
        }
        saved["entryIds"] = queue["entryIds"].clone();
    }
    saved["tracks"] = queue["tracks"].clone();
    if let Some(contexts) = queue.get("queueLoudnessContexts") {
        saved["queueLoudnessContexts"] = contexts.clone();
    }
    Ok(())
}

/// Validate an engine-switch checkpoint without mutating the running player.
/// Explicit occurrence IDs are mandatory; legacy ID regeneration is not safe here.
fn engine_switch_checkpoint(saved: &Value) -> Result<(Value, Value), String> {
    let tracks = saved["tracks"]
        .as_array()
        .ok_or("Engine-switch queue is missing")?;
    let entries = saved["entryIds"]
        .as_array()
        .ok_or("Engine-switch occurrence IDs are missing")?;
    if entries.len() != tracks.len()
        || entries.iter().any(|id| {
            id.as_str().is_none_or(|id| {
                id.is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            })
        })
        || saved["position"]
            .as_f64()
            .is_none_or(|position| !position.is_finite() || position < 0.)
        || saved["volume"]
            .as_f64()
            .is_none_or(|volume| !volume.is_finite() || !(0. ..=1.).contains(&volume))
        || !matches!(saved["repeat"].as_str(), Some("off" | "all" | "one"))
        || saved["shuffle"].as_bool().is_none()
    {
        return Err("Engine-switch checkpoint is invalid".into());
    }
    let mut restored = State::default();
    restore_state(&mut restored, saved)?;
    let queue = json!({"tracks":restored.queue,"entryIds":restored.entries,"queueRevision":restored.queue_revision,"queueLoudnessContexts":restored.loudness_queue_contexts});
    let session = json!({"index":restored.index,"currentEntryId":restored.entries.get(restored.index),"queueRevision":restored.queue_revision,"position":restored.position,"volume":restored.volume,"repeat":restored.repeat,"shuffle":restored.shuffle,"finished":false});
    Ok((queue, session))
}

fn restore_state(state: &mut State, saved: &Value) -> Result<(), String> {
    let tracks = serde_json::from_value::<Vec<Track>>(saved["tracks"].clone())
        .map_err(|_| "Saved playback queue is invalid")?;
    if tracks.len() > 10000 {
        return Err("Saved playback queue is too large".into());
    }
    let has_identity = saved.get("entryIds").is_some()
        || saved.get("queueRevision").is_some()
        || saved.get("currentEntryId").is_some();
    let (entries, queue_revision, index) = if has_identity {
        let entries: Vec<String> = serde_json::from_value(saved["entryIds"].clone())
            .map_err(|_| "Saved playback occurrence IDs are invalid")?;
        if entries.len() != tracks.len()
            || entries.iter().any(|entry| entry.trim().is_empty())
            || entries.iter().collect::<HashSet<_>>().len() != entries.len()
        {
            return Err("Saved playback occurrence IDs must be unique and match the queue".into());
        }
        let revision = saved["queueRevision"]
            .as_u64()
            .ok_or("Saved playback queue revision is invalid")?;
        let index = if tracks.is_empty() {
            if !saved["currentEntryId"].is_null() {
                return Err("Saved playback current occurrence is invalid".into());
            }
            0
        } else {
            let current = saved["currentEntryId"]
                .as_str()
                .ok_or("Saved playback current occurrence is missing")?;
            entries
                .iter()
                .position(|entry| entry == current)
                .ok_or("Saved playback current occurrence is not in the queue")?
        };
        (entries, Some(revision), index)
    } else {
        let entries = tracks
            .iter()
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        let index =
            (saved["index"].as_u64().unwrap_or(0) as usize).min(tracks.len().saturating_sub(1));
        (entries, None, index)
    };
    let mut loudness_queue_contexts = saved
        .get("queueLoudnessContexts")
        .map(|value| {
            serde_json::from_value::<HashMap<String, crate::loudness::LoudnessResourceIdentity>>(
                value.clone(),
            )
            .map_err(|_| "Saved album loudness queue contexts are invalid")
        })
        .transpose()?
        .unwrap_or_default();
    loudness_queue_contexts.retain(|entry_id, resource| {
        let Some(index) = entries.iter().position(|entry| entry == entry_id) else {
            return false;
        };
        resource.validate().is_ok()
            && resource.members.iter().any(|member| {
                loudness_album_resource_matches_track(resource, &tracks[index], member)
            })
    });
    // Validate fallible fields before changing the live queue.
    let position = positive(saved.get("position"), 0.)?;
    let volume = positive(saved.get("volume"), 0.8)?.min(1.);
    state.queue = tracks;
    state.entries = entries;
    state.loudness_queue_contexts = loudness_queue_contexts;
    mark_queue_changed(state);
    if let Some(revision) = queue_revision {
        state.queue_revision = revision;
    }
    state.persisted_queue_revision = None;
    state.persisted_session = None;
    state.published_queue_revision = None;
    state.index = index;
    state.position = position;
    state.volume = volume;
    state.repeat = saved["repeat"]
        .as_str()
        .filter(|s| ["one", "all", "off"].contains(s))
        .unwrap_or("off")
        .into();
    state.shuffle = saved["shuffle"].as_bool().unwrap_or(false);
    state.status = if state.queue.is_empty() {
        "idle"
    } else if saved["finished"].as_bool().unwrap_or(false) {
        "ended"
    } else {
        "paused"
    }
    .into();
    if state.status == "ended" {
        state.message = "Queue finished".into();
    }
    Ok(())
}
fn queue_next(
    len: usize,
    index: usize,
    repeat: &str,
    direction: i32,
    ended: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if ended && repeat == "one" {
        return Some(index);
    }
    if direction < 0 {
        return Some(index.saturating_sub(1));
    }
    if index + 1 < len {
        Some(index + 1)
    } else if repeat == "all" {
        Some(0)
    } else {
        None
    }
}

fn is_natural_queue_end(len: usize, index: usize, repeat: &str, ended: bool) -> bool {
    ended && repeat == "off" && len > 0 && index + 1 >= len
}

fn sort_upcoming(
    state: &mut State,
    field: &str,
    descending: bool,
    collator: &icu_collator::CollatorBorrowed<'static>,
) {
    let start = state.index.saturating_add(1).min(state.queue.len());
    if state.queue.len().saturating_sub(start) < 2 || field == "sourceOrder" {
        return;
    }
    let mut upcoming = (start..state.queue.len())
        .map(|index| (state.queue[index].clone(), state.entries[index].clone()))
        .collect::<Vec<_>>();
    upcoming.sort_by(|(left, _), (right, _)| {
        compare_track_sort_values(left, right, field, descending, collator)
    });
    for (index, (track, entry)) in (start..state.queue.len()).zip(upcoming) {
        state.queue[index] = track;
        state.entries[index] = entry;
    }
    mark_queue_changed(state);
}

fn compare_track_sort_values(
    left: &Track,
    right: &Track,
    field: &str,
    descending: bool,
    collator: &icu_collator::CollatorBorrowed<'static>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let left_value = track_sort_value(left, field);
    let right_value = track_sort_value(right, field);
    match (left_value, right_value) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(left), Some(right)) => {
            let ordering = match (left, right) {
                (TrackSortValue::Number(a), TrackSortValue::Number(b)) => a.total_cmp(&b),
                (TrackSortValue::Text(a), TrackSortValue::Text(b)) => collator.compare(&a, &b),
                _ => Ordering::Equal,
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
    }
}

enum TrackSortValue {
    Number(f64),
    Text(String),
}

fn track_sort_value(track: &Track, field: &str) -> Option<TrackSortValue> {
    let number = |value: Option<f64>| {
        value
            .filter(|value| value.is_finite() && *value >= 0.)
            .map(TrackSortValue::Number)
    };
    let text = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| TrackSortValue::Text(value.to_lowercase()))
    };
    let count = |keys: &[&str]| {
        let top = keys.iter().find_map(|key| track.provenance.get(*key));
        let remote = track.provenance.get("remoteCatalog");
        top.and_then(numeric_sort_value).or_else(|| {
            keys.iter()
                .find_map(|key| remote.and_then(|value| value.get(*key)))
                .and_then(numeric_sort_value)
        })
    };
    match field {
        "title" => text(Some(&track.title)),
        "artist" => text(Some(&track.artist)),
        "album" => text(track.album.as_deref()),
        "duration" => {
            let seconds = track
                .playback_range
                .as_ref()
                .map(|range| range.end_time - range.start_time)
                .or(track.duration_seconds)?;
            (seconds.is_finite() && seconds > 0.).then(|| TrackSortValue::Number(seconds.floor()))
        }
        "releaseDate" => date_sort_value(
            track
                .release_date
                .as_deref()
                .or(track.recording_date.as_deref()),
        ),
        "dateAdded" => date_sort_value(
            ["dateAdded", "addedAt", "importedAt", "createdAt"]
                .iter()
                .find_map(|key| {
                    track
                        .provenance
                        .get(*key)
                        .and_then(serde_json::Value::as_str)
                }),
        ),
        "bpm" => number(track.bpm.filter(|value| *value > 0.)),
        "plays" => count(&["plays", "playCount", "play_count"]),
        "views" => count(&["views", "viewCount", "view_count"]),
        "key" => camelot_sort_value(track.key.as_deref()?),
        _ => None,
    }
}

fn numeric_sort_value(value: &Value) -> Option<TrackSortValue> {
    crate::track_sort_text::parse_sort_count(value).map(TrackSortValue::Number)
}

fn date_sort_value(value: Option<&str>) -> Option<TrackSortValue> {
    crate::track_sort_text::parse_sort_date(value?)
        .map(|timestamp| TrackSortValue::Number(timestamp as f64))
}

fn camelot_sort_value(value: &str) -> Option<TrackSortValue> {
    crate::track_sort_text::parse_sort_key(value)
        .map(|sort_value| TrackSortValue::Number(sort_value as f64))
}

fn should_skip_failed_track(message: &str) -> bool {
    crate::playback_failure::should_skip_recording(&crate::playback_failure::classify(message))
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RecoveryTrigger {
    OutputFailed,
    PrematureEnded,
    SourceResolution,
    SteadyBufferingTimeout,
}

// Retain the failing decoder facts across recovery and successful audio.
fn record_recovery_facts(state: &mut State, trigger: RecoveryTrigger) {
    let trigger = match trigger {
        RecoveryTrigger::OutputFailed => "output_failed",
        RecoveryTrigger::PrematureEnded => "premature_ended",
        RecoveryTrigger::SourceResolution => "source_resolution",
        RecoveryTrigger::SteadyBufferingTimeout => "steady_buffering_timeout",
    };
    let operation = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|v| {
                v.len() <= 80
                    && !v.is_empty()
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            })
            .map(str::to_owned)
    };
    const MAX_COUNTER: u64 = (1u64 << 53) - 1;
    let decoder = state.decoder_diagnostics.as_ref().map(|d| json!({
        "decoderKind": match d.decoder_kind.as_str() { "rodio" => "rodio", "ffmpeg" => "ffmpeg", "experimental-opus" => "experimental_opus", _ => "other" },
        "readerReused":d.reader_reused,"decoderReused":d.decoder_reused,
        "bytesRead":d.bytes_read.min(MAX_COUNTER),"bytesReceived":d.bytes_received.min(MAX_COUNTER),
        "inputContentLength":d.input_content_length.map(|n|n.min(MAX_COUNTER)),
        "contentRangeTotal":d.content_range_total.map(|n|n.min(MAX_COUNTER)),
        "countersCapped":d.bytes_read>MAX_COUNTER||d.bytes_received>MAX_COUNTER||d.input_content_length.is_some_and(|n|n>MAX_COUNTER)||d.content_range_total.is_some_and(|n|n>MAX_COUNTER)
    }));
    state.last_recovery = Some(json!({"version":1,"trigger":trigger,
        "generation":state.generation,"seekSerial":state.seek_serial,
        "operationId":operation(&state.operation_id),"seekOperationId":operation(&state.seek_operation_id),
        "recoveryAttempt":state.recovery.saturating_add(1),
        "positionSeconds":state.position.is_finite().then_some(state.position.max(0.)),
        "decoderEvidence":"last_started_snapshot","decoder":decoder}));
}

fn can_retry_failed_stream(state: &State, message: &str) -> bool {
    can_retry_failed_stream_at_stage(state, message, false)
}
fn can_retry_failed_stream_at_stage(state: &State, message: &str, output_failed: bool) -> bool {
    let failure = crate::playback_failure::classify(message);
    let startup_transport = output_failed
        && message.contains("Native HTTP")
        && failure.retryable
        && matches!(
            failure.scope,
            crate::playback_failure::FailureScope::Source
                | crate::playback_failure::FailureScope::Global
        )
        && Instant::now() < state.request_started + state.startup_budget;
    (state.first_audio_ms.is_some() || startup_transport)
        && state.recovery < 2
        && state
            .recovery_deadline
            .is_none_or(|deadline| Instant::now() < deadline)
        && state
            .seek_requested
            .is_none_or(|started| Instant::now() < started + state.startup_budget)
        && (state.candidate.is_some() || state.station.is_some())
        && !message.to_ascii_lowercase().contains("output device")
        && !message.contains("Cancelled")
}

fn claim_recovery_failure_generation(state: &mut State, generation: u64) -> bool {
    if state.generation != generation || state.recovery_failure_generation == Some(generation) {
        return false;
    }
    // Multiple watchdog/decoder callbacks can converge on one source failure.
    state.recovery_failure_generation = Some(generation);
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecurringStallDecision {
    NoRepeatedStall,
    ReplaceSource,
    Exhausted,
}

fn record_recurring_transport_stall(state: &mut State, now: Instant) -> RecurringStallDecision {
    if state.station.is_some() || state.seek_requested.is_some() || state.recovery >= 2 {
        return RecurringStallDecision::NoRepeatedStall;
    }
    let (Some(track), Some(candidate)) = (state.track(), state.candidate.as_ref()) else {
        return RecurringStallDecision::NoRepeatedStall;
    };
    if candidate.provider.is_empty() || candidate.id.is_empty() {
        return RecurringStallDecision::NoRepeatedStall;
    }
    let track_id = track.id.clone();
    let candidate_key = format!("{}:{}", candidate.provider, candidate.id);
    let repeated = state
        .recent_stall
        .as_ref()
        .is_some_and(|(old_track, old_candidate, at)| {
            old_track == &track_id
                && old_candidate == &candidate_key
                && now >= *at
                && now.duration_since(*at) <= RECURRING_STALL_WINDOW
        });
    if repeated {
        state.recent_stall = None;
        return if state.recurring_source_swaps < MAX_RECURRING_SOURCE_SWAPS {
            RecurringStallDecision::ReplaceSource
        } else {
            RecurringStallDecision::Exhausted
        };
    }
    state.recent_stall = Some((track_id, candidate_key, now));
    RecurringStallDecision::NoRepeatedStall
}

fn remember_failed_recurring_candidate(state: &mut State, candidate: &Candidate) {
    if candidate.provider.is_empty() || candidate.id.is_empty() {
        return;
    }
    let key = (candidate.provider.clone(), candidate.id.clone());
    state
        .failed_recurring_candidates
        .retain(|entry| entry != &key);
    state.failed_recurring_candidates.push(key);
    if state.failed_recurring_candidates.len() > MAX_RECURRING_SOURCE_SWAPS as usize {
        state.failed_recurring_candidates.remove(0);
    }
}

fn failure_counts_as_native_transport_stall(trigger: RecoveryTrigger, message: &str) -> bool {
    if matches!(
        trigger,
        RecoveryTrigger::SteadyBufferingTimeout | RecoveryTrigger::PrematureEnded
    ) {
        return true;
    }
    if trigger != RecoveryTrigger::OutputFailed {
        return false;
    }
    let failure = crate::playback_failure::classify(message);
    failure.retryable
        && matches!(
            failure.category,
            crate::playback_failure::FailureCategory::Network
                | crate::playback_failure::FailureCategory::RateLimited
                | crate::playback_failure::FailureCategory::SourceAccess
                | crate::playback_failure::FailureCategory::SourceUnavailable
        )
}

fn has_native_stall_history(state: &State) -> bool {
    state.recent_stall.is_some() || state.recurring_source_swaps > 0
}

fn clear_native_stall_history(state: &mut State) {
    state.recent_stall = None;
    state.recurring_source_swaps = 0;
    state.failed_recurring_candidates.clear();
}

fn native_recovery_output_is_healthy(
    since: Option<(Instant, f64, u64)>,
    now: Instant,
    position: f64,
    seek_serial: u64,
) -> bool {
    since.is_some_and(|(started, initial_position, initial_seek_serial)| {
        initial_seek_serial == seek_serial
            && now >= started
            && now.duration_since(started) >= RECOVERY_HEALTHY_RESET
            && position.is_finite()
            && initial_position.is_finite()
            && position - initial_position >= RECOVERY_HEALTHY_RESET.as_secs_f64()
    })
}

fn recovery_can_replace_source(
    attempt: u8,
    explicit: bool,
    track: &Track,
    settings: &crate::models::Settings,
) -> bool {
    attempt >= 2
        && !explicit
        && !crate::playback_sources::source_resolution_is_pinned(track, false, settings, true)
}

fn recovery_should_replace_source(
    attempt: u8,
    explicit: bool,
    track: &Track,
    settings: &crate::models::Settings,
    recurring_stall: bool,
    source_swaps: u8,
) -> bool {
    (recovery_can_replace_source(attempt, explicit, track, settings)
        || recurring_stall && recovery_can_replace_source(2, explicit, track, settings))
        && source_swaps < MAX_RECURRING_SOURCE_SWAPS
}

fn recurring_stall_requires_terminal_failure(
    decision: RecurringStallDecision,
    track: &Track,
    explicit: bool,
    settings: Option<&crate::models::Settings>,
    is_radio: bool,
    seek_pending: bool,
) -> bool {
    decision == RecurringStallDecision::ReplaceSource
        && !is_radio
        && !seek_pending
        && settings
            .is_some_and(|settings| !recovery_can_replace_source(2, explicit, track, settings))
}

fn record_seek_context(
    state: &mut State,
    id: Option<String>,
    received: Instant,
    client_started_at_ms: Option<u64>,
    click_to_ipc_ms: Option<u64>,
    command_bytes: Option<usize>,
) {
    state.last_buffering = None;
    state.seek_operation_id = id;
    state.seek_operation_received_at = Some(received);
    state.seek_client_started_at_ms = client_started_at_ms;
    state.seek_click_to_ipc_ms = click_to_ipc_ms;
    state.seek_command_bytes = command_bytes;
}

fn deferred_seek_status(status: &str, output_ready: bool) -> bool {
    matches!(status, "idle" | "ended") || status == "paused" && !output_ready
}

fn defer_paused_seek(state: &mut State) {
    state.status = "paused".into();
    state.natural_queue_end = false;
    state.message = "Paused".into();
    state.pending_seek_generation = Some(state.generation);
    state.seek_requested = None;
    state.seek_first_audio_ms = None;
    state.seek_operation_first_audio_ms = None;
}

fn reset_seek_metrics_for_start(state: &mut State, resume: bool) {
    if !resume || state.seek_requested.is_none() {
        state.seek_requested = None;
        state.seek_first_audio_ms = None;
        state.seek_operation_first_audio_ms = None;
    }
}

fn restart_deadline(state: &State) -> Instant {
    state
        .seek_requested
        .map(|started| started + state.startup_budget)
        .or(state.recovery_deadline)
        .unwrap_or_else(|| {
            if state.first_audio_ms.is_some() {
                Instant::now() + state.startup_budget
            } else {
                state.request_started + state.startup_budget
            }
        })
}

// Paused seeks defer decoder latency until Resume, while receipt latency still
// includes the user's paused interval. Only the matching generation may arm it.
fn arm_resumed_seek_metrics(state: &mut State, resumed_at: Instant) {
    if state.pending_seek_generation == Some(state.generation) && state.seek_requested.is_none() {
        state.seek_requested = Some(resumed_at);
    }
}

fn note_audio_started(state: &mut State) {
    state.last_local_storage_failure = None;
    state.failed_occurrence_ids.clear();
    state.last_source_failure = None;
    state.decoder_diagnostics = None;
    if let Some(started) = state.seek_requested.take() {
        let consumed_at = Instant::now();
        state.seek_first_audio_ms = Some(consumed_at.duration_since(started).as_millis() as u64);
        state.seek_operation_first_audio_ms = state
            .seek_operation_received_at
            .map(|received| consumed_at.duration_since(received).as_millis() as u64);
    }
}

fn native_operation_id() -> String {
    format!("native-{}", uuid::Uuid::new_v4())
}

fn native_decoder_diagnostic_payload(
    context: &NativeDecoderDiagnosticContext,
    phase: &str,
    outcome: Option<&str>,
    category: Option<&str>,
) -> Value {
    let mut details = json!({"generation": context.generation});
    if phase == "decoder_end" {
        details["durationMs"] = json!(context.decoder_started.elapsed().as_millis());
    }
    if let Some(outcome) = outcome {
        details["outcome"] = json!(outcome);
    }
    if let Some(category) = category {
        details["category"] = json!(category);
    }
    json!({
        "resolutionId":context.resolution_id,
        "nativeOperationId":context.native_operation_id,
        "atUnixMs":chrono::Utc::now().timestamp_millis(),
        "elapsedMs":context.request_started.elapsed().as_millis(),
        "phase":phase,
        "provider":context.provider,
        "details":details,
    })
}

fn record_native_decoder_diagnostic(
    context: &NativeDecoderDiagnosticContext,
    phase: &str,
    outcome: Option<&str>,
    category: Option<&str>,
) {
    if crate::playback_diagnostics::is_enabled() {
        crate::playback_diagnostics::record_safe_event(native_decoder_diagnostic_payload(
            context, phase, outcome, category,
        ));
    }
}

fn ensure_command_operation_id(id: Option<String>, action: &str) -> Option<String> {
    match action {
        "play" | "select" | "seek" | "next" | "previous" => {
            Some(id.unwrap_or_else(native_operation_id))
        }
        _ => id,
    }
}

fn parse_operation_id(value: Option<&Value>) -> Result<Option<String>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let id = value
        .as_str()
        .ok_or("Playback operation ID must be a string")?;
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err("Playback operation ID has invalid format".into());
    }
    Ok(Some(id.to_owned()))
}

fn parse_client_started_at_ms(value: Option<&Value>) -> Result<Option<u64>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let timestamp = value
        .as_f64()
        .filter(|timestamp| timestamp.is_finite() && *timestamp >= 0.)
        .ok_or("Playback clientStartedAtMs must be a nonnegative timestamp")?;
    Ok(Some(timestamp.min(u64::MAX as f64).round() as u64))
}

fn system_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

fn finish_external_queued_owner(state: &mut State, action: &str) {
    if !crate::queued_playback::native_command_preserves_root(action) {
        state.queued_operation_id = None;
    }
    if !matches!(action, "seek" | "volume") {
        state.queued_intent_epoch = state.queued_intent_epoch.wrapping_add(1);
    }
}
fn begin_operation(
    state: &mut State,
    id: Option<&str>,
    kind: &str,
    received_at: Instant,
    client_started_at_ms: Option<u64>,
    click_to_ipc_ms: Option<u64>,
    command_bytes: Option<usize>,
) {
    state.operation_id = Some(id.map(str::to_owned).unwrap_or_else(native_operation_id));
    state.operation_kind = Some(kind.to_owned());
    state.operation_received_at = Some(received_at);
    state.operation_client_started_at_ms = client_started_at_ms;
    state.operation_click_to_ipc_ms = click_to_ipc_ms;
    state.operation_command_bytes = command_bytes;
    state.operation_source_ready_ms = None;
    state.operation_first_audio_ms = None;
}

fn source_failure_snapshot(track: &Track, reason: &str) -> Value {
    json!({
        "trackId": safe_failure_identity(&track.id),
        "provider": safe_failure_identity(&track.provider),
        "providerId": safe_failure_identity(&track.provider_id),
        "title": safe_failure_text(&track.title, 160),
        "artist": safe_failure_text(&track.artist, 160),
        "version": track.version.as_deref().map(|value| safe_failure_text(value, 120)),
        "durationSeconds": track.duration_seconds,
        "reason": safe_failure_text(reason, 250),
    })
}

pub(crate) fn safe_failure_identity(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > 200
        || value.contains('/')
        || value.contains('\\')
        || value.contains('=')
        || value.contains('?')
        || value.contains('#')
        || ["http:", "https:", "ftp:", "file:"]
            .iter()
            .any(|scheme| value.to_ascii_lowercase().contains(scheme))
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_owned())
}

pub(crate) const MAX_RECENT_TRACK_FAILURES: usize = 64;

pub(crate) fn track_failure_category(reason: &str) -> &'static str {
    match reason {
        crate::playback_sources::SOURCE_NO_RESULTS => return "no_audio_results",
        crate::playback_sources::SOURCE_IDENTITY_REJECTED
        | crate::playback_sources::SOURCE_MISSING_DURATION => {
            return "recording_identity_unverified";
        }
        _ => {}
    }
    let message = reason.to_ascii_lowercase();
    let no_match = "no playable audio match was found";
    if message.strip_prefix(no_match).is_some_and(|tail| {
        tail.is_empty()
            || tail.starts_with(" for ")
            || tail.starts_with('.')
            || tail.starts_with(':')
    }) {
        "no_playable_match"
    } else if matches!(
        message.as_str(),
        "the resolved audio source identity does not match this recording"
            | "source identity changed during audio resolution; select it again"
            | "source identity changed during inspection; select it again"
    ) {
        "source_identity_mismatch"
    } else if message == "this source has no supported audio stream" {
        "unsupported_audio_stream"
    } else {
        "failed_with_reason"
    }
}

fn record_track_failure(state: &mut State, generation: u64, reason: &str, outcome: &str) {
    if state.generation != generation
        || !["resolving", "buffering", "playing"].contains(&state.status.as_str())
        || !should_skip_failed_track(reason)
        || state.cancel.as_ref().is_some_and(|cancel| *cancel.borrow())
    {
        return;
    }
    let (Some(track), Some(entry)) = (state.track(), state.entries.get(state.index)) else {
        return;
    };
    let Some(entry_id) = safe_failure_identity(entry) else {
        return;
    };
    if state
        .recent_track_failures
        .iter()
        .any(|failure| failure["generation"] == generation && failure["entryId"] == entry_id)
    {
        return;
    }
    let mut failure = source_failure_snapshot(track, reason);
    failure["queueRevision"] = json!(state.queue_revision);
    failure["generation"] = json!(generation);
    failure["operationId"] = json!(state
        .queued_operation_id
        .as_deref()
        .or(state.operation_id.as_deref())
        .and_then(safe_failure_identity));
    failure["entryId"] = json!(entry_id);
    failure["outcome"] = json!(outcome);
    failure["category"] = json!(track_failure_category(reason));
    if state.recent_track_failures.len() == MAX_RECENT_TRACK_FAILURES {
        state.recent_track_failures.remove(0);
    }
    state.recent_track_failures.push(failure);
}

pub(crate) fn safe_failure_text(raw: &str, max_chars: usize) -> String {
    let mut redact_next = 0usize;
    raw.split_whitespace()
        .map(|part| {
            if redact_next > 0 {
                redact_next -= 1;
                return "[redacted]".to_owned();
            }
            let lower = part.to_ascii_lowercase();
            let path = part.trim_start_matches(['(', '\"', '\'']);
            if ["http://", "https://", "ftp://", "file://"]
                .iter()
                .any(|scheme| lower.contains(scheme))
                || path.starts_with('/')
                || path.starts_with("\\\\")
                || (path.as_bytes().get(1) == Some(&b':') && path.as_bytes().get(2) == Some(&b'\\'))
            {
                return "[source]".to_owned();
            }
            let keys = [
                "token",
                "access_token",
                "access-token",
                "refresh_token",
                "refresh-token",
                "authorization",
                "password",
                "cookie",
                "signature",
                "sig",
                "api_key",
                "api-key",
                "apikey",
            ];
            if keys.iter().any(|key| {
                lower == *key
                    || lower.starts_with(&format!("{key}="))
                    || lower.starts_with(&format!("{key}:"))
            }) {
                if lower == "authorization" || lower.starts_with("authorization:") {
                    redact_next = 2;
                } else if !lower.contains(['=', ':']) || lower.ends_with(['=', ':']) {
                    redact_next = 1;
                }
                return "[redacted]".to_owned();
            }
            if lower == "bearer" {
                redact_next = 1;
                return "[redacted]".to_owned();
            }
            part.chars().filter(|c| !c.is_control()).collect()
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn reset_failed_resume_diagnostics(state: &mut State) {
    state.recovery_deadline = None;
    state.recovered_audio_since = None;
    state.healthy_output_since = None;
    state.recovery = 0;
    state.first_audio_ms = None;
    state.source_ready_ms = None;
    state.seek_requested = None;
    state.seek_first_audio_ms = None;
    state.seek_operation_first_audio_ms = None;
    state.buffering_count = 0;
    state.request_started = Instant::now();
}

fn ended_prematurely(state: &State) -> bool {
    state.station.is_some()
        || !state.output_ready
        || (state.duration_verified && state.duration > 0. && state.position + 1.5 < state.duration)
}

fn failed_track_next_index(
    state: &mut State,
    generation: u64,
    message: &str,
) -> Result<Option<usize>, ()> {
    if state.generation != generation
        || !["resolving", "buffering", "playing"].contains(&state.status.as_str())
        || !should_skip_failed_track(message)
        || state.cancel.as_ref().is_some_and(|cancel| *cancel.borrow())
        || state.queue.is_empty()
        || state.index >= state.queue.len()
        || state.entries.len() != state.queue.len()
    {
        return Err(());
    }
    failed_track_traversal::next_unfailed_occurrence(
        &state.entries,
        state.index,
        &state.repeat,
        &mut state.failed_occurrence_ids,
    )
}

fn is_cropped_chapter_asset(track: &Track, asset: &crate::models::LocalAsset) -> bool {
    track.provider == "chapter" && asset.track.id == track.id && asset.processing == "excerpt"
}

fn source_bounds_for_asset(
    track: &Track,
    asset: Option<&crate::models::LocalAsset>,
) -> (f64, Option<f64>) {
    source_bounds_for_track(
        track,
        asset.is_some_and(|asset| is_cropped_chapter_asset(track, asset)),
    )
}

fn source_bounds_for_track(track: &Track, cropped_excerpt: bool) -> (f64, Option<f64>) {
    match track.playback_range.as_ref() {
        Some(range) if cropped_excerpt && track.provider == "chapter" => {
            (0., Some(range.end_time - range.start_time))
        }
        Some(range) => (range.start_time, Some(range.end_time)),
        None => (0., None),
    }
}

// A new play request must honor the existing shuffle preference, just as the
// WebView engine does. Keep the selected occurrence and its preceding history.
fn replace_play_queue(state: &mut State, tracks: Vec<Track>, index: usize) {
    state.queue = tracks;
    state.entries = state
        .queue
        .iter()
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect();
    state.index = index;
    if state.shuffle {
        shuffle_upcoming(state);
    }
    mark_queue_changed(state);
}

fn shuffle_upcoming(state: &mut State) {
    let start = state.index + 1;
    for i in (start + 1..state.queue.len()).rev() {
        let bytes = *uuid::Uuid::new_v4().as_bytes();
        let random = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
        let j = start + random % (i - start + 1);
        state.queue.swap(i, j);
        state.entries.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_local_asset_identity_precedes_remote_candidate_for_detection() {
        let candidate = Candidate {
            id: "remote-candidate".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let local =
            native_detection_source_identity(Some("asset:active"), Some(&candidate)).unwrap();
        assert_eq!(local.provider_source_id(), ("local", "asset:active"));

        let remote = native_detection_source_identity(None, Some(&candidate)).unwrap();
        assert_eq!(remote.provider_source_id(), ("youtube", "remote-candidate"));
        let catalog_track = Track {
            id: "musicbrainz:recording-a".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-a".into(),
            asset_id: Some("unselected-downloaded-copy".into()),
            ..Track::default()
        };
        assert!(
            !crate::playing_track_detections::requires_verified_local_hash(
                &catalog_track,
                Some(&remote),
                false,
            )
        );
        assert!(
            crate::playing_track_detections::requires_verified_local_hash(
                &catalog_track,
                Some(&local),
                true,
            )
        );
    }

    #[test]
    fn detection_owner_rejects_a_different_active_candidate_even_when_track_is_same() {
        let candidate = Candidate {
            id: "video-A".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        assert!(same_detection_candidate_identity(
            Some(&candidate),
            Some(("youtube", "video-A"))
        ));
        assert!(!same_detection_candidate_identity(
            Some(&candidate),
            Some(("youtube", "video-B"))
        ));
        assert!(!same_detection_candidate_identity(Some(&candidate), None));
        assert!(same_detection_candidate_identity(None, None));
    }

    fn opus_decode_policy_fixture() -> (crate::native_output::AudioSource, crate::models::LocalAsset)
    {
        let track = Track {
            id: "track-opus".into(),
            ..Track::default()
        };
        let asset = crate::models::LocalAsset {
            id: "asset-opus".into(),
            recording_id: track.id.clone(),
            path: "/music/test.opus".into(),
            hash: "content-hash".into(),
            source_codec: "opus".into(),
            final_codec: "opus".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(48_000),
            channels: Some(2),
            bit_depth: None,
            duration_seconds: Some(20.0),
            size_bytes: Some(1024),
            bitrate_origin: None,
            processing: "original".into(),
            track,
        };
        let source = crate::native_output::AudioSource {
            input: asset.path.clone(),
            ffmpeg: "ffmpeg".into(),
            headers: vec![],
            decoder_hint: None,
            cenc: None,
        };
        (source, asset)
    }

    #[test]
    fn invalid_opus_tags_fall_back_only_to_independent_verified_analysis() {
        let tags = crate::loudness_analysis::LocalTagMeasurements {
            album: None,
            album_artist: None,
            track_replaygain: None,
            album_replaygain: None,
            replaygain_comment_present: true,
            opus_r128_track_gain_db: None,
            opus_r128_album_gain_db: None,
            opus_r128_track_tag_present: true,
            opus_r128_album_tag_present: false,
            opus_output_gain_tag_db: None,
            file_seal: None,
        };
        let resource = crate::loudness::LoudnessResourceIdentity::track(
            "asset-opus",
            "a".repeat(64),
            crate::loudness::AudioRange::WHOLE_FILE,
        );
        let mut record = crate::loudness::LoudnessRecord {
            schema_version: crate::loudness::LOUDNESS_SCHEMA_VERSION,
            resource,
            analyzer_version: "fixture".into(),
            provenance: crate::loudness::MeasurementProvenance::FfmpegAnalysis,
            integrated_lufs: Some(-24.0),
            true_peak_dbtp: Some(-3.0),
            sample_rate: Some(48_000),
            channels: 2,
            measured_at: "2026-10-27T00:00:00Z".into(),
            stereo_mix_true_peak_dbtp: Some(-3.0),
            stereo_mix_integrated_lufs: Some(-24.0),
            stereo_mix_matrix: Some("web-audio-stereo-identity-v1".into()),
            stereo_mix_layout: Some("stereo".into()),
            maximum_member_safe_true_peak_dbtp: None,
            maximum_member_native_safe_true_peak_dbtp: None,
            conservative_channel_sum_bound: false,
            local_file_seals: vec![],
            opus_decoder_proofs: vec![],
        };
        assert!(record.validate().is_ok());
        let selected = local_track_normalization_decision(
            Some(&tags),
            true,
            crate::loudness::AudioRange::WHOLE_FILE,
            Some(-3.0),
            true,
            Some(&record),
            crate::loudness::GainPolicy::default(),
            crate::loudness::NormalizationMode::Track,
            true,
        );
        assert_eq!(
            selected,
            Some((2.0, crate::loudness::MeasurementProvenance::FfmpegAnalysis))
        );
        assert_eq!(
            local_track_normalization_decision(
                Some(&tags),
                true,
                crate::loudness::AudioRange::WHOLE_FILE,
                Some(-3.0),
                true,
                None,
                crate::loudness::GainPolicy::default(),
                crate::loudness::NormalizationMode::Track,
                true,
            ),
            None,
        );
        record.integrated_lufs = None;
        record.stereo_mix_integrated_lufs = None;
        assert_eq!(
            local_track_normalization_decision(
                Some(&tags),
                true,
                crate::loudness::AudioRange::WHOLE_FILE,
                Some(-3.0),
                true,
                Some(&record),
                crate::loudness::GainPolicy::default(),
                crate::loudness::NormalizationMode::Track,
                true,
            ),
            None,
        );
    }

    #[test]
    fn refresh_and_preload_adoption_preserve_the_exact_qualified_opus_decoder() {
        let capability = ActiveOpusDecoderCapability {
            executable_path: if cfg!(windows) {
                std::path::PathBuf::from(r"C:\qualified\ffmpeg.exe")
            } else {
                std::path::PathBuf::from("/qualified/ffmpeg")
            },
            executable_sha256: "a".repeat(64),
        };
        assert!(capability.executable_path.is_absolute());
        let refreshed =
            registered_source_with_opus_capability("/music/track.ogg", Some(&capability));
        assert_eq!(refreshed.ffmpeg, capability.executable_path);
        assert_eq!(
            active_opus_decoder_capability(&refreshed),
            Some(capability.clone()),
        );
        let mut state = State::default();
        state.opus_decoder_capability = Some(capability.clone());
        let active_refresh = registered_source_with_opus_capability(
            "/music/track.ogg",
            state.opus_decoder_capability.as_ref(),
        );
        assert_eq!(
            active_opus_decoder_capability(&active_refresh),
            Some(capability.clone())
        );

        let preload = Preloaded {
            preparation_trace: None,
            generation: 8,
            index: 0,
            entry: "entry".into(),
            candidate: None,
            quality: json!({}),
            duration: Some(20.0),
            steady_http_stream: false,
            offset: 0.0,
            end: None,
            loudness_status: crate::loudness_manager::LoudnessStatus::off(),
            loudness_album_context: None,
            opus_decoder_capability: Some(capability.clone()),
        };
        state.opus_decoder_capability = None;
        adopt_preloaded_opus_capability(&mut state, &preload);
        let adopted = registered_source_with_opus_capability(
            "/music/track.ogg",
            state.opus_decoder_capability.as_ref(),
        );
        assert_eq!(adopted.ffmpeg, capability.executable_path);
        assert_eq!(active_opus_decoder_capability(&adopted), Some(capability));
    }

    #[test]
    fn opus_decoder_proof_is_bound_to_current_asset_hash_and_decoder_bytes() {
        let proof = crate::loudness::OpusDecoderProof {
            asset_id: "asset-a".into(),
            file_hash: "a".repeat(64),
            decoder_sha256: "b".repeat(64),
            probe_schema_version: 1,
        };
        assert!(proof.validates_for("asset-a", &"a".repeat(64), &"b".repeat(64)));
        assert!(!proof.validates_for("asset-b", &"a".repeat(64), &"b".repeat(64)));
        assert!(!proof.validates_for("asset-a", &"c".repeat(64), &"b".repeat(64)));
        assert!(!proof.validates_for("asset-a", &"a".repeat(64), &"d".repeat(64)));
    }

    #[test]
    fn opus_r128_album_identity_requires_exact_album_and_artist_metadata() {
        let mut track = Track {
            album: Some("Night Drive".into()),
            album_artist: Some("The Example Band".into()),
            ..Track::default()
        };
        let tags = crate::loudness_analysis::LocalTagMeasurements {
            album: Some("night drive".into()),
            album_artist: Some("the example band".into()),
            track_replaygain: None,
            album_replaygain: None,
            replaygain_comment_present: false,
            opus_r128_track_gain_db: None,
            opus_r128_album_gain_db: Some(-2.0),
            opus_r128_track_tag_present: false,
            opus_r128_album_tag_present: true,
            opus_output_gain_tag_db: None,
            file_seal: None,
        };
        assert!(opus_r128_album_identity_matches(&tags, &track));
        track.album = Some("Night Drive (Deluxe)".into());
        assert!(!opus_r128_album_identity_matches(&tags, &track));
        track.album = Some("Night Drive".into());
        track.album_artist = Some("Different Artist".into());
        assert!(!opus_r128_album_identity_matches(&tags, &track));
        track.album_artist = Some("The Example Band".into());
        let mut missing_artist = tags.clone();
        missing_artist.album_artist = None;
        assert!(!opus_r128_album_identity_matches(&missing_artist, &track));
        let mut no_album = tags.clone();
        no_album.album = None;
        track.album = None;
        assert!(!opus_r128_album_identity_matches(&no_album, &track));
        track.album = Some("Night Drive".into());
        let mut no_artist = tags.clone();
        no_artist.album_artist = None;
        track.album_artist = None;
        assert!(!opus_r128_album_identity_matches(&no_artist, &track));
        let mut no_track_album = track.clone();
        no_track_album.album = None;
        let mut no_track_artist = track.clone();
        no_track_artist.album_artist = None;
        assert!(!opus_r128_album_identity_matches(&tags, &no_track_album));
        assert!(!opus_r128_album_identity_matches(&tags, &no_track_artist));
        let both_unidentified = crate::loudness_analysis::LocalTagMeasurements {
            album: None,
            album_artist: None,
            ..tags.clone()
        };
        let both_track_unidentified = Track::default();
        assert!(!opus_r128_album_identity_matches(
            &both_unidentified,
            &both_track_unidentified
        ));
        track.album_artist = Some("The Example Band".into());
        let exact_context = crate::loudness::LoudnessResourceIdentity::album(
            vec![crate::loudness::LoudnessAssetIdentity {
                asset_id: "opus-asset".into(),
                file_hash: "opus-hash".into(),
                range: crate::loudness::AudioRange::WHOLE_FILE,
            }],
            vec![],
        );
        assert!(opus_r128_album_context_matches(
            &tags,
            &track,
            Some(&exact_context),
            "opus-asset",
            "opus-hash",
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!opus_r128_album_context_matches(
            &tags,
            &track,
            Some(&exact_context),
            "different-asset",
            "opus-hash",
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!opus_r128_album_context_matches(
            &tags,
            &track,
            Some(&exact_context),
            "opus-asset",
            "other-hash",
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!opus_r128_album_context_matches(
            &tags,
            &track,
            None,
            "opus-asset",
            "opus-hash",
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!opus_r128_album_context_matches(
            &tags,
            &track,
            Some(&exact_context),
            "opus-asset",
            "opus-hash",
            crate::loudness::AudioRange {
                start_seconds: 8.0,
                end_seconds: None
            },
        ));
    }

    #[test]
    fn native_opus_decode_policy_requires_exact_registered_whole_local_resource() {
        let (source, asset) = opus_decode_policy_fixture();
        let probed = CurrentPlayableCodec {
            codec: "opus".into(),
            container: "ogg".into(),
        };
        assert!(native_opus_decode_policy_eligible(
            &source,
            &asset,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(native_opus_decode_preprobe_eligible(
            &source,
            &asset,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!native_opus_decode_preprobe_eligible(
            &source,
            &asset,
            crate::loudness::NormalizationMode::Off,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!native_opus_decode_preprobe_eligible(
            &source,
            &asset,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange {
                start_seconds: 2.0,
                end_seconds: None
            },
        ));
        let mut selected = source.clone();
        apply_native_opus_decode_policy(&mut selected, &asset, "a".repeat(64));
        assert_eq!(
            selected.decoder_hint.as_ref().unwrap().decode_policy,
            crate::native_output::AudioSourceDecodePolicy::FfmpegOggOpusHeadGain,
        );
        let mut transcoded = asset.clone();
        transcoded.source_codec = "opus".into();
        transcoded.final_codec = "mp3".into();
        assert!(!current_file_is_ogg_opus(&transcoded, &probed));
        assert!(!native_opus_decode_policy_eligible(
            &source,
            &transcoded,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        let mut mp3_playable = source.clone();
        mp3_playable.input = "/music/test.mp3".into();
        let mut mp3_asset = asset.clone();
        mp3_asset.path = mp3_playable.input.clone();
        mp3_asset.source_codec = "opus".into();
        mp3_asset.final_codec = "mp3".into();
        assert!(!native_opus_decode_policy_eligible(
            &mp3_playable,
            &mp3_asset,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!current_file_is_ogg_opus(
            &asset,
            &CurrentPlayableCodec {
                codec: "mp3".into(),
                container: "ogg".into()
            },
        ));
        assert!(!current_file_is_ogg_opus(
            &asset,
            &CurrentPlayableCodec {
                codec: "opus".into(),
                container: "matroska,webm".into()
            },
        ));
        assert!(!native_opus_decode_policy_eligible(
            &source,
            &asset,
            &probed,
            crate::loudness::NormalizationMode::Off,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        assert!(!native_opus_decode_policy_eligible(
            &source,
            &asset,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange {
                start_seconds: 2.0,
                end_seconds: None
            },
        ));
        let mut other_path = source.clone();
        other_path.input = "/music/other.opus".into();
        assert!(!native_opus_decode_policy_eligible(
            &other_path,
            &asset,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        let mut remote = source.clone();
        remote.input = "https://media.example/test.opus".into();
        assert!(!native_opus_decode_policy_eligible(
            &remote,
            &asset,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        let mut cenc = source.clone();
        cenc.cenc = Some(crate::native_output::EncryptedMp4Audio::new(
            crate::native_output::CencDecryptionKey::from_hex("00112233445566778899aabbccddeeff")
                .unwrap(),
        ));
        assert!(!native_opus_decode_policy_eligible(
            &cenc,
            &asset,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
        let mut non_opus = asset.clone();
        non_opus.source_codec = "flac".into();
        non_opus.final_codec = "flac".into();
        assert!(!native_opus_decode_policy_eligible(
            &source,
            &non_opus,
            &probed,
            crate::loudness::NormalizationMode::Track,
            crate::loudness::AudioRange::WHOLE_FILE,
        ));
    }

    #[test]
    fn native_normalization_route_status_is_generation_bound() {
        let unavailable = |reason: &str| {
            crate::loudness_manager::LoudnessStatus::unavailable(
                crate::loudness::NormalizationMode::Track,
                reason,
                Some("verified-resource".into()),
            )
        };
        let mut state = State::default();
        state.generation = 4;
        assert!(!apply_normalization_selection(
            &mut state,
            3,
            unavailable("stale")
        ));
        assert_eq!(
            state.loudness_status,
            crate::loudness_manager::LoudnessStatus::off()
        );
        assert!(apply_normalization_selection(
            &mut state,
            4,
            unavailable("current")
        ));
        assert_eq!(state.loudness_status.reason.as_deref(), Some("current"));

        state.entries.push("next-entry".into());
        state.preload = Some(Preloaded {
            preparation_trace: None,
            generation: 9,
            index: 0,
            entry: "next-entry".into(),
            candidate: None,
            quality: json!({}),
            duration: Some(20.0),
            steady_http_stream: false,
            offset: 0.0,
            end: None,
            loudness_status: crate::loudness_manager::LoudnessStatus::off(),
            loudness_album_context: None,
            opus_decoder_capability: None,
        });
        assert!(apply_normalization_selection(
            &mut state,
            9,
            unavailable("preload")
        ));
        assert_eq!(
            state
                .preload
                .as_ref()
                .unwrap()
                .loudness_status
                .reason
                .as_deref(),
            Some("preload")
        );
        assert!(!apply_normalization_selection(
            &mut state,
            10,
            unavailable("obsolete preload")
        ));
    }

    #[test]
    fn album_loudness_context_follows_queue_occurrence_ids_across_reorder_and_remove() {
        let first = Track {
            id: "same-recording".into(),
            asset_id: Some("asset-first".into()),
            ..Track::default()
        };
        let second = Track {
            id: "same-recording".into(),
            asset_id: Some("asset-second".into()),
            ..Track::default()
        };
        let resource = crate::loudness::LoudnessResourceIdentity::album(
            vec![
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "asset-first".into(),
                    file_hash: "hash-first".into(),
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
                crate::loudness::LoudnessAssetIdentity {
                    asset_id: "asset-second".into(),
                    file_hash: "hash-second".into(),
                    range: crate::loudness::AudioRange::WHOLE_FILE,
                },
            ],
            vec![0.0],
        );
        let mut state = State::default();
        state.queue = vec![first.clone(), second.clone(), Track::default()];
        state.entries = vec!["same-1".into(), "same-2".into(), "other".into()];
        validate_loudness_album_queue_binding(&resource, &state.queue[..2]).unwrap();
        let measured_with_silence =
            crate::loudness::LoudnessResourceIdentity::album(resource.members.clone(), vec![1.0]);
        assert!(
            validate_loudness_album_queue_binding(&measured_with_silence, &state.queue[..2])
                .is_err()
        );
        bind_loudness_context_to_queue_range(&mut state, 0, 2, Some(&resource));

        reorder_queue_state(&mut state, 0, 1, None).unwrap();
        assert_eq!(
            state.entries,
            vec!["same-2".to_owned(), "same-1".to_owned(), "other".to_owned()]
        );
        assert_eq!(state.loudness_queue_contexts.get("same-1"), Some(&resource));
        assert_eq!(state.loudness_queue_contexts.get("same-2"), Some(&resource));

        remove_queue_occurrences(&mut state, &[0], None).unwrap();
        assert!(!state.loudness_queue_contexts.contains_key("same-2"));
        assert_eq!(state.loudness_queue_contexts.get("same-1"), Some(&resource));
    }

    #[test]
    fn webview_album_context_requires_unique_exact_zero_gap_resource_members() {
        use crate::loudness::{AudioRange, LoudnessAssetIdentity, LoudnessResourceIdentity};

        let members = vec![
            LoudnessAssetIdentity {
                asset_id: "asset-a".into(),
                file_hash: "a".repeat(64),
                range: AudioRange::WHOLE_FILE,
            },
            LoudnessAssetIdentity {
                asset_id: "asset-b".into(),
                file_hash: "b".repeat(64),
                range: AudioRange::WHOLE_FILE,
            },
        ];
        let exact = LoudnessResourceIdentity::album(members.clone(), vec![0.0]);
        assert!(validate_loudness_album_resource_member(
            &exact,
            "asset-a",
            &"a".repeat(64),
            AudioRange::WHOLE_FILE,
        )
        .is_ok());
        assert!(validate_loudness_album_resource_member(
            &exact,
            "asset-a",
            &"a".repeat(64),
            AudioRange {
                start_seconds: 10.0,
                end_seconds: Some(20.0),
            },
        )
        .is_err());

        let duplicate = LoudnessResourceIdentity::album(
            vec![members[0].clone(), members[0].clone()],
            vec![0.0],
        );
        assert!(validate_loudness_album_resource_member(
            &duplicate,
            "asset-a",
            &"a".repeat(64),
            AudioRange::WHOLE_FILE,
        )
        .is_err());

        let nonzero_gap = LoudnessResourceIdentity::album(members, vec![0.25]);
        assert!(validate_loudness_album_resource_member(
            &nonzero_gap,
            "asset-a",
            &"a".repeat(64),
            AudioRange::WHOLE_FILE,
        )
        .is_err());
    }

    #[test]
    fn new_play_queue_honors_enabled_shuffle_and_preserves_selected_occurrence() {
        let tracks: Vec<_> = (0..64)
            .map(|i| Track {
                id: format!("track-{i}"),
                ..Track::default()
            })
            .collect();
        let mut state = State::default();
        replace_play_queue(&mut state, tracks.clone(), 3);
        assert_eq!(
            state.queue.iter().map(|t| &t.id).collect::<Vec<_>>(),
            tracks.iter().map(|t| &t.id).collect::<Vec<_>>()
        );
        assert_eq!(state.queue_revision, 1);
        state.shuffle = true;
        replace_play_queue(&mut state, tracks.clone(), 3);
        assert_eq!(state.index, 3);
        assert!(state.shuffle);
        assert_eq!(state.queue_revision, 2);
        assert_eq!(
            state.queue[..4].iter().map(|t| &t.id).collect::<Vec<_>>(),
            tracks[..4].iter().map(|t| &t.id).collect::<Vec<_>>()
        );
        assert_ne!(
            state.queue[4..].iter().map(|t| &t.id).collect::<Vec<_>>(),
            tracks[4..].iter().map(|t| &t.id).collect::<Vec<_>>()
        );
        let mut ids = state.queue.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        ids.sort();
        let mut expected = tracks.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        expected.sort();
        assert_eq!(ids, expected);
        let mut entries = state.entries.clone();
        entries.sort();
        entries.dedup();
        assert_eq!(entries.len(), tracks.len());
    }

    #[test]
    fn shuffle_keeps_occurrence_ids_attached_to_tracks_and_leaves_history_untouched() {
        let mut state = State::default();
        state.queue = (0..64)
            .map(|i| Track {
                id: format!("track-{i}"),
                ..Track::default()
            })
            .collect();
        state.entries = state
            .queue
            .iter()
            .map(|t| format!("occurrence-{}", t.id))
            .collect();
        state.index = 3;
        shuffle_upcoming(&mut state);
        for (track, entry) in state.queue.iter().zip(&state.entries) {
            assert_eq!(entry, &format!("occurrence-{}", track.id));
        }
        assert_eq!(state.queue[3].id, "track-3");
        assert_eq!(state.queue[0].id, "track-0");
        assert_ne!(state.queue[4].id, state.queue[5].id);
    }

    fn youtube_handoff_fixture() -> (Track, Candidate) {
        let track = Track {
            id: "youtube:6MAzUT1YhWE".into(),
            provider: "youtube".into(),
            provider_id: "6MAzUT1YhWE".into(),
            source_url: "https://www.youtube.com/watch?v=6MAzUT1YhWE".into(),
            title: "Rooftop Live".into(),
            duration_seconds: Some(6128.0),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "youtube:6MAzUT1YhWE".into(),
            provider: "youtube".into(),
            url: track.source_url.clone(),
            title: track.title.clone(),
            ..Candidate::default()
        };
        (track, candidate)
    }

    fn resolved_handoff_fixture() -> ResolvedSourceHandoff {
        let track = Track {
            id: "spotify:recording-1".into(),
            provider: "spotify".into(),
            provider_id: "recording-1".into(),
            title: "Fixture recording".into(),
            duration_seconds: Some(240.0),
            ..Track::default()
        };
        ResolvedSourceHandoff {
            handoff_id: uuid::Uuid::new_v4().to_string(),
            source_handle: uuid::Uuid::new_v4().to_string(),
            source_generation: 12,
            seek_serial: 3,
            target_position: 120.0,
            tracks: vec![track],
            entry_ids: vec!["occurrence-1".into()],
            index: 0,
            session_id: "session-1".into(),
            entry_id: "occurrence-1".into(),
            paused: false,
            volume: 0.8,
            repeat: "off".into(),
            shuffle: false,
            recorded: false,
        }
    }

    fn pending_source_fixture(
        generation: u64,
        request_track: &Track,
        expires_at: Instant,
    ) -> PendingNativeSource {
        PendingNativeSource {
            source_generation: generation,
            request_track: request_track.clone(),
            explicit_source: false,
            matching_level: crate::recording_match::MatchLevel::Balanced,
            expires_at,
            resolved: crate::playback_sources::ResolvedSource {
                origin: crate::playback_sources::ResolvedSourceOrigin::Existing,
                source: crate::native_output::AudioSource {
                    input: "http://127.0.0.1/fixture".into(),
                    ffmpeg: "ffmpeg".into(),
                    headers: vec![],
                    decoder_hint: None,
                    cenc: None,
                },
                candidate: None,
                quality: serde_json::json!({"matchLevel":"balanced"}),
                station: None,
                duration: Some(240.0),
            },
        }
    }

    #[test]
    fn resolved_source_handoff_binds_generation_track_expiry_and_one_use_handle() {
        let handoff = resolved_handoff_fixture();
        let now = Instant::now();
        let track = &handoff.tracks[0];
        let current = pending_source_fixture(12, track, now + Duration::from_secs(1));
        assert!(pending_source_owner_matches(&current, &handoff, track, now));

        let stale_generation = pending_source_fixture(11, track, now + Duration::from_secs(1));
        assert!(!pending_source_owner_matches(
            &stale_generation,
            &handoff,
            track,
            now
        ));
        let mut changed_track = track.clone();
        changed_track.artist = "Changed artist".into();
        let stale_track = pending_source_fixture(12, &changed_track, now + Duration::from_secs(1));
        assert!(!pending_source_owner_matches(
            &stale_track,
            &handoff,
            track,
            now
        ));
        let mut mutable_library_fields = track.clone();
        mutable_library_fields.favorite = true;
        mutable_library_fields.local_availability = "available".into();
        let retained =
            pending_source_fixture(12, &mutable_library_fields, now + Duration::from_secs(1));
        assert!(pending_source_owner_matches(
            &retained, &handoff, track, now
        ));
        let expired = pending_source_fixture(12, track, now);
        assert!(!pending_source_owner_matches(
            &expired, &handoff, track, now
        ));

        let mut registry = HashMap::new();
        registry.insert(handoff.source_handle.clone(), current);
        assert!(take_pending_native_source(&mut registry, &handoff.source_handle, now).is_some());
        assert!(take_pending_native_source(&mut registry, &handoff.source_handle, now).is_none());
        assert!(registry.is_empty());

        registry.insert(
            handoff.source_handle.clone(),
            pending_source_fixture(12, track, now),
        );
        assert!(take_pending_native_source(&mut registry, &handoff.source_handle, now).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn pending_native_source_purge_releases_only_superseded_generations() {
        let now = Instant::now();
        let track = Track {
            id: "monochrome:purge-test".into(),
            ..Track::default()
        };
        let mut registry = HashMap::new();
        registry.insert(
            "old".into(),
            pending_source_fixture(7, &track, now + Duration::from_secs(1)),
        );
        registry.insert(
            "current".into(),
            pending_source_fixture(8, &track, now + Duration::from_secs(1)),
        );
        registry.insert(
            "future".into(),
            pending_source_fixture(9, &track, now + Duration::from_secs(1)),
        );

        assert_eq!(purge_pending_native_sources_older_than(&mut registry, 8), 1);
        assert!(!registry.contains_key("old"));
        assert!(registry.contains_key("current"));
        assert!(registry.contains_key("future"));
        assert_eq!(
            purge_pending_native_sources_older_than(&mut registry, 10),
            2
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn native_handoff_track_binding_covers_identity_evidence_not_library_state() {
        let expected = Track {
            id: "monochrome:recording-1".into(),
            provider: "monochrome".into(),
            provider_id: "recording-1".into(),
            source_url: "https://media.example/recording".into(),
            artist: "Artist".into(),
            title: "Recording".into(),
            version: Some("Live".into()),
            duration_seconds: Some(240.0),
            isrc: Some("USAAA0000001".into()),
            provenance: serde_json::json!({"recordingContext":{"authority":"catalog","isrc":"USAAA0000001"}}),
            ..Track::default()
        };
        let mut changed = expected.clone();
        changed.title = "Different recording".into();
        assert!(!native_handoff_track_identity_matches(&expected, &changed));
        changed = expected.clone();
        changed.duration_seconds = Some(241.0);
        assert!(!native_handoff_track_identity_matches(&expected, &changed));
        changed = expected.clone();
        changed.provenance["recordingContext"]["authority"] = serde_json::json!("unverified");
        assert!(!native_handoff_track_identity_matches(&expected, &changed));
        changed = expected.clone();
        changed.favorite = true;
        changed.local_availability = "available".into();
        changed.artwork_url = Some("https://img.example/cover".into());
        assert!(native_handoff_track_identity_matches(&expected, &changed));
    }

    #[test]
    fn native_handoff_rejects_missing_unknown_or_changed_matching_level() {
        let make = |level: &str| crate::playback_sources::ResolvedSource {
            origin: crate::playback_sources::ResolvedSourceOrigin::Existing,
            source: crate::native_output::AudioSource {
                input: String::new(),
                ffmpeg: Default::default(),
                headers: vec![],
                decoder_hint: None,
                cenc: None,
            },
            candidate: None,
            quality: serde_json::json!({"matchLevel":level}),
            station: None,
            duration: None,
        };
        assert_eq!(
            resolved_match_level(&make("exact")),
            Some(crate::recording_match::MatchLevel::Exact)
        );
        assert_eq!(
            resolved_match_level(&make("balanced")),
            Some(crate::recording_match::MatchLevel::Balanced)
        );
        assert_eq!(
            resolved_match_level(&make("flexible")),
            Some(crate::recording_match::MatchLevel::Flexible)
        );
        assert_eq!(resolved_match_level(&make("strict")), None);
        assert_eq!(resolved_match_level(&make("")), None);
        assert_eq!(
            resolved_match_level(&make("balanced")),
            Some(crate::recording_match::MatchLevel::Balanced)
        );
        assert_ne!(
            resolved_match_level(&make("balanced")),
            Some(crate::recording_match::MatchLevel::Exact)
        );
        let source = pending_source_fixture(
            1,
            &Track::default(),
            Instant::now() + Duration::from_secs(1),
        );
        assert!(pending_source_policy_matches(
            &source,
            crate::recording_match::MatchLevel::Balanced
        ));
        assert!(!pending_source_policy_matches(
            &source,
            crate::recording_match::MatchLevel::Exact
        ));
    }

    #[test]
    fn resolved_source_handoff_rejects_queue_occurrence_mismatch() {
        let mut handoff = resolved_handoff_fixture();
        assert_eq!(
            validate_resolved_handoff_occurrence(&handoff)
                .unwrap()
                .id
                .as_str(),
            handoff.tracks[0].id.as_str()
        );
        handoff.entry_id = "different-occurrence".into();
        assert!(validate_resolved_handoff_occurrence(&handoff).is_err());
        handoff.entry_id = "occurrence-1".into();
        let duplicate_track = handoff.tracks[0].clone();
        handoff.tracks.push(duplicate_track);
        handoff.entry_ids.push("occurrence-1".into());
        assert!(validate_resolved_handoff_occurrence(&handoff).is_err());
    }

    #[test]
    fn webview_handoff_requires_same_full_youtube_video_and_valid_seek_position() {
        let (track, candidate) = youtube_handoff_fixture();
        assert_eq!(
            exact_handoff_youtube_id(&track, &candidate, 1800.0),
            Some("6MAzUT1YhWE")
        );

        let mut mismatched = candidate.clone();
        mismatched.url = "https://www.youtube.com/watch?v=aaaaaaaaaaa".into();
        assert!(exact_handoff_youtube_id(&track, &mismatched, 1800.0).is_none());

        let mut excerpt = track.clone();
        excerpt.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: track.id.clone(),
            chapter_id: "excerpt".into(),
            source_url: track.source_url.clone(),
            start_time: 100.0,
            end_time: 200.0,
            parent_title: track.title.clone(),
            parent_duration_seconds: track.duration_seconds,
        });
        assert!(exact_handoff_youtube_id(&excerpt, &candidate, 150.0).is_none());
        assert!(exact_handoff_youtube_id(&track, &candidate, 6128.0).is_none());
        assert!(exact_handoff_youtube_id(&track, &candidate, f64::NAN).is_none());
    }

    #[test]
    fn stale_frontend_stop_cannot_stop_a_different_handoff_owner() {
        assert!(handoff_stop_is_owned("handoff-a", Some("handoff-a")));
        assert!(!handoff_stop_is_owned("handoff-a", Some("handoff-b")));
        assert!(!handoff_stop_is_owned("handoff-a", None));
    }

    #[test]
    fn next_preparation_diagnostic_window_is_bounded_to_thirty_or_forty_five_seconds() {
        assert_eq!(preparation_window_seconds(Some("30")), 30.);
        assert_eq!(preparation_window_seconds(Some("45")), 45.);
        for invalid in [
            None,
            Some("0"),
            Some("-1"),
            Some("60"),
            Some("NaN"),
            Some("45.0"),
            Some(" 45"),
        ] {
            assert_eq!(preparation_window_seconds(invalid), 30.);
        }
    }

    fn preparation_fixture() -> (State, PreloadPreparationOwner, watch::Receiver<bool>) {
        let mut state = State::default();
        let track = Track {
            id: "same-recording".into(),
            title: "Repeated recording".into(),
            ..Track::default()
        };
        state.queue = vec![track.clone(), track];
        state.entries = vec!["current-occurrence".into(), "next-occurrence".into()];
        state.status = "playing".into();
        state.generation = 41;
        state.generation_counter = 41;
        state.output_ready = true;
        state.offset = 5.;
        let (cancel, receiver) = watch::channel(false);
        state.preload_cancel = Some(cancel.clone());
        let owner = PreloadPreparationOwner {
            generation: 41,
            index: 1,
            entry: "next-occurrence".into(),
            cancel,
        };
        (state, owner, receiver)
    }

    fn replace_preparation(state: &mut State) -> PreloadPreparationOwner {
        let (cancel, _) = watch::channel(false);
        state.preload_cancel = Some(cancel.clone());
        PreloadPreparationOwner {
            generation: state.generation,
            index: 1,
            entry: state.entries[1].clone(),
            cancel,
        }
    }

    #[test]
    fn preload_completion_rechecks_cancel_and_owner_at_the_publication_boundary() {
        for success in [false, true] {
            let (mut state, old, old_receiver) = preparation_fixture();
            assert!(preload_preparation_is_current(&state, &old));
            // Resolve has already completed and the old task is about to lock
            // State. Pause/resume can replace its owner without changing this
            // generation, occurrence, or final playing status.
            let result = if success {
                Ok("old-source")
            } else {
                Err("old-network-error".into())
            };
            state.status = "paused".into();
            cancel_preload_state(&mut state);
            assert!(*old_receiver.borrow());
            state.status = "playing".into();
            let replacement = replace_preparation(&mut state);
            assert!(finish_preload_resolution(&mut state, &old, result).is_none());
            assert!(state
                .preload_cancel
                .as_ref()
                .unwrap()
                .same_channel(&replacement.cancel));
            assert!(preload_preparation_is_current(&state, &replacement));
            assert_eq!(
                finish_preload_resolution(&mut state, &replacement, Ok("replacement-source")),
                Some("replacement-source")
            );
        }
    }

    #[test]
    fn settings_change_fences_unresolved_preload_before_late_source_publication() {
        let (mut unchanged, unchanged_owner, unchanged_receiver) = preparation_fixture();
        assert!(!fence_unresolved_preload_for_settings_change(
            &mut unchanged,
            false
        ));
        assert!(!*unchanged_receiver.borrow());
        assert!(preload_preparation_is_current(&unchanged, &unchanged_owner));

        let (mut changed, old_owner, old_receiver) = preparation_fixture();
        assert!(changed.preload.is_none());
        // This executes the state-lock fence before the resolver can reacquire
        // State to install its source or append the native PCM queue.
        assert!(fence_unresolved_preload_for_settings_change(
            &mut changed,
            true
        ));
        assert!(*old_receiver.borrow());
        assert!(!preload_preparation_is_current(&changed, &old_owner));
        assert!(finish_preload_resolution(&mut changed, &old_owner, Ok("stale-source")).is_none());
        assert!(changed.preload.is_none());
        assert!(changed.preload_cancel.is_none());

        let replacement = replace_preparation(&mut changed);
        assert!(preload_preparation_is_current(&changed, &replacement));
        assert!(finish_preload_resolution(
            &mut changed,
            &old_owner,
            Err::<&str, _>("late old failure".into())
        )
        .is_none());
        assert!(changed
            .preload_cancel
            .as_ref()
            .unwrap()
            .same_channel(&replacement.cancel));
    }

    #[test]
    fn preload_channel_identity_fences_an_abandoned_owner_even_without_a_cancel_signal() {
        let (mut state, old, receiver) = preparation_fixture();
        let replacement = replace_preparation(&mut state);
        assert!(!*receiver.borrow());
        assert!(!preload_preparation_is_current(&state, &old));
        assert!(
            finish_preload_resolution::<()>(&mut state, &old, Err("late failure".into())).is_none()
        );
        assert!(state
            .preload_cancel
            .as_ref()
            .unwrap()
            .same_channel(&replacement.cancel));
    }

    #[test]
    fn preload_failure_releases_only_its_current_slot_and_stale_occurrences_cannot_adopt() {
        let (mut state, owner, _) = preparation_fixture();
        assert!(finish_preload_resolution::<()>(
            &mut state,
            &owner,
            Err("temporary failure".into())
        )
        .is_none());
        assert!(
            state.preload_cancel.is_none(),
            "owned failure must allow a later preparation"
        );
        let replacement = replace_preparation(&mut state);
        state.entries[1] = "different-occurrence-of-same-recording".into();
        assert_eq!(state.queue[0].id, state.queue[1].id);
        assert!(
            finish_preload_resolution(&mut state, &replacement, Ok("old-occurrence-source"))
                .is_none()
        );
        assert!(state
            .preload_cancel
            .as_ref()
            .unwrap()
            .same_channel(&replacement.cancel));
    }

    #[test]
    fn preload_active_device_loss_cancels_reader_and_releases_reconnect_preparation() {
        let (mut state, old, receiver) = preparation_fixture();
        state.preload = Some(Preloaded {
            preparation_trace: None,
            generation: 42,
            index: 1,
            entry: old.entry.clone(),
            candidate: None,
            quality: json!({}),
            duration: Some(60.),
            steady_http_stream: false,
            offset: 0.,
            end: None,
            loudness_status: crate::loudness_manager::LoudnessStatus::off(),
            loudness_album_context: None,
            opus_decoder_capability: None,
        });
        let event = OutputEvent::DeviceLost {
            generation: 41,
            position: 17.,
            reason: "fixture device removed".into(),
        };
        let OutputEvent::DeviceLost {
            generation,
            position,
            reason,
        } = event
        else {
            unreachable!()
        };
        assert!(apply_device_lost(&mut state, generation, position, &reason));
        assert!(*receiver.borrow());
        assert!(state.preload.is_none());
        assert!(state.preload_cancel.is_none());
        assert_eq!(state.position, 12.);
        assert_eq!(state.status, "buffering");
        assert!(!state.output_ready);
        // Reconnected Started keeps the current generation but must be able to
        // acquire a new speculative owner, rather than keeping a stranded slot.
        state.status = "playing".into();
        let replacement = replace_preparation(&mut state);
        assert!(preload_preparation_is_current(&state, &replacement));
        assert!(finish_preload_resolution(&mut state, &old, Ok("discarded-reader")).is_none());
        assert!(preload_preparation_is_current(&state, &replacement));
    }

    #[test]
    fn preload_stale_device_loss_preserves_new_owner_and_paused_state() {
        let (mut state, owner, receiver) = preparation_fixture();
        assert!(!apply_device_lost(&mut state, 40, 99., "old device"));
        assert!(!*receiver.borrow());
        assert!(preload_preparation_is_current(&state, &owner));
        assert_eq!(state.status, "playing");
        assert!(state.output_ready);
        state.status = "paused".into();
        assert!(apply_device_lost(&mut state, 41, 17., "current device"));
        assert_eq!(state.status, "paused");
        assert!(*receiver.borrow());
        assert!(state.preload_cancel.is_none());
        assert!(!state.output_ready);
    }

    #[tokio::test]
    async fn native_acceptance_commits_before_a_competing_command_can_replace_owner() {
        let commands = Arc::new(tokio::sync::Mutex::new(()));
        let owner = Arc::new(Mutex::new("first"));
        let receipt = Arc::new(Mutex::new(None));
        let command = commands.lock().await;
        let (attempting_tx, attempting_rx) = std::sync::mpsc::channel();
        let (replaced_tx, replaced_rx) = std::sync::mpsc::channel();
        let competitor_commands = commands.clone();
        let competitor_owner = owner.clone();
        let competitor = std::thread::spawn(move || {
            // Prove that this command attempted acquisition while acceptance
            // still owns the mutex, before allowing the commit to proceed.
            assert!(competitor_commands.try_lock().is_err());
            attempting_tx.send(()).unwrap();
            let _command = competitor_commands.blocking_lock();
            *competitor_owner.lock().unwrap() = "replacement";
            // The commit callback may have returned and dropped its receiver.
            let _ = replaced_tx.send(());
        });
        attempting_rx.recv().unwrap();
        let committed = receipt.clone();
        commit_queued_playback(
            &command,
            || json!({"queuedOperationId":*owner.lock().unwrap(),"queueEntryIds":["one","two"]}),
            Some(Box::new(move |full| {
                assert_eq!(full["queuedOperationId"], "first");
                assert!(replaced_rx.try_recv().is_err());
                *committed.lock().unwrap() = Some(full);
                Ok(())
            })),
        )
        .unwrap();
        assert_eq!(*owner.lock().unwrap(), "first");
        assert_eq!(
            receipt.lock().unwrap().as_ref().unwrap()["queueEntryIds"],
            json!(["one", "two"])
        );
        drop(command);
        competitor.join().unwrap();
        assert_eq!(*owner.lock().unwrap(), "replacement");
        assert_eq!(
            receipt.lock().unwrap().as_ref().unwrap()["queuedOperationId"],
            "first"
        );
    }

    #[tokio::test]
    async fn native_acceptance_rejection_does_not_publish_a_receipt() {
        use crate::queued_playback::{Registry, Status};

        let commands = tokio::sync::Mutex::new(());
        let command = commands.lock().await;
        let registry = Arc::new(Mutex::new(Registry::<Value>::new(4)));
        registry
            .lock()
            .unwrap()
            .begin("cancelled".into(), "transport".into(), vec!["track".into()])
            .unwrap();
        assert!(registry.lock().unwrap().cancel("cancelled").unwrap());
        let commit_registry = registry.clone();
        let result = commit_queued_playback(
            &command,
            || json!({"queuedOperationId":"cancelled"}),
            Some(Box::new(move |receipt| {
                if commit_registry
                    .lock()
                    .unwrap()
                    .accept("cancelled", "transport", receipt)
                {
                    Ok(())
                } else {
                    Err("operation was cancelled before commit".into())
                }
            })),
        );
        assert!(result.is_err());
        let operation = registry.lock().unwrap().get("cancelled").unwrap();
        assert_eq!(operation.status, Status::Cancelled);
        assert!(operation.receipt.is_none());
    }

    #[test]
    fn buffering_event_diagnostics_ignore_stale_generation_and_clear_on_new_track_and_seek() {
        let mut state = State::default();
        state.queue = vec![Track {
            id: "fixture".into(),
            title: "private title".into(),
            source_url: "https://secret.invalid/token".into(),
            ..Track::default()
        }];
        state.generation = 8;
        state.seek_serial = 3;
        state.status = "playing".into();
        let facts = || BufferingDiagnostics {
            reader_observed: true,
            network_reader: true,
            bytes_read: 123,
            downloaded_range_end: 456,
            download_progress_age_ms: Some(70),
            read_in_flight_ms: Some(20),
            max_read_ms: 9,
            seek_in_flight_ms: Some(30),
            max_seek_ms: 25,
            pcm_produced_samples: 4096,
            pcm_consumed_samples: 4096,
            synthetic_silence_samples: 50,
        };
        // Construct the real native event and use the same state handler as output_event.
        for generation in [7, 8] {
            let event = OutputEvent::Buffering {
                generation,
                diagnostics: Some(facts()),
            };
            let OutputEvent::Buffering {
                generation,
                diagnostics,
            } = event
            else {
                unreachable!()
            };
            let applied = apply_buffering_event(&mut state, generation, diagnostics);
            assert_eq!(applied, generation == 8);
            if generation == 7 {
                assert!(state.last_buffering.is_none());
                assert_eq!(state.buffering_count, 0);
            }
        }
        let value = state.snapshot(true, false)["diagnostics"]["lastBuffering"].clone();
        assert_eq!(value["generation"], 8);
        assert_eq!(value["seekSerial"], 3);
        assert_eq!(value["stream"]["bytesRead"], 123);
        assert!(value["stream"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v.is_number() || v.is_boolean() || v.is_null()));
        let encoded = serde_json::to_string(&value).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("http"));
        clear_current_source_failure(&mut state); // Existing new-track/queue reset boundary.
        assert!(state.last_buffering.is_none());
        assert!(apply_buffering_event(&mut state, 8, Some(facts())));
        record_seek_context(&mut state, None, Instant::now(), None, None, None);
        assert!(state.last_buffering.is_none());
        // Source-less initial Buffering doesn't invent runtime facts.
        assert!(apply_buffering_event(&mut state, 8, None));
        assert!(state.last_buffering.is_none());
    }

    #[test]
    fn paused_ready_storage_fault_requires_a_real_retry_not_resume_of_removed_output() {
        let mut state = State::default();
        state.queue = vec![Track {
            id: "paused-current".into(),
            ..Track::default()
        }];
        state.entries = vec!["paused-occurrence".into()];
        state.status = "paused".into();
        state.output_ready = true;
        state.position = 10.;
        let generation = state.generation;
        let fault = crate::native_output::storage_fault::Fault {
            operation: crate::native_output::storage_fault::Operation::Read,
            kind: crate::native_output::storage_fault::Kind::NoSpace,
        };
        assert!(apply_local_storage_failure(&mut state, generation, fault));
        assert_eq!(state.status, "failed");
        assert_eq!(state.resume_target(), (10., false, true));
        assert_eq!(state.entries[0], "paused-occurrence");
        // An actual cancelled operation cannot turn Pause into a failure.
        let (cancel, _) = watch::channel(true);
        state.status = "paused".into();
        state.cancel = Some(cancel);
        state.last_local_storage_failure = None;
        let generation = state.generation;
        assert!(!apply_local_storage_failure(&mut state, generation, fault));
        assert!(state.last_local_storage_failure.is_none());
    }

    #[test]
    fn local_storage_failure_retains_current_occurrence_without_source_recovery() {
        let mut state = State::default();
        state.queue = vec![
            Track {
                id: "current".into(),
                ..Track::default()
            },
            Track {
                id: "next".into(),
                ..Track::default()
            },
        ];
        state.entries = vec!["occurrence-one".into(), "occurrence-two".into()];
        state.status = "playing".into();
        state.position = 42.;
        state.generation = 7;
        state.generation_counter = 7;
        state.candidate = Some(Candidate {
            id: "same-signed-source".into(),
            ..Candidate::default()
        });
        let fault = crate::native_output::storage_fault::Fault {
            operation: crate::native_output::storage_fault::Operation::Write,
            kind: crate::native_output::storage_fault::Kind::QuotaExceeded,
        };
        assert!(apply_local_storage_failure(&mut state, 7, fault));
        assert_eq!(state.index, 0);
        assert_eq!(state.queue.len(), 2);
        assert_eq!(state.entries[0], "occurrence-one");
        assert_eq!(state.position, 42.);
        assert_eq!(state.candidate.as_ref().unwrap().id, "same-signed-source");
        assert_eq!(state.resume_target(), (42., false, true));
        assert_eq!(state.status, "failed");
        assert_eq!(state.recovery, 0);
        assert!(state.last_source_failure.is_none());
        assert!(state.last_skipped_track_failure.is_none());
        assert_eq!(
            state.snapshot(true, false)["diagnostics"]["lastLocalStorageFailure"]["kind"],
            "quotaExceeded"
        );
        assert!(!apply_local_storage_failure(&mut state, 7, fault));
        clear_current_source_failure(&mut state);
        assert!(state.last_local_storage_failure.is_none());
        for status in ["idle", "ended"] {
            state.status = status.into();
            let generation = state.generation;
            assert!(!apply_local_storage_failure(&mut state, generation, fault));
            assert!(state.last_local_storage_failure.is_none());
        }
    }

    #[test]
    fn recovery_facts_survive_restart_and_audio_without_exposing_free_form_metadata() {
        let mut state = State::default();
        state.generation = 7;
        state.seek_serial = 3;
        state.position = 12.35;
        state.operation_id = Some("native-start-1".into());
        state.seek_operation_id = Some("https://private.invalid/?token=secret".into());
        state.decoder_diagnostics = Some(DecoderDiagnostics {
            startup_reserve: None,
            failed_native_attempt: None,
            decoder_kind: "rodio".into(),
            codec_hint: Some("secret codec".into()),
            content_type: Some("secret header".into()),
            input_content_length: Some(18_638_054),
            content_range_total: None,
            bytes_read: 24_576,
            bytes_received: 53_259,
            sample_rate: 44100,
            channels: 2,
            total_duration_seconds: Some(181.4),
            reader_setup_ms: None,
            decoder_setup_ms: None,
            first_decoded_block_ms: None,
            reader_reused: true,
            decoder_reused: true,
        });
        record_recovery_facts(&mut state, RecoveryTrigger::PrematureEnded);
        let facts = state.last_recovery.clone().unwrap();
        assert_eq!(facts["trigger"], "premature_ended");
        assert_eq!(facts["decoder"]["bytesRead"], 24_576);
        assert_eq!(facts["decoderEvidence"], "last_started_snapshot");
        assert!(facts["seekOperationId"].is_null());
        assert!(!facts.to_string().contains("secret"));
        assert!(!facts.to_string().contains("https://"));
        state.decoder_diagnostics.as_mut().unwrap().bytes_read = u64::MAX;
        state.decoder_diagnostics.as_mut().unwrap().decoder_kind = "secret unknown decoder".into();
        record_recovery_facts(&mut state, RecoveryTrigger::OutputFailed);
        let bounded = state.last_recovery.as_ref().unwrap();
        assert_eq!(bounded["decoder"]["bytesRead"], (1u64 << 53) - 1);
        assert_eq!(bounded["decoder"]["countersCapped"], true);
        assert_eq!(bounded["decoder"]["decoderKind"], "other");
        state.last_recovery = Some(facts.clone());
        state.generation = 8;
        state.decoder_diagnostics = None;
        note_audio_started(&mut state);
        assert_eq!(
            state.snapshot(true, false)["diagnostics"]["lastRecovery"],
            facts
        );
        clear_playback_failures(&mut state);
        assert!(state.last_recovery.is_none());
    }

    #[test]
    fn recovery_trigger_is_finite_and_counters_are_bounded() {
        let mut state = State::default();
        record_recovery_facts(&mut state, RecoveryTrigger::OutputFailed);
        assert_eq!(
            state.last_recovery.as_ref().unwrap()["trigger"],
            "output_failed"
        );
        record_recovery_facts(&mut state, RecoveryTrigger::SourceResolution);
        assert_eq!(
            state.last_recovery.as_ref().unwrap()["trigger"],
            "source_resolution"
        );
        assert!(state.last_recovery.as_ref().unwrap()["decoder"].is_null());
    }

    #[test]
    fn native_identification_ignores_position_only_updates_and_rejects_identity_changes() {
        let original = json!({
            "status":"playing", "trackId":"track-a", "entryId":"entry-a",
            "generation":7, "seekSerial":2, "position":20.0, "volume":0.8,
            "chapter":{"title":"first","startSeconds":0.0,"endSeconds":40.0}
        });
        let chapter = original["chapter"].clone();
        let mut position_only = original.clone();
        position_only["position"] = json!(31.0);
        position_only["volume"] = json!(0.4);
        assert!(native_identification_matches(
            &position_only,
            "track-a",
            "entry-a",
            7,
            2,
            &chapter
        ));
        for changed in [
            json!({"status":"paused", "trackId":"track-a", "entryId":"entry-a", "generation":7, "seekSerial":2, "chapter":{"title":"first","startSeconds":0.0,"endSeconds":40.0}}),
            json!({"status":"playing", "trackId":"track-b", "entryId":"entry-a", "generation":8, "seekSerial":2, "chapter":{"title":"first","startSeconds":0.0,"endSeconds":40.0}}),
            json!({"status":"playing", "trackId":"track-a", "entryId":"entry-b", "generation":7, "seekSerial":2, "chapter":{"title":"first","startSeconds":0.0,"endSeconds":40.0}}),
            json!({"status":"playing", "trackId":"track-a", "entryId":"entry-a", "generation":7, "seekSerial":3, "chapter":{"title":"first","startSeconds":0.0,"endSeconds":40.0}}),
            json!({"status":"playing", "trackId":"track-a", "entryId":"entry-a", "generation":7, "seekSerial":2, "chapter":{"title":"second","startSeconds":40.0,"endSeconds":80.0}}),
        ] {
            assert!(!native_identification_matches(
                &changed, "track-a", "entry-a", 7, 2, &chapter
            ));
        }
    }

    #[tokio::test]
    async fn completed_identification_job_false_channel_closure_cannot_mask_success() {
        use std::future::Future;
        use std::task::{Context, Waker};
        let registry = PlayingTrackIdentifications::default();
        let job = registry.begin(None).unwrap();
        let cancelled = job.cancellation();
        let (updates_tx, updates_rx) = watch::channel(json!({"status":"playing",
            "trackId":"a","entryId":"occurrence","generation":7,"seekSerial":2,"chapter":null}));
        let mut completed = tokio::spawn(async move {
            let _job = job;
            "recognized"
        });
        while !completed.is_finished() {
            tokio::task::yield_now().await;
        }
        assert!(!*cancelled.borrow());
        assert!(
            cancelled.has_changed().is_err(),
            "worker must already have removed its sender"
        );
        let mut invalidated = Box::pin(wait_for_native_identification_invalidation(
            cancelled,
            updates_rx,
            "a".into(),
            "occurrence".into(),
            7,
            2,
            Value::Null,
        ));
        // Force the previously failing order: poll invalidation AFTER registry
        // closure and BEFORE consuming the successful worker JoinHandle.
        let mut context = Context::from_waker(Waker::noop());
        assert!(invalidated.as_mut().poll(&mut context).is_pending());
        tokio::select! {
            reason = &mut invalidated => panic!("successful worker falsely invalidated: {reason}"),
            result = &mut completed => assert_eq!(result.unwrap(), "recognized"),
        }
        drop(updates_tx);
    }

    #[tokio::test]
    async fn false_cancellation_closure_keeps_playback_watch_active_and_true_cancel_wins() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        for ending in ["pause", "seek", "cancel"] {
            let (cancel_tx, cancel_rx) = watch::channel(false);
            let original = json!({"status":"playing","trackId":"a","entryId":"occurrence",
                "generation":7,"seekSerial":2,"chapter":null});
            let (updates_tx, updates_rx) = watch::channel(original.clone());
            if ending == "cancel" {
                cancel_tx.send_replace(true);
            }
            drop(cancel_tx);
            let mut waiting = Box::pin(wait_for_native_identification_invalidation(
                cancel_rx,
                updates_rx,
                "a".into(),
                "occurrence".into(),
                7,
                2,
                Value::Null,
            ));
            let mut context = Context::from_waker(Waker::noop());
            if ending != "cancel" {
                assert!(
                    waiting.as_mut().poll(&mut context).is_pending(),
                    "false closure must neither cancel nor spin"
                );
                let mut changed = original;
                if ending == "pause" {
                    changed["status"] = json!("paused");
                } else {
                    changed["seekSerial"] = json!(3);
                }
                updates_tx.send_replace(changed);
            }
            let Poll::Ready(reason) = waiting.as_mut().poll(&mut context) else {
                panic!("real invalidation must complete");
            };
            assert_eq!(
                reason,
                if ending == "cancel" {
                    "Audio identification was cancelled"
                } else {
                    "Playback changed during identification"
                }
            );
        }
    }

    #[tokio::test]
    async fn native_identification_accepts_muted_playing_and_volume_changes_but_invalidates_pause_seek_and_cancel(
    ) {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let original = json!({"status":"playing","trackId":"track-a","entryId":"entry-a",
            "generation":7,"seekSerial":2,"volume":0.0,"chapter":null});
        assert!(native_identification_matches(
            &original,
            "track-a",
            "entry-a",
            7,
            2,
            &Value::Null
        ));
        for ending in ["pause", "seek", "cancel"] {
            let (cancel_tx, cancel_rx) = watch::channel(false);
            let (updates_tx, updates_rx) = watch::channel(original.clone());
            let mut waiting = Box::pin(wait_for_native_identification_invalidation(
                cancel_rx,
                updates_rx,
                "track-a".into(),
                "entry-a".into(),
                7,
                2,
                Value::Null,
            ));
            let mut context = Context::from_waker(Waker::noop());
            assert!(waiting.as_mut().poll(&mut context).is_pending());
            for volume in [0.8, 0.0] {
                let mut changed = original.clone();
                changed["volume"] = json!(volume);
                updates_tx.send_replace(changed);
                assert!(waiting.as_mut().poll(&mut context).is_pending());
            }
            if ending == "cancel" {
                cancel_tx.send_replace(true);
            } else {
                let mut changed = original.clone();
                if ending == "pause" {
                    changed["status"] = json!("paused");
                } else {
                    changed["seekSerial"] = json!(3);
                }
                updates_tx.send_replace(changed);
            }
            let Poll::Ready(reason) = waiting.as_mut().poll(&mut context) else {
                panic!("identity change must invalidate immediately");
            };
            assert_eq!(
                reason,
                if ending == "cancel" {
                    "Audio identification was cancelled"
                } else {
                    "Playback changed during identification"
                }
            );
        }
    }

    #[test]
    fn radio_identification_ignores_song_metadata_but_rejects_changed_audio_identity() {
        let mut snapshot = json!({"status":"playing", "generation":7,
            "seekSerial":2, "trackId":"radio:nova", "revision":1,
            "radioNowPlaying":{"title":"old title"}});
        assert!(native_radio_identification_matches(&snapshot, "nova", 7, 2));
        snapshot["revision"] = json!(20);
        snapshot["radioNowPlaying"] = json!({"title":"new title"});
        assert!(native_radio_identification_matches(&snapshot, "nova", 7, 2));
        for changed in [
            json!({"status":"paused", "generation":7,"seekSerial":2,"trackId":"radio:nova"}),
            json!({"status":"playing", "generation":8,"seekSerial":2,"trackId":"radio:nova"}),
            json!({"status":"playing", "generation":7,"seekSerial":3,"trackId":"radio:nova"}),
            json!({"status":"playing", "generation":7,"seekSerial":2,"trackId":"radio:other"}),
        ] {
            assert!(!native_radio_identification_matches(&changed, "nova", 7, 2));
        }
    }

    #[test]
    fn native_identification_uses_available_chapter_excerpt_and_keeps_explicit_ranges() {
        assert_eq!(
            native_identification_capture_seconds(false, Some(10.0), 18.0).unwrap(),
            8.0
        );
        assert_eq!(
            native_identification_capture_seconds(false, Some(10.0), 21.0).unwrap(),
            11.0
        );
        assert_eq!(
            native_identification_capture_seconds(false, Some(10.0), 30.0).unwrap(),
            12.0
        );
        assert_eq!(
            native_identification_capture_seconds(true, Some(10.0), 18.0).unwrap(),
            12.0
        );
        assert!(native_identification_capture_seconds(false, Some(10.0), 12.99).is_err());
    }

    #[test]
    fn detection_intervals_are_normalized_to_the_shared_source_clock() {
        assert_eq!(
            source_absolute_detection_interval(12.0, 8.0, 300.0),
            Some((312.0, 8.0))
        );
        assert_eq!(
            source_absolute_detection_interval(12.0, 8.0, 0.0),
            Some((12.0, 8.0))
        );
        assert_eq!(source_absolute_detection_interval(12.0, 0.0, 300.0), None);
    }

    #[test]
    fn native_identification_temporary_wav_is_removed_after_request_cleanup() {
        let path = std::env::temp_dir().join(format!(
            "burrata-native-identification-test-{}.wav",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"fixture").unwrap();
        drop(TemporaryPcmFile(path.clone()));
        assert!(!path.exists());
    }

    #[test]
    fn native_queue_uses_shared_unicode_collation_equal_pairs() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/track-sort-conformance.json"
        ))
        .unwrap();
        let collator = crate::track_sort_text::collator().unwrap();
        for pair in fixture["equalPairs"].as_array().unwrap() {
            assert_eq!(
                collator.compare(pair[0].as_str().unwrap(), pair[1].as_str().unwrap()),
                std::cmp::Ordering::Equal
            );
        }
    }

    #[test]
    fn steady_buffer_watch_is_scoped_to_owned_post_start_buffering_state() {
        let started = Instant::now();
        assert_eq!(
            steady_buffering_deadline(started),
            started + Duration::from_secs(5)
        );
        let mut state = State::default();
        state.generation = 42;
        state.seek_serial = 3;
        state.buffering_count = 7;
        state.status = "buffering".into();
        state.first_audio_ms = Some(1_372);
        state.output_ready = true;
        state.steady_http_stream = true;
        state.duration = 181.4;
        state.candidate = Some(Candidate::default());
        let watched = (42, 3, 7);
        assert!(steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));

        state.status = "playing".into(); // A short buffering gap recovered naturally.
        assert!(!steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
        state.status = "buffering".into();
        assert!(!steady_buffering_watch_matches(
            &state, 43, watched.1, watched.2
        ));
        assert!(!steady_buffering_watch_matches(
            &state,
            watched.0,
            watched.1 + 1,
            watched.2
        ));
        assert!(!steady_buffering_watch_matches(
            &state,
            watched.0,
            watched.1,
            watched.2 + 1
        ));

        state.buffering_count = watched.2;
        state.pending_seek_generation = Some(watched.0);
        assert!(!steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
        state.pending_seek_generation = None;
        state.output_ready = false;
        assert!(!steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
        state.output_ready = true;
        state.first_audio_ms = Some(1_372);
        state.steady_http_stream = false; // Local, radio, or unqualified source.
        assert!(!steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
        state.steady_http_stream = true;
        state.station = Some(
            serde_json::from_value(json!({
                "id": "radio-test",
                "name": "Test station",
                "streamUrl": "https://radio.example/live",
                "sourceUrl": "https://radio.example/"
            }))
            .unwrap(),
        );
        assert!(steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
        state.station = None;
        state.duration = 0.;
        assert!(steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
        state.duration = 181.4;
        state.first_audio_ms = None;
        assert!(!steady_buffering_watch_matches(
            &state, watched.0, watched.1, watched.2
        ));
    }

    #[test]
    fn steady_buffer_watch_requires_known_http_stream_and_excludes_radio_local_or_unknown() {
        assert!(steady_http_stream(
            "https://media.example/audio",
            false,
            Some(181.4)
        ));
        assert!(steady_http_stream(
            "http://media.example/audio",
            false,
            Some(181.4)
        ));
        assert!(!steady_http_stream(
            "/library/audio.flac",
            false,
            Some(181.4)
        ));
        assert!(!steady_http_stream(
            "file:///library/audio.flac",
            false,
            Some(181.4)
        ));
        assert!(!steady_http_stream(
            "https://radio.example/live",
            true,
            Some(181.4)
        ));
        assert!(!steady_http_stream(
            "https://media.example/audio",
            false,
            None
        ));
        assert!(!steady_http_stream(
            "https://media.example/audio",
            false,
            Some(0.0)
        ));
        assert!(!steady_http_stream(
            "https://media.example/audio",
            false,
            Some(f64::NAN)
        ));
    }

    #[tokio::test]
    async fn steady_buffer_wait_expires_only_while_owner_is_still_buffering() {
        let (_sender, updates) = watch::channel(json!({"revision": 0}));
        assert!(
            wait_for_steady_buffering_deadline(
                updates,
                Instant::now() - Duration::from_millis(1),
                || true,
            )
            .await
        );

        let (_sender, updates) = watch::channel(json!({"revision": 0}));
        assert!(
            !wait_for_steady_buffering_deadline(
                updates,
                Instant::now() - Duration::from_millis(1),
                || false, // Already resumed or replaced before the deadline task ran.
            )
            .await
        );
    }

    #[tokio::test]
    async fn steady_buffer_wait_exits_on_closed_updates_or_changed_owner_without_sleeping() {
        let (sender, updates) = watch::channel(json!({"revision": 0}));
        drop(sender);
        assert!(
            !wait_for_steady_buffering_deadline(
                updates,
                Instant::now() + Duration::from_secs(60),
                || true,
            )
            .await
        );

        let (sender, updates) = watch::channel(json!({"revision": 0}));
        let current = Arc::new(Mutex::new(true));
        let observed = current.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let first_check = Mutex::new(Some(entered_tx));
        let waiter = tokio::spawn(async move {
            wait_for_steady_buffering_deadline(
                updates,
                Instant::now() + Duration::from_secs(60),
                move || {
                    if let Some(entered) = first_check.lock().unwrap().take() {
                        let _ = entered.send(());
                    }
                    *observed.lock().unwrap()
                },
            )
            .await
        });
        entered_rx.await.unwrap();
        *current.lock().unwrap() = false; // New buffering count, seek, pause, or replacement.
        sender.send_replace(json!({"revision": 1}));
        assert!(!waiter.await.unwrap());
    }

    #[test]
    fn steady_buffer_timeout_respects_existing_bounded_recovery_limit() {
        let mut state = State::default();
        state.candidate = Some(Candidate::default());
        state.first_audio_ms = Some(100);
        assert!(can_retry_failed_stream(
            &state,
            "Audio stalled for 20 seconds after playback started. Check the connection or choose another source."
        ));
        state.recovery = 1;
        assert!(can_retry_failed_stream(
            &state,
            "Audio stalled for 20 seconds after playback started. Check the connection or choose another source."
        ));
        state.recovery = 2;
        assert!(!can_retry_failed_stream(
            &state,
            "Audio stalled for 20 seconds after playback started. Check the connection or choose another source."
        ));
    }

    #[test]
    fn startup_retries_share_the_original_resolution_deadline() {
        let first_request = Instant::now() - Duration::from_secs(18);
        let deadline = startup_deadline(first_request, false);
        let retry_deadline = startup_deadline(first_request, false);
        let fresh_retry_deadline = startup_deadline(first_request, true);

        assert_eq!(deadline, retry_deadline);
        assert!(deadline <= Instant::now() + Duration::from_secs(3));
        assert!(fresh_retry_deadline > deadline + Duration::from_secs(15));
    }

    #[test]
    fn ready_seek_gets_a_fresh_generation_scoped_deadline_but_paused_seek_waits_for_resume() {
        let old_startup = Instant::now() - Duration::from_secs(30);
        assert!(startup_deadline(old_startup, false) < Instant::now());

        let seek_started = Instant::now();
        let deadline = ready_seek_watchdog_deadline("playing", seek_started).unwrap();
        assert!(deadline > Instant::now() + Duration::from_secs(15));
        assert_eq!(ready_seek_watchdog_deadline("paused", seek_started), None);

        assert!(seek_watchdog_waiting_for_audio(42, "buffering", 42, 1, 1));
        assert!(!seek_watchdog_waiting_for_audio(42, "playing", 42, 1, 1));
        assert!(!seek_watchdog_waiting_for_audio(42, "paused", 42, 1, 1));
        // Stop or another playback action advances generation; the old waiter
        // must then exit without expiring or stopping the new session.
        assert!(!seek_watchdog_waiting_for_audio(43, "buffering", 42, 1, 1));
        // A pause/resume can retain the generation but still starts a fresh
        // watchdog epoch, so an old deadline cannot expire the resumed seek.
        assert!(!seek_watchdog_waiting_for_audio(42, "buffering", 42, 2, 1));
    }

    #[tokio::test]
    async fn paused_seek_resume_invalidates_old_watchdog_and_starts_a_fresh_wait() {
        let (sender, updates) = watch::channel(json!({"revision": 0}));
        let current = Arc::new(Mutex::new((42, "buffering".to_owned(), 1)));
        let observed = current.clone();
        let (checked_tx, checked_rx) = tokio::sync::oneshot::channel();
        let first_check = Mutex::new(Some(checked_tx));
        let old_waiter = tokio::spawn(async move {
            wait_for_ready_seek_audio(
                updates,
                42,
                1,
                Instant::now() + Duration::from_secs(5),
                move || {
                    if let Some(tx) = first_check.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    observed.lock().unwrap().clone()
                },
            )
            .await
        });

        checked_rx.await.unwrap();
        *current.lock().unwrap() = (42, "paused".into(), 2);
        sender.send_replace(json!({"revision": 1}));
        *current.lock().unwrap() = (42, "buffering".into(), 3);
        sender.send_replace(json!({"revision": 2}));

        assert!(!old_waiter.await.unwrap());

        let (_fresh_sender, fresh_updates) = watch::channel(json!({"revision": 2}));
        let fresh_state = current.clone();
        assert!(
            wait_for_ready_seek_audio(
                fresh_updates,
                42,
                3,
                Instant::now() + Duration::from_millis(10),
                move || fresh_state.lock().unwrap().clone(),
            )
            .await
        );
    }

    #[test]
    fn native_operation_ids_cover_startup_seek_and_transitions_without_reusing_ids() {
        let startup = ensure_command_operation_id(None, "play").unwrap();
        let seek = ensure_command_operation_id(None, "seek").unwrap();
        let mut state = State::default();
        begin_operation(
            &mut state,
            None,
            "transition",
            Instant::now(),
            None,
            None,
            None,
        );
        let transition = state.operation_id.clone().unwrap();

        assert!(startup.starts_with("native-"));
        assert!(seek.starts_with("native-"));
        assert!(transition.starts_with("native-"));
        assert_ne!(startup, seek);
        assert_ne!(startup, transition);
        assert_ne!(seek, transition);
        let trace = crate::playback_sources::SourceTrace::new(
            Some(startup.clone()),
            Some(seek.clone()),
            None,
        )
        .payload("resolution_start", "youtube", json!({}));
        assert_eq!(trace["nativeOperationId"], startup);
        assert_eq!(trace["nativeSeekOperationId"], seek);
    }

    #[test]
    fn explicit_operation_ids_are_preserved_for_commands_and_transitions() {
        let explicit = "client:op.42".to_owned();
        assert_eq!(
            ensure_command_operation_id(Some(explicit.clone()), "seek"),
            Some(explicit.clone())
        );
        let mut state = State::default();
        begin_operation(
            &mut state,
            Some(&explicit),
            "startup",
            Instant::now(),
            None,
            None,
            None,
        );
        assert_eq!(state.operation_id.as_deref(), Some(explicit.as_str()));
        assert_eq!(
            parse_operation_id(Some(&json!(explicit))).unwrap(),
            Some(explicit.clone())
        );
        let payload = crate::playback_sources::SourceTrace::new(Some(explicit.clone()), None, None)
            .payload("resolution_start", "youtube", json!({}));
        assert_eq!(payload["nativeOperationId"], explicit);
    }

    #[test]
    fn stopped_and_finished_seek_stays_paused_and_resumes_chosen_position() {
        for status in ["idle", "ended"] {
            for output_ready in [false, true] {
                let mut state = State::default();
                state.queue = vec![Track {
                    id: "track".into(),
                    duration_seconds: Some(120.),
                    ..Default::default()
                }];
                state.entries = vec!["occurrence".into()];
                state.status = status.into();
                state.output_ready = output_ready;
                state.duration = 120.;
                state.position = 120.;
                state.natural_queue_end = true;
                assert_eq!(state.snapshot(true, true)["canSeek"], json!(true));
                assert!(deferred_seek_status(status, output_ready));
                // execute cancels the old output before this state helper.
                state.output_ready = false;
                state.generation = 2;
                defer_paused_seek(&mut state);
                state.position = 37.;
                assert_eq!(state.status, "paused");
                assert!(!state.natural_queue_end);
                assert_eq!(state.resume_target(), (37., false, true));
                assert_eq!(state.entries[0], "occurrence");
                assert!(state.seek_first_audio_ms.is_none());
            }
        }
        assert!(!deferred_seek_status("failed", false));
        assert!(!deferred_seek_status("playing", false));
        assert!(!deferred_seek_status("paused", true));
    }

    #[test]
    fn paused_not_ready_seek_retains_context_until_restarted_audio_is_consumed() {
        for interrupted_status in ["buffering", "resolving"] {
            let mut state = State::default();
            state.status = interrupted_status.into();
            state.generation_counter = 6;
            state.generation = 6;
            state.request_started = Instant::now() - Duration::from_secs(60);
            // Pause cancels unready output. A subsequent deferred seek cancels
            // again to invalidate callbacks, but cannot start output while paused.
            state.status = "paused".into();
            state.output_ready = false;
            state.generation_counter += 1;
            state.generation = state.generation_counter;
            let received = Instant::now() - Duration::from_secs(10);
            record_seek_context(
                &mut state,
                Some("native-deferred-seek".into()),
                received,
                Some(1000),
                Some(12),
                Some(88),
            );
            defer_paused_seek(&mut state);
            assert_eq!(state.status, "paused");
            assert!(!state.output_ready);
            assert_eq!(state.pending_seek_generation, Some(7));
            assert!(state.seek_requested.is_none());
            assert!(state.seek_first_audio_ms.is_none());

            let resumed = Instant::now();
            arm_resumed_seek_metrics(&mut state, resumed);
            // A restored paused queue may have no session. Its startup metric
            // reset must not erase this freshly armed seek request.
            assert!(state.session.is_empty());
            reset_seek_metrics_for_start(&mut state, true);
            assert_eq!(state.seek_requested, Some(resumed));
            assert_eq!(restart_deadline(&state), operation_deadline(resumed));
            assert!(restart_deadline(&state) > startup_deadline(state.request_started, false));
            note_audio_started(&mut state);
            assert!(state.seek_first_audio_ms.is_some());
            assert!(state.seek_operation_first_audio_ms.is_some());
            assert_eq!(state.seek_operation_received_at, Some(received));
            assert_eq!(
                state.seek_operation_id.as_deref(),
                Some("native-deferred-seek")
            );
            assert_eq!(state.seek_client_started_at_ms, Some(1000));
            assert_eq!(state.seek_click_to_ipc_ms, Some(12));
            assert_eq!(state.seek_command_bytes, Some(88));
        }
    }

    #[test]
    fn native_decoder_diagnostic_events_share_generation_and_operation_without_media_metadata() {
        assert!(crate::playback_diagnostics::phase_is_allowed(
            "decoder_start"
        ));
        assert!(crate::playback_diagnostics::phase_is_allowed("decoder_end"));
        let context = NativeDecoderDiagnosticContext {
            resolution_id: uuid::Uuid::new_v4().to_string(),
            native_operation_id: Some("native-operation-123".into()),
            provider: "youtube".into(),
            generation: 27,
            request_started: Instant::now() - Duration::from_millis(40),
            decoder_started: Instant::now() - Duration::from_millis(12),
        };
        let start = native_decoder_diagnostic_payload(&context, "decoder_start", None, None);
        let end = native_decoder_diagnostic_payload(
            &context,
            "decoder_end",
            Some("success"),
            Some("decoder"),
        );
        assert_eq!(start["resolutionId"], context.resolution_id);
        assert_eq!(end["resolutionId"], context.resolution_id);
        assert_eq!(start["nativeOperationId"], "native-operation-123");
        assert_eq!(end["nativeOperationId"], start["nativeOperationId"]);
        assert_eq!(start["details"]["generation"], 27);
        assert_eq!(end["details"]["generation"], 27);
        assert_eq!(start["phase"], "decoder_start");
        assert_eq!(end["phase"], "decoder_end");
        assert_eq!(end["details"]["outcome"], "success");
        assert_eq!(end["details"]["category"], "decoder");
        let encoded = serde_json::to_string(&json!([start, end])).unwrap();
        assert!(!encoded.contains("title"));
        assert!(!encoded.contains("url"));
        assert!(!encoded.contains("error"));
    }

    #[test]
    fn new_start_clears_old_seek_timer_but_resume_without_seek_does_not_fabricate_one() {
        let mut state = State::default();
        state.session = "existing-prior-track-session".into();
        state.seek_requested = Some(Instant::now() - Duration::from_secs(60));
        state.seek_first_audio_ms = Some(3);
        reset_seek_metrics_for_start(&mut state, false);
        assert!(state.seek_requested.is_none());
        assert!(state.seek_first_audio_ms.is_none());
        assert_eq!(
            restart_deadline(&state),
            startup_deadline(state.request_started, false)
        );
        reset_seek_metrics_for_start(&mut state, true);
        assert!(state.seek_requested.is_none());
    }

    #[test]
    fn paused_seek_records_metrics_only_after_resume_and_consumption() {
        let mut state = State::default();
        state.generation = 42;
        state.pending_seek_generation = Some(42);
        state.seek_operation_id = Some("native-paused-seek".into());
        let received_at = Instant::now() - Duration::from_secs(10);
        state.seek_operation_received_at = Some(received_at);
        assert!(state.seek_requested.is_none());
        assert!(state.seek_first_audio_ms.is_none());
        assert!(state.seek_operation_first_audio_ms.is_none());

        let resumed_at = Instant::now();
        arm_resumed_seek_metrics(&mut state, resumed_at);
        assert_eq!(state.seek_requested, Some(resumed_at));
        // Resume itself is not mixer consumption and cannot publish audio metrics.
        assert!(state.seek_first_audio_ms.is_none());
        assert!(state.seek_operation_first_audio_ms.is_none());
        arm_resumed_seek_metrics(&mut state, Instant::now());
        assert_eq!(state.seek_requested, Some(resumed_at));
        note_audio_started(&mut state);
        assert!(state.seek_requested.is_none());
        assert!(state.seek_first_audio_ms.is_some());
        assert!(state.seek_operation_first_audio_ms.unwrap() >= 10_000);
        assert_eq!(state.seek_operation_received_at, Some(received_at));
        assert_eq!(
            state.seek_operation_id.as_deref(),
            Some("native-paused-seek")
        );
    }

    #[test]
    fn ordinary_resume_and_stale_pending_seek_do_not_arm_seek_metrics() {
        let mut state = State::default();
        state.generation = 42;
        arm_resumed_seek_metrics(&mut state, Instant::now());
        assert!(state.seek_requested.is_none());
        state.pending_seek_generation = Some(41);
        arm_resumed_seek_metrics(&mut state, Instant::now());
        assert!(state.seek_requested.is_none());
    }

    #[test]
    fn seek_operation_latency_includes_receipt_wait_without_changing_decoder_latency() {
        let now = Instant::now();
        let mut state = State::default();
        state.seek_operation_received_at = Some(now - std::time::Duration::from_millis(50));
        state.seek_requested = Some(now - std::time::Duration::from_millis(10));
        note_audio_started(&mut state);
        let decoder = state.seek_first_audio_ms.unwrap();
        let complete = state.seek_operation_first_audio_ms.unwrap();
        assert!(decoder >= 10);
        assert!(complete >= decoder + 39);
    }

    #[test]
    fn operation_ids_are_bounded_and_snapshot_traces_startup_and_seek_phases() {
        assert_eq!(
            parse_operation_id(Some(&json!("op-123"))).unwrap(),
            Some("op-123".into())
        );
        assert!(parse_operation_id(Some(&json!("bad id"))).is_err());
        assert!(parse_operation_id(Some(&json!("x".repeat(81)))).is_err());
        assert_eq!(
            parse_client_started_at_ms(Some(&json!(123.5))).unwrap(),
            Some(124)
        );
        assert!(parse_client_started_at_ms(Some(&json!("123"))).is_err());

        let mut state = State::default();
        begin_operation(
            &mut state,
            Some("op-123"),
            "startup",
            Instant::now(),
            Some(100),
            Some(25),
            Some(256),
        );
        state.operation_source_ready_ms = Some(40);
        state.operation_first_audio_ms = Some(95);
        state.seek_operation_id = Some("seek-456".into());
        state.seek_operation_received_at = Some(Instant::now());
        state.seek_first_audio_ms = Some(12);
        state.seek_command_bytes = Some(64);
        state.seek_operation_first_audio_ms = Some(16);
        let snapshot = state.snapshot(true, false);

        assert_eq!(snapshot["diagnostics"]["operationId"], "op-123");
        assert_eq!(snapshot["diagnostics"]["operationKind"], "startup");
        assert_eq!(snapshot["diagnostics"]["clientStartedAtMs"], 100);
        assert_eq!(snapshot["diagnostics"]["clickToIpcMs"], 25);
        assert_eq!(snapshot["diagnostics"]["commandSerializedBytes"], 256);
        assert_eq!(
            snapshot["diagnostics"]["operationTrace"][0]["phase"],
            "ipc_received"
        );
        assert_eq!(snapshot["diagnostics"]["operationTrace"][1]["atMs"], 40);
        assert_eq!(snapshot["diagnostics"]["operationTrace"][2]["atMs"], 95);
        assert_eq!(snapshot["diagnostics"]["seekOperationId"], "seek-456");
        assert_eq!(snapshot["diagnostics"]["seekCommandSerializedBytes"], 64);
        assert_eq!(snapshot["diagnostics"]["seekTrace"][1]["atMs"], 16);
        assert_eq!(snapshot["diagnostics"]["seekFirstAudioMs"], 12);
        assert_eq!(snapshot["diagnostics"]["seekOperationFirstAudioMs"], 16);
    }

    #[test]
    fn bulk_removal_preserves_occurrence_and_rejects_stale_or_invalid_sets_atomically() {
        let mut state = State::default();
        state.queue = vec![Track::default(); 5];
        state.entries = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        state.index = 2;
        remove_queue_occurrences(&mut state, &[4, 0, 4, 3], Some(0)).unwrap();
        assert_eq!(state.entries, ["b", "c"]);
        assert_eq!(state.index, 1);
        assert_eq!(state.queue_revision, 1);
        let before = state.snapshot(true, true);
        assert!(remove_queue_occurrences(&mut state, &[0], Some(0)).is_err());
        assert!(remove_queue_occurrences(&mut state, &[0, 1], Some(1)).is_err());
        assert!(remove_queue_occurrences(&mut state, &[0, 99], Some(1)).is_err());
        assert_eq!(state.snapshot(true, true), before);
        remove_queue_occurrences(&mut state, &[0], Some(1)).unwrap();
        assert_eq!(state.entries, ["c"]);
        assert_eq!(state.index, 0);
    }

    #[test]
    fn guarded_reorders_reject_competing_revision_before_any_mutation() {
        let mut state = State::default();
        state.queue = vec![Track::default(); 3];
        state.entries = vec!["a".into(), "b".into(), "c".into()];
        state.index = 1;
        let shared = std::sync::Arc::new(std::sync::Mutex::new(state));
        let gate = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|competitor| {
                let shared = shared.clone();
                let gate = gate.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    reorder_queue_state(
                        &mut shared.lock().unwrap(),
                        if competitor == 0 { 2 } else { 0 },
                        if competitor == 0 { 0 } else { 2 },
                        Some(0),
                    )
                })
            })
            .collect();
        gate.wait();
        assert_eq!(
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(Result::is_ok)
                .count(),
            1
        );
        let mut state = shared.lock().unwrap();
        assert!(state.entries == ["c", "a", "b"] || state.entries == ["b", "c", "a"]);
        let before = state.snapshot(true, true);
        assert!(reorder_queue_state(&mut state, 0, 2, Some(0)).is_err());
        assert_eq!(state.snapshot(true, true), before);
        let revision = state.queue_revision;
        let (from, to) = if state.entries[0] == "c" {
            (0, 2)
        } else {
            (2, 0)
        };
        reorder_queue_state(&mut state, from, to, Some(revision)).unwrap();
        assert_eq!(state.entries, ["a", "b", "c"]);
        assert_eq!(state.index, 1);
        assert_eq!(state.queue_revision, 2);
        reorder_queue_state(&mut state, 0, 1, None).unwrap();
        assert_eq!(state.queue_revision, 3);
    }

    #[test]
    fn queue_snapshot_pages_are_bounded_and_revision_tracks_queue_mutations() {
        let mut state = State::default();
        state.queue = (0..500)
            .map(|index| Track {
                id: format!("track-{index}"),
                title: format!("Track {index}"),
                ..Track::default()
            })
            .collect();
        state.entries = (0..500).map(|index| format!("entry-{index}")).collect();
        let first = state.snapshot_page(true, 0, 500);
        assert_eq!(
            first["queue"].as_array().unwrap().len(),
            MAX_PLAYBACK_QUEUE_PAGE
        );
        assert_eq!(
            first["queueEntryIds"].as_array().unwrap().len(),
            MAX_PLAYBACK_QUEUE_PAGE
        );
        assert_eq!(first["queueTotal"], 500);
        assert_eq!(first["queueOffset"], 0);
        assert_eq!(first["queueLimit"], MAX_PLAYBACK_QUEUE_PAGE);
        assert_eq!(first["queueHasMore"], true);
        assert_eq!(first["queue"][0]["id"], "track-0");
        assert_eq!(first["queueRevision"], 0);

        let next = state.snapshot_page(true, 200, 50);
        assert_eq!(next["queue"].as_array().unwrap().len(), 50);
        assert_eq!(next["queue"][0]["id"], "track-200");
        assert_eq!(next["queueEntryIds"][0], "entry-200");
        assert_eq!(next["queueHasMore"], true);

        let session_before = persisted_session_key(&state);
        mark_queue_changed(&mut state);
        assert_eq!(state.snapshot(true, false)["queueRevision"], 1);
        let watch = state.snapshot(true, false);
        assert_eq!(watch["queueTotal"], 500);
        assert!(
            watch.get("queue").is_none(),
            "queue count metadata must not materialize the queue"
        );
        assert_ne!(persisted_session_key(&state), session_before);
        let last = state.snapshot_page(true, 499, 50);
        assert_eq!(last["queue"].as_array().unwrap().len(), 1);
        assert_eq!(last["queue"][0]["id"], "track-499");
        assert_eq!(last["queueHasMore"], false);
    }

    #[test]
    fn backup_checkpoint_values_capture_full_queue_and_occurrence_contexts() {
        let mut state = State::default();
        state.queue = (0..501)
            .map(|index| Track {
                id: format!("track-{index}"),
                title: format!("Track {index}"),
                ..Track::default()
            })
            .collect();
        state.entries = (0..501).map(|index| format!("entry-{index}")).collect();
        state.index = 320;
        state.position = 12.75;
        state.queue_revision = 9;
        let album = crate::loudness::LoudnessResourceIdentity::album(
            vec![crate::loudness::LoudnessAssetIdentity {
                asset_id: "asset-320".into(),
                file_hash: "a".repeat(64),
                range: crate::loudness::AudioRange::WHOLE_FILE,
            }],
            vec![],
        );
        state
            .loudness_queue_contexts
            .insert("entry-320".into(), album.clone());

        // The routine status snapshot deliberately omits queue materialization;
        // the backup seam has to preserve every queue occurrence itself.
        assert!(state.snapshot(true, false).get("queue").is_none());
        let (queue, session) = state.backup_checkpoint_values();
        assert_eq!(queue["tracks"].as_array().unwrap().len(), 501);
        assert_eq!(queue["entryIds"].as_array().unwrap().len(), 501);
        assert_eq!(queue["entryIds"][500], "entry-500");
        assert_eq!(
            queue["queueLoudnessContexts"]["entry-320"]["scope"],
            "album"
        );
        assert_eq!(session["index"], 320);
        assert_eq!(session["currentEntryId"], "entry-320");
        assert_eq!(session["position"], 12.75);
        assert_eq!(session["queueRevision"], 9);
        assert_eq!(
            serde_json::from_value::<HashMap<String, crate::loudness::LoudnessResourceIdentity>>(
                queue["queueLoudnessContexts"].clone()
            )
            .unwrap()
            .get("entry-320"),
            Some(&album)
        );
    }

    fn chapter_track() -> Track {
        Track {
            id: "chapter:spotify:parent:chapter-1".into(),
            provider: "chapter".into(),
            provider_id: "spotify:parent:chapter-1".into(),
            title: "Excerpt".into(),
            duration_seconds: Some(30.),
            playback_range: Some(crate::models::PlaybackRange {
                parent_track_id: "spotify:parent".into(),
                chapter_id: "chapter-1".into(),
                source_url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
                start_time: 120.,
                end_time: 150.,
                parent_title: "Parent recording".into(),
                parent_duration_seconds: Some(240.),
            }),
            ..Track::default()
        }
    }

    fn chapter_asset(track: &Track, processing: &str) -> crate::models::LocalAsset {
        crate::models::LocalAsset {
            id: "asset-1".into(),
            recording_id: track.id.clone(),
            path: "/music/chapter.flac".into(),
            hash: "hash".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: Some(30.),
            size_bytes: None,
            bitrate_origin: None,
            processing: processing.into(),
            track: track.clone(),
        }
    }

    #[test]
    fn queue_end_repeat_and_previous() {
        assert_eq!(queue_next(2, 1, "off", 1, true), None);
        assert_eq!(queue_next(2, 1, "all", 1, true), Some(0));
        assert_eq!(queue_next(2, 0, "one", 1, true), Some(0));
        assert_eq!(queue_next(2, 0, "one", 1, false), Some(1));
        assert_eq!(queue_next(2, 0, "off", -1, false), Some(0));
    }

    #[test]
    fn natural_queue_end_marker_only_enables_autoplay_after_repeat_off_exhaustion() {
        // The App uses this snapshot marker as its authorization to request
        // automatic queue radio; routine track ends must never set it.
        assert!(is_natural_queue_end(1, 0, "off", true));
        assert!(!is_natural_queue_end(2, 0, "off", true));
        assert!(!is_natural_queue_end(1, 0, "all", true));
        assert!(!is_natural_queue_end(1, 0, "one", true));
        assert!(!is_natural_queue_end(1, 0, "off", false));
        assert!(!is_natural_queue_end(0, 0, "off", true));
    }

    #[test]
    fn sorting_upcoming_preserves_played_prefix_current_and_duplicate_occurrences() {
        let mut state = State::default();
        state.queue = vec![
            Track {
                id: "played".into(),
                ..Track::default()
            },
            Track {
                id: "current".into(),
                ..Track::default()
            },
            Track {
                id: "long".into(),
                title: "Long".into(),
                duration_seconds: Some(200.),
                ..Track::default()
            },
            Track {
                id: "same".into(),
                title: "Same".into(),
                duration_seconds: Some(90.),
                ..Track::default()
            },
            Track {
                id: "same".into(),
                title: "Same".into(),
                duration_seconds: None,
                ..Track::default()
            },
        ];
        state.entries = ["p", "c", "a", "b", "d"].map(str::to_owned).to_vec();
        state.index = 1;

        let collator = crate::track_sort_text::collator().unwrap();
        sort_upcoming(&mut state, "duration", false, collator);

        assert_eq!(state.queue[0].id, "played");
        assert_eq!(state.queue[1].id, "current");
        assert_eq!(state.entries, ["p", "c", "b", "a", "d"]);
        assert_eq!(state.queue[2].duration_seconds, Some(90.));
        assert_eq!(state.queue[4].duration_seconds, None);
    }

    #[test]
    fn native_sort_matches_shared_zero_unknown_rules_and_keeps_reported_count_zero() {
        let collator = crate::track_sort_text::collator().unwrap();
        let zero_duration = Track {
            duration_seconds: Some(0.),
            ..Track::default()
        };
        let positive_duration = Track {
            duration_seconds: Some(10.),
            ..Track::default()
        };
        let zero_bpm = Track {
            bpm: Some(0.),
            ..Track::default()
        };
        let positive_bpm = Track {
            bpm: Some(100.),
            ..Track::default()
        };
        let zero_count = Track {
            provenance: json!({"plays": 0}),
            ..Track::default()
        };
        let unknown_count = Track::default();

        for descending in [false, true] {
            assert_eq!(
                compare_track_sort_values(
                    &zero_duration,
                    &positive_duration,
                    "duration",
                    descending,
                    collator
                ),
                std::cmp::Ordering::Greater,
                "zero duration is unknown and stays last",
            );
            assert_eq!(
                compare_track_sort_values(&zero_bpm, &positive_bpm, "bpm", descending, collator),
                std::cmp::Ordering::Greater,
                "zero BPM is unknown and stays last",
            );
            assert_eq!(
                compare_track_sort_values(
                    &zero_count,
                    &unknown_count,
                    "plays",
                    descending,
                    collator
                ),
                std::cmp::Ordering::Less,
                "reported count zero remains known in both directions",
            );
        }
    }

    #[test]
    fn native_sort_values_match_shared_duration_key_and_nested_count_rules() {
        let major_c = camelot_sort_value("C Major").unwrap();
        let camelot_8b = camelot_sort_value("8B").unwrap();
        let minor_a = camelot_sort_value("A minor").unwrap();
        let camelot_8a = camelot_sort_value("8A").unwrap();
        assert!(
            matches!((major_c, camelot_8b), (TrackSortValue::Number(a), TrackSortValue::Number(b)) if a == b)
        );
        assert!(
            matches!((minor_a, camelot_8a), (TrackSortValue::Number(a), TrackSortValue::Number(b)) if a == b)
        );
        assert!(
            matches!(camelot_sort_value("A"), Some(TrackSortValue::Number(value)) if value == 18.)
        );
        let collator = crate::track_sort_text::collator().unwrap();
        assert_eq!(
            collator.compare("Track 2", "Track 10"),
            std::cmp::Ordering::Less
        );

        let short = Track {
            duration_seconds: Some(91.9),
            provenance: json!({ "remoteCatalog": { "play_count": "1.2k plays" } }),
            ..Track::default()
        };
        let long = Track {
            duration_seconds: Some(92.1),
            provenance: json!({ "remoteCatalog": { "play_count": "2,000" } }),
            ..Track::default()
        };
        assert_eq!(
            compare_track_sort_values(&short, &long, "duration", false, collator),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_track_sort_values(&short, &long, "plays", false, collator),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn skipped_source_failure_diagnostic_survives_next_audio_without_marking_it_failed() {
        let mut state = State::default();
        state.queue = vec![
            Track {
                id: "sasa".into(),
                provider: "spotify".into(),
                provider_id: "track-234".into(),
                title: "Sasa".into(),
                artist: "Timmy Regisford".into(),
                version: Some("Main Version (4:41)".into()),
                duration_seconds: Some(281.),
                source_url: "https://private.example/source".into(),
                ..Track::default()
            },
            Track {
                id: "next".into(),
                ..Track::default()
            },
        ];
        state.index = 1;
        state.last_skipped_track_failure = Some(source_failure_snapshot(
            &state.queue[0],
            "No playable audio match was found for Sasa — Timmy Regisford. Try https://private.example/audio",
        ));

        let snapshot = state.snapshot(true, false);
        assert_eq!(
            snapshot["diagnostics"]["lastSkippedTrackFailure"]["trackId"],
            "sasa"
        );
        assert_eq!(
            snapshot["diagnostics"]["lastSkippedTrackFailure"]["version"],
            "Main Version (4:41)"
        );
        assert_eq!(
            snapshot["diagnostics"]["lastSkippedTrackFailure"]["providerId"],
            "track-234"
        );
        assert!(snapshot["diagnostics"]["lastSkippedTrackFailure"]["reason"]
            .as_str()
            .is_some_and(|reason| !reason.contains("private.example")));

        note_audio_started(&mut state);
        assert!(state.last_source_failure.is_none());
        assert!(state.last_skipped_track_failure.is_some());
        assert_eq!(snapshot["trackId"], "next");
        assert!(snapshot["diagnostics"]["lastSourceFailure"].is_null());
    }

    #[test]
    fn starting_a_new_queue_clears_current_and_skipped_failure_diagnostics() {
        let mut state = State::default();
        let failure = source_failure_snapshot(&Track::default(), "No playable audio source");
        state.last_source_failure = Some(failure.clone());
        state.last_skipped_track_failure = Some(failure);

        clear_playback_failures(&mut state);

        let snapshot = state.snapshot(true, false);
        assert!(snapshot["diagnostics"]["lastSourceFailure"].is_null());
        assert!(snapshot["diagnostics"]["lastSkippedTrackFailure"].is_null());
    }

    #[test]
    fn manual_track_changes_clear_current_error_but_keep_queue_skip_context() {
        let mut state = State::default();
        state.last_source_failure = Some(source_failure_snapshot(
            &Track::default(),
            "Current track failed",
        ));
        state.last_skipped_track_failure = Some(source_failure_snapshot(
            &Track {
                id: "spotify:skipped".into(),
                provider: "spotify".into(),
                provider_id: "skipped".into(),
                ..Track::default()
            },
            "Earlier queue item had no playable source",
        ));

        clear_current_source_failure(&mut state);

        let snapshot = state.snapshot(true, false);
        assert!(snapshot["diagnostics"]["lastSourceFailure"].is_null());
        assert_eq!(
            snapshot["diagnostics"]["lastSkippedTrackFailure"]["providerId"],
            "skipped"
        );
    }

    #[test]
    fn recent_track_failure_history_is_bounded_owned_and_keeps_duplicate_occurrences() {
        let track = Track {
            id: "same-recording".into(),
            title: "Exact recording".into(),
            ..Track::default()
        };
        let mut state = State {
            queue: vec![track.clone(), track],
            entries: vec!["first".into(), "second".into()],
            status: "resolving".into(),
            queue_revision: 4,
            ..State::default()
        };
        for generation in 1..=70 {
            state.generation = generation;
            state.index = (generation as usize - 1) % 2;
            state.operation_id = Some(format!("retry-{generation}"));
            record_track_failure(
                &mut state,
                generation,
                "No playable audio match was found",
                "skipped",
            );
        }
        assert_eq!(state.recent_track_failures.len(), 64);
        assert_eq!(state.recent_track_failures[0]["generation"], 7);
        assert_eq!(state.recent_track_failures[62]["entryId"], "first");
        assert_eq!(state.recent_track_failures[63]["entryId"], "second");
        assert_eq!(state.recent_track_failures[63]["queueRevision"], 4);
        assert_eq!(state.recent_track_failures[63]["operationId"], "retry-70");
        record_track_failure(
            &mut state,
            70,
            "No playable audio match was found",
            "skipped",
        );
        assert_eq!(state.recent_track_failures.len(), 64);
        state.generation = 71;
        record_track_failure(&mut state, 71, "Video has been deleted", "queue_exhausted");
        assert_eq!(
            state.recent_track_failures.last().unwrap()["outcome"],
            "queue_exhausted"
        );
        note_audio_started(&mut state);
        assert_eq!(
            state.snapshot_with_failure_history(true, false)["diagnostics"]["recentTrackFailures"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        mark_queue_changed(&mut state);
        assert!(state.recent_track_failures.is_empty());
    }

    #[test]
    fn routine_native_snapshots_omit_history_while_explicit_status_and_pages_include_it() {
        let track = Track {
            id: "recording".into(),
            title: "T".repeat(160),
            artist: "A".repeat(160),
            ..Track::default()
        };
        let failure =
            source_failure_snapshot(&track, &format!("HTTP 404 unavailable {}", "r".repeat(250)));
        let mut state = State {
            queue: vec![track],
            entries: vec!["entry-a".into()],
            status: "playing".into(),
            generation: 7,
            queue_revision: 4,
            last_skipped_track_failure: Some(failure.clone()),
            ..State::default()
        };
        let empty_control = state.snapshot(true, false);
        let empty_emit = state.snapshot(true, true);
        let mut record = failure.clone();
        record["queueRevision"] = json!(4);
        record["generation"] = json!(7);
        record["entryId"] = json!("entry-a");
        record["operationId"] = json!("operation-a");
        record["outcome"] = json!("skipped");
        record["category"] = json!("failed_with_reason");
        state.recent_track_failures = vec![record; MAX_RECENT_TRACK_FAILURES];
        // Publish uses both of these projections; neither builds then strips history.
        assert_eq!(state.snapshot(true, false), empty_control);
        assert_eq!(state.snapshot(true, true), empty_emit);
        for position in 1..=100 {
            state.position = position as f64;
            let routine = state.snapshot(true, false);
            assert!(routine["diagnostics"].get("recentTrackFailures").is_none());
            assert_eq!(routine["diagnostics"]["lastSkippedTrackFailure"], failure);
            assert!(routine.to_string().len() < 5000);
        }
        let status = state.snapshot_with_failure_history(true, true);
        let page = state.snapshot_page(true, 0, 50);
        assert_eq!(
            status["diagnostics"]["recentTrackFailures"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(
            page["diagnostics"]["recentTrackFailures"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(page["diagnostics"]["lastSkippedTrackFailure"], failure);
        assert!(status.to_string().len() > 30_000);
        assert_eq!(state.recent_track_failures.len(), 64);
    }

    #[test]
    fn recent_track_failure_history_excludes_cancelled_stale_and_paused_attempts() {
        let mut state = State {
            queue: vec![Track::default()],
            entries: vec!["first".into()],
            generation: 7,
            status: "resolving".into(),
            ..State::default()
        };
        record_track_failure(&mut state, 6, "HTTP 404 unavailable", "skipped");
        record_track_failure(&mut state, 7, "Playback request cancelled", "skipped");
        state.status = "paused".into();
        record_track_failure(&mut state, 7, "HTTP 404 unavailable", "skipped");
        state.status = "resolving".into();
        let (cancel, _) = watch::channel(true);
        state.cancel = Some(cancel);
        record_track_failure(&mut state, 7, "HTTP 404 unavailable", "skipped");
        assert!(state.recent_track_failures.is_empty());
    }

    #[test]
    fn stored_failure_diagnostic_is_bounded_and_redacts_signed_sources_and_credentials() {
        let failure = source_failure_snapshot(
            &Track {
                id: "spotify:track-1".into(),
                provider: "spotify".into(),
                provider_id: "track-1".into(),
                title: "Title https://private.example/signed?sig=abc".into(),
                artist: "Artist".into(),
                ..Track::default()
            },
            &format!(
                "No match at https://private.example/audio?sig=abc token=secret-value Authorization: Bearer bearer-secret {}",
                "x".repeat(400)
            ),
        );
        let encoded = failure.to_string();
        assert_eq!(failure["providerId"], "track-1");
        assert_eq!(failure["title"], "Title [source]");
        assert!(!encoded.contains("private.example"));
        assert!(!encoded.contains("abc"));
        assert!(!encoded.contains("secret-value"));
        assert!(!encoded.contains("bearer-secret"));
        assert!(failure["reason"].as_str().unwrap().chars().count() <= 250);
        assert_eq!(
            safe_failure_identity("https://private.example/signed?token=x"),
            None
        );
    }

    #[test]
    fn exhausted_recording_failures_ignore_metadata_words_and_advance_once() {
        let mut state = State::default();
        state.queue = vec![Track::default(), Track::default()];
        state.entries = vec!["entry-a".into(), "entry-b".into()];
        state.status = "resolving".into();
        state.generation = 7;
        state.repeat = "one".into();
        for message in [
            "No playable audio match was found for Network — Connection (Cancel Mix). No available source matched the requested recording and version.",
            "The resolved audio source identity does not match this recording",
            "Source identity changed during audio resolution; select it again",
            "Source identity changed during inspection; select it again",
            "This source has no supported audio stream",
            crate::playback_sources::SOURCE_NO_RESULTS,
            crate::playback_sources::SOURCE_IDENTITY_REJECTED,
        ] {
            state.failed_occurrence_ids.clear();
            assert_eq!(
                failed_track_next_index(&mut state, 7, message),
                Ok(Some(1)),
                "{message}"
            );
            assert_eq!(
                state.index, 0,
                "caller owns transition and generation change"
            );
        }
    }
    #[test]
    fn provider_global_failures_do_not_consume_occurrence_traversal() {
        let mut state = State::default();
        state.queue = vec![Track::default(), Track::default()];
        state.entries = vec!["entry-a".into(), "entry-b".into()];
        state.status = "buffering".into();
        state.generation = 7;
        for message in [
            "Cancelled",
            "HTTP 403 unavailable",
            "HTTP 404 unavailable",
            "HTTP 410 unavailable",
            "yt-dlp failed: ERROR: Private video. Sign in if you have been granted access",
            "yt-dlp failed: ERROR: This video has been removed",
            "HTTP 429 unavailable",
            "HTTP status 503 unavailable",
            "status code 502 resource unavailable",
            "HTTP 500 unavailable",
            "HTTP 504 unavailable",
            "rate limit: video unavailable",
            "network connection unavailable",
            "source timeout: unavailable",
            "audio output device unavailable",
            "Playback startup exceeded its 20-second playback deadline",
        ] {
            assert_eq!(
                failed_track_next_index(&mut state, 7, message),
                Err(()),
                "{message}"
            );
            assert!(state.failed_occurrence_ids.is_empty());
        }
        let (cancel, _) = watch::channel(true);
        state.cancel = Some(cancel);
        assert_eq!(
            failed_track_next_index(&mut state, 7, "HTTP 403 unavailable"),
            Err(())
        );
        assert!(
            state.failed_occurrence_ids.is_empty(),
            "a cancelled resolver must not consume occurrences"
        );
    }
    #[test]
    fn distinct_resolver_outcomes_advance_and_record_the_exact_failure_category() {
        for (reason, category) in [
            (
                crate::playback_sources::SOURCE_NO_RESULTS,
                "no_audio_results",
            ),
            (
                crate::playback_sources::SOURCE_IDENTITY_REJECTED,
                "recording_identity_unverified",
            ),
            (
                crate::playback_sources::SOURCE_MISSING_DURATION,
                "recording_identity_unverified",
            ),
        ] {
            let mut state = State::default();
            state.queue = vec![
                Track {
                    id: "recording-a".into(),
                    ..Track::default()
                },
                Track {
                    id: "recording-b".into(),
                    ..Track::default()
                },
            ];
            state.entries = vec!["entry-a".into(), "entry-b".into()];
            state.status = "buffering".into();
            state.generation = 7;
            record_track_failure(&mut state, 7, reason, "skipped");
            assert_eq!(state.recent_track_failures.len(), 1);
            assert_eq!(state.recent_track_failures[0]["category"], category);
            assert_eq!(failed_track_next_index(&mut state, 7, reason), Ok(Some(1)));
            assert_eq!(
                track_failure_category(&format!("Network failure: {reason}")),
                "failed_with_reason"
            );
            assert!(!should_skip_failed_track(&format!(
                "Network failure: {reason}"
            )));
        }
    }

    #[test]
    fn failure_traversal_rechecks_cancellation_after_initial_failure_eligibility() {
        let mut state = State::default();
        state.queue = vec![Track::default(), Track::default()];
        state.entries = vec!["entry-a".into(), "entry-b".into()];
        state.status = "buffering".into();
        state.generation = 7;
        let message = "No playable audio match was found";
        assert!(
            should_skip_failed_track(message),
            "resolver result initially qualifies for an occurrence skip"
        );

        let (cancel, _cancel_rx) = watch::channel(false);
        state.cancel = Some(cancel.clone());
        cancel.send(true).unwrap();
        assert_eq!(failed_track_next_index(&mut state, 7, message), Err(()));
        assert!(
            state.failed_occurrence_ids.is_empty(),
            "cancellation before the state-locked traversal cannot consume the occurrence"
        );
        assert!(
            state.recent_track_failures.is_empty(),
            "cancellation is not queue exhaustion"
        );
    }

    #[test]
    fn track_specific_failures_skip_once_but_global_failures_stop() {
        let mut state = State::default();
        state.queue = (0..4)
            .map(|index| Track {
                id: format!("track-{index}"),
                title: format!("Track {index}"),
                ..Track::default()
            })
            .collect();
        state.entries = (0..4).map(|index| format!("entry-{index}")).collect();
        state.index = 1;
        state.generation = 7;
        state.status = "buffering".into();
        state.repeat = "one".into();

        assert_eq!(
            failed_track_next_index(&mut state, 7, "Video has been deleted"),
            Ok(Some(2))
        );
        assert_eq!(
            state.index, 1,
            "skip selection must not mutate the active index"
        );
        assert_eq!(
            state.generation, 7,
            "the caller changes generation when starting next"
        );
        assert_eq!(state.failed_occurrence_ids.len(), 1);

        for message in [
            "network connection timed out",
            "could not open audio output device",
            "Cancelled by user",
        ] {
            assert_eq!(failed_track_next_index(&mut state, 7, message), Err(()));
        }
        assert_eq!(
            failed_track_next_index(&mut state, 7, crate::playback_sources::SOURCE_NO_RESULTS),
            Ok(Some(2)),
            "only an exhausted, track-specific no-match result may advance"
        );
        state.status = "paused".into();
        assert_eq!(
            failed_track_next_index(&mut state, 7, "HTTP 404 unavailable"),
            Err(()),
            "a paused/cancelled item must not auto-advance"
        );
        state.status = "buffering".into();
        assert_eq!(
            failed_track_next_index(&mut state, 6, "HTTP 404 unavailable"),
            Err(())
        );
    }

    #[test]
    fn queued_native_pause_seek_resume_preserve_root_and_stop_releases_it() {
        let mut state = State::default();
        state.queued_operation_id = Some("album-job".into());
        for action in ["pause", "seek", "resume"] {
            finish_external_queued_owner(&mut state, action);
            assert_eq!(state.queued_operation_id.as_deref(), Some("album-job"));
        }
        assert_eq!(state.queued_intent_epoch, 2);
        finish_external_queued_owner(&mut state, "stop");
        assert!(state.queued_operation_id.is_none());
    }
    #[test]
    fn queued_native_nineteen_occurrences_retain_root_through_finite_exhaustion() {
        let mut state = State::default();
        state.queued_operation_id = Some("album-job".into());
        state.queue = (0..19)
            .map(|i| Track {
                id: if i == 1 {
                    "track-0".into()
                } else {
                    format!("track-{i}")
                },
                title: format!("Track {i}"),
                ..Default::default()
            })
            .collect();
        state.entries = (0..19).map(|i| format!("entry-{i}")).collect();
        state.status = "resolving".into();
        state.repeat = "all".into();
        for index in 0..19 {
            state.generation += 1;
            let generation = state.generation;
            begin_operation(
                &mut state,
                Some(&format!("attempt-{index}")),
                "transition",
                Instant::now(),
                None,
                None,
                None,
            );
            let next = failed_track_next_index(
                &mut state,
                generation,
                "No playable audio match was found",
            )
            .unwrap();
            assert_eq!(next, if index < 18 { Some(index + 1) } else { None });
            record_track_failure(
                &mut state,
                generation,
                "No playable audio match was found",
                if next.is_some() {
                    "skipped"
                } else {
                    "queue_exhausted"
                },
            );
            if let Some(next) = next {
                state.index = next;
            }
        }
        assert_eq!(state.failed_occurrence_ids.len(), 19);
        assert_eq!(state.recent_track_failures.len(), 19);
        assert!(state
            .recent_track_failures
            .iter()
            .all(|f| f["operationId"] == "album-job"));
        assert_eq!(
            state.recent_track_failures.last().unwrap()["outcome"],
            "queue_exhausted"
        );
        assert!(!state.natural_queue_end);
    }
    #[test]
    fn queued_root_survives_attempt_trace_transition_and_owns_failure_occurrence() {
        let mut state = State::default();
        state.status = "resolving".into();
        state.queued_operation_id = Some("album-job".into());
        state.queue = vec![Track {
            id: "same".into(),
            title: "Title".into(),
            artist: "Artist".into(),
            ..Default::default()
        }];
        state.entries = vec!["occurrence-1".into()];
        state.generation = 12;
        begin_operation(
            &mut state,
            Some("new-attempt"),
            "transition",
            Instant::now(),
            None,
            None,
            None,
        );
        record_track_failure(
            &mut state,
            12,
            "No playable audio match was found",
            "queue_exhausted",
        );
        assert_eq!(state.operation_id.as_deref(), Some("new-attempt"));
        assert_eq!(state.queued_operation_id.as_deref(), Some("album-job"));
        assert_eq!(state.recent_track_failures[0]["operationId"], "album-job");
        assert_eq!(state.recent_track_failures[0]["entryId"], "occurrence-1");
    }
    #[test]
    fn failed_track_skips_exhaust_each_occurrence_once_without_repeat_all_loops() {
        let duplicate = Track {
            id: "duplicate-track".into(),
            title: "Same track".into(),
            ..Track::default()
        };
        let mut state = State::default();
        state.queue = vec![
            duplicate.clone(),
            duplicate.clone(),
            duplicate.clone(),
            duplicate,
        ];
        state.entries = vec![
            "occurrence-a".into(),
            "occurrence-b".into(),
            "occurrence-c".into(),
            "occurrence-d".into(),
        ];
        state.generation = 4;
        state.status = "resolving".into();
        state.repeat = "all".into();

        let mut selected = Vec::new();
        for expected in [1, 2, 3] {
            let next = failed_track_next_index(&mut state, 4, "No playable audio match was found");
            assert_eq!(next, Ok(Some(expected)));
            record_track_failure(
                &mut state,
                4,
                "No playable audio match was found",
                "skipped",
            );
            selected.push(state.entries[state.index].clone());
            state.index = expected;
        }
        assert_eq!(
            failed_track_next_index(&mut state, 4, "No playable audio match was found"),
            Ok(None)
        );
        record_track_failure(
            &mut state,
            4,
            "No playable audio match was found",
            "queue_exhausted",
        );
        selected.push(state.entries[state.index].clone());
        assert_eq!(
            selected,
            [
                "occurrence-a",
                "occurrence-b",
                "occurrence-c",
                "occurrence-d"
            ]
        );
        assert_eq!(state.recent_track_failures.len(), 4);
        assert_eq!(
            state
                .recent_track_failures
                .iter()
                .map(|item| item["entryId"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "occurrence-a",
                "occurrence-b",
                "occurrence-c",
                "occurrence-d"
            ]
        );
        assert_eq!(
            state.recent_track_failures.last().unwrap()["outcome"],
            "queue_exhausted"
        );
        assert!(
            !state.natural_queue_end,
            "failed queue exhaustion must not authorize natural-end radio continuation"
        );

        note_audio_started(&mut state);
        assert!(
            state.failed_occurrence_ids.is_empty(),
            "successful audio starts a fresh failure traversal"
        );
        assert_eq!(
            failed_track_next_index(&mut state, 4, "Video has been deleted"),
            Ok(Some(0))
        );
    }

    #[test]
    fn whole_source_recovery_only_retries_after_audio_began() {
        let mut state = State::default();
        state.candidate = Some(Candidate::default());
        assert!(!can_retry_failed_stream(&state, "decoder failed"));
        state.first_audio_ms = Some(250);
        assert!(can_retry_failed_stream(&state, "decoder failed"));
        assert!(!can_retry_failed_stream(
            &state,
            "audio output device disconnected"
        ));
        assert!(!can_retry_failed_stream(&state, "Cancelled by user"));
        state.recovery = 2;
        assert!(!can_retry_failed_stream(&state, "decoder failed"));
    }

    #[test]
    fn startup_http_recovery_is_bounded_to_transport_and_original_deadline() {
        let mut state = State::default();
        state.candidate = Some(Candidate::default());
        for message in [
            "Native HTTP byte range: HTTP 403 on initial media response",
            "Native HTTP byte range: HTTP 503 on initial media response",
            "Native HTTP request failed: connection reset",
            "Native HTTP request failed: error sending request",
            "Native HTTP request failed: error decoding response body",
            "Native HTTP byte range: media response is empty",
        ] {
            assert!(
                can_retry_failed_stream_at_stage(&state, message, true),
                "{message}"
            );
            assert!(!can_retry_failed_stream_at_stage(&state, message, false));
        }
        assert!(!can_retry_failed_stream_at_stage(
            &state,
            "output device unavailable",
            true
        ));
        assert!(!can_retry_failed_stream_at_stage(&state, "Cancelled", true));
        state.recovery = 2;
        assert!(!can_retry_failed_stream_at_stage(
            &state,
            "Native HTTP byte range: HTTP 403 on initial media response",
            true
        ));
        state.recovery = 0;
        state.request_started = Instant::now() - state.startup_budget - Duration::from_secs(1);
        assert!(!can_retry_failed_stream_at_stage(
            &state,
            "Native HTTP byte range: HTTP 403 on initial media response",
            true
        ));
    }

    #[test]
    fn seek_metrics_do_not_reuse_track_start_timing() {
        let mut state = State::default();
        state.first_audio_ms = Some(12_345);
        state.seek_requested = Some(Instant::now());
        note_audio_started(&mut state);
        assert!(state.seek_first_audio_ms.is_some());
        assert!(state.seek_requested.is_none());
        assert_eq!(state.first_audio_ms, Some(12_345));
        reset_failed_resume_diagnostics(&mut state);
        assert!(state.seek_first_audio_ms.is_none());
    }

    #[test]
    fn failed_resume_clears_stale_attempt_metrics_but_paused_resume_keeps_them() {
        let mut state = State::default();
        state.status = "paused".into();
        state.recovery = 2;
        state.first_audio_ms = Some(12_345);
        state.source_ready_ms = Some(10_000);
        state.buffering_count = 4;
        let _ = state.resume_target();
        assert_eq!(state.recovery, 2);
        assert_eq!(state.first_audio_ms, Some(12_345));
        assert_eq!(state.buffering_count, 4);

        state.status = "failed".into();
        reset_failed_resume_diagnostics(&mut state);
        assert_eq!(state.recovery, 0);
        assert_eq!(state.first_audio_ms, None);
        assert_eq!(state.source_ready_ms, None);
        assert_eq!(state.buffering_count, 0);
    }

    #[test]
    fn natural_end_uses_verified_duration_not_catalog_estimates() {
        let mut state = State::default();
        state.output_ready = true;
        state.duration = 240.;
        state.position = 180.;
        assert!(
            !ended_prematurely(&state),
            "an unknown stream duration must not trigger a redundant full-source retry"
        );
        state.duration_verified = true;
        assert!(
            ended_prematurely(&state),
            "verified truncation must still recover"
        );
        state.position = 239.;
        assert!(!ended_prematurely(&state));
    }

    #[test]
    fn chapter_start_and_preload_bounds_distinguish_parent_files_from_cropped_assets() {
        let chapter = chapter_track();
        let parent_asset = crate::models::LocalAsset {
            track: Track {
                id: "spotify:parent".into(),
                provider: "spotify".into(),
                provider_id: "parent".into(),
                ..Track::default()
            },
            processing: "original audio codec preserved".into(),
            ..chapter_asset(&chapter, "original audio codec preserved")
        };
        let excerpt_asset = chapter_asset(&chapter, "excerpt");

        assert_eq!(
            source_bounds_for_asset(&chapter, Some(&parent_asset)),
            (120., Some(150.))
        );
        assert_eq!(
            source_bounds_for_asset(&chapter, Some(&excerpt_asset)),
            (0., Some(30.))
        );
        assert_eq!(source_bounds_for_asset(&chapter, None), (120., Some(150.)));

        // Both initial loads and gapless preloads call source_bounds(), which
        // delegates to this tested file-aware mapping.
        assert_eq!(
            source_bounds_for_asset(&chapter, Some(&parent_asset)).0,
            120.
        );
        assert_eq!(
            source_bounds_for_asset(&chapter, Some(&excerpt_asset)).1,
            Some(30.)
        );
    }
    #[test]
    fn play_after_queue_end_restarts_instead_of_resuming_exhausted_output() {
        let mut state = State::default();
        state.status = "ended".into();
        state.position = 190.31;
        state.duration = 190.52;
        state.output_ready = true;
        assert_eq!(state.resume_target(), (0., false, false));
        state.status = "paused".into();
        assert_eq!(state.resume_target(), (190.31, true, true));
        state.output_ready = false;
        state.position = 42.;
        assert_eq!(state.resume_target(), (42., false, true));
    }
    #[test]
    fn finished_queue_survives_restore_and_replays_current_track_from_zero() {
        let mut state = State::default();
        restore_state(&mut state, &json!({"tracks":[Track{id:"a".into(),title:"A".into(),..Default::default()}],"index":0,"position":190.31,"finished":true})).unwrap();
        assert_eq!(state.status, "ended");
        assert_eq!(state.track().unwrap().id, "a");
        assert_eq!(state.resume_target(), (0., false, false));
    }
    #[test]
    fn restore_is_paused_and_preserves_duplicate_entries() {
        let mut s = State::default();
        restore_state(&mut s,&json!({"tracks":[Track{id:"a".into(),title:"A".into(),..Default::default()},Track{id:"a".into(),title:"A".into(),..Default::default()}],"index":1,"position":12.,"volume":0.5})).unwrap();
        assert_eq!(s.status, "paused");
        assert_eq!(s.index, 1);
        assert_eq!(s.position, 12.);
        assert_ne!(s.entries[0], s.entries[1]);
    }
    #[test]
    fn checkpoint_restore_preserves_the_exact_duplicate_occurrence() {
        let duplicate = Track {
            id: "duplicate".into(),
            ..Track::default()
        };
        let mut original = State::default();
        original.queue = vec![duplicate.clone(), duplicate];
        original.entries = vec!["first-occurrence".into(), "second-occurrence".into()];
        original.queue_revision = 42;
        original.index = 1;
        original.position = 12.5;
        original.status = "playing".into();
        original
            .failed_occurrence_ids
            .insert("first-occurrence".into());
        let state = Mutex::new(original);
        let checkpoint = std::cell::RefCell::new((Value::Null, Value::Null));
        persist_playback_state(
            &state,
            &Mutex::new(()),
            |queue, session| {
                *checkpoint.borrow_mut() = (queue.unwrap().clone(), session.unwrap().clone());
                Ok::<(), ()>(())
            },
            || {},
        );
        let (queue, mut session) = checkpoint.into_inner();
        // The occurrence ID, not a stale numeric index, selects duplicate tracks.
        session["index"] = json!(0);
        merge_persisted_queue(&mut session, &queue).unwrap();
        let mut restored = State::default();
        restore_state(&mut restored, &session).unwrap();
        assert_eq!(
            restored.entries,
            vec!["first-occurrence", "second-occurrence"]
        );
        assert_eq!(restored.index, 1);
        assert_eq!(restored.queue_revision, 42);
        assert_eq!(restored.position, 12.5);
        assert_eq!(restored.status, "paused");
        assert!(restored.failed_occurrence_ids.is_empty());
        assert!(restored.recent_track_failures.is_empty());
    }

    #[test]
    fn checkpoint_restore_rejects_mixed_revisions_and_invalid_occurrences() {
        let queue = json!({
            "tracks": vec![Track { id: "duplicate".into(), ..Track::default() }; 2],
            "entryIds": ["first", "second"], "queueRevision": 42
        });
        for revision in [json!(41), json!(null), json!("42")] {
            let mut session = json!({"queueRevision": revision, "currentEntryId": "second"});
            assert!(merge_persisted_queue(&mut session, &queue).is_err());
        }
        let mut valid = json!({"queueRevision": 42, "currentEntryId": "second"});
        merge_persisted_queue(&mut valid, &queue).unwrap();
        for entries in [
            json!(["first"]),
            json!(["same", "same"]),
            json!(["", "second"]),
            json!([1, 2]),
            json!(null),
        ] {
            let mut invalid = valid.clone();
            invalid["entryIds"] = entries;
            let mut state = State::default();
            assert!(restore_state(&mut state, &invalid).is_err());
            assert!(state.queue.is_empty());
        }
        for current in [json!("absent"), json!(null), json!(1)] {
            let mut invalid = valid.clone();
            invalid["currentEntryId"] = current;
            assert!(restore_state(&mut State::default(), &invalid).is_err());
        }
    }

    #[test]
    fn checkpoint_restore_accepts_legacy_split_queue_and_session() {
        let queue = json!({"tracks": [Track { id: "legacy".into(), ..Track::default() }]});
        let mut session = json!({"index": 0, "position": 8., "finished": true});
        merge_persisted_queue(&mut session, &queue).unwrap();
        let mut restored = State::default();
        restore_state(&mut restored, &session).unwrap();
        assert_eq!(restored.status, "ended");
        assert_eq!(restored.position, 8.);
        assert_eq!(restored.track().unwrap().id, "legacy");
        assert_eq!(restored.entries.len(), 1);
    }
    #[test]
    fn candidate_artwork_is_shared_with_system_controls_without_reusing_an_old_cover() {
        let mut state = State::default();
        state.queue.push(Track {
            id: "track-a".into(),
            title: "Track A".into(),
            ..Default::default()
        });
        state.candidate = Some(Candidate {
            artwork_url: Some("https://example.com/a.jpg".into()),
            ..Default::default()
        });
        state.artwork_cache = Some(("https://example.com/a.jpg".into(), "/cache/a.jpg".into()));
        let snapshot = state.snapshot(true, false);
        assert_eq!(snapshot["track"]["artworkUrl"], "https://example.com/a.jpg");
        assert_eq!(snapshot["track"]["artworkFilePath"], "/cache/a.jpg");
        state.radio_now = json!({"title":"Next song", "artist":"Artist", "metadata":{"artworkUrl":"https://example.com/b.jpg"}});
        let next = state.snapshot(true, false);
        assert_eq!(next["track"]["artworkUrl"], "https://example.com/b.jpg");
        assert!(next["track"]["artworkFilePath"].is_null());
    }

    #[test]
    fn signed_urls_do_not_escape_failure_message() {
        assert!(
            !clean_error("failed https://googlevideo.com/a?pot=private /home/user/file")
                .contains("private")
        );
    }
    #[test]
    fn public_failure_message_redacts_credentials_and_all_source_path_forms() {
        let raw = r"HTTP 403 audio decoder failed https://private.invalid/?token=url-secret file:///tmp/file-secret C:\private\windows-secret \\server\share\unc-secret /home/private/unix-secret token=token-secret cookie=cookie-secret api_key=api-secret Authorization: Bearer bearer-secret";
        let cleaned = clean_error(raw);
        assert!(cleaned.starts_with("HTTP 403 audio decoder failed"));
        for secret in [
            "private.invalid",
            "url-secret",
            "file-secret",
            "windows-secret",
            "unc-secret",
            "unix-secret",
            "token-secret",
            "cookie-secret",
            "api-secret",
            "bearer-secret",
        ] {
            assert!(
                !cleaned.contains(secret),
                "public failure exposed {secret}: {cleaned}"
            );
        }
        assert_eq!(
            crate::playback_failure::classify(&cleaned),
            crate::playback_failure::classify(raw)
        );
        assert!(clean_error(&"x".repeat(400)).chars().count() <= 250);
    }

    #[test]
    fn playback_values_default_only_when_omitted_and_reject_invalid_json() {
        assert_eq!(positive(None, 0.8).unwrap(), 0.8);
        assert_eq!(positive(Some(&json!(0.5)), 0.8).unwrap(), 0.5);
        for invalid in [json!(null), json!("0.5"), json!(true), json!(-0.1)] {
            assert!(positive(Some(&invalid), 0.8).is_err());
        }
    }
    #[test]
    fn manual_radio_identification_matches_the_current_station_song() {
        let metadata = crate::radio::RadioTrackMetadata {
            identifiers: None,
            station_id: "station-1".into(),
            artist: "Artist Name".into(),
            title: "Current Song".into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: String::new(),
            observed_at: None,
            metadata_confidence: "unknown".into(),
        };
        let current = json!({
            "artist":" artist   name ",
            "title":"CURRENT SONG",
            "metadata":{"stationId":"station-1"}
        });
        assert!(same_radio_song(&current, &metadata));
        let other_station = json!({"artist":"Artist Name","title":"Current Song","metadata":{"stationId":"station-2"}});
        assert!(!same_radio_song(&other_station, &metadata));
    }
    #[test]
    fn native_sort_values_match_shared_normalization_fixtures() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/track-sort-conformance.json"
        ))
        .unwrap();
        let cases = &fixtures["sortNormalizationCases"];
        for case in cases["numeric"].as_array().unwrap() {
            let value = &case["value"];
            let actual = numeric_sort_value(value);
            assert_eq!(actual.is_some(), case["valid"].as_bool().unwrap(), "{case}");
            if let (Some(TrackSortValue::Number(actual)), Some(expected)) =
                (actual, case["expected"].as_f64())
            {
                assert_eq!(actual, expected, "{case}");
            }
        }
        for case in cases["camelot"].as_array().unwrap() {
            assert_eq!(
                camelot_sort_value(case["value"].as_str().unwrap()).is_some(),
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
        }
        for case in cases["dates"].as_array().unwrap() {
            let actual = date_sort_value(case["value"].as_str());
            assert_eq!(actual.is_some(), case["valid"].as_bool().unwrap(), "{case}");
            if let (Some(TrackSortValue::Number(actual)), Some(expected)) =
                (actual, case["expected"].as_i64())
            {
                assert_eq!(actual, expected as f64, "{case}");
            }
        }
    }
    #[test]
    fn persistence_serializes_snapshot_through_write_against_newer_save() {
        use std::sync::mpsc;
        let mut initial = State::default();
        initial.queue = vec![Track {
            id: "old".into(),
            ..Track::default()
        }];
        initial.entries = vec!["old-entry".into()];
        initial.queue_revision = 1;
        let state = Arc::new(Mutex::new(initial));
        let persistence = Arc::new(Mutex::new(()));
        let disk = Arc::new(Mutex::new(
            std::collections::BTreeMap::<String, Value>::new(),
        ));
        let (snapped_tx, snapped_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (newer_tx, newer_rx) = mpsc::channel();
        let old_state = state.clone();
        let old_persistence = persistence.clone();
        let old_disk = disk.clone();
        let older = std::thread::spawn(move || {
            persist_playback_state(
                &old_state,
                &old_persistence,
                |queue, session| {
                    let mut disk = old_disk.lock().unwrap();
                    if let Some(queue) = queue {
                        disk.insert(QUEUE_KEY.into(), queue.clone());
                    }
                    if let Some(session) = session {
                        disk.insert(SESSION_KEY.into(), session.clone());
                    }
                    Ok::<(), ()>(())
                },
                || {
                    // This deterministic invariant fails if serialization is removed or
                    // moved below the snapshot hook; no scheduling delay is required.
                    let held = old_persistence.try_lock().is_err();
                    let state_released = old_state.try_lock().is_ok();
                    snapped_tx.send((held, state_released)).unwrap();
                    release_rx.recv().unwrap();
                },
            );
        });
        let (held, state_released) = snapped_rx.recv().unwrap();
        if !held || !state_released {
            release_tx.send(()).unwrap();
            older.join().unwrap();
            panic!("persistence lock must span snapshot and writes; state lock must be released");
        }
        {
            let mut current = state.lock().unwrap();
            current.queue = vec![
                Track {
                    id: "new-0".into(),
                    ..Track::default()
                },
                Track {
                    id: "new-1".into(),
                    ..Track::default()
                },
            ];
            current.entries = vec!["new-entry-0".into(), "new-entry-1".into()];
            current.queue_revision = 2;
            current.index = 1;
        }
        let new_state = state.clone();
        let new_persistence = persistence.clone();
        let new_disk = disk.clone();
        let newer = std::thread::spawn(move || {
            newer_tx.send(()).unwrap();
            persist_playback_state(
                &new_state,
                &new_persistence,
                |queue, session| {
                    let mut disk = new_disk.lock().unwrap();
                    if let Some(queue) = queue {
                        disk.insert(QUEUE_KEY.into(), queue.clone());
                    }
                    if let Some(session) = session {
                        disk.insert(SESSION_KEY.into(), session.clone());
                    }
                    Ok::<(), ()>(())
                },
                || {},
            );
        });
        newer_rx.recv().unwrap();
        release_tx.send(()).unwrap();
        older.join().unwrap();
        newer.join().unwrap();
        let saved = disk.lock().unwrap();
        assert_eq!(saved[QUEUE_KEY]["tracks"][0]["id"], "new-0");
        assert_eq!(saved[SESSION_KEY]["index"], 1);
        assert_eq!(
            saved[QUEUE_KEY]["entryIds"],
            json!(["new-entry-0", "new-entry-1"])
        );
        assert_eq!(saved[SESSION_KEY]["currentEntryId"], "new-entry-1");
        assert_eq!(
            saved[QUEUE_KEY]["queueRevision"],
            saved[SESSION_KEY]["queueRevision"]
        );
        let current = state.lock().unwrap();
        assert_eq!(current.persisted_queue_revision, Some(2));
        assert_eq!(
            current.persisted_session.as_ref(),
            Some(&persisted_session_key(&current))
        );
    }

    #[test]
    fn persistence_write_failure_keeps_failed_values_retryable() {
        let state = Mutex::new(State::default());
        let persistence = Mutex::new(());
        let writes = std::cell::RefCell::new(Vec::new());
        persist_playback_state(
            &state,
            &persistence,
            |queue, session| {
                writes.borrow_mut().push((queue.cloned(), session.cloned()));
                Err::<(), ()>(())
            },
            || {},
        );
        assert_eq!(writes.borrow().len(), 1);
        assert!(writes.borrow()[0].0.is_some());
        assert!(writes.borrow()[0].1.is_some());
        assert!(state.lock().unwrap().persisted_queue_revision.is_none());
        assert!(state.lock().unwrap().persisted_session.is_none());
        writes.borrow_mut().clear();
        persist_playback_state(
            &state,
            &persistence,
            |queue, session| {
                writes.borrow_mut().push((queue.cloned(), session.cloned()));
                Err::<(), ()>(())
            },
            || {},
        );
        assert_eq!(writes.borrow().len(), 1);
        assert!(writes.borrow()[0].0.is_some());
        assert!(writes.borrow()[0].1.is_some());
        assert!(state.lock().unwrap().persisted_queue_revision.is_none());
        assert!(state.lock().unwrap().persisted_session.is_none());
        writes.borrow_mut().clear();
        persist_playback_state(
            &state,
            &persistence,
            |queue, session| {
                writes.borrow_mut().push((queue.cloned(), session.cloned()));
                Ok::<(), ()>(())
            },
            || {},
        );
        assert_eq!(writes.borrow().len(), 1);
        assert!(writes.borrow()[0].0.is_some());
        assert!(writes.borrow()[0].1.is_some());
        assert!(state.lock().unwrap().persisted_queue_revision.is_some());
        assert!(state.lock().unwrap().persisted_session.is_some());
        writes.borrow_mut().clear();
        state.lock().unwrap().position = 1.;
        persist_playback_state(
            &state,
            &persistence,
            |queue, session| {
                writes.borrow_mut().push((queue.cloned(), session.cloned()));
                Ok::<(), ()>(())
            },
            || {},
        );
        assert_eq!(writes.borrow().len(), 1);
        assert!(writes.borrow()[0].0.is_none());
        assert!(writes.borrow()[0].1.is_some());
    }
}

#[cfg(test)]
mod reliability_policy_tests {
    use super::*;
    #[test]
    fn configured_start_seek_and_recovery_budgets_are_absolute() {
        let now = Instant::now();
        let mut state = State::default();
        state.startup_budget = Duration::from_secs(45);
        state.request_started = now;
        assert_eq!(restart_deadline(&state), now + Duration::from_secs(45));
        state.first_audio_ms = Some(300);
        state.candidate = Some(Candidate::default());
        let recovery_deadline = now + Duration::from_secs(7);
        state.recovery_deadline = Some(recovery_deadline);
        assert_eq!(restart_deadline(&state), recovery_deadline);
        state.recovery = 1;
        assert!(can_retry_failed_stream(&state, "network connection reset"));
        assert_eq!(restart_deadline(&state), recovery_deadline);
        state.recovery_deadline = Some(now - Duration::from_millis(1));
        assert!(!can_retry_failed_stream(&state, "network connection reset"));
        state.seek_requested = Some(now);
        assert_eq!(restart_deadline(&state), now + Duration::from_secs(45));
    }
    #[test]
    fn recovery_replaces_only_automatic_unpinned_sources_after_a_refresh() {
        let settings = crate::models::Settings::default();
        let mut track = Track {
            provider: "spotify".into(),
            ..Track::default()
        };
        assert!(!recovery_can_replace_source(1, false, &track, &settings));
        assert!(recovery_can_replace_source(2, false, &track, &settings));
        assert!(!recovery_can_replace_source(2, true, &track, &settings));
        track.provider = "youtube".into();
        assert!(!recovery_can_replace_source(2, false, &track, &settings));
        track.provider = "spotify".into();
        track.local_path = Some("fixture.flac".into());
        assert!(!recovery_can_replace_source(2, false, &track, &settings));
    }
    #[test]
    fn recurring_locked_source_stops_after_the_pair_but_radio_and_seek_can_recover() {
        let settings = crate::models::Settings::default();
        let pinned = Track {
            provider: "youtube".into(),
            ..Track::default()
        };
        let automatic = Track {
            provider: "spotify".into(),
            ..Track::default()
        };
        let pair = RecurringStallDecision::ReplaceSource;
        assert!(recurring_stall_requires_terminal_failure(
            pair,
            &pinned,
            false,
            Some(&settings),
            false,
            false
        ));
        assert!(recurring_stall_requires_terminal_failure(
            pair,
            &automatic,
            true,
            Some(&settings),
            false,
            false
        ));
        assert!(!recurring_stall_requires_terminal_failure(
            pair,
            &automatic,
            false,
            Some(&settings),
            false,
            false
        ));
        assert!(!recurring_stall_requires_terminal_failure(
            pair,
            &pinned,
            false,
            Some(&settings),
            true,
            false
        ));
        assert!(!recurring_stall_requires_terminal_failure(
            pair,
            &pinned,
            false,
            Some(&settings),
            false,
            true
        ));
        assert!(!recurring_stall_requires_terminal_failure(
            RecurringStallDecision::NoRepeatedStall,
            &pinned,
            false,
            Some(&settings),
            false,
            false,
        ));
    }
    #[test]
    fn recurring_native_stall_requests_one_bounded_unpinned_source_swap() {
        let now = Instant::now();
        let mut state = State::default();
        state.queue = vec![Track {
            id: "track-a".into(),
            ..Track::default()
        }];
        state.candidate = Some(Candidate {
            provider: "spotify".into(),
            id: "spotify:recording-a".into(),
            ..Candidate::default()
        });
        assert_eq!(
            record_recurring_transport_stall(&mut state, now),
            RecurringStallDecision::NoRepeatedStall
        );
        assert_eq!(
            record_recurring_transport_stall(&mut state, now + Duration::from_secs(91)),
            RecurringStallDecision::NoRepeatedStall
        );
        assert_eq!(state.recurring_source_swaps, 0);

        assert_eq!(
            record_recurring_transport_stall(&mut state, now + Duration::from_secs(92)),
            RecurringStallDecision::ReplaceSource
        );
        state.recurring_source_swaps += 1; // Replacement accepted by the existing pin gate.
        assert_eq!(state.recurring_source_swaps, 1);
        assert!(state.recent_stall.is_none());
        // A consumed pair cannot retrigger on a duplicate failure event.
        assert_eq!(
            record_recurring_transport_stall(&mut state, now + Duration::from_secs(93)),
            RecurringStallDecision::NoRepeatedStall
        );
        assert_eq!(
            record_recurring_transport_stall(&mut state, now + Duration::from_secs(94)),
            RecurringStallDecision::ReplaceSource
        );
        state.recurring_source_swaps += 1;
        assert_eq!(state.recurring_source_swaps, MAX_RECURRING_SOURCE_SWAPS);
        assert_eq!(
            record_recurring_transport_stall(&mut state, now + Duration::from_secs(95)),
            RecurringStallDecision::NoRepeatedStall
        );
        assert_eq!(
            record_recurring_transport_stall(&mut state, now + Duration::from_secs(96)),
            RecurringStallDecision::Exhausted
        );
        assert_eq!(state.recurring_source_swaps, MAX_RECURRING_SOURCE_SWAPS);
    }
    #[test]
    fn recurring_native_stall_replacement_respects_pinned_and_healthy_reset_gates() {
        let settings = crate::models::Settings::default();
        let mut unpinned = Track {
            provider: "spotify".into(),
            ..Track::default()
        };
        let pinned = Track {
            provider: "youtube".into(),
            ..Track::default()
        };
        assert!(recovery_should_replace_source(
            1, false, &unpinned, &settings, true, 0
        ));
        assert!(!recovery_should_replace_source(
            1, true, &unpinned, &settings, true, 0
        ));
        assert!(!recovery_should_replace_source(
            1, false, &pinned, &settings, true, 0
        ));
        assert!(!recovery_should_replace_source(
            2,
            false,
            &unpinned,
            &settings,
            false,
            MAX_RECURRING_SOURCE_SWAPS
        ));

        let now = Instant::now();
        let baseline = Some((now, 120.0, 8));
        assert!(!native_recovery_output_is_healthy(
            baseline,
            now + Duration::from_secs(59),
            180.0,
            8
        ));
        assert!(native_recovery_output_is_healthy(
            baseline,
            now + Duration::from_secs(60),
            180.0,
            8
        ));
        assert!(
            !native_recovery_output_is_healthy(baseline, now + Duration::from_secs(600), 180.0, 9),
            "seek serial changes invalidate the uninterrupted output clock"
        );
        assert!(
            !native_recovery_output_is_healthy(None, now + Duration::from_secs(600), 999.0, 8),
            "pause and buffering clear the clock"
        );

        unpinned.provider = "catalog".into();
        assert!(recovery_should_replace_source(
            1, false, &unpinned, &settings, true, 0
        ));
        assert!(!recovery_should_replace_source(
            1,
            false,
            &unpinned,
            &settings,
            true,
            MAX_RECURRING_SOURCE_SWAPS
        ));
    }
    #[test]
    fn native_recurring_history_counts_transport_stalls_but_not_output_or_resolver_failures() {
        assert!(failure_counts_as_native_transport_stall(
            RecoveryTrigger::SteadyBufferingTimeout,
            "Audio stalled for five seconds"
        ));
        assert!(failure_counts_as_native_transport_stall(
            RecoveryTrigger::PrematureEnded,
            "Audio stream ended before the recording finished"
        ));
        assert!(failure_counts_as_native_transport_stall(
            RecoveryTrigger::OutputFailed,
            "Audio source access was denied (HTTP 403)"
        ));
        assert!(!failure_counts_as_native_transport_stall(
            RecoveryTrigger::OutputFailed,
            "The audio decoder failed"
        ));
        assert!(!failure_counts_as_native_transport_stall(
            RecoveryTrigger::SourceResolution,
            "Network connection reset"
        ));
    }
    #[test]
    fn native_recovery_claims_each_failure_generation_only_once() {
        let mut state = State::default();
        state.generation = 12;
        assert!(claim_recovery_failure_generation(&mut state, 12));
        assert!(!claim_recovery_failure_generation(&mut state, 12));
        state.generation = 13;
        assert!(!claim_recovery_failure_generation(&mut state, 12));
        assert!(claim_recovery_failure_generation(&mut state, 13));
    }
    #[test]
    fn recurring_source_exclusions_keep_only_the_last_two_distinct_candidate_ids() {
        let mut state = State::default();
        for id in ["monochrome:one", "youtube:two", "soundcloud:three"] {
            let (provider, _) = id.split_once(':').unwrap();
            remember_failed_recurring_candidate(
                &mut state,
                &Candidate {
                    provider: provider.into(),
                    id: id.into(),
                    ..Candidate::default()
                },
            );
        }
        remember_failed_recurring_candidate(
            &mut state,
            &Candidate {
                provider: "soundcloud".into(),
                id: "soundcloud:three".into(),
                ..Candidate::default()
            },
        );
        assert_eq!(
            state.failed_recurring_candidates,
            vec![
                ("youtube".into(), "youtube:two".into()),
                ("soundcloud".into(), "soundcloud:three".into()),
            ]
        );
    }
    #[test]
    fn source_http_denials_do_not_discard_recordings() {
        for error in [
            "HTTP 403 unavailable",
            "HTTP 404 unavailable",
            "HTTP 410 unavailable",
            "HTTP 429 unavailable",
        ] {
            assert!(!should_skip_failed_track(error), "{error}");
        }
        assert!(should_skip_failed_track(
            crate::playback_sources::SOURCE_NO_RESULTS
        ));
    }
}

#[cfg(test)]
mod upcoming_identity_preparation_tests {
    use super::*;
    use crate::playback_sources::PlaybackDeadline;
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn queue_identity_and_raw_search_are_polled_concurrently() {
        let (_stop, cancel) = watch::channel(false);
        let (raw_started, raw_seen) = tokio::sync::oneshot::channel();
        let (identity_started, identity_seen) = tokio::sync::oneshot::channel();
        let raw = async move {
            raw_started.send(()).unwrap();
            identity_seen.await.unwrap();
            Ok(3)
        };
        let identity = async move {
            raw_seen.await.unwrap();
            identity_started.send(()).unwrap();
        };
        assert_eq!(
            finish_upcoming_preparation(
                cancel,
                PlaybackDeadline::after(Duration::from_secs(1)),
                raw,
                identity
            )
            .await
            .unwrap(),
            3
        );
    }
    #[tokio::test]
    async fn queue_identity_and_search_share_one_absolute_deadline_and_drop_both() {
        let (_stop, cancel) = watch::channel(false);
        let raw_dropped = Arc::new(AtomicBool::new(false));
        let identity_dropped = Arc::new(AtomicBool::new(false));
        let raw = {
            let dropped = raw_dropped.clone();
            async move {
                let _drop = Dropped(dropped);
                std::future::pending::<Result<usize, String>>().await
            }
        };
        let identity = {
            let dropped = identity_dropped.clone();
            async move {
                let _drop = Dropped(dropped);
                std::future::pending::<()>().await
            }
        };
        let error = finish_upcoming_preparation(
            cancel,
            PlaybackDeadline::after(Duration::from_millis(20)),
            raw,
            identity,
        )
        .await
        .unwrap_err();
        assert!(error.contains("deadline"));
        assert!(raw_dropped.load(Ordering::SeqCst));
        assert!(identity_dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn cancellation_fences_a_ready_result_and_drops_identity_waiter() {
        let (stop, cancel) = watch::channel(false);
        let identity_dropped = Arc::new(AtomicBool::new(false));
        let identity = {
            let dropped = identity_dropped.clone();
            async move {
                let _drop = Dropped(dropped);
                std::future::pending::<()>().await
            }
        };
        let raw = async move {
            stop.send_replace(true);
            Ok(3)
        };
        assert_eq!(
            finish_upcoming_preparation(
                cancel,
                PlaybackDeadline::after(Duration::from_secs(1)),
                raw,
                identity
            )
            .await
            .unwrap_err(),
            "Cancelled"
        );
        assert!(identity_dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn expired_or_cancelled_queue_preparation_does_not_poll_any_work() {
        let (stop, cancel) = watch::channel(true);
        let raw = async {
            panic!("cancelled preparation polled raw search");
            #[allow(unreachable_code)]
            Ok(0)
        };
        let identity = async {
            panic!("cancelled preparation polled identity");
        };
        assert_eq!(
            finish_upcoming_preparation(
                cancel.clone(),
                PlaybackDeadline::after(Duration::from_secs(1)),
                raw,
                identity
            )
            .await
            .unwrap_err(),
            "Cancelled"
        );
        stop.send_replace(false);
        let expired = PlaybackDeadline::at(tokio::time::Instant::now() - Duration::from_millis(1));
        let raw = async {
            panic!("expired preparation polled raw search");
            #[allow(unreachable_code)]
            Ok(0)
        };
        let identity = async {
            panic!("expired preparation polled identity");
        };
        assert!(finish_upcoming_preparation(cancel, expired, raw, identity)
            .await
            .unwrap_err()
            .contains("deadline"));
    }
}

#[cfg(test)]
mod engine_switch_checkpoint_tests {
    use super::*;
    #[test]
    fn switch_checkpoint_rejects_missing_identity_and_invalid_position() {
        let saved = json!({"tracks":[],"entryIds":[],"queueRevision":7,"currentEntryId":null,"position":22.0,"volume":0.8,"repeat":"all","shuffle":false});
        let (queue, session) = engine_switch_checkpoint(&saved).unwrap();
        assert_eq!(queue["queueRevision"], session["queueRevision"]);
        assert_eq!(session["position"], 22.0);
        assert_eq!(session["finished"], false);
        for field in [
            "entryIds",
            "queueRevision",
            "position",
            "volume",
            "repeat",
            "shuffle",
        ] {
            let mut invalid = saved.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(engine_switch_checkpoint(&invalid).is_err(), "{field}");
        }
        let mut invalid = saved.clone();
        invalid["position"] = json!(-1);
        assert!(engine_switch_checkpoint(&invalid).is_err());
    }
}
