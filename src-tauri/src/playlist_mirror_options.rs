//! Persisted per-collection settings for managed playlist mirrors.

use crate::storage::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const KEY: &str = "playlistMirrorOptions.v1";
const MAX_PLAYLISTS: usize = 10_002;

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PlaylistMirrorOptions {
    /// Whether Burrata publishes and reconciles the managed mirror for this playlist.
    pub included: bool,
    /// `None` preserves the existing output behavior for this playlist.
    pub include_media_links: Option<bool>,
    /// `None` preserves the existing output behavior for this playlist.
    pub include_ytdlp_search: Option<bool>,
}

impl Default for PlaylistMirrorOptions {
    fn default() -> Self {
        Self {
            included: true,
            include_media_links: None,
            include_ytdlp_search: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PlaylistMirrorPolicy {
    pub schema_version: u32,
    pub playlists: HashMap<String, PlaylistMirrorOptions>,
}

impl Default for PlaylistMirrorPolicy {
    fn default() -> Self {
        Self {
            schema_version: 1,
            playlists: HashMap::new(),
        }
    }
}

/// A per-playlist view with `None` resolved to that playlist's pre-feature behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistMirrorEffectiveOptions {
    pub collection_id: String,
    pub included: bool,
    pub include_media_links: bool,
    pub include_ytdlp_search: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistMirrorOptionDefaults {
    pub include_media_links: bool,
    pub include_ytdlp_search: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistMirrorOptionsView {
    pub collection_id: String,
    pub options: PlaylistMirrorOptions,
    pub effective: PlaylistMirrorEffectiveOptions,
    pub defaults: PlaylistMirrorOptionDefaults,
    pub globally_enabled: bool,
}

pub fn get_playlist_mirror_policy(store: &Store) -> Result<PlaylistMirrorPolicy> {
    let Some(value) = store
        .get_json_preference(KEY)
        .map_err(|error| error.to_string())?
    else {
        return Ok(PlaylistMirrorPolicy::default());
    };
    let policy: PlaylistMirrorPolicy = serde_json::from_value(value)
        .map_err(|error| format!("Saved playlist mirror options are invalid: {error}"))?;
    validate_policy(&policy)?;
    Ok(policy)
}

pub fn save_playlist_mirror_policy(
    store: &Store,
    policy: &PlaylistMirrorPolicy,
) -> Result<PlaylistMirrorPolicy> {
    let _library_write = store
        .try_library_write()
        .map_err(|error| error.to_string())?;
    validate_policy(policy)?;
    let mut policy = policy.clone();
    policy.schema_version = 1;
    store
        .set_json_preference(
            KEY,
            &serde_json::to_value(&policy).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    Ok(policy)
}

/// Return the effective options for one supported managed collection.
pub fn effective_playlist_mirror_options(
    store: &Store,
    collection_id: &str,
) -> Result<PlaylistMirrorEffectiveOptions> {
    validate_collection_id(store, collection_id)?;
    let options = get_playlist_mirror_policy(store)?
        .playlists
        .remove(collection_id)
        .unwrap_or_default();
    let defaults = defaults_for_collection(store, collection_id)?;
    // Before these controls existed, normal/favorites mirrors included trusted
    // local paths and explicitly selected playable URLs. Listening History used
    // Burrata identity placeholders and did not include search fallback links.
    Ok(PlaylistMirrorEffectiveOptions {
        collection_id: collection_id.to_owned(),
        included: options.included,
        include_media_links: options
            .include_media_links
            .unwrap_or(defaults.include_media_links),
        include_ytdlp_search: options
            .include_ytdlp_search
            .unwrap_or(defaults.include_ytdlp_search),
    })
}

pub fn get_playlist_mirror_options(
    store: &Store,
    collection_id: &str,
) -> Result<PlaylistMirrorOptionsView> {
    validate_collection_id(store, collection_id)?;
    let options = get_playlist_mirror_policy(store)?
        .playlists
        .remove(collection_id)
        .unwrap_or_default();
    let defaults = defaults_for_collection(store, collection_id)?;
    let effective = effective_from_options(collection_id, &options, &defaults);
    let settings = store.settings().map_err(|error| error.to_string())?;
    Ok(PlaylistMirrorOptionsView {
        collection_id: collection_id.to_owned(),
        options,
        effective,
        defaults,
        globally_enabled: settings.playlist_mirroring_enabled,
    })
}

/// Set one playlist's policy while preserving settings for every other playlist.
pub fn set_playlist_mirror_options(
    store: &Store,
    collection_id: &str,
    options: PlaylistMirrorOptions,
) -> Result<PlaylistMirrorOptionsView> {
    validate_collection_id(store, collection_id)?;
    let mut policy = get_playlist_mirror_policy(store)?;
    policy.playlists.insert(collection_id.to_owned(), options);
    save_playlist_mirror_policy(store, &policy)?;
    get_playlist_mirror_options(store, collection_id)
}

fn effective_from_options(
    collection_id: &str,
    options: &PlaylistMirrorOptions,
    defaults: &PlaylistMirrorOptionDefaults,
) -> PlaylistMirrorEffectiveOptions {
    PlaylistMirrorEffectiveOptions {
        collection_id: collection_id.to_owned(),
        included: options.included,
        include_media_links: options
            .include_media_links
            .unwrap_or(defaults.include_media_links),
        include_ytdlp_search: options
            .include_ytdlp_search
            .unwrap_or(defaults.include_ytdlp_search),
    }
}

fn defaults_for_collection(
    store: &Store,
    collection_id: &str,
) -> Result<PlaylistMirrorOptionDefaults> {
    // Before these controls existed, normal/favorites mirrors included trusted
    // local paths and explicitly selected playable URLs. Listening History used
    // Burrata identity placeholders and did not include search fallback links.
    Ok(PlaylistMirrorOptionDefaults {
        include_media_links: collection_id != "history",
        include_ytdlp_search: collection_id != "history"
            && crate::playlist_sync::legacy_mirror_has_search_fallback(store, collection_id)?,
    })
}

fn validate_policy(policy: &PlaylistMirrorPolicy) -> Result<()> {
    if policy.schema_version != 0 && policy.schema_version != 1 {
        return Err("Unsupported playlist mirror options version".into());
    }
    if policy.playlists.len() > MAX_PLAYLISTS {
        return Err("Playlist mirror options exceed the supported limit".into());
    }
    for collection_id in policy.playlists.keys() {
        if collection_id.trim().is_empty() || collection_id.len() > 128 {
            return Err("Playlist mirror options contain an invalid playlist ID".into());
        }
    }
    Ok(())
}

fn validate_collection_id(store: &Store, collection_id: &str) -> Result<()> {
    if matches!(collection_id, "favorites" | "history")
        || store
            .list_collections()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|collection| collection.id == collection_id)
    {
        Ok(())
    } else {
        Err("Choose an existing playlist, Favorites, or Listening History".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_legacy_behavior_for_each_mirror_kind() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        assert_eq!(
            get_playlist_mirror_policy(&store).unwrap(),
            PlaylistMirrorPolicy::default()
        );
        assert_eq!(
            effective_playlist_mirror_options(&store, "favorites").unwrap(),
            PlaylistMirrorEffectiveOptions {
                collection_id: "favorites".into(),
                included: true,
                include_media_links: true,
                include_ytdlp_search: false,
            }
        );
        assert_eq!(
            effective_playlist_mirror_options(&store, "history").unwrap(),
            PlaylistMirrorEffectiveOptions {
                collection_id: "history".into(),
                included: true,
                include_media_links: false,
                include_ytdlp_search: false,
            }
        );
    }

    #[test]
    fn per_playlist_options_roundtrip_and_reset_without_changing_other_playlists() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite")).unwrap();
        let collection = store.create_collection("Road Trip").unwrap();
        let mut settings = store.settings().unwrap();
        settings.playlist_mirroring_enabled = false;
        store.save_settings(&settings).unwrap();
        let effective = set_playlist_mirror_options(
            &store,
            &collection.id,
            PlaylistMirrorOptions {
                included: false,
                include_media_links: Some(false),
                include_ytdlp_search: Some(true),
            },
        )
        .unwrap();
        assert!(!effective.effective.included);
        assert!(!effective.effective.include_media_links);
        assert!(effective.effective.include_ytdlp_search);
        assert!(!effective.globally_enabled);
        set_playlist_mirror_options(
            &store,
            "favorites",
            PlaylistMirrorOptions {
                included: true,
                include_media_links: Some(false),
                include_ytdlp_search: None,
            },
        )
        .unwrap();
        let stored = get_playlist_mirror_policy(&store).unwrap();
        assert_eq!(stored.playlists.len(), 2);
        assert_eq!(
            stored.playlists[&collection.id].include_ytdlp_search,
            Some(true)
        );
        assert_eq!(
            stored.playlists["favorites"].include_media_links,
            Some(false)
        );
        let reset =
            set_playlist_mirror_options(&store, &collection.id, PlaylistMirrorOptions::default())
                .unwrap();
        assert_eq!(reset.effective.include_media_links, true);
        assert_eq!(reset.effective.include_ytdlp_search, false);
        assert_eq!(
            get_playlist_mirror_policy(&store).unwrap().playlists["favorites"].include_media_links,
            Some(false)
        );
    }
}
