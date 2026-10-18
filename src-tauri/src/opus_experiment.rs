//! Optional experimental Opus source for the native decoder pipeline.
//!
//! This module is deliberately feature-gated. Production playback continues to
//! use Rodio's decoders by default; this adapter is only considered for streams
//! whose initial Ogg/WebM bytes contain an OpusHead marker.

use rodio::Source;
use std::{
    io::{self, Read, Seek, SeekFrom},
    num::{NonZeroU16, NonZeroU32},
    sync::{Arc, Mutex},
    time::Duration,
};
use symphonia_experiment::{
    core::{
        audio::SampleBuffer,
        codecs::{CodecParameters, CodecRegistry, Decoder, DecoderOptions, CODEC_TYPE_OPUS},
        errors::Error as SymphoniaError,
        formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
        io::{MediaSource, MediaSourceStream},
        meta::MetadataOptions,
        probe::Hint,
        units::{Time, TimeBase},
    },
    default::get_probe,
};
use symphonia_opus_adapter::OpusDecoder;

struct SeekableMediaSource<R> {
    reader: R,
    byte_len: Option<u64>,
}

impl<R: Read + Seek + Send + Sync> Read for SeekableMediaSource<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl<R: Read + Seek + Send + Sync> Seek for SeekableMediaSource<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.reader.seek(position)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for SeekableMediaSource<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }
}

pub struct OpusSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    codec_params: CodecParameters,
    samples: Vec<f32>,
    sample_index: usize,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: Option<Duration>,
    time_base: Option<TimeBase>,
    seek_target_seconds: Option<f64>,
    failure: Arc<Mutex<Option<String>>>,
    exhausted: bool,
    decoded_any_packet: bool,
}

impl OpusSource {
    pub fn open<R>(reader: R, byte_len: Option<u64>) -> Result<Self, String>
    where
        R: Read + Seek + Send + Sync + 'static,
    {
        let source = SeekableMediaSource { reader, byte_len };
        let stream = MediaSourceStream::new(Box::new(source), Default::default());
        let probed = get_probe()
            .format(
                &Hint::new(),
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|error| format!("Opus source probe failed: {error}"))?;
        let format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| "Opus source has no default audio track".to_owned())?;
        if track.codec_params.codec != CODEC_TYPE_OPUS {
            return Err("experimental Opus source did not contain an Opus track".into());
        }
        let track_id = track.id;
        let params = track.codec_params.clone();
        let channels = params
            .channels
            .ok_or_else(|| "Opus source channel count is missing".to_owned())?
            .count();
        let channels = NonZeroU16::new(
            u16::try_from(channels).map_err(|_| "Opus channel count is out of range")?,
        )
        .ok_or_else(|| "Opus source has no channels".to_owned())?;
        let sample_rate = NonZeroU32::new(
            params
                .sample_rate
                .ok_or_else(|| "Opus sample rate is missing".to_owned())?,
        )
        .ok_or_else(|| "Opus source has an invalid sample rate".to_owned())?;
        let duration = params
            .n_frames
            .map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(sample_rate.get())));
        let time_base = params.time_base;

        let decoder = make_decoder(&params)?;

        Ok(Self {
            format,
            decoder,
            track_id,
            codec_params: params,
            samples: Vec::new(),
            sample_index: 0,
            channels,
            sample_rate,
            duration,
            time_base,
            seek_target_seconds: None,
            failure: Arc::new(Mutex::new(None)),
            exhausted: false,
            decoded_any_packet: false,
        })
    }

    pub fn failure_handle(&self) -> Arc<Mutex<Option<String>>> {
        self.failure.clone()
    }

    fn decode_packet(&mut self) -> Result<bool, String> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(false)
                }
                Err(error) => return Err(format!("Opus packet read failed: {error}")),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.decoded_any_packet = true;
                    let mut samples =
                        SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                    samples.copy_interleaved_ref(decoded);
                    self.samples = samples.samples().to_vec();
                    self.sample_index = 0;
                    if let Some(target) = self.seek_target_seconds {
                        let packet_time = self.time_base.map(|base| base.calc_time(packet.ts()));
                        let packet_start = packet_time
                            .map(|time| time.seconds as f64 + time.frac)
                            .unwrap_or(target)
                            + f64::from(packet.trim_start) / f64::from(self.sample_rate.get());
                        let packet_end = packet_start
                            + self.samples.len() as f64
                                / (f64::from(self.sample_rate.get())
                                    * f64::from(self.channels.get()));
                        let frames_to_skip = ((target - packet_start).max(0.)
                            * f64::from(self.sample_rate.get()))
                        .ceil() as usize;
                        let samples_to_skip = frames_to_skip
                            .saturating_mul(usize::from(self.channels.get()))
                            .min(self.samples.len());
                        self.samples.drain(..samples_to_skip);
                        if packet_end >= target {
                            self.seek_target_seconds = None;
                        }
                    }
                    if self.samples.is_empty() {
                        continue;
                    }
                    return Ok(true);
                }
                Err(SymphoniaError::ResetRequired) => self.decoder.reset(),
                Err(error) => return Err(format!("Opus packet decode failed: {error}")),
            }
        }
    }
}

impl Iterator for OpusSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.sample_index < self.samples.len() {
                let sample = self.samples[self.sample_index];
                self.sample_index += 1;
                return Some(sample);
            }
            if self.exhausted {
                return None;
            }
            match self.decode_packet() {
                Ok(true) => {}
                Ok(false) => {
                    self.exhausted = true;
                    return None;
                }
                Err(error) => {
                    *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                    self.exhausted = true;
                    return None;
                }
            }
        }
    }
}

impl Source for OpusSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.sample_index < self.samples.len() {
            Some(self.samples.len() - self.sample_index)
        } else if self.exhausted {
            Some(0)
        } else {
            None
        }
    }

    fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        // Accurate Ogg seeks may place a decoder at a page boundary whose first
        // returned packet is after the target. Start from the stream head instead
        // so pre-skip/preroll is decoded consistently, then discard to the target.
        let was_used = self.decoded_any_packet;
        self.decoder = make_decoder(&self.codec_params).map_err(unsupported_seek_error)?;
        self.decoded_any_packet = false;
        if was_used {
            self.format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time: Time::from(position.as_secs_f64()),
                        track_id: Some(self.track_id),
                    },
                )
                .map_err(|error| {
                    rodio::source::SeekError::Other(Arc::new(io::Error::other(error.to_string())))
                })?;
            self.decoder.reset();
        }
        self.samples.clear();
        self.sample_index = 0;
        self.seek_target_seconds = Some(position.as_secs_f64());
        self.exhausted = false;
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }
}

fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>, String> {
    let mut codecs = CodecRegistry::new();
    codecs.register_all::<OpusDecoder>();
    codecs
        .make(params, &DecoderOptions::default())
        .map_err(|error| format!("Opus decoder could not be created: {error}"))
}

fn unsupported_seek_error(error: String) -> rodio::source::SeekError {
    rodio::source::SeekError::Other(Arc::new(io::Error::other(error)))
}
