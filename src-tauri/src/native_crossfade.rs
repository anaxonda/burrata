//! Frame-based crossfade envelope. This module does not start or own decoders;
//! queue and output ownership must be established before applying it.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FadeGains {
    pub(crate) outgoing: f32,
    pub(crate) incoming: f32,
}

/// Limit the overlap to half of each verified whole-recording duration.
/// Missing or invalid duration disables the overlap rather than guessing.
pub(crate) fn fade_frames(
    configured_seconds: u8,
    outgoing_seconds: f64,
    incoming_seconds: f64,
    sample_rate: u32,
) -> Option<u64> {
    if !(1..=12).contains(&configured_seconds)
        || sample_rate == 0
        || !outgoing_seconds.is_finite()
        || !incoming_seconds.is_finite()
        || outgoing_seconds <= 0.0
        || incoming_seconds <= 0.0
    {
        return None;
    }
    let seconds = f64::from(configured_seconds)
        .min(outgoing_seconds / 2.0)
        .min(incoming_seconds / 2.0);
    let frames = (seconds * f64::from(sample_rate)).floor();
    (frames >= 2.0 && frames <= u64::MAX as f64).then_some(frames as u64)
}

/// Complementary gains are calculated once per frame and shared by its channels.
/// Once the fade has ended, the incoming voice remains at unity.
pub(crate) fn frame_gains(frame: u64, total_frames: u64) -> FadeGains {
    let incoming = if total_frames <= 1 {
        1.0
    } else {
        (frame.min(total_frames - 1) as f64 / (total_frames - 1) as f64) as f32
    };
    FadeGains {
        outgoing: 1.0 - incoming,
        incoming,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_limits_overlap_to_half_of_both_whole_recordings() {
        assert_eq!(fade_frames(12, 6.0, 5.0, 48_000), Some(120_000));
        assert_eq!(fade_frames(5, 240.0, 180.0, 48_000), Some(240_000));
        for (seconds, outgoing, incoming, rate) in [
            (0, 10.0, 10.0, 48_000),
            (13, 10.0, 10.0, 48_000),
            (5, f64::NAN, 10.0, 48_000),
            (5, 10.0, f64::INFINITY, 48_000),
            (5, 10.0, 0.0, 48_000),
            (5, 10.0, 10.0, 0),
        ] {
            assert_eq!(fade_frames(seconds, outgoing, incoming, rate), None);
        }
    }

    #[test]
    fn gains_are_complementary_at_every_frame_and_have_exact_endpoints() {
        let frames = 48_001;
        for frame in 0..frames {
            let gains = frame_gains(frame, frames);
            assert!((gains.outgoing + gains.incoming - 1.0).abs() < 1e-6);
            assert!((0.0..=1.0).contains(&gains.outgoing));
            assert!((0.0..=1.0).contains(&gains.incoming));
        }
        assert_eq!(
            frame_gains(0, frames),
            FadeGains {
                outgoing: 1.0,
                incoming: 0.0
            }
        );
        assert_eq!(
            frame_gains(frames - 1, frames),
            FadeGains {
                outgoing: 0.0,
                incoming: 1.0
            }
        );
        assert_eq!(
            frame_gains(frames + 20, frames),
            FadeGains {
                outgoing: 0.0,
                incoming: 1.0
            }
        );
    }
}

/// One decoded voice supplies complete interleaved frames. Implementations own
/// their source-specific normalization and analysis tap, before summation.
pub(crate) trait Voice {
    /// Whether at least the requested number of complete PCM frames are ready.
    fn buffered_frames(&mut self) -> usize;
    /// Read one whole frame; `None` means the voice has ended or failed.
    fn read_frame(&mut self, frame: &mut [f32]) -> Option<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransitionEvent {
    IncomingStarted,
    Finished,
    IncomingFailed,
}

/// Mixes two already-decoded voices. The caller must only construct this after
/// queue ownership, both verified durations, and source eligibility are known.
/// Progress is advanced by calls to `next_frame`, so pausing the output naturally
/// freezes both voices and their envelope.
pub(crate) struct TransitionMixer<A: Voice, B: Voice> {
    outgoing: A,
    incoming: B,
    channels: usize,
    frames: u64,
    frame: u64,
    started: bool,
    done: bool,
    incoming_failed: bool,
    incoming_buffer: Vec<f32>,
}

impl<A: Voice, B: Voice> TransitionMixer<A, B> {
    pub(crate) fn new(outgoing: A, incoming: B, channels: usize, frames: u64) -> Option<Self> {
        if channels == 0 || frames < 2 || frames > usize::MAX as u64 {
            return None;
        }
        Some(Self {
            outgoing,
            incoming,
            channels,
            frames,
            frame: 0,
            started: false,
            done: false,
            incoming_failed: false,
            incoming_buffer: vec![0.0; channels],
        })
    }

    /// Returns the output frame and an ownership event, if any. No incoming
    /// sample is consumed until the entire overlap is buffered.
    pub(crate) fn next_frame(&mut self, output: &mut [f32]) -> (bool, Option<TransitionEvent>) {
        assert_eq!(output.len(), self.channels);
        if self.done {
            return (false, None);
        }
        if self.incoming_failed {
            return (self.outgoing.read_frame(output).is_some(), None);
        }
        if !self.started && self.incoming.buffered_frames() < self.frames as usize {
            return (self.outgoing.read_frame(output).is_some(), None);
        }
        let incoming = self
            .incoming
            .read_frame(&mut self.incoming_buffer)
            .is_some();
        if !incoming {
            self.incoming_failed = true;
            return (
                self.outgoing.read_frame(output).is_some(),
                Some(TransitionEvent::IncomingFailed),
            );
        }
        let outgoing = self.outgoing.read_frame(output).is_some();
        let first = !self.started;
        self.started = true;
        if !outgoing {
            output.copy_from_slice(&self.incoming_buffer);
            self.done = true;
            return (true, Some(TransitionEvent::Finished));
        }
        let gains = frame_gains(self.frame, self.frames);
        for (output_sample, incoming_sample) in output.iter_mut().zip(&self.incoming_buffer) {
            *output_sample = *output_sample * gains.outgoing + *incoming_sample * gains.incoming;
        }
        self.frame += 1;
        if self.frame >= self.frames {
            self.done = true;
            (true, Some(TransitionEvent::Finished))
        } else {
            (true, first.then_some(TransitionEvent::IncomingStarted))
        }
    }

    pub(crate) fn progress(&self) -> f64 {
        if self.done {
            1.0
        } else {
            (self.frame as f64 / self.frames.max(1) as f64).clamp(0.0, 1.0)
        }
    }

    pub(crate) fn into_voices(self) -> (A, B) {
        (self.outgoing, self.incoming)
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;
    use std::collections::VecDeque;

    struct Frames(VecDeque<[f32; 2]>);
    impl Voice for Frames {
        fn buffered_frames(&mut self) -> usize {
            self.0.len()
        }
        fn read_frame(&mut self, output: &mut [f32]) -> Option<()> {
            output.copy_from_slice(&self.0.pop_front()?);
            Some(())
        }
    }
    fn voice(frames: &[[f32; 2]]) -> Frames {
        Frames(frames.iter().copied().collect())
    }

    #[test]
    fn mixes_complementary_stereo_frames_and_reports_one_start() {
        let outgoing = voice(&[[1.0, 0.5]; 3]);
        let incoming = voice(&[[0.0, 1.0]; 3]);
        let mut mixer = TransitionMixer::new(outgoing, incoming, 2, 3).unwrap();
        let mut frame = [0.0; 2];
        assert_eq!(
            mixer.next_frame(&mut frame),
            (true, Some(TransitionEvent::IncomingStarted))
        );
        assert_eq!(frame, [1.0, 0.5]);
        assert_eq!(mixer.next_frame(&mut frame), (true, None));
        assert_eq!(frame, [0.5, 0.75]);
        assert_eq!(
            mixer.next_frame(&mut frame),
            (true, Some(TransitionEvent::Finished))
        );
        assert_eq!(frame, [0.0, 1.0]);
    }

    #[test]
    fn outgoing_ending_during_overlap_keeps_incoming_playable() {
        let outgoing = voice(&[[1.0, 1.0]; 1]);
        let incoming = voice(&[[0.25, 0.5]; 3]);
        let mut mixer = TransitionMixer::new(outgoing, incoming, 2, 3).unwrap();
        let mut frame = [0.0; 2];
        assert_eq!(
            mixer.next_frame(&mut frame),
            (true, Some(TransitionEvent::IncomingStarted))
        );
        assert_eq!(
            mixer.next_frame(&mut frame),
            (true, Some(TransitionEvent::Finished))
        );
        assert_eq!(frame, [0.25, 0.5]);
        let (_, mut incoming) = mixer.into_voices();
        assert_eq!(incoming.read_frame(&mut frame), Some(()));
        assert_eq!(frame, [0.25, 0.5]);
    }

    #[test]
    fn incoming_failure_keeps_the_outgoing_tail_at_unity() {
        struct FailedVoice;
        impl Voice for FailedVoice {
            fn buffered_frames(&mut self) -> usize {
                3
            }
            fn read_frame(&mut self, _: &mut [f32]) -> Option<()> {
                None
            }
        }
        let outgoing = voice(&[[0.8, 0.4]; 3]);
        let mut mixer = TransitionMixer::new(outgoing, FailedVoice, 2, 3).unwrap();
        let mut frame = [0.0; 2];
        assert_eq!(
            mixer.next_frame(&mut frame),
            (true, Some(TransitionEvent::IncomingFailed))
        );
        assert_eq!(frame, [0.8, 0.4]);
        assert_eq!(mixer.next_frame(&mut frame), (true, None));
        assert_eq!(frame, [0.8, 0.4]);
    }

    #[test]
    fn not_pulling_while_paused_preserves_both_voices_and_envelope() {
        let outgoing = voice(&[[1.0, 1.0]; 3]);
        let incoming = voice(&[[0.0, 0.0]; 3]);
        let mut mixer = TransitionMixer::new(outgoing, incoming, 2, 3).unwrap();
        let mut frame = [0.0; 2];
        assert_eq!(
            mixer.next_frame(&mut frame).1,
            Some(TransitionEvent::IncomingStarted)
        );
        // A pause means no source pull; the next frame resumes at the midpoint.
        assert_eq!(mixer.next_frame(&mut frame), (true, None));
        assert_eq!(frame, [0.5, 0.5]);
    }

    #[test]
    fn insufficient_incoming_buffer_keeps_outgoing_unfaded_and_unclaimed() {
        let outgoing = voice(&[[1.0, 0.5]; 4]);
        let incoming = voice(&[[0.0, 1.0]; 2]);
        let mut mixer = TransitionMixer::new(outgoing, incoming, 2, 3).unwrap();
        let mut frame = [0.0; 2];
        assert_eq!(mixer.next_frame(&mut frame), (true, None));
        assert_eq!(frame, [1.0, 0.5]);
        let (_, mut incoming) = mixer.into_voices();
        assert_eq!(incoming.buffered_frames(), 2);
    }
}
