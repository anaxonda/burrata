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
    track.bpm = Some(99.);
    track.key = Some("Cm".into());
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

fn verified_spotify_bulgarian_track() -> Track {
    Track {
        id: "spotify:aaaaaaaaaaaaaaaaaaaaaa".into(),
        provider: "spotify".into(),
        provider_id: "aaaaaaaaaaaaaaaaaaaaaa".into(),
        source_url: "https://open.spotify.com/track/aaaaaaaaaaaaaaaaaaaaaa".into(),
        title: "Mir Stanke Le".into(),
        artist: "Bulgarian State Television Female Choir".into(),
        duration_seconds: Some(190.0),
        isrc: Some("USNO18716511".into()),
        provenance: serde_json::json!({
            "catalog":{"verified":true,"provider":"spotify","providerId":"aaaaaaaaaaaaaaaaaaaaaa","artistCount":1,"artists":[{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Bulgarian State Television Female Choir"}]},
            "remoteCatalog":{"trackId":"aaaaaaaaaaaaaaaaaaaaaa","title":"Mir Stanke Le","artist":"Bulgarian State Television Female Choir","durationSeconds":190.0,"isrc":"USNO18716511","artistCount":1,"artistRefs":[{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Bulgarian State Television Female Choir"}]}
        }),
        ..Default::default()
    }
}

fn install_verified_spotify_bulgarian_alias(store: &Store, request: &Track) {
    let recording_id = "00000000-0000-4000-8000-000000000091";
    let artist_id = "00000000-0000-4000-8000-000000000092";
    let recording = serde_json::json!({
        "id":recording_id,"title":request.title,"length":190000,"isrcs":["USNO18716511"],
        "artist-credit":[{"name":"Bulgarian State Television Female Choir","joinphrase":"","artist":{"id":artist_id,"name":"The Bulgarian State Radio and Television Female Vocal Choir"}}]
    });
    let profile = serde_json::json!({
        "id":artist_id,"name":"The Bulgarian State Radio and Television Female Vocal Choir",
        "aliases":[{"name":"Bulgarian State Television Female Choir"}],
        "relations":[{"url":{"resource":"https://open.spotify.com/artist/bbbbbbbbbbbbbbbbbbbbbb"}}]
    });
    store
        .set_provider_cache(
            "artists.spotify-recording-credit-crosswalk.musicbrainz-recording",
            recording_id,
            &recording,
            3600,
        )
        .unwrap();
    store
        .set_provider_cache(
            "artists.spotify-recording-credit-crosswalk.musicbrainz-artist",
            artist_id,
            &profile,
            3600,
        )
        .unwrap();
    crate::artist_credit_aliases::write_verified_spotify_artist_credit_recording(
        store,
        request,
        recording_id,
    )
    .unwrap();
}

fn verified_spotify_two_performer_track() -> Track {
    Track {
        id: "spotify:cccccccccccccccccccccc".into(),
        provider: "spotify".into(),
        provider_id: "cccccccccccccccccccccc".into(),
        source_url: "https://open.spotify.com/track/cccccccccccccccccccccc".into(),
        title: "Song of Two Choirs".into(),
        artist: "Choir Alpha & Choir Beta".into(),
        duration_seconds: Some(190.0),
        isrc: Some("USNO18716512".into()),
        provenance: serde_json::json!({
            "catalog":{"verified":true,"provider":"spotify","providerId":"cccccccccccccccccccccc","artistCount":2,"artists":[{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Choir Alpha"},{"provider":"spotify","id":"dddddddddddddddddddddd","name":"Choir Beta"}]},
            "remoteCatalog":{"trackId":"cccccccccccccccccccccc","title":"Song of Two Choirs","artist":"Choir Alpha & Choir Beta","durationSeconds":190.0,"isrc":"USNO18716512","artistCount":2,"artistRefs":[{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Choir Alpha","joinPhrase":" & "},{"provider":"spotify","id":"dddddddddddddddddddddd","name":"Choir Beta","joinPhrase":""}]}
        }),
        ..Default::default()
    }
}

fn install_verified_two_performer_alias(store: &Store, request: &Track) {
    let recording_id = "00000000-0000-4000-8000-000000000093";
    let first_artist_id = "00000000-0000-4000-8000-000000000094";
    let second_artist_id = "00000000-0000-4000-8000-000000000095";
    let recording = serde_json::json!({
        "id":recording_id,"title":request.title,"length":190000,"isrcs":["USNO18716512"],
        "artist-credit":[
            {"name":"Choir Alpha","joinphrase":" & ","artist":{"id":first_artist_id,"name":"The Alpha Choir"}},
            {"name":"Choir Beta","joinphrase":"","artist":{"id":second_artist_id,"name":"The Beta Choir"}}
        ]
    });
    for (artist_id, canonical, spotify_id, alias) in [
        (
            first_artist_id,
            "The Alpha Choir",
            "bbbbbbbbbbbbbbbbbbbbbb",
            "Choir Alpha",
        ),
        (
            second_artist_id,
            "The Beta Choir",
            "dddddddddddddddddddddd",
            "Choir Beta",
        ),
    ] {
        let profile = serde_json::json!({
            "id":artist_id,"name":canonical,"aliases":[{"name":alias}],
            "relations":[{"url":{"resource":format!("https://open.spotify.com/artist/{spotify_id}")}}]
        });
        store
            .set_provider_cache(
                "artists.spotify-recording-credit-crosswalk.musicbrainz-artist",
                artist_id,
                &profile,
                3600,
            )
            .unwrap();
    }
    store
        .set_provider_cache(
            "artists.spotify-recording-credit-crosswalk.musicbrainz-recording",
            recording_id,
            &recording,
            3600,
        )
        .unwrap();
    crate::artist_credit_aliases::write_verified_spotify_artist_credit_recording(
        store,
        request,
        recording_id,
    )
    .unwrap();
}

fn add_bulgarian_asset(
    dir: &tempfile::TempDir,
    store: &Store,
    id: &str,
    duration: f64,
    version: Option<&str>,
) {
    let path = dir.path().join(format!("{id}.flac"));
    store
        .save_asset(&asset(
            id,
            "Mir Stanke Le",
            version,
            "The Bulgarian State Radio and Television Female Vocal Choir",
            Some(duration),
            Some("USNO18716511"),
            &path,
            true,
        ))
        .unwrap();
}

#[test]
fn verified_spotify_musicbrainz_artist_alias_enables_bounded_local_reuse_and_availability() {
    let (dir, store) = db();
    let request = verified_spotify_bulgarian_track();
    store.save_track(&request).unwrap();
    install_verified_spotify_bulgarian_alias(&store, &request);
    add_bulgarian_asset(&dir, &store, "bulgarian-verified", 191.0, None);

    let aliases = crate::artist_credit_aliases::cached_local_playback_artist_aliases(
        &store.lock_connection().unwrap(),
        &request,
    );
    assert!(
        aliases.tracks().iter().any(|alias| {
            alias.artist == "The Bulgarian State Radio and Television Female Vocal Choir"
        }),
        "verified crosswalk must produce the local credited spelling; got: {:?}",
        aliases
            .tracks()
            .iter()
            .map(|alias| &alias.artist)
            .collect::<Vec<_>>()
    );
    let candidate_asset = asset(
        "bulgarian-verified",
        "Mir Stanke Le",
        None,
        "The Bulgarian State Radio and Television Female Vocal Choir",
        Some(191.0),
        Some("USNO18716511"),
        &dir.path().join("bulgarian-verified.flac"),
        false,
    );
    assert!(
        crate::storage_local_matches::accepts_with_aliases(
            &request,
            &candidate_asset,
            &Settings::default(),
            &aliases,
        ),
        "verified spelling is available but exact scorer rejects it"
    );

    let loaded = store.get_track(&request.id).unwrap().unwrap();
    assert_eq!(loaded.asset_id.as_deref(), Some("bulgarian-verified"));
    assert_eq!(
        loaded.local_path.as_deref(),
        Some(
            dir.path()
                .join("bulgarian-verified.flac")
                .to_string_lossy()
                .as_ref()
        )
    );
    assert!(page(&store, "library", None, Some("local"), None)
        .iter()
        .any(|track| track.id == request.id));
}

#[test]
fn verified_multi_performer_alias_keeps_structured_credit_boundaries() {
    let (dir, store) = db();
    let request = verified_spotify_two_performer_track();
    store.save_track(&request).unwrap();
    install_verified_two_performer_alias(&store, &request);
    let path = dir.path().join("two-performer.flac");
    store
        .save_asset(&asset(
            "two-performer",
            "Song of Two Choirs",
            None,
            "The Alpha Choir & The Beta Choir",
            Some(191.0),
            Some("USNO18716512"),
            &path,
            true,
        ))
        .unwrap();

    let loaded = store.get_track(&request.id).unwrap().unwrap();
    assert_eq!(loaded.asset_id.as_deref(), Some("two-performer"));
}

#[test]
fn local_artist_alias_requires_verified_crosswalk_and_exact_version_and_runtime() {
    for (id, verified, duration, version) in [
        ("unverified", false, 191.0, None),
        ("wrong-runtime", true, 197.0, None),
        ("wrong-version", true, 191.0, Some("Live")),
    ] {
        let (dir, store) = db();
        let request = verified_spotify_bulgarian_track();
        store.save_track(&request).unwrap();
        if verified {
            install_verified_spotify_bulgarian_alias(&store, &request);
        }
        add_bulgarian_asset(&dir, &store, id, duration, version);
        let loaded = store.get_track(&request.id).unwrap().unwrap();
        assert!(
            loaded.asset_id.is_none(),
            "{id} must not reuse a local file"
        );
        assert!(
            loaded.local_path.is_none(),
            "{id} must not expose a local path"
        );
    }

    let (dir, store) = db();
    let mut request = verified_spotify_bulgarian_track();
    request.title = "Mir Stanke Le (Harvest Song from Thrace)".into();
    request.provenance["remoteCatalog"]["title"] = serde_json::json!(request.title);
    store.save_track(&request).unwrap();
    install_verified_spotify_bulgarian_alias(&store, &request);
    add_bulgarian_asset(&dir, &store, "unknown-title-gloss", 191.0, None);
    let loaded = store.get_track(&request.id).unwrap().unwrap();
    assert!(loaded.asset_id.is_none());
    assert!(loaded.local_path.is_none());
}

#[test]
fn ambiguous_files_matching_a_verified_artist_alias_fail_closed() {
    let (dir, store) = db();
    let request = verified_spotify_bulgarian_track();
    store.save_track(&request).unwrap();
    install_verified_spotify_bulgarian_alias(&store, &request);
    add_bulgarian_asset(&dir, &store, "bulgarian-one", 191.0, None);
    add_bulgarian_asset(&dir, &store, "bulgarian-two", 191.0, None);
    let loaded = store.get_track(&request.id).unwrap().unwrap();
    assert!(loaded.asset_id.is_none());
    assert!(loaded.local_path.is_none());
    assert!(!page(&store, "library", None, Some("local"), None)
        .iter()
        .any(|track| track.id == request.id));
}

#[test]
fn manually_edited_artist_invalidates_cached_alias_even_when_listing_json_is_stale() {
    let (dir, store) = db();
    let original = verified_spotify_bulgarian_track();
    store.save_track(&original).unwrap();
    install_verified_spotify_bulgarian_alias(&store, &original);
    add_bulgarian_asset(&dir, &store, "bulgarian-manual-artist", 191.0, None);

    let mut edited = original.clone();
    edited.artist = "Different manually edited artist".into();
    let saved = store.update_track(&edited).unwrap();
    assert_eq!(saved.artist, edited.artist);
    assert_eq!(saved.provenance["manualFields"]["artist"], true);
    assert!(
        saved.asset_id.is_none(),
        "a stale raw listing must not restore the cached alias"
    );
    assert!(saved.local_path.is_none());
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
    assert_eq!(loaded.bpm, requested.bpm);
    assert_eq!(loaded.key, requested.key);
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

#[test]
fn reviewed_portable_source_disables_cross_file_hydration_and_preflight_reuse() {
    let (dir, store) = db();
    let mut requested = request();
    requested.provenance["portablePlaybackSource"] = serde_json::json!({
        "provider": "monochrome",
        "sourceId": "reviewed-source",
        "matchingLevel": "exact",
        "sourceHash": "a".repeat(64)
    });
    store.save_track(&requested).unwrap();
    add_matching_asset(&dir, &store, "portable-locked-file");

    let loaded = store.get_track(&requested.id).unwrap().unwrap();
    assert!(loaded.asset_id.is_none());
    assert!(loaded.local_path.is_none());

    let mut queued_with_retained_file = requested;
    queued_with_retained_file.asset_id = Some("portable-locked-file".into());
    queued_with_retained_file.local_path = Some(
        dir.path()
            .join("portable-locked-file.flac")
            .to_string_lossy()
            .into_owned(),
    );
    queued_with_retained_file.local_availability = "available".into();
    let prepared = crate::playback_sources::reconcile_stale_local_asset(
        &store,
        &queued_with_retained_file,
        true,
    )
    .unwrap();
    assert!(prepared.asset_id.is_none());
    assert!(prepared.local_path.is_none());
}

#[test]
fn retained_audio_candidate_mismatch_overrides_matching_local_file_tags() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    let path = dir.path().join("tag-match-wrong-source.flac");
    let mut tagged = asset(
        "tag-match-wrong-source",
        "Baba",
        Some("Extended Mix"),
        "Amadou & Mariam",
        Some(294.2405),
        None,
        &path,
        true,
    );
    let observed_source = crate::models::Candidate {
        id: "youtube:wrong-recording".into(),
        provider: "youtube".into(),
        url: "https://www.youtube.com/watch?v=wrong-recording".into(),
        title: "Different Song".into(),
        uploader: Some("Different Artist".into()),
        duration_seconds: Some(294.2405),
        source_metadata: Some(serde_json::json!({
            "track": "Different Song",
            "artist": "Different Artist",
            "artists": ["Different Artist"],
            "artistCount": 1,
            "duration": 294.2405,
            "sourceMetadataOrigin": "ytDlpFullInfo"
        })),
        ..Default::default()
    };
    tagged.track.provenance["audioCandidate"] = serde_json::to_value(observed_source).unwrap();
    store.save_asset(&tagged).unwrap();

    let loaded = store.get_track(&requested.id).unwrap().unwrap();
    assert!(
        loaded.asset_id.is_none(),
        "stale matching tags cannot override the retained source"
    );
    assert!(loaded.local_path.is_none());
}

#[test]
fn cross_recording_match_requires_measured_asset_duration_but_canonical_file_does_not() {
    let (dir, store) = db();
    let requested = request();
    store.save_track(&requested).unwrap();
    let path = dir.path().join("unmeasured-cross-file.flac");
    let mut unmeasured = asset(
        "unmeasured-cross-file",
        "Baba",
        Some("Extended Mix"),
        "Amadou & Mariam",
        Some(294.2405),
        None,
        &path,
        true,
    );
    unmeasured.duration_seconds = None;
    assert!(unmeasured.track.duration_seconds.is_some());
    store.save_asset(&unmeasured).unwrap();
    let loaded = store.get_track(&requested.id).unwrap().unwrap();
    assert!(loaded.asset_id.is_none());
    assert!(loaded.local_path.is_none());

    let (canonical_dir, canonical_store) = db();
    let canonical = request();
    canonical_store.save_track(&canonical).unwrap();
    let canonical_path = canonical_dir.path().join("canonical-unmeasured.flac");
    let mut own_asset = asset(
        "canonical-unmeasured",
        &canonical.title,
        None,
        &canonical.artist,
        canonical.duration_seconds,
        None,
        &canonical_path,
        true,
    );
    own_asset.recording_id = canonical.id.clone();
    own_asset.track = canonical.clone();
    own_asset.duration_seconds = None;
    canonical_store.save_asset(&own_asset).unwrap();
    let canonical_loaded = canonical_store.get_track(&canonical.id).unwrap().unwrap();
    assert_eq!(
        canonical_loaded.asset_id.as_deref(),
        Some("canonical-unmeasured")
    );
    assert_eq!(
        canonical_loaded.local_path.as_deref(),
        Some(canonical_path.to_str().unwrap())
    );
}

#[test]
fn local_match_indexes_allow_raw_sqlite_backup_integrity_and_writes() {
    let (dir, store) = db();
    store.save_track(&request()).unwrap();
    add_matching_asset(&dir, &store, "portable-index");
    drop(store);

    // Backup/restore and older app connections do not register our query UDFs.
    let conn = Connection::open(dir.path().join("local-matches.sqlite")).unwrap();
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    conn.execute("UPDATE local_assets SET payload=payload", [])
        .unwrap();
    conn.execute("UPDATE catalog_listings SET metadata=metadata", [])
        .unwrap();
    conn.execute("UPDATE recordings SET manual_metadata=manual_metadata", [])
        .unwrap();
}
