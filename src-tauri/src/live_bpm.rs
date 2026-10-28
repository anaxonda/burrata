//! Lightweight tempo estimate from the most recent native-output PCM window.
//!
//! This module is deliberately independent of catalog BPM and media tagging:
//! callers may display the estimate, but it must never be written to a Track.

use serde::Serialize;

pub const LIVE_BPM_WINDOW_SECONDS: usize = 12;
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
/// than the latest twelve seconds. No estimate is returned for short, silent, or
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
    let input_duration = samples.len() as f64 / f64::from(sample_rate) / channel_count as f64;
    if input_duration < MIN_SECONDS as f64 || input_duration > LIVE_BPM_WINDOW_SECONDS as f64 + 0.01
    {
        return None;
    }
    let frame_size = (sample_rate as usize / 100).max(1);
    let interleaved_frame_size = frame_size.checked_mul(channel_count)?;
    let complete_sample_count = samples.len() / interleaved_frame_size * interleaved_frame_size;
    let pcm = &samples[..complete_sample_count];
    let duration = pcm.len() as f64 / sample_rate as f64 / channel_count as f64;
    if duration > LIVE_BPM_WINDOW_SECONDS as f64 + 0.01 {
        return None;
    }

    let envelope = multiband_onsets(pcm, sample_rate, channel_count, frame_size);
    let frame_seconds = frame_size as f64 / f64::from(sample_rate);
    let (bpm, confidence, related_tempo_alternative_bpm) = stable_tempo(&envelope, frame_seconds)?;
    Some(LiveBpmEstimate {
        bpm,
        confidence,
        related_tempo_alternative_bpm,
        window_seconds: input_duration.min(LIVE_BPM_WINDOW_SECONDS as f64),
    })
}

// Independent band novelty makes a quieter kick audible to the detector even
// under loud high-frequency subdivisions. Measure channels separately so an
// inverted stereo channel cannot cancel the beat. No FFT or audio-thread work.
fn multiband_onsets(pcm: &[f32], rate: u32, channels: usize, frame_size: usize) -> Vec<f64> {
    let alpha = |hz: f64| 1. - (-std::f64::consts::TAU * hz / f64::from(rate)).exp();
    let low_alpha = alpha(200.);
    let high_alpha = alpha(2000.);
    // Smooth each band's instantaneous energy before reducing it to 100 Hz.
    // Otherwise high-frequency carrier beats can alias into plausible tempo
    // periods, while a frame-level contrast threshold rejects quieter drums.
    let envelope_alpha = alpha(10.);
    let mut filters = vec![[0.; 4]; channels];
    let mut energy_filters = vec![[0.; 3]; channels];
    let mut bands = [Vec::new(), Vec::new(), Vec::new()];
    for frame in pcm.chunks_exact(frame_size * channels) {
        let mut energy = [0.0_f64; 3];
        for samples in frame.chunks_exact(channels) {
            for (channel, sample) in samples.iter().enumerate() {
                let x = if sample.is_finite() {
                    f64::from(*sample).clamp(-1., 1.)
                } else {
                    0.
                };
                let f = &mut filters[channel];
                f[0] += low_alpha * (x - f[0]);
                f[1] += low_alpha * (f[0] - f[1]);
                f[2] += high_alpha * (x - f[2]);
                f[3] += high_alpha * (f[2] - f[3]);
                for (band, value) in [f[1], f[3] - f[1], x - f[3]].into_iter().enumerate() {
                    let state = &mut energy_filters[channel][band];
                    *state += envelope_alpha * (value * value - *state);
                    energy[band] += *state;
                }
            }
        }
        for (band, sum) in bands.iter_mut().zip(energy) {
            band.push((sum / frame.len() as f64).sqrt());
        }
    }
    let mut result = vec![0.; bands[0].len()];
    let total_energy: f64 = bands.iter().flatten().map(|v| v * v).sum();
    for (levels, weight) in bands.iter().zip([1.4, 1., 0.5]) {
        let energy: f64 = levels.iter().map(|v| v * v).sum();
        if energy < total_energy * 0.005 || energy < 1e-10 {
            continue;
        }
        let novelty: Vec<_> = levels
            .iter()
            .scan(0., |previous, level| {
                let onset = (level - *previous).max(0.);
                *previous = *level;
                Some(onset)
            })
            .collect();
        // RMS-normalizing every band's novelty by itself can amplify tiny
        // frame-boundary fluctuations from a steady pitched carrier until
        // they weigh as much as actual attacks. Require a meaningful transient
        // contrast relative to the band's sustained level before letting it
        // contribute periodic evidence. Sparse drum/bass attacks pass this
        // comfortably; smooth or stationary content is handled by other
        // bands if it contains genuine rhythmic transients.
        let level_rms = (energy / levels.len().max(1) as f64).sqrt();
        let mut strongest_attacks = novelty.clone();
        strongest_attacks.sort_by(f64::total_cmp);
        let attack_floor = strongest_attacks
            .get(strongest_attacks.len().saturating_sub(8))
            .copied()
            .unwrap_or(0.);
        if level_rms <= 1e-9 || attack_floor / level_rms < 0.1 {
            continue;
        }
        let scale = (novelty.iter().map(|v| v * v).sum::<f64>() / novelty.len() as f64).sqrt();
        if scale < 1e-7 {
            continue;
        }
        for (target, onset) in result.iter_mut().zip(novelty) {
            *target += weight * (onset / scale).min(8.);
        }
    }
    result
}

fn correlations(envelope: &[f64], max_lag: usize) -> Vec<f64> {
    let mean = envelope.iter().sum::<f64>() / envelope.len().max(1) as f64;
    let mut result = vec![0.; max_lag + 1];
    for (lag, score) in result.iter_mut().enumerate().skip(1) {
        if envelope.len() < lag * 6 {
            continue;
        }
        let (mut dot, mut left, mut right) = (0., 0., 0.);
        for i in lag..envelope.len() {
            let a = envelope[i] - mean;
            let b = envelope[i - lag] - mean;
            dot += a * b;
            left += a * a;
            right += b * b;
        }
        if left * right > 1e-14 {
            *score = dot / (left * right).sqrt();
        }
    }
    result
}

// Require the period to survive both earlier and later overlapping windows.
// A weak isolated peak or transient tempo change should produce no reading,
// rather than a precise-looking single-window guess.
fn stable_tempo(envelope: &[f64], frame_seconds: f64) -> Option<(f64, f64, Option<f64>)> {
    let min_lag = 30usize; // 200 BPM
    let max_lag = 100usize; // 60 BPM
    if envelope.len() < 800 {
        return None;
    }
    // Reject smooth swells/vibrato even when they repeat regularly. Ignore
    // filter warm-up and require several sharp attacks, not one loud transient.
    let body = &envelope[20..];
    let rms = (body.iter().map(|v| v * v).sum::<f64>() / body.len() as f64).sqrt();
    let mut attacks = body.to_vec();
    attacks.sort_by(f64::total_cmp);
    if rms <= 1e-9 || attacks[attacks.len() - 8] / rms < 2.5 {
        return None;
    }
    // Spread each onset across adjacent 10-ms bins so a true fractional period
    // (e.g. 37.5 bins at 160 BPM) is not unfairly weaker than its 75-bin octave.
    let smoothed: Vec<_> = envelope
        .iter()
        .enumerate()
        .map(|(i, value)| {
            0.5 * value
                + 0.25 * envelope.get(i.wrapping_sub(1)).unwrap_or(&0.)
                + 0.25 * envelope.get(i + 1).unwrap_or(&0.)
        })
        .collect();
    let envelope = smoothed.as_slice();
    let full = correlations(envelope, max_lag + 1);
    let coverage = envelope.len() * 3 / 4;
    let earlier = correlations(&envelope[..coverage], max_lag + 1);
    let later = correlations(&envelope[envelope.len() - coverage..], max_lag + 1);
    let evidence = |lag: usize| full[lag].min(earlier[lag]).min(later[lag]);
    let peaks: Vec<_> = (min_lag..=max_lag)
        .filter(|&lag| {
            full[lag] >= 0.35
                && evidence(lag) >= 0.25
                && full[lag] >= full[lag - 1]
                && full[lag] > full[lag + 1]
        })
        .collect();
    let &strongest = peaks
        .iter()
        .max_by(|&&a, &&b| evidence(a).total_cmp(&evidence(b)))?;
    // Near-tied repeating multiples don't establish a slower musical beat.
    // Prefer the shortest supported period, retaining its octave as ambiguity.
    let selected = *peaks
        .iter()
        .filter(|&&lag| evidence(lag) >= evidence(strongest) * 0.98)
        .min()?;
    let bpm_for = |lag: usize| {
        let curvature = full[lag - 1] - 2. * full[lag] + full[lag + 1];
        let shift = if curvature.abs() > 1e-12 {
            (0.5 * (full[lag - 1] - full[lag + 1]) / curvature).clamp(-0.5, 0.5)
        } else {
            0.
        };
        (60. / ((lag as f64 + shift) * frame_seconds) * 10.).round() / 10.
    };
    let alternative = peaks
        .iter()
        .copied()
        .filter(|&lag| {
            lag != selected
                && ((lag as f64 / selected as f64 - 2.).abs() < 0.07
                    || (selected as f64 / lag as f64 - 2.).abs() < 0.07)
                && evidence(lag) >= evidence(selected) * 0.80
        })
        .max_by(|&a, &b| evidence(a).total_cmp(&evidence(b)))
        .map(bpm_for);
    Some((
        bpm_for(selected),
        evidence(selected).clamp(0., 1.),
        alternative,
    ))
}

#[cfg(test)]
mod tests {
    use super::{estimate_interleaved_pcm, LIVE_BPM_WINDOW_SECONDS};

    fn qualification_chord_click_pcm(
        bpm: f64,
        seconds: usize,
        phase_seconds: f64,
        noise: f32,
    ) -> Vec<f32> {
        let sample_rate = 44_100usize;
        let channels = 2usize;
        let frame_count = sample_rate * seconds;
        let mut samples = Vec::with_capacity(frame_count * channels);
        let mut noise_state = 0x1f12_3bb5_u32;
        for frame in 0..frame_count {
            let time = frame as f64 / sample_rate as f64;
            let beat_phase = (time + phase_seconds).rem_euclid(60. / bpm);
            let chord = [261.63, 329.63, 392.00]
                .iter()
                .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
                .sum::<f64>()
                * 0.18;
            let click = if beat_phase < 0.012 {
                (std::f64::consts::TAU * 1_200.0 * beat_phase).sin()
                    * (-beat_phase * 260.0).exp()
                    * 0.55
            } else {
                0.0
            };
            noise_state = noise_state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let noise_sample = ((noise_state >> 8) as f32 / 16_777_215.0) * 2.0 - 1.0;
            let sample =
                (chord + click + f64::from(noise_sample * noise)).clamp(-0.95, 0.95) as f32;
            samples.extend([sample, sample]);
        }
        samples
    }

    // Matches the WebView observer's mono mix followed by area-average
    // resampling from the fixture's 44.1 kHz source to 22.05 kHz PCM16 lane.
    fn qualification_pcm_after_webview_resampling(stereo: &[f32]) -> Vec<f32> {
        stereo
            .chunks_exact(4)
            .map(|pair| {
                let mono = ((pair[0] + pair[1]) * 0.5 + (pair[2] + pair[3]) * 0.5) * 0.5;
                let pcm16 = (mono.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
                f32::from(pcm16) / 32_768.0
            })
            .collect()
    }

    fn sustained_chord_with_quiet_broadband_beats(bpm: f64) -> Vec<f32> {
        let sample_rate = 22_050usize;
        let seconds = LIVE_BPM_WINDOW_SECONDS;
        let mut samples = vec![0.0_f32; sample_rate * seconds];
        let beat_frames = (60.0 * sample_rate as f64 / bpm).round() as usize;
        for (frame, sample) in samples.iter_mut().enumerate() {
            let time = frame as f64 / sample_rate as f64;
            *sample = ([261.63, 329.63, 392.0]
                .iter()
                .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
                .sum::<f64>()
                * 0.22) as f32;
        }
        for beat in (0..samples.len()).step_by(beat_frames.max(1)) {
            for offset in 0..(sample_rate / 30) {
                if beat + offset >= samples.len() {
                    break;
                }
                let time = offset as f64 / sample_rate as f64;
                let noise = (((offset as u32)
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223)
                    >> 8) as f32
                    / 16_777_215.0)
                    * 2.0
                    - 1.0;
                samples[beat + offset] += 0.25 * (-time * 80.0).exp() as f32 * noise;
            }
        }
        samples
    }

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
            assert!(
                (estimate.bpm - bpm).abs() <= 1.0
                    || estimate
                        .related_tempo_alternative_bpm
                        .is_some_and(|alt| (alt - bpm).abs() <= 1.),
                "{bpm} -> {estimate:?}"
            );
            assert!(estimate.confidence >= 0.35);
            assert_eq!(estimate.window_seconds, LIVE_BPM_WINDOW_SECONDS as f64);
        }
    }

    #[test]
    fn recognizes_the_qualification_120_bpm_chord_after_webview_pcm_resampling() {
        let source = qualification_chord_click_pcm(120.0, LIVE_BPM_WINDOW_SECONDS, 0.0, 0.0);
        let pcm = qualification_pcm_after_webview_resampling(&source);
        let estimate = estimate_interleaved_pcm(&pcm, 22_050, 1).unwrap();
        assert!(
            (estimate.bpm - 120.0).abs() <= 1.0,
            "exact 120 BPM WebView fixture was estimated as {estimate:?}"
        );
    }

    #[test]
    fn stationary_chord_without_attacks_does_not_claim_a_live_tempo() {
        let sample_rate = 44_100usize;
        let mut stereo = Vec::with_capacity(sample_rate * LIVE_BPM_WINDOW_SECONDS * 2);
        let mut noise_state = 0x1f12_3bb5_u32;
        for frame in 0..sample_rate * LIVE_BPM_WINDOW_SECONDS {
            let time = frame as f64 / sample_rate as f64;
            let chord = [261.63, 329.63, 392.00]
                .iter()
                .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
                .sum::<f64>()
                * 0.18;
            noise_state = noise_state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let noise = ((noise_state >> 8) as f32 / 16_777_215.0) * 2.0 - 1.0;
            let sample = (chord + f64::from(noise * 0.008)).clamp(-0.95, 0.95) as f32;
            stereo.extend([sample, sample]);
        }
        let pcm = qualification_pcm_after_webview_resampling(&stereo);
        assert!(
            estimate_interleaved_pcm(&pcm, 22_050, 1).is_none(),
            "stationary chord/noise produced a tempo estimate"
        );
    }

    #[test]
    fn quiet_broadband_beats_survive_a_sustained_chord() {
        for bpm in [72.0, 86.0, 120.0] {
            let samples = sustained_chord_with_quiet_broadband_beats(bpm);
            let estimate = estimate_interleaved_pcm(&samples, 22_050, 1).unwrap();
            assert!(
                (estimate.bpm - bpm).abs() <= 2.0,
                "{bpm} BPM with quiet broadband beats -> {estimate:?}"
            );
        }
    }

    #[test]
    fn phase_shift_and_low_level_voice_noise_do_not_break_click_tempo() {
        for bpm in [72.0, 86.0, 96.0, 120.0] {
            for phase_seconds in [0.0, 0.137] {
                let source = qualification_chord_click_pcm(
                    bpm,
                    LIVE_BPM_WINDOW_SECONDS,
                    phase_seconds,
                    0.008,
                );
                let pcm = qualification_pcm_after_webview_resampling(&source);
                let estimate = estimate_interleaved_pcm(&pcm, 22_050, 1).unwrap();
                let alternative_matches = estimate
                    .related_tempo_alternative_bpm
                    .is_some_and(|alternative| (alternative - bpm).abs() <= 2.0);
                assert!(
                    (estimate.bpm - bpm).abs() <= 2.0 || alternative_matches,
                    "{bpm} BPM at phase {phase_seconds}s -> {estimate:?}"
                );
            }
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
        let mut tempos = [
            estimate.bpm,
            estimate.related_tempo_alternative_bpm.unwrap(),
        ];
        tempos.sort_by(f64::total_cmp);
        assert!(
            (tempos[0] - 87.).abs() <= 1. && (tempos[1] - 174.).abs() <= 1.,
            "{estimate:?}"
        );
        assert!(estimate.confidence >= 0.35);
    }

    #[test]
    fn rejects_silence_short_windows_invalid_channels_and_oversized_input() {
        assert!(estimate_interleaved_pcm(&vec![0.0; 48_000 * 2 * 10], 48_000, 2).is_none());
        assert!(estimate_interleaved_pcm(&pulse_train(120.0, 7, 48_000, 2), 48_000, 2).is_none());
        assert!(estimate_interleaved_pcm(&[], 48_000, 0).is_none());
        assert!(estimate_interleaved_pcm(&pulse_train(120.0, 13, 48_000, 2), 48_000, 2).is_none());
    }

    #[test]
    fn accepts_exact_minimum_duration_despite_partial_final_frame() {
        let estimate = estimate_interleaved_pcm(&pulse_train(60.0, 8, 22_050, 1), 22_050, 1)
            .expect("exactly eight seconds of valid PCM should be sufficient");
        assert!((estimate.bpm - 60.0).abs() <= 1.0, "{estimate:?}");
        assert_eq!(estimate.window_seconds, 8.0);
    }

    fn bass_and_hat_fixture(bpm: f64) -> Vec<f32> {
        let rate = 12000usize;
        let mut samples = vec![0f32; rate * LIVE_BPM_WINDOW_SECONDS];
        let step = (rate as f64 * 60. / bpm).round() as usize;
        for (period, amplitude, frequency, length, decay) in [
            (step, 0.3, 80., 1200, 45.),
            (step / 2, 0.85, 4500., 300, 220.),
        ] {
            for start in (0..samples.len()).step_by(period) {
                for n in 0..length {
                    if start + n >= samples.len() {
                        break;
                    }
                    let t = n as f64 / rate as f64;
                    samples[start + n] += (amplitude
                        * (-t * decay).exp()
                        * (std::f64::consts::TAU * frequency * t).sin())
                        as f32;
                }
            }
        }
        samples
    }

    fn legacy_estimate(samples: &[f32], rate: usize) -> Option<crate::media::BpmAnalysis> {
        let mut previous = 0.;
        let envelope: Vec<_> = samples
            .chunks_exact(rate / 100)
            .map(|frame| {
                let rms = (frame.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>()
                    / frame.len() as f64)
                    .sqrt();
                let onset = (rms - previous).max(0.);
                previous = rms;
                onset
            })
            .collect();
        crate::media::estimate_bpm(&envelope)
    }

    #[test]
    fn separates_bass_accents_from_louder_double_time_hats() {
        for bpm in [72., 86., 96.] {
            let samples = bass_and_hat_fixture(bpm);
            let estimate = estimate_interleaved_pcm(&samples, 12000, 1).unwrap();
            assert!((estimate.bpm - bpm).abs() < 1., "{bpm}: {estimate:?}");
            assert!(
                estimate.related_tempo_alternative_bpm.is_none(),
                "resolved bass accent: {estimate:?}"
            );
            if bpm == 86. {
                assert!(
                    legacy_estimate(&samples, 12000).unwrap().bpm > 150.,
                    "fixture must expose the old subdivision error"
                );
            }
        }
    }

    #[test]
    fn fast_fractional_period_keeps_the_supported_octave() {
        let estimate = estimate_interleaved_pcm(&bass_and_hat_fixture(160.), 12000, 1).unwrap();
        let mut tempos = [
            estimate.bpm,
            estimate.related_tempo_alternative_bpm.unwrap(),
        ];
        tempos.sort_by(f64::total_cmp);
        assert!(
            (tempos[0] - 80.).abs() < 1. && (tempos[1] - 160.).abs() < 1.,
            "{estimate:?}"
        );
    }

    #[test]
    fn smooth_periodic_swells_do_not_become_a_precise_172_bpm_claim() {
        let samples: Vec<f32> = (0..12000 * LIVE_BPM_WINDOW_SECONDS)
            .map(|i| {
                let t = i as f64 / 12000.;
                (0.5 * (1. + 0.5 * (std::f64::consts::TAU * (172. / 60.) * t).sin())
                    * (std::f64::consts::TAU * 220. * t).sin()) as f32
            })
            .collect();
        assert!(
            legacy_estimate(&samples, 12000).is_some(),
            "old detector incorrectly reads modulation as tempo"
        );
        assert!(estimate_interleaved_pcm(&samples, 12000, 1).is_none());
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
