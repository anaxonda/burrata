import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

from serial_same_worker import (BoundedLineReader, HERE, NODE, ROOT, WORKER, ProtocolError,
                                preflight, run, safe_response, terminate_group, validate_tracks)


class ProtocolTests(unittest.TestCase):
    def test_real_default_paths_and_bundled_node_preflight_without_worker(self):
        self.assertEqual(ROOT, Path(__file__).resolve().parents[3])
        self.assertTrue(NODE.is_file())
        self.assertTrue(WORKER.is_file())
        result = preflight()
        self.assertTrue(result["ok"], result)
        self.assertTrue(all(result["checks"].values()), result)

    def test_tracked_public_music_manifest_has_three_unique_strict_pins(self):
        tracks = json.loads((HERE / "fixtures/transport-music-tracks.json").read_text(encoding="utf-8"))
        validate_tracks(tracks)
        self.assertEqual([row["formatId"] for row in tracks], [251, 251, 251])
        self.assertEqual(len({row["videoId"] for row in tracks}), 3)

    def test_partial_frame_and_buffered_following_frame(self):
        read_fd, write_fd = os.pipe()
        reader = BoundedLineReader(read_fd)
        def write_chunks():
            os.write(write_fd, b'{"id":')
            time.sleep(0.02)
            os.write(write_fd, b'"one"}\n{"id":"two"}\n')
        thread = threading.Thread(target=write_chunks)
        thread.start()
        try:
            deadline = time.monotonic() + 1
            self.assertEqual(reader.read(deadline), b'{"id":"one"}')
            self.assertEqual(reader.read(deadline), b'{"id":"two"}')
        finally:
            thread.join()
            os.close(read_fd)
            os.close(write_fd)

    def test_oversized_frame_is_rejected(self):
        read_fd, write_fd = os.pipe()
        try:
            os.write(write_fd, b"x" * 100 + b"\n")
            with self.assertRaises(ProtocolError):
                BoundedLineReader(read_fd).read(time.monotonic() + 1, max_bytes=64)
        finally:
            os.close(read_fd)
            os.close(write_fd)

    def test_response_allowlist_drops_urls_and_tokens(self):
        raw = json.dumps({"id": "a", "ok": True, "formatId": 251,
                          "url": "https://signed.example/?secret=x", "poToken": "secret",
                          "stages": {"token": 1.2}}).encode()
        safe = safe_response(raw, "a")
        self.assertEqual(safe, {"id": "a", "ok": True, "formatId": 251,
                                "stages": {"token": 1.2}})

    def test_driver_sends_each_request_after_previous_response(self):
        with tempfile.TemporaryDirectory() as td:
            worker = Path(td) / "fake_worker.py"
            output = Path(td) / "result.json"
            worker.write_text(
                "import json,sys,time\n"
                "for line in sys.stdin:\n"
                " r=json.loads(line)\n"
                " time.sleep(.01)\n"
                " print(json.dumps({'id':r['id'],'ok':True,'formatId':r['formatId'],'codec':'audio/webm; codecs=\\\"opus\\\"'}),flush=True)\n",
                encoding="utf-8")
            tracks = [{"name": n, "videoId": vid, "formatId": 251,
                       "titleNeedle": n, "channelNeedle": n, "durationSeconds": 100,
                       "codec": 'audio/webm; codecs="opus"'} for n, vid in
                      [("alpha", "abcdefghijk"), ("bravo", "bcdefghijkl"), ("charlie", "cdefghijklm")]]
            result = run(output, tracks, node=Path(sys.executable), worker=worker,
                         total_s=5, request_s=1)
            self.assertEqual([r["status"] for r in result["requests"]], ["completed"] * 9)
            self.assertTrue(all("requestReceiptToResponseMs" in r for r in result["requests"]))
            self.assertEqual([r["role"] for r in result["requests"][:3]],
                             ["cold", "same_id_warm", "different_id_warm"])
            self.assertEqual([r["role"] for r in result["requests"][3:6]],
                             ["cold", "same_id_warm", "different_id_warm"])
            self.assertTrue(all(r["formatMatched"] for r in result["requests"]))
            self.assertEqual(len(json.loads(output.read_text())["requests"]), 9)

    def test_spawn_failure_writes_structured_artifact(self):
        with tempfile.TemporaryDirectory() as td:
            output = Path(td) / "failure.json"
            tracks = [{"name": n, "videoId": vid, "formatId": 251,
                       "titleNeedle": n, "channelNeedle": n, "durationSeconds": 100,
                       "codec": 'audio/webm; codecs="opus"'} for n, vid in
                      [("alpha", "abcdefghijk"), ("bravo", "bcdefghijkl"), ("charlie", "cdefghijklm")]]
            result = run(output, tracks, node=Path(td) / "missing-node", total_s=1, request_s=0.5)
            self.assertEqual(result["status"], "complete")
            self.assertTrue(all(row["status"] == "startup_failed" for row in result["requests"]))
            self.assertEqual(len(json.loads(output.read_text())["requests"]), 9)

    @unittest.skipUnless(os.name == "posix", "process groups are POSIX-specific")
    def test_cleanup_kills_descendant_after_group_leader_exits(self):
        with tempfile.TemporaryDirectory() as td:
            pid_file = Path(td) / "child.pid"
            code = "import subprocess,sys; c=subprocess.Popen(['sleep','30']); open(sys.argv[1],'w').write(str(c.pid))"
            leader = subprocess.Popen([sys.executable, "-c", code, str(pid_file)], start_new_session=True)
            leader.wait(timeout=2)
            child_pid = int(pid_file.read_text())
            terminate_group(leader, grace=0.05)
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    break
                # A killed orphan may briefly remain a zombie until reaped.
                stat = Path(f"/proc/{child_pid}/stat")
                if stat.exists() and stat.read_text().split()[2] == "Z":
                    break
                time.sleep(0.02)
            else:
                self.fail("process-group cleanup left descendant running")


if __name__ == "__main__":
    unittest.main()
