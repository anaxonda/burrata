#!/usr/bin/env python3
"""Validate the frozen held-out cases and separately reviewed policy probe."""
import json
from pathlib import Path

FIXTURE = Path(__file__).resolve().parents[1] / "docs/fixtures/recording-match-stage5-heldout-catalog-v2.json"
MODES = {"Exact", "Balanced", "Flexible"}
STAGES = {"Search", "Resolved"}


def validate_case(case, sources, classification):
    assert case["sourceRecords"] and set(case["sourceRecords"]) <= sources.keys()
    request, candidate, facts = case["request"], case["candidate"], case["catalogFacts"]
    assert request["artist"] and request["title"] and request["durationSeconds"] > 0
    assert request["artist"] == "The White Stripes" and request["version"] is None
    assert candidate["sourceMetadata"]["provider"] == "musicbrainz"
    assert candidate["provider"] == "fixture" and candidate["id"].startswith("fixture-")
    assert case["candidateCarrierSynthetic"] is True
    assert candidate["sourceMetadata"]["artistNames"] == facts["performerCredits"]
    assert facts["versionAnnotation"] is None and facts["versionNote"]
    assert facts["durationSeconds"] > 0 and candidate["sourceMetadata"]["duration"] == facts["durationSeconds"]
    assert request.get("isrc") == facts["requestIsrc"]
    assert candidate["sourceMetadata"].get("isrc") == facts["isrc"]
    assert facts["performerCredits"] and facts["title"] == candidate["sourceMetadata"]["catalogTitle"]
    assert "artists" not in request and "credits" not in request and "artists" not in candidate and "credits" not in candidate
    assert case["classification"] == classification
    assert set(case["expected"]) == STAGES
    for stage, outcomes in case["expected"].items():
        assert set(outcomes) == MODES
        for outcome in outcomes.values():
            assert outcome["proofStrength"] == ("recordingIdentifier" if case["groundTruth"] == "sameCatalogRecording" else "metadataOnly")
            if classification == "heldOut":
                if case["groundTruth"] == "sameCatalogRecording":
                    assert outcome["relationship"] == "Exact"
                    assert outcome["action"] == ("Inspect" if stage == "Search" else "Accept")
                else:
                    assert case["groundTruth"] == "differentCatalogRecordings"
                    assert outcome["relationship"] == "Unresolved" and outcome["action"] == "Reject"
            else:
                assert case["groundTruth"] == "undeterminedWithoutIdentifier"
                assert outcome["relationship"] == "Exact"
                assert outcome["action"] == ("Inspect" if stage == "Search" else "Accept")


def main():
    data = json.loads(FIXTURE.read_text())
    assert data["fixtureId"] == "burrata.stage5.heldout-catalog-identity.v2"
    assert data["status"] == "reviewed-after-v1-initial-run; labels-frozen-before-v2-run"
    sources = {s["id"]: s for s in data["provenance"]["sources"]}
    heldout = data["cases"]
    probes = data["policyProbes"]
    assert len(heldout) == 2 and len(probes) == 1
    ids = set()
    for group, classification in ((heldout, "heldOut"), (probes, "developmentPolicyProbe")):
        for case in group:
            assert case["id"] not in ids
            ids.add(case["id"])
            validate_case(case, sources, classification)
    positive, conflict = heldout
    probe = probes[0]
    assert positive["request"].get("isrc") == positive["candidate"]["sourceMetadata"].get("isrc")
    assert conflict["request"].get("isrc") != conflict["candidate"]["sourceMetadata"].get("isrc")
    assert probe["request"].get("isrc") is None and probe["candidate"]["sourceMetadata"].get("isrc") is None
    print(f"PASS: {len(heldout)} held-out cases ({len(heldout)*6} cells); {len(probes)} separate policy probe ({len(probes)*6} cells); no population accuracy claim")


if __name__ == "__main__":
    main()
