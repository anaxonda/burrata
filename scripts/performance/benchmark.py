#!/usr/bin/env python3
"""Run a playback adapter in isolated data directories; retain failures and redact output.

Manifest: [{"id":"local-opus", "mode":"cold", "command":["adapter", "--fixture", "..."]}]
Adapter stdout must be one JSON object containing firstAudioMs, decodedFrames,
continuedPlaybackMs and correctMatch. Optional numeric fields: readerReadyMs,
decoderReadyMs, searchMs, extractionMs, retries, bufferingCount, transferredBytes.
Adapters must respect XDG_* and BURRATA_BENCH_DATA_DIR; never use production profiles.
"""
import argparse
import csv
import json
import math
import os
from pathlib import Path
import random
import signal
import statistics
import subprocess
import tempfile
import time
import uuid

try:
    import resource
except ImportError:  # Windows adapters can supply process metrics separately.
    resource = None

METRICS = ('cpuSeconds', 'firstAudioMs', 'readerReadyMs', 'decoderReadyMs', 'searchMs',
           'extractionMs', 'retries', 'bufferingCount', 'transferredBytes',
           'appCpuSeconds', 'webviewCpuSeconds', 'helperCpuSeconds',
           'peakRssBytes', 'readBytes', 'writeBytes', 'ipcRequestBytes', 'ipcResponseBytes')


def summarize(rows):
    result = []
    for case in sorted({r['case'] for r in rows}):
        group = [r for r in rows if r['case'] == case]
        summary = {'case': case, 'attempts': len(group),
                   'failures': sum(not r['success'] for r in group),
                   'incorrectMatches': sum(r.get('correctMatch') is False for r in group)}
        for metric in METRICS:
            values = sorted(r[metric] for r in group if isinstance(r.get(metric), (int, float)))
            if values:
                summary[metric] = {'median': statistics.median(values),
                                   'p95': values[math.ceil(.95 * len(values)) - 1],
                                   'worst': values[-1]}
        result.append(summary)
    return result


def run_adapter(command, env, profile, timeout):
    with subprocess.Popen(command, env=env, cwd=profile, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True,
                          start_new_session=os.name == 'posix') as process:
        try:
            stdout, _ = process.communicate(timeout=timeout)
            return process.returncode, stdout
        except subprocess.TimeoutExpired:
            if os.name == 'posix':
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.communicate()
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--seed', type=int, default=42)
    parser.add_argument('--interval', type=float, default=2)
    parser.add_argument('--timeout', type=float, default=45)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error('repetitions must be positive')
    cases = json.loads(args.manifest.read_text())
    order = [(case, repetition) for case in cases for repetition in range(args.repetitions)]
    random.Random(args.seed).shuffle(order)
    rows = []
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='burrata-bench-') as scratch:
        for index, (case, repetition) in enumerate(order):
            # Warm/replay adapters retain their private case directory. Cold gets a fresh one.
            key = f'{index}' if case['mode'] == 'cold' else str(cases.index(case))
            profile = Path(scratch) / case['mode'] / key
            profile.mkdir(parents=True, exist_ok=True)
            env = dict(os.environ, BURRATA_BENCH_DATA_DIR=str(profile),
                       XDG_DATA_HOME=str(profile / 'data'), XDG_CONFIG_HOME=str(profile / 'config'),
                       XDG_CACHE_HOME=str(profile / 'cache'))
            operation_id = str(uuid.uuid4())
            env['BURRATA_BENCH_OPERATION_ID'] = operation_id
            row = {'case': case['id'], 'mode': case['mode'], 'repetition': repetition,
                   'operationId': operation_id, 'success': False}
            cpu_before = resource.getrusage(resource.RUSAGE_CHILDREN) if resource else None
            started = time.monotonic()
            try:
                returncode, stdout = run_adapter(case['command'], env, profile, args.timeout)
                # Store only approved numeric measurements, never URLs, stderr or credentials.
                data = json.loads(stdout)
                if not isinstance(data, dict):
                    raise ValueError('adapter must return an object')
                for field in (*METRICS, 'decodedFrames', 'continuedPlaybackMs'):
                    value = data.get(field)
                    if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0:
                        row[field] = value
                # Preserve only an exact echo; this ties adapter evidence to the harness
                # attempt without asserting that the application propagated it over IPC.
                if data.get('operationId') == operation_id:
                    row['adapterOperationId'] = operation_id
                row['correctMatch'] = data.get('correctMatch') is True
                row['success'] = (returncode == 0 and row['correctMatch']
                                  and row.get('decodedFrames', 0) > 0
                                  and row.get('continuedPlaybackMs', 0) >= 5000
                                  and 'firstAudioMs' in row)
                row['failure'] = '' if row['success'] else 'adapter_or_playback_validation'
            except subprocess.TimeoutExpired:
                row['failure'] = 'timeout'
            except (ValueError, OSError, TypeError):
                row['failure'] = 'adapter_protocol'
            if cpu_before is not None:
                cpu_after = resource.getrusage(resource.RUSAGE_CHILDREN)
                row['cpuSeconds'] = max(0, cpu_after.ru_utime + cpu_after.ru_stime - cpu_before.ru_utime - cpu_before.ru_stime)
            row['wallMs'] = round((time.monotonic() - started) * 1000, 3)
            rows.append(row)
            (args.output / 'attempts.json').write_text(json.dumps(rows, indent=2) + '\n')
            if index + 1 < len(order):
                time.sleep(max(0, args.interval))
    (args.output / 'summary.json').write_text(json.dumps(summarize(rows), indent=2) + '\n')
    with (args.output / 'attempts.csv').open('w') as output:
        writer = csv.DictWriter(output, fieldnames=sorted({key for row in rows for key in row}))
        writer.writeheader()
        writer.writerows(rows)


if __name__ == '__main__':
    main()
