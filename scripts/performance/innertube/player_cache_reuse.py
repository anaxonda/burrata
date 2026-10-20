#!/usr/bin/env python3
"""Compare fresh workers with fresh versus retained youtubei.js player cache.

All attempts use a disposable run directory and anonymous workers. The shared
arm retains only youtubei.js's versioned player record; its session-data cache
entry is deleted before and after each worker. No production resolver imports
this module.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import tempfile
import time
import unicodedata
from pathlib import Path

from serial_same_worker import (BoundedLineReader, HERE, NODE, ROOT, WORKER, ProtocolError,
                                safe_response, terminate_group)

DEFAULT_OUTER_S = 900.0
REQUEST_S = 20.0
SESSION_CACHE_KEY = "innertube_session_data"


def validate_track(track: dict) -> None:
    if (not isinstance(track, dict) or not re.fullmatch(r"[A-Za-z0-9_-]{11}", track.get("videoId", "")) or
            track.get("formatId") != 251 or not isinstance(track.get("titleNeedle"), str) or
            not track["titleNeedle"] or not isinstance(track.get("channelNeedle"), str) or
            not track["channelNeedle"] or not isinstance(track.get("durationSeconds"), (int, float)) or
            not 0 < track["durationSeconds"] < 86_400 or track.get("codec") != 'audio/webm; codecs="opus"'):
        raise ValueError("invalid explicit itag-251 track row")


def session_cache_path(data_dir: Path) -> Path:
    return data_dir / "youtube-session" / SESSION_CACHE_KEY


def clear_session_cache(data_dir: Path) -> bool:
    """Delete transient youtubei session serialization, preserving Player cache."""
    path = session_cache_path(data_dir)
    try:
        path.unlink()
        return True
    except FileNotFoundError:
        return False


def player_cache_inventory(data_dir: Path) -> tuple[int, int]:
    """Return count/bytes excluding transient session cache; never report names."""
    directory = data_dir / "youtube-session"
    if not directory.is_dir():
        return 0, 0
    files = [p for p in directory.iterdir() if p.is_file() and p.name != SESSION_CACHE_KEY]
    return len(files), sum(p.stat().st_size for p in files)


def metadata_match(row: dict, track: dict) -> bool:
    metadata = row.get("providerMetadata", {})
    normalize = lambda value: " ".join(unicodedata.normalize("NFKC", value).casefold().split()) if isinstance(value, str) else ""
    return (metadata.get("videoId") == track["videoId"] and
            normalize(track["titleNeedle"]) in normalize(metadata.get("title")) and
            normalize(track["channelNeedle"]) in normalize(metadata.get("channel")) and
            isinstance(metadata.get("durationSeconds"), (int, float)) and
            abs(metadata["durationSeconds"] - track["durationSeconds"]) <= 1)


def run(output: Path, track: dict, *, node: Path = NODE, worker: Path = WORKER,
        pairs: int = 20, outer_s: float = DEFAULT_OUTER_S, request_s: float = REQUEST_S) -> dict:
    validate_track(track)
    if isinstance(pairs, bool) or not isinstance(pairs, int) or not 1 <= pairs <= 20:
        raise ValueError("pairs must be an integer from 1 through 20")
    if isinstance(outer_s, bool) or not isinstance(outer_s, (int, float)) or not 1 <= outer_s <= 900:
        raise ValueError("outer budget must be between 1 and 900 seconds")
    if isinstance(request_s, bool) or not isinstance(request_s, (int, float)) or not 0 < request_s <= REQUEST_S:
        raise ValueError("request budget must be greater than 0 and at most 20 seconds")

    outer_start = time.monotonic()
    outer_deadline = outer_start + outer_s
    # Required live dependency metadata must fail before allocating run scratch.
    package = json.loads((HERE / "node_modules/youtubei.js/package.json").read_text(encoding="utf-8"))
    run_scratch = tempfile.TemporaryDirectory(prefix="burrata-player-cache-pairs-")
    run_root = Path(run_scratch.name)
    shared_data = run_root / "shared-data"
    shared_data.mkdir(mode=0o700)
    rows: list[dict] = []

    def checkpoint(status="running"):
        artifact = {
            "schema": "innertube-player-cache-reuse-v1", "status": status,
            "scope": "fresh anonymous worker per attempt; one explicit public ID; strict itag251; 5s PCM",
            "cachePolicy": "shared arm retains only youtubei.js player cache records; innertube_session_data deleted before/after every worker; XDG roots and token worker are per-process",
            "timingOrigin": "firstPcmFromCommandMs begins at worker JSONL receipt; processStartToResponseMs includes worker spawn and request through full response",
            "youtubeiJsVersion": package.get("version"), "requestBudgetMs": round(request_s * 1000),
            "outerBudgetMs": round(outer_s * 1000), "requestedPairs": pairs,
            "driverSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "totalWallMs": round((time.monotonic() - outer_start) * 1000, 3), "attempts": rows,
        }
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(artifact, indent=2) + "\n", encoding="utf-8")
        return artifact

    def one_attempt(label: str, pair: int, arm: str, cache_data: Path) -> dict:
        attempt_id = f"{label}-pair-{pair:02d}-{arm}"
        if time.monotonic() >= outer_deadline:
            row = {"id": attempt_id, "pair": pair, "arm": arm, "status": "NOT_RUN", "reason": "outer_deadline"}
            rows.append(row); checkpoint(); return row

        cache_data.mkdir(parents=True, exist_ok=True, mode=0o700)
        # This cache record contains the previous session visitor context. Remove
        # it before launch so only Player.fromCache can yield a retained hit.
        clear_session_cache(cache_data)
        session_absent_before = not session_cache_path(cache_data).exists()
        before_count, before_bytes = player_cache_inventory(cache_data)
        scratch = tempfile.TemporaryDirectory(prefix="burrata-player-cache-worker-")
        scratch_path = Path(scratch.name)
        if arm == "fresh_cache":
            data_dir = scratch_path / "data"
            data_dir.mkdir(mode=0o700)
        else:
            data_dir = cache_data
        xdg = {}
        for key, name in (("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "xdg-cache"),
                          ("XDG_DATA_HOME", "state")):
            path = scratch_path / name
            path.mkdir(mode=0o700)
            xdg[key] = str(path)
        env = os.environ.copy()
        env.update(xdg)
        env.update({"BURRATA_BENCH_DATA_DIR": str(data_dir), "BURRATA_BENCH_CLIENT": "MWEB",
                    "BURRATA_BENCH_TOKEN_MODE": "bgutil", "BURRATA_BENCH_IDLE_MS": "30000",
                    "BURRATA_BENCH_FFMPEG": str(ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/ffmpeg"),
                    "BURRATA_BENCH_PARALLEL_PLAYER": "0"})
        proc = None
        try:
            process_started = time.monotonic()
            try:
                proc = subprocess.Popen([str(node), str(worker)], cwd=ROOT, env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, start_new_session=True, bufsize=0)
                process_spawn_ms = (time.monotonic() - process_started) * 1000
            except (OSError, subprocess.SubprocessError) as exc:
                row = {"id": attempt_id, "pair": pair, "arm": arm, "status": "startup_failed",
                       "failure": "PROCESS_LAUNCH_FAILED", "errorType": type(exc).__name__}
                rows.append(row); checkpoint(); return row
            assert proc.stdin is not None and proc.stdout is not None
            started = time.monotonic()
            deadline = min(started + request_s, outer_deadline)
            request = {"id": attempt_id, "action": "play", "videoId": track["videoId"],
                       "expectedVideoId": track["videoId"], "formatId": 251, "seconds": 5,
                       "budgetMs": max(1, int((deadline - started) * 1000))}
            proc.stdin.write((json.dumps(request, separators=(",", ":")) + "\n").encode())
            proc.stdin.flush()
            response = safe_response(BoundedLineReader(proc.stdout.fileno()).read(deadline), attempt_id)
            row = {**response, "id": attempt_id, "pair": pair, "arm": arm, "status": "completed",
                   "processSpawnMs": round(process_spawn_ms, 3),
                   "processStartToResponseMs": round((time.monotonic() - process_started) * 1000, 3),
                   "requestReceiptToResponseMs": round((time.monotonic() - started) * 1000, 3),
                   "cachePlayerRecordsBefore": before_count, "cachePlayerBytesBefore": before_bytes,
                   "sessionEntryAbsentBefore": session_absent_before,
                   "formatMatched": response.get("formatId") == 251,
                   "codecMatched": response.get("codec") == track["codec"],
                   "metadataMatched": metadata_match(response, track)}
            row["transportEligible"] = bool(row.get("ok") is True and row.get("decodedAudioMs", 0) >= 5000 and
                                             row["formatMatched"] and row["codecMatched"] and row["metadataMatched"])
            rows.append(row); checkpoint()
            return row
        except TimeoutError:
            row = {"id": attempt_id, "pair": pair, "arm": arm, "status": "timeout"}
            rows.append(row); checkpoint(); return row
        except (EOFError, OSError, ProtocolError) as exc:
            row = {"id": attempt_id, "pair": pair, "arm": arm, "status": "protocol_error", "reason": str(exc)}
            rows.append(row); checkpoint(); return row
        finally:
            if proc is not None:
                try:
                    if proc.stdin: proc.stdin.close()
                except OSError:
                    pass
                terminate_group(proc)
                try:
                    if proc.stdout: proc.stdout.close()
                except OSError:
                    pass
            if arm == "shared_player_cache":
                removed_after = clear_session_cache(data_dir)
                count_after, bytes_after = player_cache_inventory(data_dir)
                # Attach lifecycle evidence to the just-recorded row, then
                # rewrite its checkpoint. Never emit filesystem names.
                if rows and rows[-1].get("id") == attempt_id:
                    rows[-1]["sessionEntryRemovedAfter"] = removed_after
                    rows[-1]["sessionEntryAbsentAfter"] = not session_cache_path(data_dir).exists()
                    rows[-1]["cachePlayerRecordsAfter"] = count_after
                    rows[-1]["cachePlayerBytesAfter"] = bytes_after
                    checkpoint()
            scratch.cleanup()

    try:
        # Populate the retained Player cache using a throwaway worker. Its
        # session cache key is deleted before any measured attempt.
        warmup = one_attempt("warmup", 0, "shared_player_cache", shared_data)
        warmup_artifact = rows[-1]
        if not warmup.get("transportEligible") or warmup_artifact.get("cachePlayerRecordsAfter", 0) < 1:
            for pair in range(1, pairs + 1):
                for arm in ("fresh_cache", "shared_player_cache"):
                    rows.append({"id": f"pair-{pair:02d}-{arm}", "pair": pair, "arm": arm,
                                 "status": "NOT_RUN", "reason": "cache_warmup_not_transport_eligible"})
                    checkpoint()
            return checkpoint("complete")

        for pair in range(1, pairs + 1):
            order = ("fresh_cache", "shared_player_cache") if pair % 2 else ("shared_player_cache", "fresh_cache")
            for arm in order:
                if time.monotonic() >= outer_deadline:
                    rows.append({"id": f"pair-{pair:02d}-{arm}", "pair": pair, "arm": arm,
                                 "status": "NOT_RUN", "reason": "outer_deadline"})
                    checkpoint(); continue
                one_attempt(f"pair", pair, arm, shared_data if arm == "shared_player_cache" else run_root / f"fresh-{pair}-{arm}")
        return checkpoint("complete")
    finally:
        run_scratch.cleanup()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, default=HERE / "fixtures/transport-music-tracks.json")
    parser.add_argument("--track", default="matthew", help="name from the explicit track manifest")
    parser.add_argument("--pairs", type=int, default=20, help="fresh/cache-retained pairs (1..20)")
    parser.add_argument("--total-budget-seconds", type=float, default=DEFAULT_OUTER_S)
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    track = next((row for row in manifest if row.get("name") == args.track), None)
    if track is None:
        parser.error(f"track {args.track!r} is absent from manifest")
    try:
        result = run(args.output, track, pairs=args.pairs, outer_s=args.total_budget_seconds)
    except ValueError as exc:
        parser.error(str(exc))
    print(json.dumps({"output": str(args.output), "attempts": len(result["attempts"]),
                      "status": result["status"]}))


if __name__ == "__main__":
    main()
