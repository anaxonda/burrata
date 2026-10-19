#!/usr/bin/env python3
"""Offline, privacy-preserving summaries of alias playback phase traces."""
import argparse
from collections import Counter, defaultdict
import json
import math
from pathlib import Path
import statistics


def distribution(values):
    values = sorted(values)
    return {"samples": len(values), "median": statistics.median(values) if values else None,
            "p95": values[max(0, math.ceil(len(values) * .95) - 1)] if values else None,
            "worst": max(values) if values else None}


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def read_events(path):
    events = []
    malformed = 0
    for line in Path(path).read_text().splitlines():
        if "burrata_playback_phase " not in line:
            continue
        try:
            event = json.loads(line.split("burrata_playback_phase ", 1)[1])
            if not isinstance(event, dict) or not isinstance(event.get("details", {}), dict):
                raise ValueError()
            events.append(event)
        except (ValueError, TypeError):
            malformed += 1
    return events, malformed


def summarize(events, warmup=0):
    callers = defaultdict(list)
    for event in events:
        if event.get("resolutionId") is not None:
            callers[str(event["resolutionId"])].append(event)
    excluded = set(list(callers)[:warmup])
    events = [e for e in events if str(e.get("resolutionId")) not in excluded]
    callers = {key: value for key, value in callers.items() if key not in excluded}
    metrics = defaultdict(list)
    outcomes = Counter()
    workers = defaultdict(list)
    joined = 0
    for key, group in callers.items():
        phases = {e.get("phase") for e in group}
        ends = [e for e in group if e.get("phase") == "alias_lookup_end"]
        discovery = bool(phases & {"alias_discovery_start", "alias_discovery_end", "alias_lookup_start"})
        cache = [e.get("details", {}) for e in group if e.get("phase") == "alias_cache_lookup_end"]
        category = "discovery" if discovery else "warm" if any((d.get("aliases", 0) or d.get("verifiedCreditSets", False)) and not d.get("stale", False) for d in cache) else "skipped"
        outcomes[category] += 1
        for end in ends:
            outcome = end.get("details", {}).get("outcome")
            # Never echo arbitrary detail strings (they may contain URLs/titles).
            outcomes["lookup_" + (outcome if outcome in {"ok", "success", "empty", "failed", "error", "timeout", "cancelled", "skipped", "hit", "miss", "completed"} else "other")] += 1
        joined += sum(e.get("details", {}).get("joined") is True for e in group if e.get("phase") == "alias_lookup_start")
    per_caller = {"alias_cache_lookup_end": "cache_lookup_ms", "alias_lookup_end": "caller_wait_ms",
                  "resolution_end": "resolution_ms", "alias_candidate_recheck": "candidate_recheck_ms"}
    for e in events:
        phase, details = e.get("phase"), e.get("details", {})
        duration = details.get("durationMs")
        if phase in per_caller:
            if phase == "resolution_end" and not number(duration):
                duration = e.get("elapsedMs")
            if number(duration):
                metrics[per_caller[phase]].append(duration)
        if phase in {"alias_discovery_start", "alias_discovery_end", "alias_discovery_work", "alias_metadata_request_end", "artist_alias_hydration"}:
            shared = e.get("sharedLookupId") or details.get("sharedLookupId")
            workers[str(shared or e.get("resolutionId"))].append(e)
    metadata_count = metadata_bytes = unknown_bytes = 0
    statuses = Counter()
    failed_workers = 0
    hydration_invocations = 0
    for group in workers.values():
        # A worker's discovery interval already contains its HTTP time. Keep them
        # separate rather than adding overlapping intervals to caller waits.
        ends = [e for e in group if e.get("phase") == "alias_discovery_end"]
        if ends:
            end = ends[0]["details"]
            if number(end.get("durationMs")):
                metrics["discovery_worker_wall_ms"].append(end["durationMs"])
            failed_workers += end.get("ok") is False
        hydration = [e.get("details", {}) for e in group if e.get("phase") == "artist_alias_hydration"]
        hydration_invocations += len(hydration)
        for d in hydration:
            if number(d.get("elapsedMs")):
                metrics["provider_hydration_ms"].append(d["elapsedMs"])
        seen = set()
        requests = []
        for e in group:
            if e.get("phase") != "alias_metadata_request_end":
                continue
            fingerprint = json.dumps({k: e.get(k) for k in ("phase", "atUnixMs", "elapsedMs", "details")}, sort_keys=True)
            if fingerprint not in seen:
                seen.add(fingerprint)
                requests.append(e["details"])
        metadata_count += len(requests)
        for d in requests:
            status = d.get("status")
            statuses[str(int(status)) if number(status) and 100 <= status <= 599 else "unknown"] += 1
            if number(d.get("durationMs")):
                metrics["metadata_request_ms"].append(d["durationMs"])
            if number(d.get("bytes")):
                metadata_bytes += d["bytes"]
            else:
                unknown_bytes += 1
        work = [e for e in group if e.get("phase") == "alias_discovery_work"]
        if work:
            for field, metric in (("queueMs", "metadata_queue_ms"), ("httpMs", "metadata_work_http_ms")):
                if number(work[0]["details"].get(field)):
                    metrics[metric].append(work[0]["details"][field])
        if not requests and (work or ends):
            d = (work or ends)[0]["details"]
            metadata_count += d.get("metadataRequests", 0) if number(d.get("metadataRequests")) else 0
            metadata_bytes += d.get("bytes", 0) if number(d.get("bytes")) else 0
    return {"callers": len(callers), "excludedWarmupCallers": len(excluded), "joinedCallers": joined,
            "outcomes": dict(outcomes), "workers": len(workers), "failedWorkers": failed_workers,
            "metadataRequests": metadata_count, "knownMetadataBytes": metadata_bytes,
            "metadataRequestScope": "direct public catalog HTTP only; provider session transport is opaque",
            "providerHydrationInvocations": hydration_invocations,
            "providerHydrationHttpRequests": None if hydration_invocations else 0,
            "metadataStatuses": dict(statuses),
            "requestsWithUnknownBytes": unknown_bytes,
            "timings": {name: distribution(metrics[name]) for name in
                        ("cache_lookup_ms", "caller_wait_ms", "discovery_worker_wall_ms", "metadata_request_ms", "candidate_recheck_ms", "resolution_ms", "metadata_queue_ms", "metadata_work_http_ms", "provider_hydration_ms")}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--label", default="unlabelled")
    parser.add_argument("--build", choices=("debug", "release"), required=True)
    parser.add_argument("--warmup", type=int, default=0, help="exclude first N resolution IDs per input")
    parser.add_argument("--output", type=Path, help="write summary JSON (otherwise stdout)")
    args = parser.parse_args()
    if args.warmup < 0:
        parser.error("--warmup must be nonnegative")
    report = {"label": args.label, "build": args.build,
              "interpretation": "Descriptive timings only; causal overhead requires comparable independent baseline runs. Durations overlap and must not be summed."}
    for name in ("candidate", "baseline"):
        path = getattr(args, name)
        if path:
            events, malformed = read_events(path)
            report[name] = summarize(events, args.warmup)
            report[name]["malformedTraceLines"] = malformed
    output = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(output)
    else:
        print(output, end="")


if __name__ == "__main__":
    main()
