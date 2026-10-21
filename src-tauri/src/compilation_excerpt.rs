//! Background proof and finite-asset cache for clean soundtrack compilations.
//!
//! This path is deliberately Flexible-only: it proves a bounded interval inside
//! a full album upload, then keeps the full source identity and excerpt identity
//! separate in provenance.

use crate::{
    acquisition::AcquisitionManager,
    models::{Candidate, Settings, Track},
    playback_sources::PlaybackDeadline,
    storage::Store,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::watch;
#[cfg(not(test))]
use tokio::sync::Semaphore;

const CACHE: &str = "playback.automatic-album-excerpt.v1";
const PROOF_SCHEMA: u32 = 1;
const MAX_BACKGROUND_TIME: Duration = Duration::from_secs(150);
const MAX_SOURCE_DURATION: f64 = 3000.0;
const MAX_ALBUM_DURATION_DELTA: f64 = 2.0;
const MAX_BOUNDARY_UNCERTAINTY: f64 = 2.0;

pub(crate) fn request_fingerprint(
    track: &Track,
    level: crate::recording_match::MatchLevel,
) -> String {
    let identity = json!({
        "id": track.id,
        "provider": track.provider,
        "providerId": track.provider_id,
        "sourceUrl": track.source_url,
        "title": track.title,
        "artist": track.artist,
        "album": track.album,
        "albumArtist": track.album_artist,
        "version": track.version,
        "duration": track.duration_seconds,
        "isrc": track.isrc,
        "artistCredits": track.provenance.get("artistCredits").cloned().unwrap_or(Value::Null),
        "artistCreditIds": track.provenance.get("artistCreditIds").cloned().unwrap_or(Value::Null),
        "recordingIdentity": track.provenance.get("identity").cloned().unwrap_or(Value::Null),
        "level": level,
        "evaluator": crate::recording_match::EVALUATOR_VERSION,
    });
    format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
}

pub(crate) fn asset_proof_is_eligible(
    track: &Track,
    value: &Value,
    level: crate::recording_match::MatchLevel,
) -> bool {
    let Some(proof) = value.get("automaticExcerpt") else {
        return true;
    };
    let source_id = proof["source"]["id"].as_str().unwrap_or_default();
    let source_url = proof["source"]["url"].as_str().unwrap_or_default();
    let source_video_id = source_id.strip_prefix("youtube:").unwrap_or_default();
    let source_url_matches_id = reqwest::Url::parse(source_url).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("www.youtube.com")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/watch"
            && url.query_pairs().count() == 1
            && url
                .query_pairs()
                .any(|(key, value)| key == "v" && source_video_id == value)
            && source_video_id.len() == 11
            && source_video_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });
    let hash = |name: &str| {
        proof[name].as_str().is_some_and(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    };
    let thirds_valid = |value: &Value| {
        value.as_array().is_some_and(|values| {
            values.len() == 3
                && values.iter().all(|score| {
                    score
                        .as_f64()
                        .is_some_and(|score| score >= 0.94 && score <= 1.0)
                })
        })
    };
    let score_valid = |value: &Value| {
        value
            .as_f64()
            .is_some_and(|score| score >= 0.96 && score <= 1.0)
    };
    let source_duration = proof["source"]["durationSeconds"]
        .as_f64()
        .unwrap_or_default();
    let catalog_album = proof["source"]["catalogAlbum"].as_str().unwrap_or_default();
    let expected_album_artist = track
        .album_artist
        .as_deref()
        .filter(|artist| !artist.trim().is_empty())
        .unwrap_or(&track.artist);
    // The album compilation may not report performer metadata at all. The
    // catalog target artist is established by the full-album catalog timeline
    // and waveform anchors; any reported provider credits remain separately
    // recorded and are only used to reject a contradiction.
    let reported_credits_are_consistent = ["reportedAlbumArtist", "reportedArtist"]
        .into_iter()
        .all(|field| {
            proof["source"][field].is_null()
                || proof["source"][field].as_str().is_some_and(|value| {
                    !value.trim().is_empty()
                        && value.len() <= 512
                        && normalize_credit(value) == normalize_credit(expected_album_artist)
                })
        });
    let source_title_is_clean_full_album = proof["source"]["title"]
        .as_str()
        .is_some_and(has_complete_soundtrack_caption);
    let album_title_matches = proof["source"]["sourceAlbumTitle"]
        .as_str()
        .is_some_and(|value| album_captions_match(value, catalog_album))
        || proof["source"]["title"]
            .as_str()
            .is_some_and(|value| album_captions_match(value, catalog_album));
    level == crate::recording_match::MatchLevel::Flexible
        && proof["schemaVersion"].as_u64() == Some(PROOF_SCHEMA as u64)
        && proof["policy"].as_str() == Some("flexible")
        && proof["evaluatorVersion"].as_str() == Some(crate::recording_match::EVALUATOR_VERSION)
        && proof["requestFingerprint"].as_str() == Some(request_fingerprint(track, level).as_str())
        && proof["approximate"].as_bool() == Some(true)
        && proof["relationship"].as_str() == Some("probable")
        && proof["source"]["provider"].as_str() == Some("youtube")
        && source_id.starts_with("youtube:")
        && source_url_matches_id
        && proof["source"]["title"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty() && value.len() <= 512)
        && proof["catalogTrackArtist"].as_str().is_some_and(|value| {
            !value.trim().is_empty()
                && value.len() <= 512
                && normalize_credit(value) == normalize_credit(&track.artist)
        })
        && proof["source"]["catalogAlbum"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty() && value.len() <= 512)
        && proof["source"]["sourceAlbumTitle"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty() && value.len() <= 512)
        && proof["source"]["albumMetadataTitle"]
            .as_str()
            .is_none_or(|value| value.len() <= 512)
        && reported_credits_are_consistent
        && album_title_matches
        && source_title_is_clean_full_album
        && hash("sourceSha256")
        && hash("albumFingerprint")
        && source_duration.is_finite()
        && (60.0..=MAX_SOURCE_DURATION).contains(&source_duration)
        && proof["albumDurationSeconds"].as_f64().is_some_and(|v| {
            v.is_finite()
                && (v - source_duration).abs() <= MAX_ALBUM_DURATION_DELTA
                && (60.0..=MAX_SOURCE_DURATION).contains(&v)
        })
        && proof["startSeconds"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0 && v < source_duration)
        && proof["endSeconds"]
            .as_f64()
            .zip(proof["startSeconds"].as_f64())
            .is_some_and(|(end, start)| {
                end.is_finite()
                    && end > start
                    && end - start <= 600.0
                    && end <= source_duration + 2.0
            })
        && track.duration_seconds.is_some_and(|duration| {
            let excerpt_duration = proof["endSeconds"].as_f64().unwrap_or_default()
                - proof["startSeconds"].as_f64().unwrap_or_default();
            duration.is_finite() && duration > 0.0 && (excerpt_duration - duration).abs() <= 4.1
        })
        && proof["boundaryUncertaintySeconds"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && (0.0..=MAX_BOUNDARY_UNCERTAINTY).contains(&v))
        && score_valid(&proof["targetScore"])
        && thirds_valid(&proof["targetThirdScores"])
        && proof["anchors"].as_array().is_some_and(|anchors| {
            let mut positions = anchors
                .iter()
                .filter_map(|anchor| {
                    anchor["sourceStartSeconds"].as_f64().map(|value| {
                        value / proof["source"]["durationSeconds"].as_f64().unwrap_or(1.0)
                    })
                })
                .collect::<Vec<_>>();
            positions.sort_by(f64::total_cmp);
            let anchor_ids = anchors
                .iter()
                .filter_map(|anchor| anchor["trackId"].as_u64())
                .collect::<std::collections::BTreeSet<_>>();
            anchors.len() >= 3
                && anchor_ids.len() >= 3
                && positions.len() >= 3
                && positions.iter().any(|value| *value <= 0.20)
                && positions.iter().any(|value| (0.30..=0.65).contains(value))
                && positions
                    .iter()
                    .any(|value| *value >= 0.65 && *value <= 0.90)
                && anchors.iter().all(|anchor| {
                    score_valid(&anchor["score"])
                        && thirds_valid(&anchor["thirdScores"])
                        && anchor["sourceStartSeconds"]
                            .as_f64()
                            .is_some_and(|v| v.is_finite() && v >= 0.0 && v < source_duration)
                })
        })
        && proof["startBoundary"]["confident"].as_bool() == Some(true)
        && proof["endBoundary"]["confident"].as_bool() == Some(true)
        && proof["targetTrackId"]
            .as_u64()
            .is_some_and(|value| value > 0)
        && proof["catalogAssessment"]["action"].as_str() == Some("accept")
}

pub(crate) fn asset_file_hash_matches(asset: &crate::models::LocalAsset) -> bool {
    if !asset.track.provenance["automaticExcerpt"].is_object() {
        return true;
    }
    const MAX_VERIFIED_EXCERPT_BYTES: u64 = 50 * 1024 * 1024;
    let path = std::path::Path::new(&asset.path);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_VERIFIED_EXCERPT_BYTES
    {
        return false;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(mut file) = options.open(path) else {
        return false;
    };
    use std::io::Read;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    format!("{:x}", digest.finalize()) == asset.hash
}

/// `finalize_download` records the output file's duration on both the asset
/// and its embedded track. Excerpts must keep the catalog request's duration
/// on the embedded track so the immutable request fingerprint still matches;
/// the actual cut length remains available on `LocalAsset.duration_seconds`.
pub(crate) fn restore_requested_track_duration_for_excerpt_asset(
    asset: &mut crate::models::LocalAsset,
    requested: &Track,
) {
    asset.track.duration_seconds = requested.duration_seconds;
}

#[cfg(test)]
pub(crate) fn test_eligible_excerpt_proof(track: &Track) -> Value {
    json!({"automaticExcerpt":{
        "schemaVersion":PROOF_SCHEMA,"policy":"flexible","evaluatorVersion":crate::recording_match::EVALUATOR_VERSION,
        "requestFingerprint":request_fingerprint(track,crate::recording_match::MatchLevel::Flexible),
        "approximate":true,"relationship":"probable","sourceSha256":"a".repeat(64),
        "albumFingerprint":"b".repeat(64),"albumDurationSeconds":500.0,
        "catalogTrackArtist":track.artist,"source":{"provider":"youtube","id":"youtube:abcdefghijk","url":"https://www.youtube.com/watch?v=abcdefghijk","title":"Fixture Album full soundtrack","reportedAlbumArtist":track.album_artist.as_deref().unwrap_or(&track.artist),"reportedArtist":track.artist,"durationSeconds":500.5,"catalogAlbum":"Fixture Album","sourceAlbumTitle":"Fixture Album","albumMetadataTitle":"Fixture Album"},
        "startSeconds":100.0,"endSeconds":280.0,"boundaryUncertaintySeconds":0.4,
        "targetScore":0.98,"targetThirdScores":[0.97,0.98,0.97],
        "anchors":[
            {"trackId":101,"sourceStartSeconds":35.0,"score":0.98,"thirdScores":[0.97,0.98,0.97]},
            {"trackId":102,"sourceStartSeconds":240.0,"score":0.98,"thirdScores":[0.97,0.98,0.97]},
            {"trackId":103,"sourceStartSeconds":390.0,"score":0.98,"thirdScores":[0.97,0.98,0.97]}],
        "startBoundary":{"confident":true},"endBoundary":{"confident":true},
        "targetTrackId":42,"catalogAssessment":{"action":"accept"}
    }})
}

#[derive(Default)]
struct WorkerGate {
    active: Option<watch::Sender<bool>>,
    last_foreground: Option<Instant>,
}
fn worker_gate() -> &'static Mutex<WorkerGate> {
    static GATE: OnceLock<Mutex<WorkerGate>> = OnceLock::new();
    GATE.get_or_init(Default::default)
}
fn pending_keys() -> &'static Mutex<HashSet<String>> {
    static KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    KEYS.get_or_init(Default::default)
}
struct PendingKey(String);
impl Drop for PendingKey {
    fn drop(&mut self) {
        if let Ok(mut keys) = pending_keys().lock() {
            keys.remove(&self.0);
        }
    }
}

pub(crate) fn yield_to_playback() {
    if let Ok(mut gate) = worker_gate().lock() {
        gate.last_foreground = Some(Instant::now());
        if let Some(stop) = &gate.active {
            stop.send_replace(true);
        }
    }
}

fn eligible(track: &Track, settings: &Settings) -> bool {
    settings.matching_level == crate::recording_match::MatchLevel::Flexible
        && settings.background_source_preparation
        && settings.streaming_fallback_enabled
        && track.local_path.is_none()
        && track.asset_id.is_none()
        && track.playback_range.is_none()
        && !matches!(
            track.provider.as_str(),
            "radio" | "youtube" | "soundcloud" | "local"
        )
        && track.provenance["sourceKind"].as_str() != Some("audioSearch")
        && track.provenance["userSelectedSource"].as_bool() != Some(true)
        && track.provenance["chapters"]
            .as_array()
            .is_none_or(Vec::is_empty)
        && track
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && (25.0..=600.0).contains(&duration))
        && !track.title.trim().is_empty()
        && !track.artist.trim().is_empty()
}

/// Schedule one bounded background attempt after ordinary automatic playback
/// has failed. Any new foreground request cancels the active worker.
pub(crate) fn schedule(acquisition: Arc<AcquisitionManager>, track: Track, settings: Settings) {
    // Unit tests must never leave detached work that can make public catalog or
    // provider requests after the test that triggered resolution has ended.
    #[cfg(test)]
    {
        let _ = (acquisition, track, settings);
        return;
    }
    #[cfg(not(test))]
    {
        static WORKER: OnceLock<Arc<Semaphore>> = OnceLock::new();
        static QUEUED: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let worker = WORKER.get_or_init(|| Arc::new(Semaphore::new(1))).clone();
        let queued = QUEUED.get_or_init(|| Arc::new(Semaphore::new(16))).clone();
        if !eligible(&track, &settings) {
            return;
        }
        let request = request_fingerprint(&track, settings.matching_level);
        if pending_keys()
            .lock()
            .map(|mut keys| !keys.insert(request.clone()))
            .unwrap_or(true)
        {
            return;
        }
        let Ok(queued) = queued.try_acquire_owned() else {
            if let Ok(mut keys) = pending_keys().lock() {
                keys.remove(&request);
            }
            return;
        };
        tauri::async_runtime::spawn(async move {
            let _pending = PendingKey(request.clone());
            let _queued = queued;
            tokio::time::sleep(Duration::from_secs(15)).await;
            let Ok(_worker) = worker.acquire_owned().await else {
                return;
            };
            loop {
                let wait = worker_gate()
                    .lock()
                    .ok()
                    .and_then(|gate| gate.last_foreground)
                    .map(|when| Duration::from_secs(30).saturating_sub(when.elapsed()))
                    .unwrap_or_default();
                if wait.is_zero() {
                    break;
                }
                tokio::time::sleep(wait.min(Duration::from_secs(30))).await;
            }
            let (stop_tx, mut cancel) = watch::channel(false);
            if let Ok(mut gate) = worker_gate().lock() {
                if gate
                    .last_foreground
                    .is_some_and(|when| when.elapsed() < Duration::from_secs(30))
                {
                    return;
                }
                gate.active = Some(stop_tx.clone());
            } else {
                return;
            }
            let store = acquisition.alias_store();
            let work_settings = store.settings();
            let work_track = store.get_track(&track.id);
            let result = match (work_settings, work_track) {
                (Ok(current_settings), Ok(Some(current_track)))
                    if current_settings.matching_level
                        == crate::recording_match::MatchLevel::Flexible
                        && eligible(&current_track, &current_settings)
                        && request_fingerprint(&current_track, current_settings.matching_level)
                            == request =>
                {
                    let mut failure_stage = "worker_start";
                    let mut failure_diagnostic = None;
                    tokio::time::timeout(
                        MAX_BACKGROUND_TIME,
                        verify_and_publish(
                            &acquisition,
                            &current_track,
                            &current_settings,
                            &mut cancel,
                            &mut failure_stage,
                            &mut failure_diagnostic,
                        ),
                    )
                    .await
                    .map(|result| (result, failure_stage, failure_diagnostic))
                }
                _ => {
                    if let Ok(mut gate) = worker_gate().lock() {
                        if gate
                            .active
                            .as_ref()
                            .is_some_and(|active| active.same_channel(&stop_tx))
                        {
                            gate.active = None;
                        }
                    }
                    return;
                }
            };
            if let Ok(mut gate) = worker_gate().lock() {
                if gate
                    .active
                    .as_ref()
                    .is_some_and(|active| active.same_channel(&stop_tx))
                {
                    gate.active = None;
                }
            }
            let diagnostic = match &result {
                Ok((Err(_), failure_stage, failure_diagnostic)) => Some((
                    *failure_stage,
                    match *failure_stage {
                        "catalog_album" => "catalog album could not be verified",
                        "catalog_identity" => "catalog recording did not meet policy",
                        "source_search" => "no eligible full-album source was found",
                        "source_identity" => "source metadata did not verify as a full album",
                        "source_audio" => "resolved source identity or duration changed",
                        "publication_recovery" => {
                            "pending excerpt publication could not be reconciled"
                        }
                        "source_audio_extract" => "fresh evidence format extraction failed",
                        "source_audio_format" => "fresh evidence format was not bounded AAC/M4A",
                        "source_audio_access" => "fresh evidence format failed safe access checks",
                        "source_audio_identity" => {
                            "fresh evidence format did not match the inspected source"
                        }
                        "publication_directory" => "excerpt cache directory could not be prepared",
                        "publication_source_stage" => "verified source bytes could not be staged",
                        "publication_split" => {
                            "verified interval could not be split into an excerpt"
                        }
                        "publication_finalize" => "verified excerpt media finalization failed",
                        "publication_recheck" => "request or catalog changed before publication",
                        "publication_asset_store" => "verified excerpt asset could not be stored",
                        "publication_cache_store" => {
                            "verified excerpt cache index could not be stored"
                        }
                        "finalize_validate_profile" => {
                            "media finalizer rejected its output profile"
                        }
                        "finalize_probe_source" => "excerpt source media probe failed",
                        "finalize_duration_gate" => "excerpt source duration gate failed",
                        "finalize_analysis" => "excerpt audio analysis failed",
                        "finalize_staging_directory" => {
                            "media staging directory could not be prepared"
                        }
                        "finalize_output_extension" => {
                            "excerpt codec output format was unsupported"
                        }
                        "finalize_remux" => "excerpt audio could not be staged or remuxed",
                        "finalize_artwork" => "excerpt artwork preparation failed",
                        "finalize_tag" => "excerpt metadata tagging failed",
                        "finalize_probe_output" => "finalized excerpt media probe failed",
                        "finalize_hash" => "finalized excerpt hash could not be computed",
                        "finalize_publish" => "finalized excerpt could not be published",
                        "finalize_mp3_export" => "excerpt MP3 export failed",
                        "waveform_size_limit" => {
                            "full-source bytes exceeded the album-evidence cap"
                        }
                        "waveform_range" => "full-source byte ranges failed validation",
                        "waveform_network" => "full-source or preview audio fetch failed",
                        "waveform_decode" => "full-source audio could not be decoded",
                        "waveform_match" => {
                            "distributed waveform or boundary evidence was insufficient"
                        }
                        "waveform_deadline" => "audio evidence exceeded its time budget",
                        "waveform_cancelled" => "audio evidence was cancelled",
                        "waveform_invalid_source" => {
                            "resolved audio source is not eligible for range verification"
                        }
                        "waveform_invalid_references" => {
                            "catalog preview references are incomplete"
                        }
                        "waveform_worker" => "audio evidence worker stopped unexpectedly",
                        "waveform_io" => "temporary audio evidence storage failed",
                        "waveform" => "audio anchors or excerpt boundaries did not verify",
                        "publication" => "verified excerpt could not be safely published",
                        _ => "verification did not complete",
                    },
                    failure_diagnostic.clone(),
                )),
                Err(_) => Some((
                    "timeout",
                    "background verification exceeded its time budget",
                    None,
                )),
                _ => None,
            };
            if let Some((failure_stage, failure_reason, failure_diagnostic)) = diagnostic {
                if !*cancel.borrow() {
                    // Persist only a fixed stage and fixed reason. Provider
                    // errors may contain signed URLs or other private data.
                    let cooldown = match failure_stage {
                        "source_audio_extract"
                        | "source_audio_access"
                        | "waveform_network"
                        | "waveform_deadline"
                        | "waveform_worker"
                        | "waveform_io" => 5 * 60,
                        _ => 6 * 60 * 60,
                    };
                    let _ = store.set_provider_cache(CACHE, &request, &json!({"schemaVersion":PROOF_SCHEMA,"requestFingerprint":request.clone(),"policy":"flexible","evaluatorVersion":crate::recording_match::EVALUATOR_VERSION,"status":"failed","failureStage":failure_stage,"failureReason":failure_reason,"failureDiagnostic":failure_diagnostic}), cooldown);
                }
            }
        });
    }
}

#[derive(Clone)]
struct AppleTrack {
    id: u64,
    title: String,
    artist: String,
    album: String,
    duration: f64,
    preview_url: String,
    track_number: u32,
    isrc: Option<String>,
}

async fn bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
    expected_origin: &reqwest::Url,
) -> Result<Vec<u8>, String> {
    if !response.status().is_success()
        || response.url().origin() != expected_origin.origin()
        || response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
    {
        return Err("Public catalog lookup was rejected or exceeded its size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Public catalog lookup failed")?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err("Public catalog lookup exceeded its size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn apple_json(url: reqwest::Url, cancel: watch::Receiver<bool>) -> Result<Value, String> {
    let addresses = crate::acquisition::public_addresses(&url).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .resolve_to_addrs(url.host_str().ok_or("Catalog host is missing")?, &addresses)
        .user_agent("Burrata/0.1 (public album metadata lookup)")
        .build()
        .map_err(|_| "Could not initialize public catalog lookup")?;
    let request_url = url.clone();
    let request = client.get(url).send();
    let response = tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => return Err("Cancelled".into()),
        result = request => result.map_err(|_| "Public catalog metadata is unavailable")?,
    };
    let bytes = bounded_body(response, 512 * 1024, &request_url).await?;
    serde_json::from_slice(&bytes).map_err(|_| "Public catalog metadata is invalid".into())
}

fn apple_endpoint(path: &str, params: &[(&str, String)]) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&format!("https://itunes.apple.com/{path}"))
        .map_err(|_| "Public catalog endpoint is invalid")?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(key, value)| (*key, value.as_str())));
    Ok(url)
}

fn parse_apple_track(row: &Value) -> Option<AppleTrack> {
    let id = row["trackId"].as_u64().filter(|id| *id > 0)?;
    let title = row["trackName"].as_str()?.trim();
    let artist = row["artistName"].as_str()?.trim();
    let album = row["collectionName"].as_str()?.trim();
    let duration = row["trackTimeMillis"].as_f64()? / 1000.0;
    let preview_url = row["previewUrl"].as_str()?.trim();
    let track_number = row["trackNumber"].as_u64()?.try_into().ok()?;
    if title.is_empty()
        || artist.is_empty()
        || album.is_empty()
        || preview_url.is_empty()
        || !(1.0..=600.0).contains(&duration)
        || track_number == 0
    {
        return None;
    }
    let preview = reqwest::Url::parse(preview_url).ok()?;
    let host = preview.host_str().unwrap_or_default();
    if preview.scheme() != "https"
        || !preview.username().is_empty()
        || preview.password().is_some()
        || preview.port().is_some()
        || preview.fragment().is_some()
        || !(host.ends_with(".itunes.apple.com") || host.ends_with(".mzstatic.com"))
    {
        return None;
    }
    Some(AppleTrack {
        id,
        title: title.into(),
        artist: artist.into(),
        album: album.into(),
        duration,
        preview_url: preview.to_string(),
        track_number,
        isrc: row["isrc"]
            .as_str()
            .and_then(crate::recording_identity::normalize_isrc),
    })
}

fn normalize_credit(value: &str) -> String {
    crate::source_fallback::normalize_artist_credit(value)
}

/// Reduce only ordinary soundtrack caption suffixes. Edition-bearing words
/// remain part of the identity so a remaster or live album cannot collapse to
/// the same album key as its original release.
fn album_caption_key(value: &str) -> Option<String> {
    let normalized = crate::matching::normalize(value);
    if normalized.is_empty() || normalized.len() > 512 {
        return None;
    }
    let padded = format!(" {normalized} ");
    let edition_terms = [
        " live ",
        " remix ",
        " remixed ",
        " deluxe ",
        " anniversary ",
        " remaster ",
        " remastered ",
        " re recorded ",
        " rerecorded ",
        " extended ",
        " instrumental ",
        " demo ",
        " radio edit ",
        " disc 2 ",
        " volume 2 ",
        " vol 2 ",
    ];
    if edition_terms.iter().any(|term| padded.contains(term)) {
        return Some(normalized);
    }
    let mut words = normalized.split_whitespace().collect::<Vec<_>>();
    if let Some(index) = words
        .iter()
        .rposition(|word| matches!(*word, "soundtrack" | "ost"))
    {
        let suffix = &words[index + 1..];
        if suffix.is_empty() || suffix.first() == Some(&"by") {
            words.truncate(index);
            while words.last().is_some_and(|word| {
                matches!(
                    *word,
                    "original"
                        | "official"
                        | "full"
                        | "complete"
                        | "game"
                        | "video"
                        | "motion"
                        | "picture"
                        | "music"
                        | "the"
                )
            }) {
                words.pop();
            }
        }
    }
    let key = words.join(" ");
    if key.is_empty() {
        Some(normalized)
    } else {
        Some(key)
    }
}

fn album_captions_match(left: &str, right: &str) -> bool {
    album_caption_key(left)
        .zip(album_caption_key(right))
        .is_some_and(|(left, right)| left == right)
}

fn mixed_gameplay_caption(value: &str) -> bool {
    let normalized = crate::matching::normalize(value);
    let padded = format!(" {normalized} ");
    [
        " gameplay ",
        " game play ",
        " playthrough ",
        " walkthrough ",
        " longplay ",
        " lets play ",
        " commentary ",
        " sound effects ",
        " sfx ",
    ]
    .iter()
    .any(|label| padded.contains(label))
}

fn has_complete_soundtrack_caption(value: &str) -> bool {
    let normalized = crate::matching::normalize(value);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    words
        .iter()
        .any(|word| matches!(*word, "soundtrack" | "ost"))
        && words
            .iter()
            .any(|word| matches!(*word, "full" | "complete" | "entire"))
        && !mixed_gameplay_caption(value)
}

fn apple_candidate(row: &AppleTrack) -> Candidate {
    Candidate {
        id: format!("apple_music:{}", row.id),
        provider: "apple_music".into(),
        url: String::new(),
        title: row.title.clone(),
        uploader: Some(row.artist.clone()),
        duration_seconds: Some(row.duration),
        artwork_url: None,
        chapters: None,
        formats: Vec::new(),
        permission: "public".into(),
        confidence: "high".into(),
        score: 1.0,
        reasons: vec![],
        official: true,
        source_metadata: Some(
            json!({"track":row.title,"catalogTitle":row.title,"artist":row.artist,"artists":[row.artist],"artistCount":1,"album":row.album,"duration":row.duration,"isrc":row.isrc,"sourceMetadataOrigin":"appleFullAlbum"}),
        ),
        source: None,
    }
}

async fn complete_apple_album(
    track: &Track,
    cancel: watch::Receiver<bool>,
) -> Result<(Vec<AppleTrack>, usize, String), String> {
    let search = apple_endpoint(
        "search",
        &[
            ("term", format!("{} {}", track.artist, track.title)),
            ("entity", "song".into()),
            ("limit", "25".into()),
        ],
    )?;
    // Search does not include the catalog album as a separate query parameter;
    // exact album identity is established from each full row below.
    let raw = apple_json(search, cancel.clone()).await?;
    let rows = raw["results"]
        .as_array()
        .ok_or("Public catalog returned no album candidates")?;
    if rows.len() >= 25 || raw["resultCount"].as_u64() != Some(rows.len() as u64) {
        return Err("Public catalog returned an incomplete or ambiguous search".into());
    }
    let wanted_album = track.album.as_deref();
    let matching = rows
        .iter()
        .filter(|row| {
            row["kind"] == "song"
                && wanted_album.is_none_or(|wanted| {
                    row["collectionName"]
                        .as_str()
                        .is_some_and(|album| album_captions_match(album, wanted))
                })
                && parse_apple_track(row).is_some_and(|parsed| {
                    crate::recording_match::assess(
                        track,
                        &apple_candidate(&parsed),
                        crate::recording_match::MatchLevel::Flexible,
                        crate::recording_match::MatchStage::Resolved,
                    )
                    .action
                        == crate::recording_match::MatchAction::Accept
                })
        })
        .collect::<Vec<_>>();
    let collection_ids = matching
        .iter()
        .filter_map(|row| row["collectionId"].as_u64())
        .collect::<std::collections::BTreeSet<_>>();
    if matching.is_empty() {
        return Err("No public catalog track matches this album and recording".into());
    }
    if collection_ids.len() != 1 {
        return Err("Public catalog target album is ambiguous".into());
    }
    if matching.len() != 1 {
        return Err("Public catalog target track is ambiguous".into());
    }
    let target =
        parse_apple_track(matching[0]).ok_or("Public catalog target track is incomplete")?;
    let collection_id = *collection_ids
        .iter()
        .next()
        .ok_or("Public catalog album ID is missing")?;
    let lookup = apple_endpoint(
        "lookup",
        &[("id", collection_id.to_string()), ("entity", "song".into())],
    )?;
    let album_raw = apple_json(lookup, cancel).await?;
    let album_rows = album_raw["results"]
        .as_array()
        .ok_or("Public catalog album timeline is missing")?;
    let songs = album_rows
        .iter()
        .filter(|row| row["kind"] == "song")
        .filter_map(parse_apple_track)
        .collect::<Vec<_>>();
    let declared_count = album_rows
        .iter()
        .find(|row| row["collectionId"].as_u64() == Some(collection_id))
        .and_then(|row| row["trackCount"].as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| {
            rows.iter()
                .find(|row| row["trackId"].as_u64() == Some(target.id))
                .and_then(|row| row["trackCount"].as_u64())
                .and_then(|value| usize::try_from(value).ok())
        })
        .ok_or("Public catalog album track count is missing")?;
    if !(3..=50).contains(&declared_count)
        || songs.len() != declared_count
        || !album_raw["resultCount"]
            .as_u64()
            .is_some_and(|count| count as usize == album_rows.len())
    {
        return Err("Public catalog album sequence is incomplete".into());
    }
    let mut ordered = songs;
    ordered.sort_by_key(|row| row.track_number);
    if ordered.iter().enumerate().any(|(index, row)| {
        row.track_number as usize != index + 1 || !album_captions_match(&row.album, &target.album)
    }) || !ordered.iter().any(|row| row.id == target.id)
    {
        return Err("Public catalog album order or identity is inconsistent".into());
    }
    let duration: f64 = ordered.iter().map(|row| row.duration).sum();
    if !duration.is_finite() || !(60.0..=MAX_SOURCE_DURATION).contains(&duration) {
        return Err("Public catalog album duration is outside the evidence limit".into());
    }
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(
                &ordered
                    .iter()
                    .map(|row| (&row.id, &row.title, &row.artist, &row.album, row.duration))
                    .collect::<Vec<_>>()
            )
            .map_err(|_| "Catalog album fingerprint could not be generated")?
        )
    );
    let target_index = ordered
        .iter()
        .position(|row| row.id == target.id)
        .ok_or("Catalog target left its album")?;
    Ok((ordered, target_index, fingerprint))
}

fn full_source_identity(
    candidate: &Candidate,
    album: &str,
    album_artist: &str,
    total: f64,
) -> bool {
    if candidate.provider != "youtube"
        || candidate
            .source_metadata
            .as_ref()
            .is_none_or(|m| m["sourceMetadataOrigin"] != "ytDlpFullInfo")
    {
        return false;
    }
    let metadata = candidate.source_metadata.as_ref().unwrap();
    let source_album = metadata["album"].as_str().unwrap_or_default();
    let source_title = candidate.title.as_str();
    let description = metadata["description"].as_str().unwrap_or_default();
    let album_matches =
        album_captions_match(source_album, album) || album_captions_match(source_title, album);
    let clean_complete_caption = has_complete_soundtrack_caption(source_title)
        && !mixed_gameplay_caption(source_album)
        && !mixed_gameplay_caption(source_title)
        && !mixed_gameplay_caption(description);
    let reported_album_artist = metadata["albumArtist"]
        .as_str()
        .filter(|value| !value.trim().is_empty());
    let reported_artist = metadata["artist"]
        .as_str()
        .filter(|value| !value.trim().is_empty());
    let duration = metadata["duration"].as_f64().or(candidate.duration_seconds);
    album_matches
        && clean_complete_caption
        && reported_album_artist
            .into_iter()
            .chain(reported_artist)
            .all(|value| normalize_credit(value) == normalize_credit(album_artist))
        && duration.is_some_and(|value| {
            value.is_finite()
                && (value - total).abs() <= MAX_ALBUM_DURATION_DELTA
                && (60.0..=MAX_SOURCE_DURATION).contains(&value)
        })
}

fn requested_album_artist(track: &Track) -> &str {
    track
        .album_artist
        .as_deref()
        .filter(|artist| !artist.trim().is_empty())
        .unwrap_or(&track.artist)
}

fn preview_references(
    album: &[AppleTrack],
    target_index: usize,
    total: f64,
) -> Result<
    (
        Vec<crate::album_excerpt_audio::AlbumAudioReference>,
        usize,
        Vec<usize>,
    ),
    String,
> {
    let mut starts = Vec::with_capacity(album.len());
    let mut current = 0.0;
    for row in album {
        starts.push(current);
        current += row.duration;
    }
    let anchor_indices = [0.07, 0.48, 0.78].map(|fraction| {
        let wanted = total * fraction;
        starts
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - wanted).abs().total_cmp(&(b.1 - wanted).abs()))
            .map(|(index, _)| index)
            .unwrap_or(0)
    });
    let mut selected = anchor_indices.to_vec();
    selected.push(target_index);
    selected.sort_unstable();
    selected.dedup();
    if anchor_indices
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != 3
        || starts[anchor_indices[0]] / total > 0.2
        || starts[anchor_indices[1]] / total < 0.3
        || starts[anchor_indices[1]] / total > 0.65
        || starts[anchor_indices[2]] / total < 0.65
    {
        return Err("Catalog album has no distributed waveform anchors".into());
    }
    let refs = selected
        .iter()
        .map(|&index| crate::album_excerpt_audio::AlbumAudioReference {
            catalog_track_id: album[index].id,
            preview_url: album[index].preview_url.clone(),
            expected_start_seconds: starts[index],
            duration_seconds: album[index].duration,
        })
        .collect::<Vec<_>>();
    let target_ref_index = selected
        .iter()
        .position(|index| *index == target_index)
        .ok_or("Target reference was lost")?;
    Ok((refs, target_ref_index, anchor_indices.to_vec()))
}

async fn verify_and_publish(
    acquisition: &AcquisitionManager,
    track: &Track,
    settings: &Settings,
    cancel: &mut watch::Receiver<bool>,
    failure_stage: &mut &'static str,
    failure_diagnostic: &mut Option<Value>,
) -> Result<(), String> {
    let store = acquisition.alias_store();
    let request = request_fingerprint(track, crate::recording_match::MatchLevel::Flexible);
    if !eligible(track, settings) || *cancel.borrow() {
        return Err("Ineligible or cancelled".into());
    }
    *failure_stage = "publication_recovery";
    if recover_pending_excerpt_publication(&store, track, settings, &request)? {
        return Ok(());
    }
    if cached_work_is_suppressed(&store, track, settings)? {
        return Ok(());
    }
    *failure_stage = "catalog_album";
    let (album, target_index, album_fingerprint) =
        complete_apple_album(track, cancel.clone()).await?;
    let total: f64 = album.iter().map(|row| row.duration).sum();
    let target = &album[target_index];
    *failure_stage = "catalog_identity";
    let assessment_candidate = apple_candidate(target);
    let assessment = crate::recording_match::assess(
        track,
        &assessment_candidate,
        crate::recording_match::MatchLevel::Flexible,
        crate::recording_match::MatchStage::Resolved,
    );
    if assessment.action != crate::recording_match::MatchAction::Accept
        || assessment.relationship == crate::recording_match::MatchRelationship::Alternative
        || assessment.relationship == crate::recording_match::MatchRelationship::Unresolved
    {
        return Err("Catalog identity is not accepted by the Flexible recording evaluator".into());
    }
    let mut query = track.clone();
    query.title = target.album.clone();
    query.artist = requested_album_artist(track).to_owned();
    query.album = None;
    query.duration_seconds = None;
    query.version = None;
    query.local_path = None;
    query.asset_id = None;
    query.playback_range = None;
    query.provenance = json!({});
    *failure_stage = "source_search";
    let search_deadline = PlaybackDeadline::after(Duration::from_secs(24));
    let candidates = acquisition
        .prefetch_playback_candidates_cancelled(
            &query,
            &[],
            &["youtube".into()],
            settings,
            cancel.clone(),
            search_deadline,
        )
        .await?;
    let mut candidate = None;
    for listed in candidates
        .into_iter()
        .filter(|candidate| candidate.provider == "youtube")
        .take(5)
    {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let inspected = tokio::time::timeout(
            Duration::from_secs(25),
            crate::acquisition::inspect_candidate_cancelled(
                &query,
                &listed,
                settings,
                cancel.clone(),
            ),
        )
        .await;
        let Ok(Ok(inspected)) = inspected else {
            continue;
        };
        *failure_stage = "source_identity";
        if !full_source_identity(
            &inspected,
            &target.album,
            requested_album_artist(track),
            total,
        ) {
            continue;
        }
        candidate = Some(inspected);
        break;
    }
    let mut candidate =
        candidate.ok_or("No fully identified YouTube album source passed metadata checks")?;
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    *failure_stage = "source_audio_extract";
    let mut evidence_audio = None;
    for attempt in 0..2 {
        *failure_stage = "source_audio_extract";
        *failure_diagnostic = None;
        match acquisition
            .resolve_album_evidence_audio_fresh_cancelled(
                &candidate,
                settings,
                cancel.clone(),
                failure_stage,
                failure_diagnostic,
            )
            .await
        {
            Ok(audio) => {
                evidence_audio = Some(audio);
                break;
            }
            Err(error) => {
                let retryable = matches!(
                    *failure_stage,
                    "source_audio_extract" | "source_audio_access"
                );
                if attempt == 0 && retryable && !*cancel.borrow() {
                    continue;
                }
                return Err(error);
            }
        }
    }
    let audio = evidence_audio.ok_or("Fresh evidence audio did not resolve")?;
    if audio.resolved_provider_id.as_deref() != candidate.id.split_once(':').map(|(_, id)| id)
        || !audio
            .resolved_title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty())
        || audio
            .duration_seconds
            .zip(Some(total))
            .is_none_or(|(duration, album)| (duration - album).abs() > MAX_ALBUM_DURATION_DELTA)
    {
        *failure_stage = "source_audio_identity";
        *failure_diagnostic = Some(crate::acquisition::album_evidence_audio_diagnostic(
            "identity_rejected",
            &audio,
        ));
        return Err("Resolved full album source changed its identity or duration".into());
    }
    *failure_stage = "waveform";
    let (references, target_ref, anchor_indices) = preview_references(&album, target_index, total)?;
    let proof = crate::album_excerpt_audio::verify_album_excerpt(
        &audio.url,
        &references,
        target_ref,
        total,
        &settings.ffmpeg_path,
        cancel.clone(),
    )
    .await
    .map_err(|error| {
        *failure_stage = match &error {
            crate::album_excerpt_audio::AlbumExcerptAudioError::Cancelled => "waveform_cancelled",
            crate::album_excerpt_audio::AlbumExcerptAudioError::Deadline => "waveform_deadline",
            crate::album_excerpt_audio::AlbumExcerptAudioError::Network => "waveform_network",
            crate::album_excerpt_audio::AlbumExcerptAudioError::RangeResponse => "waveform_range",
            crate::album_excerpt_audio::AlbumExcerptAudioError::SizeLimit => "waveform_size_limit",
            crate::album_excerpt_audio::AlbumExcerptAudioError::Decode => "waveform_decode",
            crate::album_excerpt_audio::AlbumExcerptAudioError::InsufficientMatch => {
                "waveform_match"
            }
            crate::album_excerpt_audio::AlbumExcerptAudioError::InvalidSource => {
                "waveform_invalid_source"
            }
            crate::album_excerpt_audio::AlbumExcerptAudioError::InvalidReferences => {
                "waveform_invalid_references"
            }
            crate::album_excerpt_audio::AlbumExcerptAudioError::Worker => "waveform_worker",
            crate::album_excerpt_audio::AlbumExcerptAudioError::Io => "waveform_io",
        };
        error.to_string()
    })?;
    if (proof.source.duration_seconds - total).abs() > MAX_ALBUM_DURATION_DELTA
        || proof.boundary_uncertainty_seconds > MAX_BOUNDARY_UNCERTAINTY
        || !proof.start_boundary.confident
        || !proof.end_boundary.confident
        || proof.target_score < 0.96
        || proof.target_thirds.iter().any(|score| *score < 0.94)
    {
        return Err(
            "Album waveform or quiet boundary evidence did not pass the Flexible gate".into(),
        );
    }
    let mut anchors = Vec::new();
    for index in &anchor_indices {
        let row = &album[*index];
        let matched = proof
            .matches
            .iter()
            .find(|matched| matched.catalog_track_id == row.id)
            .ok_or("A distributed album anchor did not match")?;
        if matched.score < 0.96 || matched.thirds.iter().any(|score| *score < 0.94) {
            return Err("A distributed album anchor did not pass waveform thresholds".into());
        }
        anchors.push(json!({"trackId":row.id,"sourceStartSeconds":matched.source_start_seconds,"score":matched.score,"thirdScores":matched.thirds}));
    }
    let start = proof
        .start_boundary
        .cue_seconds
        .unwrap_or(proof.target_start_seconds);
    let end = proof
        .end_boundary
        .cue_seconds
        .unwrap_or(proof.target_end_seconds);
    if end <= start
        || end - start > 600.0
        || (start - proof.target_start_seconds).abs() > 2.0
        || (end - proof.target_end_seconds).abs() > 2.0
    {
        return Err("Quiet-boundary excerpt interval is not plausible".into());
    }
    let automatic_proof = json!({
        "schemaVersion":PROOF_SCHEMA,"policy":"flexible","evaluatorVersion":crate::recording_match::EVALUATOR_VERSION,
        "relationship":"probable","approximate":true,"label":"Approximate album excerpt",
        "requestFingerprint":request,"albumFingerprint":album_fingerprint,"sourceSha256":proof.source.sha256,
        "albumDurationSeconds":total,
        "catalogTrackArtist":target.artist,
        "source":{"provider":"youtube","id":candidate.id,"url":format!("https://www.youtube.com/watch?v={}",candidate.id.split_once(':').map(|(_,id)|id).unwrap_or_default()),"title":audio.resolved_title,"reportedAlbumArtist":candidate.source_metadata.as_ref().and_then(|m|m["albumArtist"].as_str()),"reportedArtist":candidate.source_metadata.as_ref().and_then(|m|m["artist"].as_str()),"durationSeconds":proof.source.duration_seconds,"audioCodec":audio.codec,"audioBitrateKbps":audio.bitrate_kbps,"selectedFormatId":audio.selected_format_id,"catalogAlbum":target.album,"sourceAlbumTitle":candidate.source_metadata.as_ref().and_then(|m|m["album"].as_str()).unwrap_or(&candidate.title),"albumMetadataTitle":candidate.source_metadata.as_ref().and_then(|m|m["album"].as_str())},
        "targetTrackId":target.id,"targetScore":proof.target_score,"targetThirdScores":proof.target_thirds,
        "anchors":anchors,"startSeconds":start,"endSeconds":end,"expectedStartSeconds":proof.target_start_seconds,
        "expectedEndSeconds":proof.target_end_seconds,"boundaryUncertaintySeconds":proof.boundary_uncertainty_seconds,
        "startBoundary":proof.start_boundary,"endBoundary":proof.end_boundary,"catalogAssessment":assessment,
    });
    *failure_stage = "publication";
    let mut artifact_track = track.clone();
    artifact_track.local_path = None;
    artifact_track.asset_id = None;
    artifact_track.local_availability = "none".into();
    artifact_track.provenance["automaticExcerpt"] = automatic_proof.clone();
    let music_dir = std::path::PathBuf::from(&settings.music_directory);
    if !music_dir.is_dir() {
        return Err("Music directory is unavailable for verified excerpt storage".into());
    }
    let excerpt_dir = music_dir.join(".burrata-excerpts");
    *failure_stage = "publication_directory";
    tokio::fs::create_dir_all(&excerpt_dir)
        .await
        .map_err(|_| "Could not create excerpt cache directory")?;
    let mut excerpt_settings = settings.clone();
    excerpt_settings.music_directory = excerpt_dir.to_string_lossy().into_owned();
    let job_id = format!("album-excerpt-{}", &request[..16]);
    let chapter = crate::models::Chapter {
        id: format!("album-excerpt:{}", target.id),
        title: target.title.clone(),
        start_seconds: start,
        end_seconds: end,
        confidence: "probable_album_waveform".into(),
        suggested_track: None,
        suggestion_confidence: None,
    };
    // The evidence fetcher deliberately uses a generic `.bin` temp name, but
    // FFmpeg chooses the copied excerpt container from the input extension.
    // Preserve the verified AAC bytes in the proof-owned temp directory with
    // an explicit M4A suffix; dropping `proof` cleans both files on every path.
    let split_source = proof
        .source
        .source_path
        .parent()
        .ok_or_else(|| {
            *failure_stage = "publication_source_stage";
            "Verified source has no temporary parent directory"
        })?
        .join("verified-source.m4a");
    *failure_stage = "publication_source_stage";
    tokio::fs::copy(&proof.source.source_path, &split_source)
        .await
        .map_err(|_| "Could not stage verified AAC source for excerpting")?;
    *failure_stage = "publication_split";
    let extracted =
        crate::media::split_chapter(&split_source, &chapter, &excerpt_settings, &job_id, 0)
            .await
            .map_err(|_| "Verified interval could not be split into an excerpt")?;
    candidate.duration_seconds = Some(end - start);
    *failure_stage = "publication_finalize";
    let published = crate::media::finalize_download_with_diagnostic(
        &extracted,
        &artifact_track,
        &candidate,
        &excerpt_settings,
        "source",
        &job_id,
        failure_stage,
    )
    .await;
    let _ = tokio::fs::remove_file(&extracted).await;
    let mut asset = published?;
    restore_requested_track_duration_for_excerpt_asset(&mut asset, track);
    asset.track.provenance["automaticExcerpt"] = automatic_proof.clone();
    // A last-moment settings or catalog edit invalidates the immutable request
    // snapshot, so never publish an asset against a changed request/policy.
    *failure_stage = "publication_recheck";
    let current_settings = store.settings()?;
    let current_track = store
        .get_track(&track.id)?
        .ok_or("Catalog track disappeared during excerpt verification")?;
    if current_settings.matching_level != crate::recording_match::MatchLevel::Flexible
        || !current_settings.background_source_preparation
        || request_fingerprint(&current_track, current_settings.matching_level) != request
        || *cancel.borrow()
    {
        let _ = tokio::fs::remove_file(&asset.path).await;
        return Err("Flexible excerpt request changed before asset publication".into());
    }
    *failure_stage = "publication_asset_store";
    if let Err(error) = store.save_flexible_excerpt_asset(&asset, &request) {
        let _ = tokio::fs::remove_file(&asset.path).await;
        return Err(error);
    }
    *failure_stage = "publication_cache_store";
    store.set_provider_cache(CACHE, &request, &json!({"schemaVersion":PROOF_SCHEMA,"requestFingerprint":request,"policy":"flexible","evaluatorVersion":crate::recording_match::EVALUATOR_VERSION,"assetId":asset.id,"assetHash":asset.hash,"automaticExcerpt":automatic_proof}), 315_576_000)?;
    let _ = crate::media::acknowledge_publication(&excerpt_settings, &job_id);
    acquisition.notify_source_ready(track, "approximateAlbumExcerpt");
    Ok(())
}

fn recover_pending_excerpt_publication(
    store: &Store,
    track: &Track,
    settings: &Settings,
    request: &str,
) -> Result<bool, String> {
    let mut excerpt_settings = settings.clone();
    excerpt_settings.music_directory = std::path::Path::new(&settings.music_directory)
        .join(".burrata-excerpts")
        .to_string_lossy()
        .into_owned();
    let expected_job_id = format!("album-excerpt-{}", &request[..16]);
    let recovered = crate::media::recover_publications(&excerpt_settings)?;
    let Some((job_id, mut asset)) = recovered
        .into_iter()
        .find(|(job_id, _)| job_id == &expected_job_id)
    else {
        return Ok(false);
    };
    // The journal is written before the caller restores the requested catalog
    // duration on the embedded track. Reconstruct that immutable request
    // field before fingerprinting; measured excerpt length stays on the asset.
    restore_requested_track_duration_for_excerpt_asset(&mut asset, track);
    let proof = &asset.track.provenance["automaticExcerpt"];
    if proof["requestFingerprint"].as_str() != Some(request)
        || request_fingerprint(&asset.track, crate::recording_match::MatchLevel::Flexible)
            != request
        || !asset_proof_is_eligible(
            track,
            &asset.track.provenance,
            crate::recording_match::MatchLevel::Flexible,
        )
    {
        return Ok(false);
    }
    let current_settings = store.settings()?;
    let current_track = store
        .get_track(&track.id)?
        .ok_or("Catalog track disappeared during excerpt recovery")?;
    if current_settings.matching_level != crate::recording_match::MatchLevel::Flexible
        || !current_settings.background_source_preparation
        || !current_settings.streaming_fallback_enabled
        || request_fingerprint(&current_track, current_settings.matching_level) != request
    {
        return Ok(false);
    }
    store.save_flexible_excerpt_asset(&asset, request)?;
    store.set_provider_cache(
        CACHE,
        request,
        &json!({
            "schemaVersion": PROOF_SCHEMA,
            "requestFingerprint": request,
            "policy": "flexible",
            "evaluatorVersion": crate::recording_match::EVALUATOR_VERSION,
            "assetId": asset.id,
            "assetHash": asset.hash,
            "automaticExcerpt": proof,
        }),
        315_576_000,
    )?;
    let _ = crate::media::acknowledge_publication(&excerpt_settings, &job_id);
    Ok(true)
}

pub(crate) fn cached_asset(
    store: &Store,
    track: &Track,
    settings: &Settings,
) -> Option<crate::models::LocalAsset> {
    if settings.matching_level != crate::recording_match::MatchLevel::Flexible {
        return None;
    }
    let request = request_fingerprint(track, settings.matching_level);
    let cached = store.get_provider_cache(CACHE, &request).ok().flatten()?;
    if cached["schemaVersion"].as_u64() != Some(PROOF_SCHEMA as u64)
        || cached["policy"].as_str() != Some("flexible")
        || cached["evaluatorVersion"].as_str() != Some(crate::recording_match::EVALUATOR_VERSION)
        || cached["requestFingerprint"].as_str() != Some(request.as_str())
    {
        return None;
    }
    let asset_id = cached["assetId"].as_str()?;
    let asset = store.get_asset_by_id(asset_id).ok().flatten()?;
    if asset.hash != cached["assetHash"].as_str()?
        || !asset_proof_is_eligible(track, &asset.track.provenance, settings.matching_level)
        || !asset_file_hash_matches(&asset)
    {
        return None;
    }
    Some(asset)
}

/// A cached failure suppresses work only for as long as the storage TTL keeps
/// it live. A positive entry suppresses work only while its proof and file are
/// still valid; missing or externally changed files become cache misses and
/// can be regenerated without deleting or modifying the existing path.
fn cached_work_is_suppressed(
    store: &Store,
    track: &Track,
    settings: &Settings,
) -> Result<bool, String> {
    let request = request_fingerprint(track, settings.matching_level);
    let Some(cached) = store.get_provider_cache(CACHE, &request)? else {
        return Ok(false);
    };
    if cached["status"].as_str() == Some("failed") {
        return Ok(true);
    }
    Ok(cached_asset(store, track, settings).is_some())
}

#[allow(dead_code)]
fn _deadline_for_background() -> PlaybackDeadline {
    PlaybackDeadline::after(MAX_BACKGROUND_TIME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundtrack_caption_descriptors_are_optional_but_editions_remain_distinct() {
        assert!(album_captions_match(
            "Neon Drive (Original Game Soundtrack)",
            "Neon Drive - Full Original Soundtrack by Artist"
        ));
        assert!(album_captions_match(
            "Neon Drive Game Soundtrack",
            "Neon Drive Original Soundtrack"
        ));
        assert!(!album_captions_match(
            "Neon Drive (Original Game Soundtrack)",
            "Neon Drive Remastered Game Soundtrack"
        ));
        assert!(!album_captions_match(
            "Neon Drive (Original Game Soundtrack)",
            "Neon Drive Live Soundtrack"
        ));
        assert!(!album_captions_match(
            "Neon Drive (Original Game Soundtrack)",
            "Neon Drive II Original Soundtrack"
        ));
        assert!(has_complete_soundtrack_caption(
            "Neon Drive Full Original Game Soundtrack"
        ));
        assert!(!has_complete_soundtrack_caption(
            "Neon Drive Full Gameplay Walkthrough Soundtrack"
        ));
    }

    #[test]
    fn excerpt_assets_are_only_visible_for_the_flexible_request_snapshot() {
        let track = Track {
            id: "catalog:1".into(),
            provider: "catalog".into(),
            provider_id: "1".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        let proof = json!({"automaticExcerpt":{
            "schemaVersion":1,"policy":"flexible","evaluatorVersion":crate::recording_match::EVALUATOR_VERSION,
            "requestFingerprint":request_fingerprint(&track, crate::recording_match::MatchLevel::Flexible),
            "approximate":true,"relationship":"probable",
            "sourceSha256":"a".repeat(64),"albumFingerprint":"b".repeat(64),"albumDurationSeconds":500.0,
            "catalogTrackArtist":"Artist","source":{"provider":"youtube","id":"youtube:abcdefghijk","url":"https://www.youtube.com/watch?v=abcdefghijk","title":"Song Album full soundtrack","reportedAlbumArtist":"Artist","reportedArtist":"Artist","durationSeconds":500.5,"catalogAlbum":"Song Album","sourceAlbumTitle":"Song Album","albumMetadataTitle":"Song Album"},
            "startSeconds":100.0,"endSeconds":280.0,"boundaryUncertaintySeconds":0.4,
            "targetScore":0.98,"targetThirdScores":[0.97,0.98,0.97],
            "anchors":[
                {"trackId":101,"sourceStartSeconds":35.0,"score":0.98,"thirdScores":[0.97,0.98,0.97]},
                {"trackId":102,"sourceStartSeconds":240.0,"score":0.98,"thirdScores":[0.97,0.98,0.97]},
                {"trackId":103,"sourceStartSeconds":390.0,"score":0.98,"thirdScores":[0.97,0.98,0.97]}],
            "startBoundary":{"confident":true},"endBoundary":{"confident":true},
            "targetTrackId":42,"catalogAssessment":{"action":"accept"}
        }});
        assert!(asset_proof_is_eligible(
            &track,
            &proof,
            crate::recording_match::MatchLevel::Flexible
        ));
        // Full compilation metadata often omits performer fields. The catalog
        // target plus distributed audio anchors establishes the track artist;
        // absent source credits are unknown, not a contradiction.
        let mut unknown_provider_artist = proof.clone();
        unknown_provider_artist["automaticExcerpt"]["source"]["reportedAlbumArtist"] = Value::Null;
        unknown_provider_artist["automaticExcerpt"]["source"]["reportedArtist"] = Value::Null;
        assert!(asset_proof_is_eligible(
            &track,
            &unknown_provider_artist,
            crate::recording_match::MatchLevel::Flexible
        ));
        assert!(!asset_proof_is_eligible(
            &track,
            &proof,
            crate::recording_match::MatchLevel::Exact
        ));
        let mut changed = track.clone();
        changed.title = "Another Song".into();
        assert!(!asset_proof_is_eligible(
            &changed,
            &proof,
            crate::recording_match::MatchLevel::Flexible
        ));
        assert!(asset_proof_is_eligible(
            &track,
            &json!({}),
            crate::recording_match::MatchLevel::Exact
        ));
        let mut duplicated_anchors = proof.clone();
        duplicated_anchors["automaticExcerpt"]["anchors"][2]["trackId"] = json!(102);
        assert!(!asset_proof_is_eligible(
            &track,
            &duplicated_anchors,
            crate::recording_match::MatchLevel::Flexible
        ));
        let mut wrong_artist = proof.clone();
        wrong_artist["automaticExcerpt"]["source"]["reportedArtist"] = json!("Different artist");
        assert!(!asset_proof_is_eligible(
            &track,
            &wrong_artist,
            crate::recording_match::MatchLevel::Flexible
        ));
        let mut mixed_caption = proof.clone();
        mixed_caption["automaticExcerpt"]["source"]["title"] =
            json!("Song Album Full Gameplay Walkthrough Soundtrack");
        assert!(!asset_proof_is_eligible(
            &track,
            &mixed_caption,
            crate::recording_match::MatchLevel::Flexible
        ));
        let mut long_excerpt = proof.clone();
        long_excerpt["automaticExcerpt"]["endSeconds"] = json!(400.0);
        assert!(!asset_proof_is_eligible(
            &track,
            &long_excerpt,
            crate::recording_match::MatchLevel::Flexible
        ));
    }

    #[test]
    fn invalid_positive_excerpt_cache_is_regenerable_and_never_deletes_user_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let track = Track {
            id: "catalog:cache-recovery".into(),
            provider: "catalog".into(),
            provider_id: "cache-recovery".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Fixture Album".into()),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let mut settings = store.settings().unwrap();
        settings.music_directory = directory.path().to_string_lossy().into_owned();
        settings.matching_level = crate::recording_match::MatchLevel::Flexible;
        settings.background_source_preparation = true;
        settings.streaming_fallback_enabled = true;
        store.save_settings(&settings).unwrap();
        let request = request_fingerprint(&track, settings.matching_level);
        let cache_payload = |asset_id: &str, hash: &str| {
            json!({
                "schemaVersion":PROOF_SCHEMA,
                "policy":"flexible",
                "evaluatorVersion":crate::recording_match::EVALUATOR_VERSION,
                "requestFingerprint":request,
                "assetId":asset_id,
                "assetHash":hash,
                "automaticExcerpt":test_eligible_excerpt_proof(&track)["automaticExcerpt"].clone()
            })
        };

        store
            .set_provider_cache(
                CACHE,
                &request,
                &cache_payload("missing-asset", &"a".repeat(64)),
                600,
            )
            .unwrap();
        assert!(!cached_work_is_suppressed(&store, &track, &settings).unwrap());

        let path = directory.path().join("externally-changed-excerpt.m4a");
        std::fs::write(&path, b"original verified bytes").unwrap();
        let hash = crate::media::hash_file(&path).unwrap();
        let mut asset_track = track.clone();
        asset_track.provenance["automaticExcerpt"] =
            test_eligible_excerpt_proof(&track)["automaticExcerpt"].clone();
        store
            .save_asset(&crate::models::LocalAsset {
                id: "existing-excerpt".into(),
                recording_id: track.id.clone(),
                path: path.to_string_lossy().into_owned(),
                hash: hash.clone(),
                source_codec: "aac".into(),
                final_codec: "aac".into(),
                source_bitrate: Some(48.0),
                final_bitrate: Some(48.0),
                sample_rate: Some(48_000),
                channels: Some(2),
                bit_depth: None,
                duration_seconds: Some(180.0),
                size_bytes: Some(23),
                bitrate_origin: Some("format".into()),
                processing: "verified excerpt".into(),
                track: asset_track,
            })
            .unwrap();
        store
            .set_provider_cache(
                CACHE,
                &request,
                &cache_payload("existing-excerpt", &hash),
                600,
            )
            .unwrap();
        assert!(cached_work_is_suppressed(&store, &track, &settings).unwrap());

        std::fs::write(&path, b"externally modified user bytes").unwrap();
        store
            .set_provider_cache(
                CACHE,
                &request,
                &cache_payload("existing-excerpt", &hash),
                600,
            )
            .unwrap();
        assert!(!cached_work_is_suppressed(&store, &track, &settings).unwrap());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"externally modified user bytes"
        );

        store
            .set_provider_cache(
                CACHE,
                &request,
                &json!({"status":"failed","failureStage":"waveform_network"}),
                300,
            )
            .unwrap();
        assert!(cached_work_is_suppressed(&store, &track, &settings).unwrap());
    }

    #[test]
    fn pending_excerpt_publication_is_committed_and_acknowledged_on_retry() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let track = Track {
            id: "catalog:pending-excerpt".into(),
            provider: "catalog".into(),
            provider_id: "pending-excerpt".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Fixture Album".into()),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let mut settings = store.settings().unwrap();
        settings.music_directory = directory.path().to_string_lossy().into_owned();
        settings.matching_level = crate::recording_match::MatchLevel::Flexible;
        settings.background_source_preparation = true;
        settings.streaming_fallback_enabled = true;
        store.save_settings(&settings).unwrap();
        let request = request_fingerprint(&track, settings.matching_level);
        let proof = test_eligible_excerpt_proof(&track);

        let excerpt_root = directory.path().join(".burrata-excerpts");
        std::fs::create_dir_all(&excerpt_root).unwrap();
        let mut excerpt_settings = settings.clone();
        excerpt_settings.music_directory = excerpt_root.to_string_lossy().into_owned();
        let staging = excerpt_root
            .join(".burrata-staging")
            .join(format!("album-excerpt-{}", &request[..16]));
        let staged_file = staging.join("excerpt.m4a");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(&staged_file, b"verified excerpt bytes").unwrap();
        let mut asset_track = track.clone();
        // Simulate a crash after finalization but before the caller restores
        // the catalog duration on the embedded track.
        asset_track.duration_seconds = Some(179.4);
        asset_track.provenance["automaticExcerpt"] = proof["automaticExcerpt"].clone();
        let mut asset = crate::models::LocalAsset {
            id: "asset-pending-excerpt".into(),
            recording_id: track.id.clone(),
            path: String::new(),
            hash: crate::media::hash_file(&staged_file).unwrap(),
            source_codec: "aac".into(),
            final_codec: "aac".into(),
            source_bitrate: Some(48.0),
            final_bitrate: Some(48.0),
            sample_rate: Some(44_100),
            channels: Some(2),
            bit_depth: None,
            duration_seconds: Some(179.4),
            size_bytes: Some(23),
            bitrate_origin: Some("format".into()),
            processing: "verified excerpt".into(),
            track: asset_track,
        };
        let job_id = format!("album-excerpt-{}", &request[..16]);
        crate::media::journaled_publish(
            &staged_file,
            &excerpt_settings,
            &job_id,
            "Artist - Song",
            "m4a",
            &mut asset,
            None,
        )
        .unwrap();
        let journal = excerpt_root
            .join(".burrata-staging")
            .join(&job_id)
            .join("publication.json");
        assert!(journal.is_file());

        let (_, mut journaled_asset) = crate::media::recover_publications(&excerpt_settings)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        restore_requested_track_duration_for_excerpt_asset(&mut journaled_asset, &track);
        assert_eq!(
            journaled_asset.track.provenance["automaticExcerpt"]["requestFingerprint"],
            request
        );
        assert_eq!(
            request_fingerprint(
                &journaled_asset.track,
                crate::recording_match::MatchLevel::Flexible
            ),
            request
        );
        assert!(asset_proof_is_eligible(
            &track,
            &journaled_asset.track.provenance,
            crate::recording_match::MatchLevel::Flexible
        ));

        assert!(recover_pending_excerpt_publication(&store, &track, &settings, &request).unwrap());
        assert!(
            !journal.exists(),
            "committed retry must acknowledge the journal"
        );
        assert!(cached_asset(&store, &track, &settings).is_some());
        assert_eq!(
            std::fs::read(&asset.path).unwrap(),
            b"verified excerpt bytes"
        );
        let recovered_asset = cached_asset(&store, &track, &settings).unwrap();
        assert_eq!(
            recovered_asset.track.duration_seconds,
            track.duration_seconds
        );
        assert_eq!(recovered_asset.duration_seconds, Some(179.4));
    }

    #[test]
    fn full_compilation_identity_allows_missing_artist_but_rejects_reported_conflict() {
        let candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Neon Drive - Full Original Soundtrack by Pengus".into(),
            uploader: Some("Pengus".into()),
            duration_seconds: Some(2_499.0),
            artwork_url: None,
            chapters: None,
            formats: Vec::new(),
            permission: "unknown".into(),
            confidence: "unknown".into(),
            score: 0.0,
            reasons: Vec::new(),
            official: false,
            source: None,
            source_metadata: Some(json!({
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "album":null,"artist":null,"albumArtist":null,
                "duration":2499.0,"description":""
            })),
        };
        assert!(full_source_identity(
            &candidate,
            "Neon Drive (Original Game Soundtrack)",
            "Pengus",
            2498.4
        ));

        let mut conflict = candidate;
        conflict.source_metadata = Some(json!({
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "album":null,"artist":"Different performer","albumArtist":null,
            "duration":2499.0,"description":""
        }));
        assert!(!full_source_identity(
            &conflict,
            "Neon Drive (Original Game Soundtrack)",
            "Pengus",
            2498.4
        ));
    }

    #[test]
    fn pengus_catalog_recording_is_accepted_by_generic_game_difficulty_policy() {
        let requested = Track {
            id: "musicbrainz:046f845d-a86a-4022-b9f5-2bdcf64af662".into(),
            provider: "musicbrainz".into(),
            provider_id: "046f845d-a86a-4022-b9f5-2bdcf64af662".into(),
            title: "Level 2 - Miami (Insane)".into(),
            artist: "Pengus".into(),
            album: Some("Neon Drive Game Soundtrack".into()),
            duration_seconds: Some(98.0),
            provenance: json!({"catalog":{"provider":"musicbrainz","recordingId":"046f845d-a86a-4022-b9f5-2bdcf64af662","verified":true,"artistCredit":["Pengus"],"artists":[{"id":"dd446e72-d2cb-4ca1-a4fd-4f8bdf0ca864","name":"Pengus","provider":"musicbrainz"}],"isrcs":null}}),
            ..Track::default()
        };
        let target = AppleTrack {
            id: 42,
            title: "Level 2 Miami (Insane Difficulty)".into(),
            artist: "Pengus".into(),
            album: "Neon Drive Game Soundtrack".into(),
            duration: 97.714,
            preview_url: "https://audio-ssl.itunes.apple.com/test.m4a".into(),
            track_number: 2,
            isrc: None,
        };
        assert!(album_captions_match(
            requested.album.as_deref().unwrap(),
            &target.album
        ));
        let assessment = crate::recording_match::assess(
            &requested,
            &apple_candidate(&target),
            crate::recording_match::MatchLevel::Flexible,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_eq!(
            assessment.action,
            crate::recording_match::MatchAction::Accept,
            "shared evaluator assessment on actual Apple row: {assessment:?}; candidate={:?}",
            apple_candidate(&target)
        );
        let wrong_difficulty = AppleTrack {
            title: "Level 2 Miami (Hard Difficulty)".into(),
            ..target.clone()
        };
        assert_ne!(
            crate::recording_match::assess(
                &requested,
                &apple_candidate(&wrong_difficulty),
                crate::recording_match::MatchLevel::Flexible,
                crate::recording_match::MatchStage::Resolved,
            )
            .action,
            crate::recording_match::MatchAction::Accept
        );
        let wrong_work = AppleTrack {
            title: "Level 3 Miami (Insane Difficulty)".into(),
            ..target.clone()
        };
        assert_ne!(
            crate::recording_match::assess(
                &requested,
                &apple_candidate(&wrong_work),
                crate::recording_match::MatchLevel::Flexible,
                crate::recording_match::MatchStage::Resolved,
            )
            .action,
            crate::recording_match::MatchAction::Accept
        );
    }

    #[tokio::test]
    #[ignore = "live public Apple catalog regression; run explicitly to diagnose metadata drift"]
    async fn live_pengus_catalog_album_target_and_timeline() {
        let requested = Track {
            id: "musicbrainz:046f845d-a86a-4022-b9f5-2bdcf64af662".into(),
            provider: "musicbrainz".into(),
            provider_id: "046f845d-a86a-4022-b9f5-2bdcf64af662".into(),
            title: "Level 2 - Miami (Insane)".into(),
            artist: "Pengus".into(),
            album: Some("Neon Drive Game Soundtrack".into()),
            duration_seconds: Some(98.0),
            provenance: json!({"catalog":{"provider":"musicbrainz","recordingId":"046f845d-a86a-4022-b9f5-2bdcf64af662","verified":true,"artistCredit":["Pengus"],"artists":[{"id":"dd446e72-d2cb-4ca1-a4fd-4f8bdf0ca864","name":"Pengus","provider":"musicbrainz"}],"isrcs":null}}),
            ..Track::default()
        };
        let (_stop, cancel) = watch::channel(false);
        match complete_apple_album(&requested, cancel).await {
            Ok((album, target_index, _fingerprint)) => {
                println!(
                    "Apple album verification passed: {} ordered tracks, target index {}, target ID {}, duration {:.3}s",
                    album.len(),
                    target_index,
                    album[target_index].id,
                    album[target_index].duration
                );
            }
            // The catalog helper returns fixed errors only, so this diagnostic
            // cannot expose response bodies, preview URLs, or signed URLs.
            Err(reason) => panic!("Apple catalog verification failed: {reason}"),
        }
    }
}
