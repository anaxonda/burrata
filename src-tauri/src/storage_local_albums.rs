//! Reversible Library album links. These never merge recordings, rewrite tags,
//! or change provider matching policy. Local playback bindings require a strict
//! per-track match even when the user confirms album identity manually.
use super::library::{base_canonical_refs, canonical_refs, track_entities, LibraryEntity};
use super::*;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct LocalGroup {
    entry: LibraryEntity,
    tracks: Vec<Track>,
    assets: Vec<LocalAsset>,
    embedded: Option<String>,
    fingerprint: String,
}
#[derive(Clone)]
struct Catalog {
    entry: LibraryEntity,
    tracks: Vec<Track>,
    fingerprint: String,
}
#[derive(Clone)]
pub(super) struct Resolution {
    local: LocalGroup,
    target: String,
    catalog: Option<Catalog>,
    pairs: Vec<(String, usize)>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAlbumCandidate {
    pub album: LibraryEntity,
    pub matched_tracks: usize,
    pub local_tracks: usize,
    pub catalog_tracks: usize,
    pub complete_match: bool,
    pub can_link: bool,
    pub reason: String,
    pub token: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAlbumMatchPage {
    pub local_album: LibraryEntity,
    pub local_tracks: usize,
    pub state: String,
    pub folders: Vec<String>,
    pub candidates: Vec<LocalAlbumCandidate>,
}

fn key(value: &str) -> String {
    normalize_artist_name(value)
}
fn fingerprint(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
fn embedded(track: &Track, name: &str) -> Option<String> {
    track
        .provenance
        .get("embeddedIdentifiers")?
        .get(name)?
        .as_str()
        .filter(|id| crate::spotify_mb_crosswalk::valid_mbid(id))
        .map(str::to_ascii_lowercase)
}
fn group_fingerprint(tracks: &[Track]) -> String {
    fingerprint(&json!(tracks
        .iter()
        .map(|t| json!([
            t.id,
            t.artist,
            t.title,
            t.version,
            t.album,
            t.album_artist,
            t.disc_number,
            t.track_number,
            t.duration_seconds,
            t.isrc,
            t.provenance.get("embeddedIdentifiers")
        ]))
        .collect::<Vec<_>>()))
}
fn load_groups(conn: &Connection, tracks: &[Track]) -> Result<Vec<LocalGroup>> {
    let mut groups = BTreeMap::<String, LocalGroup>::new();
    let mut local_ids = HashMap::new();
    for track in tracks.iter().filter(|t| t.provider == "local") {
        if let Some(entry) = track_entities(track)
            .into_iter()
            .find(|e| e.kind == "album")
        {
            local_ids.insert(track.id.clone(), entry.id.clone());
            let group = groups
                .entry(entry.id.clone())
                .or_insert_with(|| LocalGroup {
                    entry,
                    tracks: Vec::new(),
                    assets: Vec::new(),
                    embedded: None,
                    fingerprint: String::new(),
                });
            group.tracks.push(track.clone());
        }
    }
    let rows = conn
        .prepare("SELECT payload FROM local_assets ORDER BY id")
        .map_err(err)?
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    for raw in rows {
        let asset: LocalAsset = decode(raw)?;
        if let Some(id) = local_ids.get(&asset.recording_id) {
            groups.get_mut(id).unwrap().assets.push(asset);
        }
    }
    for group in groups.values_mut() {
        group.tracks.sort_by(|a, b| {
            (
                a.disc_number.unwrap_or(1),
                a.track_number.unwrap_or(u32::MAX),
                &a.id,
            )
                .cmp(&(
                    b.disc_number.unwrap_or(1),
                    b.track_number.unwrap_or(u32::MAX),
                    &b.id,
                ))
        });
        let ids = group
            .tracks
            .iter()
            .filter(|t| {
                t.provenance.pointer("/manualFields/album") != Some(&json!(true))
                    && t.provenance.pointer("/manualFields/albumArtist") != Some(&json!(true))
            })
            .filter_map(|t| embedded(t, "musicbrainzReleaseId"))
            .collect::<BTreeSet<_>>();
        if ids.len() == 1 {
            group.embedded = Some(format!("album:ref:musicbrainz:{}", ids.first().unwrap()));
        }
        group.fingerprint = group_fingerprint(&group.tracks);
    }
    Ok(groups.into_values().collect())
}
fn catalogs(conn: &Connection) -> Result<Vec<Catalog>> {
    let refs = base_canonical_refs(conn, "album")?;
    let rows = conn
        .prepare("SELECT payload FROM library_entity_saves WHERE kind='album' ORDER BY id")
        .map_err(err)?
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    let mut result = BTreeMap::new();
    for raw in rows {
        let mut entry: LibraryEntity = decode(raw)?;
        if entry.provider.as_deref() == Some("local")
            || entry.provider_id.is_none()
            || entry.saved_track_ids.is_empty()
            || entry.total_tracks != Some(entry.saved_track_ids.len())
        {
            continue;
        }
        if let Some(component) = refs.get(&entry.id) {
            entry.id = component[0].clone();
            entry.aliases = component.clone();
        }
        if result.contains_key(&entry.id) {
            continue;
        }
        let mut parsed = Vec::new();
        for id in &entry.saved_track_ids {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT metadata FROM catalog_listings WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if let Some(raw) = raw {
                parsed.push(decode::<Track>(raw)?);
            }
        }
        if parsed.len() != entry.saved_track_ids.len() {
            continue;
        }
        let tracks = hydrate_page_tracks(conn, parsed)?;
        let signature = fingerprint(&json!([entry.id, group_fingerprint(&tracks)]));
        result.insert(
            entry.id.clone(),
            Catalog {
                entry,
                tracks,
                fingerprint: signature,
            },
        );
    }
    Ok(result.into_values().collect())
}
fn recording_id(track: &Track) -> Option<String> {
    embedded(track, "musicbrainzRecordingId").or_else(|| {
        track
            .provenance
            .get("recordingId")
            .and_then(Value::as_str)
            .filter(|id| crate::spotify_mb_crosswalk::valid_mbid(id))
            .map(str::to_ascii_lowercase)
    })
}
fn strict_track(local: &Track, source: &Track) -> bool {
    if key(&local.artist) != key(&source.artist)
        || key(&local.title) != key(&source.title)
        || key(local.version.as_deref().unwrap_or(""))
            != key(source.version.as_deref().unwrap_or(""))
    {
        return false;
    }
    let isrc_local = local
        .isrc
        .as_deref()
        .and_then(crate::recording_identity::normalize_isrc);
    let isrc_source = source
        .isrc
        .as_deref()
        .and_then(crate::recording_identity::normalize_isrc);
    if isrc_local.is_some() && isrc_source.is_some() && isrc_local != isrc_source {
        return false;
    }
    let mb_local = recording_id(local);
    let mb_source = recording_id(source);
    if mb_local.is_some() && mb_source.is_some() && mb_local != mb_source {
        return false;
    }
    match (
        local.duration_seconds.filter(|v| v.is_finite() && *v > 0.0),
        source
            .duration_seconds
            .filter(|v| v.is_finite() && *v > 0.0),
    ) {
        (Some(a), Some(b)) => (a - b).abs() <= 2.0,
        _ => {
            (isrc_local.is_some() && isrc_local == isrc_source)
                || (mb_local.is_some() && mb_local == mb_source)
        }
    }
}
fn align(group: &LocalGroup, catalog: &Catalog) -> (Vec<(String, usize)>, bool, bool, String) {
    if let Some(embedded) = &group.embedded {
        let known = catalog
            .entry
            .aliases
            .iter()
            .chain(std::iter::once(&catalog.entry.id))
            .filter(|id| id.starts_with("album:ref:musicbrainz:"))
            .collect::<Vec<_>>();
        if !known.is_empty() && !known.iter().any(|id| *id == embedded) {
            return (
                vec![],
                false,
                false,
                "File tags identify a different release.".into(),
            );
        }
    }
    let numbered = group
        .tracks
        .iter()
        .all(|t| t.track_number.is_some_and(|n| n > 0));
    let local_positions = group
        .tracks
        .iter()
        .filter_map(|t| {
            t.track_number
                .map(|number| (t.disc_number.unwrap_or(1), number))
        })
        .collect::<BTreeSet<_>>();
    let source_numbered = catalog
        .tracks
        .iter()
        .all(|t| t.track_number.is_some_and(|n| n > 0));
    let source_positions = catalog
        .tracks
        .iter()
        .filter_map(|t| t.track_number.map(|n| (t.disc_number.unwrap_or(1), n)))
        .collect::<BTreeSet<_>>();
    let source_numbered = source_numbered && source_positions.len() == catalog.tracks.len();
    let mut previous = (1u32, 0u32);
    let contiguous = local_positions.iter().all(|&(disc, number)| {
        let valid = (disc == previous.0 && previous.1.checked_add(1) == Some(number))
            || (previous.0.checked_add(1) == Some(disc) && number == 1 && previous.1 > 0);
        previous = (disc, number);
        valid
    });
    let complete_shape = numbered && contiguous && local_positions.len() == catalog.tracks.len();
    let mut pairs = Vec::new();
    let mut used = BTreeMap::<usize, Option<(u32, u32)>>::new();
    for local in &group.tracks {
        let position = local
            .track_number
            .map(|n| (local.disc_number.unwrap_or(1), n));
        let expected = if source_numbered {
            position.and_then(|pos| {
                catalog
                    .tracks
                    .iter()
                    .position(|t| (t.disc_number.unwrap_or(1), t.track_number.unwrap()) == pos)
            })
        } else if complete_shape {
            position.and_then(|pos| local_positions.iter().position(|p| *p == pos))
        } else {
            None
        };
        let matches = catalog
            .tracks
            .iter()
            .enumerate()
            .filter(|(index, source)| {
                expected.is_none_or(|expected| *index == expected) && strict_track(local, source)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            continue;
        }
        let index = matches[0];
        if let Some(previous) = used.get(&index) {
            if previous != &position {
                continue;
            }
        }
        used.insert(index, position);
        pairs.push((local.id.clone(), index));
    }
    let can_link = pairs.len() == group.tracks.len() && !pairs.is_empty();
    let years_agree = match (
        group.entry.release_date.as_deref(),
        catalog.entry.release_date.as_deref(),
    ) {
        (Some(a), Some(b)) => a.get(..4) == b.get(..4),
        _ => true,
    };
    let complete = can_link
        && complete_shape
        && used.len() == catalog.tracks.len()
        && key(&group.entry.name) == key(&catalog.entry.name)
        && key(&group.entry.artist) == key(&catalog.entry.artist)
        && years_agree;
    let reason = if !can_link {
        "Some files have missing, ambiguous, or conflicting track metadata."
    } else if !complete {
        "This is a partial match or the album metadata differs. Confirm the edition before linking."
    } else {
        "Every track matches in order, version, and duration or recording identity."
    };
    (pairs, can_link, complete, reason.into())
}
fn token(group: &LocalGroup, catalog: &Catalog) -> String {
    fingerprint(&json!([group.fingerprint, catalog.fingerprint]))
}
fn write_choice(conn: &Connection, group: &LocalGroup, target: &str, state: &str) -> Result<()> {
    let payload = json!({"localFingerprint":group.fingerprint,"embeddedRef":group.embedded});
    conn.execute("INSERT INTO library_album_reconciliation VALUES(?1,?2,?3,?4) ON CONFLICT(local_ref) DO UPDATE SET catalog_ref=excluded.catalog_ref,state=excluded.state,payload=excluded.payload WHERE catalog_ref!=excluded.catalog_ref OR state!=excluded.state OR payload!=excluded.payload",params![group.entry.id,target,state,encode(&payload)?]).map_err(err)?;
    Ok(())
}
pub(super) fn reconcile(conn: &Connection, tracks: &[Track]) -> Result<Vec<Resolution>> {
    let groups = load_groups(conn, tracks)?;
    if groups.is_empty() {
        return Ok(vec![]);
    }
    let catalogs = catalogs(conn)?;
    let base = base_canonical_refs(conn, "album")?;
    let mut result = Vec::new();
    for group in groups {
        let old:Option<(String,String,String)>=conn.query_row("SELECT catalog_ref,state,payload FROM library_album_reconciliation WHERE local_ref=?1",[&group.entry.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
        if old
            .as_ref()
            .is_some_and(|(_, state, _)| state == "rejected")
        {
            continue;
        }
        if let Some((target, state, payload)) = &old {
            if state == "confirmed" {
                let target = base
                    .get(target)
                    .map_or(target.as_str(), |refs| refs[0].as_str());
                let catalog = catalogs.iter().find(|c| c.entry.id == target);
                if let Some(catalog) = catalog {
                    let (pairs, can_link, _, _) = align(&group, catalog);
                    if can_link {
                        write_choice(conn, &group, target, "confirmed")?;
                        result.push(Resolution {
                            local: group,
                            target: target.to_owned(),
                            catalog: Some(catalog.clone()),
                            pairs,
                        });
                        continue;
                    }
                } else if serde_json::from_str::<Value>(payload)
                    .ok()
                    .and_then(|v| v["localFingerprint"].as_str().map(str::to_owned))
                    .as_deref()
                    == Some(group.fingerprint.as_str())
                {
                    result.push(Resolution {
                        local: group,
                        target: target.to_owned(),
                        catalog: None,
                        pairs: vec![],
                    });
                    continue;
                }
            }
        }
        let exact = group.embedded.as_ref().map(|id| {
            base.get(id)
                .map_or_else(|| id.clone(), |refs| refs[0].clone())
        });
        let candidates = catalogs
            .iter()
            .filter(|c| {
                exact.as_deref() == Some(c.entry.id.as_str())
                    || (key(&c.entry.name) == key(&group.entry.name)
                        && key(&c.entry.artist) == key(&group.entry.artist))
            })
            .map(|c| {
                let analysis = align(&group, c);
                (c, analysis)
            })
            .collect::<Vec<_>>();
        let exact_match = candidates.iter().find(|(c, (_, can_link, _, _))| {
            *can_link && exact.as_deref() == Some(c.entry.id.as_str())
        });
        let full = candidates
            .iter()
            .filter(|(_, (_, _, complete, _))| *complete)
            .collect::<Vec<_>>();
        let selected = exact_match.or_else(|| (full.len() == 1).then(|| full[0]));
        if let Some((catalog, (pairs, _, _, _))) = selected {
            let state = if exact.as_deref() == Some(catalog.entry.id.as_str()) {
                "embedded"
            } else {
                "auto"
            };
            write_choice(conn, &group, &catalog.entry.id, state)?;
            result.push(Resolution {
                local: group,
                target: catalog.entry.id.clone(),
                catalog: Some((*catalog).clone()),
                pairs: pairs.clone(),
            });
        } else if let Some(exact) = exact.filter(|id| !catalogs.iter().any(|c| c.entry.id == *id)) {
            write_choice(conn, &group, &exact, "embedded")?;
            result.push(Resolution {
                local: group,
                target: exact,
                catalog: None,
                pairs: vec![],
            });
        } else if let Some((target, _, _)) = old {
            write_choice(conn, &group, &target, "needs-review")?;
        }
    }
    Ok(result)
}

fn asset_rank(asset: &LocalAsset) -> (bool, bool, u64, std::cmp::Reverse<String>) {
    (
        Path::new(&asset.path).is_file(),
        matches!(
            asset.final_codec.to_lowercase().as_str(),
            "flac" | "alac" | "wav" | "pcm" | "aiff"
        ),
        asset.final_bitrate.unwrap_or(0.0).max(0.0) as u64,
        std::cmp::Reverse(asset.id.clone()),
    )
}
pub(super) fn populate_bindings(conn: &Connection, resolutions: &[Resolution]) -> Result<()> {
    let refs = canonical_refs(conn, "album")?;
    for resolution in resolutions {
        let Some(catalog) = &resolution.catalog else {
            continue;
        };
        let album = refs
            .get(&resolution.target)
            .map_or(resolution.target.as_str(), |refs| refs[0].as_str());
        let mut positions = BTreeMap::<usize, Vec<&LocalAsset>>::new();
        for (local, index) in &resolution.pairs {
            positions.entry(*index).or_default().extend(
                resolution
                    .local
                    .assets
                    .iter()
                    .filter(|a| &a.recording_id == local),
            );
        }
        for (position, assets) in positions {
            let mut actual:Option<String>=conn.query_row("SELECT listing_id FROM library_entity_tracks WHERE entity_id=?1 AND position=?2 ORDER BY listing_id LIMIT 1",params![album,position as i64],|r|r.get(0)).optional().map_err(err)?;
            if actual.is_none() {
                if let Some(best) = assets.iter().max_by_key(|asset| asset_rank(asset)) {
                    actual = Some(best.recording_id.clone());
                    conn.execute(
                        "INSERT OR IGNORE INTO library_entity_tracks VALUES(?1,?2,?3)",
                        params![album, best.recording_id, position as i64],
                    )
                    .map_err(err)?;
                }
            }
            let Some(actual) = actual else {
                continue;
            };
            let raw: Option<String> = conn
                .query_row(
                    "SELECT metadata FROM catalog_listings WHERE id=?1",
                    [&actual],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            let Some(raw) = raw else {
                continue;
            };
            let actual_track = hydrate(conn, decode(raw)?)?;
            for asset in assets {
                let Some(local) = resolution
                    .local
                    .tracks
                    .iter()
                    .find(|t| t.id == asset.recording_id)
                else {
                    continue;
                };
                if !strict_track(local, &catalog.tracks[position])
                    || !strict_track(local, &actual_track)
                {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO library_local_album_tracks VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        album,
                        position as i64,
                        actual,
                        local.id,
                        asset.id,
                        asset.hash
                    ],
                )
                .map_err(err)?;
                conn.execute("INSERT OR IGNORE INTO library_artist_albums SELECT t.entity_id,?1 FROM library_entity_tracks t JOIN library_entities e ON e.id=t.entity_id AND e.kind='artist' WHERE t.listing_id=?2",params![album,local.id]).map_err(err)?;
            }
        }
    }
    for resolution in resolutions {
        let Some(catalog) = &resolution.catalog else {
            continue;
        };
        let album = refs
            .get(&resolution.target)
            .map_or(resolution.target.as_str(), |refs| refs[0].as_str());
        for (local, position) in &resolution.pairs {
            let source: Option<String> = conn.query_row("SELECT listing_id FROM library_entity_tracks WHERE entity_id=?1 AND position=?2 AND listing_id!=?3 ORDER BY listing_id LIMIT 1",params![album,*position as i64,local],|row|row.get(0)).optional().map_err(err)?;
            if let Some(source) = source {
                let original = resolution
                    .local
                    .tracks
                    .iter()
                    .find(|track| track.id == *local);
                let raw: String = conn
                    .query_row(
                        "SELECT metadata FROM catalog_listings WHERE id=?1",
                        [&source],
                        |row| row.get(0),
                    )
                    .map_err(err)?;
                let actual = hydrate(conn, decode(raw)?)?;
                if original.is_some_and(|track| {
                    strict_track(track, &catalog.tracks[*position]) && strict_track(track, &actual)
                }) {
                    conn.execute(
                        "DELETE FROM library_entity_tracks WHERE entity_id=?1 AND listing_id=?2",
                        params![album, local],
                    )
                    .map_err(err)?;
                    conn.execute("DELETE FROM library_entity_tracks AS t WHERE t.entity_id LIKE 'artist:%' AND t.listing_id=?1 AND EXISTS(SELECT 1 FROM library_entity_tracks source WHERE source.entity_id=t.entity_id AND source.listing_id=?2)",params![local,source]).map_err(err)?;
                }
            }
        }
    }
    conn.execute("DELETE FROM library_entity_tracks AS t WHERE EXISTS(SELECT 1 FROM library_local_album_tracks b WHERE b.album_id=t.entity_id AND b.local_listing_id=t.listing_id) AND NOT EXISTS(SELECT 1 FROM library_local_album_tracks b WHERE b.album_id=t.entity_id AND b.catalog_listing_id=t.listing_id AND b.position=t.position)",[]).map_err(err)?;
    conn.execute("DELETE FROM library_entity_tracks AS t WHERE t.entity_id LIKE 'artist:%' AND EXISTS(SELECT 1 FROM library_local_album_tracks b JOIN library_entity_tracks source ON source.entity_id=t.entity_id AND source.listing_id=b.catalog_listing_id WHERE b.local_listing_id=t.listing_id AND b.catalog_listing_id!=b.local_listing_id)",[]).map_err(err)?;
    Ok(())
}

pub(super) fn overlay(
    conn: &Connection,
    entity: &str,
    listing: &str,
    position: i64,
    track: &mut Track,
) -> Result<()> {
    if track.local_availability == "available" && track.local_path.is_some() {
        return Ok(());
    }
    if crate::playback_sources::source_resolution_is_pinned(
        track,
        false,
        &Settings::default(),
        false,
    ) {
        return Ok(());
    }
    let raws=conn.prepare("SELECT a.payload FROM library_local_album_tracks b JOIN local_assets a ON a.id=b.asset_id AND a.hash=b.file_hash WHERE b.catalog_listing_id=?1 AND (?2 LIKE 'artist:%' OR (b.album_id=?2 AND b.position=?3))").map_err(err)?.query_map(params![listing,entity,position],|r|r.get::<_,String>(0)).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
    let assets = raws
        .into_iter()
        .map(decode::<LocalAsset>)
        .collect::<Result<Vec<_>>>()?;
    if let Some(asset) = assets
        .iter()
        .filter(|a| Path::new(&a.path).is_file())
        .max_by_key(|asset| asset_rank(asset))
    {
        track.local_path = Some(asset.path.clone());
        track.asset_id = Some(asset.id.clone());
        track.local_availability = "available".into();
        if !track.provenance.is_object() {
            track.provenance = json!({});
        }
        track.provenance["localPlaybackMatch"] =
            json!("linked album with strict track metadata and recording evidence");
    }
    Ok(())
}

fn match_page(conn: &Connection, local_ref: &str, query: &str) -> Result<LocalAlbumMatchPage> {
    let raw = conn
        .prepare("SELECT metadata FROM catalog_listings WHERE id=recording_id AND in_library=1")
        .map_err(err)?
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    let tracks = hydrate_page_tracks(
        &conn,
        raw.into_iter()
            .map(decode)
            .collect::<Result<Vec<Track>>>()?,
    )?;
    let group = load_groups(&conn, &tracks)?
        .into_iter()
        .find(|g| g.entry.id == local_ref)
        .ok_or("Local album is no longer available")?;
    let state = conn
        .query_row(
            "SELECT state FROM library_album_reconciliation WHERE local_ref=?1",
            [local_ref],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(err)?
        .unwrap_or_else(|| "unlinked".into());
    let mut candidates = Vec::new();
    for catalog in catalogs(&conn)? {
        if !query.trim().is_empty()
            && !key(&format!("{} {}", catalog.entry.name, catalog.entry.artist))
                .contains(&key(query))
        {
            continue;
        }
        let (pairs, can_link, complete, reason) = align(&group, &catalog);
        if pairs.is_empty()
            && query.trim().is_empty()
            && key(&catalog.entry.name) != key(&group.entry.name)
        {
            continue;
        }
        candidates.push(LocalAlbumCandidate {
            album: catalog.entry.clone(),
            matched_tracks: pairs.len(),
            local_tracks: group.tracks.len(),
            catalog_tracks: catalog.tracks.len(),
            complete_match: complete,
            can_link,
            reason,
            token: token(&group, &catalog),
        });
    }
    candidates.sort_by(|a, b| {
        b.complete_match
            .cmp(&a.complete_match)
            .then(b.matched_tracks.cmp(&a.matched_tracks))
            .then(a.album.name.cmp(&b.album.name))
    });
    Ok(LocalAlbumMatchPage {
        local_album: group.entry.clone(),
        local_tracks: group.tracks.len(),
        state,
        folders: group
            .assets
            .iter()
            .filter_map(|asset| Path::new(&asset.path).parent())
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        candidates,
    })
}

impl Store {
    pub fn local_album_matches(&self, local_ref: &str, query: &str) -> Result<LocalAlbumMatchPage> {
        let conn = self.lock_connection()?;
        super::library::refresh(&conn)?;
        let mut page = match_page(&conn, local_ref, query)?;
        page.candidates.truncate(200);
        Ok(page)
    }
    pub fn set_local_album_match(
        &self,
        local_ref: &str,
        target: &str,
        decision: &str,
        expected_token: Option<&str>,
    ) -> Result<Option<LibraryEntity>> {
        if !matches!(decision, "confirm" | "reject") {
            return Err("Unknown local album decision".into());
        }
        {
            let conn = self.lock_connection()?;
            super::library::refresh(&conn)?;
            let page = match_page(&conn, local_ref, "")?;
            if decision == "confirm" {
                let candidate = page
                    .candidates
                    .iter()
                    .find(|candidate| candidate.album.id == target)
                    .ok_or("Saved album is no longer available")?;
                if !candidate.can_link || expected_token != Some(candidate.token.as_str()) {
                    return Err(
                        "The files or saved album changed. Refresh the matches before linking."
                            .into(),
                    );
                }
            }
            conn.execute("INSERT INTO library_album_reconciliation VALUES(?1,?2,?3,'{}') ON CONFLICT(local_ref) DO UPDATE SET catalog_ref=excluded.catalog_ref,state=excluded.state,payload=excluded.payload",params![local_ref,target,if decision=="confirm" { "confirmed" } else { "rejected" }]).map_err(err)?;
        }
        self.library_entity_status(local_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn song(provider: &str, id: &str, number: u32) -> Track {
        Track {
            id: format!("{provider}:{id}"),
            provider: provider.into(),
            provider_id: id.into(),
            artist: "An Artist".into(),
            title: format!("Song {number}"),
            album: Some("An Album".into()),
            album_artist: Some("An Artist".into()),
            track_number: Some(number),
            disc_number: Some(1),
            duration_seconds: Some(180.0 + f64::from(number)),
            release_date: Some("2020".into()),
            ..Default::default()
        }
    }
    fn save_catalog(store: &Store, id: &str) -> LibraryEntity {
        let entry: LibraryEntity = serde_json::from_value(json!({"id":"","kind":"album","name":"An Album","artist":"An Artist","provider":"spotify","providerId":id,"releaseDate":"2020","totalTracks":2,"explicitlySaved":false,"trackCount":0,"albumCount":0})).unwrap();
        store
            .save_library_album(
                entry,
                vec![
                    song("spotify", &format!("{id}-one"), 1),
                    song("spotify", &format!("{id}-two"), 2),
                ],
                2,
            )
            .unwrap()
    }
    fn save_local(store: &Store, root: &Path, number: u32) -> (String, String) {
        let track = song("local", &format!("hash-{number}"), number);
        let path = root.join(format!("{number}.flac"));
        std::fs::write(&path, b"test audio placeholder").unwrap();
        let asset: LocalAsset=serde_json::from_value(json!({"id":format!("asset-{number}"),"recordingId":track.id,"path":path,"hash":format!("hash-{number}"),"sourceCodec":"flac","finalCodec":"flac","processing":"none","track":track})).unwrap();
        store.save_asset(&asset).unwrap();
        let entry = track_entities(&asset.track)
            .into_iter()
            .find(|e| e.kind == "album")
            .unwrap();
        (entry.id, path.to_string_lossy().into_owned())
    }
    fn page(
        store: &Store,
        id: &str,
        availability: &str,
    ) -> super::super::library::LibraryTrackPage {
        store
            .list_library_entity_tracks(id, "", "", false, availability, None, None, 200, 0)
            .unwrap()
    }
    #[test]
    fn unique_full_tracklist_links_reversibly_and_uses_present_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        let catalog = save_catalog(&store, "edition");
        let (local, path) = save_local(&store, dir.path(), 1);
        save_local(&store, dir.path(), 2);
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            1
        );
        let tracks = page(&store, &catalog.id, "all");
        assert_eq!(tracks.total, 2);
        assert!(tracks
            .tracks
            .iter()
            .all(|t| t.provider == "spotify" && t.local_path.is_some()));
        assert_eq!(store.local_album_matches(&local, "").unwrap().state, "auto");
        {
            let mut pinned = tracks.tracks[0].clone();
            pinned.local_path = None;
            pinned.asset_id = None;
            pinned.local_availability = "none".into();
            pinned.provenance["explicitSourceLock"] = json!(true);
            let conn = store.lock_connection().unwrap();
            let id = pinned.id.clone();
            overlay(&conn, &catalog.id, &id, 0, &mut pinned).unwrap();
            assert!(
                pinned.local_path.is_none(),
                "Album links must preserve an explicit source selection"
            );
        }

        assert_eq!(page(&store, &catalog.id, "local").total, 2);
        std::fs::remove_file(path).unwrap();
        assert_eq!(page(&store, &catalog.id, "local").total, 1);
        assert_eq!(page(&store, &catalog.id, "remote").total, 1);
        store
            .set_local_album_match(&local, "", "reject", None)
            .unwrap();
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            2
        );
        assert!(page(&store, &catalog.id, "all")
            .tracks
            .iter()
            .all(|t| t.local_path.is_none()));
        assert_eq!(
            store.local_album_matches(&local, "").unwrap().state,
            "rejected"
        );
        drop(store);
        let reopened = Store::open(&dir.path().join("db")).unwrap();
        assert_eq!(
            reopened
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            2
        );
    }
    #[test]
    fn partial_or_multiple_editions_require_confirmation_and_stale_tokens_fail() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        let catalog = save_catalog(&store, "edition");
        let (local, _) = save_local(&store, dir.path(), 1);
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            2
        );
        let candidates = store.local_album_matches(&local, "").unwrap().candidates;
        assert!(candidates[0].can_link);
        assert!(!candidates[0].complete_match);
        assert!(store
            .set_local_album_match(&local, &catalog.id, "confirm", Some("stale"))
            .is_err());
        store
            .set_local_album_match(&local, &catalog.id, "confirm", Some(&candidates[0].token))
            .unwrap();
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            1
        );
        assert_eq!(page(&store, &catalog.id, "all").total, 2);
        assert_eq!(page(&store, &catalog.id, "local").total, 1);
        store
            .set_local_album_match(&local, "", "reject", None)
            .unwrap();
        save_local(&store, dir.path(), 2);
        save_catalog(&store, "other-edition");
        // Explicit separation survives discovery of a complete match.
        assert_eq!(
            store.local_album_matches(&local, "").unwrap().state,
            "rejected"
        );
        {
            let conn = store.lock_connection().unwrap();
            conn.execute("DELETE FROM library_album_reconciliation", [])
                .unwrap();
        }
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            3
        );
        assert_eq!(
            store.local_album_matches(&local, "").unwrap().state,
            "unlinked"
        );
    }
    #[test]
    fn embedded_release_identity_links_a_partial_album_to_the_verified_edition() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        save_catalog(&store, "edition");
        let mb = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        store
            .link_library_entities("album", "spotify", "edition", "musicbrainz", mb)
            .unwrap();
        save_local(&store, dir.path(), 1);
        let mut track = store.get_track("local:hash-1").unwrap().unwrap();
        track.provenance["embeddedIdentifiers"] = json!({"musicbrainzReleaseId":mb});
        store.save_track(&track).unwrap();
        let local = track_entities(&track)
            .into_iter()
            .find(|entry| entry.kind == "album")
            .unwrap();
        let albums = store
            .list_library_entities("album", None, "", 200, 0)
            .unwrap();
        assert_eq!(albums.total, 1);
        assert_eq!(albums.items[0].id, format!("album:ref:musicbrainz:{mb}"));
        assert_eq!(
            store.local_album_matches(&local.id, "").unwrap().state,
            "embedded"
        );
        assert_eq!(page(&store, &albums.items[0].id, "all").total, 2);
        assert_eq!(page(&store, &albums.items[0].id, "local").total, 1);
    }
    #[test]
    fn strict_matching_rejects_identifier_version_duration_and_numbering_conflicts() {
        let local = song("local", "one", 1);
        let source = song("spotify", "one", 1);
        assert!(strict_track(&local, &source));
        let mut changed = source.clone();
        changed.version = Some("Remastered".into());
        assert!(!strict_track(&local, &changed));
        changed = source.clone();
        changed.duration_seconds = Some(190.0);
        assert!(!strict_track(&local, &changed));
        let mut local = local;
        local.isrc = Some("USAAA2000001".into());
        changed = source.clone();
        changed.isrc = Some("USAAA2000002".into());
        assert!(!strict_track(&local, &changed));
        changed = source.clone();
        changed.duration_seconds = None;
        assert!(!strict_track(&local, &changed));
        changed.isrc = local.isrc.clone();
        assert!(strict_track(&local, &changed));
        let mut entry:LibraryEntity=serde_json::from_value(json!({"id":"x","kind":"album","name":"An Album","artist":"An Artist","explicitlySaved":false,"trackCount":0,"albumCount":0})).unwrap();
        entry.release_date = Some("2020".into());
        let group = LocalGroup {
            entry: entry.clone(),
            tracks: vec![local],
            assets: vec![],
            embedded: Some("album:ref:musicbrainz:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()),
            fingerprint: String::new(),
        };
        entry.id = "album:ref:musicbrainz:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into();
        let catalog = Catalog {
            entry,
            tracks: vec![source],
            fingerprint: String::new(),
        };
        assert!(!align(&group, &catalog).1);
        let mut group = group;
        let mut catalog = catalog;
        group.embedded = None;
        group.tracks[0].track_number = Some(3);
        catalog.tracks[0].track_number = None;
        assert!(
            !align(&group, &catalog).2,
            "Gaps in numbering cannot prove a complete album"
        );
    }
}
