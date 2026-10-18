use base64::{engine::general_purpose::STANDARD, Engine as _};

pub const SAMPLE_RATE: u32 = 48_000;
pub const MIN_SECONDS: usize = 8;
pub const MAX_SECONDS: usize = 12;
pub const MAX_WAV_BYTES: usize = 44 + SAMPLE_RATE as usize * MAX_SECONDS * 2;
const MAX_BASE64_BYTES: usize = ((MAX_WAV_BYTES + 2) / 3) * 4;

pub fn decode_bounded_wav(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.is_empty() || encoded.len() > MAX_BASE64_BYTES {
        return Err("Captured audio exceeds the twelve-second upload limit".into());
    }
    let wav = STANDARD
        .decode(encoded)
        .map_err(|_| "Captured audio is not valid base64")?;
    validate_wav(&wav)?;
    Ok(wav)
}

pub fn validate_wav(wav: &[u8]) -> Result<(), String> {
    if wav.len() < 44 + SAMPLE_RATE as usize * MIN_SECONDS * 2 || wav.len() > MAX_WAV_BYTES {
        return Err("Captured audio must contain eight to twelve seconds".into());
    }
    if &wav[0..4] != b"RIFF"
        || &wav[8..12] != b"WAVE"
        || &wav[12..16] != b"fmt "
        || &wav[36..40] != b"data"
    {
        return Err("Captured audio must be a PCM WAV file".into());
    }
    let u16_at = |at| u16::from_le_bytes([wav[at], wav[at + 1]]);
    let u32_at = |at| u32::from_le_bytes([wav[at], wav[at + 1], wav[at + 2], wav[at + 3]]);
    let data_len = u32_at(40) as usize;
    if u32_at(4) as usize + 8 != wav.len()
        || u32_at(16) != 16
        || u16_at(20) != 1
        || u16_at(22) != 1
        || u32_at(24) != SAMPLE_RATE
        || u32_at(28) != SAMPLE_RATE * 2
        || u16_at(32) != 2
        || u16_at(34) != 16
        || data_len + 44 != wav.len()
        || data_len % 2 != 0
        || data_len < SAMPLE_RATE as usize * MIN_SECONDS * 2
        || data_len > SAMPLE_RATE as usize * MAX_SECONDS * 2
    {
        return Err("Captured audio WAV has an unsupported sample format or duration".into());
    }
    Ok(())
}

pub fn snapshot_matches(
    snapshot: &serde_json::Value,
    track_id: &str,
    session_id: &str,
    generation: u64,
    seek_serial: u64,
    chapter_id: Option<&str>,
) -> bool {
    snapshot["status"].as_str() == Some("playing")
        && snapshot["trackId"].as_str() == Some(track_id)
        && snapshot["sessionId"].as_str() == Some(session_id)
        && snapshot["sourceGeneration"].as_u64() == Some(generation)
        && snapshot["seekSerial"].as_u64() == Some(seek_serial)
        && snapshot
            .get("chapterId")
            .and_then(serde_json::Value::as_str)
            == chapter_id
        && matches!(
            snapshot["previewKind"].as_str(),
            Some("audio" | "local" | "direct")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(seconds: usize) -> Vec<u8> {
        let data_len = SAMPLE_RATE as usize * seconds * 2;
        let mut bytes = vec![0; 44 + data_len];
        let riff_len = (bytes.len() - 8) as u32;
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes[12..16].copy_from_slice(b"fmt ");
        bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
        bytes[20..22].copy_from_slice(&1u16.to_le_bytes());
        bytes[22..24].copy_from_slice(&1u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes[28..32].copy_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        bytes[32..34].copy_from_slice(&2u16.to_le_bytes());
        bytes[34..36].copy_from_slice(&16u16.to_le_bytes());
        bytes[36..40].copy_from_slice(b"data");
        bytes[40..44].copy_from_slice(&(data_len as u32).to_le_bytes());
        bytes
    }

    #[test]
    fn accepts_only_bounded_mono_48khz_pcm_wav() {
        assert!(validate_wav(&wav(8)).is_ok());
        assert!(validate_wav(&wav(12)).is_ok());
        assert!(validate_wav(&wav(7)).is_err());
        assert!(validate_wav(&wav(13)).is_err());
        let mut stereo = wav(8);
        stereo[22..24].copy_from_slice(&2u16.to_le_bytes());
        assert!(validate_wav(&stereo).is_err());
        let mut rate = wav(8);
        rate[24..28].copy_from_slice(&44_100u32.to_le_bytes());
        assert!(validate_wav(&rate).is_err());
        let mut trailing = wav(8);
        trailing.push(0);
        assert!(validate_wav(&trailing).is_err());
    }

    #[test]
    fn rejects_oversize_or_malformed_base64_before_recognition() {
        assert!(decode_bounded_wav("!").is_err());
        assert!(decode_bounded_wav(&"A".repeat(MAX_BASE64_BYTES + 1)).is_err());
        assert!(decode_bounded_wav(&STANDARD.encode(wav(8))).is_ok());
    }

    #[test]
    fn playback_identity_must_match_station_session_source_and_seek() {
        let snapshot = serde_json::json!({"status":"playing", "trackId":"radio:main", "sessionId":"s1", "sourceGeneration":4, "seekSerial":2, "chapterId":"chapter-a", "previewKind":"audio"});
        assert!(snapshot_matches(
            &snapshot,
            "radio:main",
            "s1",
            4,
            2,
            Some("chapter-a")
        ));
        assert!(!snapshot_matches(
            &snapshot,
            "radio:backup",
            "s1",
            4,
            2,
            Some("chapter-a")
        ));
        assert!(!snapshot_matches(
            &snapshot,
            "radio:main",
            "s0",
            4,
            2,
            Some("chapter-a")
        ));
        assert!(!snapshot_matches(
            &snapshot,
            "radio:main",
            "s1",
            3,
            2,
            Some("chapter-a")
        ));
        assert!(!snapshot_matches(
            &snapshot,
            "radio:main",
            "s1",
            4,
            1,
            Some("chapter-a")
        ));
        assert!(!snapshot_matches(&snapshot, "radio:main", "s1", 4, 2, Some("chapter-b")), "a natural chapter transition invalidates an in-flight capture even when track/session/source/seek are stable");
        let no_chapter = serde_json::json!({"status":"playing", "trackId":"radio:main", "sessionId":"s1", "sourceGeneration":4, "seekSerial":2, "previewKind":"audio"});
        assert!(snapshot_matches(
            &no_chapter,
            "radio:main",
            "s1",
            4,
            2,
            None
        ));
    }
}
