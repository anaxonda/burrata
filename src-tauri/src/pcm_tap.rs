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
        mpsc as std_mpsc, Arc,
    },
};
use tokio::sync::mpsc;

const WINDOW_SECONDS: usize = 12;
const MIN_CAPTURE_SECONDS: f64 = 8.0;
const CHUNK_SAMPLES: usize = 256;
const CHANNEL_BATCHES: usize = 16; // 16 * 256 = 4096 bounded queued samples.

struct SampleBatch {
    generation: u64,
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
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
}

/// A generation-scoped tap fed by samples at the moment the output consumes
/// them. It retains at most twelve seconds of interleaved PCM in a worker task.
pub struct PcmTap {
    samples: mpsc::Sender<SampleBatch>,
    captures: mpsc::UnboundedSender<CaptureRequest>,
    current_generation: Arc<AtomicU64>,
}

impl PcmTap {
    pub fn new() -> Arc<Self> {
        let (sample_tx, mut sample_rx) = mpsc::channel::<SampleBatch>(CHANNEL_BATCHES);
        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<CaptureRequest>();
        let tap = Arc::new(Self {
            samples: sample_tx,
            captures: capture_tx,
            current_generation: Arc::new(AtomicU64::new(0)),
        });

        tauri::async_runtime::spawn(async move {
            let mut generation = None;
            let mut sample_rate = 0u32;
            let mut channels = 0u16;
            let mut ring = VecDeque::<f32>::new();
            loop {
                tokio::select! {
                    batch = sample_rx.recv() => match batch {
                        Some(batch) => append_batch(
                            &mut generation,
                            &mut sample_rate,
                            &mut channels,
                            &mut ring,
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
                                )
                            };
                            let _ = request.response.send(result);
                        }
                        None if sample_rx.is_closed() => break,
                        None => {},
                    },
                }
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
        }
    }

    /// Write the latest requested portion of audible PCM as a 16-bit WAV.
    /// This method waits for the tap worker and must only be called from a
    /// background worker, never from the audio callback.
    pub fn capture_wav(&self, generation: u64, path: &Path, seconds: f64) -> Result<(), String> {
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
        write_wav(path, snapshot)
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
}

impl PcmTapWriter {
    pub fn push(&mut self, sample: f32) {
        self.pending
            .push(if sample.is_finite() { sample } else { 0.0 });
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
    }
    let limit = (*sample_rate as usize)
        .saturating_mul(*channels as usize)
        .saturating_mul(WINDOW_SECONDS);
    ring.extend(batch.samples);
    while ring.len() > limit {
        ring.pop_front();
    }
}

fn capture_snapshot(
    seconds: f64,
    sample_rate: u32,
    channels: u16,
    ring: &VecDeque<f32>,
) -> Result<CaptureSnapshot, String> {
    let minimum = (sample_rate as f64 * channels as f64 * MIN_CAPTURE_SECONDS).ceil() as usize;
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

    #[tokio::test]
    async fn capture_wav_checks_generation_and_eight_second_minimum() {
        let tap = PcmTap::new();
        let mut writer = tap.writer(7, 100, 1);
        for _ in 0..799 {
            writer.push(0.25);
        }
        drop(writer);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(tap.capture_wav(6, Path::new("unused.wav"), 8.0).is_err());
        assert!(tap.capture_wav(7, Path::new("unused.wav"), 8.0).is_err());
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
