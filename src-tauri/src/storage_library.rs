//! Durable library saves and a rebuildable projection of saved track membership.
//! Triggers invalidate the projection for every writer, including restore/import.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const LIBRARY_PROJECTION_REVISION: i64 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntity {
    #[serde(default)]
    pub id: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub artist: String,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub source_url: Option<String>,
    pub artwork_url: Option<String>,
    pub release_date: Option<String>,
    pub total_tracks: Option<usize>,
    #[serde(default)]
    pub explicitly_saved: bool,
    #[serde(default)]
    pub track_count: usize,
    #[serde(default)]
    pub album_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_track_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntityPage {
    pub items: Vec<LibraryEntity>,
    pub total: usize,
}

#[derive(Serialize)]
pub struct LibraryTrackPage {
    pub tracks: Vec<Track>,
    pub total: usize,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    let had_artist_albums:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='library_artist_albums')",[],|row|row.get(0)).map_err(err)?;
    let had_local_bindings:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='library_local_album_tracks')",[],|row|row.get(0)).map_err(err)?;
    conn.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS library_entity_saves(id TEXT PRIMARY KEY,kind TEXT NOT NULL CHECK(kind IN ('artist','album')),payload TEXT NOT NULL,explicit_saved INTEGER NOT NULL DEFAULT 1 CHECK(explicit_saved IN (0,1)));
        CREATE TABLE IF NOT EXISTS library_entities(id TEXT PRIMARY KEY,kind TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS library_entity_tracks(entity_id TEXT NOT NULL REFERENCES library_entities(id) ON DELETE CASCADE,listing_id TEXT NOT NULL REFERENCES catalog_listings(id) ON DELETE CASCADE,position INTEGER NOT NULL,PRIMARY KEY(entity_id,position,listing_id));
        CREATE INDEX IF NOT EXISTS library_entity_tracks_listing ON library_entity_tracks(listing_id,entity_id);
        CREATE TABLE IF NOT EXISTS library_artist_albums(artist_id TEXT NOT NULL REFERENCES library_entities(id) ON DELETE CASCADE,album_id TEXT NOT NULL REFERENCES library_entities(id) ON DELETE CASCADE,PRIMARY KEY(artist_id,album_id));
        CREATE INDEX IF NOT EXISTS library_artist_albums_album ON library_artist_albums(album_id,artist_id);
        CREATE TABLE IF NOT EXISTS library_entity_state(id INTEGER PRIMARY KEY CHECK(id=1),dirty INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS library_entity_aliases(id TEXT PRIMARY KEY,kind TEXT NOT NULL,left_ref TEXT NOT NULL,right_ref TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS library_album_reconciliation(local_ref TEXT PRIMARY KEY,catalog_ref TEXT NOT NULL,state TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS library_local_album_tracks(album_id TEXT NOT NULL REFERENCES library_entities(id) ON DELETE CASCADE,position INTEGER NOT NULL,catalog_listing_id TEXT NOT NULL REFERENCES catalog_listings(id) ON DELETE CASCADE,local_listing_id TEXT NOT NULL REFERENCES catalog_listings(id) ON DELETE CASCADE,asset_id TEXT NOT NULL REFERENCES local_assets(id) ON DELETE CASCADE,file_hash TEXT NOT NULL,PRIMARY KEY(album_id,position,asset_id));
        CREATE INDEX IF NOT EXISTS library_local_album_catalog ON library_local_album_tracks(catalog_listing_id,album_id);
        CREATE INDEX IF NOT EXISTS library_local_album_listing ON library_local_album_tracks(local_listing_id);
        CREATE INDEX IF NOT EXISTS library_local_album_asset ON library_local_album_tracks(asset_id);
        INSERT OR IGNORE INTO library_entity_state VALUES(1,1);
        CREATE TABLE IF NOT EXISTS library_entity_projection_revision(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO library_entity_projection_revision VALUES(1,0);
        CREATE TRIGGER IF NOT EXISTS library_entities_listing_insert AFTER INSERT ON catalog_listings WHEN NEW.in_library=1 BEGIN UPDATE library_entity_state SET dirty=1; END;
        DROP TRIGGER IF EXISTS library_entities_listing_update;
        CREATE TRIGGER library_entities_listing_update AFTER UPDATE OF metadata,in_library,recording_id ON catalog_listings WHEN NEW.in_library=1 OR OLD.in_library=1 BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_listing_delete AFTER DELETE ON catalog_listings WHEN OLD.in_library=1 BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_recording_update AFTER UPDATE OF manual_metadata ON recordings BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_save_insert AFTER INSERT ON library_entity_saves BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_save_delete AFTER DELETE ON library_entity_saves BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_save_update AFTER UPDATE ON library_entity_saves BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_asset_insert AFTER INSERT ON local_assets BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_asset_update AFTER UPDATE ON local_assets BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_asset_delete AFTER DELETE ON local_assets BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_autotag_update AFTER UPDATE ON asset_autotags BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_alias_insert AFTER INSERT ON library_entity_aliases BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_mapping_insert AFTER INSERT ON artist_mappings BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_mapping_update AFTER UPDATE ON artist_mappings BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_reconcile_insert AFTER INSERT ON library_album_reconciliation BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_reconcile_update AFTER UPDATE ON library_album_reconciliation BEGIN UPDATE library_entity_state SET dirty=1; END;
        CREATE TRIGGER IF NOT EXISTS library_entities_reconcile_delete AFTER DELETE ON library_album_reconciliation BEGIN UPDATE library_entity_state SET dirty=1; END;
        PRAGMA user_version=15; COMMIT;").map_err(err)?;
    conn.execute_batch("BEGIN IMMEDIATE;").map_err(err)?;
    let projection_revision: i64 = conn
        .query_row(
            "SELECT revision FROM library_entity_projection_revision WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(err)?;
    if !had_artist_albums
        || !had_local_bindings
        || projection_revision != LIBRARY_PROJECTION_REVISION
    {
        conn.execute("UPDATE library_entity_state SET dirty=1", [])
            .map_err(err)?;
    }
    if projection_revision != LIBRARY_PROJECTION_REVISION {
        conn.execute(
            "UPDATE library_entity_projection_revision SET revision=?1 WHERE id=1",
            [LIBRARY_PROJECTION_REVISION],
        )
        .map_err(err)?;
    }
    conn.execute_batch("COMMIT;").map_err(err)?;
    Ok(())
}

fn normalized(value: &str) -> String {
    crate::track_sort_text::lowercase(&value.split_whitespace().collect::<Vec<_>>().join(" "))
}

impl LibraryEntity {
    fn validate(mut self) -> Result<Self> {
        self.name = self.name.trim().to_owned();
        if !matches!(self.kind.as_str(), "artist" | "album")
            || self.name.is_empty()
            || self.name.len() > 4096
        {
            return Err("A library artist or album name is required".into());
        }
        if self.provider.as_deref().is_some_and(|v| !v.is_empty())
            && self.provider_id.as_deref().is_some_and(|v| !v.is_empty())
        {
            self.id = format!(
                "{}:ref:{}:{}",
                self.kind,
                self.provider.as_deref().unwrap(),
                self.provider_id.as_deref().unwrap()
            );
        } else {
            self.provider = None;
            self.provider_id = None;
            self.id = format!(
                "{}:metadata:{}",
                self.kind,
                if self.kind == "artist" {
                    json!([normalized(&self.name)]).to_string()
                } else {
                    json!([
                        normalized(&self.artist),
                        normalized(&self.name),
                        self.release_date.as_deref().unwrap_or("")
                    ])
                    .to_string()
                }
            );
        }
        self.aliases.clear();
        self.explicitly_saved = false;
        self.track_count = 0;
        self.album_count = 0;
        Ok(self)
    }
}

fn entity(
    kind: &str,
    name: &str,
    track: &Track,
    provider: Option<&str>,
    id: Option<&str>,
) -> LibraryEntity {
    LibraryEntity {
        id: String::new(),
        kind: kind.into(),
        name: name.into(),
        artist: track
            .album_artist
            .clone()
            .unwrap_or_else(|| track.artist.clone()),
        provider: provider.map(str::to_owned),
        provider_id: id.map(str::to_owned),
        source_url: None,
        artwork_url: track.artwork_url.clone(),
        release_date: if kind == "album" {
            track.release_date.clone()
        } else {
            None
        },
        total_tracks: if kind == "album" {
            track.track_total.map(|v| v as usize)
        } else {
            None
        },
        explicitly_saved: false,
        track_count: 0,
        album_count: 0,
        saved_track_ids: Vec::new(),
        aliases: Vec::new(),
    }
}

fn string_at<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths.iter().find_map(|path| {
        value
            .pointer(path)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
    })
}

fn has_artist_identity_evidence(provenance: &Value) -> bool {
    string_at(
        provenance,
        &[
            "/catalog/artist/id",
            "/artist/id",
            "/artistId",
            "/embeddedIdentifiers/musicbrainzArtistId",
        ],
    )
    .is_some()
}

fn artist_credit_identity<'a>(
    provider: Option<&'a str>,
    id: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if provider.is_some_and(|value| value.eq_ignore_ascii_case("discogs"))
        && id.is_some_and(|value| value.trim() == "0")
    {
        // Discogs uses 0 as a placeholder for contributor credits without a
        // real artist identity. Keep the credited name, but don't let it
        // create a provider reference that can merge unrelated artists.
        (None, None)
    } else {
        (provider, id)
    }
}

/// Weak title and filename guesses are useful for playback search, but should
/// not create permanent artist rows in the Library projection.
fn artist_fallback_is_trustworthy(track: &Track) -> bool {
    let provenance = &track.provenance;
    if provenance.pointer("/manualFields/artist") == Some(&json!(true))
        || has_artist_identity_evidence(provenance)
    {
        return true;
    }

    let artist_source = string_at(provenance, &["/artistSource"]);
    if artist_source == Some("inferred_from_title") {
        return false;
    }

    // Audio search candidates often contain a complete video title parsed as
    // an artist. Keep their track metadata, but don't project that guess as an
    // artist identity until explicit credits or user/catalog evidence exists.
    if string_at(provenance, &["/sourceKind"]) == Some("audioSearch")
        && string_at(provenance, &["/identity"])
            .is_some_and(|identity| identity.starts_with("unverified"))
    {
        return false;
    }

    // A bare track index from a peer filename is a common parser miss. The
    // provenance check keeps actual numeric stage names from being removed
    // when supplied by a catalog, embedded artist ID, or the user.
    let filename_number = artist_source == Some("filename")
        && track.artist.trim().chars().all(|ch| ch.is_ascii_digit());
    !filename_number
}

/// Only catalog services may publish an edge, after checking an exact public
/// URL relation. Display names are never identity evidence.
impl Store {
    pub(crate) fn link_library_entities(
        &self,
        kind: &str,
        left_provider: &str,
        left_id: &str,
        right_provider: &str,
        right_id: &str,
    ) -> Result<()> {
        if !matches!(kind, "artist" | "album")
            || [left_provider, left_id, right_provider, right_id]
                .iter()
                .any(|v| v.is_empty() || v.len() > 512)
        {
            return Err("Invalid library identity link".into());
        }
        let left = format!("{kind}:ref:{left_provider}:{left_id}");
        let right = format!("{kind}:ref:{right_provider}:{right_id}");
        let conn = self.lock_connection()?;
        conn.execute(
            "INSERT OR IGNORE INTO library_entity_aliases VALUES(?1,?2,?3,?4)",
            params![json!([kind, left, right]).to_string(), kind, left, right],
        )
        .map_err(err)?;
        Ok(())
    }
}

pub(super) fn canonical_refs(
    conn: &Connection,
    kind: &str,
) -> Result<HashMap<String, Vec<String>>> {
    canonical_refs_impl(conn, kind, true)
}
pub(super) fn base_canonical_refs(
    conn: &Connection,
    kind: &str,
) -> Result<HashMap<String, Vec<String>>> {
    canonical_refs_impl(conn, kind, false)
}
fn canonical_refs_impl(
    conn: &Connection,
    kind: &str,
    include_local: bool,
) -> Result<HashMap<String, Vec<String>>> {
    use std::collections::{BTreeSet, VecDeque};
    // Capture verified legacy links durably so backup/cache eviction cannot
    // split an artist already recognized by the Library.
    if kind == "artist" {
        let mappings = conn
            .prepare("SELECT provider,provider_artist_id,artist_id FROM artist_mappings")
            .map_err(err)?
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        for (provider, id, target) in mappings {
            if let Some((other, other_id)) = target.split_once(':') {
                let left = format!("artist:ref:{provider}:{id}");
                let right = format!("artist:ref:{other}:{other_id}");
                conn.execute(
                    "INSERT OR IGNORE INTO library_entity_aliases VALUES(?1,'artist',?2,?3)",
                    params![json!(["artist", left, right]).to_string(), left, right],
                )
                .map_err(err)?;
            }
        }
    }
    let mut graph = HashMap::<String, BTreeSet<String>>::new();
    let mut stmt = conn
        .prepare("SELECT left_ref,right_ref FROM library_entity_aliases WHERE kind=?1")
        .map_err(err)?;
    for row in stmt
        .query_map([kind], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?
    {
        let (left, right) = row.map_err(err)?;
        graph.entry(left.clone()).or_default().insert(right.clone());
        graph.entry(right).or_default().insert(left);
    }
    if kind == "album" && include_local {
        let rows=conn.prepare("SELECT local_ref,catalog_ref,payload FROM library_album_reconciliation WHERE state IN ('auto','confirmed','embedded')").map_err(err)?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
        for (local, target, payload) in rows {
            graph
                .entry(local.clone())
                .or_default()
                .insert(target.clone());
            graph.entry(target).or_default().insert(local.clone());
            if let Ok(payload) = decode::<Value>(payload) {
                if let Some(embedded) = payload
                    .get("embeddedRef")
                    .and_then(Value::as_str)
                    .filter(|id| id.starts_with("album:ref:musicbrainz:"))
                {
                    graph
                        .entry(local.clone())
                        .or_default()
                        .insert(embedded.to_owned());
                    graph.entry(embedded.to_owned()).or_default().insert(local);
                }
            }
        }
    }
    let mut result = HashMap::new();
    for key in graph.keys() {
        if result.contains_key(key) {
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut pending = VecDeque::from([key.clone()]);
        while let Some(node) = pending.pop_front() {
            if seen.insert(node.clone()) {
                if let Some(neighbors) = graph.get(&node) {
                    pending.extend(neighbors.iter().cloned());
                }
            }
        }
        let mut refs = seen.into_iter().collect::<Vec<_>>();
        refs.sort_by_key(|r| {
            (
                if r.contains(":ref:musicbrainz:") {
                    0
                } else if r.contains(":ref:discogs:") {
                    1
                } else if r.contains(":ref:local:") || r.contains(":metadata:") {
                    99
                } else {
                    2
                },
                r.clone(),
            )
        });
        for node in &refs {
            result.insert(node.clone(), refs.clone());
        }
    }
    Ok(result)
}

fn canonicalize(mut entry: LibraryEntity, refs: &HashMap<String, Vec<String>>) -> LibraryEntity {
    if let Some(aliases) = refs.get(&entry.id) {
        entry.id = aliases[0].clone();
        entry.aliases = aliases.clone();
    }
    entry
}

pub(super) fn track_entities(track: &Track) -> Vec<LibraryEntity> {
    let p = &track.provenance;
    let mut result = Vec::new();
    if let Some(credits) = p.get("artistCredits").and_then(Value::as_array) {
        for credit in credits {
            let name = string_at(credit, &["/name", "/artist/name"]);
            let provider = string_at(credit, &["/artist/provider", "/provider"])
                .or_else(|| (track.provider == "spotify").then_some("spotify"));
            let id = string_at(credit, &["/artist/id", "/artistId", "/providerId", "/id"]);
            if let Some(name) = name {
                let (provider, id) = artist_credit_identity(provider, id);
                result.push(entity("artist", name, track, provider, id));
            }
        }
    }
    if result.is_empty() && !track.artist.trim().is_empty() && artist_fallback_is_trustworthy(track)
    {
        let provider = string_at(p, &["/catalog/artist/provider", "/artist/provider"])
            .or_else(|| (track.provider == "spotify").then_some("spotify"));
        let embedded = if track.provider == "local"
            && p.pointer("/manualFields/artist") != Some(&json!(true))
        {
            string_at(p, &["/embeddedIdentifiers/musicbrainzArtistId"])
        } else {
            None
        };
        let id =
            embedded.or_else(|| string_at(p, &["/catalog/artist/id", "/artist/id", "/artistId"]));
        result.push(entity(
            "artist",
            &track.artist,
            track,
            if embedded.is_some() {
                Some("musicbrainz")
            } else {
                provider
            },
            id,
        ));
    }
    if let Some(name) = track.album.as_deref().filter(|v| !v.trim().is_empty()) {
        let provider = string_at(p, &["/libraryAlbum/provider", "/catalog/release/provider"])
            .or_else(|| string_at(p, &["/catalogProvider"]))
            .or_else(|| {
                (track.provider != "local" && track.provider != "catalog")
                    .then_some(track.provider.as_str())
            });
        let id = string_at(
            p,
            &[
                "/libraryAlbum/id",
                "/recordingContext/albumId",
                "/albumId",
                "/catalog/release/id",
                "/releaseId",
            ],
        );
        if track.provider == "local" {
            use sha2::Digest;
            let key = json!([
                normalized(track.album_artist.as_deref().unwrap_or(&track.artist)),
                normalized(name),
                track.release_date,
                track
                    .provenance
                    .pointer("/embeddedIdentifiers/musicbrainzReleaseId")
            ]);
            let local_id = format!("{:x}", sha2::Sha256::digest(key.to_string().as_bytes()));
            result.push(entity("album", name, track, Some("local"), Some(&local_id)));
        } else {
            result.push(entity("album", name, track, provider, id));
        }
    }
    result
        .into_iter()
        .filter_map(|e| e.validate().ok())
        .collect()
}

pub(super) fn refresh(conn: &Connection) -> Result<()> {
    let dirty: bool = conn
        .query_row(
            "SELECT dirty FROM library_entity_state WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    if !dirty {
        return Ok(());
    }
    conn.execute_batch("SAVEPOINT library_projection")
        .map_err(err)?;
    let result = (|| {
        let raw_tracks=conn.prepare("SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1 ORDER BY l.id").map_err(err)?.query_map([],|r|r.get::<_,String>(0)).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
        let tracks = hydrate_page_tracks(
            conn,
            raw_tracks
                .into_iter()
                .map(decode)
                .collect::<Result<Vec<Track>>>()?,
        )?;
        let local_resolutions = super::local_albums::reconcile(conn, &tracks)?;
        let artist_refs = canonical_refs(conn, "artist")?;
        let album_refs = canonical_refs(conn, "album")?;
        let canonical = |entry: LibraryEntity| {
            let refs = if entry.kind == "artist" {
                &artist_refs
            } else {
                &album_refs
            };
            canonicalize(entry, refs)
        };
        let raws = conn
            .prepare("SELECT payload,explicit_saved FROM library_entity_saves ORDER BY id")
            .map_err(err)?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        let mut saved = Vec::new();
        for (raw, active) in raws {
            let mut entry: LibraryEntity = decode(raw)?;
            entry.explicitly_saved = active;
            saved.push(canonical(entry));
        }
        let grouped = tracks
            .into_iter()
            .map(|track| {
                let entries = track_entities(&track)
                    .into_iter()
                    .map(&canonical)
                    .collect::<Vec<_>>();
                (track, entries)
            })
            .collect::<Vec<_>>();
        let mut known_artists = HashMap::<String, LibraryEntity>::new();
        let mut named_refs = HashMap::<String, std::collections::BTreeSet<String>>::new();
        for entry in saved
            .iter()
            .chain(grouped.iter().flat_map(|(_, entries)| entries.iter()))
        {
            if entry.kind == "artist" && entry.provider_id.is_some() {
                known_artists
                    .entry(entry.id.clone())
                    .or_insert_with(|| entry.clone());
                named_refs
                    .entry(normalized(&entry.name))
                    .or_default()
                    .insert(entry.id.clone());
            }
        }
        let mut explicit_album_track_ids =
            HashMap::<String, std::collections::HashSet<String>>::new();
        let mut saved_track_listings = HashMap::<String, Option<String>>::new();
        for entry in saved.iter().filter(|entry| entry.kind == "album") {
            let track_ids = explicit_album_track_ids
                .entry(entry.id.clone())
                .or_default();
            for saved_track_id in &entry.saved_track_ids {
                if !saved_track_listings.contains_key(saved_track_id) {
                    let listing: Option<String> = conn
                        .query_row(
                            "SELECT c.id FROM catalog_listings l JOIN catalog_listings c ON c.id=l.recording_id AND c.id=c.recording_id WHERE l.id=?1 AND c.in_library=1",
                            [saved_track_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(err)?;
                    saved_track_listings.insert(saved_track_id.clone(), listing);
                }
                if let Some(listing) = saved_track_listings
                    .get(saved_track_id)
                    .and_then(Option::as_ref)
                {
                    track_ids.insert(listing.clone());
                }
            }
        }
        conn.execute_batch("DELETE FROM library_local_album_tracks; DELETE FROM library_artist_albums; DELETE FROM library_entity_tracks; DELETE FROM library_entities;")
            .map_err(err)?;
        for entry in saved.iter().filter(|e| e.explicitly_saved) {
            conn.execute(
                "INSERT OR IGNORE INTO library_entities VALUES(?1,?2,?3)",
                params![entry.id, entry.kind, encode(entry)?],
            )
            .map_err(err)?;
        }
        for (track, entries) in grouped {
            for mut entry in entries {
                if entry.kind == "artist" && entry.provider_id.is_none() {
                    if let Some(ids) = named_refs
                        .get(&normalized(&entry.name))
                        .filter(|ids| ids.len() == 1)
                    {
                        if let Some(known) = known_artists.get(ids.first().unwrap()) {
                            entry = known.clone();
                            entry.explicitly_saved = false;
                        }
                    }
                }
                conn.execute(
                    "INSERT OR IGNORE INTO library_entities VALUES(?1,?2,?3)",
                    params![entry.id, entry.kind, encode(&entry)?],
                )
                .map_err(err)?;
                // An explicit album save carries the authoritative ordered
                // occurrences for those listings. Do not also add the
                // track-derived `libraryAlbum` membership: its track-number
                // position is a second row for the same occurrence and
                // inflates counts. Unlisted tracks with the same verified
                // album identity remain eligible for the grouped projection.
                if entry.kind == "album"
                    && explicit_album_track_ids
                        .get(&entry.id)
                        .is_some_and(|ids| ids.contains(&track.id))
                {
                    continue;
                }
                let position = i64::from(track.disc_number.unwrap_or(1)) * 1_000_000
                    + i64::from(track.track_number.unwrap_or(999_999));
                conn.execute(
                    "INSERT OR IGNORE INTO library_entity_tracks VALUES(?1,?2,?3)",
                    params![entry.id, track.id, position],
                )
                .map_err(err)?;
            }
        }
        // A verified same-edition link chooses one occurrence at each album
        // position, instead of doubling the tracklist for two provider saves.
        for album in saved.iter().filter(|e| e.kind == "album") {
            for (position, id) in album.saved_track_ids.iter().enumerate() {
                let Some(listing) = saved_track_listings.get(id).and_then(Option::as_ref) else {
                    continue;
                };
                conn.execute(
                    "INSERT OR IGNORE INTO library_entities VALUES(?1,'album',?2)",
                    params![album.id, encode(album)?],
                )
                .map_err(err)?;
                conn.execute("INSERT INTO library_entity_tracks SELECT ?1,?2,?3 WHERE NOT EXISTS(SELECT 1 FROM library_entity_tracks WHERE entity_id=?1 AND position=?3)",params![album.id,listing,position as i64]).map_err(err)?;
                conn.execute("INSERT OR IGNORE INTO library_artist_albums SELECT a.entity_id,?1 FROM library_entity_tracks a JOIN library_entities artist ON artist.id=a.entity_id AND artist.kind='artist' WHERE a.listing_id=?2",params![album.id,listing]).map_err(err)?;
            }
        }
        conn.execute("INSERT OR IGNORE INTO library_artist_albums SELECT a.entity_id,b.entity_id FROM library_entity_tracks a JOIN library_entities artist ON artist.id=a.entity_id AND artist.kind='artist' JOIN library_entity_tracks b ON b.listing_id=a.listing_id JOIN library_entities album ON album.id=b.entity_id AND album.kind='album'",[]).map_err(err)?;
        super::local_albums::populate_bindings(conn, &local_resolutions)?;
        conn.execute("UPDATE library_entity_state SET dirty=0", [])
            .map_err(err)?;
        Ok(())
    })();
    if result.is_ok() {
        conn.execute_batch("RELEASE library_projection")
            .map_err(err)?;
    } else {
        let _ = conn.execute_batch("ROLLBACK TO library_projection; RELEASE library_projection");
    }
    result
}

impl Store {
    /// Provider-page buttons need explicit save state without rebuilding the
    /// potentially large Library grouping projection during catalog browsing.
    pub fn library_saved_entity_status(&self, id: &str) -> Result<Option<LibraryEntity>> {
        let conn = self.lock_connection()?;
        let kind = if id.starts_with("artist:") {
            "artist"
        } else {
            "album"
        };
        let refs = canonical_refs(&conn, kind)?;
        let ids = refs.get(id).cloned().unwrap_or_else(|| vec![id.to_owned()]);
        for id in ids {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT payload FROM library_entity_saves WHERE id=?1 AND explicit_saved=1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if let Some(raw) = raw {
                let mut entry: LibraryEntity = decode(raw)?;
                entry.explicitly_saved = true;
                entry.saved_track_ids.clear();
                return Ok(Some(canonicalize(entry, &refs)));
            }
        }
        Ok(None)
    }

    pub fn library_entity_status(&self, id: &str) -> Result<Option<LibraryEntity>> {
        let conn = self.lock_connection()?;
        refresh(&conn)?;
        let refs = canonical_refs(
            &conn,
            if id.starts_with("artist:") {
                "artist"
            } else {
                "album"
            },
        )?;
        let id = refs
            .get(id)
            .map_or_else(|| id.to_owned(), |refs| refs[0].clone());
        let raw: Option<String> = conn
            .query_row(
                "SELECT payload FROM library_entities WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        raw.map(|raw| {
            let mut entry: LibraryEntity = decode(raw)?;
            entry.track_count = conn
                .query_row(
                    "SELECT COUNT(*) FROM library_entity_tracks WHERE entity_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(err)?;
            entry.saved_track_ids.clear();
            Ok(entry)
        })
        .transpose()
    }

    pub fn list_library_entities(
        &self,
        kind: &str,
        artist_id: Option<&str>,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<LibraryEntityPage> {
        self.list_library_entities_filtered(
            kind, artist_id, query, None, None, "name", false, limit, offset,
        )
    }

    pub fn list_library_entities_filtered(
        &self,
        kind: &str,
        artist_id: Option<&str>,
        query: &str,
        source: Option<&str>,
        year: Option<&str>,
        sort: &str,
        descending: bool,
        limit: usize,
        offset: usize,
    ) -> Result<LibraryEntityPage> {
        if !matches!(kind, "artist" | "album") {
            return Err("Unknown library entity kind".into());
        }
        let conn = self.lock_connection()?;
        refresh(&conn)?;
        let artist_refs = canonical_refs(&conn, "artist")?;
        let artist_id = artist_id.map(|id| {
            artist_refs
                .get(id)
                .map_or_else(|| id.to_owned(), |refs| refs[0].clone())
        });
        let filter = "e.kind=?1 AND instr(unicode_lower(json_extract(e.payload,'$.name') || ' ' || json_extract(e.payload,'$.artist') || ' ' || COALESCE(json_extract(e.payload,'$.releaseDate'),'') || ' ' || COALESCE(json_extract(e.payload,'$.provider'),'')),?2)>0 AND (?3 IS NULL OR EXISTS(SELECT 1 FROM library_artist_albums membership WHERE membership.artist_id=?3 AND membership.album_id=e.id)) AND (?4 IS NULL OR (?4='__metadata__' AND json_extract(e.payload,'$.provider') IS NULL) OR unicode_lower(COALESCE(json_extract(e.payload,'$.provider'),''))=unicode_lower(?4)) AND (?5 IS NULL OR substr(COALESCE(json_extract(e.payload,'$.releaseDate'),''),1,4)=?5)";
        let query = normalized(query);
        let source = source.filter(|value| !value.trim().is_empty());
        let year = year
            .filter(|value| value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit()));
        let order = match sort {
            "artist" => "unicode_lower(json_extract(e.payload,'$.artist'))",
            "year" => "COALESCE(json_extract(e.payload,'$.releaseDate'),'')",
            "source" => "unicode_lower(COALESCE(json_extract(e.payload,'$.provider'),''))",
            "tracks" => "(SELECT COUNT(*) FROM library_entity_tracks t WHERE t.entity_id=e.id)",
            _ => "unicode_lower(json_extract(e.payload,'$.name'))",
        };
        let direction = if descending { "DESC" } else { "ASC" };
        let total = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM library_entities e WHERE {filter}"),
                params![kind, query, artist_id, source, year],
                |r| r.get(0),
            )
            .map_err(err)?;
        let mut stmt = conn.prepare(&format!("SELECT e.payload,COALESCE(json_extract(e.payload,'$.explicitlySaved'),0),(SELECT COUNT(*) FROM library_entity_tracks t WHERE t.entity_id=e.id),(SELECT COUNT(*) FROM library_artist_albums membership WHERE membership.artist_id=e.id) FROM library_entities e WHERE {filter} ORDER BY {order} {direction},unicode_lower(json_extract(e.payload,'$.name')),e.id LIMIT ?6 OFFSET ?7")).map_err(err)?;
        let rows = stmt
            .query_map(
                params![
                    kind,
                    query,
                    artist_id,
                    source,
                    year,
                    limit.clamp(1, 200) as i64,
                    offset as i64
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, usize>(2)?,
                        r.get::<_, usize>(3)?,
                    ))
                },
            )
            .map_err(err)?;
        let mut items = Vec::new();
        for row in rows {
            let (raw, saved, tracks, albums) = row.map_err(err)?;
            let mut e: LibraryEntity = decode(raw)?;
            e.explicitly_saved = saved;
            e.track_count = tracks;
            e.album_count = albums;
            e.saved_track_ids.clear();
            items.push(e);
        }
        Ok(LibraryEntityPage { items, total })
    }

    pub fn list_library_entity_tracks(
        &self,
        id: &str,
        query: &str,
        sort: &str,
        descending: bool,
        availability: &str,
        min_duration: Option<f64>,
        max_duration: Option<f64>,
        limit: usize,
        offset: usize,
    ) -> Result<LibraryTrackPage> {
        let conn = self.lock_connection()?;
        refresh(&conn)?;
        let kind = if id.starts_with("artist:") {
            "artist"
        } else {
            "album"
        };
        let refs = canonical_refs(&conn, kind)?;
        let id = refs
            .get(id)
            .map_or_else(|| id.to_owned(), |refs| refs[0].clone());
        let metadata = "CASE WHEN r.manual_metadata='{}' THEN l.metadata ELSE json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')) END";
        conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS library_present_recordings(recording_id TEXT PRIMARY KEY); DELETE FROM library_present_recordings;").map_err(err)?;
        if availability != "all" {
            let paths=conn.prepare("SELECT recording_id,path FROM local_assets UNION ALL SELECT b.catalog_listing_id,a.path FROM library_local_album_tracks b JOIN local_assets a ON a.id=b.asset_id AND a.hash=b.file_hash").map_err(err)?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
            for (recording, path) in paths {
                if Path::new(&path).is_file() {
                    conn.execute(
                        "INSERT OR IGNORE INTO library_present_recordings VALUES(?1)",
                        [recording],
                    )
                    .map_err(err)?;
                }
            }
        }
        let local = "EXISTS(SELECT 1 FROM library_present_recordings a WHERE a.recording_id=l.recording_id)";
        let duration = format!("json_extract({metadata},'$.durationSeconds')");
        let filter = format!("t.entity_id=?1 AND instr(unicode_lower(json_extract({metadata},'$.title') || ' ' || json_extract({metadata},'$.artist') || ' ' || COALESCE(json_extract({metadata},'$.album'),'')),?2)>0 AND (?3='all' OR (?3='local')={local}) AND (?4 IS NULL OR {duration}>=?4) AND (?5 IS NULL OR {duration}<=?5)");
        let from = "FROM library_entity_tracks t JOIN catalog_listings l ON l.id=t.listing_id JOIN recordings r ON r.id=l.recording_id";
        let q = normalized(query);
        let total = conn
            .query_row(
                &format!("SELECT COUNT(*) {from} WHERE {filter}"),
                params![id, q, availability, min_duration, max_duration],
                |r| r.get(0),
            )
            .map_err(err)?;
        let column = match sort {
            "title" => "title",
            "artist" => "artist",
            "album" => "album",
            "releaseDate" => "releaseDate",
            "duration" => "durationSeconds",
            _ => "",
        };
        let expression = if column.is_empty() {
            "t.position".to_owned()
        } else if column == "durationSeconds" {
            duration
        } else {
            format!("unicode_lower(json_extract({metadata},'$.{column}'))")
        };
        let direction = if descending { "DESC" } else { "ASC" };
        let mut stmt = conn.prepare(&format!("SELECT l.metadata,t.position,l.id {from} WHERE {filter} ORDER BY {expression} {direction},l.id LIMIT ?6 OFFSET ?7")).map_err(err)?;
        let raws = stmt
            .query_map(
                params![
                    id,
                    q,
                    availability,
                    min_duration,
                    max_duration,
                    limit.clamp(1, 200) as i64,
                    offset as i64
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        let identities = raws
            .iter()
            .map(|(_, position, id)| (*position, id.clone()))
            .collect::<Vec<_>>();
        let mut tracks = hydrate_page_tracks(
            &conn,
            raws.into_iter()
                .map(|(raw, _, _)| raw)
                .map(decode)
                .collect::<Result<Vec<Track>>>()?,
        )?;
        for (track, (position, listing)) in tracks.iter_mut().zip(identities) {
            super::local_albums::overlay(&conn, &id, &listing, position, track)?;
        }
        Ok(LibraryTrackPage { tracks, total })
    }

    pub fn save_library_artist(&self, entry: LibraryEntity) -> Result<LibraryEntity> {
        let entry = entry.validate()?;
        if entry.kind != "artist" {
            return Err("Expected an artist".into());
        }
        let conn = self.lock_connection()?;
        conn.execute("INSERT INTO library_entity_saves(id,kind,payload,explicit_saved) VALUES(?1,?2,?3,1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,explicit_saved=1", params![entry.id,entry.kind,encode(&entry)?]).map_err(err)?;
        let refs = canonical_refs(&conn, "artist")?;
        Ok(canonicalize(
            LibraryEntity {
                explicitly_saved: true,
                ..entry
            },
            &refs,
        ))
    }

    pub fn remove_library_entity_save(&self, id: &str) -> Result<()> {
        let kind = if id.starts_with("artist:") {
            "artist"
        } else {
            "album"
        };
        let conn = self.lock_connection()?;
        let refs = canonical_refs(&conn, kind)?;
        let ids = refs.get(id).cloned().unwrap_or_else(|| vec![id.to_owned()]);
        let tx = conn.unchecked_transaction().map_err(err)?;
        for id in ids {
            tx.execute(
                "UPDATE library_entity_saves SET explicit_saved=0 WHERE id=?1",
                [id],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(())
    }

    pub fn save_library_album(
        &self,
        entry: LibraryEntity,
        tracks: Vec<Track>,
        expected_count: usize,
    ) -> Result<LibraryEntity> {
        let mut entry = entry;
        if let Some(first) = tracks.first() {
            if let (Some(provider), Some(id)) = (
                first
                    .provenance
                    .get("catalogProvider")
                    .and_then(Value::as_str),
                first.provenance.get("releaseId").and_then(Value::as_str),
            ) {
                if entry.provider.as_deref() == Some(provider)
                    && tracks
                        .iter()
                        .all(|t| t.provenance.get("releaseId").and_then(Value::as_str) == Some(id))
                {
                    entry.provider_id = Some(id.to_owned());
                    entry.source_url = Some(first.source_url.clone());
                    if first.release_date.is_some() {
                        entry.release_date = first.release_date.clone();
                    }
                }
            }
        }
        let mut entry = entry.validate()?;
        if entry.kind != "album"
            || tracks.is_empty()
            || tracks.len() > 10_000
            || expected_count != tracks.len()
            || entry
                .total_tracks
                .is_some_and(|total| total != expected_count)
        {
            return Err("The complete album tracklist is required; retry loading the album or save loaded tracks instead".into());
        }
        for track in &tracks {
            if track.provider.is_empty()
                || track.provider_id.is_empty()
                || track.id != format!("{}:{}", track.provider, track.provider_id)
            {
                return Err("Invalid album track identity".into());
            }
        }
        entry.saved_track_ids = tracks.iter().map(|t| t.id.clone()).collect();
        entry.total_tracks = Some(expected_count);
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        for mut track in tracks {
            if !track.provenance.is_object() {
                track.provenance = json!({});
            }
            track.provenance["libraryAlbum"] =
                json!({"provider":entry.provider,"id":entry.provider_id});
            track.album = Some(entry.name.clone());
            track.album_artist = Some(entry.artist.clone());
            track.release_date = entry.release_date.clone();
            track.track_total = Some(expected_count as u32);
            if track.track_number.is_none() {
                track.track_number = track
                    .provenance
                    .get("position")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok());
            }
            upsert_track(&tx, &track, false)?;
            promote_track(&tx, &track.id)?;
        }
        tx.execute("INSERT INTO library_entity_saves(id,kind,payload,explicit_saved) VALUES(?1,?2,?3,1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,explicit_saved=1",params![entry.id,entry.kind,encode(&entry)?]).map_err(err)?;
        tx.commit().map_err(err)?;
        let refs = canonical_refs(&conn, "album")?;
        Ok(canonicalize(
            LibraryEntity {
                explicitly_saved: true,
                ..entry
            },
            &refs,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: format!("fixture:{id}"),
            provider: "fixture".into(),
            provider_id: id.into(),
            title: id.into(),
            artist: "Main Artist".into(),
            album: Some("Album".into()),
            album_artist: Some("Main Artist".into()),
            ..Default::default()
        }
    }
    fn db() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        (dir, store)
    }

    #[test]
    fn artist_projection_rejects_unverified_title_and_numeric_filename_guesses() {
        let mut filename_index = track("filename-index");
        filename_index.album = None;
        filename_index.artist = "01".into();
        filename_index.provenance = json!({
            "artistSource": "filename",
            "identity": "unverified peer filename"
        });
        assert!(track_entities(&filename_index)
            .iter()
            .all(|entity| entity.kind != "artist"));

        let mut title_inference = track("title-inference");
        title_inference.album = None;
        title_inference.artist = "\"500 Miles\"".into();
        title_inference.provenance = json!({
            "sourceKind": "audioSearch",
            "artistSource": "inferred_from_title",
            "identity": "unverified source search result"
        });
        assert!(track_entities(&title_inference)
            .iter()
            .all(|entity| entity.kind != "artist"));

        let mut playlist_title = track("playlist-title");
        playlist_title.album = None;
        playlist_title.artist = "06 Miami 2017 (Billy Joel Cover)".into();
        playlist_title.provenance = json!({
            "sourceKind": "audioSearch",
            "identity": "unverified playlist audio source"
        });
        assert!(track_entities(&playlist_title)
            .iter()
            .all(|entity| entity.kind != "artist"));
    }

    #[test]
    fn artist_projection_keeps_numeric_and_composite_names_with_explicit_evidence() {
        let mut manual_numeric_name = track("manual-numeric-name");
        manual_numeric_name.album = None;
        manual_numeric_name.artist = "01".into();
        manual_numeric_name.provenance =
            json!({"artistSource":"filename","manualFields":{"artist":true}});
        let artist = track_entities(&manual_numeric_name)
            .into_iter()
            .find(|entity| entity.kind == "artist")
            .expect("manual numeric stage name remains visible");
        assert_eq!(artist.name, "01");

        let mut credited_numeric_name = track("credited-numeric-name");
        credited_numeric_name.album = None;
        credited_numeric_name.artist = "Fallback".into();
        credited_numeric_name.provenance = json!({
            "artistCredits":[{"name":"10","artist":{"provider":"musicbrainz","id":"numeric-stage"}}]
        });
        let credited = track_entities(&credited_numeric_name)
            .into_iter()
            .find(|entity| entity.kind == "artist")
            .expect("provider-backed numeric credit remains visible");
        assert_eq!(credited.name, "10");
        assert_eq!(credited.provider_id.as_deref(), Some("numeric-stage"));

        let mut composite_band = track("composite-band");
        composite_band.album = None;
        composite_band.artist = "Earth, Wind & Fire".into();
        composite_band.provenance = json!({"manualFields":{"artist":true}});
        let band = track_entities(&composite_band)
            .into_iter()
            .find(|entity| entity.kind == "artist")
            .expect("atomic composite band credit remains visible");
        assert_eq!(band.name, "Earth, Wind & Fire");
    }

    #[test]
    fn discogs_placeholder_artist_id_does_not_split_a_valid_credit_identity() {
        let (_dir, store) = db();
        let mut credited = track("discogs-placeholder-credit");
        credited.artist = "Unknown artist".into();
        credited.provenance = json!({
            "artistCredits":[
                {"name":"Gillian Welch","role":"Artist","artist":{"provider":"discogs","id":"255403"}},
                {"name":"Gillian Welch","role":"Written By","artist":{"provider":"discogs","id":"0"}}
            ]
        });
        store.save_library_tracks(&[credited.clone()]).unwrap();

        let artists = store
            .list_library_entities("artist", None, "", 200, 0)
            .unwrap();
        assert_eq!(artists.total, 1);
        let artist = &artists.items[0];
        assert_eq!(artist.name, "Gillian Welch");
        assert_eq!(artist.provider.as_deref(), Some("discogs"));
        assert_eq!(artist.provider_id.as_deref(), Some("255403"));
        assert_eq!(artist.track_count, 1);
        assert_eq!(artist.album_count, 1);

        let albums = store
            .list_library_entities("album", Some(&artist.id), "", 200, 0)
            .unwrap();
        assert_eq!(
            albums.total, 1,
            "the valid performer keeps album membership"
        );

        let conn = store.lock_connection().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM library_entities WHERE id='artist:ref:discogs:0'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap(),
            0,
            "placeholder Discogs ID 0 must not create an artist identity",
        );
        let stored: String = conn
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&credited.id],
                |row| row.get(0),
            )
            .unwrap();
        let stored_track: Track = decode(stored).unwrap();
        assert_eq!(
            stored_track.provenance["artistCredits"][1]["role"],
            "Written By"
        );
        assert_eq!(
            stored_track.provenance["artistCredits"][1]["artist"]["id"],
            "0"
        );
    }

    #[test]
    fn projection_revision_rebuilds_clean_old_artist_rows_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let mut old_track = track("old-filename-artist");
        old_track.album = None;
        old_track.artist = "01".into();
        old_track.provenance = json!({
            "artistSource": "filename",
            "identity": "unverified peer filename"
        });

        {
            let store = Store::open(&path).unwrap();
            store.save_library_tracks(&[old_track.clone()]).unwrap();
            let stale_artist = entity("artist", "01", &old_track, None, None)
                .validate()
                .unwrap();
            let conn = store.lock_connection().unwrap();
            conn.execute(
                "INSERT INTO library_entities(id,kind,payload) VALUES(?1,'artist',?2)",
                params![stale_artist.id, encode(&stale_artist).unwrap()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO library_entity_tracks(entity_id,listing_id,position) VALUES(?1,?2,1)",
                params![stale_artist.id, old_track.id],
            )
            .unwrap();
            // Model a clean projection built by the prior evaluator.
            conn.execute("UPDATE library_entity_state SET dirty=0", [])
                .unwrap();
            conn.execute(
                "UPDATE library_entity_projection_revision SET revision=0 WHERE id=1",
                [],
            )
            .unwrap();
        }

        let reopened = Store::open(&path).unwrap();
        {
            let conn = reopened.lock_connection().unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT dirty FROM library_entity_state WHERE id=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1,
                "a clean projection from an older evaluator must be rebuilt",
            );
        }
        assert_eq!(
            reopened
                .list_library_entities("artist", None, "", 200, 0)
                .unwrap()
                .total,
            0,
            "the weak filename-derived artist should disappear during rebuild",
        );
        drop(reopened);

        let reopened_again = Store::open(&path).unwrap();
        let conn = reopened_again.lock_connection().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT dirty FROM library_entity_state WHERE id=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0,
            "the current evaluator revision must not dirty the projection on every open",
        );
    }

    #[test]
    fn saved_groups_exclude_cached_tracks_and_follow_edits_and_cleanup() {
        let (_dir, store) = db();
        let cached = track("cached");
        store.cache_track(&cached).unwrap();
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            0
        );
        let saved = track("saved");
        store.save_library_tracks(&[saved.clone()]).unwrap();
        let albums = store
            .list_library_entities("album", None, "", 200, 0)
            .unwrap();
        assert_eq!(albums.total, 1);
        assert_eq!(albums.items[0].track_count, 1);
        assert_eq!(
            store
                .list_library_entities_filtered(
                    "album",
                    None,
                    "",
                    Some("__metadata__"),
                    None,
                    "name",
                    false,
                    200,
                    0,
                )
                .unwrap()
                .total,
            1
        );
        {
            let conn = store.lock_connection().unwrap();
            conn.execute(
                "UPDATE catalog_listings SET sort_projection_revision=sort_projection_revision",
                [],
            )
            .unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT dirty FROM library_entity_state WHERE id=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        let mut edited = saved.clone();
        edited.album = Some("Renamed album".into());
        store.update_track(&edited).unwrap();
        let albums = store
            .list_library_entities("album", None, "", 200, 0)
            .unwrap();
        assert_eq!(albums.items[0].name, "Renamed album");
        store
            .lock_connection()
            .unwrap()
            .execute("UPDATE catalog_listings SET in_library=0", [])
            .unwrap();
        assert_eq!(
            store
                .list_library_entities("artist", None, "", 200, 0)
                .unwrap()
                .total,
            0
        );
    }

    #[test]
    fn explicit_artist_survives_restart_and_unsaving_keeps_saved_music() {
        let (dir, store) = db();
        let t = track("one");
        let artist = entity(
            "artist",
            "Main Artist",
            &t,
            Some("spotify"),
            Some("artist-1"),
        );
        let artist = store.save_library_artist(artist).unwrap();
        assert_eq!(
            store
                .list_library_entities("artist", None, "", 200, 0)
                .unwrap()
                .items[0]
                .track_count,
            0
        );
        store.save_library_tracks(&[t]).unwrap();
        let page = store
            .list_library_entities("artist", None, "", 200, 0)
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].track_count, 1);
        drop(store);
        let reopened = Store::open(&dir.path().join("library.sqlite")).unwrap();
        assert!(
            reopened
                .list_library_entities("artist", None, "", 200, 0)
                .unwrap()
                .items[0]
                .explicitly_saved
        );
        reopened.remove_library_entity_save(&artist.id).unwrap();
        let page = reopened
            .list_library_entities("artist", None, "", 200, 0)
            .unwrap();
        assert_eq!(page.items[0].track_count, 1);
        assert!(!page.items[0].explicitly_saved);
    }

    #[test]
    fn complete_album_save_is_atomic_and_unsaving_preserves_tracks() {
        let (_dir, store) = db();
        let one = track("one");
        let mut album = entity("album", "Album", &one, Some("spotify"), Some("album-1"));
        album.total_tracks = Some(2);
        assert!(store
            .save_library_album(album.clone(), vec![one.clone()], 2)
            .is_err());
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 0);
        let mut invalid = track("bad");
        invalid.id = "invalid".into();
        assert!(store
            .save_library_album(album.clone(), vec![one.clone(), invalid], 2)
            .is_err());
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 0);
        let saved = store
            .save_library_album(album, vec![one, track("two")], 2)
            .unwrap();
        let albums = store
            .list_library_entities("album", None, "", 200, 0)
            .unwrap();
        assert_eq!(albums.total, 1);
        assert_eq!(albums.items[0].id, saved.id);
        assert_eq!(albums.items[0].track_count, 2);
        store.remove_library_entity_save(&saved.id).unwrap();
        assert!(
            !store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .items[0]
                .explicitly_saved
        );
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 2);
    }

    #[test]
    fn saved_album_preserves_repeated_track_occurrences_and_distinct_editions() {
        let (_dir, store) = db();
        let t = track("repeated");
        let mut album = entity(
            "album",
            "First edition",
            &t,
            Some("spotify"),
            Some("edition-one"),
        );
        album.total_tracks = Some(2);
        let first = store
            .save_library_album(album, vec![t.clone(), t.clone()], 2)
            .unwrap();
        let second = entity(
            "album",
            "Other edition",
            &t,
            Some("spotify"),
            Some("edition-two"),
        );
        let second = store.save_library_album(second, vec![t], 1).unwrap();
        let page = store
            .list_library_entity_tracks(
                &first.id,
                "",
                "sourceOrder",
                false,
                "all",
                None,
                None,
                200,
                0,
            )
            .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.tracks[0].id, page.tracks[1].id);
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            2
        );
        store.remove_library_entity_save(&first.id).unwrap();
        assert_eq!(
            store
                .list_library_entity_tracks(
                    &first.id,
                    "",
                    "sourceOrder",
                    false,
                    "all",
                    None,
                    None,
                    200,
                    0
                )
                .unwrap()
                .total,
            2
        );
        assert_eq!(
            store
                .list_library_entity_tracks(
                    &second.id,
                    "",
                    "sourceOrder",
                    false,
                    "all",
                    None,
                    None,
                    200,
                    0
                )
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn saved_album_does_not_hide_an_unlinked_same_title_release_using_a_shared_track() {
        let (_dir, store) = db();
        let mut shared = track("shared-recording");
        shared.release_date = Some("1998-01-01".into());
        shared.provenance =
            json!({"catalog":{"release":{"provider":"musicbrainz","id":"other-edition"}}});
        let saved = store
            .save_library_album(
                entity(
                    "album",
                    "Same title",
                    &shared,
                    Some("spotify"),
                    Some("saved-edition"),
                ),
                vec![shared.clone()],
                1,
            )
            .unwrap();

        // The saved release and listing metadata can legitimately describe two
        // unlinked editions of the same recording. Keep the listing's catalog
        // release evidence while retaining the explicit save as its own entity.
        {
            let conn = store.lock_connection().unwrap();
            let raw: String = conn
                .query_row(
                    "SELECT metadata FROM catalog_listings WHERE id=?1",
                    [&shared.id],
                    |row| row.get(0),
                )
                .unwrap();
            let mut listing: Track = decode(raw).unwrap();
            listing
                .provenance
                .as_object_mut()
                .unwrap()
                .remove("libraryAlbum");
            conn.execute(
                "UPDATE catalog_listings SET metadata=?2 WHERE id=?1",
                params![shared.id, encode(&listing).unwrap()],
            )
            .unwrap();
        }

        let albums = store
            .list_library_entities("album", None, "", 200, 0)
            .unwrap();
        assert_eq!(albums.total, 2);
        assert!(albums.items.iter().any(|album| album.id == saved.id));
        assert!(albums
            .items
            .iter()
            .any(|album| album.id == "album:ref:musicbrainz:other-edition"));
        assert_eq!(
            store
                .list_library_entities("album", None, "1998", 200, 0)
                .unwrap()
                .total,
            2
        );
        assert_eq!(
            store
                .list_library_entities("album", None, "musicbrainz", 200, 0)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            store
                .list_library_entities_filtered(
                    "album",
                    None,
                    "",
                    Some("spotify"),
                    Some("1998"),
                    "year",
                    false,
                    200,
                    0,
                )
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            store
                .list_library_entities_filtered(
                    "album",
                    None,
                    "",
                    None,
                    Some("1998"),
                    "source",
                    true,
                    200,
                    0,
                )
                .unwrap()
                .items[0]
                .id,
            saved.id
        );
    }

    #[test]
    fn verified_artist_links_merge_saves_and_track_membership_but_names_do_not() {
        let (_dir, store) = db();
        let t = track("one");
        let spotify = store
            .save_library_artist(entity(
                "artist",
                "Shared name",
                &t,
                Some("spotify"),
                Some("one"),
            ))
            .unwrap();
        let native = store
            .save_library_artist(entity(
                "artist",
                "Native spelling",
                &t,
                Some("musicbrainz"),
                Some("native-one"),
            ))
            .unwrap();
        assert!(
            store
                .library_saved_entity_status(&spotify.id)
                .unwrap()
                .unwrap()
                .explicitly_saved
        );
        assert_eq!(
            store
                .lock_connection()
                .unwrap()
                .query_row(
                    "SELECT dirty FROM library_entity_state WHERE id=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        store
            .save_library_artist(entity(
                "artist",
                "Shared name",
                &t,
                Some("spotify"),
                Some("unrelated"),
            ))
            .unwrap();
        assert_eq!(
            store
                .list_library_entities("artist", None, "", 200, 0)
                .unwrap()
                .total,
            3
        );
        store
            .link_library_entities("artist", "spotify", "one", "musicbrainz", "native-one")
            .unwrap();
        let artists = store
            .list_library_entities("artist", None, "", 200, 0)
            .unwrap();
        assert_eq!(artists.total, 2);
        let merged = store.library_entity_status(&spotify.id).unwrap().unwrap();
        assert_eq!(merged.id, native.id);
        assert!(merged.explicitly_saved);
        assert!(merged.aliases.contains(&spotify.id));
        store.remove_library_entity_save(&spotify.id).unwrap();
        assert!(store.library_entity_status(&native.id).unwrap().is_none());
        assert_eq!(
            store
                .list_library_entities("artist", None, "", 200, 0)
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn verified_album_links_merge_same_edition_without_doubling_tracks_or_merging_remasters() {
        let (_dir, store) = db();
        let mut t = track("spotify-song");
        t.provenance = json!({"artistCredits":[{"name":"Spotify guest","artist":{"provider":"spotify","id":"guest-sp"}}]});
        let spotify = store
            .save_library_album(
                entity("album", "Shared album", &t, Some("spotify"), Some("one")),
                vec![t.clone()],
                1,
            )
            .unwrap();
        let mut native_track = track("native-song");
        native_track.provenance = json!({"artistCredits":[{"name":"Catalog guest","artist":{"provider":"musicbrainz","id":"guest-mb"}}]});
        let native = store
            .save_library_album(
                entity(
                    "album",
                    "Shared album",
                    &t,
                    Some("musicbrainz"),
                    Some("edition-one"),
                ),
                vec![native_track],
                1,
            )
            .unwrap();
        store
            .save_library_album(
                entity(
                    "album",
                    "Shared album",
                    &t,
                    Some("spotify"),
                    Some("remaster"),
                ),
                vec![track("remastered-song")],
                1,
            )
            .unwrap();
        assert_eq!(
            store
                .list_library_entities("album", None, "", 200, 0)
                .unwrap()
                .total,
            3
        );
        store
            .link_library_entities("album", "spotify", "one", "musicbrainz", "edition-one")
            .unwrap();
        let albums = store
            .list_library_entities("album", None, "", 200, 0)
            .unwrap();
        assert_eq!(albums.total, 2);
        let artists = store
            .list_library_entities("artist", None, "", 200, 0)
            .unwrap();
        for name in ["Spotify guest", "Catalog guest"] {
            let artist = artists
                .items
                .iter()
                .find(|artist| artist.name == name)
                .unwrap();
            assert_eq!(artist.album_count, 1);
            assert_eq!(
                store
                    .list_library_entities("album", Some(&artist.id), "", 200, 0)
                    .unwrap()
                    .total,
                1
            );
        }
        let merged = store.library_entity_status(&spotify.id).unwrap().unwrap();
        assert_eq!(merged.id, native.id);
        assert_eq!(merged.track_count, 1);
        assert_eq!(
            store
                .list_library_entity_tracks(
                    &spotify.id,
                    "",
                    "sourceOrder",
                    false,
                    "all",
                    None,
                    None,
                    200,
                    0
                )
                .unwrap()
                .total,
            1
        );
        store.remove_library_entity_save(&native.id).unwrap();
        assert!(
            !store
                .library_entity_status(&spotify.id)
                .unwrap()
                .unwrap()
                .explicitly_saved
        );
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 3);
    }

    #[test]
    fn featured_credits_and_paging_do_not_merge_distinct_provider_artists() {
        let (_dir, store) = db();
        let mut one = track("Zulu");
        one.provenance = json!({"artistCredits":[{"name":"Same name","providerId":"one"},{"name":"Guest","providerId":"guest"}]});
        one.provider = "spotify".into();
        one.id = "spotify:Zulu".into();
        let mut two = track("Alpha");
        two.provider = "spotify".into();
        two.id = "spotify:Alpha".into();
        two.provenance = json!({"artistCredits":[{"name":"Same name","providerId":"two"},{"name":"Guest","providerId":"guest"}]});
        store.save_library_tracks(&[one, two]).unwrap();
        let artists = store
            .list_library_entities("artist", None, "", 200, 0)
            .unwrap();
        assert_eq!(artists.total, 3);
        let guest = artists.items.iter().find(|e| e.name == "Guest").unwrap();
        assert_eq!(guest.track_count, 2);
        assert_eq!(
            store
                .list_library_entities("album", Some(&guest.id), "", 200, 0)
                .unwrap()
                .total,
            1
        );
        let tracks = store
            .list_library_entity_tracks(&guest.id, "", "title", false, "all", None, None, 1, 1)
            .unwrap();
        assert_eq!(tracks.total, 2);
        assert_eq!(tracks.tracks[0].title, "Zulu");
    }
}
