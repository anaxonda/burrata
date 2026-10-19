#!/usr/bin/env python3
"""Bounded diagnostic: yt-dlp JSONL versus aggregate JSON search visibility.

Uses only public YouTube search metadata, ignores user configuration, does not
read cookies, and never resolves a media stream. Output is capped and excludes
raw stderr, URLs, and extractor payloads. Search eligibility is only a hint.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import selectors
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BIN = ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/yt-dlp"
ARTIST = "John Denver"
TITLE = "Matthew"
EXPECTED_DURATION = 222.426
QUERIES = ["john denver matthew", '"john denver" "matthew"']
REQUEST_TIMEOUT_S = 20.0
MAX_TOTAL_BYTES = 8 * 1024 * 1024
MAX_LINE_BYTES = 2 * 1024 * 1024
MAX_STDERR_BYTES = 64 * 1024
ROW_LIMIT = 10


def norm(value: str) -> str:
    value = value.lower()
    value = re.sub(r"\s*[\[(](official audio|official video|visualizer|official visualizer)[\])}]\s*$", "", value)
    return " ".join(re.findall(r"[a-z0-9]+", value))


def search_hint(item: dict) -> str:
    """Exact normalized text/duration hint, never a Rust gate or proof."""
    raw_title = str(item.get("title") or "")
    normalized = norm(raw_title)
    expected_full = norm(f"{ARTIST} {TITLE}")
    duration = item.get("duration")
    if normalized != expected_full:
        return "none"
    if not isinstance(duration, (int, float)) or isinstance(duration, bool) or not (30.0 < float(duration) <= 86_400.0):
        return "none"
    delta = abs(float(duration) - EXPECTED_DURATION)
    if delta <= 5.0:
        return "strong_title_duration_hint"
    if delta <= 7.0:
        return "provisional_title_duration_hint"
    return "duration_mismatch"


def entries_from(value):
    if isinstance(value, dict) and isinstance(value.get("entries"), list):
        return value["entries"]
    if isinstance(value, dict):
        return [value]
    return []


def safe_row(item: dict, ordinal: int, elapsed_ms: int) -> dict:
    return {
        "ordinal": ordinal,
        "arrivalMs": elapsed_ms,
        "id": str(item.get("id") or "")[:128],
        "title": str(item.get("title") or "")[:300],
        "durationSeconds": item.get("duration") if isinstance(item.get("duration"), (int, float)) else None,
        "searchHint": search_hint(item),
    }


def run_one(binary: Path, query: str, mode: str, run_number: int) -> dict:
    format_flag = "--dump-json" if mode == "jsonl" else "--dump-single-json"
    args = [
        str(binary), "--ignore-config", "--no-plugin-dirs", "--no-remote-components",
        "--no-js-runtimes", "--no-color", "--no-cache-dir", "--flat-playlist",
        "--lazy-playlist", "--skip-download", format_flag, "--", f"ytsearch{ROW_LIMIT}:{query}",
    ]
    started = time.monotonic()
    proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
    assert proc.stdout is not None
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    assert proc.stderr is not None
    selector.register(proc.stderr, selectors.EVENT_READ)
    buffer = bytearray()
    stderr_buffer = bytearray()
    total_bytes = 0
    rows: list[dict] = []
    first_row_ms = None
    first_hint_ms = None
    best_hint_ms = None
    overflow = False
    timed_out = False
    parse_error = False
    while True:
        remaining = REQUEST_TIMEOUT_S - (time.monotonic() - started)
        if remaining <= 0:
            timed_out = proc.poll() is None
            break
        events = selector.select(min(remaining, 0.2))
        if not events:
            if proc.poll() is not None:
                # Drain any bytes already buffered by the pipe.
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    break
            else:
                continue
        else:
            chunk = b""
            for key, _ in events:
                data = os.read(key.fileobj.fileno(), 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                if key.fileobj is proc.stderr:
                    remaining_stderr = MAX_STDERR_BYTES - len(stderr_buffer)
                    if remaining_stderr > 0:
                        stderr_buffer.extend(data[:remaining_stderr])
                else:
                    chunk += data
            if proc.poll() is not None and not selector.get_map():
                break
            if not chunk:
                continue
        total_bytes += len(chunk)
        if total_bytes > MAX_TOTAL_BYTES:
            overflow = True
            break
        buffer.extend(chunk)
        if len(buffer) > MAX_LINE_BYTES and b"\n" not in buffer:
            overflow = True
            break
        while b"\n" in buffer:
            line, _, rest = buffer.partition(b"\n")
            buffer = bytearray(rest)
            if not line:
                continue
            if len(line) > MAX_LINE_BYTES:
                overflow = True
                break
            try:
                decoded = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError):
                parse_error = True
                continue
            now_ms = round((time.monotonic() - started) * 1000)
            for item in entries_from(decoded):
                if not isinstance(item, dict) or len(rows) >= ROW_LIMIT:
                    continue
                row = safe_row(item, len(rows) + 1, now_ms)
                rows.append(row)
                if first_row_ms is None:
                    first_row_ms = now_ms
                if row["searchHint"] in {"strong_title_duration_hint", "provisional_title_duration_hint"}:
                    if first_hint_ms is None:
                        first_hint_ms = now_ms
                    if row["searchHint"] == "strong_title_duration_hint" and best_hint_ms is None:
                        best_hint_ms = now_ms
            if overflow:
                break
        if overflow:
            break
    selector.close()
    if timed_out or overflow:
        proc.kill()
    try:
        proc.wait(timeout=1)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
    # Classify capped stderr only; never write raw diagnostics that can contain
    # URLs or account-specific text.
    stderr_lower = stderr_buffer.decode("utf-8", "replace").lower()
    if timed_out:
        failure = "timeout"
    elif overflow:
        failure = "output_limit"
    elif proc.returncode != 0:
        if any(token in stderr_lower for token in ("429", "rate limit", "too many requests")):
            failure = "rate_limited"
        elif any(token in stderr_lower for token in ("network", "connection", "timed out", "temporary failure")):
            failure = "network_or_timeout"
        else:
            failure = "extractor_error"
    elif parse_error:
        failure = "partial_or_invalid_json"
    elif not rows:
        failure = "no_rows"
    else:
        failure = None
    return {
        "run": run_number,
        "format": mode,
        "query": query,
        "elapsedMs": round((time.monotonic() - started) * 1000),
        "firstRowAvailableMs": first_row_ms,
        "firstSearchHintMs": first_hint_ms,
        "firstStrongFlatHintMs": best_hint_ms,
        "rowCount": len(rows),
        "totalStdoutBytes": total_bytes,
        "exitCode": proc.returncode,
        "failure": failure,
        "failureEvidence": ("stderr contained network/connection/timeout wording" if failure == "network_or_timeout" else None),
        "rows": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BIN)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=3, help="paired repetitions per query")
    args = parser.parse_args()
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error(f"yt-dlp binary missing or not executable: {args.binary}")
    if not 1 <= args.runs <= 3:
        parser.error("runs must be between 1 and 3")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    version = subprocess.run([str(args.binary), "--version"], capture_output=True, text=True, timeout=5).stdout.strip()
    help_text = subprocess.run([str(args.binary), "--help"], capture_output=True, text=True, timeout=10).stdout
    required_flags = ["--dump-json", "--dump-single-json", "--flat-playlist", "--lazy-playlist", "--ignore-config", "--no-cache-dir"]
    missing = [flag for flag in required_flags if flag not in help_text]
    if missing:
        parser.error("bundled yt-dlp lacks required options: " + ", ".join(missing))
    # Alternating paired order controls for warm-up and transient service effects.
    schedule = []
    for query_number, query in enumerate(QUERIES, start=1):
        for pair in range(args.runs):
            modes = ["jsonl", "single"] if pair % 2 == 0 else ["single", "jsonl"]
            for mode in modes:
                schedule.append((query_number, query, pair + 1, mode))
    runs = []
    binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    script_sha256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()

    def save_checkpoint(complete: bool) -> None:
        payload = {
            "experiment": "yt-dlp-flat-jsonl-vs-dump-single-json",
            "dateUtc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "ytDlpVersion": version,
            "binarySha256": binary_sha256,
            "scriptSha256": script_sha256,
            "binary": str(args.binary.relative_to(ROOT)) if args.binary.is_relative_to(ROOT) else "custom",
            "publicQuery": {"artist": ARTIST, "title": TITLE, "durationSecondsFixture": EXPECTED_DURATION, "queries": QUERIES},
            "constraints": {"timeoutSecondsPerRequest": REQUEST_TIMEOUT_S, "maxStdoutBytes": MAX_TOTAL_BYTES, "maxLineBytes": MAX_LINE_BYTES, "maxStderrBytes": MAX_STDERR_BYTES, "maxRows": ROW_LIMIT, "readsUserConfig": False, "readsCookies": False, "extractsMedia": False, "flags": ["--ignore-config", "--no-plugin-dirs", "--no-remote-components", "--no-js-runtimes", "--no-cache-dir", "--flat-playlist", "--lazy-playlist", "--skip-download"]},
            "complete": complete,
        "interpretation": "Exact normalized title/duration hints are intentionally a lightweight diagnostic predicate, not parity with the Rust matcher and not proof of fresh extractor identity, recording/version, stream access, or playback safety.",
            "runs": runs,
        }
        temp = args.output.with_suffix(args.output.suffix + ".tmp")
        temp.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        temp.replace(args.output)

    for ordinal, (query_number, query, pair, mode) in enumerate(schedule, start=1):
        result = run_one(args.binary, query, mode, ordinal)
        result.update({"queryNumber": query_number, "pair": pair})
        runs.append(result)
        save_checkpoint(False)
        if result["failure"]:
            print(f"run {ordinal}/{len(schedule)}: {mode} query {query_number} => {result['failure']}", file=sys.stderr)
        else:
            print(f"run {ordinal}/{len(schedule)}: {mode} query {query_number} => {result['elapsedMs']} ms, first row {result['firstRowAvailableMs']} ms", file=sys.stderr)
    save_checkpoint(True)
    failures = sum(bool(run["failure"]) for run in runs)
    print(f"wrote {args.output}; {failures}/{len(runs)} request(s) had a bounded failure", file=sys.stderr)
    return 0 if failures < len(runs) else 2


if __name__ == "__main__":
    raise SystemExit(main())
