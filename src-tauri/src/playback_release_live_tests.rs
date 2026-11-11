//! Opt-in resolver checks using a caller-owned catalog fixture and tool directory.
//! This qualifies source resolution, not installed audio-device output.
use super::*;

async fn resolve_fixture_track(index: usize) {
    let fixture_path = std::env::var("BURRATA_LIVE_TRACK_FIXTURE").expect("set fixture path");
    let tools = std::path::PathBuf::from(
        std::env::var("BURRATA_LIVE_TOOLS_DIR").expect("set tools directory"),
    );
    let bytes = std::fs::read(fixture_path).unwrap();
    let fixture: Value = serde_json::from_slice(&bytes).unwrap();
    let mut track: Track = serde_json::from_value(fixture["tracks"][index].clone()).unwrap();
    track.provenance["representativeEdition"] = json!(false);
    track.provenance["requestedReleaseSourceUrl"] = json!(track.source_url);
    track.provenance["libraryAlbumSnapshot"] = json!({"complete":true,"total":12});
    let temp = tempfile::tempdir().unwrap();
    let music = temp.path().join("music");
    std::fs::create_dir(&music).unwrap();
    let mut settings = Settings::default();
    settings.music_directory = music.to_string_lossy().into_owned();
    settings.yt_dlp_path = tools.join("yt-dlp").to_string_lossy().into_owned();
    settings.ffmpeg_path = tools.join("ffmpeg").to_string_lossy().into_owned();
    settings.ffprobe_path = tools.join("ffprobe").to_string_lossy().into_owned();
    settings.js_runtime_path = tools.join("node").to_string_lossy().into_owned();
    settings.preferred_streaming_source = "youtube".into();
    settings.streaming_fallback_enabled = false;
    settings.playback_startup_timeout_seconds = 20;
    settings.matching_level = crate::recording_match::MatchLevel::Flexible;
    settings.flexible_matching.allow_other_releases = true;
    settings.flexible_matching.allow_other_performances = true;
    settings.flexible_matching.allow_other_mixes = true;
    settings.flexible_matching.allow_other_artists = false;
    settings.flexible_matching.allow_different_lengths = true;
    settings.flexible_matching.max_shorter_by_seconds = 12;
    settings.flexible_matching.max_longer_by_seconds = 45;
    let store = Arc::new(Store::open(&temp.path().join("library.sqlite")).unwrap());
    store.save_settings(&settings).unwrap();
    store.save_track(&track).unwrap();
    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
    let (_cancel_tx, cancel) = watch::channel(false);
    crate::playback_diagnostics::set_enabled(true);
    crate::playback_diagnostics::clear();
    let start = Instant::now();
    let result = resolve(&store, &acquisition, &track, None, cancel).await;
    eprintln!(
        "LIVE_RELEASE_RESOLUTION={}",
        json!({
            "fixtureSha256":format!("{:x}", Sha256::digest(&bytes)),
            "trackIndex":index,"elapsedMs":start.elapsed().as_millis(),
            "resolved":result.is_ok(),
            "candidateId":result.as_ref().ok().and_then(|r| r.candidate.as_ref()).map(|c| &c.id),
            "diagnostics":crate::playback_diagnostics::export()
        })
    );
    crate::playback_diagnostics::set_enabled(false);
    assert!(result.is_ok(), "fixture track {index} did not resolve");
}

#[tokio::test]
#[ignore = "requires caller-provided live catalog fixture, tools and public providers"]
async fn flexible_swing42_resolves() {
    resolve_fixture_track(0).await;
}

#[tokio::test]
#[ignore = "requires caller-provided live catalog fixture, tools and public providers"]
async fn flexible_minor_swing_resolves() {
    resolve_fixture_track(1).await;
}
