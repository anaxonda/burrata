#!/usr/bin/env python3
"""Probe the four Windows media source archives in build order, without caching."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.request


USER_AGENT = "Burrata-release-tool-preparer"
LOCKED_COMMIT = "2f2f1a0a3f4353e0d997afe2328a3fc9682987fa"
ARCHIVE_ORDER = ("ffmpeg", "lame", "opus", "zlib")
REQUEST_TIMEOUT_SECONDS = 30


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verified_result(path: pathlib.Path, expected: str) -> dict[str, object]:
    if not path.is_file():
        return {"transport": "completed", "verified": False, "category": "no_output_file"}
    actual = sha256(path)
    size = path.stat().st_size
    return {
        "transport": "completed",
        "bytes": size,
        "sha256": actual,
        "verified": size > 0 and actual == expected,
        "category": "verified" if size > 0 and actual == expected else "sha256_mismatch",
    }


def urllib_download(url: str, expected: str, destination: pathlib.Path) -> dict[str, object]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT_SECONDS) as response:
            with destination.open("wb") as output:
                shutil.copyfileobj(response, output)
        return verified_result(destination, expected)
    except urllib.error.HTTPError as exc:
        destination.unlink(missing_ok=True)
        return {"transport": "failed", "category": f"http_{exc.code}", "exceptionType": type(exc).__name__}
    except Exception as exc:  # report exception classes, never headers, URLs, or proxy details
        destination.unlink(missing_ok=True)
        reason = getattr(exc, "reason", None)
        classes = {type(exc).__name__, type(reason).__name__ if reason is not None else ""}
        if "SSLEOFError" in classes:
            category = "tls_unexpected_eof"
        elif any(name in classes for name in ("SSLError", "SSLZeroReturnError")):
            category = "tls_error"
        elif any(name in classes for name in ("TimeoutError", "TimeoutExpired")):
            category = "timeout"
        elif isinstance(exc, urllib.error.URLError):
            category = "url_transport_error"
        else:
            category = "download_error"
        return {
            "transport": "failed",
            "category": category,
            "exceptionType": type(exc).__name__,
            "reasonType": type(reason).__name__ if reason is not None else None,
        }


def curl_error_category(stderr: str) -> str:
    value = stderr.lower()
    if "unexpected eof" in value:
        return "tls_unexpected_eof"
    if any(marker in value for marker in ("ssl", "tls", "schannel", "certificate")):
        return "tls_error"
    status = re.search(r"returned error:\s*(\d{3})", value)
    if status:
        return f"http_{status.group(1)}"
    if "timed out" in value or "timeout" in value:
        return "timeout"
    return "curl_transport_error"


def curl_download(url: str, expected: str, destination: pathlib.Path) -> dict[str, object]:
    windir = os.environ.get("WINDIR")
    curl = pathlib.Path(windir, "System32", "curl.exe") if windir else None
    if curl is None or not curl.is_file():
        return {"transport": "unavailable", "verified": False, "category": "system_curl_not_found"}
    command = [
        str(curl), "--fail", "--location", "--max-redirs", "10", "--proto", "=https",
        "--proto-redir", "=https", "--silent", "--show-error", "--user-agent", USER_AGENT,
        "--connect-timeout", "10", "--max-time", str(REQUEST_TIMEOUT_SECONDS),
        "--output", str(destination), url,
    ]
    try:
        result = subprocess.run(
            command,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=REQUEST_TIMEOUT_SECONDS + 2,
        )
    except subprocess.TimeoutExpired:
        destination.unlink(missing_ok=True)
        return {"transport": "failed", "category": "timeout", "exceptionType": "TimeoutExpired"}
    except OSError as exc:
        destination.unlink(missing_ok=True)
        return {"transport": "failed", "category": "curl_launch_error", "exceptionType": type(exc).__name__}
    if result.returncode != 0:
        category = curl_error_category(result.stderr)
        destination.unlink(missing_ok=True)
        return {"transport": "failed", "category": category, "exitCode": result.returncode}
    output = verified_result(destination, expected)
    output["exitCode"] = result.returncode
    return output


def archive_specs(lock: dict[str, object]) -> list[dict[str, str]]:
    ffmpeg = lock["ffmpeg"]["source"]
    specs = [
        {"key": "ffmpeg", "archive": ffmpeg["archive"], "url": ffmpeg["url"], "sha256": ffmpeg["sha256"]},
        {"key": "lame", "archive": "lame-3.100.tar.gz", "url": lock["lame"]["url"], "sha256": lock["lame"]["sha256"]},
        {"key": "opus", "archive": "opus-1.6.1.tar.gz", "url": lock["opus"]["url"], "sha256": lock["opus"]["sha256"]},
        {"key": "zlib", "archive": "zlib-1.3.2.tar.gz", "url": lock["zlib"]["url"], "sha256": lock["zlib"]["sha256"]},
    ]
    if tuple(spec["key"] for spec in specs) != ARCHIVE_ORDER:
        raise SystemExit("locked archive probe order differs from Windows build order")
    for spec in specs:
        digest = spec["sha256"].lower()
        if len(digest) != 64 or any(ch not in "0123456789abcdef" for ch in digest):
            raise SystemExit(f"invalid pinned SHA-256 for {spec['key']}")
        if not spec["url"].startswith("https://"):
            raise SystemExit(f"pinned {spec['key']} source must use HTTPS")
        spec["sha256"] = digest
    return specs


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locked-tools", required=True, type=pathlib.Path)
    args = parser.parse_args()
    lock = json.loads(args.locked_tools.read_text(encoding="utf-8"))
    specs = archive_specs(lock)
    results: list[dict[str, object]] = []

    with tempfile.TemporaryDirectory(prefix="burrata-windows-source-probe-") as temp:
        root = pathlib.Path(temp)
        for spec in specs:
            key = spec["key"]
            label = {"archive": spec["archive"], "expectedSha256": spec["sha256"]}
            item: dict[str, object] = {**label, "methods": {}}
            # Emit before network I/O and flush so a timeout still records which archive began.
            print(json.dumps({"event": "archive_start", **label}, sort_keys=True), flush=True)
            print(json.dumps({"event": "method_start", "archive": spec["archive"], "method": "pythonUrllib"}, sort_keys=True), flush=True)
            urllib_result = urllib_download(spec["url"], spec["sha256"], root / f"{key}-urllib")
            item["methods"]["pythonUrllib"] = urllib_result
            print(json.dumps({"event": "method_result", "archive": spec["archive"], "method": "pythonUrllib", **urllib_result}, sort_keys=True), flush=True)

            # Retain the earlier FFmpeg transport comparison as a verified control.
            # For other archives, use curl only when urllib fails; this limits duplicate traffic.
            if key == "ffmpeg" or urllib_result.get("verified") is not True:
                print(json.dumps({"event": "method_start", "archive": spec["archive"], "method": "windowsSystemCurl"}, sort_keys=True), flush=True)
                curl_result = curl_download(spec["url"], spec["sha256"], root / f"{key}-curl")
                item["methods"]["windowsSystemCurl"] = curl_result
                print(json.dumps({"event": "method_result", "archive": spec["archive"], "method": "windowsSystemCurl", **curl_result}, sort_keys=True), flush=True)
            else:
                item["methods"]["windowsSystemCurl"] = {"transport": "not_attempted", "reason": "urllib_verified"}
            item["verified"] = any(result.get("verified") is True for result in item["methods"].values())
            results.append(item)
            print(json.dumps({"event": "archive_result", **item}, sort_keys=True), flush=True)

    integrity_failure = any(
        method.get("category") == "sha256_mismatch"
        for archive in results
        for method in archive["methods"].values()
    )
    all_verified = len(results) == len(ARCHIVE_ORDER) and all(item["verified"] for item in results)
    outcome = "integrity_mismatch" if integrity_failure else ("all_archives_verified" if all_verified else "archive_unverified")
    print(json.dumps({
        "diagnostic": "windows-locked-media-source-download-sequence",
        "sourceCommit": LOCKED_COMMIT,
        "archiveOrder": [item["archive"] for item in results],
        "results": results,
        "outcome": outcome,
        "requestTimeoutSeconds": REQUEST_TIMEOUT_SECONDS,
        "urllibRedirectPolicy": "production urllib default redirect handlers (may follow an HTTP redirect)",
        "curlRedirectPolicy": "HTTPS-only initial URL and redirects",
        "proxyEnvironmentOrHeadersLogged": False,
        "cacheWritesPerformed": False,
        "temporaryFilesRemoved": True,
    }, sort_keys=True), flush=True)
    return 2 if integrity_failure else (0 if all_verified else 1)


if __name__ == "__main__":
    raise SystemExit(main())
