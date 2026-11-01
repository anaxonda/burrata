// Included inside native_output.rs::tests; exercises Rodio's real sequential
// queue, not NativeCrossfadeSource's private transition slot.
#[test]
fn plain_rodio_append_reports_old_clean_eof_before_incoming_started_once() {
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let next_generation = Arc::new(AtomicU64::new(2));
    let active_generation = Arc::new(AtomicU64::new(1));
    let mut outgoing = queued_gain_fixture(2, 2, 0.0);
    outgoing.events = events.clone();
    outgoing.active_generation = active_generation.clone();
    outgoing.queued_successor_generation = next_generation.clone();
    outgoing.pending = vec![0.2, 0.3, 0.2, 0.3, 0.2, 0.3, 0.2, 0.3, 0.0, 0.0, 0.0, 0.0].into();
    // An advertised duration must not substitute for consumed PCM duration.
    outgoing
        .duration_bits
        .store(400.0f64.to_bits(), Ordering::Release);
    outgoing
        .decoder_session
        .decode_thread_finished
        .store(true, Ordering::Release);
    let outgoing = NativeCrossfadeSource::new(
        outgoing,
        Arc::new(std::sync::Mutex::new(None)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let mut incoming = queued_gain_fixture(2, 2, 0.0);
    incoming.generation = 2;
    incoming.events = events;
    incoming.active_generation = active_generation;
    incoming.queued_successor_generation = next_generation;
    incoming.pending = vec![0.4, 0.5].into();
    incoming
        .decoder_session
        .decode_thread_finished
        .store(true, Ordering::Release);
    let (queue, mut output) = rodio::queue::queue(false);
    queue.append(outgoing);
    // Production crossfade=None installs the queued reader as a plain append.
    queue.append(incoming);
    let pcm = output.by_ref().take(100).collect::<Vec<_>>();
    assert_eq!(
        pcm,
        [0.2, 0.3, 0.2, 0.3, 0.2, 0.3, 0.2, 0.3, 0.0, 0.0, 0.0, 0.0, 0.4, 0.5]
    );
    let events = std::iter::from_fn(|| received.try_recv().ok()).collect::<Vec<_>>();
    assert!(matches!(events.as_slice(), [
        OutputEvent::Started { generation: 1, .. },
        OutputEvent::GaplessCompleted { from_generation: 1, to_generation: 2, position },
        OutputEvent::Started { generation: 2, .. }
    ] if *position == 3.0));
    assert_eq!(output.next(), None);
    assert!(
        received.try_recv().is_err(),
        "final occurrence and repeated pulls cannot duplicate EOF"
    );
}

#[test]
fn plain_rodio_append_never_calls_failure_unfinished_or_cancelled_output_clean() {
    for outcome in ["failed", "unfinished", "cancelled"] {
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut outgoing = queued_gain_fixture(2, 2, 0.0);
        outgoing.events = events;
        outgoing
            .queued_successor_generation
            .store(2, Ordering::Release);
        outgoing.pending = vec![0.25, 0.25].into();
        outgoing
            .decode_failed
            .store(outcome == "failed", Ordering::Release);
        outgoing
            .cancelled
            .store(outcome == "cancelled", Ordering::Release);
        outgoing
            .decoder_session
            .decode_thread_finished
            .store(outcome != "unfinished", Ordering::Release);
        let (queue, mut output) = rodio::queue::queue(false);
        queue.append(outgoing);
        let _ = output.by_ref().take(100).collect::<Vec<_>>();
        assert!(
            std::iter::from_fn(|| received.try_recv().ok())
                .all(|event| !matches!(event, OutputEvent::GaplessCompleted { .. })),
            "{outcome} is not clean EOF"
        );
    }
}

#[test]
fn cleared_preload_does_not_fabricate_a_gapless_completion() {
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let mut outgoing = queued_gain_fixture(2, 2, 0.0);
    outgoing.events = events;
    outgoing.pending = vec![0.25, 0.25].into();
    outgoing
        .queued_successor_generation
        .store(2, Ordering::Release);
    // Same operation as OutputCommand::ClearPreload before source exhaustion.
    outgoing
        .queued_successor_generation
        .store(0, Ordering::Release);
    outgoing
        .decoder_session
        .decode_thread_finished
        .store(true, Ordering::Release);
    let (queue, mut output) = rodio::queue::queue(false);
    queue.append(outgoing);
    let _ = output.by_ref().take(100).collect::<Vec<_>>();
    assert!(std::iter::from_fn(|| received.try_recv().ok())
        .all(|event| !matches!(event, OutputEvent::GaplessCompleted { .. })));
}

#[test]
fn production_plain_install_updates_shared_successor_and_preserves_event_fifo() {
    let (mixer, mut output) =
        rodio::mixer::mixer(NonZeroU16::new(2).unwrap(), NonZeroU32::new(2).unwrap());
    let (player, source) = Player::new();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    // Stale hint is replaced by the exact reader production installs.
    let successor = Arc::new(AtomicU64::new(999));
    let active = Arc::new(AtomicU64::new(1));
    let mut old = queued_gain_fixture(2, 2, 0.0);
    old.events = events.clone();
    old.active_generation = active.clone();
    old.queued_successor_generation = successor.clone();
    old.pending = vec![0.2, 0.3, 0.0, 0.0].into();
    old.decoder_session
        .decode_thread_finished
        .store(true, Ordering::Release);
    player.append(NativeCrossfadeSource::new(
        old,
        Arc::new(std::sync::Mutex::new(None)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ));
    let mut incoming = queued_gain_fixture(2, 2, 0.0);
    incoming.generation = 2;
    incoming.events = events;
    incoming.active_generation = active;
    incoming.queued_successor_generation = successor.clone();
    incoming.pending = vec![0.4, 0.5].into();
    incoming
        .decoder_session
        .decode_thread_finished
        .store(true, Ordering::Release);
    // Exact production branch used by append_source when crossfade=None.
    install_queued_reader(&player, incoming, None);
    assert_eq!(successor.load(Ordering::Acquire), 2);
    // Attach after queueing so this tiny 2 Hz fixture does not initialize the
    // converter from Rodio's empty mono/48 kHz placeholder format.
    mixer.add(source);
    player.play();
    let mut pcm = Vec::new();
    for _ in 0..100 {
        pcm.push(output.next().expect("mixer remains alive"));
        if pcm.len() >= 6 && player.empty() {
            break;
        }
    }
    assert!(player.empty(), "finite real queue must drain");
    assert!(pcm.len() >= 6);
    assert_eq!(&pcm[..6], &[0.2, 0.3, 0.0, 0.0, 0.4, 0.5]);
    assert!(pcm[6..].iter().all(|sample| *sample == 0.0));
    let events = std::iter::from_fn(|| received.try_recv().ok()).collect::<Vec<_>>();
    assert!(matches!(events.as_slice(), [
        OutputEvent::Started { generation: 1, .. },
        OutputEvent::GaplessCompleted { from_generation: 1, to_generation: 2, position },
        OutputEvent::Started { generation: 2, .. }
    ] if *position == 1.0));
    for _ in 0..4 {
        assert_eq!(output.next(), Some(0.0));
    }
    assert!(
        received.try_recv().is_err(),
        "final EOF is left to existing Ended, never duplicated"
    );
}
