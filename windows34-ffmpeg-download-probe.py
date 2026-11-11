#!/usr/bin/env python3
"""Compare Windows urllib and system curl against the frozen FFmpeg lock."""
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
        "verified": actual == expected and size > 0,
        "category": "verified" if actual == expected and size > 0 else "sha256_mismatch",
    }


def urllib_download(url: str, expected: str, destination: pathlib.Path) -> dict[str, object]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            with destination.open("wb") as output:
                shutil.copyfileobj(response, output)
        return verified_result(destination, expected)
    except urllib.error.HTTPError as exc:
        destination.unlink(missing_ok=True)
        return {
            "transport": "failed",
            "category": f"http_{exc.code}",
            "exceptionType": type(exc).__name__,
        }
    except Exception as exc:  # report classes only; never stringify headers or proxy errors
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
        str(curl),
        "--fail",
        "--location",
        "--max-redirs", "10",
        "--proto", "=https",
        "--proto-redir", "=https",
        "--silent",
        "--show-error",
        "--user-agent", USER_AGENT,
        "--connect-timeout", "20",
        "--max-time", "120",
        "--output", str(destination),
        url,
    ]
    try:
        result = subprocess.run(
            command,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=130,
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locked-tools", required=True, type=pathlib.Path)
    args = parser.parse_args()
    lock = json.loads(args.locked_tools.read_text(encoding="utf-8"))
    source = lock["ffmpeg"]["source"]
    expected = source["sha256"].lower()
    if len(expected) != 64 or any(ch not in "0123456789abcdef" for ch in expected):
        raise SystemExit("invalid pinned SHA-256 in locked-tools.json")
    if not source["url"].startswith("https://"):
        raise SystemExit("pinned FFmpeg source must use HTTPS")

    with tempfile.TemporaryDirectory(prefix="burrata-ffmpeg-probe-") as temp:
        root = pathlib.Path(temp)
        urllib_result = urllib_download(source["url"], expected, root / "urllib-download")
        curl_result = curl_download(source["url"], expected, root / "curl-download")

    verified = [
        method for method, result in (("urllib", urllib_result), ("curl", curl_result))
        if result.get("verified") is True
    ]
    integrity_failure = any(result.get("category") == "sha256_mismatch" for result in (urllib_result, curl_result))
    if integrity_failure:
        outcome = "integrity_mismatch"
    elif verified == ["curl"]:
        outcome = "curl_verified_urllib_failed"
    elif verified == ["urllib"]:
        outcome = "urllib_verified_curl_failed"
    elif len(verified) == 2:
        outcome = "both_verified"
    else:
        outcome = "neither_transport_verified"

    print(json.dumps({
        "diagnostic": "pinned-ffmpeg-download-transport-comparison",
        "sourceCommit": LOCKED_COMMIT,
        "archive": source["archive"],
        "url": source["url"],
        "expectedSha256": expected,
        "requestsPerMethod": 1,
        "redirectPolicy": {
            "pythonUrllib": "production urllib default redirect handlers (may follow an HTTP redirect)",
            "windowsSystemCurl": "HTTPS-only initial URL and redirects",
        },
        "methods": {"pythonUrllib": urllib_result, "windowsSystemCurl": curl_result},
        "outcome": outcome,
        "proxyEnvironmentOrHeadersLogged": False,
        "temporaryFilesRemoved": True,
    }, sort_keys=True))
    return 2 if integrity_failure else (0 if verified else 1)


if __name__ == "__main__":
    raise SystemExit(main())
