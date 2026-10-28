use crate::{backup_state, storage::Store};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const PENDING_KEY: &str = "backupFrontendPreferences.v1";
const SCHEMA_VERSION: u32 = 1;
const MAX_PENDING_BYTES: usize = 1024 * 1024;
const MIGRATION_ATTEMPTS: usize = 4;

/// Durable imported preferences. Process-session identity is intentionally not
/// stored here; it is minted afresh by AppState and returned with each read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingEnvelope {
    pub schema_version: u32,
    pub nonce: String,
    pub preferences: BTreeMap<String, Value>,
}

impl PendingEnvelope {
    pub fn new(preferences: BTreeMap<String, Value>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            nonce: uuid::Uuid::new_v4().to_string(),
            preferences,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResponse {
    pub nonce: String,
    pub current_session_id: String,
    pub preferences: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AckResult {
    Acknowledged,
    Deferred,
    Stale,
}

/// Read a pending envelope and compare-and-swap any legacy bare map into the
/// current shape. The exact stored bytes fence migration from a concurrent
/// newer import even if it uses another SQLite connection.
pub fn read_or_migrate(
    store: &Store,
    current_session_id: &str,
) -> Result<Option<ReadResponse>, String> {
    if uuid::Uuid::parse_str(current_session_id).is_err() {
        return Err("Backend process session identity is invalid".into());
    }
    store.with_backup_connection(|connection| {
        for _ in 0..MIGRATION_ATTEMPTS {
            let raw: Option<String> = connection
                .query_row(
                    "SELECT value FROM settings WHERE key=?1",
                    [PENDING_KEY],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            let Some(raw) = raw else { return Ok(None) };
            if raw.len() > MAX_PENDING_BYTES {
                return Err("Pending frontend preferences exceed the size limit".into());
            }
            let value: Value = serde_json::from_str(&raw)
                .map_err(|_| "Pending frontend preferences are malformed".to_owned())?;
            let object = value
                .as_object()
                .ok_or("Pending frontend preferences must be an object")?;

            // An empty object is the historical cleared value. Do not turn it
            // back into a fresh envelope, which would create a receipt on
            // every startup despite there being nothing to apply.
            if object.is_empty() {
                return Ok(None);
            }

            let is_envelope = object.contains_key("schemaVersion")
                || object.contains_key("nonce")
                || object.contains_key("preferences");
            let preferences_value = if is_envelope {
                let safe_preferences = object
                    .get("preferences")
                    .map(backup_state::sanitize_frontend_preferences)
                    .unwrap_or_default();
                let metadata_valid = object.get("schemaVersion").and_then(Value::as_u64)
                    == Some(u64::from(SCHEMA_VERSION))
                    && object
                        .get("nonce")
                        .and_then(Value::as_str)
                        .is_some_and(|nonce| uuid::Uuid::parse_str(nonce).is_ok())
                    && object.get("preferences").is_some_and(Value::is_object);
                if metadata_valid {
                    let nonce = object
                        .get("nonce")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    return Ok(Some(ReadResponse {
                        nonce,
                        current_session_id: current_session_id.to_owned(),
                        preferences: safe_preferences,
                    }));
                }
                Value::Object(safe_preferences.into_iter().collect())
            } else {
                value
            };

            // Bare maps from prior versions and envelopes with damaged metadata
            // are replaced only if the exact bytes read above are still current.
            let safe_preferences = backup_state::sanitize_frontend_preferences(&preferences_value);
            if safe_preferences.is_empty() {
                return Ok(None);
            }
            let envelope = PendingEnvelope::new(safe_preferences.clone());
            let serialized = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
            let changed = connection
                .execute(
                    "UPDATE settings SET value=?1 WHERE key=?2 AND value=?3",
                    rusqlite::params![serialized, PENDING_KEY, raw],
                )
                .map_err(|error| error.to_string())?;
            if changed == 1 {
                return Ok(Some(ReadResponse {
                    nonce: envelope.nonce,
                    current_session_id: current_session_id.to_owned(),
                    preferences: safe_preferences,
                }));
            }
        }
        Err("Pending frontend preferences changed during migration; retry startup".into())
    })
}

pub fn acknowledge(
    store: &Store,
    current_session_id: &str,
    nonce: &str,
    receipt_session_id: &str,
) -> Result<AckResult, String> {
    acknowledge_key(
        store,
        current_session_id,
        PENDING_KEY,
        nonce,
        receipt_session_id,
    )
}

pub fn acknowledge_key(
    store: &Store,
    current_session_id: &str,
    key: &str,
    nonce: &str,
    receipt_session_id: &str,
) -> Result<AckResult, String> {
    if uuid::Uuid::parse_str(current_session_id).is_err()
        || uuid::Uuid::parse_str(receipt_session_id).is_err()
        || uuid::Uuid::parse_str(nonce).is_err()
    {
        return Ok(AckResult::Stale);
    }
    let pending = store.get_json_preference(key)?;
    if pending
        .as_ref()
        .and_then(|value| value.get("nonce"))
        .and_then(Value::as_str)
        != Some(nonce)
    {
        return Ok(AckResult::Stale);
    }
    if receipt_session_id == current_session_id {
        return Ok(AckResult::Deferred);
    }
    let cleared = store.clear_json_preference_if_field_matches(key, "nonce", nonce)?;
    Ok(if cleared {
        AckResult::Acknowledged
    } else {
        AckResult::Stale
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn session() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn legacy_map_migrates_with_cas_and_keeps_session_identity_ephemeral() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("burrata.sqlite");
        let store = Store::open(&path).unwrap();
        let session_id = session();
        store
            .set_json_preference(
                PENDING_KEY,
                &serde_json::json!({"burrata.waveform-enabled.v1":true}),
            )
            .unwrap();

        let response = read_or_migrate(&store, &session_id).unwrap().unwrap();
        assert_eq!(response.current_session_id, session_id);
        assert_eq!(response.preferences["burrata.waveform-enabled.v1"], true);
        assert!(uuid::Uuid::parse_str(&response.nonce).is_ok());
        let persisted = store.get_json_preference(PENDING_KEY).unwrap().unwrap();
        assert_eq!(persisted["schemaVersion"], SCHEMA_VERSION);
        assert_eq!(persisted["nonce"], response.nonce);
        assert_eq!(
            persisted["preferences"]["burrata.waveform-enabled.v1"],
            true
        );
        assert!(persisted.get("currentSessionId").is_none());
    }

    #[test]
    fn empty_cleared_value_and_empty_legacy_map_do_not_mint_a_pending_nonce() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("burrata.sqlite")).unwrap();
        let current_session_id = session();
        for cleared in [
            serde_json::json!({}),
            serde_json::json!({"unknown.preference": true}),
        ] {
            store.set_json_preference(PENDING_KEY, &cleared).unwrap();
            assert!(read_or_migrate(&store, &current_session_id)
                .unwrap()
                .is_none());
            assert_eq!(
                store.get_json_preference(PENDING_KEY).unwrap().unwrap(),
                cleared
            );
        }
    }

    #[test]
    fn acknowledgement_defers_same_session_and_never_clears_a_newer_import() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("burrata.sqlite")).unwrap();
        let session_one = session();
        let session_two = session();
        let session_three = session();
        let first = PendingEnvelope::new(BTreeMap::from([(
            "burrata.waveform-enabled.v1".into(),
            Value::Bool(true),
        )]));
        store
            .set_json_preference(PENDING_KEY, &serde_json::to_value(&first).unwrap())
            .unwrap();
        assert_eq!(
            acknowledge(&store, &session_one, &first.nonce, &session_one).unwrap(),
            AckResult::Deferred
        );

        let newer = PendingEnvelope::new(BTreeMap::from([(
            "burrata.waveform-enabled.v1".into(),
            Value::Bool(false),
        )]));
        store
            .set_json_preference(PENDING_KEY, &serde_json::to_value(&newer).unwrap())
            .unwrap();
        assert_eq!(
            acknowledge(&store, &session_two, &first.nonce, &session_one).unwrap(),
            AckResult::Stale
        );
        assert_eq!(
            acknowledge(&store, &session_two, &first.nonce, &session_two).unwrap(),
            AckResult::Stale
        );
        assert_eq!(
            store.get_json_preference(PENDING_KEY).unwrap().unwrap()["nonce"],
            newer.nonce
        );
        assert_eq!(
            acknowledge(&store, &session_two, &newer.nonce, &session_two).unwrap(),
            AckResult::Deferred
        );
        assert_eq!(
            acknowledge(&store, &session_three, &newer.nonce, &session_two).unwrap(),
            AckResult::Acknowledged
        );
        assert_eq!(
            store.get_json_preference(PENDING_KEY).unwrap().unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn generic_nonce_ack_defers_same_session_and_clears_playback_handoff_later() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("burrata.sqlite")).unwrap();
        let key = "backupPlaybackHydration.v1";
        let current_session = session();
        let prior_session = session();
        let envelope = serde_json::json!({"version":1,"nonce":session(),"paused":true});
        let nonce = envelope["nonce"].as_str().unwrap().to_owned();
        store.set_json_preference(key, &envelope).unwrap();

        assert_eq!(
            acknowledge_key(&store, &current_session, key, &nonce, &current_session).unwrap(),
            AckResult::Deferred
        );
        assert_eq!(
            store.get_json_preference(key).unwrap().unwrap()["nonce"],
            nonce
        );
        let newer_nonce = session();
        store
            .set_json_preference(
                key,
                &serde_json::json!({"version":1,"nonce":newer_nonce,"paused":true}),
            )
            .unwrap();
        assert_eq!(
            acknowledge_key(&store, &current_session, key, &nonce, &prior_session).unwrap(),
            AckResult::Stale
        );
        assert_eq!(
            store.get_json_preference(key).unwrap().unwrap()["nonce"],
            newer_nonce
        );
        assert_eq!(
            acknowledge_key(&store, &current_session, key, &newer_nonce, &prior_session).unwrap(),
            AckResult::Acknowledged
        );
        assert_eq!(
            store.get_json_preference(key).unwrap().unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn migration_cas_does_not_replace_a_concurrent_newer_import() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("burrata.sqlite");
        let store = Store::open(&path).unwrap();
        let old_raw = serde_json::json!({"burrata.waveform-enabled.v1":true}).to_string();
        store
            .set_json_preference(PENDING_KEY, &serde_json::from_str(&old_raw).unwrap())
            .unwrap();
        let newer = PendingEnvelope::new(BTreeMap::from([(
            "burrata.waveform-enabled.v1".into(),
            Value::Bool(false),
        )]));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key=?2",
                rusqlite::params![serde_json::to_string(&newer).unwrap(), PENDING_KEY],
            )
            .unwrap();
        let stale_migration = PendingEnvelope::new(BTreeMap::from([(
            "burrata.waveform-enabled.v1".into(),
            Value::Bool(true),
        )]));
        let changed = connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key=?2 AND value=?3",
                rusqlite::params![
                    serde_json::to_string(&stale_migration).unwrap(),
                    PENDING_KEY,
                    old_raw
                ],
            )
            .unwrap();
        assert_eq!(changed, 0);
        assert_eq!(
            store.get_json_preference(PENDING_KEY).unwrap().unwrap()["nonce"],
            newer.nonce
        );
    }

    #[test]
    fn corrupt_envelope_metadata_is_repaired_without_persisting_session_id() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("burrata.sqlite")).unwrap();
        let current_session_id = session();
        store
            .set_json_preference(
                PENDING_KEY,
                &serde_json::json!({
                    "schemaVersion": 99,
                    "nonce": "invalid",
                    "preferences": {"burrata.waveform-enabled.v1": true}
                }),
            )
            .unwrap();
        let response = read_or_migrate(&store, &current_session_id)
            .unwrap()
            .unwrap();
        assert_eq!(response.preferences["burrata.waveform-enabled.v1"], true);
        assert_eq!(response.current_session_id, current_session_id);
        let persisted = store.get_json_preference(PENDING_KEY).unwrap().unwrap();
        assert_eq!(persisted["schemaVersion"], SCHEMA_VERSION);
        assert!(uuid::Uuid::parse_str(persisted["nonce"].as_str().unwrap()).is_ok());
        assert!(persisted.get("currentSessionId").is_none());
    }
}
