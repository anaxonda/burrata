//! Subprocess acquisition stays in Rust; only explicit queue requests download media.
use crate::{connections::ConnectionManager, matching, media, models::*, Store};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::Emitter;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, watch, Semaphore, SemaphorePermit},
};
use uuid::Uuid;

// Peer queues must not consume the workers used for direct downloads or file processing.
const SOULSEEK_PENDING_LIMIT: usize = 32;
const SLSKD_COMPLETED_SEARCH_TTL: Duration = Duration::from_secs(300);
const SLSKD_COMPLETED_SEARCH_LIMIT: usize = 4;

fn trace_soulseek_album(event: &str, details: serde_json::Value) {
    if std::env::var("BURRATA_PLAYBACK_TRACE").is_ok_and(|value| value == "1") {
        eprintln!(
            "burrata_soulseek_album {}",
            serde_json::json!({"event": event, "details": details})
        );
    }
}

fn soulseek_album_query_fingerprint(query: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(query.as_bytes()))
}

fn soulseek_album_group_fingerprint(peer: &str, directory: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{peer}\0{directory}").as_bytes());
    format!("{:x}", digest)[..16].to_owned()
}

fn soulseek_album_track_input_diagnostics(
    tracks: &[Track],
    aliases: &[crate::artist_credit_aliases::VerifiedPlaybackArtistAliases],
) -> Vec<Value> {
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| {
            serde_json::json!({
                "index": index,
                "trackIdHash": soulseek_album_query_fingerprint(&track.id)[..16].to_owned(),
                "titleHash": soulseek_album_query_fingerprint(&track.title)[..16].to_owned(),
                "artistHash": soulseek_album_query_fingerprint(&track.artist)[..16].to_owned(),
                "albumHash": track.album.as_deref().map(|value| soulseek_album_query_fingerprint(value)[..16].to_owned()),
                "durationMillis": track.duration_seconds.map(|value| (value * 1000.0).round() as i64),
                "isrcPresent": track.isrc.as_deref().is_some_and(|value| !value.trim().is_empty()),
                "versionPresent": track.version.as_deref().is_some_and(|value| !value.trim().is_empty()),
                "localAssetPresent": track.asset_id.is_some() || track.local_path.is_some(),
                "sourceKind": track.provenance["sourceKind"].as_str(),
                "expectedOrdinals": soulseek_expected_album_ordinals(track),
                "aliasTrackCount": aliases.get(index).map(|context| context.tracks().len()).unwrap_or(0),
                "releaseRuntimeVariantCount": aliases.get(index).map(|context| context.release_runtime_variants().len()).unwrap_or(0),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoulseekAlbumQueryDecision {
    Cancel,
    Return,
    Continue,
}

fn soulseek_album_query_decision(
    cancelled: bool,
    deadline_reached: bool,
    has_assignments: bool,
    is_last_query: bool,
) -> SoulseekAlbumQueryDecision {
    if cancelled {
        SoulseekAlbumQueryDecision::Cancel
    } else if has_assignments || is_last_query || deadline_reached {
        SoulseekAlbumQueryDecision::Return
    } else {
        SoulseekAlbumQueryDecision::Continue
    }
}

fn download_matching_level(track: &Track) -> crate::recording_match::MatchLevel {
    track.provenance["downloadMatchingLevel"]
        .as_str()
        .and_then(|value| {
            serde_json::from_value::<crate::recording_match::MatchLevel>(Value::String(
                value.to_owned(),
            ))
            .ok()
        })
        // Jobs created before policy snapshots were introduced retain the
        // historical default regardless of the user's current settings.
        .unwrap_or(crate::recording_match::MatchLevel::Balanced)
}

fn canonical_source_url(raw: &str) -> String {
    url::Url::parse(raw)
        .ok()
        .map(|url| url.to_string())
        .unwrap_or_else(|| raw.to_owned())
}

fn explicit_download_source(track: &Track, candidate: &Candidate) -> bool {
    if track.provenance["downloadAutomaticFallback"] == true {
        return false;
    }
    let selected = &track.provenance["userSelectedSource"];
    if selected.is_object() {
        return selected["provider"].as_str() == Some(candidate.provider.as_str())
            && selected["id"].as_str() == Some(candidate.id.as_str())
            && selected["url"].as_str() == Some(canonical_source_url(&candidate.url).as_str());
    }
    track.provenance["sourceKind"].as_str() == Some("audioSearch")
        && track.provider == candidate.provider
        && track.provider_id
            == candidate
                .id
                .strip_prefix(&format!("{}:", candidate.provider))
                .unwrap_or_default()
        && canonical_source_url(&track.source_url) == canonical_source_url(&candidate.url)
}

fn snapshot_user_selected_source(track: &mut Track, candidate: &Candidate) {
    if track.provenance["userSelectedSource"] == true {
        track.provenance["userSelectedSource"] = serde_json::json!({
            "provider": candidate.provider,
            "id": candidate.id,
            "url": canonical_source_url(&candidate.url),
        });
    }
}

fn artifact_track_for_match(
    track: &Track,
    candidate: &Candidate,
    relationship: crate::recording_match::MatchRelationship,
    separate_source_identity: bool,
) -> Result<Track, String> {
    let mut artifact = track.clone();
    if let Some(provenance) = artifact.provenance.as_object_mut() {
        provenance.remove("userSelectedSource");
        provenance.remove("downloadAutomaticFallback");
    }
    if !separate_source_identity {
        artifact = promote_literal_source_metadata(&artifact, candidate).0;
        artifact = promote_matched_catalog_metadata(&artifact, candidate, relationship);
        return Ok(artifact);
    }
    let metadata = candidate.source_metadata.as_ref().unwrap_or(&Value::Null);
    let prefix = format!("{}:", candidate.provider);
    let actual_provider_id = candidate
        .id
        .strip_prefix(&prefix)
        .filter(|id| !id.is_empty())
        .or_else(|| {
            metadata["resolvedProviderId"]
                .as_str()
                .filter(|id| !id.is_empty())
        })
        .ok_or("Alternative source is missing its stable provider identity")?;
    let actual_id = format!("{}:{actual_provider_id}", candidate.provider);
    let already_actual_source = track.provider == candidate.provider
        && track.provider_id == actual_provider_id
        && canonical_source_url(&track.source_url) == canonical_source_url(&candidate.url);
    if actual_id == track.id && !already_actual_source {
        return Err("Alternative source resolves to the requested catalog identity".into());
    }
    let actual_title = metadata["catalogTitle"]
        .as_str()
        .or(metadata["track"].as_str())
        .or(metadata["title"].as_str())
        .unwrap_or(&candidate.title)
        .trim();
    if !actual_title.is_empty() && track.provenance["manualFields"]["title"] != true {
        artifact.title = actual_title.to_owned();
    }
    let actual_artist = metadata["artist"]
        .as_str()
        .or(metadata["catalogArtist"].as_str())
        .map(str::trim)
        .filter(|artist| !artist.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            metadata["artists"].as_array().and_then(|artists| {
                let names = artists
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .collect::<Vec<_>>();
                (!names.is_empty()).then(|| names.join(", "))
            })
        });
    if !already_actual_source && track.provenance["manualFields"]["artist"] != true {
        artifact.artist = actual_artist
            .map(|artist| artist.trim().to_owned())
            .unwrap_or_default();
    }
    if track.provenance["manualFields"]["version"] != true {
        artifact.version = metadata["version"]
            .as_str()
            .or(metadata["mix"].as_str())
            .map(str::to_owned)
            .or_else(|| crate::recording_identity::parse_title(actual_title).version);
    }
    if !already_actual_source {
        // A different recording must not inherit the requested catalog's
        // release identity. Keep explicit user edits, use fresh source
        // metadata when available, and clear automatic values when absent.
        if track.provenance["manualFields"]["album"] != true {
            artifact.album = metadata["album"]
                .as_str()
                .map(str::trim)
                .filter(|album| !album.is_empty())
                .map(str::to_owned);
        }
        if track.provenance["manualFields"]["albumArtist"] != true {
            artifact.album_artist = metadata["albumArtist"]
                .as_str()
                .map(str::trim)
                .filter(|artist| !artist.is_empty())
                .map(str::to_owned);
        }
        if track.provenance["manualFields"]["isrc"] != true {
            artifact.isrc = metadata["isrc"]
                .as_str()
                .and_then(crate::recording_identity::normalize_isrc);
        }
        if track.provenance["manualFields"]["releaseDate"] != true {
            artifact.release_date = None;
        }
        if track.provenance["manualFields"]["recordingDate"] != true {
            artifact.recording_date = None;
        }
        if track.provenance["manualFields"]["catalogNumber"] != true {
            artifact.catalog_number = None;
        }
        if track.provenance["manualFields"]["label"] != true {
            artifact.label = None;
        }
        if track.provenance["manualFields"]["genre"] != true {
            artifact.genre = None;
            artifact.normalized_genre = None;
        }
        if track.provenance["manualFields"]["bpm"] != true {
            artifact.bpm = None;
        }
        if track.provenance["manualFields"]["key"] != true {
            artifact.key = None;
        }
        if track.provenance["manualFields"]["remixers"] != true {
            artifact.remixers = None;
        }
        if track.provenance["manualFields"]["trackNumber"] != true {
            artifact.track_number = None;
        }
        if track.provenance["manualFields"]["trackTotal"] != true {
            artifact.track_total = None;
        }
        if track.provenance["manualFields"]["discNumber"] != true {
            artifact.disc_number = None;
        }
        if track.provenance["manualFields"]["discTotal"] != true {
            artifact.disc_total = None;
        }
        // The old cover belongs to the requested recording. Use the selected
        // source thumbnail when available, or leave artwork unset.
        if track.provenance["manualFields"]["artwork"] != true {
            artifact.artwork_url = candidate.artwork_url.clone();
        }
    }
    if !already_actual_source {
        artifact.provider = candidate.provider.clone();
        artifact.provider_id = actual_provider_id.to_owned();
        artifact.id = actual_id;
        artifact.source_url = candidate.url.clone();
    }
    if !artifact.provenance.is_object() {
        artifact.provenance = serde_json::json!({});
    }
    // When an explicitly selected source is already the Track's own exact
    // identity, retain its structured extractor tags just as we do on the
    // matched-source path. The helper binds them to provider, ID, URL, and
    // resolved yt-dlp ID, and records them as unverified source reports.
    if already_actual_source {
        artifact = promote_literal_source_metadata(&artifact, candidate).0;
    }
    artifact.provenance["requestedTrackIdentity"] = serde_json::json!({
        "id": track.id,
        "originalId": track.id,
        "provider": track.provider,
        "providerId": track.provider_id,
        "title": track.title,
        "artist": track.artist,
        "version": track.version,
        "matchingLevel": track.provenance["downloadMatchingLevel"],
    });
    artifact.provenance["downloadMatchRelationship"] =
        serde_json::to_value(relationship).unwrap_or_else(|_| serde_json::json!("unresolved"));
    artifact.provenance["actualSourceIdentity"] = serde_json::json!({
        "provider": candidate.provider,
        "providerId": actual_provider_id,
        "id": artifact.id,
        "sourceUrl": candidate.url,
    });
    Ok(artifact)
}

/// Promote only missing descriptive tags from a current, accepted Monochrome
/// catalog result. Recording identity fields (title, artist, provider, ID) are
/// never rewritten here; the assessment gate and stable catalog ID bind these
/// optional values to the selected recording.
fn promote_matched_catalog_metadata(
    track: &Track,
    candidate: &Candidate,
    relationship: crate::recording_match::MatchRelationship,
) -> Track {
    use crate::recording_match::MatchRelationship::{Exact, Probable};
    if !matches!(relationship, Exact | Probable) || candidate.provider != "monochrome" {
        return track.clone();
    }
    let Some(metadata) = candidate.source_metadata.as_ref() else {
        return track.clone();
    };
    let Some(catalog_id) = candidate.id.strip_prefix("monochrome:") else {
        return track.clone();
    };
    if metadata["provider"].as_str() != Some("monochrome")
        || metadata["trackId"].as_str() != Some(catalog_id)
    {
        return track.clone();
    }

    let mut promoted = track.clone();
    let manual = &track.provenance["manualFields"];
    let mut fields = Vec::new();
    let missing = |value: Option<&str>| value.is_none_or(|value| value.trim().is_empty());

    if missing(track.album.as_deref()) && manual["album"] != true {
        if let Some(value) = bounded_source_text(&metadata["album"], 512) {
            promoted.album = Some(value);
            fields.push("album".to_owned());
        }
    }
    if missing(track.album_artist.as_deref()) && manual["albumArtist"] != true {
        if let Some(value) = bounded_source_text(&metadata["albumArtist"], 512) {
            promoted.album_artist = Some(value);
            fields.push("albumArtist".to_owned());
        }
    }
    if track.isrc.is_none() && manual["isrc"] != true {
        if let Some(value) = metadata["isrc"]
            .as_str()
            .and_then(crate::recording_identity::normalize_isrc)
        {
            promoted.isrc = Some(value);
            fields.push("isrc".to_owned());
        }
    }
    if missing(track.genre.as_deref()) && manual["genre"] != true {
        if let Some(value) = bounded_source_text(&metadata["genre"], 256) {
            promoted.genre = Some(value);
            fields.push("genre".to_owned());
        }
    }
    if missing(track.label.as_deref()) && manual["label"] != true {
        if let Some(value) = bounded_source_text(&metadata["label"], 256) {
            promoted.label = Some(value);
            fields.push("label".to_owned());
        }
    }
    if !fields.is_empty() {
        if !promoted.provenance.is_object() {
            promoted.provenance = serde_json::json!({});
        }
        promoted.provenance["acceptedSourceMetadata"] = serde_json::json!({
            "identityStatus":"accepted_recording_match",
            "provider":candidate.provider,
            "providerId":catalog_id,
            "relationship":relationship,
            "fields":fields,
        });
    }
    promoted
}

fn bounded_source_text(value: &Value, max_len: usize) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty() && text.len() <= max_len).then(|| text.to_owned())
}

fn same_source_metadata<'a>(track: &Track, candidate: &'a Candidate) -> Option<&'a Value> {
    let metadata = candidate.source_metadata.as_ref()?;
    let expected_id = format!("{}:{}", track.provider, track.provider_id);
    let metadata_origin = metadata["sourceMetadataOrigin"].as_str()?;
    (track.id == expected_id
        && candidate.provider == track.provider
        && candidate.id == expected_id
        && candidate.url == track.source_url
        && metadata["resolvedProviderId"].as_str() == Some(track.provider_id.as_str())
        && metadata_origin.starts_with("ytDlp"))
    .then_some(metadata)
}

/// Fill missing track fields only from the exact source URL/ID's own yt-dlp
/// metadata. These values remain source-reported and are never catalog proof.
fn promote_literal_source_metadata(track: &Track, candidate: &Candidate) -> (Track, Vec<String>) {
    let Some(metadata) = same_source_metadata(track, candidate) else {
        return (track.clone(), Vec::new());
    };
    let artist = bounded_source_text(&metadata["artist"], 256)
        .or_else(|| bounded_source_text(&metadata["catalogArtist"], 256))
        .or_else(|| {
            let artists = metadata["artists"].as_array()?;
            if artists.is_empty() || artists.len() > 32 {
                return None;
            }
            let names = artists
                .iter()
                .map(|artist| bounded_source_text(artist, 256))
                .collect::<Option<Vec<_>>>()?;
            Some(names.join(", "))
        });
    let title = bounded_source_text(&metadata["catalogTitle"], 512)
        .or_else(|| bounded_source_text(&metadata["track"], 512))
        .or_else(|| bounded_source_text(&metadata["title"], 512));
    let album = bounded_source_text(&metadata["album"], 512);
    let album_artist = bounded_source_text(&metadata["albumArtist"], 512);
    let version = bounded_source_text(&metadata["version"], 512)
        .or_else(|| bounded_source_text(&metadata["mix"], 512));
    let genre = bounded_source_text(&metadata["genre"], 256);
    let isrc = metadata["isrc"]
        .as_str()
        .and_then(crate::recording_identity::normalize_isrc);

    let mut promoted = track.clone();
    let mut fields = Vec::new();
    let manual = &track.provenance["manualFields"];
    if let Some(value) = artist.filter(|_| {
        manual["artist"] != true
            && (track.artist.trim().is_empty() || track.provenance["weakFields"]["artist"] == true)
    }) {
        promoted.artist = value;
        fields.push("artist".to_owned());
    }
    if let Some(value) = title.filter(|_| {
        manual["title"] != true
            && (track.title.trim().is_empty() || track.provenance["weakFields"]["title"] == true)
    }) {
        promoted.title = value;
        fields.push("title".to_owned());
    }
    if promoted
        .album
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && manual["album"] != true
    {
        if let Some(value) = album {
            promoted.album = Some(value);
            fields.push("album".to_owned());
        }
    }
    if promoted
        .album_artist
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && manual["albumArtist"] != true
    {
        if let Some(value) = album_artist {
            promoted.album_artist = Some(value);
            fields.push("albumArtist".to_owned());
        }
    }
    if promoted
        .version
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && manual["version"] != true
    {
        if let Some(value) = version {
            promoted.version = Some(value);
            fields.push("version".to_owned());
        }
    }
    if promoted
        .genre
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && manual["genre"] != true
    {
        if let Some(value) = genre {
            promoted.genre = Some(value);
            fields.push("genre".to_owned());
        }
    }
    if promoted.isrc.is_none() && manual["isrc"] != true {
        if let Some(value) = isrc {
            promoted.isrc = Some(value);
            fields.push("isrc".to_owned());
        }
    }
    if !fields.is_empty() {
        if !promoted.provenance.is_object() {
            promoted.provenance = serde_json::json!({});
        }
        promoted.provenance["sourceReportedMetadata"] = serde_json::json!({
            "identityStatus":"source_reported_unverified",
            "provider":track.provider,
            "providerId":track.provider_id,
            "sourceUrl":track.source_url,
            "origin":metadata["sourceMetadataOrigin"],
            "fields":fields.clone(),
        });
    }
    (promoted, fields)
}

/// Build an ephemeral catalog-search identity from exact-source metadata. The
/// caller must keep candidates in review when these fields supplied identity.
pub(crate) fn source_reported_track_for_lookup(track: &Track) -> (Track, Vec<String>) {
    let Some(candidate) = track
        .provenance
        .get("audioCandidate")
        .cloned()
        .and_then(|value| serde_json::from_value::<Candidate>(value).ok())
    else {
        return (track.clone(), Vec::new());
    };
    source_reported_track_for_lookup_from_candidate(track, &candidate)
}

pub(crate) fn source_reported_track_for_lookup_from_candidate(
    track: &Track,
    candidate: &Candidate,
) -> (Track, Vec<String>) {
    let (mut enriched, mut fields) = promote_literal_source_metadata(track, candidate);
    if let Some(metadata) = track.provenance.get("sourceReportedMetadata") {
        if metadata["identityStatus"] == "source_reported_unverified"
            && metadata["provider"].as_str() == Some(track.provider.as_str())
            && metadata["providerId"].as_str() == Some(track.provider_id.as_str())
            && metadata["sourceUrl"].as_str() == Some(track.source_url.as_str())
        {
            if let Some(stored_fields) = metadata["fields"].as_array() {
                for field in stored_fields.iter().filter_map(Value::as_str) {
                    if !fields.iter().any(|existing| existing == field) {
                        fields.push(field.to_owned());
                    }
                }
            }
            if enriched == *track {
                enriched = track.clone();
            }
        }
    }
    (enriched, fields)
}

struct DownloadAdmission<'a> {
    processing: Option<SemaphorePermit<'a>>,
    peer: Option<SemaphorePermit<'a>>,
}

async fn cancellable_slot<'a>(
    slots: &'a Semaphore,
    cancel: &mut watch::Receiver<bool>,
) -> Result<SemaphorePermit<'a>, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    tokio::select! {
        permit = slots.acquire() => permit.map_err(|_| "Download queue closed".into()),
        _ = cancel.changed() => Err("Cancelled".into()),
    }
}

async fn download_admission<'a>(
    provider: &str,
    processing: &'a Semaphore,
    peers: &'a Semaphore,
    cancel: &mut watch::Receiver<bool>,
) -> Result<DownloadAdmission<'a>, String> {
    if provider == "slskd" {
        Ok(DownloadAdmission {
            processing: None,
            peer: Some(cancellable_slot(peers, cancel).await?),
        })
    } else {
        Ok(DownloadAdmission {
            processing: Some(cancellable_slot(processing, cancel).await?),
            peer: None,
        })
    }
}

impl<'a> DownloadAdmission<'a> {
    async fn begin_processing(
        &mut self,
        processing: &'a Semaphore,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        // slskd enforces its own transfer limit; this permit only bounded pending monitors.
        if self.peer.take().is_some() {
            self.processing = Some(cancellable_slot(processing, cancel).await?);
        }
        Ok(())
    }
}

const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const SEARCH_CACHE_MAX: usize = 128;
const RAW_AUDIO_SEARCH_CACHE_TTL: Duration = Duration::from_secs(2 * 60);
const RAW_AUDIO_SEARCH_CACHE_MAX: usize = 64;
const PREVIEW_SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
const PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT: usize = 10;
const OPTIONAL_PLAYBACK_RESEARCH_MAX: Duration = Duration::from_secs(3);
pub(crate) const PLAYBACK_EXTRACTION_RESERVE: Duration = Duration::from_secs(8);
const PREVIEW_AUDIO_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const PREVIEW_AUDIO_CACHE_MAX: usize = 64;
const PLAYBACK_EXTRACTION_PROFILE: &str = "playback-metadata-v1-managed-mweb-progressive";
const PLAYBACK_PROFILE_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const PLAYBACK_PROFILE_COOLDOWN_MAX: usize = 256;
const CHAPTER_SOURCE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const CHAPTER_SOURCE_CACHE_MAX: usize = 16;
const INSPECTION_CACHE_TTL: Duration = Duration::from_secs(90);
const INSPECTION_CACHE_MAX: usize = 128;
const INSPECTION_PROFILE: &str = "acquisition-inspect-v3-youtube-support";
const PREVIEW_AUDIO_FORMAT_SELECTOR: &str = "bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/bestaudio[protocol^=http][protocol!*=dash]/bestaudio";
const PREVIEW_AUDIO_ALTERNATE_FORMAT_SELECTOR: &str = "bestaudio[ext=webm][protocol^=http][protocol!*=dash]/bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/bestaudio[protocol^=http][protocol!*=dash]/bestaudio";
const PREVIEW_AUDIO_YOUTUBE_FORMAT_SELECTOR: &str = "bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/bestaudio[protocol^=http][protocol!*=dash]/bestaudio/worst[ext=mp4][acodec!=none][vcodec!=none][protocol^=http][protocol!*=dash]";
const PREVIEW_AUDIO_YOUTUBE_ALTERNATE_FORMAT_SELECTOR: &str = "bestaudio[ext=webm][protocol^=http][protocol!*=dash]/bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/bestaudio[protocol^=http][protocol!*=dash]/bestaudio/worst[ext=mp4][acodec!=none][vcodec!=none][protocol^=http][protocol!*=dash]";
/// Small AAC representation used only for background album waveform evidence.
/// Normal playback continues to use the higher-quality selectors above.
const ALBUM_EVIDENCE_AUDIO_SELECTOR: &str = "worstaudio[ext=m4a][protocol^=http][protocol!*=dash]";
const MAX_PREVIEW_FILE_SIZE: u64 = 16 * 1024 * 1024 * 1024;
const PREVIEW_TAIL_PROBE_THRESHOLD: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ProviderSessionStamp {
    epoch: Arc<AtomicU64>,
    expected: u64,
}
impl ProviderSessionStamp {
    pub(crate) fn is_current(&self) -> bool {
        self.epoch.load(Ordering::SeqCst) == self.expected
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAudio {
    #[serde(skip)]
    pub(crate) provider_session: Option<ProviderSessionStamp>,
    pub url: String,
    pub mime_type: String,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<f64>,
    pub duration_seconds: Option<f64>,
    /// Selected extractor format identifier (for example `140` or `251`). This is
    /// safe diagnostic metadata; the signed media URL remains private.
    #[serde(skip_serializing)]
    pub selected_format_id: Option<String>,
    /// Whether yt-dlp selected a muxed stream containing video as well as audio.
    #[serde(skip_serializing)]
    pub selected_format_has_video: bool,
    /// Small identity-only metadata from the same full yt-dlp response used to
    /// resolve the stream. Never serialize descriptions or signed stream URLs.
    #[serde(skip_serializing)]
    pub source_metadata: Option<Value>,
    /// Stable identity fields from the extractor's full metadata response. These
    /// are kept separate from search-result metadata for playback verification.
    #[serde(skip_serializing)]
    pub resolved_title: Option<String>,
    #[serde(skip_serializing)]
    pub resolved_provider_id: Option<String>,
    /// `Some(empty)` means the full metadata lookup completed and found no
    /// chapters, so playback does not need a second inspection request.
    #[serde(skip_serializing)]
    pub chapters: Option<Vec<Chapter>>,
    #[serde(skip_serializing)]
    pub expected_content_length: Option<u64>,
    #[serde(skip_serializing)]
    pub headers: Vec<(String, String)>,
}

pub(crate) fn album_evidence_audio_diagnostic(code: &'static str, audio: &PreviewAudio) -> Value {
    let safe_format_id = audio.selected_format_id.as_deref().filter(|value| {
        !value.is_empty()
            && value.len() <= 24
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    });
    let safe_codec = audio.codec.as_deref().filter(|value| {
        !value.is_empty()
            && value.len() <= 48
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    });
    serde_json::json!({
        "code": code,
        "formatId": safe_format_id,
        "codec": safe_codec,
        "durationSeconds": audio.duration_seconds.filter(|value| value.is_finite() && (0.0..=5000.0).contains(value)),
        "bitrateKbps": audio.bitrate_kbps.filter(|value| value.is_finite() && (0.0..=10000.0).contains(value)),
    })
}

fn album_evidence_access_failure_code(error: &str) -> (&'static str, Option<u16>) {
    if let Some(status) = error
        .strip_prefix("Audio source denied stream access (HTTP ")
        .and_then(|value| value.strip_suffix(')'))
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|status| (100..=599).contains(status))
    {
        return ("http_status", Some(status));
    }
    let code = match error {
        "Audio source did not honor a bounded byte-range request"
        | "Audio source returned an invalid byte range"
        | "Audio source returned an inconsistent byte range"
        | "Audio source returned a truncated byte range"
        | "Audio source returned an oversized byte range" => "range_rejected",
        "The audio source returned a web page instead of audio" => "html_response",
        "Audio stream resolution timed out" | "yt-dlp operation timed out" => "timeout",
        "Audio stream access check failed" | "Audio stream could not deliver audio bytes" => {
            "network_error"
        }
        "The audio source returned an empty stream" => "empty_stream",
        _ => "access_check_failed",
    };
    (code, None)
}

pub(crate) fn album_evidence_access_diagnostic(
    code: &'static str,
    error: &str,
    audio: &PreviewAudio,
) -> Value {
    let (access_code, http_status) = album_evidence_access_failure_code(error);
    let mut diagnostic = album_evidence_audio_diagnostic(code, audio);
    diagnostic["accessCode"] = serde_json::json!(access_code);
    if let Some(status) = http_status {
        diagnostic["httpStatus"] = serde_json::json!(status);
    }
    diagnostic
}

impl PreviewAudio {
    pub(crate) fn provider_session_current(&self) -> bool {
        self.provider_session
            .as_ref()
            .is_none_or(ProviderSessionStamp::is_current)
    }
}

#[derive(Debug, Clone)]
pub struct AudioSearchPage {
    pub tracks: Vec<Track>,
    pub entry_count: usize,
}

/// Independent acquisition capability; catalog and preview providers need not implement it.
#[async_trait::async_trait]
pub trait AcquisitionProvider: Send + Sync {
    async fn find_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Candidate>, String>;
    async fn inspect_formats(
        &self,
        track: &Track,
        candidate: &Candidate,
        settings: &Settings,
    ) -> Result<Candidate, String>;
    async fn download(
        &self,
        request: DownloadRequest,
        settings: Settings,
    ) -> Result<DownloadJob, String>;
}

fn base_args(settings: &Settings) -> Vec<String> {
    let mut args = vec![
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-remote-components",
        "--no-js-runtimes",
        "--no-color",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if !settings.js_runtime_path.trim().is_empty() {
        let configured = settings
            .js_runtime_path
            .trim()
            .strip_prefix("system:")
            .or_else(|| settings.js_runtime_path.trim().strip_prefix("bundled:"))
            .unwrap_or(settings.js_runtime_path.trim());
        let name = Path::new(configured)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("deno");
        let runtime = match name {
            "node" | "nodejs" => "node",
            "bun" => "bun",
            "qjs" | "quickjs" => "quickjs",
            _ => "deno",
        };
        args.extend([
            "--js-runtimes".into(),
            format!(
                "{runtime}:{}",
                crate::tools::executable(&settings.js_runtime_path, runtime).display()
            ),
        ]);
    }
    args
}

pub(crate) fn uses_managed_ytdlp(configured: &str) -> bool {
    let configured = configured.trim();
    configured.is_empty() || configured == "yt-dlp" || configured.starts_with("bundled:")
}

async fn add_youtube_support_args(
    args: &mut Vec<String>,
    provider: &str,
    settings: &Settings,
    playback: Option<(
        tokio::time::Instant,
        watch::Receiver<bool>,
        Option<crate::playback_sources::SourceTrace>,
    )>,
) -> Result<(bool, Option<crate::youtube_support::ProviderInvocation>), String> {
    if provider != "youtube" || !uses_managed_ytdlp(&settings.yt_dlp_path) {
        return Ok((false, None));
    }
    if let Some(manager) = crate::youtube_support::global() {
        match managed_provider_action(
            manager.provider_assets_incomplete(),
            system_ytdlp().is_file(),
        ) {
            ManagedProviderAction::SystemFallback => {
                // A partial optional bundle must not block the public system
                // yt-dlp fallback. Do not start the bundled helper at all.
                return Ok((false,None));
            }
            ManagedProviderAction::Unavailable => return Err(
                "The managed YouTube provider bundle is incomplete and no system yt-dlp is available. Repair the provider bundle or install yt-dlp.".into(),
            ),
            ManagedProviderAction::Managed => {}
        }
        let invocation = if let Some((deadline, cancel, trace)) = playback {
            manager
                .playback_provider_invocation(deadline, cancel, trace)
                .await?
        } else {
            manager.provider_invocation().await?
        };
        let installed = !invocation.args.is_empty();
        append_managed_youtube_provider_args(args, invocation.args.clone());
        return Ok((installed, Some(invocation)));
    }
    Ok((false, None))
}

fn should_force_managed_mweb(
    provider: &str,
    configured_ytdlp: &str,
    managed_provider_args_installed: bool,
) -> bool {
    provider == "youtube" && uses_managed_ytdlp(configured_ytdlp) && managed_provider_args_installed
}

fn should_force_managed_mweb_for_extraction(
    provider: &str,
    configured_ytdlp: &str,
    managed_provider_args_installed: bool,
    format_selector_override: Option<&str>,
) -> bool {
    format_selector_override.is_none()
        && should_force_managed_mweb(provider, configured_ytdlp, managed_provider_args_installed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedProviderAction {
    Managed,
    SystemFallback,
    Unavailable,
}

fn managed_provider_action(incomplete: bool, system_available: bool) -> ManagedProviderAction {
    match (incomplete, system_available) {
        (true, true) => ManagedProviderAction::SystemFallback,
        (true, false) => ManagedProviderAction::Unavailable,
        (false, _) => ManagedProviderAction::Managed,
    }
}

fn system_ytdlp() -> std::path::PathBuf {
    crate::tools::executable("system:yt-dlp", "yt-dlp")
}

fn yt_dlp_executable(settings: &Settings) -> std::path::PathBuf {
    if uses_managed_ytdlp(&settings.yt_dlp_path) {
        if let Some(manager) = crate::youtube_support::global() {
            return choose_managed_ytdlp(
                manager.active_executable(),
                manager.provider_assets_incomplete(),
                system_ytdlp(),
            );
        }
    }
    crate::tools::executable(&settings.yt_dlp_path, "yt-dlp")
}

fn choose_managed_ytdlp(
    active: std::path::PathBuf,
    provider_incomplete: bool,
    system: std::path::PathBuf,
) -> std::path::PathBuf {
    if provider_incomplete && system.is_file() {
        system
    } else {
        active
    }
}

fn append_managed_youtube_provider_args(
    args: &mut Vec<String>,
    provider_args: Vec<std::ffi::OsString>,
) {
    let provides_node_runtime = provider_args
        .windows(2)
        .any(|pair| pair[0] == "--js-runtimes" && pair[1].to_string_lossy().starts_with("node:"));
    if provides_node_runtime {
        // The managed BgUtils provider supplies the Node runtime yt-dlp uses
        // for YouTube extraction. Clear a configured runtime such as Deno so
        // one extraction does not initialize multiple JS engines.
        args.push("--no-js-runtimes".into());
    }
    args.extend(
        provider_args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
}

fn ffmpeg_location(configured: &str) -> Option<String> {
    // yt-dlp's --ffmpeg-location checks filesystem existence, unlike Command's PATH lookup.
    // Passing the default bare "ffmpeg" incorrectly disables an installed PATH executable.
    if matches!(configured, "ffmpeg" | "ffmpeg.exe") {
        return None;
    }
    let path = Path::new(configured);
    if path.is_absolute() || path.components().count() > 1 {
        return Some(configured.into());
    }
    if let Some(search) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&search) {
            let candidate = directory.join(configured);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into());
            }
            #[cfg(windows)]
            {
                let candidate = directory.join(format!("{configured}.exe"));
                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().into());
                }
            }
        }
    }
    Some(configured.into())
}

fn is_preserved_source(asset: &LocalAsset, candidate: &Candidate) -> bool {
    let same_source = if candidate.provider == "slskd" {
        asset.track.provenance["acquisition"]["id"].as_str() == Some(candidate.id.as_str())
    } else {
        asset.track.provenance["acquisition"]["url"].as_str() == Some(candidate.url.as_str())
    };
    asset.track.provenance.get("derivedFromAssetId").is_none()
        && asset.track.provenance.get("chapter").is_none()
        && asset.track.provenance["acquisition"]["provider"].as_str()
            == Some(candidate.provider.as_str())
        && same_source
}

/// A previously acquired source can seed a distinct managed copy only when
/// its stored locator is still exactly the selected locator and current shared
/// recording evidence binds that source to the requested catalog track.
fn preserved_source_reuse_assessment(
    store: &Store,
    requested: &Track,
    existing: &LocalAsset,
    candidate: &Candidate,
    matching_level: crate::recording_match::MatchLevel,
) -> Option<crate::recording_match::MatchAssessment> {
    if !is_preserved_source(existing, candidate)
        || existing.track.provenance["acquisition"]["id"].as_str() != Some(candidate.id.as_str())
    {
        return None;
    }
    if candidate.provider == "slskd" {
        let Some(CandidateSource::Soulseek {
            username: candidate_user,
            filename: candidate_filename,
            ..
        }) = candidate.source.as_ref()
        else {
            return None;
        };
        let acquisition_source = existing.track.provenance["acquisition"].get("source")?;
        if acquisition_source["username"].as_str() != Some(candidate_user.as_str())
            || acquisition_source["filename"].as_str() != Some(candidate_filename.as_str())
        {
            return None;
        }
    } else if canonical_source_url(
        existing.track.provenance["acquisition"]["url"]
            .as_str()
            .unwrap_or_default(),
    ) != canonical_source_url(&candidate.url)
    {
        return None;
    }

    let assessment = crate::recording_match_context::assess(
        store,
        requested,
        candidate,
        matching_level,
        crate::recording_match::MatchStage::Resolved,
    );
    use crate::recording_match::{Comparison, MatchAction, MatchRelationship, ProofStrength};
    let evidence = assessment.evidence.as_ref()?;
    if assessment.action != MatchAction::Accept
        || !matches!(
            assessment.relationship,
            MatchRelationship::Exact | MatchRelationship::Probable
        )
        || assessment.proof_strength != ProofStrength::RecordingIdentifier
        || assessment.dimensions.isrc != Comparison::Agree
        || assessment.dimensions.artists != Comparison::Agree
        || assessment.dimensions.runtime != Comparison::Agree
        || assessment.dimensions.title == Comparison::Conflict
        || assessment.dimensions.version == Comparison::Conflict
        || evidence.identifier_conflict
        || evidence.conflicting_version_observations
        || evidence.parsing_incomplete
    {
        return None;
    }

    let manual = &requested.provenance["manualFields"];
    if (manual["title"] == true && assessment.dimensions.title == Comparison::Conflict)
        || (manual["artist"] == true && assessment.dimensions.artists == Comparison::Conflict)
        || (manual["version"] == true && assessment.dimensions.version == Comparison::Conflict)
        || (manual["isrc"] == true && assessment.dimensions.isrc == Comparison::Conflict)
    {
        return None;
    }
    Some(assessment)
}

fn preserved_source_reuse_track(
    requested: &Track,
    existing: &LocalAsset,
    candidate: &Candidate,
    assessment: &crate::recording_match::MatchAssessment,
    format_id: Option<&str>,
) -> Result<Track, String> {
    let mut track = artifact_track_for_match(requested, candidate, assessment.relationship, false)?;
    track.provenance["acquisition"] = serde_json::json!({
        "provider": candidate.provider,
        "url": candidate.url,
        "id": candidate.id,
        "permission": candidate.permission,
        "formatId": format_id,
        "source": candidate.source,
    });
    track.provenance["sourceReuse"] = serde_json::json!({
        "assetId": existing.id,
        "assetHash": existing.hash,
        "proofStrength": assessment.proof_strength,
        "relationship": assessment.relationship,
        "isrcAgreement": true,
        "method": "verified-copy-preserving-catalog-identity",
    });
    Ok(track)
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

pub fn validate_source_url(raw: &str, provider: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| "Invalid source URL".to_string())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
    {
        return Err("Sources must use public HTTPS URLs without credentials".into());
    }
    let host = url.host_str().ok_or("Missing source hostname")?;
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
    {
        return Err("Local and IP-literal source URLs are not supported".into());
    }
    let allowed = match provider {
        "youtube" => host_matches(host, "youtube.com") || host == "youtu.be",
        "soundcloud" => host_matches(host, "soundcloud.com"),
        "bandcamp" => host_matches(host, "bandcamp.com"),
        "direct" => true,
        "monochrome" => host == "tracks.monochrome.st",
        _ => false,
    };
    if !allowed {
        return Err("Source host does not match the acquisition provider".into());
    }
    Ok(url)
}

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            !v.is_private()
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_unspecified()
                && !v.is_multicast()
                && !v.is_broadcast()
                && v.octets()[0] != 0
                && v.octets()[0] < 224
                && !(v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        std::net::IpAddr::V6(v) => {
            !v.is_loopback()
                && !v.is_unspecified()
                && !v.is_multicast()
                && (v.segments()[0] & 0xfe00) != 0xfc00
                && (v.segments()[0] & 0xffc0) != 0xfe80
                && v.to_ipv4_mapped()
                    .map(|x| is_public_ip(x.into()))
                    .unwrap_or(true)
        }
    }
}

#[cfg(test)]
tokio::task_local! {
    static PLAYBACK_TEST_PUBLIC_DNS: bool;
}

/// Scope fake-extractor resolver tests to deterministic public DNS without
/// modifying production URL validation, DNS filtering, pinning, or HTTP probes.
#[cfg(test)]
pub(crate) async fn with_playback_test_public_addresses<F: std::future::Future>(
    future: F,
) -> F::Output {
    PLAYBACK_TEST_PUBLIC_DNS.scope(true, future).await
}

pub(crate) async fn public_addresses(url: &url::Url) -> Result<Vec<std::net::SocketAddr>, String> {
    #[cfg(test)]
    if PLAYBACK_TEST_PUBLIC_DNS
        .try_with(|enabled| *enabled)
        .unwrap_or(false)
    {
        return Ok(vec![std::net::SocketAddr::from(([8, 8, 8, 8], 443))]);
    }
    let host = url.host_str().ok_or("Missing hostname")?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, 443)),
    )
    .await
    .map_err(|_| "DNS lookup timed out")?
    .map_err(|e| format!("DNS lookup failed: {e}"))?
    .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|a| !is_public_ip(a.ip())) {
        return Err("Source resolves to a nonpublic address".into());
    }
    Ok(addresses)
}

async fn run_playback_stage<T, Fut>(
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    timeout_message: &'static str,
    stage: Fut,
) -> Result<T, String>
where
    Fut: std::future::Future<Output = Result<T, String>>,
{
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(timeout_message.into());
    }
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => Err("Cancelled".into()),
        _ = tokio::time::sleep_until(deadline) => Err(timeout_message.into()),
        result = stage => result,
    }
}

/// Own the helper's process group until its leader is reaped. An outer
/// deadline/cancellation can drop run_tool_until without polling its select.
/// Keep the leader unreaped until pipe processing is complete so its PID cannot
/// be recycled while this guard may still need to signal the process group.
struct ToolProcess {
    child: Option<Child>,
    temp_dir: Option<tempfile::TempDir>,
}
impl std::ops::Deref for ToolProcess {
    type Target = Child;
    fn deref(&self) -> &Child {
        self.child.as_ref().expect("tool child is present")
    }
}
impl std::ops::DerefMut for ToolProcess {
    fn deref_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("tool child is present")
    }
}
impl Drop for ToolProcess {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let temp_dir = self.temp_dir.take();
        let pid = child.id();
        #[cfg(unix)]
        if let Some(pid) = pid {
            // The child was spawned with process_group(0). SIGKILL is
            // synchronous, so the group cannot keep writing as the reaper waits.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        // This guard can be dropped when an outer cancellation/deadline drops
        // the future. Keep its private temp directory alive until the killed
        // process has been reaped, then remove it with TempDir's Drop.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                #[cfg(windows)]
                if let Some(pid) = pid {
                    let mut command = Command::new("taskkill");
                    crate::tools::configure_no_console_tokio(&mut command);
                    let _ = command
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .output()
                        .await;
                }
                let _ = child.kill().await;
                let _ = child.wait().await;
                drop(temp_dir);
            });
        } else {
            // A future can outlive its runtime. Start termination before
            // handing the child to a plain reaper thread, which retains the
            // directory until try_wait confirms the leader has exited.
            #[cfg(not(windows))]
            let _ = child.start_kill();
            std::thread::spawn(move || {
                #[cfg(windows)]
                if let Some(pid) = pid {
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .creation_flags(0x08000000)
                        .output();
                }
                #[cfg(windows)]
                let _ = child.start_kill();
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
                drop(temp_dir);
            });
        }
    }
}

struct ToolPipeTasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for ToolPipeTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let mut command = Command::new("taskkill");
            crate::tools::configure_no_console_tokio(&mut command);
            let _ = command
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output()
                .await;
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn pump<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stdout: bool,
    tx: mpsc::Sender<(bool, String)>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stdout, line)).await.is_err() {
            break;
        }
    }
}

#[cfg(unix)]
async fn wait_for_tool_spawn_retry(
    cancel: &mut watch::Receiver<bool>,
    wake_at: tokio::time::Instant,
) -> Result<(), String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let mut cancel_open = true;
    let sleep = tokio::time::sleep_until(wake_at);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return Ok(()),
            changed = cancel.changed(), if cancel_open => match changed {
                Ok(()) if *cancel.borrow() => return Err("Cancelled".into()),
                Ok(()) => {},
                Err(_) => {
                    // A closed watch channel with a retained false value is not
                    // cancellation. Disable it while waiting so it cannot spin.
                    cancel_open = false;
                    if *cancel.borrow() { return Err("Cancelled".into()); }
                }
            }
        }
    }
}

async fn run_tool<F: FnMut(&str)>(
    settings: &Settings,
    args: Vec<String>,
    mut cancel: watch::Receiver<bool>,
    limit: Duration,
    mut on_line: F,
) -> Result<String, String> {
    run_tool_until(
        settings,
        args,
        cancel,
        tokio::time::Instant::now() + limit,
        on_line,
    )
    .await
}

async fn run_tool_until<F: FnMut(&str)>(
    settings: &Settings,
    args: Vec<String>,
    mut cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    mut on_line: F,
) -> Result<String, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let yt_dlp = yt_dlp_executable(settings);
    let temp_dir = tempfile::Builder::new()
        .prefix("burrata-yt-dlp-")
        .tempdir()
        .map_err(|error| format!("Could not create yt-dlp temporary directory: {error}"))?;
    let mut command = Command::new(yt_dlp);
    command
        .args(args)
        .env("TMPDIR", temp_dir.path())
        .env("TMP", temp_dir.path())
        .env("TEMP", temp_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    if tokio::time::Instant::now() >= deadline {
        return Err("yt-dlp operation timed out".into());
    }
    let spawn_result = command.spawn();
    #[cfg(unix)]
    let mut child = match spawn_result {
        Ok(child) => child,
        Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
            let retry_at = (tokio::time::Instant::now() + Duration::from_millis(10)).min(deadline);
            wait_for_tool_spawn_retry(&mut cancel, retry_at).await?;
            if tokio::time::Instant::now() >= deadline {
                return Err("yt-dlp operation timed out".into());
            }
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
            command
                .spawn()
                .map_err(|error| format!("Could not start yt-dlp: {error}"))?
        }
        Err(error) => return Err(format!("Could not start yt-dlp: {error}")),
    };
    #[cfg(not(unix))]
    let mut child = spawn_result.map_err(|error| format!("Could not start yt-dlp: {error}"))?;

    let mut child = ToolProcess {
        child: Some(child),
        temp_dir: Some(temp_dir),
    };
    let (tx, mut rx) = mpsc::channel(32);
    let out = tokio::spawn(pump(
        child.stdout.take().ok_or("Missing stdout")?,
        true,
        tx.clone(),
    ));
    let err = tokio::spawn(pump(
        child.stderr.take().ok_or("Missing stderr")?,
        false,
        tx,
    ));
    let _pipe_tasks = ToolPipeTasks(vec![out, err]);
    let timeout = tokio::time::sleep_until(deadline);
    tokio::pin!(timeout);
    let mut output = String::new();
    let mut errors = String::new();
    let mut stream_open = true;
    let mut cancel_open = true;
    let result = loop {
        tokio::select! {
            _ = &mut timeout => { terminate(&mut child).await; break Err("yt-dlp operation timed out".into()); }
            changed = cancel.changed(), if cancel_open => {
                match changed {
                    Ok(()) if *cancel.borrow() => { terminate(&mut child).await; break Err("Cancelled".into()); }
                    Ok(()) => {},
                    Err(_) => {
                        // A watch sender can send `true` and then be dropped in
                        // the same turn. `has_changed()` reports a closed
                        // channel before this task polls `changed()`, so check
                        // the retained value before disabling this branch.
                        cancel_open = false;
                        if *cancel.borrow() { terminate(&mut child).await; break Err("Cancelled".into()); }
                    }
                }
            }
            // Reaping is the final await: after it succeeds no future drop
            // can need to signal the now-recyclable process-group ID.
            exit = child.wait(), if !stream_open => {
                break match exit {
                    Ok(status) if status.success() => Ok(output),
                    Ok(_) => Err(format!("yt-dlp failed: {}", errors.trim())),
                    Err(error) => Err(format!("yt-dlp wait failed: {error}")),
                };
            }
            line = rx.recv(), if stream_open => match line {
                Some((stdout,line)) => {
                    on_line(&line);
                    if stdout {
                        if output.len()+line.len() > MAX_OUTPUT { terminate(&mut child).await; break Err("yt-dlp response exceeded size limit".into()); }
                        output.push_str(&line); output.push('\n');
                    } else {
                        errors.push_str(&line); errors.push('\n');
                        if errors.len()>8192 { errors=errors.chars().rev().take(4096).collect::<String>().chars().rev().collect(); }
                    }
                }
                None => {
                    stream_open=false;
                }
            }
        }
    };
    if child.id().is_none() {
        // All ordinary exits and handled timeouts/cancellations reap in the
        // same future, so remove their directory before returning.
        drop(child.temp_dir.take());
    }
    result
}

fn idle_cancel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Resolve the bounded flat-playlist entries from YouTube's per-video Mix
/// (`RD<video-id>`). It never requests audio formats or downloads media.
pub(crate) async fn fetch_youtube_mix_entries(
    seed_url: &str,
    settings: &Settings,
    recommendation_limit: usize,
) -> Result<Vec<Value>, String> {
    if !(1..=50).contains(&recommendation_limit) {
        return Err("Radio recommendation limit must be between 1 and 50".into());
    }
    let seed_id = youtube_video_id(seed_url).ok_or("Radio seed must be a valid YouTube video")?;
    let mix_url =
        format!("https://www.youtube.com/watch?v={seed_id}&list=RD{seed_id}&start_radio=1");
    let mut args = base_args(settings);
    // Flat catalog/Mix reads do not extract formats or require a token provider.
    args.extend([
        "--flat-playlist".into(),
        "--dump-single-json".into(),
        "--skip-download".into(),
        "--playlist-end".into(),
        (recommendation_limit + 1).to_string(),
        "--".into(),
        mix_url,
    ]);
    let (_, cancel) = idle_cancel();
    let raw = run_tool(settings, args, cancel, Duration::from_secs(35), |_| {}).await?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("yt-dlp returned invalid YouTube Mix metadata: {e}"))?;
    let entries = value["entries"]
        .as_array()
        .ok_or("yt-dlp did not return YouTube Mix playlist entries")?;
    Ok(entries
        .iter()
        .take(recommendation_limit.saturating_add(1))
        .cloned()
        .collect())
}

pub(crate) fn youtube_video_id(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.scheme(), "http" | "https")
    {
        return None;
    }
    let host = url.host_str()?;
    let id = if matches!(
        host,
        "youtube.com" | "www.youtube.com" | "music.youtube.com"
    ) {
        url.query_pairs()
            .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))?
    } else if matches!(host, "youtu.be" | "www.youtu.be") {
        url.path_segments()?.next()?.to_owned()
    } else {
        return None;
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return None;
    }
    Some(id)
}

fn candidate_from_json(info: &Value, provider: &str) -> Candidate {
    let id = info["id"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_default();
    let url = ["webpage_url", "original_url", "url"]
        .iter()
        .filter_map(|key| info[*key].as_str())
        .find(|value| !value.trim().is_empty())
        .map(str::trim)
        .unwrap_or_default();
    let availability = info["availability"].as_str().unwrap_or("");
    let restricted =
        ["private", "premium_only", "subscriber_only", "needs_auth"].contains(&availability);
    let formats = info["formats"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|f| {
                    f["vcodec"].as_str() == Some("none")
                        && f["acodec"].as_str().is_some_and(|v| v != "none")
                })
                .map(|f| {
                    let codec = f["acodec"].as_str().unwrap_or("unknown").to_string();
                    AudioFormat {
                        bit_depth: None,
                        channels: None,
                        size_bytes: None,
                        id: f["format_id"].as_str().unwrap_or_default().into(),
                        lossless: ["flac", "alac", "wav", "pcm"]
                            .iter()
                            .any(|c| codec.starts_with(c)),
                        codec,
                        extension: f["ext"].as_str().unwrap_or_default().into(),
                        bitrate: f["abr"].as_f64(),
                        sample_rate: f["asr"].as_u64().map(|x| x as u32),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let valid_artwork = |value: &str| {
        let parsed = url::Url::parse(value).ok()?;
        (matches!(parsed.scheme(), "http" | "https")
            && parsed.username().is_empty()
            && parsed.password().is_none())
        .then(|| value.to_owned())
    };
    let artwork_url = info["thumbnail"]
        .as_str()
        .and_then(valid_artwork)
        .or_else(|| {
            info["thumbnails"].as_array().and_then(|items| {
                items
                    .iter()
                    .rev()
                    .find_map(|item| item["url"].as_str().and_then(valid_artwork))
            })
        });
    let chapters = parse_source_chapters(info);
    // Keep optional provider-reported statistics from this existing response.
    // Counts never trigger another request or become part of recording matching.
    let mut reported = serde_json::Map::new();
    let valid_count = |value: &Value| value.as_u64().map(Value::from);
    if provider == "soundcloud" {
        if let Some(count) =
            valid_count(&info["playback_count"]).or_else(|| valid_count(&info["view_count"]))
        {
            reported.insert("plays".into(), count);
        }
    } else if let Some(count) = valid_count(&info["view_count"]) {
        reported.insert("views".into(), count);
    }
    if let Some(count) = valid_count(&info["play_count"]) {
        reported.insert("plays".into(), count);
    }
    let mut source_metadata = reported;
    source_metadata.insert("extractor".into(), Value::Bool(true));
    source_metadata.insert(
        "sourceMetadataOrigin".into(),
        Value::String("ytDlpCandidateMetadata".into()),
    );
    source_metadata.insert("resolvedProviderId".into(), Value::String(id.to_owned()));
    if let Some(title) = metadata_text(&info["track"])
        .or_else(|| metadata_text(&info["title"]))
        .filter(|title| title.len() <= 512)
    {
        source_metadata.insert("track".into(), Value::String(title.clone()));
        source_metadata.insert("catalogTitle".into(), Value::String(title));
    }
    if let Some(artists) = info["artists"].as_array().filter(|artists| {
        !artists.is_empty()
            && artists.len() <= 32
            && artists.iter().all(|artist| {
                artist
                    .as_str()
                    .is_some_and(|name| !name.trim().is_empty() && name.trim().len() <= 256)
            })
    }) {
        let artists = artists
            .iter()
            .filter_map(Value::as_str)
            .map(|name| name.trim().to_owned())
            .collect::<Vec<_>>();
        source_metadata.insert("artistCount".into(), Value::from(artists.len()));
        source_metadata.insert("artists".into(), serde_json::json!(artists));
    } else if let Some(artist) = metadata_text(&info["artist"]).filter(|artist| artist.len() <= 256)
    {
        source_metadata.insert("artist".into(), Value::String(artist));
    }
    for key in ["version", "mix", "album"] {
        if let Some(value) = metadata_text(&info[key]).filter(|value| value.len() <= 512) {
            source_metadata.insert(key.into(), Value::String(value));
        }
    }
    if let Some(value) = metadata_text(&info["genre"]).filter(|value| value.len() <= 256) {
        source_metadata.insert("genre".into(), Value::String(value));
    }
    if let Some(value) = metadata_text(&info["album_artist"])
        .or_else(|| metadata_text(&info["albumArtist"]))
        .filter(|value| value.len() <= 512)
    {
        source_metadata.insert("albumArtist".into(), Value::String(value));
    }
    if let Some(isrc) = info["isrc"]
        .as_str()
        .and_then(crate::recording_identity::normalize_isrc)
    {
        source_metadata.insert("isrc".into(), Value::String(isrc));
    }
    if let Some(duration) = info["duration"]
        .as_f64()
        .filter(|duration| duration.is_finite() && (1.0..=86400.0).contains(duration))
    {
        source_metadata.insert("duration".into(), serde_json::json!(duration));
    }
    Candidate {
        source_metadata: Some(Value::Object(source_metadata)),
        id: format!("{provider}:{id}"),
        provider: provider.into(),
        url: url.into(),
        title: info["title"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Untitled")
            .into(),
        uploader: info["uploader"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        duration_seconds: info["duration"].as_f64(),
        artwork_url,
        chapters,
        formats,
        permission: if restricted { "restricted" } else { "unknown" }.into(),
        ..Default::default()
    }
}

fn parse_source_chapters(info: &Value) -> Option<Vec<Chapter>> {
    info["chapters"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let start_seconds = item["start_time"].as_f64()?;
                    let end_seconds = item["end_time"].as_f64()?;
                    let title = item["title"].as_str()?.trim();
                    if !start_seconds.is_finite()
                        || !end_seconds.is_finite()
                        || start_seconds < 0.0
                        || end_seconds <= start_seconds
                        || title.is_empty()
                    {
                        return None;
                    }
                    Some(Chapter {
                        id: format!("chapter-{index}-{}", start_seconds.to_bits()),
                        title: title.to_owned(),
                        start_seconds,
                        end_seconds,
                        confidence: "source_metadata_unverified".into(),
                        suggested_track: None,
                        suggestion_confidence: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|chapters| !chapters.is_empty())
}

pub async fn find_candidates(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
) -> Result<Vec<Candidate>, String> {
    let (_tx, cancel) = idle_cancel();
    find_candidates_with_cancel(
        track,
        provider_ids,
        settings,
        cancel,
        10,
        Duration::from_secs(60),
    )
    .await
}

async fn find_candidates_with_cancel(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
) -> Result<Vec<Candidate>, String> {
    find_candidates_with_cancel_until(
        track,
        provider_ids,
        settings,
        cancel,
        result_limit,
        per_provider_timeout,
        None,
    )
    .await
}

async fn find_candidates_with_cancel_until(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
    shared_deadline: Option<crate::playback_sources::PlaybackDeadline>,
) -> Result<Vec<Candidate>, String> {
    find_candidates_with_cancel_until_alias(
        track,
        provider_ids,
        settings,
        cancel,
        result_limit,
        per_provider_timeout,
        shared_deadline,
        None,
    )
    .await
}

async fn find_candidates_with_cancel_until_alias(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
    shared_deadline: Option<crate::playback_sources::PlaybackDeadline>,
    alternate_artist: Option<String>,
) -> Result<Vec<Candidate>, String> {
    find_candidates_with_cancel_until_alias_detailed(
        track,
        provider_ids,
        settings,
        cancel,
        result_limit,
        per_provider_timeout,
        shared_deadline,
        alternate_artist,
        None,
    )
    .await
    .map(|(candidates, _complete)| candidates)
}

async fn find_candidates_with_cancel_until_alias_detailed(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
    shared_deadline: Option<crate::playback_sources::PlaybackDeadline>,
    alternate_artist: Option<String>,
    provider_query_cache: Option<&CandidateSearchCache>,
) -> Result<(Vec<Candidate>, bool), String> {
    if let Some(candidate) = track
        .provenance
        .get("audioCandidate")
        .and_then(|value| serde_json::from_value::<Candidate>(value.clone()).ok())
    {
        if validate_source_url(&candidate.url, &candidate.provider).is_ok()
            && provider_ids
                .iter()
                .any(|provider| provider == &candidate.provider)
            // Imported audio-search rows can retain an old candidate. For
            // YouTube, bind its stable ID to the exact validated page before
            // returning it; otherwise let the normal search replace the hint.
            && (candidate.provider != "youtube"
                || playback_youtube_search_url(&candidate).is_some())
        {
            return Ok((vec![candidate], true));
        }
    }
    use futures_util::StreamExt;
    let provider_results =
        futures_util::stream::iter(provider_ids.iter().cloned().map(|provider| {
            let cancel = cancel.clone();
            let alternate_artist = alternate_artist.clone();
            async move {
                search_provider_candidates(
                    &provider,
                    track,
                    settings,
                    cancel,
                    result_limit,
                    per_provider_timeout,
                    shared_deadline,
                    alternate_artist,
                    provider_query_cache,
                    false,
                )
                .await
            }
        }))
        .buffered(2)
        .collect::<Vec<_>>()
        .await;
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for (found, provider_errors) in provider_results {
        candidates.extend(found);
        errors.extend(provider_errors);
    }
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    if candidates.is_empty() && !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        if !errors.is_empty() {
            for candidate in &mut candidates {
                candidate
                    .reasons
                    .push(format!("Some providers failed: {}", errors.join("; ")));
            }
        }
        Ok((candidates, errors.is_empty()))
    }
}

/// Playback searches must not let an unverified candidate embedded in an
/// imported playlist row hide alternative provider results. Explicit audio
/// search tracks keep their captured candidate; other catalog rows search by
/// recording identity and let playback's strict matcher decide.
fn track_for_playback_source_search(track: &Track) -> Track {
    let mut search_track = crate::matching::ephemeral_playback_identity(track).into_owned();
    if search_track.provenance["sourceKind"].as_str() != Some("audioSearch") {
        if let Some(provenance) = search_track.provenance.as_object_mut() {
            provenance.remove("audioCandidate");
        }
    }
    search_track
}

/// Monochrome uses a canonical query plus at most the first exact cached alias,
/// matching the existing two-query bound used by YouTube and SoundCloud.
fn monochrome_search_tracks(track: &Track, alternate_artist: Option<&str>) -> Vec<Track> {
    let mut tracks = vec![track.clone()];
    if let Some(artist) = alternate_artist.map(str::trim).filter(|artist| {
        !artist.is_empty()
            && crate::matching::normalize(artist) != crate::matching::normalize(track.artist.trim())
    }) {
        let mut alternate = track.clone();
        alternate.artist = artist.to_owned();
        tracks.push(alternate);
    }
    tracks
}

/// Only playback's managed anonymous search uses the native adapter. Custom
/// helper selection and interactive/discovery search keep their existing path.
/// The environment override permits immediate rollback without changing tags,
/// provider preferences, cached identity or the extractor/access verifier.
fn native_youtube_playback_search_enabled(
    provider: &str,
    helper: &str,
    has_playback_deadline: bool,
    runtime_override: Option<&str>,
) -> bool {
    provider == "youtube"
        && uses_managed_ytdlp(helper)
        && has_playback_deadline
        && matches!(runtime_override, None | Some("1"))
}

fn playback_provider_query_cache_key(
    provider: &str,
    prefix: &str,
    query: &str,
    result_limit: usize,
    settings: &Settings,
    native_youtube: bool,
) -> String {
    serde_json::json!([
        "playback-provider-query-v1",
        provider,
        prefix,
        query,
        result_limit,
        if native_youtube {
            "youtube-web-anonymous-v1"
        } else {
            "ytdlp-ignored-config-v1"
        },
        crate::youtube_support::active_executable().to_string_lossy(),
        settings.yt_dlp_path,
        settings.js_runtime_path,
        // The current playback discovery adapters deliberately use anonymous
        // public queries; include the context explicitly so a future
        // authenticated profile cannot share these rows by accident.
        "anonymous-session",
    ])
    .to_string()
}

fn monochrome_playback_query_cache_key(profile: &str, query: &str) -> String {
    serde_json::json!([
        "playback-provider-query-v1",
        "monochrome",
        profile,
        query,
        crate::monochrome::CANDIDATE_SEARCH_LIMIT,
        "anonymous-session",
    ])
    .to_string()
}

async fn search_provider_candidates(
    provider: &str,
    track: &Track,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    result_limit: usize,
    per_provider_timeout: Duration,
    shared_deadline: Option<crate::playback_sources::PlaybackDeadline>,
    alternate_artist: Option<String>,
    provider_query_cache: Option<&CandidateSearchCache>,
    canonical_only: bool,
) -> (Vec<Candidate>, Vec<String>) {
    if provider == "monochrome" {
        let client = match crate::monochrome::Client::new() {
            Ok(client) => client,
            Err(error) => return (Vec::new(), vec![error]),
        };
        let query_tracks = monochrome_search_tracks(track, alternate_artist.as_deref());
        let provider_deadline = tokio::time::Instant::now() + per_provider_timeout;
        let deadline = shared_deadline
            .map(crate::playback_sources::PlaybackDeadline::instant)
            .map_or(provider_deadline, |shared| shared.min(provider_deadline));
        let profile = match client.search_cache_profile() {
            Ok(profile) => profile,
            Err(error) => return (Vec::new(), vec![error]),
        };
        let mut candidates = Vec::new();
        let mut errors = Vec::new();
        for query_track in query_tracks {
            if *cancel.borrow() {
                return (Vec::new(), vec!["Cancelled".into()]);
            }
            if tokio::time::Instant::now() >= deadline {
                errors.push("Monochrome search timed out".into());
                break;
            }
            let query = crate::monochrome::search_query_for_track(&query_track);
            let cache_key = monochrome_playback_query_cache_key(&profile, &query);
            let load = || async {
                let result =
                    tokio::time::timeout_at(deadline, client.search_candidates_raw(&query))
                        .await
                        .map_err(|_| "Monochrome search timed out".to_owned())??;
                let (candidates, row_count) = result;
                // The adapter rejects malformed/missing `tracks` values and
                // returns an empty vector only for a valid `tracks: []` page.
                // Empty successful pages are cacheable negative results.
                let completeness = ProviderQueryCompleteness::ReturnedPage {
                    row_count,
                    reached_limit: row_count >= crate::monochrome::CANDIDATE_SEARCH_LIMIT,
                };
                Ok(ProviderQueryResponse {
                    candidates,
                    completeness,
                })
            };
            let response = if let Some(cache) = provider_query_cache {
                cache
                    .get_or_search_provider_query(cache_key, cancel.clone(), load)
                    .await
            } else {
                tokio::select! {
                    biased;
                    _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
                    result = load() => result,
                }
            };
            match response {
                Ok(response) => {
                    let mut rows = response.candidates;
                    for candidate in &mut rows {
                        crate::monochrome::score_candidate_for_track(&query_track, candidate);
                        candidate
                            .reasons
                            .push("Download permission has not been established".into());
                    }
                    candidates.extend(rows);
                }
                Err(error) if error == "Cancelled" || *cancel.borrow() => {
                    return (candidates, vec!["Cancelled".into()]);
                }
                Err(error) => errors.push(error),
            }
        }
        let mut by_id = HashMap::<String, usize>::new();
        let mut unique: Vec<Candidate> = Vec::new();
        for candidate in candidates {
            if let Some(index) = by_id.get(&candidate.id).copied() {
                if candidate.score > unique[index].score {
                    unique[index] = candidate;
                }
            } else {
                by_id.insert(candidate.id.clone(), unique.len());
                unique.push(candidate);
            }
        }
        unique.sort_by(|a, b| b.score.total_cmp(&a.score));
        return (unique, errors);
    }
    let prefix = match provider {
        "youtube" => format!("ytsearch{result_limit}"),
        "soundcloud" => format!("scsearch{result_limit}"),
        _ => return (Vec::new(), Vec::new()),
    };
    let queries = playback_search_queries(track, alternate_artist.as_deref());
    let deadline = Instant::now() + per_provider_timeout;
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for query in queries.iter().take(if canonical_only { 1 } else { 2 }) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            errors.push(format!("{provider}: search timed out"));
            break;
        }
        let native_enabled = native_youtube_playback_search_enabled(
            provider,
            &settings.yt_dlp_path,
            shared_deadline.is_some(),
            std::env::var("BURRATA_NATIVE_YOUTUBE_SEARCH")
                .ok()
                .as_deref(),
        );
        let query_key = playback_provider_query_cache_key(
            provider,
            &prefix,
            query,
            result_limit,
            settings,
            native_enabled,
        );
        let load_rows = || async {
            let mut args = base_args(settings);
            // Defer managed token initialization until a candidate needs extraction.
            args.extend([
                "--flat-playlist".into(),
                "--dump-single-json".into(),
                "--skip-download".into(),
                "--".into(),
                format!("{prefix}:{query}"),
            ]);
            let legacy_cancel = cancel.clone();
            let legacy = || async move {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(format!("{provider}: search timed out"));
                }
                if let Some(shared) = shared_deadline {
                    run_tool_until(
                        settings,
                        args,
                        legacy_cancel.clone(),
                        shared.cap(remaining),
                        |_| {},
                    )
                    .await
                } else {
                    run_tool(settings, args, legacy_cancel.clone(), remaining, |_| {}).await
                }
            };
            let raw = if native_enabled {
                let native = crate::youtube_web_search::search_candidates(
                    query,
                    cancel.clone(),
                    crate::playback_sources::PlaybackDeadline::at(
                        shared_deadline.unwrap().cap(remaining),
                    ),
                );
                crate::youtube_web_search::search_or_fallback(
                    native,
                    legacy,
                    cancel.clone(),
                    result_limit,
                )
                .await?
            } else {
                legacy().await?
            };
            let info = serde_json::from_str::<Value>(&raw)
                .map_err(|error| format!("{provider}: invalid metadata: {error}"))?;
            let Some(entries) = info["entries"].as_array() else {
                return Ok(ProviderQueryResponse {
                    candidates: Vec::new(),
                    completeness: ProviderQueryCompleteness::Partial { row_count: 0 },
                });
            };
            let row_count = entries.len();
            let completeness = ProviderQueryCompleteness::ReturnedPage {
                row_count,
                reached_limit: row_count >= result_limit,
            };
            let candidates = entries
                .iter()
                .take(result_limit)
                .filter_map(|entry| {
                    let mut candidate = candidate_from_json(entry, provider);
                    if provider == "youtube" {
                        candidate.url = playback_youtube_search_url(&candidate)?;
                    }
                    (!candidate.id.ends_with(':')
                        && validate_source_url(&candidate.url, provider).is_ok())
                    .then_some(candidate)
                })
                .collect::<Vec<_>>();
            Ok(ProviderQueryResponse {
                candidates,
                completeness,
            })
        };
        let loaded = if let Some(cache) = provider_query_cache {
            cache
                .get_or_search_provider_query(query_key, cancel.clone(), load_rows)
                .await
        } else {
            load_rows().await
        };
        match loaded {
            Ok(response) => {
                if matches!(
                    &response.completeness,
                    ProviderQueryCompleteness::Partial { .. }
                ) {
                    errors.push(format!("{provider}: incomplete search response"));
                }
                for mut candidate in response.candidates {
                    matching::score_candidate(track, &mut candidate);
                    candidate
                        .reasons
                        .push("Download permission has not been established".into());
                    candidates.push(candidate);
                }
            }
            Err(error) if error == "Cancelled" || *cancel.borrow() => {
                return (candidates, vec!["Cancelled".into()]);
            }
            Err(error) => errors.push(error),
        }
        // Stop query expansion only when the shared evaluator considers a
        // result inspectable or acceptable at Search stage.
        if candidates.iter().any(|candidate| {
            matches!(
                crate::recording_match::assess(
                    track,
                    candidate,
                    settings.matching_level,
                    crate::recording_match::MatchStage::Search,
                )
                .action,
                crate::recording_match::MatchAction::Inspect
                    | crate::recording_match::MatchAction::Accept
            )
        }) {
            break;
        }
    }
    (candidates, errors)
}

/// Normalize only the bare-video-ID form of a flat YouTube search result.
/// An explicit URL must still be a permitted video URL bound to the same ID.
/// This is retrieval parsing; recording/title/version/duration gates are unchanged.
fn playback_youtube_search_url(candidate: &Candidate) -> Option<String> {
    if candidate.provider != "youtube" {
        return None;
    }
    let id = candidate.id.strip_prefix("youtube:")?;
    let raw = candidate.url.as_str();
    if raw.is_empty() {
        // Retain the existing missing-URL fallback, including its later URL/ID
        // validation in search_provider_candidates.
        return Some(format!("https://www.youtube.com/watch?v={id}"));
    }
    let canonical_id = id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if canonical_id && raw == id {
        return Some(format!("https://www.youtube.com/watch?v={id}"));
    }
    let url = validate_source_url(raw, "youtube").ok()?;
    let actual_id = match url.host_str()? {
        "youtube.com" | "www.youtube.com" | "music.youtube.com" if url.path() == "/watch" => url
            .query_pairs()
            .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))?,
        "youtu.be" | "www.youtu.be" => {
            let mut segments = url.path_segments()?;
            let first = segments.next()?;
            if first.is_empty() || segments.any(|segment| !segment.is_empty()) {
                return None;
            }
            first.to_owned()
        }
        _ => return None,
    };
    (actual_id == id).then(|| raw.to_owned())
}

fn metadata_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => Some(text.trim().to_owned()),
        Value::Array(values) => {
            if values.is_empty() || values.len() > 32 {
                return None;
            }
            let artists = values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty() && name.len() <= 512)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(artists.join(", "))
        }
        _ => None,
    }?;
    (!text.is_empty()).then_some(text)
}

fn split_source_credit_title(value: &str) -> Option<(String, String)> {
    let separators = [" — ", " – ", " - "];
    let matches = separators
        .iter()
        .filter_map(|separator| value.find(*separator).map(|index| (index, *separator)))
        .collect::<Vec<_>>();
    let [(index, separator)] = matches.as_slice() else {
        return None;
    };
    let artist = value[..*index].trim();
    let title = value[*index + separator.len()..].trim();
    (!artist.is_empty() && !title.is_empty()).then(|| (artist.to_owned(), title.to_owned()))
}

fn raw_search_track(candidate: Candidate, info: &Value) -> Track {
    let provider_id = info["id"].as_str().unwrap_or_default().trim().to_owned();
    let provided_artist =
        metadata_text(&info["artists"]).or_else(|| metadata_text(&info["artist"]));
    let provided_title = metadata_text(&info["track"]);
    let raw_title = provided_title
        .clone()
        .or_else(|| metadata_text(&info["title"]))
        .unwrap_or_default();
    let (artist, title, artist_source) = if let Some(artist) = provided_artist {
        (artist, raw_title, "metadata")
    } else if let Some((artist, title)) = split_source_credit_title(&raw_title) {
        (artist, title, "inferred_from_title")
    } else {
        (String::new(), raw_title, "unknown")
    };
    let provenance_candidate = serde_json::to_value(&candidate).unwrap_or(Value::Null);
    let mut track = Track {
        id: format!("{}:{provider_id}", candidate.provider),
        provider: candidate.provider.clone(),
        provider_id,
        source_url: candidate.url.clone(),
        artist,
        title: title.clone(),
        album: metadata_text(&info["album"]),
        genre: metadata_text(&info["genre"]),
        isrc: info["isrc"]
            .as_str()
            .and_then(crate::recording_identity::normalize_isrc),
        duration_seconds: candidate.duration_seconds,
        release_date: metadata_text(&info["release_date"]),
        artwork_url: candidate.artwork_url.clone(),
        provenance: serde_json::json!({
            "sourceKind": "audioSearch",
            "identity": "unverified source search result",
            "artistSource": artist_source,
            "parsedTitle": crate::recording_identity::parse_title(&title),
            "weakFields": {"artist": artist_source != "metadata", "title": provided_title.is_none()},
            "uploader": candidate.uploader,
            "uploadDate": metadata_text(&info["upload_date"]),
            "audioCandidate": provenance_candidate,
            "remoteCatalog": candidate.source_metadata,
        }),
        ..Track::default()
    };
    if candidate.provider == "youtube" {
        crate::metadata_normalization::normalize_selected_track(
            &mut track,
            &candidate.title,
            &candidate.url,
        );
    }
    crate::recording_identity::hydrate_identifiers(&mut track);
    track
}

fn youtube_search_results_url(query: &str) -> String {
    let mut url = url::Url::parse("https://www.youtube.com/results")
        .expect("static YouTube results URL is valid");
    url.query_pairs_mut().append_pair("search_query", query);
    url.into()
}

/// Return a canonical watch URL only when yt-dlp identified a real video URL.
/// Search rows can also contain channel or playlist entries with unrelated IDs;
/// those must not be turned into fabricated watch URLs.
fn youtube_search_video_url(info: &Value, candidate: &Candidate) -> Option<String> {
    let raw_url = ["webpage_url", "original_url", "url"]
        .iter()
        .filter_map(|key| info[*key].as_str())
        .find(|value| !value.trim().is_empty())?;
    let video_id = youtube_video_id(raw_url)?;
    (candidate.id.strip_prefix("youtube:") == Some(video_id.as_str()))
        .then(|| format!("https://www.youtube.com/watch?v={video_id}"))
}

async fn search_audio_tracks_raw(
    query: &str,
    provider_ids: &[String],
    settings: &Settings,
) -> Result<Vec<Track>, String> {
    let (_tx, cancel) = idle_cancel();
    let mut tracks = Vec::new();
    let mut errors = Vec::new();
    for provider in provider_ids {
        let search_input = match provider.as_str() {
            "youtube" => youtube_search_results_url(query),
            "soundcloud" => format!("scsearch50:{query}"),
            _ => continue,
        };
        let mut args = base_args(settings);
        // Flat searches must remain usable before the extraction helper is ready.
        // The results URL has no ytsearchN prefix, so keep its legacy 50-row cap explicit.
        if provider == "youtube" {
            args.extend(["--playlist-end".into(), "50".into()]);
        }
        args.extend([
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--skip-download".into(),
            "--".into(),
            search_input,
        ]);
        match run_tool(
            settings,
            args,
            cancel.clone(),
            Duration::from_secs(20),
            |_| {},
        )
        .await
        {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(info) => {
                    if let Some(entries) = info["entries"].as_array() {
                        for entry in entries.iter().take(50) {
                            let mut candidate = candidate_from_json(entry, provider);
                            if provider == "youtube" {
                                let Some(video_url) = youtube_search_video_url(entry, &candidate)
                                else {
                                    continue;
                                };
                                candidate.url = video_url;
                            }
                            if validate_source_url(&candidate.url, provider).is_err()
                                || candidate.id.ends_with(':')
                            {
                                continue;
                            }
                            candidate.score = 0.8;
                            candidate.confidence = "review".into();
                            candidate.reasons = vec![
                                "Direct source result; track identity has not been verified".into(),
                                "Download permission has not been established".into(),
                            ];
                            tracks.push(raw_search_track(candidate, entry));
                        }
                    }
                }
                Err(error) => errors.push(format!("{provider}: invalid metadata: {error}")),
            },
            Err(error) => errors.push(format!("{provider}: {error}")),
        }
    }
    if tracks.is_empty() && !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Ok(tracks)
    }
}

async fn search_audio_tracks_page_raw(
    query: &str,
    provider: &str,
    offset: usize,
    limit: usize,
    settings: &Settings,
) -> Result<AudioSearchPage, String> {
    let search_input = match provider {
        "youtube" => youtube_search_results_url(query),
        "soundcloud" => format!("scsearchall:{query}"),
        _ => return Err("Choose YouTube or SoundCloud for audio search".into()),
    };
    if offset >= 10_000 || limit == 0 || limit > 30 || offset + limit > 10_000 {
        return Err("Audio search page is outside the supported range".into());
    }
    let start = offset + 1;
    let end = offset + limit;
    let mut args = base_args(settings);
    args.extend([
        "--flat-playlist".into(),
        "--playlist-start".into(),
        start.to_string(),
        "--playlist-end".into(),
        end.to_string(),
        "--dump-single-json".into(),
        "--skip-download".into(),
        "--".into(),
        search_input,
    ]);
    let (_tx, cancel) = idle_cancel();
    let raw = run_tool(settings, args, cancel, Duration::from_secs(20), |_| {}).await?;
    let info: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("{provider}: invalid metadata: {error}"))?;
    let mut tracks = Vec::new();
    let mut entry_count = 0;
    if let Some(entries) = info["entries"].as_array() {
        entry_count = entries.len().min(limit);
        for entry in entries.iter().take(limit) {
            let mut candidate = candidate_from_json(entry, provider);
            if provider == "youtube" {
                let Some(video_url) = youtube_search_video_url(entry, &candidate) else {
                    continue;
                };
                candidate.url = video_url;
            }
            if validate_source_url(&candidate.url, provider).is_err() || candidate.id.ends_with(':')
            {
                continue;
            }
            candidate.score = 0.8;
            candidate.confidence = "review".into();
            candidate.reasons = vec![
                "Direct source result; track identity has not been verified".into(),
                "Download permission has not been established".into(),
            ];
            tracks.push(raw_search_track(candidate, entry));
        }
    }
    Ok(AudioSearchPage {
        tracks,
        entry_count,
    })
}

fn playback_search_queries(track: &Track, alternate_artist: Option<&str>) -> Vec<String> {
    let query = search_query(track, true);
    let fallback_query = search_query(track, false);
    let mut queries = vec![query.clone()];
    // Keep the existing two-call provider bound. A trusted cached identity
    // variant occupies the old fallback slot and keeps the requested version.
    if let Some(alias_query) = alternate_artist
        .map(|artist| search_query_for_artist(track, artist, true))
        .filter(|candidate| !candidate.is_empty() && candidate != &query)
    {
        queries.push(alias_query);
    } else if !fallback_query.is_empty() && fallback_query != query {
        queries.push(fallback_query);
    }
    // When an ordinary known-duration query has no other fallback, reserve
    // its existing second slot for a precision query. It is issued only if
    // the first page lacks a full recording match (provisionals are weaker).
    if queries.len() == 1 && track.duration_seconds.is_some() {
        let artist = matching::normalize(&track.artist);
        let title = matching::normalize(&track.title);
        if !artist.is_empty() && !title.is_empty() {
            queries.push(format!("\"{artist}\" \"{title}\""));
        }
    }
    if queries.len() == 1 {
        let folded = crate::recording_identity::search_fold(&query);
        if folded != matching::normalize(&query) && !folded.is_empty() {
            queries.push(folded);
        }
    }
    queries.truncate(2);
    queries
}

fn search_query(track: &Track, include_version: bool) -> String {
    search_query_for_artist(track, &track.artist, include_version)
}

fn search_query_for_artist(track: &Track, artist: &str, include_version: bool) -> String {
    let mut parts = vec![
        matching::normalize(artist),
        matching::normalize(&track.title),
    ];
    if include_version {
        let version = matching::normalize_version_for_title(
            &track.title,
            track.version.as_deref().unwrap_or(""),
        );
        if !version.is_empty() && !matches!(version.as_str(), "original" | "original mix") {
            parts.push(version);
        }
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
struct CachedCandidates {
    cached_at: Instant,
    candidates: Vec<Candidate>,
}

#[derive(Clone)]
struct CachedProviderQueryRows {
    cached_at: Instant,
    response: ProviderQueryResponse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProviderQueryCompleteness {
    /// A valid response page. Whether it filled the requested window is
    /// recorded separately; neither case proves the provider had no more rows.
    ReturnedPage {
        row_count: usize,
        reached_limit: bool,
    },
    /// Use only when the adapter has authoritative evidence that no further
    /// rows exist. Current search endpoints do not expose that evidence.
    Exhaustive { row_count: usize },
    /// The response could not be parsed into a trustworthy complete window.
    Partial { row_count: usize },
}

impl ProviderQueryCompleteness {
    fn cacheable(&self) -> bool {
        !matches!(self, Self::Partial { .. })
    }
}

#[derive(Clone, Debug)]
struct ProviderQueryResponse {
    candidates: Vec<Candidate>,
    completeness: ProviderQueryCompleteness,
}

#[derive(Clone)]
struct CachedPreviewAudio {
    cached_at: Instant,
    ttl: Duration,
    audio: PreviewAudio,
}

#[derive(Clone)]
struct CachedTracks {
    cached_at: Instant,
    tracks: Vec<Track>,
    entry_count: usize,
}

#[derive(Clone)]
struct CachedChapterSource {
    cached_at: Instant,
    path: PathBuf,
    cache_root: PathBuf,
    staging_root: PathBuf,
}

#[derive(Default)]
struct ChapterSourceCacheState {
    entries: HashMap<String, CachedChapterSource>,
    in_flight: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

#[derive(Default)]
struct ChapterSourceCache {
    state: Mutex<ChapterSourceCacheState>,
}

struct ChapterSourceCacheGate {
    cache: Arc<ChapterSourceCache>,
    key: String,
    gate: Arc<tokio::sync::Mutex<()>>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for ChapterSourceCacheGate {
    fn drop(&mut self) {
        self.guard.take();
        if let Ok(mut state) = self.cache.state.lock() {
            if state.in_flight.get(&self.key).is_some_and(|current| {
                Arc::ptr_eq(current, &self.gate) && Arc::strong_count(current) == 2
            }) {
                state.in_flight.remove(&self.key);
            }
        }
    }
}

impl ChapterSourceCache {
    async fn gate(self: &Arc<Self>, key: &str) -> Result<ChapterSourceCacheGate, String> {
        let gate = self
            .state
            .lock()
            .map_err(|_| "Chapter source cache lock poisoned")?
            .in_flight
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.clone().lock_owned().await;
        Ok(ChapterSourceCacheGate {
            cache: self.clone(),
            key: key.to_owned(),
            gate,
            guard: Some(guard),
        })
    }

    async fn copy_to_staging(&self, key: &str, staging: &Path) -> Option<PathBuf> {
        let (cached, expired) = {
            let mut state = self.state.lock().ok()?;
            let expired_keys = state
                .entries
                .iter()
                .filter(|(_, value)| value.cached_at.elapsed() >= CHAPTER_SOURCE_CACHE_TTL)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            let expired = expired_keys
                .into_iter()
                .filter_map(|key| state.entries.remove(&key).map(|value| value.path))
                .collect::<Vec<_>>();
            (state.entries.get(key).cloned(), expired)
        };
        for path in expired {
            let _ = tokio::fs::remove_file(path).await;
        }
        let cached = cached?;
        let result = async {
            let real_root = tokio::fs::canonicalize(&cached.cache_root).await.ok()?;
            let real_staging_parent = tokio::fs::canonicalize(&cached.staging_root).await.ok()?;
            if !real_root.starts_with(&real_staging_parent) {
                return None;
            }
            let real_source = tokio::fs::canonicalize(&cached.path).await.ok()?;
            if !real_source.starts_with(&real_root)
                || !tokio::fs::metadata(&real_source).await.ok()?.is_file()
            {
                return None;
            }
            let real_staging = tokio::fs::canonicalize(staging).await.ok()?;
            if !real_staging.starts_with(&real_staging_parent) {
                return None;
            }
            let extension = safe_audio_extension(&real_source);
            let target = real_staging.join(format!("chapter-source.{extension}"));
            if tokio::fs::copy(&real_source, &target).await.is_err() {
                let _ = tokio::fs::remove_file(&target).await;
                return None;
            }
            let real_target = tokio::fs::canonicalize(&target).await.ok()?;
            if !real_target.starts_with(&real_staging)
                || !tokio::fs::metadata(&real_target).await.ok()?.is_file()
            {
                let _ = tokio::fs::remove_file(&target).await;
                return None;
            }
            Some(real_target)
        }
        .await;
        if result.is_none() {
            if let Ok(mut state) = self.state.lock() {
                state.entries.remove(key);
            }
            let _ = tokio::fs::remove_file(cached.path).await;
        }
        result
    }

    async fn insert_from_staging(
        &self,
        key: &str,
        source: &Path,
        job_staging: &Path,
        music_directory: &Path,
    ) {
        let cache_root = music_directory
            .join(".burrata-staging")
            .join(".chapter-source-cache");
        if tokio::fs::create_dir_all(&cache_root).await.is_err() {
            return;
        }
        let Some(real_cache_root) = tokio::fs::canonicalize(&cache_root).await.ok() else {
            return;
        };
        let staging_root = music_directory.join(".burrata-staging");
        let Some(real_staging_parent) = tokio::fs::canonicalize(&staging_root).await.ok() else {
            return;
        };
        if !real_cache_root.starts_with(&real_staging_parent) {
            return;
        }
        let Some(real_source) = tokio::fs::canonicalize(source).await.ok() else {
            return;
        };
        let Some(real_job_staging) = tokio::fs::canonicalize(job_staging).await.ok() else {
            return;
        };
        if !real_job_staging.starts_with(&real_staging_parent)
            || !real_source.starts_with(&real_job_staging)
            || !tokio::fs::metadata(&real_source)
                .await
                .is_ok_and(|m| m.is_file())
        {
            return;
        }
        let extension = safe_audio_extension(&real_source);
        let target =
            real_cache_root.join(format!("{}.{}", chapter_source_key_hash(key), extension));
        let temp = real_cache_root.join(format!(
            "{}.{}.tmp",
            chapter_source_key_hash(key),
            Uuid::new_v4()
        ));
        if tokio::fs::copy(&real_source, &temp).await.is_err() {
            let _ = tokio::fs::remove_file(temp).await;
            return;
        }
        let _ = tokio::fs::remove_file(&target).await;
        if tokio::fs::rename(&temp, &target).await.is_err() {
            let _ = tokio::fs::remove_file(temp).await;
            return;
        }
        let mut evicted = Vec::new();
        if let Ok(mut state) = self.state.lock() {
            let expired_keys = state
                .entries
                .iter()
                .filter(|(_, value)| value.cached_at.elapsed() >= CHAPTER_SOURCE_CACHE_TTL)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            evicted.extend(
                expired_keys
                    .into_iter()
                    .filter_map(|key| state.entries.remove(&key).map(|value| value.path)),
            );
            if let Some(previous) = state.entries.remove(key) {
                evicted.push(previous.path);
            }
            state.entries.insert(
                key.to_owned(),
                CachedChapterSource {
                    cached_at: Instant::now(),
                    path: target.clone(),
                    cache_root: real_cache_root.clone(),
                    staging_root: real_staging_parent.clone(),
                },
            );
            while state.entries.len() > CHAPTER_SOURCE_CACHE_MAX {
                let Some(oldest) = state
                    .entries
                    .iter()
                    .min_by_key(|(_, value)| value.cached_at)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                if let Some(entry) = state.entries.remove(&oldest) {
                    evicted.push(entry.path);
                }
            }
        }
        for path in evicted {
            if path != target {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
    }
}

fn safe_audio_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 12
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .unwrap_or("media")
        .to_ascii_lowercase()
}

fn chapter_source_key_hash(key: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Default)]
struct PreviewSearchState {
    latest_generation: AtomicU64,
    active: Mutex<Option<(u64, watch::Sender<bool>)>>,
}

impl PreviewSearchState {
    fn begin(&self, generation: u64) -> Result<watch::Receiver<bool>, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Preview search lock poisoned")?;
        let latest = self.latest_generation.load(Ordering::SeqCst);
        if generation < latest
            || (generation == latest
                && active
                    .as_ref()
                    .is_some_and(|(active_generation, _)| *active_generation == generation))
        {
            return Err("Preview search superseded by a newer request".into());
        }
        if let Some((_, previous)) = active.take() {
            let _ = previous.send(true);
        }
        self.latest_generation.store(generation, Ordering::SeqCst);
        let (tx, rx) = idle_cancel();
        *active = Some((generation, tx));
        Ok(rx)
    }

    fn finish(&self, generation: u64) -> Result<bool, String> {
        let current = self.latest_generation.load(Ordering::SeqCst) == generation;
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Preview search lock poisoned")?;
        if active
            .as_ref()
            .is_some_and(|(active_generation, _)| *active_generation == generation)
        {
            active.take();
        }
        Ok(current)
    }
}

#[derive(Default)]
struct CandidateSearchCache {
    entries: Mutex<HashMap<String, CachedCandidates>>,
    provider_query_entries: Mutex<HashMap<String, CachedProviderQueryRows>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct InspectionCache {
    entries: Mutex<HashMap<String, (Instant, Candidate)>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

static INSPECTION_CACHE: std::sync::OnceLock<InspectionCache> = std::sync::OnceLock::new();

fn inspection_cache_key(candidate: &Candidate, settings: &Settings) -> String {
    serde_json::json!([
        INSPECTION_PROFILE,
        candidate.provider,
        candidate.url,
        crate::youtube_support::active_executable().to_string_lossy(),
        settings.yt_dlp_path,
        settings.js_runtime_path
    ])
    .to_string()
}

impl InspectionCache {
    async fn get_or_inspect<F, Fut>(&self, key: String, inspect: F) -> Result<Candidate, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Candidate, String>>,
    {
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Inspection cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.lock().await;
        if let Some(cached) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, (at, _)| at.elapsed() < INSPECTION_CACHE_TTL);
            entries.get(&key).map(|(_, candidate)| candidate.clone())
        }) {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Ok(cached);
        }
        let candidate = match inspect().await {
            Ok(candidate) => candidate,
            Err(error) => {
                drop(guard);
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, (at, _)| at.elapsed() < INSPECTION_CACHE_TTL);
            if entries.len() >= INSPECTION_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, (at, _))| at)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(key.clone(), (Instant::now(), candidate.clone()));
        }
        drop(guard);
        remove_flight_if_unused(&self.in_flight, &key, &gate);
        Ok(candidate)
    }
}

struct PreviewAudioFlight {
    gate: Arc<tokio::sync::Mutex<()>>,
    trace_id: uuid::Uuid,
    invalidated: AtomicBool,
}

#[derive(Default)]
struct PreviewAudioCache {
    // Only a cache keyed by the exact signed URL can suppress an access denial.
    // An extractor 403 does not prove that its client/format profile is broken.
    cooldown_signed_stream_denials: bool,
    entries: Mutex<HashMap<String, CachedPreviewAudio>>,
    cooldowns: Mutex<HashMap<String, (Instant, String)>>,
    in_flight: Mutex<HashMap<String, Arc<PreviewAudioFlight>>>,
}

#[derive(Default)]
struct RawAudioSearchCache {
    entries: Mutex<HashMap<String, CachedTracks>>,
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RawAudioSearchCache {
    async fn get_or_search<F, Fut>(&self, key: String, search: F) -> Result<Vec<Track>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Track>, String>>,
    {
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Audio search cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.lock().await;
        if let Some(tracks) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, value| value.cached_at.elapsed() < RAW_AUDIO_SEARCH_CACHE_TTL);
            entries.get(&key).map(|value| value.tracks.clone())
        }) {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Ok(tracks);
        }
        let tracks = match search().await {
            Ok(tracks) => tracks,
            Err(error) => {
                drop(guard);
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, value| value.cached_at.elapsed() < RAW_AUDIO_SEARCH_CACHE_TTL);
            if entries.len() >= RAW_AUDIO_SEARCH_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, value)| value.cached_at)
                    .map(|(key, _)| key.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(
                key.clone(),
                CachedTracks {
                    cached_at: Instant::now(),
                    tracks: tracks.clone(),
                    entry_count: tracks.len(),
                },
            );
        }
        drop(guard);
        remove_flight_if_unused(&self.in_flight, &key, &gate);
        Ok(tracks)
    }

    async fn get_or_search_page<F, Fut>(
        &self,
        key: String,
        search: F,
    ) -> Result<AudioSearchPage, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<AudioSearchPage, String>>,
    {
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Audio search cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = gate.lock().await;
        if let Some(page) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, value| value.cached_at.elapsed() < RAW_AUDIO_SEARCH_CACHE_TTL);
            entries.get(&key).map(|value| AudioSearchPage {
                tracks: value.tracks.clone(),
                entry_count: value.entry_count,
            })
        }) {
            drop(guard);
            remove_flight_if_unused(&self.in_flight, &key, &gate);
            return Ok(page);
        }
        let page = match search().await {
            Ok(page) => page,
            Err(error) => {
                drop(guard);
                remove_flight_if_unused(&self.in_flight, &key, &gate);
                return Err(error);
            }
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, value| value.cached_at.elapsed() < RAW_AUDIO_SEARCH_CACHE_TTL);
            if entries.len() >= RAW_AUDIO_SEARCH_CACHE_MAX {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, value)| value.cached_at)
                    .map(|(key, _)| key.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(
                key.clone(),
                CachedTracks {
                    cached_at: Instant::now(),
                    tracks: page.tracks.clone(),
                    entry_count: page.entry_count,
                },
            );
        }
        drop(guard);
        remove_flight_if_unused(&self.in_flight, &key, &gate);
        Ok(page)
    }
}

impl PreviewAudioCache {
    fn for_signed_stream_access() -> Self {
        Self {
            cooldown_signed_stream_denials: true,
            ..Self::default()
        }
    }

    fn invalidate(&self, key: &str) {
        self.invalidate_matching(|entry| entry == key);
    }

    fn invalidate_matching(&self, matches: impl Fn(&str) -> bool) {
        if let Ok(mut flights) = self.in_flight.lock() {
            flights.retain(|key, flight| {
                if matches(key) {
                    flight.invalidated.store(true, Ordering::SeqCst);
                    false
                } else {
                    true
                }
            });
        }
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|key, _| !matches(key));
        }
    }

    fn profile_cooldown(&self, key: &str) -> Option<String> {
        let mut cooldowns = self.cooldowns.lock().ok()?;
        cooldowns.retain(|_, (at, _)| at.elapsed() < PLAYBACK_PROFILE_COOLDOWN);
        cooldowns.get(key).map(|(_, error)| error.clone())
    }

    fn remember_profile_failure(&self, key: &str, error: &str) {
        use crate::playback_failure::FailureCategory;
        let failure = crate::playback_failure::classify(error);
        if !failure.retryable
            || failure.scope != crate::playback_failure::FailureScope::Source
            || !(failure.category == FailureCategory::UnsupportedAudio
                || (self.cooldown_signed_stream_denials
                    && failure.category == FailureCategory::SourceAccess))
        {
            return;
        }
        if let Ok(mut cooldowns) = self.cooldowns.lock() {
            cooldowns.retain(|_, (at, _)| at.elapsed() < PLAYBACK_PROFILE_COOLDOWN);
            if cooldowns.len() >= PLAYBACK_PROFILE_COOLDOWN_MAX {
                if let Some(oldest) = cooldowns
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(key, _)| key.clone())
                {
                    cooldowns.remove(&oldest);
                }
            }
            // These are fixed diagnostics; never retain raw extractor output.
            let safe_error = crate::playback_failure::safe_extractor_error(error);
            cooldowns.insert(key.to_owned(), (Instant::now(), safe_error));
        }
    }

    async fn get_or_resolve<F, Fut>(&self, key: String, resolve: F) -> Result<PreviewAudio, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<PreviewAudio, String>>,
    {
        let (_tx, cancel) = idle_cancel();
        self.get_or_resolve_cancelled(key, cancel, None, |_| resolve())
            .await
    }

    async fn get_or_resolve_cancelled<F, Fut>(
        &self,
        key: String,
        mut cancel: watch::Receiver<bool>,
        trace: Option<crate::playback_sources::SourceTrace>,
        resolve: F,
    ) -> Result<PreviewAudio, String>
    where
        F: FnOnce(uuid::Uuid) -> Fut,
        Fut: std::future::Future<Output = Result<PreviewAudio, String>>,
    {
        if *cancel.borrow() {
            if let Some(trace) = &trace {
                trace.event_scoped(
                    "preview_audio_lookup_cancelled",
                    serde_json::json!({"stage":"before_lookup"}),
                );
            }
            return Err("Cancelled".into());
        }
        let (flight, created) = {
            let mut flights = self
                .in_flight
                .lock()
                .map_err(|_| "Preview audio cache lock poisoned")?;
            if let Some(flight) = flights.get(&key) {
                (flight.clone(), false)
            } else {
                let flight = Arc::new(PreviewAudioFlight {
                    gate: Arc::new(tokio::sync::Mutex::new(())),
                    trace_id: uuid::Uuid::new_v4(),
                    invalidated: AtomicBool::new(false),
                });
                flights.insert(key.clone(), flight.clone());
                (flight, true)
            }
        };
        if let Some(trace) = &trace {
            trace.event_scoped(
                if created {
                    "preview_audio_lookup_start"
                } else {
                    "preview_audio_lookup_join"
                },
                serde_json::json!({"sharedLookupId":flight.trace_id.to_string()}),
            );
        }
        let gate = flight.gate.clone();
        let guard = tokio::select! {
            guard = gate.lock() => guard,
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => {
                if let Some(trace) = &trace {
                    trace.event_scoped("preview_audio_lookup_cancelled", serde_json::json!({"sharedLookupId":flight.trace_id.to_string(),"stage":"waiting_for_worker"}));
                }
                remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
                return Err("Cancelled".into());
            }
        };
        if *cancel.borrow() {
            drop(guard);
            if let Some(trace) = &trace {
                trace.event_scoped("preview_audio_lookup_cancelled", serde_json::json!({"sharedLookupId":flight.trace_id.to_string(),"stage":"after_wait"}));
            }
            remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
            return Err("Cancelled".into());
        }
        if flight.invalidated.load(Ordering::SeqCst) {
            drop(guard);
            remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
            return Err("Audio stream was invalidated during resolution".into());
        }
        if let Some(cached) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, v| v.cached_at.elapsed() < v.ttl);
            entries.get(&key).map(|v| v.audio.clone())
        }) {
            if let Some(trace) = &trace {
                trace.event_scoped(
                    "preview_audio_cache_hit",
                    serde_json::json!({"sharedLookupId":flight.trace_id.to_string()}),
                );
            }
            drop(guard);
            remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
            if flight.invalidated.load(Ordering::SeqCst) || !cached.provider_session_current() {
                return Err("Audio stream was invalidated during resolution".into());
            }
            return Ok(cached);
        }
        if let Some(error) = self.profile_cooldown(&key) {
            if let Some(trace) = &trace {
                trace.event_scoped(
                    "preview_audio_profile_cooldown",
                    serde_json::json!({"sharedLookupId":flight.trace_id.to_string()}),
                );
            }
            drop(guard);
            remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
            return Err(error);
        }
        if let Some(trace) = &trace {
            trace.event_scoped(
                "preview_audio_cache_miss",
                serde_json::json!({"sharedLookupId":flight.trace_id.to_string()}),
            );
        }
        let audio = match tokio::select! {
            result = resolve(flight.trace_id) => result,
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        } {
            Ok(audio) => audio,
            Err(error) => {
                if !flight.invalidated.load(Ordering::SeqCst) {
                    self.remember_profile_failure(&key, &error);
                }
                if let Some(trace) = &trace {
                    trace.event_scoped("preview_audio_lookup_end", serde_json::json!({"sharedLookupId":flight.trace_id.to_string(),"ok":false,"cancelled":error == "Cancelled"}));
                }
                drop(guard);
                remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
                return Err(error);
            }
        };
        if let Some(trace) = &trace {
            trace.event_scoped(
                "preview_audio_lookup_end",
                serde_json::json!({"sharedLookupId":flight.trace_id.to_string(),"ok":true}),
            );
        }
        if !audio.provider_session_current() {
            drop(guard);
            remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
            return Err("Audio source provider session changed during resolution".into());
        }
        if let Ok(mut entries) = self.entries.lock() {
            // A failed URL may be invalidated while its older lookup is still running.
            // Never let that lookup restore an entry behind a fresh-stream retry.
            if !flight.invalidated.load(Ordering::SeqCst) {
                entries.retain(|_, v| v.cached_at.elapsed() < v.ttl);
                if entries.len() >= PREVIEW_AUDIO_CACHE_MAX {
                    if let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, v)| v.cached_at)
                        .map(|(k, _)| k.clone())
                    {
                        entries.remove(&oldest);
                    }
                }
                entries.insert(
                    key.clone(),
                    CachedPreviewAudio {
                        cached_at: Instant::now(),
                        ttl: preview_audio_cache_ttl(&audio.url),
                        audio: audio.clone(),
                    },
                );
            }
        }
        drop(guard);
        remove_preview_flight_if_unused(&self.in_flight, &key, &flight);
        if flight.invalidated.load(Ordering::SeqCst) || !audio.provider_session_current() {
            return Err("Audio stream was invalidated during resolution".into());
        }
        Ok(audio)
    }
}

fn remove_preview_flight_if_unused(
    flights: &Mutex<HashMap<String, Arc<PreviewAudioFlight>>>,
    key: &str,
    flight: &Arc<PreviewAudioFlight>,
) {
    if let Ok(mut in_flight) = flights.lock() {
        if in_flight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, flight) && Arc::strong_count(current) == 2)
        {
            in_flight.remove(key);
        }
    }
}

fn preview_audio_cache_ttl(raw_url: &str) -> Duration {
    preview_audio_cache_ttl_at(raw_url, Utc::now().timestamp())
}

fn preview_audio_cache_ttl_at(raw_url: &str, now: i64) -> Duration {
    let Ok(url) = url::Url::parse(raw_url) else {
        return Duration::ZERO;
    };
    let mut pairs = HashMap::new();
    for (key, value) in url.query_pairs() {
        let key = key.to_ascii_lowercase();
        let is_expiry = matches!(
            key.as_str(),
            "expire"
                | "expires"
                | "x-amz-date"
                | "x-amz-expires"
                | "x-goog-date"
                | "x-goog-expires"
        );
        if pairs.insert(key, value.into_owned()).is_some() && is_expiry {
            // Duplicate signing fields have ambiguous server semantics.
            return Duration::ZERO;
        }
    }
    let mut expiries: Vec<i64> = Vec::new();
    for key in ["expire", "expires"] {
        if let Some(raw) = pairs.get(key) {
            let Ok(expiry) = raw.parse() else {
                return Duration::ZERO;
            };
            expiries.push(expiry);
        }
    }
    // AWS and Google signatures specify a signing timestamp plus a lifetime.
    // An incomplete signature cannot safely be assigned the ordinary cache TTL.
    for prefix in ["x-amz", "x-goog"] {
        let date = pairs.get(&format!("{prefix}-date"));
        let lifetime = pairs.get(&format!("{prefix}-expires"));
        if date.is_some() || lifetime.is_some() {
            let expiry = date
                .and_then(|date| chrono::NaiveDateTime::parse_from_str(date, "%Y%m%dT%H%M%SZ").ok())
                .zip(lifetime.and_then(|value| value.parse::<i64>().ok()))
                .and_then(|(date, lifetime)| {
                    (lifetime >= 0).then(|| date.and_utc().timestamp().saturating_add(lifetime))
                });
            match expiry {
                Some(expiry) => expiries.push(expiry),
                None => return Duration::ZERO,
            }
        }
    }
    let Some(expiry) = expiries.into_iter().min() else {
        return PREVIEW_AUDIO_CACHE_TTL;
    };
    Duration::from_secs(expiry.saturating_sub(now).saturating_sub(30).max(0) as u64)
        .min(PREVIEW_AUDIO_CACHE_TTL)
}

fn remove_flight_if_unused(
    flights: &Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    key: &str,
    gate: &Arc<tokio::sync::Mutex<()>>,
) {
    if let Ok(mut in_flight) = flights.lock() {
        if in_flight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, gate) && Arc::strong_count(current) == 2)
        {
            in_flight.remove(key);
        }
    }
}

impl CandidateSearchCache {
    /// Cache one exact provider request independently from matching level and
    /// recording policy. The namespace shares the existing search owner's
    /// cancellation and singleflight gate, but raw rows cannot alias completed
    /// or flat playback candidate results.
    async fn get_or_search_provider_query<F, Fut>(
        &self,
        key: String,
        cancel: watch::Receiver<bool>,
        load: F,
    ) -> Result<ProviderQueryResponse, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<ProviderQueryResponse, String>>,
    {
        const MAX_PROVIDER_QUERY_ENTRIES: usize = 256;
        const PROVIDER_QUERY_TTL: Duration = Duration::from_secs(5 * 60);
        let key = format!("provider-query:{key}");
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Search cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let guard = tokio::select! {
            guard = gate.lock() => guard,
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => {
                self.remove_flight_if_unused(&key, &gate);
                return Err("Cancelled".into());
            }
        };
        if *cancel.borrow() {
            drop(guard);
            self.remove_flight_if_unused(&key, &gate);
            return Err("Cancelled".into());
        }
        if let Some(response) = self
            .provider_query_entries
            .lock()
            .ok()
            .and_then(|mut entries| {
                entries.retain(|_, row| row.cached_at.elapsed() < PROVIDER_QUERY_TTL);
                entries.get(&key).map(|row| row.response.clone())
            })
        {
            drop(guard);
            self.remove_flight_if_unused(&key, &gate);
            return Ok(response);
        }
        let loaded = tokio::select! {
            biased;
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
            response = load() => response,
        };
        let response = match loaded {
            Ok(response) => response,
            Err(error) => {
                drop(guard);
                self.remove_flight_if_unused(&key, &gate);
                return Err(error);
            }
        };
        if *cancel.borrow() {
            drop(guard);
            self.remove_flight_if_unused(&key, &gate);
            return Err("Cancelled".into());
        }
        if response.completeness.cacheable() {
            if let Ok(mut entries) = self.provider_query_entries.lock() {
                entries.retain(|_, row| row.cached_at.elapsed() < PROVIDER_QUERY_TTL);
                if entries.len() >= MAX_PROVIDER_QUERY_ENTRIES {
                    if let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, row)| row.cached_at)
                        .map(|(key, _)| key.clone())
                    {
                        entries.remove(&oldest);
                    }
                }
                entries.insert(
                    key.clone(),
                    CachedProviderQueryRows {
                        cached_at: Instant::now(),
                        response: response.clone(),
                    },
                );
            }
        }
        drop(guard);
        self.remove_flight_if_unused(&key, &gate);
        Ok(response)
    }

    async fn get_or_search<F, Fut>(&self, key: String, load: F) -> Result<Vec<Candidate>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
    {
        let (_tx, cancel) = idle_cancel();
        self.get_or_search_cancelled(key, cancel, load).await
    }

    async fn get_or_search_cancelled<F, Fut>(
        &self,
        key: String,
        cancel: watch::Receiver<bool>,
        load: F,
    ) -> Result<Vec<Candidate>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
    {
        self.get_or_search_cancelled_observed(key, cancel, None, load)
            .await
    }

    async fn get_or_search_cancelled_traced<F, Fut>(
        &self,
        key: String,
        cancel: watch::Receiver<bool>,
        trace: crate::playback_sources::SourceTrace,
        load: F,
    ) -> Result<Vec<Candidate>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
    {
        self.get_or_search_cancelled_observed(key, cancel, Some(trace), load)
            .await
    }

    async fn get_or_search_cancelled_observed<F, Fut>(
        &self,
        key: String,
        cancel: watch::Receiver<bool>,
        trace: Option<crate::playback_sources::SourceTrace>,
        load: F,
    ) -> Result<Vec<Candidate>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
    {
        self.get_or_search_cancelled_observed_decided(key, cancel, trace, || async {
            load().await.map(|candidates| (candidates, true))
        })
        .await
    }

    async fn get_or_search_cancelled_observed_decided<F, Fut>(
        &self,
        key: String,
        cancel: watch::Receiver<bool>,
        trace: Option<crate::playback_sources::SourceTrace>,
        load: F,
    ) -> Result<Vec<Candidate>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(Vec<Candidate>, bool), String>>,
    {
        self.get_or_search_cancelled_observed_decided_with_completeness(key, cancel, trace, load)
            .await
            .map(|(candidates, _complete)| candidates)
    }

    async fn get_or_search_cancelled_observed_decided_with_completeness<F, Fut>(
        &self,
        key: String,
        cancel: watch::Receiver<bool>,
        trace: Option<crate::playback_sources::SourceTrace>,
        load: F,
    ) -> Result<(Vec<Candidate>, bool), String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(Vec<Candidate>, bool), String>>,
    {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let gate = self
            .in_flight
            .lock()
            .map_err(|_| "Search cache lock poisoned")?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let trace_enabled = trace.as_ref().is_some_and(|trace| trace.is_enabled());
        let (guard, waited_for_gate) = if trace_enabled {
            match gate.try_lock() {
                Ok(guard) => (guard, false),
                Err(_) => {
                    let guard = tokio::select! {
                        guard = gate.lock() => guard,
                        _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => {
                            self.remove_flight_if_unused(&key, &gate);
                            return Err("Cancelled".into());
                        }
                    };
                    (guard, true)
                }
            }
        } else {
            let guard = tokio::select! {
                guard = gate.lock() => guard,
                _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => {
                    self.remove_flight_if_unused(&key, &gate);
                    return Err("Cancelled".into());
                }
            };
            (guard, false)
        };
        if *cancel.borrow() {
            drop(guard);
            self.remove_flight_if_unused(&key, &gate);
            return Err("Cancelled".into());
        }
        if let Some(cached) = self.entries.lock().ok().and_then(|mut entries| {
            entries.retain(|_, v| v.cached_at.elapsed() < SEARCH_CACHE_TTL);
            entries.get(&key).map(|v| v.candidates.clone())
        }) {
            drop(guard);
            self.remove_flight_if_unused(&key, &gate);
            if trace_enabled {
                if let Some(trace) = &trace {
                    trace.event_scoped(
                        if waited_for_gate {
                            "playback_candidate_search_cache_waited_hit"
                        } else {
                            "playback_candidate_search_cache_hit"
                        },
                        serde_json::json!({"resultCount":cached.len()}),
                    );
                }
            }
            // Only complete responses enter this cache. A loaded incomplete
            // response is returned to its caller and retried on the next call.
            return Ok((cached, true));
        }
        let cache_miss_observed_elapsed_ms = trace_enabled
            .then(|| trace.as_ref().map(|trace| trace.elapsed_ms()))
            .flatten();
        let (candidates, cacheable) = match load().await {
            Ok(result) => result,
            Err(error) => {
                drop(guard);
                self.remove_flight_if_unused(&key, &gate);
                if trace_enabled {
                    if let Some(trace) = &trace {
                        trace.event_scoped(
                            "playback_candidate_search_cache_miss",
                            serde_json::json!({
                                "waitedForGate":waited_for_gate,
                                "observedElapsedMs":cache_miss_observed_elapsed_ms
                            }),
                        );
                    }
                }
                return Err(error);
            }
        };
        if cacheable {
            if let Ok(mut entries) = self.entries.lock() {
                entries.retain(|_, v| v.cached_at.elapsed() < SEARCH_CACHE_TTL);
                if entries.len() >= SEARCH_CACHE_MAX {
                    if let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, v)| v.cached_at)
                        .map(|(k, _)| k.clone())
                    {
                        entries.remove(&oldest);
                    }
                }
                entries.insert(
                    key.clone(),
                    CachedCandidates {
                        cached_at: Instant::now(),
                        candidates: candidates.clone(),
                    },
                );
            }
        }
        drop(guard);
        self.remove_flight_if_unused(&key, &gate);
        if trace_enabled {
            if let Some(trace) = &trace {
                trace.event_scoped(
                    "playback_candidate_search_cache_miss",
                    serde_json::json!({
                        "waitedForGate":waited_for_gate,
                        "observedElapsedMs":cache_miss_observed_elapsed_ms
                    }),
                );
            }
        }
        Ok((candidates, cacheable))
    }

    fn remove_flight_if_unused(&self, key: &str, gate: &Arc<tokio::sync::Mutex<()>>) {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            if in_flight.get(key).is_some_and(|current| {
                Arc::ptr_eq(current, gate) && Arc::strong_count(current) == 2
            }) {
                in_flight.remove(key);
            }
        }
    }
}

fn search_cache_key(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    result_limit: usize,
) -> String {
    serde_json::json!([
        track.artist,
        track.title,
        track.version,
        track.duration_seconds,
        provider_ids,
        result_limit,
        settings.yt_dlp_path,
        settings.js_runtime_path,
    ])
    .to_string()
}

fn optional_playback_research_budget(remaining: Duration) -> Duration {
    remaining
        .saturating_sub(PLAYBACK_EXTRACTION_RESERVE)
        .min(OPTIONAL_PLAYBACK_RESEARCH_MAX)
}

/// Complete the flat search before publishing its shared cache entry. The
/// catalog supplies locators only; normal recording/identity gates still apply.
async fn complete_playback_candidate_search<F, Fut>(
    track: &Track,
    providers: &[String],
    _alternate_artist: Option<&str>,
    mut found: Vec<Candidate>,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
    catalog: F,
) -> Result<Vec<Candidate>, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
{
    complete_playback_candidate_search_observed(
        track,
        providers,
        _alternate_artist,
        found,
        cancel,
        deadline,
        catalog,
        None,
    )
    .await
    .map(|(found, _cacheable)| found)
}

/// Diagnostic-only observation point: `found` is the complete flat-search
/// batch, while the optional catalog branch is still pending. This never
/// changes what is cached or returned by the search.
async fn complete_playback_candidate_search_observed<F, Fut>(
    track: &Track,
    providers: &[String],
    _alternate_artist: Option<&str>,
    mut found: Vec<Candidate>,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
    catalog: F,
    observe_flat_batch: Option<Box<dyn FnOnce(Vec<Candidate>) + Send>>,
) -> Result<(Vec<Candidate>, bool), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
{
    complete_playback_candidate_search_observed_with_level(
        track,
        providers,
        _alternate_artist,
        found,
        cancel,
        deadline,
        catalog,
        observe_flat_batch,
        crate::recording_match::MatchLevel::Balanced,
    )
    .await
}

async fn complete_playback_candidate_search_observed_with_level<F, Fut>(
    track: &Track,
    providers: &[String],
    _alternate_artist: Option<&str>,
    mut found: Vec<Candidate>,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
    catalog: F,
    observe_flat_batch: Option<Box<dyn FnOnce(Vec<Candidate>) + Send>>,
    matching_level: crate::recording_match::MatchLevel,
) -> Result<(Vec<Candidate>, bool), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Candidate>, String>>,
{
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Playback candidate search timed out".into());
    }
    let optional_budget = optional_playback_research_budget(deadline.remaining());
    let optional_eligible = providers.len() == 1
        && providers[0] == "youtube"
        && track.playback_range.is_none()
        && !track.provenance["chapters"]
            .as_array()
            .is_some_and(|chapters| !chapters.is_empty())
        && track.version.as_deref().is_none_or(|version| {
            matches!(
                crate::matching::normalize_version(version).as_str(),
                "" | "original" | "original mix"
            )
        })
        && !found.iter().any(|candidate| {
            crate::recording_match::assess(
                track,
                candidate,
                matching_level,
                crate::recording_match::MatchStage::Search,
            )
            .relationship
                != crate::recording_match::MatchRelationship::Unresolved
        });
    let mut cacheable = !optional_eligible;
    if optional_eligible && !optional_budget.is_zero() {
        if let Some(observe) = observe_flat_batch {
            observe(found.clone());
        }
        match tokio::select! {
            biased;
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
            result = tokio::time::timeout(optional_budget, catalog()) => result.unwrap_or_else(|_| Err("Optional playback catalog search timed out".into())),
        } {
            Ok(items) => {
                found.extend(items);
                cacheable = true;
            }
            Err(error) if *cancel.borrow() || error == "Cancelled" => {
                return Err("Cancelled".into())
            }
            Err(_) => cacheable = false, // Keep flat rows available, but retry optional work later.
        }
    }
    Ok((found, cacheable))
}

fn playback_candidate_search_cache_key(
    track: &Track,
    provider_ids: &[String],
    settings: &Settings,
    alternate_artists: &[String],
) -> String {
    format!(
        "{}:playback-source-search:recording-match-{}:{:?}:{}",
        search_cache_key(track, provider_ids, settings, 3),
        crate::recording_match::EVALUATOR_VERSION,
        settings.matching_level,
        serde_json::to_string(alternate_artists).unwrap_or_default(),
    )
}

fn flat_playback_candidate_search_cache_key(
    track: &Track,
    provider: &str,
    settings: &Settings,
    alternate_artists: &[String],
) -> String {
    format!(
        "flat:{}:{}",
        playback_candidate_search_cache_key(
            track,
            &[provider.to_owned()],
            settings,
            alternate_artists,
        ),
        provider
    )
}

fn preview_audio_cache_key(
    candidate: &Candidate,
    settings: &Settings,
    alternate_format: bool,
) -> String {
    serde_json::json!([
        candidate.provider,
        candidate.url,
        settings.yt_dlp_path,
        settings.js_runtime_path,
        alternate_format,
        PLAYBACK_EXTRACTION_PROFILE,
    ])
    .to_string()
}

fn playback_access_cache_key(candidate: &Candidate, audio: &PreviewAudio) -> String {
    format!(
        "{}\n{}\n{}",
        candidate.provider,
        candidate.url,
        serde_json::json!([audio.url, audio.headers, audio.expected_content_length])
    )
}

pub async fn inspect_candidate(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
) -> Result<Candidate, String> {
    let (_tx, cancel) = idle_cancel();
    inspect_with_cancel(track, candidate, settings, cancel).await
}

pub(crate) async fn inspect_candidate_cancelled(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    inspect_with_cancel(track, candidate, settings, cancel).await
}

pub async fn inspect_chapters(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
) -> Result<Vec<Chapter>, String> {
    let inspected = inspect_candidate(track, candidate, settings).await?;
    inspected
        .chapters
        .ok_or_else(|| "This source does not provide chapter metadata".into())
}

fn chapter_artist_title(label: &str) -> (String, String, &'static str) {
    if let Some((separator_at, separator)) = [" — ", " – ", " - "]
        .iter()
        .filter_map(|separator| label.find(*separator).map(|index| (index, *separator)))
        .min_by_key(|(index, _)| *index)
    {
        let (artist, title) = label.split_at(separator_at);
        let title = &title[separator.len()..];
        let artist = artist.trim();
        let title = title.trim();
        if !artist.is_empty() && !title.is_empty() {
            return (artist.to_owned(), title.to_owned(), "chapter_label");
        }
    }
    (String::new(), label.trim().to_owned(), "unknown")
}

fn chapter_track_identity(parent_id: &str, chapter_id: &str) -> (String, String) {
    let provider_id = format!("{parent_id}:{chapter_id}");
    (format!("chapter:{provider_id}"), provider_id)
}

fn verified_chapter_suggestion<'a>(
    chapter: &'a Chapter,
    required_version: Option<&str>,
) -> Option<&'a Track> {
    let track = chapter.suggested_track.as_deref()?;
    let expected_url = format!("https://musicbrainz.org/recording/{}", track.provider_id);
    (chapter.suggestion_confidence.as_deref() == Some("exact_unique_catalog_match")
        && track.provider == "musicbrainz"
        && !track.provider_id.is_empty()
        && track.id == format!("musicbrainz:{}", track.provider_id)
        && track.source_url == expected_url
        && !track.artist.trim().is_empty()
        && !track.title.trim().is_empty()
        && required_version.is_none_or(|version| {
            track.version.as_deref().is_some_and(|catalog_version| {
                matching::normalize_version(catalog_version) == matching::normalize_version(version)
            })
        }))
    .then_some(track)
}

fn preview_operation_deadline(settings: &Settings) -> crate::playback_sources::PlaybackDeadline {
    crate::playback_sources::PlaybackDeadline::after(settings.playback_startup_timeout())
}

async fn resolve_preview_audio_with_options(
    candidate: &Candidate,
    settings: &Settings,
    mut cancel: watch::Receiver<bool>,
    alternate_format: bool,
) -> Result<PreviewAudio, String> {
    resolve_preview_audio_with_trace(
        candidate,
        settings,
        cancel,
        alternate_format,
        None,
        preview_operation_deadline(settings),
    )
    .await
}

async fn extract_album_evidence_preview_audio(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<PreviewAudio, String> {
    extract_preview_audio_with_prepared_alternate(
        candidate,
        settings,
        cancel,
        false,
        None,
        preview_operation_deadline(settings),
        None,
        Some(ALBUM_EVIDENCE_AUDIO_SELECTOR),
    )
    .await
}

async fn resolve_preview_audio_with_trace(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    alternate_format: bool,
    trace: Option<crate::playback_sources::SourceTrace>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<PreviewAudio, String> {
    resolve_preview_audio_with_prepared_alternate(
        candidate,
        settings,
        cancel,
        alternate_format,
        trace,
        deadline,
        None,
    )
    .await
}

async fn resolve_preview_audio_with_prepared_alternate(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    alternate_format: bool,
    trace: Option<crate::playback_sources::SourceTrace>,
    deadline: crate::playback_sources::PlaybackDeadline,
    prepared: Option<Arc<Mutex<Option<PreviewAudio>>>>,
) -> Result<PreviewAudio, String> {
    let audio = extract_preview_audio_with_prepared_alternate(
        candidate,
        settings,
        cancel.clone(),
        alternate_format,
        trace.clone(),
        deadline,
        prepared,
        None,
    )
    .await?;
    if matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
        validate_prepared_preview_audio(audio, cancel, trace, &candidate.provider, deadline).await
    } else {
        Ok(audio)
    }
}

async fn extract_preview_audio_with_prepared_alternate(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
    alternate_format: bool,
    trace: Option<crate::playback_sources::SourceTrace>,
    deadline: crate::playback_sources::PlaybackDeadline,
    prepared: Option<Arc<Mutex<Option<PreviewAudio>>>>,
    format_selector_override: Option<&'static str>,
) -> Result<PreviewAudio, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("yt-dlp operation timed out".into());
    }
    if candidate.provider == "monochrome" {
        crate::monochrome::validate_url(&candidate.url)?;
        let client_span = trace.clone().map(|trace| {
            crate::playback_sources::TraceSpan::begin(
                trace,
                "monochrome",
                "monochrome_client_setup_start",
                "monochrome_client_setup_end",
            )
        });
        let client = match crate::monochrome::Client::new() {
            Ok(client) => {
                if let Some(span) = client_span {
                    span.finish("success");
                }
                client
            }
            Err(error) => {
                if let Some(span) = client_span {
                    span.finish("failed");
                }
                return Err(error);
            }
        };
        let inspected =
            traced_monochrome_probe(trace.clone(), cancel.clone(), || client.inspect(candidate))
                .await?;
        return Ok(PreviewAudio {
            provider_session: None,
            url: inspected.url,
            codec: inspected.formats.first().map(|format| format.codec.clone()),
            bitrate_kbps: inspected.formats.first().and_then(|format| format.bitrate),
            duration_seconds: inspected.duration_seconds,
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: None,
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
            mime_type: if inspected.formats[0].lossless {
                "audio/flac"
            } else {
                "audio/mp4"
            }
            .into(),
        });
    }
    if candidate.provider == "bandcamp" {
        return resolve_bandcamp_preview_audio(candidate, cancel, trace, deadline).await;
    }
    if !matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
        return Err("Audio preview is available only for YouTube and SoundCloud candidates".into());
    }
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let source = validate_source_url(&candidate.url, &candidate.provider)?;
    let dns_started = Instant::now();
    if let Some(trace) = &trace {
        trace.event(
            "dns_source_start",
            &candidate.provider,
            serde_json::json!({}),
        );
    }
    let addresses_result = run_playback_stage(
        cancel.clone(),
        deadline.instant(),
        "yt-dlp operation timed out",
        public_addresses(&source),
    )
    .await;
    if let Some(trace) = &trace {
        trace.event("dns_source_end", &candidate.provider, serde_json::json!({"durationMs":dns_started.elapsed().as_millis(), "ok":addresses_result.is_ok(), "addressCount":addresses_result.as_ref().map_or(0, Vec::len)}));
    }
    let _addresses = addresses_result?;
    // No new helper/broker work when both shared extraction slots are gone.
    // The atomic reservation immediately before process launch remains decisive.
    if trace
        .as_ref()
        .is_some_and(|trace| trace.extraction_attempts_used() >= 2)
    {
        return Err(crate::playback_sources::EXTRACTION_ATTEMPTS_EXHAUSTED.into());
    }
    let mut args = base_args(settings);
    let helper_started = Instant::now();
    if let Some(trace) = &trace {
        trace.event(
            "provider_prepare_start",
            &candidate.provider,
            serde_json::json!({}),
        );
    }
    let helper_result = run_playback_stage(
        cancel.clone(),
        deadline.instant(),
        "yt-dlp operation timed out",
        add_youtube_support_args(
            &mut args,
            &candidate.provider,
            settings,
            Some((deadline.instant(), cancel.clone(), trace.clone())),
        ),
    )
    .await;
    if let Some(trace) = &trace {
        trace.event("provider_prepare_end", &candidate.provider, serde_json::json!({"durationMs":helper_started.elapsed().as_millis(), "ok":helper_result.is_ok()}));
    }
    let (managed_provider_args_installed, mut provider_invocation) = helper_result?;
    // The bundled provider supports token-bearing mobile web playback. Avoid
    // selecting a TV URL that does not use the available GVS token provider.
    if should_force_managed_mweb_for_extraction(
        &candidate.provider,
        &settings.yt_dlp_path,
        managed_provider_args_installed,
        format_selector_override,
    ) {
        args.extend([
            "--extractor-args".into(),
            "youtube:player_client=mweb".into(),
        ]);
    }
    args.extend([
        "--dump-single-json".into(),
        "--no-playlist".into(),
        "--skip-download".into(),
        "--format".into(),
        format_selector_override
            .unwrap_or_else(|| {
                preview_audio_format_selector(alternate_format, candidate.provider == "youtube")
            })
            .into(),
        "--".into(),
        candidate.url.clone(),
    ]);
    if trace
        .as_ref()
        .is_some_and(|trace| !trace.try_spend_extraction())
    {
        return Err(crate::playback_sources::EXTRACTION_ATTEMPTS_EXHAUSTED.into());
    }
    let extraction_started = Instant::now();
    if let Some(trace) = &trace {
        trace.event(
            "extraction_start",
            &candidate.provider,
            serde_json::json!({}),
        );
    }
    let extraction_result =
        run_tool_until(settings, args, cancel.clone(), deadline.instant(), |_| {}).await;
    if extraction_result.is_ok() {
        if let Some(invocation) = provider_invocation.as_mut() {
            invocation.complete();
        }
    }

    if let Some(trace) = &trace {
        trace.event("extraction_end", &candidate.provider, serde_json::json!({"durationMs":extraction_started.elapsed().as_millis(), "ok":extraction_result.is_ok()}));
    }
    // Classify stderr before reducing it to a safe, fixed diagnostic. Extractor
    // output may contain signed URLs or account material and must never escape.
    let raw = extraction_result.map_err(|error| {
        if error == "Cancelled" {
            error
        } else {
            crate::playback_failure::safe_extractor_error(&error)
        }
    })?;
    let info: Value = serde_json::from_str(&raw)
        .map_err(|_| "yt-dlp returned invalid audio stream metadata".to_string())?;
    let audio = preview_audio_from_metadata(candidate, &info)?;
    if let Some(prepared) = prepared {
        if !alternate_format && candidate.provider == "youtube" {
            if let Ok(mut slot) = prepared.lock() {
                *slot = prepared_audio_alternate(candidate, &info, &audio);
            }
        }
    }
    Ok(audio)
}

async fn validate_prepared_preview_audio(
    audio: PreviewAudio,
    mut cancel: watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
    provider: &str,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<PreviewAudio, String> {
    if !audio.provider_session_current() {
        return Err("Audio source provider session changed during resolution".into());
    }
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Audio stream resolution timed out".into());
    }
    let stream = validate_source_url(&audio.url, "direct")?;
    let dns_started = Instant::now();
    if let Some(trace) = &trace {
        trace.event("dns_stream_start", provider, serde_json::json!({}));
    }
    let stream_addresses_result = run_playback_stage(
        cancel.clone(),
        deadline.instant(),
        "yt-dlp operation timed out",
        public_addresses(&stream),
    )
    .await;
    if let Some(trace) = &trace {
        trace.event("dns_stream_end", provider, serde_json::json!({"durationMs":dns_started.elapsed().as_millis(), "ok":stream_addresses_result.is_ok(), "addressCount":stream_addresses_result.as_ref().map_or(0, Vec::len)}));
    }
    let stream_addresses = stream_addresses_result?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(
            stream.host_str().ok_or("Missing stream host")?,
            &stream_addresses,
        )
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "Could not initialize audio access check".to_owned())?;
    run_playback_stage(
        cancel.clone(),
        deadline.instant(),
        "Audio stream resolution timed out",
        async {
            verify_preview_audio_access_with_trace(
                &client,
                &stream,
                &audio.headers,
                audio.expected_content_length,
                &mut cancel,
                trace.clone(),
                provider,
            )
            .await
        },
    )
    .await?;
    if !audio.provider_session_current() {
        return Err("Audio source provider session changed during resolution".into());
    }
    Ok(audio)
}

/// Test-only native resolver handoff. Retains production public-DNS, URL,
/// bounded head/tail byte checks and the caller's original deadline/cancel.
#[cfg(test)]
pub(crate) async fn diagnostic_validate_prepared_preview_audio(
    audio: PreviewAudio,
    cancel: watch::Receiver<bool>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<PreviewAudio, String> {
    validate_prepared_preview_audio(audio, cancel, None, "youtube", deadline).await
}

async fn traced_monochrome_probe<F, Fut>(
    trace: Option<crate::playback_sources::SourceTrace>,
    cancel: watch::Receiver<bool>,
    probe: F,
) -> Result<crate::models::Candidate, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<crate::models::Candidate, String>>,
{
    let span = trace.map(|trace| {
        crate::playback_sources::TraceSpan::begin(
            trace,
            "monochrome",
            "monochrome_probe_start",
            "monochrome_probe_end",
        )
    });
    let result = tokio::select! {
        result = probe() => result,
        _ = crate::playback_sources::wait_for_cancellation(cancel) => Err("Cancelled".into()),
    };
    if let Some(span) = span {
        span.finish(if result.is_ok() {
            "success"
        } else if result
            .as_ref()
            .err()
            .is_some_and(|error| error == "Cancelled")
        {
            "cancelled"
        } else {
            "failed"
        });
    }
    result
}

async fn resolve_bandcamp_preview_audio(
    candidate: &Candidate,
    mut cancel: watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
    deadline: crate::playback_sources::PlaybackDeadline,
) -> Result<PreviewAudio, String> {
    if !crate::bandcamp::playback_enabled() {
        return Err("Bandcamp playback is disabled".into());
    }
    let resolved = tokio::select! {
        result = crate::bandcamp::resolve_track_page_audio(&candidate.url, cancel.clone(), deadline.instant()) => result?,
        _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
        _ = tokio::time::sleep_until(deadline.instant()) => return Err("Audio stream resolution timed out".into()),
    };
    let stream = validate_source_url(resolved.stream_url.as_str(), "direct")?;
    let stream_host = stream.host_str().ok_or("Bandcamp audio host is missing")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .resolve_to_addrs(stream_host, &resolved.stream_addresses)
        .build()
        .map_err(|_| "Could not initialize Bandcamp audio access".to_owned())?;
    let probe = verify_bandcamp_stream_access(
        &client,
        &stream,
        resolved.duration_seconds,
        &mut cancel,
        trace,
    );
    let expected_content_length = tokio::time::timeout_at(deadline.instant(), probe)
        .await
        .map_err(|_| "Bandcamp audio resolution exceeded the playback deadline".to_owned())??;
    Ok(PreviewAudio {
        provider_session: None,
        url: resolved.stream_url.to_string(),
        mime_type: "audio/mpeg".into(),
        codec: Some("mp3".into()),
        bitrate_kbps: Some(128.0),
        duration_seconds: Some(resolved.duration_seconds),
        selected_format_id: Some("mp3-128".into()),
        selected_format_has_video: false,
        source_metadata: Some(serde_json::json!({
            "channel": resolved.artist.clone(),
            "uploader": resolved.artist.clone(),
            "albumUrl": resolved.album_url.as_ref().map(|url| url.as_str()),
            "albumId": resolved.album_id,
        })),
        // The shared strict matcher expects provider titles to include the
        // credited artist. Keep the parsed artist separately as well so the
        // identity gate can reject an artist mismatch directly.
        resolved_title: Some(format!("{} {}", resolved.artist, resolved.title)),
        resolved_provider_id: Some(resolved.provider_id),
        chapters: Some(vec![]),
        expected_content_length: Some(expected_content_length),
        headers: vec![],
    })
}

/// Metadata extraction can succeed before a CDN edge is ready to serve a fresh
/// signed URL. Check real bytes before caching it or handing it to the webview.
/// A single delayed 403 retry gives the same URL time to propagate; throttling
/// and server errors use a shorter backoff.
#[cfg(test)]
pub(crate) async fn diagnostic_verify_preview_audio_access(
    client: &reqwest::Client,
    stream: &url::Url,
    expected_content_length: u64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    verify_preview_audio_access(client, stream, &[], Some(expected_content_length), cancel).await
}

/// Diagnostic hybrid arm: the production bounded tail checks, with no separate
/// head request. Its initial native reader must validate and retain the prefix.
#[cfg(test)]
pub(crate) async fn diagnostic_verify_preview_audio_tail(
    client: &reqwest::Client,
    stream: &url::Url,
    length: u64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    if length <= PREVIEW_TAIL_PROBE_THRESHOLD {
        return Err("Diagnostic requires the production tail threshold".into());
    }
    let range = (length - 1024, length - 1);
    let response = send_preview_range(
        client,
        stream,
        &[],
        range,
        cancel.clone(),
        None,
        "other",
        "tail",
    )
    .await?;
    validate_preview_range(response, range, length, cancel, None, "other", "tail").await
}

async fn verify_preview_audio_access(
    client: &reqwest::Client,
    stream: &url::Url,
    headers: &[(String, String)],
    expected_content_length: Option<u64>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    verify_preview_audio_access_with_trace(
        client,
        stream,
        headers,
        expected_content_length,
        cancel,
        None,
        "other",
    )
    .await
}

async fn verify_preview_audio_access_with_trace(
    client: &reqwest::Client,
    stream: &url::Url,
    headers: &[(String, String)],
    expected_content_length: Option<u64>,
    cancel: &mut watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
    provider: &str,
) -> Result<(), String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if let Some(length) = expected_content_length.filter(|n| *n > 0) {
        let first_end = length.saturating_sub(1).min(1023);
        let first_range = (0, first_end);
        let tail_range =
            (length > PREVIEW_TAIL_PROBE_THRESHOLD).then(|| (length - 1024, length - 1));
        let first_future = send_preview_range(
            client,
            stream,
            headers,
            first_range,
            cancel.clone(),
            trace.clone(),
            provider,
            "head",
        );
        if let Some(tail_range) = tail_range {
            let tail_future = send_preview_range(
                client,
                stream,
                headers,
                tail_range,
                cancel.clone(),
                trace.clone(),
                provider,
                "tail",
            );
            let (first, tail) = tokio::try_join!(first_future, tail_future)?;
            validate_preview_range(
                first,
                first_range,
                length,
                cancel,
                trace.clone(),
                provider,
                "head",
            )
            .await?;
            validate_preview_range(
                tail,
                tail_range,
                length,
                cancel,
                trace.clone(),
                provider,
                "tail",
            )
            .await?;
        } else {
            validate_preview_range(
                first_future.await?,
                first_range,
                length,
                cancel,
                trace.clone(),
                provider,
                "head",
            )
            .await?;
        }
        return Ok(());
    }

    let mut response = send_preview_range(
        client,
        stream,
        headers,
        (0, 1023),
        cancel.clone(),
        trace.clone(),
        provider,
        "head",
    )
    .await?;
    if !response.status().is_success() {
        return Err(format!(
            "Audio source denied stream access (HTTP {})",
            response.status().as_u16()
        ));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("text/html"))
    {
        return Err("The audio source returned a web page instead of audio".into());
    }
    let body_result = tokio::select! {
        bytes = response.chunk() => bytes.map_err(|error| ("Audio stream could not deliver audio bytes".to_owned(), preview_probe_error_class(&error))),
        _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
    };
    if let Some(trace) = &trace {
        trace.event("stream_probe_body_end", provider, serde_json::json!({"role":"head","ok":body_result.is_ok(),"errorClass":body_result.as_ref().err().map(|(_, class)| *class)}));
    }
    let bytes = body_result.map_err(|(message, _)| message)?;
    if bytes.is_some_and(|bytes| !bytes.is_empty()) {
        Ok(())
    } else {
        Err("The audio source returned an empty stream".into())
    }
}

fn parse_bandcamp_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    if total == "*" {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

async fn verify_bandcamp_stream_access(
    client: &reqwest::Client,
    stream: &url::Url,
    duration_seconds: f64,
    cancel: &mut watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
) -> Result<u64, String> {
    let mut response = send_preview_range(
        client,
        stream,
        &[],
        (0, 1023),
        cancel.clone(),
        trace.clone(),
        "bandcamp",
        "head",
    )
    .await?;
    let content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_length = response.content_length();
    let mut bytes = Vec::with_capacity(1024);
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| "Bandcamp audio stream could not deliver bytes".to_owned())?,
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
        };
        let Some(chunk) = chunk else { break };
        if bytes.len().saturating_add(chunk.len()) > 1024 {
            return Err("Bandcamp audio stream exceeded its bounded probe".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let total = validate_bandcamp_probe(
        response.status(),
        content_range.as_deref(),
        content_length,
        content_type.as_deref(),
        duration_seconds,
        &bytes,
    )?;

    let tail_range = (total - 1024, total - 1);
    let mut tail = send_preview_range(
        client,
        stream,
        &[],
        tail_range,
        cancel.clone(),
        trace,
        "bandcamp",
        "tail",
    )
    .await?;
    let tail_status = tail.status();
    let tail_content_range = tail
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let tail_content_type = tail
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let tail_content_length = tail.content_length();
    let mut tail_bytes = Vec::with_capacity(1024);
    loop {
        let chunk = tokio::select! {
            chunk = tail.chunk() => chunk.map_err(|_| "Bandcamp audio stream could not deliver bytes".to_owned())?,
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
        };
        let Some(chunk) = chunk else { break };
        if tail_bytes.len().saturating_add(chunk.len()) > 1024 {
            return Err("Bandcamp audio stream exceeded its bounded tail probe".into());
        }
        tail_bytes.extend_from_slice(&chunk);
    }
    validate_bandcamp_tail_probe(
        tail_status,
        tail_content_range.as_deref(),
        tail_content_length,
        tail_content_type.as_deref(),
        tail_range,
        total,
        &tail_bytes,
    )?;
    Ok(total)
}

fn validate_bandcamp_probe(
    status: reqwest::StatusCode,
    content_range: Option<&str>,
    content_length: Option<u64>,
    content_type: Option<&str>,
    duration_seconds: f64,
    bytes: &[u8],
) -> Result<u64, String> {
    if status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(if status.is_client_error() {
            "Bandcamp did not allow a bounded public audio read".into()
        } else {
            "Bandcamp audio stream did not honor a bounded byte-range request".into()
        });
    }
    let Some((start, end, total)) = content_range.and_then(parse_bandcamp_content_range) else {
        return Err("Bandcamp audio stream returned an invalid bounded range".into());
    };
    if start != 0
        || end < start
        || end > 1023
        || total <= end
        || bytes.len() as u64 != end - start + 1
        || content_length.is_some_and(|length| length != end - start + 1)
    {
        return Err("Bandcamp audio stream returned an invalid bounded range".into());
    }
    if !content_type.is_some_and(|value| value.to_ascii_lowercase().starts_with("audio/mpeg")) {
        return Err("Bandcamp did not return MP3 audio".into());
    }
    if !valid_mp3_header(bytes) {
        return Err("Bandcamp audio stream did not contain a valid MP3 header".into());
    }
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return Err("Bandcamp track duration is invalid".into());
    }
    // At 128 kbps, a full MP3 should be close to 16 kB per second. A 75%
    // floor tolerates variable bitrate and tag overhead while rejecting short
    // previews whose page metadata still advertises the full track duration.
    let expected_minimum = (duration_seconds * 16_000.0 * 0.75).floor() as u64;
    if total < expected_minimum {
        return Err("Bandcamp stream is shorter than the advertised full track".into());
    }
    Ok(total)
}

fn valid_mp3_header(bytes: &[u8]) -> bool {
    if bytes.len() >= 10
        && &bytes[..3] == b"ID3"
        && (2..=4).contains(&bytes[3])
        && bytes[4] != 0xff
        && bytes[5] & 0x0f == 0
        && bytes[6..10].iter().all(|byte| byte & 0x80 == 0)
    {
        return true;
    }
    bytes.windows(4).any(|header| {
        header[0] == 0xff
            && header[1] & 0xe0 == 0xe0
            && (header[1] >> 1) & 0x03 != 0
            && header[2] >> 4 != 0
            && header[2] >> 4 != 0x0f
            && (header[2] >> 2) & 0x03 != 0x03
    })
}

fn validate_bandcamp_tail_probe(
    status: reqwest::StatusCode,
    content_range: Option<&str>,
    content_length: Option<u64>,
    content_type: Option<&str>,
    expected_range: (u64, u64),
    expected_total: u64,
    bytes: &[u8],
) -> Result<(), String> {
    let valid_range = content_range.and_then(parse_bandcamp_content_range)
        == Some((expected_range.0, expected_range.1, expected_total));
    let expected_bytes = expected_range.1 - expected_range.0 + 1;
    if status != reqwest::StatusCode::PARTIAL_CONTENT
        || !valid_range
        || expected_bytes > 1024
        || bytes.len() as u64 != expected_bytes
        || content_length.is_some_and(|length| length != expected_bytes)
        || !content_type.is_some_and(|value| value.to_ascii_lowercase().starts_with("audio/mpeg"))
    {
        return Err("Bandcamp audio stream returned an invalid bounded tail range".into());
    }
    Ok(())
}

async fn send_preview_range(
    client: &reqwest::Client,
    stream: &url::Url,
    headers: &[(String, String)],
    range: (u64, u64),
    mut cancel: watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
    provider: &str,
    role: &'static str,
) -> Result<reqwest::Response, String> {
    for attempt in 0..2 {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let mut request = client.get(stream.clone()).header(
            reqwest::header::RANGE,
            format!("bytes={}-{}", range.0, range.1),
        );
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let probe_started = Instant::now();
        if let Some(trace) = &trace {
            trace.event(
                "stream_probe_start",
                provider,
                serde_json::json!({"role":role,"attempt":attempt+1}),
            );
        }
        let (response_result, error_class) = tokio::select! {
            response = request.send() => match response {
                Ok(response) => (Ok(response), None),
                Err(error) => (Err("Audio stream access check failed".to_owned()), Some(preview_probe_error_class(&error))),
            },
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => (Err("Cancelled".into()), None),
        };
        if let Some(trace) = &trace {
            trace.event("stream_probe_end", provider, serde_json::json!({"role":role,"attempt":attempt+1,"durationMs":probe_started.elapsed().as_millis(),"ok":response_result.is_ok(),"status":response_result.as_ref().ok().map(|r| r.status().as_u16()),"errorClass":error_class}));
        }
        let response = response_result?;
        if attempt == 0 && provider != "bandcamp" {
            let Some(delay) = preview_access_retry_delay(response.status()) else {
                return Ok(response);
            };
            if let Some(trace) = &trace {
                trace.event("stream_probe_retry_wait", provider, serde_json::json!({"role":role,"attempt":attempt+1,"status":response.status().as_u16(),"expectedDelayMs":delay.as_millis()}));
            }
            drop(response);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
            }
            continue;
        }
        return Ok(response);
    }
    unreachable!()
}

async fn validate_preview_range(
    mut response: reqwest::Response,
    range: (u64, u64),
    total: u64,
    cancel: &mut watch::Receiver<bool>,
    trace: Option<crate::playback_sources::SourceTrace>,
    provider: &str,
    role: &'static str,
) -> Result<(), String> {
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(if response.status().is_success() {
            "Audio source did not honor a bounded byte-range request".into()
        } else {
            format!(
                "Audio source denied stream access (HTTP {})",
                response.status().as_u16()
            )
        });
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("text/html"))
    {
        return Err("The audio source returned a web page instead of audio".into());
    }
    let content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range)
        .ok_or("Audio source returned an invalid byte range")?;
    if content_range != (range.0, range.1, total) {
        return Err("Audio source returned an inconsistent byte range".into());
    }
    let Some(expected_bytes) = range
        .1
        .checked_sub(range.0)
        .and_then(|size| size.checked_add(1))
        .filter(|size| (1..=1024).contains(size))
    else {
        return Err("Audio source returned an invalid byte range".into());
    };
    if response
        .content_length()
        .is_some_and(|declared| declared != expected_bytes)
    {
        if let Some(trace) = &trace {
            trace.event(
                "stream_probe_body_end",
                provider,
                serde_json::json!({"role":role,"ok":false,"errorClass":"body"}),
            );
        }
        return Err("Audio source returned an inconsistent byte range".into());
    }
    let mut received = 0u64;
    let body_error = loop {
        let next = tokio::select! {
            bytes = response.chunk() => bytes.map_err(|error| ("Audio stream could not deliver audio bytes".to_owned(), preview_probe_error_class(&error))),
            _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
        };
        match next {
            Err((message, class)) => break Some((message, class)),
            Ok(None) if received != expected_bytes => {
                break Some((
                    "Audio source returned a truncated byte range".to_owned(),
                    "body",
                ));
            }
            Ok(None) => break None,
            Ok(Some(bytes)) => {
                let Some(next_received) = received.checked_add(bytes.len() as u64) else {
                    break Some((
                        "Audio source returned an oversized byte range".to_owned(),
                        "body",
                    ));
                };
                if next_received > expected_bytes {
                    break Some((
                        "Audio source returned an oversized byte range".to_owned(),
                        "body",
                    ));
                }
                received = next_received;
                if received == expected_bytes {
                    break None;
                }
            }
        }
    };
    if let Some(trace) = &trace {
        trace.event("stream_probe_body_end", provider, serde_json::json!({"role":role,"ok":body_error.is_none(),"errorClass":body_error.as_ref().map(|(_, class)| *class)}));
    }
    body_error.map_or(Ok(()), |(message, _)| Err(message))
}

/// Classify only reqwest's finite error-kind flags. Never inspect or serialize
/// the error text, request URL, peer address, headers, or response body.
fn preview_probe_error_class(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() || error.is_builder() {
        "request"
    } else if error.is_body() || error.is_decode() {
        "body"
    } else {
        "other"
    }
}

fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

fn preview_access_retry_delay(status: reqwest::StatusCode) -> Option<Duration> {
    if status == reqwest::StatusCode::FORBIDDEN {
        Some(Duration::from_millis(1500))
    } else if status.as_u16() == 429 || status.is_server_error() {
        Some(Duration::from_millis(300))
    } else {
        None
    }
}

fn preview_audio_from_metadata(
    candidate: &Candidate,
    info: &Value,
) -> Result<PreviewAudio, String> {
    if info["entries"].is_array()
        || matches!(info["_type"].as_str(), Some("playlist" | "multi_video"))
    {
        return Err("Audio preview requires one individual recording".into());
    }
    let stream_url = info["url"]
        .as_str()
        .ok_or("yt-dlp returned no audio stream URL")?;
    let codec = info["acodec"]
        .as_str()
        .filter(|codec| *codec != "none")
        .ok_or("yt-dlp did not select an audio stream")?;
    let progressive = matches!(info["protocol"].as_str(), Some("http" | "https"));
    let soundcloud_hls = candidate.provider == "soundcloud"
        && matches!(info["protocol"].as_str(), Some("m3u8" | "m3u8_native"));
    if !progressive && !soundcloud_hls {
        return Err("This source has no supported audio stream".into());
    }
    let has_video = info["vcodec"].as_str().is_some_and(|codec| codec != "none");
    let muxed_youtube_mp4 = candidate.provider == "youtube"
        && info["ext"].as_str() == Some("mp4")
        && info["protocol"].as_str() == Some("https")
        && has_video;
    if has_video && !muxed_youtube_mp4 {
        return Err("yt-dlp selected an unsupported video stream".into());
    }
    if !candidate.id.is_empty() {
        let resolved_id = info["id"].as_str().unwrap_or_default();
        if candidate.id != format!("{}:{resolved_id}", candidate.provider) {
            return Err("Source identity changed during audio resolution; select it again".into());
        }
    }
    let stream = validate_source_url(stream_url, "direct")?;
    if stream.scheme() != "https" {
        return Err("Audio stream must use public HTTPS".into());
    }
    let host = stream.host_str().ok_or("Missing audio stream hostname")?;
    let allowed_cdn = match candidate.provider.as_str() {
        "youtube" => host_matches(host, "googlevideo.com"),
        "soundcloud" => {
            host_matches(host, "sndcdn.com")
                || host_matches(host, "soundcloud.com")
                // SoundCloud's AAC HLS migration uses this first-party host;
                // yt-dlp returns it for tracks such as Fred again..'s Secret
                // Life piano set. Keep the exception exact rather than
                // trusting arbitrary subdomains of soundcloud.cloud.
                || host == "playback.media-streaming.soundcloud.cloud"
        }
        _ => false,
    };
    if !allowed_cdn {
        return Err("yt-dlp returned a stream outside the provider media hosts".into());
    }
    let mime_type = if info["protocol"]
        .as_str()
        .is_some_and(|protocol| protocol.starts_with("m3u8"))
        || stream.path().to_ascii_lowercase().ends_with(".m3u8")
    {
        "application/vnd.apple.mpegurl".into()
    } else {
        audio_mime_type(info["ext"].as_str().unwrap_or_default(), codec)?
    };
    let duration_seconds = info["duration"]
        .as_f64()
        .filter(|duration| duration.is_finite() && *duration > 0.0);
    Ok(PreviewAudio {
        provider_session: None,
        url: stream_url.into(),
        mime_type,
        codec: Some(codec.to_owned()),
        bitrate_kbps: info["abr"]
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0),
        duration_seconds,
        selected_format_id: safe_extractor_format_id(info["format_id"].as_str()),
        selected_format_has_video: info["vcodec"].as_str().is_some_and(|codec| codec != "none"),
        source_metadata: if candidate.provider == "youtube" {
            // Topic status must come from this extraction response. Search
            // hints and remembered candidates can be stale or have no
            // uploader at all, so they cannot select the richer proof path.
            let richer_metadata = if fresh_youtube_topic_identity(info) {
                topic_identity_metadata(candidate, info)
            } else {
                verified_youtube_channel_metadata(candidate, info)
            };
            richer_metadata.or_else(|| youtube_identity_only_metadata(candidate, info))
        } else {
            None
        },
        resolved_title: info["title"].as_str().map(str::to_owned),
        resolved_provider_id: info["id"].as_str().map(str::to_owned),
        chapters: Some(parse_source_chapters(info).unwrap_or_default()),
        expected_content_length: info["filesize"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= MAX_PREVIEW_FILE_SIZE),
        headers: preview_audio_headers(info),
    })
}

fn prepared_audio_alternate(
    candidate: &Candidate,
    info: &Value,
    selected: &PreviewAudio,
) -> Option<PreviewAudio> {
    if candidate.provider != "youtube" {
        return None;
    }
    let mut choices = info["formats"]
        .as_array()?
        .iter()
        .filter(|format| {
            format["protocol"].as_str() == Some("https")
                && format["ext"].as_str() == Some("webm")
                && format["vcodec"].as_str() == Some("none")
                && format["acodec"]
                    .as_str()
                    .is_some_and(|codec| codec == "opus" || codec == "vorbis")
        })
        .collect::<Vec<_>>();
    choices.sort_by(|left, right| {
        let bitrate = |value: &Value| {
            value["abr"]
                .as_f64()
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(0.0)
        };
        bitrate(right).total_cmp(&bitrate(left))
    });
    for format in choices {
        if format["url"].as_str() == Some(selected.url.as_str()) {
            continue;
        }
        let mut merged = info.clone();
        let target = merged.as_object_mut()?;
        for field in [
            "url",
            "format_id",
            "acodec",
            "vcodec",
            "ext",
            "protocol",
            "abr",
            "filesize",
        ] {
            target.remove(field);
            if let Some(value) = format.get(field) {
                target.insert(field.into(), value.clone());
            }
        }
        if let Some(headers) = format.get("http_headers") {
            target.insert("http_headers".into(), headers.clone());
        }
        if let Ok(audio) = preview_audio_from_metadata(candidate, &merged) {
            if !preview_audio_cache_ttl(&audio.url).is_zero() {
                return Some(audio);
            }
        }
    }
    None
}

fn safe_extractor_format_id(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
        })
        .map(str::to_owned)
}

fn playback_music_catalog_candidates(
    track: &Track,
    items: Vec<crate::youtube_music::YouTubeMusicItem>,
) -> Vec<Candidate> {
    let mut music = std::collections::BTreeMap::new();
    for item in items {
        if let Some(candidate) = playback_music_catalog_candidate(track, item) {
            music.entry(candidate.id.clone()).or_insert(candidate);
        }
    }
    // Distinct uploads are candidates, not a recording ambiguity by themselves.
    music.into_values().collect()
}

fn playback_music_catalog_candidate(
    track: &Track,
    item: crate::youtube_music::YouTubeMusicItem,
) -> Option<Candidate> {
    if item.id.len() != 11
        || !item
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    let artist = crate::source_fallback::youtube_catalog_artist_credit(&item.title, &item.artist?);
    let candidate = Candidate {
        id: format!("youtube:{}", item.id),
        provider: "youtube".into(),
        title: item.title.clone(),
        url: format!("https://www.youtube.com/watch?v={}", item.id),
        duration_seconds: item.duration_seconds,
        confidence: "review".into(),
        permission: "unknown".into(),
        score: 0.7,
        source_metadata: Some(serde_json::json!({
            "provider":"youtube", "playbackMusicCatalogCandidate":true,
            "catalogTitle":item.title, "catalogArtist":artist,
        })),
        reasons: vec![
            "Music catalog locator; fresh recording identity verification required".into(),
        ],
        ..Candidate::default()
    };
    // Retrieval retains same-title locators without making a performer/version
    // acceptance decision. The shared evaluator applies current evidence later.
    let wanted = crate::recording_identity::parse_title(&track.title).title;
    let observed = crate::recording_identity::parse_title(&candidate.title).title;
    (crate::matching::normalize(&wanted) == crate::matching::normalize(&observed))
        .then_some(candidate)
}

/// Retain the narrowly useful, fresh YouTube channel identity exposed by the
/// extractor. This is separate from Topic's structured artist/credit metadata
/// and is usable only when yt-dlp itself marks the current channel verified.
fn verified_youtube_channel_metadata(candidate: &Candidate, info: &Value) -> Option<Value> {
    if candidate.provider != "youtube"
        || fresh_youtube_topic_identity(info)
        || info["channel_is_verified"].as_bool() != Some(true)
    {
        return None;
    }
    let channel_id = info["channel_id"].as_str().filter(|id| {
        id.len() == 24
            && id.starts_with("UC")
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })?;
    let channel = info["channel"]
        .as_str()
        .or_else(|| info["uploader"].as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 200)?;
    let uploader = info["uploader"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 200);
    let track = info["track"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty() && title.len() <= 300);
    let artists = info["artists"].as_array().and_then(|values| {
        if values.is_empty() || values.len() > 8 {
            return None;
        }
        let names = values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|name| !name.is_empty() && name.len() <= 120)
                    .map(str::to_owned)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(names)
    });
    Some(serde_json::json!({
        "provider": "youtube",
        "channelId": channel_id,
        "channel": channel,
        "uploader": uploader,
        "channelIsVerified": true,
        "track": track,
        "artistCount": artists.as_ref().map(Vec::len),
        "artists": artists,
    }))
}

/// Retain only the fresh channel ID as an equality anchor when yt-dlp has not
/// supplied complete verified-channel or Topic credit metadata. This object
/// carries no claim about channel verification, uploader identity, or artist
/// credits and must not be used to satisfy those gates.
fn youtube_identity_only_metadata(candidate: &Candidate, info: &Value) -> Option<Value> {
    if candidate.provider != "youtube" {
        return None;
    }
    let channel_id = info["channel_id"].as_str().filter(|id| {
        id.len() == 24
            && id.starts_with("UC")
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })?;
    Some(serde_json::json!({
        "provider":"youtube",
        "channelId":channel_id,
    }))
}

fn fresh_youtube_topic_identity(info: &Value) -> bool {
    [info["channel"].as_str(), info["uploader"].as_str()]
        .into_iter()
        .flatten()
        .any(|name| name.trim_end().ends_with("- Topic"))
}

fn topic_identity_metadata(candidate: &Candidate, info: &Value) -> Option<Value> {
    let channel = info["channel"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 200)?;
    let track = info["track"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 300)?;
    let duration = info["duration"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)?;
    if candidate.provider != "youtube" || !fresh_youtube_topic_identity(info) {
        return None;
    }
    let raw_artists = info["artists"].as_array()?;
    if raw_artists.is_empty() || raw_artists.len() > 32 {
        return None;
    }
    let artists = raw_artists
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty() && name.len() <= 120)
        })
        .collect::<Option<Vec<_>>>()?;
    if artists.is_empty() {
        return None;
    }
    let description = info["description"].as_str().unwrap_or_default();
    let credit_line = description
        .lines()
        .take(80)
        .map(str::trim)
        .find(|line| {
            line.len() <= 300
                && !line.contains("http://")
                && !line.contains("https://")
                && artists.iter().all(|artist| {
                    crate::matching::normalize(line).contains(&crate::matching::normalize(artist))
                })
                && crate::matching::normalize(line).contains(&crate::matching::normalize(track))
        })
        .map(str::to_owned)?;
    let channel_id = info["channel_id"].as_str().filter(|id| {
        id.len() == 24
            && id.starts_with("UC")
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })?;
    Some(serde_json::json!({
        "provider": "youtube",
        "channelId": channel_id,
        "channel": channel,
        "track": track,
        "artist": info["artist"].as_str().unwrap_or_default(),
        "artistCount": artists.len(),
        "artists": artists,
        "descriptionCreditLine": credit_line,
        "duration": duration,
    }))
}

fn preview_audio_format_selector(
    alternate_format: bool,
    allow_muxed_youtube: bool,
) -> &'static str {
    if allow_muxed_youtube && alternate_format {
        PREVIEW_AUDIO_YOUTUBE_ALTERNATE_FORMAT_SELECTOR
    } else if allow_muxed_youtube {
        PREVIEW_AUDIO_YOUTUBE_FORMAT_SELECTOR
    } else if alternate_format {
        PREVIEW_AUDIO_ALTERNATE_FORMAT_SELECTOR
    } else {
        PREVIEW_AUDIO_FORMAT_SELECTOR
    }
}

fn preview_audio_headers(info: &Value) -> Vec<(String, String)> {
    const ALLOWED: [(&str, &str); 4] = [
        ("user-agent", "User-Agent"),
        ("referer", "Referer"),
        ("origin", "Origin"),
        ("accept-language", "Accept-Language"),
    ];
    let Some(headers) = info.get("http_headers").and_then(Value::as_object) else {
        return vec![];
    };
    ALLOWED
        .iter()
        .filter_map(|(lookup, output)| {
            let value = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(lookup))?
                .1
                .as_str()?
                .trim();
            if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
                return None;
            }
            if matches!(*lookup, "referer" | "origin") {
                let url = url::Url::parse(value).ok()?;
                if !matches!(url.scheme(), "http" | "https")
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return None;
                }
            }
            Some(((*output).to_owned(), value.to_owned()))
        })
        .collect()
}

fn audio_mime_type(extension: &str, codec: &str) -> Result<String, String> {
    let mime = match extension.to_ascii_lowercase().as_str() {
        "m4a" | "mp4" => "audio/mp4",
        "webm" => "audio/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        _ if codec.starts_with("mp4a.") => "audio/mp4",
        _ => return Err("Unsupported audio preview format".into()),
    };
    Ok(mime.into())
}

async fn inspect_with_cancel(
    track: &Track,
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    let key = inspection_cache_key(candidate, settings);
    let cache = INSPECTION_CACHE.get_or_init(InspectionCache::default);
    let mut inspected = cache
        .get_or_inspect(key, || {
            inspect_metadata_with_cancel(candidate, settings, cancel)
        })
        .await?;
    matching::score_candidate(track, &mut inspected);
    if candidate.provider == "direct" {
        inspected
            .reasons
            .push("Direct download permission must be verified by you".into());
    } else {
        inspected
            .reasons
            .push("Streaming availability does not establish download permission".into());
    }
    Ok(inspected)
}

async fn inspect_metadata_with_cancel(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    if candidate.provider == "monochrome" {
        crate::monochrome::validate_url(&candidate.url)?;
        let client = crate::monochrome::Client::new()?;
        let mut cancel = cancel;
        let mut result = tokio::select! { result = client.inspect(candidate) => result, _ = cancel.changed() => Err("Cancelled".into()) }?;
        result.score = 0.0;
        result.reasons.clear();
        return Ok(result);
    }
    let url = validate_source_url(&candidate.url, &candidate.provider)?;
    public_addresses(&url).await?;
    if candidate.provider == "direct" {
        let mut result = candidate.clone();
        result.formats = Vec::new();
        result.official = false;
        result.permission = "unknown".into();
        return Ok(result);
    }
    inspect_ytdlp_metadata(candidate, settings, cancel).await
}

async fn inspect_ytdlp_metadata(
    candidate: &Candidate,
    settings: &Settings,
    cancel: watch::Receiver<bool>,
) -> Result<Candidate, String> {
    let mut args = base_args(settings);
    let (_, mut provider_invocation) =
        add_youtube_support_args(&mut args, &candidate.provider, settings, None).await?;
    args.extend([
        "--dump-single-json".into(),
        "--no-playlist".into(),
        "--skip-download".into(),
        "--".into(),
        candidate.url.clone(),
    ]);
    let raw = run_tool(settings, args, cancel, Duration::from_secs(60), |_| {}).await?;
    if let Some(invocation) = provider_invocation.as_mut() {
        invocation.complete();
    }

    let info: Value =
        serde_json::from_str(&raw).map_err(|e| format!("Invalid source metadata: {e}"))?;
    if info["entries"].is_array()
        || matches!(info["_type"].as_str(), Some("playlist" | "multi_video"))
    {
        return Err("This URL contains multiple recordings. Choose an individual track; collections never download implicitly.".into());
    }
    let mut result = candidate_from_json(&info, &candidate.provider);
    if let Some(metadata) = result
        .source_metadata
        .as_mut()
        .and_then(Value::as_object_mut)
    {
        metadata.insert(
            "sourceMetadataOrigin".into(),
            Value::String("ytDlpFullInfo".into()),
        );
    }
    if result.url.is_empty() {
        result.url = candidate.url.clone();
    }
    validate_source_url(&result.url, &result.provider)?;
    if !candidate.id.is_empty() && candidate.id != result.id {
        return Err("Source identity changed during inspection; select it again".into());
    }
    result.score = 0.0;
    result.reasons.clear();
    Ok(result)
}

pub async fn check_dependencies(settings: &Settings) -> Vec<Dependency> {
    let mut result = Vec::new();
    for (name, path, default_name) in [
        ("yt-dlp", &settings.yt_dlp_path, "yt-dlp"),
        ("ffmpeg", &settings.ffmpeg_path, "ffmpeg"),
        ("ffprobe", &settings.ffprobe_path, "ffprobe"),
        ("JavaScript runtime", &settings.js_runtime_path, "deno"),
    ] {
        let executable = crate::tools::executable(path, default_name);
        let mut command = Command::new(executable);
        command.arg(if name == "ffmpeg" || name == "ffprobe" {
            "-version"
        } else {
            "--version"
        });
        command
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
        }
        let dependency = match tokio::time::timeout(Duration::from_secs(5), command.output()).await
        {
            Ok(Ok(out)) if out.status.success() => Dependency {
                name: name.into(),
                available: true,
                version: Some(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("Available")
                        .chars()
                        .take(200)
                        .collect(),
                ),
                message: if name == "yt-dlp" {
                    "Executable available; YouTube also requires EJS and a supported JavaScript runtime".into()
                } else {
                    "Available".into()
                },
            },
            Ok(Ok(_)) => Dependency {
                name: name.into(),
                available: false,
                version: None,
                message: format!("{path} returned a failed version check"),
            },
            Ok(Err(e)) => Dependency {
                name: name.into(),
                available: false,
                version: None,
                message: format!("Configure an installed {name} executable: {e}"),
            },
            Err(_) => Dependency {
                name: name.into(),
                available: false,
                version: None,
                message: "Version check timed out after 5 seconds".into(),
            },
        };
        result.push(dependency);
    }
    result.push(Dependency { name: "yt-dlp-ejs".into(), available: false, version: None,
        message: "Unknown: EJS availability cannot be established from executable version checks. Official yt-dlp binaries include it; package installs need yt-dlp-ejs. A real YouTube inspection validates support.".into() });
    result
}

#[derive(Clone)]
pub struct AcquisitionManager {
    inner: Arc<Inner>,
}
struct Inner {
    app: Option<tauri::AppHandle>,
    store: Arc<Store>,
    connections: Arc<ConnectionManager>,
    monochrome_playback: Mutex<Option<Arc<crate::monochrome_playback::MonochromePlaybackService>>>,
    remote_playlists: Mutex<Option<Arc<crate::remote_playlists::RemotePlaylistService>>>,
    analysis: Mutex<Option<Arc<crate::analysis::AnalysisManager>>>,
    autotag: Mutex<Option<Arc<crate::autotag::AutoTagger>>>,
    slskd_searches: Mutex<HashMap<String, SlskdSearchSession>>,
    slskd_completed: Mutex<HashMap<String, (Instant, SlskdSearchPoll)>>,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    search_cache: CandidateSearchCache,
    raw_audio_search_cache: RawAudioSearchCache,
    preview_audio_cache: PreviewAudioCache,
    playback_metadata_cache: PreviewAudioCache,
    playback_access_cache: PreviewAudioCache,
    playback_session_epochs: Mutex<HashMap<String, Arc<AtomicU64>>>,
    chapter_source_cache: Arc<ChapterSourceCache>,
    preview_search: PreviewSearchState,
    slots: Semaphore,
    soulseek_slots: Semaphore,
}
struct SlskdSearchSession {
    started: Instant,
    retain_completed_results: bool,
    poll_gate: Arc<tokio::sync::Mutex<()>>,
}

fn cached_slskd_search(
    completed: &Mutex<HashMap<String, (Instant, SlskdSearchPoll)>>,
    search_id: &str,
) -> Result<Option<SlskdSearchPoll>, String> {
    let mut completed = completed
        .lock()
        .map_err(|_| "Soulseek result cache lock poisoned")?;
    completed.retain(|_, (at, _)| at.elapsed() < SLSKD_COMPLETED_SEARCH_TTL);
    Ok(completed.get(search_id).map(|(_, result)| result.clone()))
}

fn remember_completed_slskd_search(
    completed: &Mutex<HashMap<String, (Instant, SlskdSearchPoll)>>,
    search_id: &str,
    result: &SlskdSearchPoll,
) -> Result<(), String> {
    let mut completed = completed
        .lock()
        .map_err(|_| "Soulseek result cache lock poisoned")?;
    completed.retain(|_, (at, _)| at.elapsed() < SLSKD_COMPLETED_SEARCH_TTL);
    while completed.len() >= SLSKD_COMPLETED_SEARCH_LIMIT {
        if let Some(oldest) = completed
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(id, _)| id.clone())
        {
            completed.remove(&oldest);
        } else {
            break;
        }
    }
    completed.insert(search_id.to_owned(), (Instant::now(), result.clone()));
    Ok(())
}

fn slskd_search_poll_gate(
    searches: &Mutex<HashMap<String, SlskdSearchSession>>,
    search_id: &str,
) -> Result<Arc<tokio::sync::Mutex<()>>, String> {
    searches
        .lock()
        .map_err(|_| "slskd search state lock poisoned")?
        .get(search_id)
        .map(|session| session.poll_gate.clone())
        .ok_or_else(|| "Unknown or expired slskd search".into())
}

fn active_slskd_search_session(
    searches: &Mutex<HashMap<String, SlskdSearchSession>>,
    search_id: &str,
) -> Result<(Instant, bool), String> {
    searches
        .lock()
        .map_err(|_| "slskd search state lock poisoned")?
        .get(search_id)
        .map(|session| (session.started, session.retain_completed_results))
        .ok_or_else(|| "Unknown or expired slskd search".into())
}

fn slskd_response_file_count(responses: &[crate::slskd::SearchResponse]) -> usize {
    responses.iter().map(|response| response.files.len()).sum()
}

/// Merge the status endpoint's embedded snapshot with the dedicated response
/// endpoint. The latter is usually fresher, while slskd can briefly expose
/// rows in only one endpoint as a search completes.
fn merge_slskd_search_responses(
    embedded: &[crate::slskd::SearchResponse],
    separate: &[crate::slskd::SearchResponse],
) -> Vec<crate::slskd::SearchResponse> {
    let mut merged = Vec::<crate::slskd::SearchResponse>::new();
    for (responses, fresh) in [(embedded, false), (separate, true)] {
        for response in responses {
            let Some(existing) = merged
                .iter_mut()
                .find(|existing| existing.username.eq_ignore_ascii_case(&response.username))
            else {
                merged.push(response.clone());
                continue;
            };
            let mut files = std::mem::take(&mut existing.files);
            for file in &response.files {
                if let Some(index) = files.iter().position(|candidate| {
                    candidate.filename == file.filename && candidate.size == file.size
                }) {
                    if fresh {
                        files[index] = file.clone();
                    }
                } else {
                    files.push(file.clone());
                }
            }
            existing.files = files;
            if fresh {
                existing.has_free_upload_slot = response
                    .has_free_upload_slot
                    .or(existing.has_free_upload_slot);
                existing.queue_length = response.queue_length.or(existing.queue_length);
                existing.upload_speed = response.upload_speed.or(existing.upload_speed);
            }
        }
    }
    merged
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlskdSearchPoll {
    pub id: String,
    pub state: String,
    pub tracks: Vec<Track>,
    pub candidates: Vec<Candidate>,
    pub complete: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulseekAlbumAssignment {
    pub track_id: String,
    pub candidate: Candidate,
    pub group_peer: String,
    pub group_directory: String,
    pub group_coverage: usize,
    pub expected_tracks: usize,
}

impl AcquisitionManager {
    pub fn new(
        app: tauri::AppHandle,
        store: Arc<Store>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        Self::with_optional_app(Some(app), store, connections)
    }

    /// Construct the production acquisition pipeline for headless diagnostics.
    /// Uses the supplied store and connections without emitting GUI events.
    pub fn new_for_diagnostics(store: Arc<Store>, connections: Arc<ConnectionManager>) -> Self {
        Self::with_optional_app(None, store, connections)
    }

    #[cfg(test)]
    pub(crate) fn new_for_playback_test(
        store: Arc<Store>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        Self::with_optional_app(None, store, connections)
    }

    fn with_optional_app(
        app: Option<tauri::AppHandle>,
        store: Arc<Store>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                app,
                store,
                connections,
                monochrome_playback: Mutex::new(None),
                remote_playlists: Mutex::new(None),
                analysis: Mutex::new(None),
                autotag: Mutex::new(None),
                slskd_searches: Mutex::new(HashMap::new()),
                slskd_completed: Mutex::new(HashMap::new()),
                active: Mutex::new(HashMap::new()),
                search_cache: CandidateSearchCache::default(),
                raw_audio_search_cache: RawAudioSearchCache::default(),
                preview_audio_cache: PreviewAudioCache::default(),
                playback_metadata_cache: PreviewAudioCache::default(),
                playback_access_cache: PreviewAudioCache::for_signed_stream_access(),
                playback_session_epochs: Mutex::new(HashMap::new()),
                chapter_source_cache: Arc::new(ChapterSourceCache::default()),
                preview_search: PreviewSearchState::default(),
                slots: Semaphore::new(2),
                soulseek_slots: Semaphore::new(SOULSEEK_PENDING_LIMIT),
            }),
        }
    }
    pub async fn resume_interrupted_soulseek(&self, settings: Settings) -> Result<usize, String> {
        if settings.music_directory.trim().is_empty() || self.inner.connections.slskd()?.is_none() {
            return Ok(0);
        }
        let mut resumed = 0;
        for job in self.inner.store.list_jobs()? {
            if resumable_soulseek_job(&job) {
                match self.retry(&job.id, settings.clone()).await {
                    Ok(_) => resumed += 1,
                    Err(error) => eprintln!("Soulseek job {} recovery failed: {error}", job.id),
                }
            }
        }
        Ok(resumed)
    }

    pub(crate) fn notify_bandcamp_source_ready(&self, track: &Track) {
        self.notify_source_ready(track, "bandcamp");
    }

    /// Install the resolver path only when the host has an explicitly
    /// authorized auth source and registered native relay.
    pub(crate) fn set_monochrome_playback_service(
        &self,
        service: Option<Arc<crate::monochrome_playback::MonochromePlaybackService>>,
    ) {
        if let Ok(mut slot) = self.inner.monochrome_playback.lock() {
            *slot = service;
        }
    }

    pub(crate) fn monochrome_playback_service(
        &self,
    ) -> Option<Arc<crate::monochrome_playback::MonochromePlaybackService>> {
        self.inner
            .monochrome_playback
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    pub(crate) fn notify_source_ready(&self, track: &Track, provider: &str) {
        if let Some(app) = &self.inner.app {
            let _ = app.emit(
                "playback-source-ready",
                serde_json::json!({"trackId":track.id,"title":track.title,"provider":provider}),
            );
        }
    }

    pub(crate) fn alias_store(&self) -> Arc<Store> {
        self.inner.store.clone()
    }

    /// Search one download provider using only an exact alias variant already
    /// validated and retained by the shared recording identity context.
    /// Preparation is a separate per-track operation so album batches do not
    /// repeat metadata discovery for each provider.
    pub(crate) async fn find_download_candidates_with_cached_aliases(
        &self,
        track: &Track,
        provider: &str,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<Vec<Candidate>, String> {
        if !matches!(provider, "youtube" | "soundcloud" | "monochrome") {
            return Err("Download alias search requires a supported web provider".into());
        }
        let alternate_artist = cached_download_search_alternate(&self.inner.store, track);
        let providers = vec![provider.to_owned()];
        let key = serde_json::json!([
            "download-candidate-search-v1",
            search_cache_key(track, &providers, settings, 10),
            alternate_artist,
        ])
        .to_string();
        let alternate_for_search = alternate_artist.clone();
        cached_download_candidate_search(&self.inner.search_cache, key, cancel.clone(), || async {
            let (candidates, complete) = find_candidates_with_cancel_until_alias_detailed(
                track,
                &providers,
                settings,
                cancel,
                10,
                PREVIEW_SEARCH_TIMEOUT,
                Some(deadline),
                alternate_for_search,
                Some(&self.inner.search_cache),
            )
            .await?;
            Ok((candidates, complete))
        })
        .await
    }

    pub fn set_remote_playlist_service(
        &self,
        service: Arc<crate::remote_playlists::RemotePlaylistService>,
    ) {
        if let Ok(mut slot) = self.inner.remote_playlists.lock() {
            *slot = Some(service);
        }
    }
    pub(crate) async fn prepare_catalog_duration_track(
        &self,
        track: &Track,
        cancel: watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> Result<Option<crate::remote_playlists::PreparedCatalogDuration>, String> {
        let service = self
            .inner
            .remote_playlists
            .lock()
            .map_err(|_| "Catalog service unavailable")?
            .clone();
        match service {
            Some(service) => {
                service
                    .prepare_catalog_duration_track(track, cancel, deadline)
                    .await
            }
            None => Ok(None),
        }
    }
    pub(crate) async fn prepare_alias_track(
        &self,
        track: &Track,
        cancel: watch::Receiver<bool>,
        deadline: tokio::time::Instant,
        trace: &crate::playback_sources::SourceTrace,
    ) -> Result<Track, String> {
        let service = self
            .inner
            .remote_playlists
            .lock()
            .map_err(|_| "Alias service unavailable")?
            .clone()
            .ok_or("Alias service unavailable")?;
        let started = std::time::Instant::now();
        let result = service.prepare_alias_track(track, cancel, deadline).await;
        trace.event(
            "artist_alias_hydration",
            "spotify",
            serde_json::json!({
            "elapsedMs":started.elapsed().as_millis(), "count":usize::from(result.is_ok()),
            "category":if result.is_ok() {"complete"} else {"unavailable"}}),
        );
        result
    }

    pub fn set_analysis_manager(&self, analysis: Arc<crate::analysis::AnalysisManager>) {
        if let Ok(mut slot) = self.inner.analysis.lock() {
            *slot = Some(analysis);
        }
    }
    pub fn set_autotag_manager(&self, manager: Arc<crate::autotag::AutoTagger>) {
        if let Ok(mut slot) = self.inner.autotag.lock() {
            *slot = Some(manager);
        }
    }
    fn autotag_download(&self, asset: &LocalAsset) {
        if let Ok(slot) = self.inner.autotag.lock() {
            if let Some(manager) = slot.as_ref() {
                let _ = manager.queue_new_download(&asset.id);
            }
        }
    }
    pub async fn search_audio_tracks(
        &self,
        query: &str,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Track>, String> {
        if provider_ids.len() == 1 && provider_ids[0] == "monochrome" {
            let rows = crate::monochrome::Client::new()?.search(query).await?;
            return rows
                .into_iter()
                .map(|(track, _)| self.inner.store.cache_track(&track))
                .collect();
        }
        let query = query.trim();
        if query.is_empty() {
            return Err("Enter a search query".into());
        }
        if query.chars().count() > 500 {
            return Err("Search query must be 500 characters or fewer".into());
        }
        let providers = provider_ids
            .iter()
            .filter(|provider| matches!(provider.as_str(), "youtube" | "soundcloud"))
            .cloned()
            .collect::<Vec<_>>();
        if providers.is_empty() {
            return Err("Choose YouTube or SoundCloud for audio search".into());
        }
        let key = serde_json::json!([
            "raw-audio-search-v1",
            query,
            providers,
            settings.yt_dlp_path,
            settings.js_runtime_path,
        ])
        .to_string();
        self.inner
            .raw_audio_search_cache
            .get_or_search(key, || search_audio_tracks_raw(query, &providers, settings))
            .await
    }
    pub async fn search_audio_tracks_page(
        &self,
        query: &str,
        provider: &str,
        offset: usize,
        limit: usize,
        settings: &Settings,
    ) -> Result<AudioSearchPage, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err("Search query must contain 1 to 500 characters".into());
        }
        if !matches!(provider, "youtube" | "soundcloud") {
            return Err("Choose YouTube or SoundCloud for audio search".into());
        }
        let key = serde_json::json!([
            "raw-audio-search-page-v1",
            query,
            provider,
            offset,
            limit,
            settings.yt_dlp_path,
            settings.js_runtime_path,
        ])
        .to_string();
        self.inner
            .raw_audio_search_cache
            .get_or_search_page(key, || {
                search_audio_tracks_page_raw(query, provider, offset, limit, settings)
            })
            .await
    }

    pub async fn start_slskd_search(&self, query: &str) -> Result<String, String> {
        self.start_slskd_search_with_retention(query, true).await
    }

    async fn start_slskd_search_ephemeral(&self, query: &str) -> Result<String, String> {
        self.start_slskd_search_with_retention(query, false).await
    }

    async fn start_slskd_search_with_retention(
        &self,
        query: &str,
        retain_completed_results: bool,
    ) -> Result<String, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err("Search query must contain 1 to 500 characters".into());
        }
        let connection = self
            .inner
            .connections
            .slskd()?
            .ok_or("Enable and configure slskd in Connections first")?;
        let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
        let id = client.start_search(query, 15).await?;
        self.inner
            .slskd_searches
            .lock()
            .map_err(|_| "slskd search state lock poisoned")?
            .insert(
                id.clone(),
                SlskdSearchSession {
                    started: Instant::now(),
                    retain_completed_results,
                    poll_gate: Arc::new(tokio::sync::Mutex::new(())),
                },
            );
        Ok(id)
    }

    pub async fn poll_slskd_search(&self, search_id: &str) -> Result<SlskdSearchPoll, String> {
        if let Some(result) = cached_slskd_search(&self.inner.slskd_completed, search_id)? {
            return Ok(result);
        }

        let poll_gate = match slskd_search_poll_gate(&self.inner.slskd_searches, search_id) {
            Ok(gate) => gate,
            Err(error) => {
                // Completion can land between the first cache check and the
                // session lookup. The completed result is authoritative then.
                if let Some(result) = cached_slskd_search(&self.inner.slskd_completed, search_id)? {
                    return Ok(result);
                }
                return Err(error);
            }
        };
        // Serialize polls for a single native search. A waiter rechecks the
        // result cache after taking ownership, so it observes the first poll's
        // completed result instead of asking slskd for an already deleted ID.
        let _poll_guard = poll_gate.lock().await;
        if let Some(result) = cached_slskd_search(&self.inner.slskd_completed, search_id)? {
            return Ok(result);
        }
        let (session, retain_completed_results) =
            active_slskd_search_session(&self.inner.slskd_searches, search_id)?;
        let connection = self
            .inner
            .connections
            .slskd()?
            .ok_or("slskd is disabled in Connections")?;
        let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
        let mut status = client.search_status(search_id).await?;
        let mut separate_responses = client.search_responses(search_id).await.unwrap_or_default();
        let mut responses = merge_slskd_search_responses(&status.responses, &separate_responses);
        let complete_from_status = |status: &crate::slskd::SearchStatus, elapsed: Duration| {
            let state = status
                .state
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            status.is_complete.unwrap_or(false)
                || state.contains("complete")
                || state.contains("timedout")
                || state.contains("cancelled")
                || elapsed >= Duration::from_secs(125)
        };
        // slskd can mark a search complete before the response endpoint has
        // published the rows. When its own counters prove rows are pending,
        // give that propagation one short bounded interval and merge again.
        let visible_peer_count = responses.len();
        let visible_file_count = slskd_response_file_count(&responses);
        let advertised_rows_pending = status
            .response_count
            .is_some_and(|count| count > visible_peer_count)
            || status
                .file_count
                .is_some_and(|count| count > visible_file_count);
        let mut settle_ms = 0;
        if complete_from_status(&status, session.elapsed()) && advertised_rows_pending {
            tokio::time::sleep(Duration::from_millis(200)).await;
            settle_ms = 200;
            if let Ok(refreshed_status) = client.search_status(search_id).await {
                status = refreshed_status;
            }
            separate_responses = client.search_responses(search_id).await.unwrap_or_default();
            responses = merge_slskd_search_responses(&status.responses, &separate_responses);
        }
        let embedded_response_count = status.responses.len();
        let separate_response_count = separate_responses.len();
        let raw_file_count = slskd_response_file_count(&responses);
        let mut candidates = crate::slskd::candidates_from_responses(&responses, search_id);
        trace_soulseek_album(
            "poll_response_visibility",
            serde_json::json!({
                "embeddedResponseCount": embedded_response_count,
                "separateResponseCount": separate_response_count,
                "rawFileCount": raw_file_count,
                "normalizedCandidateCount": candidates.len(),
                "advertisedResponseCount": status.response_count,
                "advertisedFileCount": status.file_count,
                "state": status.state.as_deref().unwrap_or("unknown"),
                "settleMs": settle_ms,
            }),
        );
        // Keep recording identity/confidence unchanged; availability orders peer copies only.
        let history = self.inner.store.list_jobs()?;
        candidates.sort_by_key(|candidate| {
            (
                slskd_peer_failure_order(candidate, &history),
                slskd_availability_order(candidate),
            )
        });
        let tracks = candidates
            .iter()
            .map(slskd_candidate_track)
            .collect::<Vec<_>>();
        let state = status.state.clone().unwrap_or_else(|| {
            if status.is_complete.unwrap_or(false) {
                "complete"
            } else {
                "searching"
            }
            .into()
        });
        let complete = complete_from_status(&status, session.elapsed());
        let mut warnings = Vec::new();
        if session.elapsed() >= Duration::from_secs(120) && !complete {
            warnings.push("The slskd search timed out while waiting for results".into());
        }
        if candidates.is_empty() && complete {
            warnings.push("No audio files were returned by this search".into());
        }
        let result = SlskdSearchPoll {
            id: search_id.into(),
            state,
            tracks,
            candidates,
            complete,
            warnings,
        };
        if complete && retain_completed_results {
            remember_completed_slskd_search(&self.inner.slskd_completed, search_id, &result)?;
        }
        if complete {
            let _ = client.delete_search(search_id).await;
            self.inner
                .slskd_searches
                .lock()
                .map_err(|_| "slskd search state lock poisoned")?
                .remove(search_id);
        }
        Ok(result)
    }

    async fn find_slskd_candidates_for_track(
        &self,
        track: &Track,
    ) -> Result<Vec<Candidate>, String> {
        let query = [
            track.artist.as_str(),
            track.title.as_str(),
            track.version.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        let id = self.start_slskd_search_ephemeral(&query).await?;
        let deadline = Instant::now() + Duration::from_secs(18);
        let mut seen = HashMap::<String, Candidate>::new();
        loop {
            let page = self.poll_slskd_search(&id).await?;
            for mut candidate in page.candidates {
                let availability = candidate
                    .reasons
                    .iter()
                    .filter(|reason| reason.starts_with("availability:"))
                    .cloned()
                    .collect::<Vec<_>>();
                matching::score_candidate(track, &mut candidate);
                candidate.reasons.extend(availability);
                candidate.confidence = "review".into();
                candidate.permission = "unknown".into();
                seen.insert(candidate.id.clone(), candidate);
            }
            if page.complete {
                let mut candidates = seen.into_values().collect::<Vec<_>>();
                let history = self.inner.store.list_jobs()?;
                // This is a tie-breaker after retaining all identity scores:
                // peers with an open upload slot are more likely to start now.
                candidates.sort_by(|a, b| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| {
                            slskd_peer_failure_order(a, &history)
                                .cmp(&slskd_peer_failure_order(b, &history))
                        })
                        .then_with(|| slskd_availability_order(a).cmp(&slskd_availability_order(b)))
                });
                return Ok(candidates);
            }
            if Instant::now() >= deadline {
                let _ = self.cancel_slskd_search(&id).await;
                let mut candidates = seen.into_values().collect::<Vec<_>>();
                let history = self.inner.store.list_jobs()?;
                candidates.sort_by(|a, b| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| {
                            slskd_peer_failure_order(a, &history)
                                .cmp(&slskd_peer_failure_order(b, &history))
                        })
                        .then_with(|| slskd_availability_order(a).cmp(&slskd_availability_order(b)))
                });
                return Ok(candidates);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Search Soulseek once for an album, then retain only files that form a
    /// coherent same-peer, same-directory set and independently pass the
    /// shared recording matcher against one unique requested track.
    pub async fn find_soulseek_album_candidates(
        &self,
        tracks: &[Track],
        mut cancel: watch::Receiver<bool>,
    ) -> Result<Vec<SoulseekAlbumAssignment>, String> {
        if tracks.is_empty() || tracks.len() > 100 {
            return Err("Album Soulseek search requires 1 to 100 tracks".into());
        }
        let album = tracks[0]
            .album
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("Album Soulseek search requires album metadata")?;
        let album_key = matching::normalize(album);
        let mut seen_track_ids = std::collections::HashSet::new();
        if tracks.iter().any(|track| {
            track.id.trim().is_empty()
                || track.title.trim().is_empty()
                || matching::normalize(track.album.as_deref().unwrap_or_default()) != album_key
                || !seen_track_ids.insert(track.id.as_str())
        }) {
            return Err(
                "Album Soulseek search requires distinct tracks from one named album".into(),
            );
        }
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let operation_started = Instant::now();
        let deadline = operation_started + Duration::from_secs(22);
        let alias_started = Instant::now();
        let alias_contexts = tracks
            .iter()
            .map(|track| {
                crate::artist_credit_aliases::cached_verified_playback_artist_aliases(
                    &self.inner.store,
                    track,
                )
            })
            .collect::<Vec<_>>();
        let aliases = alias_contexts[0].search_artist_candidates();
        let query = soulseek_album_search_query(&tracks[0], album, &aliases)?;
        let queries = if query == album.trim() {
            vec![query]
        } else {
            vec![query, album.trim().to_owned()]
        };
        let alias_prep_ms = alias_started.elapsed().as_millis();
        let selection_deadline = deadline - Duration::from_secs(2);
        trace_soulseek_album(
            "begin",
            serde_json::json!({
                "trackCount": tracks.len(),
                "trackInputs": soulseek_album_track_input_diagnostics(tracks, &alias_contexts),
                "queryCount": queries.len(),
                "queryHashes": queries.iter().map(|query| soulseek_album_query_fingerprint(query)).collect::<Vec<_>>(),
                "aliasPrepMs": alias_prep_ms,
                "deadlineMs": 22_000,
            }),
        );
        let matching_level = self.inner.store.settings()?.matching_level;
        for (index, query) in queries.iter().enumerate() {
            if *cancel.borrow() || Instant::now() >= selection_deadline {
                break;
            }
            let query_started = Instant::now();
            let query_deadline = if index + 1 < queries.len() {
                (operation_started + Duration::from_secs(10)).min(selection_deadline)
            } else {
                selection_deadline
            };
            let query_selection_deadline = if index + 1 < queries.len() {
                (operation_started + Duration::from_secs(12)).min(selection_deadline)
            } else {
                selection_deadline
            };
            let (candidates, search_diagnostics) = self
                .collect_soulseek_album_search(query, &mut cancel, query_deadline)
                .await?;
            let network_ms = query_started.elapsed().as_millis();
            let candidate_count = candidates.len();
            let selection_started = Instant::now();
            let selection = select_soulseek_album_group(
                &self.inner.store,
                tracks,
                &alias_contexts,
                candidates,
                matching_level,
                query_selection_deadline,
                &cancel,
            );
            let selection_ms = selection_started.elapsed().as_millis();
            let assignments = selection.assignments;
            trace_soulseek_album(
                "query_complete",
                serde_json::json!({
                    "queryIndex": index,
                    "queryHash": soulseek_album_query_fingerprint(query),
                    "queryLength": query.chars().count(),
                    "aliasPrepMs": alias_prep_ms,
                    "networkMs": network_ms,
                    "selectionMs": selection_ms,
                    "candidateCount": candidate_count,
                    "searchComplete": search_diagnostics.complete,
                    "searchPollCount": search_diagnostics.poll_count,
                    "searchResponseRows": search_diagnostics.response_rows,
                    "searchUniqueCandidates": search_diagnostics.unique_candidates,
                    "searchFinalState": search_diagnostics.final_state,
                    "searchWarningCount": search_diagnostics.warning_count,
                    "assignmentCount": assignments.len(),
                    "groupsTotal": selection.groups_total,
                    "groupsExamined": selection.groups_examined,
                    "candidatePairsTotal": selection.candidate_pairs_total,
                    "candidatePairsAfterOrdinal": selection.candidate_pairs_after_ordinal,
                    "candidatePairsAfterTitle": selection.candidate_pairs_after_title,
                    "sharedAssessments": selection.shared_assessments,
                    "assessmentRejections": selection.assessment_rejections,
                    "bestCoverageAtStop": selection.best_coverage_at_stop,
                    "selectionStoppedOnDeadline": selection.stopped_on_deadline,
                    "selectedFullCoverage": assignments.len() == tracks.len(),
                    "deadlineRemainingMs": query_selection_deadline.saturating_duration_since(Instant::now()).as_millis(),
                    "cancelled": *cancel.borrow(),
                }),
            );
            let deadline_reached = Instant::now() >= selection_deadline;
            match soulseek_album_query_decision(
                *cancel.borrow(),
                deadline_reached,
                !assignments.is_empty(),
                index + 1 == queries.len(),
            ) {
                SoulseekAlbumQueryDecision::Cancel => {
                    trace_soulseek_album(
                        "end",
                        serde_json::json!({
                            "elapsedMs": operation_started.elapsed().as_millis(),
                            "deadlineReached": deadline_reached,
                            "cancelled": true,
                            "assignmentCount": assignments.len(),
                        }),
                    );
                    return Err("Cancelled".into());
                }
                SoulseekAlbumQueryDecision::Return => {
                    trace_soulseek_album(
                        "end",
                        serde_json::json!({
                            "elapsedMs": operation_started.elapsed().as_millis(),
                            "deadlineReached": deadline_reached,
                            "cancelled": false,
                            "assignmentCount": assignments.len(),
                        }),
                    );
                    return Ok(assignments);
                }
                SoulseekAlbumQueryDecision::Continue => {}
            }
        }
        trace_soulseek_album(
            "end",
            serde_json::json!({
                "elapsedMs": operation_started.elapsed().as_millis(),
                "deadlineReached": Instant::now() >= selection_deadline,
                "cancelled": *cancel.borrow(),
            }),
        );
        Ok(Vec::new())
    }

    async fn collect_soulseek_album_search(
        &self,
        query: &str,
        cancel: &mut watch::Receiver<bool>,
        deadline: Instant,
    ) -> Result<(Vec<Candidate>, SoulseekAlbumSearchDiagnostics), String> {
        let search_id = tokio::select! {
            result = self.start_slskd_search_ephemeral(query) => result?,
            _ = cancel.changed() => return Err("Cancelled".into()),
            _ = tokio::time::sleep_until(deadline.into()) => return Ok((Vec::new(), SoulseekAlbumSearchDiagnostics {
                final_state: "start_timeout".into(),
                ..SoulseekAlbumSearchDiagnostics::default()
            })),
        };
        let mut candidates = HashMap::<String, Candidate>::new();
        let mut diagnostics = SoulseekAlbumSearchDiagnostics::default();
        let complete = loop {
            if *cancel.borrow() {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    self.cancel_slskd_search(&search_id),
                )
                .await;
                return Err("Cancelled".into());
            }
            let page = tokio::select! {
                result = self.poll_slskd_search(&search_id) => match result {
                    Ok(page) => page,
                    Err(error) => {
                        let _ = tokio::time::timeout(Duration::from_secs(2), self.cancel_slskd_search(&search_id)).await;
                        return Err(error);
                    }
                },
                _ = cancel.changed() => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), self.cancel_slskd_search(&search_id)).await;
                    return Err("Cancelled".into());
                }
                _ = tokio::time::sleep_until(deadline.into()) => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), self.cancel_slskd_search(&search_id)).await;
                    break false;
                }
            };
            diagnostics.poll_count = diagnostics.poll_count.saturating_add(1);
            diagnostics.response_rows = diagnostics
                .response_rows
                .saturating_add(page.candidates.len());
            diagnostics.warning_count = page.warnings.len();
            diagnostics.final_state = match page.state.as_str() {
                "completed" | "complete" | "done" => "complete".into(),
                "inprogress" | "in_progress" | "running" => "in_progress".into(),
                "queued" | "pending" => "queued".into(),
                "cancelled" | "canceled" => "cancelled".into(),
                "failed" | "error" => "failed".into(),
                _ => "other".into(),
            };
            for mut candidate in page.candidates {
                if let Some(CandidateSource::Soulseek { search_id, .. }) = candidate.source.as_mut()
                {
                    *search_id = None;
                }
                candidates.insert(candidate.id.clone(), candidate);
            }
            if page.complete {
                break true;
            }
            if Instant::now() >= deadline {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    self.cancel_slskd_search(&search_id),
                )
                .await;
                break false;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(350)) => {},
                _ = cancel.changed() => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), self.cancel_slskd_search(&search_id)).await;
                    return Err("Cancelled".into());
                }
                _ = tokio::time::sleep_until(deadline.into()) => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), self.cancel_slskd_search(&search_id)).await;
                    break false;
                }
            }
        };
        let candidates = candidates.into_values().collect::<Vec<_>>();
        diagnostics.complete = complete;
        diagnostics.unique_candidates = candidates.len();
        Ok((candidates, diagnostics))
    }

    pub async fn cancel_slskd_search(&self, search_id: &str) -> Result<(), String> {
        let connection = self
            .inner
            .connections
            .slskd()?
            .ok_or("slskd is disabled in Connections")?;
        let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
        client.cancel_search(search_id).await?;
        let _ = client.delete_search(search_id).await;
        self.inner
            .slskd_searches
            .lock()
            .map_err(|_| "slskd search state lock poisoned")?
            .remove(search_id);
        Ok(())
    }
    fn update(&self, job: &DownloadJob) -> Result<(), String> {
        self.inner.store.save_job(job)?;
        if let Some(app) = &self.inner.app {
            let _ = app.emit("download-progress", job);
        }
        Ok(())
    }
    async fn cached_preview_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
    ) -> Result<Vec<Candidate>, String> {
        let key = search_cache_key(track, provider_ids, settings, 3);
        self.inner
            .search_cache
            .get_or_search_cancelled(key, cancel.clone(), || {
                find_candidates_with_cancel(
                    track,
                    provider_ids,
                    settings,
                    cancel,
                    3,
                    PREVIEW_SEARCH_TIMEOUT,
                )
            })
            .await
    }

    pub async fn prefetch_preview_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Candidate>, String> {
        let (_tx, cancel) = idle_cancel();
        self.prefetch_preview_candidates_cancelled(track, provider_ids, settings, cancel)
            .await
    }

    pub async fn prefetch_preview_candidates_cancelled(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
    ) -> Result<Vec<Candidate>, String> {
        self.cached_preview_candidates(track, provider_ids, settings, cancel)
            .await
    }

    pub async fn prefetch_playback_candidates_cancelled(
        &self,
        track: &Track,
        alternate_artists: &[String],
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<Vec<Candidate>, String> {
        self.playback_candidates_cached(
            track,
            alternate_artists,
            provider_ids,
            settings,
            cancel.clone(),
            deadline,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn prefetch_playback_candidates_cancelled_traced(
        &self,
        track: &Track,
        alternate_artists: &[String],
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
        trace: crate::playback_sources::SourceTrace,
    ) -> Result<Vec<Candidate>, String> {
        self.playback_candidates_cached(
            track,
            alternate_artists,
            provider_ids,
            settings,
            cancel,
            deadline,
            Some(trace),
            None,
        )
        .await
    }

    /// Warm only the flat provider discovery rows. These entries are never
    /// returned as a completed playback search: foreground still performs its
    /// request-scoped optional catalog work before populating the full cache.
    ///
    /// This variant warms only the canonical first query, so alias preparation
    /// can overlap retrieval without spending the ordinary second-query slot.
    pub(crate) async fn warm_canonical_playback_query_cancelled(
        &self,
        track: &Track,
        provider: &str,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<usize, String> {
        let search_track = track_for_playback_source_search(track);
        let (candidates, errors) = search_provider_candidates(
            provider,
            &search_track,
            settings,
            cancel.clone(),
            PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT,
            PREVIEW_SEARCH_TIMEOUT,
            Some(deadline),
            None,
            Some(&self.inner.search_cache),
            true,
        )
        .await;
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        if candidates.is_empty() && !errors.is_empty() {
            Err(errors.join("; "))
        } else {
            Ok(candidates.len())
        }
    }

    pub(crate) async fn warm_playback_candidates_cancelled(
        &self,
        track: &Track,
        alternate_artists: &[String],
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<usize, String> {
        let search_track = track_for_playback_source_search(track);
        let alternate_artist = alternate_artists.first().cloned();
        let mut warmed_count = 0;
        for provider in provider_ids {
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
            if deadline.remaining().is_zero() {
                return Err("Playback candidate search timed out".into());
            }
            let key = flat_playback_candidate_search_cache_key(
                track,
                provider,
                settings,
                alternate_artists,
            );
            let found = self
                .inner
                .search_cache
                .get_or_search_cancelled_observed_decided_with_completeness(
                    key,
                    cancel.clone(),
                    None,
                    || async {
                        find_candidates_with_cancel_until_alias_detailed(
                            &search_track,
                            std::slice::from_ref(provider),
                            settings,
                            cancel.clone(),
                            PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT,
                            PREVIEW_SEARCH_TIMEOUT,
                            Some(deadline),
                            alternate_artist.clone(),
                            Some(&self.inner.search_cache),
                        )
                        .await
                    },
                )
                .await?;
            warmed_count += found.0.len();
        }
        Ok(warmed_count)
    }

    #[cfg(feature = "playback-early-candidate-experiment")]
    pub(crate) async fn prefetch_playback_candidates_with_early_observer(
        &self,
        track: &Track,
        alternate_artists: &[String],
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
        trace: crate::playback_sources::SourceTrace,
        observe: Box<dyn FnOnce(Vec<Candidate>) + Send>,
    ) -> Result<Vec<Candidate>, String> {
        self.playback_candidates_cached(
            track,
            alternate_artists,
            provider_ids,
            settings,
            cancel,
            deadline,
            Some(trace),
            Some(observe),
        )
        .await
    }

    async fn playback_candidates_cached(
        &self,
        track: &Track,
        alternate_artists: &[String],
        provider_ids: &[String],
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        deadline: crate::playback_sources::PlaybackDeadline,
        trace: Option<crate::playback_sources::SourceTrace>,
        observe_flat_batch: Option<Box<dyn FnOnce(Vec<Candidate>) + Send>>,
    ) -> Result<Vec<Candidate>, String> {
        let search_track = track_for_playback_source_search(track);
        // The completed cache stores only full request-scoped results; warmup
        // rows live under a separate flat-only key and are merged below.
        let key =
            playback_candidate_search_cache_key(track, provider_ids, settings, alternate_artists);
        let alternate_artist = alternate_artists.first().cloned();
        let flat_key = if provider_ids.len() == 1 {
            Some(flat_playback_candidate_search_cache_key(
                track,
                &provider_ids[0],
                settings,
                alternate_artists,
            ))
        } else {
            None
        };
        self.inner
            .search_cache
            .get_or_search_cancelled_observed_decided(key, cancel.clone(), trace, || async {
                let search_flat = || async {
                    find_candidates_with_cancel_until_alias_detailed(
                        &search_track,
                        provider_ids,
                        settings,
                        cancel.clone(),
                        PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT,
                        PREVIEW_SEARCH_TIMEOUT,
                        Some(deadline),
                        alternate_artist.clone(),
                        Some(&self.inner.search_cache),
                    )
                    .await
                };
                let (found, flat_complete) = if let Some(flat_key) = flat_key {
                    self.inner
                        .search_cache
                        .get_or_search_cancelled_observed_decided_with_completeness(
                            flat_key,
                            cancel.clone(),
                            None,
                            search_flat,
                        )
                        .await?
                } else {
                    search_flat().await?
                };
                let (completed, catalog_complete) =
                    complete_playback_candidate_search_observed_with_level(
                        &search_track,
                        provider_ids,
                        alternate_artist.as_deref(),
                        found,
                        cancel.clone(),
                        deadline,
                        || async {
                            let query = format!("{} {}", search_track.artist, search_track.title);
                            let items = crate::youtube_music::search_playback_candidates(
                                &query,
                                10,
                                cancel.clone(),
                                deadline,
                            )
                            .await?;
                            Ok(playback_music_catalog_candidates(&search_track, items))
                        },
                        observe_flat_batch,
                        settings.matching_level,
                    )
                    .await?;
                Ok((completed, flat_complete && catalog_complete))
            })
            .await
    }

    pub async fn find_preview_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
        generation: u64,
    ) -> Result<Vec<Candidate>, String> {
        let cancel = self.inner.preview_search.begin(generation)?;
        let result = self
            .cached_preview_candidates(track, provider_ids, settings, cancel)
            .await;
        if !self.inner.preview_search.finish(generation)? {
            return Err("Preview search superseded by a newer request".into());
        }
        result
    }

    pub async fn resolve_preview_audio(
        &self,
        candidate: &Candidate,
        settings: &Settings,
    ) -> Result<PreviewAudio, String> {
        let (_tx, cancel) = idle_cancel();
        self.resolve_preview_audio_cancelled(candidate, settings, cancel)
            .await
    }

    pub async fn resolve_preview_audio_cancelled(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
    ) -> Result<PreviewAudio, String> {
        if candidate.provider == "bandcamp" {
            return resolve_preview_audio_with_options(candidate, settings, cancel, false).await;
        }
        let key = preview_audio_cache_key(candidate, settings, false);
        self.inner
            .preview_audio_cache
            .get_or_resolve_cancelled(key, cancel.clone(), None, |_| {
                resolve_preview_audio_with_options(candidate, settings, cancel, false)
            })
            .await
    }

    pub(crate) fn playback_provider_stamp(
        &self,
        provider: &str,
    ) -> Result<ProviderSessionStamp, String> {
        // Supported provider names bound this map independently of recording count.
        if !matches!(
            provider,
            "youtube" | "soundcloud" | "bandcamp" | "monochrome"
        ) {
            return Err("Unsupported playback provider".into());
        }
        let epoch = self
            .inner
            .playback_session_epochs
            .lock()
            .map_err(|_| "Provider session state lock poisoned")?
            .entry(provider.to_owned())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        let expected = epoch.load(Ordering::SeqCst);
        Ok(ProviderSessionStamp { epoch, expected })
    }

    /// Playback must verify the full extractor identity before touching its CDN URL.
    /// This cache is deliberately separate from the cache of access-validated audio.
    pub(crate) async fn extract_playback_audio_traced_preparing_alternate(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
        prepared: Option<Arc<Mutex<Option<PreviewAudio>>>>,
    ) -> Result<PreviewAudio, String> {
        let stamp = self.playback_provider_stamp(&candidate.provider)?;
        let trace = trace.with_event_provider(&candidate.provider);
        let key = preview_audio_cache_key(candidate, settings, false);
        self.inner
            .playback_metadata_cache
            .get_or_resolve_cancelled(
                key,
                cancel.clone(),
                Some(trace.clone()),
                |worker_id| async move {
                    let prepared_slot = prepared.clone();
                    let mut audio = extract_preview_audio_with_prepared_alternate(
                        candidate,
                        settings,
                        cancel,
                        false,
                        Some(trace.with_shared_lookup(worker_id)),
                        deadline,
                        prepared,
                        None,
                    )
                    .await?;
                    audio.provider_session = Some(stamp.clone());
                    if !audio.provider_session_current() {
                        return Err(
                            "Audio source provider session changed during resolution".into()
                        );
                    }
                    if let Some(slot) = prepared_slot {
                        if let Ok(mut slot) = slot.lock() {
                            if let Some(alternate) = slot.as_mut() {
                                alternate.provider_session = Some(stamp);
                            }
                        }
                    }
                    Ok(audio)
                },
            )
            .await
    }

    pub(crate) async fn extract_playback_audio_fresh_or_prepared_traced(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        alternate_format: bool,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
        prepared: Option<PreviewAudio>,
    ) -> Result<PreviewAudio, String> {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        if let Some(audio) = prepared {
            if !audio.provider_session_current() {
                return Err("Audio source provider session changed during resolution".into());
            }
            trace.event(
                "prepared_alternate_metadata_reused",
                &candidate.provider,
                serde_json::json!({"extractionReused":true}),
            );
            return Ok(audio);
        }
        let stamp = self.playback_provider_stamp(&candidate.provider)?;
        self.invalidate_preview_audio(candidate, settings);
        let trace = trace.with_event_provider(&candidate.provider);
        let key = preview_audio_cache_key(candidate, settings, alternate_format);
        self.inner
            .playback_metadata_cache
            .get_or_resolve_cancelled(
                key,
                cancel.clone(),
                Some(trace.clone()),
                |worker_id| async move {
                    let mut audio = extract_preview_audio_with_prepared_alternate(
                        candidate,
                        settings,
                        cancel,
                        alternate_format,
                        Some(trace.with_shared_lookup(worker_id)),
                        deadline,
                        None,
                        None,
                    )
                    .await?;
                    audio.provider_session = Some(stamp);
                    if !audio.provider_session_current() {
                        return Err(
                            "Audio source provider session changed during resolution".into()
                        );
                    }
                    Ok(audio)
                },
            )
            .await
    }

    /// Alternate direct responses do not enter URL/access caches until their
    /// response TTL and profile provenance can be represented in cache keys.
    pub(crate) async fn validate_playback_audio_uncached_traced(
        &self,
        candidate: &Candidate,
        audio: PreviewAudio,
        cancel: watch::Receiver<bool>,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<PreviewAudio, String> {
        let audio = validate_prepared_preview_audio(
            audio,
            cancel,
            Some(trace),
            &candidate.provider,
            deadline,
        )
        .await?;
        if !audio.provider_session_current() {
            return Err("Audio source provider session changed during resolution".into());
        }
        Ok(audio)
    }

    pub(crate) async fn validate_playback_audio_traced(
        &self,
        candidate: &Candidate,
        audio: PreviewAudio,
        cancel: watch::Receiver<bool>,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<PreviewAudio, String> {
        if !audio.provider_session_current() {
            return Err("Audio source provider session changed during resolution".into());
        }
        if matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
            let key = playback_access_cache_key(candidate, &audio);
            let probe_audio = audio.clone();
            self.inner
                .playback_access_cache
                .get_or_resolve_cancelled(key, cancel.clone(), Some(trace.clone()), |worker_id| {
                    validate_prepared_preview_audio(
                        probe_audio,
                        cancel,
                        Some(trace.with_shared_lookup(worker_id)),
                        &candidate.provider,
                        deadline,
                    )
                })
                .await?;
        }
        if !audio.provider_session_current() {
            return Err("Audio source provider session changed during resolution".into());
        }
        // Cached access is proof for the bytes only; preserve the exact metadata
        // whose recording identity the caller just verified.
        Ok(audio)
    }

    pub(crate) async fn resolve_preview_audio_traced(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<PreviewAudio, String> {
        self.resolve_preview_audio_traced_preparing_alternate(
            candidate, settings, cancel, trace, deadline, None,
        )
        .await
    }

    pub(crate) async fn resolve_preview_audio_traced_preparing_alternate(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
        prepared: Option<Arc<Mutex<Option<PreviewAudio>>>>,
    ) -> Result<PreviewAudio, String> {
        if candidate.provider == "bandcamp" {
            return resolve_preview_audio_with_trace(
                candidate,
                settings,
                cancel,
                false,
                Some(trace),
                deadline,
            )
            .await;
        }
        let trace = trace.with_event_provider(&candidate.provider);
        let key = preview_audio_cache_key(candidate, settings, false);
        self.inner
            .preview_audio_cache
            .get_or_resolve_cancelled(key, cancel.clone(), Some(trace.clone()), |worker_id| {
                resolve_preview_audio_with_prepared_alternate(
                    candidate,
                    settings,
                    cancel,
                    false,
                    Some(trace.with_shared_lookup(worker_id)),
                    deadline,
                    prepared,
                )
            })
            .await
    }

    pub(crate) async fn resolve_preview_audio_fresh_traced_cancelled(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        alternate_format: bool,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
    ) -> Result<PreviewAudio, String> {
        self.resolve_preview_audio_fresh_or_prepared_traced(
            candidate,
            settings,
            cancel,
            alternate_format,
            trace,
            deadline,
            None,
        )
        .await
    }

    pub(crate) async fn resolve_preview_audio_fresh_or_prepared_traced(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        alternate_format: bool,
        trace: crate::playback_sources::SourceTrace,
        deadline: crate::playback_sources::PlaybackDeadline,
        prepared: Option<PreviewAudio>,
    ) -> Result<PreviewAudio, String> {
        if candidate.provider == "bandcamp" {
            return resolve_preview_audio_with_trace(
                candidate,
                settings,
                cancel,
                alternate_format,
                Some(trace),
                deadline,
            )
            .await;
        }
        let trace = trace.with_event_provider(&candidate.provider);
        self.invalidate_preview_audio(candidate, settings);
        let key = preview_audio_cache_key(candidate, settings, alternate_format);
        self.inner
            .preview_audio_cache
            .get_or_resolve_cancelled(
                key,
                cancel.clone(),
                Some(trace.clone()),
                |worker_id| async move {
                    if let Some(audio) = prepared {
                        trace.event(
                            "prepared_alternate_validation_start",
                            &candidate.provider,
                            serde_json::json!({"extractionReused":true}),
                        );
                        return validate_prepared_preview_audio(
                            audio,
                            cancel,
                            Some(trace.with_shared_lookup(worker_id)),
                            &candidate.provider,
                            deadline,
                        )
                        .await;
                    }
                    resolve_preview_audio_with_trace(
                        candidate,
                        settings,
                        cancel,
                        alternate_format,
                        Some(trace.with_shared_lookup(worker_id)),
                        deadline,
                    )
                    .await
                },
            )
            .await
    }

    /// Resolve a fresh, preflighted stream for the same candidate, bypassing cached signed URLs.
    pub async fn resolve_preview_audio_fresh_cancelled(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        alternate_format: bool,
    ) -> Result<PreviewAudio, String> {
        if candidate.provider == "bandcamp" {
            return resolve_preview_audio_with_options(
                candidate,
                settings,
                cancel,
                alternate_format,
            )
            .await;
        }
        self.invalidate_preview_audio(candidate, settings);
        let key = preview_audio_cache_key(candidate, settings, alternate_format);
        self.inner
            .preview_audio_cache
            .get_or_resolve_cancelled(key, cancel.clone(), None, |_| {
                resolve_preview_audio_with_options(candidate, settings, cancel, alternate_format)
            })
            .await
    }

    /// Resolve a fresh small AAC representation for album waveform evidence.
    /// This bypasses the ordinary quality/cache path; the candidate identity,
    /// duration and audio access checks still run before returning the URL.
    pub(crate) async fn resolve_album_evidence_audio_fresh_cancelled(
        &self,
        candidate: &Candidate,
        settings: &Settings,
        cancel: watch::Receiver<bool>,
        failure_stage: &mut &'static str,
        failure_diagnostic: &mut Option<Value>,
    ) -> Result<PreviewAudio, String> {
        self.invalidate_preview_audio(candidate, settings);
        let deadline = preview_operation_deadline(settings);
        *failure_stage = "source_audio_extract";
        let audio = extract_album_evidence_preview_audio(candidate, settings, cancel.clone())
            .await
            .map_err(|_| "Fresh evidence audio extraction failed")?;
        let codec_is_aac = audio.codec.as_deref().is_some_and(|codec| {
            codec.eq_ignore_ascii_case("aac") || codec.to_ascii_lowercase().starts_with("mp4a.")
        });
        if audio.mime_type != "audio/mp4" || !codec_is_aac {
            *failure_stage = "source_audio_format";
            *failure_diagnostic = Some(album_evidence_audio_diagnostic("format_rejected", &audio));
            return Err("Album evidence requires an AAC/M4A source representation".into());
        }
        if matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
            *failure_stage = "source_audio_access";
            validate_prepared_preview_audio(
                audio.clone(),
                cancel,
                None,
                &candidate.provider,
                deadline,
            )
            .await
            .map_err(|error| {
                *failure_diagnostic = Some(album_evidence_access_diagnostic(
                    "access_rejected",
                    &error,
                    &audio,
                ));
                "Fresh evidence audio access check failed".to_owned()
            })
        } else {
            Ok(audio)
        }
    }

    /// Invalidate signed URLs and rejected extractor profiles after a provider
    /// account/session change. In-flight callers cannot adopt pre-change results.
    pub(crate) fn invalidate_playback_provider_session(&self, provider: &str) {
        if let Ok(stamp) = self.playback_provider_stamp(provider) {
            stamp.epoch.fetch_add(1, Ordering::SeqCst);
        }
        let is_profile = |key: &str| {
            serde_json::from_str::<Value>(key)
                .ok()
                .is_some_and(|value| {
                    value[0].as_str() == Some(provider)
                        || (value[0].as_str() == Some("recording-match-v1")
                            && value[2].as_str() == Some(provider))
                })
        };
        for cache in [
            &self.inner.preview_audio_cache,
            &self.inner.playback_metadata_cache,
        ] {
            cache.invalidate_matching(is_profile);
            if let Ok(mut cooldowns) = cache.cooldowns.lock() {
                cooldowns.retain(|key, _| !is_profile(key));
            }
        }
        let access_prefix = format!("{provider}\n");
        self.inner
            .playback_access_cache
            .invalidate_matching(|key| key.starts_with(&access_prefix));
        if let Ok(mut cooldowns) = self.inner.playback_access_cache.cooldowns.lock() {
            cooldowns.retain(|key, _| !key.starts_with(&access_prefix));
        }
    }

    /// Drop a previously validated stream URL after the playback engine reports a later
    /// transport failure. The next attempt will resolve a fresh signed URL for the same source.
    pub fn invalidate_preview_audio(&self, candidate: &Candidate, settings: &Settings) {
        let prefix = format!("{}\n{}\n", candidate.provider, candidate.url);
        self.inner
            .playback_access_cache
            .invalidate_matching(|key| key.starts_with(&prefix));
        for alternate_format in [false, true] {
            self.inner
                .playback_metadata_cache
                .invalidate(&preview_audio_cache_key(
                    candidate,
                    settings,
                    alternate_format,
                ));
            self.inner
                .preview_audio_cache
                .invalidate(&preview_audio_cache_key(
                    candidate,
                    settings,
                    alternate_format,
                ));
        }
    }
    pub async fn enqueue(
        &self,
        mut request: DownloadRequest,
        settings: Settings,
    ) -> Result<DownloadJob, String> {
        if !matches!(
            request.disposition,
            crate::models::ReplacementDisposition::Keep
        ) && request
            .chapter_ids
            .as_ref()
            .is_some_and(|ids| !ids.is_empty())
        {
            return Err("Replacing a file cannot be combined with chapter splitting".into());
        }
        if let Some(stored) = self.inner.store.get_track(&request.track.id)? {
            let before = serde_json::to_value(stored).map_err(|e| e.to_string())?;
            let requested = serde_json::to_value(&request.track).map_err(|e| e.to_string())?;
            if !request.track.provenance.is_object() {
                request.track.provenance = serde_json::json!({});
            }
            for field in [
                "artist",
                "title",
                "album",
                "albumArtist",
                "version",
                "genre",
                "label",
                "releaseDate",
                "recordingDate",
                "isrc",
                "catalogNumber",
                "bpm",
                "key",
            ] {
                if before[field] != requested[field] {
                    request.track.provenance["manualFields"][field] = serde_json::json!(true);
                }
            }
        }
        if request.candidate.provider == "youtube" {
            let is_catalog_identity = matches!(
                request.track.provider.as_str(),
                "catalog" | "musicbrainz" | "discogs" | "spotify" | "beatport" | "traxsource"
            ) || request.track.provenance["sourceKind"].as_str()
                == Some("catalog");
            if is_catalog_identity {
                let video_id = youtube_video_id(&request.candidate.url)
                    .ok_or("Selected YouTube candidate has no valid video identity")?;
                crate::metadata_normalization::normalize_catalog_live_download(
                    &mut request.track,
                    &request.candidate.title,
                    &request.candidate.url,
                    &video_id,
                    request.candidate.artwork_url.as_deref(),
                    request.candidate.duration_seconds,
                )?;
            } else {
                crate::metadata_normalization::normalize_selected_track(
                    &mut request.track,
                    &request.candidate.title,
                    &request.candidate.url,
                );
            }
        }
        if request.track.provenance["sourceKind"] == "audioSearch" {
            request.track.id = format!("{}:{}", request.track.provider, request.track.provider_id);
        }
        if request.track.id != format!("{}:{}", request.track.provider, request.track.provider_id) {
            return Err("Track identity must be a canonical provider identity".into());
        }
        if request.candidate.provider == "bandcamp" {
            return Err("Bandcamp public streams are available for playback only".into());
        }
        if let Some(ids) = request.chapter_ids.as_ref().filter(|ids| !ids.is_empty()) {
            let chapters = request
                .candidate
                .chapters
                .as_ref()
                .ok_or("Inspect source chapters before requesting a chapter download")?;
            if ids
                .iter()
                .any(|id| !chapters.iter().any(|chapter| &chapter.id == id))
                || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
            {
                return Err("Chapter selection is invalid; inspect the source again".into());
            }
        }
        if request.candidate.provider == "slskd" {
            let source = request
                .candidate
                .source
                .as_ref()
                .ok_or("Soulseek candidate is missing its typed source")?;
            let CandidateSource::Soulseek { filename, .. } = source;
            crate::slskd::validate_remote_path(filename)?;
            if self.inner.connections.slskd()?.is_none() {
                return Err("Enable and configure slskd in Connections first".into());
            }
        } else {
            validate_source_url(&request.candidate.url, &request.candidate.provider)?;
        }
        if request.candidate.permission == "restricted" {
            return Err("This source is restricted and cannot be acquired".into());
        }
        if settings.music_directory.trim().is_empty() {
            return Err("Configure a music directory before downloading".into());
        }
        if !["source", "mp3"].contains(&request.output_profile.as_str()) {
            return Err("Unknown output profile".into());
        }
        let id = Uuid::new_v4().to_string();
        if !request.track.provenance.is_object() {
            request.track.provenance = serde_json::json!({});
        }
        // A chooser selection is job-local authority. Bind it to the actual
        // selected source so a queued request cannot authorize another result.
        snapshot_user_selected_source(&mut request.track, &request.candidate);
        // The matching policy belongs to the queued request. A settings change
        // while a job is waiting must not silently change its identity policy.
        request.track.provenance["downloadMatchingLevel"] =
            serde_json::to_value(settings.matching_level).map_err(|error| error.to_string())?;
        let mut candidate = request.candidate;
        if let Some(CandidateSource::Soulseek {
            batch_id,
            search_id,
            transfer_id,
            ..
        }) = candidate.source.as_mut()
        {
            *batch_id = Some(id.clone());
            // Search sessions are cleaned up as soon as the result set is final;
            // the selected peer path can be downloaded independently afterwards.
            *search_id = None;
            *transfer_id = None;
        }
        let job = DownloadJob {
            id,
            track: request.track,
            candidate,
            state: "queued".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: Utc::now().to_rfc3339(),
            output_profile: request.output_profile,
            format_id: request.format_id,
            chapter_ids: request.chapter_ids,
            disposition: request.disposition,
            soulseek: None,
        };
        self.start(job, settings).await
    }
    async fn start(&self, job: DownloadJob, settings: Settings) -> Result<DownloadJob, String> {
        let (tx, rx) = idle_cancel();
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "Queue lock poisoned")?;
            if active.contains_key(&job.id) {
                return Err("Job is already active".into());
            }
            for other in self.inner.store.list_jobs()? {
                if other.id != job.id
                    && !["completed", "failed", "cancelled", "interrupted"]
                        .contains(&other.state.as_str())
                    && other.candidate.provider == job.candidate.provider
                    && if job.candidate.provider == "slskd" {
                        other.candidate.id == job.candidate.id
                    } else {
                        other.candidate.url == job.candidate.url
                    }
                {
                    return Err("This source already has an active acquisition job".into());
                }
            }
            self.update(&job)?;
            active.insert(job.id.clone(), tx);
        }
        let manager = self.clone();
        let work = job.clone();
        tokio::spawn(async move {
            manager.work(work, settings, rx).await;
        });
        Ok(job)
    }
    pub async fn cancel(&self, job_id: &str) -> Result<(), String> {
        let job = self.inner.store.get_job(job_id)?;
        if [
            "validating",
            "tagging",
            "publishing",
            "finalizing",
            "completed",
        ]
        .contains(&job.state.as_str())
        {
            return Err(
                "Publication has started; the completed file must be managed from the library"
                    .into(),
            );
        }
        self.inner
            .active
            .lock()
            .map_err(|_| "Queue lock poisoned")?
            .get(job_id)
            .ok_or("Job is not active")?
            .send(true)
            .map_err(|_| "Job has already stopped".to_string())
    }
    pub async fn retry(&self, job_id: &str, settings: Settings) -> Result<DownloadJob, String> {
        let mut job = self.inner.store.get_job(job_id)?;
        if !can_retry(&job.state) {
            return Err("Only failed, cancelled or interrupted jobs can be retried".into());
        }
        if self
            .inner
            .active
            .lock()
            .map_err(|_| "Queue lock poisoned")?
            .contains_key(job_id)
        {
            return Err("Job is already active".into());
        }
        if let Some(CandidateSource::Soulseek { search_id, .. }) = job.candidate.source.as_mut() {
            *search_id = None;
        }
        if job.candidate.provider == "slskd" {
            let connection = self
                .inner
                .connections
                .slskd()?
                .ok_or("slskd is disabled in Connections")?;
            let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
            if let Some(source) = job.candidate.source.as_mut() {
                client.prepare_retry(source).await?;
            }
        }
        job.soulseek = None;
        job.state = "queued".into();
        job.progress = 0.0;
        job.error = None;
        job.output_path = None;
        self.start(job, settings).await
    }
    async fn work(&self, mut job: DownloadJob, settings: Settings, rx: watch::Receiver<bool>) {
        let result = self
            .acquire_with_fallback(&mut job, &settings, rx.clone())
            .await
            .and_then(|asset| {
                self.inner
                    .store
                    .commit_download_asset(&asset, &job.disposition)
            });
        match result {
            Ok(mut asset) => {
                let _ = crate::directory_browser::stamp_registered_asset_fingerprint(
                    &self.inner.store,
                    &mut asset,
                );
                job.output_path = Some(asset.path.clone());
                match self.inner.store.save_asset(&asset) {
                    Ok(()) => {
                        let _ = self
                            .inner
                            .store
                            .set_preferred_track_file(&asset.track.id, &asset.id);
                        if let Err(e) = self.inner.store.add_asset_to_downloads(&asset) {
                            job.state = "failed".into();
                            job.error = Some(format!(
                                "File and library record were saved at {}, but Downloads membership failed: {e}",
                                asset.path
                            ));
                        } else {
                            self.autotag_download(&asset);
                            if let Some(publication_id) =
                                asset.track.provenance["publicationJobId"].as_str()
                            {
                                let _ = media::acknowledge_publication(&settings, publication_id);
                            }
                            job.state = "completed".into();
                            job.progress = 100.0;
                        }
                    }
                    Err(e) => {
                        job.state = "failed".into();
                        job.error = Some(format!(
                            "File published at {} but library record failed: {e}",
                            asset.path
                        ));
                    }
                }
            }
            Err(e) => {
                job.state = if *rx.borrow() || e == "Cancelled" {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                job.error = Some(e);
            }
        }
        let _ = self.update(&job);
        if let Ok(mut active) = self.inner.active.lock() {
            active.remove(&job.id);
        }
    }
    async fn acquire_with_fallback(
        &self,
        job: &mut DownloadJob,
        settings: &Settings,
        mut rx: watch::Receiver<bool>,
    ) -> Result<LocalAsset, String> {
        let original_track = job.track.clone();
        let original_provider = job.candidate.provider.clone();
        let lossless = crate::source_fallback::requires_lossless(job);
        let mut expected = original_track.clone();
        expected.duration_seconds = expected.duration_seconds.or(job.candidate.duration_seconds);
        let matching_level = download_matching_level(&original_track);
        let first_error = match self.acquire(job, settings, rx.clone()).await {
            Ok(asset) => return Ok(asset),
            Err(error) => error,
        };
        let lossless = lossless || crate::source_fallback::requires_lossless(job);
        if !crate::source_fallback::may_switch(job, &first_error, settings, *rx.borrow()) {
            return Err(first_error);
        }
        let providers = crate::source_fallback::provider_order(
            &original_provider,
            true,
            &settings.download_fallback_sources,
            true,
        );
        let mut assessment_context = crate::recording_match_context::AssessmentContext::default();
        let mut failures = vec![
            serde_json::json!({"provider":original_provider,"candidateId":job.candidate.id,"error":first_error}),
        ];
        for provider in providers {
            if *rx.borrow() {
                return Err("Cancelled".into());
            }
            let ids = vec![provider.clone()];
            let found = tokio::select! {
                found = self.find_candidates(&expected, &ids, settings) => found,
                _ = rx.changed() => return Err("Cancelled".into()),
            };
            let mut candidates = match found {
                Ok(found) => found,
                Err(error) => {
                    failures.push(serde_json::json!({"provider":provider,"error":error}));
                    continue;
                }
            };
            candidates.sort_by_key(|candidate| {
                crate::recording_match_context::priority(
                    &crate::recording_match_context::assess_with_context(
                        &self.inner.store,
                        &expected,
                        candidate,
                        matching_level,
                        crate::recording_match::MatchStage::Search,
                        &mut assessment_context,
                    ),
                )
            });
            let mut attempted = false;
            let candidates = candidates
                .into_iter()
                .filter_map(|candidate| {
                    if candidate.provider != provider {
                        return None;
                    }
                    let assessment = crate::recording_match_context::assess_with_context(
                        &self.inner.store,
                        &expected,
                        &candidate,
                        matching_level,
                        crate::recording_match::MatchStage::Search,
                        &mut assessment_context,
                    );
                    matches!(
                        assessment.action,
                        crate::recording_match::MatchAction::Inspect
                            | crate::recording_match::MatchAction::Accept
                    )
                    .then_some(candidate)
                })
                .take(2)
                .collect::<Vec<_>>();
            for candidate in candidates {
                let mut candidate = if provider == "slskd" {
                    candidate
                } else {
                    match inspect_with_cancel(&expected, &candidate, settings, rx.clone()).await {
                        Ok(inspected) => inspected,
                        Err(error) => {
                            if *rx.borrow() || error == "Cancelled" {
                                return Err("Cancelled".into());
                            }
                            failures.push(serde_json::json!({"provider":provider,"candidateId":candidate.id,"error":error}));
                            continue;
                        }
                    }
                };
                let assessment = crate::recording_match_context::assess_with_context(
                    &self.inner.store,
                    &expected,
                    &candidate,
                    matching_level,
                    crate::recording_match::MatchStage::Resolved,
                    &mut assessment_context,
                );
                if assessment.action != crate::recording_match::MatchAction::Accept {
                    failures.push(serde_json::json!({
                        "provider": provider,
                        "candidateId": candidate.id,
                        "error": "Resolved source did not meet the selected recording match policy",
                        "relationship": assessment.relationship,
                    }));
                    continue;
                }
                if candidate.permission == "restricted" {
                    continue;
                }
                let format_id = if lossless && !settings.download_fallback_allow_lossy {
                    let Some(format) = candidate.formats.iter().find(|f| f.lossless) else {
                        continue;
                    };
                    Some(format.id.clone())
                } else {
                    None
                };
                let staging = PathBuf::from(&settings.music_directory)
                    .join(".burrata-staging")
                    .join(&job.id);
                match tokio::fs::remove_dir_all(&staging).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("Cannot clear failed source staging: {error}"))
                    }
                }
                if let Some(CandidateSource::Soulseek {
                    batch_id,
                    search_id,
                    transfer_id,
                    ..
                }) = candidate.source.as_mut()
                {
                    *batch_id = Some(Uuid::new_v4().to_string());
                    *search_id = None;
                    *transfer_id = None;
                }
                candidate
                    .reasons
                    .push(format!("Automatic fallback from {original_provider}"));
                job.track = original_track.clone();
                if !job.track.provenance.is_object() {
                    job.track.provenance = serde_json::json!({});
                }
                job.track.provenance["downloadFallbackAttempts"] = serde_json::json!(failures);
                job.track.provenance["downloadAutomaticFallback"] = serde_json::json!(true);
                job.track.provenance["downloadFallbackRequiresLossless"] =
                    serde_json::json!(lossless && !settings.download_fallback_allow_lossy);
                job.candidate = candidate;
                job.format_id = format_id;
                job.soulseek = None;
                job.state = "queued".into();
                job.progress = 0.0;
                job.error = None;
                self.update(job)?;
                attempted = true;
                match self.acquire(job, settings, rx.clone()).await {
                    Ok(asset) => {
                        job.error = None;
                        return Ok(asset);
                    }
                    Err(error) => {
                        if !crate::source_fallback::may_switch(job, &error, settings, *rx.borrow())
                        {
                            return Err(error);
                        }
                        failures.push(serde_json::json!({"provider":provider,"candidateId":job.candidate.id,"error":error}));
                    }
                }
            }
            if !attempted {
                failures.push(serde_json::json!({"provider":provider,"error":"No permitted recording match meeting the fallback quality policy"}));
            }
        }
        if !job.track.provenance.is_object() {
            job.track.provenance = serde_json::json!({});
        }
        job.track.provenance["downloadFallbackAttempts"] = serde_json::json!(failures);
        Err(format!("{first_error}; configured download fallbacks exhausted (no successful recording/quality match)"))
    }

    async fn acquire(
        &self,
        job: &mut DownloadJob,
        settings: &Settings,
        mut rx: watch::Receiver<bool>,
    ) -> Result<LocalAsset, String> {
        if *rx.borrow() {
            return Err("Cancelled".into());
        }
        let mut admission = download_admission(
            &job.candidate.provider,
            &self.inner.slots,
            &self.inner.soulseek_slots,
            &mut rx,
        )
        .await?;
        if let Some(existing) = self
            .inner
            .store
            .list_assets()?
            .into_iter()
            .find(|a| Path::new(&a.path).is_file() && is_preserved_source(a, &job.candidate))
        {
            admission
                .begin_processing(&self.inner.slots, &mut rx)
                .await?;
            media::verify_asset(&existing).await?;
            if existing.recording_id != self.inner.store.recording_id(&job.track.id)? {
                if !matches!(job.disposition, crate::models::ReplacementDisposition::Keep) {
                    return Err(
                        "This source is already saved for another catalog entry; choose Keep to create a separate verified copy".into(),
                    );
                }
                let original_id = job.candidate.id.clone();
                let mut candidate = if job.candidate.provider == "slskd" {
                    job.candidate.clone()
                } else {
                    let mut inspected =
                        inspect_metadata_with_cancel(&job.candidate, settings, rx.clone()).await?;
                    matching::score_candidate(&job.track, &mut inspected);
                    inspected
                };
                if *rx.borrow() {
                    return Err("Cancelled".into());
                }
                if candidate.id != original_id
                    || candidate.permission == "restricted"
                    || !is_preserved_source(&existing, &candidate)
                {
                    return Err(
                        "The existing file no longer matches the exact selected source locator"
                            .into(),
                    );
                }
                let assessment = preserved_source_reuse_assessment(
                    &self.inner.store,
                    &job.track,
                    &existing,
                    &candidate,
                    download_matching_level(&job.track),
                )
                .ok_or_else(|| {
                    "This source is saved for another catalog entry and did not pass the identifier-based reuse policy".to_owned()
                })?;
                if job.track.provenance["downloadFallbackRequiresLossless"] == true {
                    media::verify_lossless_fallback(Path::new(&existing.path), settings).await?;
                }
                if *rx.borrow() {
                    return Err("Cancelled".into());
                }
                let artifact_track = preserved_source_reuse_track(
                    &job.track,
                    &existing,
                    &candidate,
                    &assessment,
                    job.format_id.as_deref(),
                )?;
                job.candidate = candidate.clone();
                {
                    let mut active = self
                        .inner
                        .active
                        .lock()
                        .map_err(|_| "Queue lock poisoned")?;
                    if *rx.borrow() {
                        return Err("Cancelled".into());
                    }
                    active.remove(&job.id);
                    job.state = "validating".into();
                    job.progress = 92.0;
                    self.update(job)?;
                }
                let mut source = media::finalize_download(
                    Path::new(&existing.path),
                    &artifact_track,
                    &candidate,
                    settings,
                    "source",
                    &job.id,
                )
                .await?;
                let _ = crate::directory_browser::stamp_registered_asset_fingerprint(
                    &self.inner.store,
                    &mut source,
                );
                self.inner.store.save_asset(&source).map_err(|error| {
                    format!(
                        "Verified copy was published at {} but library record failed: {error}",
                        source.path
                    )
                })?;
                self.inner
                    .store
                    .add_asset_to_downloads(&source)
                    .map_err(|error| {
                        format!(
                            "Verified copy was saved at {}, but Downloads membership failed: {error}",
                            source.path
                        )
                    })?;
                job.output_path = Some(source.path.clone());
                job.track = source.track.clone();
                media::acknowledge_publication(settings, &job.id)?;
                if job.output_profile == "mp3" {
                    job.state = "publishing".into();
                    job.progress = 96.0;
                    self.update(job)?;
                    return media::export_mp3(&source, settings).await.map_err(|error| {
                        format!(
                            "Verified source copy retained at {}; MP3 export failed: {error}",
                            source.path
                        )
                    });
                }
                return Ok(source);
            }
            if job.track.provenance["downloadFallbackRequiresLossless"] == true {
                media::verify_lossless_fallback(Path::new(&existing.path), settings).await?;
            }
            self.inner.store.add_asset_to_downloads(&existing)?;
            job.output_path = Some(existing.path.clone());
            {
                let mut active = self
                    .inner
                    .active
                    .lock()
                    .map_err(|_| "Queue lock poisoned")?;
                if *rx.borrow() {
                    return Err("Cancelled".into());
                }
                active.remove(&job.id);
                job.state = "publishing".into();
                self.update(job)?;
            }
            return if job.output_profile == "mp3" {
                media::export_mp3(&existing, settings).await
            } else {
                Ok(existing)
            };
        }
        job.state = "downloading".into();
        self.update(job)?;
        if job.candidate.provider == "bandcamp" {
            return Err("Bandcamp public streams are available for playback only".into());
        }
        // Re-extract formats immediately before acquisition; never reuse expired stream URLs.
        if job.candidate.provider != "slskd" {
            // Acquisition must verify against current provider identity evidence.
            // The chooser's short-lived inspection cache is useful for UI, but
            // it is not the final automatic-download gate.
            let mut inspected =
                inspect_metadata_with_cancel(&job.candidate, settings, rx.clone()).await?;
            matching::score_candidate(&job.track, &mut inspected);
            job.candidate = inspected;
        }
        let selected_assessment = crate::recording_match_context::assess(
            &self.inner.store,
            &job.track,
            &job.candidate,
            download_matching_level(&job.track),
            crate::recording_match::MatchStage::Resolved,
        );
        let explicitly_selected = explicit_download_source(&job.track, &job.candidate);
        if selected_assessment.action != crate::recording_match::MatchAction::Accept
            && !explicitly_selected
        {
            return Err(
                "The selected source did not meet the automatic recording match policy".into(),
            );
        }
        if explicitly_selected
            && selected_assessment.action != crate::recording_match::MatchAction::Accept
        {
            if !job.track.provenance.is_object() {
                job.track.provenance = serde_json::json!({});
            }
            job.track.provenance["actualSourceVerification"] =
                serde_json::json!("unverified_user_selected_source");
            job.track.provenance["actualSourceMatchReasons"] =
                serde_json::to_value(&selected_assessment.reasons)
                    .unwrap_or_else(|_| serde_json::json!([]));
        }
        if *rx.borrow() {
            return Err("Cancelled".into());
        }
        if job.candidate.permission == "restricted" {
            return Err("Source now reports restricted access".into());
        }
        let mut provenance = job
            .track
            .provenance
            .as_object()
            .cloned()
            .unwrap_or_default();
        provenance.insert(
            "acquisition".into(),
            serde_json::json!({
                "provider": job.candidate.provider, "url": job.candidate.url,
                "id": job.candidate.id, "permission": job.candidate.permission,
                "formatId": job.format_id,
                "source": job.candidate.source,
            }),
        );
        job.track.provenance = Value::Object(provenance);
        let staging = PathBuf::from(&settings.music_directory)
            .join(".burrata-staging")
            .join(&job.id);
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| format!("Cannot create staging directory: {e}"))?;
        let chapter_source_key = if job.chapter_ids.as_ref().is_some_and(|ids| !ids.is_empty())
            && job.candidate.provider != "slskd"
        {
            Some(
                serde_json::json!([
                    "chapter-source-v1",
                    job.candidate.provider,
                    job.candidate.url,
                    job.format_id.as_deref().unwrap_or("bestaudio"),
                    settings.music_directory,
                ])
                .to_string(),
            )
        } else {
            None
        };
        let _chapter_source_gate = if let Some(key) = chapter_source_key.as_deref() {
            Some(self.inner.chapter_source_cache.gate(key).await?)
        } else {
            None
        };
        let cached_chapter_source = if let Some(key) = chapter_source_key.as_deref() {
            self.inner
                .chapter_source_cache
                .copy_to_staging(key, &staging)
                .await
        } else {
            None
        };
        let used_cached_chapter_source = cached_chapter_source.is_some();
        let path = if let Some(cached) = cached_chapter_source {
            cached
        } else if job.candidate.provider == "slskd" {
            let connection = self
                .inner
                .connections
                .slskd()?
                .ok_or("slskd is disabled in Connections")?;
            let source = job
                .candidate
                .source
                .clone()
                .ok_or("Soulseek candidate is missing its typed source")?;
            let (username, batch_id) = match &source {
                CandidateSource::Soulseek {
                    username, batch_id, ..
                } => (
                    username.clone(),
                    batch_id.clone().unwrap_or_else(|| job.id.clone()),
                ),
            };
            let client = crate::slskd::SlskdClient::new(&connection.base_url, &connection.api_key)?;
            crate::slskd::wait_for_batch_file(
                &client,
                &batch_id,
                &username,
                &source,
                Path::new(&connection.downloads_path),
                &staging,
                &mut rx,
                |progress, transfer_id, transfer_state, details| {
                    job.soulseek = Some(details.clone());
                    if let Some(transfer_id) = transfer_id {
                        if let Some(CandidateSource::Soulseek {
                            transfer_id: stored,
                            ..
                        }) = job.candidate.source.as_mut()
                        {
                            if stored.as_deref() != Some(transfer_id.as_str()) {
                                *stored = Some(transfer_id);
                            }
                        }
                    }
                    if ["waiting_for_peer", "queued", "stalled"].contains(&transfer_state) {
                        job.state = transfer_state.to_owned();
                    } else {
                        job.state = "downloading".into();
                    }
                    job.progress = progress;
                    let _ = self.update(job);
                },
            )
            .await?
        } else if matches!(job.candidate.provider.as_str(), "direct" | "monochrome") {
            download_direct(&job.candidate.url, &staging, rx.clone()).await?
        } else {
            if job
                .format_id
                .as_ref()
                .is_some_and(|id| !job.candidate.formats.iter().any(|format| &format.id == id))
            {
                return Err(
                    "Selected audio format is no longer available; inspect and choose again"
                        .into(),
                );
            }
            let same_candidate = job.candidate.clone();
            let selected_format_id = job.format_id.clone();
            let (raw, resolved_format_id) =
                crate::youtube_download_recovery::download_with_403_recovery(
                    &same_candidate,
                    selected_format_id.as_deref(),
                    job,
                    rx.clone(),
                    |cancel| {
                        let candidate = same_candidate.clone();
                        Box::pin(async move {
                            inspect_metadata_with_cancel(&candidate, settings, cancel).await
                        })
                    },
                    || {
                        let staging = staging.clone();
                        Box::pin(async move {
                            match tokio::fs::remove_dir_all(&staging).await {
                                Ok(()) => {}
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => {
                                    return Err(format!(
                                        "Cannot clear failed-format staging: {error}"
                                    ))
                                }
                            }
                            tokio::fs::create_dir_all(&staging).await.map_err(|error| {
                                format!("Cannot recreate failed-format staging: {error}")
                            })
                        })
                    },
                    |attempt_job, format_id, cancel| {
                        let staging = staging.clone();
                        let manager = self.clone();
                        let settings = settings.clone();
                        Box::pin(async move {
                            let format = format_id.unwrap_or_else(|| "bestaudio".into());
                            let provider = attempt_job.candidate.provider.clone();
                            let candidate_url = attempt_job.candidate.url.clone();
                            let mut args = base_args(&settings);
                            let (_, mut provider_invocation) =
                                add_youtube_support_args(&mut args, &provider, &settings, None)
                                    .await?;
                            args.extend([
                                "--no-playlist".into(),
                                "--no-simulate".into(),
                                "--newline".into(),
                                "--progress".into(),
                                "--progress-delta".into(),
                                "0.5".into(),
                                "--progress-template".into(),
                                "download:BURRATA_PROGRESS %(progress)j".into(),
                                "--print".into(),
                                "after_move:BURRATA_FILE %(filepath)j".into(),
                                "--retries".into(),
                                "3".into(),
                                "--fragment-retries".into(),
                                "3".into(),
                                "--socket-timeout".into(),
                                "20".into(),
                                "-f".into(),
                                format,
                                "-x".into(),
                                "--audio-format".into(),
                                "best".into(),
                                "-o".into(),
                                staging
                                    .join("source.%(ext)s")
                                    .to_string_lossy()
                                    .into_owned(),
                                "--".into(),
                                candidate_url,
                            ]);
                            let ffmpeg = crate::tools::executable(&settings.ffmpeg_path, "ffmpeg");
                            if let Some(location) = ffmpeg_location(&ffmpeg.to_string_lossy()) {
                                let position = args.len() - 2;
                                args.splice(
                                    position..position,
                                    ["--ffmpeg-location".into(), location],
                                );
                            }
                            let raw = run_tool(
                                &settings,
                                args,
                                cancel,
                                Duration::from_secs(3600),
                                |line| {
                                    if let Some(raw) = line.strip_prefix("BURRATA_PROGRESS ") {
                                        if let Ok(progress) = serde_json::from_str::<Value>(raw) {
                                            let total =
                                                progress["total_bytes"].as_f64().or_else(|| {
                                                    progress["total_bytes_estimate"].as_f64()
                                                });
                                            if let (Some(n), Some(d)) =
                                                (progress["downloaded_bytes"].as_f64(), total)
                                            {
                                                if d > 0.0 {
                                                    attempt_job.progress =
                                                        (n / d * 90.0).clamp(0.0, 90.0);
                                                    let _ = manager.update(attempt_job);
                                                }
                                            }
                                        }
                                    }
                                },
                            )
                            .await;
                            if let Some(invocation) = provider_invocation.as_mut() {
                                invocation.complete();
                            }
                            raw
                        })
                    },
                )
                .await?;
            if let Some(format_id) = resolved_format_id {
                job.track.provenance["acquisition"]["resolvedFormatId"] =
                    serde_json::json!(format_id);
            }
            let filename = raw
                .lines()
                .filter_map(|line| line.strip_prefix("BURRATA_FILE "))
                .filter_map(|s| serde_json::from_str::<String>(s).ok())
                .last()
                .ok_or("Downloader did not report a completed audio file")?;
            let path = PathBuf::from(filename);
            let real = tokio::fs::canonicalize(&path)
                .await
                .map_err(|e| e.to_string())?;
            let root = tokio::fs::canonicalize(&staging)
                .await
                .map_err(|e| e.to_string())?;
            if !real.starts_with(root) {
                return Err("Downloader returned a file outside its staging directory".into());
            }
            real
        };
        if job.track.provenance["downloadFallbackRequiresLossless"] == true {
            media::verify_lossless_fallback(&path, settings).await?;
        }
        job.state = "validating".into();
        self.update(job)?;
        admission
            .begin_processing(&self.inner.slots, &mut rx)
            .await?;
        if !used_cached_chapter_source {
            if let Some(key) = chapter_source_key.as_deref() {
                self.inner
                    .chapter_source_cache
                    .insert_from_staging(key, &path, &staging, Path::new(&settings.music_directory))
                    .await;
            }
        }
        drop(_chapter_source_gate);
        let selected_chapters = if let Some(ids) =
            job.chapter_ids.clone().filter(|ids| !ids.is_empty())
        {
            let chapters = job
                .candidate
                .chapters
                .clone()
                .ok_or("Selected source has no inspected chapter metadata")?;
            let assets = self.inner.store.list_assets()?;
            let mut selected = Vec::new();
            for chapter_id in ids {
                let chapter = chapters
                    .iter()
                    .find(|chapter| chapter.id == chapter_id)
                    .cloned()
                    .ok_or("Selected chapter is no longer available; inspect the source again")?;
                let existing = assets
                    .iter()
                    .find(|asset| {
                        asset.track.provenance["chapter"]["jobId"] == job.id
                            && asset.track.provenance["chapter"]["id"] == chapter.id
                            && Path::new(&asset.path).is_file()
                            && if job.output_profile == "mp3" {
                                asset.track.provenance.get("derivedFromAssetId").is_some()
                            } else {
                                asset.track.provenance.get("derivedFromAssetId").is_none()
                            }
                    })
                    .cloned();
                let segment = if existing.is_none() {
                    if *rx.borrow() {
                        return Err("Cancelled".into());
                    }
                    Some(
                        media::split_chapter(&path, &chapter, settings, &job.id, selected.len())
                            .await?,
                    )
                } else {
                    None
                };
                selected.push((chapter, segment, existing));
            }
            Some(selected)
        } else {
            None
        };
        // Make the cancellation boundary atomic with cancel(): after publication starts,
        // removing the signal prevents a racing request from reporting false cancellation.
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "Queue lock poisoned")?;
            if *rx.borrow() {
                return Err("Cancelled".into());
            }
            active.remove(&job.id);
            job.state = "validating".into();
            job.progress = 92.0;
            self.update(job)?;
        }
        if let Some(selected) = selected_chapters {
            let mut assets = Vec::new();
            for (position, (chapter, segment, existing)) in selected.into_iter().enumerate() {
                if let Some(existing) = existing {
                    self.inner.store.add_asset_to_downloads(&existing)?;
                    if position == 0 {
                        job.output_path = Some(existing.path.clone());
                    }
                    assets.push(existing);
                    continue;
                }
                let segment = segment.ok_or("Chapter segment staging failed")?;
                let mut chapter_track = job.track.clone();
                let parent_title = chapter_track.title.clone();
                let source_label = chapter.title.clone();
                let (label_artist, label_title, _) = chapter_artist_title(&source_label);
                let catalog_suggestion =
                    verified_chapter_suggestion(&chapter, job.track.version.as_deref());
                let artist_source = if catalog_suggestion.is_some() {
                    "exact_unique_catalog_match"
                } else if !label_artist.is_empty() {
                    "chapter_label"
                } else {
                    "unknown"
                };
                let (chapter_track_id, chapter_provider_id) =
                    chapter_track_identity(&job.track.id, &chapter.id);
                chapter_track.id = chapter_track_id;
                chapter_track.provider = "chapter".into();
                chapter_track.provider_id = chapter_provider_id;
                chapter_track.source_url = job.candidate.url.clone();
                chapter_track.artist = catalog_suggestion
                    .map(|track| track.artist.clone())
                    .unwrap_or(label_artist);
                chapter_track.title = catalog_suggestion
                    .map(|track| track.title.clone())
                    .unwrap_or(label_title);
                chapter_track.version = None;
                chapter_track.album = Some(parent_title.clone());
                chapter_track.label = None;
                chapter_track.genre = catalog_suggestion.and_then(|track| track.genre.clone());
                chapter_track.normalized_genre = None;
                chapter_track.release_date =
                    catalog_suggestion.and_then(|track| track.release_date.clone());
                chapter_track.catalog_number = None;
                chapter_track.duration_seconds = Some(chapter.end_seconds - chapter.start_seconds);
                chapter_track.isrc = None;
                chapter_track.bpm = None;
                chapter_track.key = None;
                if let Some(provenance) = chapter_track.provenance.as_object_mut() {
                    provenance.remove("userSelectedSource");
                    provenance.remove("downloadAutomaticFallback");
                }
                chapter_track.provenance["chapter"] = serde_json::json!({
                        "id": chapter.id,
                        "jobId": job.id,
                        "parentTrackId": job.track.id,
                        "parentTitle": parent_title,
                        "startSeconds": chapter.start_seconds,
                        "endSeconds": chapter.end_seconds,
                        "label": source_label,
                        "confidence": chapter.confidence,
                        "identity": "unverified chapter-derived recording",
                        "artistSource": artist_source,
                "weakFields": {"artist": artist_source == "chapter_label", "title": true},
                        "artistIdentity": "excerpt identity remains distinct from any full recording match",
                        "catalogSuggestion": catalog_suggestion.map(|track| serde_json::json!({
                            "provider": track.provider,
                            "recordingId": track.provider_id,
                            "confidence": "exact_unique_catalog_match"
                        }))
                    });
                let mut chapter_candidate = job.candidate.clone();
                chapter_candidate.duration_seconds = chapter_track.duration_seconds;
                let publication_id = format!("{}-chapter-{}", job.id, position);
                let mut asset = media::finalize_download(
                    &segment,
                    &chapter_track,
                    &chapter_candidate,
                    settings,
                    "source",
                    &publication_id,
                )
                .await?;
                let _ = crate::directory_browser::stamp_registered_asset_fingerprint(
                    &self.inner.store,
                    &mut asset,
                );
                self.inner.store.save_asset(&asset).map_err(|e| {
                    format!(
                        "Chapter published at {} but library record failed: {e}",
                        asset.path
                    )
                })?;
                self.inner
                    .store
                    .add_asset_to_downloads(&asset)
                    .map_err(|e| {
                        format!(
                            "Chapter was saved at {}, but Downloads membership failed: {e}",
                            asset.path
                        )
                    })?;
                media::acknowledge_publication(settings, &publication_id)?;
                let result_asset = if job.output_profile == "mp3" {
                    let mut exported = media::export_mp3(&asset, settings).await?;
                    let _ = crate::directory_browser::stamp_registered_asset_fingerprint(
                        &self.inner.store,
                        &mut exported,
                    );
                    self.inner.store.save_asset(&exported).map_err(|e| {
                        format!(
                            "MP3 published at {} but library record failed: {e}",
                            exported.path
                        )
                    })?;
                    self.inner
                        .store
                        .add_asset_to_downloads(&exported)
                        .map_err(|e| {
                            format!(
                                "MP3 was saved at {}, but Downloads membership failed: {e}",
                                exported.path
                            )
                        })?;
                    exported
                } else {
                    asset
                };
                if position == 0 {
                    job.output_path = Some(result_asset.path.clone());
                }
                self.autotag_download(&result_asset);
                assets.push(result_asset);
                job.progress = 92.0
                    + (position + 1) as f64 / job.chapter_ids.as_ref().unwrap().len() as f64 * 8.0;
                let _ = self.update(job);
            }
            if let Ok(analysis) = self.inner.analysis.lock() {
                if let Some(analysis) = analysis.as_ref() {
                    let asset_ids = assets.iter().map(|asset| asset.id.clone()).collect();
                    let _ = analysis.queue(Some(asset_ids), false);
                }
            }
            return assets
                .into_iter()
                .next()
                .ok_or_else(|| "No chapters selected".into());
        }
        let separate_source_identity = selected_assessment.relationship
            == crate::recording_match::MatchRelationship::Alternative
            || (explicitly_selected
                && selected_assessment.action != crate::recording_match::MatchAction::Accept);
        let artifact_track = artifact_track_for_match(
            &job.track,
            &job.candidate,
            selected_assessment.relationship,
            separate_source_identity,
        )?;
        let mut source = media::finalize_download(
            &path,
            &artifact_track,
            &job.candidate,
            settings,
            "source",
            &job.id,
        )
        .await?;
        if matches!(job.disposition, crate::models::ReplacementDisposition::Keep) {
            let _ = crate::directory_browser::stamp_registered_asset_fingerprint(
                &self.inner.store,
                &mut source,
            );
            self.inner.store.save_asset(&source).map_err(|e| {
                format!(
                    "File published at {} but source library record failed: {e}",
                    source.path
                )
            })?;
            self.inner
                .store
                .add_asset_to_downloads(&source)
                .map_err(|e| {
                    format!(
                        "File was saved at {}, but Downloads membership failed: {e}",
                        source.path
                    )
                })?;
        }
        job.output_path = Some(source.path.clone());
        job.track = source.track.clone();
        media::acknowledge_publication(settings, &job.id)?;
        if job.output_profile == "mp3" {
            job.state = "publishing".into();
            job.progress = 96.0;
            self.update(job)?;
            media::export_mp3(&source, settings)
                .await
                .map_err(|e| format!("Source retained at {}; MP3 export failed: {e}", source.path))
        } else {
            Ok(source)
        }
    }
}

fn cached_download_search_alternate(store: &Store, track: &Track) -> Option<String> {
    crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
        .search_artist_candidates()
        .into_iter()
        .next()
}

fn soulseek_album_search_query(
    track: &Track,
    album: &str,
    verified_artist_aliases: &[String],
) -> Result<String, String> {
    let album_key = matching::normalize(album);
    if verified_artist_aliases
        .iter()
        .map(|alias| alias.trim())
        .any(|alias| !alias.is_empty() && matching::normalize(alias) == album_key)
    {
        return Ok(album.trim().to_owned());
    }
    let artist = verified_artist_aliases
        .iter()
        .map(|alias| alias.trim())
        .find(|alias| !alias.is_empty())
        .or_else(|| {
            track
                .album_artist
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(track.artist.trim());
    if artist.is_empty() {
        return Err("Album Soulseek search requires an artist credit".into());
    }
    let query = format!("{artist} {}", album.trim());
    if query.chars().count() > 500 {
        return Err("Album Soulseek search query is too long".into());
    }
    Ok(query)
}

fn soulseek_album_folder_matches(folder_album: &str, track: &Track) -> bool {
    let Some(album) = track.album.as_deref() else {
        return false;
    };
    let folder_key = matching::normalize(folder_album);
    let album_key = matching::normalize(album);
    // Providers often append volume, edition, or year labels to a folder.
    // Preserve them; shared per-track version/runtime checks decide whether
    // the files actually fit this requested release.
    folder_key == album_key || folder_key.starts_with(&album_key)
}

fn minimum_soulseek_album_coverage(track_count: usize) -> usize {
    track_count.min(2).max(1)
}

fn soulseek_expected_album_ordinals(track: &Track) -> (Option<u32>, Option<u32>) {
    let (verified_track, verified_disc) =
        crate::recording_identity::trusted_musicbrainz_ordinals(track);
    (
        track.track_number.or(verified_track),
        track.disc_number.or(verified_disc),
    )
}

async fn cached_download_candidate_search<F, Fut>(
    cache: &CandidateSearchCache,
    key: String,
    cancel: watch::Receiver<bool>,
    load: F,
) -> Result<Vec<Candidate>, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(Vec<Candidate>, bool), String>>,
{
    cache
        .get_or_search_cancelled_observed_decided(key, cancel, None, load)
        .await
}

fn slskd_availability_order(candidate: &Candidate) -> (u8, u8, u32, std::cmp::Reverse<u64>) {
    let free_slot = candidate
        .reasons
        .iter()
        .any(|r| r == "availability:free-upload-slot");
    let queued = candidate
        .reasons
        .iter()
        .any(|r| r == "availability:queued-remotely");
    let rank = if free_slot {
        0
    } else if queued {
        2
    } else {
        1
    };
    let queue = candidate
        .reasons
        .iter()
        .find_map(|reason| {
            reason
                .strip_prefix("availability:queue-length:")?
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(u32::MAX);
    // A free-slot hint must not hide a queue of thousands of files.
    let queue_band = match queue {
        0..=100 => 0,
        101..=1000 => 1,
        1001..=u32::MAX => 2,
    };
    let speed = candidate
        .reasons
        .iter()
        .find_map(|reason| {
            reason
                .strip_prefix("availability:upload-speed:")?
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0);
    (queue_band, rank, queue, std::cmp::Reverse(speed))
}

fn slskd_peer_failure_order(candidate: &Candidate, history: &[DownloadJob]) -> (bool, usize) {
    let Some(CandidateSource::Soulseek { username, .. }) = candidate.source.as_ref() else {
        return (false, 0);
    };
    let mut restricted = false;
    let mut failures = 0;
    // Only the most recent few outcomes affect ranking, so old failures do not
    // permanently blacklist a peer. Application shutdown is not a peer failure.
    for job in history.iter().filter(|job| {
        job.candidate.provider == "slskd" && matches!(job.candidate.source.as_ref(), Some(CandidateSource::Soulseek { username: peer, .. }) if peer == username)
    }).take(5) {
        let error = job.error.as_deref().unwrap_or_default().to_ascii_lowercase();
        if job.state == "failed" && !error.contains("application shut down") {
            failures += 1;
            restricted |= error.contains("banned") || error.contains("sharing") || error.contains("rejected");
        }
    }

    (restricted, failures)
}

fn slskd_candidate_track(candidate: &Candidate) -> Track {
    let provider_id = candidate
        .id
        .strip_prefix("slskd:")
        .unwrap_or(&candidate.id)
        .to_owned();
    let filename_observations = candidate
        .source_metadata
        .as_ref()
        .filter(|metadata| metadata["sourceMetadataOrigin"] == "slskdFilename")
        .map(|metadata| &metadata["filenameObservations"]);
    let display_title = filename_observations
        .and_then(|observations| observations["normalizedTitle"].as_str())
        .unwrap_or(&candidate.title);
    let (artist, title) = split_remote_title(display_title);
    let mut track = Track {
        id: format!("slskd:{provider_id}"),
        provider: "slskd".into(),
        provider_id,
        source_url: String::new(),
        artist,
        title,
        duration_seconds: candidate.duration_seconds,
        ..Default::default()
    };
    track.provenance = serde_json::json!({
        "identity": "unverified peer filename",
        "artistSource": if track.artist.is_empty() { "unknown" } else { "filename" },
        "uploader": candidate.uploader,
        "audioCandidate": candidate,
        "sourceProvider": "slskd",
    });
    track
}

struct EvaluatedSoulseekAlbumGroup {
    peer: String,
    directory: String,
    assignments: Vec<(usize, Candidate)>,
    lossless_count: usize,
    bitrate_total: u64,
    availability: (u8, u8, u32, std::cmp::Reverse<u64>),
}

#[derive(Default)]
struct SoulseekAlbumSelection {
    assignments: Vec<SoulseekAlbumAssignment>,
    groups_total: usize,
    groups_examined: usize,
    candidate_pairs_total: usize,
    candidate_pairs_after_ordinal: usize,
    candidate_pairs_after_title: usize,
    shared_assessments: usize,
    assessment_rejections: HashMap<String, usize>,
    best_coverage_at_stop: usize,
    stopped_on_deadline: bool,
}

struct SoulseekAlbumCandidateGroup {
    peer: String,
    directory: String,
    files: Vec<Candidate>,
    potential_lossless_count: usize,
    potential_bitrate_total: u64,
    folder_artist_file_count: usize,
    best_availability: (u8, u8, u32, std::cmp::Reverse<u64>),
    exact_title_pair_count: usize,
    ordinal_pair_count: usize,
    exact_title_track_count: usize,
    ordinal_track_count: usize,
}

#[derive(Default)]
struct SoulseekAlbumSearchDiagnostics {
    complete: bool,
    poll_count: usize,
    response_rows: usize,
    unique_candidates: usize,
    final_state: String,
    warning_count: usize,
}

fn compare_soulseek_album_candidate_groups(
    left: &SoulseekAlbumCandidateGroup,
    right: &SoulseekAlbumCandidateGroup,
) -> std::cmp::Ordering {
    right
        .exact_title_track_count
        .cmp(&left.exact_title_track_count)
        .then_with(|| right.ordinal_track_count.cmp(&left.ordinal_track_count))
        .then_with(|| {
            right
                .exact_title_pair_count
                .cmp(&left.exact_title_pair_count)
        })
        .then_with(|| right.ordinal_pair_count.cmp(&left.ordinal_pair_count))
        .then_with(|| {
            right
                .potential_lossless_count
                .cmp(&left.potential_lossless_count)
        })
        .then_with(|| {
            right
                .potential_bitrate_total
                .cmp(&left.potential_bitrate_total)
        })
        .then_with(|| left.best_availability.cmp(&right.best_availability))
        .then_with(|| left.peer.cmp(&right.peer))
        .then_with(|| left.directory.cmp(&right.directory))
}

fn soulseek_album_format_rank(candidate: &Candidate) -> (bool, u64, u32, u32, u64) {
    let format = candidate.formats.first();
    (
        format.is_some_and(|format| format.lossless),
        format
            .and_then(|format| format.bitrate)
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| (value as u64).min(1_000_000))
            .unwrap_or(0),
        format.and_then(|format| format.sample_rate).unwrap_or(0),
        format.and_then(|format| format.bit_depth).unwrap_or(0),
        format.and_then(|format| format.size_bytes).unwrap_or(0),
    )
}

fn ordered_soulseek_album_pair_indices(
    ordinal_matches: &[usize],
    exact_title_matches: &[usize],
) -> Vec<usize> {
    let mut ordered = Vec::with_capacity(ordinal_matches.len());
    for index in exact_title_matches.iter().chain(ordinal_matches) {
        if ordinal_matches.contains(index) && !ordered.contains(index) {
            ordered.push(*index);
        }
    }
    ordered
}

fn unique_soulseek_album_track_match(matches: &[usize]) -> Option<usize> {
    match matches {
        [index] => Some(*index),
        _ => None,
    }
}

fn soulseek_album_assessment_rejections(
    assessment: &crate::recording_match::MatchAssessment,
    track: &Track,
    candidate: &Candidate,
) -> Vec<&'static str> {
    use crate::recording_match::{Comparison, MatchAction, MatchRelationship};

    let mut reasons = Vec::new();
    if assessment.action != MatchAction::Accept {
        reasons.push("action_not_accept");
    }
    if !matches!(
        assessment.relationship,
        MatchRelationship::Exact | MatchRelationship::Probable
    ) {
        reasons.push("relationship_not_exact_or_probable");
    }
    if assessment.dimensions.title != Comparison::Agree {
        reasons.push("title_not_agree");
    }
    if assessment.dimensions.artists != Comparison::Agree {
        reasons.push("artists_not_agree");
    }
    if assessment.dimensions.version == Comparison::Conflict {
        reasons.push("version_conflict");
    }
    if assessment.dimensions.runtime == Comparison::Conflict {
        reasons.push("runtime_conflict");
    }
    if track
        .version
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && assessment.dimensions.version != Comparison::Agree
    {
        reasons.push("requested_version_not_agree");
    }
    if track.duration_seconds.is_some()
        && candidate.duration_seconds.is_some()
        && assessment.dimensions.runtime != Comparison::Agree
    {
        reasons.push("requested_runtime_not_agree");
    }
    if assessment
        .evidence
        .as_ref()
        .is_none_or(|evidence| evidence.parsing_incomplete)
    {
        reasons.push("evidence_parsing_incomplete_or_missing");
    }
    if assessment
        .evidence
        .as_ref()
        .is_none_or(|evidence| evidence.conflicting_version_observations)
    {
        reasons.push("evidence_conflicting_version_or_missing");
    }
    reasons
}

fn select_soulseek_album_group(
    store: &Store,
    tracks: &[Track],
    aliases: &[crate::artist_credit_aliases::VerifiedPlaybackArtistAliases],
    candidates: Vec<Candidate>,
    matching_level: crate::recording_match::MatchLevel,
    deadline: Instant,
    cancel: &watch::Receiver<bool>,
) -> SoulseekAlbumSelection {
    use crate::recording_match::MatchStage;

    let mut selection = SoulseekAlbumSelection::default();
    if aliases.len() != tracks.len() || *cancel.borrow() || Instant::now() >= deadline {
        selection.stopped_on_deadline = Instant::now() >= deadline && !*cancel.borrow();
        return selection;
    }
    let Some(_album) = tracks.first().and_then(|track| track.album.as_deref()) else {
        return selection;
    };
    selection.candidate_pairs_total = candidates.len().saturating_mul(tracks.len());
    let expected_ordinals = tracks
        .iter()
        .map(soulseek_expected_album_ordinals)
        .collect::<Vec<_>>();
    let title_keys = tracks
        .iter()
        .map(|track| matching::normalize(&track.title))
        .collect::<Vec<_>>();
    let mut groups = HashMap::<(String, String), Vec<Candidate>>::new();
    for candidate in candidates {
        if *cancel.borrow() || Instant::now() >= deadline {
            selection.stopped_on_deadline = Instant::now() >= deadline && !*cancel.borrow();
            return selection;
        }
        if candidate.provider != "slskd" || candidate.permission == "restricted" {
            continue;
        }
        let Some(CandidateSource::Soulseek {
            username, filename, ..
        }) = candidate.source.as_ref()
        else {
            continue;
        };
        let Some(directory) = crate::recording_filename::remote_parent_directory_key(filename)
        else {
            continue;
        };
        let Some(observations) =
            crate::recording_filename::verified_filename_observations(filename, &candidate.title)
        else {
            continue;
        };
        if observations["folderAlbum"]
            .as_str()
            .is_some_and(|folder_album| {
                tracks
                    .first()
                    .is_none_or(|track| !soulseek_album_folder_matches(folder_album, track))
            })
        {
            continue;
        }
        if candidate.formats.is_empty()
            || candidate.formats.len() > 4
            || candidate.formats.iter().any(|format| {
                !matches!(
                    format.extension.to_ascii_lowercase().as_str(),
                    "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "wma"
                )
            })
        {
            continue;
        }
        groups
            .entry((username.clone(), directory))
            .or_default()
            .push(candidate);
    }

    let mut ranked_groups = groups
        .into_iter()
        .map(|((peer, directory), files)| {
            let mut ordinal_pair_count = 0usize;
            let mut exact_title_pair_count = 0usize;
            let mut ordinal_tracks = std::collections::HashSet::new();
            let mut exact_title_tracks = std::collections::HashSet::new();
            for candidate in &files {
                let Some(CandidateSource::Soulseek { filename, .. }) = candidate.source.as_ref()
                else {
                    continue;
                };
                let Some(observations) = crate::recording_filename::verified_filename_observations(
                    filename,
                    &candidate.title,
                ) else {
                    continue;
                };
                let filename_track = observations["trackNumber"]
                    .as_u64()
                    .and_then(|number| u32::try_from(number).ok());
                let filename_disc = observations["discNumber"]
                    .as_u64()
                    .and_then(|number| u32::try_from(number).ok());
                let filename_title = observations["normalizedTitle"]
                    .as_str()
                    .unwrap_or(&candidate.title);
                let filename_title_key = matching::normalize(filename_title);
                for (index, _) in tracks.iter().enumerate() {
                    let (expected_track, expected_disc) = expected_ordinals[index];
                    if expected_track.is_some_and(|expected| {
                        filename_track.is_some_and(|found| expected != found)
                    }) || expected_disc.is_some_and(|expected| {
                        filename_disc.is_some_and(|found| expected != found)
                    }) {
                        continue;
                    }
                    ordinal_pair_count = ordinal_pair_count.saturating_add(1);
                    ordinal_tracks.insert(index);
                    if filename_title_key == title_keys[index] {
                        exact_title_pair_count = exact_title_pair_count.saturating_add(1);
                        exact_title_tracks.insert(index);
                    }
                }
            }
            let mut formats = files
                .iter()
                .map(soulseek_album_format_rank)
                .collect::<Vec<_>>();
            formats.sort_by(|left, right| right.cmp(left));
            let potential_slots = tracks.len().min(formats.len());
            let potential_lossless_count = formats
                .iter()
                .take(potential_slots)
                .filter(|format| format.0)
                .count();
            let potential_bitrate_total = formats
                .iter()
                .take(potential_slots)
                .map(|format| format.1)
                .fold(0u64, u64::saturating_add);
            let folder_artist_file_count = files
                .iter()
                .filter(|candidate| {
                    candidate.source_metadata.as_ref().is_some_and(|metadata| {
                        metadata["filenameObservations"]["folderArtist"]
                            .as_str()
                            .is_some_and(|value| !value.trim().is_empty())
                    })
                })
                .count();
            let best_availability = files.iter().map(slskd_availability_order).min().unwrap_or((
                u8::MAX,
                u8::MAX,
                u32::MAX,
                std::cmp::Reverse(0),
            ));
            SoulseekAlbumCandidateGroup {
                peer,
                directory,
                files,
                potential_lossless_count,
                potential_bitrate_total,
                folder_artist_file_count,
                best_availability,
                exact_title_pair_count,
                ordinal_pair_count,
                exact_title_track_count: exact_title_tracks.len(),
                ordinal_track_count: ordinal_tracks.len(),
            }
        })
        .collect::<Vec<_>>();
    ranked_groups.sort_by(compare_soulseek_album_candidate_groups);
    selection.groups_total = ranked_groups.len();
    trace_soulseek_album(
        "group_plan",
        serde_json::json!({
            "groups": ranked_groups.iter().enumerate().map(|(rank, group)| serde_json::json!({
                "rank": rank + 1,
                "groupHash": soulseek_album_group_fingerprint(&group.peer, &group.directory),
                "fileCount": group.files.len(),
                "exactTitleTrackCount": group.exact_title_track_count,
                "ordinalTrackCount": group.ordinal_track_count,
                "exactTitlePairCount": group.exact_title_pair_count,
                "ordinalPairCount": group.ordinal_pair_count,
                "potentialLosslessCount": group.potential_lossless_count,
                "potentialBitrateTotal": group.potential_bitrate_total,
                "folderArtistFileCount": group.folder_artist_file_count,
            })).collect::<Vec<_>>(),
        }),
    );

    let mut evaluations = Vec::new();
    let mut context = crate::recording_match_context::AssessmentContext::default();
    'groups: for (group_rank, group) in ranked_groups.into_iter().enumerate() {
        if *cancel.borrow() {
            return selection;
        }
        if Instant::now() >= deadline {
            selection.stopped_on_deadline = true;
            break 'groups;
        }
        selection.groups_examined += 1;
        let group_hash = soulseek_album_group_fingerprint(&group.peer, &group.directory);
        let mut group_rejections = HashMap::<String, usize>::new();
        let mut group_assessments = 0usize;
        let mut group_ambiguous_files = 0usize;
        let mut group_unmatched_files = 0usize;
        let group_file_count = group.files.len();
        let mut per_track = vec![Vec::<Candidate>::new(); tracks.len()];
        for candidate in group.files {
            if *cancel.borrow() {
                return selection;
            }
            if Instant::now() >= deadline {
                selection.stopped_on_deadline = true;
                break 'groups;
            }
            let Some(CandidateSource::Soulseek { filename, .. }) = candidate.source.as_ref() else {
                continue;
            };
            let Some(observations) = crate::recording_filename::verified_filename_observations(
                filename,
                &candidate.title,
            ) else {
                continue;
            };
            let filename_track = observations["trackNumber"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok());
            let filename_disc = observations["discNumber"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok());
            let filename_title = observations["normalizedTitle"]
                .as_str()
                .unwrap_or(&candidate.title);
            let filename_title_key = matching::normalize(filename_title);
            let mut matches = Vec::new();
            let mut ordinal_matches = Vec::new();
            for (index, track) in tracks.iter().enumerate() {
                if *cancel.borrow() {
                    return selection;
                }
                if Instant::now() >= deadline {
                    selection.stopped_on_deadline = true;
                    break 'groups;
                }
                let (expected_track, expected_disc) = soulseek_expected_album_ordinals(track);
                if let (Some(expected), Some(observed)) = (expected_track, filename_track) {
                    if expected != observed {
                        continue;
                    }
                }
                if let (Some(expected), Some(observed)) = (expected_disc, filename_disc) {
                    if expected != observed {
                        continue;
                    }
                }
                ordinal_matches.push(index);
            }
            selection.candidate_pairs_after_ordinal = selection
                .candidate_pairs_after_ordinal
                .saturating_add(ordinal_matches.len());
            // Exact normalized filename titles run first for cheapness, but
            // alternate titles can still be accepted by shared evidence.
            // Keep every ordinal-compatible track in the uniqueness check.
            let exact_title_matches = ordinal_matches
                .iter()
                .copied()
                .filter(|index| filename_title_key == title_keys[*index])
                .collect::<Vec<_>>();
            let pair_indices =
                ordered_soulseek_album_pair_indices(&ordinal_matches, &exact_title_matches);
            selection.candidate_pairs_after_title = selection
                .candidate_pairs_after_title
                .saturating_add(pair_indices.len());
            for index in pair_indices {
                if *cancel.borrow() {
                    return selection;
                }
                if Instant::now() >= deadline {
                    selection.stopped_on_deadline = true;
                    break 'groups;
                }
                let track = &tracks[index];
                let aliases = &aliases[index];
                selection.shared_assessments = selection.shared_assessments.saturating_add(1);
                group_assessments = group_assessments.saturating_add(1);
                let assessment = crate::recording_match_context::assess_with_aliases_and_context(
                    store,
                    track,
                    &candidate,
                    matching_level,
                    MatchStage::Resolved,
                    aliases.tracks(),
                    aliases.isrc_enriched(),
                    aliases.verified_recording_isrc(),
                    aliases.release_runtime_variants(),
                    &mut context,
                );
                let rejection_reasons =
                    soulseek_album_assessment_rejections(&assessment, track, &candidate);
                if !rejection_reasons.is_empty() {
                    for reason in rejection_reasons {
                        *selection
                            .assessment_rejections
                            .entry(reason.to_owned())
                            .or_default() += 1;
                        *group_rejections.entry(reason.to_owned()).or_default() += 1;
                    }
                    continue;
                }
                matches.push(index);
            }
            // A peer file that plausibly maps to more than one track is not
            // auto-assigned; repeated titles require stronger number evidence.
            if let Some(index) = unique_soulseek_album_track_match(&matches) {
                per_track[index].push(candidate);
            } else if matches.is_empty() {
                group_unmatched_files = group_unmatched_files.saturating_add(1);
            } else {
                group_ambiguous_files = group_ambiguous_files.saturating_add(1);
            }
        }

        let mut assignments = Vec::new();
        let mut lossless_count = 0;
        let mut bitrate_total = 0u64;
        for (index, mut options) in per_track.into_iter().enumerate() {
            if options.is_empty() {
                continue;
            }
            options.sort_by(|left, right| {
                soulseek_album_format_rank(right)
                    .cmp(&soulseek_album_format_rank(left))
                    .then_with(|| {
                        slskd_availability_order(left).cmp(&slskd_availability_order(right))
                    })
                    .then_with(|| left.id.cmp(&right.id))
            });
            let best = options.remove(0);
            let (lossless, bitrate, _, _, _) = soulseek_album_format_rank(&best);
            lossless_count += usize::from(lossless);
            bitrate_total = bitrate_total.saturating_add(bitrate);
            assignments.push((index, best));
        }
        let availability = assignments
            .iter()
            .map(|(_, candidate)| slskd_availability_order(candidate))
            .min()
            .unwrap_or((u8::MAX, u8::MAX, u32::MAX, std::cmp::Reverse(0)));
        let assigned_track_count = assignments.len();
        if assigned_track_count >= minimum_soulseek_album_coverage(tracks.len()) {
            selection.best_coverage_at_stop =
                selection.best_coverage_at_stop.max(assigned_track_count);
            evaluations.push(EvaluatedSoulseekAlbumGroup {
                peer: group.peer,
                directory: group.directory,
                assignments,
                lossless_count,
                bitrate_total,
                availability,
            });
        }
        trace_soulseek_album(
            "group_evaluated",
            serde_json::json!({
                "rank": group_rank + 1,
                "groupHash": group_hash,
                "fileCount": group_file_count,
                "sharedAssessments": group_assessments,
                "assessmentRejections": group_rejections,
                "unmatchedFiles": group_unmatched_files,
                "ambiguousFiles": group_ambiguous_files,
                "assignedTracks": assigned_track_count,
                "losslessAssigned": lossless_count,
                "bitrateAssigned": bitrate_total,
            }),
        );
    }
    finish_soulseek_album_selection(selection, evaluations, tracks)
}

fn finish_soulseek_album_selection(
    mut selection: SoulseekAlbumSelection,
    mut evaluations: Vec<EvaluatedSoulseekAlbumGroup>,
    tracks: &[Track],
) -> SoulseekAlbumSelection {
    evaluations.sort_by(|left, right| {
        right
            .assignments
            .len()
            .cmp(&left.assignments.len())
            .then_with(|| right.lossless_count.cmp(&left.lossless_count))
            .then_with(|| right.bitrate_total.cmp(&left.bitrate_total))
            .then_with(|| left.availability.cmp(&right.availability))
            .then_with(|| left.peer.cmp(&right.peer))
            .then_with(|| left.directory.cmp(&right.directory))
    });
    let Some(best) = evaluations.into_iter().next() else {
        return selection;
    };
    let coverage = best.assignments.len();
    selection.best_coverage_at_stop = selection.best_coverage_at_stop.max(coverage);
    selection.assignments = best
        .assignments
        .into_iter()
        .map(|(index, candidate)| SoulseekAlbumAssignment {
            track_id: tracks[index].id.clone(),
            candidate,
            group_peer: best.peer.clone(),
            group_directory: best.directory.clone(),
            group_coverage: coverage,
            expected_tracks: tracks.len(),
        })
        .collect();
    selection
}

fn split_remote_title(value: &str) -> (String, String) {
    let mut parts = value.split(" - ");
    let first = parts.next().unwrap_or_default().trim();
    let second = parts.next().unwrap_or_default().trim();
    if !first.is_empty()
        && !is_numbered_track_prefix(first)
        && !second.is_empty()
        && parts.next().is_none()
    {
        (first.to_owned(), second.to_owned())
    } else {
        (String::new(), value.trim().to_owned())
    }
}

fn is_numbered_track_prefix(value: &str) -> bool {
    fn short_number(value: &str) -> bool {
        (1..=2).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
    }
    if short_number(value) {
        return true;
    }
    let Some((disc, track)) = value.split_once('-') else {
        return false;
    };
    short_number(disc) && short_number(track)
}

#[async_trait::async_trait]
impl AcquisitionProvider for AcquisitionManager {
    async fn find_candidates(
        &self,
        track: &Track,
        provider_ids: &[String],
        settings: &Settings,
    ) -> Result<Vec<Candidate>, String> {
        let web_providers = provider_ids
            .iter()
            .filter(|id| matches!(id.as_str(), "youtube" | "soundcloud" | "monochrome"))
            .cloned()
            .collect::<Vec<_>>();
        let wants_slskd = provider_ids.iter().any(|id| id == "slskd");
        let mut found = Vec::new();
        let mut errors = Vec::new();
        let selected_slskd = track
            .provenance
            .get("audioCandidate")
            .and_then(|value| serde_json::from_value::<Candidate>(value.clone()).ok())
            .filter(|candidate| candidate.provider == "slskd" && candidate.source.is_some());
        if let Some(candidate) = &selected_slskd {
            found.push(candidate.clone());
        }
        if !web_providers.is_empty() {
            let key = search_cache_key(track, &web_providers, settings, 10);
            match self
                .inner
                .search_cache
                .get_or_search(key, || find_candidates(track, &web_providers, settings))
                .await
            {
                Ok(candidates) => found.extend(candidates),
                Err(error) => errors.push(error),
            }
        }
        if wants_slskd {
            let slskd_key = search_cache_key(track, &["slskd".into()], settings, 10);
            match self
                .inner
                .search_cache
                .get_or_search(slskd_key, || self.find_slskd_candidates_for_track(track))
                .await
            {
                Ok(candidates) => found.extend(candidates),
                Err(error) => errors.push(error),
            }
        }
        if found.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let history = self.inner.store.list_jobs()?;
        found.sort_by(|a, b| {
            b.score.total_cmp(&a.score).then_with(|| {
                if a.provider == "slskd" && b.provider == "slskd" {
                    slskd_peer_failure_order(a, &history)
                        .cmp(&slskd_peer_failure_order(b, &history))
                        .then_with(|| slskd_availability_order(a).cmp(&slskd_availability_order(b)))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        let mut ids = std::collections::HashSet::new();
        found.retain(|candidate| ids.insert(candidate.id.clone()));
        Ok(found)
    }
    async fn inspect_formats(
        &self,
        track: &Track,
        candidate: &Candidate,
        settings: &Settings,
    ) -> Result<Candidate, String> {
        if candidate.provider == "slskd" {
            if candidate.source.is_none() {
                return Err("Soulseek candidate is missing its typed source".into());
            }
            Ok(candidate.clone())
        } else {
            inspect_candidate(track, candidate, settings).await
        }
    }
    async fn download(
        &self,
        request: DownloadRequest,
        settings: Settings,
    ) -> Result<DownloadJob, String> {
        self.enqueue(request, settings).await
    }
}

fn resumable_soulseek_job(job: &DownloadJob) -> bool {
    job.state == "interrupted"
        && job.candidate.provider == "slskd"
        && job.candidate.permission != "restricted"
        && matches!(
            job.candidate.source.as_ref(),
            Some(CandidateSource::Soulseek { .. })
        )
}

fn can_retry(state: &str) -> bool {
    ["failed", "cancelled", "interrupted"].contains(&state)
}

async fn download_direct(
    raw: &str,
    staging: &Path,
    mut cancel: watch::Receiver<bool>,
) -> Result<PathBuf, String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let mut url = validate_source_url(raw, "direct")?;
    let mut response = None;
    for _ in 0..=5 {
        let addresses = public_addresses(&url).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(url.host_str().ok_or("Missing host")?, &addresses)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(1800))
            .build()
            .map_err(|e| e.to_string())?;
        let value = tokio::select! {
            value=client.get(url.clone()).send()=>value.map_err(|e|format!("Direct download failed: {e}"))?,
            _=cancel.changed()=>return Err("Cancelled".into()),
        };
        if value.status().is_redirection() {
            let next = value
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|x| x.to_str().ok())
                .ok_or("Invalid download redirect")?;
            url = validate_source_url(
                url.join(next).map_err(|e| e.to_string())?.as_str(),
                "direct",
            )?;
        } else {
            response = Some(value.error_for_status().map_err(|e| e.to_string())?);
            break;
        }
    }
    let response = response.ok_or("Too many direct download redirects")?;
    if response
        .content_length()
        .is_some_and(|x| x > 2 * 1024 * 1024 * 1024)
    {
        return Err("Audio file exceeds 2 GB limit".into());
    }
    let extension = url
        .path()
        .rsplit('.')
        .next()
        .filter(|x| ["mp3", "flac", "opus", "m4a", "aac", "wav", "aiff", "ogg"].contains(x))
        .unwrap_or("audio");
    let path = staging.join(format!("source.{extension}"));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut size = 0u64;
    loop {
        let chunk = tokio::select! { value=stream.next()=>value, _=cancel.changed()=>return Err("Cancelled".into()) };
        match chunk {
            Some(Ok(bytes)) => {
                size += bytes.len() as u64;
                if size > 2 * 1024 * 1024 * 1024 {
                    return Err("Audio file exceeds 2 GB limit".into());
                }
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
            }
            Some(Err(e)) => return Err(e.to_string()),
            None => break,
        }
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slskd_poll_fixture(id: &str) -> SlskdSearchPoll {
        SlskdSearchPoll {
            id: id.into(),
            state: "complete".into(),
            tracks: vec![Track {
                id: format!("track:{id}"),
                ..Track::default()
            }],
            candidates: vec![],
            complete: true,
            warnings: vec![],
        }
    }

    fn slskd_response_fixture(
        username: &str,
        filenames: &[(&str, u64)],
        free_slot: Option<bool>,
    ) -> crate::slskd::SearchResponse {
        crate::slskd::SearchResponse {
            username: username.into(),
            files: filenames
                .iter()
                .map(|(filename, size)| crate::slskd::RemoteFile {
                    filename: (*filename).into(),
                    size: *size,
                    bit_rate: None,
                    length: None,
                    attributes: vec![],
                })
                .collect(),
            has_free_upload_slot: free_slot,
            queue_length: None,
            upload_speed: None,
        }
    }

    #[test]
    fn slskd_poll_response_merge_keeps_embedded_rows_when_separate_endpoint_is_empty() {
        let embedded = vec![slskd_response_fixture(
            "peer-a",
            &[("Music/Artist/Album/01 Song.flac", 1_000)],
            Some(true),
        )];
        let merged = merge_slskd_search_responses(&embedded, &[]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].files.len(), 1);
        assert_eq!(slskd_response_file_count(&merged), 1);
    }

    #[test]
    fn slskd_poll_response_merge_unions_files_and_prefers_fresh_duplicate_rows() {
        let embedded = vec![slskd_response_fixture(
            "Peer-A",
            &[("Music/Artist/Album/01 Song.flac", 1_000)],
            Some(false),
        )];
        let separate = vec![slskd_response_fixture(
            "peer-a",
            &[
                ("Music/Artist/Album/01 Song.flac", 1_000),
                ("Music/Artist/Album/02 Song.flac", 2_000),
            ],
            Some(true),
        )];
        let merged = merge_slskd_search_responses(&embedded, &separate);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].files.len(), 2);
        assert_eq!(merged[0].has_free_upload_slot, Some(true));
        assert_eq!(slskd_response_file_count(&merged), 2);
    }

    #[tokio::test]
    async fn concurrent_slskd_poll_waiter_reuses_cached_result_after_owner_deletes_session() {
        let searches = Arc::new(Mutex::new(HashMap::from([(
            "search-1".to_owned(),
            SlskdSearchSession {
                started: Instant::now(),
                retain_completed_results: true,
                poll_gate: Arc::new(tokio::sync::Mutex::new(())),
            },
        )])));
        let completed = Arc::new(Mutex::new(HashMap::new()));
        let remote_poll_count = Arc::new(AtomicU64::new(0));
        let owner_entered = Arc::new(tokio::sync::Notify::new());
        let waiter_waiting = Arc::new(tokio::sync::Notify::new());
        let release_owner = Arc::new(tokio::sync::Notify::new());

        let owner_searches = searches.clone();
        let owner_completed = completed.clone();
        let owner_count = remote_poll_count.clone();
        let owner_entered_signal = owner_entered.clone();
        let owner_release = release_owner.clone();
        let owner = tokio::spawn(async move {
            if let Some(result) = cached_slskd_search(&owner_completed, "search-1")? {
                return Ok::<SlskdSearchPoll, String>(result);
            }
            let gate = slskd_search_poll_gate(&owner_searches, "search-1")?;
            let _guard = gate.lock().await;
            if let Some(result) = cached_slskd_search(&owner_completed, "search-1")? {
                return Ok::<SlskdSearchPoll, String>(result);
            }
            let _ = active_slskd_search_session(&owner_searches, "search-1")?;
            owner_count.fetch_add(1, Ordering::SeqCst);
            owner_entered_signal.notify_one();
            owner_release.notified().await;
            let result = slskd_poll_fixture("search-1");
            remember_completed_slskd_search(&owner_completed, "search-1", &result)?;
            owner_searches.lock().unwrap().remove("search-1");
            Ok::<SlskdSearchPoll, String>(result)
        });

        owner_entered.notified().await;
        let waiter_searches = searches.clone();
        let waiter_completed = completed.clone();
        let waiter_count = remote_poll_count.clone();
        let waiter_waiting_signal = waiter_waiting.clone();
        let waiter = tokio::spawn(async move {
            if let Some(result) = cached_slskd_search(&waiter_completed, "search-1")? {
                return Ok::<SlskdSearchPoll, String>(result);
            }
            let gate = slskd_search_poll_gate(&waiter_searches, "search-1")?;
            waiter_waiting_signal.notify_one();
            let _guard = gate.lock().await;
            if let Some(result) = cached_slskd_search(&waiter_completed, "search-1")? {
                return Ok::<SlskdSearchPoll, String>(result);
            }
            let _ = active_slskd_search_session(&waiter_searches, "search-1")?;
            waiter_count.fetch_add(1, Ordering::SeqCst);
            panic!("serialized waiter must use the owner's completed result");
        });

        waiter_waiting.notified().await;
        release_owner.notify_one();
        let owner_result = owner.await.unwrap().unwrap();
        let waiter_result = waiter.await.unwrap().unwrap();
        assert_eq!(remote_poll_count.load(Ordering::SeqCst), 1);
        assert_eq!(waiter_result.tracks[0].id, owner_result.tracks[0].id);
        assert!(active_slskd_search_session(&searches, "search-1").is_err());
    }

    #[test]
    fn slskd_completed_result_cache_is_bounded_and_expires_old_entries() {
        let completed = Mutex::new(HashMap::new());
        for index in 0..(SLSKD_COMPLETED_SEARCH_LIMIT + 1) {
            let id = format!("search-{index}");
            remember_completed_slskd_search(&completed, &id, &slskd_poll_fixture(&id)).unwrap();
        }
        let cache = completed.lock().unwrap();
        assert_eq!(cache.len(), SLSKD_COMPLETED_SEARCH_LIMIT);
        assert!(!cache.contains_key("search-0"));
        drop(cache);

        completed.lock().unwrap().insert(
            "expired".into(),
            (
                Instant::now() - SLSKD_COMPLETED_SEARCH_TTL - Duration::from_secs(1),
                slskd_poll_fixture("expired"),
            ),
        );
        assert!(cached_slskd_search(&completed, "expired")
            .unwrap()
            .is_none());
        assert!(!completed.lock().unwrap().contains_key("expired"));
    }

    #[test]
    fn soulseek_album_uses_only_verified_missing_musicbrainz_ordinals() {
        let release_id = "1f602996-f661-3611-b25a-93c3337fd76e";
        let mut track = Track {
            provider: "catalog".into(),
            provider_id: "mb-track-id".into(),
            source_url: format!("https://musicbrainz.org/release/{release_id}"),
            provenance: serde_json::json!({
                "sourceKind": "catalog",
                "catalogProvider": "musicbrainz",
                "releaseId": release_id,
                "sourceUrl": format!("https://musicbrainz.org/release/{release_id}"),
                "trackPositionOrdinal": 8,
                "sourceMediumPosition": 2,
                "manualFields": {}
            }),
            ..Track::default()
        };
        assert_eq!(soulseek_expected_album_ordinals(&track), (Some(8), Some(2)));

        track.provenance["manualFields"] = serde_json::json!({
            "trackNumber": true,
            "discNumber": true
        });
        assert_eq!(soulseek_expected_album_ordinals(&track), (None, None));

        track.provenance["manualFields"] = serde_json::json!({});
        track.provenance["sourceUrl"] = serde_json::json!("https://example.test/release");
        assert_eq!(soulseek_expected_album_ordinals(&track), (None, None));
    }

    #[test]
    fn slskd_track_display_uses_weak_normalized_title_without_guessing_number_as_artist() {
        let candidate = Candidate {
            id: "slskd:fixture".into(),
            provider: "slskd".into(),
            title: "01 - Pilentze Pee".into(),
            duration_seconds: Some(141.0),
            source_metadata: Some(serde_json::json!({
                "sourceMetadataOrigin":"slskdFilename",
                "filenameObservations":{
                    "normalizedTitle":"Pilentze Pee",
                    "folderArtist":"Bulgarian State Television Female Choir"
                }
            })),
            ..Candidate::default()
        };
        let track = slskd_candidate_track(&candidate);
        assert_eq!(track.artist, "");
        assert_eq!(track.title, "Pilentze Pee");
        assert_eq!(candidate.title, "01 - Pilentze Pee");
        assert_eq!(
            candidate.source_metadata.as_ref().unwrap()["filenameObservations"]["folderArtist"],
            "Bulgarian State Television Female Choir"
        );
    }

    #[test]
    fn numbered_filename_segments_are_not_inferred_as_artist_credits() {
        for raw in [
            "01 - Pilentze Pee",
            "12 - Pilentze Pee",
            "1-01 - Pilentze Pee",
        ] {
            let (artist, title) = split_remote_title(raw);
            assert!(artist.is_empty(), "{raw}");
            assert_eq!(title, raw, "{raw}");
        }
        assert_eq!(
            split_remote_title("Bulgarian State Choir - Pilentze Pee"),
            ("Bulgarian State Choir".into(), "Pilentze Pee".into())
        );
    }

    #[test]
    fn download_policy_snapshot_survives_settings_change() {
        let mut track = Track::default();
        assert_eq!(
            download_matching_level(&track),
            crate::recording_match::MatchLevel::Balanced
        );
        track.provenance = serde_json::json!({"downloadMatchingLevel":"exact"});
        assert_eq!(
            download_matching_level(&track),
            crate::recording_match::MatchLevel::Exact
        );
    }

    #[test]
    fn full_inspection_metadata_retains_bounded_recording_identity() {
        let inspected = candidate_from_json(
            &serde_json::json!({
                "id":"fresh-video-id",
                "title":"Artist - Song (Live)",
                "track":"Song (Live)",
                "artists":["Artist", "Guest Artist"],
                "album":"Live at the Hall",
                "album_artist":"Live Ensemble",
                "version":"Live",
                "genre":"rumba",
                "isrc":"USABC2600001",
                "duration":180.0,
                "view_count":27
            }),
            "youtube",
        );
        let metadata = inspected.source_metadata.unwrap();
        assert_eq!(metadata["sourceMetadataOrigin"], "ytDlpCandidateMetadata");
        assert_eq!(metadata["resolvedProviderId"], "fresh-video-id");
        assert_eq!(metadata["catalogTitle"], "Song (Live)");
        assert_eq!(
            metadata["artists"],
            serde_json::json!(["Artist", "Guest Artist"])
        );
        assert_eq!(metadata["artistCount"], 2);
        assert_eq!(metadata["album"], "Live at the Hall");
        assert_eq!(metadata["albumArtist"], "Live Ensemble");
        assert_eq!(metadata["version"], "Live");
        assert_eq!(metadata["genre"], "rumba");
        assert_eq!(metadata["views"], 27);
    }

    fn source_reported_fixture() -> (Track, Candidate) {
        let mut track = Track::default();
        track.id = "youtube:video-123".into();
        track.provider = "youtube".into();
        track.provider_id = "video-123".into();
        track.source_url = "https://youtube.com/watch?v=video-123".into();
        track.title = "La Cubanita".into();
        let candidate = Candidate {
            id: track.id.clone(),
            provider: track.provider.clone(),
            url: track.source_url.clone(),
            source_metadata: Some(serde_json::json!({
                "sourceMetadataOrigin":"ytDlpFullInfo",
                "resolvedProviderId":"video-123",
                "artists":["Los Niños de Sara"],
                "catalogTitle":"La Cubanita",
                "album":"Gipsyolé",
                "genre":"rumba"
            })),
            ..Candidate::default()
        };
        (track, candidate)
    }

    #[test]
    fn exact_source_metadata_fills_missing_fields_with_unverified_provenance() {
        let (track, candidate) = source_reported_fixture();
        let (enriched, fields) = promote_literal_source_metadata(&track, &candidate);
        assert_eq!(enriched.artist, "Los Niños de Sara");
        assert_eq!(enriched.title, "La Cubanita");
        assert_eq!(enriched.album.as_deref(), Some("Gipsyolé"));
        assert_eq!(enriched.genre.as_deref(), Some("rumba"));
        assert_eq!(enriched.id, track.id);
        assert_eq!(enriched.provider_id, track.provider_id);
        assert_eq!(
            enriched.provenance["sourceReportedMetadata"]["identityStatus"],
            "source_reported_unverified"
        );
        assert_eq!(
            enriched.provenance["sourceReportedMetadata"]["fields"]
                .as_array()
                .unwrap()
                .len(),
            fields.len()
        );
    }

    #[test]
    fn exact_original_source_selection_keeps_structured_metadata_unverified() {
        let (track, candidate) = source_reported_fixture();
        let artifact = artifact_track_for_match(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Unresolved,
            true,
        )
        .unwrap();

        assert_eq!(artifact.artist, "Los Niños de Sara");
        assert_eq!(artifact.album.as_deref(), Some("Gipsyolé"));
        assert_eq!(
            artifact.provenance["sourceReportedMetadata"]["identityStatus"],
            "source_reported_unverified"
        );
        assert_eq!(
            artifact.provenance["sourceReportedMetadata"]["fields"],
            serde_json::json!(["artist", "album", "genre"])
        );
        assert_eq!(artifact.provider, track.provider);
        assert_eq!(artifact.provider_id, track.provider_id);
        assert_eq!(artifact.source_url, track.source_url);
    }

    #[test]
    fn exact_original_source_selection_never_promotes_uploader_or_cross_locator_metadata() {
        let (track, mut candidate) = source_reported_fixture();
        candidate.source_metadata = Some(serde_json::json!({
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "resolvedProviderId":"video-123",
            "uploader":"Unrelated Channel"
        }));
        let uploader_only = artifact_track_for_match(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Unresolved,
            true,
        )
        .unwrap();
        assert!(uploader_only.artist.is_empty());
        assert_eq!(uploader_only.album, None);
        assert!(uploader_only.provenance.get("sourceReportedMetadata").is_none());

        candidate.source_metadata = Some(serde_json::json!({
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "resolvedProviderId":"different-video-id",
            "artists":["Wrong Artist"],
            "album":"Wrong Album"
        }));
        let mismatched_id = artifact_track_for_match(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Unresolved,
            true,
        )
        .unwrap();
        assert!(mismatched_id.artist.is_empty());
        assert_eq!(mismatched_id.album, None);
        assert!(mismatched_id.provenance.get("sourceReportedMetadata").is_none());

        candidate.source_metadata = Some(serde_json::json!({
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "resolvedProviderId":"video-123",
            "artists":["Wrong Artist"],
            "album":"Wrong Album"
        }));
        candidate.url = "https://youtube.com/watch?v=other-video".into();
        let mismatched_url = artifact_track_for_match(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Unresolved,
            true,
        );
        assert_eq!(
            mismatched_url.unwrap_err(),
            "Alternative source resolves to the requested catalog identity"
        );
    }

    #[test]
    fn exact_original_source_metadata_does_not_replace_manual_fields() {
        let (mut track, candidate) = source_reported_fixture();
        track.artist = "Curated Artist".into();
        track.album = Some("Curated Album".into());
        track.provenance = serde_json::json!({"manualFields":{"artist":true,"album":true}});
        let artifact = artifact_track_for_match(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Unresolved,
            true,
        )
        .unwrap();
        assert_eq!(artifact.artist, "Curated Artist");
        assert_eq!(artifact.album.as_deref(), Some("Curated Album"));
        let reported_fields = artifact.provenance["sourceReportedMetadata"]["fields"]
            .as_array()
            .expect("the remaining structured source field is retained");
        assert!(reported_fields.iter().any(|field| field == "genre"));
        assert!(!reported_fields.iter().any(|field| field == "artist"));
        assert!(!reported_fields.iter().any(|field| field == "album"));
    }

    #[test]
    fn source_metadata_cannot_replace_manual_fields_or_cross_source_identity() {
        let (mut track, candidate) = source_reported_fixture();
        track.provenance = serde_json::json!({"manualFields":{"artist":true}});
        track.artist = "Curated Artist".into();
        let (enriched, fields) = promote_literal_source_metadata(&track, &candidate);
        assert_eq!(enriched.artist, "Curated Artist");
        assert!(!fields.iter().any(|field| field == "artist"));

        let mut mismatched = candidate.clone();
        mismatched.url.push_str("&list=other");
        let (unchanged, fields) = promote_literal_source_metadata(&track, &mismatched);
        assert_eq!(unchanged, track);
        assert!(fields.is_empty());
    }

    fn accepted_catalog_fixture() -> (Track, Candidate) {
        let track = Track {
            id: "spotify:requested-track".into(),
            provider: "spotify".into(),
            provider_id: "requested-track".into(),
            title: "Requested title".into(),
            artist: "Requested artist".into(),
            duration_seconds: Some(141.5),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "monochrome:catalog-track-1".into(),
            provider: "monochrome".into(),
            url: "https://monochrome.test/track/catalog-track-1".into(),
            source_metadata: Some(serde_json::json!({
                "provider":"monochrome",
                "trackId":"catalog-track-1",
                "catalogTitle":"Requested title",
                "artistNames":["Requested artist"],
                "isrc":"USNO18716501",
                "album":"Catalog album",
                "albumArtist":"Catalog album artist",
                "genre":"Traditional"
            })),
            ..Candidate::default()
        };
        (track, candidate)
    }

    #[test]
    fn accepted_catalog_metadata_fills_only_missing_non_identity_fields() {
        let (track, candidate) = accepted_catalog_fixture();
        let artifact = promote_matched_catalog_metadata(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Exact,
        );
        assert_eq!(artifact.id, track.id);
        assert_eq!(artifact.provider_id, track.provider_id);
        assert_eq!(artifact.title, track.title);
        assert_eq!(artifact.artist, track.artist);
        assert_eq!(artifact.isrc.as_deref(), Some("USNO18716501"));
        assert_eq!(artifact.album.as_deref(), Some("Catalog album"));
        assert_eq!(
            artifact.album_artist.as_deref(),
            Some("Catalog album artist")
        );
        assert_eq!(artifact.genre.as_deref(), Some("Traditional"));
        assert_eq!(
            artifact.provenance["acceptedSourceMetadata"]["identityStatus"],
            "accepted_recording_match"
        );
    }

    #[test]
    fn catalog_metadata_does_not_override_manual_or_conflicting_values() {
        let (mut track, candidate) = accepted_catalog_fixture();
        track.isrc = Some("USOTHER000001".into());
        track.album = Some("Manually assigned album".into());
        track.genre = Some("Curated genre".into());
        track.provenance = serde_json::json!({"manualFields":{"album":true,"genre":true}});
        let artifact = promote_matched_catalog_metadata(
            &track,
            &candidate,
            crate::recording_match::MatchRelationship::Probable,
        );
        assert_eq!(artifact.isrc.as_deref(), Some("USOTHER000001"));
        assert_eq!(artifact.album.as_deref(), Some("Manually assigned album"));
        assert_eq!(artifact.genre.as_deref(), Some("Curated genre"));
        assert_eq!(
            artifact.album_artist.as_deref(),
            Some("Catalog album artist")
        );
    }

    #[test]
    fn catalog_metadata_requires_accepted_relationship_and_bound_catalog_identity() {
        let (track, candidate) = accepted_catalog_fixture();
        for relationship in [
            crate::recording_match::MatchRelationship::Alternative,
            crate::recording_match::MatchRelationship::Unresolved,
        ] {
            assert_eq!(
                promote_matched_catalog_metadata(&track, &candidate, relationship),
                track
            );
        }
        let mut mismatched = candidate;
        mismatched.source_metadata.as_mut().unwrap()["trackId"] = serde_json::json!("other-id");
        assert_eq!(
            promote_matched_catalog_metadata(
                &track,
                &mismatched,
                crate::recording_match::MatchRelationship::Exact,
            ),
            track
        );
    }

    #[test]
    fn flexible_alternative_tags_keep_requested_identity_in_provenance() {
        let mut requested = Track::default();
        requested.id = "spotify:spotify-track-id".into();
        requested.provider = "spotify".into();
        requested.provider_id = "spotify-track-id".into();
        requested.artist = "Requested Artist".into();
        requested.title = "Requested Title".into();
        requested.album = Some("Requested Album".into());
        requested.album_artist = Some("Requested Album Artist".into());
        requested.isrc = Some("USABC2600001".into());
        let candidate = Candidate {
            id: "youtube:live-video-id".into(),
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=live-video-id".into(),
            title: "Song (Live)".into(),
            source_metadata: Some(serde_json::json!({
                "catalogTitle":"Song (Live)",
                "artist":"Requested Artist",
                "version":"Live",
                "album":"Live at the Hall",
                "albumArtist":"Live Ensemble",
                "isrc":"USXYZ2600002"
            })),
            ..Candidate::default()
        };
        let artifact = artifact_track_for_match(
            &requested,
            &candidate,
            crate::recording_match::MatchRelationship::Alternative,
            true,
        )
        .unwrap();
        assert_eq!(artifact.title, "Song (Live)");
        assert_eq!(artifact.version.as_deref(), Some("Live"));
        assert_eq!(artifact.album.as_deref(), Some("Live at the Hall"));
        assert_eq!(artifact.album_artist.as_deref(), Some("Live Ensemble"));
        assert_eq!(artifact.isrc.as_deref(), Some("USXYZ2600002"));
        assert_eq!(artifact.provider, "youtube");
        assert_eq!(artifact.provider_id, "live-video-id");
        assert_ne!(artifact.id, requested.id);
        assert_eq!(requested.title, "Requested Title");
        assert_eq!(requested.provider_id, "spotify-track-id");
        assert_eq!(
            artifact.provenance["requestedTrackIdentity"]["id"],
            "spotify:spotify-track-id"
        );
        assert_eq!(
            artifact.provenance["requestedTrackIdentity"]["originalId"],
            "spotify:spotify-track-id"
        );
        assert_eq!(
            artifact.provenance["requestedTrackIdentity"]["title"],
            "Requested Title"
        );
        assert_eq!(
            artifact.provenance["requestedTrackIdentity"]["matchingLevel"],
            Value::Null
        );
        assert_eq!(
            artifact.provenance["actualSourceIdentity"]["id"],
            artifact.id
        );
    }

    #[test]
    fn alternative_without_fresh_release_identity_clears_catalog_values_but_keeps_manual_edits() {
        let mut requested = Track {
            id: "spotify:requested".into(),
            provider: "spotify".into(),
            provider_id: "requested".into(),
            title: "Requested title".into(),
            artist: "Requested artist".into(),
            album: Some("Requested album".into()),
            album_artist: Some("Requested album artist".into()),
            release_date: Some("2021-05-06".into()),
            recording_date: Some("2021".into()),
            catalog_number: Some("CAT-123".into()),
            label: Some("Requested label".into()),
            genre: Some("Requested genre".into()),
            normalized_genre: Some("requested-genre".into()),
            bpm: Some(128.0),
            key: Some("8A".into()),
            remixers: Some(vec!["Requested remixer".into()]),
            track_number: Some(7),
            disc_number: Some(2),
            artwork_url: Some("https://example.test/manual-cover.jpg".into()),
            isrc: Some("USABC2600001".into()),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "youtube:alternative".into(),
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=alternative".into(),
            title: "Actual title (Live)".into(),
            source_metadata: Some(serde_json::json!({
                "catalogTitle":"Actual title (Live)",
                "resolvedProviderId":"alternative"
            })),
            ..Candidate::default()
        };
        let artifact = artifact_track_for_match(
            &requested,
            &candidate,
            crate::recording_match::MatchRelationship::Alternative,
            true,
        )
        .unwrap();
        assert_eq!(artifact.album, None);
        assert_eq!(artifact.album_artist, None);
        assert_eq!(artifact.isrc, None);
        assert_eq!(artifact.artist, "");
        assert_eq!(artifact.release_date, None);
        assert_eq!(artifact.recording_date, None);
        assert_eq!(artifact.catalog_number, None);
        assert_eq!(artifact.label, None);
        assert_eq!(artifact.genre, None);
        assert_eq!(artifact.normalized_genre, None);
        assert_eq!(artifact.bpm, None);
        assert_eq!(artifact.key, None);
        assert_eq!(artifact.remixers, None);
        assert_eq!(artifact.track_number, None);
        assert_eq!(artifact.disc_number, None);
        assert_eq!(artifact.artwork_url, None);

        requested.provenance = serde_json::json!({"manualFields":{
            "album":true,"albumArtist":true,"isrc":true,
            "releaseDate":true,"recordingDate":true,"catalogNumber":true,
            "label":true,"genre":true,"bpm":true,"key":true,"remixers":true,
            "trackNumber":true,"discNumber":true,"artwork":true
        }});
        let manually_tagged = artifact_track_for_match(
            &requested,
            &candidate,
            crate::recording_match::MatchRelationship::Alternative,
            true,
        )
        .unwrap();
        assert_eq!(manually_tagged.album.as_deref(), Some("Requested album"));
        assert_eq!(
            manually_tagged.album_artist.as_deref(),
            Some("Requested album artist")
        );
        assert_eq!(manually_tagged.isrc.as_deref(), Some("USABC2600001"));
        assert_eq!(manually_tagged.release_date.as_deref(), Some("2021-05-06"));
        assert_eq!(manually_tagged.recording_date.as_deref(), Some("2021"));
        assert_eq!(manually_tagged.catalog_number.as_deref(), Some("CAT-123"));
        assert_eq!(manually_tagged.label.as_deref(), Some("Requested label"));
        assert_eq!(manually_tagged.genre.as_deref(), Some("Requested genre"));
        assert_eq!(manually_tagged.bpm, Some(128.0));
        assert_eq!(manually_tagged.key.as_deref(), Some("8A"));
        assert_eq!(
            manually_tagged.remixers.as_ref().unwrap(),
            &vec!["Requested remixer".to_owned()]
        );
        assert_eq!(manually_tagged.track_number, Some(7));
        assert_eq!(manually_tagged.disc_number, Some(2));
        assert_eq!(
            manually_tagged.artwork_url.as_deref(),
            Some("https://example.test/manual-cover.jpg")
        );
    }

    #[test]
    fn explicit_audio_search_source_keeps_its_own_canonical_identity() {
        let requested = Track {
            id: "youtube:live-video-id".into(),
            provider: "youtube".into(),
            provider_id: "live-video-id".into(),
            source_url: "https://youtube.com/watch?v=live-video-id".into(),
            title: "Song (Live)".into(),
            artist: "Artist".into(),
            album: Some("Live Album".into()),
            isrc: Some("USABC2600001".into()),
            provenance: serde_json::json!({
                "sourceKind":"audioSearch",
                "downloadMatchingLevel":"balanced"
            }),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "youtube:live-video-id".into(),
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=live-video-id".into(),
            title: "Artist - Song (Live)".into(),
            source_metadata: Some(serde_json::json!({
                "resolvedProviderId":"live-video-id",
                "track":"Song (Live)"
            })),
            ..Candidate::default()
        };
        let artifact = artifact_track_for_match(
            &requested,
            &candidate,
            crate::recording_match::MatchRelationship::Unresolved,
            true,
        )
        .unwrap();
        assert_eq!(artifact.id, requested.id);
        assert_eq!(artifact.provider_id, requested.provider_id);
        assert_eq!(artifact.album.as_deref(), Some("Live Album"));
        assert_eq!(artifact.isrc.as_deref(), Some("USABC2600001"));
        assert_eq!(artifact.artist, "Artist");
        assert_eq!(
            artifact.provenance["requestedTrackIdentity"]["originalId"],
            requested.id
        );
        assert_eq!(
            artifact.provenance["requestedTrackIdentity"]["matchingLevel"],
            "balanced"
        );
        assert_eq!(
            artifact.provenance["actualSourceIdentity"]["id"],
            requested.id
        );
    }

    #[test]
    fn source_selection_intent_is_ephemeral_and_audio_search_must_match_candidate() {
        let track = Track {
            id: "youtube:video-id".into(),
            provider: "youtube".into(),
            provider_id: "video-id".into(),
            source_url: "https://www.youtube.com/watch?v=video-id".into(),
            provenance: serde_json::json!({
                "sourceKind":"audioSearch",
                "userSelectedSource":true,
                "downloadAutomaticFallback":false
            }),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "youtube:video-id".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=video-id".into(),
            ..Candidate::default()
        };
        let mut chosen = track.clone();
        snapshot_user_selected_source(&mut chosen, &candidate);
        assert!(explicit_download_source(&chosen, &candidate));
        let mut unrelated = candidate.clone();
        unrelated.id = "youtube:another-video".into();
        unrelated.url = "https://www.youtube.com/watch?v=another-video".into();
        assert!(!explicit_download_source(&chosen, &unrelated));
        let artifact = artifact_track_for_match(
            &chosen,
            &candidate,
            crate::recording_match::MatchRelationship::Exact,
            false,
        )
        .unwrap();
        assert!(artifact.provenance.get("userSelectedSource").is_none());
        assert!(artifact
            .provenance
            .get("downloadAutomaticFallback")
            .is_none());
    }

    #[test]
    fn native_playback_search_defaults_on_but_preserves_custom_helpers_and_discovery() {
        assert!(native_youtube_playback_search_enabled(
            "youtube", "yt-dlp", true, None
        ));
        assert!(native_youtube_playback_search_enabled(
            "youtube",
            "yt-dlp",
            true,
            Some("1")
        ));
        for value in [Some("0"), Some("unexpected")] {
            assert!(!native_youtube_playback_search_enabled(
                "youtube", "yt-dlp", true, value
            ));
        }
        for provider in ["soundcloud", "monochrome"] {
            assert!(!native_youtube_playback_search_enabled(
                provider, "yt-dlp", true, None
            ));
        }
        assert!(!native_youtube_playback_search_enabled(
            "youtube", "yt-dlp", false, None
        ));
        assert!(!native_youtube_playback_search_enabled(
            "youtube",
            "/custom/yt-dlp",
            true,
            None
        ));
    }

    #[test]
    fn public_music_feature_locator_retains_complete_explicit_performers_without_proving_playback()
    {
        let raw: Value = serde_json::from_str(include_str!(
            "../fixtures/youtube-music-reckless-performer-runs.json"
        ))
        .unwrap();
        let track = Track {
            title: "Reckless".into(),
            artist: "Nipsey Hussle,Bino Rideaux,Static Major".into(),
            ..Track::default()
        };
        let items = crate::youtube_music::parse_search_response(&raw, 10);
        let candidates = playback_music_catalog_candidates(&track, items.clone());
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].source_metadata.as_ref().unwrap()["catalogArtist"],
            "Nipsey Hussle, Bino Rideaux, Static Major"
        );
        assert!(crate::source_fallback::is_provisional_playback_candidate(
            &track,
            &candidates[0]
        ));
        assert!(
            !crate::source_fallback::playback_music_catalog_recording_matches_verified_extractor(
                &track,
                &candidates[0]
            )
        );
        let mut wrong = items[0].clone();
        wrong.artist = Some("Nipsey Hussle, Wrong Artist".into());
        let wrong_candidates = playback_music_catalog_candidates(&track, vec![wrong]);
        assert!(wrong_candidates.iter().all(|candidate| {
            !crate::source_fallback::is_provisional_playback_candidate(&track, candidate)
        }));
        let mut missing = items[0].clone();
        missing.title = "Reckless".into();
        assert!(playback_music_catalog_candidates(&track, vec![missing])
            .iter()
            .all(
                |candidate| !crate::source_fallback::is_provisional_playback_candidate(
                    &track, candidate
                )
            ));
    }

    #[test]
    fn music_catalog_locator_retains_distinct_same_title_recordings() {
        let track = Track {
            artist: "Imael Angel".into(),
            title: "BAD TIMES".into(),
            ..Track::default()
        };
        let item = || crate::youtube_music::YouTubeMusicItem {
            id: "Hqp8vxkPrI8".into(),
            title: "BAD TIMES".into(),
            artist: Some("Imael Angel".into()),
            album: Some("BAD TIMES".into()),
            duration_seconds: Some(168.0),
            url: "https://music.youtube.com/watch?v=Hqp8vxkPrI8".into(),
            artwork_url: None,
            isrc: None,
        };
        let selected = playback_music_catalog_candidates(&track, vec![item(), item()]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "youtube:Hqp8vxkPrI8");
        assert_eq!(selected[0].confidence, "review");
        assert!(selected[0].uploader.is_none());
        assert!(selected[0]
            .source_metadata
            .as_ref()
            .unwrap()
            .get("channelIsVerified")
            .is_none());
        let mut alternative = item();
        alternative.id = "abcdefghijk".into();
        let distinct = playback_music_catalog_candidates(&track, vec![item(), alternative]);
        assert_eq!(distinct.len(), 2);
        assert_eq!(distinct[0].id, "youtube:Hqp8vxkPrI8");
        assert_eq!(distinct[1].id, "youtube:abcdefghijk");
        let mut long = item();
        long.title = "BAD TIMES (Long Version)".into();
        long.duration_seconds = Some(401.0);
        assert_eq!(
            playback_music_catalog_candidates(&track, vec![item(), long]).len(),
            1
        );
        let mut wrong = item();
        wrong.artist = Some("Other Artist".into());
        let wrong_candidate = playback_music_catalog_candidates(&track, vec![wrong]);
        assert_eq!(wrong_candidate.len(), 1);
        assert_eq!(wrong_candidate[0].id, "youtube:Hqp8vxkPrI8");
        assert!(!crate::source_fallback::is_provisional_playback_candidate(
            &track,
            &wrong_candidate[0]
        ));
        let mut short = item();
        short.duration_seconds = Some(30.0);
        let short_candidates = playback_music_catalog_candidates(&track, vec![short]);
        assert_eq!(short_candidates.len(), 1);
        assert_eq!(
            crate::recording_match::assess(
                &track,
                &short_candidates[0],
                crate::recording_match::MatchLevel::Balanced,
                crate::recording_match::MatchStage::Search
            )
            .action,
            crate::recording_match::MatchAction::Reject
        );
        assert!(track.duration_seconds.is_none());
    }

    #[test]
    fn monochrome_search_is_canonical_plus_at_most_one_distinct_alias() {
        let track = Track {
            artist: "Ансамбъл Пирин".into(),
            title: "Track (Original Mix)".into(),
            isrc: Some("BGAAA0000001".into()),
            ..Track::default()
        };
        let queries = monochrome_search_tracks(&track, Some("Ensemble Pirin"));
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].artist, "Ансамбъл Пирин");
        assert_eq!(queries[1].artist, "Ensemble Pirin");
        assert_eq!(queries[1].title, track.title);
        assert_eq!(queries[1].isrc, track.isrc);
        assert_eq!(monochrome_search_tracks(&track, None).len(), 1);
        assert_eq!(monochrome_search_tracks(&track, Some("  ")).len(), 1);
        assert_eq!(
            monochrome_search_tracks(&track, Some("ансамбъл пирин")).len(),
            1
        );
    }

    #[tokio::test]
    async fn expired_or_cancelled_playback_stage_skips_setup_callback() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (_expired_cancel_tx, expired_cancel) = watch::channel(false);
        let expired_calls = calls.clone();
        let expired = run_playback_stage(
            expired_cancel,
            tokio::time::Instant::now(),
            "deadline expired",
            async move {
                expired_calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>(())
            },
        )
        .await;
        assert_eq!(expired.unwrap_err(), "deadline expired");

        let (cancel_tx, cancelled_rx) = watch::channel(true);
        let cancelled_calls = calls.clone();
        let cancelled = run_playback_stage(
            cancelled_rx,
            tokio::time::Instant::now() + Duration::from_secs(1),
            "deadline expired",
            async move {
                cancelled_calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>(())
            },
        )
        .await;
        assert_eq!(cancelled.unwrap_err(), "Cancelled");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(cancel_tx);
    }

    #[test]
    fn bandcamp_probe_requires_a_small_audio_partial_response_without_purchase_retry() {
        let mut id3_header = vec![0; 1024];
        id3_header[..10].copy_from_slice(b"ID3\x04\x00\x00\x00\x00\x00\x00");
        let accepted = validate_bandcamp_probe(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 0-1023/5098066"),
            Some(1024),
            Some("audio/mpeg"),
            310.254,
            &id3_header,
        );
        assert_eq!(accepted.unwrap(), 5_098_066);
        for (status, range, length, mime) in [
            (reqwest::StatusCode::FORBIDDEN, None, None, None),
            (
                reqwest::StatusCode::OK,
                None,
                Some(5_098_066),
                Some("audio/mpeg"),
            ),
            (
                reqwest::StatusCode::PARTIAL_CONTENT,
                Some("bytes 0-2048/5098066"),
                Some(2049),
                Some("audio/mpeg"),
            ),
            (
                reqwest::StatusCode::PARTIAL_CONTENT,
                Some("bytes 0-1023/5098066"),
                Some(1024),
                Some("text/html"),
            ),
            (
                reqwest::StatusCode::PARTIAL_CONTENT,
                None,
                Some(1024),
                Some("audio/mpeg"),
            ),
        ] {
            assert!(
                validate_bandcamp_probe(status, range, length, mime, 310.254, &id3_header).is_err()
            );
        }
        assert!(validate_bandcamp_probe(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 0-1023/480000"),
            Some(1024),
            Some("audio/mpeg"),
            310.254,
            &id3_header,
        )
        .is_err());
        assert!(validate_bandcamp_probe(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 0-1023/5098066"),
            Some(1024),
            Some("audio/mpeg"),
            310.254,
            &id3_header[..512],
        )
        .is_err());
        assert!(validate_bandcamp_probe(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 0-1023/5098066"),
            Some(1024),
            Some("audio/mpeg"),
            310.254,
            &[0; 1024],
        )
        .is_err());
        let tail_start = 5_098_066 - 1024;
        assert!(validate_bandcamp_tail_probe(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 5097042-5098065/5098066"),
            Some(1024),
            Some("audio/mpeg"),
            (tail_start, 5_098_065),
            5_098_066,
            &[0; 1024],
        )
        .is_ok());
        assert!(validate_bandcamp_tail_probe(
            reqwest::StatusCode::PARTIAL_CONTENT,
            Some("bytes 5097042-5098065/5098066"),
            Some(1024),
            Some("audio/mpeg"),
            (tail_start, 5_098_065),
            5_098_067,
            &[0; 1024],
        )
        .is_err());
        assert_eq!(
            parse_bandcamp_content_range("bytes 0-1023/5098066"),
            Some((0, 1023, 5_098_066))
        );
        assert_eq!(parse_bandcamp_content_range("bytes 0-1023/*"), None);
    }

    #[test]
    fn incomplete_managed_provider_falls_back_to_system_ytdlp_only_when_available() {
        let directory = tempfile::tempdir().unwrap();
        let active = directory.path().join("managed-ytdlp");
        let system = directory.path().join("system-ytdlp");
        std::fs::write(&active, b"managed").unwrap();
        std::fs::write(&system, b"system").unwrap();

        assert_eq!(
            choose_managed_ytdlp(active.clone(), true, system.clone()),
            system
        );
        assert_eq!(
            choose_managed_ytdlp(active.clone(), false, system.clone()),
            active
        );

        let missing_system = directory.path().join("missing-system-ytdlp");
        assert_eq!(
            choose_managed_ytdlp(active.clone(), true, missing_system),
            active
        );
        assert_eq!(
            managed_provider_action(true, true),
            ManagedProviderAction::SystemFallback
        );
        assert_eq!(
            managed_provider_action(true, false),
            ManagedProviderAction::Unavailable
        );
        assert_eq!(
            managed_provider_action(false, false),
            ManagedProviderAction::Managed
        );
    }

    #[test]
    fn mweb_requires_installed_managed_youtube_provider_args() {
        assert!(!should_force_managed_mweb("youtube", "", false));
        assert!(!should_force_managed_mweb("youtube", "yt-dlp", false));
        assert!(!should_force_managed_mweb(
            "youtube",
            "bundled:yt-dlp",
            false
        ));
        assert!(!should_force_managed_mweb("soundcloud", "", true));
        assert!(!should_force_managed_mweb(
            "youtube",
            "/usr/bin/yt-dlp",
            true
        ));
        assert!(should_force_managed_mweb("youtube", "", true));

        // Incomplete managed assets with an available system executable take
        // this path and must not force a client that needs the managed token plugin.
        assert_eq!(
            managed_provider_action(true, true),
            ManagedProviderAction::SystemFallback
        );
        assert!(!should_force_managed_mweb("youtube", "", false));
    }

    #[test]
    fn album_evidence_override_keeps_default_client_but_normal_playback_keeps_mweb() {
        assert!(should_force_managed_mweb_for_extraction(
            "youtube", "", true, None
        ));
        assert!(!should_force_managed_mweb_for_extraction(
            "youtube",
            "",
            true,
            Some(ALBUM_EVIDENCE_AUDIO_SELECTOR),
        ));
        assert!(!should_force_managed_mweb_for_extraction(
            "soundcloud",
            "",
            true,
            Some(ALBUM_EVIDENCE_AUDIO_SELECTOR),
        ));
    }

    #[test]
    fn playback_source_search_drops_imported_unverified_candidate_but_keeps_explicit_audio_search()
    {
        let candidate = Candidate {
            id: "youtube:stale".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=stale".into(),
            confidence: "low".into(),
            ..Candidate::default()
        };
        let imported = Track {
            provenance: serde_json::json!({
                "sourceKind":"playlist",
                "audioCandidate":candidate
            }),
            ..Track::default()
        };
        let search_track = track_for_playback_source_search(&imported);
        assert!(search_track.provenance.get("audioCandidate").is_none());

        let explicit = Track {
            provenance: serde_json::json!({
                "sourceKind":"audioSearch",
                "audioCandidate":candidate
            }),
            ..Track::default()
        };
        assert!(track_for_playback_source_search(&explicit)
            .provenance
            .get("audioCandidate")
            .is_some());
    }

    #[tokio::test]
    async fn valid_embedded_youtube_candidate_is_reused() {
        let candidate = Candidate {
            id: "youtube:QWERTY12345".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=QWERTY12345".into(),
            title: "Artist - Song".into(),
            ..Candidate::default()
        };
        let track = Track {
            provenance: serde_json::json!({
                "sourceKind":"audioSearch",
                "audioCandidate":candidate
            }),
            ..Track::default()
        };
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &Settings::default(),
            idle_cancel().1,
            10,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "youtube:QWERTY12345");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_embedded_youtube_candidate_id_falls_through_to_search() {
        let entry = serde_json::json!({
            "id":"QWERTY12345",
            "url":"https://www.youtube.com/watch?v=QWERTY12345",
            "title":"John Williams, London Symphony Orchestra - Marion's Theme / The Crate",
            "duration":128,
            "uploader":"John Williams - Topic"
        });
        let output = serde_json::json!({"entries":[entry]}).to_string();
        let (_directory, settings) = fake_tool(&format!("cat <<'JSON'\n{output}\nJSON"));
        let stale = Candidate {
            id: "youtube:ASDFGH12345".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=QWERTY12345".into(),
            title: "stale imported candidate".into(),
            ..Candidate::default()
        };
        let track = Track {
            id: "fixture:audio-search-stale-candidate".into(),
            provider: "musicbrainz".into(),
            artist: "John Williams, London Symphony Orchestra".into(),
            title: "Marion's Theme / The Crate".into(),
            duration_seconds: Some(128.0),
            provenance: serde_json::json!({
                "sourceKind":"audioSearch",
                "audioCandidate":stale
            }),
            ..Track::default()
        };
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &settings,
            idle_cancel().1,
            10,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "youtube:QWERTY12345");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn playback_search_uses_provider_alternatives_after_imported_hint() {
        let mut entries = (0..3)
            .map(|index| {
                serde_json::json!({
                    "id":format!("unrelated-{index}"),
                    "url":format!("https://www.youtube.com/watch?v=unrelated-{index}"),
                    "title":format!("Unrelated result {index}"),
                    "duration":180,
                    "uploader":"Other Artist"
                })
            })
            .collect::<Vec<_>>();
        let exact = serde_json::json!({
            "id":"XW9ycmy4CQA",
            "url":"https://www.youtube.com/watch?v=XW9ycmy4CQA",
            "title":"Stamatis Kraounakis - Poso S' Agapo",
            "duration":246,
            "uploader":"Stamatis Kraounakis"
        });
        entries.push(exact);
        entries.extend((4..11).map(|index| {
            serde_json::json!({
                "id":format!("unrelated-{index}"),
                "url":format!("https://www.youtube.com/watch?v=unrelated-{index}"),
                "title":format!("Unrelated result {index}"),
                "duration":180,
                "uploader":"Other Artist"
            })
        }));
        let output = serde_json::json!({"entries":entries}).to_string();
        let (_directory, settings) = fake_tool(&format!("cat <<'JSON'\n{output}\nJSON"));
        let stale = Candidate {
            id: "youtube:5OdVxV30Ids".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=5OdVxV30Ids".into(),
            title: "Greek title (Live) (Official Video)".into(),
            confidence: "low".into(),
            score: 0.1,
            ..Candidate::default()
        };
        let track = Track {
            id: "spotify:0Kvqc2zK0UKPTK6pAgyv1v".into(),
            provider: "spotify".into(),
            artist: "Stamatis Kraounakis".into(),
            title: "Poso S' Agapo".into(),
            duration_seconds: Some(245.48),
            provenance: serde_json::json!({
                "sourceKind":"playlist",
                "audioCandidate":stale
            }),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        assert_ne!(
            search_cache_key(&track, &providers, &settings, 3),
            playback_candidate_search_cache_key(&track, &providers, &settings, &[])
        );
        let found = find_candidates_with_cancel(
            &track_for_playback_source_search(&track),
            &providers,
            &settings,
            idle_cancel().1,
            PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT,
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert_eq!(found.len(), PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT);
        let exact = found
            .iter()
            .find(|candidate| candidate.id == "youtube:XW9ycmy4CQA")
            .expect("eligible exact source beyond initial search results");
        assert!(!found
            .iter()
            .any(|candidate| candidate.id == "youtube:unrelated-10"));
        assert!(crate::source_fallback::playback_recording_matches(
            &track, exact
        ));
    }

    #[tokio::test]
    async fn warmup_cache_includes_catalog_locator_before_foreground_hit() {
        let cache = CandidateSearchCache::default();
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let settings = Settings::default();
        let key = playback_candidate_search_cache_key(&track, &providers, &settings, &[]);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(20));
        let (_tx, cancel) = idle_cancel();
        let warmed = cache
            .get_or_search_cancelled(key.clone(), cancel.clone(), || async {
                complete_playback_candidate_search(
                    &track,
                    &providers,
                    None,
                    vec![Candidate {
                        id: "youtube:unrelated".into(),
                        title: "Unrelated video".into(),
                        ..Candidate::default()
                    }],
                    cancel.clone(),
                    deadline,
                    || async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![Candidate {
                            id: "youtube:catalog-locator".into(),
                            ..Candidate::default()
                        }])
                    },
                )
                .await
            })
            .await
            .unwrap();
        assert_eq!(warmed.len(), 2);
        let foreground = cache
            .get_or_search_cancelled_traced(
                key,
                cancel,
                crate::playback_sources::SourceTrace::new(None, None, None),
                || async {
                    panic!("complete warmup must satisfy foreground without a second search")
                },
            )
            .await
            .unwrap();
        assert_eq!(foreground[1].id, "youtube:catalog-locator");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn flat_warm_cache_is_distinct_and_foreground_completes_catalog_before_caching() {
        let cache = CandidateSearchCache::default();
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let provider = "youtube".to_owned();
        let providers = vec![provider.clone()];
        let aliases = vec!["Verified Alias".to_owned()];
        let settings = Settings::default();
        let flat_key =
            flat_playback_candidate_search_cache_key(&track, &provider, &settings, &aliases);
        let full_key = playback_candidate_search_cache_key(&track, &providers, &settings, &aliases);
        assert_ne!(flat_key, full_key);

        let (_tx, cancel) = idle_cancel();
        let flat_row = Candidate {
            id: "youtube:flat-row".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        cache
            .get_or_search_cancelled(flat_key.clone(), cancel.clone(), || async {
                Ok(vec![flat_row.clone()])
            })
            .await
            .unwrap();

        let catalog_calls = std::sync::atomic::AtomicUsize::new(0);
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(20));
        let completed = cache
            .get_or_search_cancelled(full_key.clone(), cancel.clone(), || async {
                let flat = cache
                    .get_or_search_cancelled(flat_key.clone(), cancel.clone(), || async {
                        panic!("warm flat result should be reused")
                    })
                    .await?;
                complete_playback_candidate_search(
                    &track,
                    &providers,
                    aliases.first().map(String::as_str),
                    flat,
                    cancel.clone(),
                    deadline,
                    || async {
                        catalog_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![Candidate {
                            id: "youtube:catalog-locator".into(),
                            provider: "youtube".into(),
                            ..Candidate::default()
                        }])
                    },
                )
                .await
            })
            .await
            .unwrap();
        assert_eq!(completed.len(), 2);
        assert_eq!(catalog_calls.load(Ordering::SeqCst), 1);

        let foreground = cache
            .get_or_search_cancelled(full_key, cancel, || async {
                panic!("completed foreground result should now be cached")
            })
            .await
            .unwrap();
        assert_eq!(foreground[1].id, "youtube:catalog-locator");
        assert_eq!(catalog_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn flat_batch_observer_runs_before_catalog_and_does_not_finish_search_early() {
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let (_tx, cancel) = idle_cancel();
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(20));
        let (batch_tx, mut batch_rx) = tokio::sync::oneshot::channel();
        let (catalog_tx, catalog_rx) = tokio::sync::oneshot::channel();
        let search = complete_playback_candidate_search_observed(
            &track,
            &providers,
            None,
            vec![Candidate {
                id: "youtube:flat-row".into(),
                ..Candidate::default()
            }],
            cancel,
            deadline,
            || async move {
                let _ = catalog_tx.send(());
                std::future::pending::<Result<Vec<Candidate>, String>>().await
            },
            Some(Box::new(move |batch| {
                let _ = batch_tx.send(batch);
            })),
        );
        tokio::pin!(search);
        // Poll the search while waiting for its observer. Awaiting the receiver
        // alone would leave this lazy future unpolled and falsely time out.
        let batch = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::select! {
                batch = &mut batch_rx => batch.expect("observer should deliver the flat batch"),
                _ = &mut search => panic!("search must remain pending in catalog"),
            }
        })
        .await
        .expect("flat batch should be available before catalog completion");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, "youtube:flat-row");
        tokio::time::timeout(Duration::from_millis(100), catalog_rx)
            .await
            .expect("catalog branch should start after observer delivery")
            .expect("catalog-start signal should be delivered");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut search)
                .await
                .is_err(),
            "the full search must remain pending until catalog settles"
        );
    }

    #[tokio::test]
    async fn known_duration_unverified_search_results_allow_exact_catalog_fallback() {
        let track = Track {
            artist: "John Denver".into(),
            title: "Matthew".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let (_tx, cancel) = idle_cancel();
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(20));
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let candidates = complete_playback_candidate_search(
            &track,
            &providers,
            None,
            vec![Candidate {
                provider: "youtube".into(),
                id: "youtube:ordinary-search-result".into(),
                title: "John Denver - Matthew".into(),
                duration_seconds: Some(216.0),
                confidence: "review".into(),
                permission: "unknown".into(),
                ..Candidate::default()
            }],
            cancel,
            deadline,
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![Candidate {
                    provider: "youtube".into(),
                    id: "youtube:catalog-locator".into(),
                    title: "Matthew".into(),
                    duration_seconds: Some(228.0),
                    ..Candidate::default()
                }])
            },
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[1].id, "youtube:catalog-locator");
    }

    #[tokio::test]
    async fn cancelled_catalog_completion_is_not_cached_and_foreground_can_retry() {
        let cache = CandidateSearchCache::default();
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let (tx, cancel) = watch::channel(false);
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(20));
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_callback = observed.clone();
        let result = cache
            .get_or_search_cancelled_observed_decided(
                "completion".into(),
                cancel.clone(),
                None,
                || async {
                    complete_playback_candidate_search_observed(
                        &track,
                        &providers,
                        None,
                        vec![],
                        cancel.clone(),
                        deadline,
                        || async {
                            tx.send_replace(true);
                            std::future::pending::<Result<Vec<Candidate>, String>>().await
                        },
                        Some(Box::new(move |_batch| {
                            observed_callback.store(true, Ordering::SeqCst);
                        })),
                    )
                    .await
                },
            )
            .await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert!(observed.load(Ordering::SeqCst));
        let (_tx, fresh) = idle_cancel();
        let result = cache
            .get_or_search_cancelled("completion".into(), fresh, || async {
                Ok(vec![Candidate {
                    id: "retry".into(),
                    ..Candidate::default()
                }])
            })
            .await
            .unwrap();
        assert_eq!(result[0].id, "retry");
    }

    #[test]
    fn preview_operation_deadline_uses_the_configured_startup_timeout() {
        for (configured, expected) in [(20, 20), (30, 30), (45, 45), (0, 20)] {
            let settings = Settings {
                playback_startup_timeout_seconds: configured,
                ..Settings::default()
            };
            let remaining = preview_operation_deadline(&settings).remaining();
            assert!(remaining <= Duration::from_secs(expected));
            assert!(remaining > Duration::from_secs(expected - 1));
        }
    }

    #[test]
    fn optional_catalog_research_leaves_time_for_extraction_and_access_checks() {
        assert_eq!(
            optional_playback_research_budget(Duration::from_secs(45)),
            Duration::from_secs(3)
        );
        assert_eq!(
            optional_playback_research_budget(Duration::from_secs(10)),
            Duration::from_secs(2)
        );
        assert_eq!(
            optional_playback_research_budget(Duration::from_secs(8)),
            Duration::ZERO
        );
        assert_eq!(
            optional_playback_research_budget(Duration::from_secs(1)),
            Duration::ZERO
        );
    }

    #[tokio::test]
    async fn optional_catalog_timeout_returns_flat_rows_without_caching_incomplete_search() {
        let cache = CandidateSearchCache::default();
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let (_tx, cancel) = idle_cancel();
        let deadline = crate::playback_sources::PlaybackDeadline::after(
            PLAYBACK_EXTRACTION_RESERVE + Duration::from_millis(25),
        );
        let result = cache
            .get_or_search_cancelled_observed_decided(
                "completion".into(),
                cancel.clone(),
                None,
                || async {
                    complete_playback_candidate_search_observed(
                        &track,
                        &providers,
                        None,
                        vec![Candidate {
                            id: "ordinary-search-row".into(),
                            ..Candidate::default()
                        }],
                        cancel.clone(),
                        deadline,
                        || std::future::pending::<Result<Vec<Candidate>, String>>(),
                        None,
                    )
                    .await
                },
            )
            .await
            .unwrap();
        assert_eq!(result[0].id, "ordinary-search-row");
        assert!(deadline.remaining() > Duration::from_secs(7));
        let cached = cache
            .get_or_search_cancelled("completion".into(), cancel, || async {
                Ok(vec![Candidate {
                    id: "foreground-retry".into(),
                    ..Candidate::default()
                }])
            })
            .await
            .unwrap();
        assert_eq!(cached[0].id, "foreground-retry");
    }

    #[tokio::test]
    async fn warm_budget_without_catalog_time_is_not_a_full_search_cache_hit() {
        let cache = CandidateSearchCache::default();
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let (_tx, cancel) = idle_cancel();
        let deadline =
            crate::playback_sources::PlaybackDeadline::after(PLAYBACK_EXTRACTION_RESERVE);
        let warm_key = "budgeted-completion".to_owned();
        let warmed = cache
            .get_or_search_cancelled_observed_decided(
                warm_key.clone(),
                cancel.clone(),
                None,
                || async {
                    complete_playback_candidate_search_observed(
                        &track,
                        &providers,
                        None,
                        vec![Candidate {
                            id: "flat-only".into(),
                            ..Candidate::default()
                        }],
                        cancel.clone(),
                        deadline,
                        || async { panic!("catalog must not start with zero budget") },
                        None,
                    )
                    .await
                },
            )
            .await
            .unwrap();
        assert_eq!(warmed[0].id, "flat-only");

        let retry_calls = std::sync::atomic::AtomicUsize::new(0);
        let foreground = cache
            .get_or_search_cancelled(warm_key, cancel, || async {
                retry_calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![Candidate {
                    id: "foreground-complete".into(),
                    ..Candidate::default()
                }])
            })
            .await
            .unwrap();
        assert_eq!(foreground[0].id, "foreground-complete");
        assert_eq!(retry_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn optional_catalog_does_not_start_when_extraction_reserve_is_unavailable() {
        let track = Track {
            artist: "Catalog Artist".into(),
            title: "Recording".into(),
            ..Track::default()
        };
        let providers = vec!["youtube".into()];
        let (_tx, cancel) = idle_cancel();
        let result = complete_playback_candidate_search(
            &track,
            &providers,
            None,
            vec![Candidate {
                id: "ordinary-search-row".into(),
                ..Candidate::default()
            }],
            cancel,
            crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(1)),
            || async { panic!("optional catalog must preserve extraction reserve") },
        )
        .await
        .unwrap();
        assert_eq!(result[0].id, "ordinary-search-row");
    }

    #[test]
    fn source_statistics_preserve_zero_and_distinguish_soundcloud_plays() {
        let youtube = candidate_from_json(
            &serde_json::json!({"id":"fixture","view_count":0}),
            "youtube",
        );
        assert_eq!(youtube.source_metadata.unwrap()["views"], 0);
        let soundcloud = candidate_from_json(
            &serde_json::json!({"id":"fixture","view_count":123}),
            "soundcloud",
        );
        assert_eq!(soundcloud.source_metadata.unwrap()["plays"], 123);
        let metadata = candidate_from_json(&serde_json::json!({"id":"fixture"}), "youtube")
            .source_metadata
            .unwrap();
        assert_eq!(metadata["sourceMetadataOrigin"], "ytDlpCandidateMetadata");
        assert!(metadata.get("views").is_none());
        assert!(metadata.get("plays").is_none());
    }

    #[test]
    fn public_magalenha_flat_results_use_the_existing_strict_playback_gate() {
        let track = Track {
            id: "traxsource:fixture-recording".into(),
            provider: "traxsource".into(),
            provider_id: "fixture-recording".into(),
            artist: "Simon Fava, Sergio Mendes".into(),
            title: "Magalenha".into(),
            version: Some("Extended Mix (4:27)".into()),
            duration_seconds: Some(267.0),
            ..Track::default()
        };
        // Sanitized fields from the public host-permitted flat search; rank is
        // retained, while provider IDs and URLs are intentionally omitted.
        let rows = [
            (
                "Sergio Mendes, Simon Fava - Magalenha (Extended Mix)",
                311.0,
                "Cefan's sound",
            ),
            (
                "Simon Fava feat. Sergio Mendes - Magalenha (Extended Mix)",
                267.0,
                "VeryHouseMusic",
            ),
            ("Magalenha (feat. Sergio Mendes)", 171.0, "Release - Topic"),
            (
                "Magalenha (feat. Sergio Mendes) [Extended Mix]",
                311.0,
                "Simon Fava - Topic",
            ),
            (
                "Simon Fava (Feat. Sergio Mendes) - Magalenha (Extended Mix) [TOOLROOM]",
                268.0,
                "PureHouseMusic 2024",
            ),
            (
                "Magalenha (feat. Sergio Mendes) (Extended Mix)",
                292.0,
                "Simon Fava - Topic",
            ),
            (
                "Simon Fava ft Sergio Mendes - Magalenha (Extended Mix)",
                265.0,
                "Vinyl Idol Music",
            ),
            (
                "Simon Fava & Gregor Salto feat. Sergio Mendes - Magalenha (Official Music Video)",
                215.0,
                "Spinnin' Records",
            ),
            (
                "Simon Fava, Sergio Mendes - Magalenha (Extended Mix)",
                268.0,
                "Dj Coco",
            ),
            (
                "Simon Fava & Yvvan Back – Magalenha (feat. Sergio Mendes) [Official Music Video]",
                196.0,
                "Spinnin' Records",
            ),
            (
                "Simon Fava Feat. Sergio Mendes - Magalenha (Extended Mix)",
                268.0,
                "Addicted To Music",
            ),
            (
                "Simon Fava (feat. Sergio Mendes) - Magalenha [Latin House]",
                267.0,
                "Toolroom Records",
            ),
            (
                "Simon Fava & Yvvan Back Feat. Sergio Mendes - Magalenha (Extended Mix)",
                295.0,
                "Runnin' Music",
            ),
            (
                "Sergio Mendes, Simon Fava - Magalenha (Extended Mix) [Toolroom]",
                407.0,
                "SONIQ SOCIETY",
            ),
            (
                "Sergio Mendes - Magalenha (Official Visualizer)",
                218.0,
                "The Sergio Mendes Band",
            ),
            (
                "Simon Fava, Gregor Salto Feat. Sergio Mendes - Magalenha (Extended Mix)",
                311.0,
                "Nine Music",
            ),
            (
                "Simon Fava, Yvvan Back Feat. Sergio Mendes - Magalenha (Extended Mix)",
                292.0,
                "Nine Music",
            ),
            ("Magalenha (Sunset Mix)", 203.0, "Simon Fava - Topic"),
            (
                "Magalenha (Jøsh Ramz & VANDAM Remix) [Afro House]",
                310.0,
                "AFROGON",
            ),
            ("Sergio Mendes - Magalenha (Video Original)", 217.0, "VART"),
        ];
        let observations = rows.iter().enumerate().map(|(index, (title, duration, uploader))| {
            let row = serde_json::json!({"id":format!("public-result-{}", index + 1), "title":title, "duration":duration, "uploader":uploader});
            let mut candidate = candidate_from_json(&row, "youtube");
            candidate.url = format!("https://www.youtube.com/watch?v=public-result-{}", index + 1);
            matching::score_candidate(&track, &mut candidate);
            let eligible = crate::source_fallback::playback_recording_matches(&track, &candidate)
                || crate::source_fallback::is_topic_playback_candidate(&track, &candidate)
                || crate::source_fallback::is_provisional_playback_candidate(&track, &candidate);
            (index + 1, candidate.confidence, eligible)
        }).collect::<Vec<_>>();
        let eligible_ranks = observations
            .iter()
            .filter_map(|(rank, _, eligible)| eligible.then_some(*rank))
            .collect::<Vec<_>>();
        assert_eq!(eligible_ranks, vec![9]);
        // Feat./ft variants have perfect token coverage and nearly-perfect scores,
        // but the exact contiguous artist-credit condition leaves them at review;
        // the strict matcher rejects their extra credit token. The clean credited
        // result is high confidence and is within production's ten-result cap.
        for rank in [2, 5, 7, 11] {
            assert_eq!(
                observations[rank - 1].1,
                "review",
                "public result rank {rank}"
            );
            assert!(!observations[rank - 1].2, "public result rank {rank}");
        }
        assert_eq!(observations[8].1, "high");
        assert!(observations[8].2);
        assert!(eligible_ranks[0] <= PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT);
    }

    #[test]
    fn known_duration_precision_fallback_uses_only_the_available_second_query_slot() {
        let track = Track {
            title: "Matthew".into(),
            artist: "John Denver".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        assert_eq!(
            playback_search_queries(&track, None),
            vec!["john denver matthew", "\"john denver\" \"matthew\""]
        );
        let alias = playback_search_queries(&track, Some("Verified Name"));
        assert_eq!(alias.len(), 2);
        assert_eq!(alias[1], "verified name matthew");
        let missing = Track {
            duration_seconds: None,
            ..track.clone()
        };
        assert_eq!(
            playback_search_queries(&missing, None),
            vec!["john denver matthew"]
        );
        let make = |title: &str, duration| Candidate {
            provider: "youtube".into(),
            title: title.into(),
            duration_seconds: Some(duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Candidate::default()
        };
        let early = make("John Denver - Matthew", 216.0);
        assert!(crate::source_fallback::is_provisional_playback_candidate(
            &track, &early
        ));
        assert!(!crate::source_fallback::playback_recording_matches(
            &track, &early
        ));
        let precise = make("JOHN DENVER - MATTHEW.", 225.0);
        assert!(crate::source_fallback::playback_recording_matches(
            &track, &precise
        ));
    }

    #[test]
    fn alias_query_uses_second_existing_slot_keeps_version_and_stays_within_two_calls() {
        let track = Track {
            artist: "Le Mystère Des Voix Bulgares".into(),
            title: "Duda E Bolna Legnada".into(),
            version: Some("Live".into()),
            ..Track::default()
        };
        let queries =
            playback_search_queries(&track, Some("Bulgarian State Radio Female Vocal Choir"));
        assert_eq!(queries.len(), 2);
        assert!(queries[0].contains("live"));
        assert!(queries[1].contains("bulgarian state radio female vocal choir"));
        assert!(queries[1].contains("live"));
        let fallback = playback_search_queries(&track, None);
        assert_eq!(fallback.len(), 2);
        assert!(!fallback[1].contains("live"));
        let unversioned = Track {
            version: None,
            ..track
        };
        assert_eq!(
            playback_search_queries(
                &unversioned,
                Some("Bulgarian State Radio Female Vocal Choir")
            )
            .len(),
            2
        );
    }

    #[test]
    fn download_search_uses_a_validated_cached_scoped_alias_only_as_query_text() {
        use sha2::Digest;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("download-alias.sqlite")).unwrap();
        let track = Track {
            provider: "spotify".into(),
            artist: "Lakis Chalkias".into(),
            title: "To Miroloi Tis Manas".into(),
            version: Some("Live".into()),
            ..Track::default()
        };
        let id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        let key = format!(
            "v1:{:x}",
            sha2::Sha256::digest(crate::matching::normalize_single_artist_name(&track.artist))
        );
        store
            .set_provider_cache(
                "artists.alias-discovery.scoped-name-search.v2",
                &key,
                &serde_json::json!({"outcome":"unique_exact_name_profile","artistId":id}),
                3600,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                id,
                &serde_json::json!({"id":id,"name":"Lakis Halkias","aliases":[{"name":"Lakis Chalkias"}]}),
            )
            .unwrap();

        let alternate = cached_download_search_alternate(&store, &track).unwrap();
        assert_eq!(alternate, "Lakis Halkias");
        let queries = playback_search_queries(&track, Some(&alternate));
        assert_eq!(queries.len(), 2);
        assert!(queries[0].contains("lakis chalkias"));
        assert!(queries[1].contains("lakis halkias"));
        assert!(queries[1].contains("live"));
        assert_eq!(track.artist, "Lakis Chalkias");
    }

    #[tokio::test]
    async fn download_search_returns_partial_rows_but_retries_instead_of_caching_them() {
        let cache = CandidateSearchCache::default();
        let (_cancel_tx, cancel) = idle_cancel();
        let calls = std::sync::atomic::AtomicUsize::new(0);

        let partial = cached_download_candidate_search(
            &cache,
            "download-partial-search".into(),
            cancel.clone(),
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok((
                    vec![Candidate {
                        id: "partial-row".into(),
                        ..Default::default()
                    }],
                    false,
                ))
            },
        )
        .await
        .unwrap();
        assert_eq!(partial[0].id, "partial-row");

        let complete = cached_download_candidate_search(
            &cache,
            "download-partial-search".into(),
            cancel,
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok((
                    vec![Candidate {
                        id: "complete-row".into(),
                        ..Default::default()
                    }],
                    true,
                ))
            },
        )
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(complete[0].id, "complete-row");
    }

    #[test]
    fn default_original_versions_do_not_pollute_provider_queries() {
        let mut track = Track {
            artist: "Massivedrum, Bruno Rosado".into(),
            title: "Make Me Sway".into(),
            version: Some("Original Mix (4:23)".into()),
            ..Track::default()
        };
        assert_eq!(
            search_query(&track, true),
            "massivedrum bruno rosado make me sway"
        );
        track.version = Some("Radio Edit".into());
        assert_eq!(
            search_query(&track, true),
            "massivedrum bruno rosado make me sway radio edit"
        );
    }

    #[test]
    fn captured_remix_query_does_not_repeat_title() {
        let track = Track {
            artist: "Mike Dunn, DJ Tai".into(),
            title: "(I'm Sick) I've Been Infected Wit House Musick (Remix)".into(),
            version: Some(
                "I'm Sick I've Been Infected Wit House Musick [DJ Tai Vocal Remix] (8:14)".into(),
            ),
            ..Track::default()
        };
        let query = search_query(&track, true);
        assert_eq!(query.matches("infected").count(), 1);
        assert!(query.ends_with("dj tai vocal remix"));
    }

    #[test]
    fn topic_metadata_retains_only_structured_identity_and_credit() {
        let candidate = Candidate {
            provider: "youtube".into(),
            uploader: Some("Massivedrum - Topic".into()),
            ..Candidate::default()
        };
        let info = serde_json::json!({
            "channel_id":"UCOoxO3pt54tPbTMmAj16c9A",
            "channel":"Massivedrum - Topic",
            "track":"Make Me Sway",
            "artist":"Massivedrum, Bruno Rosado",
            "artists":["Massivedrum","Bruno Rosado"],
            "duration":263.0,
            "description":"Make Me Sway · Massivedrum · Bruno Rosado\nhttps://example.invalid/private"
        });
        let metadata = topic_identity_metadata(&candidate, &info).unwrap();
        assert_eq!(metadata["track"], "Make Me Sway");
        assert_eq!(
            metadata["artists"],
            serde_json::json!(["Massivedrum", "Bruno Rosado"])
        );
        assert_eq!(
            metadata["descriptionCreditLine"],
            "Make Me Sway · Massivedrum · Bruno Rosado"
        );
        assert!(!metadata.to_string().contains("example.invalid"));
        let mut incomplete = info.clone();
        incomplete["artists"] = serde_json::json!(["Massivedrum", null]);
        assert!(topic_identity_metadata(&candidate, &incomplete).is_none());
        incomplete["artists"] =
            serde_json::json!((0..33).map(|i| format!("Artist {i}")).collect::<Vec<_>>());
        assert!(topic_identity_metadata(&candidate, &incomplete).is_none());
    }

    #[test]
    fn fresh_topic_metadata_supports_remembered_candidate_without_uploader_hint() {
        let candidate = Candidate {
            id: "youtube:PIJ6QOUnuCg".into(),
            provider: "youtube".into(),
            uploader: None,
            ..Candidate::default()
        };
        let info = serde_json::json!({
            "id":"PIJ6QOUnuCg",
            "url":"https://rr1---sn.googlevideo.com/videoplayback?signature=redacted",
            "ext":"webm","protocol":"https","format_id":"251",
            "acodec":"opus","vcodec":"none","duration":413.0,
            "channel_id":"UCGCTe_N5qNs1TmXl3foMRww",
            "channel":"Domna Samiou - Topic",
            "uploader":"Domna Samiou - Topic",
            "artists":["Domna Samiou"],
            "artist":"Domna Samiou",
            "track":"Μοιρολόι (Ήπειρος)",
            "album":"Ιστορικά - Κλέφτικα Τραγούδια",
            "description":"Μοιρολόι (Ήπειρος) · Domna Samiou\nΙστορικά - Κλέφτικα Τραγούδια"
        });
        let audio = preview_audio_from_metadata(&candidate, &info).unwrap();
        let metadata = audio.source_metadata.unwrap();
        assert_eq!(metadata["channelId"], "UCGCTe_N5qNs1TmXl3foMRww");
        assert_eq!(metadata["channel"], "Domna Samiou - Topic");
        assert_eq!(metadata["track"], "Μοιρολόι (Ήπειρος)");
        assert_eq!(metadata["artists"], serde_json::json!(["Domna Samiou"]));
        assert_eq!(metadata["duration"], 413.0);
        assert_eq!(
            metadata["descriptionCreditLine"],
            "Μοιρολόι (Ήπειρος) · Domna Samiou"
        );
    }

    #[test]
    fn public_moiroloi_topic_fixture_passes_fresh_verified_extractor_gate() {
        // Allowlisted public yt-dlp metadata observed 2026-10-19 for
        // https://www.youtube.com/watch?v=PIJ6QOUnuCg. This fixture contains
        // no media URL, request headers, or signed query values.
        let public_metadata = serde_json::json!({
            "id":"PIJ6QOUnuCg",
            "title":"Μοιρολόι (Ήπειρος)",
            "channel_id":"UCGCTe_N5qNs1TmXl3foMRww",
            "channel":"Domna Samiou - Topic",
            "uploader":"Domna Samiou - Topic",
            "artists":["Domna Samiou"],
            "artist":"Domna Samiou",
            "track":"Μοιρολόι (Ήπειρος)",
            "album":"Ιστορικά - Κλέφτικα Τραγούδια",
            "duration":413.0,
            "description":"Provided to YouTube by DistroKid\nΜοιρολόι (Ήπειρος) · Domna Samiou\nΙστορικά - Κλέφτικα Τραγούδια\n℗ Domna Samiou Greek Folk Music Association\nAuto-generated by YouTube."
        });
        // Preview parsing needs a selected audio format. These transport fields
        // are synthetic parser inputs and are not part of the public fixture.
        let mut extractor_info = public_metadata;
        extractor_info["url"] = serde_json::json!("https://rr1---sn.googlevideo.com/videoplayback");
        extractor_info["ext"] = serde_json::json!("webm");
        extractor_info["protocol"] = serde_json::json!("https");
        extractor_info["format_id"] = serde_json::json!("251");
        extractor_info["acodec"] = serde_json::json!("opus");
        extractor_info["vcodec"] = serde_json::json!("none");

        let track = Track {
            id: "spotify:4MiDHSmsHlgaf8W2tzwT6Z".into(),
            provider: "spotify".into(),
            provider_id: "4MiDHSmsHlgaf8W2tzwT6Z".into(),
            artist: "Domna Samiou".into(),
            title: "Μοιρολόι (Ήπειρος)".into(),
            duration_seconds: Some(413.32),
            ..Track::default()
        };
        let mut candidate = Candidate {
            id: "youtube:PIJ6QOUnuCg".into(),
            provider: "youtube".into(),
            title: "Μοιρολόι (Ήπειρος)".into(),
            uploader: None,
            permission: "unknown".into(),
            confidence: "review".into(),
            ..Candidate::default()
        };
        let audio = preview_audio_from_metadata(&candidate, &extractor_info).unwrap();

        // Mirror verify_resolved_identity's fresh extraction updates before
        // calling the real verified-extractor recording matcher.
        candidate.source_metadata = audio.source_metadata.clone();
        candidate.uploader = audio.source_metadata.as_ref().and_then(|metadata| {
            metadata["channel"]
                .as_str()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    metadata["uploader"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                })
                .map(str::to_owned)
        });
        candidate.title = audio.resolved_title.clone().unwrap();
        candidate.duration_seconds = audio.duration_seconds;
        candidate.confidence = "review".into();
        assert!(
            crate::source_fallback::playback_recording_matches_verified_extractor(
                &track, &candidate
            )
        );

        let wrong_artist = Track {
            artist: "Wrong Artist".into(),
            ..track.clone()
        };
        assert!(
            !crate::source_fallback::playback_recording_matches_verified_extractor(
                &wrong_artist,
                &candidate
            )
        );
        let wrong_duration = Track {
            duration_seconds: Some(420.0),
            ..track
        };
        assert!(
            !crate::source_fallback::playback_recording_matches_verified_extractor(
                &wrong_duration,
                &candidate
            )
        );
        assert!(candidate
            .source_metadata
            .as_ref()
            .unwrap()
            .get("channelIsVerified")
            .is_none());
    }

    #[test]
    fn topic_proof_uses_fresh_channel_fields_not_candidate_hints() {
        let stale_topic = Candidate {
            id: "youtube:PIJ6QOUnuCg".into(),
            provider: "youtube".into(),
            uploader: Some("Wrong Artist - Topic".into()),
            ..Candidate::default()
        };
        let fresh_non_topic = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "channel":"Ordinary Channel",
            "uploader":"Ordinary Channel",
            "channel_is_verified":false,
            "artists":["Wrong Artist"],
            "track":"Some Song",
            "duration":225.0,
            "description":"Unrelated"
        });
        assert!(!fresh_youtube_topic_identity(&fresh_non_topic));
        assert!(topic_identity_metadata(&stale_topic, &fresh_non_topic).is_none());
        assert!(verified_youtube_channel_metadata(&stale_topic, &fresh_non_topic).is_none());

        // A Topic uploader alone cannot stand in for missing fresh channel
        // metadata, even when the search candidate carried the same hint.
        let missing_channel = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "uploader":"Right Artist - Topic",
            "artists":["Right Artist"],
            "track":"Some Song",
            "duration":225.0,
            "description":"Some Song · Right Artist"
        });
        assert!(topic_identity_metadata(&stale_topic, &missing_channel).is_none());

        let fresh_topic = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "channel":"Right Artist - Topic",
            "uploader":"Right Artist - Topic",
            "artists":["Right Artist"],
            "track":"Some Song",
            "duration":225.0,
            "description":"Some Song · Right Artist"
        });
        assert!(topic_identity_metadata(&stale_topic, &fresh_topic).is_some());
        let mut bad_channel_id = fresh_topic.clone();
        bad_channel_id["channel_id"] = serde_json::json!("UC-short");
        assert!(topic_identity_metadata(&stale_topic, &bad_channel_id).is_none());
        let mut missing_duration = fresh_topic.clone();
        missing_duration.as_object_mut().unwrap().remove("duration");
        assert!(topic_identity_metadata(&stale_topic, &missing_duration).is_none());
        let mut missing_credit = fresh_topic;
        missing_credit.as_object_mut().unwrap().remove("artists");
        assert!(topic_identity_metadata(&stale_topic, &missing_credit).is_none());
    }

    #[test]
    fn ordinary_youtube_identity_requires_fresh_verified_channel_fields() {
        let candidate = Candidate {
            provider: "youtube".into(),
            uploader: Some("KAROL G".into()),
            ..Candidate::default()
        };
        let info = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "channel":"KAROL G",
            "uploader":"KAROL G",
            "channel_is_verified":true,
            "track":"BbY WOW",
            "artists":["KAROL G","Judeline","rusowsky"],
            "description":"untrusted arbitrary description",
        });
        let metadata = verified_youtube_channel_metadata(&candidate, &info).unwrap();
        assert_eq!(metadata["provider"], "youtube");
        assert_eq!(metadata["channelId"], "UC1234567890123456789012");
        assert_eq!(metadata["channelIsVerified"], true);
        assert_eq!(metadata["channel"], "KAROL G");
        assert_eq!(metadata["track"], "BbY WOW");
        assert_eq!(
            metadata["artists"],
            serde_json::json!(["KAROL G", "Judeline", "rusowsky"])
        );
        assert!(metadata.get("description").is_none());

        let unverified = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "channel":"KAROL G",
            "channel_is_verified":false,
        });
        assert!(verified_youtube_channel_metadata(&candidate, &unverified).is_none());
        let missing_id = serde_json::json!({
            "channel":"KAROL G",
            "channel_is_verified":true,
        });
        assert!(verified_youtube_channel_metadata(&candidate, &missing_id).is_none());

        let topic = Candidate {
            uploader: Some("KAROL G - Topic".into()),
            ..candidate
        };
        // A stale Topic search label does not select Topic proof; a fresh
        // verified non-Topic extractor response may still use its own proof.
        assert_eq!(
            verified_youtube_channel_metadata(&topic, &info).unwrap()["channelIsVerified"],
            true
        );
    }

    #[test]
    fn unverified_youtube_channel_id_is_retained_only_as_an_equality_fact() {
        let ordinary = Candidate {
            id: "youtube:hghqd1eBTYQ".into(),
            provider: "youtube".into(),
            uploader: Some("Unverified Uploader".into()),
            ..Candidate::default()
        };
        let unverified = serde_json::json!({
            "id":"hghqd1eBTYQ",
            "url":"https://rr5---sn.googlevideo.com/videoplayback?signature=redacted",
            "ext":"webm","protocol":"https","format_id":"251",
            "acodec":"opus","vcodec":"none","duration":225.0,
            "channel_id":"UC1234567890123456789012",
            "channel":"Unverified Uploader","uploader":"Unverified Uploader",
            "channel_is_verified":false,"artists":["Wrong Artist"],
        });
        let audio = preview_audio_from_metadata(&ordinary, &unverified).unwrap();
        assert_eq!(
            audio.source_metadata,
            Some(serde_json::json!({
                "provider":"youtube",
                "channelId":"UC1234567890123456789012"
            }))
        );
        let metadata = audio.source_metadata.unwrap();
        assert!(metadata.get("channelIsVerified").is_none());
        assert!(metadata.get("channel").is_none());
        assert!(metadata.get("uploader").is_none());
        assert!(metadata.get("artists").is_none());
        assert!(verified_youtube_channel_metadata(&ordinary, &unverified).is_none());

        let uploader_only = serde_json::json!({
            "id":"hghqd1eBTYQ","uploader":"Unverified Uploader"
        });
        assert!(youtube_identity_only_metadata(&ordinary, &uploader_only).is_none());
        let malformed_id = serde_json::json!({
            "channel_id":"UC-short"
        });
        assert!(youtube_identity_only_metadata(&ordinary, &malformed_id).is_none());

        // Wrong Topic credits and an unverified catalog channel stay without
        // their richer proof facts; the identity-only field adds no credit or
        // verified-channel signal.
        let topic = Candidate {
            uploader: Some("Right Artist - Topic".into()),
            ..ordinary.clone()
        };
        let wrong_topic = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "artists":["Wrong Artist"],
            "track":"Some Song",
            "description":"Unrelated description"
        });
        assert!(topic_identity_metadata(&topic, &wrong_topic).is_none());
        assert_eq!(
            youtube_identity_only_metadata(&topic, &wrong_topic),
            Some(serde_json::json!({
                "provider":"youtube",
                "channelId":"UC1234567890123456789012"
            }))
        );

        let catalog = Candidate {
            source_metadata: Some(serde_json::json!({
                "provider":"youtube","playbackMusicCatalogCandidate":true
            })),
            ..ordinary
        };
        let catalog_unverified = serde_json::json!({
            "channel_id":"UC1234567890123456789012",
            "channel_is_verified":false,
        });
        assert!(verified_youtube_channel_metadata(&catalog, &catalog_unverified).is_none());
        let identity = youtube_identity_only_metadata(&catalog, &catalog_unverified).unwrap();
        assert!(identity.get("channelIsVerified").is_none());
        assert!(identity.get("artists").is_none());

        let title_only = Candidate {
            id: "youtube:uZGRATckoBM".into(),
            provider: "youtube".into(),
            title: "Matthew".into(),
            duration_seconds: Some(228.0),
            confidence: "review".into(),
            permission: "unknown".into(),
            source_metadata: Some(identity),
            ..Candidate::default()
        };
        let matthew = Track {
            title: "Matthew".into(),
            artist: "John Denver".into(),
            duration_seconds: Some(222.426),
            ..Track::default()
        };
        assert!(
            !crate::source_fallback::playback_recording_matches_verified_extractor(
                &matthew,
                &title_only
            )
        );
        assert!(
            !crate::source_fallback::playback_music_catalog_recording_matches_verified_extractor(
                &matthew,
                &title_only
            )
        );

        // Equal duration still cannot promote the identity-only channel ID to
        // an artist credit for either strong matching path.
        let exact_duration_track = Track {
            duration_seconds: Some(228.0),
            ..matthew
        };
        assert!(
            !crate::source_fallback::playback_recording_matches_verified_extractor(
                &exact_duration_track,
                &title_only
            )
        );
        assert!(
            !crate::source_fallback::playback_music_catalog_recording_matches_verified_extractor(
                &exact_duration_track,
                &title_only
            )
        );
    }

    #[test]
    fn managed_provider_runtime_replaces_configured_runtime_with_bundled_node() {
        let mut args = vec![
            "--no-js-runtimes".to_owned(),
            "--js-runtimes".to_owned(),
            "deno:/configured/deno".to_owned(),
        ];
        append_managed_youtube_provider_args(
            &mut args,
            vec![
                "--plugin-dirs".into(),
                "/trusted/bgutil/plugin".into(),
                "--js-runtimes".into(),
                "node:/bundled/node".into(),
            ],
        );

        assert_eq!(
            args,
            [
                "--no-js-runtimes",
                "--js-runtimes",
                "deno:/configured/deno",
                "--no-js-runtimes",
                "--plugin-dirs",
                "/trusted/bgutil/plugin",
                "--js-runtimes",
                "node:/bundled/node",
            ]
        );

        let mut without_runtime = vec!["--js-runtimes".to_owned(), "deno:/configured/deno".into()];
        append_managed_youtube_provider_args(&mut without_runtime, vec!["--plugin-dirs".into()]);
        assert_eq!(
            without_runtime,
            ["--js-runtimes", "deno:/configured/deno", "--plugin-dirs"]
        );
    }

    #[test]
    fn inspection_cache_key_tracks_source_and_effective_tool_profile() {
        let mut candidate = Candidate {
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=a".into(),
            ..Default::default()
        };
        let settings = Settings::default();
        let original = inspection_cache_key(&candidate, &settings);
        candidate.url = "https://youtube.com/watch?v=b".into();
        assert_ne!(original, inspection_cache_key(&candidate, &settings));
        candidate.url = "https://youtube.com/watch?v=a".into();
        let mut changed = settings.clone();
        changed.js_runtime_path = "deno".into();
        assert_ne!(original, inspection_cache_key(&candidate, &changed));
    }

    #[test]
    fn preview_audio_cache_separates_default_and_alternate_format_urls() {
        let candidate = Candidate {
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=verified".into(),
            ..Candidate::default()
        };
        let settings = Settings::default();
        assert_ne!(
            preview_audio_cache_key(&candidate, &settings, false),
            preview_audio_cache_key(&candidate, &settings, true),
        );
    }

    #[test]
    fn matching_policy_scopes_search_but_not_stream_transport_cache() {
        let candidate = Candidate {
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=verified".into(),
            ..Default::default()
        };
        let track = Track {
            title: "Recording".into(),
            artist: "Performer".into(),
            ..Default::default()
        };
        let balanced = Settings::default();
        let mut flexible = balanced.clone();
        flexible.matching_level = crate::recording_match::MatchLevel::Flexible;
        assert_eq!(
            preview_audio_cache_key(&candidate, &balanced, false),
            preview_audio_cache_key(&candidate, &flexible, false)
        );
        assert_ne!(
            playback_candidate_search_cache_key(&track, &["youtube".into()], &balanced, &[]),
            playback_candidate_search_cache_key(&track, &["youtube".into()], &flexible, &[])
        );
    }

    #[tokio::test]
    async fn raw_provider_query_cache_is_mode_independent_but_reassesses_rows() {
        let cache = CandidateSearchCache::default();
        let balanced = Settings::default();
        let mut flexible = balanced.clone();
        flexible.matching_level = crate::recording_match::MatchLevel::Flexible;
        let key_balanced = playback_provider_query_cache_key(
            "youtube",
            "ytsearch10",
            "Performer Recording",
            10,
            &balanced,
            true,
        );
        let key_flexible = playback_provider_query_cache_key(
            "youtube",
            "ytsearch10",
            "Performer Recording",
            10,
            &flexible,
            true,
        );
        assert_eq!(key_balanced, key_flexible);

        let (_tx, cancel) = idle_cancel();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let row = Candidate {
            id: "youtube:raw-row".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=raw-row".into(),
            title: "Main Artist - Song".into(),
            duration_seconds: Some(200.0),
            source_metadata: Some(serde_json::json!({
                "provider":"youtube",
                "track":"Song",
                "artists":null,
                "artistNames":null,
                "duration":200.0
            })),
            ..Candidate::default()
        };
        let first = cache
            .get_or_search_provider_query(key_balanced, cancel.clone(), || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderQueryResponse {
                    candidates: vec![row.clone()],
                    completeness: ProviderQueryCompleteness::ReturnedPage {
                        row_count: 1,
                        reached_limit: false,
                    },
                })
            })
            .await
            .unwrap();
        let second = cache
            .get_or_search_provider_query(key_flexible, cancel, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderQueryResponse {
                    candidates: vec![Candidate::default()],
                    completeness: ProviderQueryCompleteness::ReturnedPage {
                        row_count: 1,
                        reached_limit: false,
                    },
                })
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.candidates[0].id, second.candidates[0].id);
        assert_eq!(
            second.completeness,
            ProviderQueryCompleteness::ReturnedPage {
                row_count: 1,
                reached_limit: false,
            }
        );

        let track = Track {
            artist: "Main Artist".into(),
            title: "Song".into(),
            duration_seconds: None,
            version: Some("Live Remix".into()),
            ..Track::default()
        };
        let exact = crate::recording_match::assess(
            &track,
            &first.candidates[0],
            crate::recording_match::MatchLevel::Exact,
            crate::recording_match::MatchStage::Resolved,
        );
        let permissive = crate::recording_match::assess(
            &track,
            &second.candidates[0],
            crate::recording_match::MatchLevel::Flexible,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_ne!(exact.action, crate::recording_match::MatchAction::Accept);
        assert_eq!(
            permissive.action,
            crate::recording_match::MatchAction::Accept
        );
    }

    #[test]
    fn raw_provider_query_key_isolated_by_provider_profile_and_tool_context() {
        let settings = Settings::default();
        let key = playback_provider_query_cache_key(
            "youtube",
            "ytsearch10",
            "Performer Recording",
            10,
            &settings,
            false,
        );
        let native = playback_provider_query_cache_key(
            "youtube",
            "ytsearch10",
            "Performer Recording",
            10,
            &settings,
            true,
        );
        let other_query = playback_provider_query_cache_key(
            "youtube",
            "ytsearch10",
            "Performer Recording version",
            10,
            &settings,
            false,
        );
        let other_provider = playback_provider_query_cache_key(
            "soundcloud",
            "scsearch10",
            "Performer Recording",
            10,
            &settings,
            false,
        );
        let mut other_tool = settings.clone();
        other_tool.yt_dlp_path = "custom-ytdlp".into();
        let different_tool = playback_provider_query_cache_key(
            "youtube",
            "ytsearch10",
            "Performer Recording",
            10,
            &other_tool,
            false,
        );
        let monochrome = monochrome_playback_query_cache_key(
            r#"["monochrome-track-search-v1","https://tracks.monochrome.st","/search/tracks",12]"#,
            "Performer Recording",
        );
        let other_monochrome_profile = monochrome_playback_query_cache_key(
            r#"["monochrome-track-search-v2","https://tracks.monochrome.st","/search/tracks",12]"#,
            "Performer Recording",
        );
        assert_ne!(key, native);
        assert_ne!(key, other_query);
        assert_ne!(key, other_provider);
        assert_ne!(key, different_tool);
        assert_ne!(monochrome, other_monochrome_profile);
        let mono_key_value: Value = serde_json::from_str(&monochrome).unwrap();
        assert_eq!(mono_key_value[4], crate::monochrome::CANDIDATE_SEARCH_LIMIT);
        assert_eq!(mono_key_value[5], "anonymous-session");
        assert!(ProviderQueryCompleteness::Exhaustive { row_count: 1 }.cacheable());
    }

    #[tokio::test]
    async fn partial_provider_query_rows_are_not_cached_as_complete() {
        let cache = CandidateSearchCache::default();
        let (_tx, cancel) = idle_cancel();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let partial = cache
            .get_or_search_provider_query(
                "partial-provider-response".into(),
                cancel.clone(),
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(ProviderQueryResponse {
                        candidates: vec![Candidate {
                            id: "partial".into(),
                            ..Default::default()
                        }],
                        completeness: ProviderQueryCompleteness::Partial { row_count: 1 },
                    })
                },
            )
            .await
            .unwrap();
        assert_eq!(partial.candidates[0].id, "partial");
        assert!(!partial.completeness.cacheable());
        let complete = cache
            .get_or_search_provider_query("partial-provider-response".into(), cancel, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderQueryResponse {
                    candidates: vec![Candidate {
                        id: "complete".into(),
                        ..Default::default()
                    }],
                    completeness: ProviderQueryCompleteness::ReturnedPage {
                        row_count: 1,
                        reached_limit: false,
                    },
                })
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(complete.candidates[0].id, "complete");
    }

    #[tokio::test]
    async fn successful_empty_provider_page_is_cached_as_a_negative_result() {
        let cache = CandidateSearchCache::default();
        let (_tx, cancel) = idle_cancel();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let empty = cache
            .get_or_search_provider_query(
                "empty-successful-page".into(),
                cancel.clone(),
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(ProviderQueryResponse {
                        candidates: Vec::new(),
                        completeness: ProviderQueryCompleteness::ReturnedPage {
                            row_count: 0,
                            reached_limit: false,
                        },
                    })
                },
            )
            .await
            .unwrap();
        assert!(empty.candidates.is_empty());
        assert!(empty.completeness.cacheable());

        let cached = cache
            .get_or_search_provider_query("empty-successful-page".into(), cancel, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("a valid empty provider page should be cached")
            })
            .await
            .unwrap();
        assert!(cached.candidates.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn incomplete_candidate_search_is_returned_but_not_cached() {
        let cache = CandidateSearchCache::default();
        let (_tx, cancel) = idle_cancel();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let first = cache
            .get_or_search_cancelled_observed_decided_with_completeness(
                "partial-search-key".into(),
                cancel.clone(),
                None,
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok((
                        vec![Candidate {
                            id: "partial".into(),
                            ..Default::default()
                        }],
                        false,
                    ))
                },
            )
            .await
            .unwrap();
        assert_eq!(first.0.len(), 1);
        assert!(!first.1);
        let second = cache
            .get_or_search_cancelled_observed_decided_with_completeness(
                "partial-search-key".into(),
                cancel,
                None,
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok((
                        vec![Candidate {
                            id: "complete".into(),
                            ..Default::default()
                        }],
                        true,
                    ))
                },
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(second.0[0].id, "complete");
        assert!(second.1);
    }

    #[tokio::test]
    async fn cancelled_provider_query_waiter_does_not_take_over_or_poison_owner() {
        let cache = std::sync::Arc::new(CandidateSearchCache::default());
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let owner_cache = cache.clone();
        let owner_entered = entered.clone();
        let owner_release = release.clone();
        let owner = tokio::spawn(async move {
            let (_tx, cancel) = idle_cancel();
            owner_cache
                .get_or_search_provider_query("same-query".into(), cancel, || async move {
                    owner_entered.notify_one();
                    owner_release.notified().await;
                    Ok(ProviderQueryResponse {
                        candidates: vec![Candidate {
                            id: "owner".into(),
                            ..Default::default()
                        }],
                        completeness: ProviderQueryCompleteness::ReturnedPage {
                            row_count: 1,
                            reached_limit: false,
                        },
                    })
                })
                .await
        });
        entered.notified().await;

        let (cancel_tx, cancel) = watch::channel(false);
        let waiter_cache = cache.clone();
        let waiter = tokio::spawn(async move {
            waiter_cache
                .get_or_search_provider_query("same-query".into(), cancel, || async {
                    panic!("a waiter must not become a second query owner")
                })
                .await
        });
        tokio::task::yield_now().await;
        cancel_tx.send(true).unwrap();
        assert_eq!(waiter.await.unwrap().unwrap_err(), "Cancelled");

        release.notify_one();
        assert_eq!(owner.await.unwrap().unwrap().candidates[0].id, "owner");
        let (_tx, cancel) = idle_cancel();
        let cached = cache
            .get_or_search_provider_query("same-query".into(), cancel, || async {
                panic!("completed owner response should be cached")
            })
            .await
            .unwrap();
        assert_eq!(cached.candidates[0].id, "owner");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preview_audio_worker_and_joiner_survive_closed_false_channels() {
        let cache = PreviewAudioCache::default();
        let (worker_cancel_tx, worker_cancel_rx) = watch::channel(false);
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let worker_release = release.clone();
        let mut worker = Box::pin(cache.get_or_resolve_cancelled(
            "closed-channel-test-key".into(),
            worker_cancel_rx,
            None,
            move |_| async move {
                worker_release.notified().await;
                Ok(PreviewAudio {
                    provider_session: None,
                    url: "https://cdn.example/audio".into(),
                    mime_type: "audio/mp4".into(),
                    codec: Some("aac".into()),
                    bitrate_kbps: None,
                    duration_seconds: None,
                    selected_format_id: None,
                    selected_format_has_video: false,
                    source_metadata: None,
                    resolved_title: None,
                    resolved_provider_id: None,
                    chapters: None,
                    expected_content_length: None,
                    headers: vec![],
                })
            },
        ));

        // The worker has acquired the flight gate and is blocked in its resolver.
        // Closing false must leave that work pending rather than cancel it.
        drop(worker_cancel_tx);
        assert!(
            poll_once(&mut worker).is_pending(),
            "closed-false worker remains pending"
        );

        let (joiner_cancel_tx, joiner_cancel_rx) = watch::channel(false);
        let mut joiner = Box::pin(cache.get_or_resolve_cancelled(
            "closed-channel-test-key".into(),
            joiner_cancel_rx,
            None,
            |_| async { Err("joiner must use worker result".into()) },
        ));
        // The worker holds the gate. A closed-false joiner must remain queued.
        drop(joiner_cancel_tx);
        assert!(
            poll_once(&mut joiner).is_pending(),
            "closed-false joiner remains pending"
        );

        release.notify_one();
        let worker_audio = worker.await.expect("closed-false worker continues");
        let joiner_audio = joiner.await.expect("closed-false joiner continues");
        assert_eq!(worker_audio.url, joiner_audio.url);
    }

    #[tokio::test]
    async fn preview_audio_joiner_cancellation_keeps_worker_identity_and_worker_alive() {
        let cache = std::sync::Arc::new(PreviewAudioCache::default());
        let (worker_cancel_tx, worker_cancel_rx) = watch::channel(false);
        let (worker_id_tx, worker_id_rx) = tokio::sync::oneshot::channel();
        let worker_release = std::sync::Arc::new(tokio::sync::Notify::new());
        let worker = {
            let cache = cache.clone();
            let worker_release = worker_release.clone();
            tokio::spawn(async move {
                cache
                    .get_or_resolve_cancelled(
                        "internal-test-key".into(),
                        worker_cancel_rx,
                        None,
                        move |worker_id| async move {
                            let _ = worker_id_tx.send(worker_id);
                            worker_release.notified().await;
                            Ok(PreviewAudio {
                                provider_session: None,
                                url: "https://cdn.example/audio".into(),
                                mime_type: "audio/mp4".into(),
                                codec: Some("aac".into()),
                                bitrate_kbps: None,
                                duration_seconds: None,
                                selected_format_id: None,
                                selected_format_has_video: false,
                                source_metadata: None,
                                resolved_title: None,
                                resolved_provider_id: None,
                                chapters: None,
                                expected_content_length: None,
                                headers: vec![],
                            })
                        },
                    )
                    .await
            })
        };
        let worker_id = worker_id_rx.await.unwrap();
        assert_eq!(
            cache
                .in_flight
                .lock()
                .unwrap()
                .get("internal-test-key")
                .unwrap()
                .trace_id,
            worker_id,
        );
        let worker_trace = crate::playback_sources::SourceTrace::new(None, None, None);
        assert_eq!(
            worker_trace.with_shared_lookup(worker_id).payload(
                "lookup",
                "youtube",
                serde_json::json!({})
            )["sharedLookupId"],
            worker_id.to_string(),
        );

        let (joiner_cancel_tx, joiner_cancel_rx) = watch::channel(false);
        let (joiner_invoked_tx, mut joiner_invoked_rx) = tokio::sync::oneshot::channel();
        let joiner = {
            let cache = cache.clone();
            tokio::spawn(async move {
                cache
                    .get_or_resolve_cancelled(
                        "internal-test-key".into(),
                        joiner_cancel_rx,
                        None,
                        move |_| async move {
                            let _ = joiner_invoked_tx.send(());
                            Err("joiner should not start a second worker".into())
                        },
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let joined = cache
                    .in_flight
                    .lock()
                    .unwrap()
                    .get("internal-test-key")
                    .is_some_and(|flight| Arc::strong_count(flight) >= 3);
                if joined {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second caller joined the shared worker");
        joiner_cancel_tx.send_replace(true);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), joiner)
                .await
                .expect("cancelled caller completed")
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        assert!(joiner_invoked_rx.try_recv().is_err());
        assert_eq!(
            cache
                .in_flight
                .lock()
                .unwrap()
                .get("internal-test-key")
                .unwrap()
                .trace_id,
            worker_id
        );
        worker_release.notify_one();
        assert!(tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .expect("uncancelled worker completed")
            .unwrap()
            .is_ok());
        worker_cancel_tx.send_replace(true);
    }

    #[tokio::test]
    async fn coalesced_lookup_short_deadline_does_not_expire_later_caller() {
        let cache = Arc::new(PreviewAudioCache::default());
        let (first_cancel_tx, first_cancel_rx) = watch::channel(false);
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_started_in_resolver = first_started.clone();
        let first_cache = cache.clone();
        let first = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(60);
            tokio::time::timeout_at(
                deadline,
                first_cache.get_or_resolve_cancelled(
                    "caller-specific-deadline".into(),
                    first_cancel_rx,
                    None,
                    move |_| async move {
                        first_started_in_resolver.notify_one();
                        std::future::pending::<Result<PreviewAudio, String>>().await
                    },
                ),
            )
            .await
        });
        first_started.notified().await;

        let (second_started_tx, second_started_rx) = tokio::sync::oneshot::channel();
        let second_cache = cache.clone();
        let second = tokio::spawn(async move {
            second_cache
                .get_or_resolve_cancelled(
                    "caller-specific-deadline".into(),
                    watch::channel(false).1,
                    None,
                    move |_| async move {
                        let _ = second_started_tx.send(());
                        Ok(PreviewAudio {
                            provider_session: None,
                            url: "https://cdn.example/second-caller-audio".into(),
                            mime_type: "audio/mp4".into(),
                            codec: Some("aac".into()),
                            bitrate_kbps: None,
                            duration_seconds: Some(120.0),
                            selected_format_id: None,
                            selected_format_has_video: false,
                            source_metadata: None,
                            resolved_title: None,
                            resolved_provider_id: None,
                            chapters: None,
                            expected_content_length: None,
                            headers: vec![],
                        })
                    },
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(2), second_started_rx)
            .await
            .expect("later caller takes over after the short caller expires")
            .expect("second resolver started");
        assert!(
            first.await.unwrap().is_err(),
            "first caller hits its own deadline"
        );
        let second_audio = tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .expect("later caller is not bounded by first caller's deadline")
            .unwrap()
            .expect("later caller resolves successfully");
        assert_eq!(second_audio.url, "https://cdn.example/second-caller-audio");
        drop(first_cancel_tx);
    }

    #[tokio::test]
    async fn cached_audio_url_and_format_metadata_stay_from_the_same_resolution() {
        fn audio(
            url: &str,
            mime_type: &str,
            codec: &str,
            format_id: &str,
            has_video: bool,
            expected_bytes: u64,
        ) -> PreviewAudio {
            PreviewAudio {
                provider_session: None,
                url: url.into(),
                mime_type: mime_type.into(),
                codec: Some(codec.into()),
                bitrate_kbps: Some(128.0),
                duration_seconds: Some(250.0),
                selected_format_id: Some(format_id.into()),
                selected_format_has_video: has_video,
                source_metadata: None,
                resolved_title: Some("Poso S’ Agapo".into()),
                resolved_provider_id: Some("hRNx674Z4QE".into()),
                chapters: Some(vec![]),
                expected_content_length: Some(expected_bytes),
                headers: vec![],
            }
        }

        let candidate = Candidate {
            id: "youtube:hRNx674Z4QE".into(),
            provider: "youtube".into(),
            url: "https://youtube.com/watch?v=hRNx674Z4QE".into(),
            ..Candidate::default()
        };
        let settings = Settings::default();
        let cache = PreviewAudioCache::default();
        let default_key = preview_audio_cache_key(&candidate, &settings, false);
        let alternate_key = preview_audio_cache_key(&candidate, &settings, true);

        let default = cache
            .get_or_resolve(default_key.clone(), || async {
                Ok(audio(
                    "https://rr1.googlevideo.com/audio140?sig=redacted",
                    "audio/mp4",
                    "mp4a.40.2",
                    "140",
                    false,
                    4_048_489,
                ))
            })
            .await
            .unwrap();
        let alternate = cache
            .get_or_resolve(alternate_key.clone(), || async {
                Ok(audio(
                    "https://rr1.googlevideo.com/muxed18?sig=redacted",
                    "video/mp4",
                    "mp4a.40.2",
                    "18",
                    true,
                    10_032_375,
                ))
            })
            .await
            .unwrap();

        assert_eq!(default.selected_format_id.as_deref(), Some("140"));
        assert!(!default.selected_format_has_video);
        assert_eq!(default.mime_type, "audio/mp4");
        assert_eq!(default.expected_content_length, Some(4_048_489));
        assert_eq!(alternate.selected_format_id.as_deref(), Some("18"));
        assert!(alternate.selected_format_has_video);
        assert_eq!(alternate.mime_type, "video/mp4");
        assert_eq!(alternate.expected_content_length, Some(10_032_375));

        let cached_default = cache
            .get_or_resolve(default_key, || async {
                Err::<PreviewAudio, _>("default format cache miss".into())
            })
            .await
            .unwrap();
        let cached_alternate = cache
            .get_or_resolve(alternate_key, || async {
                Err::<PreviewAudio, _>("alternate format cache miss".into())
            })
            .await
            .unwrap();
        assert_eq!(cached_default.url, default.url);
        assert_eq!(
            cached_default.selected_format_id,
            default.selected_format_id
        );
        assert_eq!(
            cached_default.expected_content_length,
            default.expected_content_length
        );
        assert_eq!(cached_alternate.url, alternate.url);
        assert_eq!(
            cached_alternate.selected_format_id,
            alternate.selected_format_id
        );
        assert_eq!(
            cached_alternate.expected_content_length,
            alternate.expected_content_length
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cached_inspection_is_singleflight_and_rescored_for_each_track() {
        use std::os::unix::fs::PermissionsExt;
        let cache = InspectionCache::default();
        let directory = tempfile::tempdir().unwrap();
        let counter = directory.path().join("inspection-count");
        let executable = directory.path().join("fake-yt-dlp");
        let output = r#"{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist - Night Drive","duration":180}"#;
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\necho inspected >> '{}'\necho '{}'\n",
                counter.display(),
                output
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Default::default()
        };
        let candidate = Candidate {
            id: "youtube:x".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=x".into(),
            ..Default::default()
        };
        let key = inspection_cache_key(&candidate, &settings);
        let load = || inspect_ytdlp_metadata(&candidate, &settings, idle_cancel().1);
        let mut first = cache.get_or_inspect(key.clone(), load).await.unwrap();
        let mut second = cache.get_or_inspect(key, load).await.unwrap();
        let matching_track = Track {
            artist: "Artist".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let other_track = Track {
            artist: "Different Artist".into(),
            title: "Different Song".into(),
            ..Default::default()
        };
        matching::score_candidate(&matching_track, &mut first);
        matching::score_candidate(&other_track, &mut second);
        assert!(first.score > second.score);
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 1);
    }

    #[tokio::test]
    async fn waiting_peers_do_not_consume_processing_workers() {
        let processing = Semaphore::new(2);
        let peers = Semaphore::new(SOULSEEK_PENDING_LIMIT);
        let (_tx, mut cancel) = watch::channel(false);
        let mut admissions = Vec::new();
        for _ in 0..20 {
            admissions.push(
                download_admission("slskd", &processing, &peers, &mut cancel)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(processing.available_permits(), 2);
        let direct = download_admission("youtube", &processing, &peers, &mut cancel)
            .await
            .unwrap();
        assert_eq!(processing.available_permits(), 1);
        admissions[0]
            .begin_processing(&processing, &mut cancel)
            .await
            .unwrap();
        assert_eq!(processing.available_permits(), 0);
        assert_eq!(peers.available_permits(), SOULSEEK_PENDING_LIMIT - 19);
        drop(direct);
        drop(admissions);
        assert_eq!(processing.available_permits(), 2);
        assert_eq!(peers.available_permits(), SOULSEEK_PENDING_LIMIT);
    }

    #[tokio::test]
    async fn peer_monitor_limit_can_be_cancelled_without_leaking_slots() {
        let processing = Arc::new(Semaphore::new(2));
        let peers = Arc::new(Semaphore::new(1));
        let (_tx, mut first_cancel) = watch::channel(false);
        let first = download_admission("slskd", &processing, &peers, &mut first_cancel)
            .await
            .unwrap();
        let (tx, mut cancel) = watch::channel(false);
        let waiter = {
            let processing = processing.clone();
            let peers = peers.clone();
            tokio::spawn(async move {
                download_admission("slskd", &processing, &peers, &mut cancel)
                    .await
                    .map(|_| ())
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(processing.available_permits(), 2);
        tx.send(true).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        drop(first);
        assert_eq!(peers.available_permits(), 1);
    }

    #[test]
    fn startup_recovers_only_interrupted_authorized_soulseek_jobs() {
        let mut job = DownloadJob {
            disposition: Default::default(),
            id: "job".into(),
            track: Track::default(),
            candidate: Candidate {
                provider: "slskd".into(),
                permission: "unknown".into(),
                source: Some(CandidateSource::Soulseek {
                    username: "peer".into(),
                    filename: "song.flac".into(),
                    size_bytes: 123,
                    bitrate_kbps: None,
                    search_id: None,
                    batch_id: Some("job".into()),
                    transfer_id: None,
                }),
                ..Candidate::default()
            },
            state: "interrupted".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: String::new(),
            output_profile: "source".into(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
        };
        assert!(resumable_soulseek_job(&job));
        for state in ["failed", "cancelled", "completed"] {
            job.state = state.into();
            assert!(!resumable_soulseek_job(&job));
        }
        job.state = "interrupted".into();
        job.candidate.permission = "restricted".into();
        assert!(!resumable_soulseek_job(&job));
        job.candidate.permission = "unknown".into();
        job.candidate.provider = "youtube".into();
        assert!(!resumable_soulseek_job(&job));
    }

    #[tokio::test]
    async fn chapter_raw_source_is_copied_to_each_jobs_confined_staging() {
        let temp = tempfile::tempdir().unwrap();
        let music = temp.path().join("music");
        let first_stage = music.join(".burrata-staging").join("job-one");
        let second_stage = music.join(".burrata-staging").join("job-two");
        tokio::fs::create_dir_all(&first_stage).await.unwrap();
        tokio::fs::create_dir_all(&second_stage).await.unwrap();
        let source = first_stage.join("source.webm");
        tokio::fs::write(&source, b"raw audio bytes").await.unwrap();
        let cache = ChapterSourceCache::default();
        cache
            .insert_from_staging("candidate-format-dir", &source, &first_stage, &music)
            .await;
        let copied = cache
            .copy_to_staging("candidate-format-dir", &second_stage)
            .await
            .unwrap();
        assert!(copied.starts_with(tokio::fs::canonicalize(&second_stage).await.unwrap()));
        assert_eq!(tokio::fs::read(copied).await.unwrap(), b"raw audio bytes");
        assert!(cache
            .copy_to_staging("missing", &second_stage)
            .await
            .is_none());
    }

    #[test]
    fn slskd_free_slot_is_availability_tiebreaker_and_not_identity_score() {
        let free = Candidate {
            score: 0.4,
            reasons: vec![
                "availability:free-upload-slot".into(),
                "availability:queue-length:0".into(),
            ],
            ..Candidate::default()
        };
        let queued = Candidate {
            score: 0.99,
            reasons: vec![
                "availability:queued-remotely".into(),
                "availability:queue-length:5".into(),
            ],
            ..Candidate::default()
        };
        assert!(slskd_availability_order(&free) < slskd_availability_order(&queued));
        assert_eq!(free.score, 0.4);
        assert_eq!(queued.score, 0.99);
        let overloaded = Candidate {
            reasons: vec![
                "availability:free-upload-slot".into(),
                "availability:queue-length:10000".into(),
            ],
            ..Candidate::default()
        };
        assert!(slskd_availability_order(&queued) < slskd_availability_order(&overloaded));
    }

    #[test]
    fn rejected_peers_are_downranked_without_blaming_them_for_shutdown() {
        let candidate = Candidate {
            provider: "slskd".into(),
            score: 0.9,
            source: Some(CandidateSource::Soulseek {
                username: "peer".into(),
                filename: "song.mp3".into(),
                size_bytes: 100,
                bitrate_kbps: None,
                search_id: None,
                batch_id: None,
                transfer_id: None,
            }),
            ..Candidate::default()
        };
        // Old saved jobs did not have telemetry; deserialize one as a regression.
        let mut job: DownloadJob = serde_json::from_value(serde_json::json!({
            "id":"test", "track":Track::default(), "candidate":candidate,
            "state":"failed", "progress":0, "error":"Banned (Please consider sharing)",
            "outputPath":null, "createdAt":"", "outputProfile":"source",
        }))
        .unwrap();
        assert!(job.soulseek.is_none());
        assert_eq!(
            slskd_peer_failure_order(&candidate, &[job.clone()]),
            (true, 1)
        );
        job.error = Some("slskd transfer failed: Application shut down".into());
        assert_eq!(
            slskd_peer_failure_order(&candidate, &[job.clone()]),
            (false, 0)
        );
        job.state = "completed".into();
        assert_eq!(slskd_peer_failure_order(&candidate, &[job]), (false, 0));
        assert_eq!(candidate.score, 0.9);
    }
    #[test]
    fn parses_chapters_and_thumbnail_fallback_as_unverified_metadata() {
        let info = serde_json::json!({
            "id":"abc", "thumbnail":"javascript:alert(1)", "thumbnails":[{"url":"javascript:alert(1)"},{"url":"https://img.youtube.com/cover.jpg"}],
            "chapters":[{"start_time":0.0,"end_time":12.5,"title":"Intro"},{"start_time":12.5,"end_time":40.0,"title":" Song "},{"start_time":40.0,"end_time":30.0,"title":"bad"}]
        });
        let candidate = candidate_from_json(&info, "youtube");
        assert_eq!(
            candidate.artwork_url.as_deref(),
            Some("https://img.youtube.com/cover.jpg")
        );
        let chapters = candidate.chapters.unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].title, "Song");
        assert_eq!(chapters[1].confidence, "source_metadata_unverified");
    }
    #[test]
    fn playback_flat_youtube_bare_id_is_canonicalized_without_broadening_url_admission() {
        let make = |id: &str, raw: &str| Candidate {
            provider: "youtube".into(),
            id: format!("youtube:{id}"),
            url: raw.into(),
            ..Candidate::default()
        };
        let id = "XW9ycmy4CQA";
        let canonical = "https://www.youtube.com/watch?v=XW9ycmy4CQA";
        assert_eq!(
            playback_youtube_search_url(&make(id, id)).as_deref(),
            Some(canonical)
        );
        assert_eq!(
            playback_youtube_search_url(&make(id, "")).as_deref(),
            Some(canonical)
        );
        assert_eq!(
            playback_youtube_search_url(&make(id, canonical)).as_deref(),
            Some(canonical)
        );
        assert_eq!(
            playback_youtube_search_url(&make(id, "https://youtu.be/XW9ycmy4CQA")).as_deref(),
            Some("https://youtu.be/XW9ycmy4CQA")
        );
        // Preserve legacy missing-URL behavior; this change only adds the
        // canonical bare-ID representation and binds explicit URLs to their ID.
        assert_eq!(
            playback_youtube_search_url(&make("legacy-fixture", "")).as_deref(),
            Some("https://www.youtube.com/watch?v=legacy-fixture")
        );
        for (id, raw) in [
            ("short", "short"),
            ("XW9ycmy4CQ!", "XW9ycmy4CQ!"),
            ("XW9ycmy4CQA", "A1G8kgXWzig"),
            ("XW9ycmy4CQA", "https://www.youtube.com/watch?v=A1G8kgXWzig"),
            ("XW9ycmy4CQA", "https://youtu.be/A1G8kgXWzig"),
            ("XW9ycmy4CQA", "https://youtu.be/XW9ycmy4CQA/extra"),
            ("XW9ycmy4CQA", "https://other.invalid/watch?v=XW9ycmy4CQA"),
            ("XW9ycmy4CQA", "http://www.youtube.com/watch?v=XW9ycmy4CQA"),
            (
                "XW9ycmy4CQA",
                "https://user:secret@www.youtube.com/watch?v=XW9ycmy4CQA",
            ),
            ("XW9ycmy4CQA", "https://www.youtube.com/channel/XW9ycmy4CQA"),
            (
                "XW9ycmy4CQA",
                "https://www.youtube.com/playlist?list=XW9ycmy4CQA",
            ),
        ] {
            assert!(
                playback_youtube_search_url(&make(id, raw)).is_none(),
                "id={id}, raw={raw}"
            );
        }
        let mut other_provider = make(id, id);
        other_provider.provider = "soundcloud".into();
        assert!(playback_youtube_search_url(&other_provider).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn playback_flat_search_retains_bare_and_missing_urls_but_rejects_id_mismatch() {
        let output = serde_json::json!({"entries":[
            {"id":"XW9ycmy4CQA","url":"XW9ycmy4CQA","title":"Fixture Artist - Fixture Song","duration":180},
            {"id":"A1G8kgXWzig","url":"https://www.youtube.com/watch?v=A1G8kgXWzig","title":"Fixture Artist - Fixture Song","duration":180},
            {"id":"5OdVxV30Ids","title":"Fixture Artist - Fixture Song","duration":180},
            {"id":"short","url":"short","title":"Fixture Artist - Fixture Song","duration":180},
            {"id":"A1G8kgXWzig","url":"https://www.youtube.com/watch?v=XW9ycmy4CQA","title":"Fixture Artist - Fixture Song","duration":180},
            {"id":"A1G8kgXWzig","url":"https://other.invalid/watch?v=A1G8kgXWzig","title":"Fixture Artist - Fixture Song","duration":180}
        ]}).to_string();
        let (_directory, settings) = fake_tool(&format!("cat <<'JSON'\n{output}\nJSON"));
        let track = Track {
            artist: "Fixture Artist".into(),
            title: "Fixture Song".into(),
            duration_seconds: Some(180.),
            ..Track::default()
        };
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &settings,
            idle_cancel().1,
            PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 3);
        let bare = found
            .iter()
            .find(|candidate| candidate.id == "youtube:XW9ycmy4CQA")
            .unwrap();
        assert_eq!(bare.url, "https://www.youtube.com/watch?v=XW9ycmy4CQA");
        assert!(found
            .iter()
            .all(|candidate| validate_source_url(&candidate.url, "youtube").is_ok()));
    }

    #[test]
    fn blank_candidate_fields_use_nonempty_fallbacks_without_inventing_identity() {
        let info = serde_json::json!({
            "id":"  ",
            "webpage_url":"",
            "original_url":"https://www.youtube.com/watch?v=abc",
            "url":"https://fallback.example/video",
            "title":"  ",
            "uploader":""
        });
        let candidate = candidate_from_json(&info, "youtube");
        assert_eq!(candidate.id, "youtube:");
        assert_eq!(candidate.url, "https://www.youtube.com/watch?v=abc");
        assert_eq!(candidate.title, "Untitled");
        assert!(candidate.uploader.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn candidate_search_discards_results_with_no_source_identity() {
        let result = serde_json::json!({
            "entries":[{
                "id":"",
                "webpage_url":"https://www.youtube.com/watch?v=abc",
                "title":"Artist - A Specific Song",
                "duration":180
            }]
        });
        let script = format!("printf '%s\\n' '{}'", result);
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist".into(),
            title: "A Specific Song".into(),
            ..Default::default()
        };
        let found = find_candidates(&track, &["youtube".into()], &settings)
            .await
            .unwrap();
        assert!(found.is_empty());
    }
    #[test]
    fn chapter_label_artist_and_title_are_unverified_and_conservative() {
        for separator in [" - ", " — ", " – "] {
            assert_eq!(
                chapter_artist_title(&format!("Artist Name{separator}Night Drive")),
                ("Artist Name".into(), "Night Drive".into(), "chapter_label")
            );
        }
        assert_eq!(
            chapter_artist_title("Night Drive"),
            (String::new(), "Night Drive".into(), "unknown")
        );
    }
    #[test]
    fn chapter_metadata_is_used_only_for_a_valid_unique_catalog_match() {
        let track = Track {
            id: "musicbrainz:recording-id".into(),
            provider: "musicbrainz".into(),
            provider_id: "recording-id".into(),
            source_url: "https://musicbrainz.org/recording/recording-id".into(),
            artist: "Verified Artist Credit".into(),
            title: "Verified Song Title".into(),
            isrc: Some("FULL-RECORDING-ISRC".into()),
            ..Track::default()
        };
        let mut chapter = Chapter {
            id: "chapter-1".into(),
            title: "Raw Artist - Raw Song".into(),
            start_seconds: 0.0,
            end_seconds: 180.0,
            confidence: "source_metadata_unverified".into(),
            suggested_track: Some(Box::new(track)),
            suggestion_confidence: Some("exact_unique_catalog_match".into()),
        };
        assert_eq!(
            verified_chapter_suggestion(&chapter, None).unwrap().title,
            "Verified Song Title"
        );
        chapter.suggestion_confidence = Some("ambiguous_catalog_matches".into());
        assert!(verified_chapter_suggestion(&chapter, None).is_none());
        chapter.suggestion_confidence = Some("exact_unique_catalog_match".into());
        chapter.suggested_track.as_mut().unwrap().source_url =
            "https://attacker.invalid/recording/recording-id".into();
        assert!(verified_chapter_suggestion(&chapter, None).is_none());
        chapter.suggested_track.as_mut().unwrap().source_url =
            "https://musicbrainz.org/recording/recording-id".into();
        assert!(verified_chapter_suggestion(
            &chapter,
            Some("Live at Pitchfork Music Festival 2018")
        )
        .is_none());
        chapter.suggested_track.as_mut().unwrap().version =
            Some("Live at Pitchfork Music Festival 2018".into());
        assert!(verified_chapter_suggestion(
            &chapter,
            Some("Live at Pitchfork Music Festival 2018")
        )
        .is_some());
    }
    #[test]
    fn chapter_track_identity_is_accepted_by_asset_registration() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let (id, provider_id) = chapter_track_identity("youtube:parent", "chapter-1");
        let track = Track {
            id: id.clone(),
            provider: "chapter".into(),
            provider_id,
            title: "A song excerpt".into(),
            ..Track::default()
        };
        let asset = LocalAsset {
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            id: "asset-1".into(),
            recording_id: id,
            path: directory
                .path()
                .join("chapter.flac")
                .to_string_lossy()
                .into(),
            hash: "chapter-hash".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "excerpt".into(),
            track,
        };
        store.save_asset(&asset).unwrap();
        assert_eq!(store.list_assets().unwrap()[0].track.id, asset.track.id);
    }
    #[test]
    fn provider_host_boundaries() {
        assert!(validate_source_url("https://www.youtube.com/watch?v=a", "youtube").is_ok());
        assert!(
            validate_source_url("https://youtube.com.attacker.test/watch?v=a", "youtube").is_err()
        );
        assert!(validate_source_url("https://localhost/a.mp3", "direct").is_err());
        assert!(validate_source_url("https://127.0.0.1/a.mp3", "direct").is_err());
        assert!(
            validate_source_url("https://user:password@soundcloud.com/a", "soundcloud").is_err()
        );
    }
    #[test]
    fn retry_only_terminal_errors() {
        for state in ["failed", "cancelled", "interrupted"] {
            assert!(can_retry(state));
        }
        for state in ["queued", "downloading", "publishing", "completed"] {
            assert!(!can_retry(state));
        }
    }
    #[test]
    fn default_ffmpeg_uses_path_lookup() {
        assert_eq!(ffmpeg_location("ffmpeg"), None);
        assert_eq!(
            ffmpeg_location("/opt/tools/ffmpeg"),
            Some("/opt/tools/ffmpeg".into())
        );
    }
    #[test]
    fn archive_reuses_preserved_source_instead_of_newer_mp3_derivative() {
        let candidate = Candidate {
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=a".into(),
            ..Default::default()
        };
        let source = LocalAsset {
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            id: "source".into(),
            recording_id: "recording".into(),
            path: "/music/source.opus".into(),
            hash: "source-hash".into(),
            source_codec: "opus".into(),
            final_codec: "opus".into(),
            source_bitrate: Some(128.0),
            final_bitrate: Some(128.0),
            processing: "original audio codec preserved".into(),
            track: Track {
                provenance: serde_json::json!({"acquisition": {"provider": candidate.provider, "url": candidate.url}}),
                ..Default::default()
            },
        };
        let mut derivative = source.clone();
        derivative.id = "derivative".into();
        derivative.final_codec = "mp3".into();
        derivative.processing = "MP3320 derivative".into();
        derivative.track.provenance["derivedFromAssetId"] = Value::String(source.id.clone());
        let newest_first = [derivative, source.clone()];
        let selected = newest_first
            .iter()
            .find(|a| is_preserved_source(a, &candidate))
            .unwrap();
        assert_eq!(selected.id, source.id);
        assert_eq!(selected.final_codec, "opus");
    }

    #[tokio::test]
    async fn preserved_cross_catalog_source_reuse_requires_exact_identifier_evidence() {
        fn fixture(isrc: &str) -> (tempfile::TempDir, Store, Track, LocalAsset, Candidate) {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open(&directory.path().join("source-reuse.sqlite")).unwrap();
            let candidate = Candidate {
                id: "monochrome:recording-123".into(),
                provider: "monochrome".into(),
                url: "https://tracks.monochrome.st/track/recording-123".into(),
                title: "Song".into(),
                duration_seconds: Some(4.0),
                source_metadata: Some(serde_json::json!({
                    "provider":"monochrome",
                    "trackId":"recording-123",
                    "catalogTitle":"Song",
                    "artistNames":["Artist"],
                    "duration":4.0,
                    "isrc":isrc,
                    "verified":true
                })),
                ..Candidate::default()
            };
            let requested = Track {
                id: "musicbrainz:release-track-1".into(),
                provider: "musicbrainz".into(),
                provider_id: "release-track-1".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Requested Album".into()),
                duration_seconds: Some(4.0),
                isrc: Some(isrc.into()),
                provenance: serde_json::json!({"manualFields":{"album":true}}),
                ..Track::default()
            };
            let existing_track = Track {
                id: "spotify:existing-track".into(),
                provider: "spotify".into(),
                provider_id: "existing-track".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Existing Album".into()),
                duration_seconds: Some(4.0),
                isrc: Some(isrc.into()),
                provenance: serde_json::json!({"acquisition":{
                    "provider":"monochrome",
                    "id":candidate.id,
                    "url":candidate.url
                }}),
                ..Track::default()
            };
            store.save_track(&requested).unwrap();
            store.save_track(&existing_track).unwrap();
            let existing = LocalAsset {
                id: "existing-asset".into(),
                recording_id: existing_track.id.clone(),
                path: "/music/existing.flac".into(),
                hash: "a".repeat(64),
                source_codec: "flac".into(),
                final_codec: "flac".into(),
                source_bitrate: None,
                final_bitrate: None,
                sample_rate: Some(44100),
                channels: Some(2),
                bit_depth: Some(16),
                duration_seconds: Some(4.0),
                size_bytes: Some(1000),
                bitrate_origin: None,
                processing: "original audio codec preserved".into(),
                track: existing_track,
            };
            (directory, store, requested, existing, candidate)
        }

        let isrc = "USABC2400001";
        let (_directory, store, requested, existing, candidate) = fixture(isrc);
        let assessment = preserved_source_reuse_assessment(
            &store,
            &requested,
            &existing,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
        )
        .expect("the same verified recording source can be copied to the requested listing");
        let artifact =
            preserved_source_reuse_track(&requested, &existing, &candidate, &assessment, None)
                .unwrap();
        assert_eq!(artifact.id, requested.id);
        assert_eq!(artifact.provider_id, requested.provider_id);
        assert_eq!(artifact.album, requested.album);
        assert_eq!(artifact.provenance["manualFields"]["album"], true);
        assert_eq!(artifact.provenance["acquisition"]["id"], candidate.id);
        assert_eq!(artifact.provenance["sourceReuse"]["assetId"], existing.id);
        assert_eq!(artifact.provenance["sourceReuse"]["isrcAgreement"], true);

        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let original_path = _directory.path().join("original.wav");
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-c:a",
                "pcm_s16le",
                "-y",
            ])
            .arg(&original_path)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let original_hash = media::hash_file(&original_path).unwrap();
        let mut existing = existing;
        existing.path = original_path.to_string_lossy().into_owned();
        existing.hash = original_hash.clone();
        media::verify_asset(&existing).await.unwrap();
        store.save_asset(&existing).unwrap();

        let settings = Settings {
            music_directory: _directory
                .path()
                .join("managed")
                .to_string_lossy()
                .into_owned(),
            ..Settings::default()
        };
        let copy = media::finalize_download(
            &original_path,
            &artifact,
            &candidate,
            &settings,
            "source",
            "verified-source-reuse-test",
        )
        .await
        .unwrap();
        store.save_asset(&copy).unwrap();
        assert_ne!(copy.path, existing.path);
        assert_eq!(copy.recording_id, requested.id);
        assert_eq!(copy.track.id, requested.id);
        assert_eq!(copy.track.album.as_deref(), Some("Requested Album"));
        assert_eq!(copy.track.provenance["sourceReuse"]["assetId"], existing.id);
        assert_eq!(media::hash_file(&original_path).unwrap(), original_hash);
        assert!(store.list_assets().unwrap().iter().any(|asset| {
            asset.id == existing.id
                && asset.recording_id == existing.recording_id
                && asset.path == existing.path
                && asset.hash == original_hash
        }));

        let (_directory, store, mut requested, existing, candidate) = fixture(isrc);
        requested.isrc = Some("USXYZ2400002".into());
        requested.provenance["manualFields"]["isrc"] = serde_json::json!(true);
        assert!(preserved_source_reuse_assessment(
            &store,
            &requested,
            &existing,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
        )
        .is_none());

        let (_directory, store, requested, existing, mut candidate) = fixture(isrc);
        candidate.id = "monochrome:another-recording".into();
        candidate.url = "https://tracks.monochrome.st/track/another-recording".into();
        candidate.source_metadata.as_mut().unwrap()["trackId"] =
            serde_json::json!("another-recording");
        assert!(preserved_source_reuse_assessment(
            &store,
            &requested,
            &existing,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
        )
        .is_none());
    }

    #[test]
    fn soulseek_album_selection_groups_exact_peer_and_directory_and_checks_numbering() {
        fn expected(id: &str, title: &str, number: u32) -> Track {
            Track {
                id: id.into(),
                provider: "musicbrainz".into(),
                provider_id: id.into(),
                artist: "Artist".into(),
                album_artist: Some("Artist".into()),
                title: title.into(),
                album: Some("Album".into()),
                track_number: Some(number),
                duration_seconds: Some(120.0),
                ..Track::default()
            }
        }
        fn file(peer: &str, directory: &str, number: u32, title: &str) -> Candidate {
            let filename = format!("{directory}/{number:02} - {title}.flac");
            let basename = format!("{number:02} - {title}");
            Candidate {
                id: format!("slskd:{peer}:{directory}:{number}"),
                provider: "slskd".into(),
                title: basename.clone(),
                uploader: Some(peer.into()),
                duration_seconds: Some(120.0),
                formats: vec![AudioFormat {
                    id: "flac".into(),
                    codec: "flac".into(),
                    extension: "flac".into(),
                    bitrate: Some(900.0),
                    sample_rate: Some(44100),
                    bit_depth: Some(16),
                    channels: Some(2),
                    size_bytes: Some(2_000_000),
                    lossless: true,
                }],
                permission: "unknown".into(),
                source_metadata: Some(serde_json::json!({
                    "sourceMetadataOrigin":"slskdFilename",
                    "filenameObservations":crate::recording_filename::filename_observations(&filename,&basename)
                })),
                source: Some(CandidateSource::Soulseek {
                    username: peer.into(),
                    filename,
                    size_bytes: 2_000_000,
                    bitrate_kbps: Some(900),
                    search_id: None,
                    batch_id: None,
                    transfer_id: None,
                }),
                ..Candidate::default()
            }
        }
        fn mp3(mut candidate: Candidate) -> Candidate {
            if let Some(format) = candidate.formats.first_mut() {
                format.id = "mp3".into();
                format.codec = "mp3".into();
                format.extension = "mp3".into();
                format.bitrate = Some(320.0);
                format.sample_rate = Some(44100);
                format.bit_depth = None;
                format.lossless = false;
            }
            candidate
        }

        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("album-selection.sqlite")).unwrap();
        let expected_tracks = vec![
            expected("track-1", "Song One", 1),
            expected("track-2", "Song Two", 2),
        ];
        let aliases = expected_tracks
            .iter()
            .map(|track| {
                crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, track)
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(10);
        let (_cancel_tx, cancel) = watch::channel(false);
        let assignments = select_soulseek_album_group(
            &store,
            &expected_tracks,
            &aliases,
            vec![
                file("peer-b", "Music/Artist/Album", 1, "Song One"),
                file("peer-b", "Music/Artist/Album", 2, "Song Two"),
                file("peer-b", "Music/Artist/Album", 2, "Song One"),
                file("peer-a", "Music/Artist/Album", 1, "Song One"),
                file("peer-a", "Music/Artist/Album", 2, "Song Two"),
                file("peer-a", "Music/Artist/Album", 1, "Wrong Song"),
                file("peer-c", "Music/Artist/Other Album", 1, "Song One"),
                file("peer-c", "Music/Artist/Other Album", 2, "Song Two"),
            ],
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert_eq!(assignments.assignments.len(), 2);
        assert!(assignments.assignments.iter().all(|assignment| {
            assignment.group_peer == "peer-a"
                && assignment.group_directory == "Music/Artist/Album"
                && assignment.group_coverage == 2
        }));
        assert_eq!(
            assignments
                .assignments
                .iter()
                .map(|assignment| assignment.track_id.as_str())
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from(["track-1", "track-2"])
        );
        assert!(assignments.assignments.iter().all(|assignment| {
            !matches!(assignment.candidate.source.as_ref(), Some(CandidateSource::Soulseek { filename, .. }) if filename.contains("02 - Song One") || filename.contains("Wrong Song"))
        }));

        let reversed = select_soulseek_album_group(
            &store,
            &expected_tracks,
            &aliases,
            vec![
                file("peer-c", "Music/Artist/Other Album", 2, "Song Two"),
                file("peer-c", "Music/Artist/Other Album", 1, "Song One"),
                file("peer-a", "Music/Artist/Album", 2, "Song Two"),
                file("peer-a", "Music/Artist/Album", 1, "Song One"),
                file("peer-b", "Music/Artist/Album", 2, "Song Two"),
                file("peer-b", "Music/Artist/Album", 1, "Song One"),
            ],
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert!(reversed
            .assignments
            .iter()
            .all(|assignment| assignment.group_peer == "peer-a"));

        let quality_selection = select_soulseek_album_group(
            &store,
            &expected_tracks,
            &aliases,
            vec![
                mp3(file("aaa-mp3", "Music/Artist/Album", 1, "Song One")),
                mp3(file("aaa-mp3", "Music/Artist/Album", 2, "Song Two")),
                file("zzz-flac", "Music/Artist/Album", 1, "Song One"),
                file("zzz-flac", "Music/Artist/Album", 2, "Song Two"),
            ],
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert_eq!(quality_selection.groups_examined, 2);
        assert_eq!(quality_selection.assignments.len(), 2);
        assert!(quality_selection
            .assignments
            .iter()
            .all(|assignment| assignment.group_peer == "zzz-flac"));

        let singleton = select_soulseek_album_group(
            &store,
            &expected_tracks,
            &aliases,
            vec![file("peer-single", "Music/Artist/Album", 1, "Song One")],
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert!(
            singleton.assignments.is_empty(),
            "one accidental hit is not an album group"
        );
        let already_expired = select_soulseek_album_group(
            &store,
            &expected_tracks,
            &aliases,
            vec![file("peer-single", "Music/Artist/Album", 1, "Song One")],
            crate::recording_match::MatchLevel::Balanced,
            Instant::now() - Duration::from_millis(1),
            &cancel,
        );
        assert!(already_expired.assignments.is_empty());
        let (_cancel_tx, cancelled) = watch::channel(true);
        let cancelled_selection = select_soulseek_album_group(
            &store,
            &expected_tracks,
            &aliases,
            vec![file("peer-single", "Music/Artist/Album", 1, "Song One")],
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancelled,
        );
        assert!(cancelled_selection.assignments.is_empty());
    }

    #[test]
    fn soulseek_album_group_ranking_uses_title_coverage_before_ordinals_then_quality() {
        fn group(
            peer: &str,
            title_tracks: usize,
            ordinal_tracks: usize,
            title_pairs: usize,
            ordinal_pairs: usize,
            lossless: usize,
        ) -> SoulseekAlbumCandidateGroup {
            SoulseekAlbumCandidateGroup {
                peer: peer.into(),
                directory: format!("Music/{peer}/Album"),
                files: Vec::new(),
                potential_lossless_count: lossless,
                potential_bitrate_total: 3_200,
                folder_artist_file_count: 13,
                best_availability: (0, 0, 0, std::cmp::Reverse(0)),
                exact_title_pair_count: title_pairs,
                ordinal_pair_count: ordinal_pairs,
                exact_title_track_count: title_tracks,
                ordinal_track_count: ordinal_tracks,
            }
        }

        let ordinal_only = group("ordinal", 0, 13, 0, 13, 13);
        let title_supported = group("title", 10, 13, 10, 13, 0);
        assert!(compare_soulseek_album_candidate_groups(&title_supported, &ordinal_only).is_lt());

        // Once identity-supporting title coverage ties, retain the existing
        // preference for a coherent lossless group over a lossy one.
        let lossless = group("lossless", 10, 13, 10, 13, 13);
        let lossy = group("lossy", 10, 13, 10, 13, 0);
        assert!(compare_soulseek_album_candidate_groups(&lossless, &lossy).is_lt());
    }

    #[test]
    fn soulseek_album_selection_accepts_bulgaria_release_fixture_group() {
        const RELEASE_ID: &str = "1f602996-f661-3611-b25a-93c3337fd76e";
        const ARTIST_ID: &str = "80fb9c75-af15-4a6b-9528-715f9ef4ebea";
        const ALBUM: &str = "Le Mystère des voix bulgares";
        const CATALOG_ARTIST: &str = "The Bulgarian State Radio and Television Female Vocal Choir";
        const FOLDER_ARTIST: &str = "Bulgarian State Radio & Television Female Vocal Choir";
        let rows = [
            ("Pilentze Pee", 146.666, 146.0),
            ("Svatba", 91.0, 91.0),
            ("Kalimankou Denkou", 310.0, 311.0),
            ("Strati na Angelaki Doumasche", 160.0, 161.0),
            ("Polegnala e Pschenitza", 124.0, 124.0),
            ("Messetschinko lio Greïlivko", 154.0, 154.0),
            ("Breï Yvane", 94.973, 94.0),
            ("Erghen Diado", 174.0, 174.0),
            ("Sableyalo mi Agontze", 275.0, 275.0),
            ("Prïtourïtze Planinata", 168.04, 170.0),
            ("Mir Stanke Le", 197.0, 197.0),
            ("Schopska Pesen", 88.52, 90.0),
            ("Polegnala e Todora", 217.134, 217.0),
        ];
        let source_url = format!("https://musicbrainz.org/release/{RELEASE_ID}");
        let tracks = rows
            .iter()
            .enumerate()
            .map(|(index, (title, duration, _))| {
                let position = index + 1;
                Track {
                    id: format!("catalog:musicbrainz:{RELEASE_ID}:1:{position}:1:fixture"),
                    provider: "catalog".into(),
                    provider_id: format!("musicbrainz:{RELEASE_ID}:1:{position}:1:fixture"),
                    source_url: source_url.clone(),
                    artist: CATALOG_ARTIST.into(),
                    title: (*title).into(),
                    album: Some(ALBUM.into()),
                    duration_seconds: Some(*duration),
                    provenance: serde_json::json!({
                        "artistCredits": [{
                            "artist": {"provider":"musicbrainz", "id":ARTIST_ID},
                            "creditedAs":CATALOG_ARTIST,
                            "joinPhrase":"",
                            "name":CATALOG_ARTIST,
                            "role":"artist credit",
                            "sourceUrl":source_url,
                        }],
                        "catalogProvider":"musicbrainz",
                        "manualFields":{},
                        "mediumPosition":1,
                        "occurrence":1,
                        "releaseId":RELEASE_ID,
                        "sourceKind":"catalog",
                        "sourceUrl":source_url,
                        "trackPosition":position.to_string(),
                    }),
                    ..Track::default()
                }
            })
            .collect::<Vec<_>>();
        let candidates = rows
            .iter()
            .enumerate()
            .map(|(index, (title, _, candidate_duration))| {
                let position = index + 1;
                let basename = format!("{position:02} - {title}");
                let filename = format!("Root/{FOLDER_ARTIST}/{ALBUM}, Volume 1 (1975)/{basename}.flac");
                Candidate {
                    id: format!("slskd:fixture-peer:track-{position}"),
                    provider: "slskd".into(),
                    title: basename.clone(),
                    uploader: Some("fixture-peer".into()),
                    duration_seconds: Some(*candidate_duration),
                    formats: vec![AudioFormat {
                        id: "flac".into(),
                        codec: "flac".into(),
                        extension: "flac".into(),
                        bitrate: None,
                        sample_rate: None,
                        bit_depth: None,
                        channels: None,
                        size_bytes: None,
                        lossless: true,
                    }],
                    permission: "unknown".into(),
                    source_metadata: Some(serde_json::json!({
                        "sourceMetadataOrigin":"slskdFilename",
                        "filenameObservations":crate::recording_filename::filename_observations(&filename, &basename),
                    })),
                    source: Some(CandidateSource::Soulseek {
                        username: "fixture-peer".into(),
                        filename,
                        size_bytes: 16_000_000,
                        bitrate_kbps: None,
                        search_id: None,
                        batch_id: None,
                        transfer_id: None,
                    }),
                    ..Candidate::default()
                }
            })
            .collect::<Vec<_>>();

        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("bulgaria-album-selection.sqlite")).unwrap();
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                ARTIST_ID,
                &serde_json::json!({
                    "id":ARTIST_ID,
                    "name":"Bulgarian State Radio and Television Female Vocal Choir",
                    "aliases":[
                        {"name":FOLDER_ARTIST,"type":"Artist name"},
                        {"name":"The Bulgarian State Radio and Television Female Vocal Choir","type":"Artist name"},
                    ],
                }),
            )
            .unwrap();
        let aliases = tracks
            .iter()
            .map(|track| {
                crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store, track)
            })
            .collect::<Vec<_>>();
        let mut assessment_context = crate::recording_match_context::AssessmentContext::default();
        for (index, (track, candidate)) in tracks.iter().zip(&candidates).enumerate() {
            let direct = crate::recording_match_context::assess(
                &store,
                track,
                candidate,
                crate::recording_match::MatchLevel::Balanced,
                crate::recording_match::MatchStage::Resolved,
            );
            let prepared = crate::recording_match_context::assess_with_aliases_and_context(
                &store,
                track,
                candidate,
                crate::recording_match::MatchLevel::Balanced,
                crate::recording_match::MatchStage::Resolved,
                aliases[index].tracks(),
                aliases[index].isrc_enriched(),
                aliases[index].verified_recording_isrc(),
                aliases[index].release_runtime_variants(),
                &mut assessment_context,
            );
            assert_eq!(prepared.action, direct.action, "{}", track.title);
            assert_eq!(
                prepared.relationship, direct.relationship,
                "{}",
                track.title
            );
            assert_eq!(prepared.dimensions, direct.dimensions, "{}", track.title);
            assert_eq!(
                prepared
                    .evidence
                    .as_ref()
                    .map(|e| (e.parsing_incomplete, e.conflicting_version_observations,)),
                direct
                    .evidence
                    .as_ref()
                    .map(|e| (e.parsing_incomplete, e.conflicting_version_observations,)),
                "{}",
                track.title
            );
            assert_eq!(
                direct.action,
                crate::recording_match::MatchAction::Accept,
                "{}",
                track.title
            );
            assert!(
                direct
                    .evidence
                    .as_ref()
                    .and_then(|evidence| evidence.scoped_artist_credit_set_alias.as_ref())
                    .is_some(),
                "expected typed MusicBrainz profile credit-set alias for {}",
                track.title
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let (_cancel_tx, cancel) = watch::channel(false);
        let selection = select_soulseek_album_group(
            &store,
            &tracks,
            &aliases,
            candidates.clone(),
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert_eq!(selection.assignments.len(), rows.len());
        assert!(selection
            .assignments
            .iter()
            .all(|assignment| assignment.group_coverage == rows.len()));

        // With no filename ordinal, one exact-title file is compatible with
        // both duplicate requested rows. Both shared assessments accept, so
        // the selector must retain ambiguity instead of assigning it to the
        // first row only.
        let duplicate_tracks = [tracks[0].clone(), tracks[0].clone()]
            .into_iter()
            .enumerate()
            .map(|(index, mut track)| {
                track.id = format!("{}:duplicate-{index}", track.id);
                track
            })
            .collect::<Vec<_>>();
        let duplicate_aliases = duplicate_tracks
            .iter()
            .map(|track| {
                crate::artist_credit_aliases::cached_verified_playback_artist_aliases(
                    &store, track,
                )
            })
            .collect::<Vec<_>>();
        let mut unnumbered = candidates[0].clone();
        unnumbered.id = "slskd:fixture-peer:unnumbered-duplicate-title".into();
        unnumbered.title = rows[0].0.into();
        let filename = format!(
            "Root/{FOLDER_ARTIST}/{ALBUM}, Volume 1 (1975)/{}.flac",
            rows[0].0
        );
        if let Some(CandidateSource::Soulseek {
            filename: source_filename,
            ..
        }) = unnumbered.source.as_mut()
        {
            *source_filename = filename.clone();
        }
        unnumbered.source_metadata = Some(serde_json::json!({
            "sourceMetadataOrigin":"slskdFilename",
            "filenameObservations":crate::recording_filename::filename_observations(&filename, rows[0].0),
        }));
        assert!(unnumbered.source_metadata.as_ref().unwrap()["filenameObservations"]
            ["trackNumber"]
            .is_null());
        let mut duplicate_assessment_context =
            crate::recording_match_context::AssessmentContext::default();
        for (track, aliases) in duplicate_tracks.iter().zip(&duplicate_aliases) {
            let assessment = crate::recording_match_context::assess_with_aliases_and_context(
                &store,
                track,
                &unnumbered,
                crate::recording_match::MatchLevel::Balanced,
                crate::recording_match::MatchStage::Resolved,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
                aliases.release_runtime_variants(),
                &mut duplicate_assessment_context,
            );
            assert_eq!(assessment.action, crate::recording_match::MatchAction::Accept);
            assert_eq!(
                assessment.dimensions.title,
                crate::recording_match::Comparison::Agree
            );
        }
        let duplicate_selection = select_soulseek_album_group(
            &store,
            &duplicate_tracks,
            &duplicate_aliases,
            vec![unnumbered],
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert_eq!(duplicate_selection.shared_assessments, 2);
        assert_eq!(duplicate_selection.candidate_pairs_after_ordinal, 2);
        assert_eq!(duplicate_selection.candidate_pairs_after_title, 2);
        assert!(duplicate_selection.assignments.is_empty());

        let rebase_candidates = |folder_artist: &str, root: &str| {
            candidates
                .iter()
                .map(|candidate| {
                    let mut rebased = candidate.clone();
                    let filename = format!(
                        "{root}/{folder_artist}/{ALBUM}, Volume 1 (1975)/{}.flac",
                        candidate.title
                    );
                    if let Some(CandidateSource::Soulseek {
                        filename: current, ..
                    }) = rebased.source.as_mut()
                    {
                        *current = filename.clone();
                    }
                    rebased.source_metadata = Some(serde_json::json!({
                        "sourceMetadataOrigin":"slskdFilename",
                        "filenameObservations":crate::recording_filename::filename_observations(&filename, &candidate.title),
                    }));
                    rebased
                })
                .collect::<Vec<_>>()
        };
        let music_root_selection = select_soulseek_album_group(
            &store,
            &tracks,
            &aliases,
            rebase_candidates(FOLDER_ARTIST, "Music"),
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert_eq!(music_root_selection.assignments.len(), rows.len());
        let unrelated_artist_selection = select_soulseek_album_group(
            &store,
            &tracks,
            &aliases,
            rebase_candidates("Unrelated Folder Artist", "Root"),
            crate::recording_match::MatchLevel::Balanced,
            deadline,
            &cancel,
        );
        assert!(unrelated_artist_selection.assignments.is_empty());
    }

    #[test]
    fn soulseek_album_deadline_keeps_best_fully_validated_group() {
        let track = Track {
            id: "track-one".into(),
            title: "Song One".into(),
            ..Track::default()
        };
        let candidate = Candidate {
            id: "slskd:peer:album:1".into(),
            ..Candidate::default()
        };
        let selection = SoulseekAlbumSelection {
            stopped_on_deadline: true,
            best_coverage_at_stop: 1,
            ..SoulseekAlbumSelection::default()
        };
        let finished = finish_soulseek_album_selection(
            selection,
            vec![EvaluatedSoulseekAlbumGroup {
                peer: "peer-good".into(),
                directory: "Music/Artist/Album".into(),
                assignments: vec![(0, candidate.clone())],
                lossless_count: 1,
                bitrate_total: 900,
                availability: (0, 0, 0, std::cmp::Reverse(100)),
            }],
            &[track],
        );
        assert!(finished.stopped_on_deadline);
        assert_eq!(finished.assignments.len(), 1);
        assert_eq!(finished.assignments[0].group_peer, "peer-good");
        assert_eq!(finished.assignments[0].candidate.id, candidate.id);
    }

    #[test]
    fn soulseek_album_query_uses_verified_compact_alias_and_title_only_when_alias_is_album() {
        let mut track = Track {
            artist: "The Long Canonical Artist Credit".into(),
            album_artist: Some("The Long Canonical Artist Credit".into()),
            album: Some("Le Mystère des voix bulgares".into()),
            ..Track::default()
        };
        assert_eq!(
            soulseek_album_search_query(
                &track,
                track.album.as_deref().unwrap(),
                &["Le Mystère des voix bulgares".into()]
            )
            .unwrap(),
            "Le Mystère des voix bulgares"
        );
        assert_eq!(
            soulseek_album_search_query(
                &track,
                track.album.as_deref().unwrap(),
                &["Verified Compact Artist".into()]
            )
            .unwrap(),
            "Verified Compact Artist Le Mystère des voix bulgares"
        );
        track.album = Some("Greatest Hits".into());
        assert_eq!(
            soulseek_album_search_query(&track, "Greatest Hits", &[]).unwrap(),
            "The Long Canonical Artist Credit Greatest Hits"
        );
        assert_eq!(minimum_soulseek_album_coverage(13), 2);
        assert_eq!(minimum_soulseek_album_coverage(1), 1);
    }

    #[test]
    fn soulseek_album_caller_returns_validated_assignments_after_selection_deadline() {
        assert_eq!(
            soulseek_album_query_decision(false, true, true, false),
            SoulseekAlbumQueryDecision::Return,
            "a deadline must not discard assignments already validated by the selector"
        );
        assert_eq!(
            soulseek_album_query_decision(false, true, false, false),
            SoulseekAlbumQueryDecision::Return,
            "an exhausted deadline must stop before starting another query"
        );
        assert_eq!(
            soulseek_album_query_decision(false, false, false, false),
            SoulseekAlbumQueryDecision::Continue
        );
        assert_eq!(
            soulseek_album_query_decision(true, true, true, true),
            SoulseekAlbumQueryDecision::Cancel,
            "cancellation takes precedence over returning an assignment"
        );
    }

    #[test]
    fn soulseek_album_folder_suffix_does_not_override_track_identity() {
        let mut track = Track {
            album: Some("Le Mystère des voix bulgares".into()),
            release_date: Some("1986".into()),
            ..Track::default()
        };
        assert!(soulseek_album_folder_matches(
            "Le Mystère des voix bulgares, Volume 1 [1986]",
            &track
        ));
        // A reissue year alone is not a recording contradiction.
        assert!(soulseek_album_folder_matches(
            "Le Mystère des voix bulgares [1990 Remaster]",
            &track
        ));
        track.album = Some("Other Album".into());
        assert!(!soulseek_album_folder_matches(
            "Le Mystère des voix bulgares, Volume 1",
            &track
        ));
    }

    #[test]
    fn restrictions_are_not_inferred_as_permissions() {
        let public = candidate_from_json(
            &serde_json::json!({"id":"a","availability":"public"}),
            "youtube",
        );
        assert_eq!(public.permission, "unknown");
        let private = candidate_from_json(
            &serde_json::json!({"id":"b","availability":"private"}),
            "youtube",
        );
        assert_eq!(private.permission, "restricted");
    }
    #[test]
    fn raw_search_track_preserves_known_artist_and_does_not_promote_uploader() {
        let metadata = serde_json::json!({"id":"abc","artist":"Artist Name","track":"Track Name","title":"Artist Name - Track Name","uploader":"Label Channel","album":"EP","genre":"House","release_date":"20250101"});
        let mut candidate = candidate_from_json(&metadata, "youtube");
        candidate.url = "https://www.youtube.com/watch?v=abc".into();
        candidate.score = 0.8;
        candidate.confidence = "review".into();
        let track = raw_search_track(candidate.clone(), &metadata);
        assert_eq!(track.artist, "Artist Name");
        assert_eq!(track.title, "Track Name");
        assert_eq!(track.album.as_deref(), Some("EP"));
        assert_eq!(
            track.provenance["identity"],
            "unverified source search result"
        );
        assert_eq!(track.provenance["audioCandidate"]["url"], candidate.url);
        assert_eq!(track.provenance["uploader"], "Label Channel");

        let unknown = serde_json::json!({"id":"def","title":"Ambient Session","uploader":"Uploader Channel","upload_date":"20261012"});
        let candidate = candidate_from_json(&unknown, "soundcloud");
        let track = raw_search_track(candidate, &unknown);
        assert!(track.artist.is_empty());
        assert_eq!(track.title, "Ambient Session");
        assert_eq!(track.provenance["uploader"], "Uploader Channel");
        assert_eq!(track.provenance["artistSource"], "unknown");
        assert!(track.release_date.is_none());
        assert_eq!(track.provenance["uploadDate"], "20261012");

        let missing_title = serde_json::json!({
            "id":"ghi",
            "title":"  ",
            "track":null,
            "uploader":"Channel Name"
        });
        let candidate = candidate_from_json(&missing_title, "youtube");
        assert_eq!(candidate.title, "Untitled");
        let track = raw_search_track(candidate, &missing_title);
        assert!(track.title.is_empty());
        assert!(track.artist.is_empty());
        assert_eq!(track.provenance["uploader"], "Channel Name");
    }
    #[test]
    fn raw_search_track_recognizes_single_spaced_credit_separators() {
        for separator in [" — ", " – ", " - "] {
            let info = serde_json::json!({
                "id":"source-id",
                "title":format!("Artist Name{separator}Song Title"),
                "uploader":"Unrelated Channel"
            });
            let candidate = candidate_from_json(&info, "soundcloud");
            let track = raw_search_track(candidate, &info);
            assert_eq!(track.artist, "Artist Name");
            assert_eq!(track.title, "Song Title");
            assert_eq!(track.provenance["artistSource"], "inferred_from_title");
        }
        let ambiguous = split_source_credit_title("Artist - Song – Subtitle");
        assert!(ambiguous.is_none());
    }
    #[test]
    fn raw_youtube_performance_title_gets_live_version_without_promoting_uploader() {
        let info = serde_json::json!({
            "id":"video12345",
            "title":"Big Thief Perform “Mary” | Pitchfork Music Festival 2018",
            "uploader":"Pitchfork",
            "upload_date":"20261012"
        });
        let mut candidate = candidate_from_json(&info, "youtube");
        candidate.url = "https://www.youtube.com/watch?v=video12345".into();
        let track = raw_search_track(candidate, &info);
        assert_eq!(track.artist, "Big Thief");
        assert_eq!(track.title, "Mary");
        assert_eq!(
            track.version.as_deref(),
            Some("Live at Pitchfork Music Festival 2018")
        );
        assert_eq!(track.provenance["uploader"], "Pitchfork");
        assert_eq!(track.provenance["sourceTitle"], info["title"]);
        assert_eq!(track.release_date, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_audio_search_uses_query_directly_and_maps_unverified_tracks() {
        let entry = serde_json::json!({
            "id":"aBc12345678", "url":"https://www.youtube.com/watch?v=aBc12345678",
            "title":"Luna Vale - Night Drive (Extended Mix)", "artist":"Luna Vale",
            "track":"Night Drive (Extended Mix)", "uploader":"Luna Vale - Topic",
            "duration":367, "thumbnail":"https://i.ytimg.com/vi/aBc12345678/hqdefault.jpg"
        });
        let script = format!("echo '{}'", serde_json::json!({"entries":[entry]}));
        let (_directory, settings) = fake_tool(&script);
        let found =
            search_audio_tracks_raw("night drive luna vale", &["youtube".into()], &settings)
                .await
                .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artist, "Luna Vale");
        assert_eq!(found[0].title, "Night Drive (Extended Mix)");
        assert_eq!(found[0].provider, "youtube");
        assert_eq!(found[0].provider_id, "aBc12345678");
        assert_eq!(
            found[0].provenance["audioCandidate"]["confidence"],
            "review"
        );

        let direct = find_candidates_with_cancel(
            &found[0],
            &["youtube".into()],
            &settings,
            idle_cancel().1,
            3,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].url, found[0].source_url);
    }

    #[test]
    fn youtube_results_search_url_preserves_verbatim_query_with_safe_encoding() {
        let query = r#""Ro Myra" and Lena & Rich 東京"#;
        let url = url::Url::parse(&youtube_search_results_url(query)).unwrap();
        assert_eq!(url.host_str(), Some("www.youtube.com"));
        assert_eq!(url.path(), "/results");
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "search_query")
                .unwrap()
                .1,
            query
        );
        assert_eq!(url.query_pairs().count(), 1);

        let video = serde_json::json!({
            "id": "A1G8kgXWzig",
            "url": "https://www.youtube.com/watch?v=A1G8kgXWzig",
            "title": "Ro Myra - Irene (Official Video) | Indie Folk Rock"
        });
        let candidate = candidate_from_json(&video, "youtube");
        assert_eq!(
            youtube_search_video_url(&video, &candidate).as_deref(),
            Some("https://www.youtube.com/watch?v=A1G8kgXWzig")
        );
        let conflicting = candidate_from_json(
            &serde_json::json!({"id":"different", "url":"https://www.youtube.com/watch?v=A1G8kgXWzig"}),
            "youtube",
        );
        assert!(youtube_search_video_url(&video, &conflicting).is_none());
        for row in [
            serde_json::json!({"id":"UC1234567890", "url":"https://www.youtube.com/channel/UC1234567890"}),
            serde_json::json!({"id":"PL1234567890", "url":"https://www.youtube.com/playlist?list=PL1234567890"}),
            serde_json::json!({"id":"A1G8kgXWzig"}),
        ] {
            let candidate = candidate_from_json(&row, "youtube");
            assert!(youtube_search_video_url(&row, &candidate).is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn youtube_results_url_search_preserves_query_paging_and_video_identity() {
        let args_directory = tempfile::tempdir().unwrap();
        let args_file = args_directory.path().join("args");
        let fixture = serde_json::json!({"entries": [
            {"id":"A1G8kgXWzig", "url":"https://www.youtube.com/watch?v=A1G8kgXWzig", "title":"Ro Myra - Irene"},
            {"id":"UC1234567890", "url":"https://www.youtube.com/channel/UC1234567890", "title":"Ro Myra"},
            {"id":"PL1234567890", "url":"https://www.youtube.com/playlist?list=PL1234567890", "title":"Ro Myra set"}
        ]}).to_string();
        let args_path = args_file.clone();
        let script = format!(
            "for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\ncat <<'JSON'\n{fixture}\nJSON",
            args_path.display()
        );
        let (_tool_directory, settings) = fake_tool(&script);
        let query = r#""Ro Myra" and Lena & Rich 東京"#;
        let page = search_audio_tracks_page_raw(query, "youtube", 1, 3, &settings)
            .await
            .unwrap();
        assert_eq!(page.entry_count, 3);
        assert_eq!(page.tracks.len(), 1);
        assert_eq!(page.tracks[0].provider_id, "A1G8kgXWzig");

        let args = std::fs::read_to_string(&args_file).unwrap();
        let args = args.lines().collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--playlist-start", "2"]));
        assert!(args.windows(2).any(|pair| pair == ["--playlist-end", "4"]));
        let search_url = args
            .iter()
            .find(|argument| argument.starts_with("https://www.youtube.com/results?"))
            .expect("YouTube results URL must be passed as one argv item");
        let parsed = url::Url::parse(search_url).unwrap();
        assert_eq!(parsed.query_pairs().count(), 1);
        assert_eq!(
            parsed.query_pairs().next().unwrap(),
            ("search_query".into(), query.into())
        );
        assert!(!args.iter().any(|argument| argument.starts_with("ytsearch")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_youtube_results_url_search_keeps_fifty_row_cap_and_video_identity() {
        let args_directory = tempfile::tempdir().unwrap();
        let args_file = args_directory.path().join("args");
        let fixture = serde_json::json!({"entries": [
            {"id":"A1G8kgXWzig", "url":"https://www.youtube.com/watch?v=A1G8kgXWzig", "title":"Ro Myra - Irene"},
            {"id":"UC1234567890", "url":"https://www.youtube.com/channel/UC1234567890", "title":"Ro Myra channel"}
        ]}).to_string();
        let script = format!(
            "for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\ncat <<'JSON'\n{fixture}\nJSON",
            args_file.display()
        );
        let (_tool_directory, settings) = fake_tool(&script);
        let query = r#""Ro Myra" and Lena & Rich"#;
        let tracks = search_audio_tracks_raw(query, &["youtube".into()], &settings)
            .await
            .unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].provider_id, "A1G8kgXWzig");

        let args = std::fs::read_to_string(&args_file).unwrap();
        let args = args.lines().collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["--playlist-end", "50"]));
        let search_url = args
            .iter()
            .find(|argument| argument.starts_with("https://www.youtube.com/results?"))
            .expect("YouTube results URL must be passed as one argv item");
        let parsed = url::Url::parse(search_url).unwrap();
        assert_eq!(parsed.query_pairs().count(), 1);
        assert_eq!(parsed.query_pairs().next().unwrap().1, query);
        assert!(!args.iter().any(|argument| argument.starts_with("ytsearch")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_audio_search_pages_use_playlist_offsets_and_stop_on_empty_page() {
        let first_page = serde_json::json!({"entries": (0..30).map(|index| serde_json::json!({
            "id": format!("a{index:010}"), "url": format!("https://www.youtube.com/watch?v=a{index:010}"),
            "title": format!("Artist - Track {index}"), "duration": 180
        })).collect::<Vec<_>>()}).to_string();
        let short_page = serde_json::json!({"entries": (30..32).map(|index| serde_json::json!({
            "id": format!("a{index:010}"), "url": format!("https://www.youtube.com/watch?v=a{index:010}"),
            "title": format!("Artist - Track {index}"), "duration": 180
        })).collect::<Vec<_>>()}).to_string();
        let script = format!(
            "start=''; previous=''; for arg in \"$@\"; do if [ \"$previous\" = '--playlist-start' ]; then start=\"$arg\"; fi; previous=\"$arg\"; done; case \"$start\" in 1) echo '{first_page}' ;; 31) echo '{short_page}' ;; *) echo '{{\"entries\":[]}}' ;; esac"
        );
        let (_directory, settings) = fake_tool(&script);
        let first = search_audio_tracks_page_raw("night drive", "youtube", 0, 30, &settings)
            .await
            .unwrap();
        let second = search_audio_tracks_page_raw("night drive", "youtube", 30, 30, &settings)
            .await
            .unwrap();
        let final_page = search_audio_tracks_page_raw("night drive", "youtube", 32, 30, &settings)
            .await
            .unwrap();
        assert_eq!(first.entry_count, 30);
        assert_eq!(first.tracks.len(), 30);
        assert_eq!(second.entry_count, 2);
        assert_eq!(second.tracks.len(), 2);
        assert_eq!(second.tracks[0].provider_id, "a0000000030");
        let ids = first
            .tracks
            .iter()
            .chain(second.tracks.iter())
            .map(|track| track.provider_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 32);
        assert_eq!(final_page.entry_count, 0);
        assert!(final_page.tracks.is_empty());
    }

    #[tokio::test]
    async fn concurrent_raw_audio_searches_share_results_and_cache_empty_successes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = RawAudioSearchCache::default();
        let calls = AtomicUsize::new(0);
        let search = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(Vec::<Track>::new())
        };
        let (first, second) = tokio::join!(
            cache.get_or_search("same-query".into(), search),
            cache.get_or_search("same-query".into(), search),
        );
        assert!(first.unwrap().is_empty());
        assert!(second.unwrap().is_empty());
        assert!(cache
            .get_or_search("same-query".into(), search)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn private_networks_rejected() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "100.64.0.1",
            "169.254.1.1",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
    }

    #[cfg(unix)]
    fn poll_once<F: std::future::Future>(
        future: &mut std::pin::Pin<Box<F>>,
    ) -> std::task::Poll<F::Output> {
        struct NoopWake;
        impl std::task::Wake for NoopWake {
            fn wake(self: std::sync::Arc<Self>) {}
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(NoopWake));
        let mut context = std::task::Context::from_waker(&waker);
        future.as_mut().poll(&mut context)
    }

    #[cfg(unix)]
    fn fake_tool(script: &str) -> (tempfile::TempDir, Settings) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-yt-dlp");
        // Publish the executable only after its contents are fully written and
        // its writable handle has closed. This avoids a transient ETXTBSY when
        // parallel tests start the fake tool on filesystems that observe the
        // create/write/execute sequence more strictly.
        let staging = directory.path().join("fake-yt-dlp.staging");
        std::fs::write(&staging, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::rename(staging, &executable).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Default::default()
        };
        (directory, settings)
    }

    #[cfg(target_os = "linux")]
    fn fixture_process_running(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // A killed orphan can remain a zombie until the system reaps it.
        !stat
            .rsplit_once(") ")
            .is_some_and(|(_, fields)| fields.starts_with('Z'))
    }

    #[cfg(target_os = "linux")]
    async fn assert_outer_drop_kills_helper_descendant(parent_exits_first: bool, cancelled: bool) {
        assert_outer_drop_kills_helper_descendant_with_spawn_retry(
            parent_exits_first,
            cancelled,
            false,
        )
        .await;
    }

    #[cfg(target_os = "linux")]
    async fn assert_outer_drop_kills_helper_descendant_with_spawn_retry(
        parent_exits_first: bool,
        cancelled: bool,
        force_busy_spawn: bool,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("processes");
        // Even the unfixed regression fixture self-terminates within two seconds.
        // No persistent orphan survives a failing assertion.
        let script = format!(
            "sleep 2 &\ndescendant=$!\nprintf '%s %s' \"$$\" \"$descendant\" > '{}'\n{}",
            marker.display(),
            if parent_exits_first { "exit 0" } else { "wait" },
        );
        let (_tool_directory, settings) = fake_tool(&script);
        // Holding this executable open for writing deterministically makes
        // Linux reject the first exec with ETXTBSY. Releasing it after the
        // initial poll exercises readiness through the async retry boundary.
        let busy_writer = force_busy_spawn.then(|| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&settings.yt_dlp_path)
                .unwrap()
        });
        let (_inner_stop, inner_cancel) = idle_cancel();
        let inner_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut inner = Box::pin(run_tool_until(
            &settings,
            vec![],
            inner_cancel,
            inner_deadline,
            |_| {},
        ));
        assert!(poll_once(&mut inner).is_pending());
        if force_busy_spawn {
            assert!(!marker.exists(), "busy fixture must not have launched yet");
        }
        drop(busy_writer);
        let pids = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&marker) {
                    let pids = text
                        .split_whitespace()
                        .map(str::parse::<u32>)
                        .collect::<Result<Vec<_>, _>>();
                    if let Ok(pids) = pids {
                        if pids.len() == 2 {
                            break pids;
                        }
                    }
                }
                // The first poll may suspend during the supported spawn retry,
                // before any process exists. Drive it with the real task waker
                // while waiting for readiness rather than assuming Pending
                // proves that the shell already started.
                tokio::select! {
                    result = &mut inner => panic!(
                        "fixture tool completed before readiness (success={})",
                        result.is_ok()
                    ),
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {},
                }
            }
        })
        .await
        .expect("fixture parent and descendant did not start");
        assert!(fixture_process_running(pids[1]));
        if parent_exits_first {
            // Poll once after leader exit, while its descendant still owns the
            // pipes. Reaping here would lose safe process-group ownership.
            tokio::time::timeout(Duration::from_millis(500), async {
                while fixture_process_running(pids[0]) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("fixture leader did not exit before the drop test");
            assert!(poll_once(&mut inner).is_pending());
        }
        let (outer_stop, outer_cancel) = idle_cancel();
        if cancelled {
            outer_stop.send_replace(true);
        }
        let outer_deadline = if cancelled {
            inner_deadline
        } else {
            tokio::time::Instant::now()
        };
        let result = run_playback_stage(outer_cancel, outer_deadline, "outer timeout", inner).await;
        assert_eq!(
            result.unwrap_err(),
            if cancelled {
                "Cancelled"
            } else {
                "outer timeout"
            }
        );
        tokio::time::timeout(Duration::from_millis(500), async {
            while fixture_process_running(pids[0]) || fixture_process_running(pids[1]) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("dropping the nested tool left a fixture process running");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn outer_cancellation_drop_kills_live_helper_process_group() {
        assert_outer_drop_kills_helper_descendant(false, true).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn outer_cancellation_drop_drives_spawn_retry_before_helper_readiness() {
        assert_outer_drop_kills_helper_descendant_with_spawn_retry(false, true, true).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn outer_cancellation_drop_preserves_group_until_descendant_pipes_close() {
        assert_outer_drop_kills_helper_descendant(true, true).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn outer_deadline_drop_kills_helper_process_group() {
        assert_outer_drop_kills_helper_descendant(false, false).await;
    }

    // ETXTBSY while an executable is open for writing is Linux-kernel behavior;
    // other Unix platforms may execute the file and invalidate this fixture.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn run_tool_retries_one_transient_text_file_busy_spawn() {
        use std::fs::OpenOptions;

        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("launched");
        let script = format!("touch '{}'; echo launched", marker.display());
        let (_tool_directory, settings) = fake_tool(&script);
        let writer = OpenOptions::new()
            .write(true)
            .open(&settings.yt_dlp_path)
            .unwrap();
        let error = std::process::Command::new(&settings.yt_dlp_path)
            .spawn()
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ETXTBSY));

        let (_tx, cancel) = idle_cancel();
        let mut run = Box::pin(run_tool(
            &settings,
            vec![],
            cancel,
            Duration::from_secs(1),
            |_| {},
        ));
        assert!(poll_once(&mut run).is_pending(), "expected ETXTBSY backoff");
        drop(writer);
        let output = run.await.unwrap();
        assert!(output.contains("launched"));
        assert!(marker.exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn run_tool_busy_spawn_deadline_is_bounded_and_does_not_launch() {
        use std::fs::OpenOptions;

        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("launched");
        let (_tool_directory, settings) = fake_tool(&format!("touch '{}'", marker.display()));
        let _writer = OpenOptions::new()
            .write(true)
            .open(&settings.yt_dlp_path)
            .unwrap();
        let (_tx, cancel) = idle_cancel();
        let started = Instant::now();
        let mut run = Box::pin(run_tool(
            &settings,
            vec![],
            cancel,
            Duration::from_millis(3),
            |_| {},
        ));
        assert!(poll_once(&mut run).is_pending(), "expected ETXTBSY backoff");
        let result = run.await;
        assert_eq!(result.unwrap_err(), "yt-dlp operation timed out");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nested_search_and_extraction_share_one_absolute_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let search_started = directory.path().join("search-started");
        let extraction_started = directory.path().join("extraction-started");
        let (_tool_directory, settings) = fake_tool(&format!(
            "case \"$*\" in *search*) touch '{}'; sleep 60 ;; *extract*) touch '{}'; echo extracted ;; esac",
            search_started.display(), extraction_started.display()
        ));
        let (_tx, cancel) = idle_cancel();
        let shared_deadline = tokio::time::Instant::now() + Duration::from_millis(80);

        let search = tokio::time::timeout(
            Duration::from_secs(2),
            run_tool_until(
                &settings,
                vec!["search".into()],
                cancel.clone(),
                shared_deadline,
                |_| {},
            ),
        )
        .await
        .expect("nested search must stop at the shared deadline");
        assert_eq!(search.unwrap_err(), "yt-dlp operation timed out");
        assert!(
            search_started.exists(),
            "the slow search fixture should start"
        );

        let extraction = run_tool_until(
            &settings,
            vec!["extract".into()],
            cancel,
            shared_deadline,
            |_| {},
        )
        .await;
        assert_eq!(extraction.unwrap_err(), "yt-dlp operation timed out");
        assert!(
            !extraction_started.exists(),
            "the nested extraction must not receive a fresh timeout"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_deadline_keeps_attempt_cancellation_independent() {
        let directory = tempfile::tempdir().unwrap();
        let started = directory.path().join("started");
        let (_tool_directory, settings) =
            fake_tool(&format!("touch '{}'; sleep 60", started.display()));
        let (cancel_sender, cancel) = idle_cancel();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let worker = tokio::spawn(async move {
            run_tool_until(&settings, vec![], cancel, deadline, |_| {}).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fake tool process did not start");
        cancel_sender.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .expect("cancelled nested operation must stop promptly")
            .unwrap();
        assert_eq!(result.unwrap_err(), "Cancelled");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn run_tool_persistent_busy_spawn_uses_at_most_one_retry() {
        use std::fs::OpenOptions;

        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("launched");
        let (_tool_directory, settings) = fake_tool(&format!("touch '{}'", marker.display()));
        let _writer = OpenOptions::new()
            .write(true)
            .open(&settings.yt_dlp_path)
            .unwrap();
        let (_tx, cancel) = idle_cancel();
        let mut run = Box::pin(run_tool(
            &settings,
            vec![],
            cancel,
            Duration::from_secs(1),
            |_| {},
        ));
        assert!(
            poll_once(&mut run).is_pending(),
            "expected first ETXTBSY backoff"
        );
        let error = run.await.unwrap_err();
        assert!(error.starts_with("Could not start yt-dlp:"), "{error}");
        assert!(!marker.exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn run_tool_cancellation_during_busy_spawn_backoff_prevents_launch() {
        use std::fs::OpenOptions;

        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("launched");
        let (_tool_directory, settings) = fake_tool(&format!("touch '{}'", marker.display()));
        let _writer = OpenOptions::new()
            .write(true)
            .open(&settings.yt_dlp_path)
            .unwrap();
        let (tx, cancel) = idle_cancel();
        let mut run = Box::pin(run_tool(
            &settings,
            vec![],
            cancel,
            Duration::from_secs(1),
            |_| {},
        ));
        assert!(poll_once(&mut run).is_pending(), "expected ETXTBSY backoff");
        tx.send(true).unwrap();
        assert!(
            matches!(poll_once(&mut run), std::task::Poll::Ready(Err(error)) if error == "Cancelled")
        );
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_cancellation_is_prompt() {
        let (_directory, settings) = fake_tool("sleep 60");
        let (tx, rx) = idle_cancel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(true);
        });
        let started = std::time::Instant::now();
        let result = run_tool(&settings, vec![], rx, Duration::from_secs(10), |_| {}).await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(target_os = "linux")]
    fn fake_tool_temp_path(marker: &Path) -> (PathBuf, u32) {
        let contents = std::fs::read_to_string(marker).expect("fake tool marker exists");
        let fields = contents.trim().split('|').collect::<Vec<_>>();
        assert_eq!(fields.len(), 4, "unexpected fake tool marker: {contents:?}");
        assert_eq!(fields[0], fields[1], "TMPDIR and TMP must agree");
        assert_eq!(fields[0], fields[2], "TMPDIR and TEMP must agree");
        let temp_path = PathBuf::from(fields[0]);
        assert_ne!(temp_path, std::env::temp_dir());
        assert!(temp_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("burrata-yt-dlp-")));
        (temp_path, fields[3].parse().unwrap())
    }

    #[cfg(target_os = "linux")]
    fn fake_tool_writing_to_owned_temp(marker: &Path) -> String {
        format!(
            "mkdir -p \"$TMPDIR/_MEI_fixture\"; : > \"$TMPDIR/_MEI_fixture/extracted\"; printf '%s|%s|%s|%s\\n' \"$TMPDIR\" \"$TMP\" \"$TEMP\" \"$$\" > '{}'",
            marker.display()
        )
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn run_tool_removes_owned_temp_directory_after_completion() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("temp-path");
        let (_tool_directory, settings) = fake_tool(&fake_tool_writing_to_owned_temp(&marker));
        let (_tx, cancel) = idle_cancel();
        run_tool(&settings, vec![], cancel, Duration::from_secs(5), |_| {})
            .await
            .unwrap();
        let (temp_path, _) = fake_tool_temp_path(&marker);
        assert!(!temp_path.exists(), "owned temp directory was not removed");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn run_tool_removes_owned_temp_directory_after_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("temp-path");
        let script = format!("{}\nsleep 60", fake_tool_writing_to_owned_temp(&marker));
        let (_tool_directory, settings) = fake_tool(&script);
        let (cancel_sender, cancel) = idle_cancel();
        let worker = tokio::spawn(async move {
            run_tool(&settings, vec![], cancel, Duration::from_secs(10), |_| {}).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fake tool did not create its extraction marker");
        let (temp_path, _) = fake_tool_temp_path(&marker);
        cancel_sender.send(true).unwrap();
        assert_eq!(worker.await.unwrap().unwrap_err(), "Cancelled");
        assert!(!temp_path.exists(), "owned temp directory was not removed");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropped_tool_future_reaps_helper_before_removing_owned_temp_directory() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("temp-path");
        let script = format!("{}\nsleep 60", fake_tool_writing_to_owned_temp(&marker));
        let (_tool_directory, settings) = fake_tool(&script);
        let (_tx, cancel) = idle_cancel();
        let mut run = Box::pin(run_tool(
            &settings,
            vec![],
            cancel,
            Duration::from_secs(10),
            |_| {},
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if marker.exists() {
                    break;
                }
                tokio::select! {
                    result = &mut run => panic!("fake tool finished early: {}", result.is_ok()),
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {},
                }
            }
        })
        .await
        .expect("fake tool did not create its extraction marker");
        let (temp_path, pid) = fake_tool_temp_path(&marker);
        assert!(temp_path.join("_MEI_fixture/extracted").exists());
        drop(run);
        tokio::time::timeout(Duration::from_secs(2), async {
            while temp_path.exists() || fixture_process_running(pid) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("dropped tool future did not reap helper and remove its temp directory");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_timeout_terminates_instead_of_waiting_for_output() {
        let (_directory, settings) = fake_tool("sleep 60");
        let (_tx, rx) = idle_cancel();
        let result = run_tool(&settings, vec![], rx, Duration::from_millis(50), |_| {}).await;
        let error = result.unwrap_err();
        assert!(
            error.contains("timed out"),
            "Unexpected subprocess error: {error}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_progress_and_final_path_are_separate_messages() {
        let (_directory, settings) = fake_tool(
            "printf '%s\\n' 'BURRATA_PROGRESS {\"downloaded_bytes\":5,\"total_bytes\":10}' 'BURRATA_FILE \"/tmp/source.opus\"'",
        );
        let (_tx, rx) = idle_cancel();
        let mut messages = Vec::new();
        let result = run_tool(&settings, vec![], rx, Duration::from_secs(5), |line| {
            messages.push(line.to_string())
        })
        .await
        .unwrap();
        assert_eq!(messages.len(), 2);
        assert!(result.contains("BURRATA_FILE"));
        let progress: Value =
            serde_json::from_str(messages[0].strip_prefix("BURRATA_PROGRESS ").unwrap()).unwrap();
        assert_eq!(progress["downloaded_bytes"], 5);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_exact_version_search_retries_without_version_and_keeps_low_match_uncertain() {
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Radio Edit)","duration":360,"uploader":"Artist One"}]}"#;
        let script = format!(
            "case \"$*\" in *extended\\ mix*) echo '{{\"entries\":[]}}' ;; *) echo '{}' ;; esac",
            output
        );
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let found = find_candidates(&track, &["youtube".into()], &settings)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].confidence, "low");
        assert_eq!(found[0].permission, "unknown");
        assert!(found[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("conflicts with catalog")));
    }

    #[test]
    fn spotify_catalog_recording_has_a_plausible_review_candidate() {
        let track = Track {
            artist: "HUGEL, SOLTO (FR)".into(),
            title: "Jamaican (Bam Bam)".into(),
            duration_seconds: Some(156.0),
            ..Default::default()
        };
        let mut candidate = candidate_from_json(
            &serde_json::json!({
                "id": "hghqd1eBTYQ",
                "url": "https://www.youtube.com/watch?v=hghqd1eBTYQ",
                "title": "Hugel, SOLTO (FR) - Jamaican (Bam Bam) (Original Mix)",
                "uploader": "MoBlack Records",
                "duration": 157
            }),
            "youtube",
        );
        matching::score_candidate(&track, &mut candidate);
        assert!(validate_source_url(&candidate.url, "youtube").is_ok());
        assert!((candidate.score - 0.8).abs() < 0.001);
        assert_eq!(candidate.confidence, "review");
        assert_eq!(candidate.permission, "unknown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn one_provider_failure_does_not_discard_other_provider_candidates() {
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let script = format!(
            "case \"$*\" in *scsearch*) echo 'SoundCloud unavailable' >&2; exit 1 ;; *) echo '{}' ;; esac",
            output
        );
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let found = find_candidates(&track, &["youtube".into(), "soundcloud".into()], &settings)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("Some providers failed")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn independent_web_provider_queries_run_concurrently() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let youtube_marker = directory.path().join("youtube-started");
        let soundcloud_marker = directory.path().join("soundcloud-started");
        let executable = directory.path().join("fake-yt-dlp");
        let script = format!(
            r#"
case "$*" in
  *ytsearch*) own='{}'; other='{}'; result='{{"entries":[{{"id":"yt","url":"https://www.youtube.com/watch?v=yt","title":"Artist - Night Drive","duration":180}}]}}' ;;
  *scsearch*) own='{}'; other='{}'; result='{{"entries":[{{"id":"sc","url":"https://soundcloud.com/artist/night-drive","title":"Artist - Night Drive","duration":180}}]}}' ;;
  *) exit 2 ;;
esac
touch "$own"
i=0
while [ ! -e "$other" ] && [ "$i" -lt 300 ]; do
  sleep 0.01
  i=$((i + 1))
done
[ -e "$other" ] || {{ echo 'provider searches did not overlap' >&2; exit 9; }}
echo "$result"
"#,
            youtube_marker.display(),
            soundcloud_marker.display(),
            soundcloud_marker.display(),
            youtube_marker.display(),
        );
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = Settings {
            yt_dlp_path: executable.to_string_lossy().into(),
            ..Default::default()
        };
        let track = Track {
            artist: "Artist".into(),
            title: "Night Drive".into(),
            duration_seconds: Some(180.0),
            ..Default::default()
        };
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into(), "soundcloud".into()],
            &settings,
            idle_cancel().1,
            3,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preview_search_uses_three_result_query_and_caps_candidates() {
        let entries = (0..4)
            .map(|n| {
                serde_json::json!({
                    "id": format!("track{n}"),
                    "url": format!("https://www.youtube.com/watch?v=track{n}"),
                    "title": format!("Artist One - Night Drive (Mix {n})"),
                    "duration": 360,
                    "uploader": "Artist One"
                })
            })
            .collect::<Vec<_>>();
        let output = serde_json::json!({ "entries": entries }).to_string();
        let script = format!(
            "case \"$*\" in *ytsearch3:*) echo '{}' ;; *) echo '{{\"entries\":[]}}' ;; esac",
            output
        );
        let (_directory, settings) = fake_tool(&script);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let (_tx, cancel) = idle_cancel();
        let found = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &settings,
            cancel,
            3,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 3);
    }

    #[tokio::test]
    async fn preview_probe_error_classes_use_only_reqwest_categories() {
        use std::io::Read;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).unwrap();
            std::thread::sleep(Duration::from_millis(150));
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(25))
            .build()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let output = events.clone();
        let trace = crate::playback_sources::SourceTrace::new(None, None, None)
            .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        let (_cancel_tx, cancel) = watch::channel(false);
        let url = url::Url::parse(&format!("http://{address}/probe-private-url")).unwrap();
        let error = send_preview_range(
            &client,
            &url,
            &[],
            (0, 1023),
            cancel,
            Some(trace),
            "youtube",
            "head",
        )
        .await
        .unwrap_err();
        assert_eq!(error, "Audio stream access check failed");
        let events = events.lock().unwrap();
        let end = events
            .iter()
            .find(|event| event["phase"] == "stream_probe_end")
            .unwrap();
        assert_eq!(end["details"]["errorClass"], "timeout");
        assert!(end["details"]["status"].is_null());
        let serialized = serde_json::to_string(&*events).unwrap();
        assert!(!serialized.contains("probe-private-url"));
        assert!(!serialized.contains("127.0.0.1"));
        drop(events);
        server.join().unwrap();

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_address = closed.local_addr().unwrap();
        drop(closed);
        let connect = reqwest::Client::builder()
            .no_proxy()
            // Windows can retry a refused TCP connection beyond two seconds.
            // Allow the OS error to arrive rather than testing our timeout.
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
            .get(format!("http://{closed_address}/probe"))
            .send()
            .await
            .unwrap_err();
        assert_eq!(preview_probe_error_class(&connect), "connect");

        let request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get("http://127.0.0.1:1/probe")
            .header("not a valid header name", "value")
            .send()
            .await
            .unwrap_err();
        assert_eq!(preview_probe_error_class(&request), "request");
    }

    #[tokio::test]
    async fn preview_probe_error_classifies_truncated_local_response_body() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).unwrap();
            write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-1023/1024\r\nContent-Length: 1024\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n").unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let output = events.clone();
        let trace = crate::playback_sources::SourceTrace::new(None, None, None)
            .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        let url = url::Url::parse(&format!("http://{address}/probe-private-url")).unwrap();
        let (_cancel_tx, mut cancel) = watch::channel(false);
        let response = send_preview_range(
            &client,
            &url,
            &[],
            (0, 1023),
            cancel.clone(),
            Some(trace.clone()),
            "youtube",
            "head",
        )
        .await
        .unwrap();
        let error = validate_preview_range(
            response,
            (0, 1023),
            1024,
            &mut cancel,
            Some(trace),
            "youtube",
            "head",
        )
        .await
        .unwrap_err();
        assert_eq!(error, "Audio stream could not deliver audio bytes");
        let events = events.lock().unwrap();
        let body_end = events
            .iter()
            .find(|event| event["phase"] == "stream_probe_body_end")
            .unwrap();
        assert_eq!(body_end["details"]["errorClass"], "body");
        assert!(!serde_json::to_string(&*events)
            .unwrap()
            .contains("probe-private-url"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn known_length_range_probe_requires_exact_bounded_body_and_honors_cancellation() {
        use std::io::{Read, Write};

        async fn serve(body_headers: &str, body: &str, end: u64) -> Result<(), String> {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let body_headers = body_headers.to_owned();
            let body = body.to_owned();
            let total = end.max(7) + 1;
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0; 2048];
                let _ = socket.read(&mut request).unwrap();
                write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{end}/{total}\r\nContent-Type: audio/mpeg\r\n{body_headers}Connection: close\r\n\r\n{body}").unwrap();
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            let url = url::Url::parse(&format!("http://{address}/probe")).unwrap();
            let (_cancel_tx, mut cancel) = watch::channel(false);
            let response = send_preview_range(
                &client,
                &url,
                &[],
                (0, end),
                cancel.clone(),
                None,
                "youtube",
                "head",
            )
            .await
            .unwrap();
            let result = validate_preview_range(
                response,
                (0, end),
                total,
                &mut cancel,
                None,
                "youtube",
                "head",
            )
            .await;
            server.join().unwrap();
            result
        }

        assert!(
            serve(
                "Transfer-Encoding: chunked\r\n",
                "4\r\nabcd\r\n4\r\nefgh\r\n0\r\n\r\n",
                7
            )
            .await
            .is_ok(),
            "valid multi-chunk range body is accepted"
        );
        assert_eq!(
            serve("Transfer-Encoding: chunked\r\n", "3\r\nabc\r\n0\r\n\r\n", 7)
                .await
                .unwrap_err(),
            "Audio source returned a truncated byte range"
        );
        assert_eq!(
            serve(
                "Transfer-Encoding: chunked\r\n",
                "5\r\nabcde\r\n0\r\n\r\n",
                3
            )
            .await
            .unwrap_err(),
            "Audio source returned an oversized byte range"
        );
        assert_eq!(
            serve("Content-Length: 16\r\n", "", 7).await.unwrap_err(),
            "Audio source returned an inconsistent byte range"
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).unwrap();
            write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-7/64\r\nContent-Length: 8\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n").unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let url = url::Url::parse(&format!("http://{address}/probe")).unwrap();
        let (cancel_tx, mut cancel) = watch::channel(false);
        let response = send_preview_range(
            &client,
            &url,
            &[],
            (0, 7),
            cancel.clone(),
            None,
            "youtube",
            "head",
        )
        .await
        .unwrap();
        cancel_tx.send(true).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            validate_preview_range(response, (0, 7), 64, &mut cancel, None, "youtube", "head"),
        )
        .await
        .expect("cancelled body read must not hang");
        assert_eq!(result.unwrap_err(), "Cancelled");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn preview_access_retries_transient_denial_and_requires_real_bytes() {
        use std::io::{Read, Write};
        for (responses, expected_error) in [
            (vec![(429, "busy"), (206, "audio bytes")], None),
            (vec![(403, "warming"), (206, "audio bytes")], None),
            (
                vec![(403, "warming"), (403, "still warming")],
                Some("HTTP 403"),
            ),
            (vec![(206, "")], Some("empty stream")),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                for (status, body) in responses {
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    let (mut stream, _) = loop {
                        match listener.accept() {
                            Ok(connection) => break connection,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(
                                    std::time::Instant::now() < deadline,
                                    "timed out waiting for range probe"
                                );
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("failed to accept range probe: {error}"),
                        }
                    };
                    // Accepted sockets may inherit the listener nonblocking mode.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = [0; 4096];
                    let count = stream.read(&mut request).unwrap();
                    assert!(String::from_utf8_lossy(&request[..count])
                        .to_lowercase()
                        .contains("range: bytes=0-1023"));
                    write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: audio/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let (_sender, mut cancel) = watch::channel(false);
            let result = verify_preview_audio_access(
                &client,
                &url::Url::parse(&format!("http://{address}/audio")).unwrap(),
                &[],
                None,
                &mut cancel,
            )
            .await;
            if let Some(message) = expected_error {
                assert!(result.unwrap_err().contains(message));
            } else {
                result.unwrap();
            }
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn preview_access_checks_known_length_start_and_tail_ranges() {
        use std::io::{Read, Write};
        const TOTAL_BYTES: u64 = 2 * 1024 * 1024;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut ranges = Vec::new();
            for _ in 0..2 {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "timed out waiting for range probe"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("failed to accept range probe: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
                let range = request
                    .lines()
                    .find_map(|line| line.strip_prefix("range: bytes="))
                    .unwrap()
                    .to_owned();
                ranges.push(range.clone());
                let (start, end) = range.split_once('-').unwrap();
                let (start, end) = (start.parse::<u64>().unwrap(), end.parse::<u64>().unwrap());
                let content_len = (end - start + 1) as usize;
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mp4\r\nContent-Range: bytes {start}-{end}/{TOTAL_BYTES}\r\nContent-Length: {content_len}\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                stream.write_all(&vec![b'a'; content_len]).unwrap();
            }
            ranges
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let (_sender, mut cancel) = watch::channel(false);
        verify_preview_audio_access(
            &client,
            &url::Url::parse(&format!("http://{address}/audio")).unwrap(),
            &[("User-Agent".into(), "AudioClient/1.0".into())],
            Some(TOTAL_BYTES),
            &mut cancel,
        )
        .await
        .unwrap();
        let mut ranges = server.join().unwrap();
        ranges.sort();
        assert_eq!(ranges, vec!["0-1023", "2096128-2097151"]);
    }

    #[test]
    fn fresh_stream_forbidden_gets_longer_bounded_retry_than_throttling() {
        assert_eq!(
            preview_access_retry_delay(reqwest::StatusCode::FORBIDDEN),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            preview_access_retry_delay(reqwest::StatusCode::TOO_MANY_REQUESTS),
            Some(Duration::from_millis(300))
        );
        assert_eq!(
            preview_access_retry_delay(reqwest::StatusCode::BAD_GATEWAY),
            Some(Duration::from_millis(300))
        );
        assert_eq!(
            preview_access_retry_delay(reqwest::StatusCode::NOT_FOUND),
            None
        );
    }

    #[tokio::test]
    async fn preview_access_probe_reuses_safe_stream_headers() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "timed out waiting for header probe"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("failed to accept header probe: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]).to_lowercase();
                assert!(request.contains("range: bytes=0-1023"));
                let headers_present = request.contains("user-agent: audioclient/1.0")
                    && request.contains("referer: https://www.youtube.com/watch?v=public-id");
                if headers_present {
                    write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mp4\r\nContent-Length: 10\r\nConnection: close\r\n\r\naudio bytes").unwrap();
                } else {
                    write!(stream, "HTTP/1.1 403 Forbidden\r\nContent-Length: 7\r\nConnection: close\r\n\r\nblocked").unwrap();
                }
            }
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let headers = vec![
            ("User-Agent".into(), "AudioClient/1.0".into()),
            (
                "Referer".into(),
                "https://www.youtube.com/watch?v=public-id".into(),
            ),
        ];
        let (_sender, mut cancel) = watch::channel(false);
        let url = url::Url::parse(&format!("http://{address}/audio")).unwrap();
        assert!(
            verify_preview_audio_access(&client, &url, &[], None, &mut cancel)
                .await
                .unwrap_err()
                .contains("HTTP 403")
        );
        verify_preview_audio_access(&client, &url, &headers, None, &mut cancel)
            .await
            .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn preview_audio_metadata_requires_audio_only_and_trusted_public_hosts() {
        let candidate = Candidate {
            id: "youtube:hghqd1eBTYQ".into(),
            provider: "youtube".into(),
            ..Default::default()
        };
        let info = serde_json::json!({
            "id":"hghqd1eBTYQ",
            "url":"https://rr5---sn.googlevideo.com/videoplayback?signature=redacted",
            "ext":"m4a",
            "protocol":"https",
            "format_id":"140",
            "acodec":"mp4a.40.2",
            "vcodec":"none",
            "duration":156,
            "filesize":4096
        });
        let audio = preview_audio_from_metadata(&candidate, &info).unwrap();
        assert_eq!(audio.mime_type, "audio/mp4");
        assert_eq!(audio.duration_seconds, Some(156.0));
        assert_eq!(audio.expected_content_length, Some(4096));
        assert_eq!(audio.selected_format_id.as_deref(), Some("140"));
        assert!(!audio.selected_format_has_video);

        let muxed = preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({
                "id":"hghqd1eBTYQ",
                "url":"https://rr5---sn.googlevideo.com/video-mp4?signature=redacted",
                "ext":"mp4",
                "protocol":"https",
                "format_id":"18",
                "acodec":"mp4a.40.2",
                "vcodec":"avc1",
                "duration":156,
                "filesize":10000
            }),
        )
        .unwrap();
        assert_eq!(muxed.selected_format_id.as_deref(), Some("18"));
        assert!(muxed.selected_format_has_video);
        assert!(muxed.expected_content_length.is_some());
        assert_eq!(safe_extractor_format_id(Some("bad format/token")), None);
        let soundcloud = Candidate {
            id: "soundcloud:123".into(),
            provider: "soundcloud".into(),
            ..Default::default()
        };
        assert!(preview_audio_from_metadata(
            &soundcloud,
            &serde_json::json!({"id":"123","url":"https://cf-media.sndcdn.com/audio.m3u8","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).is_ok());
        let migrated_soundcloud = Candidate {
            id: "soundcloud:1998651815".into(),
            provider: "soundcloud".into(),
            ..Default::default()
        };
        let hls_audio = preview_audio_from_metadata(
            &migrated_soundcloud,
            &serde_json::json!({"id":"1998651815","url":"https://playback.media-streaming.soundcloud.cloud/trackid/aac_160k/uuid/playlist.m3u8?token=redacted","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).unwrap();
        assert_eq!(hls_audio.mime_type, "application/vnd.apple.mpegurl");
        assert!(preview_audio_from_metadata(
            &migrated_soundcloud,
            &serde_json::json!({"id":"1998651815","url":"https://user:pass@playback.media-streaming.soundcloud.cloud/trackid/aac_160k/uuid/playlist.m3u8","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).is_err());
        assert!(preview_audio_from_metadata(
            &migrated_soundcloud,
            &serde_json::json!({"id":"1998651815","url":"https://192.168.1.10/trackid/aac_160k/uuid/playlist.m3u8","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).is_err());
        assert!(preview_audio_from_metadata(
            &soundcloud,
            &serde_json::json!({"id":"123","url":"https://attacker.example/audio.m3u8","ext":"m4a","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"none"})
        ).is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://rr5---sn.googlevideo.com/a","ext":"mp4","acodec":"mp4a.40.2","vcodec":"avc1"})
        )
        .is_err());
        let muxed_youtube = preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({
                "id":"hghqd1eBTYQ",
                "url":"https://rr5---sn.googlevideo.com/videoplayback?signature=redacted",
                "ext":"mp4",
                "protocol":"https",
                "acodec":"mp4a.40.2",
                "vcodec":"avc1.42001E",
                "abr":96.5,
                "tbr":512.0,
                "filesize":4436187
            }),
        )
        .unwrap();
        assert_eq!(muxed_youtube.mime_type, "audio/mp4");
        assert_eq!(muxed_youtube.codec.as_deref(), Some("mp4a.40.2"));
        assert_eq!(muxed_youtube.bitrate_kbps, Some(96.5));
        assert_eq!(muxed_youtube.expected_content_length, Some(4436187));
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://rr5---sn.googlevideo.com/videoplayback","ext":"mp4","protocol":"https","acodec":"none","vcodec":"avc1.42001E"})
        )
        .is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"different","url":"https://rr5---sn.googlevideo.com/videoplayback","ext":"mp4","protocol":"https","acodec":"mp4a.40.2","vcodec":"avc1.42001E"})
        )
        .is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://attacker.example/videoplayback","ext":"mp4","protocol":"https","acodec":"mp4a.40.2","vcodec":"avc1.42001E"})
        )
        .is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://rr5---sn.googlevideo.com/videoplayback","ext":"mp4","protocol":"m3u8_native","acodec":"mp4a.40.2","vcodec":"avc1.42001E"})
        )
        .is_err());
        assert!(preview_audio_from_metadata(
            &soundcloud,
            &serde_json::json!({"id":"123","url":"https://cf-media.sndcdn.com/audio.mp4","ext":"mp4","protocol":"https","acodec":"mp4a.40.2","vcodec":"avc1"})
        )
        .is_err());
        assert!(preview_audio_from_metadata(
            &candidate,
            &serde_json::json!({"id":"hghqd1eBTYQ","url":"https://attacker.example/a","ext":"mp4","acodec":"mp4a.40.2","vcodec":"none"})
        )
        .is_err());
    }

    #[test]
    fn prepared_alternate_preserves_recording_identity_and_uses_its_own_format_fields() {
        let candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let info = serde_json::json!({
            "id":"abcdefghijk", "title":"Exact Recording", "artist":"Exact Artist", "duration":139.0,
            "url":"https://media.googlevideo.com/aac?expire=4102444800", "acodec":"mp4a.40.2", "vcodec":"none", "ext":"m4a", "protocol":"https", "format_id":"140", "filesize":123,
            "formats":[{"url":"https://media.googlevideo.com/opus?expire=4102444800", "acodec":"opus", "vcodec":"none", "ext":"webm", "protocol":"https", "format_id":"251", "abr":128, "filesize":456, "title":"Nested spoof", "id":"other-source"}]
        });
        let selected = preview_audio_from_metadata(&candidate, &info).unwrap();
        let prepared = prepared_audio_alternate(&candidate, &info, &selected).unwrap();
        assert_eq!(
            prepared.resolved_provider_id.as_deref(),
            Some("abcdefghijk")
        );
        assert_eq!(prepared.resolved_title.as_deref(), Some("Exact Recording"));
        assert_eq!(prepared.duration_seconds, Some(139.0));
        assert_eq!(prepared.selected_format_id.as_deref(), Some("251"));
        assert_eq!(prepared.expected_content_length, Some(456));
        assert_eq!(prepared.codec.as_deref(), Some("opus"));
        assert!(!prepared.selected_format_has_video);
        let mut missing_size = info.clone();
        missing_size["formats"][0]
            .as_object_mut()
            .unwrap()
            .remove("filesize");
        assert_eq!(
            prepared_audio_alternate(&candidate, &missing_size, &selected)
                .unwrap()
                .expected_content_length,
            None
        );
    }

    #[test]
    fn prepared_alternate_rejects_video_manifests_foreign_hosts_expiry_and_changed_identity() {
        let candidate = Candidate {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let base = serde_json::json!({"id":"abcdefghijk", "title":"Recording", "duration":139,
            "url":"https://media.googlevideo.com/aac", "acodec":"mp4a.40.2", "vcodec":"none", "ext":"m4a", "protocol":"https", "format_id":"140",
            "formats":[{"url":"https://media.googlevideo.com/opus", "acodec":"opus", "vcodec":"none", "ext":"webm", "protocol":"https", "format_id":"251"}]});
        let selected = preview_audio_from_metadata(&candidate, &base).unwrap();
        for (field, value) in [
            ("vcodec", "avc1"),
            ("protocol", "m3u8_native"),
            ("url", "https://attacker.example/opus"),
            ("url", "https://media.googlevideo.com/opus?expire=1"),
            ("url", "http://media.googlevideo.com/opus"),
            ("acodec", "none"),
        ] {
            let mut bad = base.clone();
            bad["formats"][0][field] = serde_json::json!(value);
            assert!(
                prepared_audio_alternate(&candidate, &bad, &selected).is_none(),
                "{field}: {value}"
            );
        }
        let mut changed = base.clone();
        changed["id"] = serde_json::json!("changed");
        assert!(prepared_audio_alternate(&candidate, &changed, &selected).is_none());
    }

    #[tokio::test]
    async fn prepared_alternate_cancel_and_deadline_stop_before_network() {
        let candidate = Candidate {
            provider: "youtube".into(),
            ..Candidate::default()
        };
        let audio=preview_audio_from_metadata(&candidate,&serde_json::json!({"url":"https://media.googlevideo.com/opus","acodec":"opus","vcodec":"none","ext":"webm","protocol":"https"})).unwrap();
        let (tx, cancel) = watch::channel(false);
        tx.send_replace(true);
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(1));
        assert_eq!(
            validate_prepared_preview_audio(audio.clone(), cancel, None, "youtube", deadline)
                .await
                .unwrap_err(),
            "Cancelled"
        );
        let (_tx, cancel) = idle_cancel();
        let expired = crate::playback_sources::PlaybackDeadline::at(
            tokio::time::Instant::now() - Duration::from_secs(1),
        );
        assert_eq!(
            validate_prepared_preview_audio(audio, cancel, None, "youtube", expired)
                .await
                .unwrap_err(),
            "Audio stream resolution timed out"
        );
    }

    #[test]
    fn alternate_preview_audio_format_prefers_webm_then_m4a_without_changing_default() {
        assert_eq!(
            preview_audio_format_selector(false, false),
            PREVIEW_AUDIO_FORMAT_SELECTOR
        );
        let alternate = preview_audio_format_selector(true, false);
        assert!(alternate.starts_with("bestaudio[ext=webm]"));
        assert!(alternate.contains("/bestaudio[ext=m4a]"));

        let youtube = preview_audio_format_selector(false, true);
        assert!(youtube.starts_with("bestaudio[ext=m4a]"));
        assert!(youtube.find("/bestaudio").unwrap() < youtube.find("/worst[ext=mp4]").unwrap());
        assert!(youtube.contains("[acodec!=none][vcodec!=none]"));
        let youtube_alternate = preview_audio_format_selector(true, true);
        assert!(youtube_alternate.starts_with("bestaudio[ext=webm]"));
        assert!(
            youtube_alternate.find("/bestaudio").unwrap()
                < youtube_alternate.find("/worst[ext=mp4]").unwrap()
        );
        assert!(!preview_audio_format_selector(false, false).contains("/worst[ext=mp4]"));
        assert_eq!(
            ALBUM_EVIDENCE_AUDIO_SELECTOR,
            "worstaudio[ext=m4a][protocol^=http][protocol!*=dash]"
        );
        assert!(!ALBUM_EVIDENCE_AUDIO_SELECTOR.contains("bestaudio"));
    }

    #[cfg(unix)]
    #[test]
    fn album_evidence_failure_diagnostics_keep_only_safe_format_facts() {
        let audio = PreviewAudio {
            provider_session: None,
            url: "https://media.googlevideo.com/audio?signature=private-secret".into(),
            mime_type: "audio/mp4".into(),
            codec: Some("mp4a.40.5".into()),
            bitrate_kbps: Some(48.784),
            duration_seconds: Some(2499.0),
            selected_format_id: Some("139".into()),
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: Some("private source title".into()),
            resolved_provider_id: Some("abcdefghijk".into()),
            chapters: None,
            expected_content_length: None,
            headers: vec![("Authorization".into(), "private".into())],
        };
        let diagnostic = album_evidence_audio_diagnostic("access_rejected", &audio);
        assert_eq!(diagnostic["code"], "access_rejected");
        assert_eq!(diagnostic["formatId"], "139");
        assert_eq!(diagnostic["codec"], "mp4a.40.5");
        assert_eq!(diagnostic["durationSeconds"], 2499.0);
        assert_eq!(diagnostic["bitrateKbps"], 48.784);
        let serialized = diagnostic.to_string();
        assert!(!serialized.contains("private-secret"));
        assert!(!serialized.contains("private source title"));
        assert!(!serialized.contains("Authorization"));

        let denied = album_evidence_access_diagnostic(
            "access_rejected",
            "Audio source denied stream access (HTTP 403)",
            &audio,
        );
        assert_eq!(denied["accessCode"], "http_status");
        assert_eq!(denied["httpStatus"], 403);
        assert!(!denied.to_string().contains("HTTP 403"));
        let range = album_evidence_access_diagnostic(
            "access_rejected",
            "Audio source returned an inconsistent byte range",
            &audio,
        );
        assert_eq!(range["accessCode"], "range_rejected");
        assert!(range.get("httpStatus").is_none());
        let unknown = album_evidence_access_diagnostic(
            "access_rejected",
            "untrusted response with private payload",
            &audio,
        );
        assert_eq!(unknown["accessCode"], "access_check_failed");
        assert!(!unknown.to_string().contains("private payload"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn album_evidence_extraction_selects_fresh_low_bitrate_aac_139() {
        let directory = tempfile::tempdir().unwrap();
        let args_path = directory.path().join("format-args");
        let args_path_display = args_path.to_string_lossy();
        let script = format!(
            "printf '%s\\n' \"$@\" > '{args_path_display}'\ncat <<'JSON'\n{{\"id\":\"evidence139\",\"title\":\"Fixture Album full soundtrack\",\"duration\":2500,\"url\":\"https://rr1.googlevideo.com/videoplayback?token=private\",\"protocol\":\"https\",\"ext\":\"m4a\",\"acodec\":\"mp4a.40.5\",\"vcodec\":\"none\",\"format_id\":\"139\",\"abr\":48,\"filesize\":4096}}\nJSON"
        );
        let (_tool_dir, settings) = fake_tool(&script);
        let candidate = Candidate {
            id: "youtube:evidence139".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=evidence139".into(),
            ..Candidate::default()
        };
        let audio = with_playback_test_public_addresses(extract_album_evidence_preview_audio(
            &candidate,
            &settings,
            idle_cancel().1,
        ))
        .await
        .unwrap();
        assert_eq!(audio.selected_format_id.as_deref(), Some("139"));
        assert_eq!(audio.codec.as_deref(), Some("mp4a.40.5"));
        assert_eq!(audio.bitrate_kbps, Some(48.0));
        let args = std::fs::read_to_string(args_path).unwrap();
        let format_position = args
            .lines()
            .position(|argument| argument == "--format")
            .expect("evidence extraction must specify a format selector");
        assert_eq!(
            args.lines().nth(format_position + 1),
            Some(ALBUM_EVIDENCE_AUDIO_SELECTOR)
        );
    }

    #[test]
    fn preview_audio_headers_keep_only_safe_allowlisted_values() {
        let headers = preview_audio_headers(&serde_json::json!({
            "http_headers": {
                "user-agent": "AudioClient/1.0",
                "Referer": "https://www.youtube.com/watch?v=public-id",
                "Origin": "https://www.youtube.com",
                "Accept-Language": "en-US,en;q=0.9",
                "Cookie": "secret=session",
                "Authorization": "Bearer secret",
                "X-Api-Key": "secret",
                "X-Newline": "safe\r\nCookie: secret"
            }
        }));
        assert_eq!(
            headers,
            vec![
                ("User-Agent".into(), "AudioClient/1.0".into()),
                (
                    "Referer".into(),
                    "https://www.youtube.com/watch?v=public-id".into()
                ),
                ("Origin".into(), "https://www.youtube.com".into()),
                ("Accept-Language".into(), "en-US,en;q=0.9".into()),
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canonical_playback_warmup_reuses_first_query_and_leaves_alias_slot() {
        let directory = tempfile::tempdir().unwrap();
        let calls = directory.path().join("calls.txt");
        let output = r#"{"entries":[{"id":"canonical","url":"https://www.youtube.com/watch?v=canonical","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let unrelated = r#"{"entries":[{"id":"unrelated","url":"https://www.youtube.com/watch?v=unrelated","title":"Other Artist - Different Recording","duration":360,"uploader":"Other Artist"}]}"#;
        let (_tool_directory, settings) = fake_tool(&format!(
            "printf '%s\\n' \"$*\" >> '{}'; case \"$*\" in *'Verified Alias'*) cat <<'JSON'\n{}\nJSON\n;; *) cat <<'JSON'\n{}\nJSON\n;; esac",
            calls.display(),
            output,
            unrelated
        ));
        let store = Arc::new(Store::open(&directory.path().join("canonical-warm.sqlite")).unwrap());
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let manager = AcquisitionManager::new_for_playback_test(store, connections);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Track::default()
        };
        let (_stop, cancel) = idle_cancel();
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(10));
        assert_eq!(
            manager
                .warm_canonical_playback_query_cancelled(
                    &track,
                    "youtube",
                    &settings,
                    cancel.clone(),
                    deadline
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 1);

        let (rows, complete) = find_candidates_with_cancel_until_alias_detailed(
            &track,
            &["youtube".into()],
            &settings,
            cancel,
            PLAYBACK_SOURCE_SEARCH_RESULT_LIMIT,
            PREVIEW_SEARCH_TIMEOUT,
            Some(deadline),
            Some("Verified Alias".into()),
            Some(&manager.inner.search_cache),
        )
        .await
        .unwrap();
        assert!(complete);
        assert!(!rows.is_empty());
        // Foreground joins the cached canonical query and spends exactly one
        // ordinary alias query. The warm phase performs no extraction.
        assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 2);
        assert!(std::fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .all(|args| args.contains("--flat-playlist") && args.contains("--skip-download")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_candidate_search_uses_cached_fake_subprocess_result() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let (_directory, settings) = fake_tool(&format!("echo '{}'", output));
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let providers = vec!["youtube".to_string()];
        let calls = AtomicUsize::new(0);
        let cache = CandidateSearchCache::default();
        for _ in 0..2 {
            let found = cache
                .get_or_search("same-track-and-provider".into(), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    find_candidates(&track, &providers, &settings)
                })
                .await
                .unwrap();
            assert_eq!(found.len(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn traced_playback_candidate_cache_reports_observed_miss_then_hit_only() {
        use std::sync::{Arc, Mutex};
        let cache = CandidateSearchCache::default();
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let trace = || {
            let events = events.clone();
            crate::playback_sources::SourceTrace::new(None, None, Some(7))
                .with_event_provider("youtube")
                .with_test_sink(Arc::new(move |value| events.lock().unwrap().push(value)))
        };
        let (_tx, cancel) = idle_cancel();
        let found = cache
            .get_or_search_cancelled_traced(
                "private-playback-key".into(),
                cancel.clone(),
                trace(),
                || async {
                    Ok(vec![Candidate {
                        id: "youtube:fixture".into(),
                        ..Candidate::default()
                    }])
                },
            )
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            events.lock().unwrap()[0]["phase"],
            "playback_candidate_search_cache_miss"
        );
        assert_eq!(events.lock().unwrap()[0]["details"]["waitedForGate"], false);
        assert!(events.lock().unwrap()[0]["details"]["observedElapsedMs"].is_number());

        let hit = cache
            .get_or_search_cancelled_traced(
                "private-playback-key".into(),
                cancel,
                trace(),
                || async { Err("cache-hit-loader-was-invoked".into()) },
            )
            .await
            .unwrap();
        assert_eq!(hit.len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events[1]["phase"], "playback_candidate_search_cache_hit");
        assert_eq!(events[1]["details"]["resultCount"], 1);
        assert!(!events[1].to_string().contains("private-playback-key"));
    }

    #[tokio::test]
    async fn traced_playback_candidate_cache_distinguishes_waited_hit() {
        use std::sync::{Arc, Mutex};
        use tokio::sync::oneshot;
        let cache = Arc::new(CandidateSearchCache::default());
        let (owner_started_tx, owner_started_rx) = oneshot::channel();
        let (release_owner_tx, release_owner_rx) = oneshot::channel();
        let owner_cache = cache.clone();
        let owner = tokio::spawn(async move {
            owner_cache
                .get_or_search("waited-hit-key".into(), || async move {
                    let _ = owner_started_tx.send(());
                    let _ = release_owner_rx.await;
                    Ok(vec![Candidate {
                        id: "youtube:fixture".into(),
                        ..Candidate::default()
                    }])
                })
                .await
        });
        owner_started_rx.await.unwrap();

        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let traced_events = events.clone();
        let trace = crate::playback_sources::SourceTrace::new(None, None, Some(9))
            .with_event_provider("youtube")
            .with_test_sink(Arc::new(move |value| {
                traced_events.lock().unwrap().push(value)
            }));
        let (_tx, cancel) = idle_cancel();
        let waiter = cache.get_or_search_cancelled_traced(
            "waited-hit-key".into(),
            cancel,
            trace,
            || async { Err("waited-cache-hit-loader-was-invoked".into()) },
        );
        tokio::pin!(waiter);
        assert!(matches!(
            futures_util::poll!(waiter.as_mut()),
            std::task::Poll::Pending
        ));
        release_owner_tx.send(()).unwrap();
        assert_eq!(owner.await.unwrap().unwrap().len(), 1);
        assert_eq!(waiter.await.unwrap().len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0]["phase"],
            "playback_candidate_search_cache_waited_hit"
        );
        assert_eq!(events[0]["details"]["resultCount"], 1);
        assert!(!events[0].to_string().contains("waited-hit-key"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_preview_cache_misses_share_one_search() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let output = r#"{"entries":[{"id":"x","url":"https://www.youtube.com/watch?v=x","title":"Artist One - Night Drive (Extended Mix)","duration":360,"uploader":"Artist One"}]}"#;
        let (_directory, settings) = fake_tool(&format!("sleep 0.05; echo '{}'", output));
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            duration_seconds: Some(360.0),
            ..Default::default()
        };
        let providers = vec!["youtube".to_string()];
        let calls = AtomicUsize::new(0);
        let cache = CandidateSearchCache::default();
        let first = cache.get_or_search("same-preview-key".into(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            find_candidates(&track, &providers, &settings)
        });
        let second = cache.get_or_search("same-preview-key".into(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            find_candidates(&track, &providers, &settings)
        });
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().len(), 1);
        assert_eq!(second.unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_speculative_search_owner_leaves_foreground_waiter_independent() {
        use tokio::sync::oneshot;
        let cache = Arc::new(CandidateSearchCache::default());
        let (cancel_speculative, speculative_cancel) = watch::channel(false);
        let (started, owner_started) = oneshot::channel();
        let owner_cache = cache.clone();
        let owner = tokio::spawn(async move {
            owner_cache
                .get_or_search_cancelled(
                    "upcoming-playback-search".into(),
                    speculative_cancel.clone(),
                    || async move {
                        let _ = started.send(());
                        crate::playback_sources::wait_for_cancellation(speculative_cancel).await;
                        Err("Cancelled".into())
                    },
                )
                .await
        });
        owner_started.await.unwrap();
        let foreground_cache = cache.clone();
        let (_foreground_sender, foreground_cancel) = idle_cancel();
        let foreground = tokio::spawn(async move {
            foreground_cache
                .get_or_search_cancelled(
                    "upcoming-playback-search".into(),
                    foreground_cancel,
                    || async {
                        Ok(vec![Candidate {
                            id: "independent-foreground-result".into(),
                            ..Candidate::default()
                        }])
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        cancel_speculative.send(true).unwrap();
        assert_eq!(owner.await.unwrap().unwrap_err(), "Cancelled");
        let result = tokio::time::timeout(Duration::from_secs(1), foreground)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result[0].id, "independent-foreground-result");
        assert_eq!(
            cache
                .get_or_search("upcoming-playback-search".into(), || async {
                    panic!("foreground result must remain cached")
                })
                .await
                .unwrap()[0]
                .id,
            "independent-foreground-result"
        );
    }

    #[tokio::test]
    async fn cancelled_candidate_cache_waiter_does_not_cancel_owner_or_lose_result() {
        use tokio::sync::oneshot;

        let cache = Arc::new(CandidateSearchCache::default());
        let owner_cache = cache.clone();
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let owner = tokio::spawn(async move {
            owner_cache
                .get_or_search("shared-candidate-key".into(), || async move {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    Ok(Vec::<Candidate>::new())
                })
                .await
        });
        started_rx.await.unwrap();

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let waiter_cache = cache.clone();
        let waiter = tokio::spawn(async move {
            waiter_cache
                .get_or_search_cancelled("shared-candidate-key".into(), cancel_rx, || async {
                    panic!("a waiter must not become the search owner after cancellation")
                })
                .await
        });
        tokio::task::yield_now().await;
        cancel_tx.send(true).unwrap();
        let cancelled = tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .expect("cancelled waiter should leave the cache gate promptly")
            .unwrap();
        assert_eq!(cancelled.unwrap_err(), "Cancelled");
        assert!(
            !owner.is_finished(),
            "waiter cancellation must not cancel owner"
        );

        release_tx.send(()).unwrap();
        assert!(owner.await.unwrap().unwrap().is_empty());
        let cached = cache
            .get_or_search("shared-candidate-key".into(), || async {
                panic!("the completed owner's result should remain cached")
            })
            .await
            .unwrap();
        assert!(cached.is_empty());
    }

    #[tokio::test]
    async fn concurrent_preview_audio_resolutions_share_one_lookup() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let cache = PreviewAudioCache::default();
        let resolve = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(PreviewAudio {
                provider_session: None,
                url: "https://rr5---sn.googlevideo.com/audio".into(),
                mime_type: "audio/mp4".into(),
                codec: None,
                bitrate_kbps: None,
                duration_seconds: Some(156.0),
                selected_format_id: None,
                selected_format_has_video: false,
                source_metadata: None,
                resolved_title: None,
                resolved_provider_id: None,
                chapters: None,
                expected_content_length: None,
                headers: vec![],
            })
        };
        let first = cache.get_or_resolve("same-candidate".into(), resolve);
        let second = cache.get_or_resolve("same-candidate".into(), resolve);
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().mime_type, "audio/mp4");
        assert_eq!(second.unwrap().duration_seconds, Some(156.0));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn preview_audio_cache_hit_is_correlated_but_emits_no_monochrome_probe() {
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let output = events.clone();
        let trace = crate::playback_sources::SourceTrace::new(
            Some("native-fruit-start".into()),
            None,
            None,
        )
        .with_event_provider("monochrome")
        .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        let cache = PreviewAudioCache::default();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let audio = || PreviewAudio {
            provider_session: None,
            url: "https://tracks.monochrome.st/track/239910648984977408".into(),
            mime_type: "audio/flac".into(),
            codec: Some("flac".into()),
            bitrate_kbps: None,
            duration_seconds: Some(181.4),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: None,
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
        };

        let worker_trace = trace.clone();
        let probe_cancel = cancel_rx.clone();
        cache
            .get_or_resolve_cancelled(
                "pinned-monochrome-recording".into(),
                cancel_rx.clone(),
                Some(trace.clone()),
                move |lookup_id| async move {
                    traced_monochrome_probe(
                        Some(worker_trace.with_shared_lookup(lookup_id)),
                        probe_cancel,
                        || async { Ok(crate::models::Candidate::default()) },
                    )
                    .await
                    .unwrap();
                    Ok(audio())
                },
            )
            .await
            .unwrap();
        let first_events = events.lock().unwrap().len();

        cache
            .get_or_resolve_cancelled(
                "pinned-monochrome-recording".into(),
                cancel_rx,
                Some(trace),
                |_| async { panic!("cache hit must not invoke the probe worker") },
            )
            .await
            .unwrap();
        drop(cancel_tx);

        let events = events.lock().unwrap();
        let second = &events[first_events..];
        assert!(second
            .iter()
            .any(|event| event["phase"] == "preview_audio_cache_hit"));
        assert!(!second
            .iter()
            .any(|event| event["phase"] == "monochrome_probe_start"));
        assert!(!second
            .iter()
            .any(|event| event["phase"] == "monochrome_probe_end"));
        assert!(second
            .iter()
            .all(|event| event["nativeOperationId"] == "native-fruit-start"));
        assert!(second.iter().all(|event| event["provider"] == "monochrome"));
    }

    #[tokio::test]
    async fn monochrome_probe_trace_marks_explicit_cancel_and_outer_deadline_drop() {
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let output = events.clone();
        let trace = crate::playback_sources::SourceTrace::new(
            Some("native-fruit-start".into()),
            None,
            None,
        )
        .with_event_provider("monochrome")
        .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        let lookup_id = uuid::Uuid::new_v4();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let cancel_trace = trace.clone().with_shared_lookup(lookup_id);
        let cancelled_probe = tokio::spawn(async move {
            traced_monochrome_probe(Some(cancel_trace), cancel_rx, || async move {
                let _ = started_tx.send(());
                std::future::pending::<Result<crate::models::Candidate, String>>().await
            })
            .await
        });
        started_rx.await.unwrap();
        cancel_tx.send(true).unwrap();
        assert_eq!(cancelled_probe.await.unwrap().unwrap_err(), "Cancelled");

        let (deadline_sender, deadline_cancel) = watch::channel(false);
        let deadline_trace = trace.clone().with_shared_lookup(lookup_id);
        let deadline = tokio::time::timeout(
            Duration::from_millis(5),
            traced_monochrome_probe(Some(deadline_trace.clone()), deadline_cancel, || async {
                std::future::pending::<Result<crate::models::Candidate, String>>().await
            }),
        )
        .await;
        assert!(
            deadline.is_err(),
            "fixture must exercise future-drop cleanup"
        );
        deadline_trace.event(
            "resolution_end",
            "monochrome",
            serde_json::json!({"deadline":true,"cancelled":false}),
        );
        drop(deadline_sender);

        let events = events.lock().unwrap();
        let endings = events
            .iter()
            .filter(|event| event["phase"] == "monochrome_probe_end")
            .map(|event| event["details"]["outcome"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(endings, ["cancelled", "abandoned"]);
        assert!(events.iter().any(|event| {
            event["phase"] == "resolution_end" && event["details"]["deadline"] == true
        }));
        assert!(events.iter().all(|event| {
            event["nativeOperationId"] == "native-fruit-start"
                && event["sharedLookupId"] == lookup_id.to_string()
                && event["provider"] == "monochrome"
                && event.get("url").is_none()
                && event.get("headers").is_none()
                && event.get("title").is_none()
        }));
    }

    #[tokio::test]
    async fn failed_stream_cache_entry_can_be_invalidated_before_same_source_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let cache = PreviewAudioCache::default();
        let resolve = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(PreviewAudio {
                provider_session: None,
                url: "https://rr5---sn.googlevideo.com/audio?expire=4102444800".into(),
                mime_type: "audio/mp4".into(),
                codec: None,
                bitrate_kbps: None,
                duration_seconds: Some(156.0),
                selected_format_id: None,
                selected_format_has_video: false,
                source_metadata: None,
                resolved_title: None,
                resolved_provider_id: None,
                chapters: None,
                expected_content_length: None,
                headers: vec![(
                    "Referer".into(),
                    "https://www.youtube.com/watch?v=id".into(),
                )],
            })
        };
        let first = cache
            .get_or_resolve("same-source".into(), resolve)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        cache.invalidate("same-source");
        let second = cache
            .get_or_resolve("same-source".into(), resolve)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(first.url, second.url);
        assert_eq!(second.headers[0].0, "Referer");
    }

    #[tokio::test]
    async fn invalidation_during_lookup_cannot_restore_a_stale_stream() {
        let cache = PreviewAudioCache::default();
        let audio = |url: &str| PreviewAudio {
            provider_session: None,
            url: url.into(),
            mime_type: "audio/mp4".into(),
            codec: None,
            bitrate_kbps: None,
            duration_seconds: Some(100.),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: None,
            chapters: None,
            expected_content_length: None,
            headers: vec![],
        };
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let stale = cache.get_or_resolve("source".into(), || async {
            let _ = started.send(());
            released.await.unwrap();
            Ok(audio("https://cdn.example/expired"))
        });
        tokio::pin!(stale);
        tokio::select! {
            _ = &mut stale => panic!("lookup must wait for invalidation"),
            _ = waiting => {},
        }
        cache.invalidate("source");
        release.send(()).unwrap();
        assert_eq!(
            stale.await.unwrap_err(),
            "Audio stream was invalidated during resolution"
        );
        let next = cache
            .get_or_resolve("source".into(), || async {
                Ok(audio("https://cdn.example/fresh"))
            })
            .await
            .unwrap();
        assert_eq!(next.url, "https://cdn.example/fresh");
    }

    #[tokio::test]
    async fn invalidating_one_source_does_not_discard_an_independent_source_cache() {
        let cache = PreviewAudioCache::default();
        let audio = preview_audio_from_metadata(
            &Candidate {
                provider: "youtube".into(),
                ..Candidate::default()
            },
            &serde_json::json!({
                "url":"https://rr1.googlevideo.com/videoplayback", "acodec":"aac",
                "vcodec":"none", "protocol":"https", "ext":"m4a"
            }),
        )
        .unwrap();
        cache
            .get_or_resolve("independent-source".into(), || async {
                cache.invalidate("failed-source");
                Ok(audio)
            })
            .await
            .unwrap();
        cache
            .get_or_resolve("independent-source".into(), || async {
                panic!("source invalidation must not poison an independent caller/cache entry")
            })
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provider_reset_fences_inflight_metadata_and_prepared_audio_before_access() {
        let output = serde_json::json!({
            "id":"sessiontest", "title":"Artist - Recording", "duration":180,
            "url":"https://rr1.googlevideo.com/videoplayback?token=private-primary",
            "acodec":"aac", "vcodec":"none", "protocol":"https", "ext":"m4a",
            "formats":[{"url":"https://rr1.googlevideo.com/videoplayback?token=private-alternate",
                "acodec":"opus", "vcodec":"none", "protocol":"https", "ext":"webm"}]
        });
        let (_tools, settings) = fake_tool(&format!("printf '%s\n' '{}'", output));
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("provider-reset.sqlite")).unwrap());
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let manager = AcquisitionManager::new_for_playback_test(store, connections);
        let candidate = Candidate {
            id: "youtube:sessiontest".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=sessiontest".into(),
            ..Candidate::default()
        };
        let reset_manager = manager.clone();
        let reset_trace = crate::playback_sources::SourceTrace::new(None, None, None)
            .with_test_sink(Arc::new(move |event| {
                if event["phase"] == "extraction_end" {
                    reset_manager.invalidate_playback_provider_session("youtube");
                }
            }));
        let (_tx, cancel) = idle_cancel();
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(10));
        let reset_during_extraction = with_playback_test_public_addresses(
            manager.extract_playback_audio_traced_preparing_alternate(
                &candidate,
                &settings,
                cancel.clone(),
                reset_trace,
                deadline,
                None,
            ),
        )
        .await;
        assert!(reset_during_extraction
            .unwrap_err()
            .contains("session changed"));

        let prepared = Arc::new(Mutex::new(None));
        let trace = crate::playback_sources::SourceTrace::new(None, None, None);
        let audio = with_playback_test_public_addresses(
            manager.extract_playback_audio_traced_preparing_alternate(
                &candidate,
                &settings,
                cancel.clone(),
                trace.clone(),
                deadline,
                Some(prepared.clone()),
            ),
        )
        .await
        .unwrap();
        let alternate = prepared.lock().unwrap().take().unwrap();
        assert!(audio.provider_session_current() && alternate.provider_session_current());
        let profile = preview_audio_cache_key(&candidate, &settings, false);
        manager
            .inner
            .playback_metadata_cache
            .remember_profile_failure(&profile, "Audio source format is unavailable");
        assert!(manager
            .inner
            .playback_metadata_cache
            .profile_cooldown(&profile)
            .is_some());
        manager.invalidate_playback_provider_session("youtube");
        assert!(manager
            .inner
            .playback_metadata_cache
            .profile_cooldown(&profile)
            .is_none());
        assert!(!audio.provider_session_current() && !alternate.provider_session_current());
        let phases = Arc::new(Mutex::new(Vec::new()));
        let observed = phases.clone();
        let trace = trace.with_test_sink(Arc::new(move |event| {
            observed
                .lock()
                .unwrap()
                .push(event["phase"].as_str().unwrap().to_owned());
        }));
        assert!(manager
            .validate_playback_audio_traced(
                &candidate,
                audio,
                cancel.clone(),
                trace.clone(),
                deadline
            )
            .await
            .unwrap_err()
            .contains("session changed"));
        assert!(manager
            .extract_playback_audio_fresh_or_prepared_traced(
                &candidate,
                &settings,
                cancel,
                true,
                trace,
                deadline,
                Some(alternate)
            )
            .await
            .unwrap_err()
            .contains("session changed"));
        assert!(
            phases.lock().unwrap().is_empty(),
            "a reset must fence before any CDN DNS/probe"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn denied_signed_url_does_not_block_fresh_extraction_of_the_same_format() {
        let metadata = serde_json::json!({
            "id":"refreshsame", "title":"Artist - Recording", "duration":180,
            "url":"https://rr1.googlevideo.com/videoplayback?token=fresh",
            "acodec":"aac", "vcodec":"none", "protocol":"https", "ext":"m4a"
        });
        let (_tools, settings) = fake_tool(&format!("printf '%s\n' '{}'", metadata));
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("fresh-url.sqlite")).unwrap());
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let manager = AcquisitionManager::new_for_playback_test(store, connections);
        let candidate = Candidate {
            id: "youtube:refreshsame".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=refreshsame".into(),
            ..Candidate::default()
        };
        let mut expired_metadata = metadata.clone();
        expired_metadata["url"] = "https://rr1.googlevideo.com/videoplayback?token=expired".into();
        let expired_audio = preview_audio_from_metadata(&candidate, &expired_metadata).unwrap();
        let expired_key = playback_access_cache_key(&candidate, &expired_audio);
        let denied = "Audio source access was denied (HTTP 403)";
        manager
            .inner
            .playback_access_cache
            .get_or_resolve(expired_key.clone(), || async { Err(denied.into()) })
            .await
            .unwrap_err();
        assert!(manager
            .inner
            .playback_access_cache
            .profile_cooldown(&expired_key)
            .is_some());
        let profile = preview_audio_cache_key(&candidate, &settings, false);
        manager
            .inner
            .playback_metadata_cache
            .remember_profile_failure(&profile, denied);
        assert!(
            manager
                .inner
                .playback_metadata_cache
                .profile_cooldown(&profile)
                .is_none(),
            "an extractor denial does not prove a client/format failure"
        );

        let (_tx, cancel) = idle_cancel();
        let trace = crate::playback_sources::SourceTrace::new(None, None, None);
        let deadline = crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(10));
        let fresh = with_playback_test_public_addresses(
            manager.extract_playback_audio_fresh_or_prepared_traced(
                &candidate, &settings, cancel, false, trace, deadline, None,
            ),
        )
        .await
        .unwrap();
        assert_eq!(fresh.url, metadata["url"].as_str().unwrap());
        let fresh_key = playback_access_cache_key(&candidate, &fresh);
        assert_ne!(expired_key, fresh_key);
        assert!(manager
            .inner
            .playback_access_cache
            .profile_cooldown(&fresh_key)
            .is_none());
        // Exercise the access-cache boundary with a successful fresh probe result;
        // strict HTTP/range/real-byte probing is covered by the probe tests below.
        let accepted = manager
            .inner
            .playback_access_cache
            .get_or_resolve(fresh_key, || async { Ok(fresh) })
            .await
            .unwrap();
        assert_eq!(accepted.url, metadata["url"].as_str().unwrap());
    }

    #[tokio::test]
    async fn failed_playback_profile_cooldown_is_bounded_and_leaves_alternates_eligible() {
        let cache = PreviewAudioCache::default();
        let denied = "Audio source format is unavailable; refresh or choose another source";
        assert!(cache
            .get_or_resolve("candidate:default".into(), || async { Err(denied.into()) })
            .await
            .is_err());
        let repeated = cache
            .get_or_resolve("candidate:default".into(), || async {
                panic!("a rejected client/format profile must cool down")
            })
            .await
            .unwrap_err();
        assert_eq!(
            crate::playback_failure::classify(&repeated).category,
            crate::playback_failure::FailureCategory::UnsupportedAudio
        );
        let alternate = cache
            .get_or_resolve("candidate:alternate".into(), || async {
                Err("alternate profile was attempted".into())
            })
            .await
            .unwrap_err();
        assert_eq!(alternate, "alternate profile was attempted");
        for index in 0..(PLAYBACK_PROFILE_COOLDOWN_MAX + 10) {
            cache.remember_profile_failure(&format!("profile:{index}"), denied);
        }
        assert_eq!(
            cache.cooldowns.lock().unwrap().len(),
            PLAYBACK_PROFILE_COOLDOWN_MAX
        );
        cache.cooldowns.lock().unwrap().insert(
            "expired".into(),
            (Instant::now() - PLAYBACK_PROFILE_COOLDOWN, denied.into()),
        );
        assert!(cache.profile_cooldown("expired").is_none());
    }

    #[tokio::test]
    async fn transient_network_and_cancellation_never_cool_down_a_playback_profile() {
        let cache = PreviewAudioCache::default();
        for message in [
            "Cancelled",
            "Audio source network connection failed",
            "Audio source extraction timed out",
            "Audio source access was denied (HTTP 403)",
            "Audio source access was denied; refresh or choose another source",
            "The resolved audio source identity does not match this recording",
        ] {
            let first = cache
                .get_or_resolve("candidate".into(), || async { Err(message.into()) })
                .await;
            assert_eq!(first.unwrap_err(), message);
            assert!(cache.profile_cooldown("candidate").is_none());
        }
    }

    #[test]
    fn preview_audio_cache_respects_signed_url_expiry() {
        let expiry = Utc::now().timestamp() + 90;
        let url = format!("https://cdn.example/audio?expire={expiry}");
        let ttl = preview_audio_cache_ttl(&url);
        assert!(ttl <= Duration::from_secs(90));
        assert!(ttl >= Duration::from_secs(58));
        assert_eq!(
            preview_audio_cache_ttl("https://cdn.example/audio"),
            PREVIEW_AUDIO_CACHE_TTL
        );
    }

    #[test]
    fn relative_signed_url_expiry_is_anchored_to_signing_time() {
        let now = chrono::NaiveDateTime::parse_from_str("20261015T120000Z", "%Y%m%dT%H%M%SZ")
            .unwrap()
            .and_utc()
            .timestamp();
        for prefix in ["X-Amz", "X-Goog"] {
            assert_eq!(
                preview_audio_cache_ttl_at(
                    &format!(
                        "https://cdn.example/a?{prefix}-Date=20261015T115900Z&{prefix}-Expires=120"
                    ),
                    now
                ),
                Duration::from_secs(30)
            );
            assert_eq!(
                preview_audio_cache_ttl_at(
                    &format!(
                        "https://cdn.example/a?{prefix}-Date=20261015T115000Z&{prefix}-Expires=120"
                    ),
                    now
                ),
                Duration::ZERO
            );
            assert_eq!(
                preview_audio_cache_ttl_at(
                    &format!("https://cdn.example/a?{prefix}-Expires=120"),
                    now
                ),
                Duration::ZERO
            );
        }
        assert_eq!(
            preview_audio_cache_ttl_at(
                &format!(
                    "https://cdn.example/a?Expires={}&expire={}",
                    now + 90,
                    now + 45
                ),
                now
            ),
            Duration::from_secs(15)
        );
        assert_eq!(
            preview_audio_cache_ttl_at("https://cdn.example/a?expire=1&expire=4102444800", now),
            Duration::ZERO
        );
        assert_eq!(
            preview_audio_cache_ttl_at("https://cdn.example/a?Expires=invalid", now),
            Duration::ZERO
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_search_result_is_cached_to_avoid_repeating_failed_lookups() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (_directory, settings) = fake_tool(r#"echo '{"entries":[]}'"#);
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let providers = vec!["youtube".to_string()];
        let calls = AtomicUsize::new(0);
        let cache = CandidateSearchCache::default();
        for _ in 0..2 {
            let found = cache
                .get_or_search("empty-search".into(), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    find_candidates(&track, &providers, &settings)
                })
                .await
                .unwrap();
            assert!(found.is_empty());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn newer_preview_generation_cancels_old_search_and_rejects_late_result() {
        let directory = tempfile::tempdir().unwrap();
        let started = directory.path().join("started");
        let (_tool_directory, settings) =
            fake_tool(&format!("touch '{}'\nsleep 60", started.display()));
        let state = Arc::new(PreviewSearchState::default());
        let old_cancel = state.begin(1_000).unwrap();
        let old_state = state.clone();
        let old_search = tokio::spawn(async move {
            run_tool(
                &settings,
                vec![],
                old_cancel,
                Duration::from_secs(30),
                |_| {},
            )
            .await
            .map(|_| ())
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake yt-dlp process did not start");
        let _new_cancel = state.begin(2_000).unwrap();
        let error = match old_search.await.unwrap() {
            Err(error) => error,
            Ok(output) => panic!("expected old preview to be cancelled, got {output:?}"),
        };
        assert!(error.contains("Cancelled"), "unexpected error: {error}");
        assert!(!old_state.finish(1_000).unwrap());
        assert!(state.finish(2_000).unwrap());
        assert!(state.begin(1_500).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_tool_cancels_when_watch_sender_closes_with_pending_true() {
        let (_directory, settings) = fake_tool("echo ready\nsleep 60");
        let state = Arc::new(PreviewSearchState::default());
        let old_cancel = state.begin(1_000).unwrap();
        let mut new_cancel = None;
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            run_tool(
                &settings,
                vec![],
                old_cancel,
                Duration::from_secs(1),
                |_| {
                    if new_cancel.is_none() {
                        // Cancel synchronously while run_tool is handling the
                        // line, so send(true) and dropping the old sender both
                        // happen before its next select iteration.
                        new_cancel = Some(state.begin(2_000).unwrap());
                    }
                },
            ),
        )
        .await
        .expect("run_tool did not stop promptly after cancellation");
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert!(new_cancel.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preview_search_honors_provider_timeout() {
        let (_directory, settings) = fake_tool("sleep 60");
        let track = Track {
            artist: "Artist One".into(),
            title: "Night Drive".into(),
            ..Default::default()
        };
        let started = Instant::now();
        let (_tx, cancel) = idle_cancel();
        let result = find_candidates_with_cancel(
            &track,
            &["youtube".into()],
            &settings,
            cancel,
            3,
            Duration::from_millis(50),
        )
        .await;
        let error = result.unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn preview_audio_cancelled_owner_hands_off_to_already_queued_waiter() {
        use tokio::sync::oneshot;

        let cache = Arc::new(PreviewAudioCache::default());
        let (owner_cancel_tx, owner_cancel_rx) = watch::channel(false);
        let (owner_started_tx, owner_started_rx) = oneshot::channel();
        let owner_cache = cache.clone();
        let owner = tokio::spawn(async move {
            owner_cache
                .get_or_resolve_cancelled(
                    "owner-cancel-handoff".into(),
                    owner_cancel_rx,
                    None,
                    move |_| async move {
                        let _ = owner_started_tx.send(());
                        std::future::pending::<Result<PreviewAudio, String>>().await
                    },
                )
                .await
        });
        owner_started_rx.await.unwrap();

        let (waiter_resolver_tx, waiter_resolver_rx) = oneshot::channel();
        let waiter_cache = cache.clone();
        let waiter = tokio::spawn(async move {
            waiter_cache
                .get_or_resolve_cancelled(
                    "owner-cancel-handoff".into(),
                    watch::channel(false).1,
                    None,
                    move |_| async move {
                        let _ = waiter_resolver_tx.send(());
                        Ok(PreviewAudio {
                            provider_session: None,
                            url: "https://cdn.example/takeover-audio".into(),
                            mime_type: "audio/mp4".into(),
                            codec: Some("aac".into()),
                            bitrate_kbps: None,
                            duration_seconds: Some(131.706),
                            selected_format_id: None,
                            selected_format_has_video: false,
                            source_metadata: None,
                            resolved_title: None,
                            resolved_provider_id: None,
                            chapters: None,
                            expected_content_length: None,
                            headers: vec![],
                        })
                    },
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiter_has_joined = cache
                    .in_flight
                    .lock()
                    .unwrap()
                    .get("owner-cancel-handoff")
                    .is_some_and(|flight| Arc::strong_count(flight) >= 3);
                if waiter_has_joined {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("waiter should be queued on the owner's shared flight");

        owner_cancel_tx.send_replace(true);
        assert_eq!(owner.await.unwrap().unwrap_err(), "Cancelled");
        let audio = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("queued waiter should take over after owner cancellation")
            .unwrap()
            .unwrap();
        assert_eq!(audio.url, "https://cdn.example/takeover-audio");
        waiter_resolver_rx
            .await
            .expect("waiter's resolver should run after owner releases the gate");

        let cached = cache
            .get_or_resolve("owner-cancel-handoff".into(), || async {
                panic!("waiter takeover result should be cached")
            })
            .await
            .unwrap();
        assert_eq!(cached.url, audio.url);
    }

    #[tokio::test]
    async fn expired_candidate_search_entry_is_refreshed_and_then_reused() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = CandidateSearchCache::default();
        let key = "expired-candidate-fixture".to_string();
        cache.entries.lock().unwrap().insert(
            key.clone(),
            CachedCandidates {
                cached_at: Instant::now() - SEARCH_CACHE_TTL - Duration::from_secs(1),
                candidates: vec![Candidate {
                    id: "youtube:stale-fixture".into(),
                    ..Candidate::default()
                }],
            },
        );

        let refresh_calls = AtomicUsize::new(0);
        let refreshed = cache
            .get_or_search(key.clone(), || async {
                refresh_calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![Candidate {
                    id: "youtube:refreshed-fixture".into(),
                    ..Candidate::default()
                }])
            })
            .await
            .unwrap();
        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].id, "youtube:refreshed-fixture");
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);

        let cached = cache
            .get_or_search(key, || async {
                panic!("refreshed candidate result should be reused")
            })
            .await
            .unwrap();
        assert_eq!(cached[0].id, "youtube:refreshed-fixture");
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn sequential_provider_fallback_can_reuse_generation() {
        let state = PreviewSearchState::default();
        let _youtube = state.begin(2_000).unwrap();
        assert!(state.begin(2_000).is_err());
        assert!(state.finish(2_000).unwrap());
        let _soundcloud = state.begin(2_000).unwrap();
        assert!(state.finish(2_000).unwrap());
        assert!(state.begin(1_999).is_err());
    }
}
