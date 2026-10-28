//! Provisional key estimate from the recent, owner-bound playback PCM window.
//!
//! This intentionally does not run the full-recording five-window consensus
//! and never writes catalog metadata or tags.

use serde::Serialize;

pub const LIVE_KEY_SAMPLE_RATE: u32 = 22_050;
pub const LIVE_KEY_MIN_SECONDS: usize = 8;
pub const LIVE_KEY_MAX_SECONDS: usize = 12;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveKeyEstimate {
    pub key: String,
    /// LibKeyFinder profile fit; this is not a calibrated probability.
    pub profile_fit: f64,
    /// Difference between the best and runner-up profile fits.
    pub profile_margin: f64,
    pub window_seconds: f64,
}

/// Classify no more than twelve seconds of interleaved playback PCM. A single
/// qualified profile fit is reported as a provisional live key; callers must
/// keep it separate from full-recording key metadata.
pub fn estimate_interleaved_pcm(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Option<LiveKeyEstimate> {
    if sample_rate < 100 || channels == 0 || channels > 32 {
        return None;
    }
    let channel_count = usize::from(channels);
    let input_duration = samples.len() as f64 / f64::from(sample_rate) / channel_count as f64;
    if !(LIVE_KEY_MIN_SECONDS as f64..=LIVE_KEY_MAX_SECONDS as f64 + 0.01).contains(&input_duration)
    {
        return None;
    }
    let mut downsampler =
        crate::stream_analysis::MonoDownsampler::new(sample_rate, LIVE_KEY_SAMPLE_RATE, channels)
            .ok()?;
    downsampler.push_interleaved(samples).ok()?;
    let output = downsampler.output();
    let window_seconds = output.len() as f64 / f64::from(LIVE_KEY_SAMPLE_RATE);
    if !(LIVE_KEY_MIN_SECONDS as f64..=LIVE_KEY_MAX_SECONDS as f64 + 0.01).contains(&window_seconds)
    {
        return None;
    }
    let (key, profile_fit, profile_margin) = crate::media::classify_live_key_window(output)?;
    Some(LiveKeyEstimate {
        key,
        profile_fit,
        profile_margin,
        window_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_window_uses_the_existing_qualified_key_profile() {
        let sample_rate = LIVE_KEY_SAMPLE_RATE as usize;
        let samples = (0..sample_rate * 10)
            .map(|index| {
                let time = index as f64 / sample_rate as f64;
                ([220.0, 261.6256, 329.6276]
                    .iter()
                    .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
                    .sum::<f64>()
                    / 3.0) as f32
            })
            .collect::<Vec<_>>();

        let estimate = estimate_interleaved_pcm(&samples, LIVE_KEY_SAMPLE_RATE, 1)
            .expect("the qualified A-minor window should produce a live key");
        assert_eq!(estimate.key, "Am");
        assert!(estimate.profile_fit >= 0.20);
        assert!(estimate.profile_margin >= 0.01);
        assert!((estimate.window_seconds - 10.0).abs() < 0.01);
    }

    #[test]
    fn short_silent_or_oversized_windows_have_no_live_key() {
        let rate = LIVE_KEY_SAMPLE_RATE as usize;
        assert!(estimate_interleaved_pcm(&vec![0.2; rate * 7], LIVE_KEY_SAMPLE_RATE, 1).is_none());
        assert!(estimate_interleaved_pcm(&vec![0.0; rate * 10], LIVE_KEY_SAMPLE_RATE, 1).is_none());
        assert!(estimate_interleaved_pcm(&vec![0.2; rate * 13], LIVE_KEY_SAMPLE_RATE, 1).is_none());
    }
}
