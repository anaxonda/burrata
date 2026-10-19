"""Offline checks for the matched-source extractor diagnostic helpers."""
import importlib.util
from pathlib import Path
from unittest import mock
import unittest

MODULE_PATH = Path(__file__).with_name("extractor_reuse_experiment.py")
spec = importlib.util.spec_from_file_location("extractor_reuse_matched", MODULE_PATH)
experiment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(experiment)


def stream(video_id="video-a", format_id="140", **extra):
    return {
        "id": video_id,
        "format_id": format_id,
        "url": "https://media.example.test/audio?sig=PRIVATE",
        "protocol": "https",
        "acodec": "mp4a.40.2",
        "ext": "m4a",
        "http_headers": {"User-Agent": "ua", "Referer": "https://www.youtube.com/",
                         "Cookie": "secret=PRIVATE"},
        **extra,
    }


class MatchedStreamTests(unittest.TestCase):
    def test_selects_only_the_resolved_top_level_stream(self):
        info = stream(formats=[stream(format_id="251")])
        selected = experiment.select_resolved_stream(info, "video-a", "140")
        self.assertIs(selected, info)
        with self.assertRaises(experiment.DiagnosticFailure):
            experiment.select_resolved_stream(stream(video_id="video-b", formats=[stream()]), "video-a", "140")

    def test_rejects_mismatched_identity_without_format_fallback(self):
        cases = [
            (stream().copy() | {"id": "wrong"}, "IDENTITY_MISMATCH"),
            ({k: v for k, v in stream().items() if k != "id"}, "IDENTITY_MISSING"),
            (stream().copy() | {"format_id": "251", "formats": [stream(format_id="140")]}, "FORMAT_MISMATCH"),
            ({k: v for k, v in stream().items() if k != "format_id"}, "FORMAT_MISSING"),
        ]
        for info, code in cases:
            with self.subTest(code=code):
                with self.assertRaises(experiment.DiagnosticFailure) as caught:
                    experiment.select_resolved_stream(info, "video-a", "140")
                self.assertEqual(caught.exception.code, code)

    def test_rejects_non_http_or_missing_audio(self):
        for info, code in ((stream(protocol="m3u8_native"), "STREAM_PROTOCOL_UNSUPPORTED"),
                           (stream(acodec="none"), "AUDIO_MISSING"),
                           (stream().copy() | {"url": ""}, "AUDIO_MISSING")):
            with self.subTest(code=code):
                with self.assertRaises(experiment.DiagnosticFailure) as caught:
                    experiment.select_resolved_stream(info, "video-a", "140")
                self.assertEqual(caught.exception.code, code)

    def test_fingerprint_is_allowlisted_and_contains_no_credentials_or_url(self):
        info = stream(format_id="140", samplerate=44100, audio_channels=2, filesize=123456,
                      headers={"Authorization": "secret"}, cookiejar="private")
        fingerprint = experiment.selection_fingerprint(info)
        self.assertEqual(set(fingerprint), {"formatId", "acodec", "ext", "protocol", "sampleRate", "channels", "contentLength"})
        self.assertEqual(fingerprint["formatId"], "140")
        self.assertEqual(fingerprint["sampleRate"], 44100)
        self.assertEqual(fingerprint["channels"], 2)
        self.assertEqual(fingerprint["contentLength"], 123456)
        self.assertNotIn("PRIVATE", repr(fingerprint))
        self.assertNotIn("url", repr(fingerprint).lower())

    def test_header_filter_normalizes_names_and_discards_unsafe_values(self):
        headers = {
            "USER-AGENT": "agent", "User-Agent": "x" * 2049, "Referer": "https://www.youtube.com/",
            "ORIGIN": "https://www.youtube.com", "Accept-Language": "en-US",
            "Cookie": "secret=PRIVATE", "Authorization": "Bearer PRIVATE",
            "X-Bad": "safe", "X-Evil\r\nInjected": "bad", "Origin2": "bad",
            "User-Agent-long": "x" * 2049, "User-Agent\r\nInjected": "bad",
        }
        cleaned = experiment.safe_stream_headers({"http_headers": headers})
        self.assertEqual(cleaned, [("user-agent", "agent"), ("referer", "https://www.youtube.com/"),
                                   ("origin", "https://www.youtube.com"), ("accept-language", "en-US")])
        self.assertNotIn("PRIVATE", repr(cleaned))

    def test_each_gate_owns_a_scoped_handler_without_opening_sockets(self):
        servers = []

        class FakeServer:
            server_port = 12345
            daemon_threads = False
            timeout = None

            def __init__(self, address, handler):
                self.address = address
                self.RequestHandlerClass = handler
                servers.append(self)

            def shutdown(self):
                pass

            def serve_forever(self):
                pass

            def server_close(self):
                pass

        class FakeThread:
            def __init__(self, *args, **kwargs):
                pass

            def start(self):
                pass

            def join(self, timeout=None):
                pass

        tokens = iter(("token-one", "token-two"))
        with mock.patch.object(experiment.http.server, "ThreadingHTTPServer", FakeServer), \
                mock.patch.object(experiment.threading, "Thread", FakeThread), \
                mock.patch.object(experiment.uuid, "uuid4", side_effect=lambda: type("Token", (), {"hex": next(tokens)})()):
            first = experiment.Gate("http://helper-one.test")
            second = experiment.Gate("http://helper-two.test")

        first_handler, second_handler = (server.RequestHandlerClass for server in servers)
        self.assertIsNot(first_handler, second_handler)
        self.assertEqual((first_handler.helper_url, first_handler.token), ("http://helper-one.test", "token-one"))
        self.assertEqual((second_handler.helper_url, second_handler.token), ("http://helper-two.test", "token-two"))
        self.assertEqual(first.token, "token-one")
        self.assertEqual(second.token, "token-two")
        first.close()
        second.close()


class MatchedSummaryTests(unittest.TestCase):
    def test_schedule_rotates_all_arms_and_balances_positions(self):
        schedule = experiment.build_schedule(3)
        arms = {"bundled_cli_fresh", "python_process_fresh_object", "python_resident_reused_object"}
        self.assertEqual(len(schedule), 9)
        positions = {arm: [] for arm in arms}
        rounds = {}
        for row in schedule:
            rounds.setdefault(row["pairId"], []).append(row["arm"])
        self.assertEqual(set(rounds), {1, 2, 3})
        self.assertEqual(rounds, {
            1: ["bundled_cli_fresh", "python_process_fresh_object", "python_resident_reused_object"],
            2: ["python_process_fresh_object", "python_resident_reused_object", "bundled_cli_fresh"],
            3: ["python_resident_reused_object", "bundled_cli_fresh", "python_process_fresh_object"],
        })
        for order in rounds.values():
            self.assertEqual(set(order), arms)
            for position, arm in enumerate(order):
                positions[arm].append(position)
        for arm_positions in positions.values():
            self.assertEqual(sorted(arm_positions), [0, 1, 2])
        for invalid in (0, 4):
            with self.assertRaises(ValueError):
                experiment.build_schedule(invalid)

    def test_summary_uses_only_successful_matched_tuples_and_hides_p95_for_three(self):
        rows = []
        fingerprint = {"formatId": "140", "acodec": "mp4a.40.2", "ext": "m4a", "protocol": "https",
                       "sampleRate": 44100, "channels": 2, "contentLength": 123456}
        for arm, times in (("bundled_cli_fresh", [10, 20, 30]),
                           ("python_process_fresh_object", [12, 22, 32]),
                           ("python_resident_reused_object", [8, 18, 28])):
            for index, latency in enumerate(times, 1):
                rows.append({"arm": arm, "pairId": f"pair-{index}", "videoId": f"video-{index}",
                             "resolvedVideoId": f"video-{index}", "expectedFormatId": "140", "formatId": "140",
                             "identityMatched": True, "formatMatched": True, "ok": True,
                             "requestToFirstPcmMs": latency, "requestWallMs": latency + 5,
                             "selectionFingerprint": fingerprint})

        # Same source and itag still do not establish equivalent streams when
        # codec/container/transport metadata differs across arms.
        changed_fingerprint = dict(fingerprint, protocol="https_native")
        next(row for row in rows if row["arm"] == "python_resident_reused_object" and row["pairId"] == "pair-1")[
            "selectionFingerprint"] = changed_fingerprint
        # These tempting low numbers must not contribute to latency summaries or deltas.
        rows.extend([
            {"arm": "python_resident_reused_object", "pairId": "bad-source", "videoId": "video-1",
             "resolvedVideoId": "other", "expectedFormatId": "140", "formatId": "140", "identityMatched": False,
             "formatMatched": True, "ok": True, "requestToFirstPcmMs": 1, "requestWallMs": 2,
             "selectionFingerprint": fingerprint},
            {"arm": "python_resident_reused_object", "pairId": "bad-format", "videoId": "video-1",
             "resolvedVideoId": "video-1", "expectedFormatId": "140", "formatId": "251", "identityMatched": True,
             "formatMatched": False, "ok": True, "requestToFirstPcmMs": 2, "requestWallMs": 3,
             "selectionFingerprint": fingerprint},
            {"arm": "python_resident_reused_object", "pairId": "failed", "videoId": "video-1",
             "resolvedVideoId": "video-1", "expectedFormatId": "140", "formatId": "140", "identityMatched": True,
             "formatMatched": True, "ok": False, "requestToFirstPcmMs": 0.1, "requestWallMs": 100,
             "selectionFingerprint": fingerprint},
        ])
        result = experiment.summarize_matched(rows)
        self.assertNotIn("p95", repr(result).lower())
        self.assertIn("pairedDeltas", result)
        excluded = [delta for delta in result["pairedDeltas"] if not delta["included"]]
        self.assertTrue(excluded)
        self.assertTrue(all(delta["requestToFirstPcmDeltaMs"] is None for delta in excluded))
        self.assertEqual(result["arms"]["python_resident_reused_object"]["successes"], 3)
        changed = [delta for delta in result["pairedDeltas"]
                   if delta["pairId"] == "pair-1" and delta["treatmentArm"] == "python_resident_reused_object"]
        self.assertEqual(len(changed), 1)
        self.assertFalse(changed[0]["included"])
        self.assertEqual(changed[0]["reason"], "STREAM_FINGERPRINT_CHANGED")


if __name__ == "__main__":
    unittest.main()
