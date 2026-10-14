#!/usr/bin/env python3
"""Install a native Burrata installer and smoke-test its packaged runtime."""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time


TARGETS = {
    "linux": "x86_64-unknown-linux-gnu",
    "darwin": "aarch64-apple-darwin",
    "win32": "x86_64-pc-windows-msvc",
}
ROOT = pathlib.Path(__file__).resolve().parents[2]


class SmokeFailure(RuntimeError):
    def __init__(self, stage: str, reason: str):
        super().__init__(reason)
        self.stage = stage


def canonical_temp_root(path: pathlib.Path) -> pathlib.Path:
    """Return a canonical temp root before launching Tauri from it."""
    return path.resolve(strict=True)


def run(args: list[str | os.PathLike[str]], *, timeout: int = 60,
        env: dict[str, str] | None = None, input_bytes: bytes | None = None,
        check: bool = True) -> subprocess.CompletedProcess:
    command = [os.fspath(arg) for arg in args]
    try:
        result = subprocess.run(command, input=input_bytes, capture_output=True,
                                timeout=timeout, env=env, check=False)
    except subprocess.TimeoutExpired as error:
        raise SmokeFailure("command", f"{pathlib.Path(command[0]).name} exceeded {timeout}s") from error
    except OSError as error:
        raise SmokeFailure("command", f"could not launch {pathlib.Path(command[0]).name}: {error}") from error
    if check and result.returncode != 0:
        raise SmokeFailure("command", f"{pathlib.Path(command[0]).name} exited with {result.returncode}")
    return result


def platform_environment(home: pathlib.Path, app_data: pathlib.Path) -> dict[str, str]:
    env = dict(os.environ)
    env["TMPDIR"] = str(home / "tmp")
    env["TEMP"] = str(home / "tmp")
    env["TMP"] = str(home / "tmp")
    env["BURRATA_APP_DATA"] = str(app_data)
    if sys.platform == "linux":
        env["HOME"] = str(home)
        env["XDG_DATA_HOME"] = str(app_data.parent)
        env["XDG_CONFIG_HOME"] = str(home / ".config")
        env["XDG_CACHE_HOME"] = str(home / ".cache")
        env["PATH"] = "/usr/bin:/bin"
    elif sys.platform == "darwin":
        env["HOME"] = str(home)
        env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
    else:
        # Keep Windows' real profile and Known Folders intact so WebView2 and
        # Tauri exercise the same environment as an ordinary installation.
        # Burrata app data is isolated through its dedicated override; only
        # WebView2's browser data receives a temporary private location.
        env["WEBVIEW2_USER_DATA_FOLDER"] = str(home / "webview2")
        system_root = env.get("SystemRoot", r"C:\Windows")
        env["PATH"] = os.pathsep.join([str(pathlib.Path(system_root) / "System32"), system_root])
    return env


def install_linux(installer: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, str]:
    if installer.suffix.lower() != ".deb":
        raise SmokeFailure("install", "Linux smoke requires the built DEB installer")
    package = run(["dpkg-deb", "-f", installer, "Package"]).stdout.decode().strip()
    if not package:
        raise SmokeFailure("install", "DEB did not declare a package name")
    run(["sudo", "-n", "apt-get", "install", "-y", str(installer)], timeout=600)
    files = run(["dpkg-query", "-L", package]).stdout.decode(errors="replace").splitlines()
    app = next((pathlib.Path(p) for p in files
                if pathlib.Path(p).name.lower() == "burrata" and pathlib.Path(p).is_file()
                and os.access(p, os.X_OK)), None)
    mcp_script = next((pathlib.Path(p) for p in files
                       if p.replace("\\", "/").endswith("/mcp/server.cjs") and pathlib.Path(p).is_file()), None)
    if app is None or mcp_script is None:
        raise SmokeFailure("install", "installed DEB is missing its app executable or MCP bridge")
    return app, mcp_script.parent.parent, package


def install_macos(installer: pathlib.Path, temp: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, str]:
    if installer.suffix.lower() != ".dmg":
        raise SmokeFailure("install", "macOS smoke requires the built DMG installer")
    mount = temp / "dmg"
    mount.mkdir()
    run(["hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, installer], timeout=120)
    app_bundle = next(mount.glob("*.app"), None)
    if app_bundle is None:
        raise SmokeFailure("install", "DMG did not contain an application bundle")
    installed = temp / "installed" / app_bundle.name
    installed.parent.mkdir(parents=True)
    run(["ditto", app_bundle, installed], timeout=120)
    app = installed / "Contents" / "MacOS" / "burrata"
    resource_base = installed / "Contents" / "Resources"
    mcp_script = next(resource_base.rglob("server.cjs"), None)
    mcp_script = mcp_script if mcp_script and mcp_script.parent.name == "mcp" else None
    if not app.is_file() or not mcp_script:
        raise SmokeFailure("install", "copied DMG app is missing its executable or MCP bridge")
    return app, mcp_script.parent.parent, str(installed)


def install_windows(installer: pathlib.Path, temp: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, str]:
    if installer.suffix.lower() != ".exe":
        raise SmokeFailure("install", "Windows smoke requires the built NSIS installer")
    installed = temp / "installed"
    installed.mkdir()
    run([installer, "/S", f"/D={installed}"], timeout=300)
    app = next((path for path in installed.rglob("*.exe")
                if path.name.lower() == "burrata.exe"), None)
    mcp_script = next((path for path in installed.rglob("server.cjs")
                       if path.parent.name == "mcp"), None)
    if app is None or mcp_script is None:
        raise SmokeFailure("install", "silent NSIS install is missing its app executable or MCP bridge")
    return app, mcp_script.parent.parent, str(installed)


def bundled_tool_checks(resource_root: pathlib.Path, target: str,
                        env: dict[str, str]) -> dict[str, dict[str, str]]:
    tools = resource_root / "tools" / target
    ext = ".exe" if sys.platform == "win32" else ""
    checks: dict[str, dict[str, str]] = {}
    probes = {
        "node": (["--version"], "node" + ext),
        "yt-dlp": (["--version"], "yt-dlp" + ext),
        "ffmpeg": (["-version"], "ffmpeg" + ext),
        "ffprobe": (["-version"], "ffprobe" + ext),
        "fpcalc": (["-version"], "fpcalc" + ext),
    }
    for name, (args, binary) in probes.items():
        path = tools / binary
        if not path.is_file():
            raise SmokeFailure("resources", f"packaged {name} is missing")
        result = run([path, *args], timeout=30, env=env)
        output = (result.stdout + result.stderr).decode(errors="replace").splitlines()
        if not output or not output[0].strip():
            raise SmokeFailure("tools", f"packaged {name} returned no version output")
        checks[name] = {"path": str(path.relative_to(resource_root)), "version": output[0].strip()[:160]}

    recognizer = tools / ("burrata-recognize" + ext)
    if not recognizer.is_file():
        raise SmokeFailure("resources", "packaged recognizer is missing")
    result = run([recognizer], timeout=10, env=env, check=False)
    if result.returncode != 2 or b"usage:" not in result.stderr.lower():
        raise SmokeFailure("tools", "packaged recognizer did not start and report its usage")
    checks["recognizer"] = {"path": str(recognizer.relative_to(resource_root)), "started": "true"}

    host = tools / ("burrata-browser-host" + ext)
    if not host.is_file():
        raise SmokeFailure("resources", "packaged native messaging host is missing")
    checks["browserHost"] = {"path": str(host.relative_to(resource_root)), "protocol": "present"}
    return checks


def native_host_check(resource_root: pathlib.Path, target: str,
                      env: dict[str, str]) -> dict[str, str]:
    ext = ".exe" if sys.platform == "win32" else ""
    host = resource_root / "tools" / target / ("burrata-browser-host" + ext)
    request = json.dumps({"type": "heartbeat"}, separators=(",", ":")).encode()
    frame = struct.pack("<I", len(request)) + request
    response = run([host], timeout=10, env=env, input_bytes=frame)
    if len(response.stdout) < 5:
        raise SmokeFailure("native-host", "packaged browser host returned no framed response")
    length = struct.unpack("<I", response.stdout[:4])[0]
    try:
        payload = json.loads(response.stdout[4:4 + length])
    except (ValueError, json.JSONDecodeError) as error:
        raise SmokeFailure("native-host", "packaged browser host returned invalid JSON") from error
    if length != len(response.stdout) - 4 or not isinstance(payload, dict) or payload.get("ok") is not True:
        raise SmokeFailure("native-host", "packaged browser host heartbeat was not accepted by the app")
    return {"path": str(host.relative_to(resource_root)), "heartbeat": "accepted"}


def launch_app(app: pathlib.Path, env: dict[str, str], temp: pathlib.Path) -> subprocess.Popen:
    if sys.platform == "linux":
        pulse = shutil.which("pulseaudio")
        dbus = shutil.which("dbus-run-session")
        xvfb = shutil.which("xvfb-run")
        if not pulse or not dbus or not xvfb:
            raise SmokeFailure("launch", "Linux smoke requires pulseaudio, dbus-run-session, and xvfb-run")
        run([pulse, "--start", "--exit-idle-time=-1"], timeout=20, env={**os.environ, **env})
        command = [dbus, "--", xvfb, "-a", "--server-args=-screen 0 1280x800x24", app]
    else:
        command = [app]
    stdout_log = (temp / "installed-app.stdout.log").open("wb")
    stderr_log = (temp / "installed-app.stderr.log").open("wb")
    kwargs = {"env": env, "stdin": subprocess.DEVNULL, "stdout": stdout_log,
              "stderr": stderr_log}
    if os.name == "nt":
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kwargs["start_new_session"] = True
    try:
        return subprocess.Popen([os.fspath(item) for item in command], **kwargs)
    except OSError as error:
        raise SmokeFailure("launch", f"could not start installed GUI executable: {error}") from error
    finally:
        stdout_log.close()
        stderr_log.close()


def wait_for_app(app_process: subprocess.Popen, discovery: pathlib.Path,
                 stderr_log: pathlib.Path) -> None:
    deadline = time.monotonic() + 75
    while time.monotonic() < deadline:
        if app_process.poll() is not None:
            detail = ""
            try:
                tail = stderr_log.read_text(encoding="utf-8", errors="replace").strip()[-2400:]
                if tail:
                    detail = f"; stderr: {tail}"
            except OSError:
                pass
            raise SmokeFailure("launch", f"installed GUI exited before publishing automation discovery (exit {app_process.returncode}){detail}")
        try:
            value = json.loads(discovery.read_text(encoding="utf-8"))
            if isinstance(value.get("url"), str) and isinstance(value.get("token"), str) and len(value["token"]) >= 32:
                return
        except (OSError, ValueError):
            pass
        time.sleep(0.5)
    raise SmokeFailure("launch", "installed GUI did not publish authenticated automation discovery within 75s")


def run_mcp_status(resource_root: pathlib.Path, target: str, discovery: pathlib.Path,
                   env: dict[str, str]) -> dict:
    ext = ".exe" if sys.platform == "win32" else ""
    mcp = resource_root / "tools" / target / ("burrata-mcp" + ext)
    if not mcp.is_file():
        raise SmokeFailure("mcp", "packaged console MCP launcher is missing")
    node = shutil.which("node")
    if not node:
        raise SmokeFailure("mcp", "test runner Node.js is unavailable")
    result = run([node, ROOT / "scripts/release/check-installed-mcp.mjs", mcp, discovery],
                 timeout=30, env=env)
    try:
        return json.loads(result.stdout.decode().strip().splitlines()[-1])
    except (ValueError, IndexError) as error:
        raise SmokeFailure("mcp", "installed MCP client returned invalid evidence") from error


def stop_process(process: subprocess.Popen | None, temp: pathlib.Path) -> None:
    if process is None:
        return
    if os.name == "nt":
        # WebView2 is a child process. Kill the process tree even if the app
        # already exited during startup. Then find only WebView2 processes
        # whose command line includes this run's unique temporary directory;
        # the app may have exited before taskkill can walk its child tree.
        run(["taskkill", "/PID", str(process.pid), "/T", "/F"], timeout=10, check=False)
        cleanup_script = r"""
$needle = $env:BURRATA_SMOKE_TEMP_NAME
Get-CimInstance Win32_Process | Where-Object {
  $_.Name -ieq 'msedgewebview2.exe' -and $_.CommandLine -and
  $_.CommandLine.IndexOf($needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
"""
        run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", cleanup_script],
            timeout=20,
            env={**os.environ, "BURRATA_SMOKE_TEMP_NAME": temp.name}, check=False)
        if process.poll() is None:
            process.terminate()
    else:
        if process.poll() is not None:
            return
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            process.kill()
        else:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--installer", type=pathlib.Path, required=True)
    parser.add_argument("--evidence", type=pathlib.Path, required=True)
    parser.add_argument("--source-run-id")
    parser.add_argument("--source-commit")
    args = parser.parse_args()
    installer = args.installer.resolve()
    evidence_path = args.evidence.resolve()
    evidence: dict = {"schemaVersion": 1, "platform": sys.platform,
                      "target": TARGETS.get(sys.platform, "unknown"),
                      "installer": installer.name, "status": "failed", "checks": {},
                      "osSecurityPrompts": "not tested"}
    if args.source_run_id:
        evidence["sourceRunId"] = args.source_run_id
    if args.source_commit:
        evidence["sourceCommit"] = args.source_commit
    app_process = None
    package_name = None
    mount = None
    installed_path = None
    temp_obj = tempfile.TemporaryDirectory(prefix="burrata-install-smoke-", ignore_cleanup_errors=True)
    temp = canonical_temp_root(pathlib.Path(temp_obj.name))
    mount = temp / "dmg" if sys.platform == "darwin" else None
    home = temp / "home"
    if sys.platform == "linux":
        app_data = temp / "appdata" / "org.burrata.discovery"
    elif sys.platform == "darwin":
        app_data = home / "Library" / "Application Support" / "org.burrata.discovery"
    else:
        app_data = temp / "appdata" / "org.burrata.discovery"
    home.mkdir()
    app_data.mkdir(parents=True)
    (home / "tmp").mkdir()
    if sys.platform == "win32":
        (home / "webview2").mkdir()
    env = platform_environment(home, app_data)
    try:
        if sys.platform == "linux":
            app, resource_root, package_name = install_linux(installer)
            installed_path = str(app)
        elif sys.platform == "darwin":
            app, resource_root, installed_path = install_macos(installer, temp)
        elif sys.platform == "win32":
            app, resource_root, installed_path = install_windows(installer, temp)
        else:
            raise SmokeFailure("install", f"unsupported smoke platform: {sys.platform}")
        evidence["checks"]["installer"] = "installed"
        evidence["installedLocation"] = pathlib.Path(installed_path).name if sys.platform != "linux" else installed_path

        target = TARGETS[sys.platform]
        evidence["checks"]["packagedTools"] = bundled_tool_checks(resource_root, target, env)
        app_process = launch_app(app, env, temp)
        discovery = app_data / "automation.json"
        wait_for_app(app_process, discovery, temp / "installed-app.stderr.log")
        evidence["checks"]["guiStartup"] = "automation discovery published"
        evidence["checks"]["nativeHost"] = native_host_check(resource_root, target, env)
        evidence["checks"]["mcp"] = run_mcp_status(resource_root, target, discovery, env)
        evidence["status"] = "passed"
        print(json.dumps({"status": evidence["status"], "platform": sys.platform,
                          "checks": evidence["checks"]}, separators=(",", ":")))
        return 0
    except SmokeFailure as error:
        evidence["failedStage"] = error.stage
        evidence["error"] = str(error)
        print(f"Installed smoke failed at {error.stage}: {error}", file=sys.stderr)
        return 1
    finally:
        stop_process(app_process, temp)
        if sys.platform == "linux" and package_name:
            run(["sudo", "-n", "apt-get", "remove", "-y", package_name], timeout=300, check=False)
        if sys.platform == "darwin" and mount:
            run(["hdiutil", "detach", "-quiet", mount], timeout=30, check=False)
        if sys.platform == "win32" and installed_path:
            install_dir = pathlib.Path(installed_path)
            uninstaller = next(install_dir.glob("*uninstall*.exe"), None) if install_dir.is_dir() else None
            if uninstaller:
                run([uninstaller, "/S"], timeout=120, check=False)
        if sys.platform == "linux":
            pulse = shutil.which("pulseaudio")
            if pulse:
                run([pulse, "--kill"], timeout=10, check=False)
        evidence_path.parent.mkdir(parents=True, exist_ok=True)
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
        temp_obj.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
