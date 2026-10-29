//! Bounded, opt-in playback diagnostics. Events contain only allowlisted
//! phases, safe correlations, categorical values, counters, and timings.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};

pub const MAX_EVENTS: usize = 2_000;
const MAX_EVENT_BYTES: usize = 8 * 1024;

static ENABLED: AtomicBool = AtomicBool::new(false);
static EVENTS: OnceLock<Mutex<EventRing>> = OnceLock::new();

#[derive(Default)]
struct EventRing {
    events: VecDeque<PlaybackDiagnosticEvent>,
    dropped_events: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackDiagnosticEvent {
    pub resolution_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_seek_operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webview_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_lookup_id: Option<String>,
    pub at_unix_ms: i64,
    pub elapsed_ms: u64,
    pub phase: String,
    pub provider: String,
    pub details: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsStatus {
    pub enabled: bool,
    pub event_count: usize,
    pub max_events: usize,
    pub dropped_events: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackDiagnosticsExport {
    pub schema_version: u32,
    pub status: DiagnosticsStatus,
    pub events: Vec<PlaybackDiagnosticEvent>,
}

fn ring() -> &'static Mutex<EventRing> {
    EVENTS.get_or_init(|| Mutex::new(EventRing::default()))
}

/// Keep only the numeric version, never arbitrary helper stdout suffixes.
pub(crate) fn numeric_helper_version(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|word| {
        let raw = word.strip_prefix('v').unwrap_or(word);
        let numeric = raw
            .chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
            .collect::<String>();
        let numeric = numeric.trim_end_matches('.');
        let parts = numeric.split('.').collect::<Vec<_>>();
        (numeric.len() <= 40
            && (2..=5).contains(&parts.len())
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.len() <= 12))
        .then(|| numeric.to_owned())
    })
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub fn status() -> DiagnosticsStatus {
    let enabled = is_enabled();
    match ring().lock() {
        Ok(events) => DiagnosticsStatus {
            enabled,
            event_count: events.events.len(),
            max_events: MAX_EVENTS,
            dropped_events: events.dropped_events,
        },
        Err(_) => DiagnosticsStatus {
            enabled,
            event_count: 0,
            max_events: MAX_EVENTS,
            dropped_events: 0,
        },
    }
}

/// Clear the bounded in-memory report without changing the recording setting.
pub fn clear() -> DiagnosticsStatus {
    if let Ok(mut events) = ring().lock() {
        events.events.clear();
        events.dropped_events = 0;
    }
    status()
}

pub fn export() -> PlaybackDiagnosticsExport {
    let enabled = is_enabled();
    let (status, events) = match ring().lock() {
        Ok(events) => (
            DiagnosticsStatus {
                enabled,
                event_count: events.events.len(),
                max_events: MAX_EVENTS,
                dropped_events: events.dropped_events,
            },
            events.events.iter().cloned().collect(),
        ),
        Err(_) => (
            DiagnosticsStatus {
                enabled,
                event_count: 0,
                max_events: MAX_EVENTS,
                dropped_events: 0,
            },
            Vec::new(),
        ),
    };
    PlaybackDiagnosticsExport {
        schema_version: 1,
        status,
        events,
    }
}

/// Record one SourceTrace payload after applying the same sanitizer used by
/// event construction. Disabled mode returns before parsing or allocating.
pub fn record_safe_event(payload: Value) {
    if !is_enabled() {
        return;
    }
    if let Ok(mut events) = ring().lock() {
        record_into(&mut events, true, payload);
    }
}

fn record_into(events: &mut EventRing, enabled: bool, payload: Value) {
    if !enabled {
        return;
    }
    let Some(event) = sanitize_event(payload) else {
        events.dropped_events = events.dropped_events.saturating_add(1);
        return;
    };
    if serde_json::to_vec(&event).map_or(true, |value| value.len() > MAX_EVENT_BYTES) {
        events.dropped_events = events.dropped_events.saturating_add(1);
        return;
    }
    let background = |event: &PlaybackDiagnosticEvent| {
        event
            .native_operation_id
            .as_deref()
            .is_some_and(|id| id.starts_with("preparation:"))
    };
    // Reserve most of the report for foreground activity. Background lookups
    // must not evict the failure the user is trying to diagnose.
    if background(&event)
        && events
            .events
            .iter()
            .filter(|event| background(event))
            .count()
            >= MAX_EVENTS / 4
    {
        if let Some(index) = events.events.iter().position(background) {
            events.events.remove(index);
        }
        events.dropped_events = events.dropped_events.saturating_add(1);
    } else if events.events.len() == MAX_EVENTS {
        if let Some(index) = events.events.iter().position(background) {
            events.events.remove(index);
        } else {
            events.events.pop_front();
        }
        events.dropped_events = events.dropped_events.saturating_add(1);
    }
    events.events.push_back(event);
}

/// Sanitize details at the SourceTrace boundary so environment trace output
/// and test sinks receive exactly the same safe shape as exported diagnostics.
pub(crate) fn sanitize_details(details: Value) -> Map<String, Value> {
    let Some(details) = details.as_object() else {
        return Map::new();
    };
    let mut safe = Map::new();
    for (key, value) in details {
        if safe.len() >= 48 {
            break;
        }
        let sanitized = if key == "counts" {
            sanitize_counts(value)
        } else {
            sanitize_detail_value(key, value, 0)
        };
        if let Some(value) = sanitized {
            safe.insert(key.clone(), value);
        }
    }
    safe
}

/// Export only reproducible public page identities, never stream URLs or queries.
pub(crate) fn public_source_page(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return None;
    }
    let host = url.host_str()?;
    if matches!(
        host,
        "youtube.com" | "www.youtube.com" | "music.youtube.com" | "youtu.be"
    ) {
        let id = if host == "youtu.be" {
            url.path().trim_start_matches('/').to_owned()
        } else if url.path() == "/watch" {
            url.query_pairs().find(|(k, _)| k == "v")?.1.into_owned()
        } else {
            url.path().strip_prefix("/shorts/")?.to_owned()
        };
        if id.len() == 11
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Some(format!("https://www.youtube.com/watch?v={id}"));
        }
    } else if matches!(host, "soundcloud.com" | "www.soundcloud.com") {
        let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
        if parts.len() == 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.len() <= 200
                    && !p.starts_with("s-")
                    && p.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            })
            && !matches!(parts[0], "search" | "discover" | "you")
        {
            return Some(format!("https://soundcloud.com/{}/{}", parts[0], parts[1]));
        }
    }
    None
}

/// Closed vocabulary preserves diagnostic sentences and exception types while
/// removing arbitrary source text, URLs, paths, credentials and quoted values.
pub(crate) fn extractor_error_summary(raw: &str) -> String {
    const WORDS: &str = "error warning unable failed failure download downloading extract extraction extractor client id json api response request requested format available unavailable found not no invalid unsupported expected unexpected missing required key value type object string integer list dictionary none attribute index certificate verify verification ssl tls expired issuer local self signed connection connect timeout timed out reset refused remote host server name resolution temporary forbidden unauthorized authentication authorization login sign in bot confirm country region restricted subscription premium soundcloud youtube http https status code traceback most recent call last exception caused by during handling another occurred retry retries exhausted permission denied file directory module import runtime javascript challenge signature decipher token provider po gvs nsig cookie cookies account ffmpeg ffprobe keyerror typeerror valueerror indexerror attributeerror httperror urlerror sslerror certificateerror sslcertverificationerror extractorerror downloaderror runtimeerror importerror modulenotfounderror connectionerror timeouterror os system exit returned process terminated signal metadata audio video stream formats parsing parse result data empty malformed decode decoding too many requests rate limit geo restricted soundcloudgo urn policy transcodings protocol transcoding url progressive hls client_id";
    raw.split_whitespace()
        .take(512)
        .map(|word| {
            let clean = word
                .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_ascii_lowercase();
            if !word.contains(['/', '=', '@', '\\'])
                && !clean.is_empty()
                && (WORDS.split_whitespace().any(|known| known == clean)
                    || clean.parse::<u16>().is_ok_and(|n| (100..=599).contains(&n)))
            {
                clean
            } else {
                "[redacted]".to_owned()
            }
        })
        .fold(Vec::<String>::new(), |mut words, word| {
            if word != "[redacted]" || words.last() != Some(&word) {
                words.push(word);
            }
            words
        })
        .join(" ")
        .chars()
        .take(2048)
        .collect()
}

fn sanitize_detail_value(key: &str, value: &Value, depth: usize) -> Option<Value> {
    if depth > 2 {
        return None;
    }
    if key == "rows" {
        if let Some(rows) = value.as_array() {
            let safe_rows = rows
                .iter()
                .take(100)
                .filter_map(|row| sanitize_row(row, depth + 1))
                .collect::<Vec<_>>();
            return Some(Value::Array(safe_rows));
        }
        return value
            .as_number()
            .and_then(|number| bounded_number("rows", number))
            .map(Value::Number);
    }
    if bool_key(key) {
        return value.as_bool().map(Value::Bool);
    }
    if number_key(key) {
        let number = value.as_number()?;
        return bounded_number(key, number).map(Value::Number);
    }
    if key == "sourcePageUrl" {
        return public_source_page(value.as_str()?).map(Value::String);
    }
    if key == "extractorErrorSummary" {
        return Some(Value::String(extractor_error_summary(value.as_str()?)));
    }
    if key == "formatId" {
        let value = value.as_str()?;
        return ((!value.is_empty()
            && value.len() <= 4
            && value.bytes().all(|byte| byte.is_ascii_digit()))
            || ["hls_aac_96k", "hls_aac_160k", "http_mp3_128", "hls_mp3_128"].contains(&value))
        .then(|| Value::String(value.to_owned()));
    }
    if key == "sharedLookupId" {
        let value = value.as_str()?;
        return uuid::Uuid::parse_str(value)
            .ok()
            .map(|id| Value::String(id.to_string()));
    }
    if matches!(key, "sourceFingerprint" | "trackFingerprint") {
        let value = value.as_str()?;
        return is_hex_fingerprint(value).then(|| Value::String(value.to_ascii_lowercase()));
    }
    if let Some(allowed) = enum_values(key) {
        return value
            .as_str()
            .filter(|value| allowed.contains(value))
            .map(|value| Value::String(value.to_owned()));
    }
    if key == "candidateRows" {
        let rows = value.as_array()?;
        let safe_rows = rows
            .iter()
            .take(100)
            .filter_map(|row| sanitize_row(row, depth + 1))
            .collect::<Vec<_>>();
        return Some(Value::Array(safe_rows));
    }
    None
}

fn sanitize_counts(value: &Value) -> Option<Value> {
    const SAFE_COUNTERS: &[&str] = &[
        "candidateDurationMissingOrInvalid",
        "candidatesEvaluatedBeforeForegroundCap",
        "canonicalArtistPhrasePresentInCandidateTitle",
        "confidenceLow",
        "durationDeltaGt5s",
        "durationDeltaLe5s",
        "expectedDurationMissingOrInvalid",
        "monochromeCatalogTitleExact",
        "permissionRestricted",
        "requestedTitlePhrasePresentInCandidateTitle",
        "requestedVersionPresent",
        "requestedVersionTextPresentInCandidateTitle",
        "uploaderMatchesCanonicalArtistCredit",
        "uploaderMatchesVerifiedAliasCredit",
        "verifiedAliasArtistPhrasePresentInCandidateTitle",
        "versionConflictReasonFlagAsciiCaseInsensitive",
    ];
    let counts = value.as_object()?;
    let mut safe = Map::new();
    for (key, value) in counts {
        if !SAFE_COUNTERS.contains(&key.as_str()) {
            continue;
        }
        if let Some(number) = value.as_u64() {
            safe.insert(
                key.clone(),
                Value::Number(Number::from(number.min(1_000_000))),
            );
        }
    }
    Some(Value::Object(safe))
}

fn sanitize_row(value: &Value, depth: usize) -> Option<Value> {
    let row = value.as_object()?;
    let mut safe = Map::new();
    for (key, value) in row {
        if safe.len() >= 32 {
            break;
        }
        if let Some(value) = sanitize_detail_value(key, value, depth + 1) {
            safe.insert(key.clone(), value);
        }
    }
    Some(Value::Object(safe))
}

fn bool_key(key: &str) -> bool {
    matches!(
        key,
        "accepted"
            | "ok"
            | "cancelled"
            | "deadline"
            | "abandoned"
            | "selected"
            | "pinned"
            | "completed"
            | "rateLimited"
            | "joined"
            | "localSource"
            | "background"
            | "attempted"
            | "waitedForGate"
            | "lossless"
            | "candidateRowsTruncated"
            | "versionConflictFlag"
            | "requestedTitlePhrasePresent"
            | "catalogTitleExact"
            | "canonicalArtistPhrasePresent"
            | "verifiedAliasArtistPhrasePresent"
            | "recordingCanonicalMatch"
            | "recordingVerifiedAliasMatch"
            | "topicCanonicalMatch"
            | "topicVerifiedAliasMatch"
            | "provisionalCanonicalMatch"
            | "provisionalVerifiedAliasMatch"
            | "descriptiveDurationWithinFiveSeconds"
            | "requestedVersionPresent"
            | "descriptiveVersionTextPresentOrNotRequired"
            | "hasResults"
            | "providerIdMatches"
            | "resolvedTitlePresent"
            | "topicUploader"
            | "requestedVersion"
            | "reused"
            | "sameSearchRow"
            | "verifiedCreditSets"
            | "poTokenPresent"
            | "signaturePresent"
            | "nParameterPresent"
            | "ipParameterPresent"
            | "expiryPresent"
            | "refererPresent"
            | "originPresent"
            | "userAgentPresent"
            | "cookiesPresent"
            | "managedProviderInstalled"
            | "customHelper"
    )
}

fn number_key(key: &str) -> bool {
    matches!(
        key,
        "streamExpirySecondsRemaining"
            | "helperExitCode"
            | "elapsedMs"
            | "durationMs"
            | "budgetMs"
            | "recoveryAttemptsUsed"
            | "attemptsUsed"
            | "attempt"
            | "attemptCount"
            | "count"
            | "cacheHits"
            | "cacheMisses"
            | "waiterCount"
            | "flightWaiters"
            | "resultCount"
            | "observedElapsedMs"
            | "responseCount"
            | "fileCount"
            | "candidateCount"
            | "eligible"
            | "rejected"
            | "remainingQuerySlots"
            | "addressCount"
            | "status"
            | "httpStatus"
            | "expectedDelayMs"
            | "sharedExtractionAttemptsUsed"
            | "sharedExtractionAttemptsRemaining"
            | "sharedExtractionBudgetLimit"
            | "extractionAttemptsUsed"
            | "remainingMs"
            | "remainingBudgetMs"
            | "ordinal"
            | "candidateRowsLimit"
            | "firstOrdinal"
            | "totalCandidates"
            | "webviewGeneration"
            | "index"
            | "redirectHop"
            | "metadataRequests"
            | "bytes"
            | "queueMs"
            | "httpMs"
            | "lastStatus"
            | "windowSeconds"
            | "generation"
            | "runtimeMs"
            | "sampleRate"
            | "sampleRateHz"
            | "channels"
            | "bitDepth"
            | "bitrate"
            | "bitrateKbps"
            | "durationSeconds"
            | "catalogDuration"
            | "catalogDurationSeconds"
            | "candidateDurationSeconds"
            | "resolvedDuration"
            | "aliases"
            | "contentLength"
            | "expectedContentLength"
            | "rangeStart"
            | "rangeEnd"
            | "expiresInSeconds"
    )
}

fn bounded_number(key: &str, number: &Number) -> Option<Number> {
    if key == "helperExitCode" {
        return number
            .as_i64()
            .filter(|code| (-1..=255).contains(code))
            .map(Number::from);
    }
    if matches!(key, "status" | "httpStatus" | "lastStatus") {
        return number
            .as_u64()
            .filter(|value| (100..=599).contains(value))
            .map(Number::from);
    }
    if let Some(value) = number.as_u64() {
        let limit = if matches!(key, "webviewGeneration" | "generation") {
            u64::MAX
        } else if matches!(
            key,
            "contentLength" | "expectedContentLength" | "rangeStart" | "rangeEnd"
        ) {
            20_000_000_000
        } else if matches!(key, "sampleRate" | "sampleRateHz") {
            384_000
        } else if key == "channels" {
            32
        } else if key == "bitDepth" {
            64
        } else if matches!(key, "bitrate" | "bitrateKbps") {
            10_000_000
        } else if key == "expiresInSeconds" {
            2_592_000
        } else if matches!(
            key,
            "durationSeconds"
                | "catalogDuration"
                | "resolvedDuration"
                | "catalogDurationSeconds"
                | "candidateDurationSeconds"
        ) {
            86_400
        } else if matches!(
            key,
            "elapsedMs"
                | "durationMs"
                | "budgetMs"
                | "queueMs"
                | "httpMs"
                | "observedElapsedMs"
                | "runtimeMs"
        ) {
            86_400_000
        } else {
            1_000_000
        };
        return Some(Number::from(value.min(limit)));
    }
    let value = number.as_f64()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let limit = match key {
        "windowSeconds"
        | "durationSeconds"
        | "catalogDuration"
        | "resolvedDuration"
        | "catalogDurationSeconds"
        | "candidateDurationSeconds"
        | "expiresInSeconds" => 86_400.0,
        "sampleRate" | "sampleRateHz" => 384_000.0,
        "channels" => 32.0,
        "bitDepth" => 64.0,
        "bitrate" | "bitrateKbps" => 10_000_000.0,
        "contentLength" | "expectedContentLength" | "rangeStart" | "rangeEnd" => 20_000_000_000.0,
        _ => 86_400_000.0,
    };
    Number::from_f64(value.min(limit))
}

fn enum_values(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "urlClient" => Some(&["MWEB", "VISIONOS", "ANDROID", "WEB", "TVHTML5", "other"]),
        "youtubeClient" => Some(&["mweb", "visionos", "extractor_default", "not_youtube"]),
        "streamTransport" => Some(&["progressive", "hls", "other"]),
        "sourceHostClass" | "destinationHostClass" => Some(&[
            "youtube",
            "googlevideo",
            "account",
            "invalid",
            "soundcloud",
            "sndcdn",
            "monochrome",
            "bandcamp",
            "other",
        ]),
        "codec" => Some(&[
            "opus", "aac", "mp3", "flac", "vorbis", "alac", "pcm", "unknown",
        ]),
        "container" => Some(&[
            "webm", "m4a", "mp4", "flac", "mp3", "ogg", "wav", "aac", "m3u8", "mpegts", "unknown",
        ]),
        "source" => Some(&[
            "amazon",
            "tidal",
            "qobuz",
            "deezer",
            "spotify",
            "youtube",
            "apple",
            "bandcamp",
            "soundcloud",
            "monochrome",
            "direct",
            "cache",
            "network",
            "other",
            "unknown",
        ]),
        "cacheSource" => Some(&[
            "memory",
            "provider",
            "playback_candidate_search",
            "preview_audio",
            "unknown",
        ]),
        "runtime" => Some(&[
            "native",
            "webview",
            "extractor",
            "resolver",
            "direct",
            "other",
        ]),
        "extractorReason" => Some(&[
            "drm_protected",
            "tls_certificate",
            "helper_exception",
            "provider_response",
            "geo_restricted",
            "subscription_required",
            "provider_client",
            "unsupported_url",
            "cancelled",
            "timeout",
            "network",
            "rate_limited",
            "access_denied",
            "private",
            "removed",
            "format_unavailable",
            "source_unavailable",
            "other",
        ]),
        "category" => Some(&[
            "cancelled",
            "timeout",
            "network",
            "rate_limited",
            "source_access",
            "source_unavailable",
            "source_private",
            "source_deleted",
            "no_match",
            "recording_identity_mismatch",
            "unsupported_audio",
            "output",
            "storage",
            "unknown",
            "complete",
            "unavailable",
            "redirect",
            "decoder",
            "probe",
        ]),
        "role" => Some(&["head", "tail"]),
        "outcome" => Some(&[
            "success",
            "completed",
            "failed",
            "cancelled",
            "deadline",
            "abandoned",
            "aborted",
            "rate_limited",
            "http_error",
            "response_limit",
            "invalid_json",
            "unique_exact_name_profile",
            "no_exact_name_owner",
            "ambiguous",
            "no_verified_evidence",
        ]),
        "reason" => Some(&[
            "cdn_destination_changed",
            "insufficient_time",
            "recovery_budget_exhausted",
            "ordinary_resolution_succeeded",
            "ineligible",
            "no_artist_identity_block",
            "distinct_semantic_versions",
            "source_binding_mismatch",
            "unsupported_or_unmatched",
            "negative_cache",
        ]),
        "decision" => Some(&["inspect"]),
        "gate" => Some(&[
            "recording_canonical",
            "verified_artist_alias",
            "topic_canonical",
            "topic_verified_alias",
            "provisional_canonical",
            "provisional_verified_alias",
            "rejected",
        ]),
        "confidenceClass" => Some(&["high", "review", "low", "unknown"]),
        "permissionClass" => Some(&["restricted", "unknown", "allowed", "other"]),
        "providerClass" => Some(&["youtube", "soundcloud", "monochrome", "bandcamp", "other"]),
        "authority" => Some(&["musicbrainz", "discogs", "spotify", "other"]),
        "identity" => Some(&["complete", "incomplete"]),
        "scope" => Some(&["candidatesEvaluatedBeforeForegroundCap"]),
        "catalogProvider" => Some(&["spotify", "musicbrainz", "discogs"]),
        "stage" => Some(&[
            "request",
            "visitor",
            "player",
            "parse",
            "stream",
            "before_lookup",
            "waiting_for_worker",
            "after_wait",
        ]),
        "errorClass" | "dnsErrorClass" | "probeErrorClass" => {
            Some(&["timeout", "connect", "request", "body", "other"])
        }
        "proofKind" => Some(&[
            "recording",
            "verified_extractor",
            "youtube_music_catalog",
            "explicit_source",
            "none",
        ]),
        "code" => Some(&[
            "CANCELLED",
            "CIPHER_REQUIRED",
            "FORMAT_DURATION_MISSING",
            "FORMAT_LENGTH_MISSING",
            "FORMAT_MISMATCH",
            "FORMAT_UNAVAILABLE",
            "HTTP_CLIENT_UNAVAILABLE",
            "INVALID_BUDGET",
            "INVALID_DECODER_DIAGNOSTICS",
            "INVALID_MEDIA_URL",
            "INVALID_PLAYBACK_HEADERS",
            "INVALID_PLAYBACK_REQUEST",
            "INVALID_RESPONSE",
            "LIVE_OR_UPCOMING_SOURCE",
            "MANIFEST_EXPIRED",
            "METADATA_MISMATCH",
            "METADATA_MISSING",
            "NETWORK_FAILED",
            "N_TRANSFORM_REQUIRED",
            "PLAYABILITY_REJECTED",
            "PLAYER_HTTP_ERROR",
            "PREVIEW_OR_TRUNCATED_FORMAT",
            "RECORDING_MISMATCH",
            "RESPONSE_TOO_LARGE",
            "TIMEOUT",
            "UNSUPPORTED_TRANSPORT",
            "VISITOR_CONTEXT_UNAVAILABLE",
            "auth_unavailable",
            "auth_failed",
            "resolver_unauthorized",
            "resolver_forbidden",
            "resolver_verification_required",
            "resolver_rate_limited",
            "resolver_not_found",
            "resolver_http_error",
            "resolver_invalid_request",
            "resolver_invalid_auth",
            "resolver_transport_error",
            "resolver_response_too_large",
            "resolver_invalid_response",
            "match_missing_identity",
            "match_source_binding",
            "match_conflicting_isrc",
            "match_unsupported_resource",
            "match_rejected",
            "resource_unavailable",
            "relay_failed",
        ]),
        _ => None,
    }
}

fn is_hex_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sanitize_event(value: Value) -> Option<PlaybackDiagnosticEvent> {
    let value = value.as_object()?;
    let resolution_id = value.get("resolutionId")?.as_str()?;
    uuid::Uuid::parse_str(resolution_id).ok()?;
    let phase = value.get("phase")?.as_str()?;
    if !phase_is_allowed(phase) {
        return None;
    }
    let provider = match value.get("provider")?.as_str()? {
        "youtube" => "youtube",
        "soundcloud" => "soundcloud",
        "monochrome" => "monochrome",
        "bandcamp" => "bandcamp",
        "musicbrainz" => "musicbrainz",
        _ => "other",
    };
    let token = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .filter(|id| safe_token(id))
            .map(str::to_owned)
    };
    Some(PlaybackDiagnosticEvent {
        resolution_id: resolution_id.to_owned(),
        native_operation_id: token("nativeOperationId"),
        native_seek_operation_id: token("nativeSeekOperationId"),
        webview_generation: value.get("webviewGeneration").and_then(Value::as_u64),
        shared_lookup_id: token("sharedLookupId"),
        at_unix_ms: value.get("atUnixMs")?.as_i64()?,
        elapsed_ms: value.get("elapsedMs")?.as_u64()?.min(86_400_000),
        phase: phase.to_owned(),
        provider: provider.to_owned(),
        details: sanitize_details(value.get("details").cloned().unwrap_or(Value::Null)),
    })
}

fn safe_token(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
        || value
            .strip_prefix("native-")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        || value.strip_prefix("retry-").is_some_and(|generation| {
            !generation.is_empty()
                && generation.len() <= 20
                && generation.bytes().all(|byte| byte.is_ascii_digit())
        })
        || value
            .strip_prefix("preparation:")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

pub(crate) fn phase_is_allowed(value: &str) -> bool {
    matches!(
        value,
        "resolution_start"
            | "other"
            | "verified_source_transport_recovery_skipped"
            | "resolution_end"
            | "startup_end"
            | "source_context"
            | "stream_context"
            | "playback_candidate_search_cache_miss"
            | "playback_candidate_search_cache_hit"
            | "playback_candidate_search_cache_waited_hit"
            | "preview_audio_cache_hit"
            | "preview_audio_cache_miss"
            | "preview_audio_lookup_start"
            | "preview_audio_lookup_join"
            | "preview_audio_lookup_cancelled"
            | "preview_audio_lookup_end"
            | "preview_audio_profile_cooldown"
            | "settings_read_start"
            | "settings_read_end"
            | "identifier_lookup_end"
            | "catalog_duration_lookup_start"
            | "catalog_duration_lookup_end"
            | "bandcamp_fallback_start"
            | "bandcamp_fallback_end"
            | "canonical_search_warmup_end"
            | "catalog_alias_preparation_end"
            | "alias_lookup_skipped"
            | "alias_candidate_recheck"
            | "alias_cache_lookup_end"
            | "alias_discovery_start"
            | "alias_discovery_work"
            | "alias_discovery_end"
            | "alias_lookup_start"
            | "alias_lookup_end"
            | "alias_metadata_request_end"
            | "artist_alias_hydration"
            | "alias_search_start"
            | "provider_start"
            | "provider_cooldown"
            | "monochrome_resolver_end"
            | "monochrome_client_setup_start"
            | "monochrome_client_setup_end"
            | "monochrome_probe_start"
            | "monochrome_probe_end"
            | "mapping_resolve_start"
            | "mapping_resolve_end"
            | "mapping_identity_check"
            | "search_start"
            | "search_end"
            | "candidate_gate"
            | "candidate_gate_ambiguous_versions"
            | "candidate_gate_evidence"
            | "candidate_gate_evidence_rows"
            | "early_candidate_batch"
            | "early_candidate_final_rank"
            | "early_candidate_verification_start"
            | "early_candidate_verification_end"
            | "direct_youtube_start"
            | "direct_youtube_failure"
            | "direct_youtube_end"
            | "mapped_direct_youtube_start"
            | "mapped_direct_youtube_failure"
            | "mapped_direct_youtube_end"
            | "extract_and_validate_start"
            | "extract_and_validate_end"
            | "extraction_start"
            | "extraction_end"
            | "provider_prepare_start"
            | "provider_prepare_end"
            | "provider_helper_ready_end"
            | "full_identity_check"
            | "dns_source_start"
            | "dns_source_end"
            | "dns_stream_start"
            | "dns_stream_end"
            | "stream_probe_recheck"
            | "stream_probe_start"
            | "stream_probe_end"
            | "stream_probe_retry_wait"
            | "stream_probe_body_end"
            | "stream_probe_response"
            | "stream_redirect"
            | "decoder_start"
            | "decoder_end"
            | "prepared_alternate_metadata_reused"
            | "prepared_alternate_validation_start"
            | "verified_source_transport_recovery_start"
            | "verified_source_transport_recovery_end"
            | "recovery_start"
            | "recovery_end"
            | "next_preparation_start"
            | "next_preparation_end"
            | "next_preparation_policy"
            | "next_preparation_queued"
            | "next_preparation_discovery_end"
            | "next_preparation_identity_end"
            | "next_preparation_resolve_start"
            | "next_preparation_resolve_end"
            | "next_preparation_adopted"
            | "bandcamp_reference_verification_end"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(index: usize) -> Value {
        let operation_id = format!("native-{}", uuid::Uuid::new_v4());
        json!({
            "resolutionId":uuid::Uuid::new_v4().to_string(),
            "nativeOperationId":operation_id,
            "atUnixMs":1_800_000_000_000i64,
            "elapsedMs":index as u64,
            "phase":"stream_probe_end",
            "provider":"youtube",
            "details":{"role":"head","status":302,"ok":false}
        })
    }

    #[test]
    fn public_source_pages_strip_secrets_and_reject_streams() {
        assert_eq!(
            public_source_page("https://youtu.be/abcdefghijk?token=secret#private").as_deref(),
            Some("https://www.youtube.com/watch?v=abcdefghijk")
        );
        assert_eq!(
            public_source_page("https://soundcloud.com/artist/recording?secret_token=secret")
                .as_deref(),
            Some("https://soundcloud.com/artist/recording")
        );
        for raw in [
            "https://r1.googlevideo.com/videoplayback?sig=secret",
            "https://soundcloud.com/a/b/s-secret",
            "https://user:pass@youtube.com/watch?v=abcdefghijk",
            "https://youtube.com.evil/watch?v=abcdefghijk",
        ] {
            assert!(public_source_page(raw).is_none());
        }
    }

    #[test]
    fn extractor_summary_retains_exception_type_but_excludes_arbitrary_values() {
        let raw = "ERROR: Traceback (most recent call last): KeyError: 'secret' token=canary https://private.invalid/?sig=canary /Users/canary/file.py HTTP Error 403 forbidden";
        let safe = sanitize_details(
            json!({"extractorErrorSummary":raw,"sourcePageUrl":"https://youtube.com/watch?v=abcdefghijk&token=canary"}),
        );
        let encoded = serde_json::to_string(&safe).unwrap();
        assert!(encoded.contains("keyerror"));
        assert!(encoded.contains("403"));
        for secret in ["canary", "private.invalid", "Users", "secret"] {
            assert!(!encoded.contains(secret));
        }
        assert_eq!(
            safe["extractorErrorSummary"],
            extractor_error_summary(safe["extractorErrorSummary"].as_str().unwrap())
        );
    }

    #[test]
    fn sanitizer_removes_urls_titles_errors_and_nested_secret_canaries() {
        let mut payload = event(1);
        payload["details"] = json!({
            "status":302,
            "ok":false,
            "url":"https://cdn.example/private?token=do-not-export",
            "title":"secret song title",
            "error":"raw extractor stderr private-token",
            "nested":{"authorization":"Bearer very-secret"},
            "candidateRows":[{
                "ordinal":1,"accepted":true,"gate":"recording_canonical",
                "candidateTitle":"secret song title","confidenceClass":"high"
            }]
        });
        let sanitized = sanitize_event(payload).unwrap();
        let encoded = serde_json::to_string(&sanitized).unwrap();
        for secret in [
            "https://",
            "do-not-export",
            "secret song title",
            "extractor stderr",
            "very-secret",
            "candidateTitle",
        ] {
            assert!(!encoded.contains(secret), "leaked {secret}: {encoded}");
        }
        assert_eq!(sanitized.details["status"], 302);
        assert_eq!(
            sanitized.details["candidateRows"][0]["gate"],
            "recording_canonical"
        );
    }

    #[test]
    fn sanitizer_retains_safe_identity_evidence_and_drops_private_text() {
        let details = sanitize_details(json!({
            "providerIdMatches":true,
            "resolvedTitlePresent":false,
            "topicUploader":true,
            "requestedVersion":false,
            "reused":true,
            "sameSearchRow":false,
            "verifiedCreditSets":true,
            "catalogDuration":214.0,
            "resolvedDuration":218.5,
            "aliases":12,
            "title":"private title",
            "artist":"private artist",
            "url":"https://cdn.example/signed?token=private-token",
            "error":"private raw error"
        }));

        assert_eq!(details["providerIdMatches"], true);
        assert_eq!(details["resolvedTitlePresent"], false);
        assert_eq!(details["topicUploader"], true);
        assert_eq!(details["requestedVersion"], false);
        assert_eq!(details["reused"], true);
        assert_eq!(details["sameSearchRow"], false);
        assert_eq!(details["verifiedCreditSets"], true);
        assert_eq!(details["catalogDuration"], 214.0);
        assert_eq!(details["resolvedDuration"], 218.5);
        assert_eq!(details["aliases"], 12);

        let encoded = serde_json::to_string(&details).unwrap();
        for secret in [
            "private title",
            "private artist",
            "https://",
            "private-token",
            "private raw error",
        ] {
            assert!(!encoded.contains(secret), "leaked {secret}: {encoded}");
        }
    }

    #[test]
    fn sanitizer_bounds_identity_durations_and_alias_count() {
        let details = sanitize_details(json!({
            "catalogDuration":100_000.0,
            "resolvedDuration":-1.0,
            "aliases":u64::MAX
        }));

        assert_eq!(details["catalogDuration"], 86_400.0);
        assert!(details.get("resolvedDuration").is_none());
        assert_eq!(details["aliases"], 1_000_000);
    }

    #[test]
    fn ring_is_bounded_and_disable_stops_new_events() {
        let mut events = EventRing::default();
        for index in 0..=MAX_EVENTS {
            record_into(&mut events, true, event(index));
        }
        assert_eq!(events.events.len(), MAX_EVENTS);
        assert_eq!(events.dropped_events, 1);
        record_into(&mut events, false, event(MAX_EVENTS + 1));
        assert_eq!(events.events.len(), MAX_EVENTS);
    }

    #[test]
    fn background_preparation_cannot_crowd_out_foreground_failure() {
        let mut events = EventRing::default();
        let foreground = event(0);
        let foreground_id = foreground["resolutionId"].clone();
        record_into(&mut events, true, foreground);
        for index in 0..MAX_EVENTS * 2 {
            let mut payload = event(index);
            payload["nativeOperationId"] = json!(format!("preparation:{}", uuid::Uuid::new_v4()));
            record_into(&mut events, true, payload);
        }
        assert!(events
            .events
            .iter()
            .any(|event| event.resolution_id == foreground_id.as_str().unwrap()));
        assert!(events.events.len() <= MAX_EVENTS / 4 + 1);
        assert!(events.dropped_events > 0);
    }

    #[test]
    fn disabled_recording_returns_before_retaining_events() {
        let mut events = EventRing::default();
        record_into(&mut events, false, event(1));
        assert!(events.events.is_empty());
        assert_eq!(events.dropped_events, 0);
    }

    #[test]
    fn unsafe_phase_and_correlations_are_not_exported() {
        let mut payload = event(2);
        payload["phase"] = json!("https://private.example/?token=secret");
        assert!(sanitize_event(payload).is_none());

        let mut payload = event(3);
        payload["nativeOperationId"] = json!("https://private.example/?token=secret");
        let sanitized = sanitize_event(payload).unwrap();
        assert_eq!(sanitized.native_operation_id, None);
        assert!(!serde_json::to_string(&sanitized)
            .unwrap()
            .contains("private.example"));
    }

    #[test]
    fn comparison_fields_keep_only_valid_fingerprints_and_host_classes() {
        let mut payload = event(4);
        payload["details"] = json!({
            "sourceFingerprint":"a".repeat(64),
            "trackFingerprint":"not-a-hash",
            "sourceHostClass":"googlevideo",
            "destinationHostClass":"cdn.private.example",
            "redirectHop":2,
            "status":302,
            "category":"source_access"
        });
        let sanitized = sanitize_event(payload).unwrap();
        assert_eq!(sanitized.details["sourceFingerprint"], "a".repeat(64));
        assert!(sanitized.details.get("trackFingerprint").is_none());
        assert_eq!(sanitized.details["sourceHostClass"], "googlevideo");
        assert!(sanitized.details.get("destinationHostClass").is_none());
        assert_eq!(sanitized.details["redirectHop"], 2);
        assert_eq!(sanitized.details["status"], 302);
        assert_eq!(sanitized.details["category"], "source_access");
    }

    #[test]
    fn candidate_identity_evidence_retains_only_public_links_and_fixed_proof_kinds() {
        let safe = sanitize_details(json!({
            "proofKind":"verified_extractor",
            "candidateRows":[{
                "catalogDurationSeconds":120.093,
                "candidateDurationSeconds":240,
                "sourcePageUrl":"https://youtube.com/watch?v=abcdefghijk&token=secret",
                "title":"private title"
            }]
        }));
        assert_eq!(safe["proofKind"], "verified_extractor");
        assert_eq!(safe["candidateRows"][0]["catalogDurationSeconds"], 120.093);
        assert_eq!(safe["candidateRows"][0]["candidateDurationSeconds"], 240);
        assert_eq!(
            safe["candidateRows"][0]["sourcePageUrl"],
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        let encoded = serde_json::to_string(&safe).unwrap();
        assert!(!encoded.contains("secret") && !encoded.contains("private title"));
        assert!(sanitize_details(json!({"proofKind":"private title"}))
            .get("proofKind")
            .is_none());
    }

    #[test]
    fn extractor_reasons_retain_only_fixed_labels() {
        let safe = sanitize_details(json!({"helperExitCode":1,"stderr":"secret"}));
        assert_eq!(safe["helperExitCode"], 1);
        assert!(safe.get("stderr").is_none());
        assert!(sanitize_details(json!({"helperExitCode":9999}))
            .get("helperExitCode")
            .is_none());
        for reason in [
            "drm_protected",
            "tls_certificate",
            "helper_exception",
            "provider_response",
            "geo_restricted",
            "subscription_required",
            "provider_client",
            "access_denied",
            "other",
        ] {
            let safe = sanitize_details(
                json!({"extractorReason":reason,"httpStatus":403,"stderr":"private token=secret"}),
            );
            assert_eq!(safe["extractorReason"], reason);
            assert_eq!(safe["httpStatus"], 403);
            assert!(safe.get("stderr").is_none());
        }
        assert!(sanitize_details(
            json!({"extractorReason":"https://private.invalid/?token=secret"})
        )
        .get("extractorReason")
        .is_none());
    }

    #[test]
    fn redirect_diagnostic_classes_and_status_are_retained() {
        for class in ["googlevideo", "account", "other", "invalid"] {
            let safe = sanitize_details(json!({
                "status":302,
                "sourceHostClass":"googlevideo",
                "destinationHostClass":class,
                "redirectHop":1
            }));
            assert_eq!(safe["status"], 302);
            assert_eq!(safe["destinationHostClass"], class);
            assert_eq!(safe["redirectHop"], 1);
        }
    }

    #[test]
    fn cached_search_phases_and_safe_audio_format_fields_survive() {
        for phase in [
            "playback_candidate_search_cache_miss",
            "playback_candidate_search_cache_hit",
            "playback_candidate_search_cache_waited_hit",
            "preview_audio_cache_hit",
            "preview_audio_cache_miss",
            "preview_audio_lookup_start",
            "preview_audio_lookup_join",
            "preview_audio_lookup_cancelled",
            "preview_audio_lookup_end",
            "preview_audio_profile_cooldown",
        ] {
            assert!(phase_is_allowed(phase), "missing phase {phase}");
        }
        let mut payload = event(5);
        payload["phase"] = json!("playback_candidate_search_cache_miss");
        payload["details"] = json!({
            "waitedForGate":true,
            "observedElapsedMs":14.0,
            "resultCount":3,
            "role":"head",
            "codec":"opus",
            "container":"webm",
            "formatId":"251",
            "sampleRateHz":48000,
            "channels":2,
            "bitDepth":24,
            "bitrateKbps":160,
            "durationSeconds":212.5,
            "expectedContentLength":123456,
            "rangeStart":0,
            "rangeEnd":1023,
            "expiresInSeconds":60,
            "sharedLookupId":"550e8400-e29b-41d4-a716-446655440000",
            "errorClass":"connect",
            "rawError":"private URL https://secret.invalid"
        });
        let sanitized = sanitize_event(payload).unwrap();
        for field in [
            "waitedForGate",
            "observedElapsedMs",
            "resultCount",
            "role",
            "codec",
            "container",
            "formatId",
            "sampleRateHz",
            "channels",
            "bitDepth",
            "bitrateKbps",
            "durationSeconds",
            "expectedContentLength",
            "rangeStart",
            "rangeEnd",
            "expiresInSeconds",
            "sharedLookupId",
            "errorClass",
        ] {
            assert!(sanitized.details.contains_key(field), "lost {field}");
        }
        assert!(!sanitized.details.contains_key("rawError"));
        assert!(!serde_json::to_string(&sanitized)
            .unwrap()
            .contains("secret.invalid"));
        for format_id in ["1", "139", "9999"] {
            assert_eq!(
                sanitize_details(json!({"formatId":format_id}))["formatId"],
                format_id
            );
        }
        for format_id in ["12345", "format-139", ""] {
            assert!(sanitize_details(json!({"formatId":format_id}))
                .get("formatId")
                .is_none());
        }
    }

    #[test]
    fn exported_operation_correlations_require_known_uuid_forms() {
        let mut payload = event(6);
        payload["nativeOperationId"] = json!("track-title-secret");
        payload["nativeSeekOperationId"] = json!("native-not-a-uuid");
        payload["sharedLookupId"] = json!("lookup:arbitrary");
        let sanitized = sanitize_event(payload).unwrap();
        assert_eq!(sanitized.native_operation_id, None);
        assert_eq!(sanitized.native_seek_operation_id, None);
        assert_eq!(sanitized.shared_lookup_id, None);

        let operation_uuid = uuid::Uuid::new_v4();
        let mut payload = event(7);
        payload["nativeOperationId"] = json!(format!("native-{operation_uuid}"));
        payload["nativeSeekOperationId"] = json!(format!("preparation:{operation_uuid}"));
        payload["sharedLookupId"] = json!(operation_uuid.to_string());
        let sanitized = sanitize_event(payload).unwrap();
        assert_eq!(
            sanitized.native_operation_id,
            Some(format!("native-{operation_uuid}"))
        );
        assert_eq!(
            sanitized.native_seek_operation_id,
            Some(format!("preparation:{operation_uuid}"))
        );
        assert_eq!(sanitized.shared_lookup_id, Some(operation_uuid.to_string()));
    }
}

#[cfg(test)]
mod helper_version_privacy_tests {
    #[test]
    fn arbitrary_helper_suffixes_are_not_exported() {
        assert_eq!(
            super::numeric_helper_version("tool 1.2.3-privateAccountSecret"),
            Some("1.2.3".into())
        );
        assert_eq!(
            super::numeric_helper_version("v24.21.0"),
            Some("24.21.0".into())
        );
        assert_eq!(
            super::numeric_helper_version("https://private.example/1.2.3?token=secret"),
            None
        );
        assert_eq!(super::numeric_helper_version("SECRET=value"), None);
    }
}
