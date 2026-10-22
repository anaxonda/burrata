#!/usr/bin/env python3
"""Structurally validate the frozen v3 source-backed catalog fixture."""
import json
import sys
from pathlib import Path

FIXTURE = Path(__file__).resolve().parents[1] / "docs/fixtures/recording-match-stage5-heldout-catalog-v3.json"
MODES = {"Exact", "Balanced", "Flexible"}
STAGES = {"Search", "Resolved"}


def target(case, stage, mode):
    proof = "recordingIdentifier" if case["groundTruth"] == "sameCatalogRecording" else "metadataOnly"
    if case["groundTruth"] == "sameCatalogRecording":
        relationship = "Exact"
        action = "Inspect" if stage == "Search" else "Accept"
    elif case["groundTruth"] == "differentCatalogRecordings":
        relationship, action = "Unresolved", "Reject"
    elif (
        case["groundTruth"] == "identifiedAlternativeRecording"
        and mode == "Flexible"
        and case.get("flexiblePolicyEligible") is not False
    ):
        relationship = "Alternative"
        action = "Inspect" if stage == "Search" else "Accept"
    else:
        relationship, action = "Unresolved", "Reject"
    return {"relationship": relationship, "action": action, "proofStrength": proof}


def main():
    fixture = Path(sys.argv[1]) if len(sys.argv) > 1 else FIXTURE
    data = json.loads(fixture.read_text())
    supported = {
        "burrata.stage5.heldout-catalog-identity.v3": "source-labels-frozen-before-v3-evaluator-run",
        "burrata.stage5.heldout-catalog-identity.v4": "policy-corrected-after-v3-initial-run; labels-frozen-before-v4-run",
    }
    assert data["fixtureId"] in supported
    assert data["status"] == supported[data["fixtureId"]]
    sources = {source["id"]: source for source in data["provenance"]["sources"]}
    assert len(sources) == len(data["provenance"]["sources"])
    assert len(data["cases"]) == 5 and len(data["policyProbes"]) == 1
    seen = set()
    for group, classification in ((data["cases"], "heldOut"), (data["policyProbes"], "developmentPolicyProbe")):
        for case in group:
            assert case["id"] not in seen
            seen.add(case["id"])
            assert case["classification"] == classification
            assert case["candidateCarrierSynthetic"] is True
            assert case["sourceRecords"] and set(case["sourceRecords"]) <= sources.keys()
            assert case["candidate"]["provider"] == "fixture"
            assert case["candidate"]["sourceMetadata"]["provider"] == "musicbrainz"
            assert case["request"]["title"] and case["request"]["artist"]
            assert set(case["expected"]) == STAGES
            for stage in STAGES:
                assert set(case["expected"][stage]) == MODES
                for mode in MODES:
                    assert case["expected"][stage][mode] == target(case, stage, mode)
    by_id = {case["id"]: case for case in data["cases"]}
    assert by_id["same-isrc-crazy-in-love-verified-spelling-alias"]["request"]["isrc"] == by_id["same-isrc-crazy-in-love-verified-spelling-alias"]["candidate"]["sourceMetadata"]["isrc"]
    assert by_id["crazy-in-love-no-rap-omits-requested-guest"]["request"]["isrc"] != by_id["crazy-in-love-no-rap-omits-requested-guest"]["candidate"]["sourceMetadata"]["isrc"]
    live = by_id["crazy-in-love-live-performance-alternative"]
    assert live["request"]["isrc"] != live["candidate"]["sourceMetadata"]["isrc"]
    if data["fixtureId"].endswith(".v4"):
        radio = by_id["u2-one-radio-edit-alternative"]
        assert radio["groundTruth"] == "identifiedAlternativeRecording"
        assert radio["flexiblePolicyEligible"] is False
        assert all(
            radio["expected"][stage][mode]
            == {"relationship": "Unresolved", "action": "Reject", "proofStrength": "metadataOnly"}
            for stage in STAGES for mode in MODES
        )
    alias = by_id["same-isrc-crazy-in-love-verified-spelling-alias"]["verifiedArtistNameAlias"]
    assert alias["provider"] == "musicbrainz" and alias["sourceRecord"] == "beyonce-name-aliases"
    assert alias["artistId"] == "859d0860-d480-4efd-970c-c05d5f1776b8"
    assert alias["requestedName"] == "Beyonce" and alias["candidateName"] == "Beyoncé"
    missing_duration = data["policyProbes"][0]
    assert missing_duration["candidate"]["durationSeconds"] is None
    assert missing_duration["request"]["isrc"] == missing_duration["candidate"]["sourceMetadata"]["isrc"]
    cells = (len(data["cases"]) + len(data["policyProbes"])) * 6
    print(f"PASS: {len(data['cases'])} frozen held-out metadata cases and 1 separate synthetic policy probe ({cells} target cells) in {data['fixtureId']}; no population accuracy claim")


if __name__ == "__main__":
    main()
