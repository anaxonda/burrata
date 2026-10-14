use crate::browser_bridge::{
    snapshot_complete, snapshot_warnings, BrowserImportResult, BrowserSnapshot,
};
use crate::models::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Mutex};

pub struct Store {
    connection: Mutex<Connection>,
}
type Result<T> = std::result::Result<T, String>;
/// Reserved collection identity for the automatically populated Downloads view.
/// Collection names are user-editable and therefore are not used as identity.
pub const DOWNLOADS_COLLECTION_ID: &str = "00000000-0000-4000-8000-000000000001";
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn encode<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(err)
}
fn decode<T: serde::de::DeserializeOwned>(value: String) -> Result<T> {
    serde_json::from_str(&value).map_err(err)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(err)?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;",
        )
        .map_err(err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version > 6 {
            return Err("Database was created by a newer Burrata version".into());
        }
        if version == 0 {
            conn.execute_batch(include_str!("schema.sql"))
                .map_err(err)?;
        }
        if version < 2 {
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE browser_sources(source_url TEXT PRIMARY KEY, provider TEXT NOT NULL,
                    kind TEXT NOT NULL, collection_id TEXT NOT NULL REFERENCES collections(id),
                    latest_snapshot_id TEXT NOT NULL REFERENCES chart_snapshots(id));
                CREATE TABLE browser_imports(snapshot_id TEXT PRIMARY KEY REFERENCES chart_snapshots(id),
                    payload TEXT NOT NULL);
                PRAGMA user_version=2; COMMIT;").map_err(err)?;
        }
        if version < 3 {
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE provider_cache(namespace TEXT NOT NULL,key TEXT NOT NULL,payload TEXT NOT NULL,expires_at TEXT NOT NULL,PRIMARY KEY(namespace,key));
                CREATE TABLE artist_mappings(provider TEXT NOT NULL,provider_artist_id TEXT NOT NULL,artist_id TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(provider,provider_artist_id));
                CREATE TABLE asset_analyses(asset_id TEXT NOT NULL REFERENCES local_assets(id),file_hash TEXT NOT NULL,version TEXT NOT NULL,status TEXT NOT NULL,payload TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(asset_id,file_hash,version));
                PRAGMA user_version=3; COMMIT;").map_err(err)?;
        }
        if version < 4 {
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE playlist_ordered_collections(collection_id TEXT PRIMARY KEY REFERENCES collections(id) ON DELETE CASCADE);
                CREATE TABLE playlist_ordered_entries(collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,position INTEGER NOT NULL,listing_id TEXT NOT NULL REFERENCES catalog_listings(id),PRIMARY KEY(collection_id,position));
                CREATE INDEX playlist_ordered_listing ON playlist_ordered_entries(listing_id);
                PRAGMA user_version=4; COMMIT;").map_err(err)?;
        }
        if version < 5 {
            conn.execute_batch(
                "BEGIN IMMEDIATE; ALTER TABLE collections ADD COLUMN deleted_at TEXT; PRAGMA user_version=5; COMMIT;",
            )
            .map_err(err)?;
        }
        if version < 6 {
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE asset_autotags(asset_id TEXT NOT NULL REFERENCES local_assets(id),file_hash TEXT NOT NULL,version TEXT NOT NULL,status TEXT NOT NULL,payload TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(asset_id,file_hash,version));
                PRAGMA user_version=6; COMMIT;").map_err(err)?;
        }
        let store = Self {
            connection: Mutex::new(conn),
        };
        for mut job in store.list_jobs()? {
            if !matches!(
                job.state.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            ) {
                job.state = "interrupted".into();
                job.error = Some(if job.candidate.provider == "slskd" {
                    "Application stopped; the existing Soulseek transfer will be reconciled on startup.".into()
                } else {
                    "Application stopped before this job completed. Review and retry explicitly."
                        .into()
                });
                store.save_job(&job)?;
            }
        }
        Ok(store)
    }
    pub fn settings(&self) -> Result<Settings> {
        let conn = self.connection.lock().map_err(err)?;
        let mut s = conn
            .prepare("SELECT value FROM settings WHERE key='application'")
            .map_err(err)?;
        let mut rows = s.query([]).map_err(err)?;
        match rows.next().map_err(err)? {
            Some(row) => decode(row.get(0).map_err(err)?),
            None => Ok(Settings::default()),
        }
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.connection.lock().map_err(err)?.execute("INSERT INTO settings(key,value) VALUES('application',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [encode(settings)?]).map_err(err)?;
        Ok(())
    }
    pub fn get_json_preference(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.connection.lock().map_err(err)?;
        let raw: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(err)?;
        raw.map(decode).transpose()
    }
    pub fn set_json_preference(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        if key.trim().is_empty() || key.chars().count() > 100 {
            return Err("Preference key must contain 1–100 characters".into());
        }
        self.connection
            .lock()
            .map_err(err)?
            .execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, encode(value)?],
            )
            .map_err(err)?;
        Ok(())
    }
    /// Durable state owned by the playlist synchronizer. Keeping this in SQLite means
    /// source bindings and immutable snapshots survive a missing or moved playlist file.
    pub fn playlist_sync_state(&self) -> Result<Option<serde_json::Value>> {
        self.get_json_preference("playlistSync.v1")
    }
    pub fn save_playlist_sync_state(&self, value: &serde_json::Value) -> Result<()> {
        self.set_json_preference("playlistSync.v1", value)
    }
    pub fn get_provider_cache(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>> {
        let conn = self.connection.lock().map_err(err)?;
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT payload,expires_at FROM provider_cache WHERE namespace=?1 AND key=?2",
                params![namespace, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(err)?;
        let Some((payload, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at <= chrono::Utc::now().to_rfc3339() {
            conn.execute(
                "DELETE FROM provider_cache WHERE namespace=?1 AND key=?2",
                params![namespace, key],
            )
            .map_err(err)?;
            return Ok(None);
        }
        Ok(Some(decode(payload)?))
    }
    pub fn set_provider_cache(
        &self,
        namespace: &str,
        key: &str,
        payload: &serde_json::Value,
        ttl_seconds: u64,
    ) -> Result<()> {
        if namespace.trim().is_empty() || key.trim().is_empty() {
            return Err("Cache namespace and key are required".into());
        }
        let encoded = encode(payload)?;
        if encoded.len() > 1024 * 1024 {
            return Err("Cached provider data is limited to 1 MB".into());
        }
        let ttl = ttl_seconds.min(315_576_000) as i64;
        let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(ttl)).to_rfc3339();
        self.connection
            .lock()
            .map_err(err)?
            .execute(
                "INSERT INTO provider_cache(namespace,key,payload,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,key) DO UPDATE SET payload=excluded.payload,expires_at=excluded.expires_at",
                params![namespace, key, encoded, expires_at],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn get_artist_mapping(
        &self,
        provider: &str,
        provider_artist_id: &str,
    ) -> Result<Option<String>> {
        self.connection
            .lock()
            .map_err(err)?
            .query_row(
                "SELECT artist_id FROM artist_mappings WHERE provider=?1 AND provider_artist_id=?2",
                params![provider, provider_artist_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)
    }
    pub fn save_artist_mapping(
        &self,
        provider: &str,
        provider_artist_id: &str,
        artist_id: &str,
    ) -> Result<()> {
        if provider.trim().is_empty()
            || provider_artist_id.trim().is_empty()
            || artist_id.trim().is_empty()
        {
            return Err("Provider and artist identities are required".into());
        }
        self.connection
            .lock()
            .map_err(err)?
            .execute(
                "INSERT INTO artist_mappings(provider,provider_artist_id,artist_id,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(provider,provider_artist_id) DO UPDATE SET artist_id=excluded.artist_id,updated_at=excluded.updated_at",
                params![provider, provider_artist_id, artist_id, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn save_track(&self, track: &Track) -> Result<Track> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        upsert_track(&tx, track)?;
        let raw: String = tx
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&track.id],
                |r| r.get(0),
            )
            .map_err(err)?;
        let result = hydrate(&tx, decode(raw)?)?;
        tx.commit().map_err(err)?;
        Ok(result)
    }
    /// Remember the public source that actually played without treating it as a verified recording match.
    pub fn save_audio_match(&self, track: &Track, candidate: &Candidate) -> Result<Track> {
        let conn = self.connection.lock().map_err(err)?;
        ensure_track(&conn, track)?;
        let raw: String = conn
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&track.id],
                |row| row.get(0),
            )
            .map_err(err)?;
        let mut saved: Track = decode(raw)?;
        if !saved.provenance.is_object() {
            saved.provenance = serde_json::json!({});
        }
        saved.provenance["audioCandidate"] = serde_json::to_value(candidate).map_err(err)?;
        saved.provenance["candidateUrl"] = serde_json::json!(candidate.url);
        saved.provenance["audioMatchStatus"] =
            serde_json::json!("played source; recording match unverified");
        conn.execute(
            "UPDATE catalog_listings SET metadata=?1,updated_at=?2 WHERE id=?3",
            params![encode(&saved)?, chrono::Utc::now().to_rfc3339(), saved.id],
        )
        .map_err(err)?;
        hydrate(&conn, saved)
    }
    pub fn update_track(&self, track: &Track) -> Result<Track> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, track)?;
        let before: serde_json::Value = tx
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&track.id],
                |r| r.get::<_, String>(0),
            )
            .map_err(err)
            .and_then(decode)?;
        let prior: String = tx.query_row("SELECT r.manual_metadata FROM recordings r JOIN catalog_listings l ON r.id=l.recording_id WHERE l.id=?1",[&track.id],|r|r.get(0)).map_err(err)?;
        let mut manual: serde_json::Value = decode(prior)?;
        if !manual.is_object() {
            manual = serde_json::json!({});
        }
        let requested = serde_json::to_value(track).map_err(err)?;
        let prior_track = hydrate(&tx, serde_json::from_value(before.clone()).map_err(err)?)?;
        let previous = serde_json::to_value(prior_track).map_err(err)?;
        for (key, value) in requested
            .as_object()
            .ok_or("Track metadata must be an object")?
        {
            if previous.get(key) != Some(value) {
                manual[key] = value.clone();
            }
        }
        if let Some(obj) = manual.as_object_mut() {
            for key in [
                "id",
                "provider",
                "providerId",
                "sourceUrl",
                "rank",
                "favorite",
                "disposition",
                "localPath",
                "localAvailability",
                "assetId",
                "previewUrl",
                "provenance",
                "playbackRange",
            ] {
                obj.remove(key);
            }
        }
        tx.execute("UPDATE recordings SET manual_metadata=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)", params![encode(&manual)?, track.id]).map_err(err)?;
        let result = hydrate(&tx, track.clone())?;
        tx.commit().map_err(err)?;
        Ok(result)
    }
    pub fn recording_id(&self, listing_id: &str) -> Result<String> {
        self.connection
            .lock()
            .map_err(err)?
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [listing_id],
                |r| r.get(0),
            )
            .map_err(err)
    }
    pub fn get_track(&self, id: &str) -> Result<Option<Track>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn
            .prepare("SELECT metadata FROM catalog_listings WHERE id=?1")
            .map_err(err)?;
        let mut rows = stmt.query([id]).map_err(err)?;
        match rows.next().map_err(err)? {
            Some(r) => Ok(Some(hydrate(&conn, decode(r.get(0).map_err(err)?)?)?)),
            None => Ok(None),
        }
    }
    pub fn list_tracks(&self, view: &str, collection_id: Option<&str>) -> Result<Vec<Track>> {
        let conn = self.connection.lock().map_err(err)?;
        if view == "collection" {
            let active: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NULL)",
                    [collection_id.unwrap_or("")],
                    |row| row.get(0),
                )
                .map_err(err)?;
            if !active {
                return Err("Collection not found or deleted".into());
            }
        }
        let ordered = if view == "collection" {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM playlist_ordered_collections WHERE collection_id=?1)",
                [collection_id.unwrap_or("")],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false)
        } else {
            false
        };
        let sql = match view {
            "favorites" => {
                "SELECT l.metadata FROM catalog_listings l JOIN recordings r ON l.recording_id=r.id WHERE r.favorite=1 AND l.id=l.recording_id ORDER BY l.rowid DESC"
            }
            "shortlist" => {
                "SELECT l.metadata FROM catalog_listings l JOIN recordings r ON l.recording_id=r.id WHERE r.disposition='shortlist' AND l.id=l.recording_id ORDER BY l.rowid DESC"
            }
            "collection" if ordered => {
                "SELECT l.metadata,e.position FROM playlist_ordered_entries e JOIN catalog_listings l ON l.id=e.listing_id WHERE e.collection_id=?1 ORDER BY e.position"
            }
            "collection" => {
                "SELECT l.metadata FROM catalog_listings l JOIN collection_members m ON l.recording_id=m.recording_id WHERE m.collection_id=?1 AND l.id=l.recording_id ORDER BY m.position,l.rowid"
            }
            "library" => {
                "SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id ORDER BY l.rowid DESC"
            }
            _ => "SELECT metadata FROM catalog_listings ORDER BY rowid DESC",
        };
        if view == "collection" && ordered {
            let mut stmt = conn.prepare(sql).map_err(err)?;
            let values = stmt
                .query_map([collection_id.unwrap_or("")], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            return values
                .into_iter()
                .map(|(raw, position)| {
                    let mut track = hydrate(&conn, decode(raw)?)?;
                    if !track.provenance.is_object() {
                        track.provenance = serde_json::json!({});
                    }
                    // This view-only occurrence position leaves source capture metadata intact.
                    track.provenance["playlistPosition"] = serde_json::json!(position + 1);
                    Ok(track)
                })
                .collect();
        }
        let mut stmt = conn.prepare(sql).map_err(err)?;
        let values: Vec<String> = if view == "collection" {
            stmt.query_map([collection_id.unwrap_or("")], |r| r.get(0))
                .map_err(err)?
                .collect::<std::result::Result<_, _>>()
                .map_err(err)?
        } else {
            stmt.query_map([], |r| r.get(0))
                .map_err(err)?
                .collect::<std::result::Result<_, _>>()
                .map_err(err)?
        };
        values
            .into_iter()
            .map(|v| hydrate(&conn, decode(v)?))
            .collect()
    }
    /// Search the saved library in SQLite, returning only the requested stable page.
    /// Matching is across the same five metadata fields used by discovery search.
    pub fn search_library_tracks(
        &self,
        terms: &[String],
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Track>, usize)> {
        let conn = self.connection.lock().map_err(err)?;
        let mut filters = Vec::new();
        let mut values = Vec::<rusqlite::types::Value>::new();
        let haystack = "lower(COALESCE(json_extract(l.metadata,'$.artist'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.title'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.album'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.version'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.remixers'),''))";
        for term in terms {
            filters.push(format!("instr({haystack},?)>0"));
            values.push(term.to_lowercase().into());
        }
        let where_clause = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };
        // Match the existing discovery haystack fields explicitly. SQLite lower() is
        // ASCII-only, so Unicode terms use Rust matching over the unhydrated metadata
        // rows while retaining the same row order and hydration savings.
        let has_unicode = terms.iter().any(|term| !term.is_ascii());
        let (count, raw_rows) = if has_unicode {
            let mut stmt = conn.prepare("SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id ORDER BY l.rowid DESC").map_err(err)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            let mut matches = Vec::new();
            for raw in rows {
                let track: Track = decode(raw.clone())?;
                let haystack = format!(
                    "{} {} {} {} {}",
                    track.artist,
                    track.title,
                    track.album.as_deref().unwrap_or(""),
                    track.version.as_deref().unwrap_or(""),
                    track
                        .remixers
                        .as_ref()
                        .map(|names| names.join(" "))
                        .unwrap_or_default()
                )
                .to_lowercase();
                if terms
                    .iter()
                    .all(|term| haystack.contains(&term.to_lowercase()))
                {
                    matches.push(raw);
                }
            }
            let count = matches.len();
            (
                count,
                matches
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .collect::<Vec<_>>(),
            )
        } else {
            // Keep each query word required while allowing it to match any searchable
            // metadata field, as the prior Rust haystack did.
            let where_sql = where_clause;
            let count_sql = format!(
                "SELECT COUNT(*) FROM catalog_listings l WHERE l.id=l.recording_id{where_sql}"
            );
            let count: i64 = conn
                .query_row(
                    &count_sql,
                    rusqlite::params_from_iter(values.iter()),
                    |row| row.get(0),
                )
                .map_err(err)?;
            let sql = format!("SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id{where_sql} ORDER BY l.rowid DESC LIMIT ? OFFSET ?");
            let mut page_values = values;
            page_values.push((limit as i64).into());
            page_values.push((offset as i64).into());
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(page_values.iter()), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            (count as usize, rows)
        };
        let tracks = raw_rows
            .into_iter()
            .map(|raw| hydrate(&conn, decode(raw)?))
            .collect::<Result<Vec<_>>>()?;
        Ok((tracks, count))
    }
    pub fn set_favorite(&self, track: &Track, value: bool) -> Result<()> {
        {
            let conn = self.connection.lock().map_err(err)?;
            ensure_track(&conn, track)?;
        }
        self.connection.lock().map_err(err)?.execute("UPDATE recordings SET favorite=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)", params![value, track.id]).map_err(err)?;
        Ok(())
    }
    pub fn set_disposition(&self, track: &Track, value: Option<&str>) -> Result<()> {
        if !matches!(value, None | Some("shortlist") | Some("reject")) {
            return Err("Disposition must be shortlist, reject, or empty".into());
        }
        {
            let conn = self.connection.lock().map_err(err)?;
            ensure_track(&conn, track)?;
        }
        self.connection.lock().map_err(err)?.execute("UPDATE recordings SET disposition=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)", params![value, track.id]).map_err(err)?;
        Ok(())
    }
    pub fn create_collection(&self, name: &str) -> Result<Collection> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err("Collection name must contain 1–120 characters".into());
        }
        let c = Collection {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            track_count: 0,
        };
        self.connection
            .lock()
            .map_err(err)?
            .execute(
                "INSERT INTO collections(id,name) VALUES(?1,?2)",
                params![c.id, c.name],
            )
            .map_err(err)?;
        Ok(c)
    }
    pub fn rename_collection(&self, collection_id: &str, name: &str) -> Result<Collection> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err("Collection name must contain 1–120 characters".into());
        }
        let conn = self.connection.lock().map_err(err)?;
        let changed = conn
            .execute(
                "UPDATE collections SET name=?1 WHERE id=?2 AND deleted_at IS NULL",
                params![name, collection_id],
            )
            .map_err(err)?;
        if changed == 0 {
            return Err("Collection not found".into());
        }
        let track_count = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_members WHERE collection_id=?1",
                [collection_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        Ok(Collection {
            id: collection_id.into(),
            name: name.into(),
            track_count,
        })
    }
    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn.prepare("SELECT c.id,c.name,CASE WHEN EXISTS(SELECT 1 FROM playlist_ordered_collections o WHERE o.collection_id=c.id) THEN (SELECT COUNT(*) FROM playlist_ordered_entries e WHERE e.collection_id=c.id) ELSE COUNT(m.recording_id) END FROM collections c LEFT JOIN collection_members m ON m.collection_id=c.id WHERE c.deleted_at IS NULL GROUP BY c.id ORDER BY c.name").map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Collection {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    track_count: r.get(2)?,
                })
            })
            .map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)
    }
    pub fn list_deleted_collections(&self) -> Result<Vec<Collection>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn.prepare("SELECT c.id,c.name,CASE WHEN EXISTS(SELECT 1 FROM playlist_ordered_collections o WHERE o.collection_id=c.id) THEN (SELECT COUNT(*) FROM playlist_ordered_entries e WHERE e.collection_id=c.id) ELSE (SELECT COUNT(*) FROM collection_members m WHERE m.collection_id=c.id) END FROM collections c WHERE c.deleted_at IS NOT NULL ORDER BY c.name").map_err(err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    track_count: row.get(2)?,
                })
            })
            .map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)
    }
    /// Soft-delete a collection without removing its entries, source bindings, or audio assets.
    pub fn delete_collection(&self, collection_id: &str) -> Result<Collection> {
        let conn = self.connection.lock().map_err(err)?;
        let changed = conn
            .execute(
                "UPDATE collections SET deleted_at=COALESCE(deleted_at,?1) WHERE id=?2",
                params![chrono::Utc::now().to_rfc3339(), collection_id],
            )
            .map_err(err)?;
        if changed == 0 {
            return Err("Collection not found".into());
        }
        let (name, track_count): (String, i64) = conn
            .query_row(
                "SELECT name,CASE WHEN EXISTS(SELECT 1 FROM playlist_ordered_collections o WHERE o.collection_id=c.id) THEN (SELECT COUNT(*) FROM playlist_ordered_entries e WHERE e.collection_id=c.id) ELSE (SELECT COUNT(*) FROM collection_members m WHERE m.collection_id=c.id) END FROM collections c WHERE c.id=?1",
                [collection_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        Ok(Collection {
            id: collection_id.to_owned(),
            name,
            track_count: track_count.max(0).min(u32::MAX as i64) as u32,
        })
    }
    /// Restore a soft-deleted collection under the same stable ID and ordered entries.
    pub fn restore_collection(&self, collection_id: &str) -> Result<Collection> {
        let conn = self.connection.lock().map_err(err)?;
        let changed = conn
            .execute(
                "UPDATE collections SET deleted_at=NULL WHERE id=?1 AND deleted_at IS NOT NULL",
                [collection_id],
            )
            .map_err(err)?;
        if changed == 0 {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1)",
                    [collection_id],
                    |row| row.get(0),
                )
                .map_err(err)?;
            if !exists {
                return Err("Collection not found".into());
            }
        }
        self.active_collection(&conn, collection_id)
    }
    fn active_collection(&self, conn: &Connection, collection_id: &str) -> Result<Collection> {
        let (name, track_count): (String, i64) = conn
            .query_row(
                "SELECT c.name,CASE WHEN EXISTS(SELECT 1 FROM playlist_ordered_collections o WHERE o.collection_id=c.id) THEN (SELECT COUNT(*) FROM playlist_ordered_entries e WHERE e.collection_id=c.id) ELSE (SELECT COUNT(*) FROM collection_members m WHERE m.collection_id=c.id) END FROM collections c WHERE c.id=?1 AND c.deleted_at IS NULL",
                [collection_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        Ok(Collection {
            id: collection_id.to_owned(),
            name,
            track_count: track_count.max(0).min(u32::MAX as i64) as u32,
        })
    }
    /// Ensure the stable Downloads system collection exists and is active.
    /// Safe to call during startup, even before the first asset is downloaded.
    pub fn ensure_downloads_collection(&self) -> Result<Collection> {
        let conn = self.connection.lock().map_err(err)?;
        conn.execute(
            "INSERT INTO collections(id,name) VALUES(?1,'Downloads') ON CONFLICT(id) DO UPDATE SET name='Downloads',deleted_at=NULL",
            [DOWNLOADS_COLLECTION_ID],
        )
        .map_err(err)?;
        self.active_collection(&conn, DOWNLOADS_COLLECTION_ID)
    }
    pub fn add_to_collection(&self, collection_id: &str, tracks: &[Track]) -> Result<()> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let active: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NULL)",
                [collection_id],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !active {
            return Err("Collection not found or deleted".into());
        }
        for track in tracks {
            ensure_track(&tx, track)?;
            let inserted = tx.execute("INSERT OR IGNORE INTO collection_members(collection_id,recording_id,position) SELECT ?1,recording_id,(SELECT COALESCE(MAX(position),0)+1 FROM collection_members WHERE collection_id=?1) FROM catalog_listings WHERE id=?2", params![collection_id,track.id]).map_err(err)?;
            if inserted > 0 {
                let ordered: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM playlist_ordered_collections WHERE collection_id=?1)", [collection_id], |r| r.get(0)).map_err(err)?;
                if ordered {
                    tx.execute("INSERT INTO playlist_ordered_entries(collection_id,position,listing_id) VALUES(?1,(SELECT COALESCE(MAX(position),-1)+1 FROM playlist_ordered_entries WHERE collection_id=?1),?2)", params![collection_id, track.id]).map_err(err)?;
                }
            }
        }
        tx.commit().map_err(err)
    }
    /// Add a managed audio asset to the stable system Downloads collection.
    /// Repeated calls (including publication retries) leave membership unchanged.
    pub fn add_asset_to_downloads(&self, asset: &LocalAsset) -> Result<Collection> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, &asset.track)?;
        tx.execute(
            "INSERT INTO collections(id,name) VALUES(?1,'Downloads') ON CONFLICT(id) DO UPDATE SET name='Downloads',deleted_at=NULL",
            [DOWNLOADS_COLLECTION_ID],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT OR IGNORE INTO collection_members(collection_id,recording_id,position) SELECT ?1,recording_id,(SELECT COALESCE(MAX(position),0)+1 FROM collection_members WHERE collection_id=?1) FROM catalog_listings WHERE id=?2",
            params![DOWNLOADS_COLLECTION_ID, asset.track.id],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        self.active_collection(&conn, DOWNLOADS_COLLECTION_ID)
    }
    /// Backfill only existing on-disk assets carrying the managed-publication
    /// marker. Imported local files and guessed paths are deliberately excluded.
    pub fn backfill_managed_downloads(&self) -> Result<usize> {
        let assets = self.list_assets()?;
        let mut added = 0;
        for asset in assets {
            if asset.track.provenance["publicationJobId"]
                .as_str()
                .is_some_and(|job_id| !job_id.is_empty())
                && Path::new(&asset.path).is_file()
            {
                self.add_asset_to_downloads(&asset)?;
                added += 1;
            }
        }
        Ok(added)
    }
    /// Replace ordered playlist entries, preserving duplicates in the sidecar while keeping
    /// legacy collection membership populated for older queries.
    pub fn replace_playlist_entries(&self, collection_id: &str, tracks: &[Track]) -> Result<()> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NULL)",
                [collection_id],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Err("Collection not found".into());
        }
        for track in tracks {
            ensure_track(&tx, track)?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO playlist_ordered_collections(collection_id) VALUES(?1)",
            [collection_id],
        )
        .map_err(err)?;
        tx.execute(
            "DELETE FROM playlist_ordered_entries WHERE collection_id=?1",
            [collection_id],
        )
        .map_err(err)?;
        tx.execute(
            "DELETE FROM collection_members WHERE collection_id=?1",
            [collection_id],
        )
        .map_err(err)?;
        for (position, track) in tracks.iter().enumerate() {
            tx.execute("INSERT INTO playlist_ordered_entries(collection_id,position,listing_id) VALUES(?1,?2,?3)", params![collection_id,position as i64,track.id]).map_err(err)?;
            tx.execute("INSERT OR IGNORE INTO collection_members(collection_id,recording_id,position) SELECT ?1,recording_id,?2 FROM catalog_listings WHERE id=?3", params![collection_id,position as i64,track.id]).map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    /// Remove selected recordings from one collection while preserving their catalog entries
    /// and memberships in every other collection.
    pub fn remove_from_collection(&self, collection_id: &str, track_ids: &[String]) -> Result<()> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NULL)",
                [collection_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Err("Collection not found".into());
        }
        for track_id in track_ids {
            let recording_id: String = tx
                .query_row(
                    "SELECT recording_id FROM catalog_listings WHERE id=?1",
                    [track_id],
                    |row| row.get(0),
                )
                .map_err(err)?;
            tx.execute(
                "DELETE FROM collection_members WHERE collection_id=?1 AND recording_id=?2",
                params![collection_id, recording_id],
            )
            .map_err(err)?;
            tx.execute("DELETE FROM playlist_ordered_entries WHERE collection_id=?1 AND listing_id IN (SELECT id FROM catalog_listings WHERE recording_id=?2)", params![collection_id, recording_id]).map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    /// Explicit user verification is required; names alone never invoke this operation.
    pub fn merge_recordings(&self, primary_id: &str, other_ids: &[String]) -> Result<()> {
        if other_ids.is_empty() {
            return Err("Choose at least two recordings to merge".into());
        }
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let primary: String = tx
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [primary_id],
                |r| r.get(0),
            )
            .map_err(err)?;
        for listing in other_ids {
            let other: String = tx
                .query_row(
                    "SELECT recording_id FROM catalog_listings WHERE id=?1",
                    [listing],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if primary == other {
                continue;
            }
            tx.execute("UPDATE recordings SET favorite=MAX(favorite,(SELECT favorite FROM recordings WHERE id=?2)),disposition=COALESCE(disposition,(SELECT disposition FROM recordings WHERE id=?2)) WHERE id=?1",params![primary,other]).map_err(err)?;
            tx.execute("INSERT OR IGNORE INTO collection_members(collection_id,recording_id,position) SELECT collection_id,?1,position FROM collection_members WHERE recording_id=?2",params![primary,other]).map_err(err)?;
            tx.execute(
                "DELETE FROM collection_members WHERE recording_id=?1",
                [&other],
            )
            .map_err(err)?;
            tx.execute(
                "UPDATE catalog_listings SET recording_id=?1 WHERE recording_id=?2",
                params![primary, other],
            )
            .map_err(err)?;
            tx.execute(
                "UPDATE local_assets SET recording_id=?1 WHERE recording_id=?2",
                params![primary, other],
            )
            .map_err(err)?;
            // Preserve original work/recording rows as identity history; incoming refreshes do not undo the merge.
        }
        tx.commit().map_err(err)
    }
    pub fn save_chart(&self, chart: &Chart) -> Result<Chart> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let mut saved = chart.clone();
        saved.id = uuid::Uuid::new_v4().to_string();
        for track in &saved.tracks {
            upsert_track(&tx, track)?;
        }
        tx.execute("INSERT INTO chart_snapshots(id,provider,source_url,fetched_at,payload) VALUES(?1,?2,?3,?4,?5)", params![saved.id,saved.provider,saved.source_url,saved.fetched_at,encode(&saved)?]).map_err(err)?;
        for track in &saved.tracks {
            tx.execute(
                "INSERT INTO chart_entries(snapshot_id,listing_id,rank) VALUES(?1,?2,?3)",
                params![saved.id, track.id, track.rank],
            )
            .map_err(err)?;
        }
        saved.tracks = saved
            .tracks
            .into_iter()
            .map(|t| hydrate(&tx, t))
            .collect::<Result<_>>()?;
        tx.commit().map_err(err)?;
        Ok(saved)
    }
    /// Browser snapshots and collection membership are committed together. Incomplete
    /// captures only add members; a verified complete capture synchronizes this collection.
    pub fn import_browser_chart(
        &self,
        chart: &Chart,
        snapshot: &BrowserSnapshot,
    ) -> Result<BrowserImportResult> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let mut saved = chart.clone();
        saved.id = uuid::Uuid::new_v4().to_string();
        let collection_id: Option<String> = tx
            .query_row(
                "SELECT collection_id FROM browser_sources WHERE source_url=?1",
                [&saved.source_url],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let collection_id = match collection_id {
            Some(id) => id,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO collections(id,name) VALUES(?1,?2)",
                    params![id, saved.title],
                )
                .map_err(err)?;
                id
            }
        };
        let complete = snapshot_complete(snapshot);
        if complete {
            tx.execute(
                "DELETE FROM collection_members WHERE collection_id=?1",
                [&collection_id],
            )
            .map_err(err)?;
        }
        let mut position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position),0) FROM collection_members WHERE collection_id=?1",
                [&collection_id],
                |r| r.get(0),
            )
            .map_err(err)?;
        // The adapter retains capture order and deduplicates listings. Distinct listings
        // may already have been manually merged, so membership also deduplicates recordings.
        for track in &saved.tracks {
            upsert_track(&tx, track)?;
            position += 1;
            tx.execute(
                "INSERT OR IGNORE INTO collection_members(collection_id,recording_id,position)
                SELECT ?1,recording_id,?3 FROM catalog_listings WHERE id=?2",
                params![collection_id, track.id, position],
            )
            .map_err(err)?;
        }
        tx.execute("INSERT INTO chart_snapshots(id,provider,source_url,fetched_at,payload) VALUES(?1,?2,?3,?4,?5)",
            params![saved.id,saved.provider,saved.source_url,saved.fetched_at,encode(&saved)?]).map_err(err)?;
        for track in &saved.tracks {
            tx.execute(
                "INSERT INTO chart_entries(snapshot_id,listing_id,rank) VALUES(?1,?2,?3)",
                params![saved.id, track.id, track.rank],
            )
            .map_err(err)?;
        }
        let result = BrowserImportResult {
            id: saved.id.clone(),
            provider: saved.provider.clone(),
            kind: snapshot.kind.clone(),
            title: saved.title.clone(),
            source_url: saved.source_url.clone(),
            fetched_at: saved.fetched_at.clone(),
            collection_id: collection_id.clone(),
            captured_count: snapshot.entries.len(),
            imported_count: saved.tracks.len(),
            expected_total: snapshot.expected_total,
            complete,
            warnings: snapshot_warnings(snapshot, &saved),
        };
        tx.execute(
            "INSERT INTO browser_imports(snapshot_id,payload) VALUES(?1,?2)",
            params![saved.id, encode(&result)?],
        )
        .map_err(err)?;
        tx.execute("INSERT INTO browser_sources(source_url,provider,kind,collection_id,latest_snapshot_id)
            VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source_url) DO UPDATE SET
            provider=excluded.provider,kind=excluded.kind,latest_snapshot_id=excluded.latest_snapshot_id",
            params![saved.source_url,saved.provider,snapshot.kind,collection_id,saved.id]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(result)
    }
    pub fn list_browser_imports(&self) -> Result<Vec<BrowserImportResult>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn
            .prepare("SELECT b.payload FROM browser_imports b JOIN chart_snapshots s ON s.id=b.snapshot_id JOIN browser_sources src ON src.source_url=s.source_url AND src.provider=s.provider JOIN collections c ON c.id=src.collection_id WHERE c.deleted_at IS NULL ORDER BY b.rowid DESC LIMIT 50")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| decode(r.map_err(err)?)).collect()
    }
    pub fn load_browser_import(&self, id: &str) -> Result<Chart> {
        let conn = self.connection.lock().map_err(err)?;
        let raw: String = conn
            .query_row(
                "SELECT s.payload FROM chart_snapshots s
            JOIN browser_imports b ON b.snapshot_id=s.id WHERE s.id=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(err)?;
        let mut chart: Chart = decode(raw)?;
        chart.tracks = chart
            .tracks
            .into_iter()
            .map(|t| hydrate(&conn, t))
            .collect::<Result<_>>()?;
        chart.tracks = crate::browser_bridge::expand_playlist_occurrences(chart.tracks);
        Ok(chart)
    }
    pub fn list_snapshots(&self, provider: &str, url: &str) -> Result<Vec<Chart>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn.prepare("SELECT payload FROM chart_snapshots WHERE provider=?1 AND source_url=?2 ORDER BY fetched_at DESC,rowid DESC").map_err(err)?;
        let rows = stmt
            .query_map(params![provider, url], |r| r.get::<_, String>(0))
            .map_err(err)?;
        rows.map(|r| {
            let mut chart: Chart = decode(r.map_err(err)?)?;
            chart.tracks = chart
                .tracks
                .into_iter()
                .map(|track| hydrate(&conn, track))
                .collect::<Result<_>>()?;
            Ok(chart)
        })
        .collect()
    }
    /// Full stored snapshots for a browser-owned playlist collection, newest first.
    /// Each payload contains that capture's actual track list rather than the cumulative membership union.
    pub fn browser_snapshots_for_collection(&self, collection_id: &str) -> Result<Vec<Chart>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn.prepare("SELECT s.payload FROM browser_sources b JOIN chart_snapshots s ON s.source_url=b.source_url AND s.provider=b.provider WHERE b.collection_id=?1 ORDER BY s.fetched_at DESC,s.rowid DESC").map_err(err)?;
        let rows = stmt
            .query_map([collection_id], |row| row.get::<_, String>(0))
            .map_err(err)?;
        rows.map(|row| decode(row.map_err(err)?)).collect()
    }
    pub fn is_browser_source_collection(&self, collection_id: &str) -> Result<bool> {
        self.connection
            .lock()
            .map_err(err)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM browser_sources WHERE collection_id=?1)",
                [collection_id],
                |row| row.get(0),
            )
            .map_err(err)
    }
    pub fn chart_memberships(&self, listing_id: &str) -> Result<Vec<ChartMembership>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut statement=conn.prepare("SELECT s.provider,json_extract(s.payload,'$.title'),s.source_url,e.rank,MAX(s.fetched_at) FROM chart_entries e JOIN chart_snapshots s ON s.id=e.snapshot_id JOIN catalog_listings l ON l.id=e.listing_id WHERE l.recording_id=(SELECT recording_id FROM catalog_listings WHERE id=?1) GROUP BY s.provider,s.source_url ORDER BY MAX(s.fetched_at) DESC LIMIT 50").map_err(err)?;
        let rows = statement
            .query_map([listing_id], |r| {
                Ok(ChartMembership {
                    provider: r.get(0)?,
                    title: r.get(1)?,
                    source_url: r.get(2)?,
                    rank: r.get(3)?,
                    fetched_at: r.get(4)?,
                })
            })
            .map_err(err)?;
        rows.collect::<std::result::Result<_, _>>().map_err(err)
    }
    pub fn latest_snapshot(&self, provider: &str, url: &str) -> Result<Option<Chart>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut statement=conn.prepare("SELECT payload FROM chart_snapshots WHERE provider=?1 AND source_url=?2 ORDER BY fetched_at DESC,rowid DESC LIMIT 1").map_err(err)?;
        let mut rows = statement.query(params![provider, url]).map_err(err)?;
        match rows.next().map_err(err)? {
            Some(r) => {
                let mut chart: Chart = decode(r.get(0).map_err(err)?)?;
                chart.tracks = chart
                    .tracks
                    .into_iter()
                    .map(|track| hydrate(&conn, track))
                    .collect::<Result<_>>()?;
                Ok(Some(chart))
            }
            None => Ok(None),
        }
    }
    pub fn load_snapshot(&self, id: &str) -> Result<Chart> {
        let conn = self.connection.lock().map_err(err)?;
        let raw: String = conn
            .query_row(
                "SELECT payload FROM chart_snapshots WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(err)?;
        let mut c: Chart = decode(raw)?;
        c.tracks = c
            .tracks
            .into_iter()
            .map(|t| hydrate(&conn, t))
            .collect::<Result<_>>()?;
        Ok(c)
    }
    pub fn save_job(&self, job: &DownloadJob) -> Result<()> {
        {
            let conn = self.connection.lock().map_err(err)?;
            ensure_track(&conn, &job.track)?;
        }
        self.connection.lock().map_err(err)?.execute("INSERT INTO download_jobs(id,state,payload) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload", params![job.id,job.state,encode(job)?]).map_err(err)?;
        Ok(())
    }
    pub fn get_job(&self, id: &str) -> Result<DownloadJob> {
        let raw: String = self
            .connection
            .lock()
            .map_err(err)?
            .query_row("SELECT payload FROM download_jobs WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(err)?;
        decode(raw)
    }
    pub fn list_jobs(&self) -> Result<Vec<DownloadJob>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn
            .prepare("SELECT payload FROM download_jobs ORDER BY rowid DESC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| decode(r.map_err(err)?)).collect()
    }
    pub fn save_asset(&self, asset: &LocalAsset) -> Result<()> {
        {
            let conn = self.connection.lock().map_err(err)?;
            ensure_track(&conn, &asset.track)?;
        }
        self.connection.lock().map_err(err)?.execute("INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?1,(SELECT recording_id FROM catalog_listings WHERE id=?2),?3,?4,?5) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,path=excluded.path,hash=excluded.hash", params![asset.id,asset.track.id,asset.path,asset.hash,encode(asset)?]).map_err(err)?;
        Ok(())
    }
    pub fn save_autotag_record(
        &self,
        asset_id: &str,
        input_hash: &str,
        version: &str,
        status: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let encoded = encode(payload)?;
        if encoded.len() > 256 * 1024
            || asset_id.is_empty()
            || input_hash.is_empty()
            || version.is_empty()
            || status.is_empty()
        {
            return Err("Invalid autotag record".into());
        }
        let conn = self.connection.lock().map_err(err)?;
        let current: String = conn
            .query_row(
                "SELECT hash FROM local_assets WHERE id=?1",
                [asset_id],
                |r| r.get(0),
            )
            .map_err(err)?;
        if current != input_hash {
            return Err("Local asset changed while tagging was running".into());
        }
        conn.execute("INSERT INTO asset_autotags(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(asset_id,file_hash,version) DO UPDATE SET status=excluded.status,payload=excluded.payload,updated_at=excluded.updated_at", params![asset_id,input_hash,version,status,encoded,chrono::Utc::now().to_rfc3339()]).map_err(err)?;
        Ok(())
    }
    pub fn list_autotag_records(&self) -> Result<Vec<serde_json::Value>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn.prepare("SELECT asset_id,file_hash,version,status,payload FROM asset_autotags ORDER BY updated_at DESC").map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(err)?;
        rows.map(|row| { let (id,hash,version,status,payload)=row.map_err(err)?; Ok(serde_json::json!({"assetId":id,"inputHash":hash,"version":version,"status":status,"payload":decode::<serde_json::Value>(payload)?})) }).collect()
    }
    pub fn save_autotagged_asset(
        &self,
        asset: &LocalAsset,
        prior_hash: &str,
        version: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let (current_hash, path, recording): (String, String, String) = tx
            .query_row(
                "SELECT hash,path,recording_id FROM local_assets WHERE id=?1",
                [&asset.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(err)?;
        if current_hash != prior_hash || path != asset.path || recording != asset.recording_id {
            return Err("Autotag cannot replace a changed asset or its identity".into());
        }
        tx.execute(
            "UPDATE local_assets SET hash=?1,payload=?2 WHERE id=?3",
            params![asset.hash, encode(asset)?, asset.id],
        )
        .map_err(err)?;
        tx.execute("UPDATE asset_autotags SET status='superseded' WHERE asset_id=?1 AND file_hash=?2 AND version=?3",params![asset.id,prior_hash,version]).map_err(err)?;
        tx.execute("INSERT INTO asset_autotags(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,?3,'completed',?4,?5) ON CONFLICT(asset_id,file_hash,version) DO UPDATE SET status='completed',payload=excluded.payload,updated_at=excluded.updated_at",params![asset.id,asset.hash,version,encode(payload)?,chrono::Utc::now().to_rfc3339()]).map_err(err)?;
        tx.commit().map_err(err)
    }
    /// Atomically publish metadata retagged into a local file and record the
    /// analysis under the resulting hash. A stale worker cannot overwrite an
    /// asset that changed after its input hash was captured.
    pub fn save_analyzed_asset(
        &self,
        asset: &LocalAsset,
        prior_hash: &str,
        version: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        if version.trim().is_empty() || asset.hash.trim().is_empty() {
            return Err("Analysis version and resulting asset hash are required".into());
        }
        let encoded_asset = encode(asset)?;
        let encoded_payload = encode(payload)?;
        if encoded_payload.len() > 64 * 1024 {
            return Err("Analysis record is limited to 64 KB".into());
        }
        if payload
            .get("bpm")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|bpm| !bpm.is_finite() || bpm <= 0.0 || bpm > 1000.0)
        {
            return Err("Analysis BPM must be positive and finite".into());
        }
        let mut conn = self.connection.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, &asset.track)?;
        let current: Option<(String, String)> = tx
            .query_row(
                "SELECT hash,recording_id FROM local_assets WHERE id=?1",
                [&asset.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(err)?;
        let Some((current_hash, recording_id)) = current else {
            return Err("Local asset not found".into());
        };
        if current_hash != prior_hash {
            return Err("Local asset changed while analysis was running".into());
        }
        let track_recording: String = tx
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [&asset.track.id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if track_recording != recording_id {
            return Err("Analyzed asset cannot change its recording identity".into());
        }
        tx.execute(
            "UPDATE asset_analyses SET status='superseded',updated_at=?1 WHERE asset_id=?2 AND file_hash=?3 AND version=?4",
            params![chrono::Utc::now().to_rfc3339(), asset.id, prior_hash, version],
        )
        .map_err(err)?;
        tx.execute(
            "UPDATE local_assets SET path=?1,hash=?2,payload=?3 WHERE id=?4",
            params![asset.path, asset.hash, encoded_asset, asset.id],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,?3,'completed',?4,?5) ON CONFLICT(asset_id,file_hash,version) DO UPDATE SET status='completed',payload=excluded.payload,updated_at=excluded.updated_at",
            params![asset.id, asset.hash, version, encoded_payload, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    }
    pub fn save_analysis_record(
        &self,
        asset_id: &str,
        input_hash: &str,
        version: &str,
        status: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        if asset_id.trim().is_empty()
            || input_hash.trim().is_empty()
            || version.trim().is_empty()
            || status.trim().is_empty()
        {
            return Err("Analysis asset, hash, version, and status are required".into());
        }
        if payload
            .get("bpm")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|bpm| !bpm.is_finite() || bpm <= 0.0 || bpm > 1000.0)
        {
            return Err("Analysis BPM must be positive and finite".into());
        }
        let encoded = encode(payload)?;
        if encoded.len() > 64 * 1024 {
            return Err("Analysis record is limited to 64 KB".into());
        }
        let conn = self.connection.lock().map_err(err)?;
        let stored_hash: String = conn
            .query_row(
                "SELECT hash FROM local_assets WHERE id=?1",
                [asset_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if stored_hash != input_hash {
            return Err("Analysis hash does not match the saved local asset".into());
        }
        conn.execute(
            "INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(asset_id,file_hash,version) DO UPDATE SET status=excluded.status,payload=excluded.payload,updated_at=excluded.updated_at",
            params![asset_id, input_hash, version, status, encoded, chrono::Utc::now().to_rfc3339()],
        ).map_err(err)?;
        Ok(())
    }
    pub fn list_analysis_records(&self) -> Result<Vec<serde_json::Value>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn.prepare("SELECT asset_id,file_hash,version,status,payload FROM asset_analyses ORDER BY updated_at DESC").map_err(err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(err)?;
        rows.map(|row| {
            let (asset_id,input_hash,version,status,payload) = row.map_err(err)?;
            Ok(serde_json::json!({"assetId":asset_id,"inputHash":input_hash,"version":version,"status":status,"payload":decode::<serde_json::Value>(payload)?}))
        }).collect()
    }
    pub fn find_asset_by_hash(&self, hash: &str) -> Result<Option<LocalAsset>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn
            .prepare("SELECT payload FROM local_assets WHERE hash=?1 LIMIT 1")
            .map_err(err)?;
        let mut rows = stmt.query([hash]).map_err(err)?;
        match rows.next().map_err(err)? {
            Some(r) => {
                let a: LocalAsset = decode(r.get(0).map_err(err)?)?;
                Ok(Some(hydrate_asset(&conn, a)?))
            }
            None => Ok(None),
        }
    }
    pub fn get_asset_by_path(&self, path: &str) -> Result<Option<LocalAsset>> {
        let conn = self.connection.lock().map_err(err)?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT payload FROM local_assets WHERE path=?1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        raw.map(|payload| hydrate_asset(&conn, decode(payload)?))
            .transpose()
    }
    pub fn list_assets(&self) -> Result<Vec<LocalAsset>> {
        let conn = self.connection.lock().map_err(err)?;
        let mut stmt = conn
            .prepare("SELECT payload FROM local_assets ORDER BY rowid DESC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| hydrate_asset(&conn, decode(r.map_err(err)?)?))
            .collect()
    }
}

fn ensure_track(conn: &Connection, track: &Track) -> Result<()> {
    if track.id != format!("{}:{}", track.provider, track.provider_id) {
        return Err("Invalid track identity".into());
    }
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_listings WHERE id=?1)",
            [&track.id],
            |r| r.get(0),
        )
        .map_err(err)?;
    if !exists {
        upsert_track(conn, track)?;
    }
    Ok(())
}
fn upsert_track(conn: &Connection, track: &Track) -> Result<()> {
    if track.id.is_empty() || track.provider_id.is_empty() || track.provider.is_empty() {
        return Err("Track identity is missing".into());
    }
    if track.id != format!("{}:{}", track.provider, track.provider_id) {
        return Err("Track ID must be the canonical provider:provider_id identity".into());
    }
    conn.execute(
        "INSERT OR IGNORE INTO works(id,title,artist) VALUES(?1,?2,?3)",
        params![format!("work:{}", track.id), track.title, track.artist],
    )
    .map_err(err)?;
    conn.execute(
        "INSERT OR IGNORE INTO recordings(id,work_id,version) VALUES(?1,?2,?3)",
        params![track.id, format!("work:{}", track.id), track.version],
    )
    .map_err(err)?;
    let mut metadata = serde_json::to_value(track).map_err(err)?;
    let old: Option<String> = conn
        .query_row(
            "SELECT metadata FROM catalog_listings WHERE provider=?1 AND provider_id=?2",
            params![track.provider, track.provider_id],
            |r| r.get(0),
        )
        .ok();
    if let Some(old) = old {
        let old: serde_json::Value = decode(old)?;
        if let (Some(new), Some(old)) = (metadata.as_object_mut(), old.as_object()) {
            for (k, v) in old {
                if new.get(k).is_some_and(|v| v.is_null()) && !v.is_null() {
                    new.insert(k.clone(), v.clone());
                }
            }
        }
    }
    conn.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?1,?2,?3,?1,?4,?5,?6) ON CONFLICT(provider,provider_id) DO UPDATE SET metadata=excluded.metadata,source_url=excluded.source_url,updated_at=excluded.updated_at", params![track.id,track.provider,track.provider_id,track.source_url,encode(&metadata)?,chrono::Utc::now().to_rfc3339()]).map_err(err)?;
    Ok(())
}

fn hydrate(conn: &Connection, mut track: Track) -> Result<Track> {
    let values: (bool,Option<String>,String) = conn.query_row("SELECT r.favorite,r.disposition,r.manual_metadata FROM recordings r JOIN catalog_listings l ON l.recording_id=r.id WHERE l.id=?1", [&track.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(err)?;
    track.favorite = values.0;
    track.disposition = values.1;
    let manual: serde_json::Value = decode(values.2)?;
    let mut metadata = serde_json::to_value(&track).map_err(err)?;
    if let (Some(obj), Some(overrides)) = (metadata.as_object_mut(), manual.as_object()) {
        for (k, v) in overrides {
            obj.insert(k.clone(), v.clone());
        }
    }
    track = serde_json::from_value(metadata).map_err(err)?;
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    let mut manual_fields = track.provenance["manualFields"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(overrides) = manual.as_object() {
        for key in overrides.keys() {
            manual_fields.insert(key.clone(), serde_json::json!(true));
        }
    }
    track.provenance["manualFields"] = serde_json::Value::Object(manual_fields.clone());
    let mut asset_stmt = conn
        .prepare("SELECT id,path,hash,payload FROM local_assets WHERE recording_id=(SELECT recording_id FROM catalog_listings WHERE id=?1) ORDER BY rowid ASC")
        .map_err(err)?;
    let asset_rows = asset_stmt
        .query_map([&track.id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    let asset_row = asset_rows
        .iter()
        .position(|(_, path, _, _)| Path::new(path).is_file())
        .or_else(|| (!asset_rows.is_empty()).then_some(0))
        .map(|index| asset_rows[index].clone());
    track.asset_id = None;
    track.local_path = None;
    track.local_availability = "none".into();
    if let Some((asset_id, path, hash, payload)) = asset_row {
        let asset: LocalAsset = decode(payload)?;
        if asset.track.provenance["autotag"].is_object() {
            let overlay = serde_json::to_value(&asset.track).map_err(err)?;
            let mut display = serde_json::to_value(&track).map_err(err)?;
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
                "trackNumber",
                "trackTotal",
                "discNumber",
                "discTotal",
                "remixers",
            ] {
                if manual_fields.get(field).is_none()
                    && overlay
                        .get(field)
                        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
                {
                    display[field] = overlay[field].clone();
                }
            }
            track = serde_json::from_value(display).map_err(err)?;
            track.provenance["autotag"] = asset.track.provenance["autotag"].clone();
        }
        let is_available = Path::new(&path).is_file();
        track.asset_id = Some(asset_id.clone());
        track.local_availability = if is_available { "available" } else { "missing" }.into();
        if is_available {
            track.local_path = Some(path);
        }
        let analysis: Option<(String, String, String)> = conn
            .query_row(
                "SELECT status,version,payload FROM asset_analyses WHERE asset_id=?1 AND file_hash=?2 ORDER BY updated_at DESC LIMIT 1",
                params![asset_id, hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(err)?;
        let analysis = analysis
            .map(|(status, version, payload)| {
                Ok::<_, String>((status, version, decode::<serde_json::Value>(payload)?))
            })
            .transpose()?;
        // Preserve provider values and explicit user edits. Saved file tags and
        // matching analysis only fill fields that remain absent after overrides.
        let manual_has_bpm = manual.get("bpm").is_some();
        let manual_has_key = manual.get("key").is_some();
        if track.bpm.is_none() && !manual_has_bpm {
            track.bpm = asset.track.bpm;
            if track.bpm.is_some() {
                copy_analysis_provenance(&mut track, &asset.track, "bpmAnalysis");
            } else if let Some((_, _, payload)) = analysis
                .as_ref()
                .filter(|(status, ..)| matches!(status.as_str(), "complete" | "completed"))
            {
                track.bpm = payload.get("bpm").and_then(serde_json::Value::as_f64);
                if track.bpm.is_some() {
                    copy_payload_provenance(&mut track, payload, "bpmAnalysis");
                }
            }
        }
        if track.key.is_none() && !manual_has_key {
            track.key = asset.track.key.clone();
            if track.key.is_some() {
                copy_analysis_provenance(&mut track, &asset.track, "keyAnalysis");
            } else if let Some((_, _, payload)) = analysis
                .as_ref()
                .filter(|(status, ..)| matches!(status.as_str(), "complete" | "completed"))
            {
                track.key = payload
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                if track.key.is_some() {
                    copy_payload_provenance(&mut track, payload, "keyAnalysis");
                }
            }
        }
        if let Some((status, version, payload)) = analysis {
            if !track.provenance.is_object() {
                track.provenance = serde_json::json!({});
            }
            track.provenance["localAnalysis"] = serde_json::json!({"status":status,"version":version,"provenance":payload.get("provenance").unwrap_or(&payload)});
        }
    }
    if track.normalized_genre.is_none() {
        track.normalized_genre = track.genre.as_ref().map(|g| {
            g.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        });
    }
    Ok(track)
}

fn copy_analysis_provenance(target: &mut Track, source: &Track, key: &str) {
    if let Some(value) = source.provenance.get(key).filter(|value| !value.is_null()) {
        copy_provenance_value(target, key, value.clone());
    }
}

fn copy_payload_provenance(target: &mut Track, payload: &serde_json::Value, key: &str) {
    if let Some(value) = payload.get(key).filter(|value| !value.is_null()) {
        copy_provenance_value(target, key, value.clone());
    }
}

fn copy_provenance_value(target: &mut Track, key: &str, value: serde_json::Value) {
    if !target.provenance.is_object() {
        target.provenance = serde_json::json!({});
    }
    target.provenance[key] = value;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Candidate;
    fn track(id: &str, version: &str) -> Track {
        Track {
            id: format!("fixture:{id}"),
            provider: "fixture".into(),
            provider_id: id.into(),
            artist: "Artist".into(),
            title: "Title".into(),
            version: Some(version.into()),
            ..Default::default()
        }
    }
    fn db() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(&d.path().join("library.sqlite")).unwrap();
        (d, s)
    }
    #[test]
    fn asset_lookup_by_path_returns_only_the_indexed_exact_path() {
        let (_directory, store) = db();
        let track = store.save_track(&track("path-lookup", "Original")).unwrap();
        let asset = LocalAsset {
            id: "asset-path-lookup".into(),
            recording_id: track.id.clone(),
            path: "/library/album/track.flac".into(),
            hash: "hash-path-lookup".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "copy".into(),
            track,
        };
        store.save_asset(&asset).unwrap();
        let found = store
            .get_asset_by_path("/library/album/track.flac")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, asset.id);
        assert!(store
            .get_asset_by_path("/library/album/Track.flac")
            .unwrap()
            .is_none());
    }
    #[test]
    fn hydration_merges_acquisition_and_database_manual_fields() {
        let (_directory, store) = db();
        let mut source = track("manual-merge", "Original");
        source.artist = "Catalog Artist".into();
        source.title = "Catalog Title".into();
        source.provenance = serde_json::json!({"acquisition":{"provider":"youtube"},"manualFields":{"artist":true}});
        store.save_track(&source).unwrap();
        let mut edited = store.get_track(&source.id).unwrap().unwrap();
        edited.title = "Chosen Title".into();
        let hydrated = store.update_track(&edited).unwrap();
        assert_eq!(hydrated.provenance["manualFields"]["artist"], true);
        assert_eq!(hydrated.provenance["manualFields"]["title"], true);
        assert_eq!(hydrated.provenance["acquisition"]["provider"], "youtube");
        assert_eq!(hydrated.artist, "Catalog Artist");
        assert_eq!(hydrated.title, "Chosen Title");
        assert!(hydrated.provenance["manualFields"].get("album").is_none());
    }

    #[test]
    fn autotag_overlay_respects_acquisition_manual_flags_and_fills_other_fields() {
        let (_directory, store) = db();
        let mut source = track("overlay", "Original");
        source.artist = "Chosen Artist".into();
        source.title = "Chosen Title".into();
        source.provenance = serde_json::json!({"manualFields":{"artist":true}});
        store.save_track(&source).unwrap();
        let mut chosen = store.get_track(&source.id).unwrap().unwrap();
        chosen.title = "Manual Title".into();
        let source = store.update_track(&chosen).unwrap();
        let mut tagged = source.clone();
        tagged.artist = "Catalog Artist".into();
        tagged.title = "Catalog Title".into();
        tagged.album = Some("Matched Album".into());
        tagged.provenance["autotag"] = serde_json::json!({"provider":"musicbrainz"});
        let asset = LocalAsset {
            id: "overlay-asset".into(),
            recording_id: source.id.clone(),
            path: "/not-present/overlay.flac".into(),
            hash: "asset-hash".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "managed".into(),
            track: tagged,
        };
        store.save_asset(&asset).unwrap();
        let visible = store.get_track(&source.id).unwrap().unwrap();
        assert_eq!(visible.artist, "Chosen Artist");
        assert_eq!(visible.title, "Manual Title");
        assert_eq!(visible.album.as_deref(), Some("Matched Album"));
        assert_eq!(visible.provenance["manualFields"]["artist"], true);
        assert_eq!(visible.provenance["autotag"]["provider"], "musicbrainz");
    }

    #[test]
    fn stale_autotag_hash_cannot_publish_or_replace_local_asset() {
        let (_directory, store) = db();
        let source = track("stale-tag", "Original");
        store.save_track(&source).unwrap();
        let asset = LocalAsset {
            id: "stale-asset".into(),
            recording_id: source.id.clone(),
            path: "/not-present/stale.flac".into(),
            hash: "current-hash".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "managed".into(),
            track: source,
        };
        store.save_asset(&asset).unwrap();
        let mut updated = asset.clone();
        updated.hash = "tagged-hash".into();
        let result = store.save_autotagged_asset(
            &updated,
            "stale-hash",
            "autotag-v1",
            &serde_json::json!({"status":"applied"}),
        );
        assert!(result.is_err());
        assert_eq!(store.list_assets().unwrap()[0].hash, "current-hash");
        assert!(store.list_autotag_records().unwrap().is_empty());
    }

    #[test]
    fn update_track_marks_only_fields_changed_from_hydrated_values() {
        let (_directory, store) = db();
        let mut source = track("manual-diff", "Original");
        source.artist = "Catalog Artist".into();
        source.title = "Catalog Title".into();
        source.album = Some("Catalog Album".into());
        store.save_track(&source).unwrap();
        let mut edit = store.get_track(&source.id).unwrap().unwrap();
        edit.artist = "Chosen Artist".into();
        store.update_track(&edit).unwrap();
        let loaded = store.get_track(&source.id).unwrap().unwrap();
        assert_eq!(loaded.provenance["manualFields"]["artist"], true);
        assert!(loaded.provenance["manualFields"].get("title").is_none());
        assert!(loaded.provenance["manualFields"].get("album").is_none());
        assert_eq!(loaded.title, "Catalog Title");
        assert_eq!(loaded.album.as_deref(), Some("Catalog Album"));
    }
    #[test]
    fn library_shows_saved_metadata_without_requiring_downloaded_audio() {
        let (_directory, store) = db();
        let first = track("1", "Extended Mix");
        let second = track("2", "Radio Edit");
        store.save_track(&first).unwrap();
        store.save_track(&second).unwrap();
        assert!(store.list_assets().unwrap().is_empty());
        let library = store.list_tracks("library", None).unwrap();
        assert_eq!(library.len(), 2);
        assert!(library.iter().all(|item| item.local_path.is_none()));
        store.merge_recordings(&first.id, &[second.id]).unwrap();
        let merged = store.list_tracks("library", None).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, first.id);
    }

    #[test]
    fn remembered_audio_candidate_preserves_metadata_and_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let store = Store::open(&path).unwrap();
        let catalog = Track {
            id: "spotify:catalog-id".into(),
            provider: "spotify".into(),
            provider_id: "catalog-id".into(),
            source_url: "https://open.spotify.com/track/catalog-id".into(),
            artist: "Catalog Artist".into(),
            title: "Catalog Title".into(),
            isrc: Some("CATALOG12345".into()),
            ..Track::default()
        };
        store.save_track(&catalog).unwrap();
        let mut manually_edited = catalog.clone();
        manually_edited.artist = "Manual Artist".into();
        manually_edited.title = "Manual Title".into();
        manually_edited.isrc = Some("MANUAL12345".into());
        store.update_track(&manually_edited).unwrap();
        let candidate = Candidate {
            id: "youtube:Known12345".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=Known12345".into(),
            title: "Unverified source title".into(),
            permission: "unknown".into(),
            confidence: "low".into(),
            ..Candidate::default()
        };
        let remembered = store
            .save_audio_match(&manually_edited, &candidate)
            .unwrap();
        assert_eq!(remembered.artist, "Manual Artist");
        assert_eq!(remembered.title, "Manual Title");
        assert_eq!(remembered.isrc.as_deref(), Some("MANUAL12345"));
        assert_eq!(remembered.provenance["candidateUrl"], candidate.url);
        drop(store);

        let reopened = Store::open(&path).unwrap();
        let persisted = reopened.get_track(&catalog.id).unwrap().unwrap();
        assert_eq!(persisted.artist, "Manual Artist");
        assert_eq!(persisted.title, "Manual Title");
        assert_eq!(persisted.isrc.as_deref(), Some("MANUAL12345"));
        assert_eq!(persisted.provenance["candidateUrl"], candidate.url);
        assert_eq!(persisted.provenance["audioCandidate"]["id"], candidate.id);
    }
    fn browser_snapshot(ids: &[&str], complete: bool) -> BrowserSnapshot {
        serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"provider":"spotify","kind":"playlist","title":"Imported playlist",
            "sourceUrl":"https://open.spotify.com/playlist/1234567890123456789012",
            "capturedAt":"2026-10-11T12:00:00Z","complete":complete,
            "expectedTotal":if complete {ids.len()} else {ids.len()+1},"warnings":[],
            "entries":ids.iter().enumerate().map(|(i,id)|serde_json::json!({
                "providerId":id,"artist":"Artist","title":format!("Track {id}"),
                "position":i+1,"rank":i+1
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    }
    fn import(s: &Store, snapshot: &BrowserSnapshot) -> BrowserImportResult {
        let chart = crate::browser_bridge::validate_snapshot(snapshot).unwrap();
        s.import_browser_chart(&chart, snapshot).unwrap()
    }
    #[test]
    fn browser_partial_union_complete_replaces_and_history_remains() {
        let (_d, s) = db();
        let a = "1111111111111111111111";
        let b = "2222222222222222222222";
        let c = "3333333333333333333333";
        let first = import(&s, &browser_snapshot(&[a, b], false));
        let partial = import(&s, &browser_snapshot(&[b, c], false));
        assert_eq!(first.collection_id, partial.collection_id);
        let members = s
            .list_tracks("collection", Some(&first.collection_id))
            .unwrap();
        assert_eq!(
            members
                .iter()
                .map(|t| t.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec![a, b, c]
        );
        let complete = import(&s, &browser_snapshot(&[c, a], true));
        assert!(complete.complete);
        let members = s
            .list_tracks("collection", Some(&first.collection_id))
            .unwrap();
        assert_eq!(
            members
                .iter()
                .map(|t| t.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec![c, a]
        );
        assert!(members.iter().all(|t| t.rank.is_none()));
        let historic = s.load_browser_import(&first.id).unwrap();
        assert_eq!(historic.tracks.len(), 2);
        assert_eq!(historic.tracks[0].provenance["playlistPosition"], 1);
        assert_eq!(s.list_browser_imports().unwrap().len(), 3);
        assert_eq!(s.list_collections().unwrap().len(), 1);
    }
    #[test]
    fn browser_reimport_preserves_manual_merge_and_counts_duplicate_occurrences() {
        let (_d, s) = db();
        let a = "1111111111111111111111";
        let b = "2222222222222222222222";
        let snapshot = browser_snapshot(&[a, b, a], true);
        let first = import(&s, &snapshot);
        assert_eq!((first.captured_count, first.imported_count), (3, 2));
        assert!(first.complete);
        assert!(!first.warnings.is_empty());
        let primary = format!("spotify:{a}");
        s.set_favorite(&s.get_track(&primary).unwrap().unwrap(), true)
            .unwrap();
        s.merge_recordings(&primary, &[format!("spotify:{b}")])
            .unwrap();
        let again = import(&s, &snapshot);
        let members = s
            .list_tracks("collection", Some(&again.collection_id))
            .unwrap();
        assert_eq!(members.len(), 1);
        assert!(members[0].favorite);
        assert_eq!(s.recording_id(&format!("spotify:{b}")).unwrap(), primary);
        assert_eq!(
            s.load_browser_import(&first.id).unwrap().tracks[0].provenance["occurrencePositions"],
            serde_json::json!([1, 3])
        );
        let displayed = s.load_browser_import(&first.id).unwrap();
        assert_eq!(displayed.tracks.len(), 3);
        assert_eq!(
            displayed
                .tracks
                .iter()
                .map(|track| track.provenance["playlistPosition"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
    #[test]
    fn browser_complete_without_known_total_does_not_remove_existing_members() {
        let (_d, s) = db();
        let a = "1111111111111111111111";
        let b = "2222222222222222222222";
        let first = import(&s, &browser_snapshot(&[a, b], true));
        let mut snapshot = browser_snapshot(&[b], true);
        snapshot.expected_total = None;
        let result = import(&s, &snapshot);
        assert!(!result.complete);
        assert_eq!(
            s.list_tracks("collection", Some(&first.collection_id))
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn browser_schema_migrates_v1_database_and_survives_reopen() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("old.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        drop(conn);
        let s = Store::open(&path).unwrap();
        let result = import(&s, &browser_snapshot(&["1111111111111111111111"], true));
        drop(s);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.list_browser_imports().unwrap()[0].id, result.id);
        assert_eq!(
            reopened
                .load_browser_import(&result.id)
                .unwrap()
                .tracks
                .len(),
            1
        );
        assert!(reopened.load_browser_import("unknown").is_err());
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            6
        );
    }
    #[test]
    fn personal_playlists_are_independent_and_source_snapshot_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let store = Store::open(&path).unwrap();
        let source = import(
            &store,
            &browser_snapshot(&["1111111111111111111111", "2222222222222222222222"], true),
        );
        let tracks = store
            .list_tracks("collection", Some(&source.collection_id))
            .unwrap();
        let first = store.create_collection("Warmup").unwrap();
        let second = store.create_collection("Closing Set").unwrap();
        store.add_to_collection(&first.id, &tracks).unwrap();
        store.add_to_collection(&second.id, &tracks).unwrap();

        store
            .remove_from_collection(&first.id, &[tracks[0].id.clone()])
            .unwrap();
        let renamed = store.rename_collection(&second.id, " Late Set ").unwrap();
        assert_eq!(renamed.name, "Late Set");
        assert_eq!(renamed.track_count, 2);
        assert_eq!(
            store
                .list_tracks("collection", Some(&first.id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&second.id))
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&source.collection_id))
                .unwrap()
                .len(),
            2
        );
        drop(store);

        let reopened = Store::open(&path).unwrap();
        let collections = reopened.list_collections().unwrap();
        assert_eq!(collections.len(), 3);
        assert!(collections
            .iter()
            .any(|c| c.id == first.id && c.name == "Warmup" && c.track_count == 1));
        assert!(collections
            .iter()
            .any(|c| c.id == second.id && c.name == "Late Set" && c.track_count == 2));
        assert_eq!(
            reopened
                .list_tracks("collection", Some(&source.collection_id))
                .unwrap()
                .len(),
            2
        );
        assert_eq!(reopened.list_browser_imports().unwrap().len(), 1);
        reopened.delete_collection(&source.collection_id).unwrap();
        assert!(reopened.list_browser_imports().unwrap().is_empty());
        assert!(reopened
            .list_deleted_collections()
            .unwrap()
            .iter()
            .any(|collection| collection.id == source.collection_id));
        reopened.restore_collection(&source.collection_id).unwrap();
        assert_eq!(reopened.list_browser_imports().unwrap().len(), 1);
        assert_eq!(
            reopened
                .load_browser_import(&source.id)
                .unwrap()
                .tracks
                .len(),
            2
        );
    }
    #[test]
    fn collection_edits_reject_unknown_collections_and_invalid_names() {
        let (_directory, store) = db();
        assert!(store.rename_collection("missing", "Valid").is_err());
        assert!(store.rename_collection("missing", "  ").is_err());
        assert!(store.remove_from_collection("missing", &[]).is_err());
    }
    #[test]
    fn provider_cache_expires_and_artist_mappings_persist() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .set_provider_cache(
                "artist-search",
                "query:house",
                &serde_json::json!(["ok"]),
                60,
            )
            .unwrap();
        assert_eq!(
            store
                .get_provider_cache("artist-search", "query:house")
                .unwrap(),
            Some(serde_json::json!(["ok"]))
        );
        store
            .set_provider_cache(
                "artist-search",
                "expired",
                &serde_json::json!({"stale":true}),
                0,
            )
            .unwrap();
        assert_eq!(
            store
                .get_provider_cache("artist-search", "expired")
                .unwrap(),
            None
        );
        store
            .save_artist_mapping("discogs", "123", "artist:local-1")
            .unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened
                .get_artist_mapping("discogs", "123")
                .unwrap()
                .as_deref(),
            Some("artist:local-1")
        );
        assert_eq!(
            reopened
                .get_provider_cache("artist-search", "query:house")
                .unwrap(),
            Some(serde_json::json!(["ok"]))
        );
    }
    #[test]
    fn local_asset_hydration_fills_missing_metadata_without_changing_catalog_or_manual_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let file = directory.path().join("track.flac");
        std::fs::write(&file, b"fixture audio bytes").unwrap();
        let store = Store::open(&path).unwrap();
        let source = track("local-hydrate", "Extended Mix");
        store.save_track(&source).unwrap();
        let asset = LocalAsset {
            id: "asset-1".into(),
            recording_id: source.id.clone(),
            path: file.to_string_lossy().into_owned(),
            hash: "source-hash-1".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "copied".into(),
            track: Track {
                bpm: None,
                key: Some("8A".into()),
                ..source.clone()
            },
        };
        store.save_asset(&asset).unwrap();
        let version = "bpm-v2-key-v1";
        let queued_payload = serde_json::json!({"message":"awaiting analysis"});
        store
            .save_analysis_record(&asset.id, &asset.hash, version, "running", &queued_payload)
            .unwrap();
        let payload = serde_json::json!({
            "bpm": 127.0,
            "key": null,
            "bpmAnalysis": {"method":"test autocorrelation","confidence":0.91},
            "provenance": {"method":"test autocorrelation","confidence":0.91}
        });
        let mut analyzed_asset = asset.clone();
        analyzed_asset.hash = "source-hash-2".into();
        analyzed_asset.track.bpm = None;
        store
            .save_analyzed_asset(&analyzed_asset, &asset.hash, version, &payload)
            .unwrap();
        let records = store.list_analysis_records().unwrap();
        let old_record = records
            .iter()
            .find(|r| r["inputHash"] == asset.hash)
            .unwrap();
        assert_eq!(old_record["status"], "superseded");
        let record = records
            .iter()
            .find(|r| r["inputHash"] == analyzed_asset.hash)
            .unwrap();
        assert_eq!(record["assetId"], asset.id);
        assert_eq!(record["version"], version);
        assert_eq!(record["status"], "completed");
        assert_eq!(record["payload"], payload);
        let loaded = store.list_tracks("library", None).unwrap().remove(0);
        assert_eq!(loaded.bpm, Some(127.0));
        assert_eq!(loaded.key.as_deref(), Some("8A"));
        assert_eq!(
            loaded.provenance["bpmAnalysis"]["method"],
            "test autocorrelation"
        );
        assert!(loaded.provenance.get("keyAnalysis").is_none());
        assert_eq!(loaded.asset_id.as_deref(), Some("asset-1"));
        assert_eq!(loaded.local_availability, "available");
        assert_eq!(
            loaded.local_path.as_deref(),
            Some(file.to_string_lossy().as_ref())
        );
        {
            let conn = store.connection.lock().unwrap();
            let raw: String = conn
                .query_row(
                    "SELECT metadata FROM catalog_listings WHERE id=?1",
                    [&source.id],
                    |row| row.get(0),
                )
                .unwrap();
            let raw: Track = decode(raw).unwrap();
            assert_eq!(raw.bpm, None);
            assert_eq!(raw.key, None);
        }
        let mut manual = loaded.clone();
        manual.bpm = Some(130.0);
        manual.key = Some("10A".into());
        store.update_track(&manual).unwrap();
        let loaded = store.get_track(&source.id).unwrap().unwrap();
        assert_eq!(loaded.bpm, Some(130.0));
        assert_eq!(loaded.key.as_deref(), Some("10A"));
        std::fs::remove_file(&file).unwrap();
        let missing = store.get_track(&source.id).unwrap().unwrap();
        assert_eq!(missing.local_availability, "missing");
        assert!(missing.local_path.is_none());
        assert_eq!(missing.asset_id.as_deref(), Some("asset-1"));
        drop(store);

        let reopened = Store::open(&path).unwrap();
        let reopened_records = reopened.list_analysis_records().unwrap();
        assert_eq!(reopened_records.len(), 2);
        assert!(reopened_records
            .iter()
            .any(|r| r["inputHash"] == asset.hash && r["status"] == "superseded"));
        assert!(reopened_records
            .iter()
            .any(|r| r["inputHash"] == analyzed_asset.hash && r["payload"] == payload));
        let loaded = reopened.get_track(&source.id).unwrap().unwrap();
        assert_eq!(loaded.bpm, Some(130.0));
        assert_eq!(loaded.local_availability, "missing");
    }
    #[test]
    fn collection_deduplicates_identity_not_version() {
        let (_d, s) = db();
        let c = s.create_collection("House").unwrap();
        let a = track("1", "Extended Mix");
        let b = track("2", "Radio Edit");
        s.add_to_collection(&c.id, &[a.clone(), a, b]).unwrap();
        assert_eq!(s.list_tracks("collection", Some(&c.id)).unwrap().len(), 2);
    }
    #[test]
    fn soft_deleted_collection_restores_same_order_without_deleting_audio_or_other_memberships() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("library.sqlite");
        let audio_path = directory.path().join("track.flac");
        std::fs::write(&audio_path, b"audio fixture").unwrap();
        let store = Store::open(&db_path).unwrap();
        let first = track("recover-a", "Extended Mix");
        let second = track("recover-b", "Radio Edit");
        store.save_track(&first).unwrap();
        store.save_track(&second).unwrap();
        store
            .save_asset(&LocalAsset {
                id: "recover-asset".into(),
                recording_id: first.id.clone(),
                path: audio_path.to_string_lossy().into_owned(),
                hash: "recover-hash".into(),
                source_codec: "flac".into(),
                final_codec: "flac".into(),
                source_bitrate: None,
                final_bitrate: None,
                processing: "copied".into(),
                track: first.clone(),
            })
            .unwrap();
        let deleted = store.create_collection("Recoverable set").unwrap();
        let other = store.create_collection("Other set").unwrap();
        store
            .replace_playlist_entries(&deleted.id, &[first.clone(), second.clone(), first.clone()])
            .unwrap();
        store
            .add_to_collection(&other.id, &[first.clone()])
            .unwrap();

        let tombstone = store.delete_collection(&deleted.id).unwrap();
        assert_eq!(tombstone.id, deleted.id);
        assert!(store
            .list_collections()
            .unwrap()
            .iter()
            .all(|item| item.id != deleted.id));
        assert_eq!(store.list_deleted_collections().unwrap()[0].id, deleted.id);
        assert!(store.list_tracks("collection", Some(&deleted.id)).is_err());
        assert_eq!(
            store
                .list_tracks("collection", Some(&other.id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.list_assets().unwrap().len(), 1);
        assert!(audio_path.is_file());
        drop(store);

        let reopened = Store::open(&db_path).unwrap();
        let restored = reopened.restore_collection(&deleted.id).unwrap();
        assert_eq!(restored.id, deleted.id);
        assert_eq!(restored.name, "Recoverable set");
        assert_eq!(restored.track_count, 3);
        assert!(reopened.list_deleted_collections().unwrap().is_empty());
        let entries = reopened
            .list_tracks("collection", Some(&deleted.id))
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec![first.id.as_str(), second.id.as_str(), first.id.as_str()]
        );
        assert_eq!(reopened.list_assets().unwrap().len(), 1);
        assert!(audio_path.is_file());
    }

    #[test]
    fn downloads_membership_uses_reserved_identity_and_is_idempotent() {
        let (_directory, store) = db();
        let user_collection = store.create_collection("Downloads").unwrap();
        let legacy_chapters = store
            .create_collection("Parent recording — Chapters (old-job)")
            .unwrap();
        let chapter = track("downloaded-chapter", "Excerpt");
        store.save_track(&chapter).unwrap();
        store
            .add_to_collection(&legacy_chapters.id, &[chapter.clone()])
            .unwrap();
        let asset = LocalAsset {
            id: "downloaded-chapter-asset".into(),
            recording_id: chapter.id.clone(),
            path: "/not-present/chapter.flac".into(),
            hash: "chapter-hash".into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            processing: "excerpt".into(),
            track: chapter.clone(),
        };
        store.save_asset(&asset).unwrap();

        let downloads = store.add_asset_to_downloads(&asset).unwrap();
        store.add_asset_to_downloads(&asset).unwrap();

        assert_eq!(downloads.id, DOWNLOADS_COLLECTION_ID);
        assert_eq!(downloads.name, "Downloads");
        assert_eq!(user_collection.id != downloads.id, true);
        assert_eq!(
            store
                .list_tracks("collection", Some(&downloads.id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_tracks("collection", Some(&legacy_chapters.id))
                .unwrap()
                .len(),
            1
        );
        let names = store.list_collections().unwrap();
        assert_eq!(names.iter().filter(|c| c.name == "Downloads").count(), 2);
        assert!(names.iter().any(|c| c.id == legacy_chapters.id));
    }

    #[test]
    fn ensure_downloads_collection_is_startup_safe_and_restores_soft_deletion() {
        let (_directory, store) = db();
        let first = store.ensure_downloads_collection().unwrap();
        let second = store.ensure_downloads_collection().unwrap();
        assert_eq!(first.id, DOWNLOADS_COLLECTION_ID);
        assert_eq!(second.id, first.id);
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .iter()
                .filter(|c| c.id == DOWNLOADS_COLLECTION_ID)
                .count(),
            1
        );
        store.delete_collection(DOWNLOADS_COLLECTION_ID).unwrap();
        let restored = store.ensure_downloads_collection().unwrap();
        assert_eq!(restored.id, first.id);
        assert!(store
            .list_collections()
            .unwrap()
            .iter()
            .any(|c| c.id == DOWNLOADS_COLLECTION_ID));
    }

    #[test]
    fn managed_download_backfill_skips_unmarked_local_files() {
        let (directory, store) = db();
        let acquired_path = directory.path().join("downloaded.flac");
        std::fs::write(&acquired_path, b"managed fixture").unwrap();
        let acquired = track("backfill-acquired", "Original Mix");
        let mut imported = track("backfill-imported", "Original Mix");
        imported.provenance = serde_json::json!({"source":"local file tags"});
        for (id, track, path, provenance) in [
            (
                "managed-asset",
                acquired,
                acquired_path.clone(),
                serde_json::json!({"publicationJobId":"job-1"}),
            ),
            (
                "imported-asset",
                imported,
                directory.path().join("imported.flac"),
                serde_json::json!({}),
            ),
        ] {
            let mut track = track;
            if provenance.get("publicationJobId").is_some() {
                track.provenance = provenance;
            }
            store
                .save_asset(&LocalAsset {
                    id: id.into(),
                    recording_id: track.id.clone(),
                    path: path.to_string_lossy().into_owned(),
                    hash: format!("{id}-hash"),
                    source_codec: "flac".into(),
                    final_codec: "flac".into(),
                    source_bitrate: None,
                    final_bitrate: None,
                    processing: "managed".into(),
                    track,
                })
                .unwrap();
        }
        assert_eq!(store.backfill_managed_downloads().unwrap(), 1);
        let collection = store.ensure_downloads_collection().unwrap();
        let tracks = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "fixture:backfill-acquired");
    }
    #[test]
    fn ordered_playlist_positions_follow_reordered_duplicate_entries_without_rewriting_source() {
        let (_directory, store) = db();
        let mut a = track("playlist-a", "Original Mix");
        a.provenance = serde_json::json!({
            "sourceKind":"playlist","position":7,"playlistPosition":7,"occurrencePositions":[7,9]
        });
        let mut b = track("playlist-b", "Original Mix");
        b.provenance = serde_json::json!({
            "sourceKind":"playlist","position":8,"playlistPosition":8,"occurrencePositions":[8]
        });
        store.save_track(&a).unwrap();
        store.save_track(&b).unwrap();
        let collection = store.create_collection("Reordered playlist").unwrap();
        store
            .replace_playlist_entries(&collection.id, &[b.clone(), a.clone(), a.clone()])
            .unwrap();

        let entries = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec![b.id.as_str(), a.id.as_str(), a.id.as_str()]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.provenance["playlistPosition"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(entries[0].provenance["position"], 8);
        assert_eq!(entries[1].provenance["position"], 7);
        assert_eq!(entries[2].provenance["position"], 7);

        // The move command performs this list/reorder/replace round trip. Returned
        // view positions must not leak back into the stored source provenance.
        let reordered = vec![entries[2].clone(), entries[0].clone(), entries[1].clone()];
        store
            .replace_playlist_entries(&collection.id, &reordered)
            .unwrap();
        let moved = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(
            moved
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec![a.id.as_str(), b.id.as_str(), a.id.as_str()]
        );
        assert_eq!(
            moved
                .iter()
                .map(|entry| entry.provenance["playlistPosition"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let saved_a = store.get_track(&a.id).unwrap().unwrap();
        let saved_b = store.get_track(&b.id).unwrap().unwrap();
        assert_eq!(saved_a.provenance["playlistPosition"], 7);
        assert_eq!(
            saved_a.provenance["occurrencePositions"],
            serde_json::json!([7, 9])
        );
        assert_eq!(saved_b.provenance["playlistPosition"], 8);
    }
    #[test]
    fn favorites_and_manual_metadata_survive_provider_refresh() {
        let (_d, s) = db();
        let mut t = track("1", "Original Mix");
        s.set_favorite(&t, true).unwrap();
        t.bpm = Some(123.5);
        s.update_track(&t).unwrap();
        t.bpm = Some(120.0);
        s.save_track(&t).unwrap();
        let loaded = s.get_track(&t.id).unwrap().unwrap();
        assert!(loaded.favorite);
        assert_eq!(loaded.bpm, Some(123.5));
    }
    #[test]
    fn user_actions_do_not_replace_raw_catalog_metadata() {
        let (_d, s) = db();
        let mut t = track("1", "Original Mix");
        t.bpm = Some(120.0);
        s.save_track(&t).unwrap();
        t.bpm = Some(123.5);
        s.update_track(&t).unwrap();
        s.set_favorite(&t, true).unwrap();
        let conn = s.connection.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&t.id],
                |r| r.get(0),
            )
            .unwrap();
        let raw: Track = decode(raw).unwrap();
        assert_eq!(raw.bpm, Some(120.0));
        drop(conn);
        assert_eq!(s.get_track(&t.id).unwrap().unwrap().bpm, Some(123.5));
    }
    #[test]
    fn verified_merge_combines_memberships_without_erasing_listings() {
        let (_d, s) = db();
        let c = s.create_collection("House").unwrap();
        let a = track("1", "Original Mix");
        let mut b = track("2", "Original Mix");
        b.provider = "other".into();
        b.id = "other:2".into();
        s.add_to_collection(&c.id, &[a.clone(), b.clone()]).unwrap();
        s.merge_recordings(&a.id, &[b.id.clone()]).unwrap();
        assert_eq!(s.list_collections().unwrap()[0].track_count, 1);
        assert_eq!(s.list_tracks("collection", Some(&c.id)).unwrap().len(), 1);
        assert_eq!(s.list_tracks("all", None).unwrap().len(), 2);
        s.save_track(&b).unwrap();
        assert_eq!(s.recording_id(&b.id).unwrap(), a.id);
    }
    #[test]
    fn invalid_alias_id_is_rejected_without_identity_corruption() {
        let (_d, s) = db();
        let a = track("1", "Original Mix");
        s.save_track(&a).unwrap();
        let mut b = a.clone();
        b.id = "wrong:alias".into();
        assert!(s.save_track(&b).is_err());
        assert_eq!(s.get_track(&a.id).unwrap().unwrap().id, a.id);
        assert_eq!(s.list_tracks("all", None).unwrap().len(), 1);
    }
    #[test]
    fn snapshots_preserve_rank_and_missing_metadata() {
        let (_d, s) = db();
        let mut t = track("1", "Extended Mix");
        t.rank = Some(8);
        let c = Chart {
            id: "x".into(),
            provider: "fixture".into(),
            title: "House".into(),
            source_url: "https://example.invalid".into(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
            tracks: vec![t],
            stale: false,
            warning: None,
        };
        let c = s.save_chart(&c).unwrap();
        let c = s.load_snapshot(&c.id).unwrap();
        assert_eq!(c.tracks[0].rank, Some(8));
        assert_eq!(c.tracks[0].bpm, None);
    }
}

fn hydrate_asset(conn: &Connection, mut asset: LocalAsset) -> Result<LocalAsset> {
    asset.recording_id = conn
        .query_row(
            "SELECT recording_id FROM local_assets WHERE id=?1",
            [&asset.id],
            |r| r.get(0),
        )
        .map_err(err)?;
    asset.track = hydrate(conn, asset.track)?;
    Ok(asset)
}
