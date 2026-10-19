#!/usr/bin/env python3
"""Deterministic protocol simulation: extraction overlaps delayed search batches.

This models the proposed API semantics only; it is not production code and
does not claim measured yt-dlp extraction latency or playback behavior.
"""
from __future__ import annotations

import json
from pathlib import Path


def scenario(name, batches, candidates, batch_search_done_ms, extraction_ms, access_ms):
    """Run one fixture through whole-batch and incremental orchestration.

    Candidate fixture fields: arrivalMs, tier, freshIdentity, accessOk.
    Extraction starts only for candidates at or above provisional, but playback
    succeeds only when fresh identity and access proof both pass.
    """
    whole_batch_eligible = [c for c in candidates if c["tier"] in {"canonical", "provisional"}]
    priority = {"canonical": 0, "provisional": 1}
    chosen = min(whole_batch_eligible, key=lambda c: (priority[c["tier"]], c["arrivalMs"])) if whole_batch_eligible else None
    batch_attempts = 1 if chosen else 0
    batch_accept = bool(chosen and chosen["freshIdentity"] and chosen["accessOk"])
    batch_ready = batch_search_done_ms + extraction_ms + access_ms if batch_accept else None

    stream_now = 0
    stream_attempts = 0
    stream_accepted = None
    attempted = set()
    timeline = []
    for candidate in sorted(candidates, key=lambda c: c["arrivalMs"]):
        if candidate["tier"] not in {"canonical", "provisional"}:
            continue
        if stream_accepted:
            break
        # A result arriving while another extraction runs remains available;
        # candidates are considered by priority once the active proof ends.
        start = max(candidate["arrivalMs"], stream_now)
        if candidate["key"] in attempted:
            continue
        attempted.add(candidate["key"])
        stream_attempts += 1
        if stream_attempts > 2:
            raise AssertionError("shared extraction budget exceeded")
        identity_at = start + extraction_ms
        if candidate["freshIdentity"]:
            ready_at = identity_at + access_ms
            timeline.append({"candidate": candidate["key"], "tier": candidate["tier"], "startMs": start, "identityMs": identity_at, "accessMs": ready_at, "accepted": candidate["accessOk"]})
            if candidate["accessOk"]:
                stream_accepted = ready_at
                stream_now = ready_at
                break
            stream_now = ready_at
        else:
            timeline.append({"candidate": candidate["key"], "tier": candidate["tier"], "startMs": start, "identityMs": identity_at, "accepted": False, "rejection": "fresh identity/version mismatch"})
            stream_now = identity_at
    if stream_accepted and stream_attempts > 2:
        raise AssertionError("too many extractions")
    if stream_accepted is not None and stream_accepted > batch_search_done_ms + extraction_ms + access_ms * 10:
        raise AssertionError("unexpected simulation ordering")
    return {
        "scenario": name,
        "batchesArriveMs": batches,
        "searchDoneMs": batch_search_done_ms,
        "freshExtractionMs": extraction_ms,
        "accessValidationMs": access_ms,
        "wholeBatch": {"candidateStarted": chosen["key"] if chosen else None, "verifiedSourceReadyMs": batch_ready, "extractions": batch_attempts},
        "incremental": {"verifiedSourceReadyMs": stream_accepted, "extractions": stream_attempts, "candidateProofs": timeline, "searchCancelledAfterSuccess": stream_accepted is not None},
        "savedMs": (batch_ready - stream_accepted) if batch_ready is not None and stream_accepted is not None else None,
    }


def main():
    cases = [
        scenario(
            "early_canonical_candidate_passes_fresh_identity",
            [250, 700, 1200, 1600],
            [{"key": "canonical-a", "arrivalMs": 250, "tier": "canonical", "freshIdentity": True, "accessOk": True}],
            1800, 700, 150,
        ),
        scenario(
            "provisional_wrong_version_is_rejected_then_canonical_passes",
            [200, 500, 900, 1200],
            [
                {"key": "provisional-wrong-version", "arrivalMs": 200, "tier": "provisional", "freshIdentity": False, "accessOk": False},
                {"key": "canonical-correct-version", "arrivalMs": 500, "tier": "canonical", "freshIdentity": True, "accessOk": True},
            ],
            1200, 600, 100,
        ),
        scenario(
            "rejected_metadata_waits_for_later_exact_batch",
            [250, 800, 1400],
            [{"key": "provisional-only", "arrivalMs": 250, "tier": "provisional", "freshIdentity": False, "accessOk": False}],
            1400, 600, 100,
        ),
    ]
    assert cases[0]["incremental"]["extractions"] == 1
    assert cases[1]["incremental"]["extractions"] == 2
    assert cases[1]["incremental"]["candidateProofs"][0]["accepted"] is False
    assert cases[1]["incremental"]["verifiedSourceReadyMs"] is not None
    assert cases[2]["incremental"]["verifiedSourceReadyMs"] is None
    out = {
        "experiment": "deterministic-early-candidate-overlap-protocol-simulation",
        "synthetic": True,
        "productionCodeChanged": False,
        "notAProductionSchedulerTest": True,
        "interpretation": "Illustrative toy timing model only. It assumes a provisional candidate may speculatively consume one extraction slot while search continues, then a canonical candidate may use slot two after fresh identity/version failure. It does not verify production scheduling, safety, or measured latency. Any accepted source still requires fresh identity and access checks in the model.",
        "policyAssumptions": [
            "Whole-batch baseline waits until search completion and ranks canonical before provisional candidates.",
            "Incremental lane in this toy allows provisional speculation before the complete priority frontier is known; this differs from the conservative policy of waiting for all current higher-priority rows.",
            "Extraction and access durations are invented fixed values, not measured yt-dlp timings.",
            "The two-attempt limit is modeled from SourceTrace's request-wide budget; cancellation and deadlines are not measured here.",
        ],
        "cases": cases,
    }
    output = Path(__file__).resolve().parent / "results/early_candidate_overlap_simulation.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
