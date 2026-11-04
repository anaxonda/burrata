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

fn captured_halka_history_request() -> Track {
    // Exact identity fields captured from the connected Spotify artist page;
    // this source did not expose complete performer roles or a role list.
    Track {
        id: "spotify:2u9k9X8d1uD4qDLjMhVxU0".into(),
        provider: "spotify".into(),
        provider_id: "2u9k9X8d1uD4qDLjMhVxU0".into(),
        source_url: "https://open.spotify.com/track/2u9k9X8d1uD4qDLjMhVxU0".into(),
        title: "Halka Halka (From \"Fanney Khan\")".into(),
        artist: "Sunidhi Chauhan, Divya Kumar, Amit Trivedi, Nusrat Fateh Ali Khan, Irshad Kamil"
            .into(),
        album: Some("Feel Good Valentines".into()),
        duration_seconds: Some(247.0),
        provenance: json!({
            "artistId":"5HcunTidTUrOaf8V0iJcvl",
            "available":true,
            "position":2,
            "positionScope":"artist",
            "recordingContext":{"albumId":"3giSO1A5PjcJ77tSclYv5r","albumUri":"spotify:album:3giSO1A5PjcJ77tSclYv5r"},
            "remoteEntryId":"artist-5HcunTidTUrOaf8V0iJcvl-1",
            "remotePlaylist":false
        }),
        ..Track::default()
    }
}

fn synthetic_halka_candidate(title: &str, artists: &[&str]) -> Candidate {
    Candidate {
        id: "monochrome:synthetic-halka-history-recovery".into(),
        provider: "monochrome".into(),
        title: title.into(),
        duration_seconds: Some(247.0),
        source_metadata: Some(json!({
            "provider":"monochrome",
            "trackId":"synthetic-halka-history-recovery",
            "catalogTitle":title,
            "artists":artists,
            "artistCount":artists.len(),
            "duration":247.0
        })),
        ..Candidate::default()
    }
}

fn captured_public_candidate(document: &serde_json::Value, provider: &str, id: &str) -> Candidate {
    let row = document["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["provider"] == provider
                && row["id"]
                    .as_str()
                    .is_some_and(|value| value.rsplit(':').next() == Some(id))
        })
        .unwrap();
    Candidate {
        id: format!("{provider}:{id}"),
        provider: provider.into(),
        title: row["title"].as_str().unwrap().into(),
        uploader: row["uploader"].as_str().map(str::to_owned),
        duration_seconds: row["durationSeconds"].as_f64(),
        source_metadata: row.get("sourceMetadata").cloned(),
        ..Candidate::default()
    }
}

const HALKA_CAPTURED_CREDITS: [&str; 5] = [
    "Sunidhi Chauhan",
    "Divya Kumar",
    "Amit Trivedi",
    "Nusrat Fateh Ali Khan",
    "Irshad Kamil",
];

#[test]
fn captured_halka_history_track_gets_soundtrack_query_fallback_without_roles() {
    let requested = captured_halka_history_request();
    assert!(requested.provenance.get("artistCredits").is_none());
    let plan = crate::search_plan::SearchPlan::for_track(&requested, &[]);
    assert_eq!(plan.queries.len(), 2);
    assert!(plan.queries[0]
        .text
        .contains("Halka Halka (From \"Fanney Khan\")"));
    assert_eq!(
        plan.queries[1].reason,
        crate::search_plan::SearchQueryReason::SoundtrackContextFallback
    );
    assert!(!plan.queries[1].text.contains("Fanney Khan"));
    assert!(plan.queries[1].text.contains("Halka Halka"));
    assert_eq!(requested.title, "Halka Halka (From \"Fanney Khan\")");
}

#[test]
fn captured_halka_history_positive_and_paired_negatives_keep_unknown_completeness() {
    let requested = captured_halka_history_request();
    let correct = synthetic_halka_candidate(&requested.title, &HALKA_CAPTURED_CREDITS);
    let mut wrong_credit = HALKA_CAPTURED_CREDITS;
    wrong_credit[4] = "Wrong Credit";
    let negatives = [
        synthetic_halka_candidate(&requested.title, &wrong_credit),
        synthetic_halka_candidate("Halka Halka (Live)", &HALKA_CAPTURED_CREDITS),
    ];

    for stage in [MatchStage::Search, MatchStage::Resolved] {
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let positive = assess(&requested, &correct, level, stage);
            assert_eq!(
                positive.dimensions.title,
                Comparison::Agree,
                "{level:?} {stage:?}: {positive:?}"
            );
            assert_eq!(
                positive.dimensions.artists,
                Comparison::Agree,
                "{level:?} {stage:?}: {positive:?}"
            );
            assert!(
                !positive
                    .evidence
                    .as_ref()
                    .unwrap()
                    .complete_performer_credits,
                "{level:?} {stage:?}: {positive:?}"
            );
            assert_ne!(
                positive.action,
                MatchAction::Reject,
                "{level:?} {stage:?}: {positive:?}"
            );

            for negative_candidate in &negatives {
                let negative = assess(&requested, negative_candidate, level, stage);
                assert_ne!(
                    negative.action,
                    MatchAction::Accept,
                    "{level:?} {stage:?}: {negative:?}"
                );
                assert!(
                    !negative
                        .evidence
                        .as_ref()
                        .unwrap()
                        .complete_performer_credits
                );
            }
        }
    }
}

#[test]
fn captured_public_halka_candidates_remain_unverified_and_remix_is_rejected() {
    let requested = captured_halka_history_request();
    let youtube: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/performance-results/2026-11-02-remediation/halka-live-youtube.json"
    ))
    .unwrap();
    let soundcloud: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/performance-results/2026-11-02-remediation/halka-live-soundcloud.json"
    ))
    .unwrap();
    assert_eq!(
        youtube["qualification"],
        "Retrieved metadata only; identity/delivered media/native output not verified"
    );
    assert_eq!(
        soundcloud["qualification"],
        "Retrieved metadata only; identity/delivered media/native output not verified"
    );

    let exact_youtube = captured_public_candidate(&youtube, "youtube", "z-wNY9Uu4E4");
    let remix_youtube = captured_public_candidate(&youtube, "youtube", "3kZNuv5Mhcg");
    let exact_soundcloud = captured_public_candidate(&soundcloud, "soundcloud", "474216888");
    for candidate in [&exact_youtube, &exact_soundcloud] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let assessment = assess(&requested, candidate, level, stage);
                assert!(
                    !assessment
                        .evidence
                        .as_ref()
                        .unwrap()
                        .complete_performer_credits,
                    "{level:?} {stage:?}: {assessment:?}"
                );
                assert_ne!(
                    assessment.action,
                    MatchAction::Accept,
                    "{level:?} {stage:?}: {assessment:?}"
                );
            }
        }
    }
    for stage in [MatchStage::Search, MatchStage::Resolved] {
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let assessment = assess(&requested, &remix_youtube, level, stage);
            assert_ne!(
                assessment.action,
                MatchAction::Accept,
                "{level:?} {stage:?}: {assessment:?}"
            );
        }
    }
}

#[test]
fn monochrome_id_and_artist_count_do_not_close_performer_credits() {
    let requested = Track {
        id: "catalog:requested".into(),
        provider: "catalog".into(),
        provider_id: "requested".into(),
        title: "Halka Halka".into(),
        artist: "Sunidhi Chauhan, Vishal-Shekhar".into(),
        ..Track::default()
    };
    let candidate = Candidate {
        id: "monochrome:synthetic-fixture".into(),
        provider: "monochrome".into(),
        title: "Halka Halka".into(),
        source_metadata: Some(json!({
            "provider":"monochrome",
            "trackId":"synthetic-fixture",
            "catalogTitle":"Halka Halka",
            "artistCount":2
        })),
        ..Candidate::default()
    };

    for stage in [MatchStage::Search, MatchStage::Resolved] {
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let assessment = assess(&requested, &candidate, level, stage);
            assert!(
                !assessment
                    .evidence
                    .as_ref()
                    .unwrap()
                    .complete_performer_credits,
                "{level:?} {stage:?}: {assessment:?}"
            );
        }
    }
}

#[test]
fn nonempty_mix_is_seen_when_the_separate_version_field_is_empty() {
    let requested = Track {
        title: "Song (Live)".into(),
        artist: "Artist".into(),
        duration_seconds: Some(210.0),
        ..Track::default()
    };
    let candidate = Candidate {
        id: "youtube:live-song".into(),
        provider: "youtube".into(),
        title: "Song".into(),
        duration_seconds: Some(210.0),
        source_metadata: Some(json!({
            "provider":"youtube", "track":"Song", "artists":["Artist"],
            "version":"", "mix":"Live", "duration":210.0
        })),
        ..Candidate::default()
    };
    for stage in [MatchStage::Search, MatchStage::Resolved] {
        let assessment = assess(&requested, &candidate, MatchLevel::Balanced, stage);
        assert_eq!(
            assessment.dimensions.version,
            Comparison::Agree,
            "{assessment:?}"
        );
    }
}

#[test]
fn conflicting_candidate_version_fields_remain_a_hard_identity_conflict() {
    let requested = Track {
        title: "Song (Live)".into(),
        artist: "Artist".into(),
        duration_seconds: Some(210.0),
        ..Track::default()
    };
    let candidate = Candidate {
        id: "youtube:conflicting-version".into(),
        provider: "youtube".into(),
        title: "Song".into(),
        duration_seconds: Some(210.0),
        source_metadata: Some(json!({
            "provider":"youtube", "track":"Song", "artists":["Artist"],
            "version":"Studio", "mix":"Live", "duration":210.0
        })),
        ..Candidate::default()
    };
    for stage in [MatchStage::Search, MatchStage::Resolved] {
        let assessment = assess(&requested, &candidate, MatchLevel::Flexible, stage);
        assert_eq!(
            assessment.dimensions.version,
            Comparison::Conflict,
            "{assessment:?}"
        );
        assert_ne!(assessment.action, MatchAction::Accept, "{assessment:?}");
    }
}

#[test]
fn featuring_spellings_have_positive_and_paired_negative_controls_across_policy_matrix() {
    let requested = Track {
        title: "Song (feat. Guest)".into(),
        artist: "Lead A & Lead B".into(),
        ..Track::default()
    };
    let positives = [
        "Lead A x Lead B - Song (ft. Guest) (Official Audio)",
        "Lead A & Lead B — Song (w/ Guest) [Audio Oficial]",
        "Lead A, Lead B - Song (featuring Guest)",
    ];
    let negatives = [
        "Lead A x Lead B - Song (ft. Other Guest) (Official Audio)",
        "Lead A x Lead B - Song (Live) (ft. Guest) (Official Audio)",
        "Lead A x Different Artist - Song (ft. Guest) (Official Audio)",
        "Lead A x Lead B - Song (Unclassified Session) (ft. Guest)",
    ];
    for stage in [MatchStage::Search, MatchStage::Resolved] {
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            for caption in positives {
                let assessment = assess(&requested, &candidate(caption), level, stage);
                assert_eq!(
                    assessment.dimensions.title,
                    Comparison::Agree,
                    "{level:?} {stage:?}: {caption}: {assessment:?}"
                );
                assert_eq!(
                    assessment.dimensions.guests,
                    Comparison::Agree,
                    "{level:?} {stage:?}: {caption}: {assessment:?}"
                );
                assert_ne!(
                    assessment.action,
                    MatchAction::Reject,
                    "{level:?} {stage:?}: {caption}: {assessment:?}"
                );
            }
            for caption in negatives {
                let assessment = assess(&requested, &candidate(caption), level, stage);
                assert_ne!(
                    assessment.action,
                    MatchAction::Accept,
                    "{level:?} {stage:?}: {caption}: {assessment:?}"
                );
            }
        }
    }
}

#[test]
fn multiple_structured_guest_roles_are_compared_as_guests_not_primary_artists() {
    let requested = Track {
        title: "Song".into(),
        artist: "Lead".into(),
        provenance: json!({"artistCredits":[
            {"name":"Lead","role":"Main_Artist","joinPhrase":" & "},
            {"name":"Guest One","role":"Featured-Artist","joinPhrase":" & "},
            {"name":"Guest Two","role":"guest","joinPhrase":""},
            {"name":"Composer","role":"composer"}
        ]}),
        ..Track::default()
    };
    let candidate = Candidate {
        id: "spotify:structured-guests".into(),
        provider: "spotify".into(),
        title: "Song".into(),
        source_metadata: Some(json!({
            "provider":"spotify", "track":"Song", "artists":[
                {"name":"Lead","role":"Artist"},
                {"name":"Guest One","role":"Featured Artist"},
                {"name":"Guest Two","role":"Guest"},
                {"name":"Composer","role":"Composer"}
            ]
        })),
        ..Candidate::default()
    };
    for stage in [MatchStage::Search, MatchStage::Resolved] {
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let assessment = assess(&requested, &candidate, level, stage);
            assert_eq!(
                assessment.dimensions.artists,
                Comparison::Agree,
                "{level:?} {stage:?}: {assessment:?}"
            );
            assert_eq!(
                assessment.dimensions.guests,
                Comparison::Agree,
                "{level:?} {stage:?}: {assessment:?}"
            );
            assert_ne!(
                assessment.action,
                MatchAction::Reject,
                "{level:?} {stage:?}: {assessment:?}"
            );
        }
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

fn captured_django_parisian_tracks() -> Vec<Track> {
    let capture: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/performance-results/2026-11-02-remediation/django-parisian-source-capture.json"
    ))
    .unwrap();
    capture["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .unwrap()
}

fn same_musicbrainz_recording_candidate(track: &Track) -> Candidate {
    // Synthetic evaluator fixture only: it reuses the captured request's real
    // recording ID and ordered names to model independently verified MB
    // evidence. The read-only YouTube captures below remain negative metadata
    // diagnostics and are not converted into verified candidates.
    let recording_id = track.provenance["recordingId"].as_str().unwrap();
    let title = crate::recording_identity::parse_title(&track.title).title;
    let artists = track.provenance["artistCredits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|credit| credit["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    Candidate {
        id: format!("musicbrainz:{recording_id}"),
        provider: "musicbrainz".into(),
        title: title.clone(),
        duration_seconds: track.duration_seconds,
        source_metadata: Some(json!({
            "provider":"musicbrainz",
            "verified":true,
            "recordingId":recording_id,
            "track":title,
            "artists":artists,
            "duration":track.duration_seconds
        })),
        ..Candidate::default()
    }
}

fn captured_django_youtube_candidate(id: &str) -> Candidate {
    let capture: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/performance-results/2026-11-02-remediation/django-live-youtube.json"
    ))
    .unwrap();
    let row = capture["captures"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|capture| capture["candidates"].as_array().unwrap())
        .find(|row| row["id"] == id)
        .unwrap();
    Candidate {
        id: format!("youtube:{id}"),
        provider: "youtube".into(),
        title: row["title"].as_str().unwrap().into(),
        uploader: row["uploader"].as_str().map(str::to_owned),
        duration_seconds: row["durationSeconds"].as_f64(),
        source_metadata: row.get("sourceMetadata").cloned(),
        ..Candidate::default()
    }
}

#[test]
fn captured_parisian_series_flexible_allows_only_bound_restoration_substitutes() {
    let requests = captured_django_parisian_tracks();
    assert_eq!(requests.len(), 12);
    let default_flexible_options = FlexibleMatchingOptions::default();
    for request in &requests {
        assert!(request.title.contains("(Restored Edition '25)"));
        let candidate = same_musicbrainz_recording_candidate(request);
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [MatchLevel::Exact, MatchLevel::Balanced] {
                let result = assess(request, &candidate, level, stage);
                assert_ne!(
                    result.action,
                    MatchAction::Accept,
                    "{level:?} {stage:?}: {result:?}"
                );
            }
            let flexible = assess_with_options(
                request,
                &candidate,
                MatchLevel::Flexible,
                stage,
                &default_flexible_options,
            );
            if stage == MatchStage::Resolved {
                assert_eq!(flexible.action, MatchAction::Accept, "{flexible:?}");
                assert_eq!(flexible.alternative_kind, Some(AlternativeKind::Release));
            } else {
                assert_ne!(flexible.action, MatchAction::Accept, "{flexible:?}");
            }
            assert_eq!(flexible.dimensions.title, Comparison::Agree, "{flexible:?}");
            assert_eq!(
                flexible.dimensions.version,
                Comparison::Unknown,
                "{flexible:?}"
            );
        }
    }

    let request = &requests[0];
    let mut wrong_artist = same_musicbrainz_recording_candidate(request);
    wrong_artist.source_metadata.as_mut().unwrap()["artists"] = json!(["Wrong Artist"]);
    let mut live = same_musicbrainz_recording_candidate(request);
    live.title = "Swing 42 (Live)".into();
    live.source_metadata.as_mut().unwrap()["track"] = json!("Swing 42 (Live)");
    let mut mix = same_musicbrainz_recording_candidate(request);
    mix.title = "Swing 42 (Remix)".into();
    mix.source_metadata.as_mut().unwrap()["track"] = json!("Swing 42 (Remix)");
    let mut missing_runtime_and_isrc = same_musicbrainz_recording_candidate(request);
    missing_runtime_and_isrc.duration_seconds = None;
    let metadata = missing_runtime_and_isrc.source_metadata.as_mut().unwrap();
    metadata.as_object_mut().unwrap().remove("duration");
    metadata.as_object_mut().unwrap().remove("isrc");
    metadata["recordingId"] = json!("ec12bebd-e1b9-42ff-892b-4aa4825ac8eb");
    for candidate in [&wrong_artist, &live, &mix, &missing_runtime_and_isrc] {
        let result = assess_with_options(
            request,
            candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &default_flexible_options,
        );
        assert_ne!(result.action, MatchAction::Accept, "{result:?}");
    }
}

#[test]
fn all_version_categories_allow_only_a_fresh_unversioned_single_recording_alternative() {
    let requests = captured_django_parisian_tracks();
    let request = &requests[0];
    assert_eq!(request.duration_seconds, Some(165.0));
    let video_id = "lK_BGCe3q6I";
    let candidate = Candidate {
        id: format!("youtube:{video_id}"),
        provider: "youtube".into(),
        title: "Swing 42".into(),
        duration_seconds: Some(168.0),
        source_metadata: Some(json!({
            "provider":"youtube",
            "resolvedProviderId":video_id,
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "track":"Swing 42",
            "artists":["Django Reinhardt","Quintette du Hot Club de France"],
            "duration":168.0
        })),
        ..Candidate::default()
    };
    let options = FlexibleMatchingOptions {
        allow_other_releases: true,
        allow_other_performances: true,
        allow_other_mixes: true,
        allow_different_lengths: true,
        max_shorter_by_seconds: 30,
        max_longer_by_seconds: 30,
        ..Default::default()
    };

    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let result = assess_with_options(request, &candidate, level, stage, &options);
            if level == MatchLevel::Flexible && stage == MatchStage::Resolved {
                assert_eq!(result.action, MatchAction::Accept, "{result:?}");
                assert_eq!(result.relationship, MatchRelationship::Alternative);
                assert_eq!(result.alternative_kind, None);
                assert_eq!(result.dimensions.version, Comparison::Unknown);
                let evidence = result.evidence.as_ref().unwrap();
                assert!(!evidence.complete_performer_credits);
                assert!(result
                    .reasons
                    .contains(&MatchReason::UnspecifiedVersionAllowed));
            } else {
                assert_ne!(
                    result.action,
                    MatchAction::Accept,
                    "{level:?} {stage:?}: {result:?}"
                );
            }
        }
    }

    let mut wrong_artist = candidate.clone();
    wrong_artist.source_metadata.as_mut().unwrap()["artists"] = json!(["Wrong Artist"]);
    let mut partial_credits = candidate.clone();
    partial_credits.source_metadata.as_mut().unwrap()["artists"] = json!(["Django Reinhardt"]);
    let mut extra_artist = candidate.clone();
    extra_artist.source_metadata.as_mut().unwrap()["artists"] = json!([
        "Django Reinhardt",
        "Quintette du Hot Club de France",
        "Unrequested Guest"
    ]);
    let mut live = candidate.clone();
    live.title = "Swing 42 (Live)".into();
    live.source_metadata.as_mut().unwrap()["track"] = json!("Swing 42 (Live)");
    let mut remix = candidate.clone();
    remix.title = "Swing 42 (Remix)".into();
    remix.source_metadata.as_mut().unwrap()["track"] = json!("Swing 42 (Remix)");
    let mut missing_runtime = candidate.clone();
    missing_runtime.duration_seconds = None;
    missing_runtime
        .source_metadata
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("duration");
    let mut isrc_conflict = candidate.clone();
    isrc_conflict.source_metadata.as_mut().unwrap()["isrc"] = json!("USAAA2600001");
    let mut isrc_conflict_request = request.clone();
    isrc_conflict_request.isrc = Some("USBBB2600002".into());
    let mut request_identifier_conflict = request.clone();
    request_identifier_conflict.isrc = Some("USBBB2600002".into());
    request_identifier_conflict.provenance["remoteCatalog"] = json!({"isrc":"USCCC2600003"});
    let mut wrong_id = candidate.clone();
    wrong_id.source_metadata.as_mut().unwrap()["resolvedProviderId"] = json!("different-video");
    let mut wrong_origin = candidate.clone();
    wrong_origin.source_metadata.as_mut().unwrap()["sourceMetadataOrigin"] =
        json!("youtubeMusicSearch");
    let mut full_album = candidate.clone();
    full_album.source_metadata.as_mut().unwrap()["isFullAlbum"] = json!(true);
    let mut preview = candidate.clone();
    preview.source_metadata.as_mut().unwrap()["isPreview"] = json!(true);
    let mut chapters = candidate.clone();
    chapters.chapters = Some(vec![crate::models::Chapter {
        id: "chapter-1".into(),
        title: "Part 1".into(),
        start_seconds: 0.0,
        end_seconds: 30.0,
        confidence: "high".into(),
        suggested_track: None,
        suggestion_confidence: None,
    }]);
    let mut over_cap_request = request.clone();
    over_cap_request.duration_seconds = Some(200.0);
    let mut over_cap_candidate = candidate.clone();
    over_cap_candidate.duration_seconds = Some(203.0);
    over_cap_candidate.source_metadata.as_mut().unwrap()["duration"] = json!(203.0);
    let over_cap_options = FlexibleMatchingOptions {
        max_longer_by_seconds: 2,
        ..options
    };

    let negative_cases = [
        (request, &wrong_artist, &options, "wrong artist"),
        (request, &partial_credits, &options, "partial credits"),
        (request, &extra_artist, &options, "extra performer"),
        (
            request,
            &candidate,
            &FlexibleMatchingOptions {
                allow_other_releases: false,
                ..options
            },
            "forbidden release permission",
        ),
        (
            request,
            &live,
            &FlexibleMatchingOptions {
                allow_other_performances: false,
                ..options
            },
            "forbidden live permission",
        ),
        (
            request,
            &remix,
            &FlexibleMatchingOptions {
                allow_other_mixes: false,
                ..options
            },
            "forbidden remix permission",
        ),
        (request, &missing_runtime, &options, "missing runtime"),
        (
            &isrc_conflict_request,
            &isrc_conflict,
            &options,
            "ISRC conflict",
        ),
        (
            &request_identifier_conflict,
            &candidate,
            &options,
            "conflicting request identifiers",
        ),
        (request, &wrong_id, &options, "wrong resolved ID"),
        (request, &wrong_origin, &options, "untrusted source origin"),
        (request, &full_album, &options, "full album"),
        (request, &preview, &options, "preview source"),
        (request, &chapters, &options, "chaptered source"),
        (
            &over_cap_request,
            &over_cap_candidate,
            &over_cap_options,
            "duration cap exceeded",
        ),
    ];
    for (level, stage) in [
        (MatchLevel::Exact, MatchStage::Search),
        (MatchLevel::Exact, MatchStage::Resolved),
        (MatchLevel::Balanced, MatchStage::Search),
        (MatchLevel::Balanced, MatchStage::Resolved),
        (MatchLevel::Flexible, MatchStage::Search),
        (MatchLevel::Flexible, MatchStage::Resolved),
    ] {
        for (request, candidate, options, reason) in negative_cases {
            let result = assess_with_options(request, candidate, level, stage, options);
            assert_ne!(
                result.action,
                MatchAction::Accept,
                "{reason} {level:?} {stage:?}: {result:?}"
            );
        }
    }
}

#[test]
fn malformed_request_credit_sources_block_unversioned_flexible_acceptance() {
    let request = Track {
        id: "catalog:requested-song".into(),
        provider: "catalog".into(),
        provider_id: "requested-song".into(),
        title: "Song (Restored Edition '25)".into(),
        artist: "Artist".into(),
        version: Some("Restored Edition '25".into()),
        duration_seconds: Some(120.0),
        provenance: json!({"catalog":{"artists":{"unexpected":"object"}}}),
        ..Track::default()
    };
    let candidate = Candidate {
        id: "youtube:bound-song".into(),
        provider: "youtube".into(),
        title: "Song".into(),
        duration_seconds: Some(120.0),
        source_metadata: Some(json!({
            "provider":"youtube",
            "resolvedProviderId":"bound-song",
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "track":"Song",
            "artists":["Artist"],
            "duration":120.0
        })),
        ..Candidate::default()
    };
    let options = FlexibleMatchingOptions {
        allow_other_releases: true,
        allow_other_performances: true,
        allow_other_mixes: true,
        ..Default::default()
    };
    assert!(crate::recording_identity::RecordingIdentity::from_track(&request).credits_uncertain);
    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let assessment = assess_with_options(&request, &candidate, level, stage, &options);
            assert_ne!(
                assessment.action,
                MatchAction::Accept,
                "{level:?} {stage:?}: {assessment:?}"
            );
            assert!(assessment.evidence.unwrap().parsing_incomplete);
        }
    }

    let empty_or_null = Track {
        provenance: json!({
            "catalog":{"artists":null},
            "remoteCatalog":{"artistRefs":[]},
            "artistCredits":null
        }),
        ..request.clone()
    };
    assert!(
        !crate::recording_identity::RecordingIdentity::from_track(&empty_or_null).credits_uncertain
    );
    assert_eq!(
        assess_with_options(
            &empty_or_null,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options
        )
        .action,
        MatchAction::Accept
    );

    let manual_artist = Track {
        provenance: json!({
            "catalog":{"artists":{"unexpected":"object"}},
            "manualFields":{"artist":true}
        }),
        ..request.clone()
    };
    assert_eq!(
        assess_with_options(
            &manual_artist,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options
        )
        .action,
        MatchAction::Accept
    );

    let manual_artist_credits = Track {
        provenance: json!({
            "catalog":{"artists":{"unexpected":"object"}},
            "manualFields":{"artist":true,"artistCredits":true}
        }),
        ..request
    };
    assert_ne!(
        assess_with_options(
            &manual_artist_credits,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options
        )
        .action,
        MatchAction::Accept
    );
}

#[test]
fn captured_public_django_youtube_rows_do_not_gain_release_completeness() {
    let requests = captured_django_parisian_tracks();
    for (request_index, candidate_id) in
        [(0, "AVgvKGkxDLc"), (1, "kt6ULcZxwRo"), (9, "Rkrvuszati8")]
    {
        let request = &requests[request_index];
        let candidate = captured_django_youtube_candidate(candidate_id);
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let result = assess(request, &candidate, level, stage);
                assert_ne!(
                    result.action,
                    MatchAction::Accept,
                    "{candidate_id} {level:?} {stage:?}: {result:?}"
                );
                assert!(
                    !result.evidence.unwrap().complete_performer_credits,
                    "{candidate_id} {level:?} {stage:?}"
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
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Live)","artistNames":["Artist"],"artistCount":1,"performerCreditsComplete":true,"duration":400.0}),
        ),
        ..Candidate::default()
    };
    let mut options = FlexibleMatchingOptions {
        allow_other_performances: true,
        max_longer_by_seconds: 200,
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
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song","artistNames":["Other Artist"],"artistCount":1,"performerCreditsComplete":true,"duration":200.0,"workId":"work-1"}),
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
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Cover)","artistNames":["Other Artist"],"artistCount":1,"performerCreditsComplete":true,"duration":200.0,"workId":"work-1"}),
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
            json!({"provider":"monochrome","trackId":"123","catalogTitle":"Song (Live Cover)","artistNames":["Other Artist"],"artistCount":1,"performerCreditsComplete":true,"duration":200.0,"workId":"work-1"}),
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
            json!({"provider":"youtube","resolvedProviderId":"abcdefghijk","sourceMetadataOrigin":"ytDlpCandidateMetadata","track":"Song","artists":["Artist"],"artistCount":1,"performerCreditsComplete":true,"duration":200.0}),
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

#[test]
fn captured_i_will_remaster_labels_preserve_recording_and_policy_dimensions() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/i-will-remastered-2009-candidate.json"
    ))
    .unwrap();
    let request: Track = serde_json::from_value(fixture["request"].clone()).unwrap();
    let candidate: Candidate = serde_json::from_value(fixture["candidate"].clone()).unwrap();
    let parenthesized = Track {
        title: "I Will (Remastered 2009)".into(),
        ..request.clone()
    };
    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let actual = assess(&request, &candidate, level, stage);
            let control = assess(&parenthesized, &candidate, level, stage);
            assert_eq!(actual.action, control.action, "{level:?} {stage:?}");
            assert_eq!(actual.dimensions, control.dimensions, "{level:?} {stage:?}");
            assert_eq!(actual.dimensions.title, Comparison::Agree);
            assert_eq!(actual.dimensions.version, Comparison::Agree);
            // A singular artist is structured performer evidence, without
            // establishing exhaustive credits. Source binding is verified by
            // playback_match::verify against fresh PreviewAudio separately.
            assert_eq!(actual.dimensions.artists, Comparison::Agree);
            assert_eq!(
                actual.action,
                if stage == MatchStage::Search {
                    MatchAction::Inspect
                } else {
                    MatchAction::Accept
                },
                "{level:?} {stage:?}: {actual:?}"
            );
            assert!(!actual.evidence.as_ref().unwrap().complete_performer_credits);
        }
    }

    for label in [
        "Remaster 2009",
        "2009 Digital Remaster",
        "Digitally Remastered 2009",
    ] {
        let mut equivalent = candidate.clone();
        set_captured_i_will_title(&mut equivalent, &format!("I Will ({label})"));
        let actual = assess(
            &request,
            &equivalent,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(actual.action, MatchAction::Accept, "{label}: {actual:?}");
        assert_eq!(actual.dimensions.version, Comparison::Agree);
    }

    let mut wrong_artist = candidate.clone();
    wrong_artist.source_metadata.as_mut().unwrap()["artist"] = json!("The Who");
    for title in [
        "I Will (Live)",
        "I Will (Remix)",
        "Another Song (Remastered 2009)",
    ] {
        let mut wrong_recording = candidate.clone();
        set_captured_i_will_title(&mut wrong_recording, title);
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            for stage in [MatchStage::Search, MatchStage::Resolved] {
                assert_ne!(
                    assess(&request, &wrong_recording, level, stage).action,
                    MatchAction::Accept,
                    "{title} {level:?} {stage:?}"
                );
                assert_ne!(
                    assess(&request, &wrong_artist, level, stage).action,
                    MatchAction::Accept,
                    "wrong artist {level:?} {stage:?}"
                );
            }
        }
    }

    let mut other_year = candidate.clone();
    set_captured_i_will_title(&mut other_year, "I Will (Remastered 2010)");
    for level in [MatchLevel::Exact, MatchLevel::Balanced] {
        for stage in [MatchStage::Search, MatchStage::Resolved] {
            let actual = assess(&request, &other_year, level, stage);
            assert_eq!(actual.dimensions.version, Comparison::Conflict);
            assert_ne!(actual.action, MatchAction::Accept);
        }
    }
    let flexible = assess(
        &request,
        &other_year,
        MatchLevel::Flexible,
        MatchStage::Resolved,
    );
    assert_eq!(flexible.dimensions.version, Comparison::Conflict);
    assert_ne!(flexible.action, MatchAction::Accept);
    // Selecting another release needs stronger evidence than the captured
    // scalar artist field. Keep that requirement distinct from title parsing.
    let mut verified_other_year = other_year;
    let metadata = verified_other_year.source_metadata.as_mut().unwrap();
    metadata["provider"] = json!("youtube");
    metadata["resolvedProviderId"] = json!("p-abNGP1BK4");
    metadata["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
    metadata["artists"] = json!(["The Beatles"]);
    metadata["performerCreditsComplete"] = json!(true);
    let flexible = assess(
        &request,
        &verified_other_year,
        MatchLevel::Flexible,
        MatchStage::Resolved,
    );
    assert_eq!(flexible.action, MatchAction::Accept);
    assert_eq!(flexible.relationship, MatchRelationship::Alternative);
    assert_eq!(flexible.alternative_kind, Some(AlternativeKind::Release));
    let strict_release = FlexibleMatchingOptions {
        allow_other_releases: false,
        ..Default::default()
    };
    assert_ne!(
        assess_with_options(
            &request,
            &verified_other_year,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &strict_release
        )
        .action,
        MatchAction::Accept
    );

    let mut manual_live = request.clone();
    manual_live.version = Some("Live".into());
    manual_live.provenance["manualFields"] = json!({"version":true});
    assert_ne!(
        assess(
            &manual_live,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Accept
    );
}

fn set_captured_i_will_title(candidate: &mut Candidate, title: &str) {
    candidate.title = title.to_owned();
    if let Some(metadata) = candidate.source_metadata.as_mut() {
        metadata["title"] = json!(title);
        metadata["song"] = json!(title);
        for field in ["track", "catalogTitle"] {
            if metadata.get(field).is_some() {
                metadata[field] = json!(title);
            }
        }
        let description = metadata["description"]
            .as_str()
            .map(|description| description.replace("I Will (Remastered 2009)", title));
        if let Some(description) = description {
            metadata["description"] = json!(description);
        }
    }
}
