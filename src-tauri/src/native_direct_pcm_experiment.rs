//! Test-only finite PCM collector used by the direct YouTube experiment.
//! This module never opens an output device or changes production playback.
use super::*;
use serde::Serialize;
use std::{
    sync::{atomic::AtomicU64, mpsc, Arc},
    thread,
    time::Instant as StdInstant,
};
use tokio::sync::watch;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PcmReport {
    pub(crate) ok: bool,
    pub(crate) failure: Option<&'static str>,
    pub(crate) first_pcm_ms: Option<f64>,
    pub(crate) decoded_frames: u64,
    pub(crate) decoded_audio_ms: f64,
    pub(crate) pcm_rms: Option<f64>,
    pub(crate) wall_ms: f64,
    pub(crate) cleanup_ms: f64,
    pub(crate) decoder: Option<DecoderDiagnostics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decoder_failure_stage: Option<&'static str>,
}

/// Decode a bounded range to 48 kHz stereo PCM and count frames actually
/// delivered by the existing native decoder. Errors are reduced to fixed codes.
pub(crate) async fn collect_until(
    source: AudioSource,
    position: f64,
    seconds: f64,
    deadline: StdInstant,
    cancel: watch::Receiver<bool>,
) -> PcmReport {
    collect_until_with_opus_control(source, position, seconds, deadline, cancel, false, false).await
}

/// Test-only route controls for the paired native and FFmpeg Opus experiment.
/// The production classifier still rejects local HTTP; the loopback switch is
/// consumed only by the existing decoder's explicitly test-gated route.
pub(crate) async fn collect_until_with_opus_control(
    source: AudioSource,
    position: f64,
    seconds: f64,
    deadline: StdInstant,
    mut cancel: watch::Receiver<bool>,
    legacy_probe: bool,
    loopback_direct: bool,
) -> PcmReport {
    let started = StdInstant::now();
    let mut report = PcmReport {
        ok: false,
        failure: None,
        first_pcm_ms: None,
        decoded_frames: 0,
        decoded_audio_ms: 0.0,
        pcm_rms: None,
        wall_ms: 0.0,
        cleanup_ms: 0.0,
        decoder: None,
        decoder_failure_stage: None,
    };
    if !position.is_finite()
        || position < 0.0
        || !seconds.is_finite()
        || seconds <= 0.0
        || !(position + seconds).is_finite()
    {
        report.failure = Some("INVALID_RANGE");
        report.wall_ms = elapsed_ms(started);
        return report;
    }
    if *cancel.borrow() || cancel.has_changed().is_err() {
        report.failure = Some("CANCELLED");
        report.wall_ms = elapsed_ms(started);
        return report;
    }
    if StdInstant::now() >= deadline {
        report.failure = Some("TIMEOUT");
        report.wall_ms = elapsed_ms(started);
        return report;
    }

    let target_frames = (seconds * f64::from(RATE)).ceil();
    if !target_frames.is_finite() || target_frames > u64::MAX as f64 {
        report.failure = Some("INVALID_RANGE");
        report.wall_ms = elapsed_ms(started);
        return report;
    }
    let target_frames = target_frames as u64;
    let cancelled = Arc::new(AtomicBool::new(false));
    let session = Arc::new(DecoderSession::default());
    session
        .force_legacy_opus_probe
        .store(legacy_probe, Ordering::Relaxed);
    session
        .force_loopback_opus_ffmpeg
        .store(loopback_direct, Ordering::Relaxed);
    session.bind(&cancelled);
    session.set_startup_deadline(Some(deadline));
    let diagnostics = Arc::new(std::sync::Mutex::new(None));
    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(4);
    let samples = Arc::new(AtomicU64::new(0));
    let sample_rate = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let channels = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let duration = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    let worker_session = session.clone();
    let worker_cancelled = cancelled.clone();
    let worker_diagnostics = diagnostics.clone();
    let worker = thread::spawn(move || {
        decode_with_session_and_diagnostics(
            source,
            position,
            Some(position + seconds),
            RATE,
            CHANNELS,
            tx,
            samples,
            sample_rate,
            channels,
            duration,
            Some(worker_diagnostics),
            worker_cancelled,
            worker_session,
        )
    });

    let mut worker = Some(worker);
    let mut worker_result: Option<Result<(), String>> = None;
    let mut worker_panicked = false;
    let mut failure = None;
    let mut sum_squares = 0.0f64;
    let mut sample_count = 0u64;
    loop {
        if *cancel.borrow() {
            failure = Some("CANCELLED");
            break;
        }
        if StdInstant::now() >= deadline {
            failure = Some("TIMEOUT");
            break;
        }
        loop {
            if *cancel.borrow() {
                failure = Some("CANCELLED");
                break;
            }
            if StdInstant::now() >= deadline {
                failure = Some("TIMEOUT");
                break;
            }
            let block = match rx.try_recv() {
                Ok(block) => block,
                Err(_) => break,
            };
            if !block.is_empty() {
                if report.first_pcm_ms.is_none() {
                    report.first_pcm_ms = Some(elapsed_ms(started));
                }
                for sample in &block {
                    let sample = f64::from(*sample);
                    sum_squares += sample * sample;
                    sample_count = sample_count.saturating_add(1);
                }
                report.decoded_frames = report
                    .decoded_frames
                    .saturating_add((block.len() / usize::from(CHANNELS)) as u64);
                if report.decoded_frames >= target_frames {
                    break;
                }
            }
        }
        if failure.is_some() {
            break;
        }
        // PCM only qualifies if it arrived before the caller's terminal events.
        if *cancel.borrow() {
            failure = Some("CANCELLED");
            break;
        }
        if StdInstant::now() >= deadline {
            failure = Some("TIMEOUT");
            break;
        }
        if report.decoded_frames >= target_frames {
            report.ok = true;
            break;
        }
        if let Some(result) = worker_result.as_ref() {
            failure = Some(if result.is_ok() {
                "SHORT_PCM"
            } else {
                "DECODER_FAILED"
            });
            break;
        }
        if let Some(handle) = worker.as_ref() {
            if handle.is_finished() {
                match worker.take().unwrap().join() {
                    Ok(result) => worker_result = Some(result),
                    Err(_) => {
                        worker_panicked = true;
                        worker_result = Some(Err(String::new()));
                    }
                }
                // The sender is closed now; iterate once to drain its final queued blocks.
                continue;
            }
        }
        if *cancel.borrow() {
            failure = Some("CANCELLED");
            break;
        }
        if StdInstant::now() >= deadline {
            failure = Some("TIMEOUT");
            break;
        }
        tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() { failure = Some("CANCELLED"); break; }
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                failure = Some("TIMEOUT"); break;
            }
            _ = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
    }

    let cleanup_started = StdInstant::now();
    if worker.is_some() {
        cancelled.store(true, Ordering::Relaxed);
        if let Some(stop) = session
            .cancel_download
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            stop();
        }
    }
    drop(rx);
    if let Some(handle) = worker.take() {
        match handle.join() {
            Ok(result) => worker_result = Some(result),
            Err(_) => {
                worker_panicked = true;
                worker_result = Some(Err(String::new()));
            }
        }
    }
    report.cleanup_ms = elapsed_ms(cleanup_started);
    if !report.ok {
        report.failure = failure.or_else(|| {
            Some(if worker_result.as_ref().is_some_and(|r| r.is_ok()) {
                "SHORT_PCM"
            } else {
                "DECODER_FAILED"
            })
        });
        if report.failure == Some("DECODER_FAILED") {
            report.decoder_failure_stage = Some(if worker_panicked {
                "WORKER_PANIC"
            } else {
                worker_result
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .map(|error| decoder_failure_stage(error))
                    .unwrap_or("UNKNOWN")
            });
        }
    }
    report.decoded_audio_ms = report.decoded_frames as f64 / 48.0;
    report.pcm_rms = (sample_count > 0).then(|| (sum_squares / sample_count as f64).sqrt());
    report.decoder = diagnostics
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    report.wall_ms = elapsed_ms(started);
    report
}

fn decoder_failure_stage(message: &str) -> &'static str {
    let mut message = message;
    for _ in 0..3 {
        let unwrapped = [
            "Stream creation failure:",
            "Failed to fetch:",
            "Failed to get response:",
        ]
        .iter()
        .find_map(|prefix| message.strip_prefix(prefix));
        match unwrapped {
            Some(rest) => message = rest.trim_start(),
            None => break,
        }
    }

    [
        ("could not open audio file", "SOURCE_OPEN"),
        ("Storage creation failure", "NATIVE_STORAGE_SETUP"),
        ("Native HTTP request failed", "NATIVE_HTTP_REQUEST"),
        ("Native HTTP byte range", "NATIVE_HTTP_RANGE"),
        ("Audio reader setup exceeded", "NATIVE_READER_DEADLINE"),
        ("Local playback storage failed", "STORAGE_FAILURE"),
        ("could not start ffmpeg", "FFMPEG_START"),
        ("ffmpeg did not provide an audio pipe", "FFMPEG_PIPE"),
        ("could not read ffmpeg audio", "FFMPEG_READ"),
        ("ffmpeg exited with status", "FFMPEG_EXIT"),
        ("ffmpeg process lock was poisoned", "FFMPEG_PROCESS_STATE"),
    ]
    .iter()
    .find_map(|(prefix, stage)| message.starts_with(prefix).then_some(*stage))
    .unwrap_or("UNKNOWN")
}

fn elapsed_ms(started: StdInstant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, time::SystemTime};

    fn wav_source() -> (AudioSource, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "burrata-direct-pcm-{}-{}.wav",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let frames = 48_000u32 * 8;
        let mut wav = Vec::with_capacity(44 + frames as usize * 4);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + frames * 4).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(frames * 4).to_le_bytes());
        wav.resize(44 + frames as usize * 4, 0);
        fs::write(&path, wav).unwrap();
        (
            AudioSource {
                input: path.to_string_lossy().into_owned(),
                ffmpeg: PathBuf::from("ffmpeg-unavailable-for-wav"),
                headers: Vec::new(),
                decoder_hint: None,
            },
            path,
        )
    }

    #[tokio::test]
    async fn seeks_and_counts_delivered_stereo_frames() {
        let (source, path) = wav_source();
        let (_stop, cancel) = watch::channel(false);
        let report = collect_until(
            source,
            2.0,
            1.0,
            StdInstant::now() + Duration::from_secs(5),
            cancel,
        )
        .await;
        let _ = fs::remove_file(path);
        assert!(report.ok, "{report:?}");
        assert!(report.first_pcm_ms.is_some());
        assert!(report.decoded_frames >= 48_000);
        assert!(report.decoded_frames < 48_000 * 2);
        assert!(report.decoder.is_some());
        assert_eq!(report.decoder_failure_stage, None);
        let (source, path) = wav_source();
        let (_stop, cancel) = watch::channel(false);
        let tail = collect_until(
            source,
            7.5,
            1.0,
            StdInstant::now() + Duration::from_secs(5),
            cancel,
        )
        .await;
        let _ = fs::remove_file(path);
        assert!(!tail.ok);
        assert_eq!(tail.failure, Some("SHORT_PCM"));
        assert_eq!(tail.decoder_failure_stage, None);
        assert!((20_000..=30_000).contains(&tail.decoded_frames));
    }

    #[tokio::test]
    async fn cancelled_or_expired_requests_do_not_start_decode() {
        let (source, path) = wav_source();
        let (_stop, cancel) = watch::channel(true);
        let cancelled = collect_until(
            source.clone(),
            0.0,
            1.0,
            StdInstant::now() + Duration::from_secs(5),
            cancel,
        )
        .await;
        assert_eq!(cancelled.failure, Some("CANCELLED"));
        assert_eq!(cancelled.decoder_failure_stage, None);
        let (_stop, cancel) = watch::channel(false);
        let expired = collect_until(source, 0.0, 1.0, StdInstant::now(), cancel).await;
        let _ = fs::remove_file(path);
        assert_eq!(expired.failure, Some("TIMEOUT"));
        assert_eq!(expired.decoder_failure_stage, None);
    }

    #[test]
    fn decoder_failure_stages_are_fixed_and_only_strip_known_wrappers() {
        let cases = [
            ("could not open audio file: /secret/path", "SOURCE_OPEN"),
            ("Storage creation failure: secret", "NATIVE_STORAGE_SETUP"),
            (
                "Native HTTP request failed: https://secret",
                "NATIVE_HTTP_REQUEST",
            ),
            ("Native HTTP byte range: secret", "NATIVE_HTTP_RANGE"),
            (
                "Audio reader setup exceeded: secret",
                "NATIVE_READER_DEADLINE",
            ),
            ("Local playback storage failed: secret", "STORAGE_FAILURE"),
            ("could not start ffmpeg: secret", "FFMPEG_START"),
            (
                "ffmpeg did not provide an audio pipe: secret",
                "FFMPEG_PIPE",
            ),
            ("could not read ffmpeg audio: secret", "FFMPEG_READ"),
            ("ffmpeg exited with status 1: secret", "FFMPEG_EXIT"),
            (
                "ffmpeg process lock was poisoned: secret",
                "FFMPEG_PROCESS_STATE",
            ),
        ];
        for (message, expected) in cases {
            assert_eq!(decoder_failure_stage(message), expected);
        }

        assert_eq!(
            decoder_failure_stage(
                "Stream creation failure: Failed to fetch: Failed to get response: could not start ffmpeg: secret"
            ),
            "FFMPEG_START"
        );
        assert_eq!(
            decoder_failure_stage(
                "Stream creation failure: Failed to fetch: Failed to get response: Stream creation failure: could not start ffmpeg"
            ),
            "UNKNOWN"
        );
        assert_eq!(
            decoder_failure_stage("stderr: could not start ffmpeg"),
            "UNKNOWN"
        );
        assert_eq!(
            decoder_failure_stage("secret URL https://private.invalid"),
            "UNKNOWN"
        );
    }

    #[test]
    fn reports_serialize_only_safe_failure_stage_codes() {
        let report = PcmReport {
            ok: false,
            failure: Some("DECODER_FAILED"),
            first_pcm_ms: None,
            decoded_frames: 0,
            decoded_audio_ms: 0.0,
            pcm_rms: None,
            wall_ms: 0.0,
            cleanup_ms: 0.0,
            decoder: None,
            decoder_failure_stage: Some("NATIVE_HTTP_REQUEST"),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("NATIVE_HTTP_REQUEST"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private.invalid"));
        assert!(!json.contains("\"decoderFailureStage\":null"));
    }
}
