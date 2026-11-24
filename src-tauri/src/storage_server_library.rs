//! Read-only membership from personal media servers. A catalog entry becomes
//! a saved library item only through the regular save or favorite actions.
use super::*;

pub(super) const VISIBLE_TRACK: &str = "EXISTS(SELECT 1 FROM library_server_members m
    JOIN library_server_catalogs s ON s.provider=m.provider AND s.account=m.account AND s.revision=m.revision
    WHERE m.listing_id=l.id AND s.visible=1)";

#[derive(Default)]
pub(super) struct SourceSelection {
    pub saved: bool,
    pub jellyfin: bool,
    pub subsonic: bool,
}

impl SourceSelection {
    /// Combined Library filter sent by the multi-select control. Single-source
    /// values keep their existing meaning for older callers.
    pub(super) fn combined(value: &str) -> Option<Self> {
        if value == "none" {
            return Some(Self::default());
        }
        if !value.contains(',') {
            return None;
        }
        let mut selection = Self::default();
        for source in value.split(',') {
            match source {
                "saved" => selection.saved = true,
                "jellyfin" => selection.jellyfin = true,
                "subsonic" => selection.subsonic = true,
                _ => return None,
            }
        }
        Some(selection)
    }
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS library_server_catalogs(
        provider TEXT PRIMARY KEY, account TEXT NOT NULL, revision TEXT NOT NULL, visible INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS library_server_members(
        listing_id TEXT PRIMARY KEY REFERENCES catalog_listings(id) ON DELETE CASCADE,
        provider TEXT NOT NULL, account TEXT NOT NULL, revision TEXT NOT NULL,
        cache_created INTEGER NOT NULL
      );").map_err(err)
}

pub(super) fn has_visible_catalog(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM library_server_catalogs WHERE visible=1)",
        [],
        |row| row.get(0),
    )
    .map_err(err)
}

impl Store {
    pub(crate) fn has_saved_server_track(&self, id: &str) -> Result<bool> {
        self.lock_connection()?.query_row("SELECT EXISTS(
            SELECT 1 FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id
            WHERE l.id=?1 AND (l.in_library=1 OR r.favorite=1
            OR EXISTS(SELECT 1 FROM playlist_ordered_entries p WHERE p.listing_id=l.id)
            OR EXISTS(SELECT 1 FROM collection_members c WHERE c.recording_id=r.id)))",
            [id], |row| row.get(0)).map_err(err)
    }

    /// Persist account selection and fence the old projection atomically. A
    /// newly connected catalog is published separately after indexing it.
    pub(crate) fn save_server_library_connection(
        &self,
        provider: &str,
        preference: &str,
        account: Option<&str>,
        visible: bool,
        value: &serde_json::Value,
    ) -> Result<()> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![preference, encode(value)?]).map_err(err)?;
        let changed = tx.execute("UPDATE library_server_catalogs SET visible=0 WHERE provider=?1 AND visible=1 AND (?2=0 OR account IS NOT ?3)", params![provider,visible,account]).map_err(err)?;
        if changed > 0 {
            tx.execute("UPDATE library_entity_state SET dirty=1", [])
                .map_err(err)?;
        }
        tx.commit().map_err(err)
    }

    pub(crate) fn server_library_catalog_revision(
        &self,
        provider: &str,
        account: &str,
    ) -> Result<Option<String>> {
        self.lock_connection()?
            .query_row(
                "SELECT revision FROM library_server_catalogs WHERE provider=?1 AND account=?2",
                params![provider, account],
                |row| row.get(0),
            )
            .optional()
            .map_err(err)
    }

    pub(crate) fn show_server_library_catalog(
        &self,
        provider: &str,
        account: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction().map_err(err)?;
        let changed = tx
            .execute(
                "UPDATE library_server_catalogs SET visible=COALESCE(account=?2,0)
             WHERE provider=?1 AND visible<>COALESCE(account=?2,0)",
                params![provider, account],
            )
            .map_err(err)?;
        if changed > 0 {
            tx.execute("UPDATE library_entity_state SET dirty=1", [])
                .map_err(err)?;
        }
        tx.commit().map_err(err)
    }

    pub(crate) fn replace_server_library_catalog(
        &self,
        provider: &str,
        account: &str,
        revision: &str,
        tracks: &[Track],
    ) -> Result<()> {
        let id_prefix = format!("{account}.");
        if tracks
            .iter()
            .any(|track| track.provider != provider || !track.provider_id.starts_with(&id_prefix))
        {
            return Err("Server library track belongs to a different connection".into());
        }
        let mut conn = self.lock_connection()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        {
            let mut preexisting = tx
                .prepare("SELECT EXISTS(SELECT 1 FROM catalog_listings WHERE id=?1)")
                .map_err(err)?;
            let mut register = tx.prepare("INSERT INTO library_server_members(listing_id,provider,account,revision,cache_created)
                VALUES(?1,?2,?3,?4,?5) ON CONFLICT(listing_id) DO UPDATE SET revision=excluded.revision").map_err(err)?;
            for track in tracks {
                let exists: bool = preexisting
                    .query_row([&track.id], |row| row.get(0))
                    .map_err(err)?;
                upsert_track(&tx, track, false)?;
                register
                    .execute(params![track.id, provider, account, revision, !exists])
                    .map_err(err)?;
            }
        }
        tx.execute("INSERT INTO library_server_catalogs(provider,account,revision,visible) VALUES(?1,?2,?3,1)
            ON CONFLICT(provider) DO UPDATE SET account=excluded.account,revision=excluded.revision,visible=excluded.visible",
            params![provider,account,revision]).map_err(err)?;
        tx.execute("UPDATE library_entity_state SET dirty=1", [])
            .map_err(err)?;
        tx.commit().map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(provider: &str, provider_id: &str, title: &str) -> Track {
        Track {
            id: format!("{provider}:{provider_id}"),
            provider: provider.into(),
            provider_id: provider_id.into(),
            title: title.into(),
            artist: "Same Name".into(),
            album: Some("Release".into()),
            provenance: serde_json::json!({ "subsonicArtistId": "server-artist", "subsonicAlbumId": "album" }),
            ..Track::default()
        }
    }

    #[test]
    fn server_library_browses_tracks_artists_and_albums_without_saving_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let saved = track("local", "saved", "Saved Song");
        let server = track("subsonic", "account.one", "Server Song");
        let mut browse_only = track("subsonic", "account.two", "Browse Song");
        browse_only.album = Some("Browse-only album".into());
        browse_only.provenance["subsonicAlbumId"] = serde_json::json!("browse-album");
        store.save_track(&saved).unwrap();
        store
            .replace_server_library_catalog(
                "subsonic",
                "account",
                "r1",
                &[server.clone(), browse_only.clone()],
            )
            .unwrap();
        let page = |source| {
            store
                .list_tracks_page_with_sort_state(
                    "library", None, 20, 0, None, None, None, None, None, None, source,
                )
                .unwrap()
                .0
        };
        assert_eq!(page(None).len(), 3);
        assert_eq!(page(Some("saved"))[0].id, saved.id);
        assert_eq!(page(Some("subsonic")).len(), 2);
        assert_eq!(
            store
                .search_library_tracks(&["server".into()], 20, 0)
                .unwrap()
                .1,
            1
        );
        let entities = |kind, source| {
            store
                .list_library_entities_filtered(
                    kind,
                    None,
                    "",
                    source,
                    None,
                    "name",
                    false,
                    20,
                    0,
                    library::ArtistGrouping::Individual,
                )
                .unwrap()
                .items
        };
        let artists = entities("artist", None);
        assert_eq!(artists.len(), 2);
        assert_ne!(artists[0].id, artists[1].id);
        assert_eq!(entities("album", Some("subsonic")).len(), 2);
        assert_eq!(entities("artist", Some("__saved__")).len(), 1);

        let jellyfin = track("jellyfin", "account.jelly", "Jellyfin Song");
        store.replace_server_library_catalog("jellyfin", "account", "r1", &[jellyfin.clone()]).unwrap();
        let saved_and_jellyfin = page(Some("saved,jellyfin"));
        assert_eq!(saved_and_jellyfin.len(), 2);
        assert!(saved_and_jellyfin.iter().any(|track| track.id == saved.id));
        assert!(saved_and_jellyfin.iter().any(|track| track.id == jellyfin.id));
        assert_eq!(page(Some("jellyfin,subsonic")).len(), 3);
        assert!(page(Some("none")).is_empty());
        assert_eq!(entities("artist", Some("saved,jellyfin")).len(), 2);
        assert_eq!(entities("album", Some("jellyfin,subsonic")).len(), 3);
        assert!(entities("album", Some("none")).is_empty());
        store.show_server_library_catalog("jellyfin", None).unwrap();

        let playlist = store.create_collection("My playlist").unwrap();
        store
            .append_playlist_entries(&playlist.id, &[server.clone()])
            .unwrap();
        store.show_server_library_catalog("subsonic", None).unwrap();
        assert_eq!(page(None).len(), 2);
        assert_eq!(entities("album", Some("subsonic"))[0].name, "Release");
        assert_eq!(entities("album", Some("subsonic")).len(), 1);
        assert!(!store
            .list_tracks("all", None)
            .unwrap()
            .iter()
            .any(|track| track.id == browse_only.id));
        assert_eq!(
            store.list_tracks("collection", Some(&playlist.id)).unwrap()[0].id,
            server.id
        );
        store
            .show_server_library_catalog("subsonic", Some("account"))
            .unwrap();
        assert_eq!(page(None).len(), 3);
        store
            .save_server_library_connection(
                "subsonic",
                "subsonic.connection.v1",
                Some("new-account"),
                true,
                &serde_json::json!({}),
            )
            .unwrap();
        assert_eq!(page(None).len(), 2);
    }
}
