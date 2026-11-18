//! Bounded compressed read-ahead for native finite HTTP playback.
//! The decoder reads a stable local range while a producer recovers upstream
//! reads. Bytes stay in order and neither the decoder nor the output is reset.

use super::*;
use axum::body::Bytes;
use futures_util::future::BoxFuture;
use std::io;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

const FRAME_BYTES: usize = 32 * 1024;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const OVERLAP_BYTES: usize = 16 * 1024;
const LOOKAHEAD_SECONDS: f64 = 30.;
const READ_TIMEOUT: Duration = Duration::from_secs(4);

pub(super) struct Progressive {
    length: u64,
    content_type: Option<HeaderValue>,
    codec: Option<String>,
    buffer_frames: usize,
    memory: Arc<Semaphore>,
    representation: Mutex<Representation>,
}

#[derive(Default)]
struct Representation {
    etag: Option<HeaderValue>,
    started: bool,
    endpoint: Option<Endpoint>,
}

#[derive(Clone)]
struct Endpoint {
    url: url::Url,
    client: reqwest::Client,
    headers: HeaderMap,
}

impl Progressive {
    pub(super) fn new(
        length: u64,
        duration: f64,
        content_type: Option<String>,
        codec: Option<String>,
    ) -> Self {
        let buffer_frames =
            ((length as f64 / duration * LOOKAHEAD_SECONDS) / FRAME_BYTES as f64).ceil() as usize;
        Self {
            length,
            content_type: content_type
                .as_deref()
                .and_then(|value| HeaderValue::from_str(value).ok()),
            codec,
            buffer_frames: buffer_frames.clamp(1, CACHE_BYTES / FRAME_BYTES),
            memory: Arc::new(Semaphore::new(CACHE_BYTES / FRAME_BYTES)),
            representation: Mutex::new(Representation::default()),
        }
    }
}

fn request_bounds(value: Option<&HeaderValue>, length: u64) -> Option<(u64, u64)> {
    let last = length.checked_sub(1)?;
    let Some(value) = value else {
        return Some((0, last));
    };
    let value = value.to_str().ok()?.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return (suffix > 0).then_some((length.saturating_sub(suffix), last));
    }
    let start = start.parse().ok()?;
    let end = if end.is_empty() {
        last
    } else {
        last.min(end.parse().ok()?)
    };
    (start <= end).then_some((start, end))
}

pub(super) fn serve(
    session: Arc<Session>,
    headers: HeaderMap,
    method: Method,
    permit: OwnedSemaphorePermit,
) -> Response<Body> {
    serve_with_refresh(
        session,
        headers,
        method,
        permit,
        |session, progressive, deadline| Box::pin(refresh_endpoint(session, progressive, deadline)),
    )
}

type OpenRefreshedEndpoint = for<'a> fn(
    &'a Session,
    &'a Progressive,
    PlaybackDeadline,
) -> BoxFuture<'a, Result<Endpoint, ReadFailure>>;

fn serve_with_refresh(
    session: Arc<Session>,
    headers: HeaderMap,
    method: Method,
    permit: OwnedSemaphorePermit,
    refresh: OpenRefreshedEndpoint,
) -> Response<Body> {
    let Some(progressive) = session.progressive.as_ref() else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    };
    let range = headers.get(header::RANGE);
    let length = progressive.length;
    let Some((start, end)) = request_bounds(range, length) else {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{length}"))
            .body(Body::empty())
            .unwrap();
    };
    let mut response = Response::builder()
        .header(
            header::CONTENT_TYPE,
            progressive
                .content_type
                .clone()
                .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream")),
        )
        .header(header::CONTENT_LENGTH, end - start + 1)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if range.is_some() {
        response = response.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{length}"),
        );
    }
    if method == Method::HEAD {
        return response.body(Body::empty()).unwrap();
    }

    let (tx, rx) = mpsc::channel(progressive.buffer_frames);
    let mut stop = session.stop.subscribe();
    let body_stop = stop.clone();
    tokio::spawn(async move {
        if *stop.borrow() {
            return;
        }
        if let Err(error) = tokio::select! {
            biased;
            _ = stop.changed() => Ok(()),
            _ = tx.closed() => Ok(()),
            outcome = transfer(&session, start, end, &tx, refresh) => outcome,
        } {
            tokio::select! {
                biased;
                _ = stop.changed() => {},
                _ = tx.send(Err(error)) => {},
            }
        }
    });
    let stream = futures_util::stream::unfold(
        (rx, permit, body_stop),
        |(mut rx, permit, stop)| async move {
            let next: Frame = tokio::select! {
                biased;
                _ = crate::playback_sources::wait_for_cancellation(stop.clone()) => return None,
                next = rx.recv() => next?,
            };
            Some((next.map(|(bytes, _memory)| bytes), (rx, permit, stop)))
        },
    );
    response.body(Body::from_stream(stream)).unwrap()
}

type Frame = Result<(Bytes, OwnedSemaphorePermit), io::Error>;

struct EncodedFrames {
    pending: Vec<u8>,
    tail: Vec<u8>,
    sent_first: bool,
}

impl Default for EncodedFrames {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(FRAME_BYTES),
            tail: Vec::with_capacity(OVERLAP_BYTES),
            sent_first: false,
        }
    }
}

impl EncodedFrames {
    async fn push(
        &mut self,
        mut bytes: &[u8],
        progressive: &Progressive,
        output: &mpsc::Sender<Frame>,
    ) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let keep = OVERLAP_BYTES
            .saturating_sub(bytes.len())
            .min(self.tail.len());
        self.tail.drain(..self.tail.len() - keep);
        self.tail
            .extend_from_slice(&bytes[bytes.len().saturating_sub(OVERLAP_BYTES)..]);
        while !bytes.is_empty() {
            let count = bytes.len().min(FRAME_BYTES - self.pending.len());
            self.pending.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if !self.sent_first || self.pending.len() == FRAME_BYTES {
                self.flush(progressive, output).await?;
            }
        }
        Ok(())
    }

    async fn flush(
        &mut self,
        progressive: &Progressive,
        output: &mpsc::Sender<Frame>,
    ) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let memory = progressive
            .memory
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("Playback cache closed"))?;
        let buffer = std::mem::replace(&mut self.pending, Vec::with_capacity(FRAME_BYTES));
        output
            .send(Ok((Bytes::from(buffer), memory)))
            .await
            .map_err(|_| io::Error::other("Decoder closed the audio range"))?;
        self.sent_first = true;
        Ok(())
    }
}

enum ReadFailure {
    SourceDenied,
    Transient,
    Invalid,
}

impl ReadFailure {
    fn message(&self) -> &'static str {
        match self {
            Self::SourceDenied => "Source denied access to compressed audio",
            Self::Transient => "Compressed audio read failed",
            Self::Invalid => "Compressed audio range or representation changed",
        }
    }

    fn category(&self) -> &'static str {
        match self {
            Self::SourceDenied => "source_access",
            Self::Transient => "network",
            Self::Invalid => "representation_changed",
        }
    }
}

fn strong_etag(headers: &HeaderMap) -> Option<&HeaderValue> {
    headers.get(header::ETAG).filter(|etag| {
        etag.to_str()
            .is_ok_and(|etag| etag.len() >= 2 && etag.starts_with('"') && etag.ends_with('"'))
    })
}

fn validate_range(headers: &HeaderMap, start: u64, end: u64, length: u64) -> Option<u64> {
    if headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| value != "identity")
    {
        return None;
    }
    let (range, total) = headers
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes ")?
        .split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let last = last.parse::<u64>().ok()?;
    if first.parse::<u64>().ok()? != start
        || !(start..=end).contains(&last)
        || total.parse::<u64>().ok()? != length
    {
        return None;
    }
    let span = last - start + 1;
    if headers.get(header::CONTENT_LENGTH).is_some_and(|value| {
        value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            != Some(span)
    }) {
        return None;
    }
    Some(span)
}

async fn open_range(
    endpoint: &Endpoint,
    range: (u64, u64),
    length: u64,
    deadline: Option<PlaybackDeadline>,
) -> Result<(reqwest::Response, u64), ReadFailure> {
    let (start, end) = range;
    let request = endpoint
        .client
        .get(endpoint.url.clone())
        .headers(endpoint.headers.clone())
        .header(header::RANGE, format!("bytes={start}-{end}"))
        .send();
    let expiration = deadline
        .map(|deadline| deadline.instant())
        .unwrap_or_else(|| tokio::time::Instant::now() + READ_TIMEOUT)
        .min(tokio::time::Instant::now() + READ_TIMEOUT);
    let response = tokio::time::timeout_at(expiration, request)
        .await
        .map_err(|_| ReadFailure::Transient)?
        .map_err(|_| ReadFailure::Transient)?;
    match response.status().as_u16() {
        206 => {}
        401 | 403 | 410 => return Err(ReadFailure::SourceDenied),
        408 | 429 | 500..=599 => return Err(ReadFailure::Transient),
        _ => return Err(ReadFailure::Invalid),
    }
    let count =
        validate_range(response.headers(), start, end, length).ok_or(ReadFailure::Invalid)?;
    Ok((response, count))
}

async fn refresh_endpoint(
    session: &Session,
    progressive: &Progressive,
    deadline: PlaybackDeadline,
) -> Result<Endpoint, ReadFailure> {
    let refresh = session.refresh.as_ref().ok_or(ReadFailure::SourceDenied)?;
    let cancel = session.stop.subscribe();
    let source = tokio::time::timeout_at(deadline.instant(), refresh(deadline, cancel))
        .await
        .map_err(|_| ReadFailure::Transient)?
        .map_err(|_| ReadFailure::SourceDenied)?;
    if source.cenc.is_some()
        || source.decoder_hint.as_ref().is_none_or(|hint| {
            hint.has_video
                || hint.content_length != Some(progressive.length)
                || hint.mime_type.as_deref()
                    != progressive
                        .content_type
                        .as_ref()
                        .and_then(|content_type| content_type.to_str().ok())
                || hint.codec != progressive.codec
        })
    {
        return Err(ReadFailure::Invalid);
    }
    let (url, client, headers) = tokio::time::timeout_at(
        deadline.instant(),
        public_stream_endpoint(&source.input, &source.headers),
    )
    .await
    .map_err(|_| ReadFailure::Transient)?
    .map_err(|_| ReadFailure::SourceDenied)?;
    Ok(Endpoint {
        url,
        client,
        headers,
    })
}

async fn transfer(
    session: &Session,
    start: u64,
    end: u64,
    output: &mpsc::Sender<Frame>,
    refresh: OpenRefreshedEndpoint,
) -> io::Result<()> {
    let progressive = session
        .progressive
        .as_ref()
        .ok_or_else(|| io::Error::other("Audio range revoked"))?;
    let mut endpoint = progressive
        .representation
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .endpoint
        .clone()
        .unwrap_or_else(|| Endpoint {
            url: session.source.clone(),
            client: session.client.clone(),
            headers: session.headers.clone(),
        });
    let mut frames = EncodedFrames::default();
    let mut cursor = start;
    let mut recovery = None;
    let mut retried = false;
    let mut refreshed = false;
    let mut publish_endpoint = false;
    while cursor <= end {
        let recovering = recovery.is_some();
        let read_start = if recovering {
            cursor - frames.tail.len() as u64
        } else {
            cursor
        };
        let read_end = end.min(cursor.saturating_add(REQUEST_BYTES - 1));
        let before = cursor;
        let started = tokio::time::Instant::now();
        let result = append_range(
            progressive,
            &endpoint,
            (read_start, read_end),
            &mut cursor,
            publish_endpoint,
            recovery,
            &mut frames,
            output,
        )
        .await;
        if cursor > before {
            recovery = None;
            retried = false;
            refreshed = false;
            publish_endpoint = false;
        }
        let Err(error) = result else {
            continue;
        };
        trace_buffered_io(
            session,
            output,
            "native_buffered_read_error",
            started,
            Some(&error),
        );
        let deadline = *recovery.get_or_insert_with(|| {
            PlaybackDeadline::after(crate::playback_policy::RECOVERY_BUDGET)
        });
        if matches!(error, ReadFailure::Transient) && !retried {
            retried = true;
            continue;
        }
        if !matches!(error, ReadFailure::Invalid) && !refreshed {
            let started = tokio::time::Instant::now();
            let replacement = refresh(session, progressive, deadline).await;
            trace_buffered_io(
                session,
                output,
                "native_buffered_refresh_end",
                started,
                replacement.as_ref().err(),
            );
            endpoint = replacement.map_err(|error| io::Error::other(error.message()))?;
            refreshed = true;
            publish_endpoint = true;
            continue;
        }
        return Err(io::Error::other(error.message()));
    }
    frames.flush(progressive, output).await
}

fn trace_buffered_io(
    session: &Session,
    output: &mpsc::Sender<Frame>,
    phase: &'static str,
    started: tokio::time::Instant,
    failure: Option<&ReadFailure>,
) {
    if !crate::playback_diagnostics::is_enabled() {
        return;
    }
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    crate::playback_diagnostics::record_safe_event(serde_json::json!({
        "resolutionId": session.id,
        "phase": phase,
        "provider": "other",
        "atUnixMs": unix_ms,
        "elapsedMs": started.elapsed().as_millis() as u64,
        "details": {
            "queuedFrameCount": output.max_capacity() - output.capacity(),
            "targetFrameCount": output.max_capacity(),
            "frameBytes": FRAME_BYTES,
            "ok": failure.is_none(),
            "reason": failure.map(ReadFailure::category),
        }
    }));
}

async fn append_range(
    progressive: &Progressive,
    endpoint: &Endpoint,
    range: (u64, u64),
    cursor: &mut u64,
    publish_endpoint: bool,
    deadline: Option<PlaybackDeadline>,
    frames: &mut EncodedFrames,
    output: &mpsc::Sender<Frame>,
) -> Result<(), ReadFailure> {
    let (response, mut remaining) =
        open_range(endpoint, range, progressive.length, deadline).await?;
    let etag = strong_etag(response.headers()).cloned();
    let require_overlap = {
        let identity = progressive
            .representation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if identity
            .etag
            .as_ref()
            .is_some_and(|original| etag.as_ref() != Some(original))
        {
            return Err(ReadFailure::Invalid);
        }
        deadline.is_some() && identity.started && identity.etag.is_none()
    };
    let overlap = *cursor - range.0;
    if require_overlap && overlap == 0 || remaining <= overlap {
        return Err(ReadFailure::Invalid);
    }
    let mut compared = 0usize;
    let mut committed = false;
    let mut body = response.bytes_stream();
    while remaining > 0 {
        // Once continuity is established, normal per-read timeouts apply.
        // The producer can now pause for backpressure without using up the
        // failed request's recovery budget.
        let expiry = deadline
            .filter(|_| !committed)
            .map(|deadline| deadline.instant())
            .unwrap_or_else(|| tokio::time::Instant::now() + READ_TIMEOUT)
            .min(tokio::time::Instant::now() + READ_TIMEOUT);
        let chunk = tokio::time::timeout_at(expiry, body.next())
            .await
            .map_err(|_| ReadFailure::Transient)?
            .ok_or(ReadFailure::Transient)?
            .map_err(|_| ReadFailure::Transient)?;
        if chunk.len() as u64 > remaining {
            return Err(ReadFailure::Invalid);
        }
        remaining -= chunk.len() as u64;
        let mut bytes: &[u8] = &chunk;
        if compared < overlap as usize {
            let take = bytes.len().min(overlap as usize - compared);
            if require_overlap && frames.tail[compared..compared + take] != bytes[..take] {
                return Err(ReadFailure::Invalid);
            }
            compared += take;
            bytes = &bytes[take..];
        }
        if bytes.is_empty() {
            continue;
        }
        if !committed {
            let mut identity = progressive
                .representation
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // Another range may establish the original validator while this
            // request is in flight. Recheck before releasing any bytes.
            if identity
                .etag
                .as_ref()
                .is_some_and(|original| etag.as_ref() != Some(original))
                || deadline.is_some()
                    && identity.started
                    && identity.etag.is_none()
                    && !require_overlap
            {
                return Err(ReadFailure::Invalid);
            }
            if !identity.started {
                identity.etag = etag.clone();
                identity.started = true;
            }
            if publish_endpoint {
                identity.endpoint = Some(endpoint.clone());
            }
            committed = true;
        }
        frames
            .push(bytes, progressive, output)
            .await
            .map_err(|_| ReadFailure::Invalid)?;
        *cursor += bytes.len() as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn ranged_server(
        data: Arc<Vec<u8>>,
        requests: Vec<(u64, u64, Option<usize>)>,
    ) -> (Endpoint, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (start, end, sent) in requests {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut bytes = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let count = socket.read(&mut bytes).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&bytes[..count]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(request.contains(&format!("\r\nrange: bytes={start}-{end}\r\n")));
                let Some(sent) = sent else {
                    socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                    socket.shutdown().await.unwrap();
                    continue;
                };
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                            end - start + 1,
                            data.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket
                    .write_all(&data[start as usize..start as usize + sent])
                    .await
                    .unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        (
            Endpoint {
                url: url::Url::parse(&format!("http://{address}/audio")).unwrap(),
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                headers: HeaderMap::new(),
            },
            server,
        )
    }

    #[test]
    fn decoder_ranges_are_clamped_and_invalid_ranges_are_rejected() {
        let bounds = |value| request_bounds(Some(&HeaderValue::from_static(value)), 20);
        assert_eq!(bounds("bytes=0-"), Some((0, 19)));
        assert_eq!(bounds("bytes=17-22"), Some((17, 19)));
        assert_eq!(bounds("bytes=-4"), Some((16, 19)));
        assert_eq!(bounds("bytes=20-"), None);
        assert_eq!(bounds("bytes=0-3,6-7"), None);
        assert_eq!(bounds("bytes=4-3"), None);
        assert_eq!(bounds("bytes=-0"), None);
    }

    #[test]
    fn remote_ranges_require_exact_start_and_original_total() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 3-6/20"),
        );
        assert_eq!(validate_range(&headers, 3, 19, 20), Some(4));
        assert_eq!(validate_range(&headers, 2, 19, 20), None);
        assert_eq!(validate_range(&headers, 3, 5, 20), None);
        assert_eq!(validate_range(&headers, 3, 19, 21), None);
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("3"));
        assert_eq!(validate_range(&headers, 3, 19, 20), None);
    }

    #[tokio::test]
    async fn compressed_frames_play_immediately_and_bound_the_reserve() {
        let progressive = Progressive::new(3 * 1024 * 1024, 120., None, None);
        assert_eq!(progressive.buffer_frames, 24);
        assert_eq!(
            Progressive::new(u64::MAX, 1., None, None).buffer_frames * FRAME_BYTES,
            CACHE_BYTES
        );
        let (out, mut input) = mpsc::channel(3);
        let mut frames = EncodedFrames::default();
        frames.push(b"start", &progressive, &out).await.unwrap();
        assert_eq!(input.try_recv().unwrap().unwrap().0.as_ref(), b"start");
        let bytes = vec![2u8; FRAME_BYTES + 1];
        frames.push(&bytes, &progressive, &out).await.unwrap();
        assert_eq!(frames.tail, vec![2; OVERLAP_BYTES]);
        assert_eq!(input.try_recv().unwrap().unwrap().0.len(), FRAME_BYTES);
        frames.flush(&progressive, &out).await.unwrap();
        assert_eq!(input.try_recv().unwrap().unwrap().0.as_ref(), &[2]);
    }

    #[tokio::test]
    async fn repeated_short_bodies_resume_at_the_exact_missing_byte() {
        let data = Arc::new(
            (0..160_000)
                .map(|index| (index % 241) as u8)
                .collect::<Vec<_>>(),
        );
        let overlap = OVERLAP_BYTES as u64;
        let end = data.len() as u64 - 1;
        let (endpoint, server) = ranged_server(
            data.clone(),
            vec![
                (0, end, Some(60_000)),
                (60_000 - overlap, end, Some(OVERLAP_BYTES + 55_000)),
                (115_000 - overlap, end, Some(OVERLAP_BYTES + 45_000)),
            ],
        )
        .await;
        let session = Session {
            id: "test-range".into(),
            token: "test".into(),
            generation: 1,
            _track_id: "fixture".into(),
            source: endpoint.url,
            headers: endpoint.headers,
            client: endpoint.client,
            stop: watch::channel(false).0,
            slots: Arc::new(Semaphore::new(1)),
            hls: Mutex::new(hls::Graph::default()),
            allow_radio_http: false,
            progressive: Some(Progressive::new(data.len() as u64, 120., None, None)),
            refresh: None,
        };
        let permit = session.slots.clone().acquire_owned().await.unwrap();
        let response = serve(Arc::new(session), HeaderMap::new(), Method::GET, permit);
        let played = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), data.len()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(played.as_ref(), data.as_slice());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn native_http_decoder_finishes_audio_after_an_interrupted_read() {
        const SECONDS: u32 = 4;
        const SAMPLE_RATE: u32 = 48_000;
        let size = SECONDS * SAMPLE_RATE * 4;
        let mut data = Vec::with_capacity(44 + size as usize);
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&(36 + size).to_le_bytes());
        data.extend_from_slice(b"WAVEfmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        data.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&size.to_le_bytes());
        for frame in 0..SECONDS * SAMPLE_RATE {
            let sample: i16 = if frame % 120 < 60 { 8192 } else { -8192 };
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let bytes = data.len();
        let interrupted_at = bytes / 2;
        let resume = interrupted_at - OVERLAP_BYTES;
        let end = bytes as u64 - 1;
        let (endpoint, origin) = ranged_server(
            Arc::new(data),
            vec![
                (0, end, Some(interrupted_at)),
                (resume as u64, end, Some(bytes - resume)),
            ],
        )
        .await;
        let transport = WebviewAudioTransport::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        transport.set_base_url(format!("http://{}", listener.local_addr().unwrap()));
        let router = transport.router();
        let relay = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let lease = transport
            .commit_native_session(Arc::new(Session {
                id: uuid::Uuid::new_v4().to_string(),
                token: uuid::Uuid::new_v4().to_string(),
                generation: 1,
                _track_id: "fixture".into(),
                source: endpoint.url,
                headers: endpoint.headers,
                client: endpoint.client,
                stop: watch::channel(false).0,
                slots: Arc::new(Semaphore::new(1)),
                hls: Mutex::new(hls::Graph::default()),
                allow_radio_http: false,
                progressive: Some(Progressive::new(
                    bytes as u64,
                    f64::from(SECONDS),
                    Some("audio/wav".into()),
                    None,
                )),
                refresh: None,
            }))
            .unwrap();
        let (_stop, cancel) = watch::channel(false);
        let report = crate::native_output::direct_pcm_experiment::collect_until(
            AudioSource {
                input: lease.loopback_url().into(),
                ffmpeg: "ffmpeg-disabled-for-native-fixture".into(),
                headers: Vec::new(),
                decoder_hint: None,
                cenc: None,
            },
            0.,
            f64::from(SECONDS),
            std::time::Instant::now() + Duration::from_secs(8),
            cancel,
        )
        .await;
        assert!(report.ok, "{report:?}");
        assert_eq!(report.decoded_frames, u64::from(SECONDS * SAMPLE_RATE));
        assert!((report.pcm_rms.unwrap() - 0.25).abs() < 0.001, "{report:?}");
        let decoder = report.decoder.unwrap();
        assert_eq!(decoder.decoder_kind, "rodio");
        assert!(decoder.bytes_read > 0 && decoder.bytes_received > 0);
        origin.await.unwrap();
        relay.abort();
    }

    #[tokio::test]
    async fn revoked_lease_discards_already_buffered_frames() {
        let data = Arc::new(vec![0x11; 40_000]);
        let (endpoint, server) = ranged_server(data, vec![(0, 39_999, Some(40_000))]).await;
        let session = Arc::new(Session {
            id: "test-range".into(),
            token: "test".into(),
            generation: 1,
            _track_id: "fixture".into(),
            source: endpoint.url,
            headers: endpoint.headers,
            client: endpoint.client,
            stop: watch::channel(false).0,
            slots: Arc::new(Semaphore::new(1)),
            hls: Mutex::new(hls::Graph::default()),
            allow_radio_http: false,
            progressive: Some(Progressive::new(40_000, 120., None, None)),
            refresh: None,
        });
        let permit = session.slots.clone().acquire_owned().await.unwrap();
        let response = serve(session.clone(), HeaderMap::new(), Method::GET, permit);
        server.await.unwrap();
        let memory = &session.progressive.as_ref().unwrap().memory;
        tokio::time::timeout(Duration::from_secs(1), async {
            while memory.available_permits() == CACHE_BYTES / FRAME_BYTES {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        session.stop.send_replace(true);
        let bytes = axum::body::to_bytes(response.into_body(), 40_000)
            .await
            .unwrap();
        assert!(bytes.is_empty());
        assert_eq!(session.slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn changed_representation_cannot_replace_buffered_bytes() {
        let data = Arc::new(vec![0x17; 40_000]);
        let (old, old_server) = ranged_server(data, vec![(0, 39_999, Some(20_000))]).await;
        let (changed, new_server) = ranged_server(
            Arc::new(vec![0x25; 40_000]),
            vec![(
                20_000 - OVERLAP_BYTES as u64,
                39_999,
                Some(20_000 + OVERLAP_BYTES),
            )],
        )
        .await;
        let progressive = Progressive::new(40_000, 120., None, None);
        let (tx, mut rx) = mpsc::channel(4);
        let mut cursor = 0;
        let mut frames = EncodedFrames::default();
        let first = append_range(
            &progressive,
            &old,
            (0, 39_999),
            &mut cursor,
            false,
            None,
            &mut frames,
            &tx,
        )
        .await;
        assert!(matches!(first, Err(ReadFailure::Transient)));
        assert_eq!(cursor, 20_000);
        let emitted = rx.try_recv().unwrap().unwrap().0;
        assert!(emitted.iter().all(|value| *value == 0x17));

        let second = append_range(
            &progressive,
            &changed,
            (cursor - OVERLAP_BYTES as u64, 39_999),
            &mut cursor,
            true,
            Some(PlaybackDeadline::after(Duration::from_secs(2))),
            &mut frames,
            &tx,
        )
        .await;
        assert!(matches!(second, Err(ReadFailure::Invalid)));
        assert_eq!(cursor, 20_000);
        assert!(rx.try_recv().is_err());
        old_server.await.unwrap();
        new_server.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg with AAC support and local TCP"]
    async fn ffmpeg_decodes_complete_pcm_across_two_interruptions_and_a_denied_range() {
        use std::process::Stdio;
        use std::sync::atomic::AtomicBool;

        let ffmpeg = std::env::var_os("BURRATA_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let directory = tempfile::tempdir().unwrap();
        let encoded = directory.path().join("fixture.m4a");
        let result = tokio::process::Command::new(&ffmpeg)
            .kill_on_drop(true)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "anoisesrc=color=white:amplitude=0.3:seed=47:r=24000",
                "-t",
                "120",
                "-c:a",
                "aac",
                "-b:a",
                "64k",
                "-movflags",
                "+faststart",
            ])
            .arg(&encoded)
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let data = Arc::new(tokio::fs::read(&encoded).await.unwrap());
        assert!(data.len() > 100_000 && data.len() as u64 <= REQUEST_BYTES);
        let end = data.len() as u64 - 1;
        let first_drop = data.len() / 3;
        let second_drop = first_drop * 2;
        let (initial, initial_server) = ranged_server(
            data.clone(),
            vec![
                (0, end, Some(first_drop)),
                (
                    (first_drop - OVERLAP_BYTES) as u64,
                    end,
                    Some(OVERLAP_BYTES + second_drop - first_drop),
                ),
                ((second_drop - OVERLAP_BYTES) as u64, end, None),
            ],
        )
        .await;
        let (replacement, replacement_server) = ranged_server(
            data.clone(),
            vec![(
                (second_drop - OVERLAP_BYTES) as u64,
                end,
                Some(OVERLAP_BYTES + data.len() - second_drop),
            )],
        )
        .await;

        // Inject the already-resolved local endpoint into the otherwise
        // identical producer. Host validation and pinning have separate tests.
        fn local_refresh<'a>(
            session: &'a Session,
            _progressive: &'a Progressive,
            deadline: PlaybackDeadline,
        ) -> BoxFuture<'a, Result<Endpoint, ReadFailure>> {
            Box::pin(async move {
                let refresh = session.refresh.as_ref().ok_or(ReadFailure::Invalid)?;
                let source = refresh(deadline, session.stop.subscribe())
                    .await
                    .map_err(|_| ReadFailure::Invalid)?;
                Ok(Endpoint {
                    url: url::Url::parse(&source.input).unwrap(),
                    client: reqwest::Client::builder().no_proxy().build().unwrap(),
                    headers: HeaderMap::new(),
                })
            })
        }
        let refresh_count = Arc::new(AtomicU64::new(0));
        let refreshing = Arc::new(AtomicBool::new(false));
        let session = Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            token: "test".into(),
            generation: 1,
            _track_id: "fixture".into(),
            source: initial.url,
            headers: initial.headers,
            client: initial.client,
            stop: watch::channel(false).0,
            slots: Arc::new(Semaphore::new(1)),
            hls: Mutex::new(hls::Graph::default()),
            allow_radio_http: false,
            progressive: Some(Progressive::new(
                data.len() as u64,
                120.,
                Some("audio/mp4".into()),
                Some("aac".into()),
            )),
            refresh: Some(Arc::new({
                let refresh_count = refresh_count.clone();
                let refreshing = refreshing.clone();
                move |_, _| {
                    refresh_count.fetch_add(1, Ordering::Relaxed);
                    let input = replacement.url.to_string();
                    let refreshing = refreshing.clone();
                    Box::pin(async move {
                        refreshing.store(true, Ordering::Relaxed);
                        tokio::time::sleep(Duration::from_secs(8)).await;
                        refreshing.store(false, Ordering::Relaxed);
                        Ok(AudioSource {
                            input,
                            ffmpeg: Default::default(),
                            headers: Vec::new(),
                            decoder_hint: None,
                            cenc: None,
                        })
                    })
                }
            })),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_addr = listener.local_addr().unwrap();
        let relay = tokio::spawn(async move {
            let router = Router::new().route(
                "/audio",
                get(move |method: Method, headers: HeaderMap| {
                    let session = session.clone();
                    async move {
                        let permit = session.slots.clone().acquire_owned().await.unwrap();
                        serve_with_refresh(session, headers, method, permit, local_refresh)
                    }
                }),
            );
            axum::serve(listener, router).await.unwrap();
        });
        let decode = |input: std::ffi::OsString| {
            let ffmpeg = ffmpeg.clone();
            async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    tokio::process::Command::new(ffmpeg)
                        .kill_on_drop(true)
                        .args(["-hide_banner", "-loglevel", "error", "-xerror", "-i"])
                        .arg(input)
                        .args(["-vn", "-f", "s16le", "-acodec", "pcm_s16le", "pipe:1"])
                        .output(),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                result.stdout
            }
        };
        let reference = decode(encoded.into_os_string()).await;
        let mut player = tokio::process::Command::new(&ffmpeg)
            .kill_on_drop(true)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-xerror",
                "-re",
                "-http_proxy",
                "",
                "-i",
            ])
            .arg(format!("http://{relay_addr}/audio"))
            .args(["-vn", "-f", "s16le", "-acodec", "pcm_s16le", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut pcm = player.stdout.take().unwrap();
        let playback_started = tokio::time::Instant::now();
        let mut recovered = Vec::new();
        let mut samples_during_refresh = 0;
        let mut chunk = [0; 8192];
        loop {
            let timeout = if recovered.is_empty() { 10 } else { 2 };
            let count = tokio::time::timeout(Duration::from_secs(timeout), pcm.read(&mut chunk))
                .await
                .expect("paced decoder stopped producing audible PCM")
                .unwrap();
            if count == 0 {
                break;
            }
            recovered.extend_from_slice(&chunk[..count]);
            if refreshing.load(Ordering::Relaxed) {
                samples_during_refresh += count / 2;
            }
        }
        assert!(player.wait().await.unwrap().success());
        assert!(playback_started.elapsed() >= Duration::from_secs(115));
        assert!(
            samples_during_refresh >= 24_000 * 4,
            "refresh did not overlap at least four seconds of decoded audio"
        );
        assert!(reference.len() >= 120 * 24_000 * 2);
        assert_eq!(recovered.len(), reference.len());
        assert!(
            recovered == reference,
            "recovered PCM differs from uninterrupted playback"
        );
        assert_eq!(refresh_count.load(Ordering::Relaxed), 1);
        initial_server.await.unwrap();
        replacement_server.await.unwrap();
        relay.abort();
    }
}
