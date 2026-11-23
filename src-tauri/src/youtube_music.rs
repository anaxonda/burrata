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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub views: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_count_text: Option<String>,
    #[serde(default)]
    pub artist_credits: Vec<YouTubeMusicArtist>,
    #[serde(default)]
    pub album_browse_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeMusicArtist {
    pub name: String,
    pub provider_id: Option<String>,
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

fn view_count(value: &Value) -> Option<u64> {
    const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
    value
        .as_u64()
        .or_else(|| {
            let digits = value.as_str()?;
            digits
                .bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| digits.parse::<u64>().ok())
                .flatten()
        })
        .filter(|count| *count <= MAX_SAFE_INTEGER)
}

fn view_label(label: String) -> Option<(Option<u64>, Option<String>)> {
    if label.len() > 64 || label.chars().any(char::is_control) {
        return None;
    }
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    static VIEWS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^(?:([0-9]+|[0-9]{1,3}(?:,[0-9]{3})+)|([0-9]+(?:\.[0-9]+)?[KMBT])) views?$",
        )
        .expect("valid YouTube view label pattern")
    });
    let captures = VIEWS.captures(&label)?;
    if let Some(exact) = captures.get(1) {
        return view_count(&Value::String(exact.as_str().replace(',', "")))
            .map(|count| (Some(count), None));
    }
    Some((None, Some(label)))
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
            let columns = v["flexColumns"].as_array().map_or(&[][..], Vec::as_slice);
            let explicit_title = text(&v["title"]);
            let column_title = columns.first().and_then(renderer_text);
            let first_column_is_title = explicit_title.is_none() || explicit_title == column_title;
            let title = explicit_title.or(column_title).unwrap_or_default();
            let semantic_runs = columns
                .iter()
                .skip(usize::from(first_column_is_title))
                .flat_map(|c| flex_runs(c).iter())
                .collect::<Vec<_>>();
            let mut artist_names = Vec::new();
            let mut artist_credits = Vec::new();
            for run in semantic_runs
                .iter()
                .filter(|r| run_page_type(r) == Some("MUSIC_PAGE_TYPE_ARTIST"))
            {
                if let Some(name) = run_label(run) {
                    if !artist_names.contains(&name) {
                        artist_names.push(name.clone());
                        let provider_id = run
                            .pointer("/navigationEndpoint/browseEndpoint/browseId")
                            .and_then(Value::as_str)
                            .filter(|id| {
                                id.starts_with("UC")
                                    && id.len() == 24
                                    && id.bytes().all(|b| {
                                        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
                                    })
                            })
                            .map(str::to_owned);
                        artist_credits.push(YouTubeMusicArtist { name, provider_id });
                    }
                }
            }
            let artist = (!artist_names.is_empty()).then(|| artist_names.join(", "));
            let album = semantic_runs
                .iter()
                .find(|r| run_page_type(r) == Some("MUSIC_PAGE_TYPE_ALBUM"))
                .and_then(|r| run_label(r));
            let album_browse_id = semantic_runs
                .iter()
                .find(|r| run_page_type(r) == Some("MUSIC_PAGE_TYPE_ALBUM"))
                .and_then(|run| run.pointer("/navigationEndpoint/browseEndpoint/browseId"))
                .and_then(Value::as_str)
                .filter(|id| {
                    id.starts_with("MPREb_")
                        && id.len() <= 128
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                })
                .map(str::to_owned);
            let duration = v
                .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text")
                .and_then(text)
                .as_deref()
                .and_then(parse_duration)
                .or_else(|| {
                    let mut clocks = semantic_runs
                        .iter()
                        .filter(|run| run["navigationEndpoint"].is_null())
                        .filter_map(|run| run_label(run).and_then(|label| parse_duration(&label)));
                    let first = clocks.next()?;
                    clocks.all(|other| first == other).then_some(first)
                });
            let artwork = v
                .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
                .or_else(|| v.pointer("/thumbnail/thumbnails"))
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .and_then(|i| i["url"].as_str())
                .map(str::to_owned);
            let (views, view_count_text) = if let Some(count) = view_count(&v["viewCount"]) {
                (Some(count), None)
            } else {
                ["viewCountText", "shortViewCountText"]
                    .iter()
                    .filter_map(|key| text(&v[*key]))
                    .chain(
                        semantic_runs
                            .iter()
                            .filter(|run| run["navigationEndpoint"].is_null())
                            .filter_map(|run| run_label(run)),
                    )
                    .find_map(view_label)
                    .unwrap_or_default()
            };
            if !title.trim().is_empty() {
                out.push(YouTubeMusicItem {
                    url: format!("https://music.youtube.com/watch?v={id}"),
                    id,
                    title,
                    artist: artist.filter(|value| !value.trim().is_empty()),
                    album: album.filter(|value| !value.trim().is_empty()),
                    duration_seconds: duration,
                    isrc: None,
                    views,
                    view_count_text,
                    artwork_url: artwork,
                    artist_credits,
                    album_browse_id,
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
    if !(1..=30).contains(&limit) {
        return Err("YouTube Music playback search limit must be between 1 and 30".into());
    }
    let body = playback_search_json(
        endpoint,
        query,
        Some("EgWKAQIIAWoKEAkQChADEAQQBQ%3D%3D"),
        cancel,
        deadline,
    )
    .await?;
    Ok(parse_search_response(&body, limit))
}

async fn playback_search_json(
    endpoint: &str,
    query: &str,
    params: Option<&str>,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<Value, String> {
    let query = query.trim();
    if query.is_empty()
        || query.len() > MAX_PLAYBACK_SEARCH_QUERY_BYTES
        || query.chars().any(char::is_control)
    {
        return Err("YouTube Music playback search query is invalid".into());
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
    let mut payload = json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": "1.20250101.01.00",
                "hl": "en",
                "gl": "US"
            }
        },
        "query": query,
    });
    if let Some(params) = params {
        payload["params"] = json!(params);
    }
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
    serde_json::from_slice(&body)
        .map_err(|_| "YouTube Music playback search returned invalid JSON".to_owned())
}

/// Resolve only an exact, uniquely identified album with the complete linked
/// roster. The album's tracks are still locators for fresh audio verification.
pub(crate) async fn numbered_movement_album_tracks(
    title: &str,
    fingerprint: &crate::spotify_catalog_identity::SpotifyArtistCreditFingerprint,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<Vec<YouTubeMusicItem>, String> {
    let lead = album_search_lead(title, fingerprint).unwrap_or(&fingerprint.artist);
    let query_title = title
        .split_once(" (")
        .map_or(title, |(main, _)| main)
        .replace(';', " ");
    let query = format!("{lead} {query_title}");
    let body =
        playback_search_json(PLAYBACK_SEARCH_ENDPOINT, &query, None, cancel, deadline).await?;
    let Some((browse_id, artists)) = exact_numbered_movement_album(&body, title, fingerprint)
    else {
        return Ok(Vec::new());
    };
    let key = crate::matching::normalize_single_artist_name;
    let roster_matches = |credits: &[YouTubeMusicArtist]| {
        credits.len() == fingerprint.artists.len()
            && credits.iter().all(|credit| {
                credit
                    .provider_id
                    .as_deref()
                    .is_some_and(crate::youtube_recording_metadata::channel_id)
            })
            && fingerprint.artists.iter().all(|expected| {
                credits
                    .iter()
                    .any(|credit| key(&credit.name) == key(&expected.name))
            })
    };
    let mut results = Vec::new();
    for mut item in album_tracks(&browse_id).await? {
        if !item.duration_seconds.is_some_and(|duration| {
            crate::spotify_catalog_identity::numbered_movement_caption_matches(
                fingerprint,
                &item.title,
                duration,
            )
        }) {
            continue;
        }
        if item.artist_credits.is_empty() && item.artist.is_none() {
            // Some shelves display the linked album credits only once. The
            // extractor still has to corroborate every movement credit.
            if !roster_matches(&artists) {
                continue;
            }
            item.artist_credits = artists.clone();
            item.artist = Some(
                artists
                    .iter()
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !roster_matches(&item.artist_credits) {
            continue;
        }
        item.album = Some(title.to_owned());
        item.album_browse_id = Some(browse_id.clone());
        results.push(item);
    }
    Ok(results)
}

fn album_search_lead<'a>(
    title: &str,
    fingerprint: &'a crate::spotify_catalog_identity::SpotifyArtistCreditFingerprint,
) -> Option<&'a str> {
    let prefix = title.split_once(": ").map(|(prefix, _)| prefix);
    fingerprint
        .artists
        .iter()
        .find(|artist| {
            prefix.is_none_or(|prefix| {
                !crate::spotify_catalog_identity::name_matches_caption_prefix(prefix, &artist.name)
            })
        })
        .map(|artist| artist.name.as_str())
}

fn exact_numbered_movement_album(
    body: &Value,
    title: &str,
    fingerprint: &crate::spotify_catalog_identity::SpotifyArtistCreditFingerprint,
) -> Option<(String, Vec<YouTubeMusicArtist>)> {
    fn collect<'a>(value: &'a Value, rows: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                if let Some(row) = map.get("musicResponsiveListItemRenderer") {
                    rows.push(row);
                } else {
                    for child in map.values() {
                        collect(child, rows);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect(item, rows)),
            _ => {}
        }
    }
    let mut rows = Vec::new();
    collect(body, &mut rows);
    let key = crate::matching::normalize_single_artist_name;
    let expected = fingerprint
        .artists
        .iter()
        .map(|row| key(&row.name))
        .collect::<std::collections::HashSet<_>>();
    let lead = album_search_lead(title, fingerprint).map(key)?;
    let mut exact = std::collections::BTreeMap::new();
    let mut lead_matches = std::collections::BTreeMap::new();
    for row in rows {
        if run_page_type(row) != Some("MUSIC_PAGE_TYPE_ALBUM") {
            continue;
        }
        let Some(columns) = row["flexColumns"].as_array() else {
            continue;
        };
        if columns.first().and_then(renderer_text).as_deref() != Some(title) {
            continue;
        }
        let Some(browse_id) = row
            .pointer("/navigationEndpoint/browseEndpoint/browseId")
            .and_then(Value::as_str)
            .filter(|id| valid_playback_album_id(id))
        else {
            continue;
        };
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        let artists = columns
            .iter()
            .skip(1)
            .flat_map(flex_runs)
            .filter(|run| run_page_type(run) == Some("MUSIC_PAGE_TYPE_ARTIST"))
            .map(|run| {
                let name = run_label(run)?;
                let id = run
                    .pointer("/navigationEndpoint/browseEndpoint/browseId")
                    .and_then(Value::as_str)
                    .filter(|id| crate::youtube_recording_metadata::channel_id(id))?;
                (ids.insert(id) && names.insert(key(&name))).then(|| YouTubeMusicArtist {
                    name,
                    provider_id: Some(id.to_owned()),
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(artists) = artists else { continue };
        let editions = if names == expected {
            &mut exact
        } else if names.contains(&lead) {
            &mut lead_matches
        } else {
            continue;
        };
        if editions
            .get(browse_id)
            .is_some_and(|existing| existing != &artists)
        {
            return None;
        }
        editions.insert(browse_id.to_owned(), artists);
    }
    let editions = if exact.is_empty() {
        lead_matches
    } else {
        exact
    };
    (editions.len() == 1)
        .then(|| editions.into_iter().next())
        .flatten()
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
    fn number(part: &str) -> Option<u32> {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    }
    let (leading, seconds) = s.rsplit_once(':')?;
    let seconds = (seconds.len() == 2).then(|| number(seconds)).flatten()?;
    if seconds >= 60 {
        return None;
    }
    let total = if let Some((hours, minutes)) = leading.split_once(':') {
        let minutes = (minutes.len() == 2).then(|| number(minutes)).flatten()?;
        if minutes >= 60 {
            return None;
        }
        u64::from(number(hours)?) * 3600 + u64::from(minutes) * 60 + u64::from(seconds)
    } else {
        u64::from(number(leading)?) * 60 + u64::from(seconds)
    };
    (total > 0).then_some(total as f64)
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
                views: view_count(&e["view_count"]),
                view_count_text: None,
                artwork_url: nonempty_str(&e["thumbnail"]),
                artist_credits: Vec::new(),
                album_browse_id: None,
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

    #[test]
    fn numbered_movement_album_search_requires_one_exact_linked_edition() {
        let track = crate::spotify_catalog_identity::test_fixtures::two_performer_track();
        let fingerprint =
            crate::spotify_catalog_identity::spotify_artist_credit_fingerprint(&track).unwrap();
        let link = |id: &str, page_type: &str| {
            json!({
                "browseEndpoint": {"browseId":id,
                    "browseEndpointContextSupportedConfigs": {
                        "browseEndpointContextMusicConfig": {"pageType":page_type}
                    }
                }
            })
        };
        let title = "Original album";
        let row = |id: &str, arranger: &str| {
            json!({
                "musicResponsiveListItemRenderer": {
                    "navigationEndpoint":link(id, "MUSIC_PAGE_TYPE_ALBUM"),
                    "flexColumns":[
                        {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":title}]}}},
                        {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[
                            {"text":"Album"},
                            {"text":"Choir Alpha","navigationEndpoint":link("UC1111111111111111111111", "MUSIC_PAGE_TYPE_ARTIST")},
                            {"text":arranger,"navigationEndpoint":link("UC2222222222222222222222", "MUSIC_PAGE_TYPE_ARTIST")}
                        ]}}}
                    ]
                }
            })
        };
        let good = row("MPREb_Original", "Choir Beta");
        let wrong = row("MPREb_OtherPerformance", "Choir Gamma");
        let body = json!({"contents":[good.clone(), wrong]});
        let (id, artists) = exact_numbered_movement_album(&body, title, &fingerprint).unwrap();
        assert_eq!(id, "MPREb_Original");
        assert_eq!(artists[1].name, "Choir Beta");
        let duplicate = json!({"contents":[good, row("MPREb_OtherEdition", "Choir Beta")]});
        assert!(exact_numbered_movement_album(&duplicate, title, &fingerprint).is_none());
        assert!(exact_numbered_movement_album(&body, "Other album", &fingerprint).is_none());
    }

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

    #[test]
    fn album_shelf_rejects_continuations_missing_rows_and_multiple_shelves() {
        let row = |id: &str| {
            json!({"musicResponsiveListItemRenderer":{
                "videoId":id,"title":{"simpleText":"Song"},
                "fixedColumns":[{"musicResponsiveListItemFixedColumnRenderer":{"text":{"runs":[{"text":"3:56"}]}}}]
            }})
        };
        let shelf = json!({"contents":[row("xAJxfiF_0-w"),row("H35xiJcS8RY")]});
        let body = json!({"musicShelfRenderer":shelf});
        assert_eq!(complete_album_shelf(&body).unwrap().len(), 2);
        let mut continued = body.clone();
        continued["musicShelfRenderer"]["continuations"] = json!([{"token":"more"}]);
        assert!(complete_album_shelf(&continued).is_none());
        let mut missing = body.clone();
        missing["musicShelfRenderer"]["contents"][1] = json!({"unavailable":true});
        assert!(complete_album_shelf(&missing).is_none());
        assert!(complete_album_shelf(&json!([body.clone(), body])).is_none());
    }

    #[test]
    fn captured_playback_search_retains_official_album_and_distinct_live_version() {
        let f: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/macha-official-source-selection.json"
        ))
        .unwrap();
        let items = parse_search_response(&f["nativeSearch"], 20);
        assert_eq!(items.len(), 3);
        let expected: YouTubeMusicItem = serde_json::from_value(f["catalogItem"].clone()).unwrap();
        assert_eq!(items[0].id, expected.id);
        assert_eq!(items[0].artist_credits, expected.artist_credits);
        assert_eq!(items[0].album_browse_id, expected.album_browse_id);
        assert_eq!(items[0].album, expected.album);
        assert_eq!(items[0].duration_seconds, Some(236.));
        assert_eq!(items[1].album.as_deref(), Some("Lado A"));
        assert!(items[2].title.contains("En Vivo"));
    }

    #[test]
    fn structured_album_and_artist_endpoints_survive_search_parsing() {
        let run = |name: &str, kind: &str, id: &str| {
            json!({
                "text":name,"navigationEndpoint":{"browseEndpoint":{
                    "browseId":id,"browseEndpointContextSupportedConfigs":{
                        "browseEndpointContextMusicConfig":{"pageType":kind}
                    }
                }}
            })
        };
        let body = json!({"musicResponsiveListItemRenderer":{
            "videoId":"xAJxfiF_0-w",
            "flexColumns":[
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"La Nave del Olvido"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[
                    run("Main Act","MUSIC_PAGE_TYPE_ARTIST","UCabcdefghijklmnopqrstuv"),
                    {"text":" • "},
                    run("Guest","MUSIC_PAGE_TYPE_ARTIST","UCABCDEFGHIJKLMNOPQRSTUV"),
                    run("Album title","MUSIC_PAGE_TYPE_ALBUM","MPREb_fixture123")
                ]}}}
            ]
        }});
        let items = parse_search_response(&body, 30);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].artist.as_deref(), Some("Main Act, Guest"));
        assert_eq!(items[0].artist_credits.len(), 2);
        assert_eq!(
            items[0].artist_credits[0].provider_id.as_deref(),
            Some("UCabcdefghijklmnopqrstuv")
        );
        assert_eq!(items[0].album.as_deref(), Some("Album title"));
        assert_eq!(
            items[0].album_browse_id.as_deref(),
            Some("MPREb_fixture123")
        );
        let restored: YouTubeMusicItem =
            serde_json::from_value(serde_json::to_value(&items[0]).unwrap()).unwrap();
        assert_eq!(restored, items[0]);
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

    #[test]
    fn playback_locator_reads_title_runs_and_prefers_the_duration_column() {
        for title_runs in [
            json!([{"text":"4:33"}]),
            json!([{"text":"4:"},{"text":"33"}]),
        ] {
            let mut row = json!({
                "videoId":"validvideo1",
                "flexColumns":[
                    {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":title_runs}}},
                    {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[
                        {"text":"5:01","navigationEndpoint":{"browseEndpoint":{"browseId":"album"}}},
                        {"text":"9:99"}
                    ]}}}
                ],
                "fixedColumns":[{"musicResponsiveListItemFixedColumnRenderer":{"text":{"simpleText":"4:54"}}}]
            });
            let items = parse_search_response(&row, 10);
            assert_eq!(items[0].title, "4:33");
            assert_eq!(items[0].duration_seconds, Some(294.0));

            row["fixedColumns"] = Value::Null;
            let items = parse_search_response(&row, 10);
            assert_eq!(items[0].title, "4:33");
            assert_eq!(items[0].duration_seconds, None);
            row["flexColumns"][1]["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"][1]
                ["text"] = json!("1:02:03");
            assert_eq!(
                parse_search_response(&row, 10)[0].duration_seconds,
                Some(3723.0)
            );
        }
    }

    #[test]
    fn music_search_keeps_exact_views_and_compact_labels_distinct() {
        let mut row = json!({
            "videoId":"validvideo1",
            "title":{"simpleText":"12 Views"},
            "viewCountText":{"runs":[{"text":"1,234 "},{"text":"views"}]},
            "shortViewCountText":{"simpleText":"1.2K views"},
            "flexColumns":[
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"12 Views"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[
                    {"text":"40 views","navigationEndpoint":{"browseEndpoint":{"browseId":"album"}}},
                    {"text":"12 watching"}
                ]}}}
            ]
        });
        let item = |row: &Value| parse_search_response(row, 1).remove(0);
        assert_eq!(item(&row).views, Some(1234));
        assert!(item(&row).view_count_text.is_none());

        row["viewCount"] = json!(0);
        assert_eq!(item(&row).views, Some(0));
        row["viewCount"] = json!(9_007_199_254_740_992u64);
        row["viewCountText"] = Value::Null;
        assert_eq!(item(&row).views, None);
        assert_eq!(item(&row).view_count_text.as_deref(), Some("1.2K views"));

        row["shortViewCountText"] = Value::Null;
        assert_eq!(item(&row).views, None);
        assert!(item(&row).view_count_text.is_none());
        row["flexColumns"][1]["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"][1]
            ["text"] = json!("7,321 views");
        assert_eq!(item(&row).views, Some(7321));
    }
}

/// Read one complete, bounded album shelf. Recommendations and continuation
/// pages cannot establish an album timeline.
pub(crate) async fn album_tracks(browse_id: &str) -> Result<Vec<YouTubeMusicItem>, String> {
    if !valid_playback_album_id(browse_id) {
        return Err("Invalid YouTube Music album identity".into());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client.post("https://music.youtube.com/youtubei/v1/browse")
        .json(&json!({"context":{"client":{"clientName":"WEB_REMIX","clientVersion":"1.20250101.01.00","hl":"en","gl":"US"}},"browseId":browse_id}))
        .send().await.map_err(|_| "YouTube album metadata could not be read")?;
    if !response.status().is_success() {
        return Err("YouTube album metadata is unavailable".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "YouTube album metadata could not be read")?
    {
        if bytes.len() + chunk.len() > MAX_PLAYBACK_SEARCH_BYTES {
            return Err("YouTube album metadata exceeds its size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid YouTube album metadata")?;
    complete_album_shelf(&body)
        .ok_or_else(|| "YouTube did not return one complete album tracklist".into())
}

fn valid_playback_album_id(id: &str) -> bool {
    id.starts_with("MPREb_")
        && id.len() > "MPREb_".len()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn complete_album_shelf(body: &Value) -> Option<Vec<YouTubeMusicItem>> {
    fn collect<'a>(value: &'a Value, shelves: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if matches!(
                        key.as_str(),
                        "musicShelfRenderer" | "musicPlaylistShelfRenderer"
                    ) {
                        shelves.push(child);
                    } else {
                        collect(child, shelves);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(value, shelves);
                }
            }
            _ => {}
        }
    }
    let mut shelves = Vec::new();
    collect(body, &mut shelves);
    let mut complete = Vec::new();
    for shelf in shelves {
        let Some(rows) = shelf["contents"].as_array() else {
            continue;
        };
        if !(2..=50).contains(&rows.len())
            || shelf
                .get("continuations")
                .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
            || rows
                .iter()
                .any(|row| row.get("musicResponsiveListItemRenderer").is_none())
        {
            continue;
        }
        let tracks = parse_search_response(shelf, 50);
        if tracks.len() == rows.len()
            && tracks.iter().all(|track| {
                track
                    .duration_seconds
                    .is_some_and(|d| d.is_finite() && d > 0.0)
            })
        {
            complete.push(tracks);
        }
    }
    (complete.len() == 1).then(|| complete.remove(0))
}
