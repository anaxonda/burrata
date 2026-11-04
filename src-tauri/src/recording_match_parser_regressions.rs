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
