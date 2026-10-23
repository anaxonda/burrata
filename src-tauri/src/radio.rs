//! Read-only YouTube autoplay Mix recommendations for a selected song source.
use crate::{
    acquisition::{fetch_youtube_mix_entries, youtube_video_id},
    models::{Candidate, Settings, Track},
    storage::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

use std::future::Future;
use std::net::IpAddr;
use std::time::Duration;

#[path = "radio_artwork.rs"]
mod radio_artwork;
pub use radio_artwork::resolve_station_artwork;

#[path = "radio_metadata.rs"]
mod radio_metadata;
pub use radio_metadata::{
    get_station_details, identify_radio_station, poll_radio_station,
    recognize_captured_sample_cancellable, recognize_captured_sample_uncached,
    recognize_captured_sample_with_offset_cancellable, recognize_played_sample,
    recognize_played_sample_with_capture, recognize_played_samples_progressively,
    recognize_played_samples_progressively_manual, reconcile_played_radio_metadata,
    record_played_radio_identification, RadioStationDetails, SongRecRecognition,
};

pub(crate) use radio_metadata::safe_recognition_artwork_url;

const RADIO_PARADISE_SOURCE_URL: &str = "https://radioparadise.com/listen/stream-links";
const RADIO_PARADISE_API_URL: &str = "https://api.radioparadise.com/api/now_playing";
const MAX_RADIO_METADATA_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadioStation {
    pub id: String,
    pub name: String,
    pub stream_url: String,
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub is_custom: bool,
}

const SAVED_RADIO_STATIONS_KEY: &str = "radioStations.v1";
const HIDDEN_BUILT_IN_RADIO_STATIONS_KEY: &str = "hiddenRadioStations.v1";
const RADIO_BROWSER_SOURCE_URL: &str = "https://www.radio-browser.info/";
const RADIO_BROWSER_SERVERS: &[&str] = &[
    "https://de1.api.radio-browser.info",
    "https://nl1.api.radio-browser.info",
    "https://at1.api.radio-browser.info",
];
const MAX_STATIONS: usize = 200;
const MAX_PLAYLIST_BYTES: usize = 64 * 1024;
const RADIO_NAME_CANDIDATES: usize = 50;
const SONG_RADIO_DEADLINE: Duration = Duration::from_secs(20);

async fn within_song_radio_deadline<T>(
    duration: Duration,
    future: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(duration, future)
        .await
        .unwrap_or_else(
            |_| Err("Song radio recommendations exceeded the 20-second deadline".into()),
        )
}

#[derive(Debug, Deserialize)]
struct RadioBrowserStation {
    stationuuid: String,
    name: String,
    url_resolved: Option<String>,
    url: Option<String>,
    homepage: Option<String>,
    favicon: Option<String>,
    tags: Option<String>,
    country: Option<String>,
    bitrate: Option<u32>,
    codec: Option<String>,
    language: Option<String>,
}

impl From<RadioBrowserStation> for RadioStation {
    fn from(value: RadioBrowserStation) -> Self {
        Self {
            id: format!("rb:{}", value.stationuuid),
            name: bounded_text(&value.name, 160).unwrap_or_else(|| "Unnamed station".into()),
            stream_url: value
                .url_resolved
                .filter(|url| !url.trim().is_empty())
                .or_else(|| value.url.filter(|url| !url.trim().is_empty()))
                .unwrap_or_default(),
            source_url: RADIO_BROWSER_SOURCE_URL.into(),
            provider: Some("radio_browser".into()),
            country: bounded_text(value.country.as_deref().unwrap_or_default(), 100),
            tags: bounded_text(value.tags.as_deref().unwrap_or_default(), 300),
            homepage_url: safe_http_url(value.homepage.as_deref().unwrap_or_default()),
            favicon_url: safe_http_url(value.favicon.as_deref().unwrap_or_default()),
            bitrate_kbps: value.bitrate.filter(|n| *n <= 100_000),
            codec: bounded_text(value.codec.as_deref().unwrap_or_default(), 40),
            language: bounded_text(value.language.as_deref().unwrap_or_default(), 200),
            is_custom: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadioTrackMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifiers: Option<crate::recording_identity::RecognitionIdentifiers>,
    pub station_id: String,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub year: Option<String>,
    pub artwork_url: Option<String>,
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    /// This is program metadata supplied by Radio Paradise, not a verified
    /// catalog identity for a recording.
    pub metadata_confidence: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RadioMetadata {
    pub station_id: String,
    pub now_playing: Option<RadioTrackMetadata>,
    pub seconds_remaining: Option<u64>,
    /// The public now-playing endpoint returns a single current item, not a
    /// recent playlist. The view can maintain a short session history itself.
    pub history: Vec<RadioTrackMetadata>,
    pub history_available: bool,
    pub note: String,
}

#[derive(Debug, Deserialize)]
struct RadioParadiseResponse {
    artist: Option<String>,
    title: Option<String>,
    album: Option<String>,
    year: Option<String>,
    cover: Option<String>,
    cover_med: Option<String>,
    cover_small: Option<String>,
    time: Option<i64>,
}

fn radio_paradise_station(id: &str) -> Option<(u8, &'static str, &'static str)> {
    match id {
        "main" => Some((0, "Main Mix", "https://stream.radioparadise.com/flac")),
        "mellow" => Some((
            1,
            "Mellow Mix",
            "https://stream.radioparadise.com/mellow-flac",
        )),
        "rock" => Some((2, "Rock Mix", "https://stream.radioparadise.com/rock-flac")),
        // Radio Paradise calls this channel “Global” in the current player;
        // its stream slug remains the historic “eclectic”.
        "global" => Some((
            3,
            "Global Mix",
            "https://stream.radioparadise.com/eclectic-flac",
        )),
        _ => None,
    }
}

pub fn get_radio_stations() -> Vec<RadioStation> {
    let mut stations = ["main", "mellow", "rock", "global"]
        .into_iter()
        .filter_map(|id| {
            let (_, name, stream_url) = radio_paradise_station(id)?;
            Some(RadioStation {
                id: id.into(),
                name: format!("Radio Paradise · {name}"),
                stream_url: stream_url.into(),
                source_url: RADIO_PARADISE_SOURCE_URL.into(),
                provider: Some("radio_paradise".into()),
                country: None,
                tags: None,
                homepage_url: Some("https://radioparadise.com/".into()),
                favicon_url: None,
                bitrate_kbps: None,
                codec: Some("FLAC".into()),
                language: None,
                is_custom: false,
            })
        })
        .collect::<Vec<_>>();
    stations.push(RadioStation {
        id: "the-north".into(),
        name: "The North 103.3".into(),
        stream_url: "https://thenorth1033.streamguys1.com/live".into(),
        source_url: "https://www.thenorth1033.org/listen-live".into(),
        provider: Some("internet_radio".into()),
        country: Some("United States".into()),
        tags: Some("community radio, alternative, adult contemporary".into()),
        homepage_url: Some("https://www.thenorth1033.org/".into()),
        favicon_url: None,
        bitrate_kbps: None,
        codec: None,
        language: Some("English".into()),
        is_custom: false,
    });
    stations
}

/// List the built-in Radio Paradise stations followed by the user's persistent stations.
pub fn get_radio_stations_saved(store: &Store) -> Result<Vec<RadioStation>, String> {
    let saved = load_saved_radio_stations(store)?;
    let hidden = load_hidden_built_in_radio_station_ids(store)?;
    let mut stations = get_radio_stations()
        .into_iter()
        .filter(|station| !hidden.contains(&station.id))
        .collect::<Vec<_>>();
    stations.extend(saved);
    apply_radio_station_name_overrides(store, &mut stations)?;
    apply_radio_station_url_overrides(store, &mut stations)?;
    Ok(stations)
}

const RADIO_STATION_URL_OVERRIDES_KEY: &str = "radioStationStreamUrls.v1";

fn load_radio_station_url_overrides(
    store: &Store,
) -> Result<std::collections::HashMap<String, String>, String> {
    let Some(value) = store.get_json_preference(RADIO_STATION_URL_OVERRIDES_KEY)? else {
        return Ok(std::collections::HashMap::new());
    };
    serde_json::from_value(value)
        .map_err(|error| format!("Saved radio station URLs could not be read: {error}"))
}

fn apply_radio_station_url_overrides(
    store: &Store,
    stations: &mut [RadioStation],
) -> Result<(), String> {
    let overrides = load_radio_station_url_overrides(store)?;
    for station in stations {
        if let Some(url) = overrides.get(&station.id) {
            station.stream_url.clone_from(url);
        }
    }
    Ok(())
}

/// Change a station's stream while retaining its stable ID and station metadata.
pub async fn edit_radio_station_url(
    station_id: String,
    url: String,
    store: &Store,
) -> Result<Vec<RadioStation>, String> {
    if !get_radio_stations_saved(store)?
        .iter()
        .any(|station| station.id == station_id)
    {
        return Err("Radio station was not found".into());
    }
    let input_url = validate_stream_url(&url)?;
    let stream_url = resolve_playlist_url(input_url.as_str())
        .await?
        .unwrap_or(input_url)
        .to_string();
    // A station may have been removed while a playlist URL was resolving.
    if !get_radio_stations_saved(store)?
        .iter()
        .any(|station| station.id == station_id)
    {
        return Err("Radio station was not found".into());
    }
    let mut saved = load_saved_radio_stations(store)?;
    if let Some(station) = saved.iter_mut().find(|station| station.id == station_id) {
        station.stream_url = stream_url;
        save_radio_stations(store, &saved)?;
    } else {
        let mut overrides = load_radio_station_url_overrides(store)?;
        overrides.insert(station_id, stream_url);
        store.set_json_preference(RADIO_STATION_URL_OVERRIDES_KEY, &json!(overrides))?;
    }
    get_radio_stations_saved(store)
}

fn load_hidden_built_in_radio_station_ids(store: &Store) -> Result<HashSet<String>, String> {
    let Some(value) = store.get_json_preference(HIDDEN_BUILT_IN_RADIO_STATIONS_KEY)? else {
        return Ok(HashSet::new());
    };
    let ids: Vec<String> = serde_json::from_value(value)
        .map_err(|error| format!("Hidden radio stations could not be read: {error}"))?;
    Ok(ids.into_iter().collect())
}

fn load_saved_radio_stations(store: &Store) -> Result<Vec<RadioStation>, String> {
    let Some(value) = store.get_json_preference(SAVED_RADIO_STATIONS_KEY)? else {
        return Ok(Vec::new());
    };
    let stations: Vec<RadioStation> = serde_json::from_value(value)
        .map_err(|error| format!("Saved radio stations could not be read: {error}"))?;
    Ok(stations.into_iter().take(MAX_STATIONS).collect())
}

fn save_radio_stations(store: &Store, stations: &[RadioStation]) -> Result<(), String> {
    if stations.len() > MAX_STATIONS {
        return Err(format!(
            "A maximum of {MAX_STATIONS} radio stations can be saved"
        ));
    }
    let value = serde_json::to_value(stations).map_err(|error| error.to_string())?;
    store.set_json_preference(SAVED_RADIO_STATIONS_KEY, &value)
}

/// Persist a station selected from Radio Browser. Its stable UUID is its identity.
pub fn add_radio_station(
    station: RadioStation,
    store: &Store,
) -> Result<Vec<RadioStation>, String> {
    if !station.id.starts_with("rb:") || station.provider.as_deref() != Some("radio_browser") {
        return Err("Only Radio Browser search results can be saved with this action".into());
    }
    let station = validate_saved_station(station, false)?;
    let mut saved = load_saved_radio_stations(store)?;
    if !saved.iter().any(|item| item.id == station.id) {
        saved.push(station);
        save_radio_stations(store, &saved)?;
    }
    get_radio_stations_saved(store)
}

/// Add a station from a direct HTTP(S) URL, resolving common playlist formats.
pub async fn add_custom_radio_station(
    name: String,
    url: String,
    store: &Store,
) -> Result<Vec<RadioStation>, String> {
    let name = bounded_text(&name, 160).ok_or("Station name must contain 1–160 characters")?;
    let input_url = validate_stream_url(&url)?;
    let stream_url = resolve_playlist_url(input_url.as_str())
        .await?
        .unwrap_or(input_url);
    let id = format!("custom:{}", stable_url_hash(stream_url.as_str()));
    let station = RadioStation {
        id,
        name,
        stream_url: stream_url.to_string(),
        source_url: stream_url.to_string(),
        provider: Some("custom".into()),
        country: None,
        tags: None,
        homepage_url: None,
        favicon_url: None,
        bitrate_kbps: None,
        codec: None,
        language: None,
        is_custom: true,
    };
    let station = validate_saved_station(station, true)?;
    let mut saved = load_saved_radio_stations(store)?;
    if !saved.iter().any(|item| item.id == station.id) {
        if saved.len() >= MAX_STATIONS {
            return Err(format!(
                "A maximum of {MAX_STATIONS} radio stations can be saved"
            ));
        }
        saved.push(station);
        save_radio_stations(store, &saved)?;
    }
    get_radio_stations_saved(store)
}

/// Remove a saved station or persistently hide a built-in station.
pub fn remove_radio_station(
    station_id: String,
    store: &Store,
) -> Result<Vec<RadioStation>, String> {
    if get_radio_stations()
        .iter()
        .any(|station| station.id == station_id)
    {
        let mut hidden = load_hidden_built_in_radio_station_ids(store)?;
        if hidden.insert(station_id) {
            store
                .set_json_preference(HIDDEN_BUILT_IN_RADIO_STATIONS_KEY, &json!(hidden))
                .map_err(|error| error.to_string())?;
        }
        return get_radio_stations_saved(store);
    }
    let mut saved = load_saved_radio_stations(store)?;
    let original_len = saved.len();
    saved.retain(|station| station.id != station_id);
    if saved.len() == original_len {
        return Err("Radio station was not found".into());
    }
    save_radio_stations(store, &saved)?;
    get_radio_stations_saved(store)
}

/// Rename any listed station without changing its stable ID or stream identity.
/// Built-ins store only a name override; persistent user/directory stations use
/// the same override map so their original metadata remains intact.
pub fn rename_radio_station(
    station_id: String,
    name: String,
    store: &Store,
) -> Result<Vec<RadioStation>, String> {
    let name = bounded_text(&name, 160).ok_or("Station name must contain 1–160 characters")?;
    let exists = get_radio_stations_saved(store)?
        .iter()
        .any(|station| station.id == station_id);
    if !exists {
        return Err("Radio station was not found".into());
    }
    let mut overrides = load_radio_station_name_overrides(store)?;
    overrides.insert(station_id, name);
    store
        .set_json_preference(RADIO_STATION_NAME_OVERRIDES_KEY, &json!(overrides))
        .map_err(|error| error.to_string())?;
    get_radio_stations_saved(store)
}

const RADIO_STATION_NAME_OVERRIDES_KEY: &str = "radioStationNames.v1";

fn load_radio_station_name_overrides(
    store: &Store,
) -> Result<std::collections::HashMap<String, String>, String> {
    let Some(value) = store.get_json_preference(RADIO_STATION_NAME_OVERRIDES_KEY)? else {
        return Ok(std::collections::HashMap::new());
    };
    serde_json::from_value(value)
        .map_err(|error| format!("Saved radio station names could not be read: {error}"))
}

fn apply_radio_station_name_overrides(
    store: &Store,
    stations: &mut [RadioStation],
) -> Result<(), String> {
    let overrides = load_radio_station_name_overrides(store)?;
    for station in stations {
        if let Some(name) = overrides.get(&station.id) {
            station.name.clone_from(name);
        }
    }
    Ok(())
}

/// Search the shared Radio Browser station directory. Results are bounded and
/// broken stations are excluded per the public API's recommended query options.
pub async fn search_radio_stations(
    query: String,
    country: Option<String>,
    tag: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<RadioStation>, String> {
    let query = query.trim();
    if query.chars().count() > 100 || query.chars().any(char::is_control) {
        return Err("Search query must contain at most 100 characters".into());
    }
    let query = (!query.is_empty()).then_some(query);
    let country = normalize_radio_filter(country, "Country", 80)?;
    let tag = normalize_radio_filter(tag, "Tag", 80)?;
    if country.is_none() && tag.is_none() && query.map_or(true, |value| value.chars().count() < 2) {
        return Err("Enter at least two search characters, or filter by country or tag".into());
    }
    let limit = limit.unwrap_or(20).clamp(1, 50);
    let mut query_variants = query
        .map(radio_query_variants)
        .unwrap_or_else(|| vec![String::new()]);
    while query_variants.len() < 3 {
        query_variants.push(query_variants.last().cloned().unwrap_or_default());
    }
    let request_count = query_variants.len().min(3);
    let client = radio_http_client(Duration::from_secs(9))?;
    let mut last_error = None;
    let mut succeeded = false;
    let mut stations = Vec::<(i32, usize, RadioStation)>::new();
    let mut station_indexes = std::collections::HashMap::<String, usize>::new();
    let mut order = 0usize;
    let mut working_mirror = None;
    for attempt in 0..request_count {
        // Mirrors can be briefly out of sync. Once one mirror returns valid
        // JSON, run every candidate query against that same index rather than
        // mixing result sets from replicas with different availability.
        let mirror_index = radio_mirror_for_attempt(attempt, working_mirror);
        let server = RADIO_BROWSER_SERVERS[mirror_index];
        let mut request = client
            .get(format!("{server}/json/stations/search"))
            .query(&[
                ("hidebroken", "true"),
                ("limit", "50"),
                ("order", "votes"),
                ("reverse", "true"),
            ]);
        if !query_variants[attempt].is_empty() {
            request = request.query(&[("name", query_variants[attempt].as_str())]);
        }
        if let Some(value) = country.as_deref() {
            request = request.query(&[("country", value)]);
        }
        if let Some(value) = tag.as_deref() {
            request = request.query(&[("tag", value)]);
        }
        match request.send().await {
            Ok(response) => match read_limited_json::<Vec<RadioBrowserStation>>(
                response,
                MAX_RADIO_METADATA_BYTES * 16,
            )
            .await
            {
                Ok(entries) => {
                    succeeded = true;
                    working_mirror = Some(mirror_index);
                    for entry in entries.into_iter().take(RADIO_NAME_CANDIDATES) {
                        let station: RadioStation = entry.into();
                        if !station.id.starts_with("rb:")
                            || !is_uuid(&station.id[3..])
                            || validate_stream_url(&station.stream_url).is_err()
                        {
                            continue;
                        }
                        let score = radio_station_match_score(query.unwrap_or_default(), &station);
                        if score < 0 {
                            continue;
                        }
                        if let Some(index) = station_indexes.get(&station.id).copied() {
                            if score > stations[index].0 {
                                stations[index].0 = score;
                            }
                        } else {
                            station_indexes.insert(station.id.clone(), stations.len());
                            stations.push((score, order, station));
                            order += 1;
                        }
                    }
                }
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(format!("Radio Browser search failed: {error}")),
        }
    }
    if !succeeded {
        return Err(last_error.unwrap_or_else(|| "Radio Browser servers are unavailable".into()));
    }
    stations.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    Ok(stations
        .into_iter()
        .take(limit)
        .map(|(_, _, station)| station)
        .collect())
}

fn radio_mirror_for_attempt(attempt: usize, working_mirror: Option<usize>) -> usize {
    working_mirror.unwrap_or(attempt % RADIO_BROWSER_SERVERS.len())
}

fn radio_query_variants(query: &str) -> Vec<String> {
    let tokens = normalized_radio_tokens(query);
    if tokens.is_empty() {
        return vec![query.to_owned()];
    }
    if tokens.iter().any(|token| token == "mpr") {
        let remainder = tokens
            .iter()
            .filter(|token| *token != "mpr")
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let variants = if remainder.is_empty() {
            vec![
                "Minnesota Public Radio".to_string(),
                "The Current".to_string(),
                "YourClassical MPR".to_string(),
            ]
        } else if remainder.split_whitespace().any(|word| word == "current") {
            vec![
                "The Current".to_string(),
                "Minnesota Public Radio".to_string(),
                "YourClassical MPR".to_string(),
            ]
        } else if remainder.split_whitespace().any(|word| word == "classical") {
            vec![
                "YourClassical MPR".to_string(),
                "Minnesota Public Radio".to_string(),
                "The Current".to_string(),
            ]
        } else {
            vec![
                "Minnesota Public Radio".to_string(),
                "The Current".to_string(),
                "YourClassical MPR".to_string(),
            ]
        };
        return variants
            .into_iter()
            .take(3)
            .map(|variant| {
                if remainder.is_empty() {
                    variant.to_owned()
                } else {
                    format!("{variant} {remainder}")
                }
            })
            .collect();
    }
    let mut variants = vec![query.trim().to_owned()];
    // Radio Browser's name filter can favor a literal multi-word phrase. Query
    // each distinct word as well so reordered names such as “France Inter” are
    // still available to the local all-token ranker.
    let mut words = tokens;
    words.sort_by_key(|token| std::cmp::Reverse(token.chars().count()));
    for word in words {
        if variants.len() >= 3 {
            break;
        }
        let word = original_query_word(query, &word);
        if !variants
            .iter()
            .any(|value| fold_radio_text(value) == fold_radio_text(&word))
        {
            variants.push(word);
        }
    }
    variants.truncate(3);
    variants
}

fn original_query_word(query: &str, normalized: &str) -> String {
    query
        .split_whitespace()
        .find(|word| fold_radio_text(word) == normalized)
        .unwrap_or(normalized)
        .to_owned()
}

fn radio_station_match_score(query: &str, station: &RadioStation) -> i32 {
    let candidate = fold_radio_text(&station.name);
    let tokens = normalized_radio_tokens(query);
    let mpr_query = tokens.iter().any(|token| token == "mpr");
    if mpr_query {
        let aliases = [
            "minnesota public radio",
            "mpr news",
            "the current",
            "yourclassical mpr",
            "radio heartland",
            "carbon sound",
        ];
        let remainder = tokens
            .iter()
            .filter(|token| *token != "mpr")
            .cloned()
            .collect::<Vec<_>>();
        let matching = aliases
            .iter()
            .filter(|alias| {
                let alias_tokens = normalized_radio_tokens(alias);
                alias_tokens.iter().all(|token| {
                    candidate
                        .split_whitespace()
                        .any(|part| part == token.as_str())
                }) && remainder.iter().all(|token| {
                    candidate
                        .split_whitespace()
                        .any(|part| part == token.as_str())
                })
            })
            .count();
        return if matching > 0 {
            2000 + matching as i32
        } else {
            -1000
        };
    }
    let matching = tokens
        .iter()
        .filter(|token| {
            candidate
                .split_whitespace()
                .any(|part| part == token.as_str())
        })
        .count();
    let all_match = !tokens.is_empty() && matching == tokens.len();
    let folded_query = fold_radio_text(query);
    let mut canonical_tokens = tokens.clone();
    canonical_tokens.sort();
    let canonical_phrase = canonical_tokens.join(" ");
    (if all_match { 1000 } else { 0 })
        + matching as i32 * 10
        + if candidate.contains(folded_query.as_str()) {
            100
        } else {
            0
        }
        + if candidate == canonical_phrase {
            300
        } else if candidate.contains(canonical_phrase.as_str()) {
            150
        } else {
            0
        }
}

fn normalized_radio_tokens(value: &str) -> Vec<String> {
    fold_radio_text(value)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn fold_radio_text(value: &str) -> String {
    let mut folded = String::new();
    for character in value.chars() {
        let mapped = match character.to_lowercase().next().unwrap_or(character) {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'ç' | 'ć' | 'č' => "c",
            'ď' | 'đ' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => "i",
            'ł' => "l",
            'ñ' | 'ń' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
            'ř' => "r",
            'ś' | 'š' => "s",
            'ť' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
            'ý' | 'ÿ' => "y",
            'ž' | 'ź' | 'ż' => "z",
            'æ' => "ae",
            'œ' => "oe",
            'ß' => "ss",
            c if c.is_alphanumeric() => {
                folded.push(c);
                continue;
            }
            _ => " ",
        };
        folded.push_str(mapped);
    }
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_radio_filter(
    value: Option<String>,
    label: &str,
    max_chars: usize,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} filter must contain 1–{max_chars} characters"
        ));
    }
    Ok(Some(value.to_owned()))
}

fn validate_saved_station(mut station: RadioStation, custom: bool) -> Result<RadioStation, String> {
    station.name =
        bounded_text(&station.name, 160).ok_or("Station name must contain 1–160 characters")?;
    station.stream_url = validate_stream_url(&station.stream_url)?.to_string();
    station.source_url =
        safe_http_url(&station.source_url).unwrap_or_else(|| station.stream_url.clone());
    if custom {
        if !station.id.starts_with("custom:") {
            return Err("Invalid custom station identity".into());
        }
        station.is_custom = true;
        station.provider = Some("custom".into());
    } else if !station.id.starts_with("rb:") || !is_uuid(&station.id[3..]) {
        return Err("Radio Browser station UUID is invalid".into());
    }
    station.homepage_url = station.homepage_url.as_deref().and_then(safe_http_url);
    station.favicon_url = station.favicon_url.as_deref().and_then(safe_http_url);
    station.tags = station.tags.as_deref().and_then(|v| bounded_text(v, 300));
    station.country = station
        .country
        .as_deref()
        .and_then(|v| bounded_text(v, 100));
    station.codec = station.codec.as_deref().and_then(|v| bounded_text(v, 40));
    station.language = station
        .language
        .as_deref()
        .and_then(|v| bounded_text(v, 200));
    Ok(station)
}

fn bounded_text(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn safe_http_url(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    (matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none())
    .then(|| parsed.to_string())
}

fn validate_stream_url(value: &str) -> Result<url::Url, String> {
    let url =
        url::Url::parse(value.trim()).map_err(|_| "Stream URL must be a valid HTTP(S) URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Stream URL must use HTTP or HTTPS without embedded credentials".into());
    }
    Ok(url)
}

/// Header-verified transport for the exact saved station (including its redirects).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedStationPlayback {
    pub url: String,
    pub mime_type: Option<String>,
    pub codec: Option<String>,
}

pub async fn resolve_station_playback_url(
    station_id: String,
    store: &Store,
) -> Result<String, String> {
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    resolve_station_playback_until(
        station_id,
        store,
        tokio::time::Instant::now() + Duration::from_secs(8),
        cancel,
    )
    .await
    .map(|resolved| resolved.url)
}

pub async fn resolve_station_playback_until(
    station_id: String,
    store: &Store,
    deadline: tokio::time::Instant,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<ResolvedStationPlayback, String> {
    // An already-expired Tokio timer can require a scheduler turn before it is
    // observed. Do not start DNS/probing or report a source failure in that gap.
    if *cancel.borrow() {
        return Err("Radio playback was cancelled".into());
    }
    if tokio::time::Instant::now() >= deadline {
        return Err("Radio stream lookup exceeded playback deadline".into());
    }
    let station = get_radio_stations_saved(store)?
        .into_iter()
        .find(|station| station.id == station_id)
        .ok_or("Radio station no longer exists")?;
    let original = validate_stream_url(&station.stream_url)?;
    let work = async {
        // Never reuse a stale signed redirect target: DNS, the redirect chain
        // and MIME are revalidated for each startup.
        let mut candidates = Vec::new();
        if let Some(secure) = secure_stream_candidate(&original) {
            candidates.push(secure);
        }
        candidates.push(original.clone());
        let mut last_error = "Radio stream is unavailable".to_owned();
        for candidate in candidates {
            if tokio::time::Instant::now() >= deadline {
                return Err("Radio stream lookup exceeded playback deadline".into());
            }
            let attempt_deadline =
                deadline.min(tokio::time::Instant::now() + Duration::from_secs(4));
            match tokio::time::timeout_at(
                attempt_deadline,
                probe_station_transport(candidate, None),
            )
            .await
            {
                Ok(Ok(mut resolved)) => {
                    if resolved.codec.is_none() {
                        resolved.codec = station.codec.clone();
                    }
                    return Ok(resolved);
                }
                Ok(Err(error)) => last_error = error,
                Err(_) => last_error = "Radio stream header lookup timed out".into(),
            }
        }
        Err(last_error)
    };
    tokio::select! {
        biased;
        _ = async { loop { if *cancel.borrow() { break; } if cancel.changed().await.is_err() { std::future::pending::<()>().await; } } } => Err("Radio playback was cancelled".into()),
        _ = tokio::time::sleep_until(deadline) => Err("Radio stream lookup exceeded playback deadline".into()),
        result = work => result,
    }
}

async fn probe_station_transport(
    mut current: url::Url,
    _test_addresses: Option<&[std::net::SocketAddr]>,
) -> Result<ResolvedStationPlayback, String> {
    let mut seen = HashSet::new();
    for hop in 0..=5 {
        current = validate_stream_url(current.as_str())?;
        current.set_fragment(None);
        if !seen.insert(current.to_string()) {
            return Err("Radio stream redirect loop".into());
        }
        let mut addresses = match _test_addresses {
            #[cfg(test)]
            Some(addresses) if current.host_str() == Some("radio-fixture.test") => {
                addresses.to_vec()
            }
            _ => crate::acquisition::public_addresses(&current).await?,
        };
        let port = current
            .port_or_known_default()
            .ok_or("Radio stream has no port")?;
        for address in &mut addresses {
            address.set_port(port);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .resolve_to_addrs(
                current.host_str().ok_or("Radio stream has no host")?,
                &addresses,
            )
            .build()
            .map_err(|e| format!("Could not prepare radio transport: {e}"))?;
        let response = client
            .get(current.clone())
            .header("Icy-MetaData", "0")
            .send()
            .await
            .map_err(|e| format!("Radio stream header request failed: {e}"))?;
        if response.status().is_redirection() {
            if hop == 5 {
                return Err("Radio stream exceeded redirect limit".into());
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|h| h.to_str().ok())
                .ok_or("Radio redirect has no valid destination")?;
            current = current
                .join(location)
                .map_err(|_| "Radio redirect destination is invalid")?;
            // Each request is new: cookies, authorization and upstream response headers
            // are never forwarded across redirects, even on the same host.
            continue;
        }
        if !response.status().is_success() {
            return Err(format!(
                "Radio stream returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !radio_audio_content_type(&mime) {
            return Err("Radio stream did not return an audio or HLS content type".into());
        }
        let codec = match mime.as_str() {
            "audio/mpeg" | "audio/mp3" => Some("MP3"),
            "audio/aac" | "audio/aacp" => Some("AAC"),
            "audio/flac" | "audio/x-flac" => Some("FLAC"),
            "audio/ogg" | "application/ogg" => Some("OGG"),
            "audio/opus" => Some("OPUS"),
            _ => None,
        }
        .map(str::to_owned);
        return Ok(ResolvedStationPlayback {
            url: current.to_string(),
            mime_type: Some(mime),
            codec,
        });
    }
    Err("Radio stream exceeded redirect limit".into())
}
fn secure_stream_candidate(original: &url::Url) -> Option<url::Url> {
    if original.scheme() != "http" || original.port().is_some() {
        return None;
    }
    let mut candidate = original.clone();
    candidate.set_scheme("https").ok()?;
    Some(candidate)
}
fn radio_audio_content_type(value: &str) -> bool {
    let mime = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("audio/")
        || matches!(
            mime.as_str(),
            "application/ogg" | "application/vnd.apple.mpegurl" | "application/x-mpegurl"
        )
}

fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

fn stable_url_hash(value: &str) -> String {
    // FNV-1a is intentionally used instead of DefaultHasher so station IDs
    // remain stable across process restarts and Rust toolchain versions.
    let hash = value.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

fn radio_http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!(
            "Burrata/",
            env!("CARGO_PKG_VERSION"),
            " (internet radio directory)"
        ))
        .build()
        .map_err(|error| format!("Could not prepare radio request: {error}"))
}

async fn read_limited_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    max: usize,
) -> Result<T, String> {
    let response = response
        .error_for_status()
        .map_err(|error| format!("Radio service response failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err("Radio service response was too large".into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Could not read radio service response: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > max {
            return Err("Radio service response was too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("Radio service returned invalid data: {error}"))
}

async fn resolve_playlist_url(url: &str) -> Result<Option<url::Url>, String> {
    let parsed = validate_stream_url(url)?;
    let extension = parsed
        .path()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "m3u" | "pls") {
        return Ok(None);
    }
    // Avoid turning playlist resolution into a local-network probing endpoint.
    if let Some(host) = parsed.host_str() {
        if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
            return Err("Playlist URLs on local hosts cannot be resolved".into());
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if match ip {
                IpAddr::V4(v4) => {
                    v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
                }
                IpAddr::V6(v6) => {
                    v6.is_loopback()
                        || v6.is_unique_local()
                        || v6.is_unicast_link_local()
                        || v6.is_unspecified()
                }
            } {
                return Err("Playlist URLs on private hosts cannot be resolved".into());
            }
        }
    }
    let response = radio_http_client(Duration::from_secs(6))?
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("Could not load station playlist: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Station playlist request failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_PLAYLIST_BYTES as u64)
    {
        return Err("Station playlist is too large".into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if body.len().saturating_add(chunk.len()) > MAX_PLAYLIST_BYTES {
            return Err("Station playlist is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&body);
    let candidate = playlist_stream_url(&body);
    match candidate {
        Some(url) => Ok(Some(validate_stream_url(url)?)),
        None => Err("Playlist did not contain a direct HTTP(S) stream URL".into()),
    }
}

fn playlist_stream_url(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find_map(|line| {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            return None;
        }
        let line = if line.to_ascii_lowercase().starts_with("file") {
            line.split_once('=')?.1.trim()
        } else {
            line
        };
        (line.starts_with("http://") || line.starts_with("https://")).then_some(line)
    })
}

/// Fetches current Radio Paradise metadata for one of the four supported
/// channels. Station IDs are allowlisted before any request is made, redirects
/// are disabled, response size is capped, and the host is fixed in code.
pub async fn get_radio_metadata(station_id: String) -> Result<RadioMetadata, String> {
    let Some((channel, _, _)) = radio_paradise_station(&station_id) else {
        if station_id.starts_with("rb:") || station_id.starts_with("custom:") {
            return Ok(RadioMetadata {
                station_id,
                now_playing: None,
                seconds_remaining: None,
                history: Vec::new(),
                history_available: false,
                note: "This station does not provide supported now-playing metadata. Stream audio remains available.".into(),
            });
        }
        return Err("Unknown radio station".into());
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Burrata/1.0 (Radio Paradise now-playing metadata)")
        .build()
        .map_err(|error| format!("Could not prepare Radio Paradise metadata request: {error}"))?;
    let mut response = client
        .get(RADIO_PARADISE_API_URL)
        .query(&[("chan", channel)])
        .send()
        .await
        .map_err(|error| format!("Radio Paradise metadata request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Radio Paradise metadata response failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RADIO_METADATA_BYTES as u64)
    {
        return Err("Radio Paradise metadata response was too large".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Could not read Radio Paradise metadata: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RADIO_METADATA_BYTES {
            return Err("Radio Paradise metadata response was too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let data: RadioParadiseResponse = serde_json::from_slice(&body)
        .map_err(|error| format!("Radio Paradise returned invalid metadata: {error}"))?;
    let artist = clean_radio_text(data.artist);
    let title = clean_radio_text(data.title);
    let now_playing = match (artist, title) {
        (Some(artist), Some(title)) => Some(RadioTrackMetadata {
            identifiers: None,
            station_id: station_id.clone(),
            artist,
            title,
            album: clean_radio_text(data.album),
            year: data
                .year
                .filter(|year| year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit())),
            artwork_url: safe_radio_artwork(data.cover)
                .or_else(|| safe_radio_artwork(data.cover_med))
                .or_else(|| safe_radio_artwork(data.cover_small)),
            source_url: RADIO_PARADISE_SOURCE_URL.into(),
            observed_at: None,
            metadata_confidence: "station-reported; catalog identity unverified".into(),
        }),
        _ => None,
    };
    Ok(RadioMetadata {
        station_id,
        now_playing,
        seconds_remaining: data.time.filter(|value| (0..=24 * 60 * 60).contains(value)).map(|value| value as u64),
        history: Vec::new(),
        history_available: false,
        note: "Radio Paradise provides current program metadata here; no recent-play history was returned. Artist and title are not verified catalog identities.".into(),
    })
}

fn clean_radio_text(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_owned();
    (!value.is_empty() && value.chars().count() <= 300).then_some(value)
}

fn safe_radio_artwork(value: Option<String>) -> Option<String> {
    let value = value?;
    let parsed = url::Url::parse(&value).ok()?;
    (parsed.scheme() == "https"
        && parsed.host_str() == Some("img.radioparadise.com")
        && parsed.username().is_empty()
        && parsed.password().is_none())
    .then_some(value)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongRadioResult {
    pub seed_track_id: String,
    pub seed_video_id: String,
    pub source: String,
    pub tracks: Vec<Track>,
    pub total: usize,
}

/// Build a bounded, ordered recommendation queue from YouTube's actual
/// per-video autoplay Mix. This function only reads playlist metadata; it never
/// downloads or queues audio automatically.
pub async fn start_song_radio(
    seed_track: &Track,
    seed_candidate: &Candidate,
    settings: &Settings,
    limit: usize,
) -> Result<SongRadioResult, String> {
    within_song_radio_deadline(
        SONG_RADIO_DEADLINE,
        start_song_radio_inner(seed_track, seed_candidate, settings, limit),
    )
    .await
}

async fn start_song_radio_inner(
    seed_track: &Track,
    seed_candidate: &Candidate,
    settings: &Settings,
    limit: usize,
) -> Result<SongRadioResult, String> {
    if seed_candidate.provider != "youtube" {
        return Err("Song radio currently requires a YouTube seed candidate".into());
    }
    if !(1..=50).contains(&limit) {
        return Err("Song radio limit must be between 1 and 50".into());
    }
    let seed_video_id = youtube_video_id(&seed_candidate.url)
        .ok_or("Seed candidate is not a valid YouTube video URL")?;
    let entries = fetch_youtube_mix_entries(&seed_candidate.url, settings, limit).await?;
    let tracks = parse_mix_entries(
        &seed_track.id,
        &seed_track.title,
        &seed_video_id,
        &entries,
        limit,
    );
    if tracks.is_empty() {
        return Err("YouTube autoplay Mix returned no other playable videos".into());
    }
    Ok(SongRadioResult {
        seed_track_id: seed_track.id.clone(),
        seed_video_id,
        source: "youtube_autoplay_mix".into(),
        total: tracks.len(),
        tracks,
    })
}

fn parse_mix_entries(
    seed_track_id: &str,
    seed_title: &str,
    seed_video_id: &str,
    entries: &[Value],
    limit: usize,
) -> Vec<Track> {
    let mut seen = HashSet::new();
    seen.insert(seed_video_id.to_owned());
    let mut tracks = Vec::new();
    for entry in entries {
        if tracks.len() >= limit {
            break;
        }
        let Some(id) = entry["id"].as_str().filter(|id| valid_video_id(id)) else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let title = entry["title"].as_str().unwrap_or_default().trim();
        if title.is_empty() {
            continue;
        }
        let duration = entry["duration"].as_f64().filter(|seconds| {
            seconds.is_finite() && *seconds > 0.0 && *seconds <= 24.0 * 60.0 * 60.0
        });
        let artwork_url = entry["thumbnail"]
            .as_str()
            .filter(|url| {
                url::Url::parse(url).is_ok_and(|parsed| {
                    parsed.scheme() == "https"
                        && parsed.username().is_empty()
                        && parsed.password().is_none()
                })
            })
            .map(str::to_owned);
        let uploader = entry["uploader"].as_str().map(str::to_owned);
        let candidate = Candidate {
            id: format!("youtube:{id}"),
            provider: "youtube".into(),
            url: format!("https://www.youtube.com/watch?v={id}"),
            title: title.to_owned(),
            uploader: uploader.clone(),
            duration_seconds: duration,
            artwork_url: artwork_url.clone(),
            permission: "unknown".into(),
            confidence: "review".into(),
            score: 0.0,
            reasons: vec![
                "Suggested by YouTube autoplay Mix; recording identity is unverified".into(),
                "Download permission has not been established".into(),
            ],
            ..Candidate::default()
        };
        let candidate_json = serde_json::to_value(&candidate).unwrap_or(Value::Null);
        tracks.push(Track {
            id: format!("youtube:{id}"),
            provider: "youtube".into(),
            provider_id: id.to_owned(),
            source_url: candidate.url.clone(),
            artist: String::new(),
            title: title.to_owned(),
            duration_seconds: duration,
            artwork_url,
            provenance: json!({
                "sourceKind":"youtubeRadio",
                "identity":"unverified YouTube autoplay recommendation",
                "recommendedBy":"YouTube autoplay Mix",
                "seedTrackId":seed_track_id,
                "seedTitle":seed_title,
                "seedVideoId":seed_video_id,
                "uploader":uploader,
                "audioCandidate":candidate_json
            }),
            ..Track::default()
        });
    }
    tracks
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn song_radio_deadline_drops_the_recommendation_future() {
        use std::future::pending;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let flag = DropFlag(Arc::clone(&dropped));
        let result = within_song_radio_deadline(Duration::from_millis(5), async move {
            let _flag = flag;
            pending::<Result<(), String>>().await
        })
        .await;

        assert_eq!(
            result.unwrap_err(),
            "Song radio recommendations exceeded the 20-second deadline"
        );
        assert!(dropped.load(Ordering::SeqCst));
    }

    fn test_store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("radio.sqlite")).unwrap();
        (directory, store)
    }

    #[test]
    fn youtube_video_urls_require_a_supported_host_and_video_id() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=JGwWNGJdvx8"),
            Some("JGwWNGJdvx8".into())
        );
        assert_eq!(
            youtube_video_id("https://youtu.be/JGwWNGJdvx8"),
            Some("JGwWNGJdvx8".into())
        );
        assert!(
            youtube_video_id("https://youtube.com.attacker.test/watch?v=JGwWNGJdvx8").is_none()
        );
        assert!(youtube_video_id("https://youtube.com/watch?v=short").is_none());
    }

    #[test]
    fn mix_parser_preserves_order_dedupes_and_marks_identity_unverified() {
        let entries = vec![
            json!({"id":"seedVideo01","title":"Seed"}),
            json!({"id":"recommend01","title":"First song","uploader":"Channel A","duration":210}),
            json!({"id":"recommend01","title":"Duplicate"}),
            json!({"id":"recommend02","title":"Second song"}),
            json!({"id":"bad id","title":"Malformed"}),
            json!({"id":"recommend03","title":"Third song"}),
        ];
        let tracks = parse_mix_entries("catalog:seed", "Seed title", "seedVideo01", &entries, 2);
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["recommend01", "recommend02"]
        );
        assert_eq!(tracks[0].artist, "");
        assert_eq!(
            tracks[0].provenance["identity"],
            "unverified YouTube autoplay recommendation"
        );
        assert_eq!(
            tracks[0].provenance["audioCandidate"]["permission"],
            "unknown"
        );
        assert_eq!(tracks[0].provenance["uploader"], "Channel A");
    }

    #[test]
    fn radio_paradise_station_ids_are_allowlisted_and_streams_are_fixed() {
        let stations = get_radio_stations()
            .into_iter()
            .filter(|station| station.provider.as_deref() == Some("radio_paradise"))
            .collect::<Vec<_>>();
        assert_eq!(
            stations
                .iter()
                .map(|station| station.id.as_str())
                .collect::<Vec<_>>(),
            ["main", "mellow", "rock", "global"]
        );
        assert_eq!(
            stations
                .iter()
                .map(|station| station.stream_url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://stream.radioparadise.com/flac",
                "https://stream.radioparadise.com/mellow-flac",
                "https://stream.radioparadise.com/rock-flac",
                "https://stream.radioparadise.com/eclectic-flac",
            ]
        );
        assert!(radio_paradise_station("https://attacker.test").is_none());
        assert!(radio_paradise_station("../main").is_none());
        let north = get_radio_stations()
            .into_iter()
            .find(|station| station.id == "the-north")
            .unwrap();
        assert_eq!(
            north.stream_url,
            "https://thenorth1033.streamguys1.com/live"
        );
        assert_eq!(north.source_url, "https://www.thenorth1033.org/listen-live");
    }

    #[test]
    fn radio_station_dto_deserializes_older_saved_station_json() {
        let station: RadioStation = serde_json::from_value(json!({
            "id":"custom:older", "name":"Old station", "streamUrl":"https://radio.example/stream",
            "sourceUrl":"https://radio.example/stream"
        }))
        .unwrap();
        assert_eq!(station.provider, None);
        assert_eq!(station.bitrate_kbps, None);
        assert!(!station.is_custom);
    }

    #[test]
    fn saved_radio_browser_stations_persist_deduplicate_and_remove_by_stable_uuid() {
        let (_directory, store) = test_store();
        let station: RadioStation = serde_json::from_value(json!({
            "id":"rb:550e8400-e29b-41d4-a716-446655440000", "name":"Jazz FM",
            "streamUrl":"https://stream.example/jazz", "sourceUrl":"https://www.radio-browser.info/",
            "provider":"radio_browser", "country":"Canada", "tags":"jazz"
        })).unwrap();
        let first = add_radio_station(station.clone(), &store).unwrap();
        let second = add_radio_station(station, &store).unwrap();
        assert_eq!(first.len(), 6);
        assert_eq!(second.len(), 6);
        let removed =
            remove_radio_station("rb:550e8400-e29b-41d4-a716-446655440000".into(), &store).unwrap();
        assert_eq!(removed.len(), 5);
        assert_eq!(
            remove_radio_station("rb:missing".into(), &store).unwrap_err(),
            "Radio station was not found"
        );
    }

    #[test]
    fn radio_search_matches_tokens_in_any_order_and_ignores_diacritics() {
        let exact = station_for_search("France Inter");
        let unrelated = station_for_search("France Culture");
        let accented = station_for_search("Écoutez Mon petit France Inter");
        assert!(
            radio_station_match_score("inter France", &exact)
                > radio_station_match_score("inter France", &unrelated)
        );
        assert!(radio_station_match_score("INTER france", &accented) >= 1000);
        assert_eq!(
            normalized_radio_tokens("Montréal CAFÉ"),
            vec!["montreal".to_string(), "cafe".to_string()]
        );
    }

    #[test]
    fn radio_search_reuses_the_first_working_mirror_for_all_query_variants() {
        assert_eq!(radio_mirror_for_attempt(0, None), 0);
        assert_eq!(radio_mirror_for_attempt(1, Some(0)), 0);
        assert_eq!(radio_mirror_for_attempt(2, Some(0)), 0);
        // If the initial mirror failed before returning valid JSON, try the
        // next replica once; then keep all remaining queries on that replica.
        assert_eq!(radio_mirror_for_attempt(1, None), 1);
        assert_eq!(radio_mirror_for_attempt(2, Some(1)), 1);
    }

    #[test]
    fn mpr_expands_to_curated_official_service_names_and_ranks_full_aliases() {
        assert_eq!(
            radio_query_variants("MPR"),
            vec!["Minnesota Public Radio", "The Current", "YourClassical MPR"]
        );
        let station = station_for_search("The Current - 89.3 KCMP Minnesota Public Radio");
        assert!(radio_station_match_score("mpr", &station) >= 2000);
    }

    fn station_for_search(name: &str) -> RadioStation {
        serde_json::from_value(json!({
            "id":"rb:550e8400-e29b-41d4-a716-446655440000",
            "name":name,"streamUrl":"https://stream.example/live",
            "sourceUrl":"https://www.radio-browser.info/","provider":"radio_browser"
        }))
        .unwrap()
    }

    #[test]
    fn built_in_and_saved_radio_names_can_be_overridden_without_changing_identity() {
        let (_directory, store) = test_store();
        let renamed = rename_radio_station("main".into(), "Late Night Mix".into(), &store).unwrap();
        let main = renamed.iter().find(|station| station.id == "main").unwrap();
        assert_eq!(main.name, "Late Night Mix");
        assert_eq!(main.stream_url, "https://stream.radioparadise.com/flac");
        let without_main = remove_radio_station("main".into(), &store).unwrap();
        assert!(without_main.iter().all(|station| station.id != "main"));

        let station = station_for_search("Directory Station");
        let with_saved = add_radio_station(station.clone(), &store).unwrap();
        let id = station.id.clone();
        let stream_url = station.stream_url.clone();
        let renamed = rename_radio_station(id.clone(), "Kitchen Radio".into(), &store).unwrap();
        let saved = renamed.iter().find(|station| station.id == id).unwrap();
        assert_eq!(saved.name, "Kitchen Radio");
        assert_eq!(saved.stream_url, stream_url);
        assert_eq!(
            with_saved
                .iter()
                .find(|station| station.id == id)
                .unwrap()
                .stream_url,
            stream_url
        );
    }

    #[tokio::test]
    async fn editing_built_in_stream_keeps_station_id_and_name_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("radio.sqlite");
        let store = Store::open(&path).unwrap();
        rename_radio_station("main".into(), "My Main Mix".into(), &store).unwrap();
        let edited = edit_radio_station_url(
            "main".into(),
            "https://radio.example/custom-live".into(),
            &store,
        )
        .await
        .unwrap();
        let main = edited.iter().find(|station| station.id == "main").unwrap();
        assert_eq!(main.name, "My Main Mix");
        assert_eq!(main.stream_url, "https://radio.example/custom-live");
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let main = get_radio_stations_saved(&reopened)
            .unwrap()
            .into_iter()
            .find(|station| station.id == "main")
            .unwrap();
        assert_eq!(main.name, "My Main Mix");
        assert_eq!(main.stream_url, "https://radio.example/custom-live");
    }

    #[tokio::test]
    async fn editing_saved_custom_and_radio_browser_streams_preserves_ids_on_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("radio.sqlite");
        let store = Store::open(&path).unwrap();
        let custom = add_custom_radio_station(
            "Custom station".into(),
            "https://radio.example/custom".into(),
            &store,
        )
        .await
        .unwrap()
        .into_iter()
        .find(|station| station.name == "Custom station")
        .unwrap();
        let browser = station_for_search("Directory station");
        add_radio_station(browser.clone(), &store).unwrap();
        let edited = edit_radio_station_url(
            custom.id.clone(),
            "https://radio.example/custom-new".into(),
            &store,
        )
        .await
        .unwrap();
        assert_eq!(
            edited
                .iter()
                .find(|item| item.id == custom.id)
                .unwrap()
                .stream_url,
            "https://radio.example/custom-new"
        );
        let edited = edit_radio_station_url(
            browser.id.clone(),
            "https://radio.example/directory-new".into(),
            &store,
        )
        .await
        .unwrap();
        assert_eq!(
            edited
                .iter()
                .find(|item| item.id == browser.id)
                .unwrap()
                .stream_url,
            "https://radio.example/directory-new"
        );
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let stations = get_radio_stations_saved(&reopened).unwrap();
        assert_eq!(
            stations
                .iter()
                .find(|item| item.id == custom.id)
                .unwrap()
                .stream_url,
            "https://radio.example/custom-new"
        );
        assert_eq!(
            stations
                .iter()
                .find(|item| item.id == browser.id)
                .unwrap()
                .stream_url,
            "https://radio.example/directory-new"
        );
    }

    #[tokio::test]
    async fn editing_station_url_rejects_invalid_urls_and_unknown_ids() {
        let (_directory, store) = test_store();
        let before = get_radio_stations_saved(&store).unwrap();
        for url in [
            "file:///tmp/station",
            "https://user:password@radio.example/live",
            "not a URL",
        ] {
            assert!(edit_radio_station_url("main".into(), url.into(), &store)
                .await
                .is_err());
        }
        assert_eq!(get_radio_stations_saved(&store).unwrap(), before);
        assert_eq!(
            edit_radio_station_url(
                "missing".into(),
                "https://radio.example/live".into(),
                &store,
            )
            .await
            .unwrap_err(),
            "Radio station was not found"
        );
    }

    #[test]
    fn removing_a_built_in_station_persists_across_store_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("radio.sqlite");
        let store = Store::open(&path).unwrap();
        assert!(get_radio_stations_saved(&store)
            .unwrap()
            .iter()
            .any(|station| station.id == "the-north"));
        remove_radio_station("the-north".into(), &store).unwrap();

        let reopened = Store::open(&path).unwrap();
        assert!(get_radio_stations()
            .iter()
            .any(|station| station.id == "the-north"));
        assert!(get_radio_stations_saved(&reopened)
            .unwrap()
            .iter()
            .all(|station| station.id != "the-north"));
    }

    #[tokio::test]
    #[ignore = "requires live Radio Browser directory access"]
    async fn live_radio_browser_search_finds_mpr_and_the_north() {
        let mpr = search_radio_stations("MPR".into(), None, None, Some(50))
            .await
            .unwrap();
        assert!(mpr.iter().any(|station| {
            station.name.to_lowercase().contains("the current")
                || station
                    .name
                    .to_lowercase()
                    .contains("minnesota public radio")
        }));
        let north = search_radio_stations("The North 103.3".into(), None, None, Some(10))
            .await
            .unwrap();
        assert!(north.iter().any(
            |station| station.name.eq_ignore_ascii_case("The North 103.3")
                && station.stream_url == "https://thenorth1033.streamguys1.com/live"
        ));
        let france_inter = search_radio_stations("inter France".into(), None, None, Some(20))
            .await
            .unwrap();
        assert_eq!(
            france_inter
                .first()
                .map(|station| fold_radio_text(&station.name)),
            Some("france inter".into())
        );
    }

    #[test]
    fn custom_station_urls_require_safe_http_syntax_and_ids_are_stable() {
        assert!(validate_stream_url("file:///tmp/station").is_err());
        assert!(validate_stream_url("https://user:secret@radio.example/stream").is_err());
        assert!(validate_stream_url("https://radio.example/stream").is_ok());
        assert_eq!(
            stable_url_hash("https://radio.example/stream"),
            stable_url_hash("https://radio.example/stream")
        );
        assert_ne!(
            stable_url_hash("https://radio.example/a"),
            stable_url_hash("https://radio.example/b")
        );
    }

    #[test]
    fn radio_browser_filters_allow_country_or_tag_without_name_and_bound_inputs() {
        assert_eq!(
            normalize_radio_filter(Some("jazz".into()), "Tag", 80).unwrap(),
            Some("jazz".into())
        );
        assert!(normalize_radio_filter(Some("  ".into()), "Tag", 80).is_err());
        assert!(normalize_radio_filter(Some("x".repeat(81)), "Tag", 80).is_err());
    }

    #[test]
    fn playlist_parser_understands_m3u_and_pls_stream_lines() {
        let m3u = "#EXTM3U\n#EXTINF:-1,Station\nhttps://stream.example/live\n";
        let pls =
            "[playlist]\nNumberOfEntries=1\nFile1=https://stream.example/live\nTitle1=Station\n";
        assert_eq!(
            playlist_stream_url(m3u),
            Some("https://stream.example/live")
        );
        assert_eq!(
            playlist_stream_url(pls),
            Some("https://stream.example/live")
        );
    }

    #[test]
    fn radio_paradise_metadata_is_normalized_with_uncertainty_and_safe_artwork() {
        let response: RadioParadiseResponse = serde_json::from_value(json!({
            "artist":"  Artist  ","title":"Song","album":"Album","year":"2024","time":83,
            "cover":"https://img.radioparadise.com/covers/l/song.jpg"
        }))
        .unwrap();
        let artist = clean_radio_text(response.artist).unwrap();
        let title = clean_radio_text(response.title).unwrap();
        assert_eq!(artist, "Artist");
        assert_eq!(title, "Song");
        assert_eq!(
            safe_radio_artwork(response.cover),
            Some("https://img.radioparadise.com/covers/l/song.jpg".into())
        );
        assert!(safe_radio_artwork(Some("https://example.test/covers/song.jpg".into())).is_none());
        assert!(clean_radio_text(Some("x".repeat(301))).is_none());
        assert!(clean_radio_text(Some("  ".into())).is_none());
    }

    #[tokio::test]
    #[ignore = "requires public Radio Paradise API access"]
    async fn live_radio_paradise_main_metadata_has_artist_title_and_cover() {
        let metadata = get_radio_metadata("main".into()).await.unwrap();
        let track = metadata
            .now_playing
            .expect("Station returned no current track");
        assert!(!track.artist.is_empty());
        assert!(!track.title.is_empty());
        assert!(track.artwork_url.is_some());
        assert!(!metadata.history_available);
    }
    #[tokio::test]
    #[ignore = "requires public Radio Browser API access"]
    async fn live_radio_browser_search_returns_stable_station_uuids() {
        let stations = search_radio_stations("jazz".into(), None, None, Some(5))
            .await
            .unwrap();
        assert!(stations.iter().all(|station| {
            station.id.starts_with("rb:")
                && is_uuid(&station.id[3..])
                && !station.stream_url.is_empty()
        }));
    }
    #[tokio::test]
    async fn radio_metadata_rejects_untrusted_station_ids_before_network_access() {
        let error = get_radio_metadata("https://attacker.test/metadata".into())
            .await
            .unwrap_err();
        assert_eq!(error, "Unknown radio station");
    }
}

#[cfg(test)]
mod secure_radio_transport_tests {
    use super::*;
    #[test]
    fn upgrade_preserves_exact_mount_and_query_without_rewriting_custom_ports() {
        let source = url::Url::parse("http://stream.example/live.mp3?mount=one").unwrap();
        assert_eq!(
            secure_stream_candidate(&source).unwrap().as_str(),
            "https://stream.example/live.mp3?mount=one"
        );
        assert!(secure_stream_candidate(
            &url::Url::parse("https://stream.example/live.mp3").unwrap()
        )
        .is_none());
        assert!(secure_stream_candidate(
            &url::Url::parse("http://stream.example:8000/live.mp3").unwrap()
        )
        .is_none());
    }
    async fn fixture() -> (url::Url, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut bytes = [0u8; 4096];
                    let size = stream.read(&mut bytes).await.unwrap();
                    let request = String::from_utf8_lossy(&bytes[..size]);
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let response = match path {
                        "/redirect" => {
                            "HTTP/1.1 302 Found\r\nLocation: /audio?mount=one\r\nContent-Length: 0\r\n\r\n"
                        }
                        "/loop" => {
                            "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n"
                        }
                        "/private" => {
                            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\n\r\n"
                        }
                        "/credentials" => {
                            "HTTP/1.1 302 Found\r\nLocation: http://user:secret@radio-fixture.test/audio\r\nContent-Length: 0\r\n\r\n"
                        }
                        "/bad" => {
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n"
                        }
                        "/slow" => {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\n\r\n"
                        }
                        _ => {
                            "HTTP/1.1 200 OK\r\nContent-Type: audio/aac\r\nTransfer-Encoding: chunked\r\n\r\n"
                        }
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                    // Never complete a live body: resolving transport must only inspect headers.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                });
            }
        });
        (
            url::Url::parse(&format!("http://radio-fixture.test:{port}/redirect")).unwrap(),
            task,
        )
    }
    #[tokio::test]
    async fn saved_redirect_resolution_verifies_audio_headers_without_consuming_live_body() {
        let (url, server) = fixture().await;
        let address = [format!("127.0.0.1:{}", url.port().unwrap())
            .parse()
            .unwrap()];
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            probe_station_transport(url.clone(), Some(&address)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.url, url.join("/audio?mount=one").unwrap().as_str());
        assert_eq!(result.mime_type.as_deref(), Some("audio/aac"));
        assert_eq!(result.codec.as_deref(), Some("AAC"));
        for (path, expected) in [
            ("/loop", "loop"),
            ("/private", "nonpublic"),
            ("/credentials", "credentials"),
            ("/bad", "content type"),
        ] {
            let error = probe_station_transport(url.join(path).unwrap(), Some(&address))
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{path}: {error}");
        }
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            probe_station_transport(url.join("/slow").unwrap(), Some(&address))
        )
        .await
        .is_err());
        server.abort();
    }
    #[tokio::test]
    async fn cancellation_and_deadline_do_not_rewrite_saved_station_or_trust_old_cache() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("radio.sqlite")).unwrap();
        let stations = json!([{"id":"custom:transport","name":"Transport fixture","streamUrl":"http://127.0.0.1/private","sourceUrl":"https://radio.example","provider":"custom","isCustom":true}]);
        store
            .set_json_preference(SAVED_RADIO_STATIONS_KEY, &stations)
            .unwrap();
        store.set_provider_cache("radio.secure-playback-transport.v1", "custom:transport", &json!({"original":"http://127.0.0.1/private","playback":"https://127.0.0.1/private"}), 86400).unwrap();
        let (_sender, cancel) = tokio::sync::watch::channel(true);
        let error = resolve_station_playback_until(
            "custom:transport".into(),
            &store,
            tokio::time::Instant::now() + Duration::from_secs(1),
            cancel,
        )
        .await
        .unwrap_err();
        assert!(error.contains("cancelled"));
        let (_sender, cancel) = tokio::sync::watch::channel(false);
        let error = resolve_station_playback_until(
            "custom:transport".into(),
            &store,
            tokio::time::Instant::now() - Duration::from_millis(1),
            cancel,
        )
        .await
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert_eq!(
            store
                .get_json_preference(SAVED_RADIO_STATIONS_KEY)
                .unwrap()
                .unwrap(),
            stations
        );
        let error = resolve_station_playback_url("custom:transport".into(), &store)
            .await
            .unwrap_err();
        assert!(error.contains("nonpublic"), "{error}");
    }
    #[test]
    fn successful_html_and_playlist_pages_are_not_audio_transport_proof() {
        assert!(radio_audio_content_type("audio/mpeg"));
        assert!(radio_audio_content_type("audio/aac; charset=binary"));
        assert!(radio_audio_content_type("application/vnd.apple.mpegurl"));
        assert!(!radio_audio_content_type("text/html"));
        assert!(!radio_audio_content_type("text/plain"));
        assert!(!radio_audio_content_type("application/json"));
        assert!(!radio_audio_content_type(""));
    }
}

#[path = "radio_playlist.rs"]
mod radio_playlist;
pub use radio_playlist::{
    auto_add_played_radio_track, get_radio_playlist_option, list_radio_playlist_targets,
    set_radio_playlist_option, RadioPlaylistOption,
};
