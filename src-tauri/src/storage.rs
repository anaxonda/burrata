use crate::browser_bridge::{
    snapshot_complete, snapshot_warnings, BrowserImportResult, BrowserSnapshot,
};
use crate::models::*;
use icu_normalizer::ComposingNormalizerBorrowed;
use rusqlite::{functions::FunctionFlags, params, Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
    time::Instant,
};

/// A dedicated retained cache for stable source locators, never stream URLs.
pub(crate) const PLAYBACK_LOCATOR_CACHE_NAMESPACE: &str = "playback.matching-locators.v2";

#[derive(Debug, Clone, serde::Serialize)]
pub struct TrackPrefixEntry {
    #[serde(rename = "listingId")]
    pub listing_id: String,
    #[serde(rename = "playlistPosition")]
    pub playlist_position: Option<i64>,
}

/// A reviewable Library entry with no independent favorite, collection, or
/// local-file reference. This classification does not assert why it was added.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsortedLibraryEntry {
    pub id: String,
    pub provider: String,
    pub provider_id: String,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub selection_token: String,
    pub classification: &'static str,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsortedLibraryPreview {
    pub entries: Vec<UnsortedLibraryEntry>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntrySelection {
    pub id: String,
    pub selection_token: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCleanupSkippedEntry {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsortedLibraryRemovalResult {
    pub removed_ids: Vec<String>,
    pub skipped: Vec<LibraryCleanupSkippedEntry>,
}

/// Explicitly distinguishes an incomplete global scan from a ready map.
/// The frontend keeps sort choices available while this is pending.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", content = "fields", rename_all = "camelCase")]
pub enum SortAvailabilityState {
    Pending,
    Ready(std::collections::BTreeMap<String, bool>),
}
struct TrackPageRow {
    listing_id: String,
    playlist_position: Option<i64>,
    track: Option<Track>,
}

pub struct Store {
    connection: Mutex<Connection>,
    connection_metrics_enabled: bool,
    connection_wait_metrics: ConnectionWaitMetrics,
}

#[derive(Default)]
struct ConnectionWaitMetrics {
    lock_count: AtomicU64,
    total_wait_ns: AtomicU64,
    max_wait_ns: AtomicU64,
}
impl ConnectionWaitMetrics {
    fn record(&self, wait_ns: u64) {
        self.lock_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .ok();
        self.total_wait_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(wait_ns))
            })
            .ok();
        self.max_wait_ns.fetch_max(wait_ns, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPlaybackSource {
    pub provider: String,
    pub provider_id: String,
    pub permanent_url: String,
    pub verification_proof: Option<serde_json::Value>,
}

/// Cached provider evidence scoped to one requested recording identity and one
/// stable provider object. Callers must run the current evaluator again before
/// using the evidence to accept a source.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatchingEvidenceRecord {
    pub schema_version: u32,
    pub evaluator_version: String,
    pub requested_track_id: String,
    pub requested_fingerprint: String,
    pub source_provider: String,
    pub source_id: String,
    pub policy: crate::recording_match::MatchLevel,
    pub relationship: crate::recording_match::MatchRelationship,
    pub evidence: serde_json::Value,
}
#[derive(Debug, Clone)]
pub struct AnalysisQueueAsset {
    pub rowid: i64,
    pub id: String,
    pub path: String,
    pub hash: String,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssetArtistSummary {
    pub id: String,
    pub artist: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFilesForRecording {
    pub track_id: String,
    pub files: Vec<LocalAsset>,
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
fn normalize_artist_name(value: &str) -> String {
    let normalized = ComposingNormalizerBorrowed::new_nfkc().normalize(value);
    let mut collapsed = String::with_capacity(normalized.len());
    let mut pending_space = false;
    for character in normalized.chars() {
        if is_javascript_whitespace(character) {
            pending_space = !collapsed.is_empty();
        } else {
            if pending_space {
                collapsed.push(' ');
                pending_space = false;
            }
            collapsed.push(character);
        }
    }
    collapsed.to_lowercase()
}

// ECMAScript `\s` set used by the frontend's normalizeLocalArtistName regex.
// Rust `char::is_whitespace` differs here (notably FEFF and U+0085).
fn is_javascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}
fn materialize_artist_asset_candidates(
    conn: &Connection,
    requested: &std::collections::HashSet<String>,
    playback_sources: Option<&[Track]>,
) -> Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS artist_page_names(name TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS artist_page_candidate_recordings(id TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS artist_page_source_artists(value TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS artist_page_source_titles(value TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS artist_page_source_versions(value TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS artist_page_effective_assets(
             id TEXT PRIMARY KEY, artist TEXT NOT NULL, artist_key TEXT NOT NULL,
             sort_order INTEGER NOT NULL, track_json TEXT NOT NULL, asset_json TEXT NOT NULL
         );
         DELETE FROM artist_page_names;
         DELETE FROM artist_page_candidate_recordings;
         DELETE FROM artist_page_source_artists;
         DELETE FROM artist_page_source_titles;
         DELETE FROM artist_page_source_versions;
         DELETE FROM artist_page_effective_assets;",
    )
    .map_err(err)?;
    {
        let mut insert = conn
            .prepare("INSERT OR IGNORE INTO artist_page_names(name) VALUES(?1)")
            .map_err(err)?;
        for name in requested {
            insert.execute([name]).map_err(err)?;
        }
    }
    if let Some(sources) = playback_sources {
        for (table, values) in [
            (
                "artist_page_source_artists",
                sources
                    .iter()
                    .map(|track| normalize_artist_name(&track.artist))
                    .collect::<std::collections::HashSet<_>>(),
            ),
            (
                "artist_page_source_titles",
                sources
                    .iter()
                    .map(|track| normalize_artist_name(&track.title))
                    .collect::<std::collections::HashSet<_>>(),
            ),
            (
                "artist_page_source_versions",
                sources
                    .iter()
                    .map(|track| normalize_artist_name(track.version.as_deref().unwrap_or("")))
                    .collect::<std::collections::HashSet<_>>(),
            ),
        ] {
            let sql = format!("INSERT OR IGNORE INTO {table}(value) VALUES(?1)");
            let mut insert = conn.prepare(&sql).map_err(err)?;
            for value in values {
                insert.execute([value]).map_err(err)?;
            }
        }
    }

    let mut candidate_sql = String::from(
        "INSERT OR IGNORE INTO artist_page_candidate_recordings(id)
         SELECT r.id FROM recordings r
         WHERE EXISTS(SELECT 1 FROM local_assets a WHERE a.recording_id=r.id)
           AND (
             artist_match_key(json_extract(r.manual_metadata,'$.artist')) IN (SELECT name FROM artist_page_names)
             OR EXISTS(
               SELECT 1 FROM catalog_listings l WHERE l.recording_id=r.id
                 AND artist_match_key(json_extract(l.metadata,'$.artist')) IN (SELECT name FROM artist_page_names)
             )
             OR EXISTS(
               SELECT 1 FROM local_assets a WHERE a.recording_id=r.id
                 AND artist_match_key(json_extract(a.payload,'$.track.artist')) IN (SELECT name FROM artist_page_names)
             )
           )",
    );
    if playback_sources.is_some() {
        for (field, values_table) in [
            ("artist", "artist_page_source_artists"),
            ("title", "artist_page_source_titles"),
            ("version", "artist_page_source_versions"),
        ] {
            candidate_sql.push_str(&format!(
                " AND (
                    COALESCE(artist_match_key(json_extract(r.manual_metadata,'$.{field}')),'') IN (SELECT value FROM {values_table})
                    OR EXISTS(SELECT 1 FROM catalog_listings l WHERE l.recording_id=r.id AND COALESCE(artist_match_key(json_extract(l.metadata,'$.{field}')),'') IN (SELECT value FROM {values_table}))
                    OR EXISTS(SELECT 1 FROM local_assets a WHERE a.recording_id=r.id AND COALESCE(artist_match_key(json_extract(a.payload,'$.track.{field}')),'') IN (SELECT value FROM {values_table}))
                 )"
            ));
        }
    }
    conn.execute(&candidate_sql, []).map_err(err)?;

    let candidate_payloads = {
        let mut stmt = conn
            .prepare(
                "SELECT a.id,a.payload,a.rowid FROM local_assets a
                 JOIN artist_page_candidate_recordings c ON c.id=a.recording_id
                 ORDER BY a.rowid DESC",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(err)?;
        let collected = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        collected
    };
    {
        let mut insert = conn
            .prepare(
                "INSERT OR IGNORE INTO artist_page_effective_assets(
                   id,artist,artist_key,sort_order,track_json,asset_json
                 ) VALUES(?1,?2,?3,?4,?5,?6)",
            )
            .map_err(err)?;
        for (id, payload, sort_order) in candidate_payloads {
            let asset = hydrate_asset(conn, decode(payload)?)?;
            let artist = asset.track.artist.trim().to_owned();
            let artist_key = normalize_artist_name(&artist);
            if requested.contains(&artist_key) {
                insert
                    .execute(params![
                        id,
                        artist,
                        artist_key,
                        sort_order,
                        encode(&asset.track)?,
                        encode(&asset)?,
                    ])
                    .map_err(err)?;
            }
        }
    }
    Ok(())
}
fn unique_artist_playback_match(source: &Track, assets: &[LocalAsset]) -> Option<usize> {
    if source.local_availability == "available"
        && source
            .local_path
            .as_deref()
            .is_some_and(|path| !path.is_empty())
    {
        return None;
    }
    let source_artist = normalize_artist_name(&source.artist);
    let source_title = normalize_artist_name(&source.title);
    let source_version = normalize_artist_name(source.version.as_deref().unwrap_or(""));
    let source_duration = source.duration_seconds.filter(|duration| *duration != 0.0);
    let mut matched = None;
    for (index, asset) in assets.iter().enumerate() {
        let track = &asset.track;
        if track.local_availability != "available" || track.local_path.is_none() {
            continue;
        }
        if normalize_artist_name(&track.artist) != source_artist
            || normalize_artist_name(&track.title) != source_title
            || normalize_artist_name(track.version.as_deref().unwrap_or("")) != source_version
        {
            continue;
        }
        if let (Some(expected), Some(actual)) = (
            source_duration,
            track.duration_seconds.filter(|duration| *duration != 0.0),
        ) {
            if (expected - actual).abs() > 3.0 {
                continue;
            }
        }
        if matched.is_some() {
            return None;
        }
        matched = Some(index);
    }
    matched
}
fn restore_replacement_backup(backup: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::hard_link(backup, target)?;
    std::fs::remove_file(backup)
}

fn is_permanent_playback_url(provider: &str, value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match provider {
        "youtube" => {
            matches!(url.host_str(), Some("youtube.com" | "www.youtube.com"))
                && url.path() == "/watch"
                && url.query_pairs().count() == 1
                && url.query_pairs().any(|(key, value)| {
                    key == "v"
                        && value.len() == 11
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                })
        }
        "soundcloud" => {
            matches!(
                url.host_str(),
                Some("soundcloud.com" | "www.soundcloud.com")
            ) && url.query().is_none()
                && url.path_segments().is_some_and(|mut segments| {
                    let parts: Vec<_> = segments.by_ref().collect();
                    parts.len() == 2
                        && parts.iter().all(|part| !part.is_empty())
                        && !matches!(
                            parts[0],
                            "discover" | "search" | "charts" | "you" | "stream"
                        )
                        && parts[1] != "sets"
                })
        }
        _ => false,
    }
}

fn stable_playback_source_matches(provider: &str, provider_id: &str, value: &str) -> bool {
    if !stable_provider_id(provider, provider_id) || !is_permanent_playback_url(provider, value) {
        return false;
    }
    if provider == "youtube" {
        return url::Url::parse(value).ok().is_some_and(|url| {
            url.query_pairs()
                .any(|(key, value)| key == "v" && value == provider_id)
        });
    }
    true
}

fn stable_provider_id(provider: &str, provider_id: &str) -> bool {
    match provider {
        "youtube" => {
            provider_id.len() == 11
                && provider_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }
        "soundcloud" => {
            !provider_id.is_empty()
                && provider_id.len() <= 32
                && provider_id.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

impl Store {
    /// Recompute a bounded batch of cached sort-field masks for listings that
    /// have no local assets or exactly one local asset. Asset-free rows cache
    /// the exact raw-plus-manual JSON used by SQL sorting; single-asset rows
    /// cache their hydrated sort projection, which remains stable when that
    /// file appears or disappears because hydration selects the same sole asset.
    pub fn backfill_sort_availability_batch(&self, limit: usize) -> Result<usize> {
        let limit = limit.clamp(1, 500) as i64;
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        let found = [false; 10];
        let paths = crate::track_sort_sql::availability_json_paths(&found);
        let metadata = "CASE WHEN r.manual_metadata='{}' THEN l.metadata ELSE json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')) END";
        let projection =
            crate::track_sort_sql::availability_projection_expression(metadata, &paths);
        let effective_metadata = "CASE WHEN r.manual_metadata='{}' THEN l.metadata ELSE json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')) END";
        let compact_projection = format!("CASE WHEN COUNT(a.id)=0 THEN {projection} ELSE '[]' END");
        let stale_listing = "(l.sort_availability_mask IS NULL OR l.sort_availability_revision IS NOT ?1
                              OR l.sort_metadata_projection IS NULL OR l.sort_projection_revision IS NOT ?1)";
        let sql = format!(
            "SELECT l.id,l.metadata,COUNT(a.id),{compact_projection},
                    CASE WHEN COUNT(a.id)=0 THEN {effective_metadata} END
             FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id
             LEFT JOIN local_assets a ON a.recording_id=l.recording_id
             WHERE {stale_listing}
             GROUP BY l.id
             HAVING COUNT(a.id) IN (0,1)
             ORDER BY l.rowid LIMIT ?2"
        );
        let rows = {
            let mut stmt = tx.prepare(&sql).map_err(err)?;
            let mapped = stmt
                .query_map(
                    params![crate::track_sort_sql::AVAILABILITY_MASK_REVISION, limit],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )
                .map_err(err)?;
            mapped
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
        };
        let mut updated = 0;
        for (id, raw, asset_count, compact, effective_metadata) in rows {
            let (fields, cached_projection) = if asset_count == 0 {
                let projected: Vec<serde_json::Value> =
                    serde_json::from_str(&compact).map_err(err)?;
                let mut fields = [false; 10];
                crate::track_sort_sql::extend_available_fields_from_projection(
                    &projected,
                    &paths,
                    &mut fields,
                );
                (
                    fields,
                    Some(
                        effective_metadata
                            .ok_or("Missing effective metadata for asset-free listing")?,
                    ),
                )
            } else if asset_count == 1 {
                let decoded: Track = decode(raw)?;
                let effective = hydrate(&tx, decoded)?;
                let projection = sort_metadata_projection(&effective);
                let fields = crate::track_sort_sql::available_fields(&projection);
                (
                    fields,
                    Some(serde_json::to_string(&projection).map_err(err)?),
                )
            } else {
                continue;
            };
            let projection_revision = cached_projection
                .as_ref()
                .map(|_| crate::track_sort_sql::AVAILABILITY_MASK_REVISION);
            tx.execute(
                "UPDATE catalog_listings
                 SET sort_availability_mask=?1,sort_availability_revision=?2,
                     sort_metadata_projection=?3,sort_projection_revision=?4
                 WHERE id=?5",
                params![
                    crate::track_sort_sql::pack_available_fields(&fields),
                    crate::track_sort_sql::AVAILABILITY_MASK_REVISION,
                    cached_projection,
                    projection_revision,
                    id,
                ],
            )
            .map_err(err)?;
            updated += 1;
        }
        tx.commit().map_err(err)?;
        Ok(updated)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(err)?;
        crate::track_sort_text::register_sqlite(&conn)?;
        crate::track_sort_sql::register_availability_aggregate(&conn).map_err(err)?;
        conn.create_scalar_function(
            "artist_match_key",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let value: rusqlite::types::Value = context.get(0)?;
                Ok(match value {
                    rusqlite::types::Value::Text(value) => Some(normalize_artist_name(&value)),
                    _ => None,
                })
            },
        )
        .map_err(err)?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;",
        )
        .map_err(err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version > 10 {
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
        if version < 7 {
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE verified_playback_sources(
                    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
                    fingerprint TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    provider_id TEXT NOT NULL,
                    permanent_url TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(recording_id,fingerprint,provider));
                CREATE INDEX verified_playback_sources_expiry ON verified_playback_sources(expires_at);
                PRAGMA user_version=7; COMMIT;").map_err(err)?;
        }
        if version < 8 {
            let has_membership_column: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_listings') WHERE name='in_library')",
                    [],
                    |row| row.get(0),
                )
                .map_err(err)?;
            if !has_membership_column {
                conn.execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE catalog_listings ADD COLUMN in_library INTEGER NOT NULL DEFAULT 1 CHECK(in_library IN (0,1));
                     PRAGMA user_version=8;
                     COMMIT;",
                )
                .map_err(err)?;
            } else {
                conn.execute_batch("PRAGMA user_version=8;").map_err(err)?;
            }
        }
        if version < 9 {
            let column_exists = |name: &str| -> Result<bool> {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_listings') WHERE name=?1)",
                    [name],
                    |row| row.get(0),
                )
                .map_err(err)
            };
            let has_mask = column_exists("sort_availability_mask")?;
            let has_revision = column_exists("sort_availability_revision")?;
            let has_projection = column_exists("sort_metadata_projection")?;
            let has_projection_revision = column_exists("sort_projection_revision")?;
            let mut migration = String::from("BEGIN IMMEDIATE;");
            if !has_mask {
                migration.push_str(
                    "ALTER TABLE catalog_listings ADD COLUMN sort_availability_mask INTEGER;",
                );
            }
            if !has_revision {
                migration.push_str(
                    "ALTER TABLE catalog_listings ADD COLUMN sort_availability_revision INTEGER;",
                );
            }
            if !has_projection {
                migration.push_str(
                    "ALTER TABLE catalog_listings ADD COLUMN sort_metadata_projection TEXT;",
                );
            }
            if !has_projection_revision {
                migration.push_str(
                    "ALTER TABLE catalog_listings ADD COLUMN sort_projection_revision INTEGER;",
                );
            }
            migration.push_str("PRAGMA user_version=9; COMMIT;");
            conn.execute_batch(&migration).map_err(err)?;
        }
        if version < 10 {
            let has_proof: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('verified_playback_sources') WHERE name='verification_proof')",
                    [],
                    |row| row.get(0),
                )
                .map_err(err)?;
            if !has_proof {
                conn.execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE verified_playback_sources ADD COLUMN verification_proof TEXT;
                     PRAGMA user_version=10;
                     COMMIT;",
                )
                .map_err(err)?;
            } else {
                conn.execute_batch("PRAGMA user_version=10;").map_err(err)?;
            }
        }
        // Version 9 was not shipped; keep this guard so prototype v9 stores made
        // before the projection column was introduced can still be opened.
        for (name, sql) in [
            (
                "sort_metadata_projection",
                "ALTER TABLE catalog_listings ADD COLUMN sort_metadata_projection TEXT;",
            ),
            (
                "sort_projection_revision",
                "ALTER TABLE catalog_listings ADD COLUMN sort_projection_revision INTEGER;",
            ),
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_listings') WHERE name=?1)",
                    [name],
                    |row| row.get(0),
                )
                .map_err(err)?;
            if !exists {
                conn.execute_batch(sql).map_err(err)?;
            }
        }
        // Version 9 has not shipped yet. Recreate these triggers on every open so
        // stores created by earlier v9 builds receive the no-op guards as well.
        conn.execute_batch(
            "BEGIN IMMEDIATE; DROP TRIGGER IF EXISTS sort_availability_listing_insert; DROP TRIGGER IF EXISTS sort_availability_listing_update; DROP TRIGGER IF EXISTS sort_availability_manual_update; DROP TRIGGER IF EXISTS sort_availability_asset_insert; DROP TRIGGER IF EXISTS sort_availability_asset_update; DROP TRIGGER IF EXISTS sort_availability_asset_delete; DROP TRIGGER IF EXISTS sort_availability_analysis_insert; DROP TRIGGER IF EXISTS sort_availability_analysis_update; DROP TRIGGER IF EXISTS sort_availability_analysis_delete; DROP TRIGGER IF EXISTS sort_availability_autotag_insert; DROP TRIGGER IF EXISTS sort_availability_autotag_update; DROP TRIGGER IF EXISTS sort_availability_autotag_delete; DROP TRIGGER IF EXISTS sort_availability_preferred_asset_insert; DROP TRIGGER IF EXISTS sort_availability_preferred_asset_update; DROP TRIGGER IF EXISTS sort_availability_preferred_asset_delete; CREATE TRIGGER sort_availability_listing_insert
                AFTER INSERT ON catalog_listings BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE id=NEW.id;
             END;
             CREATE TRIGGER sort_availability_listing_update
                AFTER UPDATE OF metadata,recording_id ON catalog_listings
                WHEN OLD.metadata IS NOT NEW.metadata OR OLD.recording_id IS NOT NEW.recording_id BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE id=NEW.id;
             END;
             CREATE TRIGGER sort_availability_manual_update
                AFTER UPDATE OF manual_metadata ON recordings
                WHEN OLD.manual_metadata IS NOT NEW.manual_metadata BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=NEW.id;
             END;
             CREATE TRIGGER sort_availability_asset_insert
                AFTER INSERT ON local_assets BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=NEW.recording_id;
             END;
             CREATE TRIGGER sort_availability_asset_update
                AFTER UPDATE OF recording_id,path,hash,payload ON local_assets
                WHEN OLD.recording_id IS NOT NEW.recording_id OR OLD.path IS NOT NEW.path
                  OR OLD.hash IS NOT NEW.hash OR OLD.payload IS NOT NEW.payload BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id IN (OLD.recording_id,NEW.recording_id);
             END;
             CREATE TRIGGER sort_availability_asset_delete
                AFTER DELETE ON local_assets BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=OLD.recording_id;
             END;
             CREATE TRIGGER sort_availability_analysis_insert
                AFTER INSERT ON asset_analyses BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=(SELECT recording_id FROM local_assets WHERE id=NEW.asset_id);
             END;
             CREATE TRIGGER sort_availability_analysis_update
                AFTER UPDATE ON asset_analyses
                WHEN OLD.asset_id IS NOT NEW.asset_id OR OLD.file_hash IS NOT NEW.file_hash
                  OR OLD.version IS NOT NEW.version OR OLD.status IS NOT NEW.status
                  OR OLD.payload IS NOT NEW.payload OR OLD.updated_at IS NOT NEW.updated_at BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id IN (SELECT recording_id FROM local_assets WHERE id IN (OLD.asset_id,NEW.asset_id));
             END;
             CREATE TRIGGER sort_availability_analysis_delete
                AFTER DELETE ON asset_analyses BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=(SELECT recording_id FROM local_assets WHERE id=OLD.asset_id);
             END;
             CREATE TRIGGER sort_availability_autotag_insert
                AFTER INSERT ON asset_autotags BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=(SELECT recording_id FROM local_assets WHERE id=NEW.asset_id);
             END;
             CREATE TRIGGER sort_availability_autotag_update
                AFTER UPDATE ON asset_autotags
                WHEN OLD.asset_id IS NOT NEW.asset_id OR OLD.file_hash IS NOT NEW.file_hash
                  OR OLD.version IS NOT NEW.version OR OLD.status IS NOT NEW.status
                  OR OLD.payload IS NOT NEW.payload OR OLD.updated_at IS NOT NEW.updated_at BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id IN (SELECT recording_id FROM local_assets WHERE id IN (OLD.asset_id,NEW.asset_id));
             END;
             CREATE TRIGGER sort_availability_autotag_delete
                AFTER DELETE ON asset_autotags BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=(SELECT recording_id FROM local_assets WHERE id=OLD.asset_id);
             END;
             CREATE TRIGGER sort_availability_preferred_asset_insert
                AFTER INSERT ON settings WHEN NEW.key GLOB 'preferredAsset.*' BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=substr(NEW.key,16);
             END;
             CREATE TRIGGER sort_availability_preferred_asset_update
                AFTER UPDATE OF value ON settings
                WHEN NEW.key GLOB 'preferredAsset.*' AND OLD.value IS NOT NEW.value BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=substr(NEW.key,16);
             END;
             CREATE TRIGGER sort_availability_preferred_asset_delete
                AFTER DELETE ON settings WHEN OLD.key GLOB 'preferredAsset.*' BEGIN
                UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE recording_id=substr(OLD.key,16);
             END; COMMIT;",
        ).map_err(err)?;
        // Hydration and paged effective-value lookups are keyed by recording.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS local_assets_recording ON local_assets(recording_id);
             CREATE INDEX IF NOT EXISTS catalog_listings_recording ON catalog_listings(recording_id);
             CREATE INDEX IF NOT EXISTS catalog_import_source_url ON catalog_listings(provider,source_url);",
        )
        .map_err(err)?;
        let stale_projection_index =
            sort_projection_stale_index_name(crate::track_sort_sql::AVAILABILITY_MASK_REVISION);
        let stale_projection_predicate =
            sort_projection_stale_predicate("", crate::track_sort_sql::AVAILABILITY_MASK_REVISION);
        conn.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS {stale_projection_index}
             ON catalog_listings(recording_id) WHERE {stale_projection_predicate};"
        ))
        .map_err(err)?;
        let bpm_sort_index_expression = bpm_projection_sort_expression("sort_metadata_projection");
        let bpm_sort_null_expression =
            format!("CASE WHEN ({bpm_sort_index_expression}) IS NULL THEN 1 ELSE 0 END");
        let bpm_sort_index_sql = format!(
            "CREATE INDEX IF NOT EXISTS catalog_listings_sort_bpm_asc_tiedesc_r{revision}
                ON catalog_listings(({bpm_sort_null_expression}) DESC,({bpm_sort_index_expression}) DESC)
                WHERE in_library=1 AND sort_projection_revision={revision} AND sort_metadata_projection IS NOT NULL;
             CREATE INDEX IF NOT EXISTS catalog_listings_sort_bpm_desc_tiedesc_r{revision}
                ON catalog_listings(({bpm_sort_null_expression}) DESC,({bpm_sort_index_expression}) ASC)
                WHERE in_library=1 AND sort_projection_revision={revision} AND sort_metadata_projection IS NOT NULL;",
            revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION,
        );
        conn.execute_batch(&bpm_sort_index_sql).map_err(err)?;

        // Hydration and paged effective-value lookups are keyed by recording.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS local_assets_recording ON local_assets(recording_id);
             CREATE INDEX IF NOT EXISTS local_assets_path ON local_assets(path);
             CREATE INDEX IF NOT EXISTS catalog_listings_recording ON catalog_listings(recording_id);
             CREATE INDEX IF NOT EXISTS catalog_import_source_url ON catalog_listings(provider,source_url);",
        )
        .map_err(err)?;
        let store = Self {
            connection: Mutex::new(conn),
            connection_metrics_enabled: db_wait_metrics_enabled(),
            connection_wait_metrics: ConnectionWaitMetrics::default(),
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
    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.lock_connection_inner(None)
    }
    #[cfg(test)]
    fn lock_connection_observed(
        &self,
        on_attempt: &dyn Fn(Instant),
    ) -> Result<MutexGuard<'_, Connection>> {
        self.lock_connection_inner(Some(on_attempt))
    }
    fn lock_connection_inner(
        &self,
        on_attempt: Option<&dyn Fn(Instant)>,
    ) -> Result<MutexGuard<'_, Connection>> {
        if !self.connection_metrics_enabled {
            return self.connection.lock().map_err(err);
        }
        let started = Instant::now();
        if let Some(on_attempt) = on_attempt {
            on_attempt(started);
        }
        let result = self.connection.lock();
        let wait_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.connection_wait_metrics.record(wait_ns);
        result.map_err(err)
    }
    /// Opt-in diagnostic for the single global Store connection mutex. This
    /// intentionally does not claim to measure SQLite engine busy-handler waits.
    pub fn connection_wait_metrics(&self) -> Option<serde_json::Value> {
        self.connection_metrics_enabled.then(|| {
            serde_json::json!({
                "scope": "global_store_connection_mutex",
                "lockCount": self.connection_wait_metrics.lock_count.load(Ordering::Relaxed),
                "totalWaitNs": self.connection_wait_metrics.total_wait_ns.load(Ordering::Relaxed),
                "maxWaitNs": self.connection_wait_metrics.max_wait_ns.load(Ordering::Relaxed),
                "sqliteEngineBusyWait": "not_measured"
            })
        })
    }
    pub fn settings(&self) -> Result<Settings> {
        let conn = self.lock_connection()?;
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
        settings.validate_playback_startup_timeout()?;
        self.lock_connection()?.execute("INSERT INTO settings(key,value) VALUES('application',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [encode(settings)?]).map_err(err)?;
        Ok(())
    }
    pub fn get_json_preference(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.lock_connection()?;
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
    /// Commit queue occurrence identity and its session position together. Omitted
    /// components retain the last committed value, matching incremental callers.
    pub fn save_playback_checkpoint(
        &self,
        queue: Option<&serde_json::Value>,
        session: Option<&serde_json::Value>,
    ) -> Result<()> {
        self.save_playback_checkpoint_and_engine(queue, session, None)
    }
    /// Engine selection and its complete session must commit together.
    pub fn save_playback_checkpoint_and_engine(
        &self,
        queue: Option<&serde_json::Value>,
        session: Option<&serde_json::Value>,
        engine: Option<&serde_json::Value>,
    ) -> Result<()> {
        if queue.is_none() && session.is_none() && engine.is_none() {
            return Ok(());
        }
        let engine = engine.map(encode).transpose()?;
        let queue = queue.map(encode).transpose()?;
        let session = session.map(encode).transpose()?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(err)?;
        for (key, value) in [
            ("nativePlaybackQueue.v1", queue),
            ("nativePlaybackSession.v1", session),
            ("playbackEngine.v1", engine),
        ] {
            if let Some(value) = value {
                transaction
                    .execute(
                        "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                        params![key, value],
                    )
                    .map_err(err)?;
            }
        }
        transaction.commit().map_err(err)
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
        let conn = self.lock_connection()?;
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

    /// Find only cached raw MusicBrainz artist profiles whose canonical name or
    /// alias exactly matches the supplied bounded name. This is a scoped read;
    /// it does not expose general provider-cache enumeration.
    pub fn cached_musicbrainz_artist_profiles_for_name(
        &self,
        name: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let name = name.trim();
        if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
            return Ok(Vec::new());
        }
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT payload FROM provider_cache
                 WHERE namespace='artists.profile.musicbrainz'
                   AND julianday(expires_at)>julianday('now')
                   AND json_valid(payload)
                   AND (lower(trim(json_extract(payload,'$.name')))=lower(?1)
                     OR EXISTS(SELECT 1 FROM json_each(payload,'$.aliases') a
                       WHERE lower(trim(CASE WHEN a.type='object' THEN json_extract(a.value,'$.name') WHEN a.type='text' THEN a.value ELSE NULL END))=lower(?1)))
                 LIMIT 3",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([name], |row| row.get::<_, String>(0))
            .map_err(err)?;
        let wanted = name.to_lowercase();
        let mut profiles = std::collections::BTreeMap::<String, serde_json::Value>::new();
        for row in rows {
            let Ok(payload) = row else { continue };
            let Ok(profile) = serde_json::from_str::<serde_json::Value>(&payload) else {
                continue;
            };
            let Some(id) = profile["id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            let matches = profile["name"]
                .as_str()
                .is_some_and(|candidate| candidate.trim().to_lowercase() == wanted)
                || profile["aliases"].as_array().is_some_and(|aliases| {
                    aliases.iter().any(|alias| {
                        alias
                            .as_str()
                            .or_else(|| alias["name"].as_str())
                            .is_some_and(|candidate| candidate.trim().to_lowercase() == wanted)
                    })
                });
            if matches {
                profiles.entry(id.to_owned()).or_insert(profile);
                if profiles.len() >= 3 {
                    break;
                }
            }
        }
        Ok(profiles.into_values().collect())
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
                "INSERT INTO provider_cache(namespace,key,payload,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,key) DO UPDATE SET payload=excluded.payload,expires_at=CASE WHEN provider_cache.expires_at='9999-12-31T23:59:59Z' AND ?5 THEN provider_cache.expires_at ELSE excluded.expires_at END",
                params![namespace, key, encoded, expires_at, ttl_seconds != 0],
            )
            .map_err(err)?;
        Ok(())
    }

    fn matching_evidence_cache_key(
        requested_track_id: &str,
        requested_fingerprint: &str,
        source_provider: &str,
        source_id: &str,
        evaluator_version: &str,
        policy: crate::recording_match::MatchLevel,
    ) -> Result<String> {
        if requested_track_id.trim().is_empty()
            || requested_track_id.len() > 512
            || !requested_track_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'/' | b'_' | b'-' | b'.'))
            || requested_fingerprint.len() != 64
            || !requested_fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
            || !matches!(source_provider, "youtube" | "soundcloud")
            || !stable_provider_id(source_provider, source_id)
            || evaluator_version.trim().is_empty()
            || evaluator_version.len() > 128
        {
            return Err("Matching evidence identity is invalid".into());
        }
        let identity = serde_json::to_vec(&(
            1u32,
            requested_track_id,
            requested_fingerprint.to_ascii_lowercase(),
            source_provider,
            source_id,
            evaluator_version,
            policy,
        ))
        .map_err(err)?;
        use sha2::{Digest, Sha256};
        Ok(format!("{:x}", Sha256::digest(identity)))
    }

    fn matching_evidence_has_no_secrets(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().all(|(key, value)| {
                let key = key.to_ascii_lowercase();
                ![
                    "url",
                    "uri",
                    "token",
                    "cookie",
                    "authorization",
                    "headers",
                    "secret",
                ]
                .iter()
                .any(|blocked| key.contains(blocked))
                    && Self::matching_evidence_has_no_secrets(value)
            }),
            serde_json::Value::Array(values) => {
                values.iter().all(Self::matching_evidence_has_no_secrets)
            }
            serde_json::Value::String(text) => {
                let text = text.to_ascii_lowercase();
                !text.contains("http://") && !text.contains("https://")
            }
            _ => true,
        }
    }

    /// Store source matching evidence under requested identity, source identity,
    /// evaluator version, and policy. The cached assessment is never an accept
    /// decision; callers re-run their evaluator when it is read.
    pub fn save_matching_evidence(&self, record: &MatchingEvidenceRecord) -> Result<()> {
        if record.schema_version != 1
            || record.requested_fingerprint.len() != 64
            || !Self::matching_evidence_has_no_secrets(&record.evidence)
        {
            return Err("Matching evidence is invalid or contains sensitive source data".into());
        }
        let key = Self::matching_evidence_cache_key(
            &record.requested_track_id,
            &record.requested_fingerprint,
            &record.source_provider,
            &record.source_id,
            &record.evaluator_version,
            record.policy,
        )?;
        self.set_provider_cache(
            "playback.matching-evidence.v1",
            &key,
            &serde_json::to_value(record).map_err(err)?,
            45 * 86400,
        )
    }

    pub fn get_matching_evidence(
        &self,
        requested_track_id: &str,
        requested_fingerprint: &str,
        source_provider: &str,
        source_id: &str,
        evaluator_version: &str,
        policy: crate::recording_match::MatchLevel,
    ) -> Result<Option<MatchingEvidenceRecord>> {
        let key = Self::matching_evidence_cache_key(
            requested_track_id,
            requested_fingerprint,
            source_provider,
            source_id,
            evaluator_version,
            policy,
        )?;
        let Some(value) = self.get_provider_cache("playback.matching-evidence.v1", &key)? else {
            return Ok(None);
        };
        let record: MatchingEvidenceRecord = match serde_json::from_value(value) {
            Ok(record) => record,
            Err(_) => return Ok(None),
        };
        if record.schema_version != 1
            || record.requested_track_id != requested_track_id
            || !record
                .requested_fingerprint
                .eq_ignore_ascii_case(requested_fingerprint)
            || record.source_provider != source_provider
            || record.source_id != source_id
            || record.evaluator_version != evaluator_version
            || record.policy != policy
            || !Self::matching_evidence_has_no_secrets(&record.evidence)
        {
            return Ok(None);
        }
        Ok(Some(record))
    }

    /// Retain identity evidence independently of its refresh age. Callers must
    /// store and validate their own provenance, fingerprint and fetchedAt.
    /// Unlike ordinary provider responses this evidence never expires by age.
    pub(crate) fn set_retained_provider_cache(
        &self,
        namespace: &str,
        key: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        if !namespace.starts_with("artists.") || key.trim().is_empty() {
            return Err("Retained artist evidence requires an artist namespace and key".into());
        }
        self.write_retained_provider_cache(namespace, key, payload)
    }

    /// Only this explicit API can retain a playback locator. The general
    /// retained-provider API remains restricted to artist evidence.
    pub(crate) fn set_retained_playback_locator(
        &self,
        key: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        if key.trim().is_empty() || key.len() > 512 {
            return Err("A bounded playback locator key is required".into());
        }
        self.write_retained_provider_cache(PLAYBACK_LOCATOR_CACHE_NAMESPACE, key, payload)
    }

    fn write_retained_provider_cache(
        &self,
        namespace: &str,
        key: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let encoded = encode(payload)?;
        if encoded.len() > 1024 * 1024 {
            return Err("Cached provider data is limited to 1 MB".into());
        }
        self.lock_connection()?.execute(
            "INSERT INTO provider_cache(namespace,key,payload,expires_at) VALUES(?1,?2,?3,'9999-12-31T23:59:59Z') ON CONFLICT(namespace,key) DO UPDATE SET payload=excluded.payload,expires_at=excluded.expires_at",
            params![namespace, key, encoded],
        ).map_err(err)?;
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
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        upsert_track(&tx, track, true)?;
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
    /// Cache a provider or discovery result without adding it to the user's Library.
    /// Existing saved membership is monotonic, so enrichment cannot demote it.
    pub fn cache_track(&self, track: &Track) -> Result<Track> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        upsert_track(&tx, track, false)?;
        let raw: String = tx
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&track.id],
                |row| row.get(0),
            )
            .map_err(err)?;
        let result = hydrate(&tx, decode(raw)?)?;
        tx.commit().map_err(err)?;
        Ok(result)
    }
    /// Cache album/provider metadata and add all supplied tracks to the Library atomically.
    /// Manual recording metadata remains in `recordings.manual_metadata` and is never replaced.
    pub fn save_library_tracks(&self, tracks: &[Track]) -> Result<Vec<String>> {
        const MAX_TRACKS: usize = 10_000;
        if tracks.is_empty() {
            return Err("Choose at least one track to save to the Library".into());
        }
        if tracks.len() > MAX_TRACKS {
            return Err(format!("Cannot save more than {MAX_TRACKS} tracks at once").into());
        }
        let mut unique = Vec::with_capacity(tracks.len());
        let mut seen = std::collections::HashSet::with_capacity(tracks.len());
        for track in tracks {
            if track.id.is_empty()
                || track.provider_id.is_empty()
                || track.provider.is_empty()
                || track.id != format!("{}:{}", track.provider, track.provider_id)
            {
                return Err("Track identity is missing or invalid".into());
            }
            if seen.insert(track.id.clone()) {
                unique.push(track);
            }
        }
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        for track in &unique {
            upsert_track(&tx, track, false)?;
            promote_track(&tx, &track.id)?;
        }
        tx.commit().map_err(err)?;
        Ok(unique.into_iter().map(|track| track.id.clone()).collect())
    }
    /// Remember the public source that actually played without treating it as a verified recording match.
    pub fn save_audio_match(&self, track: &Track, candidate: &Candidate) -> Result<Track> {
        let conn = self.lock_connection()?;
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
        let mut conn = self.lock_connection()?;
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
        promote_track(&tx, &track.id)?;
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
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT metadata FROM catalog_listings WHERE id=?1")
            .map_err(err)?;
        let mut rows = stmt.query([id]).map_err(err)?;
        match rows.next().map_err(err)? {
            Some(r) => Ok(Some(hydrate(&conn, decode(r.get(0).map_err(err)?)?)?)),
            None => Ok(None),
        }
    }

    /// Read an exact provider listing only when it is currently saved in the
    /// user's Library. This avoids treating an ephemeral cached search result
    /// as a source identity for persistent playback crosswalks.
    pub fn get_library_track(&self, id: &str) -> Result<Option<Track>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT metadata FROM catalog_listings WHERE id=?1 AND in_library=1")
            .map_err(err)?;
        let mut rows = stmt.query([id]).map_err(err)?;
        match rows.next().map_err(err)? {
            Some(row) => Ok(Some(hydrate(&conn, decode(row.get(0).map_err(err)?)?)?)),
            None => Ok(None),
        }
    }

    pub fn save_verified_playback_source(
        &self,
        track_id: &str,
        fingerprint: &str,
        provider: &str,
        provider_id: &str,
        permanent_url: &str,
    ) -> Result<()> {
        self.save_verified_playback_source_proof(
            track_id,
            fingerprint,
            provider,
            provider_id,
            permanent_url,
            None,
        )
    }

    pub fn save_verified_playback_source_with_proof(
        &self,
        track_id: &str,
        fingerprint: &str,
        provider: &str,
        provider_id: &str,
        permanent_url: &str,
        proof: &serde_json::Value,
    ) -> Result<()> {
        self.save_verified_playback_source_proof(
            track_id,
            fingerprint,
            provider,
            provider_id,
            permanent_url,
            Some(proof),
        )
    }

    fn save_verified_playback_source_proof(
        &self,
        track_id: &str,
        fingerprint: &str,
        provider: &str,
        provider_id: &str,
        permanent_url: &str,
        proof: Option<&serde_json::Value>,
    ) -> Result<()> {
        if fingerprint.len() != 64
            || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !matches!(provider, "youtube" | "soundcloud")
            || !stable_provider_id(provider, provider_id)
            || permanent_url.len() > 2048
            || !stable_playback_source_matches(provider, provider_id, permanent_url)
        {
            return Err(
                "Verified playback source must use a stable provider ID and permanent URL".into(),
            );
        }
        let now = chrono::Utc::now();
        let expires_at = (now + chrono::Duration::days(45)).to_rfc3339();
        let updated_at = now.to_rfc3339();
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(err)?;
        let recording_id: Option<String> = transaction
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [track_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        let recording_id =
            recording_id.ok_or("Verified playback source track is not in the library")?;
        transaction.execute(
            "INSERT INTO verified_playback_sources(recording_id,fingerprint,provider,provider_id,permanent_url,expires_at,updated_at,verification_proof)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(recording_id,fingerprint,provider) DO UPDATE SET
                 provider_id=excluded.provider_id,permanent_url=excluded.permanent_url,
                 expires_at=excluded.expires_at,updated_at=excluded.updated_at,
                 verification_proof=excluded.verification_proof",
            params![recording_id, fingerprint.to_ascii_lowercase(), provider, provider_id, permanent_url, expires_at, updated_at, proof.map(encode).transpose()?],
        ).map_err(err)?;
        transaction.commit().map_err(err)
    }

    pub fn verified_playback_source(
        &self,
        track_id: &str,
        fingerprint: &str,
        provider: &str,
    ) -> Result<Option<VerifiedPlaybackSource>> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(err)?;
        let recording_id: Option<String> = transaction
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [track_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        let Some(recording_id) = recording_id else {
            return Ok(None);
        };
        let row: Option<(String, String, String, Option<String>)> = transaction
            .query_row(
                "SELECT provider_id,permanent_url,expires_at,verification_proof FROM verified_playback_sources
                 WHERE recording_id=?1 AND fingerprint=?2 AND provider=?3",
                params![recording_id, fingerprint.to_ascii_lowercase(), provider],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(err)?;
        let Some((provider_id, permanent_url, expires_at, verification_proof)) = row else {
            return Ok(None);
        };
        let expires = chrono::DateTime::parse_from_rfc3339(&expires_at).map_err(err)?;
        if expires <= chrono::Utc::now()
            || !stable_playback_source_matches(provider, &provider_id, &permanent_url)
        {
            transaction.execute(
                "DELETE FROM verified_playback_sources WHERE recording_id=?1 AND fingerprint=?2 AND provider=?3",
                params![recording_id, fingerprint.to_ascii_lowercase(), provider],
            ).map_err(err)?;
            transaction.commit().map_err(err)?;
            return Ok(None);
        }
        transaction.commit().map_err(err)?;
        Ok(Some(VerifiedPlaybackSource {
            provider: provider.to_owned(),
            provider_id,
            permanent_url,
            // A malformed or legacy proof is unverified, not a broken library.
            verification_proof: verification_proof.and_then(|proof| decode(proof).ok()),
        }))
    }

    pub fn invalidate_verified_playback_source(
        &self,
        track_id: &str,
        fingerprint: &str,
        provider: &str,
        provider_id: &str,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        connection
            .execute(
                "DELETE FROM verified_playback_sources
             WHERE recording_id=(SELECT recording_id FROM catalog_listings WHERE id=?1)
               AND fingerprint=?2 AND provider=?3 AND provider_id=?4",
                params![
                    track_id,
                    fingerprint.to_ascii_lowercase(),
                    provider,
                    provider_id
                ],
            )
            .map_err(err)?;
        Ok(())
    }
    /// Return canonical favorite listing IDs without hydrating full track metadata.
    pub fn list_favorite_track_ids(&self) -> Result<Vec<String>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT l.id FROM catalog_listings l JOIN recordings r ON l.recording_id=r.id WHERE r.favorite=1 AND l.id=l.recording_id ORDER BY l.rowid DESC",
            )
            .map_err(err)?;
        let ids = stmt
            .query_map([], |row| row.get(0))
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err);
        ids
    }
    pub fn list_tracks(&self, view: &str, collection_id: Option<&str>) -> Result<Vec<Track>> {
        let conn = self.lock_connection()?;
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
                "SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1 ORDER BY l.rowid DESC"
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

    /// Preview Library entries without a favorite, shortlist, collection, or
    /// local-file reference. Legacy rows have no reliable saved-vs-searched
    /// provenance, so this never claims an entry came from search.
    pub fn preview_unsorted_library_entries(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<UnsortedLibraryPreview> {
        const MAX_PAGE: usize = 100;
        const MAX_OFFSET: usize = 1_000_000;
        if offset > MAX_OFFSET {
            return Err("Library cleanup offset is out of range".into());
        }
        let limit = limit.clamp(1, MAX_PAGE);
        let limit_sql = i64::try_from(limit).map_err(err)?;
        let offset_sql = i64::try_from(offset).map_err(err)?;
        let conn = self.lock_connection()?;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM catalog_listings l
                 JOIN recordings r ON r.id=l.recording_id
                 WHERE l.in_library=1 AND COALESCE(r.favorite,0)=0
                   AND COALESCE(r.disposition,'')<>'shortlist'
                   AND NOT EXISTS(SELECT 1 FROM collection_members m WHERE m.recording_id=l.recording_id)
                   AND NOT EXISTS(SELECT 1 FROM local_assets a WHERE a.recording_id=l.recording_id)",
                [],
                |row| row.get(0),
            )
            .map_err(err)?;
        let raw_rows = {
            let mut statement = conn
                .prepare(
                    "SELECT l.id,l.metadata,l.updated_at,r.manual_metadata FROM catalog_listings l
                     JOIN recordings r ON r.id=l.recording_id
                     WHERE l.in_library=1 AND COALESCE(r.favorite,0)=0
                       AND COALESCE(r.disposition,'')<>'shortlist'
                       AND NOT EXISTS(SELECT 1 FROM collection_members m WHERE m.recording_id=l.recording_id)
                       AND NOT EXISTS(SELECT 1 FROM local_assets a WHERE a.recording_id=l.recording_id)
                     ORDER BY l.rowid DESC LIMIT ?1 OFFSET ?2",
                )
                .map_err(err)?;
            let rows = statement
                .query_map(params![limit_sql, offset_sql], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            rows
        };
        let mut entries = Vec::with_capacity(raw_rows.len());
        for (listing_id, raw_metadata, updated_at, manual_metadata) in raw_rows {
            let raw_track: Track = decode(raw_metadata.clone())?;
            let track = hydrate(&conn, raw_track)?;
            entries.push(UnsortedLibraryEntry {
                id: listing_id.clone(),
                provider: track.provider,
                provider_id: track.provider_id,
                artist: track.artist,
                title: track.title,
                album: track.album,
                selection_token: unsorted_library_selection_token(
                    &listing_id,
                    &updated_at,
                    &raw_metadata,
                    &manual_metadata,
                ),
                classification: "unsortedLibraryEntry",
            });
        }
        Ok(UnsortedLibraryPreview {
            entries,
            total: usize::try_from(total).unwrap_or(0),
            limit,
            offset,
        })
    }

    /// Demote only exact rows the user reviewed. The token binds each preview
    /// to its listing metadata and timestamp; protection conditions are
    /// rechecked atomically. Listing metadata and any local files are retained.
    pub fn remove_unsorted_library_entries(
        &self,
        selections: &[LibraryEntrySelection],
    ) -> Result<UnsortedLibraryRemovalResult> {
        const MAX_SELECTIONS: usize = 500;
        if selections.len() > MAX_SELECTIONS {
            return Err(format!("Select at most {MAX_SELECTIONS} Library entries").into());
        }
        let mut seen = std::collections::HashSet::with_capacity(selections.len());
        for selection in selections {
            if selection.id.trim().is_empty()
                || selection.id.len() > 512
                || selection.selection_token.len() != 64
                || !selection
                    .selection_token
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || !seen.insert(selection.id.as_str())
            {
                return Err("Library cleanup selection is invalid".into());
            }
        }
        let mut conn = self.lock_connection()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let mut removed_ids = Vec::new();
        let mut skipped = Vec::new();
        for selection in selections {
            let current: Option<(String, String, String, bool)> = tx
                .query_row(
                    "SELECT l.metadata,l.updated_at,r.manual_metadata,
                       (l.in_library=1
                        AND COALESCE(r.favorite,0)=0
                        AND COALESCE(r.disposition,'')<>'shortlist'
                        AND NOT EXISTS(SELECT 1 FROM collection_members m WHERE m.recording_id=l.recording_id)
                        AND NOT EXISTS(SELECT 1 FROM local_assets a WHERE a.recording_id=l.recording_id))
                     FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id
                     WHERE l.id=?1",
                    [&selection.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(err)?;
            let Some((metadata, updated_at, manual_metadata, removable)) = current else {
                skipped.push(LibraryCleanupSkippedEntry {
                    id: selection.id.clone(),
                    reason: "notFound".into(),
                });
                continue;
            };
            if unsorted_library_selection_token(
                &selection.id,
                &updated_at,
                &metadata,
                &manual_metadata,
            ) != selection.selection_token
            {
                skipped.push(LibraryCleanupSkippedEntry {
                    id: selection.id.clone(),
                    reason: "changedSincePreview".into(),
                });
                continue;
            }
            if !removable {
                skipped.push(LibraryCleanupSkippedEntry {
                    id: selection.id.clone(),
                    reason: "protectedOrNoLongerInLibrary".into(),
                });
                continue;
            }
            let changed = tx
                .execute(
                    "UPDATE catalog_listings SET in_library=0
                     WHERE id=?1 AND in_library=1 AND updated_at=?2",
                    params![selection.id, updated_at],
                )
                .map_err(err)?;
            if changed == 1 {
                removed_ids.push(selection.id.clone());
            } else {
                skipped.push(LibraryCleanupSkippedEntry {
                    id: selection.id.clone(),
                    reason: "changedSincePreview".into(),
                });
            }
        }
        tx.commit().map_err(err)?;
        Ok(UnsortedLibraryRemovalResult {
            removed_ids,
            skipped,
        })
    }

    /// Read a stable page from a saved view while hydrating only rows on that page.
    /// Exact legacy response contract with cached asset sort projections when ready.
    /// Retains the legacy aggregate/metadata flags, and falls back synchronously
    /// if that same query cannot yet use complete projections. No backfill jobs.
    pub fn list_tracks_page_exact_cached(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
    ) -> Result<(Vec<Track>, usize)> {
        // Duration-only bounds/sorts and source-order paths do not perform the
        // projection-pending check in track_page_rows: keep them exactly legacy.
        let checks_projection_readiness = query.is_some_and(|value| !value.trim().is_empty())
            || sort_field.is_some_and(|field| {
                matches!(
                    field,
                    "title" | "artist" | "album" | "releaseDate" | "bpm" | "key"
                )
            });
        if !checks_projection_readiness {
            return self.list_tracks_page(
                view,
                collection_id,
                limit,
                offset,
                query,
                sort_field,
                direction,
                min_duration_seconds,
                max_duration_seconds,
                availability,
                provider,
            );
        }
        let (rows, total, _, pending) = self.track_page_rows(
            view,
            collection_id,
            limit,
            offset,
            query,
            sort_field,
            direction,
            min_duration_seconds,
            max_duration_seconds,
            availability,
            provider,
            true,
            false,
            true,
        )?;
        if pending {
            return self.list_tracks_page(
                view,
                collection_id,
                limit,
                offset,
                query,
                sort_field,
                direction,
                min_duration_seconds,
                max_duration_seconds,
                availability,
                provider,
            );
        }
        Ok((
            rows.into_iter().filter_map(|row| row.track).collect(),
            total,
        ))
    }

    pub fn list_tracks_page(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
    ) -> Result<(Vec<Track>, usize)> {
        self.track_page_rows(
            view,
            collection_id,
            limit,
            offset,
            query,
            sort_field,
            direction,
            min_duration_seconds,
            max_duration_seconds,
            availability,
            provider,
            true,
            false,
            false,
        )
        .map(|(rows, total, _, _)| {
            (
                rows.into_iter().filter_map(|row| row.track).collect(),
                total,
            )
        })
    }
    /// Read a page and, on its first page only, report sort fields with at least
    /// one comparable value in the complete filtered view. The page API above
    /// stays unchanged for existing callers.
    pub fn list_tracks_page_with_availability(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
    ) -> Result<(
        Vec<Track>,
        usize,
        Option<std::collections::BTreeMap<String, bool>>,
    )> {
        let (mut tracks, mut total, mut state) = self.list_tracks_page_with_availability_state(
            view,
            collection_id,
            limit,
            offset,
            query,
            sort_field,
            direction,
            min_duration_seconds,
            max_duration_seconds,
            availability,
            provider,
        )?;
        // Preserve the legacy method's completed-map contract. New IPC callers
        // should use the explicit state API so a cold first page stays bounded.
        if matches!(&state, Some(SortAvailabilityState::Pending)) {
            while self.backfill_sort_availability_batch(500)? != 0 {}
            (tracks, total, state) = self.list_tracks_page_with_availability_state(
                view,
                collection_id,
                limit,
                offset,
                query,
                sort_field,
                direction,
                min_duration_seconds,
                max_duration_seconds,
                availability,
                provider,
            )?;
        }
        let ready = match state {
            Some(SortAvailabilityState::Ready(fields)) => Some(fields),
            Some(SortAvailabilityState::Pending) | None => None,
        };
        Ok((tracks, total, ready))
    }

    pub fn list_tracks_page_with_availability_state(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
    ) -> Result<(Vec<Track>, usize, Option<SortAvailabilityState>)> {
        self.track_page_rows(
            view,
            collection_id,
            limit,
            offset,
            query,
            sort_field,
            direction,
            min_duration_seconds,
            max_duration_seconds,
            availability,
            provider,
            true,
            offset == 0,
            false,
        )
        .map(|(rows, total, sort_availability, _)| {
            (
                rows.into_iter().filter_map(|row| row.track).collect(),
                total,
                sort_availability,
            )
        })
    }
    /// Read a paged view with explicit global-availability and exact-sort
    /// readiness. If asset metadata needed for this query is still backfilling,
    /// returns no page rows and `sort_pending=true` so callers can hold their
    /// existing view until the same request can be rerun exactly.
    pub fn list_tracks_page_with_sort_state(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
    ) -> Result<(Vec<Track>, usize, Option<SortAvailabilityState>, bool)> {
        self.track_page_rows(
            view,
            collection_id,
            limit,
            offset,
            query,
            sort_field,
            direction,
            min_duration_seconds,
            max_duration_seconds,
            availability,
            provider,
            true,
            offset == 0,
            true,
        )
        .map(|(rows, total, sort_availability, sort_pending)| {
            (
                rows.into_iter().filter_map(|row| row.track).collect(),
                total,
                sort_availability,
                sort_pending,
            )
        })
    }
    pub fn list_track_prefix(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
    ) -> Result<(Vec<TrackPrefixEntry>, usize)> {
        self.track_page_rows(
            view,
            collection_id,
            limit,
            0,
            query,
            sort_field,
            direction,
            min_duration_seconds,
            max_duration_seconds,
            availability,
            provider,
            false,
            false,
            false,
        )
        .map(|(rows, total, _, _)| {
            (
                rows.into_iter()
                    .map(|row| TrackPrefixEntry {
                        listing_id: row.listing_id,
                        playlist_position: row.playlist_position,
                    })
                    .collect(),
                total,
            )
        })
    }
    fn track_page_rows(
        &self,
        view: &str,
        collection_id: Option<&str>,
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
        provider: Option<&str>,
        hydrate_tracks: bool,
        include_sort_availability: bool,
        allow_sort_pending: bool,
    ) -> Result<(
        Vec<TrackPageRow>,
        usize,
        Option<SortAvailabilityState>,
        bool,
    )> {
        #[cfg(test)]
        let profile_enabled = std::env::var_os("BURRATA_AVAILABILITY_PROFILE").is_some();
        #[cfg(test)]
        let profile_started = profile_enabled.then(std::time::Instant::now);
        #[cfg(test)]
        let lock_started = profile_enabled.then(std::time::Instant::now);
        let conn = self.lock_connection()?;
        #[cfg(test)]
        let lock_wait_micros = lock_started.map_or(0, |started| started.elapsed().as_micros());
        let needs_effective_metadata = query.is_some_and(|value| !value.trim().is_empty())
            || min_duration_seconds.is_some()
            || max_duration_seconds.is_some()
            || sort_field.is_some_and(|field| {
                !matches!(
                    field,
                    "sourceOrder" | "dateAdded" | "plays" | "views" | "duration"
                )
            });
        let needs_cached_projection = query.is_some_and(|value| !value.trim().is_empty())
            || sort_field.is_some_and(|field| {
                matches!(
                    field,
                    "title" | "artist" | "album" | "releaseDate" | "bpm" | "key"
                )
            });
        #[cfg(test)]
        let projection_pending_started = profile_enabled.then(std::time::Instant::now);
        if allow_sort_pending && needs_cached_projection {
            let projection_pending_sql = projection_pending_single_asset_sql(
                crate::track_sort_sql::AVAILABILITY_MASK_REVISION,
            );
            let projection_pending: bool = conn
                .query_row(&projection_pending_sql, [], |row| row.get(0))
                .map_err(err)?;
            if projection_pending {
                #[cfg(test)]
                if let Some(started) = projection_pending_started {
                    eprintln!(
                        "BURRATA_AVAILABILITY_PROJECTION_PENDING_CHECK {}",
                        serde_json::json!({"elapsedMicros":started.elapsed().as_micros(),"pending":true,"sortField":sort_field})
                    );
                }
                let availability = (include_sort_availability && offset == 0)
                    .then_some(SortAvailabilityState::Pending);
                return Ok((Vec::new(), 0, availability, true));
            }
        }
        #[cfg(test)]
        let projection_pending_micros =
            projection_pending_started.map_or(0, |started| started.elapsed().as_micros());
        // Match hydrate's filesystem-backed availability instead of treating stale
        // local_assets rows as playable. This temporary set is shared by every
        // filter and count in this page query.
        conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS page_local_recordings(recording_id TEXT PRIMARY KEY); DELETE FROM page_local_recordings;").map_err(err)?;
        conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS page_effective_metadata(id TEXT PRIMARY KEY,metadata TEXT NOT NULL,sort_mask INTEGER NOT NULL); DELETE FROM page_effective_metadata;").map_err(err)?;
        // Local asset metadata and completed analyses can supply fields that
        // hydrate exposes in the UI. Materialize only asset-backed listings so
        // filtering and sorting see those same effective values before LIMIT.
        #[cfg(test)]
        let asset_discovery_started = profile_enabled.then(std::time::Instant::now);
        let has_local_assets = if needs_effective_metadata || include_sort_availability {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM local_assets LIMIT 1)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(err)?
        } else {
            false
        };
        let asset_backed = if has_local_assets
            && (needs_effective_metadata || include_sort_availability)
        {
            let only_multiple_assets = if allow_sort_pending {
                // Single-asset rows have a canonical projection persisted by
                // bounded backfill. Discover multi-asset recordings by ID
                // first, so the outer materialization query never aggregates
                // their (potentially large) payload JSON for every cached row.
                multiple_stale_asset_recordings_sql(
                    crate::track_sort_sql::AVAILABILITY_MASK_REVISION,
                )
            } else {
                String::new()
            };
            let query = format!(
                "SELECT l.id,l.metadata,r.manual_metadata,r.favorite,r.disposition,COUNT(a.id),MAX(a.payload),EXISTS(SELECT 1 FROM local_assets ax JOIN asset_analyses aa ON aa.asset_id=ax.id AND aa.file_hash=ax.hash WHERE ax.recording_id=r.id) FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id {only_multiple_assets} JOIN local_assets a ON a.recording_id=r.id GROUP BY l.id"
            );
            let mut stmt = conn.prepare(&query).map_err(err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, bool>(7)?,
                    ))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            rows
        } else {
            Vec::new()
        };
        #[cfg(test)]
        let asset_discovery_micros =
            asset_discovery_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        let asset_listing_count = asset_backed.len();
        #[cfg(test)]
        let asset_hydration_started = profile_enabled.then(std::time::Instant::now);
        #[cfg(test)]
        let mut asset_decode_hydrate_micros = 0_u128;
        #[cfg(test)]
        let mut asset_mask_serialize_insert_micros = 0_u128;
        conn.execute_batch("SAVEPOINT page_effective_metadata_materialize")
            .map_err(err)?;
        let materialize_result = (|| -> Result<()> {
            let mut insert = conn
                .prepare(
                    "INSERT OR REPLACE INTO page_effective_metadata(id,metadata,sort_mask) VALUES(?1,?2,?3)",
                )
                .map_err(err)?;
            for (
                id,
                raw,
                manual,
                favorite,
                disposition,
                asset_count,
                asset_payload,
                has_analysis,
            ) in asset_backed
            {
                #[cfg(test)]
                let hydrate_started = profile_enabled.then(std::time::Instant::now);
                let decoded: Track = decode(raw)?;
                let track = if let Some(track) = fast_asset_sort_track(
                    &decoded,
                    &manual,
                    favorite,
                    disposition,
                    asset_count,
                    &asset_payload,
                    has_analysis,
                )? {
                    track
                } else {
                    hydrate(&conn, decoded)?
                };
                #[cfg(test)]
                if let Some(started) = hydrate_started {
                    asset_decode_hydrate_micros += started.elapsed().as_micros();
                }
                #[cfg(test)]
                let mask_started = profile_enabled.then(std::time::Instant::now);
                // The temporary page relation is only read by filter/sort SQL.
                // Keep that JSON limited to the fields those expressions inspect;
                // the selected page rows still hydrate from the original metadata.
                let effective = sort_metadata_projection(&track);
                let fields = crate::track_sort_sql::available_fields(&effective);
                let serialized = serde_json::to_string(&effective).map_err(err)?;
                insert
                    .execute(params![
                        id,
                        serialized,
                        crate::track_sort_sql::pack_available_fields(&fields)
                    ])
                    .map_err(err)?;
                #[cfg(test)]
                if let Some(started) = mask_started {
                    asset_mask_serialize_insert_micros += started.elapsed().as_micros();
                }
            }
            Ok(())
        })();
        match materialize_result {
            Ok(()) => conn
                .execute_batch("RELEASE SAVEPOINT page_effective_metadata_materialize")
                .map_err(err)?,
            Err(error) => {
                let rollback_result = conn.execute_batch(
                    "ROLLBACK TO SAVEPOINT page_effective_metadata_materialize; RELEASE SAVEPOINT page_effective_metadata_materialize",
                );
                return Err(match rollback_result {
                    Ok(()) => error,
                    Err(rollback_error) => {
                        format!("{error}; rollback failed: {}", err(rollback_error))
                    }
                });
            }
        }
        #[cfg(test)]
        let asset_hydration_micros =
            asset_hydration_started.map_or(0, |started| started.elapsed().as_micros());
        // In pending-aware reads this relation contains only dynamic
        // multi-asset rows. If it is empty, use persisted single-asset
        // projections/masks directly and skip a pointless per-listing lookup.
        let has_page_effective_metadata = if has_local_assets {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM page_effective_metadata LIMIT 1)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(err)?
        } else {
            false
        };
        #[cfg(test)]
        let local_presence_started = profile_enabled.then(std::time::Instant::now);
        if availability.is_some_and(|value| matches!(value, "local" | "not-local"))
            || provider == Some("local")
        {
            let mut stmt = conn
                .prepare("SELECT recording_id,path FROM local_assets ORDER BY rowid")
                .map_err(err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(err)?;
            let mut insert = conn
                .prepare("INSERT OR IGNORE INTO page_local_recordings(recording_id) VALUES(?1)")
                .map_err(err)?;
            for row in rows {
                let (recording_id, path) = row.map_err(err)?;
                if Path::new(&path).is_file() {
                    insert.execute([recording_id]).map_err(err)?;
                }
            }
        }
        #[cfg(test)]
        let local_presence_micros =
            local_presence_started.map_or(0, |started| started.elapsed().as_micros());
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
        let (from_sql, mut conditions, mut values, base_order, position_expr, legacy_desc): (
            &str,
            Vec<String>,
            Vec<rusqlite::types::Value>,
            &str,
            &str,
            bool,
        ) = match view {
            "favorites" => (
                "FROM catalog_listings l JOIN recordings r ON l.recording_id=r.id",
                vec!["r.favorite=1 AND l.id=l.recording_id".to_string()],
                vec![],
                "l.rowid",
                "NULL",
                true,
            ),
            "shortlist" => (
                "FROM catalog_listings l JOIN recordings r ON l.recording_id=r.id",
                vec!["r.disposition='shortlist' AND l.id=l.recording_id".to_string()],
                vec![],
                "l.rowid",
                "NULL",
                true,
            ),
            "collection" => {
                let ordered: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM playlist_ordered_collections WHERE collection_id=?1)",
                    [collection_id.unwrap_or("")], |r| r.get(0),
                ).unwrap_or(false);
                if ordered {
                    (
                        "FROM playlist_ordered_entries e JOIN catalog_listings l ON l.id=e.listing_id JOIN recordings r ON r.id=l.recording_id",
                        vec!["e.collection_id=?".to_string()],
                        vec![collection_id.unwrap_or("").to_owned().into()],
                        "e.position,e.rowid",
                        "e.position",
                        false,
                    )
                } else {
                    (
                        "FROM (SELECT m.collection_id,m.recording_id,m.position,ROW_NUMBER() OVER(ORDER BY m.position,l.rowid)-1 AS playlist_position FROM collection_members m JOIN catalog_listings l ON l.recording_id=m.recording_id AND l.id=l.recording_id WHERE m.collection_id=?) m JOIN catalog_listings l ON l.recording_id=m.recording_id JOIN recordings r ON r.id=l.recording_id",
                        vec!["l.id=l.recording_id".to_string()],
                        vec![collection_id.unwrap_or("").to_owned().into()],
                        "m.position,l.rowid",
                        "m.playlist_position",
                        false,
                    )
                }
            }
            "library" => (
                "FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id",
                vec!["l.id=l.recording_id AND l.in_library=1".to_string()],
                vec![],
                "l.rowid",
                "NULL",
                true,
            ),
            _ => (
                "FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id",
                vec!["1=1".to_string()],
                vec![],
                "l.rowid",
                "NULL",
                true,
            ),
        };
        let track_json = if has_page_effective_metadata {
            let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
            format!(
                "CASE WHEN l.sort_projection_revision={revision} AND l.sort_metadata_projection IS NOT NULL THEN l.sort_metadata_projection ELSE COALESCE((SELECT metadata FROM page_effective_metadata p WHERE p.id=l.id),CASE WHEN r.manual_metadata='{{}}' THEN l.metadata ELSE json_patch(l.metadata,COALESCE(r.manual_metadata,'{{}}')) END) END"
            )
        } else if has_local_assets {
            let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
            format!(
                "COALESCE(CASE WHEN l.sort_projection_revision={revision} THEN l.sort_metadata_projection END,CASE WHEN r.manual_metadata='{{}}' THEN l.metadata ELSE json_patch(l.metadata,COALESCE(r.manual_metadata,'{{}}')) END)"
            )
        } else {
            "CASE WHEN r.manual_metadata='{}' THEN l.metadata ELSE json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')) END".to_owned()
        };
        let duration_exact = crate::track_sort_sql::duration_expression(&track_json);
        let duration_filter = format!("CAST({duration_exact} AS INTEGER)");
        if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
            conditions.push(format!("(instr(unicode_lower(COALESCE(json_extract({track_json},'$.title'),'')),?)>0 OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.artist'),'')),?)>0 OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.album'),'')),?)>0 OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.version'),'')),?)>0 OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.genre'),'')),?)>0)"));
            let q = crate::track_sort_text::lowercase(query.trim());
            values.extend([
                q.clone().into(),
                q.clone().into(),
                q.clone().into(),
                q.clone().into(),
                q.into(),
            ]);
        }
        if let Some(min) = min_duration_seconds {
            conditions.push(format!("{duration_filter}>=?").into());
            values.push((min as i64).into());
        }
        if let Some(max) = max_duration_seconds {
            conditions.push(format!("{duration_filter}<=?").into());
            values.push((max as i64).into());
        }
        if let Some(value) = availability.filter(|v| *v != "all") {
            conditions.push(match value {
                "local" => {
                    "EXISTS(SELECT 1 FROM page_local_recordings a WHERE a.recording_id=r.id)".into()
                }
                "not-local" => {
                    "NOT EXISTS(SELECT 1 FROM page_local_recordings a WHERE a.recording_id=r.id)"
                        .into()
                }
                _ => "1=0".into(),
            });
        }
        if let Some(value) = provider.filter(|v| *v != "all") {
            match value {
                "local" => conditions.push(
                    "EXISTS(SELECT 1 FROM page_local_recordings a WHERE a.recording_id=r.id)"
                        .into(),
                ),
                "personal" => {}
                _ => {
                    conditions.push(format!("lower(json_extract({track_json},'$.provider'))=?"));
                    values.push(value.to_lowercase().into());
                }
            }
        }
        let where_sql = conditions.join(" AND ");
        // In this view and filter subset, membership and every aggregate input
        // belong to listings. The enforced recording foreign key makes the
        // recordings join redundant. Keep the page query's relation unchanged.
        let metrics_from_sql = if view == "library"
            && query.is_none_or(|value| value.trim().is_empty())
            && min_duration_seconds.is_none()
            && max_duration_seconds.is_none()
            && availability.is_none_or(|value| value == "all")
            && provider.is_none_or(|value| matches!(value, "all" | "personal"))
        {
            "FROM catalog_listings l"
        } else {
            from_sql
        };
        #[cfg(test)]
        let count_started = profile_enabled.then(std::time::Instant::now);
        let availability_revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
        let check_bpm_projection_coverage =
            view == "library" && sort_field == Some("bpm") && !has_page_effective_metadata;
        let (total, combined_availability_mask, bpm_projection_coverage): (
            usize,
            Option<i64>,
            Option<i64>,
        ) = if include_sort_availability {
            // Count and global availability share the exact same filtered
            // relation. A single aggregate scan avoids re-running the
            // filters once for COUNT, again for a pending EXISTS, and a
            // third time for the availability OR.
            let mask_expr = if has_page_effective_metadata {
                format!(
                    "CASE WHEN l.sort_availability_revision={availability_revision} AND l.sort_availability_mask IS NOT NULL THEN l.sort_availability_mask ELSE (SELECT p.sort_mask FROM page_effective_metadata p WHERE p.id=l.id) END"
                )
            } else {
                format!(
                    "CASE WHEN l.sort_availability_revision={availability_revision} THEN l.sort_availability_mask END"
                )
            };
            let projection_coverage_expr = if check_bpm_projection_coverage {
                format!(
                    ",MIN(CASE WHEN l.sort_projection_revision={availability_revision} AND l.sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END)"
                )
            } else {
                String::new()
            };
            let metrics_sql = format!(
                "SELECT COUNT(*),sort_availability_or({mask_expr}){projection_coverage_expr} {metrics_from_sql} WHERE {where_sql}"
            );
            #[cfg(test)]
            let explain_enabled = std::env::var_os("BURRATA_AVAILABILITY_EXPLAIN").is_some();
            #[cfg(test)]
            if explain_enabled {
                let explain_sql = format!("EXPLAIN QUERY PLAN {metrics_sql}");
                let mut statement = conn.prepare(&explain_sql).map_err(err)?;
                let plan = statement
                    .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                        row.get::<_, String>(3)
                    })
                    .map_err(err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(err)?;
                eprintln!(
                    "BURRATA_AVAILABILITY_METRICS_PLAN {}",
                    serde_json::json!({"hasLocalAssets":has_local_assets,"hasPageEffectiveMetadata":has_page_effective_metadata,"plan":plan})
                );
            }
            conn.query_row(
                &metrics_sql,
                rusqlite::params_from_iter(values.iter()),
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        if check_bpm_projection_coverage {
                            row.get(2)?
                        } else {
                            None
                        },
                    ))
                },
            )
            .map_err(err)?
        } else {
            if check_bpm_projection_coverage {
                let count_sql = format!(
                    "SELECT COUNT(*),MIN(CASE WHEN l.sort_projection_revision={availability_revision} AND l.sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END) {metrics_from_sql} WHERE {where_sql}"
                );
                conn.query_row(
                    &count_sql,
                    rusqlite::params_from_iter(values.iter()),
                    |row| Ok((row.get(0)?, None, row.get(1)?)),
                )
                .map_err(err)?
            } else {
                let count_sql = format!("SELECT COUNT(*) {metrics_from_sql} WHERE {where_sql}");
                let total: usize = conn
                    .query_row(
                        &count_sql,
                        rusqlite::params_from_iter(values.iter()),
                        |row| row.get(0),
                    )
                    .map_err(err)?;
                (total, None, None)
            }
        };
        #[cfg(test)]
        let count_micros = count_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        let sort_availability_started = profile_enabled.then(std::time::Instant::now);
        let sort_availability = if include_sort_availability {
            let aggregate_mask = combined_availability_mask;
            if crate::track_sort_sql::aggregate_result_is_pending(aggregate_mask) {
                Some(SortAvailabilityState::Pending)
            } else {
                let fields =
                    crate::track_sort_sql::unpack_available_fields(aggregate_mask.unwrap_or(0));
                #[cfg(test)]
                if std::env::var_os("BURRATA_AVAILABILITY_PROFILE").is_some() {
                    eprintln!(
                        "BURRATA_AVAILABILITY_PROFILE {}",
                        serde_json::json!({"hasLocalAssets":has_local_assets,"hasPageEffectiveMetadata":has_page_effective_metadata,"metricsMicros":count_micros,"pending":false})
                    );
                }
                let mut result =
                    std::collections::BTreeMap::from([("sourceOrder".to_string(), true)]);
                result.extend(
                    [
                        "title",
                        "artist",
                        "album",
                        "duration",
                        "releaseDate",
                        "dateAdded",
                        "bpm",
                        "key",
                        "plays",
                        "views",
                    ]
                    .iter()
                    .cloned()
                    .zip(fields)
                    .map(|(field, available)| (field.to_string(), available)),
                );
                Some(SortAvailabilityState::Ready(result))
            }
        } else {
            None
        };
        #[cfg(test)]
        let sort_availability_micros =
            sort_availability_started.map_or(0, |started| started.elapsed().as_micros());
        let sort_expr =
            sort_field.and_then(|field| crate::track_sort_sql::expression(field, &track_json));
        let direction = if direction == Some("desc") {
            "DESC"
        } else {
            "ASC"
        };
        let legacy_direction = if legacy_desc { "DESC" } else { "ASC" };
        let reverse_legacy_direction = if legacy_desc { "ASC" } else { "DESC" };
        let stable_source_order = base_order
            .split(',')
            .map(|expr| format!("{expr} {legacy_direction}"))
            .collect::<Vec<_>>()
            .join(",");
        let reverse_source_order = base_order
            .split(',')
            .map(|expr| format!("{expr} {reverse_legacy_direction}"))
            .collect::<Vec<_>>()
            .join(",");
        let source_order = if direction == "DESC" {
            reverse_source_order
        } else {
            stable_source_order.clone()
        };
        let order = if sort_field.unwrap_or("sourceOrder") == "sourceOrder" {
            source_order
        } else {
            sort_expr.as_ref().map(|expr| format!("CASE WHEN {expr} IS NULL THEN 1 ELSE 0 END ASC,{expr} {direction},{stable_source_order}")).unwrap_or(source_order)
        };
        let limit = if hydrate_tracks {
            limit.clamp(1, 1000)
        } else {
            limit
        };
        let limit = i64::try_from(limit).map_err(|_| "Track prefix is too large")?;
        let offset = i64::try_from(offset).map_err(|_| "Track page offset is too large")?;
        let metadata_projection = if hydrate_tracks { "l.metadata" } else { "NULL" };
        let use_bpm_projection_index = should_use_bpm_projection_index(
            view,
            sort_field,
            has_page_effective_metadata,
            bpm_projection_coverage,
        );
        let page_sql = if use_bpm_projection_index {
            indexed_bpm_page_sql(
                metadata_projection,
                &where_sql,
                direction,
                legacy_direction,
                availability_revision,
            )
        } else if let Some(sort_expr) =
            sort_expr.filter(|_| sort_field.unwrap_or("sourceOrder") != "sourceOrder")
        {
            // SQLite otherwise re-expands the effective JSON and its sort
            // expression while building both the null-order key and the value
            // key. Materialize each filtered row's effective key once, then
            // retain the exact legacy tie-break columns explicitly.
            let source_terms = base_order.split(',').map(str::trim).collect::<Vec<_>>();
            let tie_projection = source_terms
                .iter()
                .enumerate()
                .map(|(index, expression)| format!("{expression} AS tie_{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let tie_order = source_terms
                .iter()
                .enumerate()
                .map(|(index, _)| format!("tie_{index} {legacy_direction}"))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "WITH page_candidates AS MATERIALIZED (SELECT l.id,{metadata_projection} AS page_metadata,{position_expr} AS playlist_position,{sort_expr} AS sort_value,{tie_projection} {from_sql} WHERE {where_sql}) SELECT id,page_metadata,playlist_position FROM page_candidates ORDER BY (sort_value IS NULL) ASC,sort_value {direction},{tie_order} LIMIT ? OFFSET ?"
            )
        } else {
            format!(
                "SELECT l.id,{metadata_projection},{position_expr} {from_sql} WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?"
            )
        };
        #[cfg(test)]
        let page_sql_started = profile_enabled.then(std::time::Instant::now);
        let mut page_stmt = conn.prepare(&page_sql).map_err(err)?;
        values.push(limit.into());
        values.push(offset.into());
        let raw = page_stmt
            .query_map(rusqlite::params_from_iter(values.iter()), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2).unwrap_or(None),
                ))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        #[cfg(test)]
        let page_sql_micros = page_sql_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        let page_hydration_started = profile_enabled.then(std::time::Instant::now);
        #[cfg(test)]
        let batch_hydration =
            std::env::var("BURRATA_PAGE_HYDRATION_MODE").ok().as_deref() != Some("individual");
        #[cfg(not(test))]
        let batch_hydration = true;
        let mut hydrated = if hydrate_tracks && batch_hydration {
            let tracks = raw
                .iter()
                .map(|(_, metadata, _)| {
                    decode(
                        metadata
                            .clone()
                            .ok_or("Missing track metadata in hydrated page")?,
                    )
                })
                .collect::<Result<Vec<Track>>>()?;
            Some(hydrate_page_tracks(&conn, tracks)?.into_iter())
        } else {
            None
        };
        let rows = raw
            .into_iter()
            .map(|(listing_id, raw, position)| {
                let playlist_position = position.map(|position| position + 1);
                let track = if hydrate_tracks {
                    let mut track = if let Some(hydrated) = hydrated.as_mut() {
                        hydrated.next().ok_or("Missing hydrated page occurrence")?
                    } else {
                        let raw = raw.ok_or("Missing track metadata in hydrated page")?;
                        hydrate(&conn, decode(raw)?)?
                    };
                    if let Some(position) = playlist_position {
                        if !track.provenance.is_object() {
                            track.provenance = serde_json::json!({});
                        }
                        track.provenance["playlistPosition"] = serde_json::json!(position);
                    }
                    Some(track)
                } else {
                    None
                };
                Ok(TrackPageRow {
                    listing_id,
                    playlist_position,
                    track,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        #[cfg(test)]
        let page_hydration_micros =
            page_hydration_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        if profile_enabled {
            let total_micros = profile_started.map_or(0, |started| started.elapsed().as_micros());
            eprintln!(
                "BURRATA_AVAILABILITY_STAGE_PROFILE {}",
                serde_json::json!({
                    "hasLocalAssets": has_local_assets,
                    "hasPageEffectiveMetadata": has_page_effective_metadata,
                    "useBpmProjectionIndex": use_bpm_projection_index,
                    "projectionPendingCheckMicros": projection_pending_micros,
                    "assetListings": asset_listing_count,
                    "pageRows": rows.len(),
                    "lockWaitMicros": lock_wait_micros,
                    "assetDiscoveryMicros": asset_discovery_micros,
                    "assetHydrationAndMaskMicros": asset_hydration_micros,
                    "assetDecodeAndHydrateMicros": asset_decode_hydrate_micros,
                    "assetMaskSerializeAndInsertMicros": asset_mask_serialize_insert_micros,
                    "localPresenceMicros": local_presence_micros,
                    "countMicros": if include_sort_availability { 0 } else { count_micros },
                    "countAndAvailabilityMicros": if include_sort_availability { count_micros } else { 0 },
                    "sortAvailabilityMicros": sort_availability_micros,
                    "pageSqlMicros": page_sql_micros,
                    "pageHydrationMicros": page_hydration_micros,
                    "batchHydration": hydrate_tracks && batch_hydration,
                    "totalMicros": total_micros,
                })
            );
        }
        Ok((rows, total, sort_availability, false))
    }
    /// Return tracks in requested ID order, including repeated IDs.
    pub fn get_tracks(&self, ids: &[String]) -> Result<Vec<Track>> {
        let conn = self.lock_connection()?;
        let mut by_id = std::collections::HashMap::new();
        for chunk in ids.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT metadata FROM catalog_listings WHERE id IN ({placeholders})"
                ))
                .map_err(err)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                    r.get::<_, String>(0)
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            for raw in rows {
                let track: Track = decode(raw)?;
                by_id.insert(track.id.clone(), hydrate(&conn, track)?);
            }
        }
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
    }
    /// Find only canonical imported listings matching the requested exact source URLs.
    /// This is used by playlist metadata repair to avoid hydrating the whole library.
    pub fn list_import_tracks_by_source_urls(&self, source_urls: &[String]) -> Result<Vec<Track>> {
        let conn = self.lock_connection()?;
        let mut urls = source_urls.to_vec();
        urls.sort();
        urls.dedup();
        let mut tracks = Vec::new();
        for chunk in urls.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT l.metadata FROM catalog_listings l
                 WHERE l.provider='import' AND l.id=l.recording_id
                   AND l.source_url IN ({placeholders}) ORDER BY l.rowid DESC"
            );
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            for raw in rows {
                tracks.push(hydrate(&conn, decode(raw)?)?);
            }
        }
        Ok(tracks)
    }
    /// Backfill Spotify durations only when the local value is still genuinely missing.
    /// The transaction and SQL guard protect manual edits made after the provider request began.
    pub fn missing_spotify_duration_ids(
        &self,
        after_provider_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let conn = self.lock_connection()?;
        let limit = limit.clamp(1, 100);
        let mut stmt = conn.prepare(
            "SELECT l.provider_id FROM catalog_listings l
             JOIN recordings r ON r.id=l.recording_id
             WHERE l.provider='spotify' AND (?1 IS NULL OR l.provider_id>?1)
               AND length(l.provider_id)=22 AND l.provider_id NOT GLOB '*[^A-Za-z0-9]*'
               AND (json_type(l.metadata,'$.durationSeconds') IS NULL OR json_type(l.metadata,'$.durationSeconds')='null' OR NOT (json_type(l.metadata,'$.durationSeconds') IN ('integer','real') AND CAST(json_extract(l.metadata,'$.durationSeconds') AS REAL)>0))
               AND json_type(r.manual_metadata,'$.durationSeconds') IS NULL
               AND json_type(r.manual_metadata,'$.playbackRange') IS NULL
               AND json_type(l.metadata,'$.playbackRange') IS NULL
               AND COALESCE(json_array_length(json_extract(l.metadata,'$.chapters')),0)=0
               AND COALESCE(json_array_length(json_extract(l.metadata,'$.provenance.chapters')),0)=0
               AND COALESCE(json_array_length(json_extract(r.manual_metadata,'$.chapters')),0)=0
               AND COALESCE(json_array_length(json_extract(r.manual_metadata,'$.provenance.chapters')),0)=0
               AND json_extract(r.manual_metadata,'$.provenance.manualFields.durationSeconds') IS NOT 1
               AND json_extract(l.metadata,'$.provenance.manualFields.durationSeconds') IS NOT 1
             ORDER BY l.provider_id LIMIT ?2",
        ).map_err(err)?;
        let rows = stmt
            .query_map(params![after_provider_id, limit as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)
    }

    pub fn fill_missing_spotify_durations(&self, durations: &[(String, f64)]) -> Result<usize> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        let mut changed = 0;
        for (provider_id, seconds) in durations {
            if provider_id.len() != 22
                || !provider_id.bytes().all(|b| b.is_ascii_alphanumeric())
                || !seconds.is_finite()
                || *seconds <= 0.0
            {
                continue;
            }
            let found: Option<(String, String, String)> = tx.query_row(
                "SELECT l.id,l.metadata,r.manual_metadata FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.provider='spotify' AND l.provider_id=?1",
                [provider_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional().map_err(err)?;
            let Some((id, raw, manual_raw)) = found else {
                continue;
            };
            let manual: serde_json::Value = decode(manual_raw)?;
            if manual.get("durationSeconds").is_some() {
                continue;
            }
            if manual
                .pointer("/provenance/manualFields/durationSeconds")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                continue;
            }
            if manual.get("playbackRange").is_some()
                || manual
                    .get("chapters")
                    .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
                || manual
                    .pointer("/provenance/chapters")
                    .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            {
                continue;
            }
            let mut metadata: serde_json::Value = decode(raw)?;
            if metadata
                .get("durationSeconds")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|seconds| seconds.is_finite() && seconds > 0.0)
            {
                continue;
            }
            if !metadata
                .get("playbackRange")
                .is_none_or(serde_json::Value::is_null)
            {
                continue;
            }
            if metadata
                .get("chapters")
                .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
                || metadata
                    .pointer("/provenance/chapters")
                    .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            {
                continue;
            }
            if metadata
                .pointer("/provenance/manualFields/durationSeconds")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                continue;
            }
            metadata["durationSeconds"] = serde_json::json!(seconds);
            changed += tx.execute(
                "UPDATE catalog_listings SET metadata=?1,updated_at=?2 WHERE id=?3 AND provider='spotify' AND provider_id=?4 AND (json_type(metadata,'$.durationSeconds') IS NULL OR json_type(metadata,'$.durationSeconds')='null' OR NOT (json_type(metadata,'$.durationSeconds') IN ('integer','real') AND CAST(json_extract(metadata,'$.durationSeconds') AS REAL)>0))",
                params![encode(&metadata)?, chrono::Utc::now().to_rfc3339(), id, provider_id],
            ).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(changed)
    }
    /// Search the saved library in SQLite, returning only the requested stable page.
    /// Matching covers the library fields used by discovery and MCP library search.
    pub fn search_library_tracks(
        &self,
        terms: &[String],
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Track>, usize)> {
        let conn = self.lock_connection()?;
        let mut filters = Vec::new();
        let mut values = Vec::<rusqlite::types::Value>::new();
        let haystack = "lower(COALESCE(json_extract(l.metadata,'$.artist'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.title'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.album'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.version'),'' ) || ' ' || COALESCE(json_extract(l.metadata,'$.remixers'),'' ) || ' ' || COALESCE(json_quote(json_extract(l.metadata,'$.provenance.remixers')),'null') || ' ' || COALESCE(json_quote(json_extract(l.metadata,'$.provenance.artistCredits')),'null'))";
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
            let mut stmt = conn.prepare("SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1 ORDER BY l.rowid DESC").map_err(err)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            let mut matches = Vec::new();
            for raw in rows {
                let track: Track = decode(raw.clone())?;
                let haystack = format!(
                    "{} {} {} {} {} {} {}",
                    track.artist,
                    track.title,
                    track.album.as_deref().unwrap_or(""),
                    track.version.as_deref().unwrap_or(""),
                    track
                        .remixers
                        .as_ref()
                        .map(|names| names.join(" "))
                        .unwrap_or_default(),
                    track.provenance["remixers"].to_string(),
                    track.provenance["artistCredits"].to_string()
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
                "SELECT COUNT(*) FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1{where_sql}"
            );
            let count: i64 = conn
                .query_row(
                    &count_sql,
                    rusqlite::params_from_iter(values.iter()),
                    |row| row.get(0),
                )
                .map_err(err)?;
            let sql = format!(
                "SELECT l.metadata FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1{where_sql} ORDER BY l.rowid DESC LIMIT ? OFFSET ?"
            );
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
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, track)?;
        if value {
            promote_track(&tx, &track.id)?;
        }
        tx.execute(
            "UPDATE recordings SET favorite=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![value, track.id],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    }
    pub fn set_disposition(&self, track: &Track, value: Option<&str>) -> Result<()> {
        if !matches!(value, None | Some("shortlist") | Some("reject")) {
            return Err("Disposition must be shortlist, reject, or empty".into());
        }
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, track)?;
        if value == Some("shortlist") {
            promote_track(&tx, &track.id)?;
        }
        tx.execute(
            "UPDATE recordings SET disposition=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![value, track.id],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
        conn.execute(
            "INSERT INTO collections(id,name) VALUES(?1,'Downloads') ON CONFLICT(id) DO UPDATE SET name='Downloads',deleted_at=NULL",
            [DOWNLOADS_COLLECTION_ID],
        )
        .map_err(err)?;
        self.active_collection(&conn, DOWNLOADS_COLLECTION_ID)
    }
    pub fn add_to_collection(&self, collection_id: &str, tracks: &[Track]) -> Result<()> {
        let mut conn = self.lock_connection()?;
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
            promote_track(&tx, &track.id)?;
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
    /// Append explicit playlist occurrences while retaining set membership for legacy readers.
    /// Favorites and Downloads continue to use the idempotent membership operation.
    pub fn append_playlist_entries(&self, collection_id: &str, tracks: &[Track]) -> Result<()> {
        if tracks.len() > 100_000 {
            return Err("A playlist append is limited to 100000 occurrences".into());
        }
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        let active: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NULL)",
                [collection_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if !active {
            return Err("Collection not found or deleted".into());
        }
        let preferences: Option<String> = tx
            .query_row(
                "SELECT value FROM settings WHERE key='playlistPreferences.v1'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        let favorites = preferences.map(decode::<serde_json::Value>).transpose()?;
        if collection_id == DOWNLOADS_COLLECTION_ID
            || ["favorites", "history"].contains(&collection_id)
            || favorites
                .as_ref()
                .and_then(|value| value.get("favoritesPlaylistId"))
                .and_then(serde_json::Value::as_str)
                == Some(collection_id)
        {
            return Err("Occurrence append requires a regular named playlist".into());
        }
        if tracks.is_empty() {
            return Ok(());
        }
        for track in tracks {
            ensure_track(&tx, track)?;
            promote_track(&tx, &track.id)?;
        }
        let initialized = tx
            .execute(
                "INSERT OR IGNORE INTO playlist_ordered_collections(collection_id) VALUES(?1)",
                [collection_id],
            )
            .map_err(err)?;
        if initialized > 0 {
            // Match the existing legacy read order once; subsequent appends never rewrite entries.
            tx.execute(
                "INSERT INTO playlist_ordered_entries(collection_id,position,listing_id)
                SELECT ?1,ROW_NUMBER() OVER (ORDER BY m.position,l.rowid)-1,l.id
                FROM collection_members m JOIN catalog_listings l ON l.recording_id=m.recording_id
                WHERE m.collection_id=?1 AND l.id=l.recording_id",
                [collection_id],
            )
            .map_err(err)?;
        }
        let mut position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position),-1) FROM playlist_ordered_entries WHERE collection_id=?1", [collection_id], |row| row.get(0),
        ).map_err(err)?;
        for track in tracks {
            position = position
                .checked_add(1)
                .ok_or("Playlist position exceeded the safe limit")?;
            tx.execute("INSERT INTO playlist_ordered_entries(collection_id,position,listing_id) VALUES(?1,?2,?3)", params![collection_id,position,track.id]).map_err(err)?;
            tx.execute(
                "INSERT OR IGNORE INTO collection_members(collection_id,recording_id,position)
                SELECT ?1,recording_id,?2 FROM catalog_listings WHERE id=?3",
                params![collection_id, position, track.id],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    /// Add a managed audio asset to the stable system Downloads collection.
    /// Repeated calls (including publication retries) leave membership unchanged.
    pub fn add_asset_to_downloads(&self, asset: &LocalAsset) -> Result<Collection> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, &asset.track)?;
        promote_track(&tx, &asset.track.id)?;
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
        let mut added = 0;
        let mut before_rowid = None;
        loop {
            let (assets, next_before_rowid) =
                self.managed_download_candidates_page(before_rowid, 500)?;
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
            let Some(next_before_rowid) = next_before_rowid else {
                break;
            };
            before_rowid = Some(next_before_rowid);
        }
        Ok(added)
    }

    /// Stream asset payloads in stable rowid order, decode the small provenance
    /// inputs needed to identify possible managed publications, and hydrate only
    /// those candidates. Asset and manual-metadata decode errors still fail the
    /// scan; unrelated selected analysis/autotag payloads are no longer touched.
    /// Persisted analysis blobs are expected to be valid because Store writes
    /// them through typed serialization; legacy corruption there is not a reason
    /// to hydrate all unrelated recordings during this maintenance check.
    fn managed_download_candidates_page(
        &self,
        before_rowid: Option<i64>,
        limit: usize,
    ) -> Result<(Vec<LocalAsset>, Option<i64>)> {
        let conn = self.lock_connection()?;
        let limit = limit.clamp(1, 500);
        let raw_rows = {
            let sql = if before_rowid.is_some() {
                "SELECT a.rowid,a.payload FROM local_assets a
                 WHERE a.rowid<?1 ORDER BY a.rowid DESC LIMIT ?2"
            } else {
                "SELECT a.rowid,a.payload FROM local_assets a
                 ORDER BY a.rowid DESC LIMIT ?1"
            };
            let mut stmt = conn.prepare(sql).map_err(err)?;
            let collected = if let Some(before_rowid) = before_rowid {
                stmt.query_map(params![before_rowid, limit as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
            } else {
                stmt.query_map([limit as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
            };
            collected
        };
        let Some((last_rowid, _)) = raw_rows.last() else {
            return Ok((Vec::new(), None));
        };
        let next_before_rowid = (raw_rows.len() == limit).then_some(*last_rowid);
        let mut decoded_assets = raw_rows
            .into_iter()
            .map(|(rowid, payload)| Ok((rowid, decode::<LocalAsset>(payload)?)))
            .collect::<Result<Vec<_>>>()?;
        let listing_ids = decoded_assets
            .iter()
            .map(|(_, asset)| asset.track.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let manual_by_listing = if listing_ids.is_empty() {
            std::collections::HashMap::new()
        } else {
            let placeholders = (1..=listing_ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT l.id,r.manual_metadata FROM catalog_listings l
                 JOIN recordings r ON r.id=l.recording_id WHERE l.id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let mapped = stmt
                .query_map(rusqlite::params_from_iter(listing_ids.iter()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(err)?;
            let rows = mapped
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            rows.into_iter()
                .map(|(listing_id, raw_manual)| {
                    Ok((listing_id, decode::<serde_json::Value>(raw_manual)?))
                })
                .collect::<Result<std::collections::HashMap<_, _>>>()?
        };
        let mut candidates = Vec::new();
        for (_, asset) in decoded_assets.drain(..) {
            let Some(manual_metadata) = manual_by_listing.get(&asset.track.id) else {
                // Preserve the legacy missing-recording failure path.
                hydrate_asset(&conn, asset)?;
                continue;
            };
            if !managed_publication_marker_is_effective(&asset.track, manual_metadata) {
                continue;
            }
            // Keep the original full hydration and final marker check for every
            // candidate so preferred-asset and manual-field semantics stay intact.
            let asset = hydrate_asset(&conn, asset)?;
            if asset.track.provenance["publicationJobId"]
                .as_str()
                .is_some_and(|job_id| !job_id.is_empty())
            {
                candidates.push(asset);
            }
        }
        Ok((candidates, next_before_rowid))
    }
    /// Replace ordered playlist entries, preserving duplicates in the sidecar while keeping
    /// legacy collection membership populated for older queries.
    pub fn replace_playlist_entries(&self, collection_id: &str, tracks: &[Track]) -> Result<()> {
        let mut conn = self.lock_connection()?;
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
            promote_track(&tx, &track.id)?;
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
        let mut conn = self.lock_connection()?;
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
        let mut conn = self.lock_connection()?;
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
            let merged_membership: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM catalog_listings WHERE recording_id IN (?1,?2) AND in_library=1)",
                    params![primary, other],
                    |row| row.get(0),
                )
                .map_err(err)?;
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
            if merged_membership {
                tx.execute(
                    "UPDATE catalog_listings SET in_library=1 WHERE id=?1 AND recording_id=?1",
                    [&primary],
                )
                .map_err(err)?;
            }
            tx.execute(
                "UPDATE local_assets SET recording_id=?1 WHERE recording_id=?2",
                params![primary, other],
            )
            .map_err(err)?;
            // A source confirmed for the duplicate identity must not become
            // a remembered source for the surviving recording implicitly.
            tx.execute(
                "DELETE FROM verified_playback_sources WHERE recording_id=?1",
                [&other],
            )
            .map_err(err)?;
            // Preserve original work/recording rows as identity history; incoming refreshes do not undo the merge.
        }
        tx.commit().map_err(err)
    }
    pub fn save_chart(&self, chart: &Chart) -> Result<Chart> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        let mut saved = chart.clone();
        saved.id = uuid::Uuid::new_v4().to_string();
        for track in &saved.tracks {
            upsert_track(&tx, track, false)?;
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
        let mut conn = self.lock_connection()?;
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
            upsert_track(&tx, track, true)?;
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
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT b.payload FROM browser_imports b JOIN chart_snapshots s ON s.id=b.snapshot_id JOIN browser_sources src ON src.source_url=s.source_url AND src.provider=s.provider JOIN collections c ON c.id=src.collection_id WHERE c.deleted_at IS NULL ORDER BY b.rowid DESC LIMIT 50")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| decode(r.map_err(err)?)).collect()
    }
    pub fn load_browser_import(&self, id: &str) -> Result<Chart> {
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
            let conn = self.lock_connection()?;
            ensure_track(&conn, &job.track)?;
        }
        self.lock_connection()?.execute("INSERT INTO download_jobs(id,state,payload) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload", params![job.id,job.state,encode(job)?]).map_err(err)?;
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
    pub fn get_job_if_exists(&self, id: &str) -> Result<Option<DownloadJob>> {
        let raw = self
            .connection
            .lock()
            .map_err(err)?
            .query_row(
                "SELECT payload FROM download_jobs WHERE id=?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(err)?;
        raw.map(decode).transpose()
    }
    pub fn list_jobs(&self) -> Result<Vec<DownloadJob>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT payload FROM download_jobs ORDER BY rowid DESC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| decode(r.map_err(err)?)).collect()
    }
    pub fn save_asset(&self, asset: &LocalAsset) -> Result<()> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        ensure_track(&tx, &asset.track)?;
        tx.execute("INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?1,(SELECT recording_id FROM catalog_listings WHERE id=?2),?3,?4,?5) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,path=excluded.path,hash=excluded.hash", params![asset.id,asset.track.id,asset.path,asset.hash,encode(asset)?]).map_err(err)?;
        promote_track(&tx, &asset.track.id)?;
        tx.commit().map_err(err)
    }
    pub(crate) fn save_flexible_excerpt_asset(
        &self,
        asset: &LocalAsset,
        request_fingerprint: &str,
    ) -> Result<()> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        let settings_payload: Option<String> = tx
            .query_row(
                "SELECT value FROM settings WHERE key='application'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        let settings = settings_payload
            .map(decode::<Settings>)
            .transpose()?
            .unwrap_or_default();
        if settings.matching_level != crate::recording_match::MatchLevel::Flexible
            || !settings.background_source_preparation
            || !settings.streaming_fallback_enabled
            || crate::compilation_excerpt::request_fingerprint(
                &asset.track,
                crate::recording_match::MatchLevel::Flexible,
            ) != request_fingerprint
            || asset.track.provenance["automaticExcerpt"]["requestFingerprint"].as_str()
                != Some(request_fingerprint)
            || !crate::compilation_excerpt::asset_proof_is_eligible(
                &asset.track,
                &asset.track.provenance,
                crate::recording_match::MatchLevel::Flexible,
            )
        {
            return Err("Flexible excerpt request changed before asset publication".into());
        }
        let current_metadata: Option<(String, bool, Option<String>, String)> = tx.query_row(
            "SELECT l.metadata,r.favorite,r.disposition,r.manual_metadata FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id=?1 AND l.provider=?2 AND l.provider_id=?3",
            params![asset.track.id, asset.track.provider, asset.track.provider_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).optional().map_err(err)?;
        let current_track = current_metadata
            .map(|(metadata, favorite, disposition, manual)| {
                let track = decode::<Track>(metadata)?;
                apply_hydration_metadata(track, (favorite, disposition, manual))
            })
            .transpose()?
            .map(|(track, _)| track);
        if current_track.is_none_or(|track| {
            crate::compilation_excerpt::request_fingerprint(
                &track,
                crate::recording_match::MatchLevel::Flexible,
            ) != request_fingerprint
        }) {
            return Err("Catalog recording changed before excerpt publication".into());
        }
        ensure_track(&tx, &asset.track)?;
        tx.execute("INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?1,(SELECT recording_id FROM catalog_listings WHERE id=?2),?3,?4,?5) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,path=excluded.path,hash=excluded.hash", params![asset.id,asset.track.id,asset.path,asset.hash,encode(asset)?]).map_err(err)?;
        tx.commit().map_err(err)
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
    pub fn get_autotag_record(
        &self,
        asset_id: &str,
        file_hash: &str,
        version: &str,
    ) -> Result<Option<serde_json::Value>> {
        let raw = self
            .connection
            .lock()
            .map_err(err)?
            .query_row(
                "SELECT payload FROM asset_autotags WHERE asset_id=?1 AND file_hash=?2 AND version=?3",
                params![asset_id, file_hash, version],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(err)?;
        raw.map(decode).transpose()
    }
    pub fn save_autotagged_asset(
        &self,
        asset: &LocalAsset,
        prior_hash: &str,
        version: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let mut conn = self.lock_connection()?;
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
        let mut conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
    pub fn list_analysis_records_for_assets(
        &self,
        asset_ids: &[String],
    ) -> Result<Vec<serde_json::Value>> {
        if asset_ids.len() > 200 {
            return Err("At most 200 asset IDs may be queried at once".into());
        }
        let mut ids = asset_ids.iter().map(String::as_str).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (1..=ids.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT asset_id,file_hash,version,status,payload FROM asset_analyses WHERE asset_id IN ({placeholders}) ORDER BY updated_at DESC"
        );
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(ids), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(err)?;
        rows.map(|row| { let (asset_id,input_hash,version,status,payload)=row.map_err(err)?; Ok(serde_json::json!({"assetId":asset_id,"inputHash":input_hash,"version":version,"status":status,"payload":decode::<serde_json::Value>(payload)?})) }).collect()
    }
    pub fn find_asset_by_hash(&self, hash: &str) -> Result<Option<LocalAsset>> {
        let conn = self.lock_connection()?;
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
        let conn = self.lock_connection()?;
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
    pub fn get_asset_by_id(&self, id: &str) -> Result<Option<LocalAsset>> {
        let conn = self.lock_connection()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT payload FROM local_assets WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        raw.map(|payload| hydrate_asset(&conn, decode(payload)?))
            .transpose()
    }
    /// Request-scoped exact file occurrences. Two SQL reads per <=200 paths;
    /// preferred recording assets never participate in directory metadata.
    pub fn list_directory_assets_for_paths(&self, paths: &[String]) -> Result<Vec<LocalAsset>> {
        if paths.len() > 100_000 {
            return Err("Directory asset lookup exceeds 100000 paths".into());
        }
        let conn = self.lock_connection()?;
        let mut result = Vec::new();
        for chunk in paths.chunks(200) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; chunk.len()].join(",");
            let mut stmt = conn.prepare(&format!(
                "SELECT a.id,a.path,a.hash,a.payload,a.recording_id,r.favorite,r.disposition,r.manual_metadata FROM local_assets a JOIN recordings r ON r.id=a.recording_id WHERE a.path IN ({placeholders}) AND a.rowid=(SELECT MAX(latest.rowid) FROM local_assets latest WHERE latest.path=a.path)"
            )).map_err(err)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk), |row| {
                    Ok((
                        (
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ),
                        row.get::<_, String>(4)?,
                        (
                            row.get::<_, bool>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, String>(7)?,
                        ),
                    ))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            let mut analyses = std::collections::HashMap::new();
            if !rows.is_empty() {
                let pairs = vec!["(?,?)"; rows.len()].join(",");
                let mut analysis_stmt = conn.prepare(&format!(
                    "WITH wanted(asset_id,file_hash) AS (VALUES {pairs}) SELECT a.asset_id,a.file_hash,a.status,a.version,a.payload FROM wanted w JOIN asset_analyses a ON a.rowid=(SELECT candidate.rowid FROM asset_analyses candidate WHERE candidate.asset_id=w.asset_id AND candidate.file_hash=w.file_hash ORDER BY candidate.updated_at DESC LIMIT 1)"
                )).map_err(err)?;
                analyses = analysis_stmt
                    .query_map(
                        rusqlite::params_from_iter(
                            rows.iter().flat_map(|(asset, _, _)| [&asset.0, &asset.2]),
                        ),
                        |row| {
                            Ok((
                                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                                (
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                    row.get::<_, String>(4)?,
                                ),
                            ))
                        },
                    )
                    .map_err(err)?
                    .collect::<std::result::Result<std::collections::HashMap<_, _>, _>>()
                    .map_err(err)?;
            }
            for (selected, recording_id, values) in rows {
                let mut asset: LocalAsset = decode(selected.3.clone())?;
                asset.id = selected.0.clone();
                asset.path = selected.1.clone();
                asset.hash = selected.2.clone();
                asset.recording_id = recording_id;
                let (track, manual) = apply_hydration_metadata(asset.track, values)?;
                let analysis = analyses.remove(&(selected.0.clone(), selected.2.clone()));
                asset.track = finish_hydration(track, manual, Some((selected, analysis)))?;
                result.push(asset);
            }
        }
        Ok(result)
    }
    pub fn list_assets_for_paths(&self, paths: &[String]) -> Result<Vec<LocalAsset>> {
        if paths.len() > 200 {
            return Err("At most 200 asset paths may be queried at once".into());
        }
        let mut paths = paths
            .iter()
            .map(String::as_str)
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>();
        paths.sort_unstable();
        paths.dedup();
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (1..=paths.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT payload FROM local_assets WHERE path IN ({placeholders}) ORDER BY rowid DESC"
        );
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(paths), |row| {
                row.get::<_, String>(0)
            })
            .map_err(err)?;
        rows.map(|row| hydrate_asset(&conn, decode(row.map_err(err)?)?))
            .collect()
    }
    pub fn list_assets_by_ids(&self, asset_ids: &[String]) -> Result<Vec<LocalAsset>> {
        if asset_ids.len() > 200 {
            return Err("At most 200 asset IDs may be queried at once".into());
        }
        let mut ids = asset_ids.iter().map(String::as_str).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (1..=ids.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT payload FROM local_assets WHERE id IN ({placeholders}) ORDER BY rowid DESC"
        );
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(ids), |row| {
                row.get::<_, String>(0)
            })
            .map_err(err)?;
        rows.map(|row| hydrate_asset(&conn, decode(row.map_err(err)?)?))
            .collect()
    }

    /// Return a lightweight page for exact NFKC artist matching in the frontend.
    pub fn list_asset_artist_summaries_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<AssetArtistSummary>, usize)> {
        let conn = self.lock_connection()?;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_assets", [], |row| row.get(0))
            .map_err(err)?;
        let mut stmt = conn.prepare("SELECT id,json_extract(payload,'$.track.artist') FROM local_assets ORDER BY rowid DESC LIMIT ?1 OFFSET ?2").map_err(err)?;
        let rows = stmt
            .query_map(
                rusqlite::params![limit.clamp(1, 200) as i64, offset as i64],
                |row| {
                    Ok(AssetArtistSummary {
                        id: row.get(0)?,
                        artist: row.get(1)?,
                    })
                },
            )
            .map_err(err)?;
        Ok((
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?,
            total.max(0) as usize,
        ))
    }
    /// Filter local assets by their fully hydrated effective artist before pagination.
    /// The SQL candidate query is a conservative superset over catalog, manual, and sibling
    /// asset artist values; only those candidates are hydrated before exact matching.
    pub fn list_asset_artist_matches_page(
        &self,
        names: &[String],
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<AssetArtistSummary>, usize)> {
        let (assets, total, _) = self.list_artist_local_assets_page(
            names,
            limit,
            offset.min(i64::MAX as usize),
            None,
            Some("sourceOrder"),
            Some("asc"),
            None,
            None,
            Some("all"),
        )?;
        Ok((
            assets
                .into_iter()
                .map(|asset| AssetArtistSummary {
                    id: asset.id,
                    artist: Some(asset.track.artist.trim().to_owned()),
                })
                .collect(),
            total,
        ))
    }

    /// Return a globally filtered and sorted page of effective local assets for an artist.
    pub fn list_artist_local_assets_page(
        &self,
        names: &[String],
        limit: usize,
        offset: usize,
        query: Option<&str>,
        sort_field: Option<&str>,
        direction: Option<&str>,
        min_duration_seconds: Option<u32>,
        max_duration_seconds: Option<u32>,
        availability: Option<&str>,
    ) -> Result<(
        Vec<LocalAsset>,
        usize,
        std::collections::BTreeMap<String, bool>,
    )> {
        if names.len() > 200 || names.iter().any(|name| name.len() > 1024) {
            return Err(
                "Artist matching accepts at most 200 names of up to 1024 bytes each".into(),
            );
        }
        let requested = names
            .iter()
            .map(|name| normalize_artist_name(name))
            .filter(|name| !name.is_empty())
            .collect::<std::collections::HashSet<_>>();
        if requested.is_empty() {
            return Ok((Vec::new(), 0, std::collections::BTreeMap::new()));
        }
        let conn = self.lock_connection()?;
        materialize_artist_asset_candidates(&conn, &requested, None)?;
        let track_json = "track_json";
        let mut conditions = vec!["artist_key IN (SELECT name FROM artist_page_names)".to_owned()];
        let mut values = Vec::<rusqlite::types::Value>::new();
        if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
            conditions.push(format!(
                "(instr(unicode_lower(COALESCE(json_extract({track_json},'$.title'),'')),?)>0
                OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.artist'),'')),?)>0
                OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.album'),'')),?)>0
                OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.version'),'')),?)>0
                OR instr(unicode_lower(COALESCE(json_extract({track_json},'$.genre'),'')),?)>0)"
            ));
            let query = crate::track_sort_text::lowercase(query.trim());
            values.extend([
                query.clone().into(),
                query.clone().into(),
                query.clone().into(),
                query.clone().into(),
                query.into(),
            ]);
        }
        let duration_filter = format!(
            "CAST(({}) AS INTEGER)",
            crate::track_sort_sql::duration_expression(track_json)
        );
        if let Some(minimum) = min_duration_seconds {
            conditions.push(format!("{duration_filter}>=?"));
            values.push((minimum as i64).into());
        }
        if let Some(maximum) = max_duration_seconds {
            conditions.push(format!("{duration_filter}<=?"));
            values.push((maximum as i64).into());
        }
        match availability.unwrap_or("all") {
            "all" => {}
            "on-device" => {
                conditions.push("json_extract(track_json,'$.localAvailability')='available'".into())
            }
            "not-on-device" => conditions.push(
                "COALESCE(json_extract(track_json,'$.localAvailability'),'none')!='available'"
                    .into(),
            ),
            _ => return Err("Invalid artist asset availability filter".into()),
        }
        let where_sql = conditions.join(" AND ");
        let filtered = format!("FROM artist_page_effective_assets WHERE {where_sql}");
        let count_sql = format!("SELECT COUNT(*) {filtered}");
        let total = conn
            .query_row(
                &count_sql,
                rusqlite::params_from_iter(values.iter()),
                |row| row.get::<_, usize>(0),
            )
            .map_err(err)?;

        let sort_fields = [
            "title",
            "artist",
            "album",
            "duration",
            "releaseDate",
            "dateAdded",
            "bpm",
            "key",
            "plays",
            "views",
        ];
        let mut sort_availability =
            std::collections::BTreeMap::from([("sourceOrder".to_owned(), true)]);
        for field in sort_fields {
            let expression = crate::track_sort_sql::expression(field, track_json)
                .ok_or_else(|| format!("Unsupported artist sort field: {field}"))?;
            let sql = format!("SELECT EXISTS(SELECT 1 {filtered} AND ({expression}) IS NOT NULL)");
            let available = conn
                .query_row(&sql, rusqlite::params_from_iter(values.iter()), |row| {
                    row.get::<_, bool>(0)
                })
                .map_err(err)?;
            sort_availability.insert(field.into(), available);
        }

        let field = sort_field.unwrap_or("sourceOrder");
        let direction = if direction == Some("desc") {
            "DESC"
        } else {
            "ASC"
        };
        let order = if field == "sourceOrder" {
            if direction == "DESC" {
                "sort_order ASC".to_owned()
            } else {
                "sort_order DESC".to_owned()
            }
        } else {
            let expression = crate::track_sort_sql::expression(field, track_json)
                .ok_or_else(|| format!("Unsupported artist sort field: {field}"))?;
            format!(
                "CASE WHEN ({expression}) IS NULL THEN 1 ELSE 0 END ASC,({expression}) {direction},sort_order DESC"
            )
        };
        let page_sql = format!("SELECT asset_json {filtered} ORDER BY {order} LIMIT ? OFFSET ?");
        let mut page_values = values;
        page_values.push((limit.clamp(1, 200) as i64).into());
        page_values.push(
            i64::try_from(offset)
                .map_err(|_| "Artist asset page offset is too large")?
                .into(),
        );
        let mut stmt = conn.prepare(&page_sql).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(page_values.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(err)?;
        let items = rows
            .map(|row| decode(row.map_err(err)?))
            .collect::<Result<Vec<LocalAsset>>>()?;
        Ok((items, total, sort_availability))
    }

    /// Resolve only the exact source tracks requested, preserving unique local-match semantics.
    pub fn match_artist_playback_sources(
        &self,
        artist_names: &[String],
        source_tracks: &[Track],
    ) -> Result<Vec<Option<LocalAsset>>> {
        if source_tracks.len() > 200
            || artist_names.len() > 200
            || artist_names.iter().any(|name| name.len() > 1024)
            || source_tracks.iter().any(|track| {
                track.artist.len() > 1024
                    || track.title.len() > 1024
                    || track
                        .version
                        .as_deref()
                        .is_some_and(|version| version.len() > 1024)
            })
        {
            return Err(
                "Artist playback matching accepts at most 200 source tracks and artist names"
                    .into(),
            );
        }
        let mut names = artist_names.to_vec();
        names.extend(source_tracks.iter().map(|track| track.artist.clone()));
        let requested = names
            .iter()
            .map(|name| normalize_artist_name(name))
            .filter(|name| !name.is_empty())
            .collect::<std::collections::HashSet<_>>();
        if requested.is_empty() || source_tracks.is_empty() {
            return Ok(vec![None; source_tracks.len()]);
        }
        let conn = self.lock_connection()?;
        materialize_artist_asset_candidates(&conn, &requested, Some(source_tracks))?;
        let mut stmt = conn
            .prepare("SELECT asset_json FROM artist_page_effective_assets")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(err)?;
        let assets = rows
            .map(|row| decode(row.map_err(err)?))
            .collect::<Result<Vec<LocalAsset>>>()?;
        Ok(source_tracks
            .iter()
            .map(|source| {
                unique_artist_playback_match(source, &assets).map(|index| assets[index].clone())
            })
            .collect())
    }
    pub fn list_assets(&self) -> Result<Vec<LocalAsset>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT payload FROM local_assets ORDER BY rowid DESC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| hydrate_asset(&conn, decode(r.map_err(err)?)?))
            .collect()
    }

    pub fn list_track_files_for_recordings(
        &self,
        recording_ids: &[String],
    ) -> Result<Vec<TrackFilesForRecording>> {
        if recording_ids.len() > 200 {
            return Err("At most 200 recording IDs may be queried at once".into());
        }
        let mut ids = recording_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let values_sql = (1..=ids.len())
            .map(|n| format!("(?{n})"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "WITH requested(track_id) AS (VALUES {values_sql}), resolved AS (SELECT requested.track_id,COALESCE(catalog_listings.recording_id,requested.track_id) recording_id FROM requested LEFT JOIN catalog_listings ON catalog_listings.id=requested.track_id) SELECT resolved.track_id,local_assets.payload FROM resolved JOIN local_assets ON local_assets.recording_id=resolved.recording_id ORDER BY local_assets.rowid DESC"
        );
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(ids.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(err)?;
        let mut grouped = std::collections::HashMap::<String, Vec<LocalAsset>>::new();
        for row in rows {
            let (track_id, payload) = row.map_err(err)?;
            grouped
                .entry(track_id)
                .or_default()
                .push(hydrate_asset(&conn, decode(payload)?)?);
        }
        Ok(ids
            .into_iter()
            .map(|track_id| TrackFilesForRecording {
                files: grouped.remove(&track_id).unwrap_or_default(),
                track_id,
            })
            .collect())
    }

    pub fn count_assets(&self) -> Result<usize> {
        let conn = self.lock_connection()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_assets", [], |row| row.get(0))
            .map_err(err)?;
        Ok(count.max(0) as usize)
    }

    /// Read the lightweight path projection needed to allow registered files in the asset protocol.
    pub fn list_asset_paths(&self) -> Result<Vec<String>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT path FROM local_assets ORDER BY rowid DESC")
            .map_err(err)?;
        let paths = stmt
            .query_map([], |row| row.get(0))
            .map_err(err)?
            .collect::<std::result::Result<Vec<String>, _>>()
            .map_err(err)?;
        Ok(paths)
    }
    pub fn list_asset_paths_page(&self, limit: usize, offset: usize) -> Result<Vec<String>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn
            .prepare("SELECT path FROM local_assets ORDER BY rowid DESC LIMIT ?1 OFFSET ?2")
            .map_err(err)?;
        let paths = stmt
            .query_map(
                rusqlite::params![limit.clamp(1, 500) as i64, offset as i64],
                |row| row.get(0),
            )
            .map_err(err)?
            .collect::<std::result::Result<Vec<String>, _>>()
            .map_err(err)?;
        Ok(paths)
    }

    /// Select only assets that the analysis worker may need, without decoding every LocalAsset.
    pub fn list_analysis_queue_assets(
        &self,
        asset_ids: Option<&[String]>,
        force: bool,
        version: &str,
    ) -> Result<Vec<AnalysisQueueAsset>> {
        let mut sql = String::from(
            "SELECT rowid,id,path,hash FROM local_assets WHERE COALESCE(json_extract(payload,'$.processing'),'')<>'external-readonly'",
        );
        let mut values = Vec::<rusqlite::types::Value>::new();
        if let Some(ids) = asset_ids {
            if ids.len() > 200 {
                return Err("At most 200 asset IDs may be queued at once".into());
            }
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let start = values.len() + 1;
            let placeholders = (start..start + ids.len())
                .map(|n| format!("?{n}"))
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND id IN ({placeholders})"));
            values.extend(ids.iter().cloned().map(Into::into));
        }
        if !force {
            let index = values.len() + 1;
            sql.push_str(&format!(" AND NOT EXISTS (SELECT 1 FROM asset_analyses WHERE asset_id=local_assets.id AND file_hash=local_assets.hash AND version=?{index} AND status='completed')"));
            values.push(version.to_owned().into());
        }
        sql.push_str(" ORDER BY rowid DESC");
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let assets = stmt
            .query_map(rusqlite::params_from_iter(values), |row| {
                Ok(AnalysisQueueAsset {
                    rowid: row.get(0)?,
                    id: row.get(1)?,
                    path: row.get(2)?,
                    hash: row.get(3)?,
                })
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        Ok(assets)
    }

    /// Separate paged workflow for ArtistPage's exact local-recording matching.
    pub fn list_asset_page(&self, limit: usize, offset: usize) -> Result<(Vec<LocalAsset>, usize)> {
        let conn = self.lock_connection()?;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_assets", [], |row| row.get(0))
            .map_err(err)?;
        let mut stmt = conn
            .prepare("SELECT payload FROM local_assets ORDER BY rowid DESC LIMIT ?1 OFFSET ?2")
            .map_err(err)?;
        let rows = stmt
            .query_map(
                rusqlite::params![limit.clamp(1, 200) as i64, offset as i64],
                |row| row.get::<_, String>(0),
            )
            .map_err(err)?;
        let assets = rows
            .map(|row| hydrate_asset(&conn, decode(row.map_err(err)?)?))
            .collect::<Result<Vec<_>>>()?;
        Ok((assets, total.max(0) as usize))
    }

    /// Return every local file attached to a catalog track, with canonical recording hydration.
    pub fn list_track_files(&self, track_id: &str) -> Result<Vec<LocalAsset>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare("SELECT payload FROM local_assets WHERE recording_id=(SELECT recording_id FROM catalog_listings WHERE id=?1) ORDER BY rowid DESC").map_err(err)?;
        let rows = stmt
            .query_map([track_id], |row| row.get::<_, String>(0))
            .map_err(err)?;
        rows.map(|row| hydrate_asset(&conn, decode(row.map_err(err)?)?))
            .collect()
    }

    /// Select a bounded batch of older assets that have not had technical facts recorded.
    pub fn list_assets_needing_quality_facts(&self, limit: usize) -> Result<Vec<LocalAsset>> {
        let conn = self.lock_connection()?;
        let mut stmt = conn.prepare("SELECT payload FROM local_assets WHERE json_extract(payload,'$.track.provenance.localAudioFactsHash') IS NOT json_extract(payload,'$.hash') ORDER BY rowid DESC LIMIT ?1").map_err(err)?;
        let rows = stmt
            .query_map([limit.clamp(1, 100) as i64], |row| row.get::<_, String>(0))
            .map_err(err)?;
        let mut assets = Vec::new();
        for row in rows {
            let asset: LocalAsset = decode(row.map_err(err)?)?;
            assets.push(hydrate_asset(&conn, asset)?);
        }
        Ok(assets)
    }

    /// Save probed facts only if the indexed asset still has the hash that was inspected.
    pub fn save_asset_quality_facts(
        &self,
        asset_id: &str,
        expected_hash: &str,
        facts: &crate::models::AudioFacts,
    ) -> Result<()> {
        let conn = self.lock_connection()?;
        let (hash, payload): (String, String) = conn
            .query_row(
                "SELECT hash,payload FROM local_assets WHERE id=?1",
                [asset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        if hash != expected_hash {
            return Err("Local asset changed while quality inspection was running".into());
        }
        let mut asset: LocalAsset = decode(payload)?;
        if asset.hash != expected_hash {
            return Err("Saved asset payload does not match its indexed hash".into());
        }
        asset.final_codec = facts.codec.clone();
        asset.final_bitrate = facts.bitrate_kbps;
        asset.bitrate_origin = facts.bitrate_origin.clone();
        asset.sample_rate = facts.sample_rate;
        asset.channels = facts.channels;
        asset.bit_depth = facts.bit_depth;
        asset.duration_seconds = facts.duration_seconds;
        asset.size_bytes = facts.size_bytes;
        if !asset.track.provenance.is_object() {
            asset.track.provenance = serde_json::json!({});
        }
        asset.track.provenance["localAudioFactsHash"] = serde_json::json!(expected_hash);
        conn.execute(
            "UPDATE local_assets SET payload=?1 WHERE id=?2 AND hash=?3",
            params![encode(&asset)?, asset_id, expected_hash],
        )
        .map_err(err)?;
        Ok(())
    }

    /// Persist the preferred local file for the recording represented by a track.
    /// Asset and track must resolve to the same canonical recording.
    pub fn set_preferred_track_file(&self, track_id: &str, asset_id: &str) -> Result<Track> {
        let conn = self.lock_connection()?;
        let recording: String = conn
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [track_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        let asset_recording: String = conn
            .query_row(
                "SELECT recording_id FROM local_assets WHERE id=?1",
                [asset_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if recording != asset_recording {
            return Err("Selected file does not belong to this recording".into());
        }
        let key = format!("preferredAsset.{recording}");
        conn.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, encode(&asset_id)?]).map_err(err)?;
        let raw: String = conn
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [track_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        hydrate(&conn, decode(raw)?)
    }

    pub fn get_preferred_track_file(&self, track_id: &str) -> Result<Option<String>> {
        let conn = self.lock_connection()?;
        let recording: Option<String> = conn
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [track_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)?;
        let Some(recording) = recording else {
            return Ok(None);
        };
        let key = format!("preferredAsset.{recording}");
        let raw: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(err)?;
        raw.map(decode).transpose()
    }

    /// Commit a newly downloaded managed file, optionally replacing a selected managed asset.
    /// Replacement retains the old bytes beside the library file as a recoverable backup.
    pub fn commit_download_asset(
        &self,
        asset: &LocalAsset,
        disposition: &ReplacementDisposition,
    ) -> Result<LocalAsset> {
        if matches!(disposition, ReplacementDisposition::Keep) {
            self.save_asset(asset)?;
            return Ok(asset.clone());
        }
        let ReplacementDisposition::Replace {
            target_asset_id,
            expected_hash,
        } = disposition
        else {
            unreachable!()
        };
        let settings = self.settings()?;
        let root = Path::new(&settings.music_directory)
            .canonicalize()
            .map_err(|e| format!("Cannot resolve managed music directory: {e}"))?;
        let mut conn = self.lock_connection()?;
        ensure_track(&conn, &asset.track)?;
        let (target_hash, target_path, target_payload, target_recording): (
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT hash,path,payload,recording_id FROM local_assets WHERE id=?1",
                [target_asset_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(err)?;
        let source_recording: String = conn
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [&asset.track.id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if target_recording != source_recording {
            return Err("Replacement target belongs to a different recording".into());
        }
        let target: LocalAsset = decode(target_payload)?;
        if target.processing == "external-readonly" {
            return Err("Only managed library files can be replaced".into());
        }
        if target_hash != *expected_hash {
            return Err("Replacement target changed since it was selected".into());
        }
        let old_path = Path::new(&target_path);
        let target_parent = old_path
            .parent()
            .ok_or("Managed target has no parent directory")?
            .canonicalize()
            .map_err(|e| format!("Cannot resolve managed target directory: {e}"))?;
        if !target_parent.starts_with(&root) {
            return Err("Replacement target is outside the managed music directory".into());
        }
        let source = Path::new(&asset.path);
        let source_parent = source
            .parent()
            .ok_or("Downloaded file has no parent directory")?
            .canonicalize()
            .map_err(|e| format!("Cannot resolve downloaded file directory: {e}"))?;
        if !source_parent.starts_with(&root) || !source.is_file() {
            return Err("Replacement source must be a managed downloaded file".into());
        }
        if std::fs::symlink_metadata(source)
            .map_err(err)?
            .file_type()
            .is_symlink()
            || std::fs::symlink_metadata(&target_path)
                .map_err(err)?
                .file_type()
                .is_symlink()
        {
            return Err("Replacement files must be regular managed files".into());
        }
        if crate::media::hash_file(source)? != asset.hash
            || crate::media::hash_file(old_path)? != *expected_hash
        {
            return Err("A file changed while replacement was being prepared".into());
        }
        let extension = crate::media::published_audio_extension(&asset.final_codec, source)?;
        let stem = old_path
            .file_stem()
            .ok_or("Managed target has no filename stem")?;
        let replacement_path =
            old_path.with_file_name(format!("{}.{}", stem.to_string_lossy(), extension));
        let same_target = replacement_path.as_path() == old_path;
        let same_source = replacement_path.as_path() == source;
        if !same_target && !same_source && replacement_path.exists() {
            return Err("A different file already occupies the replacement filename".into());
        }
        let replacement_path_string = replacement_path.to_string_lossy().into_owned();
        let path_occupied: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_assets WHERE path=?1 AND id<>?2)",
                params![&replacement_path_string, target_asset_id],
                |row| row.get(0),
            )
            .map_err(err)?;
        if path_occupied {
            return Err("Another library asset already uses the replacement filename".into());
        }
        let backup = old_path.with_file_name(format!(
            "{}.burrata-backup-{}",
            old_path.file_name().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        std::fs::rename(old_path, &backup)
            .map_err(|e| format!("Cannot preserve old asset as backup: {e}"))?;
        match crate::media::hash_file(&backup) {
            Ok(hash) if hash == *expected_hash => {}
            Ok(_) => {
                let _ = restore_replacement_backup(&backup, old_path);
                return Err(
                    "Replacement target changed during commit; old asset was restored".into(),
                );
            }
            Err(error) => {
                let _ = restore_replacement_backup(&backup, old_path);
                return Err(format!(
                    "Cannot verify replacement backup; old asset was restored: {error}"
                ));
            }
        }
        let moved_source = !same_source;
        if moved_source {
            if let Err(e) = std::fs::hard_link(source, &replacement_path) {
                let _ = restore_replacement_backup(&backup, old_path);
                return Err(format!(
                    "Cannot install replacement; old asset was restored: {e}"
                ));
            }
            if let Err(e) = std::fs::remove_file(source) {
                let _ = std::fs::remove_file(&replacement_path);
                let _ = restore_replacement_backup(&backup, old_path);
                return Err(format!(
                    "Cannot finish installing replacement; old asset was restored: {e}"
                ));
            }
        }
        let mut committed = asset.clone();
        committed.id = target_asset_id.clone();
        committed.recording_id = target_recording;
        committed.path = replacement_path.to_string_lossy().into_owned();
        committed.track.local_path = Some(committed.path.clone());
        if !committed.track.provenance.is_object() {
            committed.track.provenance = serde_json::json!({});
        }
        committed.track.provenance["replacementBackupPath"] =
            serde_json::json!(backup.to_string_lossy().into_owned());
        let tx_result = (|| -> Result<()> {
            let tx = conn.transaction().map_err(err)?;
            tx.execute(
                "UPDATE local_assets SET path=?1,hash=?2,payload=?3 WHERE id=?4",
                params![
                    committed.path,
                    committed.hash,
                    encode(&committed)?,
                    target_asset_id
                ],
            )
            .map_err(err)?;
            promote_track(&tx, &asset.track.id)?;
            tx.commit().map_err(err)?;
            Ok(())
        })();
        if let Err(error) = tx_result {
            if moved_source {
                if !source.exists() {
                    let _ = std::fs::rename(&replacement_path, source);
                }
            }
            let _ = restore_replacement_backup(&backup, old_path);
            return Err(format!(
                "Cannot commit replacement metadata; previous asset restored: {error}"
            ));
        }
        Ok(committed)
    }
}

fn db_wait_metrics_enabled() -> bool {
    std::env::var("BURRATA_DB_METRICS").is_ok_and(|value| value == "1")
        || std::env::var("BURRATA_PLAYBACK_TRACE").is_ok_and(|value| value == "1")
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
        upsert_track(conn, track, false)?;
    }
    Ok(())
}
fn promote_track(conn: &Connection, track_id: &str) -> Result<()> {
    let changed = conn
        .execute(
            "UPDATE catalog_listings
             SET in_library=1,
                 metadata=CASE WHEN COALESCE(
                    json_extract(metadata,'$.dateAdded'),
                    json_extract(metadata,'$.provenance.dateAdded'),
                    json_extract(metadata,'$.provenance.addedAt'),
                    json_extract(metadata,'$.provenance.importedAt'),
                    json_extract(metadata,'$.provenance.createdAt'),
                    json_extract((SELECT manual_metadata FROM recordings WHERE id=catalog_listings.recording_id),'$.dateAdded'),
                    json_extract((SELECT manual_metadata FROM recordings WHERE id=catalog_listings.recording_id),'$.provenance.dateAdded'),
                    json_extract((SELECT manual_metadata FROM recordings WHERE id=catalog_listings.recording_id),'$.provenance.addedAt'),
                    json_extract((SELECT manual_metadata FROM recordings WHERE id=catalog_listings.recording_id),'$.provenance.importedAt'),
                    json_extract((SELECT manual_metadata FROM recordings WHERE id=catalog_listings.recording_id),'$.provenance.createdAt')
                 ) IS NULL THEN json_set(metadata,'$.dateAdded',?1) ELSE metadata END
             WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![chrono::Utc::now().to_rfc3339(), track_id],
        )
        .map_err(err)?;
    if changed == 0 {
        return Err("Cannot add a missing track to the Library".into());
    }
    Ok(())
}
fn unsorted_library_selection_token(
    id: &str,
    updated_at: &str,
    metadata: &str,
    manual_metadata: &str,
) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(b"burrata-unsorted-library-selection-v1\0");
    digest.update(id.as_bytes());
    digest.update(b"\0");
    digest.update(updated_at.as_bytes());
    digest.update(b"\0");
    digest.update(metadata.as_bytes());
    digest.update(b"\0");
    digest.update(manual_metadata.as_bytes());
    format!("{:x}", digest.finalize())
}

fn upsert_track(conn: &Connection, track: &Track, promote: bool) -> Result<()> {
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
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(old) = old {
        let old: serde_json::Value = decode(old)?;
        if let (Some(new), Some(old)) = (metadata.as_object_mut(), old.as_object()) {
            if old.get("dateAdded").is_some_and(|v| !v.is_null()) {
                new.insert("dateAdded".into(), old["dateAdded"].clone());
            }
            for (k, v) in old {
                if new.get(k).is_some_and(|v| v.is_null()) && !v.is_null() {
                    new.insert(k.clone(), v.clone());
                }
            }
            let incoming_provenance = new
                .entry("provenance")
                .or_insert_with(|| serde_json::json!({}));
            if !incoming_provenance.is_object() {
                *incoming_provenance = serde_json::json!({});
            }
            for names in [
                &["plays", "playCount", "play_count"][..],
                &["views", "viewCount", "view_count"][..],
            ] {
                if known_provenance_count(incoming_provenance, names).is_none() {
                    if let Some(value) = old
                        .get("provenance")
                        .and_then(|value| known_provenance_count(value, names))
                    {
                        incoming_provenance[names[0]] = value;
                    }
                }
            }
        }
    }
    if promote {
        if let Some(obj) = metadata.as_object_mut() {
            if obj.get("dateAdded").is_none_or(serde_json::Value::is_null) {
                obj.insert("dateAdded".into(), serde_json::json!(now));
            }
        }
    }
    conn.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES(?1,?2,?3,?1,?4,?5,?6,?7) ON CONFLICT(provider,provider_id) DO UPDATE SET metadata=excluded.metadata,source_url=excluded.source_url,updated_at=excluded.updated_at,in_library=MAX(catalog_listings.in_library,excluded.in_library)", params![track.id,track.provider,track.provider_id,track.source_url,encode(&metadata)?,now,promote]).map_err(err)?;
    if promote {
        promote_track(conn, &track.id)?;
    }
    Ok(())
}

fn known_provenance_count(
    provenance: &serde_json::Value,
    names: &[&str],
) -> Option<serde_json::Value> {
    fn first_non_null<'a>(
        container: &'a serde_json::Value,
        names: &[&str],
    ) -> Option<&'a serde_json::Value> {
        names
            .iter()
            .filter_map(|name| container.get(*name))
            .find(|value| !value.is_null())
    }
    let top = first_non_null(provenance, names);
    if top.is_some_and(|value| crate::track_sort_text::parse_sort_count(value).is_some()) {
        return top.cloned();
    }
    let remote = provenance
        .get("remoteCatalog")
        .and_then(|container| first_non_null(container, names));
    remote
        .filter(|value| crate::track_sort_text::parse_sort_count(value).is_some())
        .cloned()
}

fn bpm_projection_sort_expression(metadata: &str) -> String {
    crate::track_sort_sql::expression("bpm", metadata).expect("BPM is a supported SQL sort field")
}

fn sort_projection_stale_index_name(revision: i64) -> String {
    format!("catalog_listings_sort_projection_stale_r{revision}")
}

fn sort_projection_stale_predicate(alias: &str, revision: i64) -> String {
    let qualifier = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    format!(
        "{qualifier}sort_metadata_projection IS NULL OR {qualifier}sort_projection_revision IS NOT {revision}"
    )
}

fn stale_projection_recordings_sql(revision: i64) -> String {
    let index_name = sort_projection_stale_index_name(revision);
    let predicate = sort_projection_stale_predicate("l", revision);
    format!(
        "SELECT DISTINCT l.recording_id FROM catalog_listings AS l INDEXED BY {index_name} WHERE ({predicate})"
    )
}

fn projection_pending_single_asset_sql(revision: i64) -> String {
    format!(
        "SELECT EXISTS(SELECT 1 FROM ({}) AS stale JOIN local_assets AS a ON a.recording_id=stale.recording_id GROUP BY stale.recording_id HAVING COUNT(a.id)=1)",
        stale_projection_recordings_sql(revision)
    )
}

fn multiple_stale_asset_recordings_sql(revision: i64) -> String {
    format!(
        "JOIN ({}) AS multi_asset_recordings ON multi_asset_recordings.recording_id=r.id",
        stale_multi_asset_recordings_sql(revision)
    )
}

fn stale_multi_asset_recordings_sql(revision: i64) -> String {
    format!(
        "SELECT stale.recording_id FROM ({}) AS stale JOIN local_assets AS a ON a.recording_id=stale.recording_id GROUP BY stale.recording_id HAVING COUNT(a.id)>1",
        stale_projection_recordings_sql(revision)
    )
}

fn should_use_bpm_projection_index(
    view: &str,
    sort_field: Option<&str>,
    has_dynamic_page_metadata: bool,
    filtered_projection_coverage: Option<i64>,
) -> bool {
    view == "library"
        && sort_field == Some("bpm")
        && !has_dynamic_page_metadata
        && filtered_projection_coverage != Some(0)
}

fn indexed_bpm_page_sql(
    metadata_projection: &str,
    where_sql: &str,
    direction: &str,
    tie_direction: &str,
    revision: i64,
) -> String {
    let sort_expression = bpm_projection_sort_expression("l.sort_metadata_projection");
    let null_expression = format!("CASE WHEN ({sort_expression}) IS NULL THEN 1 ELSE 0 END");
    format!(
        "SELECT l.id,{metadata_projection},NULL FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE ({where_sql}) AND l.sort_projection_revision={revision} AND l.sort_metadata_projection IS NOT NULL ORDER BY ({null_expression}) ASC,({sort_expression}) {direction},l.rowid {tie_direction} LIMIT ? OFFSET ?"
    )
}

fn sort_metadata_projection(track: &Track) -> serde_json::Value {
    use serde_json::{Map, Value};

    let mut projection = Map::new();
    macro_rules! copy_field {
        ($field:literal, $value:expr) => {
            projection.insert($field.to_owned(), serde_json::json!($value));
        };
    }
    copy_field!("provider", track.provider);
    copy_field!("title", track.title);
    copy_field!("artist", track.artist);
    copy_field!("album", track.album);
    copy_field!("version", track.version);
    copy_field!("genre", track.genre);
    copy_field!("durationSeconds", track.duration_seconds);
    copy_field!("playbackRange", track.playback_range);
    copy_field!("releaseDate", track.release_date);
    copy_field!("recordingDate", track.recording_date);
    copy_field!("dateAdded", track.date_added);
    copy_field!("bpm", track.bpm);
    copy_field!("key", track.key);

    let source = track.provenance.as_object();
    let mut provenance = Map::new();
    for key in [
        "dateAdded",
        "addedAt",
        "importedAt",
        "createdAt",
        "plays",
        "playCount",
        "play_count",
        "views",
        "viewCount",
        "view_count",
    ] {
        if let Some(value) = source.and_then(|object| object.get(key)) {
            provenance.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(remote) = source
        .and_then(|object| object.get("remoteCatalog"))
        .and_then(Value::as_object)
    {
        let mut selected = Map::new();
        for key in [
            "plays",
            "playCount",
            "play_count",
            "views",
            "viewCount",
            "view_count",
        ] {
            if let Some(value) = remote.get(key) {
                selected.insert(key.to_owned(), value.clone());
            }
        }
        if !selected.is_empty() {
            provenance.insert("remoteCatalog".to_owned(), Value::Object(selected));
        }
    }
    projection.insert("provenance".to_owned(), Value::Object(provenance));
    Value::Object(projection)
}

/// Build the sort/filter metadata without the full UI hydration path for the
/// common single-asset case. Conditions are intentionally strict: manual edits,
/// multiple assets, matching analyses, and autotag overlays use the canonical
/// hydrate() implementation below.

fn fast_asset_sort_track(
    source: &Track,
    manual_json: &str,
    favorite: bool,
    disposition: Option<String>,
    asset_count: i64,
    asset_payload: &str,
    has_matching_analysis: bool,
) -> Result<Option<Track>> {
    if manual_json != "{}" || asset_count != 1 || has_matching_analysis {
        return Ok(None);
    }
    let asset: LocalAsset = decode(asset_payload.to_owned())?;
    if asset.track.provenance["autotag"].is_object() {
        return Ok(None);
    }
    let mut track = source.clone();
    track.favorite = favorite;
    track.disposition = disposition;
    if track.bpm.is_none() {
        track.bpm = asset.track.bpm;
    }
    if track.key.is_none() {
        track.key = asset.track.key;
    }
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    let manual_fields = track.provenance["manualFields"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    track.provenance["manualFields"] = serde_json::Value::Object(manual_fields);
    Ok(Some(track))
}

/// Preload database overlays in bounded chunks, while keeping every raw track
/// occurrence separate. Page chapter JSON and ordering never pass through an
/// ID-keyed output map. No state is cached beyond this connection-locked call.
fn hydrate_page_tracks(conn: &Connection, tracks: Vec<Track>) -> Result<Vec<Track>> {
    let mut result = Vec::with_capacity(tracks.len());
    let mut tracks = tracks.into_iter();
    loop {
        // <=200 IDs per IN and <=400 parameters for exact asset/hash pairs,
        // comfortably below SQLite's conservative 999-variable limit.
        let chunk = tracks.by_ref().take(200).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }
        result.extend(hydrate_track_chunk(conn, chunk)?);
    }
    Ok(result)
}

fn hydrate_track_chunk(conn: &Connection, tracks: Vec<Track>) -> Result<Vec<Track>> {
    use std::collections::{BTreeSet, HashMap};
    let ids = tracks
        .iter()
        .map(|track| track.id.clone())
        .collect::<BTreeSet<_>>();
    let placeholders = vec!["?"; ids.len()].join(",");
    let mut recording_stmt = conn.prepare(&format!(
        "SELECT l.id,r.favorite,r.disposition,r.manual_metadata FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id IN ({placeholders})"
    )).map_err(err)?;
    let recording_rows = recording_stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ),
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<HashMap<String, RecordingHydration>, _>>()
        .map_err(err)?;
    let tracks = tracks
        .into_iter()
        .map(|track| {
            let values = recording_rows
                .get(&track.id)
                .ok_or_else(|| err(rusqlite::Error::QueryReturnedNoRows))?;
            apply_hydration_metadata(track, values.clone())
        })
        .collect::<Result<Vec<_>>>()?;
    // Preserve the single-track path's asset lookup *after* manual overrides,
    // including legacy metadata that changes the track's ID.
    let ids = tracks
        .iter()
        .map(|(track, _)| track.id.clone())
        .collect::<BTreeSet<_>>();
    let placeholders = vec!["?"; ids.len()].join(",");
    let mut context_stmt = conn.prepare(&format!(
        "SELECT l.id,l.recording_id,s.value FROM catalog_listings l LEFT JOIN settings s ON s.key=('preferredAsset.' || l.recording_id) WHERE l.id IN ({placeholders})"
    )).map_err(err)?;
    let contexts = context_stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    let mut recording_for_id = HashMap::new();
    let mut preferences = HashMap::new();
    for (id, recording, preferred) in contexts {
        // The individual path validates even a preference with no assets.
        let preferred = preferred.map(decode::<String>).transpose()?;
        recording_for_id.insert(id, recording.clone());
        preferences.insert(recording, preferred);
    }
    let recordings = recording_for_id.values().cloned().collect::<BTreeSet<_>>();
    let mut assets: HashMap<String, Vec<AssetHydrationRow>> = HashMap::new();
    if !recordings.is_empty() {
        let placeholders = vec!["?"; recordings.len()].join(",");
        let mut asset_stmt = conn.prepare(&format!(
            "SELECT recording_id,id,path,hash,payload FROM local_assets WHERE recording_id IN ({placeholders}) ORDER BY rowid ASC"
        )).map_err(err)?;
        let rows = asset_stmt
            .query_map(rusqlite::params_from_iter(recordings.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ),
                ))
            })
            .map_err(err)?;
        for row in rows {
            let (recording, asset) = row.map_err(err)?;
            assets.entry(recording).or_default().push(asset);
        }
    }
    let matching_level = current_matching_level(conn);
    let selected = recordings
        .iter()
        .map(|recording| {
            let rows = assets.get(recording).map(Vec::as_slice).unwrap_or(&[]);
            let preferred = preferences.get(recording).and_then(Option::as_deref);
            let request = tracks
                .iter()
                .find(|(track, _)| recording_for_id.get(&track.id) == Some(recording))
                .map(|(track, _)| track);
            let rows = request
                .map(|track| eligible_hydration_rows(rows, track, matching_level))
                .unwrap_or_else(|| rows.to_vec());
            (recording.clone(), select_hydration_asset(&rows, preferred))
        })
        .collect::<HashMap<_, _>>();
    let pairs = selected
        .values()
        .filter_map(Option::as_ref)
        .map(|row| (row.0.clone(), row.2.clone()))
        .collect::<BTreeSet<_>>();
    let mut analyses: HashMap<(String, String), AnalysisHydrationRow> = HashMap::new();
    if !pairs.is_empty() {
        let placeholders = vec!["(?,?)"; pairs.len()].join(",");
        let parameters = pairs.iter().flat_map(|(id, hash)| [id, hash]);
        // Use the original latest-row ORDER BY policy. Equal updated_at ties
        // remain unspecified; stale hashes and unselected assets cannot
        // contribute payloads or errors to this page.
        let mut analysis_stmt = conn.prepare(&format!(
            "WITH wanted(asset_id,file_hash) AS (VALUES {placeholders}) SELECT a.asset_id,a.file_hash,a.status,a.version,a.payload FROM wanted w JOIN asset_analyses a ON a.rowid=(SELECT candidate.rowid FROM asset_analyses candidate WHERE candidate.asset_id=w.asset_id AND candidate.file_hash=w.file_hash ORDER BY candidate.updated_at DESC LIMIT 1)"
        )).map_err(err)?;
        analyses = analysis_stmt
            .query_map(rusqlite::params_from_iter(parameters), |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    (
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ),
                ))
            })
            .map_err(err)?
            .collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(err)?;
    }
    tracks
        .into_iter()
        .map(|(track, manual)| {
            let selected = recording_for_id
                .get(&track.id)
                .and_then(|recording| selected.get(recording))
                .and_then(Option::as_ref)
                .map(|row| {
                    let analysis = analyses.get(&(row.0.clone(), row.2.clone())).cloned();
                    (row.clone(), analysis)
                });
            finish_hydration(track, manual, selected)
        })
        .collect()
}

type RecordingHydration = (bool, Option<String>, String);
type AssetHydrationRow = (String, String, String, String);
type AnalysisHydrationRow = (String, String, String);
type SelectedHydrationAsset = (AssetHydrationRow, Option<AnalysisHydrationRow>);

fn hydrate(conn: &Connection, track: Track) -> Result<Track> {
    let values: RecordingHydration = conn.query_row("SELECT r.favorite,r.disposition,r.manual_metadata FROM recordings r JOIN catalog_listings l ON l.recording_id=r.id WHERE l.id=?1", [&track.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(err)?;
    let (track, manual) = apply_hydration_metadata(track, values)?;
    let selected = load_hydration_asset(conn, &track)?;
    finish_hydration(track, manual, selected)
}

fn apply_hydration_metadata(
    mut track: Track,
    values: RecordingHydration,
) -> Result<(Track, serde_json::Value)> {
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
    Ok((track, manual))
}

fn load_hydration_asset(
    conn: &Connection,
    request: &Track,
) -> Result<Option<SelectedHydrationAsset>> {
    let listing_id = request.id.as_str();
    let mut asset_stmt = conn
        .prepare("SELECT id,path,hash,payload FROM local_assets WHERE recording_id=(SELECT recording_id FROM catalog_listings WHERE id=?1) ORDER BY rowid ASC")
        .map_err(err)?;
    let asset_rows = asset_stmt
        .query_map([listing_id], |r| {
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
    let preferred: Option<String> = conn.query_row(
        "SELECT value FROM settings WHERE key=('preferredAsset.' || (SELECT recording_id FROM catalog_listings WHERE id=?1))",
        [listing_id], |r| r.get(0)).optional().map_err(err)?;
    let preferred = preferred.map(decode::<String>).transpose()?;
    let eligible = eligible_hydration_rows(&asset_rows, request, current_matching_level(conn));
    let row = select_hydration_asset(&eligible, preferred.as_deref());
    row.map(|row| {
        let analysis = conn.query_row(
            "SELECT status,version,payload FROM asset_analyses WHERE asset_id=?1 AND file_hash=?2 ORDER BY updated_at DESC LIMIT 1",
            params![row.0, row.2], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional().map_err(err)?;
        Ok((row, analysis))
    }).transpose()
}

fn current_matching_level(conn: &Connection) -> crate::recording_match::MatchLevel {
    conn.query_row(
        "SELECT value FROM settings WHERE key='application'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| decode::<Settings>(value).ok())
    .map(|settings| settings.matching_level)
    .unwrap_or_default()
}

fn eligible_hydration_rows(
    rows: &[AssetHydrationRow],
    request: &Track,
    level: crate::recording_match::MatchLevel,
) -> Vec<AssetHydrationRow> {
    rows.iter()
        .filter(|row| {
            serde_json::from_str::<LocalAsset>(&row.3)
                .ok()
                .is_some_and(|asset| {
                    crate::compilation_excerpt::asset_proof_is_eligible(
                        request,
                        &asset.track.provenance,
                        level,
                    )
                })
        })
        .cloned()
        .collect()
}

fn select_hydration_asset(
    asset_rows: &[AssetHydrationRow],
    preferred: Option<&str>,
) -> Option<AssetHydrationRow> {
    preferred
        .and_then(|id| {
            asset_rows
                .iter()
                .find(|(asset_id, path, _, _)| asset_id == id && Path::new(path).is_file())
        })
        .or_else(|| {
            asset_rows
                .iter()
                .position(|(_, path, _, _)| Path::new(path).is_file())
                .map(|index| &asset_rows[index])
        })
        .or_else(|| (!asset_rows.is_empty()).then(|| &asset_rows[0]))
        .cloned()
}

fn finish_hydration(
    mut track: Track,
    manual: serde_json::Value,
    selected: Option<SelectedHydrationAsset>,
) -> Result<Track> {
    if let Some(provenance) = track.provenance.as_object_mut() {
        provenance.remove("approximateExcerpt");
    }
    let manual_fields = track.provenance["manualFields"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    track.asset_id = None;
    track.local_path = None;
    track.local_availability = "none".into();
    if let Some(((asset_id, path, _hash, payload), analysis)) = selected {
        let asset: LocalAsset = decode(payload)?;
        if asset.track.provenance["automaticExcerpt"].is_object() {
            if !track.provenance.is_object() {
                track.provenance = serde_json::json!({});
            }
            track.provenance["approximateExcerpt"] =
                asset.track.provenance["automaticExcerpt"].clone();
        }
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
    crate::recording_identity::hydrate_identifiers(&mut track);
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
    fn cleanup_selection(store: &Store, id: &str) -> LibraryEntrySelection {
        let connection = store.connection.lock().unwrap();
        let (metadata, updated_at, manual_metadata): (String, String, String) = connection
            .query_row(
                "SELECT l.metadata,l.updated_at,r.manual_metadata FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        LibraryEntrySelection {
            id: id.into(),
            selection_token: unsorted_library_selection_token(
                id,
                &updated_at,
                &metadata,
                &manual_metadata,
            ),
        }
    }

    #[test]
    fn unsorted_library_cleanup_only_demotes_reviewed_unprotected_rows() {
        let (_directory, store) = db();
        let removable = store.save_track(&track("cleanup-unsorted", "")).unwrap();
        let favorite = store.save_track(&track("cleanup-favorite", "")).unwrap();
        let shortlisted = store.save_track(&track("cleanup-shortlist", "")).unwrap();
        let collected = store.save_track(&track("cleanup-collection", "")).unwrap();
        let asset = store.save_track(&track("cleanup-asset", "")).unwrap();
        store.set_favorite(&favorite, true).unwrap();
        store
            .set_disposition(&shortlisted, Some("shortlist"))
            .unwrap();
        let collection = store
            .create_collection("Protected cleanup collection")
            .unwrap();
        store
            .add_to_collection(&collection.id, std::slice::from_ref(&collected))
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES('cleanup-asset-file',?1,'/tmp/cleanup-asset.flac','fixture-hash','{}')",
                [&asset.id],
            )
            .unwrap();

        let preview = store.preview_unsorted_library_entries(100, 0).unwrap();
        assert_eq!(preview.total, 1);
        assert_eq!(preview.entries.len(), 1);
        assert_eq!(preview.entries[0].id, removable.id);
        assert_eq!(preview.entries[0].classification, "unsortedLibraryEntry");
        assert!(!preview.entries[0].classification.contains("legacySearch"));

        let selections = vec![
            LibraryEntrySelection {
                id: preview.entries[0].id.clone(),
                selection_token: preview.entries[0].selection_token.clone(),
            },
            cleanup_selection(&store, &favorite.id),
            cleanup_selection(&store, &shortlisted.id),
            cleanup_selection(&store, &collected.id),
            cleanup_selection(&store, &asset.id),
        ];
        let result = store.remove_unsorted_library_entries(&selections).unwrap();
        assert_eq!(result.removed_ids, vec![removable.id.clone()]);
        assert_eq!(result.skipped.len(), 4);
        assert!(result
            .skipped
            .iter()
            .all(|row| row.reason == "protectedOrNoLongerInLibrary"));
        assert!(store.get_track(&removable.id).unwrap().is_some());
        assert_eq!(store.list_tracks("all", None).unwrap().len(), 5);
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 4);
        let asset_count: i64 = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM local_assets WHERE id='cleanup-asset-file' AND recording_id=?1",
                [&asset.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(asset_count, 1);
    }

    #[test]
    fn unsorted_library_cleanup_selection_token_rejects_changed_or_newly_protected_rows() {
        let (_directory, store) = db();
        let changed = store.save_track(&track("cleanup-changed", "")).unwrap();
        let protected = store
            .save_track(&track("cleanup-protected-later", ""))
            .unwrap();
        let preview = store.preview_unsorted_library_entries(100, 0).unwrap();
        let selection_for = |id: &str| {
            preview
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| LibraryEntrySelection {
                    id: entry.id.clone(),
                    selection_token: entry.selection_token.clone(),
                })
                .unwrap()
        };
        let changed_selection = selection_for(&changed.id);
        let protected_selection = selection_for(&protected.id);

        let mut revised = changed.clone();
        revised.title = "Updated after preview".into();
        store.update_track(&revised).unwrap();
        store.set_favorite(&protected, true).unwrap();

        let result = store
            .remove_unsorted_library_entries(&[changed_selection, protected_selection])
            .unwrap();
        assert!(result.removed_ids.is_empty());
        assert_eq!(result.skipped.len(), 2);
        assert!(result
            .skipped
            .iter()
            .any(|row| row.id == changed.id && row.reason == "changedSincePreview"));
        assert!(result
            .skipped
            .iter()
            .any(|row| { row.id == protected.id && row.reason == "protectedOrNoLongerInLibrary" }));
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 2);
    }

    #[test]
    fn unsorted_library_cleanup_preview_includes_each_saved_listing_for_merged_recording() {
        let (_directory, store) = db();
        let canonical = store.save_track(&track("cleanup-canonical", "")).unwrap();
        let alternate = store.save_track(&track("cleanup-alternate", "")).unwrap();
        store
            .merge_recordings(&canonical.id, std::slice::from_ref(&alternate.id))
            .unwrap();

        let preview = store.preview_unsorted_library_entries(100, 0).unwrap();
        let ids = preview
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(preview.total, 2);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(canonical.id.as_str()));
        assert!(ids.contains(alternate.id.as_str()));
    }

    #[test]
    fn flexible_excerpt_publication_is_transactional_policy_scoped_and_preserves_catalog_row() {
        let (directory, store) = db();
        let mut requested = track("excerpt-policy", "");
        requested.title = "Verified soundtrack recording".into();
        requested.duration_seconds = Some(180.0);
        requested.album = Some("Fixture Album".into());
        store.save_track(&requested).unwrap();
        let raw_before: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&requested.id],
                |row| row.get(0),
            )
            .unwrap();

        let mut settings = store.settings().unwrap();
        settings.matching_level = crate::recording_match::MatchLevel::Flexible;
        settings.background_source_preparation = true;
        settings.streaming_fallback_enabled = true;
        store.save_settings(&settings).unwrap();
        let proof = crate::compilation_excerpt::test_eligible_excerpt_proof(&requested);
        let request = crate::compilation_excerpt::request_fingerprint(
            &requested,
            crate::recording_match::MatchLevel::Flexible,
        );
        let path = directory.path().join("verified-excerpt.flac");
        std::fs::write(&path, b"verified excerpt").unwrap();
        let mut asset_track = requested.clone();
        // `finalize_download` sets the embedded track duration to the actual
        // cut length, which differs slightly from the catalog runtime.
        asset_track.duration_seconds = Some(179.4);
        asset_track.provenance["automaticExcerpt"] = proof["automaticExcerpt"].clone();
        let mut asset = LocalAsset {
            id: "asset-verified-excerpt".into(),
            recording_id: requested.id.clone(),
            path: path.to_string_lossy().into_owned(),
            hash: crate::media::hash_file(&path).unwrap(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(48_000),
            channels: Some(2),
            bit_depth: Some(24),
            duration_seconds: Some(179.4),
            size_bytes: Some(16),
            bitrate_origin: None,
            processing: "verified excerpt".into(),
            track: asset_track,
        };

        assert_ne!(
            crate::compilation_excerpt::request_fingerprint(
                &asset.track,
                crate::recording_match::MatchLevel::Flexible
            ),
            request,
            "finalized media duration must not replace catalog request identity"
        );
        crate::compilation_excerpt::restore_requested_track_duration_for_excerpt_asset(
            &mut asset, &requested,
        );
        assert_eq!(asset.track.duration_seconds, Some(180.0));
        assert_eq!(asset.duration_seconds, Some(179.4));

        store.save_flexible_excerpt_asset(&asset, &request).unwrap();
        let hydrated = store.get_track(&requested.id).unwrap().unwrap();
        assert_eq!(hydrated.local_path.as_deref(), Some(path.to_str().unwrap()));
        assert_eq!(
            hydrated.provenance["approximateExcerpt"]["relationship"],
            "probable"
        );
        let raw_after: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&requested.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            raw_after, raw_before,
            "excerpt proof must not rewrite the catalog listing"
        );

        for level in [
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchLevel::Exact,
        ] {
            settings.matching_level = level;
            store.save_settings(&settings).unwrap();
            let hidden = store.get_track(&requested.id).unwrap().unwrap();
            assert!(hidden.local_path.is_none());
            assert!(hidden.provenance.get("approximateExcerpt").is_none());
            let stale_queued =
                crate::playback_sources::reconcile_stale_local_asset(&store, &hydrated, false)
                    .unwrap();
            assert!(stale_queued.local_path.is_none());
            assert!(stale_queued.asset_id.is_none());
            assert!(stale_queued.provenance.get("approximateExcerpt").is_none());
            assert!(store.get_asset_by_id(&asset.id).unwrap().is_some());
        }

        let mut stale_asset = asset.clone();
        stale_asset.id = "asset-stale-policy".into();
        assert!(store
            .save_flexible_excerpt_asset(&stale_asset, &request)
            .is_err());
        assert!(store.get_asset_by_id(&stale_asset.id).unwrap().is_none());
        assert_eq!(raw_after, raw_before);
    }

    #[test]
    fn engine_switch_checkpoint_rolls_back_all_three_preferences_on_failure() {
        let (_directory, store) = db();
        let queue = serde_json::json!({"tracks":[],"entryIds":[],"queueRevision":1});
        let session =
            serde_json::json!({"index":0,"currentEntryId":null,"queueRevision":1,"position":12.5});
        store
            .save_playback_checkpoint_and_engine(
                Some(&queue),
                Some(&session),
                Some(&serde_json::json!("webview")),
            )
            .unwrap();
        store.lock_connection().unwrap().execute_batch("CREATE TRIGGER reject_engine_switch BEFORE UPDATE ON settings WHEN NEW.key = 'playbackEngine.v1' BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;").unwrap();
        let changed_queue = serde_json::json!({"tracks":[],"entryIds":[],"queueRevision":2});
        let changed_session = serde_json::json!({"queueRevision":2,"position":99});
        assert!(store
            .save_playback_checkpoint_and_engine(
                Some(&changed_queue),
                Some(&changed_session),
                Some(&serde_json::json!("native"))
            )
            .is_err());
        assert_eq!(
            store.get_json_preference("nativePlaybackQueue.v1").unwrap(),
            Some(queue)
        );
        assert_eq!(
            store
                .get_json_preference("nativePlaybackSession.v1")
                .unwrap(),
            Some(session)
        );
        assert_eq!(
            store.get_json_preference("playbackEngine.v1").unwrap(),
            Some(serde_json::json!("webview"))
        );
    }

    #[test]
    fn playback_checkpoint_commits_occurrences_and_session_as_one_pair() {
        let (directory, store) = db();
        let queue = serde_json::json!({
            "tracks": [track("repeat", ""), track("repeat", "")],
            "entryIds": ["first", "second"],
            "queueRevision": 7
        });
        let session = serde_json::json!({
            "index": 1, "currentEntryId": "second", "queueRevision": 7,
            "position": 12.5, "finished": false
        });
        store
            .save_playback_checkpoint(Some(&queue), Some(&session))
            .unwrap();
        drop(store);
        let reopened = Store::open(&directory.path().join("library.sqlite")).unwrap();
        assert_eq!(
            reopened
                .get_json_preference("nativePlaybackQueue.v1")
                .unwrap(),
            Some(queue.clone())
        );
        assert_eq!(
            reopened
                .get_json_preference("nativePlaybackSession.v1")
                .unwrap(),
            Some(session)
        );
        let updated_session = serde_json::json!({"index": 1, "position": 15});
        reopened
            .save_playback_checkpoint(None, Some(&updated_session))
            .unwrap();
        assert_eq!(
            reopened
                .get_json_preference("nativePlaybackQueue.v1")
                .unwrap(),
            Some(queue)
        );
        assert_eq!(
            reopened
                .get_json_preference("nativePlaybackSession.v1")
                .unwrap(),
            Some(updated_session)
        );
    }

    #[test]
    fn playback_checkpoint_write_failure_rolls_back_both_components() {
        let (_directory, store) = db();
        let queue = serde_json::json!({"tracks": ["old"], "queueRevision": 1});
        let session = serde_json::json!({"index": 0, "queueRevision": 1});
        store
            .save_playback_checkpoint(Some(&queue), Some(&session))
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_playback_session_update
             BEFORE UPDATE ON settings WHEN NEW.key='nativePlaybackSession.v1'
             BEGIN SELECT RAISE(ABORT, 'simulated checkpoint failure'); END;",
            )
            .unwrap();
        let error = store
            .save_playback_checkpoint(
                Some(&serde_json::json!({"tracks": ["new"], "queueRevision": 2})),
                Some(&serde_json::json!({"index": 1, "queueRevision": 2})),
            )
            .unwrap_err();
        assert!(error.contains("simulated checkpoint failure"));
        assert_eq!(
            store.get_json_preference("nativePlaybackQueue.v1").unwrap(),
            Some(queue)
        );
        assert_eq!(
            store
                .get_json_preference("nativePlaybackSession.v1")
                .unwrap(),
            Some(session)
        );
    }

    #[test]
    fn invalid_playback_timeout_does_not_replace_saved_settings() {
        let (_directory, store) = db();
        let mut settings = Settings {
            playback_startup_timeout_seconds: 30,
            ..Settings::default()
        };
        store.save_settings(&settings).unwrap();
        for invalid in [0, 21, 300, u32::MAX] {
            settings.playback_startup_timeout_seconds = invalid;
            assert!(store.save_settings(&settings).is_err());
            assert_eq!(
                store.settings().unwrap().playback_startup_timeout_seconds,
                30
            );
        }
    }

    #[test]
    fn exact_cached_mcp_pages_match_legacy_across_pending_ready_and_file_changes() {
        let (dir, store) = db();
        let mut sources = Vec::new();
        let mut paths = Vec::new();
        for index in 0..4 {
            let mut source = track(&format!("mcp-parity-{index}"), "");
            source.title = if index < 2 {
                "Älpha".into()
            } else {
                "Beta".into()
            };
            source.duration_seconds = Some(20.5 + index as f64);
            store.save_track(&source).unwrap();
            store.set_favorite(&source, index < 2).unwrap();
            if index < 3 {
                let path = dir.path().join(format!("parity-{index}.flac"));
                std::fs::write(&path, b"fixture").unwrap();
                let mut tags = source.clone();
                tags.bpm = (index < 2).then_some(100.);
                tags.key = (index < 2).then(|| "8A".into());
                store
                    .save_asset(&asset(
                        &format!("parity-asset-{index}"),
                        &tags,
                        &path,
                        &format!("parity-hash-{index}"),
                    ))
                    .unwrap();
                paths.push(path);
            }
            sources.push(source);
        }
        let preferred_path = dir.path().join("parity-preferred.flac");
        std::fs::write(&preferred_path, b"preferred fixture").unwrap();
        let mut preferred_tags = sources[2].clone();
        preferred_tags.bpm = Some(151.);
        preferred_tags.key = Some("9A".into());
        store
            .save_asset(&asset(
                "parity-preferred",
                &preferred_tags,
                &preferred_path,
                "parity-preferred-hash",
            ))
            .unwrap();
        store
            .set_preferred_track_file(&sources[2].id, "parity-preferred")
            .unwrap();
        let collection = store.create_collection("MCP occurrence parity").unwrap();
        store
            .append_playlist_entries(
                &collection.id,
                &[
                    sources[0].clone(),
                    sources[1].clone(),
                    sources[0].clone(),
                    sources[2].clone(),
                    sources[3].clone(),
                ],
            )
            .unwrap();
        for phase in 0..3 {
            if phase == 1 {
                while store.backfill_sort_availability_batch(20).unwrap() != 0 {}
            }
            if phase == 2 {
                std::fs::remove_file(&paths[0]).unwrap();
                std::fs::remove_file(&preferred_path).unwrap();
                let mut edited = store.get_track(&sources[1].id).unwrap().unwrap();
                edited.title = "Manual needle".into();
                edited.bpm = None;
                store.update_track(&edited).unwrap();
            }
            for (view, collection_id) in [
                ("library", None),
                ("favorites", None),
                ("collection", Some(collection.id.as_str())),
            ] {
                for field in [
                    "title",
                    "artist",
                    "album",
                    "bpm",
                    "key",
                    "duration",
                    "sourceOrder",
                ] {
                    for direction in ["asc", "desc"] {
                        for offset in [0, 1, 20] {
                            let old = store
                                .list_tracks_page(
                                    view,
                                    collection_id,
                                    2,
                                    offset,
                                    None,
                                    Some(field),
                                    Some(direction),
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .unwrap();
                            let new = store
                                .list_tracks_page_exact_cached(
                                    view,
                                    collection_id,
                                    2,
                                    offset,
                                    None,
                                    Some(field),
                                    Some(direction),
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .unwrap();
                            assert_eq!(
                                new, old,
                                "phase={phase} view={view} field={field} direction={direction} offset={offset}"
                            );
                        }
                    }
                }
            }
            for (query, availability, min, max) in [
                (Some("älp"), None, None, None),
                (Some("needle"), None, None, None),
                (None, Some("local"), None, None),
                (None, Some("not-local"), Some(20), Some(22)),
            ] {
                let old = store
                    .list_tracks_page(
                        "library",
                        None,
                        2,
                        0,
                        query,
                        Some("bpm"),
                        Some("desc"),
                        min,
                        max,
                        availability,
                        None,
                    )
                    .unwrap();
                let new = store
                    .list_tracks_page_exact_cached(
                        "library",
                        None,
                        2,
                        0,
                        query,
                        Some("bpm"),
                        Some("desc"),
                        min,
                        max,
                        availability,
                        None,
                    )
                    .unwrap();
                assert_eq!(new, old, "phase={phase} filtered query");
            }
        }
    }

    #[test]
    fn exact_cached_mcp_ready_path_skips_single_asset_materialization_and_pending_falls_back() {
        let (dir, store) = db();
        let source = track("mcp-routing", "");
        store.save_track(&source).unwrap();
        let path = dir.path().join("mcp-routing.flac");
        std::fs::write(&path, b"fixture").unwrap();
        let mut tags = source.clone();
        tags.bpm = Some(123.);
        store
            .save_asset(&asset(
                "mcp-routing-asset",
                &tags,
                &path,
                "mcp-routing-hash",
            ))
            .unwrap();
        let materialized = || {
            store
                .connection
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM page_effective_metadata", [], |row| {
                    row.get::<_, usize>(0)
                })
                .unwrap()
        };
        let read_new = || {
            store
                .list_tracks_page_exact_cached(
                    "library",
                    None,
                    10,
                    0,
                    None,
                    Some("bpm"),
                    Some("asc"),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap()
        };
        let cold = read_new();
        assert_eq!(cold.0[0].bpm, Some(123.));
        assert_eq!(cold.1, 1);
        assert_eq!(
            materialized(),
            1,
            "pending must synchronously preserve legacy exact rows"
        );
        while store.backfill_sort_availability_batch(20).unwrap() != 0 {}
        let ready = read_new();
        assert_eq!(ready, cold);
        assert_eq!(
            materialized(),
            0,
            "ready must actually avoid per-single-asset materialization"
        );
        let old = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                None,
                Some("bpm"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(ready, old);
        assert_eq!(materialized(), 1);
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE catalog_listings SET sort_projection_revision=-1 WHERE id=?1",
                [&source.id],
            )
            .unwrap();
        assert_eq!(read_new(), old);
        assert_eq!(materialized(), 1, "stale revision cannot use cached rows");
        // Duration bounds do not have a projection-pending gate: stay legacy.
        let old = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                None,
                Some("duration"),
                Some("desc"),
                Some(0),
                Some(300),
                None,
                None,
            )
            .unwrap();
        let new = store
            .list_tracks_page_exact_cached(
                "library",
                None,
                10,
                0,
                None,
                Some("duration"),
                Some("desc"),
                Some(0),
                Some(300),
                None,
                None,
            )
            .unwrap();
        assert_eq!(new, old);
        assert_eq!(materialized(), 1);
    }

    #[test]
    fn append_playlist_entries_preserves_duplicates_existing_members_and_paged_order() {
        let (_dir, store) = db();
        let first = track("append-first", "");
        let second = track("append-second", "");
        let collection = store.create_collection("Occurrence append").unwrap();
        store
            .add_to_collection(&collection.id, &[first.clone()])
            .unwrap();
        store.cache_track(&second).unwrap();
        let mut manual = first.clone();
        manual.title = "Manual title".into();
        store.update_track(&manual).unwrap();
        store
            .append_playlist_entries(
                &collection.id,
                &[first.clone(), second.clone(), first.clone()],
            )
            .unwrap();
        let listed = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(
            listed.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![
                first.id.as_str(),
                first.id.as_str(),
                second.id.as_str(),
                first.id.as_str()
            ]
        );
        assert!(listed
            .iter()
            .filter(|row| row.id == first.id)
            .all(|row| row.title == "Manual title"));
        for offset in [0, 2] {
            let (page, total) = store
                .list_tracks_page(
                    "collection",
                    Some(&collection.id),
                    2,
                    offset,
                    Some(""),
                    Some("sourceOrder"),
                    Some("asc"),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(total, 4);
            assert_eq!(
                page.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
                listed[offset..offset + 2]
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>()
            );
        }
        let conn = store.connection.lock().unwrap();
        let members: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_members WHERE collection_id=?1",
                [&collection.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(members, 2);
        let promoted: bool = conn
            .query_row(
                "SELECT in_library FROM catalog_listings WHERE id=?1",
                [&second.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(promoted);
        drop(conn);
        assert_eq!(
            store
                .list_collections()
                .unwrap()
                .into_iter()
                .find(|value| value.id == collection.id)
                .unwrap()
                .track_count,
            4
        );
        // An already ordered collection appends again rather than replacing prior occurrences.
        store
            .append_playlist_entries(&collection.id, &[second])
            .unwrap();
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap()
                .len(),
            5
        );
        let range = crate::models::PlaybackRange {
            parent_track_id: first.id.clone(),
            chapter_id: "excerpt-10-25".into(),
            source_url: "https://example.test/parent".into(),
            start_time: 10.0,
            end_time: 25.0,
            parent_title: first.title.clone(),
            parent_duration_seconds: Some(120.0),
        };
        let chapter = Track {
            id: "chapter:append-excerpt".into(),
            provider: "chapter".into(),
            provider_id: "append-excerpt".into(),
            title: "Distinct excerpt".into(),
            duration_seconds: Some(15.0),
            playback_range: Some(range.clone()),
            ..first.clone()
        };
        store
            .append_playlist_entries(&collection.id, &[chapter.clone(), chapter.clone()])
            .unwrap();
        let with_chapters = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(with_chapters.len(), 7);
        for row in &with_chapters[5..] {
            assert_eq!(row.id, chapter.id);
            assert_ne!(row.id, first.id);
            assert_eq!(row.playback_range.as_ref(), Some(&range));
            assert_eq!(row.duration_seconds, Some(15.0));
        }
    }
    #[test]
    fn append_playlist_entries_keeps_legacy_representative_and_alternate_occurrences() {
        let (_dir, store) = db();
        let canonical = track("canonical-append", "");
        let alternate = track("alternate-append", "");
        store.save_track(&canonical).unwrap();
        store.cache_track(&alternate).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "UPDATE catalog_listings SET recording_id=?1 WHERE id=?2",
                params![canonical.id, alternate.id],
            )
            .unwrap();
        }
        let collection = store.create_collection("Alternate occurrences").unwrap();
        store
            .add_to_collection(&collection.id, &[alternate.clone()])
            .unwrap();
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap()[0]
                .id,
            canonical.id
        );
        store
            .append_playlist_entries(&collection.id, &[alternate.clone(), alternate.clone()])
            .unwrap();
        let listed = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(
            listed.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![
                canonical.id.as_str(),
                alternate.id.as_str(),
                alternate.id.as_str()
            ]
        );
        // Existing ordered positions/IDs survive a failed append and a subsequent successful one.
        {
            let conn = store.connection.lock().unwrap();
            conn.execute("UPDATE playlist_ordered_entries SET position=8 WHERE collection_id=?1 AND position=2",[&collection.id]).unwrap();
        }
        let fresh = track("rollback-ordered", "");
        let invalid = Track {
            id: "invalid".into(),
            ..fresh.clone()
        };
        assert!(store
            .append_playlist_entries(&collection.id, &[fresh.clone(), invalid])
            .is_err());
        assert!(store.get_track(&fresh.id).unwrap().is_none());
        store
            .append_playlist_entries(&collection.id, &[canonical.clone()])
            .unwrap();
        let conn = store.connection.lock().unwrap();
        let rows=conn.prepare("SELECT position,listing_id FROM playlist_ordered_entries WHERE collection_id=?1 ORDER BY position").unwrap().query_map([&collection.id],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(
            rows,
            vec![
                (0, canonical.id.clone()),
                (1, alternate.id.clone()),
                (8, alternate.id),
                (9, canonical.id)
            ]
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn append_playlist_entries_empty_is_noop_and_invalid_batch_rolls_back() {
        let (_dir, store) = db();
        let collection = store.create_collection("Empty append").unwrap();
        store.append_playlist_entries(&collection.id, &[]).unwrap();
        let ordered: i64 = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM playlist_ordered_collections WHERE collection_id=?1",
                [&collection.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ordered, 0);
        let good = track("rollback-new", "");
        let bad = Track {
            id: "invalid".into(),
            ..good.clone()
        };
        assert!(store
            .append_playlist_entries(&collection.id, &[good.clone(), bad])
            .is_err());
        assert!(store.get_track(&good.id).unwrap().is_none());
        assert!(store
            .list_tracks("collection", Some(&collection.id))
            .unwrap()
            .is_empty());
        store.delete_collection(&collection.id).unwrap();
        assert!(store
            .append_playlist_entries(&collection.id, &[good.clone()])
            .is_err());
        assert!(store.append_playlist_entries("missing", &[good]).is_err());
    }
    #[test]
    fn append_playlist_entries_rejects_system_membership_targets() {
        let (_dir, store) = db();
        let favorites = store.create_collection("Favorite target").unwrap();
        store
            .set_json_preference(
                "playlistPreferences.v1",
                &serde_json::json!({"favoritesPlaylistId":favorites.id}),
            )
            .unwrap();
        assert!(store
            .append_playlist_entries(&favorites.id, &[track("system-rejected", "")])
            .is_err());
        assert!(store
            .get_track("fixture:system-rejected")
            .unwrap()
            .is_none());
        // Existing membership operations remain idempotent, including for Favorites.
        let item = track("system-accepted", "");
        store
            .add_to_collection(&favorites.id, &[item.clone(), item])
            .unwrap();
        assert_eq!(
            store
                .list_tracks("collection", Some(&favorites.id))
                .unwrap()
                .len(),
            1
        );
        let downloads = store.ensure_downloads_collection().unwrap();
        assert!(store
            .append_playlist_entries(&downloads.id, &[track("download-rejected", "")])
            .is_err());
    }

    fn assert_batch_matches_individual(conn: &Connection, tracks: &[Track]) -> Vec<Track> {
        let individual = tracks
            .iter()
            .cloned()
            .map(|track| hydrate(conn, track))
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let batched = hydrate_page_tracks(conn, tracks.to_vec()).unwrap();
        assert_eq!(batched, individual);
        batched
    }

    #[test]
    fn page_batch_hydration_preserves_manual_nulls_autotags_and_file_preference_fallbacks() {
        let (dir, store) = db();
        let mut source = track("batch-overlays", "Original");
        source.genre = Some("Deep House".into());
        store.save_track(&source).unwrap();
        let missing = dir.path().join("missing.flac");
        let first = dir.path().join("first.flac");
        let preferred = dir.path().join("preferred.flac");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&preferred, b"preferred").unwrap();
        store
            .save_asset(&asset("batch-missing", &source, &missing, "missing-hash"))
            .unwrap();
        store
            .save_asset(&asset("batch-first", &source, &first, "first-hash"))
            .unwrap();
        let mut tagged = asset("batch-preferred", &source, &preferred, "preferred-hash");
        tagged.track.title = "Autotagged title".into();
        tagged.track.album = Some("Autotagged album".into());
        tagged.track.bpm = Some(129.0);
        tagged.track.key = Some("9A".into());
        tagged.track.provenance = serde_json::json!({"autotag":{"source":"fixture"}});
        store.save_asset(&tagged).unwrap();
        let conn = store.connection.lock().unwrap();
        conn.execute(
            "UPDATE recordings SET favorite=1,disposition='shortlist',manual_metadata=?1 WHERE id=?2",
            params![
                serde_json::json!({"title":"Manual title","bpm":null,"key":null}).to_string(),
                source.id
            ],
        )
        .unwrap();
        let setting = format!("preferredAsset.{}", source.id);
        conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)",
            params![setting, encode(&"batch-preferred").unwrap()],
        )
        .unwrap();
        let result = assert_batch_matches_individual(&conn, &[source.clone(), source.clone()]);
        assert_eq!(result[0].asset_id.as_deref(), Some("batch-preferred"));
        assert_eq!(result[0].title, "Manual title");
        assert_eq!(result[0].album.as_deref(), Some("Autotagged album"));
        assert_eq!(result[0].normalized_genre.as_deref(), Some("deep-house"));
        assert!(result[0].favorite);
        assert_eq!(result[0].disposition.as_deref(), Some("shortlist"));
        assert_eq!((result[0].bpm, result[0].key.clone()), (None, None));
        assert_eq!(result[0].provenance["manualFields"]["bpm"], true);
        // Preference IDs outside this recording never bypass row-order fallback.
        conn.execute(
            "UPDATE settings SET value=?1 WHERE key=?2",
            params![encode(&"unrelated-asset").unwrap(), setting],
        )
        .unwrap();
        assert_eq!(
            assert_batch_matches_individual(&conn, &[source.clone()])[0]
                .asset_id
                .as_deref(),
            Some("batch-first")
        );
        conn.execute(
            "UPDATE settings SET value=?1 WHERE key=?2",
            params![encode(&"batch-preferred").unwrap(), setting],
        )
        .unwrap();
        std::fs::remove_file(&preferred).unwrap();
        assert_eq!(
            assert_batch_matches_individual(&conn, &[source.clone()])[0]
                .asset_id
                .as_deref(),
            Some("batch-first")
        );
        std::fs::remove_file(&first).unwrap();
        let missing = assert_batch_matches_individual(&conn, &[source]);
        assert_eq!(missing[0].asset_id.as_deref(), Some("batch-missing"));
        assert_eq!(missing[0].local_availability, "missing");
        assert!(missing[0].local_path.is_none());
    }

    #[test]
    fn page_batch_hydration_uses_exact_selected_asset_hash_and_latest_analysis() {
        let (dir, store) = db();
        let source = track("batch-analysis", "");
        store.save_track(&source).unwrap();
        let path = dir.path().join("analysis.flac");
        std::fs::write(&path, b"analysis").unwrap();
        store
            .save_asset(&asset(
                "batch-analysis-asset",
                &source,
                &path,
                "current-hash",
            ))
            .unwrap();
        let conn = store.connection.lock().unwrap();
        for (hash, version, status, payload, updated) in [
            (
                "old-hash",
                "stale",
                "completed",
                "INVALID STALE JSON",
                "2099-01-01",
            ),
            (
                "current-hash",
                "older",
                "completed",
                r#"{"bpm":110,"key":"1A"}"#,
                "2025-01-01",
            ),
            (
                "current-hash",
                "latest",
                "completed",
                r#"{"bpm":128,"key":"8A","provenance":{"source":"analysis"}}"#,
                "2026-01-01",
            ),
        ] {
            conn.execute("INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES('batch-analysis-asset',?1,?2,?3,?4,?5)", params![hash,version,status,payload,updated]).unwrap();
        }
        let result = assert_batch_matches_individual(&conn, &[source.clone()]);
        assert_eq!(result[0].bpm, Some(128.0));
        assert_eq!(result[0].key.as_deref(), Some("8A"));
        assert_eq!(result[0].provenance["localAnalysis"]["version"], "latest");
        conn.execute(
            "UPDATE asset_analyses SET status='running' WHERE version='latest'",
            [],
        )
        .unwrap();
        let running = assert_batch_matches_individual(&conn, &[source.clone()]);
        assert!(running[0].bpm.is_none());
        assert!(running[0].key.is_none());
        assert_eq!(running[0].provenance["localAnalysis"]["status"], "running");
        // Equal timestamps have no defined winner; compare these executions of
        // the same ORDER BY updated_at DESC policy without asserting a version.
        conn.execute(
            "UPDATE asset_analyses SET updated_at='2026-01-01' WHERE file_hash='current-hash'",
            [],
        )
        .unwrap();
        assert_batch_matches_individual(&conn, &[source.clone()]);
        conn.execute("UPDATE asset_analyses SET status='completed',payload='null' WHERE file_hash='current-hash'", []).unwrap();
        let null = assert_batch_matches_individual(&conn, &[source]);
        assert!(null[0].bpm.is_none());
        assert!(null[0].key.is_none());
        assert!(null[0].provenance["localAnalysis"]["provenance"].is_null());
    }

    #[test]
    fn page_batch_hydration_preserves_raw_chapter_occurrences_order_and_aliases_across_chunks() {
        let (_dir, store) = db();
        let source = track("batch-occurrences", "");
        let alias = track("batch-alias", "");
        store.save_track(&source).unwrap();
        store.save_track(&alias).unwrap();
        store
            .merge_recordings(&source.id, &[alias.id.clone()])
            .unwrap();
        let mut occurrences = Vec::new();
        for index in 0..405 {
            let mut occurrence = if index % 2 == 0 {
                source.clone()
            } else {
                alias.clone()
            };
            occurrence.title = format!("Chapter occurrence {index}");
            occurrence.provenance = serde_json::json!({"chapters":[{"id":format!("chapter-{index}"),"startTime":index,"endTime":index+1}],"rawOccurrence":index});
            occurrence.playback_range = Some(PlaybackRange {
                parent_track_id: source.id.clone(),
                chapter_id: format!("chapter-{index}"),
                source_url: "https://example.test/parent".into(),
                start_time: index as f64,
                end_time: (index + 1) as f64,
                parent_title: "Parent".into(),
                parent_duration_seconds: Some(1000.0),
            });
            occurrences.push(occurrence);
        }
        let conn = store.connection.lock().unwrap();
        let hydrated = assert_batch_matches_individual(&conn, &occurrences);
        assert_eq!(hydrated.len(), 405);
        for (index, track) in hydrated.iter().enumerate() {
            assert_eq!(track.title, format!("Chapter occurrence {index}"));
            assert_eq!(track.provenance["rawOccurrence"], index);
            assert_eq!(
                track.playback_range.as_ref().unwrap().chapter_id,
                format!("chapter-{index}")
            );
        }
        assert!(hydrate_page_tracks(&conn, Vec::new()).unwrap().is_empty());
    }

    #[test]
    fn page_batch_hydration_retains_manual_identity_lookup_and_invalid_preference_errors() {
        let (dir, store) = db();
        let source = track("batch-manual-id", "");
        let target = track("batch-manual-target", "");
        let empty = track("batch-no-assets", "");
        for track in [&source, &target, &empty] {
            store.save_track(track).unwrap();
        }
        let path = dir.path().join("manual-target.flac");
        std::fs::write(&path, b"target").unwrap();
        store
            .save_asset(&asset("manual-target-asset", &target, &path, "target-hash"))
            .unwrap();
        let conn = store.connection.lock().unwrap();
        conn.execute(
            "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
            params![serde_json::json!({"id":target.id}).to_string(), source.id],
        )
        .unwrap();
        let result = assert_batch_matches_individual(&conn, &[source]);
        assert_eq!(result[0].id, target.id);
        assert_eq!(result[0].asset_id.as_deref(), Some("manual-target-asset"));
        conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,'null')",
            [format!("preferredAsset.{}", empty.id)],
        )
        .unwrap();
        assert!(hydrate(&conn, empty.clone()).is_err());
        assert!(hydrate_page_tracks(&conn, vec![empty]).is_err());
    }

    #[test]
    fn page_batch_hydration_collection_page_retains_duplicate_positions_and_source_json() {
        let (_dir, store) = db();
        let mut first = track("batch-page-one", "");
        first.provenance = serde_json::json!({"position":17,"chapters":[{"title":"First","startTime":0,"endTime":60}]});
        let second = track("batch-page-two", "");
        store.save_track(&first).unwrap();
        store.save_track(&second).unwrap();
        let collection = store.create_collection("Batch occurrence page").unwrap();
        store
            .replace_playlist_entries(
                &collection.id,
                &[second.clone(), first.clone(), first.clone()],
            )
            .unwrap();
        let individual = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        let (batched, total) = store
            .list_tracks_page(
                "collection",
                Some(&collection.id),
                200,
                0,
                None,
                Some("sourceOrder"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(batched, individual);
        assert_eq!(
            batched
                .iter()
                .map(|track| track.provenance["playlistPosition"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            batched[1].provenance["chapters"],
            first.provenance["chapters"]
        );
        assert_eq!(
            store.get_track(&first.id).unwrap().unwrap().provenance["position"],
            17
        );
    }

    #[test]
    fn stale_projection_partial_index_bounds_pending_and_multiasset_discovery() {
        let (directory, store) = db();
        let canonical = track("stale-index-canonical", "");
        let alias = track("stale-index-alias", "");
        store.save_track(&canonical).unwrap();
        store.save_track(&alias).unwrap();
        store
            .merge_recordings(&canonical.id, std::slice::from_ref(&alias.id))
            .unwrap();

        let one_path = directory.path().join("stale-index-one.flac");
        let two_path = directory.path().join("stale-index-two.flac");
        std::fs::write(&one_path, b"one").unwrap();
        std::fs::write(&two_path, b"two").unwrap();
        let mut local = canonical.clone();
        local.bpm = Some(128.0);
        let one = asset(
            "stale-index-asset-one",
            &local,
            &one_path,
            "stale-index-hash-one",
        );
        store.save_asset(&one).unwrap();

        let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
        let pending_sql = super::projection_pending_single_asset_sql(revision);
        let multi_sql = format!(
            "SELECT recording_id FROM ({}) AS multi_asset_recordings ORDER BY recording_id",
            super::stale_multi_asset_recordings_sql(revision)
        );
        let index_name = super::sort_projection_stale_index_name(revision);
        {
            let conn = store.connection.lock().unwrap();
            let pending: bool = conn.query_row(&pending_sql, [], |row| row.get(0)).unwrap();
            assert!(pending, "one stale asset needs bounded projection backfill");
            let mut statement = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {pending_sql}"))
                .unwrap();
            let plan = statement
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                plan.iter().any(|step| step.contains(&index_name)),
                "pending discovery must use the revision-qualified stale index: {plan:?}"
            );
        }
        assert_eq!(store.backfill_sort_availability_batch(20).unwrap(), 2);
        {
            let conn = store.connection.lock().unwrap();
            let pending: bool = conn.query_row(&pending_sql, [], |row| row.get(0)).unwrap();
            assert!(
                !pending,
                "a completed one-asset projection leaves the stale index"
            );
        }

        let two = asset(
            "stale-index-asset-two",
            &local,
            &two_path,
            "stale-index-hash-two",
        );
        store.save_asset(&two).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            let stale_aliases: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM ({})",
                        super::stale_projection_recordings_sql(revision)
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stale_aliases, 1, "merged listings deduplicate by recording");
            let mut statement = conn.prepare(&multi_sql).unwrap();
            let dynamic_ids = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(dynamic_ids, [canonical.id.clone()]);
            let mut statement = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {multi_sql}"))
                .unwrap();
            let plan = statement
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                plan.iter().any(|step| step.contains(&index_name)),
                "multi-asset discovery must use the stale index: {plan:?}"
            );
        }
        let (page, _, _, pending) = store
            .list_tracks_page_with_sort_state(
                "library",
                None,
                20,
                0,
                None,
                Some("bpm"),
                Some("asc"),
                None,
                None,
                Some("local"),
                None,
            )
            .unwrap();
        assert!(!pending);
        assert!(
            page.iter().any(|track| track.id == canonical.id),
            "the live multi-asset path must retain the canonical library listing"
        );
        let effective_ids = {
            let conn = store.connection.lock().unwrap();
            let mut statement = conn
                .prepare("SELECT id FROM page_effective_metadata ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            effective_ids,
            [alias.id.clone(), canonical.id.clone()],
            "dynamic effective metadata must retain both merged listing rows"
        );

        let collection = store
            .create_collection("stale index ordered aliases")
            .unwrap();
        store
            .replace_playlist_entries(
                &collection.id,
                &[canonical.clone(), alias.clone(), alias.clone()],
            )
            .unwrap();
        let (ordered_page, total, _, pending) = store
            .list_tracks_page_with_sort_state(
                "collection",
                Some(&collection.id),
                20,
                0,
                None,
                Some("bpm"),
                Some("asc"),
                None,
                None,
                Some("local"),
                None,
            )
            .unwrap();
        assert!(!pending);
        assert_eq!(total, 3);
        assert_eq!(
            ordered_page
                .iter()
                .map(|track| (track.id.as_str(), track.bpm))
                .collect::<Vec<_>>(),
            [
                (canonical.id.as_str(), Some(128.0)),
                (alias.id.as_str(), Some(128.0)),
                (alias.id.as_str(), Some(128.0)),
            ],
            "dynamic BPM metadata must preserve ordered alias occurrences and stable ties"
        );

        store
            .connection
            .lock()
            .unwrap()
            .execute("DELETE FROM local_assets WHERE id=?1", [&two.id])
            .unwrap();
        {
            let conn = store.connection.lock().unwrap();
            let pending: bool = conn.query_row(&pending_sql, [], |row| row.get(0)).unwrap();
            assert!(
                pending,
                "asset deletion invalidates the surviving one-asset projection"
            );
        }
    }

    #[test]
    fn no_op_metadata_and_asset_updates_preserve_sort_projections_after_reopen() {
        let (directory, store) = db();
        let mut source = track("projection-no-op", "");
        source.album = Some("Stable album".into());
        store.save_track(&source).unwrap();
        let path = directory.path().join("projection-no-op.flac");
        std::fs::write(&path, b"fixture").unwrap();
        let local = asset(
            "projection-no-op-asset",
            &source,
            &path,
            "projection-no-op-hash",
        );
        store.save_asset(&local).unwrap();
        store
            .set_preferred_track_file(&source.id, &local.id)
            .unwrap();
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,'fixture-v1','completed','{}','2026-01-01')",
                    params![local.id, local.hash],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO asset_autotags(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,'fixture-v1','completed','{}','2026-01-01')",
                    params![local.id, local.hash],
                )
                .unwrap();
        }
        store.backfill_sort_availability_batch(500).unwrap();
        drop(store);

        // An existing v9 database may carry the old broad trigger definitions.
        // Opening it must replace them instead of leaving IF NOT EXISTS versions in place.
        let path = directory.path().join("library.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP TRIGGER sort_availability_listing_update;
                 CREATE TRIGGER sort_availability_listing_update
                 AFTER UPDATE OF metadata,recording_id ON catalog_listings BEGIN
                   UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,
                     sort_metadata_projection=NULL,sort_projection_revision=NULL WHERE id=NEW.id;
                 END;",
            )
            .unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        let read_snapshot = || {
            store
                .connection
                .lock()
                .unwrap()
                .query_row(
                    "SELECT sort_availability_mask,sort_availability_revision,sort_metadata_projection,sort_projection_revision FROM catalog_listings WHERE id=?1",
                    [&source.id],
                    |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<i64>>(3)?)),
                )
                .unwrap()
        };
        let ready = read_snapshot();
        assert!(
            ready.0.is_some() && ready.2.is_some(),
            "fixture projection was backfilled"
        );

        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE catalog_listings SET metadata=metadata,recording_id=recording_id WHERE id=?1",
                    [&source.id],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE recordings SET manual_metadata=manual_metadata WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?1)",
                    [&source.id],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE local_assets SET recording_id=recording_id,path=path,hash=hash,payload=payload WHERE id=?1",
                    [&local.id],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE settings SET value=value WHERE key=?1",
                    [format!("preferredAsset.{}", source.id)],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE asset_analyses SET status=status,payload=payload,updated_at=updated_at WHERE asset_id=?1",
                    [&local.id],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE asset_autotags SET status=status,payload=payload,updated_at=updated_at WHERE asset_id=?1",
                    [&local.id],
                )
                .unwrap();
        }
        store.set_favorite(&source, true).unwrap();
        store.set_favorite(&source, true).unwrap();
        assert_eq!(read_snapshot(), ready);
    }

    #[test]
    fn promoting_a_merged_cached_alias_invalidates_the_new_date_projection() {
        let (_directory, store) = db();
        let canonical = track("projection-promotion-canonical", "");
        let alias = track("projection-promotion-alias", "");
        store.cache_track(&canonical).unwrap();
        store.cache_track(&alias).unwrap();
        store
            .merge_recordings(&canonical.id, &[alias.id.clone()])
            .unwrap();
        store.backfill_sort_availability_batch(500).unwrap();
        let before: (Option<i64>, Option<String>) = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT sort_availability_mask,sort_metadata_projection FROM catalog_listings WHERE id=?1",
                [&canonical.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(before.0.is_some() && before.1.is_some());

        store.set_favorite(&alias, true).unwrap();

        let (mask, projection, added_date): (Option<i64>, Option<String>, Option<String>) = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT sort_availability_mask,sort_metadata_projection,json_extract(metadata,'$.dateAdded') FROM catalog_listings WHERE id=?1",
                [&canonical.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(
            added_date.is_some(),
            "promotion supplies dateAdded to the canonical row"
        );
        assert_eq!(mask, None);
        assert_eq!(projection, None);
    }

    #[test]
    fn bounded_availability_backfill_skips_ready_and_multiasset_rows_before_eligible_rows() {
        let (directory, store) = db();
        let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;

        // This earliest row is made current before later stale candidates exist.
        let current = track("backfill-pushdown-current", "");
        store.save_track(&current).unwrap();
        assert_eq!(store.backfill_sort_availability_batch(1).unwrap(), 1);
        let read_state = |id: &str| {
            store.connection.lock().unwrap().query_row(
                "SELECT sort_availability_mask,sort_availability_revision,sort_metadata_projection,sort_projection_revision FROM catalog_listings WHERE id=?1",
                [id],
                |row| Ok((row.get::<_, Option<i64>>(0)?,row.get::<_, Option<i64>>(1)?,row.get::<_, Option<String>>(2)?,row.get::<_, Option<i64>>(3)?)),
            ).unwrap()
        };
        let current_before = read_state(&current.id);
        assert!(current_before.0.is_some() && current_before.2.is_some());

        // A stale two-asset recording has an earlier rowid than the batchable
        // candidates. It must be excluded without consuming the limit.
        let multi = track("backfill-pushdown-multi", "");
        store.save_track(&multi).unwrap();
        for index in 0..2 {
            let path = directory
                .path()
                .join(format!("backfill-pushdown-multi-{index}.flac"));
            std::fs::write(&path, b"multi asset").unwrap();
            store
                .save_asset(&asset(
                    &format!("backfill-pushdown-multi-asset-{index}"),
                    &multi,
                    &path,
                    &format!("backfill-pushdown-multi-hash-{index}"),
                ))
                .unwrap();
        }

        let zero = track("backfill-pushdown-zero", "");
        store.save_track(&zero).unwrap();

        let single = track("backfill-pushdown-single", "");
        store.save_track(&single).unwrap();
        let single_path = directory.path().join("backfill-pushdown-single.flac");
        std::fs::write(&single_path, b"single asset").unwrap();
        let mut single_tags = single.clone();
        single_tags.bpm = Some(123.5);
        single_tags.key = Some("8A".into());
        store
            .save_asset(&asset(
                "backfill-pushdown-single-asset",
                &single_tags,
                &single_path,
                "backfill-pushdown-single-hash",
            ))
            .unwrap();

        let nonlibrary = track("backfill-pushdown-nonlibrary", "");
        store.save_track(&nonlibrary).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "UPDATE catalog_listings SET in_library=0 WHERE id=?1",
                [&nonlibrary.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
                params![
                    r#"{"title":"Manual title","album":"Manual album"}"#,
                    nonlibrary.id
                ],
            )
            .unwrap();
        }

        let null_revision = track("backfill-pushdown-null-revision", "");
        store.save_track(&null_revision).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "UPDATE catalog_listings SET sort_availability_mask=0,sort_availability_revision=NULL,sort_metadata_projection='{}',sort_projection_revision=?1 WHERE id=?2",
                params![revision, null_revision.id],
            ).unwrap();
        }

        for _ in 0..4 {
            assert_eq!(store.backfill_sort_availability_batch(1).unwrap(), 1);
        }
        assert_eq!(store.backfill_sort_availability_batch(1).unwrap(), 0);
        assert_eq!(
            read_state(&current.id),
            current_before,
            "current rows stay untouched"
        );
        assert_eq!(
            read_state(&multi.id),
            (None, None, None, None),
            "multi-asset rows remain live and unpersisted"
        );

        for id in [&zero.id, &single.id, &nonlibrary.id, &null_revision.id] {
            let (mask, mask_revision, projection_json, projection_revision) = read_state(id);
            assert_eq!(
                mask_revision,
                Some(revision),
                "stale row {id} gets the current mask revision"
            );
            assert_eq!(
                projection_revision,
                Some(revision),
                "stale row {id} gets a projection revision"
            );
            let hydrated = store.get_track(id).unwrap().unwrap();
            let expected_projection = super::sort_metadata_projection(&hydrated);
            let expected_mask = crate::track_sort_sql::pack_available_fields(
                &crate::track_sort_sql::available_fields(&expected_projection),
            );
            assert_eq!(mask, Some(expected_mask), "effective mask for {id}");
            let cached: serde_json::Value =
                serde_json::from_str(&projection_json.unwrap()).unwrap();
            if id == &single.id {
                assert_eq!(
                    cached, expected_projection,
                    "single-asset effective projection"
                );
                assert_eq!(cached["bpm"], 123.5);
                assert_eq!(cached["key"], "8A");
            } else {
                assert_eq!(
                    cached["title"], hydrated.title,
                    "asset-free effective title"
                );
                if id == &nonlibrary.id {
                    assert_eq!(cached["title"], "Manual title");
                    assert_eq!(cached["album"], "Manual album");
                }
            }
        }
    }

    #[test]
    fn persisted_sort_masks_backfill_and_pending_is_scoped_to_exact_filtered_relation() {
        let (_dir, store) = db();
        let mut alpha = track("availability-alpha", "");
        alpha.title = "Alpha result".into();
        alpha.bpm = Some(120.0);
        let mut beta = track("availability-beta", "");
        beta.title = "Beta result".into();
        beta.album = Some("Known album".into());
        store.save_track(&alpha).unwrap();
        store.save_track(&beta).unwrap();

        let all = store
            .list_tracks_page_with_availability_state(
                "library",
                None,
                20,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .2
            .unwrap();
        assert!(matches!(all, SortAvailabilityState::Pending));
        assert_eq!(store.backfill_sort_availability_batch(1).unwrap(), 1);

        let alpha_only = store
            .list_tracks_page_with_availability_state(
                "library",
                None,
                20,
                0,
                Some("Alpha"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .2
            .unwrap();
        let SortAvailabilityState::Ready(fields) = alpha_only else {
            panic!("a filtered relation with current masks should be ready")
        };
        assert_eq!(fields.get("title"), Some(&true));

        // The second row remains pending, so only a relation that includes it
        // reports Pending. This prevents partial/global false disablement.
        let all_again = store
            .list_tracks_page_with_availability_state(
                "library",
                None,
                20,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .2
            .unwrap();
        assert!(matches!(all_again, SortAvailabilityState::Pending));

        store.backfill_sort_availability_batch(500).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
                params![r#"{"album":"Manual album"}"#, beta.id],
            )
            .unwrap();
        }
        let invalidated: Option<i64> = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT sort_availability_mask FROM catalog_listings WHERE id=?1",
                [&beta.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(invalidated, None);
    }

    #[test]
    fn single_asset_projection_backfill_is_pending_then_drives_exact_page_sort_and_filter() {
        let (dir, store) = db();
        let mut source = track("projection-backfill", "");
        source.genre = Some("Projection needle".into());
        store.save_track(&source).unwrap();
        let path = dir.path().join("projection-backfill.flac");
        std::fs::write(&path, b"fixture").unwrap();
        let mut local = source.clone();
        local.bpm = Some(127.0);
        local.key = Some("8A".into());
        store
            .save_asset(&asset(
                "projection-backfill-asset",
                &local,
                &path,
                "projection-backfill-hash",
            ))
            .unwrap();

        let (source_order_page, _, source_order_state, source_order_pending) = store
            .list_tracks_page_with_sort_state(
                "library",
                None,
                10,
                0,
                None,
                Some("sourceOrder"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(!source_order_pending);
        assert_eq!(source_order_page.len(), 1);
        assert!(matches!(
            source_order_state,
            Some(SortAvailabilityState::Pending)
        ));

        let (pending_page, _, pending_state, sort_pending) = store
            .list_tracks_page_with_sort_state(
                "library",
                None,
                10,
                0,
                Some("Projection needle"),
                Some("bpm"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(sort_pending);
        assert!(
            pending_page.is_empty(),
            "never return a partial filtered/sorted page"
        );
        assert!(matches!(
            pending_state,
            Some(SortAvailabilityState::Pending)
        ));

        assert_eq!(store.backfill_sort_availability_batch(10).unwrap(), 1);
        let projection_before_file_change: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT sort_metadata_projection FROM catalog_listings WHERE id=?1",
                [&source.id],
                |row| row.get(0),
            )
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        let (ready_page, total, ready_state, ready_pending) = store
            .list_tracks_page_with_sort_state(
                "library",
                None,
                10,
                0,
                Some("Projection needle"),
                Some("bpm"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(!ready_pending);
        assert_eq!(total, 1);
        assert_eq!(ready_page[0].id, source.id);
        assert_eq!(ready_page[0].bpm, Some(127.0));
        assert_eq!(ready_page[0].local_availability, "missing");
        assert!(matches!(ready_state, Some(SortAvailabilityState::Ready(_))));
        let projection_after_file_change: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT sort_metadata_projection FROM catalog_listings WHERE id=?1",
                [&source.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projection_after_file_change, projection_before_file_change);
    }

    #[test]
    fn multi_asset_rows_stay_live_and_are_not_persisted_as_one_asset_projections() {
        let (dir, store) = db();
        let source = track("projection-multi", "");
        store.save_track(&source).unwrap();
        for (id, bpm) in [("projection-multi-a", 90.0), ("projection-multi-b", 110.0)] {
            let path = dir.path().join(format!("{id}.flac"));
            std::fs::write(&path, b"fixture").unwrap();
            let mut local = source.clone();
            local.bpm = Some(bpm);
            store
                .save_asset(&asset(id, &local, &path, &format!("{id}-hash")))
                .unwrap();
        }
        assert_eq!(store.backfill_sort_availability_batch(10).unwrap(), 0);
        let (page, total, state, sort_pending) = store
            .list_tracks_page_with_sort_state(
                "library",
                None,
                10,
                0,
                None,
                Some("bpm"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(!sort_pending);
        assert_eq!(total, 1);
        assert_eq!(page[0].bpm, Some(90.0));
        assert!(matches!(state, Some(SortAvailabilityState::Ready(_))));
        let conn = store.connection.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT sort_metadata_projection FROM catalog_listings WHERE id=?1",
                [&source.id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn exact_prefix_reuses_page_order_and_preserves_duplicate_playlist_occurrences() {
        let (_dir, store) = db();
        let mut first = track("prefix-a", "");
        first.title = "Zulu".into();
        let mut second = track("prefix-b", "");
        second.title = "Alpha".into();
        store.save_track(&first).unwrap();
        store.save_track(&second).unwrap();
        let collection = store.create_collection("Repeated entries").unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "INSERT INTO playlist_ordered_collections(collection_id) VALUES(?1)",
                [&collection.id],
            )
            .unwrap();
            for (position, id) in [
                (0, first.id.as_str()),
                (1, second.id.as_str()),
                (2, first.id.as_str()),
            ] {
                conn.execute("INSERT INTO playlist_ordered_entries(collection_id,position,listing_id) VALUES(?1,?2,?3)", params![collection.id, position, id]).unwrap();
            }
        }
        for sort_field in ["sourceOrder", "title"] {
            for direction in ["asc", "desc"] {
                let (page, page_total) = store
                    .list_tracks_page(
                        "collection",
                        Some(&collection.id),
                        3,
                        0,
                        Some(""),
                        Some(sort_field),
                        Some(direction),
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                let (prefix, prefix_total) = store
                    .list_track_prefix(
                        "collection",
                        Some(&collection.id),
                        3,
                        Some(""),
                        Some(sort_field),
                        Some(direction),
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                assert_eq!(prefix_total, page_total);
                assert_eq!(
                    prefix
                        .iter()
                        .map(|row| row.listing_id.as_str())
                        .collect::<Vec<_>>(),
                    page.iter().map(|row| row.id.as_str()).collect::<Vec<_>>()
                );
                if sort_field == "sourceOrder" && direction == "asc" {
                    assert_eq!(
                        prefix
                            .iter()
                            .map(|row| row.playlist_position)
                            .collect::<Vec<_>>(),
                        vec![Some(1), Some(2), Some(3)]
                    );
                    assert_eq!(
                        prefix
                            .iter()
                            .map(|row| row.listing_id.as_str())
                            .collect::<Vec<_>>(),
                        vec![first.id.as_str(), second.id.as_str(), first.id.as_str()]
                    );
                }
            }
        }
        let repeated = store
            .get_tracks(&[first.id.clone(), second.id.clone(), first.id.clone()])
            .unwrap();
        assert_eq!(
            repeated
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec![first.id.as_str(), second.id.as_str(), first.id.as_str()]
        );
    }

    #[test]
    fn artist_playback_matching_obeys_shared_source_conformance_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../src/lib/artist-playback-conformance.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let source: Track = serde_json::from_value(case["sourceTrack"].clone()).unwrap();
            let assets: Vec<LocalAsset> = serde_json::from_value(case["assets"].clone()).unwrap();
            let expected = case["expectedAssetId"].as_str();
            let actual = unique_artist_playback_match(&source, &assets)
                .map(|index| assets[index].id.as_str());
            assert_eq!(actual, expected, "case {}", case["name"]);
        }
    }

    #[test]
    fn artist_asset_page_sorts_and_filters_globally_before_limit() {
        let (dir, store) = db();
        for (id, title) in [
            ("artist-page-z", "Zulu"),
            ("artist-page-a", "Alpha"),
            ("artist-page-b", "Needle Beta"),
        ] {
            let mut track = track(id, "");
            track.artist = "Page Artist".into();
            track.title = title.into();
            store.save_track(&track).unwrap();
            store
                .save_asset(&asset(
                    &format!("{id}-asset"),
                    &track,
                    &dir.path().join(format!("{id}.flac")),
                    &format!("{id}-hash"),
                ))
                .unwrap();
        }
        let names = ["page artist".to_owned()];
        let (page, total, availability) = store
            .list_artist_local_assets_page(
                &names,
                1,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                Some("all"),
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].track.title, "Alpha");
        assert!(availability["title"]);

        let (filtered, filtered_total, _) = store
            .list_artist_local_assets_page(
                &names,
                1,
                0,
                Some("needle"),
                Some("title"),
                Some("desc"),
                None,
                None,
                Some("all"),
            )
            .unwrap();
        assert_eq!(filtered_total, 1);
        assert_eq!(filtered[0].track.title, "Needle Beta");
    }

    #[test]
    fn playback_lookup_adds_exact_featured_source_credit_to_candidate_names() {
        let (dir, store) = db();
        let mut local = track("artist-playback-featured", "");
        local.artist = "Aster feat. Guest".into();
        local.title = "Song".into();
        local.version = Some(String::new());
        store.save_track(&local).unwrap();
        let path = dir.path().join("artist-playback-featured.flac");
        std::fs::write(&path, b"fixture audio").unwrap();
        store
            .save_asset(&asset(
                "artist-playback-featured-asset",
                &local,
                &path,
                "featured-hash",
            ))
            .unwrap();
        let mut source = local.clone();
        source.id = "lastfm:featured".into();
        source.provider = "lastfm".into();
        source.provider_id = "featured".into();
        source.artist = "Ａster\u{00A0}feat. Guest".into();
        source.version = None;
        source.duration_seconds = None;
        let matches = store
            .match_artist_playback_sources(&["Main profile artist".into()], &[source])
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].as_ref().map(|asset| asset.id.as_str()),
            Some("artist-playback-featured-asset")
        );
    }

    #[test]
    fn long_prefix_matches_filtered_duration_pages_and_null_last_order_past_one_thousand() {
        let (_dir, store) = db();
        for index in 0usize..1105 {
            let mut value = track(&format!("long-prefix-{index:04}"), "");
            value.artist = "Prefix Artist".into();
            value.title = format!("Track {index:04}");
            value.duration_seconds = if index % 101 == 0 {
                None
            } else {
                Some(index.saturating_sub(1) as f64)
            };
            store.save_track(&value).unwrap();
        }
        let collect_pages =
            |query: Option<&str>, min: Option<u32>, max: Option<u32>, direction: &str| {
                let mut ids = Vec::new();
                let mut offset = 0;
                let mut expected_total = None;
                loop {
                    let (page, total) = store
                        .list_tracks_page(
                            "library",
                            None,
                            1000,
                            offset,
                            query,
                            Some("duration"),
                            Some(direction),
                            min,
                            max,
                            None,
                            None,
                        )
                        .unwrap();
                    expected_total.get_or_insert(total);
                    if page.is_empty() {
                        break;
                    }
                    offset += page.len();
                    ids.extend(page.into_iter().map(|track| track.id));
                }
                (ids, expected_total.unwrap_or_default())
            };
        for direction in ["asc", "desc"] {
            let (prefix, total) = store
                .list_track_prefix(
                    "library",
                    None,
                    1105,
                    Some("Prefix Artist"),
                    Some("duration"),
                    Some(direction),
                    Some(0),
                    Some(1800),
                    None,
                    None,
                )
                .unwrap();
            let (paged, page_total) =
                collect_pages(Some("Prefix Artist"), Some(0), Some(1800), direction);
            assert_eq!(total, page_total);
            assert!(total > 1000);
            assert_eq!(prefix.len(), 1093);
            assert_eq!(
                prefix
                    .iter()
                    .map(|row| row.listing_id.clone())
                    .collect::<Vec<_>>(),
                paged
            );

            let (nullable_prefix, nullable_total) = store
                .list_track_prefix(
                    "library",
                    None,
                    1105,
                    Some("Prefix Artist"),
                    Some("duration"),
                    Some(direction),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            let (nullable_pages, pages_total) =
                collect_pages(Some("Prefix Artist"), None, None, direction);
            assert_eq!(nullable_total, pages_total);
            assert_eq!(nullable_prefix.len(), 1105);
            assert_eq!(
                nullable_prefix
                    .iter()
                    .map(|row| row.listing_id.clone())
                    .collect::<Vec<_>>(),
                nullable_pages
            );
            let index_of = |id: &str| id.rsplit('-').next().unwrap().parse::<usize>().unwrap();
            assert!(nullable_prefix.iter().rev().take(12).all(|row| {
                let index = index_of(&row.listing_id);
                index % 101 == 0 || index == 1
            }));
            assert!(nullable_prefix.iter().take(1093).all(|row| {
                let index = index_of(&row.listing_id);
                index % 101 != 0 && index != 1
            }));
        }
    }

    #[test]
    fn legacy_collection_position_is_global_source_order_under_filters_and_sorts() {
        let (_dir, store) = db();
        let mut first = track("legacy-position-first", "");
        first.title = "Zebra Recording".into();
        let mut second = track("legacy-position-second", "");
        second.title = "Alpha Recording".into();
        let mut third = track("legacy-position-third", "");
        third.title = "Middle Recording".into();
        for value in [&first, &second, &third] {
            store.save_track(value).unwrap();
        }
        let collection = store.create_collection("Legacy order").unwrap();
        store
            .add_to_collection(&collection.id, &[first.clone(), second, third.clone()])
            .unwrap();

        let (page, total) = store
            .list_tracks_page(
                "collection",
                Some(&collection.id),
                10,
                0,
                Some("Middle"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let (prefix, prefix_total) = store
            .list_track_prefix(
                "collection",
                Some(&collection.id),
                10,
                Some("Middle"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(prefix_total, total);
        assert_eq!(page[0].id, third.id);
        assert_eq!(page[0].provenance["playlistPosition"], 3);
        assert_eq!(prefix[0].listing_id, third.id);
        assert_eq!(prefix[0].playlist_position, Some(3));
    }

    #[test]
    fn paged_sort_availability_covers_filtered_view_and_only_returns_on_first_page() {
        let (_dir, store) = db();
        let mut matching = track("availability-match", "");
        matching.title = "Unique matching title".into();
        let mut unrelated = track("availability-other", "");
        unrelated.title = "Other title".into();
        unrelated.album = Some("Known album".into());
        for value in [&matching, &unrelated] {
            store.save_track(value).unwrap();
        }

        let (rows, total, availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                1,
                0,
                Some("Unique matching"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(total, 1);
        let availability = availability.unwrap();
        assert_eq!(availability.get("sourceOrder"), Some(&true));
        assert_eq!(availability.get("title"), Some(&true));
        assert_eq!(availability.get("artist"), Some(&true));
        assert_eq!(availability.get("album"), Some(&false));

        let (_, total, later_page_availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                1,
                1,
                Some("Unique matching"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 1);
        assert!(later_page_availability.is_none());

        let (empty_page, empty_total, empty_availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                1,
                0,
                Some("No matching track"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(empty_page.is_empty());
        assert_eq!(empty_total, 0);
        let empty_availability = empty_availability.unwrap();
        assert_eq!(empty_availability.get("sourceOrder"), Some(&true));
        assert!(empty_availability
            .iter()
            .filter(|(field, _)| field.as_str() != "sourceOrder")
            .all(|(_, available)| !available));
    }

    #[test]
    fn save_library_tracks_promotes_cached_metadata_and_preserves_manual_edits() {
        let (_dir, store) = db();
        let mut source = track("album-save-one", "");
        source.title = "Provider title".into();
        source.album = Some("Complete album metadata".into());
        source.duration_seconds = Some(222.0);
        store.save_track(&source).unwrap();

        let mut edited = source.clone();
        edited.title = "My manual title".into();
        store.update_track(&edited).unwrap();
        {
            let conn = store.lock_connection().unwrap();
            conn.execute(
                "UPDATE catalog_listings SET in_library=0 WHERE id=?1",
                [&source.id],
            )
            .unwrap();
        }

        let mut sparse_refresh = source.clone();
        sparse_refresh.duration_seconds = None;
        let result = store.save_library_tracks(&[sparse_refresh]).unwrap();
        assert_eq!(result, vec![source.id.clone()]);
        let saved = store.list_tracks("library", None).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].title, "My manual title");
        assert_eq!(saved[0].album.as_deref(), Some("Complete album metadata"));
        assert_eq!(saved[0].duration_seconds, Some(222.0));
    }

    #[test]
    fn save_library_tracks_accepts_musicbrainz_recording_identity() {
        let (_dir, store) = db();
        let mbid = "00000000-0000-4000-8000-000000000001";
        let musicbrainz = Track {
            id: format!("musicbrainz:{mbid}"),
            provider: "musicbrainz".into(),
            provider_id: mbid.into(),
            title: "Release recording".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        assert_eq!(
            store.save_library_tracks(&[musicbrainz.clone()]).unwrap(),
            vec![musicbrainz.id.clone()]
        );
        assert_eq!(
            store.list_tracks("library", None).unwrap()[0].id,
            musicbrainz.id
        );
    }

    #[test]
    fn save_library_tracks_rejects_invalid_album_atomically() {
        let (_dir, store) = db();
        let candidate = track("album-save-atomic", "");
        let mut invalid = track("album-save-invalid", "");
        invalid.id = "not-the-canonical-id".into();

        assert!(store
            .save_library_tracks(&[candidate.clone(), invalid])
            .is_err());
        assert!(store.get_track(&candidate.id).unwrap().is_none());
        assert!(store.list_tracks("library", None).unwrap().is_empty());
    }

    #[test]
    fn cached_tracks_remain_retrievable_but_library_membership_is_explicit_and_monotonic() {
        let (_dir, store) = db();
        let mut ascii = track("cached-ascii", "");
        ascii.title = "Needle cached recording".into();
        ascii.album = Some("Cached album".into());
        let mut unicode = track("cached-unicode", "");
        unicode.title = "Máté composition".into();

        store.cache_track(&ascii).unwrap();
        store.cache_track(&unicode).unwrap();
        assert!(store.get_track(&ascii.id).unwrap().is_some());
        assert_eq!(store.list_tracks("all", None).unwrap().len(), 2);
        assert!(store.list_tracks("library", None).unwrap().is_empty());
        let (page, total, availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                20,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                Some("personal"),
            )
            .unwrap();
        assert!(page.is_empty());
        assert_eq!(total, 0);
        let availability = availability.unwrap();
        assert_eq!(availability.get("sourceOrder"), Some(&true));
        assert_eq!(availability.get("title"), Some(&false));
        assert_eq!(availability.get("album"), Some(&false));
        assert_eq!(
            store
                .search_library_tracks(&["needle".into()], 20, 0)
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            store
                .search_library_tracks(&["máté".into()], 20, 0)
                .unwrap()
                .1,
            0
        );

        let cached_json: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&ascii.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&cached_json)
            .unwrap()
            .get("dateAdded")
            .is_none());

        store.save_track(&ascii).unwrap();
        // A later discovery refresh may enrich the listing, but cannot undo an
        // explicit save or move its added date back to discovery time.
        store.cache_track(&ascii).unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 1);
        assert_eq!(
            store
                .search_library_tracks(&["needle".into()], 20, 0)
                .unwrap()
                .1,
            1
        );
        assert_eq!(
            store
                .search_library_tracks(&["máté".into()], 20, 0)
                .unwrap()
                .1,
            0
        );
        let saved_json: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&ascii.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&saved_json)
            .unwrap()
            .get("dateAdded")
            .and_then(serde_json::Value::as_str)
            .is_some());

        store.save_track(&unicode).unwrap();
        assert_eq!(
            store
                .search_library_tracks(&["máté".into()], 20, 0)
                .unwrap()
                .1,
            1
        );
    }

    #[test]
    fn explicit_curation_collection_and_saved_asset_promote_cached_tracks() {
        let (dir, store) = db();
        let favorite = track("promote-favorite", "");
        store.cache_track(&favorite).unwrap();
        store.set_favorite(&favorite, true).unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 1);
        assert_eq!(store.list_tracks("favorites", None).unwrap().len(), 1);
        store.set_favorite(&favorite, false).unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 1);

        let shortlist = track("promote-shortlist", "");
        store.cache_track(&shortlist).unwrap();
        store.set_disposition(&shortlist, Some("reject")).unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 1);
        store
            .set_disposition(&shortlist, Some("shortlist"))
            .unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 2);
        store.set_disposition(&shortlist, None).unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 2);

        let member = track("promote-collection", "");
        store.cache_track(&member).unwrap();
        let collection = store.create_collection("Promotion boundary").unwrap();
        store
            .add_to_collection(&collection.id, &[member.clone()])
            .unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 3);
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap()
                .len(),
            1
        );

        let ordered_member = track("promote-ordered-collection", "");
        store.cache_track(&ordered_member).unwrap();
        store
            .replace_playlist_entries(
                &collection.id,
                &[ordered_member.clone(), ordered_member.clone()],
            )
            .unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 4);
        assert_eq!(
            store
                .list_tracks("collection", Some(&collection.id))
                .unwrap()
                .len(),
            2
        );

        let asset_track = track("promote-asset", "");
        store.cache_track(&asset_track).unwrap();
        let path = dir.path().join("promoted.flac");
        std::fs::write(&path, b"local audio").unwrap();
        store
            .save_asset(&asset(
                "promote-local-asset",
                &asset_track,
                &path,
                "promote-hash",
            ))
            .unwrap();
        assert_eq!(store.list_tracks("library", None).unwrap().len(), 5);
    }

    #[test]
    fn recording_merges_preserve_library_membership_on_canonical_and_alias_actions() {
        let (_dir, store) = db();
        let canonical = track("merge-cached-canonical", "");
        let alias = track("merge-cached-alias", "");
        store.cache_track(&canonical).unwrap();
        store.cache_track(&alias).unwrap();
        store
            .merge_recordings(&canonical.id, &[alias.id.clone()])
            .unwrap();
        assert!(store.list_tracks("library", None).unwrap().is_empty());
        store.set_favorite(&alias, true).unwrap();
        let library = store.list_tracks("library", None).unwrap();
        assert_eq!(library.len(), 1);
        assert_eq!(library[0].id, canonical.id);

        let cached_primary = track("merge-saved-alias-primary", "");
        let saved_alias = track("merge-saved-alias-secondary", "");
        store.cache_track(&cached_primary).unwrap();
        store.save_track(&saved_alias).unwrap();
        store
            .merge_recordings(&cached_primary.id, &[saved_alias.id])
            .unwrap();
        assert!(store
            .list_tracks("library", None)
            .unwrap()
            .iter()
            .any(|track| track.id == cached_primary.id));
    }

    #[test]
    fn paged_sort_availability_includes_values_after_the_first_200_rows() {
        let (_dir, store) = db();
        for index in 0..200 {
            let mut value = track(&format!("availability-empty-{index}"), "");
            value.title = format!("Row {index:03}");
            store.save_track(&value).unwrap();
        }
        let mut later = track("availability-later-album", "");
        later.title = "Row 200".into();
        later.album = Some("Album beyond the first page".into());
        store.save_track(&later).unwrap();

        let (page, total, availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                200,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 201);
        assert_eq!(page.len(), 200);
        assert!(page.iter().all(|value| value.album.is_none()));
        assert_eq!(availability.unwrap().get("album"), Some(&true));
    }

    #[test]
    fn paged_sort_availability_uses_shared_validation_for_count_key_and_date() {
        let (_dir, store) = db();
        let mut value = track("availability-normalizers", "");
        value.provenance = serde_json::json!({"plays":0});
        value.key = Some("C not-a-valid-key".into());
        value.release_date = Some("2024-02-31".into());
        store.save_track(&value).unwrap();

        let (_, _, availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                10,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let availability = availability.unwrap();
        assert_eq!(availability.get("plays"), Some(&true));
        assert_eq!(availability.get("key"), Some(&false));
        assert_eq!(availability.get("releaseDate"), Some(&false));
    }

    #[test]
    fn paged_sort_availability_reads_shared_fields_from_materialized_jsonb() {
        let (_dir, store) = db();
        let mut value = track("availability-jsonb", "");
        value.album = Some("Known album".into());
        value.duration_seconds = Some(42.0);
        value.bpm = Some(128.0);
        value.key = Some("8A".into());
        value.release_date = Some("2024-04-05".into());
        value.provenance = serde_json::json!({
            "dateAdded":"2025-01-02",
            "plays":0
        });
        store.save_track(&value).unwrap();

        let (_, _, availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                10,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let availability = availability.unwrap();
        for field in [
            "title",
            "artist",
            "album",
            "duration",
            "releaseDate",
            "dateAdded",
            "bpm",
            "key",
            "plays",
        ] {
            assert_eq!(availability.get(field), Some(&true), "{field}");
        }
        assert_eq!(availability.get("views"), Some(&false));
    }

    #[test]
    fn paged_sort_availability_counts_asset_derived_bpm_and_key() {
        let (dir, store) = db();
        let source = track("availability-local-analysis", "");
        store.save_track(&source).unwrap();
        let path = dir.path().join("availability.flac");
        std::fs::write(&path, b"audio").unwrap();
        let mut local = source.clone();
        local.bpm = Some(128.0);
        local.key = Some("8A".into());
        store
            .save_asset(&asset(
                "availability-asset",
                &local,
                &path,
                "availability-hash",
            ))
            .unwrap();

        let (_, _, availability) = store
            .list_tracks_page_with_availability(
                "library",
                None,
                10,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let availability = availability.unwrap();
        assert_eq!(availability.get("bpm"), Some(&true));
        assert_eq!(availability.get("key"), Some(&true));
    }

    #[test]
    fn paged_sort_availability_respects_duplicate_ordered_collection_entries() {
        let (_dir, store) = db();
        let mut value = track("availability-duplicate", "");
        value.album = Some("Repeated album".into());
        value.duration_seconds = Some(180.0);
        store.save_track(&value).unwrap();
        let collection = store.create_collection("Duplicate availability").unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "INSERT INTO playlist_ordered_collections(collection_id) VALUES(?1)",
                [&collection.id],
            )
            .unwrap();
            for position in 0..2 {
                conn.execute(
                    "INSERT INTO playlist_ordered_entries(collection_id,position,listing_id) VALUES(?1,?2,?3)",
                    params![collection.id, position, value.id],
                )
                .unwrap();
            }
        }

        let (page, total, availability) = store
            .list_tracks_page_with_availability(
                "collection",
                Some(&collection.id),
                10,
                0,
                Some("Title"),
                Some("album"),
                Some("asc"),
                Some(60),
                Some(200),
                Some("all"),
                Some("fixture"),
            )
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(page.len(), 2);
        assert_eq!(availability.unwrap().get("album"), Some(&true));
    }

    #[test]
    fn shared_sort_contract_nulls_ties_and_duplicate_occurrences() {
        let (_directory, store) = db();
        // Keys follow normalized pitch order: C, C# (also Camelot 3B), D.
        let cases = [
            (
                "a",
                "Alpha",
                "Alpha artist",
                Some("Alpha album"),
                Some(40.0),
                Some("2020-01-01"),
                Some("2020-01-01"),
                Some(100.0),
                Some("C"),
                Some(10),
                Some(100),
            ),
            (
                "b",
                "Zulu",
                "Zulu artist",
                Some("Zulu album"),
                Some(30.0),
                Some("2024-01-01"),
                Some("2024-01-01"),
                Some(140.0),
                Some("D"),
                Some(30),
                Some(300),
            ),
            (
                "c",
                "Middle",
                "Middle artist",
                Some("Middle album"),
                Some(20.0),
                Some("2022-01-01"),
                Some("2022-01-01"),
                Some(120.0),
                Some("C#"),
                Some(20),
                Some(200),
            ),
            (
                "d",
                "Middle",
                "Middle artist",
                Some("Middle album"),
                Some(20.0),
                Some("2022-01-01"),
                Some("2022-01-01"),
                Some(120.0),
                Some("3B"),
                Some(20),
                Some(200),
            ),
            (
                "missing", "", "", None, None, None, None, None, None, None, None,
            ),
        ];
        let mut tracks = std::collections::BTreeMap::new();
        for (
            id,
            title,
            artist,
            album,
            duration,
            release_date,
            date_added,
            bpm,
            key,
            plays,
            views,
        ) in cases
        {
            let mut value = track(&format!("sort-qualification-{id}"), "");
            value.title = title.into();
            value.artist = artist.into();
            value.album = album.map(str::to_owned);
            value.duration_seconds = duration;
            value.release_date = release_date.map(str::to_owned);
            value.date_added = date_added.map(|date| serde_json::json!(date));
            value.bpm = bpm;
            value.key = key.map(str::to_owned);
            value.provenance = serde_json::json!({
                "plays": plays,
                "views": views,
            });
            if id == "a" {
                value.duration_seconds = Some(40.0);
                value.playback_range = Some(PlaybackRange {
                    parent_track_id: value.id.clone(),
                    chapter_id: "chapter-a".into(),
                    source_url: String::new(),
                    start_time: 10.0,
                    end_time: 20.0,
                    parent_title: value.title.clone(),
                    parent_duration_seconds: Some(40.0),
                });
            }
            store.save_track(&value).unwrap();
            tracks.insert(id, value);
        }
        let collection = store
            .create_collection("sort qualification contract")
            .unwrap();
        let playlist = ["a", "b", "c", "d", "missing", "c"]
            .into_iter()
            .map(|id| tracks[id].clone())
            .collect::<Vec<_>>();
        store
            .replace_playlist_entries(&collection.id, &playlist)
            .unwrap();
        // Playlist replacement also promotes/upserts its rows, so remove the
        // generated added date only after all fixture writes have finished.
        {
            let value = &tracks["missing"];
            let connection = store.connection.lock().unwrap();
            connection
                    .execute(
                        "UPDATE catalog_listings SET metadata=json_remove(metadata,'$.dateAdded','$.provenance.dateAdded','$.provenance.addedAt','$.provenance.importedAt','$.provenance.createdAt') WHERE id=?1",
                        [&value.id],
                    )
                    .unwrap();
            let date_added_is_absent: bool = connection
                    .query_row(
                        "SELECT COALESCE(
                            json_extract(json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')),'$.dateAdded'),
                            json_extract(json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')),'$.provenance.dateAdded'),
                            json_extract(json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')),'$.provenance.addedAt'),
                            json_extract(json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')),'$.provenance.importedAt'),
                            json_extract(json_patch(l.metadata,COALESCE(r.manual_metadata,'{}')),'$.provenance.createdAt')
                        ) IS NULL FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id=?1",
                        [&value.id],
                        |row| row.get(0),
                    )
                    .unwrap();
            assert!(
                date_added_is_absent,
                "fixture must remain missing dateAdded after promotion"
            );
        }

        let asc = ["a", "c", "d", "c", "b", "missing"];
        let desc = ["b", "c", "d", "c", "a", "missing"];
        for field in [
            "title",
            "artist",
            "album",
            "duration",
            "releaseDate",
            "dateAdded",
            "bpm",
            "key",
            "plays",
            "views",
        ] {
            for (direction, expected) in [("asc", &asc), ("desc", &desc)] {
                let (page, total) = store
                    .list_tracks_page(
                        "collection",
                        Some(&collection.id),
                        20,
                        0,
                        None,
                        Some(field),
                        Some(direction),
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                let expected_ids = expected
                    .iter()
                    .map(|id| tracks[*id].id.as_str())
                    .collect::<Vec<_>>();
                let actual_ids = page.iter().map(|row| row.id.as_str()).collect::<Vec<_>>();
                assert_eq!(total, expected.len(), "{field} {direction} count");
                assert_eq!(
                    actual_ids, expected_ids,
                    "{field} {direction} null/tie order"
                );
            }
        }
        for (direction, expected) in [
            ("asc", ["a", "b", "c", "d", "missing", "c"]),
            ("desc", ["c", "missing", "d", "c", "b", "a"]),
        ] {
            let (page, total) = store
                .list_tracks_page(
                    "collection",
                    Some(&collection.id),
                    20,
                    0,
                    None,
                    Some("sourceOrder"),
                    Some(direction),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            let expected_ids = expected
                .iter()
                .map(|id| tracks[*id].id.as_str())
                .collect::<Vec<_>>();
            let actual_ids = page.iter().map(|row| row.id.as_str()).collect::<Vec<_>>();
            assert_eq!(total, expected.len(), "sourceOrder {direction} count");
            assert_eq!(actual_ids, expected_ids, "sourceOrder {direction} order");
        }
    }

    #[test]
    fn library_metrics_match_joined_relation_with_partial_and_complete_projections() {
        let (_dir, store) = db();
        let mut known = track("metrics-known", "");
        known.bpm = Some(125.0);
        known.album = Some("Known album".into());
        let missing = track("metrics-missing", "");
        store.save_track(&known).unwrap();
        store.save_track(&missing).unwrap();
        // Cached listings outside the library must not enter either relation.
        store.cache_track(&track("metrics-cache-only", "")).unwrap();
        for complete in [false, true] {
            if complete {
                while store.backfill_sort_availability_batch(500).unwrap() != 0 {}
            }
            let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
            let expected: (usize, Option<i64>, Option<i64>) = {
                let conn = store.connection.lock().unwrap();
                conn.query_row(&format!("SELECT COUNT(*),sort_availability_or(CASE WHEN l.sort_availability_revision={revision} THEN l.sort_availability_mask END),MIN(CASE WHEN l.sort_projection_revision={revision} AND l.sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END) FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id=l.recording_id AND l.in_library=1"), [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
            };
            assert_eq!(expected.0, 2);
            assert_eq!(expected.2, Some(i64::from(complete)));
            for sort in ["sourceOrder", "bpm"] {
                for (query, availability, provider) in [
                    (None, None, None),
                    (Some("  "), Some("all"), Some("all")),
                    (None, None, Some("personal")),
                ] {
                    let (page, total, state, pending) = store
                        .list_tracks_page_with_sort_state(
                            "library",
                            None,
                            20,
                            0,
                            query,
                            Some(sort),
                            Some("asc"),
                            None,
                            None,
                            availability,
                            provider,
                        )
                        .unwrap();
                    assert!(!pending);
                    assert_eq!(total, expected.0);
                    assert_eq!(page.len(), 2);
                    match state.unwrap() {
                        SortAvailabilityState::Pending => assert!(!complete),
                        SortAvailabilityState::Ready(fields) => {
                            assert!(complete);
                            let values =
                                crate::track_sort_sql::unpack_available_fields(expected.1.unwrap());
                            for (field, value) in [
                                "title",
                                "artist",
                                "album",
                                "duration",
                                "releaseDate",
                                "dateAdded",
                                "bpm",
                                "key",
                                "plays",
                                "views",
                            ]
                            .into_iter()
                            .zip(values)
                            {
                                assert_eq!(fields.get(field), Some(&value), "{sort} {field}");
                            }
                        }
                    }
                    // Offset pages exercise the count-only/BPM coverage branch.
                    let (_, offset_total, state, _) = store
                        .list_tracks_page_with_sort_state(
                            "library",
                            None,
                            1,
                            1,
                            query,
                            Some(sort),
                            Some("asc"),
                            None,
                            None,
                            availability,
                            provider,
                        )
                        .unwrap();
                    assert_eq!(offset_total, expected.0);
                    assert!(state.is_none());
                }
            }
        }
    }

    #[test]
    fn library_metrics_keep_effective_filters_and_other_view_multiplicity() {
        let (_dir, store) = db();
        let mut edited = track("metrics-edited", "");
        edited.bpm = Some(120.0);
        edited.duration_seconds = Some(10.0);
        let other = track("metrics-other", "");
        store.save_track(&edited).unwrap();
        store.save_track(&other).unwrap();
        store.set_favorite(&edited, true).unwrap();
        let collection = store.create_collection("Metrics duplicates").unwrap();
        store
            .replace_playlist_entries(
                &collection.id,
                &[edited.clone(), other.clone(), edited.clone()],
            )
            .unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute("UPDATE recordings SET manual_metadata=?1 WHERE id=?2", params![r#"{"title":"Unique override","durationSeconds":90,"provider":"manual-provider","bpm":null}"#, edited.id]).unwrap();
        }
        while store.backfill_sort_availability_batch(500).unwrap() != 0 {}
        for (query, min, max, availability, provider, expected) in [
            (Some("Unique override"), None, None, None, None, 1),
            (None, Some(80), Some(100), None, None, 1),
            (None, None, None, None, Some("manual-provider"), 1),
            (None, None, None, Some("local"), None, 0),
            (None, None, None, Some("not-local"), None, 2),
            (None, None, None, None, Some("local"), 0),
        ] {
            let (_, total, fields) = store
                .list_tracks_page_with_availability(
                    "library",
                    None,
                    20,
                    0,
                    query,
                    Some("sourceOrder"),
                    Some("asc"),
                    min,
                    max,
                    availability,
                    provider,
                )
                .unwrap();
            assert_eq!(
                total, expected,
                "{query:?} {min:?} {availability:?} {provider:?}"
            );
            assert_eq!(
                fields.unwrap().get("bpm"),
                Some(&false),
                "explicit manual null must survive"
            );
        }
        for (view, collection_id, count) in [
            ("favorites", None, 1),
            ("collection", Some(collection.id.as_str()), 3),
        ] {
            let (page, total, fields) = store
                .list_tracks_page_with_availability(
                    view,
                    collection_id,
                    20,
                    0,
                    None,
                    Some("sourceOrder"),
                    Some("asc"),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(total, count);
            assert_eq!(page.len(), count);
            assert_eq!(fields.unwrap().get("bpm"), Some(&false));
            if view == "collection" {
                assert_eq!(
                    page.iter()
                        .map(|value| value.id.as_str())
                        .collect::<Vec<_>>(),
                    vec![edited.id.as_str(), other.id.as_str(), edited.id.as_str()]
                );
            }
        }
    }

    // Candidate indexes exist only on this ignored benchmark's disposable Store.
    // Keep this out of Store::open until paired SQL and end-to-end gates pass.
    fn benchmark_canonical_metrics_indexes(conn: &Connection, samples: usize, revision: i64) {
        let coverage = format!(
            "CASE WHEN l.sort_projection_revision={revision} AND l.sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END"
        );
        let projections = [
            ("count", "COUNT(*)".to_owned()),
            (
                "countAndAvailability",
                format!(
                    "COUNT(*),sort_availability_or(CASE WHEN l.sort_availability_revision={revision} THEN l.sort_availability_mask END)"
                ),
            ),
            (
                "countAvailabilityAndBpmCoverage",
                format!(
                    "COUNT(*),sort_availability_or(CASE WHEN l.sort_availability_revision={revision} THEN l.sort_availability_mask END),MIN({coverage})"
                ),
            ),
        ];
        let predicate = "l.id=l.recording_id AND l.in_library=1";
        let table_root: i64 = conn
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE type='table' AND name='catalog_listings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let plan = |sql: &str| {
            let details = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            let ops = conn
                .prepare(&format!("EXPLAIN {sql}"))
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            let table_cursors = ops
                .iter()
                .filter(|(op, _, p2)| op == "OpenRead" && *p2 == table_root)
                .map(|(_, p1, _)| *p1)
                .collect::<Vec<_>>();
            serde_json::json!({"details":details,"baseTableColumnReads":ops.iter().filter(|(op,p1,_)| op=="Column" && table_cursors.contains(p1)).count(),"deferredSeekOps":ops.iter().filter(|(op,_,_)|op=="DeferredSeek").count()})
        };
        let read = |sql: &str, phase: usize| -> (usize, Option<i64>, Option<i64>) {
            conn.query_row(sql, [], |row| {
                Ok((
                    row.get(0)?,
                    if phase > 0 { row.get(1)? } else { None },
                    if phase > 1 { row.get(2)? } else { None },
                ))
            })
            .unwrap_or_else(|error| {
                panic!(
                    "canonical metrics read failed (phase={}): {error}; SQL={sql}",
                    projections[phase].0
                )
            })
        };
        let natural_baseline = projections
            .iter()
            .map(|(_, projection)| {
                plan(&format!(
                    "SELECT {projection} FROM catalog_listings l WHERE {predicate}"
                ))
            })
            .collect::<Vec<_>>();
        let mut index_builds = Vec::new();
        for (name, columns) in [
            (
                "bench_canonical_metrics_lean",
                "sort_availability_revision,sort_availability_mask".to_owned(),
            ),
            (
                "bench_canonical_metrics_coverage",
                format!(
                    "sort_availability_revision,sort_availability_mask,(CASE WHEN sort_projection_revision={revision} AND sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END)"
                ),
            ),
        ] {
            let started = Instant::now();
            conn.execute_batch(&format!("CREATE INDEX {name} ON catalog_listings({columns}) WHERE id=recording_id AND in_library=1;")).unwrap();
            let create_ms = started.elapsed().as_secs_f64() * 1000.0;
            let bytes: Option<i64> = conn
                .query_row(
                    "SELECT SUM(pgsize) FROM dbstat WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )
                .ok()
                .flatten();
            index_builds.push(serde_json::json!({"name":name,"columns":columns,"creationMsSingleObservation":create_ms,"indexBytesIfDbstatAvailable":bytes}));
        }
        let relations = [
            ("baselineTableScan", "catalog_listings l NOT INDEXED"),
            (
                "leanPartial",
                "catalog_listings l INDEXED BY bench_canonical_metrics_lean",
            ),
            (
                "coverageCasePartial",
                "catalog_listings l INDEXED BY bench_canonical_metrics_coverage",
            ),
        ];
        // A constant AND 0 can make SQLite discard the partial-index predicate
        // before planning INDEXED BY and fail with "no query solution". A real
        // empty fixture filter retains the predicate; preflight before timing.
        for (_, relation) in relations {
            assert_eq!(
                read(
                    &format!(
                        "SELECT {} FROM {relation} WHERE {predicate} AND l.provider='empty_fixture'",
                        projections[2].1
                    ),
                    2
                ),
                (0, None, None)
            );
        }
        let mut phases = Vec::new();
        for (phase, (name, projection)) in projections.iter().enumerate() {
            let queries = relations
                .iter()
                .map(|(_, relation)| {
                    format!("SELECT {projection} FROM {relation} WHERE {predicate}")
                })
                .collect::<Vec<_>>();
            let mut timings = vec![Vec::new(); relations.len()];
            let expected = read(&queries[0], phase);
            assert_eq!(expected.0, 50_000);
            if phase > 1 {
                assert_eq!(expected.2, Some(1));
            }
            for iteration in 0..samples + 3 {
                // Rotate then reverse the triple so no variant is always first/last.
                let mut order = (0..relations.len())
                    .map(|index| (index + iteration) % relations.len())
                    .collect::<Vec<_>>();
                if iteration % 2 != 0 {
                    order.reverse();
                }
                for index in order {
                    let started = Instant::now();
                    let actual = read(&queries[index], phase);
                    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                    assert_eq!(actual, expected);
                    if iteration >= 3 {
                        timings[index].push(elapsed_ms);
                    }
                }
            }
            let variants = relations.iter().enumerate().map(|(index, (variant, _))| {
                let mut sorted = timings[index].clone(); sorted.sort_by(f64::total_cmp);
                let middle = sorted.len() / 2;
                let median = if sorted.len() % 2 == 0 { (sorted[middle - 1] + sorted[middle]) / 2.0 } else { sorted[middle] };
                serde_json::json!({"variant":variant,"sql":queries[index],"plan":plan(&queries[index]),"samplesMs":timings[index],"medianMs":median,"p95Ms":sorted[((sorted.len() as f64*0.95).ceil() as usize)-1],"worstMs":sorted.last()})
            }).collect::<Vec<_>>();
            phases.push(serde_json::json!({"phase":name,"baselineNaturalPlanBeforeCandidates":natural_baseline[phase],"naturalPlanWithBothCandidates":plan(&format!("SELECT {projection} FROM catalog_listings l WHERE {predicate}")),"identicalResult":expected,"variants":variants}));
        }
        // Untimed semantic checks exercise ordinary SQLite index maintenance and
        // the existing metadata invalidation trigger. Roll back each mutation.
        let mut semantic_checks = Vec::new();
        let first_id: String = conn
            .query_row(
                "SELECT id FROM catalog_listings ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let second_id: String = conn
            .query_row(
                "SELECT id FROM catalog_listings ORDER BY id LIMIT 1 OFFSET 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        for (label, sql, expected_count, pending, projected) in [
            ("demotedMembership", "UPDATE catalog_listings SET in_library=0 WHERE id=?1".to_owned(), 49_999, false, 1),
            ("aliasMembership", "UPDATE catalog_listings SET recording_id=?2 WHERE id=?1".to_owned(), 49_999, false, 1),
            ("staleAvailability", format!("UPDATE catalog_listings SET sort_availability_revision={} WHERE id=?1", revision - 1), 50_000, true, 1),
            ("nullAvailability", "UPDATE catalog_listings SET sort_availability_mask=NULL WHERE id=?1".to_owned(), 50_000, true, 1),
            ("nullProjection", "UPDATE catalog_listings SET sort_metadata_projection=NULL WHERE id=?1".to_owned(), 50_000, false, 0),
            ("staleProjection", format!("UPDATE catalog_listings SET sort_projection_revision={} WHERE id=?1", revision - 1), 50_000, false, 0),
            ("metadataInvalidation", "UPDATE catalog_listings SET metadata=json_set(metadata,'$.title','Index invalidation control') WHERE id=?1".to_owned(), 50_000, true, 0),
        ] {
            conn.execute_batch("SAVEPOINT canonical_metrics_check").unwrap();
            if label == "aliasMembership" { conn.execute(&sql, params![first_id, second_id]).unwrap(); }
            else { conn.execute(&sql, [&first_id]).unwrap(); }
            let results = relations.iter().map(|(_, relation)| read(&format!("SELECT {} FROM {relation} WHERE {predicate}", projections[2].1), 2)).collect::<Vec<_>>();
            assert!(results.iter().all(|value| *value == results[0]));
            assert_eq!(results[0].0, expected_count);
            assert_eq!(crate::track_sort_sql::aggregate_result_is_pending(results[0].1), pending);
            assert_eq!(results[0].2, Some(projected));
            semantic_checks.push(serde_json::json!({"check":label,"identicalResult":results[0]}));
            if label == "demotedMembership" {
                conn.execute("UPDATE catalog_listings SET in_library=1 WHERE id=?1", [&first_id]).unwrap();
                for (_, relation) in relations {
                    let restored = read(&format!("SELECT {} FROM {relation} WHERE {predicate}", projections[2].1), 2);
                    assert_eq!(restored.0, 50_000);
                    assert!(!crate::track_sort_sql::aggregate_result_is_pending(restored.1));
                    assert_eq!(restored.2, Some(1));
                }
                semantic_checks.push(serde_json::json!({"check":"promotedMembership","identicalCount":50_000}));
            }

            conn.execute_batch("ROLLBACK TO canonical_metrics_check; RELEASE canonical_metrics_check;").unwrap();
        }
        let foreign_key_errors: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_errors, 0);
        println!(
            "{}",
            serde_json::json!({"benchmark":"canonical_library_metrics_partial_indexes","sqliteVersion":rusqlite::version(),"recordCount":50_000,"warmupsPerQuery":3,"measuredPerQuery":samples,"availabilityRevision":revision,"scope":"paired aggregate SQL on one disposable actual Store; forced baseline/index paths; excludes page hydration, mutex contention, GUI and end-to-end work; index build observations are not qualification","indexBuilds":index_builds,"phases":phases,"semanticChecks":semantic_checks,"foreignKeyErrors":foreign_key_errors})
        );
    }

    // Run only as the exact ignored test in a process with --test-threads=1:
    // hydration mode is a test-only process environment switch.
    fn benchmark_metrics_index_write_gate(
        store: &Store,
        samples: usize,
        shared_revision: i64,
        index_name: &str,
        toggle_index: &mut impl FnMut(bool),
        summary: fn(&[f64]) -> serde_json::Value,
    ) -> serde_json::Value {
        const CHANGED_ROWS: usize = 250;
        let ids = "SELECT id FROM temp.bench_metrics_write_ids";
        {
            let conn = store.connection.lock().unwrap();
            conn.execute_batch(&format!("CREATE TEMP TABLE bench_metrics_write_ids(id TEXT PRIMARY KEY); INSERT INTO bench_metrics_write_ids SELECT id FROM catalog_listings WHERE id=recording_id AND in_library=1 ORDER BY rowid LIMIT {CHANGED_ROWS};")).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM temp.bench_metrics_write_ids",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
                CHANGED_ROWS
            );
        }
        let projection = format!(
            "COUNT(*),sort_availability_or(CASE WHEN l.sort_availability_revision={shared_revision} THEN l.sort_availability_mask END),MIN(CASE WHEN l.sort_projection_revision={shared_revision} AND l.sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END)"
        );
        let check = |conn: &Connection,
                     indexed: bool,
                     count: usize,
                     pending: bool,
                     coverage: i64| {
            let mut relations = vec![
                "catalog_listings l NOT INDEXED".to_owned(),
                "catalog_listings l".to_owned(),
            ];
            if indexed {
                relations.push(format!("catalog_listings l INDEXED BY {index_name}"));
            }
            let mut expected = None;
            for relation in relations {
                let sql = format!(
                    "SELECT {projection} FROM {relation} WHERE l.id=l.recording_id AND l.in_library=1"
                );
                let actual: (usize, Option<i64>, Option<i64>) = conn
                    .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .unwrap_or_else(|error| {
                        panic!("write gate aggregate failed: {error}; SQL={sql}")
                    });
                assert_eq!(actual.0, count);
                assert_eq!(
                    crate::track_sort_sql::aggregate_result_is_pending(actual.1),
                    pending
                );
                assert_eq!(actual.2, Some(coverage));
                if let Some(expected) = expected {
                    assert_eq!(actual, expected);
                } else {
                    expected = Some(actual);
                }
            }
        };
        let assert_ready = |indexed: bool, bpm: f64| {
            let conn = store.connection.lock().unwrap();
            check(&conn, indexed, 50_000, false, 1);
            let ready:usize=conn.query_row(&format!("SELECT COUNT(*) FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id IN ({ids}) AND l.in_library=1 AND l.sort_availability_revision={shared_revision} AND l.sort_availability_mask IS NOT NULL AND l.sort_projection_revision={shared_revision} AND l.sort_metadata_projection IS NOT NULL AND CAST(json_extract(l.sort_metadata_projection,'$.bpm') AS REAL)=?1"),[bpm],|row|row.get(0)).unwrap();
            assert_eq!(ready, CHANGED_ROWS);
        };
        let mut semantics = Vec::new();
        for indexed in [false, true] {
            toggle_index(indexed);
            assert_ready(indexed, 128.0);
            let conn = store.connection.lock().unwrap();
            for (label, update, pending, coverage) in [
                (
                    "staleAvailability",
                    format!("sort_availability_revision={}", shared_revision - 1),
                    true,
                    1,
                ),
                (
                    "nullAvailability",
                    "sort_availability_mask=NULL".to_owned(),
                    true,
                    1,
                ),
                (
                    "nullAvailabilityRevision",
                    "sort_availability_revision=NULL".to_owned(),
                    true,
                    1,
                ),
                (
                    "staleProjection",
                    format!("sort_projection_revision={}", shared_revision - 1),
                    false,
                    0,
                ),
                (
                    "nullProjection",
                    "sort_metadata_projection=NULL".to_owned(),
                    false,
                    0,
                ),
                (
                    "nullProjectionRevision",
                    "sort_projection_revision=NULL".to_owned(),
                    false,
                    0,
                ),
            ] {
                conn.execute_batch("SAVEPOINT metrics_write_semantics")
                    .unwrap();
                conn.execute_batch(&format!("UPDATE catalog_listings SET {update} WHERE id=(SELECT id FROM temp.bench_metrics_write_ids LIMIT 1);")).unwrap();
                check(&conn, indexed, 50_000, pending, coverage);
                conn.execute_batch(
                    "ROLLBACK TO metrics_write_semantics; RELEASE metrics_write_semantics",
                )
                .unwrap();
                semantics.push(serde_json::json!({"indexed":indexed,"check":label,"passed":true}));
            }
            conn.execute_batch("SAVEPOINT metrics_write_alias").unwrap();
            conn.execute_batch("UPDATE catalog_listings SET recording_id=(SELECT id FROM temp.bench_metrics_write_ids ORDER BY id LIMIT 1 OFFSET 1) WHERE id=(SELECT id FROM temp.bench_metrics_write_ids ORDER BY id LIMIT 1)").unwrap();
            check(&conn, indexed, 49_999, false, 1);
            conn.execute_batch("ROLLBACK TO metrics_write_alias; RELEASE metrics_write_alias")
                .unwrap();
            semantics.push(
                serde_json::json!({"indexed":indexed,"check":"canonicalBecameAlias","passed":true}),
            );
            let integrity: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .unwrap();
            assert_eq!(integrity, "ok");
        }
        let mut edit = vec![Vec::new(); 2];
        let mut fill = edit.clone();
        let mut cycle = edit.clone();
        let mut restore = edit.clone();
        let mut restore_fill = edit.clone();
        let mut demote = edit.clone();
        let mut promote = edit.clone();
        let mut orders = Vec::new();
        for iteration in 0..samples + 3 {
            let order = if iteration % 2 == 0 { [0, 1] } else { [1, 0] };
            for index in order {
                let indexed = index == 1;
                toggle_index(indexed);
                assert_ready(indexed, 128.0);
                {
                    let conn = store.connection.lock().unwrap();
                    let clean:usize=conn.query_row(&format!("SELECT COUNT(*) FROM recordings WHERE id IN ({ids}) AND manual_metadata='{{}}'"),[],|row|row.get(0)).unwrap();
                    assert_eq!(clean, CHANGED_ROWS);
                }
                eprintln!(
                    "BURRATA_METRICS_WRITE_SAMPLE {}",
                    serde_json::json!({"iteration":iteration,"warmup":iteration<3,"indexed":indexed,"changedRows":CHANGED_ROWS})
                );
                let changed_at = Instant::now();
                let changed = {
                    let conn = store.connection.lock().unwrap();
                    conn.execute(&format!("UPDATE recordings SET manual_metadata=json_set(manual_metadata,'$.bpm',129) WHERE id IN ({ids})"),[]).unwrap()
                };
                let edit_ms = changed_at.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(changed, CHANGED_ROWS);
                {
                    let conn = store.connection.lock().unwrap();
                    check(&conn, indexed, 50_000, true, 0);
                }
                let fill_at = Instant::now();
                let processed = store
                    .backfill_sort_availability_batch(CHANGED_ROWS)
                    .unwrap();
                let fill_ms = fill_at.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(processed, CHANGED_ROWS);
                assert_ready(indexed, 129.0);
                let restore_at = Instant::now();
                let restored = {
                    let conn = store.connection.lock().unwrap();
                    conn.execute(
                        &format!(
                            "UPDATE recordings SET manual_metadata='{{}}' WHERE id IN ({ids})"
                        ),
                        [],
                    )
                    .unwrap()
                };
                let restore_ms = restore_at.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(restored, CHANGED_ROWS);
                {
                    let conn = store.connection.lock().unwrap();
                    check(&conn, indexed, 50_000, true, 0);
                }
                let restore_fill_at = Instant::now();
                let processed = store
                    .backfill_sort_availability_batch(CHANGED_ROWS)
                    .unwrap();
                let restore_fill_ms = restore_fill_at.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(processed, CHANGED_ROWS);
                assert_ready(indexed, 128.0);
                let demote_at = Instant::now();
                let demoted = {
                    let conn = store.connection.lock().unwrap();
                    conn.execute(
                        &format!("UPDATE catalog_listings SET in_library=0 WHERE id IN ({ids})"),
                        [],
                    )
                    .unwrap()
                };
                let demote_ms = demote_at.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(demoted, CHANGED_ROWS);
                {
                    let conn = store.connection.lock().unwrap();
                    check(&conn, indexed, 50_000 - CHANGED_ROWS, false, 1);
                }
                let promote_at = Instant::now();
                let promoted = {
                    let conn = store.connection.lock().unwrap();
                    conn.execute(
                        &format!("UPDATE catalog_listings SET in_library=1 WHERE id IN ({ids})"),
                        [],
                    )
                    .unwrap()
                };
                let promote_ms = promote_at.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(promoted, CHANGED_ROWS);
                assert_ready(indexed, 128.0);
                if iteration >= 3 {
                    edit[index].push(edit_ms);
                    fill[index].push(fill_ms);
                    cycle[index].push(edit_ms + fill_ms);
                    restore[index].push(restore_ms);
                    restore_fill[index].push(restore_fill_ms);
                    demote[index].push(demote_ms);
                    promote[index].push(promote_ms);
                }
            }
            orders.push(order);
        }
        let conn = store.connection.lock().unwrap();
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let foreign_keys: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_keys, 0);
        conn.execute_batch("DROP TABLE temp.bench_metrics_write_ids")
            .unwrap();
        let variants = [0,1].iter().map(|&index|serde_json::json!({"indexed":index==1,"metadataInvalidation":summary(&edit[index]),"oneProductionSizedBackfillBatch":summary(&fill[index]),"invalidationPlusBackfillStageSum":summary(&cycle[index]),"restoreInvalidation":summary(&restore[index]),"restoreBackfill":summary(&restore_fill[index]),"membershipRemoval":summary(&demote[index]),"membershipPromotion":summary(&promote[index])})).collect::<Vec<_>>();
        serde_json::json!({"recordCount":50_000,"changedRows":CHANGED_ROWS,"backfillBatchSize":CHANGED_ROWS,"warmupsPerVariant":3,"measuredPairs":samples,"variantOrderByIteration":orders,"semanticChecks":semantics,"integrity":integrity,"foreignKeyErrors":foreign_keys,"variants":variants,"scope":"paired actual metadata-trigger invalidation, 250-row projection backfill and partial-membership updates on one ready metadata-only Store; index DDL and semantic verification excluded from stage timers","limitations":["Stage sums exclude intervening semantic queries and are not contiguous end-to-end cycle latency.","Exactly one 250-row backfill batch is measured; no empty terminal scan or full-library migration cost is claimed.","Membership writes exercise partial-index maintenance directly, not complete UI import/remove command latency.","No assets, file I/O, analyses, GUI, IPC, CPU/RSS or concurrent-writer qualification."]})
    }

    // Seed non-audio files plus metadata-only LocalAsset rows for the opt-in
    // paired page benchmark. This measures filesystem presence checks and
    // analysis hydration, not decoding or playback of audio.
    fn seed_paired_page_asset_fixture(
        store: &Store,
        fixture_dir: &Path,
        asset_track_count: usize,
    ) -> serde_json::Value {
        assert!(matches!(asset_track_count, 1_000 | 5_000 | 20_000 | 50_000));
        let files_dir = fixture_dir.join("paired-page-assets");
        std::fs::create_dir_all(&files_dir).unwrap();
        let file_count = asset_track_count;
        let file_bytes = {
            let mut bytes = b"SYNTHETIC NON-AUDIO FILE FIXTURE ".to_vec();
            bytes.resize(1_024, b'x');
            bytes
        };
        for index in 0..file_count {
            std::fs::write(
                files_dir.join(format!("present-{index:05}.bin")),
                &file_bytes,
            )
            .unwrap();
        }
        let missing_path = files_dir.join("missing-fixture-file.bin");
        let (physical_file_count, physical_file_bytes) = std::fs::read_dir(&files_dir)
            .unwrap()
            .map(|entry| {
                let metadata = entry.unwrap().metadata().unwrap();
                (1usize, metadata.len() as usize)
            })
            .fold(
                (0usize, 0usize),
                |(count, bytes), (next_count, next_bytes)| (count + next_count, bytes + next_bytes),
            );
        assert_eq!(physical_file_count, file_count);
        assert_eq!(physical_file_bytes, file_bytes.len() * file_count);
        let mut asset_rows = 0usize;
        let mut existing_asset_rows = 0usize;
        let mut missing_asset_rows = 0usize;
        let mut multiple_asset_tracks = 0usize;
        let mut preferred_asset_tracks = 0usize;
        let mut preferred_missing_tracks = 0usize;
        let mut exact_analysis_rows = 0usize;
        let mut stale_hash_analysis_rows = 0usize;
        {
            let mut conn = store.connection.lock().unwrap();
            let tx = conn.transaction().unwrap();
            let mut insert_asset = tx.prepare(
                "INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?1,?2,?3,?4,?5)",
            ).unwrap();
            let mut set_preference = tx.prepare(
                "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            ).unwrap();
            let mut insert_analysis = tx.prepare(
                "INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?1,?2,?3,'completed',?4,?5)",
            ).unwrap();
            for index in 0..asset_track_count {
                let recording_id = format!("fixture:metrics-benchmark:{index:05}");
                let source = Track {
                    id: recording_id.clone(),
                    ..Track::default()
                };
                let first_id = format!("bench-asset-first:{index:05}");
                let second_id = format!("bench-asset-second:{index:05}");
                let present_path = files_dir.join(format!("present-{index:05}.bin"));
                let kind = index % 10;
                let mut rows: Vec<(String, std::path::PathBuf, String)> = Vec::new();
                match kind {
                    5 | 6 => {
                        rows.push((
                            first_id.clone(),
                            missing_path.clone(),
                            format!("missing-first-{index:05}"),
                        ));
                        rows.push((
                            second_id.clone(),
                            present_path.clone(),
                            format!("present-second-{index:05}"),
                        ));
                        set_preference
                            .execute(params![
                                format!("preferredAsset.{recording_id}"),
                                encode(&second_id).unwrap()
                            ])
                            .unwrap();
                        multiple_asset_tracks += 1;
                        preferred_asset_tracks += 1;
                    }
                    7 => {
                        rows.push((
                            first_id.clone(),
                            present_path.clone(),
                            format!("present-first-{index:05}"),
                        ));
                        rows.push((
                            second_id.clone(),
                            missing_path.clone(),
                            format!("missing-preferred-{index:05}"),
                        ));
                        set_preference
                            .execute(params![
                                format!("preferredAsset.{recording_id}"),
                                encode(&second_id).unwrap()
                            ])
                            .unwrap();
                        multiple_asset_tracks += 1;
                        preferred_asset_tracks += 1;
                        preferred_missing_tracks += 1;
                    }
                    8 => rows.push((
                        first_id.clone(),
                        missing_path.clone(),
                        format!("missing-only-{index:05}"),
                    )),
                    _ => rows.push((
                        first_id.clone(),
                        present_path.clone(),
                        format!("present-first-{index:05}"),
                    )),
                }
                for (asset_id, path, hash) in &rows {
                    let value = asset(asset_id, &source, path, hash);
                    insert_asset
                        .execute(params![
                            asset_id,
                            recording_id,
                            path.to_string_lossy(),
                            hash,
                            encode(&value).unwrap()
                        ])
                        .unwrap();
                    asset_rows += 1;
                    if path.is_file() {
                        existing_asset_rows += 1;
                    } else {
                        missing_asset_rows += 1;
                    }
                }
                if index % 4 == 0 {
                    let selected = rows
                        .iter()
                        .find(|(_, path, _)| path.is_file())
                        .unwrap_or(&rows[0]);
                    let (asset_id, _, hash) = selected;
                    insert_analysis
                        .execute(params![
                            asset_id,
                            hash,
                            "fixture-analysis-v1",
                            r#"{"provenance":{"fixture":"exact-hash"}}"#,
                            "2026-01-01T00:00:00Z"
                        ])
                        .unwrap();
                    insert_analysis
                        .execute(params![
                            asset_id,
                            format!("stale-{hash}"),
                            "fixture-analysis-stale",
                            "INVALID STALE HASH PAYLOAD",
                            "2026-01-02T00:00:00Z"
                        ])
                        .unwrap();
                    exact_analysis_rows += 1;
                    stale_hash_analysis_rows += 1;
                }
            }
            drop(insert_analysis);
            drop(set_preference);
            drop(insert_asset);
            tx.commit().unwrap();
        }
        let (
            asset_payload_bytes,
            analysis_payload_bytes,
            asset_table_bytes,
            analysis_table_bytes,
            database_page_bytes,
        ): (i64, i64, Option<i64>, Option<i64>, i64) = {
            let conn = store.connection.lock().unwrap();
            let asset_payload_bytes = conn.query_row(
                "SELECT COALESCE(SUM(length(payload)),0) FROM local_assets WHERE recording_id LIKE 'fixture:metrics-benchmark:%'", [], |row| row.get(0)
            ).unwrap();
            let analysis_payload_bytes = conn.query_row(
                "SELECT COALESCE(SUM(length(payload)),0) FROM asset_analyses WHERE version LIKE 'fixture-analysis-%'", [], |row| row.get(0)
            ).unwrap();
            let page_count: i64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .unwrap();
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .unwrap();
            let asset_table_bytes: Option<i64> = conn
                .query_row(
                    "SELECT SUM(pgsize) FROM dbstat WHERE name='local_assets'",
                    [],
                    |row| row.get(0),
                )
                .ok()
                .flatten();
            let analysis_table_bytes: Option<i64> = conn
                .query_row(
                    "SELECT SUM(pgsize) FROM dbstat WHERE name='asset_analyses'",
                    [],
                    |row| row.get(0),
                )
                .ok()
                .flatten();
            (
                asset_payload_bytes,
                analysis_payload_bytes,
                asset_table_bytes,
                analysis_table_bytes,
                page_count * page_size,
            )
        };
        let mut counts = serde_json::json!({
            "enabled": true,
            "requestedAssetTracks": asset_track_count,
            "assetRows": asset_rows,
            "existingAssetRows": existing_asset_rows,
            "missingAssetRows": missing_asset_rows,
            "multipleAssetTracks": multiple_asset_tracks,
            "preferredAssetTracks": preferred_asset_tracks,
            "preferredMissingTracks": preferred_missing_tracks,
            "exactAnalysisRows": exact_analysis_rows,
            "staleHashAnalysisRows": stale_hash_analysis_rows,
        });
        counts["physicalFixtureFiles"] = physical_file_count.into();
        counts["physicalFixtureFileBytes"] = physical_file_bytes.into();
        counts["localAssetPayloadBytes"] = asset_payload_bytes.into();
        counts["analysisPayloadBytes"] = analysis_payload_bytes.into();
        counts["localAssetsAndAnalysesTableBytes"] = serde_json::to_value(
            asset_table_bytes
                .zip(analysis_table_bytes)
                .map(|(assets, analyses)| assets + analyses),
        )
        .unwrap();
        counts["localAssetsTableBytes"] = serde_json::to_value(asset_table_bytes).unwrap();
        counts["assetAnalysesTableBytes"] = serde_json::to_value(analysis_table_bytes).unwrap();
        counts["databasePageBytesAfterFixture"] = database_page_bytes.into();
        counts["classification"] =
            "synthetic metadata and non-audio filesystem fixture; not audio/playback qualification"
                .into();
        counts
    }

    fn benchmark_paired_page_hydration_and_metrics_index(
        store: &Store,
        samples: usize,
        shared_revision: i64,
        asset_fixture: serde_json::Value,
    ) {
        // Production currently uses one availability revision for both masks
        // and persisted sort projections; match that exact expression here.
        // If production separates their revisions, update both this CASE and
        // the index naming/diagnostics together rather than inventing a value.
        struct HydrationMode(Option<std::ffi::OsString>);
        impl HydrationMode {
            fn set(individual: bool) -> Self {
                let previous = std::env::var_os("BURRATA_PAGE_HYDRATION_MODE");
                std::env::set_var(
                    "BURRATA_PAGE_HYDRATION_MODE",
                    if individual { "individual" } else { "batch" },
                );
                Self(previous)
            }
        }
        impl Drop for HydrationMode {
            fn drop(&mut self) {
                if let Some(previous) = &self.0 {
                    std::env::set_var("BURRATA_PAGE_HYDRATION_MODE", previous);
                } else {
                    std::env::remove_var("BURRATA_PAGE_HYDRATION_MODE");
                }
            }
        }
        fn summary(samples: &[f64]) -> serde_json::Value {
            let mut sorted = samples.to_vec();
            sorted.sort_by(f64::total_cmp);
            let middle = sorted.len() / 2;
            let median = if sorted.len() % 2 == 0 {
                (sorted[middle - 1] + sorted[middle]) / 2.0
            } else {
                sorted[middle]
            };
            serde_json::json!({"samplesMs":samples,"measured":samples.len(),"medianMs":median,"p95Ms":sorted[((sorted.len() as f64*0.95).ceil() as usize)-1],"worstMs":sorted.last()})
        }
        fn read_page(store: &Store, sort: &str) -> (f64, serde_json::Value) {
            let started = Instant::now();
            let (tracks, total, availability) = store
                .list_tracks_page_with_availability(
                    "library",
                    None,
                    200,
                    0,
                    None,
                    Some(sort),
                    Some("asc"),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(total, 50_000);
            assert_eq!(tracks.len(), 200);
            assert!(
                availability.is_some(),
                "ready fixture must retain global availability"
            );
            // Complete hydrated rows, order, count and global field map, checked
            // outside timing; hashes/IDs alone could hide changed overlays.
            (
                elapsed,
                serde_json::json!({"tracks":tracks,"total":total,"availability":availability}),
            )
        }
        let index_name = format!("bench_paired_metrics_coverage_r{shared_revision}");
        let mut index_state = false;
        let mut index_operations = Vec::new();
        let mut toggle_index = |enabled: bool| {
            if enabled != index_state {
                let conn = store.connection.lock().unwrap();
                let started = Instant::now();
                if enabled {
                    conn.execute_batch(&format!("CREATE INDEX {index_name} ON catalog_listings(sort_availability_revision,sort_availability_mask,(CASE WHEN sort_projection_revision={shared_revision} AND sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END)) WHERE id=recording_id AND in_library=1;")).unwrap();
                } else {
                    conn.execute_batch(&format!("DROP INDEX {index_name};"))
                        .unwrap();
                }
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let bytes: Option<i64> = if enabled {
                    conn.query_row(
                        "SELECT SUM(pgsize) FROM dbstat WHERE name=?1",
                        [&index_name],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten()
                } else {
                    None
                };
                index_operations.push(serde_json::json!({"enabled":enabled,"wallMs":elapsed_ms,"indexBytesIfDbstatAvailable":bytes}));
                index_state = enabled;
            }
        };
        if std::env::var("BURRATA_METRICS_INDEX_WRITE_GATE")
            .ok()
            .as_deref()
            == Some("1")
        {
            let gate = benchmark_metrics_index_write_gate(
                store,
                samples,
                shared_revision,
                &index_name,
                &mut toggle_index,
                summary,
            );
            toggle_index(false);
            drop(toggle_index);
            println!(
                "{}",
                serde_json::json!({"benchmark":"paired_metrics_index_projection_membership_writes","sqliteVersion":rusqlite::version(),"sharedAvailabilityAndProjectionRevision":shared_revision,"candidateProductionAdopted":false,"candidateIndexName":index_name,"assetFixtureEnabled":false,"exploratoryRun":samples==3,"indexOperationsOutsideStageTiming":index_operations,"gate":gate})
            );
            return;
        }
        let variants = [(true, false), (false, false), (true, true), (false, true)];
        let variant_names = [
            "individualIndexOff",
            "batchIndexOff",
            "individualIndexOn",
            "batchIndexOn",
        ];
        let mut phases = Vec::new();
        for sort in ["sourceOrder", "bpm"] {
            let mut timings = vec![Vec::new(); variants.len()];
            let mut expected = None;
            let mut sample_order = Vec::new();
            let mut diagnostic_plans = Vec::new();
            for iteration in 0..samples + 3 {
                let order = (0..variants.len())
                    .map(|index| (index + iteration) % variants.len())
                    .collect::<Vec<_>>();
                for &index in &order {
                    let (individual, indexed) = variants[index];
                    toggle_index(indexed); // DDL/index warming excluded from page timer.
                    let _mode = HydrationMode::set(individual);
                    if iteration == 0 {
                        let projection = if sort == "bpm" {
                            format!(
                                ",MIN(CASE WHEN l.sort_projection_revision={shared_revision} AND l.sort_metadata_projection IS NOT NULL THEN 1 ELSE 0 END)"
                            )
                        } else {
                            String::new()
                        };
                        let sql = format!(
                            "SELECT COUNT(*),sort_availability_or(CASE WHEN l.sort_availability_revision={shared_revision} THEN l.sort_availability_mask END){projection} FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1"
                        );
                        let conn = store.connection.lock().unwrap();
                        let plan = conn
                            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                            .unwrap()
                            .query_map([], |row| row.get::<_, String>(3))
                            .unwrap()
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .unwrap();
                        diagnostic_plans.push(serde_json::json!({"variant":variant_names[index],"fixtureMetricsSql":sql,"plan":plan}));
                    }
                    eprintln!(
                        "BURRATA_PAIRED_PAGE_SAMPLE {}",
                        serde_json::json!({"sort":sort,"iteration":iteration,"warmup":iteration<3,"variant":variant_names[index],"contention":false})
                    );
                    let (elapsed, actual) = read_page(store, sort);
                    if let Some(expected) = &expected {
                        assert_eq!(
                            &actual, expected,
                            "whole-page output changed for {} ({sort})",
                            variant_names[index]
                        );
                    } else {
                        expected = Some(actual);
                    }
                    if iteration >= 3 {
                        timings[index].push(elapsed);
                    }
                }
                sample_order.push(order);
            }
            phases.push(serde_json::json!({"sortField":sort,"direction":"asc","offset":0,"fullHydratedOutputIdentical":true,"fixtureMetricsPlansOutsideTiming":diagnostic_plans,"variantOrderByIteration":sample_order,"variants":variant_names.iter().enumerate().map(|(index,name)|serde_json::json!({"variant":name,"timing":summary(&timings[index])})).collect::<Vec<_>>()}));
        }
        let mut contention = serde_json::Value::Null;
        let contention_enabled = std::env::var("BURRATA_AVAILABILITY_BENCH_CONTENTION")
            .ok()
            .as_deref()
            == Some("1");
        if contention_enabled {
            let track = store
                .get_track("fixture:metrics-benchmark:00000")
                .unwrap()
                .unwrap();
            let mut readers = vec![Vec::new(); variants.len()];
            let mut writers = vec![Vec::new(); variants.len()];
            let mut order_rows = Vec::new();
            for iteration in 0..samples + 3 {
                let order = (0..variants.len())
                    .map(|index| (index + iteration) % variants.len())
                    .collect::<Vec<_>>();
                for &index in &order {
                    let (individual, indexed) = variants[index];
                    toggle_index(indexed);
                    let _mode = HydrationMode::set(individual);
                    let next_favorite = !store.get_track(&track.id).unwrap().unwrap().favorite;
                    eprintln!(
                        "BURRATA_PAIRED_PAGE_SAMPLE {}",
                        serde_json::json!({"sort":"sourceOrder","iteration":iteration,"warmup":iteration<3,"variant":variant_names[index],"contention":true})
                    );
                    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
                    let (read_ms, write_ms) = std::thread::scope(|scope| {
                        let read_barrier = barrier.clone();
                        let read = scope.spawn(move || {
                            read_barrier.wait();
                            read_page(store, "sourceOrder").0
                        });
                        let write_barrier = barrier.clone();
                        let track = &track;
                        let write = scope.spawn(move || {
                            write_barrier.wait();
                            let started = Instant::now();
                            store.set_favorite(track, next_favorite).unwrap();
                            started.elapsed().as_secs_f64() * 1000.0
                        });
                        (read.join().unwrap(), write.join().unwrap())
                    });
                    if iteration >= 3 {
                        readers[index].push(read_ms);
                        writers[index].push(write_ms);
                    }
                }
                order_rows.push(order);
            }
            contention = serde_json::json!({"sortField":"sourceOrder","measuredPairsPerVariant":samples,"warmupsPerVariant":3,"variantOrderByIteration":order_rows,"variants":variant_names.iter().enumerate().map(|(index,name)|serde_json::json!({"variant":name,"readerFirstPage":summary(&readers[index]),"writerFavoriteUpdate":summary(&writers[index])})).collect::<Vec<_>>(),"scope":"separate reader/favorite-writer observations on the same Store mutex, excludes DDL; favorite toggles each write; not merged with uncontended page samples"});
        }
        toggle_index(false);
        drop(toggle_index);
        let foreign_key_errors: i64 = store
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_errors, 0);
        println!(
            "{}",
            serde_json::json!({"benchmark":"paired_page_hydration_metrics_index","sqliteVersion":rusqlite::version(),"recordCount":50_000,"pageSize":200,"warmupsPerVariant":3,"measuredPerVariant":samples,"exploratoryRun":samples==3,"scope":"paired 2x2 actual Store whole-page reads; same ready 50k fixture; excludes GUI, IPC, audio, fixture setup and index DDL","assetFixtureEnabled":asset_fixture["enabled"],"assetFixture":asset_fixture,"assetLimitation":"Synthetic LocalAsset metadata and non-audio files exercise existence checks and exact-analysis hydration; not audio decode, audio qualification, or user-library distribution.","bpmDistribution":"constant 128 BPM, tie-heavy; not varied-BPM qualification","candidateProductionAdopted":false,"sharedAvailabilityAndProjectionRevision":shared_revision,"candidateIndexName":index_name,"indexOperationsOutsidePageTiming":index_operations,"phases":phases,"contention":contention,"foreignKeyErrors":foreign_key_errors,"limitations":["Index creation/deletion between samples changes cache state; no claim of cold filesystem or independent index warm-state equivalence.","Index-operation samples are unpaired diagnostic observations, not build-cost qualification.","Environment mode mutation requires this exact ignored test alone with --test-threads=1."]})
        );
    }

    #[test]
    #[ignore = "paired aggregate benchmark; coordinate explicitly; BURRATA_METRICS_JOIN_BENCH_SAMPLES=3 or 20"]
    fn benchmark_library_metrics_join_elision() {
        let (fixture_dir, store) = db();
        let samples = std::env::var("BURRATA_METRICS_JOIN_BENCH_SAMPLES")
            .ok()
            .map(|value| value.parse::<usize>().unwrap())
            .unwrap_or(3);
        assert!(matches!(samples, 3 | 20));
        let paired_page_mode = std::env::var("BURRATA_PAGE_HYDRATION_PAIRED_BENCH")
            .ok()
            .as_deref()
            == Some("1");
        let asset_track_count = if paired_page_mode {
            std::env::var("BURRATA_PAIRED_PAGE_ASSET_COUNT")
                .ok()
                .map(|value| value.parse::<usize>().unwrap())
                .unwrap_or(0)
        } else {
            0
        };
        assert!(matches!(
            asset_track_count,
            0 | 1_000 | 5_000 | 20_000 | 50_000
        ));
        assert!(
            asset_track_count == 0
                || std::env::var("BURRATA_METRICS_INDEX_WRITE_GATE")
                    .ok()
                    .as_deref()
                    != Some("1"),
            "asset-backed fixture is incompatible with the metadata-only metrics index write gate"
        );
        let mut value = track("metrics-benchmark", "");
        value.album = Some("Benchmark album".into());
        value.duration_seconds = Some(180.0);
        value.bpm = Some(128.0);
        value.key = Some("8A".into());
        value.release_date = Some("2024-01-02".into());
        value.provenance = serde_json::json!({"dateAdded":"2025-01-02","plays":0,"views":"1.2k"});
        {
            let mut conn = store.connection.lock().unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute("INSERT INTO works(id,title,artist) VALUES('metrics-benchmark-work','Benchmark work','Benchmark artist')", []).unwrap();
            {
                let mut recording = tx
                    .prepare(
                        "INSERT INTO recordings(id,work_id) VALUES(?1,'metrics-benchmark-work')",
                    )
                    .unwrap();
                let mut listing = tx.prepare("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at,in_library) VALUES(?1,'fixture',?2,?1,'',?3,'2026-01-01',1)").unwrap();
                for index in 0..50_000 {
                    value.provider_id = format!("metrics-benchmark:{index:05}");
                    value.id = format!("fixture:{}", value.provider_id);
                    value.title = format!("Benchmark track {index:05}");
                    recording.execute([&value.id]).unwrap();
                    listing
                        .execute(params![
                            value.id,
                            value.provider_id,
                            encode(&value).unwrap()
                        ])
                        .unwrap();
                }
            }
            tx.commit().unwrap();
        }
        let asset_fixture = if asset_track_count == 0 {
            serde_json::json!({"enabled":false,"requestedAssetTracks":0,"classification":"existing metadata-only fixture"})
        } else {
            seed_paired_page_asset_fixture(&store, fixture_dir.path(), asset_track_count)
        };
        while store.backfill_sort_availability_batch(500).unwrap() != 0 {}
        let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
        if std::env::var("BURRATA_PAGE_HYDRATION_PAIRED_BENCH")
            .ok()
            .as_deref()
            == Some("1")
        {
            assert!(
                std::env::var_os("BURRATA_METRICS_COVER_INDEX_BENCH").is_none(),
                "select exactly one candidate benchmark mode"
            );
            benchmark_paired_page_hydration_and_metrics_index(
                &store,
                samples,
                revision,
                asset_fixture,
            );
            return;
        }
        let conn = store.connection.lock().unwrap();
        if std::env::var_os("BURRATA_METRICS_COVER_INDEX_BENCH").is_some() {
            benchmark_canonical_metrics_indexes(&conn, samples, revision);
            return;
        }
        let mut output = Vec::new();
        for (phase, projection) in [
            ("count", "COUNT(*)".to_string()),
            (
                "countAndAvailability",
                format!(
                    "COUNT(*),sort_availability_or(CASE WHEN l.sort_availability_revision={revision} THEN l.sort_availability_mask END)"
                ),
            ),
        ] {
            let queries = [
                format!(
                    "SELECT {projection} FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id WHERE l.id=l.recording_id AND l.in_library=1"
                ),
                format!(
                    "SELECT {projection} FROM catalog_listings l WHERE l.id=l.recording_id AND l.in_library=1"
                ),
            ];
            let plans = queries.iter().map(|sql| {
                conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap().query_map([], |row| Ok(serde_json::json!({"id":row.get::<_, i64>(0)?,"parent":row.get::<_, i64>(1)?,"detail":row.get::<_, String>(3)?}))).unwrap().collect::<std::result::Result<Vec<_>, _>>().unwrap()
            }).collect::<Vec<_>>();
            let mut timings = [Vec::new(), Vec::new()];
            let mut expected = None;
            for iteration in 0..samples + 3 {
                // Alternate pair order to avoid consistently favoring one query.
                for index in if iteration % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let started = Instant::now();
                    let result: (usize, Option<i64>) = conn
                        .query_row(&queries[index], [], |row| {
                            Ok((
                                row.get(0)?,
                                if phase == "count" { None } else { row.get(1)? },
                            ))
                        })
                        .unwrap();
                    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                    assert_eq!(result.0, 50_000);
                    if let Some(expected) = expected {
                        assert_eq!(result, expected);
                    } else {
                        expected = Some(result);
                    }
                    if iteration >= 3 {
                        timings[index].push(elapsed);
                    }
                }
            }
            let summaries = timings.into_iter().map(|samples| {
                let mut sorted = samples.clone();
                sorted.sort_by(f64::total_cmp);
                serde_json::json!({"samplesMs":samples,"medianMs":if sorted.len() % 2 == 0 {(sorted[sorted.len()/2 - 1] + sorted[sorted.len()/2]) / 2.0} else {sorted[sorted.len()/2]},"p95Ms":sorted[((sorted.len() as f64 * 0.95).ceil() as usize)-1],"worstMs":sorted.last()})
            }).collect::<Vec<_>>();
            output.push(serde_json::json!({"phase":phase,"originalJoined":{"sql":queries[0],"plan":plans[0],"timings":summaries[0]},"eligibleListingsOnly":{"sql":queries[1],"plan":plans[1],"timings":summaries[1]},"identicalResult":expected}));
        }
        println!(
            "{}",
            serde_json::json!({"benchmark":"library_metrics_join_elision","recordCount":50_000,"warmupsPerQuery":3,"measuredPerQuery":samples,"scope":"paired SQL aggregates on one actual Store connection; excludes hydration, mutex contention, GUI and end-to-end work","phases":output})
        );
    }

    #[test]
    #[ignore = "storage-only sort-availability microbenchmark; run explicitly with --ignored --nocapture"]
    fn benchmark_first_page_global_sort_availability() {
        use std::time::Instant;

        fn summarize(samples: Vec<f64>) -> serde_json::Value {
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            let p95_index = ((samples.len() as f64 * 0.95).ceil() as usize)
                .saturating_sub(1)
                .min(sorted.len().saturating_sub(1));
            let middle = sorted.len() / 2;
            let median = if sorted.len() % 2 == 0 {
                (sorted[middle - 1] + sorted[middle]) / 2.0
            } else {
                sorted[middle]
            };
            serde_json::json!({
                "warmups": 3,
                "measured": samples.len(),
                "metric": "wall_elapsed_ms",
                "samplesMs": samples,
                "medianMs": median,
                "p95Ms": sorted[p95_index],
                "worstMs": sorted.last().copied().unwrap_or_default(),
            })
        }

        fn run_read(
            store: &Store,
            record_count: usize,
            with_availability: bool,
            sparse: bool,
        ) -> f64 {
            let start = Instant::now();
            if with_availability {
                let (page, total, availability) = store
                    .list_tracks_page_with_availability(
                        "library",
                        None,
                        200,
                        0,
                        None,
                        Some("sourceOrder"),
                        Some("asc"),
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                assert_eq!(total, record_count);
                assert_eq!(page.len(), record_count.min(200));
                let availability = availability.expect("first page includes availability");
                for (field, expected) in [
                    ("sourceOrder", true),
                    ("title", true),
                    ("artist", true),
                    ("album", true),
                    ("duration", true),
                    ("releaseDate", !sparse),
                    ("dateAdded", true),
                    ("bpm", !sparse),
                    ("key", !sparse),
                    ("plays", !sparse),
                    ("views", !sparse),
                ] {
                    assert_eq!(availability.get(field), Some(&expected), "{field}");
                }
            } else {
                let (page, total) = store
                    .list_tracks_page(
                        "library",
                        None,
                        200,
                        0,
                        None,
                        Some("sourceOrder"),
                        Some("asc"),
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                assert_eq!(total, record_count);
                assert_eq!(page.len(), record_count.min(200));
            }
            start.elapsed().as_secs_f64() * 1000.0
        }

        fn run_contention_phase(
            store: &Store,
            record_count: usize,
            track: &Track,
            measured: usize,
        ) -> serde_json::Value {
            let mut read_ms = Vec::with_capacity(measured);
            let mut write_ms = Vec::with_capacity(measured);
            let total_iterations = measured + 3;
            for iteration in 0..total_iterations {
                let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
                let (read_elapsed, write_elapsed) = std::thread::scope(|scope| {
                    let read_barrier = barrier.clone();
                    let read = scope.spawn(move || {
                        read_barrier.wait();
                        let started = Instant::now();
                        let (page, total, availability) = store
                            .list_tracks_page_with_availability(
                                "library",
                                None,
                                200,
                                0,
                                None,
                                Some("sourceOrder"),
                                Some("asc"),
                                None,
                                None,
                                None,
                                None,
                            )
                            .unwrap();
                        assert_eq!(total, record_count);
                        assert_eq!(page.len(), 200);
                        assert!(availability.is_some());
                        started.elapsed().as_secs_f64() * 1000.0
                    });
                    let write_barrier = barrier.clone();
                    let write = scope.spawn(move || {
                        write_barrier.wait();
                        let started = Instant::now();
                        store.set_favorite(track, iteration % 2 == 0).unwrap();
                        started.elapsed().as_secs_f64() * 1000.0
                    });
                    (read.join().unwrap(), write.join().unwrap())
                });
                if iteration >= 3 {
                    read_ms.push(read_elapsed);
                    write_ms.push(write_elapsed);
                }
            }
            serde_json::json!({
                "fixture": "50,000-row ready source-order page and concurrent favorite write sharing one Store mutex",
                "warmups": 3,
                "measuredPairs": measured,
                "readerFirstPage": summarize(read_ms),
                "writerFavoriteUpdate": summarize(write_ms),
                "note": "Contention observations are reported separately and excluded from uncontended qualification samples."
            })
        }

        let sparse = std::env::var("BURRATA_AVAILABILITY_BENCH_SPARSE")
            .ok()
            .as_deref()
            == Some("1");
        let measured_samples = std::env::var("BURRATA_AVAILABILITY_BENCH_SAMPLES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(3, 20))
            .unwrap_or(20);
        let contention_enabled = std::env::var("BURRATA_AVAILABILITY_BENCH_CONTENTION")
            .ok()
            .as_deref()
            == Some("1");
        let mut output = Vec::new();
        for record_count in [1_000usize, 5_000, 20_000, 50_000] {
            let (directory, store) = db();
            {
                let mut connection = store.connection.lock().unwrap();
                let transaction = connection.transaction().unwrap();
                let mut work = transaction
                    .prepare("INSERT INTO works(id,title,artist) VALUES(?1,'Benchmark work','Benchmark artist')")
                    .unwrap();
                let mut recording = transaction
                    .prepare("INSERT INTO recordings(id,work_id,version) VALUES(?1,?2,NULL)")
                    .unwrap();
                let mut listing = transaction
                    .prepare("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?1,'benchmark',?2,?1,'',?3,'2026-01-01T00:00:00Z')")
                    .unwrap();
                for index in 0..record_count {
                    let id = format!("benchmark:{index:05}");
                    let mut provenance = serde_json::json!({"dateAdded":"2025-01-02"});
                    if !sparse {
                        provenance["plays"] = serde_json::json!(0);
                        provenance["views"] = serde_json::json!("1.2k");
                    }
                    let track = Track {
                        id: id.clone(),
                        provider: "benchmark".into(),
                        provider_id: format!("{index:05}"),
                        artist: "Benchmark artist".into(),
                        title: format!("Benchmark track {index:05}"),
                        album: Some("Benchmark album".into()),
                        duration_seconds: Some(180.0),
                        bpm: (!sparse).then_some(128.0),
                        key: (!sparse).then(|| "8A".into()),
                        release_date: (!sparse).then(|| "2024-01-02".into()),
                        provenance,
                        ..Track::default()
                    };
                    let metadata = encode(&track).unwrap();
                    let work_id = format!("work:{id}");
                    work.execute([&work_id]).unwrap();
                    recording.execute(rusqlite::params![id, work_id]).unwrap();
                    listing
                        .execute(rusqlite::params![id, format!("{index:05}"), metadata])
                        .unwrap();
                }
                drop(listing);
                drop(recording);
                drop(work);
                transaction.commit().unwrap();
            }

            let mut legacy = Vec::with_capacity(measured_samples);
            let mut global = Vec::with_capacity(measured_samples);
            for _ in 0..3 {
                run_read(&store, record_count, false, sparse);
                run_read(&store, record_count, true, sparse);
            }
            for _ in 0..measured_samples {
                legacy.push(run_read(&store, record_count, false, sparse));
                global.push(run_read(&store, record_count, true, sparse));
            }
            let contention = if contention_enabled && record_count == 50_000 {
                let track = store.get_track("benchmark:00000").unwrap().unwrap();
                Some(run_contention_phase(
                    &store,
                    record_count,
                    &track,
                    measured_samples,
                ))
            } else {
                None
            };
            output.push(serde_json::json!({
                "recordCount": record_count,
                "view": "library",
                "sortField": "sourceOrder",
                "pageSize": 200,
                "warmupsPerMode": 3,
                "measuredPerMode": measured_samples,
                "fixture": if sparse { "remote metadata-only canonical listings; releaseDate, BPM, key, plays, and views are absent" } else { "remote metadata-only canonical listings; all sort metadata values are known" },
                "legacyFirstPage": summarize(legacy),
                "firstPageWithGlobalAvailability": summarize(global),
                "contention": contention,
            }));
            drop(store);
            drop(directory);
        }
        println!(
            "{}",
            serde_json::json!({
                "benchmark": "first_page_global_sort_availability",
                "scope": "actual Store storage microbenchmark; excludes GUI and end-to-end application work",
                "guiIncluded": false,
                "endToEnd": false,
                "cpuTimeReported": false,
                "contentionEnabled": contention_enabled,
                "sparseFixture": sparse,
                "fixtures": output,
            })
        );
    }

    #[test]
    #[ignore = "BPM index comparison benchmark; exploratory 3-sample and qualification 20-sample runs require explicit coordination"]
    fn benchmark_bpm_projection_index_pages_and_maintenance() {
        use std::time::Instant;

        fn summarize(samples: Vec<f64>, warmups: usize) -> serde_json::Value {
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            let p95_index = ((sorted.len() as f64 * 0.95).ceil() as usize)
                .saturating_sub(1)
                .min(sorted.len().saturating_sub(1));
            let middle = sorted.len() / 2;
            let median = if sorted.len() % 2 == 0 {
                (sorted[middle - 1] + sorted[middle]) / 2.0
            } else {
                sorted[middle]
            };
            serde_json::json!({
                "warmups": warmups,
                "measured": samples.len(),
                "metric": "wall_elapsed_ms",
                "medianMs": median,
                "p95Ms": sorted[p95_index],
                "worstMs": sorted.last().copied().unwrap_or_default(),
                "samplesMs": samples,
            })
        }

        fn toggle_bpm_indexes(store: &Store, enabled: bool) -> f64 {
            let started = Instant::now();
            let connection = store.connection.lock().unwrap();
            let revision = crate::track_sort_sql::AVAILABILITY_MASK_REVISION;
            let asc_name = format!("catalog_listings_sort_bpm_asc_tiedesc_r{revision}");
            let desc_name = format!("catalog_listings_sort_bpm_desc_tiedesc_r{revision}");
            connection
                .execute_batch(&format!(
                    "DROP INDEX IF EXISTS \"{asc_name}\"; DROP INDEX IF EXISTS \"{desc_name}\";"
                ))
                .unwrap();
            if enabled {
                let sort_expression =
                    crate::track_sort_sql::expression("bpm", "sort_metadata_projection").unwrap();
                let null_expression =
                    format!("CASE WHEN ({sort_expression}) IS NULL THEN 1 ELSE 0 END");
                connection
                    .execute_batch(&format!(
                        "CREATE INDEX \"{asc_name}\"
                            ON catalog_listings(({null_expression}) DESC,({sort_expression}) DESC)
                            WHERE in_library=1 AND sort_projection_revision={revision} AND sort_metadata_projection IS NOT NULL;
                         CREATE INDEX \"{desc_name}\"
                            ON catalog_listings(({null_expression}) DESC,({sort_expression}) ASC)
                            WHERE in_library=1 AND sort_projection_revision={revision} AND sort_metadata_projection IS NOT NULL;"
                    ))
                    .unwrap();
            }
            started.elapsed().as_secs_f64() * 1000.0
        }

        fn reset_projection(store: &Store) -> f64 {
            let started = Instant::now();
            store
                .connection
                .lock()
                .unwrap()
                .execute(
                    "UPDATE catalog_listings SET sort_availability_mask=NULL,sort_availability_revision=NULL,sort_metadata_projection=NULL,sort_projection_revision=NULL",
                    [],
                )
                .unwrap();
            started.elapsed().as_secs_f64() * 1000.0
        }

        fn run_page(
            store: &Store,
            record_count: usize,
            offset: usize,
            direction: &str,
            sparse: bool,
            assets: bool,
            expect_pending: bool,
            index_mode: &str,
        ) -> (f64, Vec<(Option<f64>, String)>) {
            let started = Instant::now();
            let (page, total, availability, sort_pending) = store
                .list_tracks_page_with_sort_state(
                    "library",
                    None,
                    200,
                    offset,
                    None,
                    Some("bpm"),
                    Some(direction),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            if assets && expect_pending {
                assert!(sort_pending, "asset-backed BPM sort reports sortPending");
                assert_eq!(total, 0);
                assert!(page.is_empty());
            } else {
                assert!(!sort_pending);
                assert_eq!(total, record_count);
                assert_eq!(page.len(), record_count.saturating_sub(offset).min(200));
            }
            let availability_status = if offset == 0 {
                let state = availability.expect("first page reports availability");
                match (expect_pending, state) {
                    (true, SortAvailabilityState::Pending) => "pending",
                    (false, SortAvailabilityState::Ready(fields)) => {
                        for (field, expected) in [
                            ("sourceOrder", true),
                            ("title", true),
                            ("artist", true),
                            ("album", true),
                            ("duration", true),
                            ("releaseDate", !sparse),
                            ("dateAdded", true),
                            ("bpm", assets || !sparse),
                            ("key", assets || !sparse),
                            ("plays", !sparse),
                            ("views", !sparse),
                        ] {
                            assert_eq!(fields.get(field), Some(&expected), "{field}");
                        }
                        "ready"
                    }
                    other => panic!("unexpected first-page availability: {other:?}"),
                }
            } else {
                assert!(availability.is_none(), "later pages omit availability");
                "not_returned_on_later_pages"
            };
            let ordered = page
                .iter()
                .map(|track| (track.bpm, track.id.clone()))
                .collect::<Vec<_>>();
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "BURRATA_BPM_INDEX_PAGE {}",
                serde_json::json!({
                    "recordCount":record_count,
                    "sortField":"bpm",
                    "direction":direction,
                    "offset":offset,
                    "pageSize":200,
                    "availabilityPayload":offset==0,
                    "indexMode":index_mode,
                    "sortPending":sort_pending,
                    "availabilityState":availability_status,
                    "assetFixture":assets,
                    "elapsedMs":elapsed
                })
            );
            (elapsed, ordered)
        }

        fn run_page_pair(
            store: &Store,
            record_count: usize,
            direction: &str,
            sparse: bool,
            assets: bool,
            expect_pending: bool,
            index_mode: &str,
        ) -> (
            f64,
            f64,
            Vec<(Option<f64>, String)>,
            Vec<(Option<f64>, String)>,
        ) {
            let (first_ms, first) = run_page(
                store,
                record_count,
                0,
                direction,
                sparse,
                assets,
                expect_pending,
                index_mode,
            );
            let (later_ms, later) = run_page(
                store,
                record_count,
                200,
                direction,
                sparse,
                assets,
                expect_pending,
                index_mode,
            );
            if !(assets && expect_pending) {
                let all = first.iter().chain(&later).collect::<Vec<_>>();
                assert!(
                    all.windows(2).all(|pair| {
                        let (left_bpm, left_id) = pair[0];
                        let (right_bpm, right_id) = pair[1];
                        let value_order = match (left_bpm, right_bpm) {
                            (Some(left), Some(right)) if left < right && direction == "asc" => true,
                            (Some(left), Some(right)) if left > right && direction == "desc" => {
                                true
                            }
                            (Some(left), Some(right)) if left == right => left_id > right_id,
                            (Some(_), Some(_)) => false,
                            (Some(_), None) => true,
                            (None, Some(_)) => false,
                            (None, None) => left_id > right_id,
                        };
                        value_order
                    }),
                    "page boundary violates BPM null/order/tie semantics"
                );
            }
            (first_ms, later_ms, first, later)
        }

        fn run_backfill(store: &Store) -> (f64, usize, f64) {
            let started = Instant::now();
            let mut batches = 0;
            let mut max_batch_ms: f64 = 0.0;
            loop {
                let batch_started = Instant::now();
                let updated = store.backfill_sort_availability_batch(500).unwrap();
                max_batch_ms = max_batch_ms.max(batch_started.elapsed().as_secs_f64() * 1000.0);
                if updated == 0 {
                    break;
                }
                batches += 1;
            }
            (
                started.elapsed().as_secs_f64() * 1000.0,
                batches,
                max_batch_ms,
            )
        }

        let assets = std::env::var("BURRATA_AVAILABILITY_BENCH_ASSETS")
            .ok()
            .as_deref()
            == Some("1");
        let tied_bpm = std::env::var("BURRATA_AVAILABILITY_BENCH_TIED")
            .ok()
            .as_deref()
            == Some("1");
        let sparse = assets
            || std::env::var("BURRATA_AVAILABILITY_BENCH_SPARSE")
                .ok()
                .as_deref()
                == Some("1");
        let measured_samples = std::env::var("BURRATA_AVAILABILITY_BENCH_SAMPLES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(3, 20))
            .unwrap_or(20);
        let measure_maintenance = std::env::var("BURRATA_AVAILABILITY_BENCH_INDEX_MAINTENANCE")
            .ok()
            .as_deref()
            == Some("1");
        let selected_counts = std::env::var("BURRATA_AVAILABILITY_BENCH_COUNTS").ok();
        let mut fixtures = Vec::new();
        for record_count in [1_000usize, 5_000, 20_000, 50_000]
            .into_iter()
            .filter(|count| {
                selected_counts.as_ref().is_none_or(|selected| {
                    selected
                        .split(',')
                        .any(|value| value.trim().parse::<usize>().ok() == Some(*count))
                })
            })
        {
            let fixture_started = Instant::now();
            let (directory, store) = db();
            {
                let mut connection = store.connection.lock().unwrap();
                let transaction = connection.transaction().unwrap();
                let mut work = transaction
                    .prepare("INSERT INTO works(id,title,artist) VALUES(?1,'BPM benchmark work','BPM benchmark artist')")
                    .unwrap();
                let mut recording = transaction
                    .prepare("INSERT INTO recordings(id,work_id,version) VALUES(?1,?2,NULL)")
                    .unwrap();
                let mut listing = transaction
                    .prepare("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?1,'benchmark',?2,?1,'',?3,'2026-01-01T00:00:00Z')")
                    .unwrap();
                let mut asset_insert = if assets {
                    Some(transaction.prepare("INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?1,?2,?3,?4,?5)").unwrap())
                } else {
                    None
                };
                for index in 0..record_count {
                    let id = format!("benchmark:{index:05}");
                    let mut provenance = serde_json::json!({"dateAdded":"2025-01-02"});
                    if !sparse {
                        provenance["plays"] = serde_json::json!(0);
                        provenance["views"] = serde_json::json!("1.2k");
                    }
                    let track = Track {
                        id: id.clone(),
                        provider: "benchmark".into(),
                        provider_id: format!("{index:05}"),
                        artist: "BPM benchmark artist".into(),
                        title: format!("BPM benchmark track {index:05}"),
                        album: Some("BPM benchmark album".into()),
                        duration_seconds: Some(180.0),
                        bpm: (!sparse).then_some(if tied_bpm {
                            128.0
                        } else {
                            80.0 + (index % 120) as f64
                        }),
                        key: (!sparse).then(|| "8A".into()),
                        release_date: (!sparse).then(|| "2024-01-02".into()),
                        provenance,
                        ..Track::default()
                    };
                    let metadata = encode(&track).unwrap();
                    let work_id = format!("work:{id}");
                    work.execute([&work_id]).unwrap();
                    recording.execute(rusqlite::params![id, work_id]).unwrap();
                    listing
                        .execute(rusqlite::params![id, format!("{index:05}"), metadata])
                        .unwrap();
                    if let Some(asset_insert) = asset_insert.as_mut() {
                        let asset_id = format!("benchmark-asset:{index:05}");
                        let asset_path = directory.path().join(format!("asset-{index:05}.bin"));
                        std::fs::write(&asset_path, []).unwrap();
                        let mut asset_track = track.clone();
                        asset_track.bpm = Some(if tied_bpm {
                            128.0
                        } else {
                            80.0 + (index % 120) as f64
                        });
                        asset_track.key = Some(if index % 2 == 0 { "8A" } else { "9B" }.into());
                        let asset = LocalAsset {
                            id: asset_id.clone(),
                            recording_id: id.clone(),
                            path: asset_path.to_string_lossy().into_owned(),
                            hash: format!("fixture-hash-{index:05}"),
                            source_codec: "fixture".into(),
                            final_codec: "fixture".into(),
                            source_bitrate: None,
                            final_bitrate: None,
                            sample_rate: None,
                            channels: None,
                            bit_depth: None,
                            duration_seconds: None,
                            size_bytes: Some(0),
                            bitrate_origin: None,
                            processing: "complete".into(),
                            track: asset_track,
                        };
                        asset_insert
                            .execute(rusqlite::params![
                                asset_id,
                                id,
                                asset_path.to_string_lossy(),
                                asset.hash,
                                encode(&asset).unwrap()
                            ])
                            .unwrap();
                    }
                }
                drop(asset_insert);
                drop(listing);
                drop(recording);
                drop(work);
                transaction.commit().unwrap();
            }
            let fixture_setup_ms = fixture_started.elapsed().as_secs_f64() * 1000.0;

            // Exercise both incomplete-state contracts before any hidden backfill:
            // source-order data can remain visible, while incomplete asset-backed
            // BPM sorts must return pending and no rows on either page.
            let mut pending_first = Vec::new();
            let mut pending_later = Vec::new();
            for _ in 0..3 {
                let (first, later, _, _) = run_page_pair(
                    &store,
                    record_count,
                    "asc",
                    sparse,
                    assets,
                    true,
                    "indexed-pending",
                );
                pending_first.push(first);
                pending_later.push(later);
            }
            let (initial_backfill_ms, initial_backfill_batches, max_backfill_batch_ms) =
                run_backfill(&store);
            // Include a distinct, populated-index build cost. The normal Store
            // open creates these indexes while projections are empty, which does
            // not represent the cost of indexing a migrated/ready large library.
            let populated_index_build_ms = toggle_bpm_indexes(&store, true);

            let mut indexed_first_asc = Vec::new();
            let mut indexed_later_asc = Vec::new();
            let mut no_index_first_asc = Vec::new();
            let mut no_index_later_asc = Vec::new();
            let mut indexed_first_desc = Vec::new();
            let mut indexed_later_desc = Vec::new();
            let mut no_index_first_desc = Vec::new();
            let mut no_index_later_desc = Vec::new();
            let mut populated_index_rebuild_ms = Vec::new();
            let mut read_warmups = 3;
            for warmup in 0..3 {
                for direction in ["asc", "desc"] {
                    let (indexed_first, indexed_later, _, _) = run_page_pair(
                        &store,
                        record_count,
                        direction,
                        sparse,
                        assets,
                        false,
                        "indexed",
                    );
                    let _ = (indexed_first, indexed_later);
                    toggle_bpm_indexes(&store, false);
                    let (no_index_first, no_index_later, _, _) = run_page_pair(
                        &store,
                        record_count,
                        direction,
                        sparse,
                        assets,
                        false,
                        "no-bpm-index",
                    );
                    let _ = (no_index_first, no_index_later);
                    populated_index_rebuild_ms.push(toggle_bpm_indexes(&store, true));
                }
                read_warmups = warmup + 1;
            }
            populated_index_rebuild_ms.clear();

            for sample in 0..measured_samples {
                for direction in ["asc", "desc"] {
                    if sample % 2 == 0 {
                        let (first, later, first_rows, later_rows) = run_page_pair(
                            &store,
                            record_count,
                            direction,
                            sparse,
                            assets,
                            false,
                            "indexed",
                        );
                        toggle_bpm_indexes(&store, false);
                        let (plain_first, plain_later, plain_first_rows, plain_later_rows) =
                            run_page_pair(
                                &store,
                                record_count,
                                direction,
                                sparse,
                                assets,
                                false,
                                "no-bpm-index",
                            );
                        assert_eq!(
                            first_rows, plain_first_rows,
                            "index changed first-page BPM results ({direction})"
                        );
                        assert_eq!(
                            later_rows, plain_later_rows,
                            "index changed later-page BPM results ({direction})"
                        );
                        populated_index_rebuild_ms.push(toggle_bpm_indexes(&store, true));
                        match direction {
                            "asc" => {
                                indexed_first_asc.push(first);
                                indexed_later_asc.push(later);
                                no_index_first_asc.push(plain_first);
                                no_index_later_asc.push(plain_later);
                            }
                            _ => {
                                indexed_first_desc.push(first);
                                indexed_later_desc.push(later);
                                no_index_first_desc.push(plain_first);
                                no_index_later_desc.push(plain_later);
                            }
                        }
                    } else {
                        toggle_bpm_indexes(&store, false);
                        let (plain_first, plain_later, plain_first_rows, plain_later_rows) =
                            run_page_pair(
                                &store,
                                record_count,
                                direction,
                                sparse,
                                assets,
                                false,
                                "no-bpm-index",
                            );
                        populated_index_rebuild_ms.push(toggle_bpm_indexes(&store, true));
                        let (first, later, first_rows, later_rows) = run_page_pair(
                            &store,
                            record_count,
                            direction,
                            sparse,
                            assets,
                            false,
                            "indexed",
                        );
                        assert_eq!(
                            first_rows, plain_first_rows,
                            "index changed first-page BPM results ({direction})"
                        );
                        assert_eq!(
                            later_rows, plain_later_rows,
                            "index changed later-page BPM results ({direction})"
                        );
                        match direction {
                            "asc" => {
                                indexed_first_asc.push(first);
                                indexed_later_asc.push(later);
                                no_index_first_asc.push(plain_first);
                                no_index_later_asc.push(plain_later);
                            }
                            _ => {
                                indexed_first_desc.push(first);
                                indexed_later_desc.push(later);
                                no_index_first_desc.push(plain_first);
                                no_index_later_desc.push(plain_later);
                            }
                        }
                    }
                }
            }

            let mut maintenance = serde_json::Value::Null;
            if measure_maintenance {
                let mut indexed_reset = Vec::new();
                let mut indexed_fill = Vec::new();
                let mut no_index_reset = Vec::new();
                let mut no_index_fill = Vec::new();
                let mut rebuild = Vec::new();
                for sample in 0..measured_samples {
                    if sample % 2 == 0 {
                        indexed_reset.push(reset_projection(&store));
                        indexed_fill.push(run_backfill(&store).0);
                        toggle_bpm_indexes(&store, false);
                        no_index_reset.push(reset_projection(&store));
                        no_index_fill.push(run_backfill(&store).0);
                        rebuild.push(toggle_bpm_indexes(&store, true));
                    } else {
                        toggle_bpm_indexes(&store, false);
                        no_index_reset.push(reset_projection(&store));
                        no_index_fill.push(run_backfill(&store).0);
                        rebuild.push(toggle_bpm_indexes(&store, true));
                        indexed_reset.push(reset_projection(&store));
                        indexed_fill.push(run_backfill(&store).0);
                    }
                }
                maintenance = serde_json::json!({
                    "warmupsPerMode":0,
                    "measuredPairs":measured_samples,
                    "indexEnabledReset":summarize(indexed_reset,0),
                    "indexEnabledBackfill":summarize(indexed_fill,0),
                    "indexDisabledReset":summarize(no_index_reset,0),
                    "indexDisabledBackfill":summarize(no_index_fill,0),
                    "rebuildAfterBackfill":summarize(rebuild,0),
                    "note":"Reset and backfill are reported separately; compare their sum for the complete projection update cycle. Index maintenance is exercised on both removal and insertion of projected rows."
                });
            }

            fixtures.push(serde_json::json!({
                "recordCount":record_count,
                "fixtureSetupWallMs":fixture_setup_ms,
                "view":"library",
                "sortField":"bpm",
                "assetFixtureEnabled":assets,
                "fixture":if assets {"sparse remote metadata plus one distinct zero-byte local asset per listing; asset payload supplies BPM/key, no audio decode"} else if sparse {"remote-only listings with missing BPM/key/release/count fields"} else {"remote-only listings with all BPM/key/release/count fields known"},
                "bpmDistribution":if tied_bpm {"constant 128 BPM; tie-heavy"} else {"varied 80-199 BPM, repeated modulo 120"},
                "pageSize":200,
                "beforeBackfillFirstPage":summarize(pending_first,3),
                "beforeBackfillLaterPage":summarize(pending_later,3),
                "beforeBackfillContract":if assets { "sortPending=true, availability=Pending, and no BPM rows are returned until projections are ready" } else { "sortPending=false, availability=Pending on page one, and the raw-metadata BPM sort still returns rows" },
                "initialBackfillWallMs":initial_backfill_ms,
                "initialBackfillBatchCount":initial_backfill_batches,
                "initialBackfillMaxBatchMs":max_backfill_batch_ms,
                "populatedIndexBuildMs":populated_index_build_ms,
                "readWarmupsPerMode":read_warmups,
                "measuredPerMode":measured_samples,
                "exploratoryRun":measured_samples==3,
                "indexEnabled":{
                    "ascFirstPage":summarize(indexed_first_asc,read_warmups),
                    "ascLaterPage":summarize(indexed_later_asc,read_warmups),
                    "descFirstPage":summarize(indexed_first_desc,read_warmups),
                    "descLaterPage":summarize(indexed_later_desc,read_warmups)
                },
                "indexDisabledMatchedControl":{
                    "control":"same SQLite database/rows/projections; only the two candidate revision-named BPM indexes are dropped",
                    "order":"index-enabled and index-disabled observations alternate across samples",
                    "ascFirstPage":summarize(no_index_first_asc,read_warmups),
                    "ascLaterPage":summarize(no_index_later_asc,read_warmups),
                    "descFirstPage":summarize(no_index_first_desc,read_warmups),
                    "descLaterPage":summarize(no_index_later_desc,read_warmups)
                },
                "indexMaintenance":maintenance
            }));
            drop(store);
            drop(directory);
        }
        println!(
            "{}",
            serde_json::json!({
                "benchmark":"bpm_projection_index_first_and_later_pages",
                "scope":"actual Store storage microbenchmark; excludes GUI and end-to-end application work",
                "guiIncluded":false,
                "endToEnd":false,
                "cpuTimeReported":false,
                "maintenanceEnabled":measure_maintenance,
                "samplePolicy":"BURRATA_AVAILABILITY_BENCH_SAMPLES=3 is exploratory; 20 measured samples is the qualification sample count after correctness and resource checks",
                "fixtures":fixtures
            })
        );
    }

    #[test]
    fn paged_text_filter_matches_version_and_genre_before_limit() {
        let (_dir, store) = db();
        let mut version_match = track("version", "Extended Mix");
        version_match.title = "Track".into();
        store.save_track(&version_match).unwrap();
        let mut genre_match = track("genre", "");
        genre_match.title = "Track".into();
        genre_match.genre = Some("Ambient".into());
        store.save_track(&genre_match).unwrap();
        // Newer nonmatching rows sort ahead of both matches in the legacy order.
        for id in ["newer-1", "newer-2"] {
            let mut decoy = track(id, "");
            decoy.title = "Other".into();
            store.save_track(&decoy).unwrap();
        }

        for (query, expected_id) in [
            ("extended", "fixture:version"),
            ("ambient", "fixture:genre"),
        ] {
            let (page, total) = store
                .list_tracks_page(
                    "library",
                    None,
                    1,
                    0,
                    Some(query),
                    Some("sourceOrder"),
                    Some("asc"),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(total, 1, "query {query}");
            assert_eq!(page.len(), 1, "query {query}");
            assert_eq!(page[0].id, expected_id, "query {query}");
        }
    }

    #[test]
    fn unicode_query_filters_database_before_first_page() {
        let (_dir, store) = db();
        let mut match_track = track("greek", "");
        match_track.title = "ΑΘΉΝΑ Nights".into();
        store.save_track(&match_track).unwrap();
        // These later rows lead the legacy newest-first page. The matching row is
        // outside that unfiltered page and must still be found by SQL filtering.
        for id in ["decoy-1", "decoy-2"] {
            let mut decoy = track(id, "");
            decoy.title = "Other".into();
            store.save_track(&decoy).unwrap();
        }
        let (unfiltered, _) = store
            .list_tracks_page(
                "library",
                None,
                1,
                0,
                None,
                Some("sourceOrder"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_ne!(unfiltered[0].id, "fixture:greek");

        let (page, total) = store
            .list_tracks_page(
                "library",
                None,
                1,
                0,
                Some("αθή"),
                Some("sourceOrder"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "fixture:greek");
    }

    #[test]
    fn duration_sort_keeps_whole_second_ties_in_source_order_for_both_directions() {
        let (_dir, store) = db();
        for (id, duration) in [("earlier", 30.1), ("later", 30.9)] {
            let mut value = track(id, "");
            value.duration_seconds = Some(duration);
            store.save_track(&value).unwrap();
        }
        let (source, _) = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                None,
                Some("sourceOrder"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let expected = source
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>();
        for direction in ["asc", "desc"] {
            let (sorted, _) = store
                .list_tracks_page(
                    "library",
                    None,
                    10,
                    0,
                    None,
                    Some("duration"),
                    Some(direction),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(
                sorted
                    .iter()
                    .map(|track| track.id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn paged_track_reads_filter_and_sort_before_offset_and_batch_keeps_requested_order() {
        let (_dir, store) = db();
        for (id, title) in [("one", "Beta"), ("two", "Alpha"), ("three", "Gamma")] {
            let mut t = track(id, "");
            t.title = title.into();
            t.duration_seconds = Some(60.0);
            store.save_track(&t).unwrap();
        }
        let (page, total) = store
            .list_tracks_page(
                "library",
                None,
                1,
                0,
                Some("a"),
                Some("title"),
                Some("asc"),
                Some(60),
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(page[0].title, "Alpha");
        let legacy = store.list_tracks("library", None).unwrap();
        let (ascending, _) = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                None,
                Some("sourceOrder"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            ascending.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            legacy.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            "sourceOrder ascending preserves the legacy library order"
        );
        let (reverse, _) = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                None,
                Some("sourceOrder"),
                Some("desc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            reverse.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Beta", "Alpha", "Gamma"]
        );
        let mut edited = store.get_track("fixture:one").unwrap().unwrap();
        edited.title = "Omega Manual".into();
        store.update_track(&edited).unwrap();
        let (manual_page, manual_total) = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                Some("omega"),
                Some("title"),
                Some("asc"),
                Some(60),
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(manual_total, 1);
        assert_eq!(manual_page[0].title, "Omega Manual");
        let batch = store
            .get_tracks(&[
                "fixture:three".into(),
                "fixture:one".into(),
                "fixture:three".into(),
            ])
            .unwrap();
        assert_eq!(
            batch.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Gamma", "Omega Manual", "Gamma"]
        );
    }

    #[test]
    fn paged_filters_and_sort_use_hydrated_autotag_values_before_limit() {
        let (dir, store) = db();
        for (id, title) in [("local", "Zulu"), ("plain", "Alpha")] {
            let mut t = track(id, "");
            t.title = title.into();
            store.save_track(&t).unwrap();
        }
        let path = dir.path().join("track.flac");
        std::fs::write(&path, b"audio").unwrap();
        let mut asset_track = store.get_track("fixture:local").unwrap().unwrap();
        asset_track.title = "Aardvark".into();
        asset_track.provenance["autotag"] = serde_json::json!({"source":"embedded"});
        store
            .save_asset(&asset("asset-local", &asset_track, &path, "hash-local"))
            .unwrap();

        let (page, total) = store
            .list_tracks_page(
                "library",
                None,
                1,
                0,
                None,
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(page[0].id, "fixture:local");
        assert_eq!(page[0].title, "Aardvark");
        let (filtered, total) = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                Some("aardvark"),
                Some("title"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(filtered[0].id, "fixture:local");
    }

    #[test]
    fn spotify_duration_backfill_obeys_manual_and_chapter_guards() {
        let (_dir, store) = db();
        let make = |provider_id: &str| Track {
            id: format!("spotify:{provider_id}"),
            provider: "spotify".into(),
            provider_id: provider_id.into(),
            duration_seconds: None,
            ..Default::default()
        };
        let good = "1234567890123456789012";
        let manual = "2234567890123456789012";
        let chapter = "3234567890123456789012";
        let explicit_null = "4234567890123456789012";
        let mut t = make(good);
        t.provenance = serde_json::json!({"manualFields":{}});
        store.save_track(&t).unwrap();
        let mut t = make(manual);
        t.provenance = serde_json::json!({"manualFields":{"durationSeconds":true}});
        store.save_track(&t).unwrap();
        let mut t = make(chapter);
        t.playback_range = Some(PlaybackRange {
            parent_track_id: "p".into(),
            chapter_id: "c".into(),
            source_url: "".into(),
            start_time: 0.0,
            end_time: 20.0,
            parent_title: "p".into(),
            parent_duration_seconds: None,
        });
        store.save_track(&t).unwrap();
        store.save_track(&make(explicit_null)).unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
                params![
                    r#"{"durationSeconds":null}"#,
                    format!("spotify:{explicit_null}")
                ],
            )
            .unwrap();
        assert_eq!(
            store
                .fill_missing_spotify_durations(&[
                    (good.into(), 123.0),
                    (manual.into(), 100.0),
                    (chapter.into(), 80.0),
                    (explicit_null.into(), 70.0)
                ])
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{good}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(123.0)
        );
    }

    #[test]
    fn spotify_duration_scan_repairs_unknown_zero_and_invalid_values_but_not_manual_zero() {
        let (_dir, store) = db();
        let zero_id = "1234567890123456789012";
        let invalid_id = "2234567890123456789012";
        let manual_id = "3234567890123456789012";
        let long_id = "4234567890123456789012";
        let make = |id: &str, duration| Track {
            id: format!("spotify:{id}"),
            provider: "spotify".into(),
            provider_id: id.into(),
            duration_seconds: duration,
            ..Default::default()
        };
        store.save_track(&make(zero_id, Some(0.0))).unwrap();
        store.save_track(&make(invalid_id, None)).unwrap();
        store.save_track(&make(manual_id, Some(0.0))).unwrap();
        store.save_track(&make(long_id, None)).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute("UPDATE catalog_listings SET metadata=json_set(metadata,'$.durationSeconds','invalid') WHERE provider='spotify' AND provider_id=?1", [invalid_id]).unwrap();
            conn.execute("UPDATE catalog_listings SET metadata=json_set(metadata,'$.provenance',json_object('manualFields',json_object('durationSeconds',json('true')))) WHERE provider='spotify' AND provider_id=?1", [manual_id]).unwrap();
            conn.execute("UPDATE catalog_listings SET metadata=json_set(metadata,'$.durationSeconds',90000) WHERE provider='spotify' AND provider_id=?1", [long_id]).unwrap();
        }
        assert_eq!(
            store.missing_spotify_duration_ids(None, 10).unwrap(),
            vec![zero_id.to_owned(), invalid_id.to_owned()]
        );
        assert_eq!(
            store
                .fill_missing_spotify_durations(&[
                    (zero_id.into(), 184.25),
                    (invalid_id.into(), 201.5),
                    (manual_id.into(), 99.0),
                    (long_id.into(), 100.0)
                ])
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{zero_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(184.25)
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{invalid_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(201.5)
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{manual_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(0.0)
        );
        assert_eq!(
            store
                .get_track(&format!("spotify:{long_id}"))
                .unwrap()
                .unwrap()
                .duration_seconds,
            Some(90_000.0)
        );
    }

    #[test]
    fn existing_spotify_duration_scan_updates_unbound_listings_without_changing_occurrences() {
        let (_dir, store) = db();
        let missing_id = "1234567890123456789012";
        let manual_id = "2234567890123456789012";
        let chapter_id = "3234567890123456789012";
        let known_id = "4234567890123456789012";
        let spotify = |provider_id: &str| Track {
            id: format!("spotify:{provider_id}"),
            provider: "spotify".into(),
            provider_id: provider_id.into(),
            duration_seconds: None,
            ..Default::default()
        };
        let missing = spotify(missing_id);
        store.save_track(&missing).unwrap();
        let local_asset_path = _dir.path().join("local-duration-fixture.flac");
        std::fs::write(&local_asset_path, b"local audio bytes").unwrap();
        let local_asset = asset(
            "local-duration-fixture",
            &missing,
            &local_asset_path,
            "local-duration-hash",
        );
        store.save_asset(&local_asset).unwrap();
        let asset_payload_before: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM local_assets WHERE id=?1",
                [&local_asset.id],
                |row| row.get(0),
            )
            .unwrap();
        let manual = spotify(manual_id);
        store.save_track(&manual).unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
                params![r#"{"durationSeconds":180.0}"#, manual.id.as_str()],
            )
            .unwrap();
        let mut chapter = spotify(chapter_id);
        chapter.playback_range = Some(PlaybackRange {
            parent_track_id: "spotify:parent".into(),
            chapter_id: "chapter-1".into(),
            source_url: String::new(),
            start_time: 0.0,
            end_time: 20.0,
            parent_title: "Parent".into(),
            parent_duration_seconds: None,
        });
        store.save_track(&chapter).unwrap();
        let mut known = spotify(known_id);
        known.duration_seconds = Some(99.0);
        store.save_track(&known).unwrap();

        let collection = store.create_collection("Repeated imported set").unwrap();
        store
            .replace_playlist_entries(
                &collection.id,
                &[missing.clone(), missing.clone(), manual.clone()],
            )
            .unwrap();
        let before = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(
            before
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            vec![missing.id.as_str(), missing.id.as_str(), manual.id.as_str()]
        );

        assert_eq!(
            store.missing_spotify_duration_ids(None, 30).unwrap(),
            vec![missing_id.to_owned()]
        );
        assert_eq!(
            store
                .fill_missing_spotify_durations(&[(missing_id.into(), 245.48)])
                .unwrap(),
            1
        );
        let after = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(
            after
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            vec![missing.id.as_str(), missing.id.as_str(), manual.id.as_str()]
        );
        assert_eq!(after[0].duration_seconds, Some(245.48));
        assert_eq!(after[1].duration_seconds, Some(245.48));
        assert_eq!(after[2].duration_seconds, Some(180.0));
        let asset_payload_after: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM local_assets WHERE id=?1",
                [&local_asset.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(asset_payload_after, asset_payload_before);
        assert_eq!(
            store
                .missing_spotify_duration_ids(Some(missing_id), 30)
                .unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn verified_playback_source_proof_migrates_legacy_rows_and_survives_reopen() {
        let (directory, store) = db();
        let source = track("proof-migration", "");
        store.save_track(&source).unwrap();
        let fingerprint = "c".repeat(64);
        let provider_id = "4-XSYttqtls";
        let url = format!("https://www.youtube.com/watch?v={provider_id}");
        store
            .save_verified_playback_source(&source.id, &fingerprint, "youtube", provider_id, &url)
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "ALTER TABLE verified_playback_sources DROP COLUMN verification_proof;
             PRAGMA user_version=9;",
            )
            .unwrap();
        drop(store);
        let path = directory.path().join("library.sqlite");
        let migrated = Store::open(&path).unwrap();
        let mapping = migrated
            .verified_playback_source(&source.id, &fingerprint, "youtube")
            .unwrap()
            .unwrap();
        assert_eq!(mapping.permanent_url, url);
        assert_eq!(mapping.verification_proof, None);
        let proof = serde_json::json!({"version": 1, "kind": "youtube_music_catalog", "catalogLocator": true});
        migrated
            .save_verified_playback_source_with_proof(
                &source.id,
                &fingerprint,
                "youtube",
                provider_id,
                &url,
                &proof,
            )
            .unwrap();
        drop(migrated);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened
                .verified_playback_source(&source.id, &fingerprint, "youtube")
                .unwrap()
                .unwrap()
                .verification_proof,
            Some(proof)
        );
        reopened
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE verified_playback_sources SET verification_proof='invalid legacy JSON'",
                [],
            )
            .unwrap();
        assert_eq!(
            reopened
                .verified_playback_source(&source.id, &fingerprint, "youtube")
                .unwrap()
                .unwrap()
                .verification_proof,
            None
        );
    }

    #[test]
    fn verified_playback_sources_are_identity_scoped_stable_and_expire() {
        let (_dir, store) = db();
        let mut track = track("mapping", "radio edit");
        track.title = "A Track".into();
        track.artist = "An Artist".into();
        store.save_track(&track).unwrap();
        let fingerprint = "a".repeat(64);
        let provider_id = "4-XSYttqtls";
        let url = format!("https://www.youtube.com/watch?v={provider_id}");
        store
            .save_verified_playback_source(&track.id, &fingerprint, "youtube", provider_id, &url)
            .unwrap();
        assert_eq!(
            store
                .verified_playback_source(&track.id, &fingerprint, "youtube")
                .unwrap(),
            Some(VerifiedPlaybackSource {
                provider: "youtube".into(),
                provider_id: provider_id.into(),
                permanent_url: url.clone(),
                verification_proof: None,
            })
        );
        assert!(store
            .verified_playback_source(&track.id, &"b".repeat(64), "youtube")
            .unwrap()
            .is_none());
        assert!(store
            .save_verified_playback_source(
                &track.id,
                &fingerprint,
                "youtube",
                provider_id,
                &format!("{url}&expire=999&signature=transient"),
            )
            .is_err());
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE verified_playback_sources SET expires_at='2000-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
        assert!(store
            .verified_playback_source(&track.id, &fingerprint, "youtube")
            .unwrap()
            .is_none());
        store
            .save_verified_playback_source(&track.id, &fingerprint, "youtube", provider_id, &url)
            .unwrap();
        store
            .invalidate_verified_playback_source(&track.id, &fingerprint, "youtube", provider_id)
            .unwrap();
        assert!(store
            .verified_playback_source(&track.id, &fingerprint, "youtube")
            .unwrap()
            .is_none());
    }

    #[test]
    fn paged_key_sort_normalizes_enharmonics_and_camelot() {
        let (_dir, store) = db();
        for (id, key) in [("flat", "D♭"), ("sharp", "C# major"), ("camelot", "8A")] {
            let mut t = track(id, "");
            t.key = Some(key.into());
            store.save_track(&t).unwrap();
        }
        let (tracks, total) = store
            .list_tracks_page(
                "library",
                None,
                10,
                0,
                None,
                Some("key"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(
            tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            // D-flat major and C-sharp major are equal keys. The stable tie
            // follows legacy library order (newest first), like stable JS sort.
            vec!["fixture:sharp", "fixture:flat", "fixture:camelot"]
        );
    }

    #[test]
    fn import_source_url_lookup_uses_index_and_skips_unrelated_library_rows() {
        let (_dir, store) = db();
        let shared_url = "https://youtu.be/ABCDEF12345";
        for id in ["match-one", "match-two"] {
            let imported = Track {
                id: format!("import:{id}"),
                provider: "import".into(),
                provider_id: id.into(),
                source_url: shared_url.into(),
                artist: "Unknown artist".into(),
                title: shared_url.into(),
                ..Track::default()
            };
            store.save_track(&imported).unwrap();
        }
        let other = Track {
            id: "import:other".into(),
            provider: "import".into(),
            provider_id: "other".into(),
            source_url: "https://youtu.be/OTHER123456".into(),
            ..Track::default()
        };
        store.save_track(&other).unwrap();

        // Model the smoke library: many unrelated metadata-only catalog rows and
        // very few exact imported URLs. Seed directly to avoid measuring save_track.
        {
            let mut connection = store.connection.lock().unwrap();
            let transaction = connection.transaction().unwrap();
            let unrelated: Track = Track {
                id: "fixture:unrelated".into(),
                provider: "fixture".into(),
                provider_id: "unrelated".into(),
                ..Track::default()
            };
            let metadata = encode(&unrelated).unwrap();
            {
                let mut work = transaction
                    .prepare("INSERT INTO works(id,title,artist) VALUES(?1,'Noise','Noise')")
                    .unwrap();
                let mut recording = transaction
                    .prepare("INSERT INTO recordings(id,work_id,version) VALUES(?1,?2,NULL)")
                    .unwrap();
                let mut listing = transaction
                    .prepare("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?1,'fixture',?2,?1,'',?3,'2000-01-01T00:00:00Z')")
                    .unwrap();
                for index in 0..20_000 {
                    let id = format!("fixture:unrelated-{index:05}");
                    work.execute([format!("work:{id}")]).unwrap();
                    recording
                        .execute(rusqlite::params![id, format!("work:{id}")])
                        .unwrap();
                    listing
                        .execute(rusqlite::params![
                            id,
                            format!("unrelated-{index:05}"),
                            metadata
                        ])
                        .unwrap();
                }
            }
            transaction.commit().unwrap();
        }

        let tracks = store
            .list_import_tracks_by_source_urls(&[
                shared_url.into(),
                "https://youtu.be/NO_MATCH12345".into(),
                shared_url.into(),
            ])
            .unwrap();
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<std::collections::HashSet<_>>(),
            ["import:match-one", "import:match-two"]
                .into_iter()
                .collect()
        );

        let connection = store.connection.lock().unwrap();
        let plan = connection
            .prepare("EXPLAIN QUERY PLAN SELECT l.metadata FROM catalog_listings l WHERE l.provider='import' AND l.id=l.recording_id AND l.source_url IN (?1) ORDER BY l.rowid DESC")
            .unwrap()
            .query_map([shared_url], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("catalog_import_source_url")),
            "expected indexed exact-source lookup, got {plan:?}"
        );
    }

    fn asset(id: &str, track: &Track, path: &Path, hash: &str) -> LocalAsset {
        LocalAsset {
            id: id.into(),
            recording_id: track.id.clone(),
            path: path.to_string_lossy().into_owned(),
            hash: hash.into(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: Some(48_000),
            channels: Some(2),
            bit_depth: Some(24),
            duration_seconds: Some(180.0),
            size_bytes: Some(1024),
            bitrate_origin: Some("stream".into()),
            processing: "managed copy".into(),
            track: track.clone(),
        }
    }

    #[test]
    fn asset_id_lookup_returns_same_hydrated_asset_without_listing_all_assets() {
        let (dir, store) = db();
        let track = track("asset-lookup-track", "");
        store.save_track(&track).unwrap();
        let path = dir.path().join("asset-lookup.flac");
        std::fs::write(&path, b"fixture audio").unwrap();
        let saved = asset("asset-lookup-id", &track, &path, "asset-lookup-hash");
        store.save_asset(&saved).unwrap();

        let listed = store.list_assets().unwrap().pop().unwrap();
        let looked_up = store.get_asset_by_id("asset-lookup-id").unwrap().unwrap();
        assert_eq!(looked_up.id, listed.id);
        assert_eq!(looked_up.recording_id, listed.recording_id);
        assert_eq!(looked_up.path, listed.path);
        assert_eq!(looked_up.hash, listed.hash);
        assert_eq!(looked_up.track.id, listed.track.id);
        assert_eq!(looked_up.track.asset_id, listed.track.asset_id);
        assert_eq!(looked_up.track.local_path, listed.track.local_path);
        assert!(store.get_asset_by_id("missing-asset-id").unwrap().is_none());
    }

    #[test]
    fn bounded_asset_queries_return_only_requested_recordings_paths_and_pages() {
        let (dir, store) = db();
        let first_track = track("bounded-assets-one", "");
        let second_track = track("bounded-assets-two", "");
        store.save_track(&first_track).unwrap();
        store.save_track(&second_track).unwrap();
        let first_path = dir.path().join("bounded-one.flac");
        let second_path = dir.path().join("bounded-two.flac");
        std::fs::write(&first_path, b"one").unwrap();
        std::fs::write(&second_path, b"two").unwrap();
        store
            .save_asset(&asset(
                "bounded-asset-one",
                &first_track,
                &first_path,
                "bounded-hash-one",
            ))
            .unwrap();
        store
            .save_asset(&asset(
                "bounded-asset-two",
                &second_track,
                &second_path,
                "bounded-hash-two",
            ))
            .unwrap();

        let files = store
            .list_track_files_for_recordings(&[first_track.id.clone()])
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].track_id, first_track.id);
        assert_eq!(
            files[0]
                .files
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            vec!["bounded-asset-one"]
        );
        let by_path = store
            .list_assets_for_paths(&[second_path.to_string_lossy().into_owned()])
            .unwrap();
        assert_eq!(
            by_path
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            vec!["bounded-asset-two"]
        );
        let (page, total) = store.list_asset_page(1, 0).unwrap();
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);
        let (artist_page, artist_total) = store.list_asset_artist_summaries_page(1, 1).unwrap();
        assert_eq!(artist_total, 2);
        assert_eq!(artist_page.len(), 1);
        assert_eq!(artist_page[0].artist.as_deref(), Some("Artist"));
        let resolved = store
            .list_assets_by_ids(&[artist_page[0].id.clone()])
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(store.count_assets().unwrap(), 2);
        store
            .save_analysis_record(
                "bounded-asset-two",
                "bounded-hash-two",
                "queue-v1",
                "completed",
                &serde_json::json!({}),
            )
            .unwrap();
        let candidates = store
            .list_analysis_queue_assets(None, false, "queue-v1")
            .unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>(),
            vec!["bounded-asset-one"]
        );
        assert_eq!(
            store
                .list_analysis_queue_assets(None, true, "queue-v1")
                .unwrap()
                .len(),
            2
        );
        assert!(store
            .list_track_files_for_recordings(&vec![String::new(); 201])
            .is_err());
        assert!(store
            .list_assets_for_paths(&vec![String::new(); 201])
            .is_err());
    }

    #[test]
    fn effective_artist_filter_normalizes_alias_names_and_paginates_after_filtering() {
        let (dir, store) = db();
        let mut edited = track("artist-filter-manual", "");
        edited.artist = "Raw catalog name".into();
        store.save_track(&edited).unwrap();
        let asset_path = dir.path().join("artist-filter-missing.flac");
        store
            .save_asset(&asset(
                "artist-filter-manual-asset",
                &edited,
                &asset_path,
                "artist-filter-hash",
            ))
            .unwrap();
        edited.artist = "  Ａlpha   Artist  ".into();
        store.update_track(&edited).unwrap();
        let mut alias = track("artist-filter-catalog-alias", "");
        alias.artist = "Alias Listing Name".into();
        store.save_track(&alias).unwrap();
        store
            .merge_recordings(&edited.id, &[alias.id.clone()])
            .unwrap();
        store
            .save_asset(&asset(
                "artist-filter-alias-asset",
                &alias,
                &dir.path().join("artist-filter-alias-missing.flac"),
                "artist-filter-alias-hash",
            ))
            .unwrap();

        // The raw catalog value is a candidate, but the manual edit is the exact
        // hydrated value. Profile aliases are supplied alongside the selected name.
        let names = vec!["Profile Artist Alias".into(), " alpha artist ".into()];
        let (matched, total) = store
            .list_asset_artist_matches_page(&names, 200, 0)
            .unwrap();
        assert_eq!(total, 2);
        assert!(matched
            .iter()
            .any(|item| item.id == "artist-filter-manual-asset"));
        assert!(matched
            .iter()
            .any(|item| item.id == "artist-filter-alias-asset"));
        assert!(matched
            .iter()
            .all(|item| item.artist.as_deref() == Some("Ａlpha   Artist")));
        // Preserve the legacy summary endpoint's saturating offset behavior.
        let (past_end, past_end_total) = store
            .list_asset_artist_matches_page(&names, 200, usize::MAX)
            .unwrap();
        assert!(past_end.is_empty());
        assert_eq!(past_end_total, total);
        let (raw_match, raw_total) = store
            .list_asset_artist_matches_page(&["raw catalog name".into()], 200, 0)
            .unwrap();
        assert_eq!(raw_total, 0);
        assert!(raw_match.is_empty());

        for index in 0..205 {
            let mut track = track(&format!("artist-filter-page-{index:03}"), "");
            track.artist = "Shared  Artist".into();
            store.save_track(&track).unwrap();
            store
                .save_asset(&asset(
                    &format!("artist-filter-page-asset-{index:03}"),
                    &track,
                    &dir.path().join(format!("missing-{index:03}.flac")),
                    &format!("artist-filter-page-hash-{index:03}"),
                ))
                .unwrap();
        }
        let names = [" shared artist ".into()];
        let (first, first_total) = store
            .list_asset_artist_matches_page(&names, 200, 0)
            .unwrap();
        let (second, second_total) = store
            .list_asset_artist_matches_page(&names, 200, 200)
            .unwrap();
        assert_eq!(first_total, 205);
        assert_eq!(second_total, 205);
        assert_eq!(first.len(), 200);
        assert_eq!(second.len(), 5);
        let first_ids = first
            .iter()
            .map(|item| item.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(second
            .iter()
            .all(|item| !first_ids.contains(item.id.as_str())));
    }

    #[test]
    fn artist_normalization_matches_javascript_whitespace_set() {
        assert_eq!(
            normalize_artist_name("\u{FEFF}Ａlpha\u{00A0}\u{3000}Artist\u{FEFF}"),
            "alpha artist"
        );
        assert_eq!(normalize_artist_name("a\u{0085}b"), "a\u{0085}b");
        assert_ne!(
            normalize_artist_name("a\u{0085}b"),
            normalize_artist_name("a b")
        );
    }

    #[test]
    fn bounded_track_file_lookup_preserves_catalog_alias_and_all_recording_files() {
        let (dir, store) = db();
        let canonical = track("alias-recording", "");
        store.save_track(&canonical).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute(
                "INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?1,'fixture','alias-provider-id',?2,'', '{}','2000-01-01T00:00:00Z')",
                rusqlite::params!["catalog-alias", canonical.id],
            ).unwrap();
        }
        for (id, name) in [
            ("alias-file-one", "alias-one.flac"),
            ("alias-file-two", "alias-two.flac"),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, id.as_bytes()).unwrap();
            store.save_asset(&asset(id, &canonical, &path, id)).unwrap();
        }

        let groups = store
            .list_track_files_for_recordings(&["catalog-alias".into()])
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].track_id, "catalog-alias");
        assert_eq!(
            groups[0]
                .files
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alias-file-two", "alias-file-one"]
        );
    }

    #[test]
    fn preferred_local_file_survives_hydration_and_restart() {
        let (dir, store) = db();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let mut settings = Settings::default();
        settings.music_directory = music.to_string_lossy().into_owned();
        store.save_settings(&settings).unwrap();
        let track = track("preferred-file", "");
        store.save_track(&track).unwrap();
        let first_path = music.join("first.flac");
        let preferred_path = music.join("preferred.flac");
        std::fs::write(&first_path, b"first audio").unwrap();
        std::fs::write(&preferred_path, b"preferred audio").unwrap();
        store
            .save_asset(&asset("asset-first", &track, &first_path, "first-hash"))
            .unwrap();
        store
            .save_asset(&asset(
                "asset-preferred",
                &track,
                &preferred_path,
                "preferred-hash",
            ))
            .unwrap();

        let selected = store
            .set_preferred_track_file(&track.id, "asset-preferred")
            .unwrap();
        assert_eq!(selected.asset_id.as_deref(), Some("asset-preferred"));
        assert_eq!(
            selected.local_path.as_deref(),
            Some(preferred_path.to_str().unwrap())
        );
        assert_eq!(store.list_track_files(&track.id).unwrap().len(), 2);
        assert!(store
            .set_preferred_track_file("fixture:other", "asset-preferred")
            .is_err());

        drop(store);
        let reopened = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let hydrated = reopened.get_track(&track.id).unwrap().unwrap();
        assert_eq!(hydrated.asset_id.as_deref(), Some("asset-preferred"));
    }

    #[test]
    fn replacement_checks_expected_hash_and_retains_previous_file_as_backup() {
        let (dir, store) = db();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let mut settings = Settings::default();
        settings.music_directory = music.to_string_lossy().into_owned();
        store.save_settings(&settings).unwrap();
        let track = track("replace-file", "");
        store.save_track(&track).unwrap();
        let old_path = music.join("track.mp3");
        let staged_path = music.join("new-download.flac");
        let new_path = music.join("track.flac");
        let occupied_path = new_path.clone();
        let old_bytes = b"old audio";
        let new_bytes = b"new validated audio";
        std::fs::write(&old_path, old_bytes).unwrap();
        std::fs::write(&staged_path, new_bytes).unwrap();
        let old_hash = crate::media::hash_file(&old_path).unwrap();
        let new_hash = crate::media::hash_file(&staged_path).unwrap();
        let mut old_asset = asset("target-asset", &track, &old_path, &old_hash);
        old_asset.final_codec = "mp3".into();
        store.save_asset(&old_asset).unwrap();
        let downloaded = asset("new-asset", &track, &staged_path, &new_hash);

        let stale = ReplacementDisposition::Replace {
            target_asset_id: "target-asset".into(),
            expected_hash: "stale-hash".into(),
        };
        assert!(store.commit_download_asset(&downloaded, &stale).is_err());
        assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
        assert!(staged_path.exists());

        std::fs::write(&occupied_path, b"unrelated flac").unwrap();
        let replace = ReplacementDisposition::Replace {
            target_asset_id: "target-asset".into(),
            expected_hash: old_hash.clone(),
        };
        assert!(store.commit_download_asset(&downloaded, &replace).is_err());
        assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
        assert_eq!(std::fs::read(&staged_path).unwrap(), new_bytes);
        assert_eq!(std::fs::read(&occupied_path).unwrap(), b"unrelated flac");
        std::fs::remove_file(&occupied_path).unwrap();

        let replace = ReplacementDisposition::Replace {
            target_asset_id: "target-asset".into(),
            expected_hash: old_hash,
        };
        let committed = store.commit_download_asset(&downloaded, &replace).unwrap();
        assert_eq!(committed.id, "target-asset");
        assert_eq!(committed.path, new_path.to_string_lossy().into_owned());
        assert_eq!(std::fs::read(&new_path).unwrap(), new_bytes);
        assert!(!old_path.exists());
        assert_eq!(
            store
                .get_asset_by_path(new_path.to_str().unwrap())
                .unwrap()
                .unwrap()
                .hash,
            new_hash
        );
        let backups = std::fs::read_dir(&music)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("burrata-backup")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), old_bytes);
        let backup_path = backups[0].path().to_string_lossy().into_owned();
        assert_eq!(
            committed.track.provenance["replacementBackupPath"].as_str(),
            Some(backup_path.as_str())
        );
    }

    #[test]
    fn indexed_library_search_includes_unicode_remixer_and_credit_metadata_with_pagination() {
        let (_dir, store) = db();
        let mut top_level = track("top-level", "");
        top_level.remixers = Some(vec!["Máté Remixer".into()]);
        let mut provenance_remixer = track("provenance-remixer", "");
        provenance_remixer.provenance = serde_json::json!({"remixers": ["Máté Remixer"]});
        let mut artist_credit = track("artist-credit", "");
        artist_credit.provenance = serde_json::json!({"artistCredits": ["Máté Remixer"]});
        for saved in [&top_level, &provenance_remixer, &artist_credit] {
            store.save_track(saved).unwrap();
        }

        let (ascii_matches, total) = store
            .search_library_tracks(&["Remixer".into()], 10, 0)
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(ascii_matches.len(), 3);

        let (first_page, total) = store.search_library_tracks(&["máté".into()], 2, 0).unwrap();
        assert_eq!(total, 3);
        assert_eq!(first_page.len(), 2);
        assert_eq!(first_page[0].id, "fixture:artist-credit");
        assert_eq!(first_page[1].id, "fixture:provenance-remixer");
        let (last_page, total) = store.search_library_tracks(&["máté".into()], 2, 2).unwrap();
        assert_eq!(total, 3);
        assert_eq!(last_page.len(), 1);
        assert_eq!(last_page[0].id, "fixture:top-level");
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
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
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
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
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
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
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
            10
        );
    }
    #[test]
    fn fresh_schema_has_saved_membership_default_and_v7_rows_migrate_as_saved() {
        let (directory, store) = db();
        {
            let connection = store.connection.lock().unwrap();
            let default_membership: String = connection
                .query_row(
                    "SELECT dflt_value FROM pragma_table_info('catalog_listings') WHERE name='in_library'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(default_membership, "1");
            connection
                .execute(
                    "INSERT INTO works(id,title,artist) VALUES('work:default-membership','Default','Default')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO recordings(id,work_id,version) VALUES('default-membership','work:default-membership',NULL)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES('default-membership','fixture','default-membership','default-membership','','{}','2026-01-01T00:00:00Z')",
                    [],
                )
                .unwrap();
            let membership: i64 = connection
                .query_row(
                    "SELECT in_library FROM catalog_listings WHERE id='default-membership'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(membership, 1);
            connection
                .execute(
                    "DELETE FROM catalog_listings WHERE id='default-membership'",
                    [],
                )
                .unwrap();
        }
        store.save_track(&track("legacy-v7-one", "")).unwrap();
        store.save_track(&track("legacy-v7-two", "")).unwrap();
        drop(store);

        let path = directory.path().join("library.sqlite");
        let legacy = Connection::open(&path).unwrap();
        // Reconstruct a v7 schema without any indexes introduced in v9.
        let indexes: Vec<String> = legacy
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='catalog_listings' AND sql LIKE '%in_library%'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for name in indexes {
            legacy
                .execute_batch(&format!("DROP INDEX \"{}\"", name.replace('"', "\"\"")))
                .unwrap();
        }
        legacy
            .execute_batch(
                "ALTER TABLE catalog_listings DROP COLUMN in_library; PRAGMA user_version=7;",
            )
            .unwrap();
        drop(legacy);

        let migrated = Store::open(&path).unwrap();
        assert_eq!(migrated.list_tracks("library", None).unwrap().len(), 2);
        let connection = migrated.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        let unsaved_default: i64 = connection
            .query_row(
                "SELECT in_library FROM catalog_listings WHERE id='fixture:legacy-v7-one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unsaved_default, 1);
    }

    #[test]
    fn promotion_sets_added_date_once_and_cache_refresh_preserves_provider_counts() {
        let (_dir, store) = db();
        let favorite = track("promoted-date", "");
        store.cache_track(&favorite).unwrap();
        let before: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&favorite.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&before)
            .unwrap()
            .get("dateAdded")
            .is_none());
        store.set_favorite(&favorite, true).unwrap();
        let after_first_promotion: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&favorite.id],
                |row| row.get(0),
            )
            .unwrap();
        let first_date = serde_json::from_str::<serde_json::Value>(&after_first_promotion).unwrap()
            ["dateAdded"]
            .as_str()
            .unwrap()
            .to_owned();
        store.cache_track(&favorite).unwrap();
        store.set_favorite(&favorite, true).unwrap();
        let after_refresh: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&favorite.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&after_refresh).unwrap()["dateAdded"],
            first_date
        );

        let mut counted = track("preserve-counts", "");
        counted.provenance = serde_json::json!({"plays": 640, "views": 18});
        store.cache_track(&counted).unwrap();
        let mut sparse_refresh = counted.clone();
        sparse_refresh.provenance = serde_json::json!({});
        store.cache_track(&sparse_refresh).unwrap();
        let hydrated = store.get_track(&counted.id).unwrap().unwrap();
        assert_eq!(hydrated.provenance["plays"], 640);
        assert_eq!(hydrated.provenance["views"], 18);

        let mut zero_refresh = counted;
        zero_refresh.provenance = serde_json::json!({"plays": 0});
        store.cache_track(&zero_refresh).unwrap();
        let hydrated = store.get_track(&zero_refresh.id).unwrap().unwrap();
        assert_eq!(hydrated.provenance["plays"], 0);
        assert_eq!(hydrated.provenance["views"], 18);
    }

    #[test]
    fn promotion_preserves_known_provider_added_date() {
        let (_dir, store) = db();
        let mut imported = track("promoted-provider-date", "");
        imported.provenance = serde_json::json!({"importedAt":"2021-05-06T12:30:00Z"});
        store.cache_track(&imported).unwrap();
        store.set_favorite(&imported, true).unwrap();
        let metadata: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT metadata FROM catalog_listings WHERE id=?1",
                [&imported.id],
                |row| row.get(0),
            )
            .unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert!(metadata.get("dateAdded").is_none());
        assert_eq!(metadata["provenance"]["importedAt"], "2021-05-06T12:30:00Z");
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
    fn retained_artist_evidence_survives_ordinary_refresh_and_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retained.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                "artist",
                &serde_json::json!({"name":"Old", "fetchedAt":0}),
            )
            .unwrap();
        store
            .set_provider_cache(
                "artists.profile.musicbrainz",
                "artist",
                &serde_json::json!({"name":"Refreshed", "fetchedAt":0}),
                60,
            )
            .unwrap();
        let expires:String=store.lock_connection().unwrap().query_row("SELECT expires_at FROM provider_cache WHERE namespace='artists.profile.musicbrainz' AND key='artist'",[],|row|row.get(0)).unwrap();
        assert_eq!(expires, "9999-12-31T23:59:59Z");
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened
                .get_provider_cache("artists.profile.musicbrainz", "artist")
                .unwrap()
                .unwrap()["name"],
            "Refreshed"
        );
        reopened
            .set_provider_cache(
                "artists.profile.musicbrainz",
                "artist",
                &serde_json::json!({}),
                0,
            )
            .unwrap();
        assert!(reopened
            .get_provider_cache("artists.profile.musicbrainz", "artist")
            .unwrap()
            .is_none());
        assert!(reopened
            .set_retained_provider_cache("unrelated", "key", &serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn retained_playback_locator_has_a_dedicated_namespace_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retained-locator.sqlite");
        let store = Store::open(&path).unwrap();
        let payload =
            serde_json::json!({"schemaVersion":2,"provider":"youtube","providerId":"abcdefghijk"});
        assert!(store
            .set_retained_provider_cache(PLAYBACK_LOCATOR_CACHE_NAMESPACE, "key", &payload)
            .is_err());
        assert!(store
            .set_retained_provider_cache("unrelated", "key", &payload)
            .is_err());
        assert!(store.set_retained_playback_locator("", &payload).is_err());
        store
            .set_retained_playback_locator("key", &payload)
            .unwrap();
        let expires: String = store
            .lock_connection()
            .unwrap()
            .query_row(
                "SELECT expires_at FROM provider_cache WHERE namespace=?1 AND key='key'",
                [PLAYBACK_LOCATOR_CACHE_NAMESPACE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expires, "9999-12-31T23:59:59Z");
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened
                .get_provider_cache(PLAYBACK_LOCATOR_CACHE_NAMESPACE, "key")
                .unwrap(),
            Some(payload)
        );
        reopened
            .set_provider_cache(
                PLAYBACK_LOCATOR_CACHE_NAMESPACE,
                "key",
                &serde_json::Value::Null,
                0,
            )
            .unwrap();
        assert!(reopened
            .get_provider_cache(PLAYBACK_LOCATOR_CACHE_NAMESPACE, "key")
            .unwrap()
            .is_none());
    }

    fn sample_matching_evidence(
        policy: crate::recording_match::MatchLevel,
    ) -> MatchingEvidenceRecord {
        MatchingEvidenceRecord {
            schema_version: 1,
            evaluator_version: "recording-match-v1".into(),
            requested_track_id: "spotify:track:requested".into(),
            requested_fingerprint: "a".repeat(64),
            source_provider: "youtube".into(),
            source_id: "abcdefghijk".into(),
            policy,
            relationship: crate::recording_match::MatchRelationship::Probable,
            evidence: serde_json::json!({"title":"Song","artist":"Artist"}),
        }
    }

    #[test]
    fn matching_evidence_isolated_by_policy_identity_and_evaluator() {
        let (_dir, store) = db();
        let record = sample_matching_evidence(crate::recording_match::MatchLevel::Flexible);
        store.save_matching_evidence(&record).unwrap();
        assert!(store
            .get_matching_evidence(
                &record.requested_track_id,
                &record.requested_fingerprint,
                &record.source_provider,
                &record.source_id,
                &record.evaluator_version,
                crate::recording_match::MatchLevel::Exact,
            )
            .unwrap()
            .is_none());
        assert!(store
            .get_matching_evidence(
                &record.requested_track_id,
                &"b".repeat(64),
                &record.source_provider,
                &record.source_id,
                &record.evaluator_version,
                record.policy,
            )
            .unwrap()
            .is_none());
        assert!(store
            .get_matching_evidence(
                &record.requested_track_id,
                &record.requested_fingerprint,
                &record.source_provider,
                &record.source_id,
                "recording-match-v2",
                record.policy,
            )
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .get_matching_evidence(
                    &record.requested_track_id,
                    &record.requested_fingerprint,
                    &record.source_provider,
                    &record.source_id,
                    &record.evaluator_version,
                    record.policy,
                )
                .unwrap(),
            Some(record)
        );
    }

    #[test]
    fn matching_evidence_rejects_legacy_schema_and_secret_urls() {
        let (_dir, store) = db();
        let mut record = sample_matching_evidence(crate::recording_match::MatchLevel::Balanced);
        record.schema_version = 0;
        assert!(store.save_matching_evidence(&record).is_err());
        record.schema_version = 1;
        record.evidence["streamUrl"] = serde_json::json!("https://example.test/audio");
        assert!(store.save_matching_evidence(&record).is_err());
    }

    #[test]
    fn cached_musicbrainz_profiles_match_exact_names_and_aliases_with_ambiguity_bound() {
        let (_dir, store) = db();
        for (id, name, aliases) in [
            (
                "one",
                "Canonical",
                serde_json::json!([{"name":"Stage Name"}]),
            ),
            ("two", "Stage Name", serde_json::json!([])),
            ("three", "Another", serde_json::json!(["Stage Name"])),
            ("four", "Fourth", serde_json::json!([{"name":"Stage Name"}])),
            ("unrelated", "Stage Name Deluxe", serde_json::json!([])),
        ] {
            store
                .set_retained_provider_cache(
                    "artists.profile.musicbrainz",
                    id,
                    &serde_json::json!({"id":id,"name":name,"aliases":aliases}),
                )
                .unwrap();
        }
        let matches = store
            .cached_musicbrainz_artist_profiles_for_name(" stage name ")
            .unwrap();
        assert_eq!(matches.len(), 3);
        assert!(matches.iter().all(|profile| {
            profile["name"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case("stage name")
                || profile["aliases"].as_array().is_some_and(|aliases| {
                    aliases.iter().any(|alias| {
                        alias
                            .as_str()
                            .or_else(|| alias["name"].as_str())
                            .is_some_and(|value| value.eq_ignore_ascii_case("stage name"))
                    })
                })
        }));
        assert!(store
            .cached_musicbrainz_artist_profiles_for_name("Stage Name Deluxe Extra")
            .unwrap()
            .is_empty());
        assert!(store
            .cached_musicbrainz_artist_profiles_for_name(&"x".repeat(201))
            .unwrap()
            .is_empty());
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
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
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
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_seconds: None,
                size_bytes: None,
                bitrate_origin: None,
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
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
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
                    sample_rate: None,
                    channels: None,
                    bit_depth: None,
                    duration_seconds: None,
                    size_bytes: None,
                    bitrate_origin: None,
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
    fn managed_publication_marker_respects_manual_provenance_replacement() {
        let mut asset_track = track("publication-marker", "");
        asset_track.provenance = serde_json::json!({"publicationJobId":"asset-job"});
        let has_marker = |manual: serde_json::Value| {
            managed_publication_marker_is_effective(&asset_track, &manual)
        };

        assert!(has_marker(serde_json::json!({})));
        assert!(has_marker(serde_json::json!(null)));
        assert!(has_marker(serde_json::json!({"title":"Manual title"})));
        assert!(!has_marker(serde_json::json!({"provenance":{}})));
        assert!(!has_marker(serde_json::json!({"provenance":null})));
        assert!(!has_marker(serde_json::json!({"provenance":"legacy"})));
        assert!(!has_marker(serde_json::json!({"provenance":[]})));
        assert!(!has_marker(
            serde_json::json!({"provenance":{"publicationJobId":""}})
        ));
        assert!(!has_marker(
            serde_json::json!({"provenance":{"publicationJobId":42}})
        ));
        assert!(managed_publication_marker_is_effective(
            &track("unmarked-publication", ""),
            &serde_json::json!({"provenance":{"publicationJobId":"manual-job"}}),
        ));
    }

    #[test]
    fn managed_download_backfill_streams_candidates_and_preserves_result_count() {
        let (directory, store) = db();
        let music = directory.path();
        let save_asset = |asset_id: &str, asset_track: &Track, path: &Path| {
            store
                .save_asset(&LocalAsset {
                    id: asset_id.into(),
                    recording_id: asset_track.id.clone(),
                    path: path.to_string_lossy().into_owned(),
                    hash: format!("{asset_id}-hash"),
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
                    processing: "managed".into(),
                    track: asset_track.clone(),
                })
                .unwrap();
        };

        let mut duplicate = track("managed-duplicate", "");
        duplicate.provenance = serde_json::json!({"publicationJobId":"duplicate-job"});
        let first_path = music.join("managed-first.flac");
        std::fs::write(&first_path, b"managed fixture").unwrap();
        save_asset("managed-first", &duplicate, &first_path);

        // More than one page of unmarked local files must not prevent finding
        // older candidates; cursoring is by stable descending asset rowid.
        for index in 0..500 {
            let local = track(&format!("unmarked-{index}"), "");
            save_asset(
                &format!("unmarked-asset-{index}"),
                &local,
                &music.join(format!("unmarked-{index}.flac")),
            );
        }

        let second_path = music.join("managed-second.flac");
        std::fs::write(&second_path, b"managed duplicate fixture").unwrap();
        save_asset("managed-second", &duplicate, &second_path);

        let manual = track("managed-manual", "");
        let manual_path = music.join("managed-manual.flac");
        std::fs::write(&manual_path, b"manual provenance fixture").unwrap();
        save_asset("managed-manual-asset", &manual, &manual_path);
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
                params![
                    r#"{"provenance":{"publicationJobId":"manual-job"}}"#,
                    manual.id
                ],
            )
            .unwrap();

        let suppressed = track("managed-suppressed", "");
        let mut suppressed_asset = suppressed.clone();
        suppressed_asset.provenance = serde_json::json!({"publicationJobId":"hidden-by-manual"});
        let suppressed_path = music.join("managed-suppressed.flac");
        std::fs::write(&suppressed_path, b"suppressed fixture").unwrap();
        save_asset(
            "managed-suppressed-asset",
            &suppressed_asset,
            &suppressed_path,
        );
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE recordings SET manual_metadata=?1 WHERE id=?2",
                params![r#"{"provenance":null}"#, suppressed.id],
            )
            .unwrap();

        let missing = track("managed-missing-path", "");
        let mut missing_asset = missing.clone();
        missing_asset.provenance = serde_json::json!({"publicationJobId":"missing-file-job"});
        save_asset(
            "managed-missing-file",
            &missing_asset,
            &music.join("absent.flac"),
        );

        // The asset's payload references this listing, while its normalized
        // recording_id points at a different but valid recording. Hydration
        // applies manual metadata by payload listing ID, so the candidate scan
        // must use that same ID rather than local_assets.recording_id.
        let remapped = track("managed-remapped-alias", "");
        let decoy = track("managed-remapped-decoy", "");
        store.save_track(&decoy).unwrap();
        let remapped_path = music.join("managed-remapped.flac");
        std::fs::write(&remapped_path, b"remapped fixture").unwrap();
        save_asset("managed-remapped-asset", &remapped, &remapped_path);
        store.connection.lock().unwrap().execute(
            "UPDATE recordings SET manual_metadata=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![r#"{"provenance":{"publicationJobId":"remapped-manual-job"}}"#, remapped.id],
        ).unwrap();
        // An absent decoy marker must not hide the payload listing marker.
        store.connection.lock().unwrap().execute(
            "UPDATE recordings SET manual_metadata=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![r#"{"provenance":null}"#, decoy.id],
        ).unwrap();
        let decoy_recording_id: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [&decoy.id],
                |row| row.get(0),
            )
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE local_assets SET recording_id=?1 WHERE id=?2",
                params![decoy_recording_id, "managed-remapped-asset"],
            )
            .unwrap();
        let hydrated_remapped = store
            .get_asset_by_id("managed-remapped-asset")
            .unwrap()
            .unwrap();
        assert_eq!(
            hydrated_remapped.track.provenance["publicationJobId"],
            "remapped-manual-job"
        );
        assert!(managed_publication_marker_is_effective(
            &remapped,
            &serde_json::json!({"provenance":{"publicationJobId":"remapped-manual-job"}}),
        ));

        let remapped_suppressed = track("managed-remapped-suppressed-alias", "");
        let mut remapped_suppressed_asset = remapped_suppressed.clone();
        remapped_suppressed_asset.provenance =
            serde_json::json!({"publicationJobId":"asset-job-hidden-by-manual"});
        let remapped_suppressed_decoy = track("managed-remapped-suppressed-decoy", "");
        store.save_track(&remapped_suppressed_decoy).unwrap();
        let remapped_suppressed_path = music.join("managed-remapped-suppressed.flac");
        std::fs::write(&remapped_suppressed_path, b"remapped suppression fixture").unwrap();
        save_asset(
            "managed-remapped-suppressed-asset",
            &remapped_suppressed_asset,
            &remapped_suppressed_path,
        );
        store.connection.lock().unwrap().execute(
            "UPDATE recordings SET manual_metadata=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![r#"{"provenance":null}"#, remapped_suppressed.id],
        ).unwrap();
        store.connection.lock().unwrap().execute(
            "UPDATE recordings SET manual_metadata=?1 WHERE id=(SELECT recording_id FROM catalog_listings WHERE id=?2)",
            params![r#"{"provenance":{"publicationJobId":"wrong-injected-decoy-job"}}"#, remapped_suppressed_decoy.id],
        ).unwrap();
        let remapped_suppressed_decoy_recording_id: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT recording_id FROM catalog_listings WHERE id=?1",
                [&remapped_suppressed_decoy.id],
                |row| row.get(0),
            )
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE local_assets SET recording_id=?1 WHERE id=?2",
                params![
                    remapped_suppressed_decoy_recording_id,
                    "managed-remapped-suppressed-asset"
                ],
            )
            .unwrap();
        let hydrated_suppressed = store
            .get_asset_by_id("managed-remapped-suppressed-asset")
            .unwrap()
            .unwrap();
        assert!(hydrated_suppressed
            .track
            .provenance
            .get("publicationJobId")
            .is_none());
        assert!(!managed_publication_marker_is_effective(
            &remapped_suppressed_asset,
            &serde_json::json!({"provenance":null}),
        ));

        let legacy_candidates = store
            .list_assets()
            .unwrap()
            .into_iter()
            .filter(|asset| {
                asset.track.provenance["publicationJobId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
            })
            .collect::<Vec<_>>();
        let mut paged_candidates = Vec::new();
        let mut before = None;
        loop {
            let (page, next) = store.managed_download_candidates_page(before, 500).unwrap();
            paged_candidates.extend(page);
            if next.is_none() {
                break;
            }
            before = next;
        }
        assert_eq!(
            serde_json::to_value(&paged_candidates).unwrap(),
            serde_json::to_value(&legacy_candidates).unwrap()
        );
        let expected_added = legacy_candidates
            .iter()
            .filter(|asset| Path::new(&asset.path).is_file())
            .count();
        assert_eq!(expected_added, 4);
        assert_eq!(store.backfill_managed_downloads().unwrap(), expected_added);
        let downloads = store.ensure_downloads_collection().unwrap();
        let rows = store
            .list_tracks("collection", Some(&downloads.id))
            .unwrap();
        assert_eq!(rows.len(), 3); // Two duplicate occurrences collapse in the legacy membership table.
        assert!(rows.iter().any(|row| row.id == duplicate.id));
        assert!(rows.iter().any(|row| row.id == manual.id));
        assert!(rows.iter().any(|row| row.id == remapped.id));
        assert!(!rows.iter().any(|row| row.id == suppressed.id));
        assert!(!rows.iter().any(|row| row.id == missing.id));
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
    fn favorite_ids_match_favorites_view_and_return_only_canonical_listing_ids() {
        let (_d, store) = db();
        let favorite = track("1", "Favorite");
        let alias = track("2", "Alias");
        let ordinary = track("3", "Ordinary");
        store.save_track(&favorite).unwrap();
        store.save_track(&alias).unwrap();
        store.save_track(&ordinary).unwrap();
        store.set_favorite(&favorite, true).unwrap();
        store
            .merge_recordings(&favorite.id, &[alias.id.clone()])
            .unwrap();

        let expected = store
            .list_tracks("favorites", None)
            .unwrap()
            .into_iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();
        assert_eq!(store.list_favorite_track_ids().unwrap(), expected);
        assert_eq!(expected, vec![favorite.id]);
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
    fn date_added_survives_store_reads_manual_edits_and_global_page_sorting() {
        let (_directory, store) = db();
        let mut edited = track("date-dto-edited", "");
        edited.date_added = Some(serde_json::json!("2030-01-01"));
        store.save_track(&edited).unwrap();
        let loaded = store.get_track(&edited.id).unwrap().unwrap();
        assert_eq!(loaded.date_added, edited.date_added);
        let mut manual = loaded;
        manual.date_added = Some(serde_json::json!("2022-01-01"));
        store.update_track(&manual).unwrap();
        let mut other = track("date-dto-other", "");
        other.date_added = Some(serde_json::json!("2025-01-01"));
        store.save_track(&other).unwrap();
        let mut legacy = track("date-dto-invalid", "");
        legacy.date_added = Some(serde_json::json!(false));
        legacy.provenance = serde_json::json!({"dateAdded":"2020-01-01"});
        store.save_track(&legacy).unwrap();
        let (page, total) = store
            .list_tracks_page(
                "library",
                None,
                200,
                0,
                None,
                Some("dateAdded"),
                Some("asc"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(
            page.iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            vec![edited.id.as_str(), other.id.as_str(), legacy.id.as_str()]
        );
        assert_eq!(
            serde_json::to_value(&page[0]).unwrap()["dateAdded"],
            "2022-01-01"
        );
        assert_eq!(serde_json::to_value(&page[2]).unwrap()["dateAdded"], false);
        let batch = store
            .get_tracks(&[edited.id.clone(), edited.id.clone()])
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch
            .iter()
            .all(|track| track.date_added == Some(serde_json::json!("2022-01-01"))));
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

    #[test]
    fn connection_wait_metrics_record_uncontended_and_contended_locks() {
        let (_dir, mut store) = db();
        store.connection_metrics_enabled = true;
        store.connection_wait_metrics = ConnectionWaitMetrics::default();
        let store = std::sync::Arc::new(store);

        drop(store.lock_connection().unwrap());
        let held = store.lock_connection().unwrap();
        let worker_store = store.clone();
        let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let on_attempt = |started| attempt_tx.send(started).unwrap();
            drop(worker_store.lock_connection_observed(&on_attempt).unwrap());
        });
        let attempt_started = attempt_rx.recv().unwrap();
        let minimum_wait = std::time::Duration::from_millis(50);
        std::thread::sleep(minimum_wait.saturating_sub(attempt_started.elapsed()));
        drop(held);
        worker.join().unwrap();

        let metrics = store.connection_wait_metrics().unwrap();
        assert_eq!(metrics["scope"], "global_store_connection_mutex");
        assert_eq!(metrics["lockCount"], 3);
        let minimum_wait_ns = minimum_wait.as_nanos() as u64;
        assert!(metrics["totalWaitNs"].as_u64().unwrap() >= minimum_wait_ns);
        assert!(metrics["maxWaitNs"].as_u64().unwrap() >= minimum_wait_ns);
        assert_eq!(metrics["sqliteEngineBusyWait"], "not_measured");
    }

    #[test]
    fn connection_wait_metrics_are_absent_when_disabled() {
        let (_dir, mut store) = db();
        store.connection_metrics_enabled = false;
        assert!(store.connection_wait_metrics().is_none());
        let _ = store.settings().unwrap();
        assert!(store.connection_wait_metrics().is_none());
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

fn managed_publication_marker_is_effective(
    track: &Track,
    manual_metadata: &serde_json::Value,
) -> bool {
    // apply_hydration_metadata overlays a manual `provenance` value wholesale;
    // an explicit null, scalar, array, or object without an ID suppresses the
    // asset payload value. A missing manual field leaves it untouched.
    let provenance = manual_metadata
        .as_object()
        .and_then(|manual| manual.get("provenance"))
        .unwrap_or(&track.provenance);
    provenance
        .as_object()
        .and_then(|provenance| provenance.get("publicationJobId"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|job_id| !job_id.is_empty())
}

#[cfg(test)]
mod persisted_text_sort_index_proposal {
    use rusqlite::{params, Connection};
    use serde_json::Value;

    #[test]
    #[ignore = "50k synthetic expression-index benchmark; explicitly coordinate; 3 warmups/20 paired reads per case"]
    fn persisted_text_sort_index_50k_report() {
        run_persisted_text_sort_index_report(50_000, 3, 20, "qualification_50k");
    }

    #[test]
    #[ignore = "5k exploratory expression-index benchmark; 1 warmup/1 paired read per case"]
    fn persisted_text_sort_index_5k_exploratory_report() {
        run_persisted_text_sort_index_report(5_000, 1, 1, "exploratory_5k");
    }

    fn run_persisted_text_sort_index_report(
        rows: usize,
        warmups: usize,
        repetitions: usize,
        run_kind: &str,
    ) {
        use std::time::Instant;

        eprintln!(
            "PERSISTED_TEXT_SORT_CASE_BEGIN {}",
            serde_json::json!({"test":run_kind,"rows":rows,"coverage":"in_memory_expression_sort_only_not_whole_store_or_effective_metadata_preparation","warmupsPerCase":warmups,"timedPairsPerCase":repetitions,"cases":4,"aggregation":"only emitted after all four cases; inspect CASE_JSON records for completed cases"})
        );
        eprintln!(
            "PERSISTED_TEXT_SORT_PHASE {}",
            serde_json::json!({"test":run_kind,"phase":"connection_and_collation_begin"})
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&connection).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE catalog_listings(
                id TEXT NOT NULL,
                in_library INTEGER NOT NULL,
                sort_projection_revision INTEGER,
                sort_metadata_projection TEXT
            );",
            )
            .unwrap();
        eprintln!(
            "PERSISTED_TEXT_SORT_PHASE {}",
            serde_json::json!({"test":run_kind,"phase":"table_ready"})
        );
        {
            let transaction = connection.unchecked_transaction().unwrap();
            {
                let mut insert = transaction.prepare(
                    "INSERT INTO catalog_listings(id,in_library,sort_projection_revision,sort_metadata_projection) VALUES(?1,1,1,?2)",
                ).unwrap();
                for index in 0..rows {
                    let title = if index % 97 == 0 {
                        None
                    } else if index % 43 == 0 {
                        Some(String::new())
                    } else {
                        Some(match index % 5 {
                            0 => format!("Café Track {}", index % 211),
                            1 => format!("Cafe\u{301} track {}", index % 211),
                            2 => format!("CAFÉ TRACK {}", index % 211),
                            3 => format!("Track {}", index % 211),
                            _ => format!("track {}", index % 211),
                        })
                    };
                    let artist = if index % 89 == 0 {
                        None
                    } else if index % 31 == 0 {
                        Some("   ".to_owned())
                    } else {
                        Some(match index % 4 {
                            0 => format!("Élan {}", index % 101),
                            1 => format!("ELAN {}", index % 101),
                            2 => format!("Artist {}", index % 101),
                            _ => format!("artist {}", index % 101),
                        })
                    };
                    let projection =
                        serde_json::json!({"title": title, "artist": artist}).to_string();
                    insert
                        .execute(params![format!("occurrence-{index}"), projection])
                        .unwrap();
                }
            }
            transaction.commit().unwrap();
        }
        eprintln!(
            "PERSISTED_TEXT_SORT_PHASE {}",
            serde_json::json!({"test":run_kind,"phase":"seed_complete","rows":rows})
        );

        let mut report = Vec::new();
        for field in ["title", "artist"] {
            let key = crate::track_sort_sql::expression(field, "sort_metadata_projection").unwrap();
            // This integer null-rank must use BINARY in the ORDER BY and index expression.
            // Without the explicit collation SQLite does not match the CASE rank to the index.
            let null_key = format!("(CASE WHEN ({key}) IS NULL THEN 1 ELSE 0 END) COLLATE BINARY");
            for (label, direction) in [("asc", "ASC"), ("desc", "DESC")] {
                let case = format!("{field}_{label}");
                eprintln!(
                    "PERSISTED_TEXT_SORT_PHASE {}",
                    serde_json::json!({"test":run_kind,"phase":"case_begin","case":&case,"rows":rows})
                );
                connection
                    .execute_batch(
                        "DROP INDEX IF EXISTS bench_sort_title_asc;
                     DROP INDEX IF EXISTS bench_sort_title_desc;
                     DROP INDEX IF EXISTS bench_sort_artist_asc;
                     DROP INDEX IF EXISTS bench_sort_artist_desc;",
                    )
                    .unwrap();
                let index_name = format!("bench_sort_{field}_{label}");
                // Reverse scanning then yields the requested sort and rowid DESC tie;
                // expression indexes otherwise append rowid ASC.
                let index_key_direction = if direction == "ASC" { "DESC" } else { "ASC" };
                connection.execute_batch(&format!(
                    "CREATE INDEX {index_name} ON catalog_listings(({null_key}) DESC,({key}) {index_key_direction}) WHERE in_library=1 AND sort_projection_revision=1 AND sort_metadata_projection IS NOT NULL;"
                )).unwrap();
                eprintln!(
                    "PERSISTED_TEXT_SORT_PHASE {}",
                    serde_json::json!({"test":run_kind,"phase":"index_ready","case":&case})
                );
                let reference_sql = format!(
                    "SELECT id FROM catalog_listings NOT INDEXED WHERE in_library=1 AND sort_projection_revision=1 AND sort_metadata_projection IS NOT NULL ORDER BY ({null_key}) ASC,({key}) {direction},rowid DESC"
                );
                let candidate_sql = format!(
                    "SELECT id FROM catalog_listings WHERE in_library=1 AND sort_projection_revision=1 AND sort_metadata_projection IS NOT NULL ORDER BY ({null_key}) ASC,({key}) {direction},rowid DESC"
                );
                let collect = |sql: &str| {
                    connection
                        .prepare(sql)
                        .unwrap()
                        .query_map([], |row| row.get::<_, String>(0))
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap()
                };
                let reference_order = collect(&reference_sql);
                let candidate_order = collect(&candidate_sql);
                assert_eq!(
                    candidate_order, reference_order,
                    "full {field} {label} ordering must match"
                );
                eprintln!(
                    "PERSISTED_TEXT_SORT_PHASE {}",
                    serde_json::json!({"test":run_kind,"phase":"full_order_parity_complete","case":&case,"rowsCompared":reference_order.len()})
                );

                let plan = connection
                    .prepare(&format!("EXPLAIN QUERY PLAN {candidate_sql} LIMIT 200"))
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(3))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                assert!(
                    plan.iter().any(|detail| detail.contains(&index_name)),
                    "candidate plan did not use index: {plan:?}"
                );
                let full_temp_sort = plan
                    .iter()
                    .any(|detail| detail.contains("USE TEMP B-TREE FOR ORDER BY"));
                let right_part_temp_sort = plan
                    .iter()
                    .any(|detail| detail.contains("RIGHT PART OF ORDER BY"));
                eprintln!(
                    "PERSISTED_TEXT_SORT_PHASE {}",
                    serde_json::json!({"test":run_kind,"phase":"explain_complete","case":&case,"plan":&plan,"fullTempSort":full_temp_sort,"rightPartTempSort":right_part_temp_sort})
                );

                let read_page = |sql: &str| {
                    connection
                        .prepare(&format!("{sql} LIMIT 200 OFFSET 0"))
                        .unwrap()
                        .query_map([], |row| row.get::<_, String>(0))
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap()
                };
                for _ in 0..warmups {
                    assert_eq!(read_page(&reference_sql), read_page(&candidate_sql));
                }
                eprintln!(
                    "PERSISTED_TEXT_SORT_PHASE {}",
                    serde_json::json!({"test":run_kind,"phase":"warmups_complete","case":&case,"count":warmups})
                );
                let mut samples = Vec::with_capacity(repetitions);
                for repetition in 0..repetitions {
                    let (first_sql, second_sql, first_is_reference) = if repetition % 2 == 0 {
                        (&reference_sql, &candidate_sql, true)
                    } else {
                        (&candidate_sql, &reference_sql, false)
                    };
                    let started = Instant::now();
                    let _ = read_page(first_sql);
                    let first_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let started = Instant::now();
                    let _ = read_page(second_sql);
                    let second_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let sample = if first_is_reference {
                        serde_json::json!({"pair":repetition+1,"referenceMs":first_ms,"candidateMs":second_ms})
                    } else {
                        serde_json::json!({"pair":repetition+1,"referenceMs":second_ms,"candidateMs":first_ms})
                    };
                    samples.push(sample.clone());
                    eprintln!(
                        "PERSISTED_TEXT_SORT_PAIR {}",
                        serde_json::json!({"test":run_kind,"case":&case,"rows":rows,"coverage":"in_memory_expression_sort_only_not_whole_store_or_effective_metadata_preparation","sample":sample})
                    );
                }
                let summarize = |field: &str| {
                    let mut values = samples
                        .iter()
                        .map(|sample| sample[field].as_f64().unwrap())
                        .collect::<Vec<_>>();
                    values.sort_by(f64::total_cmp);
                    let middle = values.len() / 2;
                    let median = if values.len() % 2 == 0 {
                        (values[middle - 1] + values[middle]) / 2.0
                    } else {
                        values[middle]
                    };
                    serde_json::json!({
                        "medianMs": median,
                        "p95Ms": values[((values.len()-1)*95)/100],
                        "worstMs": values[values.len()-1],
                    })
                };
                let case_report = serde_json::json!({
                    "field": field,
                    "direction": label,
                    "rows": rows,
                    "scope": "in_memory_expression_sort_only_not_whole_store_or_effective_metadata_preparation",
                    "warmups": warmups,
                    "timedPairs": repetitions,
                    "reference": summarize("referenceMs"),
                    "candidate": summarize("candidateMs"),
                    "pairedSamples": samples,
                    "plan": plan,
                    "fullTempSort": full_temp_sort,
                    "rightPartTempSort": right_part_temp_sort,
                    "tieOrderNote": "rowid DESC is the stable source tie; SQLite expression indexes append rowid ASC implicitly, so inspect whether only equal-key groups are temp-sorted",
                });
                eprintln!(
                    "PERSISTED_TEXT_SORT_CASE_JSON {}",
                    serde_json::json!({"test":run_kind,"rows":rows,"coverage":"in_memory_expression_sort_only_not_whole_store_or_effective_metadata_preparation","completedCase":&case_report})
                );
                report.push(case_report);
            }
        }
        eprintln!(
            "PERSISTED_TEXT_SORT_AGGREGATE {}",
            serde_json::json!({"test":run_kind,"rows":rows,"completedCases":report.len(),"report":report})
        );
    }

    #[test]
    fn persisted_title_artist_expression_indexes_preserve_generic_order_and_are_plannable() {
        let connection = Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&connection).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE catalog_listings(
                id TEXT NOT NULL,
                in_library INTEGER NOT NULL,
                sort_projection_revision INTEGER,
                sort_metadata_projection TEXT
            );",
            )
            .unwrap();

        let conformance: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/track-sort-conformance.json"
        ))
        .unwrap();
        let mut values = Vec::<Option<String>>::new();
        for case in conformance["sortCases"].as_array().unwrap() {
            values.extend(
                case["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| Some(value.as_str().unwrap().to_owned())),
            );
        }
        for pair in conformance["equalPairs"].as_array().unwrap() {
            values.push(Some(pair[0].as_str().unwrap().to_owned()));
            values.push(Some(pair[1].as_str().unwrap().to_owned()));
        }
        values.extend([Some(String::new()), Some("   ".to_owned()), None]);
        for (index, value) in values.iter().enumerate() {
            let projection = serde_json::json!({
                "title": value,
                "artist": value,
            })
            .to_string();
            connection.execute(
                "INSERT INTO catalog_listings(id,in_library,sort_projection_revision,sort_metadata_projection) VALUES(?1,1,1,?2)",
                params![format!("occurrence-{index}"), projection],
            ).unwrap();
        }

        for field in ["title", "artist"] {
            let key = crate::track_sort_sql::expression(field, "sort_metadata_projection").unwrap();
            let null_key = format!("CASE WHEN ({key}) IS NULL THEN 1 ELSE 0 END");
            for (label, direction) in [("asc", "ASC"), ("desc", "DESC")] {
                let index_name = format!("sort_{field}_{label}");
                connection.execute_batch(&format!(
                    "CREATE INDEX {index_name} ON catalog_listings(({null_key}) ASC,({key}) {direction}) WHERE in_library=1 AND sort_projection_revision=1 AND sort_metadata_projection IS NOT NULL;"
                )).unwrap();

                let generic_sql = format!(
                    "SELECT id FROM catalog_listings NOT INDEXED WHERE in_library=1 AND sort_projection_revision=1 AND sort_metadata_projection IS NOT NULL ORDER BY ({null_key}) ASC,({key}) {direction},rowid DESC"
                );
                let indexed_sql = format!(
                    "SELECT id FROM catalog_listings WHERE in_library=1 AND sort_projection_revision=1 AND sort_metadata_projection IS NOT NULL ORDER BY ({null_key}) ASC,({key}) {direction},rowid DESC"
                );
                let collect = |sql: &str| {
                    connection
                        .prepare(sql)
                        .unwrap()
                        .query_map([], |row| row.get::<_, String>(0))
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap()
                };
                let generic = collect(&generic_sql);
                let indexed = collect(&indexed_sql);
                assert_eq!(
                    indexed, generic,
                    "{field} {label} index must preserve null and stable occurrence ordering"
                );

                let plan = connection
                    .prepare(&format!("EXPLAIN QUERY PLAN {indexed_sql}"))
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(3))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                assert!(
                    plan.iter().any(|detail| detail.contains(&index_name)),
                    "{field} {label} should use its matching expression index: {plan:?}"
                );
            }
        }
    }
}
