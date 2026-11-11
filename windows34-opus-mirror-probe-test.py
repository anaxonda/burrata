#!/usr/bin/env python3
"""Offline regression checks for final-block socket closure in the Opus probe."""
from __future__ import annotations

import hashlib
import importlib.util
import pathlib
import tempfile
import unittest
from unittest import mock


PROBE_PATH = pathlib.Path(__file__).with_name("windows34-opus-mirror-probe.py")
SPEC = importlib.util.spec_from_file_location("opus_mirror_probe", PROBE_PATH)
assert SPEC is not None and SPEC.loader is not None
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)
CHUNK_SIZE = 1024 * 1024


class ClosingSocket:
    def __init__(self) -> None:
        self.closed = False
        self.settimeout_calls = 0
        self.calls_after_close = 0

    def settimeout(self, _timeout: float) -> None:
        self.settimeout_calls += 1
        if self.closed:
            self.calls_after_close += 1
            raise OSError(10038, "closed test socket")

    def fileno(self) -> int:
        return -1 if self.closed else 17


class RawSocket:
    def __init__(self, sock: ClosingSocket) -> None:
        self._sock = sock


class BufferedSocket:
    def __init__(self, sock: ClosingSocket) -> None:
        self.raw = RawSocket(sock)


class FakeResponse:
    def __init__(self, body: bytes, sock: ClosingSocket) -> None:
        self.body = body
        self.sock = sock
        self.fp = BufferedSocket(sock)
        self.read_calls = 0

    def __enter__(self) -> FakeResponse:
        return self

    def __exit__(self, *_args: object) -> None:
        self.sock.closed = True

    def geturl(self) -> str:
        return PROBE.SOURCES[0]["url"]

    def isclosed(self) -> bool:
        return self.sock.closed

    def read(self, _amount: int) -> bytes:
        self.read_calls += 1
        if self.read_calls > 1:
            return b""
        # Model http.client.HTTPResponse: exhausting Content-Length closes the
        # underlying socket before a final short or exact-size read returns.
        self.sock.closed = True
        return self.body


class FakeOpener:
    def __init__(self, response: FakeResponse) -> None:
        self.response = response

    def open(self, _request: object, timeout: int) -> FakeResponse:
        assert timeout == PROBE.REQUEST_TIMEOUT_SECONDS
        return self.response


class FinalBlockSocketClosureTests(unittest.TestCase):
    def test_final_short_chunk_does_not_reuse_closed_socket(self) -> None:
        self.assert_verified_after_final_block(b"small final block")

    def test_final_exact_chunk_does_not_reuse_closed_socket(self) -> None:
        self.assert_verified_after_final_block(b"x" * CHUNK_SIZE)

    def assert_verified_after_final_block(self, body: bytes) -> None:
        expected = hashlib.sha256(body).hexdigest()
        sock = ClosingSocket()
        response = FakeResponse(body, sock)
        opener = FakeOpener(response)
        with tempfile.TemporaryDirectory(prefix="opus-probe-test-") as temp:
            destination = pathlib.Path(temp) / "archive.download"
            with mock.patch.object(PROBE.urllib.request, "build_opener", return_value=opener):
                result = PROBE.download_and_verify(PROBE.SOURCES[0]["url"], expected, destination)
        self.assertTrue(result["verified"], result)
        self.assertEqual(result["bytes"], len(body))
        self.assertEqual(result["bytesWrittenBeforeFailure"], len(body))
        self.assertEqual(result["phase"], "verified")
        self.assertEqual(sock.calls_after_close, 0)
        self.assertEqual(response.read_calls, 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
