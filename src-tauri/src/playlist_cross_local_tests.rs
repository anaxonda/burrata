use super::*;
use crate::models::{LocalAsset, Track};

fn provider_track(provider: &str, id: &str, title: &str, version: Option<&str>) -> Track {
    Track {
        id: format!("{provider}:{id}"),
        provider: provider.into(),
        provider_id: id.into(),
        source_url: match provider {
            "youtube" => format!("https://www.youtube.com/watch?v={id}"),
            _ => format!("https://example.test/{provider}/{id}"),
        },
        artist: "Amadou & Mariam".into(),
        title: title.into(),
        version: version.map(str::to_owned),
        duration_seconds: Some(294.214),
        ..Track::default()
    }
}

fn youtube_asset(path: &Path) -> LocalAsset {
    let track = provider_track("youtube", "abcdefghijk", "Baba", Some("Extended Mix"));
    LocalAsset {
        id: "fixture-youtube-file".into(),
        recording_id: track.id.clone(),
        path: path.to_string_lossy().into_owned(),
        hash: "fixture-content-hash".into(),
        source_codec: "flac".into(),
        final_codec: "flac".into(),
        source_bitrate: None,
        final_bitrate: None,
        sample_rate: Some(48_000),
        channels: Some(2),
        bit_depth: Some(24),
        duration_seconds: Some(294.2405),
        size_bytes: Some(1024),
        bitrate_origin: Some("format".into()),
        processing: "managed copy".into(),
        track,
    }
}

#[test]
fn cross_provider_asset_insert_delete_restore_invalidates_only_changed_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    fs::create_dir_all(&music).unwrap();
    let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
    let request = provider_track("spotify", "request-b", "Baba - Extended Mix", None);
    store.save_track(&request).unwrap();
    let collection = store
        .create_collection("Cross provider mirror fixture")
        .unwrap();
    store
        .add_to_collection(&collection.id, &[request.clone()])
        .unwrap();
    let unrelated = store.create_collection("Unrelated mirror fixture").unwrap();
    let other = provider_track("spotify", "unrelated", "Different recording", None);
    store.add_to_collection(&unrelated.id, &[other]).unwrap();
    let asset = youtube_asset(&music.join("cross-provider-baba.flac"));
    assert_ne!(
        request.id, asset.track.id,
        "fixture must use distinct recordings"
    );
    let settings = Settings {
        music_directory: music.to_string_lossy().into_owned(),
        playlist_mirroring_enabled: true,
        matching_level: crate::recording_match::MatchLevel::Balanced,
        ..Settings::default()
    };

    // The first pass writes the file, and the next verifies its new metadata
    // stamp. Keep a third pass so later assertions start from a clean cache.
    for _ in 0..3 {
        sync_playlists(&store, &settings).unwrap();
    }
    let state = read_state(&store).unwrap();
    let mirror = PathBuf::from(&state.mirror_paths[&collection.id]);
    let unrelated_path = PathBuf::from(&state.mirror_paths[&unrelated.id]);
    let unrelated_before = fs::metadata(&unrelated_path).unwrap().modified().unwrap();
    let initial = fs::read_to_string(&mirror).unwrap();
    assert!(initial.contains("burrata://track/"), "{initial}");

    let asset_path = music.join("cross-provider-baba.flac");
    fs::write(&asset_path, b"registered local audio fixture").unwrap();
    store.save_asset(&asset).unwrap();

    let mirror_work_before_insert = store.playlist_incremental.lock().unwrap().mirror_work;

    // This sync runs against the same Store/cache after the clean mirror was
    // recorded; a restart would clear the cache and hide a missed invalidation.
    sync_playlists(&store, &settings).unwrap();
    let with_local = fs::read_to_string(&mirror).unwrap();
    let relative = relative_path_resource(mirror.parent().unwrap(), &asset_path).unwrap();
    assert!(
        with_local.contains(&relative),
        "expected local resource {relative:?} in {with_local}"
    );
    assert!(!with_local.contains("burrata://track/"), "{with_local}");
    assert_eq!(
        fs::metadata(&unrelated_path).unwrap().modified().unwrap(),
        unrelated_before
    );
    assert_eq!(
        store.playlist_incremental.lock().unwrap().mirror_work,
        mirror_work_before_insert + 1,
        "only the affected mirror should render"
    );
    let after_write_mtime = fs::metadata(&mirror).unwrap().modified().unwrap();
    // One verification pass may be required because the successful render
    // itself changes the playlist file metadata used by the stamp.
    sync_playlists(&store, &settings).unwrap();
    assert_eq!(fs::read_to_string(&mirror).unwrap(), with_local);
    let local_mtime = fs::metadata(&mirror).unwrap().modified().unwrap();
    assert_eq!(
        after_write_mtime, local_mtime,
        "verification must not rewrite identical mirror contents"
    );

    sync_playlists(&store, &settings).unwrap();
    assert_eq!(fs::read_to_string(&mirror).unwrap(), with_local);
    assert_eq!(
        fs::metadata(&mirror).unwrap().modified().unwrap(),
        local_mtime,
        "idle sync should not rewrite the clean mirror"
    );
    let mirror_work_after_update = store.playlist_incremental.lock().unwrap().mirror_work;

    sync_playlists(&store, &settings).unwrap();
    assert_eq!(fs::read_to_string(&mirror).unwrap(), with_local);
    assert_eq!(
        fs::metadata(&mirror).unwrap().modified().unwrap(),
        local_mtime,
        "the fourth sync should still leave the clean mirror untouched"
    );
    assert_eq!(
        store.playlist_incremental.lock().unwrap().mirror_work,
        mirror_work_after_update,
        "the fourth sync should reuse the clean mirror stamp"
    );

    fs::remove_file(&asset_path).unwrap();
    sync_playlists(&store, &settings).unwrap();
    let without_local = fs::read_to_string(&mirror).unwrap();
    assert!(
        without_local.contains("burrata://track/"),
        "{without_local}"
    );
    assert!(!without_local.contains(&relative), "{without_local}");
    sync_playlists(&store, &settings).unwrap();

    fs::write(&asset_path, b"registered local audio fixture").unwrap();
    sync_playlists(&store, &settings).unwrap();
    let restored = fs::read_to_string(&mirror).unwrap();
    assert!(restored.contains(&relative), "{restored}");
    assert!(!restored.contains("burrata://track/"), "{restored}");
    sync_playlists(&store, &settings).unwrap();
}
