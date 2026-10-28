//! Reviewable imports share adapters and storage semantics with the companion.
use crate::{
    browser_bridge::{BrowserImportResult, BrowserSnapshot},
    models::Chart,
    storage::Store,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn changes(previous: Option<&Chart>, next: &Chart, complete: bool) -> Value {
    fn positions(chart: Option<&Chart>) -> HashMap<&str, BTreeSet<u64>> {
        let mut result: HashMap<&str, BTreeSet<u64>> = HashMap::new();
        for (index, track) in chart.into_iter().flat_map(|c| c.tracks.iter().enumerate()) {
            let positions = result.entry(track.id.as_str()).or_default();
            if let Some(values) = track.provenance["occurrencePositions"].as_array() {
                positions.extend(values.iter().filter_map(Value::as_u64));
            } else {
                positions.insert(index as u64 + 1);
            }
        }
        result
    }
    let old = positions(previous);
    let new = positions(Some(next));
    let added = new.keys().filter(|id| !old.contains_key(**id)).count();
    let absent = old.keys().filter(|id| !new.contains_key(**id)).count();
    let moved = new
        .iter()
        .filter(|(id, pos)| old.get(**id).is_some_and(|old| old != *pos))
        .count();
    json!({"added":added,"removed":if complete {absent} else {0},"retainedFromEarlierCapture":if complete {0} else {absent},"moved":if complete {moved} else {0},"partial":!complete,"comparison":"previousCapture"})
}

pub fn previous(store: &Store, source: &str) -> Result<Option<Chart>, String> {
    store
        .latest_browser_import(source)?
        .map(|i| store.load_browser_import(&i.id))
        .transpose()
}

pub fn preview(store: &Store, snapshot: BrowserSnapshot) -> Result<Value, String> {
    let chart = crate::browser_bridge::validate_snapshot(&snapshot)?;
    let previous = previous(store, &chart.source_url)?;
    let id = uuid::Uuid::new_v4().to_string();
    let complete = crate::browser_bridge::snapshot_complete(&snapshot);
    let delta = changes(previous.as_ref(), &chart, complete);
    store.set_provider_cache(
        "collection-import-preview.v1",
        &id,
        &json!({"snapshot":snapshot,"baseId":previous.as_ref().map(|c|&c.id)}),
        900,
    )?;
    Ok(
        json!({"previewId":id,"title":chart.title,"sourceUrl":chart.source_url,"capturedCount":snapshot.entries.len(),"uniqueRecordings":chart.tracks.len(),"expectedTotal":snapshot.expected_total,"complete":complete,"changes":delta,"failedSources":snapshot.failed_sources,"albumUrls":snapshot.album_urls,"warnings":crate::browser_bridge::snapshot_warnings(&snapshot,&chart),"sample":chart.tracks.iter().take(20).map(|t|json!({"title":t.title,"artist":t.artist,"version":t.version})).collect::<Vec<_>>()}),
    )
}

pub fn apply(store: &Store, preview_id: &str) -> Result<BrowserImportResult, String> {
    let _library_write = store.try_library_write()?;
    apply_inner(store, preview_id)
}

fn apply_inner(store: &Store, preview_id: &str) -> Result<BrowserImportResult, String> {
    uuid::Uuid::parse_str(preview_id).map_err(|_| "Invalid import preview ID")?;
    if let Some(result) = store.browser_import_by_id(preview_id)? {
        return Ok(result);
    }
    let cached = store
        .get_provider_cache("collection-import-preview.v1", preview_id)?
        .ok_or("Import preview expired; prepare a new preview")?;
    let snapshot: BrowserSnapshot =
        serde_json::from_value(cached["snapshot"].clone()).map_err(|_| "Invalid import preview")?;
    let current = previous(store, &snapshot.source_url)?;
    if current.as_ref().map(|c| c.id.as_str()) != cached["baseId"].as_str() {
        return Err("The source changed after this preview; prepare a fresh preview".into());
    }
    let mut chart = crate::browser_bridge::validate_snapshot(&snapshot)?;
    chart.id = preview_id.to_owned();
    store.import_browser_chart_reviewed(&chart, &snapshot, Some(cached["baseId"].as_str()))
}

/// Apply an import preview and optionally append its reviewed tracks to a chosen playlist.
pub fn apply_with_destination(
    store: &Store,
    preview_id: &str,
    destination_collection_id: Option<&str>,
    duplicate_policy: Option<&str>,
    folder_path: Option<&str>,
    section_name: Option<&str>,
) -> Result<BrowserImportResult, String> {
    let Some(destination_id) = destination_collection_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return apply(store, preview_id);
    };
    let _library_write = store.try_library_write()?;
    let policy = duplicate_policy.unwrap_or("keep");
    if !matches!(policy, "keep" | "skip") {
        return Err("Choose keep or skip for duplicate handling".into());
    }
    let binding = json!({"destinationCollectionId":destination_id,"duplicatePolicy":policy,"folderPath":folder_path.unwrap_or(""),"sectionName":section_name.unwrap_or("")});
    if let Some(previous) =
        store.get_provider_cache("collection-import-destination.v1", preview_id)?
    {
        if previous != binding {
            return Err("This preview was already applied with a different destination".into());
        }
        return apply_inner(store, preview_id);
    }
    let target = crate::playlist_sync::ensure_editable_collection(store, destination_id)?;
    let previous_count = store
        .list_collections()?
        .into_iter()
        .find(|collection| collection.id == target)
        .map(|collection| collection.track_count)
        .unwrap_or(0);
    let result = apply_inner(store, preview_id)?;
    let imported = store.load_browser_import(&result.id)?;
    if policy == "skip" {
        store.add_to_collection(&target, &imported.tracks)?;
    } else {
        store.append_playlist_entries(&target, &imported.tracks)?;
    }
    let after_count = store
        .list_collections()?
        .into_iter()
        .find(|collection| collection.id == target)
        .map(|collection| collection.track_count)
        .unwrap_or(previous_count);
    if folder_path.is_some_and(|value| !value.trim().is_empty())
        || section_name.is_some_and(|value| !value.trim().is_empty())
    {
        crate::playlist_hierarchy::place_imported_items(
            store,
            &target,
            folder_path.unwrap_or(""),
            section_name.unwrap_or(""),
            if policy == "skip" {
                after_count.saturating_sub(previous_count) as usize
            } else {
                imported.tracks.len()
            },
        )?;
    }
    store.set_provider_cache(
        "collection-import-destination.v1",
        preview_id,
        &binding,
        900,
    )?;
    Ok(result)
}

pub async fn retry(store: &Store, import_id: &str) -> Result<Value, String> {
    let saved = store
        .browser_import_by_id(import_id)?
        .ok_or("Saved import not found")?;
    if saved.provider != "bandcamp" || saved.failed_sources.is_empty() {
        return Err("This import has no failed album pages to retry".into());
    }
    let mut snapshot = crate::collection_sources::import_bandcamp_album_collection(
        &saved.source_url,
        &saved.title,
        saved.failed_sources.clone(),
    )
    .await?;
    let old = store.load_browser_import(import_id)?;
    let entry_key = |e: &crate::browser_bridge::BrowserEntry| {
        (
            e.provider_id.clone(),
            e.source_url.clone(),
            e.title.clone(),
            e.metadata["albumUrl"].as_str().map(str::to_owned),
            e.metadata["trackNumber"].as_u64(),
        )
    };
    let mut seen: HashSet<_> = snapshot.entries.iter().map(&entry_key).collect();
    for track in old.tracks {
        for value in track.provenance["rawCaptures"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let entry: crate::browser_bridge::BrowserEntry =
                serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            if seen.insert(entry_key(&entry)) {
                snapshot.entries.push(entry);
            }
        }
    }
    snapshot.album_urls = saved.album_urls.clone();
    for entry in &mut snapshot.entries {
        if let Some(index) = saved
            .album_urls
            .iter()
            .position(|url| entry.metadata["albumUrl"].as_str() == Some(url.as_str()))
        {
            entry.metadata["albumOrder"] = json!(index + 1);
        }
    }
    snapshot.entries.sort_by_key(|e| {
        (
            e.metadata["albumOrder"].as_u64().unwrap_or(u64::MAX),
            e.metadata["trackNumber"]
                .as_u64()
                .unwrap_or(e.position as u64),
        )
    });
    for (i, entry) in snapshot.entries.iter_mut().enumerate() {
        entry.position = i as u32 + 1;
    }
    // Retrying failures does not prove the full Discover page is captured.
    snapshot.complete = false;
    preview(store, snapshot)
}

pub async fn enrich(store: &Store, track_id: &str) -> Result<Value, String> {
    let track = store.get_track(track_id)?.ok_or("Saved track not found")?;
    let raw = track.provenance["importMetadata"]["albumUrl"]
        .as_str()
        .unwrap_or(&track.source_url);
    let details = if url::Url::parse(raw).is_ok_and(|page| page.path().starts_with("/album/")) {
        let snapshot = crate::collection_sources::import_bandcamp(raw).await?;
        let entry = snapshot
            .entries
            .iter()
            .find(|e| e.source_url.as_deref() == Some(track.source_url.as_str()))
            .or_else(|| {
                snapshot
                    .entries
                    .iter()
                    .find(|e| e.title == track.title && e.artist == track.artist)
            })
            .ok_or("Saved track was not present on its album page")?;
        json!({"artworkUrl":entry.artwork_url,"album":entry.album,"releaseDate":entry.release_date,"metadata":entry.metadata})
    } else {
        let (_sender, cancel) = tokio::sync::watch::channel(false);
        crate::bandcamp::resolve_track_page_audio(
            raw,
            cancel,
            tokio::time::Instant::now() + std::time::Duration::from_secs(12),
        )
        .await?
        .page_metadata
    };
    let updated = crate::import_metadata::fill_bandcamp(store, &track, &details, raw)?;
    Ok(
        json!({"trackId":updated.id,"artworkUrl":updated.artwork_url,"album":updated.album,"metadata":updated.provenance["importMetadata"]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_capture_never_proposes_removals() {
        let chart = |ids: &[&str]| Chart {
            id: String::new(),
            provider: "test".into(),
            title: "test".into(),
            source_url: String::new(),
            fetched_at: String::new(),
            tracks: ids
                .iter()
                .map(|id| crate::models::Track {
                    id: (*id).into(),
                    ..Default::default()
                })
                .collect(),
            stale: false,
            warning: None,
        };
        let old = chart(&["a", "b"]);
        let next = chart(&["b", "c"]);
        assert_eq!(changes(Some(&old), &next, false)["removed"], 0);
        assert_eq!(
            changes(Some(&old), &next, false)["retainedFromEarlierCapture"],
            1
        );
        assert_eq!(changes(Some(&old), &next, true)["removed"], 1);
    }

    fn snapshot(title: &str) -> BrowserSnapshot {
        serde_json::from_value(json!({"schemaVersion":1,"provider":"spotify","kind":"playlist","title":"Test","sourceUrl":"https://open.spotify.com/playlist/0123456789ABCDEFGHIJKL","capturedAt":"2026-10-24T12:00:00Z","complete":true,"expectedTotal":1,"warnings":[],"entries":[{"providerId":"0123456789ABCDEFGHIJKL","sourceUrl":"https://open.spotify.com/track/0123456789ABCDEFGHIJKL","artist":"Artist","title":title,"position":1}]})).unwrap()
    }
    #[test]
    fn reviewed_import_is_idempotent_and_rejects_a_stale_base() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("imports.sqlite")).unwrap();
        let first = preview(&store, snapshot("First")).unwrap();
        assert!(store.list_browser_imports().unwrap().is_empty());
        let id = first["previewId"].as_str().unwrap();
        let applied = apply(&store, id).unwrap();
        assert_eq!(apply(&store, id).unwrap().id, applied.id);
        assert_eq!(store.list_browser_imports().unwrap().len(), 1);
        let pending = preview(&store, snapshot("Pending")).unwrap();
        let intervening = snapshot("New capture");
        let chart = crate::browser_bridge::validate_snapshot(&intervening).unwrap();
        store.import_browser_chart(&chart, &intervening).unwrap();
        assert!(apply(&store, pending["previewId"].as_str().unwrap())
            .unwrap_err()
            .contains("changed"));
    }
}
