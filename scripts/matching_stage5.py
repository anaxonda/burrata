#!/usr/bin/env python3
"""Validate stage-5 fixtures and summarize paired cold/warm matching traces.

This tool does not run the matcher. Populate JSONL records from a frozen build's
instrumented runner, then use `summarize` without discarding failed attempts.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MATRIX = ROOT / "src-tauri/tests/fixtures/recording-match-stage5-matrix-v1.json"
MODES = ("Exact", "Balanced", "Flexible")
STAGES = ("Search", "Resolved")
RELATIONSHIPS = {"Exact", "Probable", "Alternative", "Unresolved"}
ACTIONS = {"Accept", "Enrich", "Inspect", "Reject"}
PROOF_STRENGTHS = {"metadataOnly", "recordingIdentifier", "audioFingerprint"}
CONDITIONS = {"cold", "warm"}
PHASES = (
    "parseMs",
    "comparisonMs",
    "aliasLookupMs",
    "providerSearchMs",
    "extractionMs",
    "verificationMs",
    "accessValidationMs",
    "resolutionMs",
    "firstAudioMs",
)
COUNTS = ("cacheHits", "externalRequests", "extractionAttempts")
BUILD_FIELDS = ("gitCommit", "artifactSha256", "evaluatorVersion", "evidenceVersion", "policyVersion", "platform")
MIN_P95_OBSERVATIONS = 20


class ValidationError(ValueError):
    pass


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ValidationError(message)


def validate_matrix(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValidationError(f"cannot read matrix {path}: {error}") from error
    _require(data.get("schemaVersion") == 1, "matrix schemaVersion must be 1")
    _require(data.get("schemaId") == "burrata.recording-match.stage5-matrix", "unexpected matrix schemaId")
    _require(data.get("modes") == list(MODES), "matrix must include Exact, Balanced, Flexible in order")
    _require(data.get("stages") == list(STAGES), "matrix must include Search and Resolved in order")
    tests = data.get("metamorphicTests")
    _require(isinstance(tests, list) and len(tests) >= 4, "matrix needs explicit metamorphic tests")
    for test in tests:
        _require(isinstance(test, dict) and isinstance(test.get("id"), str), "metamorphic test needs an id")
        _require(test.get("evidenceBasis") == "syntheticPolicyProbe", f"{test.get('id')}: metamorphic inputs must be labeled synthetic")
        _require(isinstance(test.get("transformation"), str) and isinstance(test.get("assertion"), str), f"{test.get('id')}: transformation/assertion required")
    cases = data.get("cases")
    _require(isinstance(cases, list) and cases, "matrix needs a non-empty cases array")
    ids: set[str] = set()
    case_ids: set[str] = set()
    for case in cases:
        _require(isinstance(case, dict), "each case must be an object")
        case_id = case.get("id")
        _require(isinstance(case_id, str) and case_id and case_id not in ids, f"duplicate or empty case id: {case_id!r}")
        ids.add(case_id)
        case_ids.add(case_id)
        labels = case.get("labels")
        _require(isinstance(labels, dict), f"{case_id}: labels object missing")
        _require(labels.get("evidenceBasis") in {"providerShapedPublicRegression", "publicSourceObservation", "syntheticPolicyProbe", "reportedUnresolvedCase"}, f"{case_id}: evidenceBasis is not explicit")
        _require(isinstance(labels.get("candidateProof"), str) and labels["candidateProof"], f"{case_id}: candidateProof missing")
        _require(isinstance(labels.get("uncertainty"), str) and labels["uncertainty"], f"{case_id}: uncertainty must be explained")
        _require(isinstance(labels.get("sourceReference"), str) and labels["sourceReference"], f"{case_id}: sourceReference missing")
        _require(isinstance(labels.get("synthetic"), bool), f"{case_id}: synthetic flag missing")
        if labels["evidenceBasis"] == "syntheticPolicyProbe":
            _require(labels["synthetic"] is True, f"{case_id}: synthetic policy probe cannot be labeled real")
        _require(isinstance(case.get("request"), dict) and isinstance(case.get("candidate"), dict), f"{case_id}: full request and candidate objects required")
        expectations = case.get("expectations")
        _require(isinstance(expectations, dict) and set(expectations) == set(STAGES), f"{case_id}: expectations must cover both stages")
        for stage in STAGES:
            cells = expectations[stage]
            _require(isinstance(cells, dict) and set(cells) == set(MODES), f"{case_id}/{stage}: expectations must cover all modes")
            for mode in MODES:
                cell = cells[mode]
                _require(isinstance(cell, dict), f"{case_id}/{stage}/{mode}: expectation must be an object")
                _require(cell.get("relationship") in RELATIONSHIPS, f"{case_id}/{stage}/{mode}: invalid relationship")
                _require(cell.get("action") in ACTIONS, f"{case_id}/{stage}/{mode}: invalid action")
                _require(cell.get("proofStrength") in PROOF_STRENGTHS, f"{case_id}/{stage}/{mode}: invalid proofStrength")
    for test in tests:
        source_id = test.get("sourceCaseId")
        _require(source_id == "allCases" or source_id in case_ids, f"{test['id']}: unknown sourceCaseId")
        _require(test.get("assertion") in {"preserveAllSixOutcomesAndProofStrength", "allSixCellsUnresolvedRejectMetadataOnly", "acceptanceSetIsMonotonicExactToBalancedToFlexible"}, f"{test['id']}: unknown metamorphic assertion")
    return data


def _load_rows(path: Path) -> list[dict[str, Any]]:
    rows = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ValidationError(f"cannot read observations {path}: {error}") from error
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValidationError(f"{path}:{line_number}: invalid JSON: {error}") from error
        _validate_row(row, f"{path}:{line_number}")
        rows.append(row)
    _require(rows, f"no observation rows in {path}")
    return rows


def _validate_row(row: dict[str, Any], where: str) -> None:
    _require(isinstance(row, dict), f"{where}: row must be an object")
    _require(row.get("schemaVersion") == 1, f"{where}: schemaVersion must be 1")
    for key in ("runId", "pairId", "caseId", "operation"):
        _require(isinstance(row.get(key), str) and row[key], f"{where}: {key} missing")
    _require(row.get("condition") in CONDITIONS, f"{where}: condition must be cold or warm")
    _require(row.get("mode") in MODES and row.get("stage") in STAGES, f"{where}: invalid mode/stage")
    build = row.get("frozenBuild")
    _require(isinstance(build, dict), f"{where}: frozenBuild missing")
    for field in BUILD_FIELDS:
        _require(isinstance(build.get(field), str) and build[field], f"{where}: frozenBuild.{field} missing")
    request = row.get("requestIdentity")
    candidate = row.get("candidateIdentity")
    _require(isinstance(request, dict) and all(isinstance(request.get(key), str) and request[key] for key in ("provider", "providerId", "title", "artist")), f"{where}: request identity incomplete")
    _require(isinstance(candidate, dict) and all(isinstance(candidate.get(key), str) and candidate[key] for key in ("provider", "id")), f"{where}: candidate identity incomplete")
    outcome = row.get("outcome")
    _require(isinstance(outcome, dict) and outcome.get("status") in {"success", "failure"}, f"{where}: outcome status must retain success/failure")
    _require(isinstance(outcome.get("failureStage", ""), str), f"{where}: failureStage must be a string")
    timings = row.get("timingsMs")
    _require(isinstance(timings, dict), f"{where}: timingsMs missing")
    for name, value in timings.items():
        _require(name in PHASES and isinstance(value, (int, float)) and math.isfinite(value) and value >= 0, f"{where}: invalid timing {name}={value!r}")
    counts = row.get("counts")
    _require(isinstance(counts, dict), f"{where}: counts missing")
    for name in COUNTS:
        _require(isinstance(counts.get(name), int) and counts[name] >= 0, f"{where}: invalid count {name}")


def _percentile(values: list[float], percent: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    position = (len(ordered) - 1) * percent / 100
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def _pair_identity(row: dict[str, Any]) -> str:
    return json.dumps({
        "caseId": row["caseId"],
        "operation": row["operation"],
        "mode": row["mode"],
        "stage": row["stage"],
        "frozenBuild": row["frozenBuild"],
        "requestIdentity": row["requestIdentity"],
        "candidateIdentity": row["candidateIdentity"],
    }, sort_keys=True, ensure_ascii=False)


def summarize(path: Path) -> tuple[str, dict[str, Any]]:
    rows = _load_rows(path)
    by_pair: dict[str, dict[str, list[dict[str, Any]]]] = defaultdict(lambda: defaultdict(list))
    by_group: dict[tuple[str, str, str, str, str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        identity = _pair_identity(row)
        by_pair[row["pairId"]][row["condition"]].append({"identity": identity, "row": row})
        by_group[(row["operation"], row["stage"], row["mode"], row["condition"], row["requestIdentity"]["provider"], row["candidateIdentity"]["provider"])].append(row)
    pair_issues = []
    for pair_id, conditions in sorted(by_pair.items()):
        if not conditions.get("cold") or not conditions.get("warm"):
            pair_issues.append(f"{pair_id}: missing cold or warm observations")
        elif len(conditions["cold"]) != len(conditions["warm"]):
            pair_issues.append(f"{pair_id}: cold/warm repetition counts differ")
        elif {item["identity"] for item in conditions["cold"]} != {item["identity"] for item in conditions["warm"]}:
            pair_issues.append(f"{pair_id}: cold/warm request, candidate, mode, stage, or frozen build differs")

    summary = {"observations": len(rows), "pairs": len(by_pair), "pairIssues": pair_issues, "groups": []}
    lines = ["Cold/warm matching measurements", "", f"Rows: {len(rows)} · matched-pair IDs: {len(by_pair)} · pair issues: {len(pair_issues)}", "", f"Failures remain in latency samples when phase timings exist. p95 is suppressed below n={MIN_P95_OBSERVATIONS}; report p50 for smaller samples without implying a stable tail estimate. `firstAudioMs` is reported separately from resolver latency and does not by itself establish audible-start success.", "", "| Operation | Stage | Mode | Condition | Request provider | Candidate provider | n | Success | Failure | Resolution p50/p95 ms | First audio p50/p95 ms | External requests | Extractions | Cache hits |", "|---|---|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for (operation, stage, mode, condition, request_provider, candidate_provider), group in sorted(by_group.items()):
        success = sum(row["outcome"]["status"] == "success" for row in group)
        failure = len(group) - success
        resolution = [row["timingsMs"]["resolutionMs"] for row in group if "resolutionMs" in row["timingsMs"]]
        audio = [row["timingsMs"]["firstAudioMs"] for row in group if "firstAudioMs" in row["timingsMs"]]
        external = sum(row["counts"]["externalRequests"] for row in group)
        extraction = sum(row["counts"]["extractionAttempts"] for row in group)
        cache_hits = sum(row["counts"]["cacheHits"] for row in group)
        def p50p95(values: list[float]) -> str:
            if not values:
                return "—"
            p95 = f"{_percentile(values, 95):.1f}" if len(values) >= MIN_P95_OBSERVATIONS else "suppressed"
            return f"{_percentile(values, 50):.1f}/{p95}"
        lines.append(f"| {operation} | {stage} | {mode} | {condition} | {request_provider} | {candidate_provider} | {len(group)} | {success} | {failure} | {p50p95(resolution)} | {p50p95(audio)} | {external} | {extraction} | {cache_hits} |")
        summary["groups"].append({"operation":operation,"stage":stage,"mode":mode,"condition":condition,"requestProvider":request_provider,"candidateProvider":candidate_provider,"n":len(group),"success":success,"failure":failure,"resolutionP50Ms":_percentile(resolution,50),"resolutionP95Ms":_percentile(resolution,95) if len(resolution) >= MIN_P95_OBSERVATIONS else None,"resolutionP95Suppressed":len(resolution) < MIN_P95_OBSERVATIONS,"firstAudioP50Ms":_percentile(audio,50),"firstAudioP95Ms":_percentile(audio,95) if len(audio) >= MIN_P95_OBSERVATIONS else None,"firstAudioP95Suppressed":len(audio) < MIN_P95_OBSERVATIONS,"externalRequests":external,"extractionAttempts":extraction,"cacheHits":cache_hits})
    if pair_issues:
        lines.extend(["", "Pairing issues:", *[f"- {issue}" for issue in pair_issues]])
    return "\n".join(lines) + "\n", summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate-matrix", help="validate all six mode/stage cells in the versioned matrix")
    validate.add_argument("matrix", nargs="?", type=Path, default=DEFAULT_MATRIX)
    report = subparsers.add_parser("summarize", help="summarize JSONL cold/warm traces without dropping failures")
    report.add_argument("observations", type=Path)
    report.add_argument("--json-output", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "validate-matrix":
            data = validate_matrix(args.matrix)
            print(f"validated {len(data['cases'])} cases × {len(MODES)} modes × {len(STAGES)} stages")
            return 0
        text, summary = summarize(args.observations)
        print(text, end="")
        if args.json_output:
            args.json_output.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        return 0 if not summary["pairIssues"] else 2
    except ValidationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
