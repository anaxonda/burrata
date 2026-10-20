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
import shlex
from contextlib import closing
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
            with closing(sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite')) as connection:
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
        self.assertEqual(driver.resource_component('burrata'), 'unknown')

    def test_mainthread_node_is_a_proven_helper_and_app_is_only_the_selected_root_identity(self):
        from unittest.mock import Mock
        reader = Mock(return_value='node')
        attribution = driver.ProcessAttribution((10, 100), reader)
        self.assertEqual(attribution.classify(10, dict(start=100, name='MainThread')), 'app')
        reader.assert_not_called()
        child = dict(start=200, name='MainThread')
        self.assertEqual(attribution.classify(20, child), 'helper')
        self.assertEqual(attribution.classify(20, {**child, 'name': 'burrata'}), 'helper')
        reader.assert_called_once_with(20, 200)
        # A reused root PID is a different identity even if its executable is Node.
        self.assertEqual(attribution.classify(10, dict(start=101, name='burrata')), 'helper')
        self.assertEqual(attribution.metadata()['seenProcessIdentitiesByComponent']['app'], 1)
        self.assertEqual(attribution.metadata()['version'], driver.RESOURCE_CLASSIFICATION_VERSION)

    def test_executable_cache_is_keyed_by_pid_and_start_and_does_not_reuse_previous_role(self):
        from unittest.mock import Mock
        reader = Mock(side_effect=['node', 'WebKitWebProcess'])
        attribution = driver.ProcessAttribution((10, 100), reader)
        self.assertEqual(attribution.classify(20, dict(start=200, name='MainThread')), 'helper')
        self.assertEqual(attribution.classify(20, dict(start=201, name='MainThread')), 'webview')
        self.assertEqual(reader.call_count, 2)
        self.assertEqual(len(attribution.basename_cache), 2)
        with self.assertRaisesRegex(RuntimeError, 'sampled_app_root_identity_lost'):
            attribution.require_root({(10, 0): dict(start=101)})

    def test_inaccessible_or_unrecognized_executable_remains_unknown_without_comm_fallback(self):
        from unittest.mock import Mock
        reader = Mock(side_effect=[PermissionError('private path is unavailable'), 'customer-secret.bin'])
        attribution = driver.ProcessAttribution((10, 100), reader)
        self.assertEqual(attribution.classify(20, dict(start=200, name='burrata')), 'unknown')
        self.assertEqual(attribution.classify(20, dict(start=200, name='node')), 'unknown')
        self.assertEqual(reader.call_count, 1, 'failed executable reads are cached for this identity')
        self.assertEqual(attribution.classify(30, dict(start=300, name='WebKitWebProcess')), 'unknown')
        metadata = attribution.metadata()
        self.assertEqual(metadata['seenProcessIdentitiesByComponent']['unknown'], 2)
        self.assertEqual(metadata['cachedExecutableBasenameCounts'], {'unavailable': 1, 'unrecognized': 1})
        self.assertNotIn('secret', json.dumps(metadata)); self.assertNotIn('private', json.dumps(metadata))

    def test_readlink_requires_the_same_process_start_before_and_after_read(self):
        with patch.object(driver, 'read_stat', side_effect=[{'start': 1}, {'start': 2}]), \
                patch.object(driver.os, 'readlink', return_value='/packaged/tools/node'):
            self.assertIsNone(driver.executable_basename(20, 1))
        with patch.object(driver, 'read_stat', return_value={'start': 2}), patch.object(driver.os, 'readlink') as readlink:
            self.assertIsNone(driver.executable_basename(20, 1)); readlink.assert_not_called()
        with patch.object(driver, 'read_stat', return_value={'start': 1}), \
                patch.object(driver.os, 'readlink', side_effect=PermissionError):
            self.assertIsNone(driver.executable_basename(20, 1))
        with patch.object(driver, 'read_stat', return_value={'start': 1}), \
                patch.object(driver.os, 'readlink', return_value='/packaged/tools/node'):
            self.assertEqual(driver.executable_basename(20, 1), 'node')

    def test_sampler_totals_include_unknown_and_pid_reuse_starts_a_new_cpu_baseline(self):
        basenames = {(20, 200): 'node', (30, 300): None, (40, 400): 'WebKitWebProcess', (20, 201): None}
        attribution = driver.ProcessAttribution((10, 100), lambda pid, start: basenames.get((pid, start)))
        processes = {
            (10, 0): dict(start=100, ticks=120, rss=100, name='MainThread'),
            (20, 0): dict(start=200, ticks=230, rss=200, name='MainThread'),
            (30, 0): dict(start=300, ticks=305, rss=300, name='burrata'),
            (40, 0): dict(start=400, ticks=407, rss=400, name='node'),
        }
        prior = {(10, 100): 100, (20, 200): 200, (30, 300): 300, (40, 400): 400}
        totals = {}
        components = driver.resource_components(processes, attribution, prior, totals, .2, 100)
        self.assertEqual(components['app']['cpuMs'], 200)
        self.assertEqual(components['helper']['cpuMs'], 300)
        self.assertEqual(components['unknown']['cpuMs'], 50)
        self.assertEqual(components['webview']['cpuMs'], 70)
        self.assertEqual(sum(row['rssBytes'] for row in components.values()), 1000)
        self.assertEqual(sum(row['cpuMs'] for row in components.values()), 620)
        self.assertEqual(sum(row['cpuPercentOneCore'] for row in components.values()), 310)
        self.assertEqual(sum(totals.values())*10, 620)
        reused = {**processes, (20, 0): dict(start=201, ticks=99999, rss=222, name='node')}
        newer = driver.resource_components(reused, attribution, prior, totals, .2, 100)
        self.assertEqual(newer['helper']['cpuMs'], 0)
        self.assertEqual(newer['unknown']['cpuMs'], 0, 'a reused PID must not create a false CPU spike')
        self.assertEqual(newer['unknown']['rssBytes'], 522)
        self.assertTrue(attribution.metadata()['unknownIncludedInTreeTotals'])
        prior_before, totals_before = dict(prior), dict(totals)
        lost_root = {**reused, (10, 0): dict(start=101, ticks=99999, rss=99999, name='burrata')}
        with self.assertRaisesRegex(RuntimeError, 'sampled_app_root_identity_lost'):
            driver.resource_components(lost_root, attribution, prior, totals, .2, 100)
        self.assertEqual(prior, prior_before)
        self.assertEqual(totals, totals_before, 'a rejected root identity cannot change sampled accounting')

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

    def test_launcher_failure_is_typed_and_raw_log_material_is_not_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = Path(directory)
            text = ('https://private.example/?sig=secret Cookie: account=secret\n'
                    f'The native executable is older than {driver.ROOT}/src-tauri/src/native_output.rs; compile Burrata before launching.\n')
            (profile/'launcher.log').write_text(text)
            evidence = driver.launcher_evidence(profile, False, 1, 'launcher_failed')
            self.assertEqual(evidence['category'], 'native_binary_stale')
            self.assertTrue(evidence['deterministic']); self.assertEqual(evidence['exitCodeBeforeCleanup'], 1)
            self.assertEqual(evidence['staleSourceRelativePath'], 'src-tauri/src/native_output.rs')
            self.assertTrue(evidence['log']['present']); self.assertNotIn('secret', json.dumps(evidence))
            self.assertFalse(evidence['appProcessStarted']); self.assertFalse(evidence['frontendMounted'])
            (profile/'launcher.log').write_text('x'*200_000+'\nThe authenticated MCP get_app_status handshake did not confirm a mounted frontend.\n')
            text, fingerprint = driver.bounded_log(profile/'launcher.log')
            self.assertTrue(fingerprint['truncated']); self.assertLessEqual(fingerprint['sampledBytes'], 128*1024)
            self.assertEqual(driver.launcher_evidence(profile, False, 1, 'launcher_failed')['category'], 'frontend_handshake_failed')

    def test_exit_only_fake_launcher_retains_evidence_and_stops_after_first_setup_failure(self):
        from contextlib import nullcontext
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory); binary = base/'burrata'; binary.write_bytes(b'never executed')
            fake = base/'launcher.sh'
            message = f'The native executable is older than {driver.ROOT}/src-tauri/src/native_output.rs; compile Burrata before launching.'
            phase = dict(phase='next_preparation_start', nativeOperationId='preparation:11111111-1111-1111-1111-111111111111',
                         atUnixMs=100, details={'ok': False, 'url': 'https://private.example/?sig=secret'})
            fake.write_text('#!/bin/sh\n'+"printf '%s\\n' "+shlex.quote(message)+"\n"
                            +"printf '%s\\n' "+shlex.quote('burrata_playback_phase '+json.dumps(phase))+' > "$2/app.log"\n'
                            +"printf '%s\\n' 'https://private.example/?sig=secret'\nexit 1\n")
            profiles = []
            original_seed = driver.seed_attempt
            def seeded(profile, track):
                profiles.append(profile); return original_seed(profile, track)
            with patch.object(driver, 'active_gui_pids', return_value=[]), \
                    patch.object(driver, 'single_driver_lock', return_value=nullcontext()), \
                    patch.object(driver.seed, 'LAUNCHER', fake), patch.object(driver, 'seed_attempt', side_effect=seeded):
                result = driver.run(base/'result.json', binary, driver.select_track(driver.MANIFEST, 'matthew'), budget_seconds=10)
            self.assertEqual(result['status'], 'setup_failed')
            first = result['attempts'][0]
            self.assertEqual(first['originalFailure'], 'launcher_failed')
            self.assertEqual(first['launcherFailureCategory'], 'native_binary_stale')
            self.assertEqual(first['launcher']['exitCodeBeforeCleanup'], 1)
            self.assertEqual(result['resourceClassificationVersion'], driver.RESOURCE_CLASSIFICATION_VERSION)
            self.assertEqual(first['resourceClassification']['version'], driver.RESOURCE_CLASSIFICATION_VERSION)
            self.assertFalse(first['resourceClassification']['initialized'])
            self.assertTrue(first['resourceClassification']['unknownIncludedInTreeTotals'])
            self.assertTrue(first['cleanupCompleted']); self.assertEqual(first['playbackObservationCount'], 0)
            self.assertEqual(len(first['partialPreparationTrace']), 1)
            self.assertEqual(sum(row['status'] == 'not_run' for row in result['attempts']), 5)
            self.assertEqual(result['summary']['groups'][0]['setupFailures'], {'native_binary_stale': 1})
            self.assertNotIn('secret', json.dumps(result)); self.assertTrue(all(not path.exists() for path in profiles))
            self.assertEqual([outcome['stage'] for outcome in first['outcomes']], ['launcher', 'launcher', 'cleanup'])

    def test_partial_observer_resource_and_trace_evidence_preserves_each_original_outcome(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = Path(directory); (profile/'fixture').mkdir()
            (profile/'launcher.log').write_text('MCP confirms the app is running with its frontend mounted.\n')
            partial = dict(samples=[dict(trackId=None, atUnixMs=2200, positionSeconds=None)],
                           originalFailure='original_playback_failure', originalPlaybackFailureCategory='source_access',
                           controlGeneration=4, seekSettledAtUnixMs=100,
                           lastControl=dict(atUnixMs=1000, positionSeconds=89), firstNextPlaying=None, transitionOk=False)
            (profile/'fixture/next-preparation-observation.json').write_text(json.dumps(partial))
            events = [dict(phase=phase, nativeOperationId='preparation:11111111-1111-1111-1111-111111111111',
                           atUnixMs=at, details=details) for phase, at, details in [
                ('next_preparation_policy', 100, dict(generation=4, windowSeconds=30)),
                ('next_preparation_resolve_start', 500, {}), ('next_preparation_resolve_end', 800, dict(ok=False))]]
            (profile/'app.log').write_text(''.join('burrata_playback_phase '+json.dumps(event)+'\n' for event in events))
            row = dict(windowSeconds=30, originalFailure='observer_deadline')
            resources = [dict(atUnixMs=2200, components={'helper': dict(cpuMs=20, cpuPercentOneCore=100, rssBytes=1000)})]
            driver.collect_attempt_evidence(row, profile, True, None, resources)
            self.assertEqual(row['originalFailure'], 'observer_deadline')
            self.assertEqual(row['partialObserverOriginalFailure'], 'original_playback_failure')
            self.assertEqual(row['originalPlaybackFailureCategory'], 'source_access')
            self.assertEqual(row['playbackObservationCount'], 1)
            self.assertEqual(row['resourcePhases'][0]['phase'], 'transition_waiting_or_failed')
            self.assertEqual(row['resourcePhases'][0]['sampledCpuMs'], 20)
            self.assertEqual(len(row['partialPreparationTrace']), 3)

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
