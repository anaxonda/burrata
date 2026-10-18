#!/usr/bin/env python3
"""Run an isolated Linux Burrata playback/memory workload after the current build is ready."""
import argparse
import hashlib
import json
import math
import os
import platform
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import uuid
import wave
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / "scripts/dev/launch-current.sh"
WORKLOAD = ROOT / "scripts/performance/isolated_memory_workload.mjs"
LOCAL_QUALIFICATION = ROOT / "scripts/performance/local_playback_qualification.mjs"
SCHEMA = ROOT / "src-tauri/src/schema.sql"
ROWS = 20_000
FIXTURE_SECONDS = 360
QUEUE_COPIES = 6


def make_wav(path: Path) -> tuple[str, int]:
    """Create six minutes of low-amplitude 22050 Hz mono PCM without large RAM use."""
    sample_rate = 22_050
    frames = sample_rate * FIXTURE_SECONDS
    cycle = b"".join(struct.pack("<h", int(1600 * math.sin(2 * math.pi * i / 50))) for i in range(50))
    block_cycles = 8_820  # 20 seconds of 440 Hz samples.
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        cycles = frames // 50
        for start in range(0, cycles, block_cycles):
            count = min(block_cycles, cycles - start)
            output.writeframesraw(cycle * count)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    return digest, path.stat().st_size


def track_record(track_id: str, provider: str, provider_id: str, title: str,
                 artist: str, duration: float | None, local_path: str | None = None,
                 asset_id: str | None = None, codec: str | None = None) -> dict:
    source_url = f"local:{provider_id}" if provider == "local" else ""
    return {
        "id": track_id, "provider": provider, "providerId": provider_id,
        "sourceUrl": source_url, "artist": artist, "title": title,
        "version": "Synthetic fixture" if provider == "local" else "Original Mix",
        "album": "Isolated memory test", "albumArtist": None, "recordingDate": None,
        "trackNumber": None, "trackTotal": None, "discNumber": None, "discTotal": None,
        "remixers": [], "label": None, "genre": None, "normalizedGenre": None,
        "durationSeconds": duration, "bpm": None, "key": None, "releaseDate": None,
        "artworkUrl": None, "isrc": None, "catalogNumber": None, "rank": None,
        "previewUrl": None, "favorite": False, "disposition": None,
        "localPath": local_path, "localAvailability": "available" if local_path else "none",
        "assetId": asset_id, "provenance": ({"codec": codec, "synthetic": True} if codec else {"synthetic": True}),
    }


def seed_profile(profile: Path) -> tuple[str, str]:
    data = profile / "data"
    for directory in (data, profile / "config", profile / "cache", profile / "state", profile / "tmp"):
        directory.mkdir(parents=True, exist_ok=True)
    (profile / ".burrata-isolated-profile").write_text("burrata isolated memory run\n", encoding="utf-8")
    fixture_dir = profile / "fixture"
    fixture_dir.mkdir()
    # The application normally scans ~/Music. Keep the integration workload
    # away from the operator's real library without repurposing HOME.
    (fixture_dir / "Music").mkdir()
    fixture = fixture_dir / "synthetic-tone.wav"
    digest, file_size = make_wav(fixture)
    local_id = f"local:{digest}"
    asset_id = str(uuid.uuid4())
    local_track = track_record(local_id, "local", digest, "Synthetic Playback Fixture",
                               "Burrata Test", FIXTURE_SECONDS, str(fixture), asset_id, "wav")
    db_path = data / "org.burrata.discovery" / "burrata.sqlite"
    db_path.parent.mkdir(parents=True)
    conn = sqlite3.connect(db_path)
    conn.executescript(SCHEMA.read_text(encoding="utf-8"))
    now = datetime.now(timezone.utc).isoformat()
    try:
        with conn:
            for index in range(ROWS):
                provider_id = f"synthetic-{index:05d}"
                track_id = f"fixture:{provider_id}"
                title = f"Synthetic Library Track {index:05d}"
                track = track_record(track_id, "fixture", provider_id, title,
                                     f"Synthetic Artist {index % 500:03d}", 180.0)
                work_id = f"work:{track_id}"
                metadata = json.dumps(track, separators=(",", ":"))
                conn.execute("INSERT INTO works(id,title,artist) VALUES(?,?,?)",
                             (work_id, title, track["artist"]))
                conn.execute("INSERT INTO recordings(id,work_id,version) VALUES(?,?,?)",
                             (track_id, work_id, track["version"]))
                conn.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)",
                             (track_id, "fixture", provider_id, track_id, "", metadata, now))
            local_metadata = json.dumps(local_track, separators=(",", ":"))
            local_work = f"work:{local_id}"
            conn.execute("INSERT INTO works(id,title,artist) VALUES(?,?,?)",
                         (local_work, local_track["title"], local_track["artist"]))
            conn.execute("INSERT INTO recordings(id,work_id,version) VALUES(?,?,?)",
                         (local_id, local_work, local_track["version"]))
            conn.execute("INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)",
                         (local_id, "local", digest, local_id, local_track["sourceUrl"], local_metadata, now))
            asset = {
                "id": asset_id, "recordingId": local_id, "path": str(fixture), "hash": digest,
                "sourceCodec": "wav", "finalCodec": "wav", "sourceBitrate": None,
                "finalBitrate": None, "sampleRate": 22050, "channels": 1, "bitDepth": 16,
                "durationSeconds": FIXTURE_SECONDS, "sizeBytes": file_size,
                "bitrateOrigin": "format", "processing": "existing-local-file", "track": local_track,
            }
            conn.execute("INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?,?,?,?,?)",
                         (asset_id, local_id, str(fixture), digest,
                          json.dumps(asset, separators=(",", ":"))))
        integrity = conn.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"seeded SQLite integrity check failed: {integrity}")
    finally:
        conn.close()
    return str(local_id), digest


def wait_for_app(launcher: subprocess.Popen, profile: Path, log_path: Path, timeout: float = 45) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if launcher.poll() is not None:
            raise RuntimeError(f"launcher exited with status {launcher.returncode}; see {log_path}")
        if log_path.exists() and "MCP confirms the app is running with its frontend mounted." in log_path.read_text(errors="replace"):
            return
        time.sleep(0.25)
    raise TimeoutError(f"isolated app did not complete MCP handshake within {timeout}s; see {log_path}")


def find_app_pid(expected: Path, timeout: float = 10) -> int:
    expected = expected.resolve()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                running = (entry / "exe").resolve()
                if running == expected:
                    found.append(int(entry.name))
            except OSError:
                continue
        if len(found) == 1:
            return found[0]
        if len(found) > 1:
            raise RuntimeError("more than one process matches the requested Burrata binary")
        time.sleep(0.2)
    raise TimeoutError("could not find the isolated Burrata executable in /proc")


def launcher_descendants(root_pid: int) -> set[int]:
    parent = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            raw = (entry / "stat").read_text()
            fields = raw[raw.rfind(")") + 2:].split()
            parent[int(entry.name)] = int(fields[1])
        except (OSError, ValueError, IndexError):
            continue
    selected = {root_pid}
    while True:
        expanded = selected | {pid for pid, ppid in parent.items() if ppid in selected}
        if expanded == selected:
            return expanded - {root_pid}
        selected = expanded


def find_vite_pid(pid_file: Path) -> tuple[int, str]:
    try:
        fields = pid_file.read_text(encoding="ascii").split()
        pid, uid, start_ticks = map(int, fields[:3])
        owner = fields[3]
        if len(fields) != 4 or owner not in {"owned", "shared"}:
            raise ValueError("invalid owner field")
    except (OSError, ValueError) as error:
        raise RuntimeError(f"launcher did not leave a verified Vite process identity in {pid_file}") from error
    proc = Path(f"/proc/{pid}")
    try:
        stat_text = (proc / "stat").read_text(encoding="ascii")
        stat_fields = stat_text[stat_text.rfind(")") + 2:].split()
        live_start_ticks = int(stat_fields[19])
        live_uid = int(next(line.split()[1] for line in (proc / "status").read_text(encoding="ascii").splitlines()
                            if line.startswith("Uid:")))
    except (OSError, ValueError, IndexError, StopIteration) as error:
        raise RuntimeError("launcher-recorded Vite process is no longer accessible") from error
    if pid <= 1 or uid != os.getuid() or live_uid != uid or live_start_ticks != start_ticks:
        raise RuntimeError("launcher-recorded Vite process identity changed before sampling")
    return pid, owner


def summarize_samples(path: Path, output: Path, vite_owner: str) -> dict:
    rows = json.loads(path.read_text(encoding="utf-8"))
    app_groups = [row for row in rows if row.get("kind") == "group" and row.get("group") == "app"]
    vite_groups = [row for row in rows if row.get("kind") == "group" and row.get("group") == "vite"]
    if not app_groups or (vite_owner != "not-used" and not vite_groups):
        raise RuntimeError("profiler output contains no app process-group samples")
    if vite_owner == "not-used" and vite_groups:
        raise RuntimeError("embedded frontend unexpectedly has Vite process-group samples")

    def p95(values):
        ordered = sorted(values)
        return ordered[max(0, math.ceil(0.95 * len(ordered)) - 1)]

    def group_stats(groups):
        cpu = [row["cpuPercent"] for row in groups]
        rss = [row["rssBytes"] for row in groups]
        return {
            "intervalSamples": len(groups),
            "cpuPercentOneLogicalCore": {
                "median": round(float(__import__("statistics").median(cpu)), 3),
                "p95": round(p95(cpu), 3), "worst": max(cpu),
                "cpuSeconds": round(sum(row["cpuSeconds"] for row in groups), 3),
            },
            "processGroupRssBytes": {
                "median": int(__import__("statistics").median(rss)),
                "p95": int(p95(rss)), "peak": max(rss),
            },
            "storageIoBytes": {
                "read": sum(row["readBytes"] for row in groups),
                "write": sum(row["writeBytes"] for row in groups),
            },
        }

    result = {
        "applicationProcessTree": group_stats(app_groups),
        "viteServerProcessTree": ({**group_stats(vite_groups), "ownership": vite_owner,
            "attribution": ("launched for this isolated app" if vite_owner == "owned" else "shared pre-existing server; activity cannot be attributed only to this app")}
            if vite_owner != "not-used" else None),
        "limitations": (("No Vite server is used or sampled for the embedded frontend. " if vite_owner == "not-used" else "Application group excludes the separately sampled Vite server. When shared, Vite CPU/RSS/I/O can include unrelated clients and activity and is not attributable only to this app. ") + "Each process-group RSS sums resident pages and can double-count shared memory; storage IO is not network traffic; short-lived children between polls may be missed."),
    }
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "src-tauri/target/debug/burrata")
    parser.add_argument("--output", type=Path, required=True, help="new directory for trace and metrics artifacts")
    parser.add_argument("--duration-seconds", type=int, default=1800)
    parser.add_argument("--interval-seconds", type=float, default=1.0)
    parser.add_argument("--settle-seconds", type=int, default=120)
    parser.add_argument("--qualify-local", action="store_true",
                        help="run the real-MCP 3-warmup/20-start/60-seek local playback qualification before the memory workload")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("this process profiler is Linux-only")
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"compiled executable not found: {binary}")
    if args.duration_seconds < 60 or not 0.05 <= args.interval_seconds <= 5 or args.settle_seconds < 0:
        parser.error("duration must be at least 60s; interval must be 0.05–5s; settle cannot be negative")
    output = args.output.resolve()
    if output.exists():
        parser.error(f"output directory already exists: {output}")
    output.mkdir(parents=True)
    profile = Path(tempfile.mkdtemp(prefix="burrata-launch.", dir="/tmp"))
    fixture_id = None
    digest = None
    launcher = None
    profiler = None
    workload = None
    operation_id = str(uuid.uuid4())
    run_started_at = datetime.now(timezone.utc).isoformat()
    log_path = output / "launcher.log"
    try:
        fixture_id, digest = seed_profile(profile)
        with log_path.open("w", encoding="utf-8") as log:
            environment = os.environ.copy()
            environment["BURRATA_APP_BINARY"] = str(binary)
            environment["BURRATA_PLAYBACK_ENGINE"] = "native"
            environment["TMPDIR"] = "/tmp"
            environment["BURRATA_LAUNCH_LOG_DIR"] = str(output / "logs")
            environment["BURRATA_LAUNCH_VITE_PID_FILE"] = str(output / "vite.pid")
            environment["BURRATA_ISOLATED_MUSIC_ROOT"] = str(profile / "fixture" / "Music")
            environment.pop("BURRATA_APP_DATA", None)
            launcher = subprocess.Popen(
                ["bash", str(LAUNCHER), "--isolated-profile", str(profile)],
                cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            wait_for_app(launcher, profile, log_path)
            app_pid = find_app_pid(binary)
            embedded_frontend = "release" in binary.parts
            vite_pid, vite_owner = (None, "not-used") if embedded_frontend else find_vite_pid(output / "vite.pid")
            discovery = profile / "data/org.burrata.discovery/automation.json"
            qualification = None
            if args.qualify_local:
                qualification_file = output / "local-playback-qualification.json"
                qualification = subprocess.run(
                    ["node", str(LOCAL_QUALIFICATION), str(discovery), fixture_id, str(qualification_file)],
                    cwd=ROOT, env=environment, capture_output=True, text=True, timeout=900,
                )
                if qualification.returncode:
                    raise RuntimeError(f"local playback qualification failed: {qualification.stderr.strip() or qualification.stdout.strip()}")
                qualification = json.loads(qualification_file.read_text(encoding="utf-8"))
            prepare = subprocess.run(
                ["node", str(WORKLOAD), "prepare", str(discovery), fixture_id],
                cwd=ROOT, env=environment, capture_output=True, text=True, timeout=90,
            )
            if prepare.returncode:
                raise RuntimeError(f"MCP playback setup failed: {prepare.stderr.strip() or prepare.stdout.strip()}")
            time.sleep(args.settle_seconds)
            profiler_command = [sys.executable, str(ROOT / "scripts/performance/profile_process.py"),
                 "--group", f"app={app_pid}"]
            if vite_pid is not None:
                profiler_command.extend(["--group", f"vite={vite_pid}"])
            profiler_command.extend([
                 "--operation-id", operation_id,
                 "--seconds", str(args.duration_seconds), "--interval", str(args.interval_seconds),
                 "--output", str(output / "profile")])
            profiler = subprocess.Popen(
                profiler_command,
                cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            measurement_started_at = datetime.now(timezone.utc).isoformat()
            workload = subprocess.Popen(
                ["node", str(WORKLOAD), "measure", str(discovery), str(fixture_id),
                 str(args.duration_seconds), str(operation_id), str(output / "workload.ndjson")],
                cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            workload_out, workload_err = workload.communicate(timeout=args.duration_seconds + 180)
            profiler_out, profiler_err = profiler.communicate(timeout=60)
            if workload.returncode:
                raise RuntimeError(f"MCP workload failed: {workload_err.strip() or workload_out.strip()}")
            if profiler.returncode:
                raise RuntimeError(f"process profiler failed: {profiler_err.strip() or profiler_out.strip()}")
            measurement_finished_at = datetime.now(timezone.utc).isoformat()
            summary = summarize_samples(output / "profile/samples.json", output, vite_owner)
            metadata = {
                "runStartedAtUtc": run_started_at,
                "measurementStartedAtUtc": measurement_started_at,
                "measurementFinishedAtUtc": measurement_finished_at,
                "platform": platform.platform(), "python": platform.python_version(),
                "binary": str(binary), "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "frontendMode": "embedded" if embedded_frontend else "vite",
                "workload": "isolated synthetic local WAV playback, volume 0, six queued copies of one six-minute fixture; 20,000 seeded metadata-only fixture listings; MCP library search pages during playback",
                "fixture": {"id": fixture_id, "sha256": digest, "durationSeconds": FIXTURE_SECONDS},
                "libraryMetadataRows": ROWS, "measurementSeconds": args.duration_seconds,
                "intervalSeconds": args.interval_seconds, "settleSecondsBeforeSampling": args.settle_seconds,
                "operationId": operation_id,
                "appProcessId": app_pid,
                "viteProcessId": vite_pid,
                "viteProcessOwnership": vite_owner,
                "mcpPrepare": json.loads(prepare.stdout) if prepare.stdout.strip().startswith("{") else prepare.stdout.strip(),
                "mcpMeasurement": json.loads(workload_out) if workload_out.strip().startswith("{") else workload_out.strip(),
                "localPlaybackQualification": ({"file": qualification_file.name,
                    "failureCount": qualification["failureCount"], "warmups": len(qualification["warmups"]),
                    "starts": len(qualification["starts"]), "seeks": len(qualification["seeks"])}
                    if qualification is not None else None),
                "measurement": summary,
                "limitations": [
                    "Synthetic local audio and synthetic metadata only; no provider login or network playback was used.",
                    "This measures the isolated app and observed descendants on this Linux host, not cross-platform performance.",
                    "Profiler RSS aggregates process RSS and may double-count shared pages; storage IO is not network traffic.",
                    ("The frontend is embedded in the release executable; no Vite process is used or sampled." if embedded_frontend else
                     "Vite is sampled as a separate sibling process group; its CPU/RSS/I/O are excluded from application totals." if vite_owner == "owned" else "Vite is a shared pre-existing server sampled separately; its CPU/RSS/I/O may include unrelated work and cannot be attributed only to this app."),
                    f"The {args.settle_seconds}-second playback settle time is excluded from the profiler window; build/compilation is not run by this script.",
                ],
            }
            (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
            print(json.dumps({"output": str(output), "measurementSeconds": args.duration_seconds,
                              "appCpuSeconds": summary["applicationProcessTree"]["cpuPercentOneLogicalCore"]["cpuSeconds"],
                              "appPeakRssBytes": summary["applicationProcessTree"]["processGroupRssBytes"]["peak"],
                              "vitePeakRssBytes": (summary["viteServerProcessTree"]["processGroupRssBytes"]["peak"] if vite_pid is not None else None),
                              "viteOwnership": vite_owner}, indent=2))
    except Exception as error:
        (output / "failure.txt").write_text(f"{type(error).__name__}: {error}\n", encoding="utf-8")
        print(f"Memory run failed; details: {output / 'failure.txt'}", file=sys.stderr)
        return 1
    finally:
        for process in (workload, profiler):
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        if launcher is not None and launcher.poll() is None:
            try:
                os.killpg(launcher.pid, signal.SIGTERM)
                launcher.wait(timeout=20)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                try:
                    os.killpg(launcher.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                launcher.wait()
        if profile.exists():
            import shutil
            shutil.rmtree(profile)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
