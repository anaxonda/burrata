//! Keep fractional resampling phase across decoder packets. Rodio's uniform
//! iterator starts a new rate converter for each Source span (and every 32768
//! samples), rounding each span independently and making audio lag its clock.
use rodio::{
    conversions::{ChannelCountConverter, SampleRateConverter},
    Source,
};
use std::num::{NonZeroU16, NonZeroU32};

type Converted<I> = ChannelCountConverter<SampleRateConverter<FormatRun<I>>>;

pub(super) struct ContinuousSourceIterator<I: Source> {
    inner: Option<Converted<I>>,
    channels: NonZeroU16,
    rate: NonZeroU32,
}

impl<I: Source> ContinuousSourceIterator<I> {
    pub(super) fn new(input: I, channels: NonZeroU16, rate: NonZeroU32) -> Self {
        Self {
            inner: Some(Self::convert(input, None, channels, rate)),
            channels,
            rate,
        }
    }

    fn convert(
        input: I,
        pending: Option<f32>,
        channels: NonZeroU16,
        rate: NonZeroU32,
    ) -> Converted<I> {
        let from_channels = input.channels();
        let from_rate = input.sample_rate();
        let run = FormatRun {
            input,
            pending,
            ended: false,
            channels: from_channels,
            rate: from_rate,
        };
        ChannelCountConverter::new(
            SampleRateConverter::new(run, from_rate, rate, from_channels),
            from_channels,
            channels,
        )
    }
}

impl<I: Source> Iterator for ContinuousSourceIterator<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(sample) = self.inner.as_mut()?.next() {
            return Some(sample);
        }
        let run = self.inner.take()?.into_inner().into_inner();
        // Only a real sample-rate/channel change ends a run. Decoder packet
        // boundaries do not. Preserve the first sample that revealed the change.
        let pending = run.pending?;
        let mut converted = Self::convert(run.input, Some(pending), self.channels, self.rate);
        let sample = converted.next();
        self.inner = Some(converted);
        sample
    }
}

struct FormatRun<I: Source> {
    input: I,
    pending: Option<f32>,
    ended: bool,
    channels: NonZeroU16,
    rate: NonZeroU32,
}

impl<I: Source> Iterator for FormatRun<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.ended {
            return None;
        }
        if let Some(sample) = self.pending.take() {
            return Some(sample);
        }
        let Some(sample) = self.input.next() else {
            self.ended = true;
            return None;
        };
        if self.input.channels() != self.channels || self.input.sample_rate() != self.rate {
            self.pending = Some(sample);
            self.ended = true;
            None
        } else {
            Some(sample)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    struct Packets {
        sample: usize,
        total: usize,
        rate: NonZeroU32,
        channels: NonZeroU16,
        packet: usize,
    }
    impl Iterator for Packets {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            if self.sample >= self.total {
                return None;
            }
            self.sample += 1;
            Some(0.25)
        }
    }
    impl Source for Packets {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.packet.min(self.total - self.sample))
        }
        fn channels(&self) -> NonZeroU16 {
            self.channels
        }
        fn sample_rate(&self) -> NonZeroU32 {
            self.rate
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }
    #[test]
    fn packet_size_does_not_accumulate_clock_drift_or_change_channel_count() {
        for (from, to, channels, target_channels) in [
            (44100, 48000, 1, 2),
            (48000, 44100, 2, 1),
            (44100, 44100, 2, 2),
        ] {
            for packet in [1024, 1152, 4096, 32768] {
                let source = Packets {
                    sample: 0,
                    total: from as usize * channels as usize * 60,
                    rate: NonZeroU32::new(from).unwrap(),
                    channels: NonZeroU16::new(channels).unwrap(),
                    packet,
                };
                let mut converted = ContinuousSourceIterator::new(
                    source,
                    NonZeroU16::new(target_channels).unwrap(),
                    NonZeroU32::new(to).unwrap(),
                );
                let count = converted
                    .by_ref()
                    .inspect(|sample| assert!((*sample - 0.25).abs() < 1e-6))
                    .count();
                let expected = to as usize * target_channels as usize * 60;
                assert!(count.abs_diff(expected)<=target_channels as usize*2,"{from}->{to}, {channels}->{target_channels}, packet {packet}: {count} vs {expected}");
                assert_eq!(converted.next(), None);
                assert_eq!(converted.next(), None);
            }
        }
    }

    #[test]
    fn real_format_change_preserves_first_new_frame_and_remains_fused_at_eof() {
        struct Changing {
            next: usize,
            second: bool,
        }
        impl Iterator for Changing {
            type Item = f32;
            fn next(&mut self) -> Option<f32> {
                if self.next >= 1500 {
                    return None;
                }
                self.second = self.next >= 500;
                self.next += 1;
                Some(if self.second { 0.75 } else { 0.25 })
            }
        }
        impl Source for Changing {
            fn current_span_len(&self) -> Option<usize> {
                Some(100)
            }
            fn channels(&self) -> NonZeroU16 {
                NonZeroU16::new(if self.second { 2 } else { 1 }).unwrap()
            }
            fn sample_rate(&self) -> NonZeroU32 {
                NonZeroU32::new(if self.second { 2000 } else { 1000 }).unwrap()
            }
            fn total_duration(&self) -> Option<Duration> {
                None
            }
        }
        let mut converted = ContinuousSourceIterator::new(
            Changing {
                next: 0,
                second: false,
            },
            NonZeroU16::new(1).unwrap(),
            NonZeroU32::new(1000).unwrap(),
        );
        let samples = converted.by_ref().collect::<Vec<_>>();
        assert!(
            samples.len().abs_diff(750) <= 2,
            "{} samples",
            samples.len()
        );
        assert_eq!(samples[0], 0.25);
        assert_eq!(*samples.last().unwrap(), 0.75);
        let first_new = samples.iter().position(|sample| *sample == 0.75).unwrap();
        assert!(
            first_new.abs_diff(500) <= 2,
            "format boundary at {first_new}"
        );
        assert_eq!(converted.next(), None);
        assert_eq!(converted.next(), None);
    }
}
