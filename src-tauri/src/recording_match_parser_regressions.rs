use crate::models::{Candidate, Track};
use crate::recording_match::{
    assess, assess_with_options, FlexibleMatchingOptions, MatchAction, MatchLevel, MatchStage,
};
use serde_json::json;

fn pair(request_title: &str, candidate_title: &str) -> (Track, Candidate) {
    let request = Track {
        provider: "spotify".into(),
        provider_id: "parser-regression".into(),
        artist: "Artist".into(),
        title: request_title.into(),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let candidate = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: candidate_title.into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"monochrome", "trackId":"123", "catalogTitle":candidate_title, "artistNames":["Artist"], "performerCreditsComplete":true, "duration":200.0}),
        ),
        ..Candidate::default()
    };
    (request, candidate)
}
fn assert_same_recording(request: &Track, candidate: &Candidate) {
    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        let search = assess(request, candidate, level, MatchStage::Search);
        assert!(
            matches!(search.action, MatchAction::Inspect | MatchAction::Accept),
            "{level:?} {request:?} {candidate:?} {search:?}"
        );
        let resolved = assess(request, candidate, level, MatchStage::Resolved);
        assert_eq!(
            resolved.action,
            MatchAction::Accept,
            "{level:?} {request:?} {candidate:?} {resolved:?}"
        );
    }
}
#[test]
fn square_brackets_share_balanced_trimmed_delimiters() {
    for label in [
        "Live",
        "Radio Edit",
        "DJ Named Remix",
        "Remastered 2009",
        "feat. Guest",
    ] {
        let plain = crate::recording_identity::parse_recording_text(&format!("Song ({label})"));
        let padded =
            crate::recording_identity::parse_recording_text(&format!("Song [  {label}  ]"));
        assert_eq!(plain.base_title, padded.base_title);
        assert_eq!(plain.annotations[0].kind, padded.annotations[0].kind);
        assert_eq!(plain.annotations[0].value, padded.annotations[0].value);
        let span = &padded.annotations[0].content_span;
        assert_eq!(&padded.original[span.start..span.end], label);
    }
    for malformed in ["Song [[Live]", "Song ] [Live]", "Song [Live", "[Live]"] {
        assert!(
            crate::matching::split_balanced_trailing_square_group(malformed).is_none(),
            "{malformed}"
        );
    }
    let (request, candidate) = pair("Song [ Radio Edit ]", "Song (Radio Edit)");
    assert_same_recording(&request, &candidate);
}
#[test]
fn mastering_and_recording_annotations_are_composable_and_order_independent() {
    for recording in ["Live at Wembley", "Acoustic", "DJ Named Remix"] {
        let reference = format!("Song ({recording}) (Remastered 2009)");
        let labels = [recording, "Remastered 2009", "Official Audio"];
        for i in 0..3 {
            for j in 0..3 {
                if i == j {
                    continue;
                }
                let k = 3 - i - j;
                for (open, close) in [("(", ")"), ("[ ", " ]")] {
                    let rendered = format!(
                        "Song {open}{}{close} {open}{}{close} {open}{}{close}",
                        labels[i], labels[j], labels[k]
                    );
                    let (request, candidate) = pair(&reference, &rendered);
                    assert_same_recording(&request, &candidate);
                }
            }
        }
        let (mut request, candidate) = pair(&format!("Song ({recording})"), &reference);
        request.version = Some("Remastered 2009".into());
        assert_same_recording(&request, &candidate);
    }
}
#[test]
fn neutral_labels_and_guest_suffix_order_do_not_conflict_with_mastering() {
    for neutral in ["Original Mix", "Album Version"] {
        let (request, candidate) = pair(
            &format!("Song ({neutral}) (Remastered 2009)"),
            "Song (Remastered 2009)",
        );
        assert_same_recording(&request, &candidate);
    }
    let (mut request, mut candidate) = pair(
        "Song (feat. Guest) (Remastered 2009)",
        "Song (Remastered 2009) (feat. Guest)",
    );
    request.artist = "Artist feat. Guest".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Artist", "Guest"]);
    assert_same_recording(&request, &candidate);
}
#[test]
fn structural_normalization_preserves_wrong_recording_and_unknown_title_negatives() {
    for (requested, observed) in [
        (
            "Song (Live at Wembley) (Remastered 2009)",
            "Song (Live at Madrid) (Remastered 2009)",
        ),
        (
            "Song (DJ Alpha Remix) (Remastered 2009)",
            "Song (DJ Beta Remix) (Remastered 2009)",
        ),
        (
            "Song (Live) (Remastered 2009)",
            "Other Song (Live) (Remastered 2009)",
        ),
        ("Song (At Last) (Live)", "Song (Other Words) (Live)"),
    ] {
        let (request, candidate) = pair(requested, observed);
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_ne!(
                assess(&request, &candidate, level, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{requested} / {observed}"
            );
        }
    }
    let (request, mut candidate) = pair(
        "Song (Live) (Remastered 2009)",
        "Song (Remastered 2009) (Live)",
    );
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Other Artist"]);
    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        assert_eq!(
            assess(&request, &candidate, level, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    for conflicting in [
        "Song (Live at Wembley) (Live at Madrid)",
        "Song (Remastered 2009) (Remastered 2019)",
    ] {
        let (request, candidate) = pair(conflicting, conflicting);
        let options = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: true,
            ..Default::default()
        };
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options
            )
            .action,
            MatchAction::Accept,
            "{conflicting}"
        );
    }
}

#[test]
fn structured_version_labels_match_title_labels_and_respect_mix_permission() {
    for label in [
        "Extended Version",
        "Dub",
        "DJ Named Dub",
        "A Cappella",
        "Mono",
        "Stereo",
        "Sped Up",
        "Slowed + Reverb",
    ] {
        let (mut request, candidate) = pair("Song", &format!("Song ({label})"));
        request.version = Some(label.into());
        assert_same_recording(&request, &candidate);
        let (request, candidate) = pair(&format!("Song - {label}"), &format!("Song ({label})"));
        assert_same_recording(&request, &candidate);
        let (request, candidate) = pair("Song", &format!("Song ({label})"));
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_ne!(
                assess(&request, &candidate, level, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{label}: {level:?}"
            );
        }
        let mixes = FlexibleMatchingOptions {
            allow_other_releases: false,
            allow_other_mixes: true,
            ..Default::default()
        };
        let accepted = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &mixes,
        );
        assert_eq!(
            accepted.action,
            MatchAction::Accept,
            "{label}: {accepted:?}"
        );
        assert_eq!(
            accepted.alternative_kind,
            Some(crate::recording_match::AlternativeKind::Remix)
        );
        let performance_only = FlexibleMatchingOptions {
            allow_other_releases: false,
            allow_other_performances: true,
            ..Default::default()
        };
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &performance_only
            )
            .action,
            MatchAction::Accept
        );
    }
    let (mut request, candidate) = pair("Song", "Song (A Cappella)");
    request.version = Some("Acapella".into());
    assert_same_recording(&request, &candidate);
    for unknown in ["Stereo Love", "A Cappella Dreams", "Mystery Label"] {
        let (request, candidate) = pair("Song", &format!("Song ({unknown})"));
        let all = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: true,
            ..Default::default()
        };
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &all
            )
            .action,
            MatchAction::Accept,
            "{unknown}"
        );
    }
}
#[test]
fn flexible_options_apply_only_to_changed_recording_dimensions() {
    let (request, candidate) = pair(
        "Song (Live at Wembley) (Remastered 2009)",
        "Song (Remastered 2019) (Live at Wembley)",
    );
    for level in [MatchLevel::Exact, MatchLevel::Balanced] {
        assert_eq!(
            assess(&request, &candidate, level, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    let release_only = FlexibleMatchingOptions::default();
    let release = assess_with_options(
        &request,
        &candidate,
        MatchLevel::Flexible,
        MatchStage::Resolved,
        &release_only,
    );
    assert_eq!(release.action, MatchAction::Accept, "{release:?}");
    assert_eq!(
        release.alternative_kind,
        Some(crate::recording_match::AlternativeKind::Release)
    );
    assert_eq!(
        crate::recording_match::alternative_rank(&release, &request, &candidate),
        1
    );
    let (_, different_venue) = pair("Song", "Song (Remastered 2019) (Live at Madrid)");
    assert_eq!(
        assess_with_options(
            &request,
            &different_venue,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &release_only
        )
        .action,
        MatchAction::Reject
    );
    let performance_only = FlexibleMatchingOptions {
        allow_other_releases: false,
        allow_other_performances: true,
        ..Default::default()
    };
    assert_eq!(
        assess_with_options(
            &request,
            &different_venue,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &performance_only
        )
        .action,
        MatchAction::Reject
    );
    let both = FlexibleMatchingOptions {
        allow_other_performances: true,
        ..Default::default()
    };
    assert_eq!(
        assess_with_options(
            &request,
            &different_venue,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &both
        )
        .action,
        MatchAction::Accept
    );
}

#[test]
fn search_budget_preserves_verified_title_and_artist_alternatives() {
    use crate::search_plan::{SearchPlan, SearchQueryReason};
    let (mut track, _) = pair("Canonical Title", "Canonical Title");
    let original = track.clone();
    let plan = SearchPlan::for_track_with_alternatives(
        &track,
        &["Verified Artist".into()],
        &["Verified Title".into()],
    );
    assert_eq!(plan.queries.len(), 2);
    assert_eq!(plan.queries[0].text, "Artist Canonical Title");
    assert_eq!(plan.queries[1].text, "Verified Artist Verified Title");
    assert_eq!(
        plan.queries[1].reason,
        SearchQueryReason::VerifiedTitleAlternative
    );
    assert_eq!(track.title, original.title);
    assert_eq!(track.artist, original.artist);
    track.provenance = json!({"manualFields":{"artist":true}});
    let manual_artist = SearchPlan::for_track_with_alternatives(
        &track,
        &["Verified Artist".into()],
        &["Verified Title".into()],
    );
    assert_eq!(manual_artist.queries[1].text, "Artist Verified Title");
    track.provenance = json!({"manualFields":{"title":true}});
    let manual_title = SearchPlan::for_track_with_alternatives(
        &track,
        &["Verified Artist".into()],
        &["Verified Title".into()],
    );
    assert!(manual_title
        .queries
        .iter()
        .all(|query| query.title == "Canonical Title"));
    let equivalent = SearchPlan::for_track_with_alternatives(
        &original,
        &["Verified Artist".into()],
        &["Canonical  Title".into()],
    );
    assert_eq!(
        equivalent.queries[1].reason,
        SearchQueryReason::VerifiedArtistAlternative
    );
}
#[test]
fn retrieval_uses_shared_version_keys_and_preserves_performance_context() {
    use crate::search_plan::SearchPlan;
    for (title, version) in [
        ("Song (Radio Edit) (Official Audio)", "Radio Edit"),
        ("Song [ Radio Edit ]", "Radio Edit (3:20)"),
        ("Song (2009 Digital Remaster)", "Remastered 2009"),
        ("Song (A Cappella)", "Acapella"),
        ("Song (Radio Edit) (Official Audio)", "Song Radio Edit"),
    ] {
        let (mut track, _) = pair(title, title);
        track.version = Some(version.into());
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries[0].title, title, "{title} / {version}");
    }
    let (track, _) = pair("Song (Live at Remaster Hall)", "Song");
    let unchanged = SearchPlan::for_track_with_release_policy(&track, &[], true);
    assert!(unchanged
        .queries
        .iter()
        .all(|query| query.title.contains("Live at Remaster Hall")));
    let (track, _) = pair("Song (Live at Wembley) (Remastered 2009)", "Song");
    let release = SearchPlan::for_track_with_release_policy(&track, &[], true);
    assert_eq!(release.queries.len(), 2);
    assert_eq!(release.queries[1].title, "Song (Live at Wembley)");
    assert_eq!(track.title, "Song (Live at Wembley) (Remastered 2009)");
}

#[test]
fn original_labels_still_conflict_with_explicit_performance_or_mix() {
    for version in ["Live", "DJ Named Remix", "Dub"] {
        let (mut request, mut candidate) =
            pair(&format!("Song ({version})"), &format!("Song ({version})"));
        request.version = Some("Original Mix".into());
        candidate.source_metadata.as_mut().unwrap()["version"] = json!("Original Mix");
        let all = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: true,
            ..Default::default()
        };
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let result =
                assess_with_options(&request, &candidate, level, MatchStage::Resolved, &all);
            assert_ne!(result.action, MatchAction::Accept, "{version}: {result:?}");
            assert!(result
                .reasons
                .contains(&crate::recording_match::MatchReason::ConflictingRequestVersions));
        }
    }
}
