use super::*;
use serde_json::json;

fn request() -> Track {
    Track {
        title: "Vibe With Me".into(),
        artist: "Nipsey Hussle,Bino Rideaux,Leon Thomas".into(),
        ..Track::default()
    }
}
fn candidate(title: &str) -> Candidate {
    Candidate {
        id: "youtube:ExdT24gJngk".into(),
        provider: "youtube".into(),
        title: title.into(),
        uploader: Some("Nipsey Hussle".into()),
        duration_seconds: Some(232.),
        source_metadata: Some(
            json!({"catalogTitle":title,"track":title,"duration":232,"extractor":true,"resolvedProviderId":"ExdT24gJngk","sourceMetadataOrigin":"ytDlpCandidateMetadata"}),
        ),
        ..Candidate::default()
    }
}

#[test]
fn complete_caption_variants_match_only_after_resolution() {
    for title in [
        "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)",
        "Nipsey Hussle × Bino Rideaux – Vibe With Me [featuring Leon Thomas] [Official Audio]",
        "Nipsey Hussle & Bino Rideaux - Vibe With Me[feat. Leon Thomas][Official Audio]",
        "Nipsey Hussle & Bino Rideaux - Vibe With Me (Official Audio) (feat. Leon Thomas)",
    ] {
        for (stage, expected) in [
            (MatchStage::Search, MatchAction::Inspect),
            (MatchStage::Resolved, MatchAction::Accept),
        ] {
            let a = assess(&request(), &candidate(title), MatchLevel::Balanced, stage);
            assert_eq!(a.action, expected, "{title}: {stage:?}: {a:?}");
        }
    }
}

#[test]
fn caption_versions_and_credit_conflicts_are_never_accepted() {
    for title in [
        "Nipsey Hussle x Bino Rideaux - Vibe With Me[feat. Leon Thomas][Live][Official Audio]",
        "Nipsey Hussle x Bino Rideaux - Vibe With Me (Remix) (feat. Leon Thomas) (Official Audio)",
        "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Other Guest) (Official Audio)",
        "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas & Other Guest) (Official Audio)",
        "Nipsey Hussle Tribute x Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)",
        "Nipsey Hussle x Bino Rideaux - Vibe With Me[Unclassified Session][feat. Leon Thomas][Official Audio]",
    ] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let a = assess(&request(), &candidate(title), MatchLevel::Balanced, stage);
            assert_ne!(a.action, MatchAction::Accept, "{title}: {stage:?}: {a:?}");
        }
    }
}

#[test]
fn restored_edition_remains_recording_version_evidence() {
    let requested = Track {
        title: "Swing 42 (Restored Edition '25)".into(),
        artist: "Django Reinhardt & Quintette du Hot Club de France".into(),
        duration_seconds: Some(165.0),
        provenance: json!({"artistCredits":[
            {"name":"Django Reinhardt","role":"artist credit","joinPhrase":" & "},
            {"name":"Quintette du Hot Club de France","role":"artist credit","joinPhrase":""}
        ]}),
        ..Track::default()
    };
    let row = |title: &str| Candidate {
        id: "youtube:ExdT24gJngk".into(),
        provider: "youtube".into(),
        title: title.into(),
        duration_seconds: Some(165.0),
        source_metadata: Some(json!({"provider":"youtube","track":title,
            "artists":["Django Reinhardt","Quintette du Hot Club de France"],"artistCount":2,"duration":165.0})),
        ..Candidate::default()
    };
    for stage in [MatchStage::Search, MatchStage::Resolved] {
        let matched = assess(
            &requested,
            &row("Swing 42 (Restored Edition '25)"),
            MatchLevel::Balanced,
            stage,
        );
        assert_eq!(
            matched.dimensions.title,
            Comparison::Agree,
            "{stage:?}: {matched:?}"
        );
        assert_eq!(
            matched.dimensions.version,
            Comparison::Agree,
            "{stage:?}: {matched:?}"
        );
        if stage == MatchStage::Resolved {
            assert_eq!(matched.action, MatchAction::Accept, "{matched:?}");
        } else {
            assert_ne!(matched.action, MatchAction::Reject, "{matched:?}");
        }
        for title in [
            "Swing 42 (Restored Edition '24)",
            "Swing 42 (Live)",
            "Swing 42 (Remastered)",
            "Swing 42",
        ] {
            let other = assess(&requested, &row(title), MatchLevel::Balanced, stage);
            assert_ne!(
                other.action,
                MatchAction::Accept,
                "{title}: {stage:?}: {other:?}"
            );
            if title.contains("'24") || title.contains("Live") {
                assert_eq!(
                    other.dimensions.version,
                    Comparison::Conflict,
                    "{title}: {other:?}"
                );
            }
        }
    }
}

#[test]
fn swing_remastered_2002_flexible_policy_qualification() {
    let track = Track {
        id: "catalog:musicbrainz:4d654402-6a53-46c5-bf68-26c19e4d0fa9:1:1:1:ec12bebd-e1b9-42ff-892b-4aa4825ac8ea".into(),
        provider: "catalog".into(),
        provider_id: "musicbrainz:4d654402-6a53-46c5-bf68-26c19e4d0fa9:1:1:1:ec12bebd-e1b9-42ff-892b-4aa4825ac8ea".into(),
        artist: "Django Reinhardt & Quintette du Hot Club de France".into(),
        title: "Swing 42 (Restored Edition '25)".into(), duration_seconds: Some(165.0),
        provenance: json!({"artistCredits":[
            {"name":"Django Reinhardt","role":"artist credit","joinPhrase":" & "},
            {"name":"Quintette du Hot Club de France","role":"artist credit","joinPhrase":""}
        ]}), ..Track::default()
    };
    let title = "Swing 42 (Remastered 2002)";
    let candidate = Candidate {
        provider: "youtube".into(),
        id: "youtube:AVgvKGkxDLc".into(),
        title: title.into(),
        duration_seconds: Some(147.0),
        source_metadata: Some(
            json!({"provider":"youtube","track":title,"artists":["Django Reinhardt","The Quintet of the Hot Club of France","Stéphane Grappelli"],"artistCount":3,"duration":147.0,"album":"Djangology","channel":"Django Reinhardt"}),
        ),
        ..Candidate::default()
    };
    for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let result = assess(&track, &candidate, level, stage);
            println!("{level:?} {stage:?}: {result:?}");
            assert_ne!(
                result.action,
                MatchAction::Accept,
                "unverified performer and version: {result:?}"
            );
        }
    }
}

#[test]
fn flexible_options_gate_explicit_alternatives_independently() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let candidate = |title: &str| Candidate {
        provider: "youtube".into(),
        id: "youtube:abcdefghijk".into(),
        title: title.into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"track":title,"artists":["Artist"],"artistCount":1,"duration":200.0}),
        ),
        ..Candidate::default()
    };
    let defaults = FlexibleMatchingOptions::default();
    for title in ["Song (Live)", "Song (Club Remix)"] {
        let result = assess_with_options(
            &track,
            &candidate(title),
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &defaults,
        );
        assert_ne!(result.action, MatchAction::Accept, "{title}: {result:?}");
    }
    let enabled = FlexibleMatchingOptions {
        allow_other_performances: true,
        ..defaults
    };
    let live = assess_with_options(
        &track,
        &candidate("Song (Live)"),
        MatchLevel::Flexible,
        MatchStage::Resolved,
        &enabled,
    );
    assert_eq!(live.action, MatchAction::Accept, "{live:?}");
    assert_eq!(live.alternative_kind, Some(AlternativeKind::Performance));
    let remix = assess_with_options(
        &track,
        &candidate("Song (Club Remix)"),
        MatchLevel::Flexible,
        MatchStage::Resolved,
        &enabled,
    );
    assert_ne!(remix.action, MatchAction::Accept, "{remix:?}");
    let balanced = assess_with_options(
        &track,
        &candidate("Song (Live)"),
        MatchLevel::Balanced,
        MatchStage::Resolved,
        &enabled,
    );
    assert_ne!(balanced.action, MatchAction::Accept, "{balanced:?}");
}

#[test]
fn flexible_combined_versions_require_each_permission() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let title = "Song (Live Remix)";
    let candidate = Candidate {
        provider: "youtube".into(),
        id: "youtube:abcdefghijk".into(),
        title: title.into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"track":title,"artists":["Artist"],"artistCount":1,"duration":200.0}),
        ),
        ..Candidate::default()
    };
    for (performances, mixes, expected) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let options = FlexibleMatchingOptions {
            allow_other_performances: performances,
            allow_other_mixes: mixes,
            ..Default::default()
        };
        let a = assess_with_options(
            &track,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            a.action == MatchAction::Accept,
            expected,
            "{performances} {mixes}: {a:?}"
        );
        if expected {
            assert_eq!(a.alternative_kind, Some(AlternativeKind::Remix));
        }
    }
}

#[test]
fn flexible_longer_alternative_requires_single_bound_recording() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let candidate = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: "Song (Live)".into(),
        duration_seconds: Some(400.0),
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Live)","artistNames":["Artist"],"artistCount":1,"duration":400.0}),
        ),
        ..Candidate::default()
    };
    let mut options = FlexibleMatchingOptions {
        allow_other_performances: true,
        ..Default::default()
    };
    let test = |row: &Candidate, options: &FlexibleMatchingOptions| {
        assess_with_options(
            &track,
            row,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            options,
        )
    };
    assert_ne!(test(&candidate, &options).action, MatchAction::Accept);
    options.allow_different_lengths = true;
    assert_eq!(test(&candidate, &options).action, MatchAction::Accept);
    let mut album = candidate.clone();
    album.source_metadata.as_mut().unwrap()["isFullAlbum"] = json!(true);
    assert_ne!(test(&album, &options).action, MatchAction::Accept);
    let mut preview = candidate.clone();
    preview.source_metadata.as_mut().unwrap()["isPreview"] = json!(true);
    assert_ne!(test(&preview, &options).action, MatchAction::Accept);
    let mut unbound = candidate.clone();
    unbound.id = "monochrome:456".into();
    assert_ne!(test(&unbound, &options).action, MatchAction::Accept);
}

#[test]
fn flexible_other_artist_requires_explicit_permission_and_full_credit() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        provenance: json!({"workIds":["work-1"]}),
        ..Track::default()
    };
    let mut row = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song","artistNames":["Other Artist"],"artistCount":1,"duration":200.0,"workId":"work-1"}),
        ),
        ..Candidate::default()
    };
    let mut options = FlexibleMatchingOptions::default();
    let test = |c: &Candidate, o: &FlexibleMatchingOptions| {
        assess_with_options(&track, c, MatchLevel::Flexible, MatchStage::Resolved, o)
    };
    assert_ne!(test(&row, &options).action, MatchAction::Accept);
    options.allow_other_artists = true;
    assert_eq!(test(&row, &options).action, MatchAction::Accept);
    assert_eq!(
        test(&row, &options).alternative_kind,
        Some(AlternativeKind::OtherArtist)
    );
    row.source_metadata.as_mut().unwrap()["artistNames"] = json!([]);
    assert_ne!(test(&row, &options).action, MatchAction::Accept);
}

#[test]
fn flexible_release_choice_requires_release_permission() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song (Restored Edition '25)".into(),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let row = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: "Song (Remastered 2002)".into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Remastered 2002)","artistNames":["Artist"],"artistCount":1,"duration":200.0}),
        ),
        ..Candidate::default()
    };
    let mut options = FlexibleMatchingOptions::default();
    let test = |level, o: &FlexibleMatchingOptions| {
        assess_with_options(&track, &row, level, MatchStage::Resolved, o)
    };
    assert_eq!(
        test(MatchLevel::Flexible, &options).action,
        MatchAction::Accept
    );
    assert_eq!(
        test(MatchLevel::Flexible, &options).alternative_kind,
        Some(AlternativeKind::Release)
    );
    assert_ne!(
        test(MatchLevel::Balanced, &options).action,
        MatchAction::Accept
    );
    options.allow_other_releases = false;
    assert_ne!(
        test(MatchLevel::Flexible, &options).action,
        MatchAction::Accept
    );
}

#[test]
fn cover_requires_other_artist_and_verified_work() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        provenance: json!({"workIds":["work-1"]}),
        ..Track::default()
    };
    let row = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: "Song (Cover)".into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Cover)","artistNames":["Other Artist"],"artistCount":1,"duration":200.0,"workId":"work-1"}),
        ),
        ..Candidate::default()
    };
    for (performance, artist, accepted) in [
        (false, false, false),
        (true, false, false),
        (false, true, true),
        (true, true, true),
    ] {
        let options = FlexibleMatchingOptions {
            allow_other_performances: performance,
            allow_other_artists: artist,
            ..Default::default()
        };
        let a = assess_with_options(
            &track,
            &row,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            a.action == MatchAction::Accept,
            accepted,
            "{performance}/{artist}: {a:?}"
        );
    }
}

#[test]
fn live_cover_requires_both_artist_and_performance_permissions() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        provenance: json!({"workIds":["work-1"]}),
        ..Track::default()
    };
    let row = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: "Song (Live Cover)".into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Live Cover)","artistNames":["Other Artist"],"artistCount":1,"duration":200.0,"workId":"work-1"}),
        ),
        ..Candidate::default()
    };
    for (performance, artist, accepted) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let options = FlexibleMatchingOptions {
            allow_other_performances: performance,
            allow_other_artists: artist,
            ..Default::default()
        };
        let a = assess_with_options(
            &track,
            &row,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            a.action == MatchAction::Accept,
            accepted,
            "{performance}/{artist}: {a:?}"
        );
    }
}

#[test]
fn broad_version_options_can_accept_fresh_complete_unannotated_source() {
    let track = Track {
        artist: "Artist".into(),
        title: "Song".into(),
        version: Some("Live Remix".into()),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let row = Candidate {
        provider: "youtube".into(),
        id: "youtube:abcdefghijk".into(),
        title: "Song".into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"youtube","resolvedProviderId":"abcdefghijk","sourceMetadataOrigin":"ytDlpCandidateMetadata","track":"Song","artists":["Artist"],"artistCount":1,"duration":200.0}),
        ),
        ..Candidate::default()
    };
    let broad = FlexibleMatchingOptions {
        allow_other_releases: true,
        allow_other_performances: true,
        allow_other_mixes: true,
        ..Default::default()
    };
    let test = |c: &Candidate, o: &FlexibleMatchingOptions| {
        assess_with_options(&track, c, MatchLevel::Flexible, MatchStage::Resolved, o)
    };
    assert_eq!(
        test(&row, &FlexibleMatchingOptions::default()).action,
        MatchAction::Enrich
    );
    assert_eq!(test(&row, &broad).action, MatchAction::Accept);
    assert_eq!(test(&row, &broad).alternative_kind, None);
    let partial = FlexibleMatchingOptions {
        allow_other_mixes: false,
        ..broad
    };
    assert_ne!(test(&row, &partial).action, MatchAction::Accept);
    for (field, value) in [
        ("track", json!("Song (Unknown Session)")),
        ("artists", json!(["Wrong Artist"])),
        ("resolvedProviderId", json!("other-id")),
        ("isPreview", json!(true)),
    ] {
        let mut bad = row.clone();
        bad.source_metadata.as_mut().unwrap()[field] = value;
        assert_ne!(test(&bad, &broad).action, MatchAction::Accept, "{field}");
    }
}
