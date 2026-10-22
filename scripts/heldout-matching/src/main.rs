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
use recording_match::{MatchAction, MatchLevel, MatchRelationship, MatchStage, ProofStrength};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn overlay<T: DeserializeOwned + Serialize + Default>(value: &Value) -> Result<T, String> {
    let mut merged = serde_json::to_value(T::default()).map_err(|e| e.to_string())?;
    let target = merged.as_object_mut().ok_or("default is not an object")?;
    for (key, field) in value.as_object().ok_or("fixture model is not an object")? {
        target.insert(key.clone(), field.clone());
    }
    serde_json::from_value(merged).map_err(|e| e.to_string())
}
fn parse_mode(s: &str) -> Result<MatchLevel, String> {
    match s {
        "Exact" => Ok(MatchLevel::Exact),
        "Balanced" => Ok(MatchLevel::Balanced),
        "Flexible" => Ok(MatchLevel::Flexible),
        _ => Err(format!("unknown mode {s}")),
    }
}
fn parse_stage(s: &str) -> Result<MatchStage, String> {
    match s {
        "Search" => Ok(MatchStage::Search),
        "Resolved" => Ok(MatchStage::Resolved),
        _ => Err(format!("unknown stage {s}")),
    }
}
fn parse_relationship(s: &str) -> Result<MatchRelationship, String> {
    match s {
        "Exact" => Ok(MatchRelationship::Exact),
        "Probable" => Ok(MatchRelationship::Probable),
        "Alternative" => Ok(MatchRelationship::Alternative),
        "Unresolved" => Ok(MatchRelationship::Unresolved),
        _ => Err(format!("unknown relationship {s}")),
    }
}
fn parse_action(s: &str) -> Result<MatchAction, String> {
    match s {
        "Accept" => Ok(MatchAction::Accept),
        "Enrich" => Ok(MatchAction::Enrich),
        "Inspect" => Ok(MatchAction::Inspect),
        "Reject" => Ok(MatchAction::Reject),
        _ => Err(format!("unknown action {s}")),
    }
}
fn parse_proof(s: &str) -> Result<ProofStrength, String> {
    match s {
        "metadataOnly" => Ok(ProofStrength::MetadataOnly),
        "recordingIdentifier" => Ok(ProofStrength::RecordingIdentifier),
        "audioFingerprint" => Ok(ProofStrength::AudioFingerprint),
        _ => Err(format!("unknown proof strength {s}")),
    }
}
fn main_path() -> PathBuf {
    env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/fixtures/recording-match-stage5-heldout-catalog-v2.json")
    })
}
fn run_group(cases: &[Value], classification: &str) -> Result<(usize, usize), String> {
    let (mut differences, mut evaluated) = (0, 0);
    for case in cases {
        let id = case["id"].as_str().ok_or("case id missing")?;
        let request = overlay::<Track>(&case["request"])?;
        let candidate = overlay::<Candidate>(&case["candidate"])?;
        for stage_name in ["Search", "Resolved"] {
            for mode_name in ["Exact", "Balanced", "Flexible"] {
                let expected = &case["expected"][stage_name][mode_name];
                let exp_rel = parse_relationship(
                    expected["relationship"]
                        .as_str()
                        .ok_or("expected relationship missing")?,
                )?;
                let exp_act = parse_action(
                    expected["action"]
                        .as_str()
                        .ok_or("expected action missing")?,
                )?;
                let exp_proof = parse_proof(
                    expected["proofStrength"]
                        .as_str()
                        .ok_or("expected proof missing")?,
                )?;
                let selected_mode = parse_mode(mode_name)?;
                let selected_stage = parse_stage(stage_name)?;
                let result = std::panic::catch_unwind(|| {
                    recording_match::assess(&request, &candidate, selected_mode, selected_stage)
                });
                evaluated += 1;
                match result {
                    Ok(actual) => {
                        let matched = actual.relationship == exp_rel
                            && actual.action == exp_act
                            && actual.proof_strength == exp_proof;
                        if !matched {
                            differences += 1;
                        }
                        let mark = if matched {
                            format!("{classification}-PASS")
                        } else {
                            format!("{classification}-DIFF")
                        };
                        println!("{mark} case={id} stage={stage_name} mode={mode_name} expected={:?}/{:?}/{:?} actual={:?}/{:?}/{:?} reasons={:?} carrierSynthetic={}", exp_rel, exp_act, exp_proof, actual.relationship, actual.action, actual.proof_strength, actual.reasons, case["candidateCarrierSynthetic"]);
                    }
                    Err(_) => {
                        differences += 1;
                        println!("{classification}-PANIC case={id} stage={stage_name} mode={mode_name} expected={:?}/{:?}/{:?} actual=panic reasons=unavailable carrierSynthetic={}", exp_rel, exp_act, exp_proof, case["candidateCarrierSynthetic"]);
                    }
                }
            }
        }
    }
    Ok((evaluated, differences))
}
fn run() -> Result<(), String> {
    let path = main_path();
    let root: Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| e.to_string())?;
    if root["fixtureId"] != "burrata.stage5.heldout-catalog-identity.v2"
        || root["status"] != "reviewed-after-v1-initial-run; labels-frozen-before-v2-run"
    {
        return Err("unsupported or unfrozen v2 fixture".into());
    }
    let cases = root["cases"].as_array().ok_or("held-out cases missing")?;
    let probes = root["policyProbes"]
        .as_array()
        .ok_or("policy probes missing")?;
    let (heldout_count, heldout_diffs) = run_group(cases, "HELDOUT")?;
    let (probe_count, probe_diffs) = run_group(probes, "POLICY")?;
    println!("heldoutEvaluated={heldout_count} heldoutDifferences={heldout_diffs}; policyProbeEvaluated={probe_count} policyProbeDifferences={probe_diffs}; no population accuracy estimate");
    if heldout_diffs + probe_diffs > 0 {
        Err(format!(
            "{} held-out and {} policy-probe expectations differ",
            heldout_diffs, probe_diffs
        ))
    } else {
        Ok(())
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
