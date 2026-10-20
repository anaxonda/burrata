#!/usr/bin/env python3
"""Plan (default), or explicitly run isolated serial native 30/45s preparation trials.

No compilation, credential loading, normal-profile access, or release promotion.
The --run-isolated mode intentionally refuses an already-running Burrata GUI.
"""
from __future__ import annotations
import argparse
from contextlib import closing, contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone

import run_isolated_memory as seed
from profile_process import snapshot as process_snapshot
from innertube.serial_same_worker import terminate_group

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OBSERVER = HERE / 'next_preparation_observer.mjs'
MANIFEST = HERE / 'innertube/fixtures/transport-music-tracks.json'
SCHEMA = 'burrata.next-preparation/v1'
PHASES = {
    'next_preparation_start', 'next_preparation_policy', 'next_preparation_discovery_end',
    'next_preparation_resolve_start', 'next_preparation_resolve_end', 'next_preparation_queued',
    'next_preparation_adopted', 'next_preparation_end', 'resolution_start', 'resolution_end',
    'extraction_start', 'extraction_end', 'stream_probe_start', 'stream_probe_end',
    'stream_probe_body_end', 'same_source_refresh_start', 'same_source_refresh_end',
}
DETAILS = {'windowSeconds', 'generation', 'localSource', 'ok', 'resultCount', 'durationMs', 'status'}
LIMITATIONS = [
    'Diagnostic only; no production policy change or automatic release/performance promotion.',
    'Fresh app/profile per arm; OS/media/provider caches outside the profile are not declared cold.',
    'Exactly one queue selection and one initial local seek to30s; then natural transition and5s of advancing native playhead.',
    'A90s local control leaves approximately60s after setup; initial discovery before the setup seek may warm the same public source.',
    'Polling estimates handoff timing at100ms resolution; neither physical audibility nor sample-level gaplessness is measured.',
    'Source ID is pinned; format is observed, not forced. Deltas require equal actual provider/video/format IDs.',
    'Fresh expiring URLs and existing bounded refresh/recovery remain production behavior; failures are retained, never retried by the harness.',
    'CPU100% means one logical core. App/WebView/helper components are sampled every200ms, exclude Vite, and can miss short-lived or reparented children; RSS can double-count shared pages.',
    'Network wire bytes are unknown. Decoder read counts and range high-water marks are not substituted for network traffic.',
]
LAUNCHER_FAILURES = [
    ('native_binary_stale', 'The native executable is older than ', True),
    ('native_binary_missing', 'Compiled Burrata executable not found:', True),
    ('frontend_dependencies_missing', 'Vite/Node modules are not installed;', True),
    ('node_version_unsupported', 'Burrata requires Node.js 22.12 or newer', True),
    ('frontend_build_missing', 'Compiled frontend entry is missing;', True),
    ('frontend_entry_invalid', 'dist/index.html does not reference an existing compiled JavaScript entry.', True),
    ('frontend_build_stale', 'The compiled frontend is older than ', True),
    ('native_predates_frontend', 'The native executable predates the compiled frontend;', True),
    ('frontend_shared_listener_refused', 'Protected frontend requires its own Vite listener;', True),
    ('frontend_listener_unidentified', 'Could not identify the live listener serving port1420', True),
    ('frontend_listener_unidentified', 'Could not identify the live listener serving port 1420', True),
    ('frontend_listener_identity_invalid', 'Port1420 listener identity is inaccessible', True),
    ('frontend_listener_identity_invalid', 'Port 1420 listener identity is inaccessible', True),
    ('profile_conflict', 'Burrata already has a process using this or an overlapping profile', True),
    ('profile_identity_unverifiable', 'Cannot safely verify a running Burrata process', True),
    ('isolated_profile_invalid', 'Isolated profile must be ', True),
    ('isolated_profile_invalid', 'Isolated profile marker is missing.', True),
    ('isolated_music_root_invalid', 'Isolated Music ', True),
    ('xdg_profile_invalid', 'XDG_DATA_HOME and XDG_CONFIG_HOME must be absolute', True),
    ('launch_environment_invalid', 'BURRATA_AUTOMATION_REGISTRATION_TIMEOUT_SECONDS must be', True),
    ('launch_environment_invalid', 'BURRATA_PROTECTED_FRONTEND must be', True),
    ('frontend_modules_invalid', 'Frontend module verification failed:', False),
    ('frontend_modules_invalid', 'The compiled Vite module check failed;', False),
    ('app_exited_before_registration', 'Burrata exited before registering a fresh automation endpoint.', False),
    ('automation_registration_timeout', 'Burrata did not register a fresh automation endpoint within ', False),
    ('frontend_handshake_failed', 'The authenticated MCP get_app_status handshake did not confirm a mounted frontend.', False),
]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def safe_repo_path(raw):
    try: relative = Path(raw).resolve().relative_to(ROOT).as_posix()
    except (OSError, ValueError): return None
    return relative if re.fullmatch(r'[A-Za-z0-9_./-]{1,240}', relative) else None


def bounded_log(path):
    """Return transient text for classification; only fingerprints are retained."""
    if path.is_symlink(): return '', dict(present=False, symlinkRefused=True)
    try:
        if not path.is_file(): return '', dict(present=False)
        size = path.stat().st_size
        with path.open('rb') as stream:
            data = stream.read(64*1024)
            if size > len(data):
                stream.seek(max(len(data), size-64*1024)); data += stream.read(64*1024)
    except OSError: return '', dict(present=False, readFailed=True)
    return data.decode('utf-8', errors='replace'), dict(present=True, sizeBytes=size,
        sampledBytes=len(data), truncated=len(data) < size, sampleSha256=hashlib.sha256(data).hexdigest())


def launcher_evidence(profile, ready, exit_code, original_failure):
    text, fingerprint = bounded_log(profile/'launcher.log')
    evidence = dict(ready=ready, exitCodeBeforeCleanup=exit_code, log=fingerprint,
                    frontendModulesVerified='Verified compiled frontend modules:' in text,
                    appProcessStarted='Started compiled Burrata (' in text,
                    frontendMounted='MCP confirms the app is running with its frontend mounted.' in text)
    if ready: return evidence
    evidence.update(category='launcher_readiness_timeout' if original_failure == 'launcher_deadline' else 'unclassified_launcher_exit',
                    deterministic=False)
    for category, marker, deterministic in LAUNCHER_FAILURES:
        if marker in text:
            evidence.update(category=category, deterministic=deterministic); break
    stale = re.search(r'The native executable is older than ([^\r\n]+); compile Burrata before launching\.', text)
    if stale:
        relative = safe_repo_path(stale[1])
        if relative: evidence['staleSourceRelativePath'] = relative
    return evidence


def collect_attempt_evidence(row, profile, launcher_ready, launcher_exit, resource_samples):
    """Capture allowlisted partial evidence before disposable files are removed."""
    row['launcher'] = launcher_evidence(profile, launcher_ready, launcher_exit, row.get('originalFailure'))
    if not launcher_ready and row.get('originalFailure') in ['launcher_failed', 'launcher_deadline']:
        row['launcherFailureCategory'] = row['launcher']['category']
        row['stopRemainingSetupAttempts'] = True
        row.setdefault('outcomes', []).append(dict(stage='launcher', category=row['launcher']['category'],
            deterministic=row['launcher']['deterministic'], exitCode=launcher_exit))
    _, app_log = bounded_log(profile/'app.log')
    observer_text, observer_log = bounded_log(profile/'observer.log')
    row['appLog'] = app_log; row['observerLog'] = observer_log
    # A failed observer may still have completed a partial report. Never replace
    # an already captured original failure with a later nominal success.
    observed = profile/'fixture/next-preparation-observation.json'
    if 'samples' not in row and observed.is_file() and observed.stat().st_size <= 3*1024*1024:
        try:
            partial = json.loads(observed.read_text())
            # This file is written by the fixed owned observer, which already
            # allowlists its fields. Driver outcome fields keep their ownership.
            original = row.get('originalFailure')
            for key in ['samples', 'lastControl', 'firstNextPlaying', 'controlGeneration', 'seekSettledAtUnixMs',
                        'originalPlaybackFailureCategory', 'wallMs', 'transitionOk', 'sourceVideoId',
                        'selectedFormatId', 'observedHandoffDelayMs', 'sustainedNextSeconds']:
                if key in partial: row[key] = partial[key]
            row['partialObserverOriginalFailure'] = partial.get('originalFailure')
            row['originalFailure'] = original or partial.get('originalFailure')
        except (OSError, ValueError, TypeError):
            row.setdefault('evidenceCaptureFailures', []).append('partial_observer_report_unavailable')
    for category in ['invalid_arguments', 'isolated_profile_required', 'invalid_manifest', 'isolated_output_required',
                     'observer_setup_failed', 'native_engine_required']:
        if category in observer_text.splitlines() or row.get('originalFailure') == category:
            row['observerSetupFailureCategory'] = category
            row['stopRemainingSetupAttempts'] = True
            row.setdefault('outcomes', []).append(dict(stage='observer_setup', category=category, deterministic=True))
            break
    try:
        events = safe_trace(profile/'app.log')
        if events: row['partialPreparationTrace'] = events
        if row.get('controlGeneration') is not None: attach_preparation(row, events)
    except (OSError, RuntimeError, ValueError, TypeError):
        row.setdefault('evidenceCaptureFailures', []).append('preparation_trace_unavailable')
    row['resourceSamples'] = resource_samples
    row['resourcePhases'] = resource_phases(row, resource_samples)
    row['playbackObservationCount'] = len(row.get('samples', []))


def select_track(path, name):
    rows = json.loads(Path(path).read_text())
    matches = [row for row in rows if row.get('name') == name]
    if len(matches) != 1:
        raise ValueError('track_must_select_one_public_fixture')
    row = matches[0]
    if not re.fullmatch(r'[A-Za-z0-9_-]{11}', row.get('videoId', '')):
        raise ValueError('invalid_public_video_id')
    duration = row.get('durationSeconds')
    if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not 60 <= duration <= 600:
        raise ValueError('independent_duration_required')
    if any(not isinstance(row.get(key), str) or not 1 <= len(row[key]) <= 160
           for key in ['titleNeedle', 'channelNeedle']):
        raise ValueError('public_recording_metadata_required')
    return {key: row[key] for key in ['name', 'videoId', 'durationSeconds', 'titleNeedle', 'channelNeedle']}


def plan(phase='exploratory'):
    if phase not in {'exploratory', 'metric'}:
        raise ValueError('invalid_phase')
    pairs = 3 if phase == 'exploratory' else 20
    return [dict(pair=pair, windowSeconds=window, ordinal=(pair-1)*2+offset+1)
            for pair in range(1, pairs+1)
            for offset, window in enumerate([30, 45] if pair % 2 else [45, 30])]


def summarize(rows, phase):
    groups = []
    for window in [30, 45]:
        attempts = [row for row in rows if row['windowSeconds'] == window]
        eligible = [row for row in attempts if row.get('eligible')]
        values = [row['observedHandoffDelayMs'] for row in eligible
                  if isinstance(row.get('observedHandoffDelayMs'), (int, float))]
        failures = {}
        setup_failures = {}
        for row in attempts:
            category = row.get('originalFailure') or ('not_run' if row.get('status') == 'not_run' else None)
            if category: failures[category] = failures.get(category, 0) + 1
            setup = row.get('launcherFailureCategory') or row.get('observerSetupFailureCategory')
            if setup: setup_failures[setup] = setup_failures.get(setup, 0)+1
        groups.append(dict(windowSeconds=window, planned=len(attempts), attempted=sum(row.get('status') != 'not_run' for row in attempts),
                           eligible=len(eligible), failures=failures, setupFailures=setup_failures,
                           handoffMedianMs=statistics.median(values) if values else None,
                           handoffWorstMs=max(values) if values else None,
                           handoffP95Ms=sorted(values)[math.ceil(len(values)*.95)-1] if phase == 'metric' and len(values) >= 20 else None))
    pairs = []
    for pair in sorted({row['pair'] for row in rows}):
        arms = {row['windowSeconds']: row for row in rows if row['pair'] == pair}
        before, after = arms.get(30, {}), arms.get(45, {})
        same = bool(before.get('sourceVideoId') and before.get('sourceVideoId') == after.get('sourceVideoId')
                    and before.get('selectedFormatId') and before.get('selectedFormatId') == after.get('selectedFormatId'))
        eligible = bool(same and before.get('eligible') and after.get('eligible'))
        pairs.append(dict(pair=pair, sameActualSourceAndFormat=same, eligible=eligible,
                          handoff45Minus30Ms=after['observedHandoffDelayMs']-before['observedHandoffDelayMs'] if eligible else None))
    return dict(groups=groups, pairs=pairs, qualification=False, promoted=False,
                p95Rule='Only the explicitly requested metric phase with20 eligible transitions per arm has a descriptive nearest-rank p95; all failures stay in the denominator.')


def active_gui_pids(binary, proc=Path('/proc')):
    found = []
    for entry in proc.iterdir():
        if not entry.name.isdigit(): continue
        try:
            name = (entry / 'comm').read_text().strip().lower()
            executable = (entry / 'exe').resolve(strict=True)
            if name == 'burrata' or executable == binary or executable.name.lower() == 'burrata':
                found.append(int(entry.name))
        except (OSError, ValueError):
            # A named Burrata that cannot be inspected still prevents a launch.
            try:
                if (entry / 'comm').read_text().strip().lower() == 'burrata': found.append(int(entry.name))
            except OSError: pass
    return sorted(found)


@contextmanager
def single_driver_lock():
    import fcntl  # Live orchestration is Linux-only; plan imports stay portable.
    path = Path('/tmp') / f'burrata-next-preparation-{os.getuid()}.lock'
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        if os.fstat(fd).st_uid != os.getuid(): raise RuntimeError('driver_lock_owner_mismatch')
        try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError: raise RuntimeError('another_preparation_driver_is_running') from None
        yield
    finally:
        os.close(fd)


def seed_attempt(profile, track):
    old_rows, old_seconds = seed.ROWS, seed.FIXTURE_SECONDS
    try:
        seed.ROWS, seed.FIXTURE_SECONDS = 0, 90
        local_id, _ = seed.seed_profile(profile)
    finally:
        seed.ROWS, seed.FIXTURE_SECONDS = old_rows, old_seconds
    next_id = f"youtube:{track['videoId']}"
    record = seed.track_record(next_id, 'youtube', track['videoId'], track['titleNeedle'], track['channelNeedle'], track['durationSeconds'])
    record.update(sourceUrl=f"https://www.youtube.com/watch?v={track['videoId']}", version=None,
                  album=None, provenance={'diagnosticExplicitSource': True})
    db = profile / 'data/org.burrata.discovery/burrata.sqlite'
    with closing(sqlite3.connect(db)) as connection, connection:
        connection.execute('INSERT INTO works(id,title,artist) VALUES(?,?,?)', (f'work:{next_id}', record['title'], record['artist']))
        connection.execute('INSERT INTO recordings(id,work_id,version) VALUES(?,?,NULL)', (next_id, f'work:{next_id}'))
        connection.execute('INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)',
                           (next_id, 'youtube', track['videoId'], next_id, record['sourceUrl'], json.dumps(record), datetime.now(timezone.utc).isoformat()))
        connection.execute("INSERT INTO settings(key,value) VALUES('playbackEngine.v1','\"native\"')")
        if connection.execute('PRAGMA integrity_check').fetchone()[0] != 'ok': raise RuntimeError('fixture_integrity_failed')
    spec = dict(localTrackId=local_id, localDurationSeconds=90, nextTrackId=next_id,
                nextVideoId=track['videoId'], nextDurationSeconds=track['durationSeconds'],
                automationFile=str(profile / 'data/org.burrata.discovery/automation.json'))
    path = profile / 'fixture/next-preparation.json'
    path.write_text(json.dumps(spec) + '\n')
    return path


def safe_trace(path):
    result = []
    if not path.exists(): return result
    if path.stat().st_size > 16*1024*1024: raise RuntimeError('trace_size_limit')
    for line in path.read_text(errors='replace').splitlines():
        if not line.startswith('burrata_playback_phase '): continue
        try: event = json.loads(line.removeprefix('burrata_playback_phase '))
        except ValueError: continue
        if not isinstance(event, dict) or not isinstance(event.get('details', {}), dict): continue
        operation = event.get('nativeOperationId', '')
        if event.get('phase') not in PHASES or not isinstance(operation, str) or not re.fullmatch(r'preparation:[a-f0-9-]{36}', operation): continue
        details = {key: value for key, value in event.get('details', {}).items()
                   if key in DETAILS and isinstance(value, (bool, int, float)) and math.isfinite(value)}
        if not isinstance(event.get('atUnixMs'), int): continue
        result.append(dict(phase=event['phase'], operationId=operation, atUnixMs=event['atUnixMs'], details=details))
    return result


def attach_preparation(row, events):
    policies = [event for event in events if event['phase'] == 'next_preparation_policy'
                and event['details'].get('generation') == row.get('controlGeneration')
                and event['atUnixMs'] >= row.get('seekSettledAtUnixMs', float('inf'))-2000]
    if not policies:
        row['originalFailure'] = row.get('originalFailure') or 'preparation_trace_missing'; return
    policy = policies[-1]
    selected = [event for event in events if event['operationId'] == policy['operationId']]
    row['preparationTrace'] = selected
    if policy['details'].get('windowSeconds') != row['windowSeconds']:
        row['originalFailure'] = row.get('originalFailure') or 'diagnostic_window_mismatch'; return
    def first(phase): return next((event for event in selected if event['phase'] == phase), None)
    start, queued, adopted = (first(phase) for phase in ['next_preparation_resolve_start', 'next_preparation_queued', 'next_preparation_adopted'])
    samples = [sample for sample in row.get('samples', [])
               if isinstance(sample.get('trackId'), str) and sample['trackId'].startswith('local:')
               and isinstance(sample.get('positionSeconds'), (int, float))]
    if start and samples:
        sample = min(samples, key=lambda item: abs(item['atUnixMs']-start['atUnixMs']))
        row['preparationRemainingEstimateSeconds'] = round(90-sample['positionSeconds']-(start['atUnixMs']-sample['atUnixMs'])/1000, 3)
    row['preparedReaderQueuedAgeAtAdoptionMs'] = adopted['atUnixMs']-queued['atUnixMs'] if queued and adopted else None
    row['signedUrlExpiryAtAdoption'] = None  # No URL or expiry is inferred from queue time.
    row['probeDeniedResponses'] = sum(event['details'].get('status') in [403, 410] for event in selected if event['phase'] == 'stream_probe_end')
    if any(event['details'].get('ok') is False for event in selected if event['phase'] in ['next_preparation_resolve_end', 'next_preparation_queued']):
        row['originalFailure'] = row.get('originalFailure') or 'original_preparation_failed'
    if not queued or not adopted:
        row['originalFailure'] = row.get('originalFailure') or 'preparation_not_adopted'


def resource_component(name):
    lower = name.lower()
    if 'webkit' in lower: return 'webview'
    if any(lower.startswith(prefix) for prefix in ['node', 'deno', 'yt-dlp', 'ffmpeg', 'python', 'bgutil']): return 'helper'
    return 'app'


def resource_phases(row, samples):
    start = next((event['atUnixMs'] for event in row.get('preparationTrace', [])
                  if event['phase'] == 'next_preparation_resolve_start'), None)
    first = (row.get('firstNextPlaying') or {}).get('atUnixMs')
    settled = row.get('seekSettledAtUnixMs')
    local = row.get('lastControl') or {}
    predicted_end = (local['atUnixMs']+(90-local['positionSeconds'])*1000
                     if local.get('positionSeconds') is not None and local.get('atUnixMs') is not None else first)
    groups = {}
    for sample in samples:
        at = sample['atUnixMs']
        phase = ('control_setup' if settled is None or at < settled else
                 'unattributed_after_setup' if start is None else
                 'steady_before_preparation' if at < start else
                 'preparation_while_control_plays' if predicted_end is None or at < predicted_end else
                 'transition_waiting_or_failed' if first is None else
                 'transition_first_second' if at < first+1000 else 'next_sustained')
        for component, metrics in sample['components'].items():
            group = groups.setdefault((phase, component), dict(phase=phase, component=component,
                samples=0, sampledCpuMs=0, peakCpuPercentOneCore=0, peakRssBytes=0))
            group['samples'] += 1; group['sampledCpuMs'] += metrics['cpuMs']
            group['peakCpuPercentOneCore'] = max(group['peakCpuPercentOneCore'], metrics['cpuPercentOneCore'])
            group['peakRssBytes'] = max(group['peakRssBytes'], metrics['rssBytes'])
    return list(groups.values())


def live_group_members(group):
    members = []
    for path in Path('/proc').glob('[0-9]*/stat'):
        try:
            text = path.read_text(); fields = text[text.rfind(')')+2:].split()
            if int(fields[2]) == group and fields[0] != 'Z': members.append(int(path.parent.name))
        except (OSError, ValueError, IndexError): pass
    return members


def stop_owned(process):
    if process is None: return True
    try: terminate_group(process, grace=1)
    except (OSError, subprocess.SubprocessError):
        return process.poll() is not None and not live_group_members(process.pid)
    try: process.wait(timeout=2)
    except subprocess.TimeoutExpired: return False
    deadline = time.monotonic()+2
    while time.monotonic() < deadline:
        if not live_group_members(process.pid): return True
        time.sleep(.05)
    return not live_group_members(process.pid)


def execute_attempt(attempt, track, binary, deadline):
    row = {**attempt, 'status': 'started', 'eligible': False, 'originalFailure': None}
    if active_gui_pids(binary): return {**row, 'status': 'failed', 'originalFailure': 'existing_gui_refused', 'cleanupCompleted': True}
    profile = Path(tempfile.mkdtemp(prefix='burrata-launch.', dir='/tmp'))
    launcher = observer = None
    launcher_ready = False
    resource_samples = []
    totals, peak_rss, sample_count = {}, 0, 0
    row['outcomes'] = []
    stage = 'seed'
    try:
        manifest = seed_attempt(profile, track)
        stage = 'launcher'
        environment = dict(os.environ, BURRATA_APP_BINARY=str(binary), BURRATA_PLAYBACK_ENGINE='native',
                           BURRATA_PLAYBACK_TRACE='1', BURRATA_DIAGNOSTIC_NEXT_PREPARATION_SECONDS=str(attempt['windowSeconds']),
                           BURRATA_ISOLATED_MUSIC_ROOT=str(profile / 'fixture/Music'), TMPDIR='/tmp',
                           BURRATA_PROTECTED_FRONTEND='1')
        for key in ['BURRATA_APP_DATA', 'BURRATA_AUTOMATION_FILE', 'BURRATA_LAUNCH_LOG_DIR', 'BURRATA_LAUNCH_VITE_PID_FILE']:
            environment.pop(key, None)
        with (profile / 'launcher.log').open('wb') as log:
            launcher = subprocess.Popen(['bash', str(seed.LAUNCHER), '--isolated-profile', str(profile), '--keep-profile'],
                                        cwd=ROOT, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                        start_new_session=True)
            ready_deadline = min(deadline, time.monotonic()+90)
            while time.monotonic() < ready_deadline:
                if launcher.poll() is not None: raise RuntimeError('launcher_failed')
                if 'MCP confirms the app is running with its frontend mounted.' in (profile / 'launcher.log').read_text(errors='replace'):
                    launcher_ready = True; break
                time.sleep(.1)
            else: raise TimeoutError('launcher_deadline')
            stage = 'observer_setup'
            observed = profile / 'fixture/next-preparation-observation.json'
            observer_errors = (profile/'observer.log').open('wb')
            try:
                observer = subprocess.Popen(['node', str(OBSERVER), str(manifest), str(observed)], cwd=ROOT, env=environment,
                                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=observer_errors, start_new_session=True)
            finally: observer_errors.close()
            app_pids = active_gui_pids(binary)
            if len(app_pids) != 1 or Path(f'/proc/{app_pids[0]}/exe').resolve() != binary:
                raise RuntimeError('single_gui_ownership_failed')
            app_pid = app_pids[0]
            if (app_pid, 0) not in process_snapshot([launcher.pid]): raise RuntimeError('single_gui_ownership_failed')
            stage = 'observation'
            prior, sampled_at = {}, time.monotonic()
            work_deadline = min(deadline, time.monotonic()+105)
            while observer.poll() is None and time.monotonic() < work_deadline:
                sampled = process_snapshot([app_pid]) if app_pid else {}
                processes = {key: value for key, value in sampled.items() if key[1] == 0}
                peak_rss = max(peak_rss, sum(value['rss'] for value in processes.values()))
                now = time.monotonic(); elapsed = max(.001, now-sampled_at); sampled_at = now
                components = {name: dict(cpuMs=0., cpuPercentOneCore=0., rssBytes=0) for name in ['app', 'webview', 'helper']}
                for key, value in processes.items():
                    identity = (key[0], value['start'])
                    component = components[resource_component(value['name'])]
                    component['rssBytes'] += value['rss']
                    if identity in prior:
                        ticks = max(0, value['ticks']-prior[identity])
                        totals[identity] = totals.get(identity, 0)+ticks
                        component['cpuMs'] += ticks*1000/os.sysconf('SC_CLK_TCK')
                    prior[identity] = value['ticks']
                for component in components.values(): component['cpuPercentOneCore'] = component['cpuMs']/elapsed/10
                resource_samples.append(dict(atUnixMs=int(time.time()*1000), components=components))
                sample_count += 1; time.sleep(.2)
            if observer.poll() is None: raise TimeoutError('observer_deadline')
            if not observed.is_file(): raise RuntimeError('observation_missing')
            observation = json.loads(observed.read_text())
            row.update(observation, status='completed', observerExitCode=observer.returncode,
                       sampledAppTreeCpuMs=sum(totals.values())*1000/os.sysconf('SC_CLK_TCK'),
                       sampledAppTreePeakRssBytes=peak_rss, processSamples=sample_count)
    except (RuntimeError, TimeoutError) as error:
        row['originalFailure'] = row.get('originalFailure') or str(error)
        row['status'] = 'failed'
        row['failureStage'] = stage
        row['stopRemainingSetupAttempts'] = stage in ['seed', 'launcher', 'observer_setup']
    except Exception:
        row['originalFailure'] = row.get('originalFailure') or 'driver_error'; row['status'] = 'failed'
        row['failureStage'] = stage
        row['stopRemainingSetupAttempts'] = stage in ['seed', 'launcher', 'observer_setup']
    except KeyboardInterrupt:
        row['originalFailure'] = row.get('originalFailure') or 'interrupted'
        row['status'] = 'interrupted'
    finally:
        launcher_exit = launcher.poll() if launcher is not None else None
        row['observerStarted'] = observer is not None
        row['observerExitCodeBeforeCleanup'] = observer.poll() if observer is not None else None
        clean_observer = stop_owned(observer)
        clean_launcher = stop_owned(launcher)
        row['cleanupCompleted'] = clean_observer and clean_launcher
        try:
            collect_attempt_evidence(row, profile, launcher_ready, launcher_exit, resource_samples)
        except Exception:
            row.setdefault('evidenceCaptureFailures', []).append('attempt_evidence_unavailable')
        if resource_samples:
            row.update(sampledAppTreeCpuMs=sum(totals.values())*1000/os.sysconf('SC_CLK_TCK'),
                       sampledAppTreePeakRssBytes=peak_rss, processSamples=sample_count)
        if row['cleanupCompleted']:
            try: shutil.rmtree(profile)
            except OSError:
                row['cleanupCompleted'] = False
                row['originalFailure'] = row.get('originalFailure') or 'profile_cleanup_failed'
        if not row['cleanupCompleted']:
            row['retainedIsolatedProfile'] = str(profile)
            row['originalFailure'] = row.get('originalFailure') or 'owned_cleanup_failed'
        if 'transitionOk' in row:
            row['outcomes'].append(dict(stage='observation', transitionOk=row['transitionOk'],
                originalFailure=row.get('partialObserverOriginalFailure', row.get('originalFailure'))))
        cleanup_outcome = dict(stage='cleanup', completed=row['cleanupCompleted'],
            observerStopped=clean_observer, launcherStopped=clean_launcher)
        if not row['cleanupCompleted']:
            cleanup_outcome['category'] = 'profile_cleanup_failed' if clean_observer and clean_launcher else 'owned_cleanup_failed'
        row['outcomes'].append(cleanup_outcome)
        if row.get('originalFailure'):
            row['outcomes'].insert(0, dict(stage=row.get('failureStage', 'attempt'), category=row['originalFailure']))
    row['eligible'] = bool(row.get('transitionOk') and not row.get('originalFailure') and row['cleanupCompleted']
                           and row.get('sourceVideoId') == track['videoId'] and row.get('selectedFormatId')
                           and isinstance(row.get('observedHandoffDelayMs'), (int, float)))
    return row


def run(output, binary, track, phase='exploratory', budget_seconds=None):
    maximum = 600 if phase == 'exploratory' else 3600
    budget_seconds = maximum if budget_seconds is None else budget_seconds
    if isinstance(budget_seconds, bool) or not isinstance(budget_seconds, (int, float)) or not 1 <= budget_seconds <= maximum:
        raise ValueError('invalid_run_budget')
    if output.exists(): raise ValueError('output_already_exists')
    if active_gui_pids(binary): raise RuntimeError('existing_gui_refused')
    report = dict(schema=SCHEMA, phase=phase, qualification=False, promoted=False, status='running',
                  binarySha256=digest(binary), driverSha256=digest(__file__), observerSha256=digest(OBSERVER),
                  binaryRelativePath=safe_repo_path(binary),
                  track=track, wholeBudgetSeconds=budget_seconds, warmups=0,
                  limitations=LIMITATIONS, attempts=[{**row, 'status': 'not_run', 'eligible': False} for row in plan(phase)])
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x') as stream: json.dump(report, stream)
    def checkpoint():
        report['summary'] = summarize(report['attempts'], phase)
        with tempfile.NamedTemporaryFile(mode='w', dir=output.parent, prefix=f'.{output.name}.', delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(report, stream, indent=2, allow_nan=False); stream.write('\n')
        try: temporary.replace(output)
        finally: temporary.unlink(missing_ok=True)
    deadline = time.monotonic()+budget_seconds
    try:
        with single_driver_lock():
            for index, attempt in enumerate(report['attempts']):
                if time.monotonic() >= deadline: report['status'] = 'outer_deadline'; break
                result = execute_attempt(attempt, track, binary, deadline)
                report['attempts'][index] = result; checkpoint()
                if result.get('stopRemainingSetupAttempts'):
                    report['status'] = 'setup_failed'; break
                if not result.get('cleanupCompleted') or result.get('originalFailure') == 'existing_gui_refused':
                    report['status'] = 'cleanup_or_ownership_refusal'; break
                if result.get('status') == 'interrupted': report['status'] = 'interrupted'; break
            else: report['status'] = 'completed'
    except BaseException:
        report['status'] = 'interrupted'; raise
    finally:
        if (digest(binary), digest(__file__), digest(OBSERVER)) != (report['binarySha256'], report['driverSha256'], report['observerSha256']):
            report['status'] = 'source_changed_during_run'
            for row in report['attempts']: row['eligible'] = False
        checkpoint()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--phase', choices=['exploratory', 'metric'], default='exploratory')
    parser.add_argument('--track-manifest', type=Path, default=MANIFEST)
    parser.add_argument('--track', default='matthew')
    parser.add_argument('--binary', type=lambda value: Path(value).resolve())
    parser.add_argument('--output', type=Path)
    parser.add_argument('--budget-seconds', type=float)
    parser.add_argument('--run-isolated', action='store_true')
    args = parser.parse_args()
    track = select_track(args.track_manifest, args.track)
    if not args.run_isolated:
        print(json.dumps(dict(schema=SCHEMA, mode='plan-only', phase=args.phase, track=track, schedule=plan(args.phase),
                              wholeBudgetSeconds=600 if args.phase == 'exploratory' else 3600,
                              qualification=False, promoted=False, limitations=LIMITATIONS), indent=2)); return 0
    if not args.binary or not args.binary.is_file() or not os.access(args.binary, os.X_OK) or not args.output:
        parser.error('--run-isolated requires an existing debug executable and a new --output file')
    if sys.platform != 'linux': parser.error('--run-isolated requires Linux process ownership and cleanup support')
    if 'release' in args.binary.parts: parser.error('the30/45 diagnostic override requires a debug executable')
    def interrupted(_signal, _frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try: result = run(args.output.resolve(), args.binary, track, args.phase, args.budget_seconds)
    except KeyboardInterrupt:
        print('next_preparation_cancelled'); return 130
    print(json.dumps(dict(status=result['status'], eligible=sum(row['eligible'] for row in result['attempts']), qualification=False)))
    return 0 if result['status'] == 'completed' and all(row['eligible'] for row in result['attempts']) else 1


if __name__ == '__main__': raise SystemExit(main())
