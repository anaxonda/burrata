import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from contextlib import redirect_stderr
from io import StringIO

SCRIPT = Path(__file__).parents[2] / "scripts/performance/matching_cohort.py"
spec = importlib.util.spec_from_file_location("matching_cohort", SCRIPT)
cohort = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cohort)


class CohortTests(unittest.TestCase):
    def test_source_snapshot_records_base_dirty_state_and_diff_fingerprint(self):
        snapshot = cohort.source_snapshot(Path(__file__).parents[2])
        self.assertRegex(snapshot["sourceRevision"], r"^[0-9a-f]{40}$")
        self.assertIsInstance(snapshot["worktreeDirty"], bool)
        self.assertRegex(snapshot["trackedDiffSha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(snapshot["untrackedBuildSourceFileCount"],
                         len(snapshot["untrackedBuildSourceFiles"]))

    def test_level_selector_defaults_to_all_and_accepts_two_canonical_levels(self):
        parser = cohort.build_parser()
        base = ["--binary", "bin", "--manifest", "manifest.json", "--outputdir", "out"]
        self.assertIsNone(parser.parse_args(base).levels)
        self.assertEqual(parser.parse_args(base + ["--levels", "Balanced", "Flexible"]).levels,
                         ["balanced", "flexible"])
        with redirect_stderr(StringIO()):
            with self.assertRaises(SystemExit):
                parser.parse_args(base + ["--levels", "other"])

    def test_trace_parser_counts_duplicates_and_ignores_bad_unmatched_or_unknown_events(self):
        def line(operation, phase, details):
            return cohort.TRACE_PREFIX + json.dumps({"nativeOperationId": operation,
                "phase": phase, "details": details, "secret": "must not escape"})
        log = "\n".join([
            line("op1", "search_start", {}),
            line("op1", "search_start", {}),
            line("other", "resolution_end", {"durationMs": 999}),
            line("op1", "direct_youtube_end", {"elapsedMs": 24, "attemptsUsed": 2}),
            line("op1", "unknown_phase", {"durationMs": 400}),
            cohort.TRACE_PREFIX + "{bad json",
        ])
        result = cohort.parse_trace_events(log, "op1")
        self.assertEqual(result["matchedEventCount"], 3)
        self.assertEqual(result["phaseEventCounts"], {"search_start": 2, "direct_youtube_end": 1})
        self.assertEqual(result["phaseDurationsMs"]["direct_youtube_end"], [24.0])
        self.assertEqual(result["observedResolverExtractionBudgetCounter"], 2)

    def test_trace_aggregation_excludes_warmups_and_separates_counter_from_extraction_events(self):
        def line(op, phase, details):
            return cohort.TRACE_PREFIX + json.dumps({"nativeOperationId": op,
                "phase": phase, "details": details})
        report = {"attempts": [
            {"scenario": "cold", "warmup": True, "nativeOperationId": "warmup-op", "ok": True},
            {"scenario": "cold", "warmup": False, "nativeOperationId": "measured-op", "ok": False},
        ]}
        log = "\n".join([line("warmup-op", "search_start", {}),
                          line("measured-op", "search_start", {}),
                          line("measured-op", "candidate_gate", {"sharedExtractionAttemptsUsed": 0}),
                          line("measured-op", "extraction_start", {}),
                          line("measured-op", "search_end", {"durationMs": 12})])
        result = cohort.attempt_summary(report, log, trace_enabled=True)
        cold = result["traceByScenario"]["cold"]
        self.assertEqual(cold["attemptsWithCorrelatedTrace"], 1)
        self.assertEqual(cold["phaseEventCounts"], {"candidate_gate": 1, "extraction_start": 1,
                                                     "search_start": 1, "search_end": 1})
        self.assertEqual(cold["logicalExtractionStartEvents"], 1)
        self.assertEqual(cold["phaseDurationObservationsMs"]["search_end"]["p50"], 12.0)
        self.assertEqual(cold["observedResolverExtractionBudgetCounter"]["sum"], 0)
        self.assertEqual(result["attempts"][0]["trace"]["observedResolverExtractionBudgetCounter"], 0)

    def test_nested_trace_links_only_through_one_known_shared_lookup_consumer(self):
        def line(native, phase, top_shared=None, details=None):
            return cohort.TRACE_PREFIX + json.dumps({"nativeOperationId": native,
                "sharedLookupId": top_shared, "phase": phase, "details": details or {}})
        log = "\n".join([
            line("op-a", "preview_audio_lookup_start", details={"sharedLookupId": "shared-a"}),
            line("op-b", "preview_audio_lookup_start", details={"sharedLookupId": "shared-b"}),
            line(None, "extraction_start", top_shared="shared-a"),
            line("unrelated-native", "search_start", top_shared="shared-a"),
            line(None, "extraction_end", top_shared="unknown-shared"),
            line("op-a", "preview_audio_lookup_start", details={"sharedLookupId": "shared-b"}),
            line(None, "extraction_start", top_shared="shared-b"),
        ])
        buckets, diagnostics = cohort.parse_trace_log(log, ["op-a", "op-b"])
        self.assertEqual(buckets["op-a"]["phaseEventCounts"]["extraction_start"], 1)
        self.assertNotIn("extraction_end", buckets.get("op-a", {}).get("phaseEventCounts", {}))
        self.assertEqual(diagnostics["sharedLookupLinkedEvents"], 1)
        self.assertEqual(diagnostics["unknownNativeEvents"], 1)
        self.assertEqual(diagnostics["unknownSharedLookupEvents"], 1)
        self.assertEqual(diagnostics["ambiguousSharedLookupEvents"], 1)
        self.assertEqual(diagnostics["directNativeEvents"], 3)

    def test_failures_stay_in_denominator_and_p95_requires_twenty(self):
        rows = [{"scenario": "cold", "warmup": False, "ok": True, "resolveMs": i}
                for i in range(19)]
        rows.append({"scenario": "cold", "warmup": False, "ok": False, "resolveMs": 100})
        result = cohort.attempt_summary({"attempts": rows, "runPassed": False})
        cold = result["scenarios"]["cold"]
        self.assertEqual(cold["attempts"], 20)
        self.assertEqual(cold["failures"], 1)
        self.assertIsNotNone(cold["resolveMsP95"])
        self.assertIsNone(result["scenarios"]["warm"]["resolveMsP95"])

    def test_paired_source_change_is_reported(self):
        rows = [
            {"scenario": "cold", "warmup": False, "pairIndex": 0, "resolvedRecordingIdentity": {"providerId": "abc"}, "ok": True},
            {"scenario": "warm", "warmup": False, "pairIndex": 0, "resolvedRecordingIdentity": {"providerId": "def"}, "ok": True},
        ]
        result = cohort.attempt_summary({"attempts": rows})
        self.assertEqual(result["pairedSourceIdentityDifferenceCount"], 1)

    def test_shared_process_batch_pairs_each_entry_and_keeps_batch_resource_scope(self):
        rows = [
            {"scenario": "cold", "warmup": False, "pairIndex": 0, "batchEntryIndex": 0,
             "resolvedRecordingIdentity": {"providerId": "a"}, "ok": True, "resolveMs": 10,
             "instrumentedMetrics": {"rustHttpSendAttempts": 4, "secret": "do-not-copy"}},
            {"scenario": "warm", "warmup": False, "pairIndex": 0, "batchEntryIndex": 0,
             "resolvedRecordingIdentity": {"providerId": "a"}, "ok": True, "resolveMs": 2},
            {"scenario": "cold", "warmup": False, "pairIndex": 0, "batchEntryIndex": 1,
             "resolvedRecordingIdentity": {"providerId": "b"}, "ok": True, "resolveMs": 20},
            {"scenario": "warm", "warmup": False, "pairIndex": 0, "batchEntryIndex": 1,
             "resolvedRecordingIdentity": {"providerId": "b"}, "ok": True, "resolveMs": 3},
        ]
        result = cohort.attempt_summary({"attempts": rows})
        self.assertEqual(result["perBatchEntry"]["1"]["cold"]["resolveMsP50"], 20)
        self.assertEqual(result["pairedSourceIdentityDifferenceCount"], 0)
        self.assertEqual(result["attempts"][0]["instrumentedMetrics"]["rustHttpSendAttempts"], 4)
        self.assertIsNone(result["attempts"][0]["instrumentedMetrics"]["matchPrepareCpuElapsedNs"])

    def test_cpu_metric_without_clock_observations_remains_null(self):
        rows = [{"scenario": "cold", "warmup": False, "ok": True, "resolveMs": 1,
                 "instrumentedMetrics": {"matchPrepareCpuElapsedNs": None,
                                         "matchPrepareCpuObservations": 0}}]
        result = cohort.attempt_summary({"attempts": rows, "batches": [
            {"scenario": "cold", "warmup": False, "failureCount": 0,
             "instrumentedMetrics": {"matchPrepareCpuElapsedNs": None,
                                     "matchPrepareCpuObservations": 0}}
        ]})
        metrics = result["attempts"][0]["instrumentedMetrics"]
        self.assertIsNone(metrics["matchPrepareCpuElapsedNs"])
        cold = result["batchSummary"]["cold"]
        self.assertIsNone(cold["instrumentedMetricTotals"]["matchPrepareCpuElapsedNs"])
        self.assertEqual(cold["instrumentedMetricDistributions"]["matchPrepareCpuElapsedNs"]["n"], 0)
        self.assertEqual(cold["instrumentedMetricTotals"]["matchPrepareCpuObservations"], 0)

    def test_rejects_profile_or_secret_settings(self):
        with self.assertRaises(ValueError):
            cohort.check_public({"preferredStreamingSource": "youtube", "apiToken": "x"})

    def test_fake_binary_preserves_failed_process_report_and_sanitizes_aggregate(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / "fake.py"
            binary.write_text('''#!/usr/bin/env python3
import json,sys
from pathlib import Path
args=dict(zip(sys.argv[1::2],sys.argv[2::2]))
settings=json.loads(Path(args["--settings"]).read_text())
rows=[{"scenario":"cold","warmup":False,"pairIndex":0,"ok":False,"failure":"TIMEOUT","resolveMs":77},
{"scenario":"warm","warmup":False,"pairIndex":0,"ok":True,"resolvedRecordingIdentity":{"providerId":"src1"},"resolveMs":4}]
Path(args["--output"]).write_text(json.dumps({"schema":"burrata.production-resolver-diagnostic/v1","runPassed":False,"providerShutdownOk":True,"attempts":rows,"privateUrl":"https://secret.invalid/?x=1"}))
print("run failed")
sys.exit(1 if settings["matchingLevel"] == "exact" else 0)
''')
            binary.chmod(0o755)
            (root / "track.json").write_text('{"id":"track1","title":"Song"}')
            (root / "settings.json").write_text('{"preferredStreamingSource":"youtube"}')
            (root / "manifest.json").write_text(json.dumps({"cases": [{"caseId": "one", "track": "track.json",
                "settings": "settings.json", "expectedSourceId": "src1"}]}))
            out = root / "out"
            args = SimpleNamespace(manifest=str(root / "manifest.json"), binary=str(binary),
                                   outputdir=str(out), smoke=True, reps=20, warmups=3, levels=None,
                                   trace=False)
            result = cohort.run(args)
            exact = result["cases"][0]["modes"]["exact"]
            self.assertFalse(exact["runPassed"])
            self.assertEqual(exact["processExitCode"], 1)
            self.assertEqual(exact["scenarios"]["cold"]["failures"], 1)
            self.assertEqual(exact["expectedSourceIdMismatchCount"], 0)
            self.assertEqual(os.stat(out / ".one-exact-raw.json").st_mode & 0o777, 0o600)
            self.assertNotIn("secret.invalid", (out / "aggregate.json").read_text())

    def test_two_level_cohort_records_only_selected_modes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / "fake.py"
            binary.write_text('''#!/usr/bin/env python3
import json,sys
from pathlib import Path
args=dict(zip(sys.argv[1::2],sys.argv[2::2]))
settings=json.loads(Path(args["--settings"]).read_text())
Path(args["--output"]).write_text(json.dumps({"schema":"burrata.production-resolver-diagnostic/v1","runPassed":True,"providerShutdownOk":True,"attempts":[],"streamingPolicy":{"matchingLevel":settings["matchingLevel"]}}))
''')
            binary.chmod(0o755)
            (root / "track.json").write_text('{"id":"track1","title":"Song"}')
            (root / "settings.json").write_text('{}')
            (root / "manifest.json").write_text(json.dumps({"cases": [{"caseId": "two", "track": "track.json",
                "settings": "settings.json"}]}))
            args = SimpleNamespace(manifest=str(root / "manifest.json"), binary=str(binary),
                                   outputdir=str(root / "out"), smoke=True, reps=20, warmups=3,
                                   levels=["balanced", "flexible"], trace=False)
            result = cohort.run(args)
            self.assertEqual(result["frozen"]["levels"], ["balanced", "flexible"])
            self.assertEqual(list(result["cases"][0]["modes"]), ["balanced", "flexible"])
            self.assertNotIn("exact", result["cases"][0]["modes"])

    def test_fake_batch_invocation_retains_entry_and_shared_resource_summaries(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); binary = root / "fake.py"
            binary.write_text('''#!/usr/bin/env python3
import json,sys
from pathlib import Path
args=dict(zip(sys.argv[1::2],sys.argv[2::2]))
tracks=[sys.argv[i+1] for i,x in enumerate(sys.argv[:-1]) if x=="--track"]
rows=[]
for scenario in ("cold","warm"):
 for entry in range(len(tracks)):
  rows.append({"scenario":scenario,"pairIndex":0,"batchId":scenario+"-0","batchEntryIndex":entry,"batchEntryCount":len(tracks),"warmup":False,"ok":True,"resolveMs":10+entry,"resolvedRecordingIdentity":{"providerId":"src"+str(entry)}})
Path(args["--output"]).write_text(json.dumps({"schema":"burrata.production-resolver-diagnostic/v1","runPassed":True,"providerShutdownOk":True,"attempts":rows,"batches":[{"scenario":"cold","warmup":False,"entryCount":len(tracks),"wallMs":20,"helperMaxRssBytes":1000,"instrumentedMetrics":{"rustHttpSendAttempts":7,"successfulYtDlpSpawns":2,"matchPrepareElapsedMs":1.5,"matchAssessElapsedMs":3.0,"matchPrepareCpuElapsedNs":1500000,"matchPrepareCpuObservations":3,"matchAssessCpuElapsedNs":200000,"matchAssessCpuObservations":3}}]}))
''')
            binary.chmod(0o755)
            for name in ("a.json", "b.json"):
                (root / name).write_text('{"id":"' + name + '","title":"Song"}')
            (root / "settings.json").write_text('{}')
            (root / "manifest.json").write_text(json.dumps({"cases":[{"caseId":"batch","tracks":["a.json","b.json"],"settings":"settings.json"}]}))
            args = SimpleNamespace(manifest=str(root / "manifest.json"), binary=str(binary), outputdir=str(root / "out"),
                                   smoke=True, reps=20, warmups=3, levels=["balanced"], trace=False)
            result = cohort.run(args)
            mode = result["cases"][0]["modes"]["balanced"]
            self.assertEqual(result["cases"][0]["batchEntryCount"], 2)
            self.assertEqual(mode["perBatchEntry"]["1"]["warm"]["successes"], 1)
            self.assertEqual(mode["batches"][0]["helperMaxRssBytes"], 1000)
            self.assertEqual(mode["batchSummary"]["cold"]["wallMs"]["p50"], 20)
            self.assertEqual(mode["batchSummary"]["cold"]["instrumentedMetricTotals"]["rustHttpSendAttempts"], 7)
            self.assertEqual(mode["batchSummary"]["cold"]["instrumentedMetricDistributions"]["rustHttpSendAttempts"]["p50"], 7)
            self.assertEqual(mode["batchSummary"]["cold"]["instrumentedMetricTotals"]["matchPrepareCpuObservations"], 3)
            self.assertEqual(mode["batchSummary"]["cold"]["instrumentedMetricDistributions"]["matchPrepareCpuElapsedNs"]["p50"], 1500000)
            self.assertIn("lifetime high-water", mode["batchSummary"]["cold"]["resourceScope"])


if __name__ == "__main__":
    unittest.main()
