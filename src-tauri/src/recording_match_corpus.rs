//! Public-only regression examples for recording identity decisions.
//!
//! This module is wired by lib.rs once the recording-match policy lands.

use crate::{
    models::{Candidate, Track},
    recording_match::{self, MatchLevel, MatchStage},
};
use serde_json::Value;

const CORPUS: &str = include_str!("../tests/fixtures/recording-match-corpus.json");

fn overlay<T: serde::de::DeserializeOwned + serde::Serialize + Default>(value: &Value) -> T {
    let mut merged = serde_json::to_value(T::default()).expect("default model serializes");
    let target = merged.as_object_mut().expect("model serializes as object");
    for (key, field) in value.as_object().expect("corpus record is an object") {
        target.insert(key.clone(), field.clone());
    }
    serde_json::from_value(merged).expect("corpus record overlays model defaults")
}

fn expected_relationship(label: &str) -> recording_match::MatchRelationship {
    use recording_match::MatchRelationship as R;
    match label {
        "Exact" => R::Exact,
        "Probable" => R::Probable,
        "Alternative" => R::Alternative,
        "Unresolved" => R::Unresolved,
        other => panic!("unknown relationship label {other}"),
    }
}

#[test]
fn public_corpus_records_expected_relationships_across_policy_levels() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("valid corpus JSON");
    assert_eq!(corpus["schemaVersion"], 1);
    let cases = corpus["cases"].as_array().expect("cases array");
    assert!(
        cases.len() >= 10,
        "corpus should preserve reported examples"
    );

    let levels = [
        ("Exact", MatchLevel::Exact),
        ("Balanced", MatchLevel::Balanced),
        ("Flexible", MatchLevel::Flexible),
    ];
    let mut ids = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        assert!(ids.insert(id), "duplicate corpus case {id}");
        let request = overlay::<Track>(&case["request"]);
        let candidate = overlay::<Candidate>(&case["candidate"]);
        let expected = case["expected"].as_object().expect("expected object");

        for (key, level) in levels {
            let value = expected
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{id} missing expected {key} relationship"));
            let assessment =
                recording_match::assess(&request, &candidate, level, MatchStage::Resolved);
            if assessment.relationship != expected_relationship(value) {
                failures.push(format!(
                    "{id} {key}: expected relationship {value}, got {assessment:?}"
                ));
            }
        }
        let actions = expected
            .get("actions")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("{id} missing per-level actions"));
        for (key, level) in levels {
            let action = actions
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{id} missing {key} action"));
            let expected_action = match action {
                "Accept" => recording_match::MatchAction::Accept,
                "Enrich" => recording_match::MatchAction::Enrich,
                "Inspect" => recording_match::MatchAction::Inspect,
                "Reject" => recording_match::MatchAction::Reject,
                other => panic!("unknown action {other}"),
            };
            let assessment =
                recording_match::assess(&request, &candidate, level, MatchStage::Resolved);
            if assessment.action != expected_action {
                failures.push(format!(
                    "{id} {key}: expected action {expected_action:?}, got {assessment:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn uncertain_reports_are_not_promoted_to_verified_recordings() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("valid corpus JSON");
    for case in corpus["cases"].as_array().unwrap() {
        let expected = case["expected"].as_object().unwrap();
        if expected.get("relationship").and_then(Value::as_str) == Some("needsEvidence") {
            for level in ["Exact", "Balanced"] {
                assert_eq!(expected[level], "Unresolved", "{}", case["id"]);
            }
            assert!(
                ["Unresolved", "Probable"]
                    .contains(&expected["Flexible"].as_str().unwrap_or_default()),
                "{} cannot become Exact under Flexible without source proof",
                case["id"]
            );
        }
    }
}
