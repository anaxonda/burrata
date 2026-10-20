"""Offline plans, seeded SQLite, redaction and fake-attempt tests; no app/provider."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import next_preparation as driver


@unittest.skipUnless(sys.platform == 'linux', 'Linux isolated launcher and process accounting fixtures')
class NextPreparationTests(unittest.TestCase):
    def test_windows_and_macos_discovery_imports_skip_before_linux_fixture_execution(self):
        script = r'''
import builtins, runpy, subprocess, sys, unittest, unittest.mock
# Warm the host's standard-library imports before simulating another platform.
# In particular, Linux Python3.14 pathlib itself may import fcntl internally.
import next_preparation
del sys.modules['next_preparation']
original_import = builtins.__import__
def portable_import(name, *args, **kwargs):
    if name == 'fcntl':
        raise AssertionError('fcntl imported during non-Linux test discovery')
    return original_import(name, *args, **kwargs)
builtins.__import__ = portable_import
for platform in ['win32', 'darwin']:
    sys.platform = platform
    namespace = runpy.run_path(sys.argv[1], run_name='next_preparation_portability_probe')
    case = namespace['NextPreparationTests']
    assert case.__unittest_skip__
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(case)
    result = unittest.TestResult()
    suite.run(result)
    assert result.wasSuccessful() and result.testsRun == len(result.skipped)
print('non-Linux discovery imports and skips safely')
'''
        result = subprocess.run([sys.executable, '-c', script, __file__], capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn(b'non-Linux discovery imports and skips safely', result.stdout)

    def test_default_plan_is_three_rotated_pairs_and_metric_requires_explicit_twenty(self):
        self.assertEqual([row['windowSeconds'] for row in driver.plan()], [30, 45, 45, 30, 30, 45])
        metric = driver.plan('metric')
        self.assertEqual(len(metric), 40)
        self.assertEqual(sum(row['windowSeconds'] == 45 for row in metric), 20)
        with self.assertRaises(ValueError): driver.plan('unbounded')
        completed = subprocess.run([sys.executable, str(Path(driver.__file__))], capture_output=True, timeout=5, check=True)
        report = json.loads(completed.stdout)
        self.assertEqual(report['mode'], 'plan-only'); self.assertFalse(report['promoted'])
        self.assertEqual(len(report['schedule']), 6)

    def test_isolated_seed_is_only_a_ninety_second_control_and_explicit_stable_video(self):
        with tempfile.TemporaryDirectory(prefix='burrata-launch.', dir='/tmp') as directory:
            profile = Path(directory)
            track = driver.select_track(driver.MANIFEST, 'matthew')
            manifest = driver.seed_attempt(profile, track)
            spec = json.loads(manifest.read_text())
            self.assertEqual(spec['localDurationSeconds'], 90)
            self.assertEqual(spec['nextTrackId'], 'youtube:'+track['videoId'])
            with sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite') as connection:
                rows = connection.execute('SELECT provider,source_url,metadata FROM catalog_listings').fetchall()
                self.assertEqual(len(rows), 2)
                local = next(json.loads(row[2]) for row in rows if row[0] == 'local')
                remote = next(row for row in rows if row[0] == 'youtube')
                self.assertEqual(local['durationSeconds'], 90)
                self.assertEqual(remote[1], 'https://www.youtube.com/watch?v='+track['videoId'])
                self.assertNotIn('sig=', remote[2]); self.assertEqual(json.loads(remote[2])['durationSeconds'], track['durationSeconds'])
                self.assertEqual(connection.execute('SELECT COUNT(*) FROM settings').fetchone()[0], 1)

    def test_trace_correlates_the_post_seek_owner_and_rejects_ignored_window_or_failed_prep(self):
        operation = 'preparation:11111111-1111-1111-1111-111111111111'
        def event(phase, at, **details): return dict(phase=phase, operationId=operation, atUnixMs=at, details=details)
        events = [event('next_preparation_policy', 100, generation=4, windowSeconds=45),
                  event('next_preparation_resolve_start', 15100), event('next_preparation_resolve_end', 20100, ok=True),
                  event('next_preparation_queued', 20100, ok=True), event('next_preparation_adopted', 60100)]
        row = dict(windowSeconds=45, controlGeneration=4, seekSettledAtUnixMs=100, originalFailure=None,
                   samples=[dict(trackId='local:fixture', atUnixMs=15100, positionSeconds=45)])
        driver.attach_preparation(row, events)
        self.assertEqual(row['preparationRemainingEstimateSeconds'], 45)
        self.assertEqual(row['preparedReaderQueuedAgeAtAdoptionMs'], 40_000)
        self.assertIsNone(row['signedUrlExpiryAtAdoption']); self.assertIsNone(row['originalFailure'])
        ignored = {**row, 'windowSeconds': 30}; driver.attach_preparation(ignored, events)
        self.assertEqual(ignored['originalFailure'], 'diagnostic_window_mismatch')
        failed = {**row}; driver.attach_preparation(failed, events+[event('next_preparation_resolve_end', 22000, ok=False)])
        self.assertEqual(failed['originalFailure'], 'original_preparation_failed')
        missing = {**row}; driver.attach_preparation(missing, [])
        self.assertEqual(missing['originalFailure'], 'preparation_trace_missing')

    def test_safe_trace_discards_urls_ids_and_unapproved_details(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'app.log'
            event = dict(phase='next_preparation_policy', nativeOperationId='preparation:11111111-1111-1111-1111-111111111111',
                         atUnixMs=100, url='https://private/?sig=secret', details=dict(windowSeconds=45, generation=4, cookie='secret'))
            path.write_text('burrata_playback_phase '+json.dumps(event)+'\n')
            self.assertNotIn('secret', json.dumps(driver.safe_trace(path)))
            self.assertEqual(driver.safe_trace(path)[0]['details'], dict(windowSeconds=45, generation=4))

    def test_original_failure_and_expiry_denials_are_preserved_without_harness_refresh(self):
        operation = 'preparation:11111111-1111-1111-1111-111111111111'
        def event(phase, **details): return dict(phase=phase, operationId=operation, atUnixMs=100, details=details)
        row = dict(windowSeconds=30, controlGeneration=4, seekSettledAtUnixMs=100,
                   originalFailure='original_playback_failure', transitionOk=True)
        driver.attach_preparation(row, [event('next_preparation_policy', generation=4, windowSeconds=30),
            event('next_preparation_resolve_end', ok=False), event('stream_probe_end', status=403),
            event('stream_probe_end', status=410)])
        self.assertEqual(row['originalFailure'], 'original_playback_failure')
        self.assertEqual(row['probeDeniedResponses'], 2)
        self.assertIsNone(row['signedUrlExpiryAtAdoption'])

    def test_failures_and_different_formats_are_retained_and_exploration_has_no_p95(self):
        rows = [{**row, 'eligible': True, 'sourceVideoId': 'abcdefghijk', 'selectedFormatId': '251',
                 'observedHandoffDelayMs': row['windowSeconds'], 'status': 'completed'} for row in driver.plan()]
        rows[1].update(selectedFormatId='140')
        rows[3].update(eligible=False, originalFailure='original_preparation_failed')
        result = driver.summarize(rows, 'exploratory')
        self.assertEqual([pair['eligible'] for pair in result['pairs']], [False, False, True])
        self.assertTrue(all(group['handoffP95Ms'] is None for group in result['groups']))
        self.assertEqual(result['groups'][0]['failures'], {'original_preparation_failed': 1})
        metric = [{**row, 'eligible': True, 'sourceVideoId': 'abcdefghijk', 'selectedFormatId': '251',
                   'observedHandoffDelayMs': row['ordinal'], 'status': 'completed'} for row in driver.plan('metric')]
        self.assertTrue(all(group['handoffP95Ms'] is not None for group in driver.summarize(metric, 'metric')['groups']))
        self.assertFalse(driver.summarize(metric, 'metric')['promoted'])

    def test_resource_metrics_distinguish_preparation_transition_and_sustained_cpu(self):
        row = dict(seekSettledAtUnixMs=100, firstNextPlaying={'atUnixMs': 1000},
                   preparationTrace=[dict(phase='next_preparation_resolve_start', atUnixMs=500)])
        samples = [dict(atUnixMs=at, components={'helper': dict(cpuMs=20, cpuPercentOneCore=100, rssBytes=1000)})
                   for at in [0, 200, 700, 1200, 2200]]
        groups = driver.resource_phases(row, samples)
        self.assertEqual([group['phase'] for group in groups], ['control_setup', 'steady_before_preparation',
            'preparation_while_control_plays', 'transition_first_second', 'next_sustained'])
        self.assertTrue(all(group['peakCpuPercentOneCore'] == 100 for group in groups))
        self.assertEqual(driver.resource_component('WebKitWebProcess'), 'webview')
        self.assertEqual(driver.resource_component('yt-dlp'), 'helper')
        self.assertEqual(driver.resource_component('burrata'), 'app')

    def test_missing_first_audio_keeps_original_failure_and_partial_resource_samples(self):
        row = dict(seekSettledAtUnixMs=100, firstNextPlaying=None,
                   lastControl=dict(atUnixMs=1000, positionSeconds=89),
                   originalFailure='original_playback_failure', originalPlaybackFailureCategory='source_access',
                   controlGeneration=4, windowSeconds=30,
                   samples=[dict(trackId=None, atUnixMs=2200, positionSeconds=None)],
                   preparationTrace=[dict(phase='next_preparation_resolve_start', atUnixMs=500)])
        samples = [dict(atUnixMs=at, components={'helper': dict(cpuMs=20, cpuPercentOneCore=100, rssBytes=1000)})
                   for at in [700, 2200]]
        groups = driver.resource_phases(row, samples)
        self.assertEqual([group['phase'] for group in groups], ['preparation_while_control_plays', 'transition_waiting_or_failed'])
        self.assertEqual(sum(group['sampledCpuMs'] for group in groups), 40)
        operation = 'preparation:11111111-1111-1111-1111-111111111111'
        driver.attach_preparation(row, [dict(phase='next_preparation_policy', operationId=operation, atUnixMs=100,
            details=dict(generation=4, windowSeconds=30)), dict(phase='next_preparation_resolve_start', operationId=operation,
            atUnixMs=500, details={})])
        self.assertEqual(row['originalFailure'], 'original_playback_failure')
        self.assertEqual(row['originalPlaybackFailureCategory'], 'source_access')
        self.assertNotIn('preparationRemainingEstimateSeconds', row)

    def test_existing_gui_and_output_are_refused_before_any_attempt(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory); output = base / 'result.json'; binary = base / 'burrata'; binary.write_bytes(b'fake')
            with patch.object(driver, 'active_gui_pids', return_value=[123]), patch.object(driver, 'execute_attempt') as execute:
                with self.assertRaisesRegex(RuntimeError, 'existing_gui_refused'): driver.run(output, binary, {})
                execute.assert_not_called(); self.assertFalse(output.exists())
            output.write_text('original')
            with self.assertRaisesRegex(ValueError, 'output_already_exists'): driver.run(output, binary, {})
            self.assertEqual(output.read_text(), 'original')

    def test_six_fake_attempts_are_serial_and_failed_cleanup_stops_the_run(self):
        from contextlib import nullcontext
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory); binary = base / 'burrata'; binary.write_bytes(b'fake')
            seen = []
            def fake(attempt, track, executable, deadline):
                seen.append(attempt['windowSeconds'])
                return {**attempt, 'status': 'completed', 'eligible': True, 'cleanupCompleted': len(seen) < 3,
                        'sourceVideoId': 'abcdefghijk', 'selectedFormatId': '251', 'observedHandoffDelayMs': 0,
                        'originalFailure': 'owned_cleanup_failed' if len(seen) == 3 else None}
            with patch.object(driver, 'active_gui_pids', return_value=[]), patch.object(driver, 'single_driver_lock', return_value=nullcontext()), \
                    patch.object(driver, 'execute_attempt', side_effect=fake):
                result = driver.run(base/'result.json', binary, {}, budget_seconds=10)
            self.assertEqual(seen, [30, 45, 45]); self.assertEqual(result['status'], 'cleanup_or_ownership_refusal')
            self.assertEqual(sum(row['status'] == 'not_run' for row in result['attempts']), 3)

    def test_outer_deadline_retains_unrun_rows_and_source_change_invalidates_deltas(self):
        from contextlib import nullcontext
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory); binary = base/'burrata'; binary.write_bytes(b'original')
            clock = [0.]
            def fake(attempt, track, executable, deadline):
                clock[0] = deadline+1
                executable.write_bytes(b'changed')
                return {**attempt, 'status': 'completed', 'eligible': True, 'cleanupCompleted': True,
                        'sourceVideoId': 'abcdefghijk', 'selectedFormatId': '251', 'observedHandoffDelayMs': 0}
            with patch.object(driver, 'active_gui_pids', return_value=[]), patch.object(driver, 'single_driver_lock', return_value=nullcontext()), \
                    patch.object(driver, 'execute_attempt', side_effect=fake), patch.object(driver.time, 'monotonic', side_effect=lambda: clock[0]):
                result = driver.run(base/'result.json', binary, {}, budget_seconds=1)
            self.assertEqual(result['status'], 'source_changed_during_run')
            self.assertEqual(sum(row['status'] == 'not_run' for row in result['attempts']), 5)
            self.assertTrue(all(not row['eligible'] for row in result['attempts']))

    @unittest.skipUnless(os.name == 'posix', 'POSIX process groups')
    def test_cleanup_terminates_owned_fake_child_after_its_leader_exits(self):
        with tempfile.TemporaryDirectory() as directory:
            child_path = Path(directory)/'child.pid'
            process = subprocess.Popen([sys.executable, '-c',
                "import subprocess,sys; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']);open(sys.argv[1],'w').write(str(p.pid))",
                str(child_path)], start_new_session=True)
            try:
                process.wait(timeout=3)
                self.assertTrue(driver.stop_owned(process))
                self.assertFalse(driver.live_group_members(process.pid))
            finally: driver.stop_owned(process)

    def test_cleanup_exception_is_reported_without_aborting_other_owner_cleanup(self):
        from unittest.mock import Mock
        process = Mock(pid=123); process.poll.return_value = None
        with patch.object(driver, 'terminate_group', side_effect=subprocess.TimeoutExpired('fixture', 1)), \
                patch.object(driver, 'live_group_members', return_value=[123]):
            self.assertFalse(driver.stop_owned(process))


if __name__ == '__main__': unittest.main()
