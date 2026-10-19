"""Offline diagnostic protocol checks; never contact providers or audio devices."""
import importlib.util
import json
import os
from pathlib import Path
import threading
import time
import unittest

spec = importlib.util.spec_from_file_location("extractor_reuse", Path(__file__).with_name("extractor_reuse_experiment.py"))
experiment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(experiment)


class DiagnosticProtocolTests(unittest.TestCase):
    def test_sensitive_error_material_never_crosses_protocol(self):
        secret = "https://name:password@example.test/media?sig=secret Cookie: account=private"
        error = RuntimeError("No usable challenge solver lib script available " + secret)
        result = experiment.classify_extractor_failure(error)
        self.assertEqual(result, {"errorClass": "challenge_failed", "errorType": "RuntimeError"})
        self.assertNotIn("secret", json.dumps(result))
        unusual = type("secret_from_exception_type", (Exception,), {})(secret)
        self.assertEqual(experiment.classify_extractor_failure(unusual)["errorType"], "UnknownError")

    def test_dependency_and_provider_failure_boundaries(self):
        self.assertEqual(experiment.classify_extractor_failure(ModuleNotFoundError("yt_dlp_ejs"))["errorClass"], "module_unavailable")
        self.assertEqual(experiment.classify_extractor_failure(RuntimeError("Requested format is not available"))["errorClass"], "format_unavailable")
        self.assertEqual(experiment.classify_extractor_failure(RuntimeError("HTTP Error 429: Too Many Requests"))["errorClass"], "rate_limited")
        self.assertEqual(experiment.classify_extractor_failure(RuntimeError("PO Token provider failed"))["errorClass"], "token_failed")
        self.assertEqual(experiment.classify_extractor_failure(RuntimeError("Usage: tool\nERROR: no such option: --no-netrc"))["errorClass"], "cli_usage_error")

    def test_partial_protocol_frame_has_absolute_deadline(self):
        read_fd, write_fd = os.pipe()
        try:
            os.write(write_fd, b'{"unfinished":')
            start = time.monotonic()
            self.assertIsNone(experiment.JsonLineReader(read_fd).read_until(start + .03))
            self.assertLess(time.monotonic() - start, .5)
        finally:
            os.close(read_fd)
            os.close(write_fd)

    def test_oversized_protocol_frame_is_bounded(self):
        read_fd, write_fd = os.pipe()
        def write():
            try:
                os.write(write_fd, b'x' * (experiment.JsonLineReader.MAX_FRAME_BYTES + 4096))
            except BrokenPipeError:
                pass
            finally:
                os.close(write_fd)
        writer = threading.Thread(target=write)
        writer.start()
        try:
            self.assertIsNone(experiment.JsonLineReader(read_fd).read_until(time.monotonic() + 1))
        finally:
            os.close(read_fd)
            writer.join(timeout=1)
        self.assertFalse(writer.is_alive())


if __name__ == "__main__":
    unittest.main()
