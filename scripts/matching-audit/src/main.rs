#![allow(dead_code)]
#[path = "../../../src-tauri/src/matching.rs"]
mod matching;
#[path = "../../../src-tauri/src/models.rs"]
mod models;
#[path = "../../../src-tauri/src/recording_filename.rs"]
mod recording_filename;
#[path = "../../../src-tauri/src/recording_identity.rs"]
mod recording_identity;
#[path = "../../../src-tauri/src/recording_match.rs"]
mod recording_match;

use models::{Candidate, Track};
use recording_match::{
    MatchAction, MatchAssessment, MatchLevel, MatchRelationship, MatchStage, ProofStrength,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn overlay<T: DeserializeOwned + Serialize + Default>(value: &Value) -> Result<T, String> {
    let mut merged = serde_json::to_value(T::default()).map_err(|error| error.to_string())?;
    let target = merged
        .as_object_mut()
        .ok_or_else(|| "default model is not an object".to_owned())?;
    for (key, field) in value
        .as_object()
        .ok_or_else(|| "fixture model is not an object".to_owned())?
    {
        target.insert(key.clone(), field.clone());
    }
    serde_json::from_value(merged).map_err(|error| error.to_string())
}

fn mode(value: &str) -> Result<MatchLevel, String> {
    match value {
        "Exact" => Ok(MatchLevel::Exact),
        "Balanced" => Ok(MatchLevel::Balanced),
        "Flexible" => Ok(MatchLevel::Flexible),
        _ => Err(format!("unknown mode {value}")),
    }
}

fn stage(value: &str) -> Result<MatchStage, String> {
    match value {
        "Search" => Ok(MatchStage::Search),
        "Resolved" => Ok(MatchStage::Resolved),
        _ => Err(format!("unknown stage {value}")),
    }
}

fn relationship(value: &str) -> Result<MatchRelationship, String> {
    match value {
        "Exact" => Ok(MatchRelationship::Exact),
        "Probable" => Ok(MatchRelationship::Probable),
        "Alternative" => Ok(MatchRelationship::Alternative),
        "Unresolved" => Ok(MatchRelationship::Unresolved),
        _ => Err(format!("unknown expected relationship {value}")),
    }
}

fn proof_strength(value: &str) -> Result<ProofStrength, String> {
    match value {
        "metadataOnly" => Ok(ProofStrength::MetadataOnly),
        "recordingIdentifier" => Ok(ProofStrength::RecordingIdentifier),
        "audioFingerprint" => Ok(ProofStrength::AudioFingerprint),
        _ => Err(format!("unknown expected proof strength {value}")),
    }
}

fn evaluate(
    request_value: &Value,
    candidate_value: &Value,
    mode_name: &str,
    stage_name: &str,
) -> Result<MatchAssessment, String> {
    let request = overlay::<Track>(request_value)?;
    let candidate = overlay::<Candidate>(candidate_value)?;
    let selected_mode = mode(mode_name)?;
    let selected_stage = stage(stage_name)?;
    std::panic::catch_unwind(|| {
        recording_match::assess(&request, &candidate, selected_mode, selected_stage)
    })
    .map_err(|_| format!("panic at {stage_name}/{mode_name}"))
}

fn action(value: &str) -> Result<MatchAction, String> {
    match value {
        "Accept" => Ok(MatchAction::Accept),
        "Enrich" => Ok(MatchAction::Enrich),
        "Inspect" => Ok(MatchAction::Inspect),
        "Reject" => Ok(MatchAction::Reject),
        _ => Err(format!("unknown expected action {value}")),
    }
}

fn matrix_path() -> PathBuf {
    env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../src-tauri/tests/fixtures/recording-match-stage5-matrix-v1.json")
    })
}

fn run() -> Result<(), String> {
    let path = matrix_path();
    let root: Value = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
    .map_err(|error| error.to_string())?;
    if root["schemaVersion"] != 1 || root["schemaId"] != "burrata.recording-match.stage5-matrix" {
        return Err("unsupported stage-5 matrix schema".into());
    }
    let cases = root["cases"]
        .as_array()
        .ok_or_else(|| "matrix cases array is missing".to_owned())?;
    let mut evaluated = 0usize;
    let mut mismatches = 0usize;
    for case in cases {
        let id = case["id"]
            .as_str()
            .ok_or_else(|| "case id missing".to_owned())?;
        let labels = &case["labels"];
        let request = overlay::<Track>(&case["request"])?;
        let candidate = overlay::<Candidate>(&case["candidate"])?;
        for stage_name in ["Search", "Resolved"] {
            for mode_name in ["Exact", "Balanced", "Flexible"] {
                let expected = &case["expectations"][stage_name][mode_name];
                let expected_relationship =
                    relationship(expected["relationship"].as_str().ok_or_else(|| {
                        format!("{id}/{stage_name}/{mode_name}: relationship missing")
                    })?)?;
                let expected_action =
                    action(expected["action"].as_str().ok_or_else(|| {
                        format!("{id}/{stage_name}/{mode_name}: action missing")
                    })?)?;
                let expected_proof =
                    proof_strength(expected["proofStrength"].as_str().ok_or_else(|| {
                        format!("{id}/{stage_name}/{mode_name}: proofStrength missing")
                    })?)?;
                let selected_mode = mode(mode_name)?;
                let selected_stage = stage(stage_name)?;
                let assessed = std::panic::catch_unwind(|| {
                    recording_match::assess(&request, &candidate, selected_mode, selected_stage)
                });
                evaluated += 1;
                let actual = match assessed {
                    Ok(assessment) => assessment,
                    Err(_) => {
                        eprintln!(
                            "PANIC {id} {stage_name} {mode_name} proof={} synthetic={}",
                            labels["candidateProof"], labels["synthetic"]
                        );
                        mismatches += 1;
                        continue;
                    }
                };
                let matched = actual.relationship == expected_relationship
                    && actual.action == expected_action
                    && actual.proof_strength == expected_proof;
                if !matched {
                    mismatches += 1;
                }
                println!(
                    "{} case={} stage={} mode={} expected={:?}/{:?}/{:?} actual={:?}/{:?}/{:?} labels={} synthetic={}",
                    if matched { "PASS" } else { "DIFF" },
                    id,
                    stage_name,
                    mode_name,
                    expected_relationship,
                    expected_action,
                    expected_proof,
                    actual.relationship,
                    actual.action,
                    actual.proof_strength,
                    labels["candidateProof"].as_str().unwrap_or("unknown"),
                    labels["synthetic"].as_bool().unwrap_or(true),
                );
            }
        }
    }
    mismatches += run_metamorphic(cases, &root["metamorphicTests"]);
    println!("evaluated={evaluated} differences={mismatches}; fixture labels describe evidence basis, not empirical accuracy");
    if mismatches != 0 {
        return Err(format!(
            "{mismatches} stage/mode results differ from the reviewed contract matrix"
        ));
    }
    Ok(())
}

fn run_metamorphic(cases: &[Value], tests: &Value) -> usize {
    let mut failures = 0usize;
    for test in tests.as_array().into_iter().flatten() {
        let test_id = test["id"].as_str().unwrap_or("unknown");
        let source_id = test["sourceCaseId"].as_str().unwrap_or("");
        if source_id == "allCases" {
            for case in cases {
                let id = case["id"].as_str().unwrap_or("unknown");
                for stage_name in ["Search", "Resolved"] {
                    let mut accepted = [false; 3];
                    for (index, mode_name) in ["Exact", "Balanced", "Flexible"].iter().enumerate() {
                        match evaluate(&case["request"], &case["candidate"], mode_name, stage_name)
                        {
                            Ok(assessment) => {
                                accepted[index] = assessment.action == MatchAction::Accept
                            }
                            Err(error) => {
                                eprintln!("FAIL {test_id}/{id}/{stage_name}: {error}");
                                failures += 1;
                            }
                        }
                    }
                    if (accepted[0] && !accepted[1]) || (accepted[1] && !accepted[2]) {
                        eprintln!("FAIL {test_id}/{id}/{stage_name}: Accept eligibility is not cumulative Exact→Balanced→Flexible");
                        failures += 1;
                    }
                }
            }
            continue;
        }
        let Some(source) = cases.iter().find(|case| case["id"] == source_id) else {
            eprintln!("FAIL {test_id}: source case {source_id} missing");
            failures += 1;
            continue;
        };
        let mut request = source["request"].clone();
        let mut candidate = source["candidate"].clone();
        match test["transformation"].as_str().unwrap_or("") {
            "renameSourceMetadataArtistsToArtistNames" => {
                let metadata = candidate["sourceMetadata"].as_object_mut().unwrap();
                if let Some(artists) = metadata.remove("artists") {
                    metadata.insert("artistNames".into(), artists);
                }
            }
            "decomposeRequestAndCandidateArtistText" => {
                fn decompose(value: &mut Value) {
                    if let Some(text) = value.as_str() {
                        *value =
                            Value::String(text.replace('İ', "I\u{307}").replace('Å', "A\u{30A}"));
                    }
                }
                decompose(&mut request["artist"]);
                decompose(&mut candidate["title"]);
                if let Some(artists) = candidate["sourceMetadata"]["artists"].as_array_mut() {
                    for artist in artists {
                        decompose(artist);
                    }
                }
            }
            "addDifferentRequestAndCandidateIsrc" => {
                request["provenance"]["remoteCatalog"]["isrc"] =
                    Value::String("USAAA2400001".into());
                candidate["sourceMetadata"]["isrc"] = Value::String("USBBB2400002".into());
            }
            other => {
                eprintln!("FAIL {test_id}: unknown transformation {other}");
                failures += 1;
                continue;
            }
        }
        for stage_name in ["Search", "Resolved"] {
            for mode_name in ["Exact", "Balanced", "Flexible"] {
                let base = &source["expectations"][stage_name][mode_name];
                match (
                    evaluate(
                        &source["request"],
                        &source["candidate"],
                        mode_name,
                        stage_name,
                    ),
                    evaluate(&request, &candidate, mode_name, stage_name),
                ) {
                    (Ok(before), Ok(after)) => {
                        let expected = test["assertion"].as_str()
                            == Some("allSixCellsUnresolvedRejectMetadataOnly");
                        let okay = if expected {
                            after.relationship == MatchRelationship::Unresolved
                                && after.action == MatchAction::Reject
                                && after.proof_strength == ProofStrength::MetadataOnly
                        } else {
                            after.relationship
                                == relationship(
                                    base["relationship"].as_str().unwrap_or("Unresolved"),
                                )
                                .unwrap_or(MatchRelationship::Unresolved)
                                && after.action
                                    == action(base["action"].as_str().unwrap_or("Inspect"))
                                        .unwrap_or(MatchAction::Inspect)
                                && after.proof_strength
                                    == proof_strength(
                                        base["proofStrength"].as_str().unwrap_or("metadataOnly"),
                                    )
                                    .unwrap_or(ProofStrength::MetadataOnly)
                                && before.relationship == after.relationship
                                && before.action == after.action
                                && before.proof_strength == after.proof_strength
                        };
                        if !okay {
                            eprintln!("FAIL {test_id}/{stage_name}/{mode_name}: metamorphic expectation not met");
                            failures += 1;
                        }
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        eprintln!("FAIL {test_id}/{stage_name}/{mode_name}: {error}");
                        failures += 1;
                    }
                }
            }
        }
        if failures == 0 {
            println!("PASS metamorphic={test_id}");
        }
    }
    failures
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
