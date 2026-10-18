import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "scripts/performance/imported_playback_matrix.py"
spec = importlib.util.spec_from_file_location("imported_playback_matrix", SCRIPT)
matrix = importlib.util.module_from_spec(spec)
spec.loader.exec_module(matrix)


class MatrixFixtureTests(unittest.TestCase):
    def setUp(self):
        self.case = {
            "id": "mono-a", "trackId": "spotify:catalog-a",
            "expectedRecordingId": "monochrome:source-a",
            "expectedProvider": "monochrome", "expectedDurationSeconds": 245.48,
        }
        self.manifest = {
            "schemaVersion": 1, "automationFile": "/tmp/profile/automation.json",
            "binary": "/tmp/burrata", "binarySha256": "a" * 64,
            "version": "release-1", "cases": [self.case],
        }

    def test_manifest_rejects_arbitrary_execution_fields(self):
        bad = {**self.manifest, "cases": [{**self.case, "command": ["sh", "-c", "bad"]}]}
        with self.assertRaisesRegex(ValueError, "case_fields"):
            matrix.validate_manifest(bad)

    def test_manifest_rejects_duplicate_case_ids(self):
        bad = {**self.manifest, "cases": [self.case, dict(self.case)]}
        with self.assertRaisesRegex(ValueError, "duplicate_case_id"):
            matrix.validate_manifest(bad)

    def test_source_identity_is_checked_separately_from_active_track(self):
        right = {"native": {"trackId": "spotify:catalog-a", "sourceRecordingId": "monochrome:source-a"}}
        wrong = {"native": {"trackId": "spotify:catalog-a", "sourceRecordingId": "monochrome:wrong-source"}}
        absent = {"native": {"trackId": "spotify:catalog-a"}}
        self.assertTrue(matrix.source_identity_matches(self.case["expectedRecordingId"], right))
        self.assertFalse(matrix.source_identity_matches(self.case["expectedRecordingId"], wrong))
        self.assertFalse(matrix.source_identity_matches(self.case["expectedRecordingId"], absent))

    def test_summary_never_claims_acoustic_verification(self):
        report = {"initialState": {"sourceRecordingId": "monochrome:source-a"},
                  "warmups": [{"native": {"sourceRecordingId": "monochrome:wrong-source"}}],
                  "starts": [{"native": {"sourceRecordingId": "monochrome:source-a"}}],
                  "seeks": [], "failureCount": 1}
        result = matrix.summarize_case(self.case, report)
        self.assertEqual(result["wrongOrUnobservedSourceIdentityCount"], 1)
        self.assertFalse(result["acousticContentVerified"])


if __name__ == "__main__":
    unittest.main()
