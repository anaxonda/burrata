//! Public Monochrome Tracks catalog/direct audio adapter. No broker credentials
//! or origin spoofing: unavailable sources fail normally and permit fallback.
use crate::models::{AudioFormat, Candidate, Track};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

const BASE: &str = "https://tracks.monochrome.st";

pub struct Client {
    http: reqwest::Client,
    base: String,
}
impl Client {
    pub fn new() -> Result<Self, String> {
        Self::at(BASE)
    }
    fn at(base: &str) -> Result<Self, String> {
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| e.to_string())?,
        })
    }
    pub async fn search(&self, query: &str) -> Result<Vec<(Track, Candidate)>, String> {
        if query.trim().is_empty() || query.chars().count() > 500 {
            return Err("Enter a search of 1–500 characters".into());
        }
        let response = self
            .http
            .get(format!("{}/search/tracks", self.base))
            .query(&[("q", query), ("limit", "12")])
            .send()
            .await
            .map_err(|_| "Monochrome search connection failed")?;
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
        Ok(data
            .get("tracks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(12)
            .filter_map(|raw| catalog_entry(raw, &self.base))
            .collect())
    }
    pub async fn candidates(&self, track: &Track) -> Result<Vec<Candidate>, String> {
        let query = format!("{} {}", track.artist, recording_title(&track.title));
        let rows = self.search(&query).await?;
        let mut candidates = rows
            .into_iter()
            .map(|(_, mut candidate)| {
                score(track, &mut candidate);
                candidate
            })
            .filter(|candidate| candidate.confidence != "low" && candidate.score >= 0.55)
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(candidates)
    }
    pub async fn inspect(&self, candidate: &Candidate) -> Result<Candidate, String> {
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
        let response = self
            .http
            .get(parsed)
            .header(reqwest::header::RANGE, "bytes=0-63")
            .send()
            .await
            .map_err(|_| "Monochrome audio connection failed")?;
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
    let artist = raw
        .get("artistNames")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let duration = raw
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n / 1000.0);
    let isrc = raw
        .get("isrc")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let artwork = raw
        .get("artwork")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let url = format!("{base}/track/{id}");
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
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":id,"isrc":isrc,"catalogTitle":title}),
        ),
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
fn score(track: &Track, candidate: &mut Candidate) {
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
        .is_some_and(|(a, b)| !a.is_empty() && a.eq_ignore_ascii_case(b));
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
    let exact_artist = crate::matching::normalize(&track.artist)
        == crate::matching::normalize(candidate.uploader.as_deref().unwrap_or(""))
        && !track.artist.trim().is_empty();
    let version = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    let version_supported = version.is_empty()
        || matches!(version.as_str(), "original" | "original mix")
        || candidate
            .reasons
            .iter()
            .any(|r| r == "Catalog version is present in upload title");
    let contradictory_isrc = track
        .isrc
        .as_deref()
        .zip(isrc)
        .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && !a.eq_ignore_ascii_case(b));
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
        score(&track, &mut exact);
        assert_eq!(exact.confidence, "high");
        let mut radio = candidate.clone();
        radio.duration_seconds = Some(247.0);
        score(&track, &mut radio);
        assert_ne!(radio.confidence, "high");
        let mut remix = candidate.clone();
        remix.title.push_str(" (Radio Edit)");
        let versioned = Track {
            version: Some("Original Mix".into()),
            ..track
        };
        score(&versioned, &mut remix);
        assert_eq!(remix.confidence, "low");
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
        score(&track, &mut result);
        assert_eq!(result.confidence, "high");
        for title in ["Quizás, Porque (Live)", "Quizás, Porque Cover"] {
            let mut result = candidate.clone();
            score(
                &Track {
                    title: title.into(),
                    ..track.clone()
                },
                &mut result,
            );
            assert_ne!(result.confidence, "high");
        }
        let mut result = candidate.clone();
        score(
            &Track {
                artist: "Another artist".into(),
                ..track.clone()
            },
            &mut result,
        );
        assert_ne!(result.confidence, "high");
        let mut result = candidate.clone();
        score(
            &Track {
                provenance: json!({}),
                ..track
            },
            &mut result,
        );
        assert_ne!(result.confidence, "high");
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
}
