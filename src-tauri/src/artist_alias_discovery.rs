//! Demand-driven raw evidence acquisition. All playback identity gates remain in
//! artist_credit_aliases; search results are locators, never identity proof.
use crate::{artist_credit_aliases as aliases, models::Track, Store};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Digest;
use std::{collections::BTreeSet, sync::OnceLock, time::Duration};
const TTL: u64 = 30 * 24 * 60 * 60;
#[derive(Debug, Clone, Default)]
pub(crate) struct DiscoveryStats {
    pub requests: u32,
    pub bytes: u64,
    pub queue_ms: u64,
    pub http_ms: u64,
    pub last_status: Option<u16>,
}
#[derive(Debug)]
pub(crate) struct DiscoveryReport {
    pub stats: DiscoveryStats,
    pub result: Result<(), String>,
}
static COOLDOWN: tokio::sync::Mutex<Option<tokio::time::Instant>> =
    tokio::sync::Mutex::const_new(None);
tokio::task_local! { static STATS: std::cell::RefCell<DiscoveryStats>; }
tokio::task_local! { static TRACE: crate::playback_sources::SourceTrace; }
struct RequestTrace {
    trace: Option<crate::playback_sources::SourceTrace>,
    started: std::time::Instant,
    authority: &'static str,
    status: Option<u16>,
    bytes: u64,
    outcome: &'static str,
}
impl Drop for RequestTrace {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().as_millis() as u64;
        let _ = STATS.try_with(|stats| stats.borrow_mut().http_ms += elapsed);
        if let Some(trace) = &self.trace {
            trace.event("alias_metadata_request_end",self.authority,json!({"durationMs":elapsed,"status":self.status,"bytes":self.bytes,"authority":self.authority,"outcome":self.outcome}));
        }
    }
}

const EVIDENCE: &str = "artists.alias-discovery.wikidata-evidence.v1";
const SCOPED_NAME_SEARCH: &str = "artists.alias-discovery.scoped-name-search.v2";

fn scoped_name_profile_query(name: &str) -> String {
    let phrase = name
        .chars()
        .map(|character| {
            if character == '\\' || character == '"' || character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    format!("(artist:\"{phrase}\" OR alias:\"{phrase}\")")
}

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let value = Client::builder()
        .user_agent("Burrata/0.1.3 (artist identity discovery)")
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let _ = CLIENT.set(value);
    Ok(CLIENT.get().expect("client initialized"))
}
async fn fetch(url: &str, query: &[(&str, String)]) -> Result<Value, String> {
    if COOLDOWN
        .lock()
        .await
        .is_some_and(|until| until > tokio::time::Instant::now())
    {
        return Err("Artist discovery rate limited".into());
    }
    let queued_at = std::time::Instant::now();
    if url.starts_with("https://musicbrainz.org/") {
        static GATE: tokio::sync::Mutex<Option<tokio::time::Instant>> =
            tokio::sync::Mutex::const_new(None);
        let mut last = GATE.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep_until(at + Duration::from_millis(1100)).await;
        }
        *last = Some(tokio::time::Instant::now());
    }
    let _ = STATS
        .try_with(|stats| stats.borrow_mut().queue_ms += queued_at.elapsed().as_millis() as u64);
    if COOLDOWN
        .lock()
        .await
        .is_some_and(|until| until > tokio::time::Instant::now())
    {
        return Err("Artist discovery rate limited".into());
    }
    let allowed = STATS
        .try_with(|stats| {
            let mut stats = stats.borrow_mut();
            if stats.requests >= 24 {
                return false;
            }
            stats.requests += 1;
            true
        })
        .unwrap_or(true);
    if !allowed {
        return Err("Artist discovery request budget exhausted".into());
    }
    let mut request_trace = RequestTrace {
        trace: TRACE.try_with(Clone::clone).ok(),
        started: std::time::Instant::now(),
        authority: if url.starts_with("https://musicbrainz.org/") {
            "musicbrainz"
        } else if url.starts_with("https://api.discogs.com/") {
            "discogs"
        } else {
            "wikidata"
        },
        status: None,
        bytes: 0,
        outcome: "aborted",
    };
    crate::diagnostic_metrics::record_http_send_attempt();
    let response = client()?
        .get(url)
        .query(query)
        .send()
        .await
        .map_err(|_| "Artist discovery transport failed".to_string())?;
    request_trace.status = Some(response.status().as_u16());
    let _ = STATS.try_with(|stats| stats.borrow_mut().last_status = request_trace.status);
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.parse::<u64>().ok().or_else(|| {
                    chrono::DateTime::parse_from_rfc2822(v).ok().map(|date| {
                        date.timestamp()
                            .saturating_sub(chrono::Utc::now().timestamp())
                            .max(1) as u64
                    })
                })
            })
            .unwrap_or(60)
            .clamp(1, 3600);
        *COOLDOWN.lock().await = Some(tokio::time::Instant::now() + Duration::from_secs(seconds));
        request_trace.outcome = "rate_limited";
        return Err("Artist discovery rate limited".into());
    }
    request_trace.outcome = "http_error";
    let status = response.status().as_u16();
    let response = response
        .error_for_status()
        .map_err(|_| format!("Artist discovery HTTP {status}"))?;
    request_trace.outcome = "aborted";
    // Bound response memory even for unexpectedly large public metadata.
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        request_trace.bytes += chunk.len() as u64;
        let _ = STATS.try_with(|stats| stats.borrow_mut().bytes += chunk.len() as u64);
        if bytes.len() + chunk.len() > 2_000_000 {
            request_trace.outcome = "response_limit";
            return Err("Artist evidence response too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let result = serde_json::from_slice(&bytes).map_err(|e| e.to_string());
    request_trace.outcome = if result.is_ok() {
        "complete"
    } else {
        "invalid_json"
    };
    result
}
async fn mb(
    store: &Store,
    namespace: &str,
    id: &str,
    kind: &str,
    inc: &str,
) -> Result<Value, String> {
    if let Some(raw) = store.get_provider_cache(namespace, id)? {
        let freshness = store.get_provider_cache(
            "artists.alias-discovery.fetched-at.v1",
            &format!("{namespace}:{id}"),
        )?;
        if freshness
            .as_ref()
            .and_then(|value| value["fetchedAt"].as_i64())
            .is_some_and(|at| chrono::Utc::now().timestamp().saturating_sub(at) < TTL as i64)
        {
            return Ok(raw);
        }
    }
    let raw = fetch(
        &format!("https://musicbrainz.org/ws/2/{kind}/{id}"),
        &[("fmt", "json".into()), ("inc", inc.into())],
    )
    .await?;
    if raw["id"].as_str() != Some(id) {
        return Err("Artist discovery returned a different exact ID".into());
    }
    store.set_retained_provider_cache(namespace, id, &raw)?;
    store.set_retained_provider_cache(
        "artists.alias-discovery.fetched-at.v1",
        &format!("{namespace}:{id}"),
        &json!({"fetchedAt":chrono::Utc::now().timestamp()}),
    )?;
    Ok(raw)
}

/// The caller owns the overall playback deadline and cancellation. Dropping this
/// future cancels its in-flight HTTP; no library metadata or provider is edited.
pub(crate) async fn discover_verified_artist_aliases_traced(
    store: &Store,
    track: &Track,
    trace: crate::playback_sources::SourceTrace,
) -> DiscoveryReport {
    TRACE
        .scope(trace, discover_verified_artist_aliases(store, track))
        .await
}
pub(crate) async fn discover_verified_artist_aliases(
    store: &Store,
    track: &Track,
) -> DiscoveryReport {
    STATS
        .scope(std::cell::RefCell::new(DiscoveryStats::default()), async {
            let result = discover_inner(store, track).await;
            let stats = STATS.with(|stats| stats.borrow().clone());
            DiscoveryReport { stats, result }
        })
        .await
}
async fn discover_inner(store: &Store, track: &Track) -> Result<(), String> {
    if matches!(
        track.provider.as_str(),
        "beatport" | "traxsource" | "local" | "radio_song"
    ) {
        return discover_generic_isrc(store, track).await;
    }

    if track.provider == "catalog" && track.provenance["catalogProvider"] == "discogs" {
        let id = track.provenance["releaseId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 20 && id.bytes().all(|c| c.is_ascii_digit()))
            .ok_or("Invalid Discogs release identity")?;
        // Existing browser fetches are reused. Only after an ordinary playback
        // identity failure may the missing exact public evidence be fetched.
        if aliases::verified_discogs_credit_refs(store, track).is_none() {
            let raw = fetch(&format!("https://api.discogs.com/releases/{id}"), &[]).await?;
            if raw["id"].as_u64().map(|value| value.to_string()).as_deref() != Some(id) {
                return Err("Discogs returned a different release ID".into());
            }
            store.set_retained_provider_cache(aliases::DISCOGS_ALIAS_RELEASE_CACHE, id, &raw)?;
        }
        let credits = aliases::verified_discogs_credit_refs(store, track)
            .ok_or("Saved Discogs occurrence does not match public release evidence")?;
        for (id, name) in credits {
            if store
                .get_provider_cache(aliases::DISCOGS_ALIAS_PROFILE_CACHE, &id)?
                .is_some()
            {
                continue;
            }
            let raw = fetch(&format!("https://api.discogs.com/artists/{id}"), &[]).await?;
            if raw["id"].as_u64().map(|value| value.to_string()).as_deref() != Some(&id)
                || raw["name"].as_str() != Some(&name)
            {
                return Err("Discogs returned a different artist identity".into());
            }
            store.set_retained_provider_cache(aliases::DISCOGS_ALIAS_PROFILE_CACHE, &id, &raw)?;
        }
        return Ok(());
    }
    if track.provider == "catalog" && track.provenance["catalogProvider"] == "musicbrainz" {
        if let Some(id) = track.provenance["releaseId"]
            .as_str()
            .filter(|id| aliases::valid_musicbrainz_recording_id(id))
        {
            let raw = mb(
                store,
                "artists.release-tracks.musicbrainz",
                id,
                "release",
                "recordings+artist-credits+isrcs",
            )
            .await?;
            let mut ids = BTreeSet::new();
            collect_artist_ids(&raw, &mut ids);
            for id in ids.into_iter().take(16) {
                hydrate_artist(store, &id, "artists.profile.musicbrainz").await?;
            }
        }
        discover_musicbrainz_occurrence_runtime_variants(store, track).await?;
        return Ok(());
    }
    let Some(fingerprint) = aliases::spotify_artist_credit_fingerprint(track) else {
        return Ok(());
    };
    let freshness_key = serde_json::to_string(&fingerprint)
        .map_err(|_| "Artist fingerprint serialization failed")?;
    if store
        .get_provider_cache("artists.alias-discovery.verified-fresh.v1", &freshness_key)?
        .is_some()
        && !aliases::cached_verified_playback_artist_alias_tracks(store, track).is_empty()
    {
        return Ok(());
    }
    // Name equivalence is independent of a Spotify artist URL crosswalk. Fetch
    // it first and best-effort: a transient URL lookup failure must not prevent
    // scoped lexical evidence from being retained for candidate matching.
    let _ = discover_scoped_name_profile(store, track).await;
    let mut mapped = true;
    for artist in &fingerprint.artists {
        match discover_spotify_artist(store, &artist.id).await {
            Ok(true) => {}
            Ok(false) | Err(_) => mapped = false,
        }
    }
    if mapped {
        return Ok(());
    }
    // Search text only locates candidates; the existing verifier requires the
    // exact ordered Spotify artist IDs in raw MusicBrainz URL relations.
    let query = if let Some(isrc) = fingerprint.isrc.as_deref() {
        format!("isrc:{isrc}")
    } else {
        let title = fingerprint.title.replace(['\\', '"'], " ");
        let artist = fingerprint
            .artists
            .first()
            .map(|a| a.name.replace(['\\', '"'], " "))
            .unwrap_or_default();
        format!("recording:\"{title}\" AND artist:\"{artist}\"")
    };
    let search = fetch(
        "https://musicbrainz.org/ws/2/recording/",
        &[
            ("fmt", "json".into()),
            ("query", query),
            ("limit", "5".into()),
        ],
    )
    .await?;
    let hits = search["recordings"]
        .as_array()
        .ok_or("Incomplete recording search response")?;
    if search["count"]
        .as_u64()
        .map_or(true, |count| count > 5 || count != hits.len() as u64)
    {
        return Ok(());
    }
    let mut verified = BTreeSet::new();
    for hit in search["recordings"]
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
    {
        let Some(id) = hit["id"]
            .as_str()
            .filter(|id| aliases::valid_musicbrainz_recording_id(id))
        else {
            continue;
        };
        let raw = mb(
            store,
            aliases::spotify_recording_credit_raw_cache_namespace(),
            id,
            "recording",
            "artist-credits+isrcs",
        )
        .await?;
        let mut ids = BTreeSet::new();
        collect_artist_ids(&raw, &mut ids);
        for artist in ids.into_iter().take(16) {
            hydrate_artist(
                store,
                &artist,
                aliases::spotify_recording_credit_profile_cache_namespace(),
            )
            .await?;
        }
        if aliases::verify_spotify_artist_credit_recording(store, track, id).is_some()
            || isrc_authority_variants(store, track, id, Some(&search)).is_some()
        {
            verified.insert(id.to_owned());
        }
    }
    if verified.len() > 1 {
        aliases::invalidate_spotify_artist_credit_recording(store, &track.provider_id)?;
        store.set_retained_provider_cache(
            ISRC_BINDING,
            &track.provider_id,
            &json!({"version":1,"ambiguous":true,"search":search}),
        )?;
    }
    if hits.len() <= 5 && verified.len() == 1 {
        let recording_id = verified.iter().next().expect("one verified recording");
        if aliases::verify_spotify_artist_credit_recording(store, track, recording_id).is_some() {
            aliases::write_verified_spotify_artist_credit_recording(store, track, recording_id)?;
        } else {
            store.set_retained_provider_cache(ISRC_BINDING,&track.provider_id,&json!({"version":1,"fingerprint":fingerprint,"recordingId":recording_id,"search":search,"fetchedAt":chrono::Utc::now().timestamp()}))?;
        }
        store.set_provider_cache(
            "artists.alias-discovery.verified-fresh.v1",
            &freshness_key,
            &json!({"verified":true}),
            TTL,
        )?;
    }
    Ok(())
}
fn collect_artist_ids(raw: &Value, ids: &mut BTreeSet<String>) {
    match raw {
        Value::Object(map) => {
            if let Some(credits) = map.get("artist-credit").and_then(Value::as_array) {
                for credit in credits {
                    if let Some(id) = credit["artist"]["id"]
                        .as_str()
                        .filter(|id| aliases::valid_musicbrainz_recording_id(id))
                    {
                        ids.insert(id.into());
                    }
                }
            }
            for value in map.values() {
                collect_artist_ids(value, ids);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_artist_ids(value, ids);
            }
        }
        _ => {}
    }
}
async fn hydrate_artist(store: &Store, id: &str, namespace: &str) -> Result<(), String> {
    let profile = mb(store, namespace, id, "artist", "aliases+url-rels").await?;
    if store
        .get_provider_cache(EVIDENCE, id)?
        .as_ref()
        .and_then(|value| value["fetchedAt"].as_i64())
        .is_some_and(|at| chrono::Utc::now().timestamp().saturating_sub(at) < TTL as i64)
    {
        return Ok(());
    }
    let mut locator = Value::Null;
    let qid = if let Some(qid) = wikidata_id(&profile) {
        qid
    } else {
        let ids = external_ids(&profile, "www.discogs.com", "/artist/")
            .into_iter()
            .chain(external_ids(&profile, "discogs.com", "/artist/"))
            .collect::<BTreeSet<_>>();
        let Some(discogs) = ids
            .iter()
            .find(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        else {
            return Ok(());
        };
        let query = format!("SELECT ?item WHERE {{ ?item wdt:P1953 \"{discogs}\" }} LIMIT 2");
        let results = fetch(
            "https://query.wikidata.org/sparql",
            &[("query", query), ("format", "json".into())],
        )
        .await?;
        locator = results.clone();
        let Some(bindings) = results["results"]["bindings"]
            .as_array()
            .filter(|v| v.len() == 1)
        else {
            return Ok(());
        };
        let Some(uri) = bindings[0]["item"]["value"].as_str() else {
            return Ok(());
        };
        let Some(qid) = uri
            .strip_prefix("http://www.wikidata.org/entity/")
            .filter(|id| {
                id.len() > 1 && id.starts_with('Q') && id[1..].bytes().all(|b| b.is_ascii_digit())
            })
        else {
            return Ok(());
        };
        qid.to_owned()
    };
    let raw = fetch(
        &format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"),
        &[],
    )
    .await?;
    let entity = &raw["entities"][&qid];
    if corroborated(&profile, entity, id) {
        store.set_retained_provider_cache(
            EVIDENCE,
            id,
            &json!({"version":1,"artistId":id,"profile":profile,"entity":entity,"locator":locator,"fetchedAt":chrono::Utc::now().timestamp()}),
        )?;
    }
    Ok(())
}

/// Cache a unique exact-name MusicBrainz artist profile for later candidate-
/// scoped lexical alias checks. This is name evidence only: it never creates a
/// Spotify artist crosswalk or a recording credit-set binding.
async fn discover_scoped_name_profile(store: &Store, track: &Track) -> Result<(), String> {
    let name = track.artist.trim();
    if name.is_empty()
        || name.len() > 256
        || name.chars().any(char::is_control)
        || name.contains(',')
        || name.contains(" & ")
        || name.contains(" and ")
    {
        return Ok(());
    }
    let key = format!(
        "v1:{:x}",
        sha2::Sha256::digest(crate::matching::normalize_single_artist_name(name))
    );
    if let Some(cached) = store.get_provider_cache(SCOPED_NAME_SEARCH, &key)? {
        match cached["outcome"].as_str() {
            Some("ambiguous") => return Ok(()),
            Some("unique_exact_name_profile") => {
                let valid = cached["artistId"].as_str().is_some_and(|id| {
                    store
                        .get_provider_cache("artists.profile.musicbrainz", id)
                        .ok()
                        .flatten()
                        .is_some_and(|profile| {
                            profile["id"].as_str() == Some(id) && profile_owns_name(&profile, name)
                        })
                });
                if valid {
                    return Ok(());
                }
            }
            Some("no_exact_name_owner") => {
                if let Some(id) = cached_name_profile_owner(store, name)? {
                    store.set_provider_cache(
                        SCOPED_NAME_SEARCH,
                        &key,
                        &json!({"outcome":"unique_exact_name_profile","artistId":id}),
                        7 * 24 * 60 * 60,
                    )?;
                }
                // A persisted no-hit can be repaired from a raw profile that
                // was already fetched through another verified artist route.
                // If there is no such profile, retain the negative cache.
                return Ok(());
            }
            _ => {}
        }
    } else if let Some(id) = cached_name_profile_owner(store, name)? {
        store.set_provider_cache(
            SCOPED_NAME_SEARCH,
            &key,
            &json!({"outcome":"unique_exact_name_profile","artistId":id}),
            7 * 24 * 60 * 60,
        )?;
        return Ok(());
    }
    let search = fetch(
        "https://musicbrainz.org/ws/2/artist/",
        &[
            ("query", scoped_name_profile_query(name)),
            ("fmt", "json".into()),
            ("limit", "5".into()),
        ],
    )
    .await?;
    let hits = search["artists"]
        .as_array()
        .ok_or("Incomplete artist name search")?;
    let count = search["count"]
        .as_u64()
        .ok_or("Incomplete artist name search")?;
    let result = if count > hits.len() as u64 || hits.len() > 5 {
        json!({"outcome":"ambiguous"})
    } else {
        let mut profiles = Vec::new();
        for hit in hits {
            let Some(id) = hit["id"]
                .as_str()
                .filter(|id| aliases::valid_musicbrainz_recording_id(id))
            else {
                continue;
            };
            let profile = mb(
                store,
                "artists.profile.musicbrainz",
                id,
                "artist",
                "aliases+url-rels",
            )
            .await?;
            profiles.push((id.to_owned(), profile));
        }
        match unique_exact_name_owner(&profiles, name).as_deref() {
            Some(id) => json!({"outcome":"unique_exact_name_profile","artistId":id}),
            None if profiles
                .iter()
                .any(|(_, profile)| profile_owns_name(profile, name)) =>
            {
                json!({"outcome":"ambiguous"})
            }
            None => json!({"outcome":"no_exact_name_owner"}),
        }
    };
    store.set_provider_cache(SCOPED_NAME_SEARCH, &key, &result, 7 * 24 * 60 * 60)?;
    Ok(())
}

/// Names from a cached, uniquely-owned MusicBrainz profile that explicitly
/// owns the request spelling. This is lexical metadata only.
pub(crate) fn cached_scoped_name_aliases(store: &Store, track: &Track) -> Vec<String> {
    let name = track.artist.trim();
    if name.is_empty()
        || name.len() > 256
        || name.contains(',')
        || name.contains(" & ")
        || name.contains(" and ")
        || name.chars().any(char::is_control)
    {
        return Vec::new();
    }
    let key = format!(
        "v1:{:x}",
        sha2::Sha256::digest(crate::matching::normalize_single_artist_name(name))
    );
    let result = store
        .get_provider_cache(SCOPED_NAME_SEARCH, &key)
        .ok()
        .flatten();
    let id = match result.as_ref().and_then(|value| value["outcome"].as_str()) {
        Some("ambiguous") => return Vec::new(),
        Some("unique_exact_name_profile") => result
            .as_ref()
            .and_then(|value| value["artistId"].as_str())
            .map(str::to_owned),
        // A raw profile may have arrived after an old no-hit or without a
        // scoped search marker. Re-evaluate only exact profile ownership from
        // cache; this remains lexical evidence and never asserts a Spotify or
        // recording-credit crosswalk.
        Some("no_exact_name_owner") | None => cached_name_profile_owner(store, name).ok().flatten(),
        _ => None,
    };
    let Some(id) = id else {
        return Vec::new();
    };
    let Ok(Some(profile)) = store.get_provider_cache("artists.profile.musicbrainz", &id) else {
        return Vec::new();
    };
    if profile["id"].as_str() != Some(id.as_str()) || !profile_owns_name(&profile, name) {
        return Vec::new();
    }
    let mut names = Vec::new();
    if let Some(canonical) = profile["name"].as_str() {
        names.push(canonical.to_owned());
    }
    names.extend(
        profile["aliases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|alias| {
                !alias["type"]
                    .as_str()
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("Search hint"))
            })
            .filter_map(|alias| alias["name"].as_str())
            .map(str::to_owned),
    );
    names.sort_by_key(|alias| crate::matching::normalize_single_artist_name(alias));
    names.dedup_by(|left, right| {
        crate::matching::normalize_single_artist_name(left)
            == crate::matching::normalize_single_artist_name(right)
    });
    names.retain(|alias| {
        !alias.trim().is_empty()
            && alias.len() <= 256
            && crate::matching::normalize_single_artist_name(alias)
                != crate::matching::normalize_single_artist_name(name)
    });
    names
}

fn profile_owns_name(profile: &Value, name: &str) -> bool {
    let requested = crate::matching::normalize_single_artist_name(name);
    profile["name"]
        .as_str()
        .into_iter()
        .chain(
            profile["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|alias| {
                    !alias["type"]
                        .as_str()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("Search hint"))
                })
                .filter_map(|alias| alias["name"].as_str()),
        )
        .any(|candidate| crate::matching::normalize_single_artist_name(candidate) == requested)
}

fn cached_name_profile_owner(store: &Store, name: &str) -> Result<Option<String>, String> {
    let profiles = store.cached_musicbrainz_artist_profiles_for_name(name)?;
    let profiles = profiles
        .into_iter()
        .filter(|profile| {
            profile["id"]
                .as_str()
                .is_some_and(aliases::valid_musicbrainz_recording_id)
                && profile_owns_name(profile, name)
        })
        .map(|profile| (profile["id"].as_str().unwrap().to_owned(), profile))
        .collect::<Vec<_>>();
    Ok(unique_exact_name_owner(&profiles, name))
}

fn unique_exact_name_owner(profiles: &[(String, Value)], name: &str) -> Option<String> {
    let owners = profiles
        .iter()
        .filter(|(_, profile)| profile_owns_name(profile, name))
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    (owners.len() == 1)
        .then(|| owners.into_iter().next())
        .flatten()
}

#[cfg(test)]
mod scoped_name_profile_tests {
    use super::*;
    use tempfile::tempdir;

    fn scoped_name_cache_key(name: &str) -> String {
        format!(
            "v1:{:x}",
            sha2::Sha256::digest(crate::matching::normalize_single_artist_name(name))
        )
    }

    #[tokio::test]
    async fn cached_exact_alias_repairs_a_previous_search_no_hit() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("scoped-alias.sqlite")).unwrap();
        let name = "Lakis Chalkias";
        let id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        let key = scoped_name_cache_key(name);
        store
            .set_provider_cache(
                SCOPED_NAME_SEARCH,
                &key,
                &json!({"outcome":"no_exact_name_owner"}),
                3600,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                id,
                &json!({"id":id,"name":"Lakis Halkias","aliases":[{"name":"Lakis Chalkias"}]}),
            )
            .unwrap();
        let track = Track {
            provider: "spotify".into(),
            artist: name.into(),
            ..Track::default()
        };

        discover_scoped_name_profile(&store, &track).await.unwrap();

        assert_eq!(
            store
                .get_provider_cache(SCOPED_NAME_SEARCH, &key)
                .unwrap()
                .unwrap()["outcome"],
            "unique_exact_name_profile"
        );
        assert_eq!(
            cached_scoped_name_aliases(&store, &track),
            vec!["Lakis Halkias"]
        );
    }

    #[tokio::test]
    async fn cached_homonymous_name_profiles_do_not_expand_aliases() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("scoped-homonyms.sqlite")).unwrap();
        let name = "Lakis Chalkias";
        let key = scoped_name_cache_key(name);
        store
            .set_provider_cache(
                SCOPED_NAME_SEARCH,
                &key,
                &json!({"outcome":"no_exact_name_owner"}),
                3600,
            )
            .unwrap();
        for (id, canonical) in [
            ("612d1573-bdea-4657-bfc9-f78593c9ab09", "Lakis Halkias"),
            ("712d1573-bdea-4657-bfc9-f78593c9ab09", "Lakis Chalkias II"),
        ] {
            store
                .set_retained_provider_cache(
                    "artists.profile.musicbrainz",
                    id,
                    &json!({"id":id,"name":canonical,"aliases":[{"name":name}]}),
                )
                .unwrap();
        }
        let track = Track {
            provider: "spotify".into(),
            artist: name.into(),
            ..Track::default()
        };

        discover_scoped_name_profile(&store, &track).await.unwrap();

        assert!(cached_scoped_name_aliases(&store, &track).is_empty());
        assert_eq!(
            store
                .get_provider_cache(SCOPED_NAME_SEARCH, &key)
                .unwrap()
                .unwrap()["outcome"],
            "no_exact_name_owner"
        );
    }

    #[test]
    fn cache_only_alias_lookup_uses_newly_cached_owned_profile_after_old_negative() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("scoped-late-profile.sqlite")).unwrap();
        let name = "Lakis Chalkias";
        let id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        store
            .set_provider_cache(
                "artists.alias-discovery.negative.v5",
                "previous-complete-identity",
                &json!({"outcome":"no_verified_evidence","identity":"complete"}),
                3600,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                id,
                &json!({"id":id,"name":"Lakis Halkias","aliases":[{"name":name}]}),
            )
            .unwrap();
        let track = Track {
            provider: "spotify".into(),
            artist: name.into(),
            ..Track::default()
        };

        assert_eq!(
            cached_scoped_name_aliases(&store, &track),
            vec!["Lakis Halkias"]
        );
    }

    #[test]
    fn cache_only_alias_lookup_respects_ambiguous_profile_owners_and_searches() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("scoped-late-homonyms.sqlite")).unwrap();
        let name = "Lakis Chalkias";
        let key = scoped_name_cache_key(name);
        for (id, canonical) in [
            ("612d1573-bdea-4657-bfc9-f78593c9ab09", "Lakis Halkias"),
            ("712d1573-bdea-4657-bfc9-f78593c9ab09", "Lakis Chalkias II"),
        ] {
            store
                .set_retained_provider_cache(
                    "artists.profile.musicbrainz",
                    id,
                    &json!({"id":id,"name":canonical,"aliases":[{"name":name}]}),
                )
                .unwrap();
        }
        let track = Track {
            provider: "spotify".into(),
            artist: name.into(),
            ..Track::default()
        };
        assert!(cached_scoped_name_aliases(&store, &track).is_empty());

        store
            .set_provider_cache(
                SCOPED_NAME_SEARCH,
                &key,
                &json!({"outcome":"ambiguous"}),
                3600,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                "812d1573-bdea-4657-bfc9-f78593c9ab09",
                &json!({"id":"812d1573-bdea-4657-bfc9-f78593c9ab09","name":"Lakis Chalkias","aliases":[]}),
            )
            .unwrap();
        assert!(cached_scoped_name_aliases(&store, &track).is_empty());
    }

    #[test]
    fn artist_alias_search_includes_name_and_alias_fields_with_safe_phrase() {
        let query = scoped_name_profile_query("Lakis Chalkias\\\"\nII");
        assert_eq!(
            query,
            "(artist:\"Lakis Chalkias   II\" OR alias:\"Lakis Chalkias   II\")"
        );
    }

    #[test]
    fn cold_alias_search_hit_hydrates_only_the_profile_that_owns_the_spelling() {
        let query = scoped_name_profile_query("Lakis Chalkias");
        assert!(query.contains("alias:\"Lakis Chalkias\""));

        // These are the profile payloads produced by the bounded hydration
        // step for a cold artist/alias search. Ownership is checked against
        // the hydrated aliases, not the search hit's ranking or canonical name.
        let profiles = vec![
            (
                "612d1573-bdea-4657-bfc9-f78593c9ab09".to_owned(),
                json!({
                    "name":"Lakis Halkias",
                    "aliases":[{"name":"Lakis Chalkias","type":"Artist name"}]
                }),
            ),
            (
                "712d1573-bdea-4657-bfc9-f78593c9ab09".to_owned(),
                json!({"name":"Another Artist","aliases":[]}),
            ),
        ];
        assert_eq!(
            unique_exact_name_owner(&profiles, "Lakis Chalkias").as_deref(),
            Some("612d1573-bdea-4657-bfc9-f78593c9ab09")
        );
    }

    #[test]
    fn exact_owned_alias_can_be_canonical_or_noncanonical_but_homonyms_stay_ambiguous() {
        let profiles = vec![(
            "612d1573-bdea-4657-bfc9-f78593c9ab09".to_owned(),
            json!({"name":"Lakis Halkias","aliases":[{"name":"Lakis Chalkias"}]}),
        )];
        assert_eq!(
            unique_exact_name_owner(&profiles, "Lakis Chalkias").as_deref(),
            Some("612d1573-bdea-4657-bfc9-f78593c9ab09")
        );
        assert!(unique_exact_name_owner(&profiles, "Unrelated Artist").is_none());

        let homonyms = vec![
            profiles[0].clone(),
            (
                "712d1573-bdea-4657-bfc9-f78593c9ab09".to_owned(),
                json!({"name":"Another Artist","aliases":[{"name":"Lakis Chalkias"}]}),
            ),
        ];
        assert!(unique_exact_name_owner(&homonyms, "Lakis Chalkias").is_none());
        assert!(unique_exact_name_owner(
            &[(
                "812d1573-bdea-4657-bfc9-f78593c9ab09".into(),
                json!({
                    "name":"Lakis Halkias",
                    "aliases":[{"name":"Lakis Chalkias","type":"Search hint"}]
                })
            )],
            "Lakis Chalkias"
        )
        .is_none());
    }
}
fn relation_urls(profile: &Value) -> impl Iterator<Item = &str> {
    profile["relations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["url"]["resource"].as_str())
}
fn wikidata_id(profile: &Value) -> Option<String> {
    relation_urls(profile).find_map(|raw| {
        let url = url::Url::parse(raw).ok()?;
        if !matches!(url.host_str(), Some("www.wikidata.org" | "wikidata.org")) {
            return None;
        }
        let id = url.path().strip_prefix("/wiki/")?;
        (id.starts_with('Q') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit()))
            .then(|| id.to_string())
    })
}
fn claims(entity: &Value, property: &str) -> BTreeSet<String> {
    entity["claims"][property]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["rank"] != "deprecated")
        .filter_map(|c| {
            c["mainsnak"]["datavalue"]["value"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}
fn external_ids(profile: &Value, host: &str, prefix: &str) -> BTreeSet<String> {
    relation_urls(profile)
        .filter_map(|raw| {
            let url = url::Url::parse(raw).ok()?;
            if url.host_str() != Some(host) {
                return None;
            }
            let id = url.path().strip_prefix(prefix)?;
            (!id.is_empty() && !id.contains('/'))
                .then(|| id.split('-').next().unwrap_or(id).to_owned())
        })
        .collect()
}
fn corroborated(profile: &Value, entity: &Value, id: &str) -> bool {
    profile["id"].as_str() == Some(id)
        && wikidata_id(profile)
            .as_deref()
            .map_or(true, |qid| Some(qid) == entity["id"].as_str())
        && (claims(entity, "P434").is_empty() || claims(entity, "P434").contains(id))
        && [
            ("P1953", "www.discogs.com", "/artist/"),
            ("P1953", "discogs.com", "/artist/"),
            ("P1902", "open.spotify.com", "/artist/"),
        ]
        .iter()
        .any(|(property, host, prefix)| {
            !external_ids(profile, host, prefix).is_disjoint(&claims(entity, property))
        })
}
/// Retained raw evidence is revalidated at read time; caller must independently
/// prove the recording's credit is attached to this exact MusicBrainz artist ID.
pub(crate) fn verified_wikidata_alias_names(
    store: &Store,
    id: &str,
    profile: &Value,
) -> Vec<String> {
    let Some(evidence) = store.get_provider_cache(EVIDENCE, id).ok().flatten() else {
        return vec![];
    };
    let entity = &evidence["entity"];
    if wikidata_id(&evidence["profile"]).is_none() {
        let Some(bindings) = evidence["locator"]["results"]["bindings"]
            .as_array()
            .filter(|items| items.len() == 1)
        else {
            return vec![];
        };
        let Some(uri) = bindings[0]["item"]["value"].as_str() else {
            return vec![];
        };
        if uri.strip_prefix("http://www.wikidata.org/entity/") != entity["id"].as_str() {
            return vec![];
        }
    }
    if evidence["version"] != 1
        || evidence["artistId"].as_str() != Some(id)
        || !corroborated(profile, entity, id)
        || !corroborated(&evidence["profile"], entity, id)
    {
        return vec![];
    }
    let mut names = BTreeSet::new();
    for key in ["labels", "aliases"] {
        if let Some(languages) = entity[key].as_object() {
            for values in languages.values() {
                let values = values
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![values.clone()]);
                for value in values {
                    if let Some(name) = value["value"]
                        .as_str()
                        .filter(|name| !name.trim().is_empty() && name.len() <= 256)
                    {
                        names.insert(name.to_owned());
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_shared_ids_required() {
        let profile = json!({"id":"mb","relations":[{"url":{"resource":"https://www.wikidata.org/wiki/Q42"}},{"url":{"resource":"https://www.discogs.com/artist/123-Example"}}]});
        let mut entity = json!({"id":"Q42","claims":{"P434":[{"mainsnak":{"datavalue":{"value":"mb"}}}],"P1953":[{"mainsnak":{"datavalue":{"value":"123"}}}]}});
        assert!(corroborated(&profile, &entity, "mb"));
        entity["claims"]["P1953"][0]["mainsnak"]["datavalue"]["value"] = json!("456");
        assert!(!corroborated(&profile, &entity, "mb"));
    }
    #[test]
    fn shared_discogs_identity_can_bridge_missing_direct_link_but_not_conflict() {
        let profile = json!({"id":"mb","relations":[{"url":{"resource":"https://www.discogs.com/artist/123"}}]});
        let mut entity =
            json!({"id":"Q42","claims":{"P1953":[{"mainsnak":{"datavalue":{"value":"123"}}}]}});
        assert!(corroborated(&profile, &entity, "mb"));
        entity["claims"]["P434"] = json!([{"mainsnak":{"datavalue":{"value":"another-mb"}}}]);
        assert!(!corroborated(&profile, &entity, "mb"));
    }
    #[test]
    fn similar_hosts_and_deprecated_claims_fail() {
        let profile = json!({"id":"mb","relations":[{"url":{"resource":"https://www.wikidata.org/wiki/Q42"}},{"url":{"resource":"https://open.spotify.com.evil.test/artist/abc"}}]});
        let entity = json!({"id":"Q42","claims":{"P434":[{"mainsnak":{"datavalue":{"value":"mb"}}}],"P1902":[{"mainsnak":{"datavalue":{"value":"abc"}}}]}});
        assert!(!corroborated(&profile, &entity, "mb"));
        assert!(claims(&json!({"claims":{"P434":[{"rank":"deprecated","mainsnak":{"datavalue":{"value":"mb"}}}]}}),"P434").is_empty());
    }
}

const ISRC_BINDING: &str = "artists.alias-discovery.isrc-authority-binding.v1";
const MB_OCCURRENCE_RUNTIME_CACHE: &str =
    "artists.alias-discovery.musicbrainz-occurrence-runtime.v2";
const MB_OCCURRENCE_RUNTIME_LEGACY_CACHE: &str =
    "artists.alias-discovery.musicbrainz-occurrence-runtime.v1";
const MB_OCCURRENCE_RUNTIME_TTL: u64 = 24 * 60 * 60;
const MB_OCCURRENCE_MAX_ISRCS: usize = 16;

#[derive(Debug, Clone)]
pub(crate) struct VerifiedReleaseRuntimeVariant {
    pub recording_id: String,
    pub release_id: String,
    pub isrc: String,
    /// ISRCs shared by the verified source occurrence and recording hit.
    /// Candidate codes may be selected only from this set.
    pub source_isrcs: Vec<String>,
    pub recording_title: String,
    pub release_track_title: String,
    pub release_artist_credit: String,
    /// Atomic artist-credit names in validated source order. These names are
    /// comparison-only and must not be split from the rendered group credit.
    pub release_artist_names: Vec<String>,
    pub release_artist_ids: Vec<String>,
    pub duration_seconds: f64,
}

fn mb_occurrence_runtime_fingerprint(
    track: &Track,
    occurrence: &aliases::VerifiedMusicBrainzOccurrence,
) -> Value {
    json!({
        "trackId": track.id,
        "providerId": track.provider_id,
        "releaseId": occurrence.release_id,
        "recordingId": occurrence.recording_id,
        "trackGuid": occurrence.track_guid,
        "title": occurrence.title,
        "durationSeconds": occurrence.duration_seconds,
        "isrc": track.isrc,
        "sourceIsrcs": occurrence.isrcs,
        "artistIds": occurrence.artist_ids,
    })
}

fn mb_occurrence_runtime_legacy_fingerprint(
    track: &Track,
    occurrence: &aliases::VerifiedMusicBrainzOccurrence,
) -> Value {
    json!({
        "trackId": track.id,
        "providerId": track.provider_id,
        "releaseId": occurrence.release_id,
        "recordingId": occurrence.recording_id,
        "trackGuid": occurrence.track_guid,
        "title": occurrence.title,
        "durationSeconds": occurrence.duration_seconds,
        "isrc": track.isrc,
        "artistIds": occurrence.artist_ids,
    })
}

fn normalized_isrc_set(values: &[String]) -> Option<Vec<String>> {
    if values.is_empty() || values.len() > MB_OCCURRENCE_MAX_ISRCS {
        return None;
    }
    let mut normalized = values
        .iter()
        .map(|value| crate::recording_identity::normalize_isrc(value))
        .collect::<Option<Vec<_>>>()?;
    normalized.sort();
    normalized.dedup();
    (!normalized.is_empty() && normalized.len() <= MB_OCCURRENCE_MAX_ISRCS).then_some(normalized)
}

fn verified_isrc_intersection(
    hit: &Value,
    occurrence: &aliases::VerifiedMusicBrainzOccurrence,
    query_isrc: &str,
) -> Option<Vec<String>> {
    let source_codes = normalized_isrc_set(&occurrence.isrcs)?;
    let raw_hit_codes = hit["isrcs"].as_array()?;
    if raw_hit_codes.is_empty() || raw_hit_codes.len() > MB_OCCURRENCE_MAX_ISRCS {
        return None;
    }
    let hit_codes = raw_hit_codes
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(crate::recording_identity::normalize_isrc)
        .collect::<Option<std::collections::BTreeSet<_>>>()?;
    let source_codes = source_codes
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let intersection = source_codes
        .intersection(&hit_codes)
        .cloned()
        .collect::<Vec<_>>();
    (intersection.iter().any(|code| code == query_isrc) && !intersection.is_empty())
        .then_some(intersection)
}

fn mb_occurrence_query_isrc(
    track: &Track,
    occurrence: &aliases::VerifiedMusicBrainzOccurrence,
) -> Option<String> {
    if occurrence.isrcs.is_empty() || occurrence.isrcs.len() > MB_OCCURRENCE_MAX_ISRCS {
        return None;
    }
    let manually_owned = track.provenance["manualFields"]["isrc"].as_bool() == Some(true);
    let scalar = track
        .isrc
        .as_deref()
        .and_then(crate::recording_identity::normalize_isrc);
    if scalar
        .as_ref()
        .is_some_and(|code| !occurrence.isrcs.contains(code))
        || scalar.is_none() && manually_owned
    {
        return None;
    }
    occurrence.isrcs.first().cloned()
}

pub(crate) fn same_recording_release_title(expected: &str, actual: &str) -> bool {
    let expected = crate::recording_identity::parse_recording_text(expected);
    let actual = crate::recording_identity::parse_recording_text(actual);
    let has_conflicting_annotation = |parsed: &crate::recording_identity::ParsedRecordingText| {
        parsed.parsing_incomplete
            || parsed.annotations.iter().any(|annotation| {
                matches!(
                    annotation.kind,
                    crate::recording_identity::AnnotationKind::Presentation
                        | crate::recording_identity::AnnotationKind::SemanticVersion
                        | crate::recording_identity::AnnotationKind::FeaturedCredit
                        | crate::recording_identity::AnnotationKind::Soundtrack
                )
            })
    };
    !has_conflicting_annotation(&expected)
        && !has_conflicting_annotation(&actual)
        && crate::matching::normalize(&expected.base_title)
            == crate::matching::normalize(&actual.base_title)
}

/// Allow small catalog spelling/transliteration differences only after the
/// caller has independently bound both strings to one exact recording and
/// release occurrence. Semantic version, presentation, guest and soundtrack
/// annotations remain hard exclusions.
pub(crate) fn recording_title_variant_compatible(expected: &str, actual: &str) -> bool {
    let expected = crate::recording_identity::parse_recording_text(expected);
    let actual = crate::recording_identity::parse_recording_text(actual);
    let safe = |parsed: &crate::recording_identity::ParsedRecordingText| {
        !parsed.parsing_incomplete
            && !parsed.annotations.iter().any(|annotation| {
                matches!(
                    annotation.kind,
                    crate::recording_identity::AnnotationKind::Presentation
                        | crate::recording_identity::AnnotationKind::SemanticVersion
                        | crate::recording_identity::AnnotationKind::FeaturedCredit
                        | crate::recording_identity::AnnotationKind::Soundtrack
                )
            })
    };
    if !safe(&expected) || !safe(&actual) {
        return false;
    }
    let expected = crate::matching::normalize(&expected.base_title);
    let actual = crate::matching::normalize(&actual.base_title);
    if expected.is_empty() || actual.is_empty() {
        return false;
    }
    expected == actual || strsim::normalized_levenshtein(&expected, &actual) >= 0.88
}

/// Compare two title renderings already bound to the same recording and
/// release occurrence. This allows a small transliteration difference in the
/// provider's rendering while still rejecting semantic-version, presentation,
/// guest, and soundtrack annotations. Callers must independently validate
/// recording/ISRC, ordered credits, and runtime before using this result.
pub(crate) fn occurrence_bound_title_rendering_compatible(expected: &str, actual: &str) -> bool {
    let expected = crate::recording_identity::parse_recording_text(expected);
    let actual = crate::recording_identity::parse_recording_text(actual);
    let safe = |parsed: &crate::recording_identity::ParsedRecordingText| {
        !parsed.parsing_incomplete
            && !parsed.annotations.iter().any(|annotation| {
                matches!(
                    annotation.kind,
                    crate::recording_identity::AnnotationKind::Presentation
                        | crate::recording_identity::AnnotationKind::SemanticVersion
                        | crate::recording_identity::AnnotationKind::FeaturedCredit
                        | crate::recording_identity::AnnotationKind::Soundtrack
                )
            })
    };
    if !safe(&expected) || !safe(&actual) {
        return false;
    }
    let expected = crate::matching::normalize(&expected.base_title);
    let actual = crate::matching::normalize(&actual.base_title);
    !expected.is_empty()
        && !actual.is_empty()
        && (expected == actual || strsim::normalized_levenshtein(&expected, &actual) >= 0.80)
}

fn mb_occurrence_runtime_variants_from_search(
    search: &Value,
    occurrence: &aliases::VerifiedMusicBrainzOccurrence,
    requested_isrc: &str,
) -> Vec<VerifiedReleaseRuntimeVariant> {
    let Some(hits) = search["recordings"].as_array() else {
        return Vec::new();
    };
    if hits.is_empty() || hits.len() > 5 || search["count"].as_u64() != Some(hits.len() as u64) {
        return Vec::new();
    }
    let supported = hits
        .iter()
        .filter(|hit| {
            hit["id"].as_str() == Some(occurrence.recording_id.as_str())
                && hit["title"].as_str().is_some_and(|title| {
                    recording_title_variant_compatible(&occurrence.title, title)
                })
                && verified_isrc_intersection(hit, occurrence, requested_isrc).is_some()
                && hit["artist-credit"].as_array().is_some_and(|credits| {
                    credits.len() == occurrence.artist_ids.len()
                        && credits
                            .iter()
                            .zip(&occurrence.artist_ids)
                            .all(|(credit, id)| {
                                credit["artist"]["id"].as_str() == Some(id.as_str())
                            })
                })
        })
        .collect::<Vec<_>>();
    if supported.len() != 1 {
        return Vec::new();
    }
    let hit = supported[0];
    let Some(source_isrcs) = verified_isrc_intersection(hit, occurrence, requested_isrc) else {
        return Vec::new();
    };
    let recording_credits = hit["artist-credit"].as_array().unwrap();
    let mut variants = Vec::new();
    for release in hit["releases"].as_array().into_iter().flatten().take(128) {
        let Some(release_id) = release["id"]
            .as_str()
            .filter(|id| aliases::valid_musicbrainz_recording_id(id))
        else {
            continue;
        };
        let Some(release_credits) = release["artist-credit"].as_array() else {
            continue;
        };
        if release_credits.len() != recording_credits.len()
            || !release_credits
                .iter()
                .zip(recording_credits)
                .all(|(a, b)| a["artist"]["id"].as_str() == b["artist"]["id"].as_str())
        {
            continue;
        }
        let Some(release_credit_parts) = release_credits
            .iter()
            .enumerate()
            .map(|(index, credit)| {
                let name = credit["name"]
                    .as_str()
                    .or(credit["artist"]["name"].as_str())?
                    .trim();
                let join = aliases::verified_musicbrainz_joinphrase(
                    credit,
                    index + 1 == release_credits.len(),
                )?;
                (!name.is_empty() && name.len() <= 256 && join.len() <= 32)
                    .then(|| (name.to_owned(), format!("{name}{join}")))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let release_artist_names = release_credit_parts
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let release_artist_ids = release_credits
            .iter()
            .filter_map(|credit| credit["artist"]["id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let release_artist_credit = release_credit_parts
            .iter()
            .map(|(_, rendered)| rendered.as_str())
            .collect::<String>();
        if release_artist_credit.is_empty() || release_artist_credit.len() > 4096 {
            continue;
        }
        for medium in release["media"].as_array().into_iter().flatten().take(16) {
            let tracks = medium["tracks"]
                .as_array()
                .or_else(|| medium["track"].as_array());
            for track in tracks.into_iter().flatten().take(256) {
                if !track["id"]
                    .as_str()
                    .is_some_and(aliases::valid_musicbrainz_recording_id)
                    || track["recording"]["id"]
                        .as_str()
                        .is_some_and(|id| id != occurrence.recording_id)
                {
                    continue;
                }
                let Some(length) = track["length"].as_u64() else {
                    continue;
                };
                if !(1..=86_400_000).contains(&length) {
                    continue;
                }
                let Some(title) = track["title"]
                    .as_str()
                    .filter(|title| !title.trim().is_empty() && title.len() <= 512)
                else {
                    continue;
                };
                if !recording_title_variant_compatible(&occurrence.title, title) {
                    continue;
                }
                variants.push(VerifiedReleaseRuntimeVariant {
                    recording_id: occurrence.recording_id.clone(),
                    release_id: release_id.to_owned(),
                    isrc: requested_isrc.to_owned(),
                    source_isrcs: source_isrcs.clone(),
                    recording_title: occurrence.title.clone(),
                    release_track_title: title.to_owned(),
                    release_artist_credit: release_artist_credit.clone(),
                    release_artist_names: release_artist_names.clone(),
                    release_artist_ids: release_artist_ids.clone(),
                    duration_seconds: length as f64 / 1000.0,
                });
            }
        }
    }
    variants.sort_by(|a, b| {
        a.release_id
            .cmp(&b.release_id)
            .then(a.duration_seconds.total_cmp(&b.duration_seconds))
            .then(a.release_track_title.cmp(&b.release_track_title))
    });
    variants.dedup_by(|a, b| {
        a.release_id == b.release_id
            && a.duration_seconds == b.duration_seconds
            && a.release_track_title == b.release_track_title
    });
    variants
}

pub(crate) fn cached_musicbrainz_occurrence_runtime_variants(
    store: &Store,
    track: &Track,
) -> Vec<VerifiedReleaseRuntimeVariant> {
    let Some(occurrence) = aliases::verified_musicbrainz_occurrence(store, track) else {
        return Vec::new();
    };
    let Some(requested_isrc) = mb_occurrence_query_isrc(track, &occurrence) else {
        return Vec::new();
    };
    let cached_v2 = store
        .get_provider_cache(MB_OCCURRENCE_RUNTIME_CACHE, &track.id)
        .ok()
        .flatten();
    if let Some(cached) = cached_v2 {
        if cached["version"] == 2
            && cached["fingerprint"] == mb_occurrence_runtime_fingerprint(track, &occurrence)
            && cached["queryIsrc"].as_str() == Some(requested_isrc.as_str())
        {
            return mb_occurrence_runtime_variants_from_search(
                &cached["search"],
                &occurrence,
                &requested_isrc,
            );
        }
    }
    // v1 contains raw provider responses, not decisions. Reuse it only when
    // its old full source-occurrence fingerprint still matches, then run the
    // current unique-hit, code-set, artist-order and occurrence validators.
    let Some(legacy) = store
        .get_provider_cache(MB_OCCURRENCE_RUNTIME_LEGACY_CACHE, &track.id)
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    if legacy["version"] != 1
        || legacy["fingerprint"] != mb_occurrence_runtime_legacy_fingerprint(track, &occurrence)
    {
        return Vec::new();
    }
    mb_occurrence_runtime_variants_from_search(&legacy["search"], &occurrence, &requested_isrc)
}

async fn discover_musicbrainz_occurrence_runtime_variants(
    store: &Store,
    track: &Track,
) -> Result<(), String> {
    let Some(occurrence) = aliases::verified_musicbrainz_occurrence(store, track) else {
        return Ok(());
    };
    let Some(requested_isrc) = mb_occurrence_query_isrc(track, &occurrence) else {
        return Ok(());
    };
    let fingerprint = mb_occurrence_runtime_fingerprint(track, &occurrence);
    if store
        .get_provider_cache(MB_OCCURRENCE_RUNTIME_CACHE, &track.id)?
        .is_some_and(|cached| {
            cached["version"] == 2
                && cached["fingerprint"] == fingerprint
                && cached["queryIsrc"].as_str() == Some(requested_isrc.as_str())
        })
    {
        return Ok(());
    }
    let query = format!("isrc:{requested_isrc}");
    let search = fetch(
        "https://musicbrainz.org/ws/2/recording/",
        &[
            ("fmt", "json".into()),
            ("query", query),
            ("limit", "5".into()),
            ("inc", "artist-credits+isrcs+releases".into()),
        ],
    )
    .await?;
    let _ = mb_occurrence_runtime_variants_from_search(&search, &occurrence, &requested_isrc);
    // Store complete raw metadata tied to this exact saved occurrence. Every
    // playback read reparses the source release and repeats the recording,
    // ISRC, ordered-credit and release-occurrence checks.
    let hits = search["recordings"].as_array();
    if hits.is_some_and(|hits| {
        !hits.is_empty() && hits.len() <= 5 && search["count"].as_u64() == Some(hits.len() as u64)
    }) {
        store.set_provider_cache(
            MB_OCCURRENCE_RUNTIME_CACHE,
            &track.id,
            &json!({"version":2,"fingerprint":fingerprint,"queryIsrc":requested_isrc,"search":search}),
            MB_OCCURRENCE_RUNTIME_TTL,
        )?;
    }
    Ok(())
}

/// A release occurrence may document a distinct physical/digital runtime for
/// the same exact recording. Return only a uniquely bound recording and a
/// release occurrence whose full ordered MB artist IDs match that recording.
pub(crate) fn cached_isrc_authority_runtime_variants(
    store: &Store,
    track: &Track,
) -> Vec<VerifiedReleaseRuntimeVariant> {
    let hydrated = crate::remote_playlists::read_alias_hydrated_track(store, track)
        .unwrap_or_else(|| track.clone());
    let Some(source) = aliases::spotify_artist_credit_fingerprint(&hydrated) else {
        return Vec::new();
    };
    let Some(requested_isrc) = source.isrc.as_deref() else {
        return Vec::new();
    };
    let Some(binding) = store
        .get_provider_cache(ISRC_BINDING, &track.provider_id)
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    if binding["version"] != 1
        || binding["fingerprint"] != serde_json::to_value(&source).unwrap_or(Value::Null)
    {
        return Vec::new();
    }
    let Some(recording_id) = binding["recordingId"].as_str() else {
        return Vec::new();
    };
    let Some(search_hits) = binding["search"]["recordings"].as_array() else {
        return Vec::new();
    };
    if binding["search"]["count"].as_u64() != Some(search_hits.len() as u64)
        || search_hits.len() > 5
    {
        return Vec::new();
    }
    let Some(recording) = store
        .get_provider_cache(
            aliases::spotify_recording_credit_raw_cache_namespace(),
            recording_id,
        )
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    if recording["id"].as_str() != Some(recording_id)
        || crate::matching::normalize(recording["title"].as_str().unwrap_or_default())
            != crate::matching::normalize(&source.title)
    {
        return Vec::new();
    }
    let Some(isrcs) = recording["isrcs"].as_array() else {
        return Vec::new();
    };
    if !isrcs.iter().any(|value| {
        value
            .as_str()
            .is_some_and(|code| code.eq_ignore_ascii_case(requested_isrc))
    }) {
        return Vec::new();
    }
    // Spotify artist IDs and MusicBrainz artist IDs are different namespaces.
    // Validate their exact ordered crosswalk through the cached owning profile
    // relations rather than comparing the raw identifiers directly.
    if aliases::verify_spotify_artist_credit_recording(store, &hydrated, recording_id).is_none() {
        return Vec::new();
    }
    let Some(credits) = recording["artist-credit"].as_array() else {
        return Vec::new();
    };
    if credits.is_empty() || credits.len() > 16 {
        return Vec::new();
    }
    let mut exact_hits = search_hits
        .iter()
        .filter(|hit| hit["id"].as_str() == Some(recording_id));
    let Some(exact_hit) = exact_hits.next() else {
        return Vec::new();
    };
    if exact_hits.next().is_some() {
        return Vec::new();
    }
    let mut variants = Vec::new();
    for release in exact_hit["releases"]
        .as_array()
        .into_iter()
        .flatten()
        .take(128)
    {
        let Some(release_id) = release["id"]
            .as_str()
            .filter(|id| aliases::valid_musicbrainz_recording_id(id))
        else {
            continue;
        };
        let Some(release_credits) = release["artist-credit"].as_array() else {
            continue;
        };
        if release_credits.len() != credits.len()
            || !release_credits.iter().zip(credits).all(|(a, b)| {
                a["artist"]["id"].as_str().is_some_and(|id| {
                    aliases::valid_musicbrainz_recording_id(id)
                        && b["artist"]["id"].as_str() == Some(id)
                })
            })
        {
            continue;
        }
        let Some(release_credit_parts) = release_credits
            .iter()
            .enumerate()
            .map(|(index, credit)| {
                let name = credit["name"]
                    .as_str()
                    .or(credit["artist"]["name"].as_str())?
                    .trim();
                let join = aliases::verified_musicbrainz_joinphrase(
                    credit,
                    index + 1 == release_credits.len(),
                )?;
                (!name.is_empty() && name.len() <= 256 && join.len() <= 32)
                    .then(|| (name.to_owned(), format!("{name}{join}")))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let release_artist_names = release_credit_parts
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let release_artist_ids = release_credits
            .iter()
            .filter_map(|credit| credit["artist"]["id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let release_artist_credit = release_credit_parts
            .iter()
            .map(|(_, rendered)| rendered.as_str())
            .collect::<String>();
        if release_artist_credit.is_empty() || release_artist_credit.len() > 4096 {
            continue;
        }
        for medium in release["media"].as_array().into_iter().flatten().take(16) {
            for occurrence in medium["track"].as_array().into_iter().flatten().take(256) {
                if !occurrence["id"]
                    .as_str()
                    .is_some_and(aliases::valid_musicbrainz_recording_id)
                    || occurrence["recording"]["id"]
                        .as_str()
                        .is_some_and(|id| id != recording_id)
                {
                    continue;
                }
                let Some(length) = occurrence["length"].as_u64() else {
                    continue;
                };
                if !(1..=86_400_000).contains(&length) {
                    continue;
                }
                let Some(title) = occurrence["title"]
                    .as_str()
                    .filter(|title| !title.trim().is_empty() && title.len() <= 512)
                else {
                    continue;
                };
                let recording_title = recording["title"].as_str().unwrap_or_default();
                if !same_recording_release_title(recording_title, title) {
                    continue;
                }
                variants.push(VerifiedReleaseRuntimeVariant {
                    recording_id: recording_id.to_owned(),
                    release_id: release_id.to_owned(),
                    isrc: requested_isrc.to_owned(),
                    source_isrcs: vec![requested_isrc.to_owned()],
                    recording_title: recording_title.to_owned(),
                    release_track_title: title.to_owned(),
                    release_artist_credit: release_artist_credit.clone(),
                    release_artist_names: release_artist_names.clone(),
                    release_artist_ids: release_artist_ids.clone(),
                    duration_seconds: length as f64 / 1000.0,
                });
            }
        }
    }
    variants.sort_by(|a, b| {
        a.release_id
            .cmp(&b.release_id)
            .then(a.duration_seconds.total_cmp(&b.duration_seconds))
            .then(a.release_track_title.cmp(&b.release_track_title))
    });
    variants.dedup_by(|a, b| {
        a.release_id == b.release_id
            && a.duration_seconds == b.duration_seconds
            && a.release_track_title == b.release_track_title
    });
    variants
}

/// Add release-specific renderings only when an exact recording search hit
/// links the release occurrence and its complete ordered artist IDs agree with
/// the recording credit. These spellings are recording-scoped; they are not
/// promoted to reusable artist aliases.
fn release_credit_renderings(
    search: &Value,
    recording_id: &str,
    recording_credits: &[Value],
) -> Vec<Vec<String>> {
    let Some(hits) = search["recordings"].as_array() else {
        return Vec::new();
    };
    if hits.is_empty()
        || hits.len() > 5
        || search["count"].as_u64() != Some(hits.len() as u64)
        || !aliases::valid_musicbrainz_recording_id(recording_id)
        || recording_credits.is_empty()
        || recording_credits.len() > 16
    {
        return Vec::new();
    }
    let mut exact_hits = hits
        .iter()
        .filter(|hit| hit["id"].as_str() == Some(recording_id));
    let Some(hit) = exact_hits.next() else {
        return Vec::new();
    };
    if exact_hits.next().is_some() {
        return Vec::new();
    }
    let Some(mut names_by_credit) = recording_credits
        .iter()
        .map(|credit| {
            let id = credit["artist"]["id"].as_str()?;
            aliases::valid_musicbrainz_recording_id(id).then(|| BTreeSet::new())
        })
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };

    for release in hit["releases"].as_array().into_iter().flatten().take(128) {
        let Some(release_id) = release["id"].as_str() else {
            continue;
        };
        if !aliases::valid_musicbrainz_recording_id(release_id) {
            continue;
        }
        // The release list is nested in this exact recording hit. Still require
        // an occurrence in its media and reject any explicit conflicting
        // recording ID when the provider supplies one.
        let occurrence_present = release["media"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|medium| medium["track"].as_array().into_iter().flatten())
            .any(|track| {
                track["id"]
                    .as_str()
                    .is_some_and(aliases::valid_musicbrainz_recording_id)
                    && track["recording"]["id"]
                        .as_str()
                        .is_none_or(|id| id == recording_id)
            });
        if !occurrence_present {
            continue;
        }
        let Some(credits) = release["artist-credit"].as_array() else {
            continue;
        };
        if credits.len() != recording_credits.len() || credits.is_empty() {
            continue;
        }
        let same_ordered_ids =
            credits
                .iter()
                .zip(recording_credits)
                .all(|(release_credit, recording_credit)| {
                    release_credit["artist"]["id"].as_str().is_some_and(|id| {
                        aliases::valid_musicbrainz_recording_id(id)
                            && recording_credit["artist"]["id"].as_str() == Some(id)
                    })
                });
        if !same_ordered_ids {
            continue;
        }
        for (index, credit) in credits.iter().enumerate() {
            if let Some(name) = credit["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty() && name.len() <= 256)
            {
                names_by_credit[index].insert(name.to_owned());
            }
        }
    }
    names_by_credit
        .into_iter()
        .map(|names| names.into_iter().collect())
        .collect()
}

#[cfg(test)]
mod release_credit_rendering_tests {
    use super::*;

    const RECORDING: &str = "00000000-0000-4000-8000-000000000001";
    const ARTIST_A: &str = "00000000-0000-4000-8000-000000000002";
    const ARTIST_B: &str = "00000000-0000-4000-8000-000000000003";
    const RELEASE: &str = "00000000-0000-4000-8000-000000000004";
    const TRACK: &str = "00000000-0000-4000-8000-000000000005";

    fn release(credits: Value, recording_id: &str) -> Value {
        json!({
            "id": RELEASE,
            "artist-credit": credits,
            "media": [{"track": [{"id": TRACK, "recording": {"id": recording_id}}]}]
        })
    }

    fn search(release: Value, recording_id: &str) -> Value {
        json!({
            "count": 1,
            "recordings": [{"id": recording_id, "releases": [release]}]
        })
    }

    #[test]
    fn exact_recording_release_spelling_requires_same_full_ordered_credit() {
        let recording_credits = json!([
            {"artist":{"id":ARTIST_A,"name":"Authority A"}},
            {"artist":{"id":ARTIST_B,"name":"Authority B"}}
        ]);
        let exact = search(
            release(
                json!([
                    {"name":"Alternate A","artist":{"id":ARTIST_A}},
                    {"name":"Alternate B","artist":{"id":ARTIST_B}}
                ]),
                RECORDING,
            ),
            RECORDING,
        );
        assert_eq!(
            release_credit_renderings(&exact, RECORDING, recording_credits.as_array().unwrap()),
            vec![
                vec!["Alternate A".to_owned()],
                vec!["Alternate B".to_owned()]
            ]
        );

        let missing_guest = search(
            release(
                json!([{"name":"Alternate A","artist":{"id":ARTIST_A}}]),
                RECORDING,
            ),
            RECORDING,
        );
        assert!(release_credit_renderings(
            &missing_guest,
            RECORDING,
            recording_credits.as_array().unwrap()
        )
        .iter()
        .all(Vec::is_empty));

        let reordered = search(
            release(
                json!([
                    {"name":"Alternate B","artist":{"id":ARTIST_B}},
                    {"name":"Alternate A","artist":{"id":ARTIST_A}}
                ]),
                RECORDING,
            ),
            RECORDING,
        );
        assert!(release_credit_renderings(
            &reordered,
            RECORDING,
            recording_credits.as_array().unwrap()
        )
        .iter()
        .all(Vec::is_empty));
    }

    #[test]
    fn release_credit_variant_requires_occurrence_of_exact_recording() {
        let recording_credits = json!([
            {"artist":{"id":ARTIST_A,"name":"Authority A"}}
        ]);
        let other_recording = search(
            release(
                json!([{"name":"Alternate A","artist":{"id":ARTIST_A}}]),
                "00000000-0000-4000-8000-000000000006",
            ),
            RECORDING,
        );
        assert!(release_credit_renderings(
            &other_recording,
            RECORDING,
            recording_credits.as_array().unwrap()
        )
        .iter()
        .all(Vec::is_empty));

        let wrong_search_hit = search(
            release(
                json!([{"name":"Alternate A","artist":{"id":ARTIST_A}}]),
                RECORDING,
            ),
            "00000000-0000-4000-8000-000000000006",
        );
        assert!(release_credit_renderings(
            &wrong_search_hit,
            RECORDING,
            recording_credits.as_array().unwrap()
        )
        .iter()
        .all(Vec::is_empty));
    }
}

fn isrc_authority_choices(
    store: &Store,
    track: &Track,
    recording_id: &str,
    release_search: Option<&Value>,
) -> Option<Vec<(BTreeSet<String>, String)>> {
    let source = aliases::spotify_artist_credit_fingerprint(track)?;
    let source_isrc = source.isrc.as_deref()?;
    let recording = store
        .get_provider_cache(
            aliases::spotify_recording_credit_raw_cache_namespace(),
            recording_id,
        )
        .ok()??;
    if recording["id"].as_str() != Some(recording_id)
        || crate::matching::normalize(recording["title"].as_str()?)
            != crate::matching::normalize(&source.title)
        || recording["length"].as_u64()? == 0
        || recording["length"].as_u64()?.abs_diff(source.duration_ms) > 5000
    {
        return None;
    }
    let isrcs = recording["isrcs"]
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(|isrc| {
            (isrc.len() == 12 && isrc.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                .then(|| isrc.to_ascii_uppercase())
        })
        .collect::<Option<BTreeSet<_>>>()?;
    if isrcs.is_empty() || !isrcs.contains(source_isrc) {
        return None;
    }
    let credits = recording["artist-credit"].as_array()?;
    if credits.len() != source.artists.len() || credits.is_empty() || credits.len() > 16 {
        return None;
    }
    let cached_binding = store
        .get_provider_cache(ISRC_BINDING, &track.provider_id)
        .ok()
        .flatten();
    let rendered_release_names = if let Some(search) = release_search {
        release_credit_renderings(search, recording_id, credits)
    } else if let Some(binding) = cached_binding.as_ref().filter(|binding| {
        binding["version"] == 1
            && binding["recordingId"].as_str() == Some(recording_id)
            && binding["fingerprint"] == serde_json::to_value(&source).unwrap_or(Value::Null)
    }) {
        release_credit_renderings(&binding["search"], recording_id, credits)
    } else {
        Vec::new()
    };
    let mut choices = Vec::new();
    for (index, (credit, provider)) in credits.iter().zip(&source.artists).enumerate() {
        let id = credit["artist"]["id"].as_str()?;
        if !aliases::valid_musicbrainz_recording_id(id) {
            return None;
        }
        let profile = store
            .get_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                id,
            )
            .ok()??;
        if profile["id"].as_str() != Some(id) || profile["name"] != credit["artist"]["name"] {
            return None;
        }
        let mut names = BTreeSet::new();
        names.insert(profile["name"].as_str()?.to_owned());
        for name in profile["aliases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["type"].as_str() != Some("Search hint"))
            .filter_map(|a| a["name"].as_str())
        {
            if !name.trim().is_empty() && name.len() <= 256 {
                names.insert(name.to_owned());
            }
        }
        names.extend(verified_wikidata_alias_names(store, id, &profile));
        if let Some(release_names) = rendered_release_names.get(index) {
            names.extend(release_names.iter().cloned());
        }
        if !names.iter().any(|name| {
            crate::matching::normalize(name) == crate::matching::normalize(&provider.name)
        }) {
            return None;
        }
        let join = aliases::verified_musicbrainz_joinphrase(credit, index + 1 == credits.len())?;
        if join.len() > 32 {
            return None;
        }
        choices.push((names, join.to_owned()));
    }
    Some(choices)
}
fn isrc_authority_variants(
    store: &Store,
    track: &Track,
    recording_id: &str,
    release_search: Option<&Value>,
) -> Option<Vec<Track>> {
    let choices = isrc_authority_choices(store, track, recording_id, release_search)?;
    let mut renderings = vec![String::new()];
    for (names, join) in choices {
        renderings = renderings
            .iter()
            .flat_map(|prefix| {
                let join = &join;
                names
                    .iter()
                    .map(move |name| format!("{prefix}{name}{join}"))
            })
            .take(32)
            .collect();
    }
    Some(
        renderings
            .into_iter()
            .filter(|name| name != &track.artist)
            .map(|artist| {
                let mut alias = track.clone();
                alias.artist = artist;
                alias
            })
            .collect(),
    )
}
/// This fallback is scoped to an exact source ISRC plus a unique independently
/// supported recording and complete ordered authority aliases for every member.
pub(crate) fn cached_isrc_authority_alias_tracks(store: &Store, track: &Track) -> Vec<Track> {
    let hydrated = crate::remote_playlists::read_alias_hydrated_track(store, track)
        .unwrap_or_else(|| track.clone());
    let Some(source) = aliases::spotify_artist_credit_fingerprint(&hydrated) else {
        return vec![];
    };
    let Some(saved) = store.get_library_track(&track.id).ok().flatten() else {
        return vec![];
    };
    let saved = crate::remote_playlists::read_alias_hydrated_track(store, &saved).unwrap_or(saved);
    if aliases::spotify_artist_credit_fingerprint(&saved).as_ref() != Some(&source) {
        return vec![];
    }
    let Some(binding) = store
        .get_provider_cache(ISRC_BINDING, &track.provider_id)
        .ok()
        .flatten()
    else {
        return vec![];
    };
    if binding["version"] != 1
        || binding["fingerprint"] != serde_json::to_value(&source).unwrap_or(Value::Null)
    {
        return vec![];
    }
    let Some(id) = binding["recordingId"].as_str() else {
        return vec![];
    };
    let Some(hits) = binding["search"]["recordings"].as_array() else {
        return vec![];
    };
    if binding["search"]["count"].as_u64() != Some(hits.len() as u64) || hits.len() > 5 {
        return vec![];
    }
    let supported = hits
        .iter()
        .filter_map(|hit| hit["id"].as_str())
        .filter(|candidate| {
            aliases::verify_spotify_artist_credit_recording(store, &hydrated, candidate).is_some()
                || isrc_authority_variants(store, &hydrated, candidate, None).is_some()
        })
        .collect::<BTreeSet<_>>();
    if supported.len() != 1 || !supported.contains(id) {
        return vec![];
    }
    isrc_authority_variants(store, &hydrated, id, Some(&binding["search"]))
        .unwrap_or_default()
        .into_iter()
        .map(|alias| {
            let mut result = track.clone();
            result.artist = alias.artist;
            result
        })
        .collect()
}

#[cfg(test)]
mod recording_proof_tests {
    use super::*;

    #[test]
    fn captured_public_kalimankou_gloss_hit_allows_only_same_recording_base_title() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../docs/fixtures/musicbrainz-kalimankou-gloss-search-v1.json"
        ))
        .unwrap();
        let source = &fixture["sourceOccurrence"];
        let occurrence = aliases::VerifiedMusicBrainzOccurrence {
            release_id: source["releaseId"].as_str().unwrap().into(),
            recording_id: source["recordingId"].as_str().unwrap().into(),
            track_guid: source["trackGuid"].as_str().unwrap().into(),
            title: source["title"].as_str().unwrap().into(),
            duration_seconds: source["durationSeconds"].as_f64().unwrap(),
            artist_ids: source["artistIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect(),
            isrcs: source["isrcs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect(),
        };
        let variants = mb_occurrence_runtime_variants_from_search(
            &fixture["search"],
            &occurrence,
            "USNO18716503",
        );
        assert_eq!(variants.len(), 1);
        assert_eq!(
            variants[0].release_id,
            "7da26152-ccc7-4e0c-9f75-28b5edbcaea5"
        );
        assert_eq!(
            variants[0].release_track_title,
            "Kalimankou Denkou (The Evening Gathering)"
        );
        assert_eq!(
            variants[0].release_artist_credit,
            "Bulgarian State Television Female Choir"
        );
        assert_eq!(variants[0].source_isrcs, vec!["USNO18716503"]);

        let mut conflicting_version = fixture["search"].clone();
        conflicting_version["recordings"][0]["title"] = json!("Kalimankou Denkou (Live)");
        assert!(mb_occurrence_runtime_variants_from_search(
            &conflicting_version,
            &occurrence,
            "USNO18716503",
        )
        .is_empty());
    }

    #[test]
    fn musicbrainz_catalog_occurrence_uses_same_release_credit_and_runtime_variant() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("mb-runtime.sqlite")).unwrap();
        let release_id = "1f602996-f661-3611-b25a-93c3337fd76e";
        let recording_id = "32f1893e-8c1b-4640-a001-54cf66971988";
        let source_artist_id = "80fb9c75-af15-4a6b-9528-715f9ef4ebea";
        let source_track_guid = "1ffea9ee-597f-3c9d-8859-85b991c7e067";
        let alternate_release_id = "00000000-0000-4000-8000-000000000015";
        let alternate_track_guid = "00000000-0000-4000-8000-000000000016";
        let isrc = "USNO18716501";
        let second_isrc = "CH1648980010";
        let source_release = json!({
            "id":release_id,
            "title":"Requested Album",
            "artist-credit":[{"name":"Bulgarian State Radio and Television Female Vocal Choir","artist":{"id":source_artist_id,"name":"Bulgarian State Radio and Television Female Vocal Choir"}}],
            "media":[{"position":1,"tracks":[{
                "id":source_track_guid,"position":1,"number":"1","title":"Pilentze Pee","length":146666,
                "recording":{"id":recording_id,"title":"Pilentze Pee","length":146666,"isrcs":[isrc,second_isrc],"artist-credit":[{"name":"Bulgarian State Radio and Television Female Vocal Choir","artist":{"id":source_artist_id,"name":"Bulgarian State Radio and Television Female Vocal Choir"}}]}
            }]}]
        });
        let mut source_tracks = crate::artists::parse_musicbrainz_release_tracks(
            &source_release,
            release_id,
            &format!("https://musicbrainz.org/release/{release_id}"),
            false,
            None,
        );
        let source = source_tracks.remove(0);
        store
            .set_retained_provider_cache(
                "artists.release-tracks.musicbrainz",
                release_id,
                &source_release,
            )
            .unwrap();
        let occurrence = aliases::verified_musicbrainz_occurrence(&store, &source).unwrap();
        assert_eq!(occurrence.recording_id, recording_id);
        assert_eq!(occurrence.artist_ids, vec![source_artist_id]);
        assert_eq!(occurrence.isrcs, vec![second_isrc, isrc]);
        assert_eq!(source.isrc, None);

        let search = json!({
            "count":1,
            "recordings":[{
                "id":recording_id,"title":"Pilentze Pee (The Evening Gathering)","length":146666,"isrcs":[isrc,second_isrc],
                "artist-credit":[{"name":"Bulgarian State Radio and Television Female Vocal Choir","artist":{"id":source_artist_id,"name":"Bulgarian State Radio and Television Female Vocal Choir"}}],
                "releases":[{
                    "id":alternate_release_id,
                    "artist-credit":[{"name":"Bulgarian State Television Female Choir","artist":{"id":source_artist_id,"name":"Bulgarian State Radio and Television Female Vocal Choir"}}],
                    "media":[{"tracks":[{"id":alternate_track_guid,"position":1,"title":"Pilentze Pee (The Evening Gathering)","length":175000,"recording":{"id":recording_id}}]}]
                }]
            }]
        });
        store
            .set_provider_cache(
                MB_OCCURRENCE_RUNTIME_CACHE,
                &source.id,
                &json!({"version":2,"fingerprint":mb_occurrence_runtime_fingerprint(&source, &occurrence),"queryIsrc":second_isrc,"search":search}),
                MB_OCCURRENCE_RUNTIME_TTL,
            )
            .unwrap();
        let runtime = cached_musicbrainz_occurrence_runtime_variants(&store, &source);
        assert_eq!(runtime.len(), 1);
        assert_eq!(
            runtime[0].release_artist_credit,
            "Bulgarian State Television Female Choir"
        );
        assert_eq!(runtime[0].duration_seconds, 175.0);
        assert_eq!(runtime[0].source_isrcs, vec![second_isrc, isrc]);

        // Older installs retained the raw recording search under v1. Reuse
        // that raw response only after rebinding it to the current source
        // occurrence and rerunning every current validator.
        store
            .set_provider_cache(
                MB_OCCURRENCE_RUNTIME_LEGACY_CACHE,
                &source.id,
                &json!({
                    "version":1,
                    "fingerprint":mb_occurrence_runtime_legacy_fingerprint(&source, &occurrence),
                    "search":search
                }),
                MB_OCCURRENCE_RUNTIME_TTL,
            )
            .unwrap();
        store
            .set_provider_cache(
                MB_OCCURRENCE_RUNTIME_CACHE,
                &source.id,
                &json!({"version":2,"fingerprint":"stale","search":search}),
                MB_OCCURRENCE_RUNTIME_TTL,
            )
            .unwrap();
        let legacy_runtime = cached_musicbrainz_occurrence_runtime_variants(&store, &source);
        assert_eq!(legacy_runtime.len(), 1);
        assert_eq!(
            legacy_runtime[0].release_artist_credit,
            "Bulgarian State Television Female Choir"
        );
        assert_eq!(legacy_runtime[0].duration_seconds, 175.0);

        let mut different_version = search.clone();
        different_version["recordings"][0]["releases"][0]["media"][0]["tracks"][0]["title"] =
            json!("Pilentze Pee (Live)");
        different_version["recordings"][0]["title"] = json!("Pilentze Pee (Live)");
        store
            .set_provider_cache(
                MB_OCCURRENCE_RUNTIME_CACHE,
                &source.id,
                &json!({"version":2,"fingerprint":mb_occurrence_runtime_fingerprint(&source, &occurrence),"queryIsrc":second_isrc,"search":different_version}),
                MB_OCCURRENCE_RUNTIME_TTL,
            )
            .unwrap();
        assert!(cached_musicbrainz_occurrence_runtime_variants(&store, &source).is_empty());
        store
            .set_provider_cache(
                MB_OCCURRENCE_RUNTIME_CACHE,
                &source.id,
                &json!({"version":2,"fingerprint":mb_occurrence_runtime_fingerprint(&source, &occurrence),"queryIsrc":second_isrc,"search":search}),
                MB_OCCURRENCE_RUNTIME_TTL,
            )
            .unwrap();

        let candidate = crate::models::Candidate {
            provider: "monochrome".into(),
            id: "monochrome:recording".into(),
            title: "Bulgarian State Television Female Choir - Pilentze Pee (The Evening Gathering)"
                .into(),
            duration_seconds: Some(175.5),
            source_metadata: Some(json!({
                "provider":"monochrome",
                "trackId":"recording",
                "track":"Pilentze Pee (The Evening Gathering)",
                "catalogTitle":"Pilentze Pee (The Evening Gathering)",
                "artistNames":["Bulgarian State Television Female Choir"],
                "duration":175.5,
                "isrc":isrc
            })),
            ..crate::models::Candidate::default()
        };
        let assessment = crate::recording_match_context::assess(
            &store,
            &source,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_eq!(
            assessment.relationship,
            crate::recording_match::MatchRelationship::Probable
        );
        assert_eq!(
            assessment.action,
            crate::recording_match::MatchAction::Accept
        );
        let evidence = assessment.evidence.unwrap();
        assert_eq!(
            evidence.dimensions.runtime,
            crate::recording_match::Comparison::Agree
        );
        let credit = evidence.recording_scoped_credit_variant.as_ref().unwrap();
        assert_eq!(credit.recording_id, recording_id);
        assert_eq!(credit.release_id, alternate_release_id);
        assert_eq!(
            credit.requested_artist_credit,
            "Bulgarian State Radio and Television Female Vocal Choir"
        );
        assert_eq!(
            credit.release_artist_names,
            vec!["Bulgarian State Television Female Choir".to_owned()]
        );
        assert_eq!(credit.release_artist_ids, vec![source_artist_id.to_owned()]);
        let membership = evidence.recording_scoped_isrc_membership.as_ref().unwrap();
        assert_eq!(membership.recording_id, recording_id);
        assert_eq!(membership.release_id, alternate_release_id);
        assert_eq!(membership.requested_isrc, None);
        assert_eq!(membership.selected_isrc, isrc);
        assert_eq!(membership.source_isrcs, vec![second_isrc, isrc]);
        assert_eq!(
            membership.authority,
            crate::recording_match::RecordingScopedIsrcAuthority::VerifiedMusicBrainzOccurrence
        );

        let mut wrong_code = candidate.clone();
        wrong_code.source_metadata.as_mut().unwrap()["isrc"] = json!("USZZZ2400001");
        let denied = crate::recording_match_context::assess(
            &store,
            &source,
            &wrong_code,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_ne!(denied.action, crate::recording_match::MatchAction::Accept);
        assert!(denied
            .evidence
            .as_ref()
            .unwrap()
            .recording_scoped_isrc_membership
            .is_none());

        let mut manually_cleared = source.clone();
        manually_cleared.provenance["manualFields"] = json!({"isrc":true});
        let denied = crate::recording_match_context::assess(
            &store,
            &manually_cleared,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
        );
        assert_ne!(denied.action, crate::recording_match::MatchAction::Accept);
        assert!(denied
            .evidence
            .as_ref()
            .unwrap()
            .recording_scoped_isrc_membership
            .is_none());

        let mut manual_isrc_clear = source.clone();
        manual_isrc_clear.provenance["manualFields"] = json!({"isrc":true});
        let mut hidden_isrc_alias = source.clone();
        hidden_isrc_alias.isrc = Some(isrc.to_owned());
        let manually_cleared = crate::recording_match_context::assess_with_aliases(
            &store,
            &manual_isrc_clear,
            &candidate,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchStage::Resolved,
            std::slice::from_ref(&hidden_isrc_alias),
            std::slice::from_ref(&hidden_isrc_alias),
            Some(isrc),
        );
        assert_ne!(
            manually_cleared.dimensions.isrc,
            crate::recording_match::Comparison::Agree,
            "an explicit manual clear cannot be filled from an alias or a cached occurrence"
        );
        assert!(manually_cleared
            .evidence
            .as_ref()
            .unwrap()
            .recording_scoped_isrc_membership
            .is_none());
    }

    #[test]
    fn independent_isrc_alias_proof_requires_every_identity_field() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("evidence.sqlite")).unwrap();
        let track_id = "bbbbbbbbbbbbbbbbbbbbbb";
        let artists =
            json!([{"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"Provider Name"}]);
        let mut track = Track {
            id: format!("spotify:{track_id}"),
            provider: "spotify".into(),
            provider_id: track_id.into(),
            source_url: format!("https://open.spotify.com/track/{track_id}"),
            title: "Song".into(),
            artist: "Provider Name".into(),
            duration_seconds: Some(180.0),
            isrc: Some("USABC2600001".into()),
            provenance: json!({"catalog":{"verified":true,"provider":"spotify","providerId":track_id,"artistCount":1,"artists":artists},"remoteCatalog":{"trackId":track_id,"title":"Song","artist":"Provider Name","durationSeconds":180.0,"isrc":"USABC2600001","artistCount":1,"artistRefs":artists}}),
            ..Track::default()
        };
        let recording_id = "00000000-0000-4000-8000-000000000001";
        let artist_id = "00000000-0000-4000-8000-000000000002";
        let mut recording = json!({"id":recording_id,"title":"Song","length":180000,"isrcs":["USABC2600001"],"artist-credit":[{"name":"Canonical Name","joinphrase":"","artist":{"id":artist_id,"name":"Canonical Name"}}]});
        let profile = json!({"id":artist_id,"name":"Canonical Name","aliases":[{"name":"Provider Name"}],"relations":[{"url":{"resource":"https://open.spotify.com/artist/aaaaaaaaaaaaaaaaaaaaaa"}}]});
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                recording_id,
                &recording,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                artist_id,
                &profile,
            )
            .unwrap();
        let release_search = json!({
            "count":1,
            "recordings":[{
                "id":recording_id,
                "releases":[{
                    "id":"00000000-0000-4000-8000-000000000004",
                    "artist-credit":[{"name":"Provider Name","artist":{"id":artist_id}}],
                    "media":[{"track":[{"id":"00000000-0000-4000-8000-000000000005","recording":{"id":recording_id},"title":"Song","length":175000}]}]
                }]
            }]
        });
        let variants =
            isrc_authority_variants(&store, &track, recording_id, Some(&release_search)).unwrap();
        assert_eq!(variants[0].artist, "Canonical Name");
        let mut restored = variants[0].clone();
        restored.artist = track.artist.clone();
        assert_eq!(restored, track);

        store.save_track(&track).unwrap();
        store.set_retained_provider_cache(ISRC_BINDING,&track.provider_id,&json!({"version":1,"fingerprint":aliases::spotify_artist_credit_fingerprint(&track),"recordingId":recording_id,"search":release_search})).unwrap();
        let runtime = cached_isrc_authority_runtime_variants(&store, &track);
        assert_eq!(runtime.len(), 1);
        assert_eq!(runtime[0].recording_id, recording_id);
        assert_eq!(
            runtime[0].release_id,
            "00000000-0000-4000-8000-000000000004"
        );
        assert_eq!(runtime[0].duration_seconds, 175.0);

        let mut wrong_occurrence = release_search.clone();
        wrong_occurrence["recordings"][0]["releases"][0]["media"][0]["track"][0]["recording"]
            ["id"] = json!("00000000-0000-4000-8000-000000000006");
        store.set_retained_provider_cache(ISRC_BINDING,&track.provider_id,&json!({"version":1,"fingerprint":aliases::spotify_artist_credit_fingerprint(&track),"recordingId":recording_id,"search":wrong_occurrence})).unwrap();
        assert!(cached_isrc_authority_runtime_variants(&store, &track).is_empty());

        let mut ambiguous = release_search.clone();
        ambiguous["count"] = json!(2);
        ambiguous["recordings"]
            .as_array_mut()
            .unwrap()
            .push(release_search["recordings"][0].clone());
        store.set_retained_provider_cache(ISRC_BINDING,&track.provider_id,&json!({"version":1,"fingerprint":aliases::spotify_artist_credit_fingerprint(&track),"recordingId":recording_id,"search":ambiguous})).unwrap();
        assert!(cached_isrc_authority_runtime_variants(&store, &track).is_empty());

        recording["isrcs"] = json!(["CH1648980010", "USABC2600001"]);
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                recording_id,
                &recording,
            )
            .unwrap();
        assert!(
            isrc_authority_variants(&store, &track, recording_id, Some(&release_search)).is_some()
        );
        recording["isrcs"] = json!(["USABC2600002"]);
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                recording_id,
                &recording,
            )
            .unwrap();
        assert!(
            isrc_authority_variants(&store, &track, recording_id, Some(&release_search)).is_none()
        );
        track.version = Some("Live".into());
        assert!(
            isrc_authority_variants(&store, &track, recording_id, Some(&release_search)).is_none()
        );
    }
}

/// Complete ordered credit choices for the proof-checked retained binding.
pub(crate) fn cached_isrc_authority_credit_choices(
    store: &Store,
    track: &Track,
) -> Option<Vec<(Vec<String>, String)>> {
    if cached_isrc_authority_alias_tracks(store, track).is_empty() {
        return None;
    }
    let hydrated = crate::remote_playlists::read_alias_hydrated_track(store, track)
        .unwrap_or_else(|| track.clone());
    let binding = store
        .get_provider_cache(ISRC_BINDING, &track.provider_id)
        .ok()??;
    let choices = isrc_authority_choices(
        store,
        &hydrated,
        binding["recordingId"].as_str()?,
        Some(&binding["search"]),
    )?;
    Some(
        choices
            .into_iter()
            .map(|(names, join)| (names.into_iter().collect(), join))
            .collect(),
    )
}

#[cfg(test)]
mod live_qualification {
    use super::*;
    /// Explicit opt-in network qualification, entirely isolated from app state.
    /// This proves artist authority aliases only, never a Spotify recording match.
    #[tokio::test]
    #[ignore = "public metadata HTTP; run explicitly with --ignored --nocapture"]
    async fn public_artist_authority_aliases_are_independently_corroborated() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("isolated.sqlite")).unwrap();
        let id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        hydrate_artist(
            &store,
            id,
            aliases::spotify_recording_credit_profile_cache_namespace(),
        )
        .await
        .expect("public metadata available");
        let profile = store
            .get_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                id,
            )
            .unwrap()
            .unwrap();
        let names = verified_wikidata_alias_names(&store, id, &profile);
        assert!(names.iter().any(|name| name == "Lakis Chalkias"));
        assert!(names.iter().any(|name| name == "Lakis Halkias"));
        println!(
            "Verified exact artist authority aliases from independent shared identifiers; {} names; no Spotify recording proof asserted",
            names.len()
        );
    }
}

const PROVIDER_ARTIST: &str = "artists.alias-discovery.provider-artist.v1";
pub(crate) fn cached_spotify_musicbrainz_artist(store: &Store, artist_id: &str) -> Option<String> {
    if artist_id.len() != 22 || !artist_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let binding = store
        .get_provider_cache(PROVIDER_ARTIST, &format!("spotify:{artist_id}"))
        .ok()??;
    let id = binding["musicbrainzArtistId"].as_str()?;
    let resource = format!("https://open.spotify.com/artist/{artist_id}");
    if binding["version"] != 1
        || binding["provider"].as_str() != Some("spotify")
        || binding["providerArtistId"].as_str() != Some(artist_id)
        || binding["profile"]["id"].as_str() != Some(id)
        || !aliases::valid_musicbrainz_recording_id(id)
        || !exact_url_artist_ids(&binding["lookup"], &resource)
            .is_some_and(|ids| ids.len() == 1 && ids.contains(id))
        || !exact_spotify_artist_profile(&binding["profile"], artist_id)
    {
        return None;
    }
    Some(id.to_owned())
}
fn exact_spotify_artist_profile(profile: &Value, artist_id: &str) -> bool {
    let expected = format!("https://open.spotify.com/artist/{artist_id}");
    let urls = relation_urls(profile)
        .filter(|raw| {
            url::Url::parse(raw).ok().is_some_and(|url| {
                url.host_str() == Some("open.spotify.com") && url.path().starts_with("/artist/")
            })
        })
        .collect::<BTreeSet<_>>();
    urls.len() == 1 && urls.contains(expected.as_str())
}
fn exact_url_artist_ids(lookup: &Value, resource: &str) -> Option<BTreeSet<String>> {
    if lookup["resource"].as_str() != Some(resource) {
        return None;
    }
    let mut ids = BTreeSet::new();
    for relation in lookup["relations"].as_array()? {
        if relation["target-type"].as_str() != Some("artist") {
            continue;
        }
        let id = relation["artist"]["id"].as_str()?;
        if !aliases::valid_musicbrainz_recording_id(id) {
            return None;
        }
        ids.insert(id.to_owned());
    }
    Some(ids)
}
async fn discover_spotify_artist(store: &Store, artist_id: &str) -> Result<bool, String> {
    type ArtistLock = tokio::sync::Mutex<()>;
    static LOCKS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<ArtistLock>>>,
    > = OnceLock::new();
    let key = format!("{store:p}:spotify:{artist_id}");
    let lock = {
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Artist lookup lock unavailable")?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
            lock
        } else {
            let lock = std::sync::Arc::new(ArtistLock::new(()));
            locks.insert(key, std::sync::Arc::downgrade(&lock));
            lock
        }
    };
    let _guard = lock.lock().await;
    discover_spotify_artist_inner(store, artist_id).await
}
async fn discover_spotify_artist_inner(store: &Store, artist_id: &str) -> Result<bool, String> {
    let key = format!("spotify:{artist_id}");
    if let Some(binding) = store.get_provider_cache(PROVIDER_ARTIST, &key)? {
        let fresh = binding["fetchedAt"]
            .as_i64()
            .is_some_and(|at| chrono::Utc::now().timestamp().saturating_sub(at) < TTL as i64);
        let resource = format!("https://open.spotify.com/artist/{artist_id}");
        let id = binding["musicbrainzArtistId"].as_str().unwrap_or_default();
        if fresh
            && binding["version"] == 1
            && binding["providerArtistId"].as_str() == Some(artist_id)
            && binding["profile"]["id"].as_str() == Some(id)
            && exact_url_artist_ids(&binding["lookup"], &resource)
                .is_some_and(|ids| ids.len() == 1 && ids.contains(id))
            && exact_spotify_artist_profile(&binding["profile"], artist_id)
        {
            return Ok(true);
        }
    }
    if store
        .get_provider_cache("artists.alias-discovery.unlinked-provider-artist.v1", &key)?
        .is_some()
    {
        return Ok(false);
    }
    let resource = format!("https://open.spotify.com/artist/{artist_id}");
    let lookup = match fetch(
        "https://musicbrainz.org/ws/2/url",
        &[
            ("resource", resource.clone()),
            ("inc", "artist-rels".into()),
            ("fmt", "json".into()),
        ],
    )
    .await
    {
        Ok(value) => value,
        Err(error) if error == "Artist discovery HTTP 404" => {
            store.set_retained_provider_cache(
                PROVIDER_ARTIST,
                &key,
                &json!({"version":1,"unlinked":true}),
            )?;
            store.set_provider_cache(
                "artists.alias-discovery.unlinked-provider-artist.v1",
                &key,
                &json!({"unlinked":true}),
                24 * 60 * 60,
            )?;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let Some(ids) = exact_url_artist_ids(&lookup, &resource) else {
        return Ok(false);
    };
    if ids.len() != 1 {
        if ids.len() > 1 {
            store.set_retained_provider_cache(
                PROVIDER_ARTIST,
                &key,
                &json!({"version":1,"ambiguous":true,"lookup":lookup}),
            )?;
        } else {
            store.set_retained_provider_cache(
                PROVIDER_ARTIST,
                &key,
                &json!({"version":1,"unlinked":true,"lookup":lookup}),
            )?;
            store.set_provider_cache(
                "artists.alias-discovery.unlinked-provider-artist.v1",
                &key,
                &json!({"unlinked":true}),
                24 * 60 * 60,
            )?;
        }
        return Ok(false);
    }
    let id = ids.iter().next().expect("one artist");
    let profile = mb(
        store,
        "artists.profile.musicbrainz",
        id,
        "artist",
        "aliases+url-rels",
    )
    .await?;
    if !exact_spotify_artist_profile(&profile, artist_id) {
        store.set_retained_provider_cache(
            PROVIDER_ARTIST,
            &key,
            &json!({"version":1,"conflicting":true,"lookup":lookup,"profile":profile}),
        )?;
        return Ok(false);
    }
    store.set_retained_provider_cache(PROVIDER_ARTIST,&key,&json!({"version":1,"provider":"spotify","providerArtistId":artist_id,"musicbrainzArtistId":id,"lookup":lookup,"profile":profile,"fetchedAt":chrono::Utc::now().timestamp()}))?;
    // MusicBrainz aliases are already authorized by the exact provider ID; WD
    // adds optional independently corroborated spellings to this reusable ID.
    let _ = hydrate_artist(store, id, "artists.profile.musicbrainz").await;
    Ok(true)
}

#[cfg(test)]
mod provider_identity_tests {
    use super::*;
    #[test]
    fn cached_spotify_artist_requires_consistent_verified_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("artist.sqlite")).unwrap();
        let spotify = "aaaaaaaaaaaaaaaaaaaaaa";
        let id = "00000000-0000-4000-8000-000000000002";
        assert!(cached_spotify_musicbrainz_artist(&store, spotify).is_none());
        let resource = format!("https://open.spotify.com/artist/{spotify}");
        let mut binding = json!({"version":1,"provider":"spotify","providerArtistId":spotify,"musicbrainzArtistId":id,"lookup":{"resource":resource,"relations":[{"target-type":"artist","artist":{"id":id}}]},"profile":{"id":id,"relations":[{"url":{"resource":resource}}]}});
        store
            .set_retained_provider_cache(PROVIDER_ARTIST, &format!("spotify:{spotify}"), &binding)
            .unwrap();
        assert_eq!(
            cached_spotify_musicbrainz_artist(&store, spotify).as_deref(),
            Some(id)
        );
        binding["profile"]["id"] = json!("00000000-0000-4000-8000-000000000003");
        store
            .set_retained_provider_cache(PROVIDER_ARTIST, &format!("spotify:{spotify}"), &binding)
            .unwrap();
        assert!(cached_spotify_musicbrainz_artist(&store, spotify).is_none());
    }
    #[test]
    fn url_lookup_requires_exact_resource_and_unique_identity() {
        let resource = "https://open.spotify.com/artist/aaaaaaaaaaaaaaaaaaaaaa";
        let id = "00000000-0000-4000-8000-000000000002";
        let mut lookup =
            json!({"resource":resource,"relations":[{"target-type":"artist","artist":{"id":id}}]});
        assert_eq!(exact_url_artist_ids(&lookup, resource).unwrap().len(), 1);
        assert!(exact_url_artist_ids(
            &lookup,
            "https://open.spotify.com/artist/bbbbbbbbbbbbbbbbbbbbbb"
        )
        .is_none());
        lookup["relations"].as_array_mut().unwrap().push(
            json!({"target-type":"artist","artist":{"id":"00000000-0000-4000-8000-000000000003"}}),
        );
        assert_eq!(exact_url_artist_ids(&lookup, resource).unwrap().len(), 2);
        let mut profile = json!({"relations":[{"url":{"resource":resource}}]});
        assert!(exact_spotify_artist_profile(
            &profile,
            "aaaaaaaaaaaaaaaaaaaaaa"
        ));
        profile["relations"].as_array_mut().unwrap().push(
            json!({"url":{"resource":"https://open.spotify.com/artist/bbbbbbbbbbbbbbbbbbbbbb"}}),
        );
        assert!(!exact_spotify_artist_profile(
            &profile,
            "aaaaaaaaaaaaaaaaaaaaaa"
        ));
    }
}

const GENERIC_ISRC_BINDING: &str = "artists.alias-discovery.generic-isrc-authority.v1";
fn generic_source_fingerprint(track: &Track) -> Option<Value> {
    if !matches!(
        track.provider.as_str(),
        "beatport" | "traxsource" | "local" | "radio_song"
    ) || track.id.is_empty()
        || track.title.trim().is_empty()
        || track.artist.trim().is_empty()
        || track.version.is_some()
        || track.playback_range.is_some()
    {
        return None;
    }
    let identity = crate::recording_identity::RecordingIdentity::from_track(track);
    if identity.identifier_conflict {
        return None;
    }
    let isrc = identity.isrc?;
    let duration = track.duration_seconds;
    if duration
        .is_some_and(|duration| !duration.is_finite() || duration <= 0.0 || duration > 86400.0)
    {
        return None;
    }
    Some(
        json!({"id":track.id,"provider":track.provider,"providerId":track.provider_id,"sourceUrl":track.source_url,"title":track.title,"artist":track.artist,"version":track.version,"album":track.album,"albumArtist":track.album_artist,"duration":duration,"isrc":isrc,"playbackRange":track.playback_range,"remixers":track.remixers}),
    )
}
fn saved_generic_fingerprint(store: &Store, track: &Track) -> Option<Value> {
    let fingerprint = generic_source_fingerprint(track)?;
    let saved = if track.provider == "radio_song" {
        store.get_track(&track.id).ok()??
    } else {
        store.get_library_track(&track.id).ok()??
    };
    let saved_fingerprint = generic_source_fingerprint(&saved)?;
    if saved_fingerprint == fingerprint {
        return Some(fingerprint);
    }
    // A playback-only duration overlay must itself be bound to retained proof.
    if saved.duration_seconds.is_some()
        || saved.provenance["manualFields"]["durationSeconds"] == true
    {
        return None;
    }
    let mut original = track.clone();
    original.duration_seconds = saved.duration_seconds;
    if generic_source_fingerprint(&original).as_ref() != Some(&saved_fingerprint) {
        return None;
    }
    let binding = store
        .get_provider_cache(GENERIC_ISRC_BINDING, &track.id)
        .ok()??;
    if binding["version"] != 1 || binding["fingerprint"] != saved_fingerprint {
        return None;
    }
    let raw = store
        .get_provider_cache(
            aliases::spotify_recording_credit_raw_cache_namespace(),
            binding["recordingId"].as_str()?,
        )
        .ok()??;
    (track.duration_seconds == Some(raw["length"].as_u64()? as f64 / 1000.0))
        .then_some(saved_fingerprint)
}
fn full_credit_choices_match(choices: &[(Vec<String>, String)], artist: &str) -> bool {
    let artist = crate::matching::normalize(artist);
    if artist.is_empty() || choices.is_empty() {
        return false;
    }
    for comma_separated in [false, true] {
        let mut offsets = BTreeSet::from([0usize]);
        for (index, (names, join)) in choices.iter().enumerate() {
            let mut next = BTreeSet::new();
            for offset in offsets {
                for name in names {
                    let join = if comma_separated {
                        if index + 1 == choices.len() {
                            ""
                        } else {
                            ", "
                        }
                    } else {
                        join.as_str()
                    };
                    let fragment = crate::matching::normalize(&format!("{name}{join}"));
                    if fragment.is_empty() {
                        continue;
                    }
                    if let Some(tail) = artist[offset..].strip_prefix(&fragment) {
                        if tail.is_empty() {
                            next.insert(artist.len());
                        } else if let Some(tail) = tail.strip_prefix(' ') {
                            next.insert(artist.len() - tail.len());
                        }
                    }
                }
            }
            offsets = next;
        }
        if offsets.contains(&artist.len()) {
            return true;
        }
    }
    false
}
fn generic_recording_choices(
    store: &Store,
    track: &Track,
    recording_id: &str,
    release_search: Option<&Value>,
) -> Option<Vec<(Vec<String>, String)>> {
    generic_source_fingerprint(track)?;
    if !aliases::valid_musicbrainz_recording_id(recording_id) {
        return None;
    }
    let recording = store
        .get_provider_cache(
            aliases::spotify_recording_credit_raw_cache_namespace(),
            recording_id,
        )
        .ok()??;
    let duration = track.duration_seconds.map(|d| (d * 1000.0).round() as u64);
    if recording["id"].as_str() != Some(recording_id)
        || crate::matching::normalize(recording["title"].as_str()?)
            != crate::matching::normalize(&track.title)
        || recording["length"].as_u64()? == 0
        || duration.is_some_and(|duration| {
            recording["length"]
                .as_u64()
                .is_none_or(|length| length.abs_diff(duration) > 5000)
        })
    {
        return None;
    }
    let isrcs = recording["isrcs"]
        .as_array()?
        .iter()
        .map(|value| crate::recording_identity::normalize_isrc(value.as_str()?))
        .collect::<Option<BTreeSet<_>>>()?;
    if !isrcs.contains(&crate::recording_identity::RecordingIdentity::from_track(track).isrc?) {
        return None;
    }
    let credits = recording["artist-credit"].as_array()?;
    if credits.is_empty() || credits.len() > 16 {
        return None;
    }
    let mut choices = Vec::new();
    for (index, credit) in credits.iter().enumerate() {
        let id = credit["artist"]["id"].as_str()?;
        if !aliases::valid_musicbrainz_recording_id(id) {
            return None;
        }
        let profile = store
            .get_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                id,
            )
            .ok()??;
        if profile["id"].as_str() != Some(id) || profile["name"] != credit["artist"]["name"] {
            return None;
        }
        let canonical = profile["name"].as_str()?.trim();
        if canonical.is_empty() || canonical.len() > 256 {
            return None;
        }
        let mut names = BTreeSet::from([canonical.to_owned()]);
        for name in profile["aliases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|alias| alias["type"] != "Search hint")
            .filter_map(|alias| alias["name"].as_str())
            .filter(|name| !name.trim().is_empty() && name.len() <= 256)
        {
            names.insert(name.to_owned());
        }
        names.extend(verified_wikidata_alias_names(store, id, &profile));
        let join = aliases::verified_musicbrainz_joinphrase(credit, index + 1 == credits.len())?;
        if join.len() > 32 {
            return None;
        }
        let mut names = names.into_iter().collect::<Vec<_>>();
        names.retain(|name| name != canonical);
        names.insert(0, canonical.to_owned());
        choices.push((names, join.to_owned()));
    }
    let cached_binding = store
        .get_provider_cache(GENERIC_ISRC_BINDING, &track.id)
        .ok()
        .flatten();
    let rendered_release_names = if let Some(search) = release_search {
        release_credit_renderings(search, recording_id, credits)
    } else if let Some(binding) = cached_binding.as_ref().filter(|binding| {
        binding["version"] == 1
            && binding["fingerprint"] == generic_source_fingerprint(track).unwrap_or(Value::Null)
            && binding["recordingId"].as_str() == Some(recording_id)
    }) {
        release_credit_renderings(&binding["search"], recording_id, credits)
    } else {
        Vec::new()
    };
    for (choice, names) in choices.iter_mut().zip(rendered_release_names) {
        choice.0.extend(names);
    }
    full_credit_choices_match(&choices, &track.artist).then_some(choices)
}
fn generic_supported_recording(store: &Store, track: &Track, search: &Value) -> Option<String> {
    let hits = search["recordings"].as_array()?;
    if search["count"].as_u64() != Some(hits.len() as u64) || hits.len() > 5 {
        return None;
    }
    let mut supported = BTreeSet::new();
    for hit in hits {
        let id = hit["id"].as_str()?;
        if !aliases::valid_musicbrainz_recording_id(id) {
            return None;
        }
        // Missing raw evidence is an incomplete lookup, never proof uniqueness.
        let raw = store
            .get_provider_cache(aliases::spotify_recording_credit_raw_cache_namespace(), id)
            .ok()??;
        if raw["id"].as_str() != Some(id)
            || raw["title"].as_str().is_none()
            || raw["length"].as_u64().is_none()
        {
            return None;
        }
        let credits = raw["artist-credit"].as_array()?;
        if credits.is_empty() || credits.len() > 16 {
            return None;
        }
        for (index, credit) in credits.iter().enumerate() {
            let artist_id = credit["artist"]["id"].as_str()?;
            if !aliases::valid_musicbrainz_recording_id(artist_id)
                || aliases::verified_musicbrainz_joinphrase(credit, index + 1 == credits.len())
                    .is_none()
            {
                return None;
            }
            let profile = store
                .get_provider_cache(
                    aliases::spotify_recording_credit_profile_cache_namespace(),
                    artist_id,
                )
                .ok()??;
            if profile["id"].as_str() != Some(artist_id)
                || profile["name"].as_str().is_none()
                || profile["name"] != credit["artist"]["name"]
            {
                return None;
            }
        }
        raw["isrcs"]
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?;
        if generic_recording_choices(store, track, id, Some(search)).is_some() {
            supported.insert(id.to_owned());
        }
    }
    (supported.len() == 1).then(|| supported.into_iter().next().expect("one recording"))
}
pub(crate) fn cached_generic_isrc_authority_credit_choices(
    store: &Store,
    track: &Track,
) -> Option<Vec<(Vec<String>, String)>> {
    let fingerprint = saved_generic_fingerprint(store, track)?;
    let binding = store
        .get_provider_cache(GENERIC_ISRC_BINDING, &track.id)
        .ok()??;
    if binding["version"] != 1 || binding["fingerprint"] != fingerprint {
        return None;
    }
    let id = generic_supported_recording(store, track, &binding["search"])?;
    if binding["recordingId"].as_str() != Some(id.as_str()) {
        return None;
    }
    generic_recording_choices(store, track, &id, Some(&binding["search"]))
}
/// Revalidate retained identifier evidence against the unchanged saved recording.
/// This enriches only the playback copy, never user tags or history.
pub(crate) fn cached_identifier_track(store: &Store, track: &Track) -> Option<Track> {
    cached_generic_isrc_authority_credit_choices(store, track)?;
    let binding = store
        .get_provider_cache(GENERIC_ISRC_BINDING, &track.id)
        .ok()??;
    let id = binding["recordingId"].as_str()?;
    let raw = store
        .get_provider_cache(aliases::spotify_recording_credit_raw_cache_namespace(), id)
        .ok()??;
    let mut enriched = track.clone();
    crate::recording_identity::hydrate_identifiers(&mut enriched);
    if enriched.duration_seconds.is_none()
        && enriched.provenance["manualFields"]["durationSeconds"] != true
    {
        let duration = raw["length"].as_u64()? as f64 / 1000.0;
        if !(1.0..=86400.0).contains(&duration) {
            return None;
        }
        enriched.duration_seconds = Some(duration);
    }
    Some(enriched)
}

async fn discover_generic_isrc(store: &Store, track: &Track) -> Result<(), String> {
    let Some(fingerprint) = saved_generic_fingerprint(store, track) else {
        return Ok(());
    };
    if let Some(binding) = store.get_provider_cache(GENERIC_ISRC_BINDING, &track.id)? {
        if binding["fetchedAt"]
            .as_i64()
            .is_some_and(|at| chrono::Utc::now().timestamp().saturating_sub(at) < TTL as i64)
            && cached_generic_isrc_authority_credit_choices(store, track).is_some()
        {
            return Ok(());
        }
    }
    let search = fetch(
        "https://musicbrainz.org/ws/2/recording/",
        &[
            ("fmt", "json".into()),
            (
                "query",
                format!(
                    "isrc:{}",
                    crate::recording_identity::RecordingIdentity::from_track(track)
                        .isrc
                        .expect("validated ISRC")
                ),
            ),
            ("limit", "5".into()),
        ],
    )
    .await?;
    let hits = search["recordings"]
        .as_array()
        .ok_or("Incomplete recording search response")?;
    if search["count"].as_u64() != Some(hits.len() as u64) || hits.len() > 5 {
        return Ok(());
    }
    for hit in hits {
        let id = hit["id"]
            .as_str()
            .filter(|id| aliases::valid_musicbrainz_recording_id(id))
            .ok_or("Incomplete recording identity")?;
        let recording = mb(
            store,
            aliases::spotify_recording_credit_raw_cache_namespace(),
            id,
            "recording",
            "artist-credits+isrcs",
        )
        .await?;
        let mut artists = BTreeSet::new();
        collect_artist_ids(&recording, &mut artists);
        for artist in artists.into_iter().take(16) {
            hydrate_artist(
                store,
                &artist,
                aliases::spotify_recording_credit_profile_cache_namespace(),
            )
            .await?;
        }
    }
    // Recheck the saved input after network work to avoid authorizing stale edits.
    if saved_generic_fingerprint(store, track).as_ref() != Some(&fingerprint) {
        return Ok(());
    }
    if let Some(id) = generic_supported_recording(store, track, &search) {
        store.set_retained_provider_cache(GENERIC_ISRC_BINDING,&track.id,&json!({"version":1,"fingerprint":fingerprint,"recordingId":id,"search":search,"fetchedAt":chrono::Utc::now().timestamp()}))?;
    } else if !hits.is_empty() {
        store.set_retained_provider_cache(
            GENERIC_ISRC_BINDING,
            &track.id,
            &json!({"version":1,"unresolved":true,"search":search}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod generic_isrc_tests {
    use super::*;
    const RECORDING: &str = "00000000-0000-4000-8000-000000000001";
    fn fixture(store: &Store, provider: &str) -> Track {
        let track = Track {
            id: format!("{provider}:123"),
            provider: provider.into(),
            provider_id: "123".into(),
            source_url: format!("https://example.test/{provider}/123"),
            title: "A Song".into(),
            artist: "Source A feat. Guest".into(),
            duration_seconds: if provider == "radio_song" {
                None
            } else {
                Some(180.0)
            },
            isrc: Some("USABC2600001".into()),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        let a = "00000000-0000-4000-8000-000000000002";
        let b = "00000000-0000-4000-8000-000000000003";
        let recording = json!({"id":RECORDING,"title":"A Song","length":180000,"isrcs":["USABC2600001"],"artist-credit":[{"name":"Canonical A","joinphrase":" feat. ","artist":{"id":a,"name":"Canonical A"}},{"name":"Guest","joinphrase":"","artist":{"id":b,"name":"Guest"}}]});
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                RECORDING,
                &recording,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                a,
                &json!({"id":a,"name":"Canonical A","aliases":[{"name":"Source A"}]}),
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                b,
                &json!({"id":b,"name":"Guest","aliases":[]}),
            )
            .unwrap();
        store.set_retained_provider_cache(GENERIC_ISRC_BINDING,&track.id,&json!({"version":1,"fingerprint":generic_source_fingerprint(&track),"recordingId":RECORDING,"search":{"count":1,"recordings":[{"id":RECORDING}]},"fetchedAt":chrono::Utc::now().timestamp()})).unwrap();
        track
    }
    #[test]
    fn radio_isrc_can_establish_missing_duration_without_modifying_saved_tags() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("radio-isrc.sqlite")).unwrap();
        let mut track = fixture(&store, "radio_song");
        track.duration_seconds = None;
        store.save_track(&track).unwrap();
        store.set_retained_provider_cache(GENERIC_ISRC_BINDING,&track.id,&json!({"version":1,"fingerprint":generic_source_fingerprint(&track),"recordingId":RECORDING,"search":{"count":1,"recordings":[{"id":RECORDING}]}})).unwrap();
        let enriched = cached_identifier_track(&store, &track).unwrap();
        assert_eq!(enriched.duration_seconds, Some(180.0));
        assert_eq!(
            store
                .get_library_track(&track.id)
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
        let mut wrong = track.clone();
        wrong.isrc = Some("USABC2600002".into());
        assert!(cached_identifier_track(&store, &wrong).is_none());
    }

    #[test]
    fn imports_share_recording_scoped_alias_proof_without_spotify_ids() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("generic.sqlite")).unwrap();
        for provider in ["beatport", "traxsource", "local"] {
            let track = fixture(&store, provider);
            let choices = cached_generic_isrc_authority_credit_choices(&store, &track).unwrap();
            assert_eq!(choices.len(), 2);
            assert!(choices[0].0.iter().any(|name| name == "Canonical A"));
            assert!(full_credit_choices_match(
                &choices,
                "Canonical A feat. Guest"
            ));
            assert!(!full_credit_choices_match(&choices, "Canonical A"));
            let mut changed = track.clone();
            changed.artist = "Source A".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.artist = "Source A feat. Other".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.title = "Different Song".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.duration_seconds = Some(200.0);
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.isrc = Some("USABC2600002".into());
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.version = Some("Live".into());
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.playback_range = Some(crate::models::PlaybackRange {
                parent_track_id: "parent".into(),
                chapter_id: "chapter".into(),
                source_url: "https://example.test/parent".into(),
                start_time: 0.0,
                end_time: 180.0,
                parent_title: "Parent".into(),
                parent_duration_seconds: Some(180.0),
            });
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.artist = "Guest feat. Source A".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.isrc = None;
            assert!(generic_recording_choices(&store, &changed, RECORDING, None).is_none());
            changed = track.clone();
            changed.title = "Manual edit".into();
            store.save_track(&changed).unwrap();
            assert!(cached_generic_isrc_authority_credit_choices(&store, &track).is_none());
            assert!(cached_generic_isrc_authority_credit_choices(&store, &changed).is_none());
        }
    }

    #[test]
    fn initial_generic_isrc_check_can_use_exact_release_credit_rendering() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("generic-release-credit.sqlite")).unwrap();
        let track = fixture(&store, "local");
        let a = "00000000-0000-4000-8000-000000000002";
        let b = "00000000-0000-4000-8000-000000000003";
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_profile_cache_namespace(),
                a,
                &json!({"id":a,"name":"Canonical A","aliases":[]}),
            )
            .unwrap();
        let search = json!({
            "count":1,
            "recordings":[{
                "id":RECORDING,
                "releases":[{
                    "id":"00000000-0000-4000-8000-000000000004",
                    "artist-credit":[
                        {"name":"Source A","artist":{"id":a}},
                        {"name":"Guest","artist":{"id":b}}
                    ],
                    "media":[{"track":[{"id":"00000000-0000-4000-8000-000000000005"}]}]
                }]
            }]
        });
        let choices = generic_recording_choices(&store, &track, RECORDING, Some(&search)).unwrap();
        assert!(full_credit_choices_match(&choices, "Source A feat. Guest"));
        assert!(!full_credit_choices_match(&choices, "Source A feat. Other"));
    }

    #[tokio::test]
    async fn cached_generic_discovery_reuses_proof_without_http() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("cached.sqlite")).unwrap();
        let track = fixture(&store, "traxsource");
        let report = discover_verified_artist_aliases(&store, &track).await;
        assert!(report.result.is_ok());
        assert_eq!(report.stats.requests, 0);
        let context = aliases::cached_verified_playback_artist_aliases(&store, &track);
        let alias = context
            .matching_track(&track, "Canonical A feat. Guest")
            .unwrap();
        assert_eq!(alias.artist, "Canonical A feat. Guest");
        let mut original = alias;
        original.artist = track.artist.clone();
        assert_eq!(original, track);
        assert!(context.matching_track(&track, "Canonical A").is_none());
    }
    #[test]
    fn complete_search_uniqueness_and_provider_boundaries_are_required() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("ambiguous.sqlite")).unwrap();
        let mut track = fixture(&store, "beatport");
        let second = "00000000-0000-4000-8000-000000000004";
        let mut raw = store
            .get_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                RECORDING,
            )
            .unwrap()
            .unwrap();
        raw["id"] = json!(second);
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                second,
                &raw,
            )
            .unwrap();
        assert!(generic_supported_recording(
            &store,
            &track,
            &json!({"count":2,"recordings":[{"id":RECORDING},{"id":second}]})
        )
        .is_none());
        assert!(generic_supported_recording(
            &store,
            &track,
            &json!({"count":2,"recordings":[{"id":RECORDING}]})
        )
        .is_none());
        for provider in ["catalog", "spotify", "unknown"] {
            track.provider = provider.into();
            assert!(generic_source_fingerprint(&track).is_none());
        }
    }
}
