//! Playback-only, bounded Discogs-to-Apple duration recovery.
use crate::remote_playlists::catalog_duration_proof::discogs_apple_duration_crosswalk as crosswalk;
use crate::{
    artists::ArtistService,
    connections::ConnectionManager,
    models::Track,
    remote_playlists::{self, PreparedCatalogDuration},
    storage::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, future::Future, sync::Arc, time::Duration};
use tokio::sync::watch;

const CACHE_NAMESPACE: &str = "playback.discogs-apple-duration.v3";
const CACHE_TTL_SECONDS: u64 = 15 * 60;
const MAX_APPLE_COLLECTIONS: usize = 3;
const MAX_OPPORTUNITY: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogDurationPolicy {
    pub matching_level: crate::recording_match::MatchLevel,
    pub flexible_options: crate::recording_match::FlexibleMatchingOptions,
}

impl CatalogDurationPolicy {
    fn permits_other_release_date(self) -> bool {
        self.matching_level == crate::recording_match::MatchLevel::Flexible
            && self.flexible_options.allow_other_releases
    }

    pub(crate) fn fingerprint(self, request_fingerprint: &str) -> Option<String> {
        let payload = serde_json::to_vec(&json!({
            "requestFingerprint": request_fingerprint,
            "matchingLevel": self.matching_level,
            "flexibleOptions": self.flexible_options,
        }))
        .ok()?;
        Some(format!("{:x}", Sha256::digest(payload)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchRequest {
    DiscogsRelease { release_id: String },
    AppleSearch { term: String },
    AppleArtistAlbums { artist_id: u64 },
    AppleAlbum { collection_id: u64 },
}

fn full_request_value(track: &Track) -> Option<Value> {
    serde_json::to_value(track).ok()
}

pub(crate) fn full_request_fingerprint(track: &Track) -> Option<String> {
    let bytes = serde_json::to_vec(&full_request_value(track)?).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn positive_id(value: &Value) -> Option<u64> {
    value.as_u64().filter(|id| *id > 0)
}

#[derive(Debug, PartialEq, Eq)]
enum DiscogsCredentialState {
    Disabled,
    Enabled(Option<String>),
}

fn enabled_discogs_credential<F>(
    enabled: bool,
    credential: F,
) -> Result<DiscogsCredentialState, String>
where
    F: FnOnce() -> Result<Option<String>, String>,
{
    if enabled {
        credential().map(DiscogsCredentialState::Enabled)
    } else {
        Ok(DiscogsCredentialState::Disabled)
    }
}

fn unlocked_discogs_request(track: &Track) -> bool {
    track.provider == "catalog"
        && track.provenance["catalogProvider"] == "discogs"
        && track.provenance["sourceKind"] == "catalog"
        && track.duration_seconds.is_none()
        && track.playback_range.is_none()
        && track.local_path.is_none()
        && track.asset_id.is_none()
        && track.provenance["chapters"]
            .as_array()
            .is_none_or(|rows| rows.is_empty())
        && track
            .provenance
            .get("playbackCatalogDurationProof")
            .is_none()
        && (track.provenance.get("manualFields").is_none()
            || track.provenance["manualFields"]
                .as_object()
                .is_some_and(|fields| {
                    [
                        "title",
                        "artist",
                        "artistCredits",
                        "album",
                        "albumArtist",
                        "durationSeconds",
                        "version",
                        "trackNumber",
                        "discNumber",
                        "isrc",
                        "catalogNumber",
                    ]
                    .iter()
                    .all(|field| {
                        fields.get(*field).is_none_or(Value::is_boolean)
                            && fields.get(*field).and_then(Value::as_bool) != Some(true)
                    })
                }))
}

fn cache_key(fingerprint: &str) -> String {
    format!("discogs-apple-v3:{fingerprint}")
}

fn apple_artist_album_locator_url(artist_id: u64) -> Result<url::Url, String> {
    crate::compilation_excerpt::apple_endpoint(
        "lookup",
        &[
            ("id", artist_id.to_string()),
            ("entity", "album".into()),
            ("limit", "200".into()),
            ("explicit", "Yes".into()),
        ],
    )
}

fn prepared_from_evidence(
    original: &Track,
    fingerprint: &str,
    policy_fingerprint: &str,
    policy: CatalogDurationPolicy,
    discogs: &Value,
    apple_search: &Value,
    apple_artist_albums: Option<&Value>,
    apple_albums: &[Value],
) -> Option<PreparedCatalogDuration> {
    if apple_albums.is_empty() || apple_albums.len() > MAX_APPLE_COLLECTIONS {
        return None;
    }
    let mut candidate_ids = crosswalk::apple_search_collection_ids(original, apple_search)?;
    if let Some(locator) = apple_artist_albums {
        let artist_id = crosswalk::apple_search_artist_id(original, apple_search)?;
        candidate_ids.extend(crosswalk::apple_artist_album_collection_ids(
            original, artist_id, locator,
        )?);
    }
    candidate_ids.sort_unstable();
    candidate_ids.dedup();
    if candidate_ids.len() > MAX_APPLE_COLLECTIONS || candidate_ids.len() != apple_albums.len() {
        return None;
    }
    let mut album_ids = apple_albums
        .iter()
        .map(|album| positive_id(&album["results"][0]["collectionId"]))
        .collect::<Option<Vec<_>>>()?;
    album_ids.sort_unstable();
    album_ids.dedup();
    if album_ids != candidate_ids {
        return None;
    }
    let mut proofs = apple_albums
        .iter()
        .enumerate()
        .filter_map(|(index, album)| {
            remote_playlists::catalog_duration_proof::discogs_apple_duration_crosswalk::verify(
                original, discogs, album,
            )
                .or_else(|| {
                    policy
                        .permits_other_release_date()
                        .then(|| {
                            remote_playlists::catalog_duration_proof::discogs_apple_duration_crosswalk::verify_same_year_other_release(
                                original,
                                discogs,
                                album,
                                policy.matching_level,
                                policy.flexible_options,
                            )
                        })
                        .flatten()
                })
                .map(|proof| (index, proof))
        })
        .collect::<Vec<_>>();
    if proofs.len() != 1 {
        return None;
    }
    let (apple_index, proof) = proofs.pop()?;
    let source_fingerprint = serde_json::to_vec(&json!({
        "discogsRelease": discogs,
        "appleSearch": apple_search,
        "appleArtistAlbums": apple_artist_albums,
        "appleAlbums": apple_albums,
        "appleAlbum": apple_albums.get(apple_index)?,
    }))
    .ok()
    .map(|bytes| format!("{:x}", Sha256::digest(bytes)))?;
    let mut track = original.clone();
    track.duration_seconds = Some(proof.duration_seconds);
    if !track.provenance.is_object() {
        return None;
    }
    track.provenance["playbackCatalogDurationProof"] = json!({
        "version": 2,
        "provider": "discogsAppleCompleteAlbum",
        "requestFingerprint": fingerprint,
        "policyFingerprint": policy_fingerprint,
        "matchingLevel": policy.matching_level,
        "flexibleOptions": policy.flexible_options,
        "discoveryMethod": if apple_artist_albums.is_some() { "artistAlbumLocator" } else { "songSearch" },
        "releaseDateClaim": proof.evidence["releaseDateClaim"],
        "sourceFingerprint": source_fingerprint,
        "checkedAt": chrono::Utc::now().timestamp(),
        "evidence": proof.evidence,
    });
    Some(PreparedCatalogDuration {
        track,
        original_fingerprint: remote_playlists::alias_hydration_identity(original),
        verified_artist_variants: Vec::new(),
    })
}

fn cached_raw_evidence<'a>(
    value: &'a Value,
    fingerprint: &str,
    policy_fingerprint: &str,
    policy: CatalogDurationPolicy,
) -> Option<(&'a Value, &'a Value, Option<&'a Value>, Vec<&'a Value>)> {
    if value["version"] != 3
        || value["requestFingerprint"] != fingerprint
        || value["policyFingerprint"] != policy_fingerprint
        || value["matchingLevel"] != serde_json::to_value(policy.matching_level).ok()?
        || value["flexibleOptions"] != serde_json::to_value(policy.flexible_options).ok()?
    {
        return None;
    }
    value.get("discogsRelease")?.as_object()?;
    value.get("appleSearch")?.as_object()?;
    let artist_albums = value
        .get("appleArtistAlbums")
        .filter(|value| !value.is_null());
    if artist_albums.is_some_and(|value| !value.is_object()) {
        return None;
    }
    let albums = value["appleAlbums"].as_array()?;
    if albums.is_empty() || albums.len() > MAX_APPLE_COLLECTIONS {
        return None;
    }
    Some((
        value.get("discogsRelease")?,
        value.get("appleSearch")?,
        artist_albums,
        albums.iter().collect(),
    ))
}

async fn bounded<T, Fut>(
    cancel: &watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    future: Fut,
) -> Result<Option<T>, String>
where
    Fut: Future<Output = T>,
{
    if *cancel.borrow() || tokio::time::Instant::now() >= deadline {
        return Ok(None);
    }
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        result = tokio::time::timeout_at(deadline, future) => Ok(result.ok()),
    }
}

async fn checkpoint<Current, Fut>(
    cancel: &watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    current: &Current,
) -> Result<bool, String>
where
    Current: Fn() -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    let Some(unchanged) = bounded(cancel, deadline, current()).await? else {
        return Ok(false);
    };
    Ok(unchanged? && !*cancel.borrow() && tokio::time::Instant::now() < deadline)
}

async fn fetch_step<F, Fut, Current, CurrentFut>(
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    current: &Current,
    fetch: &mut F,
    request: FetchRequest,
) -> Result<Option<Value>, String>
where
    F: FnMut(FetchRequest) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
    Current: Fn() -> CurrentFut,
    CurrentFut: Future<Output = Result<bool, String>>,
{
    if !checkpoint(&cancel, deadline, current).await? {
        return Ok(None);
    }
    let response = match bounded(&cancel, deadline, fetch(request)).await? {
        Some(Ok(value)) => value,
        Some(Err(error)) => return Err(error),
        None => return Ok(None),
    };
    if !checkpoint(&cancel, deadline, current).await? {
        return Ok(None);
    }
    Ok(Some(response))
}

async fn prepare_with_fetch<F, Fut, Current, CurrentFut>(
    store: Arc<Store>,
    original: &Track,
    policy: CatalogDurationPolicy,
    cancel: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    current: Current,
    mut fetch: F,
) -> Result<Option<PreparedCatalogDuration>, String>
where
    F: FnMut(FetchRequest) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
    Current: Fn() -> CurrentFut,
    CurrentFut: Future<Output = Result<bool, String>>,
{
    if !unlocked_discogs_request(original) || *cancel.borrow() {
        return Ok(None);
    }
    let Some(fingerprint) = full_request_fingerprint(original) else {
        return Ok(None);
    };
    let Some(policy_fingerprint) = policy.fingerprint(&fingerprint) else {
        return Ok(None);
    };
    if !checkpoint(&cancel, deadline, &current).await? {
        return Ok(None);
    }

    let cache_store = store.clone();
    let cache_fingerprint = policy_fingerprint.clone();
    let cache_read = tokio::task::spawn_blocking(move || {
        cache_store.get_provider_cache(CACHE_NAMESPACE, &cache_key(&cache_fingerprint))
    });
    let Some(cache_result) = bounded(&cancel, deadline, cache_read).await? else {
        return Ok(None);
    };
    if let Some(cached) = cache_result.map_err(|error| error.to_string())?? {
        if !checkpoint(&cancel, deadline, &current).await? {
            return Ok(None);
        }
        if let Some((discogs, search, artist_albums, albums)) =
            cached_raw_evidence(&cached, &fingerprint, &policy_fingerprint, policy)
        {
            let album_values = albums.into_iter().cloned().collect::<Vec<_>>();
            if let Some(prepared) = prepared_from_evidence(
                original,
                &fingerprint,
                &policy_fingerprint,
                policy,
                discogs,
                search,
                artist_albums,
                &album_values,
            ) {
                return if checkpoint(&cancel, deadline, &current).await? {
                    Ok(Some(prepared))
                } else {
                    Ok(None)
                };
            }
        }
    }

    let release_id = original.provenance["releaseId"]
        .as_str()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 20
                && value.bytes().all(|byte| byte.is_ascii_digit())
        })
        .ok_or("Discogs release ID is missing")?;
    let Some(discogs) = fetch_step(
        cancel.clone(),
        deadline,
        &current,
        &mut fetch,
        FetchRequest::DiscogsRelease {
            release_id: release_id.to_owned(),
        },
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(search) = fetch_step(
        cancel.clone(),
        deadline,
        &current,
        &mut fetch,
        FetchRequest::AppleSearch {
            term: format!("{} {}", original.artist, original.title),
        },
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(mut collection_ids) = crosswalk::apple_search_collection_ids(original, &search) else {
        return Ok(None);
    };
    if collection_ids.len() > MAX_APPLE_COLLECTIONS {
        return Ok(None);
    }
    collection_ids.sort_unstable();
    collection_ids.dedup();
    let mut apple_albums = Vec::with_capacity(collection_ids.len());
    for collection_id in &collection_ids {
        let Some(album) = fetch_step(
            cancel.clone(),
            deadline,
            &current,
            &mut fetch,
            FetchRequest::AppleAlbum {
                collection_id: *collection_id,
            },
        )
        .await?
        else {
            return Ok(None);
        };
        apple_albums.push(album);
    }
    let (prepared, apple_artist_albums) = if let Some(prepared) = prepared_from_evidence(
        original,
        &fingerprint,
        &policy_fingerprint,
        policy,
        &discogs,
        &search,
        None,
        &apple_albums,
    ) {
        (prepared, None)
    } else {
        let Some(artist_id) = crosswalk::apple_search_artist_id(original, &search) else {
            return Ok(None);
        };
        let Some(locator) = fetch_step(
            cancel.clone(),
            deadline,
            &current,
            &mut fetch,
            FetchRequest::AppleArtistAlbums { artist_id },
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(mut located_ids) =
            crosswalk::apple_artist_album_collection_ids(original, artist_id, &locator)
        else {
            return Ok(None);
        };
        collection_ids.append(&mut located_ids);
        collection_ids.sort_unstable();
        collection_ids.dedup();
        if collection_ids.len() > MAX_APPLE_COLLECTIONS {
            return Ok(None);
        }
        let mut fetched_ids = apple_albums
            .iter()
            .filter_map(|album| positive_id(&album["results"][0]["collectionId"]))
            .collect::<HashSet<_>>();
        for collection_id in &collection_ids {
            if !fetched_ids.insert(*collection_id) {
                continue;
            }
            let Some(album) = fetch_step(
                cancel.clone(),
                deadline,
                &current,
                &mut fetch,
                FetchRequest::AppleAlbum {
                    collection_id: *collection_id,
                },
            )
            .await?
            else {
                return Ok(None);
            };
            apple_albums.push(album);
        }
        let Some(prepared) = prepared_from_evidence(
            original,
            &fingerprint,
            &policy_fingerprint,
            policy,
            &discogs,
            &search,
            Some(&locator),
            &apple_albums,
        ) else {
            return Ok(None);
        };
        (prepared, Some(locator))
    };
    if !checkpoint(&cancel, deadline, &current).await? {
        return Ok(None);
    }
    // The Store caps provider-cache entries at 1 MiB. Larger but still bounded
    // raw evidence can be used for this request and simply will not be cached.
    let raw_size = serde_json::to_vec(&discogs)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
        .saturating_add(
            serde_json::to_vec(&search)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX),
        )
        .saturating_add(
            apple_artist_albums
                .as_ref()
                .map(|value| {
                    serde_json::to_vec(value)
                        .map(|bytes| bytes.len())
                        .unwrap_or(usize::MAX)
                })
                .unwrap_or(0),
        )
        .saturating_add(
            apple_albums
                .iter()
                .map(|album| {
                    serde_json::to_vec(album)
                        .map(|bytes| bytes.len())
                        .unwrap_or(usize::MAX)
                })
                .fold(0usize, usize::saturating_add),
        );
    if raw_size <= 900 * 1024 {
        let cache = json!({
            "version": 3,
            "requestFingerprint": fingerprint,
            "policyFingerprint": policy_fingerprint,
            "matchingLevel": policy.matching_level,
            "flexibleOptions": policy.flexible_options,
            "discogsRelease": discogs,
            "appleSearch": search,
            "appleArtistAlbums": apple_artist_albums,
            "appleAlbums": apple_albums,
        });
        let cache_store = store.clone();
        let cache_key = cache_key(&policy_fingerprint);
        let cache_write = tokio::task::spawn_blocking(move || {
            cache_store.set_provider_cache(CACHE_NAMESPACE, &cache_key, &cache, CACHE_TTL_SECONDS)
        });
        if let Some(result) = bounded(&cancel, deadline, cache_write).await? {
            let _ = result;
        } else {
            return Ok(None);
        }
    }
    if !checkpoint(&cancel, deadline, &current).await? {
        return Ok(None);
    }
    Ok(Some(prepared))
}

pub(crate) async fn prepare(
    store: &Arc<Store>,
    connections: &Arc<ConnectionManager>,
    original: &Track,
    policy: CatalogDurationPolicy,
    cancel: watch::Receiver<bool>,
    playback_deadline: tokio::time::Instant,
) -> Result<Option<PreparedCatalogDuration>, String> {
    if !unlocked_discogs_request(original) {
        return Ok(None);
    }
    let deadline = playback_deadline.min(tokio::time::Instant::now() + MAX_OPPORTUNITY);
    let snapshot_store = store.clone();
    let track_id = original.id.clone();
    let saved_future = tokio::task::spawn_blocking(move || snapshot_store.get_track(&track_id));
    let Some(saved_result) = bounded(&cancel, deadline, saved_future).await? else {
        return Ok(None);
    };
    let Some(saved) = saved_result.map_err(|error| error.to_string())?? else {
        return Ok(None);
    };
    let Some(request_fingerprint) = full_request_fingerprint(original) else {
        return Ok(None);
    };
    if full_request_fingerprint(&saved).as_deref() != Some(request_fingerprint.as_str()) {
        return Ok(None);
    }
    let saved_snapshot = full_request_value(&saved).ok_or("Could not snapshot saved track")?;
    let current_store = store.clone();
    let current_id = original.id.clone();
    let current_snapshot = saved_snapshot.clone();
    let current_cancel = cancel.clone();
    let current_deadline = deadline;
    let current = move || {
        let store = current_store.clone();
        let id = current_id.clone();
        let snapshot = current_snapshot.clone();
        let cancel = current_cancel.clone();
        let deadline = current_deadline;
        async move {
            let read = tokio::task::spawn_blocking(move || store.get_track(&id));
            let Some(result) = bounded(&cancel, deadline, read).await? else {
                return Ok(false);
            };
            let Some(track) = result.map_err(|error| error.to_string())?? else {
                return Ok(false);
            };
            Ok(full_request_value(&track).as_ref() == Some(&snapshot))
        }
    };
    let credentials_manager = connections.clone();
    let credentials_future = tokio::task::spawn_blocking(move || {
        let settings = credentials_manager.settings()?;
        enabled_discogs_credential(settings.discogs_enabled, || {
            credentials_manager.credential("discogs")
        })
    });
    let Some(credentials_result) = bounded(&cancel, deadline, credentials_future).await? else {
        return Ok(None);
    };
    let credentials = credentials_result.map_err(|error| error.to_string())??;
    let DiscogsCredentialState::Enabled(token) = credentials else {
        return Ok(None);
    };
    let artist_service = ArtistService::new(store.clone())?;
    let cancel_for_fetch = cancel.clone();
    prepare_with_fetch(
        store.clone(),
        original,
        policy,
        cancel,
        deadline,
        current,
        |request| {
            let service = artist_service.clone();
            let token = token.clone();
            let cancel = cancel_for_fetch.clone();
            async move {
                match request {
                    FetchRequest::DiscogsRelease { release_id } => {
                        service
                            .fresh_discogs_release_for_playback(
                                &release_id,
                                token.as_deref(),
                                cancel,
                                deadline,
                            )
                            .await
                    }
                    FetchRequest::AppleSearch { term } => {
                        let url = crate::compilation_excerpt::apple_endpoint(
                            "search",
                            &[
                                ("term", term),
                                ("entity", "song".into()),
                                ("limit", "25".into()),
                            ],
                        )?;
                        crate::compilation_excerpt::apple_json(url, cancel).await
                    }
                    FetchRequest::AppleArtistAlbums { artist_id } => {
                        let url = apple_artist_album_locator_url(artist_id)?;
                        crate::compilation_excerpt::apple_json(url, cancel).await
                    }
                    FetchRequest::AppleAlbum { collection_id } => {
                        let url = crate::compilation_excerpt::apple_endpoint(
                            "lookup",
                            &[
                                ("id", collection_id.to_string()),
                                ("entity", "song".into()),
                                ("limit", "200".into()),
                            ],
                        )?;
                        crate::compilation_excerpt::apple_json(url, cancel).await
                    }
                }
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture() -> (Track, Value, Value) {
        let track = serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-discogs-request-track.json"
        ))
        .unwrap();
        let release =
            serde_json::from_str(include_str!("../tests/fixtures/bacc-discogs-release.json"))
                .unwrap();
        let apple = serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-apple-album-lookup.json"
        ))
        .unwrap();
        (track, release, apple)
    }

    fn current_apple_fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-apple-current-album-lookup.json"
        ))
        .unwrap()
    }

    fn artist_album_locator_fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-apple-artist-albums.json"
        ))
        .unwrap()
    }

    fn strict_policy() -> CatalogDurationPolicy {
        CatalogDurationPolicy {
            matching_level: crate::recording_match::MatchLevel::Balanced,
            flexible_options: crate::recording_match::FlexibleMatchingOptions::default(),
        }
    }

    #[test]
    fn anonymous_release_access_requires_enabled_source_and_preserves_credential_errors() {
        let reads = Arc::new(AtomicUsize::new(0));
        let disabled_reads = reads.clone();
        assert_eq!(
            enabled_discogs_credential(false, move || {
                disabled_reads.fetch_add(1, Ordering::SeqCst);
                Err("must not inspect credential storage while disabled".into())
            })
            .unwrap(),
            DiscogsCredentialState::Disabled
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);

        assert_eq!(
            enabled_discogs_credential(true, || Ok(None)).unwrap(),
            DiscogsCredentialState::Enabled(None),
            "enabled public catalog reads can proceed without a token"
        );
        assert_eq!(
            enabled_discogs_credential(true, || Ok(Some("test-token".into()))).unwrap(),
            DiscogsCredentialState::Enabled(Some("test-token".into()))
        );
        assert_eq!(
            enabled_discogs_credential(true, || Err("keyring unavailable".into())).unwrap_err(),
            "keyring unavailable",
            "credential access errors never silently fall back to anonymous access"
        );
    }

    #[test]
    fn apple_artist_album_locator_uses_bounded_explicit_lookup_parameters() {
        let url = apple_artist_album_locator_url(128860700).unwrap();
        let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(query.get("id").map(String::as_str), Some("128860700"));
        assert_eq!(query.get("entity").map(String::as_str), Some("album"));
        assert_eq!(query.get("limit").map(String::as_str), Some("200"));
        assert_eq!(query.get("explicit").map(String::as_str), Some("Yes"));
    }

    fn test_store() -> (tempfile::TempDir, Arc<Store>) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("catalog.db")).unwrap());
        (directory, store)
    }

    fn search_result(apple: &Value) -> Value {
        json!({"resultCount":1,"results":[apple["results"][15].clone()]})
    }

    fn response_for(
        request: FetchRequest,
        release: &Value,
        apple: &Value,
    ) -> Result<Value, String> {
        match request {
            FetchRequest::DiscogsRelease { .. } => Ok(release.clone()),
            FetchRequest::AppleSearch { .. } => Ok(search_result(apple)),
            FetchRequest::AppleArtistAlbums { .. } => Ok(artist_album_locator_fixture()),
            FetchRequest::AppleAlbum { .. } => Ok(apple.clone()),
        }
    }

    fn response_for_artist_locator(
        request: FetchRequest,
        release: &Value,
        current: &Value,
        old: &Value,
        locator: &Value,
    ) -> Result<Value, String> {
        match request {
            FetchRequest::DiscogsRelease { .. } => Ok(release.clone()),
            FetchRequest::AppleSearch { .. } => Ok(search_result(current)),
            FetchRequest::AppleArtistAlbums { .. } => Ok(locator.clone()),
            FetchRequest::AppleAlbum { collection_id } => {
                let current_id = positive_id(&current["results"][0]["collectionId"]).unwrap();
                let old_id = positive_id(&old["results"][0]["collectionId"]).unwrap();
                if collection_id == current_id {
                    Ok(current.clone())
                } else if collection_id == old_id {
                    Ok(old.clone())
                } else {
                    Err("Unexpected Apple album lookup ID".into())
                }
            }
        }
    }

    #[tokio::test]
    async fn actual_crosswalk_prepares_duration_and_positive_cache_hit_reverifies() {
        let (_directory, store) = test_store();
        let (track, release, apple) = fixture();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_counter = requests.clone();
        let (stop, cancel) = watch::channel(false);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let first = prepare_with_fetch(
            store.clone(),
            &track,
            strict_policy(),
            cancel.clone(),
            deadline,
            || async { Ok(true) },
            move |request| {
                request_counter.fetch_add(1, Ordering::SeqCst);
                let result = response_for(request, &release, &apple);
                async move { result }
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(first.track.duration_seconds, Some(197.9));
        assert_eq!(first.track.isrc, track.isrc);
        assert_eq!(
            first.track.provenance["artistCredits"],
            track.provenance["artistCredits"]
        );
        assert_eq!(
            first.track.provenance["playbackCatalogDurationProof"]["provider"],
            "discogsAppleCompleteAlbum"
        );
        let first_calls = requests.load(Ordering::SeqCst);
        assert_eq!(first_calls, 3);

        let no_calls = Arc::new(AtomicUsize::new(0));
        let no_call_counter = no_calls.clone();
        let cached = prepare_with_fetch(
            store,
            &track,
            strict_policy(),
            cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |_request| {
                no_call_counter.fetch_add(1, Ordering::SeqCst);
                async { Err("unexpected cache miss".into()) }
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(no_calls.load(Ordering::SeqCst), 0);
        assert_eq!(cached.track.duration_seconds, Some(197.9));
        stop.send_replace(true);
    }

    #[tokio::test]
    async fn artist_album_locator_recovers_strict_edition_and_cache_is_policy_bound() {
        let (_directory, store) = test_store();
        let (track, release, old) = fixture();
        let current = current_apple_fixture();
        let locator = artist_album_locator_fixture();
        let flexible_policy = CatalogDurationPolicy {
            matching_level: crate::recording_match::MatchLevel::Flexible,
            flexible_options: crate::recording_match::FlexibleMatchingOptions::default(),
        };
        let (stop, cancel) = watch::channel(false);
        let calls = Arc::new(AtomicUsize::new(0));
        let call_counter = calls.clone();
        let request_log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let request_log_for_fetch = request_log.clone();
        let flexible_release = release.clone();
        let flexible_current = current.clone();
        let flexible_old = old.clone();
        let flexible_locator = locator.clone();
        let prepared = prepare_with_fetch(
            store.clone(),
            &track,
            flexible_policy,
            cancel.clone(),
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |request| {
                call_counter.fetch_add(1, Ordering::SeqCst);
                request_log_for_fetch.lock().unwrap().push(request.clone());
                let result = response_for_artist_locator(
                    request,
                    &flexible_release,
                    &flexible_current,
                    &flexible_old,
                    &flexible_locator,
                );
                async move { result }
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(prepared.track.duration_seconds, Some(197.9));
        assert_eq!(
            prepared.track.provenance["playbackCatalogDurationProof"]["releaseDateClaim"],
            "dateMatched"
        );
        assert_eq!(
            prepared.track.provenance["playbackCatalogDurationProof"]["discoveryMethod"],
            "artistAlbumLocator"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        let current_id = positive_id(&current["results"][0]["collectionId"]).unwrap();
        let old_id = positive_id(&old["results"][0]["collectionId"]).unwrap();
        let expected_artist_id = remote_playlists::catalog_duration_proof::discogs_apple_duration_crosswalk::apple_search_artist_id(
            &track,
            &search_result(&current),
        )
        .unwrap();
        assert_eq!(
            *request_log.lock().unwrap(),
            vec![
                FetchRequest::DiscogsRelease {
                    release_id: track.provenance["releaseId"].as_str().unwrap().to_owned(),
                },
                FetchRequest::AppleSearch {
                    term: format!("{} {}", track.artist, track.title),
                },
                FetchRequest::AppleAlbum {
                    collection_id: current_id,
                },
                FetchRequest::AppleArtistAlbums {
                    artist_id: expected_artist_id,
                },
                FetchRequest::AppleAlbum {
                    collection_id: old_id,
                },
            ]
        );

        let no_calls = Arc::new(AtomicUsize::new(0));
        let no_call_counter = no_calls.clone();
        let cached = prepare_with_fetch(
            store.clone(),
            &track,
            flexible_policy,
            cancel.clone(),
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |_request| {
                no_call_counter.fetch_add(1, Ordering::SeqCst);
                async { Err("unexpected locator cache miss".into()) }
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(no_calls.load(Ordering::SeqCst), 0);
        assert_eq!(cached.track.duration_seconds, Some(197.9));

        for (level, allow_other_releases) in [
            (crate::recording_match::MatchLevel::Exact, true),
            (crate::recording_match::MatchLevel::Balanced, true),
        ] {
            let policy = CatalogDurationPolicy {
                matching_level: level,
                flexible_options: crate::recording_match::FlexibleMatchingOptions {
                    allow_other_releases,
                    ..Default::default()
                },
            };
            let policy_calls = Arc::new(AtomicUsize::new(0));
            let policy_counter = policy_calls.clone();
            let policy_release = release.clone();
            let policy_current = current.clone();
            let policy_old = old.clone();
            let policy_locator = locator.clone();
            let result = prepare_with_fetch(
                store.clone(),
                &track,
                policy,
                cancel.clone(),
                tokio::time::Instant::now() + Duration::from_secs(2),
                || async { Ok(true) },
                move |request| {
                    policy_counter.fetch_add(1, Ordering::SeqCst);
                    let result = response_for_artist_locator(
                        request,
                        &policy_release,
                        &policy_current,
                        &policy_old,
                        &policy_locator,
                    );
                    async move { result }
                },
            )
            .await
            .unwrap();
            let result = result.expect("the located explicit release has an exact date proof");
            assert_eq!(result.track.duration_seconds, Some(197.9));
            assert_eq!(
                result.track.provenance["playbackCatalogDurationProof"]["releaseDateClaim"],
                "dateMatched"
            );
            assert_eq!(policy_calls.load(Ordering::SeqCst), 5);
        }

        let no_cross_policy_calls = Arc::new(AtomicUsize::new(0));
        let no_cross_policy_counter = no_cross_policy_calls.clone();
        let deny_policy = CatalogDurationPolicy {
            matching_level: crate::recording_match::MatchLevel::Flexible,
            flexible_options: crate::recording_match::FlexibleMatchingOptions {
                allow_other_releases: false,
                ..Default::default()
            },
        };
        let no_cross_policy = prepare_with_fetch(
            store,
            &track,
            deny_policy,
            cancel.clone(),
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |_request| {
                no_cross_policy_counter.fetch_add(1, Ordering::SeqCst);
                async { Err("policy-specific cache miss expected".into()) }
            },
        )
        .await;
        assert!(
            no_cross_policy.is_err(),
            "cache cannot cross policy snapshots"
        );
        assert_eq!(no_cross_policy_calls.load(Ordering::SeqCst), 1);
        stop.send_replace(true);
    }

    #[tokio::test]
    async fn artist_album_locator_obeys_global_three_collection_cap() {
        let (_directory, store) = test_store();
        let (track, release, _) = fixture();
        let current = current_apple_fixture();
        let current_id = positive_id(&current["results"][0]["collectionId"]).unwrap();
        let mut locator = artist_album_locator_fixture();
        let rows = locator["results"].as_array_mut().unwrap();
        let current_row = rows
            .iter()
            .position(|row| row["collectionId"].as_u64() == Some(current_id))
            .unwrap();
        rows.remove(current_row);
        rows.pop();
        let template = rows
            .iter()
            .find(|row| row["collectionName"] == "PROLIFIC")
            .unwrap()
            .clone();
        for id in [7770000001_u64, 7770000002] {
            let mut candidate = template.clone();
            candidate["collectionId"] = json!(id);
            rows.push(candidate);
        }
        assert_eq!(rows.len(), 201);

        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let request_log = calls.clone();
        let (stop, cancel) = watch::channel(false);
        let result = prepare_with_fetch(
            store,
            &track,
            strict_policy(),
            cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |request| {
                request_log.lock().unwrap().push(request.clone());
                let result = match &request {
                    FetchRequest::DiscogsRelease { .. } => Ok(release.clone()),
                    FetchRequest::AppleSearch { .. } => Ok(search_result(&current)),
                    FetchRequest::AppleAlbum { collection_id } if *collection_id == current_id => {
                        Ok(current.clone())
                    }
                    FetchRequest::AppleArtistAlbums { .. } => Ok(locator.clone()),
                    FetchRequest::AppleAlbum { .. } => {
                        Err("global candidate cap should prevent this lookup".into())
                    }
                };
                async move { result }
            },
        )
        .await
        .unwrap();
        assert!(result.is_none());
        let requests = calls.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| matches!(request, FetchRequest::AppleAlbum { .. }))
                .count(),
            1
        );
        assert!(matches!(
            requests.last(),
            Some(FetchRequest::AppleArtistAlbums { .. })
        ));
        stop.send_replace(true);
    }

    #[tokio::test]
    async fn stale_saved_track_and_manual_lock_stop_before_more_requests() {
        let (_directory, store) = test_store();
        let (track, release, apple) = fixture();
        let checks = Arc::new(AtomicUsize::new(0));
        let check_counter = checks.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let request_counter = calls.clone();
        let (_stop, cancel) = watch::channel(false);
        let stale = prepare_with_fetch(
            store.clone(),
            &track,
            strict_policy(),
            cancel.clone(),
            tokio::time::Instant::now() + Duration::from_secs(2),
            move || {
                let count = check_counter.fetch_add(1, Ordering::SeqCst);
                async move { Ok(count < 2) }
            },
            move |request| {
                request_counter.fetch_add(1, Ordering::SeqCst);
                let result = response_for(request, &release, &apple);
                async move { result }
            },
        )
        .await
        .unwrap();
        assert!(stale.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut manually_locked = track.clone();
        manually_locked.provenance["manualFields"]["durationSeconds"] = json!(true);
        let no_calls = Arc::new(AtomicUsize::new(0));
        let counter = no_calls.clone();
        let locked = prepare_with_fetch(
            store,
            &manually_locked,
            strict_policy(),
            cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |_request| {
                counter.fetch_add(1, Ordering::SeqCst);
                async { Err("manual lock should stop before fetch".into()) }
            },
        )
        .await
        .unwrap();
        assert!(locked.is_none());
        assert_eq!(no_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancellation_deadline_and_provider_failure_never_publish() {
        let (_directory, store) = test_store();
        let (track, _release, _apple) = fixture();
        let (stop, cancel) = watch::channel(false);
        let started = Arc::new(tokio::sync::Notify::new());
        let notified = started.clone();
        let pending = prepare_with_fetch(
            store.clone(),
            &track,
            strict_policy(),
            cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |_request| {
                let notify = started.clone();
                async move {
                    notify.notify_one();
                    std::future::pending::<Result<Value, String>>().await
                }
            },
        );
        let cancel = async move {
            notified.notified().await;
            stop.send_replace(true);
        };
        let (result, ()) = tokio::join!(pending, cancel);
        assert_eq!(result.unwrap_err(), "Cancelled");

        let (_stop, cancel) = watch::channel(false);
        let deadline = prepare_with_fetch(
            store.clone(),
            &track,
            strict_policy(),
            cancel.clone(),
            tokio::time::Instant::now() + Duration::from_millis(20),
            || async { Ok(true) },
            |_request| async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(json!({}))
            },
        )
        .await
        .unwrap();
        assert!(deadline.is_none());

        let (_stop, cancel) = watch::channel(false);
        let failure = prepare_with_fetch(
            store,
            &track,
            strict_policy(),
            cancel,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || async { Ok(true) },
            |_request| async { Err("provider unavailable".into()) },
        )
        .await;
        assert!(failure.is_err());
    }

    #[tokio::test]
    async fn multiple_independently_verified_apple_editions_are_rejected() {
        let (_directory, store) = test_store();
        let (track, release, apple) = fixture();
        let mut second_apple = apple.clone();
        for row in second_apple["results"].as_array_mut().unwrap() {
            if row.get("collectionId").is_some() {
                row["collectionId"] = json!(6777173610_u64);
            }
        }
        let mut second_search_row = apple["results"][15].clone();
        second_search_row["collectionId"] = json!(6777173610_u64);
        let search =
            json!({"resultCount":2,"results":[apple["results"][15].clone(),second_search_row]});
        let (_stop, cancel) = watch::channel(false);
        let result = prepare_with_fetch(
            store,
            &track,
            strict_policy(),
            cancel,
            tokio::time::Instant::now() + Duration::from_secs(2),
            || async { Ok(true) },
            move |request| {
                let response = match request {
                    FetchRequest::DiscogsRelease { .. } => release.clone(),
                    FetchRequest::AppleSearch { .. } => search.clone(),
                    FetchRequest::AppleArtistAlbums { .. } => json!({"resultCount":0,"results":[]}),
                    FetchRequest::AppleAlbum { collection_id } if collection_id == 6777173610 => {
                        second_apple.clone()
                    }
                    FetchRequest::AppleAlbum { .. } => apple.clone(),
                };
                async move { Ok(response) }
            },
        )
        .await
        .unwrap();
        assert!(result.is_none());
    }
}
