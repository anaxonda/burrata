use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

#[tokio::test]
async fn stale_saved_mapping_allows_one_fresh_lookup_without_rewriting_the_recording() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&workspace.path().join("saved-startup.sqlite")).unwrap());
    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);
    let track = Track {
        id: "fixture:saved-recording".into(),
        provider: "fixture".into(),
        provider_id: "saved-recording".into(),
        title: "Fixture Recording".into(),
        artist: "Fixture Artist".into(),
        duration_seconds: Some(240.0),
        ..Track::default()
    };
    store.save_track(&track).unwrap();
    let saved = store.get_track(&track.id).unwrap().unwrap();

    let extractor = workspace.path().join("fake-yt-dlp");
    let stale_url = "https://soundcloud.com/fixture/111";
    let fresh_url = "https://soundcloud.com/fixture/222";
    let search = json!({"entries":[
        {"id":"111","title":track.title,"artist":track.artist,"uploader":track.artist,
         "duration":240,"webpage_url":stale_url},
        {"id":"222","title":track.title,"artist":track.artist,"uploader":track.artist,
         "duration":240,"webpage_url":fresh_url}
    ]});
    let extraction = |id, title| {
        json!({
            "id":id,"title":title,"track":title,"artist":track.artist,"artists":[track.artist],
            "uploader":track.artist,"duration":240,"url":"https://cf-media.sndcdn.com/fixture.m4a",
            "acodec":"aac","vcodec":"none","protocol":"https","ext":"m4a"
        })
    };
    std::fs::write(
        &extractor,
        format!(
            r#"#!/bin/sh
for arg do last=$arg; done
case "$last" in
    scsearch*) printf '%s\n' '{search}' ;;
    {stale_url}) printf '%s\n' '{}' ;;
    {fresh_url}) printf '%s\n' '{}' ;;
    *) exit 1 ;;
esac
"#,
            extraction("111", "A Different Recording"),
            extraction("222", "Fixture Recording"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&extractor, std::fs::Permissions::from_mode(0o700)).unwrap();
    let settings = Settings {
        yt_dlp_path: extractor.to_string_lossy().into_owned(),
        preferred_streaming_source: "soundcloud".into(),
        streaming_fallback_enabled: false,
        matching_level: crate::recording_match::MatchLevel::Balanced,
        ..Settings::default()
    };
    let fingerprint = policy_matching::request_fingerprint_with_options(
        &saved,
        settings.matching_level,
        settings.flexible_matching,
    )
    .unwrap();
    store
        .save_verified_playback_source(&saved.id, &fingerprint, "soundcloud", "111", stale_url)
        .unwrap();

    let (stop, cancel) = watch::channel(false);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let trace = SourceTrace::new(Some("native-saved-startup-fixture".into()), None, None)
        .with_test_sink(Arc::new(move |event| {
            if event["phase"] == "full_identity_check" && event["details"]["ok"] == true {
                // The replacement is verified as a recording, but access
                // still needs validation before its mapping can be persisted.
                stop.send_replace(true);
            }
            sink.lock().unwrap().push(event);
        }));
    let deadline = PlaybackDeadline::after(Duration::from_secs(8));
    let result = crate::acquisition::with_playback_test_public_addresses(within_shared_deadline(
        deadline,
        Box::pin(resolve_serial(
            &store,
            &acquisition,
            &saved,
            None,
            cancel,
            settings,
            &trace,
            &[],
            deadline,
            2,
            &AliasResolutionPass::default(),
        )),
    ))
    .await;
    let events = events.lock().unwrap();
    let phases = events
        .iter()
        .map(|event| &event["phase"])
        .collect::<Vec<_>>();
    assert_eq!(result.err().as_deref(), Some("Cancelled"), "{phases:?}");
    assert!(events.iter().any(|event| {
        event["phase"] == "mapping_identity_check" && event["details"]["ok"] == false
    }));
    for phase in ["search_start", "full_identity_check"] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event["phase"] == phase)
                .count(),
            1,
            "{phase}"
        );
    }
    assert_eq!(trace.extraction_attempts_used(), 2);
    assert!(store
        .verified_playback_source(&saved.id, &fingerprint, "soundcloud")
        .unwrap()
        .is_none());
    assert_eq!(
        serde_json::to_value(store.get_track(&saved.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(saved).unwrap(),
    );
}

#[tokio::test]
#[ignore = "explicit public-provider startup comparison using isolated fixture rows and no audio device"]
async fn live_bacc_saved_discogs_fixture_versus_fresh_source_lookup() {
    assert_eq!(
        std::env::var("BURRATA_LIVE_BACC_STARTUP").as_deref(),
        Ok("1"),
        "explicit live provider test opt-in is required"
    );
    let fixture: Vec<Track> = serde_json::from_str(include_str!(
        "../../scripts/performance/fixtures/nipsey-saved-tracks.json"
    ))
    .unwrap();
    let saved_fixture = fixture.into_iter().next().unwrap();
    assert_eq!(saved_fixture.title, "Bacc Of The Bach");
    let resources = std::path::PathBuf::from(
        std::env::var_os("BURRATA_LIVE_BACC_RESOURCES").expect("packaged tool resources"),
    );
    crate::tools::initialize(resources.clone());
    let helper_state = tempfile::tempdir().unwrap();
    let server = resources.join("youtube-support/bgutil/server/build/main.js");
    let plugin = resources.join("youtube-support/bgutil/plugin");
    assert!(
        server.is_file() && plugin.is_dir(),
        "managed YouTube support"
    );
    let manager = crate::youtube_support::YouTubeSupportManager::new(
        helper_state.path(),
        crate::tools::bundled_executable("yt-dlp").expect("packaged yt-dlp"),
        Some(crate::tools::bundled_executable("node").expect("packaged Node")),
        Some(server),
        Some(plugin),
    );
    crate::youtube_support::initialize_global(manager.clone()).unwrap();
    let mut failures = Vec::new();
    let public_discogs = std::env::var("BURRATA_LIVE_BACC_PUBLIC_DISCOGS").as_deref() == Ok("1");
    let modes: &[&str] = if public_discogs {
        &["saved_discogs_fixture_with_public_discogs"]
    } else {
        &["saved_discogs_fixture", "fresh_provider_lookup"]
    };
    for &mode in modes {
        let workspace = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&workspace.path().join("live-startup.sqlite")).unwrap());
        if public_discogs {
            store
                .set_json_preference("connections", &json!({"discogsEnabled": true}))
                .unwrap();
            // The comparison must use anonymous public data only. This
            // private preference suppresses OS keyring reads for Discogs.
            store
                .set_json_preference("connections_session_only", &json!(["discogs"]))
                .unwrap();
        }
        let mut settings = store.settings().unwrap();
        settings.preferred_streaming_source = "youtube".into();
        settings.streaming_fallback_enabled = true;
        settings.streaming_fallback_sources = vec!["youtube".into(), "soundcloud".into()];
        settings.matching_level = crate::recording_match::MatchLevel::Balanced;
        store.save_settings(&settings).unwrap();
        let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        if public_discogs {
            assert!(connections.settings().unwrap().discogs_enabled);
            assert_eq!(connections.credential("discogs").unwrap(), None);
        }
        let acquisition = AcquisitionManager::new_for_playback_test(store.clone(), connections);

        let mut track = saved_fixture.clone();
        if mode != "fresh_provider_lookup" {
            store.save_track(&track).unwrap();
            track = store.get_track(&track.id).unwrap().unwrap();
        } else {
            let provenance = track.provenance.as_object_mut().unwrap();
            for key in ["audioCandidate", "candidateUrl", "audioMatchStatus"] {
                provenance.remove(key);
            }
        }

        let (_stop, cancel) = watch::channel(false);
        let started = std::time::Instant::now();
        let deadline = PlaybackDeadline::after(Duration::from_secs(20));
        let timings = Arc::new(Mutex::new(Vec::new()));
        let sink = timings.clone();
        let trace = SourceTrace::new(Some(format!("native-live-bacc-{mode}")), None, None)
            .with_test_sink(Arc::new(move |event| {
                if matches!(
                    event["phase"].as_str(),
                    Some(
                        "mapping_resolve_end"
                            | "catalog_duration_lookup_end"
                            | "search_end"
                            | "extraction_end"
                            | "full_identity_check"
                            | "extract_and_validate_end"
                    )
                ) {
                    sink.lock().unwrap().push(json!({
                        "phase":event["phase"],
                        "elapsedMs":event["elapsedMs"],
                        "provider":event["provider"],
                        "ok":event["details"]["ok"],
                        "accepted":event["details"]["accepted"],
                    }));
                }
            }));
        let resolved = within_shared_deadline(
            deadline,
            Box::pin(resolve_inner(
                &store,
                &acquisition,
                &track,
                None,
                cancel.clone(),
                &trace,
                &[],
                deadline,
                2,
                false,
            )),
        )
        .await;
        let resolved_ms = started.elapsed().as_millis();
        let mut evidence = json!({
            "mode":mode,
            "resolveMs":resolved_ms,
            "timings":*timings.lock().unwrap()
        });
        match resolved {
            Err(error) => {
                let failure = crate::playback_failure::classify(&error);
                evidence["failure"] = json!(failure.category);
                evidence["httpStatus"] = json!(failure.http_status);
                failures.push(mode);
            }
            Ok(resolved) => {
                evidence["provider"] =
                    json!(resolved.candidate.as_ref().map(|c| c.provider.as_str()));
                let pcm = crate::native_output::direct_pcm_experiment::collect_until(
                    resolved.source,
                    0.0,
                    1.0,
                    started + Duration::from_secs(20),
                    cancel,
                )
                .await;
                evidence["decodeFirstPcmMs"] = json!(pcm.first_pcm_ms);
                evidence["decodedFrames"] = json!(pcm.decoded_frames);
                if !pcm.ok || pcm.decoded_frames == 0 || pcm.first_pcm_ms.is_none() {
                    failures.push(mode);
                }
            }
        }
        eprintln!("{evidence}");
    }
    manager.shutdown().await.unwrap();
    assert!(
        failures.is_empty(),
        "startup comparison failures: {failures:?}"
    );
}
