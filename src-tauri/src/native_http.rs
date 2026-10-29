//! Validated byte-range transport for the native decoder's stream-download client.
//! The supplied reqwest client retains its headers, DNS, TLS and proxy policy.

use axum::body::Bytes;
use futures_util::Stream;
use std::{
    error::Error,
    fmt,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use stream_download::http::reqwest::header::{self, HeaderMap};
use stream_download::{
    http::{format_range_header_bytes, reqwest, Client, ClientResponse},
    source::DecodeError,
};

#[derive(Debug)]
pub struct NativeHttpError(String);

impl fmt::Display for NativeHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl Error for NativeHttpError {}
impl DecodeError for NativeHttpError {}

fn invalid(message: &str) -> NativeHttpError {
    NativeHttpError(format!("Native HTTP byte range: {message}"))
}
fn unexpected_status(status: u16, phase: &str) -> NativeHttpError {
    // Status is enough for safe retry policy. Never include signed URLs,
    // response bodies, authentication headers or redirect locations.
    invalid(&format!("HTTP {status} on {phase} media response"))
}

fn request_error(error: reqwest::Error) -> NativeHttpError {
    NativeHttpError(format!(
        "Native HTTP request failed: {}",
        error.without_url()
    ))
}

#[derive(Clone, Debug)]
struct Representation {
    total: Option<u64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Representation {
    fn if_range(&self) -> Option<&str> {
        self.etag
            .as_deref()
            .filter(|etag| !etag.starts_with("W/"))
            .or(self.last_modified.as_deref())
    }

    fn validate(&self, headers: &HeaderMap, total: u64) -> Result<(), NativeHttpError> {
        if self.total.is_some_and(|expected| expected != total) {
            return Err(invalid("resource length changed during playback"));
        }
        if let Some(expected) = self.etag.as_deref().filter(|etag| !etag.starts_with("W/")) {
            if header_text(headers, header::ETAG)? != Some(expected) {
                return Err(invalid(
                    "resource ETag changed or disappeared during playback",
                ));
            }
        } else if let Some(expected) = self.last_modified.as_deref() {
            if header_text(headers, header::LAST_MODIFIED)? != Some(expected) {
                return Err(invalid(
                    "resource Last-Modified changed or disappeared during playback",
                ));
            }
        } else if let Some(expected) = self.etag.as_deref() {
            if header_text(headers, header::ETAG)? != Some(expected) {
                return Err(invalid(
                    "resource ETag changed or disappeared during playback",
                ));
            }
        }
        Ok(())
    }
}

/// Each instance belongs to one HttpStream, including its seeks and reconnects.
#[derive(Clone)]
pub struct NativeHttpClient {
    client: reqwest::Client,
    representation: Arc<Mutex<Option<Representation>>>,
}

impl NativeHttpClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            representation: Arc::new(Mutex::new(None)),
        }
    }

    fn representation(&self) -> Result<Option<Representation>, NativeHttpError> {
        self.representation
            .lock()
            .map(|state| state.clone())
            .map_err(|_| invalid("representation state is unavailable"))
    }
}

impl Client for NativeHttpClient {
    type Url = reqwest::Url;
    type Headers = HeaderMap;
    type Response = NativeHttpResponse;
    type Error = NativeHttpError;

    fn create() -> Self {
        Self::new(reqwest::Client::new())
    }

    async fn get(&self, url: &Self::Url) -> Result<Self::Response, Self::Error> {
        // HttpStream reconnects without a byte offset when Accept-Ranges is absent.
        // Reject that restart: accepting it would append byte zero at a later offset.
        if self.representation()?.is_some() {
            return Err(invalid("reconnect requires an explicit byte range"));
        }
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(request_error)?;
        if !matches!(
            response.status(),
            reqwest::StatusCode::OK | reqwest::StatusCode::PARTIAL_CONTENT
        ) {
            return Err(unexpected_status(response.status().as_u16(), "initial"));
        }
        let length = content_length(response.headers())?;
        if length == Some(0) {
            return Err(invalid("media response is empty"));
        }
        let total = match response.status() {
            reqwest::StatusCode::OK => {
                if response.headers().contains_key(header::CONTENT_RANGE) {
                    return Err(invalid("200 response unexpectedly contains Content-Range"));
                }
                length
            }
            reqwest::StatusCode::PARTIAL_CONTENT => {
                let range = parse_content_range(response.headers())?;
                if range.start != 0 || range.end != range.total - 1 {
                    return Err(invalid(
                        "initial response does not cover the complete resource",
                    ));
                }
                range.validate_length(length)?;
                Some(range.total)
            }
            _ => unreachable!("initial status checked above"),
        };
        let representation = Representation {
            total,
            etag: header_text(response.headers(), header::ETAG)?.map(str::to_owned),
            last_modified: header_text(response.headers(), header::LAST_MODIFIED)?
                .map(str::to_owned),
        };
        *self
            .representation
            .lock()
            .map_err(|_| invalid("representation state is unavailable"))? = Some(representation);
        Ok(NativeHttpResponse {
            response,
            expected_body: total,
        })
    }

    async fn get_range(
        &self,
        url: &Self::Url,
        start: u64,
        end: Option<u64>,
    ) -> Result<Self::Response, Self::Error> {
        if end.is_some_and(|end| end < start) {
            return Err(invalid("requested byte range is reversed"));
        }
        let representation = self
            .representation()?
            .ok_or_else(|| invalid("initial response is missing"))?;
        if representation.total.is_some_and(|total| start >= total) {
            return Err(invalid("requested byte range exceeds the resource"));
        }
        let mut request = self
            .client
            .get(url.clone())
            .header(header::RANGE, format_range_header_bytes(start, end));
        if let Some(validator) = representation.if_range() {
            request = request.header(header::IF_RANGE, validator);
        }
        let response = request.send().await.map_err(request_error)?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(unexpected_status(response.status().as_u16(), "range"));
        }
        let range = parse_content_range(response.headers())?;
        let expected_end = end.unwrap_or(range.total - 1).min(range.total - 1);
        if range.start != start || range.end != expected_end {
            return Err(invalid(
                "Content-Range does not match the requested byte range",
            ));
        }
        range.validate_length(content_length(response.headers())?)?;
        representation.validate(response.headers(), range.total)?;
        if let Some(stored) = self
            .representation
            .lock()
            .map_err(|_| invalid("representation state is unavailable"))?
            .as_mut()
        {
            stored.total = Some(range.total);
        }
        Ok(NativeHttpResponse {
            response,
            expected_body: Some(range.length()),
        })
    }
}

fn header_text(
    headers: &HeaderMap,
    name: header::HeaderName,
) -> Result<Option<&str>, NativeHttpError> {
    if headers.get_all(&name).iter().count() > 1 {
        return Err(invalid("duplicate response metadata header"));
    }
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| invalid("response metadata is not valid text"))
        })
        .transpose()
}

fn content_length(headers: &HeaderMap) -> Result<Option<u64>, NativeHttpError> {
    header_text(headers, header::CONTENT_LENGTH)?
        .map(|value| parse_decimal(value).ok_or_else(|| invalid("Content-Length is invalid")))
        .transpose()
}

fn parse_decimal(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

#[derive(Debug, PartialEq, Eq)]
struct ContentRange {
    start: u64,
    end: u64,
    total: u64,
}

impl ContentRange {
    fn length(&self) -> u64 {
        self.end - self.start + 1
    }

    fn validate_length(&self, length: Option<u64>) -> Result<(), NativeHttpError> {
        if length.is_some_and(|length| length != self.length()) {
            Err(invalid("Content-Length does not match Content-Range"))
        } else {
            Ok(())
        }
    }
}

fn parse_content_range(headers: &HeaderMap) -> Result<ContentRange, NativeHttpError> {
    let value = header_text(headers, header::CONTENT_RANGE)?
        .ok_or_else(|| invalid("206 response is missing Content-Range"))?;
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(|| invalid("Content-Range unit is invalid"))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| invalid("Content-Range is malformed"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| invalid("Content-Range is malformed"))?;
    let start = parse_decimal(start).ok_or_else(|| invalid("Content-Range start is invalid"))?;
    let end = parse_decimal(end).ok_or_else(|| invalid("Content-Range end is invalid"))?;
    let total = parse_decimal(total).ok_or_else(|| invalid("Content-Range total is invalid"))?;
    if start > end || end >= total {
        return Err(invalid("Content-Range bounds exceed the resource"));
    }
    Ok(ContentRange { start, end, total })
}

pub struct NativeHttpResponse {
    response: reqwest::Response,
    expected_body: Option<u64>,
}

impl ClientResponse for NativeHttpResponse {
    type ResponseError = NativeHttpError;
    type StreamError = NativeHttpError;
    type Headers = HeaderMap;

    fn content_length(&self) -> Option<u64> {
        self.expected_body
    }
    fn content_type(&self) -> Option<&str> {
        self.response
            .headers()
            .get(header::CONTENT_TYPE)?
            .to_str()
            .ok()
    }
    fn headers(&self) -> Self::Headers {
        self.response.headers().clone()
    }
    fn into_result(self) -> Result<Self, Self::ResponseError> {
        Ok(self)
    }
    fn stream(
        self,
    ) -> Box<dyn Stream<Item = Result<Bytes, Self::StreamError>> + Unpin + Send + Sync> {
        Box::new(CheckedBody {
            inner: Box::pin(self.response.bytes_stream()),
            expected: self.expected_body,
            received: 0,
            done: false,
        })
    }
}

struct CheckedBody {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + Sync>>,
    expected: Option<u64>,
    received: u64,
    done: bool,
}

impl Stream for CheckedBody {
    type Item = Result<Bytes, NativeHttpError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(bytes))) => {
                let received = this.received.checked_add(bytes.len() as u64);
                if received.is_none()
                    || this
                        .expected
                        .zip(received)
                        .is_some_and(|(expected, received)| received > expected)
                {
                    this.done = true;
                    return Poll::Ready(Some(Err(invalid(
                        "response body exceeds its declared byte range",
                    ))));
                }
                this.received = received.unwrap();
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.done = true;
                Poll::Ready(Some(Err(request_error(error))))
            }
            Poll::Ready(None) => {
                this.done = true;
                if this
                    .expected
                    .is_some_and(|expected| expected != this.received)
                {
                    Poll::Ready(Some(Err(invalid(
                        "response body ended before its declared byte range",
                    ))))
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::{Duration, Instant},
    };

    fn fixture(
        responses: Vec<String>,
    ) -> (
        reqwest::Url,
        Arc<Mutex<Vec<String>>>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/audio", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let worker = thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 16384);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                // A rejected response can close the socket before its body is sent.
                let _ = socket.write_all(response.as_bytes());
            }
        });
        (url, requests, worker)
    }

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n{body}")
    }
    fn initial() -> String {
        response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"recording-v1\"\r\nAccept-Ranges: bytes\r\n",
            "0123456789",
        )
    }
    fn client() -> NativeHttpClient {
        NativeHttpClient::new(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
        )
    }
    async fn collect(response: NativeHttpResponse) -> Result<Vec<u8>, NativeHttpError> {
        let mut body = response.stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            bytes.extend_from_slice(&chunk?);
        }
        Ok(bytes)
    }

    #[tokio::test]
    async fn proper_206_preserves_client_headers_and_validates_requested_bytes() {
        let ranged = response(
            "206 Partial Content",
            "Content-Range: bytes 4-7/10\r\nContent-Length: 4\r\nETag: \"recording-v1\"\r\n",
            "4567",
        );
        let (url, requests, worker) = fixture(vec![initial(), ranged]);
        let mut headers = HeaderMap::new();
        headers.insert("x-fixture-source", "retained".parse().unwrap());
        let client = NativeHttpClient::new(
            reqwest::Client::builder()
                .no_proxy()
                .default_headers(headers)
                .build()
                .unwrap(),
        );
        assert_eq!(
            collect(client.get(&url).await.unwrap()).await.unwrap(),
            b"0123456789"
        );
        assert_eq!(
            collect(client.get_range(&url, 4, Some(7)).await.unwrap())
                .await
                .unwrap(),
            b"4567"
        );
        worker.join().unwrap();
        let requests = requests.lock().unwrap();
        assert!(requests.iter().all(|request| {
            request
                .to_ascii_lowercase()
                .contains("x-fixture-source: retained\r\n")
        }));
        assert!(requests[1]
            .to_ascii_lowercase()
            .contains("range: bytes=4-7\r\n"));
        assert!(requests[1]
            .to_ascii_lowercase()
            .contains("if-range: \"recording-v1\"\r\n"));
    }

    #[tokio::test]
    async fn reversed_range_request_is_rejected_before_contacting_the_origin() {
        let (url, requests, worker) = fixture(vec![initial()]);
        let client = client();
        collect(client.get(&url).await.unwrap()).await.unwrap();
        assert!(client.get_range(&url, 8, Some(4)).await.is_err());
        worker.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bounded_range_rejects_an_origin_that_returns_through_eof() {
        let ranged = response(
            "206 Partial Content",
            "Content-Range: bytes 4-9/10\r\nContent-Length: 6\r\nETag: \"recording-v1\"\r\n",
            "456789",
        );
        let (url, _, worker) = fixture(vec![initial(), ranged]);
        let client = client();
        collect(client.get(&url).await.unwrap()).await.unwrap();
        // A hole between buffered sections requires a bounded refill. Correct
        // metadata for a different (longer) range must not be accepted for it.
        assert!(client.get_range(&url, 4, Some(7)).await.is_err());
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn nonzero_range_rejects_200_and_malformed_or_mismatched_metadata() {
        for (status, headers, body) in [
            (
                "200 OK",
                "Content-Length: 10\r\nETag: \"recording-v1\"\r\n",
                "0123456789",
            ),
            ("206 Partial Content", "Content-Length: 6\r\n", "456789"),
            (
                "206 Partial Content",
                "Content-Range: bytes 3-8/10\r\nContent-Length: 6\r\n",
                "345678",
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 4-8/10\r\nContent-Length: 5\r\n",
                "45678",
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 4-10/11\r\nContent-Length: 7\r\n",
                "4567890",
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 4-9/10\r\nContent-Length: 5\r\n",
                "45678",
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 4-9/*\r\nContent-Length: 6\r\n",
                "456789",
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 4-9/10\r\nContent-Length: 6\r\nETag: \"recording-v2\"\r\n",
                "456789",
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 4-9/10\r\nContent-Length: 6\r\n",
                "456789",
            ),
        ] {
            let (url, _, worker) = fixture(vec![initial(), response(status, headers, body)]);
            let client = client();
            collect(client.get(&url).await.unwrap()).await.unwrap();
            assert!(client.get_range(&url, 4, None).await.is_err(), "{headers}");
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn range_body_truncation_is_an_error_with_and_without_content_length() {
        for framing in ["Content-Length: 6\r\n", ""] {
            let headers =
                format!("Content-Range: bytes 4-9/10\r\nETag: \"recording-v1\"\r\n{framing}");
            let (url, _, worker) = fixture(vec![
                initial(),
                response("206 Partial Content", &headers, "45"),
            ]);
            let client = client();
            collect(client.get(&url).await.unwrap()).await.unwrap();
            let response = client.get_range(&url, 4, None).await.unwrap();
            let error = collect(response).await.unwrap_err().to_string();
            let classified = crate::playback_failure::classify(&error);
            assert!(classified.retryable);
            assert_eq!(
                classified.stage,
                crate::playback_failure::FailureStage::Transport
            );
            assert_eq!(
                classified.scope,
                crate::playback_failure::FailureScope::Source
            );
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn range_body_overflow_is_rejected_before_excess_bytes_are_returned() {
        let ranged = response(
            "206 Partial Content",
            "Content-Range: bytes 4-9/10\r\nETag: \"recording-v1\"\r\n",
            "456789extra",
        );
        let (url, _, worker) = fixture(vec![initial(), ranged]);
        let client = client();
        collect(client.get(&url).await.unwrap()).await.unwrap();
        assert!(collect(client.get_range(&url, 4, None).await.unwrap())
            .await
            .is_err());
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn last_modified_guards_reconnect_when_no_strong_etag_exists() {
        let modified = "Mon, 19 Oct 2026 12:00:00 GMT";
        let first = response(
            "200 OK",
            &format!("Content-Length: 10\r\nLast-Modified: {modified}\r\n"),
            "0123456789",
        );
        let ranged = response(
            "206 Partial Content",
            &format!("Content-Range: bytes 4-9/10\r\nLast-Modified: {modified}\r\n"),
            "456789",
        );
        let (url, requests, worker) = fixture(vec![first, ranged]);
        let client = client();
        collect(client.get(&url).await.unwrap()).await.unwrap();
        assert!(client.get(&url).await.is_err());
        assert_eq!(
            collect(client.get_range(&url, 4, None).await.unwrap())
                .await
                .unwrap(),
            b"456789"
        );
        worker.join().unwrap();
        assert!(requests.lock().unwrap()[1].contains(&format!("if-range: {modified}\r\n")));
    }

    #[tokio::test]
    async fn empty_media_and_connection_failures_retain_transport_retry_semantics() {
        for response in [
            String::new(),
            response("200 OK", "Content-Length: 0\r\n", ""),
        ] {
            let (url, _, worker) = fixture(vec![response]);
            let error = client().get(&url).await.err().unwrap().to_string();
            assert!(!error.contains("127.0.0.1"));
            let classified = crate::playback_failure::classify(&error);
            assert!(classified.retryable, "{error}");
            assert_eq!(
                classified.stage,
                crate::playback_failure::FailureStage::Transport
            );
            assert_eq!(
                classified.scope,
                crate::playback_failure::FailureScope::Source
            );
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn denied_initial_responses_preserve_only_status_for_recovery() {
        for status in [400, 401, 403, 404, 410, 416, 429, 500, 503] {
            let (url, _, worker) = fixture(vec![response(
                &format!("{status} Denied"),
                "Content-Length: 6\r\n",
                "secret",
            )]);
            let error = match client().get(&url).await {
                Ok(_) => panic!("denied response accepted"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains(&format!("HTTP {status}")));
            assert!(!error.contains("secret") && !error.contains(url.as_str()));
            let failure = crate::playback_failure::classify(&error);
            assert_eq!(failure.http_status, Some(status));
            assert!(failure.retryable, "{status}");
            assert!(!crate::playback_failure::should_skip_recording(&failure));
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn initial_partial_response_must_cover_the_whole_resource() {
        for (range, accepted) in [
            ("bytes 0-9/10", true),
            ("bytes 0-4/10", false),
            ("bytes 2-9/10", false),
        ] {
            let first = response(
                "206 Partial Content",
                &format!("Content-Range: {range}\r\nContent-Length: 10\r\n"),
                "0123456789",
            );
            let (url, _, worker) = fixture(vec![first]);
            assert_eq!(client().get(&url).await.is_ok(), accepted);
            worker.join().unwrap();
        }
    }

    #[test]
    fn content_range_parser_requires_finite_ordered_decimal_bounds() {
        for value in [
            "bytes 4-9/*",
            "bytes 9-4/10",
            "bytes 0-10/10",
            "bytes +0-9/10",
            "bytes 0-9/10/11",
            "bytes 0-9/18446744073709551616",
            "items 0-9/10",
            "bytes 0-0/0",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_RANGE, value.parse().unwrap());
            assert!(parse_content_range(&headers).is_err(), "{value}");
        }
    }

    #[tokio::test]
    async fn pending_range_request_can_be_cancelled_before_headers_arrive() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/audio", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let (release, released) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                if index == 0 {
                    socket.write_all(initial().as_bytes()).unwrap();
                } else {
                    // Keep headers pending until the caller drops the request future.
                    let _ = released.recv_timeout(Duration::from_secs(3));
                }
            }
        });
        let client = client();
        collect(client.get(&url).await.unwrap()).await.unwrap();
        let timed =
            tokio::time::timeout(Duration::from_millis(100), client.get_range(&url, 4, None)).await;
        assert!(timed.is_err());
        release.send(()).unwrap();
        worker.join().unwrap();
    }

    #[derive(Debug, PartialEq, Eq)]
    enum SeekCancellationOutcome {
        HandshakeObservedAndCancelled,
        MissingHandshakeCleanedUp,
    }

    async fn join_origin_bounded(
        origin_result: &mut tokio::sync::oneshot::Receiver<bool>,
        origin_thread: &mut Option<thread::JoinHandle<()>>,
    ) -> bool {
        let Ok(Ok(origin_ok)) = tokio::time::timeout(Duration::from_secs(3), origin_result).await
        else {
            return false;
        };
        if let Some(origin_thread) = origin_thread.take() {
            origin_thread.join().unwrap();
        }
        origin_ok
    }

    async fn exercise_seek_cancellation(withhold_handshake: bool) -> SeekCancellationOutcome {
        use std::io::{Seek, SeekFrom};
        use stream_download::{
            http::HttpStream, storage::memory::MemoryStorageProvider, Settings, StreamDownload,
        };

        // The initial reader is usable. Only its subsequent uncached seek is
        // held at response headers, where the download task must remain cancellable.
        let client = NativeHttpClient::new(
            reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/audio", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let (request_received, pending_request) = tokio::sync::oneshot::channel();
        let (range_seen, pending_range) = tokio::sync::oneshot::channel();
        let (origin_finished, mut origin_result) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let origin = thread::spawn(move || {
            let origin_ok = (|| {
                let accept_until = |listener: &TcpListener,
                                    released: &std::sync::mpsc::Receiver<()>|
                 -> std::io::Result<Option<TcpStream>> {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    loop {
                        match released.try_recv() {
                            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                return Ok(None);
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                        match listener.accept() {
                            Ok((socket, _)) => {
                                socket.set_nonblocking(false)?;
                                return Ok(Some(socket));
                            }
                            Err(error)
                                if error.kind() == std::io::ErrorKind::WouldBlock
                                    && Instant::now() < deadline =>
                            {
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                };
                let read_request = |socket: &mut std::net::TcpStream| {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        socket.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                        assert!(request.len() < 16384);
                    }
                    String::from_utf8(request).unwrap()
                };
                let mut initial = accept_until(&listener, &released)
                    .unwrap()
                    .expect("origin released before initial request");
                read_request(&mut initial);
                initial.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 65536\r\nAccept-Ranges: bytes\r\nETag: \"recording-v1\"\r\nConnection: keep-alive\r\n\r\n0123456789abcdef").unwrap();
                initial.flush().unwrap();
                let Some(mut ranged) = accept_until(&listener, &released).unwrap() else {
                    return false;
                };
                let request = read_request(&mut ranged).to_ascii_lowercase();
                assert!(
                    request.contains("range: bytes=32768-65535\r\n"),
                    "{request}"
                );
                assert!(request.contains("if-range: \"recording-v1\"\r\n"));
                request_received.send(()).unwrap();
                if !withhold_handshake {
                    range_seen.send(()).unwrap();
                }
                // Deliberately send no range response headers. Release is cleanup,
                // never the mechanism that should wake the cancelled seek. The
                // negative fixture mode also withholds the test handshake signal.
                let _ = released.recv_timeout(Duration::from_secs(10));
                ranged
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut byte = [0];
                match ranged.read(&mut byte) {
                    Ok(0) => true,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        true
                    }
                    _ => false,
                }
            })();
            let _ = origin_finished.send(origin_ok);
        });
        // Match native playback: connection establishment is bounded, but there
        // is no response/read timeout that could conceal a broken cancellation path.
        let stream = match HttpStream::new(client, url).await {
            Ok(stream) => stream,
            Err(error) => {
                let _ = release.send(());
                let mut origin_thread = Some(origin);
                let cleanup_finished =
                    join_origin_bounded(&mut origin_result, &mut origin_thread).await;
                panic!(
                    "initial HTTP stream setup failed ({error}); fixture_cleanup={cleanup_finished}"
                );
            }
        };
        let reader = match StreamDownload::from_stream(
            stream,
            MemoryStorageProvider,
            Settings::default()
                .prefetch_bytes(0)
                .retry_timeout(Duration::from_secs(30)),
        )
        .await
        {
            Ok(reader) => reader,
            Err(error) => {
                let _ = release.send(());
                let mut origin_thread = Some(origin);
                let cleanup_finished =
                    join_origin_bounded(&mut origin_result, &mut origin_thread).await;
                panic!("stream reader setup failed ({error}); fixture_cleanup={cleanup_finished}");
            }
        };
        let cancel = reader.cancellation_token();
        let (seek_done, mut seek_result) = tokio::sync::oneshot::channel();
        let seek_worker = thread::spawn(move || {
            let mut reader = reader;
            let mut prefix = [0; 8];
            let result = reader.read_exact(&mut prefix).and_then(|()| {
                if &prefix != b"01234567" {
                    return Err(std::io::Error::other("initial response bytes mismatch"));
                }
                reader.seek(SeekFrom::Start(32768))
            });
            let _ = seek_done.send(result);
        });
        let request_observed = tokio::time::timeout(Duration::from_secs(10), pending_request)
            .await
            .is_ok_and(|result| result.is_ok());
        if !request_observed {
            cancel.cancel();
            let _ = release.send(());
            let _ = tokio::time::timeout(Duration::from_secs(2), &mut seek_result).await;
            let mut origin_thread = Some(origin);
            let _ = join_origin_bounded(&mut origin_result, &mut origin_thread).await;
            panic!("decoder did not issue its uncached range request within 10 seconds");
        }
        let handshake_observed = tokio::time::timeout(
            if withhold_handshake {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(10)
            },
            pending_range,
        )
        .await
        .is_ok_and(|result| result.is_ok());
        if !handshake_observed {
            // This is the same cleanup path used for a real missing handshake:
            // cancel the reader, release the origin, and bound both joins.
            cancel.cancel();
            let _ = release.send(());
            let seek_finished =
                tokio::time::timeout(Duration::from_millis(500), &mut seek_result).await;
            let seek_result = seek_finished
                .expect("cancelled seek did not finish within 500 ms after missing handshake")
                .expect("seek worker terminated before reporting its result");
            seek_worker.join().unwrap();
            assert!(
                seek_result.is_err(),
                "cancelled seek must not claim the missing bytes are ready"
            );
            let mut origin_thread = Some(origin);
            assert!(
                join_origin_bounded(&mut origin_result, &mut origin_thread).await,
                "cleanup must release the fixture and drop the pending range request"
            );
            assert!(
                withhold_handshake,
                "range request was observed but the required test handshake was missing"
            );
            return SeekCancellationOutcome::MissingHandshakeCleanedUp;
        }
        cancel.cancel();
        let promptly_finished =
            tokio::time::timeout(Duration::from_millis(500), &mut seek_result).await;
        // Release the origin in both success and timeout paths so the fixture
        // cannot hide a cancellation failure behind its own stalled worker.
        release.send(()).unwrap();
        let seek_result = match promptly_finished {
            Ok(Ok(result)) => result,
            Err(_) => {
                let _ = tokio::time::timeout(Duration::from_secs(2), &mut seek_result).await;
                let mut origin_thread = Some(origin);
                let _ = join_origin_bounded(&mut origin_result, &mut origin_thread).await;
                panic!(
                    "StreamDownload cancellation did not wake the seek while range headers were pending"
                );
            }
            Ok(Err(error)) => panic!("seek worker terminated before reporting its result: {error}"),
        };
        seek_worker.join().unwrap();
        assert!(
            seek_result.is_err(),
            "cancelled seek must not claim the missing bytes are ready"
        );
        let mut origin_thread = Some(origin);
        assert!(
            join_origin_bounded(&mut origin_result, &mut origin_thread).await,
            "cancellation must drop the pending HTTP range request"
        );
        assert!(
            !withhold_handshake,
            "fixture unexpectedly reported a handshake that it was instructed to withhold"
        );
        SeekCancellationOutcome::HandshakeObservedAndCancelled
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_download_cancellation_interrupts_seek_waiting_for_range_headers() {
        assert_eq!(
            exercise_seek_cancellation(false).await,
            SeekCancellationOutcome::HandshakeObservedAndCancelled
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_range_handshake_cleanup_cancels_seek_and_releases_fixture() {
        // The origin confirms the real uncached range request but withholds the
        // test handshake. This exercises the failed-gate cleanup path directly.
        assert_eq!(
            exercise_seek_cancellation(true).await,
            SeekCancellationOutcome::MissingHandshakeCleanedUp
        );
    }
}
