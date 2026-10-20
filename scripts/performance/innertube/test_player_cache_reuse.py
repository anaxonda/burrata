import json
import sys
import tempfile
import unittest
from pathlib import Path

from player_cache_reuse import clear_session_cache, player_cache_inventory, run, session_cache_path
from serial_same_worker import HERE


class PlayerCacheReuseTests(unittest.TestCase):
    def test_session_entry_is_deleted_while_player_record_is_preserved(self):
        with tempfile.TemporaryDirectory() as td:
            data = Path(td)
            player_dir = data / "youtube-session"
            player_dir.mkdir()
            player = player_dir / "opaque-player-id"
            player.write_bytes(b"versioned-player")
            session = session_cache_path(data)
            session.write_bytes(b"transient-session")
            self.assertEqual(player_cache_inventory(data), (1, len(b"versioned-player")))
            self.assertTrue(clear_session_cache(data))
            self.assertFalse(session.exists())
            self.assertEqual(player.read_bytes(), b"versioned-player")
            self.assertEqual(player_cache_inventory(data), (1, len(b"versioned-player")))
            self.assertFalse(clear_session_cache(data))

    def test_fresh_worker_pair_retains_only_player_file(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            worker = root / "fake_worker.py"
            output = root / "result.json"
            worker.write_text(
                "import json,os,pathlib,sys\n"
                "data=pathlib.Path(os.environ['BURRATA_BENCH_DATA_DIR'])/'youtube-session'\n"
                "data.mkdir(parents=True,exist_ok=True)\n"
                "for line in sys.stdin:\n"
                " r=json.loads(line); (data/'innertube_session_data').write_bytes(b'secret-session')\n"
                " (data/'opaque-player-id').write_bytes(b'versioned-player')\n"
                " print(json.dumps({'id':r['id'],'ok':True,'formatId':251,'codec':'audio/webm; codecs=\\\"opus\\\"',\n"
                " 'decodedAudioMs':5000,'providerMetadata':{'videoId':'uZGRATckoBM','title':'John Denver - Matthew',\n"
                " 'channel':'John Denver','durationSeconds':228}}),flush=True)\n",
                encoding="utf-8")
            manifest = json.loads((HERE / "fixtures/transport-music-tracks.json").read_text(encoding="utf-8"))
            track = next(row for row in manifest if row["name"] == "matthew")
            result = run(output, track, node=Path(sys.executable), worker=worker,
                         pairs=1, outer_s=10, request_s=2)
            self.assertEqual(result["status"], "complete")
            self.assertEqual(len(result["attempts"]), 3)  # one cache warmup plus paired attempts
            warmup, first, second = result["attempts"]
            self.assertEqual(warmup["arm"], "shared_player_cache")
            shared = first if first["arm"] == "shared_player_cache" else second
            fresh = first if first["arm"] == "fresh_cache" else second
            self.assertTrue(warmup["transportEligible"])
            self.assertTrue(shared["transportEligible"])
            self.assertTrue(fresh["transportEligible"])
            self.assertEqual(shared["cachePlayerRecordsBefore"], 1)
            self.assertTrue(shared["sessionEntryAbsentBefore"])
            self.assertTrue(shared["sessionEntryRemovedAfter"])
            self.assertTrue(shared["sessionEntryAbsentAfter"])
            self.assertEqual(shared["cachePlayerRecordsAfter"], 1)
            self.assertEqual(fresh["cachePlayerRecordsBefore"], 0)
            self.assertEqual(len(result["driverSha256"]), 64)


if __name__ == "__main__":
    unittest.main()
