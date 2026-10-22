//! Native YouTube Music search and source-page metadata extraction.
//! Search uses the public Innertube browse/search protocol. yt-dlp is only a
//! fallback for search and for inspecting a URL; neither path downloads audio.
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;
use tokio::sync::watch;

const PLAYBACK_SEARCH_ENDPOINT: &str = "https://music.youtube.com/youtubei/v1/search";
const PLAYBACK_SEARCH_TIMEOUT: Duration = Duration::from_secs(8);
const PLAYBACK_SEARCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PLAYBACK_SEARCH_BYTES: usize = 1024 * 1024;
const MAX_PLAYBACK_SEARCH_QUERY_BYTES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeMusicItem {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<f64>,
    pub url: String,
    pub artwork_url: Option<String>,
    pub isrc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeMusicContinuation {
    pub token: String,
    pub click_tracking_params: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeMusicPage {
    pub items: Vec<YouTubeMusicItem>,
    pub continuation: Option<YouTubeMusicContinuation>,
    pub warnings: Vec<String>,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeSourceMetadata {
    pub id: String,
    pub url: String,
    pub title: String,
    pub description: Option<String>,
    pub channel: Option<String>,
    pub channel_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub artist: Option<String>,
    pub song: Option<String>,
    pub album: Option<String>,
    pub source_genres: Vec<String>,
    pub upload_date: Option<String>,
    pub release_date: Option<String>,
    pub duration_seconds: Option<f64>,
    pub chapters: Vec<Value>,
    /// Always false: source metadata is not a catalog identity assertion.
    pub catalog_verified: bool,
}

fn text(v: &Value) -> Option<String> {
    let value = v
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            v.get("simpleText")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            v.get("runs").and_then(Value::as_array).map(|runs| {
                runs.iter()
                    .filter_map(|run| run["text"].as_str())
                    .collect::<String>()
            })
        })?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn video_id(v: &Value) -> Option<String> {
    let id = v
        .get("videoId")
        .and_then(Value::as_str)
        .or_else(|| {
            v.pointer("/navigationEndpoint/watchEndpoint/videoId")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            v.pointer("/playlistItemData/videoId")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            v.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            v.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId")
                .and_then(Value::as_str)
        })?
        .trim();
    valid_video_id(id).then(|| id.to_owned())
}
fn labels(v: &Value) -> Vec<String> {
    let source = if v.is_string() {
        vec![v.clone()]
    } else {
        v.as_array().cloned().unwrap_or_default()
    };
    source.iter().filter_map(text).collect()
}
fn renderer_text(v: &Value) -> Option<String> {
    text(v).or_else(|| text(&v["musicResponsiveListItemFlexColumnRenderer"]["text"]))
}
fn flex_runs(column: &Value) -> &[Value] {
    column
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn run_page_type(run: &Value) -> Option<&str> {
    run.pointer("/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType").and_then(Value::as_str)
}
fn run_label(run: &Value) -> Option<String> {
    text(&run["text"])
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn nonempty_str(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn is_youtube_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "https" | "http")
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be"
        })
}

/// Extract a provider-issued continuation token from an Innertube response.
/// Tokens are opaque and are only submitted with the same search context.
fn search_continuation(body: &Value) -> Option<YouTubeMusicContinuation> {
    fn walk(value: &Value) -> Option<YouTubeMusicContinuation> {
        match value {
            Value::Object(map) => {
                for key in [
                    "continuationCommand",
                    "nextContinuationData",
                    "reloadContinuationData",
                ] {
                    if let Some(data) = map.get(key) {
                        if let Some(token) = data
                            .get("token")
                            .or_else(|| data.get("continuation"))
                            .and_then(Value::as_str)
                            .filter(|token| !token.is_empty())
                        {
                            return Some(YouTubeMusicContinuation {
                                token: token.to_owned(),
                                click_tracking_params: data
                                    .get("clickTrackingParams")
                                    .or_else(|| map.get("clickTrackingParams"))
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            });
                        }
                    }
                }
                map.values().find_map(walk)
            }
            Value::Array(items) => items.iter().find_map(walk),
            _ => None,
        }
    }
    walk(body)
}

/// Parse the renderer variants returned by Innertube search. Public for fixture tests.
pub fn parse_search_response(body: &Value, limit: usize) -> Vec<YouTubeMusicItem> {
    fn walk(v: &Value, out: &mut Vec<YouTubeMusicItem>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if let Some(id) = video_id(v) {
            let columns = v["flexColumns"].as_array().cloned().unwrap_or_default();
            let title = text(&v["title"])
                .or_else(|| {
                    columns
                        .first()
                        .and_then(|c| flex_runs(c).first())
                        .and_then(run_label)
                })
                .or_else(|| columns.first().and_then(|c| renderer_text(c)))
                .unwrap_or_default();
            let semantic_runs = columns
                .iter()
                .flat_map(|c| flex_runs(c).iter())
                .collect::<Vec<_>>();
            let mut artist_names = Vec::new();
            for run in semantic_runs
                .iter()
                .filter(|r| run_page_type(r) == Some("MUSIC_PAGE_TYPE_ARTIST"))
            {
                if let Some(name) = run_label(run) {
                    if !artist_names.contains(&name) {
                        artist_names.push(name);
                    }
                }
            }
            let artist = (!artist_names.is_empty()).then(|| artist_names.join(", "));
            let album = semantic_runs
                .iter()
                .find(|r| run_page_type(r) == Some("MUSIC_PAGE_TYPE_ALBUM"))
                .and_then(|r| run_label(r));
            let duration=semantic_runs.iter().filter_map(|r|run_label(r).and_then(|s|parse_duration(&s))).last().or_else(||v.pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/simpleText").and_then(Value::as_str).and_then(parse_duration));
            let artwork = v
                .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
                .or_else(|| v.pointer("/thumbnail/thumbnails"))
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .and_then(|i| i["url"].as_str())
                .map(str::to_owned);
            if !title.trim().is_empty() {
                out.push(YouTubeMusicItem {
                    url: format!("https://music.youtube.com/watch?v={id}"),
                    id,
                    title,
                    artist: artist.filter(|value| !value.trim().is_empty()),
                    album: album.filter(|value| !value.trim().is_empty()),
                    duration_seconds: duration,
                    isrc: None,
                    artwork_url: artwork,
                });
            }
        }
        match v {
            Value::Object(map) => {
                for child in map.values() {
                    walk(child, out, limit);
                }
            }
            Value::Array(items) => {
                for child in items {
                    walk(child, out, limit);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(body, &mut out, limit.min(50));
    let mut seen = std::collections::HashSet::new();
    out.retain(|x| seen.insert(x.id.clone()));
    out
}

/// Find candidates for foreground playback through the native YouTube Music
/// endpoint only. This deliberately has no yt-dlp or subprocess fallback.
pub(crate) async fn search_playback_candidates(
    query: &str,
    limit: usize,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<Vec<YouTubeMusicItem>, String> {
    search_playback_candidates_at(PLAYBACK_SEARCH_ENDPOINT, query, limit, cancel, deadline).await
}

async fn search_playback_candidates_at(
    endpoint: &str,
    query: &str,
    limit: usize,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<Vec<YouTubeMusicItem>, String> {
    let query = query.trim();
    if query.is_empty()
        || query.len() > MAX_PLAYBACK_SEARCH_QUERY_BYTES
        || query.chars().any(char::is_control)
    {
        return Err("YouTube Music playback search query is invalid".into());
    }
    if !(1..=30).contains(&limit) {
        return Err("YouTube Music playback search limit must be between 1 and 30".into());
    }
    if *cancel.borrow() {
        return Err("YouTube Music playback search cancelled".into());
    }

    let operation_deadline = deadline.cap(PLAYBACK_SEARCH_TIMEOUT);
    let budget = operation_deadline.saturating_duration_since(tokio::time::Instant::now());
    if budget.is_zero() {
        return Err("YouTube Music playback search deadline expired".into());
    }
    let client = Client::builder()
        .connect_timeout(PLAYBACK_SEARCH_CONNECT_TIMEOUT.min(budget))
        .timeout(budget)
        .user_agent("Mozilla/5.0 Burrata/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize YouTube Music playback search".to_owned())?;
    let payload = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": "1.20250101.01.00",
                "hl": "en",
                "gl": "US"
            }
        },
        "query": query,
        "params": "EgWKAQIIAWoKEAkQChADEAQQBQ%3D%3D"
    });
    let cancel_wait = wait_for_cancel(cancel.clone());
    let response = tokio::select! {
        biased;
        _ = cancel_wait => return Err("YouTube Music playback search cancelled".into()),
        _ = tokio::time::sleep_until(operation_deadline) => {
            return Err("YouTube Music playback search deadline expired".into());
        }
        result = {
            crate::diagnostic_metrics::record_http_send_attempt();
            client.post(endpoint).json(&payload).send()
        } => {
            result.map_err(|_| "YouTube Music playback search request failed".to_owned())?
        }
    };

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "YouTube Music playback search returned HTTP {status}"
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    if !content_type
        .as_deref()
        .is_some_and(|value| value == "application/json" || value.ends_with("+json"))
    {
        return Err("YouTube Music playback search returned a non-JSON response".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PLAYBACK_SEARCH_BYTES as u64)
    {
        return Err("YouTube Music playback search response exceeded its size limit".into());
    }

    let mut response = response;
    let mut body = Vec::new();
    loop {
        let cancel_wait = wait_for_cancel(cancel.clone());
        let chunk = tokio::select! {
            biased;
            _ = cancel_wait => return Err("YouTube Music playback search cancelled".into()),
            _ = tokio::time::sleep_until(operation_deadline) => {
                return Err("YouTube Music playback search deadline expired".into());
            }
            result = response.chunk() => {
                result.map_err(|_| "YouTube Music playback search response read failed".to_owned())?
            }
        };
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > MAX_PLAYBACK_SEARCH_BYTES {
            return Err("YouTube Music playback search response exceeded its size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&body)
        .map_err(|_| "YouTube Music playback search returned invalid JSON".to_owned())?;
    Ok(parse_search_response(&body, limit))
}

async fn wait_for_cancel(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

fn parse_duration(s: &str) -> Option<f64> {
    if !s.contains(':') {
        return None;
    }
    s.split(':')
        .try_fold(0.0, |n, p| p.parse::<f64>().ok().map(|x| n * 60.0 + x))
}

/// Fetch exactly one YouTube Music search page. Continuation tokens are opaque;
/// callers must bind them to the original query and filters in their cursor.
pub async fn search_page(
    query: &str,
    isrc: Option<&str>,
    continuation: Option<&YouTubeMusicContinuation>,
    yt_dlp: &str,
) -> Result<YouTubeMusicPage, String> {
    let q = isrc
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("{s} {query}"))
        .unwrap_or_else(|| query.to_owned());
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 Burrata/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let endpoint = "https://music.youtube.com/youtubei/v1/search";
    let context = json!({"context":{"client":{"clientName":"WEB_REMIX","clientVersion":"1.20250101.01.00","hl":"en","gl":"US"}}});
    let payload = if let Some(token) = continuation {
        let mut payload = json!({"context":context["context"].clone(),"continuation":&token.token});
        if let Some(click_tracking) = token.click_tracking_params.as_deref() {
            payload["clickTracking"] = json!({"clickTrackingParams":click_tracking});
        }
        payload
    } else {
        json!({"context":context["context"].clone(),"query":q,"params":"EgWKAQIIAWoKEAkQChADEAQQBQ%3D%3D"})
    };
    crate::diagnostic_metrics::record_http_send_attempt();
    let response = client.post(endpoint).json(&payload).send().await;
    if let Ok(response) = response {
        if response.status().is_success() {
            if let Ok(body) = response.json::<Value>().await {
                let items = parse_search_response(&body, 50);
                let next = search_continuation(&body);
                if !items.is_empty() || next.is_some() {
                    return Ok(YouTubeMusicPage {
                        items,
                        continuation: next,
                        warnings: vec![],
                        incomplete: false,
                    });
                }
            }
        }
    }
    if continuation.is_some() {
        return Ok(YouTubeMusicPage {
            items: vec![],
            continuation: None,
            warnings: vec![
                "YouTube Music continuation request failed; no further page is available".into(),
            ],
            incomplete: true,
        });
    }

    // Fallback is a bounded fresh search. yt-dlp does not expose its internal
    // search cursor through this API, so only the Innertube path can continue.
    let mut warnings =
        vec!["YouTube Music community search was unavailable; used yt-dlp search fallback".into()];
    let mut command = Command::new(yt_dlp);
    crate::tools::configure_no_console_tokio(&mut command);
    let output = command
        .args([
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-remote-components",
            "--no-js-runtimes",
            "--no-warnings",
            "--flat-playlist",
            "--dump-single-json",
            "--playlist-end",
            "30",
            &format!("ytsearch30:{q}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| {
            format!("YouTube Music search failed and yt-dlp fallback could not start: {e}")
        })?;
    if !output.status.success() {
        return Err("YouTube Music search failed through both native protocol and yt-dlp".into());
    }
    let v: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "yt-dlp returned invalid search data")?;
    let entries = v["entries"].as_array().into_iter().flatten();
    let items: Vec<YouTubeMusicItem> = entries
        .filter_map(|e| {
            let id = nonempty_str(&e["id"])?;
            if !valid_video_id(&id) {
                return None;
            }
            let title = nonempty_str(&e["title"])?;
            let url = nonempty_str(&e["url"])
                .filter(|url| is_youtube_url(url))
                .unwrap_or_else(|| format!("https://music.youtube.com/watch?v={id}"));
            Some(YouTubeMusicItem {
                url,
                id,
                title,
                artist: nonempty_str(&e["artist"]),
                album: nonempty_str(&e["album"]),
                duration_seconds: e["duration"].as_f64(),
                isrc: nonempty_str(&e["isrc"]),
                artwork_url: nonempty_str(&e["thumbnail"]),
            })
        })
        .collect();
    let incomplete = items.len() >= 30;
    if incomplete {
        warnings.push(
            "yt-dlp search stopped at its 30-result cap; additional results were not checked"
                .into(),
        );
    }
    Ok(YouTubeMusicPage {
        items,
        continuation: None,
        warnings,
        incomplete,
    })
}

/// Backward-compatible one-page search helper.
pub async fn search(
    query: &str,
    isrc: Option<&str>,
    yt_dlp: &str,
) -> Result<(Vec<YouTubeMusicItem>, Vec<String>), String> {
    let page = search_page(query, isrc, None, yt_dlp).await?;
    Ok((page.items, page.warnings))
}

/// Inspect metadata only. No format selection or media transfer is requested.
pub async fn inspect_url(url: &str, yt_dlp: &str) -> Result<YouTubeSourceMetadata, String> {
    inspect_url_with_runtime(url, yt_dlp, "deno").await
}
pub async fn inspect_url_with_runtime(
    url: &str,
    yt_dlp: &str,
    js_runtime: &str,
) -> Result<YouTubeSourceMetadata, String> {
    let parsed = url::Url::parse(url).map_err(|_| "Expected a public YouTube URL")?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some(
                "www.youtube.com"
                    | "youtube.com"
                    | "youtu.be"
                    | "music.youtube.com"
                    | "soundcloud.com"
                    | "www.soundcloud.com"
                    | "m.soundcloud.com"
            )
        )
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Expected a public YouTube URL".into());
    }
    let runtime_name = std::path::Path::new(js_runtime)
        .file_stem()
        .and_then(|v| v.to_str())
        .filter(|v| matches!(*v, "node" | "nodejs" | "deno" | "bun" | "qjs" | "quickjs"))
        .unwrap_or("deno");
    let runtime_kind = match runtime_name {
        "node" | "nodejs" => "node",
        "bun" => "bun",
        "qjs" | "quickjs" => "quickjs",
        _ => "deno",
    };
    let runtime_path = crate::tools::executable(js_runtime, runtime_kind);
    let mut command = Command::new(crate::tools::executable(yt_dlp, "yt-dlp"));
    crate::tools::configure_no_console_tokio(&mut command);
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        command
            .args([
                "--ignore-config",
                "--no-plugin-dirs",
                "--no-remote-components",
                "--no-warnings",
                "--skip-download",
                "--dump-single-json",
            ])
            .arg("--js-runtimes")
            .arg(format!("{runtime_kind}:{}", runtime_path.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .arg("--")
            .arg(url)
            .output(),
    )
    .await
    .map_err(|_| "yt-dlp metadata inspection timed out")?
    .map_err(|e| format!("Could not start yt-dlp metadata inspection: {e}"))?;
    if !output.status.success() {
        return Err("yt-dlp could not inspect this YouTube page".into());
    }
    let v: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "yt-dlp returned invalid metadata")?;
    let id = nonempty_str(&v["id"]).unwrap_or_default();
    Ok(YouTubeSourceMetadata {
        id,
        url: nonempty_str(&v["webpage_url"]).unwrap_or_else(|| url.to_owned()),
        title: nonempty_str(&v["title"]).unwrap_or_default(),
        description: nonempty_str(&v["description"]),
        channel: nonempty_str(&v["channel"]).or_else(|| nonempty_str(&v["uploader"])),
        channel_url: nonempty_str(&v["channel_url"]).or_else(|| nonempty_str(&v["uploader_url"])),
        thumbnail_url: nonempty_str(&v["thumbnail"]),
        artist: nonempty_str(&v["artist"]),
        song: nonempty_str(&v["track"]),
        album: nonempty_str(&v["album"]),
        source_genres: labels(&v["genres"]),
        upload_date: nonempty_str(&v["upload_date"]),
        release_date: nonempty_str(&v["release_date"]),
        duration_seconds: v["duration"].as_f64(),
        chapters: v["chapters"].as_array().cloned().unwrap_or_default(),
        catalog_verified: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
        task::JoinHandle,
    };

    async fn fixture_server(
        response: Option<Vec<u8>>,
    ) -> (String, oneshot::Receiver<()>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (entered_tx, entered_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => request.extend_from_slice(&chunk[..read]),
                }
            }
            let _ = entered_tx.send(());
            if let Some(response) = response {
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            } else {
                std::future::pending::<()>().await;
            }
        });
        (format!("http://{address}/search"), entered_rx, task)
    }

    fn fixture_http_response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
            503 => "Service Unavailable",
            _ => "Fixture",
        };
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [head.as_bytes(), body].concat()
    }

    fn chunked_fixture_http_response(content_type: &str, body: &[u8]) -> Vec<u8> {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n",
            body.len()
        );
        [head.as_bytes(), body, b"\r\n0\r\n\r\n"].concat()
    }

    fn fixture_deadline() -> crate::playback_sources::PlaybackDeadline {
        crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(3))
    }

    fn valid_search_body() -> Value {
        json!({
            "contents": {
                "musicResponsiveListItemRenderer": {
                    "videoId":"fixturevid1",
                    "title":{"simpleText":"Fixture Song"}
                }
            }
        })
    }

    #[tokio::test]
    async fn foreground_search_returns_parsed_items_from_valid_json() {
        let payload = serde_json::to_vec(&valid_search_body()).unwrap();
        let (endpoint, _entered, server) = fixture_server(Some(fixture_http_response(
            200,
            "application/json; charset=UTF-8",
            &payload,
        )))
        .await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let items = search_playback_candidates_at(
            &endpoint,
            "Fixture Song",
            30,
            cancel,
            fixture_deadline(),
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "fixturevid1");
        assert_eq!(items[0].title, "Fixture Song");
        assert_eq!(
            items[0].url,
            "https://music.youtube.com/watch?v=fixturevid1"
        );
    }

    #[tokio::test]
    async fn foreground_search_rejects_http_errors_html_and_oversize_bodies() {
        let cases = [
            fixture_http_response(503, "application/json", b"{}"),
            fixture_http_response(200, "text/html", b"<html>no</html>"),
            chunked_fixture_http_response(
                "application/json",
                &vec![b'x'; MAX_PLAYBACK_SEARCH_BYTES + 1],
            ),
        ];
        let expected = ["HTTP 503", "non-JSON", "size limit"];
        for (raw_response, expected) in cases.into_iter().zip(expected) {
            let (endpoint, _entered, server) = fixture_server(Some(raw_response)).await;
            let (_cancel_tx, cancel) = watch::channel(false);
            let error = search_playback_candidates_at(
                &endpoint,
                "Fixture Song",
                30,
                cancel,
                fixture_deadline(),
            )
            .await
            .unwrap_err();
            server.await.unwrap();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[tokio::test]
    async fn foreground_search_cancels_an_inflight_request_promptly() {
        let (endpoint, entered, server) = fixture_server(None).await;
        let (cancel_tx, cancel) = watch::channel(false);
        let search = tokio::spawn(async move {
            search_playback_candidates_at(&endpoint, "Fixture Song", 30, cancel, fixture_deadline())
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), entered)
            .await
            .expect("request reached fixture")
            .unwrap();
        cancel_tx.send(true).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), search)
            .await
            .expect("cancel should drop the request promptly")
            .unwrap()
            .unwrap_err();
        assert!(error.contains("cancelled"));
        server.abort();
    }

    #[tokio::test]
    async fn foreground_search_respects_absolute_deadline_during_request() {
        let (endpoint, entered, server) = fixture_server(None).await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let search = tokio::spawn(async move {
            search_playback_candidates_at(
                &endpoint,
                "Fixture Song",
                30,
                cancel,
                crate::playback_sources::PlaybackDeadline::after(Duration::from_millis(75)),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), entered)
            .await
            .expect("request reached fixture")
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), search)
            .await
            .expect("absolute deadline should bound the request")
            .unwrap()
            .unwrap_err();
        assert!(error.contains("deadline expired"));
        server.abort();
    }

    #[tokio::test]
    async fn foreground_search_validates_query_and_limit_without_network() {
        let (_cancel_tx, cancel) = watch::channel(false);
        for (query, limit) in [(" ", 10), ("valid query", 0), ("valid query", 31)] {
            let error = search_playback_candidates_at(
                "http://127.0.0.1:1/search",
                query,
                limit,
                cancel.clone(),
                fixture_deadline(),
            )
            .await
            .unwrap_err();
            assert!(error.contains("invalid") || error.contains("limit"));
        }
    }

    #[test]
    fn public_music_collaboration_retains_every_semantic_artist_run() {
        let raw: Value = serde_json::from_str(include_str!(
            "../fixtures/youtube-music-reckless-performer-runs.json"
        ))
        .unwrap();
        let items = parse_search_response(&raw, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "FXIvaktRqZg");
        assert_eq!(
            items[0].artist.as_deref(),
            Some("Nipsey Hussle, Bino Rideaux")
        );
        assert_eq!(items[0].title, "Reckless (feat. Static Major)");
        assert_eq!(items[0].album.as_deref(), Some("Reckless"));
        assert_eq!(items[0].duration_seconds, Some(139.0));
        assert!(!items[0].artist.as_ref().unwrap().contains("plays"));
    }

    #[test]
    fn extracts_provider_continuation_tokens_from_known_renderer_shapes() {
        assert_eq!(
            search_continuation(
                &json!({"continuations":[{"nextContinuationData":{"continuation":"opaque-token"}}]})
            ),
            Some(YouTubeMusicContinuation {
                token: "opaque-token".into(),
                click_tracking_params: None
            })
        );
        assert_eq!(
            search_continuation(
                &json!({"continuationItemRenderer":{"continuationEndpoint":{"clickTrackingParams":"sibling-ctp","continuationCommand":{"token":"opaque-two"}}}})
            ),
            Some(YouTubeMusicContinuation {
                token: "opaque-two".into(),
                click_tracking_params: Some("sibling-ctp".into())
            })
        );
        let raw_fixture = json!({
            "contents": [{
                "musicResponsiveListItemRenderer": {"videoId":"validvideo1","title":{"simpleText":"Song"}}
            }],
            "continuations": [{
                "nextContinuationData": {"continuation":"synthetic-next-token","clickTrackingParams":"synthetic-ctp"}
            }]
        });
        assert_eq!(parse_search_response(&raw_fixture, 30).len(), 1);
        let continuation = search_continuation(&raw_fixture).unwrap();
        assert_eq!(continuation.token, "synthetic-next-token");
        assert_eq!(
            continuation.click_tracking_params.as_deref(),
            Some("synthetic-ctp")
        );
        let continuation_response = json!({
            "continuationContents": {
                "musicShelfContinuation": {
                    "contents": [{
                        "musicResponsiveListItemRenderer": {
                            "videoId": "synthetic02",
                            "title": {"simpleText": "Fixture page two"}
                        }
                    }],
                    "continuations": [{
                        "nextContinuationData": {
                            "continuation": "synthetic-page-three",
                            "clickTrackingParams": "synthetic-ctp-three"
                        }
                    }]
                }
            }
        });
        assert_eq!(parse_search_response(&continuation_response, 30).len(), 1);
        assert_eq!(
            search_continuation(&continuation_response).unwrap().token,
            "synthetic-page-three"
        );
        assert_eq!(search_continuation(&json!({"contents":[]})), None);
        assert_eq!(search_continuation(&json!({"continuations":[{"nextContinuationData":{"continuation":"token-3","clickTrackingParams":"ctp-3"}}]})).unwrap().click_tracking_params.as_deref(), Some("ctp-3"));
    }

    #[test]
    fn retained_public_continuation_shape_parses_both_pages() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/discovery/youtube-music-continuation-shape.json"
        ))
        .unwrap();
        for page in ["initial", "continuation"] {
            let body = &fixture[page];
            assert_eq!(parse_search_response(body, 30).len(), 1, "{page}");
            let next = search_continuation(body).expect("provider continuation");
            assert_eq!(next.token, "synthetic-continuation-token");
            assert_eq!(
                next.click_tracking_params.as_deref(),
                Some("synthetic-click-tracking")
            );
        }
    }

    #[test]
    fn parses_music_video_renderer_fixture_and_deduplicates() {
        let artist_endpoint = json!({"browseEndpoint":{"browseEndpointContextSupportedConfigs":{"browseEndpointContextMusicConfig":{"pageType":"MUSIC_PAGE_TYPE_ARTIST"}}}});
        let album_endpoint = json!({"browseEndpoint":{"browseEndpointContextSupportedConfigs":{"browseEndpointContextMusicConfig":{"pageType":"MUSIC_PAGE_TYPE_ALBUM"}}}});
        let renderer = json!({"flexColumns":[{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Mary","navigationEndpoint":{"watchEndpoint":{"videoId":"maryvideoid"}}}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Big Thief","navigationEndpoint":artist_endpoint}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Capacity","navigationEndpoint":album_endpoint}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"3.8M plays"}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"5:31"}]}}}],"thumbnail":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[{"url":"https://img.example/cover.jpg"}]}}}});
        let fixture = json!({"contents":{"musicResponsiveListItemRenderer":renderer}});
        let parsed = parse_search_response(&fixture, 10);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "maryvideoid");
        assert_eq!(parsed[0].title, "Mary");
        assert_eq!(parsed[0].artist.as_deref(), Some("Big Thief"));
        assert_eq!(parsed[0].album.as_deref(), Some("Capacity"));
        assert_eq!(parsed[0].duration_seconds, Some(331.0));
        assert_eq!(
            parsed[0].artwork_url.as_deref(),
            Some("https://img.example/cover.jpg")
        );
    }

    #[test]
    fn music_search_rejects_blank_titles_and_malformed_video_ids() {
        for renderer in [
            json!({"videoId":"short","title":"Song"}),
            json!({"videoId":"validvideo1","title":"  "}),
        ] {
            assert!(parse_search_response(&renderer, 10).is_empty());
        }
        assert!(valid_video_id("validvideo1"));
        assert!(!valid_video_id("short"));
        assert_eq!(nonempty_str(&json!("  ")), None);
    }

    #[test]
    fn music_search_does_not_promote_untyped_second_column_to_artist() {
        let row = json!({
            "videoId":"validvideo1",
            "flexColumns":[
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Song Title"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Capacity"}]}}}
            ]
        });
        let parsed = parse_search_response(&row, 10);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Song Title");
        assert!(parsed[0].artist.is_none());
    }
}
