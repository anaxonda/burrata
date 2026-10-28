"""Fail-closed check that a private Burrata candidate has a stream on its owned sink."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


REPO = Path(__file__).resolve().parents[2]
QUALIFICATION_ROOT = Path("/tmp")


def command(args: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(args, env=env, text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(args)}: {result.stderr.strip()}")
    return result.stdout


def process_environment(pid: int) -> dict[str, str]:
    data = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
    return {
        key.decode(): value.decode(errors="replace")
        for item in data if b"=" in item
        for key, value in [item.split(b"=", 1)]
    }


def process_executable(pid: int) -> Path:
    return Path(os.readlink(f"/proc/{pid}/exe")).resolve(strict=True)


def process_parent(pid: int) -> int | None:
    try:
        # comm can contain spaces or closing parentheses; split after its last ')'.
        tail = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return int(tail[1])  # state is field 3; ppid is field 4.
    except (OSError, ValueError, IndexError):
        return None


def descendants(root_pid: int) -> set[int]:
    tree = {root_pid}
    changed = True
    while changed:
        changed = False
        for path in Path("/proc").iterdir():
            if not path.name.isdecimal():
                continue
            pid = int(path.name)
            if pid not in tree and process_parent(pid) in tree:
                tree.add(pid)
                changed = True
    return tree


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", required=True, type=int)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--candidate-sha256", required=True)
    parser.add_argument("--profile", required=True, type=Path)
    parser.add_argument("--sink", required=True)
    parser.add_argument("--module-id", required=True)
    parser.add_argument("--pulse-server", required=True)
    parser.add_argument("--alsa-config", required=True, type=Path)
    args = parser.parse_args()

    candidate = args.candidate.resolve(strict=True)
    profile = args.profile.resolve(strict=True)
    if not candidate.is_relative_to((REPO / ".cache").resolve()):
        raise RuntimeError("candidate must be inside the repository .cache directory")
    if file_sha256(candidate) != args.candidate_sha256.lower():
        raise RuntimeError("candidate hash does not match the frozen candidate identity")
    if profile.parent != QUALIFICATION_ROOT or not profile.name.startswith("burrata-first-four-profile-"):
        raise RuntimeError("profile must be a dedicated first-four qualification directory under /tmp")
    if (profile / ".burrata-qualification-profile").read_text().strip() != "synthetic fixture only":
        raise RuntimeError("profile is missing the synthetic-only marker")
    inventory = json.loads((profile / "qualification-inventory.json").read_text())
    if inventory.get("syntheticOnly") is not True or inventory.get("externalAudioUsed") is not False:
        raise RuntimeError("profile inventory is not synthetic-only")
    alsa_config = args.alsa_config.resolve(strict=True)
    if alsa_config.parent != profile:
        raise RuntimeError("ALSA_CONFIG_PATH must be inside the synthetic profile")
    config_text = alsa_config.read_text(encoding="utf-8")
    if args.sink not in config_text or args.pulse_server not in config_text:
        raise RuntimeError("private ALSA config does not target the owned Pulse sink/server")

    if process_executable(args.pid) != candidate:
        raise RuntimeError("PID does not run the exact frozen candidate executable")
    process_env = process_environment(args.pid)
    if (process_env.get("PULSE_SINK") != args.sink
        or process_env.get("PULSE_SERVER") != args.pulse_server
        or process_env.get("ALSA_CONFIG_PATH") != str(alsa_config)):
        raise RuntimeError("candidate environment does not match the owned sink/server")
    if process_env.get("BURRATA_APP_DATA") != str(profile / "data"):
        raise RuntimeError("candidate is not using the synthetic profile data directory")

    pactl_env = os.environ.copy()
    pactl_env["PULSE_SERVER"] = args.pulse_server
    modules = command(["pactl", "list", "short", "modules"], pactl_env)
    owned_modules = [
        columns for line in modules.splitlines()
        if len(columns := line.split("\t")) >= 3
        and columns[0] == args.module_id
        and columns[1] == "module-null-sink"
        and f"sink_name={args.sink}" in columns[2]
    ]
    if len(owned_modules) != 1:
        raise RuntimeError("the exact owned null-sink module is not active")

    sinks = command(["pactl", "list", "short", "sinks"], pactl_env)
    sink_indexes = [line.split("\t")[0] for line in sinks.splitlines() if len(line.split("\t")) > 1 and line.split("\t")[1] == args.sink]
    if len(sink_indexes) != 1:
        raise RuntimeError("the exact test sink is missing or ambiguous")
    sink_index = int(sink_indexes[0])

    candidate_processes = descendants(args.pid)
    inputs = json.loads(command(["pactl", "--format=json", "list", "sink-inputs"], pactl_env))
    matching_inputs = []
    for item in inputs:
        properties = item.get("properties", {})
        try:
            stream_pid = int(properties.get("application.process.id", ""))
        except (TypeError, ValueError):
            continue
        if stream_pid in candidate_processes and item.get("sink") == sink_index:
            matching_inputs.append({
                "index": item.get("index"),
                "sink": sink_index,
                "processId": stream_pid,
                "applicationName": properties.get("application.name"),
                "mediaName": properties.get("media.name"),
                "sampleSpecification": item.get("sample_specification"),
                "corked": item.get("corked"),
            })
    if not matching_inputs:
        raise RuntimeError("no candidate descendant audio stream is routed to the owned null sink")
    print(json.dumps({
        "status": "verified",
        "candidatePid": args.pid,
        "candidateSha256": args.candidate_sha256.lower(),
        "profile": str(profile),
        "pulseSink": args.sink,
        "sinkIndex": sink_index,
        "moduleId": args.module_id,
        "alsaConfig": str(alsa_config),
        "matchingSinkInputs": matching_inputs,
        "defaultSinkWasNotChanged": True,
        "safeToRaisePrivateFixtureVolume": True,
    }, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"candidate routing verification failed: {error}", file=sys.stderr)
        raise SystemExit(2)
