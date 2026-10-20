"""Offline subprocess fixtures for the cold-player A/B driver; no provider calls."""
import hashlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import cold_player_overlap as driver


FIXTURE = '''import json,os,sys,time
from pathlib import Path
request=json.loads(sys.stdin.readline())
mode='parallel-player' if os.environ['BURRATA_BENCH_PARALLEL_PLAYER']=='1' else 'sequential'
base=Path(__file__).parent
with (base/'attempts.jsonl').open('a') as log:
 log.write(json.dumps({'mode':mode,'pid':os.getpid(),'request':request,'home':os.environ.get('HOME'),
  'data':os.environ['BURRATA_BENCH_DATA_DIR'],'config':os.environ['XDG_CONFIG_HOME'],
  'cache':os.environ['XDG_CACHE_HOME'],'state':os.environ['XDG_DATA_HOME']})+'\\n')
response={'id':request['id'],'ok':True,'clientName':'MWEB','accountingComplete':True,
 'headerNames':['user-agent'],'formatId':251,'codec':'audio/webm; codecs="opus"',
 'decodedAudioMs':5000,'firstPcmFromCommandMs':80 if mode=='parallel-player' else 100,
 'providerMetadata':{'videoId':request['videoId'],'title':'Matthew','channel':'John Denver','durationSeconds':228},
 'playbackPreparation':{'mode':mode,'readyMs':10}}
BEHAVIOR
print(json.dumps(response),flush=True)
# Stay alive after a valid response so every attempt exercises group cleanup.
sys.stdin.read()
'''


@unittest.skipUnless(os.name == 'posix', 'driver uses POSIX process groups')
class ColdPlayerDriverTests(unittest.TestCase):
    def test_source_hashes_support_python310_without_file_digest_and_preserve_exact_bytes(self):
        with tempfile.TemporaryDirectory(prefix='cold-player-hashes-') as scratch:
            source = Path(scratch) / 'fixture-tool'
            for payload in [b'', b'exact\x00bytes\r\n', bytes(range(256)) * 8193]:
                source.write_bytes(payload)
                with patch.object(hashlib, 'file_digest', create=True,
                                  side_effect=AssertionError('file_digest requires Python 3.11')), \
                        patch.object(driver, 'SOURCE_FILES', [source]):
                    self.assertEqual(driver.hashes(), {source.name: hashlib.sha256(payload).hexdigest()})

    def exercise(self, directory, behavior='pass'):
        worker = directory / 'worker.mjs'
        worker.write_text(FIXTURE.replace('BEHAVIOR', behavior))
        output = directory / 'report.json'
        processes = []
        original_cleanup = driver.terminate_group

        def cleanup(process):
            original_cleanup(process, grace=0.01)
            processes.append(process)

        with patch.multiple(driver, HERE=directory, NODE=Path(sys.executable),
                            FFMPEG=Path(sys.executable), SOURCE_FILES=[Path(driver.__file__), worker]), \
                patch.object(driver, 'preflight', return_value={'ok': True, 'checks': {'offlineFixture': True}}), \
                patch.object(driver, 'terminate_group', side_effect=cleanup):
            report = driver.run(output)
        self.assertEqual(len(processes), 6)
        self.assertTrue(all(process.poll() is not None for process in processes))
        self.assertTrue(all(row['cleanupCompleted'] for row in report['requests']))
        self.assertEqual(json.loads(output.read_text()), report)
        return report, output

    def test_six_fresh_attempts_use_ab_ba_ab_and_form_three_valid_pairs(self):
        with tempfile.TemporaryDirectory(prefix='cold-player-offline-') as scratch:
            directory = Path(scratch)
            report, _ = self.exercise(directory)
            attempts = [json.loads(line) for line in (directory / 'attempts.jsonl').read_text().splitlines()]
            expected = ['sequential', 'parallel-player', 'parallel-player', 'sequential',
                        'sequential', 'parallel-player']
            self.assertEqual([row['arm'] for row in report['requests']], expected)
            self.assertEqual([row['mode'] for row in attempts], expected)
            self.assertEqual(len({row['pid'] for row in attempts}), 6)
            for key in ('data', 'config', 'cache', 'state'):
                self.assertEqual(len({row[key] for row in attempts}), 6)
            self.assertTrue(all(row['home'] == os.environ.get('HOME') for row in attempts))
            for row in attempts:
                self.assertEqual(row['request']['videoId'], driver.VIDEO)
                self.assertEqual(row['request']['expectedVideoId'], driver.VIDEO)
                self.assertEqual(row['request']['formatId'], 251)
                self.assertEqual(row['request']['seconds'], 5)
                self.assertGreater(row['request']['budgetMs'], 0)
                self.assertLessEqual(row['request']['budgetMs'], 20_000)
            self.assertEqual(report['status'], 'completed')
            self.assertFalse(report['qualification'])
            self.assertEqual(report['warmups'], 0)
            self.assertTrue(all(row['eligible'] for row in report['requests']))
            self.assertTrue(all(pair['eligible'] for pair in report['pairs']))
            self.assertEqual([pair['parallelMinusSequentialFirstPcmMs'] for pair in report['pairs']], [-20] * 3)

    def test_denials_are_retained_and_never_paired_or_overwritten(self):
        behavior = "if mode=='parallel-player': response.update(ok=False,failure='STREAM_DENIED')"
        with tempfile.TemporaryDirectory(prefix='cold-player-offline-') as scratch:
            report, output = self.exercise(Path(scratch), behavior)
            self.assertEqual(sum(row['eligible'] for row in report['requests']), 3)
            self.assertEqual(sum(row.get('failure') == 'STREAM_DENIED' for row in report['requests']), 3)
            self.assertTrue(all(not pair['eligible'] and pair['parallelMinusSequentialFirstPcmMs'] is None
                                for pair in report['pairs']))
            original = output.read_bytes()
            with patch.object(driver, 'preflight', side_effect=AssertionError('overwrite reached preflight')):
                with self.assertRaisesRegex(SystemExit, 'already exists'):
                    driver.run(output)
            self.assertEqual(output.read_bytes(), original)

    def test_returned_identity_mismatch_rejects_nominal_success(self):
        behavior = "if mode=='parallel-player': response['providerMetadata']['videoId']='abcdefghijk'"
        with tempfile.TemporaryDirectory(prefix='cold-player-offline-') as scratch:
            report, _ = self.exercise(Path(scratch), behavior)
            treatment = [row for row in report['requests'] if row['arm'] == 'parallel-player']
            self.assertTrue(all(row['ok'] and not row['metadataMatched'] and not row['eligible']
                                for row in treatment))
            self.assertTrue(all(not pair['eligible'] for pair in report['pairs']))

    def test_oversized_frame_and_wrong_response_id_are_bounded_protocol_failures(self):
        behavior = """if mode=='sequential':
 sys.stdout.write('x'*70000+'\\n');sys.stdout.flush();sys.stdin.read();sys.exit()
else: response['id']='wrong-request'"""
        with tempfile.TemporaryDirectory(prefix='cold-player-offline-') as scratch:
            report, _ = self.exercise(Path(scratch), behavior)
            self.assertEqual(len(report['requests']), 6)
            self.assertTrue(all(row['status'] == 'driver_failed' and row['failure'] == 'PROTOCOL_ERROR'
                                and not row['eligible'] for row in report['requests']))
            self.assertTrue(all(not pair['eligible'] for pair in report['pairs']))
            self.assertLess(len(json.dumps(report)), 12_000)


if __name__ == '__main__':
    unittest.main()
