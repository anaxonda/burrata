import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

from seek_durability import POSITIONS, run, validate_tracks
from serial_same_worker import HERE


class SeekDurabilityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.worker = self.root / "fake_worker.py"
        self.manifest = json.loads((HERE / "fixtures/transport-music-tracks.json").read_text(encoding="utf-8"))
        metadata = {row["videoId"]: {"title": row["titleNeedle"], "channel": row["channelNeedle"],
                                     "durationSeconds": row["durationSeconds"]} for row in self.manifest}
        self.worker.write_text(
            "import json,os,sys\n"
            f"metadata=json.loads({json.dumps(json.dumps(metadata))})\n"
            "for line in sys.stdin:\n"
            " r=json.loads(line)\n"
            " if r.get('fullRecording') and os.environ.get('BURRATA_BENCH_FULL_RECORDING')!='1': raise SystemExit(9)\n"
            " m=metadata[r['videoId']]\n"
            " print(json.dumps({'id':r['id'],'ok':True,'formatId':251,'codec':'audio/webm; codecs=\\\"opus\\\"',\n"
            " 'decodedAudioMs':r['seconds']*1000,'providerMetadata':{'videoId':r['videoId'],**m}}),flush=True)\n",
            encoding="utf-8")

    def tearDown(self):
        self.temp.cleanup()

    def test_three_track_seek_positions_are_bounded_and_serial(self):
        output = self.root / "seek.json"
        result = run(output, self.manifest, node=Path(sys.executable), worker=self.worker,
                     repetitions=1, outer_s=10, request_s=1)
        self.assertEqual(result["status"], "complete")
        self.assertEqual(len(result["attempts"]), 9)
        self.assertTrue(all(row["durabilityEligible"] for row in result["attempts"]))
        self.assertEqual([row["positionLabel"] for row in result["attempts"][:3]], [p[0] for p in POSITIONS])
        self.assertEqual([row["positionSeconds"] for row in result["attempts"][:3]], [2.28, 114.0, 216.6])
        self.assertTrue(all(row["formatMatched"] and row["codecMatched"] and row["metadataMatched"]
                            for row in result["attempts"]))

    def test_full_recording_mode_is_explicit_and_checks_pcm_duration(self):
        output = self.root / "full.json"
        result = run(output, self.manifest, node=Path(sys.executable), worker=self.worker,
                     repetitions=1, full_recording=True, outer_s=10, request_s=1)
        self.assertEqual(result["mode"], "full-source-decode")
        self.assertEqual(len(result["attempts"]), 3)
        self.assertTrue(all(row["durabilityEligible"] and row["fullPcmDurationMatched"]
                            for row in result["attempts"]))
        self.assertEqual([row["decodedAudioMs"] for row in result["attempts"]], [228000, 251000, 288000])

    def test_manifest_full_source_limit_and_repetition_bounds(self):
        original = json.loads(json.dumps(self.manifest))
        too_long = json.loads(json.dumps(original)); too_long[0]["durationSeconds"] = 601
        with self.assertRaises(ValueError):
            validate_tracks(too_long)
        with self.assertRaises(ValueError):
            run(self.root / "unused.json", original, node=Path(sys.executable), worker=self.worker,
                repetitions=21, outer_s=10, request_s=1)


if __name__ == "__main__":
    unittest.main()
