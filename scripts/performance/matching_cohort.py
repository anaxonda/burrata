#!/usr/bin/env python3
"""Sequential, source-frozen coordinator for public matching diagnostics.

Manifest: {"cases":[{"caseId":...,"track":"...json","settings":"...json",
"expectedSourceId":"optional"}]}. Paths are relative to the manifest.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import math
from datetime import datetime, timezone

MODES = ("exact", "balanced", "flexible")
SCHEMA = "burrata.production-resolver-diagnostic/v1"
TRACE_PARSER_VERSION = "native-and-unique-shared-lookup-v2"
TRACE_NOTE = ("observedResolverExtractionBudgetCounter is the maximum of emitted cumulative counter snapshots, "
              "not a physical extractor or HTTP request count; zero snapshots do not mean no extraction when "
              "logical extraction_start events are present. Shared lookup events are joined only when one "
              "native operation owns the lookup; ambiguous consumers are left unassigned.")
SECRET_KEY = re.compile(r"(credential|password|secret|token|cookie|authorization|api.?key|profile|library)", re.I)
TRACE_PREFIX = "burrata_playback_phase "
TRACE_PHASES = frozenset({
    "alias_cache_lookup_end", "alias_candidate_recheck", "alias_lookup_skipped", "alias_search_start",
    "bandcamp_fallback_end", "bandcamp_fallback_start", "candidate_gate", "candidate_gate_ambiguous_versions",
    "candidate_gate_evidence", "candidate_gate_evidence_rows", "canonical_search_warmup_end",
    "catalog_alias_preparation_end", "catalog_duration_lookup_end", "catalog_duration_lookup_start",
    "direct_youtube_end", "direct_youtube_failure", "direct_youtube_start", "early_candidate_batch",
    "early_candidate_final_rank", "early_candidate_verification_end", "early_candidate_verification_start",
    "extract_and_validate_end", "extract_and_validate_start", "extraction_end", "extraction_start",
    "full_identity_check", "identifier_lookup_end",
    "mapped_direct_youtube_end", "mapped_direct_youtube_failure", "mapped_direct_youtube_start",
    "mapping_identity_check", "mapping_resolve_end", "mapping_resolve_start", "provider_cooldown",
    "provider_start", "recovery_end", "recovery_start", "resolution_end", "resolution_start",
    "search_end", "search_start", "settings_read_end", "settings_read_start",
    "verified_source_transport_recovery_end", "verified_source_transport_recovery_start",
    "preview_audio_lookup_start", "preview_audio_lookup_join", "preview_audio_lookup_end",
    "preview_audio_lookup_cancelled",
})
EXTRACTION_COUNTER_KEYS = ("attemptsUsed", "extractionAttemptsUsed", "sharedExtractionAttemptsUsed")


def normalize_level(value):
    normalized = value.lower()
    if normalized not in MODES:
        raise argparse.ArgumentTypeError(f"level must be one of: {', '.join(MODES)}")
    return normalized


def build_parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", required=True)
    p.add_argument("--manifest", required=True)
    p.add_argument("--outputdir", required=True)
    p.add_argument("--reps", type=int, default=20)
    p.add_argument("--warmups", type=int, default=3)
    p.add_argument("--smoke", action="store_true")
    p.add_argument("--levels", nargs="+", type=normalize_level, choices=MODES,
                   help="matching levels to run (default: exact balanced flexible)")
    p.add_argument("--trace", action="store_true",
                   help="enable production phase traces and correlate allowlisted logical events")
    return p


def sha(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def check_public(value, where="root"):
    if isinstance(value, dict):
        for key, item in value.items():
            if SECRET_KEY.search(str(key)):
                raise ValueError(f"private or profile setting key disallowed: {where}.{key}")
            check_public(item, f"{where}.{key}")
    elif isinstance(value, list):
        for i, item in enumerate(value):
            check_public(item, f"{where}[{i}]")


def percentile(values, p):
    if not values:
        return None
    values = sorted(values)
    return values[max(0, min(len(values) - 1, int((len(values) * p + .999999) - 1)))]


def safe_report(path: Path):
    try:
        obj = json.loads(path.read_text())
        if obj.get("schema") != SCHEMA:
            return None, "INVALID_REPORT_SCHEMA"
        return obj, None
    except Exception:
        return None, "REPORT_MISSING_OR_INVALID"


def _new_trace_bucket():
    return {"matchedEventCount": 0, "phaseEventCounts": {}, "phaseDurationsMs": {},
            "observedResolverExtractionBudgetCounter": None, "nativeEventCount": 0,
            "sharedLookupLinkedEventCount": 0}


def parse_trace_log(text: str, native_operation_ids):
    """Correlate allowlisted events by native ID, with unique shared-lookup fallback for ID-less children."""
    operation_ids = set(native_operation_ids)
    parsed = []
    diagnostics = {"allowlistedEvents": 0, "directNativeEvents": 0,
                   "sharedLookupLinkedEvents": 0, "unknownNativeEvents": 0,
                   "unknownSharedLookupEvents": 0, "ambiguousSharedLookupEvents": 0,
                   "unmatchedIdlessEvents": 0, "malformedTraceLines": 0}
    for line in text.splitlines():
        if not line.startswith(TRACE_PREFIX):
            continue
        try:
            event = json.loads(line[len(TRACE_PREFIX):])
        except (json.JSONDecodeError, TypeError):
            diagnostics["malformedTraceLines"] += 1
            continue
        if not isinstance(event, dict):
            continue
        phase = event.get("phase")
        if phase not in TRACE_PHASES:
            continue
        details = event.get("details")
        if not isinstance(details, dict):
            details = {}
        native_id = event.get("nativeOperationId")
        shared_id = event.get("sharedLookupId")
        is_lookup_owner = phase in ("preview_audio_lookup_start", "preview_audio_lookup_join")
        if is_lookup_owner and isinstance(details.get("sharedLookupId"), str):
            shared_id = details["sharedLookupId"]
        # Keep only a small allowlisted projection; never carry arbitrary details forward.
        safe = {"nativeOperationId": native_id, "sharedLookupId": shared_id, "phase": phase,
                "details": {key: details.get(key) for key in
                            ("durationMs", "elapsedMs", *EXTRACTION_COUNTER_KEYS)
                            if isinstance(details.get(key), (int, float)) and not isinstance(details.get(key), bool)}}
        parsed.append(safe)

    lookup_consumers = {}
    for event in parsed:
        if event["phase"] not in ("preview_audio_lookup_start", "preview_audio_lookup_join"):
            continue
        owner = event["nativeOperationId"]
        shared = event["sharedLookupId"]
        if owner in operation_ids and isinstance(shared, str) and shared:
            lookup_consumers.setdefault(shared, set()).add(owner)

    buckets = {}
    for event in parsed:
        diagnostics["allowlistedEvents"] += 1
        native_id = event["nativeOperationId"]
        linked = False
        if isinstance(native_id, str) and native_id in operation_ids:
            owner = native_id
            diagnostics["directNativeEvents"] += 1
        elif native_id is None or native_id == "":
            shared_id = event["sharedLookupId"]
            consumers = lookup_consumers.get(shared_id, set()) if isinstance(shared_id, str) else set()
            if len(consumers) == 1:
                owner = next(iter(consumers))
                diagnostics["sharedLookupLinkedEvents"] += 1
                linked = True
            elif len(consumers) > 1:
                diagnostics["ambiguousSharedLookupEvents"] += 1
                continue
            elif isinstance(shared_id, str) and shared_id:
                diagnostics["unknownSharedLookupEvents"] += 1
                continue
            else:
                diagnostics["unmatchedIdlessEvents"] += 1
                continue
        else:
            diagnostics["unknownNativeEvents"] += 1
            continue

        bucket = buckets.setdefault(owner, _new_trace_bucket())
        bucket["matchedEventCount"] += 1
        bucket["nativeEventCount"] += int(not linked)
        bucket["sharedLookupLinkedEventCount"] += int(linked)
        phase = event["phase"]
        counts = bucket["phaseEventCounts"]
        counts[phase] = counts.get(phase, 0) + 1
        details = event["details"]
        for key in ("durationMs", "elapsedMs"):
            value = details.get(key)
            if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0:
                bucket["phaseDurationsMs"].setdefault(phase, []).append(float(value))
        for key in EXTRACTION_COUNTER_KEYS:
            value = details.get(key)
            if isinstance(value, int) and not isinstance(value, bool) and 0 <= value <= 2:
                previous = bucket["observedResolverExtractionBudgetCounter"]
                bucket["observedResolverExtractionBudgetCounter"] = max(previous if previous is not None else 0, value)
    return buckets, diagnostics


def parse_trace_events(text: str, native_operation_id: str):
    """Compatibility helper: return one operation's sanitized trace projection."""
    buckets, _ = parse_trace_log(text, [native_operation_id])
    return buckets.get(native_operation_id, _new_trace_bucket())


def phase_percentiles(values):
    clean = [value for value in values if isinstance(value, (int, float)) and math.isfinite(value) and value >= 0]
    if not clean:
        return {"sampleCount": 0, "p50": None, "p95": None}
    clean.sort()
    return {"sampleCount": len(clean), "p50": percentile(clean, .5),
            "p95": percentile(clean, .95) if len(clean) >= 20 else None}


def summarize_trace_scenarios(attempts):
    result = {}
    for scenario in ("cold", "warm"):
        rows = [row for row in attempts if row.get("scenario") == scenario and isinstance(row.get("trace"), dict)]
        phase_counts = {}
        phase_durations = {}
        extraction = []
        for row in rows:
            trace = row["trace"]
            for phase, count in trace.get("phaseEventCounts", {}).items():
                phase_counts[phase] = phase_counts.get(phase, 0) + count
            for phase, values in trace.get("phaseDurationsMs", {}).items():
                phase_durations.setdefault(phase, []).extend(values)
            used = trace.get("observedResolverExtractionBudgetCounter")
            if isinstance(used, int):
                extraction.append(used)
        result[scenario] = {
            "attemptsWithCorrelatedTrace": sum(row["trace"].get("matchedEventCount", 0) > 0 for row in rows),
            "attemptsWithoutCorrelatedEvents": sum(row["trace"].get("matchedEventCount", 0) == 0 for row in rows),
            "phaseEventCounts": phase_counts,
            "logicalExtractionStartEvents": phase_counts.get("extraction_start", 0),
            "phaseDurationObservationsMs": {phase: phase_percentiles(values)
                                             for phase, values in sorted(phase_durations.items())},
            "observedResolverExtractionBudgetCounter": {
                "observationCount": len(extraction), "sum": sum(extraction) if extraction else None,
                "p50": percentile(extraction, .5) if extraction else None,
                "p95": percentile(extraction, .95) if len(extraction) >= 20 else None,
                "max": max(extraction) if extraction else None,
            },
        }
    return result


def attempt_summary(report, trace_text="", trace_enabled=False):
    rows = report.get("attempts", [])
    trace_by_id, trace_diagnostics = ({}, {})
    if trace_enabled:
        trace_by_id, trace_diagnostics = parse_trace_log(
            trace_text, [row.get("nativeOperationId") for row in rows
                         if isinstance(row.get("nativeOperationId"), str)])
    all_rows = []
    for row in rows:
        trace = None
        native_id = row.get("nativeOperationId")
        if trace_enabled and isinstance(native_id, str):
            trace = trace_by_id.get(native_id, _new_trace_bucket())
        all_rows.append((row, trace))
    measured = [(row, trace) for row, trace in all_rows if row.get("warmup") is False]
    # Keep only numeric timing/resource fields and public source identity.
    out = []
    for r, trace in measured:
        identity = r.get("resolvedRecordingIdentity")
        provider_id = identity.get("providerId") if isinstance(identity, dict) else None
        out.append({k: r.get(k) for k in (
            "scenario", "pairIndex", "iteration", "ok", "failure", "resolveMs",
            "wallMs", "firstPcmFromRequestMs", "helperCpuMs", "reapedChildCpuMs"
        ) if isinstance(r.get(k), (str, int, float, bool)) or r.get(k) is None} | {"providerId": provider_id})
        if trace is not None:
            out[-1]["trace"] = trace
    groups = {}
    for scenario in ("cold", "warm"):
        rows_s = [r for r, _ in measured if r.get("scenario") == scenario]
        times = [r["resolveMs"] for r in rows_s if isinstance(r.get("resolveMs"), (int, float))]
        groups[scenario] = {
            "attempts": len(rows_s), "successes": sum(r.get("ok") is True for r in rows_s),
            "failures": sum(r.get("ok") is not True for r in rows_s),
            "resolveMsP50": percentile(times, .5),
            "resolveMsP95": percentile(times, .95) if len(times) >= 20 else None,
            "p95SampleCount": len(times) if len(times) >= 20 else 0,
        }
    pairs = {}
    for r, _ in measured:
        if isinstance(r.get("pairIndex"), int):
            pairs.setdefault(r["pairIndex"], {})[r.get("scenario")] = r
    source_differences = [i for i, pair in pairs.items()
            if pair.get("cold", {}).get("resolvedRecordingIdentity", {}).get("providerId") and
                          pair.get("warm", {}).get("resolvedRecordingIdentity", {}).get("providerId") and
                          pair["cold"]["resolvedRecordingIdentity"]["providerId"] != pair["warm"]["resolvedRecordingIdentity"]["providerId"]]
    result = {"runPassed": report.get("runPassed") is True, "attempts": out,
            "scenarios": groups, "pairedSourceIdentityDifferenceCount": len(source_differences),
            "pairedSourceIdentityDifferencePairIndexes": source_differences,
            "providerShutdownOk": report.get("providerShutdownOk") is True}
    if trace_enabled:
        result["traceByScenario"] = summarize_trace_scenarios(out)
        result["traceJoinDiagnostics"] = trace_diagnostics
    return result


def run(args):
    manifest = Path(args.manifest).resolve()
    binary = Path(args.binary).resolve()
    if not binary.is_file():
        raise ValueError("binary does not exist")
    data = json.loads(manifest.read_text())
    cases = data.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError("manifest must contain a non-empty cases list")
    outdir = Path(args.outputdir).resolve()
    outdir.mkdir(parents=True, exist_ok=True)
    modes = tuple(args.levels) if args.levels is not None else MODES
    if not modes or len(set(modes)) != len(modes):
        raise ValueError("levels must be a non-empty list without duplicates")
    if any(mode not in MODES for mode in modes):
        raise ValueError("unsupported matching level")
    reps, warmups = (1, 0) if args.smoke else (args.reps, args.warmups)
    if reps < 1 or (not args.smoke and reps < 20) or warmups < 0:
        raise ValueError("production cohort requires >=20 reps; use --smoke for one diagnostic")
    frozen = {"binarySha256": sha(binary), "manifestSha256": sha(manifest),
              "sourceRevision": subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True,
                  text=True).stdout.strip() or None,
              "repetitions": reps, "warmups": warmups,
              "levels": list(modes),
              "providerPolicy": "public settings fixture only; same fixture policy across modes",
              "startedAtUtc": datetime.now(timezone.utc).isoformat()}
    results = []
    for case in cases:
        case_id = case.get("caseId")
        if not isinstance(case_id, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+", case_id):
            raise ValueError("caseId must be a safe identifier")
        track = (manifest.parent / case["track"]).resolve()
        settings_path = (manifest.parent / case["settings"]).resolve()
        track_data = json.loads(track.read_text())
        settings = json.loads(settings_path.read_text())
        check_public(settings)
        case_entry = {"caseId": case_id, "trackSha256": sha(track), "settingsSha256": sha(settings_path), "modes": {}}
        for mode in modes:
            mode_settings = dict(settings)
            mode_settings["matchingLevel"] = mode
            mode_file = outdir / f".{case_id}-{mode}-settings.json"
            report_file = outdir / f".{case_id}-{mode}-raw.json"
            stdout_file = outdir / f".{case_id}-{mode}-stdout.log"
            stderr_file = outdir / f".{case_id}-{mode}-stderr.log"
            mode_file.write_text(json.dumps(mode_settings))
            os.chmod(mode_file, 0o600)
            cmd = [str(binary), "--track", str(track), "--settings", str(mode_file), "--output", str(report_file),
                   "--scenario", "paired", "--repetitions", str(reps), "--warmups", str(warmups)]
            if args.smoke:
                cmd.append("--smoke")
            child_env = os.environ.copy()
            if args.trace:
                child_env["BURRATA_PLAYBACK_TRACE"] = "1"
            else:
                child_env.pop("BURRATA_PLAYBACK_TRACE", None)
            with stdout_file.open("w") as stdout, stderr_file.open("w") as stderr:
                os.chmod(stdout_file, 0o600)
                os.chmod(stderr_file, 0o600)
                old_umask = os.umask(0o077)
                try:
                    proc = subprocess.run(cmd, stdout=stdout, stderr=stderr, text=True,
                                          env=child_env, check=False)
                finally:
                    os.umask(old_umask)
            if report_file.exists():
                os.chmod(report_file, 0o600)
            else:
                report_file.write_text(json.dumps({"schema": SCHEMA, "runPassed": False,
                                                   "harnessFailure": "PROCESS_DID_NOT_WRITE_REPORT",
                                                   "processExitCode": proc.returncode}))
                os.chmod(report_file, 0o600)
            obj, parse_failure = safe_report(report_file)
            trace_text = stderr_file.read_text(errors="replace") if args.trace else ""
            summary = attempt_summary(obj, trace_text, args.trace) if obj else {"runPassed": False, "attempts": [], "scenarios": {},
                                                        "pairedSourceIdentityDifferenceCount": 0,
                                                        "providerShutdownOk": False}
            summary["matchingLevel"] = mode
            if args.trace:
                summary["traceParserVersion"] = TRACE_PARSER_VERSION
                summary["traceMeasurementNote"] = TRACE_NOTE
            summary.update({"processExitCode": proc.returncode,
                            "failure": parse_failure or (obj.get("harnessFailure") if obj else "PROCESS_FAILED")})
            if obj:
                summary["diagnosticSourceSha256"] = obj.get("inputHashes", {}).get("exampleSourceSha256")
                policy = obj.get("streamingPolicy", {})
                summary["providerPolicy"] = {key: policy.get(key) for key in
                    ("matchingLevel", "preferred", "fallbackEnabled", "fallbackSources")}
            expected = case.get("expectedSourceId")
            if expected:
                observed = [r.get("providerId") for r in summary["attempts"] if r.get("providerId")]
                summary["expectedSourceIdMatchCount"] = sum(x == expected for x in observed)
                summary["expectedSourceIdMismatchCount"] = sum(x != expected for x in observed)
            case_entry["modes"][mode] = summary
            # stdout/stderr and original report may include URLs, headers, or private details.
            # Remove transient settings; raw JSON remains owner-readable for diagnosis.
            mode_file.unlink(missing_ok=True)
            del proc
        results.append(case_entry)
    aggregate = {"schema": "burrata.matching-performance-cohort/v1", "qualification": False,
                 "frozen": frozen, "cases": results,
                 "note": "Failures remain in attempt denominators; p95 is withheld unless >=20 measured timing samples. Trace fields count allowlisted logical events and observed resolver counter snapshots only; they do not represent HTTP request totals. Concurrent phase duration samples are not additive. Per-attempt RSS is omitted because available child RSS is a lifetime high-water mark."}
    target = outdir / "aggregate.json"
    target.write_text(json.dumps(aggregate, indent=2) + "\n")
    os.chmod(target, 0o644)
    return aggregate


def main():
    args = build_parser().parse_args()
    try:
        aggregate = run(args)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as e:
        print(f"matching cohort failed: {e}", file=sys.stderr)
        return 2
    failed = sum(not m["runPassed"] for c in aggregate["cases"] for m in c["modes"].values())
    print(json.dumps({"cases": len(aggregate["cases"]),
                      "levels": aggregate["frozen"]["levels"],
                      "modeRuns": len(aggregate["cases"]) * len(aggregate["frozen"]["levels"]),
                      "failedModeRuns": failed, "aggregate": str(Path(args.outputdir).resolve() / "aggregate.json")}))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
