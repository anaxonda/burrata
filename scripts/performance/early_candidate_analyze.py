#!/usr/bin/env python3
"""Offline analysis for the early candidate timing capture; performs no network I/O."""
from __future__ import annotations

import hashlib
import json
import re
import statistics
import sys
from pathlib import Path


def norm(value: str) -> str:
    value = value.lower()
    value = re.sub(r"\s*[\[(](official audio|official video|visualizer|official visualizer)[\])}]\s*$", "", value)
    return " ".join(re.findall(r"[a-z0-9]+", value))


def classify(row: dict, expected: str, duration: float) -> str:
    title = str(row.get("title") or "")
    actual = norm(title)
    seconds = row.get("durationSeconds")
    if actual != expected or not isinstance(seconds, (int, float)) or isinstance(seconds, bool):
        return "none"
    if not (30 < float(seconds) <= 86_400):
        return "none"
    delta = abs(float(seconds) - duration)
    if delta <= 5.0:
        return "strong_title_duration_hint"
    if delta <= 7.0:
        return "provisional_title_duration_hint"
    return "none"


def median(values):
    values = [value for value in values if value is not None]
    return round(statistics.median(values), 1) if values else None


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: early_candidate_analyze.py INPUT.json OUTPUT.json")
    source, destination = map(Path, sys.argv[1:])
    raw = json.loads(source.read_text(encoding="utf-8"))
    expected = norm(f"{raw['publicQuery']['artist']} {raw['publicQuery']['title']}")
    duration = float(raw["publicQuery"]["durationSecondsFixture"])
    runs = []
    for old in raw["runs"]:
        run = dict(old)
        rows = []
        for item in old["rows"]:
            row = dict(item)
            row["searchHint"] = classify(row, expected, duration)
            rows.append(row)
        run["rows"] = rows
        hinted = [row for row in rows if row["searchHint"] != "none"]
        strong = [row for row in rows if row["searchHint"] == "strong_title_duration_hint"]
        run["firstTitleDurationHintMs"] = hinted[0]["arrivalMs"] if hinted else None
        run["firstStrongTitleDurationHintMs"] = strong[0]["arrivalMs"] if strong else None
        run["lastRowAvailableMs"] = max((row["arrivalMs"] for row in rows), default=None)
        run["lastRowToProcessExitMs"] = (
            run["elapsedMs"] - run["lastRowAvailableMs"]
            if run["lastRowAvailableMs"] is not None else None
        )
        run["eligibleHintIds"] = [row["id"] for row in hinted]
        run["strongHintIds"] = [row["id"] for row in strong]
        runs.append(run)

    paired = []
    for query_number in sorted({r["queryNumber"] for r in runs}):
        for pair in sorted({r["pair"] for r in runs if r["queryNumber"] == query_number}):
            match = [r for r in runs if r["queryNumber"] == query_number and r["pair"] == pair]
            jsonl = next(r for r in match if r["format"] == "jsonl")
            single = next(r for r in match if r["format"] == "single")
            paired.append({
                "queryNumber": query_number,
                "pair": pair,
                "jsonlFirstRowMs": jsonl["firstRowAvailableMs"],
                "singleFirstRowMs": single["firstRowAvailableMs"],
                "jsonlFirstHintMs": jsonl["firstTitleDurationHintMs"],
                "singleFirstHintMs": single["firstTitleDurationHintMs"],
                "jsonlFirstStrongHintMs": jsonl["firstStrongTitleDurationHintMs"],
                "singleFirstStrongHintMs": single["firstStrongTitleDurationHintMs"],
                "jsonlLastRowMs": jsonl["lastRowAvailableMs"],
                "singleLastRowMs": single["lastRowAvailableMs"],
                "jsonlExitMs": jsonl["elapsedMs"],
                "singleExitMs": single["elapsedMs"],
                "jsonlStrongHintIds": jsonl["strongHintIds"],
                "singleStrongHintIds": single["strongHintIds"],
                "strongHintIdIntersection": sorted(set(jsonl["strongHintIds"]) & set(single["strongHintIds"])),
            })
    summary = {}
    for mode in ("jsonl", "single"):
        subset = [r for r in runs if r["format"] == mode]
        summary[mode] = {
            "runs": len(subset),
            "requestExitMedianMs": median([r["elapsedMs"] for r in subset]),
            "firstRowMedianMs": median([r["firstRowAvailableMs"] for r in subset]),
            "firstTitleDurationHintMedianMs": median([r["firstTitleDurationHintMs"] for r in subset]),
            "firstStrongTitleDurationHintMedianMs": median([r["firstStrongTitleDurationHintMs"] for r in subset]),
            "lastRowMedianMs": median([r["lastRowAvailableMs"] for r in subset]),
            "lastRowToExitMedianMs": median([r["lastRowToProcessExitMs"] for r in subset]),
            "runsWithAnyTitleDurationHint": sum(bool(r["eligibleHintIds"]) for r in subset),
            "runsWithStrongTitleDurationHint": sum(bool(r["strongHintIds"]) for r in subset),
        }
    payload = {
        "analysis": "offline reclassification of captured flat search metadata",
        "input": source.name,
        "inputSha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "hintRule": "normalize title to exact `artist title`; require 30<duration<=86400; canonical-like hint when duration delta<=5s; weaker provisional-like hint when delta<=7s. This is not implementation parity or recording proof.",
        "summary": summary,
        "pairedRuns": paired,
        "runs": runs,
        "limits": [
            "Both arms used --flat-playlist and --lazy-playlist; this isolates output framing and does not reproduce production's non-lazy --dump-single-json command.",
            "Public result membership/order varied between paired requests; row IDs are retained for assessing that instability.",
            "Timing covers candidate JSON visibility and process exit only. It does not measure yt-dlp extraction, fresh identity verification, stream access, or playback startup.",
            "Search hints only locate interesting metadata rows. Correct recording/version and safe playback still require fresh extractor identity and access validation.",
        ],
    }
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"summary": summary, "output": str(destination)}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
