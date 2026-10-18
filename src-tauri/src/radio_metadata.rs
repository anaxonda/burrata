//! Station descriptions, stream metadata, and automatic fallback song recognition.
use super::{
    get_radio_metadata, get_radio_stations_saved, RadioMetadata, RadioStation, RadioTrackMetadata,
};
use crate::{storage::Store, tools};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;
use std::{
    collections::HashMap,
    env, fs,
    future::Future,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
use tokio::sync::{watch, Mutex};

const HISTORY_KEY: &str = "radioHistory.v1";
const HISTORY_STATION_LIMIT: usize = 200;
const HISTORY_ITEM_LIMIT: usize = 50;
const MAX_HOME_PAGE_BYTES: usize = 256 * 1024;
const MAX_ICY_BYTES: usize = 1024 * 1024;
const SONGREC_CAPTURE_SECONDS: u8 = 12;
const RECOGNITION_COOLDOWN: Duration = Duration::from_secs(90);
const RECOGNITION_CACHE_LIMIT: usize = 128;
static SONGREC_GATE: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
static RECOGNITION_STATE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, RecognitionState>>> =
    std::sync::OnceLock::new();

#[derive(Clone)]
struct RecognitionState {
    attempted_at: Instant,
    in_flight: bool,
    cached: Option<(RadioTrackMetadata, Instant)>,
}

enum RecognitionPolicy {
    Cached(RadioTrackMetadata),
    Skip(&'static str),
    Capture,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecognitionMode {
    Automatic,
    Manual,
}

fn recognition_policy_for(
    state: Option<&RecognitionState>,
    now: Instant,
    mode: RecognitionMode,
) -> RecognitionPolicy {
    let Some(state) = state else {
        return RecognitionPolicy::Capture;
    };
    match mode {
        RecognitionMode::Automatic => {
            if let Some((track, expires_at)) = &state.cached {
                if *expires_at > now {
                    return RecognitionPolicy::Cached(track.clone());
                }
            }
            if state.in_flight {
                return RecognitionPolicy::Skip(
                    "Automatic song recognition is already running for this station.",
                );
            }
            if now.saturating_duration_since(state.attempted_at) < RECOGNITION_COOLDOWN {
                return RecognitionPolicy::Skip("Song recognition was tried recently; Burrata will retry shortly if stream metadata remains unavailable.");
            }
        }
        RecognitionMode::Manual => {
            if state.in_flight {
                return RecognitionPolicy::Skip(
                    "Song identification is already running for this station.",
                );
            }
            if now.saturating_duration_since(state.attempted_at) < RECOGNITION_COOLDOWN {
                return RecognitionPolicy::Skip(
                    "Song identification was tried recently; wait for its cooldown before retrying.",
                );
            }
        }
    }
    RecognitionPolicy::Capture
}

fn recognition_states() -> &'static std::sync::Mutex<HashMap<String, RecognitionState>> {
    RECOGNITION_STATE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

struct RecognitionFlightGuard {
    station_id: String,
    attempted_at: Instant,
    completed: bool,
}

impl Drop for RecognitionFlightGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if let Some(state) = recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_mut(&self.station_id)
        {
            if state.attempted_at == self.attempted_at {
                state.in_flight = false;
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RadioHistoryState {
    stations: std::collections::BTreeMap<String, Vec<RadioTrackMetadata>>,
}

impl Default for RadioHistoryState {
    fn default() -> Self {
        Self {
            stations: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadioStationDetails {
    pub station_id: String,
    pub name: String,
    pub stream_url: String,
    pub source_url: String,
    pub homepage_url: Option<String>,
    pub description: Option<String>,
    pub now_playing: Option<RadioTrackMetadata>,
    pub history: Vec<RadioTrackMetadata>,
    pub history_available: bool,
    pub note: String,
}

/// Fetch descriptive station information without updating history. Opening a
/// station's details must not imply that it is playing or create listening history.
pub async fn get_station_details(
    station_id: String,
    store: &Store,
) -> Result<RadioStationDetails, String> {
    let station = find_station(&station_id, store)?;
    let homepage_url = station
        .homepage_url
        .as_deref()
        .and_then(super::safe_http_url);
    let fetched_description = match homepage_url.as_deref() {
        Some(url) => fetch_station_description(url).await.ok().flatten(),
        None => None,
    };
    let description = fetched_description.or_else(|| curated_description(&station));
    let metadata = read_current_metadata(&station).await;
    let history = read_history(store, &station_id)?;
    let note = if metadata.now_playing.is_some() {
        "Current metadata is supplied by the station stream or service; it is not a verified catalog match.".into()
    } else {
        "No current track metadata is available. Automatic song recognition runs while this station is playing.".into()
    };
    Ok(RadioStationDetails {
        station_id,
        name: station.name,
        stream_url: station.stream_url,
        source_url: station.source_url,
        homepage_url,
        description,
        now_playing: metadata.now_playing,
        history_available: !history.is_empty(),
        history,
        note,
    })
}

fn curated_description(station: &RadioStation) -> Option<String> {
    match station.id.as_str() {
        "main" => Some(
            "Radio Paradise's Main Mix: eclectic rock, world music, electronica and more.".into(),
        ),
        "mellow" => Some("Radio Paradise's Mellow Mix.".into()),
        "rock" => Some("Radio Paradise's Rock Mix.".into()),
        "global" => Some("Radio Paradise's Global Mix.".into()),
        "the-north" => Some("Community public radio serving Duluth–Superior.".into()),
        _ => None,
    }
}

/// Poll metadata only for a stream that the app considers actively playing.
/// When `recognize` is true, a 12-second stream excerpt is processed through
/// configured ffmpeg and an already-installed SongRec CLI. No microphone is used.
pub async fn poll_radio_station(
    station_id: String,
    recognize: bool,
    store: &Store,
) -> Result<RadioMetadata, String> {
    let station = find_station(&station_id, store)?;
    let mut metadata = read_current_metadata(&station).await;
    if metadata.now_playing.is_none() && recognize {
        match recognize_station_track(&station, store).await {
            Ok((track, note)) => {
                metadata.now_playing = track;
                if let Some(note) = note {
                    metadata.note = note;
                }
            }
            Err(error) => {
                // Recognition is an optional enhancement; preserve stream polling and
                // history even when local capture or SongRec is unavailable.
                metadata.note = format!("{} Song recognition unavailable: {error}", metadata.note);
            }
        }
    }
    if let Some(mut track) = metadata.now_playing.take() {
        if valid_history_track(&track) {
            if track.observed_at.is_none() {
                track.observed_at = Some(chrono::Utc::now().to_rfc3339());
            }
            append_history(store, &station_id, &track)?;
        }
        metadata.now_playing = Some(track);
    }
    metadata.history = read_history(store, &station_id)?;
    metadata.history_available = !metadata.history.is_empty();
    if metadata.history_available {
        metadata.note = "Showing tracks observed while this station was playing in Burrata. Station metadata and SongRec results are not catalog-verified.".into();
    }
    Ok(metadata)
}

/// Force a fresh SongRec sample of the selected station's stream. This path is
/// user initiated and does not depend on the automatic-recognition setting or
/// the station's current ICY/provider metadata. Per-station cooldown and the
/// global SongRec gate still prevent duplicate captures.
pub async fn identify_radio_station(
    station_id: String,
    store: &Store,
) -> Result<RadioMetadata, String> {
    let station = find_station(&station_id, store)?;
    let (now_playing, note) =
        recognize_station_track_with(&station, RecognitionMode::Manual, || {
            recognize_station_track_uncached(&station, store)
        })
        .await?;
    let note = note.unwrap_or_else(|| {
        if now_playing.is_some() {
            "SongRec sampled the current station stream; catalog identity is unverified.".into()
        } else {
            "SongRec did not identify a track in this station stream excerpt.".into()
        }
    });
    let history = read_history(store, &station_id)?;
    Ok(RadioMetadata {
        station_id,
        now_playing,
        seconds_remaining: None,
        history_available: !history.is_empty(),
        history,
        note,
    })
}

fn find_station(station_id: &str, store: &Store) -> Result<RadioStation, String> {
    if station_id.is_empty() || station_id.len() > 120 {
        return Err("Radio station ID is invalid".into());
    }
    get_radio_stations_saved(store)?
        .into_iter()
        .find(|station| station.id == station_id)
        .ok_or_else(|| "Radio station was not found".to_string())
}

async fn read_current_metadata(station: &RadioStation) -> RadioMetadata {
    if matches!(station.id.as_str(), "main" | "mellow" | "rock" | "global") {
        return get_radio_metadata(station.id.clone())
            .await
            .unwrap_or_else(|error| empty_metadata(&station.id, error));
    }
    match fetch_icy_track(station).await {
        Ok(Some(track)) => RadioMetadata {
            station_id: station.id.clone(),
            now_playing: Some(track),
            seconds_remaining: None,
            history: Vec::new(),
            history_available: false,
            note: "Current track was read from the stream's ICY metadata; catalog identity is unverified.".into(),
        },
        Ok(None) => empty_metadata(
            &station.id,
            "This stream did not provide a readable current-track ICY tag.".into(),
        ),
        Err(error) => empty_metadata(&station.id, error),
    }
}

fn empty_metadata(station_id: &str, note: String) -> RadioMetadata {
    RadioMetadata {
        station_id: station_id.to_owned(),
        now_playing: None,
        seconds_remaining: None,
        history: Vec::new(),
        history_available: false,
        note,
    }
}

async fn fetch_station_description(url: &str) -> Result<Option<String>, String> {
    let parsed = super::validate_stream_url(url)?;
    if parsed.scheme() != "https" || !public_host(parsed.host_str().unwrap_or_default()) {
        return Err("Station homepage must be a public HTTPS site".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(7))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!(
            "Burrata/",
            env!("CARGO_PKG_VERSION"),
            " (station details)"
        ))
        .build()
        .map_err(|error| format!("Could not prepare station details request: {error}"))?;
    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("Could not load station details: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Station details response failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HOME_PAGE_BYTES as u64)
    {
        return Err("Station homepage is too large".into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Could not read station homepage: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_HOME_PAGE_BYTES {
            return Err("Station homepage is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let html = String::from_utf8_lossy(&body);
    Ok(parse_station_description(&html))
}

fn public_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_broadcast()
                && !ip.is_multicast()
        }
        Ok(IpAddr::V6(ip)) => {
            !ip.is_loopback()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
        }
        Err(_) => !host.is_empty(),
    }
}

fn parse_station_description(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for selector in [
        "meta[name=description]",
        "meta[property='og:description']",
        "meta[name='twitter:description']",
    ] {
        let selector = Selector::parse(selector).ok()?;
        if let Some(value) = document
            .select(&selector)
            .find_map(|meta| meta.value().attr("content"))
            .and_then(clean_description)
        {
            return Some(value);
        }
    }
    None
}

fn clean_description(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.trim();
    (!normalized.is_empty() && normalized.chars().count() <= 500).then(|| normalized.to_owned())
}

async fn fetch_icy_track(station: &RadioStation) -> Result<Option<RadioTrackMetadata>, String> {
    let url = super::validate_stream_url(&station.stream_url)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!(
            "Burrata/",
            env!("CARGO_PKG_VERSION"),
            " (ICY metadata)"
        ))
        .build()
        .map_err(|error| format!("Could not prepare stream metadata request: {error}"))?;
    let response = client
        .get(url)
        .header("Icy-MetaData", "1")
        .send()
        .await
        .map_err(|error| format!("Could not read station stream metadata: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Station stream metadata response failed: {error}"))?;
    let interval = response
        .headers()
        .get("icy-metaint")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=MAX_ICY_BYTES).contains(value));
    let Some(interval) = interval else {
        return Ok(None);
    };
    let mut response = response;
    let target = interval.saturating_add(1 + 4080);
    let mut body = Vec::with_capacity(target.min(MAX_ICY_BYTES + 4081));
    while body.len() < target {
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("Could not read ICY metadata: {error}"))?
        else {
            break;
        };
        let take = (target - body.len()).min(chunk.len());
        body.extend_from_slice(&chunk[..take]);
        if body.len() >= MAX_ICY_BYTES + 1 {
            break;
        }
    }
    if body.len() <= interval {
        return Ok(None);
    }
    let block_length = usize::from(body[interval]) * 16;
    if block_length == 0 || interval + 1 + block_length > body.len() {
        return Ok(None);
    }
    let block = String::from_utf8_lossy(&body[interval + 1..interval + 1 + block_length]);
    let Some(title) = parse_icy_stream_title(&block) else {
        return Ok(None);
    };
    let metadata = icy_track_metadata(station, &title);
    if is_station_branding_title(station, &metadata) {
        return Ok(None);
    }
    Ok(Some(metadata))
}

fn is_station_branding_title(station: &RadioStation, metadata: &RadioTrackMetadata) -> bool {
    if !metadata.artist.trim().is_empty() {
        return false;
    }
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let station_name = normalize(&station.name);
    let title = normalize(&metadata.title);
    station_name.chars().count() >= 4 && title.contains(&station_name)
}

fn parse_icy_stream_title(block: &str) -> Option<String> {
    let marker = "StreamTitle=";
    let start = block.find(marker)? + marker.len();
    let rest = block[start..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let value = rest[quote.len_utf8()..].split(quote).next()?;
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= 300 && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn icy_track_metadata(station: &RadioStation, stream_title: &str) -> RadioTrackMetadata {
    let (artist, title) = stream_title
        .split_once(" - ")
        .map(|(artist, title)| (artist.trim().to_owned(), title.trim().to_owned()))
        .filter(|(artist, title)| !artist.is_empty() && !title.is_empty())
        .unwrap_or_else(|| (String::new(), stream_title.trim().to_owned()));
    RadioTrackMetadata {
        station_id: station.id.clone(),
        artist,
        title,
        album: None,
        year: None,
        artwork_url: None,
        source_url: station.source_url.clone(),
        observed_at: Some(chrono::Utc::now().to_rfc3339()),
        metadata_confidence: "station-reported ICY stream metadata; catalog identity unverified"
            .into(),
    }
}

fn valid_history_track(track: &RadioTrackMetadata) -> bool {
    !track.title.trim().is_empty()
        && track.title.chars().count() <= 300
        && !track.title.chars().any(char::is_control)
        && track.artist.chars().count() <= 300
        && !track.artist.chars().any(char::is_control)
}

fn read_history(store: &Store, station_id: &str) -> Result<Vec<RadioTrackMetadata>, String> {
    let Some(value) = store.get_json_preference(HISTORY_KEY)? else {
        return Ok(Vec::new());
    };
    let state: RadioHistoryState = serde_json::from_value(value)
        .map_err(|error| format!("Saved radio history could not be read: {error}"))?;
    Ok(state
        .stations
        .get(station_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(HISTORY_ITEM_LIMIT)
        .collect())
}

fn append_history(
    store: &Store,
    station_id: &str,
    track: &RadioTrackMetadata,
) -> Result<(), String> {
    let mut state = match store.get_json_preference(HISTORY_KEY)? {
        Some(value) => serde_json::from_value::<RadioHistoryState>(value)
            .unwrap_or_else(|_| RadioHistoryState::default()),
        None => RadioHistoryState::default(),
    };
    let history = state.stations.entry(station_id.to_owned()).or_default();
    if history
        .first()
        .is_some_and(|previous| same_track(previous, track))
    {
        return Ok(());
    }
    let mut observed = track.clone();
    if observed.observed_at.is_none() {
        observed.observed_at = Some(chrono::Utc::now().to_rfc3339());
    }
    history.insert(0, observed);
    history.truncate(HISTORY_ITEM_LIMIT);
    while state.stations.len() > HISTORY_STATION_LIMIT {
        if let Some(oldest_key) = state.stations.keys().next().cloned() {
            state.stations.remove(&oldest_key);
        }
    }
    let value = serde_json::to_value(state).map_err(|error| error.to_string())?;
    store.set_json_preference(HISTORY_KEY, &value)
}

fn same_track(a: &RadioTrackMetadata, b: &RadioTrackMetadata) -> bool {
    a.station_id == b.station_id
        && a.artist.trim().eq_ignore_ascii_case(b.artist.trim())
        && a.title.trim().eq_ignore_ascii_case(b.title.trim())
}

async fn recognize_station_track(
    station: &RadioStation,
    store: &Store,
) -> Result<(Option<RadioTrackMetadata>, Option<String>), String> {
    recognize_station_track_with(station, RecognitionMode::Automatic, || {
        recognize_station_track_uncached(station, store)
    })
    .await
}

async fn recognize_station_track_with<F, Fut>(
    station: &RadioStation,
    mode: RecognitionMode,
    recognize: F,
) -> Result<(Option<RadioTrackMetadata>, Option<String>), String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Option<RadioTrackMetadata>, String>>,
{
    let now = Instant::now();
    {
        let mut states = recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match recognition_policy_for(states.get(&station.id), now, mode) {
            RecognitionPolicy::Cached(track) => {
                return Ok((Some(track), Some("Showing a recent SongRec match; it will be refreshed after the cache expires.".into())));
            }
            RecognitionPolicy::Skip(note) => return Ok((None, Some(note.into()))),
            RecognitionPolicy::Capture => {}
        }
        if states.len() >= RECOGNITION_CACHE_LIMIT && !states.contains_key(&station.id) {
            if let Some(oldest) = states
                .iter()
                .min_by_key(|(_, state)| state.attempted_at)
                .map(|(id, _)| id.clone())
            {
                states.remove(&oldest);
            }
        }
        states.insert(
            station.id.clone(),
            RecognitionState {
                attempted_at: now,
                in_flight: true,
                cached: None,
            },
        );
    }

    let mut flight_guard = RecognitionFlightGuard {
        station_id: station.id.clone(),
        attempted_at: now,
        completed: false,
    };
    let result = recognize().await;
    let mut states = recognition_states()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(state) = states.get_mut(&station.id) {
        state.in_flight = false;
        if let Ok(Some(track)) = &result {
            state.cached = Some((track.clone(), Instant::now() + RECOGNITION_COOLDOWN));
        }
    }
    flight_guard.completed = true;
    match result {
        Ok(track) => Ok((track, None)),
        Err(error) => Err(error),
    }
}

async fn recognize_station_track_uncached(
    station: &RadioStation,
    store: &Store,
) -> Result<Option<RadioTrackMetadata>, String> {
    let Some(songrec) = find_executable(&["songrec", "SongRec"]) else {
        return Err("SongRec is not installed. Burrata will not install recognition software automatically.".into());
    };
    let settings = store.settings()?;
    let gate = SONGREC_GATE.get_or_init(|| Mutex::new(()));
    let _permit = gate.lock().await;
    let wav = env::temp_dir().join(format!("burrata-radio-{}.wav", uuid::Uuid::new_v4()));
    let mut capture =
        tokio::process::Command::new(tools::executable(&settings.ffmpeg_path, "ffmpeg"));
    capture
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&station.stream_url)
        .args([
            "-t",
            &SONGREC_CAPTURE_SECONDS.to_string(),
            "-vn",
            "-ac",
            "2",
            "-ar",
            "44100",
            "-fs",
            "4000000",
            "-f",
            "wav",
        ])
        .arg(&wav)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let captured = tokio::time::timeout(Duration::from_secs(30), capture.status()).await;
    if !matches!(captured, Ok(Ok(status)) if status.success()) {
        let _ = fs::remove_file(&wav);
        return Err(
            "Could not capture a short audio excerpt from the selected radio stream.".into(),
        );
    }
    let output = run_songrec(&songrec, &wav).await;
    let _ = fs::remove_file(&wav);
    parse_songrec_output(station, output?)
}

/// Recognize the last audible PCM sample supplied by the playback tap. This
/// deliberately accepts a WAV path and never opens the station stream. The
/// same per-station cache/cooldown and global SongRec gate used by automatic
/// station recognition apply here.
pub async fn recognize_played_sample(
    station: &RadioStation,
    wav_path: &Path,
    manual: bool,
) -> Result<(Option<RadioTrackMetadata>, Option<String>), String> {
    let wav_path = wav_path.to_path_buf();
    recognize_played_sample_with_capture(station, manual, || async move { Ok(wav_path) }).await
}

/// Identify a caller-captured WAV without reopening a station stream or
/// consulting/updating the station-keyed radio metadata cache.
pub async fn recognize_captured_sample_uncached(
    station: &RadioStation,
    wav_path: &Path,
) -> Result<Option<RadioTrackMetadata>, String> {
    let Some(songrec) = find_executable(&["songrec", "SongRec"]) else {
        return Err("SongRec is not installed. Burrata will not install recognition software automatically.".into());
    };
    let gate = SONGREC_GATE.get_or_init(|| Mutex::new(()));
    let _permit = gate.lock().await;
    let output = run_songrec(&songrec, wav_path).await?;
    parse_songrec_output(station, output)
}

/// Uncached recognition with cooperative cancellation. Callers must await this
/// future after signalling cancellation so child reaping precedes WAV cleanup.
pub async fn recognize_captured_sample_cancellable(
    station: &RadioStation,
    wav_path: &Path,
    cancel: watch::Receiver<bool>,
) -> Result<Option<RadioTrackMetadata>, String> {
    let Some(songrec) = find_executable(&["songrec", "SongRec"]) else {
        return Err("SongRec is not installed. Burrata will not install recognition software automatically.".into());
    };
    let gate = SONGREC_GATE.get_or_init(|| Mutex::new(()));
    let output =
        run_songrec_cancellable(&songrec, wav_path, cancel, gate, Duration::from_secs(45)).await?;
    parse_songrec_output(station, output)
}

async fn recognition_cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn run_songrec_cancellable(
    songrec: &Path,
    wav: &Path,
    mut cancel: watch::Receiver<bool>,
    gate: &Mutex<()>,
    deadline: Duration,
) -> Result<Vec<u8>, String> {
    let _permit = tokio::select! {
        biased;
        _ = recognition_cancelled(&mut cancel) => return Err("Audio identification was cancelled".into()),
        permit = gate.lock() => permit,
    };
    if *cancel.borrow() {
        return Err("Audio identification was cancelled".into());
    }
    let mut child = tokio::process::Command::new(songrec)
        .arg("recognize")
        .arg("--json")
        .arg(wav)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "SongRec could not start recognition.".to_owned())?;
    let stdout = child.stdout.take().ok_or("SongRec output is unavailable")?;
    // Recognition JSON is small. Bound hostile/broken recognizer output while
    // reading concurrently so a full pipe cannot prevent child completion.
    let work = async {
        let read = async {
            let mut bytes = Vec::new();
            stdout.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
            if bytes.len() > 1024 * 1024 {
                return Err(std::io::Error::other("recognition output exceeded limit"));
            }
            Ok(bytes)
        };
        tokio::try_join!(child.wait(), read)
    };
    let result = tokio::select! {
        biased;
        _ = recognition_cancelled(&mut cancel) => Err("Audio identification was cancelled".to_owned()),
        result = tokio::time::timeout(deadline, work) => match result {
            Ok(Ok((status, bytes))) if status.success() => Ok(bytes),
            Ok(_) => Err("SongRec could not recognize this audio excerpt.".to_owned()),
            Err(_) => Err("SongRec did not return a recognition result in time.".to_owned()),
        },
    };
    if result.is_err() {
        // Explicit kill+wait is the primary cleanup; kill_on_drop is only the
        // backup if the owning task itself is forcibly aborted.
        let _ = child.start_kill();
        match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(_)) => {}
            _ => return Err("SongRec cleanup did not finish within two seconds.".into()),
        }
    }
    result
}

/// Variant for callers that must acquire a short PCM snapshot. Recognition
/// policy is checked before `capture` runs, so cooldown/cache hits do not read
/// or write another sample.
pub async fn recognize_played_sample_with_capture<F, Fut>(
    station: &RadioStation,
    manual: bool,
    capture: F,
) -> Result<(Option<RadioTrackMetadata>, Option<String>), String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<std::path::PathBuf, String>>,
{
    let mode = if manual {
        RecognitionMode::Manual
    } else {
        RecognitionMode::Automatic
    };
    recognize_station_track_with(station, mode, || async move {
        let Some(songrec) = find_executable(&["songrec", "SongRec"]) else {
            return Err("SongRec is not installed. Burrata will not install recognition software automatically.".into());
        };
        let gate = SONGREC_GATE.get_or_init(|| Mutex::new(()));
        let _permit = gate.lock().await;
        let wav_path = capture().await?;
        let output = run_songrec(&songrec, &wav_path).await;
        parse_songrec_output(station, output?)
    })
    .await
}

async fn run_songrec(songrec: &Path, wav: &Path) -> Result<Vec<u8>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new(songrec)
            .arg("recognize")
            .arg("--json")
            .arg(wav)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        return Err("SongRec did not return a recognition result in time.".into());
    };
    if !output.status.success() {
        return Err("SongRec could not recognize this audio excerpt.".into());
    }
    Ok(output.stdout)
}

fn parse_songrec_output(
    station: &RadioStation,
    stdout: Vec<u8>,
) -> Result<Option<RadioTrackMetadata>, String> {
    // SongRec emits no JSON when it has no match. Empty output is a clean
    // no-result; nonempty malformed output is an explicit provider failure.
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_slice::<Value>(&stdout) else {
        return Err("SongRec returned a nonempty response that was not valid JSON.".into());
    };
    let track = value.get("track").unwrap_or(&value);
    let title = clean_songrec_field(
        track["title"]
            .as_str()
            .or_else(|| track["heading"].as_str()),
    );
    let artist = clean_songrec_field(
        track["subtitle"]
            .as_str()
            .or_else(|| track["artist"].as_str())
            .or_else(|| track["artists"].as_array()?.first()?.get("name")?.as_str()),
    );
    let (Some(title), Some(artist)) = (title, artist) else {
        return Ok(None);
    };
    Ok(Some(RadioTrackMetadata {
        station_id: station.id.clone(),
        artist,
        title,
        album: None,
        year: None,
        artwork_url: None,
        source_url: station.source_url.clone(),
        observed_at: Some(chrono::Utc::now().to_rfc3339()),
        metadata_confidence: "SongRec audio-fingerprint result; catalog identity unverified".into(),
    }))
}

fn clean_songrec_field(value: Option<&str>) -> Option<String> {
    let value = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty() && value.chars().count() <= 300 && !value.chars().any(char::is_control))
        .then_some(value)
}

fn find_executable(names: &[&str]) -> Option<std::path::PathBuf> {
    names
        .iter()
        .map(|name| tools::executable(name, name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    async fn fake_songrec_cleanup(cancel_after_ready: bool) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-songrec");
        let wav = dir.path().join("sample.wav");
        let marker = dir.path().join("sample.wav.pid");
        std::fs::write(&wav, b"owned disposable WAV").unwrap();
        // exec preserves the handshake PID and avoids creating grandchildren.
        std::fs::write(
            &script,
            "#!/bin/sh\necho $$ > \"$3.pid\"\nexec /bin/sleep 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = watch::channel(false);
        let gate = std::sync::Arc::new(Mutex::new(()));
        let owned_gate = gate.clone();
        let owned_wav = wav.clone();
        let work = tokio::spawn(async move {
            run_songrec_cancellable(
                &script,
                &owned_wav,
                rx,
                &owned_gate,
                if cancel_after_ready {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(300)
                },
            )
            .await
        });
        let pid = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = text.trim().parse::<libc::pid_t>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if cancel_after_ready {
            tx.send_replace(true);
        }
        let result = tokio::time::timeout(Duration::from_secs(3), work)
            .await
            .unwrap()
            .unwrap();
        let error = result.unwrap_err();
        assert!(
            error.contains(if cancel_after_ready {
                "cancelled"
            } else {
                "in time"
            }),
            "{error}"
        );
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "owned recognizer must be gone before return"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        // waitpid ECHILD additionally proves the owned child was reaped.
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        std::fs::remove_file(&wav).unwrap();
        assert!(gate.try_lock().is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellable_songrec_reaps_owned_child_before_wav_cleanup() {
        fake_songrec_cleanup(true).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_songrec_reaps_owned_child_before_wav_cleanup() {
        fake_songrec_cleanup(false).await;
    }

    #[tokio::test]
    async fn cancelled_songrec_gate_wait_never_spawns_or_unlocks_other_owner() {
        let gate = Mutex::new(());
        let permit = gate.lock().await;
        let (tx, rx) = watch::channel(false);
        let work = run_songrec_cancellable(
            Path::new("nonexistent-owned-fixture"),
            Path::new("unused"),
            rx,
            &gate,
            Duration::from_secs(1),
        );
        tokio::pin!(work);
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
                work.as_mut(),
                cx
            )))
            .await
            .is_pending()
        );
        tx.send_replace(true);
        assert!(tokio::time::timeout(Duration::from_millis(100), work)
            .await
            .unwrap()
            .unwrap_err()
            .contains("cancelled"));
        assert!(gate.try_lock().is_err());
        drop(permit);
    }

    use super::*;

    fn station() -> RadioStation {
        RadioStation {
            id: "test-station".into(),
            name: "Test Station".into(),
            stream_url: "https://radio.example/live".into(),
            source_url: "https://radio.example/listen".into(),
            provider: Some("custom".into()),
            country: None,
            tags: None,
            homepage_url: Some("https://radio.example/".into()),
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: true,
        }
    }

    fn test_store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("radio-metadata.sqlite")).unwrap();
        let mut stations = vec![];
        stations.push(station());
        store
            .set_json_preference("radioStations.v1", &serde_json::to_value(stations).unwrap())
            .unwrap();
        (directory, store)
    }

    #[test]
    fn description_parser_accepts_standard_meta_tags_and_caps_text() {
        assert_eq!(
            parse_station_description(
                "<meta name='description' content='  Local  radio &amp; music  '>"
            ),
            Some("Local radio & music".into())
        );
        assert_eq!(
            parse_station_description("<meta property='og:description' content='A station'>"),
            Some("A station".into())
        );
        assert!(parse_station_description(&format!(
            "<meta name='description' content='{}'>",
            "x".repeat(501)
        ))
        .is_none());
    }

    #[test]
    fn curated_station_descriptions_cover_homepages_without_meta_tags() {
        assert_eq!(
            curated_description(&station()),
            None,
            "custom stations should not receive invented descriptions"
        );
        let north = RadioStation {
            id: "the-north".into(),
            name: "The North 103.3".into(),
            stream_url: "https://thenorth1033.streamguys1.com/live".into(),
            source_url: "https://www.thenorth1033.org/listen-live".into(),
            provider: Some("internet_radio".into()),
            country: Some("United States".into()),
            tags: None,
            homepage_url: Some("https://www.thenorth1033.org/".into()),
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: false,
        };
        assert_eq!(
            curated_description(&north).as_deref(),
            Some("Community public radio serving Duluth–Superior.")
        );
    }

    #[test]
    fn homepage_fetch_policy_requires_public_https_host() {
        assert!(public_host("thenorth1033.org"));
        assert!(!public_host("localhost"));
        assert!(!public_host("192.168.1.2"));
        assert!(!public_host("::1"));
    }

    #[test]
    fn icy_stream_metadata_requires_a_nonempty_reported_track() {
        assert_eq!(
            parse_icy_stream_title("StreamTitle='Artist - Song';"),
            Some("Artist - Song".into())
        );
        assert_eq!(parse_icy_stream_title("StreamTitle='';"), None);
        assert_eq!(parse_icy_stream_title("StreamTitle=broken;"), None);
        let track = icy_track_metadata(&station(), "Artist - Song");
        assert_eq!(track.artist, "Artist");
        assert_eq!(track.title, "Song");
        assert!(track.observed_at.is_some());
    }

    #[test]
    fn north_station_branding_is_not_a_current_song_but_artist_tracks_remain() {
        let mut north = station();
        north.id = "the-north".into();
        north.name = "The North 103.3".into();
        let branding = icy_track_metadata(
            &north,
            "The North 103.3 FM. Independent. Intelligent. Diverse. Radio.",
        );
        assert!(is_station_branding_title(&north, &branding));
        let song = icy_track_metadata(&north, "Artist - Song");
        assert!(!is_station_branding_title(&north, &song));
        assert_eq!(song.artist, "Artist");
        assert_eq!(song.title, "Song");
    }

    #[test]
    fn history_deduplicates_only_adjacent_tracks_and_survives_store_reload() {
        let (_directory, store) = test_store();
        let track = |title: &str| RadioTrackMetadata {
            station_id: "test-station".into(),
            artist: "Artist".into(),
            title: title.into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: "https://radio.example/listen".into(),
            observed_at: None,
            metadata_confidence: "reported".into(),
        };
        append_history(&store, "test-station", &track("A")).unwrap();
        append_history(&store, "test-station", &track("A")).unwrap();
        append_history(&store, "test-station", &track("B")).unwrap();
        append_history(&store, "test-station", &track("A")).unwrap();
        let history = read_history(&store, "test-station").unwrap();
        assert_eq!(
            history
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "A"]
        );
        assert!(history.iter().all(|item| item.observed_at.is_some()));
    }

    #[test]
    fn empty_or_invalid_metadata_never_enters_history() {
        let (_directory, store) = test_store();
        let mut track = RadioTrackMetadata {
            station_id: "test-station".into(),
            artist: "Artist".into(),
            title: "  ".into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: "https://radio.example/listen".into(),
            observed_at: None,
            metadata_confidence: "reported".into(),
        };
        assert!(!valid_history_track(&track));
        track.title = "Real title".into();
        track.artist = "A".repeat(301);
        assert!(!valid_history_track(&track));
        assert!(read_history(&store, "test-station").unwrap().is_empty());
    }

    #[test]
    fn recognition_policy_reuses_fresh_match_and_cools_down_no_match_attempts() {
        let now = Instant::now();
        let recent_match = RecognitionState {
            attempted_at: now,
            in_flight: false,
            cached: Some((
                icy_track_metadata(&station(), "Artist - Song"),
                now + RECOGNITION_COOLDOWN,
            )),
        };
        assert!(matches!(
            recognition_policy_for(Some(&recent_match), now, RecognitionMode::Automatic),
            RecognitionPolicy::Cached(_)
        ));

        let no_match_attempt = RecognitionState {
            cached: None,
            ..recent_match.clone()
        };
        assert!(matches!(
            recognition_policy_for(Some(&no_match_attempt), now, RecognitionMode::Automatic),
            RecognitionPolicy::Skip(_)
        ));
        assert!(matches!(
            recognition_policy_for(
                Some(&no_match_attempt),
                now + RECOGNITION_COOLDOWN,
                RecognitionMode::Automatic,
            ),
            RecognitionPolicy::Capture
        ));

        let expired_match = RecognitionState {
            cached: Some((icy_track_metadata(&station(), "Artist - Old Song"), now)),
            ..recent_match
        };
        assert!(matches!(
            recognition_policy_for(
                Some(&expired_match),
                now + RECOGNITION_COOLDOWN,
                RecognitionMode::Automatic,
            ),
            RecognitionPolicy::Capture
        ));
    }

    #[test]
    fn recognition_policy_skips_overlapping_capture() {
        let now = Instant::now();
        let in_flight = RecognitionState {
            attempted_at: now,
            in_flight: true,
            cached: None,
        };
        assert!(matches!(
            recognition_policy_for(Some(&in_flight), now, RecognitionMode::Automatic),
            RecognitionPolicy::Skip(_)
        ));
    }

    #[test]
    fn songrec_parser_accepts_mock_result_and_handles_no_match() {
        let station = station();
        let parsed = parse_songrec_output(
            &station,
            br#"{"track":{"title":"Song","subtitle":"Artist"}}"#.to_vec(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.artist, "Artist");
        assert_eq!(parsed.title, "Song");
        assert!(parsed.metadata_confidence.contains("SongRec"));
        assert!(parse_songrec_output(&station, Vec::new())
            .unwrap()
            .is_none());
        assert!(parse_songrec_output(&station, b"not-json".to_vec()).is_err());
    }

    #[tokio::test]
    async fn played_sample_policy_skips_capture_during_automatic_cooldown() {
        let mut station = station();
        station.id = format!("played-sample-test-{}", uuid::Uuid::new_v4());
        recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                station.id.clone(),
                RecognitionState {
                    attempted_at: Instant::now(),
                    in_flight: false,
                    cached: None,
                },
            );
        let captures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let capture_count = captures.clone();
        let (_track, note) = recognize_played_sample_with_capture(&station, false, || async move {
            capture_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(std::path::PathBuf::from("unused.wav"))
        })
        .await
        .unwrap();
        assert!(note.unwrap().contains("tried recently"));
        assert_eq!(captures.load(std::sync::atomic::Ordering::SeqCst), 0);
        recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&station.id);
    }

    #[tokio::test]
    async fn cancelled_played_sample_releases_station_singleflight() {
        let mut station = station();
        station.id = format!("played-sample-cancel-{}", uuid::Uuid::new_v4());
        let station_id = station.id.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            recognize_station_track_with(&station, RecognitionMode::Automatic, || async move {
                let _ = started_tx.send(());
                std::future::pending::<Result<Option<RadioTrackMetadata>, String>>().await
            })
            .await
        });
        started_rx.await.unwrap();
        assert!(
            recognition_states()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&station_id)
                .unwrap()
                .in_flight
        );
        task.abort();
        let _ = task.await;
        assert!(
            !recognition_states()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&station_id)
                .unwrap()
                .in_flight
        );
        recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&station_id);
    }

    #[tokio::test]
    async fn manual_identification_ignores_cached_metadata_and_respects_cooldown_and_singleflight()
    {
        let mut station = station();
        station.id = format!("manual-test-{}", uuid::Uuid::new_v4());
        let now = Instant::now();
        let stale = RadioTrackMetadata {
            station_id: station.id.clone(),
            artist: "Old artist".into(),
            title: "Old track".into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: station.source_url.clone(),
            observed_at: None,
            metadata_confidence: "stale cached result".into(),
        };
        recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                station.id.clone(),
                RecognitionState {
                    attempted_at: now - RECOGNITION_COOLDOWN,
                    in_flight: false,
                    cached: Some((stale, now + RECOGNITION_COOLDOWN)),
                },
            );

        let captures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let capture_count = captures.clone();
        let fresh = RadioTrackMetadata {
            station_id: station.id.clone(),
            artist: "Fresh artist".into(),
            title: "Fresh track".into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: station.source_url.clone(),
            observed_at: None,
            metadata_confidence: "mock SongRec result".into(),
        };
        let (identified, note) =
            recognize_station_track_with(&station, RecognitionMode::Manual, || async move {
                capture_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Some(fresh))
            })
            .await
            .unwrap();
        assert_eq!(identified.unwrap().title, "Fresh track");
        assert!(note.is_none());
        assert_eq!(captures.load(std::sync::atomic::Ordering::SeqCst), 1);

        let capture_count = captures.clone();
        let (suppressed, note) =
            recognize_station_track_with(&station, RecognitionMode::Manual, || async move {
                capture_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            })
            .await
            .unwrap();
        assert!(suppressed.is_none());
        assert!(note.unwrap().contains("cooldown"));
        assert_eq!(captures.load(std::sync::atomic::Ordering::SeqCst), 1);

        {
            let mut states = recognition_states()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let state = states.get_mut(&station.id).unwrap();
            state.in_flight = true;
            state.attempted_at = now - RECOGNITION_COOLDOWN;
        }
        let capture_count = captures.clone();
        let (suppressed, note) =
            recognize_station_track_with(&station, RecognitionMode::Manual, || async move {
                capture_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            })
            .await
            .unwrap();
        assert!(suppressed.is_none());
        assert!(note.unwrap().contains("already running"));
        assert_eq!(captures.load(std::sync::atomic::Ordering::SeqCst), 1);
        recognition_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&station.id);
    }

    #[tokio::test]
    #[ignore = "requires public The North 103.3 stream access"]
    async fn live_the_north_stream_accepts_bounded_icy_metadata_request() {
        let north = super::super::get_radio_stations()
            .into_iter()
            .find(|station| station.id == "the-north")
            .unwrap();
        let description = fetch_station_description("https://www.thenorth1033.org/")
            .await
            .unwrap();
        assert!(description.is_some());
        let result = fetch_icy_track(&north).await.unwrap();
        if let Some(track) = result {
            assert!(!track.title.trim().is_empty());
            assert!(track.title.chars().count() <= 300);
        }
    }

    #[tokio::test]
    #[ignore = "explicitly captures a short The North stream excerpt and queries installed SongRec"]
    async fn live_the_north_songrec_fallback_uses_only_the_stream_excerpt() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("radio-recognition.sqlite")).unwrap();
        let north = super::super::get_radio_stations()
            .into_iter()
            .find(|station| station.id == "the-north")
            .unwrap();
        let (result, _) = recognize_station_track(&north, &store).await.unwrap();
        if let Some(track) = result {
            assert!(!track.artist.is_empty());
            assert!(!track.title.is_empty());
            assert!(track.metadata_confidence.contains("SongRec"));
        }
    }
}
