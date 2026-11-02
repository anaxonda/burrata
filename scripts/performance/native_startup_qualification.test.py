import importlib.util
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('native_startup_qualification.py')
SPEC = importlib.util.spec_from_file_location('native_startup_qualification', SCRIPT)
QUAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(QUAL)
CAPTURE_SPEC = importlib.util.spec_from_file_location('native_runtime_capture', Path(__file__).with_name('native_runtime_capture.py'))
CAPTURE = importlib.util.module_from_spec(CAPTURE_SPEC)
CAPTURE_SPEC.loader.exec_module(CAPTURE)


class StartupQualificationTests(unittest.TestCase):
    def test_percentile_uses_nearest_rank(self):
        self.assertEqual(QUAL.percentile(list(range(1, 21)), .95), 19)

    def test_warmups_are_counted_but_excluded_from_measured_statistics(self):
        rows = []
        for pending in (0, 100):
            for index in range(1, 24):
                role = 'warmup' if index <= 3 else 'measured'
                measured_index = index - 3
                value = 1000 if role == 'warmup' else measured_index
                rows.append({
                    'build': 'baseline', 'pendingQualityAssets': pending,
                    'sampleRole': role, 'appLaunchToAutomationMs': value,
                    'firstAudioMs': value, 'capturePassed': True, 'pcmPassed': True,
                    'requiredMetricGate': {'missingMetrics': []}, 'binarySha256': 'fixture-hash',
                })
        result = QUAL.summarize(rows, ['baseline'], measured_runs=20, warmups=3)['baseline']['0']
        self.assertEqual(result['runCount'], 20)
        self.assertEqual(result['warmupCount'], 3)
        self.assertEqual(result['measures']['firstAudioMs']['medianMs'], 10.5)
        self.assertEqual(result['measures']['firstAudioMs']['p95Ms'], 19)
        self.assertTrue(result['allCapturesPassed'])

    def test_missing_first_audio_fails_even_if_pcm_passed(self):
        gate = QUAL.required_metric_gate(120.0, None, None, True, 0)
        self.assertFalse(gate['passed'])
        self.assertIn('firstAudioMs', gate['missingMetrics'])
        self.assertIn('firstAudioGeneration', gate['missingMetrics'])

    def test_no_pcm_fails_even_when_startup_metrics_are_present(self):
        gate = QUAL.required_metric_gate(120.0, 250.0, 9, False, 0)
        self.assertFalse(gate['passed'])
        self.assertFalse(gate['pcmPassed'])

    def test_wrong_track_first_audio_is_missing_for_target(self):
        runtime = {'samples': [
            {'trackId': 'other-track', 'firstAudioMs': 2.0, 'generation': 7},
        ]}
        self.assertEqual(QUAL.first_audio_observation(runtime, 'target-track'), (None, None))

    def test_startup_pcm_requires_real_tone_rms_and_no_clipping(self):
        bins = [{'firstLeft': .09, 'firstRight': .09, 'rms': .07} for _ in range(10)]
        result = {'bins': bins, 'firstToneSeconds': 2.5, 'stereoSeparated': True, 'peak': .5}
        self.assertTrue(CAPTURE.startup_pcm_gate(result)['passed'])
        self.assertFalse(CAPTURE.startup_pcm_gate({**result, 'bins': [], 'firstToneSeconds': 0})['passed'])
        self.assertFalse(CAPTURE.startup_pcm_gate({**result, 'peak': 1.0})['passed'])
        self.assertFalse(CAPTURE.startup_pcm_gate({**result, 'bins': [{**bins[0], 'rms': .001} for _ in bins]})['passed'])

    def test_existing_natural_preflight_is_reused_only_for_matching_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'usr/bin/burrata'
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'qualified baseline binary')
            evidence = root / 'natural.json'
            evidence.write_text(json.dumps({
                'mode': 'natural',
                'passed': True,
                'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                'runtime': {'passed': True},
                'pcm': {'passed': True},
            }))
            row = QUAL.existing_full_natural_check(root, 'baseline', evidence)
            self.assertTrue(row['passed'])
            binary.write_bytes(b'different binary')
            mismatch = QUAL.existing_full_natural_check(root, 'baseline', evidence)
            self.assertFalse(mismatch['passed'])

    def test_first_audio_is_bound_to_target_track_and_generation(self):
        runtime = {'samples': [
            {'trackId': 'other-track', 'firstAudioMs': 2.0, 'generation': 7},
            {'trackId': 'target-track', 'firstAudioMs': 18.5, 'generation': 9},
        ]}
        self.assertEqual(QUAL.first_audio_observation(runtime, 'target-track'), (18.5, 9))


if __name__ == '__main__':
    unittest.main()
