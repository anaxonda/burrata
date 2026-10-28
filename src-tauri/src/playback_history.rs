use crate::{
    models::Track,
    radio::{RadioStation, RadioTrackMetadata},
    storage::Store,
};
use chrono::{DateTime, Days, NaiveDate, SecondsFormat, Utc};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, MutexGuard};
use std::{collections::HashMap, path::Path};

const INCLUDE_RADIO_SONGS_KEY: &str = "includeRadioSongsInPlaybackHistory.v1";

/// Existing profiles without an explicit preference default to enabled.
pub fn include_radio_songs_in_history(store: &Store) -> Result<bool, String> {
    Ok(store
        .get_json_preference(INCLUDE_RADIO_SONGS_KEY)?
        .and_then(|value| value.as_bool().map(|enabled| enabled.to_owned()))
        .unwrap_or(true))
}

pub fn set_include_radio_songs_in_history(store: &Store, enabled: bool) -> Result<(), String> {
    store.set_json_preference(INCLUDE_RADIO_SONGS_KEY, &Value::Bool(enabled))
}

/// Build a reusable unverified song identity from station metadata. The station
/// stream never becomes the song's `source_url`.
pub fn radio_song_track(station: &RadioStation, metadata: &RadioTrackMetadata) -> Track {
    let normalized_artist = metadata.artist.trim().to_lowercase();
    let normalized_title = metadata.title.trim().to_lowercase();
    let track_hash = format!(
        "{:x}",
        Sha256::digest(format!("{normalized_artist}\n{normalized_title}").as_bytes())
    );
    Track {
        id: format!("radio_song:{track_hash}"),
        provider: "radio_song".into(),
        provider_id: track_hash,
        artist: metadata.artist.trim().to_owned(),
        title: metadata.title.trim().to_owned(),
        album: metadata.album.clone(),
        artwork_url: metadata.artwork_url.clone(),
        isrc: metadata
            .identifiers
            .as_ref()
            .and_then(|ids| ids.isrc.as_deref())
            .and_then(crate::recording_identity::normalize_isrc),
        source_url: String::new(),
        provenance: serde_json::json!({
            "sourceKind":"radioSong",
            "identity":"unverified station metadata",
            "catalogVerified":false,
            "radioStationId":station.id,
            "radioStationName":station.name,
            "radioStationUrl":station.homepage_url.as_deref().unwrap_or(&station.source_url),
            "radioMetadataConfidence":metadata.metadata_confidence,
            "recognitionIdentifiers":metadata.identifiers,
            "radioObservedAt":metadata.observed_at
        }),
        ..Track::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackHistoryEntry {
    pub session_id: String,
    pub played_at: String,
    pub provider: String,
    pub track: Track,
    pub chapter: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrackPlaylistMembership {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrackLibrarySummary {
    pub track_id: String,
    pub play_count: usize,
    /// Up to 100 recent recorded successful playback timestamps, newest first.
    pub played_at: Vec<String>,
    /// True when additional older timestamps exist beyond `playedAt`.
    pub played_at_has_more: bool,
    /// Active named playlist memberships; system Downloads is reported separately.
    pub playlists: Vec<TrackPlaylistMembership>,
    pub in_library: bool,
    /// True only when an attached local audio asset currently exists on disk.
    pub downloaded: bool,
}

fn default_search_limit() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackHistorySearchOptions {
    #[serde(default)]
    pub availability: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub station_id: Option<String>,
    /// RFC3339 instant or YYYY-MM-DD (inclusive start of day).
    #[serde(default)]
    pub from: Option<String>,
    /// RFC3339 instant or YYYY-MM-DD (inclusive end of day).
    #[serde(default)]
    pub to: Option<String>,
    /// `oldest` sorts oldest-first. Any other value sorts newest-first.
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub sort_field: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub min_duration_seconds: Option<f64>,
    #[serde(default)]
    pub max_duration_seconds: Option<f64>,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackHistoryPage {
    pub items: Vec<PlaybackHistoryEntry>,
    pub total: usize,
    pub has_more: bool,
}

/// Persistent playback history, stored independently from the library database.
pub struct PlaybackHistory {
    connection: Mutex<Connection>,
    library_barrier: crate::storage::LibraryWriteBarrier,
}

pub struct HistoryRestoreFreeze {
    history: Arc<PlaybackHistory>,
    armed: bool,
}
impl HistoryRestoreFreeze {
    pub fn keep_frozen(&mut self) {
        self.armed = false;
    }
}
impl Drop for HistoryRestoreFreeze {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(connection) = self.history.connection.lock() {
                let _ = connection.execute_batch("PRAGMA query_only=OFF;");
            }
        }
    }
}

impl PlaybackHistory {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_barrier(path, Arc::new(tokio::sync::RwLock::new(())))
    }

    pub fn open_with_barrier(
        path: &Path,
        library_barrier: crate::storage::LibraryWriteBarrier,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        crate::track_sort_text::register_sqlite(&connection)?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS playback_history (
               session_id TEXT PRIMARY KEY NOT NULL,
               played_at TEXT NOT NULL,
               provider TEXT NOT NULL,
               track_json TEXT NOT NULL,
               chapter_json TEXT
             );
             CREATE INDEX IF NOT EXISTS playback_history_played_at ON playback_history(played_at DESC);
             CREATE INDEX IF NOT EXISTS playback_history_track_id ON playback_history(json_extract(track_json,'$.id'),played_at DESC);",
        ).map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
            library_barrier,
        })
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        Ok(connection)
    }

    pub(crate) fn freeze_for_restore(
        self: &Arc<Self>,
        permit: &crate::storage::RestorePermit,
        expected_change_count: u64,
    ) -> Result<HistoryRestoreFreeze, String> {
        if !permit.belongs_to(&self.library_barrier) {
            return Err("Restore permit belongs to another library profile".into());
        }
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        let current: u64 = connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if current != expected_change_count {
            return Err("Playback history changed after the restore recovery snapshot; inspect and retry restore".into());
        }
        connection
            .execute_batch("PRAGMA query_only=ON;")
            .map_err(|e| e.to_string())?;
        Ok(HistoryRestoreFreeze {
            history: self.clone(),
            armed: true,
        })
    }

    pub(crate) fn change_count_for_restore(
        &self,
        permit: &crate::storage::RestorePermit,
    ) -> Result<u64, String> {
        if !permit.belongs_to(&self.library_barrier) {
            return Err("Restore permit belongs to another library profile".into());
        }
        self.connection
            .lock()
            .map_err(|e| e.to_string())?
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .map_err(|e| e.to_string())
    }

    /// Run a read-only snapshot operation while holding the history
    /// connection mutex. Backup coordination acquires Store first, then this
    /// lock, so both database snapshots represent one quiesced pair.
    pub(crate) fn with_backup_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let connection = self.lock_connection()?;
        operation(&connection)
    }

    /// Record one successful start. Repeated calls for the same playback session (such as resume)
    /// are ignored by the session primary key. Callers should use a new UUID for each new start.
    pub fn record(
        &self,
        session_id: &str,
        provider: &str,
        track: &Track,
        chapter: Option<Value>,
    ) -> Result<bool, String> {
        self.record_at(
            session_id,
            provider,
            track,
            chapter,
            &Utc::now().to_rfc3339(),
        )
    }

    fn record_at(
        &self,
        session_id: &str,
        provider: &str,
        track: &Track,
        chapter: Option<Value>,
        played_at: &str,
    ) -> Result<bool, String> {
        let track_json = serde_json::to_string(track).map_err(|e| e.to_string())?;
        let chapter_json = chapter
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;
        let connection = self.lock_connection()?;
        let changed = connection.execute(
            "INSERT OR IGNORE INTO playback_history(session_id, played_at, provider, track_json, chapter_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, played_at, provider, track_json, chapter_json],
        ).map_err(|e| e.to_string())?;
        Ok(changed == 1)
    }

    /// Record a successful playback after resolving only placeholder fields from the
    /// current library row with the same stable Track ID.
    pub fn record_enriched(
        &self,
        store: &Store,
        session_id: &str,
        provider: &str,
        track: &Track,
        chapter: Option<Value>,
    ) -> Result<bool, String> {
        let track = enrich_track(store, track.clone())?;
        self.record(session_id, provider, &track, chapter)
    }

    /// Record an identified song only when the caller confirms it was heard during
    /// an active station session. `occurrence_key` must stay stable for retries of
    /// one transition and differ when the same song is heard again later.
    pub fn record_radio_occurrence(
        &self,
        store: &Store,
        listening_session_id: &str,
        occurrence_key: &str,
        listened_at: &str,
        station: &RadioStation,
        metadata: &RadioTrackMetadata,
    ) -> Result<bool, String> {
        if listening_session_id.trim().is_empty()
            || occurrence_key.trim().is_empty()
            || station.id.trim().is_empty()
            || metadata.station_id != station.id
            || metadata.title.trim().is_empty()
        {
            return Err(
                "Radio history occurrence is missing its listening identity or song metadata"
                    .into(),
            );
        }
        let listened_at = DateTime::parse_from_rfc3339(listened_at)
            .map_err(|_| "Radio listening time must be RFC3339")?
            .with_timezone(&Utc)
            .to_rfc3339();
        // Capture is optional; an unavailable target must not discard listening history.
        if crate::radio::auto_add_played_radio_track(store, station, metadata).is_err() {
            eprintln!("Radio playlist capture unavailable; listening history continues");
        }
        if !include_radio_songs_in_history(store)? {
            return Ok(false);
        }
        // Never attach the live station stream as a past-song source.
        let track = radio_song_track(station, metadata);
        // A reusable stable track row lets existing play/queue/playlist tools act
        // on the history item; it represents the identified song, never the live
        // stream URL.
        store.cache_track(&track)?;
        let occurrence_hash = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}\n{}\n{}",
                    listening_session_id.trim(),
                    station.id,
                    occurrence_key.trim()
                )
                .as_bytes()
            )
        );
        self.record_at(
            &format!("radio:{occurrence_hash}"),
            "radio",
            &track,
            None,
            &listened_at,
        )
    }

    pub fn list(&self, limit: u32, offset: u32) -> Result<Vec<PlaybackHistoryEntry>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare(
            "SELECT session_id, played_at, provider, track_json, chapter_json FROM playback_history ORDER BY played_at DESC, rowid DESC LIMIT ?1 OFFSET ?2",
        ).map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(params![limit.clamp(1, 500), offset], |row| {
                let track_json: String = row.get(3)?;
                let chapter_json: Option<String> = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    track_json,
                    chapter_json,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|row| {
            let (session_id, played_at, provider, track_json, chapter_json) =
                row.map_err(|e| e.to_string())?;
            Ok(PlaybackHistoryEntry {
                session_id,
                played_at,
                provider,
                track: serde_json::from_str(&track_json).map_err(|e| e.to_string())?,
                chapter: chapter_json
                    .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                    .transpose()?,
            })
        })
        .collect()
    }

    /// Build a read-only summary from persisted successful playback sessions and actual
    /// Library, playlist, and on-disk asset state. Queue duplicates are not inferred.
    pub fn track_library_summary(
        &self,
        store: &Store,
        track_id: &str,
    ) -> Result<TrackLibrarySummary, String> {
        if track_id.trim().is_empty() || track_id.len() > 1024 {
            return Err("Track identity is invalid".into());
        }
        let connection = self.lock_connection()?;
        let play_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM playback_history WHERE json_extract(track_json,'$.id')=?1",
                [track_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT played_at FROM playback_history
                 WHERE json_extract(track_json,'$.id')=?1
                 ORDER BY julianday(played_at) IS NULL ASC,julianday(played_at) DESC,rowid DESC",
            )
            .map_err(|error| error.to_string())?;
        let mut played_at = statement
            .query_map([track_id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .take(101)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let played_at_has_more = played_at.len() > 100;
        played_at.truncate(100);
        drop(statement);
        drop(connection);

        let playlists = store
            .track_collection_memberships(track_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|(id, name)| TrackPlaylistMembership { id, name })
            .collect();
        let in_library = store
            .get_library_track(track_id)
            .map_err(|error| error.to_string())?
            .is_some();
        let downloaded = store
            .list_track_files(track_id)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|asset| Path::new(&asset.path).is_file());
        Ok(TrackLibrarySummary {
            track_id: track_id.to_owned(),
            play_count: usize::try_from(play_count.max(0)).unwrap_or(usize::MAX),
            played_at,
            played_at_has_more,
            playlists,
            in_library,
            downloaded,
        })
    }

    pub fn list_enriched(
        &self,
        store: &Store,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<PlaybackHistoryEntry>, String> {
        self.list(limit, offset)?
            .into_iter()
            .map(|entry| enrich_entry(store, entry))
            .collect()
    }

    pub fn get(
        &self,
        store: &Store,
        session_id: &str,
    ) -> Result<Option<PlaybackHistoryEntry>, String> {
        self.get_enriched(store, session_id)
    }

    pub fn get_enriched(
        &self,
        store: &Store,
        session_id: &str,
    ) -> Result<Option<PlaybackHistoryEntry>, String> {
        let connection = self.lock_connection()?;
        let row = connection
            .query_row(
                "SELECT session_id, played_at, provider, track_json, chapter_json FROM playback_history WHERE session_id=?1",
                [session_id],
                raw_history_row,
            )
            .optional()
            .map_err(|e| e.to_string())?;
        drop(connection);
        row.map(decode_history_row)
            .transpose()?
            .map(|entry| enrich_entry(store, entry))
            .transpose()
    }

    /// Search every row matching provider/date bounds, enriching snapshot placeholders
    /// before applying the literal text query and page window. Rows are streamed in
    /// stable SQL order; only the requested result page is retained in memory.
    pub fn search(
        &self,
        store: &Store,
        options: PlaybackHistorySearchOptions,
    ) -> Result<PlaybackHistoryPage, String> {
        if options
            .availability
            .as_deref()
            .is_some_and(|value| !matches!(value, "all" | "downloaded" | "not-downloaded"))
        {
            return Err("Unsupported playback history downloaded status".into());
        }
        let query = options
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let provider = options
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        if options
            .min_duration_seconds
            .is_some_and(|value| !value.is_finite() || value < 0.0)
            || options
                .max_duration_seconds
                .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            return Err("Duration bounds must be finite and nonnegative".into());
        }
        if options
            .min_duration_seconds
            .zip(options.max_duration_seconds)
            .is_some_and(|(min, max)| min > max)
        {
            return Err("Minimum duration must not exceed maximum duration".into());
        }
        if options.sort_field.as_deref().is_some_and(|field| {
            !matches!(
                field,
                "sourceOrder"
                    | "title"
                    | "artist"
                    | "album"
                    | "duration"
                    | "releaseDate"
                    | "dateAdded"
                    | "bpm"
                    | "key"
                    | "plays"
                    | "views"
            )
        }) {
            return Err("Unsupported playback history sort field".into());
        }
        if options
            .direction
            .as_deref()
            .is_some_and(|direction| !matches!(direction, "asc" | "desc"))
        {
            return Err("Playback history sort direction must be asc or desc".into());
        }
        let mut filters = Vec::new();
        let mut values = Vec::<SqlValue>::new();
        let duration_sql = crate::track_sort_sql::duration_expression("track_json");
        if let Some(provider) = provider {
            if provider.eq_ignore_ascii_case("non-radio") {
                filters.push("lower(provider)<>'radio'".to_owned());
            } else {
                filters.push(format!("lower(provider)=lower(?{})", values.len() + 1));
                values.push(provider.to_owned().into());
            }
        }
        if let Some(station_id) = options
            .station_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            filters.push(format!(
                "json_extract(track_json,'$.provenance.radioStationId')=?{}",
                values.len() + 1
            ));
            values.push(station_id.to_owned().into());
        }
        if let Some(from) = options.from.as_deref() {
            let (value, _) = normalize_date_bound(from, false)?;
            filters.push(format!(
                "julianday(played_at)>=julianday(?{})",
                values.len() + 1
            ));
            values.push(value.into());
        }
        if let Some(to) = options.to.as_deref() {
            let (value, exclusive) = normalize_date_bound(to, true)?;
            let operator = if exclusive { "<" } else { "<=" };
            filters.push(format!(
                "julianday(played_at){operator}julianday(?{})",
                values.len() + 1
            ));
            values.push(value.into());
        }
        if let Some(minimum) = options.min_duration_seconds {
            filters.push(format!("({duration_sql})>=?{}", values.len() + 1));
            values.push(minimum.into());
        }
        if let Some(maximum) = options.max_duration_seconds {
            filters.push(format!("({duration_sql})<=?{}", values.len() + 1));
            values.push(maximum.into());
        }
        let where_sql = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };
        let order_sql = history_order_sql(&options);
        let offset = options.offset as usize;
        let limit = options.limit.clamp(1, 500) as usize;
        let query = query.map(str::to_lowercase);
        // With no text or downloaded-state filter, every SQL-matching history
        // row is part of the result. Count that relation and fetch only the
        // requested page before enriching against the library. The filtered
        // path below must still walk all SQL candidates because those two
        // predicates depend on current hydrated metadata/filesystem state.
        if query.is_none()
            && !matches!(
                options.availability.as_deref(),
                Some("downloaded" | "not-downloaded")
            )
        {
            let (total, raw_entries) = {
                let connection = self.lock_connection()?;
                let count_sql = format!("SELECT COUNT(*) FROM playback_history{where_sql}");
                let total: i64 = connection
                    .query_row(&count_sql, params_from_iter(values.iter()), |row| {
                        row.get(0)
                    })
                    .map_err(|e| e.to_string())?;
                let mut page_values = values.clone();
                let limit_parameter = page_values.len() + 1;
                let offset_parameter = page_values.len() + 2;
                page_values.push(SqlValue::Integer(limit as i64));
                page_values.push(SqlValue::Integer(offset as i64));
                let page_sql = format!(
                    "SELECT session_id, played_at, provider, track_json, chapter_json FROM playback_history{where_sql} ORDER BY {order_sql} LIMIT ?{limit_parameter} OFFSET ?{offset_parameter}"
                );
                let mut statement = connection.prepare(&page_sql).map_err(|e| e.to_string())?;
                let raw_entries = statement
                    .query_map(params_from_iter(page_values.iter()), raw_history_row)
                    .map_err(|e| e.to_string())?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                (
                    usize::try_from(total.max(0)).unwrap_or(usize::MAX),
                    raw_entries,
                )
            };
            let mut items = Vec::with_capacity(raw_entries.len());
            for raw in raw_entries {
                items.push(enrich_entry(store, decode_history_row(raw)?)?);
            }
            return Ok(PlaybackHistoryPage {
                has_more: offset.saturating_add(items.len()) < total,
                items,
                total,
            });
        }
        // Resolve the stable history ordering while holding the history mutex,
        // then release it before enriching against Store. This avoids a
        // History→Store lock inversion with coordinated backups (Store→History).
        let row_ids = {
            let connection = self.lock_connection()?;
            let sql = format!("SELECT rowid FROM playback_history{where_sql} ORDER BY {order_sql}");
            let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
            let mut rows = statement
                .query(params_from_iter(values.iter()))
                .map_err(|e| e.to_string())?;
            let mut row_ids = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                row_ids.push(row.get::<_, i64>(0).map_err(|e| e.to_string())?);
            }
            row_ids
        };
        let mut total = 0usize;
        let mut items = Vec::with_capacity(limit);
        for ids in row_ids.chunks(256) {
            let placeholders = (1..=ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT rowid, session_id, played_at, provider, track_json, chapter_json FROM playback_history WHERE rowid IN ({placeholders})"
            );
            let raw_entries = {
                let connection = self.lock_connection()?;
                let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
                let values = ids
                    .iter()
                    .copied()
                    .map(SqlValue::Integer)
                    .collect::<Vec<_>>();
                let mut rows = statement
                    .query(params_from_iter(values.iter()))
                    .map_err(|e| e.to_string())?;
                let mut entries = HashMap::with_capacity(ids.len());
                while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                    let row_id = row.get::<_, i64>(0).map_err(|e| e.to_string())?;
                    let raw: RawHistoryEntry = (
                        row.get(1).map_err(|e| e.to_string())?,
                        row.get(2).map_err(|e| e.to_string())?,
                        row.get(3).map_err(|e| e.to_string())?,
                        row.get(4).map_err(|e| e.to_string())?,
                        row.get(5).map_err(|e| e.to_string())?,
                    );
                    entries.insert(row_id, decode_history_row(raw)?);
                }
                entries
            };
            // The IN query has no explicit order, so walk the captured rowid
            // sequence to retain the original SQL sort order.
            for row_id in ids {
                let Some(entry) = raw_entries.get(row_id).cloned() else {
                    continue;
                };
                let entry = enrich_entry(store, entry)?;
                let downloaded = entry.track.local_availability == "available";
                if matches!(options.availability.as_deref(), Some("downloaded")) && !downloaded
                    || matches!(options.availability.as_deref(), Some("not-downloaded"))
                        && downloaded
                {
                    continue;
                }
                if query.as_deref().is_some_and(|query| {
                    let text = history_search_text(&entry);
                    !query.split_whitespace().all(|term| text.contains(term))
                }) {
                    continue;
                }
                if total >= offset && items.len() < limit {
                    items.push(entry);
                }
                total += 1;
            }
        }
        Ok(PlaybackHistoryPage {
            has_more: offset.saturating_add(items.len()) < total,
            items,
            total,
        })
    }

    pub fn clear(&self) -> Result<(), String> {
        self.lock_connection()?
            .execute("DELETE FROM playback_history", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn contains_session(&self, session_id: &str) -> Result<bool, String> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT 1 FROM playback_history WHERE session_id = ?1",
                [session_id],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|e| e.to_string())
    }
}

fn history_order_sql(options: &PlaybackHistorySearchOptions) -> String {
    let chronological = if options
        .sort
        .as_deref()
        .is_some_and(|sort| sort.eq_ignore_ascii_case("oldest"))
    {
        "played_at ASC, rowid ASC"
    } else {
        "played_at DESC, rowid DESC"
    };
    let field = options.sort_field.as_deref().unwrap_or("sourceOrder");
    if field == "sourceOrder" {
        if let Some(direction) = options.direction.as_deref() {
            return if direction == "asc" {
                "played_at ASC, rowid ASC".to_owned()
            } else {
                "played_at DESC, rowid DESC".to_owned()
            };
        }
        return chronological.to_owned();
    }
    let Some(expression) = crate::track_sort_sql::expression(field, "track_json") else {
        return chronological.to_owned();
    };
    let direction = if options.direction.as_deref() == Some("desc") {
        "DESC"
    } else {
        "ASC"
    };
    format!("({expression}) IS NULL ASC, {expression} {direction}, played_at DESC, rowid DESC")
}

type RawHistoryEntry = (String, String, String, String, Option<String>);

fn raw_history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawHistoryEntry> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn decode_history_row(
    (session_id, played_at, provider, track_json, chapter_json): RawHistoryEntry,
) -> Result<PlaybackHistoryEntry, String> {
    Ok(PlaybackHistoryEntry {
        session_id,
        played_at,
        provider,
        track: serde_json::from_str(&track_json).map_err(|e| e.to_string())?,
        chapter: chapter_json
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()?,
    })
}

fn enrich_entry(
    store: &Store,
    mut entry: PlaybackHistoryEntry,
) -> Result<PlaybackHistoryEntry, String> {
    entry.track = enrich_track(store, entry.track)?;
    Ok(entry)
}

fn enrich_track(store: &Store, mut historical: Track) -> Result<Track, String> {
    let Some(current) = store.get_track(&historical.id)? else {
        historical.local_availability = if historical
            .local_path
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file())
        {
            "available"
        } else {
            "none"
        }
        .into();
        return Ok(historical);
    };
    if current.id != historical.id {
        return Ok(historical);
    }
    // Availability is current state, not a property of the historical play.
    historical.local_availability = current.local_availability.clone();
    historical.local_path = current.local_path.clone();
    historical.asset_id = current.asset_id.clone();
    let title_is_placeholder = historical.title.trim() == historical.source_url.trim()
        || url::Url::parse(historical.title.trim()).is_ok();
    if title_is_placeholder
        && !current.title.trim().is_empty()
        && current.title.trim() != current.source_url.trim()
        && url::Url::parse(current.title.trim()).is_err()
    {
        historical.title = current.title;
    }
    if matches!(historical.artist.trim(), "" | "Unknown artist")
        && !current.artist.trim().is_empty()
        && !current.artist.eq_ignore_ascii_case("Unknown artist")
    {
        historical.artist = current.artist;
    }
    if historical.album.as_deref().is_none_or(str::is_empty) {
        historical.album = current.album;
    }
    Ok(historical)
}

fn history_search_text(entry: &PlaybackHistoryEntry) -> String {
    let provenance = &entry.track.provenance;
    [
        entry.track.title.as_str(),
        entry.track.artist.as_str(),
        entry.track.album.as_deref().unwrap_or_default(),
        provenance["stationName"].as_str().unwrap_or_default(),
        provenance["station"]["name"].as_str().unwrap_or_default(),
        provenance["stationTitle"].as_str().unwrap_or_default(),
        provenance["radioProvider"].as_str().unwrap_or_default(),
        entry
            .chapter
            .as_ref()
            .and_then(|c| c["title"].as_str())
            .unwrap_or_default(),
    ]
    .join(" ")
    .to_lowercase()
}

fn normalize_date_bound(value: &str, is_to: bool) -> Result<(String, bool), String> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let date = if is_to {
            date.checked_add_days(Days::new(1))
                .ok_or("History date is out of range")?
        } else {
            date
        };
        let instant = date
            .and_hms_opt(0, 0, 0)
            .ok_or("History date is invalid")?
            .and_utc();
        return Ok((instant.to_rfc3339_opts(SecondsFormat::Secs, true), is_to));
    }
    let instant = DateTime::parse_from_rfc3339(value)
        .map_err(|_| "History dates must be RFC3339 or YYYY-MM-DD")?
        .with_timezone(&Utc);
    Ok((instant.to_rfc3339_opts(SecondsFormat::Nanos, true), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PlaybackRange;
    use crate::storage::Store;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    fn track() -> Track {
        Track {
            id: "id-1".into(),
            provider: "catalog".into(),
            provider_id: "p-1".into(),
            title: "Track".into(),
            artist: "Artist".into(),
            ..Track::default()
        }
    }

    #[test]
    fn restore_freeze_rejects_history_writes_and_thaws_on_abort() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("library.sqlite")).unwrap();
        let history = Arc::new(
            PlaybackHistory::open_with_barrier(
                &directory.path().join("history.sqlite"),
                store.library_write_barrier(),
            )
            .unwrap(),
        );
        let changes = history
            .with_backup_connection(|connection| {
                connection
                    .query_row("SELECT total_changes()", [], |row| row.get(0))
                    .map_err(|error| error.to_string())
            })
            .unwrap();
        let restore = store.try_restore_permit().unwrap();
        let frozen = history.freeze_for_restore(&restore, changes).unwrap();
        assert!(history
            .record("session-restore-test", "catalog", &track(), None)
            .is_err());
        drop(frozen);
        drop(restore);
        assert!(history
            .record("session-restore-test", "catalog", &track(), None)
            .unwrap());
    }

    fn imported_url_track(url: &str) -> Track {
        let hash = format!("{:x}", Sha256::digest(url.as_bytes()));
        Track {
            id: format!("import:{hash}"),
            provider: "import".into(),
            provider_id: hash,
            source_url: url.into(),
            artist: "Unknown artist".into(),
            title: url.into(),
            provenance: serde_json::json!({"importFormat":"m3u-url", "identity":"unverified playlist URL"}),
            ..Track::default()
        }
    }

    #[test]
    fn records_once_per_session_and_pages_and_clears() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        assert!(history
            .record(
                "session-1",
                "youtube",
                &track(),
                Some(serde_json::json!({"index": 2}))
            )
            .unwrap());
        assert!(!history
            .record("session-1", "youtube", &track(), None)
            .unwrap());
        assert!(history
            .record("session-2", "local", &track(), None)
            .unwrap());
        assert_eq!(history.list(1, 1).unwrap().len(), 1);
        assert_eq!(history.list(10, 0).unwrap().len(), 2);
        assert_eq!(history.list(10, 0).unwrap()[0].chapter, None);
        history.clear().unwrap();
        assert!(history.list(10, 0).unwrap().is_empty());
    }

    #[test]
    fn history_survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        PlaybackHistory::open(&path)
            .unwrap()
            .record("s", "local", &track(), None)
            .unwrap();
        assert_eq!(
            PlaybackHistory::open(&path)
                .unwrap()
                .list(10, 0)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn track_library_summary_counts_successful_sessions_not_playlist_occurrences() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let history = PlaybackHistory::open(&dir.path().join("history.sqlite")).unwrap();
        let track = Track {
            id: "catalog:summary-track".into(),
            provider: "catalog".into(),
            provider_id: "summary-track".into(),
            title: "Summary fixture".into(),
            artist: "Fixture artist".into(),
            ..Track::default()
        };
        let other = Track {
            id: "catalog:other-summary-track".into(),
            provider_id: "other-summary-track".into(),
            title: "Other fixture".into(),
            ..track.clone()
        };

        // A resume/retry in the same successful session is idempotent; a later
        // session is a separate play. The unrelated recording is not counted.
        assert!(history
            .record_at(
                "same-session",
                "local",
                &track,
                None,
                "2026-10-25T08:00:00Z"
            )
            .unwrap());
        assert!(!history
            .record_at(
                "same-session",
                "local",
                &track,
                None,
                "2026-10-25T08:05:00Z"
            )
            .unwrap());
        assert!(history
            .record_at(
                "later-session",
                "local",
                &track,
                None,
                "2026-10-27T09:00:00Z"
            )
            .unwrap());
        assert!(history
            .record_at(
                "other-session",
                "local",
                &other,
                None,
                "2026-10-27T10:00:00Z"
            )
            .unwrap());
        for index in 0..105 {
            let timestamp = (DateTime::parse_from_rfc3339("2026-10-27T08:00:00Z").unwrap()
                + chrono::Duration::seconds(index))
            .to_rfc3339();
            assert!(history
                .record_at(
                    &format!("extra-session-{index}"),
                    "local",
                    &track,
                    None,
                    &timestamp,
                )
                .unwrap());
        }

        let collection = store.create_collection("Summary fixture playlist").unwrap();
        store
            .append_playlist_entries(&collection.id, &[track.clone(), track.clone()])
            .unwrap();
        let audio_path = dir.path().join("summary-fixture.flac");
        std::fs::write(&audio_path, b"fixture audio").unwrap();
        store
            .save_asset(&crate::models::LocalAsset {
                id: "summary-asset".into(),
                recording_id: String::new(),
                path: audio_path.to_string_lossy().into_owned(),
                hash: "fixture-hash".into(),
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
                track: track.clone(),
            })
            .unwrap();

        let summary = history.track_library_summary(&store, &track.id).unwrap();
        assert_eq!(summary.track_id, track.id);
        assert_eq!(summary.play_count, 107);
        assert_eq!(summary.played_at.len(), 100);
        assert_eq!(summary.played_at[0], "2026-10-27T09:00:00Z");
        assert!(summary.played_at_has_more);
        assert_eq!(
            summary.playlists,
            vec![TrackPlaylistMembership {
                id: collection.id,
                name: "Summary fixture playlist".into(),
            }]
        );
        assert!(summary.in_library);
        assert!(summary.downloaded);

        // Missing files are not reported as downloaded even when an asset row
        // still exists.
        std::fs::remove_file(audio_path).unwrap();
        assert!(
            !history
                .track_library_summary(&store, &track.id)
                .unwrap()
                .downloaded
        );
    }

    #[test]
    fn archived_url_snapshot_enriches_without_changing_history_identity_or_chapter() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let url = "https://www.youtube.com/watch?v=History1234";
        let snapshot = imported_url_track(url);
        let chapter = serde_json::json!({"id":"chapter-3", "title":"Act III", "start":42.0});
        history
            .record(
                "history-session",
                "youtube",
                &snapshot,
                Some(chapter.clone()),
            )
            .unwrap();
        let mut repaired = snapshot.clone();
        repaired.artist = "Mercedes Sosa".into();
        repaired.title = "Alfonsina y el Mar".into();
        store.save_track(&repaired).unwrap();

        let enriched = history
            .get_enriched(&store, "history-session")
            .unwrap()
            .unwrap();
        assert_eq!(enriched.session_id, "history-session");
        assert_eq!(enriched.provider, "youtube");
        assert_eq!(enriched.track.id, snapshot.id);
        assert_eq!(enriched.track.source_url, url);
        assert_eq!(enriched.track.artist, "Mercedes Sosa");
        assert_eq!(enriched.track.title, "Alfonsina y el Mar");
        assert_eq!(enriched.chapter, Some(chapter));
        assert_eq!(history.list(10, 0).unwrap()[0].track.title, url);
    }

    #[test]
    fn history_search_filters_after_enrichment_and_before_pagination() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let url = "https://www.youtube.com/watch?v=History1234";
        let snapshot = imported_url_track(url);
        let mut repaired = snapshot.clone();
        repaired.artist = "Bebo Valdés".into();
        repaired.title = "Lágrimas Negras".into();
        store.save_track(&repaired).unwrap();
        history
            .record("target", "youtube", &snapshot, None)
            .unwrap();
        // These newer nonmatching items precede the target in the unfiltered history.
        history.record("newer-1", "local", &track(), None).unwrap();
        history.record("newer-2", "local", &track(), None).unwrap();

        let page = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    query: Some("valdés lágrimas".into()),
                    provider: Some("youtube".into()),
                    limit: 1,
                    ..PlaybackHistorySearchOptions::default()
                },
            )
            .unwrap();

        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert!(!page.has_more);
        assert_eq!(page.items[0].session_id, "target");
        assert_eq!(page.items[0].track.title, "Lágrimas Negras");
    }

    #[test]
    fn unfiltered_history_pages_preserve_total_and_stable_tie_order() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let played_at = "2026-10-27T12:00:00Z";
        for index in 0..4 {
            let mut value = track();
            value.id = format!("history-page-{index}");
            value.title = format!("History page {index}");
            history
                .record_at(
                    &format!("history-page-session-{index}"),
                    "local",
                    &value,
                    None,
                    played_at,
                )
                .unwrap();
        }

        let first = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    offset: 1,
                    limit: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(first.total, 4);
        assert!(first.has_more);
        assert_eq!(
            first
                .items
                .iter()
                .map(|entry| entry.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["history-page-session-2", "history-page-session-1"]
        );

        let last = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    offset: 3,
                    limit: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(last.total, 4);
        assert!(!last.has_more);
        assert_eq!(last.items.len(), 1);
        assert_eq!(last.items[0].session_id, "history-page-session-0");
    }

    #[test]
    fn history_download_filter_uses_current_files_before_counting_and_paging() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        for (index, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
            let path = dir.path().join(format!("{name}.flac"));
            if index % 2 == 1 {
                std::fs::write(&path, b"local fixture").unwrap();
            }
            let mut value = track();
            value.id = name.into();
            value.title = name.into();
            value.local_path = Some(path.to_string_lossy().into_owned());
            // Old snapshots may claim available even after a file is removed.
            value.local_availability = "available".into();
            history.record(name, "local", &value, None).unwrap();
        }
        let page = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    availability: Some("downloaded".into()),
                    sort_field: Some("title".into()),
                    direction: Some("asc".into()),
                    offset: 1,
                    limit: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 2);
        assert!(!page.has_more);
        assert_eq!(page.items[0].track.title, "D");
        let missing = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    availability: Some("not-downloaded".into()),
                    limit: 100,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(missing.total, 2);
        assert!(missing
            .items
            .iter()
            .all(|entry| entry.track.local_availability != "available"));
        assert!(history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    availability: Some("invalid".into()),
                    ..Default::default()
                }
            )
            .is_err());
    }

    #[test]
    fn history_duration_bounds_and_sort_apply_before_page_window_and_use_chapter_length() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let mut long = track();
        long.id = "long".into();
        long.title = "Zulu".into();
        long.duration_seconds = Some(180.0);
        history.record("long", "local", &long, None).unwrap();

        let mut short = track();
        short.id = "short".into();
        short.title = "Alpha".into();
        short.duration_seconds = Some(120.0);
        history.record("short", "local", &short, None).unwrap();

        let mut chapter = track();
        chapter.id = "chapter".into();
        chapter.title = "Beta".into();
        chapter.duration_seconds = Some(360.0);
        chapter.playback_range = Some(PlaybackRange {
            parent_track_id: "mix".into(),
            chapter_id: "beta".into(),
            source_url: "https://example.test/mix".into(),
            start_time: 30.0,
            end_time: 60.0,
            parent_title: "Long mix".into(),
            parent_duration_seconds: Some(600.0),
        });
        history
            .record("chapter", "youtube", &chapter, None)
            .unwrap();

        let page = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    min_duration_seconds: Some(60.0),
                    max_duration_seconds: Some(240.0),
                    sort_field: Some("title".into()),
                    direction: Some("asc".into()),
                    limit: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 2);
        assert!(page.has_more);
        assert_eq!(page.items[0].session_id, "short");

        let next = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    min_duration_seconds: Some(60.0),
                    max_duration_seconds: Some(240.0),
                    sort_field: Some("title".into()),
                    direction: Some("asc".into()),
                    limit: 1,
                    offset: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(next.items[0].session_id, "long");

        let descending = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    sort_field: Some("title".into()),
                    direction: Some("desc".into()),
                    limit: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(descending.items[0].session_id, "long");
    }

    #[test]
    fn new_history_records_are_enriched_before_persistence() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let url = "https://www.youtube.com/watch?v=History1234";
        let snapshot = imported_url_track(url);
        let mut repaired = snapshot.clone();
        repaired.artist = "Cesária Évora".into();
        repaired.title = "Sodade".into();
        store.save_track(&repaired).unwrap();

        assert!(history
            .record_enriched(&store, "new-session", "youtube", &snapshot, None)
            .unwrap());
        let saved = history.list(10, 0).unwrap().remove(0);
        assert_eq!(saved.track.id, snapshot.id);
        assert_eq!(saved.track.artist, "Cesária Évora");
        assert_eq!(saved.track.title, "Sodade");
    }

    #[test]
    fn recording_a_cached_search_result_keeps_it_out_of_the_library() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let mut search_result = track();
        search_result.id = "youtube:search-result".into();
        search_result.provider = "youtube".into();
        search_result.provider_id = "search-result".into();

        // This is the cache-then-history sequence used by record_playback for
        // a result that has not otherwise been persisted.
        assert!(store.get_track(&search_result.id).unwrap().is_none());
        store.cache_track(&search_result).unwrap();
        assert!(history
            .record_enriched(
                &store,
                "search-result-session",
                "youtube",
                &search_result,
                None,
            )
            .unwrap());

        assert!(store.get_track(&search_result.id).unwrap().is_some());
        assert_eq!(history.list(10, 0).unwrap().len(), 1);
        assert!(store.list_tracks("library", None).unwrap().is_empty());
        assert_eq!(
            store
                .search_library_tracks(&["Search".into()], 10, 0)
                .unwrap()
                .1,
            0
        );
    }

    #[test]
    fn opted_in_playlist_capture_is_independent_of_history_and_target_failures() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let station = crate::radio::get_radio_stations_saved(&store)
            .unwrap()
            .remove(0);
        let metadata: RadioTrackMetadata = serde_json::from_value(serde_json::json!({
            "stationId":station.id,"artist":"Fixture performer","title":"Fixture song",
            "sourceUrl":"","metadataConfidence":"station metadata"
        }))
        .unwrap();
        let capture =
            crate::radio::set_radio_playlist_option(&store, &station.id, true, None).unwrap();
        set_include_radio_songs_in_history(&store, false).unwrap();
        assert!(!history
            .record_radio_occurrence(
                &store,
                "session",
                "one",
                "2026-10-21T06:00:00Z",
                &station,
                &metadata
            )
            .unwrap());
        assert!(history.list(10, 0).unwrap().is_empty());
        let target = capture.collection_id.unwrap();
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .into_iter()
                .find(|c| c.id == target)
                .unwrap()
                .track_count,
            1
        );
        store.delete_collection(&target).unwrap();
        set_include_radio_songs_in_history(&store, true).unwrap();
        assert!(history
            .record_radio_occurrence(
                &store,
                "session",
                "two",
                "2026-10-21T06:01:00Z",
                &station,
                &metadata
            )
            .unwrap());
        assert_eq!(history.list(10, 0).unwrap().len(), 1);
    }

    #[test]
    fn radio_song_history_obeys_default_on_setting_and_dedupes_per_occurrence() {
        let dir = tempdir().unwrap();
        let history = PlaybackHistory::open(&dir.path().join("playback-history.sqlite")).unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        assert!(include_radio_songs_in_history(&store).unwrap());
        let station = RadioStation {
            id: "station-1".into(),
            name: "Example FM".into(),
            stream_url: "https://radio.example/live".into(),
            source_url: "https://radio.example/about".into(),
            provider: Some("internet_radio".into()),
            country: None,
            tags: None,
            homepage_url: Some("https://radio.example/".into()),
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: true,
        };
        let metadata = RadioTrackMetadata {
            identifiers: None,
            station_id: station.id.clone(),
            artist: "Cesária Évora".into(),
            title: "Sodade".into(),
            album: Some("Miss Perfumado".into()),
            year: Some("1992".into()),
            artwork_url: Some("https://is1-ssl.mzstatic.com/recognized-cover.jpg".into()),
            source_url: station.stream_url.clone(),
            observed_at: Some("2026-10-14T10:00:00Z".into()),
            metadata_confidence: "station-reported metadata".into(),
        };
        let listened_at = "2026-10-14T10:03:00Z";

        set_include_radio_songs_in_history(&store, false).unwrap();
        assert!(!history
            .record_radio_occurrence(
                &store,
                "listen-session",
                "transition-1",
                listened_at,
                &station,
                &metadata,
            )
            .unwrap());
        assert!(history.list(10, 0).unwrap().is_empty());
        set_include_radio_songs_in_history(&store, true).unwrap();

        assert!(history
            .record_radio_occurrence(
                &store,
                "listen-session",
                "transition-1",
                listened_at,
                &station,
                &metadata,
            )
            .unwrap());
        assert!(!history
            .record_radio_occurrence(
                &store,
                "listen-session",
                "transition-1",
                listened_at,
                &station,
                &metadata,
            )
            .unwrap());
        assert!(history
            .record_radio_occurrence(
                &store,
                "listen-session",
                "transition-2",
                "2026-10-14T10:07:00Z",
                &station,
                &metadata,
            )
            .unwrap());

        let items = history.list(10, 0).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].provider, "radio");
        assert_eq!(items[0].played_at, "2026-10-14T10:07:00+00:00");
        assert_eq!(items[0].track.id, items[1].track.id);
        assert_eq!(items[0].track.source_url, "");
        assert_eq!(items[0].track.artwork_url, metadata.artwork_url);
        assert_eq!(items[0].track.provenance["radioStationName"], "Example FM");
        assert_eq!(
            items[0].track.provenance["radioObservedAt"],
            "2026-10-14T10:00:00Z"
        );
        assert_eq!(
            items[0].track.provenance["radioStationUrl"],
            "https://radio.example/"
        );
        let station_page = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    station_id: Some("station-1".into()),
                    ..PlaybackHistorySearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(station_page.total, 2);
        let non_radio_page = history
            .search(
                &store,
                PlaybackHistorySearchOptions {
                    provider: Some("non-radio".into()),
                    ..PlaybackHistorySearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(non_radio_page.total, 0);
    }
}
