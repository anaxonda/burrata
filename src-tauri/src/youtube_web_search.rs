//! Bounded anonymous YouTube WEB playback candidate discovery.
//!
//! This module returns provisional IDs and display metadata only. It performs no
//! recording match, uploader/channel verification, proof construction, ranking,
//! or persistence. The caller must run the existing fresh-player identity and
//! access gates before accepting or returning PCM.
//!
//! Protocol provenance: follows yt-dlp `YoutubeSearchIE` videos-only params
//! `EgIQAfABAQ==` from pinned source revision
//! `897341241f2bfb2439fe0367f4c7c9ccd29a3828`; WEB profile version
//! `2.20260623.01.00` is from checked-in youtubei.js 18.1.0
//! `dist/src/utils/Constants.js`. This profile successfully answered the two
//! anonymous host feasibility requests recorded in
//! `/tmp/burrata-native-web-search-feasibility-host-v2.json` (request set
//! `burrata-web-search-2026-10-19-JD-Matthew-Pengus-Victory-Lap-v2`). Those
//! timings are feasibility observations during a concurrent build, not startup
//! qualification. No cookies, authorization, API keys, visitor IDs, or search
//! tokens are sent or logged here.

use reqwest::{header, Client, StatusCode};
use serde_json::Value;
use std::{sync::OnceLock, time::Duration};
use tokio::sync::watch;

const SEARCH_URL: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";
const CLIENT_NAME: &str = "WEB";
const CLIENT_VERSION: &str = "2.20260623.01.00";
const VIDEO_ONLY_PARAMS: &str = "EgIQAfABAQ==";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131 Safari/537.36";
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 30;
const MAX_QUERY_BYTES: usize = 512;
const REQUEST_BUDGET: Duration = Duration::from_secs(2);

/// Untrusted provisional search row. Every field comes from public search JSON;
/// none of it is an identity proof or verified source metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct YoutubeWebSearchCandidate {
    pub(crate) video_id: String,
    pub(crate) title: String,
    pub(crate) duration_seconds: Option<u32>,
    pub(crate) uploader: Option<String>,
}

fn client() -> Result<&'static Client, &'static str> {
    static CLIENT: OnceLock<Result<Client, ()>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(REQUEST_BUDGET)
            .timeout(REQUEST_BUDGET)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| ())
    }) {
        Ok(client) => Ok(client),
        Err(()) => Err("YouTube WEB search client unavailable"),
    }
}

/// Search one literal playback query. The network opportunity is capped at
/// two seconds and at the caller's original absolute playback deadline.
/// Cancellation drops the request/body stream immediately.
pub(crate) async fn search_candidates(
    query: &str,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<Vec<YoutubeWebSearchCandidate>, &'static str> {
    if *cancel.borrow() {
        return Err("Cancelled");
    }
    let query = safe_query_part(query)?;
    if query.len() > MAX_QUERY_BYTES {
        return Err("YouTube WEB search query too long");
    }
    search_candidates_at(client()?, SEARCH_URL, query, cancel, deadline).await
}

async fn search_candidates_at(
    client: &Client,
    endpoint: &str,
    query: &str,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<Vec<YoutubeWebSearchCandidate>, &'static str> {
    let opportunity_deadline = deadline.cap(REQUEST_BUDGET);
    if opportunity_deadline <= tokio::time::Instant::now() {
        return Err("YouTube WEB search deadline exceeded");
    }

    let work = async {
        let body = serde_json::json!({
            "context": {"client": {
                "clientName": CLIENT_NAME,
                "clientVersion": CLIENT_VERSION,
                "hl": "en",
                "gl": "US"
            }},
            "query": query,
            "params": VIDEO_ONLY_PARAMS
        });
        crate::diagnostic_metrics::record_http_send_attempt();
        let mut response = client
            .post(endpoint)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .header(header::ORIGIN, "https://www.youtube.com")
            .header(header::REFERER, "https://www.youtube.com/")
            .json(&body)
            .send()
            .await
            .map_err(|_| "YouTube WEB search transport failed")?;

        if !response.status().is_success() {
            return Err("YouTube WEB search returned a non-success status");
        }
        if !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err("YouTube WEB search returned a non-JSON response");
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY_BYTES as u64)
        {
            return Err("YouTube WEB search response exceeded size limit");
        }

        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "YouTube WEB search body read failed")?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
                return Err("YouTube WEB search response exceeded size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_response(response.status(), &bytes)
    };

    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => Err("Cancelled"),
        _ = tokio::time::sleep_until(opportunity_deadline) => Err("YouTube WEB search deadline exceeded"),
        result = work => result,
    }
}

/// A provider failure or changed/empty layout preserves the existing helper.
/// Cancellation never starts a helper, including a cancellation arriving with
/// the native response. Both producers retain the caller's absolute deadline.
pub(crate) async fn search_or_fallback<N, L, LF>(
    native: N,
    legacy: L,
    cancel: watch::Receiver<bool>,
    limit: usize,
) -> Result<String, String>
where
    N: std::future::Future<Output = Result<Vec<YoutubeWebSearchCandidate>, &'static str>>,
    L: FnOnce() -> LF,
    LF: std::future::Future<Output = Result<String, String>>,
{
    let result = native.await;
    if *cancel.borrow() || matches!(result, Err("Cancelled")) {
        return Err("Cancelled".into());
    }
    match result {
        Ok(rows) if !rows.is_empty() => Ok(serde_json::json!({"entries":rows.into_iter().take(limit.min(MAX_ROWS)).map(|row| serde_json::json!({
            "id":row.video_id,"url":format!("https://www.youtube.com/watch?v={}",row.video_id),
            "title":row.title,"duration":row.duration_seconds,"uploader":row.uploader,
        })).collect::<Vec<_>>()}).to_string()),
        _ => legacy().await,
    }
}

fn safe_query_part(value: &str) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_QUERY_BYTES || value.chars().any(char::is_control) {
        return Err("Invalid YouTube WEB search query");
    }
    Ok(value)
}

fn parse_response(
    status: StatusCode,
    body: &[u8],
) -> Result<Vec<YoutubeWebSearchCandidate>, &'static str> {
    if !status.is_success() {
        return Err("YouTube WEB search returned a non-success status");
    }
    if body.len() > MAX_BODY_BYTES {
        return Err("YouTube WEB search response exceeded size limit");
    }
    let root: Value =
        serde_json::from_slice(body).map_err(|_| "YouTube WEB search JSON invalid")?;
    if root
        .pointer(
            "/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents",
        )
        .and_then(Value::as_array)
        .is_none()
    {
        return Err("YouTube WEB search layout changed");
    }
    let sections = root
        .pointer(
            "/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents",
        )
        .and_then(Value::as_array)
        .unwrap();
    let primary_videos = sections
        .iter()
        .filter_map(|section| {
            section
                .pointer("/itemSectionRenderer/contents")
                .and_then(Value::as_array)
        })
        .flatten()
        .filter(|row| row.get("videoRenderer").is_some())
        .cloned()
        .collect::<Vec<_>>();
    let rows = parse_items(&serde_json::json!({"items":primary_videos}));
    // Empty or changed layouts must fall back to the existing helper instead
    // of publishing an authoritative empty search cache.
    if rows.is_empty() {
        return Err("YouTube WEB search has no usable rows");
    }
    Ok(rows)
}

fn parse_items(root: &Value) -> Vec<YoutubeWebSearchCandidate> {
    let mut pending = vec![root];
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut visited = 0usize;
    // A 1 MiB JSON response is already bounded; this node cap protects callers
    // from unexpectedly broad but shallow renderer trees.
    const MAX_VISITED_NODES: usize = 100_000;
    while let Some(value) = pending.pop() {
        visited += 1;
        if visited > MAX_VISITED_NODES || rows.len() >= MAX_ROWS {
            break;
        }
        match value {
            Value::Object(map) => {
                if let Some(renderer) = map.get("videoRenderer").and_then(Value::as_object) {
                    let Some(video_id) = renderer.get("videoId").and_then(Value::as_str) else {
                        continue;
                    };
                    if !valid_video_id(video_id) || seen.contains(video_id) {
                        // Continue descending; nested renderers may still exist.
                    } else if let Some(title) = renderer
                        .get("title")
                        .and_then(display_text)
                        .and_then(safe_display_text)
                    {
                        let duration_seconds = if is_live_or_upcoming(renderer) {
                            None
                        } else {
                            renderer
                                .get("lengthSeconds")
                                .and_then(Value::as_u64)
                                .and_then(|seconds| u32::try_from(seconds).ok())
                                .filter(|seconds| *seconds <= 86_400)
                                .or_else(|| {
                                    renderer
                                        .get("lengthText")
                                        .and_then(display_text)
                                        .and_then(parse_duration)
                                })
                        };
                        let uploader = ["ownerText", "shortBylineText", "longBylineText"]
                            .iter()
                            .find_map(|key| renderer.get(*key).and_then(display_text))
                            .and_then(safe_display_text);
                        seen.insert(video_id.to_owned());
                        rows.push(YoutubeWebSearchCandidate {
                            video_id: video_id.to_owned(),
                            title,
                            duration_seconds,
                            uploader,
                        });
                    }
                }
                pending.extend(map.values().rev());
            }
            Value::Array(values) => pending.extend(values.iter().rev()),
            _ => {}
        }
    }
    rows
}

fn is_live_or_upcoming(renderer: &serde_json::Map<String, Value>) -> bool {
    ["isLive", "isLiveContent", "isUpcoming", "isUpcomingContent"]
        .iter()
        .any(|key| renderer.get(*key).and_then(Value::as_bool) == Some(true))
        || renderer
            .get("upcomingEventData")
            .is_some_and(|value| !value.is_null())
}

fn display_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("simpleText").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    let runs = value.get("runs")?.as_array()?;
    let mut joined = String::new();
    for run in runs {
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            joined.push_str(text);
        }
        if joined.len() > 512 {
            return None;
        }
    }
    (!joined.is_empty()).then_some(joined)
}

fn safe_display_text(text: String) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized.len() > 512 {
        None
    } else {
        Some(normalized)
    }
}

fn valid_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn parse_duration(text: String) -> Option<u32> {
    if text.is_empty() || text.len() > 32 || text.chars().any(char::is_control) {
        return None;
    }
    let parts = text
        .split(':')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None; // Includes LIVE / upcoming / malformed display strings.
            }
            part.parse::<u32>().ok()
        })
        .collect::<Option<Vec<_>>>()?;
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    if parts.len() == 2 {
        if parts[1] >= 60 {
            return None;
        }
    } else if parts[1] >= 60 || parts[2] >= 60 {
        return None;
    }
    let mut total = 0u32;
    for part in parts {
        total = total.checked_mul(60)?.checked_add(part)?;
    }
    (total <= 86_400).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn renderer(id: &str, title: Value, duration: Value, uploader: Value) -> Value {
        json!({"videoRenderer": {
            "videoId": id,
            "title": title,
            "lengthText": duration,
            "shortBylineText": uploader
        }})
    }

    #[test]
    fn extracts_bounded_provisional_rows_without_claiming_identity() {
        let root = json!({"contents": [renderer(
            "ps9X-DXUFGY",
            json!({"simpleText":"JOHN DENVER - MATTHEW."}),
            json!({"simpleText":"3:45"}),
            json!({"runs":[{"text":"Electricchris1"}]})
        )]});
        let rows = parse_items(&root);
        assert_eq!(
            rows,
            vec![YoutubeWebSearchCandidate {
                video_id: "ps9X-DXUFGY".into(),
                title: "JOHN DENVER - MATTHEW.".into(),
                duration_seconds: Some(225),
                uploader: Some("Electricchris1".into()),
            }]
        );
    }

    #[test]
    fn rejects_wrong_ids_and_deduplicates_exact_ids() {
        let good = renderer(
            "ps9X-DXUFGY",
            json!({"simpleText":"title"}),
            json!({"simpleText":"3:45"}),
            json!({"simpleText":"uploader"}),
        );
        let wrong = renderer(
            "not-an-id",
            json!({"simpleText":"title"}),
            json!({"simpleText":"3:45"}),
            json!({"simpleText":"uploader"}),
        );
        let root = json!({"items": [good.clone(), good, wrong]});
        let rows = parse_items(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].video_id, "ps9X-DXUFGY");
    }

    #[test]
    fn missing_and_live_durations_remain_unknown() {
        let mut explicitly_live = renderer(
            "M5NPZmqTOOU",
            json!({"simpleText":"live song"}),
            json!({"simpleText":"3:45"}),
            Value::Null,
        );
        explicitly_live["videoRenderer"]["isLiveContent"] = json!(true);
        let mut upcoming = renderer(
            "dM9lHTjt-mk",
            json!({"simpleText":"upcoming song"}),
            json!({"simpleText":"3:45"}),
            Value::Null,
        );
        upcoming["videoRenderer"]["upcomingEventData"] = json!({"startTime":"123"});
        let root = json!({"items": [
            renderer("ps9X-DXUFGY", json!({"simpleText":"song"}), json!({"simpleText":"LIVE"}), Value::Null),
            {"videoRenderer":{"videoId":"7zynorOnTXk","title":{"simpleText":"song"}}},
            explicitly_live,
            upcoming
        ]});
        let rows = parse_items(&root);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.duration_seconds.is_none()));
        assert!(parse_duration("LIVE".into()).is_none());
    }

    #[test]
    fn rejects_control_and_overlong_display_fields() {
        let control = renderer(
            "ps9X-DXUFGY",
            json!({"simpleText":"bad\u{0001}title"}),
            json!({"simpleText":"3:45"}),
            Value::Null,
        );
        let long = renderer(
            "7zynorOnTXk",
            json!({"simpleText":"x".repeat(513)}),
            json!({"simpleText":"3:45"}),
            Value::Null,
        );
        let root = json!({"items": [control, long]});
        assert!(parse_items(&root).is_empty());
    }

    #[test]
    fn caps_rows_and_rejects_oversized_or_bad_json_responses() {
        let values = (0..MAX_ROWS + 5)
            .map(|index| {
                renderer(
                    &format!("{index:011}"),
                    json!({"simpleText":"song"}),
                    json!({"simpleText":"1:00"}),
                    Value::Null,
                )
            })
            .collect::<Vec<_>>();
        let rows = parse_items(&json!({"items": values}));
        assert_eq!(rows.len(), MAX_ROWS);
        assert_eq!(
            rows.first().map(|row| row.video_id.as_str()),
            Some("00000000000")
        );
        assert_eq!(
            rows.last().map(|row| row.video_id.as_str()),
            Some("00000000029")
        );
        assert_eq!(
            parse_response(StatusCode::OK, &vec![b' '; MAX_BODY_BYTES + 1]),
            Err("YouTube WEB search response exceeded size limit")
        );
        assert_eq!(
            parse_response(StatusCode::OK, b"not-json"),
            Err("YouTube WEB search JSON invalid")
        );
        assert!(parse_response(StatusCode::BAD_GATEWAY, b"{}").is_err());
    }

    fn search_page(rows: Value) -> Value {
        json!({"contents":{"twoColumnSearchResultsRenderer":{"primaryContents":{"sectionListRenderer":{"contents":[{"itemSectionRenderer":{"contents":rows}}]}}}}})
    }

    #[test]
    fn response_uses_only_primary_video_rows_and_rejects_unknown_or_empty_layouts() {
        let good = renderer(
            "ps9X-DXUFGY",
            json!({"simpleText":"John Denver - Matthew"}),
            json!({"simpleText":"3:45"}),
            Value::Null,
        );
        let mut page =
            search_page(json!([good.clone(), {"adSlotRenderer":{"content":good.clone()}}]));
        page["sidebar"] = good;
        let rows = parse_response(StatusCode::OK, &serde_json::to_vec(&page).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(parse_response(StatusCode::OK, br#"{"sidebar":{"videoRenderer":{"videoId":"ps9X-DXUFGY","title":{"simpleText":"song"}}}}"#).is_err());
        assert!(parse_response(
            StatusCode::OK,
            &serde_json::to_vec(&search_page(json!([]))).unwrap()
        )
        .is_err());
    }

    async fn fixture(
        status: u16,
        content_type: &'static str,
        body: String,
        delay_ms: u64,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{response::IntoResponse, routing::post, Router};
        let app = Router::new().route(
            "/search",
            post(move || {
                let body = body.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        [("content-type", content_type)],
                        body,
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/search", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn http_boundaries_reject_status_html_and_oversize_without_a_helper() {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for (status, kind, body) in [
            (503, "application/json", "{}".into()),
            (302, "application/json", "{}".into()),
            (200, "text/html", "<html>login</html>".into()),
            (200, "application/json", " ".repeat(MAX_BODY_BYTES + 1)),
        ] {
            let (url, task) = fixture(status, kind, body, 0).await;
            let (_sender, cancel) = watch::channel(false);
            let result = search_candidates_at(
                &client,
                &url,
                "John Denver Matthew",
                cancel,
                crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(2)),
            )
            .await;
            task.abort();
            assert!(result.is_err(), "status={status}, kind={kind}");
        }
    }

    #[tokio::test]
    async fn request_preserves_literal_query_and_sends_no_account_credentials() {
        use axum::{routing::post, Json, Router};
        let (send, receive) = tokio::sync::oneshot::channel();
        let send = std::sync::Arc::new(std::sync::Mutex::new(Some(send)));
        let app = Router::new().route(
            "/search",
            post(
                move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                    let send = send.clone();
                    async move {
                        send.lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send((headers, body))
                            .unwrap();
                        Json(search_page(json!([renderer(
                            "ps9X-DXUFGY",
                            json!({"simpleText":"John Denver - Matthew"}),
                            json!({"simpleText":"3:45"}),
                            Value::Null
                        )])))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/search", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::builder().no_proxy().build().unwrap();
        let (_sender, cancel) = watch::channel(false);
        let rows = search_candidates_at(
            &client,
            &url,
            "\"Ro Myra\" and Lena Rich",
            cancel,
            crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(2)),
        )
        .await
        .unwrap();
        let (headers, body) = receive.await.unwrap();
        task.abort();
        assert_eq!(body["query"], "\"Ro Myra\" and Lena Rich");
        assert_eq!(body["params"], VIDEO_ONLY_PARAMS);
        assert!(!headers.contains_key("authorization"));
        assert!(!headers.contains_key("cookie"));
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn absolute_deadline_and_cancellation_drop_inflight_requests() {
        let client = Client::builder().no_proxy().build().unwrap();
        let (url, task) = fixture(200, "application/json", "{}".into(), 1000).await;
        let (_sender, cancel) = watch::channel(false);
        let started = std::time::Instant::now();
        let result = search_candidates_at(
            &client,
            &url,
            "song",
            cancel,
            crate::playback_sources::PlaybackDeadline::after(Duration::from_millis(30)),
        )
        .await;
        assert_eq!(result, Err("YouTube WEB search deadline exceeded"));
        assert!(started.elapsed() < Duration::from_millis(500));
        let (sender, cancel) = watch::channel(false);
        let request = search_candidates_at(
            &client,
            &url,
            "song",
            cancel,
            crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(2)),
        );
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            sender.send(true).unwrap();
        };
        let (result, ()) = tokio::join!(request, cancel);
        task.abort();
        assert_eq!(result, Err("Cancelled"));
    }

    #[tokio::test]
    async fn unavailable_and_empty_responses_fall_back_but_cancellation_never_launches_helper() {
        for native in [Err("YouTube WEB search transport failed"), Ok(vec![])] {
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let (_sender, cancel) = watch::channel(false);
            let result = search_or_fallback(
                async { native },
                || async {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok("legacy".into())
                },
                cancel,
                10,
            )
            .await;
            assert_eq!(result.unwrap(), "legacy");
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
        for late_cancel in [false, true] {
            let (sender, cancel) = watch::channel(false);
            let native = async {
                if late_cancel {
                    sender.send(true).unwrap();
                    Err("transport failed")
                } else {
                    Err("Cancelled")
                }
            };
            let result = search_or_fallback(
                native,
                || async { panic!("cancelled request launched helper") },
                cancel,
                10,
            )
            .await;
            assert_eq!(result, Err("Cancelled".into()));
        }
    }

    #[tokio::test]
    async fn successful_native_rows_remain_provisional_and_keep_the_foreground_cap() {
        let row = YoutubeWebSearchCandidate {
            video_id: "ps9X-DXUFGY".into(),
            title: "Matthew".into(),
            duration_seconds: Some(225),
            uploader: Some("Display name".into()),
        };
        let (_sender, cancel) = watch::channel(false);
        let result = search_or_fallback(
            async { Ok(vec![row.clone(), row]) },
            || async { panic!("successful native search launched helper") },
            cancel,
            1,
        )
        .await
        .unwrap();
        let page: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        let entry = &page["entries"][0];
        assert_eq!(entry["url"], "https://www.youtube.com/watch?v=ps9X-DXUFGY");
        assert_eq!(entry.as_object().unwrap().len(), 5);
        for key in [
            "artist",
            "channel_is_verified",
            "formats",
            "chapters",
            "source_metadata",
        ] {
            assert!(entry.get(key).is_none());
        }
    }

    #[test]
    fn parses_only_finite_clock_durations() {
        assert_eq!(parse_duration("3:45".into()), Some(225));
        assert_eq!(parse_duration("1:02:03".into()), Some(3723));
        assert_eq!(parse_duration("0:00".into()), Some(0));
        assert_eq!(parse_duration("99:99:99".into()), None);
        assert_eq!(parse_duration("1:99".into()), None);
        assert_eq!(parse_duration("1:60:00".into()), None);
        assert_eq!(parse_duration("1:00:60".into()), None);
        assert_eq!(parse_duration("upcoming".into()), None);
        assert_eq!(parse_duration("1:2:3:4".into()), None);
    }
}
