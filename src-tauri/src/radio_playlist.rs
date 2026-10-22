//! Explicit per-station local playlist capture, called only by played-song ingestion.
use super::*;
use crate::models::Collection;
use std::sync::Mutex;
static CAPTURE_GATE: Mutex<()> = Mutex::new(());
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioPlaylistOption {
    pub enabled: bool,
    pub collection_id: Option<String>,
}
fn key(station_id: &str) -> String {
    format!("radioPlaylistCapture.v1:{station_id}")
}
fn local_target(store: &Store, id: &str) -> Result<Collection, String> {
    let collection = store
        .list_collections()?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or("Choose an active local playlist")?;
    if id == crate::storage::DOWNLOADS_COLLECTION_ID || store.is_browser_source_collection(id)? {
        return Err("Choose a local playlist rather than a provider or Downloads playlist".into());
    }
    Ok(collection)
}
pub fn list_radio_playlist_targets(store: &Store) -> Result<Vec<Collection>, String> {
    let mut targets = Vec::new();
    for collection in store.list_collections()? {
        if collection.id != crate::storage::DOWNLOADS_COLLECTION_ID
            && !store.is_browser_source_collection(&collection.id)?
        {
            targets.push(collection);
        }
    }
    Ok(targets)
}
pub fn get_radio_playlist_option(
    store: &Store,
    station_id: &str,
) -> Result<RadioPlaylistOption, String> {
    if station_id.is_empty() || station_id.len() > 256 {
        return Err("Invalid station identity".into());
    }
    store
        .get_json_preference(&key(station_id))?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| "Invalid radio playlist option".into())
        .map(|v| v.unwrap_or_default())
}
pub fn set_radio_playlist_option(
    store: &Store,
    station_id: &str,
    enabled: bool,
    collection_id: Option<&str>,
) -> Result<RadioPlaylistOption, String> {
    let _guard = CAPTURE_GATE
        .lock()
        .map_err(|_| "Radio playlist settings unavailable")?;
    let station = get_radio_stations_saved(store)?
        .into_iter()
        .find(|s| s.id == station_id)
        .ok_or("Radio station no longer exists")?;
    let previous = get_radio_playlist_option(store, station_id)?;
    let target = if enabled {
        if collection_id == Some("") {
            Some(
                store
                    .create_collection(&station.name.chars().take(120).collect::<String>())?
                    .id,
            )
        } else if let Some(id) = collection_id.filter(|id| !id.is_empty()) {
            Some(local_target(store, id)?.id)
        } else if let Some(id) = previous
            .collection_id
            .as_deref()
            .filter(|id| local_target(store, id).is_ok())
        {
            Some(id.to_owned())
        } else {
            Some(
                store
                    .create_collection(&station.name.chars().take(120).collect::<String>())?
                    .id,
            )
        }
    } else {
        collection_id
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .or(previous.collection_id)
    };
    let option = RadioPlaylistOption {
        enabled,
        collection_id: target,
    };
    store.set_json_preference(
        &key(station_id),
        &serde_json::to_value(&option).map_err(|e| e.to_string())?,
    )?;
    Ok(option)
}
pub fn auto_add_played_radio_track(
    store: &Store,
    station: &RadioStation,
    metadata: &RadioTrackMetadata,
) -> Result<bool, String> {
    let _guard = CAPTURE_GATE
        .lock()
        .map_err(|_| "Radio playlist settings unavailable")?;
    let option = get_radio_playlist_option(store, &station.id)?;
    if !option.enabled {
        return Ok(false);
    }
    if metadata.station_id != station.id
        || metadata.artist.trim().is_empty()
        || metadata.title.trim().is_empty()
    {
        return Ok(false);
    }
    let target = local_target(
        store,
        option
            .collection_id
            .as_deref()
            .ok_or("Radio playlist target is missing")?,
    )?;
    // Set membership performs an atomic INSERT OR IGNORE by recording identity,
    // including for ordered playlists. Repeat recognition never creates repeats.
    let track = crate::playback_history::radio_song_track(station, metadata);
    store.add_to_collection(&target.id, &[track])?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_requires_opt_in_creates_station_named_playlist_and_can_disable() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite")).unwrap();
        let station = get_radio_stations_saved(&store).unwrap().remove(0);
        assert!(
            !get_radio_playlist_option(&store, &station.id)
                .unwrap()
                .enabled
        );
        let enabled = set_radio_playlist_option(&store, &station.id, true, None).unwrap();
        let target = local_target(&store, enabled.collection_id.as_deref().unwrap()).unwrap();
        assert_eq!(target.name, station.name);
        let repeated = set_radio_playlist_option(&store, &station.id, true, None).unwrap();
        assert_eq!(repeated.collection_id, enabled.collection_id);
        let disabled = set_radio_playlist_option(&store, &station.id, false, None).unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.collection_id, enabled.collection_id);
    }
    #[test]
    fn rejects_deleted_or_downloads_targets_without_enabling() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite")).unwrap();
        let station = get_radio_stations_saved(&store).unwrap().remove(0);
        let target = store.create_collection("Deleted").unwrap();
        store.delete_collection(&target.id).unwrap();
        assert!(set_radio_playlist_option(&store, &station.id, true, Some(&target.id)).is_err());
        let downloads = store.ensure_downloads_collection().unwrap();
        assert!(set_radio_playlist_option(&store, &station.id, true, Some(&downloads.id)).is_err());
        assert!(
            !get_radio_playlist_option(&store, &station.id)
                .unwrap()
                .enabled
        );
    }
    #[test]
    fn played_capture_deduplicates_and_disable_stops_new_items() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite")).unwrap();
        let station = get_radio_stations_saved(&store).unwrap().remove(0);
        let mut metadata:RadioTrackMetadata=serde_json::from_value(json!({"stationId":station.id,"artist":"Fixture Artist","title":"Fixture Song","sourceUrl":"","metadataConfidence":"station metadata"})).unwrap();
        assert!(!auto_add_played_radio_track(&store, &station, &metadata).unwrap());
        let option = set_radio_playlist_option(&store, &station.id, true, None).unwrap();
        let target = option.collection_id.unwrap();
        assert_eq!(local_target(&store, &target).unwrap().track_count, 0);
        auto_add_played_radio_track(&store, &station, &metadata).unwrap();
        auto_add_played_radio_track(&store, &station, &metadata).unwrap();
        assert_eq!(local_target(&store, &target).unwrap().track_count, 1);
        metadata.station_id = "different".into();
        assert!(!auto_add_played_radio_track(&store, &station, &metadata).unwrap());
        metadata.station_id = station.id.clone();
        metadata.title = "Next Song".into();
        set_radio_playlist_option(&store, &station.id, false, None).unwrap();
        assert!(!auto_add_played_radio_track(&store, &station, &metadata).unwrap());
        assert_eq!(local_target(&store, &target).unwrap().track_count, 1);
    }
}
