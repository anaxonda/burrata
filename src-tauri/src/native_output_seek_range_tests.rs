// Included only inside native_output::tests. Staged, unexecuted causal fixture.
fn run_cached_seek_fixture_child() {
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
        .args([
            "--ignored",
            "--exact",
            "native_output::tests::cached_rodio_seek_same_range_body_gap_starves_then_recovers",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("BURRATA_SEEK_RANGE_FIXTURE_CHILD_V1", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn owned cached seek fixture");
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
    let deadline = Instant::now() + Duration::from_secs(40);
    let outcome = loop {
        match owner.child.as_mut().unwrap().try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {
                thread::park_timeout(Duration::from_millis(10))
            }
            Ok(None) => break Err("owned cached seek fixture exceeded40s"),
            Err(_) => break Err("could not wait for owned cached seek fixture"),
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
            "owned cached seek fixture failed: {status}\n{output}"
        ),
        Err(reason) => panic!("{reason}; child killed/reaped\n{output}"),
    }
    // Keep finite child phases visible on --nocapture successful runs too.
    eprintln!("{output}");
}

#[test]
#[ignore = "paced cached-seek range causal fixture; isolated child has a40s deadline"]
fn cached_rodio_seek_same_range_body_gap_starves_then_recovers() {
    if std::env::var("BURRATA_SEEK_RANGE_FIXTURE_CHILD_V1").as_deref() != Ok("1") {
        run_cached_seek_fixture_child();
        return;
    }
    use std::panic::{catch_unwind, AssertUnwindSafe};
    //120s leaves6s at95%, enough for a strict5s consumed window. This is
    // synthetic WAV coverage of the generic reader path, not FLAC seek-table proof.
    // pcm_wav emits ZERO-VALUED PCM: decoder-produced sample accounting is
    // distinguished from injected starvation zeros; no audible/nonzero proof.
    let body = Arc::new(pcm_wav(44_100, 2, 120));
    for delayed in [false, true] {
        eprintln!("cached_seek_fixture delayed={delayed} phase=warm payload=zero_pcm");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(!delayed));
        let range_claimed = Arc::new(AtomicBool::new(false));
        let range_count = Arc::new(AtomicU64::new(0));
        let server_failed = Arc::new(AtomicBool::new(false));
        let (range_tx, range_rx) = std_mpsc::channel::<(Instant, u64, u64)>();
        let server_stop = stopped.clone();
        let server_release = released.clone();
        let server_claim = range_claimed.clone();
        let server_ranges = range_count.clone();
        let server_fault = server_failed.clone();
        let bytes = body.clone();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            while !server_stop.load(Ordering::Acquire) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::park_timeout(Duration::from_millis(1));
                        continue;
                    }
                    Err(_) => {
                        server_fault.store(true, Ordering::Release);
                        break;
                    }
                };
                if requests.len() >= 12 {
                    server_fault.store(true, Ordering::Release);
                    break;
                }
                let bytes = bytes.clone();
                let stop = server_stop.clone();
                let release = server_release.clone();
                let claim = server_claim.clone();
                let ranges = server_ranges.clone();
                let fault = server_fault.clone();
                let range_tx = range_tx.clone();
                requests.push(thread::spawn(move || {
                    let serve = || -> std::io::Result<()> {
                        let mut socket=socket;
                        socket.set_read_timeout(Some(Duration::from_secs(1)))?;
                        socket.set_write_timeout(Some(Duration::from_secs(1)))?;
                        let mut request=Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            let mut byte=[0];socket.read_exact(&mut byte)?;request.push(byte[0]);
                            if request.len()>8192 {return Err(std::io::ErrorKind::InvalidData.into());}
                        }
                        let request=String::from_utf8(request).map_err(|_|std::io::ErrorKind::InvalidData)?;
                        let range=request.lines().find_map(|line|line.split_once(':').filter(|(name,_)|name.eq_ignore_ascii_case("range")).map(|(_,value)|value.trim()));
                        let Some(range)=range else {
                            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len())?;
                            //Enough data to fill16PCM blocks, but never pre-cache95%.
                            socket.write_all(&bytes[..262144])?;socket.flush()?;
                            while !stop.load(Ordering::Acquire) {thread::park_timeout(Duration::from_millis(1));}
                            return Ok(());
                        };
                        let (start,end)=range.strip_prefix("bytes=").and_then(|v|v.split_once('-')).ok_or(std::io::ErrorKind::InvalidData)?;
                        let start=start.parse::<usize>().map_err(|_|std::io::ErrorKind::InvalidData)?;
                        let end=if end.is_empty(){bytes.len()-1}else{end.parse::<usize>().map_err(|_|std::io::ErrorKind::InvalidData)?};
                        if start>end||end>=bytes.len(){return Err(std::io::ErrorKind::InvalidInput.into());}
                        ranges.fetch_add(1,Ordering::Relaxed);
                        write!(socket,"HTTP/1.1 206 Partial Content\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len(),end-start+1)?;
                        let payload=&bytes[start..=end];
                        let gate=start>bytes.len()*9/10&&payload.len()>131072&&!claim.swap(true,Ordering::AcqRel);
                        if gate {
                            socket.write_all(&payload[..65536])?;socket.flush()?;
                            //Both arms send this independent range-delivery origin;
                            //the delayed arm releases at origin+1500ms, not firstPCM.
                            let _=range_tx.send((Instant::now(),start as u64,end as u64));
                            while !release.load(Ordering::Acquire)&&!stop.load(Ordering::Acquire){thread::park_timeout(Duration::from_millis(1));}
                            if stop.load(Ordering::Acquire){return Ok(());}
                            socket.write_all(&payload[65536..])?;
                        }else{socket.write_all(payload)?;}
                        socket.flush()?;Ok(())
                    };
                    //Peer close during owned cancellation is normal; malformed
                    //fixture requests are still reported as structural failures.
                    if let Err(error)=serve(){
                        if matches!(error.kind(),std::io::ErrorKind::InvalidData|std::io::ErrorKind::InvalidInput){fault.store(true,Ordering::Release);}
                    }
                }));
            }
            for request in requests {
                if request.join().is_err() {
                    server_fault.store(true, Ordering::Release);
                }
            }
        });
        let session = Arc::new(DecoderSession::default());
        session.set_startup_deadline(Some(Instant::now() + Duration::from_secs(5)));
        let directory = tempfile::tempdir().unwrap();
        let source = AudioSource {
            input: format!("http://{address}/fixture.wav"),
            ffmpeg: directory.path().join("absent-ffmpeg"),
            headers: vec![],
            decoder_hint: None,

            cenc: None,
        };
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (warm_mixer, mut warm_output) =
            rodio::mixer::mixer(NonZeroU16::new(2).unwrap(), NonZeroU32::new(44100).unwrap());
        let warm = start_playback(
            &warm_mixer,
            0.,
            201,
            source.clone(),
            0.,
            None,
            0.,
            44100,
            2,
            events.clone(),
            crate::pcm_tap::PcmTap::new(),
            session.clone(),
            false,
        );
        let warm_result = catch_unwind(AssertUnwindSafe(|| {
            let deadline = Instant::now() + Duration::from_secs(4);
            while warm.samples.load(Ordering::Acquire) == 0 {
                assert!(Instant::now() < deadline, "warm first PCM missing");
                for _ in 0..4096 {
                    let _ = warm_output.next();
                }
                assert!(!warm.decode_failed.load(Ordering::Acquire));
                thread::park_timeout(Duration::from_millis(1));
            }
            //Do not continue output consumption: retire a producer blocked on
            //the full PCM queue, not on an absent HTTP byte range.
            while session
                .stream_stats
                .pcm_produced_samples
                .load(Ordering::Acquire)
                < 16 * 4096
            {
                assert!(Instant::now() < deadline, "warm queue never filled");
                thread::park_timeout(Duration::from_millis(1));
            }
        }));
        //Mirror OutputCommand::Seek's owned cancellation handoff; keeping the
        //download alive is essential to exercise cached Rodio.try_seek.
        let handoff = Arc::new(AtomicBool::new(false));
        session.bind(&handoff);
        session.set_startup_deadline(None);
        warm.cancel.store(true, Ordering::Release);
        warm.player.stop();
        drop(warm_output);
        let settle = Instant::now() + Duration::from_secs(2);
        while !session.decode_thread_finished.load(Ordering::Acquire) && Instant::now() < settle {
            thread::park_timeout(Duration::from_millis(1));
        }
        let warm_finished = session.decode_thread_finished.load(Ordering::Acquire);
        let cached_exists = warm_finished
            && session
                .decoder
                .lock()
                .map(|value| value.is_some())
                .unwrap_or(false);
        if warm_result.is_err() || !warm_finished || !cached_exists {
            handoff.store(true, Ordering::Release);
            stopped.store(true, Ordering::Release);
            released.store(true, Ordering::Release);
            let _ = server.join();
            if let Err(panic) = warm_result {
                std::panic::resume_unwind(panic);
            }
            panic!("warm decoder did not retire with a reusable cached decoder");
        }
        session
            .decode_thread_finished
            .store(false, Ordering::Release); //new worker completion latch
        while received.try_recv().is_ok() {}
        let (mixer, mut output) =
            rodio::mixer::mixer(NonZeroU16::new(2).unwrap(), NonZeroU32::new(44100).unwrap());
        let seek = start_playback(
            &mixer,
            0.,
            202,
            source,
            114.,
            None,
            114.,
            44100,
            2,
            events,
            crate::pcm_tap::PcmTap::new(),
            session.clone(),
            false,
        );
        let result = catch_unwind(AssertUnwindSafe(|| {
            let (origin, range_start, range_end) = range_rx
                .recv_timeout(Duration::from_secs(4))
                .expect("no actual95% range request");
            assert!(
                origin.elapsed() < Duration::from_millis(100),
                "range fixture scheduler gap before pacing"
            );
            //Start both clocks from range delivery, independent of first decoded-PCM consumption.
            let mut first = None;
            let mut first_samples = 0;
            let mut first_synthetic = 0;
            let mut starvation = None;
            let mut last_tick = Instant::now();
            let mut started = false;
            for tick in 1..1000u64 {
                let due = origin + Duration::from_millis(tick * 10);
                while Instant::now() < due {
                    thread::park_timeout(due.saturating_duration_since(Instant::now()));
                }
                let now = Instant::now();
                assert!(
                    now.duration_since(last_tick) < Duration::from_millis(100),
                    "paced seek scheduler gap"
                );
                last_tick = now;
                if now.duration_since(origin) >= Duration::from_millis(1500) {
                    released.store(true, Ordering::Release);
                }
                for _ in 0..882 {
                    assert!(output.next().is_some());
                }
                let consumed = seek.samples.load(Ordering::Acquire);
                if consumed > 0 && first.is_none() {
                    first = Some(now);
                    first_samples = consumed;
                    first_synthetic = session
                        .stream_stats
                        .synthetic_silence_samples
                        .load(Ordering::Acquire);
                }
                while let Ok(event) = received.try_recv() {
                    match event {
                        OutputEvent::Started {
                            generation,
                            decoder: Some(decoder),
                            ..
                        } => {
                            assert_eq!(generation, 202);
                            assert!(
                                decoder.reader_reused && decoder.decoder_reused,
                                "seek must use actual cached Rodio path"
                            );
                            assert_eq!(decoder.decoder_kind, "rodio");
                            assert!(decoder.startup_reserve.is_none());
                            started = true;
                        }
                        OutputEvent::Failed { .. } | OutputEvent::LocalStorageFailed { .. } => {
                            panic!("actual cached seek source failed")
                        }
                        _ => {}
                    }
                }
                if first.is_some()
                    && starvation.is_none()
                    && session
                        .stream_stats
                        .synthetic_silence_samples
                        .load(Ordering::Acquire)
                        > first_synthetic
                {
                    let OutputEvent::Buffering {
                        generation,
                        diagnostics: Some(facts),
                    } = buffering_event(&seek)
                    else {
                        panic!("missing seek buffering facts")
                    };
                    assert_eq!(generation, 202);
                    starvation = Some((now.duration_since(origin), facts));
                }
                assert!(!seek.decode_failed.load(Ordering::Acquire));
                if let Some(first) = first {
                    if now.duration_since(first) >= Duration::from_secs(5)
                        && consumed.saturating_sub(first_samples) >= 44100 * 2 * 5
                    {
                        break;
                    }
                }
            }
            let first = first.expect("seek never consumed actual PCM");
            let consumed = seek.samples.load(Ordering::Acquire);
            assert!(first.elapsed() >= Duration::from_secs(5));
            assert!(consumed.saturating_sub(first_samples) >= 44100 * 2 * 5);
            assert!(started);
            assert!(range_count.load(Ordering::Acquire) > 0);
            assert!(session.storage_faults.get().is_none());
            if delayed {
                let (at, facts) =
                    starvation.expect("held range body did not reproduce post-seek starvation");
                assert!(
                    at < Duration::from_millis(1500),
                    "starvation must precede independent gate release"
                );
                assert!(facts.read_in_flight_ms.is_some());
                assert_eq!(facts.pcm_produced_samples, facts.pcm_consumed_samples);
                assert!(facts.pcm_consumed_samples > 0);
                assert!(facts.synthetic_silence_samples > 0);
            } else {
                assert!(starvation.is_none(), "immediate same-range control starved");
                assert_eq!(
                    session
                        .stream_stats
                        .synthetic_silence_samples
                        .load(Ordering::Acquire)
                        .saturating_sub(first_synthetic),
                    0
                );
            }
            eprintln!("cached_seek_fixture delayed={delayed} phase=decoded_consumption_proved payload=zero_pcm ranges={} rangeStart={range_start} rangeEnd={range_end} consumed={consumed} elapsedMs={} firstPCMms={}",range_count.load(Ordering::Relaxed),origin.elapsed().as_millis(),first.duration_since(origin).as_millis());
        }));
        seek.cancel.store(true, Ordering::Release);
        seek.player.stop();
        drop(output);
        released.store(true, Ordering::Release);
        stopped.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !session.decode_thread_finished.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::park_timeout(Duration::from_millis(1));
        }
        let finished = session.decode_thread_finished.load(Ordering::Acquire);
        if finished {
            session.decoder.lock().unwrap().take();
        }
        let server_result = server.join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        assert!(finished, "seek decoder cleanup incomplete");
        server_result.unwrap();
        assert!(!server_failed.load(Ordering::Acquire));
        eprintln!("cached_seek_fixture delayed={delayed} phase=cleaned");
    }
}
