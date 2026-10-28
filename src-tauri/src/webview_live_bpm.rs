//! Stateless, bounded WebView PCM request validation for the live BPM command.

use serde::Serialize;
use serde_json::Value;

pub const SAMPLE_RATE: u32 = 22_050;
pub const MIN_SECONDS: usize = 8;
pub const MAX_SECONDS: usize = 12;
const MAX_PCM_BYTES: usize = SAMPLE_RATE as usize * MAX_SECONDS * 2;
const MAX_BASE64_BYTES: usize = MAX_PCM_BYTES.div_ceil(3) * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Success,
    NoEstimate,
    OwnerMismatchBefore,
    OwnerChangedAfter,
    Disabled,
    NativeActive,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub status: OutcomeStatus,
    pub estimate: Option<Value>,
    pub key_estimate: Option<Value>,
    pub sample_window_seconds: Option<f64>,
    pub analysis_ms: Option<u64>,
}

impl Outcome {
    pub fn status(status: OutcomeStatus) -> Self {
        Self {
            status,
            estimate: None,
            key_estimate: None,
            sample_window_seconds: None,
            analysis_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub track_id: String,
    pub generation: u64,
    pub seek_serial: u64,
}

pub fn matches_playing_owner(snapshot: &Value, owner: &Owner) -> bool {
    snapshot.get("status").and_then(Value::as_str) == Some("playing")
        && snapshot.get("paused").and_then(Value::as_bool) == Some(false)
        && snapshot.get("trackId").and_then(Value::as_str) == Some(owner.track_id.as_str())
        && snapshot.get("sourceGeneration").and_then(Value::as_u64) == Some(owner.generation)
        && snapshot.get("seekSerial").and_then(Value::as_u64) == Some(owner.seek_serial)
}

pub fn decode_pcm16(sample_rate: u32, encoded: &str) -> Result<Vec<f32>, &'static str> {
    use base64::Engine as _;
    if sample_rate != SAMPLE_RATE || encoded.is_empty() || encoded.len() > MAX_BASE64_BYTES {
        return Err("WebView live BPM PCM exceeds its transport or sample-rate bound");
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "WebView live BPM PCM is malformed")?;
    let min_bytes = SAMPLE_RATE as usize * MIN_SECONDS * 2;
    if bytes.len() < min_bytes || bytes.len() > MAX_PCM_BYTES || bytes.len() % 2 != 0 {
        return Err("WebView live BPM PCM must contain 8 to 12 seconds of mono PCM16");
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn owner() -> Owner {
        Owner {
            track_id: "track-a".into(),
            generation: 8,
            seek_serial: 2,
        }
    }

    #[test]
    fn only_exact_current_unpaused_webview_owner_matches() {
        let current = json!({"status":"playing","paused":false,"trackId":"track-a","sourceGeneration":8,"seekSerial":2});
        assert!(matches_playing_owner(&current, &owner()));
        for stale in [
            json!({"status":"paused","paused":true,"trackId":"track-a","sourceGeneration":8,"seekSerial":2}),
            json!({"status":"playing","paused":false,"trackId":"track-b","sourceGeneration":8,"seekSerial":2}),
            json!({"status":"playing","paused":false,"trackId":"track-a","sourceGeneration":7,"seekSerial":2}),
            json!({"status":"playing","paused":false,"trackId":"track-a","sourceGeneration":8,"seekSerial":1}),
            json!({"status":"buffering","paused":false,"trackId":"track-a","sourceGeneration":8,"seekSerial":2}),
        ] {
            assert!(!matches_playing_owner(&stale, &owner()));
        }
    }

    #[test]
    fn accepts_only_bounded_eight_to_twelve_second_pcm16_at_fixed_rate() {
        use base64::Engine as _;
        let eight_seconds = vec![0u8; SAMPLE_RATE as usize * MIN_SECONDS * 2];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&eight_seconds);
        assert_eq!(
            decode_pcm16(SAMPLE_RATE, &encoded).unwrap().len(),
            SAMPLE_RATE as usize * MIN_SECONDS
        );
        assert!(decode_pcm16(SAMPLE_RATE - 1, &encoded).is_err());
        assert!(decode_pcm16(SAMPLE_RATE, "???").is_err());
        let too_short = base64::engine::general_purpose::STANDARD.encode([0u8; 4]);
        assert!(decode_pcm16(SAMPLE_RATE, &too_short).is_err());
        let too_long =
            base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_PCM_BYTES + 2]);
        assert!(decode_pcm16(SAMPLE_RATE, &too_long).is_err());
    }

    #[test]
    fn outcome_status_serializes_as_bounded_reason_codes() {
        assert_eq!(
            serde_json::to_value(Outcome::status(OutcomeStatus::OwnerMismatchBefore)).unwrap()
                ["status"],
            "owner_mismatch_before"
        );
        assert_eq!(
            serde_json::to_value(Outcome::status(OutcomeStatus::OwnerChangedAfter)).unwrap()
                ["status"],
            "owner_changed_after"
        );
        assert_eq!(
            serde_json::to_value(Outcome::status(OutcomeStatus::NoEstimate)).unwrap()["status"],
            "no_estimate"
        );
    }
}
