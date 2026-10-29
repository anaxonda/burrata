//! Resource-scoped observations made by opt-in track detection while a mix,
//! set, or other continuous recording is playing. These are display hints, not
//! recording boundaries or catalog metadata.

use crate::{models::Track, radio::RadioTrackMetadata, storage::Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

const CACHE_KEY: &str = "playingTrackDetectionCache.v1";
const POLICY_KEY: &str = "playingTrackDetectionPolicy.v1";
const MAX_RESOURCES: usize = 48;
// A durable row is capped independently, while library recordings are
// protected from the generic analysis-cache LRU.
const MAX_DETECTIONS_PER_RESOURCE: usize = 256;
const MAX_LEGACY_DETECTIONS_PER_RESOURCE: usize = 1024;
const MAX_STORE_BYTES: usize = 256 * 1024;
const MAX_DURABLE_RECORD_BYTES: usize = 240 * 1024;
const DEDUPE_GAP_SECONDS: f64 = 45.0;
const DURABLE_CACHE_NAMESPACE: &str = "detected-tracks-v1";

static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static LOCAL_HASH_CACHE: OnceLock<Mutex<HashMap<String, CachedLocalHash>>> = OnceLock::new();
static LOCAL_HASH_CACHE_CLOCK: AtomicU64 = AtomicU64::new(0);
const MAX_LOCAL_HASH_CACHE_ENTRIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalFileStamp {
    canonical_path: PathBuf,
    size: u64,
    modified_ns: Option<u128>,
    created_ns: Option<u128>,
    #[cfg(unix)]
    identity_and_change: (u64, u64, i64, i64), // device, inode, ctime seconds/nanoseconds
}

struct CachedLocalHash {
    registered_hash: String,
    stamp: LocalFileStamp,
    last_used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionResourceKey(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    provider: String,
    source_id: String,
    duration_seconds: Option<u32>,
}

impl SourceIdentity {
    pub fn new(provider: &str, source_id: &str) -> Option<Self> {
        Some(Self {
            provider: normalize_provider(provider)?,
            source_id: normalize_component(source_id, 256)?,
            duration_seconds: None,
        })
    }

    pub fn with_duration_seconds(mut self, duration_seconds: Option<f64>) -> Self {
        self.duration_seconds = normalized_duration(duration_seconds);
        self
    }

    pub fn provider_source_id(&self) -> (&str, &str) {
        (&self.provider, &self.source_id)
    }

    /// Hash a server-validated finite preview URI without retaining its query,
    /// credentials, or path in the cache. This fallback is only for preview
    /// playback that has no provider candidate identifier.
    pub fn from_validated_url(
        provider: &str,
        value: &str,
        duration_seconds: Option<f64>,
    ) -> Option<Self> {
        if value.is_empty() || value.len() > 8192 {
            return None;
        }
        let url = reqwest::Url::parse(value).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let digest = Sha256::digest(value.as_bytes());
        Some(
            Self::new(provider, &format!("uri-sha256:{digest:x}"))?
                .with_duration_seconds(duration_seconds),
        )
    }
}

/// Whether the effective playback identity can use a local content hash.
/// An explicit remote candidate takes precedence over any downloaded local
/// asset attached to the catalog track.
pub fn requires_verified_local_hash(
    track: &Track,
    source_identity: Option<&SourceIdentity>,
    active_asset_present: bool,
) -> bool {
    if let Some(source) = source_identity {
        source.provider == "local"
    } else {
        normalize_provider(&track.provider).as_deref() == Some("local")
            || track.asset_id.is_some()
            || active_asset_present
    }
}

pub fn local_asset_id_for_source<'a>(
    track: &'a Track,
    source_identity: Option<&'a SourceIdentity>,
    active_asset_id: Option<&'a str>,
) -> Option<&'a str> {
    active_asset_id
        .or_else(|| {
            source_identity
                .filter(|source| source.provider == "local")
                .map(|source| source.source_id.as_str())
        })
        .or(track.asset_id.as_deref())
}

impl DetectionResourceKey {
    /// Construct only from a stable source identifier or a verified local file
    /// hash. Track/recording IDs alone are intentionally insufficient.
    pub fn for_track(track: &Track, verified_local_hash: Option<&str>) -> Option<Self> {
        Self::for_playback_source(track, None, verified_local_hash)
    }

    pub fn for_playback_source(
        track: &Track,
        source_identity: Option<&SourceIdentity>,
        verified_local_hash: Option<&str>,
    ) -> Option<Self> {
        let provider = normalize_provider(&track.provider).unwrap_or_default();
        let active_provider = source_identity
            .map(|source| source.provider.as_str())
            .unwrap_or(provider.as_str());
        let local_source = active_provider == "local"
            || (source_identity.is_none()
                && (track.asset_id.is_some() || verified_local_hash.is_some()));
        let identity = if local_source {
            let hash = verified_local_hash?;
            if !valid_sha256(hash) {
                return None;
            }
            // Asset IDs can change when backups are merged or files are
            // re-registered. The verified content hash is the resource ID.
            format!("local-content-v1\0{}", hash.to_ascii_lowercase())
        } else {
            let (provider, source_id) = if let Some(source) = source_identity {
                (source.provider.as_str(), source.source_id.as_str())
            } else {
                (provider.as_str(), track.provider_id.as_str())
            };
            let provider = normalize_provider(provider)?;
            let source_id = normalize_component(source_id, 256)?;
            if matches!(provider.as_str(), "unknown" | "catalog" | "musicbrainz") {
                return None;
            }
            format!(
                "remote\0{provider}\0{source_id}\0{}",
                source_identity
                    .and_then(|source| source.duration_seconds)
                    .or_else(|| normalized_duration(track.duration_seconds))
                    .map_or_else(|| "unknown".to_owned(), |duration| duration.to_string())
            )
        };
        let digest = Sha256::digest(identity.as_bytes());
        Some(Self(format!("{:x}", digest)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Return a local content identity only when the registered asset still
/// matches the bytes currently present at its path. The database hash alone
/// can be stale after an external file change.
pub fn verified_local_asset_hash(store: &Store, asset_id: &str) -> Option<String> {
    let asset = store.get_asset_by_id(asset_id).ok().flatten()?;
    if !valid_sha256(&asset.hash) {
        return None;
    }
    let path = Path::new(&asset.path);
    let stamp = local_file_stamp(path)?;
    let key = asset.id.clone();
    let cache = LOCAL_HASH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let use_tick = LOCAL_HASH_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed);
    {
        let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = cache.get_mut(&key) {
            if entry.registered_hash.eq_ignore_ascii_case(&asset.hash) && entry.stamp == stamp {
                entry.last_used = use_tick;
                return Some(entry.registered_hash.clone());
            }
        }
    }

    // Cold or invalidated entries are cryptographically verified once. Check
    // metadata on both sides of the read so a concurrent replacement cannot
    // publish a hash for a different file version.
    let actual_hash = crate::media::hash_file(path).ok()?;
    let after = local_file_stamp(path)?;
    if stamp != after || !actual_hash.eq_ignore_ascii_case(&asset.hash) {
        return None;
    }
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    if cache.len() >= MAX_LOCAL_HASH_CACHE_ENTRIES && !cache.contains_key(&key) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(
        key,
        CachedLocalHash {
            registered_hash: asset.hash.to_ascii_lowercase(),
            stamp: after,
            last_used: use_tick,
        },
    );
    Some(asset.hash.to_ascii_lowercase())
}

fn local_file_stamp(path: &Path) -> Option<LocalFileStamp> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos());
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    #[cfg(unix)]
    let identity_and_change = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    Some(LocalFileStamp {
        canonical_path: std::fs::canonicalize(path).ok()?,
        size: metadata.len(),
        modified_ns,
        created_ns,
        #[cfg(unix)]
        identity_and_change,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DetectedTrackObservation {
    pub title: String,
    pub artist: String,
    /// Approximate midpoint of the captured PCM in the current source clock.
    pub detected_at_seconds: f64,
    /// Latest nearby confirmation, used to suppress periodic duplicate rows.
    /// The displayed detection point remains the first observation.
    pub last_confirmed_at_seconds: f64,
    /// Captured interval retained to communicate the uncertainty of the point.
    pub captured_start_seconds: f64,
    pub captured_end_seconds: f64,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifiers: Option<crate::recording_identity::RecognitionIdentifiers>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DetectionResourceView {
    /// Opaque stable SHA-256 identifier of the exact active resource.
    pub resource_key: String,
    pub enabled: bool,
    pub observations: Vec<DetectedTrackObservation>,
    /// True when a saved library recording reached the per-resource history
    /// limit. Further detections are withheld to preserve its existing times.
    pub retention_limit_reached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DetectionCache {
    version: u8,
    resources: Vec<ResourceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceRecord {
    key: String,
    last_used_at: String,
    observations: Vec<DetectedTrackObservation>,
    #[serde(default)]
    retention_limit_reached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DetectionPolicy {
    version: u8,
    resources: Vec<PolicyRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyRecord {
    key: String,
    enabled: bool,
    last_used_at: String,
}

pub fn read(store: &Store, key: &DetectionResourceKey) -> Result<DetectionResourceView, String> {
    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    read_unlocked(store, key)
}

pub fn set_enabled(
    store: &Store,
    key: &DetectionResourceKey,
    enabled: bool,
    library_recording_id: Option<&str>,
) -> Result<DetectionResourceView, String> {
    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut policy = load_policy(store)?;
    let now = chrono::Utc::now().to_rfc3339();
    let record = find_or_insert_policy(&mut policy, key, &now);
    record.enabled = enabled;
    record.last_used_at = now.clone();
    trim_policy(&mut policy);
    save_policy(store, policy)?;
    if let Some(mut record) = load_cache_record(store, key)? {
        record.last_used_at = now;
        save_cache_record(store, key, &record, library_recording_id)?;
    }
    read_unlocked(store, key)
}

pub fn record_if_enabled(
    store: &Store,
    key: &DetectionResourceKey,
    metadata: &RadioTrackMetadata,
    library_recording_id: Option<&str>,
    captured_start_seconds: f64,
    captured_duration_seconds: f64,
) -> Result<bool, String> {
    if !captured_start_seconds.is_finite()
        || captured_start_seconds < 0.0
        || !captured_duration_seconds.is_finite()
        || !(1.0..=12.0).contains(&captured_duration_seconds)
        || !safe_text(&metadata.title, 240)
        || !safe_text(&metadata.artist, 240)
    {
        return Ok(false);
    }
    let end = captured_start_seconds + captured_duration_seconds;
    if !end.is_finite() || end > 86_400.0 {
        return Ok(false);
    }
    let title = metadata.title.trim().to_owned();
    let artist = metadata.artist.trim().to_owned();
    let identifiers = metadata.identifiers.as_ref().and_then(safe_identifiers);
    let artwork_url = crate::radio::safe_recognition_artwork_url(metadata.artwork_url.as_deref());

    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    let policy = load_policy(store)?;
    if !policy
        .resources
        .iter()
        .any(|record| record.key == key.0 && record.enabled)
    {
        return Ok(false);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut record = load_cache_record(store, key)?.unwrap_or_else(|| ResourceRecord {
        key: key.0.clone(),
        last_used_at: now.clone(),
        observations: Vec::new(),
        retention_limit_reached: false,
    });
    let library_owned = library_recording_id
        .map(|recording_id| store.recording_is_in_library(recording_id))
        .transpose()?
        .unwrap_or(false);
    if record.observations.len() >= MAX_DETECTIONS_PER_RESOURCE && library_owned {
        record.retention_limit_reached = true;
        record.last_used_at = now;
        save_cache_record(store, key, &record, library_recording_id)?;
        return Ok(false);
    }
    let midpoint = captured_start_seconds + captured_duration_seconds / 2.0;
    let same_index = record.observations.iter().position(|last| {
        let identity_conflicts = last
            .identifiers
            .as_ref()
            .zip(identifiers.as_ref())
            .is_some_and(|(a, b)| {
                a.isrc
                    .as_ref()
                    .zip(b.isrc.as_ref())
                    .is_some_and(|(a, b)| a != b)
                    || a.shazam_id
                        .as_ref()
                        .zip(b.shazam_id.as_ref())
                        .is_some_and(|(a, b)| a != b)
            });
        let identity_match = last
            .identifiers
            .as_ref()
            .zip(identifiers.as_ref())
            .is_some_and(|(a, b)| {
                (a.shazam_id.is_some() && a.shazam_id == b.shazam_id)
                    || (a.isrc.is_some() && a.isrc == b.isrc)
            });
        let text_match = !identity_conflicts
            && normalize_text(&last.artist) == normalize_text(&artist)
            && normalize_text(&last.title) == normalize_text(&title);
        !identity_conflicts && (identity_match || text_match)
            // Grow a continuous run of confirmations, preserving its first
            // detected point. A fixed radius around that first point would
            // create a duplicate every other 35-second detection interval.
            // Search the whole observed interval on a backward seek as well.
            && midpoint >= last.detected_at_seconds - DEDUPE_GAP_SECONDS
            && midpoint <= last.last_confirmed_at_seconds + DEDUPE_GAP_SECONDS
    });
    if let Some(index) = same_index {
        let last = &mut record.observations[index];
        last.last_confirmed_at_seconds = last.last_confirmed_at_seconds.max(midpoint);
        if last.identifiers.is_none() {
            last.identifiers = identifiers;
        }
        if last.artwork_url.is_none() {
            last.artwork_url = artwork_url;
        }
    } else {
        record.observations.push(DetectedTrackObservation {
            title,
            artist,
            detected_at_seconds: midpoint,
            last_confirmed_at_seconds: midpoint,
            captured_start_seconds,
            captured_end_seconds: end,
            observed_at: now.clone(),
            identifiers,
            artwork_url,
        });
    }
    record.last_used_at = now;
    record.retention_limit_reached = false;
    trim_resource_record(&mut record);
    save_cache_record(store, key, &record, library_recording_id)?;
    Ok(true)
}

/// Typed, privacy-filtered projection for the derived observations cache.
pub fn sanitize_backup_value(value: serde_json::Value) -> Option<serde_json::Value> {
    if serde_json::to_vec(&value).ok()?.len() > MAX_STORE_BYTES {
        return None;
    }
    let mut cache: DetectionCache = serde_json::from_value(value).ok()?;
    if cache.version != 1 || cache.resources.len() > MAX_RESOURCES {
        return None;
    }
    // Strip unsafe optional identifiers independently so a valid observation
    // survives legacy backup payloads that included URL-bearing ID fields.
    for record in &mut cache.resources {
        for observation in &mut record.observations {
            observation.identifiers = observation
                .identifiers
                .take()
                .and_then(|ids| safe_identifiers(&ids));
            observation.artwork_url = observation
                .artwork_url
                .take()
                .and_then(|url| crate::radio::safe_recognition_artwork_url(Some(&url)));
        }
    }
    cache.resources.retain(valid_resource_record);
    trim_cache(&mut cache);
    serde_json::to_value(cache).ok()
}

/// Per-resource consent survives full-profile backup as a record, but all
/// restored policies are disabled until the user opts in again.
pub fn sanitize_policy_backup_value(value: serde_json::Value) -> Option<serde_json::Value> {
    if serde_json::to_vec(&value).ok()?.len() > 32 * 1024 {
        return None;
    }
    let mut policy: DetectionPolicy = serde_json::from_value(value).ok()?;
    if policy.version != 1 || policy.resources.len() > MAX_RESOURCES {
        return None;
    }
    policy
        .resources
        .retain(|record| valid_resource_key(&record.key) && valid_timestamp(&record.last_used_at));
    for record in &mut policy.resources {
        record.enabled = false;
    }
    trim_policy(&mut policy);
    serde_json::to_value(policy).ok()
}

fn read_unlocked(
    store: &Store,
    key: &DetectionResourceKey,
) -> Result<DetectionResourceView, String> {
    let cached_record = load_cache_record(store, key)?;
    let enabled = load_policy(store)?
        .resources
        .iter()
        .find(|record| record.key == key.0)
        .is_some_and(|record| record.enabled);
    Ok(cached_record
        .map(|record| DetectionResourceView {
            resource_key: key.0.clone(),
            enabled,
            observations: chronological(record.observations),
            retention_limit_reached: record.retention_limit_reached,
        })
        .unwrap_or(DetectionResourceView {
            resource_key: key.0.clone(),
            enabled,
            observations: Vec::new(),
            retention_limit_reached: false,
        }))
}

fn chronological(mut observations: Vec<DetectedTrackObservation>) -> Vec<DetectedTrackObservation> {
    observations.sort_by(|a, b| a.detected_at_seconds.total_cmp(&b.detected_at_seconds));
    observations
}

fn durable_cache_key(key: &DetectionResourceKey) -> String {
    format!("{:x}", Sha256::digest(key.0.as_bytes()))
}

/// Migrate the old aggregate preference once. Old records have no owning
/// recording reference, so they remain ordinary LRU cache entries until a
/// live library playback writes them with a verified recording ID.
fn migrate_legacy_cache(store: &Store) -> Result<(), String> {
    let Some(value) = store
        .get_json_preference(CACHE_KEY)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    if serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > MAX_STORE_BYTES) {
        return Ok(());
    }
    let mut cache: DetectionCache = match serde_json::from_value(value) {
        Ok(cache) => cache,
        Err(_) => return Ok(()),
    };
    if cache.version != 1 {
        return Ok(());
    }
    if cache.resources.is_empty() {
        return Ok(());
    }
    if cache.resources.len() > MAX_RESOURCES {
        return Ok(());
    }
    for record in cache.resources {
        let key = DetectionResourceKey(record.key.clone());
        let mut record = record;
        if record.observations.len() > MAX_LEGACY_DETECTIONS_PER_RESOURCE {
            continue;
        }
        trim_resource_record(&mut record);
        if !record.observations.is_empty() && valid_resource_record(&record) {
            save_cache_record(store, &key, &record, None)?;
        }
    }
    store
        .set_json_preference(CACHE_KEY, &serde_json::json!({"version":1,"resources":[]}))
        .map_err(|error| error.to_string())
}

fn load_cache_record(
    store: &Store,
    key: &DetectionResourceKey,
) -> Result<Option<ResourceRecord>, String> {
    migrate_legacy_cache(store)?;
    let Some(value) =
        store.read_analysis_cache_record(DURABLE_CACHE_NAMESPACE, &durable_cache_key(key))?
    else {
        return Ok(None);
    };
    let record: ResourceRecord = serde_json::from_value(value)
        .map_err(|_| "Detected track cache record is invalid".to_owned())?;
    if record.key != key.0 || !valid_resource_record(&record) {
        return Ok(None);
    }
    Ok(Some(record))
}

fn save_cache_record(
    store: &Store,
    key: &DetectionResourceKey,
    record: &ResourceRecord,
    library_recording_id: Option<&str>,
) -> Result<(), String> {
    if record.key != key.0 || !valid_resource_record(record) {
        return Err("Detected track cache record is invalid".into());
    }
    let value = serde_json::to_value(record).map_err(|error| error.to_string())?;
    if serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > MAX_DURABLE_RECORD_BYTES) {
        return Err("Detected track cache record reached its size limit".into());
    }
    store.write_analysis_cache_record(
        DURABLE_CACHE_NAMESPACE,
        &durable_cache_key(key),
        &value,
        library_recording_id,
    )
}

fn trim_resource_record(record: &mut ResourceRecord) {
    if record.observations.len() > MAX_DETECTIONS_PER_RESOURCE {
        let excess = record.observations.len() - MAX_DETECTIONS_PER_RESOURCE;
        record.observations.drain(..excess);
    }
    while serde_json::to_vec(record).map_or(true, |bytes| bytes.len() > MAX_DURABLE_RECORD_BYTES)
        && record.observations.len() > 1
    {
        record.observations.remove(0);
    }
}

fn load_policy(store: &Store) -> Result<DetectionPolicy, String> {
    let value = store
        .get_json_preference(POLICY_KEY)
        .map_err(|e| e.to_string())?;
    let Some(value) = value else {
        return Ok(DetectionPolicy {
            version: 1,
            resources: Vec::new(),
        });
    };
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 32 * 1024 {
        return Ok(DetectionPolicy {
            version: 1,
            resources: Vec::new(),
        });
    }
    let mut policy: DetectionPolicy = serde_json::from_value(value).unwrap_or(DetectionPolicy {
        version: 1,
        resources: Vec::new(),
    });
    if policy.version != 1 {
        policy = DetectionPolicy {
            version: 1,
            resources: Vec::new(),
        };
    }
    policy
        .resources
        .retain(|record| valid_resource_key(&record.key) && valid_timestamp(&record.last_used_at));
    trim_policy(&mut policy);
    Ok(policy)
}

fn save_policy(store: &Store, policy: DetectionPolicy) -> Result<(), String> {
    let value = serde_json::to_value(policy).map_err(|e| e.to_string())?;
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 32 * 1024 {
        return Err("Track detection preferences reached their size limit".into());
    }
    store
        .set_json_preference(POLICY_KEY, &value)
        .map_err(|e| e.to_string())
}

fn find_or_insert_policy<'a>(
    policy: &'a mut DetectionPolicy,
    key: &DetectionResourceKey,
    now: &str,
) -> &'a mut PolicyRecord {
    if let Some(index) = policy
        .resources
        .iter()
        .position(|record| record.key == key.0)
    {
        return &mut policy.resources[index];
    }
    if policy.resources.len() >= MAX_RESOURCES {
        let oldest = policy
            .resources
            .iter()
            .enumerate()
            .min_by_key(|(_, record)| record.last_used_at.as_str())
            .map(|(index, _)| index)
            .unwrap_or(0);
        policy.resources.remove(oldest);
    }
    policy.resources.push(PolicyRecord {
        key: key.0.clone(),
        enabled: false,
        last_used_at: now.to_owned(),
    });
    policy.resources.last_mut().expect("policy was inserted")
}

fn trim_cache(cache: &mut DetectionCache) {
    cache.resources.retain(valid_resource_record);
    cache
        .resources
        .sort_by(|a, b| a.last_used_at.cmp(&b.last_used_at));
    if cache.resources.len() > MAX_RESOURCES {
        cache
            .resources
            .drain(..cache.resources.len() - MAX_RESOURCES);
    }
    for record in &mut cache.resources {
        if record.observations.len() > MAX_DETECTIONS_PER_RESOURCE {
            let excess = record.observations.len() - MAX_DETECTIONS_PER_RESOURCE;
            record.observations.drain(..excess);
        }
    }
    while serde_json::to_vec(cache).map_or(true, |bytes| bytes.len() > MAX_STORE_BYTES) {
        let Some((resource_index, observation_index)) = oldest_observation(cache) else {
            break;
        };
        cache.resources[resource_index]
            .observations
            .remove(observation_index);
    }
    cache
        .resources
        .retain(|record| !record.observations.is_empty());
}

fn oldest_observation(cache: &DetectionCache) -> Option<(usize, usize)> {
    cache
        .resources
        .iter()
        .enumerate()
        .flat_map(|(resource_index, record)| {
            record
                .observations
                .iter()
                .enumerate()
                .map(move |(observation_index, observation)| {
                    (
                        resource_index,
                        observation_index,
                        observation.observed_at.as_str(),
                    )
                })
        })
        .min_by_key(|(_, _, observed_at)| *observed_at)
        .map(|(resource_index, observation_index, _)| (resource_index, observation_index))
}

fn valid_resource_record(record: &ResourceRecord) -> bool {
    valid_resource_key(&record.key)
        && valid_timestamp(&record.last_used_at)
        && record.observations.len() <= MAX_DETECTIONS_PER_RESOURCE
        && record.observations.iter().all(|observation| {
            safe_text(&observation.title, 240)
                && safe_text(&observation.artist, 240)
                && observation.detected_at_seconds.is_finite()
                && observation.last_confirmed_at_seconds.is_finite()
                && observation.captured_start_seconds.is_finite()
                && observation.captured_end_seconds.is_finite()
                && observation.captured_start_seconds >= 0.0
                && observation.captured_end_seconds > observation.captured_start_seconds
                && observation.detected_at_seconds >= observation.captured_start_seconds
                && observation.detected_at_seconds <= observation.captured_end_seconds
                && observation.last_confirmed_at_seconds >= observation.detected_at_seconds
                && observation.last_confirmed_at_seconds <= 86_400.0
                && observation.captured_end_seconds <= 86_400.0
                && valid_timestamp(&observation.observed_at)
                && observation.artwork_url.as_deref().is_none_or(|url| {
                    crate::radio::safe_recognition_artwork_url(Some(url)).is_some()
                })
                && observation
                    .identifiers
                    .as_ref()
                    .is_none_or(valid_identifiers)
        })
}

fn valid_resource_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn trim_policy(policy: &mut DetectionPolicy) {
    policy
        .resources
        .retain(|record| valid_resource_key(&record.key) && valid_timestamp(&record.last_used_at));
    policy
        .resources
        .sort_by(|a, b| a.last_used_at.cmp(&b.last_used_at));
    if policy.resources.len() > MAX_RESOURCES {
        policy
            .resources
            .drain(..policy.resources.len() - MAX_RESOURCES);
    }
}

fn safe_identifiers(
    identifiers: &crate::recording_identity::RecognitionIdentifiers,
) -> Option<crate::recording_identity::RecognitionIdentifiers> {
    let isrc = identifiers
        .isrc
        .as_deref()
        .filter(|value| {
            value.len() == 12
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map(str::to_ascii_uppercase);
    let shazam_id = identifiers
        .shazam_id
        .as_deref()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 32
                && value.bytes().all(|byte| byte.is_ascii_digit())
        })
        .map(str::to_owned);
    (isrc.is_some() || shazam_id.is_some()).then_some(
        crate::recording_identity::RecognitionIdentifiers {
            isrc,
            shazam_id,
            apple_music_url: None,
        },
    )
}

fn valid_identifiers(value: &crate::recording_identity::RecognitionIdentifiers) -> bool {
    safe_identifiers(value).as_ref() == Some(value)
}

fn normalize_component(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)))
    .then(|| value.to_owned())
}

fn normalize_provider(value: &str) -> Option<String> {
    normalize_component(value, 48).map(|provider| provider.to_ascii_lowercase())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normalized_duration(value: Option<f64>) -> Option<u32> {
    value
        .filter(|duration| duration.is_finite() && (1.0..=86_400.0).contains(duration))
        .map(|duration| duration.round() as u32)
}

fn safe_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
        && value.trim() == value
        && !value.contains("://")
        && !value.starts_with('/')
        && !value.starts_with('\\')
}

fn valid_timestamp(value: &str) -> bool {
    value.len() <= 40 && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn normalize_text(value: &str) -> String {
    value
        .trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn lock() -> &'static Mutex<()> {
    WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{LocalAsset, Track},
        storage::Store,
    };
    use tempfile::tempdir;

    fn store() -> (tempfile::TempDir, Store) {
        let root = tempdir().unwrap();
        let path = root.path().join("detections.sqlite");
        let store = Store::open(&path).unwrap();
        (root, store)
    }

    #[test]
    fn durable_rows_protect_only_verified_library_recordings() {
        let (root, store) = store();
        let owner = Track {
            id: "youtube:set-in-library".into(),
            provider: "youtube".into(),
            provider_id: "set-in-library".into(),
            ..Track::default()
        };
        store
            .save_library_tracks(std::slice::from_ref(&owner))
            .unwrap();
        let protected_key = DetectionResourceKey::for_track(&owner, None).unwrap();
        set_enabled(&store, &protected_key, true, Some(&owner.id)).unwrap();
        assert!(record_if_enabled(
            &store,
            &protected_key,
            &metadata("Track A", "Artist"),
            Some(&owner.id),
            1.0,
            12.0,
        )
        .unwrap());
        for index in 1..MAX_DETECTIONS_PER_RESOURCE {
            assert!(record_if_enabled(
                &store,
                &protected_key,
                &metadata(&format!("Track {index}"), "Artist"),
                Some(&owner.id),
                index as f64 * 15.0,
                12.0,
            )
            .unwrap());
        }
        assert!(!record_if_enabled(
            &store,
            &protected_key,
            &metadata("Track newest", "Artist"),
            Some(&owner.id),
            MAX_DETECTIONS_PER_RESOURCE as f64 * 15.0,
            12.0,
        )
        .unwrap());
        let protected_view = read(&store, &protected_key).unwrap();
        assert!(protected_view.retention_limit_reached);
        assert_eq!(
            protected_view.observations.len(),
            MAX_DETECTIONS_PER_RESOURCE
        );
        assert_eq!(
            protected_view
                .observations
                .first()
                .unwrap()
                .detected_at_seconds,
            7.0
        );
        assert_eq!(
            protected_view
                .observations
                .last()
                .unwrap()
                .detected_at_seconds,
            3_831.0
        );

        let unowned = track("youtube", "not-in-library");
        let unowned_key = DetectionResourceKey::for_track(&unowned, None).unwrap();
        set_enabled(&store, &unowned_key, true, Some(&unowned.id)).unwrap();
        record_if_enabled(
            &store,
            &unowned_key,
            &metadata("Track B", "Artist"),
            Some(&unowned.id),
            1.0,
            12.0,
        )
        .unwrap();

        let connection = rusqlite::Connection::open(root.path().join("detections.sqlite")).unwrap();
        let protected: i64 = connection
            .query_row(
                "SELECT protected FROM analysis_cache_records WHERE namespace=?1 AND cache_key=?2",
                rusqlite::params![DURABLE_CACHE_NAMESPACE, durable_cache_key(&protected_key)],
                |row| row.get(0),
            )
            .unwrap();
        let unprotected: i64 = connection
            .query_row(
                "SELECT protected FROM analysis_cache_records WHERE namespace=?1 AND cache_key=?2",
                rusqlite::params![DURABLE_CACHE_NAMESPACE, durable_cache_key(&unowned_key)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(protected, 1);
        assert_eq!(unprotected, 0);
        assert!(store.get_json_preference(CACHE_KEY).unwrap().is_none());
    }

    #[test]
    fn legacy_preference_migrates_into_durable_detection_rows() {
        let (_root, store) = store();
        let key = DetectionResourceKey::for_track(&track("youtube", "legacy-set"), None).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let record = ResourceRecord {
            key: key.0.clone(),
            last_used_at: now.clone(),
            observations: vec![DetectedTrackObservation {
                title: "Legacy Song".into(),
                artist: "Legacy Artist".into(),
                detected_at_seconds: 10.0,
                last_confirmed_at_seconds: 10.0,
                captured_start_seconds: 4.0,
                captured_end_seconds: 16.0,
                observed_at: now,
                identifiers: None,
                artwork_url: None,
            }],
            retention_limit_reached: false,
        };
        store
            .set_json_preference(
                CACHE_KEY,
                &serde_json::to_value(DetectionCache {
                    version: 1,
                    resources: vec![record],
                })
                .unwrap(),
            )
            .unwrap();

        let view = read(&store, &key).unwrap();
        assert_eq!(view.observations.len(), 1);
        assert_eq!(view.observations[0].detected_at_seconds, 10.0);
        assert_eq!(
            store.get_json_preference(CACHE_KEY).unwrap().unwrap()["resources"],
            serde_json::json!([])
        );
        assert!(store
            .read_analysis_cache_record(DURABLE_CACHE_NAMESPACE, &durable_cache_key(&key))
            .unwrap()
            .is_some());
    }

    fn track(provider: &str, provider_id: &str) -> Track {
        Track {
            id: "recording:shared".into(),
            provider: provider.into(),
            provider_id: provider_id.into(),
            title: "Set recording".into(),
            artist: "DJ Example".into(),
            ..Track::default()
        }
    }

    fn metadata(title: &str, artist: &str) -> RadioTrackMetadata {
        RadioTrackMetadata {
            identifiers: None,
            station_id: "playing-detection".into(),
            artist: artist.into(),
            title: title.into(),
            album: None,
            year: None,
            artwork_url: None,
            source_url: String::new(),
            observed_at: None,
            metadata_confidence: "audio_recognition".into(),
        }
    }

    #[test]
    fn resource_identity_requires_stable_source_and_separates_same_recording_sources() {
        let youtube = track("youtube", "video-A");
        let case_variant = track("youtube", "Video-A");
        let bandcamp = track("bandcamp", "album-track-A");
        assert_ne!(
            DetectionResourceKey::for_track(&youtube, None),
            DetectionResourceKey::for_track(&case_variant, None),
        );
        assert_ne!(
            DetectionResourceKey::for_track(&youtube, None),
            DetectionResourceKey::for_track(&bandcamp, None)
        );
        assert!(
            DetectionResourceKey::for_track(&track("musicbrainz", "recording:shared"), None)
                .is_none()
        );
        let mut local = track("local", "recording:shared");
        local.asset_id = Some("asset-1".into());
        assert!(DetectionResourceKey::for_track(&local, None).is_none());
        assert!(DetectionResourceKey::for_track(&local, Some(&"a".repeat(64))).is_some());
        assert!(DetectionResourceKey::for_track(&local, Some("bad-hash")).is_none());
        let mut remapped_local = local.clone();
        remapped_local.asset_id = Some("asset-remapped-after-restore".into());
        assert_eq!(
            DetectionResourceKey::for_track(&local, Some(&"a".repeat(64))),
            DetectionResourceKey::for_track(&remapped_local, Some(&"a".repeat(64))),
            "identical verified bytes remain the same resource after asset-ID remapping"
        );
        assert_ne!(
            DetectionResourceKey::for_track(&local, Some(&"a".repeat(64))),
            DetectionResourceKey::for_track(&remapped_local, Some(&"b".repeat(64))),
            "changed file content must not inherit observations"
        );
        let local_without_asset_id = track("musicbrainz", "recording:shared");
        let active_local = SourceIdentity::new("local", "active-resource").unwrap();
        assert!(DetectionResourceKey::for_playback_source(
            &local_without_asset_id,
            Some(&active_local),
            Some(&"c".repeat(64)),
        )
        .is_some());
        let actual_remote = SourceIdentity::new("youtube", "video-A").unwrap();
        assert!(
            DetectionResourceKey::for_playback_source(&local, Some(&actual_remote), None).is_some()
        );
        let catalog = track("musicbrainz", "recording:shared");
        let active_youtube = SourceIdentity::new("youtube", "video-A").unwrap();
        let active_bandcamp = SourceIdentity::new("bandcamp", "track-A").unwrap();
        let youtube_variant = SourceIdentity::new("youtube", "video-A")
            .unwrap()
            .with_duration_seconds(Some(180.0));
        assert!(
            DetectionResourceKey::for_playback_source(&catalog, Some(&active_youtube), None)
                .is_some()
        );
        assert_ne!(
            DetectionResourceKey::for_playback_source(&catalog, Some(&active_youtube), None),
            DetectionResourceKey::for_playback_source(&catalog, Some(&active_bandcamp), None),
        );
        assert_ne!(
            DetectionResourceKey::for_playback_source(&catalog, Some(&active_youtube), None),
            DetectionResourceKey::for_playback_source(&catalog, Some(&youtube_variant), None),
        );
    }

    #[test]
    fn explicit_remote_source_skips_attached_local_asset_hash_verification() {
        let mut catalog_track = track("musicbrainz", "recording:shared");
        catalog_track.asset_id = Some("downloaded-copy".into());
        let remote = SourceIdentity::new("youtube", "active-video").unwrap();
        assert!(!requires_verified_local_hash(
            &catalog_track,
            Some(&remote),
            false
        ));
        assert!(requires_verified_local_hash(&catalog_track, None, false));
        let local = SourceIdentity::new("local", "active-file").unwrap();
        assert!(requires_verified_local_hash(
            &catalog_track,
            Some(&local),
            true
        ));
    }

    #[test]
    fn registered_local_source_identity_supplies_asset_id_without_catalog_asset_id() {
        let catalog_track = track("musicbrainz", "recording:shared");
        let local = SourceIdentity::new("local", "resolved-asset-42").unwrap();
        assert_eq!(
            local_asset_id_for_source(&catalog_track, Some(&local), None),
            Some("resolved-asset-42")
        );
        let remote = SourceIdentity::new("youtube", "video-42").unwrap();
        assert_eq!(
            local_asset_id_for_source(&catalog_track, Some(&remote), None),
            None
        );
    }

    #[test]
    fn local_file_stamp_invalidates_hash_memo_on_content_change() {
        let root = tempdir().unwrap();
        let path = root.path().join("audio.flac");
        std::fs::write(&path, b"first audio bytes").unwrap();
        let before = local_file_stamp(&path).unwrap();
        std::fs::write(&path, b"different audio bytes").unwrap();
        let after = local_file_stamp(&path).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn verified_local_asset_hash_memo_rejects_changed_same_size_file() {
        let (root, store) = store();
        let path = root.path().join("memo-audio.flac");
        std::fs::write(&path, b"first audio bytes").unwrap();
        let local_track = Track {
            id: "local:memo-audio".into(),
            provider: "local".into(),
            provider_id: "memo-audio".into(),
            ..Track::default()
        };
        let hash = crate::media::hash_file(&path).unwrap();
        store
            .save_asset(&LocalAsset {
                id: "detection-memo-asset".into(),
                recording_id: local_track.id.clone(),
                path: path.to_string_lossy().into_owned(),
                hash: hash.clone(),
                source_codec: "flac".into(),
                final_codec: "flac".into(),
                source_bitrate: None,
                final_bitrate: None,
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_seconds: None,
                size_bytes: None,
                bitrate_origin: None,
                processing: "fixture".into(),
                track: local_track,
            })
            .unwrap();

        assert_eq!(
            verified_local_asset_hash(&store, "detection-memo-asset").as_deref(),
            Some(hash.as_str())
        );
        // This exercises the warm cache path before invalidating its stamp.
        assert_eq!(
            verified_local_asset_hash(&store, "detection-memo-asset").as_deref(),
            Some(hash.as_str())
        );
        std::fs::write(&path, b"other audio bytes").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            b"first audio bytes".len() as u64
        );
        assert!(verified_local_asset_hash(&store, "detection-memo-asset").is_none());
    }

    #[test]
    fn opt_in_is_per_resource_default_off_and_disable_preserves_observations() {
        let (_root, store) = store();
        let first = DetectionResourceKey::for_track(&track("youtube", "one"), None).unwrap();
        let second = DetectionResourceKey::for_track(&track("youtube", "two"), None).unwrap();
        assert!(!read(&store, &first).unwrap().enabled);
        assert_eq!(read(&store, &first).unwrap().resource_key, first.as_str());
        set_enabled(&store, &first, true, None).unwrap();
        assert!(read(&store, &first).unwrap().enabled);
        assert!(!read(&store, &second).unwrap().enabled);
        assert!(record_if_enabled(
            &store,
            &first,
            &metadata("Track A", "Artist"),
            None,
            10.0,
            12.0
        )
        .unwrap());
        set_enabled(&store, &first, false, None).unwrap();
        let view = read(&store, &first).unwrap();
        assert!(!view.enabled);
        assert_eq!(view.observations.len(), 1);
        assert!(!record_if_enabled(
            &store,
            &first,
            &metadata("Track B", "Artist"),
            None,
            30.0,
            12.0
        )
        .unwrap());
        assert_eq!(read(&store, &first).unwrap().observations.len(), 1);
    }

    #[test]
    fn repeated_matches_coalesce_after_seekback_but_later_repeats_remain() {
        let (_root, store) = store();
        let key = DetectionResourceKey::for_track(&track("youtube", "set-video"), None).unwrap();
        set_enabled(&store, &key, true, None).unwrap();
        let song = metadata("Track A", "Artist");
        record_if_enabled(&store, &key, &song, None, 0.0, 12.0).unwrap();
        record_if_enabled(&store, &key, &song, None, 35.0, 12.0).unwrap();
        record_if_enabled(&store, &key, &song, None, 70.0, 12.0).unwrap();
        record_if_enabled(
            &store,
            &key,
            &metadata("Track B", "Artist"),
            None,
            100.0,
            12.0,
        )
        .unwrap();
        record_if_enabled(
            &store,
            &key,
            &metadata("Track C", "Artist"),
            None,
            130.0,
            12.0,
        )
        .unwrap();
        record_if_enabled(&store, &key, &song, None, 2.0, 12.0).unwrap();
        let view = read(&store, &key).unwrap();
        assert_eq!(view.observations.len(), 3);
        assert_eq!(view.observations[0].detected_at_seconds, 6.0);
        assert_eq!(view.observations[0].last_confirmed_at_seconds, 76.0);
        assert_eq!(view.observations[0].captured_start_seconds, 0.0);
        record_if_enabled(&store, &key, &song, None, 300.0, 12.0).unwrap();
        assert_eq!(read(&store, &key).unwrap().observations.len(), 4);
        // Seeking back after a much later observation still finds the first
        // point by searching its complete confirmation interval rather than
        // comparing only the latest confirmation or the latest row.
        record_if_enabled(&store, &key, &song, None, 2.0, 12.0).unwrap();
        let view = read(&store, &key).unwrap();
        assert_eq!(view.observations.len(), 4);
        assert_eq!(view.observations[0].last_confirmed_at_seconds, 76.0);
    }

    #[test]
    fn per_resource_retention_is_bounded_without_a_global_observation_cap() {
        let (_root, store) = store();
        let key = DetectionResourceKey::for_track(&track("youtube", "long-set"), None).unwrap();
        set_enabled(&store, &key, true, None).unwrap();
        for index in 0..(MAX_DETECTIONS_PER_RESOURCE + 1) {
            let song = metadata(&format!("Track {index}"), "Artist");
            record_if_enabled(&store, &key, &song, None, index as f64 * 15.0, 12.0).unwrap();
        }
        let view = read(&store, &key).unwrap();
        assert_eq!(view.observations.len(), MAX_DETECTIONS_PER_RESOURCE);
        assert_eq!(view.observations.first().unwrap().detected_at_seconds, 21.0);
        assert_eq!(
            view.observations.last().unwrap().detected_at_seconds,
            3_846.0
        );
    }

    #[test]
    fn conflicting_recognition_ids_do_not_coalesce_even_when_another_id_matches() {
        let (_root, store) = store();
        let key = DetectionResourceKey::for_track(&track("youtube", "conflict-set"), None).unwrap();
        set_enabled(&store, &key, true, None).unwrap();
        let mut first = metadata("Track A", "Artist");
        first.identifiers = Some(crate::recording_identity::RecognitionIdentifiers {
            isrc: Some("USABC2400001".into()),
            shazam_id: Some("1001".into()),
            ..Default::default()
        });
        let mut second = first.clone();
        second.identifiers.as_mut().unwrap().shazam_id = Some("1002".into());
        record_if_enabled(&store, &key, &first, None, 0.0, 12.0).unwrap();
        record_if_enabled(&store, &key, &second, None, 35.0, 12.0).unwrap();
        assert_eq!(read(&store, &key).unwrap().observations.len(), 2);
    }

    #[test]
    fn rejects_bad_intervals_and_never_persists_raw_source_urls() {
        let (_root, store) = store();
        let key = DetectionResourceKey::for_track(&track("youtube", "set-video"), None).unwrap();
        set_enabled(&store, &key, true, None).unwrap();
        assert!(
            record_if_enabled(&store, &key, &metadata("Track", "Artist"), None, 1.0, 12.0).unwrap()
        );
        assert!(!record_if_enabled(
            &store,
            &key,
            &metadata("Track", "Artist"),
            None,
            f64::NAN,
            12.0
        )
        .unwrap());
        let payload = store
            .read_analysis_cache_record(DURABLE_CACHE_NAMESPACE, &durable_cache_key(&key))
            .unwrap()
            .unwrap();
        assert!(serde_json::to_string(&payload).unwrap().contains(&key.0));
        assert!(!serde_json::to_string(&payload)
            .unwrap()
            .contains("https://"));
        assert_eq!(payload["observations"].as_array().unwrap().len(), 1);
        assert!(store.get_json_preference(POLICY_KEY).unwrap().is_some());
    }

    #[test]
    fn backup_cache_and_policy_are_separate_and_policy_projection_resets_opt_in() {
        let (_root, store) = store();
        let key = DetectionResourceKey::for_track(&track("youtube", "set-video"), None).unwrap();
        set_enabled(&store, &key, true, None).unwrap();
        record_if_enabled(
            &store,
            &key,
            &metadata("Track A", "Artist"),
            None,
            2.0,
            12.0,
        )
        .unwrap();
        let cache = serde_json::to_value(DetectionCache {
            version: 1,
            resources: vec![load_cache_record(&store, &key).unwrap().unwrap()],
        })
        .unwrap();
        let policy =
            sanitize_policy_backup_value(store.get_json_preference(POLICY_KEY).unwrap().unwrap())
                .unwrap();
        assert_eq!(
            cache["resources"][0]["observations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(policy["resources"][0]["enabled"], false);
        assert_eq!(cache["resources"][0]["key"], policy["resources"][0]["key"]);
    }
}
