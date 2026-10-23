//! Bounded, nonblocking capture of PCM samples that the output mixer consumes.
//!
//! The writer is intended for the audio consumption callback. It only appends
//! to a small local batch and tries to enqueue that batch; disk work, locks, and
//! SongRec never run on the callback thread.

use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    },
};
use tokio::sync::mpsc;

const WINDOW_SECONDS: usize = 12;
pub const RECOGNITION_CAPTURE_WINDOWS: [f64; 3] = [3.0, 6.0, 12.0];
const MIN_CAPTURE_SECONDS: f64 = RECOGNITION_CAPTURE_WINDOWS[0];
const CHUNK_SAMPLES: usize = 256;
// One mixer callback may consume thousands of stereo frames before the async
// worker runs. Keep up to 65,536 queued samples (256 KiB, ~0.68 s at 48 kHz
// stereo), rather than dropping audio halfway through an ordinary callback.
// Overload still drops batches instead of blocking the audio callback.
const CHANNEL_BATCHES: usize = 256;

struct SampleBatch {
    generation: u64,
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
    playback_end_position_seconds: Option<f64>,
}

struct CaptureRequest {
    generation: u64,
    seconds: f64,
    response: std_mpsc::SyncSender<Result<CaptureSnapshot, String>>,
}

struct CaptureSnapshot {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
    playback_end_position_seconds: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PcmCaptureTiming {
    pub start_position_seconds: Option<f64>,
    pub duration_seconds: f64,
    pub sample_rate: u32,
    pub channels: u16,
}

/// A generation-scoped tap fed by samples at the moment the output consumes
/// them. It retains at most twelve seconds of interleaved PCM in a worker task.
pub struct PcmTap {
    samples: mpsc::Sender<SampleBatch>,
    captures: mpsc::UnboundedSender<CaptureRequest>,
    current_generation: Arc<AtomicU64>,
    available: Arc<Mutex<(u64, f64)>>,
}

impl PcmTap {
    pub fn new() -> Arc<Self> {
        let (sample_tx, mut sample_rx) = mpsc::channel::<SampleBatch>(CHANNEL_BATCHES);
        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<CaptureRequest>();
        let tap = Arc::new(Self {
            samples: sample_tx,
            captures: capture_tx,
            current_generation: Arc::new(AtomicU64::new(0)),
            available: Arc::new(Mutex::new((u64::MAX, 0.0))),
        });

        let available = tap.available.clone();
        tauri::async_runtime::spawn(async move {
            let mut generation = None;
            let mut sample_rate = 0u32;
            let mut channels = 0u16;
            let mut ring = VecDeque::<f32>::new();
            let mut playback_end_position_seconds = None;
            loop {
                tokio::select! {
                    batch = sample_rx.recv() => match batch {
                        Some(batch) => append_batch(
                            &mut generation,
                            &mut sample_rate,
                            &mut channels,
                            &mut ring,
                            &mut playback_end_position_seconds,
                            batch,
                        ),
                        None if capture_rx.is_closed() => break,
                        None => {},
                    },
                    request = capture_rx.recv() => match request {
                        Some(request) => {
                            // Consume all already queued PCM before taking the
                            // snapshot, so the sample batch boundary does not
                            // make recognition trail behind audible output.
                            while let Ok(batch) = sample_rx.try_recv() {
                                append_batch(
                                    &mut generation,
                                    &mut sample_rate,
                                    &mut channels,
                                    &mut ring,
                                    &mut playback_end_position_seconds,
                                    batch,
                                );
                            }
                            let result = if request.generation != generation.unwrap_or(u64::MAX) {
                                Err("No PCM samples are available for this playback generation".into())
                            } else {
                                capture_snapshot(
                                    request.seconds,
                                    sample_rate,
                                    channels,
                                    &ring,
                                    playback_end_position_seconds,
                                )
                            };
                            let _ = request.response.send(result);
                        }
                        None if sample_rx.is_closed() => break,
                        None => {},
                    },
                }
                let seconds = if sample_rate > 0 && channels > 0 {
                    ring.len() as f64 / sample_rate as f64 / channels as f64
                } else {
                    0.0
                };
                *available.lock().unwrap_or_else(|e| e.into_inner()) =
                    (generation.unwrap_or(u64::MAX), seconds);
            }
        });
        tap
    }

    /// Create the lightweight producer used by an output generation. Sample
    /// rate and channel count must describe the interleaved values passed to
    /// `push`, not the source file before resampling.
    pub fn writer(&self, generation: u64, sample_rate: u32, channels: u16) -> PcmTapWriter {
        PcmTapWriter {
            sender: self.samples.clone(),
            current_generation: self.current_generation.clone(),
            generation,
            sample_rate: sample_rate.max(1),
            channels: channels.max(1),
            pending: Vec::with_capacity(CHUNK_SAMPLES),
            pending_playback_end_position_seconds: None,
        }
    }

    /// Actual retained PCM, rather than playback position or time since tune-in.
    pub fn available_seconds(&self, generation: u64) -> f64 {
        let available = self.available.lock().unwrap_or_else(|e| e.into_inner());
        if available.0 == generation
            && self.current_generation.load(Ordering::Acquire) == generation
        {
            available.1
        } else {
            0.0
        }
    }

    /// Copy the most recent requested PCM samples for background analysis.
    /// This waits for the tap worker and must never be called from the audio
    /// callback. Keep callers off the UI/runtime worker threads as well.
    pub fn capture_pcm(
        &self,
        generation: u64,
        seconds: f64,
    ) -> Result<(u32, u16, Vec<f32>), String> {
        let snapshot = self.request_snapshot(generation, seconds)?;
        Ok((snapshot.sample_rate, snapshot.channels, snapshot.samples))
    }

    /// Write the latest requested portion of audible PCM as a 16-bit WAV.
    /// This method waits for the tap worker and must only be called from a
    /// background worker, never from the audio callback.
    pub fn capture_wav(&self, generation: u64, path: &Path, seconds: f64) -> Result<(), String> {
        let snapshot = self.request_snapshot(generation, seconds)?;
        write_wav(path, snapshot)
    }

    /// Capture a WAV and retain the source-position anchor carried by the same
    /// consumed PCM batches. Unpositioned or discontinuous taps return no start
    /// position, so callers cannot infer one from request time.
    pub fn capture_wav_with_timing(
        &self,
        generation: u64,
        path: &Path,
        seconds: f64,
    ) -> Result<PcmCaptureTiming, String> {
        let snapshot = self.request_snapshot(generation, seconds)?;
        let duration_seconds =
            snapshot.samples.len() as f64 / snapshot.sample_rate as f64 / snapshot.channels as f64;
        let start_position_seconds = snapshot
            .playback_end_position_seconds
            .map(|end| end - duration_seconds)
            .filter(|start| start.is_finite() && *start >= 0.0);
        let timing = PcmCaptureTiming {
            start_position_seconds,
            duration_seconds,
            sample_rate: snapshot.sample_rate,
            channels: snapshot.channels,
        };
        write_wav(path, snapshot)?;
        Ok(timing)
    }

    fn request_snapshot(&self, generation: u64, seconds: f64) -> Result<CaptureSnapshot, String> {
        if !seconds.is_finite() || seconds <= 0.0 || seconds > WINDOW_SECONDS as f64 {
            return Err(
                "PCM capture duration must be greater than zero and at most twelve seconds".into(),
            );
        }
        if self.current_generation.load(Ordering::Acquire) != generation {
            return Err("No PCM samples are available for this playback generation".into());
        }
        let (response_tx, response_rx) = std_mpsc::sync_channel(1);
        self.captures
            .send(CaptureRequest {
                generation,
                seconds,
                response: response_tx,
            })
            .map_err(|_| "PCM tap worker has stopped".to_owned())?;
        let snapshot = response_rx
            .recv()
            .map_err(|_| "PCM tap worker stopped before capture completed".to_owned())??;
        Ok(snapshot)
    }
}

/// A per-generation PCM producer. `push` never waits for the worker and never
/// locks shared state. Queue pressure drops a batch instead of delaying audio.
pub struct PcmTapWriter {
    sender: mpsc::Sender<SampleBatch>,
    current_generation: Arc<AtomicU64>,
    generation: u64,
    sample_rate: u32,
    channels: u16,
    pending: Vec<f32>,
    pending_playback_end_position_seconds: Option<f64>,
}

impl PcmTapWriter {
    pub fn push(&mut self, sample: f32) {
        self.push_with_position(sample, None);
    }

    pub fn push_with_position(&mut self, sample: f32, playback_end_position_seconds: Option<f64>) {
        self.pending
            .push(if sample.is_finite() { sample } else { 0.0 });
        self.pending_playback_end_position_seconds = playback_end_position_seconds
            .filter(|position| position.is_finite() && *position >= 0.0);
        if self.pending.len() == CHUNK_SAMPLES {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let samples = std::mem::replace(&mut self.pending, Vec::with_capacity(CHUNK_SAMPLES));
        if self
            .sender
            .try_send(SampleBatch {
                generation: self.generation,
                sample_rate: self.sample_rate,
                channels: self.channels,
                samples,
                playback_end_position_seconds: self.pending_playback_end_position_seconds.take(),
            })
            .is_ok()
        {
            self.current_generation
                .fetch_max(self.generation, Ordering::Release);
        }
    }
}

impl Drop for PcmTapWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

fn append_batch(
    generation: &mut Option<u64>,
    sample_rate: &mut u32,
    channels: &mut u16,
    ring: &mut VecDeque<f32>,
    playback_end_position_seconds: &mut Option<f64>,
    batch: SampleBatch,
) {
    if generation.is_some_and(|active| batch.generation < active) {
        return;
    }
    if *generation != Some(batch.generation)
        || *sample_rate != batch.sample_rate
        || *channels != batch.channels
    {
        *generation = Some(batch.generation);
        *sample_rate = batch.sample_rate;
        *channels = batch.channels;
        ring.clear();
        *playback_end_position_seconds = None;
    }
    let batch_duration =
        batch.samples.len() as f64 / batch.sample_rate.max(1) as f64 / batch.channels.max(1) as f64;
    if let (Some(previous_end), Some(batch_end)) = (
        *playback_end_position_seconds,
        batch.playback_end_position_seconds,
    ) {
        let batch_start = batch_end - batch_duration;
        let frame_tolerance = 2.0 / batch.sample_rate.max(1) as f64;
        if (batch_start - previous_end).abs() > frame_tolerance {
            // A dropped batch, seek, or source discontinuity invalidates the
            // retained window as a timing anchor.
            ring.clear();
        }
    } else if playback_end_position_seconds.is_some()
        != batch.playback_end_position_seconds.is_some()
    {
        ring.clear();
    }
    let limit = (*sample_rate as usize)
        .saturating_mul(*channels as usize)
        .saturating_mul(WINDOW_SECONDS);
    ring.extend(batch.samples);
    while ring.len() > limit {
        ring.pop_front();
    }
    *playback_end_position_seconds = batch.playback_end_position_seconds;
}

fn capture_snapshot(
    seconds: f64,
    sample_rate: u32,
    channels: u16,
    ring: &VecDeque<f32>,
    playback_end_position_seconds: Option<f64>,
) -> Result<CaptureSnapshot, String> {
    let minimum =
        (sample_rate as f64 * channels as f64 * seconds.max(MIN_CAPTURE_SECONDS)).ceil() as usize;
    if sample_rate == 0 || channels == 0 || ring.len() < minimum {
        return Err("Not enough audible PCM has been captured yet".into());
    }
    let wanted = (sample_rate as f64 * channels as f64 * seconds).ceil() as usize;
    let count = wanted.min(ring.len());
    let samples = ring.iter().skip(ring.len() - count).copied().collect();
    Ok(CaptureSnapshot {
        sample_rate,
        channels,
        samples,
        playback_end_position_seconds,
    })
}

fn write_wav(path: &Path, snapshot: CaptureSnapshot) -> Result<(), String> {
    let data_len = snapshot.samples.len().saturating_mul(2);
    let data_len = u32::try_from(data_len).map_err(|_| "PCM capture is too large")?;
    let byte_rate = snapshot
        .sample_rate
        .checked_mul(snapshot.channels as u32)
        .and_then(|rate| rate.checked_mul(2))
        .ok_or("PCM capture format is too large")?;
    let mut file =
        File::create(path).map_err(|error| format!("Could not create PCM WAV: {error}"))?;
    file.write_all(b"RIFF").map_err(io_error)?;
    file.write_all(&(36u32.saturating_add(data_len)).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(b"WAVEfmt ").map_err(io_error)?;
    file.write_all(&16u32.to_le_bytes()).map_err(io_error)?;
    file.write_all(&1u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(&snapshot.channels.to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&snapshot.sample_rate.to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&byte_rate.to_le_bytes()).map_err(io_error)?;
    file.write_all(&(snapshot.channels.saturating_mul(2)).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&16u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(b"data").map_err(io_error)?;
    file.write_all(&data_len.to_le_bytes()).map_err(io_error)?;
    for sample in snapshot.samples {
        let quantized = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        file.write_all(&quantized.to_le_bytes()).map_err(io_error)?;
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> String {
    format!("Could not write PCM WAV: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_single_stereo_output_callback_fits_without_waiting_for_the_worker() {
        // The mixer can consume a full device buffer before the async worker
        // gets scheduled. At 48 kHz stereo, 4096 frames are only 85 ms.
        let (sender, mut receiver) = mpsc::channel(CHANNEL_BATCHES);
        let mut writer = PcmTapWriter {
            sender,
            current_generation: Arc::new(AtomicU64::new(0)),
            generation: 1,
            sample_rate: 48_000,
            channels: 2,
            pending: Vec::with_capacity(CHUNK_SAMPLES),
            pending_playback_end_position_seconds: None,
        };
        let samples = 4096 * 2;
        for index in 0..samples {
            writer.push_with_position(0.25, Some((index + 1) as f64 / 96_000.0));
        }
        writer.flush();
        let mut captured = 0;
        while let Ok(batch) = receiver.try_recv() {
            captured += batch.samples.len();
        }
        assert_eq!(
            captured, samples,
            "ordinary output bursts must not starve the continuous capture window"
        );
    }

    #[tokio::test]
    async fn capture_wav_checks_generation_and_three_second_minimum() {
        let tap = PcmTap::new();
        let mut writer = tap.writer(7, 100, 1);
        for _ in 0..299 {
            writer.push(0.25);
        }
        drop(writer);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(tap.capture_wav(6, Path::new("unused.wav"), 3.0).is_err());
        assert!(tap.capture_wav(7, Path::new("unused.wav"), 3.0).is_err());
        assert_eq!(tap.available_seconds(6), 0.0);
        assert!((tap.available_seconds(7) - 2.99).abs() < 0.001);
    }

    #[test]
    fn output_callback_bursts_retain_a_continuous_tempo_and_recognition_window() {
        let (sender, mut receiver) = mpsc::channel(CHANNEL_BATCHES);
        let mut writer = PcmTapWriter {
            sender,
            current_generation: Arc::new(AtomicU64::new(0)),
            generation: 9,
            sample_rate: 48_000,
            channels: 2,
            pending: Vec::with_capacity(CHUNK_SAMPLES),
            pending_playback_end_position_seconds: None,
        };
        let mut generation = None;
        let mut rate = 0;
        let mut channels = 0;
        let mut ring = VecDeque::new();
        let mut end = None;
        let frames = 12 * 48_000;
        for start in (0..frames).step_by(4096) {
            for frame in start..(start + 4096).min(frames) {
                let pulse = frame % 24_000;
                let sample = if pulse < 480 {
                    0.8 * (1.0 - pulse as f32 / 480.0)
                } else {
                    0.0
                };
                for _ in 0..2 {
                    writer.push_with_position(sample, Some((frame + 1) as f64 / 48_000.0));
                }
            }
            writer.flush();
            // The worker only runs between device callbacks in this fixture.
            while let Ok(batch) = receiver.try_recv() {
                append_batch(
                    &mut generation,
                    &mut rate,
                    &mut channels,
                    &mut ring,
                    &mut end,
                    batch,
                );
            }
        }
        let capture = capture_snapshot(10.0, rate, channels, &ring, end).unwrap();
        assert_eq!(capture.samples.len(), 10 * 48_000 * 2);
        assert!((capture.playback_end_position_seconds.unwrap() - 12.0).abs() < 1e-9);
        let tempo =
            crate::live_bpm::estimate_interleaved_pcm(&capture.samples, rate, channels).unwrap();
        assert!((tempo.bpm - 120.0).abs() <= 1.0, "{tempo:?}");
        assert!(capture_snapshot(12.0, rate, channels, &ring, end).is_ok());
    }

    #[test]
    fn positioned_pcm_window_maps_capture_to_source_time_and_resets_on_discontinuity() {
        let mut generation = None;
        let mut sample_rate = 0;
        let mut channels = 0;
        let mut ring = VecDeque::new();
        let mut playback_end = None;
        append_batch(
            &mut generation,
            &mut sample_rate,
            &mut channels,
            &mut ring,
            &mut playback_end,
            SampleBatch {
                generation: 4,
                sample_rate: 10,
                channels: 1,
                samples: vec![0.1; 30],
                playback_end_position_seconds: Some(9.0),
            },
        );
        append_batch(
            &mut generation,
            &mut sample_rate,
            &mut channels,
            &mut ring,
            &mut playback_end,
            SampleBatch {
                generation: 4,
                sample_rate: 10,
                channels: 1,
                samples: vec![0.2; 30],
                playback_end_position_seconds: Some(12.0),
            },
        );
        let capture = capture_snapshot(3.0, sample_rate, channels, &ring, playback_end).unwrap();
        let start = capture.playback_end_position_seconds.unwrap()
            - capture.samples.len() as f64 / sample_rate as f64 / channels as f64;
        assert!((start - 9.0).abs() < 1e-9);

        // A seek or dropped batch creates a source-time jump. The old audio
        // stays out of the new window even though the output generation is same.
        append_batch(
            &mut generation,
            &mut sample_rate,
            &mut channels,
            &mut ring,
            &mut playback_end,
            SampleBatch {
                generation: 4,
                sample_rate: 10,
                channels: 1,
                samples: vec![0.3; 30],
                playback_end_position_seconds: Some(20.0),
            },
        );
        assert_eq!(ring.len(), 30);
        let capture = capture_snapshot(3.0, sample_rate, channels, &ring, playback_end).unwrap();
        let start = capture.playback_end_position_seconds.unwrap()
            - capture.samples.len() as f64 / sample_rate as f64 / channels as f64;
        assert!((start - 17.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn capture_pcm_returns_only_the_requested_latest_generation_window() {
        let tap = PcmTap::new();
        let mut writer = tap.writer(17, 100, 2);
        for sample in 0..2400 {
            writer.push(sample as f32 / 2400.0);
        }
        drop(writer);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let (sample_rate, channels, samples) = tap.capture_pcm(17, 10.0).unwrap();
        assert_eq!((sample_rate, channels), (100, 2));
        assert_eq!(samples.len(), 2000);
        assert!((samples[0] - 400.0 / 2400.0).abs() < f32::EPSILON * 4.0);
        assert!(tap.capture_pcm(16, 10.0).is_err());
        let (_, _, long_window) = tap.capture_pcm(17, 11.0).unwrap();
        assert_eq!(long_window.len(), 2200);
        assert!(tap.capture_pcm(17, 13.0).is_err());
    }

    #[test]
    fn progressive_capture_requires_requested_audio_not_only_minimum() {
        let ring = VecDeque::from(vec![0.25; 300]);
        assert_eq!(
            capture_snapshot(3.0, 100, 1, &ring, None)
                .unwrap()
                .samples
                .len(),
            300
        );
        assert!(capture_snapshot(6.0, 100, 1, &ring, None).is_err());
        assert!(capture_snapshot(12.0, 100, 1, &ring, None).is_err());
    }

    #[tokio::test]
    async fn capture_wav_has_valid_header_and_caps_ring_to_twelve_seconds() {
        let tap = PcmTap::new();
        let mut writer = tap.writer(8, 100, 1);
        for i in 0..2000 {
            writer.push(if i % 2 == 0 { 0.5 } else { -0.5 });
        }
        drop(writer);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let path =
            std::env::temp_dir().join(format!("burrata-pcm-tap-{}.wav", uuid::Uuid::new_v4()));
        tap.capture_wav(8, &path, 12.0).unwrap();
        let wav = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // 12 seconds * 100 samples/sec * one channel * 2 bytes/sample + header.
        assert_eq!(wav.len(), 44 + 1200 * 2);
    }
}
