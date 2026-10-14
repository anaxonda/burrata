//! User-defined playlist sections. Moving a playlist changes organization only.
use crate::storage::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
const KEY: &str = "sidebarLayout.v1";
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SidebarLayout {
    pub sections: Vec<PlaylistSection>,
    pub assignments: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistSection {
    pub id: String,
    pub name: String,
}
pub fn load(store: &Store) -> Result<SidebarLayout, String> {
    store
        .get_json_preference(KEY)?
        .map(serde_json::from_value)
        .transpose()
        .map(|v| v.unwrap_or_default())
        .map_err(|_| "Saved playlist sections are invalid".into())
}
pub fn save(store: &Store, mut layout: SidebarLayout) -> Result<SidebarLayout, String> {
    if layout.sections.len() > 100 || layout.assignments.len() > 10000 {
        return Err("Too many playlist sections or assignments".into());
    }
    if layout.names.len() > 10000 {
        return Err("Too many playlist names".into());
    }
    for (id, name) in &mut layout.names {
        *name = name.trim().to_owned();
        if id.is_empty()
            || id.len() > 200
            || id.chars().any(char::is_control)
            || name.is_empty()
            || name.chars().count() > 160
            || name.chars().any(char::is_control)
        {
            return Err("Playlist names must contain 1–160 printable characters".into());
        }
    }
    let mut ids = HashSet::new();
    for section in &mut layout.sections {
        section.name = section.name.trim().to_owned();
        if section.id.is_empty()
            || section.id.len() > 100
            || !section
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(section.id.clone())
        {
            return Err("Playlist section IDs must be unique and valid".into());
        }
        if section.name.is_empty()
            || section.name.chars().count() > 80
            || section.name.chars().any(char::is_control)
        {
            return Err("Section names must contain 1–80 printable characters".into());
        }
    }
    if layout.assignments.iter().any(|(playlist, section)| {
        playlist.is_empty()
            || playlist.len() > 200
            || playlist.chars().any(char::is_control)
            || !ids.contains(section)
    }) {
        return Err("Playlist assignment refers to an invalid section".into());
    }
    store.set_json_preference(
        KEY,
        &serde_json::to_value(&layout).map_err(|_| "Cannot save playlist sections")?,
    )?;
    Ok(layout)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn section_changes_preserve_playlists_and_persist() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let playlist = store.create_collection("Afternoon").unwrap();
        let layout = SidebarLayout {
            sections: vec![PlaylistSection {
                id: "favorites-folder".into(),
                name: " DJ sets ".into(),
            }],
            assignments: BTreeMap::from([(playlist.id.clone(), "favorites-folder".into())]),
            names: BTreeMap::from([("favorites".into(), " Favorites ".into())]),
        };
        let saved = save(&store, layout).unwrap();
        assert_eq!(saved.sections[0].name, "DJ sets");
        assert_eq!(load(&store).unwrap().names["favorites"], "Favorites");
        assert_eq!(load(&store).unwrap().assignments, saved.assignments);
        assert!(store
            .list_collections()
            .unwrap()
            .iter()
            .any(|p| p.id == playlist.id));
        assert!(save(
            &store,
            SidebarLayout {
                sections: vec![],
                names: saved.names,
                assignments: saved.assignments
            }
        )
        .is_err());
    }
}
