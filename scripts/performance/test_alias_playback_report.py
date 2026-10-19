import json
from pathlib import Path
import tempfile
import unittest

from alias_playback_report import read_events, summarize


def event(caller, phase, duration=None, shared=None, **details):
    if duration is not None:
        details["durationMs"] = duration
    return {"resolutionId": caller, "phase": phase, "details": details,
            "sharedLookupId": shared, "atUnixMs": 100, "elapsedMs": duration or 0}


class AliasReportTests(unittest.TestCase):
    def test_cold_warm_failure_and_skipped(self):
        events = [event("cold", "alias_discovery_start", shared="worker"),
                  event("cold", "alias_discovery_end", 90, "worker", ok=True, metadataRequests=2, bytes=400),
                  event("cold", "alias_lookup_end", 100, outcome="ok"),
                  event("warm", "alias_cache_lookup_end", 1, aliases=2),
                  event("warm", "alias_lookup_end", 2, outcome="hit"),
                  event("failure", "alias_discovery_end", 20, "failed", ok=False),
                  event("failure", "alias_lookup_end", 22, outcome="error"),
                  event("skipped", "resolution_end", 10)]
        report = summarize(events)
        self.assertEqual(report["outcomes"]["discovery"], 2)
        self.assertEqual(report["outcomes"]["warm"], 1)
        self.assertEqual(report["outcomes"]["skipped"], 1)
        self.assertEqual(report["failedWorkers"], 1)
        self.assertEqual(report["metadataRequests"], 2)
        self.assertEqual(report["knownMetadataBytes"], 400)

    def test_concurrent_callers_share_worker_and_keep_waits(self):
        request = event("a", "alias_metadata_request_end", 40, "worker", bytes=200, status=200)
        events = [event("a", "alias_lookup_start", shared="worker", joined=False),
                  event("b", "alias_lookup_start", shared="worker", joined=True),
                  event("a", "alias_discovery_end", 50, "worker", ok=True, metadataRequests=1, bytes=200),
                  event("b", "alias_discovery_end", 50, "worker", ok=True, metadataRequests=1, bytes=200),
                  request, {**request, "resolutionId": "b"},
                  event("a", "alias_lookup_end", 55), event("b", "alias_lookup_end", 30)]
        report = summarize(events)
        self.assertEqual(report["workers"], 1)
        self.assertEqual(report["joinedCallers"], 1)
        self.assertEqual(report["metadataRequests"], 1)
        self.assertEqual(report["knownMetadataBytes"], 200)
        self.assertEqual(report["timings"]["caller_wait_ms"]["median"], 42.5)
        self.assertEqual(report["timings"]["discovery_worker_wall_ms"]["samples"], 1)

    def test_per_credit_cache_and_cancelled_http_are_counted(self):
        events = [event("cached", "alias_cache_lookup_end", .2, aliases=0, verifiedCreditSets=True),
                  event("cancelled", "alias_lookup_start", shared="worker"),
                  event("cancelled", "alias_metadata_request_end", 3, "worker", bytes=20, status=200, outcome="aborted"),
                  event("cancelled", "alias_lookup_end", 4, outcome="cancelled")]
        report = summarize(events)
        self.assertEqual(report["outcomes"]["warm"], 1)
        self.assertEqual(report["metadataRequests"], 1)
        self.assertEqual(report["knownMetadataBytes"], 20)
        self.assertEqual(report["timings"]["caller_wait_ms"]["worst"], 4)
        self.assertEqual(report["timings"]["discovery_worker_wall_ms"]["samples"], 0)

    def test_warmup_and_distribution(self):
        report = summarize([event(str(i), "alias_lookup_end", i) for i in range(23)], warmup=3)
        self.assertEqual(report["callers"], 20)
        self.assertEqual(report["timings"]["caller_wait_ms"], {"samples": 20, "median": 12.5, "p95": 21, "worst": 22})

    def test_no_private_details_in_summary_and_malformed_lines(self):
        secret = "https://private.example/title"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.log"
            path.write_text("noise\nburrata_playback_phase {bad}\nburrata_playback_phase " +
                            json.dumps(event("caller", "alias_lookup_end", 2, outcome=secret, title=secret)) + "\n")
            events, malformed = read_events(path)
        self.assertEqual(malformed, 1)
        self.assertNotIn(secret, json.dumps(summarize(events)))


if __name__ == "__main__":
    unittest.main()
