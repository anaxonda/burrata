//! Bounded, nonblocking capture of PCM samples that the output mixer consumes.
//!
//! The writer is intended for the audio consumption callback. It only appends
//! to a small local batch and tries to enqueue that batch; disk work, locks, and
//! SongRec never run on the callback thread.

use std::{
    collections::{HashMap, VecDeque},
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
const SUBSCRIBER_QUEUE_MAX: usize = 1024;
const MAX_PCM_SUBSCRIBERS: usize = 4;
const WAVEFORM_BINS_PER_SECOND: usize = 10;
const MAX_WAVEFORM_BINS: usize = 4 * 60 * 60 * WAVEFORM_BINS_PER_SECOND;
const MAX_WAVEFORM_SNAPSHOT_BINS: usize = 4096;
const WORKER_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const VERIFIED_END_DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapGapReason {
    /// A missing/overlapping playback timeline may be a seek or a dropped
    /// sample batch. Consumers must not treat the surrounding audio as
    /// continuous.
    TimelineDiscontinuity,
    /// A subscriber did not drain its bounded channel quickly enough.
    SubscriberBackpressure,
    /// PCM reached the output tap without a trustworthy source position.
    UnpositionedAudio,
}

#[derive(Debug, Clone)]
pub enum PcmTapEvent {
    Samples {
        generation: u64,
        sample_rate: u32,
        channels: u16,
        start_position_seconds: Option<f64>,
        end_position_seconds: Option<f64>,
        samples: Arc<[f32]>,
    },
    Gap {
        generation: u64,
        start_position_seconds: Option<f64>,
        end_position_seconds: Option<f64>,
        reason: TapGapReason,
    },
    Superseded {
        generation: u64,
    },
    VerifiedEnd {
        generation: u64,
        duration_seconds: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformPeak {
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedWaveform {
    pub generation: u64,
    pub duration_seconds: f64,
    pub bins: Vec<Option<WaveformPeak>>,
    pub observed_fraction: f64,
    pub continuous_from_zero: bool,
    pub observed_end_seconds: Option<f64>,
    /// True only after an explicit verified-end signal and gap-free coverage
    /// from zero through the complete resource duration.
    pub complete: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct WaveformBin {
    min: f32,
    max: f32,
}

#[derive(Default)]
struct WaveformState {
    generation: Option<u64>,
    sample_rate: u32,
    channels: u16,
    bins: Vec<Option<WaveformBin>>,
    first_position_seconds: Option<f64>,
    last_position_seconds: Option<f64>,
    observed_end_seconds: Option<f64>,
    continuous_from_zero: bool,
    has_gap: bool,
    truncated: bool,
    verified_end_seconds: Option<f64>,
}

struct TapSubscriber {
    generation: u64,
    sender: mpsc::Sender<PcmTapEvent>,
    pending_gap: Option<PcmTapEvent>,
    retain_until_verified_end: bool,
    superseded: bool,
}

/// A bounded local consumer of PCM batches. Dropping it unregisters the
/// receiver and releases queued sample buffers.
pub struct PcmTapSubscription {
    id: u64,
    subscribers: Arc<Mutex<HashMap<u64, TapSubscriber>>>,
    receiver: mpsc::Receiver<PcmTapEvent>,
}

impl PcmTapSubscription {
    pub async fn recv(&mut self) -> Option<PcmTapEvent> {
        self.receiver.recv().await
    }
    pub fn try_recv(&mut self) -> Result<PcmTapEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for PcmTapSubscription {
    fn drop(&mut self) {
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

enum WorkerRequest {
    Snapshot {
        generation: u64,
        start_seconds: f64,
        duration_seconds: f64,
        bin_count: usize,
        response: std_mpsc::SyncSender<Result<ObservedWaveform, String>>,
    },
    VerifiedEnd {
        generation: u64,
        duration_seconds: f64,
        response: std_mpsc::SyncSender<Result<(), String>>,
    },
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
    worker_requests: mpsc::UnboundedSender<WorkerRequest>,
    current_generation: Arc<AtomicU64>,
    available: Arc<Mutex<(u64, f64)>>,
    subscribers: Arc<Mutex<HashMap<u64, TapSubscriber>>>,
    next_subscriber: AtomicU64,
}

impl PcmTap {
    pub fn new() -> Arc<Self> {
        let (sample_tx, mut sample_rx) = mpsc::channel::<SampleBatch>(CHANNEL_BATCHES);
        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<CaptureRequest>();
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<WorkerRequest>();
        let subscribers = Arc::new(Mutex::new(HashMap::<u64, TapSubscriber>::new()));
        let tap = Arc::new(Self {
            samples: sample_tx,
            captures: capture_tx,
            worker_requests: request_tx,
            current_generation: Arc::new(AtomicU64::new(0)),
            available: Arc::new(Mutex::new((u64::MAX, 0.0))),
            subscribers: subscribers.clone(),
            next_subscriber: AtomicU64::new(1),
        });

        let available = tap.available.clone();
        tauri::async_runtime::spawn(async move {
            let mut generation = None;
            let mut sample_rate = 0u32;
            let mut channels = 0u16;
            let mut ring = VecDeque::<f32>::new();
            let mut playback_end_position_seconds = None;
            let mut waveform = WaveformState::default();
            loop {
                tokio::select! {
                    batch = sample_rx.recv() => match batch {
                        Some(batch) => process_batch(
                            &mut generation, &mut sample_rate, &mut channels, &mut ring,
                            &mut playback_end_position_seconds, &mut waveform, &subscribers, batch,
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
                                process_batch(
                                    &mut generation, &mut sample_rate, &mut channels, &mut ring,
                                    &mut playback_end_position_seconds, &mut waveform, &subscribers, batch,
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
                    request = request_rx.recv() => match request {
                        Some(WorkerRequest::Snapshot { generation: requested, start_seconds, duration_seconds, bin_count, response }) => {
                            while let Ok(batch) = sample_rx.try_recv() {
                                process_batch(
                                    &mut generation, &mut sample_rate, &mut channels, &mut ring,
                                    &mut playback_end_position_seconds, &mut waveform, &subscribers, batch,
                                );
                            }
                            let result = snapshot_waveform(&waveform, requested, start_seconds, duration_seconds, bin_count);
                            let _ = response.send(result);
                        }
                        Some(WorkerRequest::VerifiedEnd { generation: requested, duration_seconds, response }) => {
                            // Natural-end confirmation is ordered after every sample
                            // batch already accepted from the output callback.
                            while let Ok(batch) = sample_rx.try_recv() {
                                process_batch(
                                    &mut generation, &mut sample_rate, &mut channels, &mut ring,
                                    &mut playback_end_position_seconds, &mut waveform, &subscribers, batch,
                                );
                            }
                            if !duration_seconds.is_finite() || duration_seconds <= 0.0 || duration_seconds > MAX_WAVEFORM_BINS as f64 / WAVEFORM_BINS_PER_SECOND as f64 {
                                let _ = response.send(Err("Verified waveform duration is outside the supported range".into()));
                            } else {
                                let has_waveform = waveform.generation == Some(requested);
                                let has_subscriber = subscribers.lock().unwrap_or_else(|e| e.into_inner()).values().any(|item| item.generation == requested);
                                if !has_waveform && !has_subscriber {
                                    let _ = response.send(Err("No playback analysis is waiting for this generation".into()));
                                } else {
                                    if has_waveform { waveform.verified_end_seconds = Some(duration_seconds); }
                                    let result = publish_verified_end(&subscribers, requested, duration_seconds).await;
                                    let _ = response.send(result);
                                }
                            }
                        }
                        None if sample_rx.is_closed() && capture_rx.is_closed() => break,
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
            subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        });
        tap
    }

    /// Create the lightweight producer used by an output generation. Sample
    /// rate and channel count must describe the interleaved values passed to
    /// `push`, not the source file before resampling.
    pub fn writer(&self, generation: u64, sample_rate: u32, channels: u16) -> PcmTapWriter {
        let channels = channels.max(1);
        let channel_count = channels as usize;
        let batch_capacity =
            CHUNK_SAMPLES + (channel_count - CHUNK_SAMPLES % channel_count) % channel_count;
        PcmTapWriter {
            sender: self.samples.clone(),
            current_generation: self.current_generation.clone(),
            generation,
            sample_rate: sample_rate.max(1),
            channels,
            pending: Vec::with_capacity(batch_capacity),
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

    /// Subscribe to local PCM consumed by a specific output generation. A
    /// bounded queue protects playback when analysis is slow; overload is
    /// reported as a coalesced gap event rather than blocking output.
    pub fn subscribe(
        &self,
        generation: u64,
        capacity_batches: usize,
    ) -> Result<PcmTapSubscription, String> {
        self.subscribe_with_finalize(generation, capacity_batches, false)
    }

    /// Subscribe before a speculative output generation starts. A later
    /// generation sends Superseded but keeps this subscriber available for the
    /// actual natural-end verification, which may arrive after that switch.
    pub fn subscribe_until_verified_end(
        &self,
        generation: u64,
        capacity_batches: usize,
    ) -> Result<PcmTapSubscription, String> {
        self.subscribe_with_finalize(generation, capacity_batches, true)
    }

    fn subscribe_with_finalize(
        &self,
        generation: u64,
        capacity_batches: usize,
        retain_until_verified_end: bool,
    ) -> Result<PcmTapSubscription, String> {
        if capacity_batches == 0 || capacity_batches > SUBSCRIBER_QUEUE_MAX {
            return Err(format!(
                "PCM subscriber capacity must be from 1 to {SUBSCRIBER_QUEUE_MAX}"
            ));
        }
        if self.current_generation.load(Ordering::Acquire) > generation {
            return Err("PCM subscription generation has already been superseded".into());
        }
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        if subscribers
            .values()
            .filter(|item| item.generation == generation)
            .count()
            >= MAX_PCM_SUBSCRIBERS
        {
            return Err("Too many PCM subscribers are active for this generation".into());
        }
        let id = self.next_subscriber.fetch_add(1, Ordering::Relaxed);
        if id == u64::MAX {
            return Err("PCM subscriber capacity is exhausted".into());
        }
        let (sender, receiver) = mpsc::channel(capacity_batches);
        subscribers.insert(
            id,
            TapSubscriber {
                generation,
                sender,
                pending_gap: None,
                retain_until_verified_end,
                superseded: false,
            },
        );
        Ok(PcmTapSubscription {
            id,
            subscribers: self.subscribers.clone(),
            receiver,
        })
    }

    /// Close subscribers that belong to an older playback generation. Playback
    /// owners should call this as soon as a new generation is accepted, even
    /// if the new source never reaches its first decoded sample.
    pub fn supersede_older_subscribers(&self, generation: u64) -> usize {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        let older = subscribers
            .iter()
            .filter_map(|(id, item)| (item.generation < generation).then_some(*id))
            .collect::<Vec<_>>();
        for id in &older {
            if let Some(item) = subscribers.get_mut(id) {
                let mut delivered = true;
                if !item.superseded {
                    delivered = item
                        .sender
                        .try_send(PcmTapEvent::Superseded {
                            generation: item.generation,
                        })
                        .is_ok();
                    item.superseded = true;
                }
                let remove = !item.retain_until_verified_end || !delivered;
                if remove {
                    subscribers.remove(id);
                }
            }
        }
        older.len()
    }

    /// Return only bounded amplitude summaries for a generation. The worker
    /// downsamples its fixed-resolution observations; raw PCM never leaves the
    /// tap. A gap or seek keeps `complete` false even if the user later visits
    /// every visible bin.
    pub fn waveform_snapshot(
        &self,
        generation: u64,
        duration_seconds: f64,
        bin_count: usize,
    ) -> Result<ObservedWaveform, String> {
        self.waveform_snapshot_range(generation, 0.0, duration_seconds, bin_count)
    }

    /// Summarize one source-time interval on a zero-based playback-range axis.
    /// The stored tap bins and recognition timestamps remain on absolute source
    /// time; only this returned view is cropped and rebased.
    pub fn waveform_snapshot_range(
        &self,
        generation: u64,
        start_seconds: f64,
        duration_seconds: f64,
        bin_count: usize,
    ) -> Result<ObservedWaveform, String> {
        if !duration_seconds.is_finite()
            || duration_seconds <= 0.0
            || duration_seconds > MAX_WAVEFORM_BINS as f64 / WAVEFORM_BINS_PER_SECOND as f64
            || !start_seconds.is_finite()
            || start_seconds < 0.0
            || start_seconds + duration_seconds
                > MAX_WAVEFORM_BINS as f64 / WAVEFORM_BINS_PER_SECOND as f64
        {
            return Err("Waveform range must be nonnegative and no more than four hours".into());
        }
        if bin_count == 0 || bin_count > MAX_WAVEFORM_SNAPSHOT_BINS {
            return Err(format!(
                "Waveform bin count must be from 1 to {MAX_WAVEFORM_SNAPSHOT_BINS}"
            ));
        }
        let (response_tx, response_rx) = std_mpsc::sync_channel(1);
        self.worker_requests
            .send(WorkerRequest::Snapshot {
                generation,
                start_seconds,
                duration_seconds,
                bin_count,
                response: response_tx,
            })
            .map_err(|_| "PCM tap worker has stopped".to_owned())?;
        response_rx
            .recv_timeout(WORKER_RESPONSE_TIMEOUT)
            .map_err(|error| match error {
                std_mpsc::RecvTimeoutError::Timeout => "PCM waveform snapshot timed out".to_owned(),
                std_mpsc::RecvTimeoutError::Disconnected => {
                    "PCM tap worker stopped before waveform snapshot completed".to_owned()
                }
            })?
    }

    /// Mark an actual, verified natural end. Callers must not use this for
    /// seeks, chapter ranges, incomplete streams, or metadata-only durations.
    pub fn mark_verified_end(&self, generation: u64, duration_seconds: f64) -> Result<(), String> {
        let (response_tx, response_rx) = std_mpsc::sync_channel(1);
        self.worker_requests
            .send(WorkerRequest::VerifiedEnd {
                generation,
                duration_seconds,
                response: response_tx,
            })
            .map_err(|_| "PCM tap worker has stopped".to_owned())?;
        response_rx
            .recv_timeout(WORKER_RESPONSE_TIMEOUT)
            .map_err(|error| match error {
                std_mpsc::RecvTimeoutError::Timeout => "PCM end verification timed out".to_owned(),
                std_mpsc::RecvTimeoutError::Disconnected => {
                    "PCM tap worker stopped before end verification completed".to_owned()
                }
            })?
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
            .recv_timeout(WORKER_RESPONSE_TIMEOUT)
            .map_err(|error| match error {
                std_mpsc::RecvTimeoutError::Timeout => "PCM capture timed out".to_owned(),
                std_mpsc::RecvTimeoutError::Disconnected => {
                    "PCM tap worker stopped before capture completed".to_owned()
                }
            })??;
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
        if self.pending.len() >= CHUNK_SAMPLES && self.pending.len() % self.channels as usize == 0 {
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

fn process_batch(
    generation: &mut Option<u64>,
    sample_rate: &mut u32,
    channels: &mut u16,
    ring: &mut VecDeque<f32>,
    playback_end_position_seconds: &mut Option<f64>,
    waveform: &mut WaveformState,
    subscribers: &Arc<Mutex<HashMap<u64, TapSubscriber>>>,
    batch: SampleBatch,
) {
    let (gap, start, end) = observe_waveform_batch(waveform, &batch);
    publish_subscriber_batch(subscribers, &batch, start, end, gap);
    append_batch(
        generation,
        sample_rate,
        channels,
        ring,
        playback_end_position_seconds,
        batch,
    );
}

fn observe_waveform_batch(
    state: &mut WaveformState,
    batch: &SampleBatch,
) -> (Option<PcmTapEvent>, Option<f64>, Option<f64>) {
    if state
        .generation
        .is_some_and(|active| batch.generation < active)
    {
        return (None, None, None);
    }
    if state.generation != Some(batch.generation) {
        *state = WaveformState {
            generation: Some(batch.generation),
            sample_rate: batch.sample_rate,
            channels: batch.channels,
            continuous_from_zero: true,
            ..WaveformState::default()
        };
    }
    let format_changed = state.sample_rate != batch.sample_rate || state.channels != batch.channels;
    if format_changed {
        state.sample_rate = batch.sample_rate;
        state.channels = batch.channels;
        state.has_gap = true;
        state.continuous_from_zero = false;
    }
    let Some(end) = batch
        .playback_end_position_seconds
        .filter(|value| value.is_finite() && *value >= 0.0)
    else {
        state.has_gap = true;
        state.continuous_from_zero = false;
        return (
            Some(PcmTapEvent::Gap {
                generation: batch.generation,
                start_position_seconds: None,
                end_position_seconds: None,
                reason: TapGapReason::UnpositionedAudio,
            }),
            None,
            None,
        );
    };
    let duration =
        batch.samples.len() as f64 / batch.sample_rate.max(1) as f64 / batch.channels.max(1) as f64;
    let start = end - duration;
    if !duration.is_finite() || duration <= 0.0 || !start.is_finite() || start < 0.0 {
        state.has_gap = true;
        state.continuous_from_zero = false;
        return (
            Some(PcmTapEvent::Gap {
                generation: batch.generation,
                start_position_seconds: None,
                end_position_seconds: Some(end),
                reason: TapGapReason::UnpositionedAudio,
            }),
            None,
            Some(end),
        );
    }
    let tolerance = 2.0 / batch.sample_rate.max(1) as f64;
    let format_gap = format_changed;
    let time_gap = match state.last_position_seconds {
        Some(previous_end) => (start - previous_end).abs() > tolerance,
        None => start > tolerance,
    };
    let gap = if format_gap || time_gap {
        state.has_gap = true;
        state.continuous_from_zero = false;
        Some(PcmTapEvent::Gap {
            generation: batch.generation,
            start_position_seconds: state.last_position_seconds.or(Some(0.0)),
            end_position_seconds: Some(start),
            reason: TapGapReason::TimelineDiscontinuity,
        })
    } else {
        None
    };

    if state.first_position_seconds.is_none() {
        state.first_position_seconds = Some(start);
        state.continuous_from_zero = start <= tolerance && !format_gap;
    }
    if start < 0.0 {
        state.continuous_from_zero = false;
    }
    let frames = batch.samples.len() / batch.channels.max(1) as usize;
    for frame in 0..frames {
        let position = start + frame as f64 / batch.sample_rate.max(1) as f64;
        if position < 0.0 {
            continue;
        }
        let index = (position * WAVEFORM_BINS_PER_SECOND as f64).floor() as usize;
        if index >= MAX_WAVEFORM_BINS {
            state.truncated = true;
            continue;
        }
        if state.bins.len() <= index {
            state.bins.resize(index + 1, None);
        }
        let frame_start = frame * batch.channels.max(1) as usize;
        let frame_end = (frame_start + batch.channels.max(1) as usize).min(batch.samples.len());
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for sample in &batch.samples[frame_start..frame_end] {
            if sample.is_finite() {
                min = min.min(*sample);
                max = max.max(*sample);
            }
        }
        if min.is_finite() && max.is_finite() {
            let bin = &mut state.bins[index];
            if let Some(previous) = bin {
                previous.min = previous.min.min(min.max(-1.0));
                previous.max = previous.max.max(max.min(1.0));
            } else {
                *bin = Some(WaveformBin {
                    min: min.max(-1.0),
                    max: max.min(1.0),
                });
            }
        }
    }
    state.last_position_seconds = Some(end);
    state.observed_end_seconds = Some(
        state
            .observed_end_seconds
            .map_or(end, |observed| observed.max(end)),
    );
    (gap, Some(start), Some(end))
}

fn publish_subscriber_batch(
    subscribers: &Arc<Mutex<HashMap<u64, TapSubscriber>>>,
    batch: &SampleBatch,
    start: Option<f64>,
    end: Option<f64>,
    gap: Option<PcmTapEvent>,
) {
    let mut registry = subscribers.lock().unwrap_or_else(|e| e.into_inner());
    let older = registry
        .iter()
        .filter_map(|(id, item)| (item.generation < batch.generation).then_some(*id))
        .collect::<Vec<_>>();
    for id in older {
        if let Some(item) = registry.get_mut(&id) {
            let mut delivered = true;
            if !item.superseded {
                delivered = item
                    .sender
                    .try_send(PcmTapEvent::Superseded {
                        generation: item.generation,
                    })
                    .is_ok();
                item.superseded = true;
            }
            let remove = !item.retain_until_verified_end || !delivered;
            if remove {
                registry.remove(&id);
            }
        }
    }
    let has_receivers = registry
        .values()
        .any(|item| item.generation == batch.generation);
    if !has_receivers {
        return;
    }
    let samples: Arc<[f32]> = Arc::from(batch.samples.clone());
    let event = PcmTapEvent::Samples {
        generation: batch.generation,
        sample_rate: batch.sample_rate,
        channels: batch.channels,
        start_position_seconds: start,
        end_position_seconds: end,
        samples,
    };
    let mut closed = Vec::new();
    for (id, subscriber) in registry
        .iter_mut()
        .filter(|(_, item)| item.generation == batch.generation && !item.superseded)
    {
        if let Some(pending) = subscriber.pending_gap.take() {
            match subscriber.sender.try_send(pending.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    subscriber.pending_gap = Some(merge_gap(pending, &event));
                    continue;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    closed.push(*id);
                    continue;
                }
            }
        }
        if let Some(discontinuity) = gap.as_ref() {
            match subscriber.sender.try_send(discontinuity.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    subscriber.pending_gap = Some(merge_gap(discontinuity.clone(), &event));
                    continue;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    closed.push(*id);
                    continue;
                }
            }
        }
        if let Err(error) = subscriber.sender.try_send(event.clone()) {
            match error {
                mpsc::error::TrySendError::Full(event) => {
                    subscriber.pending_gap = Some(merge_gap(
                        PcmTapEvent::Gap {
                            generation: batch.generation,
                            start_position_seconds: start,
                            end_position_seconds: end,
                            reason: TapGapReason::SubscriberBackpressure,
                        },
                        &event,
                    ))
                }
                mpsc::error::TrySendError::Closed(_) => closed.push(*id),
            }
        }
    }
    for id in closed {
        registry.remove(&id);
    }
}

/// Deliver verified completion only after all preceding batch/gap events have
/// been queued. A subscriber that cannot accept the terminal event promptly
/// is detached rather than allowed to stall the audio consumer indefinitely.
async fn publish_verified_end(
    subscribers: &Arc<Mutex<HashMap<u64, TapSubscriber>>>,
    generation: u64,
    duration_seconds: f64,
) -> Result<(), String> {
    let pending = {
        let mut registry = subscribers.lock().unwrap_or_else(|e| e.into_inner());
        registry
            .iter_mut()
            .filter(|(_, item)| item.generation == generation)
            .map(|(id, item)| (*id, item.sender.clone(), item.pending_gap.take()))
            .collect::<Vec<_>>()
    };
    let terminal = PcmTapEvent::VerifiedEnd {
        generation,
        duration_seconds,
    };
    let delivery_deadline = tokio::time::Instant::now() + VERIFIED_END_DELIVERY_TIMEOUT;
    let mut detach = Vec::new();
    for (id, sender, pending_gap) in pending {
        let mut delivered = true;
        if let Some(gap) = pending_gap {
            delivered = matches!(
                tokio::time::timeout_at(delivery_deadline, sender.send(gap)).await,
                Ok(Ok(()))
            );
        }
        if delivered {
            delivered = matches!(
                tokio::time::timeout_at(delivery_deadline, sender.send(terminal.clone())).await,
                Ok(Ok(()))
            );
        }
        if !delivered {
            detach.push(id);
        }
    }
    if !detach.is_empty() {
        let mut registry = subscribers.lock().unwrap_or_else(|e| e.into_inner());
        for id in detach {
            registry.remove(&id);
        }
    }
    Ok(())
}

fn merge_gap(previous: PcmTapEvent, dropped: &PcmTapEvent) -> PcmTapEvent {
    fn interval(event: &PcmTapEvent) -> (u64, Option<f64>, Option<f64>) {
        match event {
            PcmTapEvent::Samples {
                generation,
                start_position_seconds,
                end_position_seconds,
                ..
            }
            | PcmTapEvent::Gap {
                generation,
                start_position_seconds,
                end_position_seconds,
                ..
            } => (*generation, *start_position_seconds, *end_position_seconds),
            PcmTapEvent::Superseded { generation }
            | PcmTapEvent::VerifiedEnd { generation, .. } => (*generation, None, None),
        }
    }
    let (generation, old_start, old_end) = interval(&previous);
    let (_, new_start, new_end) = interval(dropped);
    PcmTapEvent::Gap {
        generation,
        start_position_seconds: match (old_start, new_start) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        },
        end_position_seconds: match (old_end, new_end) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        },
        reason: TapGapReason::SubscriberBackpressure,
    }
}

fn snapshot_waveform(
    state: &WaveformState,
    generation: u64,
    start_seconds: f64,
    duration_seconds: f64,
    bin_count: usize,
) -> Result<ObservedWaveform, String> {
    if state.generation != Some(generation) {
        return Err("No waveform data is available for this playback generation".into());
    }
    // `scale` is seconds per output bin. Convert each interval endpoint into
    // the tap's fixed 10 Hz source-bin coordinates only below.
    let scale = duration_seconds / bin_count as f64;
    let mut bins = Vec::with_capacity(bin_count);
    for target in 0..bin_count {
        let start = ((start_seconds + target as f64 * scale) * WAVEFORM_BINS_PER_SECOND as f64)
            .floor() as usize;
        let end =
            (((start_seconds + (target + 1) as f64 * scale) * WAVEFORM_BINS_PER_SECOND as f64)
                .ceil() as usize)
                .max(start + 1);
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for source in state
            .bins
            .get(start..end.min(state.bins.len()))
            .unwrap_or_default()
            .iter()
            .flatten()
        {
            min = min.min(source.min);
            max = max.max(source.max);
        }
        bins.push((min.is_finite() && max.is_finite()).then_some(WaveformPeak { min, max }));
    }
    let observed = bins.iter().filter(|item| item.is_some()).count();
    let fraction = observed as f64 / bin_count as f64;
    let tolerance = 0.25;
    let complete = start_seconds <= 0.025
        && !state.truncated
        && !state.has_gap
        && state.continuous_from_zero
        && state
            .first_position_seconds
            .is_some_and(|start| start <= 0.025)
        && state
            .verified_end_seconds
            .is_some_and(|verified| (verified - duration_seconds).abs() <= tolerance)
        && state
            .observed_end_seconds
            .is_some_and(|end| end + tolerance >= duration_seconds)
        && observed == bin_count;
    let reason = if complete {
        None
    } else if start_seconds > 0.025 {
        Some(
            "This view covers a selected playback range; whole-track completeness is not asserted."
                .into(),
        )
    } else if state.verified_end_seconds.is_none() {
        Some("Playback has not reported a verified natural end.".into())
    } else if state.truncated {
        Some("Waveform observation exceeded the bounded capture duration.".into())
    } else if state.has_gap || !state.continuous_from_zero {
        Some("Playback contained a seek, gap, unpositioned audio, or output format change.".into())
    } else {
        Some("The entire audio duration has not been observed.".into())
    };
    Ok(ObservedWaveform {
        generation,
        duration_seconds,
        bins,
        observed_fraction: fraction,
        continuous_from_zero: state.continuous_from_zero,
        observed_end_seconds: state
            .observed_end_seconds
            .map(|end| (end - start_seconds).clamp(0.0, duration_seconds)),
        complete,
        reason,
    })
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

    fn positioned_batch(
        generation: u64,
        start: f64,
        duration: f64,
        sample_rate: u32,
    ) -> SampleBatch {
        let frames = (duration * f64::from(sample_rate)).round() as usize;
        SampleBatch {
            generation,
            sample_rate,
            channels: 1,
            samples: vec![0.25; frames],
            playback_end_position_seconds: Some(start + duration),
        }
    }

    #[test]
    fn observed_waveform_is_partial_until_gap_free_verified_natural_end() {
        let mut state = WaveformState::default();
        let batch = positioned_batch(5, 0.0, 1.0, 100);
        let (gap, start, end) = observe_waveform_batch(&mut state, &batch);
        assert!(gap.is_none());
        assert_eq!(start, Some(0.0));
        assert_eq!(end, Some(1.0));
        let partial = snapshot_waveform(&state, 5, 0.0, 1.0, 10).unwrap();
        assert_eq!(partial.observed_fraction, 1.0);
        assert!(
            !partial.complete,
            "full-looking observed bins are still partial before verified Ended"
        );
        state.verified_end_seconds = Some(1.0);
        assert!(snapshot_waveform(&state, 5, 0.0, 1.0, 10).unwrap().complete);

        let seek = positioned_batch(5, 2.0, 1.0, 100);
        let (gap, _, _) = observe_waveform_batch(&mut state, &seek);
        assert!(matches!(
            gap,
            Some(PcmTapEvent::Gap {
                reason: TapGapReason::TimelineDiscontinuity,
                ..
            })
        ));
        state.verified_end_seconds = Some(3.0);
        let after_seek = snapshot_waveform(&state, 5, 0.0, 3.0, 30).unwrap();
        assert!(!after_seek.complete);
        assert_eq!(after_seek.bins[15], None);
    }

    #[test]
    fn range_snapshot_rebases_absolute_chapter_time_and_never_claims_whole_track() {
        let mut state = WaveformState::default();
        let batch = positioned_batch(12, 300.0, 10.0, 100);
        observe_waveform_batch(&mut state, &batch);
        state.verified_end_seconds = Some(310.0);

        let chapter = snapshot_waveform(&state, 12, 300.0, 10.0, 100).unwrap();
        assert_eq!(chapter.bins.len(), 100);
        assert!(chapter.bins.iter().all(Option::is_some));
        assert!(chapter.bins[0].unwrap().min > 0.24);
        assert!((chapter.observed_end_seconds.unwrap() - 10.0).abs() < 0.01);
        assert!(!chapter.complete);
        assert!(chapter
            .reason
            .as_deref()
            .unwrap()
            .contains("playback range"));
    }

    #[test]
    fn subscriber_backpressure_is_bounded_and_reported_as_a_gap() {
        let (sender, mut receiver) = mpsc::channel(1);
        let subscribers = Arc::new(Mutex::new(HashMap::from([(
            1,
            TapSubscriber {
                generation: 7,
                sender,
                pending_gap: None,
                retain_until_verified_end: false,
                superseded: false,
            },
        )])));
        let first = positioned_batch(7, 0.0, 1.0, 100);
        publish_subscriber_batch(&subscribers, &first, Some(0.0), Some(1.0), None);
        let second = positioned_batch(7, 1.0, 1.0, 100);
        publish_subscriber_batch(&subscribers, &second, Some(1.0), Some(2.0), None);
        let PcmTapEvent::Samples {
            start_position_seconds,
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected first sample batch");
        };
        assert_eq!(start_position_seconds, Some(0.0));
        let third = positioned_batch(7, 2.0, 1.0, 100);
        publish_subscriber_batch(&subscribers, &third, Some(2.0), Some(3.0), None);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            PcmTapEvent::Gap {
                reason: TapGapReason::SubscriberBackpressure,
                start_position_seconds: Some(1.0),
                end_position_seconds: Some(2.0),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn preload_subscriber_survives_generation_switch_until_verified_end() {
        let (sender, mut receiver) = mpsc::channel(8);
        let subscribers = Arc::new(Mutex::new(HashMap::from([(
            41,
            TapSubscriber {
                generation: 40,
                sender,
                pending_gap: None,
                retain_until_verified_end: true,
                superseded: false,
            },
        )])));
        let old_audio = positioned_batch(40, 0.0, 1.0, 100);
        publish_subscriber_batch(&subscribers, &old_audio, Some(0.0), Some(1.0), None);
        let new_audio = positioned_batch(41, 0.0, 1.0, 100);
        publish_subscriber_batch(&subscribers, &new_audio, Some(0.0), Some(1.0), None);
        assert!(subscribers.lock().unwrap().contains_key(&41));
        assert!(matches!(
            receiver.try_recv(),
            Ok(PcmTapEvent::Samples { generation: 40, .. })
        ));
        assert!(matches!(
            receiver.try_recv(),
            Ok(PcmTapEvent::Superseded { generation: 40 })
        ));
        assert!(
            receiver.try_recv().is_err(),
            "supersession alone must not finalize capture"
        );

        publish_verified_end(&subscribers, 40, 1.0).await.unwrap();
        assert!(matches!(
            receiver.recv().await,
            Some(PcmTapEvent::VerifiedEnd {
                generation: 40,
                duration_seconds: 1.0
            })
        ));
        assert!(subscribers.lock().unwrap().contains_key(&41));
    }

    #[test]
    fn nonretained_subscribers_are_closed_on_supersession() {
        let (sender, mut receiver) = mpsc::channel(8);
        let subscribers = Arc::new(Mutex::new(HashMap::from([(
            8,
            TapSubscriber {
                generation: 7,
                sender,
                pending_gap: None,
                retain_until_verified_end: false,
                superseded: false,
            },
        )])));
        publish_subscriber_batch(
            &subscribers,
            &positioned_batch(8, 0.0, 1.0, 100),
            Some(0.0),
            Some(1.0),
            None,
        );
        assert!(!subscribers.lock().unwrap().contains_key(&8));
        assert!(matches!(
            receiver.try_recv(),
            Ok(PcmTapEvent::Superseded { generation: 7 })
        ));
    }

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

    #[test]
    fn tap_batches_keep_multichannel_frames_intact() {
        let (sender, mut receiver) = mpsc::channel(CHANNEL_BATCHES);
        let mut writer = PcmTapWriter {
            sender,
            current_generation: Arc::new(AtomicU64::new(0)),
            generation: 1,
            sample_rate: 48_000,
            channels: 6,
            pending: Vec::with_capacity(CHUNK_SAMPLES + 2),
            pending_playback_end_position_seconds: None,
        };
        for index in 0..1_200 {
            writer.push_with_position(0.25, Some((index + 1) as f64 / 288_000.0));
        }
        drop(writer);
        while let Ok(batch) = receiver.try_recv() {
            assert_eq!(batch.samples.len() % batch.channels as usize, 0);
        }
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
