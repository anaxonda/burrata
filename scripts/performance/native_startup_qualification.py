#!/usr/bin/env python3
"""Matched packaged startup/first-audio measurements on isolated synthetic profiles.

Each build/fixture condition gets discarded warmups followed by at least 20
measured captures. No global page-cache flush is attempted or claimed.
"""
from pathlib import Path
import argparse
import hashlib
import importlib.util
import json
import math
import os
import re
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
CAPTURE = ROOT / 'scripts/performance/native_runtime_capture.py'


def percentile(values, p):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.ceil(p * len(ordered)) - 1)]


def first_audio_observation(runtime, target_track_id):
    """Read first-audio only from the first fixture track's playback generation."""
    for sample in runtime.get('samples', []):
        if sample.get('trackId') != target_track_id:
            continue
        value = sample.get('firstAudioMs')
        generation = sample.get('generation')
        if (isinstance(value, (int, float)) and math.isfinite(value) and value >= 0
                and isinstance(generation, int) and generation > 0):
            return float(value), generation
    for case in runtime.get('cases', []):
        for key in ('last', 'state', 'snapshot'):
            snapshot = case.get(key, {})
            if snapshot.get('trackId') != target_track_id:
                continue
            value = snapshot.get('firstAudioMs')
            generation = snapshot.get('generation')
            if (isinstance(value, (int, float)) and math.isfinite(value) and value >= 0
                    and isinstance(generation, int) and generation > 0):
                return float(value), generation
    return None, None


def required_metric_gate(launch_to_automation_ms, first_audio_ms, generation, pcm_passed, exit_code):
    missing = []
    if launch_to_automation_ms is None:
        missing.append('appLaunchToAutomationMs')
    if first_audio_ms is None:
        missing.append('firstAudioMs')
    if generation is None:
        missing.append('firstAudioGeneration')
    return {
        'passed': not missing and pcm_passed is True and exit_code == 0,
        'missingMetrics': missing,
        'pcmPassed': pcm_passed is True,
        'processExitCode': exit_code,
    }


def capture(payload, output, build, condition, index, role, args):
    run_dir = output / build / f'pending-{condition}' / f'sample-{index:02d}'
    run_dir.mkdir(parents=True, exist_ok=False)
    command = [sys.executable, str(CAPTURE), '--payload', str(payload), '--evidence', str(run_dir),
               '--mode', 'startup', '--build-profile', args.build_profile, '--pending-quality-assets', str(condition),
               '--controller', str(args.controller), '--xvfb', str(args.xvfb)]
    environment = dict(os.environ)
    environment['BURRATA_NATIVE_RUNTIME_SCRATCH_ROOT'] = str(args.scratch_root)
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True, capture_output=True)
    evidence_path = run_dir / 'startup.json'
    if not evidence_path.is_file():
        raise RuntimeError(f'Capture produced no evidence for build={build} pending={condition} sample={index}: {result.stderr[-2000:]}')
    evidence = json.loads(evidence_path.read_text())
    runtime = evidence.get('runtime') or {}
    launch = evidence.get('appLaunchedAtUnixMs')
    ready = evidence.get('automationReadyAtUnixMs')
    profile = Path(evidence['profile'])
    fixture = json.loads((profile / 'fixture/runtime-cases.json').read_text())
    target_track_id = fixture['ids'][0]
    first_audio, generation = first_audio_observation(runtime, target_track_id)
    launch_to_automation = (ready - launch) if isinstance(ready, (int, float)) and isinstance(launch, (int, float)) else None
    pcm_passed = evidence.get('pcm', {}).get('passed') is True
    metric_gate = required_metric_gate(launch_to_automation, first_audio, generation, pcm_passed, result.returncode)
    row = {
        'build': build,
        'buildProfile': args.build_profile,
        'pendingQualityAssets': condition,
        'run': index,
        'sampleRole': role,
        'measuredRun': index - args.warmups if role == 'measured' else None,
        'processExitCode': result.returncode,
        'capturePassed': evidence.get('passed') is True and metric_gate['passed'],
        'requiredMetricGate': metric_gate,
        'binarySha256': evidence.get('binarySha256'),
        'appLaunchToAutomationMs': launch_to_automation,
        'firstAudioMs': first_audio,
        'firstAudioTrackId': target_track_id,
        'firstAudioGeneration': generation,
        'pcmPassed': pcm_passed,
        'storeMutexMetricsBefore': runtime.get('storeMutexMetricsBefore'),
        'storeMutexMetricsAfter': runtime.get('storeMutexMetricsAfter'),
        'storeMutexMetricsScope': runtime.get('storeMutexMetricsScope'),
        'storeMutexMetricsInterpretation': runtime.get('storeMutexMetricsInterpretation'),
        'evidence': str(evidence_path),
        'profile': evidence.get('profile'),
        'error': evidence.get('error'),
    }
    (run_dir / 'summary.json').write_text(json.dumps(row, indent=2) + '\n')
    return row


def full_natural_capture(payload, output, build, args):
    """Qualify one complete two-track natural queue separately from startup timing."""
    run_dir = output / 'full-natural' / build
    run_dir.mkdir(parents=True, exist_ok=False)
    command = [sys.executable, str(CAPTURE), '--payload', str(payload), '--evidence', str(run_dir),
               '--mode', 'natural', '--build-profile', args.build_profile, '--pending-quality-assets', '0',
               '--controller', str(args.controller), '--xvfb', str(args.xvfb)]
    environment = dict(os.environ)
    environment['BURRATA_NATIVE_RUNTIME_SCRATCH_ROOT'] = str(args.scratch_root)
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True, capture_output=True)
    evidence_path = run_dir / 'natural.json'
    if not evidence_path.is_file():
        raise RuntimeError(f'Full natural capture produced no evidence for build={build}: {result.stderr[-2000:]}')
    evidence = json.loads(evidence_path.read_text())
    runtime = evidence.get('runtime') or {}
    passed = (result.returncode == 0 and evidence.get('passed') is True
              and runtime.get('passed') is True and evidence.get('pcm', {}).get('passed') is True)
    return {'build': build, 'captureMode': 'full-natural-two-track', 'processExitCode': result.returncode,
            'passed': passed, 'binarySha256': evidence.get('binarySha256'),
            'pcmPassed': evidence.get('pcm', {}).get('passed') is True,
            'evidence': str(evidence_path), 'profile': evidence.get('profile'), 'error': evidence.get('error')}


def existing_full_natural_check(payload, build, evidence_path):
    """Reuse a separately captured natural run only when it matches this payload."""
    evidence_path = evidence_path.resolve(strict=True)
    evidence = json.loads(evidence_path.read_text())
    runtime = evidence.get('runtime') or {}
    expected_binary_hash = hashlib.sha256((payload / 'usr/bin/burrata').read_bytes()).hexdigest()
    passed = (evidence.get('mode') == 'natural' and evidence.get('passed') is True and runtime.get('passed') is True
              and evidence.get('pcm', {}).get('passed') is True
              and evidence.get('binarySha256') == expected_binary_hash)
    return {'build': build, 'captureMode': 'full-natural-two-track-preflight', 'reused': True,
            'passed': passed, 'binarySha256': evidence.get('binarySha256'),
            'pcmPassed': evidence.get('pcm', {}).get('passed') is True,
            'evidence': str(evidence_path), 'profile': evidence.get('profile'),
            'error': None if passed else 'Preflight evidence failed or its binary hash did not match the payload'}


def idle_ipc_capture(payload, output, build, args):
    """Measure one 20-second idle WebView ::ipc window in a separate process."""
    run_dir = output / 'idle-ipc' / build
    run_dir.mkdir(parents=True, exist_ok=False)
    command = [sys.executable, str(CAPTURE), '--payload', str(payload), '--evidence', str(run_dir),
               '--mode', 'idle-ipc', '--build-profile', args.build_profile, '--pending-quality-assets', '0',
               '--controller', str(args.controller), '--xvfb', str(args.xvfb), '--ipc-counter',
               '--ipc-counter-shim', str(args.ipc_counter_shim)]
    environment = dict(os.environ)
    environment['BURRATA_NATIVE_RUNTIME_SCRATCH_ROOT'] = str(args.scratch_root)
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True, capture_output=True)
    evidence_path = run_dir / 'idle-ipc.json'
    if not evidence_path.is_file():
        raise RuntimeError(f'Idle IPC capture produced no evidence for build={build}: {result.stderr[-2000:]}')
    evidence = json.loads(evidence_path.read_text())
    runtime = evidence.get('runtime') or {}
    sample = runtime.get('idleIpc') or {}
    passed = (result.returncode == 0 and evidence.get('passed') is True
              and runtime.get('passed') is True and evidence.get('ipcCounterPassed') is True)
    return {'build': build, 'captureMode': 'idle-webview-ipc-3-warmup-plus-20-buckets', 'processExitCode': result.returncode,
            'passed': passed, 'binarySha256': evidence.get('binarySha256'),
            'registeredBefore': sample.get('registeredBefore'), 'registeredAfter': sample.get('registeredAfter'),
            'countBefore': sample.get('countBefore'), 'countAfter': sample.get('countAfter'),
            'countDelta': sample.get('countDelta'), 'warmupBucketCount': sample.get('warmupBucketCount'),
            'measuredBucketCount': sample.get('measuredBucketCount'),
            'actualElapsedMs': sample.get('actualElapsedMs'), 'measuredElapsedMs': sample.get('measuredElapsedMs'),
            'ratePerSecond': sample.get('ratePerSecond'), 'measuredBuckets': sample.get('measuredBuckets'),
            'counterTargetVerified': evidence.get('ipcCounterTargetVerified') is True,
            'evidence': str(evidence_path), 'profile': evidence.get('profile'), 'error': evidence.get('error')}


def summarize(rows, builds, measured_runs, warmups):
    result = {}
    for build in builds:
        result[build] = {}
        for pending in (0, 100):
            all_samples = [row for row in rows if row['build'] == build and row['pendingQualityAssets'] == pending]
            measured = [row for row in all_samples if row['sampleRole'] == 'measured']
            discarded = [row for row in all_samples if row['sampleRole'] == 'warmup']
            measures = {}
            for metric in ('appLaunchToAutomationMs', 'firstAudioMs'):
                values = [row[metric] for row in measured if row[metric] is not None]
                measures[metric] = {
                    'count': len(values),
                    'medianMs': statistics.median(values) if values else None,
                    'p95Ms': percentile(values, .95),
                }
            result[build][str(pending)] = {
                'runCount': len(measured),
                'warmupCount': len(discarded),
                'allCapturesPassed': len(measured) == measured_runs and len(discarded) == warmups
                    and all(row['capturePassed'] and row['pcmPassed'] for row in all_samples),
                'allRequiredMetricsPresent': len(all_samples) == measured_runs + warmups
                    and all(not row['requiredMetricGate']['missingMetrics'] for row in all_samples),
                'binarySha256': sorted({row['binarySha256'] for row in all_samples}),
                'measures': measures,
            }
    return result


def preflight(controller_path, xvfb_path):
    if not controller_path.is_file() or not os.access(controller_path, os.R_OK):
        raise RuntimeError(f'Private audio controller is unavailable: {controller_path}')
    if not xvfb_path.is_file() or not os.access(xvfb_path, os.X_OK):
        raise RuntimeError(f'Extracted Xvfb is unavailable: {xvfb_path}')
    spec = importlib.util.spec_from_file_location('startup_private_audio', controller_path)
    controller = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(controller)
    state = controller.running_state()
    status = state.get('status')
    if status != 'ready-private-no-sinks':
        raise RuntimeError(f'Private audio controller is not ready: {status}')
    return {'controller': str(controller_path.resolve()), 'xvfb': str(xvfb_path.resolve()),
            'privateAudioStatus': status}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--payload', type=Path, help='One extracted DEB payload root')
    parser.add_argument('--label', default='build', help='Label for --payload')
    parser.add_argument('--baseline-payload', type=Path, help='Extracted baseline DEB payload root')
    parser.add_argument('--final-payload', type=Path, help='Extracted final DEB payload root')
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--runs', type=int, default=20, help='Measured runs per condition; minimum 20')
    parser.add_argument('--warmups', type=int, default=3, help='Discarded warmup samples per condition')
    parser.add_argument('--build-profile', choices=['diagnostic', 'release'], default='diagnostic',
                        help='Evidence label for the already-built payload (does not build it)')
    parser.add_argument('--baseline-natural-evidence', type=Path,
                        help='Reuse a separately captured baseline full-natural preflight if its binary hash matches')
    parser.add_argument('--ipc-counter', action='store_true',
                        help='Opt in to one separate 20-second idle WebView IPC capture per payload')
    parser.add_argument('--ipc-counter-shim', type=Path,
                        help='Built PID-gated shared object (required with --ipc-counter)')
    parser.add_argument('--controller', type=Path, default=Path('/tmp/burrata-queue-recovery-private-audio-controller.py'))
    parser.add_argument('--xvfb', type=Path, default=Path('/tmp/burrata-private-xvfb/root/usr/bin/Xvfb'))
    parser.add_argument('--scratch-root', type=Path, default=Path('/tmp/burrata-startup-scratch'))
    args = parser.parse_args()
    if args.runs < 20:
        parser.error('--runs must be at least 20 measured samples')
    if args.warmups < 1:
        parser.error('--warmups must be at least 1')
    if args.ipc_counter and not args.ipc_counter_shim:
        parser.error('--ipc-counter requires --ipc-counter-shim')
    if args.ipc_counter_shim and not args.ipc_counter:
        parser.error('--ipc-counter-shim requires --ipc-counter')
    if args.ipc_counter:
        args.ipc_counter_shim = args.ipc_counter_shim.resolve(strict=True)
        if not args.ipc_counter_shim.is_file():
            parser.error('--ipc-counter-shim must be a regular file')
    if args.payload and (args.baseline_payload or args.final_payload):
        parser.error('use --payload or baseline/final payloads')
    if not args.payload and not (args.baseline_payload or args.final_payload):
        parser.error('provide --payload or at least one baseline/final payload')
    if args.baseline_natural_evidence and not args.baseline_payload:
        parser.error('--baseline-natural-evidence requires --baseline-payload')
    if not re.fullmatch(r'[A-Za-z0-9._-]{1,40}', args.label):
        parser.error('--label must contain only letters, digits, dot, underscore, or hyphen')
    payloads = {}
    if args.payload:
        payloads[args.label] = args.payload.resolve(strict=True)
    if args.baseline_payload:
        payloads['baseline'] = args.baseline_payload.resolve(strict=True)
    if args.final_payload:
        payloads['final'] = args.final_payload.resolve(strict=True)
    args.scratch_root = args.scratch_root.resolve()
    if args.scratch_root == Path('/tmp') or not args.scratch_root.is_relative_to(Path('/tmp')):
        parser.error('--scratch-root must be a dedicated directory under /tmp')
    prerequisites = preflight(args.controller, args.xvfb)
    args.evidence.mkdir(parents=True, exist_ok=True)
    args.scratch_root.mkdir(parents=True, exist_ok=True)
    rows = []
    total_samples = args.warmups + args.runs
    for sample_index in range(1, total_samples + 1):
        conditions = (0, 100) if sample_index % 2 else (100, 0)
        builds = list(payloads)
        if sample_index % 2:
            builds.reverse()
        role = 'warmup' if sample_index <= args.warmups else 'measured'
        for pending in conditions:
            for build in builds:
                rows.append(capture(payloads[build], args.evidence.resolve(), build, pending,
                                    sample_index, role, args))
    full_natural = []
    for build, payload in payloads.items():
        if build=='baseline' and args.baseline_natural_evidence:
            full_natural.append(existing_full_natural_check(payload, build, args.baseline_natural_evidence))
        else:
            full_natural.append(full_natural_capture(payload, args.evidence.resolve(), build, args))
    idle_ipc = ([idle_ipc_capture(payload, args.evidence.resolve(), build, args)
                 for build, payload in payloads.items()] if args.ipc_counter else [])
    summary = {
        'payloads': {name: str(path) for name, path in payloads.items()},
        'measuredRunsPerCondition': args.runs,
        'warmupsPerCondition': args.warmups,
        'buildProfile': args.build_profile,
        'captureMode': 'first-track-startup',
        'fullNaturalChecks': full_natural,
        'idleIpcChecks': idle_ipc,
        'profilePolicy': 'fresh isolated /tmp profile for every sample; same synthetic fixture recipe and backlog count per arm',
        'warmups': 'first samples per condition are excluded from medians and p95; no global page-cache flush or cold-cache claim',
        'prerequisites': prerequisites,
        'results': summarize(rows, list(payloads), args.runs, args.warmups),
        'runs': rows,
    }
    path = args.evidence / 'summary.json'
    path.write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps({'summary': str(path), 'results': summary['results']}))
    startup_passed = all(condition['runCount'] >= 20 and condition['allCapturesPassed']
                         and condition['allRequiredMetricsPresent']
                         for build in summary['results'].values() for condition in build.values())
    natural_passed = all(check['passed'] for check in full_natural)
    idle_ipc_passed = all(check['passed'] for check in idle_ipc)
    return 0 if startup_passed and natural_passed and idle_ipc_passed else 1


if __name__ == '__main__':
    raise SystemExit(main())
