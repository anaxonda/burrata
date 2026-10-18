#!/usr/bin/env python3
"""Sample one verified Burrata process and correlate its metadata-free phase logs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
SAFE_PHASE = re.compile(
    r'\[perf\] playlist_sync\.[a-z_]+'
    r'(?: (?:files|warnings|elapsed_ms|sources|migrated|repaired|collections_bound|count|pass|running|outcome)='
    r'(?:[0-9]+|true|false|ok|error|before_mirrors|after_mirrors))+'
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--vite-pid', type=int)
    parser.add_argument('--seconds', type=float, default=120)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    process = Path('/proc') / str(args.pid)
    if args.pid <= 0 or args.seconds <= 0 or (process / 'exe').resolve() != binary:
        parser.error('the exact Burrata executable must be running')
    # Guard PID reuse as well as a changed executable during the observation.
    start_ticks = (process / 'stat').read_text().rsplit(')', 1)[1].split()[19]
    log_path = Path(os.readlink(process / 'fd/1'))
    if log_path.name != 'app.log' or not str(log_path).startswith('/tmp/burrata-launch.'):
        parser.error('expected the verified launcher app log')
    args.output.mkdir(parents=True, exist_ok=True)
    command = [sys.executable, str(ROOT / 'scripts/performance/profile_process.py'),
               '--group', f'app={args.pid}', '--seconds', str(args.seconds),
               '--interval', '0.5', '--output', str(args.output / 'profile')]
    if args.vite_pid:
        command += ['--group', f'vite={args.vite_pid}']
    phases = []
    with log_path.open(errors='replace') as log, (args.output / 'sampler.log').open('w') as output:
        log.seek(0, 2)
        started = time.monotonic()
        sampler = subprocess.Popen(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT)
        try:
            while True:
                line = log.readline()
                if line:
                    if SAFE_PHASE.fullmatch(line.rstrip()):
                        phases.append({'observedAtSeconds': round(time.monotonic() - started, 3),
                                       'line': line.rstrip()})
                    continue
                if sampler.poll() is not None:
                    break
                time.sleep(0.05)
            if sampler.wait() != 0:
                raise RuntimeError('process sampler failed; see sampler.log')
        finally:
            if sampler.poll() is None:
                sampler.terminate()
                sampler.wait(timeout=10)
    if ((process / 'exe').resolve() != binary or
            (process / 'stat').read_text().rsplit(')', 1)[1].split()[19] != start_ticks):
        raise RuntimeError('the sampled process changed')
    report = {'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'durationSeconds': args.seconds, 'phases': phases,
              'limitations': ['Phase observation times are polling times, not application timestamps.',
                              'This observes only the chosen process and its descendants.',
                              'Group RSS can double-count shared pages; IO counters are storage, not network.']}
    (args.output / 'phase-observations.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'complete': True, 'phaseLines': len(phases),
                      'binarySha256': report['binarySha256']}))


if __name__ == '__main__':
    main()
