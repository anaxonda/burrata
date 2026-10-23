//! Lightweight tempo estimate from the most recent native-output PCM window.
//!
//! This module is deliberately independent of catalog BPM and media tagging:
//! callers may display the estimate, but it must never be written to a Track.

use serde::Serialize;

pub const LIVE_BPM_WINDOW_SECONDS: usize = 10;
const FRAME_SECONDS: f64 = 0.01;
const MIN_SECONDS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBpmEstimate {
    pub bpm: f64,
    pub confidence: f64,
    pub related_tempo_alternative_bpm: Option<f64>,
    pub window_seconds: f64,
}

/// Estimate tempo from interleaved float PCM. Samples must be from the
/// playback output rate (after resampling), and the caller should pass no more
/// than the latest ten seconds. No estimate is returned for short, silent, or
/// weakly periodic material.
pub fn estimate_interleaved_pcm(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Option<LiveBpmEstimate> {
    if sample_rate < 100 || channels == 0 {
        return None;
    }
    let channel_count = usize::from(channels);
    let frame_size = (sample_rate as usize / 100).max(1);
    let interleaved_frame_size = frame_size.checked_mul(channel_count)?;
    let complete_sample_count = samples.len() / interleaved_frame_size * interleaved_frame_size;
    let pcm = &samples[..complete_sample_count];
    let duration = pcm.len() as f64 / sample_rate as f64 / channel_count as f64;
    if duration < MIN_SECONDS as f64 || duration > LIVE_BPM_WINDOW_SECONDS as f64 + FRAME_SECONDS {
        return None;
    }

    let mut envelope = Vec::with_capacity(pcm.len() / interleaved_frame_size);
    let mut previous_level = 0.0_f64;
    for frame in pcm.chunks_exact(interleaved_frame_size) {
        // Measure energy across channels rather than summing/downmixing them;
        // this avoids losing the beat when stereo channels are out of phase.
        let energy = frame
            .iter()
            .map(|sample| {
                let value = if sample.is_finite() {
                    f64::from(*sample).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                value * value
            })
            .sum::<f64>()
            / frame.len() as f64;
        let level = energy.sqrt();
        envelope.push((level - previous_level).max(0.0));
        previous_level = level;
    }

    // Reuse the catalog analyzer's confidence thresholds and its handling of
    // half/double-time ambiguity; only the PCM-to-envelope front end differs.
    let estimate = crate::media::estimate_bpm(&envelope)?;
    Some(LiveBpmEstimate {
        bpm: estimate.bpm,
        confidence: estimate.confidence,
        related_tempo_alternative_bpm: estimate.related_tempo_bpm,
        window_seconds: duration.min(LIVE_BPM_WINDOW_SECONDS as f64),
    })
}

#[cfg(test)]
mod tests {
    use super::{estimate_interleaved_pcm, LIVE_BPM_WINDOW_SECONDS};

    fn pulse_train(bpm: f64, seconds: usize, sample_rate: u32, channels: u16) -> Vec<f32> {
        let frames = seconds * sample_rate as usize;
        let mut samples = vec![0.0_f32; frames * usize::from(channels)];
        let beat_frames = (60.0 * sample_rate as f64 / bpm).round() as usize;
        for frame in (0..frames).step_by(beat_frames.max(1)) {
            // A short decaying pulse gives each beat measurable energy without
            // needing a fixture file or external audio decoder.
            for offset in 0..(sample_rate as usize / 100).max(1) {
                let at = frame.saturating_add(offset);
                if at >= frames {
                    break;
                }
                let value =
                    (0.8 * (1.0 - offset as f32 / (sample_rate as f32 / 100.0).max(1.0))).max(0.0);
                for channel in 0..usize::from(channels) {
                    samples[at * usize::from(channels) + channel] = if channels == 2 && channel == 1
                    {
                        -value
                    } else {
                        value
                    };
                }
            }
        }
        samples
    }

    #[test]
    fn recognizes_common_tempos_from_stereo_pcm_even_when_channels_are_inverted() {
        for bpm in [60.0, 90.0, 120.0] {
            let samples = pulse_train(bpm, LIVE_BPM_WINDOW_SECONDS, 48_000, 2);
            let estimate = estimate_interleaved_pcm(&samples, 48_000, 2).unwrap();
            assert!((estimate.bpm - bpm).abs() <= 1.0, "{bpm} -> {estimate:?}");
            assert!(estimate.confidence >= 0.35);
            assert_eq!(estimate.window_seconds, 10.0);
        }
    }

    #[test]
    fn unaccented_174_bpm_pulse_train_does_not_claim_octave_identity() {
        // A metronomic pulse train has no accent or phrase information to say
        // whether each pulse is a beat at 174 BPM or an eighth-note pulse at
        // 87 BPM. The analyzer's 87 BPM reading is therefore a valid tempo
        // interpretation; don't force the generator's pulse rate into the UI.
        let samples = pulse_train(174.0, LIVE_BPM_WINDOW_SECONDS, 48_000, 2);
        let estimate = estimate_interleaved_pcm(&samples, 48_000, 2).unwrap();
        assert!((estimate.bpm - 87.0).abs() <= 1.0, "{estimate:?}");
        assert!(estimate.confidence >= 0.35);
        assert_eq!(estimate.related_tempo_alternative_bpm, None);
    }

    #[test]
    fn rejects_silence_short_windows_invalid_channels_and_oversized_input() {
        assert!(estimate_interleaved_pcm(&vec![0.0; 48_000 * 2 * 10], 48_000, 2).is_none());
        assert!(estimate_interleaved_pcm(&pulse_train(120.0, 7, 48_000, 2), 48_000, 2).is_none());
        assert!(estimate_interleaved_pcm(&[], 48_000, 0).is_none());
        assert!(estimate_interleaved_pcm(&pulse_train(120.0, 11, 48_000, 2), 48_000, 2).is_none());
    }

    #[test]
    fn serialization_uses_display_contract_and_does_not_contain_track_metadata() {
        let estimate =
            estimate_interleaved_pcm(&pulse_train(120.0, 10, 48_000, 2), 48_000, 2).unwrap();
        let value = serde_json::to_value(estimate).unwrap();
        assert_eq!(value["bpm"], 120.0);
        assert_eq!(value["windowSeconds"], 10.0);
        assert!(value.get("track").is_none());
        assert!(value.get("trackId").is_none());
    }
}
