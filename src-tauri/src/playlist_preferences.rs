//! Persisted playlist targets and keyboard shortcut assignments.
use crate::{models::Track, storage::Store};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const KEY: &str = "playlistPreferences.v1";
const SHORTCUT_COUNT: usize = 9;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PlaylistPreferences {
    pub favorites_playlist_id: String,
    pub shortcuts: Vec<Option<String>>,
    #[serde(default)]
    pub shortcut_bindings: Vec<String>,
}

impl Default for PlaylistPreferences {
    fn default() -> Self {
        Self {
            favorites_playlist_id: String::new(),
            shortcuts: vec![None; SHORTCUT_COUNT],
            shortcut_bindings: (1..=SHORTCUT_COUNT).map(|n| format!("Alt+{n}")).collect(),
        }
    }
}

/// Loads settings and performs the one-time migration from the legacy liked flag.
pub fn load(store: &Store) -> Result<PlaylistPreferences, String> {
    let collections = store.list_collections().map_err(|e| e.to_string())?;
    let mut saved: PlaylistPreferences = store
        .get_json_preference(KEY)
        .map_err(|e| e.to_string())?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| "Saved playlist preferences are invalid".to_owned())?
        .unwrap_or_default();
    saved.shortcuts.resize(SHORTCUT_COUNT, None);
    saved.shortcuts.truncate(SHORTCUT_COUNT);
    if saved.shortcut_bindings.len() != SHORTCUT_COUNT {
        saved.shortcut_bindings = PlaylistPreferences::default().shortcut_bindings;
    }
    for target in &mut saved.shortcuts {
        if target.as_ref().is_some_and(|id| {
            !collections.iter().any(|c| &c.id == id)
                || store.is_browser_source_collection(id).unwrap_or(true)
        }) {
            *target = None;
        }
    }

    if saved.favorites_playlist_id.is_empty() {
        let legacy_name = crate::sidebar_layout::load(store)
            .ok()
            .and_then(|layout| layout.names.get("favorites").cloned())
            .unwrap_or_else(|| "Liked Songs".into());
        let collection = store
            .create_collection(&legacy_name)
            .map_err(|e| e.to_string())?;
        // The first read migrates every previously liked recording by its stable listing ID.
        let liked = store
            .list_tracks("favorites", None)
            .map_err(|e| e.to_string())?;
        store
            .add_to_collection(&collection.id, &liked)
            .map_err(|e| e.to_string())?;
        saved.favorites_playlist_id = collection.id;
    }
    store
        .set_json_preference(
            KEY,
            &serde_json::to_value(&saved).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    Ok(saved)
}

pub fn set_shortcut(
    store: &Store,
    slot: usize,
    collection_id: Option<String>,
) -> Result<PlaylistPreferences, String> {
    if slot >= SHORTCUT_COUNT {
        return Err("Shortcut slot must be between 1 and 9".into());
    }
    let collections = store.list_collections().map_err(|e| e.to_string())?;
    if collection_id.as_ref().is_some_and(|id| {
        !collections.iter().any(|c| &c.id == id)
            || store.is_browser_source_collection(id).unwrap_or(true)
    }) {
        return Err("Choose an active playlist for this shortcut".into());
    }
    let mut preferences = load(store)?;
    if collection_id.as_ref().is_some_and(|id| {
        preferences
            .shortcuts
            .iter()
            .enumerate()
            .any(|(index, assigned)| index != slot && assigned.as_ref() == Some(id))
    }) {
        return Err("That playlist is already assigned to another shortcut".into());
    }
    preferences.shortcuts[slot] = collection_id;
    store
        .set_json_preference(
            KEY,
            &serde_json::to_value(&preferences).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    Ok(preferences)
}

/// Selects an existing mutable playlist without moving its contents or the old target's contents.
pub fn set_favorites_playlist(
    store: &Store,
    collection_id: &str,
) -> Result<PlaylistPreferences, String> {
    let active = store.list_collections().map_err(|e| e.to_string())?;
    if !active
        .iter()
        .any(|collection| collection.id == collection_id)
    {
        return Err("Choose an active playlist for Favorites".into());
    }
    if store
        .is_browser_source_collection(collection_id)
        .map_err(|e| e.to_string())?
    {
        return Err(
            "Imported snapshots are read-only; choose a regular playlist for Favorites".into(),
        );
    }
    let mut preferences = load(store)?;
    preferences.favorites_playlist_id = collection_id.to_owned();
    let result = save(store, &preferences)?;
    resync_favorite_flags(store)?;
    Ok(result)
}

pub fn set_shortcut_binding(
    store: &Store,
    slot: usize,
    binding: &str,
) -> Result<PlaylistPreferences, String> {
    if slot >= SHORTCUT_COUNT {
        return Err("Shortcut slot must be between 1 and 9".into());
    }
    let normalized = normalize_binding(binding)?;
    let mut preferences = load(store)?;
    if preferences
        .shortcut_bindings
        .iter()
        .enumerate()
        .any(|(index, existing)| index != slot && existing.eq_ignore_ascii_case(&normalized))
    {
        return Err("That key combination is already assigned".into());
    }
    preferences.shortcut_bindings[slot] = normalized;
    save(store, &preferences)
}

fn normalize_binding(binding: &str) -> Result<String, String> {
    let mut parts = binding
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let mut modifiers = Vec::new();
    let mut key = None;
    for part in parts.by_ref() {
        let normalized = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl".to_owned(),
            "alt" | "option" => "Alt".to_owned(),
            "shift" => "Shift".to_owned(),
            "meta" | "cmd" | "command" | "super" => "Meta".to_owned(),
            _ => {
                if key.is_some() {
                    return Err("Use one key with Ctrl, Alt, Shift, or Meta modifiers".into());
                }
                let candidate = part.to_ascii_uppercase();
                if candidate.len() == 1 && candidate.as_bytes()[0].is_ascii_alphanumeric()
                    || matches!(
                        candidate.as_str(),
                        "F1" | "F2"
                            | "F3"
                            | "F4"
                            | "F5"
                            | "F6"
                            | "F7"
                            | "F8"
                            | "F9"
                            | "F10"
                            | "F11"
                            | "F12"
                    )
                {
                    key = Some(candidate);
                    continue;
                }
                return Err("Use a letter, number, or F1–F12 key".into());
            }
        };
        if modifiers.contains(&normalized) {
            return Err("Duplicate key modifier".into());
        }
        modifiers.push(normalized);
    }
    let key = key.ok_or("Include a key after the modifiers")?;
    if modifiers.is_empty() {
        return Err("Shortcuts must include Ctrl, Alt, Shift, or Meta".into());
    }
    modifiers.sort_by_key(|modifier| match modifier.as_str() {
        "Ctrl" => 0,
        "Alt" => 1,
        "Shift" => 2,
        _ => 3,
    });
    modifiers.push(key);
    Ok(modifiers.join("+"))
}

fn save(store: &Store, preferences: &PlaylistPreferences) -> Result<PlaylistPreferences, String> {
    store
        .set_json_preference(
            KEY,
            &serde_json::to_value(preferences).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    Ok(preferences.clone())
}

/// Recompute legacy favorite flags after changing the configured target or editing its membership.
pub fn resync_favorite_flags(store: &Store) -> Result<(), String> {
    let preferences = load(store)?;
    let members = store
        .list_tracks("collection", Some(&preferences.favorites_playlist_id))?
        .into_iter()
        .map(|track| track.id)
        .collect::<HashSet<_>>();
    let library = store
        .list_tracks("library", None)
        .map_err(|e| e.to_string())?;
    for track in library {
        store
            .set_favorite(&track, members.contains(&track.id))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// No-op for ordinary playlists; use after any collection membership mutation, including MCP.
pub fn resync_favorite_flags_after_collection_change(
    store: &Store,
    collection_id: &str,
) -> Result<(), String> {
    if load(store)?.favorites_playlist_id == collection_id {
        resync_favorite_flags(store)?;
    }
    Ok(())
}

/// Keeps the legacy liked flag and the editable Favorites playlist in sync.
/// Collection insertion and deletion are repeat-safe, so retries cannot duplicate entries.
pub fn set_favorite_membership(store: &Store, track: &Track, liked: bool) -> Result<(), String> {
    let preferences = load(store)?;
    if liked {
        store
            .add_to_collection(
                &preferences.favorites_playlist_id,
                std::slice::from_ref(track),
            )
            .map_err(|e| e.to_string())?;
    } else {
        store
            .remove_from_collection(&preferences.favorites_playlist_id, &[track.id.clone()])
            .map_err(|e| e.to_string())?;
    }
    store.set_favorite(track, liked).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_bridge::BrowserSnapshot;
    use crate::models::Track;

    fn track(id: &str) -> Track {
        Track {
            id: format!("test:{id}"),
            provider: "test".into(),
            provider_id: id.into(),
            source_url: String::new(),
            artist: "Artist".into(),
            title: id.into(),
            favorite: true,
            ..Track::default()
        }
    }

    #[test]
    fn first_load_migrates_liked_tracks_and_shortcuts_are_validated_and_persisted() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let liked = track("liked-1");
        store.save_track(&liked).unwrap();
        store.set_favorite(&liked, true).unwrap();
        let playlist = store.create_collection("Shortcut").unwrap();
        let preferences = load(&store).unwrap();
        let migrated = store
            .list_tracks("collection", Some(&preferences.favorites_playlist_id))
            .unwrap();
        assert_eq!(
            migrated.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["test:liked-1"]
        );
        let saved = set_shortcut(&store, 8, Some(playlist.id.clone())).unwrap();
        assert_eq!(saved.shortcuts.len(), 9);
        assert_eq!(
            load(&store).unwrap().shortcuts[8].as_deref(),
            Some(playlist.id.as_str())
        );
        assert!(set_shortcut(&store, 9, None).is_err());
        assert!(set_shortcut(&store, 7, Some(playlist.id.clone())).is_err());
        store.delete_collection(&playlist.id).unwrap();
        assert!(load(&store).unwrap().shortcuts[8].is_none());
    }

    #[test]
    fn favorite_membership_toggles_idempotently_and_deleted_target_remains_safe() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let track = track("favorite-2");
        store.save_track(&track).unwrap();
        let id = load(&store).unwrap().favorites_playlist_id;
        set_favorite_membership(&store, &track, true).unwrap();
        set_favorite_membership(&store, &track, true).unwrap();
        assert_eq!(store.list_tracks("collection", Some(&id)).unwrap().len(), 1);
        store.delete_collection(&id).unwrap();
        assert_eq!(load(&store).unwrap().favorites_playlist_id, id);
        assert!(set_favorite_membership(&store, &track, false).is_err());
        let fallback = store.create_collection("Fresh Favorites").unwrap();
        set_favorites_playlist(&store, &fallback.id).unwrap();
        assert!(!store.get_track(&track.id).unwrap().unwrap().favorite);
        assert_eq!(
            store
                .list_tracks("collection", Some(&id))
                .unwrap_err()
                .to_string(),
            "Collection not found or deleted"
        );
    }

    #[test]
    fn changing_favorites_resyncs_flags_and_manual_membership_edits() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let old_favorite = track("old");
        let new_favorite = track("new");
        store.save_track(&old_favorite).unwrap();
        store.save_track(&new_favorite).unwrap();
        store.set_favorite(&old_favorite, true).unwrap();
        let old_target = load(&store).unwrap().favorites_playlist_id;
        let new_target = store.create_collection("New Favorites").unwrap();
        store
            .add_to_collection(&new_target.id, std::slice::from_ref(&new_favorite))
            .unwrap();

        set_favorites_playlist(&store, &new_target.id).unwrap();
        assert!(!store.get_track(&old_favorite.id).unwrap().unwrap().favorite);
        assert!(store.get_track(&new_favorite.id).unwrap().unwrap().favorite);
        assert_eq!(
            store
                .list_tracks("collection", Some(&old_target))
                .unwrap()
                .len(),
            1
        );

        store
            .add_to_collection(&new_target.id, std::slice::from_ref(&old_favorite))
            .unwrap();
        resync_favorite_flags_after_collection_change(&store, &new_target.id).unwrap();
        assert!(store.get_track(&old_favorite.id).unwrap().unwrap().favorite);
        store
            .remove_from_collection(&new_target.id, &[new_favorite.id.clone()])
            .unwrap();
        resync_favorite_flags_after_collection_change(&store, &new_target.id).unwrap();
        assert!(!store.get_track(&new_favorite.id).unwrap().unwrap().favorite);
        assert!(store.get_track(&old_favorite.id).unwrap().unwrap().favorite);
    }

    fn import_browser_playlist(store: &Store) -> String {
        let snapshot: BrowserSnapshot = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "provider": "spotify",
            "kind": "playlist",
            "title": "Imported snapshot",
            "sourceUrl": "https://open.spotify.com/playlist/1234567890123456789012",
            "capturedAt": "2026-10-14T12:00:00Z",
            "complete": true,
            "expectedTotal": 1,
            "warnings": [],
            "entries": [{"providerId":"1234567890123456789012","artist":"Artist","title":"Song","position":1,"rank":1}]
        })).unwrap();
        let chart = crate::browser_bridge::validate_snapshot(&snapshot).unwrap();
        store
            .import_browser_chart(&chart, &snapshot)
            .unwrap()
            .collection_id
    }

    #[test]
    fn imported_snapshot_collections_cannot_be_favorites_or_shortcuts() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let imported = import_browser_playlist(&store);
        assert!(set_favorites_playlist(&store, &imported)
            .unwrap_err()
            .contains("read-only"));
        assert!(set_shortcut(&store, 0, Some(imported)).is_err());
    }

    #[test]
    fn shortcut_bindings_normalize_and_reject_collisions() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let defaults = load(&store).unwrap();
        assert_eq!(
            defaults.shortcut_bindings,
            (1..=9).map(|n| format!("Alt+{n}")).collect::<Vec<_>>()
        );
        assert_eq!(normalize_binding("shift+ctrl+k").unwrap(), "Ctrl+Shift+K");
        let saved = set_shortcut_binding(&store, 0, "ctrl+shift+k").unwrap();
        assert_eq!(saved.shortcut_bindings[0], "Ctrl+Shift+K");
        assert!(set_shortcut_binding(&store, 1, "CTRL+SHIFT+K").is_err());
        assert!(set_shortcut_binding(&store, 1, "K").is_err());
    }
}
