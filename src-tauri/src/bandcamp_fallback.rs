//! Bounded public Bandcamp fallback. Search rows only locate candidates;
//! fresh page metadata or an independent public catalog audio preview verifies
//! the recording. No catalog preview is ever returned as playback audio.
use crate::{
    acquisition::AcquisitionManager,
    models::{Candidate, Settings, Track},
    native_output::{AudioSource, AudioSourceDecoderHint},
    playback_sources::{PlaybackDeadline, ResolvedSource, ResolvedSourceOrigin, SourceTrace},
    storage::Store,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, sync::watch};

#[path = "bandcamp_album_crosswalk.rs"]
mod album_crosswalk;

const CACHE: &str = "playback.bandcamp-reference.v1";
const MAX_JSON: usize = 512 * 1024;
const MAX_AUDIO: usize = 10 * 1024 * 1024;
const RATE: usize = 8000;

fn fingerprint(track: &Track) -> String {
    format!("{:x}", Sha256::digest(json!({"id":track.id,"provider":track.provider,"providerId":track.provider_id,"sourceUrl":track.source_url,"title":track.title,"artist":track.artist,"album":track.album,"version":track.version,"duration":track.duration_seconds,"isrc":track.isrc}).to_string().as_bytes()))
}
fn compact(value: &str) -> String {
    crate::matching::normalize(value).replace(' ', "")
}
fn same_credit(a: &str, b: &str) -> bool {
    !a.trim().is_empty()
        && crate::source_fallback::normalize_artist_credit(a)
            == crate::source_fallback::normalize_artist_credit(b)
}
fn plausible(track: &Track, title: &str, artist: &str) -> bool {
    !track.title.trim().is_empty()
        && compact(&track.title) == compact(title)
        && !artist.trim().is_empty()
        && (same_credit(&track.artist, artist)
            || crate::matching::normalize(&track.artist)
                .starts_with(&format!("{} ", crate::matching::normalize(artist))))
}
pub(crate) fn eligible(track: &Track, settings: &Settings, pinned: bool, attempts: usize) -> bool {
    eligible_with_policy(
        track,
        settings,
        pinned,
        attempts,
        crate::bandcamp::playback_enabled(),
    )
}
fn eligible_with_policy(
    track: &Track,
    settings: &Settings,
    pinned: bool,
    attempts: usize,
    enabled: bool,
) -> bool {
    !pinned
        && attempts == 2
        && settings.streaming_fallback_enabled
        && settings.bandcamp_fallback_enabled
        && enabled
        && !matches!(track.provider.as_str(), "radio" | "youtube" | "soundcloud")
        && track.local_path.is_none()
        && track.asset_id.is_none()
        && track.playback_range.is_none()
        && track.version.as_deref().is_none_or(|v| v.trim().is_empty())
        && !track.title.trim().is_empty()
        && !track.artist.trim().is_empty()
        && track.title.len() <= 512
        && track.artist.len() <= 512
        && track
            .duration_seconds
            .is_none_or(|d| d.is_finite() && (25.0..=600.0).contains(&d))
        && track.provenance["chapters"]
            .as_array()
            .is_none_or(Vec::is_empty)
}

async fn bounded<T>(
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => Err("Cancelled".into()),
        _ = tokio::time::sleep_until(deadline.instant()) => Err("Bandcamp fallback exceeded the playback deadline".into()),
        result = future => result,
    }
}
async fn body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if !response.status().is_success()
        || response.content_length().is_some_and(|n| n > limit as u64)
    {
        return Err("Public recording reference is unavailable or exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Public recording reference could not be read")?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Public recording reference exceeds the size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Burrata/0.1 (public recording playback)")
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "Could not initialize Bandcamp fallback".into())
}

fn search_rows(track: &Track, raw: &Value) -> Vec<Candidate> {
    raw["auto"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .take(50)
        .filter_map(|row| {
            let id = row["id"].as_u64().filter(|id| *id > 0)?;
            let title = row["name"].as_str()?;
            let artist = row["band_name"].as_str()?;
            if row["type"] != "t" || !plausible(track, title, artist) {
                return None;
            }
            let url =
                crate::bandcamp::validate_track_page_url(row["item_url_path"].as_str()?).ok()?;
            Some(Candidate {
                id: format!("bandcamp:{id}"),
                provider: "bandcamp".into(),
                url: url.to_string(),
                title: title.into(),
                uploader: Some(artist.into()),
                duration_seconds: None,
                artwork_url: None,
                chapters: Some(vec![]),
                formats: vec![],
                permission: "stream_only".into(),
                confidence: "review".into(),
                score: 0.0,
                reasons: vec![],
                official: false,
                source: None,
                source_metadata: None,
            })
        })
        .collect()
}
async fn discover(
    track: &Track,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
) -> Result<Vec<Candidate>, String> {
    let client = client()?;
    // The second query tolerates compounds such as Nighttime/Night-time without
    // treating a partial query as evidence that an artist or guest may be omitted.
    let primary = track
        .artist
        .split(['&', ','])
        .next()
        .unwrap_or(&track.artist)
        .trim();
    let last = track
        .title
        .split_whitespace()
        .last()
        .unwrap_or(&track.title);
    let queries = [
        format!("{} {}", track.artist, track.title),
        format!("{primary} {last}"),
    ];
    for query in queries {
        let raw = bounded(cancel.clone(), deadline, async {
            crate::diagnostic_metrics::record_http_send_attempt();
            let response = client
                .post("https://bandcamp.com/api/bcsearch_public_api/1/autocomplete_elastic")
                .json(&json!({"search_text":query,"search_filter":"t","full_page":true}))
                .send()
                .await
                .map_err(|_| "Bandcamp track search is unavailable")?;
            serde_json::from_slice::<Value>(&body(response, MAX_JSON).await?)
                .map_err(|_| "Bandcamp track search returned invalid data".into())
        })
        .await?;
        let rows = search_rows(track, &raw);
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    Ok(vec![])
}

fn catalog_reference(track: &Track, raw: &Value) -> Option<Value> {
    let rows = raw["results"].as_array()?;
    if rows.len() >= 25 || raw["resultCount"].as_u64()? != rows.len() as u64 {
        return None;
    }
    let hits = rows
        .iter()
        .filter(|row| {
            row["kind"] == "song"
                && row["trackName"].as_str().is_some_and(|title| {
                    crate::matching::normalize(title) == crate::matching::normalize(&track.title)
                })
                && row["artistName"]
                    .as_str()
                    .is_some_and(|artist| same_credit(&track.artist, artist))
                && row["trackTimeMillis"].as_f64().is_some_and(|ms| {
                    (25_000.0..=600_000.0).contains(&ms)
                        && track
                            .duration_seconds
                            .is_none_or(|duration| (ms / 1000.0 - duration).abs() <= 1.0)
                })
                && track.album.as_deref().is_none_or(|album| {
                    row["collectionName"].as_str().is_some_and(|name| {
                        crate::matching::normalize(album) == crate::matching::normalize(name)
                    })
                })
        })
        .collect::<Vec<_>>();
    if hits.len() != 1 || hits[0]["trackId"].as_u64().is_none() {
        return None;
    }
    let url = reference_url(hits[0]["previewUrl"].as_str()?).ok()?;
    Some(
        json!({"trackId":hits[0]["trackId"],"title":hits[0]["trackName"],"artist":hits[0]["artistName"],"duration":hits[0]["trackTimeMillis"].as_f64()? / 1000.0,"collectionId":hits[0]["collectionId"],"album":hits[0]["collectionName"],"previewUrl":url.as_str()}),
    )
}
fn reference_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "Invalid public catalog preview")?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !(host.ends_with(".itunes.apple.com") || host.ends_with(".mzstatic.com"))
    {
        return Err("Unsupported public catalog preview".into());
    }
    Ok(url)
}
async fn public_audio(
    url: reqwest::Url,
    max: usize,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
) -> Result<Vec<u8>, String> {
    bounded(cancel, deadline, async {
        let addresses = crate::acquisition::public_addresses(&url).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(6))
            .resolve_to_addrs(url.host_str().ok_or("Missing audio host")?, &addresses)
            .build()
            .map_err(|_| "Could not initialize public audio reference")?;
        crate::diagnostic_metrics::record_http_send_attempt();
        body(
            client
                .get(url)
                .send()
                .await
                .map_err(|_| "Public audio reference is unavailable")?,
            max,
        )
        .await
    })
    .await
}

async fn decode(
    bytes: &[u8],
    format: &str,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
) -> Result<Vec<f32>, String> {
    let file = tempfile::NamedTempFile::new().map_err(|_| "Could not stage audio verification")?;
    std::fs::write(file.path(), bytes).map_err(|_| "Could not stage audio verification")?;
    bounded(cancel, deadline, async {
        let mut command =
            tokio::process::Command::new(crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"));
        command
            .args([
                "-nostdin",
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-threads",
                "1",
                "-f",
                format,
                "-i",
            ])
            .arg(file.path())
            .args([
                "-t", "601", "-vn", "-ac", "1", "-ar", "8000", "-f", "f32le", "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start audio verification decoder")?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or("Missing audio verification output")?
            .take((RATE * 601 * 4 + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Audio verification decode failed")?;
        if bytes.len() > RATE * 601 * 4 {
            return Err("Audio verification output exceeds the limit".into());
        }
        if !child
            .wait()
            .await
            .map_err(|_| "Audio verification decoder failed")?
            .success()
            || bytes.len() % 4 != 0
        {
            return Err("Audio verification decoder failed".into());
        }
        let samples = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect::<Vec<_>>();
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("Invalid decoded reference audio".into());
        }
        Ok(samples)
    })
    .await
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut aa, mut bb, mut ab) = (0.0f64, 0.0f64, 0.0f64);
    for (&a, &b) in a.iter().zip(b) {
        let (a, b) = (a as f64, b as f64);
        aa += a * a;
        bb += b * b;
        ab += a * b;
    }
    if aa < 1e-6 || bb < 1e-6 {
        return 0.0;
    }
    ab / (aa * bb).sqrt()
}
/// Narrow waveform identity test, not a fuzzy music classifier. Lossy re-encodes
/// may pass; altered tempo, performances and remixes generally fail closed.
fn compare(
    reference: &[f32],
    candidate: &[f32],
    cancel: &watch::Receiver<bool>,
    deadline: PlaybackDeadline,
) -> Result<(f64, usize), String> {
    if reference.len() < RATE * 24
        || reference.len() > RATE * 95
        || candidate.len() < reference.len()
        || candidate.len() > RATE * 601
    {
        return Err("Insufficient bounded audio for recording verification".into());
    }
    let reference = &reference[RATE..reference.len() - RATE];
    let down = |audio: &[f32]| {
        audio
            .chunks_exact(64)
            .map(|chunk| chunk.iter().sum::<f32>() / 64.0)
            .collect::<Vec<_>>()
    };
    let a = down(reference);
    let b = down(candidate);
    let window = 250; // Two seconds at 125 Hz; choose a non-silent seed.
    let seed = (0..a.len() - window)
        .step_by(125)
        .max_by(|&i, &j| {
            a[i..i + window]
                .iter()
                .map(|v| v * v)
                .sum::<f32>()
                .total_cmp(&a[j..j + window].iter().map(|v| v * v).sum::<f32>())
        })
        .ok_or("Reference audio is too short")?;
    let mut peaks: Vec<(f64, usize)> = vec![];
    for offset in 0..=b.len() - a.len() {
        if offset % 512 == 0 && (*cancel.borrow() || deadline.remaining().is_zero()) {
            return Err("Audio verification canceled or exceeded the playback deadline".into());
        }
        let score = correlation(
            &a[seed..seed + window],
            &b[offset + seed..offset + seed + window],
        )
        .abs();
        if peaks.last().is_none_or(|p| score > p.0) {
            peaks.push((score, offset));
            peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut kept = vec![];
            for peak in peaks.drain(..) {
                if kept
                    .iter()
                    .all(|p: &(f64, usize)| p.1.abs_diff(peak.1) > 125)
                {
                    kept.push(peak);
                }
                if kept.len() == 4 {
                    break;
                }
            }
            peaks = kept;
        }
    }
    let mut best = (0.0f64, 0usize);
    for (_, offset) in peaks {
        for start in (offset * 64).saturating_sub(64)
            ..=(offset * 64 + 64).min(candidate.len() - reference.len())
        {
            if *cancel.borrow() || deadline.remaining().is_zero() {
                return Err("Audio verification canceled or exceeded the playback deadline".into());
            }
            let len = (RATE * 4).min(reference.len());
            let score = correlation(&reference[..len], &candidate[start..start + len]).abs();
            if score > best.0 {
                best = (score, start);
            }
        }
    }
    let aligned = &candidate[best.1..best.1 + reference.len()];
    let score = correlation(reference, aligned).abs();
    if score < 0.96
        || (0..3).any(|part| {
            let start = reference.len() * part / 3;
            let end = reference.len() * (part + 1) / 3;
            correlation(&reference[start..end], &aligned[start..end]).abs() < 0.94
        })
    {
        return Err("Bandcamp audio does not verify the catalog recording".into());
    }
    Ok((score, best.1))
}

fn cached_binding_matches(
    proof: &Value,
    candidate: &Candidate,
    audio: &crate::acquisition::PreviewAudio,
) -> bool {
    ((proof["method"] == "waveform-v1"
        && proof["score"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && n >= 0.96))
        || (proof["method"] == "album-crosswalk-v1"
            && proof["albumTrackCount"]
                .as_u64()
                .is_some_and(|n| (8..=50).contains(&n))
            && proof["albumFingerprint"]
                .as_str()
                .is_some_and(|s| s.len() == 64)))
        && proof["referenceId"].as_u64().is_some()
        && proof["sourceId"] == candidate.id
        && proof["sourceUrl"] == candidate.url
        && proof["resolvedTitle"].as_str() == audio.resolved_title.as_deref()
        && proof["resolvedArtist"]
            == audio
                .source_metadata
                .as_ref()
                .map(|m| &m["channel"])
                .unwrap_or(&Value::Null)
                .clone()
        && proof["duration"]
            .as_f64()
            .zip(audio.duration_seconds)
            .is_some_and(|(a, b)| (a - b).abs() < 0.01)
}

/// Derive the core matcher’s internal exact-recording marker only from a
/// currently source-bound waveform or complete-album proof. Provider metadata
/// never gets to supply this marker.
fn strong_binding_id(
    proof: &Value,
    candidate: &Candidate,
    audio: &crate::acquisition::PreviewAudio,
) -> Option<String> {
    if !cached_binding_matches(proof, candidate, audio) {
        return None;
    }
    let identity = json!({
        "method": proof["method"],
        "referenceId": proof["referenceId"],
        "referenceSha256": proof["referenceSha256"],
        "albumFingerprint": proof["albumFingerprint"],
        "score": proof["score"],
        "sourceId": candidate.id,
        "sourceUrl": candidate.url,
        "resolvedTitle": audio.resolved_title,
        "resolvedArtist": audio.source_metadata.as_ref().map(|m| &m["channel"]),
        "duration": audio.duration_seconds,
    });
    Some(format!(
        "bandcamp-proof:{:x}",
        Sha256::digest(identity.to_string().as_bytes())
    ))
}

fn policy_artists(credit: &str) -> Vec<String> {
    credit
        .replace(" & ", ",")
        .replace(" and ", ",")
        .replace(" feat. ", ",")
        .replace(" featuring ", ",")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn proof_identity(proof: &Value, fallback_title: &str, fallback_artist: &str) -> (String, String) {
    if proof["method"] == "waveform-v1" {
        return (
            proof["referenceTitle"]
                .as_str()
                .unwrap_or(fallback_title)
                .to_owned(),
            proof["referenceArtist"]
                .as_str()
                .unwrap_or(fallback_artist)
                .to_owned(),
        );
    }
    if proof["method"] == "album-crosswalk-v1" {
        if let Some(row) = proof["albumMatches"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["catalogTrackId"] == proof["referenceId"])
        {
            return (
                row["catalogTitle"]
                    .as_str()
                    .unwrap_or(fallback_title)
                    .to_owned(),
                row["catalogArtist"]
                    .as_str()
                    .unwrap_or(fallback_artist)
                    .to_owned(),
            );
        }
    }
    (fallback_title.to_owned(), fallback_artist.to_owned())
}

/// Route the verified Bandcamp identity through the shared 3-level policy.
/// Strong audio/album proofs are added to cloned policy inputs only; the saved
/// request and its provider metadata remain untouched.
fn shared_policy_assessment(
    track: &Track,
    candidate: &Candidate,
    title: &str,
    artist: &str,
    duration: Option<f64>,
    binding: Option<&str>,
    level: crate::recording_match::MatchLevel,
) -> Option<crate::recording_match::MatchAssessment> {
    let mut requested = track.clone();
    let mut evaluated = candidate.clone();
    let mut metadata = json!({
        "provider": "bandcamp",
        "track": title,
        "catalogTitle": title,
        "artist": artist,
        "artists": policy_artists(artist),
        "artistCount": policy_artists(artist).len(),
        "duration": duration,
    });
    if let Some(binding) = binding {
        requested.provenance["recordingId"] = json!(binding);
        metadata["recordingId"] = json!(binding);
    }
    evaluated.title = title.to_owned();
    evaluated.duration_seconds = duration;
    evaluated.source_metadata = Some(metadata);

    let assessment = crate::recording_match::assess(
        &requested,
        &evaluated,
        level,
        crate::recording_match::MatchStage::Resolved,
    );
    (assessment.action == crate::recording_match::MatchAction::Accept
        && assessment.relationship != crate::recording_match::MatchRelationship::Alternative)
        .then_some(assessment)
}

pub(crate) async fn resolve(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    trace: SourceTrace,
    deadline: PlaybackDeadline,
    failed: &[crate::playback_sources::FailedPlaybackCandidate],
    reference_verification: bool,
) -> Result<ResolvedSource, String> {
    let key = fingerprint(track);
    let cached = store.get_provider_cache(CACHE, &key)?;
    let mut candidates = vec![];
    if let Some(row) = cached
        .as_ref()
        .and_then(|p| p.get("candidate"))
        .and_then(|c| serde_json::from_value::<Candidate>(c.clone()).ok())
    {
        if row.provider == "bandcamp" && crate::bandcamp::validate_track_page_url(&row.url).is_ok()
        {
            candidates.push(row);
        }
    }
    if candidates.is_empty() {
        candidates = discover(track, cancel.clone(), deadline).await?;
    }
    for mut candidate in candidates.into_iter().take(2) {
        if failed
            .iter()
            .any(|failed| failed.provider == candidate.provider && failed.id == candidate.id)
        {
            continue;
        }
        let audio = match acquisition
            .resolve_preview_audio_traced(
                &candidate,
                settings,
                cancel.clone(),
                trace.clone(),
                deadline,
            )
            .await
        {
            Ok(audio) => audio,
            Err(error) if *cancel.borrow() || error == "Cancelled" => {
                return Err("Cancelled".into());
            }
            Err(_) => continue,
        };
        if audio.resolved_provider_id.as_deref() != candidate.id.strip_prefix("bandcamp:") {
            continue;
        }
        let artist = audio
            .source_metadata
            .as_ref()
            .and_then(|m| m["channel"].as_str())
            .unwrap_or_default();
        let title = audio.resolved_title.as_deref().unwrap_or_default();
        let fresh_title = title.strip_prefix(artist).unwrap_or(title).trim();
        if !plausible(track, fresh_title, artist) {
            continue;
        }
        let strict = same_credit(&track.artist, artist)
            && crate::matching::normalize(&track.title) == crate::matching::normalize(fresh_title)
            && track
                .duration_seconds
                .zip(audio.duration_seconds)
                .is_some_and(|(a, b)| (a - b).abs() <= 1.0);
        let cached_audio = cached
            .as_ref()
            .is_some_and(|p| cached_binding_matches(p, &candidate, &audio));
        let mut proof = cached.clone().unwrap_or(Value::Null);
        if !strict && !cached_audio {
            store.set_provider_cache(
                CACHE,
                &key,
                &json!({"method":"pending","candidate":candidate}),
                300,
            )?;
            let metadata_deadline = if reference_verification {
                deadline
            } else {
                PlaybackDeadline::at(deadline.cap(Duration::from_secs(3)))
            };
            let client = client()?;
            let raw = bounded(cancel.clone(), metadata_deadline, async {
                crate::diagnostic_metrics::record_http_send_attempt();
                let response = client
                    .get("https://itunes.apple.com/search")
                    .query(&[
                        ("term", format!("{} {}", track.title, track.artist)),
                        ("entity", "song".into()),
                        ("limit", "25".into()),
                    ])
                    .send()
                    .await
                    .map_err(|_| "Public catalog reference is unavailable")?;
                serde_json::from_slice::<Value>(&body(response, MAX_JSON).await?)
                    .map_err(|_| "Public catalog reference returned invalid data".into())
            })
            .await;
            let raw = match raw {
                Ok(raw) => raw,
                Err(_) if !reference_verification && !*cancel.borrow() => continue,
                Err(error) => return Err(error),
            };
            let reference = catalog_reference(track, &raw)
                .ok_or("No unambiguous full-credit public catalog reference")?;
            if reference["duration"]
                .as_f64()
                .zip(audio.duration_seconds)
                .is_none_or(|(a, b)| (a - b).abs() > 1.0)
            {
                continue;
            }
            let album_proof = album_crosswalk::resolve(
                track,
                &candidate,
                &reference,
                &audio,
                cancel.clone(),
                metadata_deadline,
            )
            .await;
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
            if let Ok(Some(mut album_proof)) = album_proof {
                album_proof["referenceId"] = reference["trackId"].clone();
                album_proof["sourceId"] = json!(candidate.id);
                album_proof["sourceUrl"] = json!(candidate.url);
                album_proof["resolvedTitle"] = json!(audio.resolved_title);
                album_proof["resolvedArtist"] = json!(artist);
                album_proof["duration"] = json!(audio.duration_seconds);
                proof = album_proof;
            } else {
                if !reference_verification {
                    continue;
                }
                let reference_bytes = public_audio(
                    reference_url(reference["previewUrl"].as_str().unwrap_or_default())?,
                    2 * 1024 * 1024,
                    cancel.clone(),
                    deadline,
                )
                .await?;
                let candidate_bytes = public_audio(
                    reqwest::Url::parse(&audio.url).map_err(|_| "Invalid Bandcamp stream")?,
                    MAX_AUDIO,
                    cancel.clone(),
                    deadline,
                )
                .await?;
                let reference_pcm =
                    decode(&reference_bytes, "mov", settings, cancel.clone(), deadline).await?;
                let candidate_pcm =
                    decode(&candidate_bytes, "mp3", settings, cancel.clone(), deadline).await?;
                let comparison_cancel = cancel.clone();
                let comparison = tokio::task::spawn_blocking(move || {
                    compare(&reference_pcm, &candidate_pcm, &comparison_cancel, deadline)
                });
                let (score, offset) = comparison
                    .await
                    .map_err(|_| "Audio verification failed")??;
                proof = json!({"method":"waveform-v1","referenceId":reference["trackId"],"referenceTitle":reference["title"],"referenceArtist":reference["artist"],"referenceSha256":format!("{:x}",Sha256::digest(&reference_bytes)),"score":score,"offsetSamples":offset,"sampleRate":RATE,"sourceId":candidate.id,"sourceUrl":candidate.url,"resolvedTitle":audio.resolved_title,"resolvedArtist":artist,"duration":audio.duration_seconds});
            }
        }
        if *cancel.borrow() || deadline.remaining().is_zero() {
            return Err("Cancelled".into());
        }
        if strict {
            proof = json!({"method":"metadata-v1"});
        }
        let binding = if strict {
            None
        } else {
            strong_binding_id(&proof, &candidate, &audio)
        };
        let (policy_title, policy_artist) = proof_identity(&proof, fresh_title, artist);
        let assessment = shared_policy_assessment(
            track,
            &candidate,
            &policy_title,
            &policy_artist,
            audio.duration_seconds,
            binding.as_deref(),
            settings.matching_level,
        );
        let Some(assessment) = assessment else {
            // The independent source proof remains necessary, and the shared
            // policy must also accept the resolved recording identity.
            continue;
        };
        candidate.duration_seconds = audio.duration_seconds;
        candidate.title = policy_title.clone();
        candidate.confidence = "high".into();
        candidate.score = 1.0;
        let mut resolved_metadata = audio.source_metadata.clone().unwrap_or_else(|| json!({}));
        resolved_metadata["track"] = json!(policy_title);
        resolved_metadata["catalogTitle"] = json!(policy_title);
        resolved_metadata["artist"] = json!(policy_artist);
        resolved_metadata["artists"] = json!(policy_artists(&policy_artist));
        resolved_metadata["artistCount"] = json!(policy_artists(&policy_artist).len());
        resolved_metadata["duration"] = json!(audio.duration_seconds);
        resolved_metadata["recordingMatch"] = json!(assessment);
        resolved_metadata["matching"] = json!({
            "evaluatorVersion": crate::recording_match::EVALUATOR_VERSION,
            "level": settings.matching_level,
            "assessment": assessment,
            "resolvedRecording": {
                "title": policy_title,
                "artist": policy_artist,
                "version": crate::recording_identity::parse_title(&policy_title).version,
                "sourceId": candidate.id,
            }
        });
        if let Some(binding) = &binding {
            resolved_metadata["recordingId"] = json!(binding);
        }
        candidate.source_metadata = Some(resolved_metadata);
        proof["recordingMatch"] = json!(assessment);
        proof["candidate"] = json!(candidate);
        // Identity evidence survives ordinary cache refreshes; never retain signed URLs.
        if let Some(saved) = store.get_track(&track.id)? {
            if fingerprint(&saved) != key {
                return Err("Saved recording changed during Bandcamp verification".into());
            }
        } else {
            return Err("Saved recording no longer exists".into());
        }
        store.set_provider_cache(CACHE, &key, &proof, 315_576_000)?;
        return Ok(ResolvedSource {
            origin: ResolvedSourceOrigin::Existing,
            source: AudioSource {
                input: audio.url,
                ffmpeg: crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                headers: audio.headers,
                decoder_hint: Some(AudioSourceDecoderHint {
                    mime_type: Some(audio.mime_type),
                    codec: audio.codec,
                    duration_seconds: audio.duration_seconds,
                    content_length: audio.expected_content_length,
                    has_video: false,
                }),

                cenc: None,
            },
            candidate: Some(candidate),
            quality: json!({"codec":"mp3","bitrateKbps":128,"bitrateOrigin":"provider","source":"bandcamp"}),
            station: None,
            duration: audio.duration_seconds,
        });
    }
    Err("No verified publicly streamable Bandcamp recording was found".into())
}

/// Audio-reference work is never awaited by a play click. Only a failed ordinary
/// request with a plausible pending candidate schedules it, once per cooldown,
/// globally one at a time. Completion changes cache evidence, not playback.
pub(crate) fn schedule_verification(
    acquisition: &AcquisitionManager,
    track: &Track,
    settings: &Settings,
) {
    use std::sync::{Arc, OnceLock};
    static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let store = acquisition.alias_store();
    let key = fingerprint(track);
    if !store
        .get_provider_cache(CACHE, &key)
        .ok()
        .flatten()
        .is_some_and(|p| p["method"] == "pending")
        || store
            .get_provider_cache("playback.bandcamp-reference-jobs.v1", &key)
            .ok()
            .flatten()
            .is_some()
    {
        return;
    }
    let Ok(permit) = SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
        .try_acquire_owned()
    else {
        return;
    };
    if store
        .set_provider_cache(
            "playback.bandcamp-reference-jobs.v1",
            &key,
            &json!({"attemptedAt":chrono::Utc::now().timestamp()}),
            300,
        )
        .is_err()
    {
        return;
    }
    let acquisition = acquisition.clone();
    let track = track.clone();
    let settings = settings.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let (_stop, cancel) = watch::channel(false);
        let trace = SourceTrace::new(None, None, None);
        let deadline = PlaybackDeadline::after(Duration::from_secs(45));
        let started = std::time::Instant::now();
        let result = bounded(
            cancel.clone(),
            deadline,
            resolve(
                &store,
                &acquisition,
                &track,
                &settings,
                cancel.clone(),
                trace.clone(),
                deadline,
                &[],
                true,
            ),
        )
        .await;
        trace.event("bandcamp_reference_verification_end","bandcamp",json!({"accepted":result.is_ok(),"elapsedMs":started.elapsed().as_millis(),"background":true}));
        if result.is_ok() {
            acquisition.notify_bandcamp_source_ready(&track);
        }
    });
}

/// Transport recovery must refresh the exact cached source, never search for a
/// replacement or manufacture a proof from the caller's candidate sidecar.
pub(crate) async fn refresh(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    trace: SourceTrace,
    deadline: PlaybackDeadline,
) -> Result<ResolvedSource, String> {
    let proof = store
        .get_provider_cache(CACHE, &fingerprint(track))?
        .ok_or("Bandcamp recording verification is no longer available")?;
    let bound: Candidate = serde_json::from_value(proof["candidate"].clone())
        .map_err(|_| "Invalid Bandcamp recording verification")?;
    if bound.provider != "bandcamp" || bound.id != candidate.id || bound.url != candidate.url {
        return Err("Bandcamp recovery source identity changed".into());
    }
    resolve(
        store,
        acquisition,
        track,
        settings,
        cancel,
        trace,
        deadline,
        &[],
        false,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            id: "radio_song:fixture".into(),
            provider: "radio_song".into(),
            provider_id: "fixture".into(),
            title: "Nighttime in the Backyard".into(),
            artist: "Lucas Santtana & Arto Lindsay".into(),
            ..Default::default()
        }
    }
    #[test]
    fn discovery_does_not_treat_spelling_or_partial_credits_as_proof() {
        let raw = json!({"auto":{"results":[
            {"type":"t","id":2065435071u64,"name":"Night-time in the backyard","band_name":"Lucas Santtana","item_url_path":"https://lucassanttana.bandcamp.com/track/night-time-in-the-backyard"},
            {"type":"t","id":2,"name":"Nighttime in the Backyard (Remix)","band_name":"Lucas Santtana","item_url_path":"https://lucassanttana.bandcamp.com/track/remix"},
            {"type":"t","id":3,"name":"Nighttime in the Backyard","band_name":"Tostex","item_url_path":"https://tostex.bandcamp.com/track/cover"},
            {"type":"t","id":4,"name":"Nighttime in the Backyard","band_name":"Lucas Santtana","item_url_path":"https://evil.example/track/song"}
        ]}});
        let found = search_rows(&track(), &raw);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].confidence, "review");
        assert!(!same_credit(
            &track().artist,
            found[0].uploader.as_deref().unwrap()
        ));
    }
    #[test]
    fn preview_requires_unique_complete_full_credit_catalog_match() {
        let row = json!({"kind":"song","trackId":1677838124u64,"trackName":"Nighttime in the Backyard","artistName":"Lucas Santtana & Arto Lindsay","collectionName":"Sem Nostalgia","trackTimeMillis":197760,"previewUrl":"https://audio-ssl.itunes.apple.com/audio/preview.m4a"});
        let page = |rows: Vec<Value>| json!({"resultCount":rows.len(),"results":rows});
        assert!(catalog_reference(&track(), &page(vec![row.clone()])).is_some());
        assert!(catalog_reference(&track(), &page(vec![row.clone(), row.clone()])).is_none());
        for (field, value) in [
            ("artistName", json!("Lucas Santtana")),
            ("trackName", json!("Nighttime in the Backyard (Remix)")),
            ("previewUrl", json!("https://localhost/audio")),
            ("trackTimeMillis", json!(0)),
        ] {
            let mut changed = row.clone();
            changed[field] = value;
            assert!(
                catalog_reference(&track(), &page(vec![changed])).is_none(),
                "{field}"
            );
        }
        assert!(catalog_reference(&track(), &json!({"resultCount":2,"results":[row]})).is_none());
    }
    #[test]
    fn fallback_respects_pins_settings_versions_and_attempt_budget() {
        let original = track();
        let settings = Settings::default();
        assert!(eligible_with_policy(&original, &settings, false, 2, true));
        assert!(!eligible_with_policy(&original, &settings, true, 2, true));
        assert!(!eligible_with_policy(&original, &settings, false, 1, true));
        let disabled = Settings {
            bandcamp_fallback_enabled: false,
            ..settings.clone()
        };
        assert!(!eligible_with_policy(&original, &disabled, false, 2, true));
        let disabled = Settings {
            streaming_fallback_enabled: false,
            ..settings
        };
        assert!(!eligible_with_policy(&original, &disabled, false, 2, true));
        for changed in [
            Track {
                local_path: Some("local.mp3".into()),
                ..original.clone()
            },
            Track {
                version: Some("live".into()),
                ..original.clone()
            },
            Track {
                duration_seconds: Some(f64::NAN),
                ..original.clone()
            },
            Track {
                provenance: json!({"chapters":[{}]}),
                ..original.clone()
            },
        ] {
            assert!(!eligible_with_policy(
                &changed,
                &Settings::default(),
                false,
                2,
                true
            ));
        }
    }

    fn preview_audio() -> crate::acquisition::PreviewAudio {
        crate::acquisition::PreviewAudio {
            provider_session: None,
            url: "https://audio.example/stream".into(),
            mime_type: "audio/mpeg".into(),
            codec: Some("mp3".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(197.76),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: Some(json!({"channel":"Lucas Santtana & Arto Lindsay"})),
            resolved_title: Some(
                "Lucas Santtana & Arto Lindsay - Nighttime in the Backyard".into(),
            ),
            resolved_provider_id: Some("2065435071".into()),
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
        }
    }

    #[test]
    fn strong_proof_binding_is_source_scoped_and_shared_policy_accepts_verified_recording() {
        let track = track();
        let candidate = Candidate {
            id: "bandcamp:2065435071".into(),
            provider: "bandcamp".into(),
            url: "https://lucassanttana.bandcamp.com/track/nighttime".into(),
            title: track.title.clone(),
            ..Default::default()
        };
        let audio = preview_audio();
        let proof = json!({
            "method":"waveform-v1", "referenceId":1677838124u64,
            "referenceTitle":"Nighttime in the Backyard",
            "referenceArtist":"Lucas Santtana & Arto Lindsay", "score":0.98,
            "referenceSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sourceId":candidate.id, "sourceUrl":candidate.url,
            "resolvedTitle":audio.resolved_title, "resolvedArtist":"Lucas Santtana & Arto Lindsay",
            "duration":197.76
        });
        let binding = strong_binding_id(&proof, &candidate, &audio).expect("fresh bound proof");
        for level in [
            crate::recording_match::MatchLevel::Exact,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchLevel::Flexible,
        ] {
            let assessment = shared_policy_assessment(
                &track,
                &candidate,
                "Nighttime in the Backyard",
                "Lucas Santtana & Arto Lindsay",
                Some(197.76),
                Some(&binding),
                level,
            )
            .expect("shared policy accepts typed proof at selected level");
            assert_eq!(
                assessment.action,
                crate::recording_match::MatchAction::Accept
            );
            assert_eq!(
                assessment.dimensions.title,
                crate::recording_match::Comparison::Agree
            );
        }
        assert_eq!(track.provenance.get("recordingId"), None);

        let mut wrong_source = candidate.clone();
        wrong_source.url.push_str("-other");
        assert!(strong_binding_id(&proof, &wrong_source, &audio).is_none());
        assert!(shared_policy_assessment(
            &track,
            &candidate,
            "A Different Recording",
            "Lucas Santtana & Arto Lindsay",
            Some(197.76),
            Some(&binding),
            crate::recording_match::MatchLevel::Balanced,
        )
        .is_none());
    }
    #[tokio::test]
    async fn cancellation_and_deadline_drop_pending_work() {
        let (stop, cancel) = watch::channel(true);
        assert!(bounded(
            cancel,
            PlaybackDeadline::after(Duration::from_secs(1)),
            std::future::pending::<Result<(), String>>()
        )
        .await
        .is_err());
        drop(stop);
        let (_stop, cancel) = watch::channel(false);
        assert!(bounded(
            cancel,
            PlaybackDeadline::at(tokio::time::Instant::now()),
            std::future::pending::<Result<(), String>>()
        )
        .await
        .is_err());
    }
    fn signal(seed: u64, seconds: usize) -> Vec<f32> {
        let mut state = seed;
        // Noise plus a changing low-frequency component gives the coarse scan
        // a distinctive seed while exercising alignment at arbitrary samples.
        (0..RATE * seconds)
            .map(|i| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let noise = ((state >> 32) as u32 as f64 / u32::MAX as f64 - 0.5) * 0.2;
                let t = i as f64 / RATE as f64;
                (noise + (t * (31.0 + t * 0.1)).sin() * 0.3) as f32
            })
            .collect()
    }
    #[tokio::test]
    async fn waveform_accepts_gain_and_offset_but_rejects_unrelated_or_spliced_audio() {
        let reference = signal(41, 26);
        let mut candidate = signal(99, 2);
        candidate.extend(reference.iter().map(|v| v * 0.7));
        candidate.extend(signal(79, 2));
        let (_stop, cancel) = watch::channel(false);
        let (score, offset) = compare(
            &reference,
            &candidate,
            &cancel,
            PlaybackDeadline::after(Duration::from_secs(10)),
        )
        .unwrap();
        assert!(score > 0.999);
        assert_eq!(offset, RATE * 3);
        assert!(compare(
            &reference,
            &signal(99, 30),
            &cancel,
            PlaybackDeadline::after(Duration::from_secs(10))
        )
        .is_err());
        candidate[RATE * 15..RATE * 21].copy_from_slice(&signal(63, 6));
        assert!(compare(
            &reference,
            &candidate,
            &cancel,
            PlaybackDeadline::after(Duration::from_secs(10))
        )
        .is_err());
        assert!(compare(
            &vec![0.0; RATE * 26],
            &vec![0.0; RATE * 30],
            &cancel,
            PlaybackDeadline::after(Duration::from_secs(10))
        )
        .is_err());
    }
    #[tokio::test]
    #[ignore = "Local research PCM files, not a network or CI fixture"]
    async fn research_nighttime_waveform_comparison() {
        let read = |path: &str| {
            std::fs::read(path)
                .unwrap()
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect::<Vec<_>>()
        };
        let reference = read("/tmp/burrata-nighttime-apple-preview.mp3.pcm");
        let candidate = read("/tmp/burrata-nighttime-bandcamp.mp3.pcm");
        let (_stop, cancel) = watch::channel(false);
        let started = std::time::Instant::now();
        let (score, offset) = compare(
            &reference,
            &candidate,
            &cancel,
            PlaybackDeadline::after(Duration::from_secs(10)),
        )
        .unwrap();
        eprintln!(
            "Nighttime waveform score={score} offsetSeconds={} elapsedMs={}",
            offset as f64 / RATE as f64,
            started.elapsed().as_millis()
        );
        assert!(score > 0.99);
    }

    #[tokio::test]
    #[ignore = "Explicit public-network Bandcamp and Apple preview qualification"]
    async fn live_nighttime_verification_then_cached_foreground_playback() {
        assert_eq!(std::env::var("BURRATA_BANDCAMP_LIVE").as_deref(), Ok("1"));
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let original = track();
        store.save_track(&original).unwrap();
        let original = store.get_track(&original.id).unwrap().unwrap();
        let connections =
            std::sync::Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = AcquisitionManager::new_for_diagnostics(store.clone(), connections);
        let settings = Settings {
            ffmpeg_path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources/tools")
                .join(crate::tools::target_triple())
                .join(if cfg!(windows) {
                    "ffmpeg.exe"
                } else {
                    "ffmpeg"
                })
                .to_string_lossy()
                .into_owned(),
            ..Settings::default()
        };
        let (_stop, cancel) = watch::channel(false);
        let trace = SourceTrace::new(None, None, None);
        let first = resolve(
            &store,
            &acquisition,
            &original,
            &settings,
            cancel.clone(),
            trace.clone(),
            PlaybackDeadline::after(Duration::from_secs(20)),
            &[],
            false,
        )
        .await;
        let metadata_first_play_ready = first.is_ok();
        if metadata_first_play_ready {
            assert_eq!(
                store
                    .get_provider_cache(CACHE, &fingerprint(&original))
                    .unwrap()
                    .unwrap()["method"],
                "album-crosswalk-v1"
            );
        }
        let started = std::time::Instant::now();
        let verified = resolve(
            &store,
            &acquisition,
            &original,
            &settings,
            cancel.clone(),
            trace.clone(),
            PlaybackDeadline::after(Duration::from_secs(45)),
            &[],
            true,
        )
        .await
        .unwrap();
        let verification_ms = started.elapsed().as_millis();
        assert_eq!(verified.candidate.unwrap().id, "bandcamp:2065435071");
        let proof = store
            .get_provider_cache(CACHE, &fingerprint(&original))
            .unwrap()
            .unwrap();
        assert_eq!(proof["method"], "album-crosswalk-v1");
        assert_eq!(proof["albumTrackCount"], 12);
        assert!(!proof.to_string().contains("mp3-128"));
        assert!(!proof.to_string().contains("previewUrl"));
        let started = std::time::Instant::now();
        let deadline = PlaybackDeadline::after(Duration::from_secs(20));
        let source = resolve(
            &store,
            &acquisition,
            &original,
            &settings,
            cancel.clone(),
            trace.clone(),
            deadline,
            &[],
            false,
        )
        .await
        .unwrap();
        let resolution_ms = started.elapsed().as_millis();
        assert!((source.duration.unwrap() - 197.76).abs() < 0.01);
        let candidate = source.candidate.clone().unwrap();
        let pcm = crate::native_output::direct_pcm_experiment::collect_until(
            source.source,
            0.0,
            5.0,
            deadline.instant().into_std(),
            cancel.clone(),
        )
        .await;
        let pcm = serde_json::to_value(pcm).unwrap();
        assert_eq!(pcm["ok"], true, "{pcm}");
        assert!(pcm["decodedFrames"].as_u64().unwrap() >= 240000);
        refresh(
            &store,
            &acquisition,
            &original,
            &candidate,
            &settings,
            cancel,
            trace,
            PlaybackDeadline::after(Duration::from_secs(20)),
        )
        .await
        .unwrap();
        assert_eq!(store.get_track(&original.id).unwrap().unwrap(), original);
        eprintln!(
            "{}",
            json!({"case":"nighttime-bandcamp","verificationMs":verification_ms,"foregroundResolutionMs":resolution_ms,"method":proof["method"],"albumTrackCount":proof["albumTrackCount"],"metadataFirstPlayReady":metadata_first_play_ready,"decodedFrames":pcm["decodedFrames"],"firstPcmAfterResolutionMs":pcm["firstPcmMs"],"exactRefresh":true,"savedTagsUnchanged":true})
        );
    }
}
