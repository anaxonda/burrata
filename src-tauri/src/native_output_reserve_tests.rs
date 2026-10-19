// Included only inside native_output::tests; shares the real production helpers.
fn reserve_fixture_source(
    rx: std_mpsc::Receiver<Vec<f32>>,
    failed: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    target: usize,
) -> (
    QueuedSamples,
    tokio::sync::mpsc::UnboundedReceiver<OutputEvent>,
) {
    let (events, received) = tokio::sync::mpsc::unbounded_channel();
    let (activation_tx, _) = std_mpsc::channel();
    let source = AudioSource {
        input: "https://fixture.invalid/audio".into(),
        ffmpeg: "unused".into(),
        headers: vec![],
        decoder_hint: None,
    };
    (
        QueuedSamples {
            reserve: Some(StartupPcmReserve::new(target)),
            reserve_diagnostics: None,
            decoder_session: Arc::new(DecoderSession::default()),
            rx,
            pending: VecDeque::new(),
            channels: 2,
            sample_rate: 44_100,
            samples_played: Arc::new(AtomicU64::new(0)),
            generation: 1,
            active_generation: Arc::new(AtomicU64::new(1)),
            base_position: Arc::new(AtomicU64::new(0f64.to_bits())),
            initial_position: 0.,
            started: false,
            activated: false,
            cancelled,
            tap_writer: crate::pcm_tap::PcmTap::new().writer(1, 44_100, 2),
            events,
            duration_bits: Arc::new(AtomicU64::new(8f64.to_bits())),
            decoder_diagnostics: Arc::new(std::sync::Mutex::new(None)),
            source_rate: Arc::new(std::sync::atomic::AtomicU32::new(44_100)),
            source_channels: Arc::new(std::sync::atomic::AtomicU32::new(2)),
            decode_failed: failed,
            source,
            end: None,
            activation_tx,
            buffering: Arc::new(AtomicBool::new(false)),
            exhausted: false,
        },
        received,
    )
}

#[test]
fn experimental_reserve_admission_is_format_derived_bounded_and_explicit() {
    let source = AudioSource {
        input: "https://fixture.invalid/audio".into(),
        ffmpeg: "unused".into(),
        headers: vec![],
        decoder_hint: Some(AudioSourceDecoderHint {
            mime_type: Some("audio/flac".into()),
            codec: Some("flac".into()),
            duration_seconds: Some(180.),
            content_length: None,
            has_video: false,
        }),
    };
    let policy = StartupReservePolicy::DecodedTwoSeconds;
    assert_eq!(
        startup_reserve_target(&source, 0., None, false, 44_100, 2, policy),
        Some(176_400)
    );
    assert_eq!(
        startup_reserve_target(&source, 0., None, false, 96_000, 2, policy),
        Some(384_000)
    );
    assert_eq!(
        startup_reserve_target(&source, 0., None, false, 192_000, 2, policy),
        None
    );
    assert_eq!(
        startup_reserve_target(
            &source,
            0.,
            None,
            false,
            44_100,
            2,
            StartupReservePolicy::Disabled
        ),
        None
    );
    for (position, end, paused) in [(1., None, false), (0., Some(5.), false), (0., None, true)] {
        assert_eq!(
            startup_reserve_target(&source, position, end, paused, 44_100, 2, policy),
            None
        );
    }
    let mut local = source.clone();
    local.input = "/fixture.wav".into();
    assert_eq!(
        startup_reserve_target(&local, 0., None, false, 44_100, 2, policy),
        None
    );
    for duration in [None, Some(f64::NAN), Some(0.)] {
        let mut unknown = source.clone();
        unknown.decoder_hint.as_mut().unwrap().duration_seconds = duration;
        assert_eq!(
            startup_reserve_target(&unknown, 0., None, false, 44_100, 2, policy),
            None
        );
    }
}

#[test]
fn reserve_clean_short_eof_starts_real_pcm_but_failure_cannot_masquerade_as_short_eof() {
    let failed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std_mpsc::sync_channel(16);
    tx.send(vec![0.25; 128]).unwrap();
    drop(tx);
    let (mut short, mut events) =
        reserve_fixture_source(rx, failed, Arc::new(AtomicBool::new(false)), 8192);
    assert_eq!(short.samples_played.load(Ordering::Relaxed), 0);
    assert_eq!(short.next(), Some(0.25));
    assert_eq!(short.samples_played.load(Ordering::Relaxed), 1);
    assert!(short.reserve_diagnostics.as_ref().unwrap().short_clean_eof);
    assert!(matches!(events.try_recv(), Ok(OutputEvent::Started { .. })));
    assert_eq!(short.count(), 127);

    let failed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std_mpsc::sync_channel(16);
    let completion = ReserveProducerCompletion {
        _sender: tx.clone(),
        failed: failed.clone(),
        cancelled: Arc::new(AtomicBool::new(false)),
        events: tokio::sync::mpsc::unbounded_channel().0,
        generation: 1,
    };
    tx.send(vec![0.25; 128]).unwrap();
    drop(tx);
    let (mut source, mut events) =
        reserve_fixture_source(rx, failed.clone(), Arc::new(AtomicBool::new(false)), 8192);
    assert_eq!(source.next(), Some(0.)); // sender guard keeps partial data pending, not cleanEOF.
    assert_eq!(source.samples_played.load(Ordering::Relaxed), 0);
    assert!(events.try_recv().is_err());
    failed.store(true, Ordering::Release);
    drop(completion);
    assert_eq!(source.next(), None);
    assert_eq!(source.samples_played.load(Ordering::Relaxed), 0);
    assert!(events.try_recv().is_err());
}

#[test]
fn reserve_wait_retains_original_startup_deadline_and_empty_eof_never_starts() {
    let (tx, rx) = std_mpsc::sync_channel(16);
    let (mut source, mut events) = reserve_fixture_source(
        rx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        8192,
    );
    let deadline = Instant::now() + crate::playback_sources::PLAYBACK_RESOLUTION_DEADLINE;
    source.decoder_session.set_startup_deadline(Some(deadline));
    for _ in 0..32 {
        assert_eq!(source.next(), Some(0.));
    }
    assert_eq!(source.decoder_session.startup_deadline(), Some(deadline));
    assert_eq!(source.samples_played.load(Ordering::Relaxed), 0);
    assert!(events.try_recv().is_err());
    drop(tx);
    assert_eq!(source.next(), None);
    assert!(!source.started);
    assert!(events.try_recv().is_err());
}

#[test]
fn reserve_cancel_before_ready_or_ready_source_replacement_never_starts_old_pcm() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std_mpsc::sync_channel(16);
    tx.send(vec![0.25; 4096]).unwrap();
    let (mut source, mut events) = reserve_fixture_source(
        rx,
        Arc::new(AtomicBool::new(false)),
        cancelled.clone(),
        8192,
    );
    assert_eq!(source.next(), Some(0.));
    assert!(!source.started);
    tx.send(vec![0.25; 4096]).unwrap();
    cancelled.store(true, Ordering::Release);
    assert_eq!(source.next(), None);
    assert_eq!(source.samples_played.load(Ordering::Relaxed), 0);
    assert!(events.try_recv().is_err());
    let (other_tx, other_rx) = std_mpsc::sync_channel(16);
    other_tx.send(vec![0.5; 4096]).unwrap();
    other_tx.send(vec![0.5; 4096]).unwrap();
    let (mut other, mut other_events) = reserve_fixture_source(
        other_rx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        8192,
    );
    assert_eq!(other.next(), Some(0.5));
    assert_eq!(other.samples_played.load(Ordering::Relaxed), 1);
    assert!(!other.reserve_diagnostics.as_ref().unwrap().short_clean_eof);
    assert!(matches!(
        other_events.try_recv(),
        Ok(OutputEvent::Started { .. })
    ));
}

#[test]
#[ignore = "paced independent-delivery reserve AB; run alone on idle host with external timeout"]
fn decoded_startup_reserve_independent_timeline_ab_bridges_bounded_body_gap() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let wav = Arc::new(pcm_wav(44_100, 2, 8));
    for policy in [
        StartupReservePolicy::Disabled,
        StartupReservePolicy::DecodedTwoSeconds,
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = wav.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let server_stopped = stopped.clone();
        let (prefix_tx, prefix_rx) = std_mpsc::channel();
        let server = thread::spawn(move || {
            let mut socket = accept_loopback(&listener, Duration::from_secs(4)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            socket.write_all(&body[..65536]).unwrap();
            socket.flush().unwrap();
            let origin = Instant::now();
            prefix_tx.send(origin).unwrap();
            // Identical sender timeline in both arms, independent of first audio/reserve.
            // ~1x PCM delivery, with fixed1750ms gap beginning at2s.
            for (index, chunk) in body[65536..].chunks(1764).enumerate() {
                let slot = index as u64 + 1;
                let due =
                    origin + Duration::from_millis(slot * 10 + if slot >= 200 { 1750 } else { 0 });
                while Instant::now() < due {
                    if server_stopped.load(Ordering::Acquire) {
                        return;
                    }
                    thread::park_timeout(
                        due.saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(10)),
                    );
                }
                if server_stopped.load(Ordering::Acquire) || socket.write_all(chunk).is_err() {
                    return;
                }
                if socket.flush().is_err() {
                    return;
                }
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
        let playback = start_playback_with_reserve_policy(
            &mixer,
            0.,
            91,
            AudioSource {
                input: format!("http://{address}/fixture.wav"),
                ffmpeg: "unused-wav".into(),
                headers: vec![],
                decoder_hint: Some(AudioSourceDecoderHint {
                    mime_type: Some("audio/wav".into()),
                    codec: Some("pcm".into()),
                    duration_seconds: Some(8.),
                    content_length: Some(wav.len() as u64),
                    has_video: false,
                }),
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
            policy,
        );
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let origin = prefix_rx.recv_timeout(Duration::from_secs(4)).unwrap();
            assert!(
                origin.elapsed() < Duration::from_millis(100),
                "AB fixture startup scheduling gap"
            );
            let mut first_real = None;
            let mut initial_silence = 0;
            let mut first_samples = 0;
            let mut last_tick = Instant::now();
            for tick in 1..1400u32 {
                let due = origin + Duration::from_millis(u64::from(tick) * 10);
                while Instant::now() < due {
                    thread::park_timeout(due.saturating_duration_since(Instant::now()));
                }
                let now = Instant::now();
                assert!(
                    now.duration_since(last_tick) < Duration::from_millis(100),
                    "AB fixture scheduler gap"
                );
                last_tick = now;
                for _ in 0..882 {
                    assert!(output.next().is_some());
                }
                let consumed = playback.samples.load(Ordering::Relaxed);
                if consumed > 0 && first_real.is_none() {
                    first_real = Some(now);
                    first_samples = consumed;
                    initial_silence = session
                        .stream_stats
                        .synthetic_silence_samples
                        .load(Ordering::Relaxed);
                }
                assert!(!playback.decode_failed.load(Ordering::Relaxed));
                if let Some(first) = first_real {
                    if now.duration_since(first) >= Duration::from_secs(5)
                        && consumed.saturating_sub(first_samples) >= 44_100 * 2 * 5
                    {
                        break;
                    }
                }
            }
            let first = first_real.expect("no actual first audio");
            let consumed = playback.samples.load(Ordering::Relaxed);
            assert!(first.elapsed() >= Duration::from_secs(5));
            assert!(consumed.saturating_sub(first_samples) >= 44_100 * 2 * 5);
            let after_start_zeros = session
                .stream_stats
                .synthetic_silence_samples
                .load(Ordering::Relaxed)
                .saturating_sub(initial_silence);
            let reserve_enabled = policy == StartupReservePolicy::DecodedTwoSeconds;
            if reserve_enabled {
                assert_eq!(
                    after_start_zeros, 0,
                    "reserve failed to bridge fixed bounded input gap"
                );
            } else {
                assert!(
                    after_start_zeros > 0,
                    "same timeline must reproduce baseline starvation"
                );
            }
            let mut observed_reserve = false;
            while let Ok(event) = received.try_recv() {
                match event {
                    OutputEvent::Started {
                        decoder: Some(decoder),
                        ..
                    } => {
                        if let Some(facts) = decoder.startup_reserve {
                            assert!(reserve_enabled);
                            assert_eq!(facts.target_samples, 176400);
                            assert!(facts.released_samples >= facts.target_samples);
                            assert!(!facts.short_clean_eof);
                            observed_reserve = true;
                        }
                    }
                    OutputEvent::Failed { .. } | OutputEvent::LocalStorageFailed { .. } => {
                        panic!("AB source failed")
                    }
                    _ => {}
                }
            }
            assert_eq!(observed_reserve, reserve_enabled);
            eprintln!("reserve_ab enabled={reserve_enabled} firstRealMs={} consumed={consumed} postStartZeros={after_start_zeros}", first.duration_since(origin).as_millis());
        }));
        playback.cancel.store(true, Ordering::Release);
        playback.player.stop();
        drop(output);
        stopped.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !session.decode_thread_finished.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::park_timeout(Duration::from_millis(1));
        }
        let finished = session.decode_thread_finished.load(Ordering::Acquire);
        if finished {
            session.decoder.lock().unwrap().take();
        }
        server.join().unwrap();
        assert!(finished, "AB decode cleanup did not finish");
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}

#[test]
fn reserve_panicking_producer_publishes_terminal_failure_unless_cancelled() {
    for cancelled in [false, true] {
        let (tx, rx) = std_mpsc::sync_channel(16);
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let worker = thread::spawn(move || {
            let _completion = ReserveProducerCompletion {
                _sender: tx,
                failed: worker_failed,
                cancelled: Arc::new(AtomicBool::new(cancelled)),
                events,
                generation: 1,
            };
            panic!("injected fixture producer panic");
        });
        assert!(worker.join().is_err());
        assert!(failed.load(Ordering::Acquire));
        assert!(matches!(
            rx.try_recv(),
            Err(std_mpsc::TryRecvError::Disconnected)
        ));
        if cancelled {
            assert!(received.try_recv().is_err());
        } else {
            assert!(matches!(
                received.try_recv(),
                Ok(OutputEvent::Failed { generation: 1, .. })
            ));
        }
    }
}

// Actual HTTP -> observed temporary storage -> Rodio decoder -> software mixer.
// Each case owns its server, decoder, mixer and cancellation; no audio device or
// environment mutation. These complement the small state-machine unit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReserveWorkerCase {
    ShortCleanEof,
    DecodeError,
    CancelPartialReserve,
    OriginalReaderDeadline,
}

fn run_actual_reserve_worker_case(case: ReserveWorkerCase) {
    use std::io::Read;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let wav = pcm_wav(44_100, 2, 1);
    let body = if case == ReserveWorkerCase::DecodeError {
        b"this is not an audio container".to_vec()
    } else {
        wav
    };
    let body_len = body.len();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let server_stopped = stopped.clone();
    let (sent_tx, sent_rx) = std_mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let Ok(mut socket) = accept_loopback(&listener, Duration::from_secs(3)) else {
            return;
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request);
        if case != ReserveWorkerCase::OriginalReaderDeadline {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if socket.write_all(header.as_bytes()).is_err() {
                return;
            }
            let bytes = if case == ReserveWorkerCase::CancelPartialReserve {
                &body[..65_536]
            } else {
                &body[..]
            };
            if socket.write_all(bytes).is_err() || socket.flush().is_err() {
                return;
            }
        }
        let _ = sent_tx.send(());
        if matches!(
            case,
            ReserveWorkerCase::CancelPartialReserve | ReserveWorkerCase::OriginalReaderDeadline
        ) {
            while !server_stopped.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_millis(1));
            }
        }
    });
    let (mixer, mut output) = rodio::mixer::mixer(
        NonZeroU16::new(2).unwrap(),
        NonZeroU32::new(44_100).unwrap(),
    );
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let session = Arc::new(DecoderSession::default());
    let origin = Instant::now();
    let original_deadline = origin
        + if case == ReserveWorkerCase::OriginalReaderDeadline {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(3)
        };
    session.set_startup_deadline(Some(original_deadline));
    // Guaranteed absent executable: malformed input must fail via the actual
    // fallback branch, without relying on a host-installed FFmpeg binary.
    let directory = tempfile::tempdir().unwrap();
    let playback = start_playback_with_reserve_policy(
        &mixer,
        0.,
        193,
        AudioSource {
            input: format!("http://{address}/fixture.wav"),
            ffmpeg: directory.path().join("absent-ffmpeg"),
            headers: vec![],
            decoder_hint: Some(AudioSourceDecoderHint {
                mime_type: Some("audio/wav".into()),
                codec: Some("pcm".into()),
                duration_seconds: Some(1.),
                content_length: Some(body_len as u64),
                has_video: false,
            }),
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
        StartupReservePolicy::DecodedTwoSeconds,
    );
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if case == ReserveWorkerCase::CancelPartialReserve {
            sent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let mut started = Vec::new();
        let mut failures = 0usize;
        let observation_deadline = origin
            + if case == ReserveWorkerCase::OriginalReaderDeadline {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(3)
            };
        loop {
            assert!(
                Instant::now() < observation_deadline,
                "actual reserve worker stalled: {case:?}"
            );
            // Mixer framing is not one iterator call per consumed PCM sample.
            for _ in 0..4096 {
                let _ = output.next();
            }
            while let Ok(event) = received.try_recv() {
                match event {
                    OutputEvent::Started { decoder, .. } => started.push(decoder),
                    OutputEvent::Failed { .. } | OutputEvent::LocalStorageFailed { .. } => {
                        failures += 1
                    }
                    _ => {}
                }
            }
            let consumed = playback.samples.load(Ordering::Acquire);
            match case {
                ReserveWorkerCase::ShortCleanEof if consumed >= 88_200 => {
                    assert_eq!(consumed, 88_200);
                    assert_eq!(failures, 0);
                    assert_eq!(started.len(), 1);
                    let facts = started[0]
                        .as_ref()
                        .unwrap()
                        .startup_reserve
                        .as_ref()
                        .unwrap();
                    assert_eq!(facts.target_samples, 176_400);
                    assert_eq!(facts.released_samples, 88_200);
                    assert!(facts.short_clean_eof);
                    break;
                }
                ReserveWorkerCase::DecodeError | ReserveWorkerCase::OriginalReaderDeadline
                    if failures > 0 =>
                {
                    assert_eq!(failures, 1);
                    assert!(started.is_empty());
                    assert_eq!(consumed, 0);
                    assert!(playback.decode_failed.load(Ordering::Acquire));
                    if case == ReserveWorkerCase::OriginalReaderDeadline {
                        assert_eq!(session.startup_deadline(), Some(original_deadline));
                        assert!(
                            origin.elapsed() < Duration::from_secs(1),
                            "reader reset the shared startup budget"
                        );
                    }
                    break;
                }
                ReserveWorkerCase::CancelPartialReserve
                    if session
                        .stream_stats
                        .pcm_produced_samples
                        .load(Ordering::Acquire)
                        > 0 =>
                {
                    assert!(started.is_empty());
                    assert_eq!(consumed, 0);
                    assert_eq!(failures, 0);
                    // Real producer has emitted partial PCM, but below the
                    // reserve. Cancel before any real sample can be started.
                    playback.cancel.store(true, Ordering::Release);
                    break;
                }
                _ => {}
            }
            thread::park_timeout(Duration::from_millis(1));
        }
    }));
    // Always release both ends, including an assertion panic inside observation.
    playback.cancel.store(true, Ordering::Release);
    playback.player.stop();
    drop(output);
    stopped.store(true, Ordering::Release);
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while !session.decode_thread_finished.load(Ordering::Acquire)
        && Instant::now() < cleanup_deadline
    {
        thread::park_timeout(Duration::from_millis(1));
    }
    let finished = session.decode_thread_finished.load(Ordering::Acquire);
    if finished {
        session.decoder.lock().unwrap().take();
    }
    let server_result = server.join();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    assert!(finished, "actual reserve decoder did not stop: {case:?}");
    server_result.unwrap();
    if case == ReserveWorkerCase::CancelPartialReserve {
        assert_eq!(playback.samples.load(Ordering::Acquire), 0);
        assert!(!playback.decode_failed.load(Ordering::Acquire));
        while let Ok(event) = received.try_recv() {
            assert!(!matches!(
                event,
                OutputEvent::Started { .. }
                    | OutputEvent::Failed { .. }
                    | OutputEvent::LocalStorageFailed { .. }
            ));
        }
    }
}

#[test]
fn actual_reserve_worker_releases_short_clean_eof_with_actual_sample_count() {
    run_actual_reserve_worker_case(ReserveWorkerCase::ShortCleanEof);
}
#[test]
fn actual_reserve_worker_decode_error_never_releases_partial_start() {
    run_actual_reserve_worker_case(ReserveWorkerCase::DecodeError);
}
#[test]
fn actual_reserve_worker_cancellation_during_partial_fill_never_starts_or_fails() {
    run_actual_reserve_worker_case(ReserveWorkerCase::CancelPartialReserve);
}
#[test]
fn actual_reserve_worker_reader_honors_original_100ms_deadline() {
    run_actual_reserve_worker_case(ReserveWorkerCase::OriginalReaderDeadline);
}
