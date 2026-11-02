use super::*;

impl Store {
    /// One bounded sidebar read; never loads full playlists or fetches providers.
    pub fn playlist_artwork(&self, ids: &[String]) -> Result<HashMap<String, Vec<String>>> {
        if ids.len() > 200 {
            return Err("At most 200 playlists may be queried at once".into());
        }
        let conn = self.lock_connection()?;
        let mut ordered = conn.prepare("SELECT json_extract(CASE WHEN r.manual_metadata='{}' THEN l.metadata ELSE json_patch(l.metadata,r.manual_metadata) END,'$.artworkUrl') FROM playlist_ordered_entries e JOIN catalog_listings l ON l.id=e.listing_id JOIN recordings r ON r.id=l.recording_id JOIN collections c ON c.id=e.collection_id AND c.deleted_at IS NULL WHERE e.collection_id=?1 ORDER BY e.position LIMIT 32").map_err(err)?;
        let mut legacy = conn.prepare("SELECT json_extract(CASE WHEN r.manual_metadata='{}' THEN l.metadata ELSE json_patch(l.metadata,r.manual_metadata) END,'$.artworkUrl') FROM collection_members m JOIN catalog_listings l ON l.id=l.recording_id AND l.recording_id=m.recording_id JOIN recordings r ON r.id=l.recording_id JOIN collections c ON c.id=m.collection_id AND c.deleted_at IS NULL WHERE m.collection_id=?1 ORDER BY m.position,l.id LIMIT 32").map_err(err)?;
        let mut result = HashMap::new();
        for id in ids {
            let is_ordered:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM playlist_ordered_collections WHERE collection_id=?1)",[id],|r|r.get(0)).map_err(err)?;
            let rows = if is_ordered {
                &mut ordered
            } else {
                &mut legacy
            }
            .query_map([id], |r| r.get::<_, Option<String>>(0))
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
            let mut urls = Vec::new();
            for url in rows
                .into_iter()
                .flatten()
                .filter(|url| url.starts_with("https://"))
            {
                if !urls.contains(&url) {
                    urls.push(url);
                }
                if urls.len() == 4 {
                    break;
                }
            }
            result.insert(id.clone(), urls);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playlist_covers_are_bounded_ordered_distinct_and_handle_empty_playlists() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let playlist = store.create_collection("Covers").unwrap();
        let empty = store.create_collection("Empty").unwrap();
        let tracks = (0..6)
            .map(|i| Track {
                id: format!("fixture:cover-{i}"),
                provider: "fixture".into(),
                provider_id: format!("cover-{i}"),
                title: format!("Track {i}"),
                artist: "Artist".into(),
                artwork_url: Some(format!(
                    "https://example.test/cover-{}.png",
                    if i == 0 { 1 } else { i }
                )),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        for track in &tracks {
            store.save_track(track).unwrap();
        }
        {
            let conn = store.lock_connection().unwrap();
            for (i, track) in tracks.iter().enumerate() {
                conn.execute(
                    "INSERT INTO collection_members VALUES(?1,?2,?3)",
                    params![playlist.id, track.id, i as i64],
                )
                .unwrap();
            }
        }
        let covers = store
            .playlist_artwork(&[playlist.id.clone(), empty.id.clone()])
            .unwrap();
        assert_eq!(
            covers[&playlist.id],
            (1..=4)
                .map(|i| format!("https://example.test/cover-{i}.png"))
                .collect::<Vec<_>>()
        );
        assert!(covers[&empty.id].is_empty());
        assert!(store.playlist_artwork(&vec!["x".into(); 201]).is_err());
    }
}
