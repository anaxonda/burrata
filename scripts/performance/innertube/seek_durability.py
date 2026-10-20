#!/usr/bin/env python3
"""Bounded mid-file seek and opt-in full-source PCM durability probes."""
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

from serial_same_worker import BoundedLineReader, HERE, NODE, ROOT, WORKER, ProtocolError, safe_response, terminate_group

POSITIONS = (("early", 0.01), ("middle", 0.50), ("late", 0.95))
REQUEST_S = 20.0
DEFAULT_OUTER_S = 600.0


def validate_tracks(tracks: list[dict]) -> None:
    if len(tracks) != 3:
        raise ValueError("exactly three explicit public tracks are required")
    ids = set()
    for track in tracks:
        if (not isinstance(track, dict) or not isinstance(track.get("name"), str) or
                not re.fullmatch(r"[A-Za-z0-9_-]{11}", track.get("videoId", "")) or
                track.get("videoId") in ids or track.get("formatId") != 251 or
                not isinstance(track.get("titleNeedle"), str) or not track["titleNeedle"] or
                not isinstance(track.get("channelNeedle"), str) or not track["channelNeedle"] or
                not isinstance(track.get("durationSeconds"), (int, float)) or
                not 5 <= track["durationSeconds"] <= 600 or track.get("codec") != 'audio/webm; codecs="opus"'):
            raise ValueError("manifest requires unique explicit tracks with pinned itag251 Opus and duration")
        ids.add(track["videoId"])


def metadata_matches(response: dict, track: dict) -> bool:
    observed = response.get("providerMetadata", {})
    normalize = lambda value: " ".join(unicodedata.normalize("NFKC", value).casefold().split()) if isinstance(value, str) else ""
    return (observed.get("videoId") == track["videoId"] and
            normalize(track["titleNeedle"]) in normalize(observed.get("title")) and
            normalize(track["channelNeedle"]) in normalize(observed.get("channel")) and
            isinstance(observed.get("durationSeconds"), (int, float)) and
            abs(observed["durationSeconds"] - track["durationSeconds"]) <= 1)


def run(output: Path, tracks: list[dict], *, node: Path = NODE, worker: Path = WORKER,
        repetitions: int = 3, full_recording: bool = False, outer_s: float = DEFAULT_OUTER_S,
        request_s: float = REQUEST_S) -> dict:
    validate_tracks(tracks)
    if isinstance(repetitions, bool) or not isinstance(repetitions, int) or not 1 <= repetitions <= 20:
        raise ValueError("repetitions must be an integer from 1 through 20")
    if isinstance(outer_s, bool) or not isinstance(outer_s, (int, float)) or not 1 <= outer_s <= 4200:
        raise ValueError("outer budget must be between 1 and 4200 seconds")
    if isinstance(request_s, bool) or not isinstance(request_s, (int, float)) or not 0 < request_s <= REQUEST_S:
        raise ValueError("per-request budget must be greater than 0 and at most 20 seconds")

    outer_start = time.monotonic()
    outer_deadline = outer_start + outer_s
    rows: list[dict] = []
    package = json.loads((HERE / "node_modules/youtubei.js/package.json").read_text(encoding="utf-8"))

    def checkpoint(status="running"):
        result = {"schema": "innertube-seek-durability-v1", "status": status,
                  "scope": "anonymous fresh worker per track/repetition; explicit IDs; pinned itag251 Opus; no PCM persistence",
                  "mode": "full-source-decode" if full_recording else "seek-5s-pcm",
                  "timingOrigin": "per-request deadline starts before worker spawn on the first request; firstPcmFromCommandMs begins at JSONL receipt",
                  "youtubeiJsVersion": package.get("version"), "requestBudgetMs": round(request_s * 1000),
                  "outerBudgetMs": round(outer_s * 1000), "requestedRepetitions": repetitions,
                  "driverSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  "totalWallMs": round((time.monotonic() - outer_start) * 1000, 3), "attempts": rows}
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        return result

    def launch(track: dict, repetition: int):
        scratch = tempfile.TemporaryDirectory(prefix="burrata-seek-durability-")
        root = Path(scratch.name)
        data = root / "data"
        data.mkdir(mode=0o700)
        env = os.environ.copy()
        env.update({"BURRATA_BENCH_DATA_DIR": str(data), "BURRATA_BENCH_CLIENT": "MWEB",
                    "BURRATA_BENCH_TOKEN_MODE": "bgutil", "BURRATA_BENCH_IDLE_MS": "30000",
                    "BURRATA_BENCH_FFMPEG": str(ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/ffmpeg"),
                    "BURRATA_BENCH_PARALLEL_PLAYER": "0",
                    "XDG_CONFIG_HOME": str(root / "config"), "XDG_CACHE_HOME": str(root / "xdg-cache"),
                    "XDG_DATA_HOME": str(root / "state")})
        for key in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
            Path(env[key]).mkdir(mode=0o700)
        if full_recording:
            env["BURRATA_BENCH_FULL_RECORDING"] = "1"
        else:
            env.pop("BURRATA_BENCH_FULL_RECORDING", None)
        try:
            proc = subprocess.Popen([str(node), str(worker)], cwd=ROOT, env=env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, start_new_session=True, bufsize=0)
        except (OSError, subprocess.SubprocessError):
            scratch.cleanup()
            raise
        assert proc.stdin is not None and proc.stdout is not None
        return scratch, proc, BoundedLineReader(proc.stdout.fileno())

    for repetition in range(1, repetitions + 1):
        for offset in range(len(tracks)):
            track = tracks[(repetition - 1 + offset) % len(tracks)]
            if full_recording:
                position_plan = [("full", 0.0, float(track["durationSeconds"]))]
            else:
                position_plan = [(label, round(track["durationSeconds"] * fraction, 3), 5.0)
                                  for label, fraction in POSITIONS]
            if time.monotonic() >= outer_deadline:
                for label, position, _seconds in position_plan:
                    rows.append({"id": f"rep-{repetition}-{track['name']}-{label}", "track": track["name"],
                                 "videoId": track["videoId"], "repetition": repetition,
                                 "positionLabel": label, "positionSeconds": position,
                                 "status": "NOT_RUN", "reason": "outer_deadline"})
                    checkpoint()
                continue
            first_request_deadline = min(time.monotonic() + request_s, outer_deadline)
            try:
                scratch, proc, reader = launch(track, repetition)
            except (OSError, subprocess.SubprocessError) as exc:
                for label, position, seconds in position_plan:
                    row = {"id": f"rep-{repetition}-{track['name']}-{label}", "track": track["name"],
                           "videoId": track["videoId"], "repetition": repetition, "positionLabel": label,
                           "positionSeconds": position, "status": "startup_failed",
                           "failure": "PROCESS_LAUNCH_FAILED", "errorType": type(exc).__name__}
                    rows.append(row); checkpoint()
                continue
            try:
                aborted_worker = False
                for index, (label, position, seconds) in enumerate(position_plan):
                    request_id = f"rep-{repetition}-{track['name']}-{label}"
                    started = time.monotonic()
                    deadline = first_request_deadline if index == 0 else min(started + request_s, outer_deadline)
                    if started >= deadline:
                        row = {"id": request_id, "track": track["name"], "videoId": track["videoId"],
                               "repetition": repetition, "positionLabel": label, "positionSeconds": position,
                               "status": "NOT_RUN", "reason": "request_deadline_before_send"}
                        rows.append(row); checkpoint(); aborted_worker = True; continue
                    request = {"id": request_id, "action": "play" if full_recording else "seek",
                               "videoId": track["videoId"], "expectedVideoId": track["videoId"],
                               "formatId": 251, "position": position, "seconds": seconds,
                               "budgetMs": max(1, int((deadline - started) * 1000))}
                    if full_recording:
                        request["fullRecording"] = True
                    try:
                        proc.stdin.write((json.dumps(request, separators=(",", ":")) + "\n").encode())
                        proc.stdin.flush()
                        response = safe_response(reader.read(deadline), request_id)
                        expected_ms = track["durationSeconds"] * 1000
                        pcm_ms = response.get("decodedAudioMs", 0)
                        row = {**response, "id": request_id, "track": track["name"], "videoId": track["videoId"],
                               "repetition": repetition, "positionLabel": label, "positionSeconds": position,
                               "expectedDurationSeconds": track["durationSeconds"], "status": "completed",
                               "requestReceiptToResponseMs": round((time.monotonic() - started) * 1000, 3),
                               "metadataMatched": metadata_matches(response, track),
                               "formatMatched": response.get("formatId") == 251,
                               "codecMatched": response.get("codec") == track["codec"]}
                        if full_recording:
                            row["fullPcmDurationMatched"] = isinstance(pcm_ms, (int, float)) and abs(pcm_ms - expected_ms) <= 1000
                        row["durabilityEligible"] = bool(response.get("ok") is True and
                            (pcm_ms >= 5000 if not full_recording else row.get("fullPcmDurationMatched")) and
                            row["metadataMatched"] and row["formatMatched"] and row["codecMatched"])
                        rows.append(row); checkpoint()
                    except TimeoutError:
                        rows.append({"id": request_id, "track": track["name"], "videoId": track["videoId"],
                                     "repetition": repetition, "positionLabel": label, "positionSeconds": position,
                                     "status": "timeout"})
                        checkpoint(); aborted_worker = True
                        for rest_label, rest_position, _rest_seconds in position_plan[index + 1:]:
                            rows.append({"id": f"rep-{repetition}-{track['name']}-{rest_label}",
                                         "track": track["name"], "videoId": track["videoId"],
                                         "repetition": repetition, "positionLabel": rest_label,
                                         "positionSeconds": rest_position, "status": "NOT_RUN",
                                         "reason": "prior_timeout"})
                            checkpoint()
                        break
                    except (EOFError, OSError, ProtocolError) as exc:
                        rows.append({"id": request_id, "track": track["name"], "videoId": track["videoId"],
                                     "repetition": repetition, "positionLabel": label,
                                     "positionSeconds": position, "status": "protocol_error", "reason": str(exc)})
                        checkpoint(); aborted_worker = True
                        for rest_label, rest_position, _rest_seconds in position_plan[index + 1:]:
                            rows.append({"id": f"rep-{repetition}-{track['name']}-{rest_label}",
                                         "track": track["name"], "videoId": track["videoId"],
                                         "repetition": repetition, "positionLabel": rest_label,
                                         "positionSeconds": rest_position, "status": "NOT_RUN",
                                         "reason": "prior_protocol_error"})
                            checkpoint()
                        break
                    if aborted_worker:
                        break
            finally:
                try:
                    if proc.stdin: proc.stdin.close()
                except OSError:
                    pass
                terminate_group(proc)
                try:
                    if proc.stdout: proc.stdout.close()
                except OSError:
                    pass
                scratch.cleanup()
    return checkpoint("complete")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, default=HERE / "fixtures/transport-music-tracks.json")
    parser.add_argument("--repetitions", type=int, default=None,
                        help="1..20; default 3 seek rounds or 1 full-recording pass")
    parser.add_argument("--full-recording", action="store_true",
                        help="opt-in decode up to manifest duration (<=600s); still uses a 20s wall deadline")
    parser.add_argument("--total-budget-seconds", type=float, default=DEFAULT_OUTER_S)
    args = parser.parse_args()
    tracks = json.loads(args.manifest.read_text(encoding="utf-8"))
    repetitions = args.repetitions if args.repetitions is not None else (1 if args.full_recording else 3)
    try:
        result = run(args.output, tracks, repetitions=repetitions, full_recording=args.full_recording,
                     outer_s=args.total_budget_seconds)
    except ValueError as exc:
        parser.error(str(exc))
    print(json.dumps({"output": str(args.output), "attempts": len(result["attempts"]),
                      "status": result["status"], "mode": result["mode"]}))


if __name__ == "__main__":
    main()
