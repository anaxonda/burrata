#!/usr/bin/env python3
"""Hash-verify the Windows Opus mirror into the existing source cache."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import tempfile
import time
import urllib.error
import urllib.request
from urllib.parse import urlparse
from typing import Callable


ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK_PATH = ROOT / "scripts/release/locked-tools.json"
OPUS_VERSION = "1.6.1"
ARCHIVE = f"opus-{OPUS_VERSION}.tar.gz"
EXPECTED_SHA256 = "6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1"
OFFICIAL_URL = "https://downloads.xiph.org/releases/opus/opus-1.6.1.tar.gz"
MIRROR_URL = "https://ftp.osuosl.org/pub/xiph/releases/opus/opus-1.6.1.tar.gz"
MIRROR_HOST = "ftp.osuosl.org"
USER_AGENT = "Burrata-release-tool-preparer"
REQUEST_TIMEOUT_SECONDS = 45
CHUNK_BYTES = 1024 * 1024


class RedirectOutsideMirror(Exception):
    pass


class PreseedFailure(Exception):
    def __init__(self, facts: dict[str, object]) -> None:
        super().__init__()
        self.facts = facts


class HTTPSMirrorRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parsed = urlparse(newurl)
        if parsed.scheme != "https" or parsed.hostname != MIRROR_HOST:
            raise urllib.error.URLError(RedirectOutsideMirror())
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(CHUNK_BYTES), b""):
            digest.update(chunk)
    return digest.hexdigest()


def stream_closed(response: object, sock: object | None) -> bool:
    isclosed = getattr(response, "isclosed", None)
    if callable(isclosed):
        try:
            if isclosed():
                return True
        except (OSError, ValueError):
            return True
    if sock is not None:
        try:
            return sock.fileno() < 0
        except (OSError, ValueError):
            return True
    return False


def cause_chain_types(exc: BaseException, limit: int = 5) -> list[str]:
    result: list[str] = []
    seen: set[int] = set()
    current: BaseException | None = exc
    while current is not None and len(result) < limit and id(current) not in seen:
        seen.add(id(current))
        result.append(type(current).__name__)
        current = current.__cause__ if current.__cause__ is not None else current.__context__
    return result


def int_fact(exc: BaseException, name: str) -> int | None:
    value = getattr(exc, name, None)
    return value if isinstance(value, int) else None


def failure_facts(exc: BaseException, phase: str, final_host: str | None, bytes_written: int) -> dict[str, object]:
    reason = getattr(exc, "reason", None)
    classes = {type(exc).__name__, type(reason).__name__ if reason is not None else ""}
    if "SSLEOFError" in classes:
        category = "tls_unexpected_eof"
    elif any(name in classes for name in ("SSLError", "SSLZeroReturnError")):
        category = "tls_error"
    elif any(name in classes for name in ("TimeoutError", "TimeoutExpired")):
        category = "timeout"
    elif isinstance(exc, urllib.error.HTTPError):
        category = f"http_{exc.code}"
    elif isinstance(exc, urllib.error.URLError) and isinstance(reason, RedirectOutsideMirror):
        category = "redirect_outside_https_mirror"
    elif isinstance(exc, urllib.error.URLError):
        category = "url_transport_error"
    elif isinstance(exc, OSError):
        category = "os_error"
    else:
        category = "download_error"
    return {
        "status": "failed",
        "category": category,
        "phase": phase,
        "finalHost": final_host,
        "bytesWrittenBeforeFailure": bytes_written,
        "exceptionType": type(exc).__name__,
        "reasonType": type(reason).__name__ if reason is not None else None,
        "causeChainTypes": cause_chain_types(exc),
        "errno": int_fact(exc, "errno"),
        "winerror": int_fact(exc, "winerror"),
    }


def default_open_response(request: urllib.request.Request, timeout: float):
    opener = urllib.request.build_opener(HTTPSMirrorRedirect())
    return opener.open(request, timeout=timeout)


def fetch_to_temp(
    url: str,
    destination: pathlib.Path,
    open_response: Callable[[urllib.request.Request, float], object] = default_open_response,
) -> int:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    deadline = time.monotonic() + REQUEST_TIMEOUT_SECONDS
    phase = "opener"
    final_host: str | None = None
    bytes_written = 0
    try:
        with open_response(request, REQUEST_TIMEOUT_SECONDS) as response:
            phase = "response_url"
            final = urlparse(response.geturl())
            if final.scheme != "https" or final.hostname != MIRROR_HOST:
                raise RedirectOutsideMirror()
            final_host = final.hostname
            raw = getattr(getattr(response, "fp", None), "raw", None)
            sock = getattr(raw, "_sock", None)
            phase = "open_temp_file"
            with destination.open("wb") as output:
                while True:
                    phase = "deadline_check"
                    if stream_closed(response, sock):
                        break
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("request_deadline")
                    if sock is not None:
                        phase = "set_socket_timeout"
                        sock.settimeout(remaining)
                    phase = "read_response"
                    read_once = getattr(response, "read1", None)
                    if not callable(read_once):
                        read_once = response.read
                    block = read_once(CHUNK_BYTES)
                    if not block:
                        break
                    phase = "write_temp_file"
                    output.write(block)
                    bytes_written += len(block)
                    if stream_closed(response, sock):
                        break
        return bytes_written
    except Exception as exc:
        destination.unlink(missing_ok=True)
        raise PreseedFailure(failure_facts(exc, phase, final_host, bytes_written)) from None


def preseed_cache_file(
    cache_dir: pathlib.Path,
    archive: str,
    url: str,
    expected_sha256: str,
    open_response: Callable[[urllib.request.Request, float], object] = default_open_response,
) -> dict[str, object]:
    cache_dir.mkdir(parents=True, exist_ok=True)
    destination = cache_dir / archive
    if destination.is_file() and sha256(destination) == expected_sha256:
        return {
            "status": "valid_cache_hit",
            "archive": archive,
            "sha256": expected_sha256,
            "cachePath": f".cache/media-tool-sources/{archive}",
            "mirrorRequested": False,
        }

    fd, raw_temp_path = tempfile.mkstemp(prefix=f".{archive}.", suffix=".partial", dir=cache_dir)
    os.close(fd)
    temp_path = pathlib.Path(raw_temp_path)
    try:
        bytes_written = fetch_to_temp(url, temp_path, open_response)
        actual = sha256(temp_path)
        if actual != expected_sha256 or bytes_written <= 0:
            raise PreseedFailure({
                "status": "failed",
                "category": "sha256_mismatch",
                "phase": "verify_temp_file",
                "archive": archive,
                "expectedSha256": expected_sha256,
                "actualSha256": actual,
                "bytesWritten": bytes_written,
                "cachePublished": False,
            })
        os.replace(temp_path, destination)
        return {
            "status": "published_verified_cache_file",
            "archive": archive,
            "bytes": bytes_written,
            "sha256": actual,
            "sourceHost": MIRROR_HOST,
            "cachePath": f".cache/media-tool-sources/{archive}",
            "atomicReplace": True,
        }
    finally:
        temp_path.unlink(missing_ok=True)


def self_test() -> int:
    import unittest
    from types import SimpleNamespace
    from unittest import mock

    clock = [0.0]

    class FakeSocket:
        def __init__(self) -> None:
            self.closed = False
            self.timeouts: list[float] = []
            self.calls_after_close = 0

        def settimeout(self, value: float) -> None:
            if self.closed:
                self.calls_after_close += 1
                raise OSError(10038, "closed test socket")
            self.timeouts.append(value)

        def fileno(self) -> int:
            return -1 if self.closed else 17

    class FakeResponse:
        def __init__(self, data: bytes | list[bytes], *, close_on_read: bool = True, delay_per_read: float = 0.0) -> None:
            self.blocks = list(data) if isinstance(data, list) else [data]
            self.sock = FakeSocket()
            self.fp = SimpleNamespace(raw=SimpleNamespace(_sock=self.sock))
            self.close_on_read = close_on_read
            self.delay_per_read = delay_per_read
            self.read_calls = 0

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            self.sock.closed = True

        def geturl(self) -> str:
            return MIRROR_URL

        def isclosed(self) -> bool:
            return self.sock.closed

        def read1(self, _amount: int) -> bytes:
            self.read_calls += 1
            if self.delay_per_read:
                timeout = self.sock.timeouts[-1]
                if self.delay_per_read > timeout:
                    clock[0] += timeout
                    raise TimeoutError("simulated socket read timeout")
                clock[0] += self.delay_per_read
            data = self.blocks.pop(0) if self.blocks else b""
            if self.close_on_read and not self.blocks:
                self.sock.closed = True
            return data

        def read(self, amount: int) -> bytes:
            return self.read1(amount)

    class Tests(unittest.TestCase):
        def test_valid_cache_skips_mirror(self):
            data = b"already cached exact data"
            expected = hashlib.sha256(data).hexdigest()
            with tempfile.TemporaryDirectory() as temp:
                cache = pathlib.Path(temp)
                (cache / ARCHIVE).write_bytes(data)
                result = preseed_cache_file(cache, ARCHIVE, MIRROR_URL, expected, open_response=mock.Mock(side_effect=AssertionError("mirror should be skipped")))
            self.assertEqual(result["status"], "valid_cache_hit")
            self.assertFalse(result["mirrorRequested"])

        def test_verified_download_publishes_atomically(self):
            data = b"exact mirror bytes"
            expected = hashlib.sha256(data).hexdigest()
            with tempfile.TemporaryDirectory() as temp:
                cache = pathlib.Path(temp)
                destination = cache / ARCHIVE
                destination.write_bytes(b"old invalid cache")
                opener = mock.Mock(return_value=FakeResponse(data))
                result = preseed_cache_file(cache, ARCHIVE, MIRROR_URL, expected, open_response=opener)
                self.assertEqual(destination.read_bytes(), data)
                self.assertEqual(result["status"], "published_verified_cache_file")
                self.assertTrue(result["atomicReplace"])
                self.assertEqual(list(cache.iterdir()), [destination])
                opener.assert_called_once()

        def test_final_short_and_exact_chunks_do_not_reuse_closed_socket(self):
            for data in (b"short final block", b"x" * CHUNK_BYTES):
                with self.subTest(size=len(data)):
                    expected = hashlib.sha256(data).hexdigest()
                    response = FakeResponse(data)
                    opener = mock.Mock(return_value=response)
                    with tempfile.TemporaryDirectory() as temp:
                        result = preseed_cache_file(pathlib.Path(temp), ARCHIVE, MIRROR_URL, expected, open_response=opener)
                    self.assertEqual(result["status"], "published_verified_cache_file")
                    self.assertEqual(result["bytes"], len(data))
                    self.assertEqual(response.sock.calls_after_close, 0)
                    self.assertEqual(response.read_calls, 1)

        def test_absolute_deadline_bounds_slow_body_and_preserves_existing_cache(self):
            clock[0] = 0.0
            response = FakeResponse([b"first", b"second", b"third"], close_on_read=False, delay_per_read=20.0)
            opener = mock.Mock(return_value=response)
            previous_cache = b"existing cache remains untouched"
            expected = hashlib.sha256(b"locked archive bytes").hexdigest()
            with tempfile.TemporaryDirectory() as temp:
                cache = pathlib.Path(temp)
                destination = cache / ARCHIVE
                destination.write_bytes(previous_cache)
                with mock.patch.object(time, "monotonic", side_effect=lambda: clock[0]):
                    with self.assertRaises(PreseedFailure) as caught:
                        preseed_cache_file(cache, ARCHIVE, MIRROR_URL, expected, open_response=opener)
                facts = caught.exception.facts
                self.assertEqual(facts["category"], "timeout")
                self.assertEqual(facts["phase"], "read_response")
                self.assertEqual(facts["bytesWrittenBeforeFailure"], len(b"firstsecond"))
                self.assertEqual(destination.read_bytes(), previous_cache)
                self.assertEqual(list(cache.iterdir()), [destination])
                self.assertEqual(response.sock.timeouts, [45.0, 25.0, 5.0])
                self.assertEqual(response.read_calls, 3)
                opener.assert_called_once()

        def test_hash_mismatch_fails_without_replacing_cache_or_retrying(self):
            data = b"wrong mirror bytes"
            expected = hashlib.sha256(b"expected source bytes").hexdigest()
            original_cache = b"existing invalid cache stays untouched"
            with tempfile.TemporaryDirectory() as temp:
                cache = pathlib.Path(temp)
                destination = cache / ARCHIVE
                destination.write_bytes(original_cache)
                opener = mock.Mock(return_value=FakeResponse(data))
                with self.assertRaises(PreseedFailure) as caught:
                    preseed_cache_file(cache, ARCHIVE, MIRROR_URL, expected, open_response=opener)
                self.assertEqual(caught.exception.facts["category"], "sha256_mismatch")
                self.assertFalse(caught.exception.facts["cachePublished"])
                self.assertEqual(destination.read_bytes(), original_cache)
                self.assertEqual(list(cache.iterdir()), [destination])
                opener.assert_called_once()

    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Tests)
    return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    if os.name != "nt" or not os.environ.get("MSYSTEM", "").startswith("MINGW64"):
        raise SystemExit("Opus mirror cache preseed is restricted to Windows MSYS2 MINGW64")
    lock = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
    opus = lock.get("opus", {})
    version = opus.get("version")
    archive = f"opus-{version}.tar.gz"
    expected = str(opus.get("sha256", "")).lower()
    if version != OPUS_VERSION or archive != ARCHIVE or expected != EXPECTED_SHA256:
        raise SystemExit("frozen Opus lock differs from the reviewed preseed payload; refusing mirror use")
    if opus.get("url") != OFFICIAL_URL:
        raise SystemExit("canonical Opus source URL changed; refusing mirror use")

    cache_dir = ROOT / ".cache/media-tool-sources"
    try:
        result = preseed_cache_file(cache_dir, archive, MIRROR_URL, expected)
    except PreseedFailure as exc:
        print(json.dumps({"status": "preseed_failed", **exc.facts}, sort_keys=True), flush=True)
        return 1
    print(json.dumps(result, sort_keys=True), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
