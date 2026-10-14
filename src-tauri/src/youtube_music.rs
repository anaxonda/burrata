//! Native YouTube Music search and source-page metadata extraction.
//! Search uses the public Innertube browse/search protocol. yt-dlp is only a
//! fallback for search and for inspecting a URL; neither path downloads audio.
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;

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
    v.as_str().map(str::to_owned).or_else(|| {
        v.get("simpleText")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                v.get("runs")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x["text"].as_str())
                            .collect::<String>()
                    })
                    .filter(|s| !s.is_empty())
            })
    })
}
fn video_id(v: &Value) -> Option<String> {
    v.get("videoId").and_then(Value::as_str).or_else(||v.pointer("/navigationEndpoint/watchEndpoint/videoId").and_then(Value::as_str)).or_else(||v.pointer("/playlistItemData/videoId").and_then(Value::as_str)).or_else(||v.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId").and_then(Value::as_str)).map(str::to_owned)
        .or_else(||v.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId").and_then(Value::as_str).map(str::to_owned))
}
fn labels(v: &Value) -> Vec<String> {
    let source = if v.is_string() {
        vec![v.clone()]
    } else {
        v.as_array().cloned().unwrap_or_default()
    };
    source.iter().into_iter().filter_map(text).collect()
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
    run["text"].as_str().map(str::to_owned)
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
            let artist = semantic_runs
                .iter()
                .find(|r| run_page_type(r) == Some("MUSIC_PAGE_TYPE_ARTIST"))
                .and_then(|r| run_label(r))
                .or_else(|| {
                    columns
                        .get(1)
                        .and_then(|c| flex_runs(c).first())
                        .and_then(run_label)
                });
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
            if !title.is_empty() {
                out.push(YouTubeMusicItem {
                    url: format!("https://music.youtube.com/watch?v={id}"),
                    id,
                    title,
                    artist,
                    album,
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
fn parse_duration(s: &str) -> Option<f64> {
    if !s.contains(':') {
        return None;
    }
    s.split(':')
        .try_fold(0.0, |n, p| p.parse::<f64>().ok().map(|x| n * 60.0 + x))
}

/// Supports regular text and explicit ISRC queries. Community endpoint failures
/// fall back to the caller's configured yt-dlp executable.
pub async fn search(
    query: &str,
    isrc: Option<&str>,
    yt_dlp: &str,
) -> Result<(Vec<YouTubeMusicItem>, Vec<String>), String> {
    let q = isrc
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("{s} {query}"))
        .unwrap_or_else(|| query.to_owned());
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 Burrata/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    // Public search works without an embedded API key. Keep credentials out of
    // both the request URL and distributed source/binaries.
    let response = client.post("https://music.youtube.com/youtubei/v1/search").json(&json!({"context":{"client":{"clientName":"WEB_REMIX","clientVersion":"1.20250101.01.00","hl":"en","gl":"US"}},"query":q,"params":"EgWKAQIIAWoKEAkQChADEAQQBQ%3D%3D"})).send().await;
    if let Ok(response) = response {
        if response.status().is_success() {
            if let Ok(body) = response.json::<Value>().await {
                let items = parse_search_response(&body, 30);
                if !items.is_empty() {
                    return Ok((items, vec![]));
                }
            }
        }
    }
    let mut warnings =
        vec!["YouTube Music community search was unavailable; used yt-dlp search fallback".into()];
    let output = Command::new(yt_dlp)
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
    let items = entries
        .filter_map(|e| {
            let id = e["id"].as_str()?.to_owned();
            Some(YouTubeMusicItem {
                url: e["url"]
                    .as_str()
                    .map(str::to_owned)
                    .filter(|u| u.starts_with("http"))
                    .unwrap_or_else(|| format!("https://music.youtube.com/watch?v={id}")),
                id,
                title: e["title"].as_str()?.to_owned(),
                artist: e["artist"].as_str().map(str::to_owned),
                album: e["album"].as_str().map(str::to_owned),
                duration_seconds: e["duration"].as_f64(),
                isrc: e["isrc"].as_str().map(str::to_owned),
                artwork_url: e["thumbnail"].as_str().map(str::to_owned),
            })
        })
        .collect();
    warnings.shrink_to_fit();
    Ok((items, warnings))
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
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        Command::new(crate::tools::executable(yt_dlp, "yt-dlp"))
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
    let id = v["id"].as_str().unwrap_or_default().to_owned();
    Ok(YouTubeSourceMetadata {
        id,
        url: v["webpage_url"].as_str().unwrap_or(url).to_owned(),
        title: v["title"].as_str().unwrap_or_default().to_owned(),
        description: v["description"].as_str().map(str::to_owned),
        channel: v["channel"]
            .as_str()
            .or(v["uploader"].as_str())
            .map(str::to_owned),
        channel_url: v["channel_url"]
            .as_str()
            .or(v["uploader_url"].as_str())
            .map(str::to_owned),
        thumbnail_url: v["thumbnail"].as_str().map(str::to_owned),
        artist: v["artist"].as_str().map(str::to_owned),
        song: v["track"].as_str().map(str::to_owned),
        album: v["album"].as_str().map(str::to_owned),
        source_genres: labels(&v["genres"]),
        upload_date: v["upload_date"].as_str().map(str::to_owned),
        release_date: v["release_date"].as_str().map(str::to_owned),
        duration_seconds: v["duration"].as_f64(),
        chapters: v["chapters"].as_array().cloned().unwrap_or_default(),
        catalog_verified: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_music_video_renderer_fixture_and_deduplicates() {
        let artist_endpoint = json!({"browseEndpoint":{"browseEndpointContextSupportedConfigs":{"browseEndpointContextMusicConfig":{"pageType":"MUSIC_PAGE_TYPE_ARTIST"}}}});
        let album_endpoint = json!({"browseEndpoint":{"browseEndpointContextSupportedConfigs":{"browseEndpointContextMusicConfig":{"pageType":"MUSIC_PAGE_TYPE_ALBUM"}}}});
        let renderer = json!({"flexColumns":[{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Mary","navigationEndpoint":{"watchEndpoint":{"videoId":"mary-video-id"}}}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Big Thief","navigationEndpoint":artist_endpoint}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Capacity","navigationEndpoint":album_endpoint}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"3.8M plays"}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"5:31"}]}}}],"thumbnail":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[{"url":"https://img.example/cover.jpg"}]}}}});
        let fixture = json!({"contents":{"musicResponsiveListItemRenderer":renderer}});
        let parsed = parse_search_response(&fixture, 10);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "mary-video-id");
        assert_eq!(parsed[0].title, "Mary");
        assert_eq!(parsed[0].artist.as_deref(), Some("Big Thief"));
        assert_eq!(parsed[0].album.as_deref(), Some("Capacity"));
        assert_eq!(parsed[0].duration_seconds, Some(331.0));
        assert_eq!(
            parsed[0].artwork_url.as_deref(),
            Some("https://img.example/cover.jpg")
        );
    }
}
