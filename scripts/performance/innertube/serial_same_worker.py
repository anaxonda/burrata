#!/usr/bin/env python3
"""Bounded, serial JSONL driver for the isolated Innertube reuse probe.

No request is queued before its predecessor has returned. Live execution is
intentionally opt-in by the caller; unit tests use local fixture processes.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import select
import signal
import subprocess
import tempfile
import time
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
NODE = ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/node"
WORKER = HERE / "worker.mjs"
MAX_LINE = 64 * 1024
PER_REQUEST_S = 20.0
OUTER_S = 240.0
SAFE_FIELDS = {
    "ok", "failure", "stage", "stages", "accountingComplete", "id",
    "clientName", "preparedMs", "extractionMs", "firstPcmFromCommandMs",
    "decodedAudioMs", "decodeWallMs", "formatId", "codec", "infoCacheHit",
    "refreshedAfterDenial", "tokenCacheHit", "tokenExpiresInMs",
    "tokenHelperCpuMs", "tokenHelperMaxRssBytes", "tokenBindings",
    "identityScope", "recordingVerified", "manifestExpiresInMs", "wallMs", "headerNames", "providerMetadata", "playbackPreparation",
}


def preflight() -> dict:
    """Check the real default local assets without starting the worker or network."""
    bgutil = ROOT / "src-tauri/resources/youtube-support/bgutil/server/build/session_manager.js"
    ffmpeg = ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/ffmpeg"
    try:
        version = subprocess.run([str(NODE), "--version"], cwd=ROOT, check=False,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                 timeout=3, text=True)
        node_exec = version.returncode == 0 and version.stdout.strip().startswith("v")
    except (OSError, subprocess.SubprocessError):
        node_exec = False
    checks = {"repoRoot": ROOT.is_dir() and (ROOT / ".git").exists(),
              "bundledNode": NODE.is_file() and os.access(NODE, os.X_OK),
              "bundledNodeExec": node_exec,
              "worker": WORKER.is_file(), "ffmpeg": ffmpeg.is_file() and os.access(ffmpeg, os.X_OK),
              "bgutil": bgutil.is_file(), "packageLock": (HERE / "package-lock.json").is_file()}
    return {"ok": all(checks.values()), "checks": checks}


class ProtocolError(Exception):
    pass


class BoundedLineReader:
    def __init__(self, fd: int):
        self.fd = fd
        self.buffer = bytearray()

    def read(self, deadline: float, *, max_bytes: int = MAX_LINE) -> bytes:
        """Read one bounded newline frame with a monotonic deadline, including partial writes."""
        while True:
            newline = self.buffer.find(b"\n")
            if newline >= 0:
                if newline > max_bytes:
                    raise ProtocolError("response frame too large")
                line = bytes(self.buffer[:newline])
                del self.buffer[:newline + 1]
                return line
            if len(self.buffer) > max_bytes:
                raise ProtocolError("response frame too large")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("response deadline")
            ready, _, _ = select.select([self.fd], [], [], remaining)
            if not ready:
                raise TimeoutError("response deadline")
            chunk = os.read(self.fd, min(4096, max_bytes + 1 - len(self.buffer)))
            if not chunk:
                raise EOFError("worker stdout closed")
            self.buffer.extend(chunk)


def safe_response(raw: bytes, expected_id: str) -> dict:
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProtocolError("invalid JSON response") from exc
    if not isinstance(value, dict) or value.get("id") != expected_id:
        raise ProtocolError("response identity mismatch")
    safe = {key: value[key] for key in SAFE_FIELDS if key in value}
    # Stage names and values are numeric timings; reject any unexpected shape.
    if "stages" in safe and (not isinstance(safe["stages"], dict) or
                             any(not isinstance(v, (int, float)) for v in safe["stages"].values())):
        safe.pop("stages")
    if "playbackPreparation" in safe:
        prep = safe["playbackPreparation"]
        if not isinstance(prep, dict) or prep.get("mode") not in {"sequential", "parallel-player"} or any(
                key not in {"mode", "sdkCreateMs", "sessionContextMs", "playerMs", "readyMs"} or
                (key != "mode" and not isinstance(value, (int, float)) and value is not None)
                for key, value in prep.items()):
            safe.pop("playbackPreparation")
    for key in ("codec", "failure", "stage", "clientName", "identityScope"):
        if key in safe and not isinstance(safe[key], str):
            safe.pop(key)
    if "formatId" in safe and not isinstance(safe["formatId"], int):
        safe.pop("formatId")
    if "headerNames" in safe and (not isinstance(safe["headerNames"], list) or
                                    not all(isinstance(item, str) for item in safe["headerNames"])):
        safe.pop("headerNames")
    if "providerMetadata" in safe:
        metadata = safe["providerMetadata"]
        if not isinstance(metadata, dict):
            safe.pop("providerMetadata")
        else:
            safe["providerMetadata"] = {
                key: value for key, value in metadata.items()
                if key in {"videoId", "title", "channel", "durationSeconds"} and
                ((key == "videoId" and isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9_-]{11}", value)) or
                 (key in {"title", "channel"} and (value is None or isinstance(value, str)) and len(value or "") <= 256) or
                 (key == "durationSeconds" and (value is None or isinstance(value, (int, float)))))}
    return safe


def terminate_group(proc: subprocess.Popen, grace: float = 0.3) -> None:
    """Stop worker and same-group descendants, even if the group leader exited."""
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    end = time.monotonic() + grace
    while time.monotonic() < end:
        try:
            os.killpg(proc.pid, 0)
        except ProcessLookupError:
            break
        time.sleep(0.02)
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        proc.wait(timeout=1)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=1)


def validate_tracks(tracks: list[dict]) -> None:
    if len(tracks) != 3:
        raise ValueError("exactly three explicit tracks are required")
    names, ids = set(), set()
    for row in tracks:
        if (not isinstance(row, dict) or not isinstance(row.get("name"), str) or
                not row["name"] or len(row["name"]) > 64 or not re.fullmatch(r"[A-Za-z0-9_-]{11}", row.get("videoId", "")) or
                row.get("formatId") != 251 or not isinstance(row.get("titleNeedle"), str) or
                not isinstance(row.get("channelNeedle"), str) or not isinstance(row.get("durationSeconds"), (int, float)) or
                not row["titleNeedle"] or not row["channelNeedle"] or not 0 < row["durationSeconds"] < 86_400 or
                row.get("codec") != 'audio/webm; codecs="opus"'):
            raise ValueError("invalid track manifest row")
        if row["name"] in names or row["videoId"] in ids:
            raise ValueError("track names and video IDs must be unique")
        names.add(row["name"]); ids.add(row["videoId"])


def run(output: Path, tracks: list[dict], *, node: Path = NODE, worker: Path = WORKER,
        total_s: float = OUTER_S, request_s: float = PER_REQUEST_S) -> dict:
    validate_tracks(tracks)
    outer_start = time.monotonic()
    outer_deadline = outer_start + total_s
    rows: list[dict] = []
    def checkpoint(status="running"):
        result = {"schema": "innertube-serial-format-pinned-v1", "status": status,
                  "scope": "anonymous MWEB; three explicit public source IDs; no catalog/acoustic equivalence implied",
                  "timingOrigin": "worker firstPcmFromCommandMs begins on JSONL receipt; requestReceiptToResponseMs begins before write; process launch excluded from worker field",
                  "formatId": 251, "perRequestBudgetMs": round(request_s * 1000),
                  "outerBudgetMs": round(total_s * 1000),
                  "totalWallMs": round((time.monotonic() - outer_start) * 1000, 3), "requests": rows}
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        return result

    def launch_worker():
        scratch = tempfile.TemporaryDirectory(prefix="burrata-innertube-serial-")
        scratch_path = Path(scratch.name)
        data = scratch_path / "data"
        data.mkdir(mode=0o700)
        env = os.environ.copy()
        env.update({
            "BURRATA_BENCH_DATA_DIR": str(data), "BURRATA_BENCH_CLIENT": "MWEB",
            "BURRATA_BENCH_TOKEN_MODE": "bgutil", "BURRATA_BENCH_IDLE_MS": "30000",
            "BURRATA_BENCH_FFMPEG": str(ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/ffmpeg"),
            "XDG_CONFIG_HOME": str(scratch_path / "config"), "XDG_CACHE_HOME": str(scratch_path / "cache"),
            "XDG_DATA_HOME": str(scratch_path / "state"),
        })
        for key in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
            Path(env[key]).mkdir(mode=0o700)
        env["YOUTUBEI_JS_DISABLE_SESSION_CACHE"] = "1"
        try:
            proc = subprocess.Popen([str(node), str(worker)], cwd=ROOT, env=env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, start_new_session=True, bufsize=0)
        except (OSError, subprocess.SubprocessError):
            scratch.cleanup()
            raise
        assert proc.stdin is not None and proc.stdout is not None
        return scratch, proc, BoundedLineReader(proc.stdout.fileno())

    for triplet_index, track in enumerate(tracks):
        if time.monotonic() >= outer_deadline:
            break
        cross_track = tracks[(triplet_index + 1) % len(tracks)]
        plan = [(track, "cold"), (track, "same_id_warm"), (cross_track, "different_id_warm")]
        request_prefix = f"triplet-{triplet_index + 1}-{track['name']}"
        try:
            scratch, proc, reader = launch_worker()
        except (OSError, subprocess.SubprocessError) as exc:
            for ordinal, (source, role) in enumerate(plan, start=1):
                rows.append({"id": f"{request_prefix}-{ordinal}", "triplet": triplet_index + 1,
                             "track": source["name"], "role": role, "status": "startup_failed",
                             "failure": "PROCESS_LAUNCH_FAILED", "errorType": type(exc).__name__})
                checkpoint()
            continue
        try:
            for ordinal, (source, role) in enumerate(plan, start=1):
                request_id = f"{request_prefix}-{ordinal}"
                if time.monotonic() >= outer_deadline:
                    rows.append({"id": request_id, "triplet": triplet_index + 1,
                                 "track": source["name"], "role": role, "status": "NOT_RUN",
                                 "reason": "outer_deadline"})
                    checkpoint(); continue
                started = time.monotonic()
                deadline = min(started + request_s, outer_deadline)
                request = {"id": request_id, "action": "play", "videoId": source["videoId"],
                           "expectedVideoId": source["videoId"], "formatId": source["formatId"],
                           "seconds": 5, "budgetMs": max(1, int((deadline - started) * 1000))}
                try:
                    proc.stdin.write((json.dumps(request, separators=(",", ":")) + "\n").encode())
                    proc.stdin.flush()
                    row = safe_response(reader.read(deadline), request_id)
                    observed = row.get("providerMetadata", {})
                    normalize = lambda value: " ".join(unicodedata.normalize("NFKC", value).casefold().split()) if isinstance(value, str) else ""
                    metadata_matched = (
                        observed.get("videoId") == source["videoId"] and
                        normalize(source["titleNeedle"]) in normalize(observed.get("title")) and
                        normalize(source["channelNeedle"]) in normalize(observed.get("channel")) and
                        isinstance(observed.get("durationSeconds"), (int, float)) and
                        abs(observed["durationSeconds"] - source["durationSeconds"]) <= 1
                    )
                    row.update({"triplet": triplet_index + 1, "track": source["name"], "videoId": source["videoId"],
                                "role": role, "status": "completed",
                                "requestReceiptToResponseMs": round((time.monotonic() - started) * 1000, 3),
                                "expectedFormatId": source["formatId"],
                                "formatMatched": row.get("formatId") == source["formatId"],
                                "expectedCodec": source["codec"], "codecMatched": row.get("codec") == source["codec"],
                                "expectedTitleNeedle": source["titleNeedle"], "expectedChannelNeedle": source["channelNeedle"],
                                "expectedDurationSeconds": source["durationSeconds"], "metadataMatched": metadata_matched})
                    row["transportEligible"] = bool(row.get("ok") is True and row.get("decodedAudioMs", 0) >= 5000 and
                                                     row["formatMatched"] and row["codecMatched"] and metadata_matched)
                    rows.append(row); checkpoint()
                except TimeoutError:
                    rows.append({"id": request_id, "triplet": triplet_index + 1, "track": source["name"],
                                 "videoId": source["videoId"], "role": role, "status": "timeout"})
                    checkpoint()
                    # No further requests enter this worker after a timeout.
                    for rest_ordinal, (rest_source, rest_role) in enumerate(plan[ordinal:], start=ordinal + 1):
                        rows.append({"id": f"{request_prefix}-{rest_ordinal}", "triplet": triplet_index + 1,
                                     "track": rest_source["name"], "videoId": rest_source["videoId"],
                                     "role": rest_role, "status": "NOT_RUN", "reason": "prior_timeout"})
                        checkpoint()
                    break
                except (EOFError, OSError, ProtocolError) as exc:
                    rows.append({"id": request_id, "triplet": triplet_index + 1, "track": source["name"],
                                 "videoId": source["videoId"], "role": role, "status": "protocol_error",
                                 "reason": str(exc)})
                    checkpoint()
                    for rest_ordinal, (rest_source, rest_role) in enumerate(plan[ordinal:], start=ordinal + 1):
                        rows.append({"id": f"{request_prefix}-{rest_ordinal}", "triplet": triplet_index + 1,
                                     "track": rest_source["name"], "videoId": rest_source["videoId"],
                                     "role": rest_role, "status": "NOT_RUN", "reason": "prior_protocol_error"})
                        checkpoint()
                    break
        finally:
            try: proc.stdin.close()
            except OSError: pass
            terminate_group(proc)
            proc.stdout.close()
            scratch.cleanup()
    return checkpoint("complete")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--preflight", action="store_true", help="check default local assets only; no worker or network")
    parser.add_argument("--manifest", type=Path, help="JSON array of exactly three unique {name,videoId,formatId:251} rows")
    args = parser.parse_args()
    if args.preflight:
        result = preflight()
        print(json.dumps(result))
        raise SystemExit(0 if result["ok"] else 2)
    if not args.manifest:
        parser.error("--manifest is required for live transport experiments")
    tracks = json.loads(args.manifest.read_text(encoding="utf-8"))
    result = run(args.output, tracks)
    print(json.dumps({"output": str(args.output), "requestCount": len(result["requests"]),
                      "statuses": [row["status"] for row in result["requests"]]}))


if __name__ == "__main__":
    main()
