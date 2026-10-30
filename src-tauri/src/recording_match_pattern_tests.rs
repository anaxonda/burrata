use super::*;
use serde_json::json;

fn request() -> Track {
    Track { title: "Vibe With Me".into(), artist: "Nipsey Hussle,Bino Rideaux,Leon Thomas".into(), ..Track::default() }
}
fn candidate(title: &str) -> Candidate {
    Candidate { id: "youtube:ExdT24gJngk".into(), provider: "youtube".into(), title: title.into(), uploader: Some("Nipsey Hussle".into()), duration_seconds: Some(232.), source_metadata: Some(json!({"catalogTitle":title,"track":title,"duration":232,"extractor":true,"resolvedProviderId":"ExdT24gJngk","sourceMetadataOrigin":"ytDlpCandidateMetadata"})), ..Candidate::default() }
}

#[test]
fn complete_caption_variants_match_only_after_resolution() {
    for title in [
        "Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)",
        "Nipsey Hussle × Bino Rideaux – Vibe With Me [featuring Leon Thomas] [Official Audio]",
        "Nipsey Hussle & Bino Rideaux - Vibe With Me[feat. Leon Thomas][Official Audio]",
        "Nipsey Hussle & Bino Rideaux - Vibe With Me (Official Audio) (feat. Leon Thomas)",
    ] {
        for (stage, expected) in [(MatchStage::Search, MatchAction::Inspect), (MatchStage::Resolved, MatchAction::Accept)] {
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
