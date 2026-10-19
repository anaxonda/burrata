#!/usr/bin/env python3
"""Isolated playback reliability experiments; no GUI, profile, account or device.

Modes: offline FFmpeg fixture, live anonymous extraction, existing native Rust
same-byte decoder comparison. Results explicitly distinguish warm helper state
from retained decoder state. Every command has an absolute deadline.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import selectors
import signal
import statistics
import subprocess
import tempfile
import time

from playback_reliability_fixture import FixtureServer

ROOT = Path(__file__).resolve().parents[2]
MEASURE = ROOT / 'scripts/performance/innertube/measure_child.py'
SCHEMA = 'burrata.playback-reliability/v1'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def summary(rows):
    groups = []
    for mode, scenario in sorted({(r['strategy'], r['scenario']) for r in rows if not r['warmup']}):
        attempts = [r for r in rows if not r['warmup'] and r['strategy'] == mode and r['scenario'] == scenario]
        item = dict(strategy=mode, scenario=scenario, attempts=len(attempts), successes=sum(r['ok'] for r in attempts), failures={})
        for row in attempts:
            if not row['ok']:
                code = row.get('failure', 'UNKNOWN')
                item['failures'][code] = item['failures'].get(code, 0) + 1
        for key in ['firstPcmMs', 'firstPcmFromCommandMs', 'wallMs', 'totalCpuMs', 'helperCpuMs', 'decoderCpuMs', 'helperRssBytes', 'decoderMaxRssBytes']:
            values = sorted(r[key] for r in attempts if r['ok'] and isinstance(r.get(key), (int, float)) and math.isfinite(r[key]))
            if values:
                item[key] = dict(median=statistics.median(values), p95=values[math.ceil(.95 * len(values)) - 1], worst=values[-1])
        groups.append(item)
    return groups


def terminate_owned_tree(root_pid, signum, previous=()):
    """Catch owned children that deliberately have their own process groups."""
    parents = {}
    if Path('/proc').is_dir():
        for path in Path('/proc').glob('[0-9]*/stat'):
            try:
                stat = path.read_text().rsplit(')', 1)[1].split()
                parents[int(path.parent.name)] = int(stat[1])
            except (OSError, ValueError, IndexError):
                pass
    else:
        try:
            for line in subprocess.check_output(['ps', '-eo', 'pid=,ppid='], text=True, timeout=2).splitlines():
                pid, parent = map(int, line.split())
                parents[pid] = parent
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    owned = set(previous) | {root_pid}
    while True:
        found = {pid for pid, parent in parents.items() if parent in owned}
        if found <= owned:
            break
        owned |= found
    for pid in sorted(owned - {root_pid}, reverse=True) + [root_pid]:
        try:
            os.kill(pid, signum)
        except ProcessLookupError:
            pass
    return owned


class Worker:
    """Owned process group. wait4 wrapper gives kernel CPU/maxRSS on shutdown."""
    def __init__(self, command, directory, env, budget_ms):
        self.directory = directory
        self.accounting = directory / ('process-' + str(time.monotonic_ns()) + '.json')
        self.process = subprocess.Popen(['python3', str(MEASURE), str(self.accounting), *command],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                        env=env, start_new_session=True, bufsize=0)
        self.budget_ms = budget_ms
        self.buffer = b''
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)

    def request(self, request):
        started = time.monotonic()
        try:
            self.process.stdin.write((json.dumps(request) + '\n').encode())
            self.process.stdin.flush()
            deadline = started + self.budget_ms / 1000 + .5
            while b'\n' not in self.buffer:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not self.selector.select(remaining):
                    self.close(force=True)
                    return dict(ok=False, failure='HARNESS_TIMEOUT', accountingComplete=False, wallMs=(time.monotonic() - started) * 1000)
                chunk = os.read(self.process.stdout.fileno(), 65536)
                if not chunk:
                    return dict(ok=False, failure='HELPER_EXITED', accountingComplete=False, wallMs=(time.monotonic() - started) * 1000)
                self.buffer += chunk
                if len(self.buffer) > 1024 * 1024:
                    self.close(force=True)
                    return dict(ok=False, failure='INVALID_PROTOCOL', accountingComplete=False)
            line, self.buffer = self.buffer.split(b'\n', 1)
            result = json.loads(line)
            if not isinstance(result.get('ok'), bool):
                raise ValueError()
            result['wallMs'] = (time.monotonic() - started) * 1000
            return result
        except (OSError, ValueError):
            return dict(ok=False, failure='INVALID_PROTOCOL', accountingComplete=False, wallMs=(time.monotonic() - started) * 1000)

    def close(self, force=False):
        if self.process.poll() is None:
            try:
                if force:
                    owned = terminate_owned_tree(self.process.pid, signal.SIGTERM)
                else:
                    self.process.stdin.close()
                self.process.wait(timeout=1)
            except (OSError, subprocess.TimeoutExpired):
                try:
                    terminate_owned_tree(self.process.pid, signal.SIGKILL, locals().get('owned', ()))
                except ProcessLookupError:
                    pass
                self.process.wait()
        self.selector.close()
        self.process.stdout.close()
        if not self.process.stdin.closed:
            self.process.stdin.close()
        return json.loads(self.accounting.read_text()) if self.accounting.exists() else dict(accountingComplete=False)


def run_command(command, directory, env, timeout):
    """Bounded fixture/build/test command; all output is private and discarded."""
    usage_path = directory / ('command-' + str(time.monotonic_ns()) + '.json')
    started = time.monotonic()
    proc = subprocess.Popen(['python3', str(MEASURE), str(usage_path), *command],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            env=env, cwd=ROOT, start_new_session=True)
    try:
        code = proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        owned = terminate_owned_tree(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=.5)
        except subprocess.TimeoutExpired:
            terminate_owned_tree(proc.pid, signal.SIGKILL, owned)
            proc.wait()
        terminate_owned_tree(proc.pid, signal.SIGKILL, owned)
        code = -1
    usage = json.loads(usage_path.read_text()) if usage_path.exists() else dict(accountingComplete=False)
    return dict(ok=code == 0, failure=None if code == 0 else 'COMMAND_TIMEOUT' if code == -1 else 'COMMAND_FAILED',
                wallMs=(time.monotonic() - started) * 1000, **usage)


def run_worker_matrix(args, scratch, report):
    env = dict(os.environ)
    transport = None
    if args.mode == 'offline':
        fixture = args.fixture or scratch / 'controlled.opus'
        if not args.fixture:
            generation = run_command(['ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i',
                                      f'sine=frequency=440:sample_rate=48000:duration={args.duration}', '-ac', '2', '-c:a', 'libopus', '-b:a', '128k', '-y', str(fixture)], scratch, env, args.budget_ms / 1000)
            report['fixtureGeneration'] = generation
            if not generation['ok']:
                report['failures'].append(dict(stage='fixture', failure=generation['failure']))
                return
        report['fixture'] = dict(sha256=digest(fixture), bytes=fixture.stat().st_size, source='supplied-local-bytes' if args.fixture else 'generated-sine-440Hz', durationSeconds=args.duration)
        media_input = str(fixture)
        transport = FixtureServer(fixture, args.fixture_fault) if args.transport == 'loopback' else None
        if transport:
            media_input = transport.url
        env['BURRATA_RELIABILITY_FIXTURE'] = media_input
        command = ['node', str(ROOT / 'scripts/performance/playback_reliability_fixture_worker.mjs')]
        base = dict(fixture=media_input, budgetMs=args.budget_ms)
        strategy = 'ffmpeg-fresh-decoder'
    else:
        command = ['node', str(ROOT / 'scripts/performance/innertube/worker.mjs')]
        env['BURRATA_BENCH_TOKEN_MODE'] = args.token_mode
        base = dict(action='play', videoId=args.video_id, expectedVideoId=args.expected_video_id, budgetMs=args.budget_ms)
        strategy = 'youtubei-18.1.0-ffmpeg'
        report['identity'] = dict(videoId=args.video_id, expectedVideoId=args.expected_video_id, scope='explicit-provider-video-id; not acoustic verification')
    persistent = None
    try:
        scenarios = [('cold-helper', 0), ('warm-helper', 0), ('seek-early', .01), ('seek-middle', .5), ('seek-late', .95)]
        for scenario, fraction in scenarios:
            for iteration in range(-3, args.repetitions):
                warmup = iteration < 0
                cold = scenario == 'cold-helper'
                if cold or persistent is None:
                    session = scratch / ('session-' + str(time.monotonic_ns()))
                    session.mkdir(mode=0o700)
                    worker_env = dict(env, BURRATA_BENCH_DATA_DIR=str(session))
                    worker = Worker(command, scratch, worker_env, args.budget_ms)
                    if not cold:
                        persistent = worker
                else:
                    worker = persistent
                position = round(max(0, args.duration - 5) * fraction, 3)
                request = dict(base, position=position)
                if args.mode == 'live' and scenario.startswith('seek'):
                    request['action'] = 'seek'
                row = worker.request(request)
                row.update(strategy=strategy, scenario=scenario, iteration=max(0, iteration), warmup=warmup, positionSeconds=position)
                if args.mode == 'live':
                    # Do not carry search titles, URLs, session bindings or tokens into reports.
                    row.pop('candidates', None)
                report['attempts'].append(row)
                if cold:
                    report['processAccounting'].append(dict(scope='cold-helper-lifecycle', **worker.close()))
                if row.get('failure') in ['HELPER_EXITED', 'HARNESS_TIMEOUT', 'INVALID_PROTOCOL'] and worker is persistent:
                    report['processAccounting'].append(dict(scope='failed-helper-lifecycle', **worker.close(force=True)))
                    persistent = None
        if args.mode == 'offline':
            broken = scratch / 'corrupt.opus'
            broken.write_bytes(b'controlled invalid media\n')
            broken_env = dict(env, BURRATA_RELIABILITY_FIXTURE=str(broken))
            worker = Worker(command, scratch, broken_env, args.budget_ms)
            row = worker.request(dict(fixture=str(broken), budgetMs=args.budget_ms))
            row.update(strategy=strategy, scenario='corrupt-media', iteration=0, warmup=False, expectedFailure='DECODE_FAILED')
            report['attempts'].append(row)
            report['processAccounting'].append(dict(scope='corrupt-fixture', **worker.close()))
    finally:
        if persistent:
            report['processAccounting'].append(dict(scope='warm-helper-lifecycle', **persistent.close()))
        if transport:
            transport.close()
            report['transportRequests'] = transport.requests
        if args.mode == 'offline' and digest(fixture) != report['fixture']['sha256']:
            report['failures'].append(dict(stage='fixture', failure='FIXTURE_CHANGED'))


def validate_native_rows(rows, compare):
    strategies = ['ffmpegFresh', 'opusRetained'] if compare else ['fresh', 'retained']
    expected = {(strategy, iteration, fraction) for strategy in strategies for iteration in range(20) for fraction in [.01, .5, .95]}
    if not isinstance(rows, list) or len(rows) != len(expected):
        return False
    observed = set()
    for row in rows:
        if not isinstance(row, dict):
            return False
        try:
            observed.add((row['strategy'], row['iteration'], row['fraction']))
        except (KeyError, TypeError):
            return False
        for key in ['firstPcmMs', 'wallMs', 'decodeThreadCpuMs']:
            value = row.get(key)
            if not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
                return False
    return observed == expected


def run_native(args, scratch, report):
    """Call repository's existing single-test native benchmark on identical bytes."""
    fixture = args.fixture
    report['fixture'] = dict(sha256=digest(fixture), bytes=fixture.stat().st_size,
                             source='supplied-local-bytes', durationSeconds=args.duration)
    report['warmupsPerScenario'] = 3
    report['nativeWarmups'] = {'existing-native': 3, 'opus-vs-ffmpeg': 3}
    report['limitations'] += ['Native benchmark has fixed 20 repetitions per position and three warmups per path in both comparisons.',
                              'Native per-attempt CPU covers decode thread and FFmpeg children; process accounting includes all native helper threads over each full run.',
                              'Native benchmark exposes seek PCM only; cold-start, paused seek and audible output require separate production qualification.']
    transport = FixtureServer(fixture, args.fixture_fault) if args.transport == 'loopback' else None
    media_input = transport.url if transport else str(fixture)
    try:
        for strategy, features, compare in [('existing-native', [], False), ('opus-vs-ffmpeg', ['experimental-opus'], True)]:
            result_path = scratch / (strategy + '.json')
            env = dict(os.environ, BURRATA_SEEK_BENCH_FIXTURE=media_input, BURRATA_SEEK_BENCH_OUTPUT=str(result_path),
                       BURRATA_SEEK_BENCH_DURATION=str(args.duration), TAURI_CONFIG='{"bundle":{"resources":[]}}')
            env.pop('BURRATA_FORCE_FFMPEG', None)
            env.pop('BURRATA_SEEK_BENCH_COMPARE_OPUS', None)
            if compare:
                env['BURRATA_SEEK_BENCH_COMPARE_OPUS'] = '1'
            command = ['cargo', 'test', '--offline', '--manifest-path', str(ROOT / 'src-tauri/Cargo.toml'), '--lib']
            if features:
                command += ['--features', ','.join(features)]
            if args.profile == 'release':
                command.append('--release')
            command += ['seek_decoder_benchmark', '--', '--ignored', '--test-threads=1']
            outcome = run_command(command, scratch, env, args.native_budget_seconds)
            report['processAccounting'].append(dict(scope=strategy + '-build-and-full-test', **outcome))
            if not outcome['ok'] or not result_path.exists():
                report['failures'].append(dict(stage=strategy, failure=outcome.get('failure') or 'MISSING_NATIVE_REPORT'))
                continue
            native_rows = json.loads(result_path.read_text())
            if not validate_native_rows(native_rows, compare):
                report['failures'].append(dict(stage=strategy, failure='INCOMPLETE_NATIVE_REPORT'))
                continue
            for raw in native_rows:
                report['attempts'].append(dict(strategy=strategy + ':' + raw['strategy'], scenario='seek-' + str(raw['fraction']),
                                              warmup=False, ok=True, iteration=raw['iteration'], positionSeconds=args.duration * raw['fraction'],
                                              firstPcmMs=raw['firstPcmMs'], wallMs=raw['wallMs'],
                                              decodeThreadCpuMs=raw['decodeThreadCpuMs'], decoderCpuMs=raw.get('decoderChildCpuMs'),
                                              accountingScope='decode-thread-and-reaped-child-cpu; whole-process totals in processAccounting'))
    finally:
        if transport:
            transport.close()
            report['transportRequests'] = transport.requests
        if digest(fixture) != report['fixture']['sha256']:
            report['failures'].append(dict(stage='fixture', failure='FIXTURE_CHANGED'))


def version(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=3, check=True)
        return result.stdout.splitlines()[0]
    except (OSError, subprocess.SubprocessError, IndexError):
        return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['offline', 'live', 'native'], default='offline')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--fixture', type=lambda p: Path(p).resolve())
    parser.add_argument('--transport', choices=['file', 'loopback'], default='file')
    parser.add_argument('--fixture-fault', choices=['none', 'deny-first', 'truncate', 'stall'], default='none')
    parser.add_argument('--duration', type=float, default=30)
    parser.add_argument('--repetitions', type=int, default=20)
    parser.add_argument('--smoke', action='store_true', help='allow fewer than 20; never a qualification')
    parser.add_argument('--budget-ms', type=int, default=45000)
    parser.add_argument('--native-budget-seconds', type=int, default=1800)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--video-id')
    parser.add_argument('--expected-video-id')
    parser.add_argument('--token-mode', choices=['bgutil', 'none'], default='bgutil')
    args = parser.parse_args(argv)
    if args.repetitions < 1 or (args.repetitions < 20 and not args.smoke):
        parser.error('qualification requires at least 20 measured repetitions; use --smoke for a smaller diagnostic')
    if not math.isfinite(args.duration) or args.duration < 6 or not 1 <= args.budget_ms <= 120000:
        parser.error('duration must be >=6 seconds and budget-ms in 1..120000')
    if args.native_budget_seconds < 1:
        parser.error('native budget must be positive')
    if args.mode == 'native' and (not args.fixture or args.repetitions != 20):
        parser.error('native mode requires a local --fixture and fixed --repetitions 20')
    if args.fixture_fault != 'none' and args.transport != 'loopback':
        parser.error('fixture faults require --transport loopback')
    if args.mode == 'live':
        import re
        if not re.fullmatch(r'[A-Za-z0-9_-]{11}', args.video_id or '') or args.video_id != args.expected_video_id:
            parser.error('live mode requires matching explicit --video-id and --expected-video-id')
    report = dict(schema=SCHEMA, mode=args.mode, qualification=False,
                  transport=args.transport if args.mode != 'live' else 'provider', fixtureFault=args.fixture_fault,
                  repetitions=args.repetitions, warmupsPerScenario=3, smoke=args.smoke,
                  firstPcmScope='firstPcmMs measures decoder startup; firstPcmFromCommandMs includes helper preparation and extraction when available',
                  scope='first decoded PCM; excludes mixer, device, GUI and acoustic recording verification',
                  environment=dict(system=platform.system(), architecture=platform.machine(), python=platform.python_version(),
                                   node=version(['node', '--version']), ffmpeg=version(['ffmpeg', '-version']), rustc=version(['rustc', '--version']),
                                   youtubeiVersion='18.1.0', bgutilVersion='2.0.2', revision=version(['git', 'rev-parse', 'HEAD'])),
                  limits=dict(commandBudgetMs=args.budget_ms, nativeCommandBudgetSeconds=args.native_budget_seconds),
                  attempts=[], failures=[], processAccounting=[],
                  limitations=['Warm helper is distinct from a retained decoder; FFmpeg subprocess starts anew for every excerpt.',
                               'Signed URLs, tokens and visitor bindings are never included in reports.',
                               'OS page cache is not dropped; cold means a new isolated helper/session.',
                               'maxRSS is a per-process lifetime high-water mark, not simultaneous tree RSS.',
                               'Failure rows may lack per-stage accounting; process lifecycle accounting includes reaped descendants.'])
    report['sourceHashes'] = {str(path.relative_to(ROOT)): digest(path) for path in [ROOT / name for name in ['scripts/performance/playback_reliability.py', 'scripts/performance/playback_reliability_fixture.py', 'scripts/performance/playback_reliability_fixture_worker.mjs', 'scripts/performance/innertube/core.mjs', 'scripts/performance/innertube/budget.mjs', 'scripts/performance/innertube/worker.mjs', 'scripts/performance/innertube/token.mjs', 'scripts/performance/innertube/token_child.mjs', 'scripts/performance/innertube/measure_child.py', 'scripts/performance/innertube/package-lock.json', 'src-tauri/src/native_output.rs', 'src-tauri/Cargo.lock']]}
    with tempfile.TemporaryDirectory(prefix='burrata-reliability-') as directory:
        scratch = Path(directory)
        try:
            (run_native if args.mode == 'native' else run_worker_matrix)(args, scratch, report)
        except (OSError, ValueError, subprocess.SubprocessError):
            report['failures'].append(dict(stage='harness', failure='HARNESS_FAILED'))
    report['summary'] = summary(report['attempts'])
    report['expectedFailureChecks'] = [dict(scenario=r['scenario'], passed=r.get('failure') == r['expectedFailure']) for r in report['attempts'] if r.get('expectedFailure')]
    report['runPassed'] = bool(report['attempts']) and not report['failures'] and all(r['ok'] or r.get('expectedFailure') == r.get('failure') for r in report['attempts'])
    # Qualification is evidence coverage, never inferred from successful excerpts.
    report['qualification'] = False
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    print(json.dumps(dict(report=str(args.output), runPassed=report['runPassed'], qualification=False, attempts=len(report['attempts']))))
    return 0 if report['runPassed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
