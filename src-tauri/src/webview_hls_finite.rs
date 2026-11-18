use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

const LOOK_AHEAD_SECONDS: f64 = 30.0;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SEGMENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_BUFFERED_SEGMENTS: usize = 16;
const MAX_INDEXED_RENDITIONS: usize = 8;

#[derive(Clone, Default)]
pub(super) struct FiniteManifests {
    playlists: HashMap<url::Url, Arc<Playlist>>,
    master_layouts: HashMap<url::Url, [u8; 32]>,
    window: Option<Arc<Window>>,
}

struct Playlist {
    // The first entry below the root, through the selected media playlist.
    lineage: Vec<(url::Url, [u8; 32])>,
    layout: [u8; 32],
    segments: Vec<Segment>,
    // Before a refresh, changing initialization bytes would require resetting
    // the decoder. Plain segment playback can still use the reserve.
    can_refresh: bool,
}

struct Segment {
    id: String,
    url: url::Url,
    seconds: f64,
}

struct ManifestLayout {
    fingerprint: [u8; 32],
    segments: Vec<(url::Url, f64)>,
    references: Vec<url::Url>,
    map_present: bool,
}

fn stable_locator(url: &url::Url) -> String {
    let mut locator = url.clone();
    // A query can identify a rendition or segment. Only known authentication
    // fields can change while preserving the identity of the media resource.
    let identity_query = url.query().map(|query| {
        query
            .split('&')
            .filter(|parameter| {
                !matches!(
                    parameter
                        .split('=')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .as_str(),
                    "signature"
                        | "sig"
                        | "token"
                        | "expire"
                        | "expires"
                        | "policy"
                        | "key-pair-id"
                        | "x-amz-credential"
                        | "x-amz-date"
                        | "x-amz-expires"
                        | "x-amz-signature"
                        | "x-amz-security-token"
                )
            })
            .collect::<Vec<_>>()
            .join("&")
    });
    locator.set_query(identity_query.as_deref().filter(|query| !query.is_empty()));
    locator.set_fragment(None);
    locator.to_string()
}

fn describe(text: &str, url: &url::Url, root: &url::Url) -> Result<ManifestLayout, String> {
    if text.len() > MAX_MANIFEST || !text.trim_start_matches('\u{feff}').starts_with("#EXTM3U") {
        return Err("Invalid HLS refresh manifest".into());
    }
    let mut references = Vec::new();
    let mut reference_bytes = 0usize;
    let mut fingerprint = Sha256::new();
    let mut segments = Vec::new();
    let mut seconds = None;
    let mut map_present = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(duration) = line.strip_prefix("#EXTINF:") {
            seconds = duration
                .split(',')
                .next()
                .and_then(|value| value.parse::<f64>().ok());
        }
        if line.starts_with("#EXT-X-MAP:") {
            map_present = true;
        }
        if line.starts_with("#EXT-X-BYTERANGE:")
            || line.starts_with("#EXT-X-DEFINE:")
            || line.starts_with("#EXT-X-CONTENT-STEERING:")
            || line.starts_with("#EXT-X-PART:")
            || line.starts_with("#EXT-X-PRELOAD-HINT:")
            || line.starts_with("#EXT-X-RENDITION-REPORT:")
            || (line.starts_with("#EXT-X-KEY:") || line.starts_with("#EXT-X-SESSION-KEY:"))
                && line != "#EXT-X-KEY:METHOD=NONE"
        {
            return Err("HLS manifest requires its original relay path".into());
        }
        let mut register = |raw: &str| -> Result<String, String> {
            let target = child_url(url, raw, false, root)?;
            let size = target.as_str().len();
            reference_bytes = reference_bytes.saturating_add(size);
            if size > MAX_URI
                || reference_bytes > MAX_URL_BYTES
                || references.len() >= MAX_RESOURCES
            {
                return Err("HLS refresh reference limit exceeded".into());
            }
            let locator = stable_locator(&target);
            references.push(target);
            Ok(locator)
        };
        let normalized = if line.starts_with('#') {
            rewrite_attributes(line, &mut register)?
        } else if line.is_empty() {
            String::new()
        } else {
            let locator = register(line)?;
            if let Some(seconds) = seconds
                .take()
                .filter(|value| value.is_finite() && *value > 0.0)
            {
                segments.push((
                    references.last().expect("registered reference").clone(),
                    seconds,
                ));
            }
            locator
        };
        fingerprint.update(normalized.as_bytes());
        fingerprint.update(b"\n");
    }
    Ok(ManifestLayout {
        fingerprint: fingerprint.finalize().into(),
        segments,
        references,
        map_present,
    })
}

fn is_end_list(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "#EXT-X-ENDLIST")
}

pub(super) fn index_manifest(graph: &mut Graph, text: &str, url: &url::Url, root: &url::Url) {
    let finite = is_end_list(text);
    if !finite {
        let master = text
            .lines()
            .any(|line| line.trim().starts_with("#EXT-X-STREAM-INF:"));
        if master
            && (graph.finite.master_layouts.contains_key(url)
                || graph.finite.master_layouts.len() < MAX_INDEXED_RENDITIONS)
        {
            if let Ok(layout) = describe(text, url, root) {
                graph
                    .finite
                    .master_layouts
                    .insert(url.clone(), layout.fingerprint);
            }
        }
        return;
    }
    if !graph.finite.playlists.contains_key(url)
        && graph.finite.playlists.len() >= MAX_INDEXED_RENDITIONS
    {
        return;
    }
    let Ok(layout) = describe(text, url, root) else {
        return;
    };
    let mut lineage = Vec::new();
    let mut current = url;
    while current != root {
        if lineage.len() >= MAX_DEPTH {
            return;
        }
        let Some(parent) = graph
            .entries
            .values()
            .find(|entry| entry.url == *current)
            .and_then(|entry| entry.parent.as_deref())
        else {
            return;
        };
        let Some(parent_layout) = graph.finite.master_layouts.get(parent) else {
            return;
        };
        lineage.push((current.clone(), *parent_layout));
        current = parent;
    }
    lineage.reverse();
    let by_url: HashMap<_, _> = graph
        .entries
        .iter()
        .map(|(id, entry)| (&entry.url, id))
        .collect();
    let mut unique = std::collections::HashSet::new();
    let mut segments = Vec::with_capacity(layout.segments.len());
    for (url, seconds) in layout.segments {
        let Some(id) = by_url.get(&url) else {
            return;
        };
        if !unique.insert((*id).clone()) {
            // Repeated resources need range or per-occurrence semantics.
            return;
        }
        segments.push(Segment {
            id: (*id).clone(),
            url,
            seconds,
        });
    }
    if segments.is_empty() {
        return;
    }
    if graph.finite.playlists.get(url).is_some_and(|playlist| {
        playlist.layout == layout.fingerprint
            && playlist
                .segments
                .iter()
                .map(|s| &s.url)
                .eq(segments.iter().map(|s| &s.url))
    }) {
        return;
    }
    graph.finite.playlists.insert(
        url.clone(),
        Arc::new(Playlist {
            lineage,
            layout: layout.fingerprint,
            segments,
            can_refresh: !layout.map_present,
        }),
    );
}

struct Window {
    playlist: Arc<Playlist>,
    cursor: AtomicUsize,
    worker: AtomicBool,
    cells: Mutex<HashMap<usize, Arc<SegmentCell>>>,
    bytes: Arc<AtomicUsize>,
    fresh: AsyncMutex<Option<FreshRendition>>,
    stop: watch::Sender<bool>,
}

type SegmentCell = OnceCell<Result<CachedSegment, SegmentError>>;

impl Window {
    fn new(playlist: Arc<Playlist>, cursor: usize, bytes: Arc<AtomicUsize>) -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            playlist,
            cursor: AtomicUsize::new(cursor),
            worker: AtomicBool::new(false),
            cells: Mutex::new(HashMap::new()),
            bytes,
            fresh: AsyncMutex::new(None),
            stop,
        }
    }

    fn advance(&self, cursor: usize) -> Result<(), SegmentError> {
        let old = self.cursor.fetch_max(cursor, Ordering::Relaxed);
        if cursor > old {
            self.cells
                .lock()
                .map_err(|_| SegmentError::Bypass)?
                .retain(|index, _| *index >= cursor.saturating_sub(1));
        }
        Ok(())
    }

    fn cell(&self, index: usize) -> Result<Arc<SegmentCell>, SegmentError> {
        let mut cells = self.cells.lock().map_err(|_| SegmentError::Bypass)?;
        let cursor = self.cursor.load(Ordering::Relaxed);
        if index < cursor.saturating_sub(1) || index >= cursor.saturating_add(MAX_BUFFERED_SEGMENTS)
        {
            return Err(SegmentError::Bypass);
        }
        Ok(cells.entry(index).or_default().clone())
    }

    fn read_ahead_limit(&self, cursor: usize) -> usize {
        let mut seconds = 0.0;
        let mut end = cursor;
        for segment in self
            .playlist
            .segments
            .iter()
            .skip(cursor)
            .take(MAX_BUFFERED_SEGMENTS)
        {
            seconds += segment.seconds;
            end += 1;
            if seconds >= LOOK_AHEAD_SECONDS {
                break;
            }
        }
        end
    }

    async fn load(&self, session: &Session, index: usize) -> Result<CachedSegment, SegmentError> {
        let cell = self.cell(index)?;
        let mut stop = self.stop.subscribe();
        let mut session_stop = session.stop.subscribe();
        if *stop.borrow() || *session_stop.borrow() {
            return Err(SegmentError::Cancelled);
        }
        let fetch = cell.get_or_init(|| self.fetch_segment(session, index));
        tokio::select! {
            biased;
            _ = stop.changed() => Err(SegmentError::Cancelled),
            _ = session_stop.changed() => Err(SegmentError::Cancelled),
            result = fetch => result.clone(),
        }
    }

    async fn fetch_segment(
        &self,
        session: &Session,
        index: usize,
    ) -> Result<CachedSegment, SegmentError> {
        let existing = self.fresh.lock().await.clone();
        let (target, root, headers) = existing.as_ref().map_or_else(
            || {
                (
                    &self.playlist.segments[index].url,
                    &session.source,
                    &session.headers,
                )
            },
            |fresh| (&fresh.segments[index], &fresh.root, &fresh.headers),
        );
        let fetch = || fetch_segment_body(session, target, root, headers, self.bytes.clone());
        let first = fetch().await;
        let result = match first {
            Err(SegmentError::Transient) => fetch().await,
            otherwise => otherwise,
        };
        if !matches!(&result, Err(SegmentError::Denied)) || !self.playlist.can_refresh {
            return result;
        }

        // One refresh for concurrent misses. Publish only when the same
        // rendition and media timeline have both been checked.
        let latest = {
            let mut fresh = self.fresh.lock().await;
            if fresh.as_ref().map(|entry| entry.id) == existing.as_ref().map(|entry| entry.id) {
                *fresh = Some(self.refresh(session).await?);
            }
            fresh.clone().ok_or(SegmentError::Invalid)?
        };
        fetch_segment_body(
            session,
            &latest.segments[index],
            &latest.root,
            &latest.headers,
            self.bytes.clone(),
        )
        .await
    }

    async fn refresh(&self, session: &Session) -> Result<FreshRendition, SegmentError> {
        let refresh = session.refresh.as_ref().ok_or(SegmentError::Denied)?;
        let budget = crate::playback_policy::RECOVERY_BUDGET;
        let stop = self.stop.subscribe();
        let work = async {
            let replacement = refresh(PlaybackDeadline::after(budget), stop)
                .await
                .map_err(|_| SegmentError::Denied)?;
            if !super::super::is_hls(&replacement) || replacement.cenc.is_some() {
                return Err(SegmentError::Invalid);
            }
            let root = refreshed_root(session, &replacement.input)?;
            let headers = refreshed_headers(&replacement.headers)?;
            let mut target = root.clone();
            for (expected, expected_layout) in &self.playlist.lineage {
                let text = fetch_manifest(session, &target, &root, &headers).await?;
                let layout = describe(&text, &target, &root).map_err(|_| SegmentError::Invalid)?;
                if layout.fingerprint != *expected_layout {
                    return Err(SegmentError::Invalid);
                }
                let mut matches = layout
                    .references
                    .into_iter()
                    .filter(|candidate| stable_locator(candidate) == stable_locator(expected));
                let found = matches.next().ok_or(SegmentError::Invalid)?;
                if matches.any(|candidate| candidate != found) {
                    return Err(SegmentError::Invalid);
                }
                target = found;
            }
            let text = fetch_manifest(session, &target, &root, &headers).await?;
            let layout = describe(&text, &target, &root).map_err(|_| SegmentError::Invalid)?;
            if !is_end_list(&text)
                || layout.fingerprint != self.playlist.layout
                || layout.segments.len() != self.playlist.segments.len()
            {
                return Err(SegmentError::Invalid);
            }
            Ok(FreshRendition {
                id: uuid::Uuid::new_v4(),
                root,
                headers,
                segments: layout.segments.into_iter().map(|(url, _)| url).collect(),
            })
        };
        tokio::time::timeout(budget, work)
            .await
            .map_err(|_| SegmentError::Denied)?
    }
}

#[derive(Clone)]
struct FreshRendition {
    id: uuid::Uuid,
    root: url::Url,
    headers: HeaderMap,
    segments: Vec<url::Url>,
}

#[derive(Clone)]
struct CachedSegment {
    bytes: axum::body::Bytes,
    mime: Option<HeaderValue>,
}

#[derive(Clone, Copy, Debug)]
enum SegmentError {
    Bypass,
    Cancelled,
    Denied,
    Invalid,
    Transient,
}

struct SegmentBytes {
    bytes: Vec<u8>,
    budget: Arc<AtomicUsize>,
}

fn refreshed_root(session: &Session, raw: &str) -> Result<url::Url, SegmentError> {
    #[cfg(test)]
    if session.source.host_str() == Some("127.0.0.1") {
        if let Ok(url) = url::Url::parse(raw) {
            if url.origin() == session.source.origin() {
                return Ok(url);
            }
        }
    }
    let _ = session;
    super::super::validated_url(raw, false).map_err(|_| SegmentError::Invalid)
}
impl AsRef<[u8]> for SegmentBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for SegmentBytes {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes.len(), Ordering::Relaxed);
    }
}
impl SegmentBytes {
    fn append(&mut self, chunk: &[u8]) -> Result<(), SegmentError> {
        if self.bytes.len().saturating_add(chunk.len()) > MAX_SEGMENT_BYTES {
            return Err(SegmentError::Bypass);
        }
        self.budget
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bytes| {
                bytes
                    .checked_add(chunk.len())
                    .filter(|&total| total <= MAX_CACHE_BYTES)
            })
            .map_err(|_| SegmentError::Bypass)?;
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }
}

fn refreshed_headers(raw: &[(String, String)]) -> Result<HeaderMap, SegmentError> {
    let mut headers = HeaderMap::new();
    for (name, value) in raw {
        if [
            "authorization",
            "cookie",
            "origin",
            "referer",
            "user-agent",
            "accept-language",
        ]
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
        {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| SegmentError::Invalid)?,
                HeaderValue::from_str(value).map_err(|_| SegmentError::Invalid)?,
            );
        }
    }
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    Ok(headers)
}

async fn request(
    session: &Session,
    target: &url::Url,
    root: &url::Url,
    headers: &HeaderMap,
) -> Result<reqwest::Response, SegmentError> {
    let work = async {
        let client = if target.origin() == session.source.origin() {
            session.client.clone()
        } else {
            public_client(target)
                .await
                .map_err(|_| SegmentError::Transient)?
        };
        let mut headers = headers.clone();
        if target.origin() != root.origin() {
            for name in [
                header::AUTHORIZATION,
                header::COOKIE,
                header::ORIGIN,
                header::REFERER,
            ] {
                headers.remove(name);
            }
        }
        let reply = client
            .get(target.clone())
            .headers(headers)
            .send()
            .await
            .map_err(|_| SegmentError::Transient)?;
        match reply.status().as_u16() {
            200 => Ok(reply),
            403 => Err(SegmentError::Denied),
            408 | 429 | 500..=599 => Err(SegmentError::Transient),
            _ => Err(SegmentError::Invalid),
        }
    };
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .map_err(|_| SegmentError::Transient)?
}

async fn fetch_manifest(
    session: &Session,
    target: &url::Url,
    root: &url::Url,
    headers: &HeaderMap,
) -> Result<String, SegmentError> {
    let response = request(session, target, root, headers).await?;
    let mut stream = response.bytes_stream();
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| SegmentError::Transient)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST {
                return Err(SegmentError::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).map_err(|_| SegmentError::Invalid)
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .map_err(|_| SegmentError::Transient)?
}

async fn fetch_segment_body(
    session: &Session,
    target: &url::Url,
    root: &url::Url,
    headers: &HeaderMap,
    budget: Arc<AtomicUsize>,
) -> Result<CachedSegment, SegmentError> {
    let response = request(session, target, root, headers).await?;
    if response.headers().contains_key(header::CONTENT_RANGE)
        || response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|value| value != "identity")
    {
        return Err(SegmentError::Invalid);
    }
    let mime = response.headers().get(header::CONTENT_TYPE).cloned();
    if let Some(mime) = mime.as_ref() {
        let mime = mime
            .to_str()
            .map_err(|_| SegmentError::Invalid)?
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !(mime.starts_with("audio/")
            || matches!(
                mime.as_str(),
                "video/mp4"
                    | "video/mp2t"
                    | "video/webm"
                    | "application/octet-stream"
                    | "binary/octet-stream"
            ))
        {
            return Err(SegmentError::Invalid);
        }
    }
    let expected = response.content_length();
    if expected.is_some_and(|size| size > MAX_SEGMENT_BYTES as u64) {
        return Err(SegmentError::Bypass);
    }
    if expected.is_none()
        && !matches!(
            response.version(),
            reqwest::Version::HTTP_2 | reqwest::Version::HTTP_3
        )
        && response
            .headers()
            .get(header::TRANSFER_ENCODING)
            .is_none_or(|value| value != "chunked")
    {
        // A connection-delimited body cannot prove a clean segment end.
        return Err(SegmentError::Bypass);
    }
    let mut stream = response.bytes_stream();
    let read = async {
        let mut bytes = SegmentBytes {
            bytes: Vec::new(),
            budget,
        };
        while let Some(part) = stream.next().await {
            bytes.append(&part.map_err(|_| SegmentError::Transient)?)?;
        }
        if bytes.bytes.is_empty() || expected.is_some_and(|size| size != bytes.bytes.len() as u64) {
            return Err(SegmentError::Transient);
        }
        Ok(CachedSegment {
            bytes: axum::body::Bytes::from_owner(bytes),
            mime,
        })
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .map_err(|_| SegmentError::Transient)?
}

fn window_for(graph: &mut Graph, resource: &str) -> Option<(Arc<Window>, usize)> {
    let (playlist, cursor) = graph.finite.playlists.values().find_map(|playlist| {
        playlist
            .segments
            .iter()
            .position(|segment| segment.id == resource)
            .map(|cursor| (playlist.clone(), cursor))
    })?;
    if let Some(window) = graph.finite.window.as_ref() {
        let current = window.cursor.load(Ordering::Relaxed);
        if Arc::ptr_eq(&window.playlist, &playlist)
            && cursor >= current.saturating_sub(1)
            && cursor <= current.saturating_add(1)
        {
            window.advance(cursor).ok()?;
            return Some((window.clone(), cursor));
        }
    }
    let budget = graph
        .finite
        .window
        .take()
        .map(|old| {
            old.stop.send_replace(true);
            old.bytes.clone()
        })
        .unwrap_or_default();
    let window = Arc::new(Window::new(playlist, cursor, budget));
    graph.finite.window = Some(window.clone());
    Some((window, cursor))
}

fn prefetch(window: Arc<Window>, session: Arc<Session>) {
    if window.worker.swap(true, Ordering::Relaxed) {
        return;
    }
    tokio::spawn(async move {
        let mut next = window.cursor.load(Ordering::Relaxed) + 1;
        loop {
            let cursor = window.cursor.load(Ordering::Relaxed);
            next = next.max(cursor + 1);
            if next >= window.read_ahead_limit(cursor) || window.load(&session, next).await.is_err()
            {
                break;
            }
            next += 1;
        }
        window.worker.store(false, Ordering::Relaxed);
    });
}

pub(in super::super) async fn buffered_segment(
    session: &Arc<Session>,
    resource: Option<&str>,
    headers: &HeaderMap,
    method: &Method,
) -> Option<Response<Body>> {
    if *method != Method::GET {
        return None;
    }
    let ranged = match headers.get(header::RANGE) {
        None => false,
        Some(range) if range == "bytes=0-" => true,
        Some(_) => return None,
    };
    let resource = resource?;
    let (window, cursor) = window_for(&mut *session.hls.lock().ok()?, resource)?;
    let segment = match window.load(session, cursor).await {
        Ok(segment) => segment,
        Err(SegmentError::Bypass) => return None,
        Err(SegmentError::Cancelled) => {
            return Some(
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::empty())
                    .unwrap(),
            )
        }
        Err(_) => return Some(error("HLS segment could not be recovered")),
    };
    prefetch(window, session.clone());
    let mut builder = Response::builder()
        .status(if ranged {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_LENGTH, segment.bytes.len())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if ranged {
        let length = segment.bytes.len();
        builder = builder
            .header(
                header::CONTENT_RANGE,
                format!("bytes 0-{}/{length}", length - 1),
            )
            .header(header::ACCEPT_RANGES, "bytes");
    }
    if let Some(mime) = segment.mime {
        builder = builder.header(header::CONTENT_TYPE, mime);
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        builder = builder
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
            .header(header::VARY, "Origin")
            .header(
                header::ACCESS_CONTROL_EXPOSE_HEADERS,
                "Content-Range, Accept-Ranges, Content-Length",
            );
    }
    Some(builder.body(Body::from(segment.bytes)).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::OriginalUri, routing::get};
    use std::net::Ipv4Addr;

    fn manifest(signature: &str, sequence: u64) -> String {
        format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n#EXTINF:6,\nfirst.ts?signature={signature}\n#EXTINF:6,\nsecond.ts?signature={signature}\n#EXTINF:6,\nthird.ts?signature={signature}\n#EXTINF:6,\nfourth.ts?signature={signature}\n#EXT-X-ENDLIST\n"
        )
    }

    #[test]
    fn continuity_matches_rotated_signature_and_rejects_timeline_or_path_change() {
        let root = url::Url::parse("https://recordings.example/album.m3u8").unwrap();
        let original = describe(&manifest("old", 5), &root, &root).unwrap();
        let refreshed = describe(&manifest("new", 5), &root, &root).unwrap();
        assert_eq!(original.fingerprint, refreshed.fingerprint);
        assert_ne!(original.segments, refreshed.segments);
        for changed in [
            manifest("new", 6),
            manifest("new", 5).replace("#EXTINF:6,", "#EXTINF:7,"),
            manifest("new", 5).replace("second.ts", "other.ts"),
            manifest("new", 5).replace("second.ts?signature=", "second.ts?format=4&signature="),
        ] {
            assert_ne!(
                original.fingerprint,
                describe(&changed, &root, &root).unwrap().fingerprint
            );
        }
        let range = manifest("new", 5).replace("second.ts", "#EXT-X-BYTERANGE:10@0\nsecond.ts");
        assert!(describe(&range, &root, &root).is_err());
    }

    #[test]
    fn live_manifests_are_never_indexed_and_cache_leases_release_the_budget() {
        let root = url::Url::parse("https://recordings.example/radio.m3u8").unwrap();
        let mut graph = Graph::default();
        let live = manifest("old", 8).replace("#EXT-X-ENDLIST", "");
        index_manifest(&mut graph, &live, &root, &root);
        assert!(graph.finite.playlists.is_empty());

        let budget = Arc::new(AtomicUsize::new(MAX_CACHE_BYTES - 1));
        let mut bytes = SegmentBytes {
            bytes: Vec::new(),
            budget: budget.clone(),
        };
        assert!(matches!(
            bytes.append(b"too much"),
            Err(SegmentError::Bypass)
        ));
        bytes.append(b"x").unwrap();
        let bytes = axum::body::Bytes::from_owner(bytes);
        let leased = bytes.clone();
        drop(bytes);
        assert_eq!(budget.load(Ordering::Relaxed), MAX_CACHE_BYTES);
        drop(leased);
        assert_eq!(budget.load(Ordering::Relaxed), MAX_CACHE_BYTES - 1);
    }

    #[tokio::test]
    async fn segment_prefetch_recovers_repeated_403_and_bounds_permanent_denial() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let good_segment_requests = Arc::new(AtomicUsize::new(0));
        let permanent_denials = Arc::new(AtomicUsize::new(0));
        let app = {
            let attempts = attempts.clone();
            let good_segment_requests = good_segment_requests.clone();
            let permanent_denials = permanent_denials.clone();
            Router::new().fallback_service(get(move |OriginalUri(uri): OriginalUri| {
                let attempts = attempts.clone();
                let good_segment_requests = good_segment_requests.clone();
                let permanent_denials = permanent_denials.clone();
                async move {
                    match (uri.path(), uri.query()) {
                        ("/album.m3u8", Some(query)) => {
                            let signature = query.strip_prefix("signature=").unwrap();
                            Response::new(Body::from(manifest(signature, 5)))
                        }
                        ("/first.ts", _) if attempts.fetch_add(1, Ordering::Relaxed) == 0 => {
                            Response::builder()
                                .status(StatusCode::SERVICE_UNAVAILABLE)
                                .body(Body::empty())
                                .unwrap()
                        }
                        ("/first.ts", _) => Response::new(Body::from("first")),
                        ("/second.ts", Some("signature=new")) => {
                            good_segment_requests.fetch_add(1, Ordering::Relaxed);
                            Response::new(Body::from("second"))
                        }
                        ("/third.ts", Some("signature=next")) => {
                            good_segment_requests.fetch_add(1, Ordering::Relaxed);
                            Response::new(Body::from("third"))
                        }
                        ("/fourth.ts", _) => {
                            permanent_denials.fetch_add(1, Ordering::Relaxed);
                            Response::builder()
                                .status(StatusCode::FORBIDDEN)
                                .body(Body::empty())
                                .unwrap()
                        }
                        ("/second.ts" | "/third.ts", _) => Response::builder()
                            .status(StatusCode::FORBIDDEN)
                            .body(Body::empty())
                            .unwrap(),
                        _ => Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::empty())
                            .unwrap(),
                    }
                }
            }))
        };
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let root = url::Url::parse(&format!("http://{address}/album.m3u8?signature=old")).unwrap();

        let refreshes = Arc::new(AtomicUsize::new(0));
        let refresh = {
            let refreshes = refreshes.clone();
            Arc::new(move |_, _| -> futures_util::future::BoxFuture<'static, Result<AudioSource, String>> {
                let attempt = refreshes.fetch_add(1, Ordering::Relaxed);
                let signature = ["new", "next", "last"].get(attempt).copied().unwrap_or("unexpected");
                let input = format!("http://{address}/album.m3u8?signature={signature}");
                Box::pin(async move {
                    Ok(AudioSource { input, ffmpeg: Default::default(), headers: Vec::new(), decoder_hint: None, cenc: None })
                })
            }) as super::super::super::NativeSourceRefresh
        };
        let mut graph = Graph::default();
        let text = manifest("old", 5);
        rewrite_manifest(&text, &root, 0, &root, false, &mut graph, "local", "token").unwrap();
        index_manifest(&mut graph, &text, &root, &root);
        let playlist = graph.finite.playlists.get(&root).unwrap();
        let first = playlist.segments[0].id.clone();
        let second = playlist.segments[1].id.clone();
        let third = playlist.segments[2].id.clone();
        let fourth = playlist.segments[3].id.clone();
        let (stop, _) = watch::channel(false);
        let session = Arc::new(Session {
            id: "fixture".into(),
            token: "fixture".into(),
            generation: 0,
            _track_id: "fixture".into(),
            source: root,
            headers: HeaderMap::new(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            stop,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
            hls: Mutex::new(graph),
            allow_radio_http: false,
            progressive: None,
            refresh: Some(refresh),
        });
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-"));
        let initial = buffered_segment(&session, Some(&first), &headers, &Method::GET)
            .await
            .unwrap();
        assert_eq!(initial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(initial.headers()[header::CONTENT_RANGE], "bytes 0-4/5");
        assert_eq!(
            axum::body::to_bytes(initial.into_body(), 64).await.unwrap(),
            "first"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let window = session.hls.lock().unwrap().finite.window.clone().unwrap();
                if window.cell(3).unwrap().initialized() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(refreshes.load(Ordering::Relaxed), 3);
        let next = buffered_segment(&session, Some(&second), &headers, &Method::GET)
            .await
            .unwrap();
        assert_eq!(next.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            axum::body::to_bytes(next.into_body(), 64).await.unwrap(),
            "second"
        );
        let next = buffered_segment(&session, Some(&third), &headers, &Method::GET)
            .await
            .unwrap();
        assert_eq!(next.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            axum::body::to_bytes(next.into_body(), 64).await.unwrap(),
            "third"
        );
        for _ in 0..2 {
            let denied = buffered_segment(&session, Some(&fourth), &headers, &Method::GET)
                .await
                .unwrap();
            assert_eq!(denied.status(), StatusCode::BAD_GATEWAY);
        }
        assert_eq!(refreshes.load(Ordering::Relaxed), 3);
        assert_eq!(permanent_denials.load(Ordering::Relaxed), 2);
        assert_eq!(good_segment_requests.load(Ordering::Relaxed), 2);
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        session.stop.send_replace(true);
        server.abort();
    }
}
