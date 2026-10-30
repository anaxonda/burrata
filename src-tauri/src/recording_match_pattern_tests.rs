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
