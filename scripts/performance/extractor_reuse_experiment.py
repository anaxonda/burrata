#!/usr/bin/env python3
"""Isolated yt-dlp process/state reuse experiment.

The live run uses the locked Linux CLI and the exact locked Python source archive.
It records summaries only: extractor output, signed URLs, headers, and raw PCM are
never written to disk or stdout. One fresh loopback helper is shared by all arms;
each request gets a one-use loopback readiness gate and random token, matching the
production provider lease shape while keeping the helper warm across arms.
"""
from __future__ import annotations

import argparse
import hashlib
import http.server
import io
import json
import os
import pathlib
import re
import runpy
import select
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from urllib.parse import urlsplit
from contextlib import redirect_stderr, redirect_stdout

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu/build-recipe/locked-tools.json"
TOOLS = ROOT / "src-tauri/resources/tools/x86_64-unknown-linux-gnu"
BGUTIL = ROOT / "src-tauri/resources/youtube-support/bgutil"
VIDEO_IDS = ("jNQXAC9IVRw", "BaW_jenozKc", "aqz-KE-bpKQ")
HARD_LIMIT_SECONDS = 20.0
FORMAT_SPEC = ("bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/"
    "bestaudio[protocol^=http][protocol!*=dash]/bestaudio/"
    "worst[ext=mp4][acodec!=none][vcodec!=none][protocol^=http][protocol!*=dash]")
MAX_GATE_BODY = 256 * 1024
MAX_GATE_RESPONSE = 1024 * 1024


def safe_env(home: pathlib.Path) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in {
        "YOUTUBE_COOKIES", "YTDLP_COOKIES", "NETRC", "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME", "XDG_DATA_HOME", "PYTHONPATH", "PYTHONHOME", "PYTHONSTARTUP",
    } and not k.startswith("YTDLP_")}
    env.update({
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_CACHE_HOME": str(home / "cache"), "XDG_DATA_HOME": str(home / "data"),
        "PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1",
        "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
    })
    return env


def reserve_port() -> int:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


class GateHandler(http.server.BaseHTTPRequestHandler):
    helper_url = ""
    token = ""
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        return

    def _forward(self):
        parts = self.path.strip("/").split("/")
        if len(parts) != 2 or parts[0] != self.token or parts[1] not in ("ping", "get_pot"):
            self.send_error(404)
            return
        kind = parts[1]
        if (self.command, kind) not in (("GET", "ping"), ("POST", "get_pot")):
            self.send_error(404)
            return
        if self.headers.get("Origin") or self.headers.get("Sec-Fetch-Site", "").lower() == "cross-site":
            self.send_error(403)
            return
        body = b""
        if self.command == "POST":
            length = int(self.headers.get("Content-Length", "0"))
            if length < 0 or length > MAX_GATE_BODY:
                self.send_error(413)
                return
            body = self.rfile.read(length)
        request = urllib.request.Request(self.helper_url + "/" + kind, data=body or None,
            method=self.command, headers={k: v for k, v in self.headers.items()
                if k.lower() not in ("host", "connection", "content-length", "transfer-encoding")})
        try:
            with urllib.request.urlopen(request, timeout=HARD_LIMIT_SECONDS) as response:
                data = response.read(MAX_GATE_RESPONSE + 1)
                status, content_type = response.status, response.headers.get("Content-Type")
        except urllib.error.HTTPError as error:
            data, status, content_type = error.read(MAX_GATE_RESPONSE + 1), error.code, error.headers.get("Content-Type")
        except Exception:
            self.send_error(502)
            return
        if len(data) > MAX_GATE_RESPONSE:
            self.send_error(502)
            return
        self.send_response(status)
        if content_type:
            self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = _forward
    do_POST = _forward


class Gate:
    def __init__(self, helper_url: str):
        self.token = uuid.uuid4().hex
        handler = type("ScopedGateHandler", (GateHandler,), {
            "helper_url": helper_url, "token": self.token,
        })
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.server.daemon_threads = True
        self.server.timeout = 0.2
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base_url = f"http://127.0.0.1:{self.server.server_port}/{self.token}"

    @property
    def args(self) -> list[str]:
        root = BGUTIL
        return ["--plugin-dirs", str(root), "--js-runtimes", f"node:{TOOLS / 'node'}",
            "--extractor-args", f"youtubepot-burratagate:base_url={self.base_url}",
            "--extractor-args", f"youtubepot-bgutilhttp:base_url={self.base_url}",
            "--extractor-args", f"youtubepot-bgutilscript:server_home={BGUTIL / 'server'}"]

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)


class GateReference:
    def __init__(self, base_url: str):
        self.base_url = base_url


class JsonLineReader:
    MAX_FRAME_BYTES = 64 * 1024

    def __init__(self, fd: int):
        self.fd = fd
        self.buffer = bytearray()

    def read_until(self, deadline: float) -> str | None:
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            ready, _, _ = select.select([self.fd], [], [], remaining)
            if not ready:
                return None
            chunk = os.read(self.fd, 4096)
            if not chunk:
                return None
            self.buffer.extend(chunk)
            if len(self.buffer) > self.MAX_FRAME_BYTES:
                return None
        line, _, rest = self.buffer.partition(b"\n")
        self.buffer = bytearray(rest)
        return line.decode("utf-8")


def stop_process_group(proc: subprocess.Popen):
    if proc is None:
        return
    # The leader may already have exited while an owned JS child remains.
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=1)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    proc.wait(timeout=1)


def run_helper(tmp: pathlib.Path) -> tuple[subprocess.Popen, str]:
    started = time.monotonic()
    port = reserve_port()
    env = safe_env(tmp / "helper-home")
    helper = subprocess.Popen([str(TOOLS / "node"), str(BGUTIL / "server/build/main.js"), "--port", str(port)],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env, start_new_session=True)
    url = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if helper.poll() is not None:
            raise RuntimeError("helper exited before readiness")
        try:
            with urllib.request.urlopen(url + "/ping", timeout=0.2) as response:
                if response.status == 200:
                    return helper, url, (time.monotonic() - started) * 1000
        except Exception:
            time.sleep(0.05)
    stop_process_group(helper)
    raise RuntimeError("helper readiness deadline exceeded")


def pcm_probe(url: str, headers: dict, ffmpeg: pathlib.Path, deadline: float, env: dict[str, str]) -> tuple[bool, int, float]:
    started = time.monotonic()
    args = [str(ffmpeg), "-nostdin", "-v", "error", "-rw_timeout", "7000000"]
    allowed = {"user-agent", "referer", "origin", "accept-language"}
    clean_headers = [(name, value) for name, value in headers.items() if name.lower() in allowed
        and isinstance(value, str) and len(value) <= 2048 and not any(ord(char) < 32 or ord(char) == 127 for char in value)]
    if clean_headers:
        args.extend(["-headers", "".join(f"{name}: {value}\r\n" for name, value in clean_headers)])
    args.extend(["-i", url, "-t", "5", "-vn", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:1"])
    # Worker FFmpeg belongs to the worker group so the supervising parent can
    # kill it even when Python is blocked or receives a hard deadline.
    shares_worker_group = os.environ.get("BURRATA_REUSE_WORKER_GROUP") == "1"
    proc = subprocess.Popen(args,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env,
        start_new_session=not shares_worker_group)
    wanted = 5 * 48_000 * 2 * 2
    got = 0
    try:
        while got < wanted and time.monotonic() < deadline:
            ready, _, _ = select.select([proc.stdout], [], [], max(0.0, min(0.2, deadline - time.monotonic())))
            if not ready:
                if proc.poll() is not None:
                    break
                continue
            chunk = proc.stdout.read1(min(32_768, wanted - got))
            if not chunk:
                break
            got += len(chunk)
        if shares_worker_group:
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=0.5)
        else:
            stop_process_group(proc)
    finally:
        if proc.stdout:
            proc.stdout.close()
    return got >= wanted, got, (time.monotonic() - started) * 1000


def ydl_params(gate: Gate) -> dict:
    return {
        "ignoreconfig": True, "no_warnings": True, "quiet": True, "no_color": True,
        "cachedir": False, "no_cache_dir": True, "noplaylist": True,
        "usenetrc": False, "cookiefile": None, "cookiesfrombrowser": None,
        "format": FORMAT_SPEC,
        "extractor_args": {"youtube": {"player_client": ["mweb"]},
            "youtubepot-burratagate": {"base_url": [gate.base_url]},
            "youtubepot-bgutilhttp": {"base_url": [gate.base_url]},
            "youtubepot-bgutilscript": {"server_home": [str(BGUTIL / "server")]}},
        "js_runtimes": {"node": {"path": str(TOOLS / "node")}},
        "logger": type("QuietLogger", (), {"debug": lambda *_: None, "info": lambda *_: None,
            "warning": lambda *_: None, "error": lambda *_: None})(),
    }


def classify_extractor_failure(error: Exception) -> dict:
    """Inspect locally; only fixed categories/types may cross the protocol."""
    name = type(error).__name__
    text = str(error).lower()[:16_384]
    category = "extractor_error"
    if "no such option" in text or ("usage:" in text and "error:" in text):
        category = "cli_usage_error"
    elif isinstance(error, (ModuleNotFoundError, ImportError)):
        category = "module_unavailable"
    elif isinstance(error, (TimeoutError, subprocess.TimeoutExpired)) or "timed out" in text:
        category = "deadline"
    elif any(value in text for value in ("no supported javascript runtime", "javascript runtime unavailable", "node executable")):
        category = "runtime_unavailable"
    elif any(value in text for value in ("challenge solver", "yt_dlp_ejs", "yt-dlp-ejs", "signature extraction", "n challenge", "sig challenge")):
        category = "challenge_failed"
    elif any(value in text for value in ("po token", "pot provider", "botguard", "content binding")):
        category = "token_failed"
    elif any(value in text for value in ("sign in", "login required", "authentication", "age-restricted", "private video")):
        category = "authentication_required"
    elif any(value in text for value in ("requested format is not available", "no video formats", "no playable formats")):
        category = "format_unavailable"
    elif any(value in text for value in ("429", "too many requests", "rate limit")):
        category = "rate_limited"
    elif any(value in text for value in ("http error 403", "http error 401", "access denied")):
        category = "access_denied"
    elif isinstance(error, (urllib.error.URLError, ConnectionError)) or any(value in text for value in ("connection refused", "connection reset", "network is unreachable", "name or service not known", "temporary failure in name resolution")):
        category = "transport_failed"
    allowed_types = {"ModuleNotFoundError", "ImportError", "TimeoutError", "TimeoutExpired",
        "DownloadError", "ExtractorError", "URLError", "HTTPError", "ConnectionError",
        "ConnectionRefusedError", "ValueError", "RuntimeError"}
    return {"errorClass": category, "errorType": name if name in allowed_types else "UnknownError"}


def run_embedded(video_id: str, source_root: pathlib.Path, gate: Gate, home: pathlib.Path,
                 deadline: float, reused=None) -> dict:
    result = {"videoId": video_id, "ok": False, "identityMatched": False, "pcm5s": False,
        "pcmBytes": 0, "initMs": None, "extractMs": None, "pcmMs": None, "errorClass": None,
        "errorType": None}
    env = safe_env(home)
    os.environ.update({k: v for k, v in env.items() if k.startswith("XDG_") or k == "HOME"})
    try:
        stage = "ydl_init"
        start = time.monotonic()
        if reused is None:
            sys.path.insert(0, str(source_root))
            import yt_dlp
            ydl = yt_dlp.YoutubeDL(ydl_params(gate))
            result["initMs"] = (time.monotonic() - start) * 1000
        else:
            ydl = reused
        stage = "extract_info"
        extract_started = time.monotonic()
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            info = ydl.extract_info(f"https://www.youtube.com/watch?v={video_id}", download=False)
        result["extractMs"] = (time.monotonic() - extract_started) * 1000
        if time.monotonic() >= deadline:
            result["errorClass"] = "deadline"
            return result
        if isinstance(info, dict) and info.get("entries"):
            info = next((entry for entry in info["entries"] if isinstance(entry, dict)), {})
        result["identityMatched"] = isinstance(info, dict) and str(info.get("id", "")) == video_id
        formats = info.get("requested_formats") or info.get("formats") or []
        if isinstance(formats, dict):
            formats = [formats]
        if info.get("formats") and "ydl" in locals():
            formats = ydl._select_formats(info["formats"], ydl.build_format_selector(FORMAT_SPEC))
        audio = next((f for f in formats if isinstance(f, dict) and f.get("url") and f.get("acodec") not in (None, "none")), None)
        if audio is None and isinstance(info, dict) and info.get("url") and info.get("acodec") not in (None, "none"):
            audio = info
        if not result["identityMatched"] or audio is None:
            result["errorClass"] = "identity_or_format"
            return result
        result["formatId"] = audio.get("format_id")
        result["ok"] = True
        result["pcm5s"], result["pcmBytes"], result["pcmMs"] = pcm_probe(audio["url"], audio.get("http_headers", {}), TOOLS / "ffmpeg", deadline, env)
        if not result["pcm5s"]:
            result["errorClass"] = "pcm_short_or_deadline"
        return result
    except Exception as error:
        result.update(classify_extractor_failure(error))
        result["errorStage"] = stage
        return result


def run_cli(video_id: str, binary: pathlib.Path, gate: Gate, home: pathlib.Path, deadline: float) -> dict:
    result = {"videoId": video_id, "ok": False, "identityMatched": False, "pcm5s": False,
        "pcmBytes": 0, "processMs": None, "extractMs": None, "pcmMs": None, "errorClass": None}
    env = safe_env(home)
    start = time.monotonic()
    remaining = max(0.1, deadline - start)
    args = [str(binary), "--ignore-config", "--no-cache-dir", "--no-warnings", "--quiet",
        "--no-playlist", "--format", FORMAT_SPEC,
        "--dump-single-json", "--skip-download", "--extractor-args",
        "youtube:player_client=mweb", *gate.args, "--", f"https://www.youtube.com/watch?v={video_id}"]
    try:
        proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=env, start_new_session=True)
        try:
            stdout, _ = proc.communicate(timeout=remaining)
        except subprocess.TimeoutExpired:
            stop_process_group(proc)
            result["processMs"] = (time.monotonic() - start) * 1000
            result["errorClass"] = "deadline"
            return result
        result["processMs"] = (time.monotonic() - start) * 1000
        result["extractMs"] = result["processMs"]
        if proc.returncode:
            result["errorClass"] = "extractor_error"
            return result
        info = json.loads(stdout)
        result["identityMatched"] = isinstance(info, dict) and str(info.get("id", "")) == video_id
        formats = info.get("requested_formats") or info.get("formats") or []
        if isinstance(formats, dict):
            formats = [formats]
        audio_formats = [f for f in formats if isinstance(f, dict) and f.get("url") and f.get("acodec") not in (None, "none")]
        eligible = [f for f in audio_formats if f.get("ext") == "m4a" and str(f.get("protocol", "")).startswith("http") and "dash" not in str(f.get("protocol", ""))]
        eligible = eligible or [f for f in audio_formats if str(f.get("protocol", "")).startswith("http") and "dash" not in str(f.get("protocol", ""))]
        worst_muxed = [f for f in audio_formats if f.get("ext") == "mp4" and f.get("vcodec") not in (None, "none")
            and str(f.get("protocol", "")).startswith("http") and "dash" not in str(f.get("protocol", ""))]
        audio = eligible[-1] if eligible else (worst_muxed[0] if worst_muxed else (audio_formats[-1] if audio_formats else None))
        if audio is None and info.get("acodec") not in (None, "none"):
            audio = info
        if not result["identityMatched"] or audio is None:
            result["errorClass"] = "identity_or_format"
            return result
        result["ok"] = True
        result["formatId"] = audio.get("format_id")
        result["pcm5s"], result["pcmBytes"], result["pcmMs"] = pcm_probe(audio["url"], audio.get("http_headers", {}), TOOLS / "ffmpeg", deadline, env)
        if not result["pcm5s"]:
            result["errorClass"] = "pcm_short_or_deadline"
        return result
    except subprocess.TimeoutExpired:
        result["processMs"] = (time.monotonic() - start) * 1000
        result["errorClass"] = "deadline"
        return result


def verify_assets() -> dict:
    locked = json.loads(LOCK.read_text())
    exe = TOOLS / "yt-dlp"
    digest = hashlib.sha256(exe.read_bytes()).hexdigest()
    expected = locked["ytDlp"]["sha256"]["x86_64-unknown-linux-gnu"]
    if digest != expected:
        raise RuntimeError("bundled yt-dlp digest does not match tool lock")
    return {"ytDlpVersion": locked["ytDlp"]["version"], "ytDlpCommit": locked["ytDlp"]["sourceCommit"],
        "bundledExecutableSha256": digest, "helperVersion": "2.0.2",
        "helperCommit": "26475e9d3665b972c4aeb83b4dbaafcf64b88438",
        "nodeVersion": subprocess.run([str(TOOLS / "node"), "--version"], capture_output=True, text=True).stdout.strip(),
        "python": sys.version.split()[0]}


def checkpoint(path: pathlib.Path, payload: dict):
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    temp.replace(path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--source-root", type=pathlib.Path, required=True,
        help="yt-dlp release tar root containing yt_dlp/")
    parser.add_argument("--source-archive", type=pathlib.Path)
    parser.add_argument("--run-live", action="store_true", help="make exactly three public extractions per arm")
    parser.add_argument("--smoke", action="store_true", help="run one bundled CLI public extraction only")
    parser.add_argument("--mode", choices=("run", "worker"), default="run", help=argparse.SUPPRESS)
    parser.add_argument("--worker-video", help=argparse.SUPPRESS)
    parser.add_argument("--worker-home", type=pathlib.Path, help=argparse.SUPPRESS)
    parser.add_argument("--dependency-root", type=pathlib.Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.mode == "worker":
        os.environ["BURRATA_REUSE_WORKER_GROUP"] = "1"
        gate = GateReference(os.environ["BURRATA_GATE_URL"])
        worker_init = time.monotonic()
        if args.dependency_root:
            sys.path.insert(0, str(args.dependency_root))
        sys.path.insert(0, str(args.source_root))
        import yt_dlp
        from yt_dlp.globals import plugin_dirs
        plugin_dirs.value = [str(BGUTIL)]
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            ydl = yt_dlp.YoutubeDL(ydl_params(gate))
        print(json.dumps({"workerReady": True, "initMs": (time.monotonic() - worker_init) * 1000}), flush=True)
        for line in sys.stdin:
            request = json.loads(line)
            row = run_embedded(request["videoId"], args.source_root, gate, args.worker_home,
                time.monotonic() + min(HARD_LIMIT_SECONDS, request.get("budgetMs", HARD_LIMIT_SECONDS * 1000) / 1000), reused=ydl)
            print(json.dumps(row, separators=(",", ":")), flush=True)
        ydl.close()
        return
    assets = verify_assets()
    src = args.source_root / "yt_dlp"
    if not src.is_dir():
        raise RuntimeError("source root has no yt_dlp module")
    sys.path.insert(0, str(args.source_root))
    import yt_dlp
    if yt_dlp.version.__version__ != assets["ytDlpVersion"]:
        raise RuntimeError("source module version does not match locked binary")
    assets["sourceGitHead"] = yt_dlp.version.RELEASE_GIT_HEAD
    if not assets["sourceGitHead"].startswith(assets["ytDlpCommit"]):
        raise RuntimeError("Python source commit does not match tool lock")
    if args.source_archive:
        assets["sourceArchiveSha256"] = hashlib.sha256(args.source_archive.read_bytes()).hexdigest()
    result = {"schemaVersion": 1, "assets": assets, "protocol": "metadata+selected-audio-URL+5s-PCM",
        "live": args.run_live, "urlLogged": False, "arms": {}}
    if not args.run_live:
        checkpoint(args.output, result)
        print(json.dumps(result, indent=2))
        return
    with tempfile.TemporaryDirectory(prefix="burrata-extractor-reuse-") as scratch:
        tmp = pathlib.Path(scratch)
        try:
            helper, helper_url, helper_ready_ms = run_helper(tmp)
        except Exception as error:
            result["setupError"] = "helper_unavailable"
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            return
        result["helperWarmReadyMs"] = helper_ready_ms
        try:
            # A single helper is shared across arms; every process/worker owns its gate.
            arm_rows = {"bundled_cli_fresh": [], "python_process_fresh_object": [], "python_resident_reused_object": []}
            result["arms"] = arm_rows
            checkpoint(args.output, result)
            # This arm owns one persistent, serial worker, one YoutubeDL object,
            # and one private per-worker lease/token for its complete lifetime.
            worker_home = tmp / "python-resident"
            worker = gate = None
            try:
                # Round robin order makes the primary Python comparison use
                # the same helper state, video, client, and format policy.
                for video_id in VIDEO_IDS[:1] if args.smoke else VIDEO_IDS:
                    cli_gate = Gate(helper_url)
                    proc = None
                    try:
                        start = time.monotonic()
                        cli_row = run_cli(video_id, TOOLS / "yt-dlp", cli_gate,
                            tmp / f"cli-{video_id}", start + HARD_LIMIT_SECONDS)
                    finally:
                        cli_gate.close()
                    arm_rows["bundled_cli_fresh"].append(cli_row)
                    checkpoint(args.output, result)

                    if args.smoke:
                        break

                    py_gate = Gate(helper_url)
                    worker_cmd = [sys.executable, "-S", str(pathlib.Path(__file__).resolve()), "--output", str(args.output),
                        "--source-root", str(args.source_root), "--mode", "worker", "--worker-video", video_id,
                        "--worker-home", str(tmp / f"py-fresh-{video_id}")]
                    py_env = safe_env(tmp / f"py-fresh-{video_id}")
                    py_env["BURRATA_GATE_URL"] = py_gate.base_url
                    py_env["PYTHONPATH"] = str(args.source_root)
                    started = time.monotonic()
                    try:
                        proc = subprocess.Popen(worker_cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, env=py_env, start_new_session=True)
                        try:
                            stdout, _ = proc.communicate((json.dumps({"videoId": video_id}) + "\n").encode(), timeout=HARD_LIMIT_SECONDS)
                        except subprocess.TimeoutExpired:
                            stop_process_group(proc)
                            stdout = b""
                        row = json.loads(stdout.decode().splitlines()[-1]) if stdout.strip() else {"videoId": video_id,
                            "ok": False, "identityMatched": False, "pcm5s": False, "pcmBytes": 0, "errorClass": "worker_error"}
                        row["processMs"] = (time.monotonic() - started) * 1000
                    except Exception:
                        row = {"videoId": video_id, "ok": False, "identityMatched": False, "pcm5s": False,
                            "pcmBytes": 0, "processMs": (time.monotonic() - started) * 1000, "errorClass": "deadline_or_worker_error"}
                    finally:
                        if proc is not None:
                            stop_process_group(proc)
                        py_gate.close()
                    arm_rows["python_process_fresh_object"].append(row)
                    checkpoint(args.output, result)

                    # Start lazily here: CLI/fresh arm timing must not consume
                    # the resident arm's own first-use absolute budget.
                    started = time.monotonic()
                    resident_deadline = started + HARD_LIMIT_SECONDS
                    if worker is None:
                        worker_started = started
                        gate = Gate(helper_url)
                        env = safe_env(worker_home)
                        env["BURRATA_GATE_URL"] = gate.base_url
                        env["PYTHONPATH"] = str(args.source_root)
                        worker = subprocess.Popen([sys.executable, "-S", str(pathlib.Path(__file__).resolve()), "--output", str(args.output),
                            "--source-root", str(args.source_root), "--mode", "worker", "--worker-home", str(worker_home)],
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, start_new_session=True)
                        reader = JsonLineReader(worker.stdout.fileno())
                        ready_line = reader.read_until(min(resident_deadline, time.monotonic() + 5))
                        worker_ready = json.loads(ready_line) if ready_line else {}
                        if not worker_ready.get("workerReady"):
                            arm_rows["python_resident_reused_object"].append({"videoId": video_id,
                                "ok": False, "identityMatched": False, "pcm5s": False, "pcmBytes": 0,
                                "errorClass": "worker_readiness_failed", "requestWallMs": (time.monotonic() - started) * 1000})
                            checkpoint(args.output, result)
                            break
                        result["residentWorkerStartupMs"] = worker_ready.get("initMs")
                        result["residentWorkerReadinessWallMs"] = (time.monotonic() - worker_started) * 1000
                    remaining_ms = max(0, int((resident_deadline - time.monotonic()) * 1000))
                    worker.stdin.write((json.dumps({"videoId": video_id, "budgetMs": remaining_ms}) + "\n").encode())
                    worker.stdin.flush()
                    line = reader.read_until(resident_deadline)
                    row = json.loads(line) if line else {"videoId": video_id, "ok": False, "identityMatched": False,
                        "pcm5s": False, "pcmBytes": 0, "errorClass": "deadline_or_worker_exit"}
                    row["requestWallMs"] = (time.monotonic() - started) * 1000
                    arm_rows["python_resident_reused_object"].append(row)
                    checkpoint(args.output, result)
                    if not line:
                        stop_process_group(worker)
                        break
            finally:
                stop_process_group(worker)
                if gate is not None:
                    gate.close()
        finally:
            stop_process_group(helper)
    result["summary"] = {}
    for name, rows in result["arms"].items():
        valid = [row for row in rows if row.get("identityMatched")]
        result["summary"][name] = {"attempts": len(rows), "identityMatches": len(valid),
            "fiveSecondPcm": sum(bool(row.get("pcm5s")) for row in rows),
            "medianProcessOrRequestMs": statistics.median([row.get("processMs", row.get("requestWallMs", 0)) for row in rows]) if rows else None,
            "medianExtractMs": statistics.median([row["extractMs"] for row in rows if row.get("extractMs") is not None]) if any(row.get("extractMs") is not None for row in rows) else None,
            "medianPcmMs": statistics.median([row["pcmMs"] for row in rows if row.get("pcmMs") is not None]) if any(row.get("pcmMs") is not None for row in rows) else None,
            "failuresByClass": {label: sum(row.get("errorClass") == label for row in rows) for label in sorted({row.get("errorClass") for row in rows if row.get("errorClass")})}}
    checkpoint(args.output, result)
    print(json.dumps({"output": str(args.output), "summary": result["summary"]}, indent=2))


MATCHED_ARMS = ("bundled_cli_fresh", "python_process_fresh_object", "python_resident_reused_object")
MATCHED_SCHEMA = "burrata.extractor-reuse-matched/v2"


class DiagnosticFailure(Exception):
    def __init__(self, code, stage="protocol"):
        self.code, self.stage = code, stage
        super().__init__(code)


def select_resolved_stream(info, expected_video_id, expected_format_id):
    """Never rescore all formats: use exactly the extractor-selected stream."""
    if not isinstance(info, dict) or not info.get("id"):
        raise DiagnosticFailure("IDENTITY_MISSING", "identity")
    if str(info["id"]) != expected_video_id:
        raise DiagnosticFailure("IDENTITY_MISMATCH", "identity")
    if info.get("format_id") is None:
        raise DiagnosticFailure("FORMAT_MISSING", "format")
    if str(info["format_id"]) != str(expected_format_id):
        raise DiagnosticFailure("FORMAT_MISMATCH", "format")
    if not info.get("url") or info.get("acodec") in (None, "none"):
        raise DiagnosticFailure("AUDIO_MISSING", "format")
    parsed = urlsplit(info["url"])
    if (info.get("protocol") not in ("http", "https") or parsed.scheme not in ("http", "https")
            or not parsed.hostname or parsed.username or parsed.password):
        raise DiagnosticFailure("STREAM_PROTOCOL_UNSUPPORTED", "format")
    return info


def _finite_number(value):
    import math
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def selection_fingerprint(info):
    def text(value):
        return value if isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,80}", value) else None
    def number(value):
        return value if _finite_number(value) and value > 0 else None
    return {"formatId": text(str(info.get("format_id", ""))), "acodec": text(info.get("acodec")),
        "ext": text(info.get("ext")), "protocol": text(info.get("protocol")),
        "sampleRate": number(info.get("asr", info.get("samplerate"))),
        "channels": number(info.get("audio_channels")), "contentLength": number(info.get("filesize"))}


def safe_stream_headers(info):
    headers = info.get("http_headers") or {}
    if not isinstance(headers, dict):
        return []
    result = {}
    for name, value in headers.items():
        if not isinstance(name, str) or name.lower() not in ("user-agent", "referer", "origin", "accept-language"):
            continue
        if (not isinstance(value, str) or not value or len(value) > 2048
                or any(ord(char) < 32 or ord(char) == 127 for char in value)):
            continue
        result[name.lower()] = value
    return [(name, result[name]) for name in ("user-agent", "referer", "origin", "accept-language") if name in result]


def build_schedule(repetitions):
    if not isinstance(repetitions, int) or isinstance(repetitions, bool) or not 1 <= repetitions <= 3:
        raise ValueError("exploratory repetitions must be between one and three")
    return [{"pairId": index + 1, "arm": arm}
        for index in range(repetitions) for arm in MATCHED_ARMS[index:] + MATCHED_ARMS[:index]]


def _successful_matched(row):
    return (row.get("ok") is True and row.get("identityMatched") is True and row.get("formatMatched") is True
        and row.get("videoId") == row.get("resolvedVideoId")
        and str(row.get("expectedFormatId")) == str(row.get("formatId"))
        and _finite_number(row.get("requestToFirstPcmMs")) and row["requestToFirstPcmMs"] >= 0)


def summarize_matched(rows):
    result = {"arms": {}, "pairedDeltas": []}
    for arm in MATCHED_ARMS:
        attempts = [row for row in rows if row.get("arm") == arm]
        valid = [row for row in attempts if _successful_matched(row)]
        failures = {}
        for row in attempts:
            if not _successful_matched(row):
                code = row.get("failure") or "INCOMPLETE_MATCHED_RESULT"
                failures[code] = failures.get(code, 0) + 1
        result["arms"][arm] = {"attempts": len(attempts), "successes": len(valid), "failuresByClass": failures,
            "medianRequestToFirstPcmMs": statistics.median(row["requestToFirstPcmMs"] for row in valid) if valid else None,
            "medianRequestWallMs": statistics.median(row["requestWallMs"] for row in valid) if valid else None}
    pair_ids = list(dict.fromkeys(row.get("pairId") for row in rows))
    for pair_id in pair_ids:
        group = {row["arm"]: row for row in rows if row.get("pairId") == pair_id}
        for control, treatment in ((MATCHED_ARMS[0], MATCHED_ARMS[1]), (MATCHED_ARMS[1], MATCHED_ARMS[2])):
            a, b = group.get(control), group.get(treatment)
            reason = None
            if not a or not b:
                reason = "MISSING_ARM"
            elif not _successful_matched(a) or not _successful_matched(b):
                reason = "FAILED_OR_UNMATCHED_ATTEMPT"
            elif (a["videoId"], a["formatId"]) != (b["videoId"], b["formatId"]):
                reason = "SOURCE_OR_FORMAT_CHANGED"
            elif a.get("selectionFingerprint") != b.get("selectionFingerprint"):
                reason = "STREAM_FINGERPRINT_CHANGED"
            elif not a.get("selectionFingerprint") or any(a["selectionFingerprint"].get(key) is None
                    for key in ("acodec", "ext", "protocol", "sampleRate", "channels")):
                reason = "INCOMPLETE_STREAM_FINGERPRINT"
            result["pairedDeltas"].append({"pairId": pair_id, "controlArm": control, "treatmentArm": treatment,
                "included": reason is None, "reason": reason,
                "requestToFirstPcmDeltaMs": b["requestToFirstPcmMs"] - a["requestToFirstPcmMs"] if reason is None else None})
    return result


def _collect_bounded(proc, deadline, stdout_limit=8 * 1024 * 1024, stderr_limit=65536):
    import selectors
    selector = selectors.DefaultSelector()
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    for name in captured:
        pipe = getattr(proc, name)
        if pipe:
            selector.register(pipe, selectors.EVENT_READ, name)
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise DiagnosticFailure("DEADLINE", "extract")
            for key, _ in selector.select(min(remaining, .1)):
                chunk = os.read(key.fd, 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                limit = stdout_limit if key.data == "stdout" else stderr_limit
                if len(captured[key.data]) + len(chunk) > limit:
                    raise DiagnosticFailure("OUTPUT_LIMIT", "extract")
                captured[key.data].extend(chunk)
        proc.wait(timeout=max(.001, deadline - time.monotonic()))
        return bytes(captured["stdout"]), bytes(captured["stderr"]), proc.returncode
    except subprocess.TimeoutExpired:
        raise DiagnosticFailure("DEADLINE", "extract") from None
    finally:
        selector.close()
        stop_process_group(proc)
        for name in captured:
            pipe = getattr(proc, name)
            if pipe:
                pipe.close()


def _matched_pcm(info, deadline, env, in_worker=False):
    """All arms use this exact decoder and the same first-byte boundary."""
    started_ns = time.monotonic_ns()
    headers = safe_stream_headers(info)
    args = [str(TOOLS / "ffmpeg"), "-nostdin", "-v", "error", "-rw_timeout", "7000000"]
    if headers:
        args += ["-headers", "".join(f"{name}: {value}\r\n" for name, value in headers)]
    args += ["-i", info["url"], "-t", "5", "-vn", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:1"]
    proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=env, start_new_session=not in_worker, bufsize=0)
    import selectors
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ, "pcm")
    selector.register(proc.stderr, selectors.EVENT_READ, "stderr")
    frames_bytes = 0
    first_ns = None
    stderr = bytearray()
    wanted = 5 * 48000 * 2 * 2
    failure = None
    completion_ns = None
    try:
        while selector.get_map() and frames_bytes < wanted:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                failure = "DEADLINE"
                break
            for key, _ in selector.select(min(remaining, .1)):
                chunk = os.read(key.fd, 32768)
                now_ns = time.monotonic_ns()
                if not chunk:
                    selector.unregister(key.fileobj)
                elif key.data == "pcm":
                    first_ns = first_ns or now_ns
                    frames_bytes += len(chunk)
                    if frames_bytes >= wanted:
                        completion_ns = now_ns
                elif len(stderr) < 65536:
                    stderr.extend(chunk[:65536 - len(stderr)])
        if frames_bytes < wanted and not failure:
            text = stderr.decode("utf-8", "replace").lower()
            failure = "STREAM_DENIED" if any(value in text for value in ("403", "410", "forbidden")) else "PCM_SHORT"
    finally:
        selector.close()
        if in_worker:
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.wait(timeout=.3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=.5)
        else:
            stop_process_group(proc)
        proc.stdout.close()
        proc.stderr.close()
    return {"pcm5s": frames_bytes >= wanted, "pcmBytes": frames_bytes,
        "firstPcmMonotonicNs": first_ns, "decoderFirstPcmMs": (first_ns - started_ns) / 1e6 if first_ns else None,
        "decoderFiveSecondCompleteMs": (completion_ns - started_ns) / 1e6 if completion_ns else None,
        "decoderWallMs": (time.monotonic_ns() - started_ns) / 1e6, "failure": failure}


def _new_matched_row(video_id, format_id):
    return {"videoId": video_id, "resolvedVideoId": None, "expectedFormatId": format_id, "formatId": None,
        "ok": False, "identityMatched": False, "formatMatched": False, "pcm5s": False, "pcmBytes": 0,
        "requestToFirstPcmMs": None, "failure": None, "failureStage": None, "stages": {}}


class _DiagnosticLogger:
    def __init__(self):
        self.categories = set()

    def debug(self, _message):
        pass

    def warning(self, message):
        category = classify_extractor_failure(RuntimeError(str(message)))["errorClass"]
        if category != "extractor_error":
            self.categories.add(category)

    error = warning


class _DiscardOutput:
    def write(self, value):
        return len(value)

    def flush(self):
        pass


def _matched_params(gate_url, format_id, logger):
    params = ydl_params(GateReference(gate_url))
    params.update({"format": format_id, "logger": logger, "remote_components": set(),
        "socket_timeout": 7, "retries": 0, "extractor_retries": 0})
    return params


def _matched_embedded(ydl, request, deadline):
    row = _new_matched_row(request["videoId"], request["formatId"])
    logger = ydl.params["logger"]
    logger.categories.clear()
    started = time.monotonic_ns()
    try:
        with redirect_stdout(_DiscardOutput()), redirect_stderr(_DiscardOutput()):
            info = ydl.extract_info(f'https://www.youtube.com/watch?v={request["videoId"]}', download=False)
        row["stages"]["extractionMs"] = (time.monotonic_ns() - started) / 1e6
        selected = select_resolved_stream(info, request["videoId"], request["formatId"])
        row.update({"resolvedVideoId": str(info["id"]), "formatId": str(info["format_id"]),
            "identityMatched": True, "formatMatched": True, "selectionFingerprint": selection_fingerprint(selected),
            "headerNames": [name for name, _ in safe_stream_headers(selected)]})
        row.update(_matched_pcm(selected, deadline, os.environ.copy(), in_worker=True))
        row["ok"] = row["pcm5s"] and not row["failure"]
        row["failureStage"] = "decode" if row["failure"] else None
    except DiagnosticFailure as error:
        row.update(failure=error.code, failureStage=error.stage)
    except Exception as error:
        diagnostic = classify_extractor_failure(error)
        row.update(failure=diagnostic["errorClass"].upper(), failureStage="extract", errorType=diagnostic["errorType"])
    finally:
        row["diagnosticCategories"] = sorted(logger.categories)
        row["stages"].setdefault("extractionMs", (time.monotonic_ns() - started) / 1e6)
    return row


def _matched_worker(args):
    for path in (args.dependency_root, args.source_root):
        sys.path.insert(0, str(path))
    import yt_dlp
    from yt_dlp.globals import plugin_dirs
    plugin_dirs.value = [str(BGUTIL)]
    logger = _DiagnosticLogger()
    init_started = time.monotonic_ns()
    with redirect_stdout(_DiscardOutput()), redirect_stderr(_DiscardOutput()):
        ydl = yt_dlp.YoutubeDL(_matched_params(os.environ["BURRATA_MATCHED_GATE_URL"], args.format_id, logger))
    print(json.dumps({"kind": "ready", "epoch": args.worker_epoch,
        "initMs": (time.monotonic_ns() - init_started) / 1e6}), flush=True)
    reader = JsonLineReader(sys.stdin.fileno())
    try:
        while True:
            line = reader.read_until(time.monotonic() + args.idle_seconds)
            if line is None:
                return
            try:
                request = json.loads(line)
                if (request.get("epoch") != args.worker_epoch or request.get("action") != "resolve"
                        or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", request.get("requestId", ""))
                        or request.get("videoId") != args.video_id or request.get("formatId") != args.format_id
                        or not _finite_number(request.get("deadlineMonotonic"))):
                    raise DiagnosticFailure("INVALID_REQUEST")
                deadline = min(request["deadlineMonotonic"], time.monotonic() + args.budget_seconds)
                row = _matched_embedded(ydl, request, deadline) if time.monotonic() < deadline else dict(_new_matched_row(args.video_id, args.format_id), failure="DEADLINE")
                row.update(kind="result", epoch=args.worker_epoch, requestId=request["requestId"])
                print(json.dumps(row), flush=True)
            except Exception:
                print(json.dumps({"kind": "protocol_error", "epoch": args.worker_epoch, "failure": "INVALID_REQUEST"}), flush=True)
                return
    finally:
        with redirect_stdout(_DiscardOutput()), redirect_stderr(_DiscardOutput()):
            ydl.close()


def _matched_cli_command(args, gate):
    # netrc is opt-in; --ignore-config plus absence of --netrc leaves it off.
    # The pinned CLI has no --no-netrc switch (and --version skips validation).
    return [str(TOOLS / "yt-dlp"), "--ignore-config", "--no-cache-dir", "--no-warnings", "--quiet",
        "--no-playlist", "--no-remote-components", "--no-plugin-dirs", "--no-js-runtimes",
        "--format", args.format_id, "--socket-timeout", "7", "--retries", "0", "--extractor-retries", "0",
        "--dump-single-json", "--skip-download", "--extractor-args", "youtube:player_client=mweb",
        *Gate.args.fget(gate)]


def _matched_cli(args, gate, scratch, deadline):
    row = _new_matched_row(args.video_id, args.format_id)
    started = time.monotonic_ns()
    command = _matched_cli_command(args, gate) + ["--", f"https://www.youtube.com/watch?v={args.video_id}"]
    try:
        proc = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=safe_env(scratch), start_new_session=True)
        stdout, stderr, status = _collect_bounded(proc, deadline)
        row["stages"]["extractionMs"] = (time.monotonic_ns() - started) / 1e6
        if status:
            diagnostic = classify_extractor_failure(RuntimeError(stderr.decode("utf-8", "replace")))
            raise DiagnosticFailure(diagnostic["errorClass"].upper(), "extract")
        try:
            info = json.loads(stdout)
        except (ValueError, UnicodeDecodeError):
            raise DiagnosticFailure("INVALID_METADATA", "extract") from None
        selected = select_resolved_stream(info, args.video_id, args.format_id)
        row.update({"resolvedVideoId": str(info["id"]), "formatId": str(info["format_id"]),
            "identityMatched": True, "formatMatched": True, "selectionFingerprint": selection_fingerprint(selected),
            "headerNames": [name for name, _ in safe_stream_headers(selected)]})
        row.update(_matched_pcm(selected, deadline, safe_env(scratch)))
        row["ok"] = row["pcm5s"] and not row["failure"]
        row["failureStage"] = "decode" if row["failure"] else None
    except DiagnosticFailure as error:
        row.update(failure=error.code, failureStage=error.stage)
    return row


class _MatchedResident:
    def __init__(self, args, helper_url, scratch):
        self.args, self.helper_url, self.scratch = args, helper_url, scratch
        self.proc = self.gate = self.reader = None
        self.epoch = None

    def close(self):
        stop_process_group(self.proc)
        if self.gate:
            self.gate.close()
        self.proc = self.gate = self.reader = None

    def resolve(self, deadline, request_id):
        startup_ms = 0
        restarted = False
        try:
            if self.proc and self.proc.poll() is not None:
                self.close()
                restarted = True
            if self.proc is None:
                started = time.monotonic_ns()
                self.gate = Gate(self.helper_url)
                self.epoch = uuid.uuid4().hex
                env = safe_env(self.scratch)
                env["BURRATA_MATCHED_GATE_URL"] = self.gate.base_url
                command = [sys.executable, "-S", str(pathlib.Path(__file__).resolve()), "--matched-worker",
                    "--source-root", str(self.args.source_root), "--dependency-root", str(self.args.dependency_root),
                    "--video-id", self.args.video_id, "--format-id", self.args.format_id,
                    "--worker-epoch", self.epoch, "--idle-seconds", str(self.args.idle_seconds),
                    "--budget-seconds", str(self.args.budget_seconds)]
                self.proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, env=env, start_new_session=True, bufsize=0)
                self.reader = JsonLineReader(self.proc.stdout.fileno())
                line = self.reader.read_until(deadline)
                ready = json.loads(line) if line else {}
                if ready.get("kind") != "ready" or ready.get("epoch") != self.epoch:
                    raise DiagnosticFailure("WORKER_READINESS_FAILED", "worker")
                startup_ms = (time.monotonic_ns() - started) / 1e6
            request = {"action": "resolve", "epoch": self.epoch, "requestId": request_id,
                "videoId": self.args.video_id, "formatId": self.args.format_id, "deadlineMonotonic": deadline}
            self.proc.stdin.write((json.dumps(request) + "\n").encode())
            self.proc.stdin.flush()
            line = self.reader.read_until(deadline)
            response = json.loads(line) if line else {}
            if (response.get("kind") != "result" or response.get("epoch") != self.epoch
                    or response.get("requestId") != request_id):
                raise DiagnosticFailure("DEADLINE" if time.monotonic() >= deadline else "WORKER_PROTOCOL_FAILED", "worker")
            for key in ("kind", "epoch", "requestId"):
                response.pop(key, None)
            response["stages"]["workerReadinessMs"] = startup_ms
            response["residentRestarted"] = restarted
            return response
        except (DiagnosticFailure, ValueError, OSError) as error:
            self.close()
            row = _new_matched_row(self.args.video_id, self.args.format_id)
            row.update(failure=error.code if isinstance(error, DiagnosticFailure) else "WORKER_PROTOCOL_FAILED",
                failureStage="worker", stages={"workerReadinessMs": startup_ms})
            return row


def _verify_matched_assets(args):
    assets = verify_assets()
    version = runpy.run_path(str(args.source_root / "yt_dlp/version.py"))
    if version.get("__version__") != assets["ytDlpVersion"] or not version.get("RELEASE_GIT_HEAD", "").startswith(assets["ytDlpCommit"]):
        raise DiagnosticFailure("SOURCE_PIN_MISMATCH", "setup")
    assets["sourceGitHead"] = version["RELEASE_GIT_HEAD"]
    if args.source_archive:
        assets["sourceArchiveSha256"] = hashlib.sha256(args.source_archive.read_bytes()).hexdigest()
        if assets["sourceArchiveSha256"] != "f983c6d3ded7148de5b8b22a7db41e12cef05f38f1b5396985d969593efad059":
            raise DiagnosticFailure("SOURCE_ARCHIVE_PIN_MISMATCH", "setup")
    extras = {path.name for path in args.dependency_root.iterdir()} - {"yt_dlp_ejs"}
    if extras:
        raise DiagnosticFailure("DEPENDENCY_SCOPE_MISMATCH", "setup")
    vendor = runpy.run_path(str(args.source_root / "yt_dlp/extractor/youtube/jsc/_builtin/vendor/_info.py"))
    package = args.dependency_root / "yt_dlp_ejs"
    script_hashes = {}
    for kind in ("core", "lib"):
        path = package / f"yt/solver/{kind}.min.js"
        digest = hashlib.sha3_512(path.read_bytes()).hexdigest()
        if digest != vendor["HASHES"][f"yt.solver.{kind}.min.js"]:
            raise DiagnosticFailure("EJS_PIN_MISMATCH", "setup")
        script_hashes[kind] = digest
    assets.update(ejsVersion=vendor["VERSION"], ejsSolverSha3_512=script_hashes,
        dependencyFilesSha256={str(path.relative_to(args.dependency_root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(package.rglob("*")) if path.is_file() and "__pycache__" not in path.parts},
        scriptSha256=hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),
        frozenCliRuntimeParity=False,
        pythonDependencyPolicy="python-S; pinned-source plus explicit EJS package; no site/user packages")
    source_manifest = hashlib.sha256()
    for path in sorted((args.source_root / "yt_dlp").rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts:
            source_manifest.update(str(path.relative_to(args.source_root)).encode() + b"\0")
            source_manifest.update(hashlib.sha256(path.read_bytes()).digest())
    assets["sourceTreeManifestSha256"] = source_manifest.hexdigest()
    # --version exits before the parser rejects unknown switches. Exercise the
    # actual complete option set through a local, network-free action instead.
    with tempfile.TemporaryDirectory(prefix="burrata-matched-cli-preflight-") as scratch:
        command = _matched_cli_command(args, GateReference("http://127.0.0.1:9/offline-preflight"))
        try:
            proc = subprocess.Popen(command + ["--list-extractors"], stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=safe_env(pathlib.Path(scratch)), start_new_session=True)
            _, stderr, status = _collect_bounded(proc, time.monotonic() + 12)
        except DiagnosticFailure:
            raise DiagnosticFailure("CLI_PREFLIGHT_FAILED", "setup") from None
        if status:
            diagnostic = classify_extractor_failure(RuntimeError(stderr.decode("utf-8", "replace")))
            raise DiagnosticFailure(diagnostic["errorClass"].upper(), "setup")
    assets["cliOptionPreflight"] = "full-option-set --list-extractors; no URL/network"
    return assets


def matched_main():
    parser = argparse.ArgumentParser(description="Same-ID/itag extractor experiment; diagnostic only.")
    parser.add_argument("--matched", action="store_true")
    parser.add_argument("--matched-worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--source-root", type=pathlib.Path, required=True)
    parser.add_argument("--source-archive", type=pathlib.Path)
    parser.add_argument("--dependency-root", type=pathlib.Path, required=True)
    parser.add_argument("--video-id", default="uZGRATckoBM")
    parser.add_argument("--format-id", default="251")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--budget-seconds", type=float, default=20)
    parser.add_argument("--total-budget-seconds", type=float, default=240)
    parser.add_argument("--idle-seconds", type=float, default=60)
    parser.add_argument("--worker-epoch", default="", help=argparse.SUPPRESS)
    parser.add_argument("--run-live", action="store_true")
    args = parser.parse_args()
    if (not re.fullmatch(r"[A-Za-z0-9_-]{11}", args.video_id) or not re.fullmatch(r"[0-9]{1,6}", args.format_id)
            or not 1 <= args.repetitions <= 3 or not 1 <= args.budget_seconds <= 30
            or not 1 <= args.idle_seconds <= 120 or not 10 <= args.total_budget_seconds <= 300):
        parser.error("invalid identity, format, repetition or bounded budget")
    if args.matched_worker:
        _matched_worker(args)
        return
    if not args.output:
        parser.error("--output is required")
    if args.output.exists():
        parser.error("output already exists; preserve previous raw artifacts")
    assets = _verify_matched_assets(args)
    report = {"schema": MATCHED_SCHEMA, "qualification": False, "complete": False, "live": args.run_live,
        "assets": assets, "videoId": args.video_id, "expectedFormatId": args.format_id,
        "client": "mweb", "warmups": 0, "repetitionsPerArm": args.repetitions,
        "requestBudgetSeconds": args.budget_seconds, "totalBudgetSeconds": args.total_budget_seconds,
        "residentIdleSeconds": args.idle_seconds, "helperPolicy": "one shared anonymous warm helper; setup excluded from arm timings",
        "timingScope": "parent request start through first decoder PCM; same monotonic clock in child; worker startup included when needed",
        "pcmScope": "FFmpeg decoder only; five seconds48kHz stereo s16le; no mixer/device",
        "accountingComplete": False, "limitations": ["frozen CLI and experimental Python have different runtime/dependency graphs",
            "itag/source equality is not exact signedURL/CDN equality", "small Latin rotation; no performance qualification",
            "helper readiness excluded; no whole-application cold-start claim"], "attempts": []}
    checkpoint(args.output, report)
    if not args.run_live:
        print(json.dumps({"output": str(args.output), "live": False, "assetsVerified": True}))
        return
    def terminate_requested(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, terminate_requested)
    started = time.monotonic()
    overall_deadline = started + args.total_budget_seconds
    helper = resident = None
    try:
        with tempfile.TemporaryDirectory(prefix="burrata-matched-extractor-") as scratch_name:
            scratch = pathlib.Path(scratch_name)
            helper, helper_url, ready_ms = run_helper(scratch)
            report["helperSetupMs"] = ready_ms
            resident = _MatchedResident(args, helper_url, scratch / "resident")
            for entry in build_schedule(args.repetitions):
                begin_ns = time.monotonic_ns()
                deadline = min(overall_deadline, time.monotonic() + args.budget_seconds)
                gate = None
                fresh = None
                try:
                    if time.monotonic() >= deadline:
                        raise DiagnosticFailure("TOTAL_DEADLINE", "request")
                    if entry["arm"] == MATCHED_ARMS[0]:
                        gate = Gate(helper_url)
                        row = _matched_cli(args, gate, scratch / f'cli-{entry["pairId"]}', deadline)
                    else:
                        target = resident
                        if entry["arm"] == MATCHED_ARMS[1]:
                            fresh = target = _MatchedResident(args, helper_url, scratch / f'fresh-{entry["pairId"]}')
                        row = target.resolve(deadline, f'pair-{entry["pairId"]}-{entry["arm"]}')
                    first_ns = row.pop("firstPcmMonotonicNs", None)
                    row["requestToFirstPcmMs"] = (first_ns - begin_ns) / 1e6 if isinstance(first_ns, int) else None
                    row["requestWallMs"] = (time.monotonic_ns() - begin_ns) / 1e6
                except DiagnosticFailure as error:
                    row = _new_matched_row(args.video_id, args.format_id)
                    row.update(failure=error.code, failureStage=error.stage, requestWallMs=(time.monotonic_ns() - begin_ns) / 1e6)
                finally:
                    if fresh:
                        fresh.close()
                    if gate:
                        gate.close()
                row.update(entry)
                row["warmup"] = False
                report["attempts"].append(row)
                report["summary"] = summarize_matched(report["attempts"])
                checkpoint(args.output, report)
            report["complete"] = True
    except KeyboardInterrupt:
        report["runFailure"] = "CANCELLED"
    except Exception as error:
        report["runFailure"] = error.code if isinstance(error, DiagnosticFailure) else "HARNESS_FAILED"
    finally:
        if resident:
            resident.close()
        stop_process_group(helper)
        report["helperShutdownObserved"] = helper is None or helper.poll() is not None
        report["wallMs"] = (time.monotonic() - started) * 1000
        report["summary"] = summarize_matched(report["attempts"])
        checkpoint(args.output, report)
    print(json.dumps({"output": str(args.output), "complete": report["complete"], "summary": report["summary"]}))


if __name__ == "__main__":
    matched_main() if "--matched" in sys.argv or "--matched-worker" in sys.argv else main()
