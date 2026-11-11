#!/usr/bin/env python3
"""Probe the locked Opus archive from Xiph and two pinned mirror candidates."""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import tempfile
import time
import urllib.error
import urllib.request
from urllib.parse import urlparse


LOCKED_COMMIT = "2f2f1a0a3f4353e0d997afe2328a3fc9682987fa"
ARCHIVE = "opus-1.6.1.tar.gz"
REQUEST_TIMEOUT_SECONDS = 30
USER_AGENT = "Burrata-release-tool-preparer"
SOURCES = (
    {
        "sourceId": "xiph_official_primary",
        "url": "https://downloads.xiph.org/releases/opus/opus-1.6.1.tar.gz",
        "provenance": "Official stable-release download linked from opus-codec.org/downloads/.",
    },
    {
        "sourceId": "osuosl_xiph_mirror",
        "url": "https://ftp.osuosl.org/pub/xiph/releases/opus/opus-1.6.1.tar.gz",
        "provenance": "OSU Open Source Lab mirror directory for Xiph Opus releases.",
    },
    {
        "sourceId": "macports_libopus_distfiles_mirror",
        "url": "https://distfiles.macports.org/libopus/opus-1.6.1.tar.gz",
        "provenance": "Official MacPorts distfiles cache for libopus; exact locked digest is still mandatory.",
    },
)
ALLOWED_HOSTS = {urlparse(item["url"]).hostname for item in SOURCES}


class HTTPSAllowlistRedirect(urllib.request.HTTPRedirectHandler):
    """Keep redirects encrypted and within the explicitly listed mirror hosts."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parsed = urlparse(newurl)
        if parsed.scheme != "https" or parsed.hostname not in ALLOWED_HOSTS:
            raise urllib.error.URLError("redirect_outside_https_allowlist")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download_and_verify(url: str, expected: str, path: pathlib.Path) -> dict[str, object]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    opener = urllib.request.build_opener(HTTPSAllowlistRedirect())
    deadline = time.monotonic() + REQUEST_TIMEOUT_SECONDS
    try:
        with opener.open(request, timeout=REQUEST_TIMEOUT_SECONDS) as response:
            final = urlparse(response.geturl())
            if final.scheme != "https" or final.hostname not in ALLOWED_HOSTS:
                return {"transport": "failed", "category": "redirect_outside_https_allowlist"}
            raw = getattr(getattr(response, "fp", None), "raw", None)
            sock = getattr(raw, "_sock", None)
            with path.open("wb") as output:
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("source_request_deadline")
                    if sock is not None:
                        sock.settimeout(remaining)
                    block = response.read(1024 * 1024)
                    if not block:
                        break
                    output.write(block)
        actual = sha256(path)
        size = path.stat().st_size
        return {
            "transport": "completed",
            "bytes": size,
            "sha256": actual,
            "verified": size > 0 and actual == expected,
            "category": "verified" if size > 0 and actual == expected else "sha256_mismatch",
        }
    except urllib.error.HTTPError as exc:
        path.unlink(missing_ok=True)
        return {"transport": "failed", "category": f"http_{exc.code}", "exceptionType": type(exc).__name__}
    except Exception as exc:  # Log exception classes/categories, not headers, URLs, or proxy details.
        path.unlink(missing_ok=True)
        reason = getattr(exc, "reason", None)
        classes = {type(exc).__name__, type(reason).__name__ if reason is not None else ""}
        if "SSLEOFError" in classes:
            category = "tls_unexpected_eof"
        elif any(name in classes for name in ("SSLError", "SSLZeroReturnError")):
            category = "tls_error"
        elif any(name in classes for name in ("TimeoutError", "TimeoutExpired")):
            category = "timeout"
        elif isinstance(exc, urllib.error.URLError):
            reason_text = str(reason) if reason is not None else ""
            category = "redirect_outside_https_allowlist" if "redirect_outside_https_allowlist" in reason_text else "url_transport_error"
        else:
            category = "download_error"
        return {
            "transport": "failed",
            "category": category,
            "exceptionType": type(exc).__name__,
            "reasonType": type(reason).__name__ if reason is not None else None,
        }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locked-tools", required=True, type=pathlib.Path)
    args = parser.parse_args()
    lock = json.loads(args.locked_tools.read_text(encoding="utf-8"))
    opus = lock.get("opus", {})
    expected = str(opus.get("sha256", "")).lower()
    locked_url = opus.get("url")
    if len(expected) != 64 or any(ch not in "0123456789abcdef" for ch in expected):
        raise SystemExit("invalid pinned Opus SHA-256 in locked-tools.json")
    if locked_url != SOURCES[0]["url"]:
        raise SystemExit("official Opus source URL differs from frozen locked-tools.json")
    if any(not item["url"].startswith("https://") for item in SOURCES):
        raise SystemExit("all Opus diagnostic source URLs must use HTTPS")

    results: list[dict[str, object]] = []
    hash_mismatch = False
    with tempfile.TemporaryDirectory(prefix="burrata-opus-mirror-probe-") as temp:
        root = pathlib.Path(temp)
        for source in SOURCES:
            host = urlparse(source["url"]).hostname
            print(json.dumps({
                "event": "source_start",
                "sourceId": source["sourceId"],
                "host": host,
                "archive": ARCHIVE,
                "expectedSha256": expected,
                "provenance": source["provenance"],
            }, sort_keys=True), flush=True)
            result = download_and_verify(source["url"], expected, root / f"{source['sourceId']}.download")
            item = {
                "sourceId": source["sourceId"],
                "host": host,
                "archive": ARCHIVE,
                "expectedSha256": expected,
                **result,
            }
            results.append(item)
            print(json.dumps({"event": "source_result", **item}, sort_keys=True), flush=True)
            if result.get("category") == "sha256_mismatch":
                hash_mismatch = True
                print(json.dumps({
                    "event": "probe_stopped",
                    "reason": "sha256_mismatch_fail_closed",
                    "notAttemptedSourceIds": [next_source["sourceId"] for next_source in SOURCES[len(results):]],
                }, sort_keys=True), flush=True)
                break

    verified = [item["sourceId"] for item in results if item.get("verified") is True]
    outcome = "hash_mismatch_fail_closed" if hash_mismatch else ("at_least_one_exact_archive_verified" if verified else "no_source_verified")
    print(json.dumps({
        "diagnostic": "windows-opus-source-mirror-identity-probe",
        "sourceCommit": LOCKED_COMMIT,
        "archive": ARCHIVE,
        "expectedSha256": expected,
        "results": results,
        "verifiedSourceIds": verified,
        "outcome": outcome,
        "requestTimeoutSecondsPerSource": REQUEST_TIMEOUT_SECONDS,
        "redirectPolicy": "HTTPS-only redirects restricted to the three hardcoded source hosts",
        "proxyEnvironmentOrHeadersLogged": False,
        "cacheWritesPerformed": False,
        "archivePersistedOnlyInTemporaryDirectory": True,
        "temporaryFilesRemoved": True,
    }, sort_keys=True), flush=True)
    return 2 if hash_mismatch else (0 if verified else 1)


if __name__ == "__main__":
    raise SystemExit(main())
