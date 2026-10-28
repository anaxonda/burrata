//! On-demand LRCLIB lookup for the current track only.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use url::Url;

const ENDPOINT: &str = "https://lrclib.net/api/get";
const SEARCH_ENDPOINT: &str = "https://lrclib.net/api/search";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_LYRICS_BYTES: usize = 64 * 1024;
const MAX_LINES: usize = 800;
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_LIMIT: usize = 128;

#[derive(Debug, Clone)]
pub struct LyricsRequest {
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration_seconds: f64,
}

/// Metadata from the visible player item. WebView playback does not always
/// publish a current item through Rust's native playback snapshot, so the UI
/// supplies its current item when that snapshot is idle. The UI discards late
/// responses when its playback scope changes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsRequestInput {
    #[serde(default)]
    pub track_id: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration_seconds: f64,
    #[serde(default)]
    pub playback_range: Option<Value>,
    #[serde(default)]
    pub radio: bool,
}

impl From<LyricsRequestInput> for LyricsRequest {
    fn from(input: LyricsRequestInput) -> Self {
        Self {
            track_name: input.track_name,
            artist_name: input.artist_name,
            album_name: input.album_name,
            duration_seconds: input.duration_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    pub id: String,
    /// Absolute source timestamp, or `None` for an unsynchronized paragraph.
    pub time_seconds: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsMatch {
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration_seconds: f64,
    pub duration_delta_seconds: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsResponse {
    pub status: String,
    pub provider: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub matched: Option<LyricsMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<LyricsLine>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plain_lyrics: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_offset_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiRecord {
    // LRCLIB sends both `name` and `trackName`. `name` must remain an
    // ignored extra field, not an alias that makes serde reject duplicates.
    pub(crate) track_name: String,
    pub(crate) artist_name: String,
    pub(crate) album_name: Option<String>,
    pub(crate) duration: f64,
    pub(crate) instrumental: bool,
    pub(crate) synced_lyrics: Option<String>,
    pub(crate) plain_lyrics: Option<String>,
}

#[derive(Clone)]
struct Cached {
    inserted: Instant,
    value: LyricsResponse,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalized(value: &str) -> String {
    crate::matching::normalize(value)
}

fn valid_text(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub(crate) fn request_key(request: &LyricsRequest) -> String {
    format!(
        "{}\0{}\0{}\0{:.3}",
        normalized(&request.track_name),
        normalized(&request.artist_name),
        request
            .album_name
            .as_deref()
            .map(normalized)
            .unwrap_or_default(),
        request.duration_seconds
    )
}

pub(crate) fn response(status: &str) -> LyricsResponse {
    LyricsResponse {
        status: status.into(),
        provider: "lrclib",
        source_hash: None,
        matched: None,
        lines: None,
        plain_lyrics: None,
        source_offset_seconds: None,
    }
}

/// Encode source lyrics as UTF-8 text in the format-native lyrics tag. Timed
/// sources retain LRC timestamps and offset; related choices remain plain text.
pub(crate) fn embedding_text(document: &LyricsResponse) -> Option<String> {
    if !matches!(document.status.as_str(), "synced" | "unsynced") {
        return None;
    }
    let lines = document.lines.as_ref()?;
    let mut text = if document.status == "synced" {
        lines
            .iter()
            .map(|line| {
                let centis = (line.time_seconds.unwrap_or(0.0) * 100.0).round() as u64;
                format!(
                    "[{:02}:{:02}.{:02}]{}",
                    centis / 6000,
                    (centis / 100) % 60,
                    centis % 100,
                    line.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        document.plain_lyrics.clone().unwrap_or_else(|| {
            lines
                .iter()
                .map(|l| l.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        })
    };
    if document.status == "synced" {
        if let Some(offset) = document.source_offset_seconds {
            if offset.is_finite() && offset.abs() <= 300.0 && offset != 0.0 {
                text = format!("[offset:{:+}]\n{text}", (offset * 1000.0).round() as i64);
            }
        }
    }
    (!text.trim().is_empty() && text.len() <= 65536 && !text.contains('\0')).then_some(text)
}

/// Reuse lyrics already returned by an explicit SongRec identification. This
/// never starts recognition or fetches an unrelated public-page lyric block.
static SHAZAM_LYRICS: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
fn shazam_cache() -> &'static Mutex<HashMap<String, Cached>> {
    SHAZAM_LYRICS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn shazam_pair(title: &str, artist: &str) -> String {
    format!(
        "{}\0{}",
        crate::matching::normalize(title),
        crate::matching::normalize(artist)
    )
}
pub(crate) fn retain_shazam_lyrics(track: &Value) {
    let Some(title) = track["title"].as_str().filter(|text| valid_text(text)) else {
        return;
    };
    let Some(artist) = track["subtitle"].as_str().filter(|text| valid_text(text)) else {
        return;
    };
    let Some(id) = track["key"].as_str() else {
        return;
    };
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|c| c.is_ascii_digit()) {
        return;
    }
    let Some(sections) = track["sections"].as_array().filter(|s| s.len() <= 32) else {
        return;
    };
    let Some(section) = sections
        .iter()
        .find(|s| s["type"].as_str() == Some("LYRICS"))
    else {
        return;
    };
    let Some(text) = section["text"]
        .as_array()
        .filter(|lines| !lines.is_empty() && lines.len() <= MAX_LINES)
    else {
        return;
    };
    let mut paragraphs = Vec::new();
    let mut bytes = 0usize;
    for value in text {
        let Some(line) = value.as_str() else {
            return;
        };
        bytes = bytes.saturating_add(line.len()).saturating_add(1);
        if bytes > MAX_LYRICS_BYTES || line.chars().any(|c| c.is_control()) {
            return;
        }
        paragraphs.push(line);
    }
    let plain = paragraphs.join("\n");
    if plain.trim().is_empty() {
        return;
    }
    let lines = parse_plain_lines(&plain);
    let value = LyricsResponse {
        status: "unsynced".into(),
        provider: "shazam",
        source_hash: Some(source_hash_for_lines(&lines)),
        matched: None,
        lines: Some(lines),
        plain_lyrics: Some(plain),
        source_offset_seconds: None,
    };
    if let Ok(mut cache) = shazam_cache().lock() {
        cache.retain(|_, value| value.inserted.elapsed() < CACHE_TTL);
        if cache.len() >= 64 {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }
        cache.insert(
            shazam_pair(title, artist),
            Cached {
                inserted: Instant::now(),
                value,
            },
        );
    }
}
fn shazam_fallback(request: &LyricsRequest, primary: LyricsResponse) -> LyricsResponse {
    if !matches!(
        primary.status.as_str(),
        "notFound" | "unmatched" | "unavailable" | "providerUnavailable"
    ) {
        return primary;
    }
    shazam_cache()
        .lock()
        .ok()
        .and_then(|cache| {
            cache
                .get(&shazam_pair(&request.track_name, &request.artist_name))
                .filter(|cached| cached.inserted.elapsed() < CACHE_TTL)
                .map(|cached| cached.value.clone())
        })
        .unwrap_or(primary)
}

pub(crate) fn valid_request(request: &LyricsRequest) -> bool {
    valid_text(&request.track_name)
        && valid_text(&request.artist_name)
        && request.album_name.as_deref().is_none_or(valid_text)
        && request.duration_seconds.is_finite()
        && (1.0..=3600.0).contains(&request.duration_seconds)
}

pub async fn get_current_track_lyrics(request: LyricsRequest) -> LyricsResponse {
    if !valid_request(&request) {
        return response("invalidRequest");
    }
    let key = request_key(&request);
    if let Ok(mut cache) = cache().lock() {
        cache.retain(|_, value| value.inserted.elapsed() < CACHE_TTL);
        if let Some(value) = cache.get(&key) {
            return shazam_fallback(&request, value.value.clone());
        }
    }

    let mut url = Url::parse(ENDPOINT).expect("static LRCLIB endpoint");
    url.query_pairs_mut()
        .append_pair("track_name", request.track_name.trim())
        .append_pair("artist_name", request.artist_name.trim());
    if let Some(album) = request.album_name.as_deref() {
        url.query_pairs_mut()
            .append_pair("album_name", album.trim());
    }
    url.query_pairs_mut()
        .append_pair("duration", &format!("{:.0}", request.duration_seconds));

    let primary = tokio::time::timeout(Duration::from_secs(14), lookup(&url, &request))
        .await
        .unwrap_or_else(|_| response("providerUnavailable"));
    let result = shazam_fallback(&request, primary);
    if !matches!(
        result.status.as_str(),
        "providerUnavailable" | "invalidRequest" | "noCurrentTrack"
    ) {
        if let Ok(mut cache) = cache().lock() {
            if cache.len() >= CACHE_LIMIT {
                if let Some(key) = cache.keys().next().cloned() {
                    cache.remove(&key);
                }
            }
            cache.insert(
                key,
                Cached {
                    inserted: Instant::now(),
                    value: result.clone(),
                },
            );
        }
    }
    result
}

/// Radio identity is the identified song, never the station name or elapsed
/// stream position. Zero runtime is deliberately confined to this plain-text path.
pub(crate) fn radio_request(
    snapshot: &Value,
    visible: Option<&LyricsRequestInput>,
) -> Option<LyricsRequest> {
    let native = snapshot["engine"] == "native"
        && matches!(
            snapshot["status"].as_str(),
            Some("playing" | "paused" | "buffering" | "ended")
        );
    let request = if native {
        if !matches!(
            snapshot["track"]["provider"].as_str(),
            Some("radio" | "radioparadise" | "internet-radio")
        ) {
            return None;
        }
        let identified = &snapshot["radioNowPlaying"];
        LyricsRequest {
            track_name: identified["title"].as_str()?.to_owned(),
            artist_name: identified["artist"].as_str()?.to_owned(),
            album_name: identified["metadata"]["album"]
                .as_str()
                .filter(|v| !v.trim().is_empty())
                .map(str::to_owned),
            duration_seconds: 0.,
        }
    } else {
        let input = visible.filter(|input| input.radio && input.playback_range.is_none())?;
        LyricsRequest {
            track_name: input.track_name.clone(),
            artist_name: input.artist_name.clone(),
            album_name: input.album_name.clone(),
            duration_seconds: 0.,
        }
    };
    (valid_text(&request.track_name)
        && valid_text(&request.artist_name)
        && request.album_name.as_deref().is_none_or(valid_text))
    .then_some(request)
}

async fn get_radio_track_lyrics(request: LyricsRequest) -> LyricsResponse {
    let key = format!("radio\0{}", request_key(&request));
    if let Some(value) = cache().lock().ok().and_then(|entries| {
        entries
            .get(&key)
            .filter(|entry| entry.inserted.elapsed() < CACHE_TTL)
            .map(|entry| entry.value.clone())
    }) {
        return value;
    }
    let lookup = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Burrata/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| ())?;
        Ok::<_, ()>(search_fallback(&client, &Url::parse(SEARCH_ENDPOINT).unwrap(), &request).await)
    };
    let result = shazam_fallback(
        &request,
        lookup
            .await
            .unwrap_or_else(|_| response("providerUnavailable")),
    );
    if matches!(result.status.as_str(), "unsynced") {
        if let Ok(mut entries) = cache().lock() {
            if entries.len() >= CACHE_LIMIT {
                if let Some(old) = entries.keys().next().cloned() {
                    entries.remove(&old);
                }
            }
            entries.insert(
                key,
                Cached {
                    inserted: Instant::now(),
                    value: result.clone(),
                },
            );
        }
    }
    result
}

fn radio_record_matches(request: &LyricsRequest, record: &ApiRecord) -> bool {
    !record.instrumental
        && normalized(&request.track_name) == normalized(&record.track_name)
        && complete_artist_credit_matches(&request.artist_name, &record.artist_name)
        && record.duration.is_finite()
        && record.duration > 0.
        && record.duration <= 3600.
        && (record
            .plain_lyrics
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
            || record
                .synced_lyrics
                .as_ref()
                .is_some_and(|text| !text.trim().is_empty()))
}

/// Build a request only from the authoritative currently loaded playback item.
/// Chapter playback is skipped because its local time axis may be an excerpt.
pub(crate) fn request_from_snapshot(snapshot: &Value) -> Option<LyricsRequest> {
    if !matches!(
        snapshot.get("status")?.as_str()?,
        "playing" | "paused" | "buffering" | "ended"
    ) {
        return None;
    }
    let track = snapshot.get("track")?.as_object()?;
    if matches!(
        track.get("provider").and_then(Value::as_str),
        Some("radio" | "radioparadise" | "internet-radio")
    ) {
        return None;
    }
    if track
        .get("playbackRange")
        .is_some_and(|range| !range.is_null())
        || track
            .get("provenance")
            .and_then(|p| p.get("approximateExcerpt"))
            .is_some_and(|v| !v.is_null())
    {
        return None;
    }
    let valid_duration = |duration: f64| duration.is_finite() && (1.0..=3600.0).contains(&duration);
    // Imported rows can lack runtime even after the decoder has established it.
    // Use only the authoritative current item's duration, never another visible
    // track, and never persist this playback observation as catalog metadata.
    let duration = track
        .get("durationSeconds")
        .and_then(Value::as_f64)
        .filter(|d| valid_duration(*d))
        .or_else(|| {
            let id = track.get("id")?.as_str()?;
            if snapshot.get("trackId")?.as_str()? != id {
                return None;
            }
            snapshot
                .get("duration")?
                .as_f64()
                .filter(|d| valid_duration(*d))
        })?;
    Some(LyricsRequest {
        track_name: track.get("title")?.as_str()?.to_owned(),
        artist_name: track.get("artist")?.as_str()?.to_owned(),
        album_name: track
            .get("album")
            .and_then(Value::as_str)
            .map(str::to_owned),
        duration_seconds: duration,
    })
}

pub async fn get_current_track_lyrics_from_snapshot(
    snapshot: &Value,
    visible_track: Option<LyricsRequestInput>,
) -> LyricsResponse {
    if let Some(request) = radio_request(snapshot, visible_track.as_ref()) {
        return get_radio_track_lyrics(request).await;
    }
    match request_for_current_track(snapshot, visible_track) {
        Some(request) => get_current_track_lyrics(request).await,
        None => response("noCurrentTrack"),
    }
}

/// Only saved, metadata-consistent player identity can supply catalog aliases.
pub async fn get_current_track_lyrics_with_catalog(
    snapshot: &Value,
    visible: Option<LyricsRequestInput>,
    store: &crate::Store,
    artists: &crate::artists::ArtistService,
) -> LyricsResponse {
    if let Some(request) = radio_request(snapshot, visible.as_ref()) {
        return get_radio_track_lyrics(request).await;
    }
    let native = snapshot["engine"] == "native"
        && matches!(
            snapshot["status"].as_str(),
            Some("playing" | "paused" | "buffering" | "ended")
        );
    let track_id = if native {
        snapshot["track"]["id"].as_str().map(str::to_owned)
    } else {
        visible.as_ref().and_then(|input| input.track_id.clone())
    };
    let selected_path = if native {
        snapshot["track"]["localPath"].as_str().map(str::to_owned)
    } else {
        visible.as_ref().and_then(|input| input.local_path.clone())
    };
    let Some(request) = request_for_current_track(snapshot, visible) else {
        return response("noCurrentTrack");
    };
    if !valid_request(&request) {
        return response("invalidRequest");
    }
    if let (Some(id), Some(path)) = (track_id.as_deref(), selected_path.as_deref()) {
        if let Some(result) = local_lyrics(store, id, path, &request).await {
            return result;
        }
    }
    let primary = get_current_track_lyrics(request.clone()).await;
    if !matches!(
        primary.status.as_str(),
        "notFound" | "unmatched" | "providerUnavailable" | "unavailable"
    ) {
        return primary;
    }
    let Some(track) = track_id
        .filter(|id| id.len() <= 300)
        .and_then(|id| store.get_track(&id).ok().flatten())
    else {
        return primary;
    };
    if normalized(&track.title) != normalized(&request.track_name)
        || normalized(&track.artist) != normalized(&request.artist_name)
        || track.playback_range.is_some()
        || track
            .duration_seconds
            .is_none_or(|duration| (duration - request.duration_seconds).abs() > 2.0)
    {
        return primary;
    }
    let lookup = async {
        let requests = crate::lyrics_aliases::requests(store, artists, &track, &request).await;
        if requests.len() <= 1 {
            return None;
        }
        if let Some(result) = verified_variant_shazam_lyrics(&requests) {
            return Some(result);
        }
        let result = search_verified_variants(&requests).await;
        matches!(
            result.status.as_str(),
            "synced" | "unsynced" | "instrumental"
        )
        .then_some(result)
    };
    match tokio::time::timeout(Duration::from_secs(10), lookup).await {
        Ok(Some(result)) => result,
        _ => primary,
    }
}

async fn local_lyrics(
    store: &crate::Store,
    track_id: &str,
    selected_path: &str,
    request: &LyricsRequest,
) -> Option<LyricsResponse> {
    let track = store.get_track(track_id).ok()??;
    if track.playback_range.is_some()
        || normalized(&track.title) != normalized(&request.track_name)
        || normalized(&track.artist) != normalized(&request.artist_name)
        || track
            .duration_seconds
            .is_none_or(|d| (d - request.duration_seconds).abs() > 2.0)
    {
        return None;
    }
    let asset = store.get_asset_by_path(selected_path).ok()??;
    if asset.recording_id != track_id {
        return None;
    }
    let raw = crate::local_lyrics::read(&asset).await?;
    let mut result = classify_record(
        request,
        ApiRecord {
            track_name: request.track_name.clone(),
            artist_name: request.artist_name.clone(),
            album_name: request.album_name.clone(),
            duration: request.duration_seconds,
            instrumental: false,
            synced_lyrics: Some(raw.text.clone()),
            plain_lyrics: (!raw.sidecar).then_some(raw.text),
        },
    );
    if !matches!(result.status.as_str(), "synced" | "unsynced") {
        return None;
    }
    result.provider = if raw.sidecar {
        "localLrc"
    } else {
        "embeddedLyrics"
    };
    Some(result)
}

// Call only with the recording-scoped variants returned by lyrics_aliases.
fn verified_variant_shazam_lyrics(requests: &[LyricsRequest]) -> Option<LyricsResponse> {
    requests.iter().find_map(|request| {
        let result = shazam_fallback(request, response("notFound"));
        (result.provider == "shazam").then_some(result)
    })
}

fn matching_variant_record(
    requests: &[LyricsRequest],
    record: ApiRecord,
) -> Option<LyricsResponse> {
    let request = requests
        .iter()
        .find(|request| record_matches(request, &record))?;
    Some(classify_record(request, record))
}

async fn search_verified_variants(requests: &[LyricsRequest]) -> LyricsResponse {
    let key = format!(
        "verified-aliases\0{}",
        requests
            .iter()
            .map(request_key)
            .collect::<Vec<_>>()
            .join("\0")
    );
    if let Ok(entries) = cache().lock() {
        if let Some(entry) = entries
            .get(&key)
            .filter(|entry| entry.inserted.elapsed() < CACHE_TTL)
        {
            return entry.value.clone();
        }
    }
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Burrata/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return response("providerUnavailable");
    };
    let mut seen = std::collections::HashSet::new();
    for request in requests
        .iter()
        .filter(|request| seen.insert(normalized(&request.track_name)))
        .take(4)
    {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mut url = Url::parse(SEARCH_ENDPOINT).expect("static LRCLIB search endpoint");
        url.query_pairs_mut()
            .append_pair("track_name", &request.track_name);
        let Ok(mut reply) = client.get(url).send().await else {
            continue;
        };
        if !reply.status().is_success() {
            continue;
        }
        let Some(bytes) = read_bounded_body(&mut reply).await else {
            continue;
        };
        let Ok(mut records) = serde_json::from_slice::<Vec<ApiRecord>>(&bytes) else {
            continue;
        };
        records.truncate(100);
        records.sort_by(|a, b| {
            record_score(&requests[0], a).total_cmp(&record_score(&requests[0], b))
        });
        if let Some(result) = records
            .into_iter()
            .find_map(|record| matching_variant_record(requests, record))
        {
            if let Ok(mut entries) = cache().lock() {
                entries.retain(|_, entry| entry.inserted.elapsed() < CACHE_TTL);
                if entries.len() >= CACHE_LIMIT {
                    if let Some(old) = entries.keys().next().cloned() {
                        entries.remove(&old);
                    }
                }
                entries.insert(
                    key,
                    Cached {
                        inserted: Instant::now(),
                        value: result.clone(),
                    },
                );
            }
            return result;
        }
    }
    response("notFound")
}

pub(crate) fn request_for_current_track(
    snapshot: &Value,
    visible_track: Option<LyricsRequestInput>,
) -> Option<LyricsRequest> {
    if let Some(request) = radio_request(snapshot, visible_track.as_ref()) {
        return Some(request);
    }
    let native_active = snapshot.get("engine").and_then(Value::as_str) == Some("native")
        && matches!(
            snapshot.get("status").and_then(Value::as_str),
            Some("playing" | "paused" | "buffering" | "ended")
        );
    if native_active {
        request_from_snapshot(snapshot)
    } else {
        visible_track.and_then(|input| {
            if input
                .playback_range
                .as_ref()
                .is_some_and(|range| !range.is_null())
            {
                None
            } else {
                Some(input.into())
            }
        })
    }
}

async fn lookup(url: &Url, request: &LyricsRequest) -> LyricsResponse {
    lookup_with_search(
        url,
        &Url::parse(SEARCH_ENDPOINT).expect("static LRCLIB search endpoint"),
        request,
    )
    .await
}

async fn lookup_with_search(
    url: &Url,
    search_url: &Url,
    request: &LyricsRequest,
) -> LyricsResponse {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Burrata/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(_) => return response("providerUnavailable"),
    };
    let mut http_response = match client.get(url.clone()).send().await {
        Ok(response) => response,
        Err(error) => {
            #[cfg(test)]
            eprintln!(
                "LRCLIB request failure: timeout={}, connect={}, builder={}, status={:?}",
                error.is_timeout(),
                error.is_connect(),
                error.is_builder(),
                error.status().map(|status| status.as_u16())
            );
            return search_fallback(&client, search_url, request).await;
        }
    };
    if http_response.status() == reqwest::StatusCode::NOT_FOUND
        || http_response.status().is_server_error()
    {
        return search_fallback(&client, search_url, request).await;
    }
    if http_response.status() != reqwest::StatusCode::OK
        || http_response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        #[cfg(test)]
        eprintln!(
            "LRCLIB response unavailable: HTTP {}, content_length={:?}",
            http_response.status().as_u16(),
            http_response.content_length()
        );
        return response("providerUnavailable");
    }
    let mut bytes = Vec::new();
    loop {
        match http_response.chunk().await {
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) | Err(_) => return response("providerUnavailable"),
            Ok(None) => break,
        }
    }
    let record: ApiRecord = match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        Err(_) => return response("providerUnavailable"),
    };
    let classified = classify_record(request, record);
    if classified.status == "unmatched" {
        search_fallback(&client, search_url, request).await
    } else {
        classified
    }
}

async fn search_fallback(
    client: &reqwest::Client,
    endpoint: &Url,
    request: &LyricsRequest,
) -> LyricsResponse {
    let mut reachable = false;
    let mut has_results = false;
    for qualified in [true, false] {
        let mut url = endpoint.clone();
        url.query_pairs_mut()
            .append_pair("track_name", request.track_name.trim());
        if qualified {
            url.query_pairs_mut()
                .append_pair("artist_name", request.artist_name.trim());
        }
        let Ok(mut reply) = client.get(url).send().await else {
            continue;
        };
        // A rate limit is not permission to immediately issue a broader query.
        if reply.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return response("providerUnavailable");
        }
        if !reply.status().is_success() {
            continue;
        }
        // Search can return many legitimate lyric documents. Keep a separate
        // bounded array budget instead of rejecting it at the single-item cap.
        let Some(bytes) = read_bounded_body_limit(&mut reply, 1024 * 1024).await else {
            continue;
        };
        let Ok(rows) = serde_json::from_slice::<Vec<Value>>(&bytes) else {
            continue;
        };
        reachable = true;
        has_results |= !rows.is_empty();
        let records = rows
            .into_iter()
            .take(200)
            .filter_map(|row| serde_json::from_value::<ApiRecord>(row).ok())
            .collect::<Vec<_>>();
        if request.duration_seconds == 0. {
            let mut matches = records
                .into_iter()
                .filter(|record| radio_record_matches(request, record))
                .collect::<Vec<_>>();
            matches.sort_by_key(|record| {
                !request
                    .album_name
                    .as_deref()
                    .zip(record.album_name.as_deref())
                    .is_some_and(|(a, b)| normalized(a) == normalized(b))
            });
            if let Some(record) = matches.into_iter().next() {
                return untimed_record(request, record);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let mut candidates = records
            .iter()
            .filter(|record| record_matches(request, record))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| record_score(request, a).total_cmp(&record_score(request, b)));
        if let Some(record) = candidates.into_iter().next() {
            return classify_record(request, record);
        }
        let mut plain = records
            .into_iter()
            .filter(|record| plain_text_record_matches(request, record))
            .collect::<Vec<_>>();
        plain.sort_by(|a, b| record_score(request, a).total_cmp(&record_score(request, b)));
        if let Some(record) = plain.into_iter().next() {
            return untimed_record(request, record);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    response(if has_results {
        "unmatched"
    } else if reachable {
        "notFound"
    } else {
        "providerUnavailable"
    })
}

async fn read_bounded_body(response: &mut reqwest::Response) -> Option<Vec<u8>> {
    read_bounded_body_limit(response, MAX_RESPONSE_BYTES).await
}

async fn read_bounded_body_limit(
    response: &mut reqwest::Response,
    limit: usize,
) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) <= limit => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) => return Some(bytes),
        }
    }
}

fn artist_credit_components(value: &str) -> Vec<String> {
    let lower = value.to_lowercase();
    let mut normalized = lower;
    for sep in [
        " featuring ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " with ",
        " & ",
        " and ",
        " y ",
        ";",
        ",",
    ] {
        normalized = normalized.replace(sep, "\u{1f}");
    }
    normalized
        .split('\u{1f}')
        .map(|part| {
            part.trim()
                .trim_matches(|ch: char| ch == '(' || ch == ')')
                .to_owned()
        })
        .filter(|part| !part.is_empty())
        .collect()
}

fn complete_artist_credit_matches(request_artist: &str, record_artist: &str) -> bool {
    if normalized(request_artist) == normalized(record_artist) {
        return true;
    }
    let credits = |value: &str| {
        artist_credit_components(value)
            .into_iter()
            .map(|part| normalized(&part))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let requested = credits(request_artist);
    !requested.is_empty() && requested == credits(record_artist)
}

fn artist_matches(request_artist: &str, record_artist: &str) -> bool {
    let returned = normalized(record_artist);
    complete_artist_credit_matches(request_artist, record_artist)
        || artist_credit_components(request_artist)
            .first()
            .is_some_and(|component| normalized(component) == returned)
}

pub(crate) fn record_matches(request: &LyricsRequest, record: &ApiRecord) -> bool {
    record.duration.is_finite()
        && (record.duration - request.duration_seconds).abs() <= 2.0
        && normalized(&record.track_name) == normalized(&request.track_name)
        && artist_matches(&request.artist_name, &record.artist_name)
        && (complete_artist_credit_matches(&request.artist_name, &record.artist_name)
            || request.album_name.as_deref().is_none_or(|album| {
                record
                    .album_name
                    .as_deref()
                    .is_none_or(|returned| normalized(returned) == normalized(album))
            }))
}

/// Same song and full performer credit can supply text when a nearby version
/// has a slightly different runtime. This never authorizes lyric timestamps.
pub(crate) fn plain_text_record_matches(request: &LyricsRequest, record: &ApiRecord) -> bool {
    request.duration_seconds >= 60.
        && record.duration.is_finite()
        && (record.duration - request.duration_seconds).abs() <= 8.
        && (record.duration - request.duration_seconds).abs() / request.duration_seconds <= 0.06
        && normalized(&record.track_name) == normalized(&request.track_name)
        && complete_artist_credit_matches(&request.artist_name, &record.artist_name)
        && !record.instrumental
}

/// Caller either verified a text-only same-performer match or obtained an
/// explicit choice. Source timestamps never cross into another performance.
pub(crate) fn untimed_record(request: &LyricsRequest, mut record: ApiRecord) -> LyricsResponse {
    record.plain_lyrics = record
        .plain_lyrics
        .clone()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| {
            record.synced_lyrics.as_ref().map(|text| {
                parse_lrc(text)
                    .into_iter()
                    .map(|line| line.text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        });
    record.synced_lyrics = None;
    let chosen = LyricsRequest {
        track_name: record.track_name.clone(),
        artist_name: record.artist_name.clone(),
        album_name: record.album_name.clone(),
        duration_seconds: record.duration,
    };
    let mut result = classify_record(&chosen, record);
    if let Some(matched) = result.matched.as_mut() {
        matched.duration_delta_seconds =
            (matched.duration_seconds - request.duration_seconds).abs();
    }
    result
}

fn record_score(request: &LyricsRequest, record: &ApiRecord) -> f64 {
    let album_penalty = request
        .album_name
        .as_deref()
        .zip(record.album_name.as_deref())
        .map_or(0.0, |(expected, returned)| {
            if normalized(expected) == normalized(returned) {
                0.0
            } else {
                100.0
            }
        });
    let artist_penalty = if normalized(&request.artist_name) == normalized(&record.artist_name) {
        0.0
    } else {
        0.25
    };
    album_penalty + artist_penalty + (record.duration - request.duration_seconds).abs()
}

pub(crate) fn classify_record(request: &LyricsRequest, record: ApiRecord) -> LyricsResponse {
    if !record_matches(request, &record) {
        return response("unmatched");
    }
    let matched = LyricsMatch {
        track_name: record.track_name,
        artist_name: record.artist_name,
        album_name: record.album_name,
        duration_seconds: record.duration,
        duration_delta_seconds: (record.duration - request.duration_seconds).abs(),
    };
    if record.instrumental {
        return LyricsResponse {
            status: "instrumental".into(),
            provider: "lrclib",
            source_hash: None,
            matched: Some(matched),
            lines: None,
            plain_lyrics: None,
            source_offset_seconds: None,
        };
    }
    if let Some(raw) = record
        .synced_lyrics
        .filter(|lyrics| !lyrics.trim().is_empty() && lyrics.len() <= MAX_LYRICS_BYTES)
    {
        let Ok(source_offset_seconds) = lrc_offset(&raw) else {
            return response("unavailable");
        };
        let lines = parse_lrc(&raw);
        if !lines.is_empty() {
            let source_hash = source_hash_for_lines(&lines);
            return LyricsResponse {
                status: "synced".into(),
                provider: "lrclib",
                source_hash: Some(source_hash),
                matched: Some(matched),
                lines: Some(lines),
                plain_lyrics: None,
                source_offset_seconds,
            };
        }
    }
    if let Some(plain) = record
        .plain_lyrics
        .filter(|lyrics| !lyrics.trim().is_empty() && lyrics.len() <= MAX_LYRICS_BYTES)
    {
        let lines = parse_plain_lines(&plain);
        let source_hash = source_hash_for_lines(&lines);
        return LyricsResponse {
            status: "unsynced".into(),
            provider: "lrclib",
            source_hash: Some(source_hash),
            matched: Some(matched),
            lines: Some(lines),
            plain_lyrics: Some(plain),
            source_offset_seconds: None,
        };
    }
    response("notFound")
}

// LRC positive offsets advance the display clock; preserve the original
// source timestamps and stable IDs instead of rewriting them.
fn lrc_offset(raw: &str) -> Result<Option<f64>, ()> {
    let mut offset = None;
    for line in raw.lines().take(MAX_LINES) {
        let line = line.trim().trim_start_matches('\u{feff}');
        let Some(tag) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
            continue;
        };
        let Some((key, value)) = tag.split_once(':') else {
            continue;
        };
        if !key.eq_ignore_ascii_case("offset") {
            continue;
        }
        let milliseconds = value.trim().parse::<i64>().map_err(|_| ())?;
        if !(-300_000..=300_000).contains(&milliseconds)
            || offset.is_some_and(|previous| previous != milliseconds)
        {
            return Err(());
        }
        offset = Some(milliseconds);
    }
    Ok(offset.map(|milliseconds| milliseconds as f64 / 1000.0))
}

pub(crate) fn parse_lrc(raw: &str) -> Vec<LyricsLine> {
    if raw.len() > MAX_LYRICS_BYTES {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut duplicate_ordinals: HashMap<(u64, String), usize> = HashMap::new();
    for raw_line in raw.lines().take(MAX_LINES) {
        let mut rest = raw_line.trim_start();
        let mut timestamps = Vec::new();
        while let Some(after_open) = rest.strip_prefix('[') {
            let Some((stamp, after_close)) = after_open.split_once(']') else {
                break;
            };
            let Some(seconds) = parse_timestamp(stamp) else {
                break;
            };
            timestamps.push(seconds);
            rest = after_close;
            if timestamps.len() >= 10 {
                break;
            }
        }
        let text = rest.trim();
        if text.is_empty() || text.len() > 1200 || text.chars().any(char::is_control) {
            continue;
        }
        for time_seconds in timestamps {
            let key = (time_seconds.to_bits(), text.to_owned());
            let ordinal = duplicate_ordinals.entry(key).or_default();
            let id = stable_line_id(Some(time_seconds), text, *ordinal);
            *ordinal += 1;
            lines.push(LyricsLine {
                id,
                time_seconds: Some(time_seconds),
                text: text.to_owned(),
            });
            if lines.len() >= MAX_LINES {
                return lines;
            }
        }
    }
    lines.sort_by(|a, b| {
        a.time_seconds
            .unwrap_or_default()
            .total_cmp(&b.time_seconds.unwrap_or_default())
    });
    lines
}

fn parse_plain_lines(raw: &str) -> Vec<LyricsLine> {
    let mut duplicate_ordinals: HashMap<String, usize> = HashMap::new();
    raw.split('\n')
        .take(MAX_LINES)
        .map(|raw_line| {
            let text = raw_line.strip_suffix('\r').unwrap_or(raw_line).to_owned();
            let ordinal = duplicate_ordinals.entry(text.clone()).or_default();
            let line = LyricsLine {
                id: stable_line_id(None, &text, *ordinal),
                time_seconds: None,
                text,
            };
            *ordinal += 1;
            line
        })
        .collect()
}

fn digest_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn stable_line_id(
    time_seconds: Option<f64>,
    text: &str,
    duplicate_ordinal: usize,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"burrata-lyrics-line-v2\0");
    match time_seconds {
        Some(seconds) => {
            hash.update([1]);
            hash.update(seconds.to_bits().to_le_bytes());
        }
        None => hash.update([0]),
    }
    hash.update((duplicate_ordinal as u64).to_le_bytes());
    hash.update(text.as_bytes());
    format!("line-{}", &digest_hex(hash.finalize())[..20])
}

pub(crate) fn source_hash_for_lines(lines: &[LyricsLine]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"burrata-lyrics-document-v2\0");
    for line in lines {
        hash.update((line.id.len() as u64).to_le_bytes());
        hash.update(line.id.as_bytes());
        match line.time_seconds {
            Some(seconds) => {
                hash.update([1]);
                hash.update(seconds.to_bits().to_le_bytes());
            }
            None => hash.update([0]),
        }
        hash.update((line.text.len() as u64).to_le_bytes());
        hash.update(line.text.as_bytes());
    }
    digest_hex(hash.finalize())
}

/// UI JSON transports can round a binary float by a few ulps. Accept only
/// that timestamp noise, while binding the displayed text to the authoritative
/// source hash, stable line identities, exact text, and exact ordering.
pub(crate) fn displayed_document_matches(
    current: &LyricsResponse,
    source_hash: &str,
    received: &[LyricsLine],
) -> bool {
    if current.source_hash.as_deref() != Some(source_hash)
        || !matches!(current.status.as_str(), "synced" | "unsynced")
    {
        return false;
    }
    let Some(canonical) = current.lines.as_ref() else {
        return false;
    };
    canonical.len() == received.len()
        && validate_source_document(source_hash, canonical)
        && canonical.iter().zip(received).all(|(a, b)| {
            a.id == b.id
                && a.text == b.text
                && match (a.time_seconds, b.time_seconds) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9,
                    _ => false,
                }
        })
}

pub(crate) fn validate_source_document(source_hash: &str, lines: &[LyricsLine]) -> bool {
    if lines.is_empty() || lines.len() > MAX_LINES || source_hash.len() != 64 {
        return false;
    }
    let mut duplicate_ordinals: HashMap<(Option<u64>, String), usize> = HashMap::new();
    for line in lines {
        let valid_time = line
            .time_seconds
            .is_none_or(|seconds| seconds.is_finite() && seconds >= 0.0);
        let valid_text = (line.time_seconds.is_none() || !line.text.trim().is_empty())
            && line.text.len() <= 1200
            && !line.text.chars().any(char::is_control);
        if !valid_time || !valid_text {
            return false;
        }
        let key = (line.time_seconds.map(f64::to_bits), line.text.clone());
        let ordinal = duplicate_ordinals.entry(key).or_default();
        if line.id != stable_line_id(line.time_seconds, &line.text, *ordinal) {
            return false;
        }
        *ordinal += 1;
    }
    source_hash_for_lines(lines) == source_hash
}

fn parse_timestamp(stamp: &str) -> Option<f64> {
    let parts = stamp.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0.0, minutes.parse::<u64>().ok()?, *seconds),
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()? as f64,
            minutes.parse::<u64>().ok()?,
            *seconds,
        ),
        _ => return None,
    };
    let seconds = seconds.parse::<f64>().ok()?;
    if minutes >= 60 || !(0.0..60.0).contains(&seconds) || !seconds.is_finite() {
        return None;
    }
    let value = hours * 3600.0 + minutes as f64 * 60.0 + seconds;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    #[test]
    fn existing_shazam_lyrics_are_an_identity_scoped_unsynced_fallback() {
        let source = serde_json::json!({"key":"999123456789", "title":"Synthetic fallback title", "subtitle":"Synthetic fallback artist", "sections":[{"type":"LYRICS", "text":["Synthetic first paragraph", "", "Synthetic second paragraph"]}]});
        super::retain_shazam_lyrics(&source);
        let mut request = super::LyricsRequest {
            track_name: "Synthetic fallback title".into(),
            artist_name: "Synthetic fallback artist".into(),
            album_name: None,
            duration_seconds: 180.0,
        };
        let result = super::shazam_fallback(&request, super::response("notFound"));
        assert_eq!(result.provider, "shazam");
        assert_eq!(result.status, "unsynced");
        let lines = result.lines.as_ref().unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|line| line.time_seconds.is_none()));
        assert!(super::validate_source_document(
            result.source_hash.as_deref().unwrap(),
            lines
        ));
        assert_eq!(
            super::shazam_fallback(&request, super::response("instrumental")).status,
            "instrumental"
        );
        request.artist_name = "Wrong artist".into();
        assert_eq!(
            super::shazam_fallback(&request, super::response("notFound")).provider,
            "lrclib"
        );
        request.artist_name = "Synthetic fallback artist".into();
        request.track_name += " live";
        assert_eq!(
            super::shazam_fallback(&request, super::response("notFound")).provider,
            "lrclib"
        );
    }

    #[test]
    fn malformed_shazam_lyrics_are_not_retained() {
        let mut source = serde_json::json!({"key":"999123456788", "title":"Synthetic invalid lyric", "subtitle":"Synthetic invalid artist", "sections":[{"type":"LYRICS","text":["valid",false]}]});
        super::retain_shazam_lyrics(&source);
        let request = super::LyricsRequest {
            track_name: "Synthetic invalid lyric".into(),
            artist_name: "Synthetic invalid artist".into(),
            album_name: None,
            duration_seconds: 180.0,
        };
        assert_eq!(
            super::shazam_fallback(&request, super::response("notFound")).provider,
            "lrclib"
        );
        source["sections"][0]["text"] = serde_json::json!(["oversized".repeat(10000)]);
        super::retain_shazam_lyrics(&source);
        assert_eq!(
            super::shazam_fallback(&request, super::response("notFound")).provider,
            "lrclib"
        );
    }

    use super::*;
    #[test]
    fn displayed_timed_lyrics_survive_json_rounding_without_accepting_a_changed_document() {
        let record:ApiRecord=serde_json::from_value(serde_json::json!({"trackName":"Example Song","artistName":"Example Artist","albumName":"Example Album","duration":180,"instrumental":false,"syncedLyrics":"[01:30.21]Synthetic first line\n[01:31.29]Synthetic next line"})).unwrap();
        let doc = classify_record(&request(), record);
        let canonical = doc.lines.as_ref().unwrap();
        let received: Vec<LyricsLine> =
            serde_json::from_str(&serde_json::to_string(canonical).unwrap()).unwrap();
        assert_ne!(
            canonical, &received,
            "fixture reproduces the real JSON float precision mismatch"
        );
        let hash = doc.source_hash.as_deref().unwrap();
        assert!(displayed_document_matches(&doc, hash, &received));
        let mut changed = received.clone();
        changed[0].text.push('!');
        assert!(!displayed_document_matches(&doc, hash, &changed));
        let mut changed = received.clone();
        changed[0].time_seconds = Some(changed[0].time_seconds.unwrap() + 0.01);
        assert!(!displayed_document_matches(&doc, hash, &changed));
        let mut changed = received.clone();
        changed[0].id.push('x');
        assert!(!displayed_document_matches(&doc, hash, &changed));
        let mut changed = received.clone();
        changed.reverse();
        assert!(!displayed_document_matches(&doc, hash, &changed));
        assert!(!displayed_document_matches(
            &doc,
            &"f".repeat(64),
            &received
        ));
        assert!(validate_source_document(hash, canonical));
    }

    #[test]
    fn real_lrclib_response_can_include_both_name_and_track_name() {
        let record: ApiRecord = serde_json::from_value(serde_json::json!({
            "id":123, "name":"Provider display name", "trackName":"Example Song",
            "artistName":"Example Artist", "albumName":"Example Album", "duration":180,
            "instrumental":false, "syncedLyrics":"[00:01.00] Synthetic line", "plainLyrics":"Synthetic line",
            "hasWordSync":false, "lyricsfile":"ignored provider-specific document"
        })).unwrap();
        assert_eq!(record.track_name, "Example Song");
        assert_eq!(classify_record(&request(), record).status, "synced");
    }
    #[tokio::test]
    async fn exact_endpoint_503_falls_back_to_qualified_search_and_ignores_a_bad_row() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                let incoming = String::from_utf8_lossy(&bytes[..n]);
                let (status, body) = if index == 0 {
                    assert!(incoming.starts_with("GET /get"));
                    (
                        "503 Service Unavailable",
                        "temporary backend failure".to_string(),
                    )
                } else {
                    assert!(incoming.contains("artist_name=Example+Artist"));
                    ("200 OK",serde_json::json!([
                    {"bad":"provider row without required fields"},
                    {"trackName":"Example Song","artistName":"Wrong Artist","duration":180,"instrumental":false,"plainLyrics":"Wrong source"},
                    {"trackName":"Example Song","artistName":"Example Artist","duration":180,"instrumental":false,"syncedLyrics":"[00:01.00] Synthetic matching line","plainLyrics":"Synthetic matching line"}
                ]).to_string())
                };
                let reply=format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let get = Url::parse(&format!("http://{address}/get")).unwrap();
        let search = Url::parse(&format!("http://{address}/search")).unwrap();
        let result = lookup_with_search(&get, &search, &request()).await;
        assert_eq!(result.status, "synced");
        assert_eq!(result.matched.unwrap().artist_name, "Example Artist");
        server.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "Explicit live Buck Owens lyric lookup; sends metadata only"]
    async fn live_buck_owens_automatically_returns_lyrics_with_safe_timing() {
        let result = get_current_track_lyrics(LyricsRequest {
            track_name: "Truck Drivin' Man".into(),
            artist_name: "Buck Owens".into(),
            album_name: None,
            duration_seconds: 156.933,
        })
        .await;
        assert!(
            matches!(result.status.as_str(), "synced" | "unsynced"),
            "{}",
            result.status
        );
        let matched = result.matched.unwrap();
        assert_eq!(matched.artist_name, "Buck Owens");
        if matched.duration_delta_seconds > 2. {
            assert_eq!(result.status, "unsynced");
            assert!(result
                .lines
                .unwrap()
                .iter()
                .all(|line| line.time_seconds.is_none()));
        }
    }

    #[tokio::test]
    #[ignore = "Explicit live LRCLIB check for reported Paper Wings; never prints lyric text"]
    async fn live_paper_wings_recovers_matching_timed_lyrics() {
        let request = LyricsRequest {
            track_name: "Paper Wings".into(),
            artist_name: "Gillian Welch".into(),
            album_name: None,
            duration_seconds: 236.,
        };
        let result = get_current_track_lyrics(request).await;
        assert_eq!(result.status, "synced");
        assert_eq!(result.matched.unwrap().artist_name, "Gillian Welch");
        assert!(result.lines.unwrap().len() > 5);
    }

    #[tokio::test]
    #[ignore = "Explicit live LRCLIB availability check; does not print lyric text"]
    async fn live_reported_nunca_recording_returns_matching_synced_lyrics() {
        let request = LyricsRequest {
            track_name: "Nunca vas a comprender".into(),
            artist_name: "Rita Payés & Elisabeth Roma".into(),
            album_name: Some("Como la piel".into()),
            duration_seconds: 244.631,
        };
        let result = get_current_track_lyrics(request.clone()).await;
        assert_eq!(result.status, "synced");
        let matched = result.matched.as_ref().expect("matched recording metadata");
        assert!(artist_matches(&request.artist_name, &matched.artist_name));
        assert!(matched.duration_delta_seconds <= 2.0);
        assert!(result.source_hash.is_some());
        assert!(result.lines.as_ref().is_some_and(|lines| !lines.is_empty()));
        eprintln!("Live reported lyrics: synced, matching performers, duration delta {:.3}s, {} timed lines; text omitted", matched.duration_delta_seconds, result.lines.as_ref().unwrap().len());
    }

    #[test]
    fn verified_cyrillic_variant_still_requires_artist_and_duration_match() {
        let mut alias = request();
        alias.track_name = "Калиманку Денку".into();
        alias.artist_name = "Хор".into();
        let mut candidate = record();
        candidate.track_name = alias.track_name.clone();
        candidate.artist_name = alias.artist_name.clone();
        assert!(matching_variant_record(&[request()], candidate.clone()).is_none());
        assert!(matching_variant_record(&[alias.clone()], candidate.clone()).is_some());
        candidate.artist_name = "Other choir".into();
        assert!(matching_variant_record(&[alias.clone()], candidate.clone()).is_none());
        candidate.artist_name = alias.artist_name.clone();
        candidate.duration = 240.0;
        assert!(matching_variant_record(&[alias], candidate).is_none());
    }

    #[test]
    fn source_offset_preserves_timestamps_and_rejects_ambiguous_tags() {
        let mut source = record();
        source.synced_lyrics = Some("[offset:+1500]\n[00:02.50] Synthetic line".into());
        let result = classify_record(&request(), source);
        assert_eq!(result.source_offset_seconds, Some(1.5));
        assert_eq!(result.lines.unwrap()[0].time_seconds, Some(2.5));
        assert_eq!(lrc_offset("[offset:-500]"), Ok(Some(-0.5)));
        assert_eq!(lrc_offset("[offset:500]\n[offset:500]"), Ok(Some(0.5)));
        for raw in [
            "[offset:500]\n[offset:600]",
            "[offset:NaN]",
            "[offset:300001]",
            "[offset:9223372036854775808]",
        ] {
            assert!(lrc_offset(raw).is_err());
        }
    }

    #[test]
    fn verified_aliases_reuse_existing_shazam_lyrics_without_recognition() {
        let source = serde_json::json!({"key":"999123456780", "title":"Synthetic alias lyric", "subtitle":"Синтетический хор", "sections":[{"type":"LYRICS","text":["Synthetic cached paragraph"]}]});
        retain_shazam_lyrics(&source);
        let original = LyricsRequest {
            track_name: "Synthetic alias lyric".into(),
            artist_name: "Synthetic transliterated choir".into(),
            album_name: None,
            duration_seconds: 180.0,
        };
        assert!(verified_variant_shazam_lyrics(&[original.clone()]).is_none());
        let mut verified_alias = original.clone();
        verified_alias.artist_name = "Синтетический хор".into();
        let result = verified_variant_shazam_lyrics(&[original, verified_alias]).unwrap();
        assert_eq!(result.provider, "shazam");
        assert_eq!(result.status, "unsynced");
    }

    fn request() -> LyricsRequest {
        LyricsRequest {
            track_name: "Example Song".into(),
            artist_name: "Example Artist".into(),
            album_name: Some("Example Album".into()),
            duration_seconds: 180.0,
        }
    }

    fn record() -> ApiRecord {
        ApiRecord {
            track_name: "Example Song".into(),
            artist_name: "Example Artist".into(),
            album_name: Some("Example Album".into()),
            duration: 181.5,
            instrumental: false,
            synced_lyrics: Some("[00:02.50] First line\n[01:03.25] Second line".into()),
            plain_lyrics: Some("First line\nSecond line".into()),
        }
    }

    #[test]
    fn exact_metadata_and_duration_return_absolute_timestamps() {
        let value = classify_record(&request(), record());
        assert_eq!(value.status, "synced");
        assert_eq!(value.matched.as_ref().unwrap().duration_delta_seconds, 1.5);
        let lines = value.lines.as_ref().unwrap();
        assert!(validate_source_document(
            value.source_hash.as_deref().unwrap(),
            lines
        ));
        let wire = serde_json::to_value(&value).unwrap();
        assert!(wire.get("match").is_some());
        assert!(wire.get("matched").is_none());
        assert!(wire.get("sourceHash").and_then(Value::as_str).is_some());
        assert!(wire["lines"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with("line-"));
        assert_eq!(
            value
                .lines
                .unwrap()
                .iter()
                .map(|line| line.time_seconds)
                .collect::<Vec<_>>(),
            vec![Some(2.5), Some(63.25)]
        );
    }

    #[test]
    fn unsynced_plain_lyrics_keep_order_blank_paragraphs_and_stable_document_identity() {
        let mut value = record();
        value.synced_lyrics = None;
        value.plain_lyrics = Some("first paragraph\n\nsecond paragraph\n".into());
        let response = classify_record(&request(), value);
        assert_eq!(response.status, "unsynced");
        let lines = response.lines.as_ref().unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["first paragraph", "", "second paragraph", ""]
        );
        assert!(lines.iter().all(|line| line.time_seconds.is_none()));
        assert_ne!(lines[1].id, lines[3].id);
        assert!(validate_source_document(
            response.source_hash.as_deref().unwrap(),
            lines
        ));
        let wire = serde_json::to_value(&response).unwrap();
        assert!(wire["lines"][0]["timeSeconds"].is_null());
        assert!(wire["plainLyrics"].as_str().is_some());
    }

    #[test]
    fn rejects_wrong_artist_title_or_duration() {
        let mutations: [fn(&mut ApiRecord); 3] = [
            |value: &mut ApiRecord| value.artist_name = "Another Artist".into(),
            |value: &mut ApiRecord| value.track_name = "Another Song".into(),
            |value: &mut ApiRecord| value.duration = 183.0,
        ];
        for mutate in mutations {
            let mut value = record();
            mutate(&mut value);
            assert_eq!(classify_record(&request(), value).status, "unmatched");
        }
    }

    #[test]
    fn reports_instrumental_unsynced_and_missing_lyrics_distinctly() {
        let mut value = record();
        value.instrumental = true;
        assert_eq!(classify_record(&request(), value).status, "instrumental");
        let mut value = record();
        value.synced_lyrics = None;
        assert_eq!(classify_record(&request(), value).status, "unsynced");
        let mut value = record();
        value.synced_lyrics = None;
        value.plain_lyrics = None;
        assert_eq!(classify_record(&request(), value).status, "notFound");
    }

    #[test]
    fn accepts_missing_album_and_a_single_provider_credit_for_a_collaboration() {
        let request = LyricsRequest {
            track_name: "Nunca vas a comprender".into(),
            artist_name: "Rita Payés & Elisabeth Roma".into(),
            album_name: Some("Como la piel".into()),
            duration_seconds: 244.631,
        };
        let record = ApiRecord {
            track_name: "Nunca vas a comprender".into(),
            artist_name: "Rita Payés".into(),
            album_name: Some("Como la piel".into()),
            duration: 245.0,
            instrumental: false,
            synced_lyrics: Some("[00:02.00] synthetic".into()),
            plain_lyrics: None,
        };
        let result = classify_record(&request, record);
        assert_eq!(result.status, "synced");
        assert_eq!(
            result.matched.as_ref().unwrap().album_name.as_deref(),
            Some("Como la piel")
        );
        let mut without_album = request;
        without_album.album_name = None;
        let album_missing_from_provider = ApiRecord {
            track_name: "Nunca vas a comprender".into(),
            artist_name: "Rita Payés".into(),
            album_name: None,
            duration: 245.0,
            instrumental: false,
            synced_lyrics: Some("[00:02.00] synthetic".into()),
            plain_lyrics: None,
        };
        assert_eq!(
            classify_record(&without_album, album_missing_from_provider).status,
            "synced"
        );
    }

    #[test]
    fn artist_credit_matching_does_not_accept_an_unrelated_artist() {
        assert!(artist_matches("Rita Payés & Elisabeth Roma", "Rita Payés"));
        assert!(!artist_matches(
            "Rita Payés feat. Elisabeth Roma",
            "Elisabeth Roma"
        ));
        assert!(!artist_matches(
            "Rita Payés & Elisabeth Roma",
            "Another Artist"
        ));
        assert!(!complete_artist_credit_matches(
            "Y; Artist One; Artist Two",
            "Artist One; Artist Two"
        ));
        assert!(!complete_artist_credit_matches(
            "Artist One & Artist Two",
            "Artist One, Y, Artist Two"
        ));
    }

    #[test]
    fn reported_recording_can_use_a_single_release_with_complete_collaboration_credits() {
        let request = LyricsRequest {
            track_name: "Nunca vas a comprender".into(),
            artist_name: "Rita Payés & Elisabeth Roma".into(),
            album_name: Some("Como la piel".into()),
            duration_seconds: 244.631,
        };
        for artist in [
            "Rita Payés; Elisabeth Roma",
            "Rita Payés and Elisabeth Roma",
            "Rita Payés y Elisabeth Roma",
        ] {
            let mut value = record();
            value.track_name = request.track_name.clone();
            value.artist_name = artist.into();
            value.album_name = Some("Nunca vas a comprender".into());
            value.duration = 245.0;
            assert_eq!(classify_record(&request, value.clone()).status, "synced");
            value.duration = 233.0;
            assert_eq!(classify_record(&request, value).status, "unmatched");
        }
        let mut incomplete = record();
        incomplete.track_name = request.track_name.clone();
        incomplete.artist_name = "Rita Payés".into();
        incomplete.album_name = Some("Different release".into());
        incomplete.duration = 245.0;
        assert_eq!(classify_record(&request, incomplete).status, "unmatched");
        assert!(!artist_matches(
            &request.artist_name,
            "Rita Payés & Another Singer"
        ));
    }

    #[test]
    fn timestamps_are_absolute_and_malformed_entries_are_ignored() {
        assert_eq!(
            parse_lrc("[02:03.4] valid\n[99:80.00] bad\n[01:02:03.5] hour"),
            vec![
                LyricsLine {
                    id: stable_line_id(Some(123.4), "valid", 0),
                    time_seconds: Some(123.4),
                    text: "valid".into()
                },
                LyricsLine {
                    id: stable_line_id(Some(3723.5), "hour", 0),
                    time_seconds: Some(3723.5),
                    text: "hour".into()
                },
            ]
        );
    }

    #[test]
    fn current_track_request_uses_only_loaded_track_metadata() {
        let snapshot = serde_json::json!({
            "status":"playing", "track":{"title":"Song","artist":"Artist","album":"Album","durationSeconds":201.5},
            "queue":[{"title":"different queued song"}], "candidate":{"url":"https://private.invalid"}
        });
        let request = request_from_snapshot(&snapshot).unwrap();
        assert_eq!(request.track_name, "Song");
        assert_eq!(request.artist_name, "Artist");
        assert_eq!(request.album_name.as_deref(), Some("Album"));
        assert_eq!(request.duration_seconds, 201.5);
        let excerpt = serde_json::json!({"status":"playing","track":{"title":"Chapter","artist":"Artist","album":"Album","durationSeconds":201.5,"playbackRange":{"startTime":20.0}}});
        assert!(request_from_snapshot(&excerpt).is_none());
        assert!(request_from_snapshot(&serde_json::json!({"status":"idle"})).is_none());
        let no_album = serde_json::json!({"status":"playing","track":{"title":"Song","artist":"Artist","durationSeconds":201.5}});
        assert_eq!(request_from_snapshot(&no_album).unwrap().album_name, None);
    }

    #[tokio::test]
    async fn local_sidecar_is_bound_to_registered_recording_and_unchanged_audio() {
        use crate::models::{LocalAsset, Track};
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let audio = dir.join("selected.flac");
        std::fs::write(&audio, b"synthetic audio identity fixture").unwrap();
        std::fs::write(audio.with_extension("lrc"), "[00:01] Synthetic lyric line").unwrap();
        let store = crate::Store::open(&dir.join("test.sqlite")).unwrap();
        let req = request();
        let track = Track {
            id: "local:fixture".into(),
            provider: "local".into(),
            provider_id: "fixture".into(),
            source_url: audio.to_string_lossy().into_owned(),
            title: req.track_name.clone(),
            artist: req.artist_name.clone(),
            duration_seconds: Some(req.duration_seconds),
            ..Track::default()
        };
        let asset = LocalAsset {
            id: "asset-fixture".into(),
            recording_id: track.id.clone(),
            path: audio.to_str().unwrap().into(),
            hash: format!("{:x}", Sha256::digest(std::fs::read(&audio).unwrap())),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: track.duration_seconds,
            size_bytes: None,
            bitrate_origin: None,
            processing: "fixture".into(),
            track: track.clone(),
        };
        store.save_asset(&asset).unwrap();
        let result = local_lyrics(&store, &track.id, &asset.path, &req)
            .await
            .unwrap();
        assert_eq!(result.provider, "localLrc");
        assert_eq!(result.status, "synced");
        assert!(local_lyrics(&store, "another-recording", &asset.path, &req)
            .await
            .is_none());
        let mut wrong = req.clone();
        wrong.artist_name = "Other performer".into();
        assert!(local_lyrics(&store, &track.id, &asset.path, &wrong)
            .await
            .is_none());
        std::fs::write(&audio, b"changed outside app").unwrap();
        assert!(local_lyrics(&store, &track.id, &asset.path, &req)
            .await
            .is_none());
    }

    fn visible_input() -> LyricsRequestInput {
        LyricsRequestInput {
            track_id: None,
            local_path: None,
            track_name: "Visible Song".into(),
            artist_name: "Visible Artist".into(),
            album_name: None,
            duration_seconds: 201.5,
            playback_range: None,
            radio: false,
        }
    }

    #[test]
    fn uses_visible_webview_metadata_but_native_snapshot_is_authoritative() {
        let webview = serde_json::json!({"engine":"webview","status":"idle"});
        let visible = request_for_current_track(&webview, Some(visible_input())).unwrap();
        assert_eq!(visible.track_name, "Visible Song");
        assert_eq!(visible.album_name, None);

        let native = serde_json::json!({
            "engine":"native","status":"playing",
            "track":{"title":"Native Song","artist":"Native Artist","durationSeconds":199.0}
        });
        let request = request_for_current_track(&native, Some(visible_input())).unwrap();
        assert_eq!(request.track_name, "Native Song");
    }

    #[tokio::test]
    #[ignore = "Explicit public metadata-only radio lyric lookup; no radio audio sent"]
    async fn live_identified_radio_song_returns_plain_lyrics_without_duration() {
        let snapshot = serde_json::json!({"engine":"native","status":"playing","duration":90000,"track":{"id":"radio:lyrics-live-fixture","provider":"radio","title":"Station","artist":"Station"},"radioNowPlaying":{"title":"Paper Wings","artist":"Gillian Welch","metadata":{}}});
        let result = get_current_track_lyrics_from_snapshot(&snapshot, None).await;
        assert_eq!(result.status, "unsynced");
        assert!(result.lines.as_ref().unwrap().len() > 5);
        assert!(result
            .lines
            .unwrap()
            .iter()
            .all(|line| line.time_seconds.is_none()));
        assert_eq!(result.matched.unwrap().artist_name, "Gillian Welch");
        let old = crate::lyrics_discovery::context(&snapshot, None).unwrap().0;
        let mut next = snapshot.clone();
        next["duration"] = serde_json::json!(100000);
        assert_eq!(
            crate::lyrics_discovery::context(&next, None).unwrap().0,
            old
        );
        next["radioNowPlaying"]["title"] = serde_json::json!("The Way It Goes");
        assert_ne!(
            crate::lyrics_discovery::context(&next, None).unwrap().0,
            old
        );
    }

    #[test]
    fn identified_radio_uses_song_metadata_and_never_stream_time_or_song_timestamps() {
        let mut snapshot = serde_json::json!({"engine":"native","status":"playing","duration":99999,"track":{"id":"radio:station","provider":"radio","title":"Station name","artist":"Station","durationSeconds":null},"radioNowPlaying":{"title":"Synthetic radio song","artist":"Synthetic performer","metadata":{"album":"Synthetic album"}}});
        let request = radio_request(&snapshot, None).unwrap();
        assert_eq!(request.duration_seconds, 0.);
        assert_eq!(request.track_name, "Synthetic radio song");
        let record:ApiRecord=serde_json::from_value(serde_json::json!({"trackName":"Synthetic radio song","artistName":"Synthetic performer","duration":240,"instrumental":false,"syncedLyrics":"[00:01]Synthetic radio lyric"})).unwrap();
        assert!(radio_record_matches(&request, &record));
        let document = untimed_record(&request, record.clone());
        assert_eq!(document.status, "unsynced");
        assert!(document
            .lines
            .unwrap()
            .iter()
            .all(|line| line.time_seconds.is_none()));
        let mut wrong = record;
        wrong.artist_name = "Another performer".into();
        assert!(!radio_record_matches(&request, &wrong));
        snapshot["radioNowPlaying"] = Value::Null;
        snapshot["duration"] = serde_json::json!(200);
        assert!(radio_request(&snapshot, None).is_none());
        assert!(request_for_current_track(&snapshot, None).is_none());
        snapshot["track"]["provider"] = serde_json::json!("youtube");
        snapshot["radioNowPlaying"] = serde_json::json!({"title":"Bogus","artist":"Bogus"});
        assert!(radio_request(&snapshot, None).is_none());
    }

    #[test]
    fn imported_full_track_lyrics_use_current_decoder_duration_without_editing_metadata() {
        let mut snapshot = serde_json::json!({"engine":"native","status":"playing","trackId":"import:song","duration":241.92,"track":{"id":"import:song","title":"The Way It Goes","artist":"Gillian Welch","durationSeconds":null}});
        let request = request_for_current_track(&snapshot, None).unwrap();
        assert_eq!(request.duration_seconds, 241.92);
        assert!(valid_request(&request));
        assert!(snapshot["track"]["durationSeconds"].is_null());
        snapshot["track"]["durationSeconds"] = serde_json::json!(242.);
        assert_eq!(
            request_for_current_track(&snapshot, None)
                .unwrap()
                .duration_seconds,
            242.
        );
        snapshot["track"]["durationSeconds"] = Value::Null;
        snapshot["trackId"] = serde_json::json!("different-item");
        assert!(request_for_current_track(&snapshot, None).is_none());
        snapshot["trackId"] = serde_json::json!("import:song");
        snapshot["track"]["playbackRange"] = serde_json::json!({"startTime":10.,"endTime":30.});
        assert!(request_for_current_track(&snapshot, None).is_none());
        snapshot["track"]["playbackRange"] = Value::Null;
        snapshot["track"]["provenance"] =
            serde_json::json!({"approximateExcerpt":{"startSeconds":10,"endSeconds":30}});
        assert!(request_for_current_track(&snapshot, None).is_none());
    }

    #[test]
    fn visible_excerpt_or_active_native_excerpt_is_not_queried() {
        let mut visible = visible_input();
        visible.playback_range = Some(serde_json::json!({"startTime":10.0,"endTime":20.0}));
        assert!(request_for_current_track(
            &serde_json::json!({"engine":"webview","status":"idle"}),
            Some(visible)
        )
        .is_none());
        let native_chapter = serde_json::json!({
            "engine":"native","status":"playing",
            "track":{"title":"Chapter","artist":"Artist","durationSeconds":201.0,"playbackRange":{"startTime":20.0}}
        });
        assert!(request_for_current_track(&native_chapter, Some(visible_input())).is_none());
    }
}
