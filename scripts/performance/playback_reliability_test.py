import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen

DIRECTORY = Path(__file__).resolve().parent
sys.path.insert(0, str(DIRECTORY))
from playback_reliability import SCHEMA, Worker, run_command, summary, validate_native_rows
from playback_reliability_fixture import FixtureServer
from playback_reliability_acoustic import fingerprint_alignment, sample_alignment
from playback_reliability_prewarm_ab import compare


class ReliabilityTests(unittest.TestCase):
    def test_summary_excludes_warmups_but_preserves_failures_and_tail(self):
        rows = [dict(strategy='a', scenario='seek', warmup=False, ok=True, firstPcmMs=i) for i in range(1, 21)]
        rows += [dict(strategy='a', scenario='seek', warmup=True, ok=True, firstPcmMs=9999),
                 dict(strategy='a', scenario='seek', warmup=False, ok=False, failure='TIMEOUT')]
        group, = summary(rows)
        self.assertEqual(group['attempts'], 21)
        self.assertEqual(group['successes'], 20)
        self.assertEqual(group['firstPcmMs'], dict(median=10.5, p95=19, worst=20))
        self.assertEqual(group['failures'], {'TIMEOUT': 1})

    def test_range_fixture_serves_identical_bytes_and_injects_denial(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'fixture'
            path.write_bytes(bytes(range(256)))
            server = FixtureServer(path, 'deny-first')
            try:
                with self.assertRaises(HTTPError) as context:
                    urlopen(server.url)
                self.assertEqual(context.exception.code, 403)
                context.exception.close()
                with urlopen(Request(server.url, headers={'Range': 'bytes=12-19'})) as response:
                    self.assertEqual(response.status, 206)
                    self.assertEqual(response.read(), path.read_bytes()[12:20])
                    self.assertEqual(response.headers['Content-Range'], 'bytes 12-19/256')
                self.assertEqual(server.requests[1]['advertisedBytes'], 8)
            finally:
                server.close()

    def test_wait4_accounts_for_real_child_cpu_and_rss(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_command([sys.executable, '-c', 'sum(i*i for i in range(1000000))'], Path(directory), dict(__import__('os').environ), 5)
            self.assertTrue(result['ok'])
            self.assertGreater(result['childCpuMs'], 0)
            self.assertGreater(result['childMaxRssBytes'], 0)
            self.assertEqual(result['accounting'], 'wait4-reaped-command-tree')

    def test_native_report_must_include_each_strategy_position_and_repetition(self):
        rows = [dict(strategy=strategy, iteration=i, fraction=fraction, firstPcmMs=1, wallMs=2, decodeThreadCpuMs=0)
                for strategy in ['ffmpegFresh', 'opusRetained'] for i in range(20) for fraction in [.01, .5, .95]]
        self.assertTrue(validate_native_rows(rows, True))
        self.assertFalse(validate_native_rows(rows[:-1], True))
        self.assertFalse(validate_native_rows(rows, False))
        rows[-1] = rows[0]
        self.assertFalse(validate_native_rows(rows, True))

    def test_fingerprint_alignment_finds_known_shift(self):
        import random
        generator = random.Random(42)
        left = [generator.getrandbits(32) for _ in range(100)]
        right = left[3:] + [generator.getrandbits(32) for _ in range(3)]
        result = fingerprint_alignment(left, right)
        self.assertEqual(result['shiftFrames'], 3)
        self.assertEqual(result['bitAgreement'], 1)

    def test_pcm_alignment_finds_offset_despite_gain_and_dc_change(self):
        import wave
        try:
            import numpy as np
        except ImportError:
            self.skipTest('optional existing NumPy analysis package unavailable')
        random = np.random.default_rng(42)
        left = random.normal(0, 2000, 32000).astype('<i2')
        right = np.concatenate((left[200:].astype(float)*1.1+17, random.normal(0,2000,200))).astype('<i2')
        with tempfile.TemporaryDirectory() as directory:
            paths = [Path(directory)/name for name in ['left.wav','right.wav']]
            for path, values in zip(paths, [left,right]):
                with wave.open(str(path),'wb') as stream:
                    stream.setnchannels(1);stream.setsampwidth(2);stream.setframerate(11025);stream.writeframes(values.tobytes())
            result = sample_alignment(*paths,max_offset_seconds=.1)
            self.assertEqual(result['shiftSamples'],200)
            self.assertGreater(result['absoluteCorrelation'],.999)

    def test_pcm_rate_search_recovers_known_uniform_drift(self):
        import wave
        try:
            import numpy as np
        except ImportError:
            self.skipTest('optional existing NumPy analysis package unavailable')
        generator = np.random.default_rng(7)
        left = np.convolve(generator.normal(0, 3000, 32000), np.ones(5)/5, mode='same')
        right = np.interp(np.arange(0,len(left),1/1.005),np.arange(len(left)),left)
        with tempfile.TemporaryDirectory() as directory:
            paths = [Path(directory)/name for name in ['left.wav','right.wav']]
            for path, values in zip(paths,[left,right]):
                with wave.open(str(path),'wb') as stream:
                    stream.setnchannels(1);stream.setsampwidth(2);stream.setframerate(11025);stream.writeframes(values.astype('<i2').tobytes())
            result=sample_alignment(*paths,max_offset_seconds=.1,rate_search=True)
            self.assertGreater(result['absoluteCorrelation'],.97)
            self.assertAlmostEqual(result['rightSampleStep'],1.005,delta=.00005)

    def test_prewarm_comparison_excludes_failed_and_different_source_pairs(self):
        def row(pair,mode,video,ok,time):
            return dict(pair=pair,mode=mode,identity={'provider':'youtube','providerId':video},ok=ok,firstPcmFromRequestMs=time,resolveMs=time-100)
        result=compare([row(1,'baseline','a',True,10000),row(1,'prewarm','a',True,8000),
                        row(2,'baseline','b',True,12000),row(2,'prewarm','c',True,7000),
                        row(3,'baseline','a',False,15000),row(3,'prewarm','a',True,9000)])
        self.assertEqual(result['matchedPairs'],1)
        self.assertEqual(result['medianFirstPcmMsSaved'],2000)
        self.assertIsNone(result['p95'])

    def test_command_timeout_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_command([sys.executable, '-c', 'import time; time.sleep(10)'], Path(directory), dict(__import__('os').environ), .05)
            self.assertFalse(result['ok'])
            self.assertEqual(result['failure'], 'COMMAND_TIMEOUT')
            self.assertLess(result['wallMs'], 1500)


if __name__ == '__main__':
    unittest.main()
