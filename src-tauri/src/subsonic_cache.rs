//! Account-scoped Subsonic metadata cache and read-only Strawberry import.
use crate::subsonic_client::Song;
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

pub(crate) const MAX_SONGS: usize = 100_000;

pub(crate) fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|_| "Could not open the Subsonic catalog cache")?;
    conn.execute_batch(
        "PRAGMA trusted_schema=OFF;
         CREATE TABLE IF NOT EXISTS snapshots (
            account TEXT PRIMARY KEY, source TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS songs (
            account TEXT NOT NULL, id TEXT NOT NULL, title TEXT NOT NULL,
            artist TEXT NOT NULL, album TEXT NOT NULL, payload TEXT NOT NULL,
            PRIMARY KEY(account,id)
         );
         CREATE INDEX IF NOT EXISTS songs_browse ON songs(account,artist,album,title,id);",
    )
    .map_err(|_| "Could not initialize the Subsonic catalog cache")?;
    Ok(conn)
}

pub(crate) fn count(conn: &Connection, account: &str) -> Result<usize, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM songs WHERE account=?1",
        [account],
        |row| row.get(0),
    )
    .map_err(|_| "Could not count the Subsonic catalog".into())
}

pub(crate) fn can_seed(conn: &Connection, account: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM snapshots WHERE account=?1)",
        [account],
        |row| row.get(0),
    )
    .map_err(|_| "Could not check Subsonic catalog history".into())
}

pub(crate) fn replace(
    conn: &mut Connection,
    account: &str,
    songs: &[Song],
    from_strawberry: bool,
) -> Result<(), String> {
    if songs.len() > MAX_SONGS {
        return Err("The Subsonic catalog is larger than the supported cache".into());
    }
    let failed = |_| "Could not save the Subsonic catalog cache";
    let tx = conn.transaction().map_err(failed)?;
    if from_strawberry && !can_seed(&tx, account)? {
        return Err("This account already has a Burrata Subsonic catalog".into());
    }
    tx.execute("DELETE FROM songs WHERE account=?1", [account])
        .map_err(failed)?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO songs(account,id,title,artist,album,payload) VALUES(?1,?2,?3,?4,?5,?6)",
        ).map_err(failed)?;
        for song in songs {
            let payload =
                serde_json::to_string(song).map_err(|_| "Could not encode a Subsonic song")?;
            insert
                .execute(params![
                    account,
                    song.id,
                    song.title.to_lowercase(),
                    song.artist.as_deref().unwrap_or_default().to_lowercase(),
                    song.album.as_deref().unwrap_or_default().to_lowercase(),
                    payload,
                ])
                .map_err(failed)?;
        }
    }
    tx.execute(
        "INSERT INTO snapshots(account,source) VALUES(?1,?2)
         ON CONFLICT(account) DO UPDATE SET source=excluded.source",
        params![
            account,
            if from_strawberry {
                "strawberry"
            } else {
                "server"
            }
        ],
    )
    .map_err(failed)?;
    tx.commit()
        .map_err(|_| "Could not commit the Subsonic catalog cache".into())
}

pub(crate) fn page(
    conn: &Connection,
    account: &str,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<(Vec<Song>, usize), String> {
    let search = query
        .chars()
        .take(160)
        .collect::<String>()
        .trim()
        .to_lowercase();
    let filter =
        "account=?1 AND (?2='' OR instr(title,?2)>0 OR instr(artist,?2)>0 OR instr(album,?2)>0)";
    let failed = |_| "Could not read the Subsonic catalog cache";
    let total = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM songs WHERE {filter}"),
            params![account, search],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let mut stmt = conn
        .prepare(&format!(
        "SELECT payload FROM songs WHERE {filter} ORDER BY artist,album,title,id LIMIT ?3 OFFSET ?4"
    ))
        .map_err(failed)?;
    let rows = stmt
        .query_map(
            params![account, search, limit.clamp(1, 200), offset.min(MAX_SONGS)],
            |row| row.get::<_, String>(0),
        )
        .map_err(failed)?;
    let songs = rows
        .map(|row| {
            serde_json::from_str(&row.map_err(failed)?)
                .map_err(|_| "Invalid song in the Subsonic catalog cache")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((songs, total))
}

pub(crate) fn song(conn: &Connection, account: &str, id: &str) -> Result<Option<Song>, String> {
    use rusqlite::OptionalExtension;
    let raw = conn
        .query_row(
            "SELECT songs.payload FROM songs JOIN snapshots USING(account)
             WHERE songs.account=?1 AND songs.id=?2 AND snapshots.source='server'",
            params![account, id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| "Could not read the Subsonic song")?;
    raw.map(|payload| {
        serde_json::from_str(&payload).map_err(|_| "Invalid cached Subsonic song".into())
    })
    .transpose()
}

pub(crate) fn strawberry(path: &Path) -> Result<Vec<Song>, String> {
    let metadata = std::fs::metadata(path).map_err(|_| "Could not open the Strawberry database")?;
    if !metadata.is_file() || metadata.len() > 2_000_000_000 {
        return Err("Choose a Strawberry database file smaller than 2 GB".into());
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "Could not open the Strawberry database read-only")?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
        .map_err(|_| "Could not prepare the Strawberry database for read-only access")?;
    let actual_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='subsonic_songs')",
        [], |row| row.get(0)
    ).map_err(|_| "Could not check the Strawberry database schema")?;
    if !actual_table {
        return Err("This Strawberry database has no Subsonic song cache".into());
    }
    // These columns are Strawberry metadata. Its cached URL and filesystem
    // path are intentionally never selected, and its duration unit is ns.
    let mut stmt = conn
        .prepare(
            "SELECT song_id, album_id, title, artist, album, albumartist, length, track, disc,
                genre, year, musicbrainz_recording_id
         FROM subsonic_songs WHERE song_id<>'' AND title<>'' ORDER BY song_id LIMIT ?1",
        )
        .map_err(|_| "The Strawberry Subsonic cache schema is unsupported")?;
    let rows = stmt
        .query_map([MAX_SONGS + 1], |row| {
            let duration = row
                .get::<_, Option<i64>>(6)?
                .map(|ns| ns as f64 / 1_000_000_000.0)
                .filter(|seconds| (0.001..=86_400.0).contains(seconds));
            let text = |column| -> rusqlite::Result<Option<String>> {
                Ok(row
                    .get::<_, Option<String>>(column)?
                    .map(|value| value.trim().to_owned())
                    .filter(|value| {
                        !value.is_empty()
                            && value.len() <= 1024
                            && !value.chars().any(char::is_control)
                    }))
            };
            let positive = |column| -> rusqlite::Result<Option<u32>> {
                Ok(row
                    .get::<_, Option<i64>>(column)?
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0))
            };
            Ok(Song {
                id: text(0)?.unwrap_or_default(),
                album_id: text(1)?,
                title: text(2)?.unwrap_or_default(),
                artist: text(3)?,
                album: text(4)?,
                album_artist: text(5)?,
                duration,
                track: positive(7)?,
                disc: positive(8)?,
                genre: text(9)?,
                year: positive(10)?,
                recording_mbid: text(11)?
                    .and_then(|value| uuid::Uuid::parse_str(&value).ok().map(|id| id.to_string())),
                ..Default::default()
            })
        })
        .map_err(|_| "Could not read the Strawberry Subsonic cache")?;
    let mut songs = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for row in rows {
        let song = row.map_err(|_| "Invalid metadata in the Strawberry Subsonic cache")?;
        if songs.len() == MAX_SONGS {
            return Err("The Strawberry Subsonic cache is larger than supported".into());
        }
        if song.id.is_empty()
            || song.title.is_empty()
            || song.album_id.is_none()
            || !ids.insert(song.id.clone())
        {
            return Err(
                "The Strawberry Subsonic cache has missing or duplicate song identities".into(),
            );
        }
        songs.push(song);
    }
    if songs.is_empty() {
        return Err("The Strawberry Subsonic cache is empty".into());
    }
    Ok(songs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_snapshots_replace_only_one_account_and_roll_back_partial_updates() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(&dir.path().join("cache.sqlite")).unwrap();
        let song = Song {
            id: "shared".into(),
            title: "One".into(),
            ..Song::default()
        };
        replace(&mut db, "first", &[song.clone()], true).unwrap();
        assert!(!can_seed(&db, "first").unwrap());
        assert!(super::song(&db, "first", "shared").unwrap().is_none());
        assert!(can_seed(&db, "second").unwrap());
        replace(
            &mut db,
            "second",
            &[Song {
                title: "Two".into(),
                ..song.clone()
            }],
            false,
        )
        .unwrap();
        assert_eq!(
            super::song(&db, "second", "shared").unwrap().unwrap().title,
            "Two"
        );
        assert!(replace(&mut db, "first", &[song.clone(), song], false).is_err());
        assert_eq!(page(&db, "first", "ONE", 0, 25).unwrap().1, 1);
        replace(&mut db, "first", &[], false).unwrap();
        assert_eq!(count(&db, "first").unwrap(), 0);
        assert_eq!(page(&db, "second", "", 0, 25).unwrap().0[0].title, "Two");
    }

    #[test]
    fn strawberry_reader_leaves_input_unchanged_and_never_imports_stream_urls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("strawberry.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE subsonic_songs(song_id TEXT, album_id TEXT, title TEXT, artist TEXT, album TEXT,
              albumartist TEXT,length INTEGER,track INTEGER,disc INTEGER,genre TEXT,year INTEGER,
              musicbrainz_recording_id TEXT,url TEXT);
             INSERT INTO subsonic_songs VALUES('song','album','Title','Artist','Album','Artist',
                175000000000,1,1,'Jazz',2020,NULL,'https://fixture.invalid/cached-token');",
        ).unwrap();
        drop(db);
        let original = std::fs::read(&path).unwrap();
        let rows = strawberry(&path).unwrap();
        assert_eq!(rows[0].duration, Some(175.0));
        assert!(!serde_json::to_string(&rows)
            .unwrap()
            .contains("cached-token"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
