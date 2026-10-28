"""Verify one ALSA process reaches an explicitly named private Pulse sink."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import wave


def pactl_json(server: str, *args: str):
    env = os.environ.copy()
    env["PULSE_SERVER"] = server
    result = subprocess.run(["pactl", "--format=json", *args], env=env, text=True, capture_output=True, check=True)
    return json.loads(result.stdout)


def matching_input(pid: int, sink_index: int, server: str):
    for item in pactl_json(server, "list", "sink-inputs"):
        properties = item.get("properties", {})
        if properties.get("application.process.id") == str(pid) and item.get("sink") == sink_index:
            return {
                "index": item.get("index"),
                "sink": item.get("sink"),
                "processId": pid,
                "applicationName": properties.get("application.name"),
                "mediaName": properties.get("media.name"),
                "sampleSpecification": item.get("sample_specification"),
            }
    return None


def one_second_fixture(source: Path, output: Path) -> None:
    with wave.open(str(source), "rb") as reader:
        channels = reader.getnchannels()
        width = reader.getsampwidth()
        rate = reader.getframerate()
        if channels != 2 or width != 2 or rate < 8_000:
            raise RuntimeError("qualification fixture must be stereo signed-16 PCM")
        frames = reader.readframes(rate)
    with wave.open(str(output), "wb") as writer:
        writer.setnchannels(channels)
        writer.setsampwidth(width)
        writer.setframerate(rate)
        writer.writeframes(frames)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--sink", required=True)
    parser.add_argument("--module-id", required=True)
    parser.add_argument("--pulse-server", required=True)
    args = parser.parse_args()
    profile = args.profile.resolve(strict=True)
    if profile.parent != Path("/tmp") or not profile.name.startswith("burrata-first-four-profile-"):
        raise RuntimeError("expected a dedicated first-four synthetic profile under /tmp")
    if (profile / ".burrata-qualification-profile").read_text().strip() != "synthetic fixture only":
        raise RuntimeError("profile is missing the synthetic-only marker")
    inventory = json.loads((profile / "qualification-inventory.json").read_text())
    if inventory.get("syntheticOnly") is not True or inventory.get("externalAudioUsed") is not False:
        raise RuntimeError("profile is not synthetic-only")

    module_rows = subprocess.run(
        ["pactl", "list", "short", "modules"],
        env={**os.environ, "PULSE_SERVER": args.pulse_server},
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    if not any(
        len(columns := line.split("\t")) >= 3
        and columns[0] == args.module_id
        and columns[1] == "module-null-sink"
        and f"sink_name={args.sink}" in columns[2]
        for line in module_rows.splitlines()
    ):
        raise RuntimeError("the specified private null-sink module is not active")
    sinks = subprocess.run(
        ["pactl", "list", "short", "sinks"],
        env={**os.environ, "PULSE_SERVER": args.pulse_server},
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    sink_indexes = [
        int(columns[0]) for line in sinks.splitlines()
        if len(columns := line.split("\t")) > 1 and columns[1] == args.sink
    ]
    if len(sink_indexes) != 1:
        raise RuntimeError("the requested private sink is missing or ambiguous")

    config = profile / "alsa-private-pulse.conf"
    config.write_text(
        "pcm.!default {\n"
        "  type pulse\n"
        f'  server "{args.pulse_server}"\n'
        f'  device "{args.sink}"\n'
        "}\n"
        "ctl.!default {\n"
        "  type pulse\n"
        f'  server "{args.pulse_server}"\n'
        "}\n",
        encoding="utf-8",
    )
    config.chmod(0o600)
    temp_root = profile / "tmp"
    temp_root.mkdir(mode=0o700, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="burrata-alsa-route-", dir=temp_root) as temp:
        fixture = Path(temp) / "route-smoke.wav"
        one_second_fixture(args.fixture.resolve(strict=True), fixture)
        env = os.environ.copy()
        env["ALSA_CONFIG_PATH"] = str(config)
        env["PULSE_SERVER"] = args.pulse_server
        env["PULSE_SINK"] = args.sink
        player = subprocess.Popen(
            ["aplay", "-q", "-D", "default", str(fixture)],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        route = None
        deadline = time.monotonic() + 5
        try:
            while time.monotonic() < deadline and player.poll() is None:
                route = matching_input(player.pid, sink_indexes[0], args.pulse_server)
                if route:
                    break
                time.sleep(0.05)
            if not route:
                stderr = player.stderr.read() if player.poll() is not None and player.stderr else ""
                raise RuntimeError(f"ALSA process did not appear on the owned sink: {stderr.strip()}")
            result = player.wait(timeout=5)
            if result != 0:
                raise RuntimeError(f"synthetic aplay exited with {result}")
        finally:
            if player.poll() is None:
                player.terminate()
                try:
                    player.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    player.kill()
                    player.wait(timeout=2)
    print(json.dumps({
        "status": "verified",
        "scope": "generated one-second fixture through per-process ALSA_CONFIG_PATH to private PulseAudio null sink",
        "profile": str(profile),
        "alsaConfig": str(config),
        "pulseServer": args.pulse_server,
        "pulseSink": args.sink,
        "sinkIndex": sink_indexes[0],
        "moduleId": args.module_id,
        "sinkInput": route,
        "systemAlsaConfigChanged": False,
        "defaultPulseSinkChanged": False,
        "candidatePlaybackQualified": False,
    }, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"ALSA/Pulse route smoke failed: {error}", file=sys.stderr)
        raise SystemExit(2)
