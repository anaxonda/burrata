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
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration_seconds: f64,
    #[serde(default)]
    pub playback_range: Option<Value>,
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiRecord {
    // LRCLIB sends both `name` and `trackName`. `name` must remain an
    // ignored extra field, not an alias that makes serde reject duplicates.
    track_name: String,
    artist_name: String,
    album_name: Option<String>,
    duration: f64,
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
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

fn request_key(request: &LyricsRequest) -> String {
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

fn response(status: &str) -> LyricsResponse {
    LyricsResponse {
        status: status.into(),
        provider: "lrclib",
        source_hash: None,
        matched: None,
        lines: None,
        plain_lyrics: None,
    }
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

pub async fn get_current_track_lyrics(request: LyricsRequest) -> LyricsResponse {
    if !valid_text(&request.track_name)
        || !valid_text(&request.artist_name)
        || request
            .album_name
            .as_deref()
            .is_some_and(|album| !valid_text(album))
        || !request.duration_seconds.is_finite()
        || !(1.0..=3600.0).contains(&request.duration_seconds)
    {
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

    let result = shazam_fallback(&request, lookup(&url, &request).await);
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

/// Build a request only from the authoritative currently loaded playback item.
/// Chapter playback is skipped because its local time axis may be an excerpt.
pub(crate) fn request_from_snapshot(snapshot: &Value) -> Option<LyricsRequest> {
    if !matches!(
        snapshot.get("status")?.as_str()?,
        "playing" | "paused" | "buffering"
    ) {
        return None;
    }
    let track = snapshot.get("track")?.as_object()?;
    if track
        .get("playbackRange")
        .is_some_and(|range| !range.is_null())
    {
        return None;
    }
    Some(LyricsRequest {
        track_name: track.get("title")?.as_str()?.to_owned(),
        artist_name: track.get("artist")?.as_str()?.to_owned(),
        album_name: track
            .get("album")
            .and_then(Value::as_str)
            .map(str::to_owned),
        duration_seconds: track.get("durationSeconds")?.as_f64()?,
    })
}

pub async fn get_current_track_lyrics_from_snapshot(
    snapshot: &Value,
    visible_track: Option<LyricsRequestInput>,
) -> LyricsResponse {
    match request_for_current_track(snapshot, visible_track) {
        Some(request) => get_current_track_lyrics(request).await,
        None => response("noCurrentTrack"),
    }
}

fn request_for_current_track(
    snapshot: &Value,
    visible_track: Option<LyricsRequestInput>,
) -> Option<LyricsRequest> {
    let native_active = snapshot.get("engine").and_then(Value::as_str) == Some("native")
        && matches!(
            snapshot.get("status").and_then(Value::as_str),
            Some("playing" | "paused" | "buffering")
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
            return response("providerUnavailable");
        }
    };
    if http_response.status() == reqwest::StatusCode::NOT_FOUND {
        return search_fallback(&client, request).await;
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
        search_fallback(&client, request).await
    } else {
        classified
    }
}

async fn search_fallback(client: &reqwest::Client, request: &LyricsRequest) -> LyricsResponse {
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut url = match Url::parse(SEARCH_ENDPOINT) {
        Ok(url) => url,
        Err(_) => return response("providerUnavailable"),
    };
    // Search title-only so provider credit spelling/guest-credit formatting
    // cannot hide the record. Results are locally filtered by exact title,
    // matching performers and duration. Album is a ranking preference: the
    // same recording may have lyrics filed under a single or compilation.
    url.query_pairs_mut()
        .append_pair("track_name", request.track_name.trim());
    let mut http_response = match client.get(url).send().await {
        Ok(response) => response,
        Err(_) => return response("providerUnavailable"),
    };
    if !http_response.status().is_success()
        || http_response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return response("providerUnavailable");
    }
    let bytes = match read_bounded_body(&mut http_response).await {
        Some(bytes) => bytes,
        None => return response("providerUnavailable"),
    };
    let records: Vec<ApiRecord> = match serde_json::from_slice(&bytes) {
        Ok(records) => records,
        Err(_) => return response("providerUnavailable"),
    };
    if records.is_empty() {
        return response("notFound");
    }
    let mut candidates = records
        .into_iter()
        .filter(|record| record_matches(request, record))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let left_score = record_score(request, left);
        let right_score = record_score(request, right);
        left_score.total_cmp(&right_score)
    });
    match candidates.into_iter().next() {
        Some(record) => classify_record(request, record),
        None => response("unmatched"),
    }
}

async fn read_bounded_body(response: &mut reqwest::Response) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES => {
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

fn record_matches(request: &LyricsRequest, record: &ApiRecord) -> bool {
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

fn classify_record(request: &LyricsRequest, record: ApiRecord) -> LyricsResponse {
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
        };
    }
    if let Some(raw) = record
        .synced_lyrics
        .filter(|lyrics| !lyrics.trim().is_empty() && lyrics.len() <= MAX_LYRICS_BYTES)
    {
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
        };
    }
    response("notFound")
}

fn parse_lrc(raw: &str) -> Vec<LyricsLine> {
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

    fn visible_input() -> LyricsRequestInput {
        LyricsRequestInput {
            track_name: "Visible Song".into(),
            artist_name: "Visible Artist".into(),
            album_name: None,
            duration_seconds: 201.5,
            playback_range: None,
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
