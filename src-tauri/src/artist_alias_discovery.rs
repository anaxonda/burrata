//! Demand-driven raw evidence acquisition. All playback identity gates remain in
//! artist_credit_aliases; search results are locators, never identity proof.
use crate::{artist_credit_aliases as aliases, models::Track, Store};
use reqwest::Client;
use serde_json::{json, Value};
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
    if matches!(track.provider.as_str(), "beatport" | "traxsource" | "local") {
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
    let mut mapped = true;
    for artist in &fingerprint.artists {
        if !discover_spotify_artist(store, &artist.id).await? {
            mapped = false;
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
            || isrc_authority_variants(store, track, id).is_some()
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
fn isrc_authority_choices(
    store: &Store,
    track: &Track,
    recording_id: &str,
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
        .map(str::to_ascii_uppercase)
        .collect::<BTreeSet<_>>();
    if isrcs.len() != 1 || !isrcs.contains(source_isrc) {
        return None;
    }
    let credits = recording["artist-credit"].as_array()?;
    if credits.len() != source.artists.len() || credits.is_empty() || credits.len() > 16 {
        return None;
    }
    let mut choices = Vec::new();
    for (credit, provider) in credits.iter().zip(&source.artists) {
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
        if !names.iter().any(|name| {
            crate::matching::normalize(name) == crate::matching::normalize(&provider.name)
        }) {
            return None;
        }
        let join = credit["joinphrase"].as_str()?;
        if join.len() > 32 {
            return None;
        }
        choices.push((names, join.to_owned()));
    }
    Some(choices)
}
fn isrc_authority_variants(store: &Store, track: &Track, recording_id: &str) -> Option<Vec<Track>> {
    let choices = isrc_authority_choices(store, track, recording_id)?;
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
                || isrc_authority_variants(store, &hydrated, candidate).is_some()
        })
        .collect::<BTreeSet<_>>();
    if supported.len() != 1 || !supported.contains(id) {
        return vec![];
    }
    isrc_authority_variants(store, &hydrated, id)
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
        let profile = json!({"id":artist_id,"name":"Canonical Name","aliases":[{"name":"Provider Name"}],"relations":[]});
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
        let variants = isrc_authority_variants(&store, &track, recording_id).unwrap();
        assert_eq!(variants[0].artist, "Canonical Name");
        let mut restored = variants[0].clone();
        restored.artist = track.artist.clone();
        assert_eq!(restored, track);
        recording["isrcs"] = json!(["USABC2600002"]);
        store
            .set_retained_provider_cache(
                aliases::spotify_recording_credit_raw_cache_namespace(),
                recording_id,
                &recording,
            )
            .unwrap();
        assert!(isrc_authority_variants(&store, &track, recording_id).is_none());
        track.version = Some("Live".into());
        assert!(isrc_authority_variants(&store, &track, recording_id).is_none());
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
    let choices = isrc_authority_choices(store, &hydrated, binding["recordingId"].as_str()?)?;
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
        println!("Verified exact artist authority aliases from independent shared identifiers; {} names; no Spotify recording proof asserted",names.len());
    }
}

const PROVIDER_ARTIST: &str = "artists.alias-discovery.provider-artist.v1";
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
    if !matches!(track.provider.as_str(), "beatport" | "traxsource" | "local")
        || track.id.is_empty()
        || track.title.trim().is_empty()
        || track.artist.trim().is_empty()
        || track.version.is_some()
        || track.playback_range.is_some()
    {
        return None;
    }
    let isrc = track.isrc.as_deref()?;
    if isrc.len() != 12
        || !isrc.is_ascii()
        || !isrc.as_bytes()[..2].iter().all(u8::is_ascii_alphabetic)
        || !isrc.as_bytes()[2..5].iter().all(u8::is_ascii_alphanumeric)
        || !isrc.as_bytes()[5..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let duration = track.duration_seconds?;
    if !duration.is_finite() || duration <= 0.0 || duration > 86400.0 {
        return None;
    }
    Some(
        json!({"id":track.id,"provider":track.provider,"providerId":track.provider_id,"sourceUrl":track.source_url,"title":track.title,"artist":track.artist,"version":track.version,"album":track.album,"albumArtist":track.album_artist,"duration":duration,"isrc":isrc,"playbackRange":track.playback_range,"remixers":track.remixers}),
    )
}
fn saved_generic_fingerprint(store: &Store, track: &Track) -> Option<Value> {
    let fingerprint = generic_source_fingerprint(track)?;
    let saved = store.get_library_track(&track.id).ok()??;
    (generic_source_fingerprint(&saved).as_ref() == Some(&fingerprint)).then_some(fingerprint)
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
    let duration = (track.duration_seconds? * 1000.0).round() as u64;
    if recording["id"].as_str() != Some(recording_id)
        || crate::matching::normalize(recording["title"].as_str()?)
            != crate::matching::normalize(&track.title)
        || recording["length"].as_u64()? == 0
        || recording["length"].as_u64()?.abs_diff(duration) > 5000
    {
        return None;
    }
    let isrcs = recording["isrcs"]
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(str::to_ascii_uppercase)
        .collect::<BTreeSet<_>>();
    if isrcs.len() != 1 || !isrcs.contains(&track.isrc.as_deref()?.to_ascii_uppercase()) {
        return None;
    }
    let credits = recording["artist-credit"].as_array()?;
    if credits.is_empty() || credits.len() > 16 {
        return None;
    }
    let mut choices = Vec::new();
    for credit in credits {
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
        let join = credit["joinphrase"].as_str()?;
        if join.len() > 32 {
            return None;
        }
        let mut names = names.into_iter().collect::<Vec<_>>();
        names.retain(|name| name != canonical);
        names.insert(0, canonical.to_owned());
        choices.push((names, join.to_owned()));
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
        for credit in credits {
            let artist_id = credit["artist"]["id"].as_str()?;
            if !aliases::valid_musicbrainz_recording_id(artist_id)
                || credit["joinphrase"].as_str().is_none()
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
        if generic_recording_choices(store, track, id).is_some() {
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
    generic_recording_choices(store, track, &id)
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
                    track
                        .isrc
                        .as_deref()
                        .expect("validated ISRC")
                        .to_ascii_uppercase()
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
            duration_seconds: Some(180.0),
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
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.artist = "Source A feat. Other".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.title = "Different Song".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.duration_seconds = Some(200.0);
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.isrc = Some("USABC2600002".into());
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.version = Some("Live".into());
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
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
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.artist = "Guest feat. Source A".into();
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.isrc = None;
            assert!(generic_recording_choices(&store, &changed, RECORDING).is_none());
            changed = track.clone();
            changed.title = "Manual edit".into();
            store.save_track(&changed).unwrap();
            assert!(cached_generic_isrc_authority_credit_choices(&store, &track).is_none());
            assert!(cached_generic_isrc_authority_credit_choices(&store, &changed).is_none());
        }
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
