//! Resolve a native playback track into a validated, local or streaming source.
use crate::{
    acquisition::{self, AcquisitionManager},
    models::{Candidate, Settings, Track},
    native_output::{AudioSource, AudioSourceDecoderHint},
    radio::{get_radio_stations_saved, RadioStation},
    storage::Store,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::Arc;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub(crate) const SOURCE_NO_RESULTS: &str = "No audio results were returned by the available providers. Open the catalog source or choose another source.";
pub(crate) const SOURCE_IDENTITY_REJECTED: &str = "Audio results were found, but none could be verified as the requested recording and version. Open the catalog source or choose another source.";
pub(crate) const SOURCE_MISSING_DURATION: &str = "Audio results were found, but the saved recording has no duration and no source provided enough verified identity evidence. Open the catalog source or choose another source.";

/// Preserve meaningful failure evidence across providers. Provider APIs still
/// return strings; fixed resolver outcomes are classified before inspecting any
/// free-form error so recording metadata cannot imply a network/cancel category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ResolutionFailureKind {
    NoResults,
    IdentityRejected,
    SourceOrSearch,
    Retryable,
    Deadline,
    Cancelled,
}

#[derive(Debug)]
struct ResolutionFailure {
    kind: ResolutionFailureKind,
    message: String,
}
impl ResolutionFailure {
    fn new(message: String) -> Self {
        let lower = message.to_ascii_lowercase();
        let kind = if message == "Cancelled" {
            ResolutionFailureKind::Cancelled
        } else if message == SOURCE_NO_RESULTS
            || lower.starts_with("no playable audio match was found")
        {
            ResolutionFailureKind::NoResults
        } else if [
            SOURCE_IDENTITY_REJECTED,
            SOURCE_MISSING_DURATION,
            "The resolved audio source identity does not match this recording",
        ]
        .contains(&message.as_str())
        {
            ResolutionFailureKind::IdentityRejected
        } else if ["deadline", "timed out", "timeout", "budget"]
            .iter()
            .any(|marker| lower.contains(marker))
        {
            ResolutionFailureKind::Deadline
        } else if [
            "network",
            "connection",
            "temporarily unavailable",
            "rate limit",
            "http 429",
            "http 500",
            "http 502",
            "http 503",
            "http 504",
            "http status 429",
            "http status 500",
            "http status 502",
            "http status 503",
            "http status 504",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            ResolutionFailureKind::Retryable
        } else {
            ResolutionFailureKind::SourceOrSearch
        };
        Self { kind, message }
    }
    fn preferred(self, other: Self) -> Self {
        // Stable provider order breaks equal-category ties, never completion
        // timing. A later empty search cannot erase a source/network failure.
        if other.kind > self.kind {
            other
        } else {
            self
        }
    }
}
fn merge_resolution_errors(preferred: String, fallback: String) -> String {
    ResolutionFailure::new(preferred)
        .preferred(ResolutionFailure::new(fallback))
        .message
}
#[derive(Default)]
struct SerialResolutionFailures {
    had_results: bool,
    failure: Option<ResolutionFailure>,
}
impl SerialResolutionFailures {
    fn record(&mut self, error: String) {
        let error = ResolutionFailure::new(error);
        self.failure = Some(match self.failure.take() {
            Some(prior) => prior.preferred(error),
            None => error,
        });
    }
    fn finish(self, missing_duration: bool) -> String {
        if let Some(error) = self.failure {
            if error.kind > ResolutionFailureKind::NoResults {
                return error.message;
            }
        }
        if self.had_results {
            if missing_duration {
                SOURCE_MISSING_DURATION
            } else {
                SOURCE_IDENTITY_REJECTED
            }
        } else {
            SOURCE_NO_RESULTS
        }
        .to_owned()
    }
}

/// Absolute monotonic deadline shared by every phase of one playback lookup.
/// Clones preserve the same instant; they never restart the startup budget.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaybackDeadline(tokio::time::Instant);

impl PlaybackDeadline {
    pub(crate) fn after(budget: Duration) -> Self {
        Self(tokio::time::Instant::now() + budget)
    }

    pub(crate) fn at(instant: tokio::time::Instant) -> Self {
        Self(instant)
    }

    pub(crate) fn instant(self) -> tokio::time::Instant {
        self.0
    }

    pub(crate) fn remaining(self) -> Duration {
        self.0
            .saturating_duration_since(tokio::time::Instant::now())
    }

    pub(crate) fn cap(self, operation_limit: Duration) -> tokio::time::Instant {
        (tokio::time::Instant::now() + operation_limit).min(self.0)
    }
}

/// One playback request has a single budget across searches, extraction, access
/// probes, retries, and provider fallbacks. Per-operation timeouts alone can add
/// up to minutes when several providers fail in sequence.
pub const PLAYBACK_RESOLUTION_DEADLINE: Duration = Duration::from_secs(20);
const PLAYBACK_DEADLINE_MESSAGE: &str =
    "Audio lookup exceeded the playback startup deadline; try again or choose a source";

fn normalize_resolver_deadline<T>(
    result: Result<T, String>,
    deadline: PlaybackDeadline,
    cancelled: bool,
) -> Result<T, String> {
    if cancelled {
        Err("Cancelled".into())
    } else if deadline.remaining().is_zero() {
        Err(PLAYBACK_DEADLINE_MESSAGE.into())
    } else {
        result
    }
}

fn is_resolver_deadline_error<T>(result: &Result<T, String>) -> bool {
    result
        .as_ref()
        .err()
        .is_some_and(|error| error == PLAYBACK_DEADLINE_MESSAGE)
}

#[derive(Clone)]
pub(crate) struct SourceTrace {
    id: uuid::Uuid,
    started: Instant,
    enabled: bool,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
    shared_lookup_id: Option<uuid::Uuid>,
    event_provider: Option<&'static str>,
    extraction_attempts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    test_sink: Option<Arc<dyn Fn(Value) + Send + Sync>>,
}
impl SourceTrace {
    pub(crate) fn new(
        native_operation_id: Option<String>,
        native_seek_operation_id: Option<String>,
        webview_generation: Option<u64>,
    ) -> Self {
        fn safe_id(id: Option<String>) -> Option<String> {
            id.filter(|id| {
                !id.is_empty()
                    && id.len() <= 80
                    && id.bytes().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':')
                    })
            })
        }
        Self {
            id: uuid::Uuid::new_v4(),
            started: Instant::now(),
            enabled: std::env::var("BURRATA_PLAYBACK_TRACE").is_ok_and(|value| value == "1"),
            native_operation_id: safe_id(native_operation_id),
            native_seek_operation_id: safe_id(native_seek_operation_id),
            webview_generation,
            shared_lookup_id: None,
            event_provider: None,
            extraction_attempts: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            test_sink: None,
        }
    }
    pub(crate) fn with_shared_lookup(mut self, id: uuid::Uuid) -> Self {
        self.shared_lookup_id = Some(id);
        self
    }

    pub(crate) fn with_event_provider(mut self, provider: &str) -> Self {
        self.event_provider = match provider {
            "youtube" => Some("youtube"),
            "soundcloud" => Some("soundcloud"),
            "monochrome" => Some("monochrome"),
            _ => None,
        };
        self
    }

    /// Clones share one request-wide budget, including parallel providers and
    /// fresh retries. Reusing already extracted metadata does not spend it.
    pub(crate) fn try_spend_extraction(&self) -> bool {
        self.extraction_attempts
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |used| (used < 2).then_some(used + 1),
            )
            .is_ok()
    }

    pub(crate) fn is_enabled(&self) -> bool {
        #[cfg(test)]
        if self.test_sink.is_some() {
            return true;
        }
        self.enabled
    }

    pub(crate) fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    #[cfg(test)]
    pub(crate) fn with_test_sink(mut self, sink: Arc<dyn Fn(Value) + Send + Sync>) -> Self {
        self.test_sink = Some(sink);
        self
    }

    pub(crate) fn event(&self, phase: &'static str, provider: &str, details: Value) {
        if !self.enabled {
            #[cfg(test)]
            if let Some(sink) = &self.test_sink {
                sink(self.payload(phase, provider, details));
            }
            return;
        }
        // Callers supply bounded categorical enums, counts, timings and booleans.
        // Never record source URLs, titles, IDs, subprocess arguments or errors.
        let payload = self.payload(phase, provider, details);
        #[cfg(test)]
        if let Some(sink) = &self.test_sink {
            sink(payload.clone());
        }
        eprintln!("burrata_playback_phase {}", payload);
    }

    pub(crate) fn event_scoped(&self, phase: &'static str, details: Value) {
        self.event(phase, self.event_provider.unwrap_or("other"), details);
    }
    pub(crate) fn payload(&self, phase: &'static str, provider: &str, details: Value) -> Value {
        let provider = match provider {
            "youtube" | "soundcloud" | "monochrome" => provider,
            _ => "other",
        };
        json!({"resolutionId":self.id.to_string(),
            "nativeOperationId":self.native_operation_id,
            "nativeSeekOperationId":self.native_seek_operation_id,
            "webviewGeneration":self.webview_generation,
            "sharedLookupId":self.shared_lookup_id.map(|id| id.to_string()),
            "atUnixMs":chrono::Utc::now().timestamp_millis(),"elapsedMs":self.started.elapsed().as_millis(),
            "phase":phase,"provider":provider,"details":details})
    }
}

/// Completes a started resolution even when its caller drops the future.
/// Retain the original trace so terminal correlation never invents a new ID.
pub(crate) struct ResolutionTraceGuard {
    trace: SourceTrace,
    provider: &'static str,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
    finished: bool,
}
impl ResolutionTraceGuard {
    pub(crate) fn begin(
        trace: SourceTrace,
        provider: &str,
        selected: bool,
        cancel: watch::Receiver<bool>,
        deadline: PlaybackDeadline,
    ) -> Self {
        let provider = match provider {
            "youtube" => "youtube",
            "soundcloud" => "soundcloud",
            "monochrome" => "monochrome",
            _ => "other",
        };
        trace.event("resolution_start", provider, json!({"selected": selected}));
        Self {
            trace,
            provider,
            cancel,
            deadline,
            finished: false,
        }
    }

    pub(crate) fn finish<T>(mut self, result: &Result<T, String>) {
        self.finished = true;
        self.trace.event(
            "resolution_end",
            self.provider,
            json!({
                "ok": result.is_ok(),
                "cancelled": result.as_ref().err().is_some_and(|error| error == "Cancelled"),
                "deadline": is_resolver_deadline_error(result),
            }),
        );
    }
}
impl Drop for ResolutionTraceGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // The outer deadline owner drops this future before publishing its
        // timer-generated cancellation, so a true watch value here is a real
        // cancellation, preserving cancellation's priority over deadline.
        let cancelled = *self.cancel.borrow();
        let deadline = !cancelled && self.deadline.remaining().is_zero();
        self.trace.event(
            "resolution_end",
            self.provider,
            json!({
                "ok": false, "cancelled": cancelled, "deadline": deadline,
                "abandoned": !cancelled && !deadline,
            }),
        );
    }
}

/// Emits a matching end phase even when the outer startup deadline drops a
/// provider future. The operation-level resolution_end event distinguishes a
/// deadline from other future drops using the same operation ID.
pub(crate) struct TraceSpan {
    trace: SourceTrace,
    provider: &'static str,
    start_phase: &'static str,
    end_phase: &'static str,
    started: Instant,
    finished: bool,
}

impl TraceSpan {
    pub(crate) fn begin(
        trace: SourceTrace,
        provider: &'static str,
        start_phase: &'static str,
        end_phase: &'static str,
    ) -> Self {
        let span = Self {
            trace,
            provider,
            start_phase,
            end_phase,
            started: Instant::now(),
            finished: false,
        };
        span.trace.event(span.start_phase, span.provider, json!({}));
        span
    }

    pub(crate) fn finish(mut self, outcome: &'static str) {
        self.emit_end(outcome);
        self.finished = true;
    }

    fn emit_end(&self, outcome: &'static str) {
        self.trace.event(
            self.end_phase,
            self.provider,
            json!({"durationMs":self.started.elapsed().as_millis(),"outcome":outcome}),
        );
    }
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        if !self.finished {
            self.emit_end("abandoned");
        }
    }
}

/// Only an explicit true value cancels. A closed false channel has no more
/// updates, so keep this branch pending rather than turning closure into a
/// source failure or spinning on repeated receive errors.
pub(crate) async fn wait_for_cancellation(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            if *cancel.borrow() {
                return;
            }
            std::future::pending::<()>().await;
        }
    }
}

async fn within_deadline<T>(
    duration: Duration,
    future: std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + '_>>,
) -> Result<T, String> {
    tokio::time::timeout(duration, future)
        .await
        .unwrap_or_else(|_| Err(PLAYBACK_DEADLINE_MESSAGE.into()))
}

async fn within_shared_deadline<T>(
    deadline: PlaybackDeadline,
    future: std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + '_>>,
) -> Result<T, String> {
    tokio::time::timeout_at(deadline.instant(), future)
        .await
        .unwrap_or_else(|_| Err(PLAYBACK_DEADLINE_MESSAGE.into()))
}

pub struct ResolvedSource {
    pub source: AudioSource,
    pub candidate: Option<Candidate>,
    pub quality: Value,
    pub station: Option<RadioStation>,
    pub duration: Option<f64>,
}

pub(crate) fn validate_candidate_attempt_limit(limit: Option<usize>) -> Result<usize, String> {
    let limit = limit.unwrap_or(2);
    if (1..=2).contains(&limit) {
        Ok(limit)
    } else {
        Err("Playback candidate attempt limit must be one or two".into())
    }
}

fn remaining_candidate_attempts(
    limit: usize,
    used: usize,
    global: bool,
    provider_used: usize,
) -> usize {
    limit.saturating_sub(if global { used } else { provider_used })
}

fn preview_audio_quality(audio: &acquisition::PreviewAudio) -> Value {
    json!({
        "codec": audio.codec,
        "mimeType": audio.mime_type,
        "bitrateKbps": audio.bitrate_kbps,
        "formatId": audio.selected_format_id,
        "formatHasVideo": audio.selected_format_has_video,
        "expectedContentLength": audio.expected_content_length,
    })
}

/// Resolve audio without exposing any short-lived stream URL through the returned candidate.
pub async fn resolve(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
) -> Result<ResolvedSource, String> {
    resolve_with_trace_context(
        store,
        acquisition,
        track,
        selected,
        cancel,
        None,
        None,
        None,
    )
    .await
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FailedPlaybackCandidate {
    pub provider: String,
    pub id: String,
}
pub(crate) fn validate_failed_candidates(values: &[FailedPlaybackCandidate]) -> Result<(), String> {
    if values.len() > 2
        || values.iter().any(|value| {
            value.provider.is_empty()
                || value.provider.len() > 32
                || !value.provider.bytes().all(|byte| byte.is_ascii_lowercase())
                || value.id.is_empty()
                || value.id.len() > 512
                || value.id.chars().any(char::is_control)
        })
    {
        return Err("Failed playback candidate exclusions are invalid".into());
    }
    Ok(())
}
fn candidate_failed(candidate: &Candidate, failed: &[FailedPlaybackCandidate]) -> bool {
    failed
        .iter()
        .any(|value| value.provider == candidate.provider && value.id == candidate.id)
}

fn eligible_after_webview_failure(
    candidates: impl IntoIterator<Item = Candidate>,
    failed: &[FailedPlaybackCandidate],
    attempted_mapping_id: Option<&str>,
    remaining: usize,
) -> Vec<Candidate> {
    candidates
        .into_iter()
        .filter(|candidate| {
            !candidate_failed(candidate, failed)
                && Some(candidate.id.as_str()) != attempted_mapping_id
        })
        .take(remaining)
        .collect()
}
fn remembered_for_request(
    track: &Track,
    mapping: &crate::storage::VerifiedPlaybackSource,
    failed: &[FailedPlaybackCandidate],
) -> Option<Candidate> {
    remembered_candidate(track, mapping).filter(|candidate| !candidate_failed(candidate, failed))
}

pub(crate) async fn resolve_with_trace_context(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
) -> Result<ResolvedSource, String> {
    resolve_with_trace_context_excluding(
        store,
        acquisition,
        track,
        selected,
        cancel,
        native_operation_id,
        native_seek_operation_id,
        webview_generation,
        &[],
    )
    .await
}

pub(crate) async fn resolve_with_trace_context_excluding(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
    failed: &[FailedPlaybackCandidate],
) -> Result<ResolvedSource, String> {
    let deadline = PlaybackDeadline::after(store.settings()?.playback_startup_timeout());
    within_shared_deadline(
        deadline,
        Box::pin(resolve_with_trace_context_excluding_until(
            store,
            acquisition,
            track,
            selected,
            cancel,
            native_operation_id,
            native_seek_operation_id,
            webview_generation,
            failed,
            deadline,
        )),
    )
    .await
}

pub(crate) async fn resolve_with_trace_context_excluding_until(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
    failed: &[FailedPlaybackCandidate],
    deadline: PlaybackDeadline,
) -> Result<ResolvedSource, String> {
    resolve_with_trace_context_excluding_until_attempt_limit(
        store,
        acquisition,
        track,
        selected,
        cancel,
        native_operation_id,
        native_seek_operation_id,
        webview_generation,
        failed,
        deadline,
        2,
    )
    .await
}

pub(crate) async fn resolve_with_trace_context_excluding_until_attempt_limit(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
    failed: &[FailedPlaybackCandidate],
    deadline: PlaybackDeadline,
    attempt_limit: usize,
) -> Result<ResolvedSource, String> {
    if !(1..=2).contains(&attempt_limit) {
        return Err("Playback candidate attempt limit must be one or two".into());
    }
    validate_failed_candidates(failed)?;
    let trace = SourceTrace::new(
        native_operation_id,
        native_seek_operation_id,
        webview_generation,
    );
    let completion = ResolutionTraceGuard::begin(
        trace.clone(),
        &track.provider,
        selected.is_some(),
        cancel.clone(),
        deadline,
    );
    let result = tokio::select! {
        biased;
        _ = wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        result = resolve_inner(store, acquisition, track, selected, cancel.clone(), &trace, failed, deadline, attempt_limit) => result,
    };
    let result = normalize_resolver_deadline(result, deadline, *cancel.borrow());
    completion.finish(&result);
    result
}

/// Refresh the exact candidate that was already playing after a transport stall.
/// This is intentionally separate from ordinary resolution: it bypasses only the
/// cached signed URL for this candidate and never performs a broad search.
pub(crate) async fn resolve_candidate_fresh_until(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    mut candidate: Candidate,
    explicitly_pinned: bool,
    cancel: watch::Receiver<bool>,
    webview_generation: u64,
    deadline: PlaybackDeadline,
) -> Result<ResolvedSource, String> {
    let trace = SourceTrace::new(None, None, Some(webview_generation));
    let completion = ResolutionTraceGuard::begin(
        trace.clone(),
        &track.provider,
        true,
        cancel.clone(),
        deadline,
    );
    let result = tokio::select! {
        biased;
        _ = wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        result = async {
            if *cancel.borrow() { return Err("Cancelled".into()); }
            let settings = store.settings()?;
            if candidate.id.split_once(':').map(|(provider, _)| provider) != Some(candidate.provider.as_str()) {
                return Err("The stalled playback candidate identity is invalid".into());
            }
            let pinned = source_resolution_is_pinned(track, explicitly_pinned, &settings, true);
            let mut aliases = if pinned {
                crate::artist_credit_aliases::VerifiedPlaybackArtistAliases::default()
            } else {
                crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
            };
            include_candidate_aliases(&mut aliases,track,&candidate,None);
            if !pinned
                && !evaluate_candidate_gate_with_proofs(
                    track,
                    aliases.tracks(),
                    aliases.isrc_enriched(),
                    aliases.verified_recording_isrc(),
                    &candidate,
                )
                .accepted()
            {
                return Err("The stalled playback candidate no longer passes recording identity checks".into());
            }
            let audio = acquisition.extract_playback_audio_fresh_or_prepared_traced(
                &candidate, &settings, cancel.clone(), true, trace.clone(), deadline, None,
            ).await?;
            if *cancel.borrow() { return Err("Cancelled".into()); }
            if !fresh_candidate_audio_identity_matches(track, &candidate, &audio) {
                return Err("The refreshed audio source did not resolve to the same provider recording".into());
            }
            include_candidate_aliases(&mut aliases,track,&candidate,Some(&audio));
            let requires_identity_check = candidate.provider == "bandcamp" || !pinned;
            if !accept_resolved_identity_with_aliases_and_proofs(
                track,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
                &mut candidate,
                &audio,
                requires_identity_check,
            ) {
                return Err("The refreshed audio source identity does not match this recording".into());
            }
            let audio = acquisition.validate_playback_audio_traced(
                &candidate, audio, cancel.clone(), trace.clone(), deadline,
            ).await?;
            let duration = audio.duration_seconds.or(candidate.duration_seconds)
                .filter(|value| value.is_finite() && *value >= 0.);
            Ok(ResolvedSource {
                source: AudioSource {
                    input: audio.url.clone(),
                    ffmpeg: crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: audio.headers.clone(),
                    decoder_hint: Some(AudioSourceDecoderHint {
                        mime_type: Some(audio.mime_type.clone()), codec: audio.codec.clone(),
                        duration_seconds: audio.duration_seconds,
                        content_length: audio.expected_content_length,
                        has_video: audio.selected_format_has_video,
                    }),
                },
                quality: preview_audio_quality(&audio),
                candidate: Some(candidate), station: None, duration,
            })
        } => result,
    };
    let result = normalize_resolver_deadline(result, deadline, *cancel.borrow());
    completion.finish(&result);
    result
}

fn fresh_candidate_audio_identity_matches(
    track: &Track,
    candidate: &Candidate,
    audio: &acquisition::PreviewAudio,
) -> bool {
    let Some((provider, expected_id)) = candidate.id.split_once(':') else {
        return false;
    };
    if provider != candidate.provider {
        return false;
    }
    if provider == "monochrome" {
        // Monochrome's normal resolver has no extractor title/id fields. Its
        // canonical catalog URL is its stable identity, and Client::inspect
        // probes that same URL before returning playable container metadata.
        let mut source_track = track.clone();
        source_track.source_url = candidate.url.clone();
        source_track.playback_range = None;
        let Some(canonical) = linked_candidate(&source_track) else {
            return false;
        };
        canonical.provider == candidate.provider
            && canonical.id == candidate.id
            && canonical.url == candidate.url
            && audio.url == canonical.url
            && crate::monochrome::validate_url(&audio.url).is_ok()
    } else {
        audio.resolved_provider_id.as_deref() == Some(expected_id)
            && audio
                .resolved_title
                .as_deref()
                .is_some_and(|title| !title.trim().is_empty())
    }
}

async fn resolve_inner(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    trace: &SourceTrace,
    failed: &[FailedPlaybackCandidate],
    deadline: PlaybackDeadline,
    attempt_limit: usize,
) -> Result<ResolvedSource, String> {
    trace.event("settings_read_start", &track.provider, json!({}));
    let settings_started = Instant::now();
    let settings = store.settings()?;
    trace.event(
        "settings_read_end",
        &track.provider,
        json!({"durationMs":settings_started.elapsed().as_millis()}),
    );
    let explicit_source = source_resolution_is_pinned(track, selected.is_some(), &settings, false);
    resolve_after_local_preflight(store, track, explicit_source, |prepared_track| {
        Box::pin(async move {
            resolve_inner_prepared(
                store,
                acquisition,
                &prepared_track,
                selected,
                cancel,
                settings,
                trace,
                failed,
                deadline,
                attempt_limit,
            )
            .await
        })
    })
    .await
}

async fn resolve_after_local_preflight<T, F, Fut>(
    store: &Store,
    incoming: &Track,
    explicit_source: bool,
    resolve: F,
) -> Result<T, String>
where
    F: FnOnce(Track) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
{
    let prepared_track = reconcile_stale_local_asset(store, incoming, explicit_source)?;
    resolve(prepared_track).await
}

fn reconcile_stale_local_asset(
    store: &Store,
    incoming: &Track,
    explicit_source: bool,
) -> Result<Track, String> {
    let Some(path) = incoming.local_path.as_deref() else {
        return Ok(incoming.clone());
    };
    if Path::new(path).is_file() || incoming.playback_range.is_some() {
        return Ok(incoming.clone());
    }

    let Some(refreshed) = store.get_track(&incoming.id)? else {
        if !explicit_source {
            return Err("Downloaded audio file is missing".into());
        }
        return Ok(without_stale_local_asset(incoming));
    };
    if refreshed
        .local_path
        .as_deref()
        .is_some_and(|refreshed_path| Path::new(refreshed_path).is_file())
    {
        let mut prepared = incoming.clone();
        prepared.local_path = refreshed.local_path;
        prepared.asset_id = refreshed.asset_id;
        prepared.local_availability = refreshed.local_availability;
        return Ok(prepared);
    }

    Ok(without_stale_local_asset(incoming))
}

fn without_stale_local_asset(incoming: &Track) -> Track {
    let mut prepared = incoming.clone();
    prepared.local_path = None;
    prepared.asset_id = None;
    prepared.local_availability = "missing".into();
    prepared
}

#[derive(Default)]
struct AliasResolutionPass {
    retry: bool,
    rejected: std::sync::Mutex<Vec<Candidate>>,
    attempts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    spare_queries: std::sync::Mutex<std::collections::HashMap<String, bool>>,
    empty_search: std::sync::atomic::AtomicBool,
}
impl AliasResolutionPass {
    fn remember(&self, candidate: &Candidate) {
        if self.retry {
            return;
        }
        if let Ok(mut rows) = self.rejected.lock() {
            if rows.len() < 200
                && !rows
                    .iter()
                    .any(|row| row.provider == candidate.provider && row.id == candidate.id)
            {
                rows.push(candidate.clone());
            }
        }
    }
    fn attempted(&self, provider: &str) {
        if let Ok(mut counts) = self.attempts.lock() {
            *counts.entry(provider.to_owned()).or_default() += 1;
        }
    }
    fn used(&self, provider: &str) -> usize {
        self.attempts
            .lock()
            .ok()
            .and_then(|counts| counts.get(provider).copied())
            .unwrap_or(0)
    }
    fn total_used(&self) -> usize {
        self.attempts
            .lock()
            .map(|counts| counts.values().sum())
            .unwrap_or(usize::MAX)
    }
    fn has_remaining_alias_attempt(&self, limit: usize) -> bool {
        if limit == 1 && self.total_used() >= 1 {
            return false;
        }
        let candidates = self
            .rejected
            .lock()
            .map(|rows| rows.iter().any(|row| self.used(&row.provider) < limit))
            .unwrap_or(false);
        let spare = self
            .spare_queries
            .lock()
            .map(|queries| {
                queries
                    .iter()
                    .any(|(provider, spare)| *spare && self.used(provider) < limit)
            })
            .unwrap_or(false);
        candidates || spare
    }
    fn collected(&self, provider: &str) -> Vec<Candidate> {
        self.rejected
            .lock()
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.provider == provider)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn resolve_inner_prepared(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    settings: Settings,
    trace: &SourceTrace,
    failed: &[FailedPlaybackCandidate],
    deadline: PlaybackDeadline,
    attempt_limit: usize,
) -> Result<ResolvedSource, String> {
    let mut pass = AliasResolutionPass::default();
    let pinned = source_resolution_is_pinned(track, selected.is_some(), &settings, true);
    let result = resolve_inner_prepared_ordinary(
        store,
        acquisition,
        track,
        selected,
        cancel.clone(),
        settings.clone(),
        trace,
        failed,
        deadline,
        attempt_limit,
        &pass,
    )
    .await;
    if result.is_ok()
        || pinned
        || !crate::artist_alias_lookup::enabled()
        || *cancel.borrow()
        || deadline.remaining().is_zero()
        || !pass.has_remaining_alias_attempt(attempt_limit)
    {
        trace.event(
            "alias_lookup_skipped",
            &track.provider,
            json!({"reason":if result.is_ok(){"ordinary_resolution_succeeded"}else{"ineligible"}}),
        );
        return result;
    }
    let plausible = pass
        .rejected
        .lock()
        .map(|rows| !rows.is_empty())
        .unwrap_or(false)
        || (pass.empty_search.load(std::sync::atomic::Ordering::Relaxed)
            && ((track.provider == "spotify"
                && track.source_url
                    == format!("https://open.spotify.com/track/{}", track.provider_id))
                || (track.provider == "catalog"
                    && matches!(
                        track.provenance["catalogProvider"].as_str(),
                        Some("musicbrainz" | "discogs")
                    ))
                || (matches!(track.provider.as_str(), "beatport" | "traxsource" | "local")
                    && track.isrc.is_some())));
    if !plausible {
        trace.event(
            "alias_lookup_skipped",
            &track.provider,
            json!({"reason":"no_artist_identity_block"}),
        );
        return result;
    }
    match crate::artist_alias_lookup::lookup(acquisition, track, cancel.clone(), deadline, trace)
        .await
    {
        Err(_) if *cancel.borrow() => return Err("Cancelled".into()),
        Err(_) if deadline.remaining().is_zero() => return Err(PLAYBACK_DEADLINE_MESSAGE.into()),
        Err(_) => return result,
        Ok(()) => {}
    }
    let context =
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track);
    if !context.has_verified_credit_sets() {
        let identity_error = if track.duration_seconds.is_none() {
            SOURCE_MISSING_DURATION
        } else {
            SOURCE_IDENTITY_REJECTED
        };
        return Err(merge_resolution_errors(
            result.err().unwrap_or_else(|| SOURCE_NO_RESULTS.into()),
            identity_error.into(),
        ));
    }
    pass.retry = true;
    let started = Instant::now();
    let retried = resolve_serial(
        store,
        acquisition,
        track,
        None,
        cancel,
        settings,
        trace,
        failed,
        deadline,
        attempt_limit,
        &pass,
    )
    .await;
    trace.event("alias_candidate_recheck",&track.provider,json!({"durationMs":started.elapsed().as_secs_f64()*1000.0,"accepted":retried.is_ok(),"count":pass.rejected.lock().map(|rows|rows.len()).unwrap_or(0)}));
    retried.map_err(|error| {
        merge_resolution_errors(
            result.err().unwrap_or_else(|| SOURCE_NO_RESULTS.into()),
            error,
        )
    })
}

async fn resolve_inner_prepared_ordinary(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    settings: Settings,
    trace: &SourceTrace,
    failed: &[FailedPlaybackCandidate],
    deadline: PlaybackDeadline,
    attempt_limit: usize,
    alias_pass: &AliasResolutionPass,
) -> Result<ResolvedSource, String> {
    let pinned = source_resolution_is_pinned(track, selected.is_some(), &settings, true);
    let primary = match settings.preferred_streaming_source.as_str() {
        "monochrome" | "soundcloud" => settings.preferred_streaming_source.as_str(),
        _ => "youtube",
    };
    let fallbacks = crate::source_fallback::provider_order(
        primary,
        settings.streaming_fallback_enabled,
        &settings.streaming_fallback_sources,
        false,
    );
    if pinned || fallbacks.is_empty() {
        return resolve_serial(
            store,
            acquisition,
            track,
            selected,
            cancel,
            settings,
            trace,
            failed,
            deadline,
            attempt_limit,
            alias_pass,
        )
        .await;
    }
    let mut preferred = settings.clone();
    preferred.streaming_fallback_enabled = false;
    if attempt_limit == 1 {
        return resolve_serial(
            store,
            acquisition,
            track,
            None,
            cancel,
            settings,
            trace,
            failed,
            deadline,
            attempt_limit,
            alias_pass,
        )
        .await;
    }
    let mut fallback = preferred.clone();
    fallback.preferred_streaming_source = fallbacks[0].clone();
    let (stop, child) = watch::channel(false);
    let result = tokio::select! {
        biased;
        _ = wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        result = prefer_with_grace(
            Box::pin(resolve_serial(store, acquisition, track, None, child.clone(), preferred, trace, failed, deadline, attempt_limit, alias_pass)),
            Box::pin(resolve_serial(store, acquisition, track, None, child, fallback, trace, failed, deadline, attempt_limit, alias_pass)),
            Duration::from_millis(settings.streaming_fallback_delay_ms.into()),
        ) => result,
    };
    stop.send_replace(true);
    match result {
        Ok(source) => Ok(source),
        Err(error) if *cancel.borrow() || fallbacks.len() < 2 => Err(error),
        Err(prior_error) => {
            let mut remaining = settings;
            remaining.preferred_streaming_source = fallbacks[1].clone();
            remaining.streaming_fallback_sources = fallbacks[2..].to_vec();
            resolve_serial(
                store,
                acquisition,
                track,
                None,
                cancel,
                remaining,
                trace,
                failed,
                deadline,
                attempt_limit,
                alias_pass,
            )
            .await
            .map_err(|error| merge_resolution_errors(prior_error, error))
        }
    }
}

pub(crate) fn source_resolution_is_pinned(
    track: &Track,
    selected: bool,
    settings: &Settings,
    local_path_pins: bool,
) -> bool {
    selected
        || (local_path_pins && track.local_path.is_some())
        || track.provider == "radio"
        || track.playback_range.is_some()
        || matches!(track.provider.as_str(), "youtube" | "soundcloud")
        || (!overrides_import_url(track, settings)
            && (linked_candidate(track).is_some()
                || track.provenance["sourceKind"].as_str() == Some("audioSearch")))
}

// Poll both attempts immediately; preserve preferred-source priority until the grace expires.
async fn prefer_with_grace<T>(
    preferred: std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + '_>>,
    fallback: std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + '_>>,
    grace: Duration,
) -> Result<T, String> {
    tokio::pin!(preferred, fallback);
    let deadline = tokio::time::sleep(grace);
    tokio::pin!(deadline);
    tokio::select! {
        biased;
        result = &mut preferred => match result {
            Ok(value) => Ok(value),
            Err(preferred_error) => fallback.await.map_err(|fallback_error| merge_resolution_errors(preferred_error, fallback_error)),
        },
        result = &mut fallback => match result {
            Err(fallback_error) => preferred.await.map_err(|preferred_error| merge_resolution_errors(preferred_error, fallback_error)),
            Ok(value) => tokio::select! {
                biased;
                result = &mut preferred => result.or(Ok(value)),
                _ = &mut deadline => Ok(value),
            },
        },
    }
}

async fn resolve_serial(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    selected: Option<Candidate>,
    cancel: watch::Receiver<bool>,
    settings: Settings,
    trace: &SourceTrace,
    failed: &[FailedPlaybackCandidate],
    deadline: PlaybackDeadline,
    attempt_limit: usize,
    alias_pass: &AliasResolutionPass,
) -> Result<ResolvedSource, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let ffmpeg = crate::tools::executable(&settings.ffmpeg_path, "ffmpeg");

    // A registered local asset is the most direct and reliable source, including for clips.
    if let Some(path) = track.local_path.as_deref() {
        let asset = store
            .get_asset_by_path(path)?
            .ok_or("Local audio is not registered in the library")?;
        if !Path::new(path).is_file() {
            return Err("Downloaded audio file is missing".into());
        }
        let duration = track
            .playback_range
            .as_ref()
            .map(|range| range.end_time - range.start_time)
            .or(track.duration_seconds)
            .filter(|value| value.is_finite() && *value >= 0.);
        return Ok(ResolvedSource {
            source: AudioSource {
                input: path.to_owned(),
                ffmpeg,
                headers: vec![],
                decoder_hint: None,
            },
            candidate: None,
            quality: json!({
                "assetId": asset.id,
                "codec": asset.final_codec,
                "bitrateKbps": asset.final_bitrate,
                "bitrateOrigin": asset.bitrate_origin,
                "sampleRate": asset.sample_rate,
                "channels": asset.channels,
                "bitDepth": asset.bit_depth,
                "durationSeconds": asset.duration_seconds,
                "sizeBytes": asset.size_bytes
            }),
            station: None,
            duration,
        });
    }

    if track.provider == "radio" {
        let station_id = track.provenance["stationId"]
            .as_str()
            .unwrap_or(&track.provider_id);
        let station = get_radio_stations_saved(store)?
            .into_iter()
            .find(|station| station.id == station_id)
            .ok_or("Radio station no longer exists")?;
        return Ok(ResolvedSource {
            source: AudioSource {
                input: station.stream_url.clone(),
                ffmpeg,
                headers: vec![],
                decoder_hint: None,
            },
            candidate: None,
            quality: json!({
                "codec": station.codec,
                "bitrateKbps": station.bitrate_kbps
            }),
            station: Some(station),
            duration: None,
        });
    }

    let linked = if overrides_import_url(track, &settings) && selected.is_none() {
        None
    } else {
        linked_candidate(track)
    };
    let pinned = selected.is_some()
        || track.playback_range.is_some()
        || matches!(track.provider.as_str(), "youtube" | "soundcloud")
        || (track.provenance["sourceKind"].as_str() == Some("audioSearch")
            && !overrides_import_url(track, &settings))
        || linked.is_some();
    let cache_started = Instant::now();
    let mut verified_artist_aliases = if pinned {
        crate::artist_credit_aliases::VerifiedPlaybackArtistAliases::default()
    } else {
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
    };
    trace.event("alias_cache_lookup_end", &track.provider, json!({"durationMs":cache_started.elapsed().as_secs_f64()*1000.0,"aliases":verified_artist_aliases.tracks().len(),"verifiedCreditSets":verified_artist_aliases.has_verified_credit_sets()}));
    let alternate_search_artists = verified_artist_aliases.search_artist_candidates();
    let mut candidates = Vec::new();
    if let Some(candidate) = selected.or(linked) {
        candidates.push(candidate);
    }

    let primary = match settings.preferred_streaming_source.as_str() {
        "youtube" | "soundcloud" | "monochrome" => settings.preferred_streaming_source.as_str(),
        _ => "youtube",
    };
    let mut providers = if pinned {
        // A selected source owns this request; trace and retry its actual provider.
        vec![candidates
            .first()
            .map(|candidate| candidate.provider.clone())
            .unwrap_or_else(|| primary.to_owned())]
    } else {
        vec![primary.to_owned()]
    };
    if !pinned {
        providers.extend(crate::source_fallback::provider_order(
            primary,
            settings.streaming_fallback_enabled,
            &settings.streaming_fallback_sources,
            false,
        ));
    }

    let mut failures = SerialResolutionFailures {
        had_results: !candidates.is_empty(),
        ..Default::default()
    };
    let mut tried_linked = false;
    let mut transient_retry_attempted = false;
    let mapping_fingerprint = if pinned {
        None
    } else {
        recording_identity_fingerprint(track)
    };
    let global_attempt_cap = attempt_limit == 1;
    let mut total_candidate_attempts = if alias_pass.retry {
        alias_pass.total_used()
    } else {
        0
    };
    for provider in providers {
        if global_attempt_cap && total_candidate_attempts >= attempt_limit {
            break;
        }
        trace.event("provider_start", &provider, json!({"pinned":pinned}));
        // A rate-limited service cannot supply even a remembered/static URL.
        // Skip it without spending the remaining foreground attempt, while
        // preserving an explicit source's provider-only scope.
        if provider == "monochrome" {
            if let Err(error) =
                crate::monochrome::Client::new().and_then(|client| client.check_cooldown())
            {
                failures.record(error);
                trace.event("provider_cooldown", &provider, json!({"rateLimited":true}));
                continue;
            }
        }
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let mut attempted_mapping_id: Option<String> = None;
        let mut provider_attempt_count = if alias_pass.retry {
            alias_pass.used(&provider)
        } else {
            0
        };
        if alias_pass.retry {
            let collected = alias_pass.collected(&provider);
            for candidate in &collected {
                include_candidate_aliases(&mut verified_artist_aliases, track, candidate, None);
            }
            let mut ranked = collected
                .into_iter()
                .filter_map(|candidate| {
                    let path = evaluate_candidate_gate_with_proofs(
                        track,
                        verified_artist_aliases.tracks(),
                        verified_artist_aliases.isrc_enriched(),
                        verified_artist_aliases.verified_recording_isrc(),
                        &candidate,
                    );
                    path.accepted().then_some((candidate, path))
                })
                .collect::<Vec<_>>();
            ranked.sort_by_key(|(_, path)| path.evidence_priority());
            candidates = eligible_after_webview_failure(
                ranked.into_iter().map(|(candidate, _)| candidate),
                failed,
                None,
                remaining_candidate_attempts(
                    attempt_limit,
                    total_candidate_attempts,
                    global_attempt_cap,
                    provider_attempt_count,
                ),
            );
        }
        if alias_pass.retry
            && candidates.is_empty()
            && provider_attempt_count < attempt_limit
            && alias_pass
                .spare_queries
                .lock()
                .ok()
                .and_then(|mut queries| queries.remove(&provider))
                .unwrap_or(false)
        {
            if let Some(alternate) = alternate_search_artists.iter().find(|artist| {
                crate::matching::normalize(artist) != crate::matching::normalize(&track.artist)
            }) {
                let mut query_track = track.clone();
                query_track.artist = alternate.clone();
                trace.event(
                    "alias_search_start",
                    &provider,
                    json!({"remainingQuerySlots":1}),
                );
                match acquisition
                    .prefetch_playback_candidates_cancelled_traced(
                        &query_track,
                        &[],
                        std::slice::from_ref(&provider),
                        &settings,
                        cancel.clone(),
                        deadline,
                        trace.clone().with_event_provider(&provider),
                    )
                    .await
                {
                    Ok(found) => {
                        for candidate in &found {
                            include_candidate_aliases(
                                &mut verified_artist_aliases,
                                track,
                                candidate,
                                None,
                            );
                        }
                        candidates = eligible_after_webview_failure(
                            found.into_iter().filter(|candidate| {
                                evaluate_candidate_gate_with_proofs(
                                    track,
                                    verified_artist_aliases.tracks(),
                                    verified_artist_aliases.isrc_enriched(),
                                    verified_artist_aliases.verified_recording_isrc(),
                                    candidate,
                                )
                                .accepted()
                            }),
                            failed,
                            None,
                            remaining_candidate_attempts(
                                attempt_limit,
                                total_candidate_attempts,
                                global_attempt_cap,
                                provider_attempt_count,
                            ),
                        );
                    }
                    Err(error) if *cancel.borrow() || error == "Cancelled" => {
                        return Err("Cancelled".into())
                    }
                    Err(error) => {
                        failures.record(error);
                    }
                }
            }
        }
        if candidates.is_empty()
            && !pinned
            && !alias_pass.retry
            && (!global_attempt_cap || total_candidate_attempts < attempt_limit)
        {
            if let Some(fingerprint) = mapping_fingerprint.as_ref() {
                if let Ok(Some(mapping)) =
                    store.verified_playback_source(&track.id, fingerprint, &provider)
                {
                    if let Some(candidate) = remembered_for_request(track, &mapping, failed) {
                        trace.event("mapping_resolve_start", &provider, json!({}));
                        attempted_mapping_id = Some(candidate.id.clone());
                        provider_attempt_count = 1;
                        total_candidate_attempts += 1;
                        alias_pass.attempted(&provider);
                        include_candidate_aliases(
                            &mut verified_artist_aliases,
                            track,
                            &candidate,
                            None,
                        );
                        match try_verified_candidate(
                            store,
                            acquisition,
                            track,
                            &verified_artist_aliases,
                            candidate.clone(),
                            &settings,
                            cancel.clone(),
                            fingerprint,
                            trace,
                            deadline,
                        )
                        .await
                        {
                            Ok(source) => {
                                trace.event("mapping_resolve_end", &provider, json!({"ok":true}));
                                return Ok(source);
                            }
                            Err(error) if *cancel.borrow() || error == "Cancelled" => {
                                return Err("Cancelled".into());
                            }
                            Err(error) if is_definitive_source_failure(&error) => {
                                trace.event(
                                    "mapping_resolve_end",
                                    &provider,
                                    json!({"ok":false,"definitive":true}),
                                );
                                let _ = store.invalidate_verified_playback_source(
                                    &track.id,
                                    fingerprint,
                                    &provider,
                                    mapping.provider_id.as_str(),
                                );
                                failures.record(error);
                            }
                            Err(error) => {
                                trace.event(
                                    "mapping_resolve_end",
                                    &provider,
                                    json!({"ok":false,"definitive":false}),
                                );
                                // Preserve the mapping on transient failures while allowing
                                // normal source fallback within this provider to proceed.
                                failures.record(error);
                            }
                        }
                    } else if remembered_candidate(track, &mapping).is_none() {
                        let _ = store.invalidate_verified_playback_source(
                            &track.id,
                            fingerprint,
                            &provider,
                            &mapping.provider_id,
                        );
                    }
                }
            }
        }
        if candidates.is_empty()
            && !pinned
            && !alias_pass.retry
            && (!global_attempt_cap || total_candidate_attempts < attempt_limit)
        {
            trace.event("search_start", &provider, json!({}));
            let found = match acquisition
                .prefetch_playback_candidates_cancelled_traced(
                    track,
                    &alternate_search_artists,
                    std::slice::from_ref(&provider),
                    &settings,
                    cancel.clone(),
                    deadline,
                    trace.clone().with_event_provider(&provider),
                )
                .await
            {
                Ok(found) => found,
                Err(error) if *cancel.borrow() || error == "Cancelled" => {
                    return Err("Cancelled".into());
                }
                Err(error) => {
                    trace.event("search_end", &provider, json!({"ok":false}));
                    failures.record(error);
                    continue;
                }
            };
            trace.event(
                "search_end",
                &provider,
                json!({"ok":true,"found":found.len()}),
            );
            failures.had_results |= !found.is_empty();
            if found.is_empty() {
                alias_pass
                    .empty_search
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let spare = !(provider == "youtube" && track.duration_seconds.is_none())
                && alternate_search_artists.is_empty()
                && track
                    .version
                    .as_deref()
                    .is_none_or(|version| version.trim().is_empty());
            if let Ok(mut queries) = alias_pass.spare_queries.lock() {
                queries.insert(provider.clone(), spare);
            }

            for candidate in &found {
                include_candidate_aliases(&mut verified_artist_aliases, track, candidate, None);
            }
            let mut gate_evidence = CandidateGateEvidence::new_if_trace_enabled(&trace);
            let mut gate_filtered = found
                .into_iter()
                .filter_map(|candidate| {
                    let path = evaluate_candidate_gate_with_proofs(
                        track,
                        verified_artist_aliases.tracks(),
                        verified_artist_aliases.isrc_enriched(),
                        verified_artist_aliases.verified_recording_isrc(),
                        &candidate,
                    );
                    if let Some(evidence) = gate_evidence.as_mut() {
                        evidence.observe_with_proofs(
                            track,
                            verified_artist_aliases.tracks(),
                            verified_artist_aliases.isrc_enriched(),
                            &candidate,
                            path,
                        );
                    }
                    if !path.accepted() && plausible_artist_alias_block(track, &candidate) {
                        alias_pass.remember(&candidate);
                    }
                    path.accepted().then_some((candidate, path))
                })
                .collect::<Vec<_>>();
            // Evaluate every collected row before spending either extraction
            // slot. Early provisional matches must not hide a later candidate
            // that already satisfies recording identity and catalog duration.
            gate_filtered.sort_by_key(|(_, path)| path.evidence_priority());
            let gate_filtered = gate_filtered.into_iter().map(|(candidate, _)| candidate);

            candidates.extend(eligible_after_webview_failure(
                gate_filtered,
                failed,
                attempted_mapping_id.as_deref(),
                remaining_candidate_attempts(
                    attempt_limit,
                    total_candidate_attempts,
                    global_attempt_cap,
                    provider_attempt_count,
                ),
            ));
            if let Some(gate_evidence) = gate_evidence {
                trace.event(
                    "candidate_gate_evidence",
                    &provider,
                    json!({
                        "scope":"candidatesEvaluatedBeforeForegroundCap",
                        "counts":gate_evidence.counts,
                        "candidateRowsLimit":MAX_CANDIDATE_GATE_ROWS,
                        "candidateRowsTruncated":gate_evidence.counts.get("candidatesEvaluatedBeforeForegroundCap").copied().unwrap_or(0) as usize > MAX_CANDIDATE_GATE_ROWS,
                        "durationDeltaBasis":"request-vs-candidate-search-metadata; descriptive-only",
                    }),
                );
                let total_candidates = gate_evidence
                    .counts
                    .get("candidatesEvaluatedBeforeForegroundCap")
                    .copied()
                    .unwrap_or(0) as usize;
                for (chunk_index, rows) in gate_evidence
                    .candidate_rows
                    .chunks(MAX_CANDIDATE_GATE_ROWS_PER_EVENT)
                    .enumerate()
                {
                    trace.event(
                        "candidate_gate_evidence_rows",
                        &provider,
                        json!({
                            "firstOrdinal":chunk_index * MAX_CANDIDATE_GATE_ROWS_PER_EVENT + 1,
                            "totalCandidates":total_candidates,
                            "candidateRowsLimit":MAX_CANDIDATE_GATE_ROWS,
                            "candidateRowsTruncated":total_candidates > MAX_CANDIDATE_GATE_ROWS,
                            "rows":rows,
                        }),
                    );
                }
            }
        }

        let attempt_count = candidates.len();
        trace.event(
            "candidate_gate",
            &provider,
            json!({"eligible":attempt_count}),
        );
        failures.had_results |= !candidates.is_empty();
        for (attempt, mut candidate) in candidates.drain(..).enumerate() {
            if provider_attempt_count >= attempt_limit
                || (global_attempt_cap && total_candidate_attempts >= attempt_limit)
            {
                break;
            }
            total_candidate_attempts += 1;
            provider_attempt_count += 1;
            alias_pass.attempted(&provider);
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
            tried_linked = true;
            let requires_identity_check = candidate.provider == "bandcamp"
                || (!pinned
                    && (matches!(candidate.provider.as_str(), "youtube" | "soundcloud")
                        || !playback_recording_matches_with_proofs(
                            track,
                            verified_artist_aliases.tracks(),
                            verified_artist_aliases.isrc_enriched(),
                            verified_artist_aliases.verified_recording_isrc(),
                            &candidate,
                        )));
            trace.event(
                "extract_and_validate_start",
                &provider,
                json!({"attempt":attempt+1}),
            );
            let prepared_alternate = std::sync::Arc::new(std::sync::Mutex::new(None));
            let resolved = async {
                let audio = acquisition
                .extract_playback_audio_traced_preparing_alternate(
                    &candidate,
                    &settings,
                    cancel.clone(),
                    trace.clone(),
                    deadline,
                    Some(prepared_alternate.clone()),
                )
                .await?;
                    if pinned && matches!(candidate.provider.as_str(), "youtube" | "soundcloud")
                        && !fresh_candidate_audio_identity_matches(track, &candidate, &audio) {
                        return Err("The resolved audio source did not retain the pinned provider recording".to_owned());
                    }
                    include_candidate_aliases(
                        &mut verified_artist_aliases,
                        track,
                        &candidate,
                        Some(&audio),
                    );
                    let proof = verify_resolved_identity_with_aliases_and_proofs(
                        track,
                        verified_artist_aliases.tracks(),
                        verified_artist_aliases.isrc_enriched(),
                        verified_artist_aliases.verified_recording_isrc(),
                        &mut candidate,
                        &audio,
                        requires_identity_check,
                    );
                    let identity_ok = proof.is_some();
                    trace.event("full_identity_check", &provider, json!({"ok":identity_ok,
                        "providerIdMatches":audio.resolved_provider_id.as_deref().is_some_and(|id|candidate.id.split_once(':').map(|(_,value)|value)==Some(id)),
                        "resolvedTitlePresent":audio.resolved_title.as_ref().is_some_and(|title|!title.is_empty()),
                        "catalogDuration":track.duration_seconds,"resolvedDuration":audio.duration_seconds,
                        "topicUploader":candidate.uploader.as_ref().is_some_and(|value|value.trim_end().ends_with("- Topic")),
                        "requestedVersion":track.version.as_ref().is_some_and(|value|!value.trim().is_empty())}));
                    if !identity_ok {
                        let mut fresh = candidate.clone();
                        if let Some(metadata) = &audio.source_metadata {
                            fresh.source_metadata = Some(metadata.clone());
                        }
                        if let Some(title) = &audio.resolved_title {
                            fresh.title = title.clone();
                        }
                        fresh.duration_seconds = audio.duration_seconds.or(fresh.duration_seconds);
                        if plausible_artist_alias_block(track, &fresh) {
                            alias_pass.remember(&fresh);
                        }
                        return Err("The resolved audio source identity does not match this recording".to_owned());
                    }
                    let audio = acquisition.validate_playback_audio_traced(
                        &candidate, audio, cancel.clone(), trace.clone(), deadline,
                    ).await?;
                    Ok::<_, String>((audio, proof))
            }.await;
            trace.event(
                "extract_and_validate_end",
                &provider,
                json!({"attempt":attempt+1,"ok":resolved.is_ok()}),
            );
            match resolved {
                Ok((audio, proof)) => {
                    remember_verified_source(
                        store,
                        track,
                        &candidate,
                        &audio,
                        proof.as_ref().filter(|_| !pinned),
                        mapping_fingerprint.as_deref(),
                    );
                    let duration = audio
                        .duration_seconds
                        .or(candidate.duration_seconds)
                        .filter(|value| value.is_finite() && *value >= 0.);
                    let quality = preview_audio_quality(&audio);
                    return Ok(ResolvedSource {
                        source: AudioSource {
                            input: audio.url,
                            ffmpeg,
                            headers: audio.headers,
                            decoder_hint: Some(AudioSourceDecoderHint {
                                mime_type: Some(audio.mime_type.clone()),
                                codec: audio.codec.clone(),
                                duration_seconds: audio.duration_seconds,
                                content_length: audio.expected_content_length,
                                has_video: audio.selected_format_has_video,
                            }),
                        },
                        quality,
                        candidate: Some(candidate),
                        station: None,
                        duration,
                    });
                }
                Err(error)
                    if candidate.provider != "bandcamp"
                        && attempt_limit > 1
                        && !transient_retry_attempted
                        && provider_attempt_count < attempt_limit
                        && should_retry_same_candidate(&error) =>
                {
                    transient_retry_attempted = true;
                    alias_pass.attempted(&provider);
                    provider_attempt_count += 1;
                    total_candidate_attempts += 1;
                    if *cancel.borrow() {
                        return Err("Cancelled".into());
                    }
                    let prepared = prepared_alternate
                        .lock()
                        .ok()
                        .and_then(|mut slot| slot.take());
                    match acquisition
                        .extract_playback_audio_fresh_or_prepared_traced(
                            &candidate,
                            &settings,
                            cancel.clone(),
                            true,
                            trace.clone(),
                            deadline,
                            prepared,
                        )
                        .await
                    {
                        Ok(audio) => {
                            if pinned
                                && matches!(candidate.provider.as_str(), "youtube" | "soundcloud")
                                && !fresh_candidate_audio_identity_matches(
                                    track, &candidate, &audio,
                                )
                            {
                                failures.record("The resolved audio source did not retain the pinned provider recording".into());
                                continue;
                            }
                            include_candidate_aliases(
                                &mut verified_artist_aliases,
                                track,
                                &candidate,
                                Some(&audio),
                            );
                            let Some(proof) = verify_resolved_identity_with_aliases_and_proofs(
                                track,
                                verified_artist_aliases.tracks(),
                                verified_artist_aliases.isrc_enriched(),
                                verified_artist_aliases.verified_recording_isrc(),
                                &mut candidate,
                                &audio,
                                requires_identity_check,
                            ) else {
                                failures.record("The resolved audio source identity does not match this recording".into());
                                continue;
                            };
                            let audio = match acquisition
                                .validate_playback_audio_traced(
                                    &candidate,
                                    audio,
                                    cancel.clone(),
                                    trace.clone(),
                                    deadline,
                                )
                                .await
                            {
                                Ok(audio) => audio,
                                Err(error) => {
                                    failures.record(error);
                                    continue;
                                }
                            };
                            remember_verified_source(
                                store,
                                track,
                                &candidate,
                                &audio,
                                Some(&proof).filter(|_| !pinned),
                                mapping_fingerprint.as_deref(),
                            );
                            let duration = audio
                                .duration_seconds
                                .or(candidate.duration_seconds)
                                .filter(|value| value.is_finite() && *value >= 0.);
                            let quality = preview_audio_quality(&audio);
                            return Ok(ResolvedSource {
                                source: AudioSource {
                                    input: audio.url,
                                    ffmpeg,
                                    headers: audio.headers,
                                    decoder_hint: Some(AudioSourceDecoderHint {
                                        mime_type: Some(audio.mime_type.clone()),
                                        codec: audio.codec.clone(),
                                        duration_seconds: audio.duration_seconds,
                                        content_length: audio.expected_content_length,
                                        has_video: audio.selected_format_has_video,
                                    }),
                                },
                                quality,
                                candidate: Some(candidate),
                                station: None,
                                duration,
                            });
                        }
                        Err(retry_error) => {
                            failures.record(retry_error);
                            if pinned {
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    trace.event(
                        "extract_and_validate_end",
                        &provider,
                        json!({"attempt":attempt+1,"ok":false}),
                    );
                    failures.record(error);
                }
            }
        }
        // A pinned video, chapter, or user-selected source must never switch recordings.
        if pinned && (tried_linked || attempt_count > 0) {
            break;
        }
    }
    Err(failures.finish(track.duration_seconds.is_none()))
}

fn candidate_artist_spellings(
    candidate: &Candidate,
    audio: Option<&acquisition::PreviewAudio>,
) -> Vec<String> {
    let mut names = Vec::new();
    for metadata in candidate
        .source_metadata
        .iter()
        .chain(audio.and_then(|audio| audio.source_metadata.as_ref()))
    {
        for key in ["artist", "catalogArtist", "channel", "uploader"] {
            if let Some(value) = metadata[key].as_str() {
                names.push(value.trim_end().trim_end_matches(" - Topic").to_owned());
            }
        }
        for key in ["artists", "artistNames"] {
            if let Some(values) = metadata[key].as_array() {
                let names_joined = values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !names_joined.is_empty() {
                    names.push(names_joined);
                }
            }
        }
    }
    if let Some(value) = candidate.uploader.as_deref() {
        names.push(value.trim_end().trim_end_matches(" - Topic").to_owned());
    }
    if let Some(artist) = crate::source_fallback::youtube_flat_artist_credit(&candidate.title) {
        names.push(artist);
    }
    if let Some(artist) = audio
        .and_then(|audio| audio.resolved_title.as_deref())
        .and_then(crate::source_fallback::youtube_flat_artist_credit)
    {
        names.push(artist);
    }
    if let Some((artist, _)) = candidate.title.split_once(" - ") {
        names.push(artist.to_owned());
    }
    if let Some((artist, _)) = audio
        .and_then(|audio| audio.resolved_title.as_deref())
        .and_then(|title| title.split_once(" - "))
    {
        names.push(artist.to_owned());
    }
    names.sort();
    names.dedup();
    names.retain(|name| !name.is_empty() && name.len() <= 4096);
    names
}
fn include_candidate_aliases(
    context: &mut crate::artist_credit_aliases::VerifiedPlaybackArtistAliases,
    track: &Track,
    candidate: &Candidate,
    audio: Option<&acquisition::PreviewAudio>,
) {
    for artist in candidate_artist_spellings(candidate, audio) {
        context.include_candidate_artist(track, &artist);
    }
}
fn plausible_artist_alias_block(track: &Track, candidate: &Candidate) -> bool {
    if candidate.permission == "restricted" {
        return false;
    }
    candidate_artist_spellings(candidate, None)
        .into_iter()
        .any(|artist| {
            if crate::matching::normalize(&artist) == crate::matching::normalize(&track.artist) {
                return false;
            }
            let mut probe = track.clone();
            probe.artist = artist;
            crate::source_fallback::playback_recording_matches(&probe, candidate)
                || crate::source_fallback::is_topic_playback_candidate(&probe, candidate)
                || crate::source_fallback::is_provisional_playback_candidate(&probe, candidate)
        })
}

fn valid_artist_alias_variant(track: &Track, variant: &Track) -> bool {
    let mut restored = variant.clone();
    restored.artist = track.artist.clone();
    &restored == track
}

fn valid_artist_alias_variant_with_proof(
    track: &Track,
    variant: &Track,
    isrc_enriched_aliases: &[Track],
) -> bool {
    if valid_artist_alias_variant(track, variant) {
        return true;
    }
    if !isrc_enriched_aliases.contains(variant) {
        return false;
    }
    let mut restored = variant.clone();
    restored.artist = track.artist.clone();
    restored.isrc = track.isrc.clone();
    &restored == track
}

fn conflicts_with_verified_recording_isrc(
    candidate: &Candidate,
    verified_recording_isrc: Option<&str>,
) -> bool {
    conflicts_with_verified_recording_isrc_metadata(
        candidate.source_metadata.as_ref(),
        verified_recording_isrc,
    )
}

fn conflicts_with_verified_recording_isrc_metadata(
    metadata: Option<&Value>,
    verified_recording_isrc: Option<&str>,
) -> bool {
    let Some(expected) = verified_recording_isrc else {
        return false;
    };
    metadata
        .and_then(|metadata| metadata.get("isrc"))
        .and_then(Value::as_str)
        .is_some_and(|actual| {
            !actual.trim().is_empty() && !expected.eq_ignore_ascii_case(actual.trim())
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CandidateGatePath {
    RecordingCanonical,
    RecordingVerifiedAlias,
    TopicCanonical,
    TopicVerifiedAlias,
    MusicCatalogLocator,
    ProvisionalCanonical,
    ProvisionalVerifiedAlias,
    Rejected,
}

impl CandidateGatePath {
    fn counter(self) -> &'static str {
        match self {
            Self::RecordingCanonical => "acceptedRecordingCanonical",
            Self::RecordingVerifiedAlias => "acceptedRecordingVerifiedAlias",
            Self::TopicCanonical => "acceptedTopicCanonical",
            Self::TopicVerifiedAlias => "acceptedTopicVerifiedAlias",
            Self::MusicCatalogLocator => "acceptedMusicCatalogLocator",
            Self::ProvisionalCanonical => "acceptedProvisionalCanonical",
            Self::ProvisionalVerifiedAlias => "acceptedProvisionalVerifiedAlias",
            Self::Rejected => "rejectedAllGates",
        }
    }

    fn accepted(self) -> bool {
        self != Self::Rejected
    }

    fn evidence_priority(self) -> u8 {
        match self {
            Self::RecordingCanonical | Self::RecordingVerifiedAlias => 0,
            Self::TopicCanonical | Self::TopicVerifiedAlias => 1,
            Self::MusicCatalogLocator => 2,
            Self::ProvisionalCanonical | Self::ProvisionalVerifiedAlias => 3,
            Self::Rejected => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RecordingCanonical => "recording_canonical",
            Self::RecordingVerifiedAlias => "recording_verified_alias",
            Self::TopicCanonical => "topic_canonical",
            Self::TopicVerifiedAlias => "topic_verified_alias",
            Self::MusicCatalogLocator => "music_catalog_locator",
            Self::ProvisionalCanonical => "provisional_canonical",
            Self::ProvisionalVerifiedAlias => "provisional_verified_alias",
            Self::Rejected => "rejected",
        }
    }
}

/// Reuses the live outer gate predicates in their original short-circuit order
/// and records only the first accepting predicate family. The recording
/// predicate itself includes `topic_original_playback_matches`, so these
/// counters do not distinguish that internal Topic subpath from its other
/// recording matches. Independent evidence counters below are not causal
/// failure labels.
fn evaluate_candidate_gate(
    track: &Track,
    aliases: &[Track],
    candidate: &Candidate,
) -> CandidateGatePath {
    evaluate_candidate_gate_with_proofs(track, aliases, &[], None, candidate)
}

fn evaluate_candidate_gate_with_proofs(
    track: &Track,
    aliases: &[Track],
    isrc_enriched_aliases: &[Track],
    verified_recording_isrc: Option<&str>,
    candidate: &Candidate,
) -> CandidateGatePath {
    if conflicts_with_verified_recording_isrc(candidate, verified_recording_isrc) {
        return CandidateGatePath::Rejected;
    }
    if crate::source_fallback::playback_recording_matches(track, candidate) {
        return CandidateGatePath::RecordingCanonical;
    }
    if aliases
        .iter()
        .filter(|variant| {
            valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
        })
        .any(|variant| crate::source_fallback::playback_recording_matches(variant, candidate))
    {
        return CandidateGatePath::RecordingVerifiedAlias;
    }
    if crate::source_fallback::is_topic_playback_candidate(track, candidate) {
        return CandidateGatePath::TopicCanonical;
    }
    if crate::source_fallback::is_provisional_youtube_music_catalog_candidate(track, candidate)
        && crate::source_fallback::is_provisional_playback_candidate(track, candidate)
    {
        return CandidateGatePath::MusicCatalogLocator;
    }
    if aliases
        .iter()
        .filter(|variant| {
            valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
        })
        .any(|variant| crate::source_fallback::is_topic_playback_candidate(variant, candidate))
    {
        return CandidateGatePath::TopicVerifiedAlias;
    }
    if crate::source_fallback::is_provisional_playback_candidate(track, candidate) {
        return CandidateGatePath::ProvisionalCanonical;
    }
    if aliases
        .iter()
        .filter(|variant| {
            valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
        })
        .any(|variant| {
            crate::source_fallback::is_provisional_playback_candidate(variant, candidate)
        })
    {
        return CandidateGatePath::ProvisionalVerifiedAlias;
    }
    CandidateGatePath::Rejected
}

/// Evidence counters are collected only when SourceTrace is enabled. Duration
/// deltas are descriptive raw fields, not effective gate thresholds: Topic
/// matching allows 2s, provisional matching 7s, and Monochrome has an exact
/// catalog-title exception when requested duration is absent.
const MAX_CANDIDATE_GATE_ROWS: usize = 200;
const MAX_CANDIDATE_GATE_ROWS_PER_EVENT: usize = 12;

#[derive(Default)]
struct CandidateGateEvidence {
    counts: std::collections::BTreeMap<&'static str, u64>,
    candidate_rows: Vec<Value>,
}

impl CandidateGateEvidence {
    fn new_if_trace_enabled(trace: &SourceTrace) -> Option<Self> {
        trace.is_enabled().then(Self::default)
    }

    fn increment(&mut self, key: &'static str) {
        *self.counts.entry(key).or_default() += 1;
    }

    fn observe(
        &mut self,
        track: &Track,
        aliases: &[Track],
        candidate: &Candidate,
        path: CandidateGatePath,
    ) {
        self.observe_with_proofs(track, aliases, &[], candidate, path);
    }

    fn observe_with_proofs(
        &mut self,
        track: &Track,
        aliases: &[Track],
        isrc_enriched_aliases: &[Track],
        candidate: &Candidate,
        path: CandidateGatePath,
    ) {
        self.increment("candidatesEvaluatedBeforeForegroundCap");
        self.increment(path.counter());
        let ordinal = self.counts["candidatesEvaluatedBeforeForegroundCap"] as usize;
        if self.candidate_rows.len() < MAX_CANDIDATE_GATE_ROWS {
            let provider_class = match candidate.provider.as_str() {
                "youtube" => "youtube",
                "soundcloud" => "soundcloud",
                "monochrome" => "monochrome",
                _ => "other",
            };
            let confidence_class = match candidate.confidence.as_str() {
                "high" => "high",
                "review" => "review",
                "low" => "low",
                _ => "unknown",
            };
            let permission_class = match candidate.permission.as_str() {
                "restricted" => "restricted",
                "unknown" => "unknown",
                "allowed" | "unrestricted" => "allowed",
                _ => "other",
            };
            let requested_version =
                crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
            let candidate_title = crate::matching::normalize(&candidate.title);
            let requested_title = crate::matching::normalize(&track.title);
            let requested_title_phrase_present = !requested_title.is_empty()
                && format!(" {candidate_title} ").contains(&format!(" {requested_title} "));
            let catalog_title_exact = candidate
                .source_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("catalogTitle"))
                .and_then(Value::as_str)
                .is_some_and(|title| crate::matching::normalize(title) == requested_title);
            let canonical_artist = crate::matching::normalize(&track.artist);
            let canonical_artist_phrase_present = !canonical_artist.is_empty()
                && format!(" {candidate_title} ").contains(&format!(" {canonical_artist} "));
            let verified_alias_artist_phrase_present = aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    let artist = crate::matching::normalize(&variant.artist);
                    !artist.is_empty()
                        && format!(" {candidate_title} ").contains(&format!(" {artist} "))
                });
            let duration_within_five_seconds = track
                .duration_seconds
                .filter(|value| value.is_finite() && *value > 0.0)
                .zip(
                    candidate
                        .duration_seconds
                        .filter(|value| value.is_finite() && *value > 0.0),
                )
                .is_some_and(|(expected, actual)| (expected - actual).abs() <= 5.0);
            let version_present = !requested_version.is_empty();
            let version_matches = !version_present
                || matches!(requested_version.as_str(), "original" | "original mix")
                || format!(" {candidate_title} ").contains(&format!(" {requested_version} "));
            let version_conflict_flag = candidate
                .reasons
                .iter()
                .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"));
            let recording_canonical_match =
                crate::source_fallback::playback_recording_matches(track, candidate);
            let recording_verified_alias_match = aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    crate::source_fallback::playback_recording_matches(variant, candidate)
                });
            let topic_canonical_match =
                crate::source_fallback::is_topic_playback_candidate(track, candidate);
            let topic_verified_alias_match = aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    crate::source_fallback::is_topic_playback_candidate(variant, candidate)
                });
            let provisional_canonical_match =
                crate::source_fallback::is_provisional_playback_candidate(track, candidate);
            let provisional_verified_alias_match = aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    crate::source_fallback::is_provisional_playback_candidate(variant, candidate)
                });
            self.candidate_rows.push(json!({
                "ordinal": ordinal,
                "providerClass": provider_class,
                "confidenceClass": confidence_class,
                "permissionClass": permission_class,
                "gate": path.label(),
                "accepted": path.accepted(),
                "requestedTitlePhrasePresent": requested_title_phrase_present,
                "catalogTitleExact": catalog_title_exact,
                "canonicalArtistPhrasePresent": canonical_artist_phrase_present,
                "verifiedAliasArtistPhrasePresent": verified_alias_artist_phrase_present,
                "recordingCanonicalMatch": recording_canonical_match,
                "recordingVerifiedAliasMatch": recording_verified_alias_match,
                "topicCanonicalMatch": topic_canonical_match,
                "topicVerifiedAliasMatch": topic_verified_alias_match,
                "provisionalCanonicalMatch": provisional_canonical_match,
                "provisionalVerifiedAliasMatch": provisional_verified_alias_match,
                "descriptiveDurationWithinFiveSeconds": duration_within_five_seconds,
                "requestedVersionPresent": version_present,
                "descriptiveVersionTextPresentOrNotRequired": version_matches,
                "versionConflictFlag": version_conflict_flag,
            }));
        }
        if candidate.permission == "restricted" {
            self.increment("permissionRestricted");
        }
        if candidate.confidence == "low" {
            self.increment("confidenceLow");
        }
        if candidate
            .reasons
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("version conflicts"))
        {
            self.increment("versionConflictReasonFlagAsciiCaseInsensitive");
        }
        let expected_version =
            crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
        if !expected_version.is_empty() {
            self.increment("requestedVersionPresent");
        }
        if !expected_version.is_empty()
            && format!(" {} ", crate::matching::normalize(&candidate.title))
                .contains(&format!(" {expected_version} "))
        {
            self.increment("requestedVersionTextPresentInCandidateTitle");
        }
        let expected_duration = track
            .duration_seconds
            .filter(|value| value.is_finite() && *value > 0.0);
        let candidate_duration = candidate
            .duration_seconds
            .filter(|value| value.is_finite() && *value > 0.0);
        if expected_duration.is_none() {
            self.increment("expectedDurationMissingOrInvalid");
        }
        if candidate_duration.is_none() {
            self.increment("candidateDurationMissingOrInvalid");
        }
        if let (Some(expected), Some(actual)) = (expected_duration, candidate_duration) {
            if (expected - actual).abs() <= 5.0 {
                self.increment("durationDeltaLe5s");
            } else {
                self.increment("durationDeltaGt5s");
            }
        }
        let requested_title = crate::matching::normalize(&track.title);
        let candidate_title = crate::matching::normalize(&candidate.title);
        if !requested_title.is_empty()
            && format!(" {candidate_title} ").contains(&format!(" {requested_title} "))
        {
            self.increment("requestedTitlePhrasePresentInCandidateTitle");
        }
        let canonical_artist = crate::matching::normalize(&track.artist);
        if !canonical_artist.is_empty()
            && format!(" {candidate_title} ").contains(&format!(" {canonical_artist} "))
        {
            self.increment("canonicalArtistPhrasePresentInCandidateTitle");
        }
        for alias in aliases.iter().filter(|variant| {
            valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
        }) {
            let alias_artist = crate::matching::normalize(&alias.artist);
            if !alias_artist.is_empty()
                && format!(" {candidate_title} ").contains(&format!(" {alias_artist} "))
            {
                self.increment("verifiedAliasArtistPhrasePresentInCandidateTitle");
                break;
            }
        }
        if candidate.provider == "monochrome"
            && candidate
                .source_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("catalogTitle"))
                .and_then(Value::as_str)
                .is_some_and(|title| crate::matching::normalize(title) == requested_title)
        {
            self.increment("monochromeCatalogTitleExact");
        }
        let normalize_credit = |value: &str| {
            crate::matching::normalize(
                &value
                    .to_lowercase()
                    .replace('&', " and ")
                    .replace(',', " and "),
            )
            .split_whitespace()
            .map(|word| match word {
                "feat" | "featuring" => "and",
                other => other,
            })
            .collect::<Vec<_>>()
            .join(" ")
        };
        let uploader = normalize_credit(candidate.uploader.as_deref().unwrap_or_default());
        if !uploader.is_empty() && uploader == normalize_credit(&track.artist) {
            self.increment("uploaderMatchesCanonicalArtistCredit");
        }
        if !uploader.is_empty()
            && aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| uploader == normalize_credit(&variant.artist))
        {
            self.increment("uploaderMatchesVerifiedAliasCredit");
        }
    }
}

fn playback_recording_matches_with_aliases(
    track: &Track,
    aliases: &[Track],
    candidate: &Candidate,
) -> bool {
    playback_recording_matches_with_proofs(track, aliases, &[], None, candidate)
}

fn playback_recording_matches_with_proofs(
    track: &Track,
    aliases: &[Track],
    isrc_enriched_aliases: &[Track],
    verified_recording_isrc: Option<&str>,
    candidate: &Candidate,
) -> bool {
    !conflicts_with_verified_recording_isrc(candidate, verified_recording_isrc)
        && (crate::source_fallback::playback_recording_matches(track, candidate)
            || aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    crate::source_fallback::playback_recording_matches(variant, candidate)
                }))
}

fn topic_playback_matches_with_aliases(
    track: &Track,
    aliases: &[Track],
    candidate: &Candidate,
) -> bool {
    topic_playback_matches_with_proofs(track, aliases, &[], None, candidate)
}

fn topic_playback_matches_with_proofs(
    track: &Track,
    aliases: &[Track],
    isrc_enriched_aliases: &[Track],
    verified_recording_isrc: Option<&str>,
    candidate: &Candidate,
) -> bool {
    !conflicts_with_verified_recording_isrc(candidate, verified_recording_isrc)
        && (crate::source_fallback::is_topic_playback_candidate(track, candidate)
            || aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    crate::source_fallback::is_topic_playback_candidate(variant, candidate)
                }))
}

fn provisional_playback_matches_with_aliases(
    track: &Track,
    aliases: &[Track],
    candidate: &Candidate,
) -> bool {
    provisional_playback_matches_with_proofs(track, aliases, &[], None, candidate)
}

fn provisional_playback_matches_with_proofs(
    track: &Track,
    aliases: &[Track],
    isrc_enriched_aliases: &[Track],
    verified_recording_isrc: Option<&str>,
    candidate: &Candidate,
) -> bool {
    !conflicts_with_verified_recording_isrc(candidate, verified_recording_isrc)
        && (crate::source_fallback::is_provisional_playback_candidate(track, candidate)
            || aliases
                .iter()
                .filter(|variant| {
                    valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
                })
                .any(|variant| {
                    crate::source_fallback::is_provisional_playback_candidate(variant, candidate)
                }))
}

fn accept_resolved_identity_with_aliases(
    track: &Track,
    aliases: &[Track],
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
) -> bool {
    accept_resolved_identity_with_aliases_and_proofs(
        track,
        aliases,
        &[],
        None,
        candidate,
        audio,
        required,
    )
}

fn accept_resolved_identity_with_aliases_and_proofs(
    track: &Track,
    aliases: &[Track],
    isrc_enriched_aliases: &[Track],
    verified_recording_isrc: Option<&str>,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
) -> bool {
    verify_resolved_identity_with_aliases_and_proofs(
        track,
        aliases,
        isrc_enriched_aliases,
        verified_recording_isrc,
        candidate,
        audio,
        required,
    )
    .is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordingProofKind {
    Recording,
    VerifiedExtractor,
    YoutubeMusicCatalog,
    ExplicitSource,
}

/// Produced only by fresh resolved-identity verification. Persist policy rather
/// than channel/artist evidence, which must be obtained again on every reuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VerifiedRecordingProof {
    kind: RecordingProofKind,
}

impl VerifiedRecordingProof {
    fn persisted(self) -> Option<serde_json::Value> {
        let kind = match self.kind {
            RecordingProofKind::Recording => "recording",
            RecordingProofKind::VerifiedExtractor => "verified_extractor",
            RecordingProofKind::YoutubeMusicCatalog => "youtube_music_catalog",
            RecordingProofKind::ExplicitSource => return None,
        };
        Some(json!({"version":1,"kind":kind}))
    }

    fn catalog_locator(value: Option<&serde_json::Value>) -> bool {
        value.is_some_and(|value| {
            value["version"].as_u64() == Some(1)
                && value["kind"].as_str() == Some("youtube_music_catalog")
        })
    }
}

fn verify_resolved_identity_with_aliases_and_proofs(
    track: &Track,
    aliases: &[Track],
    isrc_enriched_aliases: &[Track],
    verified_recording_isrc: Option<&str>,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
) -> Option<VerifiedRecordingProof> {
    if conflicts_with_verified_recording_isrc(candidate, verified_recording_isrc)
        || conflicts_with_verified_recording_isrc_metadata(
            audio.source_metadata.as_ref(),
            verified_recording_isrc,
        )
    {
        return None;
    }
    let original = candidate.clone();
    let mut checked = original.clone();
    if let Some(proof) = verify_resolved_identity(track, &mut checked, audio, required) {
        *candidate = checked;
        return Some(proof);
    }
    for variant in aliases.iter().filter(|variant| {
        valid_artist_alias_variant_with_proof(track, variant, isrc_enriched_aliases)
    }) {
        let mut checked = original.clone();
        if let Some(proof) = verify_resolved_identity(variant, &mut checked, audio, required) {
            *candidate = checked;
            return Some(proof);
        }
    }
    None
}

fn accept_resolved_identity(
    track: &Track,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
) -> bool {
    verify_resolved_identity(track, candidate, audio, required).is_some()
}

fn verify_resolved_identity(
    track: &Track,
    candidate: &mut Candidate,
    audio: &acquisition::PreviewAudio,
    required: bool,
) -> Option<VerifiedRecordingProof> {
    let youtube_music_catalog_locator = candidate
        .source_metadata
        .as_ref()
        .is_some_and(|metadata| metadata["playbackMusicCatalogCandidate"].as_bool() == Some(true));
    let no_catalog_duration_youtube = required
        && candidate.provider == "youtube"
        && crate::matching::ephemeral_playback_identity(track)
            .duration_seconds
            .is_none();
    let extractor_provider = matches!(
        candidate.provider.as_str(),
        "youtube" | "soundcloud" | "bandcamp"
    );
    if candidate.provider == "bandcamp" && candidate.id.is_empty() {
        let Some(id) = audio
            .resolved_provider_id
            .as_deref()
            .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        else {
            return None;
        };
        candidate.id = format!("bandcamp:{id}");
    }
    if required && candidate.provider == "bandcamp" {
        let resolved_artist = audio.source_metadata.as_ref().and_then(|metadata| {
            metadata["channel"]
                .as_str()
                .or_else(|| metadata["uploader"].as_str())
        });
        if resolved_artist.is_none_or(|artist| {
            let artist = crate::matching::normalize(artist);
            artist.is_empty() || artist != crate::matching::normalize(&track.artist)
        }) {
            return None;
        }
    }
    // Fresh extraction replaces search/cache evidence even when it is absent.
    // A verified channel is useful proof, never a reason to retain stale fields.
    if extractor_provider {
        candidate.source_metadata = audio.source_metadata.clone();
    }
    if let Some(chapters) = audio.chapters.as_ref() {
        candidate.chapters = Some(chapters.clone());
    }
    let resolved_id_matches = audio
        .resolved_provider_id
        .as_deref()
        .is_some_and(|resolved_id| {
            candidate.id.split_once(':').map(|(_, id)| id) == Some(resolved_id)
        });
    if required
        && extractor_provider
        && (!resolved_id_matches || audio.resolved_title.as_deref().is_none_or(str::is_empty))
    {
        return None;
    }
    if no_catalog_duration_youtube {
        let Some(metadata) = audio.source_metadata.as_ref() else {
            // Never let uploader/channel evidence inherited from search or a
            // remembered source stand in for this extractor response.
            candidate.source_metadata = None;
            return None;
        };
        if metadata["provider"].as_str() != Some("youtube")
            || metadata["channelIsVerified"].as_bool() != Some(true)
        {
            return None;
        }
    }
    if extractor_provider && resolved_id_matches {
        // Remembered mappings retain recording identity, not a stale search
        // channel label. Restore the freshly extracted channel so Topic-source
        // verification behaves identically on a cache miss and a cache hit.
        candidate.uploader = audio
            .source_metadata
            .as_ref()
            .and_then(|metadata| {
                metadata["channel"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        metadata["uploader"]
                            .as_str()
                            .filter(|value| !value.is_empty())
                    })
            })
            .map(str::to_owned);
        if let Some(title) = audio.resolved_title.as_ref() {
            candidate.title = title.clone();
            // Search confidence cannot override the metadata returned by the
            // extractor. Re-evaluate the actual title and duration instead.
            candidate.confidence = "review".into();
        }
        candidate.duration_seconds = audio.duration_seconds;
    }
    if !required {
        return Some(VerifiedRecordingProof {
            kind: RecordingProofKind::ExplicitSource,
        });
    }
    let verified_integer_duration = candidate.provider == "youtube"
        && resolved_id_matches
        && audio
            .resolved_title
            .as_deref()
            .is_some_and(|title| !title.is_empty())
        && audio.duration_seconds.is_some_and(|duration| {
            duration.is_finite() && duration > 0.0 && duration.fract().abs() < f64::EPSILON
        });
    let verified_missing_duration_youtube = no_catalog_duration_youtube
        && audio.duration_seconds.is_some_and(|duration| {
            duration.is_finite() && duration > 30.0 && duration <= 86_400.0
        });
    let kind = if (verified_missing_duration_youtube || youtube_music_catalog_locator)
        && crate::source_fallback::playback_music_catalog_recording_matches_verified_extractor(
            track, candidate,
        ) {
        RecordingProofKind::YoutubeMusicCatalog
    } else if verified_integer_duration || verified_missing_duration_youtube {
        if !crate::source_fallback::playback_recording_matches_verified_extractor(track, candidate)
        {
            return None;
        }
        RecordingProofKind::VerifiedExtractor
    } else {
        if !crate::source_fallback::playback_recording_matches(track, candidate) {
            return None;
        }
        RecordingProofKind::Recording
    };
    if kind == RecordingProofKind::YoutubeMusicCatalog {
        if let Some(metadata) = candidate
            .source_metadata
            .as_mut()
            .and_then(Value::as_object_mut)
        {
            // Keep only the locator policy alongside fresh evidence so a same-
            // candidate transport refresh can enter the same verification path.
            metadata.insert("playbackMusicCatalogCandidate".into(), json!(true));
            metadata.insert("catalogTitle".into(), json!(track.title));
            metadata.insert("catalogArtist".into(), json!(track.artist));
        }
    }
    Some(VerifiedRecordingProof { kind })
}

fn recording_identity_fingerprint(track: &Track) -> Option<String> {
    let normalize = |value: Option<&str>| {
        value
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let duration_ms = track
        .duration_seconds
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| (value * 1000.0).round() as i64);
    let range = track.playback_range.as_ref().map(|range| {
        json!({
            "parentTrackId": range.parent_track_id,
            "chapterId": range.chapter_id,
            "startMs": (range.start_time * 1000.0).round() as i64,
            "endMs": (range.end_time * 1000.0).round() as i64,
        })
    });
    let identity = json!({
        "title": normalize(Some(&track.title)),
        "artist": normalize(Some(&track.artist)),
        "albumArtist": normalize(track.album_artist.as_deref()),
        "version": normalize(track.version.as_deref()),
        "durationMs": duration_ms,
        "isrc": track.isrc.as_deref().map(|value| value.chars().filter(|ch| !ch.is_whitespace()).collect::<String>().to_ascii_uppercase()),
        "playbackRange": range,
    });
    let bytes = serde_json::to_vec(&identity).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn remembered_candidate(
    track: &Track,
    source: &crate::storage::VerifiedPlaybackSource,
) -> Option<Candidate> {
    let candidate_id = match source.provider.as_str() {
        "youtube" => {
            let id = acquisition::youtube_video_id(&source.permanent_url)?;
            if id != source.provider_id {
                return None;
            }
            if source.permanent_url != format!("https://www.youtube.com/watch?v={id}") {
                return None;
            }
            format!("youtube:{id}")
        }
        "soundcloud" => {
            if !source
                .provider_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return None;
            }
            let canonical = linked_candidate(&Track {
                source_url: source.permanent_url.clone(),
                ..Track::default()
            })?;
            if canonical.provider != "soundcloud" {
                return None;
            }
            format!("soundcloud:{}", source.provider_id)
        }
        _ => return None,
    };
    Some(Candidate {
        id: candidate_id,
        provider: source.provider.clone(),
        url: source.permanent_url.clone(),
        title: track.title.clone(),
        duration_seconds: track.duration_seconds,
        permission: "unknown".into(),
        confidence: "review".into(),
        source_metadata: VerifiedRecordingProof::catalog_locator(
            source.verification_proof.as_ref(),
        )
        .then(|| json!({"provider":"youtube","playbackMusicCatalogCandidate":true})),
        ..Candidate::default()
    })
}

fn remember_verified_source(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    audio: &acquisition::PreviewAudio,
    proof: Option<&VerifiedRecordingProof>,
    fingerprint: Option<&str>,
) {
    let Some(persisted_proof) = proof.and_then(|proof| proof.persisted()) else {
        return;
    };
    if !audio.provider_session_current()
        || audio.resolved_title.as_deref().is_none_or(str::is_empty)
        || candidate.id.split_once(':').map(|(_, id)| id) != audio.resolved_provider_id.as_deref()
    {
        return;
    }
    let Some(fingerprint) = fingerprint else {
        return;
    };
    let Some(provider_id) = candidate
        .id
        .strip_prefix(&format!("{}:", candidate.provider))
    else {
        return;
    };
    let permanent_url = match candidate.provider.as_str() {
        "youtube" => {
            if acquisition::youtube_video_id(&candidate.url).as_deref() != Some(provider_id) {
                return;
            }
            format!("https://www.youtube.com/watch?v={provider_id}")
        }
        "soundcloud" => {
            let Some(canonical) = linked_candidate(&Track {
                source_url: candidate.url.clone(),
                ..Track::default()
            }) else {
                return;
            };
            if canonical.provider != "soundcloud" {
                return;
            }
            canonical.url
        }
        _ => return,
    };
    let _ = store.save_verified_playback_source_with_proof(
        &track.id,
        fingerprint,
        &candidate.provider,
        provider_id,
        &permanent_url,
        &persisted_proof,
    );
}

fn is_definitive_source_failure(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    if [
        "cancel",
        "timeout",
        "timed out",
        "429",
        "500",
        "502",
        "503",
        "504",
        "rate limit",
        "temporar",
        "connection reset",
        "network",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        return false;
    }
    [
        "identity changed",
        "identity mismatch",
        "http 404",
        "http 410",
        "not found",
        "unavailable",
        "removed",
        "private video",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

async fn try_verified_candidate(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    context: &crate::artist_credit_aliases::VerifiedPlaybackArtistAliases,
    mut candidate: Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    fingerprint: &str,
    trace: &SourceTrace,
    deadline: PlaybackDeadline,
) -> Result<ResolvedSource, String> {
    let audio = acquisition
        .extract_playback_audio_traced_preparing_alternate(
            &candidate,
            settings,
            cancel.clone(),
            trace.clone(),
            deadline,
            None,
        )
        .await?;
    if audio.resolved_title.as_deref().is_none_or(str::is_empty)
        || candidate.id.split_once(':').map(|(_, id)| id) != audio.resolved_provider_id.as_deref()
    {
        return Err("Resolved source identity could not be verified".into());
    }
    let mut context = context.clone();
    include_candidate_aliases(&mut context, track, &candidate, Some(&audio));
    let aliases = context.tracks();
    let isrc_enriched_aliases = context.isrc_enriched();
    let verified_recording_isrc = context.verified_recording_isrc();
    let proof = verify_resolved_identity_with_aliases_and_proofs(
        track,
        aliases,
        isrc_enriched_aliases,
        verified_recording_isrc,
        &mut candidate,
        &audio,
        true,
    );
    let identity_ok = proof.is_some();
    trace.event("mapping_identity_check", &candidate.provider, json!({"ok":identity_ok,
        "providerIdMatches":audio.resolved_provider_id.as_deref().is_some_and(|id|candidate.id.split_once(':').map(|(_,value)|value)==Some(id)),
        "resolvedTitlePresent":audio.resolved_title.as_ref().is_some_and(|title|!title.is_empty()),
        "catalogDuration":track.duration_seconds,"resolvedDuration":audio.duration_seconds,
        "topicUploader":candidate.uploader.as_ref().is_some_and(|value|value.trim_end().ends_with("- Topic"))}));
    if !identity_ok {
        return Err("Verified source identity mismatch".into());
    }
    let audio = acquisition
        .validate_playback_audio_traced(&candidate, audio, cancel.clone(), trace.clone(), deadline)
        .await?;
    remember_verified_source(
        store,
        track,
        &candidate,
        &audio,
        proof.as_ref(),
        Some(fingerprint),
    );
    let duration = audio
        .duration_seconds
        .or(candidate.duration_seconds)
        .filter(|value| value.is_finite() && *value >= 0.);
    let quality = preview_audio_quality(&audio);
    Ok(ResolvedSource {
        source: AudioSource {
            input: audio.url,
            ffmpeg: crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"),
            headers: audio.headers,
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some(audio.mime_type.clone()),
                codec: audio.codec.clone(),
                duration_seconds: audio.duration_seconds,
                content_length: audio.expected_content_length,
                has_video: audio.selected_format_has_video,
            }),
        },
        quality,
        candidate: Some(candidate),
        station: None,
        duration,
    })
}

fn overrides_import_url(track: &Track, settings: &Settings) -> bool {
    settings.override_playlist_track_urls
        && track.provider == "import"
        && track.playback_range.is_none()
        && track.provenance["sourceKind"].as_str() != Some("youtubeRadio")
}

fn should_retry_same_candidate(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    if error == "cancelled" || error.contains("not found") {
        return false;
    }
    [
        "http 403",
        "http 410",
        "http 429",
        "http 5",
        "timed out",
        "timeout",
        "stream access",
        "streamaccess",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

/// Resolve a fresh stream for the same candidate after a decoder or CDN failure.
pub async fn recover(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    candidate: &Candidate,
    alternate_format: bool,
    cancel: watch::Receiver<bool>,
) -> Result<ResolvedSource, String> {
    recover_with_trace_context(
        store,
        acquisition,
        track,
        candidate,
        alternate_format,
        cancel,
        None,
        None,
        None,
    )
    .await
}

pub(crate) async fn recover_with_trace_context(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    candidate: &Candidate,
    alternate_format: bool,
    cancel: watch::Receiver<bool>,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
) -> Result<ResolvedSource, String> {
    let deadline = PlaybackDeadline::after(store.settings()?.playback_startup_timeout());
    recover_with_trace_context_until(
        store,
        acquisition,
        track,
        candidate,
        alternate_format,
        cancel,
        native_operation_id,
        native_seek_operation_id,
        webview_generation,
        deadline,
        true,
    )
    .await
}

pub(crate) async fn recover_with_trace_context_until(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    candidate: &Candidate,
    alternate_format: bool,
    cancel: watch::Receiver<bool>,
    native_operation_id: Option<String>,
    native_seek_operation_id: Option<String>,
    webview_generation: Option<u64>,
    deadline: PlaybackDeadline,
    strict_recording_verification: bool,
) -> Result<ResolvedSource, String> {
    let trace = SourceTrace::new(
        native_operation_id,
        native_seek_operation_id,
        webview_generation,
    );
    trace.event(
        "recovery_start",
        &candidate.provider,
        json!({"alternateFormat":alternate_format}),
    );
    let result = tokio::select! {
        biased;
        _ = wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        result = within_shared_deadline(
            deadline,
            Box::pin(recover_inner(store, acquisition, track, candidate, alternate_format,
                cancel.clone(), trace.clone(), deadline, strict_recording_verification)),
        ) => result,
    };
    let result = normalize_resolver_deadline(result, deadline, *cancel.borrow());
    trace.event("recovery_end", &candidate.provider, json!({"ok":result.is_ok(), "cancelled":result.as_ref().err().is_some_and(|e| e == "Cancelled"), "deadline":result.as_ref().err().is_some_and(|e| e == PLAYBACK_DEADLINE_MESSAGE)}));
    result
}

async fn recover_inner(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    candidate: &Candidate,
    alternate_format: bool,
    cancel: watch::Receiver<bool>,
    trace: SourceTrace,
    deadline: PlaybackDeadline,
    strict_recording_verification: bool,
) -> Result<ResolvedSource, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let settings = store.settings()?;
    let mut candidate = candidate.clone();
    let audio = acquisition
        .extract_playback_audio_fresh_or_prepared_traced(
            &candidate,
            &settings,
            cancel.clone(),
            alternate_format,
            trace.clone(),
            deadline,
            None,
        )
        .await?;
    if !fresh_candidate_audio_identity_matches(track, &candidate, &audio) {
        return Err(
            "The refreshed audio source did not resolve to the same provider recording".into(),
        );
    }
    let pinned = !strict_recording_verification
        || source_resolution_is_pinned(track, false, &settings, true);
    let mut aliases = if pinned {
        crate::artist_credit_aliases::VerifiedPlaybackArtistAliases::default()
    } else {
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
    };
    include_candidate_aliases(&mut aliases, track, &candidate, Some(&audio));
    let requires_identity_check = candidate.provider == "bandcamp" || !pinned;
    if verify_resolved_identity_with_aliases_and_proofs(
        track,
        aliases.tracks(),
        aliases.isrc_enriched(),
        aliases.verified_recording_isrc(),
        &mut candidate,
        &audio,
        requires_identity_check,
    )
    .is_none()
    {
        return Err("The refreshed audio source identity does not match this recording".into());
    }
    let audio = acquisition
        .validate_playback_audio_traced(&candidate, audio, cancel, trace, deadline)
        .await?;
    let duration = audio
        .duration_seconds
        .or(candidate.duration_seconds)
        .or(track.duration_seconds)
        .filter(|value| value.is_finite() && *value >= 0.);
    let quality = preview_audio_quality(&audio);
    Ok(ResolvedSource {
        source: AudioSource {
            input: audio.url,
            ffmpeg: crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"),
            headers: audio.headers,
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some(audio.mime_type.clone()),
                codec: audio.codec.clone(),
                duration_seconds: audio.duration_seconds,
                content_length: audio.expected_content_length,
                has_video: audio.selected_format_has_video,
            }),
        },
        candidate: Some(candidate.clone()),
        quality,
        station: None,
        duration,
    })
}

/// Enrich a resolved candidate with chapter metadata without delaying audio startup.
/// Call this in a background task and apply the result only if the queue item is current.
pub async fn enrich_candidate_chapters(
    track: &Track,
    mut candidate: Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Option<Candidate> {
    if candidate.chapters.is_some() {
        return Some(candidate);
    }
    if *cancel.borrow() {
        return None;
    }
    let chapters = tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(20)) => return None,
        _ = wait_for_cancellation(cancel.clone()) => return None,
        result = acquisition::inspect_chapters(track, &candidate, settings) => result.ok()?,
    };
    if *cancel.borrow() {
        return None;
    }
    candidate.chapters = Some(chapters);
    Some(candidate)
}

fn linked_candidate(track: &Track) -> Option<Candidate> {
    linked_candidate_with_bandcamp(track, crate::bandcamp::playback_enabled())
}

fn linked_candidate_with_bandcamp(track: &Track, bandcamp_enabled: bool) -> Option<Candidate> {
    let raw = track
        .playback_range
        .as_ref()
        .map(|range| range.source_url.as_str())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or(&track.source_url);

    // The source URL itself is the explicit Bandcamp selection. Imports may
    // retain `provider = import`, so do not require that catalog label here.
    if let Some(url) = bandcamp_enabled
        .then(|| crate::bandcamp::validate_track_page_url(raw).ok())
        .flatten()
    {
        return Some(Candidate {
            id: String::new(),
            provider: "bandcamp".into(),
            url: url.to_string(),
            title: track.title.clone(),
            uploader: Some(track.artist.clone()),
            duration_seconds: track.duration_seconds,
            artwork_url: track.artwork_url.clone(),
            permission: "unknown".into(),
            confidence: "review".into(),
            ..Candidate::default()
        });
    }

    // A catalog search selection is pinned just like an explicit video link.
    // Derive its identity from the validated canonical URL, never an import hash
    // or a stale provenance ID. Container/access checks still run on resolution.
    if crate::monochrome::validate_url(raw).is_ok() {
        let url = url::Url::parse(raw).ok()?;
        let id = format!("monochrome:{}", url.path().strip_prefix("/track/")?);
        if let Some(candidate) = track
            .provenance
            .get("audioCandidate")
            .and_then(|value| serde_json::from_value::<Candidate>(value.clone()).ok())
            .filter(|candidate| {
                candidate.provider == "monochrome" && candidate.id == id && candidate.url == raw
            })
        {
            return Some(candidate);
        }
        return Some(Candidate {
            id,
            provider: "monochrome".into(),
            url: raw.to_owned(),
            title: track.title.clone(),
            uploader: Some(track.artist.clone()),
            duration_seconds: track.duration_seconds,
            artwork_url: track.artwork_url.clone(),
            permission: "unknown".into(),
            confidence: "review".into(),
            ..Candidate::default()
        });
    }

    if let Some(video_id) = acquisition::youtube_video_id(raw) {
        acquisition::validate_source_url(raw, "youtube").ok()?;
        return Some(Candidate {
            id: format!("youtube:{video_id}"),
            artwork_url: track
                .artwork_url
                .clone()
                .or_else(|| Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"))),
            provider: "youtube".into(),
            url: raw.to_owned(),
            title: track.title.clone(),
            duration_seconds: track.duration_seconds,
            permission: "unknown".into(),
            confidence: "review".into(),
            ..Candidate::default()
        });
    }

    let url = acquisition::validate_source_url(raw, "soundcloud").ok()?;
    let host = url.host_str()?;
    if host != "soundcloud.com" && !host.ends_with(".soundcloud.com") {
        return None;
    }
    let parts = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 2
        || parts[1] == "sets"
        || matches!(
            parts[0],
            "discover" | "search" | "charts" | "you" | "stream"
        )
    {
        return None;
    }

    // Source imports sometimes put a catalog hash in provider_id. For SoundCloud, the
    // canonical page is the safe identity until inspection supplies a real provider ID.
    let mut canonical = url;
    canonical.set_query(None);
    canonical.set_fragment(None);
    Some(Candidate {
        id: String::new(),
        provider: "soundcloud".into(),
        url: canonical.to_string(),
        title: track.title.clone(),
        duration_seconds: track.duration_seconds,
        permission: "unknown".into(),
        confidence: "review".into(),
        ..Candidate::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_resolution_distinguishes_no_rows_from_rejected_identity_and_keeps_failures() {
        assert_eq!(
            SerialResolutionFailures::default().finish(false),
            SOURCE_NO_RESULTS
        );
        let found = || SerialResolutionFailures {
            had_results: true,
            ..Default::default()
        };
        assert_eq!(found().finish(false), SOURCE_IDENTITY_REJECTED);
        assert_eq!(found().finish(true), SOURCE_MISSING_DURATION);
        let mut source = found();
        source.record("HTTP 403: source access denied".into());
        assert_eq!(source.finish(false), "HTTP 403: source access denied");
        let mut identity = found();
        identity.record("The resolved audio source identity does not match this recording".into());
        assert_eq!(
            identity.finish(false),
            "The resolved audio source identity does not match this recording"
        );
        let mut failed_search = SerialResolutionFailures::default();
        failed_search.record("Network connection unavailable".into());
        assert_eq!(
            failed_search.finish(false),
            "Network connection unavailable"
        );
        // Legacy interpolated recording names are categorized before words
        // inside arbitrary metadata can imply a cancel/deadline/network error.
        assert_eq!(
            ResolutionFailure::new(
                "No playable audio match was found for Network — Cancel Deadline".into()
            )
            .kind,
            ResolutionFailureKind::NoResults
        );
    }

    #[tokio::test]
    async fn parallel_provider_failures_keep_evidence_in_both_completion_orders() {
        for early_preferred in [true, false] {
            for (preferred_error, fallback_error, expected) in [
                (
                    "HTTP 403: source access denied",
                    SOURCE_NO_RESULTS,
                    "HTTP 403: source access denied",
                ),
                (
                    SOURCE_NO_RESULTS,
                    "HTTP 403: source access denied",
                    "HTTP 403: source access denied",
                ),
                (
                    "Network connection unavailable",
                    SOURCE_NO_RESULTS,
                    "Network connection unavailable",
                ),
                (
                    SOURCE_IDENTITY_REJECTED,
                    SOURCE_NO_RESULTS,
                    SOURCE_IDENTITY_REJECTED,
                ),
                (
                    "HTTP 403: source access denied",
                    "HTTP 404: source removed",
                    "HTTP 403: source access denied",
                ),
                (
                    PLAYBACK_DEADLINE_MESSAGE,
                    SOURCE_NO_RESULTS,
                    PLAYBACK_DEADLINE_MESSAGE,
                ),
                ("Cancelled", PLAYBACK_DEADLINE_MESSAGE, "Cancelled"),
            ] {
                let preferred = async move {
                    tokio::time::sleep(Duration::from_millis(if early_preferred { 1 } else { 4 }))
                        .await;
                    Err::<u8, _>(preferred_error.to_owned())
                };
                let fallback = async move {
                    tokio::time::sleep(Duration::from_millis(if early_preferred { 4 } else { 1 }))
                        .await;
                    Err::<u8, _>(fallback_error.to_owned())
                };
                let result = prefer_with_grace(
                    Box::pin(preferred),
                    Box::pin(fallback),
                    Duration::from_millis(10),
                )
                .await;
                assert_eq!(
                    result.unwrap_err(),
                    expected,
                    "preferred finished first: {early_preferred}"
                );
            }
        }
        let third_provider = merge_resolution_errors(
            merge_resolution_errors(
                "HTTP 403: source access denied".into(),
                SOURCE_NO_RESULTS.into(),
            ),
            SOURCE_NO_RESULTS.into(),
        );
        assert_eq!(third_provider, "HTTP 403: source access denied");
    }

    #[tokio::test]
    async fn available_source_still_wins_over_other_provider_failure() {
        for preferred_success in [true, false] {
            let preferred = async move {
                if preferred_success {
                    Ok(17u8)
                } else {
                    Err("Network connection unavailable".to_owned())
                }
            };
            let fallback = async move {
                if preferred_success {
                    Err(SOURCE_NO_RESULTS.to_owned())
                } else {
                    Ok(17u8)
                }
            };
            assert_eq!(
                prefer_with_grace(
                    Box::pin(preferred),
                    Box::pin(fallback),
                    Duration::from_millis(10)
                )
                .await
                .unwrap(),
                17
            );
        }
    }

    #[test]
    fn rank_all_collected_recordings_before_spending_two_foreground_extractions() {
        let track = Track {
            title: "Matthew".into(),
            artist: "John Denver".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        let make = |id: &str, title: &str, duration: f64| Candidate {
            provider: "youtube".into(),
            id: format!("youtube:{id}"),
            title: title.into(),
            duration_seconds: Some(duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let mut found = vec![
            make("FRP_lYROd-I", "John Denver - Matthew", 216.0),
            make("second00000", "John Denver - Matthew", 216.0),
        ];
        found.extend(
            (0..6).map(|index| make(&format!("wrong{index:06}"), "Other Artist - Matthew", 222.0)),
        );
        found.push(make("ps9X-DXUFGY", "JOHN DENVER - MATTHEW.", 225.0));
        found.push(make("another0000", "John Denver - Matthew", 224.0));
        let mut evaluated = Vec::new();
        let mut ranked = found
            .into_iter()
            .filter_map(|candidate| {
                let path = evaluate_candidate_gate_with_proofs(&track, &[], &[], None, &candidate);
                evaluated.push(path);
                path.accepted().then_some((candidate, path))
            })
            .collect::<Vec<_>>();
        assert_eq!(evaluated.len(), 10);
        assert_eq!(evaluated[0], CandidateGatePath::ProvisionalCanonical);
        assert_eq!(evaluated[8], CandidateGatePath::RecordingCanonical);
        ranked.sort_by_key(|(_, path)| path.evidence_priority());
        let first = eligible_after_webview_failure(
            ranked.into_iter().map(|(candidate, _)| candidate),
            &[],
            None,
            2,
        );
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].id, "youtube:ps9X-DXUFGY");
        assert_eq!(first[1].id, "youtube:another0000");
        // Equally strong identities preserve provider order, rather than
        // reordering on incidental sub-second duration differences.
        assert_eq!(first[0].duration_seconds, Some(225.0));
        assert_eq!(first[1].duration_seconds, Some(224.0));
    }

    #[test]
    fn exact_music_catalog_locator_precedes_weaker_duration_provisionals() {
        let track = Track {
            title: "Matthew".into(),
            artist: "John Denver".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        let catalog = Candidate {
            provider: "youtube".into(),
            id: "youtube:uZGRATckoBM".into(),
            title: "Matthew".into(),
            duration_seconds: Some(228.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "playbackMusicCatalogCandidate":true,
                "catalogTitle":"Matthew",
                "catalogArtist":"John Denver"
            })),
            ..Candidate::default()
        };
        let weak = Candidate {
            provider: "youtube".into(),
            id: "youtube:FRP_lYROd-I".into(),
            title: "John Denver - Matthew".into(),
            duration_seconds: Some(216.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let mut ranked = [weak.clone(), catalog.clone()]
            .into_iter()
            .filter_map(|candidate| {
                let path = evaluate_candidate_gate_with_proofs(&track, &[], &[], None, &candidate);
                path.accepted().then_some((candidate, path))
            })
            .collect::<Vec<_>>();
        assert_eq!(ranked[0].1, CandidateGatePath::ProvisionalCanonical);
        assert_eq!(ranked[1].1, CandidateGatePath::MusicCatalogLocator);
        ranked.sort_by_key(|(_, path)| path.evidence_priority());
        let selected = eligible_after_webview_failure(
            ranked.into_iter().map(|(candidate, _)| candidate),
            &[],
            None,
            1,
        );
        assert_eq!(selected[0].id, catalog.id);
    }

    #[test]
    fn alias_research_requires_remaining_foreground_work() {
        let pass = AliasResolutionPass::default();
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:example".into(),
            ..Candidate::default()
        };
        pass.remember(&candidate);
        assert!(pass.has_remaining_alias_attempt(2));
        pass.attempted("youtube");
        assert!(!pass.has_remaining_alias_attempt(1));
        assert!(pass.has_remaining_alias_attempt(2));
        pass.attempted("youtube");
        assert!(!pass.has_remaining_alias_attempt(2));
        pass.spare_queries
            .lock()
            .unwrap()
            .insert("soundcloud".into(), true);
        assert!(pass.has_remaining_alias_attempt(2));
        assert_eq!(pass.used("youtube"), 2);
        assert_eq!(pass.used("soundcloud"), 0);
    }
    #[test]
    fn alias_research_does_not_use_inaccessible_sources_as_artist_evidence() {
        let track = Track {
            artist: "Requested Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        let inaccessible = Candidate {
            provider: "youtube".into(),
            title: "Different Artist - Song".into(),
            uploader: Some("Different Artist".into()),
            permission: "restricted".into(),
            duration_seconds: Some(180.0),
            ..Candidate::default()
        };
        assert!(!plausible_artist_alias_block(&track, &inaccessible));
    }

    #[test]
    fn fallback_candidate_attempt_limit_is_bounded_to_one_or_default_two() {
        assert_eq!(validate_candidate_attempt_limit(None), Ok(2));
        assert_eq!(validate_candidate_attempt_limit(Some(1)), Ok(1));
        assert_eq!(validate_candidate_attempt_limit(Some(2)), Ok(2));
        assert!(validate_candidate_attempt_limit(Some(0)).is_err());
        assert!(validate_candidate_attempt_limit(Some(3)).is_err());
    }

    #[test]
    fn one_attempt_fallback_budget_is_global_across_provider_mappings() {
        assert_eq!(remaining_candidate_attempts(1, 0, true, 0), 1);
        assert_eq!(remaining_candidate_attempts(1, 1, true, 0), 0);
        assert_eq!(remaining_candidate_attempts(1, 1, true, 1), 0);
        // The ordinary resolver retains its existing per-provider two-candidate cap.
        assert_eq!(remaining_candidate_attempts(2, 1, false, 1), 1);
    }

    #[test]
    fn fresh_candidate_identity_uses_canonical_monochrome_url_or_extractor_identity() {
        let url = "https://tracks.monochrome.st/track/239910648984977408";
        let track = Track {
            title: "Saved Recording".into(),
            artist: "Visible Artist".into(),
            ..Track::default()
        };
        let mono = Candidate {
            provider: "monochrome".into(),
            id: "monochrome:239910648984977408".into(),
            url: url.into(),
            ..Candidate::default()
        };
        let mono_audio = acquisition::PreviewAudio {
            url: url.into(),
            mime_type: "audio/flac".into(),
            codec: Some("flac".into()),
            bitrate_kbps: None,
            duration_seconds: Some(181.4),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: None,
            chapters: Some(vec![]),
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        assert!(fresh_candidate_audio_identity_matches(
            &track,
            &mono,
            &mono_audio
        ));
        let mut wrong_mono_audio = mono_audio.clone();
        wrong_mono_audio.url = "https://tracks.monochrome.st/track/240966681495732224".into();
        assert!(!fresh_candidate_audio_identity_matches(
            &track,
            &mono,
            &wrong_mono_audio
        ));
        let mut noncanonical_mono = mono.clone();
        noncanonical_mono.url.push_str("?source=other");
        let noncanonical_audio = acquisition::PreviewAudio {
            url: noncanonical_mono.url.clone(),
            ..mono_audio.clone()
        };
        assert!(!fresh_candidate_audio_identity_matches(
            &track,
            &noncanonical_mono,
            &noncanonical_audio
        ));

        let youtube = Candidate {
            provider: "youtube".into(),
            id: "youtube:abc123".into(),
            ..Candidate::default()
        };
        let yt_audio = acquisition::PreviewAudio {
            resolved_provider_id: Some("abc123".into()),
            resolved_title: Some("Saved Recording".into()),
            ..mono_audio
        };
        assert!(fresh_candidate_audio_identity_matches(
            &track, &youtube, &yt_audio
        ));
        let wrong_youtube = acquisition::PreviewAudio {
            resolved_provider_id: Some("other-id".into()),
            ..yt_audio.clone()
        };
        assert!(!fresh_candidate_audio_identity_matches(
            &track,
            &youtube,
            &wrong_youtube
        ));
        let titleless_youtube = acquisition::PreviewAudio {
            resolved_title: None,
            ..yt_audio
        };
        assert!(!fresh_candidate_audio_identity_matches(
            &track,
            &youtube,
            &titleless_youtube
        ));
    }

    #[test]
    fn candidate_gate_filter_stays_lazy_at_foreground_cap_when_tracing_is_disabled() {
        let mut trace = SourceTrace::new(None, None, None);
        trace.enabled = false;
        let mut evidence = CandidateGateEvidence::new_if_trace_enabled(&trace);
        let mut evaluated = 0usize;
        let candidates = (0..8).map(|index| Candidate {
            id: format!("candidate:{index}"),
            score: if index == 1 { 0.0 } else { 1.0 },
            ..Candidate::default()
        });
        let filtered = candidates.into_iter().filter(|candidate| {
            evaluated += 1;
            let path = if candidate.score > 0.0 {
                CandidateGatePath::RecordingCanonical
            } else {
                CandidateGatePath::Rejected
            };
            if let Some(evidence) = evidence.as_mut() {
                evidence.increment(path.counter());
            }
            path.accepted()
        });
        let accepted = eligible_after_webview_failure(filtered, &[], None, 2);
        assert_eq!(accepted.len(), 2);
        assert_eq!(evaluated, 3);
        assert!(evidence.is_none());
    }

    #[test]
    fn candidate_gate_diagnostics_are_not_allocated_when_trace_is_disabled() {
        let mut disabled = SourceTrace::new(None, None, None);
        disabled.enabled = false;
        assert!(!disabled.is_enabled());
        assert!(CandidateGateEvidence::new_if_trace_enabled(&disabled).is_none());

        let sink_trace = SourceTrace::new(None, None, None).with_test_sink(Arc::new(|_| {}));
        assert!(sink_trace.is_enabled());
        assert!(CandidateGateEvidence::new_if_trace_enabled(&sink_trace).is_some());
    }

    #[test]
    fn diagnostic_gate_evidence_preserves_existing_alias_decision_and_redacts_values() {
        let track = Track {
            artist: "Le Mystère Des Voix Bulgares".into(),
            title: "Vgradena Nevesta (Built in Bride)".into(),
            duration_seconds: Some(319.0),
            ..Track::default()
        };
        let alias = Track {
            artist: "Bulgarian State Radio & Television Female Vocal Choir".into(),
            ..track.clone()
        };
        let candidate = Candidate {
            id: "monochrome:12345678901234567".into(),
            provider: "monochrome".into(),
            url: "https://tracks.monochrome.st/track/12345678901234567".into(),
            title: "Bulgarian State Radio & Television Female Vocal Choir - Vgradena Nevesta"
                .into(),
            uploader: Some("Bulgarian State Radio & Television Female Vocal Choir".into()),
            duration_seconds: Some(319.0),
            permission: "unknown".into(),
            confidence: "review".into(),
            source_metadata: Some(json!({"catalogTitle":"Vgradena Nevesta"})),
            ..Candidate::default()
        };
        let old_decision = playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &candidate,
        ) || topic_playback_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &candidate,
        ) || provisional_playback_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &candidate,
        );
        let path = evaluate_candidate_gate(&track, std::slice::from_ref(&alias), &candidate);
        assert_eq!(path.accepted(), old_decision);
        assert_eq!(path, CandidateGatePath::Rejected);

        let mut evidence = CandidateGateEvidence::default();
        evidence.observe(&track, std::slice::from_ref(&alias), &candidate, path);
        assert_eq!(evidence.counts.get("rejectedAllGates"), Some(&1));
        assert_eq!(
            evidence.counts.get("uploaderMatchesVerifiedAliasCredit"),
            Some(&1)
        );
        assert_eq!(evidence.counts.get("durationDeltaLe5s"), Some(&1));
        assert_eq!(evidence.counts.get("monochromeCatalogTitleExact"), None);
        assert_eq!(
            evidence
                .counts
                .get("requestedTitlePhrasePresentInCandidateTitle"),
            None
        );
        assert_eq!(evidence.candidate_rows.len(), 1);
        let row = &evidence.candidate_rows[0];
        assert_eq!(row["gate"], "rejected");
        assert_eq!(row["accepted"], false);
        assert_eq!(row["requestedTitlePhrasePresent"], false);
        // The requested title includes the parenthetical subtitle; the catalog title does not.
        assert_eq!(row["catalogTitleExact"], false);
        assert_eq!(row["verifiedAliasArtistPhrasePresent"], true);
        assert_eq!(row["recordingCanonicalMatch"], false);
        assert_eq!(row["recordingVerifiedAliasMatch"], false);
        assert_eq!(row["topicCanonicalMatch"], false);
        assert_eq!(row["topicVerifiedAliasMatch"], false);
        assert_eq!(row["provisionalCanonicalMatch"], false);
        assert_eq!(row["provisionalVerifiedAliasMatch"], false);
        assert_eq!(row["descriptiveDurationWithinFiveSeconds"], true);
        assert_eq!(row["confidenceClass"], "review");
        let serialized = serde_json::to_string(
            &json!({"counts":evidence.counts,"rows":evidence.candidate_rows}),
        )
        .unwrap();
        assert!(!serialized.contains("Vgradena"));
        assert!(!serialized.contains("Bulgarian State"));
        assert!(!serialized.contains("tracks.monochrome"));
        assert!(!serialized.contains("12345678901234567"));

        let conflict = Candidate {
            reasons: vec!["RECORDING VERSION CONFLICTS WITH CATALOG".into()],
            ..candidate
        };
        let mut conflict_evidence = CandidateGateEvidence::default();
        conflict_evidence.observe(
            &track,
            std::slice::from_ref(&alias),
            &conflict,
            CandidateGatePath::Rejected,
        );
        assert_eq!(
            conflict_evidence
                .counts
                .get("versionConflictReasonFlagAsciiCaseInsensitive"),
            Some(&1)
        );
    }

    #[test]
    fn candidate_gate_rows_are_bounded_and_keep_predicates_on_each_row() {
        let track = Track {
            artist: "requested artist".into(),
            title: "requested title private-token".into(),
            duration_seconds: Some(100.0),
            ..Track::default()
        };
        let mut evidence = CandidateGateEvidence::default();
        for index in 0..(MAX_CANDIDATE_GATE_ROWS + 5) {
            let candidate = Candidate {
                provider: "youtube".into(),
                confidence: if index == 0 { "high" } else { "low" }.into(),
                permission: "unknown".into(),
                title: if index == 0 {
                    "requested artist requested title private-token"
                } else {
                    "unrelated result"
                }
                .into(),
                duration_seconds: Some(if index == 0 { 100.0 } else { 500.0 }),
                ..Candidate::default()
            };
            let path = evaluate_candidate_gate(&track, &[], &candidate);
            evidence.observe(&track, &[], &candidate, path);
        }
        assert_eq!(
            evidence.counts["candidatesEvaluatedBeforeForegroundCap"],
            (MAX_CANDIDATE_GATE_ROWS + 5) as u64
        );
        assert_eq!(evidence.candidate_rows.len(), MAX_CANDIDATE_GATE_ROWS);
        for chunk in evidence
            .candidate_rows
            .chunks(MAX_CANDIDATE_GATE_ROWS_PER_EVENT)
        {
            let detail = json!({"rows":chunk,"candidateRowsLimit":MAX_CANDIDATE_GATE_ROWS,
                "candidateRowsTruncated":true,"firstOrdinal":1,"totalCandidates":MAX_CANDIDATE_GATE_ROWS+5});
            let trace =
                SourceTrace::new(Some("o".repeat(80)), Some("s".repeat(80)), Some(u64::MAX))
                    .with_shared_lookup(uuid::Uuid::new_v4());
            assert!(
                serde_json::to_vec(&trace.payload(
                    "candidate_gate_evidence_rows",
                    "soundcloud",
                    detail
                ))
                .unwrap()
                .len()
                    < 8192
            );
        }
        let first = &evidence.candidate_rows[0];
        assert_eq!(first["accepted"], true);
        assert_eq!(first["gate"], "recording_canonical");
        assert_eq!(first["recordingCanonicalMatch"], true);
        assert_eq!(first["confidenceClass"], "high");
        assert_eq!(first["descriptiveDurationWithinFiveSeconds"], true);
        let second = &evidence.candidate_rows[1];
        assert_eq!(second["accepted"], false);
        assert_eq!(second["confidenceClass"], "low");
        assert_eq!(second["descriptiveDurationWithinFiveSeconds"], false);
        let serialized = serde_json::to_string(&evidence.candidate_rows).unwrap();
        assert!(!serialized.contains("requested artist"));
        assert!(!serialized.contains("requested title"));
        assert!(!serialized.contains("private-token"));
        assert!(!serialized.contains("unrelated result"));
    }

    #[test]
    fn cached_artist_aliases_preserve_ordinary_and_topic_gates() {
        let track = Track {
            artist: "Le Mystère Des Voix Bulgares".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        let alias = Track {
            artist: "Bulgarian State Radio & Television Female Vocal Choir".into(),
            ..track.clone()
        };
        let candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Bulgarian State Radio & Television Female Vocal Choir - Song".into(),
            duration_seconds: Some(200.0),
            permission: "public".into(),
            confidence: "review".into(),
            ..Candidate::default()
        };
        assert!(!crate::source_fallback::playback_recording_matches(
            &track, &candidate
        ));
        assert!(playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &candidate
        ));
        assert_eq!(
            evaluate_candidate_gate(&track, std::slice::from_ref(&alias), &candidate),
            CandidateGatePath::RecordingVerifiedAlias
        );
        let wrong_duration = Candidate {
            duration_seconds: Some(220.0),
            ..candidate.clone()
        };
        assert!(!playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &wrong_duration
        ));
        let wrong_version_track = Track {
            version: Some("Live".into()),
            ..track.clone()
        };
        assert!(!playback_recording_matches_with_aliases(
            &wrong_version_track,
            std::slice::from_ref(&alias),
            &candidate
        ));
        let unverified_spotify_credit = Track {
            artist: "Bulgarian State Television Female Choir".into(),
            ..track.clone()
        };
        assert!(!playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&unverified_spotify_credit),
            &candidate
        ));
    }

    #[test]
    fn verified_artist_alias_uses_full_monochrome_catalog_credit() {
        let track = Track {
            artist: "Original Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        let alias = Track {
            artist: "Artist feat. Guest".into(),
            ..track.clone()
        };
        let candidate = Candidate {
            id: "monochrome:155142501534011392".into(),
            provider: "monochrome".into(),
            title: "Artist, Guest - Song".into(),
            uploader: Some("Artist, Guest".into()),
            duration_seconds: Some(200.0),
            permission: "unknown".into(),
            confidence: "review".into(),
            source_metadata: Some(json!({
                "catalogTitle":"Song",
                "artistNames":["Artist", "Guest"]
            })),
            ..Candidate::default()
        };
        assert!(valid_artist_alias_variant(&track, &alias));
        assert!(!playback_recording_matches_with_aliases(
            &track,
            &[],
            &candidate
        ));
        assert!(playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &candidate
        ));
    }

    #[test]
    fn cached_crosswalk_proof_allows_only_its_artist_and_isrc_alias_in_the_resolver_gate() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("cached-crosswalk-gate.sqlite")).unwrap();
        let track_id = "bbbbbbbbbbbbbbbbbbbbbb";
        let spotify_a = "aaaaaaaaaaaaaaaaaaaaaa";
        let spotify_b = "cccccccccccccccccccccc";
        let recording_id = "00000000-0000-4000-8000-000000000001";
        let mb_a = "00000000-0000-4000-8000-000000000002";
        let mb_b = "00000000-0000-4000-8000-000000000003";
        let artists = json!([
            {"provider":"spotify","id":spotify_a,"name":"Postmodern Jukebox"},
            {"provider":"spotify","id":spotify_b,"name":"Haley Reinhart"}
        ]);
        let track = Track {
            id: format!("spotify:{track_id}"),
            provider: "spotify".into(),
            provider_id: track_id.into(),
            source_url: format!("https://open.spotify.com/track/{track_id}"),
            title: "Song".into(),
            artist: "Postmodern Jukebox, Haley Reinhart".into(),
            duration_seconds: Some(180.0),
            provenance: json!({
                "catalog":{"verified":true,"provider":"spotify","providerId":track_id,"artistCount":2,"artists":artists},
                "remoteCatalog":{"trackId":track_id,"title":"Song","artist":"Postmodern Jukebox, Haley Reinhart","durationSeconds":180.0,"artistCount":2,"artistRefs":artists}
            }),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let recording = json!({"id":recording_id,"title":"Song","length":180000,"isrcs":["USABC2600001"],"artist-credit":[
            {"name":"Scott Bradlee’s Postmodern Jukebox","joinphrase":" starring ","artist":{"id":mb_a,"name":"Postmodern Jukebox"}},
            {"name":"Haley Reinhart","joinphrase":"","artist":{"id":mb_b,"name":"Haley Reinhart"}}
        ]});
        let profile_a = json!({"id":mb_a,"name":"Postmodern Jukebox","aliases":[{"name":"Scott Bradlee’s Postmodern Jukebox"}],"relations":[{"url":{"resource":format!("https://open.spotify.com/artist/{spotify_a}")}}]});
        let profile_b = json!({"id":mb_b,"name":"Haley Reinhart","aliases":[],"relations":[{"url":{"resource":format!("https://open.spotify.com/artist/{spotify_b}")}}]});
        store
            .set_provider_cache(
                crate::artist_credit_aliases::spotify_recording_credit_raw_cache_namespace(),
                recording_id,
                &recording,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                crate::artist_credit_aliases::spotify_recording_credit_profile_cache_namespace(),
                mb_a,
                &profile_a,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                crate::artist_credit_aliases::spotify_recording_credit_profile_cache_namespace(),
                mb_b,
                &profile_b,
                3600,
            )
            .unwrap();
        crate::artist_credit_aliases::write_verified_spotify_artist_credit_recording(
            &store,
            &track,
            recording_id,
        )
        .unwrap();
        let aliases =
            crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, &track);
        assert!(aliases.tracks().iter().any(|alias| {
            alias.artist == "Scott Bradlee’s Postmodern Jukebox, Haley Reinhart"
        }));
        let candidate = Candidate {
            id: "monochrome:recording-1".into(),
            provider: "monochrome".into(),
            title: "Scott Bradlee’s Postmodern Jukebox, Haley Reinhart - Song".into(),
            duration_seconds: Some(180.0),
            permission: "unknown".into(),
            confidence: "review".into(),
            source_metadata: Some(
                json!({"catalogTitle":"Song","artistNames":["Scott Bradlee’s Postmodern Jukebox","Haley Reinhart"],"isrc":"USABC2600001"}),
            ),
            ..Candidate::default()
        };
        assert!(!crate::source_fallback::playback_recording_matches(
            &track, &candidate
        ));
        assert_eq!(
            evaluate_candidate_gate_with_proofs(&track, aliases.tracks(), &[], None, &candidate),
            CandidateGatePath::Rejected
        );
        assert_eq!(
            evaluate_candidate_gate_with_proofs(
                &track,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
                &candidate
            ),
            CandidateGatePath::RecordingVerifiedAlias
        );
        let mut bad_isrc = candidate;
        bad_isrc.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2600001");
        assert_eq!(
            evaluate_candidate_gate_with_proofs(
                &track,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
                &bad_isrc
            ),
            CandidateGatePath::Rejected
        );
        bad_isrc.source_metadata.as_mut().unwrap()["artistNames"] =
            json!(["Postmodern Jukebox", "Haley Reinhart"]);
        assert_eq!(
            evaluate_candidate_gate_with_proofs(
                &track,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
                &bad_isrc
            ),
            CandidateGatePath::Rejected
        );
        assert!(!playback_recording_matches_with_proofs(
            &track,
            &[],
            aliases.isrc_enriched(),
            aliases.verified_recording_isrc(),
            &bad_isrc
        ));
        let bad_audio = acquisition::PreviewAudio {
            url: "https://media.example/audio".into(),
            mime_type: "audio/mpeg".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(180.0),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: Some(json!({"isrc":"USZZZ2600001"})),
            resolved_title: Some("Song".into()),
            resolved_provider_id: Some("recording-1".into()),
            chapters: None,
            expected_content_length: None,
            provider_session: None,
            headers: Vec::new(),
        };
        let mut good_catalog_candidate = Candidate {
            source_metadata: Some(
                json!({"catalogTitle":"Song","artistNames":["Scott Bradlee’s Postmodern Jukebox","Haley Reinhart"],"isrc":"USABC2600001"}),
            ),
            ..bad_isrc
        };
        assert!(!accept_resolved_identity_with_aliases_and_proofs(
            &track,
            aliases.tracks(),
            aliases.isrc_enriched(),
            aliases.verified_recording_isrc(),
            &mut good_catalog_candidate,
            &bad_audio,
            true,
        ));

        // If the saved row already has the verified ISRC, the cache still
        // carries the recording-level contradiction guard even though no
        // ISRC-enriched alias variant is necessary.
        let mut existing_isrc_track = track.clone();
        existing_isrc_track.isrc = Some("USABC2600001".into());
        existing_isrc_track.provenance["remoteCatalog"]["isrc"] = json!("USABC2600001");
        store.save_track(&existing_isrc_track).unwrap();
        crate::artist_credit_aliases::write_verified_spotify_artist_credit_recording(
            &store,
            &existing_isrc_track,
            recording_id,
        )
        .unwrap();
        let existing_isrc_aliases =
            crate::artist_credit_aliases::cached_verified_playback_artist_aliases(
                &store,
                &existing_isrc_track,
            );
        let canonical_bad_isrc = Candidate {
            provider: "monochrome".into(),
            title: "Postmodern Jukebox, Haley Reinhart - Song".into(),
            duration_seconds: Some(180.0),
            permission: "unknown".into(),
            confidence: "review".into(),
            source_metadata: Some(
                json!({"catalogTitle":"Song","artistNames":["Postmodern Jukebox","Haley Reinhart"],"isrc":"USZZZ2600001"}),
            ),
            ..Candidate::default()
        };
        assert_eq!(
            existing_isrc_aliases.verified_recording_isrc(),
            Some("USABC2600001")
        );
        assert_eq!(
            evaluate_candidate_gate_with_proofs(
                &existing_isrc_track,
                existing_isrc_aliases.tracks(),
                existing_isrc_aliases.isrc_enriched(),
                existing_isrc_aliases.verified_recording_isrc(),
                &canonical_bad_isrc,
            ),
            CandidateGatePath::Rejected
        );
    }

    #[test]
    fn exact_aliases_apply_to_topic_metadata_only_with_all_existing_identity_guards() {
        let track = Track {
            artist: "Le Mystère Des Voix Bulgares".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        let alias = Track {
            artist: "Bulgarian State Radio & Television Female Vocal Choir".into(),
            ..track.clone()
        };
        let metadata = json!({"provider":"youtube","channel":"Bulgarian State Radio & Television Female Vocal Choir - Topic","channelId":"UC1234567890123456789012","artist":"Bulgarian State Radio & Television Female Vocal Choir","artists":["Bulgarian State Radio & Television Female Vocal Choir"],"track":"Song","descriptionCreditLine":"Bulgarian State Radio & Television Female Vocal Choir - Song","duration":200.0});
        let topic = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Song".into(),
            uploader: Some("Bulgarian State Radio & Television Female Vocal Choir - Topic".into()),
            duration_seconds: Some(200.0),
            permission: "public".into(),
            confidence: "review".into(),
            source_metadata: Some(metadata.clone()),
            ..Candidate::default()
        };
        assert!(topic_playback_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &topic
        ));
        // The old acceptance expression checks recording matches first, and
        // that predicate internally includes the guarded Topic-original path.
        assert!(playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &topic
        ));
        assert_eq!(
            evaluate_candidate_gate(&track, std::slice::from_ref(&alias), &topic),
            CandidateGatePath::RecordingVerifiedAlias
        );
        assert!(!crate::source_fallback::playback_recording_matches(
            &track, &topic
        ));
        let mut bad_metadata = metadata.clone();
        bad_metadata["channelId"] = json!("not-a-channel-id");
        let bad_topic = Candidate {
            source_metadata: Some(bad_metadata),
            ..topic.clone()
        };
        assert!(!playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &bad_topic
        ));
        let mut bad_credit = metadata;
        bad_credit["artist"] = json!("Bulgarian State Television Female Choir");
        bad_credit["artists"] = json!(["Bulgarian State Television Female Choir"]);
        bad_credit["descriptionCreditLine"] =
            json!("Bulgarian State Television Female Choir - Song");
        let unverified = Candidate {
            source_metadata: Some(bad_credit),
            ..topic
        };
        assert!(!playback_recording_matches_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &unverified
        ));
    }

    #[test]
    fn candidate_gate_evaluator_preserves_provisional_path_and_rejection() {
        let track = Track {
            artist: "Artist".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        let provisional = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Живой исполнитель Artist - Song".into(),
            duration_seconds: Some(206.0),
            permission: "public".into(),
            confidence: "review".into(),
            ..Candidate::default()
        };
        let old_decision = playback_recording_matches_with_aliases(&track, &[], &provisional)
            || topic_playback_matches_with_aliases(&track, &[], &provisional)
            || provisional_playback_matches_with_aliases(&track, &[], &provisional);
        let path = evaluate_candidate_gate(&track, &[], &provisional);
        assert_eq!(path, CandidateGatePath::ProvisionalCanonical);
        assert_eq!(path.accepted(), old_decision);

        let rejected = Candidate {
            confidence: "low".into(),
            ..provisional.clone()
        };
        let old_rejected = playback_recording_matches_with_aliases(&track, &[], &rejected)
            || topic_playback_matches_with_aliases(&track, &[], &rejected)
            || provisional_playback_matches_with_aliases(&track, &[], &rejected);
        let rejected_path = evaluate_candidate_gate(&track, &[], &rejected);
        assert_eq!(rejected_path, CandidateGatePath::Rejected);
        assert_eq!(rejected_path.accepted(), old_rejected);
    }

    #[test]
    fn alias_extractor_acceptance_keeps_provider_id_title_version_and_duration_gates() {
        let track = Track {
            artist: "Le Mystère Des Voix Bulgares".into(),
            title: "Song".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        let alias = Track {
            artist: "Bulgarian State Radio & Television Female Vocal Choir".into(),
            ..track.clone()
        };
        let candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Bulgarian State Radio & Television Female Vocal Choir - Song".into(),
            duration_seconds: Some(200.0),
            permission: "public".into(),
            confidence: "review".into(),
            ..Candidate::default()
        };
        let audio = acquisition::PreviewAudio {
            url: "https://example.invalid/audio".into(),
            mime_type: "audio/webm".into(),
            codec: Some("opus".into()),
            bitrate_kbps: None,
            duration_seconds: Some(200.0),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: Some(
                "Bulgarian State Radio & Television Female Vocal Choir - Song".into(),
            ),
            resolved_provider_id: Some("abcdefghijk".into()),
            chapters: Some(vec![]),
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        let mut accepted = candidate.clone();
        assert!(accept_resolved_identity_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &mut accepted,
            &audio,
            true
        ));
        assert_eq!(accepted.id, "youtube:abcdefghijk");
        let bad_audio = acquisition::PreviewAudio {
            duration_seconds: Some(220.0),
            ..audio.clone()
        };
        let mut rejected = candidate.clone();
        assert!(!accept_resolved_identity_with_aliases(
            &track,
            std::slice::from_ref(&alias),
            &mut rejected,
            &bad_audio,
            true
        ));
        let mut wrong_version = track.clone();
        wrong_version.version = Some("Live".into());
        let mut rejected = candidate;
        assert!(!accept_resolved_identity_with_aliases(
            &wrong_version,
            std::slice::from_ref(&alias),
            &mut rejected,
            &audio,
            true
        ));
    }

    #[tokio::test]
    async fn expired_shared_deadline_normalizes_nested_timeout_for_trace_classification() {
        let deadline = PlaybackDeadline::at(tokio::time::Instant::now());
        // This is the timeout emitted by a nested stream-access operation when
        // its cap is the shared deadline and its future wins the ready race.
        let nested = Err::<(), _>("Audio stream resolution timed out".to_string());
        let normalized = normalize_resolver_deadline(nested, deadline, false);
        assert_eq!(normalized.as_ref().unwrap_err(), PLAYBACK_DEADLINE_MESSAGE);
        assert!(is_resolver_deadline_error(&normalized));

        let cancelled = normalize_resolver_deadline(
            Err::<(), _>("Audio stream resolution timed out".to_string()),
            deadline,
            true,
        );
        assert_eq!(cancelled.as_ref().unwrap_err(), "Cancelled");
        assert!(!is_resolver_deadline_error(&cancelled));
    }

    #[test]
    fn resolution_trace_normal_finish_is_once_only_for_all_outcomes() {
        for (result, cancelled, deadline) in [
            (Ok(()), false, false),
            (Err("fixture failure".to_string()), false, false),
            (Err("Cancelled".to_string()), true, false),
            (Err(PLAYBACK_DEADLINE_MESSAGE.to_string()), false, true),
        ] {
            let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
            let output = events.clone();
            let trace = SourceTrace::new(Some("operation".into()), None, Some(21))
                .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
            let (_stop, cancel) = watch::channel(false);
            let guard = ResolutionTraceGuard::begin(
                trace,
                "soundcloud",
                false,
                cancel,
                PlaybackDeadline::after(Duration::from_secs(1)),
            );
            guard.finish(&result);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(events[1]["phase"], "resolution_end");
            assert_eq!(events[1]["resolutionId"], events[0]["resolutionId"]);
            assert_eq!(events[1]["details"]["ok"], result.is_ok());
            assert_eq!(events[1]["details"]["cancelled"], cancelled);
            assert_eq!(events[1]["details"]["deadline"], deadline);
        }
    }

    #[test]
    fn unrelated_resolution_future_drop_is_explicitly_abandoned() {
        let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let output = events.clone();
        let trace = SourceTrace::new(None, None, Some(22))
            .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        let (_stop, cancel) = watch::channel(false);
        drop(ResolutionTraceGuard::begin(
            trace,
            "other",
            false,
            cancel,
            PlaybackDeadline::after(Duration::from_secs(1)),
        ));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1]["details"],
            json!({
                "ok":false,"cancelled":false,"deadline":false,"abandoned":true,
            })
        );
    }

    #[test]
    fn correlated_provider_spans_end_on_cancellation_or_outer_deadline_drop() {
        let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let output = events.clone();
        let trace = SourceTrace::new(Some("native-fruit-start".into()), None, None)
            .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        let lookup_id = uuid::Uuid::new_v4();

        TraceSpan::begin(
            trace.clone().with_shared_lookup(lookup_id),
            "monochrome",
            "monochrome_probe_start",
            "monochrome_probe_end",
        )
        .finish("cancelled");

        let deadline_trace = trace.clone().with_shared_lookup(lookup_id);
        drop(TraceSpan::begin(
            deadline_trace.clone(),
            "monochrome",
            "monochrome_probe_start",
            "monochrome_probe_end",
        ));
        deadline_trace.event(
            "resolution_end",
            "monochrome",
            json!({"deadline":true,"cancelled":false}),
        );

        let events = events.lock().unwrap();
        let phases = events
            .iter()
            .map(|event| event["phase"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            phases,
            [
                "monochrome_probe_start",
                "monochrome_probe_end",
                "monochrome_probe_start",
                "monochrome_probe_end",
                "resolution_end"
            ]
        );
        assert_eq!(events[1]["details"]["outcome"], "cancelled");
        assert_eq!(events[3]["details"]["outcome"], "abandoned");
        assert_eq!(events[4]["details"]["deadline"], true);
        for event in events.iter() {
            assert_eq!(event["nativeOperationId"], "native-fruit-start");
            assert_eq!(event["sharedLookupId"], lookup_id.to_string());
            assert_eq!(event["provider"], "monochrome");
            assert!(event.get("url").is_none());
            assert!(event.get("headers").is_none());
            assert!(event.get("title").is_none());
        }
    }
    use crate::models::LocalAsset;

    #[test]
    fn trace_payload_keeps_correlations_and_only_safe_phase_fields() {
        let trace = SourceTrace::new(Some("native-op_123".into()), None, Some(17));
        let payload = trace.payload(
            "stream_probe_response",
            "youtube",
            json!({
                "role":"head", "attempt":1, "durationMs":12, "status":206
            }),
        );
        assert_eq!(payload["nativeOperationId"], "native-op_123");
        assert_eq!(payload["webviewGeneration"], 17);
        assert_eq!(payload["nativeSeekOperationId"], Value::Null);
        let seek_trace = SourceTrace::new(None, Some("seek-op_456".into()), Some(18));
        let seek_payload = seek_trace.payload("recovery_start", "youtube", json!({}));
        assert_eq!(seek_payload["nativeOperationId"], Value::Null);
        assert_eq!(seek_payload["nativeSeekOperationId"], "seek-op_456");
        assert_eq!(payload["resolutionId"].as_str().unwrap().len(), 36);
        assert_eq!(payload["details"]["status"], 206);
        let second = SourceTrace::new(Some("native-op_456".into()), None, Some(17));
        assert_ne!(
            payload["resolutionId"],
            second.payload("resolution_start", "youtube", json!({}))["resolutionId"]
        );
        assert!(SourceTrace::new(
            Some("https://secret.example/path?token=x".into()),
            None,
            None
        )
        .native_operation_id
        .is_none());
        assert!(SourceTrace::new(Some(String::new()), None, None)
            .native_operation_id
            .is_none());
        let encoded = payload.to_string();
        assert!(!encoded.contains("https://"));
        assert!(!encoded.contains("title"));
        assert!(!encoded.contains("token"));
        assert!(!encoded.contains("arguments"));
    }

    fn stored_asset(id: &str, track: &Track, path: &std::path::Path) -> LocalAsset {
        LocalAsset {
            id: id.into(),
            recording_id: track.id.clone(),
            path: path.to_string_lossy().into_owned(),
            hash: format!("hash-{id}"),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(48_000),
            channels: Some(2),
            bit_depth: Some(24),
            duration_seconds: Some(210.0),
            size_bytes: Some(1024),
            bitrate_origin: Some("stream".into()),
            processing: "managed copy".into(),
            track: track.clone(),
        }
    }

    #[test]
    fn stale_queued_path_rehydrates_existing_sibling_without_replacing_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("stale-sibling.sqlite")).unwrap();
        let canonical = Track {
            id: "fixture:stale-sibling".into(),
            provider: "fixture".into(),
            provider_id: "stale-sibling".into(),
            artist: "Canonical artist".into(),
            title: "Canonical title".into(),
            duration_seconds: Some(210.0),
            ..Track::default()
        };
        store.save_track(&canonical).unwrap();
        let preferred = directory.path().join("preferred.flac");
        let sibling = directory.path().join("sibling.flac");
        std::fs::write(&preferred, b"preferred").unwrap();
        std::fs::write(&sibling, b"sibling").unwrap();
        let preferred_asset = stored_asset("preferred", &canonical, &preferred);
        let sibling_asset = stored_asset("sibling", &canonical, &sibling);
        store.save_asset(&preferred_asset).unwrap();
        store.save_asset(&sibling_asset).unwrap();
        store
            .set_preferred_track_file(&canonical.id, &preferred_asset.id)
            .unwrap();

        // Playback queued this hydrated listing, then the selected file disappeared.
        let queued = store.get_track(&canonical.id).unwrap().unwrap();
        assert_eq!(
            queued.local_path.as_deref(),
            Some(preferred.to_str().unwrap())
        );
        std::fs::remove_file(&preferred).unwrap();
        let mut incoming = queued;
        incoming.artist = "User-edited artist".into();
        incoming.title = "User-edited title".into();
        incoming.duration_seconds = Some(222.0);

        let prepared = reconcile_stale_local_asset(&store, &incoming, false).unwrap();
        assert_eq!(
            prepared.local_path.as_deref(),
            Some(sibling.to_str().unwrap())
        );
        assert_eq!(prepared.asset_id.as_deref(), Some("sibling"));
        assert_eq!(prepared.artist, "User-edited artist");
        assert_eq!(prepared.title, "User-edited title");
        assert_eq!(prepared.duration_seconds, Some(222.0));
    }

    #[tokio::test]
    async fn stale_path_with_no_remaining_asset_reaches_the_existing_resolver_and_its_error() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("stale-no-assets.sqlite")).unwrap();
        let canonical = Track {
            id: "fixture:stale-no-assets".into(),
            provider: "fixture".into(),
            provider_id: "stale-no-assets".into(),
            artist: "Artist".into(),
            title: "Title".into(),
            ..Track::default()
        };
        store.save_track(&canonical).unwrap();
        let stale_path = directory.path().join("removed.flac");
        std::fs::write(&stale_path, b"queued asset").unwrap();
        let asset = stored_asset("removed", &canonical, &stale_path);
        store.save_asset(&asset).unwrap();
        let queued = store.get_track(&canonical.id).unwrap().unwrap();
        std::fs::remove_file(&stale_path).unwrap();
        let continued = Arc::new(AtomicBool::new(false));
        let continued_in_resolver = continued.clone();

        let result =
            resolve_after_local_preflight(&store, &queued, false, move |track| async move {
                assert_eq!(track.local_path, None);
                assert_eq!(track.asset_id, None);
                assert_eq!(track.local_availability, "missing");
                assert!(!source_resolution_is_pinned(
                    &track,
                    false,
                    &Settings::default(),
                    true
                ));
                // The ordinary configured source policy remains available after stale local
                // fields are cleared; this continuation stands in for that resolver boundary.
                assert!(!crate::source_fallback::provider_order(
                    "youtube",
                    true,
                    &["soundcloud".into()],
                    false,
                )
                .is_empty());
                continued_in_resolver.store(true, Ordering::SeqCst);
                Err::<(), String>("No playable audio match was found for Artist — Title".into())
            })
            .await;

        assert!(continued.load(Ordering::SeqCst));
        assert_eq!(
            result.unwrap_err(),
            "No playable audio match was found for Artist — Title"
        );
    }

    #[test]
    fn stale_path_reconciliation_preserves_chapter_and_explicit_source_pins() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("stale-pinned.sqlite")).unwrap();
        let stale_path = directory.path().join("chapter-removed.flac");
        std::fs::write(&stale_path, b"queued chapter source").unwrap();
        let chapter = Track {
            id: "fixture:chapter".into(),
            provider: "fixture".into(),
            provider_id: "chapter".into(),
            local_path: Some(stale_path.to_string_lossy().into_owned()),
            playback_range: Some(crate::models::PlaybackRange {
                parent_track_id: "fixture:parent".into(),
                chapter_id: "chapter-1".into(),
                source_url: "https://example.invalid/chapter".into(),
                start_time: 12.0,
                end_time: 48.0,
                parent_title: "Parent".into(),
                parent_duration_seconds: Some(180.0),
            }),
            ..Track::default()
        };
        std::fs::remove_file(&stale_path).unwrap();
        let preserved = reconcile_stale_local_asset(&store, &chapter, false).unwrap();
        assert_eq!(preserved.local_path, chapter.local_path);
        assert_eq!(preserved.playback_range, chapter.playback_range);

        let ordinary = without_stale_local_asset(&Track {
            local_path: chapter.local_path.clone(),
            ..Track::default()
        });
        let settings = Settings::default();
        assert!(!source_resolution_is_pinned(
            &ordinary, false, &settings, true
        ));
        assert!(source_resolution_is_pinned(
            &ordinary, true, &settings, true
        ));
        let direct = Track {
            provider: "youtube".into(),
            ..ordinary
        };
        assert!(source_resolution_is_pinned(
            &direct, false, &settings, false
        ));
    }

    #[tokio::test]
    async fn unregistered_stale_path_with_selected_source_continues_to_explicit_resolver() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("unregistered-explicit.sqlite")).unwrap();
        let path = directory.path().join("unregistered-removed.flac");
        std::fs::write(&path, b"queued source").unwrap();
        let incoming = Track {
            id: "fixture:unregistered-explicit".into(),
            local_path: Some(path.to_string_lossy().into_owned()),
            ..Track::default()
        };
        std::fs::remove_file(path).unwrap();
        let continued = Arc::new(AtomicBool::new(false));
        let continued_in_resolver = continued.clone();
        let result =
            resolve_after_local_preflight(&store, &incoming, true, move |track| async move {
                assert_eq!(track.local_path, None);
                assert!(source_resolution_is_pinned(
                    &track,
                    true,
                    &Settings::default(),
                    true
                ));
                continued_in_resolver.store(true, Ordering::SeqCst);
                Err::<(), String>("Selected source could not be resolved".into())
            })
            .await;
        assert!(continued.load(Ordering::SeqCst));
        assert_eq!(result.unwrap_err(), "Selected source could not be resolved");
    }

    #[tokio::test]
    async fn closed_false_watch_does_not_cancel_the_actual_resolver() {
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            Store::open(&directory.path().join("closed-watch.sqlite")).unwrap(),
        );
        let connections =
            std::sync::Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
        let track = Track {
            id: "closed-watch".into(),
            local_path: Some(
                directory
                    .path()
                    .join("unregistered.wav")
                    .to_string_lossy()
                    .into(),
            ),
            ..Track::default()
        };
        let (sender, cancel) = watch::channel(false);
        drop(sender);
        let error = resolve(&store, &acquisition, &track, None, cancel)
            .await
            .err()
            .unwrap();
        assert_eq!(
            error, "Downloaded audio file is missing",
            "closure must not mask the actual source result"
        );
    }

    #[tokio::test]
    async fn closed_watch_retaining_true_still_cancels_the_actual_resolver() {
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            Store::open(&directory.path().join("cancelled-watch.sqlite")).unwrap(),
        );
        let connections =
            std::sync::Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
        let track = Track {
            local_path: Some("unregistered.wav".into()),
            ..Track::default()
        };
        let (sender, cancel) = watch::channel(false);
        sender.send_replace(true);
        drop(sender);
        assert_eq!(
            resolve(&store, &acquisition, &track, None, cancel)
                .await
                .err()
                .unwrap(),
            "Cancelled"
        );
    }

    #[tokio::test]
    async fn playback_resolution_deadline_covers_the_complete_nested_operation() {
        struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let future_dropped = dropped.clone();
        let result = within_deadline(
            Duration::from_millis(10),
            Box::pin(async {
                let _drop_flag = DropFlag(future_dropped);
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<_, String>(())
            }),
        )
        .await;
        assert_eq!(result.unwrap_err(), PLAYBACK_DEADLINE_MESSAGE);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn actual_resolver_future_is_bounded_and_polls_on_small_stack() {
        const CHILD: &str = "BURRATA_SMALL_STACK_RESOLVER_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "playback_sources::tests::actual_resolver_future_is_bounded_and_polls_on_small_stack",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(
                status.success(),
                "resolver stack regression subprocess failed: {status}"
            );
            return;
        }

        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            Store::open(&directory.path().join("resolver-small-stack.sqlite")).unwrap(),
        );
        let connections =
            std::sync::Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = crate::acquisition::AcquisitionManager::new_for_playback_test(
            store.clone(),
            connections,
        );
        let track = Track {
            id: "spotify:resolver-stack-test".into(),
            local_path: Some(
                directory
                    .path()
                    .join("not-registered.wav")
                    .to_string_lossy()
                    .into(),
            ),
            ..Track::default()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || {
                runtime.block_on(async move {
                    let (_cancel_sender, cancel) = watch::channel(false);
                    let future = resolve(&store, &acquisition, &track, None, cancel);
                    let inline_size = std::mem::size_of_val(&future);
                    assert!(
                        inline_size <= 4096,
                        "top-level resolver future inline size is {inline_size} bytes"
                    );
                    match future.await {
                        Ok(_) => panic!("missing local asset unexpectedly resolved"),
                        Err(error) => assert_eq!(error, "Downloaded audio file is missing"),
                    }
                })
            })
            .unwrap()
            .join()
            .expect("actual playback resolver must fit a 512-KiB stack");
    }

    #[tokio::test]
    async fn known_empty_chapters_do_not_start_another_extractor() {
        let candidate = Candidate {
            chapters: Some(vec![]),
            ..Candidate::default()
        };
        let settings = Settings {
            yt_dlp_path: "/missing-extractor-must-not-run".into(),
            ..Settings::default()
        };
        let (_sender, cancel) = watch::channel(false);
        let result =
            enrich_candidate_chapters(&Track::default(), candidate, &settings, cancel).await;
        assert!(result.unwrap().chapters.unwrap().is_empty());
    }

    #[test]
    fn resolved_sasa_identity_accepts_exact_main_version_and_rejects_radio_edit() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/playback/sasa-metadata-shapes.json"
        ))
        .unwrap();
        let row = &fixture["shapes"][1];
        let track = Track {
            id: "import:sasa".into(),
            artist: row["artist"].as_str().unwrap().into(),
            title: row["title"].as_str().unwrap().into(),
            version: None,
            duration_seconds: None,
            provenance: json!({"sourceKind":"import"}),
            ..Track::default()
        };
        let candidate = || Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            title: "search title is replaced by extractor".into(),
            confidence: "review".into(),
            duration_seconds: Some(281.0),
            ..Candidate::default()
        };
        let audio = |title: &str| acquisition::PreviewAudio {
            url: "https://cdn.example/audio".into(),
            mime_type: "audio/mp4".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(281.0),
            source_metadata: None,
            resolved_title: Some(title.into()),
            resolved_provider_id: Some("abcdefghijk".into()),
            chapters: None,
            selected_format_id: None,
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };

        let mut exact = candidate();
        assert!(accept_resolved_identity(
            &track,
            &mut exact,
            &audio("Timmy Regisford - Sasa (Main Version)"),
            true
        ));
        let mut wrong = candidate();
        assert!(!accept_resolved_identity(
            &track,
            &mut wrong,
            &audio("Timmy Regisford - Sasa (Radio Edit)"),
            true
        ));
    }

    #[test]
    fn provisional_candidates_require_resolved_identity_and_keep_known_chapter_state() {
        let track = Track {
            title: "Make Me Sway".into(),
            artist: "Massivedrum, Bruno Rosado".into(),
            duration_seconds: Some(263.),
            ..Track::default()
        };
        let mut candidate = Candidate {
            title: "Make Me Sway".into(),
            provider: "youtube".into(),
            uploader: Some("Massivedrum - Topic".into()),
            duration_seconds: Some(263.),
            confidence: "low".into(),
            ..Candidate::default()
        };
        let audio = acquisition::PreviewAudio {
            url: String::new(),
            mime_type: "audio/mp4".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(263.),
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: None,
            chapters: Some(vec![]),
            selected_format_id: None,
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        assert!(!accept_resolved_identity(
            &track,
            &mut candidate,
            &audio,
            true
        ));
        assert_eq!(candidate.chapters.as_ref().map(Vec::len), Some(0));
        assert!(accept_resolved_identity(
            &track,
            &mut candidate,
            &audio,
            false
        ));
    }

    #[test]
    fn playlist_url_override_uses_preferred_source_but_preserves_explicit_sources() {
        let mut settings = Settings::default();
        let mut track = Track {
            provider: "import".into(),
            provenance: json!({"sourceKind":"audioSearch"}),
            ..Track::default()
        };
        assert!(!overrides_import_url(&track, &settings));
        settings.override_playlist_track_urls = true;
        assert!(overrides_import_url(&track, &settings));
        track.provenance["sourceKind"] = json!("youtubeRadio");
        assert!(!overrides_import_url(&track, &settings));
        track.provenance["sourceKind"] = json!("audioSearch");
        track.provider = "youtube".into();
        assert!(!overrides_import_url(&track, &settings));
        track.provider = "import".into();
        track.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "set".into(),
            chapter_id: "chapter".into(),
            source_url: "https://www.youtube.com/watch?v=4-XSYttqtls".into(),
            start_time: 30.,
            end_time: 60.,
            parent_title: "Set".into(),
            parent_duration_seconds: Some(120.),
        });
        assert!(!overrides_import_url(&track, &settings));
    }

    #[test]
    fn linked_monochrome_search_retains_the_selected_canonical_recording() {
        let track: Track = serde_json::from_str(include_str!(
            "../../tests/fixtures/playback/fruit-bats-monochrome-selection.json"
        ))
        .unwrap();
        let candidate =
            linked_candidate(&track).expect("pinned catalog selection needs a candidate");
        assert_eq!(candidate.id, track.id);
        assert_eq!(candidate.url, track.source_url);
        assert_eq!(candidate.duration_seconds, Some(181.4));
        assert_eq!(
            candidate.source_metadata.as_ref().unwrap()["isrc"],
            "USMRG1967001"
        );
        // Even forged/stale stored provenance cannot redirect the selected URL.
        let mut stale = track;
        stale.provenance["audioCandidate"]["id"] = json!("monochrome:240966681495732224");
        stale.provenance["audioCandidate"]["url"] =
            json!("https://tracks.monochrome.st/track/240966681495732224");
        let canonical = linked_candidate(&stale).unwrap();
        assert_eq!(canonical.id, stale.id);
        assert_eq!(canonical.url, stale.source_url);
        assert!(canonical.source_metadata.is_none());
    }

    #[test]
    fn linked_monochrome_rejects_noncanonical_source_urls() {
        for url in [
            "https://tracks.monochrome.st/track/239910648984977408?source=other",
            "https://tracks.monochrome.st/track/239910648984977408#other",
            "https://tracks.monochrome.st.attacker.test/track/239910648984977408",
            "https://tracks.monochrome.st/track/not-an-id",
        ] {
            assert!(linked_candidate(&Track {
                source_url: url.into(),
                ..Track::default()
            })
            .is_none());
        }
    }

    #[test]
    fn linked_youtube_candidate_uses_video_id_from_public_url() {
        let candidate = linked_candidate(&Track {
            source_url: "https://www.youtube.com/watch?v=4-XSYttqtls".into(),
            provider_id: "unrelated-import-hash".into(),
            title: "Afeto".into(),
            ..Track::default()
        })
        .unwrap();
        assert_eq!(candidate.id, "youtube:4-XSYttqtls");
        assert_eq!(candidate.url, "https://www.youtube.com/watch?v=4-XSYttqtls");
    }

    #[test]
    fn imported_bandcamp_source_is_pinned_only_when_experiment_is_enabled() {
        let track = Track {
            provider: "import".into(),
            provider_id: "untrusted-import-hash".into(),
            source_url: "https://cuneiformrecords.bandcamp.com/track/sorrows-of-the-moon-featuring-mary-halvorson-2".into(),
            title: "Sorrows Of The Moon (featuring Mary Halvorson)".into(),
            artist: "Ergo".into(),
            duration_seconds: Some(310.254),
            ..Track::default()
        };
        assert!(linked_candidate_with_bandcamp(&track, false).is_none());
        let candidate = linked_candidate_with_bandcamp(&track, true).unwrap();
        assert!(candidate.id.is_empty());
        assert_eq!(candidate.provider, "bandcamp");
        assert_eq!(candidate.permission, "unknown");
        assert_eq!(candidate.url, track.source_url);

        let spotify_track = Track {
            source_url: "https://open.spotify.com/track/123".into(),
            ..track
        };
        assert!(linked_candidate_with_bandcamp(&spotify_track, true).is_none());
    }

    #[test]
    fn bandcamp_experiment_preserves_other_pinned_sources() {
        for (source_url, expected_provider) in [
            ("https://www.youtube.com/watch?v=4-XSYttqtls", "youtube"),
            ("https://soundcloud.com/artist/recording", "soundcloud"),
            (
                "https://tracks.monochrome.st/track/12345678901234567",
                "monochrome",
            ),
        ] {
            let track = Track {
                source_url: source_url.into(),
                ..Track::default()
            };
            let disabled = linked_candidate_with_bandcamp(&track, false).unwrap();
            let enabled = linked_candidate_with_bandcamp(&track, true).unwrap();
            assert_eq!(enabled.provider, expected_provider);
            assert_eq!(enabled.id, disabled.id);
            assert_eq!(enabled.url, disabled.url);
        }
    }

    #[test]
    fn bandcamp_resolved_identity_binds_numeric_id_and_enforces_recording_match() {
        let track = Track {
            provider: "import".into(),
            source_url: "https://cuneiformrecords.bandcamp.com/track/sorrows-of-the-moon-featuring-mary-halvorson-2".into(),
            title: "Sorrows Of The Moon (featuring Mary Halvorson)".into(),
            artist: "Ergo".into(),
            duration_seconds: Some(310.254),
            ..Track::default()
        };
        let mut candidate = linked_candidate_with_bandcamp(&track, true).unwrap();
        let audio = acquisition::PreviewAudio {
            url: "https://invalid.example/audio".into(),
            mime_type: "audio/mpeg".into(),
            codec: Some("mp3".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(310.254),
            selected_format_id: Some("mp3-128".into()),
            selected_format_has_video: false,
            source_metadata: Some(json!({"channel":"Ergo","uploader":"Ergo"})),
            resolved_title: Some(format!("{} {}", track.artist, track.title)),
            resolved_provider_id: Some("292222166".into()),
            chapters: Some(vec![]),
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        assert!(accept_resolved_identity(
            &track,
            &mut candidate,
            &audio,
            true
        ));
        assert_eq!(candidate.id, "bandcamp:292222166");

        let mut wrong_artist_candidate = linked_candidate_with_bandcamp(&track, true).unwrap();
        let wrong_artist_audio = acquisition::PreviewAudio {
            source_metadata: Some(
                json!({"channel":"Different Artist","uploader":"Different Artist"}),
            ),
            ..audio.clone()
        };
        assert!(!accept_resolved_identity(
            &track,
            &mut wrong_artist_candidate,
            &wrong_artist_audio,
            true
        ));

        let mut short_candidate = linked_candidate_with_bandcamp(&track, true).unwrap();
        let short_audio = acquisition::PreviewAudio {
            duration_seconds: Some(30.0),
            ..audio
        };
        assert!(!accept_resolved_identity(
            &track,
            &mut short_candidate,
            &short_audio,
            true
        ));

        let mut invalid_id_candidate = linked_candidate_with_bandcamp(&track, true).unwrap();
        let invalid_id_audio = acquisition::PreviewAudio {
            resolved_provider_id: Some("bandcamp:292222166".into()),
            ..short_audio
        };
        assert!(!accept_resolved_identity(
            &track,
            &mut invalid_id_candidate,
            &invalid_id_audio,
            true
        ));
    }

    #[test]
    fn linked_soundcloud_candidate_does_not_use_untrusted_import_id() {
        let candidate = linked_candidate(&Track {
            provider: "soundcloud".into(),
            provider_id: "import-hash-not-soundcloud-id".into(),
            source_url: "https://soundcloud.com/artist/track?utm_source=import".into(),
            title: "Track".into(),
            ..Track::default()
        })
        .unwrap();
        assert!(candidate.id.is_empty());
        assert_eq!(candidate.url, "https://soundcloud.com/artist/track");
    }

    #[test]
    fn linked_source_identity_rejects_non_track_urls() {
        for source_url in [
            "https://youtube.com/shorts/4-XSYttqtls",
            "https://youtube.com.attacker.test/watch?v=4-XSYttqtls",
            "https://soundcloud.com/artist/sets",
            "https://soundcloud.com/search?q=track",
        ] {
            assert!(linked_candidate(&Track {
                source_url: source_url.into(),
                ..Track::default()
            })
            .is_none());
        }
    }

    #[test]
    fn verified_source_fingerprint_covers_recording_identity_and_chapter_context() {
        let base = Track {
            title: "Song".into(),
            artist: "Artist".into(),
            version: Some("Radio edit".into()),
            duration_seconds: Some(205.2),
            isrc: Some("usabc1234567".into()),
            ..Track::default()
        };
        let fingerprint = recording_identity_fingerprint(&base).unwrap();
        let mut changed = base.clone();
        changed.title = "Song (Live)".into();
        assert_ne!(
            fingerprint,
            recording_identity_fingerprint(&changed).unwrap()
        );
        changed = base.clone();
        changed.artist = "Another Artist".into();
        assert_ne!(
            fingerprint,
            recording_identity_fingerprint(&changed).unwrap()
        );
        changed = base.clone();
        changed.version = Some("Extended mix".into());
        assert_ne!(
            fingerprint,
            recording_identity_fingerprint(&changed).unwrap()
        );
        changed = base.clone();
        changed.duration_seconds = Some(206.0);
        assert_ne!(
            fingerprint,
            recording_identity_fingerprint(&changed).unwrap()
        );
        changed = base.clone();
        changed.isrc = Some("USABC7654321".into());
        assert_ne!(
            fingerprint,
            recording_identity_fingerprint(&changed).unwrap()
        );
        changed.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "parent".into(),
            chapter_id: "chapter".into(),
            source_url: "https://www.youtube.com/watch?v=4-XSYttqtls".into(),
            start_time: 10.0,
            end_time: 40.0,
            parent_title: "Set".into(),
            parent_duration_seconds: Some(200.0),
        });
        assert_ne!(
            fingerprint,
            recording_identity_fingerprint(&changed).unwrap()
        );
    }

    #[test]
    fn remembered_topic_mapping_rehydrates_the_verified_channel() {
        let track = Track {
            title: "Song".into(),
            artist: "Artist".into(),
            duration_seconds: Some(205.0),
            ..Track::default()
        };
        let mut candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            title: "Song".into(),
            confidence: "review".into(),
            duration_seconds: Some(205.0),
            ..Candidate::default()
        };
        let audio = acquisition::PreviewAudio {
            url: "https://cdn.example/audio".into(),
            mime_type: "audio/mp4".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(205.0),
            resolved_title: Some("Song".into()),
            resolved_provider_id: Some("abcdefghijk".into()),
            source_metadata: Some(
                json!({"provider":"youtube","channelId":"UCOoxO3pt54tPbTMmAj16c9A",
                "channel":"Artist - Topic","artist":"Artist","artists":["Artist"],"track":"Song",
                "descriptionCreditLine":"Song · Artist","duration":205.0}),
            ),
            chapters: None,
            selected_format_id: None,
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        assert!(!crate::source_fallback::playback_recording_matches(
            &track, &candidate
        ));
        assert!(accept_resolved_identity(
            &track,
            &mut candidate,
            &audio,
            true
        ));
        assert_eq!(candidate.uploader.as_deref(), Some("Artist - Topic"));
        let mut changed = audio.clone();
        changed.source_metadata.as_mut().unwrap()["artist"] = json!("Different Artist");
        assert!(!accept_resolved_identity(
            &track,
            &mut candidate,
            &changed,
            true
        ));
    }

    #[test]
    fn resolved_identity_overrides_search_metadata_before_acceptance() {
        let track = Track {
            title: "Song".into(),
            artist: "Artist".into(),
            duration_seconds: Some(205.0),
            ..Track::default()
        };
        let mut candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            title: "Artist - Song (Official Audio)".into(),
            duration_seconds: Some(205.0),
            confidence: "high".into(),
            ..Candidate::default()
        };
        let audio = acquisition::PreviewAudio {
            url: "https://cdn.example/audio".into(),
            mime_type: "audio/mp4".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(205.0),
            source_metadata: None,
            resolved_title: Some("Artist Song".into()),
            resolved_provider_id: Some("abcdefghijk".into()),
            chapters: None,
            selected_format_id: None,
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        assert!(accept_resolved_identity(
            &track,
            &mut candidate,
            &audio,
            true
        ));
        assert_eq!(candidate.title, "Artist Song");

        let changed_title = acquisition::PreviewAudio {
            resolved_title: Some("Artist Live Version".into()),
            ..audio.clone()
        };
        candidate.title = "Artist Song".into();
        candidate.confidence = "high".into();
        assert!(!accept_resolved_identity(
            &track,
            &mut candidate,
            &changed_title,
            true
        ));

        let wrong_id = acquisition::PreviewAudio {
            resolved_provider_id: Some("anothervideo".into()),
            ..audio
        };
        candidate.title = "Search metadata should remain".into();
        assert!(!accept_resolved_identity(
            &track,
            &mut candidate,
            &wrong_id,
            true
        ));
        assert_eq!(candidate.title, "Search metadata should remain");
    }

    #[test]
    fn verified_youtube_integer_duration_rounding_is_ephemeral_and_bounded() {
        let track = Track {
            title: "Poso S' Agapo".into(),
            artist: "Stamatis Kraounakis".into(),
            duration_seconds: Some(245.48),
            ..Track::default()
        };
        let make_candidate = || Candidate {
            id: "youtube:hRNx674Z4QE".into(),
            provider: "youtube".into(),
            title: "Stamatis Kraounakis - Poso S' Agapo".into(),
            duration_seconds: Some(251.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let make_audio = |duration| acquisition::PreviewAudio {
            url: String::new(),
            mime_type: "audio/webm".into(),
            codec: Some("opus".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(duration),
            source_metadata: None,
            resolved_title: Some("Stamatis Kraounakis - Poso S' Agapo".into()),
            resolved_provider_id: Some("hRNx674Z4QE".into()),
            chapters: Some(vec![]),
            selected_format_id: Some("140".into()),
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };

        let mut candidate = make_candidate();
        assert!(accept_resolved_identity(
            &track,
            &mut candidate,
            &make_audio(251.0),
            true
        ));
        assert!(candidate.source_metadata.is_none());

        let mut excessive = make_candidate();
        assert!(!accept_resolved_identity(
            &track,
            &mut excessive,
            &make_audio(251.001),
            true
        ));
        let mut short = make_candidate();
        assert!(!accept_resolved_identity(
            &track,
            &mut short,
            &make_audio(170.0),
            true
        ));
        let mut wrong_title = make_candidate();
        assert!(!accept_resolved_identity(
            &track,
            &mut wrong_title,
            &acquisition::PreviewAudio {
                resolved_title: Some("Stamatis Kraounakis - Poso S' Agapo Radio Edit".into()),
                ..make_audio(251.0)
            },
            true
        ));
    }

    #[test]
    fn discogs_feature_alias_requires_exact_cache_and_fresh_extractor_identity() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../fixtures/discogs-prolific-credit-identity.json"
        ))
        .unwrap();
        let track = crate::artists::parse_discogs_release_tracks(
            &fixture["release"],
            "38549214",
            "https://www.discogs.com/release/38549214",
            false,
            None,
        )
        .remove(0);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("discogs-playback.sqlite")).unwrap();
        store
            .set_retained_provider_cache(
                crate::artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE,
                "38549214",
                &fixture["release"],
            )
            .unwrap();
        for profile in fixture["profiles"].as_array().unwrap() {
            store
                .set_retained_provider_cache(
                    crate::artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE,
                    &profile["id"].as_u64().unwrap().to_string(),
                    profile,
                )
                .unwrap();
        }
        let mut context =
            crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, &track);
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:vqDoiUMuqQA".into(),
            title: fixture["officialTitle"].as_str().unwrap().into(),
            duration_seconds: Some(180.0),
            permission: "unknown".into(),
            confidence: "review".into(),
            ..Candidate::default()
        };
        include_candidate_aliases(&mut context, &track, &candidate, None);
        assert!(provisional_playback_matches_with_proofs(
            &track,
            context.tracks(),
            context.isrc_enriched(),
            None,
            &candidate
        ));
        let audio = acquisition::PreviewAudio {
            url: String::new(),
            mime_type: "audio/mp4".into(),
            codec: Some("mp4a.40.2".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(180.0),
            source_metadata: Some(
                json!({"provider":"youtube","channelId":"UC1234567890123456789012","channel":"Nipsey Hussle","channelIsVerified":true}),
            ),
            resolved_title: Some(candidate.title.clone()),
            resolved_provider_id: Some("vqDoiUMuqQA".into()),
            chapters: Some(vec![]),
            selected_format_id: Some("140".into()),
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        assert!(accept_resolved_identity_with_aliases(
            &track,
            context.tracks(),
            &mut candidate.clone(),
            &audio,
            true
        ));
        assert!(!accept_resolved_identity_with_aliases(
            &track,
            &[],
            &mut candidate.clone(),
            &audio,
            true
        ));
        let unverified = acquisition::PreviewAudio {
            source_metadata: None,
            ..audio.clone()
        };
        assert!(!accept_resolved_identity_with_aliases(
            &track,
            context.tracks(),
            &mut candidate.clone(),
            &unverified,
            true
        ));
        let changed = acquisition::PreviewAudio {
            resolved_title: Some(
                "Nipsey Hussle and Bino Rideaux (feat. James Fauntleroy) - Sacrifices (Remix)"
                    .into(),
            ),
            ..audio
        };
        assert!(!accept_resolved_identity_with_aliases(
            &track,
            context.tracks(),
            &mut candidate.clone(),
            &changed,
            true
        ));
    }

    #[test]
    fn missing_catalog_duration_accepts_only_fresh_verified_exact_visualizer_identity() {
        let track = Track {
            title: "BbY WOW".into(),
            artist: "KAROL G, Judeline & rusowsky".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let candidate = || Candidate {
            provider: "youtube".into(),
            id: "youtube:fRIhCiUVaKs".into(),
            title: "KAROL G, Judeline, rusowsky - BbY WOW (Visualizer)".into(),
            duration_seconds: Some(226.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let audio = || acquisition::PreviewAudio {
            url: String::new(),
            mime_type: "audio/webm".into(),
            codec: Some("opus".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(226.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "channelId":"UC1234567890123456789012",
                "channel":"KAROL G",
                "uploader":"KAROL G",
                "channelIsVerified":true
            })),
            resolved_title: Some("KAROL G, Judeline, rusowsky - BbY WOW (Visualizer)".into()),
            resolved_provider_id: Some("fRIhCiUVaKs".into()),
            chapters: Some(vec![]),
            selected_format_id: Some("140".into()),
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };

        let mut accepted = candidate();
        assert!(crate::source_fallback::is_provisional_playback_candidate(
            &track, &accepted
        ));
        assert!(accept_resolved_identity(
            &track,
            &mut accepted,
            &audio(),
            true
        ));
        assert_eq!(accepted.duration_seconds, Some(226.0));
        assert_eq!(
            accepted.source_metadata.as_ref().unwrap()["channelIsVerified"],
            true
        );

        let rejects = [
            acquisition::PreviewAudio {
                source_metadata: Some(json!({
                    "provider":"youtube","channelId":"UC1234567890123456789012",
                    "channel":"Other Official","channelIsVerified":true
                })),
                ..audio()
            },
            acquisition::PreviewAudio {
                source_metadata: Some(json!({
                    "provider":"youtube","channelId":"UC1234567890123456789012",
                    "channel":"KAROL G","channelIsVerified":false
                })),
                ..audio()
            },
            acquisition::PreviewAudio {
                resolved_title: Some("KAROL G - BbY WOW (Remix)".into()),
                ..audio()
            },
            acquisition::PreviewAudio {
                resolved_title: Some("KAROL G - BbY WOW (Live)".into()),
                ..audio()
            },
            acquisition::PreviewAudio {
                resolved_title: Some("KAROL G - BbY WOW (Preview)".into()),
                ..audio()
            },
            acquisition::PreviewAudio {
                duration_seconds: Some(30.0),
                ..audio()
            },
            acquisition::PreviewAudio {
                duration_seconds: None,
                ..audio()
            },
            acquisition::PreviewAudio {
                resolved_provider_id: Some("otherVideo123".into()),
                ..audio()
            },
        ];
        for audio in rejects {
            assert!(!accept_resolved_identity(
                &track,
                &mut candidate(),
                &audio,
                true
            ));
        }

        let mut stale_search_identity = candidate();
        stale_search_identity.source_metadata = Some(json!({
            "provider":"youtube","channelId":"UC1234567890123456789012",
            "channel":"KAROL G","channelIsVerified":true
        }));
        let no_fresh_metadata = acquisition::PreviewAudio {
            source_metadata: None,
            ..audio()
        };
        assert!(!accept_resolved_identity(
            &track,
            &mut stale_search_identity,
            &no_fresh_metadata,
            true
        ));
        assert!(stale_search_identity.source_metadata.is_none());

        let bad_times_track = Track {
            artist: "Imael Angel".into(),
            title: "BAD TIMES".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let mut descriptive = Candidate {
            id: "youtube:fRIhCiUVaKs".into(),
            title: "Imael Angel - BAD TIMES (Deep Emotional Song)".into(),
            ..candidate()
        };
        let mut descriptive_audio = acquisition::PreviewAudio {
            resolved_title: Some("Imael Angel - BAD TIMES (Deep Emotional Song)".into()),
            ..audio()
        };
        descriptive_audio.source_metadata.as_mut().unwrap()["channel"] = json!("Imael Angel");
        descriptive_audio.source_metadata.as_mut().unwrap()["uploader"] = json!("Imael Angel");
        assert!(!accept_resolved_identity(
            &bad_times_track,
            &mut descriptive,
            &descriptive_audio,
            true
        ));
    }

    #[test]
    fn youtube_music_title_only_candidate_needs_fresh_full_structured_credit() {
        let track = Track {
            title: "BAD TIMES".into(),
            artist: "Imael Angel".into(),
            duration_seconds: None,
            ..Track::default()
        };
        let candidate = || Candidate {
            provider: "youtube".into(),
            id: "youtube:Hqp8vxkPrI8".into(),
            title: "BAD TIMES".into(),
            duration_seconds: Some(168.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(json!({
                "provider":"youtube",
                "playbackMusicCatalogCandidate":true,
                "catalogTitle":"BAD TIMES",
                "catalogArtist":"Imael Angel"
            })),
            ..Candidate::default()
        };
        let audio = || acquisition::PreviewAudio {
            url: String::new(),
            mime_type: "audio/webm".into(),
            codec: Some("opus".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(168.0),
            source_metadata: Some(json!({
                "provider":"youtube",
                "channelId":"UC1234567890123456789012",
                "channel":"Imael Angel",
                "uploader":"Imael Angel",
                "channelIsVerified":true,
                "track":"BAD TIMES",
                "artists":["Imael Angel","Imael Angel"]
            })),
            resolved_title: Some("BAD TIMES".into()),
            resolved_provider_id: Some("Hqp8vxkPrI8".into()),
            chapters: Some(vec![]),
            selected_format_id: Some("140".into()),
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };

        let mut accepted = candidate();
        assert!(crate::source_fallback::is_provisional_playback_candidate(
            &track, &accepted
        ));
        assert!(accept_resolved_identity(
            &track,
            &mut accepted,
            &audio(),
            true
        ));
        assert_eq!(accepted.title, "BAD TIMES");
        assert_eq!(
            accepted.source_metadata.as_ref().unwrap()["track"],
            "BAD TIMES"
        );

        // A remembered source lacks the provisional catalog marker. Fresh,
        // fully structured extractor metadata still undergoes the same proof.
        let mut remembered = candidate();
        remembered.source_metadata = Some(json!({
            "provider":"youtube","channelId":"UC1234567890123456789012",
            "channel":"Imael Angel","channelIsVerified":true,
            "track":"BAD TIMES","artists":["Imael Angel","Imael Angel"]
        }));
        assert!(accept_resolved_identity(
            &track,
            &mut remembered,
            &audio(),
            true
        ));

        let rejects = [
            acquisition::PreviewAudio {
                source_metadata: Some(json!({
                    "provider":"youtube","channelId":"UC1234567890123456789012",
                    "channel":"Imael Angel","channelIsVerified":true,"track":"BAD TIMES",
                    "artists":["Imael Angel","Different Artist"]
                })),
                ..audio()
            },
            acquisition::PreviewAudio {
                source_metadata: Some(json!({
                    "provider":"youtube","channelId":"UC1234567890123456789012",
                    "channel":"Other Official","channelIsVerified":true,"track":"BAD TIMES",
                    "artists":["Imael Angel"]
                })),
                ..audio()
            },
            acquisition::PreviewAudio {
                source_metadata: Some(json!({
                    "provider":"youtube","channelId":"UC1234567890123456789012",
                    "channel":"Imael Angel","channelIsVerified":false,"track":"BAD TIMES",
                    "artists":["Imael Angel"]
                })),
                ..audio()
            },
            acquisition::PreviewAudio {
                source_metadata: Some(json!({
                    "provider":"youtube","channelId":"UC1234567890123456789012",
                    "channel":"Imael Angel","channelIsVerified":true,"track":"401",
                    "artists":["Imael Angel"]
                })),
                ..audio()
            },
            acquisition::PreviewAudio {
                resolved_provider_id: Some("wrongVideo123".into()),
                ..audio()
            },
            acquisition::PreviewAudio {
                source_metadata: None,
                ..audio()
            },
        ];
        for audio in rejects {
            assert!(!accept_resolved_identity(
                &track,
                &mut candidate(),
                &audio,
                true
            ));
        }
    }

    fn matthew_catalog_verification_fixture() -> (Track, Candidate, acquisition::PreviewAudio) {
        let track = Track {
            id: "spotify:matthew-verification".into(),
            provider: "spotify".into(),
            provider_id: "matthew-verification".into(),
            title: "Matthew".into(),
            artist: "John Denver".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:uZGRATckoBM".into(),
            url: "https://www.youtube.com/watch?v=uZGRATckoBM".into(),
            title: "Matthew".into(),
            duration_seconds: Some(228.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(json!({"provider":"youtube",
                "playbackMusicCatalogCandidate":true,
                "catalogTitle":"Matthew","catalogArtist":"John Denver"})),
            ..Candidate::default()
        };
        let audio = acquisition::PreviewAudio {
            url: "https://cdn.example.test/fresh-audio".into(),
            mime_type: "audio/mp4".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(228.0),
            source_metadata: Some(json!({"provider":"youtube",
                "channelId":"UC-cnCe5zradDZlcI8YZboBg","channel":"John Denver",
                "channelIsVerified":true,"track":"Matthew","artists":["John Denver"]})),
            resolved_title: Some("Matthew".into()),
            resolved_provider_id: Some("uZGRATckoBM".into()),
            chapters: Some(vec![]),
            selected_format_id: None,
            selected_format_has_video: false,
            expected_content_length: None,
            provider_session: None,
            headers: vec![],
        };
        (track, candidate, audio)
    }

    #[test]
    fn accepted_catalog_proof_is_saved_and_cache_hit_requires_fresh_full_identity() {
        let (track, mut candidate, audio) = matthew_catalog_verification_fixture();
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("catalog-proof.sqlite")).unwrap();
        store.save_track(&track).unwrap();
        let fingerprint = recording_identity_fingerprint(&track).unwrap();
        let proof = verify_resolved_identity_with_aliases_and_proofs(
            &track,
            &[],
            &[],
            None,
            &mut candidate,
            &audio,
            true,
        )
        .unwrap();
        assert_eq!(proof.kind, RecordingProofKind::YoutubeMusicCatalog);
        assert_eq!(
            candidate.source_metadata.as_ref().unwrap()["channelIsVerified"],
            true
        );
        assert_eq!(candidate.uploader.as_deref(), Some("John Denver"));
        // The strict search matcher rejects this 5.574s difference. The exact
        // accepted result, including its integer quantization policy, is saved.
        assert!(!playback_recording_matches_with_aliases(
            &track,
            &[],
            &candidate
        ));
        remember_verified_source(
            &store,
            &track,
            &candidate,
            &audio,
            Some(&proof),
            Some(&fingerprint),
        );
        let mapping = store
            .verified_playback_source(&track.id, &fingerprint, "youtube")
            .unwrap()
            .unwrap();
        assert_eq!(
            mapping.verification_proof.as_ref().unwrap()["kind"],
            "youtube_music_catalog"
        );
        let remembered = remembered_candidate(&track, &mapping).unwrap();
        assert!(remembered
            .source_metadata
            .as_ref()
            .unwrap()
            .get("channel")
            .is_none());
        let mut checked = remembered.clone();
        let cache_proof = verify_resolved_identity_with_aliases_and_proofs(
            &track,
            &[],
            &[],
            None,
            &mut checked,
            &audio,
            true,
        )
        .unwrap();
        assert_eq!(cache_proof, proof);
        assert!(evaluate_candidate_gate_with_proofs(&track, &[], &[], None, &checked).accepted());

        let mut invalid_responses = Vec::new();
        for (field, value) in [
            ("artists", json!(["John Denver", "Wrong Guest"])),
            ("channel", json!("Other Official")),
            ("channelIsVerified", json!(false)),
            ("track", json!("Matthew (Live)")),
            ("track", json!("Matthew (Cover)")),
        ] {
            let mut invalid = audio.clone();
            invalid.source_metadata.as_mut().unwrap()[field] = value;
            invalid_responses.push(invalid);
        }
        let mut no_metadata = audio.clone();
        no_metadata.source_metadata = None;
        invalid_responses.push(no_metadata);
        let mut no_duration = audio.clone();
        no_duration.duration_seconds = None;
        invalid_responses.push(no_duration);
        let mut wrong_id = audio.clone();
        wrong_id.resolved_provider_id = Some("wrong000000".into());
        invalid_responses.push(wrong_id);
        for invalid in invalid_responses {
            // Even reusing a candidate with a previously verified channel must
            // not let any of that evidence stand in for the current response.
            let mut checked = candidate.clone();
            assert!(verify_resolved_identity_with_aliases_and_proofs(
                &track,
                &[],
                &[],
                None,
                &mut checked,
                &invalid,
                true,
            )
            .is_none());
            let mut checked = remembered.clone();
            assert!(verify_resolved_identity_with_aliases_and_proofs(
                &track,
                &[],
                &[],
                None,
                &mut checked,
                &invalid,
                true,
            )
            .is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remembered_resolution_rejects_wrong_fresh_credit_before_any_cdn_probe() {
        use std::os::unix::fs::PermissionsExt;
        let (track, _, _) = matthew_catalog_verification_fixture();
        let mapping = crate::storage::VerifiedPlaybackSource {
            provider: "youtube".into(),
            provider_id: "uZGRATckoBM".into(),
            permanent_url: "https://www.youtube.com/watch?v=uZGRATckoBM".into(),
            verification_proof: Some(json!({"version":1,"kind":"youtube_music_catalog"})),
        };
        let candidate = remembered_candidate(&track, &mapping).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(Store::open(&directory.path().join("wrong-fresh-credit.sqlite")).unwrap());
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
        let executable = directory.path().join("fake-yt-dlp");
        let output = json!({"id":"uZGRATckoBM","title":"Matthew","duration":228,
            "url":"https://rr1.googlevideo.com/videoplayback?fixture=wrong-guest",
            "acodec":"aac","vcodec":"none","protocol":"https","ext":"m4a",
            "channel_id":"UC-cnCe5zradDZlcI8YZboBg","channel":"John Denver",
            "channel_is_verified":true,"track":"Matthew","artists":["John Denver","Wrong Guest"]});
        std::fs::write(
            &executable,
            format!("#!/bin/sh\ncat <<'FIXTURE'\n{output}\nFIXTURE\n"),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Settings::default()
        };
        let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let sink = events.clone();
        let trace = SourceTrace::new(None, None, None).with_test_sink(Arc::new(move |event| {
            sink.lock().unwrap().push(event);
        }));
        let (_sender, cancel) = watch::channel(false);
        let aliases = crate::artist_credit_aliases::VerifiedPlaybackArtistAliases::default();
        let result =
            crate::acquisition::with_playback_test_public_addresses(try_verified_candidate(
                &store,
                &acquisition,
                &track,
                &aliases,
                candidate,
                &settings,
                cancel,
                &recording_identity_fingerprint(&track).unwrap(),
                &trace,
                PlaybackDeadline::after(Duration::from_secs(5)),
            ))
            .await;
        assert_eq!(
            result.err().as_deref(),
            Some("Verified source identity mismatch")
        );
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|event| event["phase"] == "extraction_end"));
        assert!(events
            .iter()
            .any(|event| event["phase"] == "mapping_identity_check"
                && event["details"]["ok"] == false));
        assert!(!events.iter().any(|event| matches!(
            event["phase"].as_str(),
            Some("dns_stream_start" | "stream_probe_start")
        )));
    }

    #[test]
    fn catalog_verification_accepts_fractional_duration_only_inside_strict_window() {
        let (track, candidate, audio) = matthew_catalog_verification_fixture();
        for (duration, accepted) in [
            (226.75, true),
            (227.426, true),
            (227.75, false),
            (228.0, true),
            (229.0, false),
        ] {
            let mut candidate = candidate.clone();
            let mut audio = audio.clone();
            audio.duration_seconds = Some(duration);
            assert_eq!(
                verify_resolved_identity_with_aliases_and_proofs(
                    &track,
                    &[],
                    &[],
                    None,
                    &mut candidate,
                    &audio,
                    true,
                )
                .is_some(),
                accepted,
                "duration {duration}"
            );
        }
    }

    #[tokio::test]
    async fn parallel_provider_traces_share_two_costly_extractions_per_request() {
        let trace = SourceTrace::new(None, None, None);
        let mut attempts = Vec::new();
        for provider in ["youtube", "soundcloud", "youtube", "soundcloud"] {
            let trace = trace.clone().with_event_provider(provider);
            attempts.push(tokio::spawn(async move { trace.try_spend_extraction() }));
        }
        let mut spent = 0;
        for attempt in attempts {
            spent += usize::from(attempt.await.unwrap());
        }
        assert_eq!(spent, 2);
        assert!(!trace.try_spend_extraction());
        assert!(SourceTrace::new(None, None, None).try_spend_extraction());
    }

    #[test]
    fn remembered_sources_require_canonical_urls_and_transient_errors_do_not_invalidate() {
        let track = Track {
            title: "Song".into(),
            ..Track::default()
        };
        let mapping = crate::storage::VerifiedPlaybackSource {
            provider: "youtube".into(),
            provider_id: "4-XSYttqtls".into(),
            permanent_url: "https://www.youtube.com/watch?v=4-XSYttqtls".into(),
            verification_proof: None,
        };
        assert_eq!(
            remembered_candidate(&track, &mapping).unwrap().id,
            "youtube:4-XSYttqtls"
        );
        let signed = crate::storage::VerifiedPlaybackSource {
            permanent_url: "https://www.youtube.com/watch?v=4-XSYttqtls&sig=temporary".into(),
            ..mapping.clone()
        };
        assert!(remembered_candidate(&track, &signed).is_none());
        assert!(is_definitive_source_failure("HTTP 404: video not found"));
        assert!(is_definitive_source_failure("The video is unavailable"));
        assert!(!is_definitive_source_failure("HTTP 429: rate limited"));
        assert!(!is_definitive_source_failure("temporarily unavailable"));
        assert!(!is_definitive_source_failure(
            "network connection timed out"
        ));
    }

    #[test]
    fn same_candidate_retry_is_limited_to_transient_stream_failures() {
        for error in [
            "Audio source denied stream access (HTTP 403)",
            "Audio source denied stream access (HTTP 410)",
            "Audio stream resolution timed out",
            "Audio stream access check failed",
        ] {
            assert!(should_retry_same_candidate(error), "{error}");
        }
        for error in [
            "Cancelled",
            "Track not found: id",
            "Source URL is invalid",
            "Unsupported audio format",
        ] {
            assert!(!should_retry_same_candidate(error), "{error}");
        }
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    async fn ready(value: &str, delay: u64) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        Ok(value.into())
    }
    #[tokio::test]
    async fn losing_attempt_is_dropped() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Guard(dropped.clone());
        let preferred = async move {
            let _guard = guard;
            std::future::pending::<Result<String, String>>().await
        };
        assert_eq!(
            prefer_with_grace(
                Box::pin(preferred),
                Box::pin(ready("fallback", 0)),
                Duration::ZERO,
            )
            .await
            .unwrap(),
            "fallback"
        );
        assert!(dropped.load(Ordering::Relaxed));
    }
    #[tokio::test]
    async fn preferred_wins_inside_grace() {
        assert_eq!(
            prefer_with_grace(
                Box::pin(ready("preferred", 5)),
                Box::pin(ready("fallback", 0)),
                Duration::from_millis(50)
            )
            .await
            .unwrap(),
            "preferred"
        );
    }
    #[tokio::test]
    async fn stalled_preferred_does_not_block_fallback() {
        assert_eq!(
            prefer_with_grace(
                Box::pin(std::future::pending::<Result<String, String>>()),
                Box::pin(ready("fallback", 0)),
                Duration::ZERO,
            )
            .await
            .unwrap(),
            "fallback"
        );
    }
    #[tokio::test]
    async fn definitive_failure_skips_grace() {
        assert_eq!(
            prefer_with_grace(
                Box::pin(async { Err("unavailable".into()) }),
                Box::pin(ready("fallback", 0)),
                Duration::from_secs(10)
            )
            .await
            .unwrap(),
            "fallback"
        );
    }
    #[tokio::test]
    async fn failed_fallback_preserves_preferred() {
        assert_eq!(
            prefer_with_grace(
                Box::pin(ready("preferred", 0)),
                Box::pin(async { Err("unavailable".into()) }),
                Duration::ZERO
            )
            .await
            .unwrap(),
            "preferred"
        );
    }
}

#[cfg(test)]
mod failed_candidate_request_tests {
    use super::*;
    #[test]
    fn failed_candidates_are_provider_scoped_and_do_not_poison_other_requests() {
        let first = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let second = Candidate {
            id: "youtube:lmnopqrstuv".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let failed = vec![FailedPlaybackCandidate {
            provider: first.provider.clone(),
            id: first.id.clone(),
        }];
        assert!(validate_failed_candidates(&failed).is_ok());
        assert!(candidate_failed(&first, &failed));
        assert!(!candidate_failed(&second, &failed));
        assert!(!candidate_failed(
            &Candidate {
                provider: "soundcloud".into(),
                ..first.clone()
            },
            &failed
        ));
        assert!(
            !candidate_failed(&first, &[]),
            "independent requests retain original verified candidate/cache eligibility"
        );
        let eligible =
            eligible_after_webview_failure(vec![first.clone(), second.clone()], &failed, None, 2);
        assert_eq!(eligible[0].id, second.id);
        let mapping = crate::storage::VerifiedPlaybackSource {
            provider: "youtube".into(),
            provider_id: "abcdefghijk".into(),
            permanent_url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            verification_proof: None,
        };
        let track = Track::default();
        assert!(remembered_for_request(&track, &mapping, &failed).is_none());
        assert!(remembered_for_request(&track, &mapping, &[]).is_some());

        assert!(validate_failed_candidates(&vec![failed[0].clone(); 3]).is_err());
        assert!(validate_failed_candidates(&[FailedPlaybackCandidate {
            provider: "youtube".into(),
            id: "bad\nidentity".into()
        }])
        .is_err());
    }
}
