//! Request-scoped lexical aliases derived from an exact Discogs release row.
//!
//! This module never creates recording, ISRC, source, or completeness proof.
//! It only returns alternative names for the exact ordered Spotify artist
//! credits when public authority links and an exact release occurrence agree.
//! The replay fixture normalizes only its invalid empty `localAvailability`
//! field to the Track enum's `none` value; identity fields remain byte-for-byte.
use crate::Store;
use crate::{artist_credit_aliases, artists, models::Track};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};

const MAX_RELEASE_ID_LEN: usize = 20;
const MAX_TRACK_CREDITS: usize = 16;
const MAX_NAME_LEN: usize = 256;
const MAX_RUNTIME_DELTA_MS: u64 = 5_000;
const MAX_RELEASE_LOCATORS: usize = 5;
const MAX_ALIAS_PROFILE_FETCHES: usize = 4;
const DISCOVERY_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;
const PROVIDER_ARTIST_CACHE: &str = "artists.alias-discovery.provider-artist.v1";
const MB_PROFILE_CACHE: &str = "artists.profile.musicbrainz";
const MB_RELEASE_CACHE: &str = "artists.alias-discovery.discogs-link-release-musicbrainz.v1";
const MB_FETCHED_AT_CACHE: &str = "artists.alias-discovery.fetched-at.v1";
const DISCOGS_FETCHED_AT_CACHE: &str = "artists.alias-discovery.discogs-link-fetched-at.v1";
const ALBUM_URL_CACHE: &str = "artists.alias-discovery.spotify-album-musicbrainz-url.v1";
const RELEASE_SEARCH_CACHE: &str = "artists.alias-discovery.spotify-album-release-search.v1";
const LEXICAL_BINDING_CACHE: &str = "artists.alias-discovery.discogs-release-lexical.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpotifyRequest {
    pub request_key: String,
    pub spotify_track_id: String,
    pub spotify_album_id: String,
    pub album_title: String,
    pub track_title: String,
    pub album_position: u64,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    pub credits: Vec<artist_credit_aliases::SpotifyArtistCreditRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedLexicalCredits {
    pub release_id: String,
    pub track_position: String,
    pub names: Vec<Vec<String>>,
    pub join_phrases: Vec<String>,
}

/// Warm cache reads rederive every artist mapping from the retained raw MB
/// provider binding/profile and reparse the exact MB/Discogs release payloads.
pub(crate) fn cached_lexical_credits(
    store: &Store,
    track: &Track,
) -> Option<VerifiedLexicalCredits> {
    let request = spotify_request(track)?;
    let binding = store
        .get_provider_cache(LEXICAL_BINDING_CACHE, &request.request_key)
        .ok()??;
    if binding["version"] != 1
        || binding["requestKey"].as_str() != Some(request.request_key.as_str())
    {
        return None;
    }
    let mb_release_id = binding["musicbrainzReleaseId"].as_str()?;
    let discogs_release_id = binding["discogsReleaseId"].as_str()?;
    let locator_kind = binding["locatorKind"].as_str()?;
    if !artist_credit_aliases::valid_musicbrainz_recording_id(mb_release_id)
        || !valid_discogs_id(discogs_release_id)
    {
        return None;
    }
    if !cached_locator_contains(store, &request, locator_kind, mb_release_id) {
        return None;
    }
    let mapped_ids = request
        .credits
        .iter()
        .map(|credit| cached_discogs_artist_id(store, &credit.id, &credit.name))
        .collect::<Option<Vec<_>>>()?;
    let mb_release = fresh_mb_cache(store, MB_RELEASE_CACHE, mb_release_id)?;
    let discogs_release = fresh_discogs_cache(
        store,
        artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE,
        discogs_release_id,
    )?;
    let aliases = needed_alias_profiles(&request, &mb_release, &discogs_release, &mapped_ids)?
        .into_iter()
        .map(|id| {
            let profile = fresh_discogs_cache(
                store,
                artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE,
                &id,
            )?;
            Some((id, profile))
        })
        .collect::<Option<HashMap<_, _>>>()?;
    verify_release_track(
        &request,
        &mb_release,
        &discogs_release,
        discogs_release_id,
        &mapped_ids,
        &aliases,
    )
}

pub(crate) fn has_cached_lexical_alias(store: &Store, track: &Track) -> bool {
    cached_lexical_credits(store, track).is_some_and(|verified| {
        verified.names.iter().any(|names| {
            names
                .iter()
                .skip(1)
                .any(|name| normalize(name) != normalize(&names[0]))
        })
    })
}

fn cached_discogs_artist_id(
    store: &Store,
    spotify_artist_id: &str,
    requested_name: &str,
) -> Option<String> {
    if !valid_spotify_id(spotify_artist_id) {
        return None;
    }
    let key = format!("spotify:{spotify_artist_id}");
    let binding = store
        .get_provider_cache(PROVIDER_ARTIST_CACHE, &key)
        .ok()??;
    let fetched_at = binding["fetchedAt"].as_i64()?;
    if !timestamp_fresh(fetched_at)
        || binding["version"] != 1
        || binding["provider"].as_str() != Some("spotify")
        || binding["providerArtistId"].as_str() != Some(spotify_artist_id)
    {
        return None;
    }
    let mb_artist_id = binding["musicbrainzArtistId"].as_str()?;
    if !artist_credit_aliases::valid_musicbrainz_recording_id(mb_artist_id) {
        return None;
    }
    let spotify_url = format!("https://open.spotify.com/artist/{spotify_artist_id}");
    if !exact_musicbrainz_url_artist(&binding["lookup"], &spotify_url, mb_artist_id)
        || !exact_musicbrainz_spotify_artist_profile(&binding["profile"], spotify_artist_id)
    {
        return None;
    }
    let raw_profile = store
        .get_provider_cache(MB_PROFILE_CACHE, mb_artist_id)
        .ok()??;
    if raw_profile != binding["profile"]
        || raw_profile["id"].as_str() != Some(mb_artist_id)
        || !musicbrainz_profile_owns_name(&raw_profile, requested_name)
        || !provider_cache_fresh(
            store,
            MB_FETCHED_AT_CACHE,
            &format!("{MB_PROFILE_CACHE}:{mb_artist_id}"),
        )
    {
        return None;
    }
    let discogs_ids = relation_ids(&raw_profile, "discogs.com", "/artist/")?;
    (discogs_ids.len() == 1)
        .then(|| discogs_ids.into_iter().next())
        .flatten()
}

pub(crate) fn musicbrainz_profile_owns_name(profile: &Value, requested_name: &str) -> bool {
    let Some(requested) = bounded_name(requested_name) else {
        return false;
    };
    // This comparison is scoped by the exact Spotify artist URL, unique
    // MusicBrainz artist profile, and that profile's bounded canonical/alias
    // names. Honor the existing single-artist `&`/`and` spelling equivalence
    // here so valid provider display names can match the profile's real alias.
    let requested = crate::matching::normalize_single_artist_name(requested);
    if requested.is_empty() {
        return false;
    }
    let mut names = BTreeSet::new();
    if let Some(name) = profile
        .get("name")
        .and_then(Value::as_str)
        .and_then(bounded_name)
    {
        names.insert(crate::matching::normalize_single_artist_name(name));
    }
    if let Some(aliases) = profile.get("aliases").and_then(Value::as_array) {
        for alias in aliases {
            if alias["type"]
                .as_str()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("search hint"))
            {
                continue;
            }
            let Some(name) = alias
                .get("name")
                .and_then(Value::as_str)
                .and_then(bounded_name)
            else {
                continue;
            };
            names.insert(crate::matching::normalize_single_artist_name(name));
        }
    }
    names.contains(&requested)
}

fn exact_musicbrainz_url_artist(lookup: &Value, resource: &str, artist_id: &str) -> bool {
    if lookup["resource"].as_str() != Some(resource) {
        return false;
    }
    let Some(relations) = lookup["relations"].as_array() else {
        return false;
    };
    let ids = relations
        .iter()
        .filter(|relation| relation["target-type"].as_str() == Some("artist"))
        .filter_map(|relation| relation["artist"]["id"].as_str())
        .collect::<BTreeSet<_>>();
    ids.len() == 1 && ids.contains(artist_id)
}

fn exact_musicbrainz_spotify_artist_profile(profile: &Value, spotify_artist_id: &str) -> bool {
    if profile["id"]
        .as_str()
        .is_none_or(|id| !artist_credit_aliases::valid_musicbrainz_recording_id(id))
    {
        return false;
    }
    let expected = format!("https://open.spotify.com/artist/{spotify_artist_id}");
    let urls = relation_resources(profile)
        .filter(|url| {
            url::Url::parse(url).ok().is_some_and(|url| {
                url.host_str() == Some("open.spotify.com") && url.path().starts_with("/artist/")
            })
        })
        .collect::<BTreeSet<_>>();
    urls.len() == 1 && urls.contains(expected.as_str())
}

fn relation_ids(profile: &Value, host: &str, prefix: &str) -> Option<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for resource in relation_resources(profile) {
        let Some(id) = id_from_public_relation_url(resource, host, prefix) else {
            continue;
        };
        ids.insert(id);
    }
    (!ids.is_empty()).then_some(ids)
}

fn relation_resources(profile: &Value) -> impl Iterator<Item = &str> {
    profile["relations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|relation| relation["target-type"].as_str() == Some("url"))
        .filter_map(|relation| relation["url"]["resource"].as_str())
}

fn id_from_public_relation_url(resource: &str, host: &str, prefix: &str) -> Option<String> {
    let url = url::Url::parse(resource).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("discogs.com" | "www.discogs.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if !matches!(host, "discogs.com" | "www.discogs.com") {
        return None;
    }
    let mut segments = url.path_segments()?;
    let resource_prefix = prefix.strip_prefix('/')?;
    let expected_kind = resource_prefix.strip_suffix('/')?;
    if segments.next()? != expected_kind {
        return None;
    }
    let id_and_slug = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    let id = id_and_slug.split('-').next()?;
    valid_discogs_id(id).then(|| id.to_owned())
}

async fn musicbrainz_release_locators(
    store: &Store,
    request: &SpotifyRequest,
) -> Result<(Vec<String>, &'static str), String> {
    musicbrainz_release_locators_with(store, request, fetch_musicbrainz_locator).await
}

type LocatorQuery = Vec<(&'static str, String)>;

async fn fetch_musicbrainz_locator(url: String, query: LocatorQuery) -> Result<Value, String> {
    crate::artist_alias_discovery::fetch(&url, &query).await
}

async fn musicbrainz_release_locators_with<F, Fut>(
    store: &Store,
    request: &SpotifyRequest,
    mut fetcher: F,
) -> Result<(Vec<String>, &'static str), String>
where
    F: FnMut(String, LocatorQuery) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    let album_url = format!(
        "https://open.spotify.com/album/{}",
        request.spotify_album_id
    );
    let raw_lookup = if provider_cache_fresh(
        store,
        MB_FETCHED_AT_CACHE,
        &format!("{ALBUM_URL_CACHE}:{}", request.spotify_album_id),
    ) {
        store
            .get_provider_cache(ALBUM_URL_CACHE, &request.spotify_album_id)?
            .ok_or_else(|| "Cached MusicBrainz album URL lookup disappeared".to_owned())?
    } else {
        let raw = match fetcher(
            "https://musicbrainz.org/ws/2/url/".to_owned(),
            vec![
                ("resource", album_url.clone()),
                ("inc", "release-rels".into()),
                ("fmt", "json".into()),
            ],
        )
        .await
        {
            Ok(raw) => raw,
            // A Spotify album URL can legitimately have no MusicBrainz URL
            // entity. In that one case, use the same bounded release search
            // as an exact URL response with no release relations. Other
            // failures (429/503/transport/invalid JSON) must remain failures.
            Err(error) if error == "Artist discovery HTTP 404" => {
                return musicbrainz_release_search_with(store, request, &mut fetcher).await;
            }
            Err(error) => return Err(error),
        };
        validate_album_url_lookup(&raw, &album_url)?;
        store.set_retained_provider_cache(ALBUM_URL_CACHE, &request.spotify_album_id, &raw)?;
        store.set_provider_cache(
            MB_FETCHED_AT_CACHE,
            &format!("{ALBUM_URL_CACHE}:{}", request.spotify_album_id),
            &serde_json::json!({"fetchedAt":chrono::Utc::now().timestamp()}),
            DISCOVERY_TTL_SECONDS,
        )?;
        raw
    };
    validate_album_url_lookup(&raw_lookup, &album_url)?;
    let mut ids = BTreeSet::new();
    for relation in raw_lookup["relations"]
        .as_array()
        .expect("validated relations")
    {
        if relation["target-type"].as_str() != Some("release") {
            continue;
        }
        if let Some(id) = relation["release"]["id"]
            .as_str()
            .filter(|id| artist_credit_aliases::valid_musicbrainz_recording_id(id))
        {
            ids.insert(id.to_owned());
        }
    }
    if ids.len() > MAX_RELEASE_LOCATORS {
        return Ok((Vec::new(), "spotify_album_url"));
    }
    if !ids.is_empty() {
        return Ok((ids.into_iter().collect(), "spotify_album_url"));
    }
    musicbrainz_release_search_with(store, request, &mut fetcher).await
}

fn validate_album_url_lookup(raw: &Value, expected_resource: &str) -> Result<(), String> {
    if !raw.is_object() || raw["resource"].as_str() != Some(expected_resource) {
        return Err("MusicBrainz album URL lookup returned a mismatched resource".to_owned());
    }
    let relations = raw["relations"]
        .as_array()
        .ok_or_else(|| "MusicBrainz album URL lookup omitted relations".to_owned())?;
    if relations
        .iter()
        .any(|relation| !relation.is_object() || relation["target-type"].as_str().is_none())
    {
        return Err("MusicBrainz album URL lookup contained malformed relations".to_owned());
    }
    for relation in relations
        .iter()
        .filter(|relation| relation["target-type"].as_str() == Some("release"))
    {
        if relation["release"]["id"]
            .as_str()
            .is_none_or(|id| !artist_credit_aliases::valid_musicbrainz_recording_id(id))
        {
            return Err(
                "MusicBrainz album URL lookup contained a malformed release relation".to_owned(),
            );
        }
    }
    Ok(())
}

async fn musicbrainz_release_search_with<F, Fut>(
    store: &Store,
    request: &SpotifyRequest,
    fetcher: &mut F,
) -> Result<(Vec<String>, &'static str), String>
where
    F: FnMut(String, LocatorQuery) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    let title = quote_musicbrainz_term(&request.album_title);
    let first_credit = request
        .credits
        .first()
        .ok_or_else(|| "Spotify request has no artist credits".to_owned())?;
    let artist = quote_musicbrainz_term(&first_credit.name);
    let query = format!("release:\"{title}\" AND artist:\"{artist}\"");
    let raw = fetcher(
        "https://musicbrainz.org/ws/2/release/".to_owned(),
        vec![
            ("query", query),
            ("limit", MAX_RELEASE_LOCATORS.to_string()),
            ("fmt", "json".into()),
        ],
    )
    .await?;
    store.set_retained_provider_cache(RELEASE_SEARCH_CACHE, &request.request_key, &raw)?;
    store.set_provider_cache(
        MB_FETCHED_AT_CACHE,
        &format!("{RELEASE_SEARCH_CACHE}:{}", request.request_key),
        &serde_json::json!({"fetchedAt":chrono::Utc::now().timestamp()}),
        DISCOVERY_TTL_SECONDS,
    )?;
    let releases = raw["releases"]
        .as_array()
        .ok_or_else(|| "MusicBrainz release search was incomplete".to_owned())?;
    let count = raw["count"]
        .as_u64()
        .ok_or_else(|| "MusicBrainz release search count was incomplete".to_owned())?;
    if count > MAX_RELEASE_LOCATORS as u64 || count != releases.len() as u64 {
        return Ok((Vec::new(), "release_search"));
    }
    Ok((
        releases
            .iter()
            .filter_map(|release| release["id"].as_str())
            .filter(|id| artist_credit_aliases::valid_musicbrainz_recording_id(id))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        "release_search",
    ))
}

async fn exact_musicbrainz_release(store: &Store, release_id: &str) -> Result<Value, String> {
    if !artist_credit_aliases::valid_musicbrainz_recording_id(release_id) {
        return Err("Invalid MusicBrainz release ID".to_owned());
    }
    if let Some(raw) = fresh_mb_cache(store, MB_RELEASE_CACHE, release_id) {
        if raw["id"].as_str() == Some(release_id) {
            return Ok(raw);
        }
    }
    let raw = crate::artist_alias_discovery::fetch(
        &format!("https://musicbrainz.org/ws/2/release/{release_id}"),
        &[
            ("fmt", "json".into()),
            ("inc", "url-rels+recordings".into()),
        ],
    )
    .await?;
    if raw["id"].as_str() != Some(release_id) {
        return Err("MusicBrainz returned a different exact release ID".to_owned());
    }
    store.set_retained_provider_cache(MB_RELEASE_CACHE, release_id, &raw)?;
    store.set_provider_cache(
        MB_FETCHED_AT_CACHE,
        &format!("{MB_RELEASE_CACHE}:{release_id}"),
        &serde_json::json!({"fetchedAt":chrono::Utc::now().timestamp()}),
        DISCOVERY_TTL_SECONDS,
    )?;
    Ok(raw)
}

fn cached_locator_contains(
    store: &Store,
    request: &SpotifyRequest,
    locator_kind: &str,
    release_id: &str,
) -> bool {
    match locator_kind {
        "spotify_album_url" => {
            let album_id = &request.spotify_album_id;
            if !provider_cache_fresh(
                store,
                MB_FETCHED_AT_CACHE,
                &format!("{ALBUM_URL_CACHE}:{album_id}"),
            ) {
                return false;
            }
            let expected = format!("https://open.spotify.com/album/{album_id}");
            store
                .get_provider_cache(ALBUM_URL_CACHE, album_id)
                .ok()
                .flatten()
                .is_some_and(|raw| {
                    raw["resource"].as_str() == Some(expected.as_str())
                        && raw["relations"].as_array().is_some_and(|relations| {
                            relations.iter().any(|relation| {
                                relation["target-type"].as_str() == Some("release")
                                    && relation["release"]["id"].as_str() == Some(release_id)
                            })
                        })
                })
        }
        "release_search" => {
            if !provider_cache_fresh(
                store,
                MB_FETCHED_AT_CACHE,
                &format!("{RELEASE_SEARCH_CACHE}:{}", request.request_key),
            ) {
                return false;
            }
            let Some(raw) = store
                .get_provider_cache(RELEASE_SEARCH_CACHE, &request.request_key)
                .ok()
                .flatten()
            else {
                return false;
            };
            let Some(releases) = raw["releases"].as_array() else {
                return false;
            };
            let Some(count) = raw["count"].as_u64() else {
                return false;
            };
            count <= MAX_RELEASE_LOCATORS as u64
                && count == releases.len() as u64
                && releases
                    .iter()
                    .any(|release| release["id"].as_str() == Some(release_id))
        }
        _ => false,
    }
}

fn quote_musicbrainz_term(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '\\' || character == '"' || character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn fresh_mb_cache(store: &Store, namespace: &str, id: &str) -> Option<Value> {
    if !provider_cache_fresh(store, MB_FETCHED_AT_CACHE, &format!("{namespace}:{id}")) {
        return None;
    }
    store.get_provider_cache(namespace, id).ok().flatten()
}

fn fresh_discogs_cache(store: &Store, namespace: &str, id: &str) -> Option<Value> {
    if !provider_cache_fresh(
        store,
        DISCOGS_FETCHED_AT_CACHE,
        &format!("{namespace}:{id}"),
    ) {
        return None;
    }
    store.get_provider_cache(namespace, id).ok().flatten()
}

fn provider_cache_fresh(store: &Store, namespace: &str, key: &str) -> bool {
    store
        .get_provider_cache(namespace, key)
        .ok()
        .flatten()
        .and_then(|value| value["fetchedAt"].as_i64())
        .is_some_and(timestamp_fresh)
}

fn timestamp_fresh(fetched_at: i64) -> bool {
    let now = chrono::Utc::now().timestamp();
    fetched_at <= now && now.saturating_sub(fetched_at) <= DISCOVERY_TTL_SECONDS as i64
}

/// Cold request-scoped discovery. Locator results are never authority: each
/// selected MB release is fetched and must link one exact Discogs release and
/// contain the source-bound track occurrence before Discogs is fetched.
pub(crate) async fn discover_lexical_alias(store: &Store, track: &Track) -> Result<bool, String> {
    let Some(request) = spotify_request(track) else {
        return Ok(false);
    };
    if has_cached_lexical_alias(store, track) {
        return Ok(true);
    }
    let mut mapped_ids = Vec::with_capacity(request.credits.len());
    for credit in &request.credits {
        if cached_discogs_artist_id(store, &credit.id, &credit.name).is_none() {
            if !crate::artist_alias_discovery::discover_spotify_artist(store, &credit.id).await? {
                return Ok(false);
            }
        }
        let Some(id) = cached_discogs_artist_id(store, &credit.id, &credit.name) else {
            return Ok(false);
        };
        mapped_ids.push(id);
    }
    if mapped_ids.iter().collect::<BTreeSet<_>>().len() != mapped_ids.len() {
        return Ok(false);
    }

    let (locator_ids, locator_kind) = musicbrainz_release_locators(store, &request).await?;
    if locator_ids.is_empty() || locator_ids.len() > MAX_RELEASE_LOCATORS {
        return Ok(false);
    }
    let mut selected = Vec::new();
    for release_id in locator_ids {
        let raw = exact_musicbrainz_release(store, &release_id).await?;
        let Some((discogs_id, _, _, _)) = exact_musicbrainz_occurrence(&request, &raw) else {
            continue;
        };
        selected.push((release_id, discogs_id, raw));
    }
    if selected.len() != 1 {
        return Ok(false);
    }
    let (mb_release_id, discogs_release_id, mb_release) = selected
        .pop()
        .ok_or_else(|| "MusicBrainz release selection disappeared".to_owned())?;
    let release_cache = fresh_discogs_cache(
        store,
        artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE,
        &discogs_release_id,
    );
    let raw_discogs_release = match release_cache {
        Some(raw) => raw,
        None => {
            let raw = crate::artist_alias_discovery::fetch(
                &format!("https://api.discogs.com/releases/{discogs_release_id}"),
                &[],
            )
            .await?;
            if raw["id"].as_u64().map(|value| value.to_string()).as_deref()
                != Some(discogs_release_id.as_str())
            {
                return Ok(false);
            }
            store.set_retained_provider_cache(
                artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE,
                &discogs_release_id,
                &raw,
            )?;
            store.set_provider_cache(
                DISCOGS_FETCHED_AT_CACHE,
                &format!(
                    "{}:{discogs_release_id}",
                    artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE
                ),
                &serde_json::json!({"fetchedAt":chrono::Utc::now().timestamp()}),
                DISCOVERY_TTL_SECONDS,
            )?;
            raw
        }
    };
    let aliases_needed =
        needed_alias_profiles(&request, &mb_release, &raw_discogs_release, &mapped_ids)
            .ok_or_else(|| {
                "Discogs release does not match the exact request occurrence".to_owned()
            })?;
    if aliases_needed.len() > MAX_ALIAS_PROFILE_FETCHES {
        return Ok(false);
    }
    let mut alias_profiles = HashMap::new();
    for id in aliases_needed {
        let profile = match fresh_discogs_cache(
            store,
            artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE,
            &id,
        ) {
            Some(profile) => profile,
            None => {
                let profile = crate::artist_alias_discovery::fetch(
                    &format!("https://api.discogs.com/artists/{id}"),
                    &[],
                )
                .await?;
                if profile["id"]
                    .as_u64()
                    .map(|value| value.to_string())
                    .as_deref()
                    != Some(id.as_str())
                {
                    return Ok(false);
                }
                store.set_retained_provider_cache(
                    artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE,
                    &id,
                    &profile,
                )?;
                store.set_provider_cache(
                    DISCOGS_FETCHED_AT_CACHE,
                    &format!(
                        "{}:{id}",
                        artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE
                    ),
                    &serde_json::json!({"fetchedAt":chrono::Utc::now().timestamp()}),
                    DISCOVERY_TTL_SECONDS,
                )?;
                profile
            }
        };
        alias_profiles.insert(id, profile);
    }
    let Some(verified) = verify_release_track(
        &request,
        &mb_release,
        &raw_discogs_release,
        &discogs_release_id,
        &mapped_ids,
        &alias_profiles,
    ) else {
        return Ok(false);
    };
    if verified.names.iter().all(|names| {
        names
            .iter()
            .skip(1)
            .all(|name| normalize(name) == normalize(&names[0]))
    }) {
        return Ok(false);
    }
    store.set_provider_cache(
        LEXICAL_BINDING_CACHE,
        &request.request_key,
        &serde_json::json!({
            "version": 1,
            "requestKey": request.request_key,
            "musicbrainzReleaseId": mb_release_id,
            "discogsReleaseId": discogs_release_id,
            "locatorKind": locator_kind,
        }),
        DISCOVERY_TTL_SECONDS,
    )?;
    Ok(true)
}

/// Parse original provider-structured request credits without requiring an
/// exact track hydration. This authorizes lexical evidence only.
pub(crate) fn spotify_request(track: &Track) -> Option<SpotifyRequest> {
    if track.provider != "spotify"
        || !valid_spotify_id(&track.provider_id)
        || track.id != format!("spotify:{}", track.provider_id)
        || track.source_url != format!("https://open.spotify.com/track/{}", track.provider_id)
        || track.version.is_some()
        || track.playback_range.is_some()
        || manual_identity_override(track)
    {
        return None;
    }
    let album_id = track.provenance.get("albumId")?.as_str()?;
    if !valid_spotify_id(album_id) {
        return None;
    }
    let album_title = bounded_title(track.album.as_deref()?)?;
    let track_title = bounded_title(&track.title)?;
    if track.provenance["positionScope"].as_str() != Some("album") {
        return None;
    }
    let album_position = track.provenance["position"].as_u64()?;
    if album_position == 0 {
        return None;
    }
    let remote_entry_id = track.provenance["remoteEntryId"].as_str()?;
    let expected_entry_prefix = format!("album_{album_id}_");
    let entry_suffix = remote_entry_id.strip_prefix(&expected_entry_prefix)?;
    let entry_index = entry_suffix.parse::<u64>().ok()?;
    if entry_index.checked_add(1)? != album_position || track.provenance["remotePlaylist"] != true {
        return None;
    }
    let duration = track.duration_seconds?;
    if !duration.is_finite() || duration <= 0.0 || duration > 86_400.0 {
        return None;
    }
    let duration_ms = (duration * 1000.0).round() as u64;
    let credits = artist_credit_aliases::verified_spotify_request_credit_refs(track)?;
    if credits.is_empty() || credits.len() > MAX_TRACK_CREDITS {
        return None;
    }
    if !remote_catalog_is_consistent(track, &credits, duration) {
        return None;
    }
    let isrc = normalized_track_isrc(track)?;
    let key_material = serde_json::json!({
        "trackId": track.provider_id,
        "albumId": album_id,
        "album": album_title,
        "title": track_title,
        "position": album_position,
        "durationMs": duration_ms,
        "isrc": isrc,
        "credits": credits,
        "version": track.version,
        "playbackRange": track.playback_range,
        "manualFields": track.provenance.get("manualFields"),
    });
    let key_bytes = serde_json::to_vec(&key_material).ok()?;
    let request_key = format!("v1:{:x}", Sha256::digest(key_bytes));
    Some(SpotifyRequest {
        request_key,
        spotify_track_id: track.provider_id.clone(),
        spotify_album_id: album_id.to_owned(),
        album_title: album_title.to_owned(),
        track_title: track_title.to_owned(),
        album_position,
        duration_ms,
        isrc,
        credits,
    })
}

/// Verify one exact Discogs edition and return lexical choices per requested
/// credit. `mapped_discogs_ids` must already be derived from each exact raw
/// Spotify URL -> MusicBrainz artist -> Discogs URL relation.
pub(crate) fn verify_release_track(
    request: &SpotifyRequest,
    raw_musicbrainz_release: &Value,
    raw_release: &Value,
    expected_discogs_release_id: &str,
    mapped_discogs_ids: &[String],
    alias_profiles: &std::collections::HashMap<String, Value>,
) -> Option<VerifiedLexicalCredits> {
    if mapped_discogs_ids.len() != request.credits.len()
        || mapped_discogs_ids.is_empty()
        || mapped_discogs_ids.len() > MAX_TRACK_CREDITS
        || mapped_discogs_ids.iter().any(|id| !valid_discogs_id(id))
        || mapped_discogs_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != mapped_discogs_ids.len()
    {
        return None;
    }
    let row = exact_discogs_track_occurrence(
        request,
        raw_musicbrainz_release,
        raw_release,
        expected_discogs_release_id,
    )?;
    if request.isrc.as_deref().is_some_and(|request_isrc| {
        row.isrc
            .as_deref()
            .and_then(crate::recording_identity::normalize_isrc)
            .is_some_and(|release_isrc| release_isrc != request_isrc)
    }) {
        return None;
    }
    let raw_credits = row.provenance["artistCredits"].as_array()?;
    let performers = raw_credits
        .iter()
        .filter(|credit| {
            credit["role"]
                .as_str()
                .is_some_and(|role| role.eq_ignore_ascii_case("artist"))
        })
        .collect::<Vec<_>>();
    if performers.len() != request.credits.len() {
        return None;
    }
    let mut names = Vec::with_capacity(performers.len());
    let mut join_phrases = Vec::with_capacity(performers.len());
    for (index, (credit, requested)) in performers.iter().zip(&request.credits).enumerate() {
        let discogs_id = credit["artist"]["id"].as_str()?;
        let credit_name = bounded_name(credit["name"].as_str()?)?;
        if credit["artist"]["provider"].as_str() != Some("discogs")
            || !valid_discogs_id(discogs_id)
            || !valid_spotify_id(&requested.id)
            || requested.name.trim().is_empty()
            || !valid_discogs_id(&mapped_discogs_ids[index])
        {
            return None;
        }
        let mapped_id = mapped_discogs_ids[index].as_str();
        if discogs_id != mapped_id {
            let profile = alias_profiles.get(mapped_id)?;
            if !discogs_profile_alias_id_owns_name(profile, mapped_id, discogs_id, credit_name) {
                return None;
            }
        }
        let mut choices = vec![requested.name.clone()];
        if normalize(&requested.name) != normalize(credit_name) {
            choices.push(credit_name.to_owned());
        }
        names.push(choices);
        let join = credit["joinPhrase"].as_str().unwrap_or_default();
        if join.len() > 64 || join.chars().any(char::is_control) {
            return None;
        }
        join_phrases.push(join.to_owned());
    }
    if join_phrases.len() != names.len() {
        return None;
    }
    Some(VerifiedLexicalCredits {
        release_id: expected_discogs_release_id.to_owned(),
        track_position: row.provenance["trackPosition"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        names,
        join_phrases,
    })
}

fn needed_alias_profiles(
    request: &SpotifyRequest,
    raw_musicbrainz_release: &Value,
    raw_release: &Value,
    mapped_discogs_ids: &[String],
) -> Option<BTreeSet<String>> {
    let release_id = raw_release["id"].as_u64()?.to_string();
    let row =
        exact_discogs_track_occurrence(request, raw_musicbrainz_release, raw_release, &release_id)?;
    let credits = row.provenance["artistCredits"].as_array()?;
    let performers = credits
        .iter()
        .filter(|credit| {
            credit["role"]
                .as_str()
                .is_some_and(|role| role.eq_ignore_ascii_case("artist"))
        })
        .collect::<Vec<_>>();
    if performers.len() != request.credits.len() || mapped_discogs_ids.len() != performers.len() {
        return None;
    }
    let mut needed = BTreeSet::new();
    for (credit, mapped) in performers.iter().zip(mapped_discogs_ids) {
        let credit_id = credit["artist"]["id"].as_str()?;
        if !valid_discogs_id(credit_id) || !valid_discogs_id(mapped) {
            return None;
        }
        if credit_id != mapped {
            needed.insert(mapped.clone());
        }
    }
    Some(needed)
}

fn exact_discogs_track_occurrence(
    request: &SpotifyRequest,
    raw_musicbrainz_release: &Value,
    raw_release: &Value,
    expected_discogs_release_id: &str,
) -> Option<Track> {
    let (discogs_release_id, medium_number, musicbrainz_track_number, musicbrainz_length_ms) =
        exact_musicbrainz_occurrence(request, raw_musicbrainz_release)?;
    if discogs_release_id != expected_discogs_release_id {
        return None;
    }
    let release_id = raw_release["id"].as_u64()?.to_string();
    if release_id != expected_discogs_release_id
        || release_id.is_empty()
        || release_id.len() > MAX_RELEASE_ID_LEN
        || raw_release["title"].as_str().map(normalize) != Some(normalize(&request.album_title))
    {
        return None;
    }
    let source_url = format!("https://www.discogs.com/release/{release_id}");
    let raw_track = raw_discogs_track_at_album_position(raw_release, request.album_position)?;
    if raw_track["artists"]
        .as_array()
        .is_none_or(|artists| artists.is_empty())
    {
        // Release-level artist credits cannot fill a missing track performer.
        return None;
    }
    let rows =
        artists::parse_discogs_release_tracks(raw_release, &release_id, &source_url, false, None);
    let mut positions = std::collections::HashSet::new();
    if rows.iter().any(|row| {
        let medium = row.provenance["mediumPosition"].as_u64();
        let position = row.provenance["trackPosition"].as_str();
        match (medium, position) {
            (Some(medium), Some(position)) => !positions.insert((medium, position.to_owned())),
            _ => true,
        }
    }) {
        return None;
    }
    let mut matching_rows = rows.into_iter().enumerate().filter(|(ordinal, row)| {
        u64::try_from(*ordinal)
            .ok()
            .and_then(|value| value.checked_add(1))
            == Some(request.album_position)
            && normalize(&row.title) == normalize(&request.track_title)
            && row.album.as_deref().map(normalize) == Some(normalize(&request.album_title))
            && row.version.is_none()
            && row.duration_seconds.is_some_and(|seconds| {
                seconds.is_finite()
                    && seconds > 0.0
                    && ((seconds * 1000.0).round() as u64).abs_diff(request.duration_ms)
                        <= MAX_RUNTIME_DELTA_MS
            })
            && row.source_url == source_url
            && row.provenance["releaseId"].as_str() == Some(release_id.as_str())
            && row.provenance["mediumPosition"].as_u64() == Some(medium_number)
            && position_number(row.provenance["trackPosition"].as_str())
                == Some(musicbrainz_track_number)
            && row.duration_seconds.is_some_and(|seconds| {
                ((seconds * 1000.0).round() as u64).abs_diff(musicbrainz_length_ms)
                    <= MAX_RUNTIME_DELTA_MS
            })
    });
    let (_, row) = matching_rows.next()?;
    if matching_rows.next().is_some() {
        return None;
    }
    Some(row)
}

fn raw_discogs_track_at_album_position(release: &Value, position: u64) -> Option<&Value> {
    if position == 0 {
        return None;
    }
    let mut ordinal = 0u64;
    for row in release["tracklist"].as_array()? {
        if row["type_"].as_str() == Some("heading") {
            continue;
        }
        if let Some(children) = row["sub_tracks"]
            .as_array()
            .filter(|children| !children.is_empty())
        {
            for child in children {
                ordinal = ordinal.checked_add(1)?;
                if ordinal == position {
                    return Some(child);
                }
            }
        } else {
            ordinal = ordinal.checked_add(1)?;
            if ordinal == position {
                return Some(row);
            }
        }
    }
    None
}

/// The MB release is the locator-to-edition bridge. Require its exact title,
/// a unique Discogs release URL relation, and one exact track occurrence at
/// the original Spotify album position before reading the Discogs edition.
fn exact_musicbrainz_occurrence(
    request: &SpotifyRequest,
    release: &Value,
) -> Option<(String, u64, u64, u64)> {
    if !crate::artist_credit_aliases::valid_musicbrainz_recording_id(release.get("id")?.as_str()?)
        || release["title"].as_str().map(normalize) != Some(normalize(&request.album_title))
    {
        return None;
    }
    let mut discogs_ids = std::collections::BTreeSet::new();
    for relation in release.get("relations")?.as_array()? {
        if relation["target-type"].as_str() != Some("url") {
            continue;
        }
        let Some(resource) = relation["url"]["resource"].as_str() else {
            return None;
        };
        if let Some(id) = discogs_release_id_from_url(resource) {
            discogs_ids.insert(id);
        }
    }
    if discogs_ids.len() != 1 {
        return None;
    }
    let mut matches = Vec::new();
    let mut album_position = 0u64;
    for (medium_index, media) in release["media"].as_array()?.iter().enumerate() {
        let expected_medium = u64::try_from(medium_index).ok()?.checked_add(1)?;
        let medium_number = media["position"]
            .as_u64()
            .or_else(|| media["position"].as_str()?.parse().ok())?;
        if medium_number != expected_medium {
            return None;
        }
        for track in media["tracks"].as_array()? {
            album_position = album_position.checked_add(1)?;
            if track["title"].as_str().map(normalize) != Some(normalize(&request.track_title)) {
                continue;
            }
            let medium_position = track["position"]
                .as_str()
                .and_then(|value| value.parse().ok())
                .or_else(|| track["position"].as_u64())?;
            if album_position != request.album_position {
                continue;
            }
            let length = track["length"]
                .as_u64()
                .or_else(|| track["length"].as_str()?.parse().ok())?;
            if length.abs_diff(request.duration_ms) > MAX_RUNTIME_DELTA_MS {
                continue;
            }
            matches.push((medium_number, medium_position, length));
        }
    }
    if matches.len() != 1 {
        return None;
    }
    let (medium, position, length) = matches[0];
    Some((discogs_ids.into_iter().next()?, medium, position, length))
}

fn discogs_release_id_from_url(resource: &str) -> Option<String> {
    let url = url::Url::parse(resource).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("discogs.com" | "www.discogs.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut segments = url.path_segments()?;
    if segments.next()? != "release" {
        return None;
    }
    let id_and_slug = segments.next()?;
    let id = id_and_slug.split('-').next()?;
    if !valid_discogs_id(id) || segments.next().is_some() {
        return None;
    }
    Some(id.to_owned())
}

fn position_number(value: Option<&str>) -> Option<u64> {
    value?.parse().ok()
}

fn discogs_profile_alias_id_owns_name(
    profile: &Value,
    profile_id: &str,
    alias_id: &str,
    credited_name: &str,
) -> bool {
    if profile["id"]
        .as_u64()
        .map(|value| value.to_string())
        .as_deref()
        != Some(profile_id)
    {
        return false;
    }
    let edges = profile["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|alias| {
            alias["id"]
                .as_u64()
                .map(|value| value.to_string())
                .as_deref()
                == Some(alias_id)
        })
        .collect::<Vec<_>>();
    edges.len() == 1
        && edges[0]["name"]
            .as_str()
            .is_some_and(|name| normalize(name) == normalize(credited_name))
}

fn normalized_track_isrc(track: &Track) -> Option<Option<String>> {
    let own = match track.isrc.as_deref() {
        Some(value) => Some(crate::recording_identity::normalize_isrc(value)?),
        None => None,
    };
    let Some(remote) = track.provenance.get("remoteCatalog") else {
        return Some(own);
    };
    if !remote.is_object() {
        return None;
    }
    let direct = normalize_optional_isrc(remote.get("isrc"))?;
    let recording_context = remote.get("recordingContext");
    if recording_context.is_some_and(|value| !value.is_null() && !value.is_object()) {
        return None;
    }
    let contextual =
        normalize_optional_isrc(recording_context.and_then(|value| value.get("isrc")))?;
    if direct.is_some() && contextual.is_some() && direct != contextual
        || own.is_some() && direct.is_some() && own != direct
        || own.is_some() && contextual.is_some() && own != contextual
    {
        return None;
    }
    Some(own.or(direct).or(contextual))
}

fn normalize_optional_isrc(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(crate::recording_identity::normalize_isrc(value)?)),
        Some(_) => None,
    }
}

fn remote_catalog_is_consistent(
    track: &Track,
    credits: &[artist_credit_aliases::SpotifyArtistCreditRef],
    duration: f64,
) -> bool {
    let Some(remote) = track.provenance.get("remoteCatalog") else {
        return true;
    };
    if !remote.is_object() {
        return false;
    }
    let remote_id = remote
        .get("trackId")
        .or_else(|| remote.get("id"))
        .and_then(Value::as_str);
    let remote_duration = remote.get("durationSeconds").and_then(Value::as_f64);
    if remote_id != Some(track.provider_id.as_str())
        || remote.get("title").and_then(Value::as_str) != Some(track.title.as_str())
        || remote.get("artist").and_then(Value::as_str) != Some(track.artist.as_str())
        || !remote_duration
            .is_some_and(|value| value.is_finite() && (value - duration).abs() <= 0.001)
    {
        return false;
    }
    if let Some(refs) = remote.get("artistRefs") {
        let Some(refs) = refs.as_array() else {
            return false;
        };
        if refs.len() != credits.len()
            || !refs.iter().zip(credits).all(|(reference, credit)| {
                reference["provider"].as_str() == Some("spotify")
                    && reference["id"].as_str() == Some(credit.id.as_str())
                    && reference["name"].as_str().map(str::trim) == Some(credit.name.trim())
            })
        {
            return false;
        }
    }
    true
}

fn manual_identity_override(track: &Track) -> bool {
    [
        "artist",
        "artistCredits",
        "title",
        "album",
        "albumArtist",
        "version",
        "durationSeconds",
        "isrc",
    ]
    .iter()
    .any(|field| track.provenance["manualFields"][field] == true)
}

fn bounded_title(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control))
        .then_some(value)
}

fn bounded_name(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= MAX_NAME_LEN && !value.chars().any(char::is_control))
        .then_some(value)
}

fn normalize(value: &str) -> String {
    crate::matching::normalize(value)
}

fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn valid_discogs_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RELEASE_ID_LEN
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value != "0"
}

#[cfg(test)]
#[path = "discogs_release_lexical_aliases_tests.rs"]
mod tests;
