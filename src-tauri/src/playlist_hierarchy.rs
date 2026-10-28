//! Persisted playlist folders, section definitions, and occurrence annotations.
//!
//! Ordered entry identity/order remains in SQLite's ordered playlist sidecar.
//! This preference stores user-facing hierarchy and annotations keyed by those
//! stable occurrence IDs.

use crate::{
    models::Track,
    portable_playlist::{NamedItem, PlaybackRange, PortableEntry, PortableTrack},
    storage::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const KEY: &str = "playlistHierarchy.v1";
const MAX_FOLDERS: usize = 2_000;
const MAX_PLAYLISTS: usize = 10_000;
const MAX_SECTIONS: usize = 256;

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistHierarchy {
    pub schema_version: u32,
    #[serde(default)]
    pub folders: Vec<PlaylistFolder>,
    #[serde(default)]
    pub playlists: Vec<PlaylistPlacement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    pub position: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPlacement {
    pub collection_id: String,
    #[serde(default)]
    pub folder_id: Option<String>,
    pub position: u32,
    #[serde(default)]
    pub sections: Vec<PlaylistSection>,
    #[serde(default)]
    pub occurrences: Vec<OccurrenceMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSection {
    pub id: String,
    pub name: String,
    pub position: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OccurrenceMetadata {
    pub id: String,
    #[serde(default)]
    pub section_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub cue_points: Vec<f64>,
    #[serde(default)]
    pub playback_range: Option<PlaybackRange>,
}

pub fn load(store: &Store) -> Result<PlaylistHierarchy> {
    let Some(value) = store.get_json_preference(KEY)? else {
        return Ok(PlaylistHierarchy {
            schema_version: 1,
            ..PlaylistHierarchy::default()
        });
    };
    let hierarchy: PlaylistHierarchy = serde_json::from_value(value)
        .map_err(|error| format!("Saved playlist hierarchy is invalid: {error}"))?;
    validate(&hierarchy)?;
    Ok(hierarchy)
}

pub fn save(store: &Store, hierarchy: &PlaylistHierarchy) -> Result<PlaylistHierarchy> {
    validate(hierarchy)?;
    let mut value = hierarchy.clone();
    value.schema_version = 1;
    store.set_json_preference(
        KEY,
        &serde_json::to_value(&value).map_err(|error| error.to_string())?,
    )?;
    Ok(value)
}

pub(crate) fn validate(hierarchy: &PlaylistHierarchy) -> Result<()> {
    if hierarchy.schema_version != 0 && hierarchy.schema_version != 1 {
        return Err("Unsupported playlist hierarchy version".into());
    }
    if hierarchy.folders.len() > MAX_FOLDERS || hierarchy.playlists.len() > MAX_PLAYLISTS {
        return Err("Playlist hierarchy exceeds supported limits".into());
    }
    let folder_ids = hierarchy
        .folders
        .iter()
        .map(|folder| folder.id.as_str())
        .collect::<HashSet<_>>();
    if folder_ids.len() != hierarchy.folders.len() {
        return Err("Folder IDs must be unique".into());
    }
    for folder in &hierarchy.folders {
        if !is_uuid(&folder.id) || folder.name.trim().is_empty() || folder.name.len() > 512 {
            return Err("Playlist folder metadata is invalid".into());
        }
        if folder
            .parent_id
            .as_ref()
            .is_some_and(|parent| !folder_ids.contains(parent.as_str()) || parent == &folder.id)
        {
            return Err("Playlist folder parent is invalid".into());
        }
    }
    for folder in &hierarchy.folders {
        let mut cursor = folder.parent_id.as_deref();
        let mut visited = HashSet::new();
        while let Some(parent) = cursor {
            if !visited.insert(parent) || parent == folder.id {
                return Err("Playlist folder hierarchy contains a cycle".into());
            }
            cursor = hierarchy
                .folders
                .iter()
                .find(|candidate| candidate.id == parent)
                .and_then(|candidate| candidate.parent_id.as_deref());
        }
    }
    let playlist_ids = hierarchy
        .playlists
        .iter()
        .map(|playlist| playlist.collection_id.as_str())
        .collect::<HashSet<_>>();
    if playlist_ids.len() != hierarchy.playlists.len() {
        return Err("Playlist placements must be unique".into());
    }
    for playlist in &hierarchy.playlists {
        if playlist.collection_id.trim().is_empty()
            || playlist
                .folder_id
                .as_ref()
                .is_some_and(|id| !folder_ids.contains(id.as_str()))
            || playlist.sections.len() > MAX_SECTIONS
        {
            return Err("Playlist placement is invalid".into());
        }
        let section_ids = playlist
            .sections
            .iter()
            .map(|section| section.id.as_str())
            .collect::<HashSet<_>>();
        if section_ids.len() != playlist.sections.len()
            || playlist.sections.iter().any(|section| {
                !is_uuid(&section.id) || section.name.trim().is_empty() || section.name.len() > 512
            })
        {
            return Err("Playlist sections are invalid".into());
        }
        let occurrence_ids = playlist
            .occurrences
            .iter()
            .map(|occurrence| occurrence.id.as_str())
            .collect::<HashSet<_>>();
        if occurrence_ids.len() != playlist.occurrences.len()
            || playlist.occurrences.iter().any(|occurrence| {
                !is_uuid(&occurrence.id)
                    || occurrence
                        .section_id
                        .as_ref()
                        .is_some_and(|id| !section_ids.contains(id.as_str()))
            })
        {
            return Err("Playlist occurrence metadata is invalid".into());
        }
    }
    Ok(())
}

/// Add or move a playlist under explicit source-directory components. Components
/// are names relative to the configured playlist root; no filesystem path is saved.
pub fn set_folder_ancestry(
    store: &Store,
    collection_id: &str,
    names: &[String],
) -> Result<PlaylistHierarchy> {
    if names.len() > 32
        || names.iter().any(|name| {
            name.trim().is_empty() || name.len() > 512 || name.chars().any(char::is_control)
        })
    {
        return Err("Playlist folder ancestry is invalid".into());
    }
    let mut hierarchy = load(store)?;
    let mut parent: Option<String> = None;
    for (position, name) in names.iter().enumerate() {
        let existing = hierarchy
            .folders
            .iter()
            .find(|folder| folder.parent_id == parent && folder.name.eq_ignore_ascii_case(name));
        let id = if let Some(folder) = existing {
            folder.id.clone()
        } else {
            let folder = PlaylistFolder {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.clone(),
                parent_id: parent.clone(),
                position: position as u32,
            };
            let id = folder.id.clone();
            hierarchy.folders.push(folder);
            id
        };
        parent = Some(id);
    }
    let target = hierarchy
        .playlists
        .iter_mut()
        .find(|playlist| playlist.collection_id == collection_id);
    if let Some(placement) = target {
        placement.folder_id = parent;
    } else {
        let position = next_position(&hierarchy, parent.as_deref());
        hierarchy.playlists.push(PlaylistPlacement {
            collection_id: collection_id.to_owned(),
            folder_id: parent,
            position,
            ..PlaylistPlacement::default()
        });
    }
    save(store, &hierarchy)
}

/// Place the just-appended import occurrences under a folder path and optional section.
pub fn place_imported_items(
    store: &Store,
    collection_id: &str,
    folder_path: &str,
    section_name: &str,
    occurrence_count: usize,
) -> Result<PlaylistHierarchy> {
    if folder_path.len() > 2048 || section_name.len() > 512 || occurrence_count > 100_000 {
        return Err("Import placement exceeds supported limits".into());
    }
    let names = folder_path
        .split('/')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.len() > 32 || names.iter().any(|name| name.chars().any(char::is_control)) {
        return Err("Playlist folder path is invalid".into());
    }
    let mut hierarchy = load(store)?;
    let mut parent: Option<String> = None;
    for (position, name) in names.iter().enumerate() {
        let existing = hierarchy
            .folders
            .iter()
            .find(|folder| folder.parent_id == parent && folder.name.eq_ignore_ascii_case(name));
        let id = if let Some(folder) = existing {
            folder.id.clone()
        } else {
            let folder = PlaylistFolder {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.clone(),
                parent_id: parent.clone(),
                position: position as u32,
            };
            let id = folder.id.clone();
            hierarchy.folders.push(folder);
            id
        };
        parent = Some(id);
    }
    let placement = hierarchy
        .playlists
        .iter_mut()
        .find(|item| item.collection_id == collection_id)
        .ok_or("Destination playlist placement not found")?;
    if !folder_path.trim().is_empty() {
        placement.folder_id = parent;
    }
    let section_name = section_name.trim();
    let section_id = if section_name.is_empty() {
        None
    } else if let Some(section) = placement
        .sections
        .iter()
        .find(|item| item.name.eq_ignore_ascii_case(section_name))
    {
        Some(section.id.clone())
    } else {
        if placement.sections.len() >= MAX_SECTIONS {
            return Err("Playlist has reached its section limit".into());
        }
        let section = PlaylistSection {
            id: uuid::Uuid::new_v4().to_string(),
            name: section_name.to_owned(),
            position: placement.sections.len() as u32,
        };
        let id = section.id.clone();
        placement.sections.push(section);
        Some(id)
    };
    if occurrence_count > 0 {
        store.ensure_playlist_occurrence_ids(collection_id)?;
        let rows = store.list_playlist_occurrences(collection_id)?;
        if rows.len() < occurrence_count {
            return Err("Imported playlist occurrences could not be verified".into());
        }
        for row in rows.iter().rev().take(occurrence_count) {
            if let Some(existing) = placement
                .occurrences
                .iter_mut()
                .find(|item| item.id == row.occurrence_id)
            {
                existing.section_id = section_id.clone();
            } else {
                placement.occurrences.push(OccurrenceMetadata {
                    id: row.occurrence_id.clone(),
                    section_id: section_id.clone(),
                    ..OccurrenceMetadata::default()
                });
            }
        }
    }
    save(store, &hierarchy)
}

pub fn set_folder(
    store: &Store,
    collection_id: &str,
    folder_id: Option<&str>,
) -> Result<PlaylistHierarchy> {
    let mut hierarchy = load(store)?;
    if folder_id.is_some_and(|id| !hierarchy.folders.iter().any(|folder| folder.id == id)) {
        return Err("Playlist folder not found".into());
    }
    if let Some(placement) = hierarchy
        .playlists
        .iter_mut()
        .find(|playlist| playlist.collection_id == collection_id)
    {
        placement.folder_id = folder_id.map(str::to_owned);
    } else {
        let position = next_position(&hierarchy, folder_id);
        hierarchy.playlists.push(PlaylistPlacement {
            collection_id: collection_id.to_owned(),
            folder_id: folder_id.map(str::to_owned),
            position,
            ..PlaylistPlacement::default()
        });
    }
    save(store, &hierarchy)
}

/// Copy a playlist's folder/section layout and per-occurrence annotations to a
/// newly created personal copy. SQL occurrence IDs are regenerated for the copy;
/// section references are remapped to fresh stable identities.
pub fn copy_layout(store: &Store, source_id: &str, target_id: &str) -> Result<PlaylistHierarchy> {
    if source_id == target_id {
        return load(store);
    }
    let mut hierarchy = load(store)?;
    let Some(source) = hierarchy
        .playlists
        .iter()
        .find(|playlist| playlist.collection_id == source_id)
        .cloned()
    else {
        ensure_placement(&mut hierarchy, target_id);
        return save(store, &hierarchy);
    };
    let target_rows = store.list_playlist_occurrences(target_id)?;
    let mut sections = source.sections.clone();
    let mut section_map = HashMap::new();
    for section in &mut sections {
        let id = uuid::Uuid::new_v4().to_string();
        section_map.insert(section.id.clone(), id.clone());
        section.id = id;
    }
    let source_occurrences = source
        .occurrences
        .iter()
        .map(|occurrence| (occurrence.id.as_str(), occurrence))
        .collect::<HashMap<_, _>>();
    let source_rows = store.list_playlist_occurrences(source_id)?;
    let mut occurrences = Vec::with_capacity(target_rows.len());
    for (index, target) in target_rows.iter().enumerate() {
        let source_metadata = source_rows
            .get(index)
            .and_then(|row| source_occurrences.get(row.occurrence_id.as_str()).copied());
        let mut metadata = source_metadata.cloned().unwrap_or_default();
        metadata.id = target.occurrence_id.clone();
        metadata.section_id = metadata
            .section_id
            .as_deref()
            .and_then(|id| section_map.get(id).cloned());
        occurrences.push(metadata);
    }
    let mut position = next_position(&hierarchy, source.folder_id.as_deref());
    if let Some(existing) = hierarchy
        .playlists
        .iter()
        .find(|playlist| playlist.collection_id == target_id)
    {
        position = existing.position;
    }
    hierarchy
        .playlists
        .retain(|playlist| playlist.collection_id != target_id);
    hierarchy.playlists.push(PlaylistPlacement {
        collection_id: target_id.to_owned(),
        folder_id: source.folder_id,
        position,
        sections,
        occurrences,
    });
    save(store, &hierarchy)
}

pub fn set_sections(
    store: &Store,
    collection_id: &str,
    sections: &[NamedItem],
) -> Result<PlaylistHierarchy> {
    if sections.len() > MAX_SECTIONS
        || sections
            .iter()
            .any(|section| section.name.trim().is_empty() || section.name.len() > 512)
    {
        return Err("Playlist sections are invalid".into());
    }
    let mut hierarchy = load(store)?;
    let placement = ensure_placement(&mut hierarchy, collection_id);
    let old_ids = placement
        .sections
        .iter()
        .map(|section| section.id.clone())
        .collect::<HashSet<_>>();
    let mut new_sections = Vec::with_capacity(sections.len());
    let mut retained_ids = HashSet::new();
    for (position, section) in sections.iter().enumerate() {
        let id = if let Some(existing) = placement.sections.iter().find(|existing| {
            existing.name.eq_ignore_ascii_case(&section.name)
                && !retained_ids.contains(&existing.id)
        }) {
            existing.id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        retained_ids.insert(id.clone());
        new_sections.push(PlaylistSection {
            id,
            name: section.name.clone(),
            position: position as u32,
        });
    }
    let new_ids = new_sections
        .iter()
        .map(|section| section.id.clone())
        .collect::<HashSet<_>>();
    for occurrence in &mut placement.occurrences {
        if occurrence
            .section_id
            .as_ref()
            .is_some_and(|section| old_ids.contains(section) && !new_ids.contains(section))
        {
            occurrence.section_id = None;
        }
    }
    placement.sections = new_sections;
    save(store, &hierarchy)
}

/// Read hierarchy + ordered occurrence IDs into the portable export DTOs.
pub fn portable_entries(
    store: &Store,
    collection_id: &str,
    tracks: &[Track],
) -> Result<Vec<PortableEntry>> {
    store.ensure_playlist_occurrence_ids(collection_id)?;
    let hierarchy = load(store)?;
    let placement = hierarchy
        .playlists
        .iter()
        .find(|playlist| playlist.collection_id == collection_id);
    let occurrence_rows = store.list_playlist_occurrences(collection_id)?;
    let metadata = placement
        .map(|playlist| {
            playlist
                .occurrences
                .iter()
                .map(|item| (item.id.as_str(), item))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut entries = Vec::with_capacity(tracks.len());
    for (index, track) in tracks.iter().enumerate() {
        let row = occurrence_rows
            .get(index)
            .filter(|row| row.listing_id == track.id);
        let occurrence_id = row.map(|row| row.occurrence_id.as_str()).unwrap_or("");
        let details = metadata.get(occurrence_id).copied();
        entries.push(PortableEntry {
            id: occurrence_id.to_owned(),
            section_id: details
                .and_then(|item| item.section_id.as_deref())
                .and_then(|id| {
                    placement.and_then(|playlist| {
                        playlist.sections.iter().find(|section| section.id == id)
                    })
                })
                .map(|section| section.id.clone()),
            note: details.and_then(|item| item.note.clone()),
            cue_points: details
                .map(|item| item.cue_points.clone())
                .unwrap_or_default(),
            playback_range: details.and_then(|item| item.playback_range.clone()),
            track: PortableTrack::from_track(track),
            ..PortableEntry::default()
        });
    }
    Ok(entries)
}

pub fn folder_ancestry(store: &Store, collection_id: &str) -> Result<Vec<NamedItem>> {
    let hierarchy = load(store)?;
    let Some(placement) = hierarchy
        .playlists
        .iter()
        .find(|playlist| playlist.collection_id == collection_id)
    else {
        return Ok(Vec::new());
    };
    let mut id = placement.folder_id.as_deref();
    let mut ancestry = Vec::new();
    while let Some(folder_id) = id {
        let folder = hierarchy
            .folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .ok_or("Playlist folder not found")?;
        ancestry.push(NamedItem {
            id: folder.id.clone(),
            name: folder.name.clone(),
            ..NamedItem::default()
        });
        id = folder.parent_id.as_deref();
        if ancestry.len() > 32 {
            return Err("Playlist folder hierarchy contains a cycle".into());
        }
    }
    ancestry.reverse();
    Ok(ancestry)
}

pub fn sections(store: &Store, collection_id: &str) -> Result<Vec<NamedItem>> {
    Ok(load(store)?
        .playlists
        .into_iter()
        .find(|playlist| playlist.collection_id == collection_id)
        .map(|playlist| {
            let mut sections = playlist.sections;
            sections.sort_by_key(|section| section.position);
            sections
                .into_iter()
                .map(|section| NamedItem {
                    id: section.id,
                    name: section.name,
                    ..NamedItem::default()
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Apply portable hierarchy/occurrence data after an import has committed its
/// ordered entries. Sender IDs are translated to recipient-generated IDs.
pub fn import_portable_metadata(
    store: &Store,
    collection_id: &str,
    input: &crate::portable_playlist::PlaylistInput,
) -> Result<()> {
    let incoming_sections = input.sections.clone();
    set_sections(store, collection_id, &incoming_sections)?;
    let mut hierarchy = load(store)?;
    let placement = hierarchy
        .playlists
        .iter_mut()
        .find(|playlist| playlist.collection_id == collection_id)
        .ok_or("Playlist placement not found")?;
    let section_map = input
        .sections
        .iter()
        .zip(&placement.sections)
        .map(|(source, target)| (source.id.as_str(), target.id.as_str()))
        .collect::<HashMap<_, _>>();
    let ordered = store.list_playlist_occurrences(collection_id)?;
    if ordered.len() != input.entries.len() {
        return Err(
            "Imported playlist occurrence count changed during metadata application".into(),
        );
    }
    placement.occurrences = ordered
        .iter()
        .zip(&input.entries)
        .map(|(row, incoming)| OccurrenceMetadata {
            id: row.occurrence_id.clone(),
            section_id: incoming
                .section_id
                .as_deref()
                .and_then(|source| section_map.get(source).copied())
                .map(str::to_owned),
            note: incoming.note.clone(),
            cue_points: incoming.cue_points.clone(),
            playback_range: incoming.playback_range.clone(),
        })
        .collect();
    save(store, &hierarchy)?;
    Ok(())
}

/// Keep the preference schema readable to the backup sanitizer without making
/// the preference itself a source of entry ordering or recording identity.
pub fn backup_projection_value(store: &Store) -> Result<Value> {
    let value = serde_json::to_value(load(store)?).map_err(|error| error.to_string())?;
    Ok(value)
}

fn ensure_placement<'a>(
    hierarchy: &'a mut PlaylistHierarchy,
    collection_id: &str,
) -> &'a mut PlaylistPlacement {
    if let Some(index) = hierarchy
        .playlists
        .iter()
        .position(|playlist| playlist.collection_id == collection_id)
    {
        return &mut hierarchy.playlists[index];
    }
    let position = hierarchy.playlists.len() as u32;
    hierarchy.playlists.push(PlaylistPlacement {
        collection_id: collection_id.to_owned(),
        position,
        ..PlaylistPlacement::default()
    });
    hierarchy
        .playlists
        .last_mut()
        .expect("just inserted playlist placement")
}

fn next_position(hierarchy: &PlaylistHierarchy, folder_id: Option<&str>) -> u32 {
    hierarchy
        .playlists
        .iter()
        .filter(|playlist| playlist.folder_id.as_deref() == folder_id)
        .map(|playlist| playlist.position)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}
