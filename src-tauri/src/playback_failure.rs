//! Shared, safe playback failure policy. Classify before redacting extractor output.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    Cancelled,
    Timeout,
    Network,
    RateLimited,
    SourceAccess,
    SourceUnavailable,
    SourcePrivate,
    SourceDeleted,
    NoMatch,
    RecordingIdentityMismatch,
    UnsupportedAudio,
    Output,
    Storage,
    Unknown,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureStage {
    Resolver,
    Extraction,
    Transport,
    Output,
    Storage,
    Unknown,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureScope {
    Recording,
    Source,
    Global,
    Operation,
    Unknown,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackFailure {
    pub category: FailureCategory,
    pub stage: FailureStage,
    pub scope: FailureScope,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}
fn failure(
    category: FailureCategory,
    stage: FailureStage,
    scope: FailureScope,
    retryable: bool,
    http_status: Option<u16>,
) -> PlaybackFailure {
    PlaybackFailure {
        category,
        stage,
        scope,
        retryable,
        http_status,
    }
}
fn has_any(message: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|value| message.contains(value))
}
fn http_status(message: &str) -> Option<u16> {
    // Only explicit HTTP/status syntax counts; arbitrary title numbers do not.
    for (index, _) in message.char_indices() {
        let remaining = &message[index..];
        for prefix in [
            "http error ",
            "http status ",
            "http ",
            "status code ",
            "status ",
        ] {
            if let Some(tail) = remaining.strip_prefix(prefix) {
                let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
                if digits.len() == 3 {
                    if let Ok(status @ 400..=599) = digits.parse::<u16>() {
                        return Some(status);
                    }
                }
            }
        }
    }
    None
}
/// Compatibility classifier for legacy string errors. Unknown errors never skip a recording.
pub fn classify(reason: &str) -> PlaybackFailure {
    use FailureCategory::*;
    use FailureScope::*;
    use FailureStage as Stage;
    let message = reason.trim().to_lowercase();
    let status = http_status(&message);
    let exact_no_match = reason == "No audio results were returned by the available providers. Open the catalog source or choose another source.";
    let exact_identity = [
        "Audio results were found, but none could be verified as the requested recording and version. Open the catalog source or choose another source.",
        "Audio results were found, but the saved recording has no duration and no source provided enough verified identity evidence. Open the catalog source or choose another source.",
    ].contains(&reason);
    if exact_no_match
        || message == "no playable audio match was found"
        || message.starts_with("no playable audio match was found for ")
        || message.starts_with("no playable audio match was found.")
        || message.starts_with("no playable audio match was found:")
    {
        return failure(NoMatch, Stage::Resolver, Recording, false, None);
    }
    if exact_identity
        || [
            "the resolved audio source identity does not match this recording",
            "source identity changed during audio resolution; select it again",
            "source identity changed during inspection; select it again",
        ]
        .contains(&message.as_str())
    {
        return failure(
            RecordingIdentityMismatch,
            Stage::Resolver,
            Recording,
            false,
            None,
        );
    }
    if message == "this source has no supported audio stream" {
        return failure(UnsupportedAudio, Stage::Resolver, Recording, false, None);
    }
    if has_any(&message, &["cancel", "superseded"]) {
        return failure(Cancelled, Stage::Resolver, Operation, false, status);
    }
    if has_any(&message, &["timed out", "timeout", "deadline"]) {
        return failure(Timeout, Stage::Resolver, Operation, true, status);
    }
    if has_any(
        &message,
        &[
            "storage",
            "disk full",
            "no space left",
            "read-only file system",
            "readonly database",
            "database is locked",
            "permission denied",
            "local audio is not registered",
            "local file",
            "os error 28",
        ],
    ) {
        return failure(Storage, Stage::Storage, Global, true, status);
    }
    if has_any(
        &message,
        &[
            "output device",
            "audio output",
            "audio device",
            "audio sink",
            "cpal",
        ],
    ) {
        return failure(Output, Stage::Output, Global, true, status);
    }
    if status == Some(429) || has_any(&message, &["rate limit", "too many requests"]) {
        return failure(RateLimited, Stage::Transport, Global, true, status);
    }
    if status.is_some_and(|value| value >= 500)
        || has_any(
            &message,
            &[
                "network",
                "connection",
                "dns",
                "resolve host",
                "resolve hostname",
                "service unavailable",
                "server unavailable",
                "temporarily unavailable",
                "tls",
                "certificate",
            ],
        )
    {
        return failure(Network, Stage::Transport, Global, true, status);
    }
    if message.starts_with("audio source has no matching audio") {
        return failure(NoMatch, Stage::Extraction, Source, false, status);
    }
    if message.starts_with("audio source recording identity could not be verified") {
        return failure(
            RecordingIdentityMismatch,
            Stage::Extraction,
            Source,
            false,
            status,
        );
    }
    if has_any(
        &message,
        &["audio source is private", "audio source was removed"],
    ) {
        return failure(
            if message.contains("private") {
                SourcePrivate
            } else {
                SourceDeleted
            },
            Stage::Extraction,
            Source,
            false,
            status,
        );
    }
    if status == Some(401)
        || status == Some(403)
        || has_any(
            &message,
            &[
                "access denied",
                "access was denied",
                "forbidden",
                "sign in",
                "sign-in",
                "login required",
                "not a bot",
                "challenge",
                "expired url",
                "url expired",
            ],
        )
    {
        return failure(SourceAccess, Stage::Extraction, Source, true, status);
    }
    if has_any(
        &message,
        &[
            "audio source format is unavailable",
            "requested format is not available",
            "no video formats found",
            "no suitable formats",
        ],
    ) {
        return failure(UnsupportedAudio, Stage::Extraction, Source, true, status);
    }
    if has_any(&message, &["private video", "video is private"]) {
        let legacy_recording = ["private video", "video is private"].contains(&message.as_str());
        return failure(
            SourcePrivate,
            if legacy_recording {
                Stage::Resolver
            } else {
                Stage::Extraction
            },
            if legacy_recording { Recording } else { Source },
            false,
            status,
        );
    }
    if has_any(
        &message,
        &[
            "video has been removed",
            "video has been deleted",
            "video was deleted",
        ],
    ) {
        let legacy_recording = [
            "video has been removed",
            "video has been deleted",
            "video was deleted",
        ]
        .contains(&message.as_str());
        return failure(
            SourceDeleted,
            if legacy_recording {
                Stage::Resolver
            } else {
                Stage::Extraction
            },
            if legacy_recording { Recording } else { Source },
            false,
            status,
        );
    }
    if status == Some(404)
        || status == Some(410)
        || has_any(
            &message,
            &[
                "audio source is unavailable",
                "video unavailable",
                "source unavailable",
            ],
        )
    {
        return failure(SourceUnavailable, Stage::Transport, Source, true, status);
    }
    failure(
        FailureCategory::Unknown,
        Stage::Unknown,
        FailureScope::Unknown,
        false,
        status,
    )
}
/// Only exhausted recording outcomes advance the automatic queue.
pub fn should_skip_recording(value: &PlaybackFailure) -> bool {
    value.scope == FailureScope::Recording
        && !value.retryable
        && matches!(
            value.category,
            FailureCategory::NoMatch
                | FailureCategory::RecordingIdentityMismatch
                | FailureCategory::SourcePrivate
                | FailureCategory::SourceDeleted
                | FailureCategory::UnsupportedAudio
        )
}
/// Raw extractor failures describe one source attempt, never resolver exhaustion.
pub fn classify_extractor_failure(reason: &str) -> PlaybackFailure {
    let mut value = classify(reason);
    if matches!(
        value.category,
        FailureCategory::SourcePrivate
            | FailureCategory::SourceDeleted
            | FailureCategory::UnsupportedAudio
            | FailureCategory::NoMatch
            | FailureCategory::RecordingIdentityMismatch
    ) {
        value.scope = FailureScope::Source;
        value.stage = FailureStage::Extraction;
        if value.category == FailureCategory::UnsupportedAudio {
            value.retryable = true;
        }
    }
    value
}
/// Fixed safe messages preserve failure classes without exposing stderr/URLs/account material.
pub fn safe_extractor_error(raw: &str) -> String {
    let value = classify_extractor_failure(raw);
    use FailureCategory::*;
    let message = match value.category {
        Cancelled => "Cancelled",
        Timeout => "Audio source extraction timed out",
        Network => "Audio source network connection failed",
        RateLimited => "Audio source rate limit reached",
        SourceAccess => "Audio source access was denied; refresh or choose another source",
        SourcePrivate => "Audio source is private; choose another source",
        SourceDeleted => "Audio source was removed; choose another source",
        SourceUnavailable => "Audio source is unavailable; choose another source",
        UnsupportedAudio => "Audio source format is unavailable; refresh or choose another source",
        NoMatch => "Audio source has no matching audio; choose another source",
        RecordingIdentityMismatch => {
            "Audio source recording identity could not be verified; choose another source"
        }
        Storage => "Audio source storage operation failed",
        Output => "Audio output device failed",
        _ => "Audio source extraction failed",
    };
    match value.http_status {
        Some(status) => format!("{message} (HTTP {status})"),
        None => message.to_owned(),
    }
}
pub fn sanitize_extractor_failure(raw: &str) -> String {
    safe_extractor_error(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extractor_messages_are_safe_and_preserve_classes() {
        for raw in [
            "ERROR HTTP Error 403 https://private.invalid/stream?token=secret Authorization: Bearer secret",
            "HTTP Error 429 cookie=secret /home/user/secret",
            "network connection failed token=secret", "Private video token=secret",
            "Requested format is not available file:///private/secret", "Cancelled token=secret",
            "No playable audio match was found", "The resolved audio source identity does not match this recording",
        ] {
            let safe = safe_extractor_error(raw);
            assert!(!safe.contains("secret") && !safe.contains("private.invalid"));
            let before = classify_extractor_failure(raw);
            let after = classify(&safe);
            assert_eq!(before, after, "{raw}");
            assert!(!should_skip_recording(&after));
        }
    }
    #[test]
    fn frontend_and_native_policy_share_contract() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../src/lib/playback-failure-conformance.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let value = classify(case["reason"].as_str().unwrap());
            assert_eq!(
                serde_json::to_value(&value).unwrap(),
                case["failure"],
                "{}",
                case["reason"]
            );
            assert_eq!(
                should_skip_recording(&value),
                case["skip"].as_bool().unwrap()
            );
        }
    }
}
