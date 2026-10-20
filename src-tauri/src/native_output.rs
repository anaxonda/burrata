//! Native desktop audio output. The device stream is created and controlled on one
//! dedicated thread because CPAL streams are intentionally not `Send` on all hosts.

#[path = "native_http.rs"]
mod native_http;
#[path = "native_storage_fault.rs"]
pub(crate) mod storage_fault;

use rodio::{DeviceSinkBuilder, Player, Source};
use std::num::{NonZeroU16, NonZeroU32};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Read},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc as std_mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct AudioSource {
    pub input: String,
    pub ffmpeg: PathBuf,
    pub headers: Vec<(String, String)>,
    /// Verified metadata about the resolved source. Muxed MP4 streams can carry
    /// a container duration that Rodio/Symphonia estimates incorrectly, so they
    /// are decoded through FFmpeg while retaining the extractor's verified duration.
    pub decoder_hint: Option<AudioSourceDecoderHint>,
}

#[derive(Debug, Clone)]
pub struct AudioSourceDecoderHint {
    pub mime_type: Option<String>,
    pub codec: Option<String>,
    pub duration_seconds: Option<f64>,
    pub content_length: Option<u64>,
    pub has_video: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupReserveDiagnostics {
    pub target_samples: usize,
    pub released_samples: usize,
    pub short_clean_eof: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum StartupReservePolicy {
    Disabled,
    DecodedTwoSeconds,
}
fn configured_startup_reserve_policy() -> StartupReservePolicy {
    // Tests inject policy per source, never mutate global process configuration.
    #[cfg(test)]
    {
        StartupReservePolicy::Disabled
    }
    #[cfg(not(test))]
    {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ENABLED.get_or_init(|| {
            std::env::var("BURRATA_EXPERIMENTAL_FINITE_PCM_STARTUP_RESERVE")
                .is_ok_and(|value| value == "1")
        }) {
            StartupReservePolicy::DecodedTwoSeconds
        } else {
            StartupReservePolicy::Disabled
        }
    }
}
fn startup_reserve_target(
    source: &AudioSource,
    position: f64,
    end: Option<f64>,
    paused: bool,
    rate: u32,
    channels: u16,
    policy: StartupReservePolicy,
) -> Option<usize> {
    if policy != StartupReservePolicy::DecodedTwoSeconds
        || paused
        || position != 0.
        || end.is_some()
        || !(source.input.starts_with("https://") || source.input.starts_with("http://"))
        || !source
            .decoder_hint
            .as_ref()
            .and_then(|hint| hint.duration_seconds)
            .is_some_and(|seconds| seconds.is_finite() && seconds > 0.)
    {
        return None;
    }
    let samples = (rate as usize)
        .checked_mul(channels as usize)?
        .checked_mul(2)?;
    // Pilot allocation bound, derived from real output format; never silently shorten2s.
    (samples > 0 && samples.checked_mul(std::mem::size_of::<f32>())? <= 2 * 1024 * 1024)
        .then_some(samples)
}
struct StartupPcmReserve {
    target: usize,
    samples: usize,
    blocks: VecDeque<Vec<f32>>,
    ready: bool,
    short_clean_eof: bool,
}
enum ReserveFill {
    Waiting,
    Ready,
    Failed,
}
impl StartupPcmReserve {
    fn new(target: usize) -> Self {
        Self {
            target,
            samples: 0,
            blocks: VecDeque::with_capacity(target.div_ceil(4096) + 1),
            ready: false,
            short_clean_eof: false,
        }
    }
    fn fill(&mut self, rx: &std_mpsc::Receiver<Vec<f32>>, failed: &AtomicBool) -> ReserveFill {
        if failed.load(Ordering::Acquire) {
            return ReserveFill::Failed;
        }
        for _ in 0..4 {
            if self.samples >= self.target {
                break;
            }
            match rx.try_recv() {
                Ok(block) => {
                    self.samples += block.len();
                    self.blocks.push_back(block);
                }
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    if failed.load(Ordering::Acquire) || self.samples == 0 {
                        return ReserveFill::Failed;
                    }
                    self.short_clean_eof = self.samples < self.target;
                    self.ready = true;
                    return ReserveFill::Ready;
                }
            }
        }
        if failed.load(Ordering::Acquire) {
            return ReserveFill::Failed;
        }
        self.ready = self.samples >= self.target;
        if self.ready {
            ReserveFill::Ready
        } else {
            ReserveFill::Waiting
        }
    }
}
// A pilot consumer must not mistake sender-drop-before-error-publication for cleanEOF.
struct ReserveProducerCompletion {
    _sender: std_mpsc::SyncSender<Vec<f32>>,
    failed: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    events: UnboundedSender<OutputEvent>,
    generation: u64,
}
impl Drop for ReserveProducerCompletion {
    fn drop(&mut self) {
        if thread::panicking() {
            self.failed.store(true, Ordering::Release);
            if !self.cancelled.load(Ordering::Acquire) {
                let _ = self.events.send(OutputEvent::Failed {
                    generation: self.generation,
                    message: "Audio decoder stopped unexpectedly".into(),
                });
            }
        }
    }
}

/// Safe numeric and media-type facts about the active native decoder. No source URL,
/// response headers other than content metadata, or credentials are retained here.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoderDiagnostics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_reserve: Option<StartupReserveDiagnostics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_native_attempt: Option<FailedNativeAttemptDiagnostics>,
    pub decoder_kind: String,
    pub codec_hint: Option<String>,
    pub content_type: Option<String>,
    pub input_content_length: Option<u64>,
    pub content_range_total: Option<u64>,
    /// Native reader accounting only. FFmpeg leaves this at zero; this is not network-byte telemetry.
    pub bytes_read: u64,
    /// Native stream progress only. FFmpeg leaves this at zero; this is not network-byte telemetry.
    pub bytes_received: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub total_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader_setup_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoder_setup_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_decoded_block_ms: Option<u64>,
    pub reader_reused: bool,
    pub decoder_reused: bool,
}

/// Accounting retained when the native reader/decoder falls back to FFmpeg.
/// Range progress is a high-water mark, not network bytes transferred.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedNativeAttemptDiagnostics {
    pub reason: FailedNativeAttemptReason,
    pub duration_ms: u64,
    pub reader_setup_ms: Option<u64>,
    pub native_decoder_setup_ms: Option<u64>,
    pub reader_bytes_read: u64,
    pub downloaded_range_end: u64,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailedNativeAttemptReason {
    DecoderSetup,
    Seek,
    Decode,
}

#[derive(Debug)]
pub enum OutputCommand {
    Load {
        generation: u64,
        /// Absolute deadline for this request's first audio. Reader setup must
        /// consume the remaining startup budget rather than starting a new one.
        startup_deadline: Instant,
        allow_startup_reserve: bool,
        source: AudioSource,
        position: f64,
        end: Option<f64>,
    },
    Preload {
        generation: u64,
        next_generation: u64,
        source: AudioSource,
        position: f64,
        end: Option<f64>,
    },
    ClearPreload {
        generation: u64,
    },
    Pause,
    Resume,
    Stop,
    Seek {
        generation: u64,
        position: f64,
    },
    Volume(f32),
    Shutdown,
}

// Collected only on actual PCM-queue starvation; contains no source paths/URLs.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferingDiagnostics {
    pub reader_observed: bool,
    pub network_reader: bool,
    pub bytes_read: u64,
    pub downloaded_range_end: u64,
    pub download_progress_age_ms: Option<u64>,
    pub read_in_flight_ms: Option<u64>,
    pub max_read_ms: u64,
    pub seek_in_flight_ms: Option<u64>,
    pub max_seek_ms: u64,
    pub pcm_produced_samples: u64,
    pub pcm_consumed_samples: u64,
    pub synthetic_silence_samples: u64,
}
struct StreamRuntimeStats {
    origin: Instant,
    reader_observed: AtomicBool,
    network_reader: AtomicBool,
    bytes_read: AtomicU64,
    downloaded_range_end: AtomicU64,
    last_progress: AtomicU64,
    read_started: AtomicU64,
    max_read_ms: AtomicU64,
    seek_started: AtomicU64,
    max_seek_ms: AtomicU64,
    pcm_produced_samples: AtomicU64,
    synthetic_silence_samples: AtomicU64,
}
impl Default for StreamRuntimeStats {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            reader_observed: AtomicBool::new(false),
            network_reader: AtomicBool::new(false),
            bytes_read: AtomicU64::new(0),
            downloaded_range_end: AtomicU64::new(0),
            last_progress: AtomicU64::new(0),
            read_started: AtomicU64::new(0),
            max_read_ms: AtomicU64::new(0),
            seek_started: AtomicU64::new(0),
            max_seek_ms: AtomicU64::new(0),
            pcm_produced_samples: AtomicU64::new(0),
            synthetic_silence_samples: AtomicU64::new(0),
        }
    }
}
impl StreamRuntimeStats {
    fn ms(&self) -> u64 {
        self.origin.elapsed().as_millis().min(u64::MAX as u128 - 1) as u64
    }
    fn snapshot(&self, consumed: u64) -> BufferingDiagnostics {
        let now = self.ms();
        let age = |value: u64| (value != 0).then(|| now.saturating_sub(value - 1));
        BufferingDiagnostics {
            reader_observed: self.reader_observed.load(Ordering::Relaxed),
            network_reader: self.network_reader.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            downloaded_range_end: self.downloaded_range_end.load(Ordering::Relaxed),
            download_progress_age_ms: age(self.last_progress.load(Ordering::Relaxed)),
            read_in_flight_ms: age(self.read_started.load(Ordering::Relaxed)),
            max_read_ms: self.max_read_ms.load(Ordering::Relaxed),
            seek_in_flight_ms: age(self.seek_started.load(Ordering::Relaxed)),
            max_seek_ms: self.max_seek_ms.load(Ordering::Relaxed),
            pcm_produced_samples: self.pcm_produced_samples.load(Ordering::Relaxed),
            pcm_consumed_samples: consumed,
            synthetic_silence_samples: self.synthetic_silence_samples.load(Ordering::Relaxed),
        }
    }
}
struct TimedStreamReader<T> {
    inner: T,
    stats: Arc<StreamRuntimeStats>,
}
impl<T: Read> Read for TimedStreamReader<T> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stats.reader_observed.store(true, Ordering::Relaxed);
        let started = self.stats.ms();
        self.stats
            .read_started
            .store(started + 1, Ordering::Relaxed);
        let result = self.inner.read(buffer);
        self.stats
            .max_read_ms
            .fetch_max(self.stats.ms().saturating_sub(started), Ordering::Relaxed);
        self.stats.read_started.store(0, Ordering::Relaxed);
        if let Ok(bytes) = &result {
            self.stats
                .bytes_read
                .fetch_add(*bytes as u64, Ordering::Relaxed);
        }
        result
    }
}
impl<T: std::io::Seek> std::io::Seek for TimedStreamReader<T> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        let started = self.stats.ms();
        self.stats
            .seek_started
            .store(started + 1, Ordering::Relaxed);
        let result = self.inner.seek(position);
        self.stats
            .max_seek_ms
            .fetch_max(self.stats.ms().saturating_sub(started), Ordering::Relaxed);
        self.stats.seek_started.store(0, Ordering::Relaxed);
        result
    }
}

#[derive(Debug, Clone)]
pub enum OutputEvent {
    Started {
        generation: u64,
        duration: Option<f64>,
        sample_rate: u32,
        channels: u16,
        decoder: Option<DecoderDiagnostics>,
    },
    Position {
        generation: u64,
        seconds: f64,
    },
    Buffering {
        generation: u64,
        diagnostics: Option<BufferingDiagnostics>,
    },
    Ended {
        generation: u64,
    },
    LocalStorageFailed {
        generation: u64,
        fault: storage_fault::Fault,
    },
    Failed {
        generation: u64,
        message: String,
    },
    DeviceLost {
        generation: u64,
        position: f64,
        reason: String,
    },
    Interrupted {
        generation: u64,
    },
}

pub struct NativeOutput {
    tx: std_mpsc::Sender<OutputCommand>,
    worker: Option<JoinHandle<()>>,
    tap: Arc<crate::pcm_tap::PcmTap>,
}

impl NativeOutput {
    pub fn new(events: UnboundedSender<OutputEvent>) -> Result<Self, String> {
        let (tx, rx) = std_mpsc::channel();
        let tap = crate::pcm_tap::PcmTap::new();
        let worker_tap = tap.clone();
        let worker = thread::Builder::new()
            .name("burrata-audio-output".into())
            .spawn(move || run_worker(rx, events, worker_tap))
            .map_err(|e| format!("could not start audio output thread: {e}"))?;
        Ok(Self {
            tx,
            worker: Some(worker),
            tap,
        })
    }

    pub fn tap(&self) -> Arc<crate::pcm_tap::PcmTap> {
        self.tap.clone()
    }

    pub fn send(&self, command: OutputCommand) -> Result<(), String> {
        self.tx
            .send(command)
            .map_err(|_| "native audio output has stopped".to_owned())
    }
}

impl Drop for NativeOutput {
    fn drop(&mut self) {
        let _ = self.tx.send(OutputCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

type RodioDecoder = rodio::Decoder<BufReader<Box<dyn ReadSeekSend>>>;

enum NativeDecoder {
    Rodio(RodioDecoder),
    #[cfg(feature = "experimental-opus")]
    Opus(crate::opus_experiment::OpusSource),
}

impl NativeDecoder {
    fn failure_handle(&self) -> Option<Arc<std::sync::Mutex<Option<String>>>> {
        match self {
            Self::Rodio(_) => None,
            #[cfg(feature = "experimental-opus")]
            Self::Opus(source) => Some(source.failure_handle()),
        }
    }
}

impl Iterator for NativeDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Rodio(decoder) => decoder.next(),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Rodio(decoder) => decoder.size_hint(),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.size_hint(),
        }
    }
}

impl Source for NativeDecoder {
    fn current_span_len(&self) -> Option<usize> {
        match self {
            Self::Rodio(decoder) => decoder.current_span_len(),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.current_span_len(),
        }
    }

    fn channels(&self) -> NonZeroU16 {
        match self {
            Self::Rodio(decoder) => decoder.channels(),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.channels(),
        }
    }

    fn sample_rate(&self) -> NonZeroU32 {
        match self {
            Self::Rodio(decoder) => decoder.sample_rate(),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.sample_rate(),
        }
    }

    fn total_duration(&self) -> Option<Duration> {
        match self {
            Self::Rodio(decoder) => decoder.total_duration(),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.total_duration(),
        }
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        match self {
            Self::Rodio(decoder) => decoder.try_seek(position),
            #[cfg(feature = "experimental-opus")]
            Self::Opus(decoder) => decoder.try_seek(position),
        }
    }
}

/// One track's decoder and downloaded ranges survive output-queue replacement.
/// Only the decode worker takes `decoder`; the audio callback never locks it.
#[derive(Default)]
struct DecoderSession {
    #[cfg(test)]
    decode_thread_finished: AtomicBool,
    #[cfg(test)]
    force_legacy_opus_probe: AtomicBool,
    #[cfg(test)]
    force_loopback_opus_ffmpeg: AtomicBool,
    stream_stats: Arc<StreamRuntimeStats>,
    storage_faults: storage_fault::Faults,
    decoder: std::sync::Mutex<Option<NativeDecoder>>,
    cached_diagnostics: std::sync::Mutex<Option<DecoderDiagnostics>>,
    active_cancel: std::sync::Mutex<std::sync::Weak<AtomicBool>>,
    cancel_download: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    startup_deadline: std::sync::Mutex<Option<Instant>>,
    // A queued decoder may fail before its occurrence becomes active. The
    // output worker, never the mixer callback, delivers this retained failure.
    retained_failure: std::sync::Mutex<Option<String>>,
}
impl DecoderSession {
    fn bind(&self, cancel: &Arc<AtomicBool>) {
        *self.active_cancel.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(cancel);
    }
    fn lock_decoder(
        &self,
        cancelled: &AtomicBool,
    ) -> Result<std::sync::MutexGuard<'_, Option<NativeDecoder>>, String> {
        let started = Instant::now();
        let mut invalidated = false;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            match self.decoder.try_lock() {
                Ok(mut decoder) => {
                    if invalidated {
                        *decoder = None;
                    }
                    return Ok(decoder);
                }
                Err(std::sync::TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {
                    // A retired decoder may still be blocked waiting on HTTP bytes.
                    // Stop that download rather than making the new seek wait for its timeout.
                    if !invalidated && started.elapsed() >= Duration::from_millis(100) {
                        if let Some(cancel) = self
                            .cancel_download
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_ref()
                        {
                            cancel();
                        }
                        invalidated = true;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
    fn cancelled(&self) -> bool {
        self.active_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade()
            .is_none_or(|cancel| cancel.load(Ordering::Relaxed))
    }

    fn set_startup_deadline(&self, deadline: Option<Instant>) {
        *self
            .startup_deadline
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = deadline;
    }

    fn startup_deadline(&self) -> Option<Instant> {
        *self
            .startup_deadline
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

fn register_download_cancellation<F, D>(
    runtime: &tokio::runtime::Runtime,
    session: &Arc<DecoderSession>,
    cancel_download: F,
    download_finished: D,
) where
    F: Fn() + Send + Sync + 'static,
    D: Fn() -> bool + Send + Sync + 'static,
{
    let cancel_download: Arc<dyn Fn() + Send + Sync> = Arc::new(cancel_download);
    *session
        .cancel_download
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = {
        let cancel_download = cancel_download.clone();
        Some(Box::new(move || cancel_download()))
    };
    let weak_session = Arc::downgrade(session);
    let cancel_worker = cancel_download.clone();
    let download_finished = Arc::new(download_finished);
    runtime.spawn(async move {
        loop {
            if download_finished() {
                break;
            }
            if weak_session
                .upgrade()
                .is_none_or(|session| session.cancelled())
            {
                cancel_worker();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

async fn wait_for_decode_cancel(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Await the initial HTTP stream handshake under the playback request's
/// absolute deadline and the decoder's cancellation flag. Dropping this future
/// cancels reqwest/stream-download setup before a reader cancellation token
/// exists.
fn await_remote_reader_setup<T, F>(
    runtime: &tokio::runtime::Runtime,
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
    setup: F,
) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    runtime.block_on(async move {
        let setup = async move {
            if let Some(deadline) = deadline {
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), setup)
                    .await
                    .map_err(|_| {
                        "Audio reader setup exceeded the playback startup deadline".to_owned()
                    })?
            } else {
                setup.await
            }
        };
        tokio::select! {
            biased;
            _ = wait_for_decode_cancel(cancelled) => Err("Cancelled".into()),
            result = setup => result,
        }
    })
}

fn network_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: std::sync::OnceLock<Result<tokio::runtime::Runtime, String>> =
        std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("burrata-stream-io")
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

struct Playback {
    decoder_session: Arc<DecoderSession>,
    generation: u64,
    player: Player,
    samples: Arc<AtomicU64>,
    sample_rate: Arc<std::sync::atomic::AtomicU32>,
    channels: Arc<std::sync::atomic::AtomicU32>,
    source: AudioSource,
    end: Option<f64>,
    position: f64,
    cancel: Arc<AtomicBool>,
    decode_failed: Arc<AtomicBool>,
    active_generation: Arc<AtomicU64>,
    base_position: Arc<AtomicU64>,
    preload_cancel: Option<Arc<AtomicBool>>,
    activation_rx: std_mpsc::Receiver<Activation>,
    activation_tx: std_mpsc::Sender<Activation>,
    buffering: Arc<AtomicBool>,
    buffering_reported: bool,
    source_duration: Option<f64>,
    decoder_diagnostics: Option<DecoderDiagnostics>,
    source_sample_rate: u32,
    source_channels: u16,
    live_metadata: Option<ActiveMetadata>,
    paused: bool,
}

struct Activation {
    decoder_session: Arc<DecoderSession>,
    generation: u64,
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    cancel: Arc<AtomicBool>,
    sample_rate: u32,
    channels: u16,
    duration: Option<f64>,
    decoder_diagnostics: Option<DecoderDiagnostics>,
    decode_failed: Arc<AtomicBool>,
    live_metadata: ActiveMetadata,
}

struct ActiveMetadata {
    rate: Arc<std::sync::atomic::AtomicU32>,
    channels: Arc<std::sync::atomic::AtomicU32>,
    duration: Arc<AtomicU64>,
    decoder: Arc<std::sync::Mutex<Option<DecoderDiagnostics>>>,
}

struct Reconnect {
    generation: u64,
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    paused: bool,
    attempts: u8,
    last_error: String,
    retry_at: Instant,
}

// Only combine adjacent seeks. Preserve pause, load, stop and queue command ordering.
fn coalesce_seeks(
    mut command: Result<OutputCommand, std_mpsc::RecvTimeoutError>,
    rx: &std_mpsc::Receiver<OutputCommand>,
    pending: &mut Option<OutputCommand>,
) -> Result<OutputCommand, std_mpsc::RecvTimeoutError> {
    if matches!(&command, Ok(OutputCommand::Seek { .. })) {
        while let Ok(next) = rx.try_recv() {
            if matches!(&next, OutputCommand::Seek { .. }) {
                command = Ok(next);
            } else {
                *pending = Some(next);
                break;
            }
        }
    }
    command
}

fn run_worker(
    rx: std_mpsc::Receiver<OutputCommand>,
    events: UnboundedSender<OutputEvent>,
    tap: Arc<crate::pcm_tap::PcmTap>,
) {
    let mut player: Option<Playback> = None;
    let mut sink: Option<rodio::MixerDeviceSink> = None;
    let device_lost = Arc::new(AtomicBool::new(false));
    let device_error = Arc::new(std::sync::Mutex::new(None::<String>));
    let mut reconnect: Option<Reconnect> = None;
    let mut volume = 1.0f32;
    let mut last_position = Instant::now();
    let mut pending = None;
    loop {
        let command = pending
            .take()
            .map(Ok)
            .unwrap_or_else(|| rx.recv_timeout(Duration::from_millis(40)));
        let command = coalesce_seeks(command, &rx, &mut pending);
        match command {
            Ok(OutputCommand::Shutdown) => break,
            Ok(OutputCommand::Load {
                generation,
                startup_deadline,
                allow_startup_reserve,
                source,
                position,
                end,
            }) => {
                reconnect = None;
                // A new generation starts with a clean buffering transition state.
                if let Some(old) = player.take() {
                    old.cancel.store(true, Ordering::Relaxed);
                    if let Some(cancel) = old.preload_cancel {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    old.player.stop();
                }
                let _ = events.send(OutputEvent::Buffering {
                    generation,
                    diagnostics: None,
                });
                if sink.is_none() {
                    match open_device_sink(device_lost.clone(), device_error.clone()) {
                        Ok(device) => sink = Some(device),
                        Err(error) => {
                            let _ = events.send(OutputEvent::Failed {
                                generation,
                                message: format!("could not open audio output device: {error}"),
                            });
                            continue;
                        }
                    }
                }
                let decoder_session = Arc::new(DecoderSession::default());
                decoder_session.set_startup_deadline(Some(startup_deadline));
                player = Some(start_playback_with_reserve_policy(
                    sink.as_ref().expect("sink opened").mixer(),
                    volume,
                    generation,
                    source,
                    position,
                    end,
                    position,
                    sink.as_ref()
                        .expect("sink opened")
                        .config()
                        .sample_rate()
                        .get(),
                    sink.as_ref()
                        .expect("sink opened")
                        .config()
                        .channel_count()
                        .get(),
                    events.clone(),
                    tap.clone(),
                    decoder_session,
                    false,
                    if allow_startup_reserve {
                        configured_startup_reserve_policy()
                    } else {
                        StartupReservePolicy::Disabled
                    },
                ));
            }
            Ok(OutputCommand::Preload {
                generation,
                next_generation,
                source,
                position,
                end,
            }) => {
                if let Some(current) = player.as_mut() {
                    if current.active_generation.load(Ordering::Relaxed) != current.generation {
                        while let Ok(activation) = current.activation_rx.try_recv() {
                            apply_activation(current, activation);
                        }
                    }
                    if current.active_generation.load(Ordering::Relaxed) == generation
                        && current.preload_cancel.is_none()
                    {
                        current.preload_cancel = Some(append_source(
                            &current.player,
                            next_generation,
                            source,
                            position,
                            end,
                            current.active_generation.clone(),
                            current.samples.clone(),
                            current.base_position.clone(),
                            current.sample_rate.load(Ordering::Relaxed),
                            current.channels.load(Ordering::Relaxed) as u16,
                            events.clone(),
                            tap.clone(),
                            current.activation_tx.clone(),
                            Arc::new(AtomicBool::new(false)),
                            current.buffering.clone(),
                        ));
                    }
                }
            }
            Ok(OutputCommand::ClearPreload { generation }) => {
                let mut interrupt = false;
                if let Some(current) = player.as_mut() {
                    while let Ok(activation) = current.activation_rx.try_recv() {
                        apply_activation(current, activation);
                    }
                    interrupt = current.active_generation.load(Ordering::Relaxed) != generation;
                    if let Some(cancel) = current.preload_cancel.take() {
                        if !Arc::ptr_eq(&cancel, &current.cancel) {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    }
                }
                if interrupt {
                    if let Some(current) = player.take() {
                        current.cancel.store(true, Ordering::Relaxed);
                        current.player.stop();
                    }
                    let _ = events.send(OutputEvent::Interrupted { generation });
                }
            }
            Ok(OutputCommand::Pause) => {
                if let Some(current) = player.as_mut() {
                    current.player.pause();
                    current.paused = true;
                } else if let Some(reconnect) = reconnect.as_mut() {
                    reconnect.paused = true;
                }
            }
            Ok(OutputCommand::Resume) => {
                if let Some(current) = player.as_mut() {
                    current.player.play();
                    current.paused = false;
                } else if let Some(reconnect) = reconnect.as_mut() {
                    reconnect.paused = false;
                }
            }
            Ok(OutputCommand::Stop) => {
                reconnect = None;
                if let Some(current) = player.take() {
                    current.cancel.store(true, Ordering::Relaxed);
                    if let Some(cancel) = current.preload_cancel {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    current.player.stop();
                }
            }
            Ok(OutputCommand::Volume(value)) => {
                volume = value.clamp(0.0, 1.0);
                if let Some(current) = player.as_ref() {
                    current.player.set_volume(volume);
                }
            }
            Ok(OutputCommand::Seek {
                generation,
                position: seconds,
            }) => {
                if let Some(current) = player.as_mut() {
                    if current.active_generation.load(Ordering::Relaxed) != current.generation {
                        while let Ok(activation) = current.activation_rx.try_recv() {
                            apply_activation(current, activation);
                        }
                    }
                }
                if let Some(current) = player.take() {
                    let was_paused = current.paused;
                    if let Some(cancel) = current.preload_cancel.as_ref() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    if sink.is_none() {
                        current.cancel.store(true, Ordering::Relaxed);
                        current.player.stop();
                        continue;
                    }
                    // Keep the download alive while retiring the old output source.
                    let handoff = Arc::new(AtomicBool::new(false));
                    // A seek after first audio is steady playback, not startup.
                    current.decoder_session.set_startup_deadline(None);
                    current.decoder_session.bind(&handoff);
                    current.cancel.store(true, Ordering::Relaxed);
                    current.player.stop();
                    let _ = events.send(OutputEvent::Buffering {
                        generation,
                        diagnostics: None,
                    });
                    let restarted = start_playback(
                        sink.as_ref().expect("checked sink").mixer(),
                        volume,
                        generation,
                        current.source.clone(),
                        seconds.max(0.0),
                        current.end,
                        seconds.max(0.0),
                        sink.as_ref()
                            .expect("checked sink")
                            .config()
                            .sample_rate()
                            .get(),
                        sink.as_ref()
                            .expect("checked sink")
                            .config()
                            .channel_count()
                            .get(),
                        events.clone(),
                        tap.clone(),
                        current.decoder_session.clone(),
                        was_paused,
                    );
                    player = Some(restarted);
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
        }
        if device_lost.swap(false, Ordering::Relaxed) {
            if let Some(current) = player.take() {
                let generation = current.active_generation.load(Ordering::Relaxed);
                let position = playback_position(&current);
                let reason = device_error
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                    .unwrap_or_else(|| "audio output stream was invalidated".into());
                eprintln!("native audio output disconnected: {reason}");
                let _ = events.send(OutputEvent::DeviceLost {
                    generation,
                    position,
                    reason: reason.clone(),
                });
                reconnect = Some(Reconnect {
                    generation,
                    source: current.source.clone(),
                    position,
                    end: current.end,
                    paused: current.paused,
                    attempts: 0,
                    last_error: reason,
                    retry_at: Instant::now() + reconnect_delay(0),
                });
                current.cancel.store(true, Ordering::Relaxed);
                if let Some(cancel) = current.preload_cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                current.player.stop();
            }
            sink = None;
        }
        if reconnect
            .as_ref()
            .is_some_and(|reconnect| Instant::now() >= reconnect.retry_at)
        {
            let pending = reconnect.as_mut().expect("reconnect checked");
            match open_device_sink(device_lost.clone(), device_error.clone()) {
                Ok(device) => {
                    let output_rate = device.config().sample_rate().get();
                    let output_channels = device.config().channel_count().get();
                    player = Some(start_playback(
                        device.mixer(),
                        volume,
                        pending.generation,
                        pending.source.clone(),
                        pending.position,
                        pending.end,
                        pending.position,
                        output_rate,
                        output_channels,
                        events.clone(),
                        tap.clone(),
                        Arc::new(DecoderSession::default()),
                        pending.paused,
                    ));
                    sink = Some(device);
                    reconnect = None;
                }
                Err(error) => {
                    pending.attempts += 1;
                    pending.last_error = error;
                    if pending.attempts >= MAX_RECONNECT_ATTEMPTS {
                        let _ = events.send(OutputEvent::Failed {
                            generation: pending.generation,
                            message: format!(
                                "could not reopen audio output device after {} attempts: {}",
                                pending.attempts, pending.last_error
                            ),
                        });
                        reconnect = None;
                    } else {
                        pending.retry_at = Instant::now() + reconnect_delay(pending.attempts);
                    }
                }
            }
        }
        if let Some(current) = player.as_ref() {
            if let Ok(activation) = current.activation_rx.try_recv() {
                let current = player.as_mut().expect("playback exists");
                apply_activation(current, activation);
            }
        }
        if let Some(current) = player.as_mut() {
            if let Some(event) = take_active_decode_failure(
                current.generation,
                &current.decoder_session,
                &current.cancel,
            ) {
                let _ = events.send(event);
            }
            let is_buffering = current.buffering.load(Ordering::Relaxed);
            if is_buffering && !current.buffering_reported {
                let _ = events.send(buffering_event(current));
                current.buffering_reported = true;
            } else if !is_buffering && current.buffering_reported {
                if let Some(metadata) = &current.live_metadata {
                    current.source_sample_rate = metadata.rate.load(Ordering::Acquire);
                    current.source_channels = metadata
                        .channels
                        .load(Ordering::Acquire)
                        .clamp(1, u16::MAX as u32)
                        as u16;
                    let duration = f64::from_bits(metadata.duration.load(Ordering::Acquire));
                    current.source_duration = duration.is_finite().then_some(duration);
                    current.decoder_diagnostics = metadata
                        .decoder
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                }
                let _ = events.send(OutputEvent::Started {
                    generation: current.active_generation.load(Ordering::Relaxed),
                    duration: current.source_duration,
                    sample_rate: current.source_sample_rate,
                    channels: current.source_channels,
                    decoder: current.decoder_diagnostics.clone(),
                });
                current.buffering_reported = false;
            }
            if !current.paused && last_position.elapsed() >= Duration::from_millis(250) {
                let samples = current.samples.load(Ordering::Relaxed);
                let seconds = f64::from_bits(current.base_position.load(Ordering::Relaxed))
                    + samples as f64
                        / (current.sample_rate.load(Ordering::Relaxed) as f64
                            * current.channels.load(Ordering::Relaxed) as f64);
                let _ = events.send(OutputEvent::Position {
                    generation: current.active_generation.load(Ordering::Relaxed),
                    seconds,
                });
                last_position = Instant::now();
            }
            if current.player.empty() {
                let generation = current.active_generation.load(Ordering::Relaxed);
                if !current.cancel.load(Ordering::Relaxed) {
                    if let Some(event) = active_storage_failure_event(
                        current.generation,
                        &current.decoder_session,
                        &current.cancel,
                    ) {
                        // A preload's first event can precede adoption. Re-publish its retained
                        // fault at the active selection boundary, even if decoding already failed.
                        current.decode_failed.store(true, Ordering::Release);
                        let _ = events.send(event);
                    }
                }
                if !current.decode_failed.load(Ordering::Relaxed) {
                    let _ = events.send(OutputEvent::Position {
                        generation,
                        seconds: playback_position(current),
                    });
                    let _ = events.send(OutputEvent::Ended { generation });
                }
                player = None;
            }
        }
    }
    if let Some(current) = player {
        current.cancel.store(true, Ordering::Relaxed);
        if let Some(cancel) = current.preload_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        current.player.stop();
    }
}

fn buffering_event(current: &Playback) -> OutputEvent {
    OutputEvent::Buffering {
        generation: current.active_generation.load(Ordering::Relaxed),
        diagnostics: Some(
            current
                .decoder_session
                .stream_stats
                .snapshot(current.samples.load(Ordering::Relaxed)),
        ),
    }
}

fn active_storage_failure_event(
    generation: u64,
    session: &DecoderSession,
    cancelled: &AtomicBool,
) -> Option<OutputEvent> {
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    session
        .storage_faults
        .get()
        .map(|fault| OutputEvent::LocalStorageFailed { generation, fault })
}

fn take_active_decode_failure(
    generation: u64,
    session: &DecoderSession,
    cancelled: &AtomicBool,
) -> Option<OutputEvent> {
    if cancelled.load(Ordering::Acquire) || session.storage_faults.get().is_some() {
        return None;
    }
    session
        .retained_failure
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .map(|message| OutputEvent::Failed {
            generation,
            message,
        })
}

fn decode_failure_event(
    generation: u64,
    message: String,
    faults: &storage_fault::Faults,
    cancelled: &AtomicBool,
) -> Option<OutputEvent> {
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    Some(faults.get().map_or_else(
        || OutputEvent::Failed {
            generation,
            message,
        },
        |fault| OutputEvent::LocalStorageFailed { generation, fault },
    ))
}

const MAX_RECONNECT_ATTEMPTS: u8 = 6;

fn reconnect_delay(attempt: u8) -> Duration {
    Duration::from_millis(250u64.saturating_mul(1u64 << u32::from(attempt.min(3))))
}

fn playback_position(current: &Playback) -> f64 {
    position_from_output_samples(
        f64::from_bits(current.base_position.load(Ordering::Relaxed)),
        current.samples.load(Ordering::Relaxed),
        current.sample_rate.load(Ordering::Relaxed),
        current.channels.load(Ordering::Relaxed),
    )
}

fn position_from_output_samples(base: f64, samples: u64, sample_rate: u32, channels: u32) -> f64 {
    base + samples as f64 / (sample_rate.max(1) as f64 * channels.max(1) as f64)
}

fn should_reconnect_for_stream_error(error: &rodio::cpal::StreamError) -> bool {
    !matches!(error, rodio::cpal::StreamError::BufferUnderrun)
}

fn open_device_sink(
    device_lost: Arc<AtomicBool>,
    device_error: Arc<std::sync::Mutex<Option<String>>>,
) -> Result<rodio::MixerDeviceSink, String> {
    DeviceSinkBuilder::from_default_device()
        .map_err(|error| error.to_string())?
        .with_error_callback(move |error| {
            if should_reconnect_for_stream_error(&error) {
                *device_error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.to_string());
                device_lost.store(true, Ordering::Relaxed);
            }
        })
        .open_stream()
        .map_err(|error| error.to_string())
}

fn apply_activation(current: &mut Playback, activation: Activation) {
    current.decoder_session = activation.decoder_session;
    current.generation = activation.generation;
    current.source = activation.source;
    current.position = activation.position;
    current.end = activation.end;
    current.cancel = activation.cancel.clone();
    current.decode_failed = activation.decode_failed.clone();
    current.buffering_reported = false;
    current.source_duration = activation.duration;
    current.decoder_diagnostics = activation.decoder_diagnostics;
    current.source_sample_rate = activation.sample_rate;
    current.source_channels = activation.channels;
    current.live_metadata = Some(activation.live_metadata);
    if current
        .preload_cancel
        .as_ref()
        .is_some_and(|queued| Arc::ptr_eq(queued, &activation.cancel))
    {
        current.preload_cancel = None;
    }
}

fn start_playback(
    mixer: &rodio::mixer::Mixer,
    volume: f32,
    generation: u64,
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    base_position: f64,
    output_rate: u32,
    output_channels: u16,
    events: UnboundedSender<OutputEvent>,
    tap: Arc<crate::pcm_tap::PcmTap>,
    decoder_session: Arc<DecoderSession>,
    paused: bool,
) -> Playback {
    start_playback_with_reserve_policy(
        mixer,
        volume,
        generation,
        source,
        position,
        end,
        base_position,
        output_rate,
        output_channels,
        events,
        tap,
        decoder_session,
        paused,
        StartupReservePolicy::Disabled,
    )
}

fn start_playback_with_reserve_policy(
    mixer: &rodio::mixer::Mixer,
    volume: f32,
    generation: u64,
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    base_position: f64,
    output_rate: u32,
    output_channels: u16,
    events: UnboundedSender<OutputEvent>,
    tap: Arc<crate::pcm_tap::PcmTap>,
    decoder_session: Arc<DecoderSession>,
    paused: bool,
    reserve_policy: StartupReservePolicy,
) -> Playback {
    let player = Player::connect_new(mixer);
    player.set_volume(volume);
    if paused {
        player.pause();
    }
    let reserve_target = startup_reserve_target(
        &source,
        position,
        end,
        paused,
        output_rate,
        output_channels,
        reserve_policy,
    );
    let (sample_tx, sample_rx) = std_mpsc::sync_channel(16);
    let samples = Arc::new(AtomicU64::new(0));
    // The queue and clock use the immutable output-device format. Decoder metadata
    // lives separately because a source may begin at a different rate/channel count.
    let sample_rate = Arc::new(std::sync::atomic::AtomicU32::new(output_rate));
    let channels = Arc::new(std::sync::atomic::AtomicU32::new(output_channels as u32));
    let source_rate = Arc::new(std::sync::atomic::AtomicU32::new(output_rate));
    let source_channels = Arc::new(std::sync::atomic::AtomicU32::new(output_channels as u32));
    let cancel = Arc::new(AtomicBool::new(false));
    decoder_session.bind(&cancel);
    let worker_session = decoder_session.clone();
    let (activation_tx, activation_rx) = std_mpsc::channel();
    let buffering = Arc::new(AtomicBool::new(false));
    let decode_failed = Arc::new(AtomicBool::new(false));
    let active_generation = Arc::new(AtomicU64::new(generation));
    let base_position_bits = Arc::new(AtomicU64::new(base_position.to_bits()));
    let duration_bits = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    let decoder_diagnostics = Arc::new(std::sync::Mutex::new(None));
    let failed = decode_failed.clone();
    let (sw, sr, ch, dur, decoder_info, ev, cc, src) = (
        samples.clone(),
        source_rate.clone(),
        source_channels.clone(),
        duration_bits.clone(),
        decoder_diagnostics.clone(),
        events.clone(),
        cancel.clone(),
        source.clone(),
    );
    let decode_cancelled = cc.clone();
    let storage_faults = worker_session.storage_faults.clone();
    let spawned = thread::Builder::new()
        .name("burrata-audio-decode".into())
        .spawn(move || {
            #[cfg(test)]
            struct Finished(Arc<DecoderSession>);
            #[cfg(test)]
            impl Drop for Finished {
                fn drop(&mut self) {
                    self.0.decode_thread_finished.store(true, Ordering::Release);
                }
            }
            #[cfg(test)]
            let _finished = Finished(worker_session.clone());
            let _reserve_completion = reserve_target.map(|_| ReserveProducerCompletion {
                _sender: sample_tx.clone(),
                failed: failed.clone(),
                cancelled: decode_cancelled.clone(),
                events: ev.clone(),
                generation,
            });
            if let Err(message) = decode_with_session_and_diagnostics(
                src,
                position,
                end,
                output_rate,
                output_channels,
                sample_tx,
                sw,
                sr,
                ch,
                dur,
                Some(decoder_info),
                cc,
                worker_session,
            ) {
                if !decode_cancelled.load(Ordering::Relaxed) {
                    if !failed.swap(true, Ordering::AcqRel) {
                        if let Some(event) = decode_failure_event(
                            generation,
                            message,
                            &storage_faults,
                            &decode_cancelled,
                        ) {
                            let _ = ev.send(event);
                        }
                    }
                }
            }
        });
    if let Err(error) = spawned {
        decode_failed.store(true, Ordering::Relaxed);
        let _ = events.send(OutputEvent::Failed {
            generation,
            message: error.to_string(),
        });
    }
    player.append(QueuedSamples {
        reserve: reserve_target.map(StartupPcmReserve::new),
        reserve_diagnostics: None,
        decoder_session: decoder_session.clone(),
        rx: sample_rx,
        pending: VecDeque::new(),
        channels: output_channels,
        sample_rate: output_rate,
        samples_played: samples.clone(),
        generation,
        active_generation: active_generation.clone(),
        base_position: base_position_bits.clone(),
        initial_position: base_position,
        started: false,
        activated: false,
        cancelled: cancel.clone(),
        source: source.clone(),
        end,
        activation_tx: activation_tx.clone(),
        tap_writer: tap.writer(generation, output_rate, output_channels),
        events: events.clone(),
        duration_bits,
        decoder_diagnostics: decoder_diagnostics.clone(),
        source_rate,
        source_channels,
        decode_failed: decode_failed.clone(),
        buffering: buffering.clone(),
        exhausted: false,
    });
    if !paused {
        player.play();
    }
    Playback {
        decoder_session,
        generation,
        player,
        samples,
        sample_rate,
        channels,
        source,
        end,
        position: base_position,
        cancel,
        decode_failed,
        active_generation,
        base_position: base_position_bits,
        preload_cancel: None,
        activation_rx,
        activation_tx,
        buffering,
        buffering_reported: false,
        source_duration: None,
        decoder_diagnostics: None,
        source_sample_rate: 44_100,
        source_channels: 2,
        live_metadata: None,
        paused,
    }
}

fn append_source(
    player: &Player,
    generation: u64,
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    active_generation: Arc<AtomicU64>,
    samples_played: Arc<AtomicU64>,
    base_position: Arc<AtomicU64>,
    output_rate: u32,
    output_channels: u16,
    events: UnboundedSender<OutputEvent>,
    tap: Arc<crate::pcm_tap::PcmTap>,
    activation_tx: std_mpsc::Sender<Activation>,
    decode_failed: Arc<AtomicBool>,
    buffering: Arc<AtomicBool>,
) -> Arc<AtomicBool> {
    let (tx, rx) = std_mpsc::sync_channel(16);
    let cancel = Arc::new(AtomicBool::new(false));
    let duration_bits = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    let decoder_diagnostics = Arc::new(std::sync::Mutex::new(None));
    let rate = Arc::new(std::sync::atomic::AtomicU32::new(output_rate));
    let channels = Arc::new(std::sync::atomic::AtomicU32::new(output_channels as u32));
    let decoder_session = Arc::new(DecoderSession::default());
    decoder_session.bind(&cancel);
    let worker_session = decoder_session.clone();
    let queue_source = source.clone();
    let failed = decode_failed.clone();
    let (cc, src, dur, decoder_info, rate_writer, channels_writer) = (
        cancel.clone(),
        source,
        duration_bits.clone(),
        decoder_diagnostics.clone(),
        rate.clone(),
        channels.clone(),
    );
    let decode_cancelled = cc.clone();
    let storage_faults = worker_session.storage_faults.clone();
    let failure_session = decoder_session.clone();
    // Keep the channel connected until the retained error/failed flag is
    // published. Otherwise a zero-PCM failure can race source activation.
    let producer_lifetime = tx.clone();
    let spawned = thread::Builder::new()
        .name("burrata-audio-preload".into())
        .spawn(move || {
            let _producer_lifetime = producer_lifetime;
            if let Err(message) = decode_with_session_and_diagnostics(
                src,
                position,
                end,
                output_rate,
                output_channels,
                tx,
                Arc::new(AtomicU64::new(0)),
                rate_writer,
                channels_writer,
                dur,
                Some(decoder_info),
                cc,
                worker_session,
            ) {
                if !decode_cancelled.load(Ordering::Relaxed) {
                    if storage_faults.get().is_none() {
                        *failure_session
                            .retained_failure
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(message);
                    }
                    if !failed.swap(true, Ordering::AcqRel) {
                        // All preload faults stay with the queued reader until
                        // selection/activation, not just local storage faults.
                    }
                }
            }
        });
    if let Err(error) = spawned {
        *decoder_session
            .retained_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            Some(format!("Could not start the next-track decoder: {error}"));
        decode_failed.store(true, Ordering::Release);
    }
    player.append(QueuedSamples {
        reserve: None,
        reserve_diagnostics: None,
        decoder_session,
        rx,
        pending: VecDeque::new(),
        channels: output_channels,
        sample_rate: output_rate,
        samples_played,
        generation,
        active_generation,
        base_position,
        initial_position: position,
        started: false,
        activated: false,
        cancelled: cancel.clone(),
        tap_writer: tap.writer(generation, output_rate, output_channels),
        events,
        duration_bits,
        decoder_diagnostics,
        source_rate: rate,
        source_channels: channels,
        decode_failed,
        source: queue_source,
        end,
        activation_tx,
        buffering,
        exhausted: false,
    });
    cancel
}

struct QueuedSamples {
    reserve: Option<StartupPcmReserve>,
    reserve_diagnostics: Option<StartupReserveDiagnostics>,
    decoder_session: Arc<DecoderSession>,
    rx: std_mpsc::Receiver<Vec<f32>>,
    pending: VecDeque<f32>,
    channels: u16,
    sample_rate: u32,
    samples_played: Arc<AtomicU64>,
    generation: u64,
    active_generation: Arc<AtomicU64>,
    base_position: Arc<AtomicU64>,
    initial_position: f64,
    started: bool,
    activated: bool,
    cancelled: Arc<AtomicBool>,
    tap_writer: crate::pcm_tap::PcmTapWriter,
    events: UnboundedSender<OutputEvent>,
    duration_bits: Arc<AtomicU64>,
    decoder_diagnostics: Arc<std::sync::Mutex<Option<DecoderDiagnostics>>>,
    source_rate: Arc<std::sync::atomic::AtomicU32>,
    source_channels: Arc<std::sync::atomic::AtomicU32>,
    decode_failed: Arc<AtomicBool>,
    source: AudioSource,
    end: Option<f64>,
    activation_tx: std_mpsc::Sender<Activation>,
    buffering: Arc<AtomicBool>,
    exhausted: bool,
}
impl Iterator for QueuedSamples {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.cancelled.load(Ordering::Relaxed) {
            self.exhausted = true;
            return None;
        }
        // Selection is the mixer reaching this occurrence, not first PCM. This
        // transfers ownership of a buffering preload so its startup is bounded.
        if !self.activated && self.active_generation.load(Ordering::Acquire) != self.generation {
            self.activate();
        }
        if let Some(reserve) = self.reserve.as_mut() {
            if !reserve.ready {
                match reserve.fill(&self.rx, &self.decode_failed) {
                    ReserveFill::Waiting => {
                        self.decoder_session
                            .stream_stats
                            .synthetic_silence_samples
                            .fetch_add(1, Ordering::Relaxed);
                        self.buffering.store(true, Ordering::Relaxed);
                        return Some(0.0);
                    }
                    ReserveFill::Failed => {
                        if self.decode_failed.load(Ordering::Acquire) && !self.activated {
                            self.activate();
                        }
                        self.exhausted = true;
                        return None;
                    }
                    ReserveFill::Ready => {
                        self.reserve_diagnostics = Some(StartupReserveDiagnostics {
                            target_samples: reserve.target,
                            released_samples: reserve.samples,
                            short_clean_eof: reserve.short_clean_eof,
                        });
                    }
                }
            }
            // Cancellation wins even when a reserve becomes ready in this callback turn.
            if self.cancelled.load(Ordering::Acquire) {
                self.exhausted = true;
                return None;
            }
            if self.pending.is_empty() {
                if let Some(block) = reserve.blocks.pop_front() {
                    self.pending = block.into();
                }
            }
        }
        if let Some(value) = self.pending.pop_front() {
            if !self.started {
                self.begin_track();
            }
            self.samples_played.fetch_add(1, Ordering::Relaxed);
            self.buffering.store(false, Ordering::Relaxed);
            self.tap_writer.push(value);
            return Some(value);
        }
        if self.exhausted {
            return None;
        }
        match self.rx.try_recv() {
            Ok(block) => {
                self.pending.extend(block);
                let value = self.pending.pop_front();
                if value.is_some() {
                    if !self.started {
                        self.begin_track();
                    }
                    self.samples_played.fetch_add(1, Ordering::Relaxed);
                    self.buffering.store(false, Ordering::Relaxed);
                    if let Some(sample) = value {
                        self.tap_writer.push(sample);
                    }
                }
                value
            }
            Err(std_mpsc::TryRecvError::Empty) => {
                self.decoder_session
                    .stream_stats
                    .synthetic_silence_samples
                    .fetch_add(1, Ordering::Relaxed);
                self.buffering.store(true, Ordering::Relaxed);
                Some(0.0)
            }
            Err(std_mpsc::TryRecvError::Disconnected) => {
                if !self.activated && self.decode_failed.load(Ordering::Acquire) {
                    // The mixer has reached this occurrence, but no PCM was produced.
                    // Activate ownership only; never publish Started/first-audio for this case.
                    self.activate();
                }
                self.exhausted = true;
                None
            }
        }
    }
}
impl QueuedSamples {
    fn start_diagnostics(&self) -> Option<DecoderDiagnostics> {
        self.decoder_diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .map(|mut value| {
                value.startup_reserve = self.reserve_diagnostics.clone();
                value
            })
    }
    fn activate(&mut self) {
        self.active_generation
            .store(self.generation, Ordering::Relaxed);
        self.base_position
            .store(self.initial_position.to_bits(), Ordering::Relaxed);
        self.samples_played.store(0, Ordering::Relaxed);
        self.activated = true;
        let _ = self.activation_tx.send(Activation {
            decoder_session: self.decoder_session.clone(),
            generation: self.generation,
            source: self.source.clone(),
            position: self.initial_position,
            end: self.end,
            cancel: self.cancelled.clone(),
            sample_rate: self.source_rate.load(Ordering::Relaxed),
            channels: self
                .source_channels
                .load(Ordering::Relaxed)
                .clamp(1, u16::MAX as u32) as u16,
            duration: {
                let duration = f64::from_bits(self.duration_bits.load(Ordering::Relaxed));
                duration.is_finite().then_some(duration)
            },
            decoder_diagnostics: self.start_diagnostics(),
            decode_failed: self.decode_failed.clone(),
            live_metadata: ActiveMetadata {
                rate: self.source_rate.clone(),
                channels: self.source_channels.clone(),
                duration: self.duration_bits.clone(),
                decoder: self.decoder_diagnostics.clone(),
            },
        });
    }
    fn begin_track(&mut self) {
        if !self.activated {
            self.activate();
        }
        self.started = true;
        let duration = f64::from_bits(self.duration_bits.load(Ordering::Relaxed));
        let _ = self.events.send(OutputEvent::Started {
            generation: self.generation,
            duration: duration.is_finite().then_some(duration),
            sample_rate: self.source_rate.load(Ordering::Relaxed),
            channels: self
                .source_channels
                .load(Ordering::Relaxed)
                .clamp(1, u16::MAX as u32) as u16,
            decoder: self.start_diagnostics(),
        });
    }
}
impl Source for QueuedSamples {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> NonZeroU16 {
        NonZeroU16::new(self.channels).unwrap()
    }
    fn sample_rate(&self) -> NonZeroU32 {
        NonZeroU32::new(self.sample_rate).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
fn decode_to_blocks(
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    output_rate: u32,
    output_channels: u16,
    tx: std_mpsc::SyncSender<Vec<f32>>,
    samples: Arc<AtomicU64>,
    sample_rate_out: Arc<std::sync::atomic::AtomicU32>,
    channels_out: Arc<std::sync::atomic::AtomicU32>,
    duration_out: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
) -> Result<(), String> {
    let session = Arc::new(DecoderSession::default());
    session.bind(&cancelled);
    decode_with_session(
        source,
        position,
        end,
        output_rate,
        output_channels,
        tx,
        samples,
        sample_rate_out,
        channels_out,
        duration_out,
        cancelled,
        session,
    )
}

fn decode_with_session(
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    output_rate: u32,
    output_channels: u16,
    tx: std_mpsc::SyncSender<Vec<f32>>,
    samples: Arc<AtomicU64>,
    sample_rate_out: Arc<std::sync::atomic::AtomicU32>,
    channels_out: Arc<std::sync::atomic::AtomicU32>,
    duration_out: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    session: Arc<DecoderSession>,
) -> Result<(), String> {
    decode_with_session_and_diagnostics(
        source,
        position,
        end,
        output_rate,
        output_channels,
        tx,
        samples,
        sample_rate_out,
        channels_out,
        duration_out,
        None,
        cancelled,
        session,
    )
}

fn decode_with_session_and_diagnostics(
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    output_rate: u32,
    output_channels: u16,
    tx: std_mpsc::SyncSender<Vec<f32>>,
    samples: Arc<AtomicU64>,
    sample_rate_out: Arc<std::sync::atomic::AtomicU32>,
    channels_out: Arc<std::sync::atomic::AtomicU32>,
    duration_out: Arc<AtomicU64>,
    diagnostics_out: Option<Arc<std::sync::Mutex<Option<DecoderDiagnostics>>>>,
    cancelled: Arc<AtomicBool>,
    session: Arc<DecoderSession>,
) -> Result<(), String> {
    let decode_started = Instant::now();
    let reader_bytes_at_start = session.stream_stats.bytes_read.load(Ordering::Relaxed);
    let use_ffmpeg = should_use_ffmpeg_for_source(&source);
    #[cfg(test)]
    let use_ffmpeg = {
        let valid_loopback_opus = has_finite_opus_webm_hint(&source)
            && url::Url::parse(&source.input).ok().is_some_and(|url| {
                url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
            });
        let legacy_opus = session.force_legacy_opus_probe.load(Ordering::Relaxed)
            && (is_finite_remote_opus_webm(&source) || valid_loopback_opus);
        let loopback_opus =
            session.force_loopback_opus_ffmpeg.load(Ordering::Relaxed) && valid_loopback_opus;
        (use_ffmpeg || loopback_opus) && !legacy_opus
    };
    if use_ffmpeg {
        let hint = source.decoder_hint.as_ref();
        let duration = hint
            .and_then(|hint| hint.duration_seconds)
            .filter(|value| value.is_finite() && *value > 0.0);
        duration_out.store(duration.unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
        sample_rate_out.store(output_rate, Ordering::Relaxed);
        channels_out.store(output_channels as u32, Ordering::Relaxed);
        if let Some(diagnostics_out) = diagnostics_out.as_ref() {
            *diagnostics_out.lock().unwrap_or_else(|e| e.into_inner()) = Some(DecoderDiagnostics {
                startup_reserve: None,
                failed_native_attempt: None,
                decoder_kind: "ffmpeg".into(),
                codec_hint: hint.and_then(|hint| hint.codec.clone()),
                content_type: hint.and_then(|hint| hint.mime_type.clone()),
                input_content_length: hint.and_then(|hint| hint.content_length),
                content_range_total: None,
                bytes_read: 0,
                bytes_received: 0,
                sample_rate: output_rate,
                channels: output_channels,
                total_duration_seconds: duration,
                reader_setup_ms: None,
                decoder_setup_ms: None,
                first_decoded_block_ms: None,
                reader_reused: false,
                decoder_reused: false,
            });
        }
        return decode_ffmpeg_with_diagnostics(
            source,
            position,
            end,
            output_rate,
            output_channels,
            tx,
            samples,
            sample_rate_out,
            channels_out,
            duration_out,
            cancelled,
            diagnostics_out,
            decode_started,
        );
    }
    #[cfg(all(test, feature = "experimental-opus"))]
    if std::env::var_os("BURRATA_FORCE_FFMPEG").is_some() {
        prepare_ffmpeg_fallback_diagnostics(
            diagnostics_out.as_ref(),
            &source,
            output_rate,
            output_channels,
            None,
        );
        return decode_ffmpeg_with_diagnostics(
            source,
            position,
            end,
            output_rate,
            output_channels,
            tx,
            samples,
            sample_rate_out,
            channels_out,
            duration_out,
            cancelled,
            diagnostics_out.clone(),
            decode_started,
        );
    }
    let mut cached = match session.lock_decoder(&cancelled) {
        Ok(decoder) => decoder,
        Err(_) if cancelled.load(Ordering::Relaxed) => return Ok(()),
        Err(error) => return Err(error),
    };
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    if session.storage_faults.get().is_some() {
        return Err("Local playback storage failed".into());
    }
    session
        .stream_stats
        .pcm_produced_samples
        .store(0, Ordering::Relaxed);
    session
        .stream_stats
        .synthetic_silence_samples
        .store(0, Ordering::Relaxed);
    let reused = cached.is_some();
    let mut reader_setup_ms = None;
    let mut decoder_setup_ms = None;
    let mut source_stream_info: Option<(
        Option<u64>,
        Option<String>,
        Option<u64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
    )> = None;
    if cached.is_none() {
        let ffmpeg_fallback = source.clone();
        let input = source.input.clone();
        let bytes_read = Arc::new(AtomicU64::new(0));
        let reader_setup_started = Instant::now();
        let (reader, byte_len, content_type, range_total, received): (
            Box<dyn ReadSeekSend>,
            Option<u64>,
            Option<String>,
            Option<u64>,
            Arc<AtomicU64>,
        ) = if input.starts_with("http://") || input.starts_with("https://") {
            let rt = network_runtime()?;
            let uri = reqwest::Url::parse(&input).map_err(|e| e.to_string())?;
            let headers = source.headers.clone();
            let storage_faults = session.storage_faults.clone();
            let stream_stats = session.stream_stats.clone();
            stream_stats.network_reader.store(true, Ordering::Relaxed);
            let reader = await_remote_reader_setup(
                rt,
                cancelled.clone(),
                session.startup_deadline(),
                async move {
                    use stream_download::http::reqwest as stream_reqwest;
                    use stream_download::{
                        http::HttpStream,
                        storage::{adaptive::AdaptiveStorageProvider, temp::TempStorageProvider},
                        Settings, StreamDownload,
                    };
                    let mut builder = stream_reqwest::Client::builder();
                    // Test fixtures must not route literal loopback requests through host proxies.
                    #[cfg(test)]
                    if matches!(uri.host_str(), Some("127.0.0.1") | Some("[::1]")) {
                        builder = builder.no_proxy();
                    }
                    let mut map = stream_reqwest::header::HeaderMap::new();
                    for (name, value) in headers {
                        let name = stream_reqwest::header::HeaderName::from_bytes(name.as_bytes())
                            .map_err(|e| e.to_string())?;
                        let value = stream_reqwest::header::HeaderValue::from_str(&value)
                            .map_err(|e| e.to_string())?;
                        map.insert(name, value);
                    }
                    builder = builder
                        .connect_timeout(Duration::from_secs(10))
                        .default_headers(map);
                    let client = builder.build().map_err(|e| e.to_string())?;
                    let uri =
                        stream_reqwest::Url::parse(uri.as_str()).map_err(|e| e.to_string())?;
                    let stream = HttpStream::new(native_http::NativeHttpClient::new(client), uri)
                        .await
                        .map_err(|e| e.to_string())?;
                    let content_type = stream
                        .content_type()
                        .as_ref()
                        .map(|mime| format!("{}/{}", mime.r#type, mime.subtype));
                    let range_total = content_range_total(stream.header("content-range"));
                    let received = Arc::new(AtomicU64::new(0));
                    let progress = received.clone();
                    // Live radio must not grow a temporary file for the entire listening session.
                    let storage = AdaptiveStorageProvider::new(
                        storage_fault::Observed {
                            inner: TempStorageProvider::with_tempfile_builder({
                                let faults = storage_faults.clone();
                                move || {
                                    let result =
                                        stream_download::storage::temp::tempfile::Builder::new()
                                            .tempfile();
                                    if let Err(error) = &result {
                                        faults.record(storage_fault::Operation::Create, error);
                                    }
                                    result
                                }
                            }),
                            faults: storage_faults,
                        },
                        std::num::NonZeroUsize::new(2 * 1024 * 1024).unwrap(),
                    );
                    let reader = StreamDownload::from_stream(
                        stream,
                        storage,
                        Settings::default().prefetch_bytes(32 * 1024).on_progress(
                            move |_stream, state, _| {
                                progress.fetch_max(state.current_chunk.end, Ordering::Relaxed);
                                let end = state.current_chunk.end;
                                let old = stream_stats
                                    .downloaded_range_end
                                    .swap(end, Ordering::Relaxed);
                                if end != old {
                                    stream_stats
                                        .last_progress
                                        .store(stream_stats.ms() + 1, Ordering::Relaxed);
                                }
                            },
                        ),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    let byte_len = reader.content_length();
                    Ok::<_, String>((reader, byte_len, content_type, range_total, received))
                },
            )?;
            let (reader, byte_len, content_type, range_total, received) = reader;
            let download_cancel = reader.cancellation_token();
            let abort = download_cancel.clone();
            let finished = download_cancel.clone();
            register_download_cancellation(
                rt,
                &session,
                move || abort.cancel(),
                move || finished.is_cancelled(),
            );
            (
                Box::new(reader),
                byte_len,
                content_type,
                range_total,
                received,
            )
        } else {
            let file = File::open(&input).map_err(|e| format!("could not open audio file: {e}"))?;
            let byte_len = file.metadata().ok().map(|metadata| metadata.len());
            (Box::new(file), byte_len, None, None, bytes_read.clone())
        };
        reader_setup_ms = Some(reader_setup_started.elapsed().as_millis() as u64);
        let counted_reader: Box<dyn ReadSeekSend> = Box::new(TimedStreamReader {
            inner: CountingReader {
                inner: reader,
                bytes_read: bytes_read.clone(),
            },
            stats: session.stream_stats.clone(),
        });
        source_stream_info = Some((byte_len, content_type, range_total, bytes_read, received));
        let decoder_setup_started = Instant::now();
        *cached = Some(
            match build_native_decoder(BufReader::new(counted_reader), byte_len) {
                Ok(decoder) => decoder,
                Err(error) => {
                    if session.storage_faults.get().is_some() {
                        return Err("Local playback storage failed".into());
                    }
                    prepare_ffmpeg_fallback_diagnostics(
                        diagnostics_out.as_ref(),
                        &ffmpeg_fallback,
                        output_rate,
                        output_channels,
                        Some(failed_native_attempt(
                            FailedNativeAttemptReason::DecoderSetup,
                            &session,
                            decode_started,
                            reader_setup_ms,
                            Some(decoder_setup_started.elapsed().as_millis() as u64),
                            reader_bytes_at_start,
                        )),
                    );
                    return decode_ffmpeg_with_diagnostics(
                        ffmpeg_fallback,
                        position,
                        end,
                        output_rate,
                        output_channels,
                        tx,
                        samples,
                        sample_rate_out,
                        channels_out,
                        duration_out,
                        cancelled,
                        diagnostics_out.clone(),
                        decode_started,
                    )
                    .map_err(|fallback| {
                        format!(
                    "audio decoder rejected input ({error}); ffmpeg fallback failed: {fallback}"
                )
                    });
                }
            },
        );
        decoder_setup_ms = Some(decoder_setup_started.elapsed().as_millis() as u64);
    }
    let decoder = cached.as_mut().expect("decoder initialized");
    let decoder_failure = decoder.failure_handle();
    let sample_rate = decoder.sample_rate().get();
    let channels = decoder.channels().get();
    let duration = decoder.total_duration().map(|v| v.as_secs_f64());
    if let Some(diagnostics_out) = diagnostics_out.as_ref() {
        let decoder_kind = match decoder {
            NativeDecoder::Rodio(_) => "rodio",
            #[cfg(feature = "experimental-opus")]
            NativeDecoder::Opus(_) => "experimental-opus",
        };
        let mut diagnostics = if reused {
            session
                .cached_diagnostics
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(|| DecoderDiagnostics {
                    startup_reserve: None,
                    failed_native_attempt: None,
                    decoder_kind: decoder_kind.to_owned(),
                    codec_hint: source
                        .decoder_hint
                        .as_ref()
                        .and_then(|hint| hint.codec.clone()),
                    content_type: source
                        .decoder_hint
                        .as_ref()
                        .and_then(|hint| hint.mime_type.clone()),
                    input_content_length: source
                        .decoder_hint
                        .as_ref()
                        .and_then(|hint| hint.content_length),
                    content_range_total: None,
                    bytes_read: 0,
                    bytes_received: 0,
                    sample_rate,
                    channels,
                    total_duration_seconds: duration,
                    reader_setup_ms: None,
                    decoder_setup_ms: None,
                    first_decoded_block_ms: None,
                    reader_reused: true,
                    decoder_reused: true,
                })
        } else {
            let (content_length, content_type, range_total, bytes_read, bytes_received) =
                source_stream_info.as_ref().map_or(
                    (
                        source
                            .decoder_hint
                            .as_ref()
                            .and_then(|hint| hint.content_length),
                        source
                            .decoder_hint
                            .as_ref()
                            .and_then(|hint| hint.mime_type.clone()),
                        None,
                        0,
                        0,
                    ),
                    |(length, kind, total, read, received)| {
                        (
                            *length,
                            kind.clone(),
                            *total,
                            read.load(Ordering::Relaxed),
                            received.load(Ordering::Relaxed),
                        )
                    },
                );
            DecoderDiagnostics {
                startup_reserve: None,
                failed_native_attempt: None,
                decoder_kind: decoder_kind.to_owned(),
                codec_hint: source
                    .decoder_hint
                    .as_ref()
                    .and_then(|hint| hint.codec.clone()),
                content_type,
                input_content_length: content_length,
                content_range_total: range_total,
                bytes_read,
                bytes_received,
                sample_rate,
                channels,
                total_duration_seconds: duration,
                reader_setup_ms,
                decoder_setup_ms,
                first_decoded_block_ms: None,
                reader_reused: false,
                decoder_reused: false,
            }
        };
        diagnostics.reader_reused = reused;
        diagnostics.decoder_reused = reused;
        // Do not expose the previous decode's first-block time while this seek warms up.
        diagnostics.first_decoded_block_ms = None;
        diagnostics.reader_setup_ms = reader_setup_ms;
        diagnostics.decoder_setup_ms = decoder_setup_ms;
        *session
            .cached_diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(diagnostics.clone());
        *diagnostics_out.lock().unwrap_or_else(|e| e.into_inner()) = Some(diagnostics);
    }
    let seek_error = if reused || position > 0.0 {
        decoder.try_seek(Duration::from_secs_f64(position)).err()
    } else {
        None
    };
    if let Some(seek_error) = seek_error {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if session.storage_faults.get().is_some() {
            return Err("Local playback storage failed".into());
        }
        let seek_reason = seek_error
            .to_string()
            .split_whitespace()
            .map(|part| {
                if part.contains("http://")
                    || part.contains("https://")
                    || part.contains("file://")
                    || part.trim_start_matches(['(', '\"', '\'']).starts_with('/')
                {
                    "[source]"
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(250)
            .collect::<String>();
        *cached = None;
        drop(cached);
        prepare_ffmpeg_fallback_diagnostics(
            diagnostics_out.as_ref(),
            &source,
            output_rate,
            output_channels,
            Some(failed_native_attempt(
                FailedNativeAttemptReason::Seek,
                &session,
                decode_started,
                reader_setup_ms,
                decoder_setup_ms,
                reader_bytes_at_start,
            )),
        );
        return decode_ffmpeg_with_diagnostics(
            source,
            position,
            end,
            output_rate,
            output_channels,
            tx,
            samples,
            sample_rate_out,
            channels_out,
            duration_out,
            cancelled,
            diagnostics_out.clone(),
            decode_started,
        )
        .map_err(|error| {
            format!("decoder could not seek to {position:.3}s ({seek_reason}); ffmpeg fallback failed: {error}")
        });
    }
    duration_out.store(duration.unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
    sample_rate_out.store(sample_rate, Ordering::Relaxed);
    channels_out.store(channels as u32, Ordering::Relaxed);
    let decoder = rodio::source::UniformSourceIterator::new(
        decoder,
        NonZeroU16::new(output_channels).unwrap(),
        NonZeroU32::new(output_rate).unwrap(),
    );
    let mut decoded_samples =
        (position.max(0.0) * output_rate as f64 * output_channels as f64) as u64;
    let mut sent_samples = 0u64;
    let mut block = Vec::with_capacity(4096);
    let mut first_block_recorded = false;
    for sample in decoder {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if end.is_some_and(|end| {
            decoded_samples as f64 / (output_rate as f64 * output_channels as f64) >= end
        }) {
            break;
        }
        block.push(sample);
        decoded_samples += 1;
        if block.len() == 4096 {
            if session.storage_faults.get().is_some() {
                return Err("Local playback storage failed".into());
            }
            sent_samples += block.len() as u64;
            if !first_block_recorded {
                if let Some(diagnostics_out) = diagnostics_out.as_ref() {
                    if let Some(diagnostics) = diagnostics_out
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_mut()
                    {
                        diagnostics.first_decoded_block_ms =
                            Some(decode_started.elapsed().as_millis() as u64);
                    }
                }
                first_block_recorded = true;
            }
            if tx.send(std::mem::take(&mut block)).is_err() {
                return Ok(());
            }
            session
                .stream_stats
                .pcm_produced_samples
                .store(sent_samples, Ordering::Relaxed);
            block = Vec::with_capacity(4096);
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    if session.storage_faults.get().is_some() {
        return Err("Local playback storage failed".into());
    }
    if let Some(error) =
        decoder_failure.and_then(|failure| failure.lock().unwrap_or_else(|e| e.into_inner()).take())
    {
        *cached = None;
        drop(cached);
        let fallback_position =
            position.max(0.0) + sent_samples as f64 / (output_rate as f64 * output_channels as f64);
        prepare_ffmpeg_fallback_diagnostics(
            diagnostics_out.as_ref(),
            &source,
            output_rate,
            output_channels,
            Some(failed_native_attempt(
                FailedNativeAttemptReason::Decode,
                &session,
                decode_started,
                reader_setup_ms,
                decoder_setup_ms,
                reader_bytes_at_start,
            )),
        );
        return decode_ffmpeg_with_diagnostics(
            source,
            fallback_position,
            end,
            output_rate,
            output_channels,
            tx,
            samples,
            sample_rate_out,
            channels_out,
            duration_out,
            cancelled,
            diagnostics_out.clone(),
            decode_started,
        )
        .map_err(|fallback| {
            format!("Opus decoder failed ({error}); ffmpeg fallback failed: {fallback}")
        });
    }
    if !block.is_empty() {
        if !first_block_recorded {
            if let Some(diagnostics_out) = diagnostics_out.as_ref() {
                if let Some(diagnostics) = diagnostics_out
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                {
                    diagnostics.first_decoded_block_ms =
                        Some(decode_started.elapsed().as_millis() as u64);
                }
            }
        }
        let _ = tx.send(block);
    }
    let _ = (samples, source.ffmpeg);
    Ok(())
}

fn is_finite_remote_aac_mp4(source: &AudioSource) -> bool {
    (source.input.starts_with("https://") || source.input.starts_with("http://"))
        && source.decoder_hint.as_ref().is_some_and(|hint| {
            !hint.has_video
                && hint.mime_type.as_deref() == Some("audio/mp4")
                && hint
                    .duration_seconds
                    .is_some_and(|duration| duration.is_finite() && duration > 0.)
                && hint.content_length.is_some_and(|length| length > 0)
                && hint
                    .codec
                    .as_deref()
                    .is_some_and(|codec| codec == "aac" || codec.starts_with("mp4a.40."))
        })
}

fn has_finite_opus_webm_hint(source: &AudioSource) -> bool {
    source.decoder_hint.as_ref().is_some_and(|hint| {
        !hint.has_video
            && hint.mime_type.as_deref() == Some("audio/webm")
            && hint.codec.as_deref() == Some("opus")
            && hint
                .duration_seconds
                .is_some_and(|duration| duration.is_finite() && duration > 0.)
            && hint.content_length.is_some_and(|length| length > 0)
    })
}

fn is_finite_remote_opus_webm(source: &AudioSource) -> bool {
    has_finite_opus_webm_hint(source)
        && url::Url::parse(&source.input)
            .ok()
            .is_some_and(|url| url.scheme() == "https" && url.host_str().is_some())
}

fn should_use_ffmpeg_for_source(source: &AudioSource) -> bool {
    // Symphonia scans every MP4 atom before starting a seekable stream. For
    // verified finite remote AAC, use the existing incremental FFmpeg path.
    source.input.to_ascii_lowercase().contains(".m3u8")
        || is_finite_remote_aac_mp4(source)
        // The default Symphonia registry has no Opus decoder. Avoid opening a
        // seekable WebM reader only to fail codec creation and reopen via FFmpeg.
        || (!cfg!(feature = "experimental-opus") && is_finite_remote_opus_webm(source))
        || source
            .decoder_hint
            .as_ref()
            .is_some_and(|hint| hint.has_video && hint.mime_type.as_deref() == Some("audio/mp4"))
}

/// All exit paths, including a disconnected PCM queue, must reap the child.
struct FfmpegChildReaper {
    child: Arc<std::sync::Mutex<std::process::Child>>,
    watcher_done: Arc<AtomicBool>,
    stderr: Option<JoinHandle<String>>,
}
impl Drop for FfmpegChildReaper {
    fn drop(&mut self) {
        self.watcher_done.store(true, Ordering::Relaxed);
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(child);
        if let Some(stderr) = self.stderr.take() {
            let _ = stderr.join();
        }
    }
}

fn failed_native_attempt(
    reason: FailedNativeAttemptReason,
    session: &DecoderSession,
    started: Instant,
    reader_setup_ms: Option<u64>,
    native_decoder_setup_ms: Option<u64>,
    reader_bytes_at_start: u64,
) -> FailedNativeAttemptDiagnostics {
    FailedNativeAttemptDiagnostics {
        reason,
        duration_ms: started.elapsed().as_millis() as u64,
        reader_setup_ms,
        native_decoder_setup_ms,
        reader_bytes_read: session
            .stream_stats
            .bytes_read
            .load(Ordering::Relaxed)
            .saturating_sub(reader_bytes_at_start),
        downloaded_range_end: session
            .stream_stats
            .downloaded_range_end
            .load(Ordering::Relaxed),
    }
}

fn prepare_ffmpeg_fallback_diagnostics(
    diagnostics_out: Option<&Arc<std::sync::Mutex<Option<DecoderDiagnostics>>>>,
    source: &AudioSource,
    output_rate: u32,
    output_channels: u16,
    failed_native_attempt: Option<FailedNativeAttemptDiagnostics>,
) {
    let Some(diagnostics_out) = diagnostics_out else {
        return;
    };
    let mut guard = diagnostics_out.lock().unwrap_or_else(|e| e.into_inner());
    let hint = source.decoder_hint.as_ref();
    let duration = hint
        .and_then(|hint| hint.duration_seconds)
        .filter(|value| value.is_finite() && *value > 0.0);
    let diagnostics = guard.get_or_insert_with(|| DecoderDiagnostics {
        startup_reserve: None,
        failed_native_attempt: None,
        decoder_kind: "ffmpeg".into(),
        codec_hint: hint.and_then(|hint| hint.codec.clone()),
        content_type: hint.and_then(|hint| hint.mime_type.clone()),
        input_content_length: hint.and_then(|hint| hint.content_length),
        content_range_total: None,
        bytes_read: 0,
        bytes_received: 0,
        sample_rate: output_rate,
        channels: output_channels,
        total_duration_seconds: duration,
        reader_setup_ms: None,
        decoder_setup_ms: None,
        first_decoded_block_ms: None,
        reader_reused: false,
        decoder_reused: false,
    });
    diagnostics.decoder_kind = "ffmpeg".into();
    diagnostics.failed_native_attempt = failed_native_attempt;
    diagnostics.reader_setup_ms = None;
    diagnostics.decoder_setup_ms = None;
    diagnostics.first_decoded_block_ms = None;
    diagnostics.reader_reused = false;
    diagnostics.decoder_reused = false;
    // Native-reader counters do not describe the FFmpeg subprocess fallback.
    diagnostics.bytes_read = 0;
    diagnostics.bytes_received = 0;
}

fn decode_ffmpeg_with_diagnostics(
    source: AudioSource,
    position: f64,
    end: Option<f64>,
    output_rate: u32,
    output_channels: u16,
    tx: std_mpsc::SyncSender<Vec<f32>>,
    _samples: Arc<AtomicU64>,
    sample_rate_out: Arc<std::sync::atomic::AtomicU32>,
    channels_out: Arc<std::sync::atomic::AtomicU32>,
    duration_out: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    diagnostics_out: Option<Arc<std::sync::Mutex<Option<DecoderDiagnostics>>>>,
    decode_started: Instant,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    // Phase timing starts here; only first PCM keeps the original decode origin.
    let ffmpeg_setup_started = Instant::now();
    let mut command = Command::new(&source.ffmpeg);
    crate::tools::configure_no_console(&mut command);
    command.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-rw_timeout",
        "5000000",
    ]);
    // Offline TCP fixtures must stay on the bound loopback socket even when
    // the host environment supplies an HTTP proxy. Production is unchanged.
    #[cfg(test)]
    if url::Url::parse(&source.input).ok().is_some_and(|url| {
        url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
    }) {
        command.args(["-http_proxy", ""]);
    }
    // A finite AAC transport must not turn a truncated response into clean EOF.
    // FFmpeg otherwise permits some demux/read errors and can exit successfully.
    if is_finite_remote_aac_mp4(&source) {
        command.arg("-xerror");
    }
    if position > 0.0 {
        command.arg("-ss").arg(position.to_string());
    }
    for (name, value) in &source.headers {
        if name.eq_ignore_ascii_case("user-agent") {
            command.arg("-user_agent").arg(value);
        }
    }
    let header_block = source
        .headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("user-agent"))
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    if !header_block.is_empty() {
        command.arg("-headers").arg(header_block);
    }
    command.arg("-i").arg(&source.input);
    if let Some(end) = end {
        let duration = (end - position).max(0.0);
        command.arg("-t").arg(duration.to_string());
    }
    let child = command
        .args(["-vn", "-f", "f32le", "-acodec", "pcm_f32le", "-ar"])
        .arg(output_rate.to_string())
        .arg("-ac")
        .arg(output_channels.to_string())
        .arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start ffmpeg ({:?}): {e}", source.ffmpeg))?;
    let child = Arc::new(std::sync::Mutex::new(child));
    let watcher_done = Arc::new(AtomicBool::new(false));
    let stderr = child
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .stderr
        .take();
    let stderr = stderr.and_then(|mut pipe| {
        thread::Builder::new()
            .name("burrata-decoder-diagnostics".into())
            .spawn(move || {
                let mut tail = VecDeque::with_capacity(8192);
                let mut bytes = [0; 1024];
                while let Ok(count) = pipe.read(&mut bytes) {
                    if count == 0 {
                        break;
                    }
                    for byte in &bytes[..count] {
                        if tail.len() == 8192 {
                            tail.pop_front();
                        }
                        tail.push_back(*byte);
                    }
                }
                crate::playback::safe_failure_text(
                    &String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()),
                    512,
                )
            })
            .ok()
    });
    let mut _reap = FfmpegChildReaper {
        child: child.clone(),
        watcher_done: watcher_done.clone(),
        stderr,
    };
    let mut stdout = child
        .lock()
        .map_err(|_| "ffmpeg process lock was poisoned".to_owned())?
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg did not provide an audio pipe".to_owned())?;
    if let Some(diagnostics_out) = diagnostics_out.as_ref() {
        if let Some(diagnostics) = diagnostics_out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            diagnostics.decoder_setup_ms = Some(ffmpeg_setup_started.elapsed().as_millis() as u64);
        }
    }
    let mut first_block_recorded = false;
    // stdout.read may block while ffmpeg waits on a stalled network response.
    // A small watcher can kill the child immediately when Load/Stop/Seek cancels it.
    let watcher = {
        let child = child.clone();
        let cancelled = cancelled.clone();
        let done = watcher_done.clone();
        thread::Builder::new()
            .name("burrata-ffmpeg-cancel".into())
            .spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if cancelled.load(Ordering::Relaxed) {
                        if let Ok(mut child) = child.lock() {
                            let _ = child.kill();
                        }
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            })
            .ok()
    };
    sample_rate_out.store(output_rate, Ordering::Relaxed);
    channels_out.store(output_channels as u32, Ordering::Relaxed);
    let expected_duration = source
        .decoder_hint
        .as_ref()
        .and_then(|hint| hint.duration_seconds)
        .filter(|value| value.is_finite() && *value > 0.0);
    duration_out.store(
        expected_duration.unwrap_or(f64::NAN).to_bits(),
        Ordering::Relaxed,
    );
    let mut bytes = [0u8; 16 * 1024];
    let mut carry = Vec::<u8>::new();
    let mut block = Vec::with_capacity(4096);
    loop {
        if cancelled.load(Ordering::Relaxed) {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
            break;
        }
        let count = match stdout.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
                watcher_done.store(true, Ordering::Relaxed);
                if let Some(watcher) = watcher {
                    let _ = watcher.join();
                }
                return Err(format!("could not read ffmpeg audio: {error}"));
            }
        };
        carry.extend_from_slice(&bytes[..count]);
        let aligned = carry.len() / 4 * 4;
        for sample_bytes in carry[..aligned].chunks_exact(4) {
            block.push(f32::from_le_bytes(sample_bytes.try_into().unwrap()));
            if block.len() == 4096 {
                if !first_block_recorded {
                    if let Some(diagnostics_out) = diagnostics_out.as_ref() {
                        if let Some(diagnostics) = diagnostics_out
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_mut()
                        {
                            diagnostics.first_decoded_block_ms =
                                Some(decode_started.elapsed().as_millis() as u64);
                        }
                    }
                    first_block_recorded = true;
                }
                if tx.send(std::mem::take(&mut block)).is_err() {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    watcher_done.store(true, Ordering::Relaxed);
                    if let Some(watcher) = watcher {
                        let _ = watcher.join();
                    }
                    return Ok(());
                }
            }
        }
        carry.drain(..aligned);
    }
    if !block.is_empty() {
        if !first_block_recorded {
            if let Some(diagnostics_out) = diagnostics_out.as_ref() {
                if let Some(diagnostics) = diagnostics_out
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                {
                    diagnostics.first_decoded_block_ms =
                        Some(decode_started.elapsed().as_millis() as u64);
                }
            }
        }
        let _ = tx.send(block);
    }
    watcher_done.store(true, Ordering::Relaxed);
    if let Some(watcher) = watcher {
        let _ = watcher.join();
    }
    let status = child
        .lock()
        .map_err(|_| "ffmpeg process lock was poisoned".to_owned())?
        .wait()
        .map_err(|e| e.to_string())?;
    if !cancelled.load(Ordering::Relaxed) && !status.success() {
        let diagnostic = _reap
            .stderr
            .take()
            .and_then(|worker| worker.join().ok())
            .unwrap_or_default();
        return Err(format!("ffmpeg exited with status {status}: {diagnostic}"));
    }
    Ok(())
}

#[allow(unused_mut)] // The optional Opus header probe rewinds this reader.
fn build_native_decoder(
    mut data: BufReader<Box<dyn ReadSeekSend>>,
    byte_len: Option<u64>,
) -> Result<NativeDecoder, String> {
    #[cfg(feature = "experimental-opus")]
    {
        use std::io::Seek;
        let mut prefix = vec![0u8; 64 * 1024];
        let bytes = data
            .read(&mut prefix)
            .map_err(|error| format!("could not inspect audio header: {error}"))?;
        data.seek(std::io::SeekFrom::Start(0))
            .map_err(|error| format!("could not rewind audio source: {error}"))?;
        let prefix = &prefix[..bytes];
        let contains_opus = [b"OpusHead".as_slice(), b"A_OPUS".as_slice()]
            .iter()
            .any(|needle| prefix.windows(needle.len()).any(|window| window == *needle));
        if contains_opus {
            return crate::opus_experiment::OpusSource::open(data, byte_len)
                .map(NativeDecoder::Opus);
        }
    }

    let builder = rodio::Decoder::builder().with_data(data);
    let decoder = match byte_len {
        Some(length) => builder.with_byte_len(length).with_seekable(true).build(),
        None => builder.with_seekable(false).build(),
    }
    .map_err(|error| error.to_string())?;
    Ok(NativeDecoder::Rodio(decoder))
}

trait ReadSeekSend: Read + std::io::Seek + Send + Sync {}
impl<T: Read + std::io::Seek + Send + Sync> ReadSeekSend for T {}

struct CountingReader {
    inner: Box<dyn ReadSeekSend>,
    bytes_read: Arc<AtomicU64>,
}
impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes_read.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}
impl std::io::Seek for CountingReader {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(from)
    }
}

fn content_range_total(value: Option<&str>) -> Option<u64> {
    let (_, range) = value?.split_once(' ')?;
    let (_, total) = range.rsplit_once('/')?;
    total.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;
    use std::net::TcpListener;

    fn accept_loopback(
        listener: &TcpListener,
        timeout: Duration,
    ) -> std::io::Result<std::net::TcpStream> {
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Accepted sockets may inherit O_NONBLOCK on some platforms
                    // (notably macOS). The fixture callers rely on blocking reads
                    // plus explicit read/write timeouts, so normalize the socket.
                    stream.set_nonblocking(false)?;
                    return Ok(stream);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "loopback accept timed out",
                    ));
                }
                Err(error) => return Err(error),
            }
        }
    }

    #[test]
    fn failed_zero_pcm_preload_activates_only_at_selection_without_claiming_first_audio() {
        let healthy = DecoderSession::default();
        let failed = Arc::new(DecoderSession::default());
        failed.storage_faults.record(
            storage_fault::Operation::Write,
            &std::io::Error::from(std::io::ErrorKind::QuotaExceeded),
        );
        let (tx, rx) = std_mpsc::sync_channel(1);
        drop(tx);
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (activation_tx, activation_rx) = std_mpsc::channel();
        let active_generation = Arc::new(AtomicU64::new(1));
        let decode_failed = Arc::new(AtomicBool::new(true));
        let mut queued = QueuedSamples {
            reserve: None,
            reserve_diagnostics: None,
            decoder_session: failed.clone(),
            rx,
            pending: VecDeque::new(),
            channels: 1,
            sample_rate: 48_000,
            samples_played: Arc::new(AtomicU64::new(0)),
            generation: 2,
            active_generation: active_generation.clone(),
            base_position: Arc::new(AtomicU64::new(0f64.to_bits())),
            initial_position: 0.,
            started: false,
            activated: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            tap_writer: crate::pcm_tap::PcmTap::new().writer(2, 48_000, 1),
            events,
            duration_bits: Arc::new(AtomicU64::new(f64::NAN.to_bits())),
            decoder_diagnostics: Arc::new(std::sync::Mutex::new(None)),
            source_rate: Arc::new(std::sync::atomic::AtomicU32::new(48_000)),
            source_channels: Arc::new(std::sync::atomic::AtomicU32::new(1)),
            decode_failed: decode_failed.clone(),
            source: AudioSource {
                input: "fixture".into(),
                ffmpeg: "unused".into(),
                headers: vec![],
                decoder_hint: None,
            },
            end: None,
            activation_tx,
            buffering: Arc::new(AtomicBool::new(false)),
            exhausted: false,
        };
        // The next reader's failure cannot affect the healthy current reader.
        assert!(active_storage_failure_event(1, &healthy, &AtomicBool::new(false)).is_none());
        assert_eq!(active_generation.load(Ordering::Relaxed), 1);
        assert!(activation_rx.try_recv().is_err());
        assert!(received.try_recv().is_err());
        // The mixer reaches this occurrence: activate ownership despite zero produced PCM.
        assert_eq!(queued.next(), None);
        let activation = activation_rx.try_recv().unwrap();
        assert_eq!(activation.generation, 2);
        assert_eq!(active_generation.load(Ordering::Relaxed), 2);
        assert!(activation.decode_failed.load(Ordering::Relaxed));
        assert!(
            received.try_recv().is_err(),
            "zero PCM must not publish Started"
        );
        assert!(matches!(
            active_storage_failure_event(
                activation.generation,
                &activation.decoder_session,
                &activation.cancel
            ),
            Some(OutputEvent::LocalStorageFailed { generation: 2, .. })
        ));
        assert!(decode_failed.load(Ordering::Relaxed));
        // Re-publishing at the active boundary is independent of the already-set failed flag.
        assert!(active_storage_failure_event(2, &failed, &AtomicBool::new(false)).is_some());
        assert!(active_storage_failure_event(2, &failed, &AtomicBool::new(true)).is_none());
        assert_eq!(queued.next(), None);
        assert!(activation_rx.try_recv().is_err(), "activate only once");
    }

    #[test]
    fn cancelled_storage_fault_is_not_published_and_unrelated_failure_stays_generic() {
        let faults = storage_fault::Faults::default();
        faults.record(
            storage_fault::Operation::Write,
            &std::io::Error::from(std::io::ErrorKind::StorageFull),
        );
        assert!(
            decode_failure_event(1, "ignored".into(), &faults, &AtomicBool::new(true)).is_none()
        );
        assert!(matches!(
            decode_failure_event(2, "ignored".into(), &faults, &AtomicBool::new(false)),
            Some(OutputEvent::LocalStorageFailed { generation: 2, .. })
        ));
        assert!(matches!(
            decode_failure_event(
                3,
                "network".into(),
                &storage_fault::Faults::default(),
                &AtomicBool::new(false)
            ),
            Some(OutputEvent::Failed { generation: 3, .. })
        ));
    }

    // Uses the real downloader and Rodio decoder, without disk filling or an audio device.
    fn run_storage_fault_fixture_child() {
        use std::process::{Child, Command, Stdio};
        type Drain = JoinHandle<(Vec<u8>, usize)>;
        fn drain(mut pipe: impl Read) -> (Vec<u8>, usize) {
            const CAP: usize = 16 * 1024;
            let mut kept = Vec::new();
            let mut total = 0usize;
            let mut bytes = [0u8; 4096];
            loop {
                match pipe.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(count) => {
                        total = total.saturating_add(count);
                        let retain = count.min(CAP.saturating_sub(kept.len()));
                        kept.extend_from_slice(&bytes[..retain]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            (kept, total)
        }
        struct OwnedChild {
            child: Option<Child>,
            stdout: Option<Drain>,
            stderr: Option<Drain>,
        }
        impl OwnedChild {
            fn stop(&mut self) {
                if let Some(child) = self.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.child = None;
            }
            fn collect(&mut self) -> ((Vec<u8>, usize), (Vec<u8>, usize)) {
                let stdout = self
                    .stdout
                    .take()
                    .and_then(|pipe| pipe.join().ok())
                    .unwrap_or_default();
                let stderr = self
                    .stderr
                    .take()
                    .and_then(|pipe| pipe.join().ok())
                    .unwrap_or_default();
                (stdout, stderr)
            }
        }
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                self.stop();
                let _ = self.collect();
            }
        }
        let child = Command::new(std::env::current_exe().expect("test executable path"))
            .args(["--exact", "native_output::tests::stream_download_rodio_retains_local_write_failure_instead_of_clean_eof", "--nocapture", "--test-threads=1"])
            .env("BURRATA_STORAGE_FAULT_FIXTURE_CHILD_V1", "1")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("spawn owned storage fault fixture");
        // Install the owner before spawning drain threads so a drain-spawn panic
        // cannot leave the child running. Every pipe is drained beyond the cap.
        let mut owner = OwnedChild {
            child: Some(child),
            stdout: None,
            stderr: None,
        };
        let stdout = owner.child.as_mut().unwrap().stdout.take().unwrap();
        let stderr = owner.child.as_mut().unwrap().stderr.take().unwrap();
        owner.stdout = Some(thread::spawn(move || drain(stdout)));
        owner.stderr = Some(thread::spawn(move || drain(stderr)));
        let deadline = Instant::now() + Duration::from_secs(25);
        let outcome = loop {
            match owner.child.as_mut().unwrap().try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    thread::park_timeout(Duration::from_millis(10))
                }
                Ok(None) => break Err("owned storage fault fixture exceeded25s"),
                Err(_) => break Err("could not wait for owned storage fault fixture"),
            }
        };
        if outcome.is_ok() {
            // try_wait(Some) has already reaped this exact child; no signal on success.
            owner.child = None;
        } else {
            owner.stop(); // kill+wait before joining pipes on timeout/error.
        }
        let ((stdout, stdout_bytes), (stderr, stderr_bytes)) = owner.collect();
        let output = format!("stdout({stdout_bytes}bytes, retained={}):\n{}\nstderr({stderr_bytes}bytes, retained={}):\n{}", stdout.len(), String::from_utf8_lossy(&stdout), stderr.len(), String::from_utf8_lossy(&stderr));
        match outcome {
            Ok(status) => assert!(
                status.success(),
                "owned storage fault fixture failed: {status}\n{output}"
            ),
            Err(reason) => panic!("{reason}; child killed/reaped\n{output}"),
        }
        // Keep finite child phases visible on --nocapture successful runs too.
        eprintln!("{output}");
    }

    #[test]
    fn stream_download_rodio_retains_local_write_failure_instead_of_clean_eof() {
        if std::env::var("BURRATA_STORAGE_FAULT_FIXTURE_CHILD_V1").as_deref() != Ok("1") {
            run_storage_fault_fixture_child();
            return;
        }
        fn phase(kind: &str, phase: &str) {
            eprintln!("storage_fault_fixture kind={kind} phase={phase}");
        }

        use stream_download::storage::{
            memory::{MemoryStorage, MemoryStorageProvider},
            StorageProvider,
        };
        // StreamDownload waits for the entire requested read range. Model
        // ordinary short streaming reads so decoder read-ahead cannot demand
        // the gated tail before the first sample releases it.
        struct ChunkedReader<R> {
            inner: R,
            enabled: Arc<AtomicBool>,
            position: u64,
        }
        impl<R: Read> Read for ChunkedReader<R> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let length = bytes.len().min(8192);
                assert!(
                    self.enabled.load(Ordering::Acquire) || self.position + length as u64 <= 65536,
                    "decoder requested the gated tail before its first sample"
                );
                let read = self.inner.read(&mut bytes[..length])?;
                self.position += read as u64;
                Ok(read)
            }
        }
        impl<R: std::io::Seek> std::io::Seek for ChunkedReader<R> {
            fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
                let position = self.inner.seek(position)?;
                self.position = position;
                Ok(position)
            }
        }
        struct Inject {
            enabled: Arc<AtomicBool>,
            kind: Option<std::io::ErrorKind>,
        }
        struct Writer {
            inner: MemoryStorage,
            enabled: Arc<AtomicBool>,
            kind: Option<std::io::ErrorKind>,
        }
        impl Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.enabled.load(Ordering::Acquire) {
                    if let Some(kind) = self.kind {
                        #[cfg(unix)]
                        {
                            return Err(std::io::Error::from_raw_os_error(
                                if kind == std::io::ErrorKind::QuotaExceeded {
                                    libc::EDQUOT
                                } else {
                                    libc::ENOSPC
                                },
                            ));
                        }
                        #[cfg(not(unix))]
                        {
                            return Err(std::io::Error::from(kind));
                        }
                    }
                }
                self.inner.write(bytes)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.inner.flush()
            }
        }
        impl std::io::Seek for Writer {
            fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
                self.inner.seek(position)
            }
        }
        impl StorageProvider for Inject {
            type Reader = MemoryStorage;
            type Writer = Writer;
            fn into_reader_writer(
                self,
                length: Option<u64>,
            ) -> std::io::Result<(Self::Reader, Self::Writer)> {
                let (reader, writer) = MemoryStorageProvider.into_reader_writer(length)?;
                Ok((
                    reader,
                    Writer {
                        inner: writer,
                        enabled: self.enabled,
                        kind: self.kind,
                    },
                ))
            }
        }
        for kind in [
            None,
            Some(std::io::ErrorKind::QuotaExceeded),
            Some(std::io::ErrorKind::StorageFull),
        ] {
            let kind_name = match kind {
                None => "normal_eof",
                Some(std::io::ErrorKind::QuotaExceeded) => "quota",
                _ => "no_space",
            };
            phase(kind_name, "start");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let wav = pcm_wav(48_000, 1, 4);
            let length = wav.len();
            let (release, gate) = std_mpsc::channel();
            let server = thread::spawn(move || {
                let mut stream = accept_loopback(&listener, Duration::from_secs(5)).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                stream.write_all(&wav[..65536]).unwrap();
                stream.flush().unwrap();
                gate.recv_timeout(Duration::from_secs(5)).unwrap();
                let _ = stream.write_all(&wav[65536..]);
            });
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let enabled = Arc::new(AtomicBool::new(false));
            let session = DecoderSession::default();
            let faults = session.storage_faults.clone();
            let reader = runtime.block_on(async {
                let client = stream_download::http::reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap();
                let stream = stream_download::http::HttpStream::new(
                    client,
                    format!("http://{address}/audio").parse().unwrap(),
                )
                .await
                .unwrap();
                stream_download::StreamDownload::from_stream(
                    stream,
                    storage_fault::Observed {
                        inner: Inject {
                            enabled: enabled.clone(),
                            kind,
                        },
                        faults: faults.clone(),
                    },
                    stream_download::Settings::default().prefetch_bytes(32768),
                )
                .await
                .unwrap()
            });
            phase(kind_name, "reader_ready");
            // Unknown byte length avoids an eager seek to the yet-unreleased tail.
            let mut decoder = build_native_decoder(
                BufReader::new(Box::new(ChunkedReader {
                    inner: reader,
                    enabled: enabled.clone(),
                    position: 0,
                }) as Box<dyn ReadSeekSend>),
                None,
            )
            .unwrap();
            phase(kind_name, "decoder_ready");
            assert!(decoder.next().is_some());
            phase(kind_name, "first_sample");
            enabled.store(true, Ordering::Release);
            release.send(()).unwrap();
            phase(kind_name, "tail_released");
            let consumed = decoder.by_ref().count();
            eprintln!(
                "storage_fault_fixture kind={kind_name} phase=count_done consumed={consumed}"
            );
            server.join().unwrap();
            phase(kind_name, "server_done");
            match kind {
                None => {
                    assert!(consumed >= 48_000 * 3);
                    assert_eq!(faults.get(), None);
                }
                Some(kind) => {
                    assert!(consumed < 48_000 * 4);
                    let expected = if kind == std::io::ErrorKind::QuotaExceeded {
                        storage_fault::Kind::QuotaExceeded
                    } else {
                        storage_fault::Kind::NoSpace
                    };
                    assert_eq!(
                        faults.get(),
                        Some(storage_fault::Fault {
                            operation: storage_fault::Operation::Write,
                            kind: expected
                        })
                    );
                    // Same session/read cache retains evidence; a new reader/session is independent.
                    assert_eq!(session.storage_faults.get(), faults.get());
                    assert_eq!(DecoderSession::default().storage_faults.get(), None);
                }
            }
            phase(kind_name, "case_done");
        }
        phase("all", "child_done");
    }

    #[test]
    fn buffering_facts_distinguish_blocked_reader_from_no_pcm_without_exposing_source() {
        struct HeldRead {
            entered: std_mpsc::Sender<()>,
            release: std_mpsc::Receiver<()>,
        }
        impl Read for HeldRead {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(2)).unwrap();
                bytes[0] = 0;
                Ok(1)
            }
        }
        let stats = Arc::new(StreamRuntimeStats::default());
        let (entered, entered_rx) = std_mpsc::channel();
        let (release, release_rx) = std_mpsc::channel();
        let reader_stats = stats.clone();
        let worker = thread::spawn(move || {
            let mut reader = TimedStreamReader {
                inner: HeldRead {
                    entered,
                    release: release_rx,
                },
                stats: reader_stats,
            };
            reader.read(&mut [0]).unwrap()
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let blocked = stats.snapshot(12);
        assert!(blocked.reader_observed);
        assert!(blocked.read_in_flight_ms.is_some());
        assert_eq!(blocked.bytes_read, 0);
        assert_eq!(blocked.pcm_consumed_samples, 12);
        assert_eq!(blocked.download_progress_age_ms, None);
        release.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        let completed = stats.snapshot(12);
        assert_eq!(completed.read_in_flight_ms, None);
        assert_eq!(completed.bytes_read, 1);
        let encoded = serde_json::to_value(completed).unwrap();
        assert_eq!(encoded.as_object().unwrap().len(), 12);
        assert!(encoded
            .as_object()
            .unwrap()
            .values()
            .all(|value| value.is_boolean() || value.is_number() || value.is_null()));
    }

    #[test]
    fn buffering_facts_report_real_held_seek_and_clear_after_completion_or_error() {
        struct HeldSeek {
            entered: std_mpsc::Sender<()>,
            release: std_mpsc::Receiver<()>,
            fail: bool,
        }
        impl std::io::Seek for HeldSeek {
            fn seek(&mut self, _: std::io::SeekFrom) -> std::io::Result<u64> {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(2)).unwrap();
                if self.fail {
                    Err(std::io::Error::other("private fixture source"))
                } else {
                    Ok(123)
                }
            }
        }
        for fail in [false, true] {
            let stats = Arc::new(StreamRuntimeStats::default());
            let (entered, entered_rx) = std_mpsc::channel();
            let (release, release_rx) = std_mpsc::channel();
            let worker_stats = stats.clone();
            let worker = thread::spawn(move || {
                let mut reader = TimedStreamReader {
                    inner: HeldSeek {
                        entered,
                        release: release_rx,
                        fail,
                    },
                    stats: worker_stats,
                };
                std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(123))
            });
            entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let blocked = stats.snapshot(0);
            assert!(blocked.seek_in_flight_ms.is_some());
            assert_eq!(
                blocked.read_in_flight_ms, None,
                "seek waiting must not masquerade as read waiting"
            );
            release.send(()).unwrap();
            let result = worker.join().unwrap();
            assert_eq!(result.is_err(), fail);
            if !fail {
                assert_eq!(result.unwrap(), 123);
            }
            let completed = stats.snapshot(0);
            assert_eq!(completed.seek_in_flight_ms, None);
            assert!(completed.max_seek_ms >= blocked.seek_in_flight_ms.unwrap());
            let encoded = serde_json::to_string(&completed).unwrap();
            assert!(!encoded.contains("private"));
            assert_eq!(
                StreamRuntimeStats::default().snapshot(0).seek_in_flight_ms,
                None
            );
        }
    }

    #[test]
    fn buffering_runtime_facts_are_live_per_reader_and_have_no_invented_network_progress() {
        let stats = Arc::new(StreamRuntimeStats::default());
        let mut reader = TimedStreamReader {
            inner: Cursor::new(vec![1, 2, 3]),
            stats: stats.clone(),
        };
        assert_eq!(reader.read(&mut [0; 2]).unwrap(), 2);
        assert_eq!(stats.snapshot(0).bytes_read, 2);
        assert_eq!(reader.read(&mut [0; 2]).unwrap(), 1);
        assert_eq!(stats.snapshot(0).bytes_read, 3);
        assert_eq!(reader.read(&mut [0; 2]).unwrap(), 0);
        assert_eq!(stats.snapshot(0).bytes_read, 3);
        assert!(!stats.snapshot(0).network_reader);
        assert_eq!(stats.snapshot(0).download_progress_age_ms, None);
        assert_eq!(StreamRuntimeStats::default().snapshot(0).bytes_read, 0);
    }

    #[test]
    fn actual_loopback_body_gap_exhausts_production_pcm_while_immediate_same_wav_does_not() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        // Counter/gate driven, not a latency or realtime5-second qualification.
        for delayed in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let wav = pcm_wav(44_100, 2, 6);
            let (prefix_sent, prefix_rx) = std_mpsc::channel();
            let (release, release_rx) = std_mpsc::channel();
            let server = thread::spawn(move || {
                let mut socket = accept_loopback(&listener, Duration::from_secs(3)).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", wav.len()).unwrap();
                if delayed {
                    socket.write_all(&wav[..65536]).unwrap();
                    socket.flush().unwrap();
                    prefix_sent.send(()).unwrap();
                    // Cancellation, not timeout or a network error, ends the held case.
                    release_rx.recv_timeout(Duration::from_secs(8)).unwrap();
                    let mut byte = [0];
                    match socket.read(&mut byte) {
                        Ok(0) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::ConnectionAborted
                            ) => {}
                        _ => panic!("owned request did not close on cancellation"),
                    }
                } else {
                    socket.write_all(&wav).unwrap();
                    socket.flush().unwrap();
                    prefix_sent.send(()).unwrap();
                }
            });
            let (mixer, mut output) = rodio::mixer::mixer(
                NonZeroU16::new(2).unwrap(),
                NonZeroU32::new(44_100).unwrap(),
            );
            let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
            let session = Arc::new(DecoderSession::default());
            session.set_startup_deadline(Some(Instant::now() + Duration::from_secs(5)));
            let playback = start_playback(
                &mixer,
                0.,
                71,
                AudioSource {
                    input: format!("http://{address}/fixture.wav"),
                    ffmpeg: "not-used-by-wav-fixture".into(),
                    headers: vec![],
                    decoder_hint: None,
                },
                0.,
                None,
                0.,
                44_100,
                2,
                events,
                crate::pcm_tap::PcmTap::new(),
                session.clone(),
                true,
            );
            let result = catch_unwind(AssertUnwindSafe(|| {
                prefix_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                let ready_deadline = Instant::now() + Duration::from_secs(3);
                while session
                    .stream_stats
                    .pcm_produced_samples
                    .load(Ordering::Relaxed)
                    < 6 * 4096
                {
                    assert!(
                        Instant::now() < ready_deadline,
                        "initial prefix never decoded"
                    );
                    assert!(!playback.decode_failed.load(Ordering::Relaxed));
                    thread::park_timeout(Duration::from_millis(1));
                }
                // Equal initial decoded reserve prevents startup probing/scheduling from
                // being mistaken for the intentionally held input tail.
                playback.player.play();
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut body_wait_observed = false;
                for _ in 0..32 {
                    let consumed = playback.samples.load(Ordering::Relaxed);
                    let target = consumed + 4096;
                    while session
                        .stream_stats
                        .pcm_produced_samples
                        .load(Ordering::Relaxed)
                        < target
                    {
                        let facts = session.stream_stats.snapshot(consumed);
                        if delayed
                            && facts.read_in_flight_ms.is_some()
                            && facts.downloaded_range_end == 65536
                            && facts.pcm_produced_samples == consumed
                        {
                            body_wait_observed = true;
                            break;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "fixture producer failed to progress"
                        );
                        thread::park_timeout(Duration::from_millis(1));
                    }
                    if body_wait_observed {
                        break;
                    }
                    // Mixer framing can emit startup padding; drive by actual PCM
                    // consumption rather than assuming one iterator call per sample.
                    while playback.samples.load(Ordering::Relaxed) < target {
                        assert!(
                            Instant::now() < deadline,
                            "mixer failed to consume ready PCM"
                        );
                        assert!(output.next().is_some());
                    }
                }
                if delayed {
                    assert!(
                        body_wait_observed,
                        "known prefix should exhaust while body gate is held"
                    );
                    let consumed = playback.samples.load(Ordering::Relaxed);
                    for _ in 0..4096 {
                        assert!(output.next().is_some());
                    }
                    assert_eq!(
                        playback.samples.load(Ordering::Relaxed),
                        consumed,
                        "synthetic zeros cannot count as consumed PCM"
                    );
                    assert!(playback.buffering.load(Ordering::Relaxed));
                    let OutputEvent::Buffering {
                        generation,
                        diagnostics: Some(facts),
                    } = buffering_event(&playback)
                    else {
                        panic!("actual buffering event has no facts")
                    };
                    assert_eq!(generation, 71);
                    assert!(facts.read_in_flight_ms.is_some());
                    assert_eq!(facts.pcm_produced_samples, facts.pcm_consumed_samples);
                    assert!(facts.synthetic_silence_samples >= 4096);
                    assert!(session.storage_faults.get().is_none());
                } else {
                    assert!(!body_wait_observed);
                    assert!(playback.samples.load(Ordering::Relaxed) >= 32 * 4096);
                    assert_eq!(
                        session
                            .stream_stats
                            .synthetic_silence_samples
                            .load(Ordering::Relaxed),
                        0
                    );
                    assert!(!playback.buffering.load(Ordering::Relaxed));
                }
            }));
            // Cleanup also executes on assertion failure: stop producer, drop the queued
            // consumer to unblock send, then prove the detached decode thread returned.
            playback.cancel.store(true, Ordering::Release);
            playback.player.stop();
            drop(output);
            let finished_deadline = Instant::now() + Duration::from_secs(3);
            while !session.decode_thread_finished.load(Ordering::Acquire)
                && Instant::now() < finished_deadline
            {
                thread::park_timeout(Duration::from_millis(1));
            }
            let finished = session.decode_thread_finished.load(Ordering::Acquire);
            if finished {
                session.decoder.lock().unwrap().take();
            }
            let _ = release.send(());
            let server_result = server.join();
            assert!(
                finished,
                "owned decode thread did not settle after cancellation"
            );
            server_result.unwrap();
            if result.is_ok() {
                while let Ok(event) = received.try_recv() {
                    assert!(
                        !matches!(
                            event,
                            OutputEvent::Failed { .. } | OutputEvent::LocalStorageFailed { .. }
                        ),
                        "cancellation must not publish source/storage failure"
                    );
                }
            }
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    }

    #[test]
    #[ignore = "paced software-mixer causal fixture; run alone on an idle host under an outer timeout"]
    fn paced_same_wav_input_gap_preserves_real_consumption_and_exposes_baseline_starvation() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        // Same immutable bytes in both arms; no provider request, physical audio or policy change.
        let wav = Arc::new(pcm_wav(44_100, 2, 8));
        for delayed in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let body = wav.clone();
            let (prefix_sent, prefix_rx) = std_mpsc::channel();
            let (release, release_rx) = std_mpsc::channel::<()>();
            let server = thread::spawn(move || {
                let mut socket = accept_loopback(&listener, Duration::from_secs(4)).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                if delayed {
                    socket.write_all(&body[..65536]).unwrap();
                    socket.flush().unwrap();
                    prefix_sent.send(()).unwrap();
                    // Main test releases this on a monotonic2s consumer timeline, not a server sleep.
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                    // Early cleanup may close the request; cancellation is not a fixture server panic.
                    let _ = socket.write_all(&body[65536..]);
                    let _ = socket.flush();
                } else {
                    socket.write_all(&body).unwrap();
                    socket.flush().unwrap();
                    prefix_sent.send(()).unwrap();
                }
            });
            let (mixer, mut output) = rodio::mixer::mixer(
                NonZeroU16::new(2).unwrap(),
                NonZeroU32::new(44_100).unwrap(),
            );
            let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
            let session = Arc::new(DecoderSession::default());
            session.set_startup_deadline(Some(
                Instant::now() + crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE,
            ));
            let playback = start_playback(
                &mixer,
                0.,
                81,
                AudioSource {
                    input: format!("http://{address}/fixture.wav"),
                    ffmpeg: "unused-for-wav-fixture".into(),
                    headers: vec![],
                    decoder_hint: None,
                },
                0.,
                None,
                0.,
                44_100,
                2,
                events,
                crate::pcm_tap::PcmTap::new(),
                session.clone(),
                false,
            );
            let result = catch_unwind(AssertUnwindSafe(|| {
                prefix_rx.recv_timeout(Duration::from_secs(4)).unwrap();
                let setup_deadline = Instant::now() + Duration::from_secs(4);
                while session
                    .stream_stats
                    .pcm_produced_samples
                    .load(Ordering::Relaxed)
                    < 6 * 4096
                    || (delayed
                        && (session.stream_stats.read_started.load(Ordering::Relaxed) == 0
                            || session
                                .stream_stats
                                .downloaded_range_end
                                .load(Ordering::Relaxed)
                                != 65536))
                {
                    assert!(
                        Instant::now() < setup_deadline,
                        "paced fixture initial prefix never decoded"
                    );
                    assert!(!playback.decode_failed.load(Ordering::Relaxed));
                    thread::park_timeout(Duration::from_millis(1));
                }
                playback.player.play();
                let origin = Instant::now();
                let mut released = !delayed;
                let mut first_starvation = None;
                let mut first_real_ms = None;
                let mut last_tick = origin;
                for tick in 0..1400u32 {
                    let due = origin + Duration::from_millis(u64::from(tick + 1) * 10);
                    while Instant::now() < due {
                        thread::park_timeout(due.saturating_duration_since(Instant::now()));
                    }
                    let now = Instant::now();
                    // Scheduling pauses are a separate fixture failure, never evidence of input causality.
                    assert!(
                        now.duration_since(last_tick) < Duration::from_millis(100),
                        "paced fixture scheduler gap"
                    );
                    last_tick = now;
                    if delayed && !released && now.duration_since(origin) >= Duration::from_secs(2)
                    {
                        release.send(()).unwrap();
                        released = true;
                    }
                    // Pace ITERATOR output, then read actual PCM accounting. Mixer startup
                    // framing/zeros do not count as consumed source samples.
                    for _ in 0..882 {
                        assert!(output.next().is_some());
                    }
                    let consumed = playback.samples.load(Ordering::Relaxed);
                    if consumed > 0 && first_real_ms.is_none() {
                        first_real_ms = Some(origin.elapsed().as_millis());
                    }
                    if first_starvation.is_none()
                        && session
                            .stream_stats
                            .synthetic_silence_samples
                            .load(Ordering::Relaxed)
                            > 0
                    {
                        let OutputEvent::Buffering {
                            generation,
                            diagnostics: Some(facts),
                        } = buffering_event(&playback)
                        else {
                            panic!("paced buffering event missing facts")
                        };
                        first_starvation = Some((origin.elapsed().as_millis(), generation, facts));
                    }
                    assert!(!playback.decode_failed.load(Ordering::Relaxed));
                    if consumed >= 44_100 * 2 * 5 {
                        assert!(
                            origin.elapsed() >= Duration::from_secs(5),
                            "five consumed seconds require five paced wall seconds"
                        );
                        break;
                    }
                }
                let consumed = playback.samples.load(Ordering::Relaxed);
                assert!(
                    consumed >= 44_100 * 2 * 5,
                    "paced fixture did not consume five real PCM seconds"
                );
                assert!(first_real_ms.is_some());
                if delayed {
                    let (at_ms, generation, facts) =
                        first_starvation.expect("held body should starve baseline");
                    assert_eq!(generation, 81);
                    assert!(
                        at_ms < 2000,
                        "body gate must still be held at first starvation"
                    );
                    assert!(facts.read_in_flight_ms.is_some());
                    assert_eq!(facts.downloaded_range_end, 65536);
                    assert_eq!(facts.pcm_produced_samples, facts.pcm_consumed_samples);
                    assert!(facts.synthetic_silence_samples > 0);
                    assert!(released);
                } else {
                    assert!(
                        first_starvation.is_none(),
                        "immediate control unexpectedly starved"
                    );
                    assert_eq!(
                        session
                            .stream_stats
                            .synthetic_silence_samples
                            .load(Ordering::Relaxed),
                        0
                    );
                }
                assert!(session.storage_faults.get().is_none());
                eprintln!("paced_body_gap delayed={delayed} consumed={consumed} elapsedMs={} firstRealMs={}", origin.elapsed().as_millis(), first_real_ms.unwrap());
            }));
            // Same explicit owned cancellation/decoder completion protocol as the fast fixture.
            playback.cancel.store(true, Ordering::Release);
            playback.player.stop();
            drop(output);
            let finished_deadline = Instant::now() + Duration::from_secs(3);
            while !session.decode_thread_finished.load(Ordering::Acquire)
                && Instant::now() < finished_deadline
            {
                thread::park_timeout(Duration::from_millis(1));
            }
            let finished = session.decode_thread_finished.load(Ordering::Acquire);
            if finished {
                session.decoder.lock().unwrap().take();
            }
            let _ = release.send(());
            let server_result = server.join();
            assert!(finished, "paced fixture decode thread did not settle");
            server_result.unwrap();
            if result.is_ok() {
                while let Ok(event) = received.try_recv() {
                    assert!(!matches!(
                        event,
                        OutputEvent::Failed { .. } | OutputEvent::LocalStorageFailed { .. }
                    ));
                }
            }
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    }

    include!("native_output_reserve_tests.rs");
    include!("native_output_seek_range_tests.rs");

    #[test]
    fn content_range_diagnostics_extract_only_numeric_total() {
        assert_eq!(
            content_range_total(Some("bytes 0-32767/11844321")),
            Some(11_844_321)
        );
        assert_eq!(content_range_total(Some("bytes 10-20/*")), None);
        assert_eq!(content_range_total(Some("malformed")), None);
        assert_eq!(content_range_total(None), None);
    }

    #[test]
    fn counting_reader_tracks_actual_decoder_reads_across_seeks() {
        let bytes_read = Arc::new(AtomicU64::new(0));
        let inner: Box<dyn ReadSeekSend> = Box::new(Cursor::new(vec![1, 2, 3, 4]));
        let mut reader = CountingReader {
            inner,
            bytes_read: bytes_read.clone(),
        };
        let mut buffer = [0; 2];
        assert_eq!(reader.read(&mut buffer).unwrap(), 2);
        std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(0)).unwrap();
        assert_eq!(reader.read(&mut buffer).unwrap(), 2);
        assert_eq!(bytes_read.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn reconnect_position_keeps_the_sample_accurate_playhead() {
        let rendered = 44_100u64 * 2 * 3 / 2;
        assert_eq!(
            position_from_output_samples(85.0, rendered, 44_100, 2),
            86.5
        );
        assert_eq!(reconnect_delay(0), Duration::from_millis(250));
        assert_eq!(
            reconnect_delay(MAX_RECONNECT_ATTEMPTS),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn underruns_do_not_reopen_output_but_backend_errors_do() {
        use rodio::cpal::{BackendSpecificError, StreamError};

        assert!(!should_reconnect_for_stream_error(
            &StreamError::BufferUnderrun
        ));
        assert!(should_reconnect_for_stream_error(
            &StreamError::BackendSpecific {
                err: BackendSpecificError {
                    description: "device stream failed".into(),
                },
            }
        ));
        assert!(should_reconnect_for_stream_error(
            &StreamError::DeviceNotAvailable
        ));
        assert!(should_reconnect_for_stream_error(
            &StreamError::StreamInvalidated
        ));
    }

    fn pcm_wav(rate: u32, channels: u16, seconds: u32) -> Vec<u8> {
        let data_len = rate * u32::from(channels) * 2 * seconds;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        wav.extend_from_slice(&(channels * 2).to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 0);
        wav
    }

    #[test]
    fn generated_wav_reports_decoder_phases_and_preserves_metadata_on_reuse() {
        let path = std::env::temp_dir().join(format!(
            "burrata-native-timing-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, pcm_wav(48_000, 1, 1)).expect("write decoder timing fixture");
        let session = Arc::new(DecoderSession::default());
        let diagnostics = Arc::new(std::sync::Mutex::new(None));
        let run_decode = |diagnostics: Arc<std::sync::Mutex<Option<DecoderDiagnostics>>>| {
            let (tx, rx) = std_mpsc::sync_channel(64);
            decode_with_session_and_diagnostics(
                AudioSource {
                    input: path.to_string_lossy().into_owned(),
                    ffmpeg: "ffmpeg".into(),
                    headers: Vec::new(),
                    decoder_hint: None,
                },
                0.0,
                None,
                44_100,
                2,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                Some(diagnostics),
                Arc::new(AtomicBool::new(false)),
                session.clone(),
            )
            .expect("decode generated fixture");
            let samples = rx.into_iter().map(|block| block.len()).sum::<usize>();
            assert!(samples > 0, "fixture should yield decoded PCM");
        };

        run_decode(diagnostics.clone());
        let initial = diagnostics
            .lock()
            .unwrap()
            .clone()
            .expect("first diagnostics");
        assert_eq!(initial.decoder_kind, "rodio");
        assert!(initial.reader_setup_ms.is_some());
        assert!(initial.decoder_setup_ms.is_some());
        assert!(initial.first_decoded_block_ms.is_some());
        assert!(!initial.reader_reused);
        assert!(!initial.decoder_reused);
        let serialized = serde_json::to_value(&initial).unwrap();
        assert!(serialized.get("readerSetupMs").is_some());
        assert!(serialized.get("decoderSetupMs").is_some());
        assert!(serialized.get("firstDecodedBlockMs").is_some());
        assert!(serialized.get("readerReused").is_some());
        assert!(serialized.get("decoderReused").is_some());

        run_decode(diagnostics.clone());
        let reused = diagnostics
            .lock()
            .unwrap()
            .clone()
            .expect("reused diagnostics");
        assert!(reused.reader_reused);
        assert!(reused.decoder_reused);
        assert_eq!(reused.reader_setup_ms, None);
        assert_eq!(reused.decoder_setup_ms, None);
        assert_eq!(reused.bytes_read, initial.bytes_read);
        assert_eq!(reused.bytes_received, initial.bytes_received);
        assert_eq!(reused.content_type, initial.content_type);
        assert!(reused.first_decoded_block_ms.is_some());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decodes_generated_wav_without_audio_device() {
        // One second of 8 kHz, mono, signed 16-bit PCM silence.
        let data_len = 8_000u32 * 2;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 0);
        let bytes = std::env::var_os("BURRATA_NATIVE_PLAYBACK_FIXTURE")
            .map(std::fs::read)
            .transpose()
            .expect("playback fixture should be readable")
            .unwrap_or(wav);
        let decoder = rodio::Decoder::try_from(Cursor::new(bytes)).expect("WAV should decode");
        assert!(decoder.sample_rate().get() > 0);
        assert!(decoder.channels().get() > 0);
        assert_eq!(decoder.total_duration(), Some(Duration::from_secs(1)));
        assert!(decoder.count() > 0);
    }

    #[test]
    #[ignore = "set BURRATA_NATIVE_AUDIO_FIXTURE to a local audio file for decoder duration/late-seek qualification"]
    fn external_audio_fixture_reports_duration_and_decodes_after_late_seek() {
        let path = std::env::var_os("BURRATA_NATIVE_AUDIO_FIXTURE")
            .expect("qualification fixture path is required");
        let file = File::open(path).expect("open local fixture");
        let byte_len = file.metadata().expect("fixture metadata").len();
        let input: Box<dyn ReadSeekSend> = Box::new(file);
        let mut decoder = build_native_decoder(BufReader::new(input), Some(byte_len))
            .expect("audio fixture should decode");
        let duration = decoder
            .total_duration()
            .expect("decoder should report a known duration")
            .as_secs_f64();
        let expected = std::env::var("BURRATA_NATIVE_AUDIO_EXPECTED_SECONDS")
            .expect("independently probed duration is required")
            .parse::<f64>()
            .expect("expected duration must be numeric");
        assert!(
            (duration - expected).abs() <= 1.0,
            "decoder duration {duration} differs from independent duration {expected}"
        );

        let seek = expected * 0.95;
        decoder
            .try_seek(Duration::from_secs_f64(seek))
            .expect("95% seek should be supported");
        let sample_rate = decoder.sample_rate().get() as f64;
        let channels = decoder.channels().get() as f64;
        let decoded_samples = decoder.count() as u64;
        let decoded_seconds = decoded_samples as f64 / sample_rate / channels;
        let expected_remaining = expected - seek;
        assert!(decoded_seconds > expected_remaining - 1.0);
        assert!(decoded_seconds < expected_remaining + 1.0);
    }

    #[test]
    fn only_verified_muxed_or_finite_remote_aac_mp4_sources_force_ffmpeg() {
        let source = AudioSource {
            input: "https://cdn.example/stream".into(),
            ffmpeg: "ffmpeg".into(),
            headers: vec![],
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some("audio/mp4".into()),
                codec: Some("aac".into()),
                duration_seconds: Some(250.0),
                content_length: Some(10_000_000),
                has_video: true,
            }),
        };
        assert!(should_use_ffmpeg_for_source(&source));

        let mut audio_only = source.clone();
        audio_only.decoder_hint.as_mut().unwrap().has_video = false;
        assert!(should_use_ffmpeg_for_source(&audio_only));
        audio_only.decoder_hint.as_mut().unwrap().codec = Some("mp4a.40.2".into());
        assert!(should_use_ffmpeg_for_source(&audio_only));
        for change in 0..6 {
            let mut unsupported = audio_only.clone();
            match change {
                0 => unsupported.input = "/local/file.m4a".into(),
                1 => unsupported.decoder_hint.as_mut().unwrap().codec = None,
                2 => unsupported.decoder_hint.as_mut().unwrap().codec = Some("alac".into()),
                3 => unsupported.decoder_hint.as_mut().unwrap().duration_seconds = None,
                4 => unsupported.decoder_hint.as_mut().unwrap().duration_seconds = Some(f64::NAN),
                _ => unsupported.decoder_hint.as_mut().unwrap().content_length = None,
            }
            assert!(!should_use_ffmpeg_for_source(&unsupported));
        }

        let mut non_mp4_video = source;
        non_mp4_video.decoder_hint.as_mut().unwrap().mime_type = Some("audio/webm".into());
        assert!(!should_use_ffmpeg_for_source(&non_mp4_video));
    }

    #[test]
    fn finite_https_opus_webm_routes_only_when_native_opus_is_disabled() {
        let source = AudioSource {
            input: "https://cdn.example/stream".into(),
            ffmpeg: "ffmpeg".into(),
            headers: vec![],
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some("audio/webm".into()),
                codec: Some("opus".into()),
                duration_seconds: Some(250.0),
                content_length: Some(4_000_000),
                has_video: false,
            }),
        };
        assert!(is_finite_remote_opus_webm(&source));
        assert_eq!(
            should_use_ffmpeg_for_source(&source),
            !cfg!(feature = "experimental-opus")
        );
        for change in 0..13 {
            let mut unsupported = source.clone();
            match change {
                0 => unsupported.input = "http://cdn.example/stream".into(),
                1 => unsupported.input = "/local/file.webm".into(),
                2 => unsupported.decoder_hint = None,
                3 => unsupported.decoder_hint.as_mut().unwrap().codec = None,
                4 => unsupported.decoder_hint.as_mut().unwrap().codec = Some("vorbis".into()),
                5 => {
                    unsupported.decoder_hint.as_mut().unwrap().mime_type = Some("video/webm".into())
                }
                6 => unsupported.decoder_hint.as_mut().unwrap().duration_seconds = None,
                7 => unsupported.decoder_hint.as_mut().unwrap().duration_seconds = Some(f64::NAN),
                8 => unsupported.decoder_hint.as_mut().unwrap().duration_seconds = Some(0.),
                9 => unsupported.decoder_hint.as_mut().unwrap().content_length = None,
                10 => unsupported.decoder_hint.as_mut().unwrap().content_length = Some(0),
                11 => unsupported.decoder_hint.as_mut().unwrap().has_video = true,
                _ => {
                    unsupported.decoder_hint.as_mut().unwrap().duration_seconds =
                        Some(f64::INFINITY)
                }
            }
            assert!(!is_finite_remote_opus_webm(&unsupported), "case {change}");
            assert!(!should_use_ffmpeg_for_source(&unsupported), "case {change}");
        }
    }

    struct AacHttpFixture {
        source: AudioSource,
        requests: Arc<AtomicU64>,
        stalled_prefix_written: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Drop for AacHttpFixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.worker.take().unwrap().join().unwrap();
        }
    }
    fn aac_http_fixture(mode: u8) -> AacHttpFixture {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let bytes = include_bytes!("../tests/fixtures/aac-startup-fixture.m4a");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let requests = Arc::new(AtomicU64::new(0));
        let worker_requests = requests.clone();
        let stalled_prefix_written = Arc::new(AtomicBool::new(false));
        let worker_stalled_prefix_written = stalled_prefix_written.clone();
        let worker = thread::spawn(move || {
            let mut clients = Vec::new();
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let stop = worker_stop.clone();
                        let requests = worker_requests.clone();
                        let stalled_prefix_written = worker_stalled_prefix_written.clone();
                        clients.push(thread::spawn(move || {
                            stream.set_nonblocking(false).unwrap();
                            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                            let mut request=Vec::new(); let mut byte=[0u8;1];
                            while !request.ends_with(b"\r\n\r\n") { if stream.read_exact(&mut byte).is_err() { return; } request.push(byte[0]); }
                            requests.fetch_add(1,Ordering::Relaxed);
                            let text=String::from_utf8_lossy(&request).to_ascii_lowercase();
                            if mode==1 || !text.contains("x-fixture: exact-token") {
                                let _=stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"); return;
                            }
                            let offset=text.lines().find_map(|line|line.strip_prefix("range: bytes="))
                                .and_then(|range|range.split('-').next()).and_then(|value|value.parse::<usize>().ok()).unwrap_or(0);
                            if offset>=bytes.len(){return;}
                            if write!(stream,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nContent-Type: audio/mp4\r\nConnection: close\r\n\r\n",bytes.len()-offset,offset,bytes.len()-1,bytes.len()).is_err(){return;}
                            let end=if mode==2 { bytes.len()/2 } else if mode==3 { 1024 } else { bytes.len() };
                            if offset<end && stream.write_all(&bytes[offset..end]).is_err() { return; }
                            if mode==3 { stalled_prefix_written.store(true, Ordering::Release); }
                            if mode==3 { while !stop.load(Ordering::Relaxed){thread::sleep(Duration::from_millis(5));} }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("AAC fixture accept: {error}"),
                }
            }
            for client in clients {
                client.join().unwrap();
            }
        });
        AacHttpFixture {
            source: AudioSource {
                input: format!("http://{address}/audio.m4a"),
                ffmpeg: "ffmpeg".into(),
                headers: vec![("X-Fixture".into(), "exact-token".into())],
                decoder_hint: Some(AudioSourceDecoderHint {
                    mime_type: Some("audio/mp4".into()),
                    codec: Some("mp4a.40.2".into()),
                    duration_seconds: Some(4.),
                    content_length: Some(bytes.len() as u64),
                    has_video: false,
                }),
            },
            requests,
            stalled_prefix_written,
            stop,
            worker: Some(worker),
        }
    }

    #[test]
    fn finite_remote_aac_preserves_tones_repeated_seeks_and_chapter_end() {
        let fixture = aac_http_fixture(0);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reference.m4a");
        std::fs::write(
            &path,
            include_bytes!("../tests/fixtures/aac-startup-fixture.m4a"),
        )
        .unwrap();
        let reference = AudioSource {
            input: path.to_string_lossy().into(),
            ffmpeg: "unused".into(),
            headers: vec![],
            decoder_hint: None,
        };
        assert!(!should_use_ffmpeg_for_source(&reference));
        assert!(should_use_ffmpeg_for_source(&fixture.source));
        for (position, expected_hz) in [
            (0.2, 440.),
            (2.2, 1320.),
            (1.2, 880.),
            (0.2, 440.),
            (3.7, 1320.),
        ] {
            let actual = decode_window(
                fixture.source.clone(),
                Arc::new(DecoderSession::default()),
                position,
                position + 0.2,
                Arc::new(AtomicBool::new(false)),
            );
            let baseline = decode_window(
                reference.clone(),
                Arc::new(DecoderSession::default()),
                position,
                position + 0.2,
                Arc::new(AtomicBool::new(false)),
            );
            assert!(
                (actual.len() as i64 - baseline.len() as i64).abs() <= 512,
                "AAC sample-count parity"
            );
            assert!(
                (actual.len() as i64 - 9600).abs() <= 512,
                "chapter endpoint bounds"
            );
            let hz = actual
                .windows(2)
                .filter(|pair| pair[0] <= 0. && pair[1] > 0.)
                .count() as f64
                / (actual.len() as f64 / 48000.);
            assert!(
                (hz - expected_hz).abs() < 30.,
                "seek returned wrong tone: {hz}"
            );
        }
        assert!(fixture.requests.load(Ordering::Relaxed) >= 5);
    }

    #[test]
    fn finite_remote_aac_stalled_input_cancels_and_reaps_without_pcm() {
        let fixture = aac_http_fixture(3);
        let source = fixture.source.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (tx, _rx) = std_mpsc::sync_channel(16);
        let (done, finished) = std_mpsc::channel();
        let worker = thread::spawn(move || {
            let result = decode_to_blocks(
                source,
                0.,
                Some(1.),
                48000,
                1,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                worker_cancelled,
            );
            let _ = done.send(result);
        });
        let test_started = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.stalled_prefix_written.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let prefix_ready = fixture.stalled_prefix_written.load(Ordering::Acquire);
        let prefix_ready_ms = test_started.elapsed().as_millis();
        if !prefix_ready {
            cancelled.store(true, Ordering::Relaxed);
            fixture.stop.store(true, Ordering::Release);
            let cleanup = finished.recv_timeout(Duration::from_secs(5));
            if cleanup.is_ok() {
                worker.join().unwrap();
            }
            eprintln!(
                "{}",
                serde_json::json!({"test":"finite_remote_aac_stalled_input_cancels_and_reaps_without_pcm",
                    "prefixReadyMs":prefix_ready_ms,
                    "cleanupCompleted":cleanup.is_ok(),
                    "failure":"prefix_not_ready"})
            );
            panic!("stalled FFmpeg fixture did not write its prefix in time ({prefix_ready_ms}ms; cleanupCompleted={})", cleanup.is_ok());
        }
        cancelled.store(true, Ordering::Relaxed);
        let cancel_started = Instant::now();
        let worker_result = match finished.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => {
                let cancel_to_completion_ms = cancel_started.elapsed().as_millis();
                eprintln!(
                    "{}",
                    serde_json::json!({"test":"finite_remote_aac_stalled_input_cancels_and_reaps_without_pcm",
                        "prefixReadyMs":prefix_ready_ms,
                        "cancelToCompletionMs":cancel_to_completion_ms,
                        "completedWithinBound":true,
                        "workerOk":result.is_ok()})
                );
                Some(result)
            }
            Err(error) => {
                let cancel_to_timeout_ms = cancel_started.elapsed().as_millis();
                // Release the stalled peer so FFmpeg can unwind even if process
                // termination is delayed on a loaded CI runner. Keep the original
                // two-second cancellation bound as the assertion under test.
                fixture.stop.store(true, Ordering::Release);
                let cleanup = finished.recv_timeout(Duration::from_secs(5));
                let cleanup_ms = cancel_started.elapsed().as_millis();
                if cleanup.is_ok() {
                    worker.join().unwrap();
                }
                eprintln!(
                    "{}",
                    serde_json::json!({"test":"finite_remote_aac_stalled_input_cancels_and_reaps_without_pcm",
                        "prefixReadyMs":prefix_ready_ms,
                        "cancelToTimeoutMs":cancel_to_timeout_ms,
                        "cleanupMs":cleanup_ms.saturating_sub(cancel_to_timeout_ms),
                        "cleanupCompleted":cleanup.is_ok(),
                        "failure":"cancel_timeout"})
                );
                panic!(
                    "cancelled FFmpeg did not stop within 2s ({error:?}; cleanupCompleted={})",
                    cleanup.is_ok()
                );
            }
        };
        worker.join().unwrap();
        worker_result
            .expect("successful completion branch retains worker result")
            .unwrap();
    }

    #[test]
    fn finite_remote_aac_rejects_inaccessible_and_truncated_input() {
        for mode in [1, 2] {
            let fixture = aac_http_fixture(mode);
            let source = fixture.source.clone();
            let (tx, rx) = std_mpsc::sync_channel(128);
            let result = decode_to_blocks(
                source,
                0.,
                None,
                48000,
                1,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                Arc::new(AtomicBool::new(false)),
            );
            let _samples = rx.into_iter().flatten().count();
            assert!(
                result.is_err(),
                "inaccessible/truncated AAC must not be treated as clean EOF (mode {mode})"
            );
        }
    }

    #[test]
    #[ignore = "set BURRATA_MUXED_MP4_FIXTURE and BURRATA_MUXED_MP4_EXPECTED_SECONDS for late-seek qualification"]
    fn external_muxed_mp4_uses_ffmpeg_and_preserves_duration_after_late_seek() {
        let path = std::env::var_os("BURRATA_MUXED_MP4_FIXTURE")
            .expect("exact muxed stream fixture path is required");
        let expected = std::env::var("BURRATA_MUXED_MP4_EXPECTED_SECONDS")
            .expect("independently probed stream duration is required")
            .parse::<f64>()
            .expect("expected duration must be numeric");
        let path = PathBuf::from(path);
        let byte_len = std::fs::metadata(&path).expect("fixture metadata").len();
        let source = AudioSource {
            input: path.to_string_lossy().into_owned(),
            ffmpeg: "ffmpeg".into(),
            headers: vec![],
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some("audio/mp4".into()),
                codec: Some("aac".into()),
                duration_seconds: Some(expected),
                content_length: Some(byte_len),
                has_video: true,
            }),
        };
        assert!(should_use_ffmpeg_for_source(&source));

        let seek = expected * 0.95;
        let (tx, rx) = std_mpsc::sync_channel(64);
        let duration = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        let decoded_duration = duration.clone();
        // Drain while decoding: the bounded PCM queue otherwise fills before
        // this synchronous helper returns for a long fixture.
        let decoder = std::thread::spawn(move || {
            decode_to_blocks(
                source,
                seek,
                None,
                44_100,
                2,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                duration,
                Arc::new(AtomicBool::new(false)),
            )
        });
        let samples = rx.into_iter().map(|block| block.len() as u64).sum::<u64>();
        decoder
            .join()
            .expect("decoder thread should finish")
            .expect("FFmpeg should decode from the late seek through actual EOF");
        let reported_duration = f64::from_bits(decoded_duration.load(Ordering::Relaxed));
        assert!((reported_duration - expected).abs() <= 0.01);
        let decoded_seconds = samples as f64 / 44_100.0 / 2.0;
        let expected_remaining = expected - seek;
        assert!(
            (decoded_seconds - expected_remaining).abs() <= 1.0,
            "decoded {decoded_seconds:.3}s from a {expected_remaining:.3}s tail"
        );
    }

    #[test]
    fn decoder_resamples_to_fixed_output_format_and_keeps_source_metadata() {
        let path = std::env::temp_dir().join(format!(
            "burrata-native-format-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, pcm_wav(48_000, 1, 1)).expect("write fixture");
        let (tx, rx) = std_mpsc::sync_channel(64);
        let source_rate = Arc::new(std::sync::atomic::AtomicU32::new(44_100));
        let source_channels = Arc::new(std::sync::atomic::AtomicU32::new(2));
        let duration = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        decode_to_blocks(
            AudioSource {
                input: path.to_string_lossy().into_owned(),
                ffmpeg: "ffmpeg".into(),
                headers: Vec::new(),
                decoder_hint: None,
            },
            0.0,
            None,
            44_100,
            2,
            tx,
            Arc::new(AtomicU64::new(0)),
            source_rate.clone(),
            source_channels.clone(),
            duration,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("decode and resample");
        let output_samples: usize = rx.into_iter().map(|block| block.len()).sum();
        let _ = std::fs::remove_file(path);
        assert_eq!(source_rate.load(Ordering::Relaxed), 48_000);
        assert_eq!(source_channels.load(Ordering::Relaxed), 1);
        // Rodio's resampler rounds its conversion ratio; allow less than 2ms.
        assert!((output_samples as i64 - 44_100 * 2).abs() < 177);
    }

    #[test]
    fn known_length_file_seek_at_sixty_percent_is_fast_and_has_duration() {
        let path = std::env::temp_dir().join(format!(
            "burrata-native-seek-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut wav = pcm_wav(48_000, 1, 15);
        for frame in 0..48_000usize * 15 {
            let level = ((frame / 48_000 + 1) as i16) * 1_000;
            let offset = 44 + frame * 2;
            wav[offset..offset + 2].copy_from_slice(&level.to_le_bytes());
        }
        std::fs::write(&path, wav).expect("write seek fixture");
        let (tx, rx) = std_mpsc::sync_channel(2);
        let duration_bits = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        let decoded_duration = duration_bits.clone();
        let decode_path = path.clone();
        let decode = thread::spawn(move || {
            decode_to_blocks(
                AudioSource {
                    input: decode_path.to_string_lossy().into_owned(),
                    ffmpeg: "ffmpeg-unavailable".into(),
                    headers: Vec::new(),
                    decoder_hint: None,
                },
                9.0,
                None,
                44_100,
                2,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                duration_bits,
                Arc::new(AtomicBool::new(false)),
            )
        });
        let mut output_samples = 0usize;
        let mut first_sample = None;
        for block in rx {
            output_samples += block.len();
            first_sample.get_or_insert(block[0]);
        }
        decode
            .join()
            .unwrap()
            .expect("seek and decode from 60 percent");
        let _ = std::fs::remove_file(path);
        assert_eq!(
            f64::from_bits(decoded_duration.load(Ordering::Relaxed)),
            15.0
        );
        assert!((output_samples as i64 - 44_100 * 2 * 6).abs() < 500);
        assert!(first_sample.unwrap() > 0.25, "seek should start near t=9s");
    }

    fn decode_excerpt(
        source: AudioSource,
        session: Arc<DecoderSession>,
        position: f64,
        cancel: Arc<AtomicBool>,
    ) -> Vec<f32> {
        decode_window(source, session, position, position + 0.05, cancel)
    }

    fn decode_window(
        source: AudioSource,
        session: Arc<DecoderSession>,
        position: f64,
        end: f64,
        cancel: Arc<AtomicBool>,
    ) -> Vec<f32> {
        session.bind(&cancel);
        let (tx, rx) = std_mpsc::sync_channel(128);
        decode_with_session(
            source,
            position,
            Some(end),
            48_000,
            1,
            tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(std::sync::atomic::AtomicU32::new(0)),
            Arc::new(std::sync::atomic::AtomicU32::new(0)),
            Arc::new(AtomicU64::new(f64::NAN.to_bits())),
            cancel,
            session,
        )
        .expect("decode excerpt");
        rx.into_iter().flatten().collect()
    }

    #[cfg(all(feature = "experimental-opus", target_os = "linux"))]
    fn opus_fixture_source(directory: &tempfile::TempDir) -> AudioSource {
        let path = directory.path().join("opus-fixture.ogg");
        std::fs::write(&path, include_bytes!("../tests/fixtures/opus-fixture.ogg")).unwrap();
        AudioSource {
            input: path.to_string_lossy().into(),
            ffmpeg: "ffmpeg-unavailable".into(),
            headers: vec![],
            decoder_hint: None,
        }
    }

    #[cfg(all(feature = "experimental-opus", target_os = "linux"))]
    #[test]
    fn experimental_opus_seek_discards_preroll_and_respects_chapter_end() {
        let directory = tempfile::tempdir().unwrap();
        let source = opus_fixture_source(&directory);
        let session = Arc::new(DecoderSession::default());
        let cancel = Arc::new(AtomicBool::new(false));

        let first_segment =
            decode_window(source.clone(), session.clone(), 0.2, 0.4, cancel.clone());
        let final_segment = decode_window(
            source.clone(),
            session.clone(),
            2.2,
            2.4,
            Arc::new(AtomicBool::new(false)),
        );
        let returned_segment =
            decode_window(source, session, 0.2, 0.4, Arc::new(AtomicBool::new(false)));

        assert_eq!(first_segment.len(), 9_600);
        assert_eq!(final_segment.len(), 9_600);
        assert_eq!(returned_segment.len(), 9_600);
        let estimate_hz = |samples: &[f32]| {
            let crossings = samples
                .windows(2)
                .filter(|pair| pair[0] <= 0. && pair[1] > 0.)
                .count();
            crossings as f64 / (samples.len() as f64 / 48_000.)
        };
        assert!((estimate_hz(&first_segment) - 440.).abs() < 30.);
        assert!((estimate_hz(&final_segment) - 1320.).abs() < 60.);
        assert!((estimate_hz(&returned_segment) - 440.).abs() < 30.);
    }

    #[cfg(all(feature = "experimental-opus", target_os = "linux"))]
    #[test]
    fn experimental_opus_decode_stops_after_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let source = opus_fixture_source(&directory);
        let cancel = Arc::new(AtomicBool::new(false));
        let session = Arc::new(DecoderSession::default());
        session.bind(&cancel);
        let worker_cancel = cancel.clone();
        let (tx, rx) = std_mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            decode_with_session(
                source,
                0.,
                None,
                48_000,
                1,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                worker_cancel.clone(),
                session,
            )
        });
        let mut decoded = rx.recv_timeout(Duration::from_secs(2)).unwrap().len();
        cancel.store(true, Ordering::Relaxed);
        decoded += rx.into_iter().flatten().count();
        worker.join().unwrap().unwrap();
        assert!(decoded < 4 * 48_000, "cancelled Opus decode ran to EOF");
    }

    #[cfg(all(feature = "experimental-opus", target_os = "linux"))]
    #[test]
    fn experimental_opus_paused_chapter_seek_remains_silent() {
        let directory = tempfile::tempdir().unwrap();
        let source = opus_fixture_source(&directory);
        let (mixer, mut rendered) = rodio::mixer::mixer(
            NonZeroU16::new(2).unwrap(),
            NonZeroU32::new(48_000).unwrap(),
        );
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        let playback = start_playback(
            &mixer,
            0.8,
            7,
            source,
            2.2,
            Some(2.4),
            2.2,
            48_000,
            2,
            events,
            crate::pcm_tap::PcmTap::new(),
            Arc::new(DecoderSession::default()),
            true,
        );
        assert!(playback.player.is_paused());
        for _ in 0..1_000 {
            assert_eq!(rendered.next(), Some(0.));
        }
        assert_eq!(playback.samples.load(Ordering::Relaxed), 0);
        playback.cancel.store(true, Ordering::Relaxed);
        playback.player.stop();
    }

    #[test]
    fn local_seeks_reuse_open_decoder_and_preserve_excerpt_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("seek.wav");
        let mut wav = pcm_wav(48_000, 1, 15);
        for frame in 0..48_000usize * 15 {
            let level = ((frame / 48_000 + 1) as i16) * 1000;
            wav[44 + frame * 2..46 + frame * 2].copy_from_slice(&level.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let source = AudioSource {
            input: path.to_string_lossy().into(),
            ffmpeg: "ffmpeg-unavailable".into(),
            headers: vec![],
            decoder_hint: None,
        };
        let session = Arc::new(DecoderSession::default());
        let cancel = Arc::new(AtomicBool::new(false));
        for position in [0., 9., 1., 14., 0.] {
            let output = decode_excerpt(source.clone(), session.clone(), position, cancel.clone());
            assert_eq!(output.len(), 2400, "chapter end must remain bounded");
            let expected = (position as i16 + 1) as f32 * 1000. / 32768.;
            assert!(
                (output[0] - expected).abs() < 0.001,
                "wrong audio after seek to {position}"
            );
            // A reopen would fail. Changing the path does not affect the open decoder.
            if position == 0. && path.exists() {
                std::fs::rename(&path, directory.path().join("open.wav")).unwrap();
            }
        }
    }

    #[test]
    fn retired_blocked_reader_is_cancelled_before_waiting_for_network_timeout() {
        let session = Arc::new(DecoderSession::default());
        let aborted = Arc::new(AtomicBool::new(false));
        let flag = aborted.clone();
        *session.cancel_download.lock().unwrap() =
            Some(Box::new(move || flag.store(true, Ordering::Relaxed)));
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let old = session.clone();
        let stopped = aborted.clone();
        let worker = thread::spawn(move || {
            let _guard = old.decoder.lock().unwrap();
            ready_tx.send(()).unwrap();
            let started = Instant::now();
            while !stopped.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(2) {
                thread::sleep(Duration::from_millis(1));
            }
        });
        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let cancel = AtomicBool::new(false);
        let guard = session.lock_decoder(&cancel).unwrap();
        assert!(aborted.load(Ordering::Relaxed));
        drop(guard);
        worker.join().unwrap();
    }

    #[test]
    fn slow_reader_setup_future_is_dropped_at_its_absolute_deadline() {
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let future_dropped = dropped.clone();
        let deadline = Instant::now() + Duration::from_millis(30);
        let result = await_remote_reader_setup(&runtime, cancelled, Some(deadline), async move {
            let _drop_flag = DropFlag(future_dropped);
            // Models the pending HttpStream header future without racing client startup.
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok::<(), String>(())
        });
        assert!(result.unwrap_err().contains("startup deadline"));
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[test]
    fn cancellation_before_response_headers_drops_the_pending_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = std_mpsc::channel();
        let server = thread::spawn(move || {
            let mut stream = accept_loopback(&listener, Duration::from_secs(3)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    return;
                }
                request.push(byte[0]);
            }
            accepted_tx.send(()).unwrap();
            // Hold response headers until the client drops the request.
            let _ = stream.read(&mut byte);
        });

        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let url = format!("http://{address}/audio");
        let (result_tx, result_rx) = std_mpsc::channel();
        let worker = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let result =
                await_remote_reader_setup(&runtime, worker_cancel, Some(deadline), async move {
                    let client = stream_download::http::reqwest::Client::builder()
                        .no_proxy()
                        .connect_timeout(Duration::from_secs(1))
                        .build()
                        .map_err(|error| error.to_string())?;
                    let url = stream_download::http::reqwest::Url::parse(&url)
                        .map_err(|error| error.to_string())?;
                    stream_download::http::HttpStream::new(client, url)
                        .await
                        .map_err(|error| error.to_string())
                })
                .map(|_| ());
            result_tx.send(result).unwrap();
        });

        if accepted_rx.recv_timeout(Duration::from_secs(3)).is_err() {
            cancelled.store(true, Ordering::Relaxed);
            let _ = result_rx.recv_timeout(Duration::from_secs(1));
            let _ = worker.join();
            let _ = server.join();
            panic!("request never reached the stalled-header fixture");
        }
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        worker.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn cancellation_reaches_a_stalled_initial_prefetch_read() {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut stream = accept_loopback(&listener, Duration::from_secs(3)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    return;
                }
                request.push(byte[0]);
            }
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\nx");
            let _ = stream.flush();
            let _ = stream.read(&mut byte);
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let session = Arc::new(DecoderSession::default());
        let cancelled = Arc::new(AtomicBool::new(false));
        session.bind(&cancelled);
        let url = format!("http://{address}/audio");
        let reader = await_remote_reader_setup(&runtime, cancelled.clone(), None, async move {
            use stream_download::{
                storage::{adaptive::AdaptiveStorageProvider, temp::TempStorageProvider},
                Settings, StreamDownload,
            };
            let client = stream_download::http::reqwest::Client::builder()
                .no_proxy()
                .build()
                .map_err(|error| error.to_string())?;
            let url = stream_download::http::reqwest::Url::parse(&url)
                .map_err(|error| error.to_string())?;
            let stream = stream_download::http::HttpStream::new(client, url)
                .await
                .map_err(|error| error.to_string())?;
            StreamDownload::from_stream(
                stream,
                AdaptiveStorageProvider::new(
                    TempStorageProvider::default(),
                    std::num::NonZeroUsize::new(2 * 1024 * 1024).unwrap(),
                ),
                Settings::default().prefetch_bytes(32 * 1024),
            )
            .await
            .map_err(|error| format!("{error:?}"))
        })
        .unwrap();

        let download_cancelled = Arc::new(AtomicBool::new(false));
        let download_flag = download_cancelled.clone();
        let finished_flag = download_cancelled.clone();
        let token = reader.cancellation_token();
        register_download_cancellation(
            &runtime,
            &session,
            move || {
                download_flag.store(true, Ordering::Relaxed);
                token.cancel();
            },
            move || finished_flag.load(Ordering::Relaxed),
        );
        let stop_flag = cancelled.clone();
        let stop = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            stop_flag.store(true, Ordering::Relaxed);
        });
        runtime.block_on(async {
            let read = tokio::task::spawn_blocking(move || {
                let mut reader = reader;
                let mut buffer = [0u8; 4096];
                reader.read(&mut buffer)
            });
            tokio::time::timeout(Duration::from_secs(1), async {
                while !download_cancelled.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("startup cancellation did not reach the active download");
            let _ = tokio::time::timeout(Duration::from_secs(1), read)
                .await
                .expect("cancelled prefetch reader remained blocked")
                .unwrap();
        });
        stop.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn superseded_seek_does_not_wait_for_busy_decoder() {
        let session = DecoderSession::default();
        let _busy = session.decoder.lock().unwrap();
        assert!(session.lock_decoder(&AtomicBool::new(true)).is_err());
    }

    #[test]
    fn paused_seek_output_never_starts_audible_playback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("paused.wav");
        std::fs::write(&path, pcm_wav(48_000, 1, 1)).unwrap();
        let (mixer, mut rendered) = rodio::mixer::mixer(
            NonZeroU16::new(2).unwrap(),
            NonZeroU32::new(48_000).unwrap(),
        );
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        let playback = start_playback(
            &mixer,
            0.8,
            2,
            AudioSource {
                input: path.to_string_lossy().into(),
                ffmpeg: "ffmpeg-unavailable".into(),
                headers: vec![],
                decoder_hint: None,
            },
            0.5,
            None,
            0.5,
            48_000,
            2,
            events,
            crate::pcm_tap::PcmTap::new(),
            Arc::new(DecoderSession::default()),
            true,
        );
        assert!(playback.player.is_paused());
        for _ in 0..1000 {
            assert_eq!(rendered.next(), Some(0.));
        }
        assert_eq!(playback.samples.load(Ordering::Relaxed), 0);
        playback.cancel.store(true, Ordering::Relaxed);
        playback.player.stop();
        for _ in 0..1000 {
            let _ = rendered.next();
        }
    }

    #[test]
    fn cancellation_handoff_tracks_latest_seek_not_retired_source() {
        let session = DecoderSession::default();
        let old = Arc::new(AtomicBool::new(false));
        let next = Arc::new(AtomicBool::new(false));
        session.bind(&old);
        session.bind(&next);
        old.store(true, Ordering::Relaxed);
        assert!(!session.cancelled());
        next.store(true, Ordering::Relaxed);
        assert!(session.cancelled());
    }

    #[test]
    fn buffered_http_seeks_do_not_reopen_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let wav = pcm_wav(48_000, 1, 5);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n", wav.len()).unwrap();
            stream.write_all(&wav).unwrap();
            // The listening socket is dropped: a new extraction/download cannot succeed.
        });
        let source = AudioSource {
            input: format!("http://{address}/audio.wav"),
            ffmpeg: "ffmpeg-unavailable".into(),
            headers: vec![],
            decoder_hint: None,
        };
        let session = Arc::new(DecoderSession::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let initial = decode_window(source.clone(), session.clone(), 0., 5., cancel.clone());
        assert!(!initial.is_empty());
        server.join().unwrap();
        for position in [3., 1., 4., 0.] {
            let samples = decode_excerpt(source.clone(), session.clone(), position, cancel.clone());
            assert_eq!(samples.len(), 2400);
        }
        cancel.store(true, Ordering::Relaxed);
    }

    #[test]
    fn unbuffered_http_seek_uses_ranges_and_keeps_recording_position() {
        struct Fixture {
            stop: Arc<AtomicBool>,
            worker: Option<thread::JoinHandle<()>>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Relaxed);
                if let Some(worker) = self.worker.take() {
                    worker.join().unwrap();
                }
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut wav = pcm_wav(48_000, 1, 30);
        for frame in 0..48_000usize * 30 {
            let value = (frame / 48_000 + 1) as i16 * 1000;
            wav[44 + frame * 2..46 + frame * 2].copy_from_slice(&value.to_le_bytes());
        }
        let bytes = Arc::new(wav);
        let stop = Arc::new(AtomicBool::new(false));
        let ranged = Arc::new(AtomicBool::new(false));
        let bounded_range = Arc::new(AtomicBool::new(false));
        let (server_stop, server_ranged, server_bounded_range) =
            (stop.clone(), ranged.clone(), bounded_range.clone());
        let worker = thread::spawn(move || {
            let mut clients = Vec::new();
            while !server_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (bytes, stop, ranged, bounded_range) = (
                            bytes.clone(),
                            server_stop.clone(),
                            server_ranged.clone(),
                            server_bounded_range.clone(),
                        );
                        clients.push(thread::spawn(move || {
                            // Accepted sockets can inherit O_NONBLOCK from the listener on
                            // macOS. This fixture reads the request with blocking read_exact.
                            stream
                                .set_nonblocking(false)
                                .expect("configure accepted seek fixture socket as blocking");
                            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                            let mut request = Vec::new();
                            let mut byte = [0u8;1];
                            while !request.ends_with(b"\r\n\r\n") {
                                if stream.read_exact(&mut byte).is_err() { return; }
                                request.push(byte[0]);
                            }
                            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                            let range = text.lines().find_map(|line| line.strip_prefix("range: bytes="));
                            let (start, requested_end) = match range {
                                Some(range) => {
                                    let Some((start, end)) = range.split_once('-') else { return; };
                                    let Ok(start) = start.parse::<usize>() else { return; };
                                    let end = if end.is_empty() { None } else {
                                        let Ok(end) = end.parse::<usize>() else { return; };
                                        Some(end)
                                    };
                                    (Some(start), end)
                                }
                                None => (None, None),
                            };
                            let offset = start.unwrap_or(0);
                            if offset >= bytes.len() { return; }
                            let end = requested_end.unwrap_or(bytes.len()-1).min(bytes.len()-1);
                            if end < offset { return; }
                            if offset > 0 { ranged.store(true, Ordering::Relaxed); }
                            if requested_end.is_some_and(|end| end < bytes.len()-1) {
                                bounded_range.store(true, Ordering::Relaxed);
                            }
                            let status = if start.is_some() { "206 Partial Content" } else { "200 OK" };
                            if write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: audio/wav\r\nConnection: close\r\n", end-offset+1).is_err() { return; }
                            if start.is_some() && write!(stream, "Content-Range: bytes {}-{}/{}\r\n", offset, end, bytes.len()).is_err() { return; }
                            if stream.write_all(b"\r\n").is_err() { return; }
                            let began = Instant::now();
                            for (index, chunk) in bytes[offset..=end].chunks(8192).enumerate() {
                                if stop.load(Ordering::Relaxed) { break; }
                                // Force the first far seek to request bytes not downloaded yet.
                                while offset == 0 && index >= 8 && !ranged.load(Ordering::Relaxed)
                                    && !stop.load(Ordering::Relaxed) && began.elapsed() < Duration::from_secs(3) {
                                    thread::sleep(Duration::from_millis(1));
                                }
                                if stream.write_all(chunk).is_err() { break; }
                                thread::sleep(Duration::from_millis(1));
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            }
            for client in clients {
                client.join().unwrap();
            }
        });
        let _fixture = Fixture {
            stop,
            worker: Some(worker),
        };
        let session = Arc::new(DecoderSession::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let source = AudioSource {
            input: format!("http://{address}/range.wav"),
            ffmpeg: "ffmpeg-unavailable".into(),
            headers: vec![],
            decoder_hint: None,
        };
        for position in [0., 25., 1., 28., 0.] {
            let output = decode_excerpt(source.clone(), session.clone(), position, cancel.clone());
            assert_eq!(output.len(), 2400);
            let expected = (position as i16 + 1) as f32 * 1000. / 32768.;
            assert!(
                (output[0] - expected).abs() < 0.001,
                "incorrect source position after {position}s seek"
            );
        }
        assert!(
            ranged.load(Ordering::Relaxed),
            "far seek must exercise a real range request"
        );
        assert!(
            bounded_range.load(Ordering::Relaxed),
            "backward seek must exercise a bounded range request"
        );
        cancel.store(true, Ordering::Relaxed);
    }

    #[test]
    fn ffmpeg_cleanup_reaps_the_child_even_on_early_exit() {
        use std::process::{Command, Stdio};
        let child = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "anullsrc",
                "-t",
                "60",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let child = Arc::new(std::sync::Mutex::new(child));
        let done = Arc::new(AtomicBool::new(false));
        drop(FfmpegChildReaper {
            child: child.clone(),
            watcher_done: done.clone(),
            stderr: None,
        });
        assert!(done.load(Ordering::Relaxed));
        #[cfg(target_os = "linux")]
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "a zombie is still an unreaped process"
        );
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
        assert!(child.lock().unwrap().try_wait().unwrap().is_some());
    }

    #[cfg(target_os = "linux")]
    fn decoder_child_cpu_ms() -> Option<f64> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // Only used by the isolated, single-test benchmark process. Child processes
        // are reaped before sampling, so their consumed CPU is accounted here.
        if unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) } != 0 {
            return None;
        }
        let usage = unsafe { usage.assume_init() };
        Some(
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.,
        )
    }
    #[cfg(not(target_os = "linux"))]
    fn decoder_child_cpu_ms() -> Option<f64> {
        None
    }

    /// Explicit isolated live reproduction: production resolver and decoder,
    /// without a device, mixer, GUI, or the normal application's Store/queue.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires exact BURRATA_LIVE_TRACK_FIXTURE and BURRATA_LIVE_TRACK_OUTPUT; public network"]
    async fn live_reported_track_resolver_and_decode() {
        let fixture = std::env::var("BURRATA_LIVE_TRACK_FIXTURE")
            .expect("explicit exact Track fixture required");
        let output =
            std::env::var("BURRATA_LIVE_TRACK_OUTPUT").expect("explicit report path required");
        let track: crate::models::Track =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        assert!(
            track.local_path.is_none(),
            "live catalog fixture must not reference normal local files"
        );
        let isolated = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::Store::open(&isolated.path().join("isolated.sqlite")).unwrap());
        store
            .save_settings(&crate::models::Settings::default())
            .unwrap();
        store.save_track(&track).unwrap();
        let resources = std::env::var_os("BURRATA_LIVE_RESOURCES")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"));
        crate::tools::initialize(resources.clone());
        let yt = crate::tools::bundled_executable("yt-dlp").expect("packaged yt-dlp required");
        let node = crate::tools::bundled_executable("node").expect("packaged Node required");
        let manager = crate::youtube_support::YouTubeSupportManager::new(
            isolated.path().join("youtube-support"),
            yt,
            Some(node),
            Some(resources.join("youtube-support/bgutil/server/build/main.js")),
            Some(resources.join("youtube-support/bgutil/plugin")),
        );
        crate::youtube_support::initialize_global(manager.clone()).unwrap();
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = crate::acquisition::AcquisitionManager::new_for_playback_test(
            store.clone(),
            connections,
        );
        let (_stop, cancel) = tokio::sync::watch::channel(false);
        let started = Instant::now();
        let result =
            crate::playback_sources::resolve(&store, &acquisition, &track, None, cancel).await;
        let resolve_ms = started.elapsed().as_secs_f64() * 1000.;
        let mut report = serde_json::json!({"schemaVersion":1,"workload":"isolated_reported_track_resolver_and_decoder","platform":std::env::consts::OS,"build":if cfg!(debug_assertions){"debug"}else{"release"},"resolverBudgetMs":20000,"resolveMs":resolve_ms,"resolved":result.is_ok(),"scope":"production source resolver and first five seconds of decoder PCM; no mixer, device, GUI, normal library, or queue mutation"});
        let alias_context =
            crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, &track);
        report["aliasProofState"] = serde_json::json!({
            "hasVerifiedCreditSets":alias_context.has_verified_credit_sets(),
            "materializedAliasCount":alias_context.tracks().len(),
            "scope":"isolated store populated only by production resolver evidence discovery"
        });
        fn category(error: &str) -> &'static str {
            let error = error.to_ascii_lowercase();
            if error.contains("deadline") || error.contains("budget") || error.contains("timed out")
            {
                "deadline_or_network_timeout"
            } else if error.contains("match") || error.contains("identity") {
                "recording_identity_unverified"
            } else if error.contains("runtime")
                || error.contains("executable")
                || error.contains("spawn")
            {
                "tool_runtime"
            } else {
                "source_unavailable"
            }
        }
        let success = match result {
            Err(error) => {
                report["failureStage"] = serde_json::json!("resolver");
                report["failureCategory"] = serde_json::json!(category(&error));
                false
            }
            Ok(resolved) => {
                report["sourceProvider"] = serde_json::json!(resolved
                    .candidate
                    .as_ref()
                    .map(|candidate| candidate.provider.as_str()));
                if let Some(candidate) = &resolved.candidate {
                    let metadata = candidate.source_metadata.as_ref();
                    // Public recording fields only: never include source URLs,
                    // headers, credentials, extractor payloads, or signed URLs.
                    report["resolvedRecordingIdentity"] = serde_json::json!({
                        "provider":candidate.provider,"providerId":candidate.id,
                        "title":candidate.title,"durationSeconds":candidate.duration_seconds,
                        "channelId":metadata.and_then(|value| value["channelId"].as_str()),
                        "channel":metadata.and_then(|value| value["channel"].as_str()),
                        "channelIsVerified":metadata.and_then(|value| value["channelIsVerified"].as_bool()),
                        "track":metadata.and_then(|value| value["track"].as_str()),
                        "artists":metadata.and_then(|value| value["artists"].as_array())
                            .map(|values| values.iter().filter_map(serde_json::Value::as_str).take(16).collect::<Vec<_>>()),
                        "hasTopicDescriptionCredit":metadata.and_then(|value| value["descriptionCreditLine"].as_str()).is_some(),
                    });
                }
                let source = resolved.source;
                report["sourceHint"] = serde_json::json!(source.decoder_hint.as_ref().map(|hint| serde_json::json!({"mimeType":hint.mime_type,"codec":hint.codec,"hasVideo":hint.has_video,"contentLength":hint.content_length})));
                let decoded = tokio::task::spawn_blocking(move || {
                    let decode_started = Instant::now();
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let worker_cancelled = cancelled.clone();
                    let diagnostics = Arc::new(std::sync::Mutex::new(None));
                    let worker_diagnostics = diagnostics.clone();
                    let session = Arc::new(DecoderSession::default());
                    session.bind(&cancelled);
                    let (tx, rx) = std_mpsc::sync_channel(16);
                    let worker = thread::spawn(move || {
                        decode_with_session_and_diagnostics(
                            source,
                            0.,
                            Some(5.),
                            48_000,
                            2,
                            tx,
                            Arc::new(AtomicU64::new(0)),
                            Arc::new(std::sync::atomic::AtomicU32::new(0)),
                            Arc::new(std::sync::atomic::AtomicU32::new(0)),
                            Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                            Some(worker_diagnostics),
                            worker_cancelled,
                            session,
                        )
                    });
                    let deadline = Instant::now() + Duration::from_secs(20);
                    let mut first_pcm_ms = None;
                    let mut samples = 0usize;
                    while samples < 5 * 48_000 * 2 {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(remaining) {
                            Ok(block) => {
                                if !block.is_empty() && first_pcm_ms.is_none() {
                                    first_pcm_ms =
                                        Some(decode_started.elapsed().as_secs_f64() * 1000.);
                                }
                                samples += block.len();
                            }
                            Err(_) => break,
                        }
                    }
                    cancelled.store(true, Ordering::Relaxed);
                    drop(rx);
                    let decoder_result = worker.join().expect("decoder worker panicked");
                    let diagnostics = diagnostics.lock().unwrap().clone();
                    (
                        first_pcm_ms,
                        samples,
                        decode_started.elapsed().as_secs_f64() * 1000.,
                        decoder_result,
                        diagnostics,
                    )
                })
                .await
                .unwrap();
                report["decoderDiagnostics"] = serde_json::json!(decoded.4);
                report["firstPcmAfterResolveMs"] = serde_json::json!(decoded.0);
                report["decodedInterleavedSamples"] = serde_json::json!(decoded.1);
                report["decodedSeconds"] = serde_json::json!(decoded.1 as f64 / 96_000.);
                report["decodeWallMs"] = serde_json::json!(decoded.2);
                report["firstPcmFromRequestMs"] =
                    serde_json::json!(decoded.0.map(|first| first + resolve_ms));
                let ok = decoded.1 >= 5 * 48_000 * 2;
                if !ok {
                    report["failureStage"] = serde_json::json!("decoder");
                    report["failureCategory"] = serde_json::json!(decoded
                        .3
                        .as_ref()
                        .err()
                        .map_or("insufficient_pcm", |error| category(error)));
                }
                ok
            }
        };
        report["ok"] = serde_json::json!(success);
        report["totalMs"] = serde_json::json!(started.elapsed().as_secs_f64() * 1000.);
        let _ = manager.shutdown_for_exit().await;
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        println!("{}", report);
        assert!(
            success,
            "reported-track attempt failed; bounded redacted outcome retained"
        );
    }

    /// Local attribution only: whether a 16KiB readable WAV prefix can produce
    /// PCM before the server releases the rest of the body after 300ms.
    #[test]
    #[ignore = "explicit loopback prefetch attribution benchmark"]
    fn paced_reader_startup_attribution() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let wav = pcm_wav(48_000, 2, 2);
        let (release, first_pcm) = std_mpsc::channel();
        let server = thread::spawn(move || {
            let mut stream = accept_loopback(&listener, Duration::from_secs(3)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",wav.len()).unwrap();
            stream.write_all(&wav[..16 * 1024]).unwrap();
            stream.flush().unwrap();
            let before_remaining = first_pcm.recv_timeout(Duration::from_millis(300)).is_ok();
            let _ = stream.write_all(&wav[16 * 1024..]);
            before_remaining
        });
        let source = AudioSource {
            input: format!("http://{address}/fixture.wav"),
            ffmpeg: "ffmpeg".into(),
            headers: vec![],
            decoder_hint: None,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let diagnostics = Arc::new(std::sync::Mutex::new(None));
        let worker_diagnostics = diagnostics.clone();
        let session = Arc::new(DecoderSession::default());
        session.bind(&cancelled);
        let (tx, rx) = std_mpsc::sync_channel(16);
        let started = Instant::now();
        let worker = thread::spawn(move || {
            decode_with_session_and_diagnostics(
                source,
                0.,
                Some(1.),
                48_000,
                2,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                Some(worker_diagnostics),
                worker_cancelled,
                session,
            )
        });
        let first = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("bounded first PCM");
        let first_ms = started.elapsed().as_secs_f64() * 1000.;
        let _ = release.send(());
        let count = first.len() + rx.into_iter().map(|block| block.len()).sum::<usize>();
        worker.join().unwrap().unwrap();
        let before_remaining = server.join().unwrap();
        assert!(count >= 48_000 * 2, "one second decoded PCM");
        println!(
            "{}",
            serde_json::json!({"workload":"paced_reader_startup_attribution","initialBodyBytes":16*1024,"bodyReleaseWaitMs":300,"firstPcmBeforeRemainingBody":before_remaining,"firstPcmMs":first_ms,"decodedSamples":count,"decoderDiagnostics":diagnostics.lock().unwrap().clone()})
        );
    }

    /// Decoder benchmark only: excludes resolver, mixer, device and GUI latency.
    #[test]
    #[ignore = "requires BURRATA_SEEK_BENCH_FIXTURE and BURRATA_SEEK_BENCH_OUTPUT"]
    fn seek_decoder_benchmark() {
        let fixture = std::env::var("BURRATA_SEEK_BENCH_FIXTURE").unwrap();
        let output = std::env::var("BURRATA_SEEK_BENCH_OUTPUT").unwrap();
        let duration: f64 = std::env::var("BURRATA_SEEK_BENCH_DURATION")
            .unwrap_or("300".into())
            .parse()
            .unwrap();
        let source = AudioSource {
            input: fixture,
            ffmpeg: "ffmpeg".into(),
            headers: vec![],
            decoder_hint: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let compare_opus = cfg!(feature = "experimental-opus")
            && std::env::var_os("BURRATA_SEEK_BENCH_COMPARE_OPUS").is_some();
        let retained = Arc::new(DecoderSession::default());
        let mut rows = Vec::new();
        if compare_opus {
            for _ in 0..3 {
                std::env::set_var("BURRATA_FORCE_FFMPEG", "1");
                decode_excerpt(
                    source.clone(),
                    Arc::new(DecoderSession::default()),
                    duration * 0.5,
                    cancel.clone(),
                );
                std::env::remove_var("BURRATA_FORCE_FFMPEG");
                decode_excerpt(
                    source.clone(),
                    retained.clone(),
                    duration * 0.5,
                    cancel.clone(),
                );
            }
        } else {
            std::env::remove_var("BURRATA_FORCE_FFMPEG");
            for _ in 0..3 {
                decode_excerpt(
                    source.clone(),
                    Arc::new(DecoderSession::default()),
                    duration * 0.5,
                    cancel.clone(),
                );
                decode_excerpt(
                    source.clone(),
                    retained.clone(),
                    duration * 0.5,
                    cancel.clone(),
                );
            }
        }
        for iteration in 0..20 {
            for fraction in [0.01, 0.5, 0.95] {
                // Alternate which strategy runs first to reduce order bias.
                let strategies = if compare_opus {
                    if iteration % 2 == 0 {
                        ["ffmpegFresh", "opusRetained"]
                    } else {
                        ["opusRetained", "ffmpegFresh"]
                    }
                } else if iteration % 2 == 0 {
                    ["fresh", "retained"]
                } else {
                    ["retained", "fresh"]
                };
                for strategy in strategies {
                    if compare_opus && strategy == "ffmpegFresh" {
                        std::env::set_var("BURRATA_FORCE_FFMPEG", "1");
                    } else {
                        std::env::remove_var("BURRATA_FORCE_FFMPEG");
                    }
                    let session = if strategy == "retained" {
                        retained.clone()
                    } else if strategy == "opusRetained" {
                        retained.clone()
                    } else {
                        Arc::new(DecoderSession::default())
                    };
                    session.bind(&cancel);
                    let (tx, rx) = std_mpsc::sync_channel(2);
                    let (source, cancel) = (source.clone(), cancel.clone());
                    let child_cpu_before = decoder_child_cpu_ms();
                    let started = Instant::now();
                    let decode = thread::spawn(move || {
                        #[cfg(target_os = "linux")]
                        fn cpu_ms() -> f64 {
                            let mut time = libc::timespec {
                                tv_sec: 0,
                                tv_nsec: 0,
                            };
                            // Thread CPU time excludes waiting for ranges and channel backpressure.
                            assert_eq!(
                                unsafe {
                                    libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time)
                                },
                                0
                            );
                            time.tv_sec as f64 * 1000. + time.tv_nsec as f64 / 1_000_000.
                        }
                        #[cfg(not(target_os = "linux"))]
                        fn cpu_ms() -> f64 {
                            0.
                        }
                        let cpu = cpu_ms();
                        let result = decode_with_session(
                            source,
                            duration * fraction,
                            None,
                            48_000,
                            2,
                            tx,
                            Arc::new(AtomicU64::new(0)),
                            Arc::new(std::sync::atomic::AtomicU32::new(0)),
                            Arc::new(std::sync::atomic::AtomicU32::new(0)),
                            Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                            cancel,
                            session,
                        );
                        (result, cpu_ms() - cpu)
                    });
                    let first = rx
                        .recv_timeout(Duration::from_secs(15))
                        .expect("first decoded PCM");
                    let first_ms = started.elapsed().as_secs_f64() * 1000.;
                    assert!(!first.is_empty() && first.iter().all(|sample| sample.is_finite()));
                    drop(rx);
                    let (result, cpu) = decode.join().unwrap();
                    result.expect("seek decoding");
                    let child_cpu = decoder_child_cpu_ms()
                        .zip(child_cpu_before)
                        .map(|(after, before)| (after - before).max(0.));
                    rows.push(serde_json::json!({"strategy":strategy,"iteration":iteration,"fraction":fraction,
                        "firstPcmMs":first_ms,"decodeThreadCpuMs":cpu,"decoderChildCpuMs":child_cpu,"wallMs":started.elapsed().as_secs_f64()*1000.}));
                }
            }
        }
        std::env::remove_var("BURRATA_FORCE_FFMPEG");
        std::fs::write(output, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    }

    #[test]
    fn http_stream_keeps_runtime_alive_while_audio_arrives() {
        use std::io::Read;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
        let address = listener.local_addr().unwrap();
        let wav = pcm_wav(48_000, 1, 5);
        let wav_len = wav.len();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept stream client");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    return;
                }
                request.push(byte[0]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {wav_len}\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            for chunk in wav.chunks(8 * 1024) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let (tx, rx) = std_mpsc::sync_channel(16);
        let url = format!("http://{address}/audio.wav");
        let decode = thread::spawn(move || {
            decode_to_blocks(
                AudioSource {
                    input: url,
                    ffmpeg: "ffmpeg".into(),
                    headers: Vec::new(),
                    decoder_hint: None,
                },
                0.0,
                None,
                44_100,
                2,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                Arc::new(AtomicBool::new(false)),
            )
        });
        let output_samples: usize = rx.into_iter().map(|block| block.len()).sum();
        decode.join().unwrap().expect("decode streaming response");
        server.join().unwrap();
        assert!((output_samples as i64 - 44_100 * 2 * 5).abs() < 441);
    }
}

#[cfg(test)]
mod seek_queue_tests {
    use super::*;
    #[test]
    fn latest_seek_wins_without_crossing_pause() {
        let (tx, rx) = std_mpsc::channel();
        tx.send(OutputCommand::Seek {
            generation: 2,
            position: 20.,
        })
        .unwrap();
        tx.send(OutputCommand::Pause).unwrap();
        tx.send(OutputCommand::Seek {
            generation: 3,
            position: 30.,
        })
        .unwrap();
        let mut pending = None;
        let result = coalesce_seeks(
            Ok(OutputCommand::Seek {
                generation: 1,
                position: 10.,
            }),
            &rx,
            &mut pending,
        );
        assert!(matches!(
            result,
            Ok(OutputCommand::Seek {
                generation: 2,
                position: 20.
            })
        ));
        assert!(matches!(pending, Some(OutputCommand::Pause)));
        assert!(matches!(
            rx.try_recv(),
            Ok(OutputCommand::Seek { generation: 3, .. })
        ));
    }
}

#[cfg(test)]
#[path = "native_preload_tests.rs"]
mod reliability_tests;

#[cfg(test)]
#[path = "native_direct_pcm_experiment.rs"]
pub(crate) mod direct_pcm_experiment;

#[cfg(all(test, not(feature = "experimental-opus")))]
#[path = "native_direct_opus_tests.rs"]
mod direct_opus_tests;

/// Diagnostic-only comparison: production preflight + native URL open versus
/// a validated owned StreamDownload handed to the same native decoder. No device.
#[cfg(test)]
mod validation_handoff_diagnostic {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::net::TcpListener;

    #[derive(Clone, Copy, Debug)]
    enum Fault {
        None,
        WrongTailRange,
        TruncatedTail,
        CorruptAudio,
        WrongInitialRange,
        WrongInitialLength,
        InitialHtml,
        TruncatedPrefix,
    }

    // The downloader may repair a short response with another range request.
    // Observe the underlying transport, rather than mistaking a completed read
    // for equivalence with preflight's strict single-response validation.
    #[derive(Clone)]
    struct ObservedHttpClient {
        inner: native_http::NativeHttpClient,
        failures: Arc<AtomicU64>,
    }
    struct ObservedHttpResponse {
        inner: native_http::NativeHttpResponse,
        failures: Arc<AtomicU64>,
    }
    impl stream_download::http::Client for ObservedHttpClient {
        type Url = <native_http::NativeHttpClient as stream_download::http::Client>::Url;
        type Headers = <native_http::NativeHttpClient as stream_download::http::Client>::Headers;
        type Response = ObservedHttpResponse;
        type Error = native_http::NativeHttpError;
        fn create() -> Self {
            Self {
                inner: stream_download::http::Client::create(),
                failures: Arc::new(AtomicU64::new(0)),
            }
        }
        async fn get(&self, url: &Self::Url) -> Result<Self::Response, Self::Error> {
            let response = stream_download::http::Client::get(&self.inner, url).await;
            if response.is_err() {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
            response.map(|inner| ObservedHttpResponse {
                inner,
                failures: self.failures.clone(),
            })
        }
        async fn get_range(
            &self,
            url: &Self::Url,
            start: u64,
            end: Option<u64>,
        ) -> Result<Self::Response, Self::Error> {
            let response =
                stream_download::http::Client::get_range(&self.inner, url, start, end).await;
            if response.is_err() {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
            response.map(|inner| ObservedHttpResponse {
                inner,
                failures: self.failures.clone(),
            })
        }
    }
    impl stream_download::http::ClientResponse for ObservedHttpResponse {
        type ResponseError = native_http::NativeHttpError;
        type StreamError = native_http::NativeHttpError;
        type Headers =
            <native_http::NativeHttpResponse as stream_download::http::ClientResponse>::Headers;
        fn content_length(&self) -> Option<u64> {
            stream_download::http::ClientResponse::content_length(&self.inner)
        }
        fn content_type(&self) -> Option<&str> {
            stream_download::http::ClientResponse::content_type(&self.inner)
        }
        fn headers(&self) -> Self::Headers {
            stream_download::http::ClientResponse::headers(&self.inner)
        }
        fn into_result(self) -> Result<Self, Self::ResponseError> {
            let Self { inner, failures } = self;
            let result = stream_download::http::ClientResponse::into_result(inner);
            if result.is_err() {
                failures.fetch_add(1, Ordering::Relaxed);
            }
            result.map(|inner| Self { inner, failures })
        }
        fn stream(
            self,
        ) -> Box<
            dyn futures_util::Stream<Item = Result<axum::body::Bytes, Self::StreamError>>
                + Unpin
                + Send
                + Sync,
        > {
            use futures_util::StreamExt;
            let failures = self.failures;
            Box::new(
                stream_download::http::ClientResponse::stream(self.inner).inspect(move |result| {
                    if result.is_err() {
                        failures.fetch_add(1, Ordering::Relaxed);
                    }
                }),
            )
        }
    }

    #[derive(Clone, serde::Serialize)]
    struct Request {
        ranged: bool,
        start: usize,
        end: usize,
        accepted_body_bytes: usize,
    }

    struct Fixture {
        url: String,
        length: usize,
        requests: Arc<std::sync::Mutex<Vec<Request>>>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }
    impl Fixture {
        fn start(rtt_ms: u64, fault: Fault) -> Self {
            let mut wav = Vec::new();
            let frames = 48_000usize * 15;
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + frames as u32 * 2).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&48_000u32.to_le_bytes());
            wav.extend_from_slice(&96_000u32.to_le_bytes());
            wav.extend_from_slice(&2u16.to_le_bytes());
            wav.extend_from_slice(&16u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&(frames as u32 * 2).to_le_bytes());
            for frame in 0..frames {
                wav.extend_from_slice(&(((frame % 97) as i16 - 48) * 300).to_le_bytes());
            }
            if matches!(fault, Fault::CorruptAudio) {
                wav[..64].fill(b'X');
            }
            let bytes = Arc::new(wav);
            let length = bytes.len();
            assert!(
                length > 1024 * 1024,
                "must exercise production tail preflight"
            );
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/fixture.wav", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (worker_stop, worker_requests) = (stop.clone(), requests.clone());
            let worker = thread::spawn(move || {
                let mut clients = Vec::new();
                while !worker_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut socket, _)) => {
                            let (bytes, stop, requests) =
                                (bytes.clone(), worker_stop.clone(), worker_requests.clone());
                            clients.push(thread::spawn(move || {
                                // The listener is polled in nonblocking mode; normalize each
                                // accepted socket before the fixture performs blocking reads.
                                socket
                                    .set_nonblocking(false)
                                    .expect(
                                        "configure accepted diagnostic fixture socket as blocking",
                                    );
                                let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
                                let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
                                let mut header = Vec::new(); let mut byte = [0];
                                while !header.ends_with(b"\r\n\r\n") && header.len() < 16_384 {
                                    if socket.read_exact(&mut byte).is_err() { return; } header.push(byte[0]);
                                }
                                let header = String::from_utf8_lossy(&header).to_ascii_lowercase();
                                let range = header.lines().find_map(|line| line.strip_prefix("range: bytes="));
                                let (start, end) = range.and_then(|range| range.split_once('-')).map(|(start,end)| {
                                    (start.parse::<usize>().unwrap(), if end.is_empty() { bytes.len()-1 } else { end.parse::<usize>().unwrap().min(bytes.len()-1) })
                                }).unwrap_or((0,bytes.len()-1));
                                if start >= bytes.len() || end < start { return; }
                                let index = { let mut rows = requests.lock().unwrap(); let index = rows.len(); rows.push(Request {ranged:range.is_some(),start,end,accepted_body_bytes:0}); index };
                                thread::sleep(Duration::from_millis(rtt_ms));
                                let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
                                let is_tail = start >= bytes.len()-1024;
                                let declared_length = end-start+1 + usize::from(range.is_none() && matches!(fault, Fault::WrongInitialLength));
                                let mime = if range.is_none() && matches!(fault, Fault::InitialHtml) { "text/html" } else { "audio/wav" };
                                if write!(socket,"HTTP/1.1 {status}\r\nContent-Length: {declared_length}\r\nContent-Type: {mime}\r\nAccept-Ranges: bytes\r\nETag: \"fixture-stable\"\r\nConnection: close\r\n").is_err(){return;}
                                if range.is_none() && matches!(fault, Fault::WrongInitialRange) {
                                    if write!(socket,"Content-Range: bytes 1-{end}/{}\r\n",bytes.len()).is_err(){return;}
                                }
                                if range.is_some() {
                                    let reported_start = if is_tail && matches!(fault,Fault::WrongTailRange) { start+1 } else { start };
                                    if write!(socket,"Content-Range: bytes {reported_start}-{end}/{}\r\n",bytes.len()).is_err(){return;}
                                }
                                if socket.write_all(b"\r\n").is_err(){return;}
                                let actual_end = if (is_tail && matches!(fault,Fault::TruncatedTail)) || (start == 0 && matches!(fault,Fault::TruncatedPrefix)) { start+255 } else { end };
                                for chunk in bytes[start..=actual_end].chunks(8192) {
                                    if stop.load(Ordering::Relaxed) { break; }
                                    if socket.write_all(chunk).is_err() { break; }
                                    requests.lock().unwrap()[index].accepted_body_bytes += chunk.len();
                                    // Keep the tail outside the initial 32KiB buffered prefix.
                                    thread::sleep(Duration::from_millis(2));
                                }
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(1))
                        }
                        Err(error) => panic!("fixture accept: {error}"),
                    }
                }
                for client in clients {
                    client.join().unwrap();
                }
            });
            Self {
                url,
                length,
                requests,
                stop,
                worker: Some(worker),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    struct DeadlineStop {
        done: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }
    impl DeadlineStop {
        fn start(cancel: Arc<AtomicBool>) -> Self {
            let done = Arc::new(AtomicBool::new(false));
            let observed = done.clone();
            let worker = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(6);
                while !observed.load(Ordering::Relaxed) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                if !observed.load(Ordering::Relaxed) {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
            Self {
                done,
                worker: Some(worker),
            }
        }
    }
    impl Drop for DeadlineStop {
        fn drop(&mut self) {
            self.done.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    fn arm(
        fixture: &Fixture,
        handoff: bool,
        enforce_response_errors: bool,
    ) -> (serde_json::Value, Vec<f32>) {
        arm_with_hybrid(fixture, handoff, enforce_response_errors, false)
    }

    fn arm_with_hybrid(
        fixture: &Fixture,
        handoff: bool,
        enforce_response_errors: bool,
        hybrid: bool,
    ) -> (serde_json::Value, Vec<f32>) {
        let session = Arc::new(DecoderSession::default());
        let cancelled = Arc::new(AtomicBool::new(false));
        session.bind(&cancelled);
        session.set_startup_deadline(Some(Instant::now() + Duration::from_secs(5)));
        let _deadline_stop = DeadlineStop::start(cancelled.clone());
        let (tx, rx) = std_mpsc::sync_channel(8);
        let diagnostics = Arc::new(std::sync::Mutex::new(None));
        let source = AudioSource {
            input: fixture.url.clone(),
            ffmpeg: "diagnostic-ffmpeg-unavailable".into(),
            headers: vec![],
            decoder_hint: None,
        };
        let (url, length) = (fixture.url.clone(), fixture.length as u64);
        let (worker_session, worker_cancel, worker_diagnostics) =
            (session.clone(), cancelled.clone(), diagnostics.clone());
        let validation_http_failures = Arc::new(AtomicU64::new(0));
        let observed_failures = validation_http_failures.clone();
        let started = Instant::now();
        let worker = thread::spawn(move || -> Result<serde_json::Value, String> {
            let runtime = network_runtime()?;
            let validation_started = Instant::now();
            let mut owned_reader_setup_ms = None;
            let mut owned_decoder_setup_ms = None;
            if handoff {
                use stream_download::{
                    http::HttpStream,
                    storage::{adaptive::AdaptiveStorageProvider, temp::TempStorageProvider},
                    Settings, StreamDownload,
                };
                let setup_started = Instant::now();
                let faults = worker_session.storage_faults.clone();
                let transport_failures = observed_failures.clone();
                let mut reader = await_remote_reader_setup(
                    runtime,
                    worker_cancel.clone(),
                    worker_session.startup_deadline(),
                    async move {
                        let tail_url = url.clone();
                        let reader_setup = async move {
                            let client = stream_download::http::reqwest::Client::builder()
                                .no_proxy()
                                .connect_timeout(Duration::from_secs(2))
                                .build()
                                .map_err(|e| e.to_string())?;
                            let stream = HttpStream::new(
                                ObservedHttpClient {
                                    inner: native_http::NativeHttpClient::new(client),
                                    failures: transport_failures,
                                },
                                url.parse().map_err(|e: url::ParseError| e.to_string())?,
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                            if stream.content_type().as_ref().is_none_or(|mime| {
                                format!("{}/{}", mime.r#type, mime.subtype) != "audio/wav"
                            }) {
                                return Err("fixture MIME mismatch".into());
                            }
                            StreamDownload::from_stream(
                                stream,
                                AdaptiveStorageProvider::new(
                                    storage_fault::Observed {
                                        inner: TempStorageProvider::with_tempfile_builder(|| {
                                            stream_download::storage::temp::tempfile::Builder::new()
                                                .tempfile()
                                        }),
                                        faults,
                                    },
                                    std::num::NonZeroUsize::new(2 * 1024 * 1024).unwrap(),
                                ),
                                Settings::default().prefetch_bytes(32 * 1024),
                            )
                            .await
                            .map_err(|e| e.to_string())
                        };
                        if hybrid {
                            let tail_probe = async move {
                                let client = reqwest::Client::builder()
                                    .no_proxy()
                                    .redirect(reqwest::redirect::Policy::none())
                                    .timeout(Duration::from_secs(4))
                                    .build()
                                    .map_err(|e| e.to_string())?;
                                let (_stop, mut cancel) = tokio::sync::watch::channel(false);
                                crate::acquisition::diagnostic_verify_preview_audio_tail(
                                    &client,
                                    &tail_url
                                        .parse()
                                        .map_err(|e: url::ParseError| e.to_string())?,
                                    length,
                                    &mut cancel,
                                )
                                .await
                            };
                            let (reader, ()) = tokio::try_join!(reader_setup, tail_probe)?;
                            Ok::<_, String>(reader)
                        } else {
                            reader_setup.await
                        }
                    },
                )?;
                owned_reader_setup_ms = Some(setup_started.elapsed().as_secs_f64() * 1000.);
                if reader.content_length() != Some(length) {
                    return Err("fixture length mismatch".into());
                }
                let token = reader.cancellation_token();
                let abort = token.clone();
                register_download_cancellation(
                    runtime,
                    &worker_session,
                    move || abort.cancel(),
                    move || token.is_cancelled(),
                );
                let decoder_started;
                let decoder = if hybrid {
                    // Retain the actual initial response prefix and rewind inside
                    // this same buffer; never seek the downloader to the tail.
                    let mut retained = BufReader::with_capacity(
                        32 * 1024,
                        Box::new(reader) as Box<dyn ReadSeekSend>,
                    );
                    let mut head = [0u8; 1024];
                    retained
                        .read_exact(&mut head)
                        .map_err(|e| format!("hybrid prefix read: {e}"))?;
                    retained
                        .seek_relative(-1024)
                        .map_err(|e| format!("hybrid cached prefix rewind: {e}"))?;
                    if enforce_response_errors && observed_failures.load(Ordering::Relaxed) > 0 {
                        return Err("Hybrid initial response failed before handoff".into());
                    }
                    decoder_started = Instant::now();
                    build_native_decoder(retained, Some(length))?
                } else {
                    let mut head = [0u8; 1024];
                    reader
                        .read_exact(&mut head)
                        .map_err(|e| format!("owned head read: {e}"))?;
                    reader
                        .seek(SeekFrom::Start(length - 1024))
                        .map_err(|e| format!("owned tail seek: {e}"))?;
                    let mut tail = [0u8; 1024];
                    reader
                        .read_exact(&mut tail)
                        .map_err(|e| format!("owned tail read: {e}"))?;
                    reader
                        .seek(SeekFrom::Start(0))
                        .map_err(|e| format!("owned rewind: {e}"))?;
                    if enforce_response_errors && observed_failures.load(Ordering::Relaxed) > 0 {
                        return Err(
                            "Owned validation observed a rejected HTTP response before handoff"
                                .into(),
                        );
                    }
                    decoder_started = Instant::now();
                    build_native_decoder(
                        BufReader::new(Box::new(reader) as Box<dyn ReadSeekSend>),
                        Some(length),
                    )?
                };
                owned_decoder_setup_ms = Some(decoder_started.elapsed().as_secs_f64() * 1000.);
                *worker_session.decoder.lock().unwrap() = Some(decoder);
            } else {
                let (_stop, mut cancel) = tokio::sync::watch::channel(false);
                runtime.block_on(async {
                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(4))
                        .build()
                        .map_err(|e| e.to_string())?;
                    crate::acquisition::diagnostic_verify_preview_audio_access(
                        &client,
                        &url.parse().map_err(|e: url::ParseError| e.to_string())?,
                        length,
                        &mut cancel,
                    )
                    .await
                })?;
            }
            let validation_ms = validation_started.elapsed().as_secs_f64() * 1000.;
            decode_with_session_and_diagnostics(
                source,
                0.,
                Some(0.25),
                48_000,
                1,
                tx,
                Arc::new(AtomicU64::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(std::sync::atomic::AtomicU32::new(0)),
                Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                Some(worker_diagnostics),
                worker_cancel,
                worker_session,
            )?;
            Ok(
                serde_json::json!({"validationAndPreparationMs":validation_ms,"ownedReaderSetupMs":owned_reader_setup_ms,"ownedDecoderSetupMs":owned_decoder_setup_ms}),
            )
        });
        let mut pcm = Vec::new();
        let mut first_pcm_ms = None;
        while let Ok(block) = rx.recv_timeout(Duration::from_secs(7)) {
            if !block.is_empty() && first_pcm_ms.is_none() {
                first_pcm_ms = Some(started.elapsed().as_secs_f64() * 1000.);
            }
            pcm.extend(block);
        }
        cancelled.store(true, Ordering::Relaxed);
        let result = worker.join().unwrap();
        let mut report = serde_json::json!({"arm":if hybrid{"owned_prefix_parallel_strict_tail"}else if handoff{"owned_validated_reader_handoff"}else{"separate_preflight_then_production_decode"},"ok":result.is_ok(),"firstPcmMs":first_pcm_ms,"pcmSamples":pcm.len(),"decoder":diagnostics.lock().unwrap().clone()});
        match result {
            Ok(stages) => report["stages"] = stages,
            Err(error) => {
                report["failure"] =
                    serde_json::json!(crate::playback::safe_failure_text(&error, 250))
            }
        }
        report["validationObservedHttpFailures"] =
            serde_json::json!(validation_http_failures.load(Ordering::Relaxed));
        report["enforceValidationResponseErrors"] = serde_json::json!(enforce_response_errors);
        let rows = fixture.requests.lock().unwrap().clone();
        let requested_bytes: usize = rows.iter().map(|row| row.end - row.start + 1).sum();
        let mut ranges: Vec<_> = rows.iter().map(|row| (row.start, row.end)).collect();
        ranges.sort_unstable();
        let mut union = 0;
        let mut covered: Option<(usize, usize)> = None;
        for (start, end) in ranges {
            match covered {
                Some((old_start, old_end)) if start <= old_end + 1 => {
                    covered = Some((old_start, old_end.max(end)))
                }
                Some((old_start, old_end)) => {
                    union += old_end - old_start + 1;
                    covered = Some((start, end));
                }
                None => covered = Some((start, end)),
            }
        }
        if let Some((start, end)) = covered {
            union += end - start + 1;
        }
        report["requestCount"] = serde_json::json!(rows.len());
        report["initialByteRequests"] =
            serde_json::json!(rows.iter().filter(|row| row.start == 0).count());
        report["tailRequests"] = serde_json::json!(rows
            .iter()
            .filter(|row| row.start >= fixture.length - 1024)
            .count());
        report["duplicateRequestedBytes"] = serde_json::json!(requested_bytes - union);
        report["requests"] = serde_json::json!(rows);
        (report, pcm)
    }

    fn snapshot(
        trials: &[serde_json::Value],
        safeguards: &[serde_json::Value],
        unguarded: &Option<serde_json::Value>,
        completed: bool,
    ) -> serde_json::Value {
        let report = serde_json::json!({"schemaVersion":2,"completed":completed,"scope":"diagnostic-only local WAV fixtures; production access probe/native decoder baseline versus test-owned reader handoff; no device, queue, live source, credentials, or production change","limitations":["WAV has no trailing container metadata dependency","owned validation performs head/tail seeks sequentially while preflight probes are parallel","owned decoder cache seeding omits native reader counters; compare outer timings and server request accounting","requested byte overlap is distinct from bytes actually received","three trials per simulated RTT are diagnostic, not a release performance claim","response-error rejection is a test-only baseline-equivalence guard, not a production network retry policy"],"trials":trials,"unguardedTruncatedTailControl":unguarded,"safeguards":safeguards});
        if let Some(path) = std::env::var_os("BURRATA_VALIDATION_HANDOFF_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        report
    }

    fn hybrid_snapshot(
        trials: &[serde_json::Value],
        safeguards: &[serde_json::Value],
        completed: bool,
    ) {
        let report = serde_json::json!({
            "schemaVersion":1, "completed":completed,
            "scope":"cfg(test) only; owned initial native reader with retained 1024-byte prefix and independent production strict bounded tail probe in parallel; no production change",
            "limitations":[
                "Initial response uses native complete-resource 200/206 validation, expected total and audio MIME, not a synthetic bounded-206 head response",
                "Exactly 1024 prefix bytes are retained and rewound in the same BufReader; later complete-body errors remain native transport errors",
                "Strict tail uses unchanged production bounded range/length/body checks",
                "Local 15-second PCM WAV, no trailing container metadata dependency, no audio device or live source",
                "Server accepted bytes are successful server writes, not client-received or first-PCM byte counts",
                "Three trials per simulated RTT are diagnostic only"
            ], "trials":trials, "safeguards":safeguards
        });
        if let Some(path) = std::env::var_os("BURRATA_HYBRID_HANDOFF_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
    }

    #[test]
    #[ignore = "diagnostic-only hybrid retained-prefix/strict-tail comparison; run explicitly"]
    fn compare_preflight_same_reader_and_hybrid_handoff() {
        let mut trials = Vec::new();
        let mut safeguards = Vec::new();
        hybrid_snapshot(&trials, &safeguards, false);
        for rtt_ms in [0, 40, 100] {
            for trial in 0..3 {
                let mut reference = None;
                // Rotate arm order to avoid giving one strategy a fixed order.
                for offset in 0..3 {
                    let mode = (trial + offset) % 3;
                    let fixture = Fixture::start(rtt_ms, Fault::None);
                    let (mut report, pcm) = arm_with_hybrid(&fixture, mode != 0, true, mode == 2);
                    report["simulatedHandshakeRttMs"] = serde_json::json!(rtt_ms);
                    report["trial"] = serde_json::json!(trial);
                    trials.push(report.clone());
                    hybrid_snapshot(&trials, &safeguards, false);
                    assert_eq!(report["ok"], true, "{report}");
                    assert_eq!(pcm.len(), 12_000, "{report}");
                    if let Some(reference) = reference.as_ref() {
                        assert_eq!(&pcm, reference);
                    } else {
                        reference = Some(pcm);
                    }
                    if mode == 2 {
                        assert_eq!(
                            report["requestCount"], 2,
                            "hybrid must avoid gap/reopen: {report}"
                        );
                        assert_eq!(report["initialByteRequests"], 1, "{report}");
                        assert_eq!(report["tailRequests"], 1, "{report}");
                    }
                }
            }
        }
        for fault in [
            Fault::WrongTailRange,
            Fault::TruncatedTail,
            Fault::CorruptAudio,
            Fault::WrongInitialRange,
            Fault::WrongInitialLength,
            Fault::InitialHtml,
            Fault::TruncatedPrefix,
        ] {
            for hybrid in [false, true] {
                let fixture = Fixture::start(10, fault);
                let (mut report, pcm) = arm_with_hybrid(&fixture, hybrid, true, hybrid);
                report["fault"] = serde_json::json!(format!("{fault:?}"));
                // Initial-full-response-only faults do not affect baseline's
                // earlier bounded head response. Record that independent reopen
                // honestly; require every hybrid fault to fail before PCM.
                let required_rejection = hybrid
                    || !matches!(
                        fault,
                        Fault::WrongInitialRange | Fault::WrongInitialLength | Fault::InitialHtml
                    );
                report["requiredRejectionBeforePcm"] = serde_json::json!(required_rejection);
                safeguards.push(report.clone());
                hybrid_snapshot(&trials, &safeguards, false);
                if required_rejection {
                    assert_eq!(report["ok"], false, "fault must fail before PCM: {report}");
                    assert!(pcm.is_empty(), "fault produced PCM: {report}");
                }
            }
        }
        hybrid_snapshot(&trials, &safeguards, true);
    }

    #[test]
    #[ignore = "diagnostic-only local HTTP preflight/owned-reader comparison; run explicitly"]
    fn compare_preflight_and_owned_validated_reader_handoff() {
        let mut trials = Vec::new();
        let mut safeguards = Vec::new();
        let mut unguarded = None;
        snapshot(&trials, &safeguards, &unguarded, false);
        for rtt_ms in [0, 40, 100] {
            for trial in 0..3 {
                let order = if trial % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                };
                let mut pcm_reference = None;
                for handoff in order {
                    let fixture = Fixture::start(rtt_ms, Fault::None);
                    let (mut report, pcm) = arm(&fixture, handoff, true);
                    report["simulatedHandshakeRttMs"] = serde_json::json!(rtt_ms);
                    report["trial"] = serde_json::json!(trial);
                    report["reopenedByteZero"] =
                        serde_json::json!(report["initialByteRequests"].as_u64().unwrap() > 1);
                    trials.push(report.clone());
                    snapshot(&trials, &safeguards, &unguarded, false);
                    assert_eq!(report["ok"], true, "{report}");
                    assert!(!pcm.is_empty());
                    if let Some(reference) = pcm_reference.as_ref() {
                        assert_eq!(&pcm, reference, "arms must decode identical PCM");
                    } else {
                        pcm_reference = Some(pcm);
                    }
                    assert!(
                        report["tailRequests"].as_u64().unwrap() >= 1,
                        "tail validation must remain exercised"
                    );
                }
            }
        }
        {
            let fixture = Fixture::start(10, Fault::TruncatedTail);
            let (mut report, pcm) = arm(&fixture, true, false);
            report["fault"] = serde_json::json!("TruncatedTail");
            unguarded = Some(report.clone());
            snapshot(&trials, &safeguards, &unguarded, false);
            assert_eq!(
                report["ok"], true,
                "unguarded control must reproduce repaired response: {report}"
            );
            assert!(!pcm.is_empty());
            assert!(report["validationObservedHttpFailures"].as_u64().unwrap() > 0);
        }
        for fault in [
            Fault::WrongTailRange,
            Fault::TruncatedTail,
            Fault::CorruptAudio,
        ] {
            for handoff in [false, true] {
                let fixture = Fixture::start(10, fault);
                let (mut report, pcm) = arm(&fixture, handoff, true);
                report["fault"] = serde_json::json!(format!("{fault:?}"));
                safeguards.push(report.clone());
                snapshot(&trials, &safeguards, &unguarded, false);
                assert_eq!(
                    report["ok"], false,
                    "invalid fixture must not qualify: {report}"
                );
                assert!(pcm.is_empty(), "invalid fixture must not produce first PCM");
            }
        }
        let report = snapshot(&trials, &safeguards, &unguarded, true);
        eprintln!("{}", serde_json::to_string(&report).unwrap());
    }
}
