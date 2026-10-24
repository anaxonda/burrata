//! Broad lyric discovery; explicit candidate choices remain scoped to playback.
use crate::{
    lyrics::{self, ApiRecord, LyricsRequest, LyricsRequestInput, LyricsResponse},
    Store,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    pub id: String,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration_seconds: f64,
    pub has_synced: bool,
    pub automatic_match: bool,
    pub explanation: String,
    pub source_url: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub scope_id: String,
    pub status: String,
    pub candidates: Vec<Candidate>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Document {
    #[serde(flatten)]
    pub document: LyricsResponse,
    pub scope_id: String,
    pub candidates: Vec<Candidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_notice: Option<String>,
}
struct Entry {
    inserted: Instant,
    page: Page,
    records: HashMap<String, ApiRecord>,
    selected: Option<Document>,
}
static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn context(
    snapshot: &Value,
    visible: Option<LyricsRequestInput>,
) -> Option<(String, LyricsRequest, Option<String>)> {
    let native = snapshot["engine"] == "native"
        && matches!(
            snapshot["status"].as_str(),
            Some("playing" | "paused" | "buffering")
        );
    let id = if native {
        snapshot["track"]["id"].as_str().map(str::to_owned)
    } else {
        visible.as_ref().and_then(|v| v.track_id.clone())
    };
    let request = lyrics::request_for_current_track(snapshot, visible)?;
    if !lyrics::valid_request(&request) {
        return None;
    }
    let key = format!(
        "{}\0{}",
        id.as_deref().unwrap_or_default(),
        lyrics::request_key(&request)
    );
    Some((format!("{:x}", Sha256::digest(key.as_bytes())), request, id))
}
pub(crate) fn selected(snapshot: &Value, visible: Option<LyricsRequestInput>) -> Option<Document> {
    let (key, _, _) = context(snapshot, visible)?;
    cache()
        .lock()
        .ok()?
        .get(&key)
        .filter(|entry| entry.inserted.elapsed() < Duration::from_secs(600))?
        .selected
        .clone()
}
fn title_key(value: &str) -> String {
    let parsed = crate::recording_identity::parse_title(value);
    crate::matching::normalize(
        parsed
            .title
            .split('(')
            .next()
            .unwrap_or(&parsed.title)
            .split(" - ")
            .next()
            .unwrap_or(&parsed.title)
            .trim(),
    )
}
fn plausible(requests: &[LyricsRequest], record: &ApiRecord) -> bool {
    record.duration.is_finite()
        && record.duration > 0.0
        && record.duration <= 86_400.0
        && !record.instrumental
        && record.track_name.len() <= 512
        && record.artist_name.len() <= 512
        && (record
            .plain_lyrics
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty() && text.len() <= 65536)
            || record
                .synced_lyrics
                .as_ref()
                .is_some_and(|text| !text.trim().is_empty() && text.len() <= 65536))
        && requests
            .iter()
            .any(|request| title_key(&request.track_name) == title_key(&record.track_name))
}
fn queries(requests: &[LyricsRequest]) -> Vec<(&'static str, String)> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for request in requests {
        for (field, title) in [
            ("track_name", request.track_name.clone()),
            ("q", title_key(&request.track_name)),
        ] {
            if seen.insert((field, crate::matching::normalize(&title))) {
                result.push((field, title));
            }
        }
        if let Some(token) = title_key(&request.track_name)
            .split_whitespace()
            .find(|word| word.chars().count() >= 5)
        {
            if seen.insert(("q", token.to_owned())) {
                result.push(("q", token.to_owned()));
            }
        }
    }
    result.truncate(4);
    result
}
fn profile_names(raw: &Value, expected: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = raw["name"].as_str() {
        names.push(name.to_owned());
    }
    for alias in raw["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .take(64)
        .filter(|alias| alias["type"] != "Search hint")
    {
        if let Some(name) = alias["name"].as_str() {
            if name.len() <= 256 && !name.chars().any(char::is_control) {
                names.push(name.to_owned());
            }
        }
    }
    if !names
        .iter()
        .any(|name| crate::matching::normalize(name) == crate::matching::normalize(expected))
    {
        return vec![];
    }
    names.truncate(8);
    names
}
async fn discover_artist_names(
    artists: &crate::artists::ArtistService,
    request: &LyricsRequest,
) -> Vec<String> {
    let lookup = async {
        let found = artists
            .search_artists_for(
                "musicbrainz",
                &request.artist_name,
                1,
                3,
                &crate::artists::ArtistCredentials::default(),
            )
            .await
            .ok()?;
        let mut matches = Vec::new();
        let mut seen = HashSet::new();
        for found in found.results.into_iter().take(3) {
            if !seen.insert(found.artist.id.clone()) {
                continue;
            }
            let raw = artists
                .json_for_metadata(
                    "musicbrainz",
                    &format!("artist/{}?inc=aliases", found.artist.id),
                    "lyrics.discovery.artist.musicbrainz.v1",
                    &found.artist.id,
                    None,
                )
                .await
                .ok()?;
            if raw["id"].as_str() != Some(found.artist.id.as_str()) {
                continue;
            }
            let names = profile_names(&raw, &request.artist_name);
            if !names.is_empty() {
                matches.push(names);
            }
        }
        (matches.len() == 1).then(|| matches.remove(0))
    };
    tokio::time::timeout(Duration::from_secs(6), lookup)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}
pub(crate) async fn search(
    snapshot: &Value,
    visible: Option<LyricsRequestInput>,
    store: &Store,
    artists: &crate::artists::ArtistService,
) -> Page {
    let Some((key, request, id)) = context(snapshot, visible) else {
        return Page {
            scope_id: String::new(),
            status: "invalidRequest".into(),
            candidates: vec![],
        };
    };
    if let Ok(entries) = cache().lock() {
        if let Some(entry) = entries
            .get(&key)
            .filter(|e| e.inserted.elapsed() < Duration::from_secs(600))
        {
            return entry.page.clone();
        }
    }
    let mut requests = vec![request.clone()];
    if let Some(track) = id
        .and_then(|id| store.get_track(&id).ok().flatten())
        .filter(|track| {
            crate::matching::normalize(&track.title)
                == crate::matching::normalize(&request.track_name)
                && crate::matching::normalize(&track.artist)
                    == crate::matching::normalize(&request.artist_name)
        })
    {
        if let Ok(aliases) = tokio::time::timeout(
            Duration::from_secs(5),
            crate::lyrics_aliases::requests(store, artists, &track, &request),
        )
        .await
        {
            requests.extend(aliases);
        }
    }
    let mut records = HashMap::new();
    let mut reachable = false;
    let lookup = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Burrata/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| ())?;
        for (field, query) in queries(&requests) {
            let Ok(mut response) = client
                .get("https://lrclib.net/api/search")
                .query(&[(field, query)])
                .send()
                .await
            else {
                continue;
            };
            #[cfg(test)]
            eprintln!(
                "Lyric discovery field={field}, HTTP {}",
                response.status().as_u16()
            );
            if !response.status().is_success() {
                continue;
            }
            let mut bytes = Vec::new();
            let mut valid = true;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) if bytes.len() + chunk.len() <= 1024 * 1024 => {
                        bytes.extend_from_slice(&chunk)
                    }
                    Ok(None) => break,
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                continue;
            }
            let Ok(values) = serde_json::from_slice::<Vec<Value>>(&bytes) else {
                continue;
            };
            reachable = true;
            for raw in values.into_iter().take(100) {
                let Some(id) = raw["id"].as_u64() else {
                    continue;
                };
                let Ok(record) = serde_json::from_value::<ApiRecord>(raw) else {
                    continue;
                };
                if plausible(&requests, &record) {
                    records.entry(id.to_string()).or_insert(record);
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok::<(), ()>(())
    };
    // Artist-name verification and lyric retrieval are independent; neither
    // should add its full network wait to the other.
    let (_, artist_names) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(10), lookup),
        discover_artist_names(artists, &request)
    );
    for artist in artist_names {
        let mut alias = request.clone();
        alias.artist_name = artist;
        requests.push(alias);
    }
    let mut candidates = records
        .iter()
        .map(|(id, record)| {
            let automatic = requests
                .iter()
                .any(|request| lyrics::record_matches(request, record));
            Candidate {
                id: id.clone(),
                track_name: record.track_name.clone(),
                artist_name: record.artist_name.clone(),
                album_name: record.album_name.clone(),
                duration_seconds: record.duration,
                has_synced: record
                    .synced_lyrics
                    .as_ref()
                    .is_some_and(|v| !v.trim().is_empty()),
                automatic_match: automatic,
                explanation: if automatic {
                    "Recording metadata or verified artist alias matches"
                } else {
                    "Related lyrics; performance or artist credit requires your choice"
                }
                .into(),
                source_url: format!("https://lrclib.net/api/get/{id}"),
            }
        })
        .collect::<Vec<_>>();
    let album_matches = |candidate: &Candidate| {
        request
            .album_name
            .as_deref()
            .zip(candidate.album_name.as_deref())
            .is_some_and(|(expected, actual)| {
                crate::matching::normalize(expected) == crate::matching::normalize(actual)
            })
    };
    candidates.sort_by(|a, b| {
        b.automatic_match
            .cmp(&a.automatic_match)
            .then_with(|| album_matches(b).cmp(&album_matches(a)))
            .then_with(|| {
                (a.duration_seconds - request.duration_seconds)
                    .abs()
                    .total_cmp(&(b.duration_seconds - request.duration_seconds).abs())
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    candidates.truncate(10);
    records.retain(|id, _| candidates.iter().any(|c| &c.id == id));
    let page = Page {
        scope_id: key.clone(),
        status: if !reachable {
            "providerUnavailable"
        } else if candidates.is_empty() {
            "notFound"
        } else {
            "ok"
        }
        .into(),
        candidates,
    };
    if !page.candidates.is_empty() {
        if let Ok(mut entries) = cache().lock() {
            entries.retain(|_, e| e.inserted.elapsed() < Duration::from_secs(600));
            if entries.len() >= 64 {
                if let Some(old) = entries.keys().next().cloned() {
                    entries.remove(&old);
                }
            }
            entries.insert(
                key,
                Entry {
                    inserted: Instant::now(),
                    page: page.clone(),
                    records,
                    selected: None,
                },
            );
        }
    }
    page
}
pub(crate) fn choose(
    snapshot: &Value,
    visible: Option<LyricsRequestInput>,
    scope: &str,
    id: &str,
) -> Result<Document, String> {
    let (key, request, _) = context(snapshot, visible).ok_or("Current playback changed")?;
    if key != scope {
        return Err("Current playback changed; search lyrics again".into());
    }
    let mut entries = cache().lock().map_err(|_| "Lyric candidates unavailable")?;
    let entry = entries
        .get_mut(&key)
        .filter(|e| e.inserted.elapsed() < Duration::from_secs(600))
        .ok_or("Lyric candidates expired; search again")?;
    let candidate = entry
        .page
        .candidates
        .iter()
        .find(|c| c.id == id)
        .ok_or("Choose a current lyric candidate")?;
    let mut record = entry
        .records
        .get(id)
        .cloned()
        .ok_or("Lyric candidate unavailable")?;
    let document = if candidate.automatic_match {
        let chosen = LyricsRequest {
            track_name: record.track_name.clone(),
            artist_name: record.artist_name.clone(),
            album_name: record.album_name.clone(),
            duration_seconds: request.duration_seconds,
        };
        lyrics::classify_record(&chosen, record)
    } else {
        let plain = record
            .plain_lyrics
            .clone()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                record.synced_lyrics.as_ref().map(|v| {
                    lyrics::parse_lrc(v)
                        .into_iter()
                        .map(|line| line.text)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            });
        record.synced_lyrics = None;
        record.plain_lyrics = plain;
        let chosen = LyricsRequest {
            track_name: record.track_name.clone(),
            artist_name: record.artist_name.clone(),
            album_name: record.album_name.clone(),
            duration_seconds: record.duration,
        };
        let mut result = lyrics::classify_record(&chosen, record);
        if let Some(matched) = result.matched.as_mut() {
            matched.duration_delta_seconds =
                (matched.duration_seconds - request.duration_seconds).abs();
        }
        result
    };
    let result=Document{document,scope_id:key,candidates:entry.page.candidates.clone(),selection_notice:(!candidate.automatic_match).then(||"You selected related lyrics. The exact performance is unverified; timing is disabled.".into())};
    entry.selected = Some(result.clone());
    Ok(result)
}
pub(crate) async fn current(
    snapshot: &Value,
    visible: Option<LyricsRequestInput>,
    store: &Store,
    artists: &crate::artists::ArtistService,
) -> Document {
    if let Some(selected) = selected(snapshot, visible.clone()) {
        return selected;
    }
    let primary =
        lyrics::get_current_track_lyrics_with_catalog(snapshot, visible.clone(), store, artists)
            .await;
    let key = context(snapshot, visible.clone())
        .map(|v| v.0)
        .unwrap_or_default();
    if matches!(
        primary.status.as_str(),
        "synced" | "unsynced" | "instrumental"
    ) {
        return Document {
            document: primary,
            scope_id: key,
            candidates: vec![],
            selection_notice: None,
        };
    }
    let page = search(snapshot, visible.clone(), store, artists).await;
    if let Some(candidate) = page
        .candidates
        .iter()
        .find(|candidate| candidate.automatic_match)
    {
        if let Ok(selected) = choose(snapshot, visible, &page.scope_id, &candidate.id) {
            return selected;
        }
    }
    Document {
        document: primary,
        scope_id: page.scope_id,
        candidates: page.candidates,
        selection_notice: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Value {
        serde_json::json!({"engine":"native","status":"paused","track":{"id":format!("catalog:lyrics:{}",uuid::Uuid::new_v4()),"title":"Synthetic Song","artist":"Synthetic Artist","durationSeconds":180.0}})
    }
    #[test]
    fn related_selection_drops_timestamps_and_is_bound_to_current_playback() {
        let snapshot = snapshot();
        let (scope, request, _) = context(&snapshot, None).unwrap();
        let record:ApiRecord=serde_json::from_value(serde_json::json!({"trackName":"Synthetic Song","artistName":"Another Performer","albumName":"Another performance","duration":200.0,"instrumental":false,"syncedLyrics":"[00:00]Synthetic first line\n[00:05]Synthetic second line","plainLyrics":null})).unwrap();
        assert!(plausible(&[request], &record));
        let candidate = Candidate {
            id: "1".into(),
            track_name: record.track_name.clone(),
            artist_name: record.artist_name.clone(),
            album_name: record.album_name.clone(),
            duration_seconds: 200.0,
            has_synced: true,
            automatic_match: false,
            explanation: "Related performance".into(),
            source_url: "https://lrclib.net/api/get/1".into(),
        };
        cache().lock().unwrap().insert(
            scope.clone(),
            Entry {
                inserted: Instant::now(),
                page: Page {
                    scope_id: scope.clone(),
                    status: "ok".into(),
                    candidates: vec![candidate],
                },
                records: HashMap::from([("1".into(), record)]),
                selected: None,
            },
        );
        let result = choose(&snapshot, None, &scope, "1").unwrap();
        assert_eq!(result.document.status, "unsynced");
        assert!(result.selection_notice.is_some());
        assert!(result
            .document
            .lines
            .as_ref()
            .unwrap()
            .iter()
            .all(|line| line.time_seconds.is_none()));
        assert!(selected(&snapshot, None).is_some());
        assert!(choose(&self::snapshot(), None, &scope, "1").is_err());
        assert!(choose(&snapshot, None, &scope, "fabricated").is_err());
    }
    #[test]
    fn title_variants_are_discovery_only_and_artist_aliases_require_exact_profile_membership() {
        let request = LyricsRequest {
            track_name: "Kalimankou Denkou (The Evening Gathering)".into(),
            artist_name: "Synthetic Choir".into(),
            album_name: None,
            duration_seconds: 311.0,
        };
        assert!(queries(&[request]).contains(&("q", "kalimankou denkou".to_owned())));
        let raw = serde_json::json!({"name":"Canonical Choir","aliases":[{"name":"Synthetic Choir"},{"name":"Хор"},{"name":"Misleading hint","type":"Search hint"}]});
        let names = profile_names(&raw, "Synthetic Choir");
        assert!(names.contains(&"Хор".into()));
        assert!(!names.contains(&"Misleading hint".into()));
        assert!(profile_names(&raw, "Different Choir").is_empty());
    }
    #[tokio::test]
    #[ignore = "live public LRCLIB and MusicBrainz lookup; sends metadata only"]
    async fn live_choir_discovery_returns_selectable_plain_lyrics() {
        let root = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store::open(&root.path().join("lyrics.sqlite")).unwrap());
        let artists = crate::artists::ArtistService::new(store.clone()).unwrap();
        let snapshot = serde_json::json!({"engine":"native","status":"paused","track":{"id":"catalog:lyrics:live-choir","title":"Kalimankou Denkou (The Evening Gathering)","artist":"Bulgarian State Television Female Choir","durationSeconds":311.4666669}});
        let page = search(&snapshot, None, &store, &artists).await;
        eprintln!(
            "Choir candidates={}, automatic={}",
            page.candidates.len(),
            page.candidates.iter().filter(|c| c.automatic_match).count()
        );
        assert!(
            page.candidates.len() >= 2,
            "Expected several public lyric candidates, status={}",
            page.status
        );
        let candidate = page
            .candidates
            .iter()
            .find(|c| c.id == "18769911")
            .or_else(|| page.candidates.first())
            .unwrap();
        let result = choose(&snapshot, None, &page.scope_id, &candidate.id).unwrap();
        assert_eq!(result.document.status, "unsynced");
        assert!(result.document.lines.unwrap().len() >= 5);
        assert!(store.list_tracks("library", None).unwrap().is_empty());
    }
    #[tokio::test]
    #[ignore = "authorized live lyric translation through OpenRouter using the supplied .env path"]
    async fn live_choir_selected_document_translates_with_luna() {
        let root = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store::open(&root.path().join("lyrics.sqlite")).unwrap());
        let artists = crate::artists::ArtistService::new(store.clone()).unwrap();
        let snapshot = serde_json::json!({"engine":"native","status":"paused","track":{"id":"catalog:lyrics:live-translation-choir","title":"Kalimankou Denkou (The Evening Gathering)","artist":"Bulgarian State Television Female Choir","durationSeconds":311.4666669}});
        let page = search(&snapshot, None, &store, &artists).await;
        let candidate = page
            .candidates
            .iter()
            .find(|c| c.id == "18769911")
            .expect("Known reported lyric candidate must be discoverable");
        let doc = choose(&snapshot, None, &page.scope_id, &candidate.id)
            .unwrap()
            .document;
        let env = std::fs::read_to_string(
            std::env::var("BURRATA_TRANSLATION_QUALIFICATION_ENV")
                .expect("Authorized .env path required"),
        )
        .unwrap();
        let key = env
            .lines()
            .filter_map(|line| line.split_once('='))
            .find(|(name, _)| name.trim() == "OPENROUTER_API_KEY")
            .map(|(_, key)| {
                key.trim()
                    .trim_matches(|c| c == '\'' || c == '"')
                    .to_owned()
            })
            .expect("Authorized OpenRouter key required");
        let lines = doc.lines.unwrap();
        let hash = doc.source_hash.unwrap();
        let result = crate::lyrics_translation::translate_current_document(
            crate::connections::LyricsTranslationConnection {
                provider: crate::connections::LyricsTranslationProvider::Auto,
                base_url: "https://openrouter.ai/api/v1".into(),
                model: "openai/gpt-6-luna".into(),
                target_language: "en".into(),
                api_key: key,
            },
            hash.clone(),
            lines.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.source_hash, hash);
        assert_eq!(result.lines.len(), lines.len());
        for (source, translated) in lines.iter().zip(&result.lines) {
            assert_eq!(source.id, translated.id);
            assert_eq!(
                source.text.trim().is_empty(),
                translated.text.trim().is_empty()
            );
        }
        eprintln!(
            "Choir translation lines={}, provider={}, model={:?}, fallback={}",
            result.lines.len(),
            result.provider,
            result.model,
            result.fallback_used
        );
    }
}
