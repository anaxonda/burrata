use crate::models::Track;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackHistoryEntry {
    pub session_id: String,
    pub played_at: String,
    pub provider: String,
    pub track: Track,
    pub chapter: Option<Value>,
}

/// Persistent playback history, stored independently from the library database.
pub struct PlaybackHistory {
    connection: Mutex<Connection>,
}

impl PlaybackHistory {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS playback_history (
               session_id TEXT PRIMARY KEY NOT NULL,
               played_at TEXT NOT NULL,
               provider TEXT NOT NULL,
               track_json TEXT NOT NULL,
               chapter_json TEXT
             );
             CREATE INDEX IF NOT EXISTS playback_history_played_at ON playback_history(played_at DESC);",
        ).map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
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
        let track_json = serde_json::to_string(track).map_err(|e| e.to_string())?;
        let chapter_json = chapter
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        let changed = connection.execute(
            "INSERT OR IGNORE INTO playback_history(session_id, played_at, provider, track_json, chapter_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, now, provider, track_json, chapter_json],
        ).map_err(|e| e.to_string())?;
        Ok(changed == 1)
    }

    pub fn list(&self, limit: u32, offset: u32) -> Result<Vec<PlaybackHistoryEntry>, String> {
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
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

    pub fn clear(&self) -> Result<(), String> {
        self.connection
            .lock()
            .map_err(|e| e.to_string())?
            .execute("DELETE FROM playback_history", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn contains_session(&self, session_id: &str) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
