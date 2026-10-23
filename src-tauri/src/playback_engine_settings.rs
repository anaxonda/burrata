//! Playback engine selection is persisted for the next app launch only.
use crate::storage::Store;
use serde::{Deserialize, Serialize};
const KEY: &str = "playbackEngine.v1";
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Webview,
    Native,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSettings {
    pub selected: Engine,
    pub active: Engine,
    pub locked: bool,
    pub restart_required: bool,
}
pub fn status(store: &Store, native_active: bool, locked: bool) -> Result<EngineSettings, String> {
    let active = if native_active {
        Engine::Native
    } else {
        Engine::Webview
    };
    let saved = store
        .get_json_preference(KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or(Engine::Webview);
    let selected = if locked { active } else { saved };
    Ok(EngineSettings {
        selected,
        active,
        locked,
        restart_required: !locked && selected != active,
    })
}
pub fn save(
    store: &Store,
    engine: Engine,
    native_active: bool,
    locked: bool,
) -> Result<EngineSettings, String> {
    if locked {
        return Err(
            "This launch uses a fixed audio engine. Start Burrata normally to change it.".into(),
        );
    }
    store.set_json_preference(
        KEY,
        &serde_json::to_value(engine).map_err(|_| "Invalid audio engine")?,
    )?;
    status(store, native_active, locked)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saving_engine_preserves_other_preferences_and_requires_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.sqlite")).unwrap();
        store
            .set_json_preference("other", &serde_json::json!({"retained":true}))
            .unwrap();
        assert_eq!(
            status(&store, false, false).unwrap().selected,
            Engine::Webview
        );
        let saved = save(&store, Engine::Native, false, false).unwrap();
        assert_eq!(saved.active, Engine::Webview);
        assert_eq!(saved.selected, Engine::Native);
        assert!(saved.restart_required);
        assert!(!status(&store, true, false).unwrap().restart_required);
        assert_eq!(
            store.get_json_preference("other").unwrap(),
            Some(serde_json::json!({"retained":true}))
        );
    }
    #[test]
    fn fixed_launch_is_read_only_and_invalid_preferences_do_not_activate_native() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.sqlite")).unwrap();
        assert!(save(&store, Engine::Webview, true, true).is_err());
        let state = status(&store, true, true).unwrap();
        assert!(state.locked);
        assert_eq!(state.selected, Engine::Native);
        assert!(!state.restart_required);
        assert_eq!(store.get_json_preference(KEY).unwrap(), None);
        store
            .set_json_preference(KEY, &serde_json::json!("unsupported"))
            .unwrap();
        assert_eq!(
            status(&store, false, false).unwrap().selected,
            Engine::Webview
        );
        assert!(serde_json::from_value::<Engine>(serde_json::json!("unsupported")).is_err());
    }
}
