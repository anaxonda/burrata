import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("matching_stage5.py")
SPEC = importlib.util.spec_from_file_location("matching_stage5", MODULE_PATH)
stage5 = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(stage5)


def row(run_id: str, condition: str, resolution: float, status: str = "success") -> dict:
    return {
        "schemaVersion": 1,
        "runId": run_id,
        "pairId": "pair-one",
        "caseId": "fixture-one",
        "operation": "playback",
        "condition": condition,
        "mode": "Balanced",
        "stage": "Resolved",
        "frozenBuild": {
            "gitCommit": "0123456789abcdef",
            "artifactSha256": "a" * 64,
            "evaluatorVersion": "1",
            "evidenceVersion": "1",
            "policyVersion": "1",
            "platform": "linux-x86_64",
        },
        "requestIdentity": {"provider": "spotify", "providerId": "request-1", "title": "Song", "artist": "Artist"},
        "candidateIdentity": {"provider": "youtube", "id": "video-1"},
        "outcome": {"status": status, "failureStage": "extraction" if status == "failure" else ""},
        "timingsMs": {"resolutionMs": resolution, "firstAudioMs": resolution + 100},
        "counts": {"cacheHits": 1, "externalRequests": 2, "extractionAttempts": 1},
    }


class Stage5HarnessTests(unittest.TestCase):
    def test_matrix_has_every_mode_and_stage(self):
        data = stage5.validate_matrix(stage5.DEFAULT_MATRIX)
        self.assertEqual(len(data["cases"]), 14)

    def test_matrix_declares_transformations_and_explicit_proof_targets(self):
        data = stage5.validate_matrix(stage5.DEFAULT_MATRIX)
        tests = {item["id"]: item for item in data["metamorphicTests"]}
        self.assertEqual(len(tests), 4)
        self.assertEqual(tests["increasing-mode-permissiveness"]["sourceCaseId"], "allCases")
        self.assertEqual(tests["unicode-canonical-form"]["evidenceBasis"], "syntheticPolicyProbe")
        proof_values = {
            cell["proofStrength"]
            for case in data["cases"]
            for stage in case["expectations"].values()
            for cell in stage.values()
        }
        self.assertEqual(proof_values, {"metadataOnly"})

    def test_summary_keeps_failures_and_suppresses_small_sample_p95(self):
        with tempfile.TemporaryDirectory() as temporary:
            observations = Path(temporary) / "runs.jsonl"
            observations.write_text("\n".join(json.dumps(item) for item in (
                row("cold-1", "cold", 100, "failure"),
                row("warm-1", "warm", 80),
            )) + "\n", encoding="utf-8")
            text, summary = stage5.summarize(observations)
            self.assertIn("| playback | Resolved | Balanced | cold | spotify | youtube | 1 | 0 | 1 | 100.0/suppressed | 200.0/suppressed |", text)
            self.assertIn("p95 is suppressed below n=20", text)
            self.assertIsNone(summary["groups"][0]["resolutionP95Ms"])
            self.assertTrue(summary["groups"][0]["resolutionP95Suppressed"])
            self.assertEqual(summary["groups"][0]["failure"], 1)
            self.assertEqual(summary["pairIssues"], [])

    def test_pairing_rejects_different_candidate_identity(self):
        cold = row("cold-1", "cold", 100)
        warm = row("warm-1", "warm", 80)
        warm["candidateIdentity"]["id"] = "different-video"
        with tempfile.TemporaryDirectory() as temporary:
            observations = Path(temporary) / "runs.jsonl"
            observations.write_text(json.dumps(cold) + "\n" + json.dumps(warm) + "\n", encoding="utf-8")
            _, summary = stage5.summarize(observations)
            self.assertEqual(len(summary["pairIssues"]), 1)
            self.assertIn("candidate", summary["pairIssues"][0])


if __name__ == "__main__":
    unittest.main()
