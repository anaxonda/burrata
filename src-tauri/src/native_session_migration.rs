//! Legacy engine preference is a migration receipt only. Native is always the
//! running engine; old browser data remains available until this transaction.
use crate::storage::Store;
use serde_json::{json, Value};
const ENGINE: &str = "playbackEngine.v1";

pub(crate) fn needs_legacy_session(store: &Store) -> Result<bool, String> {
    Ok(store
        .get_json_preference(ENGINE)?
        .as_ref()
        .and_then(Value::as_str)
        != Some("native"))
}

/// The caller validates the complete queue and holds playback command/persistence
/// ownership. An existing Native owner wins over any competing browser session.
pub(crate) fn commit(store: &Store, queue: &Value, session: &Value) -> Result<bool, String> {
    if !needs_legacy_session(store)? {
        return Ok(false);
    }
    store.save_playback_checkpoint_and_engine(
        Some(queue),
        Some(session),
        Some(&json!("native")),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkpoint(empty: bool) -> (Value, Value) {
        let tracks = if empty {
            json!([])
        } else {
            json!([{"id":"same"},{"id":"same"}])
        };
        let ids = if empty {
            json!([])
        } else {
            json!(["first", "second"])
        };
        (
            json!({"tracks":tracks,"entryIds":ids,"queueRevision":7}),
            json!({"index":if empty {0}else{1},"currentEntryId":if empty {Value::Null}else{json!("second")},
            "queueRevision":7,"position":23.5,"volume":0.3,"repeat":"all","shuffle":true,"finished":false}),
        )
    }
    #[test]
    fn native_owner_wins_over_stale_competing_browser_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        let (queue, session) = checkpoint(false);
        store
            .save_playback_checkpoint_and_engine(
                Some(&queue),
                Some(&session),
                Some(&json!("native")),
            )
            .unwrap();
        let (stale, stale_session) = checkpoint(true);
        assert!(!commit(&store, &stale, &stale_session).unwrap());
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
    fn previous_browser_owner_migrates_duplicates_or_an_explicit_empty_queue() {
        for empty in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("db")).unwrap();
            let (stale, stale_session) = checkpoint(!empty);
            store
                .save_playback_checkpoint_and_engine(
                    Some(&stale),
                    Some(&stale_session),
                    Some(&json!("webview")),
                )
                .unwrap();
            let (queue, session) = checkpoint(empty);
            assert!(commit(&store, &queue, &session).unwrap());
            assert!(!needs_legacy_session(&store).unwrap());
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
    }
    #[test]
    fn interrupted_commit_retains_original_session_and_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let store = Store::open(&path).unwrap();
        let (old, old_session) = checkpoint(true);
        store
            .save_playback_checkpoint_and_engine(
                Some(&old),
                Some(&old_session),
                Some(&json!("webview")),
            )
            .unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_migration BEFORE INSERT ON settings WHEN NEW.key='playbackEngine.v1' BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
        let (queue, session) = checkpoint(false);
        assert!(commit(&store, &queue, &session).is_err());
        assert!(needs_legacy_session(&store).unwrap());
        assert_eq!(
            store.get_json_preference("nativePlaybackQueue.v1").unwrap(),
            Some(old)
        );
        assert_eq!(
            store
                .get_json_preference("nativePlaybackSession.v1")
                .unwrap(),
            Some(old_session)
        );
        connection
            .execute_batch("DROP TRIGGER reject_migration")
            .unwrap();
        assert!(commit(&store, &queue, &session).unwrap());
    }
}
