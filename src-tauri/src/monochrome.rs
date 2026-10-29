//! Public Monochrome Tracks catalog/direct audio adapter. No broker credentials
//! or origin spoofing: unavailable sources fail normally and permit fallback.
use crate::models::{AudioFormat, Candidate, Track};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BASE: &str = "https://tracks.monochrome.st";
const SEARCH_PATH: &str = "/search/tracks";
const CATALOG_SEARCH_LIMIT: usize = 25;
const SEARCH_CACHE_SCHEMA: &str = "monochrome-track-search-v2";
pub(crate) const CANDIDATE_SEARCH_LIMIT: usize = 12;
pub(crate) const INTERACTIVE_SEARCH_LIMIT: usize = 30;
static DEFAULT_HTTP_CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> =
    std::sync::OnceLock::new();

fn default_http_client() -> Result<reqwest::Client, String> {
    DEFAULT_HTTP_CLIENT
        .get_or_init(|| build_http_client())
        .as_ref()
        .map(|client| client.clone())
        .map_err(|error| error.clone())
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

static DEFAULT_COOLDOWN: std::sync::OnceLock<Arc<Mutex<Option<Instant>>>> =
    std::sync::OnceLock::new();
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    cooldown: Arc<Mutex<Option<Instant>>>,
}
fn dedupe_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut positions = std::collections::HashMap::<String, usize>::new();
    let mut unique = Vec::<Candidate>::new();
    for candidate in candidates {
        if let Some(index) = positions.get(&candidate.id).copied() {
            if candidate.score > unique[index].score {
                unique[index] = candidate;
            }
        } else {
            positions.insert(candidate.id.clone(), unique.len());
            unique.push(candidate);
        }
    }
    unique
}

impl Client {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            http: default_http_client()?,
            base: BASE.into(),
            cooldown: DEFAULT_COOLDOWN.get_or_init(Default::default).clone(),
        })
    }
    #[cfg(test)]
    pub(crate) fn at(base: &str) -> Result<Self, String> {
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            http: build_http_client()?,
            cooldown: Arc::new(Mutex::new(None)),
        })
    }
    pub(crate) fn check_cooldown(&self) -> Result<(), String> {
        if self
            .cooldown
            .lock()
            .map_err(|_| "Monochrome rate-limit state unavailable")?
            .is_some_and(|until| until > Instant::now())
        {
            return Err("Monochrome is temporarily rate limited (HTTP 429)".into());
        }
        Ok(())
    }

    /// Search the public artist/release catalog. The Tracks API accepts a
    /// result limit but exposes no offset or total count, so callers must not
    /// manufacture continuation pages.
    pub(crate) async fn search_catalog(
        &self,
        kind: &str,
        query: &str,
        requested_limit: usize,
    ) -> Result<Vec<Value>, String> {
        if query.trim().is_empty() || query.chars().count() > 120 {
            return Err("Catalog search must contain 1–120 characters".into());
        }
        let (path, field) = match kind {
            "artists" => ("/search/artists", "artists"),
            "releases" => ("/search/releases", "releases"),
            _ => return Err("Unsupported Monochrome catalog search".into()),
        };
        self.check_cooldown()?;
        let limit = requested_limit.clamp(1, CATALOG_SEARCH_LIMIT).to_string();
        crate::diagnostic_metrics::record_http_send_attempt();
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[("q", query), ("limit", limit.as_str())])
            .send()
            .await
            .map_err(|_| "Monochrome catalog connection failed")?;
        self.observe_rate_limit(&response);
        if !response.status().is_success() {
            return Err(format!(
                "Monochrome catalog returned HTTP {}",
                response.status()
            ));
        }
        let value = self.read_json_response(response, 2 * 1024 * 1024).await?;
        value
            .get(field)
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| format!("Invalid Monochrome {kind} response"))
    }

    /// Fetch one provider identity detail page. IDs are treated as opaque
    /// path segments and are restricted to the provider's public ID alphabet.
    pub(crate) async fn catalog_detail(&self, kind: &str, id: &str) -> Result<Value, String> {
        if id.is_empty()
            || id.len() > 80
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err("Invalid Monochrome catalog identity".into());
        }
        let path = match kind {
            "artists" => "artists",
            "releases" => "releases",
            _ => return Err("Unsupported Monochrome catalog identity".into()),
        };
        self.check_cooldown()?;
        crate::diagnostic_metrics::record_http_send_attempt();
        let response = self
            .http
            .get(format!("{}/{path}/{id}", self.base))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| "Monochrome catalog connection failed")?;
        self.observe_rate_limit(&response);
        if !response.status().is_success() {
            return Err(format!(
                "Monochrome catalog returned HTTP {}",
                response.status()
            ));
        }
        self.read_json_response(response, 4 * 1024 * 1024).await
    }

    async fn read_json_response(
        &self,
        response: reqwest::Response,
        max_bytes: usize,
    ) -> Result<Value, String> {
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Monochrome catalog response failed")?;
            if body.len() + chunk.len() > max_bytes {
                return Err("Monochrome catalog response is too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "Invalid Monochrome catalog response".into())
    }
    fn observe_rate_limit(&self, response: &reqwest::Response) {
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(30)
                .clamp(1, 120);
            if let Ok(mut cooldown) = self.cooldown.lock() {
                let until = Instant::now() + Duration::from_secs(seconds);
                *cooldown = Some(cooldown.map_or(until, |current| current.max(until)));
            }
        }
    }
    pub async fn search(&self, query: &str) -> Result<Vec<(Track, Candidate)>, String> {
        self.search_with_limit(query, CANDIDATE_SEARCH_LIMIT)
            .await
            .map(|(rows, _)| rows)
    }

    /// A stable cache profile for raw search rows. The endpoint and adapter
    /// schema are included so a future API or parser change cannot reuse stale
    /// rows. Plain HTTP is accepted only for loopback test clients.
    pub(crate) fn search_cache_profile(&self) -> Result<String, String> {
        let base = url::Url::parse(&self.base).map_err(|_| "Invalid Monochrome server")?;
        let loopback_test = cfg!(test)
            && base.scheme() == "http"
            && base
                .host_str()
                .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]"));
        let production_endpoint = base.scheme() == "https"
            && base.host_str() == Some("tracks.monochrome.st")
            && base.port().is_none();
        if (!production_endpoint && !loopback_test)
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !matches!(base.path(), "" | "/")
        {
            return Err("Monochrome cache profile requires a safe server origin".into());
        }
        Ok(serde_json::json!([
            SEARCH_CACHE_SCHEMA,
            base.origin().ascii_serialization(),
            SEARCH_PATH,
            CANDIDATE_SEARCH_LIMIT
        ])
        .to_string())
    }

    /// Search one exact provider query and retain provider rows before applying
    /// request-specific score/confidence calculations. These rows can be reused
    /// across matching levels and canonical/alias request variants.
    pub(crate) async fn search_candidates_raw_with_limit(
        &self,
        query: &str,
        requested_limit: usize,
    ) -> Result<(Vec<Candidate>, usize), String> {
        let (rows, raw_count) = self.search_with_limit(query, requested_limit).await?;
        Ok((
            rows.into_iter().map(|(_, candidate)| candidate).collect(),
            raw_count,
        ))
    }

    /// Fetch the bounded playback search page represented by
    /// [`Client::search_cache_profile`].
    pub(crate) async fn search_candidates_raw(
        &self,
        query: &str,
    ) -> Result<(Vec<Candidate>, usize), String> {
        self.search_candidates_raw_with_limit(query, CANDIDATE_SEARCH_LIMIT)
            .await
    }

    /// Fetch one public Tracks API result batch, with the endpoint's documented
    /// `limit` parameter. The endpoint has no offset/cursor or total-count field.
    pub(crate) async fn search_with_limit(
        &self,
        query: &str,
        requested_limit: usize,
    ) -> Result<(Vec<(Track, Candidate)>, usize), String> {
        if query.trim().is_empty() || query.chars().count() > 500 {
            return Err("Enter a search of 1–500 characters".into());
        }
        self.check_cooldown()?;
        let limit = requested_limit.clamp(1, INTERACTIVE_SEARCH_LIMIT);
        let limit_parameter = limit.to_string();
        crate::diagnostic_metrics::record_http_send_attempt();
        let response = self
            .http
            .get(format!("{}{SEARCH_PATH}", self.base))
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[("q", query), ("limit", limit_parameter.as_str())])
            .send()
            .await
            .map_err(|_| "Monochrome search connection failed")?;
        self.observe_rate_limit(&response);
        if !response.status().is_success() {
            return Err(format!(
                "Monochrome search returned HTTP {}",
                response.status()
            ));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Monochrome search response failed")?;
            if body.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err("Monochrome search response is too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        let data: Value =
            serde_json::from_slice(&body).map_err(|_| "Invalid Monochrome catalog response")?;
        let raw_tracks = data
            .get("tracks")
            .and_then(Value::as_array)
            .ok_or("Invalid Monochrome catalog tracks response")?;
        let raw_count = raw_tracks.len();
        let rows = raw_tracks
            .iter()
            .take(limit)
            .filter_map(|raw| catalog_entry(raw, &self.base))
            .collect();
        Ok((rows, raw_count))
    }
    pub async fn candidates(&self, track: &Track) -> Result<Vec<Candidate>, String> {
        let query = search_query_for_track(track);
        let (mut candidates, _) = self.search_candidates_raw(&query).await?;
        for candidate in &mut candidates {
            score_candidate_for_track(track, candidate);
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(candidates)
    }

    /// Search a caller-bounded sequence with one cancellation receiver and one
    /// absolute deadline shared by all queries. Playback supplies at most the
    /// canonical track and one verified alias.
    pub(crate) async fn candidates_for_tracks_until(
        &self,
        tracks: &[Track],
        cancel: tokio::sync::watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> (Vec<Candidate>, Vec<String>) {
        let mut candidates = Vec::new();
        let mut errors = Vec::new();
        for track in tracks {
            if *cancel.borrow() {
                return (Vec::new(), vec!["Cancelled".into()]);
            }
            if tokio::time::Instant::now() >= deadline {
                errors.push("Monochrome search timed out".into());
                break;
            }
            let result = tokio::select! {
                biased;
                _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => {
                    return (Vec::new(), vec!["Cancelled".into()]);
                }
                result = tokio::time::timeout_at(deadline, self.candidates(track)) => {
                    result.unwrap_or_else(|_| Err("Monochrome search timed out".into()))
                }
            };
            match result {
                Ok(found) => candidates.extend(found),
                Err(error) if error == "Monochrome search timed out" => {
                    errors.push(error);
                    break;
                }
                Err(error) => errors.push(error),
            }
        }
        (dedupe_candidates(candidates), errors)
    }
    pub async fn inspect(&self, candidate: &Candidate) -> Result<Candidate, String> {
        self.check_cooldown()?;
        let parsed =
            url::Url::parse(&candidate.url).map_err(|_| "Invalid Monochrome stream URL")?;
        let base = url::Url::parse(&self.base).map_err(|_| "Invalid Monochrome server")?;
        if parsed.origin() != base.origin()
            || !valid_path(parsed.path())
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("Monochrome candidate must use a catalog track URL".into());
        }
        crate::diagnostic_metrics::record_http_send_attempt();
        let response = self
            .http
            .get(parsed)
            .header(reqwest::header::RANGE, "bytes=0-63")
            .send()
            .await
            .map_err(|_| "Monochrome audio connection failed")?;
        self.observe_rate_limit(&response);
        if !response.status().is_success() {
            return Err(format!(
                "Monochrome audio returned HTTP {}",
                response.status()
            ));
        }
        let mut prefix = Vec::new();
        let mut stream = response.bytes_stream();
        while prefix.len() < 64 {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let chunk = chunk.map_err(|_| "Monochrome audio probe failed")?;
            prefix.extend_from_slice(&chunk[..chunk.len().min(64 - prefix.len())]);
        }
        let format = audio_format(&prefix)?;
        let mut result = candidate.clone();
        result.formats = vec![format];
        result
            .reasons
            .retain(|r| !r.starts_with("Verified audio container:"));
        result.reasons.push(format!(
            "Verified audio container: {}",
            result.formats[0].extension.to_uppercase()
        ));
        Ok(result)
    }
}

fn valid_path(path: &str) -> bool {
    path.strip_prefix("/track/")
        .is_some_and(|id| (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()))
}
pub fn validate_url(raw: &str) -> Result<(), String> {
    let url = crate::acquisition::validate_source_url(raw, "monochrome")?;
    if !valid_path(url.path()) || url.query().is_some() || url.fragment().is_some() {
        return Err("Invalid Monochrome track URL".into());
    }
    Ok(())
}

fn catalog_entry(raw: &Value, base: &str) -> Option<(Track, Candidate)> {
    let id = raw.get("trackId").or_else(|| raw.get("id"))?.as_str()?;
    if !(17..=20).contains(&id.len())
        || !id.bytes().all(|b| b.is_ascii_digit())
        || raw.get("playable") == Some(&Value::Bool(false))
    {
        return None;
    }
    let title = raw.get("title")?.as_str()?.trim();
    let raw_artist_names = raw.get("artistNames")?.as_array()?;
    if raw_artist_names.is_empty() || raw_artist_names.len() > 32 {
        return None;
    }
    let structured_artist_names = raw_artist_names
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty() && name.len() <= 512)
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    let artist = structured_artist_names.join(", ");
    let duration = raw
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n / 1000.0);
    let isrc = raw
        .get("isrc")
        .and_then(Value::as_str)
        .and_then(crate::recording_identity::normalize_isrc);
    let artwork = raw
        .get("artwork")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let url = format!("{base}/track/{id}");
    let mut source_metadata =
        json!({"provider":"monochrome","trackId":id,"isrc":isrc,"catalogTitle":title});
    source_metadata["artistNames"] = json!(structured_artist_names);
    // Keep only bounded, explicitly recognized descriptive fields from the
    // provider row. These are source metadata, not recording identity proof;
    // consumers still need the shared match policy before applying them.
    for (target, aliases, max_len) in [
        ("album", &["album"][..], 512usize),
        ("albumArtist", &["albumArtist", "album_artist"][..], 512),
        ("releaseDate", &["releaseDate", "release_date"][..], 64),
        ("genre", &["genre"][..], 256),
        ("label", &["label"][..], 256),
    ] {
        if let Some(value) = aliases
            .iter()
            .find_map(|key| raw.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= max_len)
        {
            source_metadata[target] = json!(value);
        }
    }
    let candidate = Candidate {
        id: format!("monochrome:{id}"),
        provider: "monochrome".into(),
        url: url.clone(),
        title: format!("{artist} - {title}"),
        uploader: Some(artist.clone()),
        duration_seconds: duration,
        artwork_url: artwork.clone(),
        confidence: "review".into(),
        permission: "unknown".into(),
        score: 0.8,
        reasons: vec![
            "Selected Monochrome catalog entry; delivered quality must be inspected".into(),
        ],
        source_metadata: Some(source_metadata),
        ..Candidate::default()
    };
    let track = Track {
        id: candidate.id.clone(),
        provider: "monochrome".into(),
        provider_id: id.into(),
        title: title.into(),
        artist,
        duration_seconds: duration,
        isrc,
        artwork_url: artwork,
        source_url: url,
        provenance: json!({"sourceKind":"audioSearch","audioCandidate":candidate}),
        ..Track::default()
    };
    Some((track, candidate))
}
// Presentation suffixes are not recording versions. Keep live/remix/cover labels.
fn recording_title(raw: &str) -> String {
    let title = crate::matching::normalize(raw);
    for suffix in [
        " official audio",
        " official video",
        " official music video",
    ] {
        if let Some(clean) = title.strip_suffix(suffix) {
            return clean.trim().into();
        }
    }
    title
}
pub(crate) fn search_query_for_track(track: &Track) -> String {
    format!("{} {}", track.artist, recording_title(&track.title))
}

pub(crate) fn score_candidate_for_track(track: &Track, candidate: &mut Candidate) {
    let mut matching_track = track.clone();
    matching_track.title = recording_title(&track.title);
    if matching_track.duration_seconds.is_none() {
        matching_track.duration_seconds = track
            .provenance
            .get("audioCandidate")
            .and_then(|c| c.get("durationSeconds"))
            .and_then(Value::as_f64);
    }
    let track = &matching_track;
    crate::matching::score_candidate(track, candidate);
    let isrc = candidate
        .source_metadata
        .as_ref()
        .and_then(|m| m.get("isrc"))
        .and_then(Value::as_str);
    let exact = track
        .isrc
        .as_deref()
        .zip(isrc)
        .is_some_and(|(a, b)| crate::recording_identity::same_isrc(a, b));
    let duration = track
        .duration_seconds
        .zip(candidate.duration_seconds)
        .is_some_and(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() <= 5.0);
    let title = crate::matching::normalize(&track.title);
    let upload = crate::matching::normalize(&candidate.title);
    let conflict = candidate
        .reasons
        .iter()
        .any(|r| r.contains("version conflicts"));
    let catalog_title = candidate
        .source_metadata
        .as_ref()
        .and_then(|m| m.get("catalogTitle"))
        .and_then(Value::as_str)
        .map(recording_title);
    let exact_artist = crate::matching::normalize_single_artist_name(&track.artist)
        == crate::matching::normalize_single_artist_name(
            candidate.uploader.as_deref().unwrap_or(""),
        )
        && !track.artist.trim().is_empty();
    let version = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    let version_supported = version.is_empty()
        || matches!(version.as_str(), "original" | "original mix")
        || candidate
            .reasons
            .iter()
            .any(|r| r == "Catalog version is present in upload title");
    let contradictory_isrc = track.isrc.as_deref().zip(isrc).is_some_and(|(a, b)| {
        !a.is_empty() && !b.is_empty() && !crate::recording_identity::same_isrc(a, b)
    });
    if duration
        && exact_artist
        && catalog_title.as_deref() == Some(title.as_str())
        && !title.is_empty()
        && version_supported
        && !conflict
        && !contradictory_isrc
    {
        candidate.score = 0.97;
        candidate.confidence = "high".into();
        candidate
            .reasons
            .push("Exact catalog artist/title and matching recording duration".into());
    }
    if exact && duration && !conflict && format!(" {upload} ").contains(&format!(" {title} ")) {
        candidate.score = 1.0;
        candidate.confidence = "high".into();
        candidate.reasons.push(format!(
            "Exact ISRC match: {}",
            track.isrc.as_deref().unwrap_or_default()
        ));
    }
}

fn audio_format(prefix: &[u8]) -> Result<AudioFormat, String> {
    let (codec, extension, lossless) = if prefix.starts_with(b"fLaC") {
        ("flac", "flac", true)
    } else if prefix.get(4..8) == Some(b"ftyp") {
        ("mp4", "m4a", false)
    } else {
        return Err("Monochrome returned an unsupported audio container (not FLAC or M4A)".into());
    };
    let sample_rate = if lossless && prefix.len() >= 26 {
        let packed = u64::from_be_bytes(prefix[18..26].try_into().unwrap());
        let rate = (packed >> 44) as u32;
        (rate > 0).then_some(rate)
    } else {
        None
    };
    Ok(AudioFormat {
        bit_depth: None,
        channels: None,
        size_bytes: None,
        id: extension.into(),
        codec: codec.into(),
        extension: extension.into(),
        bitrate: None,
        sample_rate,
        lossless,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_keeps_complete_credits_and_canonical_isrc() {
        let mut raw = json!({"trackId":"155142501534011392","title":"Song","artistNames":["Artist","Guest"],"duration":200000,"isrc":"US-QX9-13-00108"});
        let (track, candidate) = catalog_entry(&raw, BASE).unwrap();
        assert_eq!(track.isrc.as_deref(), Some("USQX91300108"));
        assert_eq!(
            candidate.source_metadata.unwrap()["artistNames"],
            json!(["Artist", "Guest"])
        );
        raw["artistNames"] = json!(["Artist", null]);
        assert!(catalog_entry(&raw, BASE).is_none());
    }

    #[test]
    fn verifies_container_instead_of_trusting_lossless_labels() {
        assert!(audio_format(b"fLaC").unwrap().lossless);
        let m4a = audio_format(b"\0\0\0\x1cftypisom").unwrap();
        assert_eq!(m4a.extension, "m4a");
        assert!(!m4a.lossless);
        assert!(audio_format(b"<html>denied</html>").is_err());
        assert!(validate_url("https://evil.example/track/155142501534011392").is_err());
        assert!(
            validate_url("https://tracks.monochrome.st/track/155142501534011392?token=x").is_err()
        );
    }
    #[test]
    fn catalog_entry_preserves_structured_artist_names_for_playback_identity() {
        let raw = json!({
            "trackId": "155142501534011392",
            "title": "Song",
            "artistNames": ["Artist", "Guest"],
            "duration": 200000,
        });
        let (_, candidate) = catalog_entry(&raw, BASE).unwrap();
        assert_eq!(candidate.uploader.as_deref(), Some("Artist, Guest"));
        assert_eq!(
            candidate.source_metadata.as_ref().unwrap()["artistNames"],
            json!(["Artist", "Guest"])
        );

        let empty = json!({
            "trackId": "155142501534011393",
            "title": "Song",
            "artistNames": [],
            "duration": 200000,
        });
        assert!(catalog_entry(&empty, BASE).is_none());
    }

    #[test]
    fn catalog_entry_retains_only_bounded_optional_metadata_fields() {
        let raw = json!({
            "trackId":"155142501534011392", "title":"Song", "artistNames":["Artist"],
            "album":"Release", "album_artist":"Album Artist", "release_date":"2024-03-01",
            "genre":"Folk", "label":"Example Label", "catalogNumber":"CAT-1",
            "notes":{"large":"ignored"}
        });
        let (_, candidate) = catalog_entry(&raw, BASE).unwrap();
        let metadata = candidate.source_metadata.unwrap();
        assert_eq!(metadata["album"], "Release");
        assert_eq!(metadata["albumArtist"], "Album Artist");
        assert_eq!(metadata["releaseDate"], "2024-03-01");
        assert_eq!(metadata["genre"], "Folk");
        assert_eq!(metadata["label"], "Example Label");
        assert!(metadata.get("catalogNumber").is_none());

        let oversized = json!({
            "trackId":"155142501534011392", "title":"Song", "artistNames":["Artist"],
            "album":"x".repeat(513), "albumArtist":17, "releaseDate":"x".repeat(65),
            "genre":"  ", "label":"Label"
        });
        let (_, candidate) = catalog_entry(&oversized, BASE).unwrap();
        let metadata = candidate.source_metadata.unwrap();
        assert!(metadata.get("album").is_none());
        assert!(metadata.get("albumArtist").is_none());
        assert!(metadata.get("releaseDate").is_none());
        assert!(metadata.get("genre").is_none());
        assert_eq!(metadata["label"], "Label");
    }

    #[test]
    fn join_phrase_credit_remains_a_review_candidate_for_strict_playback_gate() {
        let raw = json!({
            "trackId": "155142501534011392",
            "title": "Song",
            "artistNames": ["Artist", "Guest"],
            "duration": 200000,
        });
        let (_, mut candidate) = catalog_entry(&raw, BASE).unwrap();
        let track = Track {
            title: "Song".into(),
            artist: "Artist feat. Guest".into(),
            duration_seconds: Some(200.0),
            ..Track::default()
        };
        score_candidate_for_track(&track, &mut candidate);
        assert_eq!(candidate.confidence, "review");
        assert!(candidate.score >= 0.55);
    }

    #[test]
    fn isrc_match_does_not_accept_wrong_duration_or_conflicting_remix() {
        let raw = json!({"trackId":"155142501534011392","title":"Get Lucky","artistNames":["Daft Punk"],"duration":369626,"isrc":"USQX91300108"});
        let (_, candidate) = catalog_entry(&raw, BASE).unwrap();
        let track = Track {
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            isrc: Some("USQX91300108".into()),
            duration_seconds: Some(369.0),
            ..Track::default()
        };
        let mut exact = candidate.clone();
        score_candidate_for_track(&track, &mut exact);
        assert_eq!(exact.confidence, "high");
        let mut radio = candidate.clone();
        radio.duration_seconds = Some(247.0);
        score_candidate_for_track(&track, &mut radio);
        assert_ne!(radio.confidence, "high");
        let mut remix = candidate.clone();
        remix.title.push_str(" (Radio Edit)");
        let versioned = Track {
            version: Some("Original Mix".into()),
            ..track
        };
        score_candidate_for_track(&versioned, &mut remix);
        assert_eq!(remix.confidence, "low");
    }

    #[test]
    fn peter_paul_mary_catalog_and_credit_variant_matches_playback_strictly() {
        let raw = json!({
            "trackId":"168503514673217536",
            "title":"500 Miles",
            "artistNames":["Peter, Paul and Mary"],
            "duration":162933,
            "isrc":"USWB16500047"
        });
        let (_, mut candidate) = catalog_entry(&raw, BASE).unwrap();
        let track = Track {
            artist: "Peter, Paul & Mary".into(),
            title: "500 Miles".into(),
            duration_seconds: Some(162.0),
            ..Track::default()
        };

        // The review-confidence catalog path should already be playable from
        // exact title/artist and full-duration agreement; scoring may promote
        // this same identity to high confidence afterward.
        assert_eq!(candidate.confidence, "review");
        assert!(crate::source_fallback::playback_recording_matches(
            &track, &candidate
        ));
        score_candidate_for_track(&track, &mut candidate);
        assert_eq!(candidate.confidence, "high");
        assert!(candidate
            .reasons
            .iter()
            .any(|reason| reason == "Exact catalog artist/title and matching recording duration"));
        assert!(crate::source_fallback::playback_recording_matches(
            &track, &candidate
        ));

        let wrong_artist = Candidate {
            source_metadata: Some(json!({
                "catalogTitle":"500 Miles",
                "artistNames":["Peter Paul Mary"],
                "isrc":"USWB16500047"
            })),
            uploader: Some("Peter Paul Mary".into()),
            confidence: "review".into(),
            ..candidate.clone()
        };
        assert!(!crate::source_fallback::playback_recording_matches(
            &track,
            &wrong_artist
        ));

        let wrong_duration = Candidate {
            duration_seconds: Some(182.293),
            confidence: "review".into(),
            ..candidate.clone()
        };
        assert!(!crate::source_fallback::playback_recording_matches(
            &track,
            &wrong_duration
        ));

        let versioned_track = Track {
            version: Some("Extended Mix".into()),
            ..track.clone()
        };
        assert!(!crate::source_fallback::playback_recording_matches(
            &versioned_track,
            &candidate
        ));
    }
    #[test]
    fn imported_official_audio_matches_catalog_but_not_covers_or_live_versions() {
        let raw = json!({"trackId":"174208180224667648","title":"Quizás, Porque","artistNames":["Sui Generis"],"duration":132973,"isrc":"ARF109700694"});
        let (_, candidate) = catalog_entry(&raw, BASE).unwrap();
        let track = Track {
            title: "Quizás, Porque (Official Audio)".into(),
            artist: "Sui Generis".into(),
            provenance: json!({"audioCandidate":{"durationSeconds":135}}),
            ..Track::default()
        };
        let mut result = candidate.clone();
        score_candidate_for_track(&track, &mut result);
        assert_eq!(result.confidence, "high");
        for title in ["Quizás, Porque (Live)", "Quizás, Porque Cover"] {
            let mut result = candidate.clone();
            score_candidate_for_track(
                &Track {
                    title: title.into(),
                    ..track.clone()
                },
                &mut result,
            );
            assert_ne!(result.confidence, "high");
        }
        let mut result = candidate.clone();
        score_candidate_for_track(
            &Track {
                artist: "Another artist".into(),
                ..track.clone()
            },
            &mut result,
        );
        assert_ne!(result.confidence, "high");
        let mut result = candidate.clone();
        score_candidate_for_track(
            &Track {
                provenance: json!({}),
                ..track
            },
            &mut result,
        );
        assert_ne!(result.confidence, "high");
    }
    #[tokio::test]
    async fn rate_limited_probe_suppresses_catalog_retry_until_bounded_cooldown_expires() {
        use axum::{routing::get, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let app = Router::new().fallback(get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "9999")],
                    "rate limited",
                )
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let candidate = Candidate {
            url: format!("{}/track/154030613014253568", client.base),
            ..Candidate::default()
        };
        assert!(client
            .inspect(&candidate)
            .await
            .unwrap_err()
            .contains("429"));
        let remaining = client
            .cooldown
            .lock()
            .unwrap()
            .unwrap()
            .saturating_duration_since(Instant::now());
        assert!(remaining > Duration::from_secs(119) && remaining <= Duration::from_secs(120));
        let cooldown_error = client.clone().search("artist title").await.unwrap_err();
        assert_eq!(
            cooldown_error,
            "Monochrome is temporarily rate limited (HTTP 429)"
        );
        assert!(!cooldown_error.contains("trying eligible fallback sources"));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        *client.cooldown.lock().unwrap() = Some(Instant::now() - Duration::from_secs(1));
        assert!(client
            .search("artist title")
            .await
            .unwrap_err()
            .contains("429"));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn http_search_and_probe_do_not_claim_m4a_is_flac() {
        use axum::{routing::get, Json, Router};
        let app=Router::new().route("/search/tracks",get(||async{Json(json!({"tracks":[{"trackId":"155142501534011392","title":"Get Lucky","artistNames":["Daft Punk"],"duration":369626,"isrc":"USQX91300108"}]}))}))
            .route("/track/{id}",get(|headers:axum::http::HeaderMap|async move {
                assert_eq!(headers.get("range").unwrap(),"bytes=0-63");
                (axum::http::StatusCode::PARTIAL_CONTENT,b"\0\0\0\x1cftypisom".to_vec())
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let rows = client.search("Daft Punk Get Lucky").await.unwrap();
        let inspected = client.inspect(&rows[0].1).await.unwrap();
        assert_eq!(inspected.formats[0].extension, "m4a");
        assert!(!inspected.formats[0].lossless);
        server.abort();
    }

    #[tokio::test]
    async fn candidate_search_retains_flexible_alternative_below_legacy_score_cutoff() {
        use axum::{routing::get, Json, Router};

        let app = Router::new().route(
            "/search/tracks",
            get(|| async {
                Json(json!({"tracks": [
                    {"trackId":"155142501534011392","title":"Track (Original Mix)","artistNames":["Artist"],"duration":200000},
                    {"trackId":"155142501534011393","title":"Track (Alice Remix)","artistNames":["Artist"],"duration":200000}
                ]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let requested = Track {
            title: "Track".into(),
            artist: "Artist".into(),
            version: Some("Original Mix".into()),
            duration_seconds: Some(200.0),
            ..Track::default()
        };

        let cache_profile = client.search_cache_profile().unwrap();
        assert!(cache_profile.contains(SEARCH_CACHE_SCHEMA));
        assert!(cache_profile.contains("/search/tracks"));
        let (raw_candidates, raw_count) = client
            .search_candidates_raw_with_limit(
                &search_query_for_track(&requested),
                CANDIDATE_SEARCH_LIMIT,
            )
            .await
            .unwrap();
        assert_eq!(raw_count, 2);
        assert_eq!(raw_candidates.len(), 2);
        // Raw provider rows carry only the parser's neutral review score; the
        // request-specific heuristic is applied by `candidates` below.
        assert_eq!(raw_candidates[0].score, 0.8);
        assert_eq!(raw_candidates[1].score, 0.8);

        let candidates = client.candidates(&requested).await.unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].id, "monochrome:155142501534011392");
        let same_recording = crate::recording_match::assess(
            &requested,
            &candidates[0],
            crate::recording_match::MatchLevel::Flexible,
            crate::recording_match::MatchStage::Search,
        );
        assert_ne!(
            same_recording.relationship,
            crate::recording_match::MatchRelationship::Alternative
        );
        let alternative = candidates
            .iter()
            .find(|candidate| candidate.id.ends_with("155142501534011393"))
            .unwrap();
        assert_eq!(alternative.score, 0.49);
        assert_eq!(alternative.confidence, "low");

        let assessment = crate::recording_match::assess(
            &requested,
            alternative,
            crate::recording_match::MatchLevel::Flexible,
            crate::recording_match::MatchStage::Search,
        );
        assert_eq!(
            assessment.relationship,
            crate::recording_match::MatchRelationship::Alternative
        );
        assert_eq!(
            assessment.action,
            crate::recording_match::MatchAction::Inspect
        );
        server.abort();
    }

    #[test]
    fn cache_profile_rejects_non_https_non_loopback_endpoint() {
        let client = Client::at("http://example.invalid").unwrap();
        assert!(client.search_cache_profile().is_err());
    }

    #[tokio::test]
    async fn malformed_tracks_shape_is_an_error_but_empty_tracks_is_valid() {
        use axum::{extract::Query, routing::get, Json, Router};
        use std::collections::HashMap;

        let app = Router::new().route(
            "/search/tracks",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                if query.get("q").is_some_and(|value| value == "empty") {
                    Json(json!({"tracks": []}))
                } else {
                    Json(json!({"wrongField": []}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        assert!(client
            .search("malformed")
            .await
            .unwrap_err()
            .contains("tracks response"));
        assert!(client.search("empty").await.unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn bounded_candidate_sequence_issues_canonical_then_one_alias_query() {
        use axum::{extract::Query, routing::get, Json, Router};
        use std::{
            collections::HashMap,
            sync::{Arc, Mutex},
        };

        async fn search(
            axum::extract::State(seen): axum::extract::State<Arc<Mutex<Vec<String>>>>,
            Query(query): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            seen.lock().unwrap().push(query["q"].clone());
            Json(json!({"tracks": []}))
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/search/tracks", get(search))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let tracks = vec![
            Track {
                artist: "Ансамбъл Пирин".into(),
                title: "Sample (Original Mix)".into(),
                ..Track::default()
            },
            Track {
                artist: "Ensemble Pirin".into(),
                title: "Sample (Original Mix)".into(),
                ..Track::default()
            },
        ];
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let (found, errors) = client
            .candidates_for_tracks_until(
                &tracks,
                cancel,
                tokio::time::Instant::now() + Duration::from_secs(2),
            )
            .await;
        assert!(found.is_empty());
        assert!(errors.is_empty());
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                "Ансамбъл Пирин sample original mix".to_owned(),
                "Ensemble Pirin sample original mix".to_owned(),
            ]
        );
        server.abort();
    }

    #[tokio::test]
    async fn bounded_candidate_sequence_honors_cancel_and_expired_deadline_before_request() {
        use axum::{routing::get, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let app = Router::new().route(
            "/search/tracks",
            get(move || {
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"tracks": []}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let tracks = vec![Track {
            artist: "A".into(),
            title: "B".into(),
            ..Track::default()
        }];

        let (_cancel_tx, cancelled) = tokio::sync::watch::channel(true);
        let (found, errors) = client
            .candidates_for_tracks_until(
                &tracks,
                cancelled,
                tokio::time::Instant::now() + Duration::from_secs(1),
            )
            .await;
        assert!(found.is_empty());
        assert_eq!(errors, vec!["Cancelled"]);

        let (_live_tx, live) = tokio::sync::watch::channel(false);
        let (found, errors) = client
            .candidates_for_tracks_until(&tracks, live, tokio::time::Instant::now())
            .await;
        assert!(found.is_empty());
        assert_eq!(errors, vec!["Monochrome search timed out"]);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[test]
    fn duplicate_candidate_ids_keep_the_strongest_alias_scored_result() {
        let candidate = |id: &str, score: f64, title: &str| Candidate {
            source_metadata: None,
            id: id.into(),
            provider: "monochrome".into(),
            url: format!("https://tracks.monochrome.st/track/{id}"),
            title: title.into(),
            uploader: None,
            duration_seconds: None,
            artwork_url: None,
            chapters: None,
            formats: Vec::new(),
            permission: "public".into(),
            confidence: "high".into(),
            score,
            reasons: Vec::new(),
            official: true,
            source: None,
        };
        let unique = dedupe_candidates(vec![
            candidate("mono:1", 0.8, "canonical"),
            candidate("mono:1", 0.95, "alias"),
            candidate("mono:2", 0.7, "other"),
        ]);
        assert_eq!(unique.len(), 2);
        assert_eq!(unique[0].title, "alias");
        assert_eq!(unique[0].score, 0.95);
    }

    #[tokio::test]
    async fn bounded_candidate_sequence_cancels_an_in_flight_request() {
        use axum::{routing::get, Json, Router};
        use std::sync::Arc;
        use tokio::sync::Notify;

        let entered = Arc::new(Notify::new());
        let entered_route = entered.clone();
        let app = Router::new().route(
            "/search/tracks",
            get(move || {
                let entered = entered_route.clone();
                async move {
                    entered.notify_one();
                    std::future::pending::<()>().await;
                    Json(json!({"tracks": []}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let tracks = vec![Track {
            artist: "A".into(),
            title: "B".into(),
            ..Track::default()
        }];
        let (cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let lookup = tokio::spawn(async move {
            client
                .candidates_for_tracks_until(
                    &tracks,
                    cancel,
                    tokio::time::Instant::now() + Duration::from_secs(2),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        cancel_tx.send(true).unwrap();
        let (found, errors) = tokio::time::timeout(Duration::from_secs(1), lookup)
            .await
            .unwrap()
            .unwrap();
        assert!(found.is_empty());
        assert_eq!(errors, vec!["Cancelled"]);
        server.abort();
    }

    #[tokio::test]
    async fn bounded_candidate_sequence_uses_one_deadline_for_the_actual_request() {
        use axum::{routing::get, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        use tokio::sync::Notify;

        let requests = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let seen = requests.clone();
        let arrived = entered.clone();
        let app = Router::new().route(
            "/search/tracks",
            get(move || {
                let seen = seen.clone();
                let arrived = arrived.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    arrived.notify_one();
                    std::future::pending::<()>().await;
                    Json(json!({"tracks": []}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let tracks = vec![
            Track {
                artist: "Canonical".into(),
                title: "B".into(),
                ..Track::default()
            },
            Track {
                artist: "Alias".into(),
                title: "B".into(),
                ..Track::default()
            },
        ];
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let (found, errors) = tokio::time::timeout(
            Duration::from_secs(1),
            client.candidates_for_tracks_until(
                &tracks,
                cancel,
                tokio::time::Instant::now() + Duration::from_millis(80),
            ),
        )
        .await
        .unwrap();
        // Ensure the request actually entered before assessing its one shared deadline.
        // The server's notification is a retained permit if the request won the race above.
        let _ = tokio::time::timeout(Duration::from_millis(1), entered.notified()).await;
        assert!(found.is_empty());
        assert_eq!(errors, vec!["Monochrome search timed out"]);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn candidate_and_interactive_search_use_separate_bounded_limits() {
        use axum::{extract::Query, routing::get, Json, Router};
        use std::collections::HashMap;

        async fn search(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
            let limit = query["limit"].parse::<usize>().unwrap();
            assert!(!query.contains_key("offset"));
            let tracks = (0..limit)
                .map(|index| {
                    json!({
                        "trackId": format!("{:017}", 10000000000000000u64 + index as u64),
                        "title": format!("Song {index}"),
                        "artistNames": ["Test Artist"],
                        "duration": 180000,
                        "playable": true
                    })
                })
                .collect::<Vec<_>>();
            Json(json!({
                "tracks": tracks,
                "releases": [],
                "artists": [],
                "topResults": [],
                "users": [],
                "playlists": []
            }))
        }

        let app = Router::new().route("/search/tracks", get(search));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let candidates = client.search("Test Artist").await.unwrap();
        assert_eq!(candidates.len(), CANDIDATE_SEARCH_LIMIT);
        let (interactive, raw_count) = client
            .search_with_limit("Test Artist", INTERACTIVE_SEARCH_LIMIT)
            .await
            .unwrap();
        assert_eq!(interactive.len(), INTERACTIVE_SEARCH_LIMIT);
        assert_eq!(raw_count, INTERACTIVE_SEARCH_LIMIT);
        let (clamped, _) = client.search_with_limit("Test Artist", 500).await.unwrap();
        assert_eq!(clamped.len(), INTERACTIVE_SEARCH_LIMIT);

        server.abort();
    }

    #[tokio::test]
    async fn artist_and_release_catalog_routes_use_bounded_public_api_shapes() {
        use axum::{
            extract::{Path, Query},
            routing::get,
            Json, Router,
        };
        use std::collections::HashMap;
        async fn search(
            Path(kind): Path<String>,
            Query(query): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            assert_eq!(query.get("q").map(String::as_str), Some("Nipsey Hussle"));
            assert!(query
                .get("limit")
                .and_then(|v| v.parse::<usize>().ok())
                .is_some_and(|n| (1..=25).contains(&n)));
            assert!(!query.contains_key("offset"));
            if kind == "artists" {
                Json(
                    json!({"artists":[{"artistId":"123456789012345678","displayName":"Nipsey Hussle"}]}),
                )
            } else {
                Json(json!({"releases":[{"releaseId":"223456789012345678","title":"Victory Lap"}]}))
            }
        }
        async fn detail(Path((kind, id)): Path<(String, String)>) -> Json<Value> {
            assert_eq!(id, "123456789012345678");
            Json(
                json!({"artist":{"artistId":id,"displayName":"Nipsey Hussle","albums":[]},"kind":kind}),
            )
        }
        let app = Router::new()
            .route("/search/{kind}", get(search))
            .route("/{kind}/{id}", get(detail));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::at(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            client
                .search_catalog("artists", "Nipsey Hussle", 50)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            client
                .search_catalog("releases", "Nipsey Hussle", 25)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(client.catalog_detail("artists", "../unsafe").await.is_err());
        assert_eq!(
            client
                .catalog_detail("artists", "123456789012345678")
                .await
                .unwrap()["artist"]["displayName"],
            "Nipsey Hussle"
        );
        server.abort();
    }
}
