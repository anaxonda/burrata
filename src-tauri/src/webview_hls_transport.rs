//! HLS children are discovered from bounded, trusted manifests, never HTTP input.
use super::*;
use std::collections::HashMap;
const MAX_MANIFEST: usize = 1024 * 1024;
const MAX_RESOURCES: usize = 4096;
const MAX_DEPTH: usize = 4;
const MAX_URI: usize = 8192;
const MAX_URL_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default, Clone)]
pub(super) struct Graph {
    entries: HashMap<String, Entry>,
}
#[derive(Clone)]
struct Entry {
    url: url::Url,
    depth: usize,
    seen: std::time::Instant,
    persistent: bool,
}
impl Graph {
    pub(super) fn lookup(&mut self, id: &str) -> Option<(url::Url, usize)> {
        self.entries.get_mut(id).map(|entry| {
            entry.seen = std::time::Instant::now();
            (entry.url.clone(), entry.depth)
        })
    }
    fn register(&mut self, url: url::Url, depth: usize) -> Result<String, String> {
        if url.as_str().len() > MAX_URI {
            return Err("HLS resource URI exceeds the supported limit".into());
        }
        if depth > MAX_DEPTH {
            return Err("HLS playlist nesting exceeds the supported limit".into());
        }
        let now = std::time::Instant::now();
        self.entries.retain(|_, entry| {
            entry.persistent || now.duration_since(entry.seen) < Duration::from_secs(600)
        });
        if let Some((id, entry)) = self.entries.iter_mut().find(|(_, entry)| entry.url == url) {
            entry.seen = now;
            // Retain the deepest observed path so cyclic playlists cannot reset
            // nesting by repeatedly reusing an existing resource ID.
            entry.depth = entry.depth.max(depth);
            return Ok(id.clone());
        }
        let bytes: usize = self
            .entries
            .values()
            .map(|entry| entry.url.as_str().len())
            .sum();
        if bytes.saturating_add(url.as_str().len()) > MAX_URL_BYTES {
            return Err("HLS resource URL memory exceeds the supported limit".into());
        }
        if self.entries.len() >= MAX_RESOURCES {
            return Err("HLS resource window exceeds the supported limit".into());
        }
        // Preserve only a fixed media-type suffix, never any upstream filename.
        // Native HLS clients and FFmpeg apply segment extension allowlists even
        // when the response Content-Type is correct.
        let suffix = url
            .path()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let suffix = match suffix.as_str() {
            "m3u8" | "ts" | "m4s" | "mp4" | "m4a" | "aac" | "mp3" | "ac3" | "ec3" | "vtt"
            | "key" => suffix.as_str(),
            _ => "bin",
        };
        let id = format!("{}.{suffix}", uuid::Uuid::new_v4());
        self.entries.insert(
            id.clone(),
            Entry {
                url,
                depth,
                seen: now,
                persistent: false,
            },
        );
        Ok(id)
    }
}
pub(super) async fn audio_child(
    State(manager): State<Arc<WebviewAudioTransport>>,
    Path((id, resource)): Path<(String, String)>,
    Query(auth): Query<Auth>,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    serve(manager, id, Some(resource), auth, headers, method).await
}
pub(super) async fn serve(
    manager: Arc<WebviewAudioTransport>,
    id: String,
    resource: Option<String>,
    auth: Auth,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    super::direct_audio(manager, id, resource, auth, headers, method).await
}
pub(super) fn error(message: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(message.to_owned()))
        .unwrap()
}
pub(super) async fn public_client(url: &url::Url) -> Result<reqwest::Client, String> {
    let mut addresses = acquisition::public_addresses(url).await?;
    for address in &mut addresses {
        address.set_port(url.port_or_known_default().ok_or("HLS port missing")?);
    }
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(15))
        .resolve_to_addrs(url.host_str().ok_or("HLS host missing")?, &addresses)
        .build()
        .map_err(|_| "HLS client unavailable".into())
}
pub(super) fn is_manifest(url: &url::Url, headers: &HeaderMap) -> bool {
    url.path().to_ascii_lowercase().ends_with(".m3u8")
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().contains("mpegurl"))
}
fn child_url(
    parent: &url::Url,
    raw: &str,
    allow_http: bool,
    root: &url::Url,
) -> Result<url::Url, String> {
    if raw.len() > MAX_URI {
        return Err("HLS resource URI exceeds the supported limit".into());
    }
    if raw.contains("{$") {
        return Err("HLS variable substitution is not supported".into());
    }
    let url = parent
        .join(raw)
        .map_err(|_| "Invalid HLS resource reference")?;
    // Unit fixtures alone may serve an explicitly registered loopback origin.
    #[cfg(test)]
    if root.host_str() == Some("127.0.0.1") && url.origin() == root.origin() {
        return Ok(url);
    }
    if allow_http && url.scheme() == "http" && url.origin() == root.origin() {
        super::validated_url(url.as_str(), true)?;
    } else {
        acquisition::validate_source_url(url.as_str(), "direct")?;
    }
    Ok(url)
}
/// Quoted URI attributes may contain commas. Reject malformed syntax rather than
/// allowing a browser to retrieve a reference that escaped rewriting.
fn rewrite_attributes(
    line: &str,
    mut rewrite: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = line;
    while let Some(position) = rest.find("URI=") {
        output.push_str(&rest[..position + 4]);
        rest = &rest[position + 4..];
        if !rest.starts_with('"') {
            return Err("HLS URI attributes must be quoted".into());
        }
        let end = rest[1..]
            .find('"')
            .map(|v| v + 1)
            .ok_or("Unterminated HLS URI attribute")?;
        output.push('"');
        output.push_str(&rewrite(&rest[1..end])?);
        output.push('"');
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}
fn rewrite_manifest(
    text: &str,
    parent: &url::Url,
    depth: usize,
    root: &url::Url,
    allow_http: bool,
    graph: &mut Graph,
    prefix: &str,
    token: &str,
) -> Result<String, String> {
    // Reject the whole update atomically: a later invalid reference must not
    // publish an earlier partial graph, refresh timestamps or evict live URLs.
    let mut draft = graph.clone();
    let result = rewrite_manifest_inner(
        text, parent, depth, root, allow_http, &mut draft, prefix, token,
    )?;
    *graph = draft;
    Ok(result)
}
fn rewrite_manifest_inner(
    text: &str,
    parent: &url::Url,
    depth: usize,
    root: &url::Url,
    allow_http: bool,
    graph: &mut Graph,
    prefix: &str,
    token: &str,
) -> Result<String, String> {
    if text.len() > MAX_MANIFEST || !text.trim_start_matches('\u{feff}').starts_with("#EXTM3U") {
        return Err("Invalid or oversized HLS manifest".into());
    }
    let mut output = String::new();
    let mut references = 0;
    let persistent = text.lines().any(|line| {
        line.trim() == "#EXT-X-ENDLIST"
            || line.starts_with("#EXT-X-STREAM-INF:")
            || line.starts_with("#EXT-X-MEDIA:")
    });
    let mut rewrite = |raw: &str| -> Result<String, String> {
        let url = child_url(parent, raw, allow_http, root)?;
        let id = graph.register(url, depth + 1)?;
        if persistent {
            if let Some(entry) = graph.entries.get_mut(&id) {
                entry.persistent = true;
            }
        }
        references += 1;
        Ok(format!("{prefix}/{id}?token={token}"))
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("#EXT-X-DEFINE:")
            || line.starts_with("#EXT-X-CONTENT-STEERING:")
            || line.starts_with("#EXT-X-PART:")
            || line.starts_with("#EXT-X-PRELOAD-HINT:")
            || line.starts_with("#EXT-X-RENDITION-REPORT:")
        {
            return Err(
                "This HLS manifest uses unsupported dynamic or low-latency references".into(),
            );
        }
        if (line.starts_with("#EXT-X-KEY:") || line.starts_with("#EXT-X-SESSION-KEY:"))
            && (!line.contains("METHOD=AES-128") && !line.contains("METHOD=NONE")
                || line.contains("KEYFORMAT=") && !line.contains("KEYFORMAT=\"identity\""))
        {
            return Err("This HLS encryption method is not supported".into());
        }
        let rewritten = if line.starts_with('#') {
            rewrite_attributes(line, &mut rewrite)?
        } else if line.is_empty() {
            String::new()
        } else {
            rewrite(line)?
        };
        output.push_str(&rewritten);
        output.push('\n');
    }
    if references == 0 {
        return Err("HLS manifest contains no playable references".into());
    }
    Ok(output)
}
pub(super) async fn manifest_response(
    manager: &WebviewAudioTransport,
    session: &Arc<Session>,
    target: &url::Url,
    depth: usize,
    response: reqwest::Response,
    mut stop: watch::Receiver<bool>,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    if !response.status().is_success() {
        return error("HLS manifest request failed");
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_MANIFEST as u64)
    {
        return error("HLS manifest is too large");
    }
    let read = async {
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "HLS manifest read failed")?;
            if bytes.len() + chunk.len() > MAX_MANIFEST {
                return Err("HLS manifest is too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).map_err(|_| "HLS manifest is not UTF-8")
    };
    let text = tokio::select! {biased;_=stop.changed()=>return error("HLS playback was cancelled"),result=tokio::time::timeout(Duration::from_secs(5),read)=>match result{Ok(Ok(text))=>text,Ok(Err(message))=>return error(message),Err(_)=>return error("HLS manifest read timed out")}};
    let Some(base) = manager.base_url.lock().ok().and_then(|base| base.clone()) else {
        return error("HLS transport is unavailable");
    };
    let prefix = format!("{base}/webview-audio/{}", session.id);
    let rewritten = {
        let Ok(mut graph) = session.hls.lock() else {
            return error("HLS transport is unavailable");
        };
        match rewrite_manifest(
            &text,
            target,
            depth,
            &session.source,
            session.allow_radio_http,
            &mut graph,
            &prefix,
            &session.token,
        ) {
            Ok(value) => value,
            Err(message) => return error(&message),
        }
    };
    let mut builder = Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_LENGTH, rewritten.len())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(origin) = headers.get(header::ORIGIN) {
        builder = builder
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
            .header(header::VARY, "Origin");
    }
    builder
        .body(if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(rewritten)
        })
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rewrite(text: &str) -> Result<(String, Graph), String> {
        let root = url::Url::parse("https://radio.example/path/master.m3u8").unwrap();
        let mut graph = Graph::default();
        let result = rewrite_manifest(
            text,
            &root,
            0,
            &root,
            false,
            &mut graph,
            "http://127.0.0.1:9/webview-audio/id",
            "secret",
        )?;
        Ok((result, graph))
    }
    #[test]
    fn nested_attributes_keys_maps_ranges_and_relative_urls_are_owned() {
        let (text,graph)=rewrite("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"nested/audio.m3u8\",NAME=\"a,b\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"10@0\"\n#EXT-X-BYTERANGE:20@10\nsegment.ts\n").unwrap();
        assert_eq!(graph.entries.len(), 4);
        assert_eq!(text.matches("?token=secret").count(), 4);
        assert!(text.contains("BYTERANGE=\"10@0\""));
        assert!(text.contains("#EXT-X-BYTERANGE:20@10"));
        assert!(!text.contains("https://radio.example"));
        assert!(graph
            .entries
            .values()
            .any(|entry| entry.url.as_str() == "https://radio.example/path/nested/audio.m3u8"));
    }
    #[test]
    fn private_urls_schemes_malformed_and_unsupported_specs_fail_closed() {
        for target in [
            "http://evil.example/a",
            "https://127.0.0.1/a",
            "https://localhost/a",
            "file:///etc/passwd",
            "data:text/plain,a",
            "https://user:pass@radio.example/a",
            "{$secret}/a",
        ] {
            assert!(
                rewrite(&format!("#EXTM3U\n{target}\n")).is_err(),
                "{target}"
            );
        }
        for text in [
            "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\"\nsegment.ts",
            "#EXTM3U\n#EXT-X-MEDIA:URI=unquoted\nsegment.ts",
            "#EXTM3U\n#EXT-X-DEFINE:NAME=\"x\",VALUE=\"a\"\nsegment.ts",
            "#EXTM3U\n#EXT-X-CONTENT-STEERING:SERVER-URI=\"https://a.example/a\"\nsegment.ts",
            "#EXTM3U",
        ] {
            assert!(rewrite(text).is_err(), "{text}");
        }
    }
    #[test]
    fn live_refresh_reuses_ids_and_expired_windows_are_bounded() {
        let root = url::Url::parse("https://radio.example/live.m3u8").unwrap();
        let mut graph = Graph::default();
        let first = rewrite_manifest(
            "#EXTM3U\nsegment.ts",
            &root,
            0,
            &root,
            false,
            &mut graph,
            "local",
            "t",
        )
        .unwrap();
        let second = rewrite_manifest(
            "#EXTM3U\nsegment.ts",
            &root,
            0,
            &root,
            false,
            &mut graph,
            "local",
            "t",
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(graph.entries.len(), 1);
        for entry in graph.entries.values_mut() {
            entry.seen = std::time::Instant::now() - Duration::from_secs(601);
        }
        graph.register(root.join("next.ts").unwrap(), 1).unwrap();
        assert_eq!(graph.entries.len(), 1);
        for i in 1..MAX_RESOURCES {
            graph
                .register(root.join(&format!("{i}.ts")).unwrap(), 1)
                .unwrap();
        }
        assert!(graph
            .register(root.join("overflow.ts").unwrap(), 1)
            .is_err());
        assert!(graph.register(root.clone(), MAX_DEPTH + 1).is_err());
    }
    #[test]
    fn saved_http_is_scoped_to_exact_root_authority() {
        let root = url::Url::parse("http://radio.example:8000/live.m3u8").unwrap();
        assert!(child_url(&root, "segment.ts", true, &root).is_ok());
        assert!(child_url(&root, "http://other.example/seg.ts", true, &root).is_err());
        assert!(child_url(&root, "http://radio.example:9000/seg.ts", true, &root).is_err());
        assert!(child_url(&root, "https://cdn.example/seg.ts", true, &root).is_ok());
    }
    #[test]
    fn uri_and_aggregate_limits_reject_whole_manifest_without_publication() {
        let root = url::Url::parse("https://radio.example/live.m3u8").unwrap();
        let mut graph = Graph::default();
        graph.register(root.join("known.ts").unwrap(), 1).unwrap();
        let original = graph.entries.keys().cloned().collect::<Vec<_>>();
        let text = format!("#EXTM3U\nfirst.ts\n{}\n", "x".repeat(MAX_URI + 1));
        assert!(
            rewrite_manifest(&text, &root, 0, &root, false, &mut graph, "local", "token").is_err()
        );
        assert_eq!(graph.entries.len(), 1);
        assert!(original.iter().all(|id| graph.entries.contains_key(id)));
        let padding = "x".repeat(7600);
        for i in 0..1200 {
            let url = root.join(&format!("{padding}{i}.ts")).unwrap();
            if graph.register(url, 1).is_err() {
                break;
            }
        }
        let before = graph.entries.len();
        let before_bytes: usize = graph
            .entries
            .values()
            .map(|entry| entry.url.as_str().len())
            .sum();
        assert!(before_bytes <= MAX_URL_BYTES);
        assert!(before_bytes > MAX_URL_BYTES - 8192);
        let text = format!("#EXTM3U\nsmall.ts\n{padding}overflow.ts\n");
        assert!(
            rewrite_manifest(&text, &root, 0, &root, false, &mut graph, "local", "token").is_err()
        );
        assert_eq!(graph.entries.len(), before);
        assert!(!graph
            .entries
            .values()
            .any(|entry| entry.url.path() == "/small.ts"));
        assert_eq!(
            graph
                .entries
                .values()
                .map(|entry| entry.url.as_str().len())
                .sum::<usize>(),
            before_bytes
        );
    }
    #[test]
    fn opaque_resource_ids_keep_only_allowlisted_media_extensions() {
        let root = url::Url::parse("https://radio.example/path/master.m3u8").unwrap();
        let mut graph = Graph::default();
        for (name, suffix) in [
            ("secret-title.AAC?auth=hidden", "aac"),
            ("nested.m3u8", "m3u8"),
            ("key.key", "key"),
            ("init.mp4", "mp4"),
            ("script.html", "bin"),
            ("noextension", "bin"),
        ] {
            let id = graph.register(root.join(name).unwrap(), 1).unwrap();
            let (opaque, extension) = id.rsplit_once('.').unwrap();
            assert!(uuid::Uuid::parse_str(opaque).is_ok());
            assert_eq!(extension, suffix);
            assert!(!id.contains("secret"));
            assert!(!id.contains("hidden"));
        }
    }
}
