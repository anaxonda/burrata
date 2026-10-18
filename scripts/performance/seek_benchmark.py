#!/usr/bin/env python3
"""Compare fresh and retained native decoders on generated five-minute audio.

This measures decoder-to-first-PCM, not audible application seek latency.
The ignored Rust benchmark uses 20 repetitions at 1%, 50% and 95% per strategy.
"""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
FORMATS = {'wav': ['-c:a', 'pcm_s16le'], 'flac': ['-c:a', 'flac'],
           'mp3': ['-c:a', 'libmp3lame', '-b:a', '192k'],
           'm4a': ['-c:a', 'aac', '-b:a', '192k'],
           'opus': ['-c:a', 'libopus', '-b:a', '128k']}


def summarize(rows):
    summaries = []
    for codec, strategy, fraction in sorted({(r['codec'], r['strategy'], r['fraction']) for r in rows}):
        group = [r for r in rows if (r['codec'], r['strategy'], r['fraction']) == (codec, strategy, fraction)]
        summary = dict(codec=codec, strategy=strategy, fraction=fraction, attempts=len(group))
        for key in ['firstPcmMs', 'decodeThreadCpuMs', 'decoderChildCpuMs', 'wallMs']:
            # The forced FFmpeg comparison row is subprocess-only; its child CPU
            # is recorded separately. Retained Opus CPU belongs to the worker.
            if strategy == 'ffmpegFresh' and key == 'decodeThreadCpuMs':
                continue
            values = sorted(r[key] for r in group if isinstance(r.get(key), (int, float)))
            if not values:
                continue
            summary[key] = dict(median=statistics.median(values), p95=values[math.ceil(.95 * len(values))-1], worst=values[-1])
        summaries.append(summary)
    return summaries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--formats', nargs='+', choices=FORMATS, default=list(FORMATS))
    parser.add_argument('--experimental-opus', action='store_true',
                        help='compile and run with the optional experimental Opus decoder')
    parser.add_argument('--compare-opus', action='store_true',
                        help='compare default FFmpeg fresh seeks with retained experimental Opus seeks')
    args = parser.parse_args()
    if args.compare_opus and not args.experimental_opus:
        parser.error('--compare-opus requires --experimental-opus')
    if args.compare_opus and args.formats != ['opus']:
        parser.error('--compare-opus requires --formats opus')
    args.output.mkdir(parents=True, exist_ok=True)
    rows, failures = [], []
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    features = ['experimental-opus'] if args.experimental_opus else []
    metadata = dict(profile=args.profile, features=features, decoderComparison=args.compare_opus,
                    system=platform.system(), architecture=platform.machine(),
                    revision=revision, dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
                    rustc=subprocess.check_output(['rustc','--version'], text=True).strip(),
                    ffmpeg=subprocess.check_output(['ffmpeg','-version'], text=True).splitlines()[0],
                    cargoLockSha256=hashlib.sha256((ROOT/'src-tauri/Cargo.lock').read_bytes()).hexdigest(),
                    nativeOutputSha256=hashlib.sha256((ROOT/'src-tauri/src/native_output.rs').read_bytes()).hexdigest(),
                    durationSeconds=300, warmupsPerPath=3 if args.compare_opus else 1,
                    repetitionsPerPosition=20, fractions=[.01, .5, .95],
                    scope='decoder first PCM; excludes resolver, mixer, device and renderer',
                    cpuScope='decode worker thread; reaped FFmpeg child CPU separately on Linux', failures=failures)
    with tempfile.TemporaryDirectory(prefix='burrata-seek-bench-') as scratch:
        directory = Path(scratch)
        wav = directory / 'sine.wav'
        subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i',
                        'sine=frequency=440:sample_rate=48000:duration=300', '-ac', '2',
                        '-c:a', 'pcm_s16le', '-y', str(wav)], check=True)
        for codec in args.formats:
            fixture = wav if codec == 'wav' else directory / f'sine.{codec}'
            result = directory / f'{codec}.json'
            try:
                if codec != 'wav':
                    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-i', str(wav),
                                    *FORMATS[codec], '-y', str(fixture)], check=True)
                env = dict(os.environ, BURRATA_SEEK_BENCH_FIXTURE=str(fixture),
                           BURRATA_SEEK_BENCH_OUTPUT=str(result), BURRATA_SEEK_BENCH_DURATION='300',
                           TAURI_CONFIG='{"bundle":{"resources":[]}}')
                if args.compare_opus:
                    env['BURRATA_SEEK_BENCH_COMPARE_OPUS'] = '1'
                command = ['cargo', 'test', '--manifest-path', str(ROOT / 'src-tauri/Cargo.toml'), '--lib']
                if args.experimental_opus:
                    command += ['--features', 'experimental-opus']
                if args.profile == 'release':
                    command.append('--release')
                command += ['seek_decoder_benchmark', '--', '--ignored', '--test-threads=1']
                subprocess.run(command, cwd=ROOT, env=env, check=True, timeout=1800)
                rows += [dict(row, codec=codec) for row in json.loads(result.read_text())]
            except (subprocess.SubprocessError, OSError, ValueError):
                failures.append(dict(codec=codec, failure='fixture_or_benchmark_failed'))
            (args.output / 'attempts.json').write_text(json.dumps(rows, indent=2) + '\n')
            (args.output / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
    (args.output / 'summary.json').write_text(json.dumps(summarize(rows), indent=2) + '\n')
    if rows:
        with (args.output / 'attempts.csv').open('w', newline='') as output:
            writer = csv.DictWriter(output, fieldnames=sorted(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
    if failures:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
