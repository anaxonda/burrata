//! Privacy-bounded current playback context for diagnostic exports.
use serde_json::{json, Map, Value};

const MAX_TIMING_MS: u64 = 3_600_000;
const MAX_COUNTER: u64 = 1_000_000;

/// Project a normal playback snapshot onto explicitly safe scalar diagnostics.
/// Track/candidate objects, free text, identifiers, URLs and arbitrary quality
/// metadata are deliberately never copied.
pub(crate) fn project(snapshot: &Value, target_triple: &str) -> Value {
    let engine = enum_value(snapshot.get("engine"), &["native", "webview"]);
    let status = enum_value(
        snapshot.get("status"),
        &[
            "idle",
            "playing",
            "paused",
            "buffering",
            "ended",
            "failed",
            "stopped",
        ],
    );
    let diagnostics = snapshot.get("diagnostics").and_then(Value::as_object);
    let decoder_kind = diagnostics
        .and_then(|d| d.get("decoder"))
        .and_then(Value::as_object)
        .and_then(|d| d.get("decoderKind"))
        .and_then(|v| enum_value(Some(v), &["rodio", "ffmpeg", "experimental_opus", "other"]));

    let mut context = Map::new();
    context.insert("targetTriple".into(), json!(safe_target(target_triple)));
    if let Some(value) = engine {
        context.insert("engine".into(), json!(value));
    }
    if let Some(value) = status {
        context.insert("status".into(), json!(value));
    }
    if let Some(value) = decoder_kind {
        context.insert("decoderKind".into(), json!(value));
    }

    for name in ["firstAudioMs", "sourceReadyMs", "seekFirstAudioMs"] {
        if let Some(value) = diagnostics.and_then(|d| bounded_u64(d.get(name), MAX_TIMING_MS)) {
            context.insert(name.into(), json!(value));
        }
    }
    for name in ["recoveryCount", "bufferingCount"] {
        if let Some(value) = diagnostics.and_then(|d| bounded_u64(d.get(name), MAX_COUNTER)) {
            context.insert(name.into(), json!(value));
        }
    }
    Value::Object(context)
}

fn enum_value<'a>(value: Option<&'a Value>, allowed: &[&'static str]) -> Option<&'static str> {
    let value = value?.as_str()?;
    allowed.iter().copied().find(|item| *item == value)
}

fn bounded_u64(value: Option<&Value>, maximum: u64) -> Option<u64> {
    let value = value?.as_u64()?;
    (value <= maximum).then_some(value)
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

#[cfg(test)]
mod tests {
    use super::*;

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
                "decoderKind":"ffmpeg", "firstAudioMs":321,"sourceReadyMs":654,
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
}
