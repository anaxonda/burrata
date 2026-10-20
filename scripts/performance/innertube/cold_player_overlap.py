#!/usr/bin/env python3
"""Six anonymous fixed-format cold requests; no invocation runs on import."""
import argparse
import json
import math
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = ROOT / 'scripts/performance/innertube'
sys.path.insert(0, str(HERE))
from serial_same_worker import BoundedLineReader, ProtocolError, safe_response, terminate_group, preflight, file_sha256

VIDEO = 'uZGRATckoBM'
CODEC = 'audio/webm; codecs="opus"'
NODE = ROOT / 'src-tauri/resources/tools/x86_64-unknown-linux-gnu/node'
FFMPEG = NODE.with_name('ffmpeg')
SOURCE_FILES = [Path(__file__), HERE / 'serial_same_worker.py', HERE / 'worker.mjs',
                HERE / 'core.mjs', HERE / 'client_factory.mjs', HERE / 'token.mjs',
                HERE / 'token_child.mjs', HERE / 'package-lock.json', NODE, FFMPEG]

def hashes():
    result = {}
    for path in SOURCE_FILES:
        key = str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else path.name
        result[key] = file_sha256(path)
    return result

def run(output):
    if output.exists():
        raise SystemExit('output already exists')
    checks = preflight()
    if not checks['ok']:
        raise SystemExit('offline asset preflight failed')
    initial_hashes = hashes()
    started = time.monotonic()
    outer_deadline = started + 140
    rows = []
    output.parent.mkdir(parents=True, exist_ok=True)
    def checkpoint(status):
        pairs = []
        for pair in range(1, 4):
            by_arm = {r['arm']: r for r in rows if r['pair'] == pair}
            valid = (len(by_arm) == 2 and all(r.get('eligible') for r in by_arm.values()) and
                     by_arm['parallel-player'].get('headerNames') == by_arm['sequential'].get('headerNames'))
            pairs.append({'pair': pair, 'eligible': valid,
                          'parallelMinusSequentialFirstPcmMs':
                          round(by_arm['parallel-player']['firstPcmFromCommandMs'] -
                                by_arm['sequential']['firstPcmFromCommandMs'], 3) if valid else None})
        report = {'schema': 'innertube-cold-player-overlap-v1', 'qualification': False,
                  'status': status, 'preflight': checks, 'sourceSha256': initial_hashes,
                  'scope': 'anonymous exact public source ID; decoder-only five-second excerpt; no catalog/acoustic/device claim',
                  'videoId': VIDEO, 'formatId': 251, 'codec': CODEC, 'clientName': 'MWEB',
                  'warmups': 0, 'order': ['AB', 'BA', 'AB'], 'perAttemptBudgetMs': 20000,
                  'outerBudgetMs': 140000, 'startupPolicy': 'fresh worker and task XDG/cache for every attempt; HOME unchanged',
                  'timingOrigin': 'firstPcmFromCommandMs is worker JSONL-receipt to first PCM and excludes process launch; processLaunchToResponseMs includes launch but ends after five-second excerpt; no derived launch-inclusive first PCM',
                  'totalWallMs': round((time.monotonic() - started) * 1000, 3),
                  'requests': rows, 'pairs': pairs}
        temporary = output.with_suffix(output.suffix + '.tmp')
        temporary.write_text(json.dumps(report, indent=2) + '\n')
        temporary.replace(output)
        return report
    checkpoint('running')
    for pair, order in enumerate(((0, 1), (1, 0), (0, 1)), 1):
        for flag in order:
            arm = 'parallel-player' if flag else 'sequential'
            request_id = f'cold-pair-{pair}-{flag}'
            row = {'id': request_id, 'pair': pair, 'arm': arm, 'eligible': False}
            if time.monotonic() >= outer_deadline:
                rows.append({**row, 'status': 'NOT_RUN', 'failure': 'OUTER_DEADLINE'})
                checkpoint('running')
                continue
            with tempfile.TemporaryDirectory(prefix='burrata-cold-player-') as scratch:
                base = Path(scratch)
                env = os.environ.copy()
                for key in tuple(env):
                    if key.startswith('BURRATA_BENCH_'):
                        env.pop(key)
                env.update({'BURRATA_BENCH_DATA_DIR': str(base / 'data'),
                            'BURRATA_BENCH_CLIENT': 'MWEB', 'BURRATA_BENCH_TOKEN_MODE': 'bgutil',
                            'BURRATA_BENCH_PARALLEL_PLAYER': str(flag), 'BURRATA_BENCH_IDLE_MS': '30000',
                            'BURRATA_BENCH_FFMPEG': str(FFMPEG),
                            'XDG_CONFIG_HOME': str(base / 'config'), 'XDG_CACHE_HOME': str(base / 'cache'),
                            'XDG_DATA_HOME': str(base / 'state')})
                for key in ('BURRATA_BENCH_DATA_DIR', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_DATA_HOME'):
                    Path(env[key]).mkdir(mode=0o700)
                proc = None
                attempt_started = time.monotonic()
                deadline = min(attempt_started + 20, outer_deadline)
                try:
                    proc = subprocess.Popen([str(NODE), str(HERE / 'worker.mjs')], cwd=ROOT, env=env,
                                            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                            stderr=subprocess.DEVNULL, bufsize=0, start_new_session=True)
                    request = {'id': request_id, 'action': 'play', 'videoId': VIDEO,
                               'expectedVideoId': VIDEO, 'formatId': 251, 'seconds': 5,
                               'budgetMs': max(1, int((deadline - time.monotonic()) * 1000))}
                    proc.stdin.write((json.dumps(request, separators=(',', ':')) + '\n').encode())
                    proc.stdin.flush()
                    response = safe_response(BoundedLineReader(proc.stdout.fileno()).read(deadline), request_id)
                    row.update(response)
                    metadata = response.get('providerMetadata', {})
                    duration = metadata.get('durationSeconds')
                    row['metadataMatched'] = (metadata.get('videoId') == VIDEO and
                        (metadata.get('title') or '').strip().casefold() == 'matthew' and
                        'john denver' in (metadata.get('channel') or '').casefold() and
                        isinstance(duration, (int, float)) and abs(duration - 228) <= 1)
                    first = response.get('firstPcmFromCommandMs')
                    row['eligible'] = (response.get('ok') is True and row['metadataMatched'] and
                        response.get('clientName') == 'MWEB' and response.get('accountingComplete') is True and
                        response.get('formatId') == 251 and response.get('codec') == CODEC and
                        response.get('decodedAudioMs', 0) >= 5000 and isinstance(first, (int, float)) and
                        math.isfinite(first) and first >= 0 and
                        response.get('playbackPreparation', {}).get('mode') == arm)
                    row['status'] = 'completed'
                except (TimeoutError, EOFError, ProtocolError, OSError, subprocess.SubprocessError) as error:
                    row.update({'status': 'driver_failed', 'failure': {
                        'TimeoutError': 'DRIVER_DEADLINE', 'EOFError': 'WORKER_EOF',
                        'ProtocolError': 'PROTOCOL_ERROR'}.get(type(error).__name__, 'PROCESS_ERROR')})
                finally:
                    row['processLaunchToResponseMs'] = round((time.monotonic() - attempt_started) * 1000, 3)
                    if proc is not None:
                        terminate_group(proc)
                        proc.stdin.close()
                        proc.stdout.close()
                    row['cleanupCompleted'] = True
                rows.append(row)
                checkpoint('running')
    report = checkpoint('completed')
    if hashes() != initial_hashes:
        report['status'] = 'source_changed_during_run'
        report['qualification'] = False
        report['pairs'] = [{**pair, 'eligible': False, 'parallelMinusSequentialFirstPcmMs': None}
                           for pair in report['pairs']]
        output.write_text(json.dumps(report, indent=2) + '\n')
    return report

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = run(args.output)
    print(json.dumps({'status': result['status'], 'requests': len(result['requests']),
                      'eligiblePairs': sum(p['eligible'] for p in result['pairs'])}))
