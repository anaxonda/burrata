#!/usr/bin/env python3
"""Install a native Burrata installer and smoke-test its packaged runtime."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import signal
import math
import plistlib
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import uuid


TARGETS = {
    "linux": "x86_64-unknown-linux-gnu",
    "darwin": "aarch64-apple-darwin",
    "win32": "x86_64-pc-windows-msvc",
}
ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "src-tauri" / "src" / "schema.sql"


class SmokeFailure(RuntimeError):
    def __init__(self, stage: str, reason: str):
        super().__init__(reason)
        self.stage = stage


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


def build_webview_audio_fixture(resource_root: pathlib.Path, target: str,
                                env: dict[str, str], temp: pathlib.Path,
                                app_data: pathlib.Path) -> dict[str, str | float]:
    """Generate and register a short AAC asset for installed WebView playback."""
    ext = ".exe" if sys.platform == "win32" else ""
    tools = resource_root / "tools" / target
    ffmpeg = tools / ("ffmpeg" + ext)
    ffprobe = tools / ("ffprobe" + ext)
    if not ffmpeg.is_file() or not ffprobe.is_file():
        raise SmokeFailure("WebView audio setup", "packaged AAC encoder or probe is missing")
    fixture_dir = temp / "webview-audio-fixture"
    fixture_dir.mkdir()
    audio = fixture_dir / "installed-smoke.m4a"
    run([ffmpeg, "-nostdin", "-hide_banner", "-v", "error", "-f", "lavfi",
         "-i", "sine=frequency=440:duration=4:sample_rate=44100", "-ac", "2",
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
        raise SmokeFailure("WebView audio setup", "packaged ffprobe returned invalid AAC fixture metadata") from error
    if codec != "aac":
        raise SmokeFailure("WebView audio setup", "packaged FFmpeg did not encode AAC")
    fixture = register_webview_audio_fixture(app_data, audio, duration, temp)
    return {"trackId": fixture["trackId"], "codec": codec, "mimeType": "audio/mp4", "durationSeconds": fixture["durationSeconds"],
            "registered": "isolated local asset"}


def register_webview_audio_fixture(app_data: pathlib.Path, audio_path: pathlib.Path,
                                   duration_seconds: float, temp_root: pathlib.Path) -> dict[str, str | float]:
    """Register one generated AAC file in the smoke-owned, isolated app database."""
    root = temp_root.resolve(strict=True)
    path = audio_path.resolve(strict=True)
    if not path.is_relative_to(root) or path.suffix.lower() != ".m4a" or not path.is_file():
        raise SmokeFailure("WebView audio setup", "AAC fixture must be a temporary smoke-owned M4A file")
    if not math.isfinite(duration_seconds) or not 3.5 <= duration_seconds <= 5.0:
        raise SmokeFailure("WebView audio setup", "generated AAC fixture duration is outside the smoke bound")

    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    track_id = f"local:{digest}"
    asset_id = str(uuid.uuid4())
    title = "Burrata Installed WebView Audio Fixture"
    artist = "Burrata Test"
    track = {
        "id": track_id, "provider": "local", "providerId": digest,
        "sourceUrl": f"local:{digest}", "artist": artist, "title": title,
        "version": "Smoke AAC", "album": "Installed WebView Smoke",
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
        raise SmokeFailure("WebView audio setup", "smoke app database already exists")
    app_data.mkdir(parents=True, exist_ok=True)
    try:
        with sqlite3.connect(database) as connection:
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
                raise SmokeFailure("WebView audio setup", "seeded smoke database failed integrity check")
    except (OSError, sqlite3.Error) as error:
        raise SmokeFailure("WebView audio setup", "could not register the local AAC fixture") from error
    return {"trackId": track_id, "durationSeconds": duration_seconds, "codec": "aac", "mimeType": "audio/mp4"}


def webview_audio_failure_category(error_code: int | None) -> str | None:
    if error_code is None:
        return None
    return {1: "load_aborted", 2: "source_failure", 3: "decode_failure", 4: "unsupported_media"}.get(
        error_code, "unknown_media_failure")


def assess_webview_audio_progress(track_id: str, first: dict, second: dict) -> dict[str, float | str]:
    samples = (first, second)
    for sample in samples:
        if sample.get("engine") != "webview" or sample.get("applicable") is not True:
            raise SmokeFailure("WebView audio", "installed app did not expose WebView media inspection")
        if sample.get("trackId") != track_id or sample.get("queueTrackId") != track_id:
            raise SmokeFailure("WebView audio", "playback sample did not belong to the smoke fixture")
        if sample.get("playbackStatus") != "playing" or sample.get("queueTotal") != 1:
            raise SmokeFailure("WebView audio", "isolated playback queue was not the single playing fixture")
        if not isinstance(sample.get("entryId"), str) or not sample["entryId"]:
            raise SmokeFailure("WebView audio", "playback queue entry identity was missing")
        if sample.get("previewKind") != "local":
            raise SmokeFailure("WebView audio", "fixture did not use the registered local-audio route")
        media = sample.get("media")
        if not isinstance(media, dict) or media.get("present") is not True:
            raise SmokeFailure("WebView audio", "WebView audio element was not present")
        category = webview_audio_failure_category(media.get("errorCode"))
        if category:
            raise SmokeFailure("WebView audio", f"media element reported {category}")
        if media.get("sourceStable") is not True or media.get("paused") is not False:
            raise SmokeFailure("WebView audio", "audio source was unstable or playback was paused")
        if not isinstance(media.get("readyState"), (int, float)) or media["readyState"] < 2:
            raise SmokeFailure("WebView audio", "audio element did not reach a playable ready state")
        if not isinstance(media.get("duration"), (int, float)) or not 3.5 <= media["duration"] <= 5.0:
            raise SmokeFailure("WebView audio", "audio element reported an invalid fixture duration")
        if not isinstance(media.get("currentTime"), (int, float)) or not math.isfinite(media["currentTime"]):
            raise SmokeFailure("WebView audio", "audio element clock was unavailable")
    if first["entryId"] != second["entryId"] or first.get("sourceGeneration") != second.get("sourceGeneration"):
        raise SmokeFailure("WebView audio", "playback identity changed between media samples")
    advance = second["media"]["currentTime"] - first["media"]["currentTime"]
    if advance < 0.5:
        raise SmokeFailure("WebView audio", "WebView audio clock did not advance by 0.5 seconds")
    return {"engine": "webview", "previewKind": "local", "clockAdvanceSeconds": round(advance, 3)}


def run_mcp_webview_audio(resource_root: pathlib.Path, target: str,
                          discovery: pathlib.Path, env: dict[str, str],
                          track_id: str) -> dict:
    node = shutil.which("node")
    ext = ".exe" if sys.platform == "win32" else ""
    mcp = resource_root / "tools" / target / ("burrata-mcp" + ext)
    if not node or not mcp.is_file():
        raise SmokeFailure("WebView audio", "installed MCP launcher or Node.js is unavailable")
    result = run([node, ROOT / "scripts/release/check-installed-webview-audio.mjs",
                  mcp, discovery, track_id], timeout=30, env=env, check=False)
    try:
        payload = json.loads(result.stdout.decode().strip().splitlines()[-1])
        if payload.get("status") != "sampled":
            category = payload.get("failureCategory")
            messages = {
                "source_failure": "loopback audio source could not be loaded",
                "unsupported_media": "WebView does not support the generated AAC asset",
                "decode_failure": "WebView failed to decode the generated AAC asset",
                "mcp_failure": "installed MCP playback check failed",
                "unsupported_smoke_tools": "installed MCP is missing a required smoke tool",
                "smoke_permissions_unavailable": "isolated smoke MCP permissions are unavailable",
                "webview_engine_unavailable": "installed app did not expose applicable WebView media",
                "wrong_playback_track": "installed WebView played a different track",
                "audio_not_ready": "WebView media did not become ready before the deadline",
                "audio_state_not_playing": "WebView media was not playing during the second sample",
            }
            detail = messages.get(category, "installed WebView audio check did not complete")
            if category in messages:
                detail = f"{detail} (category={category})"
            raise SmokeFailure("WebView audio", detail)
        if result.returncode != 0:
            raise SmokeFailure("WebView audio", "installed MCP playback checker exited unsuccessfully")
        samples = payload
        return assess_webview_audio_progress(track_id, samples["first"], samples["second"])
    except (ValueError, IndexError, KeyError, TypeError) as error:
        raise SmokeFailure("WebView audio", "installed MCP returned invalid audio evidence") from error


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

        if sys.platform == "darwin":
            evidence["checks"]["transportSecurity"] = macos_transport_security_check(app)

        target = TARGETS[sys.platform]
        evidence["checks"]["packagedTools"] = bundled_tool_checks(resource_root, target, env)
        evidence["checks"]["mediaFixtures"] = bundled_media_checks(resource_root, target, env, temp)
        audio_fixture = build_webview_audio_fixture(resource_root, target, env, temp, app_data)
        evidence["checks"]["webviewAudioFixture"] = {
            key: value for key, value in audio_fixture.items() if key != "trackId"
        }
        env["BURRATA_PLAYBACK_ENGINE"] = "webview"
        app_process = launch_app(app, env, temp)
        discovery = app_data / "automation.json"
        wait_for_app(app_process, discovery, temp / "installed-app.stderr.log")
        evidence["checks"]["guiStartup"] = "automation discovery published"
        evidence["checks"]["nativeHost"] = native_host_check(resource_root, target, env)
        evidence["checks"]["mcp"] = run_mcp_status(resource_root, target, discovery, env)
        evidence["checks"]["webviewAudio"] = run_mcp_webview_audio(
            resource_root, target, discovery, env, str(audio_fixture["trackId"])
        )
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
