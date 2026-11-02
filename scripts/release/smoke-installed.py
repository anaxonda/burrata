#!/usr/bin/env python3
"""Install a native Burrata installer and smoke-test its packaged runtime."""
from __future__ import annotations

import argparse
from contextlib import closing
import hashlib
import json
import os
import pathlib
import shutil
import signal
import math
import plistlib
import re
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import uuid
import stat
import platform


TARGETS = {
    "linux": "x86_64-unknown-linux-gnu",
    "darwin": "aarch64-apple-darwin",
    "win32": "x86_64-pc-windows-msvc",
}
ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "src-tauri" / "src" / "schema.sql"
# Budget 30s for explicit frontend readiness, three awaited playback controls
# (45s each), the 10s clock-start deadline, final sample and IPC/process slack.
NATIVE_AUDIO_CHECK_TIMEOUT_SECONDS = 220


class SmokeFailure(RuntimeError):
    def __init__(self, stage: str, reason: str):
        super().__init__(reason)
        self.stage = stage
        self.diagnostics: dict | None = None


def canonical_temp_root(path: pathlib.Path) -> pathlib.Path:
    """Return a canonical temp root before launching Tauri from it."""
    return path.resolve(strict=True)


def macos_transport_security_check(app_bundle: pathlib.Path) -> dict[str, str]:
    """Verify the installed bundle grants HTTP only to the loopback host."""
    info_plist = app_bundle / "Contents" / "Info.plist"
    try:
        with info_plist.open("rb") as stream:
            info = plistlib.load(stream)
    except (OSError, plistlib.InvalidFileException) as error:
        raise SmokeFailure("macOS ATS", "installed app Info.plist is unreadable") from error

    ats = info.get("NSAppTransportSecurity")
    if not isinstance(ats, dict):
        raise SmokeFailure("macOS ATS", "installed app has no ATS configuration")
    domains = ats.get("NSExceptionDomains")
    if not isinstance(domains, dict) or set(domains) != {"127.0.0.1"}:
        raise SmokeFailure("macOS ATS", "ATS exceptions are not limited to 127.0.0.1")
    exception = domains["127.0.0.1"]
    if not isinstance(exception, dict) or exception.get("NSExceptionAllowsInsecureHTTPLoads") is not True:
        raise SmokeFailure("macOS ATS", "127.0.0.1 HTTP exception is missing")
    broad_exemptions = {
        "NSAllowsArbitraryLoads",
        "NSAllowsArbitraryLoadsForMedia",
        "NSAllowsArbitraryLoadsInWebContent",
        "NSAllowsLocalNetworking",
    }
    if broad_exemptions.intersection(ats):
        raise SmokeFailure("macOS ATS", "broad ATS transport exemption is present")
    return {"loopbackHttpException": "127.0.0.1", "broadExemptions": "absent"}


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


RPM_FORMAT = "%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\t%{EPOCHNUM}\n"


class RpmInstallation:
    def __init__(self, metadata: tuple[str, str, str, str, str]):
        self.metadata = metadata
        self.paths: list[pathlib.Path] = []

    @property
    def name(self) -> str:
        return self.metadata[0]

    @property
    def nevra(self) -> str:
        name, version, release, arch, epoch = self.metadata
        return f"{name}-{epoch}:{version}-{release}.{arch}"


def rpm_metadata(output: bytes) -> tuple[str, str, str, str, str]:
    fields = output.decode("utf-8").removesuffix("\n").split("\t")
    if (len(fields) != 5 or fields[0] not in {"burrata", "Burrata"}
            or not all(re.fullmatch(r"[A-Za-z0-9._+~^-]+", value) for value in fields)
            or fields[3] != "x86_64" or not re.fullmatch(r"0|[1-9][0-9]*", fields[4])):
        raise SmokeFailure("RPM install", "invalid Burrata RPM identity or architecture")
    return tuple(fields)


def verify_rpm(package: RpmInstallation) -> None:
    installed = run(["rpm", "-q", "--queryformat", RPM_FORMAT, package.name], check=False)
    if installed.returncode or rpm_metadata(installed.stdout) != package.metadata:
        raise SmokeFailure("RPM verify", "installed RPM identity differs; never remove an unrelated package")
    verified = run(["rpm", "-V", package.name], check=False)
    if verified.returncode or verified.stdout.strip() or verified.stderr.strip():
        raise SmokeFailure("RPM verify", "installed RPM files do not match their database digests/attributes")


def remove_rpm(package: RpmInstallation) -> None:
    current = run(["rpm", "-q", "--queryformat", RPM_FORMAT, package.name], check=False)
    if current.returncode == 1:
        if any(path.exists() or path.is_symlink() for path in package.paths):
            raise SmokeFailure("RPM cleanup", "RPM absent but recorded runtime paths remain")
        return
    if current.returncode or rpm_metadata(current.stdout) != package.metadata:
        raise SmokeFailure("RPM cleanup", "RPM identity changed; never uninstall an unrelated package")
    run(["sudo", "-n", "dnf", "-y", "--setopt=clean_requirements_on_remove=False",
         "remove", package.nevra], timeout=300)
    absent = run(["rpm", "-q", package.name], check=False)
    if absent.returncode != 1 or any(path.exists() or path.is_symlink() for path in package.paths):
        raise SmokeFailure("RPM cleanup", "installed package or owned runtime paths remain after removal")


def rpm_process_identity(pid: int) -> dict | None:
    """Stat/status identity; protected environments are deliberately never read."""
    root = pathlib.Path('/proc') / str(pid)
    try:
        raw = (root / 'stat').read_text()
        prefix, fields = raw.rsplit(') ', 1)
        fields = fields.split()
        comm = re.sub(r'[^A-Za-z0-9_.:-]', '?', prefix.split('(', 1)[1])[:40]
        # Proc-directory ownership changes with dumpability, not just credentials.
        # Use actual status UIDs for birth checks; never inspect protected env/argv.
        with (root / 'status').open('r') as status_file:
            status = status_file.read(65_537)
        credentials = re.search(r'^Uid:[ \t]+(\d+)[ \t]+(\d+)[ \t]+(\d+)[ \t]+(\d+)[ \t]*$', status, re.MULTILINE)
        if len(status) > 65_536 or credentials is None:
            raise ValueError('bounded process credential status unavailable')
        uid, euid, suid, fsuid = map(int, credentials.groups())
        return {'pid': pid, 'start': int(fields[19]), 'uid': uid, 'euid': euid,
                'suid': suid, 'fsuid': fsuid, 'procOwnerUid': root.stat().st_uid,
                'ppid': int(fields[1]), 'pgid': int(fields[2]), 'sid': int(fields[3]),
                'state': fields[0], 'flags': int(fields[6]), 'comm': comm}
    except (FileNotFoundError, ProcessLookupError):
        return None


def rpm_runtime_snapshot() -> dict[int, dict]:
    rows = {}
    try:
        folders = list(pathlib.Path('/proc').glob('[0-9]*'))
    except Exception as error:
        failure = SmokeFailure('RPM cleanup', 'Process identity enumeration could not be inspected')
        failure.diagnostics = {'probe': 'enumeration', 'pid': None,
                               'errorKind': type(error).__name__, 'errno': getattr(error, 'errno', None)}
        raise failure from error
    for folder in folders:
        pid = int(folder.name)
        try:
            row = rpm_process_identity(pid)
        except Exception as error:
            failure = SmokeFailure('RPM cleanup', 'Process stat identity could not be inspected')
            failure.diagnostics = {'probe': 'stat', 'pid': pid,
                                   'errorKind': type(error).__name__, 'errno': getattr(error, 'errno', None)}
            raise failure from error
        if row is not None:
            rows[pid] = row
    return rows


def rpm_same_identity(left: dict, right: dict | None) -> bool:
    return bool(right and all(left.get(key) == right.get(key) for key in ('pid', 'start', 'uid', 'euid', 'suid', 'fsuid')))


class RpmRuntimeRegistry:
    """Only recorded births may be signalled, through identity-bound Linux pidfds.

    Baseline and fresh stat snapshots distinguish pre-runtime processes from
    owned children/sessions and unexplained new processes. No argv/env probing,
    new root privileges, sandbox changes, or blanket permission-error exclusions.
    """
    def __init__(self, grace_seconds: float = 3):
        self.grace = grace_seconds
        self.baseline = rpm_runtime_snapshot()
        self.owner = self.baseline.get(os.getpid())
        if not self.owner or self.owner['uid'] != os.getuid():
            raise SmokeFailure('RPM cleanup', 'Runtime observer identity is unavailable')
        if not hasattr(os, 'pidfd_open') or not hasattr(signal, 'pidfd_send_signal'):
            raise SmokeFailure('RPM cleanup', 'Identity-bound Linux process handles are required')
        self.owned: dict[int, dict] = {}
        self.handles: dict[int, int] = {}
        self.roots: dict[str, tuple[subprocess.Popen, dict]] = {}
        self.errors: list[dict] = []
        self.snapshots = []
        self.stopping = False
        self.finished = False
        self.attempted = False
        self.record('before-runtime', self.baseline, [])

    def record(self, phase: str, rows: dict, unknown: list) -> None:
        self.snapshots.append({'phase': phase, 'owned': [dict(row) for pid, row in rows.items() if pid in self.owned],
                               'preexisting': [dict(row) for pid, row in rows.items() if pid != self.owner['pid'] and rpm_same_identity(self.baseline.get(pid, {}), row)] if phase == 'before-runtime' else [],
                               'unknownNew': unknown})
        # Bounded history; first snapshot retains concrete pre-start identities.
        if len(self.snapshots) > 30:
            del self.snapshots[1]

    def problem(self, reason: str, **facts) -> None:
        error = {'reason': reason, **facts}
        if error not in self.errors:
            self.errors.append(error)
        failure = SmokeFailure('RPM cleanup', 'Owned runtime cleanup could not be proven: ' + reason)
        failure.diagnostics = self.evidence()
        raise failure

    def evidence(self) -> dict:
        return {'scope': 'RPM-owned runtime births/sessions; no process environments or argv',
                'observer': self.owner, 'baselineProcessCount': len(self.baseline),
                'roots': {role: identity for role, (_, identity) in self.roots.items()},
                'ownedIdentities': list(self.owned.values()), 'snapshots': self.snapshots,
                'errors': self.errors, 'noLiveOwnedProcessesProven': self.finished and not self.errors}

    def bind(self, row: dict, relation: str) -> None:
        pid = row['pid']
        fd = os.pidfd_open(pid, 0)
        try:
            current = rpm_process_identity(pid)
            if current is None:
                raise ProcessLookupError(pid)
            if not rpm_same_identity(row, current):
                self.problem('identity_changed_while_binding', process=row)
        except BaseException:
            os.close(fd)
            raise
        self.handles[pid] = fd
        self.owned[pid] = {**row, 'ownership': relation}

    def register(self, role: str, process: subprocess.Popen) -> None:
        try:
            row = rpm_process_identity(process.pid)
            if (not row or process.returncode is not None or row['ppid'] != self.owner['pid']
                    or row['sid'] != row['pid'] or row['pgid'] != row['pid']
                    or row['uid'] != self.owner['uid'] or row['pid'] in self.baseline):
                self.problem('spawn_root_unverifiable', role=role, process=row or {'pid': process.pid})
            self.bind(row, 'Popen-' + role)
            self.roots[role] = (process, row)
            self.capture('registered-' + role)
        except SmokeFailure:
            raise
        except Exception as error:
            self.problem('root_probe_error', role=role, pid=process.pid,
                         errorKind=type(error).__name__, errno=getattr(error, 'errno', None))

    def capture(self, phase: str) -> dict[int, dict]:
        try:
            return self._capture(phase)
        except SmokeFailure as error:
            if error.diagnostics is not None and error.diagnostics.get('probe') in ('stat', 'enumeration'):
                self.problem('stat_probe_error', **error.diagnostics)
            raise
        except Exception as error:
            self.problem('unexpected_probe_error', errorKind=type(error).__name__, errno=getattr(error, 'errno', None))

    def _capture(self, phase: str) -> dict[int, dict]:
        rows = rpm_runtime_snapshot()
        for role, (process, identity) in self.roots.items():
            current = rows.get(identity['pid'])
            if current and not rpm_same_identity(identity, current):
                self.problem('root_pid_reused', role=role, expected=identity, observed=current)
            if not current and not self.stopping:
                self.problem('root_missing_before_teardown', role=role, expected=identity)
        for pid, identity in self.owned.items():
            if pid in rows and not rpm_same_identity(identity, rows[pid]):
                self.problem('owned_pid_reused', expected=identity, observed=rows[pid])
        # Adopt through observed parentage or an original, still-unreaped session leader.
        vanished = set()
        changed = True
        while changed:
            changed = False
            for pid, row in rows.items():
                if pid in vanished or pid == self.owner['pid'] or pid in self.owned or rpm_same_identity(self.baseline.get(pid, {}), row):
                    continue
                parent = rows.get(row['ppid'])
                anchored_parent = (parent and (rpm_same_identity(self.owner, parent)
                    or (parent['pid'] in self.owned and rpm_same_identity(self.owned[parent['pid']], parent))))
                relation = 'observed-descendant' if anchored_parent and row['start'] >= parent['start'] else None
                for role, (process, identity) in self.roots.items():
                    if (process.returncode is None and rpm_same_identity(identity, rows.get(identity['pid']))
                            and row['sid'] == identity['sid'] and row['start'] >= identity['start']):
                        relation = 'owned-session-' + role
                if relation:
                    try:
                        self.bind(row, relation)
                    except ProcessLookupError:
                        vanished.add(pid)  # Independently observed gone while binding.
                        continue
                    changed = True
        unknown = [dict(row) for pid, row in rows.items() if pid != self.owner['pid']
                   and pid not in vanished and pid not in self.owned and not rpm_same_identity(self.baseline.get(pid, {}), row)
                   and row['state'] != 'Z']
        self.record(phase, rows, unknown)
        if unknown:
            self.problem('unknown_new_process', processes=unknown)
        return rows

    def signal_owned(self, pid: int, sig: int) -> None:
        identity = self.owned[pid]
        try:
            current = rpm_process_identity(pid)
        except Exception as error:
            self.problem('owned_identity_probe_error', process=identity,
                         errorKind=type(error).__name__, errno=getattr(error, 'errno', None))
        if current is None or current['state'] == 'Z':
            return
        if not rpm_same_identity(identity, current):
            self.problem('identity_changed_before_signal', expected=identity, observed=current)
        try:
            signal.pidfd_send_signal(self.handles[pid], sig, None, 0)
        except ProcessLookupError:
            pass
        except OSError as error:
            self.problem('owned_signal_failed', process=current, signal=int(sig), errno=error.errno)

    def stop_owned(self) -> dict:
        if self.finished:
            if self.errors:
                self.problem('previous_cleanup_unproven')
            return self.evidence()
        if self.attempted:
            self.problem('previous_cleanup_unproven')
        self.attempted = True
        primary = None
        try:
            self.capture('before-teardown')
        except SmokeFailure as error:
            primary = error  # Still stop only verified known births; never signal unknowns.
        self.stopping = True
        try:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                # Newer descendants first; original Popen roots remain unreaped throughout signalling.
                for pid in sorted(self.owned, key=lambda pid: self.owned[pid]['start'], reverse=True):
                    try:
                        self.signal_owned(pid, sig)
                    except SmokeFailure as error:
                        primary = primary or error
                deadline = time.monotonic() + self.grace
                while True:
                    try:
                        rows = self.capture('stopping-' + str(int(sig)))
                    except SmokeFailure as error:
                        primary = primary or error
                        rows = {}
                        # Never treat a failed probe as proof of absence.
                        for pid in self.owned:
                            try:
                                row = rpm_process_identity(pid)
                            except Exception as probe_error:
                                # Unknown is live for the bound; never substitute absence.
                                row = {**self.owned[pid], 'state': '?'}
                                if not any(item.get('reason') == 'owned_identity_probe_error' and item.get('process', {}).get('pid') == pid for item in self.errors):
                                    self.errors.append({'reason': 'owned_identity_probe_error', 'process': self.owned[pid], 'errorKind': type(probe_error).__name__, 'errno': getattr(probe_error, 'errno', None)})
                            if row:
                                rows[pid] = row
                    live = [row for pid, row in rows.items() if pid in self.owned and row['state'] != 'Z']
                    if not live or time.monotonic() >= deadline:
                        break
                    time.sleep(.05)
                if not live:
                    break
            if live:
                self.problem('owned_processes_survived_kill', processes=live)
            # Only after live owned descendants are gone may wait() release root PID/session IDs.
            for process, _ in self.roots.values():
                process.wait(timeout=2)
            self.capture('after-root-reap')
            self.finished = True
            if primary or self.errors:
                self.problem('cleanup_identity_or_probe_failure')
            return self.evidence()
        except SmokeFailure:
            raise
        except Exception as error:
            self.problem('cleanup_exception', errorKind=type(error).__name__, errno=getattr(error, 'errno', None))
        finally:
            close_errors = []
            for pid, fd in self.handles.items():
                try:
                    os.close(fd)
                except OSError as error:
                    close_errors.append({'pid': pid, 'errorKind': type(error).__name__, 'errno': error.errno})
            self.handles.clear()
            if close_errors:
                self.problem('pidfd_close_failed', processes=close_errors)


def verify_rpm_profile_stopped(temp: pathlib.Path, registry: RpmRuntimeRegistry | None = None) -> dict:
    if registry is None:
        raise SmokeFailure('RPM cleanup', 'A pre-runtime owned-process registry is required')
    return registry.stop_owned()


def install_rpm(installer: pathlib.Path, temp: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, RpmInstallation]:
    distro = platform.freedesktop_os_release()
    if sys.platform != "linux" or distro.get("ID") != "fedora" or os.geteuid() == 0:
        raise SmokeFailure("RPM install", "RPM qualification requires an unprivileged user on Fedora")
    package = RpmInstallation(rpm_metadata(run(["rpm", "-qp", "--queryformat", RPM_FORMAT, installer]).stdout))
    expected_version = json.loads((ROOT / "package.json").read_text())["version"]
    if package.metadata[1] != expected_version:
        raise SmokeFailure("RPM install", "RPM version differs from the checked-out test source")
    absent = run(["rpm", "-q", package.name], check=False)
    if absent.returncode != 1:
        raise SmokeFailure("RPM install", "RPM already installed or absence not proven; never replace it")
    try:
        run(["sudo", "-n", "dnf", "-y", "install", str(installer.resolve(strict=True))], timeout=600)
        verify_rpm(package)
        output = run(["rpm", "-ql", package.name]).stdout.decode().splitlines()
        paths = [pathlib.Path(item) for item in output]
        if len(set(output)) != len(paths) or any(not p.is_absolute() or ".." in p.parts for p in paths):
            raise SmokeFailure("RPM install", "RPM file list is malformed")
        apps = [p for p in paths if p == pathlib.Path("/usr/bin/burrata")]
        bridges = [p for p in paths if p.as_posix().endswith("/resources/mcp/server.cjs")
                   and (p.as_posix().startswith("/usr/lib/") or p.as_posix().startswith("/usr/lib64/"))]
        if len(apps) != 1 or len(bridges) != 1:
            raise SmokeFailure("RPM install", "RPM must own exactly one app and MCP resource tree")
        for path in [apps[0], bridges[0]]:
            if not path.is_file() or path.is_symlink() or path.resolve(strict=True) != path:
                raise SmokeFailure("RPM install", "installed RPM runtime path is missing or redirected")
            owner = rpm_metadata(run(["rpm", "-qf", "--queryformat", RPM_FORMAT, path]).stdout)
            if owner != package.metadata:
                raise SmokeFailure("RPM install", "runtime path is not owned by this exact RPM")
        if not os.access(apps[0], os.X_OK):
            raise SmokeFailure("RPM install", "installed RPM app is not executable")
        package.paths = [apps[0], bridges[0]]
        return apps[0], bridges[0].parent.parent, package
    except BaseException:
        remove_rpm(package)
        raise


def install_linux(installer: pathlib.Path, temp: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, str | RpmInstallation | None]:
    if installer.suffix.lower() == ".rpm":
        return install_rpm(installer, temp)
    if installer.suffix.lower() == ".appimage":
        extracted = temp / "appimage"
        extracted.mkdir()
        try:
            result = subprocess.run([str(installer), "--appimage-extract"], cwd=extracted,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=120)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SmokeFailure("install", "AppImage extraction could not complete") from error
        if result.returncode:
            raise SmokeFailure("install", "AppImage extraction failed")
        appdir = extracted / "squashfs-root"
        app = appdir / "AppRun"
        mcp = list(appdir.glob("usr/lib/*/resources/mcp/server.cjs"))
        if not app.is_file() or len(mcp) != 1:
            raise SmokeFailure("install", "AppImage is missing its launcher or MCP bridge")
        linked = run(["ldd", "-r", appdir / "usr/bin/burrata"],
                     env={key: value for key, value in os.environ.items()
                          if key not in ("LD_LIBRARY_PATH", "LD_PRELOAD")})
        findings = (linked.stdout + linked.stderr).decode(errors="replace")
        if "undefined symbol:" in findings or "not found" in findings:
            raise SmokeFailure("AppImage linking", "bundled UI runtime has unresolved libraries or symbols")
        return app, mcp[0].parent.parent, None
    if installer.suffix.lower() != ".deb":
        raise SmokeFailure("install", "Linux smoke requires a built DEB, RPM or AppImage")
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


def bundled_media_checks(resource_root: pathlib.Path, target: str, env: dict[str, str],
                         temp: pathlib.Path) -> dict[str, dict[str, str]]:
    """Exercise the packaged FFmpeg pair with deterministic generated media.

    Commands use absolute paths under the installed resource tree while PATH is
    restricted by platform_environment(), so a runner-installed FFmpeg cannot
    satisfy these checks accidentally.
    """
    tools = resource_root / "tools" / target
    ext = ".exe" if sys.platform == "win32" else ""
    ffmpeg = tools / ("ffmpeg" + ext)
    ffprobe = tools / ("ffprobe" + ext)
    if not ffmpeg.is_file() or not ffprobe.is_file():
        raise SmokeFailure("media", "packaged FFmpeg or ffprobe is missing")

    fixture_dir = temp / "media-fixtures"
    fixture_dir.mkdir()
    source = fixture_dir / "synthetic-tone.wav"
    cover = fixture_dir / "synthetic-cover.png"
    mp3 = fixture_dir / "tagged-cover.mp3"
    opus = fixture_dir / "tagged-tone.opus"
    run([ffmpeg, "-nostdin", "-hide_banner", "-v", "error", "-f", "lavfi",
         "-i", "sine=frequency=440:duration=4:sample_rate=44100", "-ac", "2",
         "-c:a", "pcm_s16le", source], timeout=60, env=env)
    run([ffmpeg, "-nostdin", "-hide_banner", "-v", "error", "-f", "lavfi",
         "-i", "color=c=0x7f42ff:s=64x64", "-frames:v", "1", cover],
        timeout=60, env=env)

    run([ffmpeg, "-nostdin", "-hide_banner", "-v", "error", "-i", source,
         "-i", cover, "-map", "0:a:0", "-map", "1:v:0", "-c:a", "libmp3lame",
         "-b:a", "128k", "-c:v", "copy", "-disposition:v:0", "attached_pic",
         "-id3v2_version", "3", "-metadata", "title=Burrata Smoke Fixture",
         "-metadata", "artist=Burrata Test", "-metadata:s:v:0", "title=Album cover",
         "-metadata:s:v:0", "comment=Cover (front)", mp3], timeout=60, env=env)
    run([ffmpeg, "-nostdin", "-hide_banner", "-v", "error", "-i", source,
         "-map", "0:a:0", "-c:a", "libopus", "-b:a", "96k", "-metadata",
         "title=Burrata Smoke Fixture", "-metadata", "artist=Burrata Test", opus],
        timeout=60, env=env)

    def inspect(path: pathlib.Path) -> dict:
        result = run([ffprobe, "-v", "error", "-show_entries",
                      "format=duration:format_tags=title,artist:stream=codec_name,codec_type,width,height:stream_disposition=attached_pic:stream_tags=title,artist,comment",
                      "-of", "json", path], timeout=30, env=env)
        try:
            value = json.loads(result.stdout.decode("utf-8"))
            duration = float(value["format"]["duration"])
        except (ValueError, KeyError, TypeError) as error:
            raise SmokeFailure("media", f"packaged ffprobe returned invalid metadata for {path.suffix}") from error
        if not math.isfinite(duration) or duration < 3.5:
            raise SmokeFailure("media", f"packaged ffprobe reported invalid {path.suffix} duration")
        return value

    mp3_info = inspect(mp3)
    opus_info = inspect(opus)
    mp3_audio = [stream for stream in mp3_info.get("streams", [])
                 if stream.get("codec_type") == "audio"]
    mp3_picture = [stream for stream in mp3_info.get("streams", [])
                   if stream.get("codec_type") == "video"]
    opus_audio = [stream for stream in opus_info.get("streams", [])
                  if stream.get("codec_type") == "audio"]
    if not mp3_audio or mp3_audio[0].get("codec_name") != "mp3":
        raise SmokeFailure("media", "packaged FFmpeg did not produce an MP3 audio stream")
    if not opus_audio or opus_audio[0].get("codec_name") != "opus":
        raise SmokeFailure("media", "packaged FFmpeg did not produce an Opus audio stream")
    if not mp3_picture or mp3_picture[0].get("codec_name") != "png" \
            or mp3_picture[0].get("disposition", {}).get("attached_pic") != 1 \
            or mp3_picture[0].get("width") != 64 \
            or mp3_picture[0].get("height") != 64:
        raise SmokeFailure("media", "packaged ffprobe did not find the embedded PNG cover art")
    if mp3_info.get("format", {}).get("tags", {}).get("title") != "Burrata Smoke Fixture" \
            or mp3_info.get("format", {}).get("tags", {}).get("artist") != "Burrata Test":
        raise SmokeFailure("media", "packaged ffprobe did not read MP3 title and artist metadata")
    if not opus_audio or opus_audio[0].get("tags", {}).get("title") != "Burrata Smoke Fixture" \
            or opus_audio[0].get("tags", {}).get("artist") != "Burrata Test":
        raise SmokeFailure("media", "packaged ffprobe did not read Opus title and artist metadata")

    decoding: dict[str, dict[str, int | float]] = {}
    # These are the same output formats and rates used by the app's BPM and key
    # analysis subprocesses in src-tauri/src/media.rs.
    for path, label in ((mp3, "mp3"), (opus, "opus")):
        s16 = run([ffmpeg, "-nostdin", "-v", "error", "-i", path, "-map", "0:a:0",
                   "-vn", "-ac", "1", "-ar", "8000", "-f", "s16le", "pipe:1"],
                  timeout=60, env=env).stdout
        f32 = run([ffmpeg, "-nostdin", "-v", "error", "-i", path, "-map", "0:a:0",
                   "-vn", "-ac", "1", "-ar", "22050", "-f", "f32le", "pipe:1"],
                  timeout=60, env=env).stdout
        if len(s16) < 8000 * 2 or len(s16) % 2 or len(f32) < 22050 * 4 or len(f32) % 4:
            raise SmokeFailure("media", f"packaged FFmpeg returned incomplete {label} analysis audio")
        floats = (item[0] for item in struct.iter_unpack("<f", f32))
        peak = 0.0
        for sample in floats:
            if not math.isfinite(sample):
                raise SmokeFailure("media", f"packaged FFmpeg returned invalid {label} float audio")
            peak = max(peak, abs(sample))
        if peak < 0.001:
            raise SmokeFailure("media", f"packaged FFmpeg returned silent {label} analysis audio")
        decoding[label] = {
            "bpmS16Samples": len(s16) // 2,
            "keyF32Samples": len(f32) // 4,
            "keyPeak": round(peak, 6),
        }

    return {
        "ffmpeg": {"path": str(ffmpeg.relative_to(resource_root)),
                   "mp3": "encoded", "opus": "encoded", "analysisDecode": decoding},
        "ffprobe": {"path": str(ffprobe.relative_to(resource_root)),
                    "mp3Tags": "read", "opusTags": "read", "pngCover": "attached and decoded"},
    }


def build_native_audio_fixture(resource_root: pathlib.Path, target: str,
                                env: dict[str, str], temp: pathlib.Path,
                                app_data: pathlib.Path) -> dict[str, str | float]:
    """Generate and register a short AAC asset for installed Native playback."""
    ext = ".exe" if sys.platform == "win32" else ""
    tools = resource_root / "tools" / target
    ffmpeg = tools / ("ffmpeg" + ext)
    ffprobe = tools / ("ffprobe" + ext)
    if not ffmpeg.is_file() or not ffprobe.is_file():
        raise SmokeFailure("Native audio setup", "packaged AAC encoder or probe is missing")
    fixture_dir = temp / "native-audio-fixture"
    fixture_dir.mkdir()
    audio = fixture_dir / "installed-smoke.m4a"
    run([ffmpeg, "-nostdin", "-hide_banner", "-v", "error", "-f", "lavfi",
         "-i", "aevalsrc=0.15*sin(2*PI*440*t)|0.12*sin(2*PI*660*t):s=48000:d=12", "-ac", "2",
         "-c:a", "aac", "-b:a", "96k", "-movflags", "+faststart", audio],
        timeout=30, env=env)
    probed = run([ffprobe, "-v", "error", "-select_streams", "a:0", "-show_entries",
                  "stream=codec_name:format=duration", "-of", "json", audio],
                 timeout=15, env=env)
    try:
        info = json.loads(probed.stdout.decode("utf-8"))
        codec = info["streams"][0]["codec_name"]
        duration = float(info["format"]["duration"])
    except (ValueError, KeyError, IndexError, TypeError) as error:
        raise SmokeFailure("Native audio setup", "packaged ffprobe returned invalid AAC fixture metadata") from error
    if codec != "aac":
        raise SmokeFailure("Native audio setup", "packaged FFmpeg did not encode AAC")
    if not math.isfinite(duration) or not 11.5 <= duration <= 13.0:
        raise SmokeFailure("Native audio setup", "AAC fixture is too short for sustained playback qualification")
    fixture = register_native_audio_fixture(app_data, audio, duration, temp)
    return {"trackId": fixture["trackId"], "codec": codec, "mimeType": "audio/mp4", "durationSeconds": fixture["durationSeconds"],
            "registered": "isolated local asset"}


def register_native_audio_fixture(app_data: pathlib.Path, audio_path: pathlib.Path,
                                   duration_seconds: float, temp_root: pathlib.Path) -> dict[str, str | float]:
    """Register one generated AAC file in the smoke-owned, isolated app database."""
    root = temp_root.resolve(strict=True)
    path = audio_path.resolve(strict=True)
    if not path.is_relative_to(root) or path.suffix.lower() != ".m4a" or not path.is_file():
        raise SmokeFailure("Native audio setup", "AAC fixture must be a temporary smoke-owned M4A file")
    if not math.isfinite(duration_seconds) or not 3.5 <= duration_seconds <= 13.0:
        raise SmokeFailure("Native audio setup", "generated AAC fixture duration is outside the smoke bound")

    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    track_id = f"local:{digest}"
    asset_id = str(uuid.uuid4())
    title = "Burrata Installed Native Audio Fixture"
    artist = "Burrata Test"
    track = {
        "id": track_id, "provider": "local", "providerId": digest,
        "sourceUrl": f"local:{digest}", "artist": artist, "title": title,
        "version": "Smoke AAC", "album": "Installed Native Smoke",
        "albumArtist": None, "recordingDate": None, "trackNumber": None,
        "trackTotal": None, "discNumber": None, "discTotal": None,
        "remixers": [], "label": None, "genre": None, "normalizedGenre": None,
        "durationSeconds": duration_seconds, "bpm": None, "key": None,
        "releaseDate": None, "artworkUrl": None, "isrc": None,
        "catalogNumber": None, "rank": None, "previewUrl": None,
        "favorite": False, "disposition": None, "localPath": str(path),
        "localAvailability": "available", "assetId": asset_id,
        "provenance": {"codec": "aac", "synthetic": True},
    }
    asset = {
        "id": asset_id, "recordingId": track_id, "path": str(path), "hash": digest,
        "sourceCodec": "aac", "finalCodec": "aac", "sourceBitrate": None,
        "finalBitrate": None, "sampleRate": 44100, "channels": 2,
        "bitDepth": None, "durationSeconds": duration_seconds,
        "sizeBytes": path.stat().st_size, "bitrateOrigin": "format",
        "processing": "existing-local-file", "track": track,
    }
    database = app_data / "burrata.sqlite"
    if database.exists():
        raise SmokeFailure("Native audio setup", "smoke app database already exists")
    app_data.mkdir(parents=True, exist_ok=True)
    try:
        with closing(sqlite3.connect(database)) as connection:
            connection.executescript(SCHEMA.read_text(encoding="utf-8"))
            with connection:
                connection.execute("INSERT INTO works(id,title,artist) VALUES(?,?,?)",
                                   (f"work:{track_id}", title, artist))
                connection.execute("INSERT INTO recordings(id,work_id,version) VALUES(?,?,?)",
                                   (track_id, f"work:{track_id}", track["version"]))
                connection.execute(
                    "INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)",
                    (track_id, "local", digest, track_id, track["sourceUrl"],
                     json.dumps(track, separators=(",", ":")), time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())),
                )
                connection.execute(
                    "INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?,?,?,?,?)",
                    (asset_id, track_id, str(path), digest, json.dumps(asset, separators=(",", ":"))),
                )
            integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
            if integrity != "ok":
                raise SmokeFailure("Native audio setup", "seeded smoke database failed integrity check")
    except (OSError, sqlite3.Error) as error:
        raise SmokeFailure("Native audio setup", "could not register the local AAC fixture") from error
    return {"trackId": track_id, "durationSeconds": duration_seconds, "codec": "aac", "mimeType": "audio/mp4"}


def safe_media_log_tail(path: pathlib.Path) -> list[str]:
    """Bounded media-platform log lines from the isolated synthetic smoke only."""
    try:
        with path.open("rb") as source:
            source.seek(max(0, path.stat().st_size - 65_536))
            lines = source.read(65_536).decode("utf-8", errors="replace").splitlines()
    except OSError:
        return []
    selected = []
    for line in lines:
        if not re.search(r"gstreamer|gst[_.:]|webkit|pulse(?:audio)?|alsa", line, re.I):
            continue
        line = re.sub(r"[\x00-\x1f\x7f]", " ", line)
        # Drop the full line when it may carry source/account material. Keeping
        # arbitrary fragments of credential-bearing diagnostics is unnecessary.
        if re.search(r"(?:https?|ftp|file):|[/\\]|\b(?:token|access[_-]?token|refresh[_-]?token|authorization|bearer|cookie|password|secret|signature|api[_-]?key)\b", line, re.I):
            line = "[redacted media diagnostic line]"
        selected.append(line[:200])
    return selected[-20:]


def linux_private_monitor_context(app: pathlib.Path, app_process: subprocess.Popen,
                                  audio_process: subprocess.Popen, env: dict[str, str],
                                  temp: pathlib.Path) -> dict:
    """Describe only the owned Linux null-output route; Node verifies the actual stream before unmute."""
    if sys.platform != "linux" or app_process.poll() is not None or audio_process.poll() is not None:
        raise SmokeFailure("Native audio", "private Linux app/server ownership is unavailable")
    socket_path = temp / "pulse-runtime/native"
    if env.get("PULSE_SERVER") != "unix:" + str(socket_path) or env.get("PULSE_SINK") != "burrata_smoke":
        raise SmokeFailure("Native audio", "private Pulse environment does not match the owned server")
    pactl, parec = shutil.which("pactl", path=env.get("PATH")), shutil.which("parec", path=env.get("PATH"))
    if not pactl or not parec:
        raise SmokeFailure("Native audio", "Linux post-gain smoke requires pulseaudio-utils (pactl and parec)")
    executable = app.parent / "usr/bin/burrata" if app.name == "AppRun" else app
    fields = pathlib.Path(f"/proc/{audio_process.pid}/stat").read_text().split(") ", 1)[1].split()
    context = {"private": True, "uid": os.getuid(), "tempRoot": str(temp.resolve(strict=True)),
               "socket": str(socket_path), "appExe": str(executable.resolve(strict=True)),
               "appPgid": os.getpgid(app_process.pid), "pactl": pactl, "parec": parec,
               "pulse": {"pid": audio_process.pid, "startTicks": fields[19],
                         "exe": os.readlink(f"/proc/{audio_process.pid}/exe")},
               "recordFile": str(temp / "native-monitor.f32le")}
    return context


def assess_private_monitor(path: pathlib.Path, control: dict) -> dict:
    """Measure real private-monitor samples AFTER Player gain, never the in-process tap."""
    if any(control.get(key) is not expected for key, expected in {
            "verified": True, "isolatedVirtualOutput": True, "ownedAppStream": True,
            "physicalHardware": False, "postPlayerGain": True, "remuteWhilePlaying": True}.items()):
        raise SmokeFailure("Native audio", "private output route/unmute proof is incomplete")
    advance = control.get("remuteClockAdvanceSeconds")
    if isinstance(advance, bool) or not isinstance(advance, (int, float)) or not math.isfinite(advance) or advance < .5:
        raise SmokeFailure("Native audio", "independent remute playback progress is missing")
    rate, channels = 44100, 2
    if control.get("sampleRate") != rate or control.get("channels") != channels:
        raise SmokeFailure("Native audio", "private monitor format does not match the owned sink")
    if path.is_symlink() or not path.is_file() or not 1 <= path.stat().st_size <= rate * channels * 4 * 10:
        raise SmokeFailure("Native audio", "private monitor is missing, linked, empty or exceeds its bound")
    data = path.read_bytes()
    if len(data) % 8 or control.get("bytes") != len(data):
        raise SmokeFailure("Native audio", "private monitor is incomplete")
    frames = list(struct.iter_unpack("<ff", data))
    if any(not math.isfinite(v) for frame in frames for v in frame):
        raise SmokeFailure("Native audio", "private monitor contains nonfinite PCM")
    unmute, remute = control.get("unmuteOffsetSeconds"), control.get("remuteOffsetSeconds")
    if (isinstance(unmute, bool) or isinstance(remute, bool)
            or not isinstance(unmute, (int, float)) or not isinstance(remute, (int, float))
            or not math.isfinite(unmute) or not math.isfinite(remute)
            or unmute < .25 or remute - unmute < 3.5):
        raise SmokeFailure("Native audio", "private monitor control timing is incomplete")
    bins = []
    size = rate // 20  #50ms bins: exactly22cycles440Hz and33cycles660Hz.
    for start in range(0, len(frames) - size + 1, size):
        block = frames[start:start+size]
        rms = [math.sqrt(sum(frame[c] ** 2 for frame in block) / size) for c in range(2)]
        tone = []
        for c, hz in [(0,440),(0,660),(1,440),(1,660)]:
            re_part = sum(frame[c] * math.cos(math.tau * hz * i / rate) for i,frame in enumerate(block))
            im_part = sum(frame[c] * math.sin(math.tau * hz * i / rate) for i,frame in enumerate(block))
            tone.append(2 * math.hypot(re_part, im_part) / size)
        active = (.025 < rms[0] < .25 and .025 < rms[1] < .25
                  and tone[0] > .06 and tone[1] < tone[0] * .05
                  and tone[3] > .05 and tone[2] < tone[3] * .05)
        bins.append({"start":start/rate,"end":(start+size)/rate,"active":active,
                     "quiet":max(rms) < .002})
    initial = [b for b in bins if b["end"] <= unmute - .15]
    audible = [b for b in bins if b["start"] >= unmute + .25 and b["end"] <= remute - .25]
    quiet = [b for b in bins if b["start"] >= remute + .25]
    if (len(initial) < 2 or not all(b["quiet"] for b in initial)
            or len(audible) < 60 or not all(b["active"] for b in audible)
            or len(quiet) < 10 or not all(b["quiet"] for b in quiet)):
        raise SmokeFailure("Native audio", "post-gain monitor quiet/tone/continuity/remute evidence failed")
    return {"source":"owned Linux Pulse monitor", "postPlayerGain":True,
            "capturedSeconds":len(frames)/rate,"verifiedToneSeconds":len(audible)/20,
            "initialQuietSeconds":len(initial)/20,"remuteQuietSeconds":len(quiet)/20,
            "sampleRate":rate,"channels":channels,"physicalHardware":False}


def linux_process_identity(pid: int) -> dict | None:
    try:
        root = pathlib.Path(f"/proc/{pid}")
        fields = (root / "stat").read_text().split(") ", 1)[1].split()
        return {"pid": pid, "state": fields[0], "pgid": int(fields[2]),
                "sid": int(fields[3]), "start": int(fields[19]), "uid": root.stat().st_uid}
    except (FileNotFoundError, ProcessLookupError):
        return None


def check_owned_group_rows(owner: dict, rows: list[dict]) -> list[dict]:
    for row in rows:
        if (row["pgid"] != owner["pid"] or row["sid"] != owner["pid"]
                or row["uid"] != owner["uid"] or row["start"] < owner["start"]
                or (row["pid"] == owner["pid"] and row["start"] != owner["start"])):
            raise SmokeFailure("Native audio", "private checker process group identity changed; never signal foreign group")
    return rows


def owned_group_members(owner: dict) -> list[dict]:
    rows = []
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            row = linux_process_identity(int(entry.name))
        except PermissionError:
            if entry.stat().st_uid == owner["uid"]:
                raise SmokeFailure("Native audio", "private group membership cannot be verified")
            continue
        if row and row["pgid"] == owner["pid"]:
            rows.append(row)
    return check_owned_group_rows(owner, rows)


def finish_owned_checker_group(process: subprocess.Popen, owner: dict) -> bool:
    """Hold the unreaped leader PID until live descendants are gone, preventing PGID reuse."""
    needed_signal = False
    for sig in (signal.SIGTERM, signal.SIGKILL):
        rows = owned_group_members(owner)
        if not any(row["state"] != "Z" for row in rows):
            break
        leader = linux_process_identity(owner["pid"])
        if not leader or any(leader[key] != owner[key] for key in ("pid", "pgid", "sid", "uid", "start")):
            raise SmokeFailure("Native audio", "checker leader identity unavailable; never signal reused group")
        # No poll/wait/communicate has reaped this leader, including when it is a zombie.
        needed_signal = True
        try:
            os.killpg(owner["pid"], sig)
        except ProcessLookupError:
            pass
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if not any(row["state"] != "Z" for row in owned_group_members(owner)):
                break
            time.sleep(.05)
    if any(row["state"] != "Z" for row in owned_group_members(owner)):
        raise SmokeFailure("Native audio", "owned checker descendants survived SIGKILL")
    process.wait(timeout=3)  # Reap leader only AFTER all live group members are gone.
    deadline = time.monotonic() + 3
    while owned_group_members(owner) and time.monotonic() < deadline:
        time.sleep(.05)  # PID1 reaps orphan zombies; no signals after leader reaping.
    if owned_group_members(owner):
        raise SmokeFailure("Native audio", "owned checker group did not fully disappear")
    return needed_signal


def run_private_audio_check(command: list, env: dict[str, str]) -> subprocess.CompletedProcess:
    """Bound the complete owned session, including descendants after an exited leader."""
    if sys.platform != "linux":
        raise SmokeFailure("Native audio", "owned private checker group is Linux only")
    # Temporary files avoid pipe EOF/communicate reaping the leader before group cleanup.
    with tempfile.TemporaryFile() as stdout_file, tempfile.TemporaryFile() as stderr_file:
        process = subprocess.Popen([os.fspath(value) for value in command], env=env,
                                   stdin=subprocess.DEVNULL, stdout=stdout_file,
                                   stderr=stderr_file, start_new_session=True)
        owner = linux_process_identity(process.pid)
        if not owner or owner["uid"] != os.getuid() or owner["pgid"] != process.pid or owner["sid"] != process.pid:
            raise SmokeFailure("Native audio", "new checker group identity could not be established")
        deadline = time.monotonic() + NATIVE_AUDIO_CHECK_TIMEOUT_SECONDS
        timed_out = False
        while True:
            leader = linux_process_identity(process.pid)
            if not leader or any(leader[key] != owner[key] for key in ("pid", "pgid", "sid", "uid", "start")):
                raise SmokeFailure("Native audio", "checker leader identity changed before reaping")
            if leader["state"] == "Z":
                break
            if (time.monotonic() >= deadline or os.fstat(stdout_file.fileno()).st_size > 1024*1024
                    or os.fstat(stderr_file.fileno()).st_size > 1024*1024):
                timed_out = True
                break
            time.sleep(.05)
        # Brief natural-exit grace; leader remains unreaped so its session ID cannot be reused.
        grace = time.monotonic() + .5
        while not timed_out and any(row["state"] != "Z" for row in owned_group_members(owner)) and time.monotonic() < grace:
            time.sleep(.05)
        signalled = finish_owned_checker_group(process, owner)
        if timed_out or signalled:
            raise SmokeFailure("Native audio", "checker exceeded its bound or left live descendants; owned group terminated")
        stdout_file.seek(0); stderr_file.seek(0)
        return subprocess.CompletedProcess(command, process.returncode,
                                           stdout_file.read(1024*1024), stderr_file.read(1024*1024))


def sanitize_windows_diagnostic(value):
    """Codes and scalar state only: never relay error text, paths, identities or payloads."""
    if not isinstance(value, dict) or value.get('diagnosticOnly') is not True or value.get('qualificationStatus') != 'UNQUALIFIED':
        return None
    steps = {'connect','list_tools','restore_volume','mute','play_fixture','capture_first','capture_second','assess','snapshot'}
    tools = {'set_volume','play_tracks','pause_playback','inspect_native_output','get_app_status'}
    categories = {'output_device_unavailable','output_configuration','timeout','transport','fixture_not_playing','owner_changed','qualification_disabled','decode','unclassified'}
    def failure(row):
        row = row if isinstance(row, dict) else {}
        code = row.get('rpcCode')
        return {'step': row.get('step') if isinstance(row.get('step'),str) and row.get('step') in steps else 'unknown',
                'tool': row.get('tool') if isinstance(row.get('tool'),str) and row.get('tool') in tools else 'none',
                'category': row.get('category') if isinstance(row.get('category'),str) and row.get('category') in categories else 'unclassified',
                'rpcCode': code if type(code) is int and -(2**31) <= code < 2**31 else None}
    state = value.get('nativeState') if isinstance(value.get('nativeState'), dict) else {}
    states = {'idle','resolving','buffering','playing','paused','ended','failed','unavailable'}
    projected = {'engine': 'native' if state.get('engine') == 'native' else 'other_or_missing',
                 'status': state.get('status') if isinstance(state.get('status'),str) and state.get('status') in states else 'other_or_missing',
                 'messageCategory': state.get('messageCategory') if isinstance(state.get('messageCategory'),str) and state.get('messageCategory') in categories else 'unclassified',
                 'hasCurrentTrack': state.get('hasCurrentTrack') is True}
    failure_categories = {'cancelled','timeout','network','rate_limited','source_access','source_unavailable','source_private','source_deleted','no_match','recording_identity_mismatch','unsupported_audio','output','storage','unknown'}
    failure_stages = {'resolver','extraction','transport','output','storage','unknown'}
    projected['failureCategory'] = state.get('failureCategory') if isinstance(state.get('failureCategory'),str) and state['failureCategory'] in failure_categories else 'unknown'
    projected['failureStage'] = state.get('failureStage') if isinstance(state.get('failureStage'),str) and state['failureStage'] in failure_stages else 'unknown'
    for key in ('generation','seekSerial','position'):
        number = state.get(key)
        if type(number) in (int,float) and math.isfinite(number) and 0 <= number <= 2**53-1:
            projected[key] = number
    events = value.get('events') if isinstance(value.get('events'), list) else []
    result = {'diagnosticOnly': True, 'qualificationStatus': 'UNQUALIFIED',
              'muted': value.get('muted') is True, 'beforeCleanup': value.get('beforeCleanup') is True,
              'failure': failure(value.get('failure')), 'nativeState': projected,
              'events': [{**failure(row), 'result': 'ok' if isinstance(row,dict) and row.get('result') == 'ok' else 'failed'} for row in events[-12:]]}
    if isinstance(value.get('lastToolFailure'),dict):
        result['lastToolFailure'] = failure(value['lastToolFailure'])
    return result


def native_checker_failure_evidence(payload: dict, returncode: int) -> dict:
    """Retain known static checker reasons, never arbitrary stdout, paths or URLs."""
    allowed = {
        "Owned private app route unavailable; never unmuted",
        "Owned private Linux output route is not verified; remain muted",
        "Private Pulse module listings disagree",
        "Private Pulse sink monitor is ambiguous",
        "Native session did not finish restoring playback ownership",
        "Native output fixture did not reach the mixer",
        "Native output did not capture the complete stereo fixture",
        "Native output fixture tones or stereo lanes did not match",
        "Native output ownership changed or consumption stalled",
        "Private monitor cleanup or capture failed",
        "Private monitor exceeded byte bound",
        "Private monitor exceeded time bound",
        "Owned private recorder did not close after SIGKILL",
        "Owned private recorder closure not proven",
    }
    reason = payload.get("error")
    result = {"checkerExitCode": returncode,
              "checkerReason": reason if isinstance(reason, str) and reason in allowed else "unrecognized_or_absent"}
    diagnostic = sanitize_windows_diagnostic(payload.get('windowsDiagnostic'))
    if diagnostic is not None:
        result['windowsDiagnostic'] = diagnostic
    return result



def defer_windows_audio_output_failure(error: SmokeFailure, enabled: bool) -> dict | None:
    """Explicit platform deferral, never synthesized playback or PCM success."""
    if not enabled or sys.platform != "win32" or error.stage != "Native audio":
        return None
    details = error.diagnostics if isinstance(error.diagnostics, dict) else {}
    diagnostic = sanitize_windows_diagnostic(details.get("windowsDiagnostic"))
    if diagnostic is None:
        return None
    failure, state = diagnostic["failure"], diagnostic["nativeState"]
    if not (diagnostic["muted"] and diagnostic["beforeCleanup"]
            and failure["step"] == "play_fixture" and failure["tool"] == "play_tracks"
            and failure["category"] == "output_device_unavailable"
            and state["engine"] == "native" and state["status"] == "failed"
            and state["messageCategory"] == "output_device_unavailable"
            and state["failureCategory"] == "output" and state["failureStage"] == "output"
            and state["hasCurrentTrack"]):
        return None
    return {"status": "deferred", "verificationStatus": "UNVERIFIED",
            "originalCheckStatus": "failed", "reasonCode": "output_device_unavailable",
            "reason": "Windows Native audio output verification explicitly deferred because no output device was available",
            "proofScope": "Windows Native audio output and PCM consumption remain unverified",
            "diagnostics": {"windowsDiagnostic": diagnostic}}

def run_mcp_native_audio(resource_root: pathlib.Path, target: str,
                         discovery: pathlib.Path, env: dict[str, str], track_id: str, *, private_monitor: dict | None = None) -> dict:
    node = shutil.which("node")
    ext = ".exe" if sys.platform == "win32" else ""
    mcp = resource_root / "tools" / target / ("burrata-mcp" + ext)
    if not node or not mcp.is_file():
        raise SmokeFailure("Native audio", "installed MCP launcher or Node.js is unavailable")
    command = [node, ROOT / "scripts/release/check-installed-native-audio.mjs", mcp, discovery, track_id]
    if private_monitor is not None:
        if sys.platform != "linux":
            raise SmokeFailure("Native audio", "private unmute/capture is Linux only")
        config = pathlib.Path(private_monitor["tempRoot"]) / "private-monitor-config.json"
        config.write_text(json.dumps(private_monitor), encoding="utf-8")
        config.chmod(0o600)
        command.append(config)
    result = (run_private_audio_check(command, env) if private_monitor is not None
              else run(command, timeout=NATIVE_AUDIO_CHECK_TIMEOUT_SECONDS, env=env, check=False))
    try:
        payload = json.loads(result.stdout.decode().strip().splitlines()[-1])
        if result.returncode or payload.get("status") != "sampled" or payload.get("engine") != "native":
            failure = SmokeFailure("Native audio", "installed decoder/output consumption check failed")
            failure.diagnostics = native_checker_failure_evidence(payload, result.returncode)
            raise failure
        if payload.get("channels") != 2 or payload.get("capturedSeconds", 0) < 2.9 or len(payload.get("measurements", [])) != 2:
            raise SmokeFailure("Native audio", "installed output returned incomplete PCM evidence")
        if private_monitor is not None:
            payload["osOutput"] = assess_private_monitor(pathlib.Path(private_monitor["recordFile"]), payload.get("privateMonitorControl", {}))
        return payload
    except (ValueError, IndexError, KeyError, TypeError) as error:
        raise SmokeFailure("Native audio", "installed MCP returned invalid Native audio evidence") from error


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


def linux_audio_diagnostics(env: dict[str, str]) -> dict:
    """Report server/default-sink availability and bounded state counts only."""
    pactl = shutil.which("pactl", path=env.get("PATH"))
    if not pactl:
        return {"available": False}
    try:
        diagnostic_env = {**env, "LC_ALL": "C"}
        info = run([pactl, "info"], timeout=5, env=diagnostic_env, check=False)
        sinks = run([pactl, "list", "short", "sinks"], timeout=5, env=diagnostic_env, check=False)
    except SmokeFailure:
        return {"available": True, "inspectionCompleted": False}
    info_text = info.stdout[:65_536].decode("utf-8", errors="replace")
    rows = sinks.stdout[:65_536].decode("utf-8", errors="replace").splitlines()[:64]
    states = {state: 0 for state in ("RUNNING", "IDLE", "SUSPENDED", "UNKNOWN")}
    count = 0
    for row in rows:
        fields = row.split("\t")
        if len(fields) < 2 or not fields[0].isdigit():
            continue
        count += 1
        state = fields[-1].strip().upper()
        states[state if state in states else "UNKNOWN"] += 1
    return {
        "available": True, "inspectionCompleted": True,
        "serverReachable": info.returncode == 0, "sinkListingSucceeded": sinks.returncode == 0,
        "defaultSinkPresent": any(line.startswith("Default Sink:") and line.partition(":")[2].strip() for line in info_text.splitlines()),
        "sinkCount": count, "sinkStates": states,
    }


def start_linux_audio(env: dict[str, str], temp: pathlib.Path, registry: RpmRuntimeRegistry | None = None) -> subprocess.Popen:
    """Own a clocked test endpoint without changing the user's audio server."""
    pulse, pactl = shutil.which("pulseaudio"), shutil.which("pactl")
    if not pulse or not pactl:
        raise SmokeFailure("launch", "Linux smoke requires pulseaudio and pactl")
    runtime = temp / "pulse-runtime"
    runtime.mkdir(mode=0o700)
    env.update(XDG_RUNTIME_DIR=str(runtime), PULSE_SERVER="unix:" + str(runtime / "native"),
               PULSE_SINK="burrata_smoke", PULSE_SOURCE="burrata_smoke.monitor")
    # PulseAudio's ordinary null sink renders in two-second blocks and rewinds
    # on new streams. norewinds uses a 50ms maximum block instead. A dedicated
    # server also avoids suspension/latency changes from unrelated clients.
    # See PulseAudio src/modules/module-null-sink.c, BLOCK_USEC_NOREWINDS.
    command = [pulse, "--daemonize=no", "--use-pid-file=no", "--exit-idle-time=-1", "-n",
               "--load=module-native-protocol-unix socket=" + str(runtime / "native") + " auth-anonymous=1",
               "--load=module-null-sink sink_name=burrata_smoke rate=44100 channels=2 norewinds=1"]
    with (temp / "virtual-audio.log").open("wb") as log:
        process = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=log, start_new_session=True)
    if registry is not None:
        registry.register('pulse', process)
    try:
        deadline = time.monotonic() + 10
        while process.poll() is None and time.monotonic() < deadline:
            result = run([pactl, "list", "short", "sinks"], timeout=2, env=env, check=False)
            if result.returncode == 0 and any(
                    row.split("\t")[1:2] == ["burrata_smoke"]
                    for row in result.stdout.decode(errors="replace").splitlines()):
                return process
            time.sleep(0.1)
        raise SmokeFailure("launch", "private clocked Linux audio endpoint did not become ready")
    except BaseException:
        if registry is not None:
            registry.stop_owned()
        else:
            stop_process(process, temp)
        raise


def launch_app(app: pathlib.Path, env: dict[str, str], temp: pathlib.Path, registry: RpmRuntimeRegistry | None = None) -> subprocess.Popen:
    if sys.platform == "linux":
        dbus = shutil.which("dbus-run-session")
        xvfb = shutil.which("xvfb-run")
        if not dbus or not xvfb:
            raise SmokeFailure("launch", "Linux smoke requires dbus-run-session and xvfb-run")
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
        process = subprocess.Popen([os.fspath(item) for item in command], **kwargs)
        if registry is not None:
            registry.register('app', process)
        return process
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
                   env: dict[str, str], *, command: pathlib.Path | None = None,
                   command_args: tuple[str, ...] = ()) -> dict:
    ext = ".exe" if sys.platform == "win32" else ""
    mcp = command if command is not None else resource_root / "tools" / target / ("burrata-mcp" + ext)
    if not mcp.is_file():
        raise SmokeFailure("mcp", "packaged console MCP launcher is missing")
    node = shutil.which("node")
    if not node:
        raise SmokeFailure("mcp", "test runner Node.js is unavailable")
    result = run([node, ROOT / "scripts/release/check-installed-mcp.mjs", mcp, discovery, *command_args],
                 timeout=30, env=env)
    try:
        return json.loads(result.stdout.decode().strip().splitlines()[-1])
    except (ValueError, IndexError) as error:
        raise SmokeFailure("mcp", "installed MCP client returned invalid evidence") from error


def run_mcp_entrypoints(app: pathlib.Path, resource_root: pathlib.Path, target: str,
                        discovery: pathlib.Path, env: dict[str, str]) -> dict:
    """Exercise public app --mcp and any additional shipped console executable."""
    result = {"applicationMcp": run_mcp_status(
        resource_root, target, discovery, env, command=app, command_args=("--mcp",)),
        "shippedConsole": []}
    binary = "burrata-mcp.exe" if sys.platform == "win32" else "burrata-mcp"
    # Native installers place helpers beside the app. AppImage's app is its
    # AppRun wrapper, while the additional console command lives in usr/bin.
    seen = set()
    for console in [app.with_name(binary), app.parent / "usr/bin" / binary]:
        if console.is_file() and console.resolve() not in seen:
            seen.add(console.resolve())
            result["shippedConsole"].append(run_mcp_status(
                resource_root, target, discovery, env, command=console))
    return result


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
    parser.add_argument("--source-version")
    parser.add_argument("--harness-commit")
    parser.add_argument("--windows-diagnostic", action="store_true", help="Muted Windows diagnosis only; never package qualification")
    parser.add_argument("--defer-windows-audio-output", action="store_true", help="Windows only: explicitly defer unavailable output devices; never report audio verified")
    args = parser.parse_args()
    if args.defer_windows_audio_output and (sys.platform != "win32" or args.windows_diagnostic):
        parser.error("Windows audio output deferral requires Windows and cannot be combined with diagnostic-only mode")
    if args.windows_diagnostic and sys.platform != "win32":
        parser.error("Windows diagnostic is Windows only")
    installer = args.installer.resolve()
    evidence_path = args.evidence.resolve()
    evidence: dict = {"schemaVersion": 1, "platform": sys.platform,
                      "target": TARGETS.get(sys.platform, "unknown"),
                      "installer": installer.name, "status": "failed", "checks": {},
                      "osSecurityPrompts": "not tested"}
    if args.windows_diagnostic:
        evidence.update(diagnosticOnly=True, qualificationStatus="UNQUALIFIED")
    if args.source_run_id:
        evidence["sourceRunId"] = args.source_run_id
    if args.source_commit:
        evidence["sourceCommit"] = args.source_commit
    if args.source_version:
        evidence["sourceVersion"] = args.source_version
    if args.harness_commit:
        evidence["harnessCommit"] = args.harness_commit
    if installer.suffix.lower() == ".rpm":
        with installer.open("rb") as rpm_source:
            evidence["installerSha256"] = hashlib.file_digest(rpm_source, "sha256").hexdigest()
    app_process = None
    audio_process = None
    package_name = None
    rpm_registry = None
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
    if args.windows_diagnostic or args.defer_windows_audio_output:
        env["BURRATA_WINDOWS_NATIVE_DIAGNOSTIC"] = "1"
    try:
        if sys.platform == "linux":
            app, resource_root, package_name = install_linux(installer, temp)
            if installer.suffix.lower() == ".appimage":
                env["APPDIR"] = str(app.parent)
            installed_path = str(app)
            if isinstance(package_name, RpmInstallation):
                rpm_registry = RpmRuntimeRegistry()
        elif sys.platform == "darwin":
            app, resource_root, installed_path = install_macos(installer, temp)
        elif sys.platform == "win32":
            app, resource_root, installed_path = install_windows(installer, temp)
        else:
            raise SmokeFailure("install", f"unsupported smoke platform: {sys.platform}")
        evidence["checks"]["installer"] = "installed"
        evidence["installedLocation"] = pathlib.Path(installed_path).name if sys.platform != "linux" else installed_path

        if sys.platform == "darwin":
            evidence["checks"]["transportSecurity"] = macos_transport_security_check(pathlib.Path(installed_path))

        target = TARGETS[sys.platform]
        evidence["checks"]["packagedTools"] = bundled_tool_checks(resource_root, target, env)
        evidence["checks"]["mediaFixtures"] = bundled_media_checks(resource_root, target, env, temp)
        audio_fixture = build_native_audio_fixture(resource_root, target, env, temp, app_data)
        evidence["checks"]["nativeAudioFixture"] = {
            key: value for key, value in audio_fixture.items() if key != "trackId"
        }
        env["BURRATA_NATIVE_OUTPUT_QUALIFICATION"] = "1"
        if sys.platform == "linux":
            audio_process = start_linux_audio(env, temp, rpm_registry) if rpm_registry is not None else start_linux_audio(env, temp)
            evidence["checks"]["linuxAudioEndpoint"] = {"private": True, "maximumBlockMs": 50, "rewinds": False}
        app_process = launch_app(app, env, temp, rpm_registry) if rpm_registry is not None else launch_app(app, env, temp)
        discovery = app_data / "automation.json"
        wait_for_app(app_process, discovery, temp / "installed-app.stderr.log")
        if rpm_registry is not None:
            rpm_registry.capture("frontend-ready")
        evidence["checks"]["guiStartup"] = "automation discovery published"
        evidence["checks"]["nativeHost"] = native_host_check(resource_root, target, env)
        evidence["checks"]["mcp"] = run_mcp_status(resource_root, target, discovery, env)
        evidence["checks"]["mcpEntrypoints"] = run_mcp_entrypoints(app, resource_root, target, discovery, env)
        if sys.platform == "linux":
            evidence["checks"]["linuxAudio"] = {"beforePlayback": linux_audio_diagnostics(env)}
        try:
            evidence["checks"]["nativeAudio"] = run_mcp_native_audio(
                resource_root, target, discovery, env, str(audio_fixture["trackId"]),
                private_monitor=linux_private_monitor_context(app, app_process, audio_process, env, temp) if sys.platform == "linux" else None
            )
        except SmokeFailure as error:
            deferred = defer_windows_audio_output_failure(error, args.defer_windows_audio_output)
            if deferred is None:
                raise
            evidence["checks"]["nativeAudio"] = deferred
            evidence.update(qualificationStatus="WINDOWS_NATIVE_AUDIO_OUTPUT_UNVERIFIED", deferredChecks=["nativeAudio"])
        if evidence["checks"]["nativeAudio"].get("status") != "deferred":
            evidence["checks"]["nativeAudio"]["proofScope"] = ("Linux private OS monitor after Player gain; no physical hardware" if sys.platform == "linux" else "Muted pre-Player-gain mixer PCM; no OS loopback/physical proof")
        if isinstance(package_name, RpmInstallation):
            evidence["checks"]["rpmRuntimeCleanup"] = verify_rpm_profile_stopped(temp, rpm_registry)
            app_process = None
            audio_process = None
            verify_rpm(package_name)
            rpm_identity = package_name.nevra
            rpm_version = package_name.metadata[1]
            remove_rpm(package_name)
            package_name = None
            evidence["checks"]["rpm"] = {"installedIdentity": rpm_identity,
                                           "installedVersion": rpm_version,
                                           "postRuntimeVerify": True, "uninstalled": True,
                                           "packageAndRuntimePathsAbsent": True,
                                           "runtimeUid": os.geteuid()}
        evidence["status"] = ("passed_with_deferred_checks" if evidence.get("deferredChecks")
                              else "diagnostic_only" if args.windows_diagnostic else "passed")
        print(json.dumps({"status": evidence["status"], "platform": sys.platform,
                          "checks": evidence["checks"]}, separators=(",", ":")))
        return 0
    except SmokeFailure as error:
        evidence["failedStage"] = error.stage
        evidence["error"] = str(error)
        if error.stage == "Native audio" and error.diagnostics is not None:
            evidence["checks"]["nativeAudio"] = {"status": "failed", "diagnostics": error.diagnostics}
        if error.stage == "RPM cleanup" and error.diagnostics is not None:
            evidence["checks"]["rpmRuntimeCleanup"] = error.diagnostics
        print(f"Installed smoke failed at {error.stage}: {error}", file=sys.stderr)
        return 1
    finally:
        if sys.platform == "linux" and app_process is not None:
            evidence["checks"].setdefault("linuxAudio", {})["afterPlayback"] = linux_audio_diagnostics(env)
        runtime_stopped = True
        runtime_cleanup_error = None
        if rpm_registry is not None:
            try:
                evidence["checks"]["rpmRuntimeCleanup"] = rpm_registry.stop_owned()
            except SmokeFailure as error:
                runtime_stopped = False
                runtime_cleanup_error = error
                evidence.update(status="failed", failedStage="RPM cleanup", error=str(error))
                evidence["checks"]["rpmRuntimeCleanup"] = error.diagnostics or rpm_registry.evidence()
            app_process = None
            audio_process = None
        else:
            stop_process(app_process, temp)
        if evidence.get("failedStage") == "Native audio" or evidence["checks"].get("nativeAudio", {}).get("status") == "deferred":
            evidence["checks"].setdefault("nativeAudio", {"status": "failed"})["mediaLogTail"] = {
                "stderr": safe_media_log_tail(temp / "installed-app.stderr.log"),
                "stdout": safe_media_log_tail(temp / "installed-app.stdout.log"),
            }
        rpm_cleanup_error = None
        if isinstance(package_name, RpmInstallation) and runtime_stopped:
            try:
                remove_rpm(package_name)
            except SmokeFailure as error:
                rpm_cleanup_error = error
                evidence.update(status="failed", failedStage="RPM cleanup", error=str(error))
        elif isinstance(package_name, RpmInstallation):
            evidence["checks"]["rpmUninstallSkippedUnprovenRuntime"] = True
        elif sys.platform == "linux" and package_name:
            run(["sudo", "-n", "apt-get", "remove", "-y", package_name], timeout=300, check=False)
        if sys.platform == "darwin" and mount:
            run(["hdiutil", "detach", "-quiet", mount], timeout=30, check=False)
        if sys.platform == "win32" and installed_path:
            install_dir = pathlib.Path(installed_path)
            uninstaller = next(install_dir.glob("*uninstall*.exe"), None) if install_dir.is_dir() else None
            if uninstaller:
                run([uninstaller, "/S"], timeout=120, check=False)
        stop_process(audio_process, temp)
        evidence_path.parent.mkdir(parents=True, exist_ok=True)
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
        temp_obj.cleanup()
        if runtime_cleanup_error is not None:
            raise runtime_cleanup_error
        if rpm_cleanup_error is not None:
            raise rpm_cleanup_error


if __name__ == "__main__":
    raise SystemExit(main())
