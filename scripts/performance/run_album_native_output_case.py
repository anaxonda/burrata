#!/usr/bin/env python3
"""Set per-process ALSA routing for the native album qualification example."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True, type=Path)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--pulse-server", required=True)
    parser.add_argument(
        "--user-volume",
        type=float,
        default=float(os.environ.get("BURRATA_QUAL_USER_VOLUME", "0.36")),
    )
    parser.add_argument("--route-timeout-seconds", type=float, default=15.0)
    args = parser.parse_args()

    profile = args.profile.resolve(strict=True)
    if profile.parent != Path("/tmp") or not profile.name.startswith("burrata-first-four-profile-"):
        raise RuntimeError("expected a disposable first-four fixture profile under /tmp")
    if (profile / ".burrata-qualification-profile").read_text().strip() != "synthetic fixture only":
        raise RuntimeError("profile is missing the synthetic-only marker")
    inventory = json.loads((profile / "qualification-inventory.json").read_text())
    if inventory.get("syntheticOnly") is not True or inventory.get("externalAudioUsed") is not False:
        raise RuntimeError("profile inventory is not synthetic-only")

    sink = os.environ.get("BURRATA_QUAL_SINK", "")
    module_id = os.environ.get("BURRATA_QUAL_MODULE_ID", "")
    case = os.environ.get("BURRATA_QUAL_CASE", "")
    if not re.fullmatch(r"burrata_qual_[A-Za-z0-9_]+", sink):
        raise RuntimeError("the output capture harness did not provide its private sink name")
    if case not in {"off", "normalized", "analyze-only"}:
        raise RuntimeError("expected an off, normalized, or analyze-only capture case")
    if not module_id.isdecimal():
        raise RuntimeError("the output capture harness did not provide its private sink module id")
    if not 0.0 <= args.user_volume <= 1.0:
        raise RuntimeError("user volume must be between zero and one")
    binary = args.binary.resolve(strict=True)
    repository = Path(__file__).resolve().parents[2]
    if not binary.is_relative_to((repository / ".cache").resolve()):
        raise RuntimeError("qualification binary must be inside repository .cache")
    config = profile / f"alsa-qualification-{case}-{os.getpid()}.conf"
    server = args.pulse_server
    if any(character in server for character in '"\r\n'):
        raise RuntimeError("unsafe Pulse server value")
    config.write_text(
        "pcm.!default {\n"
        "  type pulse\n"
        f'  server "{server}"\n'
        f'  device "{sink}"\n'
        "}\n"
        "ctl.!default {\n"
        "  type pulse\n"
        f'  server "{server}"\n'
        "}\n",
        encoding="utf-8",
    )
    config.chmod(stat.S_IRUSR | stat.S_IWUSR)

    env = os.environ.copy()
    env["ALSA_CONFIG_PATH"] = str(config)
    env["PULSE_SERVER"] = server
    env["PULSE_SINK"] = sink
    env["BURRATA_QUAL_SINK"] = sink
    env["BURRATA_QUAL_CASE"] = case
    env["BURRATA_QUAL_MODULE_ID"] = module_id
    env["BURRATA_QUAL_USER_VOLUME"] = str(args.user_volume)
    env["BURRATA_APP_DATA"] = str(profile / "data")
    candidate_hash = __import__("hashlib").sha256(binary.read_bytes()).hexdigest()
    child = subprocess.Popen([str(binary), str(profile)], env=env)
    guard_path = repository / "scripts/performance/verify_native_gain_routing.py"
    guard_command = [
        sys.executable,
        str(guard_path),
        "--pid", str(child.pid),
        "--candidate", str(binary),
        "--candidate-sha256", candidate_hash,
        "--profile", str(profile),
        "--sink", sink,
        "--module-id", module_id,
        "--pulse-server", args.pulse_server,
        "--alsa-config", str(config),
    ]
    deadline = time.monotonic() + args.route_timeout_seconds
    guard_result = None
    last_error = "candidate output stream has not appeared yet"
    try:
        while time.monotonic() < deadline and child.poll() is None:
            guard_result = subprocess.run(guard_command, env=env, text=True, capture_output=True, check=False)
            if guard_result.returncode == 0:
                break
            last_error = guard_result.stderr.strip() or last_error
            time.sleep(0.1)
        if child.poll() is not None:
            raise RuntimeError(f"album example exited before route verification (status {child.returncode})")
        if not guard_result or guard_result.returncode != 0:
            raise RuntimeError(f"route guard did not verify before timeout: {last_error}")
        print(json.dumps({
            "scope": "synthetic-only native album capture",
            "profile": str(profile),
            "binary": str(binary),
            "candidateSha256": candidate_hash,
            "case": case,
            "privateSink": sink,
            "moduleId": module_id,
            "alsaConfig": str(config),
            "routeGuard": json.loads(guard_result.stdout),
            "systemAlsaConfigChanged": False,
            "defaultPulseSinkChanged": False,
        }), file=sys.stderr, flush=True)
        # The example opens its output stream at volume zero and is paused.
        # Signal it only after exact PID/hash/sink verification succeeds.
        child.send_signal(signal.SIGUSR1)
        return child.wait(timeout=100)
    except BaseException:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=3)
        raise


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"album native output controller failed: {error}", file=sys.stderr)
        raise SystemExit(2)
