//! Privacy-bounded current playback context for diagnostic exports.
use serde_json::{json, Map, Value};

const MAX_TIMING_MS: u64 = 3_600_000;
const MAX_COUNTER: u64 = 1_000_000;

/// Project only bounded Native facts. Text, paths, headers, queue contents and
/// raw audio are never copied. Recording/source IDs become one-way fingerprints;
/// occurrence and operation correlations accept only UUIDs.
pub(crate) fn project(snapshot: &Value, target_triple: &str) -> Value {
    use sha2::{Digest, Sha256};
    let diagnostics = snapshot.get("diagnostics").and_then(Value::as_object);
    let mut context = Map::new();
    context.insert("targetTriple".into(), json!(safe_target(target_triple)));
    for (key, values) in [
        ("engine", &["native", "webview"][..]),
        (
            "status",
            &[
                "idle",
                "resolving",
                "playing",
                "paused",
                "buffering",
                "ended",
                "failed",
                "stopped",
            ][..],
        ),
    ] {
        if let Some(value) = enum_value(snapshot.get(key), values) {
            context.insert(key.into(), json!(value));
        }
    }
    let observed_decoder = diagnostics
        .and_then(|d| d.get("decoder"))
        .and_then(Value::as_object);
    // The service retains the previous started decoder through a new pending
    // request. Never attach that old file/codec/duration to a resolving/failed
    // generation. Even a started observation has no explicit decoder-owner ID.
    let decoder = observed_decoder.filter(|_| {
        matches!(
            snapshot.get("status").and_then(Value::as_str),
            Some("playing" | "paused" | "ended")
        ) && diagnostics
            .and_then(|d| d.get("firstAudioMs"))
            .and_then(Value::as_u64)
            .is_some()
    });
    if decoder.is_some() {
        context.insert("decoderEvidence".into(), json!("lastStartedSnapshot"));
        context.insert("decoderGenerationBinding".into(), json!("notRecorded"));
    } else if observed_decoder.is_some()
        && snapshot["engine"] == "native"
        && matches!(
            snapshot.get("status").and_then(Value::as_str),
            Some("resolving" | "failed" | "buffering" | "paused")
        )
    {
        context.insert("priorDecoderSnapshotOmitted".into(), json!(true));
    }
    if let Some(kind) = decoder.and_then(|d| {
        enum_value(
            d.get("decoderKind"),
            &[
                "rodio",
                "ffmpeg",
                "experimental_opus",
                "experimental-opus",
                "other",
            ],
        )
    }) {
        context.insert("decoderKind".into(), json!(kind));
    }
    for key in [
        "firstAudioMs",
        "sourceReadyMs",
        "seekFirstAudioMs",
        "seekOperationFirstAudioMs",
        "clickToIpcMs",
        "startupBudgetMs",
        "requestElapsedMs",
        "startupRemainingMs",
    ] {
        if let Some(n) = diagnostics.and_then(|d| bounded_u64(d.get(key), MAX_TIMING_MS)) {
            context.insert(key.into(), json!(n));
        }
    }
    for key in ["recoveryCount", "bufferingCount"] {
        if let Some(n) = diagnostics.and_then(|d| bounded_u64(d.get(key), MAX_COUNTER)) {
            context.insert(key.into(), json!(n));
        }
    }
    for key in ["generation", "seekSerial", "queueRevision"] {
        if let Some(n) = bounded_u64(snapshot.get(key), 9_007_199_254_740_991) {
            context.insert(key.into(), json!(n));
        }
    }
    if let Some(n) = snapshot
        .get("currentIndex")
        .and_then(Value::as_i64)
        .filter(|n| (-1..=1_000_000).contains(n))
    {
        context.insert("currentIndex".into(), json!(n));
    }
    for key in ["position", "duration"] {
        if let Some(n) = bounded_float(snapshot.get(key), 86_400.0) {
            context.insert(key.into(), json!(n));
        }
    }
    for key in ["paused", "naturalQueueEnd"] {
        if let Some(n) = snapshot.get(key).and_then(Value::as_bool) {
            context.insert(key.into(), json!(n));
        }
    }
    for key in ["entryId", "queuedOperationId"] {
        if let Some(id) = safe_uuid(snapshot.get(key)) {
            context.insert(key.into(), json!(id));
        }
    }
    for key in ["operationId", "seekOperationId"] {
        if let Some(id) = diagnostics.and_then(|d| safe_uuid(d.get(key))) {
            context.insert(key.into(), json!(id));
        }
    }
    if let Some(n) = bounded_float(
        snapshot.get("track").and_then(|t| t.get("durationSeconds")),
        86_400.0,
    ) {
        context.insert("catalogDurationSeconds".into(), json!(n));
    }
    if let Some(id) = snapshot
        .get("trackId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 512)
    {
        context.insert(
            "trackFingerprint".into(),
            json!(format!("{:x}", Sha256::digest(id.as_bytes()))),
        );
    }
    let candidate = snapshot.get("candidate");
    if let Some(provider) = candidate.and_then(|v| {
        enum_value(
            v.get("provider"),
            &["youtube", "soundcloud", "bandcamp", "monochrome", "radio"],
        )
    }) {
        context.insert("sourceProvider".into(), json!(provider));
        if let Some(id) = candidate
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 512)
        {
            context.insert(
                "sourceFingerprint".into(),
                json!(format!(
                    "{:x}",
                    Sha256::digest(format!("{provider}:{id}").as_bytes())
                )),
            );
        }
    }
    let quality = snapshot.get("quality");
    let mut q = Map::new();
    if let Some(kind) = quality.and_then(|v| {
        enum_value(
            v.get("sourceKind"),
            &[
                "localAsset",
                "approximateAlbumExcerpt",
                "remote",
                "stream",
                "radio",
            ],
        )
    }) {
        q.insert("sourceKind".into(), json!(kind));
    }
    if let Some(codec) = quality.and_then(|v| {
        enum_value(
            v.get("codec"),
            &[
                "aac",
                "mp3",
                "flac",
                "opus",
                "vorbis",
                "alac",
                "pcm",
                "pcm_s16le",
                "pcm_f32le",
                "unknown",
            ],
        )
    }) {
        q.insert("codec".into(), json!(codec));
    }
    for (key, max) in [
        ("sampleRate", 384_000),
        ("channels", 32),
        ("bitDepth", 64),
        ("bitrateKbps", 1_000_000),
    ] {
        if let Some(n) = quality
            .and_then(|v| bounded_u64(v.get(key), max))
            .filter(|n| *n > 0)
        {
            q.insert(key.into(), json!(n));
        }
    }
    if let Some(n) = quality.and_then(|v| bounded_float(v.get("durationSeconds"), 86_400.0)) {
        q.insert("reportedDurationSeconds".into(), json!(n));
    }
    if !q.is_empty() {
        context.insert("quality".into(), Value::Object(q));
    }
    if let Some(d) = decoder {
        let mut safe = bounded_facts(
            d,
            &[
                "bytesRead",
                "bytesReceived",
                "inputContentLength",
                "contentRangeTotal",
            ],
            &["readerSetupMs", "decoderSetupMs", "firstDecodedBlockMs"],
            &["readerReused", "decoderReused"],
        );
        for (key, max) in [("sampleRate", 384_000), ("channels", 32)] {
            if let Some(n) = bounded_u64(d.get(key), max).filter(|n| *n > 0) {
                safe.insert(key.into(), json!(n));
            }
        }
        if let Some(n) = bounded_float(d.get("totalDurationSeconds"), 86_400.0) {
            safe.insert("reportedDurationSeconds".into(), json!(n));
        }
        if !safe.is_empty() {
            context.insert("decoder".into(), Value::Object(safe));
        }
    }
    if let Some(last) = diagnostics.and_then(|d| d.get("lastBuffering")) {
        let mut safe = Map::new();
        for key in ["generation", "seekSerial"] {
            if let Some(n) = bounded_u64(last.get(key), 9_007_199_254_740_991) {
                safe.insert(key.into(), json!(n));
            }
        }
        if let Some(stream) = last.get("stream").and_then(Value::as_object) {
            let facts = bounded_facts(
                stream,
                &[
                    "bytesRead",
                    "downloadedRangeEnd",
                    "pcmProducedSamples",
                    "pcmConsumedSamples",
                    "syntheticSilenceSamples",
                ],
                &[
                    "downloadProgressAgeMs",
                    "readInFlightMs",
                    "maxReadMs",
                    "seekInFlightMs",
                    "maxSeekMs",
                ],
                &["readerObserved", "networkReader"],
            );
            if !facts.is_empty() {
                safe.insert("stream".into(), Value::Object(facts));
            }
        }
        if !safe.is_empty() {
            context.insert("lastBuffering".into(), Value::Object(safe));
        }
    }
    if let Some(last) = diagnostics.and_then(|d| d.get("lastRecovery")) {
        let mut safe = Map::new();
        if let Some(trigger) = enum_value(
            last.get("trigger"),
            &[
                "decoder_start_failure",
                "startup_timeout",
                "steady_buffering_timeout",
                "premature_ended",
                "output_interrupted",
                "decoder_failure",
            ],
        ) {
            safe.insert("trigger".into(), json!(trigger));
        }
        for key in ["generation", "seekSerial", "recoveryAttempt"] {
            if let Some(n) = bounded_u64(last.get(key), 9_007_199_254_740_991) {
                safe.insert(key.into(), json!(n));
            }
        }
        if let Some(n) = bounded_float(last.get("positionSeconds"), 86_400.0) {
            safe.insert("positionSeconds".into(), json!(n));
        }
        if !safe.is_empty() {
            context.insert("lastRecovery".into(), Value::Object(safe));
        }
    }
    if let Some(mut failure) = snapshot.get("failure").and_then(|v| {
        serde_json::from_value::<crate::playback_failure::PlaybackFailure>(v.clone()).ok()
    }) {
        failure.provider_attempts.truncate(32);
        for attempt in &mut failure.provider_attempts {
            attempt.observed_elapsed_ms = attempt.observed_elapsed_ms.min(86_400_000);
            attempt.phase_elapsed_ms = attempt
                .phase_elapsed_ms
                .map(|elapsed| elapsed.min(86_400_000));
        }
        context.insert(
            "failure".into(),
            serde_json::to_value(failure).unwrap_or(Value::Null),
        );
    }
    if let Some(fault) = diagnostics.and_then(|d| d.get("lastLocalStorageFailure")) {
        let operation = enum_value(
            fault.get("operation"),
            &["create", "read", "write", "flush", "seek"],
        );
        let kind = enum_value(
            fault.get("kind"),
            &["quotaExceeded", "noSpace", "permissionDenied", "other"],
        );
        if let (Some(operation), Some(kind)) = (operation, kind) {
            context.insert(
                "lastLocalStorageFailure".into(),
                json!({"operation":operation,"kind":kind}),
            );
        }
    }
    for key in ["lastSourceFailure", "lastSkippedTrackFailure"] {
        if let Some(fault) = diagnostics.and_then(|d| d.get(key)) {
            if let Some(reason) = fault
                .get("reason")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 4096)
            {
                context.insert(key.into(), json!(crate::playback_failure::classify(reason)));
            }
        }
    }
    if let Some(t) = snapshot.get("transition") {
        let mut safe = Map::new();
        for key in ["fromEntryId", "toEntryId"] {
            if let Some(id) = safe_uuid(t.get(key)) {
                safe.insert(key.into(), json!(id));
            }
        }
        if let Some(n) = bounded_float(t.get("progress"), 1.0) {
            safe.insert("progress".into(), json!(n));
        }
        if !safe.is_empty() {
            context.insert("transition".into(), Value::Object(safe));
        }
    }
    Value::Object(context)
}

fn safe_uuid(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    let raw = value.strip_prefix("native-").unwrap_or(value);
    uuid::Uuid::parse_str(raw).ok().map(|id| {
        if value.starts_with("native-") {
            format!("native-{id}")
        } else {
            id.to_string()
        }
    })
}

fn bounded_facts(
    input: &Map<String, Value>,
    counts: &[&str],
    timings: &[&str],
    booleans: &[&str],
) -> Map<String, Value> {
    let mut out = Map::new();
    for key in counts {
        if let Some(n) = bounded_u64(input.get(*key), 9_007_199_254_740_991) {
            out.insert((*key).into(), json!(n));
        }
    }
    for key in timings {
        if let Some(n) = bounded_u64(input.get(*key), MAX_TIMING_MS) {
            out.insert((*key).into(), json!(n));
        }
    }
    for key in booleans {
        if let Some(n) = input.get(*key).and_then(Value::as_bool) {
            out.insert((*key).into(), json!(n));
        }
    }
    out
}

/// Project the optional WebView snapshot supplied by the frontend at export
/// time. All values are categorical or tightly bounded numbers; caller data
/// such as URLs, media messages, queue identities, and track metadata is never
/// copied into the report.
pub(crate) fn project_webview(input: Option<&Value>) -> Option<Value> {
    let input = input?.as_object()?;
    let mut out = Map::new();
    out.insert("engine".into(), json!("webview"));
    if let Some(status) = enum_value(
        input.get("status"),
        &[
            "idle",
            "playing",
            "paused",
            "buffering",
            "ended",
            "failed",
            "stopped",
        ],
    ) {
        out.insert("status".into(), json!(status));
    }
    if let Some(observations) = input.get("testerObservations").and_then(Value::as_object) {
        let mut safe = Map::new();
        for (key, allowed) in [
            ("browserPlayback", &["not_tested", "works", "fails"][..]),
            (
                "alternateNetworkPlayback",
                &["not_tested", "works", "fails"][..],
            ),
            ("vpn", &["unknown", "enabled", "disabled"][..]),
        ] {
            if let Some(value) = enum_value(observations.get(key), allowed) {
                safe.insert(key.into(), json!(value));
            }
        }
        if !safe.is_empty() {
            out.insert("testerObservations".into(), Value::Object(safe));
        }
    }
    for key in [
        "currentTime",
        "duration",
        "resolvedDuration",
        "requestedSeekPosition",
    ] {
        if let Some(value) = bounded_float(input.get(key), 86_400.0) {
            out.insert(key.into(), json!(value));
        }
    }
    if let Some(provider) = enum_value(
        input.get("sourceProvider"),
        &["youtube", "soundcloud", "monochrome", "bandcamp"],
    ) {
        out.insert("sourceProvider".into(), json!(provider));
        if provider == "youtube" {
            if let Some(id) = input.get("sourceId").and_then(Value::as_str).filter(|id| {
                id.len() == 11
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }) {
                out.insert("sourceId".into(), json!(id));
            }
        }
    }
    for key in ["bufferedRanges", "seekableRanges"] {
        let ranges = project_ranges(input.get(key));
        if !ranges.is_empty() {
            out.insert(key.into(), json!(ranges));
        }
    }
    if let Some(rows) = input.get("startupOutcomes").and_then(Value::as_array) {
        let rows = rows.iter().rev().take(32).filter_map(|row| {
            let attempt = bounded_u64(row.get("attempt"), MAX_COUNTER)?;
            let elapsed = bounded_u64(row.get("elapsedMs"), MAX_TIMING_MS)?;
            let outcome = enum_value(row.get("outcome"), &["playing", "failed", "cancelled"])?;
            let operation = uuid::Uuid::parse_str(row.get("operationId")?.as_str()?).ok()?;
            Some(json!({"attempt":attempt,"elapsedMs":elapsed,"outcome":outcome,"operationId":operation.to_string()}))
        }).collect::<Vec<_>>();
        if !rows.is_empty() {
            out.insert(
                "startupOutcomes".into(),
                json!(rows.into_iter().rev().collect::<Vec<_>>()),
            );
        }
    }
    if let Some(value) = project_startup_failure(input.get("startupFailure")) {
        out.insert("startupFailure".into(), value);
    }
    if let Some(failures) = input.get("startupFailures").and_then(Value::as_array) {
        let failures = failures
            .iter()
            .take(8)
            .filter_map(|failure| {
                let attempt = bounded_u64(failure.get("attempt"), MAX_COUNTER)?;
                let mut safe = project_startup_failure(Some(failure))?.as_object()?.clone();
                safe.insert("attempt".into(), json!(attempt));
                Some(Value::Object(safe))
            })
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            out.insert("startupFailures".into(), json!(failures));
        }
    }
    if let Some(format) = input.get("sourceFormat").and_then(Value::as_object) {
        let mut safe = Map::new();
        for (key, values) in [
            (
                "codec",
                &[
                    "mp3", "aac", "opus", "vorbis", "flac", "alac", "pcm", "unknown",
                ][..],
            ),
            (
                "mimeType",
                &[
                    "audio/mpeg",
                    "audio/mp4",
                    "audio/webm",
                    "audio/ogg",
                    "audio/flac",
                    "audio/wav",
                    "application/vnd.apple.mpegurl",
                    "application/x-mpegurl",
                ][..],
            ),
            ("streamKind", &["direct", "hls", "unavailable"][..]),
        ] {
            if let Some(value) = enum_value(format.get(key), values) {
                safe.insert(key.into(), json!(value));
            }
        }
        if let Some(value) = format
            .get("formatId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit()))
        {
            safe.insert("formatId".into(), json!(value));
        }
        if let Some(value) = format.get("hasVideo").and_then(Value::as_bool) {
            safe.insert("hasVideo".into(), json!(value));
        }
        if let Some(value) = bounded_u64(format.get("expectedContentLength"), 17_179_869_184) {
            safe.insert("expectedContentLength".into(), json!(value));
        }
        if !safe.is_empty() {
            out.insert("sourceFormat".into(), Value::Object(safe));
        }
    }
    if let Some(events) = input.get("mediaEvents").and_then(Value::as_array) {
        let events = events
            .iter()
            .take(64)
            .filter_map(|event| {
                let event = event.as_object()?;
                let mut safe = Map::new();
                let event_type = enum_value(
                    event.get("type"),
                    &[
                        "play",
                        "playing",
                        "waiting",
                        "stalled",
                        "canplay",
                        "canplaythrough",
                        "loadedmetadata",
                        "durationchange",
                        "seeking",
                        "seeked",
                        "watchdog-expired",
                        "ended",
                        "error",
                    ],
                )?;
                safe.insert("type".into(), json!(event_type));
                for (key, max) in [("readyState", 4), ("networkState", 3)] {
                    if let Some(value) = bounded_u64(event.get(key), max) {
                        safe.insert(key.into(), json!(value));
                    }
                }
                if let Some(value) = bounded_u64(event.get("mediaErrorCode"), 4) {
                    if value > 0 {
                        safe.insert("mediaErrorCode".into(), json!(value));
                    }
                }
                if let Some(value) = event.get("paused").and_then(Value::as_bool) {
                    safe.insert("paused".into(), json!(value));
                }
                if let Some(value) = bounded_number(event.get("atMs"), 60_000) {
                    safe.insert("atMs".into(), json!(value));
                }
                if let Some(value) = bounded_u64(event.get("attempt"), MAX_COUNTER) {
                    safe.insert("attempt".into(), json!(value));
                }
                if let Some(value) = bounded_float(event.get("currentTime"), 86_400.0) {
                    safe.insert("currentTime".into(), json!(value));
                }
                if let Some(value) = bounded_float(event.get("durationSeconds"), 86_400.0) {
                    safe.insert("durationSeconds".into(), json!(value));
                }
                if let Some(value) =
                    bounded_u64(event.get("webviewGeneration"), 9_007_199_254_740_991)
                {
                    safe.insert("webviewGeneration".into(), json!(value));
                }
                let ranges = project_ranges(event.get("buffered"));
                if !ranges.is_empty() {
                    safe.insert("bufferedRanges".into(), json!(ranges));
                }
                Some(Value::Object(safe))
            })
            .collect::<Vec<_>>();
        if !events.is_empty() {
            out.insert("mediaEvents".into(), json!(events));
        }
    }
    Some(Value::Object(out))
}

fn project_startup_failure(value: Option<&Value>) -> Option<Value> {
    let startup = value?.as_object()?;
    let mut safe = Map::new();
    for (key, values) in [
        (
            "phase",
            &["attach", "resume_seek", "play", "clock_progress"][..],
        ),
        (
            "mimeFamily",
            &[
                "audio/mpeg",
                "audio/mp4",
                "audio/webm",
                "audio/ogg",
                "audio/flac",
                "audio/wav",
                "application/vnd.apple.mpegurl",
                "application/x-mpegurl",
                "other",
                "unknown",
            ][..],
        ),
        ("canPlayType", &["probably", "maybe", "no", "unknown"][..]),
        (
            "streamKind",
            &["direct", "native-hls", "mse-hls", "unknown"][..],
        ),
        (
            "sourceTransport",
            &["direct", "loopback-relay", "loopback-transcode", "unknown"][..],
        ),
    ] {
        if let Some(value) = enum_value(startup.get(key), values) {
            safe.insert(key.into(), json!(value));
        }
    }
    if let Some(stages) = startup.get("stageTimings").and_then(Value::as_object) {
        let mut projected = Map::new();
        for key in ["attach", "resume_seek", "play", "clock_progress"] {
            if let Some(value) = bounded_u64(stages.get(key), 60_000) {
                projected.insert(key.into(), json!(value));
            }
        }
        if !projected.is_empty() {
            safe.insert("stageTimings".into(), Value::Object(projected));
        }
    }
    if let Some(value) = startup.get("resumeRequested").and_then(Value::as_bool) {
        safe.insert("resumeRequested".into(), json!(value));
    }
    for (key, max) in [
        ("readyState", 4),
        ("networkState", 3),
        ("mediaErrorCode", 4),
    ] {
        if let Some(value) = bounded_u64(startup.get(key), max) {
            safe.insert(key.into(), json!(value));
        }
    }
    for key in ["currentTime", "duration", "requestedSeekPosition"] {
        if let Some(value) = bounded_float(startup.get(key), 86_400.0) {
            safe.insert(key.into(), json!(value));
        }
    }
    if let Some(value) = startup.get("seeking").and_then(Value::as_bool) {
        safe.insert("seeking".into(), json!(value));
    }
    for key in ["bufferedRanges", "seekableRanges"] {
        let ranges = project_ranges(startup.get(key));
        if !ranges.is_empty() {
            safe.insert(key.into(), json!(ranges));
        }
    }
    if let Some(value) = bounded_u64(startup.get("elapsedMs"), MAX_TIMING_MS) {
        safe.insert("elapsedMs".into(), json!(value));
    }
    (!safe.is_empty()).then_some(Value::Object(safe))
}

fn enum_value<'a>(value: Option<&'a Value>, allowed: &[&'static str]) -> Option<&'static str> {
    let value = value?.as_str()?;
    allowed.iter().copied().find(|item| *item == value)
}

fn bounded_u64(value: Option<&Value>, maximum: u64) -> Option<u64> {
    let value = value?.as_u64()?;
    (value <= maximum).then_some(value)
}

fn bounded_number(value: Option<&Value>, maximum: u64) -> Option<u64> {
    let value = value?.as_f64()?;
    (value.is_finite() && value >= 0.0 && value <= maximum as f64).then(|| value.round() as u64)
}

fn bounded_float(value: Option<&Value>, maximum: f64) -> Option<f64> {
    let value = value?.as_f64()?;
    (value.is_finite() && (0.0..=maximum).contains(&value)).then_some(value)
}

fn project_ranges(value: Option<&Value>) -> Vec<[f64; 2]> {
    let Some(ranges) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    ranges
        .iter()
        .take(8)
        .filter_map(|range| {
            let (start, end) = if let Some(pair) = range.as_array() {
                if pair.len() != 2 {
                    return None;
                }
                (pair.first(), pair.get(1))
            } else if let Some(object) = range.as_object() {
                (object.get("start"), object.get("end"))
            } else {
                return None;
            };
            let start = bounded_float(start, 86_400.0)?;
            let end = bounded_float(end, 86_400.0)?;
            (end >= start).then_some([start, end])
        })
        .collect()
}

fn safe_target(value: &str) -> &str {
    if value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        value
    } else {
        "unknown"
    }
}

pub(crate) fn safe_os_version(value: &str) -> Option<String> {
    let value = value.trim();
    let parts = value.split('.').collect::<Vec<_>>();
    (value.len() <= 24
        && (2..=3).contains(&parts.len())
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit())
        }))
    .then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_projection_retains_duration_owner_and_decoder_facts_without_private_data() {
        let entry = uuid::Uuid::new_v4().to_string();
        let input = json!({"engine":"native","status":"playing","generation":7,"seekSerial":2,"entryId":entry,"currentIndex":1,"position":125.5,"duration":250,"trackId":"private-library-id","track":{"title":"private-title","durationSeconds":120},"candidate":{"provider":"youtube","id":"private-source-id","url":"https://secret/?token=secret"},"quality":{"sourceKind":"localAsset","codec":"aac","sampleRate":48000,"channels":2,"path":"/private/music"},"diagnostics":{"firstAudioMs":1,"decoder":{"decoderKind":"rodio","sampleRate":48000,"channels":2,"totalDurationSeconds":250,"bytesRead":8192,"readerReused":false,"contentType":"private-type"},"lastBuffering":{"generation":7,"seekSerial":2,"stream":{"pcmProducedSamples":96000,"pcmConsumedSamples":48000,"syntheticSilenceSamples":4,"readerObserved":true,"url":"private-buffer-url"}}}});
        let safe = project(&input, "aarch64-apple-darwin");
        assert_eq!(safe["position"], 125.5);
        assert_eq!(safe["duration"], 250.0);
        assert_eq!(safe["catalogDurationSeconds"], 120.0);
        assert_eq!(safe["generation"], 7);
        assert_eq!(safe["seekSerial"], 2);
        assert_eq!(safe["entryId"], entry);
        assert_eq!(safe["status"], "playing");
        assert_eq!(safe["decoder"]["reportedDurationSeconds"], 250.0);
        assert_eq!(safe["decoder"]["bytesRead"], 8192);
        assert_eq!(safe["lastBuffering"]["stream"]["pcmConsumedSamples"], 48000);
        assert_eq!(safe["trackFingerprint"].as_str().unwrap().len(), 64);
        assert_eq!(safe["sourceFingerprint"].as_str().unwrap().len(), 64);
        let encoded = safe.to_string();
        for secret in [
            "private-library-id",
            "private-source-id",
            "private-title",
            "secret",
            "/private/music",
            "private-buffer-url",
            "private-type",
        ] {
            assert!(!encoded.contains(secret), "{encoded}");
        }
    }

    #[test]
    fn pending_native_generation_does_not_inherit_old_local_decoder_evidence() {
        for status in ["resolving", "failed", "paused"] {
            let safe = project(
                &json!({"engine":"native","generation":6,"status":status,"diagnostics":{"firstAudioMs":null,"decoder":{"decoderKind":"rodio","sampleRate":48000,"channels":2,"totalDurationSeconds":12,"bytesRead":8192}}}),
                "aarch64-apple-darwin",
            );
            assert_eq!(safe["priorDecoderSnapshotOmitted"], true);
            assert!(safe.get("decoderKind").is_none());
            assert!(safe.get("decoder").is_none());
            assert!(!safe.to_string().contains("8192"));
        }
    }

    #[test]
    fn native_projection_rejects_unbounded_values_and_free_text_owners() {
        let safe = project(
            &json!({"engine":"native","position":-1,"duration":86401,"entryId":"secret","generation":9007199254740992u64,"seekSerial":-1,"quality":{"sampleRate":999999,"channels":33,"codec":"secret"},"transition":{"progress":2,"fromEntryId":"secret"},"diagnostics":{"lastBuffering":{"stream":{"bytesRead":-1,"rawPcm":[1,2,3]}}}}),
            "aarch64-apple-darwin",
        );
        for key in [
            "position",
            "duration",
            "entryId",
            "generation",
            "seekSerial",
            "quality",
            "transition",
        ] {
            assert!(safe.get(key).is_none(), "{safe}");
        }
        assert!(!safe.to_string().contains("secret"));
        assert!(!safe.to_string().contains("rawPcm"));
    }

    #[test]
    fn duration_diagnostics_bound_identity_and_metadata_events() {
        let input = json!({"resolvedDuration":263.0,"duration":525.0,"sourceProvider":"youtube","sourceId":"BTH_Nn_TtDI","mediaEvents":[{"type":"loadedmetadata","durationSeconds":263.0},{"type":"durationchange","durationSeconds":525.0}]});
        let safe = project_webview(Some(&input)).unwrap();
        assert_eq!(safe["resolvedDuration"], 263.0);
        assert_eq!(safe["duration"], 525.0);
        assert_eq!(safe["sourceId"], "BTH_Nn_TtDI");
        assert_eq!(safe["mediaEvents"].as_array().unwrap().len(), 2);
        let bad = json!({"resolvedDuration":90000,"sourceProvider":"youtube","sourceId":"BTH_Nn_TtDI?token=secret","url":"https://secret.invalid"});
        let safe = project_webview(Some(&bad)).unwrap();
        assert!(safe.get("sourceId").is_none());
        assert!(safe.get("resolvedDuration").is_none());
        assert!(!safe.to_string().contains("secret"));
    }

    #[test]
    fn playback_outcomes_keep_operation_correlation_without_free_text() {
        let id = uuid::Uuid::new_v4().to_string();
        let safe = project_webview(Some(&json!({"startupOutcomes":[
            {"attempt":1,"operationId":id,"outcome":"playing","elapsedMs":30,"error":"secret"},
            {"attempt":2,"operationId":"secret","outcome":"failed","elapsedMs":40}
        ]})))
        .unwrap();
        assert_eq!(safe["startupOutcomes"].as_array().unwrap().len(), 1);
        assert_eq!(safe["startupOutcomes"][0]["operationId"], id);
        assert!(!safe.to_string().contains("secret"));
    }

    #[test]
    fn projection_keeps_only_allowlisted_scalar_context() {
        let snapshot = json!({
            "engine":"native", "status":"playing", "track":{"title":"private title"},
            "candidate":{"url":"https://secret.invalid/?token=x","provider":"youtube"},
            "playerMessage":"secret error", "quality":{"codec":"private codec"},
            "diagnostics":{
                "firstAudioMs":321,"sourceReadyMs":654,"seekFirstAudioMs":null,
                "recoveryCount":2,"bufferingCount":4,
                "decoder":{"decoderKind":"ffmpeg","codecHint":"secret"},
                "operationId":"private-id"
            }
        });
        let projected = project(&snapshot, "x86_64-unknown-linux-gnu");
        assert_eq!(
            projected,
            json!({
                "targetTriple":"x86_64-unknown-linux-gnu", "engine":"native", "status":"playing",
                "decoderKind":"ffmpeg", "decoderEvidence":"lastStartedSnapshot", "decoderGenerationBinding":"notRecorded", "sourceProvider":"youtube", "firstAudioMs":321,"sourceReadyMs":654,
                "recoveryCount":2,"bufferingCount":4
            })
        );
        let serialized = projected.to_string();
        for secret in [
            "private title",
            "secret.invalid",
            "secret error",
            "private codec",
            "private-id",
        ] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn projection_rejects_unknown_enums_oversized_numbers_and_target_text() {
        let projected = project(
            &json!({
                "engine":"native; secret", "status":"failed: private", "diagnostics":{
                    "firstAudioMs":3_600_001,"recoveryCount":1_000_001,
                    "decoder":{"decoderKind":"unknown decoder text"}
                }
            }),
            "x86_64/linux with secret",
        );
        assert_eq!(projected, json!({"targetTriple":"unknown"}));
    }

    #[test]
    fn webview_projection_keeps_bounded_safe_diagnostics_and_discards_private_fields() {
        let source = json!({
            "engine":"native", "status":"playing", "currentTime":52.5,"duration":400,"requestedSeekPosition":53,
            "bufferedRanges":[[0,20],[10,5],[-1,4],[0,86401]],"seekableRanges":[[0,300]],
            "trackTitle":"PRIVATE TITLE", "url":"https://private.invalid/?token=secret", "userAgent":"secret ua",
            "startupFailure":{"phase":"play","mimeFamily":"audio/mpeg","canPlayType":"probably","streamKind":"direct","sourceTransport":"loopback-relay","resumeRequested":true,"readyState":0,"networkState":3,"mediaErrorCode":4,"elapsedMs":90000,"currentTime":349.4,"duration":7202.1,"seeking":true,"requestedSeekPosition":349.437,"bufferedRanges":[[340,360],[0,90000],[10,5]],"seekableRanges":[[0,7202]],"message":"private error"},
            "sourceFormat":{"codec":"opus","mimeType":"audio/ogg","formatId":"251","hasVideo":false,"expectedContentLength":1000,"url":"https://secret"},
            "mediaEvents":[{"type":"watchdog-expired","attempt":2,"readyState":1,"networkState":2,"paused":false,"atMs":42,"currentTime":99,"durationSeconds":120.093,"webviewGeneration":52,"buffered":[{"start":0,"end":5},{"start":8,"end":4}],"trackId":"private"},{"type":"bad https://secret"}]
        });
        let projected = project_webview(Some(&source)).unwrap();
        assert_eq!(
            projected,
            json!({
                "engine":"webview",
                "status":"playing",
                "currentTime":52.5,"duration":400.0,"requestedSeekPosition":53.0,"bufferedRanges":[[0.0,20.0]],"seekableRanges":[[0.0,300.0]],
                "startupFailure":{"phase":"play","mimeFamily":"audio/mpeg","canPlayType":"probably","streamKind":"direct","sourceTransport":"loopback-relay","resumeRequested":true,"readyState":0,"networkState":3,"mediaErrorCode":4,"elapsedMs":90000,"currentTime":349.4,"duration":7202.1,"seeking":true,"requestedSeekPosition":349.437,"bufferedRanges":[[340.0,360.0]],"seekableRanges":[[0.0,7202.0]]},
                "sourceFormat":{"codec":"opus","mimeType":"audio/ogg","formatId":"251","hasVideo":false,"expectedContentLength":1000},
                "mediaEvents":[{"type":"watchdog-expired","attempt":2,"readyState":1,"networkState":2,"paused":false,"atMs":42,"currentTime":99.0,"durationSeconds":120.093,"webviewGeneration":52,"bufferedRanges":[[0.0,5.0]]}]
            })
        );
        let serialized = projected.to_string();
        for private in [
            "PRIVATE TITLE",
            "private.invalid",
            "secret",
            "private error",
            "private\"",
        ] {
            assert!(!serialized.contains(private));
        }
    }

    #[test]
    fn webview_projection_caps_event_count_and_rejects_unrecognized_values() {
        let events = (0..80)
            .map(|_| json!({"type":"playing","readyState":4,"networkState":1,"atMs":1}))
            .collect::<Vec<_>>();
        let input = json!({"startupFailure":{"phase":"play;secret","mediaErrorCode":999},"sourceFormat":{"codec":"https://secret","formatId":"99999"},"mediaEvents":events});
        let projected = project_webview(Some(&input)).unwrap();
        assert_eq!(projected["mediaEvents"].as_array().unwrap().len(), 64);
        assert!(projected.get("startupFailure").is_none());
        assert!(projected.get("sourceFormat").is_none());

        let generations = json!({"mediaEvents":[
            {"type":"playing","webviewGeneration":9_007_199_254_740_991u64},
            {"type":"playing","webviewGeneration":9_007_199_254_740_992u64},
            {"type":"playing","webviewGeneration":"52"}
        ]});
        let projected = project_webview(Some(&generations)).unwrap();
        let events = projected["mediaEvents"].as_array().unwrap();
        assert_eq!(events[0]["webviewGeneration"], 9_007_199_254_740_991u64);
        assert!(events[1].get("webviewGeneration").is_none());
        assert!(events[2].get("webviewGeneration").is_none());
    }

    #[test]
    fn webview_projection_bounds_seek_positions_and_caps_valid_ranges() {
        let ranges = (0..12).map(|n| json!([n, n + 1])).collect::<Vec<_>>();
        let input = json!({"currentTime":86401,"duration":-1,"requestedSeekPosition":90000,"seekableRanges":ranges});
        let projected = project_webview(Some(&input)).unwrap();
        assert!(projected.get("currentTime").is_none());
        assert!(projected.get("duration").is_none());
        assert!(projected.get("requestedSeekPosition").is_none());
        assert_eq!(projected["seekableRanges"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn webview_projection_preserves_bounded_failure_history_across_retry_events() {
        let failures = (0..12).map(|n| json!({"attempt":n+1,"phase":"play","mediaErrorCode":3,"message":"https://private.invalid/token"})).collect::<Vec<_>>();
        let input = json!({
            "status":"playing",
            "startupFailure":null,
            "startupFailures":failures,
            "mediaEvents":[
                {"type":"error","attempt":1,"readyState":0,"networkState":3,"mediaErrorCode":3},
                {"type":"playing","attempt":2,"readyState":4,"networkState":1}
            ]
        });
        let projected = project_webview(Some(&input)).unwrap();
        assert_eq!(projected["status"], "playing");
        assert_eq!(projected["startupFailures"].as_array().unwrap().len(), 8);
        assert_eq!(projected["startupFailures"][0]["attempt"], 1);
        assert_eq!(projected["mediaEvents"][0]["attempt"], 1);
        assert_eq!(projected["mediaEvents"][1]["attempt"], 2);
        assert!(!projected.to_string().contains("private.invalid"));
    }

    #[test]
    fn operating_system_version_accepts_only_bounded_numeric_versions() {
        assert_eq!(safe_os_version(" 15.4.1\n"), Some("15.4.1".into()));
        assert_eq!(safe_os_version("14.7"), Some("14.7".into()));
        for value in ["15.4.1 (Build 24E)", "secret", "1.2.3.4", "1234.2"] {
            assert_eq!(safe_os_version(value), None);
        }
    }
    #[test]
    fn startup_progress_stage_keeps_bounded_timings_without_source_fields() {
        let result = project_startup_failure(Some(&json!({
            "phase":"clock_progress", "stageTimings":{
                "attach":0, "play":12, "clock_progress":15,
                "resume_seek":90001, "url":"https://private.invalid/token"
            }
        })))
        .unwrap();
        assert_eq!(
            result,
            json!({"phase":"clock_progress", "stageTimings":{"attach":0,"play":12,"clock_progress":15}})
        );
    }
}
