"""Start/stop the isolated Xvfb :23 + extracted Openbox qualification display."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import time


ROOT = Path("/tmp/burrata-private-xvfb")
XVFB = ROOT / "root/usr/bin/Xvfb"
OPENBOX = ROOT / "root/usr/bin/openbox"
LIBDIR = ROOT / "root/usr/lib"
STATE = ROOT / "gain-display-23.json"
LOG = ROOT / "gain-display-23.log"
DISPLAY = ":23"
LOCK = Path("/tmp/.X23-lock")
SOCKET = Path("/tmp/.X11-unix/X23")


def proc_cmdline(pid: int) -> list[str] | None:
    try:
        return [part.decode("utf-8", errors="replace") for part in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if part]
    except OSError:
        return None


def proc_env(pid: int) -> dict[str, str] | None:
    try:
        values = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        return {
            key.decode("utf-8", errors="replace"): value.decode("utf-8", errors="replace")
            for item in values if item and b"=" in item
            for key, value in [item.split(b"=", 1)]
        }
    except OSError:
        return None


def owned(state: dict, key: str, expected: Path | str) -> bool:
    pid = state.get(key)
    if not isinstance(pid, int) or pid <= 1:
        return False
    cmd = proc_cmdline(pid)
    env = proc_env(pid)
    return bool(cmd and cmd[0] == str(expected) and env and env.get("DISPLAY") == DISPLAY)


def start() -> dict:
    if not XVFB.is_file() or not OPENBOX.is_file():
        raise RuntimeError("extracted Xvfb/Openbox binaries are missing")
    if STATE.exists():
        current = json.loads(STATE.read_text(encoding="utf-8"))
        if owned(current, "xvfbPid", XVFB) and owned(current, "openboxPid", OPENBOX):
            return current
        raise RuntimeError("display state exists but is not a live owned :23 pair")
    if LOCK.exists() or SOCKET.exists():
        raise RuntimeError(":23 lock/socket already exists; refusing to take the display")
    env = os.environ.copy()
    env["DISPLAY"] = DISPLAY
    env["GDK_BACKEND"] = "x11"
    env["HOME"] = str(ROOT / "gain-openbox-home")
    env["XDG_CONFIG_HOME"] = str(ROOT / "gain-openbox-config")
    env["XDG_CACHE_HOME"] = str(ROOT / "gain-openbox-cache")
    env["XDG_DATA_DIRS"] = f"{ROOT / 'root/usr/share'}:/usr/local/share:/usr/share"
    env["LD_LIBRARY_PATH"] = f"{LIBDIR}:{env.get('LD_LIBRARY_PATH', '')}"
    for key in ("WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
        env.pop(key, None)
    for path in (Path(env["HOME"]), Path(env["XDG_CONFIG_HOME"]), Path(env["XDG_CACHE_HOME"])):
        path.mkdir(parents=True, exist_ok=True)
        path.chmod(0o700)
    private_openbox_config = Path(env["XDG_CONFIG_HOME"]) / "openbox"
    private_openbox_config.mkdir(mode=0o700, parents=True, exist_ok=True)
    for filename in ("rc.xml", "menu.xml", "environment"):
        source_config = ROOT / "root/etc/xdg/openbox" / filename
        if source_config.is_file():
            shutil.copyfile(source_config, private_openbox_config / filename)
    with LOG.open("ab") as log:
        xvfb = subprocess.Popen(
            [str(XVFB), DISPLAY, "-screen", "0", "1440x1000x24", "-nolisten", "tcp", "-noreset"],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if xvfb.poll() is not None:
                raise RuntimeError(f"Xvfb exited during startup ({xvfb.returncode})")
            if LOCK.exists() and SOCKET.exists():
                try:
                    lock_pid = int(LOCK.read_text(encoding="ascii").strip())
                except (OSError, ValueError):
                    lock_pid = -1
                if lock_pid == xvfb.pid:
                    break
            time.sleep(0.05)
        else:
            xvfb.terminate()
            xvfb.wait(timeout=3)
            raise RuntimeError("private Xvfb :23 readiness timed out")
        openbox = subprocess.Popen(
            [str(OPENBOX)],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    time.sleep(0.3)
    if openbox.poll() is not None:
        xvfb.terminate()
        xvfb.wait(timeout=3)
        raise RuntimeError(f"Openbox exited during startup ({openbox.returncode})")
    state = {
        "display": DISPLAY,
        "xvfbPid": xvfb.pid,
        "openboxPid": openbox.pid,
        "xvfbExecutable": str(XVFB),
        "openboxExecutable": str(OPENBOX),
        "privateDisplayOnly": True,
    }
    STATE.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    STATE.chmod(0o600)
    return state


def stop() -> dict:
    if not STATE.exists():
        return {"display": DISPLAY, "stopped": False, "reason": "no owned display state"}
    state = json.loads(STATE.read_text(encoding="utf-8"))
    if not owned(state, "xvfbPid", XVFB) or not owned(state, "openboxPid", OPENBOX):
        raise RuntimeError("refusing to stop :23 because the recorded processes are not both owned")
    for key in ("openboxPid", "xvfbPid"):
        os.kill(state[key], signal.SIGTERM)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if proc_cmdline(state[key]) is None:
                break
            time.sleep(0.05)
    if proc_cmdline(state["xvfbPid"]) is not None:
        raise RuntimeError("owned Xvfb did not exit; leaving state record for safe retry")
    if proc_cmdline(state["openboxPid"]) is not None:
        raise RuntimeError("owned Openbox did not exit; leaving state record for safe retry")
    STATE.unlink()
    return {"display": DISPLAY, "stopped": True}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("start", "status", "stop"))
    args = parser.parse_args()
    if args.action == "start":
        result = start()
    elif args.action == "stop":
        result = stop()
    elif STATE.exists():
        result = json.loads(STATE.read_text(encoding="utf-8"))
        result["xvfbOwned"] = owned(result, "xvfbPid", XVFB)
        result["openboxOwned"] = owned(result, "openboxPid", OPENBOX)
    else:
        result = {"display": DISPLAY, "free": not LOCK.exists() and not SOCKET.exists()}
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"private Xvfb management failed: {error}", file=sys.stderr)
        raise SystemExit(2)
