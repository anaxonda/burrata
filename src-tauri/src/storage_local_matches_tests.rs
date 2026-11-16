use super::*;

fn db() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("local-matches.sqlite")).unwrap();
    (dir, store)
}

fn track(id: &str, provider: &str, title: &str, artist: &str, duration: Option<f64>) -> Track {
    Track {
        id: format!("{provider}:{id}"),
        provider: provider.into(),
        provider_id: id.into(),
        source_url: format!("https://example.test/{provider}/{id}"),
        title: title.into(),
        artist: artist.into(),
        duration_seconds: duration,
        ..Default::default()
    }
}

fn request() -> Track {
    let mut track = track(
        "1234567890123456789012",
        "spotify",
        "Baba - Extended Mix",
        "Amadou & Mariam",
        Some(294.214),
    );
    track.album = Some("Requested Provider Album".into());
    track
}

fn asset(
    id: &str,
    title: &str,
    version: Option<&str>,
    artist: &str,
    duration: Option<f64>,
    isrc: Option<&str>,
    path: &std::path::Path,
    create_file: bool,
) -> LocalAsset {
    if create_file {
        std::fs::write(path, b"registered local audio fixture").unwrap();
    }
    let hash = if create_file {
        crate::media::hash_file(path).unwrap()
    } else {
        format!("hash-{id}")
    };
    let mut track = track(&format!("asset-{id}"), "local", title, artist, duration);
    track.version = version.map(str::to_owned);
    track.isrc = isrc.map(str::to_owned);
    track.album = Some("File Tag Album".into());
    LocalAsset {
        id: id.into(),
        recording_id: track.id.clone(),
        path: path.to_string_lossy().into_owned(),
        hash,
        source_codec: "flac".into(),
        final_codec: "flac".into(),
        source_bitrate: None,
        final_bitrate: None,
        sample_rate: Some(48_000),
        channels: Some(2),
        bit_depth: Some(24),
        duration_seconds: duration,
        size_bytes: Some(1024),
        bitrate_origin: Some("stream".into()),
        processing: "managed copy".into(),
        track,
    }
}

fn add_matching_asset(dir: &tempfile::TempDir, store: &Store, id: &str) {
    let path = dir.path().join(format!("{id}.flac"));
    store
        .save_asset(&asset(
            id,
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            Some(294.2405),
            None,
            &path,
            true,
        ))
        .unwrap();
}

fn page(
    store: &Store,
    view: &str,
    collection_id: Option<&str>,
    availability: Option<&str>,
    provider: Option<&str>,
) -> Vec<Track> {
    store
        .list_tracks_page(
            view,
            collection_id,
            100,
            0,
            None,
            None,
            None,
            None,
            None,
            availability,
            provider,
        )
        .unwrap()
        .0
}

#[test]
fn exact_cross_provider_file_hydrates_without_merging_recordings() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    add_matching_asset(&dir, &store, "baba-file");

    assert_ne!(
        store.recording_id(&requested.id).unwrap(),
        store.recording_id("local:asset-baba-file").unwrap(),
        "a local exact match must not merge catalog recordings"
    );

    let loaded = store.get_track(&requested.id).unwrap().unwrap();
    assert_eq!(loaded.id, requested.id);
    assert_eq!(loaded.title, "Baba - Extended Mix");
    assert_eq!(loaded.album.as_deref(), Some("Requested Provider Album"));
    assert_eq!(loaded.asset_id.as_deref(), Some("baba-file"));
    assert_eq!(
        loaded.local_path.as_deref(),
        Some(dir.path().join("baba-file.flac").to_string_lossy().as_ref())
    );

    let ids = vec![
        requested.id.clone(),
        "missing:track".into(),
        requested.id.clone(),
    ];
    let loaded_many = store.get_tracks(&ids).unwrap();
    assert_eq!(loaded_many.len(), 2);
    assert!(loaded_many.iter().all(|track| track.id == requested.id));
    assert!(loaded_many
        .iter()
        .all(|track| track.asset_id.as_deref() == Some("baba-file")));

    let files = store.list_track_files(&requested.id).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, "baba-file");
    assert_eq!(
        files[0].path,
        dir.path()
            .join("baba-file.flac")
            .to_string_lossy()
            .into_owned()
    );
}

#[test]
fn local_and_not_local_filters_include_cross_provider_matches_in_library_and_collection() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    add_matching_asset(&dir, &store, "baba-filter-file");
    let collection = store
        .create_collection("Local match filter fixture")
        .unwrap();
    store
        .add_to_collection(&collection.id, std::slice::from_ref(&requested))
        .unwrap();

    for (view, collection_id) in [
        ("library", None),
        ("collection", Some(collection.id.as_str())),
    ] {
        let local = page(&store, view, collection_id, Some("local"), None);
        assert!(local.iter().any(|track| track.id == requested.id));
        let not_local = page(&store, view, collection_id, Some("not-local"), None);
        assert!(!not_local.iter().any(|track| track.id == requested.id));
        let local_provider = page(&store, view, collection_id, None, Some("local"));
        assert!(local_provider.iter().any(|track| track.id == requested.id));
    }

    let hydrated = store.get_track(&requested.id).unwrap().unwrap();
    assert_eq!(hydrated.local_availability, "available");
    assert_eq!(hydrated.asset_id.as_deref(), Some("baba-filter-file"));
}

#[test]
fn missing_files_and_ambiguous_matches_are_not_hydrated() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    let missing_path = dir.path().join("missing.flac");
    store
        .save_asset(&asset(
            "missing-file",
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            Some(294.2405),
            None,
            &missing_path,
            false,
        ))
        .unwrap();
    assert!(store
        .get_track(&requested.id)
        .unwrap()
        .unwrap()
        .asset_id
        .is_none());

    let first_path = dir.path().join("ambiguous-first.flac");
    let second_path = dir.path().join("ambiguous-second.flac");
    store
        .save_asset(&asset(
            "ambiguous-first",
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            Some(294.2405),
            None,
            &first_path,
            true,
        ))
        .unwrap();
    store
        .save_asset(&asset(
            "ambiguous-second",
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            Some(294.2405),
            None,
            &second_path,
            true,
        ))
        .unwrap();
    let loaded = store.get_track(&requested.id).unwrap().unwrap();
    assert!(
        loaded.asset_id.is_none(),
        "ambiguous local evidence must fail closed"
    );
    assert!(loaded.local_path.is_none());
}

#[test]
fn incompatible_edition_artist_isrc_or_unknown_runtime_does_not_match() {
    let cases = [
        (
            "different-mix",
            "Baba",
            Some("Live Mix"),
            "Amadou & Mariam",
            Some(294.2405),
            None,
        ),
        (
            "short-edit",
            "Baba",
            Some("Short Edit"),
            "Amadou & Mariam",
            Some(230.0),
            None,
        ),
        (
            "shortened-runtime",
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            Some(245.0),
            None,
        ),
        (
            "wrong-artist",
            "Baba",
            Some("Extended Mix"),
            "Different Artist",
            Some(294.2405),
            None,
        ),
        (
            "isrc-conflict",
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            Some(294.2405),
            Some("USAAA0000001"),
        ),
        (
            "unknown-runtime",
            "Baba",
            Some("Extended Mix"),
            "Amadou & Mariam",
            None,
            None,
        ),
    ];
    for (id, title, version, artist, duration, isrc) in cases {
        let (dir, store) = db();
        let mut requested = request();
        if id == "isrc-conflict" {
            requested.isrc = Some("GBBBB0000002".into());
        }
        store.save_track(&requested).unwrap();
        let path = dir.path().join(format!("{id}.flac"));
        store
            .save_asset(&asset(
                id, title, version, artist, duration, isrc, &path, true,
            ))
            .unwrap();
        let loaded = store.get_track(&requested.id).unwrap().unwrap();
        assert!(
            loaded.asset_id.is_none(),
            "{id} must not be treated as the same recording"
        );
        assert!(loaded.local_path.is_none(), "{id}");
    }
}

#[test]
fn no_isrc_exact_identity_respects_balanced_flexible_and_exact_modes_without_merging() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    add_matching_asset(&dir, &store, "mode-policy-file");
    let requested_recording = store.recording_id(&requested.id).unwrap();
    let asset_recording = store.recording_id("local:asset-mode-policy-file").unwrap();
    assert_ne!(requested_recording, asset_recording);

    for (level, expected_available) in [
        (crate::recording_match::MatchLevel::Balanced, true),
        (crate::recording_match::MatchLevel::Flexible, true),
        (crate::recording_match::MatchLevel::Exact, false),
    ] {
        let mut settings = store.settings().unwrap();
        settings.matching_level = level;
        store.save_settings(&settings).unwrap();
        let loaded = store.get_track(&requested.id).unwrap().unwrap();
        assert_eq!(
            loaded.asset_id.as_deref() == Some("mode-policy-file"),
            expected_available,
            "unexpected local match under {level:?}"
        );
        assert_eq!(
            loaded.local_path.is_some(),
            expected_available,
            "path availability should follow the current policy"
        );
        assert_eq!(
            store.recording_id(&requested.id).unwrap(),
            requested_recording
        );
        assert_eq!(
            store.recording_id("local:asset-mode-policy-file").unwrap(),
            asset_recording
        );
    }
}

#[test]
fn retained_cross_file_playback_hint_is_rechecked_for_title_version_mode_and_source_lock() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    add_matching_asset(&dir, &store, "preflight-baba-file");
    let retained = store.get_track(&requested.id).unwrap().unwrap();
    assert_eq!(retained.asset_id.as_deref(), Some("preflight-baba-file"));
    assert!(retained.local_path.is_some());

    let mut changed_title = retained.clone();
    changed_title.title = "Different Song".into();
    let prepared =
        crate::playback_sources::reconcile_stale_local_asset(&store, &changed_title, false)
            .unwrap();
    assert!(
        prepared.local_path.is_none(),
        "a stale title must drop the retained file hint"
    );
    assert!(prepared.asset_id.is_none());

    let mut changed_version = retained.clone();
    changed_version.version = Some("Live Mix".into());
    let prepared =
        crate::playback_sources::reconcile_stale_local_asset(&store, &changed_version, false)
            .unwrap();
    assert!(
        prepared.local_path.is_none(),
        "a stale version must drop the retained file hint"
    );
    assert!(prepared.asset_id.is_none());

    let mut settings = store.settings().unwrap();
    settings.matching_level = crate::recording_match::MatchLevel::Exact;
    store.save_settings(&settings).unwrap();
    let prepared =
        crate::playback_sources::reconcile_stale_local_asset(&store, &retained, false).unwrap();
    assert!(
        prepared.local_path.is_none(),
        "a policy change must invalidate a no-ISRC hint"
    );
    assert!(prepared.asset_id.is_none());

    settings.matching_level = crate::recording_match::MatchLevel::Balanced;
    store.save_settings(&settings).unwrap();
    assert!(store
        .get_track(&requested.id)
        .unwrap()
        .unwrap()
        .local_path
        .is_some());
    let mut explicit_lock = requested;
    explicit_lock.provenance["explicitSourceLock"] = serde_json::Value::Bool(true);
    let prepared =
        crate::playback_sources::reconcile_stale_local_asset(&store, &explicit_lock, false)
            .unwrap();
    assert!(
        prepared.local_path.is_none(),
        "a pathless explicit lock must not discover a cross-file"
    );
    assert!(prepared.asset_id.is_none());
}
