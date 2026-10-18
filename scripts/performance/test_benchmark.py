import unittest
from benchmark import summarize

class SummaryTests(unittest.TestCase):
    def test_failures_remain_visible_and_p95_uses_nearest_rank(self):
        rows = [{'case':'a', 'success':True, 'firstAudioMs':n} for n in range(1,21)]
        rows.append({'case':'a', 'success':False, 'correctMatch':False})
        summary = summarize(rows)[0]
        self.assertEqual(summary['attempts'], 21)
        self.assertEqual(summary['failures'], 1)
        self.assertEqual(summary['incorrectMatches'], 1)
        self.assertEqual(summary['firstAudioMs'], {'median':10.5, 'p95':19, 'worst':20})
    def test_optional_measurements_exclude_missing_values(self):
        rows = [{'case':'a', 'success':True, 'ipcRequestBytes':10},
                {'case':'a', 'success':True, 'ipcRequestBytes':30},
                {'case':'a', 'success':False}]
        self.assertEqual(summarize(rows)[0]['ipcRequestBytes'],
                         {'median':20, 'p95':30, 'worst':30})


class RunnerTests(unittest.TestCase):
    def test_isolation_and_failed_decoding_are_recorded_without_urls(self):
        import json
        from pathlib import Path
        import subprocess
        import sys
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            adapter = root / 'adapter.py'
            adapter.write_text('''import json, os, sys
assert os.environ['XDG_DATA_HOME'].startswith(os.environ['BURRATA_BENCH_DATA_DIR'])
print(json.dumps(dict(firstAudioMs=12,decodedFrames=int(sys.argv[1]),continuedPlaybackMs=5000,correctMatch=True,url='secret-token',operationId=os.environ['BURRATA_BENCH_OPERATION_ID'],ipcRequestBytes=17)))
''')
            manifest = root / 'manifest.json'
            manifest.write_text(json.dumps([
                {'id':'decoded','mode':'cold','command':[sys.executable,str(adapter),'100']},
                {'id':'metadata-only','mode':'cold','command':[sys.executable,str(adapter),'0']},
            ]))
            output = root / 'results'
            subprocess.run([sys.executable, str(Path(__file__).with_name('benchmark.py')),
                            str(manifest), '--output', str(output), '--repetitions','1',
                            '--interval','0'], check=True)
            attempts = json.loads((output / 'attempts.json').read_text())
            self.assertEqual(sum(row['success'] for row in attempts), 1)
            self.assertTrue(all(row['operationId'] == row.get('adapterOperationId') for row in attempts))
            self.assertTrue(all(row['ipcRequestBytes'] == 17 for row in attempts))
            self.assertNotIn('secret-token', (output / 'attempts.json').read_text())

if __name__ == '__main__':
    unittest.main()
