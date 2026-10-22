//! Hardware-free regressions for queued occurrence ownership and retained errors.
use super::*;

struct PreloadFixture {
    queue: QueuedSamples,
    producer: Option<std_mpsc::SyncSender<Vec<f32>>>,
    activations: std_mpsc::Receiver<Activation>,
    events: tokio::sync::mpsc::UnboundedReceiver<OutputEvent>,
}

fn preload(generation: u64, active: Arc<AtomicU64>) -> PreloadFixture {
    let (producer, rx) = std_mpsc::sync_channel(4);
    let (events, received) = tokio::sync::mpsc::unbounded_channel();
    let (activation_tx, activations) = std_mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let session = Arc::new(DecoderSession::default());
    session.bind(&cancelled);
    PreloadFixture {
        queue: QueuedSamples {
            reserve: None,
            reserve_diagnostics: None,
            decoder_session: session,
            rx,
            pending: VecDeque::new(),
            channels: 1,
            sample_rate: 48_000,
            samples_played: Arc::new(AtomicU64::new(19)),
            generation,
            active_generation: active,
            base_position: Arc::new(AtomicU64::new(99f64.to_bits())),
            initial_position: 3.5,
            started: false,
            activated: false,
            cancelled,
            tap_writer: crate::pcm_tap::PcmTap::new().writer(generation, 48_000, 1),
            events,
            duration_bits: Arc::new(AtomicU64::new(60f64.to_bits())),
            decoder_diagnostics: Arc::new(std::sync::Mutex::new(None)),
            source_rate: Arc::new(std::sync::atomic::AtomicU32::new(48_000)),
            source_channels: Arc::new(std::sync::atomic::AtomicU32::new(1)),
            decode_failed: Arc::new(AtomicBool::new(false)),
            source: AudioSource {
                input: "same-recording-for-each-occurrence".into(),
                ffmpeg: "unused".into(),
                headers: vec![],
                decoder_hint: None,

                cenc: None,
            },
            end: None,
            activation_tx,
            buffering: Arc::new(AtomicBool::new(false)),
            exhausted: false,
        },
        producer: Some(producer),
        activations,
        events: received,
    }
}

impl PreloadFixture {
    fn fail(&mut self, message: &str) {
        *self.queue.decoder_session.retained_failure.lock().unwrap() = Some(message.into());
        self.queue.decode_failed.store(true, Ordering::Release);
        drop(self.producer.take());
    }

    fn take_failure(&self, activation: &Activation) -> Option<OutputEvent> {
        take_active_decode_failure(
            activation.generation,
            &activation.decoder_session,
            &activation.cancel,
        )
    }
}

#[test]
fn partial_pcm_failure_stays_with_preload_until_selection_and_emits_once() {
    let active = Arc::new(AtomicU64::new(11));
    let healthy = DecoderSession::default();
    let mut next = preload(12, active.clone());
    next.producer
        .as_ref()
        .unwrap()
        .send(vec![0.25, 0.5])
        .unwrap();
    next.fail("preloaded stream disconnected after a decoded block");
    assert_eq!(active.load(Ordering::Acquire), 11);
    assert!(take_active_decode_failure(11, &healthy, &AtomicBool::new(false)).is_none());
    assert!(next.events.try_recv().is_err());
    assert!(next.activations.try_recv().is_err());
    assert!(next
        .queue
        .decoder_session
        .retained_failure
        .lock()
        .unwrap()
        .is_some());

    assert_eq!(next.queue.next(), Some(0.25));
    let selected = next.activations.try_recv().unwrap();
    assert_eq!(selected.generation, 12);
    assert!(Arc::ptr_eq(
        &selected.decoder_session,
        &next.queue.decoder_session
    ));
    assert!(
        matches!(next.take_failure(&selected), Some(OutputEvent::Failed {
        generation: 12, message,
    }) if message == "preloaded stream disconnected after a decoded block")
    );
    assert!(
        next.take_failure(&selected).is_none(),
        "retained failure is consumed once"
    );
    assert!(matches!(
        next.events.try_recv(),
        Ok(OutputEvent::Started { generation: 12, .. })
    ));
    assert_eq!(next.queue.next(), Some(0.5));
    assert_eq!(next.queue.next(), None);
    assert_eq!(next.queue.next(), None);
    assert!(
        selected.decode_failed.load(Ordering::Acquire),
        "EOF must keep the worker's Ended guard set"
    );
    assert!(
        next.activations.try_recv().is_err(),
        "first PCM must not adopt the selected occurrence twice"
    );
    assert!(next.events.try_recv().is_err());
}

#[test]
fn zero_pcm_nonstorage_failure_adopts_ownership_without_started_or_first_audio() {
    let active = Arc::new(AtomicU64::new(20));
    let mut next = preload(21, active.clone());
    next.fail("decoder rejected the next stream before PCM");
    assert_eq!(next.queue.next(), None);
    let selected = next.activations.try_recv().unwrap();
    assert_eq!(active.load(Ordering::Acquire), 21);
    assert_eq!(next.queue.samples_played.load(Ordering::Acquire), 0);
    assert_eq!(
        f64::from_bits(next.queue.base_position.load(Ordering::Acquire)),
        3.5
    );
    assert!(!next.queue.started);
    assert!(
        next.events.try_recv().is_err(),
        "failed zero-PCM source is never Started"
    );
    assert!(matches!(
        next.take_failure(&selected),
        Some(OutputEvent::Failed { generation: 21, .. })
    ));
    assert!(next.take_failure(&selected).is_none());
    assert!(
        selected.decode_failed.load(Ordering::Acquire),
        "failure suppresses clean Ended"
    );
    assert_eq!(next.queue.next(), None);
    assert!(next.activations.try_recv().is_err());
}

#[test]
fn cancelled_failed_preload_neither_adopts_nor_publishes_its_retained_error() {
    let active = Arc::new(AtomicU64::new(30));
    let mut discarded = preload(31, active.clone());
    discarded
        .producer
        .as_ref()
        .unwrap()
        .send(vec![0.8])
        .unwrap();
    discarded.fail("obsolete occurrence failed");
    discarded.queue.cancelled.store(true, Ordering::Release);
    assert_eq!(discarded.queue.next(), None);
    assert_eq!(active.load(Ordering::Acquire), 30);
    assert!(!discarded.queue.activated);
    assert!(!discarded.queue.started);
    assert!(discarded.activations.try_recv().is_err());
    assert!(discarded.events.try_recv().is_err());
    assert!(take_active_decode_failure(
        31,
        &discarded.queue.decoder_session,
        &discarded.queue.cancelled
    )
    .is_none());
}

#[test]
fn pending_zero_pcm_preload_owns_buffering_but_started_waits_for_real_pcm() {
    let active = Arc::new(AtomicU64::new(40));
    let mut next = preload(41, active.clone());
    assert_eq!(next.queue.next(), Some(0.0));
    let selected = next.activations.try_recv().unwrap();
    assert_eq!(selected.generation, 41);
    assert_eq!(active.load(Ordering::Acquire), 41);
    assert!(next.queue.activated);
    assert!(!next.queue.started);
    assert!(next.queue.buffering.load(Ordering::Acquire));
    assert_eq!(next.queue.samples_played.load(Ordering::Acquire), 0);
    assert_eq!(
        next.queue
            .decoder_session
            .stream_stats
            .synthetic_silence_samples
            .load(Ordering::Acquire),
        1
    );
    assert!(next.events.try_recv().is_err());
    assert_eq!(next.queue.next(), Some(0.0));
    assert!(next.activations.try_recv().is_err());
    next.producer
        .as_ref()
        .unwrap()
        .send(vec![0.375, 0.625])
        .unwrap();
    assert_eq!(next.queue.next(), Some(0.375));
    assert!(matches!(
        next.events.try_recv(),
        Ok(OutputEvent::Started { generation: 41, .. })
    ));
    assert_eq!(next.queue.samples_played.load(Ordering::Acquire), 1);
    assert!(!next.queue.buffering.load(Ordering::Acquire));
    assert!(
        next.activations.try_recv().is_err(),
        "buffering-to-PCM transition cannot re-adopt the occurrence"
    );
    assert_eq!(next.queue.next(), Some(0.625));
    assert!(
        next.events.try_recv().is_err(),
        "first PCM reports Started once"
    );
}

#[test]
fn repeated_media_occurrences_do_not_share_cancel_failure_or_generation() {
    let active = Arc::new(AtomicU64::new(50));
    let mut stale = preload(51, active.clone());
    let mut selected = preload(52, active.clone());
    let mut later = preload(53, active.clone());
    assert_eq!(stale.queue.source.input, selected.queue.source.input);
    stale.fail("first occurrence failed");
    stale.queue.cancelled.store(true, Ordering::Release);
    selected.producer.as_ref().unwrap().send(vec![0.2]).unwrap();
    later.fail("later occurrence failed");
    assert_eq!(stale.queue.next(), None);
    assert_eq!(selected.queue.next(), Some(0.2));
    let selected_activation = selected.activations.try_recv().unwrap();
    assert_eq!(selected_activation.generation, 52);
    assert!(selected.take_failure(&selected_activation).is_none());
    assert!(!selected_activation.decode_failed.load(Ordering::Acquire));
    assert!(matches!(
        selected.events.try_recv(),
        Ok(OutputEvent::Started { generation: 52, .. })
    ));
    assert!(later.activations.try_recv().is_err());
    assert!(later.events.try_recv().is_err());
    assert_eq!(active.load(Ordering::Acquire), 52);
    assert_eq!(later.queue.next(), None);
    let later_activation = later.activations.try_recv().unwrap();
    assert!(matches!(
        later.take_failure(&later_activation),
        Some(OutputEvent::Failed { generation: 53, .. })
    ));
    assert_eq!(active.load(Ordering::Acquire), 53);
    assert!(stale.events.try_recv().is_err());
}

#[test]
fn real_preload_producer_publishes_failure_before_disconnecting_its_pcm_channel() {
    // Exercise append_source's decoder thread and sender-lifetime guard, without
    // an output device or HTTP server. A missing local file fails before any PCM.
    let directory = tempfile::tempdir().unwrap();
    let (mixer, mut output) = rodio::mixer::mixer(
        NonZeroU16::new(1).unwrap(),
        NonZeroU32::new(48_000).unwrap(),
    );
    let player = Player::connect_new(&mixer);
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (activation_tx, activation_rx) = std_mpsc::channel();
    let active = Arc::new(AtomicU64::new(60));
    let failed = Arc::new(AtomicBool::new(false));
    let cancel = append_source(
        &player,
        61,
        AudioSource {
            input: directory.path().join("absent.wav").to_string_lossy().into(),
            ffmpeg: "unused".into(),
            headers: vec![],
            decoder_hint: None,

            cenc: None,
        },
        0.,
        None,
        active.clone(),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0f64.to_bits())),
        48_000,
        1,
        events,
        crate::pcm_tap::PcmTap::new(),
        activation_tx,
        failed.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !player.empty() {
        assert!(Instant::now() < deadline, "preload decoder did not finish");
        output.next();
        thread::yield_now();
    }
    let selected = activation_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(selected.generation, 61);
    assert_eq!(active.load(Ordering::Acquire), 61);
    assert!(
        failed.load(Ordering::Acquire),
        "channel disconnect must not precede failure publication"
    );
    assert!(selected.decode_failed.load(Ordering::Acquire));
    assert!(matches!(
        take_active_decode_failure(61, &selected.decoder_session, &cancel),
        Some(OutputEvent::Failed { generation: 61, .. })
    ));
    assert!(take_active_decode_failure(61, &selected.decoder_session, &cancel).is_none());
    assert!(
        received.try_recv().is_err(),
        "zero PCM never reports Started; preload failure waits for worker adoption"
    );
    assert!(activation_rx.try_recv().is_err());
}
